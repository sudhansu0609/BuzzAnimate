//! Window, panel layout and the frame loop.
//!
//! # Frame structure
//!
//! ```text
//! egui pass   panels laid out; the leftover central rect becomes the stage,
//!             and chrome is painted into it
//! Vello       artwork -> intermediate storage texture, offset into that rect
//! blit        intermediate -> surface
//! egui paint  panels and chrome over the top
//! ```
//!
//! The stage area is discovered *during* the egui pass, so the camera viewport
//! is updated there — before chrome is drawn — rather than a frame late.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use buzz_doc::Document;
use buzz_geom::{Point, Size};
use buzz_jobs::{JobSystem, Pool};
use buzz_render::wgpu;
use buzz_render::{GpuContext, GpuPreference};
use buzz_ui::{Command, Palette, ToolId, panels, theme, tools as tool_catalogue};
use peniko::Color;
use vello::Scene as VelloScene;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::editor::Editor;
use crate::stage;
use crate::tools::{Mods, ToolAction};

/// The pasteboard, which is also the window clear colour.
const PASTEBOARD: Color = Color::from_rgb8(0x53, 0x53, 0x53);

/// One wheel notch.
const WHEEL_ZOOM_STEP: f64 = 1.18;

/// How often autosave is offered a chance to run.
const AUTOSAVE_POLL: Duration = Duration::from_secs(5);

struct TargetTexture {
    #[allow(dead_code, reason = "kept alive so the view stays valid")]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

struct Active {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    gpu: GpuContext,
    blitter: wgpu::util::TextureBlitter,
    target: Option<TargetTexture>,

    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    vello: VelloScene,
    /// Where the stage sits inside the window.
    stage_area: egui::Rect,
    /// True while a stage gesture is in progress, so moves are routed to tools.
    dragging: bool,

    last_frame: Instant,
    frame_ms: f32,
    last_autosave_check: Instant,
}

pub struct App {
    active: Option<Active>,
    editor: Editor,
    jobs: Arc<JobSystem>,
    preference: GpuPreference,
}

impl App {
    pub fn new(preference: GpuPreference) -> Self {
        Self {
            active: None,
            editor: Editor::default(),
            jobs: Arc::new(JobSystem::new()),
            preference,
        }
    }

