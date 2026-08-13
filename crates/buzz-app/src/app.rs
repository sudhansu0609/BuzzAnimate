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
    /// An export writing frames on its own thread, if one is running.
    export: Option<crate::export_job::ExportJob>,
    /// Lighting geometry kept between frames.
    ///
    /// The renderer is stateless — a `SceneBuilder` lives for one frame — so
    /// geometry that cost a boolean to build has to be owned out here, by
    /// something that outlives frames.
    lights: buzz_render::document::DrawCache,
}

impl App {
    pub fn new(preference: GpuPreference) -> Self {
        Self {
            active: None,
            editor: Editor::default(),
            jobs: Arc::new(JobSystem::new()),
            preference,
            export: None,
            lights: buzz_render::document::DrawCache::new(),
        }
    }

    /// Open a document at startup.
    pub fn with_document(mut self, doc: Document) -> Self {
        self.editor = Editor::new(doc);
        self
    }

    /// Load a script into the Actions panel and run it, as Animate's command
    /// line runs a JSFL file.
    ///
    /// The panel is opened with the script still in it, so what ran is on
    /// screen and can be corrected and run again — a script that executed
    /// invisibly and left no trace of itself would be very hard to debug.
    /// Returns whatever it traced, for printing to the terminal as well.
    pub fn with_script(mut self, source: String) -> Self {
        self.editor.actions.source = source;
        self.editor.run(Command::RunScript);
        self
    }

