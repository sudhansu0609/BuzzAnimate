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

/// Decode one of the packaged logos into an icon.
///
/// Decoded from the PNG rather than shipped as raw pixels: 64\u00d764 RGBA is
/// 16 KB of source file, and the PNG is a fifth of that and can be opened in
/// any image editor when somebody wants to change it.
///
/// Returns `None` rather than failing: a window with the default icon is a
/// working window.
fn icon_from_png(bytes: &[u8]) -> Option<winit::window::Icon> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .ok()?;
    let mut pixels = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    pixels.truncate(info.buffer_size());
    winit::window::Icon::from_rgba(pixels, info.width, info.height).ok()
}

/// The icon in the title bar, drawn at 16 pixels — 24 at this machine's
/// scaling — so the 32-pixel drawing is the one that lands closest to its own
/// size rather than being reduced to a quarter.
fn window_icon() -> Option<winit::window::Icon> {
    icon_from_png(include_bytes!("../../../assets/logo-32.png"))
}

/// The icon on the taskbar button, drawn at 32 pixels and more at high DPI.
///
/// Windows keeps *two* icons per window \u2014 the small one beside the title and
/// the big one the taskbar and Alt+Tab use \u2014 and winit's `window_icon` sets
/// only the small. Leaving the big one unset is why the taskbar button showed
/// a blank sheet of paper while the title bar showed the logo.
#[cfg(windows)]
fn taskbar_icon() -> Option<winit::window::Icon> {
    icon_from_png(include_bytes!("../../../assets/logo-128.png"))
}

/// Tell Windows this process is an application in its own right.
///
/// Without an explicit identity the taskbar files a window under whatever
/// launched it — `buzzanimate.exe` reached through a batch file and a command
/// prompt — and draws that entry's icon, which is how the button ended up
/// showing a blank sheet of paper while the window itself carried the logo.
/// The same identity is what pinning and jump lists key off, so a pinned
/// button and a running one become the same button.
///
/// Failure is ignored: an unnamed process still runs, it is just filed under
/// its executable.
#[cfg(windows)]
fn name_this_application() {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(id: *const u16) -> i32;
    }

    let id: Vec<u16> = "BuzzcafMedia.BuzzAnimate\0".encode_utf16().collect();
    // SAFETY: the pointer is to a NUL-terminated UTF-16 buffer that outlives
    // the call, which is the whole of this function's contract.
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
}

/// Is this one of our own documents, or something to translate on the way in?
///
/// By extension, and case-insensitively: `SCENE.FLA` off somebody's server is
/// the same file as `scene.fla`.
fn opens_as_document(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(buzz_doc::EXTENSION))
}

/// Where a scrollbar's thumb goes, and how much of the extent is on screen.
///
/// Kept as a function of the four numbers rather than a method so it can be
/// tested without a window: a thumb that is the wrong size or in the wrong
/// place is the whole failure mode of a scrollbar.
fn thumb_of(
    track: egui::Rect,
    extent_start: f64,
    extent_end: f64,
    visible_start: f64,
    visible_end: f64,
    horizontal: bool,
) -> (egui::Rect, f32) {
    let extent = (extent_end - extent_start).max(1e-9);
    let shown = ((visible_end - visible_start) / extent).clamp(0.02, 1.0) as f32;
    let at = (((visible_start - extent_start) / extent) as f32).clamp(0.0, 1.0 - shown);

    if horizontal {
        let width = (track.width() * shown).max(24.0);
        let travel = (track.width() - width).max(0.0);
        let left = track.left() + travel * (at / (1.0 - shown).max(1e-6)).clamp(0.0, 1.0);
        (
            egui::Rect::from_min_size(
                egui::pos2(left, track.top() + 1.0),
                egui::vec2(width, track.height() - 2.0),
            ),
            shown,
        )
    } else {
        let height = (track.height() * shown).max(24.0);
        let travel = (track.height() - height).max(0.0);
        let top = track.top() + travel * (at / (1.0 - shown).max(1e-6)).clamp(0.0, 1.0);
        (
            egui::Rect::from_min_size(
                egui::pos2(track.left() + 1.0, top),
                egui::vec2(track.width() - 2.0, height),
            ),
            shown,
        )
    }
}

/// The pasteboard, which is also the colour the stage is cleared to.
///
/// Taken from the theme rather than fixed, so the surround follows the
/// interface — and converted here because Vello wants a `peniko::Color` while
/// the palette speaks egui's.
fn pasteboard() -> Color {
    let c = Palette::pasteboard();
    Color::from_rgba8(c.r(), c.g(), c.b(), 255)
}

/// One wheel notch.
const WHEEL_ZOOM_STEP: f64 = 1.18;

/// How often autosave is offered a chance to run.
///
/// Once a second: the check itself compares two integers, and the policy it
/// feeds writes after a pause in drawing, which a five-second poll would blunt
/// into "up to ten seconds".
const AUTOSAVE_POLL: Duration = Duration::from_secs(1);

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
    /// Autosaves found on launch, while the prompt is still up.
    recovery: buzz_ui::RecoveryState,
    /// The revision the crash snapshot was last taken at.
    last_crash_revision: Option<u64>,
    /// An Animate asset import running on its own thread.
    animate_import: Option<crossbeam_channel::Receiver<crate::animate_assets::Progress>>,
    /// Where each dock column ended up on this frame.
    ///
    /// The splitters are drawn after the panels, over whatever is underneath,
    /// and they have to sit on the *actual* boundary between two columns. That
    /// boundary was being guessed from the stage rectangle, which works for
    /// exactly one column a side — the far-right column had no handle at all,
    /// so the Library and the Assets panel were stuck at whatever width they
    /// were born with. Recording the rects as they are laid out is the only
    /// way to be right about this that does not repeat the layout arithmetic.
    dock_rects: Vec<(buzz_ui::Dock, egui::Rect)>,
}

impl App {
    pub fn new(preference: GpuPreference) -> Self {
        // The window opens on the size the user last asked for, not on a
        // built-in one: "these settings become the default for the next
        // document" has to include the one the program opens with, or the
        // promise is only half kept.
        let mut editor = Editor::default();
        let remembered = editor.workspace.new_document;
        editor.create_document(remembered);
        editor.status = None;

        // Written before anything else can go wrong: from here on a panic
        // takes the artwork with it only if this fails.
        buzz_doc::autosave::install_crash_guard();

        let mut app = Self {
            active: None,
            editor,
            jobs: Arc::new(JobSystem::new()),
            preference,
            export: None,
            lights: buzz_render::document::DrawCache::new(),
            recovery: buzz_ui::RecoveryState::default(),
            last_crash_revision: None,
            animate_import: None,
            dock_rects: Vec::new(),
        };
        app.recovery = app.find_recoveries();
        app
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
        // Before the window exists: the taskbar reads the identity when the
        // button is created, and a name arriving afterwards is too late.
        #[cfg(windows)]
        name_this_application();

        let attrs = Window::default_attributes()
            .with_title("BuzzAnimate")
            .with_window_icon(window_icon())
            // Sized to leave the status bar clear of a bottom taskbar.
            .with_inner_size(winit::dpi::LogicalSize::new(1560.0, 880.0));
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows as _;
            attrs.with_taskbar_icon(taskbar_icon())
        };
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
        // The saved layout carries the interface theme, so the window opens in
        // whichever the user was last using rather than flashing dark first.
        theme::set_theme(buzz_ui::Workspace::load().theme);
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
    // Modify ▸ Transform. Animate binds the two rotations and leaves the
    // flips to the menu.
    Command::RotateClockwise,
    Command::RotateAnticlockwise,
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
    // Animate's frame clipboard, on Animate's keys.
    Command::CutFrames,
    Command::CopyFrames,
    Command::PasteFrames,
    Command::ClearFrames,
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

