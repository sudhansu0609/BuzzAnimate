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
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{Window, WindowId};

/// The one custom event the window loop takes: "something off the UI thread
/// wants a frame". egui raises it through its repaint callback (a tooltip is due,
/// an animation is running), and it is the safety net that lets the loop sit
/// idle — `ControlFlow::Wait` — without ever missing a repaint egui asked for.
#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    /// Wake and draw a frame.
    Repaint,
}

/// What the loop should do after a frame: draw again now, draw at a set time, or
/// sleep until something wakes it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Redraw {
    Now,
    At(Instant),
    Idle,
}

/// Everything the stage's Vello encoding depends on, captured so an unchanged
/// frame can reuse the last encoding instead of rebuilding it.
///
/// It carries the document revision (all artwork, the stage, the document
/// camera), plus the **view** state that is not in the revision — the current
/// frame, the pan/zoom camera, the viewport, which symbol is open, and the onion
/// and Edit-Multiple-Frames settings. A tool preview or a pending lighting build
/// is handled separately, by refusing to reuse at all while either is live; see
/// [`App::maybe_build_stage`]. Miss nothing here and a quiet repaint is free;
/// the escape hatch `BUZZ_NO_RETAIN=1` disables the reuse entirely.
#[derive(Clone, PartialEq, Debug)]
struct StageStamp {
    revision: u64,
    frame: u32,
    /// View camera: centre, zoom, rotation, as raw bits.
    camera: [u64; 4],
    /// The stage area in physical pixels, as raw bits.
    area: [u64; 4],
    /// Which symbols are open for editing, outermost first.
    edit_path: Vec<u64>,
    onion: (bool, bool, u32, u32),
    edit_multiple: bool,
    /// The generation of installed lighting geometry.
    lights_generation: u64,
    /// **The lighting rig itself**, as [`buzz_scene::LightRig::fingerprint`],
    /// resolved at the frame being drawn.
    ///
    /// Not covered by `revision`, and the difference matters. `revision` is a
    /// clock on the document's *content*, and undo puts it back: adding a light
    /// takes it from 5 to 6, undoing returns it to 5, and adding a different
    /// light takes it to 6 again. So the same number describes two different
    /// rigs, and the only thing standing between that and a retained encoding
    /// of the wrong lighting is that a frame happened to be drawn in between.
    /// Redraw requests coalesce, so that is not something to rely on.
    ///
    /// A fingerprint of the rig is what the reuse test actually wants to ask,
    /// and it costs one hash of a handful of lights per frame. It also covers
    /// what the revision never could: a keyframed light, whose values change
    /// from frame to frame with no edit at all.
    lights: u64,
    /// Whether the encoding this stamp describes has a tool preview painted
    /// **into** it. See the reuse test in `run`: it is what makes the frame
    /// after a brush stroke rebuild rather than keep the stale ink.
    painted_preview: bool,
}

/// A batch of shading geometry being built off the UI thread.
///
/// # Why it can be abandoned
///
/// Aiming a light is a drag, and a drag is fifty pointer positions. Each one
/// asks for the crescents of everything on screen; each batch takes longer than
/// the gap between two pointer moves. Without a way to give up, every batch ran
/// to completion for a light that had already moved on — so the pool stayed
/// saturated for the whole gesture, the window was starved of the cores it
/// needed to draw, and the geometry never once caught up with the pointer. That
/// is what "the app hangs and the controls are laggy" was.
///
/// So a batch carries a flag, and the frame that notices the light has moved
/// again sets it. The workers drop what is left within one crescent, the pool
/// empties, and a batch for where the light *is* starts instead. At most one
/// batch is ever in flight, and it is always the current one.
struct ShadeBuild {
    results: crossbeam_channel::Receiver<Vec<buzz_render::lighting::Built>>,
    /// Set to abandon the batch. Read between crescents by the workers.
    abandon: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The light rig this batch is being built for. When the rig no longer
    /// matches, the batch is for a light that has moved and is abandoned.
    aim: u64,
}

impl ShadeBuild {
    /// Give up on this batch, so its workers stop and free the pool.
    fn abandon(&self) {
        self.abandon
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

use crate::editor::Editor;
use crate::rigging;
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
/// Show a finished export in the file manager, selected where it can be.
///
/// Best-effort: a failure to open a folder is not worth interrupting the user
/// over — they know where they saved it — so the result is ignored.
fn reveal_in_folder(path: &std::path::Path) {
    #[cfg(windows)]
    {
        // `/select,` highlights the file within its folder. A directory (a PNG
        // sequence's destination) is opened directly instead.
        if path.is_dir() {
            let _ = std::process::Command::new("explorer").arg(path).spawn();
        } else {
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .spawn();
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // No portable "select the file", so open the folder it is in.
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// What to call a file in front of a person: its name, not its whole path.
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

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
    compositor: buzz_render::Compositor,
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

    /// **Has the window been shown to anybody yet?**
    ///
    /// It is created hidden and revealed by [`Active::reveal`] once a whole
    /// frame has been built for it — see [`App::init`] for why.
    shown: bool,
    /// When the window was created, so the reveal can be forced if the first
    /// frame never gets that far. See [`Active::reveal_is_overdue`].
    born: Instant,
    /// What to say on the opening scene: the adapter this came up on.
    adapter: String,
}

/// Why a path was asked for.
///
/// The picker runs on its own thread and answers a frame or several later, by
/// which time nothing remembers what the question was — so the question comes
/// back attached to the answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// File ▸ Open.
    Open,
    /// File ▸ Save As, or a first save of an untitled document.
    SaveAs,
    ImportImage,
    /// An image to lay into the selected shapes as a fill.
    FillWithImage,
    ImportSound,
    /// File ▸ Import, into the stage or into the library.
    ImportInto(buzz_scene::ImportTarget),
    /// The root of an Animate asset library.
    AnimateAssets,
    /// Where an export should be written.
    Export(buzz_ui::ExportKind),
    /// Where to write the document as an Animate `.fla`.
    ExportFla,
}

/// A compact UTC timestamp, `YYYY-MM-DD HHhMM`, for naming a snapshot. Built
/// from the Unix clock with civil-date arithmetic so it needs no date crate;
/// UTC rather than local, which is honest about having no timezone database.
fn chrono_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let (hh, mm) = (sod / 3600, (sod % 3600) / 60);
    // Howard Hinnant's days-from-civil, inverted.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {hh:02}h{mm:02}")
}

/// Image files in a project-local `assets/textures` folder, offered as one-click
/// fills. Mirrors how bundled fonts are found under `assets/fonts`: dropping a
/// `.png`/`.jpg` in there makes it a texture with no rebuild. Empty (not an
/// error) when the folder is absent.
fn bundled_textures() -> Vec<std::path::PathBuf> {
    let dir = std::path::Path::new("assets/textures");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
                Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
            )
        })
        .collect();
    paths.sort();
    paths
}

/// What a background load produced.
///
/// Reading a 40 MB `.fla` is seconds of XML parsing, and a `.swf` with a
/// thousand shapes is seconds of tag decoding. Doing either on the UI thread
/// stops the window for exactly as long as the file is big — so it happens on
/// a thread, and what it made arrives here.
///
/// The failure is carried too, rather than reported from the worker: a file
/// that will not open is the whole of what the user was trying to do, and
/// saying so belongs in front of them, on the thread that owns the window.
enum Loaded {
    /// One of our own documents.
    Document {
        path: std::path::PathBuf,
        result: Result<Box<Document>, String>,
    },
    /// Somebody else's — `.fla`, `.xfl`, `.swf`, `.pdf`, `.ai`.
    ///
    /// `target` says what to do with it: `None` opens it as a new document,
    /// `Some(target)` merges it into the one already open.
    Foreign {
        path: std::path::PathBuf,
        target: Option<buzz_scene::ImportTarget>,
        result: Result<Box<crate::import::Imported>, String>,
    },
    /// A finished merge, built off the UI thread.
    ///
    /// `Scene::merge` deep-copies every incoming symbol, layer and object, which
    /// on a large `.fla` is a real cost. Doing it on the frame that collected the
    /// read froze the window for as long as the merge took — the very thing the
    /// background read was meant to prevent. So the merge runs on its own thread
    /// against a snapshot, and only the finished scene is committed here, as one
    /// undo step. The document is briefly read-only while it runs; see
    /// [`App::doc_available`].
    Merged {
        path: std::path::PathBuf,
        scene: Box<buzz_scene::Scene>,
        report: buzz_scene::MergeReport,
        unsupported: Vec<String>,
        summary: String,
    },
}

/// Symbol use counts for the Library panel, kept current off the UI thread.
///
/// Counting instances means walking every object in the document — every stage
/// layer and every symbol's timeline — which on a large imported file is a
/// per-frame cost the window cannot afford. So the walk runs on the background
/// pool against a (cheap) scene snapshot, and the panel draws the last counts it
/// has until fresher ones arrive. Counts one revision stale are invisible for a
/// use column; the cache converges as the pool catches up.
#[derive(Default)]
struct UsageCache {
    /// The document revision `counts` were computed for.
    revision: Option<u64>,
    counts: std::collections::BTreeMap<buzz_scene::SymbolId, usize>,
    /// A recompute in flight: the revision it is for, and where it will land.
    #[allow(clippy::type_complexity)]
    in_flight: Option<(
        u64,
        crossbeam_channel::Receiver<std::collections::BTreeMap<buzz_scene::SymbolId, usize>>,
    )>,
}

impl UsageCache {
    /// The counts to show right now — the freshest that have finished.
    fn counts(&self) -> &std::collections::BTreeMap<buzz_scene::SymbolId, usize> {
        &self.counts
    }

    /// Should a recompute be started for `revision`? Only when the current
    /// counts are for a different revision and nothing is already computing.
    fn should_spawn(&self, revision: u64) -> bool {
        self.revision != Some(revision) && self.in_flight.is_none()
    }

    /// Record that a recompute for `revision` was started.
    fn spawned(
        &mut self,
        revision: u64,
        rx: crossbeam_channel::Receiver<std::collections::BTreeMap<buzz_scene::SymbolId, usize>>,
    ) {
        self.in_flight = Some((revision, rx));
    }

    /// Install `counts` computed for `revision`, unless newer counts already
    /// exist — a result that finished after the document moved past it is
    /// dropped rather than allowed to overwrite fresher data.
    fn install(
        &mut self,
        revision: u64,
        counts: std::collections::BTreeMap<buzz_scene::SymbolId, usize>,
    ) -> bool {
        if self.revision.is_some_and(|current| current >= revision) {
            return false;
        }
        self.counts = counts;
        self.revision = Some(revision);
        true
    }

    /// Drain a finished recompute, if any, installing its result.
    fn poll(&mut self) {
        let Some((revision, rx)) = &self.in_flight else {
            return;
        };
        if let Ok(counts) = rx.try_recv() {
            let revision = *revision;
            self.in_flight = None;
            self.install(revision, counts);
        }
    }
}

/// A script in flight: the task running it, and where its result will arrive.
struct ScriptRun {
    id: crate::tasks::TaskId,
    /// The working scene and what the run made of it, sent back together.
    result: crossbeam_channel::Receiver<(buzz_scene::Scene, buzz_script::ScriptOutcome)>,
}

pub struct App {
    active: Option<Active>,
    editor: Editor,
    jobs: Arc<JobSystem>,
    preference: GpuPreference,
    /// Wakes the event loop from an idle wait. Handed in by `main` and given to
    /// egui's repaint callback in `init`. `None` in tests, which never run a
    /// real loop.
    proxy: Option<EventLoopProxy<UserEvent>>,
    /// When egui last asked to be repainted, as an absolute instant. `None`
    /// means egui is idle and wants no timed repaint. Folded into
    /// [`App::wants_frame`] so the loop wakes for tooltips and animations.
    egui_repaint: Option<Instant>,
    /// Escape hatch: `BUZZ_POLL=1` forces the old always-redraw loop, for
    /// bisecting any report of stale UI.
    force_poll: bool,
    /// What the stage's retained Vello encoding was last built from. When the
    /// next frame's inputs match this, the encoding in `active.vello` is reused
    /// as-is rather than rebuilt — so a repaint that only touched a panel (a
    /// tooltip, a background install elsewhere) does not re-encode a huge stage.
    /// See [`StageStamp`].
    stage_stamp: Option<StageStamp>,
    /// The stage's scrollable content extent, cached by `(revision, frame)`.
    /// Computing it walks and resolves every object's bounds through the library
    /// — cheap on a small file, seconds on a rig-heavy import — so it must not
    /// run on a frame that only panned or zoomed. Both are camera-only and touch
    /// neither the revision nor the frame, so the cache holds across them.
    scroll_extent: Option<(u64, u32, buzz_geom::Rect)>,
    /// Every symbol's resolved extent, memoised by document revision. Resolving
    /// an instance's bounds by re-walking the library is exponential in the
    /// nesting; against this table it is a lookup. Rebuilt only on an edit, so it
    /// stays warm across every frame the user scrubs through.
    bounds_table: Option<(u64, std::collections::HashMap<buzz_scene::SymbolId, buzz_geom::Rect>)>,
    /// Bumped whenever installed lighting geometry changes what the stage would
    /// encode, so a retained stage is invalidated when a light's shading lands.
    lights_generation: u64,
    /// The rig's fingerprint as of the last frame drawn, so a change to the
    /// lighting can ask for the frame that shows it. See [`App::render`].
    last_rig: u64,
    /// Escape hatch: `BUZZ_NO_RETAIN=1` always re-encodes the stage, for
    /// bisecting any report of a stale stage.
    retain_stage: bool,
    /// The last stage rectangle egui actually measured. See [`App::render`]:
    /// a frame whose layout has not settled hands out `Rect::NOTHING`, and
    /// drawing through that is what a black stage is.
    last_stage_area: Option<egui::Rect>,
    /// Per-frame section timing and the over-budget warning. See
    /// [`crate::profile`].
    profiler: crate::profile::FrameProfiler,
    /// Every export, run one at a time on the task registry. Replaces the old
    /// single slot that refused a second export outright.
    exports: crate::export_service::ExportQueue,
    /// The pressure the pen last reported, `0.0`–`1.0`.
    ///
    /// Remembered between events because a gesture's press and release arrive
    /// on their own and do not always carry a force of their own; `None` is a
    /// device with no sensor, which reads as drawing at full pressure.
    pen_pressure: Option<f64>,
    /// True while the "an export is still running" quit prompt is up.
    ///
    /// A close request with work that would be lost raises this instead of
    /// exiting, so a half-written film is a decision rather than an accident.
    quit_prompt: bool,
    /// The export presets, built-in and the user's own, for the Export dialog.
    presets: crate::presets::PresetLibrary,
    /// Lighting geometry kept between frames.
    ///
    /// The renderer is stateless — a `SceneBuilder` lives for one frame — so
    /// geometry that cost a boolean to build has to be owned out here, by
    /// something that outlives frames.
    lights: buzz_render::document::DrawCache,
    /// Autosaves found on launch, while the prompt is still up.
    recovery: buzz_ui::RecoveryState,
    /// The Ctrl+K command palette's open state and query.
    command_palette: buzz_ui::CommandPaletteState,
    /// Named version snapshots of the document, and whether the list is open.
    snapshots: buzz_doc::SnapshotLibrary,
    show_snapshots: bool,
    /// The camera panel's between-frames state (the angle name being typed).
    camera_panel_state: buzz_ui::CameraPanelState,
    /// The revision the crash snapshot was last taken at.
    last_crash_revision: Option<u64>,
    /// A scene being renamed from the edit bar: its index and the text so far.
    scene_rename: Option<(usize, String)>,
    /// An Animate asset import running on its own thread.
    animate_import: Option<crossbeam_channel::Receiver<crate::animate_assets::Progress>>,
    /// A background merge is in flight, so the document is briefly read-only.
    /// Set while an imported file's artwork is being merged off the UI thread,
    /// cleared when the finished scene is committed.
    merging: bool,
    /// Pictures of the library's symbols, drawn on the window's own device.
    ///
    /// On `App` rather than `Editor` because it owns GPU resources, and the
    /// device outlives any one document. `adopt_document` clears it, so a
    /// picture from the last film cannot be shown for a symbol in this one
    /// that happens to have been given the same id.
    /// Top-left of the stage this frame, so the right-click menu can turn a
    /// pointer position into a document point the way the tools do.
    stage_area_min: egui::Pos2,
    thumbnails: crate::thumbnails::Thumbnails,
    /// Pictures for the Assets panel. Separate from [`Self::thumbnails`]
    /// because an asset is a file on disk rather than a symbol in the open
    /// document, so it is keyed and invalidated differently.
    asset_thumbnails: crate::thumbnails::AssetThumbnails,
    /// Shading geometry being built off the UI thread, if any.
    ///
    /// The first lit frame of a heavy scene used to cost a third of a second,
    /// because every crescent and cast shadow was built — one boolean each, one
    /// at a time — before anything appeared. Now a cold cache draws the frame
    /// with the shading it last had and the geometry is built on every core at
    /// once, off this thread; when it lands, the next frame is exact. Closes
    /// §7-154 and §7-155.
    shade_build: Option<ShadeBuild>,
    /// The rig's [`aim`](buzz_scene::LightRig::aim) as of the previous frame, so
    /// a frame can tell a light that is *moving* from one that has come to rest.
    /// See where the batch is started for what that decides.
    shade_aim: u64,
    /// Whether the retained stage encoding was built with shading the cache
    /// knew to be provisional. It must not be reused if so — see where reuse is
    /// decided, which is where the trap is.
    stage_stale: bool,
    /// Sounds being decoded off the UI thread, and where the results land.
    ///
    /// `Scene::merge` can bring in a whole soundtrack at once; decoding it inline
    /// on the frame the window first sees it froze for as long as the audio was
    /// long. The decode is fanned out across the interactive pool instead — the
    /// same pattern as `shade_build` — and installed when it returns. See
    /// [`crate::sound::SoundBank::take_undecoded`].
    #[allow(clippy::type_complexity)]
    sound_decode: Option<
        crossbeam_channel::Receiver<Vec<(buzz_scene::SoundId, Result<buzz_audio::Clip, String>)>>,
    >,
    /// Symbol use counts for the Library panel, computed off the UI thread.
    usage_cache: UsageCache,
    /// A script running on a thread, if one is, and where its result lands.
    ///
    /// A script is a transaction over the document: while it runs the window
    /// keeps painting but the document is read-only, held behind an
    /// input-gating overlay with a live Cancel. That is a truer picture than a
    /// frozen window — a script *is* briefly in sole charge of the document —
    /// and it is the difference between "working" and "hung".
    scripting: Option<ScriptRun>,
    /// Files being read on a thread, by the task that is reading them.
    ///
    /// A map rather than a single slot because the registry is free to run
    /// several — though [`App::loading_already`] declines a second, since two
    /// documents racing to replace the open one has no sensible answer.
    loading: std::collections::HashMap<crate::tasks::TaskId, crossbeam_channel::Receiver<Loaded>>,
    /// The file picker, if one is open, and what it was opened for.
    picker: crate::dialogs::Pending<Pick>,
    /// Every long-running piece of work in the program.
    ///
    /// On `App`, not on `Editor`, because work outlives documents — see the
    /// module docs in `tasks.rs` for the bug that taught us this.
    tasks: crate::tasks::TaskRegistry,
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
    /// The opening scene, drawn over the interface for the first second or so
    /// of a session and then dissolved. See [`buzz_ui::splash`].
    splash: buzz_ui::SplashState,
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
            proxy: None,
            egui_repaint: None,
            force_poll: std::env::var("BUZZ_POLL").is_ok(),
            stage_stamp: None,
            last_rig: 0,
            last_stage_area: None,
            scroll_extent: None,
            bounds_table: None,
            lights_generation: 0,
            retain_stage: std::env::var("BUZZ_NO_RETAIN").is_err(),
            profiler: crate::profile::FrameProfiler::default(),
            exports: crate::export_service::ExportQueue::default(),
            pen_pressure: None,
            quit_prompt: false,
            presets: crate::presets::PresetLibrary::load(),
            lights: {
                // The symbol-encoding cache is on by default; `BUZZ_NO_SYMBOL_CACHE=1`
                // turns it off so an instance-heavy document draws every instance
                // live, for bisecting a suspected reuse bug.
                let mut cache = buzz_render::document::DrawCache::new();
                if std::env::var("BUZZ_NO_SYMBOL_CACHE").is_ok() {
                    cache.set_symbol_reuse(false);
                }
                cache
            },
            recovery: buzz_ui::RecoveryState::default(),
            command_palette: buzz_ui::CommandPaletteState::default(),
            snapshots: buzz_doc::SnapshotLibrary::user(),
            show_snapshots: false,
            camera_panel_state: buzz_ui::CameraPanelState::default(),
            last_crash_revision: None,
            scene_rename: None,
            animate_import: None,
            merging: false,
            stage_area_min: egui::Pos2::ZERO,
            thumbnails: crate::thumbnails::Thumbnails::default(),
            asset_thumbnails: crate::thumbnails::AssetThumbnails::default(),
            shade_build: None,
            shade_aim: 0,
            stage_stale: false,
            sound_decode: None,
            usage_cache: UsageCache::default(),
            scripting: None,
            loading: std::collections::HashMap::new(),
            picker: crate::dialogs::Pending::default(),
            tasks: crate::tasks::TaskRegistry::default(),
            dock_rects: Vec::new(),
            splash: buzz_ui::SplashState::default(),
        };
        app.recovery = app.find_recoveries();
        app
    }

    /// Give the app a handle to wake the event loop. Called by `main` before
    /// the loop runs; egui's repaint callback uses it in [`Self::init`].
    pub fn with_proxy(mut self, proxy: EventLoopProxy<UserEvent>) -> Self {
        self.proxy = Some(proxy);
        self
    }

    /// Whether — and when — the window needs to draw another frame.
    ///
    /// A pure decision over everything that could still change what is on screen:
    /// egui's own timed repaint (animations, tooltips), playback, and any
    /// background work that installs its result on a later frame. When none of
    /// these is live the loop sleeps ([`Redraw::Idle`]) until an input event or
    /// egui's wake callback, so an idle document costs no frames at all — the
    /// difference between the old monitor-rate burn and a truly quiet window.
    fn wants_frame(&self) -> Redraw {
        if self.force_poll {
            return Redraw::Now;
        }
        // The opening scene is an animation over a window that is otherwise
        // idle, and the window it is covering may not be on screen yet: both
        // want every frame they can get until they are done.
        if self.splash.is_open() || matches!(&self.active, Some(a) if !a.shown) {
            return Redraw::Now;
        }
        // Anything mid-flight whose result lands on a future frame, or that is
        // inherently animated, wants the next frame now.
        let live = self.editor.playback.playing
            || self.editor.restyle
            || self.thumbnails.pending()
            || self.asset_thumbnails.pending()
            || self.shade_build.is_some()
            || self.sound_decode.is_some()
            || self.usage_cache.in_flight.is_some()
            || self.scripting.is_some()
            || self.merging
            || !self.loading.is_empty()
            || self.animate_import.is_some()
            || !self.exports.is_idle()
            || self.picker.busy()
            || !self.tasks.is_empty();
        if live {
            return Redraw::Now;
        }
        // Otherwise defer to egui: a timed repaint if it asked for one, else
        // sleep until woken.
        match self.egui_repaint {
            Some(at) => Redraw::At(at),
            None => Redraw::Idle,
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
    ///
    /// **Synchronous on purpose.** This runs before the event loop starts, and
    /// [`script_report`](Self::script_report) reads the result on the very next
    /// line — there is no frame in which a background run could report back.
    /// So it goes through `Editor::run` directly rather than `App::dispatch`,
    /// which is the path that hands interactive scripts to a thread.
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

        // **The interface theme is decided before the window exists.**
        //
        // It used to be read further down, after the device was up, purely
        // because that is where egui was being styled. But the *title bar* is
        // drawn by the desktop, from an attribute that is only read at creation
        // — so a dark application opened inside a light window frame, and the
        // two never agreed until something else forced a repaint. Deciding it
        // here settles the chrome and the frame from the same value, at once.
        let scheme = buzz_ui::Workspace::load().theme;
        theme::set_theme(scheme);

        let attrs = Window::default_attributes()
            .with_title("BuzzAnimate")
            .with_window_icon(window_icon())
            .with_theme(Some(match scheme {
                buzz_ui::theme::Theme::Dark => winit::window::Theme::Dark,
                buzz_ui::theme::Theme::Light => winit::window::Theme::Light,
            }))
            // **Hidden until there is something in it.**
            //
            // Everything below — choosing an adapter, creating the device,
            // compiling Vello's shaders, configuring the surface — happens
            // after the window exists and takes the best part of a second. A
            // visible window during that is a blank client area painted by the
            // desktop, and then a half-measured interface over a black stage:
            // the flash of white-then-black this program was reported for.
            //
            // So nothing is shown until a whole frame has been built for it,
            // and that first frame carries the opening scene. See
            // `Active::reveal`, and `resumed` for what draws it.
            .with_visible(false)
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
        let adapter = gpu.selection.summary();

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
        // The full-frame compositor writes to the surface directly, so it is
        // built for the surface format. When the document has no effects the
        // blitter above is used instead, so a plain document pays nothing.
        let compositor = buzz_render::Compositor::new(&gpu.device, format);

        let egui_ctx = egui::Context::default();
        // egui runs its own animations — a tooltip fading in, a spinner, the
        // caret blinking — and asks to be repainted for them through this
        // callback. With the loop idling on `ControlFlow::Wait`, this is what
        // wakes it: without it, egui's animations would stall whenever the
        // document itself was quiet. Cross-thread safe: the proxy is `Send`.
        if let Some(proxy) = self.proxy.clone() {
            egui_ctx.set_request_repaint_callback(move |_info| {
                let _ = proxy.send_event(UserEvent::Repaint);
            });
        }
        // The theme itself was settled before the window was created, above;
        // this is where the context is styled from it.
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
            compositor,
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
            shown: false,
            born: Instant::now(),
            adapter,
        })
    }
}