    /// What the last run said, for the terminal.
    pub fn script_report(&self) -> (&[String], Option<&str>) {
        (
            &self.editor.actions.output,
            self.editor.actions.error.as_deref(),
        )
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

/// Change one bone of one armature.
fn update_bone(
    scene: &mut buzz_scene::Scene,
    object: buzz_scene::ObjectId,
    bone: usize,
    change: impl FnOnce(&mut buzz_rig::Bone),
) {
    scene.update_object(object, |target| {
        if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind
            && let Some(bone) = rig.armature.bones.get_mut(bone)
        {
            change(bone);
        }
    });
}

/// The scripting shortcuts, which fire *even while a text field has focus*.
///
/// Every other shortcut is suppressed when something is being typed into —
/// otherwise `V` would switch tools instead of typing a letter. Ctrl+Enter is
/// the exception that proves the rule: it means "run this", and the moment it
/// is wanted is precisely when the caret is sitting in the code box.
///
/// Consumed before any panel is drawn, so the code editor never sees the
/// keystroke and cannot insert a newline as well as running the script.
fn script_shortcuts(ctx: &egui::Context) -> Vec<Command> {
    let mut out = Vec::new();
    for command in [Command::ToggleActionsPanel, Command::RunScript] {
        if let Some(shortcut) = command.shortcut()
            && ctx.input_mut(|i| i.consume_shortcut(&shortcut))
        {
            out.push(command);
        }
    }
    out
}

/// The built-in sample scripts, in the shape the panel takes them.
///
/// They live in `buzz-script`, where a test runs every one of them against a
/// document — sample code that no longer works is worse than none.
fn script_samples() -> Vec<buzz_ui::SampleEntry> {
    buzz_script::samples()
        .iter()
        .map(|s| buzz_ui::SampleEntry {
            name: s.name,
            summary: s.summary,
            source: s.source,
        })
        .collect()
}

/// Commands the keyboard raises.
///
/// **This list is what actually binds a key.** A command can have a
/// shortcut in the command map, printed beside it in a menu, and still do
/// nothing when pressed — which is exactly what had happened to F8 and
/// Ctrl+E. `every_shortcut_is_reachable_from_the_keyboard` guards it now.
const KEYBOARD_COMMANDS: &[Command] = &[
    Command::New,
    Command::Open,
    Command::Save,
    Command::SaveAs,
    // Dispatched by the shell, which owns the file dialogs — but raised
    // from here, like everything else with a key.
    Command::Close,
    Command::Quit,
    Command::ImportToStage,
    Command::ImportToLibrary,
    Command::Undo,
    Command::Redo,
    Command::Cut,
    Command::Copy,
    Command::Paste,
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
    // Symbols. **F8 is the most-pressed key in a symbol-based workflow**
    // and Ctrl+E is how you get inside what it made: both were in the
    // command map and in the menus, with their shortcuts printed beside
    // them, and neither did anything from the keyboard — the list here is
    // what actually binds a key, and they had never been added to it.
    Command::ConvertToSymbol,
    Command::NewSymbol,
    Command::EditSymbol,
    Command::EditDocument,
    // The remaining View toggles, for the same reason.
    Command::ToggleSnapping,
    Command::TogglePasteboard,
    Command::ToggleLayoutLock,
    Command::ZoomShowAll,
    Command::ToggleLightGizmos,
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

/// Collect commands raised by the keyboard.
fn keyboard_commands(ctx: &egui::Context, editor: &Editor) -> Vec<Command> {
    let mut out = Vec::new();

    // A focused text field owns the keyboard; stealing single letters from it
    // would make renaming a layer impossible.
    if ctx.memory(|m| m.focused().is_some()) {
        return out;
    }

    let all = KEYBOARD_COMMANDS;
    for &command in all {
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

        // Decode any sound the document has gained — on open, on import, on
        // undo. `refresh` compares the document's revision and does nothing
        // when nothing has changed, so this costs a comparison per frame and
        // means a document that *has* sound shows its waveform and can be
        // played without first being asked to.
        let scene = self.editor.doc.scene().clone();
        self.editor.sound.refresh(&scene);
        // Taken before the panels are built, so the code editor never sees the
        // keystroke that ran it.
        commands.extend(script_shortcuts(&ctx));

        let stage_area = {
            // egui 0.35 unified the panel types into `Panel::top/bottom/left/right`.
            egui::Panel::top("menu")
                .frame(egui::Frame::new().fill(Palette::CHROME).inner_margin(2))
                .show(ui, |ui| {
                    commands.extend(panels::menu_bar(
                        ui,
                        &panels::MenuState {
                            scene: self.editor.scene(),
                            selection: &self.editor.selection,
                            view: &self.editor.view,
                            can_undo,
                            can_redo,
                            workspace: &self.editor.workspace,
                            light_gizmos: self.editor.light_panel.gizmos,
                        },
                    ));
                });

            egui::Panel::bottom("status")
                .frame(egui::Frame::new().fill(Palette::CHROME).inner_margin(3))
                .show(ui, |ui| self.status_bar(ui));

            // Every panel is placed by the workspace rather than nailed to a
            // side here, which is what makes the layout the user's to arrange.
            // Requested moves are collected and applied after the frame: a
            // panel cannot be moved while it is being drawn.
            let mut moves: Vec<(buzz_ui::PanelId, buzz_ui::Dock)> = Vec::new();
            let mut reorders: Vec<(buzz_ui::PanelId, i32)> = Vec::new();
            let workspace = self.editor.workspace.clone();
            let locked = workspace.locked;

            // Bottom first: `egui` gives each side to whichever panel asks
            // first, so the order here is the order down the window.
            for id in workspace.on(buzz_ui::Dock::Bottom) {
                let height = if id == buzz_ui::PanelId::Timeline {
                    workspace.bottom_height
                } else {
                    240.0
                };
                let response = egui::Panel::bottom(egui::Id::new(("dock-bottom", id)))
                    .resizable(!locked)
                    .default_size(height)
                    .show(ui, |ui| {
                        if let Some(dock) = panel_header(ui, id, locked, !id.draws_own_title(), &mut reorders) {
                            moves.push((id, dock));
                        }
                        self.draw_panel(ui, id, &mut commands);
                    });
                if id == buzz_ui::PanelId::Timeline {
                    self.editor.workspace.bottom_height = response.response.rect.height();
                }
            }

            for (dock, id_name, width) in [
                (buzz_ui::Dock::Left, "dock-left", workspace.left_width),
            ] {
                let panels = workspace.on(dock);
                if panels.is_empty() {
                    continue;
                }
                let response = egui::Panel::left(id_name)
                    .resizable(!locked)
                    .default_size(width)
                    .show(ui, |ui| {
                        self.draw_column(ui, &panels, locked, &mut moves, &mut reorders, &mut commands);
                    });
                self.editor.workspace.left_width = response.response.rect.width();
            }

            for (dock, id_name, width) in [
                (buzz_ui::Dock::RightOuter, "dock-right-outer", workspace.right_outer_width),
                (buzz_ui::Dock::Right, "dock-right", workspace.right_width),
            ] {
                let panels = workspace.on(dock);
                if panels.is_empty() {
                    continue;
                }
                let response = egui::Panel::right(id_name)
                    .resizable(!locked)
                    .default_size(width)
                    .show(ui, |ui| {
                        self.draw_column(ui, &panels, locked, &mut moves, &mut reorders, &mut commands);
                    });
                let measured = response.response.rect.width();
                if dock == buzz_ui::Dock::Right {
                    self.editor.workspace.right_width = measured;
                } else {
                    self.editor.workspace.right_outer_width = measured;
                }
            }

            // Floating panels are windows over the stage, movable and resizable
            // unless the layout is locked.
            for id in workspace.on(buzz_ui::Dock::Float) {
                let slot = workspace.slot(id).copied();
                let mut open = true;
                let response = egui::Window::new(id.title())
                    .id(egui::Id::new(("float", id)))
                    .open(&mut open)
                    .movable(!locked)
                    .resizable(!locked)
                    .default_pos(slot.map(|s| s.float_pos).unwrap_or((320.0, 140.0)))
                    .default_size(slot.map(|s| s.float_size).unwrap_or((300.0, 380.0)))
                    .show(ui.ctx(), |ui| {
                        // Never named here: the window frame already carries
                        // the title, and the panel below may carry it again.
                        if let Some(dock) = panel_header(ui, id, locked, false, &mut reorders) {
                            moves.push((id, dock));
                        }
                        egui::ScrollArea::vertical()
                            .id_salt(("float-scroll", id))
                            .show(ui, |ui| self.draw_panel(ui, id, &mut commands));
                    });
                if let Some(response) = response
                    && let Some(slot) = self.editor.workspace.slot_mut(id)
                {
                    let rect = response.response.rect;
                    slot.float_pos = (rect.min.x, rect.min.y);
                    slot.float_size = (rect.width(), rect.height());
                }
                if !open {
                    moves.push((id, buzz_ui::Dock::Hidden));
                }
            }

            for (id, dock) in moves {
                self.editor.workspace.move_to(id, dock);
                self.editor.workspace.save();
            }
            for (id, delta) in reorders {
                self.editor.workspace.reorder(id, delta);
                self.editor.workspace.save();
            }

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

        // An export runs on its own thread; this is where what it has done
        // reaches the screen. The repaint request is what keeps the progress
        // bar moving on a document that is otherwise still.
        self.poll_export();
        if self.export.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.export_dialog(&ctx);
        self.lip_sync_dialog(&ctx);

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


    /// Draw a column of docked panels, each with its own header.
    fn draw_column(
        &mut self,
        ui: &mut egui::Ui,
        panels: &[buzz_ui::PanelId],
        locked: bool,
        moves: &mut Vec<(buzz_ui::PanelId, buzz_ui::Dock)>,
        reorders: &mut Vec<(buzz_ui::PanelId, i32)>,
        commands: &mut Vec<Command>,
    ) {
        egui::ScrollArea::vertical()
            .id_salt(("column", panels.first().copied()))
            .show(ui, |ui| {
                for (index, id) in panels.iter().copied().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    if let Some(dock) = panel_header(ui, id, locked, !id.draws_own_title(), reorders) {
                        moves.push((id, dock));
                    }
                    self.draw_panel(ui, id, commands);
                }
            });
    }

    /// One panel's contents.
    ///
    /// Every panel is reachable from here by its id, which is what lets the
    /// workspace put it anywhere: the layout decides *where*, this decides
    /// *what*, and neither knows about the other.
    fn draw_panel(&mut self, ui: &mut egui::Ui, id: buzz_ui::PanelId, commands: &mut Vec<Command>) {
        use buzz_ui::PanelId::*;
        match id {
            Tools => {
                if let Some(tool) =
                    panels::tool_bar(ui, self.editor.tool(), &mut self.editor.style)
                {
                    commands.push(Command::SelectTool(tool));
                }
            }

            Layers => {
                let editor = &mut self.editor;
                let selection = &mut editor.selection;
                let mut raised = None;
                editor.doc.edit("Layer Properties", |scene| {
                    raised = panels::layers_panel(ui, scene, selection);
                });
                if let Some(command) = raised {
                    commands.push(command);
                }
            }

            Properties => {
                let editor = &mut self.editor;
                let selection = &editor.selection;
                let style = &mut editor.style;
                let view = &mut editor.view;
                editor.doc.edit("Document Properties", |scene| {
                    panels::properties_panel(ui, scene, selection, style, view);
                });
            }

            Color => panels::color_panel(ui, &mut self.editor.style),
            Depth => self.depth_panel(ui),
            Rig => self.rig_panel(ui),
            Filters => self.filter_panel(ui),
            Lighting => self.light_panel(ui),

            Library => {
                let editor = &mut self.editor;
                let library = &mut editor.library;
                let mut raised = None;
                editor.doc.edit("Library", |scene| {
                    raised = buzz_ui::library_panel(ui, scene, library);
                });
                if let Some(command) = raised {
                    commands.push(command);
                }
            }

            Timeline => {
                let state = buzz_ui::TimelineState {
                    current_frame: self.editor.current_frame,
                    active_layer: self.editor.selection.active_layer(),
                    camera_selected: self.editor.camera_selected,
                    playing: self.editor.playback.playing,
                    onion_enabled: self.editor.onion.enabled,
                    waveforms: self.editor.waveforms(),
                };
                let response = buzz_ui::timeline_panel(ui, self.editor.scene(), &state);
                self.apply_timeline(response, commands);
            }

            Actions => {
                let response =
                    buzz_ui::actions_panel(ui, &mut self.editor.actions, &script_samples());
                // Ctrl+Enter may already have raised it this frame; clicking
                // Run as well should not run it twice.
                if response.run && !commands.contains(&Command::RunScript) {
                    commands.push(Command::RunScript);
                }
            }
        }
    }

    /// Animate's Filters panel, and the edits it raises.
    ///
    /// Each change is its own undo step: adding a glow and then softening it
    /// are two decisions, and an animator who regrets one should not lose both.
    fn filter_panel(&mut self, ui: &mut egui::Ui) {
        let editor = &mut self.editor;

        // What the panel is editing: the selected object, or the active layer.
        let object = editor.selection.iter().next();
        let layer = editor.selection.active_layer();
        let target = editor.filter_panel.target;

        let (filters, blend) = match target {
            buzz_ui::FilterTarget::Object => match object.and_then(|id| editor.scene().find_object(id))
            {
                Some((_, found)) => (found.filters.clone(), Some(found.blend)),
                None => (Vec::new(), None),
            },
            buzz_ui::FilterTarget::Layer => (
                layer
                    .and_then(|id| editor.scene().layers().get(id))
                    .map(|l| l.filters.clone())
                    .unwrap_or_default(),
                None,
            ),
        };

        let response = buzz_ui::filter_panel(
            ui,
            &filters,
            blend,
            &mut editor.filter_panel,
            object.is_some(),
        );

        // Where an edit lands. Both arms write the same `Vec`, so the whole
        // panel is one code path with one place that decides the target.
        let apply = |editor: &mut Editor, label: &'static str, change: &dyn Fn(&mut Vec<buzz_scene::Filter>)| {
            match target {
                buzz_ui::FilterTarget::Object => {
                    let Some(id) = object else { return };
                    editor.doc.edit(label, |scene| {
                        scene.update_object(id, |o| change(&mut o.filters));
                    });
                }
                buzz_ui::FilterTarget::Layer => {
                    let Some(id) = layer else { return };
                    editor.doc.edit(label, |scene| {
                        scene.update_layer(id, |l| change(&mut l.filters));
                    });
                }
            }
        };

        if let Some(kind) = response.add {
            let label = kind.label();
            apply(editor, "Add Filter", &move |list| {
                list.push(buzz_scene::Filter::new(kind.clone()));
            });
            editor.filter_panel.selected = filters.len();
            editor.status = Some(format!("Added a {}", label.to_lowercase()));
        }

        if let Some(index) = response.remove {
            apply(editor, "Delete Filter", &move |list| {
                if index < list.len() {
                    list.remove(index);
                }
            });
            editor.filter_panel.selected = editor.filter_panel.selected.saturating_sub(1);
        }

        if let Some((index, filter)) = response.changed {
            apply(editor, "Filter Settings", &move |list| {
                if let Some(slot) = list.get_mut(index) {
                    *slot = filter.clone();
                }
            });
        }

        if let Some((index, direction)) = response.reorder {
            let to = index as i32 + direction;
            apply(editor, "Reorder Filters", &move |list| {
                if to >= 0 && (to as usize) < list.len() {
                    list.swap(index, to as usize);
                }
            });
            if to >= 0 {
                editor.filter_panel.selected = to as usize;
            }
        }

        if let Some(blend) = response.set_blend
            && let Some(id) = object
        {
            editor.doc.edit("Blend Mode", |scene| {
                scene.update_object(id, |o| o.blend = blend);
            });
        }
    }

    /// The Lighting panel, and the edits it raises.
    ///
    /// Each change is its own undo step with its own label, so warming a key
    /// light and then swinging it are two things to undo rather than one.
    fn light_panel(&mut self, ui: &mut egui::Ui) {
        let editor = &mut self.editor;
        let state = &mut editor.light_panel;
        let response = buzz_ui::light_panel(ui, editor.doc.scene().lights(), state);

        if let Some(id) = response.select {
            editor.light_panel.selected = Some(id);
        }

        if let Some(kind) = response.add {
            // The same path the Insert menu takes, so a light added from the
            // panel and one added from the menu behave identically.
            editor.add_light(kind);
        }

        if let Some(id) = response.remove {
            editor.doc.edit("Delete Light", |scene| {
                scene.lights_mut().remove(id);
            });
            if editor.light_panel.selected == Some(id) {
                editor.light_panel.selected = None;
            }
        }

        if let Some(light) = response.changed {
            editor.doc.edit("Light Settings", |scene| {
                if let Some(target) = scene.lights_mut().get_mut(light.id) {
                    *target = light;
                }
            });
        }

        if let Some(enabled) = response.set_enabled {
            editor.doc.edit("Lighting", |scene| {
                scene.lights_mut().enabled = enabled;
            });
        }

        if let Some(base) = response.set_base {
            editor.doc.edit("Fill Light", |scene| {
                scene.lights_mut().base = base;
            });
        }

        if let Some(modelling) = response.set_modelling {
            editor.doc.edit("Modelling", |scene| {
                scene.lights_mut().modelling = modelling;
            });
        }
    }

    /// The Armature panel, and the edits it raises.
    ///
    /// Each change is its own undo step: limiting a joint and pinning it are
    /// two decisions, and an animator who regrets one should not lose both.
    fn rig_panel(&mut self, ui: &mut egui::Ui) {
        // The selected armature, if the selection is one. Cloned because the
        // panel is drawn while the document is borrowed mutably below.
        let selected = self.editor.selection.iter().next();
        let armature = selected
            .and_then(|id| self.editor.scene().find_object(id))
            .and_then(|(_, object)| match &object.kind {
                buzz_scene::ObjectKind::Armature(rig) => Some(rig.armature.clone()),
                _ => None,
            });

        let response = buzz_ui::rig_panel(ui, armature.as_ref());
        let Some(object) = selected else { return };

        if let Some((bone, limits)) = response.set_limits {
            self.editor.doc.edit("Joint Limits", |scene| {
                update_bone(scene, object, bone, |b| {
                    b.limits = limits.map(|(min, max)| buzz_rig::JointLimits::new(min, max));
                });
            });
        }

        if let Some((bone, pinned)) = response.set_pinned {
            self.editor.doc.edit("Pin Joint", |scene| {
                update_bone(scene, object, bone, |b| b.pinned = pinned);
            });
        }

        if let Some((bone, name)) = response.rename {
            self.editor.doc.edit("Rename Bone", |scene| {
                update_bone(scene, object, bone, |b| b.name = name);
            });
        }

        if response.reset_pose {
            self.editor.doc.edit("Reset Pose", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                        let rest = rig.armature.at_rest().pose();
                        rig.armature.set_pose(&rest);
                    }
                });
            });
        }