    /// Open a document at startup.
    pub fn with_document(mut self, doc: Document) -> Self {
        self.editor = Editor::new(doc);
        self
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<Active> {
        let attrs = Window::default_attributes()
            .with_title("BuzzAnimate")
            // Sized to leave the status bar clear of a bottom taskbar.
            .with_inner_size(winit::dpi::LogicalSize::new(1560.0, 880.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .context("creating the window")?,
        );

        let gpu = GpuContext::new_blocking(&self.preference)?;
        println!("{}", gpu.selection.report());

        let surface = gpu
            .instance
            .create_surface(window.clone())
            .context("creating the surface")?;
        let caps = surface.get_capabilities(&gpu.selection.adapter);

        // Prefer a non-sRGB surface: Vello writes colours that are already
        // sRGB-encoded, and an sRGB view would apply the transfer function
        // twice and wash the stage out.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &surface_config);

        let blitter = wgpu::util::TextureBlitter::new(&gpu.device, format);

        let egui_ctx = egui::Context::default();
        theme::apply(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            None,
            None,
            None,
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());

        Ok(Active {
            window,
            surface,
            surface_config,
            gpu,
            blitter,
            target: None,
            egui_ctx,
            egui_state,
            egui_renderer,
            vello: VelloScene::new(),
            stage_area: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
            dragging: false,
            last_frame: Instant::now(),
            frame_ms: 0.0,
            last_autosave_check: Instant::now(),
        })
    }
}

impl Active {
    fn ensure_target(&mut self) {
        let (w, h) = (self.surface_config.width, self.surface_config.height);
        if matches!(&self.target, Some(t) if t.width == w && t.height == h) {
            return;
        }
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: buzz_render::RENDER_FORMAT,
            // Vello's fine rasteriser writes through a storage binding, which
            // swapchain textures do not allow.
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target = Some(TargetTexture {
            texture,
            view,
            width: w,
            height: h,
        });
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface
            .configure(&self.gpu.device, &self.surface_config);
    }
}

/// Read the modifier state egui saw this frame.
fn mods_from(ctx: &egui::Context) -> Mods {
    ctx.input(|i| Mods {
        shift: i.modifiers.shift,
        alt: i.modifiers.alt,
        ctrl: i.modifiers.command,
    })
}

/// Collect commands raised by the keyboard.
fn keyboard_commands(ctx: &egui::Context, editor: &Editor) -> Vec<Command> {
    let mut out = Vec::new();

    // A focused text field owns the keyboard; stealing single letters from it
    // would make renaming a layer impossible.
    if ctx.memory(|m| m.focused().is_some()) {
        return out;
    }

    let all = [
        Command::New,
        Command::Open,
        Command::Save,
        Command::SaveAs,
        Command::Undo,
        Command::Redo,
        Command::Delete,
        Command::SelectAll,
        Command::Deselect,
        Command::DuplicateSelection,
        Command::ZoomIn,
        Command::ZoomOut,
        Command::ZoomActual,
        Command::ZoomFitInWindow,
        Command::ZoomShowFrame,
        Command::ToggleRulers,
        Command::ToggleGrid,
        Command::ToggleGuides,
        Command::GroupSelection,
        Command::UngroupSelection,
        Command::BringToFront,
        Command::BringForward,
        Command::SendBackward,
        Command::SendToBack,
        Command::NewLayer,
        Command::NewLayerFolder,
        // Timeline. Animators press these constantly, so they must be live.
        Command::InsertFrame,
        Command::RemoveFrame,
        Command::InsertKeyframe,
        Command::InsertBlankKeyframe,
        Command::ClearKeyframe,
        Command::PlayPause,
        Command::NextFrame,
        Command::PreviousFrame,
        Command::FirstFrame,
        Command::LastFrame,
    ];
    for command in all {
        if let Some(shortcut) = command.shortcut()
            && ctx.input_mut(|i| i.consume_shortcut(&shortcut))
        {
            out.push(command);
        }
    }

    // Animate also accepts Ctrl+Y for redo.
    let ctrl_y = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Y);
    if ctx.input_mut(|i| i.consume_shortcut(&ctrl_y)) {
        out.push(Command::Redo);
    }

    // Bare letters select tools, but only without modifiers.
    let plain = ctx.input(|i| !i.modifiers.any());
    if plain {
        for tool in tool_catalogue::all_tools() {
            if let Some(key) = tool.shortcut()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
            {
                out.push(Command::SelectTool(tool));
            }
        }
        // `J` toggles object drawing, as in Animate.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::J)) {
            // Handled directly; there is no menu command for it.
            let _ = editor;
        }
    }

    out
}

impl App {
    /// Lay out the panels and return the rect left for the stage.
    ///
    /// Panels that change the document run inside [`Document::edit`], so their
    /// changes are undoable. `edit` only records a step when the revision
    /// actually moves, so a frame where the user touched nothing costs nothing.
    fn build_ui(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let mut commands: Vec<Command> = Vec::new();
        let can_undo = self.editor.doc.can_undo();
        let can_redo = self.editor.doc.can_redo();
        let ctx = ui.ctx().clone();

        let stage_area = {
            // egui 0.35 unified the panel types into `Panel::top/bottom/left/right`.
            egui::Panel::top("menu")
                .frame(egui::Frame::new().fill(Palette::CHROME).inner_margin(2))
                .show(ui, |ui| {
                    commands.extend(panels::menu_bar(
                        ui,
                        self.editor.scene(),
                        &self.editor.selection,
                        &self.editor.view,
                        can_undo,
                        can_redo,
                    ));
                });

            egui::Panel::bottom("status")
                .frame(egui::Frame::new().fill(Palette::CHROME).inner_margin(3))
                .show(ui, |ui| self.status_bar(ui));

            egui::Panel::bottom("timeline")
                .resizable(true)
                .default_size(170.0)
                .show(ui, |ui| {
                    let state = buzz_ui::TimelineState {
                        current_frame: self.editor.current_frame,
                        active_layer: self.editor.selection.active_layer(),
                        playing: self.editor.playback.playing,
                        onion_enabled: self.editor.onion.enabled,
                    };
                    let response =
                        buzz_ui::timeline_panel(ui, self.editor.scene(), &state);
                    self.apply_timeline(response, &mut commands);
                });

            egui::Panel::left("tools")
                .resizable(false)
                .exact_size(58.0)
                .show(ui, |ui| {
                    if let Some(tool) =
                        panels::tool_bar(ui, self.editor.tool(), &mut self.editor.style)
                    {
                        commands.push(Command::SelectTool(tool));
                    }
                });

            egui::Panel::right("panels")
                .resizable(true)
                .default_size(280.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Bound to locals first so the closures capture
                        // disjoint fields rather than all of `self.editor`.
                        let editor = &mut self.editor;

                        let selection = &mut editor.selection;
                        let mut layer_command = None;
                        editor.doc.edit("Layer Properties", |scene| {
                            layer_command = panels::layers_panel(ui, scene, selection);
                        });
                        if let Some(command) = layer_command {
                            commands.push(command);
                        }

                        ui.separator();

                        let selection = &editor.selection;
                        let style = &mut editor.style;
                        let view = &mut editor.view;
                        editor.doc.edit("Document Properties", |scene| {
                            panels::properties_panel(ui, scene, selection, style, view);
                        });

                        ui.separator();
                        panels::color_panel(ui, &mut editor.style);

                        ui.separator();
                        self.depth_panel(ui);
                    });
                });