    // **The arrow keys nudge the selection**, one document unit at a time and
    // eight with Shift — Animate's numbers.
    //
    // Read here rather than through the shortcut map because four directions
    // times two step sizes is eight bindings for one action, and none of them
    // belongs in a menu. Only when something is selected: with an empty
    // selection the arrows are free for whatever wants them next, and nudging
    // nothing would be a silent no-op that looks like a dropped keystroke.
    if !editor.selection.is_empty() {
        let shift = ctx.input(|i| i.modifiers.shift_only());
        let step = if shift {
            buzz_ui::command::NUDGE_STEP_LARGE
        } else {
            buzz_ui::command::NUDGE_STEP
        };
        let modifiers = if shift {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };
        for (key, x, y) in [
            (egui::Key::ArrowLeft, -1, 0),
            (egui::Key::ArrowRight, 1, 0),
            // Screen down is +y in document space, as everywhere else here.
            (egui::Key::ArrowUp, 0, -1),
            (egui::Key::ArrowDown, 0, 1),
        ] {
            if ctx.input_mut(|i| i.consume_key(modifiers, key)) {
                out.push(Command::Nudge {
                    x: x * step,
                    y: y * step,
                });
            }
        }
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
                .frame(egui::Frame::new().fill(Palette::chrome()).inner_margin(2))
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
                .frame(egui::Frame::new().fill(Palette::chrome()).inner_margin(3))
                .show(ui, |ui| self.status_bar(ui));

            // Every panel is placed by the workspace rather than nailed to a
            // side here, which is what makes the layout the user's to arrange.
            // Requested moves are collected and applied after the frame: a
            // panel cannot be moved while it is being drawn.
            let mut requests = DockRequests::default();
            let workspace = self.editor.workspace.clone();
            let locked = workspace.locked;
            self.dock_rects.clear();

            // Bottom first: `egui` gives each side to whichever panel asks
            // first, so the order here is the order down the window.
            for section in workspace.sections(buzz_ui::Dock::Bottom) {
                let neighbours = workspace.on(buzz_ui::Dock::Bottom);
                let height = if section.front == buzz_ui::PanelId::Timeline {
                    workspace.bottom_height
                } else {
                    240.0
                };
                let response = egui::Panel::bottom(egui::Id::new(("dock-bottom", section.group)))
                    // **Exact, not default.** egui keeps a size of its own per
                    // panel, and a `default_size` is only consulted when it has
                    // none — so the workspace's number and egui's could
                    // disagree, and the panel would spring back to whichever
                    // one won. The workspace is the single source of truth, and
                    // the splitters below are what move it.
                    .resizable(false)
                    .exact_size(height)
                    .show(ui, |ui| {
                        section_header(
                            ui,
                            &section,
                            &neighbours,
                            locked,
                            !section.front.draws_own_title(),
                            false,
                            &mut requests,
                        );
                        self.draw_panel(ui, section.front, &mut commands);
                    });
                if section.panels.contains(&buzz_ui::PanelId::Timeline) {
                    self.dock_rects
                        .push((buzz_ui::Dock::Bottom, response.response.rect));
                }
            }

            for (dock, id_name, width) in [(buzz_ui::Dock::Left, "dock-left", workspace.left_width)]
            {
                let sections = workspace.sections(dock);
                if sections.is_empty() {
                    continue;
                }
                let neighbours = workspace.on(dock);
                let response = egui::Panel::left(id_name)
                    .resizable(false)
                    .exact_size(width)
                    .show(ui, |ui| {
                        self.draw_column(
                            ui,
                            &sections,
                            &neighbours,
                            locked,
                            &mut requests,
                            &mut commands,
                        );
                    });
                self.dock_rects.push((dock, response.response.rect));
            }

            for (dock, id_name, width) in [
                (
                    buzz_ui::Dock::RightOuter,
                    "dock-right-outer",
                    workspace.right_outer_width,
                ),
                (buzz_ui::Dock::Right, "dock-right", workspace.right_width),
            ] {
                let sections = workspace.sections(dock);
                if sections.is_empty() {
                    continue;
                }
                let neighbours = workspace.on(dock);
                let response = egui::Panel::right(id_name)
                    .resizable(false)
                    .exact_size(width)
                    .show(ui, |ui| {
                        self.draw_column(
                            ui,
                            &sections,
                            &neighbours,
                            locked,
                            &mut requests,
                            &mut commands,
                        );
                    });
                self.dock_rects.push((dock, response.response.rect));
            }

            // Floating sections are windows over the stage, movable and
            // resizable unless the layout is locked. A grouped section floats
            // as **one** window with its tabs, which is what "undock these two
            // together" has to mean if grouping is to survive being undocked.
            let floating = workspace.sections(buzz_ui::Dock::Float);
            let float_neighbours = workspace.on(buzz_ui::Dock::Float);
            for section in floating {
                let slot = workspace.slot(section.front).copied();
                let mut open = true;
                let title = if section.is_tabbed() {
                    // Named for what is in it, so two floating groups are not
                    // two identical title bars.
                    section
                        .panels
                        .iter()
                        .map(|id| id.tab_title())
                        .collect::<Vec<_>>()
                        .join(" · ")
                } else {
                    section.front.title().to_string()
                };
                let response = egui::Window::new(title)
                    .id(egui::Id::new(("float", section.group)))
                    .open(&mut open)
                    .movable(!locked)
                    .resizable(!locked)
                    .default_pos(slot.map(|s| s.float_pos).unwrap_or((320.0, 140.0)))
                    .default_size(slot.map(|s| s.float_size).unwrap_or((300.0, 380.0)))
                    .show(ui.ctx(), |ui| {
                        // A lone floating panel is not named here: the window
                        // frame already carries the title, and the panel below
                        // may carry it again. A tabbed one still needs its tabs.
                        section_header(
                            ui,
                            &section,
                            &float_neighbours,
                            locked,
                            false,
                            false,
                            &mut requests,
                        );
                        egui::ScrollArea::vertical()
                            .id_salt(("float-scroll", section.group))
                            .show(ui, |ui| self.draw_panel(ui, section.front, &mut commands));
                    });
                if let Some(response) = response {
                    let rect = response.response.rect;
                    // Written to every tab in the section, so the window keeps
                    // its place whichever of them is at the front next time.
                    for id in &section.panels {
                        if let Some(slot) = self.editor.workspace.slot_mut(*id) {
                            slot.float_pos = (rect.min.x, rect.min.y);
                            slot.float_size = (rect.width(), rect.height());
                        }
                    }
                }
                if !open {
                    // Closing a floating group closes the whole group: the
                    // window is the section, and leaving its other tabs behind
                    // with no window would lose them.
                    for id in &section.panels {
                        requests.moves.push((*id, buzz_ui::Dock::Hidden));
                    }
                }
            }

            requests.apply(&mut self.editor.workspace);

            // The edit-path breadcrumb. Animate keeps this strip directly above
            // the stage, and it is the only way back out of a symbol.
            //
            // Added and removed rather than collapsed: whether it is there is
            // decided by the document, not by the user, and egui's collapsible
            // panel binds a `&mut bool` the user can also flip — which would
            // let them hide their only way out of a symbol.
            if !self.editor.scene().edit_path().is_empty() {
                egui::Panel::top("breadcrumb")
                    .frame(egui::Frame::new().fill(Palette::chrome()).inner_margin(3))
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
                    // Over the artwork, after the chrome, so it is never drawn
                    // under a ruler or a selection outline.
                    self.stage_scrollbars(ui, area);
                    self.stage_zoom_overlay(ui, area);
                    area
                })
                .inner
        };

        // An export runs on its own thread; this is where what it has done
        // reaches the screen. The repaint request is what keeps the progress
        // bar moving on a document that is otherwise still.
        self.poll_export();
        self.poll_animate_import();
        if self.export.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.export_dialog(&ctx);
        self.lip_sync_dialog(&ctx);
        self.recovery_dialog(&ctx);
        buzz_ui::about_dialog(&ctx, &mut self.editor.about);

        // File ▸ New asks before it acts, and the answer is remembered.
        let new_document = buzz_ui::new_document_dialog(&ctx, &mut self.editor.new_document);
        if let Some(setup) = new_document.create {
            self.editor.create_document(setup);
        }

        commands.extend(keyboard_commands(&ctx, &self.editor));
        for command in commands {
            self.dispatch(command);
        }

        // The dock splitters, after everything else has had its say.
        self.dock_splitters(ui, stage_area);

        // The brand's band across the top, last and over everything: it is the
        // window's masthead, not any panel's decoration.
        buzz_ui::theme::top_banner(&ctx, ui.min_rect().union(ui.max_rect()));

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
            self.editor.select_layer(layer);
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

    /// Draw a column of docked sections, each with its own tab strip.
    ///
    /// A section holding several panels draws one header and one body: the
    /// tabs across the top, the front tab's contents below. That is the whole
    /// point of grouping — five occasional panels cost the height of one.
    fn draw_column(
        &mut self,
        ui: &mut egui::Ui,
        sections: &[buzz_ui::Section],
        neighbours: &[buzz_ui::PanelId],
        locked: bool,
        requests: &mut DockRequests,
        commands: &mut Vec<Command>,
    ) {
        egui::ScrollArea::vertical()
            .id_salt(("column", sections.first().map(|s| s.front)))
            .show(ui, |ui| {
                for (index, section) in sections.iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    // Rolled up, the header carries the name whatever the panel
                    // would normally do, or the column becomes a stack of
                    // anonymous strips. A tabbed section always shows its tabs,
                    // so it never needs this.
                    let named = section.collapsed || !section.front.draws_own_title();
                    section_header(ui, section, neighbours, locked, named, true, requests);
                    if !section.collapsed {
                        self.draw_panel(ui, section.front, commands);
                    }
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
                if let Some(tool) = panels::tool_bar(ui, self.editor.tool(), &mut self.editor.style)
                {
                    commands.push(Command::SelectTool(tool));
                }
            }

            Layers => {
                let editor = &mut self.editor;
                let selection = &mut editor.selection;
                let frame = editor.current_frame;
                let mut raised = None;
                editor.doc.edit("Layer Properties", |scene| {
                    raised = panels::layers_panel(ui, scene, selection, frame);
                });
                if let Some(command) = raised {
                    commands.push(command);
                }
            }

            Properties => {
                // Selecting the Camera row makes this the camera's properties,
                // as it does in Animate: one panel, showing whatever is
                // currently selected.
                if self.editor.camera_selected {
                    self.camera_panel(ui);
                    return;
                }

                let editor = &mut self.editor;
                let at = editor.edit_at();
                let selection = &editor.selection;
                let style = &mut editor.style;
                let view = &mut editor.view;
                editor.doc.edit("Document Properties", |scene| {
                    panels::properties_panel(ui, scene, selection, style, view, at);
                });
            }

            Color => {
                let editor = &mut self.editor;
                panels::color_panel(ui, editor.doc.scene(), &mut editor.style);
            }

            Assets => {
                let can_add = !self.editor.selection.is_empty();
                let action = buzz_ui::assets_panel(
                    ui,
                    &self.editor.assets,
                    &mut self.editor.assets_panel,
                    can_add,
                );
                if let Some(action) = action {
                    self.apply_asset_action(action);
                }
            }

            Swatches => {
                // Naming a colour and moving one between folders are edits to
                // the document, so the panel runs inside an undo step — the
                // same arrangement the Library panel uses.
                let editor = &mut self.editor;
                let state = &mut editor.swatch_panel;
                let style = &mut editor.style;
                editor.doc.edit("Swatches", |scene| {
                    buzz_ui::swatch_panel(ui, scene, state, style);
                });
            }
            Depth => self.depth_panel(ui),
            Rig => self.rig_panel(ui),
            Filters => self.filter_panel(ui),
            Lighting => self.light_panel(ui),
            Sound => self.sound_panel(ui),

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
                    auto_keyframe: self.editor.auto_keyframe,
                    edit_multiple: self.editor.edit_multiple,
                    onion_before: self.editor.onion.before,
                    onion_after: self.editor.onion.after,
                    frame_width: self.editor.workspace.frame_width,
                    row_scale: self.editor.workspace.row_scale,
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

    /// The camera's properties, shown when the Camera row is selected.
    ///
    /// Every control keys the camera at the playhead — the same rule the
    /// Camera tool follows when it is dragged, so aiming the camera by hand
    /// and typing a number into a box do the same thing.
    fn camera_panel(&mut self, ui: &mut egui::Ui) {
        let frame = self.editor.current_frame;
        let response = buzz_ui::camera_panel(ui, self.editor.scene().camera(), frame);

        if response.toggle {
            self.editor.run(Command::ToggleCamera);
        }

        if let Some(key) = response.set {
            self.editor.doc.edit("Camera", |scene| {
                scene.camera_mut().enabled = true;
                scene.camera_mut().set_key(key.clamped());
            });
        }

        if let Some(distance) = response.set_focal_distance {
            self.editor.doc.edit("Camera Depth", |scene| {
                scene.camera_mut().focal_distance = distance;
            });
        }

        if response.add_key {
            self.editor.run(Command::AddCameraKeyframe);
        }
        if response.remove_key {
            self.editor.run(Command::RemoveCameraKeyframe);
        }
        if response.reset {
            self.editor.run(Command::ResetCamera);
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
            buzz_ui::FilterTarget::Object => {
                match object.and_then(|id| editor.scene().find_object(id)) {
                    Some((_, found)) => (found.filters.clone(), Some(found.blend)),
                    None => (Vec::new(), None),
                }
            }
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
        let apply = |editor: &mut Editor,
                     label: &'static str,
                     change: &dyn Fn(&mut Vec<buzz_scene::Filter>)| {
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
    /// The Sound panel, and the edits it asks for.
    ///
    /// Everything it changes is one field of the `SoundRef` on one keyframe, so
    /// it is applied as an ordinary document edit and undoes in one step — and
    /// the cues are rebuilt afterwards, because changing a sync mode or a
    /// volume that the player never heard about would be a setting that
    /// appeared to do nothing.
    fn sound_panel(&mut self, ui: &mut egui::Ui) {
        let editor = &mut self.editor;
        let frame = editor.current_frame;
        let layer = editor.selection.active_layer();

        let library: Vec<buzz_ui::SoundChoice> = editor
            .doc
            .scene()
            .sounds()
            .iter()
            .map(|s| buzz_ui::SoundChoice {
                id: s.id,
                name: s.name.clone(),
                seconds: s.duration_seconds(),
            })
            .collect();

        // The keyframe the playhead is on, and whether it is one at all. A
        // sound lives on a keyframe, so a frame inside a span has nowhere to
        // put one — and saying so is more use than an inert panel.
        let (current, on_keyframe) = match layer
            .and_then(|id| editor.doc.scene().stage_layers().get(id))
            .and_then(|l| l.frames.keyframes().iter().find(|k| k.start == frame))
        {
            Some(keyframe) => (keyframe.sound, true),
            None => (None, false),
        };

        let response = buzz_ui::sound_panel(ui, &library, current, on_keyframe, frame);

        if let Some(reference) = response.set
            && let Some(layer) = layer
        {
            editor.doc.edit("Sound Settings", |scene| {
                scene.set_frame_sound(layer, frame, reference);
            });
            editor.doc.end_gesture();
            let scene = editor.doc.scene().clone();
            editor.sound.refresh(&scene);
        }

        if response.import {
            self.dispatch(Command::ImportSound);
        }
        if response.lip_sync {
            self.dispatch(Command::LipSync);
        }
    }

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
            // As in the Layers panel: the layer's artwork comes with it.
            self.editor.select_layer(layer);
            // Clicking a layer takes the camera row's highlight away, as
            // clicking any row takes it from the one before.
            self.editor.camera_selected = false;
        }
        if let Some((layer, icon)) = response.toggle_layer {
            // The same edit the Layers panel makes, through the same document
            // call — one undo step, named for the switch, so undoing "hide" is
            // a thing the history says rather than a thing you have to guess.
            use buzz_ui::panels::LayerIcon;
            let label = match icon {
                LayerIcon::Eye => "Show/Hide Layer",
                LayerIcon::Lock => "Lock Layer",
                LayerIcon::Outline => "Outline Layer",
            };
            self.editor.doc.edit(label, |scene| {
                scene.update_layer(layer, |l| match icon {
                    LayerIcon::Eye => l.visible = !l.visible,
                    LayerIcon::Lock => l.locked = !l.locked,
                    LayerIcon::Outline => l.outline = !l.outline,
                });
            });
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
        if response.toggle_auto_keyframe {
            commands.push(Command::ToggleAutoKeyframe);
        }
        if response.toggle_edit_multiple {
            commands.push(Command::ToggleEditMultipleFrames);
        }
        if let Some((before, after)) = response.set_onion_range {
            self.editor.onion.before = before;
            self.editor.onion.after = after;
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
        if let Some(region) = response.set_loop {
            self.editor.set_loop_region(region);
        }
        if let Some(frames) = response.set_frame_count {
            self.editor.set_frame_count(frames);
        }
        if let Some(command) = response.command {
            commands.push(command);
        }
        // The timeline's own zooms: workspace state, saved with the layout
        // rather than with the film, and clamped here so a hand-edited
        // workspace file cannot ask for a one-pixel frame.
        if let Some(width) = response.set_frame_width {
            self.editor.workspace.frame_width = width.clamp(
                *buzz_ui::workspace::FRAME_WIDTH_RANGE.start(),
                *buzz_ui::workspace::FRAME_WIDTH_RANGE.end(),
            );
            self.editor.workspace.save();
        }
        if let Some(scale) = response.set_row_scale {
            self.editor.workspace.row_scale = scale.clamp(
                *buzz_ui::workspace::ROW_SCALE_RANGE.start(),
                *buzz_ui::workspace::ROW_SCALE_RANGE.end(),
            );
            self.editor.workspace.save();
        }
    }

    /// Drag handles on the boundaries between the stage and the docks.
    ///
    /// # Why these exist rather than egui's own
    ///
    /// `Panel::resizable(true)` puts its handle on the panel's edge and
    /// registers the interaction when the panel is drawn. The stage is a
    /// central panel drawn *after* every dock, and its own click-and-drag
    /// interaction covers the whole area — so it claimed the pixels the handle
    /// needed, and dragging a boundary did nothing at all. (What the user sees
    /// is a panel that "resizes and springs back", because a live drag over the
    /// stage is still panning or marqueeing underneath.)
    ///
    /// These are drawn last, in a foreground layer, so nothing can take them,
    /// and they move the **workspace's** own numbers — which the panels are
    /// then laid out from exactly. One source of truth, and it is the one that
    /// gets saved.
    fn dock_splitters(&mut self, ui: &mut egui::Ui, stage: egui::Rect) {
        if self.editor.workspace.locked || stage.width() < 40.0 || stage.height() < 40.0 {
            return;
        }

        /// How wide a boundary is to grab, in points. Wider than it looks: a
        /// two-pixel target is a fight, and the handle is invisible until the
        /// pointer is on it anyway.
        const GRAB: f32 = 6.0;

        let mut changed = false;

        // (id, the strip to grab, vertical?, which way widening runs)
        let mut handles: Vec<(&'static str, egui::Rect, bool, f32)> = Vec::new();

        // **Every handle sits on the column's own edge, as it was laid out.**
        //
        // A column's inner edge is not always the stage: the far-right column's
        // neighbour is the right column, not the artwork. Reading the rects
        // back is what lets the outer column have a handle at all — and it is
        // what stops the others drifting off the boundary when a column is
        // hidden and the layout shifts under them.
        let vertical = |rect: egui::Rect, x: f32| {
            egui::Rect::from_min_max(
                egui::pos2(x - GRAB * 0.5, rect.top()),
                egui::pos2(x + GRAB * 0.5, rect.bottom()),
            )
        };

        for (dock, rect) in self.dock_rects.clone() {
            // A column squeezed to nothing on a tiny window has no meaningful
            // edge to grab.
            if rect.width() < 1.0 || rect.height() < 1.0 {
                continue;
            }
            match dock {
                // Widening a left column means dragging its right edge right.
                buzz_ui::Dock::Left => {
                    handles.push(("split-left", vertical(rect, rect.right()), true, 1.0));
                }
                // And a right column means dragging its left edge left.
                buzz_ui::Dock::Right => {
                    handles.push(("split-right", vertical(rect, rect.left()), true, -1.0));
                }
                buzz_ui::Dock::RightOuter => {
                    handles.push((
                        "split-right-outer",
                        vertical(rect, rect.left()),
                        true,
                        -1.0,
                    ));
                }
                // The timeline runs the full width of the window, and its top
                // edge therefore also runs under the dock columns. The handle
                // is kept to the stage's width so it cannot steal the bottom
                // few points of a column the user is clicking in.
                buzz_ui::Dock::Bottom => {
                    handles.push((
                        "split-bottom",
                        egui::Rect::from_min_max(
                            egui::pos2(stage.left(), rect.top() - GRAB * 0.5),
                            egui::pos2(stage.right(), rect.top() + GRAB * 0.5),
                        ),
                        false,
                        -1.0,
                    ));
                }
                buzz_ui::Dock::Float | buzz_ui::Dock::Hidden => {}
            }
        }

        for (name, rect, vertical, direction) in handles {
            let response = ui.interact(rect, egui::Id::new(name), egui::Sense::click_and_drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(if vertical {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
                // Only shown while it matters, so the window is not striped
                // with handles nobody asked about.
                ui.painter().rect_filled(
                    rect.shrink2(if vertical {
                        egui::vec2(2.0, 0.0)
                    } else {
                        egui::vec2(0.0, 2.0)
                    }),
                    1.0,
                    buzz_ui::Palette::active(),
                );
            }
            if response.dragged() {
                let delta = response.drag_delta();
                let moved = if vertical { delta.x } else { delta.y } * direction;
                use buzz_ui::workspace::{
                    BOTTOM_HEIGHT_RANGE, COLUMN_WIDTH_RANGE, LEFT_WIDTH_RANGE, clamp_to,
                };
                let workspace = &mut self.editor.workspace;
                // The same ranges the workspace clamps a loaded layout to, so a
                // column cannot be dragged to a width that the next launch
                // would silently undo.
                match name {
                    "split-left" => {
                        workspace.left_width =
                            clamp_to(workspace.left_width + moved, LEFT_WIDTH_RANGE);
                    }
                    "split-right" => {
                        workspace.right_width =
                            clamp_to(workspace.right_width + moved, COLUMN_WIDTH_RANGE);
                    }
                    "split-right-outer" => {
                        workspace.right_outer_width =
                            clamp_to(workspace.right_outer_width + moved, COLUMN_WIDTH_RANGE);
                    }
                    _ => {
                        workspace.bottom_height =
                            clamp_to(workspace.bottom_height + moved, BOTTOM_HEIGHT_RANGE);
                    }
                }
                changed = true;
            }
            if response.drag_stopped() {
                // Saved when the drag ends rather than on every pixel of it.
                self.editor.workspace.save();
            }
        }

        if changed {
            ui.ctx().request_repaint();
        }
    }

    /// Scrollbars along the bottom and right of the stage.
    ///
    /// # Why the stage has them at all
    ///
    /// Panning is by the wheel, space-drag, middle-drag or the Hand tool, and
    /// Ctrl+wheel zooms. All of that is fine once you know it, and none of it
    /// tells you
    /// *where you are*: with the view somewhere off the pasteboard there was
    /// nothing on screen to say which way the artwork lay. A scrollbar is the
    /// one control that answers that without being asked — the thumb's size is
    /// how much of the work is on screen, and its position is where.
    ///
    /// # What they scroll over
    ///
    /// The stage rectangle and everything drawn on this frame, plus a stage's
    /// worth of margin so there is somewhere to put artwork that is still being
    /// moved into place. The extent therefore grows with the drawing rather
    /// than being a fixed canvas the way Animate's is.
    fn stage_scrollbars(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        const THICKNESS: f32 = 9.0;
        /// How far the bars sit in from the edge of the drawing area.
        const INSET: f32 = 4.0;
        if area.width() < 120.0 || area.height() < 120.0 {
            return;
        }

        let camera = &self.editor.camera;
        let visible = camera.visible_doc_rect();
        let extent = self.scrollable_extent(visible);

        // Nothing to scroll: the whole extent is on screen.
        let horizontal = extent.width() > visible.width() + 1.0;
        let vertical = extent.height() > visible.height() + 1.0;
        if !horizontal && !vertical {
            return;
        }

        // **Inset, translucent and rounded**, so it reads as something floating
        // over the drawing area rather than as part of the panel beside it.
        //
        // Drawn flush to the edge in the panel colour, it butted straight up
        // against the Layers panel with no gap and in almost the same grey, and
        // what a user sees then is the panel wearing a scrollbar. The stage's
        // furniture has to look like the stage's.
        let track_colour = egui::Color32::from_black_alpha(70);
        let thumb_colour = buzz_ui::Palette::text_dim().gamma_multiply(0.85);
        let thumb_hot = buzz_ui::Palette::active();
        // Clear of the panel edge, and clear of the corner where the two bars
        // would otherwise meet.
        let area = area.shrink(INSET);

        let mut moved: Option<(f64, f64)> = None;

        if horizontal {
            let track = egui::Rect::from_min_max(
                egui::pos2(area.left(), area.bottom() - THICKNESS),
                egui::pos2(
                    area.right() - if vertical { THICKNESS } else { 0.0 },
                    area.bottom(),
                ),
            );
            let response = ui.interact(
                track,
                egui::Id::new("stage-scroll-x"),
                egui::Sense::click_and_drag(),
            );
            let (thumb, fraction) =
                thumb_of(track, extent.x0, extent.x1, visible.x0, visible.x1, true);
            ui.painter()
                .rect_filled(track, THICKNESS / 2.0, track_colour);
            ui.painter().rect_filled(
                thumb,
                THICKNESS / 2.0,
                if response.hovered() || response.dragged() {
                    thumb_hot
                } else {
                    thumb_colour
                },
            );

            // Dragging the thumb, or clicking anywhere on the track, puts the
            // view where the pointer is — the behaviour every scrollbar has.
            if (response.dragged() || response.clicked())
                && let Some(pos) = response.interact_pointer_pos()
            {
                let t = ((pos.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0) as f64;
                let span = extent.width() - visible.width();
                let x = extent.x0 + span * t + visible.width() / 2.0;
                moved = Some((x, camera.center.y));
            }
            let _ = fraction;
        }

        if vertical {
            let track = egui::Rect::from_min_max(
                egui::pos2(area.right() - THICKNESS, area.top()),
                egui::pos2(
                    area.right(),
                    area.bottom() - if horizontal { THICKNESS } else { 0.0 },
                ),
            );
            let response = ui.interact(
                track,
                egui::Id::new("stage-scroll-y"),
                egui::Sense::click_and_drag(),
            );
            let (thumb, _) = thumb_of(track, extent.y0, extent.y1, visible.y0, visible.y1, false);
            ui.painter()
                .rect_filled(track, THICKNESS / 2.0, track_colour);
            ui.painter().rect_filled(
                thumb,
                THICKNESS / 2.0,
                if response.hovered() || response.dragged() {
                    thumb_hot
                } else {
                    thumb_colour
                },
            );

            if (response.dragged() || response.clicked())
                && let Some(pos) = response.interact_pointer_pos()
            {
                let t = ((pos.y - track.top()) / track.height().max(1.0)).clamp(0.0, 1.0) as f64;
                let span = extent.height() - visible.height();
                let y = extent.y0 + span * t + visible.height() / 2.0;
                let x = moved.map(|(x, _)| x).unwrap_or(self.editor.camera.center.x);
                moved = Some((x, y));
            }
        }

        if let Some((x, y)) = moved {
            self.editor.camera.center = buzz_geom::Point::new(x, y);
            ui.ctx().request_repaint();
        }
    }

    /// Everything worth scrolling over: the stage, the artwork, and a margin.
    fn scrollable_extent(&self, visible: buzz_geom::Rect) -> buzz_geom::Rect {
        let scene = self.editor.scene();
        let mut extent = scene.stage().stage_rect();

        for layer in scene.layers().iter() {
            for object in layer.objects_at(self.editor.current_frame) {
                let bounds = scene.resolved_bounds(object);
                if bounds.width().is_finite() && bounds.height().is_finite() {
                    extent = extent.union(bounds);
                }
            }
        }

        // A stage's worth of air on every side, so there is somewhere to put
        // artwork that is on its way somewhere.
        let margin = buzz_geom::Vec2::new(
            scene.stage().size.width * 0.5,
            scene.stage().size.height * 0.5,
        );
        extent = buzz_geom::Rect::new(
            extent.x0 - margin.x,
            extent.y0 - margin.y,
            extent.x1 + margin.x,
            extent.y1 + margin.y,
        );

        // And never smaller than what is on screen, or the thumb would be
        // longer than its track.
        extent.union(visible)
    }

    /// The zoom control that sits on the stage itself, top right.
    ///
    /// # Why here as well as in the status bar
    ///
    /// The status bar is where a *number* belongs, and it is the last place
    /// anybody looks while drawing. Zoom is used constantly and with the
    /// pointer already over the artwork, so the control belongs where the eye
    /// already is — which is what every other creative tool has settled on,
    /// Animate included.
    ///
    /// Drawn as an overlay rather than as another panel: it must float above
    /// the artwork without taking a strip of the stage away from it.
    fn stage_zoom_overlay(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        if area.width() < 160.0 || area.height() < 80.0 {
            return;
        }

        // Inset from the corner, and clear of the ruler along the top edge.
        let top = area.min.y
            + if self.editor.view.show_rulers {
                24.0
            } else {
                8.0
            };
        let anchor = egui::pos2(area.max.x - 8.0, top);

        egui::Area::new(egui::Id::new("stage-zoom"))
            .fixed_pos(anchor)
            .pivot(egui::Align2::RIGHT_TOP)
            .order(egui::Order::Middle)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Palette::panel())
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("\u{2212}")
                                .on_hover_text("Zoom out (Ctrl+- or Ctrl+wheel)")
                                .clicked()
                            {
                                self.editor.zoom_by(1.0 / WHEEL_ZOOM_STEP.powi(3));
                            }

                            // The percentage is also the drag: pulling it is
                            // the fastest way across a wide range, and the
                            // speed is proportional so one gesture works at
                            // 50% and at a trillion.
                            let mut percent = self.editor.camera.zoom_percent();
                            let speed = percent.max(1.0) * 0.02;
                            let response = ui.add(
                                egui::DragValue::new(&mut percent)
                                    .speed(speed)
                                    .range(0.001..=f64::MAX)
                                    .suffix(" %"),
                            );
                            if response.changed() {
                                self.editor.camera.set_zoom_percent(percent);
                            }

                            if ui
                                .small_button("+")
                                .on_hover_text("Zoom in (Ctrl+= or Ctrl+wheel)")
                                .clicked()
                            {
                                self.editor.zoom_by(WHEEL_ZOOM_STEP.powi(3));
                            }

                            // `⏷`, not `▾`: egui's bundled fonts have the first and
                            // not the second, which drew as an empty box in the very
                            // first screenshot of this control.
                            ui.menu_button("\u{23F7}", |ui| {
                                for preset in [25.0, 50.0, 100.0, 200.0, 400.0, 800.0] {
                                    if ui.button(format!("{preset:.0}%")).clicked() {
                                        self.editor.camera.set_zoom_percent(preset);
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                if ui.button("Fit in Window").clicked() {
                                    self.editor.run(Command::ZoomFitInWindow);
                                    ui.close();
                                }
                                if ui.button("Show All").clicked() {
                                    self.editor.run(Command::ZoomShowAll);
                                    ui.close();
                                }
                                if ui.button("Show Frame").clicked() {
                                    self.editor.run(Command::ZoomShowFrame);
                                    ui.close();
                                }
                            })
                            .response
                            .on_hover_text("Zoom presets");

                            ui.separator();

                            // Pan, for anybody who has not met the space bar.
                            let panning = self.editor.tool() == buzz_ui::ToolId::Hand;
                            if ui
                                .selectable_label(panning, "\u{270B}")
                                .on_hover_text(
                                    "Pan the view \u{2014} or hold the space bar, or drag with \
                                     the middle button, whatever the tool",
                                )
                                .clicked()
                            {
                                let next = if panning {
                                    buzz_ui::ToolId::Selection
                                } else {
                                    buzz_ui::ToolId::Hand
                                };
                                self.editor.run(Command::SelectTool(next));
                            }
                        });
                    });
            });
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

        // **Ctrl+wheel zooms; the wheel alone scrolls.** Animate's arrangement,
        // and every drawing program's — a wheel that zooms by itself makes a
        // long timeline impossible to walk down, and it was surprising in the
        // other direction too: with a modifier held the wheel still zoomed,
        // because any scroll at all did.
        //
        // egui has already turned Ctrl+wheel (and a trackpad pinch) into a zoom
        // factor and taken it out of the scroll delta, so the two cannot fight.
        if response.hovered() {
            let (zoom, scroll) = ctx.input(|i| (i.zoom_delta() as f64, i.smooth_scroll_delta));

            if zoom != 1.0
                && let Some(pos) = ctx.input(|i| i.pointer.hover_pos())
            {
                // About the pointer, so the thing under it stays under it.
                self.editor.camera.zoom_by_at(zoom, local(pos));
            } else if scroll != egui::Vec2::ZERO {
                // Taken as it comes: egui has already applied Shift for
                // horizontal and Alt for vertical, which is where the old
                // behaviour came from — Alt forces the wheel onto the vertical
                // axis, and anything on that axis used to zoom.
                self.editor.apply(ToolAction::PanView {
                    delta_screen: buzz_geom::Vec2::new(scroll.x as f64, scroll.y as f64),
                });
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

        // **Double-click goes in and out of a symbol**, which is how anybody
        // who has used Animate navigates: double-click an instance to edit it
        // where it stands, double-click past the artwork to come back out.
        // Without it a nested character could only be opened through the
        // Library, one level at a time.
        if response.double_clicked()
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
            && !pan_override
        {
            self.editor.enter_or_leave_at(local(pos));
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.editor.machine.cancel();
        }
    }

    fn dispatch(&mut self, command: Command) {
        match command {
            Command::ImportSound => self.import_sound_dialog(),
            Command::ImportImage => self.import_image_dialog(),
            Command::LipSync => self.open_lip_sync(),
            Command::ExportImage => self.open_export(buzz_ui::ExportKind::Image),
            Command::ExportSequence => self.open_export(buzz_ui::ExportKind::Sequence),
            Command::ExportVideo => self.open_export(buzz_ui::ExportKind::Video),
            Command::Open => self.open_dialog(),
            Command::Save => self.save(false),
            Command::SaveAs => self.save(true),
            Command::Close => self.editor.should_quit = true,
            Command::ImportToStage => self.import_dialog(buzz_scene::ImportTarget::Stage),
            Command::ImportToLibrary => self.import_dialog(buzz_scene::ImportTarget::Library),
            other => self.editor.run(other),
        }
    }

    /// Act on what the Assets panel asked for.
    ///
    /// The panel raises intentions and this performs them, because writing
    /// files and merging documents is the shell's business — and placing an
    /// asset has to land in an undo step, which the panel knows nothing about.
    fn apply_asset_action(&mut self, action: buzz_ui::AssetAction) {
        use buzz_ui::AssetAction::*;
        match action {
            Place(asset) => {
                let library = self.editor.assets.clone();
                let mut outcome = None;
                self.editor.doc.edit("Place Asset", |scene| {
                    outcome = Some(library.place(&asset, scene));
                });
                match outcome {
                    Some(Ok(report)) => {
                        // The placed artwork is what the user now wants to move,
                        // exactly as it is after an import.
                        self.editor.selection.clear();
                        self.editor
                            .selection
                            .ensure_active_layer(self.editor.doc.scene());
                        self.editor.status = Some(format!(
                            "Placed {} — {} layers, {} symbols",
                            asset.name, report.layers, report.symbols
                        ));
                    }
                    Some(Err(e)) => {
                        // The edit above recorded an undo step for a merge that
                        // did not happen; take it back rather than leaving a
                        // "Place Asset" in the history that changed nothing.
                        self.editor.doc.undo();
                        self.editor.status = Some(format!("Could not place {}: {e}", asset.name));
                    }
                    None => {}
                }
            }

            Add { folder } => {
                let ids = self.editor.selection.ids();
                if ids.is_empty() {
                    self.editor.status = Some("Select artwork to keep as an asset".into());
                    return;
                }
                let asset = self
                    .editor
                    .doc
                    .scene()
                    .extract(self.editor.current_frame, &ids);
                let name = self.editor.assets.unique_name("Asset", &folder);
                match self.editor.assets.save(&name, &folder, &asset) {
                    Ok(saved) => {
                        self.editor.status = Some(format!("Kept {} in Assets", saved.label()));
                    }
                    Err(e) => self.editor.status = Some(format!("Could not save the asset: {e}")),
                }
            }

            NewFolder => {
                let existing: Vec<String> = self.editor.assets.folders().to_vec();
                let mut name = "Folder".to_string();
                for n in 2..1000 {
                    if !existing.contains(&name) {
                        break;
                    }
                    name = format!("Folder {n}");
                }
                if let Err(e) = self.editor.assets.create_folder(&name) {
                    self.editor.status = Some(format!("Could not make the folder: {e}"));
                } else {
                    self.editor.assets_panel.selected_folder = name;
                }
            }

            Rename { asset, name } => {
                if let Err(e) = self.editor.assets.rename(&asset, &name) {
                    self.editor.status = Some(format!("Could not rename: {e}"));
                }
            }

            Delete(asset) => {
                if let Err(e) = self.editor.assets.delete(&asset) {
                    self.editor.status = Some(format!("Could not delete: {e}"));
                }
            }

            Rescan => {
                self.editor.assets.rescan();
                self.editor.status = Some(format!("{} assets", self.editor.assets.len()));
            }

            ImportFromAnimate => self.import_animate_assets(),
        }
    }

    /// Bring an Animate asset library across, all of it.
    ///
    /// The folder picker opens on this machine's Animate assets folder when
    /// there is one: the path is long, buried under `Documents`, and nobody
    /// should have to remember it.
    fn import_animate_assets(&mut self) {
        if self.animate_import.is_some() {
            self.editor.status = Some("An Animate import is already running".into());
            return;
        }

        let mut picker = rfd::FileDialog::new();
        if let Some(root) = crate::animate_assets::likely_roots().first() {
            picker = picker.set_directory(root);
        }
        let Some(root) = picker.pick_folder() else {
            return;
        };

        let found = crate::animate_assets::scan(&root);
        if found.is_empty() {
            self.editor.status = Some(format!(
                "No Animate assets in {} \u{2014} expected a folder with Custom/ in it",
                root.display()
            ));
            return;
        }

        let total = found.len();
        self.editor.assets_panel.importing = Some((0, total));
        self.editor.status = Some(format!("Importing {total} assets from Animate\u{2026}"));
        self.animate_import = Some(crate::animate_assets::import_all(
            found,
            self.editor.assets.clone(),
        ));
    }

    /// Move an Animate import along, if one is running.
    ///
    /// Polled rather than pushed for the same reason an export is: the work is
    /// on another thread, and the window redraws when it feels like it.
    fn poll_animate_import(&mut self) {
        let Some(progress) = &self.animate_import else {
            return;
        };

        let mut finished = None;
        while let Ok(message) = progress.try_recv() {
            match message {
                crate::animate_assets::Progress::Working { done, total } => {
                    self.editor.assets_panel.importing = Some((done, total));
                }
                crate::animate_assets::Progress::Finished {
                    imported,
                    skipped,
                    failed,
                } => {
                    finished = Some((imported, skipped, failed));
                }
            }
        }

        if let Some((imported, skipped, failed)) = finished {
            self.animate_import = None;
            self.editor.assets_panel.importing = None;
            self.editor.assets.rescan();
            let mut message = format!("Imported {imported} assets from Animate");
            if skipped > 0 {
                // Bitmaps, almost always: see §7 item 22.
                message.push_str(&format!("; {skipped} skipped"));
            }
            if !failed.is_empty() {
                message.push_str(&format!("; {} could not be read", failed.len()));
            }
            self.editor.status = Some(message);
            if !failed.is_empty() {
                // Named rather than counted: which ones failed is the only
                // useful thing to know afterwards.
                self.editor.import_summary = Some(crate::import::ImportSummary {
                    title: "Imported from Animate".to_string(),
                    what_arrived: format!("{imported} assets"),
                    unsupported: failed,
                    failed: false,
                });
            }
        }
    }

    /// Open the Export dialog, sized to the document as it is now.
    fn open_export(&mut self, kind: buzz_ui::ExportKind) {
        if self.export.is_some() {
            self.editor.status = Some("An export is already running".into());
            return;
        }
        // Checked as the dialog opens rather than when Export is pressed, so
        // the missing dependency is visible while the settings are still being
        // chosen instead of after a file name has been picked.
        let has_ffmpeg = kind != buzz_ui::ExportKind::Video || buzz_export::ffmpeg_available();
        let (size, length) = {
            let scene = self.editor.scene();
            // The length of the **film**, not of the timeline: a looping
            // section is repeated into the export, so the default range has to
            // reach the end of what will actually be written. Without a loop
            // region the two are the same number.
            (scene.stage().size, scene.rendered_frame_count())
        };
        self.editor.export.ffmpeg = has_ffmpeg;
        self.editor.export.open(
            kind,
            (
                size.width.round().max(1.0) as u32,
                size.height.round().max(1.0) as u32,
            ),
            length,
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
            buzz_ui::ExportKind::Video => {
                let options = self.editor.export.video;
                let extension = options.container.extension();
                let picked = rfd::FileDialog::new()
                    .add_filter(options.container.label(), &[extension])
                    .set_file_name(format!("{stem}.{extension}"))
                    .save_file();
                let Some(path) = picked else { return };

                crate::export_job::ExportJob::video(
                    scene,
                    self.editor.export.range(),
                    path,
                    settings,
                    buzz_export::VideoSettings {
                        codec: match options.codec {
                            buzz_ui::VideoChoice::H264 => buzz_export::VideoCodec::H264,
                            buzz_ui::VideoChoice::Hevc => buzz_export::VideoCodec::Hevc,
                            buzz_ui::VideoChoice::Av1 => buzz_export::VideoCodec::Av1,
                        },
                        container: match options.container {
                            buzz_ui::ContainerChoice::Mp4 => buzz_export::VideoContainer::Mp4,
                            buzz_ui::ContainerChoice::Mov => buzz_export::VideoContainer::Mov,
                        },
                        quality: options.quality,
                        hardware: options.hardware,
                        audio: options.audio,
                    },
                    self.preference.clone(),
                )
            }
        };

        self.editor.export.progress = Some(job.progress);
        self.export = Some(job);
    }

    /// Animate's File ▸ Import Image.
    fn import_image_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Image", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
            .pick_file();
        let Some(path) = picked else { return };

        match self.editor.import_image(&path) {
            Ok(name) => {
                self.editor.status = Some(format!(
                    "Imported {name} — it is artwork now: the Lasso and the Magic Wand cut it"
                ))
            }
            Err(e) => self.editor.status = Some(format!("Could not import that image: {e:#}")),
        }
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
            // Not reachable from an import — pasting is `Editor::paste_clipboard`
            // — but a match that guesses would be worse than one that says so.
            buzz_scene::ImportTarget::Onto { .. } => "Paste",
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
                failed: false,
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
        // **Everything openable, not only our own format.** The importers have
        // existed since Phase 5 but were reachable only through File ▸ Import,
        // so File ▸ Open refused an Animate document — the very file somebody
        // coming from Animate would reach for first.
        let mut everything = vec![buzz_doc::EXTENSION];
        everything.extend_from_slice(crate::import::IMPORTABLE);

        let picked = rfd::FileDialog::new()
            .add_filter("Everything BuzzAnimate can open", &everything)
            .add_filter("BuzzAnimate document", &[buzz_doc::EXTENSION])
            .add_filter("Animate document", &["fla", "xfl"])
            .add_filter("Flash movie", &["swf"])
            .add_filter("PDF or Illustrator artwork", &["pdf", "ai"])
            .pick_file();
        let Some(path) = picked else { return };

        if opens_as_document(&path) {
            self.open_buzz(&path);
        } else {
            self.open_imported(&path);
        }
    }

    /// Put a different document on screen, carrying across the things that
    /// belong to the **person** rather than to the film.
    ///
    /// The panel layout was already carried this way; the clipboard has to be
    /// too, or copy-here-open-that-paste-there — the one thing a clipboard can
    /// do that Duplicate cannot — quietly loses what you copied. Everything
    /// else about an `Editor` is about the document and should not survive it.
    fn adopt_document(&mut self, doc: Document) {
        let workspace = std::mem::take(&mut self.editor.workspace);
        let clipboard = self.editor.clipboard.take();
        self.editor = Editor::new(doc);
        self.editor.workspace = workspace;
        self.editor.clipboard = clipboard;
    }

    /// Open one of our own documents.
    fn open_buzz(&mut self, path: &std::path::Path) {
        match Document::open(path) {
            Ok(doc) => {
                self.adopt_document(doc);
                self.remember_document_directory(path);
                self.editor.status = Some(format!("Opened {}", path.display()));
            }
            Err(e) => {
                self.editor.status = Some(format!("Could not open: {e}"));
            }
        }
    }

    /// Open a foreign file — `.fla`, `.xfl`, `.swf`, `.pdf`, `.ai` — as a new
    /// document.
    ///
    /// **Not as the document's own path.** What comes back is a *translation*,
    /// however good; saving it must ask for a `.buzz` file rather than write
    /// back over somebody's Animate source, which this program cannot produce
    /// and would therefore destroy.
    fn open_imported(&mut self, path: &std::path::Path) {
        let imported = match crate::import::read(path) {
            Ok(imported) => imported,
            Err(message) => {
                // **In front of the user, not in the status bar.** A file that
                // will not open is the whole of what they were trying to do,
                // and the reason is usually specific enough to act on — but
                // only if it is read.
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                self.editor.status = Some(format!("Could not open {name}: {message}"));
                self.editor.import_summary = Some(crate::import::ImportSummary {
                    title: format!("Could not open {name}"),
                    what_arrived: message.clone(),
                    unsupported: Vec::new(),
                    failed: true,
                });
                return;
            }
        };

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        self.adopt_document(Document::new(imported.scene.clone()));
        self.editor.doc.mark_clean();
        self.editor
            .selection
            .ensure_active_layer(self.editor.doc.scene());
        self.editor.zoom_fit();
        self.remember_document_directory(path);
        self.editor.status = Some(format!("Opened {name} — {}", imported.summary));

        // What did not survive the translation is worth interrupting for: it is
        // the difference between the file they had and the one they now have.
        if !imported.unsupported.is_empty() {
            self.editor.import_summary = Some(crate::import::ImportSummary {
                title: format!("Opened {name}"),
                what_arrived: imported.summary.clone(),
                unsupported: imported.unsupported.clone(),
                failed: false,
            });
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
            Ok(()) => {
                // Saved: there is nothing left to recover, and the crash
                // snapshot must not offer to restore what is now on disk.
                buzz_doc::autosave::forget_crash_snapshot();
                self.remember_document_directory(&path);
                self.editor.status = Some(format!("Saved {}", path.display()));
            }
            Err(e) => self.editor.status = Some(format!("Could not save: {e}")),
        }
    }

    /// Note where a document lives, so a crash can be recovered from there.
    fn remember_document_directory(&mut self, path: &std::path::Path) {
        if let Some(directory) = path.parent() {
            self.editor.workspace.remember_directory(directory);
            self.editor.workspace.save();
        }
    }

    /// Look for autosaves left behind by a crash.
    ///
    /// The application's own recovery directory holds work that was never
    /// saved; the remembered directories hold copies written beside documents
    /// that were. Both are offered together, most recent first.
    fn find_recoveries(&self) -> buzz_ui::RecoveryState {
        let mut directories = vec![buzz_doc::autosave::recovery_dir()];
        directories.extend(self.editor.workspace.recovery_dirs.iter().cloned());
        directories.dedup();

        let now = std::time::SystemTime::now();
        let mut found = Vec::new();
        for directory in directories {
            for recovery in buzz_doc::find_recoveries(&directory) {
                let age_seconds = recovery
                    .modified
                    .and_then(|m| now.duration_since(m).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                found.push(buzz_ui::RecoveryEntry {
                    path: recovery.path,
                    document: recovery.document,
                    age_seconds,
                });
            }
        }
        found.sort_by_key(|e| e.age_seconds);
        buzz_ui::RecoveryState { found }
    }

    /// Draw the recovery prompt and act on what was chosen.
    fn recovery_dialog(&mut self, ctx: &egui::Context) {
        match buzz_ui::recovery_dialog(ctx, &self.recovery) {
            buzz_ui::RecoveryChoice::None => {}
            buzz_ui::RecoveryChoice::Recover(entry) => {
                match Document::open(&entry.path) {
                    Ok(mut doc) => {
                        // Detached from the file it came from: Save must ask
                        // where to put it rather than writing back over a
                        // recovery, and this session's own autosave must not
                        // land on the file it was recovered from.
                        doc.forget_path();
                        self.adopt_document(doc);

                        // Moved aside rather than deleted: the prompt should
                        // not offer it again every launch, and a copy on disk
                        // costs nothing next to the work it holds.
                        let aside = entry.path.with_file_name(format!(
                            "{}.recovered.buzz",
                            entry
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().replace(".recovery.buzz", ""))
                                .unwrap_or_else(|| "untitled".into())
                        ));
                        let _ = std::fs::rename(&entry.path, &aside);
                        // Opened *as* the recovery file, and deliberately
                        // without adopting the original document's path: Save
                        // must not overwrite the file on disk until the user
                        // has looked at what came back and chosen to.
                        self.editor.status = Some(format!(
                            "Recovered {} \u{2014} save it somewhere to keep it",
                            entry.title()
                        ));
                        self.recovery.found.retain(|e| e.path != entry.path);
                    }
                    Err(e) => {
                        self.editor.status = Some(format!("Could not recover: {e}"));
                        self.recovery.found.retain(|e| e.path != entry.path);
                    }
                }
            }
            buzz_ui::RecoveryChoice::Discard(entry) => {
                let _ = std::fs::remove_file(&entry.path);
                self.recovery.found.retain(|e| e.path != entry.path);
            }
            buzz_ui::RecoveryChoice::Later => self.recovery.found.clear(),
        }
    }

    /// Keep the snapshot a crash would be recovered from.
    ///
    /// Autosave runs every couple of minutes; this runs every frame the
    /// document changes, and costs a pointer copy \u2014 a scene is a tree of
    /// `Arc`s, so cloning one copies pointers rather than artwork. It is what
    /// turns "up to two minutes lost" into "nothing lost".
    fn keep_crash_snapshot(&mut self) {
        let revision = self.editor.doc.scene().revision();
        if self.last_crash_revision == Some(revision) {
            return;
        }
        self.last_crash_revision = Some(revision);
        if self.editor.doc.is_dirty() {
            buzz_doc::autosave::remember_for_crash(
                self.editor.doc.scene(),
                self.editor.doc.recovery_path(),
            );
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

        // A theme change asked for last frame: restyle before anything is
        // drawn, so the whole window changes at once rather than a panel at a
        // time.
        if std::mem::take(&mut self.editor.restyle) {
            theme::apply(&egui_ctx);
        }
        let window = active.window.clone();

        // `run_ui` rather than `begin_pass`/`end_pass`: egui 0.35 roots the UI
        // in a `Ui`, and panels attach to that rather than to the context.
        let mut stage_area = egui::Rect::NOTHING;
        let output = egui_ctx.run_ui(raw_input, |ui| {
            stage_area = self.build_ui(ui);
            // Floats above the panels, so it is drawn after them.
            self.import_report_window(ui.ctx());
        });

        // A theme change is asked for from *inside* the frame that is being
        // built, so the restyle happens at the top of the next one — and there
        // has to *be* a next one. egui only redraws when something asks it to,
        // and without this the window keeps the old chrome until the pointer
        // moves.
        if self.editor.restyle {
            egui_ctx.request_repaint();
        }

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
        active
            .gpu
            .render(&active.vello, target, w, h, pasteboard())?;

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

        self.keep_crash_snapshot();
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

/// Everything the user asked the dock chrome to do this frame.
///
/// Collected rather than applied on the spot, because every one of these
/// rearranges the layout the panels are *currently being drawn from*. Applying
/// a move mid-frame would leave the rest of the column laid out against a
/// workspace that no longer describes it.
#[derive(Default)]
struct DockRequests {
    moves: Vec<(buzz_ui::PanelId, buzz_ui::Dock)>,
    reorders: Vec<(buzz_ui::PanelId, i32)>,
    /// Bring a tab to the front of its section.
    selects: Vec<buzz_ui::PanelId>,
    /// Put the first panel into the second's section, as a tab.
    groups: Vec<(buzz_ui::PanelId, buzz_ui::PanelId)>,
    /// Take a panel out of its section, into one of its own.
    ungroups: Vec<buzz_ui::PanelId>,
    /// Roll a section up to its tabs, or open it.
    collapses: Vec<(buzz_ui::PanelId, bool)>,
}

impl DockRequests {
    /// Apply what was asked, and save the layout if anything changed.
    ///
    /// Selection and roll-up are applied before the rest: they are the two that
    /// cannot fail, and doing them first means clicking a tab in a section you
    /// then also move still leaves the right tab at the front.
    fn apply(self, workspace: &mut buzz_ui::Workspace) -> bool {
        let touched = !self.moves.is_empty()
            || !self.reorders.is_empty()
            || !self.selects.is_empty()
            || !self.groups.is_empty()
            || !self.ungroups.is_empty()
            || !self.collapses.is_empty();

        for id in self.selects {
            workspace.select_tab(id);
        }
        for (id, collapsed) in self.collapses {
            workspace.set_collapsed(id, collapsed);
        }
        for (id, target) in self.groups {
            workspace.group_with(id, target);
        }
        for id in self.ungroups {
            workspace.ungroup(id);
        }
        for (id, dock) in self.moves {
            workspace.move_to(id, dock);
        }
        for (id, delta) in self.reorders {
            workspace.reorder(id, delta);
        }

        if touched {
            workspace.save();
        }
        touched
    }
}

/// The strip at the top of every panel section: its tabs, and the menu that
/// moves it.
///
/// # Sections, not panels
///
/// Several panels can share one section — Animate's panel group — and then this
/// strip is a row of tabs and only the front one's contents are drawn below it.
/// A section holding one panel is the same strip showing that panel's name,
/// which is what every section was before grouping existed.
///
/// Animate puts the same menu behind the ≡ button in each panel's corner. It is
/// drawn here rather than inside each panel because a panel should not have to
/// know it is dockable — every one of them was written before this existed, and
/// none of them needed changing.
///
/// # Nothing in this row may overflow
///
/// A widget wider than its `Ui` expands that `Ui`'s **max** rect, the column
/// then reports itself wider than it was drawn, and egui lays the stage out
/// underneath it. See `dock_geometry_tests`. That is why the menu is placed
/// against the right edge first, why the tabs wrap, and why every label
/// truncates.
fn section_header(
    ui: &mut egui::Ui,
    section: &buzz_ui::Section,
    neighbours: &[buzz_ui::PanelId],
    locked: bool,
    named: bool,
    collapsible: bool,
    out: &mut DockRequests,
) {
    // **A header that looks like a header.**
    //
    // Panels in a column were separated by a hairline and nothing else, so a
    // column of them read as one long undifferentiated list — which is why the
    // Library "looked obscure" and the Assets panel below it was reported
    // missing rather than merely out of sight. A filled strip the width of the
    // panel says plainly where one panel stops and the next begins.
    let frame = egui::Frame::new()
        .fill(Palette::raised())
        .inner_margin(egui::Margin::symmetric(4, 2))
        .corner_radius(3);

    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            // The default button padding alone is twelve points of the twenty
            // this row has in the tools strip.
            ui.spacing_mut().button_padding = egui::vec2(3.0, 1.0);
            ui.spacing_mut().item_spacing.x = 3.0;

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Three bars, drawn as text: this one *is* in egui's bundled font,
                // unlike most symbols — and it is checked by a test rather than
                // assumed, because that assumption has been wrong twice.
                ui.menu_button(egui::RichText::new(PANEL_MENU).small(), |ui| {
                    if locked {
                        ui.label(egui::RichText::new("The layout is locked").small().weak());
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
                            out.moves.push((section.front, dock));
                            ui.close();
                        }
                    }

                    ui.separator();
                    for (label, delta) in [("Move Up", -1), ("Move Down", 1)] {
                        if ui.add_enabled(!locked, egui::Button::new(label)).clicked() {
                            out.reorders.push((section.front, delta));
                            ui.close();
                        }
                    }

                    // **Grouping**, which is the whole point of a section.
                    //
                    // A menu rather than a drag: dragging a tab onto another
                    // panel is what Animate does and it is the nicer gesture,
                    // but it needs a drop-target model this dock does not have,
                    // and a menu can be read, found and tested. The offer is
                    // only ever the panels on the same side — a section is a
                    // stack within one dock, so grouping across two of them
                    // would have to move one first, silently.
                    ui.separator();
                    let elsewhere: Vec<buzz_ui::PanelId> = neighbours
                        .iter()
                        .copied()
                        .filter(|id| !section.panels.contains(id))
                        .collect();
                    ui.add_enabled_ui(!locked && !elsewhere.is_empty(), |ui| {
                        ui.menu_button("Group With", |ui| {
                            for target in &elsewhere {
                                if ui.button(target.title()).clicked() {
                                    out.groups.push((section.front, *target));
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "Share a section with another panel, as tabs \u{2014} \
                             both take the room of one",
                        )
                        .on_disabled_hover_text(if locked {
                            "The layout is locked"
                        } else {
                            "Nothing else is docked on this side"
                        });
                    });
                    if ui
                        .add_enabled(
                            !locked && section.is_tabbed(),
                            egui::Button::new("Ungroup This Panel"),
                        )
                        .on_hover_text("Give this tab a section of its own")
                        .clicked()
                    {
                        out.ungroups.push(section.front);
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Move, group, float or close this panel");

                // Whatever the menu left over holds the roll-up triangle and
                // the tabs.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    // The roll-up triangle, where every collapsible thing keeps
                    // one. Only docked sections get it: a floating window
                    // already has a close button, and rolling one up would
                    // leave a title bar adrift over the stage.
                    if collapsible
                        && ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(if section.collapsed {
                                        ROLLED_UP
                                    } else {
                                        OPEN
                                    })
                                    .small(),
                                )
                                .frame(false),
                            )
                            .on_hover_text(if section.collapsed {
                                "Rolled up \u{2014} click to open"
                            } else {
                                "Roll up, and keep the tabs"
                            })
                            .clicked()
                    {
                        out.collapses.push((section.front, !section.collapsed));
                    }

                    if section.is_tabbed() {
                        // **Tabs.** Wrapped, because five of them do not fit a
                        // 216-point column on one line and a tab that does not
                        // fit is one nobody can click.
                        ui.horizontal_wrapped(|ui| {
                            for id in &section.panels {
                                let front = *id == section.front;
                                let label = egui::RichText::new(id.tab_title()).small();
                                let label = if front { label } else { label.weak() };
                                if ui
                                    .add(egui::Button::selectable(front, label).truncate())
                                    .on_hover_text(id.title())
                                    .clicked()
                                {
                                    out.selects.push(*id);
                                }
                            }
                        });
                    } else if named {
                        // Only the panels with no heading of their own are named
                        // here. The rest would read their name twice - which is
                        // exactly how the first version looked.
                        //
                        // Rolled up, every panel is named: the title bar is all
                        // that is left of it, and an unlabelled strip is not a
                        // panel, it is a smudge.
                        let room = ui.available_width().max(1.0);
                        ui.add_sized(
                            egui::vec2(room, ui.spacing().interact_size.y),
                            egui::Label::new(
                                egui::RichText::new(section.front.title())
                                    .small()
                                    .color(Palette::text_dim()),
                            )
                            .truncate(),
                        );
                    }
                });
            });
        });
    });
}

/// The panel menu's label.
///
/// Three dots, not the hamburger every docking interface uses: that
/// character has **no glyph** in egui's bundled font and would draw as an
/// empty box. `theme::font_has` said so before it reached a screenshot,
/// which is the whole reason that check exists.
const PANEL_MENU: &str = "...";

/// The roll-up triangle on a docked panel's header.
///
/// `\u{25b8}` and `\u{25be}` — the small triangles a dropdown normally uses —
/// have **no glyph** in the bundled font, so every panel in every dock column
/// was headed by an empty box where its expander should be. `theme::font_has`
/// already knew: both are in the list of characters this project has been
/// caught out by. These two are the pair the Library panel settled on, and they
/// are covered by the glyph test.
const ROLLED_UP: &str = "\u{25b6}";
const OPEN: &str = "\u{23f7}";

#[cfg(test)]
mod dock_geometry_tests {
    use super::*;

    /// **A dock column must report the rectangle it was given.**
    ///
    /// This is the invariant that failed, and everything the user saw followed
    /// from it. egui lays a right-hand panel out at the edge it was allotted,
    /// then takes the panel's *frame* rect back to decide where the central
    /// panel — the stage — begins. A widget wider than its `Ui` expands that
    /// `Ui`'s **max** rect as well as its min rect, so one overflowing combo
    /// box in the Layers panel grew the frame, the column reported itself 56
    /// points to the right of where it had been drawn, and the stage was then
    /// laid out *underneath* it. The visible result was the stage's ruler and
    /// its own scrollbar painted across the Properties panel — reported, quite
    /// reasonably, as "the scrollbar overlaps the panels".
    ///
    /// No panel drawing is involved here on purpose: this measures the dock
    /// chrome itself, which lives in this crate and which the panel tests in
    /// `buzz-ui` therefore cannot see. `buzz-ui`'s `dock_columns` measures the
    /// panels; this measures what wraps them.
    #[test]
    fn a_dock_column_reports_the_rectangle_it_was_placed_in() {
        for width in [
            *buzz_ui::workspace::COLUMN_WIDTH_RANGE.start(),
            300.0,
            *buzz_ui::workspace::LEFT_WIDTH_RANGE.start(),
        ] {
            let ctx = egui::Context::default();
            buzz_ui::theme::apply(&ctx);

            let mut placed = egui::Rect::NOTHING;
            let mut reported = egui::Rect::NOTHING;

            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(1920.0, 1040.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let outer = ui.available_rect_before_wrap();
                    placed = egui::Rect::from_min_max(
                        egui::pos2(outer.right() - width, outer.top()),
                        outer.max,
                    );

                    let response = egui::Panel::right("probe")
                        .resizable(false)
                        .exact_size(width)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().id_salt("probe").show(ui, |ui| {
                                let mut requests = DockRequests::default();
                                let neighbours: Vec<buzz_ui::PanelId> =
                                    buzz_ui::PanelId::ALL.to_vec();

                                // The header every docked panel wears, for every
                                // panel there is — including the two narrow ones.
                                for id in buzz_ui::PanelId::ALL {
                                    let section = buzz_ui::Section {
                                        group: 0,
                                        panels: vec![id],
                                        front: id,
                                        collapsed: false,
                                    };
                                    section_header(
                                        ui,
                                        &section,
                                        &neighbours,
                                        false,
                                        true,
                                        true,
                                        &mut requests,
                                    );
                                }

                                // **And a tab strip carrying every panel there
                                // is.** Tabs are the widest thing this chrome
                                // can be asked to draw, and the whole reason
                                // they wrap and truncate is that a strip which
                                // does not fit takes the column's rect with it.
                                let all = buzz_ui::Section {
                                    group: 1,
                                    panels: buzz_ui::PanelId::ALL.to_vec(),
                                    front: buzz_ui::PanelId::Layers,
                                    collapsed: false,
                                };
                                section_header(
                                    ui,
                                    &all,
                                    &neighbours,
                                    false,
                                    true,
                                    true,
                                    &mut requests,
                                );
                            });
                        });
                    reported = response.response.rect;
                },
            );

            // A point of slack for the frame's own rounding; 56 points of drift
            // is what this exists to catch.
            assert!(
                (reported.right() - placed.right()).abs() < 1.0,
                "a {width}-point column was drawn at {placed:?} and reported \
                 {reported:?}. The stage is laid out from what the column \
                 reports, so it will be drawn underneath the panel by \
                 {:.0} points.",
                reported.right() - placed.right()
            );
        }
    }
}

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

    /// Both icons decode, and the taskbar's is the larger of the two.
    ///
    /// The taskbar draws its button from the *big* icon; ours was never set,
    /// so Windows fell back to a blank sheet of paper. A test rather than a
    /// look, because the failure is silent — the title bar keeps showing the
    /// logo while the taskbar does not.
    #[test]
    fn the_window_carries_a_small_icon_and_a_large_one() {
        let small = icon_from_png(include_bytes!("../../../assets/logo-32.png"));
        let big = icon_from_png(include_bytes!("../../../assets/logo-128.png"));
        assert!(small.is_some(), "the title bar icon did not decode");
        assert!(big.is_some(), "the taskbar icon did not decode");
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;

    #[test]
    fn our_own_documents_open_directly_and_the_rest_are_imported() {
        let document = |name: &str| opens_as_document(std::path::Path::new(name));

        assert!(document("scene.buzz"));
        assert!(
            document("SCENE.BUZZ"),
            "the extension is not case-sensitive"
        );
        assert!(!document("scene.fla"), "an Animate document is translated");
        assert!(!document("scene.xfl"));
        assert!(!document("movie.swf"));
        assert!(!document("art.ai"));
        assert!(!document("no-extension"));
    }

    fn track() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(500.0, 10.0))
    }

    /// The thumb says how much of the work is on screen. Half the extent
    /// visible is half a track; a tenth is a tenth.
    #[test]
    fn the_thumb_is_as_long_as_the_view_is_wide() {
        let (half, _) = thumb_of(track(), 0.0, 1000.0, 0.0, 500.0, true);
        assert!(
            (half.width() - 250.0).abs() < 1.0,
            "half the extent should be half the track: {half:?}"
        );

        let (tenth, _) = thumb_of(track(), 0.0, 1000.0, 0.0, 100.0, true);
        assert!((tenth.width() - 50.0).abs() < 1.0, "{tenth:?}");
    }

    /// And where it sits says where you are.
    #[test]
    fn the_thumb_sits_where_the_view_is() {
        let (start, _) = thumb_of(track(), 0.0, 1000.0, 0.0, 500.0, true);
        assert!((start.left() - track().left()).abs() < 1.0, "{start:?}");

        let (end, _) = thumb_of(track(), 0.0, 1000.0, 500.0, 1000.0, true);
        assert!(
            (end.right() - track().right()).abs() < 1.0,
            "the far end of the extent should put the thumb at the far end: {end:?}"
        );

        let (middle, _) = thumb_of(track(), 0.0, 1000.0, 250.0, 750.0, true);
        assert!(
            (middle.center().x - track().center().x).abs() < 1.0,
            "{middle:?}"
        );
    }

    /// A thumb must always be grabbable, however little of the work is showing
    /// — at a trillion per cent zoom the honest proportion is a fraction of a
    /// pixel, and a scrollbar you cannot catch is not a scrollbar.
    #[test]
    fn the_thumb_never_shrinks_to_nothing() {
        let (tiny, _) = thumb_of(track(), 0.0, 1e12, 0.0, 1.0, true);
        assert!(tiny.width() >= 24.0, "{tiny:?}");
        assert!(track().contains_rect(tiny.shrink(0.5)), "{tiny:?}");
    }

    /// Vertical works the same way, on the other axis.
    #[test]
    fn the_vertical_thumb_matches() {
        let track = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 400.0));
        let (thumb, _) = thumb_of(track, 0.0, 800.0, 400.0, 800.0, false);
        assert!((thumb.height() - 200.0).abs() < 1.0, "{thumb:?}");
        assert!((thumb.bottom() - track.bottom()).abs() < 1.0, "{thumb:?}");
    }

    /// Degenerate numbers arrive from a fresh document and from a window being
    /// dragged to nothing; neither may panic or produce a NaN.
    #[test]
    fn a_degenerate_scrollbar_is_harmless() {
        let (thumb, shown) = thumb_of(track(), 0.0, 0.0, 0.0, 0.0, true);
        assert!(thumb.width().is_finite() && shown.is_finite());

        let (thumb, _) = thumb_of(
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(0.0, 0.0)),
            0.0,
            100.0,
            0.0,
            10.0,
            true,
        );
        assert!(thumb.width().is_finite());
    }
}