        if response.set_rest_pose {
            // Adopting a new rest pose changes what the weights were bound
            // against, so the artwork has to be re-bound — otherwise it would
            // keep bending about the old skeleton.
            self.editor.doc.edit("Set Rest Pose", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                        rig.armature.set_rest_here();
                        rig.rebind();
                    }
                });
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
                    self.editor
                        .selection
                        .ensure_active_layer(self.editor.doc.scene());
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
            // Clicking a layer takes the camera row's highlight away, as
            // clicking any row takes it from the one before.
            self.editor.camera_selected = false;
        }
        if response.select_camera {
            self.editor.camera_selected = true;
            // Selecting the camera row selects the Camera tool, which is what
            // Animate does — the row and the tool are the same idea.
            commands.push(Command::SelectTool(ToolId::Camera));
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
                        if ui
                            .selectable_label(false, format!("{preset:.0}%"))
                            .clicked()
                        {
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

        let local =
            |p: egui::Pos2| Point::new((p.x - area.min.x) as f64, (p.y - area.min.y) as f64);

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
        let pan_override =
            ctx.input(|i| i.key_down(egui::Key::Space)) || ctx.input(|i| i.pointer.middle_down());

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
            Command::ImportSound => self.import_sound_dialog(),
            Command::LipSync => self.open_lip_sync(),
            Command::ExportImage => self.open_export(buzz_ui::ExportKind::Image),
            Command::ExportSequence => self.open_export(buzz_ui::ExportKind::Sequence),
            Command::Open => self.open_dialog(),
            Command::Save => self.save(false),
            Command::SaveAs => self.save(true),
            Command::Close => self.editor.should_quit = true,
            Command::ImportToStage => self.import_dialog(buzz_scene::ImportTarget::Stage),
            Command::ImportToLibrary => self.import_dialog(buzz_scene::ImportTarget::Library),
            other => self.editor.run(other),
        }
    }

    /// Open the Export dialog, sized to the document as it is now.
    fn open_export(&mut self, kind: buzz_ui::ExportKind) {
        if self.export.is_some() {
            self.editor.status = Some("An export is already running".into());
            return;
        }
        let scene = self.editor.scene();
        let size = scene.stage().size;
        self.editor.export.open(
            kind,
            (
                size.width.round().max(1.0) as u32,
                size.height.round().max(1.0) as u32,
            ),
            scene.frame_count(),
        );
    }

    /// Draw the Export dialog and act on what the user chose.
    fn export_dialog(&mut self, ctx: &egui::Context) {
        let response = buzz_ui::export_dialog(ctx, &mut self.editor.export);

        if response.cancelled
            && let Some(job) = &self.export
        {
            job.cancel();
            self.editor.status = Some("Stopping the export…".into());
        }
        if !response.confirmed {
            return;
        }

        let Some(kind) = self.editor.export.open else {
            return;
        };
        let settings = buzz_export::ExportSettings {
            width: self.editor.export.width,
            height: self.editor.export.height,
            transparent: self.editor.export.transparent,
        };
        // A snapshot, so the export renders the document as it was when the
        // user asked — and they can keep editing while it writes.
        let scene = self.editor.scene().clone();
        let stem = self
            .editor
            .doc
            .path()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".to_string());

        let job = match kind {
            buzz_ui::ExportKind::Image => {
                let frame = self.editor.current_frame;
                let picked = rfd::FileDialog::new()
                    .add_filter("PNG image", &["png"])
                    .set_file_name(format!("{stem}-{frame:04}.png"))
                    .save_file();
                let Some(path) = picked else { return };

                crate::export_job::ExportJob::image(
                    scene,
                    frame,
                    path,
                    settings,
                    self.preference.clone(),
                )
            }
            buzz_ui::ExportKind::Sequence => {
                // A folder, not a file: a sequence is many files, and asking
                // for one file name would leave the user guessing what the
                // rest were called.
                let picked = rfd::FileDialog::new()
                    .set_title("Choose a folder for the sequence")
                    .pick_folder();
                let Some(directory) = picked else { return };

                crate::export_job::ExportJob::sequence(
                    scene,
                    self.editor.export.range(),
                    directory,
                    stem,
                    settings,
                    self.preference.clone(),
                )
            }
        };

        self.editor.export.progress = Some(job.progress);
        self.export = Some(job);
    }

    /// Animate's File ▸ Import Sound.
    fn import_sound_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Sound", &["wav", "mp3", "ogg", "flac", "m4a", "aac"])
            .pick_file();
        let Some(path) = picked else { return };

        match self.editor.import_sound(&path) {
            Ok(name) => {
                self.editor.status = Some(format!(
                    "Imported {name} — put it on a keyframe with Control > Attach Sound to Frame"
                ))
            }
            Err(e) => self.editor.status = Some(format!("Could not import that sound: {e:#}")),
        }
    }

    /// Open the Lip Sync dialog.
    fn open_lip_sync(&mut self) {
        self.editor.lip_sync = buzz_ui::LipSyncState::opened();
        // Pre-select the layer being worked on: nine times in ten that is the
        // mouth layer, because it is the one the animator just clicked.
        self.editor.lip_sync.layer = self.editor.selection.active_layer().map(|l| l.0);
    }

    /// Draw the Lip Sync dialog and act on it.
    fn lip_sync_dialog(&mut self, ctx: &egui::Context) {
        if !self.editor.lip_sync.open {
            return;
        }

        let (track, mouths, layers) = self.editor.lip_sync_choices();
        let response = buzz_ui::lip_sync_dialog(
            ctx,
            &mut self.editor.lip_sync,
            track.as_deref(),
            &mouths,
            &layers,
        );

        if response.make_mouth {
            let symbol = self.editor.new_mouth_symbol();
            self.editor.lip_sync.mouth = Some(symbol.0);
        }
        if response.confirmed {
            self.editor.run_lip_sync();
        }
    }

    /// Take whatever the exporting thread has said since the last frame.
    fn poll_export(&mut self) {
        let Some(job) = &mut self.export else { return };

        let finished = job.poll();
        self.editor.export.progress = Some(job.progress);

        if let Some(message) = finished {
            self.editor.status = Some(message);
            self.editor.export.close();
            self.export = None;
        }
    }

    /// Animate's File ▸ Import, for all three formats.
    ///
    /// The whole import is one [`Document::edit`], so a file that brings in
    /// four hundred symbols is still a single Ctrl+Z.
    fn import_dialog(&mut self, target: buzz_scene::ImportTarget) {
        let picked = rfd::FileDialog::new()
            .add_filter(
                "Everything BuzzAnimate can import",
                crate::import::IMPORTABLE,
            )
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
        self.editor
            .selection
            .ensure_active_layer(self.editor.doc.scene());

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

                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
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
        stage::build_scene(&mut active.vello, &self.editor, area_px, &mut self.lights);
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
        active.gpu.render(&active.vello, target, w, h, PASTEBOARD)?;

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

/// The strip at the top of every panel: its name, and the menu that moves it.
///
/// Animate puts the same menu behind the ≡ button in each panel's corner. It is
/// drawn here rather than inside each panel because a panel should not have to
/// know it is dockable — every one of them was written before this existed, and
/// none of them needed changing.
///
/// Returns the side the user asked for, if they asked for one.
fn panel_header(
    ui: &mut egui::Ui,
    id: buzz_ui::PanelId,
    locked: bool,
    named: bool,
    reorders: &mut Vec<(buzz_ui::PanelId, i32)>,
) -> Option<buzz_ui::Dock> {
    let mut moved = None;

    ui.horizontal(|ui| {
        // Only the panels with no heading of their own are named here. The
        // rest would read their name twice — which is exactly how the first
        // version looked.
        if named {
            ui.label(
                egui::RichText::new(id.title())
                    .small()
                    .color(Palette::TEXT_DIM),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Three bars, drawn as text: this one *is* in egui's bundled font,
            // unlike most symbols — and it is checked by a test rather than
            // assumed, because that assumption has been wrong twice.
            ui.menu_button(egui::RichText::new(PANEL_MENU).small(), |ui| {
                if locked {
                    ui.label(
                        egui::RichText::new("The layout is locked")
                            .small()
                            .weak(),
                    );
                    ui.separator();
                }

                for dock in buzz_ui::Dock::CHOICES {
                    if ui
                        .add_enabled(
                            !locked || dock == buzz_ui::Dock::Hidden,
                            egui::Button::new(dock.label()),
                        )
                        .clicked()
                    {
                        moved = Some(dock);
                        ui.close();
                    }
                }

                ui.separator();
                for (label, delta) in [("Move Up", -1), ("Move Down", 1)] {
                    if ui.add_enabled(!locked, egui::Button::new(label)).clicked() {
                        reorders.push((id, delta));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Move, float or close this panel");
        });
    });

    moved
}

/// The panel menu's label.
///
/// Three dots, not the hamburger every docking interface uses: that
/// character has **no glyph** in egui's bundled font and would draw as an
/// empty box. `theme::font_has` said so before it reached a screenshot,
/// which is the whole reason that check exists.
const PANEL_MENU: &str = "...";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command that advertises a shortcut must actually be bound.
    ///
    /// A shortcut printed in a menu that does nothing is worse than no
    /// shortcut: the user learns it, presses it, and quietly loses the habit.
    /// F8 and Ctrl+E — Convert to Symbol and Edit Symbol, the two most-pressed
    /// keys in a symbol workflow — were in the map, in the menus, and bound to
    /// nothing at all.
    #[test]
    fn every_shortcut_is_reachable_from_the_keyboard() {
        // The exceptions, each for a stated reason.
        let handled_elsewhere = |command: Command| {
            matches!(
                command,
                // Consumed before the panels are drawn, and while a text field
                // has focus, by `script_shortcuts`.
                Command::RunScript | Command::ToggleActionsPanel
                    // Raised by the timeline's own transport buttons and by the
                    // frame grid, which have the frame under the pointer.
                    | Command::PlayPause
            )
        };

        for command in buzz_ui::command::all_with_shortcuts() {
            if handled_elsewhere(command) {
                continue;
            }
            assert!(
                KEYBOARD_COMMANDS.contains(&command),
                "{command:?} has the shortcut {:?} but nothing binds it",
                command.shortcut()
            );
        }
    }

    /// And nothing is bound twice, which would fire it twice per press.
    #[test]
    fn no_command_is_bound_twice() {
        let mut seen = std::collections::BTreeSet::new();
        for command in KEYBOARD_COMMANDS {
            assert!(
                seen.insert(format!("{command:?}")),
                "{command:?} appears twice in the keyboard list"
            );
        }
    }
}