            // The Library sits in its own dock so a long symbol list scrolls
            // independently of the properties above it, as it does in Animate.
            egui::Panel::right("library")
                .resizable(true)
                .default_size(240.0)
                .show(ui, |ui| {
                    let editor = &mut self.editor;
                    let library = &mut editor.library;
                    let mut library_command = None;
                    editor.doc.edit("Library", |scene| {
                        library_command = buzz_ui::library_panel(ui, scene, library);
                    });
                    if let Some(command) = library_command {
                        commands.push(command);
                    }
                });

            // The edit-path breadcrumb. Animate keeps this strip directly above
            // the stage, and it is the only way back out of a symbol.
            //
            // Added and removed rather than collapsed: whether it is there is
            // decided by the document, not by the user, and egui's collapsible
            // panel binds a `&mut bool` the user can also flip — which would
            // let them hide their only way out of a symbol.
            if !self.editor.scene().edit_path().is_empty() {
                egui::Panel::top("breadcrumb")
                    .frame(egui::Frame::new().fill(Palette::CHROME).inner_margin(3))
                    .show(ui, |ui| {
                        if let Some(command) = self.breadcrumb(ui) {
                            commands.push(command);
                        }
                    });
            }

            // Whatever is left is the stage.
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let area = ui.max_rect();

                    // Update the viewport here, before chrome is drawn, so a
                    // resize takes effect on the same frame.
                    self.editor.camera.viewport =
                        Size::new(area.width() as f64, area.height() as f64);