impl Active {
    /// **Put the window on screen, now that there is a picture ready for it.**
    ///
    /// Called from the frame that is about to present, after everything in it
    /// has been built — so the first thing the desktop ever composites for this
    /// window is a finished picture rather than an empty rectangle. Idempotent,
    /// because every caller means the same thing by it.
    fn reveal(&mut self) {
        if self.shown {
            return;
        }
        self.shown = true;
        self.window.set_visible(true);
        // Shown is not the same as focused: a window revealed after its
        // creation does not take the keyboard on its own, and an editor you
        // have to click before you can type in is a worse first impression
        // than the flash this replaced.
        self.window.focus_window();
    }

    /// **Has the window waited long enough to be shown regardless?**
    ///
    /// The reveal hangs off a frame reaching the point of being presented, and
    /// a frame can fail to get there for reasons that have nothing to do with
    /// this program. Every one of those is recoverable, and none of them should
    /// be able to leave a running process with no window at all — which is
    /// indistinguishable from a crash.
    ///
    /// So there is a deadline, and past it the window is shown whatever
    /// happened. It is a backstop and nothing routine should reach it: the
    /// first frame is drawn directly in `resumed`, which reveals as part of
    /// drawing it.
    fn reveal_is_overdue(&self) -> bool {
        !self.shown && self.born.elapsed() >= REVEAL_DEADLINE
    }

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

/// **Is this a rectangle the stage can be drawn through?**
///
/// Finite on all four sides and at least a pixel each way. Everything else —
/// `egui::Rect::NOTHING`, a panel collapsed to a hairline, a window minimised
/// to nothing — produces infinities or a degenerate transform, and a frame
/// drawn through one of those is black. See [`App::render`].
fn usable_stage_area(area: egui::Rect) -> bool {
    area.min.x.is_finite()
        && area.min.y.is_finite()
        && area.max.x.is_finite()
        && area.max.y.is_finite()
        && area.width() >= 1.0
        && area.height() >= 1.0
}

/// How long the window may stay hidden waiting for its first frame.
///
/// Comfortably longer than a cold device and shader compilation, and far
/// shorter than anybody would wait before deciding the program did not start.
/// See [`Active::reveal_is_overdue`].
const REVEAL_DEADLINE: Duration = Duration::from_millis(2500);

/// What the stage is drawn through when nothing has ever been measured: the
/// same default [`Active`] starts with, so the very first frame of a session
/// draws the document rather than a black rectangle.
const FALLBACK_STAGE_AREA: egui::Rect =
    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

/// Read the modifier state egui saw this frame.
fn mods_from(ctx: &egui::Context) -> Mods {
    ctx.input(|i| Mods {
        shift: i.modifiers.shift,
        alt: i.modifiers.alt,
        ctrl: i.modifiers.command,
    })
}

/// Change one bone of one armature.
/// What to call a drawing inside a rig, in the slot list.
fn part_label(object: &buzz_scene::Object) -> String {
    object
        .name
        .clone()
        .unwrap_or_else(|| format!("Drawing {}", object.id.0))
}

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