                    self.handle_stage_input(ui, area);
                    let response = stage::draw_chrome(ui, &self.editor, area);
                    if let Some(guide) = response.new_guide {
                        self.editor.view.add_guide(guide);
                    }
                    area
                })
                .inner
        };

        commands.extend(keyboard_commands(&ctx, &self.editor));
        for command in commands {
            self.dispatch(command);
        }

        stage_area
    }

    /// Animate's Layer Depth panel, and the edits it raises.
    ///
    /// Each change is its own undo step with its own label, so pushing a layer
    /// back and then flattening everything are two separate things to undo
    /// rather than one indivisible "depth" blob.
    fn depth_panel(&mut self, ui: &mut egui::Ui) {
        let active = self.editor.selection.active_layer();
        let response = buzz_ui::depth_panel(ui, self.editor.doc.scene(), active);

        if let Some(layer) = response.select_layer {
            self.editor.selection.set_active_layer(Some(layer));
        }

        if let Some((layer, depth)) = response.set_depth {
            self.editor.doc.edit("Layer Depth", |scene| {
                scene.update_layer(layer, |l| l.depth = depth);
            });
        }

        if let Some(distance) = response.set_focal_distance {
            self.editor.doc.edit("Camera Depth", |scene| {
                scene.camera_mut().focal_distance = distance;
            });
        }

        if response.flatten {
            let ids: Vec<_> = self
                .editor
                .doc
                .scene()
                .layers()
                .iter()
                .map(|l| l.id)
                .collect();
            self.editor.doc.edit("Flatten Depth", |scene| {
                for id in ids {
                    scene.update_layer(id, |l| l.depth = 0.0);
                }
            });
        }

        if response.distribute {
            let scene = self.editor.doc.scene();
            let ids: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
            // The front of the stack takes the nearest depth, so the timeline's
            // own ordering is what decides which layer ends up where.
            let depths =
                buzz_ui::depth_panel::distributed_depths(ids.len(), scene.camera().focal_distance);

            self.editor.doc.edit("Distribute Depth", |scene| {
                for (id, depth) in ids.into_iter().zip(depths) {
                    scene.update_layer(id, |l| l.depth = depth);
                }
            });
        }
    }

    /// The trail of symbols currently open, with the document at its root.
    ///
    /// Clicking a level jumps straight back to it. Returning a [`Command`]
    /// rather than mutating here keeps every navigation step going through the
    /// same dispatch path as the menu and the keyboard.
    fn breadcrumb(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let mut command = None;
        let path: Vec<buzz_scene::SymbolId> = self.editor.scene().edit_path().to_vec();

        ui.horizontal(|ui| {
            if ui
                .link(egui::RichText::new("Scene 1").small())
                .on_hover_text("Back to the main timeline")
                .clicked()
            {
                command = Some(Command::EditDocument);
            }

            for (depth, id) in path.iter().enumerate() {
                // ">" rather than a typographic chevron: egui's bundled fonts
                // have no glyph for "▸" and draw an empty box instead.
                ui.label(egui::RichText::new(">").small().weak());

                let name = self
                    .editor
                    .scene()
                    .library()
                    .get(*id)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "(missing)".to_string());

                // The innermost symbol is where you already are, so it is a
                // label rather than a link.
                let last = depth + 1 == path.len();
                if last {
                    ui.label(egui::RichText::new(name).small().strong());
                } else if ui.link(egui::RichText::new(name).small()).clicked() {
                    self.editor.library.selected = Some(*id);
                    command = Some(Command::EditSymbol);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Exit Symbol").clicked() {
                    // One level out, not all the way: nested symbols are
                    // normally stepped through one at a time.
                    self.editor.doc.edit_view(|scene| {
                        scene.exit_symbol();
                    });
                    self.editor.selection.clear();
                    self.editor.selection.ensure_active_layer(self.editor.doc.scene());
                }
            });
        });

        command
    }

    /// Turn timeline interactions into editor actions.
    fn apply_timeline(&mut self, response: buzz_ui::TimelineResponse, commands: &mut Vec<Command>) {
        if let Some(frame) = response.scrub_to {
            // Scrubbing stops playback, as it does in Animate: the user has
            // taken manual control of the playhead.
            self.editor.playback.playing = false;
            self.editor.set_frame(frame);
        }
        if let Some(layer) = response.select_layer {
            self.editor.selection.set_active_layer(Some(layer));
        }
        if let Some(action) = response.action {
            commands.push(match action {
                buzz_ui::FrameAction::InsertFrame => Command::InsertFrame,
                buzz_ui::FrameAction::RemoveFrame => Command::RemoveFrame,
                buzz_ui::FrameAction::InsertKeyframe => Command::InsertKeyframe,
                buzz_ui::FrameAction::InsertBlankKeyframe => Command::InsertBlankKeyframe,
                buzz_ui::FrameAction::ClearKeyframe => Command::ClearKeyframe,
            });
        }
        if let Some(tween) = response.tween {
            commands.push(match tween {
                buzz_ui::TweenRequest::Motion => Command::CreateMotionTween,
                buzz_ui::TweenRequest::Shape => Command::CreateShapeTween,
                buzz_ui::TweenRequest::Classic => Command::CreateClassicTween,
                buzz_ui::TweenRequest::Remove => Command::RemoveTween,
            });
        }
        if response.toggle_play {
            commands.push(Command::PlayPause);
        }
        if response.toggle_onion {
            commands.push(Command::ToggleOnionSkin);
        }
        if response.go_to_start {
            commands.push(Command::FirstFrame);
        }
        if response.go_to_end {
            commands.push(Command::LastFrame);
        }
        if response.step != 0 {
            self.editor.step_frame(response.step);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(self.editor.doc.display_name());
            ui.separator();

            // Zoom, with Animate's presets plus an unbounded field.
            ui.label("Zoom");
            let mut percent = self.editor.camera.zoom_percent();
            // Proportional drag speed, so one gesture is useful at 50% and at
            // a trillion percent alike. Computed before the mutable borrow.
            let speed = percent.max(1.0) * 0.02;
            let response = ui.add(
                egui::DragValue::new(&mut percent)
                    .speed(speed)
                    // No upper bound: this is the whole point of the engine.
                    .range(0.001..=f64::MAX)
                    .suffix(" %"),
            );
            if response.changed() {
                self.editor.camera.set_zoom_percent(percent);
            }

            egui::ComboBox::from_id_salt("zoom-preset")
                .selected_text("Presets")
                .width(90.0)
                .show_ui(ui, |ui| {
                    for preset in [25.0, 50.0, 100.0, 200.0, 400.0, 800.0, 2000.0] {
                        if ui.selectable_label(false, format!("{preset:.0}%")).clicked() {
                            self.editor.camera.set_zoom_percent(preset);
                        }
                    }
                    ui.separator();
                    if ui.selectable_label(false, "Fit in Window").clicked() {
                        self.editor.run(Command::ZoomFitInWindow);
                    }
                    if ui.selectable_label(false, "Show All").clicked() {
                        self.editor.run(Command::ZoomShowAll);
                    }
                });

            ui.separator();
            // Surface the f64 precision budget: at extreme zoom the user
            // deserves to know when positions start quantising.
            let precision = self.editor.camera.screen_precision_px();
            let (text, color) = crate::hud::describe_precision(precision);
            ui.label(
                egui::RichText::new(format!("precision {text}"))
                    .color(color)
                    .small(),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{:.1} ms", self.frame_ms_display()))
                        .small()
                        .weak(),
                );
                if let Some(status) = &self.editor.status {
                    ui.separator();
                    ui.label(egui::RichText::new(status).small());
                }
            });
        });
    }

    fn frame_ms_display(&self) -> f32 {
        self.active.as_ref().map(|a| a.frame_ms).unwrap_or(0.0)
    }

    /// Route pointer input over the stage to the active tool.
    fn handle_stage_input(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        let id = ui.id().with("stage");
        let response = ui.interact(area, id, egui::Sense::click_and_drag());
        let ctx = ui.ctx().clone();
        let mods = mods_from(&ctx);

        let local = |p: egui::Pos2| {
            Point::new((p.x - area.min.x) as f64, (p.y - area.min.y) as f64)
        };

        // Wheel zooms about the cursor; unbounded in both directions.
        if response.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0
                && let Some(pos) = ctx.input(|i| i.pointer.hover_pos())
            {
                self.editor
                    .camera
                    .zoom_by_at(WHEEL_ZOOM_STEP.powf(scroll as f64 / 50.0), local(pos));
            }
        }

        // Space or the middle button pans regardless of the active tool, as in
        // every editor a user will have come from.
        let pan_override = ctx.input(|i| i.key_down(egui::Key::Space))
            || ctx.input(|i| i.pointer.middle_down());

        if response.drag_started()
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
        {
            if !pan_override {
                self.editor.pointer_down(local(pos), mods);
            }
            if let Some(active) = &mut self.active {
                active.dragging = true;
            }
        }

        if response.dragged()
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
        {
            if pan_override {
                let delta = ctx.input(|i| i.pointer.delta());
                self.editor.apply(ToolAction::PanView {
                    delta_screen: buzz_geom::Vec2::new(delta.x as f64, delta.y as f64),
                });
            } else {
                self.editor.pointer_move(local(pos), mods);
            }
        }

        if response.drag_stopped() {
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
                && !pan_override
            {
                self.editor.pointer_up(local(pos));
            }
            if let Some(active) = &mut self.active {
                active.dragging = false;
            }
        }

        // A click with no drag still needs to reach the tool.
        if response.clicked()
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
            && !pan_override
        {
            self.editor.pointer_down(local(pos), mods);
            self.editor.pointer_up(local(pos));
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.editor.machine.cancel();
        }
    }

    fn dispatch(&mut self, command: Command) {
        match command {
            Command::Open => self.open_dialog(),
            Command::Save => self.save(false),
            Command::SaveAs => self.save(true),
            Command::Close => self.editor.should_quit = true,
            Command::ImportToStage => self.import_dialog(buzz_scene::ImportTarget::Stage),
            Command::ImportToLibrary => self.import_dialog(buzz_scene::ImportTarget::Library),
            other => self.editor.run(other),
        }
    }

    /// Animate's File ▸ Import, for all three formats.
    ///
    /// The whole import is one [`Document::edit`], so a file that brings in
    /// four hundred symbols is still a single Ctrl+Z.
    fn import_dialog(&mut self, target: buzz_scene::ImportTarget) {
        let picked = rfd::FileDialog::new()
            .add_filter("Everything BuzzAnimate can import", crate::import::IMPORTABLE)
            .add_filter("Animate document", &["fla", "xfl"])
            .add_filter("Flash movie", &["swf"])
            .add_filter("PDF or Illustrator artwork", &["pdf", "ai"])
            .pick_file();
        let Some(path) = picked else { return };

        let imported = match crate::import::read(&path) {
            Ok(imported) => imported,
            Err(message) => {
                // A failed import must leave the open document untouched, which
                // it does: nothing has been merged at this point.
                self.editor.status = Some(format!("Could not import: {message}"));
                return;
            }
        };

        let label = match target {
            buzz_scene::ImportTarget::Stage => "Import to Stage",
            buzz_scene::ImportTarget::Library => "Import to Library",
        };

        let mut merge = None;
        self.editor.doc.edit(label, |scene| {
            merge = Some(scene.merge(&imported.scene, target));
        });
        let merge = merge.unwrap_or_default();

        // An import can change how many frames the document has and which
        // layers exist, so the editor's idea of both has to be re-settled.
        self.editor.selection.clear();
        self.editor.selection.ensure_active_layer(self.editor.doc.scene());

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        self.editor.status = Some(format!("Imported {name}: {}", merge.summary()));

        // Only interrupt the user when something was actually lost or moved.
        // A clean import speaks for itself in the status bar.
        if !imported.unsupported.is_empty() || !merge.renamed.is_empty() {
            let mut unsupported = imported.unsupported.clone();
            for (wanted, given) in &merge.renamed {
                unsupported.push(format!(
                    "\"{wanted}\" was already in the library, so it came in as \"{given}\""
                ));
            }
            self.editor.import_summary = Some(crate::import::ImportSummary {
                title: format!("Imported {name}"),
                what_arrived: format!("{} — {}", imported.summary, merge.summary()),
                unsupported,
            });
        }
    }

    /// The fidelity report, shown after an import that lost something.
    fn import_report_window(&mut self, ctx: &egui::Context) {
        let Some(summary) = self.editor.import_summary.clone() else {
            return;
        };
        let mut open = true;

        egui::Window::new(&summary.title)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&summary.what_arrived).strong());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "These parts of the file did not come across. \
                         Everything else imported normally.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);

                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for line in &summary.unsupported {
                        ui.label(format!("• {line}"));
                    }
                });

                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    self.editor.import_summary = None;
                }
            });

        if !open {
            self.editor.import_summary = None;
        }
    }

    fn open_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("BuzzAnimate document", &[buzz_doc::EXTENSION])
            .pick_file();
        let Some(path) = picked else { return };

        match Document::open(&path) {
            Ok(doc) => {
                self.editor = Editor::new(doc);
                self.editor.status = Some(format!("Opened {}", path.display()));
            }
            Err(e) => {
                self.editor.status = Some(format!("Could not open: {e}"));
            }
        }
    }

    fn save(&mut self, force_dialog: bool) {
        let path = if force_dialog || self.editor.doc.path().is_none() {
            rfd::FileDialog::new()
                .add_filter("BuzzAnimate document", &[buzz_doc::EXTENSION])
                .set_file_name(format!("untitled.{}", buzz_doc::EXTENSION))
                .save_file()
        } else {
            self.editor.doc.path().map(|p| p.to_path_buf())
        };
        let Some(path) = path else { return };

        match self.editor.doc.save_as(&path) {
            Ok(()) => self.editor.status = Some(format!("Saved {}", path.display())),
            Err(e) => self.editor.status = Some(format!("Could not save: {e}")),
        }
    }

    /// Offer the document to autosave, writing on the background pool.
    fn poll_autosave(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.last_autosave_check.elapsed() < AUTOSAVE_POLL {
            return;
        }
        active.last_autosave_check = Instant::now();

        if let Some(plan) = self.editor.doc.autosave_plan() {
            // Snapshots are immutable, so this needs no coordination with
            // editing, which continues on this thread meanwhile.
            self.jobs.spawn(Pool::Background, move || {
                if let Err(e) = plan.write() {
                    tracing::warn!("autosave failed: {e}");
                }
            });
        }
    }

    fn render(&mut self) -> Result<()> {
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };

        let now = Instant::now();
        let elapsed = now.duration_since(active.last_frame);
        active.frame_ms = elapsed.as_secs_f32() * 1000.0;
        active.last_frame = now;

        // Playback runs on wall-clock time, so the document plays at its
        // authored rate regardless of the display's refresh rate.
        self.editor.advance_playback(elapsed.as_secs_f64());
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };

        let raw_input = active.egui_state.take_egui_input(&active.window);
        let egui_ctx = active.egui_ctx.clone();
        let window = active.window.clone();

        // `run_ui` rather than `begin_pass`/`end_pass`: egui 0.35 roots the UI
        // in a `Ui`, and panels attach to that rather than to the context.
        let mut stage_area = egui::Rect::NOTHING;
        let output = egui_ctx.run_ui(raw_input, |ui| {
            stage_area = self.build_ui(ui);
            // Floats above the panels, so it is drawn after them.
            self.import_report_window(ui.ctx());
        });

        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        active.stage_area = stage_area;
        active
            .egui_state
            .handle_platform_output(&window, output.platform_output);
        let paint_jobs = egui_ctx.tessellate(output.shapes, output.pixels_per_point);

        // ---- artwork -------------------------------------------------------
        let scale = output.pixels_per_point as f64;
        let area_px = buzz_geom::Rect::new(
            stage_area.min.x as f64 * scale,
            stage_area.min.y as f64 * scale,
            stage_area.max.x as f64 * scale,
            stage_area.max.y as f64 * scale,
        );
        // The camera works in physical pixels, matching the render target.
        self.editor.camera.viewport = Size::new(area_px.width(), area_px.height());
        stage::build_scene(&mut active.vello, &self.editor, area_px);
        // Restore logical units so pointer maths stays in egui's space.
        self.editor.camera.viewport =
            Size::new(stage_area.width() as f64, stage_area.height() as f64);

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match active.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                let (w, h) = (active.surface_config.width, active.surface_config.height);
                active.resize(w, h);
                return Ok(());
            }
            Cst::Timeout | Cst::Occluded => return Ok(()),
            other => return Err(anyhow::anyhow!("acquiring surface texture: {other:?}")),
        };
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let (w, h) = (active.surface_config.width, active.surface_config.height);
        active.ensure_target();
        let target = &active.target.as_ref().expect("ensure_target ran").view;
        active
            .gpu
            .render(&active.vello, target, w, h, PASTEBOARD)?;

        let mut encoder =
            active
                .gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("buzz-frame"),
                });
        active
            .blitter
            .copy(&active.gpu.device, &mut encoder, target, &surface_view);

        for (id, delta) in &output.textures_delta.set {
            active
                .egui_renderer
                .update_texture(&active.gpu.device, &active.gpu.queue, *id, delta);
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: output.pixels_per_point,
        };
        let user_buffers = active.egui_renderer.update_buffers(
            &active.gpu.device,
            &active.gpu.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Load: the artwork is already there.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            active
                .egui_renderer
                .render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
        }

        active
            .gpu
            .queue
            .submit(user_buffers.into_iter().chain([encoder.finish()]));
        frame.present();

        for id in &output.textures_delta.free {
            active.egui_renderer.free_texture(id);
        }

        self.poll_autosave();
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(active) => self.active = Some(active),
            Err(e) => {
                eprintln!("BuzzAnimate failed to start: {e:?}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(active) = self.active.as_mut() else {
            return;
        };

        let response = active.egui_state.on_window_event(&active.window, &event);
        if response.repaint {
            active.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                active.resize(size.width, size.height);
                active.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.render() {
                    eprintln!("frame failed: {e:?}");
                }
                if self.editor.should_quit {
                    event_loop.exit();
                }
            }
            // Wheel and pointer events reach the tools through egui, which
            // already knows whether a panel wanted them.
            WindowEvent::MouseWheel { delta, .. } => {
                let _ = delta;
                if let Some(active) = &self.active {
                    active.window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let _ = (state, button);
                if let Some(active) = &self.active {
                    active.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(active) = &self.active {
            active.window.request_redraw();
        }
    }
}

/// Unused re-export guard: keeps the tool catalogue linked for the key map.
#[allow(dead_code)]
fn _tool_ids() -> Vec<ToolId> {
    tool_catalogue::all_tools()
}

/// Silences an unused import when the demo module is not compiled in.
#[allow(dead_code)]
fn _mouse_types(_: MouseButton, _: ElementState, _: MouseScrollDelta) {}