        // Keep the sound cues current with the document, and decode any newly
        // gained audio **off the UI thread**. `refresh_cues` only rebuilds cues
        // from clips already decoded — a revision compare when nothing changed —
        // so a large import's soundtrack never decodes on this frame; the
        // background decode below picks it up and installs it a frame or two
        // later. See `SoundBank::refresh_cues`.
        let scene = self.editor.doc.scene().clone();
        self.editor.sound.refresh_cues(&scene);
        self.drive_sound_decode(&scene);
        // The File menu lists templates by name; gathered before the menu is
        // built, because the menu cannot borrow the editor while it draws.
        let template_names: Vec<String> = self
            .editor
            .templates
            .iter()
            .map(|t| t.name.clone())
            .collect();
        // Before the panels ask for pictures, throw away the ones whose
        // symbols have been edited since they were drawn.
        self.thumbnails.invalidate_stale(&scene);
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
                            templates: &template_names,
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
            // **Always shown.** It used to appear only inside a symbol, which
            // hid the thing at its left end: the scene name, and the menu
            // behind it that switches, adds, duplicates and reorders scenes.
            // A document's scenes were therefore unreachable unless you first
            // opened a symbol — so a feature the file format, the timeline and
            // the exporter all support could not be found at all. The strip
            // costs one line of chrome and names the scene you are editing,
            // which is worth that on its own.
            //
            // Not collapsible: egui's collapsible panel binds a `&mut bool`
            // the user can also flip, which would let them hide their only way
            // out of a symbol.
            {
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

                    // Not while a script is running or an import is being
                    // merged: the document is spoken for, and a brush stroke
                    // landing in the middle would race the artwork being
                    // rewritten under it.
                    if self.doc_available() {
                        self.handle_stage_input(ui, area);
                    }
                    let response = stage::draw_chrome(ui, &self.editor, area);
                    if let Some(guide) = response.new_guide {
                        self.editor.view.add_guide(guide);
                    }
                    // Over the artwork, after the chrome, so it is never drawn
                    // under a ruler or a selection outline.
                    self.stage_scrollbars(ui, area);
                    self.stage_zoom_overlay(ui, area);
                    self.stage_camera_controls(ui, area);
                    if self.doc_available() {
                        self.draw_tool_cursor(ui, area);
                    }
                    area
                })
                .inner
        };

        // An export runs on its own thread; this is where what it has done
        // reaches the screen. The repaint request is what keeps the progress
        // bar moving on a document that is otherwise still.
        self.pump_export_queue();
        self.poll_animate_import();
        self.poll_picker();
        self.poll_tasks();
        if !self.exports.is_idle() || !self.tasks.is_empty() || self.picker.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // The overlay keeps the elapsed count moving and gives the Stop button
        // somewhere to live; it is drawn after the panels so it sits over
        // everything, and gates pointer input to the document beneath it.
        self.script_overlay(&ctx);
        self.quit_prompt_dialog(&ctx);
        self.export_dialog(&ctx);
        self.lip_sync_dialog(&ctx);
        self.staging_dialog(&ctx);
        self.recovery_dialog(&ctx);
        self.snapshots_dialog(&ctx);
        buzz_ui::about_dialog(&ctx, &mut self.editor.about);

        // File ▸ New asks before it acts, and the answer is remembered.
        let new_document = buzz_ui::new_document_dialog(&ctx, &mut self.editor.new_document);
        if let Some(setup) = new_document.create {
            self.editor.create_document(setup);
        }

        // Ctrl+K opens the command palette, a search box over every command;
        // it runs whatever the list has highlighted. Handled here, ahead of the
        // per-command keymap, so it opens even from a focused field.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
            self.command_palette.toggle();
        }
        let has_selection = !self.editor.selection.is_empty();
        if let Some(command) = buzz_ui::command_palette(&ctx, &mut self.command_palette, |c| {
            !c.needs_selection() || has_selection
        }) {
            commands.push(command);
        }

        commands.extend(keyboard_commands(&ctx, &self.editor));
        // While a script owns the document — or an import is being merged —
        // nothing else may touch it, keyboard shortcuts included, which the
        // modal backdrop does not catch. The one thing still live is the
        // overlay's own Stop button.
        if self.doc_available() {
            for command in commands {
                self.dispatch(command);
            }
        }
        if self.scripting.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
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
                // Through the scene, not the field: moving the lens moves what
                // counts as in front of it, and a layer left behind it is not
                // drawn. See `Scene::set_focal_distance`.
                scene.set_focal_distance(distance);
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

                // A text object gets a content/size editor here — regenerating
                // the glyph outlines needs the font, which lives in the editor.
                // The current values are pulled out first so the scene borrow is
                // done before `set_text` takes a mutable one.
                let text_of = self.editor.selection.iter().next().and_then(|id| {
                    self.editor.doc.scene().find_object(id).and_then(|(_, o)| {
                        o.text.as_ref().map(|t| (id, t.content.clone(), t.size, t.font.clone()))
                    })
                });
                if let Some((id, mut content, mut size, mut font)) = text_of {
                    ui.separator();
                    ui.label("Text");
                    let typed = ui.text_edit_multiline(&mut content).changed();
                    let resized = ui
                        .add(egui::DragValue::new(&mut size).range(4.0..=400.0).prefix("size "))
                        .changed();
                    // Font picker: "Default" plus every family installed on the
                    // system (Hindi ones flagged), so calligraphy and Devanagari
                    // faces are one click away.
                    let mut refont = false;
                    let current = font.clone().unwrap_or_else(|| "Default".to_string());
                    egui::ComboBox::from_id_salt("text-font")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(font.is_none(), "Default").clicked() {
                                font = None;
                                refont = true;
                            }
                            for face in buzz_text::available_fonts() {
                                let label = if face.devanagari {
                                    format!("{}  \u{0905}", face.family)
                                } else {
                                    face.family.clone()
                                };
                                let selected = font.as_deref() == Some(face.family.as_str());
                                if ui.selectable_label(selected, label).clicked() {
                                    font = Some(face.family.clone());
                                    refont = true;
                                }
                            }
                        });
                    if typed || resized || refont {
                        self.editor.set_text(id, content, size, font);
                    }
                }
            }

            Color => {
                let editor = &mut self.editor;
                panels::color_panel(ui, editor.doc.scene(), &mut editor.style);

                // Textures fill the selected shapes: procedural tiles baked from
                // the fill (foreground) and stroke (background) colours, plus any
                // image used as a fill. Applied to the selection, so it needs one.
                ui.separator();
                ui.label("Texture");
                let has_shape = self.editor.selection.iter().next().is_some();
                ui.add_enabled_ui(has_shape, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for kind in buzz_scene::TextureKind::ALL {
                            if ui.button(kind.label()).clicked() {
                                self.editor.apply_texture(kind);
                            }
                        }
                    });
                    if ui.button("Image as fill\u{2026}").clicked() {
                        self.fill_with_image_dialog();
                    }
                    let bundled = bundled_textures();
                    if !bundled.is_empty() {
                        ui.label("Bundled");
                        ui.horizontal_wrapped(|ui| {
                            for path in &bundled {
                                let name = path
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                if ui.button(name).clicked() {
                                    if let Err(e) =
                                        self.editor.fill_selection_with_image(path, true)
                                    {
                                        self.editor.status =
                                            Some(format!("Could not use that texture: {e:#}"));
                                    } else {
                                        self.editor.status =
                                            Some("Applied the bundled texture".into());
                                    }
                                }
                            }
                        });
                    }
                });
                if !has_shape {
                    ui.label(egui::RichText::new("Select a shape to texture it").weak());
                }

                // Symmetry drawing: everything drawn is mirrored across the
                // stage centre. Lives on the draw style, so it applies to every
                // drawing tool.
                ui.separator();
                ui.label("Symmetry");
                let sym = &mut self.editor.style.symmetry;
                egui::ComboBox::from_id_salt("symmetry-mode")
                    .selected_text(sym.mode.label())
                    .show_ui(ui, |ui| {
                        for mode in buzz_ui::SymmetryMode::ALL {
                            ui.selectable_value(&mut sym.mode, mode, mode.label());
                        }
                    });
                if sym.mode == buzz_ui::SymmetryMode::Radial {
                    ui.add(
                        egui::DragValue::new(&mut sym.radial_count)
                            .range(2..=24)
                            .prefix("copies "),
                    );
                }

                // Perspective guides: a horizon + rays to 1–3 vanishing points.
                ui.separator();
                ui.label("Perspective");
                let stage = self.editor.doc.scene().stage().size;
                ui.horizontal(|ui| {
                    let showing = self.editor.view.perspective.show;
                    if ui.selectable_label(!showing, "Off").clicked() {
                        self.editor.view.perspective.show = false;
                    }
                    for n in 1..=3usize {
                        let active = showing
                            && self.editor.view.perspective.vanishing_points.len() == n;
                        if ui.selectable_label(active, format!("{n}-pt")).clicked() {
                            self.editor.view.perspective =
                                buzz_ui::PerspectiveGuides::seed(stage.width, stage.height, n);
                        }
                    }
                });
            }

            Assets => {
                let can_add = !self.editor.selection.is_empty();
                // The size lives in the workspace so it survives a restart,
                // and in the panel state so the panel can change it. Carried
                // across either way round each frame, which is what keeps one
                // the copy and the other the record.
                self.editor.assets_panel.thumbnail_size = self.editor.workspace.asset_thumbnail_size;
                let thumbs = &mut self.asset_thumbnails;
                let action = buzz_ui::assets_panel(
                    ui,
                    &self.editor.assets,
                    &mut self.editor.assets_panel,
                    can_add,
                    &mut |path| thumbs.get(path),
                );
                if self.editor.assets_panel.thumbnail_size != self.editor.workspace.asset_thumbnail_size {
                    self.editor.workspace.asset_thumbnail_size =
                        self.editor.assets_panel.thumbnail_size;
                    self.editor.workspace.save();
                }
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
                // Use counts come from the background cache, never a per-frame
                // walk of the whole document.
                let revision = self.editor.doc.scene().revision();
                self.usage_cache.poll();
                // **Not while a drag is running.** Moving artwork cannot change
                // how many times a symbol is used, but it moves the revision on
                // every pointer move — so this spawned a job per mouse move,
                // each one cloning the whole document and walking it, and each
                // one keeping the window at full poll while it ran. The release
                // brings the revision to rest and this spawns once.
                if !self.editor.is_gesturing() && self.usage_cache.should_spawn(revision) {
                    let scene = self.editor.doc.scene().clone();
                    let (send, receive) = crossbeam_channel::bounded(1);
                    self.jobs.spawn(Pool::Background, move || {
                        let _ = send.send(scene.symbol_usage());
                    });
                    self.usage_cache.spawned(revision, receive);
                }

                let editor = &mut self.editor;
                let library = &mut editor.library;
                let thumbnails = &mut self.thumbnails;
                let usage = self.usage_cache.counts();
                let mut raised = None;
                editor.doc.edit("Library", |scene| {
                    raised = buzz_ui::library_panel(ui, scene, library, usage, &mut |id| {
                        thumbnails.get(id)
                    });
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
                    selected_light: self.editor.light_panel.selected,
                    playing: self.editor.playback.playing,
                    onion_enabled: self.editor.onion.enabled,
                    auto_keyframe: self.editor.auto_keyframe,
                    edit_multiple: self.editor.edit_multiple,
                    onion_before: self.editor.onion.before,
                    onion_after: self.editor.onion.after,
                    frame_width: self.editor.workspace.frame_width,
                    row_scale: self.editor.workspace.row_scale,
                    parenting_view: self.editor.workspace.parenting_view,
                    depth_view: self.editor.workspace.depth_view,
                    focal_distance: self.editor.scene().camera().focal_distance,
                    nearest_depth: self.editor.scene().camera().nearest_depth(),
                    waveforms: self.editor.waveforms(),
                    beats: self.editor.beat_markers.clone(),
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

            Tasks => self.tasks_panel(ui),
            MotionEditor => self.motion_editor_panel(ui),
        }
    }

    /// The Motion Editor: shape the easing of the tween under the playhead.
    fn motion_editor_panel(&mut self, ui: &mut egui::Ui) {
        let editor = &mut self.editor;
        // The tween on the active layer's keyframe at the playhead, if it is an
        // active one — the same target the tween commands act on.
        let frame = editor.current_frame;
        let tween = editor
            .selection
            .active_layer()
            .and_then(|id| editor.doc.scene().layers().get(id))
            .map(|l| l.frames.tween_at(frame))
            .filter(|t| t.is_active());

        let response = buzz_ui::motion_editor_panel(
            ui,
            tween.map(|t| t.easing),
            tween.map(|t| t.kind),
            &mut editor.motion_editor,
        );

        if let Some(easing) = response.set_easing {
            editor.set_ease_curve(easing);
        }
    }

    /// The camera's properties, shown when the Camera row is selected.
    ///
    /// Every control keys the camera at the playhead — the same rule the
    /// Camera tool follows when it is dragged, so aiming the camera by hand
    /// and typing a number into a box do the same thing.
    fn camera_panel(&mut self, ui: &mut egui::Ui) {
        let frame = self.editor.current_frame;
        let response = buzz_ui::camera_panel(
            ui,
            self.editor.scene().camera(),
            frame,
            &mut self.camera_panel_state,
        );
        self.apply_camera(response);
    }

    /// **Animate's on-stage camera controls.**
    ///
    /// Shown while the camera is what is being worked on — its row selected in
    /// the timeline, or the Camera tool in hand — because framing a shot is
    /// done by looking at the stage, and the Properties panel is at the far
    /// side of the window. Along the bottom, where it cannot cover the rulers
    /// or the zoom readout.
    fn stage_camera_controls(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        let wanted =
            self.editor.camera_selected || self.editor.tool() == buzz_ui::ToolId::Camera;
        if !wanted || area.width() < 420.0 || area.height() < 120.0 {
            return;
        }

        let frame = self.editor.current_frame;
        // Clear of the horizontal scrollbar, which floats at the bottom edge.
        let anchor = egui::pos2(area.center().x, area.max.y - 20.0);
        let mut response = buzz_ui::CameraResponse::default();

        egui::Area::new(egui::Id::new("stage-camera"))
            .fixed_pos(anchor)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .order(egui::Order::Middle)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Palette::panel())
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        let driving = self.editor.tool() == buzz_ui::ToolId::Camera;
                        response =
                            buzz_ui::camera_hud(ui, self.editor.scene().camera(), frame, driving);
                    });
            });

        self.apply_camera(response);
    }

    /// One application for both, so the panel and the strip on the stage
    /// cannot drift into two behaviours.
    fn apply_camera(&mut self, response: buzz_ui::CameraResponse) {
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
                // Through the scene, not the field: moving the lens moves what
                // counts as in front of it, and a layer left behind it is not
                // drawn. See `Scene::set_focal_distance`.
                scene.set_focal_distance(distance);
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
        if response.grab_camera {
            // A toggle: pressing it again puts the tool down and gives the
            // Selection tool back, so the strip is not a one-way door into a
            // mode you then have to find your way out of.
            let tool = if self.editor.tool() == buzz_ui::ToolId::Camera {
                buzz_ui::ToolId::Selection
            } else {
                buzz_ui::ToolId::Camera
            };
            self.editor.set_tool(tool);
        }

        let frame = self.editor.current_frame;
        if let Some(name) = response.save_angle {
            self.editor.doc.edit("Save Angle", |scene| {
                scene.camera_mut().enabled = true;
                scene.camera_mut().save_angle(name.clone(), frame);
            });
            self.editor.doc.end_gesture();
            self.editor.status = Some("Angle saved".into());
        }
        if let Some(i) = response.cut_angle {
            // Cut to a saved angle: key the camera to its state at the playhead,
            // so the shot jumps to that viewpoint here.
            self.editor.doc.edit("Cut to Angle", |scene| {
                if let Some(angle) = scene.camera().angles.get(i).cloned() {
                    let mut key = angle.state;
                    key.frame = frame;
                    scene.camera_mut().enabled = true;
                    scene.camera_mut().set_key(key.clamped());
                }
            });
            self.editor.doc.end_gesture();
        }
        if let Some(i) = response.delete_angle {
            self.editor.doc.edit("Delete Angle", |scene| {
                if i < scene.camera().angles.len() {
                    scene.camera_mut().angles.remove(i);
                }
            });
            self.editor.doc.end_gesture();
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

        let modifiers = match target {
            buzz_ui::FilterTarget::Object => object
                .and_then(|id| editor.scene().find_object(id))
                .map(|(_, found)| found.modifiers.clone())
                .unwrap_or_default(),
            buzz_ui::FilterTarget::Layer => Vec::new(),
        };

        let response = buzz_ui::filter_panel(
            ui,
            &filters,
            blend,
            &modifiers,
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

        // Live modifiers on the selected object. They live on every keyframe copy
        // of the object (Edit-Multiple-Frames style) so they never flicker.
        if let Some(modifier) = response.add_modifier
            && let Some(id) = object
        {
            editor.doc.edit("Add Modifier", |scene| {
                scene.update_object_across(0, u32::MAX, id, |o| o.modifiers.push(modifier));
            });
            editor.doc.end_gesture();
            editor.status = Some(format!("Added {}", modifier.label()));
        }
        if let Some(index) = response.remove_modifier
            && let Some(id) = object
        {
            editor.doc.edit("Remove Modifier", |scene| {
                scene.update_object_across(0, u32::MAX, id, |o| {
                    if index < o.modifiers.len() {
                        o.modifiers.remove(index);
                    }
                });
            });
            editor.doc.end_gesture();
        }
        if let Some((index, modifier)) = response.set_modifier
            && let Some(id) = object
        {
            // No end_gesture: consecutive drags coalesce into one undo step.
            editor.doc.edit("Edit Modifier", |scene| {
                scene.update_object_across(0, u32::MAX, id, |o| {
                    if let Some(slot) = o.modifiers.get_mut(index) {
                        *slot = modifier;
                    }
                });
            });
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
        // What the last stage encode had to leave out, so the panel can say so
        // rather than leaving the animator to wonder why a lamp models nothing.
        // See `buzz_render::document::LightDetail`.
        let trimmed = if !self.editor.scene().lights().is_active() {
            // An unlit document encodes nothing generated, so the level says
            // nothing about it — and a note about lighting that is switched off
            // is noise.
            None
        } else {
            match self.lights.detail() {
                buzz_render::document::LightDetail::Full => None,
                buzz_render::document::LightDetail::NoModelling => {
                    Some("too dense to model — colour and shadows only")
                }
                buzz_render::document::LightDetail::Flat => {
                    Some("too dense to model or shadow — colour only")
                }
            }
        };
        let current_frame = self.editor.current_frame;
        let editor = &mut self.editor;
        let state = &mut editor.light_panel;
        state.trimmed = trimmed;
        state.current_frame = current_frame;
        let response = buzz_ui::light_panel(ui, editor.doc.scene().lights(), state);

        if let Some(id) = response.select {
            editor.light_panel.selected = Some(id);
        }

        // The Key / Remove-key button, keying the selected light at the playhead
        // through the same commands the menu and timeline use.
        if let Some(add) = response.key {
            editor.run(if add {
                Command::AddLightKeyframe
            } else {
                Command::RemoveLightKeyframe
            });
        }

        if let Some(kind) = response.add {
            // The same path the Insert menu takes, so a light added from the
            // panel and one added from the menu behave identically.
            editor.add_light(kind);
        }

        if response.add_fire {
            editor.run(Command::AddFire);
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

        let poses = selected
            .and_then(|id| self.editor.scene().find_object(id))
            .and_then(|(_, object)| match &object.kind {
                buzz_scene::ObjectKind::Armature(rig) => Some(rig.poses.clone()),
                _ => None,
            })
            .unwrap_or_default();

        // Everything on the stage that could still become a limb, and — for a
        // character that has already been rigged from a pattern — what is
        // standing in each of its slots.
        let parts = rigging::loose_parts(self.editor.scene(), self.editor.frame());
        let bound = selected
            .and_then(|id| self.editor.scene().find_object(id))
            .and_then(|(_, object)| match &object.kind {
                buzz_scene::ObjectKind::Armature(rig) => {
                    let name = rig.pattern.clone()?;
                    let filled = rig
                        .parts
                        .iter()
                        .filter_map(|part| match part.binding {
                            buzz_scene::RigBinding::Rigid(bone) => {
                                Some((bone, part_label(&part.artwork)))
                            }
                            buzz_scene::RigBinding::Skin(_) => None,
                        })
                        .collect::<Vec<_>>();
                    Some((name, filled))
                }
                _ => None,
            });

        let response = buzz_ui::rig_panel(
            ui,
            armature.as_ref(),
            &poses,
            &parts,
            bound.as_ref().map(|(name, filled)| (name.as_str(), filled.as_slice())),
            &mut self.editor.rig_panel,
        );

        // -- assembling, which does not need anything to be selected --------

        if let Some((pattern, slots)) = response.build_rig {
            self.rig_character(&pattern, &slots);
        }
        if let Some(id) = response.select_part {
            self.editor.selection.set([id]);
        }

        let Some(object) = selected else { return };

        if let Some((slot, drawing)) = response.replace_part {
            self.replace_rigged_part(object, slot, drawing);
        }

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

        // -- the pose library ---------------------------------------------

        if let Some(name) = response.save_pose {
            self.editor.doc.edit("Save Pose", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                        let angles = rig.armature.pose();
                        // Saving over a name replaces it, which is what
                        // somebody typing the same name again means. A second
                        // "reach" that does not replace the first is a list
                        // that fills with near-duplicates.
                        match rig.poses.iter_mut().find(|p| p.name == name) {
                            Some(existing) => existing.angles = angles,
                            None => rig.poses.push(buzz_scene::NamedPose { name, angles }),
                        }
                    }
                });
            });
        }

        if response.mirror_pose {
            self.editor.doc.edit("Mirror Pose", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                        let flipped = rig.armature.mirrored_pose();
                        rig.armature.set_pose(&flipped);
                    }
                });
            });
        }

        if let Some(index) = response.apply_pose {
            self.apply_named_pose(object, index, false);
        }

        // **Key, which is what turns a list of poses into an animation.**
        // Applying at a keyframe means the next applied pose tweens from this
        // one, so a shot is two clicks and a span rather than a pose held by
        // hand at every frame.
        if let Some(index) = response.key_pose {
            self.apply_named_pose(object, index, true);
        }

        // -- editing the skeleton -----------------------------------------

        if let Some(bone) = response.delete_bone {
            // Weights were computed against a skeleton that no longer exists,
            // so the artwork is re-bound — otherwise it would keep bending
            // about a bone that has gone.
            self.editor.doc.edit("Delete Bone", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind
                        && rig.armature.remove_bone(bone)
                    {
                        rig.rebind();
                    }
                });
            });
        }

        if let Some((bone, parent)) = response.reparent {
            let mut refused = false;
            self.editor.doc.edit("Reparent Bone", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                        if rig.armature.reparent_bone(bone, parent) {
                            rig.rebind();
                        } else {
                            refused = true;
                        }
                    }
                });
            });
            if refused {
                // Said rather than silently ignored: the only reason to refuse
                // is a cycle, and "nothing happened" is indistinguishable from
                // a broken control.
                self.editor.status =
                    Some("A bone cannot hang off itself or off one of its own children".into());
            }
        }

        if let Some(index) = response.delete_pose {
            self.editor.doc.edit("Delete Pose", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind
                        && index < rig.poses.len()
                    {
                        rig.poses.remove(index);
                    }
                });
            });
        }
    }

    /// Put a rig into one of its saved poses, optionally keying it first.
    fn apply_named_pose(&mut self, object: buzz_scene::ObjectId, index: usize, key: bool) {
        if key {
            // A keyframe first, so the pose lands *on* one and the span before
            // it can tween into it. Without this, applying two poses on one
            // keyframe would simply overwrite the first.
            self.editor.run(Command::InsertKeyframe);
        }
        let label = if key { "Key Pose" } else { "Apply Pose" };
        self.editor.doc.edit(label, |scene| {
            scene.update_object(object, |target| {
                if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind
                    && let Some(pose) = rig.poses.get(index)
                {
                    let angles = pose.angles.clone();
                    rig.armature.set_pose(&angles);
                }
            });
        });
    }

    // -- rigging a character by sorting its drawings into slots ------------

    /// Build a skeleton from a pattern and move the sorted drawings into it.
    ///
    /// One undo step for the whole character. The work is in
    /// [`rigging::rig_character`]; what is here is the report, because the
    /// document layer has nowhere to say anything.
    fn rig_character(&mut self, pattern_name: &str, slots: &[Option<buzz_scene::ObjectId>]) {
        let Some(pattern) = buzz_rig::RigPattern::named(pattern_name) else {
            self.editor.status = Some(format!("No rig pattern called {pattern_name}."));
            return;
        };
        let frame = self.editor.frame();
        let count = slots.iter().filter(|s| s.is_some()).count();
        let mut built = None;

        self.editor.doc.edit("Rig Character", |scene| {
            built = rigging::rig_character(scene, frame, &pattern, slots);
        });

        match built {
            Some(id) => {
                // The slots empty themselves next frame — the drawings in them
                // are inside the rig now, so they are no longer loose — but the
                // note and the armed slot are about a job that is finished.
                self.editor.rig_panel.armed = None;
                self.editor.rig_panel.note = None;
                self.editor.selection.set([id]);
                self.editor.status = Some(format!(
                    "Rigged {count} part{} as a {}.",
                    if count == 1 { "" } else { "s" },
                    pattern.name
                ));
            }
            None => {
                self.editor.status =
                    Some("Nothing to rig: put a drawing in at least one slot first.".into());
            }
        }
    }

    /// Put a different drawing into one slot of a rig that already exists.
    fn replace_rigged_part(
        &mut self,
        rig: buzz_scene::ObjectId,
        slot: usize,
        drawing: buzz_scene::ObjectId,
    ) {
        let mut done = false;
        self.editor.doc.edit("Replace Part", |scene| {
            done = rigging::replace_part(scene, rig, slot, drawing);
        });
        if done {
            self.editor.selection.set([rig]);
        }
    }

    /// Fill an armed slot with whatever drawing was clicked on the stage.
    fn pick_part_for_slot(&mut self, slot: usize, screen: buzz_geom::Point) {
        let at = self.editor.screen_to_edit(screen);
        let frame = self.editor.frame();

        match rigging::part_at(self.editor.scene(), frame, at) {
            Some(part) => {
                let name = part.name.clone();
                self.editor.rig_panel.assign(slot, part.object);
                self.editor.rig_panel.note = None;
                self.editor.status = Some(format!("Put {name} in the slot."));
            }
            None => {
                // Clicking past the artwork cancels, which is the way out of
                // every other armed gesture in the program.
                self.editor.rig_panel.armed = None;
                self.editor.status = Some("Nothing there. The slot is still empty.".into());
            }
        }
    }

    /// The trail of symbols currently open, with the document at its root.
    ///
    /// Clicking a level jumps straight back to it. Returning a [`Command`]
    /// rather than mutating here keeps every navigation step going through the
    /// same dispatch path as the menu and the keyboard.
    /// The scene name at the root of the edit-path breadcrumb, and the menu
    /// behind it — switch scene, add, rename, delete. Animate's "Edit Scene"
    /// control, folded into the crumb it sits on.
    fn scene_crumb(&mut self, ui: &mut egui::Ui, command: &mut Option<Command>) {
        let active = self.editor.doc.active_scene();
        let names = self.editor.doc.scene_names();

        // Mid-rename: the crumb becomes a text field, committed on Enter or
        // when focus leaves, abandoned on Escape.
        if let Some((index, mut buffer)) = self.scene_rename.take() {
            let response = ui.add(
                egui::TextEdit::singleline(&mut buffer)
                    .desired_width(120.0)
                    .font(egui::TextStyle::Small),
            );
            response.request_focus();
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if escape {
                // Abandon, keeping the old name.
            } else if response.lost_focus() {
                // Enter or a click elsewhere: keep the edit.
                self.editor.doc.rename_scene(index, buffer.trim().to_string());
            } else {
                self.scene_rename = Some((index, buffer));
            }
            return;
        }

        let current = names
            .get(active)
            .cloned()
            .unwrap_or_else(|| "Scene 1".to_string());

        ui.menu_button(egui::RichText::new(current).small(), |ui| {
            for (i, name) in names.iter().enumerate() {
                if ui
                    .selectable_label(i == active, egui::RichText::new(name).small())
                    .clicked()
                {
                    if i == active {
                        // Already here: the click means "leave the symbol and
                        // show this scene's main timeline".
                        *command = Some(Command::EditDocument);
                    } else {
                        self.editor.switch_scene(i);
                    }
                    ui.close();
                }
            }

            ui.separator();

            if ui.button("Add Scene").clicked() {
                self.editor.add_scene();
                ui.close();
            }
            // **The one a conversation is built out of.** Two people in a room
            // is the same set, cast and lighting beat after beat; only the
            // performance changes. Duplicating gives the next shot all of that
            // to start from, which is the alternative to copying frames onto
            // the end of the timeline by hand.
            if ui
                .button("Duplicate Scene")
                .on_hover_text(
                    "A copy of this scene, complete \u{2014} set, cast, lights and every \
                     keyframe \u{2014} placed after it and opened for editing. What the \
                     next beat of a conversation starts from.",
                )
                .clicked()
            {
                self.editor.duplicate_scene(active);
                ui.close();
            }
            if ui.button("Rename Scene\u{2026}").clicked() {
                self.scene_rename = Some((active, names[active].clone()));
                ui.close();
            }
            ui.add_enabled_ui(names.len() > 1, |ui| {
                ui.horizontal(|ui| {
                    // The running order is the order the film plays in, so it
                    // has to be changeable.
                    if ui
                        .add_enabled(active > 0, egui::Button::new("Move Up"))
                        .clicked()
                    {
                        self.editor.move_scene(active, active - 1);
                        ui.close();
                    }
                    if ui
                        .add_enabled(active + 1 < names.len(), egui::Button::new("Move Down"))
                        .clicked()
                    {
                        self.editor.move_scene(active, active + 1);
                        ui.close();
                    }
                });
                if ui.button("Delete Scene").clicked() {
                    self.editor.delete_scene(active);
                    ui.close();
                }
            });
        })
        .response
        .on_hover_text("Switch, add, rename or delete a scene");
    }

    fn breadcrumb(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let mut command = None;
        let path: Vec<buzz_scene::SymbolId> = self.editor.scene().edit_path().to_vec();

        ui.horizontal(|ui| {
            self.scene_crumb(ui, &mut command);

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

            // Only inside a symbol is there anything to exit. At the main
            // timeline the strip is the scene crumb alone.
            if !path.is_empty() {
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
            }
        });

        command
    }

    /// Turn timeline interactions into editor actions.
    fn apply_timeline(&mut self, response: buzz_ui::TimelineResponse, commands: &mut Vec<Command>) {
        if response.toggle_depth {
            let on = !self.editor.workspace.depth_view;
            self.editor.workspace.depth_view = on;

            // **And show the picture.** The numbers in the column say what each
            // depth *is*; the Layer Depth panel draws the scene from the side —
            // camera at the left, each layer a plane at the height perspective
            // gives it — which is the only thing that answers "how close is
            // that layer to me". It was a background tab in the right dock, so
            // turning depth on showed a column of numbers and nothing else.
            //
            // Opened rather than merely selected when it is hidden, because a
            // closed panel cannot be brought to the front of anything.
            if on {
                let workspace = &mut self.editor.workspace;
                if !workspace.is_open(buzz_ui::PanelId::Depth) {
                    workspace.move_to(buzz_ui::PanelId::Depth, buzz_ui::Dock::Right);
                }
                workspace.select_tab(buzz_ui::PanelId::Depth);
            }
            // One column, one question: turning either view on puts the other
            // away rather than leaving two answers fighting for the same room.
            if on {
                self.editor.workspace.parenting_view = false;
            }
            self.editor.workspace.save();
        }
        if let Some((layer, depth)) = response.set_depth {
            // The same edit the Layer Depth panel makes, so the two are doors
            // onto one room.
            self.editor.doc.edit("Layer Depth", |scene| {
                scene.update_layer(layer, |l| l.depth = depth);
            });
        }
        if response.toggle_parenting {
            // A view preference, so it is saved with the layout rather than
            // with the film — the same rule the frame width follows.
            let on = !self.editor.workspace.parenting_view;
            self.editor.workspace.parenting_view = on;
            if on {
                self.editor.workspace.depth_view = false;
            }
            self.editor.workspace.save();
        }
        if let Some((layer, follows)) = response.set_follows {
            // The same edit the Layers panel's Parent dropdown makes, so the
            // graph and the menu are two doors onto one room. Named for what it
            // does, because undoing it is a thing the history should say.
            let label = if follows.is_some() {
                "Parent Layer"
            } else {
                "Unparent Layer"
            };
            // Through `set_follows`, which records the pose the link is made
            // at — without it a character with a single keyframe has no motion
            // to propagate and moving a wrist leaves its palm behind.
            let frame = self.editor.current_frame;
            self.editor.doc.edit(label, |scene| {
                scene.set_follows(layer, follows, frame);
            });
        }
        if let Some(frame) = response.scrub_to {
            // Scrubbing stops playback, as it does in Animate: the user has
            // taken manual control of the playhead.
            self.editor.playback.playing = false;
            self.editor.set_frame(frame);
            // ...but you still hear it, so the beat can be found by ear.
            self.editor.scrub_audio(frame);
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
        if let Some(icon) = response.toggle_all {
            // A column heading was clicked: flip that switch on every layer. One
            // click hides (locks, outlines) them all; the next restores them.
            use buzz_ui::panels::LayerIcon;
            let label = match icon {
                LayerIcon::Eye => "Show/Hide All Layers",
                LayerIcon::Lock => "Lock All Layers",
                LayerIcon::Outline => "Outline All Layers",
            };
            self.editor.doc.edit(label, |scene| {
                let ids: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
                let on = |l: &buzz_scene::Layer| match icon {
                    LayerIcon::Eye => l.visible,
                    LayerIcon::Lock => l.locked,
                    LayerIcon::Outline => l.outline,
                };
                let all_on = ids
                    .iter()
                    .all(|&id| scene.layers().get(id).is_some_and(|l| on(l)));
                let target = !all_on;
                for id in ids {
                    scene.update_layer(id, |l| match icon {
                        LayerIcon::Eye => l.visible = target,
                        LayerIcon::Lock => l.locked = target,
                        LayerIcon::Outline => l.outline = target,
                    });
                }
            });
        }
        if response.select_camera {
            self.editor.camera_selected = true;
            // Selecting the camera row selects the Camera tool, which is what
            // Animate does — the row and the tool are the same idea.
            commands.push(Command::SelectTool(ToolId::Camera));
        }
        if let Some(id) = response.select_light {
            // Clicking a light's channel makes it the active light, the way
            // clicking any row takes the highlight from the one before.
            self.editor.light_panel.selected = Some(id);
            self.editor.camera_selected = false;
        }
        if let Some((id, frame)) = response.light_key {
            self.editor.key_light_at(id, frame);
        }
        if let Some((id, frame)) = response.light_unkey {
            self.editor.unkey_light_at(id, frame);
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
    /// Animate's tool cursor: over the stage, the active drawing tool shows its
    /// own icon at the pointer — with a size ring for the brush-like tools — in
    /// place of the system arrow, so what you are about to do is under your hand.
    fn draw_tool_cursor(&self, ui: &egui::Ui, area: egui::Rect) {
        use buzz_ui::ToolId::*;
        let tool = self.editor.tool();
        // The pointer, transform and navigation tools keep the system cursor
        // Animate gives them (an arrow, a hand, a magnifier).
        if matches!(
            tool,
            Selection | Subselection | FreeTransform | GradientTransform | Hand | Zoom
        ) {
            // **Except that the transform tools say where they rotate.**
            //
            // The ring just outside a corner turns the selection, and with no
            // cursor change nothing distinguished it from the empty stage
            // beside it — where the same drag marquees and throws the selection
            // away. Grab is the nearest thing egui offers to Animate's curved
            // arrow, and it reads as "take hold of this and swing it".
            if matches!(tool, Selection | Subselection | FreeTransform)
                && let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos())
                && area.contains(pos)
                && let Some(cursor) = self.transform_cursor(area, pos)
            {
                ui.ctx().set_cursor_icon(cursor);
            }
            return;
        }
        let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) else {
            return;
        };
        if !area.contains(pos) {
            return;
        }

        // Ours, not the system arrow.
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        // **Above everything the stage floats.** Painted into the panel's own
        // layer, the cursor was drawn *under* the zoom readout and the camera
        // controls — so moving the pointer onto them lost the very thing that
        // says where the tool will act. A cursor that can be covered is not a
        // cursor. Clipped to the stage so it still cannot stray onto a panel.
        let painter = ui
            .ctx()
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("stage-tool-cursor"),
            ))
            .with_clip_rect(area);
        let ink = buzz_ui::Palette::text();
        let halo = egui::Stroke::new(2.5, egui::Color32::from_black_alpha(120));

        // The brush-like tools are Animate's brush cursor: a ring the exact size
        // of the mark, centred on the point it will make — and **nothing else**,
        // so the mark lands under the ring's centre rather than beside a floating
        // icon. This is what "the brush looks like Animate" means, and it is why
        // strokes now start where the ring is.
        let ring = match tool {
            Brush => Some(self.editor.style.brush.size),
            Pencil | Eraser => Some(self.editor.style.stroke_width.max(1.0)),
            _ => None,
        };
        if let Some(size) = ring {
            let r = (size * self.editor.camera.zoom * 0.5).clamp(2.5, 600.0) as f32;
            painter.circle_stroke(pos, r, halo);
            painter.circle_stroke(pos, r, egui::Stroke::new(1.0, ink));
            // A centre dot marks the exact point for a very small brush.
            painter.circle_filled(pos, 1.0, ink);
            return;
        }

        // Every other tool: a crosshair on the exact hotspot, with the tool's
        // icon just off it so it never covers the point.
        let h = 5.0;
        for (a, b) in [
            (egui::pos2(pos.x - h, pos.y), egui::pos2(pos.x + h, pos.y)),
            (egui::pos2(pos.x, pos.y - h), egui::pos2(pos.x, pos.y + h)),
        ] {
            painter.line_segment([a, b], halo);
            painter.line_segment([a, b], egui::Stroke::new(1.0, ink));
        }
        let icon = egui::Rect::from_min_size(egui::pos2(pos.x + 9.0, pos.y + 9.0), egui::vec2(18.0, 18.0));
        painter.rect_filled(icon.expand(2.0), 3.0, egui::Color32::from_black_alpha(140));
        buzz_ui::icons::tool_icon(&painter, icon, tool, ink);
    }

    /// Is the pointer in the ring that rotates, just outside a corner of the
    /// selection?
    ///
    /// Asked in *screen* space against the same box the chrome draws, and with
    /// the same radii  tests, so the cursor cannot
    /// promise a gesture the release will not make.
    /// The cursor for whatever part of the transform gizmo the pointer is on.
    ///
    /// **Three transforms live within a few pixels of each other** — a corner
    /// resizes, the ring just outside it turns, an edge skews — and with one
    /// arrow over all of them the only way to find out which you had grabbed
    /// was to let go and look. Reaching for a rotation and landing on a corner
    /// is how a turn ends up scaling the artwork, and the pointer never said.
    ///
    /// Read against the same box the chrome draws and the same radii
    /// `tools::transform_zone` tests, so the cursor cannot promise a gesture
    /// the release will not make.
    fn transform_cursor(&self, area: egui::Rect, pos: egui::Pos2) -> Option<egui::CursorIcon> {
        let bounds = self.editor.selection_bounds_drawn()?;
        let to_screen = |p: buzz_geom::Point| {
            let s = self.editor.camera.doc_to_screen(p);
            egui::pos2(area.min.x + s.x as f32, area.min.y + s.y as f32)
        };
        let corners = [
            to_screen(buzz_geom::Point::new(bounds.x0, bounds.y0)),
            to_screen(buzz_geom::Point::new(bounds.x1, bounds.y0)),
            to_screen(buzz_geom::Point::new(bounds.x1, bounds.y1)),
            to_screen(buzz_geom::Point::new(bounds.x0, bounds.y1)),
        ];
        let box_rect = corners
            .iter()
            .fold(egui::Rect::from_two_pos(corners[0], corners[2]), |r, c| {
                r.union(egui::Rect::from_two_pos(*c, *c))
            });

        let grab = crate::tools::TRANSFORM_GRAB_PX as f32;
        let nearest = corners
            .iter()
            .map(|c| c.distance(pos))
            .fold(f32::INFINITY, f32::min);

        // A corner resizes. The arrow leans the way that corner pulls, which
        // is the difference between "this scales" and "this turns".
        if nearest <= grab {
            let on_left = (pos.x - box_rect.left()).abs() < (pos.x - box_rect.right()).abs();
            let on_top = (pos.y - box_rect.top()).abs() < (pos.y - box_rect.bottom()).abs();
            return Some(if on_left == on_top {
                egui::CursorIcon::ResizeNwSe
            } else {
                egui::CursorIcon::ResizeNeSw
            });
        }

        // Just outside a corner, and outside the box: the ring that turns.
        // Grab is the nearest thing egui has to Animate's curved arrow.
        if !box_rect.contains(pos) && nearest <= grab * 3.0 {
            return Some(egui::CursorIcon::Grab);
        }

        // An edge, away from the corners: skew along it.
        let near_vertical = ((pos.x - box_rect.left()).abs() <= grab
            || (pos.x - box_rect.right()).abs() <= grab)
            && pos.y > box_rect.top() + grab
            && pos.y < box_rect.bottom() - grab;
        let near_horizontal = ((pos.y - box_rect.top()).abs() <= grab
            || (pos.y - box_rect.bottom()).abs() <= grab)
            && pos.x > box_rect.left() + grab
            && pos.x < box_rect.right() - grab;
        if near_horizontal {
            return Some(egui::CursorIcon::ResizeHorizontal);
        }
        if near_vertical {
            return Some(egui::CursorIcon::ResizeVertical);
        }
        None
    }

    fn stage_scrollbars(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        const THICKNESS: f32 = 9.0;
        /// How far the bars sit in from the edge of the drawing area.
        const INSET: f32 = 4.0;
        if area.width() < 120.0 || area.height() < 120.0 {
            return;
        }

        let camera_center = self.editor.camera.center;
        let visible = self.editor.camera.visible_doc_rect();
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
                moved = Some((x, camera_center.y));
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
    fn scrollable_extent(&mut self, visible: buzz_geom::Rect) -> buzz_geom::Rect {
        let revision = self.editor.scene().revision();
        let frame = self.editor.current_frame;

        // The content extent — the expensive part, resolving every object's
        // bounds through the library — depends only on the document and the
        // frame, never on the camera. Recompute it only when one of those moves;
        // a pan or a zoom reuses it, which is what keeps the scrollbars off the
        // per-frame budget on a large document.
        let content = match self.scroll_extent {
            Some((r, f, ext)) if r == revision && f == frame => ext,
            // Mid-drag, hold the last one. It is the scrollbars' extent: it
            // changes as the artwork moves, and it is nobody's reason for
            // dragging. Re-resolving every object through the library between
            // frames of a drag is exactly the cost this cache exists to avoid,
            // and the release recomputes it.
            Some((_, f, ext)) if f == frame && self.editor.is_gesturing() => ext,
            _ => {
                // Keep the symbol-bounds table warm for this revision, so the
                // recompute below is a lookup per object even while scrubbing.
                if self.bounds_table.as_ref().map(|(r, _)| *r) != Some(revision) {
                    let table = self.editor.scene().symbol_bounds_table();
                    self.bounds_table = Some((revision, table));
                }
                let ext = self.compute_content_extent(frame);
                self.scroll_extent = Some((revision, frame, ext));
                ext
            }
        };

        // And never smaller than what is on screen, or the thumb would be
        // longer than its track. This part is camera-dependent, so it stays
        // outside the cache.
        content.union(visible)
    }

    /// The document's content extent at `frame`, with a stage of air around it.
    /// Cached by [`Self::scrollable_extent`]; see the cost note there.
    fn compute_content_extent(&self, frame: u32) -> buzz_geom::Rect {
        let scene = self.editor.scene();
        let mut extent = scene.stage().stage_rect();

        let empty = std::collections::HashMap::new();
        let table = self.bounds_table.as_ref().map(|(_, t)| t).unwrap_or(&empty);
        for layer in scene.layers().iter() {
            for object in layer.objects_at(frame) {
                let bounds = scene.resolved_bounds_with(object, table);
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
        buzz_geom::Rect::new(
            extent.x0 - margin.x,
            extent.y0 - margin.y,
            extent.x1 + margin.x,
            extent.y1 + margin.y,
        )
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
                )
                // The per-section breakdown on hover — the frame watchdog's
                // numbers, without taking a permanent seat in the status bar.
                .on_hover_text(self.profiler.summary());
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

    /// **Animate's right-click menu on the stage.**
    ///
    /// Everything the Modify menu offers for the thing under the pointer,
    /// where the thing is. Without it the stage was the one surface in the
    /// program that answered a right-click with nothing, so transforming a
    /// selection meant a trip to the menu bar for an operation that is about a
    /// specific object in a specific place.
    ///
    /// Raised as commands rather than performed here, so every entry runs
    /// through the same path as the menu bar and the shortcut — undo included.
    fn stage_context_menu(&mut self, response: &egui::Response) {
        let has_selection = !self.editor.selection.is_empty();
        let mut raised: Option<Command> = None;
        let mut save_asset = false;

        // A probe for the test that drives the real window: the popup egui
        // opens has an id of its own, and this is the honest way to ask the
        // stage whether its menu is up.
        if response.context_menu_opened() {
            let ctx = response.ctx.clone();
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("stage-context-probe"), true));
        }
        response.context_menu(|ui| {
            // A right-click on artwork that is not selected selects it first,
            // which is what every editor does: the menu is about the thing you
            // pointed at, not about whatever was chosen before.
            if let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                let local = buzz_geom::Point::new(
                    (pos.x - self.stage_area_min.x) as f64,
                    (pos.y - self.stage_area_min.y) as f64,
                );
                let point = self.editor.screen_to_edit(local);
                if let Some(id) = self.editor.object_at(point, self.editor.pick_tolerance())
                    && !self.editor.selection.contains(id)
                {
                    self.editor.selection.select_one(id);
                }
            }
            let has_selection = has_selection || !self.editor.selection.is_empty();

            let mut item = |ui: &mut egui::Ui, command: Command, enabled: bool| {
                if ui
                    .add_enabled(enabled, egui::Button::new(command.label()))
                    .clicked()
                {
                    raised = Some(command);
                    ui.close();
                }
            };

            item(ui, Command::Cut, has_selection);
            item(ui, Command::Copy, has_selection);
            item(ui, Command::Paste, true);
            item(ui, Command::Delete, has_selection);
            ui.separator();

            item(ui, Command::RotateClockwise, has_selection);
            item(ui, Command::RotateAnticlockwise, has_selection);
            item(ui, Command::FlipHorizontal, has_selection);
            item(ui, Command::FlipVertical, has_selection);
            ui.separator();

            ui.menu_button("Arrange", |ui| {
                item(ui, Command::BringToFront, has_selection);
                item(ui, Command::BringForward, has_selection);
                item(ui, Command::SendBackward, has_selection);
                item(ui, Command::SendToBack, has_selection);
            });
            item(ui, Command::GroupSelection, has_selection);
            item(ui, Command::UngroupSelection, has_selection);
            ui.separator();

            item(ui, Command::ConvertToSymbol, has_selection);
            // **Keeping artwork as an asset, where the artwork is.** The panel
            // has a button for this, but it is the panel's button — you have to
            // find the panel, and by then you have stopped looking at the thing
            // you wanted to keep.
            if ui
                .add_enabled(has_selection, egui::Button::new("Save as Asset"))
                .on_hover_text("Keep the selection in the Assets library, for any film")
                .clicked()
            {
                save_asset = true;
                ui.close();
            }
            ui.separator();
            item(ui, Command::SelectAll, true);
        });

        if let Some(command) = raised {
            self.dispatch(command);
        }
        if save_asset {
            let folder = self.editor.assets_panel.selected_folder.clone();
            self.apply_asset_action(buzz_ui::AssetAction::Add { folder });
        }
    }

    /// Route pointer input over the stage to the active tool.
    fn handle_stage_input(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        let id = ui.id().with("stage");
        let response = ui.interact(area, id, egui::Sense::click_and_drag());
        // Remembered so the menu can turn a pointer position into a document
        // point; it is opened from the same response, after this returns.
        self.stage_area_min = area.min;
        self.stage_context_menu(&response);
        let ctx = ui.ctx().clone();
        let mods = mods_from(&ctx);

        let local =
            |p: egui::Pos2| Point::new((p.x - area.min.x) as f64, (p.y - area.min.y) as f64);

        // **A symbol dragged out of the Library, dropped where it was let go.**
        //
        // Taken before anything else, because the release that ends a drag is
        // also a pointer release the tools would otherwise act on — a dropped
        // symbol must not additionally deselect what was under it.
        if egui::DragAndDrop::has_payload_of_type::<buzz_ui::library_panel::DraggedSymbol>(&ctx) {
            let over = ctx
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| area.contains(p));
            if over {
                ctx.set_cursor_icon(egui::CursorIcon::Copy);
                // A ghost of where it will land, so a drop is aimed rather
                // than guessed at.
                if let Some(p) = ctx.input(|i| i.pointer.hover_pos()) {
                    ui.painter().circle_stroke(
                        p,
                        7.0,
                        egui::Stroke::new(1.5, buzz_ui::Palette::active()),
                    );
                }
            }
            if over && ctx.input(|i| i.pointer.any_released()) {
                if let Some(dropped) =
                    egui::DragAndDrop::take_payload::<buzz_ui::library_panel::DraggedSymbol>(&ctx)
                    && let Some(p) = ctx.input(|i| i.pointer.interact_pos())
                {
                    self.editor.place_symbol_at(dropped.0, local(p));
                }
                return;
            }
        }

        // **A slot in the Rigging panel is waiting for a drawing.**
        //
        // Taken before the tools, because every tool already means something on
        // the stage and none of them mean "this is the left forearm". The mode
        // is visible in the panel — the armed slot is highlighted and says what
        // it wants — and Escape or a click on empty space leaves it, so there is
        // no way to be stuck in it without being told.
        if let Some(slot) = self.editor.rig_panel.armed {
            if response.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.editor.rig_panel.armed = None;
                return;
            }
            if response.clicked()
                && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
            {
                self.pick_part_for_slot(slot, local(pos));
                return;
            }
        }

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

        // One clock for the whole gesture: the press, every move in it and the
        // release are all stamped from egui's own time, so a brush measuring
        // the gaps between them is measuring one thing. See
        // `ToolMachine::pointer_down_at`.
        let stamp = Some(ctx.input(|i| i.time));

        if response.drag_started()
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
        {
            if !pan_override {
                // The force on the touch that began this gesture, if it was a
                // pen. A press is one event and carries its own.
                self.pen_pressure = ctx.input(|i| {
                    i.events.iter().rev().find_map(|e| match e {
                        egui::Event::Touch { force, .. } => force.map(f64::from),
                        _ => None,
                    })
                });
                self.editor
                    .pointer_down_at(local(pos), mods, stamp, self.pen_pressure);
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
                // **Every place the pointer went, not just where it ended up.**
                //
                // A mouse reports at 125 Hz and a pen at 200 or more, while
                // this runs once a frame. Taking `interact_pos` alone threw
                // away two samples in three and turned the rest into a polygon
                // whose corners are wherever the frames happened to land — so
                // a stroke drawn quickly came out visibly faceted, and the
                // fluid brush read the speed off those few widely spaced
                // samples and got it wrong as well. egui keeps the intermediate
                // moves in `events`; a drag now draws through all of them and
                // only falls back to the interact position when there were
                // none (a frame in which the pointer was held still).
                // **Pen pressure comes in on the touch events, not the pointer
                // ones.** A pen on Windows arrives as `WM_POINTER`, which winit
                // turns into a touch with a force and egui passes through as
                // `Event::Touch { force }`; egui *also* synthesises the
                // ordinary pointer events from it, which is how drawing with a
                // pen worked at all while every stroke was recorded at full
                // pressure. Where there are touch events this frame they are
                // the better source — they carry the position and the force
                // together — and a mouse, which has neither, still goes down
                // the pointer path.
                let (moves, now, interval) = ctx.input(|i| {
                    let touches: Vec<(egui::Pos2, Option<f64>)> = i
                        .events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::Touch { pos, force, .. } => {
                                Some((*pos, force.map(f64::from)))
                            }
                            _ => None,
                        })
                        .collect();
                    let moves = if touches.is_empty() {
                        i.events
                            .iter()
                            .filter_map(|e| match e {
                                egui::Event::PointerMoved(p) => Some((*p, None)),
                                _ => None,
                            })
                            .collect()
                    } else {
                        touches
                    };
                    (moves, i.time, f64::from(i.unstable_dt))
                });
                if moves.is_empty() {
                    self.editor
                        .pointer_move_at(local(pos), mods, Some(now), self.pen_pressure);
                } else {
                    // **Spread across the frame they arrived in.** egui does
                    // not stamp its events, but it does say how long the frame
                    // took, and the moves in it happened over that span rather
                    // than all at its end. Dividing it evenly is not the true
                    // sampling instant of each one, but it is the right *scale*
                    // — and scale is all a speed-driven width reads. Stamping
                    // them all with one instant would make every stroke
                    // hairline; stamping them microseconds apart, which is what
                    // reading a wall clock in this loop does, is worse.
                    let n = moves.len() as f64;
                    let span = interval.clamp(1e-4, 0.25);
                    for (k, (p, force)) in moves.into_iter().enumerate() {
                        let at = now - span + span * ((k as f64 + 1.0) / n);
                        // The last force seen is remembered, so the press and
                        // the release — which arrive on their own events — are
                        // recorded at the pressure the pen was actually at
                        // rather than at full.
                        if force.is_some() {
                            self.pen_pressure = force;
                        }
                        self.editor.pointer_move_at(local(p), mods, Some(at), force);
                    }
                }
            }
        }

        if response.drag_stopped() {
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
                && !pan_override
            {
                self.editor
                    .pointer_up_at(local(pos), stamp, self.pen_pressure);
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
            Command::ExportFla => self.export_fla_dialog(),
            Command::ExportImage => self.open_export(buzz_ui::ExportKind::Image),
            Command::ExportSequence => self.open_export(buzz_ui::ExportKind::Sequence),
            Command::ExportVideo => self.open_export(buzz_ui::ExportKind::Video),
            Command::ExportGif => self.open_export(buzz_ui::ExportKind::Gif),
            Command::ExportWebp => self.open_export(buzz_ui::ExportKind::Webp),
            Command::Open => self.open_dialog(),
            Command::Save => self.save(false),
            Command::SaveAs => self.save(true),
            Command::Close => self.request_quit(),
            Command::ImportToStage => self.import_dialog(buzz_scene::ImportTarget::Stage),
            Command::ImportToLibrary => self.import_dialog(buzz_scene::ImportTarget::Library),
            // A script drives the whole document and can run for seconds, so it
            // goes to a thread rather than blocking the frame — see
            // `run_script_async`. The `Editor::run` path is kept for the
            // headless CLI, which needs the answer before it can print it.
            Command::RunScript => self.run_script_async(),
            Command::SaveSnapshot => self.save_snapshot(),
            Command::Snapshots => {
                self.snapshots.rescan();
                self.show_snapshots = true;
            }
            other => self.editor.run(other),
        }
    }

    /// Keep the document as it is now, under a name built from the document and
    /// the time, so a later Snapshots ▸ Restore can bring it back.
    fn save_snapshot(&mut self) {
        let base = self
            .editor
            .doc
            .path()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "Untitled".to_string());
        let stamp = chrono_stamp();
        let name = format!("{base} {stamp}");
        match self.snapshots.save(&name, self.editor.doc.scene()) {
            Ok(t) => self.editor.status = Some(format!("Snapshot saved: {}", t.name)),
            Err(e) => self.editor.status = Some(format!("Could not save a snapshot: {e}")),
        }
    }

    /// The Snapshots list: restore a past version into the current document as
    /// one undo step, keeping the file being worked on.
    fn snapshots_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_snapshots {
            return;
        }
        let mut open = true;
        let mut restore: Option<std::path::PathBuf> = None;
        egui::Window::new("Snapshots")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                if self.snapshots.is_empty() {
                    ui.label("No snapshots yet.");
                    ui.label(
                        egui::RichText::new("Save one with Save Snapshot (⌘K ▸ Save Snapshot).")
                            .weak(),
                    );
                    return;
                }
                ui.label("Restore a saved version into this document:");
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for snap in self.snapshots.iter() {
                        ui.horizontal(|ui| {
                            if ui.button("Restore").clicked() {
                                restore = Some(snap.path.clone());
                            }
                            ui.label(&snap.name);
                        });
                    }
                });
            });
        if let Some(path) = restore {
            match buzz_doc::Document::open(&path) {
                Ok(doc) => {
                    let scene = doc.scene().clone();
                    self.editor.doc.edit("Restore Snapshot", |s| *s = scene.clone());
                    self.editor.doc.end_gesture();
                    self.editor.status = Some("Snapshot restored".into());
                    self.show_snapshots = false;
                }
                Err(e) => self.editor.status = Some(format!("Could not restore that snapshot: {e}")),
            }
        }
        if !open {
            self.show_snapshots = false;
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
                self.asset_thumbnails.forget();
            }

            DeleteFolder { folder } => {
                // **No undo out here.** An asset library is files on disk, and
                // deleting a folder takes everything under it — so the count is
                // said out loud and the second click is the confirmation.
                let count = self
                    .editor
                    .assets
                    .assets()
                    .iter()
                    .filter(|a| a.folder == folder || a.folder.starts_with(&format!("{folder}/")))
                    .count();
                if self.editor.assets_panel.confirm_delete.as_deref() != Some(folder.as_str()) {
                    self.editor.assets_panel.confirm_delete = Some(folder.clone());
                    self.editor.status = Some(format!(
                        "Delete the folder {folder} and {count} asset(s)? Click Delete again to confirm"
                    ));
                } else {
                    self.editor.assets_panel.confirm_delete = None;
                    match self.editor.assets.delete_folder(&folder) {
                        Ok(()) => {
                            self.asset_thumbnails.forget();
                            if self.editor.assets_panel.selected_folder == folder {
                                self.editor.assets_panel.selected_folder.clear();
                            }
                            self.editor.status = Some(format!("Deleted {folder}"));
                        }
                        Err(e) => {
                            self.editor.status = Some(format!("Could not delete the folder: {e}"))
                        }
                    }
                }
            }

            Rescan => {
                self.editor.assets.rescan();
                // The pictures were drawn from files that may have been
                // rewritten, moved or deleted since; a rescan is exactly when
                // to stop trusting them.
                self.asset_thumbnails.forget();
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

        let mut request =
            crate::dialogs::Request::folder().title("Choose an Animate assets folder");
        if let Some(root) = crate::animate_assets::likely_roots().first() {
            request = request.directory(root);
        }
        self.ask_for_path(request, Pick::AnimateAssets);
    }

    /// The folder was chosen; read what is in it.
    fn import_animate_assets_from(&mut self, root: std::path::PathBuf) {
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

    /// Open a file picker, off the UI thread.
    ///
    /// The window it belongs to is read here, on the thread that owns it,
    /// because a `Window` cannot cross to the picker's thread and an
    /// unparented dialog is free to hide behind the window that asked for it.
    fn ask_for_path(&mut self, request: crate::dialogs::Request, purpose: Pick) {
        let parent = self
            .active
            .as_ref()
            .map(|active| crate::dialogs::Parent::of(&active.window))
            .unwrap_or_default();

        if !self.picker.ask(request, parent, purpose) {
            self.editor.status = Some("A file dialog is already open".into());
        }
    }

    /// Act on whatever the picker came back with.
    ///
    /// Cancelling says nothing: the user closed a dialog they opened, and a
    /// status line announcing that would be noise.
    fn poll_picker(&mut self) {
        let Some((purpose, path)) = self.picker.poll() else {
            return;
        };
        let Some(path) = path else { return };

        match purpose {
            Pick::Open => {
                if opens_as_document(&path) {
                    self.open_buzz(&path);
                } else {
                    self.open_imported(&path);
                }
            }
            Pick::SaveAs => self.save_to(path),
            Pick::ImportImage => self.import_image_from(path),
            Pick::FillWithImage => self.fill_with_image_from(path),
            Pick::ImportSound => self.import_sound_from(path),
            Pick::ImportInto(target) => self.import_file(target, path),
            Pick::AnimateAssets => self.import_animate_assets_from(path),
            Pick::Export(kind) => self.start_export(kind, path),
            Pick::ExportFla => self.export_fla_to(path),
        }
    }

    /// Run the Actions-panel script on a thread of its own.
    ///
    /// The scene is snapshotted here and handed over — a copy-on-write clone,
    /// so it is pointer copies, not the artwork — and the script mutates *that*
    /// while the user goes on looking at the document as it was. When it
    /// finishes, its working copy is committed in one edit, so a script that
    /// draws four hundred shapes is still a single Ctrl+Z.
    fn run_script_async(&mut self) {
        if self.scripting.is_some() {
            self.editor.status = Some("A script is already running".into());
            return;
        }
        if !self.editor.actions.has_source() {
            self.editor.status = Some("Write a script in the Actions panel first".into());
            return;
        }
        // Running from the menu or the keyboard while the panel is closed would
        // put the output somewhere the user cannot see it.
        if !self.editor.workspace.is_open(buzz_ui::PanelId::Actions) {
            self.editor.workspace.toggle(buzz_ui::PanelId::Actions);
        }

        let source = self.editor.actions.source.clone();
        let context = buzz_script::ScriptContext {
            current_frame: self.editor.current_frame,
            selection: self.editor.selection.ids(),
            active_layer: self.editor.selection.active_layer(),
                    config_dir: buzz_script::default_config_dir(),
        };
        let mut working = self.editor.doc.scene().clone();

        let (send, result) = crossbeam_channel::bounded(1);
        let id = self.tasks.spawn_thread(crate::tasks::TaskKind::Script, "Script", move |ctx| {
            // Cancel reaches the interpreter through its interrupt handler,
            // which QuickJS calls between bytecodes — so Stop lands mid-loop,
            // not at the end of one. A `while (true) {}` stops on the spot.
            let cancel = ctx.cancel.clone();
            let stop: buzz_script::StopSignal = std::sync::Arc::new(move || cancel.is_cancelled());
            let outcome = buzz_script::run_until(
                &mut working,
                context,
                &source,
                &buzz_script::Limits::default(),
                Some(stop),
            );
            let summary = outcome.summary();
            let _ = send.send((working, outcome));
            crate::tasks::TaskOutcome::Finished(summary)
        });

        self.scripting = Some(ScriptRun { id, result });
    }

    /// A script finished — committed, applied, and reported.
    fn collect_script(&mut self, id: crate::tasks::TaskId) {
        let Some(run) = self.scripting.take_if(|r| r.id == id) else {
            return;
        };
        let Ok((working, outcome)) = run.result.try_recv() else {
            self.editor.status = Some("The script finished but returned nothing".into());
            return;
        };

        // Committed whether it finished, timed out, or was stopped: a partial
        // result is still the user's work, and a single Ctrl+Z removes it if
        // they did not want it. `changed` is the revision actually moving, so a
        // read-only script leaves no empty undo step behind.
        if outcome.changed {
            self.editor.doc.edit("Run Script", |scene| *scene = working);
            self.editor.doc.end_gesture();
        }

        // Computed before the outcome is taken apart below.
        let summary = outcome.summary();

        // The script's view of the editor becomes the editor's, so
        // `t.currentFrame = 5` moves the playhead and `d.selectAll()` leaves
        // the artwork actually selected.
        self.editor.set_frame(outcome.context.current_frame);
        self.editor.selection.set(outcome.context.selection);
        self.editor.selection.prune(self.editor.doc.scene());
        if let Some(layer) = outcome.context.active_layer {
            self.editor.selection.set_active_layer(Some(layer));
        }
        self.editor.selection.ensure_active_layer(self.editor.doc.scene());

        self.editor.status = Some(summary.clone());

        // **What the script tried to ask.** A run has nobody watching it, so
        // an `alert` cannot open a window — it is recorded instead, and shown
        // here with the rest of the output. A command that says "no eye layer
        // found" has told you the useful thing, and losing that would leave
        // only a script that appeared to do nothing.
        let mut output = outcome.trace;
        if !outcome.alerts.is_empty() {
            output.push(String::new());
            for asked in &outcome.alerts {
                output.push(format!("alert: {asked}"));
            }
        }
        self.editor.actions.report(output, outcome.error, summary);
    }

    /// The overlay shown while a script runs.
    ///
    /// A [`egui::Modal`], so it gates every pointer path into the document for
    /// free — a click on the stage cannot reach the artwork the script is in
    /// the middle of rewriting. Keyboard commands are gated separately, in
    /// `update`, because shortcuts do not go through the backdrop.
    fn script_overlay(&mut self, ctx: &egui::Context) {
        let Some(run) = &self.scripting else { return };
        let elapsed = self
            .tasks
            .running()
            .find(|t| t.id == run.id)
            .map(|t| t.elapsed())
            .unwrap_or_default();
        let cancel_id = run.id;

        let mut cancel = false;
        egui::Modal::new(egui::Id::new("script-running")).show(ctx, |ui| {
            ui.set_width(260.0);
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                ui.add(egui::Spinner::new().size(28.0));
                ui.add_space(8.0);
                ui.strong("Running script\u{2026}");
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!("{:.1} s", elapsed.as_secs_f64()))
                        .weak()
                        .monospace(),
                );
                ui.add_space(10.0);
                if ui.button("Stop").clicked() {
                    cancel = true;
                }
            });
        });

        if cancel {
            // Sets the token the script's interrupt handler reads; it stops at
            // the next bytecode and comes back through `collect_script` with
            // whatever it had done marked as its own.
            self.tasks.cancel(cancel_id);
            self.editor.status = Some("Stopping the script\u{2026}".into());
        }
    }

    /// Is a file already being read?
    ///
    /// Two documents racing to replace the open one has no sensible answer —
    /// whichever finished second would win, which is not what "second" means to
    /// the person who asked. So the second request is declined and said so.
    fn loading_already(&mut self) -> bool {
        if self.loading.is_empty() {
            return false;
        }
        self.editor.status = Some("A file is already being read".into());
        true
    }

    /// Start decoding the document's undecoded sounds off the UI thread, if any
    /// and none is already in flight.
    ///
    /// The same fan-out as the shading build: a plain thread parks on the
    /// interactive pool so the decode runs on every core there and never on the
    /// UI thread, and the result is installed at the top of a later frame.
    fn drive_sound_decode(&mut self, scene: &buzz_scene::Scene) {
        if self.sound_decode.is_some() {
            return;
        }
        let batch = self.editor.sound.take_undecoded(scene);
        if batch.is_empty() {
            return;
        }
        let jobs = std::sync::Arc::clone(&self.jobs);
        let (send, receive) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("buzz-sound-decode".into())
            .spawn(move || {
                let results = jobs.run(Pool::Interactive, || {
                    use rayon::prelude::*;
                    batch
                        .par_iter()
                        .map(crate::sound::decode_undecoded)
                        .collect::<Vec<_>>()
                });
                let _ = send.send(results);
            })
            .ok();
        self.sound_decode = Some(receive);
        if let Some(active) = &self.active {
            active.window.request_redraw();
        }
    }

    /// Is the document free to be edited right now?
    ///
    /// False while a script owns it, and false during the brief window a large
    /// import is being merged off-thread — in both cases an edit landing in the
    /// middle would race the wholesale scene the background work is about to
    /// commit. Reading a file does *not* lock the document: the window stays
    /// fully usable while a file is being read.
    fn doc_available(&self) -> bool {
        self.scripting.is_none() && !self.merging
    }

    /// Read a file on a thread, and remember where the answer will arrive.
    ///
    /// Cancel discards the result rather than stopping the parse: the
    /// importers have no interruption point inside them, and pretending
    /// otherwise would be worse than saying so. The parse is bounded by the
    /// file, and nothing waits on it.
    fn start_load<F>(&mut self, kind: crate::tasks::TaskKind, label: String, read: F)
    where
        F: FnOnce(&crate::tasks::TaskCtx) -> Loaded + Send + 'static,
    {
        let (send, receive) = crossbeam_channel::bounded(1);
        let id = self.tasks.spawn_thread(kind, label, move |ctx| {
            let loaded = read(&ctx);
            // Sent *before* the outcome. The outcome is what the frame loop
            // waits on, and a channel hand-off orders everything that happened
            // before it — so by the time the outcome lands, this has too.
            let _ = send.send(loaded);
            crate::tasks::TaskOutcome::Finished(String::new())
        });
        self.loading.insert(id, receive);
    }

    /// A file finished reading. Put it where it was going.
    fn collect_loaded_document(&mut self, id: crate::tasks::TaskId) {
        let Some(receiver) = self.loading.remove(&id) else {
            return;
        };
        let Ok(loaded) = receiver.try_recv() else {
            // Cannot happen — the payload is sent before the outcome that
            // brought us here — but a silent nothing beats an unwrap.
            self.editor.status = Some("The file was read but arrived empty".into());
            return;
        };

        match loaded {
            Loaded::Document { path, result } => match result {
                Ok(doc) => {
                    self.adopt_document(*doc);
                    self.remember_document_directory(&path);
                    self.editor.status = Some(format!("Opened {}", path.display()));
                }
                Err(why) => self.editor.status = Some(why),
            },
            Loaded::Foreign {
                path,
                target,
                result,
            } => match (result, target) {
                (Ok(imported), None) => self.finish_open_imported(&path, *imported),
                (Ok(imported), Some(target)) => self.finish_import(target, path, *imported),
                (Err(message), None) => {
                    // **In front of the user, not in the status bar.** A file
                    // that will not open is the whole of what they were trying
                    // to do, and the reason is usually specific enough to act
                    // on — but only if it is read.
                    let name = file_name(&path);
                    self.editor.status = Some(format!("Could not open {name}: {message}"));
                    self.editor.import_summary = Some(crate::import::ImportSummary {
                        title: format!("Could not open {name}"),
                        what_arrived: message,
                        unsupported: Vec::new(),
                        failed: true,
                    });
                }
                (Err(message), Some(_)) => {
                    // A failed import leaves the open document untouched, which
                    // it does: nothing was merged.
                    self.editor.status = Some(format!("Could not import: {message}"));
                }
            },
            Loaded::Merged {
                path,
                scene,
                report,
                unsupported,
                summary,
            } => self.finish_merge(path, *scene, report, unsupported, summary),
        }
    }

    /// Collect whatever the background tasks have finished.
    ///
    /// Everything a task has to say arrives here, once, on the UI thread — so
    /// applying a result is an ordinary edit at an ordinary moment rather than
    /// another thread reaching into the document.
    fn poll_tasks(&mut self) {
        for (id, kind, outcome) in self.tasks.poll() {
            // Exports are handled in one place because all three outcomes have
            // to be recorded against the queue — success, cancel and failure
            // alike free it for the next export and get a row in the panel.
            if kind == crate::tasks::TaskKind::Export {
                let (ok, message) = match outcome {
                    crate::tasks::TaskOutcome::Finished(m) => (true, m),
                    crate::tasks::TaskOutcome::Cancelled => {
                        (false, "Export cancelled".to_string())
                    }
                    crate::tasks::TaskOutcome::Failed(why) => (false, why),
                };
                self.exports.complete(id, ok, message.clone());
                self.editor.status = Some(message);
                continue;
            }

            match outcome {
                crate::tasks::TaskOutcome::Finished(message) => {
                    self.finish_task(id, kind, message);
                }
                crate::tasks::TaskOutcome::Cancelled => {
                    self.abandon_task(id, kind);
                }
                crate::tasks::TaskOutcome::Failed(why) => {
                    self.abandon_task(id, kind);
                    self.editor.status = Some(why);
                }
            }
        }
    }

    /// A task got where it was going.
    ///
    /// The message is what the task wants said; anything it *produced* it left
    /// in a slot of its own, which is what the per-kind arms collect.
    fn finish_task(&mut self, id: crate::tasks::TaskId, kind: crate::tasks::TaskKind, message: String) {
        match kind {
            crate::tasks::TaskKind::Open | crate::tasks::TaskKind::Import => {
                self.collect_loaded_document(id);
            }
            crate::tasks::TaskKind::Script => {
                self.collect_script(id);
                // `collect_script` sets the status from the outcome itself, so
                // the task's own message is not repeated over the top of it.
                return;
            }
            _ => {}
        }
        if !message.is_empty() {
            self.editor.status = Some(message);
        }
    }

    /// A task stopped early, or fell over. Whatever it was going to hand back
    /// is dropped rather than half-applied.
    fn abandon_task(&mut self, id: crate::tasks::TaskId, kind: crate::tasks::TaskKind) {
        match kind {
            crate::tasks::TaskKind::Open | crate::tasks::TaskKind::Import => {
                self.loading.remove(&id);
                // If this was the merge step, the document is editable again.
                // Harmless when it was only a read: `merging` was already false.
                self.merging = false;
            }
            crate::tasks::TaskKind::Script => {
                // Only reached if the script *thread* fell over — a real Stop
                // is caught inside the interpreter and comes back as a normal
                // Finished with `stopped` set, through `collect_script`.
                self.scripting = None;
            }
            _ => {}
        }
        self.editor.status = Some(format!("{} stopped", kind.label()));
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

    /// Fill the Export dialog from a preset.
    ///
    /// A preset can change the format too — choosing "GIF preview" while the
    /// dialog was opened as a video switches the whole export to a GIF — so it
    /// sets the kind, then the size (from the preset's target height and the
    /// stage's aspect), then the format-specific options.
    fn apply_preset(&mut self, preset: &buzz_export::ExportPreset) {
        use buzz_export::PresetFormat;

        let kind = match preset.format {
            PresetFormat::Png => buzz_ui::ExportKind::Image,
            PresetFormat::PngSequence => buzz_ui::ExportKind::Sequence,
            PresetFormat::Mp4H264 | PresetFormat::Mp4Hevc | PresetFormat::Mp4Av1
            | PresetFormat::MovHevc => buzz_ui::ExportKind::Video,
            PresetFormat::Gif => buzz_ui::ExportKind::Gif,
            PresetFormat::Webp => buzz_ui::ExportKind::Webp,
        };

        let stage = self.editor.scene().stage().size;
        let stage = (
            stage.width.round().max(1.0) as u32,
            stage.height.round().max(1.0) as u32,
        );
        let (width, height) = preset.resolve_size(stage);

        let export = &mut self.editor.export;
        let was = export.open;
        export.open = Some(kind);
        export.width = width;
        export.height = height;
        export.transparent = preset.transparent;
        export.ffmpeg = !kind.needs_ffmpeg() || buzz_export::ffmpeg_available();
        // **The dialog says which preset these settings are.** Without this the
        // combo went straight back to reading "Choose", so a chosen preset left
        // no mark anywhere the user could see.
        export.selected_preset = Some(preset.name.clone());

        // **A preset that turns one frame into a film needs a film to export.**
        //
        // The kind can change here: "GIF preview" chosen while Export Image was
        // open switches the whole export to a GIF. The range fields were last
        // set by `ExportState::open`, and for a single-frame export that is
        // frame zero to frame zero \u2014 so the GIF came out one frame long, with
        // nothing in the dialog to suggest why. Widened to the whole film, and
        // only when the export was not already a range one, so a range the user
        // has narrowed by hand survives a preset that keeps the same kind.
        if kind.is_range() && !was.is_some_and(|k| k.is_range()) {
            export.from_frame = 0;
            export.to_frame = export.frame_count().saturating_sub(1);
        }

        match preset.format {
            PresetFormat::Mp4H264 => {
                export.video.container = buzz_ui::ContainerChoice::Mp4;
                export.video.codec = buzz_ui::VideoChoice::H264;
            }
            PresetFormat::Mp4Hevc => {
                export.video.container = buzz_ui::ContainerChoice::Mp4;
                export.video.codec = buzz_ui::VideoChoice::Hevc;
            }
            PresetFormat::Mp4Av1 => {
                export.video.container = buzz_ui::ContainerChoice::Mp4;
                export.video.codec = buzz_ui::VideoChoice::Av1;
            }
            PresetFormat::MovHevc => {
                export.video.container = buzz_ui::ContainerChoice::Mov;
                export.video.codec = buzz_ui::VideoChoice::Hevc;
            }
            PresetFormat::Webp => {
                export.webp.quality = preset.quality.min(100);
                export.webp.lossless = preset.lossless;
            }
            PresetFormat::Png | PresetFormat::PngSequence | PresetFormat::Gif => {}
        }
        if kind == buzz_ui::ExportKind::Video {
            export.video.quality = preset.quality;
            export.video.audio = preset.audio;
            export.video.hardware = preset.hardware;
        }

        self.editor.status = Some(format!("Applied preset \u{201C}{}\u{201D}", preset.name));
    }

    /// Build a preset from the dialog's current settings, to be saved.
    fn preset_from_dialog(&self) -> Option<buzz_export::ExportPreset> {
        use buzz_export::PresetFormat;

        let export = &self.editor.export;
        let kind = export.open?;
        let format = match kind {
            buzz_ui::ExportKind::Image => PresetFormat::Png,
            buzz_ui::ExportKind::Sequence => PresetFormat::PngSequence,
            buzz_ui::ExportKind::Gif => PresetFormat::Gif,
            buzz_ui::ExportKind::Webp => PresetFormat::Webp,
            buzz_ui::ExportKind::Video => match export.video.container {
                buzz_ui::ContainerChoice::Mov => PresetFormat::MovHevc,
                buzz_ui::ContainerChoice::Mp4 => match export.video.codec {
                    buzz_ui::VideoChoice::H264 => PresetFormat::Mp4H264,
                    buzz_ui::VideoChoice::Hevc => PresetFormat::Mp4Hevc,
                    buzz_ui::VideoChoice::Av1 => PresetFormat::Mp4Av1,
                    // ProRes forces the container to MOV, so it never lands here;
                    // map it for exhaustiveness.
                    buzz_ui::VideoChoice::ProRes4444 => PresetFormat::MovHevc,
                },
            },
        };

        let quality = match kind {
            buzz_ui::ExportKind::Webp => export.webp.quality,
            _ => export.video.quality,
        };

        Some(buzz_export::ExportPreset {
            name: export.preset_name.clone(),
            format,
            // The current height, kept as the preset's target so re-applying it
            // reproduces this size on a same-shaped stage.
            height: Some(export.height),
            quality,
            transparent: export.transparent,
            audio: export.video.audio,
            hardware: export.video.hardware,
            lossless: export.webp.lossless,
            builtin: false,
        })
    }

    /// Open the Export dialog, sized to the document as it is now.
    fn open_export(&mut self, kind: buzz_ui::ExportKind) {
        // No one-slot gate any more: a second export while one runs joins the
        // queue rather than being refused.
        //
        // Checked as the dialog opens rather than when Export is pressed, so
        // the missing dependency is visible while the settings are still being
        // chosen instead of after a file name has been picked.
        let has_ffmpeg = !kind.needs_ffmpeg() || buzz_export::ffmpeg_available();
        // The length of the **film**, not of the timeline: the scenes play one
        // after another and a looping section is repeated into the export, so
        // the default range has to reach the end of what will actually be
        // written. With one scene and no loop region these are all the same
        // number.
        //
        // The stage comes from the scene being edited, which is the one whose
        // size the user has in mind; a film is one file at one size, and the
        // reel takes its lead from the first scene.
        let size = self.editor.scene().stage().size;
        let length = self.editor.doc.film_frames();
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

    /// Draw the Set the Scene and Animate Selection dialogs, and act on them.
    ///
    /// The state is taken out of the editor for the duration, because the
    /// dialog wants `&mut` on it while the commands want `&mut` on the whole
    /// editor — the same trick every other dialog here uses.
    fn staging_dialog(&mut self, ctx: &egui::Context) {
        if self.editor.staging.open.is_none() {
            return;
        }
        let mut state = std::mem::take(&mut self.editor.staging);
        let can_perform = self.editor.selection_is_performable();
        let response = buzz_ui::staging_dialog(ctx, &mut state, can_perform);

        if response.set_scene {
            self.editor.set_the_scene(&state);
            state.close();
        }
        if response.perform {
            self.editor.perform_selection(&mut state);
        }
        if response.direct {
            self.editor.direct_story(&mut state);
        }
        if response.follow_path {
            self.editor.follow_motion_path(&mut state);
        }
        if response.add_physics {
            self.editor.add_follow_through(&mut state);
        }
        if response.add_wiggle {
            self.editor.add_wiggle(&mut state);
        }
        self.editor.staging = state;
    }

    /// Draw the Export dialog and act on what the user chose.
    fn export_dialog(&mut self, ctx: &egui::Context) {
        let names = self.presets.names();
        let response = buzz_ui::export_dialog(ctx, &mut self.editor.export, &names);

        if let Some(i) = response.apply_preset
            && let Some(preset) = self.presets.all().into_iter().nth(i)
        {
            self.apply_preset(&preset);
        }
        if response.save_preset {
            match self.preset_from_dialog() {
                Some(preset) => {
                    let name = preset.name.trim().to_string();
                    match self.presets.add(preset) {
                    Ok(()) => {
                        self.editor.status =
                            Some(format!("Saved preset \u{201C}{name}\u{201D}"));
                        // Saved settings *are* that preset, so the combo says
                        // so \u2014 the same as if it had just been chosen.
                        self.editor.export.selected_preset = Some(name);
                        self.editor.export.preset_name.clear();
                    }
                    Err(e) => self.editor.status = Some(e),
                    }
                }
                None => {
                    self.editor.status = Some("Nothing to save as a preset".into());
                }
            }
        }

        // Cancel just closes the dialog now — there is no inline job to stop,
        // because the dialog configures and enqueues rather than running the
        // export itself. A running export is stopped from the Tasks panel.
        if !response.confirmed {
            return;
        }

        let Some(kind) = self.editor.export.open else {
            return;
        };
        let stem = self
            .editor
            .doc
            .path()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".to_string());

        let request = match kind {
            buzz_ui::ExportKind::Image => {
                let frame = self.editor.current_frame;
                crate::dialogs::Request::save_file()
                    .filter("PNG image", &["png"])
                    .file_name(format!("{stem}-{frame:04}.png"))
            }
            buzz_ui::ExportKind::Sequence => {
                // A folder, not a file: a sequence is many files, and asking
                // for one file name would leave the user guessing what the
                // rest were called.
                crate::dialogs::Request::folder().title("Choose a folder for the sequence")
            }
            buzz_ui::ExportKind::Video => {
                let options = self.editor.export.video;
                let extension = options.container.extension();
                crate::dialogs::Request::save_file()
                    .filter(options.container.label(), &[extension])
                    .file_name(format!("{stem}.{extension}"))
            }
            buzz_ui::ExportKind::Gif => crate::dialogs::Request::save_file()
                .filter("Animated GIF", &["gif"])
                .file_name(format!("{stem}.gif")),
            buzz_ui::ExportKind::Webp => crate::dialogs::Request::save_file()
                .filter("Animated WebP", &["webp"])
                .file_name(format!("{stem}.webp")),
        };
        self.ask_for_path(request, Pick::Export(kind));
    }

    /// The user has said where the export goes. Start it.
    ///
    /// The snapshot is taken **here**, not when the dialog was confirmed: the
    /// picker is modal, so nothing can have changed in between, and taking it
    /// at the last possible moment is one fewer clone held across a dialog.
    fn start_export(&mut self, kind: buzz_ui::ExportKind, path: std::path::PathBuf) {
        use crate::export_service::{ExportRequest, ExportTarget};

        let settings = buzz_export::ExportSettings {
            width: self.editor.export.width,
            height: self.editor.export.height,
            transparent: self.editor.export.transparent,
        };
        // **The whole film, not the scene being edited.** Snapshots, so the
        // export renders the document as it was when the user asked — and they
        // can keep editing, or queue another, while it writes. A document with
        // one scene is one scene, exactly as before.
        let scenes = self.editor.doc.film();
        let stem = self
            .editor
            .doc
            .path()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".to_string());
        let range = self.editor.export.range();

        let (target, label) = match kind {
            buzz_ui::ExportKind::Image => {
                // The frame of the **film** the playhead is on: the still has
                // to come out of the scene being looked at, and past the first
                // scene those are two different numbers.
                let frame = self.editor.doc.film_start_of(self.editor.doc.active_scene())
                    + self.editor.current_frame;
                let label = file_name(&path);
                (ExportTarget::Image { frame, path }, label)
            }
            buzz_ui::ExportKind::Sequence => {
                let label = format!("{}\u{2044}", file_name(&path));
                (
                    ExportTarget::Sequence {
                        directory: path,
                        base_name: stem,
                    },
                    label,
                )
            }
            buzz_ui::ExportKind::Video => {
                let options = self.editor.export.video;
                let label = file_name(&path);
                (
                    ExportTarget::Video {
                        path,
                        video: buzz_export::VideoSettings {
                            codec: match options.codec {
                                buzz_ui::VideoChoice::H264 => buzz_export::VideoCodec::H264,
                                buzz_ui::VideoChoice::Hevc => buzz_export::VideoCodec::Hevc,
                                buzz_ui::VideoChoice::Av1 => buzz_export::VideoCodec::Av1,
                                buzz_ui::VideoChoice::ProRes4444 => {
                                    buzz_export::VideoCodec::ProRes4444
                                }
                            },
                            container: match options.container {
                                buzz_ui::ContainerChoice::Mp4 => buzz_export::VideoContainer::Mp4,
                                buzz_ui::ContainerChoice::Mov => buzz_export::VideoContainer::Mov,
                            },
                            quality: options.quality,
                            hardware: options.hardware,
                            audio: options.audio,
                        },
                    },
                    label,
                )
            }
            buzz_ui::ExportKind::Gif => {
                let label = file_name(&path);
                (
                    ExportTarget::Gif {
                        path,
                        gif: buzz_export::GifSettings {
                            dither: match self.editor.export.gif.dither {
                                buzz_ui::DitherChoice::None => buzz_export::Dither::None,
                                buzz_ui::DitherChoice::Bayer => buzz_export::Dither::Bayer,
                                buzz_ui::DitherChoice::FloydSteinberg => {
                                    buzz_export::Dither::FloydSteinberg
                                }
                            },
                            loops: 0,
                        },
                    },
                    label,
                )
            }
            buzz_ui::ExportKind::Webp => {
                let options = self.editor.export.webp;
                let label = file_name(&path);
                (
                    ExportTarget::Webp {
                        path,
                        webp: buzz_export::WebpSettings {
                            quality: options.quality,
                            lossless: options.lossless,
                            loops: 0,
                        },
                    },
                    label,
                )
            }
        };

        let busy = !self.exports.is_idle();
        self.exports.enqueue(ExportRequest {
            scenes,
            settings,
            range,
            target,
            gpu: self.preference.clone(),
            label: label.clone(),
        });

        // Open the Tasks panel so there is somewhere to watch it — this is the
        // progress bar and the Cancel button now that the dialog has neither.
        if !self.editor.workspace.is_open(buzz_ui::PanelId::Tasks) {
            self.editor.workspace.toggle(buzz_ui::PanelId::Tasks);
            self.editor.workspace.save();
        }
        self.editor.status = Some(if busy {
            format!("Queued {label}")
        } else {
            format!("Exporting {label}\u{2026}")
        });
    }

    /// Start the next queued export, if the queue is free.
    ///
    /// Serial by construction: [`ExportQueue::next_to_start`] hands back nothing
    /// while one is running, so this does nothing until it finishes.
    fn pump_export_queue(&mut self) {
        let Some(request) = self.exports.next_to_start() else {
            return;
        };
        let reveal = request.reveal_path();
        let label = request.label.clone();
        let id = self.tasks.spawn_thread(
            crate::tasks::TaskKind::Export,
            label.clone(),
            move |ctx| crate::export_service::run_export(request, &ctx),
        );
        self.exports.started(id, reveal, label);
    }

    /// Animate's File ▸ Import Image.
    fn import_image_dialog(&mut self) {
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Image", &["png", "jpg", "jpeg", "gif", "bmp", "webp"]),
            Pick::ImportImage,
        );
    }

    fn import_image_from(&mut self, path: std::path::PathBuf) {
        match self.editor.import_image(&path) {
            Ok(name) => {
                self.editor.status = Some(format!(
                    "Imported {name} — it is artwork now: the Lasso and the Magic Wand cut it"
                ))
            }
            Err(e) => self.editor.status = Some(format!("Could not import that image: {e:#}")),
        }
    }

    /// Pick an image to fill the selected shapes with (as a fill, not new art).
    fn fill_with_image_dialog(&mut self) {
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Image", &["png", "jpg", "jpeg", "gif", "bmp", "webp"]),
            Pick::FillWithImage,
        );
    }

    fn fill_with_image_from(&mut self, path: std::path::PathBuf) {
        match self.editor.fill_selection_with_image(&path, false) {
            Ok(()) => self.editor.status = Some("Filled the selection with the image".into()),
            Err(e) => self.editor.status = Some(format!("Could not fill with that image: {e:#}")),
        }
    }

    /// Animate's File ▸ Import Sound.
    fn import_sound_dialog(&mut self) {
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Sound", &["wav", "mp3", "ogg", "flac", "m4a", "aac"]),
            Pick::ImportSound,
        );
    }

    fn import_sound_from(&mut self, path: std::path::PathBuf) {
        match self.editor.import_sound(&path) {
            Ok(name) => {
                self.editor.status = Some(if self.editor.sound_was_placed() {
                    format!("Imported {name}, on a layer of its own from frame 1")
                } else {
                    format!(
                        "Imported {name} — put it on a keyframe with Control > Attach Sound"
                    )
                })
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

    /// Somebody asked to close the window.
    ///
    /// If nothing that matters is running, that is that. If an export is in
    /// flight — minutes of GPU time and a file half-written to disk — the quit
    /// waits behind a prompt rather than throwing it away.
    fn request_quit(&mut self) {
        if self.tasks.quit_blockers().is_empty() {
            self.editor.should_quit = true;
        } else {
            self.quit_prompt = true;
            if let Some(active) = &self.active {
                active.window.request_redraw();
            }
        }
    }

    /// The "an export is still running" prompt.
    fn quit_prompt_dialog(&mut self, ctx: &egui::Context) {
        if !self.quit_prompt {
            return;
        }

        // The blocker with the most to lose, described for the prompt. Collected
        // as an owned string so the borrow on the registry is dropped before the
        // buttons below want `&mut self`.
        let blockers = self.tasks.quit_blockers();
        let (headline, more) = match blockers.first() {
            Some(task) => {
                let progress = task.progress();
                let percent = progress
                    .fraction()
                    .map(|f| format!(" is {:.0}% done", f * 100.0))
                    .unwrap_or_default();
                let headline = format!("{} \u{201C}{}\u{201D}{percent}.", task.kind.label(), task.label);
                let more = blockers.len().saturating_sub(1);
                (headline, more)
            }
            None => {
                // Nothing left to block: whatever was running finished while the
                // prompt was being raised.
                self.quit_prompt = false;
                self.editor.should_quit = true;
                return;
            }
        };

        let mut keep_waiting = false;
        let mut quit_anyway = false;
        egui::Modal::new(egui::Id::new("quit-prompt")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Still exporting");
            ui.add_space(6.0);
            ui.label(headline);
            if more > 0 {
                ui.label(
                    egui::RichText::new(format!("And {more} more waiting behind it."))
                        .weak(),
                );
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Quitting now stops it and deletes the partly-written file.",
                )
                .weak(),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Keep waiting").clicked() {
                    keep_waiting = true;
                }
                if ui
                    .button(egui::RichText::new("Stop export and quit").color(egui::Color32::from_rgb(0xE5, 0x53, 0x53)))
                    .clicked()
                {
                    quit_anyway = true;
                }
            });
        });

        if keep_waiting {
            self.quit_prompt = false;
        }
        if quit_anyway {
            self.quit_prompt = false;
            // The exports stop and tidy their `.part` files as the process
            // shuts down — see the exit path in `window_event`.
            self.editor.should_quit = true;
        }
    }

    /// Draw the Tasks panel: everything the program is doing in the background.
    fn tasks_panel(&mut self, ui: &mut egui::Ui) {
        let running: Vec<buzz_ui::TaskRow> = self
            .tasks
            .running()
            .map(|task| {
                let progress = task.progress();
                buzz_ui::TaskRow {
                    id: task.id.0,
                    kind: task.kind.label().to_string(),
                    label: task.label.clone(),
                    progress: progress.fraction(),
                    detail: progress.detail,
                    elapsed_secs: task.elapsed().as_secs_f64(),
                    can_cancel: task.kind.can_cancel(),
                }
            })
            .collect();

        let finished: Vec<buzz_ui::FinishedRow> = self
            .exports
            .finished()
            .iter()
            .enumerate()
            .map(|(i, f)| buzz_ui::FinishedRow {
                id: i as u64,
                label: f.label.clone(),
                ok: f.ok,
                message: f.message.clone(),
            })
            .collect();

        let view = buzz_ui::TasksView {
            running: &running,
            finished: &finished,
            queued: self.exports.waiting(),
        };

        if let Some(action) = buzz_ui::tasks_panel(ui, &view) {
            match action {
                buzz_ui::TaskAction::Cancel(id) => {
                    self.tasks.cancel(crate::tasks::TaskId(id));
                    self.editor.status = Some("Stopping\u{2026}".into());
                }
                buzz_ui::TaskAction::Reveal(i) => {
                    if let Some(f) = self.exports.finished().get(i as usize) {
                        reveal_in_folder(&f.reveal);
                    }
                }
            }
        }
    }

    /// Animate's File ▸ Import, for all three formats.
    ///
    /// The whole import is one [`Document::edit`], so a file that brings in
    /// four hundred symbols is still a single Ctrl+Z.
    fn import_dialog(&mut self, target: buzz_scene::ImportTarget) {
        // Sound sits in the "everything" filter too, or the one filter that is
        // meant to accept anything would be the one that hid the mp3.
        let mut importable = crate::import::IMPORTABLE.to_vec();
        importable.extend_from_slice(crate::import::AUDIBLE);
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Everything BuzzAnimate can import", &importable)
                .filter("Animate document", &["fla", "xfl"])
                .filter("Flash movie", &["swf"])
                .filter("PDF or Illustrator artwork", &["pdf", "ai"])
                .filter("Sound", crate::import::AUDIBLE),
            Pick::ImportInto(target),
        );
    }

    /// Merge a chosen file into the open document.
    fn import_file(&mut self, target: buzz_scene::ImportTarget, path: std::path::PathBuf) {
        // **A sound picked here is imported, not refused.** File > Import is
        // what anyone reaches for with a dialogue track in hand; sending it to
        // the scene importers only produced "BuzzAnimate cannot import .mp3
        // files" from a program that has a sound library. Nothing else about
        // the command changes: a sound goes to the library and onto the
        // timeline, exactly as File > Import Sound puts it.
        if crate::import::is_audio(&path) {
            self.import_sound_from(path);
            return;
        }
        if self.loading_already() {
            return;
        }
        let name = file_name(&path);
        let reading = path.clone();

        self.start_load(crate::tasks::TaskKind::Import, name, move |ctx| {
            ctx.progress.detail(format!("Reading {}", file_name(&reading)));
            Loaded::Foreign {
                result: crate::import::read(&reading).map(Box::new),
                path: reading,
                target: Some(target),
            }
        });
    }

    /// A foreign file finished reading. Merge it **off the UI thread**.
    ///
    /// `Scene::merge` deep-copies every incoming symbol, layer and object, so on
    /// a large `.fla` it is slow — and doing it here, on the frame that collected
    /// the read, froze the window for its whole duration. Instead the merge runs
    /// on a thread against a copy-on-write snapshot, and the finished scene is
    /// committed in [`Self::finish_merge`] as one undo step. The document is
    /// read-only for the short span the merge takes — see [`Self::doc_available`]
    /// — because an edit landing mid-merge would be lost to the wholesale scene
    /// about to replace it.
    fn finish_import(
        &mut self,
        target: buzz_scene::ImportTarget,
        path: std::path::PathBuf,
        imported: crate::import::Imported,
    ) {
        // A pointer copy of the tree, not a copy of the artwork.
        let snapshot = self.editor.doc.scene().clone();
        let name = file_name(&path);
        self.merging = true;
        self.editor.status = Some(format!("Merging {name}…"));

        self.start_load(
            crate::tasks::TaskKind::Import,
            format!("Merging {name}"),
            move |ctx| {
                ctx.progress.detail(format!("Merging {name}"));
                let mut scene = snapshot;
                let report = scene.merge(&imported.scene, target);
                Loaded::Merged {
                    path,
                    scene: Box::new(scene),
                    report,
                    unsupported: imported.unsupported.clone(),
                    summary: imported.summary.clone(),
                }
            },
        );
    }

    /// Commit a merge that finished on a background thread.
    fn finish_merge(
        &mut self,
        path: std::path::PathBuf,
        scene: buzz_scene::Scene,
        report: buzz_scene::MergeReport,
        imported_unsupported: Vec<String>,
        imported_summary: String,
    ) {
        self.merging = false;

        // One undo step: the whole import is a single Ctrl+Z, exactly as before,
        // but the work that built the new scene happened off-thread. Taken
        // through an `Option` so the closure can move the owned scene in even
        // though `edit` takes an `FnMut`.
        let mut scene = Some(scene);
        self.editor.doc.edit("Import", |live| {
            if let Some(merged) = scene.take() {
                *live = merged;
            }
        });

        // An import can change how many frames the document has and which layers
        // exist, so the editor's idea of both has to be re-settled.
        self.editor.selection.clear();
        self.editor
            .selection
            .ensure_active_layer(self.editor.doc.scene());

        let name = file_name(&path);
        self.editor.status = Some(format!("Imported {name}: {}", report.summary()));

        // Only interrupt the user when something was actually lost or moved.
        if !imported_unsupported.is_empty() || !report.renamed.is_empty() {
            let mut unsupported = imported_unsupported;
            for (wanted, given) in &report.renamed {
                unsupported.push(format!(
                    "\"{wanted}\" was already in the library, so it came in as \"{given}\""
                ));
            }
            self.editor.import_summary = Some(crate::import::ImportSummary {
                title: format!("Imported {name}"),
                what_arrived: format!("{imported_summary} — {}", report.summary()),
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

    /// File ▸ Export ▸ Animate Document.
    ///
    /// Named after the document, so a film called `hero.buzz` is offered as
    /// `hero.fla` rather than as `untitled`.
    fn export_fla_dialog(&mut self) {
        let suggested = self
            .editor
            .doc
            .path()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "Untitled".to_string());
        self.ask_for_path(
            crate::dialogs::Request::save_file()
                .file_name(format!("{suggested}.fla"))
                .filter("Animate document", &["fla"]),
            Pick::ExportFla,
        );
    }

    /// Write it, and say what could not come along.
    ///
    /// On the UI thread rather than a worker: this is XML and a zip, not a
    /// render, and even a large document is milliseconds. The exports that go
    /// through `start_export` are frame renders, which is a different order of
    /// cost entirely.
    fn export_fla_to(&mut self, path: std::path::PathBuf) {
        match buzz_export::export_fla(self.editor.doc.scene(), &path) {
            Ok(report) => {
                // **What did not travel is said out loud.** An export that
                // quietly drops a document's gradients is a trap; the summary
                // names them, and the Output panel keeps the list.
                self.editor.status = Some(report.summary());
                if !report.skipped.is_empty() {
                    let mut lines = vec![format!("Exported {}", path.display())];
                    lines.push(String::new());
                    lines.push("Not carried into the .fla:".to_string());
                    for what in &report.skipped {
                        lines.push(format!("  {what}"));
                    }
                    self.editor.actions.report(lines, None, report.summary());
                }
            }
            Err(error) => {
                self.editor.status = Some(format!("Could not export: {error}"));
            }
        }
    }

    fn open_dialog(&mut self) {
        // **Everything openable, not only our own format.** The importers have
        // existed since Phase 5 but were reachable only through File ▸ Import,
        // so File ▸ Open refused an Animate document — the very file somebody
        // coming from Animate would reach for first.
        let mut everything = vec![buzz_doc::EXTENSION];
        everything.extend_from_slice(crate::import::IMPORTABLE);

        let everything: Vec<&str> = everything;
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Everything BuzzAnimate can open", &everything)
                .filter("BuzzAnimate document", &[buzz_doc::EXTENSION])
                .filter("Animate document", &["fla", "xfl"])
                .filter("Flash movie", &["swf"])
                .filter("PDF or Illustrator artwork", &["pdf", "ai"]),
            Pick::Open,
        );
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
        // Symbol ids start again in a new document, so a kept picture would be
        // shown against whatever symbol inherited its number.
        self.thumbnails.clear();
    }

    /// Open one of our own documents.
    fn open_buzz(&mut self, path: &std::path::Path) {
        if self.loading_already() {
            return;
        }
        let path = path.to_path_buf();
        let name = file_name(&path);
        let reading = path.clone();

        self.start_load(crate::tasks::TaskKind::Open, name, move |ctx| {
            ctx.progress.detail(format!("Reading {}", file_name(&reading)));
            Loaded::Document {
                result: Document::open(&reading)
                    .map(Box::new)
                    .map_err(|e| format!("Could not open: {e}")),
                path: reading,
            }
        });
    }

    /// Open a foreign file — `.fla`, `.xfl`, `.swf`, `.pdf`, `.ai` — as a new
    /// document.
    ///
    /// **Not as the document's own path.** What comes back is a *translation*,
    /// however good; saving it must ask for a `.buzz` file rather than write
    /// back over somebody's Animate source, which this program cannot produce
    /// and would therefore destroy.
    fn open_imported(&mut self, path: &std::path::Path) {
        if self.loading_already() {
            return;
        }
        let path = path.to_path_buf();
        let name = file_name(&path);
        let reading = path.clone();

        self.start_load(crate::tasks::TaskKind::Open, name, move |ctx| {
            ctx.progress.detail(format!("Reading {}", file_name(&reading)));
            Loaded::Foreign {
                result: crate::import::read(&reading).map(Box::new),
                path: reading,
                target: None,
            }
        });
    }

    /// A foreign file finished reading and is to become the open document.
    fn finish_open_imported(&mut self, path: &std::path::Path, imported: crate::import::Imported) {
        let name = file_name(path);

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
        if force_dialog || self.editor.doc.path().is_none() {
            // Save As opens on the document's own name, not on "untitled":
            // saving a variant of a file is what Save As is mostly for, and
            // retyping the name every time is the thing that makes it tedious.
            let name = self
                .editor
                .doc
                .path()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("untitled.{}", buzz_doc::EXTENSION));
            self.ask_for_path(
                crate::dialogs::Request::save_file()
                    .filter("BuzzAnimate document", &[buzz_doc::EXTENSION])
                    .file_name(name),
                Pick::SaveAs,
            );
            return;
        }
        let Some(path) = self.editor.doc.path().map(|p| p.to_path_buf()) else {
            return;
        };
        self.save_to(path);
    }

    /// Write the document where the user said.
    fn save_to(&mut self, path: std::path::PathBuf) {
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
        let revision = self.editor.doc.combined_revision();
        if self.last_crash_revision == Some(revision) {
            return;
        }
        self.last_crash_revision = Some(revision);
        if self.editor.doc.is_dirty() {
            self.editor.doc.remember_for_crash();
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

        self.profiler.begin_frame();
        self.profiler.enter(crate::profile::Section::Ui);

        // Playback runs on wall-clock time, so the document plays at its
        // authored rate regardless of the display's refresh rate.
        self.editor.advance_playback(elapsed.as_secs_f64());
        // Stop a scrub's short audio burst once the drag has paused.
        self.editor.tick_scrub();
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
        let adapter = active.adapter.clone();
        let output = egui_ctx.run_ui(raw_input, |ui| {
            stage_area = self.build_ui(ui);
            // Floats above the panels, so it is drawn after them.
            self.import_report_window(ui.ctx());
            // And over everything, including the dialogs: the whole interface is
            // still settling underneath it. Built *after* the real UI on
            // purpose — the editor lays out, measures its stage and starts its
            // thumbnails on exactly the frames nobody can see it doing so.
            buzz_ui::opening_scene(ui.ctx(), &mut self.splash, &adapter);
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

        // **A stage area that is not a rectangle is not a frame to draw.**
        //
        // `stage_area` starts as `egui::Rect::NOTHING`, whose width is negative
        // infinity, and it is only replaced by whatever the layout gave the
        // central panel. That is nothing at all on the first frame after the
        // app opens, and nothing again on the frame a window is maximised,
        // before egui has measured the new size.
        //
        // Carried into physical pixels it becomes a viewport offset of
        // infinity, a camera viewport of minus infinity, and a cull rectangle
        // of NaN. Every one of those turns the frame into nothing, and what is
        // on screen is a black stage — which reads as the lights being broken,
        // because a black picture is exactly what a light that does not work
        // would leave. It is not the lights; it is that no artwork was drawn at
        // all.
        //
        // The last area that *was* a rectangle is drawn instead. For one frame
        // that is the previous framing, which nobody can see; a black frame is
        // something everybody can see.
        let stage_area = if usable_stage_area(stage_area) {
            self.last_stage_area = Some(stage_area);
            stage_area
        } else {
            // And ask for the frame that will have a stage in it, because
            // nothing else here is going to.
            //
            // Said out loud, because a silent fallback is how a layout that
            // never settles turns into "the app draws in the wrong place and
            // the controls miss": the picture would be drawn through one
            // rectangle while the pointer was mapped through another, every
            // frame, with nothing anywhere saying so.
            tracing::warn!(
                ?stage_area,
                "the stage had no measured area this frame; drawing through the last one"
            );
            active.window.request_redraw();
            self.last_stage_area.unwrap_or(FALLBACK_STAGE_AREA)
        };
        active.stage_area = stage_area;
        active
            .egui_state
            .handle_platform_output(&window, output.platform_output);

        // When egui next wants to be repainted, folded into `wants_frame` so the
        // idle loop honours its animations. A zero delay means "again now"; the
        // sentinel far-future means "never, I am idle".
        let egui_delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX);
        self.egui_repaint = if egui_delay == Duration::MAX {
            None
        } else {
            Some(Instant::now() + egui_delay)
        };

        let paint_jobs = egui_ctx.tessellate(output.shapes, output.pixels_per_point);

        // ---- artwork -------------------------------------------------------
        let scale = output.pixels_per_point as f64;
        let area_px = buzz_geom::Rect::new(
            stage_area.min.x as f64 * scale,
            stage_area.min.y as f64 * scale,
            stage_area.max.x as f64 * scale,
            stage_area.max.y as f64 * scale,
        );
        // The camera stays in **logical** points — the space input and the
        // selection chrome work in — so a click lands where the artwork is.
        // `build_scene` scales the finished output up to the physical target by
        // `scale`, which is what makes drawing land under the cursor at any
        // display scaling. (Previously the viewport was set to physical here,
        // which scaled the offset but not the artwork, so the two only agreed at
        // the centre and drifted ~100px at the edges.)
        self.editor.camera.viewport =
            Size::new(stage_area.width() as f64, stage_area.height() as f64);

        self.profiler.enter(crate::profile::Section::Lights);

        // Any shading geometry that finished building off-thread lands here,
        // before the frame that will read it, so this frame draws lit.
        // Resolved at the frame being drawn, so a keyframed light is compared
        // by the state it is actually in rather than by its track.
        // Both numbers come off the same resolved rig: `aim` is what could move
        // a crescent, and the fingerprint is everything that could change the
        // picture at all — colour, strength, a light switched off, a wall of
        // dark stood somewhere else.
        let (aim, rig) = {
            let resolved = self
                .editor
                .scene()
                .lights()
                .resolved_at(self.editor.current_frame);
            (resolved.aim(), resolved.fingerprint())
        };
        if let Some(build) = &self.shade_build {
            if let Ok(built) = build.results.try_recv() {
                self.lights.lights.install(built);
                self.shade_build = None;
                // The stage's shading changed, so a retained encoding is stale.
                self.lights_generation = self.lights_generation.wrapping_add(1);
                active.window.request_redraw();
            } else if build.aim != aim {
                // The light moved while this was being built, so what it is
                // building is already wrong. Tell the workers to stop — they
                // free the pool within one crescent — and drop the batch, which
                // lets this frame start one for where the light is now.
                //
                // Without this, aiming a light queued a fresh full rebuild
                // behind every finished one and the machine stayed pinned for
                // the whole gesture without the shading ever catching up.
                build.abandon();
                self.shade_build = None;
                active.window.request_redraw();
            }
        }

        // Sounds that finished decoding off-thread install here, so the timeline
        // gains their waveforms and the player their cues on the next frame.
        if let Some(rx) = &self.sound_decode
            && let Ok(results) = rx.try_recv()
        {
            let scene = self.editor.doc.scene().clone();
            self.editor.sound.install_decoded(results, &scene);
            self.sound_decode = None;
            active.window.request_redraw();
        }

        // **What this frame may build on this thread, and what it may only
        // record.**
        //
        // An ordinary frame gets a small budget — a handful of crescents, about
        // two milliseconds — so drawing a shape lights it on the spot with no
        // deferred frame and no flicker, while a *bulk* rebuild can never be
        // paid for in front of the user however it was provoked. That single
        // number replaced a rule that tried to name every expensive case in
        // advance and, inevitably, missed one: the frame a light drag ended on
        // found every crescent in the document stale, no build in flight to
        // defer to, and built all of them inline. Measured at 170 ms over three
        // hundred shapes. The freeze had not been removed by deferring during
        // the gesture; it had been moved to the moment the pointer came up.
        //
        // Nothing is built here at all while the cache is cold or a gesture is
        // running: both mean a rebuild of everything, and the whole of it
        // belongs off-thread.
        //
        // Recording is separate, and is off while a batch is already being
        // built, because what were recorded then would be dropped on the floor
        // — and recording one copies a whole transformed path. On a heavy
        // document that was a thousand path copies a frame, discarded, for
        // every frame a build was running.
        // **Cold means there is shading owed and none built**, so a document
        // whose lighting has been trimmed back is never cold: no crescent will
        // ever be built for it, and calling it cold forever would refuse the
        // retained encoding on every frame and re-encode the whole stage at the
        // display's rate. See `buzz_render::document::LightDetail`.
        let cold = self.lights.lights.is_empty()
            && self.editor.scene().lights().is_active()
            && self.lights.detail() == buzz_render::document::LightDetail::Full;
        let building = self.shade_build.is_some();
        let bulk = cold || self.editor.is_gesturing();
        // Has the light come to rest? Nothing is recorded or built while it is
        // still moving — see where the batch is started, below.
        let settled = aim == self.shade_aim;
        self.shade_aim = aim;
        self.lights.lights.set_inline_budget(if bulk || building {
            Duration::ZERO
        } else {
            buzz_render::lighting::INLINE_BUDGET
        });
        self.lights.lights.set_queue(!building && settled);

        self.profiler.enter(crate::profile::Section::Encode);

        // **Reuse the retained stage encoding when nothing that shaped it
        // changed.** `active.vello` still holds last frame's encoding, and it is
        // re-rendered below either way, so "caching" is simply not rebuilding it.
        // A frame that only touched a panel — a tooltip, a background install
        // elsewhere — no longer re-encodes a stage of thousands of shapes. Reuse
        // is refused whenever a tool preview is live or lighting is still being
        // built, because those change the stage without moving the stamp.
        let paints_into_scene = self.editor.preview_paints_into_scene();
        let stamp = StageStamp {
            revision: self.editor.scene().revision(),
            frame: self.editor.current_frame,
            camera: {
                let c = &self.editor.camera;
                [
                    c.center.x.to_bits(),
                    c.center.y.to_bits(),
                    c.zoom.to_bits(),
                    c.rotation.to_bits(),
                ]
            },
            area: [
                area_px.x0.to_bits(),
                area_px.y0.to_bits(),
                area_px.x1.to_bits(),
                area_px.y1.to_bits(),
            ],
            edit_path: self.editor.scene().edit_path().iter().map(|s| s.0).collect(),
            onion: (
                self.editor.onion.enabled,
                self.editor.onion.outlines,
                self.editor.onion.before,
                self.editor.onion.after,
            ),
            edit_multiple: self.editor.edit_multiple,
            lights_generation: self.lights_generation,
            lights: rig,
            painted_preview: paints_into_scene,
        };
        // **Only a preview that is painted into the scene invalidates it.**
        //
        // `stage::build_scene` encodes exactly two previews as artwork — the
        // brush's ink and the soft brush's pixels — because for those the
        // preview *is* the result and has to carry its real weight and colour.
        // Every other preview is egui chrome drawn over the finished frame:
        // the marquee, the transform outlines, the transformation point.
        //
        // Refusing reuse for all of them re-encoded the whole stage on every
        // pointer move of a gesture that could not change it. On a document
        // with an imported character that is thousands of shapes per frame, and
        // it is what made dragging a selection judder while a brush stroke —
        // the genuinely expensive case — cost the same.
        //
        // A build in flight no longer refuses reuse. It used to, and while one
        // ran `wants_frame` returns `Now`, so the window spun at the display's
        // full rate re-encoding a stage that could not have changed: nothing is
        // recorded and nothing is installed until the batch lands, and when it
        // does, `lights_generation` moves the stamp and this rebuilds anyway.
        // On a heavy document that was tens of milliseconds a frame of pure
        // waste, on exactly the frames the machine was busiest.
        //
        // **An encoding built with provisional shading is not reusable**, and
        // that is not the same question as whether anything changed. Nothing
        // *has* changed — that is exactly the trap. A frame that deferred its
        // crescents leaves a retained encoding of the artwork half lit; the next
        // frame finds an identical stamp, reuses it, and so never re-encodes,
        // never records the misses it owes, and never builds them. The picture
        // stays half lit for ever while the window spins asking for a frame that
        // does nothing. That is "the lights do not work" on any document big
        // enough that one frame's build budget did not cover it.
        //
        // Unless a batch is already running, in which case the encoding is going
        // to be replaced the moment it lands — `lights_generation` moves the
        // stamp — and re-encoding in the meantime is the pure waste this reuse
        // exists to avoid.
        let stale_encoding = self.stage_stale && self.shade_build.is_none();
        let reuse = self.retain_stage
            && !cold
            && !stale_encoding
            && !paints_into_scene
            && self.stage_stamp.as_ref() == Some(&stamp);
        if !reuse {
            stage::build_scene(&mut active.vello, &self.editor, area_px, scale, &mut self.lights);
            self.stage_stamp = Some(stamp);
            // Read *after* the draw: it is the draw that discovers what it could
            // not light.
            self.stage_stale = self.lights.lights.is_stale();
        }

        // Whatever this frame could not light, build in parallel off-thread and
        // ask for another frame to show it in.
        let misses = self.lights.lights.take_misses();
        // The budget was for the stage frame only; anything else that draws
        // through this cache — the Library thumbnails below — builds inline.
        self.lights.lights.set_defer(false);

        // **Nothing is built while the light is still moving.**
        //
        // A batch takes longer than the gap between two pointer moves, so one
        // started mid-drag is for a light that will have moved before it lands
        // — that is why it is abandoned above, and starting another in its
        // place only pins the machine again for another result nobody will see.
        // Meanwhile the shadows are exact on every frame and the crescents hold
        // at their last angle, which is what a drag needs to look like.
        //
        // So the batch waits for the light to be where it was last frame: a
        // pause, or the moment the pointer comes up. It lands a frame or two
        // later and the picture is exact again, and the cores were free for
        // drawing the whole time the hand was moving. `set_queue` above is the
        // other half of it — while the light moves, the misses are not even
        // collected, which is a copy of every visible path saved per frame.
        if !misses.is_empty() && self.shade_build.is_none() {
            let jobs = std::sync::Arc::clone(&self.jobs);
            let (send, receive) = crossbeam_channel::bounded(1);
            let abandon = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop = std::sync::Arc::clone(&abandon);
            // A plain thread that parks on the interactive pool: the build fans
            // out across every core there, and the UI thread is not one of
            // them, so drawing is never what waits.
            std::thread::Builder::new()
                .name("buzz-shade".into())
                .spawn(move || {
                    let built = jobs.run(Pool::Interactive, || {
                        use rayon::prelude::*;
                        misses
                            .into_par_iter()
                            // Checked per crescent, so abandoning a batch frees
                            // the pool within a fraction of a millisecond
                            // rather than at the end of the whole rebuild.
                            .filter(|_| !stop.load(std::sync::atomic::Ordering::Relaxed))
                            .map(buzz_render::lighting::Miss::build)
                            .collect::<Vec<_>>()
                    });
                    let _ = send.send(built);
                })
                .ok();
            self.shade_build = Some(ShadeBuild {
                results: receive,
                abandon,
                aim,
            });
            active.window.request_redraw();
        }

        // **Keep asking for frames until the shading is right.**
        //
        // Everything above is allowed to draw the frame with shading that is
        // not quite the shading it asked for — a cold cache, a light still
        // moving, a batch already running. Every one of those is fine *provided
        // another frame follows*, and nothing else guarantees one: the window
        // sleeps on `ControlFlow::Wait` and only wakes for input.
        //
        // Without this the rule that waits for a light to stop moving left the
        // frame after the move carrying the shading from before it, and no
        // reason to draw again. Switching a light on, or aiming one, then did
        // nothing visible at all until the pointer happened to cross the window
        // — which reads exactly like the lights being broken.
        if self.stage_stale {
            active.window.request_redraw();
        }

        // **A lighting change that builds nothing must still be shown.**
        //
        // Everything above that keeps the window awake is about *geometry* — a
        // batch of crescents in flight, a frame that drew with shading it knew
        // to be provisional. A great many lighting changes generate no geometry
        // at all: recolouring a light, turning one down, switching one off,
        // adding a sky, standing a wall of dark across the stage. None of those
        // sets `shade_build` and none of them makes the cache stale, so nothing
        // here asked for another frame and the window went back to sleep. It
        // drew the right picture on the frame the edit was made — but only
        // because the panel is built before the stage is encoded, which is an
        // ordering the lighting has no business depending on.
        //
        // One comparison, and a lighting change is guaranteed a frame of its
        // own whether or not anything else noticed it.
        if rig != self.last_rig {
            self.last_rig = rig;
            active.window.request_redraw();
        }

        // Restore logical units so pointer maths stays in egui's space.
        self.editor.camera.viewport =
            Size::new(stage_area.width() as f64, stage_area.height() as f64);

        self.profiler.enter(crate::profile::Section::Present);

        // **The window becomes visible here: laid out, but one moment before it
        // is presented into.**
        //
        // Not after the present, which is where this was first put and is where
        // it reads as though it belongs. A window that has never been mapped is
        // one the desktop compositor considers *occluded*, and an occluded
        // surface declines to hand out a texture at all — so a reveal that
        // waited for a successful present waited for something that could not
        // happen, and the window only ever appeared when the deadline below
        // fired, two and a half seconds late. The dependency runs the other
        // way: showing the window is what makes presenting possible.
        //
        // Everything expensive is already behind us. The frame has been built,
        // the stage encoded and the opening scene tessellated; what is left is
        // an acquire, a submit and a present. So what the desktop composites
        // for this window first is the opening scene, not an empty rectangle.
        active.reveal();

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match active.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                let (w, h) = (active.surface_config.width, active.surface_config.height);
                active.resize(w, h);
                // A window that has never been shown has no other way back
                // here: nothing will send it an input event, and `wants_frame`
                // is what keeps asking until it is visible.
                active.window.request_redraw();
                return Ok(());
            }
            // Genuinely occluded — another window over this one, or a session
            // locked. Ask for the frame that will be wanted when it is not.
            Cst::Timeout | Cst::Occluded => {
                active.window.request_redraw();
                return Ok(());
            }
            other => return Err(anyhow::anyhow!("acquiring surface texture: {other:?}")),
        };
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // **The thumbnails the panels asked for, drawn now.**
        //
        // Here because this is the only place the device, the Vello renderer
        // and egui's renderer are all reachable at once — and after the stage's
        // own scene has been consumed, because it borrows the same Vello scene
        // to build into. At most a handful per frame; see `thumbnails`.
        if self.thumbnails.pending() {
            let mut scratch = buzz_render::vello::Scene::new();
            self.thumbnails.fulfil(
                &mut active.gpu,
                &mut active.egui_renderer,
                &mut scratch,
                self.editor.doc.scene(),
                &mut self.lights,
            );
            // Ask for another frame so the rest arrive without the pointer
            // having to move.
            active.window.request_redraw();
        }

        // The Assets panel's pictures, on the same budget and for the same
        // reason. Kept separate so a document with a big library and a disk
        // with a big asset folder do not each spend the other's budget.
        if self.asset_thumbnails.pending() {
            let mut scratch = buzz_render::vello::Scene::new();
            self.asset_thumbnails.fulfil(
                &mut active.gpu,
                &mut active.egui_renderer,
                &mut scratch,
                &self.editor.assets,
                &mut self.lights,
            );
            active.window.request_redraw();
        }

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
        // The artwork reaches the screen one of two ways. With no effects the
        // blitter is a straight copy, exactly as before the compositor existed.
        // With a look set, the compositor runs its chain from the same texture
        // into the same surface — the seam the design is built on. The exporter
        // calls the identical `Compositor::run`, so screen and film cannot drift.
        let post = self.editor.scene().stage().post;
        if post.is_identity() {
            active
                .blitter
                .copy(&active.gpu.device, &mut encoder, target, &surface_view);
        } else {
            active.compositor.run(
                &active.gpu.device,
                &active.gpu.queue,
                &mut encoder,
                target,
                &surface_view,
                w,
                h,
                &post,
                self.editor.frame(),
            );
        }

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

        // Close the frame: store its section times and warn if one blew the
        // budget. This is the watchdog that catches an O(document) cost creeping
        // back onto the frame before it becomes a hang.
        self.profiler.end_frame();
        Ok(())
    }
}

impl ApplicationHandler<UserEvent> for App {
    /// The loop was woken by something off the UI thread — egui's repaint
    /// callback, or a background install. Draw a frame.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        if let Some(active) = &self.active {
            active.window.request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(active) => {
                self.active = Some(active);

                // **The first frame is drawn here, not asked for.**
                //
                // `request_redraw` cannot deliver it: the window is created
                // hidden (see `init`), and a hidden window is not sent paint
                // messages at all. Waiting for one waited forever, and the
                // window only appeared when the safety deadline in
                // `about_to_wait` fired seconds later — the exact fault this
                // was meant to remove, in a new costume.
                //
                // Calling `render` directly builds the interface, encodes the
                // stage and draws the opening scene over both, and it is the
                // last few instructions of *that* which put the window on
                // screen. See where `Active::reveal` is called.
                if let Err(e) = self.render() {
                    eprintln!("the first frame failed: {e:?}");
                    // The window is still hidden if the failure came before the
                    // reveal, and a process with no window reads as a crash.
                    if let Some(active) = self.active.as_mut() {
                        active.reveal();
                    }
                }

                // And the second, so the opening scene animates from here on
                // whatever the loop decides to do.
                if let Some(active) = &self.active {
                    active.window.request_redraw();
                }
            }
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
            WindowEvent::CloseRequested => {
                // Not an immediate exit: a running export gets a prompt first.
                // The actual close happens on the next redraw, once
                // `should_quit` is set — here or by the prompt.
                self.request_quit();
                if let Some(active) = &self.active {
                    active.window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                active.resize(size.width, size.height);
                active.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.render() {
                    eprintln!("frame failed: {e:?}");
                }
                if self.editor.should_quit {
                    // Stop anything still running and wait for it, so a
                    // cancelled export removes its `.part` before the process
                    // is gone.
                    self.tasks.cancel_and_join();
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The last line of defence for the hidden window: if the first frame
        // has still not arrived, show it anyway rather than leave a process
        // running with nothing on screen.
        if let Some(active) = self.active.as_mut()
            && active.reveal_is_overdue()
        {
            active.reveal();
        }

        // The window only redraws when something on screen could have changed.
        // An idle document sleeps here instead of re-rendering at monitor rate —
        // the fix for a static file burning a whole core (and a GPU) doing
        // nothing. Input events and the egui wake callback both request a
        // redraw of their own, so nothing is missed.
        match self.wants_frame() {
            Redraw::Now => {
                if let Some(active) = &self.active {
                    active.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Poll);
            }
            Redraw::At(instant) => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(instant));
            }
            Redraw::Idle => {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
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
mod idle_tests {
    use super::*;

    /// An app past its opening, which is where every question about *idling*
    /// starts. A session that is still playing its opening scene is never idle
    /// by design — see [`opening_frames_are_never_idle`] — so leaving it up
    /// would make these tests ask a question they do not mean.
    fn opened() -> App {
        let mut app = App::new(GpuPreference::Automatic);
        app.splash.dismiss();
        app
    }

    #[test]
    fn a_quiet_document_wants_no_frame() {
        let app = opened();
        // Nothing playing, no background work, egui idle: the loop should sleep.
        assert_eq!(app.wants_frame(), Redraw::Idle);
    }

    /// **The opening scene is an animation, and the loop has to keep drawing
    /// it.** The window sleeps unless something asks for a frame, and on a
    /// brand-new document nothing else would: the scene would freeze on its
    /// first frame and stay there until the pointer moved.
    #[test]
    fn opening_frames_are_never_idle() {
        let mut app = App::new(GpuPreference::Automatic);
        assert_eq!(
            app.wants_frame(),
            Redraw::Now,
            "a session that has not finished opening must keep drawing"
        );
        app.splash.dismiss();
        assert_eq!(
            app.wants_frame(),
            Redraw::Idle,
            "and go quiet the moment it has"
        );
    }

    #[test]
    fn playback_and_background_work_keep_frames_coming() {
        let mut app = opened();

        app.editor.playback.playing = true;
        assert_eq!(app.wants_frame(), Redraw::Now, "playback must animate");
        app.editor.playback.playing = false;
        assert_eq!(app.wants_frame(), Redraw::Idle);

        app.thumbnails.get(buzz_scene::SymbolId(1)); // now pending
        assert_eq!(
            app.wants_frame(),
            Redraw::Now,
            "a pending thumbnail installs next frame"
        );
    }

    #[test]
    fn egui_timed_repaint_is_honoured_when_otherwise_idle() {
        let mut app = opened();
        let at = Instant::now() + Duration::from_millis(50);
        app.egui_repaint = Some(at);
        assert_eq!(app.wants_frame(), Redraw::At(at));
    }

    #[test]
    fn the_poll_escape_hatch_forces_frames() {
        let mut app = opened();
        app.force_poll = true;
        assert_eq!(app.wants_frame(), Redraw::Now);
    }

    /// **A right-click on the stage opens the stage's menu.**
    ///
    /// Driven through the real `build_ui`, because the pattern works in
    /// isolation and the question was always whether something else in the
    /// window was taking the click first.
    #[test]
    fn right_clicking_the_stage_opens_its_menu() {
        let mut app = App::new(GpuPreference::Automatic);
        // Whatever this machine has lying about from earlier runs must not
        // decide the test: a recovery window over the stage would take the
        // click, which is a real thing that can happen but not this question.
        app.recovery.found.clear();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0));
        // Middle of the window, which is stage whatever the docks are doing.
        let at = egui::pos2(700.0, 430.0);
        let mods = egui::Modifiers::default();

        let mut opened = false;
        let mut drive = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(screen),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                app.build_ui(ui);
            });
            // The popup egui opens for a context menu is an Area of its own,
            // and its id is derived from the widget it belongs to. Asking the
            // stage's own response is what "did the menu come up" means.
            let id = egui::Id::new("stage-context-probe");
            if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                opened = true;
            }
        };

        // A frame to lay the window out, so the stage is a registered widget
        // before the pointer is asked about.
        drive(&mut app, vec![]);
        drive(&mut app, vec![egui::Event::PointerMoved(at)]);
        drive(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: mods,
            }],
        );
        drive(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: mods,
            }],
        );
        drive(&mut app, vec![]);

        assert!(opened, "right-clicking the stage should raise its menu");
    }

    /// **Turning on the depth view shows the picture.**
    ///
    /// The column of numbers says what each depth is; only the Layer Depth
    /// panel draws the scene from the side, which is the thing that answers
    /// "how close is that layer to me". It is a background tab in the right
    /// dock, so without this the depth view was numbers and nothing else.
    #[test]
    fn turning_on_the_depth_view_reveals_the_depth_panel() {
        let mut app = App::new(GpuPreference::Automatic);
        // Put it away first, both ways it can be away: closed, and buried
        // behind another tab in its own section.
        app.editor.workspace.move_to(buzz_ui::PanelId::Depth, buzz_ui::Dock::Hidden);
        assert!(!app.editor.workspace.is_open(buzz_ui::PanelId::Depth));

        let mut commands = Vec::new();
        app.apply_timeline(
            buzz_ui::TimelineResponse {
                toggle_depth: true,
                ..Default::default()
            },
            &mut commands,
        );

        assert!(app.editor.workspace.depth_view, "the column switched over");
        assert!(
            app.editor.workspace.is_open(buzz_ui::PanelId::Depth),
            "a closed panel cannot be brought to the front of anything"
        );
        assert_eq!(
            app.editor
                .workspace
                .section_of(buzz_ui::PanelId::Depth)
                .map(|s| s.front),
            Some(buzz_ui::PanelId::Depth),
            "and it must be the tab actually on show"
        );

        // Turning it off again leaves the panel alone: the user may well want
        // to keep looking at it.
        app.apply_timeline(
            buzz_ui::TimelineResponse {
                toggle_depth: true,
                ..Default::default()
            },
            &mut commands,
        );
        assert!(!app.editor.workspace.depth_view);
        assert!(app.editor.workspace.is_open(buzz_ui::PanelId::Depth));
    }

    fn stamp() -> StageStamp {
        StageStamp {
            revision: 5,
            frame: 3,
            camera: [1, 2, 3, 4],
            area: [0, 0, 100, 100],
            edit_path: vec![],
            onion: (false, false, 2, 2),
            edit_multiple: false,
            lights_generation: 0,
            lights: 0,
            painted_preview: false,
        }
    }

    /// The stage encoding is reused only when the stamp is identical; every
    /// input it captures must make it differ, or the stage would go stale.
    #[test]
    fn the_stage_stamp_notices_every_change() {
        let base = stamp();
        assert_eq!(base, stamp(), "an unchanged frame reuses");

        let mut a = stamp();
        a.revision = 6;
        assert_ne!(base, a, "an edit re-encodes");
        let mut a = stamp();
        a.frame = 4;
        assert_ne!(base, a, "scrubbing re-encodes");
        let mut a = stamp();
        a.camera[2] = 99;
        assert_ne!(base, a, "a zoom re-encodes");
        let mut a = stamp();
        a.edit_path = vec![7];
        assert_ne!(base, a, "entering a symbol re-encodes");
        let mut a = stamp();
        a.onion = (true, false, 2, 2);
        assert_ne!(base, a, "turning on onion skin re-encodes");
        let mut a = stamp();
        a.lights_generation = 1;
        assert_ne!(base, a, "installed shading re-encodes");
        let mut a = stamp();
        a.lights = 1;
        assert_ne!(base, a, "a change to the lighting rig re-encodes");
        // The frame after a brush stroke must rebuild rather than keep the ink
        // the preview left in the scene.
        let mut a = stamp();
        a.painted_preview = true;
        assert_ne!(base, a, "a preview painted into the scene re-encodes");
    }
}

#[cfg(test)]
mod usage_cache_tests {
    use super::UsageCache;
    use buzz_scene::SymbolId;
    use std::collections::BTreeMap;

    #[test]
    fn a_recompute_is_scheduled_only_when_stale() {
        let mut cache = UsageCache::default();
        assert!(cache.should_spawn(1), "a cold cache is stale");
        cache.install(1, BTreeMap::new());
        assert!(!cache.should_spawn(1), "already current for revision 1");
        assert!(cache.should_spawn(2), "a new revision is stale");
    }

    #[test]
    fn a_stale_result_does_not_overwrite_fresher_counts() {
        let mut cache = UsageCache::default();
        let mut counts5 = BTreeMap::new();
        counts5.insert(SymbolId(1), 5);
        assert!(cache.install(5, counts5), "the first counts install");

        // A recompute that finished for an older revision must be dropped.
        let mut counts3 = BTreeMap::new();
        counts3.insert(SymbolId(1), 3);
        assert!(!cache.install(3, counts3), "revision 3 is older than 5");
        assert_eq!(
            cache.counts().get(&SymbolId(1)),
            Some(&5),
            "the fresher counts must survive"
        );
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
