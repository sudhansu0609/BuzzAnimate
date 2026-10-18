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
    let (pixels, width, height) = decode_png_rgba(bytes)?;
    winit::window::Icon::from_rgba(pixels, width, height).ok()
}

/// Decode a PNG's raw RGBA bytes, width and height — the part of
/// [`icon_from_png`] that has nothing to do with window icons, reused to
/// build the logo texture the custom title bar draws.
///
/// Returns `None` rather than failing: an image that does not decode is
/// simply not drawn.
fn decode_png_rgba(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .ok()?;
    let mut pixels = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    pixels.truncate(info.buffer_size());
    Some((pixels, info.width, info.height))
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
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
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

/// The resize arrow that lies along `direction` on screen — horizontal,
/// vertical, or one of the two diagonals, whichever the line is nearest.
///
/// A resize arrow has no head and tail, only a line, so the direction is folded
/// into half a turn first: the handle at the top-left and the one at the
/// bottom-right want the same arrow. Screen y runs down, so a positive-slope
/// line is the north-west/south-east one.
fn resize_cursor_along(direction: buzz_geom::Vec2) -> egui::CursorIcon {
    let turn = direction.y.atan2(direction.x).rem_euclid(std::f64::consts::PI);
    match (turn / std::f64::consts::FRAC_PI_4).round() as i32 % 4 {
        0 => egui::CursorIcon::ResizeHorizontal,
        1 => egui::CursorIcon::ResizeNwSe,
        2 => egui::CursorIcon::ResizeVertical,
        _ => egui::CursorIcon::ResizeNeSw,
    }
}

fn opens_as_document(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(buzz_doc::EXTENSION))
}

/// A `DrawCache` with the same `BUZZ_NO_SYMBOL_CACHE` escape hatch `App::new`
/// honours — factored out so a document tab switch (D2) can rebuild a clean
/// one without silently dropping that setting. See
/// `App::reset_stage_caches`.
fn fresh_light_cache() -> buzz_render::document::DrawCache {
    // The symbol-encoding cache is on by default; `BUZZ_NO_SYMBOL_CACHE=1`
    // turns it off so an instance-heavy document draws every instance
    // live, for bisecting a suspected reuse bug.
    let mut cache = buzz_render::document::DrawCache::new();
    if std::env::var("BUZZ_NO_SYMBOL_CACHE").is_ok() {
        cache.set_symbol_reuse(false);
    }
    cache
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
/// Taken from the theme by default, so the surround follows the interface —
/// unless `workspace` carries a user override from View > Pasteboard
/// Colour…, in which case that wins. Converted here because Vello wants a
/// `peniko::Color` while the palette (and the override) speak egui's.
fn pasteboard(workspace: &buzz_ui::Workspace) -> Color {
    let c = match workspace.pasteboard {
        Some([r, g, b, _a]) => egui::Color32::from_rgb(r, g, b),
        None => Palette::pasteboard(),
    };
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
    /// Built lazily, only once a frame is drawn in bands. See
    /// [`Active::ensure_scratch`].
    scratch: Option<TargetTexture>,

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
    /// A video to pull apart onto a reference layer, a frame at a time.
    VideoReference,
    /// A folder of numbered pictures, one drawing to a frame.
    SequenceFolder,
    ImportSound,
    /// Sequence ▸ Set Master Audio\u{2026}
    SetMasterAudio,
    /// File ▸ Import, into the stage or into the library.
    ImportInto(buzz_scene::ImportTarget),
    /// The root of an Animate asset library.
    AnimateAssets,
    /// Where an export should be written.
    Export(buzz_ui::ExportKind),
    /// Where to write the document as an Animate `.fla`.
    ExportFla,
    /// A subtitle file to read onto the timeline.
    ImportCaptions,
    /// Where to write the caption layer as `.srt`.
    ExportCaptions,
    /// A reference image for AI ▸ Scene from Image.
    AiSceneImage,
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
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
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

/// D3: several `.fla`/`.xfl`/`.swf` files chosen at once for Import to Stage
/// or Import to Library. Each is read and merged in turn — one undo step
/// per file, exactly as a single import always was — and this is what the
/// one status line at the end is built from. A failure is recorded and the
/// rest of the batch still runs; see `App::advance_import_queue`.
struct ImportBatch {
    /// How many files this batch started with, for "Imported N files".
    total: usize,
    /// Name and reason, one entry per file that could not be read or merged.
    failed: Vec<(String, String)>,
}

/// What a background AI task produced.
///
/// The LLM/VLM call is the slow part and runs off the UI thread; the result
/// is committed through `Document::edit` on the frame that collects it, so it
/// lands as one ordinary undo step — see `tasks.rs`'s module docs, rule 5.
/// What the Arrange by type & style task reports (AS2).
enum ArrangeMsg {
    /// A plan to preview. `last` is false while a VLM pass is still to come.
    Plan {
        plan: buzz_ai::asset_index::ArrangePlan,
        note: String,
        last: bool,
    },
    Applied(Result<buzz_ai::asset_index::MoveLog, String>),
    Undone(Result<usize, String>),
}

enum AiResult {
    /// AI ▸ Scene from Image's background half: a scene reconstructed from a
    /// reference image and the current library. `embed_note` explains, for
    /// the final status, when SigLIP visual matching was skipped (model not
    /// installed, or a crop/render that failed to embed) — empty when it
    /// ran normally.
    Scene {
        reconstructed: Box<buzz_ai::scene_builder::ReconstructedScene>,
        embed_note: String,
        /// What the AI panel shows for review and rebuilds from (F7.1).
        review: Box<SceneReviewData>,
    },
    /// AI ▸ Transcribe (F7.9): timed lines from a library sound.
    Transcribed {
        sound: String,
        transcript: Box<buzz_ai::transcribe::Transcript>,
    },
    /// The Assistant's `transcribe` (H1): the text and lines of a sound or
    /// file, and which model heard it. Only the job's result — nothing lands
    /// in the document or the AI panel.
    AgentTranscript {
        source: String,
        transcript: Box<buzz_ai::transcribe::Transcript>,
        model: &'static str,
        /// `"en"`, `"hi"` or `"mixed"`.
        language: String,
        /// Whether each line's start and end are the model's own (base.en) or
        /// only the edges of the window it was heard in (multilingual).
        timing: &'static str,
    },
    /// The Assistant's `ai_character_from_image` (H2): the wizard's whole
    /// pipeline, ready to be added to the document on the UI thread, rigged
    /// as the call's `rig` asked (layers unless it said bones).
    Character(Box<buzz_ai::character::AssembledCharacter>, buzz_rig::RigKind),
    /// The pose sheet of a character just created from an image (F7.0).
    PoseSheet(Vec<(String, image::RgbaImage)>),
    /// AI ▸ Direct Story's background half: a parsed director script, still
    /// to be executed against the document on the UI thread.
    Directed {
        script: Box<buzz_ai::director::DirectorScript>,
        beat_count: usize,
    },
    /// AI ▸ Auto-Rig Vector Character's background half: a character rigged
    /// from a skeleton *detected* in a render of the symbol itself — never
    /// the old fixed-fraction guess — ready to land where `target` says.
    Rigged {
        target: RigTarget,
        sym_name: String,
        rerigged: Box<buzz_ai::rerig::ReriggedCharacter>,
    },
    /// File ▸ AI ▸ Produce Episode… / the `produce_episode` tool (P6): what
    /// the producer made; the dialog's run opens the `.buzz`.
    Produced {
        report: Box<crate::produce::ProduceReport>,
        open: bool,
    },
}

/// What the Assistant's `transcribe` listens to.
pub(crate) enum TranscribeSource {
    /// A sound in the document's library, by name.
    Sound(String),
    /// An audio file on disk.
    File(std::path::PathBuf),
}

/// What `wait_for_jobs` reports for a finished AI job, from what the task
/// produced (the commit adds the status line to it).
fn ai_job_summary(result: &AiResult) -> serde_json::Value {
    use serde_json::json;
    match result {
        AiResult::Scene { reconstructed, .. } => json!({
            "matched": reconstructed.matched.len(),
            "unmatched": reconstructed.unmatched.len(),
        }),
        AiResult::Directed { beat_count, .. } => json!({"beats": beat_count}),
        AiResult::Rigged { sym_name, rerigged, .. } => json!({
            "symbol": sym_name,
            "bones": rerigged.armature.armature.bones.len(),
        }),
        AiResult::AgentTranscript { source, transcript, model, language, timing } => {
            let segments: Vec<serde_json::Value> = transcript
                .segments
                .iter()
                .map(|s| json!({"start_s": s.start, "end_s": s.end, "text": s.text}))
                .collect();
            let text = transcript.text();
            let mut out = json!({
                "source": source,
                "language": language,
                "model": model,
                "timing": timing,
                // There is no speaker separation: the model must not assume one.
                "speakers": "not identified",
                "audio_seconds": transcript.audio_seconds,
                "segments": segments,
            });
            // The model gets a result once and only so much of it: a long
            // speech would otherwise be sent twice and cut off. The segments
            // carry every word, so the joined text is dropped when it is long.
            if text.chars().count() <= 6000 {
                out["text"] = json!(text);
            } else {
                out["text_omitted"] = json!("long speech: the full text is in the segments");
            }
            out
        }
        _ => json!({}),
    }
}

/// A Scene from Image analysis, kept so the AI panel's **Build** can place
/// the objects with the choices the user made in its dropdowns.
struct SceneReviewData {
    layout: buzz_ai::scene_builder::DetectedSceneLayout,
    candidates: Vec<buzz_ai::scene_builder::LibraryAssetCandidate>,
    /// Per object: SigLIP shortlist (candidate index, cosine), best first.
    shortlists: Vec<Vec<(usize, f64)>>,
    /// Per object: the chosen shortlist position.
    chosen: Vec<Option<usize>>,
    stage_size: buzz_geom::Size,
    image_name: String,
    secs: f64,
}

/// Where AI ▸ Auto-Rig Vector Character's finished armature should land,
/// decided on the UI thread before the background task starts (see
/// `App::ai_rig_vector_character_async`) and used unchanged once it returns.
enum RigTarget {
    /// A stage instance of the symbol was selected: replace it in place —
    /// same layer, same index, same transform — so the drawing does not end
    /// up on stage twice.
    ReplaceInstance {
        layer: buzz_scene::LayerId,
        object: buzz_scene::ObjectId,
        transform: buzz_geom::Affine,
    },
    /// Nothing was selected on stage: add the armature on a new layer, at
    /// `at` — `Editor::place_symbol`'s own convention for "here".
    NewLayer { at: Point },
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
    /// Each font family traced in its own face, for the picker. View state, and
    /// built lazily: a family is outlined the first time its row is drawn.
    font_previews: buzz_ui::FontPreviews,
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
    bounds_table: Option<(
        u64,
        std::collections::HashMap<buzz_scene::SymbolId, buzz_geom::Rect>,
    )>,
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
    /// The OS window title as it was last set, so `render` calls `set_title`
    /// only when the document's display name actually changes.
    last_window_title: String,
    /// Which title bar this platform draws. See [`crate::title_bar`].
    title_bar: crate::title_bar::TitleBar,
    /// Window actions asked for from inside the title bar row this frame —
    /// drag, minimise, maximise, close, resize — drained by `render` after
    /// `run_ui`. A queue rather than acting immediately keeps them testable
    /// without a real window to act on.
    window_requests: Vec<crate::title_bar::WindowRequest>,
    /// Where the title bar row's own hit-targets were drawn this frame.
    title_rects: crate::title_bar::TitleBarRects,
    /// Whether the window is maximised, tracked here because an undecorated
    /// window's own maximise glyph has to show Restore instead once it is.
    window_maximized: bool,
    /// The logo, decoded once and uploaded as a texture the custom title bar
    /// draws at 18 pt. `None` until the first frame that needs it.
    logo_texture: Option<egui::TextureHandle>,
    /// The pointer's last known position in the window, in physical pixels —
    /// `window_event`'s `CursorMoved` keeps this current so a `MouseInput`
    /// press elsewhere in the same handler can test it against the resize
    /// border without egui's own input state, which is a frame behind.
    last_cursor: Option<egui::Pos2>,
    /// Per-frame section timing and the over-budget warning. See
    /// [`crate::profile`].
    profiler: crate::profile::FrameProfiler,
    /// Every export, run a few at a time on the task registry. Replaces the old
    /// single slot that refused a second export outright.
    exports: crate::export_service::ExportQueue,
    /// How many exports the free VRAM allowed when the queue last started
    /// ([`crate::export_service::vram_cap`]); 2 until first sampled.
    export_vram_cap: usize,
    /// The pressure the pen last reported, `0.0`–`1.0`.
    ///
    /// Remembered between events because a gesture's press and release arrive
    /// on their own and do not always carry a force of their own; `None` is a
    /// device with no sensor, which reads as drawing at full pressure.
    pen_pressure: Option<f64>,
    /// Where a Space-drag or middle-button pan last measured the pointer, in
    /// stage-local screen points (see `local` in `handle_stage_input`).
    ///
    /// `ctx.input(|i| i.pointer.delta())` is a *per-frame* value — read only
    /// on the frames egui recognises the gesture as a drag, it silently
    /// drops however far the pointer already travelled during the frames it
    /// still read as a plain click (egui's own click/drag threshold). Set
    /// from `press_origin` the moment the drag is recognised and updated
    /// from every intermediate move after that, the same way the Hand
    /// tool's own gesture (`ToolMachine::last_screen`) already does, this
    /// recovers that motion instead of starting the pan late — I4b of
    /// `docs/INTERACTION_FIXES_PLAN.md`.
    pan_last_screen: Option<Point>,
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
    /// **The encode thread** (E4 in `docs/BIMARI5_LAG_PLAN.md`). While it runs,
    /// `lights` holds a stand-in and the real cache is on that thread: see
    /// `encode_ahead_source`. Every use of `self.lights` is one of two kinds —
    /// *stage-only* (what the stage frame's own encode reads or writes; while
    /// lent it comes from the pipeline: `light_detail`, `light_scale`,
    /// `light_last_lines`, `light_cache_empty`, `Prepared`) or *other*
    /// (thumbnails, previews, Test Movie: they run against the stand-in).
    pipeline: crate::encode_ahead::Pipeline,
    /// **E7: stamps built ahead for a seek or a scrub** (home cache only; see
    /// `encode_ahead::SeekAhead`).
    seek: crate::encode_ahead::SeekAhead,
    /// **S9: the warm-up at open** (`warm_open.rs`): the film's stamps built on
    /// workers behind the Tasks panel's progress bar, the buffer scale sized
    /// once, and a re-warm when the zoom settles somewhere new.
    warm: crate::warm_open::WarmState,
    /// The stage rectangle (physical px) and output scale of the last frame,
    /// which the warm-up reads at the top of the next one.
    warm_stage: Option<(buzz_geom::Rect, f64)>,
    /// Autosaves found on launch, while the prompt is still up.
    ///
    /// `pub(crate)` rather than private so a headless harness elsewhere in
    /// this crate — the UI snapshot harness in `ui_snapshot.rs`, following
    /// the same pattern as `dock_drag_tests::Drive` below — can clear it
    /// before drawing, the same way `dock_drag_tests::Drive::new` does: a
    /// crash-recovery dialog left over from a previous run on this machine
    /// must not appear over a picture that is meant to show the ordinary
    /// chrome.
    pub(crate) recovery: buzz_ui::RecoveryState,
    /// The Ctrl+K command palette's open state and query.
    command_palette: buzz_ui::CommandPaletteState,
    /// Named version snapshots of the document, and whether the list is open.
    snapshots: buzz_doc::SnapshotLibrary,
    show_snapshots: bool,
    /// View > Pasteboard Colour…'s open state. The colour itself lives in
    /// `Workspace::pasteboard` — this is only whether the dialog is showing.
    show_pasteboard_dialog: bool,
    /// The keyboard-shortcut editor's open/capture state.
    shortcut_editor: buzz_ui::ShortcutEditorState,
    /// The camera panel's between-frames state (the angle name being typed).
    camera_panel_state: buzz_ui::CameraPanelState,
    /// The user's saved commands (named Actions scripts).
    commands: buzz_doc::CommandLibrary,
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
    /// **Assets ▸ Arrange by type & style…** (AS2): the preview dialog, the
    /// task building the index and plan (or applying / undoing), and where
    /// that task reports.
    arrange: buzz_ui::ArrangeDialogState,
    arrange_task: Option<crate::tasks::TaskId>,
    /// **File ▸ AI ▸ Produce Episode…** (PRODUCER_PLAN P6): the dialog's choices.
    produce: buzz_ui::produce_dialog::ProduceDialogState,
    produce_task: Option<crate::tasks::TaskId>,
    arrange_rx: Option<crossbeam_channel::Receiver<ArrangeMsg>>,
    /// The open character's loaded document, and the wardrobe and poses derived
    /// from it. Held here rather than re-read from disk every frame: a
    /// character is a whole document, and enumerating its colours or posing it
    /// is a walk of the whole thing. Rebuilt when the open character changes or
    /// after a recolour writes it back. See [`Self::cast_panel`].
    cast_sheet: Option<CastSheet>,
    /// The Export dialog's test render. See [`crate::preview`].
    preview: crate::preview::Preview,
    /// The whole film, playing in an overlay — Ctrl+Enter. See
    /// [`crate::test_movie`].
    test_movie: Option<crate::test_movie::TestMovie>,
    /// Textures `test_movie` handed back on close, freed once the renderer is
    /// reachable again.
    test_movie_retired: Vec<egui::TextureId>,
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
    /// How many horizontal strips the stage is drawn in this frame — `0` for
    /// the ordinary single pass. Set once [`buzz_render::document::DrawCache::over_budget`]
    /// says the frame does not fit even at `LightDetail::Flat`, the bottom of
    /// the automatic ladder, and held across frames until a band comfortably
    /// fits, doubling (capped at 64) for the next frame if it still does not.
    /// See `render_stage_bands` and Phase 4d in
    /// `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
    stage_bands: u32,
    /// Band frames drawn since the whole-frame ladder last ran. S3
    /// (`docs/BIMARI5_LAG_PLAN.md`): band mode re-runs the ladder every
    /// [`BAND_LADDER_RECHECK_FRAMES`] so a frame that now fits leaves it.
    band_frames_since_ladder: u32,
    /// The playhead frame the last band frame was drawn at, so a jump of
    /// more than one frame re-runs the ladder. See `band_recheck_due`.
    band_last_frame: u32,
    /// Set once the bands have been composed into `Active::target`: the
    /// target's size and the background they were drawn over. `None` whenever
    /// the target holds anything else (a single-pass frame, a new document).
    /// With an equal stage stamp it means the banded picture is still there.
    banded_on_target: Option<(u32, u32, Color)>,
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
    /// AI tasks running on a thread, by the task that is computing them.
    ///
    /// The same shape as `loading`: the LLM/VLM call is the slow part and runs
    /// off the UI thread, and what it built lands here to be committed through
    /// one `Document::edit` on the frame that collects it.
    ai_results: std::collections::HashMap<crate::tasks::TaskId, crossbeam_channel::Receiver<AiResult>>,
    /// **The AI panel** (F7.0): its own state, and the AI tasks started from
    /// it — whose results wait in the panel for Build/Apply rather than
    /// being committed the moment they arrive, as the menu commands' are.
    pub(crate) ai_panel: buzz_ui::ai_panel::AiPanelState,
    /// The Assistant's run, if one is in flight (`crate::agent_bridge`).
    pub(crate) agent_bridge: crate::agent_bridge::AgentBridge,
    /// The local HTTP API other Buzzcaf apps drive the window through
    /// (`crate::api`): its server, if listening, and the jobs callers started.
    pub(crate) api: crate::api::ApiState,
    /// The clients for BuzzEdit and Buzzcaf Studio (`crate::estate`, steps E2
    /// and E3): their running flows and the dialog that starts them.
    pub(crate) estate: crate::estate::EstateState,
    /// Voice commands: the microphone service and its settings (`crate::voice_bridge`).
    pub(crate) voice_bridge: crate::voice_bridge::VoiceBridge,
    ai_from_panel: std::collections::HashSet<crate::tasks::TaskId>,
    ai_scene_pending: Option<Box<SceneReviewData>>,
    ai_story_pending: Option<Box<buzz_ai::director::DirectorScript>>,
    ai_rig_pending: Option<(RigTarget, String, Box<buzz_ai::rerig::ReriggedCharacter>)>,
    ai_pose_images: Option<Vec<(String, image::RgbaImage)>>,
    /// Set by an AI panel action just before it starts a task, so the task
    /// is remembered as the panel's (see `track_ai`).
    ai_next_from_panel: bool,
    /// **The AI jobs the Assistant started in its current run** (Stage H3),
    /// which `wait_for_jobs` reports on. They are ordinary tasks in `tasks`;
    /// this only remembers which ones the model asked for and how they ended.
    pub(crate) agent_jobs: Vec<crate::agent::jobs::AgentJob>,
    /// The task the last `track_ai` registered, so a command that has just
    /// started an AI task can learn its id without every starter returning it.
    pub(crate) last_ai_task: Option<crate::tasks::TaskId>,
    /// The file picker, if one is open, and what it was opened for.
    picker: crate::dialogs::Pending<Pick>,
    /// The multi-file picker (D3), for the four Import dialogs that accept
    /// more than one file at once: Import to Stage, Import to Library,
    /// Import Sound and Import Image.
    picker_many: crate::dialogs::PendingMany<Pick>,
    /// Files queued for `Pick::ImportInto` once more than one was chosen —
    /// each merged in turn, off the picker, so only one read-and-merge
    /// pipeline is ever in flight (see `App::advance_import_queue`).
    import_queue: std::collections::VecDeque<(buzz_scene::ImportTarget, std::path::PathBuf)>,
    /// The batch a queued multi-file import belongs to, for the one summary
    /// line it ends with. `None` for a single file — that keeps behaving
    /// exactly as it always did, one status line of its own.
    import_batch: Option<ImportBatch>,
    /// Every long-running piece of work in the program.
    ///
    /// On `App`, not on `Editor`, because work outlives documents — see the
    /// module docs in `tasks.rs` for the bug that taught us this.
    pub(crate) tasks: crate::tasks::TaskRegistry,
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
    /// Every section on screen this frame, for a dragged panel to be dropped
    /// on. Filled as the sections are drawn, like `dock_rects`.
    drop_zones: Vec<DropZone>,
    /// The panel being lit up and how brightly, resolved once per frame from
    /// `Workspace::highlight` and the clock.
    highlight_frame: Option<(buzz_ui::PanelId, f32)>,
    /// Where the Timeline's header line left room for its layer tools this
    /// frame, set by the section header just before the panel is drawn and
    /// taken by it. `None` draws them in their own strip under the grid.
    timeline_tools_slot: Option<egui::Rect>,
    /// When the current highlight began.
    highlight_since: Option<(buzz_ui::PanelId, f64)>,
    /// Floating windows to put at a point on the next frame — the point a
    /// panel was dropped at. egui remembers a window's position itself, so a
    /// new position has to be pressed on it once.
    place_floats: Vec<(buzz_ui::workspace::GroupId, egui::Pos2)>,
    /// How much width the columns and the stage had to share this frame.
    ///
    /// Recorded where the columns are laid out, and read by the splitters, so
    /// a boundary cannot be dragged past the point where its neighbours are
    /// already as narrow as they go.
    dock_room: f32,
    /// The opening scene, drawn over the interface for the first second or so
    /// of a session and then dissolved. See [`buzz_ui::splash`].
    splash: buzz_ui::SplashState,

    // --- D2: several documents open at once, as tabs ------------------
    // See "D2 design" in docs/DOCUMENTS_PLAN.md.
    /// **Every open tab.** The live one — `documents[active_document]` — is
    /// only ever a placeholder: its real `Document` and [`crate::editor::DocView`]
    /// live in `self.editor` while it is on screen, and are swapped back in
    /// here the moment a different tab is focused. See [`Self::tab_document`],
    /// which is the one place that distinction has to be made, and
    /// [`Self::switch_to_tab`], which makes the swap.
    documents: Vec<OpenDocument>,
    /// Which tab is live in `self.editor` right now.
    active_document: usize,
    /// Paths still waiting to open as their own tab after a multi-select
    /// File ▸ Open — only one background read runs at a time (see
    /// [`Self::loading_already`]), so these wait their turn and are drained
    /// one at a time by [`Self::advance_open_queue`]. D2b will fan a batch of
    /// `.fla`s out onto the job pool instead of queuing them.
    open_queue: std::collections::VecDeque<std::path::PathBuf>,
    /// The "save before closing?" prompt for a dirty document — raised by
    /// Ctrl+W on a dirty tab, or by Quit with any tab dirty. `None` when
    /// nothing is being asked.
    close_prompt: Option<ClosePrompt>,
}

/// **D2.** One open document that is not the live one in `self.editor` right
/// now — see [`App::documents`]. Built so a tiled-viewport mode (D2b) can own
/// one of these per viewport without reworking the split between `Document`
/// and its view state again.
#[derive(Default)]
struct OpenDocument {
    doc: Document,
    view: crate::editor::DocView,
}

/// **D2.** What `App::close_prompt` is asking, and what happens once the
/// document it is about to lose is out of the way.
struct ClosePrompt {
    /// Index into `App::documents` of the tab being asked about.
    index: usize,
    then: CloseThen,
}

/// **D2.** Why a document is being asked about before it goes away.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseThen {
    /// Ctrl+W: only this one tab closes.
    CloseTab,
    /// File ▸ Exit: once every dirty tab has been asked about, the whole
    /// program quits.
    Quit,
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

        let jobs = Arc::new(JobSystem::new());
        let mut app = Self {
            active: None,
            editor,
            jobs: Arc::clone(&jobs),
            preference,
            font_previews: buzz_ui::FontPreviews::default(),
            proxy: None,
            egui_repaint: None,
            force_poll: std::env::var("BUZZ_POLL").is_ok(),
            stage_stamp: None,
            last_rig: 0,
            last_stage_area: None,
            last_window_title: String::new(),
            title_bar: crate::title_bar::TitleBar::for_platform(),
            window_requests: Vec::new(),
            title_rects: crate::title_bar::TitleBarRects::default(),
            window_maximized: false,
            logo_texture: None,
            last_cursor: None,
            scroll_extent: None,
            bounds_table: None,
            lights_generation: 0,
            retain_stage: std::env::var("BUZZ_NO_RETAIN").is_err(),
            profiler: crate::profile::FrameProfiler::default(),
            exports: crate::export_service::ExportQueue::default(),
            export_vram_cap: 2,
            pen_pressure: None,
            pan_last_screen: None,
            quit_prompt: false,
            presets: crate::presets::PresetLibrary::load(),
            lights: fresh_light_cache(),
            pipeline: crate::encode_ahead::Pipeline::for_app(Arc::clone(&jobs)),
            seek: crate::encode_ahead::SeekAhead::new(jobs),
            warm: crate::warm_open::WarmState {
                on: crate::warm_open::enabled(),
                needs: crate::warm_open::enabled(),
                ..Default::default()
            },
            warm_stage: None,
            recovery: buzz_ui::RecoveryState::default(),
            command_palette: buzz_ui::CommandPaletteState::default(),
            snapshots: buzz_doc::SnapshotLibrary::user(),
            show_snapshots: false,
            show_pasteboard_dialog: false,
            shortcut_editor: buzz_ui::ShortcutEditorState::default(),
            camera_panel_state: buzz_ui::CameraPanelState::default(),
            commands: buzz_doc::CommandLibrary::user(),
            last_crash_revision: None,
            scene_rename: None,
            animate_import: None,
            merging: false,
            stage_area_min: egui::Pos2::ZERO,
            thumbnails: crate::thumbnails::Thumbnails::default(),
            asset_thumbnails: crate::thumbnails::AssetThumbnails::default(),
            arrange: buzz_ui::ArrangeDialogState::default(),
            arrange_task: None,
            produce: buzz_ui::produce_dialog::ProduceDialogState::default(),
            produce_task: None,
            arrange_rx: None,
            cast_sheet: None,
            preview: crate::preview::Preview::default(),
            test_movie: None,
            test_movie_retired: Vec::new(),
            shade_build: None,
            shade_aim: 0,
            stage_stale: false,
            stage_bands: 0,
            band_frames_since_ladder: 0,
            band_last_frame: 0,
            banded_on_target: None,
            sound_decode: None,
            usage_cache: UsageCache::default(),
            scripting: None,
            loading: std::collections::HashMap::new(),
            ai_results: std::collections::HashMap::new(),
            ai_panel: buzz_ui::ai_panel::AiPanelState::default(),
            agent_bridge: Default::default(),
            api: Default::default(),
            estate: Default::default(),
            voice_bridge: Default::default(),
            ai_from_panel: std::collections::HashSet::new(),
            ai_scene_pending: None,
            ai_story_pending: None,
            ai_rig_pending: None,
            ai_pose_images: None,
            ai_next_from_panel: false,
            agent_jobs: Vec::new(),
            last_ai_task: None,
            picker: crate::dialogs::Pending::default(),
            picker_many: crate::dialogs::PendingMany::default(),
            import_queue: std::collections::VecDeque::new(),
            import_batch: None,
            tasks: crate::tasks::TaskRegistry::default(),
            dock_rects: Vec::new(),
            drop_zones: Vec::new(),
            highlight_frame: None,
            timeline_tools_slot: None,
            highlight_since: None,
            place_floats: Vec::new(),
            dock_room: 0.0,
            splash: buzz_ui::SplashState::default(),
            // The one document `create_document` (above) just built in place
            // is tab 0; `documents[0]` is its placeholder — see
            // `App::tab_document`.
            documents: vec![OpenDocument::default()],
            active_document: 0,
            open_queue: std::collections::VecDeque::new(),
            close_prompt: None,
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
    /// Which of [`Self::wants_frame`]'s reasons hold right now, by name, for
    /// the `BUZZ_FRAME_LOG` line: an idle editor that keeps drawing says why.
    fn redraw_reasons(&self) -> String {
        let reasons = [
            ("force_poll", self.force_poll),
            ("splash", self.splash.is_open()),
            ("not_shown", matches!(&self.active, Some(a) if !a.shown)),
            ("playing", self.editor.playback.playing),
            ("test_movie", self.test_movie.is_some()),
            ("restyle", self.editor.restyle),
            ("thumbnails", self.thumbnails.pending()),
            ("asset_thumbnails", self.asset_thumbnails.pending()),
            ("shade_build", self.shade_build.is_some()),
            ("sound_decode", self.sound_decode.is_some()),
            ("usage_cache", self.usage_cache.in_flight.is_some()),
            ("scripting", self.scripting.is_some()),
            ("merging", self.merging),
            ("loading", !self.loading.is_empty()),
            ("warm", self.warm.busy()),
            ("animate_import", self.animate_import.is_some()),
            ("exports", !self.exports.is_idle()),
            ("picker", self.picker.busy()),
            ("tasks", !self.tasks.is_empty()),
            ("egui_repaint", self.egui_repaint.is_some()),
        ];
        let on: Vec<&str> = reasons.iter().filter(|(_, b)| *b).map(|(n, _)| *n).collect();
        let take = |c: &std::sync::atomic::AtomicU64| c.swap(0, std::sync::atomic::Ordering::Relaxed);
        format!(
            "{} wakes user {} egui_event {} stale {} rig {}",
            if on.is_empty() { "-".to_string() } else { on.join(",") },
            take(&WAKE_USER_EVENT),
            take(&WAKE_EGUI_EVENT),
            take(&WAKE_STALE),
            take(&WAKE_RIG),
        )
    }

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
            || self.test_movie.is_some()
            || self.editor.restyle
            || self.thumbnails.pending()
            || self.asset_thumbnails.pending()
            || self.shade_build.is_some()
            || self.sound_decode.is_some()
            || self.usage_cache.in_flight.is_some()
            || self.scripting.is_some()
            || self.merging
            || !self.loading.is_empty()
            || self.warm.busy()
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

    /// Open with a particular layout rather than whatever `Workspace::load`
    /// found on disk — `ui_snapshot.rs`'s own module and `main.rs`'s
    /// `--ui-snapshot` both need one, and `editor` is private to this
    /// module, so a builder is the way in rather than a second public field.
    pub fn with_workspace(mut self, workspace: buzz_ui::Workspace) -> Self {
        self.editor.workspace = workspace;
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
            // The system titlebar has two states, not six: what it needs to
            // know is whether the window it is framing is dark, so that its
            // own buttons are drawn to contrast with it.
            .with_theme(Some(if scheme.is_dark() {
                winit::window::Theme::Dark
            } else {
                winit::window::Theme::Light
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
        // **Windows: a custom title bar.** The app draws its own drag area,
        // menu row and minimise/maximise/close buttons — see `title_bar.rs`
        // — so the native chrome is turned off. `with_undecorated_shadow`
        // keeps the drop shadow an undecorated window would otherwise lose,
        // which is most of what makes it still read as a window.
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows as _;
            attrs
                .with_taskbar_icon(taskbar_icon())
                .with_decorations(false)
                .with_undecorated_shadow(true)
        };
        // **macOS: the native title bar, inset.** The traffic lights stay
        // where the OS puts them; only the bar behind them is pulled
        // transparent and the content view extends under it, so the app's
        // own row can sit level with them instead of below a second bar.
        #[cfg(target_os = "macos")]
        let attrs = {
            use winit::platform::macos::WindowAttributesExtMacOS as _;
            attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true)
        };
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .context("creating the window")?,
        );

        let gpu = GpuContext::new_blocking(&self.preference)?;
        // P2 (`docs/IMPORT_PERF_PLAN.md`): the budget belongs to this
        // `DrawCache`, not the process — captured once from the `GpuContext`
        // it serves, so `reconsider`/`over_budget`/`bands_needed` judge
        // against the scale this window's own GPU actually got, regardless
        // of what any other GPU context in the process later does. V2/V3
        // (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): the window starts at
        // `buffer_scale` `1` and `reconsider` grows it towards
        // `max_buffer_scale` only once a frame actually overflows, so
        // `max_scale` is set too.
        self.lights.set_scale(gpu.buffer_scale);
        self.lights.set_max_scale(gpu.max_buffer_scale);
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
        // Stage 6, item 4: without this, egui never builds the accesskit
        // tree at all — every widget's role and label (see
        // `buzz_ui::widgets::icon_button`) would exist only for a sighted
        // user to hover over.
        egui_ctx.enable_accesskit();
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
            scratch: None,
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
            // swapchain textures do not allow. `COPY_DST` is for
            // `render_stage_bands` (Phase 4d): each band renders into
            // `scratch` and is then copied, one strip at a time, into its own
            // rows of this texture.
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST,
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

    /// A second target the same size as [`Self::target`], rendered into one
    /// band at a time by [`render_stage_bands`] and then copied, strip by
    /// strip, into the real one. Kept separate from `target` because copying
    /// a texture into itself is undefined, and because a whole-target
    /// `STORAGE_BINDING` render has to land somewhere before any of it can be
    /// trusted to end up in the right rows.
    fn ensure_scratch(&mut self) {
        let (w, h) = (self.surface_config.width, self.surface_config.height);
        if matches!(&self.scratch, Some(t) if t.width == w && t.height == h) {
            return;
        }
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage-band-scratch"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: buzz_render::RENDER_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.scratch = Some(TargetTexture {
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

/// E0: `BUZZ_FRAME_LOG=1` writes one `frame_log` line per rendered frame to
/// the tracing log. Read once; a cached bool afterwards.
/// `BUZZ_AUTOPLAY=<seconds>`: see `about_to_wait`.
fn autoplay_after() -> Option<Duration> {
    static AFTER: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *AFTER.get_or_init(|| {
        std::env::var("BUZZ_AUTOPLAY").ok().and_then(|v| v.trim().parse::<f64>().ok()).map(Duration::from_secs_f64)
    })
}
static AUTOPLAYED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static AUTOPLAY_FROM: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// `BUZZ_FRAME_LOG` diagnostics: wakes since the last logged frame, by source.
static WAKE_USER_EVENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WAKE_EGUI_EVENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WAKE_STALE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WAKE_RIG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn frame_log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("BUZZ_FRAME_LOG").is_ok_and(|v| v != "0" && !v.is_empty()))
}

/// Where this frame's stage encoding comes from (E4).
enum PipeSource {
    /// Not pipelined: the render thread encodes it (the ladder, or reuse).
    Normal,
    /// The encode thread's finished encoding of exactly this frame.
    Ready(crate::encode_ahead::Prepared),
    /// The encoding of this frame is already in `active.vello`.
    Shown,
    /// The encode thread did not finish this frame within a frame interval;
    /// the previous picture stays up.
    NotReady,
}

/// The cache's detail level as the stage frame last judged it - from the
/// encode thread's cache while it is lent. Stage-only use of `App::lights`.
fn light_detail_of(
    pipeline: &crate::encode_ahead::Pipeline,
    lights: &buzz_render::document::DrawCache,
) -> buzz_render::document::LightDetail {
    pipeline.light().map_or_else(|| lights.detail(), |l| l.detail)
}

/// The cache's buffer scale, lent or at home. Stage-only.
fn light_scale_of(
    pipeline: &crate::encode_ahead::Pipeline,
    lights: &buzz_render::document::DrawCache,
) -> u32 {
    pipeline.light().map_or_else(|| lights.scale(), |l| l.scale)
}

/// The estimated lines of the last frame judged. Stage-only.
fn light_last_lines_of(
    pipeline: &crate::encode_ahead::Pipeline,
    lights: &buzz_render::document::DrawCache,
) -> u64 {
    pipeline.light().map_or_else(|| lights.last_lines(), |l| l.last_lines)
}

/// Has the cache built no shading yet? Stage-only.
fn light_cache_empty_of(
    pipeline: &crate::encode_ahead::Pipeline,
    lights: &buzz_render::document::DrawCache,
) -> bool {
    pipeline.light().map_or_else(|| lights.lights.is_empty(), |l| l.empty)
}

/// **The encode-ahead decisions, over exactly the fields they read** - borrowed
/// one by one so `App::render` can use them while it holds `self.active`.
struct Ahead<'a> {
    pipeline: &'a mut crate::encode_ahead::Pipeline,
    /// The real cache, or its stand-in while the pipeline has it lent.
    lights: &'a mut buzz_render::document::DrawCache,
    editor: &'a Editor,
    stage_bands: u32,
    test_movie_open: bool,
    stage_stale: bool,
    shade_building: bool,
}

impl Ahead<'_> {
    /// The identity of what an encode ahead is drawn from. `detail` and
    /// `buffer_scale` are the ones the cache was lent with.
    fn epoch(
        stamp: &StageStamp,
        scale: f64,
        buffer_scale: u32,
        detail: buzz_render::document::LightDetail,
    ) -> crate::encode_ahead::Epoch {
        crate::encode_ahead::Epoch {
            revision: stamp.revision,
            camera: stamp.camera,
            area: stamp.area,
            scale_bits: scale.to_bits(),
            detail,
            lights_generation: stamp.lights_generation,
            buffer_scale,
        }
    }

    /// **May the stage be drawn ahead right now?** Only while a plain stage
    /// plays (S5 revised, "Pipeline only while playing a plain stage"): nothing
    /// edited in place, no onion skin, no Edit Multiple Frames, no preview
    /// painted into the scene, no bands, no gesture, no Test Movie, and lights
    /// either inactive or settled (built, not stale, nothing building, not
    /// moving). `BUZZ_NO_ENCODE_AHEAD=1` says never.
    fn can(&self, settled: bool) -> bool {
        if !crate::encode_ahead::enabled()
            || !self.editor.playback.playing
            || self.test_movie_open
            || self.editor.is_gesturing()
            || self.stage_bands != 0
            || !stage::is_plain(self.editor)
        {
            return false;
        }
        if !self.editor.scene().lights().is_active() {
            return true;
        }
        let cold = light_cache_empty_of(self.pipeline, self.lights)
            && light_detail_of(self.pipeline, self.lights)
                == buzz_render::document::LightDetail::Full;
        !cold && !self.stage_stale && !self.shade_building && settled
    }

    /// Take the cache home from the encode thread (a no-op when it is home).
    /// Raises the cache's abort flag first, so the frame being drawn ends
    /// within a layer (E5). The stand-in held meanwhile is dropped.
    fn stop(&mut self) {
        use crate::encode_ahead::Stopped;
        match self.pipeline.stop() {
            Stopped::NotRunning => {}
            Stopped::Returned(cache) => {
                *self.lights = *cache;
            }
            Stopped::Lost => {
                // The thread died with the cache; the stand-in (which knows
                // this window's GPU limits) carries on, cold.
                tracing::warn!("the encode thread was lost; the stage cache starts cold");
            }
        }
    }

    /// **Where this frame's stage encoding comes from** while a pipeline may be
    /// running. Stops it (taking the cache home) when it no longer applies or
    /// its epoch differs from this frame's - an encoding from before an edit,
    /// a camera move or a resize is never shown - and answers `Normal` then.
    fn source(
        &mut self,
        stamp: &StageStamp,
        scale: f64,
        settled: bool,
        interval: Duration,
    ) -> PipeSource {
        use crate::encode_ahead::Taken;
        let Some(lent) = self.pipeline.epoch().copied() else {
            return PipeSource::Normal;
        };
        let now = Self::epoch(stamp, scale, lent.buffer_scale, lent.detail);
        if !self.can(settled) || now != lent {
            self.stop();
            return PipeSource::Normal;
        }
        match self.pipeline.take(self.editor.current_frame, interval) {
            Taken::Ready(prepared) if prepared.over_budget() => {
                // Past what the rasteriser holds even after the ladder: the
                // render thread's own ladder decides (bands), on the real cache.
                tracing::warn!(
                    frame = prepared.frame,
                    lines = prepared.lines(),
                    "encode ahead: frame over budget; back to the render thread"
                );
                self.pipeline.recycle(prepared.scene);
                self.stop();
                PipeSource::Normal
            }
            Taken::Ready(prepared) => PipeSource::Ready(prepared),
            Taken::Shown => PipeSource::Shown,
            Taken::NotReady => {
                tracing::debug!(
                    frame = self.editor.current_frame,
                    "encode ahead: frame not ready within a frame interval"
                );
                PipeSource::NotReady
            }
        }
    }

    /// Lend the cache to the encode thread, to draw the frames after the one
    /// just encoded here. Needs the stage on screen (`stage_stamp`) to be this
    /// frame's, and [`Self::can`].
    fn start(
        &mut self,
        stage_stamp: Option<&StageStamp>,
        area_px: buzz_geom::Rect,
        scale: f64,
        settled: bool,
    ) {
        use crate::encode_ahead::{PlayRange, step_frame};
        if self.pipeline.is_running() || !self.can(settled) {
            return;
        }
        let frame = self.editor.current_frame;
        let Some(stamp) = stage_stamp.filter(|s| s.frame == frame) else {
            return;
        };
        let play = PlayRange {
            count: self.editor.playing_length(),
            looping: self.editor.playback.looping,
        };
        let Some(first) = step_frame(self.editor.scene(), play, frame) else {
            return;
        };
        let inputs = stage::StageInputs::from_editor(self.editor, area_px, scale);
        let epoch = Self::epoch(stamp, scale, inputs.buffer_scale, self.lights.detail());
        let mut stand_in = fresh_light_cache();
        stand_in.set_scale(self.lights.scale());
        stand_in.set_max_scale(self.lights.max_scale());
        let cache = std::mem::replace(self.lights, stand_in);
        self.pipeline.start(cache, epoch, inputs, play, first, frame);
    }
}

/// Should this frame run the whole-frame `stage::build_scene` ladder?
///
/// `false` while band mode is already running (`banding_already`): the band
/// encode is what actually draws a frame like that, and running the ladder
/// too would encode the whole frame a second time for an `active.vello`
/// nothing downstream ever reads. `RENDER_FOLLOWUPS_PLAN.md` item 2 — pulled
/// out, rather than left as the two conditions inline at the one call site,
/// so the decision itself can be pinned without a GPU or a window; see
/// `tests::a_second_banded_frame_skips_the_whole_frame_ladder`.
fn wants_whole_frame_ladder(reuse: bool, banding_already: bool) -> bool {
    !reuse && !banding_already
}

/// **Is the banded picture already in the target, still right for this
/// frame?** `banding` is "already in band mode, ladder not due, retention
/// on"; `provisional` is any reason the retained encoding cannot be trusted
/// (cold cache, stale shading, a preview painted into the scene);
/// `stamp_matches` is the stage stamp equal to the one the bands were drawn
/// for; `on_target` is what they were drawn at (target size, background) and
/// `now` is the same for this frame. See `reuse_banded` in `App::render`.
fn banded_frame_reusable(
    banding: bool,
    provisional: bool,
    stamp_matches: bool,
    on_target: Option<(u32, u32, Color)>,
    now: (u32, u32, Color),
) -> bool {
    banding && !provisional && stamp_matches && on_target == Some(now)
}

/// How many band frames may pass before the whole-frame ladder runs once
/// more to see whether the frame now fits. S3 in `docs/BIMARI5_LAG_PLAN.md`.
const BAND_LADDER_RECHECK_FRAMES: u32 = 30;

/// **A frame that starts in band mode: must the whole-frame ladder run
/// anyway?** S3 in `docs/BIMARI5_LAG_PLAN.md`.
///
/// Yes when the cache's buffer scale can still grow (bands are only right once
/// growing has been exhausted — a bigger buffer costs nothing in quality),
/// every [`BAND_LADDER_RECHECK_FRAMES`] frames, and when the playhead moved
/// by more than one frame since the last band frame (a scrub or a jump may
/// have landed on a lighter frame). The ladder is what decides to leave
/// band mode: it does when `!over_budget()` for the whole frame.
fn band_recheck_due(
    scale: u32,
    max_scale: u32,
    frames_since_ladder: u32,
    frame: u32,
    last_frame: u32,
) -> bool {
    scale < max_scale
        || frames_since_ladder >= BAND_LADDER_RECHECK_FRAMES
        || frame.abs_diff(last_frame) > 1
}

/// The bookkeeping `stage::build_scene` used to do inline, run once the
/// frame's actual geometry is known: right after it draws, on an ordinary
/// frame; once after every band in band mode, since a banded frame's
/// geometry is not known until all of them have drawn — running this per
/// band would be judging each one's own partial view rather than the frame's.
/// See `RENDER_FOLLOWUPS_PLAN.md` item 2. `stamp` and `aim` are the frame's
/// own state, computed once by the caller either way.
#[allow(
    clippy::too_many_arguments,
    reason = "two call sites; a struct would only move the arguments"
)]
fn finish_stage_frame(
    lights: &mut buzz_render::document::DrawCache,
    shade_build: &mut Option<ShadeBuild>,
    jobs: &Arc<JobSystem>,
    window: &Window,
    aim: u64,
    stage_stamp: &mut Option<StageStamp>,
    stage_stale: &mut bool,
    stamp: StageStamp,
) {
    *stage_stamp = Some(stamp);
    // Read *after* the draw: it is the draw that discovers what it could not
    // light.
    *stage_stale = lights.lights.is_stale();

    // Whatever this frame could not light, build in parallel off-thread and
    // ask for another frame to show it in.
    let misses = lights.lights.take_misses();
    // The budget was for the stage frame only; anything else that draws
    // through this cache — the Library thumbnails below — builds inline.
    lights.lights.set_defer(false);

    // **Nothing is built while the light is still moving.**
    //
    // A batch takes longer than the gap between two pointer moves, so one
    // started mid-drag is for a light that will have moved before it lands
    // — that is why it is abandoned at the call site above, and starting
    // another in its place only pins the machine again for another result
    // nobody will see. Meanwhile the shadows are exact on every frame and the
    // crescents hold at their last angle, which is what a drag needs to look
    // like.
    //
    // So the batch waits for the light to be where it was last frame: a
    // pause, or the moment the pointer comes up. It lands a frame or two
    // later and the picture is exact again, and the cores were free for
    // drawing the whole time the hand was moving. `set_queue` at the call
    // site is the other half of it — while the light moves, the misses are
    // not even collected, which is a copy of every visible path saved per
    // frame.
    if !misses.is_empty() && shade_build.is_none() {
        let jobs = Arc::clone(jobs);
        let (send, receive) = crossbeam_channel::bounded(1);
        let abandon = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = Arc::clone(&abandon);
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
        *shade_build = Some(ShadeBuild {
            results: receive,
            abandon,
            aim,
        });
        window.request_redraw();
    }
}

/// One leaf band [`plan_stage_bands`] actually settled on: its own rows in
/// physical pixels, the [`vello::Scene`] already encoded for them, and what
/// that encode cost. `render_stage_bands` reads the cost fields for logging
/// and for the next frame's `n`, and renders `scene` into its own rows;
/// a headless test reads the cost fields alone, and `examples/stage_cost.rs`
/// (H0 item 6) renders `scene` itself, into its own offscreen target. See H0
/// items 2, 5 and 6 in `docs/IMPORT_PERF_PLAN.md`.
pub struct PlannedBand {
    pub y0: f64,
    pub y1: f64,
    pub segments: u32,
    pub lines: u64,
    pub scene: VelloScene,
}

/// **Plan this frame's bands.** Splits each of the `n` top-level strips of a
/// `w`×`h` output — recursively, up to 2 levels (4 leaves per top band) —
/// until every leaf's own estimate fits both budgets, the segment `ceiling`
/// and [`buzz_render::document::line_budget`], rather than rendering an
/// over-budget leaf at all. A leaf that still does not fit at the deepest
/// split is retried once at `LightDetail::Unsealed`, pinned for that leaf
/// alone, and kept either way — a band that cannot be made to fit is still
/// drawn, not left black. See H0 item 2 in `docs/IMPORT_PERF_PLAN.md`.
///
/// Pure CPU planning, no GPU: [`render_stage_bands`] is the only production
/// caller, and a headless test can call this directly with a `DrawCache` and
/// no window.
///
/// One `cache.begin()`/`end()` around every leaf, exactly as
/// `stage::build_scene` opens one around its own ladder — see there for why:
/// it is what lets the `Unsealed` retry share generated geometry with the
/// pass it replaces, rather than ageing it out from under itself.
pub fn plan_stage_bands(
    editor: &Editor,
    cache: &mut buzz_render::document::DrawCache,
    area: buzz_geom::Rect,
    scale: f64,
    ceiling: u32,
    w: u32,
    h: u32,
    n: u32,
) -> Vec<PlannedBand> {
    let held_detail = cache.detail();
    let mut planned = Vec::new();
    cache.begin();
    // The bands of one frame count as a single sighting of each symbol.
    cache.set_band_mode(true);
    for i in 0..n {
        let y0 = f64::from(h) * f64::from(i) / f64::from(n);
        let y1 = f64::from(h) * f64::from(i + 1) / f64::from(n);
        split_stage_band(
            editor,
            cache,
            area,
            scale,
            ceiling,
            w,
            y0,
            y1,
            0,
            held_detail,
            &mut planned,
        );
    }
    cache.set_band_mode(false);
    cache.end();
    planned
}

/// One recursive step of [`plan_stage_bands`]: encode `[y0, y1)` and either
/// keep it, if it fits, or split it in half and recurse — see there for the
/// budgets and the depth limit.
#[allow(
    clippy::too_many_arguments,
    reason = "a private recursive step; a struct would only move the arguments"
)]
fn split_stage_band(
    editor: &Editor,
    cache: &mut buzz_render::document::DrawCache,
    area: buzz_geom::Rect,
    scale: f64,
    ceiling: u32,
    w: u32,
    y0: f64,
    y1: f64,
    depth: u32,
    held_detail: buzz_render::document::LightDetail,
    planned: &mut Vec<PlannedBand>,
) {
    let band_px = buzz_geom::Rect::new(0.0, y0, f64::from(w), y1);
    let mut scene = VelloScene::new();
    let lines = stage::build_band(&mut scene, editor, area, scale, cache, band_px);
    let segments = scene.encoding().n_path_segments;
    // V2 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): this cache's own scale, not
    // the thread-local `vello_encoding` reads.
    let over = segments > ceiling || lines > cache.line_budget();

    // **The band is not rendered at all while it is over budget.** This
    // encode is thrown away and the strip is tried again as two narrower
    // ones, one recursive level deeper — up to 2 levels (4 leaves per top
    // band) — rather than doubling every band on the frame for the sake of
    // the one that overflowed. P1's bigger Vello buffers are meant to make
    // this rare; a leaf still over budget at the cap renders at
    // `LightDetail::Unsealed` and then renders anyway, below.
    if over && depth < 2 {
        let mid = (y0 + y1) / 2.0;
        split_stage_band(
            editor,
            cache,
            area,
            scale,
            ceiling,
            w,
            y0,
            mid,
            depth + 1,
            held_detail,
            planned,
        );
        split_stage_band(
            editor,
            cache,
            area,
            scale,
            ceiling,
            w,
            mid,
            y1,
            depth + 1,
            held_detail,
            planned,
        );
        return;
    }

    if over {
        // The deepest split still does not fit: give up the seam seal for
        // this one leaf alone, pinned so every other band this frame keeps
        // drawing at `held_detail`.
        cache.pin_detail(Some(buzz_render::document::LightDetail::Unsealed));
        let mut retry = VelloScene::new();
        let retry_lines = stage::build_band(&mut retry, editor, area, scale, cache, band_px);
        let retry_segments = retry.encoding().n_path_segments;
        // Restore the held level, then unpin — two calls because
        // `pin_detail(None)` leaves `detail` exactly as it found it, so the
        // first call has to put `held_detail` back before the second stops
        // pinning it. See `DrawCache::pin_detail`.
        cache.pin_detail(Some(held_detail));
        cache.pin_detail(None);
        if retry_segments > ceiling || retry_lines > cache.line_budget() {
            tracing::warn!(
                "stage band [{y0:.0}, {y1:.0}) still over budget after the deepest split \
                 and LightDetail::Unsealed — {retry_segments} segments of {ceiling}, \
                 {retry_lines} lines of {} — drawing it anyway",
                cache.line_budget(),
            );
        }
        planned.push(PlannedBand {
            y0,
            y1,
            segments: retry_segments,
            lines: retry_lines,
            scene: retry,
        });
        return;
    }

    planned.push(PlannedBand {
        y0,
        y1,
        segments,
        lines,
        scene,
    });
}

/// **Draw the stage in horizontal strips**, because the ordinary single-pass
/// ladder in `App::render` could not fit this frame even at
/// `LightDetail::Flat`, the bottom of the automatic ladder. [`plan_stage_bands`]
/// decides the actual leaves; each is rendered into `active`'s scratch
/// texture on its own and copied into its own rows of the real target before
/// the next leaf is built — so no single Vello pass ever has to carry the
/// whole frame's geometry. See Phase 4d in
/// `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md` and H0 in
/// `docs/IMPORT_PERF_PLAN.md`.
///
/// Returns the `stage_bands` count the *next* frame should use: the number of
/// leaves actually drawn this frame, capped at 16 — never doubled, never `0`.
/// Leaving band mode is `App::render`'s call, from the whole-frame ladder
/// (S3 in `docs/BIMARI5_LAG_PLAN.md`). See H0 item 3.
fn render_stage_bands(
    active: &mut Active,
    editor: &Editor,
    cache: &mut buzz_render::document::DrawCache,
    area: buzz_geom::Rect,
    scale: f64,
    background: Color,
    n: u32,
) -> Result<u32> {
    active.ensure_scratch();
    let (w, h) = (active.surface_config.width, active.surface_config.height);
    // V2 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): this cache's own scale, not
    // the thread-local `vello_encoding` reads — see `DrawCache::segment_ceiling`.
    let ceiling = cache.segment_ceiling(f64::from(w) * f64::from(h));

    let started = Instant::now();
    let planned = plan_stage_bands(editor, cache, area, scale, ceiling, w, h, n);

    let mut largest_segments = 0u32;
    let mut largest_lines = 0u64;
    for band in &planned {
        largest_segments = largest_segments.max(band.segments);
        largest_lines = largest_lines.max(band.lines);

        let scratch = active.scratch.as_ref().expect("ensure_scratch ran");
        active
            .gpu
            .render(&band.scene, &scratch.view, w, h, background)
            .context("rendering a stage band")?;

        // The band's own rows, copied out of the scratch texture and into
        // the real target — its own encoder and submission, so the next
        // band's render does not race this copy.
        let (y0px, y1px) = (band.y0.round() as u32, band.y1.round() as u32);
        if y1px > y0px {
            let target = active.target.as_ref().expect("ensure_target ran");
            let mut encoder =
                active
                    .gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("stage-band-copy"),
                    });
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &scratch.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: y0px, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &target.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: y0px, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: w,
                    height: y1px - y0px,
                    depth_or_array_layers: 1,
                },
            );
            active.gpu.queue.submit([encoder.finish()]);
        }
    }

    // H0 item 4: one line per frame under `RUST_LOG=buzz_app=debug`, so a
    // frame that is slow or blank is diagnosable from a log instead of a
    // guess.
    tracing::debug!(
        "stage bands: {} leaves in {:.2} ms (largest {largest_segments} segments, {largest_lines} lines)",
        planned.len(),
        started.elapsed().as_secs_f64() * 1e3,
    );

    // Hold the number of leaves actually drawn this frame, capped at 16 —
    // never doubled. See H0 item 3. **Never leaves band mode itself** (S3 in
    // `docs/BIMARI5_LAG_PLAN.md`): "the largest band is small" says nothing
    // about whether the whole frame fits, and bands barely reduce per-band
    // work on artwork that spans them. `App::render` re-runs the whole-frame
    // ladder on its own schedule (`band_recheck_due`) and exits band mode only
    // when that says `!over_budget()`.
    Ok((planned.len() as u32).clamp(1, 16))
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
    if let Some(shortcut) = Command::ToggleActionsPanel.shortcut()
        && ctx.input_mut(|i| i.consume_shortcut(&shortcut))
    {
        out.push(Command::ToggleActionsPanel);
    }

    // `RunScript` and `TestMovie` share Ctrl+Enter — which one fires depends
    // on whether the Actions code editor has focus, decided here because the
    // shortcut map has no way to say "unless this text field has focus".
    // Consumed once regardless of which command that turns out to be.
    if let Some(shortcut) = Command::RunScript.shortcut()
        && ctx.input_mut(|i| i.consume_shortcut(&shortcut))
    {
        let editor_focused = ctx.memory(|m| m.has_focus(egui::Id::new(buzz_ui::actions_panel::CODE_EDITOR_ID)));
        out.push(if editor_focused {
            Command::RunScript
        } else {
            Command::TestMovie
        });
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
    // The bracket keys re-weight the selected outlines, which is a thing done
    // repeatedly and by eye and so wants a key rather than a menu.
    Command::ThickenStroke,
    Command::ThinStroke,
    Command::New,
    Command::Open,
    Command::Save,
    Command::SaveAs,
    // Dispatched by the shell, which owns the file dialogs — but raised
    // from here, like everything else with a key.
    Command::Close,
    Command::Quit,
    Command::VoicePushToTalk,
    Command::VoiceMute,
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
    Command::BreakApart,
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
    Command::MoveLayerUp,
    Command::MoveLayerDown,
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
    Command::ToggleBoneRig,
    Command::ToggleBoneMode,
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
    Command::CreateClassicTween,
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
        // The user's keymap override wins over the built-in shortcut; an
        // unbound command resolves to `None` and matches no key.
        if let Some(shortcut) = editor.workspace.shortcut_for(command)
            && ctx.input_mut(|i| i.consume_shortcut(&shortcut))
        {
            out.push(command);
        }
    }

    // Animate also accepts Ctrl+Y / Cmd+Y for redo.
    let redo_shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);
    if ctx.input_mut(|i| i.consume_shortcut(&redo_shortcut)) {
        out.push(Command::Redo);
    }
    #[cfg(target_os = "macos")]
    {
        let ctrl_y = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Y);
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl_y)) {
            out.push(Command::Redo);
        }
    }

    // **The arrow keys nudge the selection**, one document unit at a time and
    // eight with Shift — Animate's numbers.
    //
    // Read here rather than through the shortcut map because four directions
    // times two step sizes is eight bindings for one action, and none of them
    // belongs in a menu. Only when something is selected: with an empty
    // selection the arrows are free for whatever wants them next, and nudging
    // nothing would be a silent no-op that looks like a dropped keystroke.
    //
    // **Selected anchors are nudged instead of the object**, by one unit or by
    // ten with Shift — Animate's Subselection numbers, which differ from the
    // object nudge's. Backspace deletes them as Delete does.
    let anchors = editor.anchors_selected();
    if anchors && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
        out.push(Command::Delete);
    }
    if !editor.selection.is_empty() {
        let shift = ctx.input(|i| i.modifiers.shift_only());
        let step = if shift && anchors {
            10
        } else if shift {
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

/// One `VectorPartCandidate` per non-empty layer of `symbol`, covering
/// *every* object drawn on that layer at frame 0 — not just the first, so a
/// layer built from several shapes rigs as a whole instead of losing every
/// piece but one. More than one object is wrapped in a single group (a fresh
/// id from `scene`, the snapshot the caller owns); an empty layer is skipped
/// outright rather than standing in with a dummy group the rig would have
/// nothing to attach to.
fn rig_parts_for_symbol(
    scene: &mut buzz_scene::Scene,
    symbol: &buzz_scene::Symbol,
) -> Vec<buzz_ai::rerig::VectorPartCandidate> {
    let mut parts = Vec::new();
    for layer in symbol.layers.iter() {
        let objects = layer.objects_at(0);
        if objects.is_empty() {
            continue;
        }
        let object: std::sync::Arc<buzz_scene::Object> = if objects.len() == 1 {
            objects[0].clone()
        } else {
            let id = scene.next_object_id();
            std::sync::Arc::new(buzz_scene::Object::group(id, objects.iter().cloned().collect()))
        };
        let bounds = layer.bounds().unwrap_or_else(|| object.bounds());
        parts.push(buzz_ai::rerig::VectorPartCandidate {
            name: layer.name.clone(),
            bounds,
            mask: None,
            object,
        });
    }
    parts
}

/// Where a joint the pose detector found in `render_symbol_alone`'s output
/// lands in the symbol's own coordinate space — the inverse of that render's
/// crop and placement, so a detected keypoint can be handed straight to
/// `buzz_ai::character::skeleton` and mean the same thing the symbol's own
/// parts (and `rig_parts_for_symbol`'s bounds) do.
struct SymbolRenderMap {
    region_center: Point,
    cam_zoom: f64,
    out_width: f64,
    out_height: f64,
    placed_at: Point,
}

impl SymbolRenderMap {
    fn to_symbol_local(&self, px: f32, py: f32) -> (f32, f32) {
        let world_x = (px as f64 - self.out_width * 0.5) / self.cam_zoom + self.region_center.x;
        let world_y = (py as f64 - self.out_height * 0.5) / self.cam_zoom + self.region_center.y;
        ((world_x - self.placed_at.x) as f32, (world_y - self.placed_at.y) as f32)
    }
}

/// Renders `symbol` alone — everything else off the stage, the symbol placed
/// at the stage centre on a probe layer of its own — cropped to its own
/// bounds (plus a small margin) and scaled to at least 1024px tall,
/// transparent background: the same `--crop1024` framing
/// `examples/render_symbol.rs` uses, which is also where the pixel↔document
/// mapping below comes from. `scene` is a disposable snapshot: every stage
/// layer it has is stripped.
fn render_symbol_alone(
    scene: &mut buzz_scene::Scene,
    symbol: &buzz_scene::Symbol,
) -> anyhow::Result<(image::RgbaImage, SymbolRenderMap)> {
    let ids: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
    for id in ids {
        scene.edit_layers().remove(id);
    }
    scene.camera_mut().enabled = false;
    let probe = scene.add_layer("__ai_rig_probe", buzz_scene::LayerKind::Normal);
    let centre = scene.stage().stage_rect().center();
    scene
        .add_instance_at(probe, 0, symbol.id, buzz_geom::Affine::translate(centre.to_vec2()))
        .ok_or_else(|| anyhow::anyhow!("could not place '{}' to render it", symbol.name))?;

    let local_bounds = symbol
        .bounds()
        .ok_or_else(|| anyhow::anyhow!("'{}' has no visible artwork", symbol.name))?;
    let world = buzz_geom::Rect::new(
        local_bounds.x0 + centre.x,
        local_bounds.y0 + centre.y,
        local_bounds.x1 + centre.x,
        local_bounds.y1 + centre.y,
    );
    let pad_x = world.width() * 0.04;
    let pad_y = world.height() * 0.04;
    let region = buzz_geom::Rect::new(
        world.x0 - pad_x,
        world.y0 - pad_y,
        world.x1 + pad_x,
        world.y1 + pad_y,
    );
    let out_h = 1024u32;
    let out_w = ((out_h as f64) * region.width() / region.height().max(1e-6))
        .round()
        .max(1.0) as u32;

    let settings = buzz_export::ExportSettings {
        width: out_w,
        height: out_h,
        transparent: true,
        region: Some(region),
    };

    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    let frame = exporter.render(scene, 0, &settings)?;
    let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        .ok_or_else(|| anyhow::anyhow!("the rendered buffer did not match its own dimensions"))?;

    let map = SymbolRenderMap {
        region_center: region.center(),
        cam_zoom: settings.width as f64 / region.width().max(1e-6),
        out_width: settings.width as f64,
        out_height: settings.height as f64,
        placed_at: centre,
    };
    Ok((image, map))
}

/// The background half of AI ▸ Auto-Rig Vector Character: render `sym_id`
/// alone, run the same DWPose call the Character-from-Image dialog uses
/// (`buzz_ai::pose::detect`) to detect a real skeleton in that render, map
/// the joints back to the symbol's own coordinate space, and rig it.
///
/// Fails outright — no silent fallback to a guessed proportional skeleton —
/// when the pose models are not installed or no figure is found.
fn rig_vector_character_from_detected_skeleton(
    mut scene: buzz_scene::Scene,
    sym_id: buzz_scene::SymbolId,
    sym_name: &str,
) -> Result<buzz_ai::rerig::ReriggedCharacter, String> {
    if !buzz_ai::models::status(buzz_ai::models::ModelId::DwPoseEstimator).is_ready() {
        return Err("Install pose models in AI \u{25b8} AI Models\u{2026} before auto-rigging.".into());
    }

    let symbol = scene
        .library()
        .get(sym_id)
        .cloned()
        .ok_or_else(|| "That symbol is no longer in the library".to_string())?;

    let parts = rig_parts_for_symbol(&mut scene, &symbol);
    if parts.is_empty() {
        return Err("No vector artwork found in the selected character symbol".into());
    }

    let (image, map) = render_symbol_alone(&mut scene, &symbol)
        .map_err(|e| format!("Could not render '{sym_name}' for pose detection: {e}"))?;

    let persons =
        buzz_ai::pose::detect(&image).map_err(|e| format!("Pose detection failed: {e}"))?;
    let Some(mut person) = persons.into_iter().next() else {
        return Err(format!(
            "No figure detected in '{sym_name}'. Try posing it more like a standing figure, or rig it by hand."
        ));
    };

    for kp in person.keypoints.iter_mut() {
        let (x, y) = map.to_symbol_local(kp.0, kp.1);
        kp.0 = x;
        kp.1 = y;
    }
    let (bx0, by0) = map.to_symbol_local(person.bbox[0], person.bbox[1]);
    let (bx1, by1) = map.to_symbol_local(person.bbox[2], person.bbox[3]);
    person.bbox = [bx0, by0, bx1, by1];

    let skel = buzz_ai::character::skeleton(&person);

    let limits = crate::rig_limits::load_or_default();
    buzz_ai::rerig::rerig_vector_character(&skel, &parts, sym_name, Some(&limits))
        .map_err(|e| format!("Auto-rigging failed: {e}"))
}

/// SigLIP embeddings for AI ▸ Scene from Image's asset matching: each
/// detected object's own crop of `reference` (the same normalised `bbox`
/// `build_scene_from_layout` places by), and a render of each library
/// candidate — the same thumbnail AI ▸ Identify & Rename shows, reused from
/// `buzz_ui::ai_rename_dialog` — both L2-normalised so `compute_asset_match_score`
/// can take a cosine of them directly.
///
/// Returns `None` for one image rather than failing the whole call when a
/// single crop or render could not be embedded; only errors when SigLIP
/// itself could not run at all (e.g. no GPU for the candidate renders).
fn siglip_embeddings_for_scene_from_image(
    reference: &image::RgbaImage,
    layout: &buzz_ai::scene_builder::DetectedSceneLayout,
    scene: &buzz_scene::Scene,
    candidates: &[buzz_ai::scene_builder::LibraryAssetCandidate],
) -> anyhow::Result<SiglipInputs> {
    let crops: Vec<image::RgbaImage> = layout
        .objects
        .iter()
        .map(|obj| buzz_ai::scene_builder::crop_for_matching(reference, obj))
        .collect();

    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    let mut thumbnails = Vec::with_capacity(candidates.len());
    for cand in candidates {
        let png = match scene.library().get(cand.id) {
            Some(sym) => buzz_ui::ai_rename_dialog::render_symbol_thumbnail(&mut exporter, scene, sym),
            None => Vec::new(),
        };
        let thumb = image::load_from_memory(&png)
            .map(|d| d.to_rgba8())
            .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
        thumbnails.push(thumb);
    }

    let mut detected: Vec<Option<Vec<f32>>> = match buzz_ai::siglip::embed(&crops) {
        Ok(v) => v.into_iter().map(Some).collect(),
        Err(_) => vec![None; crops.len()],
    };
    let mut candidate_embeddings: Vec<Option<Vec<f32>>> = match buzz_ai::siglip::embed(&thumbnails) {
        Ok(v) => v.into_iter().map(Some).collect(),
        Err(_) => vec![None; thumbnails.len()],
    };
    // A blank picture matches every other blank picture at cosine 1.0: a
    // crop a few pixels across, or a symbol whose thumbnail drew nothing,
    // takes no part in visual matching (F7.10 run: three objects "matched"
    // an empty symbol at 1.00).
    for (e, img) in detected.iter_mut().zip(&crops) {
        if buzz_ai::scene_builder::is_blank(img) {
            *e = None;
        }
    }
    for (e, img) in candidate_embeddings.iter_mut().zip(&thumbnails) {
        if buzz_ai::scene_builder::is_blank(img) {
            *e = None;
        }
    }

    Ok(SiglipInputs { detected, candidate_embeddings, crops, thumbnails })
}

/// The standard poses of a rigged character, each rendered alone (F7.0's
/// pose sheet, and the report's evidence).
pub(crate) fn render_pose_sheet(
    scene: &buzz_scene::Scene,
    armature: &buzz_scene::Object,
) -> anyhow::Result<Vec<(String, image::RgbaImage)>> {
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?;
    let mut out = Vec::new();
    for pose in POSE_SHEET_POSES {
        let mut s = scene.clone();
        s.edit_document();
        s.camera_mut().enabled = false;
        for id in s.layers().iter().map(|l| l.id).collect::<Vec<_>>() {
            s.remove_layer(id);
        }
        let layer = s.add_layer("Pose", buzz_scene::LayerKind::Normal);
        let mut o = armature.clone();
        if pose != "rest"
            && let buzz_scene::ObjectKind::Armature(rig) = &mut o.kind
        {
            buzz_ai::motion::PoseLibrary::standard().apply_to_armature(pose, rig);
        }
        let b = s.resolved_bounds(&o);
        s.add_object(layer, o);
        out.push((pose.to_string(), render_pose_cell(&mut exporter, &s, 0, b)?));
    }
    Ok(out)
}

/// The poses on the pose sheet, in order — the same for a bone rig and a
/// layer rig.
const POSE_SHEET_POSES: [&str; 6] = ["rest", "walk", "run", "sit", "point", "wave"];

/// One cell of the pose sheet: `bounds` (with a margin) of `scene` at `frame`,
/// 320 px tall, on transparent.
fn render_pose_cell(
    exporter: &mut buzz_export::Exporter,
    scene: &buzz_scene::Scene,
    frame: u32,
    bounds: buzz_geom::Rect,
) -> anyhow::Result<image::RgbaImage> {
    let b = bounds.inflate(bounds.width() * 0.06, bounds.height() * 0.06);
    let h = 320u32;
    let w = ((h as f64) * b.width() / b.height().max(1.0)).round().clamp(1.0, 2000.0) as u32;
    let settings = buzz_export::ExportSettings { width: w, height: h, transparent: true, region: Some(b) };
    let rendered = exporter.render(scene, frame, &settings)?;
    image::RgbaImage::from_raw(rendered.width, rendered.height, rendered.pixels)
        .ok_or_else(|| anyhow::anyhow!("render size mismatch"))
}

/// What the pose sheet poses: a bone rig (its armature object), or a layer
/// rig (LR2b — layer rigs are the default since LR2).
pub(crate) enum PoseSheetSubject {
    Armature(buzz_scene::Object),
    Layers(buzz_act::layer_rig::LayerRigView),
}

/// The character the pose sheet is for: the topmost rig at `frame`, found
/// the way every rig is found ([`buzz_act::layer_rig::rig_of`]).
pub(crate) fn pose_sheet_subject(scene: &buzz_scene::Scene, frame: u32) -> Option<PoseSheetSubject> {
    let layers: Vec<_> = scene.layers().iter().collect();
    for layer in layers.into_iter().rev() {
        for o in layer.objects_at(frame).iter().rev() {
            match buzz_act::layer_rig::rig_of(scene, o.id) {
                Some(buzz_act::layer_rig::CharacterRig::Armature(_)) => {
                    return Some(PoseSheetSubject::Armature((**o).clone()));
                }
                Some(buzz_act::layer_rig::CharacterRig::Layers(view)) => {
                    return Some(PoseSheetSubject::Layers(view));
                }
                None => {}
            }
        }
    }
    None
}

/// The pose sheet of either kind of rig.
pub(crate) fn render_pose_sheet_of(
    scene: &buzz_scene::Scene,
    subject: &PoseSheetSubject,
    frame: u32,
) -> anyhow::Result<Vec<(String, image::RgbaImage)>> {
    match subject {
        PoseSheetSubject::Armature(armature) => render_pose_sheet(scene, armature),
        PoseSheetSubject::Layers(view) => render_layer_rig_pose_sheet(scene, view, frame),
    }
}

/// Pose a layer rig, in `scene`, at `frame`, in the standard library's
/// `pose`: the library's pose on the armature built from the layers' rest
/// geometry ([`buzz_act::layer_rig::rest_armature`]), and every part layer
/// given the transform that puts its part where that pose's bone puts it —
/// what the inherited transform already does taken off, followed layers
/// first, as a performance keys a layer rig. `false` for a pose the library
/// does not know, or a rig with no rest.
fn pose_layer_rig(
    scene: &mut buzz_scene::Scene,
    view: &buzz_act::layer_rig::LayerRigView,
    frame: u32,
    pose: &str,
) -> bool {
    buzz_act::layer_rig::record_rests(scene, view);
    let Some((at_rest, parts)) = buzz_act::layer_rig::rest_armature(scene, view) else {
        return false;
    };
    let placed = buzz_act::layer_rig::placement(scene, view.body).unwrap_or(buzz_geom::Affine::IDENTITY);
    let mut data = buzz_scene::ArmatureData::new(at_rest);
    if !buzz_ai::motion::PoseLibrary::standard().apply_to_armature(pose, &mut data) {
        return false;
    }
    let posed = &data.armature;
    let turns: Vec<buzz_geom::Affine> = (0..posed.len()).map(|i| posed.pose_transform(i)).collect();
    for part in &parts {
        let Some(turn) = turns.get(part.slot) else {
            continue;
        };
        let world = placed * *turn * part.rest;
        scene.ensure_keyframe(part.layer, frame);
        let inherited = scene.layers().inherited_transform(part.layer, frame);
        let c = inherited.as_coeffs();
        let own = if (c[0] * c[3] - c[1] * c[2]).abs() > 1e-12 {
            inherited.inverse() * world
        } else {
            world
        };
        scene.update_object_at(frame, part.object, |target| target.transform = own);
    }
    true
}

/// [`render_pose_sheet`] for a **layer rig**: the same poses, each rendered
/// alone — every layer but the rig's (and the folders holding them) removed.
pub(crate) fn render_layer_rig_pose_sheet(
    scene: &buzz_scene::Scene,
    view: &buzz_act::layer_rig::LayerRigView,
    frame: u32,
) -> anyhow::Result<Vec<(String, image::RgbaImage)>> {
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?;
    let rig: Vec<buzz_scene::LayerId> = view.layers().collect();
    let mut out = Vec::new();
    for pose in POSE_SHEET_POSES {
        let mut s = scene.clone();
        s.edit_document();
        s.camera_mut().enabled = false;
        let mut keep = rig.clone();
        for &layer in &rig {
            let mut up = s.layers().get(layer).and_then(|l| l.parent);
            while let Some(p) = up {
                if keep.contains(&p) {
                    break;
                }
                keep.push(p);
                up = s.layers().get(p).and_then(|l| l.parent);
            }
        }
        for id in s.layers().iter().map(|l| l.id).collect::<Vec<_>>() {
            if !keep.contains(&id) {
                s.remove_layer(id);
            }
        }
        if pose != "rest" {
            pose_layer_rig(&mut s, view, frame, pose);
        }
        let mut bounds: Option<buzz_geom::Rect> = None;
        for &layer in &rig {
            let inherited = s.layers().inherited_transform(layer, frame);
            let Some(l) = s.layers().get(layer) else {
                continue;
            };
            for o in l.objects_at(frame) {
                let mut placed = (**o).clone();
                placed.transform = inherited * placed.transform;
                let b = s.resolved_bounds(&placed);
                bounds = Some(bounds.map_or(b, |u| u.union(b)));
            }
        }
        let b = bounds.ok_or_else(|| anyhow::anyhow!("the layer rig has nothing drawn"))?;
        out.push((pose.to_string(), render_pose_cell(&mut exporter, &s, frame, b)?));
    }
    Ok(out)
}

/// What the AI panel shows for a parsed story before Apply.
fn story_review(script: &buzz_ai::director::DirectorScript, _beats: usize) -> buzz_ui::ai_panel::StoryReview {
    buzz_ui::ai_panel::StoryReview {
        beats: script
            .beats
            .iter()
            .map(|b| {
                let mut line = format!("{} {}", b.actor, b.verb.as_str());
                if let Some(t) = &b.target {
                    line.push_str(&format!(" \u{2192} {t}"));
                }
                line
            })
            .collect(),
        cues: script
            .beats
            .iter()
            .filter_map(|b| b.dialogue.as_ref().map(|d| format!("{}: {d}", b.actor)))
            .collect(),
        cast: script.cast.clone(),
        secs: 0.0,
        model: String::new(),
    }
}

/// What Scene from Image's SigLIP step produced: the embeddings, and the
/// crops and thumbnails they came from (the vision model's confirmation
/// sheet shows the same pictures).
struct SiglipInputs {
    detected: Vec<Option<Vec<f32>>>,
    candidate_embeddings: Vec<Option<Vec<f32>>>,
    crops: Vec<image::RgbaImage>,
    thumbnails: Vec<image::RgbaImage>,
}

impl App {
    /// Lay out the panels and return the rect left for the stage.
    ///
    /// Panels that change the document run inside [`Document::edit`], so their
    /// changes are undoable. `edit` only records a step when the revision
    /// actually moves, so a frame where the user touched nothing costs nothing.
    ///
    /// `pub(crate)` rather than private so the offscreen screenshot harness in
    /// `ui_snapshot.rs` can drive the identical panel layout the real window
    /// draws, the same way the headless `dock_drag_tests::Drive` harness below
    /// already does from within this module.
    pub(crate) fn build_ui(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let mut commands: Vec<Command> = Vec::new();
        self.editor.forget_bone_if_deselected();
        let can_undo = self.editor.doc.can_undo();
        let can_redo = self.editor.doc.can_redo();
        let ctx = ui.ctx().clone();
        // The whole window, read before any panel narrows `ui`'s own rect —
        // a dock column's *reported* rect can run past this when its content
        // is wider than the column it was drawn in (`egui` trims a panel's
        // rect from the wrong edge once that happens; see
        // `dock_geometry_tests::the_stage_never_reaches_into_a_dock_column`).
        // `dock_rects` is read for hit-testing and for the splitter handles,
        // so what is stored there is clamped back to the window rather than
        // trusting the raw response.
        let window_rect = ui.max_rect();

        // Stage 6, items 1 and 5 of `docs/WORKSPACE_DESIGN_PLAN.md`: two
        // preferences that act on the `Context` itself rather than on
        // anything drawn, applied every frame so a workspace loaded with
        // either already set — not only a menu click in this same run —
        // takes effect.
        let wanted_zoom = self.editor.workspace.ui_scale;
        if (ctx.zoom_factor() - wanted_zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(wanted_zoom);
        }
        // egui's own stock pace, restored when Reduce Motion is off rather
        // than a value guessed at here — see `theme::apply`'s own
        // `all_styles_mut` for why both style slots, not just the active one.
        let stock_animation_time = egui::Style::default().animation_time;
        let wanted_animation_time = if self.editor.workspace.reduce_motion {
            0.0
        } else {
            stock_animation_time
        };
        ctx.all_styles_mut(|style| style.animation_time = wanted_animation_time);

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
        // The Sequence and Shot menus' own state, gathered the same way and
        // for the same reason as `template_names`.
        let sequence_menu = self.editor.sequence_menu_info();
        // Before the panels ask for pictures, throw away the ones whose
        // symbols have been edited since they were drawn.
        self.thumbnails.invalidate_stale(&scene);
        // Taken before the panels are built, so the code editor never sees the
        // keystroke that ran it.
        commands.extend(script_shortcuts(&ctx));

        let stage_area = {
            // egui 0.35 unified the panel types into `Panel::top/bottom/left/right`.
            egui::Panel::top("menu")
                .frame(egui::Frame::new().fill(Palette::bg()).inner_margin(2))
                .show(ui, |ui| {
                    self.title_bar_row(
                        ui,
                        &mut commands,
                        &template_names,
                        &sequence_menu,
                        can_undo,
                        can_redo,
                    );
                });

            // Every panel is placed by the workspace rather than nailed to a
            // side here, which is what makes the layout the user's to arrange.
            // Requested moves are collected and applied after the frame: a
            // panel cannot be moved while it is being drawn.
            let mut requests = DockRequests::default();
            let workspace = self.editor.workspace.clone();
            let locked = workspace.locked;
            self.dock_rects.clear();
            self.drop_zones.clear();

            // Every visible section's front panel, on every dock — built once
            // and handed to every `section_header` call, so "Group With" can
            // offer a panel on another side rather than only its own.
            let all_fronts: Vec<buzz_ui::PanelId> = {
                let mut docks = buzz_ui::Dock::COLUMNS.to_vec();
                docks.push(buzz_ui::Dock::Float);
                docks
                    .into_iter()
                    .flat_map(|d| workspace.sections(d))
                    .map(|s| s.front)
                    .collect()
            };

            // **The highlight fades.** A revealed panel's header is drawn in
            // the accent for a couple of seconds and then left alone; the
            // clock is read once here so every section agrees.
            self.highlight_frame = None;
            if let Some(panel) = self.editor.workspace.highlight {
                let now = ui.input(|i| i.time);
                let since = match self.highlight_since {
                    Some((p, t)) if p == panel => t,
                    _ => {
                        self.highlight_since = Some((panel, now));
                        now
                    }
                };
                let strength = 1.0 - ((now - since) / HIGHLIGHT_SECONDS) as f32;
                if strength <= 0.0 {
                    self.editor.workspace.highlight = None;
                    self.highlight_since = None;
                } else {
                    self.highlight_frame = Some((panel, strength));
                    ui.ctx().request_repaint();
                }
            }

            // Bottom first: `egui` gives each side to whichever panel asks
            // first, so the order here is the order down the window.
            for section in workspace.sections(buzz_ui::Dock::Bottom) {
                let height = if section.front == buzz_ui::PanelId::Timeline {
                    workspace.bottom_height
                } else {
                    240.0
                };
                let emphasis = self.emphasis_for(&section);
                let response = egui::Panel::bottom(egui::Id::new(("dock-bottom", section.group)))
                    // **Exact, not default.** egui keeps a size of its own per
                    // panel, and a `default_size` is only consulted when it has
                    // none — so the workspace's number and egui's could
                    // disagree, and the panel would spring back to whichever
                    // one won. The workspace is the single source of truth, and
                    // the splitters below are what move it.
                    .resizable(false)
                    .exact_size(height)
                    .show(ui, |panel| {
                        // **Clipped to the panel, never overflowing it.** The
                        // timeline needs ~160 points and the splitter allows
                        // 80; egui answers a bottom panel whose contents
                        // outgrow its exact size by keeping the *bottom* slice
                        // of the overflow — which lies below the window. The
                        // timeline then drew nothing, its top edge (and so the
                        // splitter's handle) went off-screen with it, and there
                        // was no way to drag it back. Drawn into a child that
                        // reports exactly the panel's rect instead, what does
                        // not fit is cut off and the edge stays put.
                        let area = panel.max_rect();
                        let mut content = panel.new_child(
                            egui::UiBuilder::new().max_rect(area).layout(*panel.layout()),
                        );
                        content.shrink_clip_rect(area);
                        let ui = &mut content;
                        self.timeline_tools_slot = section_header(
                            ui,
                            &section,
                            &workspace,
                            &all_fronts,
                            locked,
                            !section.front.draws_own_title(),
                            false,
                            emphasis,
                            &mut requests,
                        );
                        egui::Frame::new()
                            .inner_margin(buzz_ui::theme::Tokens::SPACE_2 as i8)
                            .show(ui, |ui| self.draw_panel(ui, section.front, &mut commands));
                        panel.advance_cursor_after_rect(area);
                    });
                if section.panels.contains(&buzz_ui::PanelId::Timeline) {
                    self.dock_rects.push((
                        buzz_ui::Dock::Bottom,
                        response.response.rect.intersect(window_rect),
                    ));
                }
                self.drop_zones.push(DropZone {
                    dock: buzz_ui::Dock::Bottom,
                    group: section.group,
                    front: section.front,
                    rect: response.response.rect,
                });
            }

            // **How wide the columns may actually be drawn.**
            //
            // `egui` hands each side to whichever panel asks first and clamps
            // it to whatever is left, so three columns that together want more
            // than the window has end up placed over one another — which is
            // what "the left edge drags over the property panel" is. Fitting
            // them to the window first means a column that has to give up
            // space shrinks instead, and gets it back when there is room.
            let room = ui.available_rect_before_wrap().width();
            // The boundary under the pointer, if one is being dragged: that
            // column keeps what it was dragged to and the others give way, so
            // the edge follows the pointer instead of creeping away from it.
            let dragging = [
                (buzz_ui::Dock::Left, "split-left"),
                (buzz_ui::Dock::Right, "split-right"),
                (buzz_ui::Dock::RightOuter, "split-right-outer"),
            ]
            .into_iter()
            .find(|(_, id)| ui.ctx().is_being_dragged(egui::Id::new(*id)))
            .map(|(dock, _)| dock);
            let columns = workspace.column_widths_dragging(room, dragging);
            self.dock_room = room;

            for (dock, id_name, width) in [(buzz_ui::Dock::Left, "dock-left", columns.left)] {
                let sections = workspace.sections(dock);
                if sections.is_empty() {
                    continue;
                }
                let response = egui::Panel::left(id_name)
                    .resizable(false)
                    .exact_size(width)
                    .show(ui, |ui| {
                        self.draw_column(
                            ui,
                            dock,
                            &sections,
                            &workspace,
                            &all_fronts,
                            locked,
                            &mut requests,
                            &mut commands,
                        );
                    });
                self.dock_rects
                    .push((dock, response.response.rect.intersect(window_rect)));
            }

            for (dock, id_name, width) in [
                (
                    buzz_ui::Dock::RightOuter,
                    "dock-right-outer",
                    columns.right_outer,
                ),
                (buzz_ui::Dock::Right, "dock-right", columns.right),
            ] {
                let sections = workspace.sections(dock);
                if sections.is_empty() {
                    continue;
                }
                let response = egui::Panel::right(id_name)
                    .resizable(false)
                    .exact_size(width)
                    .show(ui, |ui| {
                        self.draw_column(
                            ui,
                            dock,
                            &sections,
                            &workspace,
                            &all_fronts,
                            locked,
                            &mut requests,
                            &mut commands,
                        );
                    });
                self.dock_rects
                    .push((dock, response.response.rect.intersect(window_rect)));
            }

            // Floating sections are windows over the stage, movable and
            // resizable unless the layout is locked. A grouped section floats
            // as **one** window with its tabs, which is what "undock these two
            // together" has to mean if grouping is to survive being undocked.
            let floating = workspace.sections(buzz_ui::Dock::Float);
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
                let emphasis = self.emphasis_for(&section);
                let mut window = egui::Window::new(title)
                    .id(egui::Id::new(("float", section.group)))
                    .open(&mut open)
                    .movable(!locked)
                    .resizable(!locked)
                    .default_pos(slot.map(|s| s.float_pos).unwrap_or((320.0, 140.0)))
                    .default_size(slot.map(|s| s.float_size).unwrap_or((300.0, 380.0)));
                // Dropped somewhere: put the window there, once. egui keeps
                // its own idea of where a window is, and `default_pos` is
                // only consulted when it has none.
                if let Some(at) = self
                    .place_floats
                    .iter()
                    .position(|(group, _)| *group == section.group)
                {
                    let (_, pos) = self.place_floats.remove(at);
                    window = window.current_pos(pos);
                }
                let response = window.show(ui.ctx(), |ui| {
                    // A lone floating panel is not named here: the window
                    // frame already carries the title, and the panel below
                    // may carry it again. A tabbed one still needs its tabs.
                    self.timeline_tools_slot = section_header(
                        ui,
                        &section,
                        &workspace,
                        &all_fronts,
                        locked,
                        false,
                        false,
                        emphasis,
                        &mut requests,
                    );
                    egui::ScrollArea::vertical()
                        .id_salt(("float-scroll", section.group))
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .inner_margin(buzz_ui::theme::Tokens::SPACE_2 as i8)
                                .show(ui, |ui| self.draw_panel(ui, section.front, &mut commands));
                        });
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
                    // A window is somewhere to drop a panel too: it becomes a
                    // tab of that window.
                    self.drop_zones.push(DropZone {
                        dock: buzz_ui::Dock::Float,
                        group: section.group,
                        front: section.front,
                        rect,
                    });
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

            // **Drag-and-drop docking.** A panel picked up by its grip or its
            // tab is carried until the pointer is released, and where it lands
            // decides what happens: on a section, it becomes a tab there; in
            // the gap between two sections of a column, it docks there in a
            // section of its own; anywhere else, it floats at that point.
            // The target is outlined while the drag is in progress, so the
            // drop is never a guess.
            self.drag_docking(ui.ctx(), &mut requests);

            let floated: Vec<(buzz_ui::PanelId, (f32, f32))> = requests.floats.clone();
            requests.apply(&mut self.editor.workspace);
            for (id, (x, y)) in floated {
                if let Some(slot) = self.editor.workspace.slot(id) {
                    self.place_floats.push((slot.group, egui::pos2(x, y)));
                }
            }

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
                    .frame(egui::Frame::new().fill(Palette::bg()).inner_margin(3))
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
                    // Nor while the Assistant is running: a hand edit would be
                    // folded into its single `AI:` undo step.
                    if self.doc_available() && !self.agent_active() {
                        self.handle_stage_input(ui, area);
                    }
                    let response = stage::draw_chrome(ui, &self.editor, area);
                    if self.agent_active() {
                        let text = "Assistant is working \u{2014} press Stop to take over";
                        let font = egui::FontId::proportional(14.0);
                        let galley = ui.painter().layout_no_wrap(text.to_string(), font, egui::Color32::WHITE);
                        let rect = egui::Align2::CENTER_TOP
                            .anchor_size(area.center_top() + egui::vec2(0.0, 10.0), galley.size())
                            .expand2(egui::vec2(10.0, 4.0));
                        ui.painter().rect_filled(rect, 6.0, egui::Color32::from_rgba_unmultiplied(20, 20, 20, 210));
                        ui.painter().galley(rect.min + egui::vec2(10.0, 4.0), galley, egui::Color32::WHITE);
                    }
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
        //
        // The title row (and everything else that reads the document) was
        // already drawn *this* frame, above, using whatever the document
        // looked like before these polls ran. So a Save As or an Open that
        // resolves right here — off the UI thread, no mouse or key event
        // attached to it — finishes with the *old* name still painted on
        // screen. `request_repaint_after` below only fires while something is
        // still busy; the instant it goes idle again nothing asks for
        // another frame at all, egui goes to sleep until real input arrives,
        // and the stale title sits there until it does (D1: "the title bar
        // row is only repainted on input"). So: notice the busy → idle
        // transition and ask for one more frame right away.
        let picker_was_busy = self.picker.busy();
        let picker_many_was_busy = self.picker_many.busy();
        let tasks_were_pending = !self.tasks.is_empty();
        self.pump_export_queue();
        self.poll_animate_import();
        self.poll_picker();
        self.poll_picker_many();
        // Before `poll_tasks`, so a BuzzEdit or Studio flow that has just
        // answered lands its result before its task is reported as ended.
        self.estate_pump();
        self.poll_tasks();
        self.voice_tick(&ctx);
        // The Assistant's tool calls run here, on the UI thread, one frame at a time.
        if self.agent_pump() {
            ctx.request_repaint();
        } else if self.agent_active() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        // The local API's requests (`crate::api`) run here too, for the same
        // reason: no document is touched off the UI thread.
        if self.api_pump(&ctx) {
            ctx.request_repaint();
        }
        if !self.exports.is_idle()
            || !self.tasks.is_empty()
            || self.picker.busy()
            || self.picker_many.busy()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if (picker_was_busy && !self.picker.busy())
            || (picker_many_was_busy && !self.picker_many.busy())
            || (tasks_were_pending && self.tasks.is_empty())
        {
            ctx.request_repaint();
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
        self.trace_dialog(&ctx);
        self.produce_dialog(&ctx);
        self.snapshots_dialog(&ctx);
        self.estate_dialog(&ctx);
        self.pasteboard_colour_dialog(&ctx);
        // The shortcut editor writes straight into the workspace keymap; save it
        // when it reports a change, the way every other preference is saved.
        if buzz_ui::shortcut_editor(&ctx, &mut self.shortcut_editor, &mut self.editor.workspace) {
            self.editor.workspace.save();
        }
        buzz_ui::about_dialog(&ctx, &mut self.editor.about);
        // The AI panel draws the same bodies as these three dialogs (F7.0);
        // while it is open the windows would only be a second copy of it.
        let ai_panel_open = self.editor.workspace.is_open(buzz_ui::PanelId::Ai);
        if !ai_panel_open {
            buzz_ui::ai_models_dialog(&ctx, &mut self.editor.ai_models, &mut self.editor.workspace.ai);
            self.ensure_ai_rig_limits();
            match buzz_ui::ai_character_dialog(&ctx, &mut self.editor.ai_character) {
                buzz_ui::AiCharacterDialogAction::Created(assembled, kind) => {
                    self.editor.create_ai_character_as(&assembled, kind);
                    self.ai_render_pose_sheet();
                }
                buzz_ui::AiCharacterDialogAction::None | buzz_ui::AiCharacterDialogAction::Cancel => {}
            }
        }
        let rename_action = if ai_panel_open {
            buzz_ui::AiRenameAction::None
        } else {
            self.editor.ai_rename.show(&ctx)
        };
        match rename_action {
            buzz_ui::AiRenameAction::Apply(proposals) => {
                let count = proposals.iter().filter(|p| p.selected).count();
                self.editor.doc.edit("AI: Identify and Rename Assets", |scene| {
                    buzz_ai::naming::apply_renames_to_scene(scene, &proposals);
                });
                self.editor.status = Some(format!("Applied AI renames to {count} assets"));
            }
            buzz_ui::AiRenameAction::None => {}
        }

        // Assets ▸ Arrange by type & style… (AS2).
        self.poll_arrange();
        match buzz_ui::arrange_dialog(&ctx, &mut self.arrange) {
            buzz_ui::ArrangeAction::Refresh => self.start_arrange_plan(),
            buzz_ui::ArrangeAction::Apply(moves) => self.start_arrange_apply(moves),
            buzz_ui::ArrangeAction::Undo => self.start_arrange_undo(),
            buzz_ui::ArrangeAction::None => {}
        }

        // File ▸ New asks before it acts, and the answer is remembered.
        let new_document = buzz_ui::new_document_dialog(&ctx, &mut self.editor.new_document);
        if let Some(setup) = new_document.create {
            self.editor.create_document(setup);
        }
        // Insert > Title Card: one undo step, at the playhead.
        let title_card = buzz_ui::title_card_dialog(&ctx, &mut self.editor.title_card);
        if let Some(settings) = title_card.add {
            let _ = self.editor.add_title_card(&settings);
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
                if self.user_command_allowed(command) {
                    self.dispatch(command);
                }
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
    /// **The Story panel**: the brief, the set, and the scenery.
    ///
    /// The state is taken out of the editor for the duration, because the panel
    /// wants `&mut` on it while the actions want `&mut` on the whole editor —
    /// the same trick every dialog here uses.
    fn story_panel(&mut self, ui: &mut egui::Ui) {
        let shots = self.editor.shot_summaries();
        let symbols = self.editor.library_choices();
        let current = self.editor.doc.active_scene();

        let mut state = std::mem::take(&mut self.editor.story);
        state.assets_indexed = crate::asset_sets::has_index(&self.editor.assets);
        let response = buzz_ui::story_panel(ui, &mut state, &shots, current, &symbols);

        if let Some(index) = response.go_to {
            self.editor.switch_scene(index);
            // **The words come with the shot.** Unless something is being
            // typed: replacing a draft somebody is halfway through is how a
            // paragraph gets lost, so the panel offers it instead.
            let untouched = state.draft.trim().is_empty()
                || shots
                    .get(state.draft_from.unwrap_or(usize::MAX))
                    .is_some_and(|s| s.brief == state.draft);
            if untouched {
                let brief = shots
                    .get(index)
                    .map(|s| s.brief.clone())
                    .unwrap_or_default();
                state.load(index, &brief);
            }
        }
        if response.direct {
            self.editor.story_direct(&mut state);
        }
        if response.direct_sequence {
            let story = state.draft.clone();
            let directed = self.editor.direct_sequence_dressed(&story, state.use_my_assets);
            state.report = self.editor.status.clone();
            state.ignored.clear();
            if directed > 0 {
                state.draft_from = Some(self.editor.doc.active_scene());
            }
        }
        if response.set_scene {
            self.editor.story_set_scene(&state);
        }
        if response.lay_scenery {
            self.editor.story_lay_scenery(&state);
        }
        // **The panel stays where you can see it.** Directing a shot selects
        // what it built, and the Properties panel above this one grows to
        // describe it — which, in a column that keeps its scroll offset in
        // pixels, pushes the panel you just pressed a button in off the bottom
        // of the screen. See `Workspace::scroll_to`.
        if response.direct || response.direct_sequence || response.set_scene || response.lay_scenery
        {
            self.editor.workspace.scroll_to = Some(buzz_ui::PanelId::Story);
        }
        self.editor.story = state;
    }

    /// **The Cast panel**: the characters a film is directed from, their
    /// wardrobe and their poses.
    ///
    /// The characters are the assets filed under [`buzz_ui::CAST_FOLDER`]. The
    /// wardrobe (every colour they wear) and their named poses are the open
    /// character's own, kept in [`Self::cast_sheet`] so a whole document is not
    /// re-read from disk every frame.
    fn cast_panel(&mut self, ui: &mut egui::Ui) {
        let characters: Vec<buzz_doc::Asset> = self
            .editor
            .assets
            .assets()
            .iter()
            .filter(|a| is_cast(&a.folder))
            .cloned()
            .collect();
        let can_add = !self.editor.selection.is_empty();

        // Load the open character's sheet if it changed since the last frame.
        let open = self.editor.cast_panel.open.clone();
        self.ensure_cast_sheet(open.as_deref());
        let (palette, poses): (&[(peniko::Color, usize)], &[String]) = match &self.cast_sheet {
            Some(sheet) => (&sheet.palette, &sheet.poses),
            None => (&[], &[]),
        };

        let mut state = std::mem::take(&mut self.editor.cast_panel);
        let thumbs = &mut self.asset_thumbnails;
        let action = buzz_ui::cast_panel(
            ui,
            &characters,
            palette,
            poses,
            &mut state,
            can_add,
            &mut |path| thumbs.get(path),
        );
        self.editor.cast_panel = state;

        if let Some(action) = action {
            self.apply_cast_action(action);
        }
    }

    /// Load the character at `path` into [`Self::cast_sheet`] unless it is
    /// already the one loaded, and drop the sheet when nothing is open. Reading
    /// a character is a whole-document load, so this runs only when the open
    /// character actually changes.
    fn ensure_cast_sheet(&mut self, path: Option<&std::path::Path>) {
        let Some(path) = path else {
            self.cast_sheet = None;
            return;
        };
        if self.cast_sheet.as_ref().is_some_and(|s| s.path == path) {
            return;
        }
        let asset = self
            .editor
            .assets
            .assets()
            .iter()
            .find(|a| a.path == path)
            .cloned();
        let Some(asset) = asset else {
            self.cast_sheet = None;
            return;
        };
        self.cast_sheet = match self.editor.assets.load(&asset) {
            Ok(scene) => Some(CastSheet {
                path: asset.path.clone(),
                palette: scene.colours_used(),
                poses: poses_of(&scene),
                scene,
            }),
            Err(_) => None,
        };
    }

    /// Act on what the Cast panel asked for.
    ///
    /// Casting reuses the asset placer, so a character arrives on the stage
    /// selected with its masks and rigging intact, exactly as any placed asset
    /// does. The wardrobe and the add are the panel's own.
    fn apply_cast_action(&mut self, action: buzz_ui::CastAction) {
        use buzz_ui::CastAction::*;
        match action {
            // The panel already recorded which character is open in its own
            // state; the sheet loads on the next frame.
            Open(_) => {}
            Cast(asset) => self.apply_asset_action(buzz_ui::AssetAction::Place(asset)),
            AddSelection => self.add_to_cast(),
            Rename { asset, name } => {
                if let Err(e) = self.editor.assets.rename(&asset, &name) {
                    self.editor.status = Some(format!("Could not rename: {e}"));
                } else {
                    // The file moved, so the path the sheet was keyed on is
                    // gone; the list still shows the character under its new
                    // name.
                    self.editor.cast_panel.open = None;
                    self.cast_sheet = None;
                    self.asset_thumbnails.forget();
                }
            }
            Delete(asset) => self.delete_from_cast(asset),
            Recolour { asset, from, to } => self.recolour_character(&asset, from, to),
            Rescan => {
                self.editor.assets.rescan();
                self.asset_thumbnails.forget();
                self.cast_sheet = None;
            }
        }
    }

    /// Keep the selected character as a new member of the cast and open its
    /// sheet. Filed under [`buzz_ui::CAST_FOLDER`], which is what tells a
    /// character apart from a prop in the same library.
    fn add_to_cast(&mut self) {
        let ids = self.editor.selection.ids();
        if ids.is_empty() {
            self.editor.status = Some("Select a character to add to the cast".into());
            return;
        }
        let doc = self
            .editor
            .doc
            .scene()
            .extract(self.editor.current_frame, &ids);
        let name = self
            .editor
            .assets
            .unique_name("Character", buzz_ui::CAST_FOLDER);
        match self.editor.assets.save(&name, buzz_ui::CAST_FOLDER, &doc) {
            Ok(saved) => {
                self.editor.cast_panel.open = Some(saved.path.clone());
                self.cast_sheet = None;
                self.editor.workspace.select_tab(buzz_ui::PanelId::Cast);
                self.asset_thumbnails.forget();
                self.editor.status = Some(format!(
                    "Added {} to the cast \u{2014} double-click the name to rename",
                    saved.name
                ));
            }
            Err(e) => self.editor.status = Some(format!("Could not add to the cast: {e}")),
        }
    }

    /// Remove a character from the cast, on the second click — a file on disk
    /// has no undo, so the first click arms and the second removes.
    fn delete_from_cast(&mut self, asset: buzz_doc::Asset) {
        if self.editor.cast_panel.confirm_delete.as_deref() != Some(asset.path.as_path()) {
            self.editor.cast_panel.confirm_delete = Some(asset.path.clone());
            self.editor.status = Some(format!(
                "Delete {} \u{2014} click Delete again to confirm",
                asset.name
            ));
            return;
        }
        self.editor.cast_panel.confirm_delete = None;
        match self.editor.assets.delete(&asset) {
            Ok(()) => {
                if self.editor.cast_panel.open.as_deref() == Some(asset.path.as_path()) {
                    self.editor.cast_panel.open = None;
                }
                self.cast_sheet = None;
                self.asset_thumbnails.forget();
                self.editor.status = Some(format!("Removed {} from the cast", asset.name));
            }
            Err(e) => self.editor.status = Some(format!("Could not remove {}: {e}", asset.name)),
        }
    }

    /// **Re-skin a character**: repaint every part painted `from` with `to`, on
    /// every pose, and write the character back so the new skin is theirs from
    /// now on.
    ///
    /// The recolour is applied to the character's loaded document, so the
    /// wardrobe updates the moment the well changes; the file is rewritten and
    /// its thumbnail forgotten so the list catches up too.
    fn recolour_character(
        &mut self,
        asset: &buzz_doc::Asset,
        from: peniko::Color,
        to: peniko::Color,
    ) {
        self.ensure_cast_sheet(Some(asset.path.as_path()));
        let Some(sheet) = self.cast_sheet.as_mut() else {
            self.editor.status = Some(format!("Could not read {}", asset.name));
            return;
        };
        let painted = sheet.scene.recolour_matching(from, to);
        if painted == 0 {
            return;
        }
        sheet.palette = sheet.scene.colours_used();
        let scene = sheet.scene.clone();
        match self.editor.assets.save(&asset.name, &asset.folder, &scene) {
            Ok(_) => {
                self.asset_thumbnails.forget();
                self.editor.status = Some(match painted {
                    1 => format!("Re-skinned {} \u{2014} 1 part", asset.name),
                    n => format!("Re-skinned {} \u{2014} {n} parts", asset.name),
                });
            }
            Err(e) => self.editor.status = Some(format!("Could not save {}: {e}", asset.name)),
        }
    }

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

    /// **The title bar row**: the logo, the menu and the document's name,
    /// and — on Windows — the window's own minimise, maximise and close
    /// buttons.
    ///
    /// Laid out at a fixed 32-point height whenever the mode is not
    /// [`TitleBar::Native`][crate::title_bar::TitleBar::Native] — Native
    /// leaves the row exactly as it always drew, an ordinary menu bar of
    /// whatever height the panel gives it.
    fn title_bar_row(
        &mut self,
        ui: &mut egui::Ui,
        commands: &mut Vec<Command>,
        template_names: &[String],
        sequence_menu: &panels::SequenceMenuInfo,
        can_undo: bool,
        can_redo: bool,
    ) {
        use crate::title_bar::TitleBar;

        if self.title_bar == TitleBar::Native {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.status_strip(ui);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let state =
                            self.menu_state(template_names, sequence_menu, can_undo, can_redo);
                        commands.extend(panels::menu_bar(ui, &state));
                    });
                });
            });
            return;
        }

        ui.horizontal(|ui| {
            ui.set_height(32.0);

            // **All of the bar's bare space drags the window**, not only the
            // gap around the name. The menu's `Ui` is handed nearly the whole
            // row (see `bounded_menu`), so that gap alone was ~150 points of
            // a 1500-point bar, and nothing at all below ~1100. Registered
            // before anything else in the row, so every widget drawn after it
            // — menus, buttons, the status strip — still wins its own clicks
            // and only the pointer over empty bar falls through to this.
            let background = ui.interact(
                ui.max_rect(),
                ui.id().with("title_bar_background"),
                egui::Sense::click_and_drag(),
            );
            self.queue_title_drag(&background);

            if self.title_bar.is_custom() {
                // **The three window buttons, placed first.** `right_to_left`
                // lays each new widget to the *left* of the one before it, so
                // placing these before the menu is what keeps a wide menu
                // from being able to grow far enough to cover them — the same
                // overflow this row would otherwise share with
                // `section_header`'s own right-anchored menu.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.window_buttons(ui);
                    self.status_strip(ui);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        self.title_bar_logo(ui);
                        self.bounded_menu(ui, commands, template_names, sequence_menu, can_undo, can_redo);
                        self.title_drag_zone(ui);
                    });
                });
            } else {
                // MacInset: the traffic lights are the OS's own, drawn to the
                // left of the content view; this much space is where they
                // sit, so the app's own row starts clear of them.
                ui.add_space(78.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.status_strip(ui);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        self.bounded_menu(
                            ui,
                            commands,
                            template_names,
                            sequence_menu,
                            can_undo,
                            can_redo,
                        );
                        self.title_drag_zone(ui);
                    });
                });
            }
        });
    }

    /// `panels::menu_bar` drawn into a `Ui` capped well short of everything
    /// that is left, so there is always room for the drag zone after it.
    ///
    /// `egui::MenuBar` sets its own minimum width to *all* of whatever `Ui`
    /// it is handed (`ui.set_min_size(vec2(ui.available_width(), height))`),
    /// however few menus are actually in it — so without this, the menu
    /// alone would claim the entire row and the drag zone would never get a
    /// point of it.
    fn bounded_menu(
        &self,
        ui: &mut egui::Ui,
        commands: &mut Vec<Command>,
        template_names: &[String],
        sequence_menu: &panels::SequenceMenuInfo,
        can_undo: bool,
        can_redo: bool,
    ) {
        /// The least the drag zone is left, whatever the menu's own natural
        /// width would otherwise have been — enough for a short document
        /// name.
        const DRAG_ZONE_MIN: f32 = 160.0;

        let menu_width = (ui.available_width() - DRAG_ZONE_MIN).max(0.0);
        let menu_rect =
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(menu_width, ui.available_height()));
        let state = self.menu_state(template_names, sequence_menu, can_undo, can_redo);
        ui.scope_builder(egui::UiBuilder::new().max_rect(menu_rect), |ui| {
            commands.extend(panels::menu_bar(ui, &state));
        });
    }

    /// The state `panels::menu_bar` reads, gathered from the editor. A
    /// method rather than a closure: a closure returning something borrowed
    /// from its argument needs a higher-ranked lifetime bound Rust cannot
    /// infer on its own, and a method's `&self` sidesteps that entirely.
    fn menu_state<'a>(
        &'a self,
        template_names: &'a [String],
        sequence_menu: &'a panels::SequenceMenuInfo,
        can_undo: bool,
        can_redo: bool,
    ) -> panels::MenuState<'a> {
        panels::MenuState {
            scene: self.editor.scene(),
            selection: &self.editor.selection,
            view: &self.editor.view,
            can_undo,
            can_redo,
            workspace: &self.editor.workspace,
            light_gizmos: self.editor.light_panel.gizmos,
            templates: template_names,
            sequence: sequence_menu,
        }
    }

    /// The logo at 18 points, lazily uploaded as a texture the first time
    /// this is drawn.
    fn title_bar_logo(&mut self, ui: &mut egui::Ui) {
        let texture = self.logo_texture.get_or_insert_with(|| {
            let (pixels, w, h) = decode_png_rgba(include_bytes!("../../../assets/logo-32.png"))
                .unwrap_or_else(|| (vec![0u8, 0, 0, 0], 1, 1));
            let image =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
            ui.ctx()
                .load_texture("title-bar-logo", image, egui::TextureOptions::LINEAR)
        });
        let id = texture.id();
        ui.add_space(6.0);
        ui.add(egui::Image::new((id, egui::vec2(18.0, 18.0))));
        ui.add_space(4.0);
    }

    /// The rest of the row: a click-and-drag zone carrying the document's
    /// name, centred. Dragging it moves the window; double-clicking it
    /// toggles maximised, the same as double-clicking any native title bar.
    fn title_drag_zone(&mut self, ui: &mut egui::Ui) {
        let rest = ui.available_size();
        if rest.x <= 0.0 {
            self.title_rects.drag = None;
            return;
        }
        let (rect, response) = ui.allocate_exact_size(rest, egui::Sense::click_and_drag());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            self.editor.doc.display_name(),
            egui::FontId::proportional(13.0),
            Palette::text_dim(),
        );
        self.title_rects.drag = Some(rect);
        self.queue_title_drag(&response);
    }

    /// Dragging a piece of the title bar moves the window; double-clicking
    /// it toggles maximised. Shared by the name's zone and the row's bare
    /// background.
    fn queue_title_drag(&mut self, response: &egui::Response) {
        if response.drag_started() {
            self.window_requests
                .push(crate::title_bar::WindowRequest::StartDrag);
        }
        if response.double_clicked() {
            self.window_requests
                .push(crate::title_bar::WindowRequest::ToggleMaximize);
        }
    }

    /// The minimise, maximise and close buttons, 46×32 each, Windows'
    /// own size for them. Placed close first — see `title_bar_row` — so
    /// close ends up at the very corner, exactly where forty years of
    /// muscle memory expects it.
    fn window_buttons(&mut self, ui: &mut egui::Ui) {
        use crate::title_bar::WindowRequest;

        const SIZE: egui::Vec2 = egui::vec2(46.0, 32.0);
        const CLOSE_HOVER: egui::Color32 = egui::Color32::from_rgb(0xE8, 0x11, 0x23);
        // Click *and drag*, though only clicks are acted on: a click-only
        // button lets a drag fall through to the bar's background behind
        // it, so pressing close and sliding off would move the window. This
        // way the slide just cancels the press, as on a native button.
        let sense = egui::Sense::click_and_drag();

        let close = ui.allocate_response(SIZE, sense);
        let close_hovered = close.hovered();
        if close_hovered {
            ui.painter().rect_filled(close.rect, 0.0, CLOSE_HOVER);
        } else if close.is_pointer_button_down_on() {
            ui.painter().rect_filled(close.rect, 0.0, Palette::hover());
        }
        paint_x_glyph(
            ui.painter(),
            close.rect,
            if close_hovered {
                egui::Color32::WHITE
            } else {
                Palette::text()
            },
        );
        self.title_rects.close = Some(close.rect);
        if close.clicked() {
            self.window_requests.push(WindowRequest::Close);
        }

        let maximize = ui.allocate_response(SIZE, sense);
        if maximize.hovered() {
            ui.painter().rect_filled(maximize.rect, 0.0, Palette::hover());
        }
        if self.window_maximized {
            paint_restore_glyph(ui.painter(), maximize.rect, Palette::text());
        } else {
            paint_maximize_glyph(ui.painter(), maximize.rect, Palette::text());
        }
        self.title_rects.maximize = Some(maximize.rect);
        if maximize.clicked() {
            self.window_requests.push(WindowRequest::ToggleMaximize);
        }

        let minimize = ui.allocate_response(SIZE, sense);
        if minimize.hovered() {
            ui.painter()
                .rect_filled(minimize.rect, 0.0, Palette::hover());
        }
        paint_minimize_glyph(ui.painter(), minimize.rect, Palette::text());
        self.title_rects.minimize = Some(minimize.rect);
        if minimize.clicked() {
            self.window_requests.push(WindowRequest::Minimize);
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
        dock: buzz_ui::Dock,
        sections: &[buzz_ui::Section],
        workspace: &buzz_ui::Workspace,
        all_fronts: &[buzz_ui::PanelId],
        locked: bool,
        requests: &mut DockRequests,
        commands: &mut Vec<Command>,
    ) {
        // **Both axes, and the horizontal one is not decoration.**
        //
        // A row wider than the column used to be drawn straight past the edge —
        // and `egui` does not simply clip it. A panel's reported rectangle is
        // its *contents'*, so a right-hand column whose rows overflow by 170
        // points reports itself 170 points further right than it was drawn, and
        // the stage is then laid out through that gap: its ruler, its scroll
        // bars and its zoom pill painted across the panel, which is exactly
        // what "the stage's scrollbar overlaps the property panel" is.
        //
        // Scrolling the width instead keeps the column's rectangle honest — the
        // stage stops where the panel starts — and gives the cut-off end of a
        // row somewhere to be reached from, rather than nowhere.
        //
        // **And scrolled to a panel somebody asked for.** A column of ten
        // panels is taller than the screen, and a panel opened, fronted and
        // unrolled at row six of it is below the fold — which is what "Direct
        // a Story does nothing" turned out to mean, twice. The request comes
        // from `Workspace::reveal`, and is spent by the column that finds the
        // panel, so a panel floating or docked elsewhere leaves it standing for
        // the frame it is docked on.
        let wanted = self.editor.workspace.scroll_to;
        egui::ScrollArea::both()
            .id_salt(("column", sections.first().map(|s| s.front)))
            .show(ui, |ui| {
                for (index, section) in sections.iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    let top = ui.cursor().top();
                    // Rolled up, the header carries the name whatever the panel
                    // would normally do, or the column becomes a stack of
                    // anonymous strips. A tabbed section always shows its tabs,
                    // so it never needs this.
                    let named = section.collapsed || !section.front.draws_own_title();
                    let emphasis = self.emphasis_for(section);
                    self.timeline_tools_slot = section_header(
                        ui,
                        section,
                        workspace,
                        all_fronts,
                        locked,
                        named,
                        true,
                        emphasis,
                        requests,
                    );
                    if !section.collapsed {
                        egui::Frame::new()
                            .inner_margin(buzz_ui::theme::Tokens::SPACE_2 as i8)
                            .show(ui, |ui| self.draw_panel(ui, section.front, commands));
                    }
                    // The section's top to the top of the view: what a scroll
                    // aims at, and what a dragged panel can be dropped on.
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(ui.min_rect().left(), top),
                        egui::pos2(ui.min_rect().right(), ui.cursor().top()),
                    );
                    self.drop_zones.push(DropZone {
                        dock,
                        group: section.group,
                        front: section.front,
                        rect: rect.intersect(ui.clip_rect()),
                    });
                    if wanted.is_some_and(|id| section.panels.contains(&id)) {
                        // A panel taller than the column is shown from its
                        // title, not from wherever its bottom lands.
                        ui.scroll_to_rect(rect, Some(egui::Align::Min));
                        self.editor.workspace.scroll_to = None;
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
        // While the Assistant runs, every panel that can change the scene is
        // drawn disabled; the Assistant and Tasks panels stay live (Stop).
        let enabled = !self.agent_active()
            || matches!(id, buzz_ui::PanelId::Ai | buzz_ui::PanelId::Tasks);
        if enabled {
            self.draw_panel_inner(ui, id, commands);
        } else {
            ui.add_enabled_ui(false, |ui| self.draw_panel_inner(ui, id, commands));
        }
    }

    /// Commands a person may still raise while the Assistant runs: view,
    /// layout and playback only. Everything else would edit the document.
    pub(crate) fn user_command_allowed(&self, command: Command) -> bool {
        use Command::*;
        if !self.agent_active() {
            return true;
        }
        matches!(
            command,
            Quit | Close
                | ZoomIn
                | ZoomOut
                | ZoomActual
                | ZoomFitInWindow
                | ZoomShowFrame
                | ZoomShowAll
                | ToggleRulers
                | ToggleGrid
                | ToggleGuides
                | ToggleSnapping
                | TogglePasteboard
                | PasteboardColour
                | TogglePanel(_)
                | ToggleLayoutLock
                | ToggleTheme
                | SetTheme(_)
                | SetUiScale(_)
                | ToggleReduceMotion
                | About
                | ResetWorkspace
                | SetWorkspacePreset(_)
                | SaveWorkspace
                | ShortcutEditor
                | AiModels
                | VoicePushToTalk
                | VoiceMute
                | PlayPause
        )
    }

    fn draw_panel_inner(&mut self, ui: &mut egui::Ui, id: buzz_ui::PanelId, commands: &mut Vec<Command>) {
        use buzz_ui::PanelId::*;
        match id {
            Tools => {
                if let Some(tool) = panels::tool_bar(ui, self.editor.tool(), &mut self.editor.style)
                {
                    commands.push(Command::SelectTool(tool));
                }
            }

            ToolOptions => {
                let tool = self.editor.tool();
                let pan_speed_before = self.editor.workspace.pan_speed;
                let rig_before = self.editor.workspace.rig;
                let editor = &mut self.editor;
                panels::tool_options_panel(
                    ui,
                    tool,
                    editor.doc.scene(),
                    &mut editor.style,
                    &mut editor.workspace.rig,
                    &mut editor.workspace.pan_speed,
                );
                if self.editor.remember_brush_tips()
                    || self.editor.workspace.pan_speed != pan_speed_before
                    || self.editor.workspace.rig != rig_before
                {
                    self.editor.workspace.save();
                }
                if tool == buzz_ui::ToolId::Bone {
                    self.bone_tool_joint_options(ui);
                }
                if tool == buzz_ui::ToolId::FreeTransform && self.selection_has_followers() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Rotating this part carries the parts that follow it")
                            .weak(),
                    );
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
                let text_of = self
                    .editor
                    .selection
                    .iter()
                    .next()
                    .and_then(|id| self.editor.text_of(id).map(|t| (id, t)));
                if let Some((id, text)) = text_of {
                    let mut content = text.content.clone();
                    let mut size = text.size;
                    let mut font = text.font.clone();
                    let mut style = text.style;
                    let mut align = text.align;
                    let char_len = text.content.chars().count();

                    ui.separator();
                    ui.label("Text");
                    let output = egui::TextEdit::multiline(&mut content)
                        .id(egui::Id::new("text-props-edit").with(id.0))
                        .show(ui);
                    let mut changed = output.response.changed();

                    // **Remember the last non-empty selection across frames.**
                    // Clicking a style button below steals focus from the text
                    // box, which would otherwise drop `cursor_range` at the very
                    // moment it is needed to know which characters the click was
                    // about. Keyed by object id, so switching to another text
                    // object starts fresh rather than reusing a stale range.
                    let selection_key = egui::Id::new("text-props-selection").with(id.0);
                    if let Some(range) = output.cursor_range {
                        let range = range.as_sorted_char_range();
                        if !range.is_empty() {
                            ui.data_mut(|d| d.insert_temp(selection_key, range));
                        }
                    }
                    let selection: Option<std::ops::Range<usize>> = ui
                        .data_mut(|d| d.get_temp::<std::ops::Range<usize>>(selection_key))
                        .filter(|r| r.start < r.end && r.end <= char_len);

                    // What a run covering the selection's first character
                    // already says — enough to show a style button's state and
                    // decide which way a click toggles it; a selection spanning
                    // more than one style is rare, and this is only ever what a
                    // button *shows*, never what it silently overwrites outside
                    // the chosen range.
                    let run_at_selection = selection
                        .as_ref()
                        .and_then(|r| text.runs.iter().find(|run| run.start <= r.start && r.start < run.end));
                    let effective_bold = run_at_selection.and_then(|r| r.bold).unwrap_or(style.bold);
                    let effective_italic =
                        run_at_selection.and_then(|r| r.italic).unwrap_or(style.italic);
                    let effective_font = run_at_selection
                        .and_then(|r| r.font.clone())
                        .or_else(|| font.clone());
                    let mut display_size = run_at_selection.and_then(|r| r.size).unwrap_or(size);
                    let display_color = run_at_selection
                        .and_then(|r| r.color)
                        .unwrap_or_else(|| self.editor.text_base_color(id));

                    // **Apply one style change to the selection**, merging it
                    // into `runs` and rebuilding immediately — bypassing the
                    // whole-text `set_text_styled` path below entirely, since a
                    // ranged edit must never touch the base look.
                    let restyle = |editor: &mut Editor, mutate: &dyn Fn(&mut buzz_scene::TextRun)| {
                        if let Some(range) = &selection {
                            let runs =
                                buzz_scene::restyle_runs(&text.runs, char_len, range.start, range.end, mutate);
                            editor.set_text_runs(
                                id,
                                buzz_scene::TextData {
                                    runs,
                                    ..text.clone()
                                },
                            );
                        }
                    };

                    let size_changed = ui
                        .add(
                            egui::DragValue::new(&mut display_size)
                                .range(4.0..=400.0)
                                .prefix("size "),
                        )
                        .changed();
                    if size_changed {
                        if selection.is_some() {
                            restyle(&mut self.editor, &|r| r.size = Some(display_size));
                        } else {
                            size = display_size;
                            changed = true;
                        }
                    }

                    // Font picker: "Default" plus every family installed on the
                    // system (Hindi ones flagged), each row **set in its own
                    // face** — a list of names in the interface's own font
                    // answers the one question nobody is asking while scrolling.
                    let current = effective_font.clone().unwrap_or_else(|| "Default".to_string());
                    let previews = &mut self.font_previews;
                    let mut font_pick: Option<Option<String>> = None;
                    egui::ComboBox::from_id_salt("text-font")
                        .selected_text(current)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(320.0)
                                .show(ui, |ui| {
                                    if ui
                                        .selectable_label(effective_font.is_none(), "Default")
                                        .clicked()
                                    {
                                        font_pick = Some(None);
                                    }
                                    for family in buzz_text::font_families() {
                                        let suffix =
                                            if family.devanagari { "\u{0905}" } else { "" };
                                        let selected =
                                            effective_font.as_deref() == Some(family.name.as_str());
                                        // Previewed in the cut being edited, so
                                        // choosing a family while bold is on shows
                                        // that family's bold.
                                        let cut = if family.has(style) {
                                            style
                                        } else {
                                            buzz_scene::FontStyle::REGULAR
                                        };
                                        if previews
                                            .row(ui, &family.name, cut, selected, suffix)
                                            .clicked()
                                        {
                                            font_pick = Some(Some(family.name.clone()));
                                        }
                                    }
                                });
                        });
                    if let Some(picked) = font_pick {
                        if selection.is_some() {
                            restyle(&mut self.editor, &|r| r.font = picked.clone());
                        } else {
                            font = picked;
                            changed = true;
                        }
                    }

                    // **Bold and italic are offered only where they exist.** A
                    // family installed without an italic has nothing to draw one
                    // with, and a toggle that changes nothing is worse than no
                    // toggle at all.
                    let family = effective_font.as_deref().and_then(buzz_text::family);
                    let has_bold = family.is_none_or(|f| f.has_bold());
                    let has_italic = family.is_none_or(|f| f.has_italic());
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                has_bold,
                                egui::Button::new(egui::RichText::new("B").strong())
                                    .small()
                                    .selected(effective_bold),
                            )
                            .on_hover_text("Bold")
                            .on_disabled_hover_text("This family has no bold")
                            .clicked()
                        {
                            let next = !effective_bold;
                            if selection.is_some() {
                                restyle(&mut self.editor, &move |r| r.bold = Some(next));
                            } else {
                                style.bold = next;
                                changed = true;
                            }
                        }
                        if ui
                            .add_enabled(
                                has_italic,
                                egui::Button::new(egui::RichText::new("I").italics())
                                    .small()
                                    .selected(effective_italic),
                            )
                            .on_hover_text("Italic")
                            .on_disabled_hover_text("This family has no italic")
                            .clicked()
                        {
                            let next = !effective_italic;
                            if selection.is_some() {
                                restyle(&mut self.editor, &move |r| r.italic = Some(next));
                            } else {
                                style.italic = next;
                                changed = true;
                            }
                        }

                        // The colour of the selected characters — only
                        // meaningful with a selection, since a text object has
                        // no colour of its own outside one (its fill, or the
                        // Color panel, already sets that for the whole text).
                        let [r, g, b, a] = display_color.to_rgba8().to_u8_array();
                        let mut color32 = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
                        let color_response = ui
                            .add_enabled_ui(selection.is_some(), |ui| {
                                ui.color_edit_button_srgba(&mut color32)
                            })
                            .inner
                            .on_hover_text("Colour")
                            .on_disabled_hover_text("Select some characters first");
                        if color_response.changed() {
                            let picked = peniko::Color::from_rgba8(
                                color32.r(),
                                color32.g(),
                                color32.b(),
                                color32.a(),
                            );
                            restyle(&mut self.editor, &move |r| r.color = Some(picked));
                        }

                        ui.separator();
                        for option in buzz_scene::TextAlign::ALL {
                            if ui
                                .add(
                                    egui::Button::new(option.label())
                                        .small()
                                        .selected(align == option),
                                )
                                .on_hover_text("How the lines line up with each other")
                                .clicked()
                            {
                                align = option;
                                changed = true;
                            }
                        }
                    });

                    if changed {
                        self.editor
                            .set_text_styled(id, content, size, font, style, align);
                    }
                }
            }

            Color => {
                let editor = &mut self.editor;
                panels::color_panel(ui, editor.doc.scene(), &mut editor.style);
                // Choosing the Texture fill needs a tile in the document to
                // paint with; this bakes one the first time and then does
                // nothing until the recipe changes.
                editor.ensure_fill_texture();

                // Textures fill the selected shapes: procedural tiles baked from
                // the fill (foreground) and stroke (background) colours, plus any
                // image used as a fill. Applied to the selection, so it needs one.
                ui.separator();
                ui.label("Texture");
                let has_shape = self.editor.selection.iter().next().is_some();
                ui.add_enabled_ui(has_shape, |ui| {
                    // **Under family headings, not in one wrapped wall.** There
                    // are twenty-one of these now, and "Lawn" sitting next to
                    // "Checker" tells you nothing about which of them is
                    // ground cover. The heading is what turns the list into
                    // something you scan rather than read.
                    for (family, kinds) in buzz_scene::TextureKind::GROUPS {
                        ui.label(egui::RichText::new(family).small().weak());
                        ui.horizontal_wrapped(|ui| {
                            for kind in kinds {
                                if ui.button(kind.label()).clicked() {
                                    self.editor.apply_texture(*kind);
                                }
                            }
                        });
                    }
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

                // **What new shapes will be drawn with**, when the fill kind is
                // a texture. The same recipe controls as below, aimed at the
                // pencil rather than at the selection.
                if self.editor.style.fill_kind == buzz_ui::FillKind::Texture {
                    ui.separator();
                    ui.label("New shapes");
                    let style = &mut self.editor.style;
                    let recipe = &mut style.fill_texture;
                    egui::Grid::new("new-shape-texture")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Ink");
                            panels::color_row(ui, "new-tex-fg", &mut recipe.fg);
                            ui.end_row();
                            ui.label("Ground");
                            panels::color_row(ui, "new-tex-bg", &mut recipe.bg);
                            ui.end_row();
                            ui.label("Contrast");
                            ui.add(
                                egui::Slider::new(&mut recipe.contrast, 0.0..=4.0).step_by(0.05),
                            );
                            ui.end_row();
                        });
                    for (family, kinds) in buzz_scene::TextureKind::GROUPS {
                        ui.label(egui::RichText::new(family).small().weak());
                        ui.horizontal_wrapped(|ui| {
                            for kind in kinds {
                                if ui
                                    .add(
                                        egui::Button::new(kind.label())
                                            .small()
                                            .selected(*kind == recipe.kind),
                                    )
                                    .clicked()
                                {
                                    recipe.kind = *kind;
                                    recipe.detail = kind.default_detail();
                                }
                            }
                        });
                    }
                    self.editor.ensure_fill_texture();
                }

                // **Re-tuning what is already there.** A texture keeps the
                // recipe that made it, so the selection's own texture can be
                // adjusted in place rather than undone and re-applied — and the
                // shape under the cursor updates as the slider moves.
                if let Some((recipe, cell, rotation)) = self.editor.selected_texture() {
                    ui.separator();
                    ui.label(format!("{} texture", recipe.kind.label()));
                    let mut edited = recipe;
                    let mut placement = (cell, rotation);
                    let mut changed = false;

                    egui::Grid::new("texture-recipe")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Ink");
                            changed |= panels::color_row(ui, "tex-fg", &mut edited.fg);
                            ui.end_row();

                            ui.label("Ground");
                            changed |= panels::color_row(ui, "tex-bg", &mut edited.bg);
                            ui.end_row();

                            ui.label("Detail").on_hover_text(
                                "How many repeats of the pattern fit in one tile. \
                             Snapped to what can wrap without a seam.",
                            );
                            let mut detail = edited.detail();
                            if ui
                                .add(egui::Slider::new(
                                    &mut detail,
                                    recipe.kind.min_detail()..=16,
                                ))
                                .changed()
                            {
                                edited.detail = detail;
                                changed = true;
                            }
                            ui.end_row();

                            ui.label("Contrast")
                                .on_hover_text("How far apart the two colours are pushed");
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut edited.contrast, 0.0..=4.0)
                                        .step_by(0.05),
                                )
                                .changed();
                            ui.end_row();

                            ui.label("Tile size")
                                .on_hover_text("How big one repeat is on the shape");
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut placement.0)
                                        .range(1.0..=4096.0)
                                        .speed(0.5),
                                )
                                .changed();
                            ui.end_row();

                            ui.label("Angle")
                                .on_hover_text("Which way the tile is turned");
                            let mut degrees = placement.1.to_degrees();
                            if ui
                                .add(
                                    egui::Slider::new(&mut degrees, -180.0..=180.0)
                                        .suffix("\u{b0}")
                                        .fixed_decimals(0),
                                )
                                .changed()
                            {
                                placement.1 = degrees.to_radians();
                                changed = true;
                            }
                            ui.end_row();
                        });

                    for (family, kinds) in buzz_scene::TextureKind::GROUPS {
                        ui.label(egui::RichText::new(family).small().weak());
                        ui.horizontal_wrapped(|ui| {
                            for kind in kinds {
                                if ui
                                    .add(
                                        egui::Button::new(kind.label())
                                            .small()
                                            .selected(*kind == recipe.kind),
                                    )
                                    .clicked()
                                {
                                    edited.kind = *kind;
                                    // The new pattern's own detail, since what
                                    // suits a weave does not suit a wall.
                                    edited.detail = kind.default_detail();
                                    changed = true;
                                }
                            }
                        });
                    }

                    if changed {
                        self.editor.retexture(edited, Some(placement));
                    }
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
                        let active =
                            showing && self.editor.view.perspective.vanishing_points.len() == n;
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
                self.editor.assets_panel.thumbnail_size =
                    self.editor.workspace.asset_thumbnail_size;
                let thumbs = &mut self.asset_thumbnails;
                let action = buzz_ui::assets_panel(
                    ui,
                    &self.editor.assets,
                    &mut self.editor.assets_panel,
                    can_add,
                    &mut |path| thumbs.get(path),
                );
                if self.editor.assets_panel.thumbnail_size
                    != self.editor.workspace.asset_thumbnail_size
                {
                    self.editor.workspace.asset_thumbnail_size =
                        self.editor.assets_panel.thumbnail_size;
                    self.editor.workspace.save();
                }
                if let Some(action) = action {
                    self.apply_asset_action(action);
                }
            }

            Cast => self.cast_panel(ui),

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
            Story => self.story_panel(ui),
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
                    selected_layers: self.editor.selection.selected_layers().to_vec(),
                    range_anchor: self.editor.selection.range_anchor(),
                    timeline_selection: self.editor.selection.timeline().cloned(),
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
                    beats: self.editor.ruler_marks.clone(),
                    shot_end: self.editor.shot_end_marker(),
                };
                let response = buzz_ui::timeline_panel_with_tools_slot(
                    ui,
                    self.editor.scene(),
                    &state,
                    self.timeline_tools_slot.take(),
                );
                self.apply_timeline(response, commands);
            }

            Actions => {
                let saved: Vec<buzz_ui::SavedScript> = self
                    .commands
                    .iter()
                    .map(|c| buzz_ui::SavedScript {
                        name: c.name.clone(),
                        source: c.source.clone(),
                    })
                    .collect();
                let response =
                    buzz_ui::actions_panel(ui, &mut self.editor.actions, &script_samples(), &saved);
                // Ctrl+Enter may already have raised it this frame; clicking
                // Run as well should not run it twice.
                if response.run && !commands.contains(&Command::RunScript) {
                    commands.push(Command::RunScript);
                }
                if let Some(name) = response.save {
                    match self.commands.save(&name, &self.editor.actions.source) {
                        Ok(c) => self.editor.status = Some(format!("Saved command: {}", c.name)),
                        Err(e) => {
                            self.editor.status = Some(format!("Could not save the command: {e}"))
                        }
                    }
                }
            }

            Tasks => self.tasks_panel(ui),
            Ai => self.draw_ai_panel(ui),
            MotionEditor => self.motion_editor_panel(ui),
            Sequence => self.sequence_panel(ui),
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

    /// **The Sequence strip.** See `docs/SEQUENCE_SETUPS_PLAN.md` "The
    /// Sequence strip" and [`buzz_ui::sequence_panel`].
    fn sequence_panel(&mut self, ui: &mut egui::Ui) {
        let view = self.editor.sequence_view();
        let response = buzz_ui::sequence_panel(ui, &view);
        self.editor.apply_sequence(&response);
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
        let wanted = self.editor.camera_selected || self.editor.tool() == buzz_ui::ToolId::Camera;
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
        let layer = editor
            .selection
            .active_layer()
            .or_else(|| object.and_then(|id| editor.scene().find_object(id).map(|(l, _)| l)))
            .or_else(|| editor.selection.selected_layers().first().copied())
            .or_else(|| editor.scene().layers().iter().next().map(|l| l.id));
        let target = editor.filter_panel.target;

        let (filters, blend, layer_effects) = match target {
            buzz_ui::FilterTarget::Object => {
                match object.and_then(|id| editor.scene().find_object(id)) {
                    Some((_, found)) => (found.filters.clone(), Some(found.blend), None),
                    None => (Vec::new(), None, None),
                }
            }
            buzz_ui::FilterTarget::Layer => {
                let found = layer.and_then(|id| editor.scene().layers().get(id));
                (
                    found.map(|l| l.filters.clone()).unwrap_or_default(),
                    found.map(|l| l.blend),
                    found.map(|l| buzz_ui::LayerEffects {
                        alpha: l.alpha,
                        brightness: l.brightness,
                        tint: l.tint,
                        tint_amount: l.tint_amount,
                        blend: l.blend,
                    }),
                )
            }
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
            layer_effects,
        );

        // Re-read target: the user may have toggled between Object and Layer in the UI.
        let target = editor.filter_panel.target;

        // Where an edit lands. Both arms write the same `Vec`, so the whole
        // panel is one code path with one place that decides the target.
        let apply = |editor: &mut Editor,
                     label: &'static str,
                     change: &dyn Fn(&mut Vec<buzz_scene::Filter>)| {
            editor.edit_filter_list(target, object, layer, label, change);
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
            editor.add_modifier(id, modifier);
        }
        if let Some(index) = response.remove_modifier
            && let Some(id) = object
        {
            editor.remove_modifier(id, index);
        }
        if let Some((index, modifier)) = response.set_modifier
            && let Some(id) = object
        {
            // No end_gesture: consecutive drags coalesce into one undo step.
            editor.set_modifier(id, index, modifier);
        }

        if let Some(blend) = response.set_blend {
            match target {
                buzz_ui::FilterTarget::Object => {
                    if let Some(id) = object {
                        editor.doc.edit("Blend Mode", |scene| {
                            scene.update_object(id, |o| o.blend = blend);
                        });
                    }
                }
                buzz_ui::FilterTarget::Layer => {
                    if let Some(id) = layer {
                        editor.doc.edit("Layer Blend Mode", |scene| {
                            scene.update_layer(id, |l| l.blend = blend);
                        });
                    }
                }
            }
        }
        if let Some(alpha) = response.set_layer_alpha
            && let Some(id) = layer
        {
            editor.doc.edit("Layer Opacity", |scene| {
                scene.update_layer(id, |l| l.alpha = alpha);
            });
        }
        if let Some(brightness) = response.set_layer_brightness
            && let Some(id) = layer
        {
            editor.doc.edit("Layer Brightness", |scene| {
                scene.update_layer(id, |l| l.brightness = brightness);
            });
        }
        if let Some((tint, amount)) = response.set_layer_tint
            && let Some(id) = layer
        {
            editor.doc.edit("Layer Tint", |scene| {
                scene.update_layer(id, |l| {
                    l.tint = tint;
                    l.tint_amount = amount;
                });
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

        // **Which sound the panel edits, and the frame it sits on.**
        //
        // A sound lives on a keyframe, but requiring the playhead to sit exactly
        // on that keyframe made the whole panel — the clip, its start frame, its
        // volume — vanish the moment you scrubbed off it, which is why "there's
        // no Start-frame control" was the report. So: if the keyframe under the
        // playhead carries a sound, edit that; otherwise show the sound anywhere
        // on the layer (an imported clip is on its own layer's frame 1), and
        // edit it where it actually is. Only when the layer has no sound at all
        // does the frame-under-the-playhead decide what the panel can offer.
        let (current, sound_frame, on_keyframe) =
            match layer.and_then(|id| editor.doc.scene().stage_layers().get(id)) {
                Some(l) => {
                    let kfs = l.frames.keyframes();
                    let here = kfs.iter().find(|k| k.start == frame);
                    if let Some(k) = here.filter(|k| k.sound.is_some()) {
                        (k.sound, frame, true)
                    } else if let Some(k) = kfs.iter().find(|k| k.sound.is_some()) {
                        (k.sound, k.start, true)
                    } else if here.is_some() {
                        (None, frame, true)
                    } else {
                        (None, frame, false)
                    }
                }
                None => (None, frame, false),
            };

        let response = buzz_ui::sound_panel(ui, &library, current, on_keyframe, sound_frame);

        if let Some(reference) = response.set
            && let Some(layer) = layer
        {
            editor.doc.edit("Sound Settings", |scene| {
                scene.set_frame_sound(layer, sound_frame, reference);
            });
            editor.doc.end_gesture();
            let scene = editor.doc.scene().clone();
            editor.sound.refresh(&scene);
        }

        if let Some(new_start) = response.move_to
            && let Some(layer) = layer
        {
            editor.set_sound_start(layer, sound_frame, new_start);
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
            match self.light_detail() {
                buzz_render::document::LightDetail::Full => None,
                buzz_render::document::LightDetail::NoModelling => {
                    Some("too dense to model — colour and shadows only")
                }
                buzz_render::document::LightDetail::Flat => {
                    Some("too dense to model or shadow — colour only")
                }
                buzz_render::document::LightDetail::Unsealed => {
                    Some("too dense to model, shadow or seal seams — colour only")
                }
            }
        };
        let current_frame = self.editor.current_frame;
        let editor = &mut self.editor;
        let state = &mut editor.light_panel;
        state.trimmed = trimmed;
        state.current_frame = current_frame;
        // Keyed, not base: an animated light shows (and is edited) as it
        // actually looks at the playhead, not wherever its inert base sits.
        let keyed = editor.doc.scene().lights().keyed_at(current_frame);
        let response = buzz_ui::light_panel(ui, &keyed, state);

        if let Some(id) = response.select {
            editor.light_panel.selected = Some(id);
            editor.set_tool(ToolId::Selection);
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

        if response.add_storm {
            editor.run(Command::AddStorm);
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
            // An animated light keys the playhead instead of the (inert)
            // base; with Auto Keyframe on, an un-keyed light starts a track
            // instead — the same rule a stage drag follows.
            let frame = current_frame;
            let auto_key = editor.auto_keyframe;
            editor.doc.edit("Light Settings", |scene| {
                crate::lights::apply_light_edit(scene, light, frame, auto_key);
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

        if let Some(edges) = response.set_edges {
            editor.doc.edit("Light Edges", |scene| {
                scene.lights_mut().edges = edges;
            });
        }

        if let Some(model) = response.set_model {
            editor.doc.edit("Light Model", |scene| {
                scene.lights_mut().model = model;
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

        // The Default limits table starts as the saved one.
        if self.editor.rig_panel.default_limits.is_none() {
            self.editor.rig_panel.default_limits = Some(crate::rig_limits::load_or_default());
        }
        // **Convert to bone rig** is offered when the selection is a part of
        // a layer rig.
        let layer_rig = self.selected_layer_rig();
        self.editor.rig_panel.layer_rig = layer_rig.is_some();

        let response = buzz_ui::rig_panel(
            ui,
            armature.as_ref(),
            &poses,
            &parts,
            bound
                .as_ref()
                .map(|(name, filled)| (name.as_str(), filled.as_slice())),
            &mut self.editor.rig_panel,
            &mut self.editor.workspace.rig,
        );

        // -- assembling, which does not need anything to be selected --------

        if let Some((pattern, slots)) = response.build_rig {
            self.rig_character(&pattern, &slots);
        }
        if let Some(id) = response.select_part {
            self.editor.selection.set([id]);
        }
        if response.convert_to_bones
            && let Some(layers) = layer_rig
        {
            self.convert_to_bone_rig(&layers);
        }
        if response.convert_to_layers
            && let Some(object) = selected
        {
            self.convert_to_layer_rig(object);
        }

        if let Some(table) = response.save_default_limits {
            match crate::rig_limits::save(&table) {
                Ok(()) => {
                    self.editor.status = Some("Saved the default joint limits.".into());
                    // The AI character wizard rigs with them from now on too.
                    self.editor.ai_character.rig_limits = Some(table);
                }
                Err(e) => {
                    self.editor.status = Some(format!("Could not save the joint limits: {e}"));
                }
            }
        }

        let Some(object) = selected else { return };

        if let Some(table) = response.apply_default_limits {
            self.editor.doc.edit("Default Limits to Rig", |scene| {
                scene.update_object(object, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                        rigging::apply_default_limits(&mut rig.armature, &table);
                    }
                });
            });
        }

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

        if let Some(edit) = response.apply_limits {
            self.apply_joint_edit(object, edit);
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
            self.editor.rig_reset_pose(object);
        }

        if response.set_rest_pose {
            // Adopting a new rest pose changes what the weights were bound
            // against, so the artwork has to be re-bound — otherwise it would
            // keep bending about the old skeleton.
            self.editor.rig_set_rest_pose(object);
        }

        // -- the pose library ---------------------------------------------

        if let Some(name) = response.save_pose {
            self.editor.rig_save_pose(object, name);
        }

        if response.mirror_pose {
            self.editor.rig_mirror_pose(object);
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
            self.editor.rig_delete_bone(object, bone);
        }

        if let Some((bone, parent)) = response.reparent {
            self.editor.rig_reparent_bone(object, bone, parent);
        }

        if let Some(index) = response.delete_pose {
            self.editor.rig_delete_pose(object, index);
        }
    }

    /// The AI character wizard rigs with the saved joint limits: read once,
    /// and again whenever the Rig panel saves new ones.
    fn ensure_ai_rig_limits(&mut self) {
        if self.editor.ai_character.rig_limits.is_none() {
            self.editor.ai_character.rig_limits = Some(crate::rig_limits::load_or_default());
        }
    }

    /// A joint-limit edit from the joint widget — the Rig panel's or the Bone
    /// tool's Tool Options — as one undo step, however many bones it reaches.
    fn apply_joint_edit(
        &mut self,
        object: buzz_scene::ObjectId,
        edit: buzz_ui::joint_widget::JointEdit,
    ) {
        use buzz_ui::joint_widget::JointEdit;
        let label = match edit {
            JointEdit::Set { .. } => "Joint Limits",
            JointEdit::ApplyToChain { .. } => "Joint Limits to Chain",
            JointEdit::ApplyToAll { .. } => "Joint Limits to All Bones",
        };
        self.editor.doc.edit(label, |scene| {
            scene.update_object(object, |target| {
                if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                    rigging::apply_joint_edit(&mut rig.armature, &edit);
                }
            });
        });
    }

    /// The Bone tool's part of Tool Options: the joint widget for the bone
    /// last pressed on, or — with a rig selected but no bone — a hint and the
    /// Apply-to-all row. Nothing when no rig is selected.
    fn bone_tool_joint_options(&mut self, ui: &mut egui::Ui) {
        use buzz_ui::joint_widget;

        let rig = {
            let selection = &self.editor.selection;
            let scene = self.editor.doc.scene();
            (selection.len() == 1)
                .then(|| selection.iter().next())
                .flatten()
                .filter(|id| {
                    scene.find_object(*id).is_some_and(|(_, o)| {
                        matches!(o.kind, buzz_scene::ObjectKind::Armature(_))
                    })
                })
        };
        let Some(rig) = rig else {
            return;
        };

        ui.separator();
        let selected = self.editor.selected_bone.filter(|(object, _)| *object == rig);
        let bone = rigging::selected_bone(self.editor.doc.scene(), selected);
        let edit = match (selected, bone) {
            (Some((_, index)), Some(bone)) => {
                ui.label(egui::RichText::new(format!("Joint limits \u{2014} {}", bone.name)).strong());
                joint_widget::joint_limits(ui, &bone, index)
            }
            _ => {
                ui.label(egui::RichText::new("Click a bone to set its limits").weak());
                let key = egui::Id::new("bone-tool-apply-to-all-spec");
                let mut spec: joint_widget::LimitSpec = ui.data_mut(|d| {
                    d.get_temp(key).unwrap_or((
                        buzz_rig::RotationMode::Free,
                        buzz_rig::Direction::Both,
                        0.0,
                        0.0,
                    ))
                });
                let mut edit = None;
                ui.push_id("bone-tool-apply-to-all", |ui| {
                    ui.horizontal(|ui| {
                        joint_widget::mode_buttons(ui, &mut spec);
                    });
                    if spec.0 == buzz_rig::RotationMode::Limited {
                        ui.horizontal(|ui| {
                            joint_widget::direction_buttons(ui, &mut spec);
                        });
                        ui.horizontal(|ui| {
                            joint_widget::range_values(ui, &mut spec);
                        });
                    }
                    if ui
                        .small_button("Apply to all bones")
                        .on_hover_text(
                            "Give every bone of this rig the same rotation, direction and range, \
                             each measured from the angle that bone was drawn at",
                        )
                        .clicked()
                    {
                        edit = Some(joint_widget::JointEdit::ApplyToAll {
                            limits: joint_widget::relative_limits(spec),
                        });
                    }
                });
                ui.data_mut(|d| d.insert_temp(key, spec));
                edit
            }
        };
        if let Some(edit) = edit {
            self.apply_joint_edit(rig, edit);
        }
    }

    /// Put a rig into one of its saved poses, optionally keying it first.
    fn apply_named_pose(&mut self, object: buzz_scene::ObjectId, index: usize, key: bool) {
        self.editor.apply_named_pose(object, index, key);
    }

    // -- rigging a character by sorting its drawings into slots ------------

    /// Build a skeleton from a pattern and move the sorted drawings into it.
    ///
    /// One undo step for the whole character. The work is in
    /// [`rigging::rig_character`]; what is here is the report, because the
    /// document layer has nowhere to say anything.
    fn rig_character(&mut self, pattern_name: &str, slots: &[Option<buzz_scene::ObjectId>]) {
        if buzz_rig::RigPattern::named(pattern_name).is_none() {
            self.editor.status = Some(format!("No rig pattern called {pattern_name}."));
            return;
        }
        // The saved joint limits (`rig_limits.json`), not only the built-in ones.
        let pattern = crate::rig_limits::pattern_with_saved_limits(pattern_name);
        let frame = self.editor.frame();
        let count = slots.iter().filter(|s| s.is_some()).count();
        let mut built = None;

        if self.editor.rig_panel.kind == buzz_rig::RigKind::Layers {
            let mut layered = None;
            self.editor.doc.edit("Rig Character (Layers)", |scene| {
                layered = rigging::rig_character_layers(scene, frame, &pattern, slots);
            });
            match layered {
                Some(rig) => {
                    self.editor.rig_panel.armed = None;
                    self.editor.rig_panel.note = None;
                    let parts = self.layer_rig_parts(&rig.layers);
                    self.editor.selection.set(parts);
                    self.editor.status = Some(format!(
                        "Rigged {} part{} as a {} by layer parenting.",
                        rig.layers.len(),
                        if rig.layers.len() == 1 { "" } else { "s" },
                        pattern.name
                    ));
                }
                None => {
                    self.editor.status =
                        Some("Nothing to rig: put a drawing in at least one slot first.".into());
                }
            }
            return;
        }

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

    /// The first object on each of a layer rig's layers at the playhead.
    fn layer_rig_parts(&self, layers: &[(usize, buzz_scene::LayerId)]) -> Vec<buzz_scene::ObjectId> {
        let frame = self.editor.frame();
        let scene = self.editor.scene();
        layers
            .iter()
            .filter_map(|(_, id)| {
                scene
                    .layers()
                    .get(*id)
                    .and_then(|l| l.objects_at(frame).first().map(|o| o.id))
            })
            .collect()
    }

    /// The layer rig the selection is part of — the whole follow tree of the
    /// selected object's layer — or `None` when it is not one (or is a bone
    /// rig, which converts the other way).
    fn selected_layer_rig(&self) -> Option<Vec<buzz_scene::LayerId>> {
        let id = self.editor.selection.iter().next()?;
        let (layer, object) = self.editor.scene().find_object(id)?;
        if matches!(object.kind, buzz_scene::ObjectKind::Armature(_)) {
            return None;
        }
        rigging::layer_rig_of(self.editor.scene(), layer)
    }

    /// Whether the selected object's layer has layers following it — a part
    /// of a layer rig that carries others when it turns.
    fn selection_has_followers(&self) -> bool {
        let Some(id) = self.editor.selection.iter().next() else {
            return false;
        };
        let scene = self.editor.scene();
        scene
            .find_object(id)
            .is_some_and(|(layer, _)| !scene.layers().followers_of(layer).is_empty())
    }

    /// **Convert to layer rig**: one undo step, or a status message and no
    /// change at all when the rig cannot be taken apart (skinned artwork).
    fn convert_to_layer_rig(&mut self, rig: buzz_scene::ObjectId) {
        let frame = self.editor.frame();
        let mut built = None;
        self.editor.doc.edit("Convert to Layer Rig", |scene| {
            built = rigging::bones_to_layers(scene, frame, rig);
        });
        match built {
            Some(layers) => {
                let parts = self.layer_rig_parts(&layers.layers);
                self.editor.selection.set(parts);
                self.editor.status = Some(format!(
                    "Converted to a layer rig: {} layer{}.",
                    layers.layers.len(),
                    if layers.layers.len() == 1 { "" } else { "s" }
                ));
            }
            None => {
                self.editor.status = Some(
                    "Cannot convert: this rig has skinned artwork that bends at the joints, \
                     which cannot be one layer per bone. Nothing changed."
                        .into(),
                );
            }
        }
    }

    /// **Convert to bone rig**: the layers' names read back as slots of the
    /// best-matching pattern, one undo step.
    fn convert_to_bone_rig(&mut self, layers: &[buzz_scene::LayerId]) {
        let Some(pattern) = rigging::pattern_for_layers(self.editor.scene(), layers) else {
            self.editor.status = Some(
                "Cannot convert: no layer is named after a rig slot. Nothing changed.".into(),
            );
            return;
        };
        let pattern = crate::rig_limits::pattern_with_saved_limits(&pattern.name);
        let frame = self.editor.frame();
        let mut built = None;
        self.editor.doc.edit("Convert to Bone Rig", |scene| {
            built = rigging::layers_to_bones(scene, frame, &pattern, layers);
        });
        match built {
            Some(id) => {
                self.editor.selection.set([id]);
                self.editor.status = Some(format!("Converted to a {} bone rig.", pattern.name));
            }
            None => {
                self.editor.status = Some("Cannot convert: nothing to rig. Nothing changed.".into());
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
    fn scene_crumb(
        &mut self,
        ui: &mut egui::Ui,
        command: &mut Option<Command>,
        inside_symbol: bool,
    ) {
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
                self.editor
                    .doc
                    .rename_scene(index, buffer.trim().to_string());
            } else {
                self.scene_rename = Some((index, buffer));
            }
            return;
        }

        let active = self.editor.doc.active_scene();
        let current = self
            .editor
            .doc
            .scene_names()
            .get(active)
            .cloned()
            .unwrap_or_else(|| "Scene 1".to_string());

        // Inside a symbol the scene name is a link straight back to the main
        // timeline — click the root to leave the symbol, however deep in you are
        // — with the scene menu folded onto a caret beside it. At the main
        // timeline there is nowhere further out to go, so the name *is* the menu.
        if inside_symbol {
            if ui
                .link(egui::RichText::new(&current).small())
                .on_hover_text("Back to this scene's main timeline")
                .clicked()
            {
                *command = Some(Command::EditDocument);
            }
            ui.menu_button(egui::RichText::new("\u{23F7}").small(), |ui| {
                self.scene_menu(ui, command);
            })
            .response
            .on_hover_text("Switch, add, rename or delete a scene");
        } else {
            ui.menu_button(egui::RichText::new(&current).small(), |ui| {
                self.scene_menu(ui, command);
            })
            .response
            .on_hover_text("Switch, add, rename or delete a scene");
        }
    }

    /// The scene menu — switch, add, duplicate, rename, reorder, delete — folded
    /// behind the scene crumb. Split out so the crumb can present it under the
    /// name at the main timeline and under a caret while a symbol is open.
    fn scene_menu(&mut self, ui: &mut egui::Ui, command: &mut Option<Command>) {
        let active = self.editor.doc.active_scene();
        let names = self.editor.doc.scene_names();

        for (i, name) in names.iter().enumerate() {
            // Setup and Shot scenes get a short marker so the running order
            // still reads at a glance once a sequence is in it — see
            // `docs/SEQUENCE_SETUPS_PLAN.md` S5.
            let label = self.editor.scene_role_label(i).unwrap_or_else(|| name.clone());
            if ui
                .selectable_label(i == active, egui::RichText::new(label).small())
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
    }

    fn breadcrumb(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let mut command = None;
        let path: Vec<buzz_scene::SymbolId> = self.editor.scene().edit_path().to_vec();

        ui.horizontal(|ui| {
            self.scene_crumb(ui, &mut command, !path.is_empty());

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
                    // Clear the stage selection first: opening the selected
                    // symbol prefers a chosen instance's symbol over the Library
                    // selection (see `edit_selected_symbol`), so without this a
                    // crumb click could open the selected instance rather than
                    // the ancestor it names. Re-entering a symbol already on the
                    // path steps the edit stack back out to it.
                    self.editor.selection.clear();
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
        if let Some(layers) = response.select_layers {
            self.editor.select_layers(&layers);
            self.editor.camera_selected = false;
        }
        if let Some(anchor) = response.set_range_anchor {
            // I1: Ctrl/Cmd+click moves the anchor a later Shift+click sweeps
            // from; a plain click already moved it through `select_layer`
            // above, so this only matters for the toggle case.
            self.editor.set_range_anchor(anchor);
        }
        if let Some(layer) = response.toggle_collapsed {
            // I2: the disclosure triangle — undoable, matching how the
            // eye/lock/outline switches are applied just below, so opening
            // and closing a folder is a thing the undo history can say and
            // step back through like anything else.
            self.editor.doc.edit("Expand/Collapse Folder", |scene| {
                scene.update_layer(layer, |l| l.collapsed = !l.collapsed);
            });
        }
        if let Some(selection) = response.select_cells {
            self.editor.selection.set_timeline(selection);
            self.editor.camera_selected = false;
        }
        if let Some(m) = response.move_frames {
            // M3/M6: the frame selection was dragged — along its rows, onto
            // other layers, or both; Alt copies. One undo step, and the same
            // command the agent runs.
            self.editor.run(buzz_ui::Command::MoveFrames {
                to: m.to,
                rows: m.rows,
                copy: m.copy,
            });
        }
        if let Some((layer, to_index)) = response.reorder_layer {
            self.editor.reorder_layer(layer, to_index);
        }
        if let Some((layer, to_index, parent)) = response.move_layer {
            // F1: the timeline's own drag — a layer, and everything filed
            // under it, to a new row and parent folder — one undo step.
            self.editor.move_layer_to(layer, to_index, parent);
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
                let on = |l: &buzz_scene::Layer| match icon {
                    LayerIcon::Eye => l.visible,
                    LayerIcon::Lock => l.locked,
                    LayerIcon::Outline => l.outline,
                };
                let Some(layer_ref) = scene.layers().get(layer) else {
                    return;
                };
                let target = !on(layer_ref);
                // F2: a folder's own switch cascades to everything it holds,
                // at any depth — one undo step, so hiding a folder does not
                // take fifty.
                let mut ids = vec![layer];
                if layer_ref.kind == buzz_scene::LayerKind::Folder {
                    ids.extend(scene.layers().descendants_of(layer));
                }
                for id in ids {
                    scene.update_layer(id, |l| match icon {
                        LayerIcon::Eye => l.visible = target,
                        LayerIcon::Lock => l.locked = target,
                        LayerIcon::Outline => l.outline = target,
                    });
                }
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
        if let Some((layers, color)) = response.set_highlight {
            // H2: Animate's Highlight Layer, from either menu on the layer's
            // name. One undo step regardless of how many layers it reaches —
            // "Highlight Selected Layers" is one act, not one per layer.
            self.editor.doc.edit("Highlight Layer", |scene| {
                for id in layers {
                    scene.update_layer(id, |l| l.highlight = color);
                }
            });
        }
        if response.select_camera {
            self.editor.camera_selected = true;
            // Selecting the camera row selects the Camera tool, which is what
            // Animate does — the row and the tool are the same idea.
            commands.push(Command::SelectTool(ToolId::Camera));
        }
        if response.toggle_camera {
            // C1: the camera row's own eye — the same command the layer-tools
            // strip's camera button raises, so the two doors open one room.
            commands.push(Command::ToggleCamera);
        }
        if response.toggle_camera_lock {
            self.editor.doc.edit("Lock Camera", |scene| {
                let locked = !scene.camera().locked;
                scene.camera_mut().locked = locked;
            });
        }
        if let Some(id) = response.select_light {
            // Clicking a light's channel makes it the active light, the way
            // clicking any row takes the highlight from the one before.
            self.editor.light_panel.selected = Some(id);
            self.editor.camera_selected = false;
            self.editor.set_tool(ToolId::Selection);
        }
        if let Some((id, above)) = response.place_light {
            self.editor.place_light(id, above);
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
                    handles.push(("split-right-outer", vertical(rect, rect.left()), true, -1.0));
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
                let room = self.dock_room;
                let workspace = &mut self.editor.workspace;
                // The same ranges the workspace clamps a loaded layout to, so a
                // column cannot be dragged to a width that the next launch
                // would silently undo — and, on top of those, what this window
                // has room for. A boundary dragged outwards squeezes the
                // columns beyond it and then stops, rather than storing a width
                // nothing can show and going dead on the way back.
                match name {
                    "split-left" => {
                        let cap = workspace.column_cap(buzz_ui::Dock::Left, room);
                        workspace.left_width =
                            clamp_to(workspace.left_width + moved, LEFT_WIDTH_RANGE).min(cap);
                    }
                    "split-right" => {
                        let cap = workspace.column_cap(buzz_ui::Dock::Right, room);
                        workspace.right_width =
                            clamp_to(workspace.right_width + moved, COLUMN_WIDTH_RANGE).min(cap);
                    }
                    "split-right-outer" => {
                        let cap = workspace.column_cap(buzz_ui::Dock::RightOuter, room);
                        workspace.right_outer_width =
                            clamp_to(workspace.right_outer_width + moved, COLUMN_WIDTH_RANGE)
                                .min(cap);
                    }
                    _ => {
                        workspace.bottom_height =
                            clamp_to(workspace.bottom_height + moved, BOTTOM_HEIGHT_RANGE);
                    }
                }
                changed = true;
            }
            if response.drag_stopped() {
                // What the window forced becomes the layout's own, so the
                // boundary stays where it was dropped instead of springing
                // back when the fit is recomputed without a column to favour.
                let dragged = match name {
                    "split-left" => Some(buzz_ui::Dock::Left),
                    "split-right" => Some(buzz_ui::Dock::Right),
                    "split-right-outer" => Some(buzz_ui::Dock::RightOuter),
                    _ => None,
                };
                if dragged.is_some() {
                    self.editor
                        .workspace
                        .commit_column_widths(self.dock_room, dragged);
                }
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
            // **Except that Free Transform says where it rotates.**
            //
            // The ring just outside a corner turns the selection, and with no
            // cursor change nothing distinguished it from the empty stage
            // beside it — where the same drag marquees and throws the selection
            // away. Grab is the nearest thing egui offers to Animate's curved
            // arrow, and it reads as "take hold of this and swing it".
            //
            // Only Free Transform has a gizmo to say anything about: the
            // Selection tool is Animate's arrow, with no handles to resize, turn
            // or skew by, so its pointer stays the arrow (and, over a raw
            // shape's own geometry, the corner and edge cursors below).
            if tool == FreeTransform
                && let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos())
                && area.contains(pos)
                && let Some(cursor) = self.transform_cursor(area, pos)
            {
                ui.ctx().set_cursor_icon(cursor);
            } else if tool == Selection
                && let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos())
                && area.contains(pos)
            {
                // **The arrow over a raw shape's own geometry.** A press on an
                // unselected shape's corner moves it and a press on its edge
                // bends it, so say so before the press: a crosshair over a
                // corner, a hand over an edge (egui has no arc cursor).
                let local =
                    buzz_geom::Point::new((pos.x - area.min.x) as f64, (pos.y - area.min.y) as f64);
                match self.editor.shape_grip_hover(local) {
                    Some(crate::editor::ShapeGrip::Corner) => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                    Some(crate::editor::ShapeGrip::Edge) => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    None => {}
                }
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
        // **Each tool's own size.** The eraser rubs with `eraser_size`
        // (`tools.rs`, `ToolAction::Erase`), not the pencil's stroke width —
        // the ring was drawn from the pencil's, so it matched the rub only
        // when the two settings happened to agree.
        // Whether that size is on the stage (grows with zoom, `true`) or fixed
        // on screen (Animate's "Zoom size with stage" switched off).
        let ring = match tool {
            Brush => Some((self.editor.style.brush.size, self.editor.style.brush_zoom_size)),
            Pencil => Some((self.editor.style.stroke_width.max(1.0), true)),
            Eraser => Some((
                self.editor.style.eraser_size.max(0.5),
                self.editor.style.eraser_zoom_size,
            )),
            _ => None,
        };
        if let Some((size, zoom_size)) = ring {
            let r = if zoom_size {
                // In screen pixels per edit unit *at the pointer*: the view's
                // zoom and the document camera's, since a mark is made in
                // edit space and drawn through both.
                let local =
                    buzz_geom::Point::new((pos.x - area.min.x) as f64, (pos.y - area.min.y) as f64);
                let scale = self.editor.edit_scale_on_screen(local);
                (size * scale * 0.5).clamp(2.5, 600.0) as f32
            } else {
                // Fixed on screen: the size itself is already the screen
                // diameter, at any zoom.
                (size * 0.5).clamp(2.5, 600.0) as f32
            };
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
        let icon =
            egui::Rect::from_min_size(egui::pos2(pos.x + 9.0, pos.y + 9.0), egui::vec2(18.0, 18.0));
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
    /// **Four transforms live within a few pixels of each other** — a corner
    /// resizes, the ring just outside it turns, the handle at the middle of an
    /// edge squeezes that dimension, the edge either side of it skews — and with one
    /// arrow over all of them the only way to find out which you had grabbed
    /// was to let go and look. Reaching for a rotation and landing on a corner
    /// is how a turn ends up scaling the artwork, and the pointer never said.
    ///
    /// Read against the same box the chrome draws and the same radii
    /// `tools::transform_zone` tests, so the cursor cannot promise a gesture
    /// the release will not make.
    fn transform_cursor(&self, area: egui::Rect, pos: egui::Pos2) -> Option<egui::CursorIcon> {
        let frame = self.editor.selection_box()?;
        let local = buzz_geom::Point::new((pos.x - area.min.x) as f64, (pos.y - area.min.y) as f64);
        if self.editor.pointer_on_pivot(local) {
            return Some(egui::CursorIcon::Grab);
        }
        let at = self.editor.screen_to_edit(local);
        let pivot = self.editor.pivot().unwrap_or_else(|| frame.center());
        let grab = crate::tools::TRANSFORM_GRAB_PX / self.editor.camera.zoom.max(f64::MIN_POSITIVE);

        // **The arrow points the way the handle does, on screen.** The box
        // turns with the selection, so "top-left" no longer means the screen's
        // north-west: the direction is measured from the box's centre to the
        // handle in screen space and snapped to the nearest of the four resize
        // arrows egui has. Without a projection there is nothing on screen to
        // point along, and the pointer is left as it is.
        let projection = self.editor.edit_to_stage();
        let on_screen = |p: buzz_geom::Point| {
            projection
                .map_point(p)
                .map(|p| self.editor.camera.doc_to_screen(p))
        };
        let centre = on_screen(frame.center());
        let arrow = |towards: Option<buzz_geom::Point>| {
            Some(resize_cursor_along(towards? - centre?))
        };
        let corners = frame.corners().map(on_screen);
        let edges = frame.mid_edges().map(on_screen);
        return match crate::tools::transform_zone(frame, pivot, at, grab) {
            crate::tools::TransformZone::Rotate | crate::tools::TransformZone::Pivot => {
                Some(egui::CursorIcon::Grab)
            }
            crate::tools::TransformZone::Corner => {
                let doc_corners = frame.corners();
                let (index, _) = doc_corners
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| (**a - at).hypot().total_cmp(&(**b - at).hypot()))?;
                arrow(corners[index])
            }
            // The top and bottom handles are `mid_edges()[0..2]`, the left and
            // right ones `[2..4]`; which of the pair is under the pointer does
            // not change the line the arrow lies along.
            crate::tools::TransformZone::Side(horizontal) => {
                arrow(edges[if horizontal { 0 } else { 2 }])
            }
            // A skew slides along the edge, so the arrow lies along it: the
            // box's top and bottom run along its own x axis, its sides along y.
            crate::tools::TransformZone::Edge(horizontal) => {
                let (from, to) = if horizontal {
                    (corners[0], corners[1])
                } else {
                    (corners[0], corners[3])
                };
                Some(resize_cursor_along(to? - from?))
            }
            crate::tools::TransformZone::Inside => None,
        };
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
                                .add(egui::Button::new("\u{2212}").min_size(egui::vec2(24.0, 24.0)))
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
                                .add(egui::Button::new("+").min_size(egui::vec2(24.0, 24.0)))
                                .on_hover_text("Zoom in (Ctrl+= or Ctrl+wheel)")
                                .clicked()
                            {
                                self.editor.zoom_by(WHEEL_ZOOM_STEP.powi(3));
                            }

                            // `⏷`, not `▾`: egui's bundled fonts have the first and
                            // not the second, which drew as an empty box in the very
                            // first screenshot of this control.
                            //
                            // `MenuButton::from_button`, not `ui.menu_button`: a
                            // lone glyph button defaults to a text-tight width —
                            // under the 24 px hit-target minimum (Stage 6, item
                            // 3) — and `min_size` is not something the plain
                            // `menu_button` helper exposes.
                            let (chevron, _) = egui::containers::menu::MenuButton::from_button(
                                egui::Button::new("\u{23F7}").min_size(egui::vec2(24.0, 24.0)),
                            )
                            .ui(ui, |ui| {
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
                            });
                            chevron.on_hover_text("Zoom presets");

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

    /// **The status strip**: zoom, its presets, the precision readout, the
    /// last status message and the frame's cost — at the right end of the
    /// menu row.
    ///
    /// It was a row of its own along the bottom of the window (Stage 2,
    /// item 5 of `docs/WORKSPACE_DESIGN_PLAN.md`); moved up beside the menu
    /// (user 2026-10-02) so the stage and the timeline get that row back.
    /// The document's name it led with is already in the title bar's drag
    /// zone, so it is not repeated here.
    ///
    /// Called inside a `right_to_left` layout, so it is laid out from the
    /// right edge inwards: the frame time last on screen, drawn first.
    fn status_strip(&mut self, ui: &mut egui::Ui) {
        ui.scope(|ui| {
            // "text_dim", set once so every label in the strip inherits it.
            ui.visuals_mut().override_text_color = Some(Palette::text_dim());

            // **A fixed-width slot**, as wide as "999.9 ms". The text changes
            // every frame, and the menu is sized to whatever this strip leaves
            // over (`bounded_menu`) — so a label sized to its own text moved
            // the menu's right edge, and Reset Layout pinned to it, on every
            // repaint: a visible shake whenever the pointer over the bar kept
            // frames coming.
            let font = egui::TextStyle::Small.resolve(ui.style());
            let slot = ui
                .painter()
                .layout_no_wrap("999.9 ms".to_owned(), font, Palette::text_dim())
                .size()
                .x;
            ui.add_sized(
                [slot, ui.available_height()],
                egui::Label::new(
                    egui::RichText::new(format!("{:.1} ms", self.frame_ms_display()))
                        .small()
                        .weak(),
                )
                .extend(),
            )
            // The per-section breakdown on hover — the frame watchdog's
            // numbers, without taking a permanent seat in the strip.
            .on_hover_text(self.profiler.summary());
            // BM: the Bone tool's mode, named while the tool is held.
            if self.editor.tool() == buzz_ui::ToolId::Bone {
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("Bone: {}", self.editor.workspace.rig.mode.label()))
                        .small(),
                );
            }
            if let Some(status) = &self.editor.status {
                ui.separator();
                // Capped, so a long message truncates rather than pushing
                // into the menu on its left.
                ui.allocate_ui_with_layout(
                    egui::vec2(280.0, ui.available_height()),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(status).small()).truncate())
                            .on_hover_text(status.as_str());
                    },
                );
            }

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
            ui.separator();

            // Zoom, with Animate's presets plus an unbounded field — in
            // reverse, since this layout runs right to left.
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
            ui.label("Zoom");
        });
    }

    /// **This frame's honest cost**: the profiler's own sum of sections —
    /// UI, encode, lights, present — not `active.frame_ms`, which is the
    /// wall-clock gap since the previous frame and so includes however long
    /// the window sat idle on `ControlFlow::Wait` between them. A document
    /// that draws in 8 ms but is only asked to every two seconds would show
    /// "2000 ms" forever under the old readout — alarming, and wrong about
    /// exactly the number this status bar exists to report. `active.frame_ms`
    /// itself is untouched: playback still reads it for `elapsed`.
    fn frame_ms_display(&self) -> f32 {
        self.profiler.last().total().as_secs_f32() * 1000.0
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
            item(ui, Command::BreakApart, has_selection);
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

        if let Some(command) = raised
            && self.user_command_allowed(command)
        {
            self.dispatch(command);
        }
        if save_asset && !self.agent_active() {
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

        // **An asset dragged out of the Assets panel, dropped where it lands.**
        //
        // The same as a Library symbol above, and here for the same reasons:
        // the release that ends the drag must place the asset rather than fall
        // through to the tools and deselect what was under it, and it lands
        // where it is let go rather than at the coordinates it was saved with.
        // Dropping on a folder in the panel still files it; only a drop over
        // the stage places it.
        if egui::DragAndDrop::has_payload_of_type::<buzz_ui::assets_panel::DraggedAsset>(&ctx) {
            let over = ctx
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| area.contains(p));
            if over {
                ctx.set_cursor_icon(egui::CursorIcon::Copy);
                // A ghost of where it will land, so the drop is aimed.
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
                    egui::DragAndDrop::take_payload::<buzz_ui::assets_panel::DraggedAsset>(&ctx)
                    && let Some(p) = ctx.input(|i| i.pointer.interact_pos())
                {
                    self.editor.place_asset_at(&dropped.0, local(p));
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
            // **Where the press actually landed, not wherever the pointer has
            // since drifted to.** A widget that also senses clicks (this one
            // does — `Sense::click_and_drag()`) only turns `drag_started()`
            // true once the pointer has moved past egui's own click/drag
            // threshold (`max_click_dist`, 6 points), and by then
            // `interact_pos()` is the *current* position, several pixels
            // beyond wherever the button actually went down. The Free
            // Transform gizmo's handles are themselves only `TRANSFORM_GRAB_PX`
            // (8) screen pixels wide, so that drift alone was enough to carry
            // a press that started dead on a corner handle past it before the
            // editor ever heard about the gesture — read as a plain drag
            // inside the selection instead of a grab of the handle, which is
            // exactly "once it resizes once, it just moves". `press_origin`
            // is the pixel egui recorded when the button first went down,
            // unaffected by the click/drag disambiguation delay.
            && let Some(pos) = ctx.input(|i| i.pointer.press_origin())
        {
            if pan_override {
                // Seeded from the actual press position rather than
                // `interact_pos()` — see `pan_last_screen` — so the pan below
                // picks up the motion from the moment the button went down,
                // not from wherever egui's click/drag threshold let the
                // pointer drift to before this fired.
                self.pan_last_screen = Some(local(pos));
            } else {
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
                // **Every place the pointer went this frame, not just where
                // it ended up** — the same reason the brush path below walks
                // `events` instead of taking `interact_pos` alone: several
                // moves can arrive before this runs once. Read as a single
                // `ctx.input(|i| i.pointer.delta())` per frame, this was
                // *also* only ever applied on frames egui already calls a
                // drag, so a click/drag threshold's worth of motion (up to
                // `max_click_dist`, 6 points) was silently dropped at the
                // start of every Space or middle-button pan. Tracking our
                // own last position — seeded from `press_origin` above —
                // fixes both at once.
                let speed = self.editor.workspace.pan_speed as f64;
                let moves: Vec<egui::Pos2> = ctx.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::PointerMoved(p) => Some(*p),
                            _ => None,
                        })
                        .collect()
                });
                let targets: &[egui::Pos2] = if moves.is_empty() {
                    std::slice::from_ref(&pos)
                } else {
                    &moves
                };
                for &p in targets {
                    let cur = local(p);
                    let prev = self.pan_last_screen.unwrap_or(cur);
                    let delta = cur - prev;
                    if delta.hypot() > 0.0 {
                        self.editor.apply(ToolAction::PanView {
                            delta_screen: delta * speed,
                        });
                    }
                    self.pan_last_screen = Some(cur);
                }
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
            self.pan_last_screen = None;
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

    /// The editor, for the AI agent's executor (`crate::agent`).
    pub(crate) fn agent_editor(&self) -> &Editor {
        &self.editor
    }

    /// The mutable editor, for the AI agent's executor (`crate::agent`).
    pub(crate) fn agent_editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    pub(crate) fn dispatch(&mut self, command: Command) {
        match command {
            Command::ImportSound => self.import_sound_dialog(),
            Command::SetMasterAudio => self.set_master_audio_dialog(),
            Command::ImportCaptions => self.ask_for_path(
                crate::dialogs::Request::open_file().filter("Subtitles", &["srt", "vtt"]),
                Pick::ImportCaptions,
            ),
            Command::ExportCaptions => self.ask_for_path(
                crate::dialogs::Request::save_file()
                    .filter("Subtitles", &["srt"])
                    .file_name("captions.srt"),
                Pick::ExportCaptions,
            ),
            Command::ImportVideoReference => self.video_reference_dialog(),
            Command::ImportSequenceFolder => self.sequence_folder_dialog(),
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
            Command::VoicePushToTalk => self.voice_apply(buzz_ai::voice::VoiceInput::PushPress),
            Command::VoiceMute => self.voice_toggle_mute(),
            Command::SaveSnapshot => self.save_snapshot(),
            Command::Snapshots => {
                self.snapshots.rescan();
                self.show_snapshots = true;
            }
            Command::ShortcutEditor => self.shortcut_editor.open(),
            Command::PasteboardColour => self.show_pasteboard_dialog = true,
            Command::TestMovie => self.toggle_test_movie(),
            // The VLM call and the LLM call are both seconds of network I/O —
            // too slow for the UI thread, and a file dialog besides for the
            // first — so both are intercepted here rather than reaching
            // `Editor::run`. See `ai_scene_from_image_dialog` and
            // `ai_direct_story_async`.
            Command::AiSceneFromImage => {
                self.editor.workspace.reveal(buzz_ui::PanelId::Ai);
                self.ai_scene_from_image_dialog()
            }
            Command::AiDirectStory => {
                self.editor.workspace.reveal(buzz_ui::PanelId::Ai);
                self.ai_direct_story_async()
            }
            // Auto-rigging renders the symbol on the GPU and runs pose
            // detection on it — both too slow for the UI thread, and the GPU
            // render needs its own `Exporter`, which `Editor` has no way to
            // build. See `ai_rig_vector_character_async`.
            Command::AiRigVectorCharacter => {
                self.editor.workspace.reveal(buzz_ui::PanelId::Ai);
                self.ai_rig_vector_character_async()
            }
            // The other apps of the estate (`crate::estate`): a dialog that
            // asks them what there is, then a flow off the UI thread.
            Command::SendToBuzzEdit => self.estate_open_dialog(crate::estate::DialogKind::Send),
            Command::TranscribeWithBuzzEdit => self.estate_open_dialog(crate::estate::DialogKind::Transcribe),
            Command::ImportScriptFromStudio => self.estate_open_dialog(crate::estate::DialogKind::Studio),
            Command::ProduceEpisode => self.produce.open = true,
            other => self.editor.run(other),
        }
    }

    /// **Test Movie**: open the whole-film overlay, or close it if it is
    /// already open — Ctrl+Enter outside the Actions code editor.
    fn toggle_test_movie(&mut self) {
        if let Some(movie) = self.test_movie.take() {
            self.editor.sound.stop();
            self.test_movie_retired.extend(movie.close());
            return;
        }

        let scenes = self.editor.doc.film_with_lengths();
        // `active_index` is which scene plays this scene's sound: the
        // position the scene being edited lands at in film order (a
        // sequence's setups and inactive takes have no such position, and
        // fall back to the top of the film).
        let order = self.editor.doc.film_order();
        let active_index = order
            .iter()
            .position(|&i| i == self.editor.doc.active_scene())
            .unwrap_or(0);
        let Some(movie) = crate::test_movie::TestMovie::open(scenes, active_index) else {
            return;
        };

        // Test Movie owns the transport while it is open: the editor's own
        // playback stops, and the sound that was following it is replaced by
        // the active scene's own sound, restarted from the top so the
        // picture and the dialogue both begin at frame zero together.
        self.editor.playback.playing = false;
        self.editor.sound.stop();
        self.editor.sound.play(self.editor.doc.scene(), 0);
        self.test_movie = Some(movie);
    }

    /// Keep the document as it is now, under a name built from the document and
    /// the time, so a later Snapshots ▸ Restore can bring it back.
    fn save_snapshot(&mut self) {
        let base = self.editor.doc.title();
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
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
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
                    self.editor
                        .doc
                        .edit("Restore Snapshot", |s| *s = scene.clone());
                    self.editor.doc.end_gesture();
                    self.editor.status = Some("Snapshot restored".into());
                    self.show_snapshots = false;
                }
                Err(e) => {
                    self.editor.status = Some(format!("Could not restore that snapshot: {e}"))
                }
            }
        }
        if !open {
            self.show_snapshots = false;
        }
    }

    /// View > Pasteboard Colour…: "Match theme" (the default, `None`) or a
    /// picked colour, written straight into `Workspace::pasteboard` — see
    /// its own doc comment. Persisted with the rest of the workspace, so it
    /// survives a restart the same way the theme does.
    fn pasteboard_colour_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_pasteboard_dialog {
            return;
        }
        let mut open = true;
        let mut match_theme = self.editor.workspace.pasteboard.is_none();
        // The picker needs a colour to start from even while "Match theme"
        // is selected, so switching to "Custom colour" does not open on
        // black — the theme's own pasteboard is the obvious starting point.
        let mut colour = self.editor.workspace.pasteboard.unwrap_or_else(|| {
            let c = buzz_ui::Palette::pasteboard();
            [c.r(), c.g(), c.b(), 255]
        });
        egui::Window::new("Pasteboard Colour")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.radio_value(&mut match_theme, true, "Match theme");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut match_theme, false, "Custom colour");
                    ui.add_enabled_ui(!match_theme, |ui| {
                        ui.color_edit_button_srgba_unmultiplied(&mut colour);
                    });
                });
            });
        self.editor.workspace.pasteboard = if match_theme { None } else { Some(colour) };
        if !open {
            self.show_pasteboard_dialog = false;
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
                // What the timeline being edited already holds at this frame, so
                // that afterwards the placed artwork can be told apart from it
                // and selected — the same before/after diff Paste uses.
                let frame = self.editor.current_frame;
                let before: std::collections::HashSet<buzz_scene::ObjectId> = self
                    .editor
                    .doc
                    .scene()
                    .layers()
                    .iter()
                    .flat_map(|l| l.objects_at(frame).iter().map(|o| o.id))
                    .collect();
                let mut outcome = None;
                self.editor.doc.edit("Place Asset", |scene| {
                    outcome = Some(library.place(&asset, scene));
                });
                match outcome {
                    Some(Ok(report)) => {
                        // The placed artwork is what the user now wants to move,
                        // exactly as it is after an import — so it arrives
                        // selected, with the Free Transform handles already on
                        // it, rather than dropped and deselected.
                        let arrived: Vec<buzz_scene::ObjectId> = self
                            .editor
                            .doc
                            .scene()
                            .layers()
                            .iter()
                            .flat_map(|l| l.objects_at(frame).iter().map(|o| o.id))
                            .filter(|id| !before.contains(id))
                            .collect();
                        self.editor.selection.set(arrived);
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
                        // **Named as part of keeping it.** It used to become
                        // "Asset", then "Asset 2", with the only way to say
                        // otherwise being to find it afterwards and rename it —
                        // and an asset library outlives the document, so a
                        // shelf of "Asset 7"s is worse than a library of
                        // "Symbol 7"s. The name is a focused field the moment
                        // it lands: type over it, or press Enter and keep it.
                        self.editor
                            .assets_panel
                            .start_naming(saved.path.clone(), saved.name.clone());
                        self.editor.workspace.select_tab(buzz_ui::PanelId::Assets);
                        self.editor.status = Some(format!(
                            "Kept {} in Assets \u{2014} type its name",
                            saved.label()
                        ));
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

            MoveToFolder { asset, folder } => {
                let name = asset.name.clone();
                match self.editor.assets.move_to_folder(&asset, &folder) {
                    Ok(()) => {
                        let where_to = if folder.is_empty() {
                            "the top level".to_string()
                        } else {
                            folder.clone()
                        };
                        self.editor.status = Some(format!("Filed {name} in {where_to}"));
                    }
                    Err(e) => {
                        self.editor.status = Some(format!("Could not move {name}: {e}"));
                    }
                }
            }

            Delete(asset) => {
                // **No undo out here.** An asset is a file on disk, so the first
                // Delete arms and says so, and the second — on the "Delete?"
                // button the panel now shows — removes it.
                if self.editor.assets_panel.confirm_delete_asset.as_deref()
                    != Some(asset.path.as_path())
                {
                    self.editor.assets_panel.confirm_delete_asset = Some(asset.path.clone());
                    self.editor.status = Some(format!(
                        "Delete the asset {}? It cannot be undone — click Delete again to confirm",
                        asset.name
                    ));
                } else {
                    self.editor.assets_panel.confirm_delete_asset = None;
                    match self.editor.assets.delete(&asset) {
                        Ok(()) => {
                            self.editor.status = Some(format!("Deleted {}", asset.name));
                        }
                        Err(e) => self.editor.status = Some(format!("Could not delete: {e}")),
                    }
                    self.asset_thumbnails.forget();
                }
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

            ArrangeByTypeAndStyle => {
                self.arrange.open = true;
                self.refresh_arrange_undo();
                if self.arrange_task.is_none() {
                    self.start_arrange_plan();
                }
            }
        }
    }

    /// How many files the last arrange moved, for the dialog's Undo button.
    fn refresh_arrange_undo(&mut self) {
        self.arrange.undo_files = self
            .editor
            .assets
            .root()
            .map(|root| {
                buzz_ai::asset_index::MoveLogs::load(root)
                    .logs
                    .last()
                    .map(|l| l.moved.len())
                    .unwrap_or(0)
            })
            .unwrap_or(0);
    }

    /// **Index the library and plan the arrange** on a Tasks-panel task (AS1
    /// + AS2): first with name heuristics and SigLIP (new and changed assets
    /// only), which gives a plan at once; then, when a vision model is set
    /// in AI ▸ Models, the VLM over every record still guessed, the style
    /// clusters named, and the plan sent again.
    fn start_arrange_plan(&mut self) {
        let Some(root) = self.editor.assets.root().map(std::path::Path::to_path_buf) else {
            self.editor.status = Some("There is no assets folder on this machine".into());
            return;
        };
        let ai = &self.editor.workspace.ai;
        let vlm = ai
            .vision_model
            .clone()
            .or_else(|| ai.model.clone())
            .map(|model| (ai.backend.clone().unwrap_or_default(), model));
        let (send, receive) = crossbeam_channel::unbounded();
        self.arrange.busy = Some("Indexing assets\u{2026}".into());
        let id = self.tasks.spawn_thread(
            crate::tasks::TaskKind::Ai,
            "Assets: index & arrange plan",
            move |ctx| {
                let progress = ctx.progress.clone();
                let cancel = ctx.cancel.clone();
                let mut report_progress = |done: u64, total: u64, what: &str| {
                    progress.set(done, total);
                    progress.detail(what.to_string());
                };
                let quick = crate::asset_index::BuildOptions { limit: None, vlm: None };
                let report = match crate::asset_index::build_index(
                    &root,
                    &quick,
                    &mut report_progress,
                    &|| cancel.is_cancelled(),
                ) {
                    Ok(r) => r,
                    Err(e) => return crate::tasks::TaskOutcome::Failed(format!("Could not index the assets: {e}")),
                };
                let library = buzz_doc::AssetLibrary::at(&root);
                let mut note = format!(
                    "{} records ({} symbols inside assets); {} guessed from names",
                    report.records, report.symbol_records, report.guessed
                );
                if let Some(e) = &report.embed_error {
                    note.push_str(&format!("; no style folders (SigLIP: {e})"));
                }
                let _ = send.send(ArrangeMsg::Plan {
                    plan: crate::asset_index::plan_for(&library),
                    note: note.clone(),
                    last: vlm.is_none() || report.guessed == 0,
                });
                let Some((backend, model)) = vlm else {
                    return crate::tasks::TaskOutcome::Finished(String::new());
                };
                if report.guessed == 0 {
                    return crate::tasks::TaskOutcome::Finished(String::new());
                }
                if cancel.is_cancelled() {
                    return crate::tasks::TaskOutcome::Cancelled;
                }
                let full = crate::asset_index::BuildOptions {
                    limit: None,
                    vlm: Some((backend.clone(), model.clone())),
                };
                let report = match crate::asset_index::build_index(
                    &root,
                    &full,
                    &mut report_progress,
                    &|| cancel.is_cancelled(),
                ) {
                    Ok(r) => r,
                    Err(e) => return crate::tasks::TaskOutcome::Failed(format!("Could not index the assets: {e}")),
                };
                let mut note = format!(
                    "{} records ({} symbols inside assets); {} guessed from names",
                    report.records, report.symbol_records, report.guessed
                );
                if let Some(e) = &report.vlm_error {
                    note.push_str(&format!("; VLM: {e}"));
                } else {
                    let plan = crate::asset_index::plan_for(&library);
                    if let Err(e) = crate::asset_index::name_styles(
                        &root,
                        &plan,
                        &backend,
                        &model,
                        &mut report_progress,
                        &|| cancel.is_cancelled(),
                    ) {
                        note.push_str(&format!("; style names: {e}"));
                    }
                }
                let _ = send.send(ArrangeMsg::Plan {
                    plan: crate::asset_index::plan_for(&library),
                    note,
                    last: true,
                });
                crate::tasks::TaskOutcome::Finished(String::new())
            },
        );
        self.arrange_task = Some(id);
        self.arrange_rx = Some(receive);
    }

    /// Move the checked assets (on a task: every move rescans the library).
    fn start_arrange_apply(&mut self, moves: Vec<buzz_ai::asset_index::ArrangeMove>) {
        let mut library = self.editor.assets.clone();
        let (send, receive) = crossbeam_channel::unbounded();
        self.arrange.busy = Some(format!("Moving {} assets\u{2026}", moves.len()));
        let id = self.tasks.spawn_thread(crate::tasks::TaskKind::AssetScan, "Assets: arrange", move |ctx| {
            let result = buzz_ai::asset_index::apply_arrange(&mut library, &moves, |done, total| {
                ctx.progress.set(done as u64, total as u64)
            });
            let _ = send.send(ArrangeMsg::Applied(result));
            crate::tasks::TaskOutcome::Finished(String::new())
        });
        self.arrange_task = Some(id);
        self.arrange_rx = Some(receive);
    }

    /// Put back what the last arrange moved.
    fn start_arrange_undo(&mut self) {
        let mut library = self.editor.assets.clone();
        let (send, receive) = crossbeam_channel::unbounded();
        self.arrange.busy = Some("Undoing the last arrange\u{2026}".into());
        let id = self.tasks.spawn_thread(crate::tasks::TaskKind::AssetScan, "Assets: undo arrange", move |_ctx| {
            let _ = send.send(ArrangeMsg::Undone(buzz_ai::asset_index::undo_arrange(&mut library)));
            crate::tasks::TaskOutcome::Finished(String::new())
        });
        self.arrange_task = Some(id);
        self.arrange_rx = Some(receive);
    }

    /// Once a frame: the arrange task's progress and whatever it reported.
    fn poll_arrange(&mut self) {
        if let Some(id) = self.arrange_task {
            match self.tasks.running().find(|t| t.id == id) {
                Some(task) => {
                    let p = task.progress();
                    self.arrange.progress = (p.done, p.total);
                    if !p.detail.is_empty() {
                        self.arrange.busy = Some(p.detail);
                    }
                }
                None => {
                    self.arrange_task = None;
                    self.arrange.busy = None;
                }
            }
        }
        let Some(rx) = &self.arrange_rx else { return };
        let mut refresh = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ArrangeMsg::Plan { plan, note, last } => {
                    let root = self.editor.assets.root().map(std::path::Path::to_path_buf).unwrap_or_default();
                    let mut note = note;
                    if !last {
                        note.push_str(" \u{2014} the VLM is identifying the rest; the plan will update");
                    }
                    self.arrange.note = note;
                    self.arrange.set_plan(root, plan.moves, plan.cast_kept);
                }
                ArrangeMsg::Applied(result) => {
                    self.editor.assets.rescan();
                    self.asset_thumbnails.forget();
                    self.editor.status = Some(match &result {
                        Ok(log) => format!(
                            "Arranged {} assets into {} new folders \u{2014} Undo arrange puts them back",
                            log.moved.len(),
                            log.created_folders.len()
                        ),
                        Err(e) => format!("Could not arrange the assets: {e}"),
                    });
                    refresh = true;
                }
                ArrangeMsg::Undone(result) => {
                    self.editor.assets.rescan();
                    self.asset_thumbnails.forget();
                    self.editor.status = Some(match &result {
                        Ok(n) => format!("Put {n} assets back where they were"),
                        Err(e) => format!("Could not undo the arrange: {e}"),
                    });
                    refresh = true;
                }
            }
        }
        if refresh {
            self.arrange_task = None;
            self.arrange.busy = None;
            self.refresh_arrange_undo();
            if self.arrange.open {
                self.start_arrange_plan();
            }
        }
    }

    /// The agent's `arrange_assets` changed files under the library.
    pub(crate) fn agent_assets_moved(&mut self) {
        self.asset_thumbnails.forget();
        self.refresh_arrange_undo();
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

    /// The same, for a dialog that can answer with more than one path (D3).
    fn ask_for_paths(&mut self, request: crate::dialogs::Request, purpose: Pick) {
        let parent = self
            .active
            .as_ref()
            .map(|active| crate::dialogs::Parent::of(&active.window))
            .unwrap_or_default();

        if !self.picker_many.ask(request, parent, purpose) {
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
        self.apply_pick(purpose, path);
    }

    /// What a chosen path means for the question it answers. The one place
    /// every [`Pick`] is acted on: the file dialog's answer arrives here via
    /// `poll_picker`, and the AI agent (`crate::agent`) calls it directly with
    /// a path it was given, so the two cannot drift apart.
    pub(crate) fn apply_pick(&mut self, purpose: Pick, path: std::path::PathBuf) {
        match purpose {
            // D2: the single-file picker no longer asks for `Pick::Open` —
            // `App::open_dialog` uses the multi-select picker below — but the
            // arm stays so this match is exhaustive, and it goes through the
            // same queue so behaviour would still be right if anything else
            // ever asks this way.
            Pick::Open => {
                self.open_queue.push_back(path);
                self.advance_open_queue();
            }
            Pick::SaveAs => self.save_to(path),
            Pick::ImportImage => self.import_image_from(path),
            Pick::FillWithImage => self.fill_with_image_from(path),
            Pick::VideoReference => self.video_reference_from(path),
            Pick::SequenceFolder => self.sequence_folder_from(path),
            Pick::ImportSound => self.import_sound_from(path),
            Pick::SetMasterAudio => self.set_master_audio_from(path),
            Pick::ImportCaptions => match self.editor.import_captions(&path) {
                Ok(_) => {}
                Err(e) => self.editor.status = Some(format!("Could not read those captions: {e}")),
            },
            Pick::ExportCaptions => {
                if let Err(e) = self.editor.export_captions(&path) {
                    self.editor.status = Some(format!("Could not write those captions: {e}"));
                }
            }
            Pick::ImportInto(target) => self.import_file(target, path),
            Pick::AnimateAssets => self.import_animate_assets_from(path),
            Pick::Export(kind) => self.start_export(kind, path),
            Pick::ExportFla => self.export_fla_to(path),
            Pick::AiSceneImage => self.ai_scene_from_image_from(path),
        }
    }

    /// Act on whatever the multi-file picker came back with (D3): Import to
    /// Stage / Library, Import Sound and Import Image, the four dialogs that
    /// accept a batch. Every other purpose only ever asks through the
    /// single-file picker.
    fn poll_picker_many(&mut self) {
        let Some((purpose, paths)) = self.picker_many.poll() else {
            return;
        };
        let Some(paths) = paths else { return };

        match purpose {
            Pick::ImportInto(target) => self.start_batch_import(target, paths),
            Pick::ImportImage => self.import_images_from(paths),
            Pick::ImportSound => self.import_sounds_from(paths),
            // D2: File ▸ Open, one or several files chosen at once — each
            // becomes its own tab (or focuses one already open), one at a
            // time, off `App::open_queue`.
            Pick::Open => {
                self.open_queue.extend(paths);
                self.advance_open_queue();
            }
            other => debug_assert!(
                false,
                "the multi-file picker was opened for a purpose it does not \
                 handle: {other:?}"
            ),
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
            // The Assets panel's own folder. A script filing a cast there
            // is filing it where the panel will show it, which is the
            // whole point of an asset outliving its document.
            asset_root: buzz_doc::AssetLibrary::user()
                .root()
                .map(|p| p.to_path_buf()),
            // No interactive answers yet (J1 parked): `prompt` keeps its
            // unattended default-answer behaviour.
            prompt_answers: Vec::new(),
        };
        let mut working = self.editor.doc.scene().clone();

        let (send, result) = crossbeam_channel::bounded(1);
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Script, "Script", move |ctx| {
                // Cancel reaches the interpreter through its interrupt handler,
                // which QuickJS calls between bytecodes — so Stop lands mid-loop,
                // not at the end of one. A `while (true) {}` stops on the spot.
                let cancel = ctx.cancel.clone();
                let stop: buzz_script::StopSignal =
                    std::sync::Arc::new(move || cancel.is_cancelled());
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
        self.editor
            .selection
            .ensure_active_layer(self.editor.doc.scene());

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
            Loaded::Document { path, result } => {
                match result {
                    Ok(doc) => {
                        self.open_document_in_new_tab(*doc);
                        self.remember_document_directory(&path);
                        self.editor.status = Some(format!("Opened {}", path.display()));
                    }
                    Err(why) => self.editor.status = Some(why),
                }
                // Open (D2): the next queued path, if a multi-select chose
                // more than one — see `App::advance_open_queue`.
                self.advance_open_queue();
            }
            Loaded::Foreign {
                path,
                target,
                result,
            } => match (result, target) {
                (Ok(imported), None) => {
                    self.finish_open_imported(&path, *imported);
                    self.advance_open_queue();
                }
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
                    self.advance_open_queue();
                }
                (Err(message), Some(_)) => {
                    // A failed import leaves the open document untouched, which
                    // it does: nothing was merged.
                    let name = file_name(&path);
                    if let Some(batch) = &mut self.import_batch {
                        // Part of a batch (D3): recorded for the one summary
                        // line at the end, and the rest still runs — a bad
                        // file among good ones does not stop them.
                        batch.failed.push((name, message));
                        self.advance_import_queue();
                    } else {
                        self.editor.status = Some(format!("Could not import {name}: {message}"));
                    }
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

    /// AI ▸ Scene from Image: ask where the reference picture is. The file
    /// dialog and the VLM call are both handled off the UI thread — see
    /// `poll_picker` and `ai_scene_from_image_from`.
    fn ai_scene_from_image_dialog(&mut self) {
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Image", &["png", "jpg", "jpeg", "webp"]),
            Pick::AiSceneImage,
        );
    }

    /// Read the reference image, run the VLM, and rebuild a scene against the
    /// current library — all on a thread of its own.
    ///
    /// The library is snapshotted here, on the UI thread, which is cheap: only
    /// each symbol's id, name, tags and bounds travel, not the artwork.
    fn ai_scene_from_image_from(&mut self, path: std::path::PathBuf) {
        let backend = self.editor.workspace.ai.backend.clone().unwrap_or_default();
        let model = self
            .editor
            .workspace
            .ai
            .vision_model
            .clone()
            .or_else(|| self.editor.workspace.ai.model.clone())
            .unwrap_or_else(|| "llava".to_string());

        // F7.1: only what a person would place — symbols on stage, or used
        // by nothing — never the body parts inside a character.
        let candidates = buzz_ai::scene_builder::placeable_candidates(self.editor.doc.scene());
        let stage_size = self.editor.doc.scene().stage().size;
        let image_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let started = std::time::Instant::now();
        // Only walked if SigLIP turns out to be installed, to render each
        // candidate's thumbnail — a cheap `Scene` clone until then, pointer
        // copies rather than the artwork.
        let scene_snapshot = self.editor.doc.scene().clone();

        let (send, receive) = crossbeam_channel::bounded(1);
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI", move |_ctx| {
                let bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(e) => {
                        return crate::tasks::TaskOutcome::Failed(format!(
                            "Failed to read image file: {e}"
                        ));
                    }
                };
                let layout =
                    match buzz_ai::scene_builder::decompose_scene_image(&backend, &model, &bytes) {
                        Ok(l) => l,
                        Err(e) => {
                            return crate::tasks::TaskOutcome::Failed(format!(
                                "VLM scene decomposition failed: {e}"
                            ));
                        }
                    };

                // SigLIP asset matching, when the model is installed: each
                // detected object's own crop of the reference image, and a
                // render of each library candidate, embedded and handed to
                // `build_scene_from_layout` so matching can lean on what
                // things actually look like, not only their name and tags.
                // Never blocks the match on a missing model — it just narrows
                // it to name/tags, and says so in the final status.
                let mut candidates = candidates;
                let mut embeddings_for_layout: Option<Vec<Option<Vec<f32>>>> = None;
                let mut pictures: Option<(Vec<image::RgbaImage>, Vec<image::RgbaImage>)> = None;
                let embed_note = if buzz_ai::models::status(buzz_ai::models::ModelId::SigLip)
                    .is_ready()
                {
                    match image::load_from_memory(&bytes) {
                        Ok(decoded) => match siglip_embeddings_for_scene_from_image(
                            &decoded.to_rgba8(),
                            &layout,
                            &scene_snapshot,
                            &candidates,
                        ) {
                            Ok(SiglipInputs { detected, candidate_embeddings, crops, thumbnails }) => {
                                for (c, emb) in candidates.iter_mut().zip(candidate_embeddings) {
                                    c.visual_embedding = emb;
                                }
                                embeddings_for_layout = Some(detected);
                                pictures = Some((crops, thumbnails));
                                String::new()
                            }
                            Err(e) => format!(" (SigLIP embedding skipped: {e})"),
                        },
                        Err(e) => format!(
                            " (SigLIP embedding skipped: could not decode the reference image: {e})"
                        ),
                    }
                } else {
                    " (install the SigLIP model in AI \u{25b8} AI Models\u{2026} to match by appearance too)"
                        .to_string()
                };
                let have_embeddings = embeddings_for_layout.is_some();
                let detected_embeddings =
                    embeddings_for_layout.unwrap_or_else(|| vec![None; layout.objects.len()]);

                let crop_skeletons = vec![None; layout.objects.len()];
                // F7.1: SigLIP shortlists each object's candidates; the vision
                // model confirms one from a numbered sheet; failing that,
                // SigLIP's best counts when it clears `SIGLIP_MATCH_MIN`.
                let mut shortlists = Vec::with_capacity(layout.objects.len());
                let mut chosen = Vec::with_capacity(layout.objects.len());
                for (i, obj) in layout.objects.iter().enumerate() {
                    let emb = detected_embeddings.get(i).and_then(|e| e.as_deref());
                    let short = buzz_ai::scene_builder::siglip_shortlist(emb, &candidates, 6);
                    let confirmed = pictures.as_ref().and_then(|(crops, thumbs)| {
                        let imgs: Vec<image::RgbaImage> =
                            short.iter().map(|(ci, _)| thumbs[*ci].clone()).collect();
                        buzz_ai::scene_builder::confirm_with_vlm(&backend, &model, &obj.label, &crops[i], &imgs)
                            .ok()
                            .flatten()
                    });
                    let pick = confirmed.or_else(|| {
                        short
                            .first()
                            .filter(|(_, v)| *v >= buzz_ai::scene_builder::SIGLIP_MATCH_MIN)
                            .map(|_| 0)
                    });
                    shortlists.push(short);
                    chosen.push(pick);
                }
                let reconstructed = if have_embeddings {
                    let choices: Vec<Option<(usize, f64)>> = chosen
                        .iter()
                        .zip(&shortlists)
                        .map(|(c, s)| c.and_then(|k| s.get(k).copied()))
                        .collect();
                    buzz_ai::scene_builder::build_scene_from_choices(
                        &layout,
                        &candidates,
                        &crop_skeletons,
                        &choices,
                        Some(stage_size),
                    )
                } else {
                    buzz_ai::scene_builder::build_scene_from_layout(
                        &layout,
                        &candidates,
                        &crop_skeletons,
                        &detected_embeddings,
                        Some(stage_size),
                    )
                };
                let review = SceneReviewData {
                    layout: layout.clone(),
                    candidates: candidates.clone(),
                    shortlists,
                    chosen,
                    stage_size,
                    image_name,
                    secs: started.elapsed().as_secs_f64(),
                };
                let _ = send.send(AiResult::Scene {
                    reconstructed: Box::new(reconstructed),
                    embed_note,
                    review: Box::new(review),
                });
                crate::tasks::TaskOutcome::Finished(String::new())
            });
        self.track_ai(id, receive);
    }

    /// AI ▸ Direct Story: parse the brief on a thread, and execute the
    /// resulting script against the document once it is back — see
    /// `collect_ai_result`.
    fn ai_direct_story_async(&mut self) {
        let brief = self.editor.story.draft.trim().to_string();
        self.ai_direct_story_from(brief);
    }

    /// Direct Story from a brief given directly — the AI panel's own box.
    pub(crate) fn ai_direct_story_from(&mut self, brief: String) {
        let brief = brief.trim().to_string();
        if brief.is_empty() {
            self.editor.story.focus_draft = true;
            self.editor.raise_story_panel();
            self.editor.status = Some("Type a story brief in the Story panel first".into());
            return;
        }

        let backend = self.editor.workspace.ai.backend.clone().unwrap_or_default();
        let model = self
            .editor
            .workspace
            .ai
            .model
            .clone()
            .unwrap_or_else(|| "qwen2.5:7b".into());

        let (send, receive) = crossbeam_channel::bounded(1);
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI", move |_ctx| {
                match buzz_ai::director::parse_story_to_script(&backend, &model, &brief) {
                    Ok(script) => {
                        let beat_count = script.beats.len();
                        let _ = send.send(AiResult::Directed {
                            script: Box::new(script),
                            beat_count,
                        });
                        crate::tasks::TaskOutcome::Finished(String::new())
                    }
                    Err(e) => {
                        crate::tasks::TaskOutcome::Failed(format!("LLM brief parsing failed: {e}"))
                    }
                }
            });
        self.track_ai(id, receive);
    }

    /// AI ▸ Auto-Rig Vector Character: detect a real skeleton in a render of
    /// the symbol itself and rig it, off the UI thread — see
    /// `rig_vector_character_from_detected_skeleton` and `collect_ai_result`.
    ///
    /// The target symbol, and — if one was selected on stage — that
    /// instance's id, layer and transform are resolved here, on the UI
    /// thread, before the task starts: that is what lets the finished
    /// armature *replace* the instance later instead of duplicating the
    /// drawing. `scene.clone()` is cheap, a `Scene`'s own `Clone` being
    /// pointer copies rather than the artwork.
    pub(crate) fn ai_rig_vector_character_async(&mut self) {
        let stage_instance = self.editor.selection.iter().next().and_then(|id| {
            self.editor
                .doc
                .scene()
                .find_object(id)
                .and_then(|(layer, o)| o.instance().map(|i| (layer, o.id, i.symbol, o.transform)))
        });

        let sym_id = stage_instance
            .as_ref()
            .map(|(_, _, sym, _)| *sym)
            .or(self.editor.library.selected);

        let Some(sym_id) = sym_id else {
            self.editor.status =
                Some("Select a character symbol in the Library or on stage first".into());
            return;
        };
        let Some(symbol) = self.editor.doc.scene().library().get(sym_id) else {
            self.editor.status = Some("That symbol is no longer in the library".into());
            return;
        };
        let sym_name = symbol.name.clone();

        let target = match stage_instance {
            Some((layer, object, _, transform)) => {
                RigTarget::ReplaceInstance { layer, object, transform }
            }
            None => RigTarget::NewLayer { at: self.editor.camera.center },
        };

        let scene = self.editor.doc.scene().clone();
        let (send, receive) = crossbeam_channel::bounded(1);
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI", move |_ctx| {
                match rig_vector_character_from_detected_skeleton(scene, sym_id, &sym_name) {
                    Ok(rerigged) => {
                        let _ = send.send(AiResult::Rigged {
                            target,
                            sym_name,
                            rerigged: Box::new(rerigged),
                        });
                        crate::tasks::TaskOutcome::Finished(String::new())
                    }
                    Err(e) => crate::tasks::TaskOutcome::Failed(e),
                }
            });
        self.track_ai(id, receive);
    }

    /// An AI task finished. Commit whatever it built in one `Document::edit`,
    /// so it lands as a single undo step, and report the result.
    fn collect_ai_result(&mut self, id: crate::tasks::TaskId) {
        let Some(receiver) = self.ai_results.remove(&id) else {
            return;
        };
        let Ok(result) = receiver.try_recv() else {
            self.editor.status = Some("The AI task finished but returned nothing".into());
            return;
        };

        // A job the Assistant asked for: its result, for `wait_for_jobs`.
        let mut job_value = self
            .agent_jobs
            .iter()
            .any(|j| j.task == id)
            .then(|| ai_job_summary(&result));

        match result {
            AiResult::AgentTranscript { .. } => {
                // Nothing to commit: the transcript is the job's result.
            }
            AiResult::Character(assembled, kind) => {
                job_value = Some(self.commit_agent_character(&assembled, kind));
            }
            AiResult::Scene { reconstructed, embed_note, review } if self.ai_from_panel.remove(&id) || self.ai_review_mode() => {
                let _ = reconstructed;
                self.ai_panel.scene = Some(buzz_ui::ai_panel::SceneReview {
                    image_name: review.image_name.clone(),
                    rows: review
                        .layout
                        .objects
                        .iter()
                        .enumerate()
                        .map(|(i, o)| buzz_ui::ai_panel::SceneObjectRow {
                            label: o.label.clone(),
                            kind: o.kind.clone(),
                            shortlist: review.shortlists[i]
                                .iter()
                                .map(|(ci, v)| (review.candidates[*ci].name.clone(), *v))
                                .collect(),
                            chosen: review.chosen[i],
                        })
                        .collect(),
                    secs: review.secs,
                    note: embed_note,
                });
                self.ai_scene_pending = Some(review);
                self.editor.workspace.reveal(buzz_ui::PanelId::Ai);
                self.editor.status = Some("Scene from Image: review the matches in the AI panel, then Build".into());
            }
            AiResult::Scene { reconstructed, embed_note, .. } => {
                self.commit_reconstructed_scene(&reconstructed, &embed_note);
            }
            AiResult::Transcribed { sound, transcript } => {
                self.ai_panel.transcript = Some(buzz_ui::ai_panel::TranscriptReview {
                    sound: sound.clone(),
                    lines: transcript.segments.iter().map(|s| (s.start, s.end, s.text.clone())).collect(),
                    secs: (transcript.load_ms + transcript.run_ms) as f64 / 1000.0,
                    provider: transcript.provider.clone(),
                });
                self.editor.workspace.reveal(buzz_ui::PanelId::Ai);
                self.editor.status = Some(format!(
                    "Transcribed '{sound}': {} lines in {:.1}s \u{2014} edit them in the AI panel, then Apply",
                    transcript.segments.len(),
                    (transcript.load_ms + transcript.run_ms) as f64 / 1000.0
                ));
            }
            AiResult::PoseSheet(images) => {
                // Uploaded as textures by `draw_ai_panel`, which has a `Ui`.
                self.ai_pose_images = Some(images);
            }
            AiResult::Directed { script, beat_count } if self.ai_from_panel.remove(&id) => {
                self.ai_panel.story = Some(story_review(&script, beat_count));
                self.ai_story_pending = Some(script);
                self.editor.workspace.reveal(buzz_ui::PanelId::Ai);
                self.editor.status = Some(format!("Direct Story: {beat_count} beats parsed \u{2014} Apply in the AI panel"));
            }
            AiResult::Directed { script, beat_count } => {
                self.commit_directed(&script, beat_count);
            }
            AiResult::Rigged { target, sym_name, rerigged } if self.ai_from_panel.remove(&id) => {
                self.ai_panel.rig = Some(buzz_ui::ai_panel::RigReview {
                    symbol: sym_name.clone(),
                    mapping: rerigged
                        .mappings
                        .iter()
                        .map(|m| (m.part_name.clone(), m.slot_name.clone()))
                        .collect(),
                    secs: 0.0,
                });
                self.ai_rig_pending = Some((target, sym_name, rerigged));
                self.editor.workspace.reveal(buzz_ui::PanelId::Ai);
                self.editor.status = Some("Auto-Rig: check the layer \u{2192} bone mapping in the AI panel, then Apply".into());
            }
            AiResult::Rigged { target, sym_name, rerigged } => {
                self.commit_rig(target, sym_name, *rerigged);
            }
            AiResult::Produced { report, open } => {
                self.produce.busy = false;
                let r = &report;
                let summary = format!(
                    "Produced {} ({} cut(s), {} line(s) from the recording); report {}",
                    r.buzz.display(),
                    r.cuts,
                    r.lines_from_audio,
                    r.report.display()
                );
                self.produce.status = Some(summary.clone());
                if job_value.is_some() {
                    job_value = Some(serde_json::json!({
                        "buzz": r.buzz,
                        "mp4": r.mp4,
                        "report": r.report,
                        "cast": r.cast,
                        "backgrounds": r.backgrounds,
                        "generated_characters": r.generated_characters,
                        "cuts": r.cuts,
                        "lines_from_audio": r.lines_from_audio,
                        "lines_from_text": r.lines_from_text,
                        "skipped_steps": r.skipped_steps,
                        "warnings": r.warnings,
                    }));
                }
                if open {
                    self.open_path(&report.buzz);
                }
                self.editor.status = Some(summary);
            }
        }

        if let Some(mut value) = job_value {
            if let Some(map) = value.as_object_mut() {
                map.entry("status").or_insert_with(|| serde_json::json!(self.editor.status));
            }
            self.agent_job_done(id, value);
        }
    }

    /// Note that an Assistant job produced `value`. `poll_tasks` then calls
    /// [`Self::settle_agent_job`], which leaves a finished job alone.
    fn agent_job_done(&mut self, id: crate::tasks::TaskId, value: serde_json::Value) {
        if let Some(job) = self.agent_jobs.iter_mut().find(|j| j.task == id) {
            job.state = crate::agent::jobs::JobState::Done(value);
        }
    }

    /// A task ended. If the Assistant started it and nothing has said how it
    /// went yet, say so: a failure or a cancel is the job's `error`.
    fn settle_agent_job(&mut self, id: crate::tasks::TaskId, outcome: &crate::tasks::TaskOutcome) {
        use crate::agent::jobs::JobState;
        use crate::tasks::TaskOutcome;
        let status = self.editor.status.clone();
        let Some(job) = self.agent_jobs.iter_mut().find(|j| j.task == id) else { return };
        if job.state != JobState::Running {
            return;
        }
        job.state = match outcome {
            TaskOutcome::Finished(message) => {
                let say = if message.is_empty() { status } else { Some(message.clone()) };
                JobState::Done(serde_json::json!({"status": say}))
            }
            TaskOutcome::Cancelled => JobState::Failed("cancelled".into()),
            TaskOutcome::Failed(why) => JobState::Failed(why.clone()),
        };
    }

    /// Run the tasks that have finished: what the frame loop does once a
    /// frame, and what the Assistant's `wait_for_jobs` does before it reports.
    pub(crate) fn agent_poll_tasks(&mut self) {
        // The BuzzEdit and Studio flows (`crate::estate`) settle their own
        // jobs, on either side of the tasks ending.
        self.estate_pump();
        self.poll_tasks();
        self.estate_pump();
    }

    /// Forget which task was started last, before a command that may start one.
    pub(crate) fn agent_forget_last_task(&mut self) {
        self.last_ai_task = None;
    }

    /// If the command just run started an AI task, remember it as an Assistant
    /// job called `kind` and return its id.
    pub(crate) fn agent_track_last_job(&mut self, kind: &str) -> Option<u64> {
        let task = self.last_ai_task.take()?;
        if !self.tasks.is_running(task) {
            // Already over (a task can end before the command returns).
            return None;
        }
        self.agent_jobs.push(crate::agent::jobs::AgentJob {
            id: task.0,
            kind: kind.to_string(),
            task,
            state: crate::agent::jobs::JobState::Running,
            delivered: false,
        });
        Some(task.0)
    }

    /// Scene from Image started from the menu commits at once; the panel's
    /// own runs wait for Build. No menu run is ever in review mode.
    fn ai_review_mode(&self) -> bool {
        false
    }

    /// Place a reconstructed scene's layers in the document, one undo step.
    fn commit_reconstructed_scene(
        &mut self,
        reconstructed: &buzz_ai::scene_builder::ReconstructedScene,
        embed_note: &str,
    ) {
        {
            {
                let matched_count = reconstructed.matched.len();
                let unmatched_count = reconstructed.unmatched.len();
                self.editor.doc.edit("AI: Scene from Reference Image", |scene| {
                    // Import layers from reconstructed scene
                    for layer in reconstructed.scene.stage_layers().iter() {
                        if layer.name == "Layer 1" && layer.objects_at(0).is_empty() {
                            continue;
                        }
                        let new_layer = scene.add_layer(&layer.name, layer.kind);
                        for obj in layer.objects_at(0) {
                            let mut clone = (**obj).clone();
                            clone.id = scene.next_object_id();
                            scene.add_object(new_layer, clone);
                        }
                    }
                });
                self.editor.status = Some(format!(
                    "Reconstructed scene: {matched_count} placed, {unmatched_count} unmatched{embed_note}"
                ));
            }
        }
    }

    /// Execute a parsed story, with dialogue captions and lip sync.
    fn commit_directed(&mut self, script: &buzz_ai::director::DirectorScript, beat_count: usize) {
        {
            {
                let mut exec_result = None;
                self.editor.doc.edit("AI: Direct Story Sequence", |scene| {
                    exec_result = Some(buzz_ai::director::execute_director_script(scene, &script));
                });
                match exec_result {
                    Some(Ok(directed)) => {
                        let mut msg = format!(
                            "Directed story with {} cast, {} beats over {} frames",
                            directed.names.len(),
                            beat_count,
                            directed.frames,
                        );
                        // Dialogue: caption and lip-sync it through the same
                        // join `Lip Sync from Captions` uses for an imported
                        // `.srt`, driven by the frames `direct` already gave
                        // each Talk beat rather than by a file. A directed
                        // story usually has no recorded narration yet, so
                        // `sync_directed_dialogue` treats a missing
                        // soundtrack as expected, not a failure, and leaves a
                        // status explaining what happened; folded into this
                        // one status line rather than left to overwrite it.
                        let fps = self.editor.doc.scene().stage().frame_rate.max(1.0);
                        let cues =
                            buzz_ai::director::dialogue_cues(&script, &directed, fps);
                        if !cues.is_empty() {
                            let _ = self.editor.sync_directed_dialogue(&cues);
                            if let Some(dialogue_status) = self.editor.status.take() {
                                msg.push_str(" \u{2014} ");
                                msg.push_str(&dialogue_status);
                            }
                        }
                        self.editor.status = Some(msg);
                    }
                    Some(Err(e)) => {
                        self.editor.status = Some(format!("Directing failed: {e}"));
                    }
                    None => {}
                }
            }
        }
    }

    /// Land an auto-rigged armature where `target` says, one undo step.
    fn commit_rig(
        &mut self,
        target: RigTarget,
        sym_name: String,
        rerigged: buzz_ai::rerig::ReriggedCharacter,
    ) {
        {
            {
                let bone_count = rerigged.armature.armature.bones.len();
                let label = format!("Auto-Rig '{sym_name}'");
                let status = format!(
                    "Auto-rigged '{sym_name}' with {bone_count} bones and standard poses"
                );
                self.editor.doc.edit(label, move |scene| {
                        // Where the armature goes: the stage instance it was
                        // rigged from, if it is still exactly where it was
                        // when the task started — otherwise (nothing was
                        // selected, or the instance moved or vanished while
                        // the task ran) a fresh layer, at the same point.
                        let still_there = matches!(
                            &target,
                            RigTarget::ReplaceInstance { layer, object, .. }
                                if scene.find_object(*object).map(|(l, _)| l) == Some(*layer)
                        );
                        let (dest_layer, transform, replace) = match target {
                            RigTarget::ReplaceInstance { layer, object, transform } if still_there => {
                                (layer, transform, Some(object))
                            }
                            RigTarget::ReplaceInstance { transform, .. } => (
                                scene.add_layer(&format!("{sym_name} Rig"), buzz_scene::LayerKind::Normal),
                                transform,
                                None,
                            ),
                            RigTarget::NewLayer { at } => (
                                scene.add_layer(&format!("{sym_name} Rig"), buzz_scene::LayerKind::Normal),
                                buzz_geom::Affine::translate((at.x, at.y)),
                                None,
                            ),
                        };
                        let armature_obj = buzz_scene::Object {
                            id: scene.next_object_id(),
                            name: Some(format!("{sym_name}_Armature")),
                            transform,
                            kind: buzz_scene::ObjectKind::Armature(rerigged.armature),
                            locked: false,
                            visible: true,
                            filters: Vec::new(),
                            blend: Default::default(),
                            spatial: Default::default(),
                            pivot: None,
                            modifiers: Vec::new(),
                            text: None,
                            turnaround: Default::default(),
                        };
                        match replace {
                            Some(object_id) => {
                                scene.update_layer(dest_layer, |l| {
                                    if let Some(objects) = l.frames.objects_at_mut(0)
                                        && let Some(slot) =
                                            objects.iter_mut().find(|o| o.id == object_id)
                                    {
                                        *slot = std::sync::Arc::new(armature_obj);
                                    }
                                });
                            }
                            None => {
                                scene.add_object(dest_layer, armature_obj);
                            }
                        }
                    });
                self.editor.status = Some(status);
            }
        }
    }

    /// Remember an AI task's result channel, and whether the AI panel
    /// started it (its result then waits in the panel for Build/Apply).
    fn track_ai(&mut self, id: crate::tasks::TaskId, receive: crossbeam_channel::Receiver<AiResult>) {
        self.last_ai_task = Some(id);
        self.ai_results.insert(id, receive);
        if std::mem::take(&mut self.ai_next_from_panel) {
            self.ai_from_panel.insert(id);
        }
    }

    /// **The AI panel** (F7.0): draw it, then carry out what it asked for.
    fn draw_ai_panel(&mut self, ui: &mut egui::Ui) {
        if let Some(images) = self.ai_pose_images.take() {
            self.editor.ai_character.pose_sheet = images
                .into_iter()
                .map(|(name, img)| {
                    let size = [img.width() as usize, img.height() as usize];
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                    let tex = ui.ctx().load_texture(format!("ai_pose_{name}"), color, Default::default());
                    (name, tex)
                })
                .collect();
        }
        buzz_ai::llm::set_low_reasoning(self.editor.workspace.ai.low_reasoning);
        self.agent_refresh_label();
        let activity: Vec<buzz_ui::TaskRow> = self
            .tasks
            .running()
            .filter(|t| t.kind == crate::tasks::TaskKind::Ai)
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
        let sounds: Vec<String> = self.editor.doc.scene().sounds().iter().map(|s| s.name.clone()).collect();
        let rig_target = self.ai_rig_target_name();
        let ctx = ui.ctx().clone();
        self.ensure_ai_rig_limits();
        let editor = &mut self.editor;
        let actions = buzz_ui::ai_panel::ai_panel(
            ui,
            &ctx,
            buzz_ui::ai_panel::AiPanelView {
                models: &mut editor.ai_models,
                settings: &mut editor.workspace.ai,
                character: &mut editor.ai_character,
                rename: &mut editor.ai_rename,
                panel: &mut self.ai_panel,
                sounds: &sounds,
                activity: &activity,
                rig_target,
            },
        );
        for action in actions {
            self.ai_panel_action(action);
        }
    }

    /// The symbol Auto-Rig would work on: a selected stage instance's, else
    /// the Library's selection.
    fn ai_rig_target_name(&self) -> Option<String> {
        let scene = self.editor.doc.scene();
        let from_stage = self
            .editor
            .selection
            .iter()
            .next()
            .and_then(|id| scene.find_object(id))
            .and_then(|(_, o)| o.instance().map(|i| i.symbol));
        from_stage
            .or(self.editor.library.selected)
            .and_then(|id| scene.library().get(id))
            .map(|s| s.name.clone())
    }

    fn ai_panel_action(&mut self, action: buzz_ui::ai_panel::AiPanelAction) {
        use buzz_ui::ai_panel::AiPanelAction as A;
        match action {
            A::CharacterCreated => {
                if let Some(assembled) = self.editor.ai_character.assembled.take() {
                    let kind = self.editor.ai_character.rig_kind;
                    self.editor.create_ai_character_as(&assembled, kind);
                    self.ai_render_pose_sheet();
                    self.editor.ai_character.assembled = Some(assembled);
                }
            }
            A::AutoRigDetect => {
                self.ai_next_from_panel = true;
                self.ai_rig_vector_character_async();
                self.ai_next_from_panel = false;
            }
            A::AutoRigApply => {
                if let Some((target, name, rerigged)) = self.ai_rig_pending.take() {
                    self.commit_rig(target, name, *rerigged);
                    self.ai_panel.rig = None;
                }
            }
            A::ScenePickImage => {
                // Consumed when the picked image's task starts.
                self.ai_next_from_panel = true;
                self.ai_scene_from_image_dialog();
            }
            A::SceneBuild => self.ai_build_scene_from_panel(),
            A::VoiceMode(mode) => self.voice_set_mode(mode),
            A::VoiceMute => self.voice_toggle_mute(),
            A::VoicePush => self.voice_apply(buzz_ai::voice::VoiceInput::PushPress),
            A::VoiceDevice(device) => self.voice_set_device(device),
            A::AllowAll(on) => self.set_allow_all(on),
            A::DirectStory(brief) => {
                self.ai_next_from_panel = true;
                self.ai_direct_story_from(brief);
                self.ai_next_from_panel = false;
            }
            A::StoryApply => {
                if let Some(script) = self.ai_story_pending.take() {
                    let beats = script.beats.len();
                    self.commit_directed(&script, beats);
                    self.ai_panel.story = None;
                }
            }
            A::IdentifyStart => self.editor.open_ai_rename(),
            A::IdentifyApply(proposals) => {
                let count = proposals.iter().filter(|p| p.selected).count();
                self.editor.doc.edit("AI: Identify and Rename Assets", |scene| {
                    buzz_ai::naming::apply_renames_to_scene(scene, &proposals);
                });
                self.editor.status = Some(format!("Applied AI renames to {count} assets"));
            }
            A::Transcribe(sound) => self.ai_transcribe_async(sound),
            A::TranscriptApply => self.ai_apply_transcript(),
            A::Cancel(id) => self.tasks.cancel(crate::tasks::TaskId(id)),
            A::AssistantRun(request) => self.agent_start(request),
            A::AssistantStop => self.agent_stop(),
            A::AssistantConfirm(yes) => self.agent_confirm(yes),
        }
    }

    /// Place the Scene from Image objects with the choices made in the panel.
    fn ai_build_scene_from_panel(&mut self) {
        let Some(review) = self.ai_scene_pending.take() else {
            return;
        };
        let rows = self.ai_panel.scene.as_ref().map(|s| s.rows.clone()).unwrap_or_default();
        let choices: Vec<Option<(usize, f64)>> = review
            .shortlists
            .iter()
            .enumerate()
            .map(|(i, short)| {
                rows.get(i)
                    .and_then(|r| r.chosen)
                    .or(review.chosen.get(i).copied().flatten())
                    .and_then(|k| short.get(k).copied())
            })
            .collect();
        let crop_skeletons = vec![None; review.layout.objects.len()];
        let reconstructed = buzz_ai::scene_builder::build_scene_from_choices(
            &review.layout,
            &review.candidates,
            &crop_skeletons,
            &choices,
            Some(review.stage_size),
        );
        self.commit_reconstructed_scene(&reconstructed, "");
        self.ai_panel.scene = None;
    }

    /// AI ▸ Transcribe (F7.9): Whisper on a library sound, off the UI thread.
    fn ai_transcribe_async(&mut self, sound_name: String) {
        let Some(asset) = self
            .editor
            .doc
            .scene()
            .sounds()
            .iter()
            .find(|s| s.name == sound_name)
            .cloned()
        else {
            self.editor.status = Some(format!("There is no sound called '{sound_name}'"));
            return;
        };
        let (send, receive) = crossbeam_channel::bounded(1);
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI: Transcribe", move |ctx| {
                if !buzz_ai::models::status(buzz_ai::models::ModelId::Whisper).is_ready() {
                    return crate::tasks::TaskOutcome::Failed(
                        "Download Whisper base.en in AI \u{25b8} Models first".into(),
                    );
                }
                let clip = match buzz_audio::Clip::decode(&asset.data, &asset.name) {
                    Ok(c) => c,
                    Err(e) => return crate::tasks::TaskOutcome::Failed(format!("Could not decode the sound: {e}")),
                };
                let progress = ctx.progress.clone();
                let cancel = ctx.cancel.clone();
                match buzz_ai::transcribe::transcribe_clip(
                    &clip,
                    &mut |done, total| progress.set(done as u64, total as u64),
                    &|| cancel.is_cancelled(),
                ) {
                    Ok(transcript) => {
                        let _ = send.send(AiResult::Transcribed {
                            sound: asset.name.clone(),
                            transcript: Box::new(transcript),
                        });
                        crate::tasks::TaskOutcome::Finished(String::new())
                    }
                    Err(e) => crate::tasks::TaskOutcome::Failed(format!("Transcription failed: {e}")),
                }
            });
        self.track_ai(id, receive);
    }

    /// The panel's edited transcript → captions and lip sync.
    fn ai_apply_transcript(&mut self) {
        let Some(t) = self.ai_panel.transcript.clone() else {
            return;
        };
        let speaker = self.ai_panel.transcribe_speaker.trim().to_string();
        let segments: Vec<buzz_ai::transcribe::Segment> = t
            .lines
            .iter()
            .filter(|(_, _, text)| !text.trim().is_empty())
            .map(|(a, b, text)| buzz_ai::transcribe::Segment { start: *a, end: *b, text: text.clone() })
            .collect();
        let fps = self.editor.doc.scene().stage().frame_rate.max(1.0);
        let cues = buzz_ai::transcribe::to_dialogue_cues(&segments, &speaker, fps, 0);
        match self.editor.apply_transcript(&t.sound, &cues) {
            Ok(written) => {
                let lip = self.editor.status.take().unwrap_or_default();
                self.editor.status = Some(format!(
                    "{} caption(s) from '{}' for {speaker}; {written} lip-sync key(s). {lip}",
                    cues.len(),
                    t.sound
                ));
                self.ai_panel.transcript = None;
            }
            Err(e) => self.editor.status = Some(format!("Could not apply the transcript: {e}")),
        }
    }

    /// The Assistant's `transcribe` (H1): Whisper on a library sound or a file,
    /// on a thread of its own. The model is chosen and the source checked here,
    /// where they are cheap, so a missing model or a mistyped path is an
    /// immediate error rather than a failed job. Returns the `{started, job}`
    /// answer.
    pub(crate) fn agent_transcribe_async(
        &mut self,
        source: TranscribeSource,
    ) -> Result<serde_json::Value, String> {
        // What the job decodes: library bytes, or a file read on the thread.
        enum Input {
            Bytes(Vec<u8>, String),
            File(std::path::PathBuf),
        }
        let (label, input) = match source {
            TranscribeSource::Sound(name) => {
                let scene = self.editor.doc.scene();
                match scene.sounds().iter().find(|s| s.name == name) {
                    Some(asset) => (name.clone(), Input::Bytes(asset.data.to_vec(), name)),
                    None => {
                        let names: Vec<String> = scene.sounds().iter().map(|s| s.name.clone()).collect();
                        return Err(format!(
                            "there is no sound called '{name}' in the library; sounds: [{}]",
                            names.join(", ")
                        ));
                    }
                }
            }
            TranscribeSource::File(path) => {
                if !path.is_file() {
                    return Err(format!("no audio file at {}", path.display()));
                }
                (path.display().to_string(), Input::File(path))
            }
        };
        // After the source, so a typo is reported as a typo even with no model.
        let model = buzz_ai::transcribe::installed_whisper()?;
        let (send, receive) = crossbeam_channel::bounded(1);
        let source_label = label.clone();
        self.last_ai_task = None;
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI: Transcribe", move |ctx| {
                let clip = match input {
                    Input::Bytes(bytes, name) => buzz_audio::Clip::decode(&bytes, &name),
                    Input::File(path) => buzz_audio::Clip::open(&path),
                };
                let clip = match clip {
                    Ok(c) => c,
                    Err(e) => return crate::tasks::TaskOutcome::Failed(format!("Could not decode the sound: {e}")),
                };
                let progress = ctx.progress.clone();
                let cancel = ctx.cancel.clone();
                let mut on_progress = |done: usize, total: usize| progress.set(done as u64, total as u64);
                let is_cancelled = || cancel.is_cancelled();
                let heard = match model {
                    buzz_ai::transcribe::WhisperChoice::BaseEn => {
                        buzz_ai::transcribe::transcribe_clip(&clip, &mut on_progress, &is_cancelled)
                            .map(|t| (t, "en".to_string(), "per line"))
                    }
                    buzz_ai::transcribe::WhisperChoice::Multilingual => {
                        buzz_ai::transcribe::transcribe_clip_multi(&clip, &mut on_progress, &is_cancelled)
                            .map(|(t, lang)| (t, lang, "approximate: each line is a window of about 25 s"))
                    }
                };
                match heard {
                    Ok((transcript, language, timing)) => {
                        let _ = send.send(AiResult::AgentTranscript {
                            source: source_label,
                            transcript: Box::new(transcript),
                            model: model.label(),
                            language,
                            timing,
                        });
                        crate::tasks::TaskOutcome::Finished(String::new())
                    }
                    Err(_) if cancel.is_cancelled() => crate::tasks::TaskOutcome::Cancelled,
                    Err(e) => crate::tasks::TaskOutcome::Failed(format!("Transcription failed: {e}")),
                }
            });
        self.track_ai(id, receive);
        let job = self.agent_track_last_job("transcribe").ok_or("the transcription task did not start")?;
        Ok(crate::agent::jobs::started(
            "transcribe",
            job,
            &format!("Whisper ({}) is listening to '{label}' off the UI thread", model.label()),
        ))
    }

    /// The Assistant's `ai_character_from_image` (H2): the Character from Image
    /// wizard with its default choices, as one task. The finished character is
    /// added on the UI thread (`commit_agent_character`), as **Create Character**
    /// does, in one undo step.
    pub(crate) fn agent_character_async(
        &mut self,
        path: std::path::PathBuf,
        name: String,
        kind: buzz_rig::RigKind,
    ) -> Result<serde_json::Value, String> {
        if !path.is_file() {
            return Err(format!("no image file at {}", path.display()));
        }
        let missing = buzz_ui::ai_character_dialog::missing_character_models(|m| {
            buzz_ai::models::status(m).is_ready()
        });
        if !missing.is_empty() {
            return Err(format!(
                "Character from Image needs models that are not installed: {}. Download them in AI \u{25b8} Models, then call ai_character_from_image again.",
                missing.join(", ")
            ));
        }
        let (send, receive) = crossbeam_channel::bounded(1);
        self.last_ai_task = None;
        let shown = name.clone();
        let limits = crate::rig_limits::load_or_default();
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI: Character from Image", move |ctx| {
                let progress = ctx.progress.clone();
                let mut step = 0u64;
                let mut note = |what: &str| {
                    step += 1;
                    progress.set(step, 3);
                    progress.detail(what);
                };
                match buzz_ui::ai_character_dialog::character_from_image(&path, &name, Some(&limits), &mut note) {
                    Ok(assembled) => {
                        let _ = send.send(AiResult::Character(Box::new(assembled), kind));
                        crate::tasks::TaskOutcome::Finished(String::new())
                    }
                    Err(e) => crate::tasks::TaskOutcome::Failed(e),
                }
            });
        self.track_ai(id, receive);
        let job = self.agent_track_last_job("ai_character_from_image").ok_or("the character task did not start")?;
        Ok(crate::agent::jobs::started(
            "ai_character_from_image",
            job,
            &format!("'{shown}' is being detected, segmented, completed and rigged off the UI thread (this takes a minute or two)"),
        ))
    }

    /// **Produce an episode** (P6) as one AI task in the Tasks panel: the
    /// producer's step and line as the task's detail, Cancel between steps.
    /// `open`: open the `.buzz` when it is done (the dialog's run).
    fn start_produce(&mut self, req: crate::produce::ProduceRequest, open: bool) -> crate::tasks::TaskId {
        let (send, receive) = crossbeam_channel::bounded(1);
        self.last_ai_task = None;
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI: Produce Episode", move |ctx| {
                let progress = ctx.progress.clone();
                let cancel = ctx.cancel.clone();
                let total = crate::produce::Step::ALL.len() as u64;
                let mut on_progress = |step: crate::produce::Step, line: &str| {
                    progress.set(step.index() as u64, total);
                    progress.detail(format!("{}: {line}", step.label()));
                };
                match crate::produce::produce(&req, &mut on_progress, &|| cancel.is_cancelled()) {
                    Ok(report) => {
                        let _ = send.send(AiResult::Produced { report: Box::new(report), open });
                        crate::tasks::TaskOutcome::Finished(String::new())
                    }
                    Err(_) if cancel.is_cancelled() => crate::tasks::TaskOutcome::Cancelled,
                    Err(e) => crate::tasks::TaskOutcome::Failed(format!("Produce Episode: {e:#}")),
                }
            });
        self.track_ai(id, receive);
        id
    }

    /// The dialog behind File ▸ AI ▸ Produce Episode…
    fn produce_dialog(&mut self, ctx: &egui::Context) {
        // A run that ended without a result (cancelled, failed) frees the button.
        if self.produce.busy && self.produce_task.is_some_and(|t| !self.tasks.is_running(t)) {
            self.produce.busy = false;
            self.produce_task = None;
        }
        match buzz_ui::produce_dialog::produce_dialog(ctx, &mut self.produce) {
            buzz_ui::produce_dialog::ProduceDialogAction::Produce => {
                let state = &self.produce;
                let scenes = match state.scene_numbers() {
                    Ok(s) => s,
                    Err(e) => {
                        self.produce.status = Some(e);
                        return;
                    }
                };
                let script = std::path::PathBuf::from(state.script.trim());
                if !script.is_file() {
                    self.produce.status = Some(format!("No script file at {}", script.display()));
                    return;
                }
                let mut req = crate::produce::ProduceRequest::new(script, std::path::PathBuf::from(state.out.trim()));
                let optional = |t: &str| Some(t.trim()).filter(|t| !t.is_empty()).map(std::path::PathBuf::from);
                req.audio = optional(&state.audio);
                req.characters_dir = optional(&state.characters);
                let preset = buzz_ui::produce_dialog::STYLE_PRESETS.get(state.style).copied().unwrap_or("comic");
                let mut style = crate::backdrop::Style::by_name(preset).unwrap_or_default();
                if !state.style_text.trim().is_empty() {
                    style.suffix = format!("{}, {}", style.suffix, state.style_text.trim());
                }
                req.style = style;
                req.series = state.series.trim().to_string();
                req.world = if state.world.trim().is_empty() {
                    buzz_ui::produce_dialog::default_world(&req.series).to_string()
                } else {
                    state.world.trim().to_string()
                };
                req.scenes = scenes;
                req.backgrounds = state.backgrounds;
                req.launch_comfy = state.backgrounds && state.launch_comfy;
                req.mp4 = state.mp4;
                let id = self.start_produce(req, true);
                self.produce_task = Some(id);
                self.produce.busy = true;
                self.produce.status = Some("Producing: see the Tasks panel (Cancel is there)".into());
            }
            buzz_ui::produce_dialog::ProduceDialogAction::Cancel | buzz_ui::produce_dialog::ProduceDialogAction::None => {}
        }
    }

    /// The Assistant's `produce_episode` (P6): the producer as a job.
    pub(crate) fn agent_produce_async(&mut self, req: crate::produce::ProduceRequest) -> Result<serde_json::Value, String> {
        let out = req.out.display().to_string();
        self.start_produce(req, false);
        let job = self.agent_track_last_job("produce_episode").ok_or("the producer task did not start")?;
        Ok(crate::agent::jobs::started(
            "produce_episode",
            job,
            &format!("the episode is being produced into {out} off the UI thread (minutes: breakdown, cast, backgrounds, audio, build, render)"),
        ))
    }

    /// Add the Assistant's finished character to the document (one undo step,
    /// and into the Cast folder, exactly as **Create Character** does) and say
    /// what arrived, for `wait_for_jobs`. `kind` is the call's `rig`: a layer
    /// rig has no armature, so `armature_element` is null and
    /// `rig_root_element` names its root part (an armature's id for bones).
    fn commit_agent_character(
        &mut self,
        assembled: &buzz_ai::character::AssembledCharacter,
        kind: buzz_rig::RigKind,
    ) -> serde_json::Value {
        use serde_json::json;
        let frame = self.editor.current_frame;
        let scene = self.editor.doc.scene();
        let symbols_before: std::collections::HashSet<buzz_scene::SymbolId> =
            scene.library().iter().map(|s| s.id).collect();
        let objects_before: std::collections::HashSet<buzz_scene::ObjectId> = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(frame).iter().map(|o| o.id))
            .collect();
        let depth_before = self.editor.doc.history().undo_depth();

        self.editor.create_ai_character_as(assembled, kind);

        let scene = self.editor.doc.scene();
        let symbols: Vec<serde_json::Value> = scene
            .library()
            .iter()
            .filter(|s| !symbols_before.contains(&s.id))
            .map(|s| json!({"id": s.id.0, "name": s.name}))
            .collect();
        let mut elements = Vec::new();
        // The root part of a layer rig: on a layer that others follow and
        // that follows nothing itself.
        let mut layer_root = None;
        for (index, layer) in scene.layers().iter().enumerate() {
            let is_root = layer.follows.is_none() && !scene.layers().followers_of(layer.id).is_empty();
            for o in layer.objects_at(frame).iter().filter(|o| !objects_before.contains(&o.id)) {
                let kind = match &o.kind {
                    buzz_scene::ObjectKind::Armature(_) => "armature",
                    buzz_scene::ObjectKind::Instance(_) => "instance",
                    _ => "other",
                };
                if is_root && layer_root.is_none() {
                    layer_root = Some(json!(o.id.0));
                }
                elements.push(json!({"id": o.id.0, "kind": kind, "layer": index, "layer_name": layer.name}));
            }
        }
        let rig = elements.iter().find(|e| e["kind"] == "armature").map(|e| e["id"].clone());
        // What was actually built: a skinned rig refuses to be layers and
        // stays bones, so read it from the stage rather than from `kind`.
        let (rig_kind, rig_root) = match (&rig, layer_root) {
            (Some(id), _) => (buzz_rig::RigKind::Bones, Some(id.clone())),
            (None, Some(root)) => (buzz_rig::RigKind::Layers, Some(root)),
            (None, None) => (kind, None),
        };
        json!({
            "character": assembled.name,
            "symbols": symbols,
            "elements": elements,
            // A bone rig's armature (null for a layer rig).
            "armature_element": rig,
            "rig_kind": rig_kind.label(),
            // The rigged character on stage: pass this id as `element` to perform / ai_direct_story.
            "rig_root_element": rig_root,
            "undo_steps_added": self.editor.doc.history().undo_depth() as i64 - depth_before as i64,
        })
    }

    /// Test seam for the job plumbing: a task that does `work` on a thread of
    /// its own, tracked as an Assistant job exactly as the AI tools' are.
    #[cfg(test)]
    pub(crate) fn test_stub_job<F>(&mut self, kind: &str, work: F) -> u64
    where
        F: FnOnce(crate::tasks::TaskCtx) -> crate::tasks::TaskOutcome + Send + 'static,
    {
        self.last_ai_task = None;
        let id = self.tasks.spawn_thread(crate::tasks::TaskKind::Ai, "AI: stub", work);
        // No result channel: `collect_ai_result` has nothing to take, and
        // `settle_agent_job` records how the task ended.
        self.last_ai_task = Some(id);
        self.agent_track_last_job(kind).expect("the stub job started")
    }

    /// Render the character just created from an image in the standard
    /// poses, on a thread, for the panel's pose sheet (F7.0).
    fn ai_render_pose_sheet(&mut self) {
        let frame = self.editor.current_frame;
        let scene = self.editor.doc.scene().clone();
        // A bone rig or a layer rig (the default since LR2), found by `rig_of`.
        let Some(subject) = pose_sheet_subject(&scene, frame) else {
            return;
        };
        let (send, receive) = crossbeam_channel::bounded(1);
        let id = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI: Pose sheet", move |_ctx| {
                match render_pose_sheet_of(&scene, &subject, frame) {
                    Ok(images) => {
                        let _ = send.send(AiResult::PoseSheet(images));
                        crate::tasks::TaskOutcome::Finished(String::new())
                    }
                    Err(e) => crate::tasks::TaskOutcome::Failed(format!("Pose sheet: {e}")),
                }
            });
        self.track_ai(id, receive);
    }

    /// Collect whatever the background tasks have finished.
    ///
    /// Everything a task has to say arrives here, once, on the UI thread — so
    /// applying a result is an ordinary edit at an ordinary moment rather than
    /// another thread reaching into the document.
    fn poll_tasks(&mut self) {
        for (id, kind, outcome) in self.tasks.poll() {
            let for_job = (kind == crate::tasks::TaskKind::Ai
                && self.agent_jobs.iter().any(|j| j.task == id))
            .then(|| outcome.clone());
            // Exports are handled in one place because all three outcomes have
            // to be recorded against the queue — success, cancel and failure
            // alike free it for the next export and get a row in the panel.
            if kind == crate::tasks::TaskKind::Export {
                let (ok, message) = match outcome {
                    crate::tasks::TaskOutcome::Finished(m) => (true, m),
                    crate::tasks::TaskOutcome::Cancelled => (false, "Export cancelled".to_string()),
                    crate::tasks::TaskOutcome::Failed(why) => (false, why),
                };
                self.exports.complete(id, ok, message.clone());
                self.api.export_finished(id, ok, &message);
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
            if let Some(outcome) = for_job {
                self.settle_agent_job(id, &outcome);
            }
        }
    }

    /// A task got where it was going.
    ///
    /// The message is what the task wants said; anything it *produced* it left
    /// in a slot of its own, which is what the per-kind arms collect.
    fn finish_task(
        &mut self,
        id: crate::tasks::TaskId,
        kind: crate::tasks::TaskKind,
        message: String,
    ) {
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
            crate::tasks::TaskKind::Ai => {
                self.collect_ai_result(id);
                // `collect_ai_result` sets the status itself.
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
            crate::tasks::TaskKind::Warm => {
                // The warm-up is gone; what it built is dropped, unread. Said
                // nothing about: the app cancels it for a tab switch or a zoom,
                // and none of that is the user's business.
                if self.warm.running.as_ref().is_some_and(|r| r.task == id) {
                    self.warm.running = None;
                }
                if !std::mem::take(&mut self.warm.superseded) {
                    // Stopped from the Tasks panel (or it fell over): not again.
                    self.warm.declined = true;
                }
                return;
            }
            crate::tasks::TaskKind::Ai => {
                // Whatever the task built is dropped, unread — the document
                // stays exactly as it was.
                self.ai_results.remove(&id);
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
            PresetFormat::Mp4H264
            | PresetFormat::Mp4Hevc
            | PresetFormat::Mp4Av1
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

    /// **Ask for a test render** at whatever the Export dialog currently says.
    ///
    /// The same scenes, the same range and the same size the export itself
    /// would use — reading them from anywhere else would be a preview of a
    /// different film.
    fn request_preview(&mut self) {
        let scenes = self.editor.doc.film_with_lengths();
        if scenes.is_empty() {
            return;
        }
        self.preview.request(crate::preview::Request {
            range: self.editor.export.range(),
            width: self.editor.export.width,
            height: self.editor.export.height,
            scenes,
        });
    }

    /// Draw the Export dialog and act on what the user chose.
    fn export_dialog(&mut self, ctx: &egui::Context) {
        let names = self.presets.names();
        let shots: Vec<buzz_ui::PreviewShot> = self
            .preview
            .shots()
            .iter()
            .map(|shot| buzz_ui::PreviewShot {
                frame: shot.frame,
                texture: shot.id,
                size: shot.size(),
            })
            .collect();
        let message = self.preview.message.clone();
        let response = buzz_ui::export_dialog(
            ctx,
            &mut self.editor.export,
            &names,
            &shots,
            message.as_deref(),
        );

        if response.preview {
            self.request_preview();
        }
        // A dialog that has been closed has nothing to show, and its textures
        // are a megabyte nobody is looking at.
        if response.cancelled || response.confirmed {
            self.preview.clear();
        }

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

        // A render region: the selection's bounds at the playhead, when asked
        // for and there is a selection. Otherwise the whole stage.
        let region = if self.editor.export.region_to_selection {
            self.editor
                .selection
                .bounds_at(self.editor.scene(), self.editor.current_frame)
        } else {
            None
        };
        let settings = buzz_export::ExportSettings {
            width: self.editor.export.width,
            height: self.editor.export.height,
            transparent: self.editor.export.transparent,
            region,
        };
        // **The whole film, not the scene being edited.** Snapshots, so the
        // export renders the document as it was when the user asked — and they
        // can keep editing, or queue another, while it writes. A document with
        // one scene is one scene, exactly as before.
        let scenes = self.editor.doc.film_with_lengths();
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
                let frame = self
                    .editor
                    .doc
                    .film_start_of(self.editor.doc.active_scene())
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

    /// Start as many queued exports as there are free running slots.
    ///
    /// The cap is the workspace's **Exports at once**, held under the VRAM cap
    /// ([`crate::export_service::vram_cap`]). That is sampled only when the
    /// queue goes from idle to busy, never per job: a job that has just started
    /// has not allocated its VRAM yet.
    fn pump_export_queue(&mut self) {
        if !self.exports.is_active() && self.exports.waiting() > 0 {
            self.export_vram_cap = crate::export_service::vram_cap();
        }
        self.exports.set_max_parallel(
            (self.editor.workspace.exports_at_once as usize)
                .min(self.export_vram_cap)
                .max(1),
        );
        while let Some(request) = self.exports.next_to_start() {
            let reveal = request.reveal_path();
            let label = request.label.clone();
            let id =
                self.tasks
                    .spawn_thread(crate::tasks::TaskKind::Export, label.clone(), move |ctx| {
                        crate::export_service::run_export(request, &ctx)
                    });
            self.api.export_started(id, &reveal);
            self.exports.started(id, reveal, label);
        }
    }

    /// The local API (`crate::api`) queues an episode's previews here, beside
    /// the user's own exports.
    pub(crate) fn api_enqueue_export(&mut self, request: crate::export_service::ExportRequest) {
        self.exports.enqueue(request);
    }

    /// The GPU choice exports are made with, for the local API's own requests.
    pub(crate) fn api_gpu(&self) -> GpuPreference {
        self.preference.clone()
    }

    /// How far a running task has got, for the local API's job reports.
    pub(crate) fn api_task_progress(&self, id: crate::tasks::TaskId) -> Option<crate::tasks::TaskProgress> {
        self.tasks.running().find(|t| t.id == id).map(|t| t.progress())
    }

    /// Animate's File ▸ Import Image.
    fn import_image_dialog(&mut self) {
        self.ask_for_paths(
            crate::dialogs::Request::open_files()
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

    /// D3: several pictures chosen at once. A single file behaves exactly as
    /// it always did; more than one lands staggered, each its own object, so
    /// a batch does not stack every picture exactly on top of the last.
    fn import_images_from(&mut self, mut paths: Vec<std::path::PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if paths.len() == 1 {
            self.import_image_from(paths.remove(0));
            return;
        }
        let total = paths.len();
        let mut failed: Vec<(String, String)> = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            if let Err(e) = self.editor.import_image_at(path, Self::stagger_offset(index)) {
                failed.push((file_name(path), format!("{e:#}")));
            }
        }
        self.editor.status = Some(Self::import_batch_summary(total, &failed));
    }

    /// Where the Nth picture of a batch lands, relative to the camera centre
    /// — a diagonal cascade (D3), so each one is findable rather than hidden
    /// under whichever landed on top of it.
    fn stagger_offset(index: usize) -> (f64, f64) {
        const STEP: f64 = 24.0;
        let step = index as f64 * STEP;
        (step, step)
    }

    /// D3's one status line, shared by every multi-file import route that
    /// does not go through the async merge queue (`finish_import_batch` is
    /// that queue's own version of this): "Imported N files", or "Imported N
    /// files, K failed: name (reason), ...". A failure never stops the rest.
    fn import_batch_summary(total: usize, failed: &[(String, String)]) -> String {
        if failed.is_empty() {
            format!("Imported {total} files")
        } else {
            let reasons = failed
                .iter()
                .map(|(name, reason)| format!("{name} ({reason})"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Imported {total} files, {} failed: {reasons}",
                failed.len()
            )
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

    /// Pick a folder of numbered drawings.
    fn sequence_folder_dialog(&mut self) {
        self.ask_for_path(
            crate::dialogs::Request::folder().title("Choose a folder of numbered pictures"),
            Pick::SequenceFolder,
        );
    }

    fn sequence_folder_from(&mut self, path: std::path::PathBuf) {
        if let Err(e) = self.editor.import_image_sequence(&path) {
            self.editor.status = Some(format!("Could not import that sequence: {e:#}"));
        }
    }

    /// Pick a video to rotoscope over.
    fn video_reference_dialog(&mut self) {
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Video", &["mp4", "mov", "m4v", "avi", "mkv", "webm", "gif"]),
            Pick::VideoReference,
        );
    }

    fn video_reference_from(&mut self, path: std::path::PathBuf) {
        // Pulling a video apart takes seconds, not milliseconds; saying so
        // before it starts is the difference between "working" and "hung".
        self.editor.status = Some("Reading the video\u{2026}".into());
        if let Err(e) = self.editor.import_video_reference(&path) {
            self.editor.status = Some(format!("Could not read that video: {e:#}"));
        }
    }

    /// Animate's File ▸ Import Sound.
    fn import_sound_dialog(&mut self) {
        self.ask_for_paths(
            crate::dialogs::Request::open_files()
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
                    format!("Imported {name} — put it on a keyframe with Control > Attach Sound")
                })
            }
            Err(e) => self.editor.status = Some(format!("Could not import that sound: {e:#}")),
        }
    }

    /// D3: several sounds chosen at once. A single file behaves exactly as it
    /// always did; more than one is imported in turn, each on a layer of its
    /// own (`Editor::import_sound` already gives every call its own layer).
    fn import_sounds_from(&mut self, mut paths: Vec<std::path::PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if paths.len() == 1 {
            self.import_sound_from(paths.remove(0));
            return;
        }
        let total = paths.len();
        let mut failed: Vec<(String, String)> = Vec::new();
        for path in &paths {
            if let Err(e) = self.editor.import_sound(path) {
                failed.push((file_name(path), format!("{e:#}")));
            }
        }
        self.editor.status = Some(Self::import_batch_summary(total, &failed));
    }

    /// Sequence \u{25b8} Set Master Audio\u{2026} — see
    /// `docs/SEQUENCE_SETUPS_PLAN.md` "The audio is the timeline".
    fn set_master_audio_dialog(&mut self) {
        self.ask_for_path(
            crate::dialogs::Request::open_file()
                .filter("Sound", &["wav", "mp3", "ogg", "flac", "m4a", "aac"]),
            Pick::SetMasterAudio,
        );
    }

    fn set_master_audio_from(&mut self, path: std::path::PathBuf) {
        match self.editor.set_master_audio_from_file(&path) {
            Ok(name) => {
                self.editor.status =
                    Some(format!("{name} is the master audio \u{2014} the Sequence strip shows it"))
            }
            Err(e) => {
                self.editor.status = Some(format!("Could not set that as the master audio: {e:#}"))
            }
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
        // The Assistant must not keep editing a document that is going away.
        self.agent_abort("The app is closing, so the Assistant stopped.");
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
                let headline = format!(
                    "{} \u{201C}{}\u{201D}{percent}.",
                    task.kind.label(),
                    task.label
                );
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
                ui.label(egui::RichText::new(format!("And {more} more waiting behind it.")).weak());
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Quitting now stops it and deletes the partly-written file.")
                    .weak(),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Keep waiting").clicked() {
                    keep_waiting = true;
                }
                if ui
                    .button(
                        egui::RichText::new("Stop export and quit")
                            .color(egui::Color32::from_rgb(0xE5, 0x53, 0x53)),
                    )
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
            exports_at_once: self.editor.workspace.exports_at_once,
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
                buzz_ui::TaskAction::ExportsAtOnce(n) => {
                    self.editor.workspace.exports_at_once = n;
                    self.editor.workspace.save();
                }
            }
        }
    }

    /// Animate's File ▸ Import, for all three formats.
    ///
    /// Each file is one [`Document::edit`], so one that brings in four
    /// hundred symbols is still a single Ctrl+Z — and choosing several at
    /// once (D3) merges them the same way, one after another.
    fn import_dialog(&mut self, target: buzz_scene::ImportTarget) {
        // Sound sits in the "everything" filter too, or the one filter that is
        // meant to accept anything would be the one that hid the mp3.
        let mut importable = crate::import::IMPORTABLE.to_vec();
        importable.extend_from_slice(crate::import::AUDIBLE);
        self.ask_for_paths(
            crate::dialogs::Request::open_files()
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
            // Synchronous — no read-and-merge task follows it, so nothing
            // else here would ever move a batch (D3) on to the next file.
            if self.import_batch.is_some() {
                self.advance_import_queue();
            }
            return;
        }
        if self.loading_already() {
            // Already reading another file, which a serial queue (D3) never
            // asks for — this is only reached if something outside the queue
            // (a plain File ▸ Import) raced it. Recorded as a failure rather
            // than silently dropping the file, so the batch's own count
            // still adds up, and the rest still runs.
            if self.import_batch.is_some() {
                let name = file_name(&path);
                if let Some(batch) = &mut self.import_batch {
                    batch
                        .failed
                        .push((name, "another file was already being read".into()));
                }
                self.advance_import_queue();
            }
            return;
        }
        let name = file_name(&path);
        let reading = path.clone();

        self.start_load(crate::tasks::TaskKind::Import, name, move |ctx| {
            ctx.progress
                .detail(format!("Reading {}", file_name(&reading)));
            Loaded::Foreign {
                result: crate::import::read(&reading).map(Box::new),
                path: reading,
                target: Some(target),
            }
        });
    }

    /// D3: one or more files chosen at once for Import to Stage or Import to
    /// Library. A single file behaves exactly as it always did — no batch, no
    /// summary line beyond the one `finish_merge` already writes. More than
    /// one queues the rest and merges them in turn, each its own undo step
    /// (only one read-and-merge pipeline is ever in flight; see
    /// `advance_import_queue`).
    fn start_batch_import(
        &mut self,
        target: buzz_scene::ImportTarget,
        mut paths: Vec<std::path::PathBuf>,
    ) {
        if paths.is_empty() {
            return;
        }
        if paths.len() == 1 {
            self.import_file(target, paths.remove(0));
            return;
        }
        self.import_batch = Some(ImportBatch {
            total: paths.len(),
            failed: Vec::new(),
        });
        self.import_queue = paths.into_iter().map(|p| (target, p)).collect();
        self.advance_import_queue();
    }

    /// Start the next queued import, or — the queue empty — report the batch.
    fn advance_import_queue(&mut self) {
        let Some((target, path)) = self.import_queue.pop_front() else {
            self.finish_import_batch();
            return;
        };
        self.import_file(target, path);
    }

    /// The one status line D3 asks for: how many files, and — if any — which
    /// failed and why. Only reached for a batch of more than one file; a
    /// single import already gets its own message from `finish_merge`.
    fn finish_import_batch(&mut self) {
        let Some(batch) = self.import_batch.take() else {
            return;
        };
        self.editor.status = Some(Self::import_batch_summary(batch.total, &batch.failed));
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

        // D3: part of a multi-file import — carry on to the next queued file,
        // or (the queue empty) replace the per-file status above with the
        // one line summarising the whole batch.
        if self.import_batch.is_some() {
            self.advance_import_queue();
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

    /// **Test Movie's overlay**: a scrim over the whole window, the picture at
    /// the stage's aspect, a caption and a Close button.
    ///
    /// A full-window `egui::Area` at `Order::Foreground` rather than an
    /// `egui::Window`: this is playback, not a dialog — there is nothing to
    /// drag, resize or click through, and drawing it above every other layer
    /// this way is what keeps the stage underneath from taking the click.
    fn test_movie_overlay(&mut self, ctx: &egui::Context) {
        let Some(movie) = self.test_movie.as_mut() else {
            return;
        };

        let screen = ctx.input(|i| i.viewport_rect());
        let scale = ctx.pixels_per_point().max(0.01);
        const MARGIN: f32 = 64.0;
        const CAPTION_HEIGHT: f32 = 40.0;
        let available = egui::vec2(
            (screen.width() - MARGIN * 2.0).max(1.0),
            (screen.height() - MARGIN * 2.0 - CAPTION_HEIGHT).max(1.0),
        );
        movie.set_view_size(
            (available.x * scale).round() as u32,
            (available.y * scale).round() as u32,
        );

        let texture = movie.texture();
        let (frame, frames) = (movie.frame(), movie.frames());
        let title = self.editor.doc.title();

        // Consumed here so nothing else the frame also reads Escape for — a
        // tool deselecting, a dialog closing — fires underneath it.
        let escape_pressed =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        let mut close_clicked = false;
        egui::Area::new(egui::Id::new("test-movie-overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .movable(false)
            .interactable(true)
            .show(ctx, |ui| {
                // The scrim: catches every click, so the stage beneath cannot
                // be edited while the film plays.
                ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(235));

                ui.scope_builder(egui::UiBuilder::new().max_rect(screen), |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(MARGIN);
                        match texture {
                            Some((id, w, h)) => {
                                let size = egui::vec2(w as f32, h as f32) / scale;
                                ui.add(egui::Image::new((id, size)));
                            }
                            None => {
                                ui.add_space((available.y / 2.0 - 10.0).max(0.0));
                                ui.colored_label(egui::Color32::WHITE, "Rendering\u{2026}");
                            }
                        }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                egui::Color32::WHITE,
                                format!(
                                    "Test Movie \u{2014} {title} \u{2014} frame {}/{}",
                                    frame + 1,
                                    frames.max(1),
                                ),
                            );
                            if ui.button("Close").clicked() {
                                close_clicked = true;
                            }
                        });
                    });
                });
            });

        if escape_pressed || close_clicked {
            self.toggle_test_movie();
        }
    }

    /// File ▸ Export ▸ Animate Document.
    ///
    /// Named after the document, so a film called `hero.buzz` is offered as
    /// `hero.fla` rather than as `untitled`.
    fn export_fla_dialog(&mut self) {
        let suggested = self.editor.doc.title();
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
        // **A multi-select dialog** (D2): several files chosen at once each
        // open in their own tab — see `App::poll_picker_many`'s `Pick::Open`
        // arm and `App::advance_open_queue`.
        self.ask_for_paths(
            crate::dialogs::Request::open_file()
                .filter("Everything BuzzAnimate can open", &everything)
                .filter("BuzzAnimate document", &[buzz_doc::EXTENSION])
                .filter("Animate document", &["fla", "xfl"])
                .filter("Flash movie", &["swf"])
                .filter("PDF or Illustrator artwork", &["pdf", "ai"]),
            Pick::Open,
        );
    }

    /// **D2.** Open `doc` as a new tab, leaving every other open document —
    /// and the *shared* state (`workspace`, `style`/`tool_style`, the tool in
    /// use, the panels, the command library, `assets`) — exactly as it was.
    ///
    /// Replaces the old single-document `adopt_document`, which rebuilt the
    /// whole `Editor` (and so the tool/style/panels along with it) because
    /// there was never more than one document to keep those shared *across*.
    /// Only what belongs to the document (see [`crate::editor::DocView`]) is
    /// fresh for the incoming tab.
    fn open_document_in_new_tab(&mut self, doc: Document) {
        let view = crate::editor::DocView::fresh(&doc);
        self.documents.push(OpenDocument { doc, view });
        let index = self.documents.len() - 1;
        self.switch_to_tab(index);
    }

    /// The document showing in tab `i` — the live one in `self.editor` if
    /// `i` is the active tab, else the one parked on its `OpenDocument`. The
    /// one place the "the active slot is a placeholder" rule
    /// (`App::documents`) has to be read around.
    fn tab_document(&self, i: usize) -> &Document {
        if i == self.active_document {
            &self.editor.doc
        } else {
            &self.documents[i].doc
        }
    }

    /// Make `index` the active tab: park whatever is live in `self.editor`
    /// back on its own `OpenDocument`, and swap the tab at `index` into the
    /// editor in its place. A no-op if `index` is already active.
    fn switch_to_tab(&mut self, index: usize) {
        if index >= self.documents.len() || index == self.active_document {
            return;
        }
        let incoming_doc = std::mem::take(&mut self.documents[index].doc);
        let incoming_view = std::mem::take(&mut self.documents[index].view);
        let (outgoing_doc, outgoing_view) = self.activate_document(incoming_doc, incoming_view);
        self.documents[self.active_document] = OpenDocument {
            doc: outgoing_doc,
            view: outgoing_view,
        };
        self.active_document = index;
    }

    /// Swap `doc`/`view` into the editor, flush the outgoing document's
    /// autosave and crash snapshot if it is dirty (see the note below), and
    /// reset the per-document caches that must not leak between tabs.
    /// Returns what was swapped out, for the caller to park or drop.
    fn activate_document(
        &mut self,
        doc: Document,
        view: crate::editor::DocView,
    ) -> (Document, crate::editor::DocView) {
        let (mut outgoing_doc, outgoing_view) = self.editor.swap_document(doc, view);

        // **A background tab cannot change** — every edit runs through
        // `self.editor.doc`, which is only ever the *active* document — so
        // the one moment a parked tab's autosave and crash snapshot need
        // bringing up to date is right here, as it stops being active. A
        // periodic per-tab tick would do the same thing far more often for
        // no benefit, since nothing can touch it again until it is focused.
        if outgoing_doc.is_dirty() {
            outgoing_doc.remember_for_crash();
            if let Some(plan) = outgoing_doc.autosave_plan() {
                self.jobs.spawn(Pool::Background, move || {
                    if let Err(e) = plan.write() {
                        tracing::warn!("autosave failed: {e}");
                    }
                });
            }
        }

        // Every cache below is keyed only by `Document::combined_revision`,
        // and two unrelated documents can innocently land on the same
        // revision number (two blank Untitled tabs both start at 0) — so a
        // switch must throw them away rather than let a coincidence show one
        // tab's geometry on another's. See "D2 design" in
        // docs/DOCUMENTS_PLAN.md for what this deliberately does *not* cover
        // (in-flight shading/sound-decode jobs).
        self.stage_stamp = None;
        self.scroll_extent = None;
        self.bounds_table = None;
        self.reset_light_cache();
        self.thumbnails.clear();
        // S9: the incoming document is prepared for playback from scratch.
        self.warm_activated();

        (outgoing_doc, outgoing_view)
    }

    /// **Replace `self.lights` with a clean cache that still knows this
    /// window's GPU limits.** `fresh_light_cache()` alone starts at scale 1
    /// with the default `max_scale` of 1, so after a document open or tab
    /// switch `DrawCache::reconsider` could never grow the buffer scale again
    /// and every heavy frame fell through to band mode for good (S3 root cause
    /// in `docs/BIMARI5_LAG_PLAN.md`). The GPU's live `buffer_scale` and
    /// `max_buffer_scale` are the truth when there is a window; with none (the
    /// headless tests) the outgoing cache's own values carry over. Band mode
    /// restarts too — the incoming document is a different frame.
    fn reset_light_cache(&mut self) {
        // The real cache may be on the encode thread: take it home first, so
        // the swap below replaces the cache and not its stand-in.
        self.stop_encode_ahead();
        let (scale, max_scale) = match &self.active {
            Some(active) => (active.gpu.buffer_scale, active.gpu.max_buffer_scale),
            None => (self.lights.scale(), self.lights.max_scale()),
        };
        self.lights = fresh_light_cache();
        self.lights.set_scale(scale);
        self.lights.set_max_scale(max_scale);
        self.stage_bands = 0;
        self.band_frames_since_ladder = 0;
        self.banded_on_target = None;
    }

    /// **D2.** Start `path` reading, as our own format or a foreign one —
    /// the same choice `poll_picker`'s old `Pick::Open` arm made, factored
    /// out so `App::advance_open_queue` can make it for each queued path in
    /// turn.
    fn open_path(&mut self, path: &std::path::Path) {
        if opens_as_document(path) {
            self.open_buzz(path);
        } else {
            self.open_imported(path);
        }
    }

    /// **D2.** If `path` is already open in some tab, focus it and say so.
    /// Animate does not open a second window onto a file already on screen.
    fn focus_tab_for_path(&mut self, path: &std::path::Path) -> bool {
        let Some(index) = (0..self.documents.len())
            .find(|&i| self.tab_document(i).path() == Some(path) || self.tab_document(i).origin() == Some(path))
        else {
            return false;
        };
        self.switch_to_tab(index);
        self.editor.status = Some(format!("Switched to {}", file_name(path)));
        true
    }

    /// **D2.** Work through `open_queue` one path at a time: a path already
    /// open just focuses its tab; anything else starts a background read,
    /// and waits — only one runs at once (`App::loading_already`) — for
    /// `App::collect_loaded_document` to call this again once it lands.
    fn advance_open_queue(&mut self) {
        while let Some(path) = self.open_queue.pop_front() {
            if self.focus_tab_for_path(&path) {
                continue;
            }
            self.open_path(&path);
            return;
        }
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
            ctx.progress
                .detail(format!("Reading {}", file_name(&reading)));
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
            ctx.progress
                .detail(format!("Reading {}", file_name(&reading)));
            Loaded::Foreign {
                result: crate::import::read(&reading).map(Box::new),
                path: reading,
                target: None,
            }
        });
    }

    /// A foreign file finished reading and is to become **a new tab** (D2).
    fn finish_open_imported(&mut self, path: &std::path::Path, imported: crate::import::Imported) {
        let name = file_name(path);

        let mut doc = imported.document().with_origin(path);
        doc.mark_clean();
        self.open_document_in_new_tab(doc);
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

    /// The Save As dialog request for the current document: named after the
    /// document's own title (its saved path, or the file it was imported
    /// from), and opened in the origin's folder for a not-yet-saved import so
    /// the `.buzz` lands next to the `.fla` it came from.
    fn save_request(&self) -> crate::dialogs::Request {
        let doc = &self.editor.doc;
        let name = format!("{}.{}", doc.title(), buzz_doc::EXTENSION);
        let mut request = crate::dialogs::Request::save_file()
            .filter("BuzzAnimate document", &[buzz_doc::EXTENSION])
            .file_name(name);
        if doc.path().is_none() {
            if let Some(origin_dir) = doc.origin().and_then(|o| o.parent()) {
                request = request.directory(origin_dir);
            }
        }
        request
    }

    fn save(&mut self, force_dialog: bool) {
        if force_dialog || self.editor.doc.path().is_none() {
            // Save As opens on the document's own name, not on "untitled":
            // saving a variant of a file is what Save As is mostly for, and
            // retyping the name every time is the thing that makes it tedious.
            self.ask_for_path(self.save_request(), Pick::SaveAs);
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
                        self.open_document_in_new_tab(doc);

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

    /// Draw the Trace Bitmap dialog and act on what was chosen. `Some` only
    /// while it is open — see `Command::TraceBitmapOptions`.
    fn trace_dialog(&mut self, ctx: &egui::Context) {
        let Some(options) = self.editor.trace_dialog.as_mut() else {
            return;
        };
        match buzz_ui::trace_dialog(ctx, options) {
            buzz_ui::TraceDialogAction::None => {}
            buzz_ui::TraceDialogAction::Trace => {
                let o = *options;
                self.editor.confirm_trace_options(o);
            }
            buzz_ui::TraceDialogAction::Cancel => {
                self.editor.trace_dialog = None;
            }
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

    /// An [`Ahead`] over this app's own fields (for callers that do not hold
    /// `self.active`; `render` builds its own).
    fn ahead(&mut self) -> Ahead<'_> {
        Ahead {
            pipeline: &mut self.pipeline,
            lights: &mut self.lights,
            editor: &self.editor,
            stage_bands: self.stage_bands,
            test_movie_open: self.test_movie.is_some(),
            stage_stale: self.stage_stale,
            shade_building: self.shade_build.is_some(),
        }
    }

    /// Take the stage cache home from the encode thread (a no-op when it is
    /// home). See [`Ahead::stop`].
    fn stop_encode_ahead(&mut self) {
        self.ahead().stop();
    }

    /// **S9: a document was just activated** (opened or switched to): whatever
    /// warm-up was running belonged to another one, and this one has not been
    /// prepared yet.
    fn warm_activated(&mut self) {
        if let Some(r) = &self.warm.running {
            self.warm.superseded = true;
            self.tasks.cancel(r.task);
        }
        self.warm.activation += 1;
        self.warm.needs = self.warm.on;
        self.warm.declined = false;
        self.warm.warmed_level = None;
        self.warm.settle = None;
        self.warm.last = None;
    }

    /// **S9: prepare the open document for playback.** Called at the top of
    /// every frame (and by tests) with the stage rectangle in physical pixels
    /// and the output scale. Collects a finished warm-up (installs its stamps,
    /// sizes the buffers once), cancels one whose camera level has moved on,
    /// and starts one when the document needs it or the zoom has settled
    /// somewhere not warmed: encode-only, on the worker pool, behind a Tasks
    /// panel progress bar. Off unless `BUZZ_WARM_OPEN=1` (`warm_open::enabled`).
    fn tick_warm(&mut self, area_px: buzz_geom::Rect, scale: f64) {
        use crate::warm_open as warm;
        if !self.warm.on {
            return;
        }
        let level = self.editor.camera.zoom * scale;
        let pixels = area_px.width() * area_px.height();

        // 1. A finished warm-up.
        let got = self.warm.running.as_ref().map(|r| r.rx.try_recv());
        if let Some(Ok(out)) = got
            && let Some(r) = self.warm.running.take()
        {
            let current = r.activation == self.warm.activation && warm::same_level(r.level, level);
            if current && !out.report.cancelled {
                self.finish_warm(&r, out, pixels);
            }
        }

        // 2. One running for a level the camera has left: stop it.
        if let Some(r) = &self.warm.running {
            if (r.activation != self.warm.activation || !warm::same_level(r.level, level))
                && !self.warm.superseded
            {
                self.warm.superseded = true;
                self.tasks.cancel(r.task);
            }
            return;
        }

        // 3. Start one?
        if self.warm.declined {
            return;
        }
        let first = self.warm.needs && !self.editor.doc.is_dirty();
        let zoom = !first && self.warm.warmed_level.is_some_and(|w| !warm::same_level(w, level));
        let count = self.editor.playing_length();
        let ready = (first || zoom)
            && count > 1
            && self.lights.symbol_reuse()
            && self.loading.is_empty()
            && !self.merging
            && self.scripting.is_none()
            && self.test_movie.is_none()
            && !self.editor.is_gesturing()
            && stage::is_plain(&self.editor)
            && pixels.is_finite()
            && pixels > 0.0;
        if !ready {
            self.warm.settle = None;
            return;
        }
        // The camera holds still first: a zoom gesture or the first layout
        // must not warm a level nobody stays at.
        let now = Instant::now();
        match self.warm.settle {
            Some((l, since)) if warm::same_level(l, level) => {
                if now.duration_since(since) < warm::SETTLE {
                    return;
                }
            }
            _ => {
                self.warm.settle = Some((level, now));
                return;
            }
        }
        self.warm.settle = None;

        // The cache must be home: it is read and (for a zoom) emptied here.
        self.stop_encode_ahead();
        if zoom {
            // Every stamp is keyed on the old zoom: none can be hit any more.
            let old = self.lights.take_all_stamps();
            self.jobs.spawn(Pool::Background, move || drop(old));
        }
        let frames = warm::frames_from(if zoom { self.editor.current_frame } else { 0 }, count);
        let limit = warm::default_budget_bytes();
        let budget = warm::warm_share(limit).saturating_sub(self.lights.stamp_bytes());
        let mut inputs = stage::StageInputs::from_editor(&self.editor, area_px, scale);
        inputs.frame = frames[0];
        // The line budget a pass judges against is the buffer the stage will
        // have once sized, not the one it opens with.
        inputs.buffer_scale = self.lights.max_scale().max(self.lights.scale());
        let job = warm::WarmJob {
            inputs,
            frames,
            budget_bytes: budget,
            limit_bytes: limit,
            workers: self.jobs.interactive_threads(),
            detail: self.lights.detail(),
            symbols: self.lights.symbols.clone(),
        };
        let jobs = Arc::clone(&self.jobs);
        let (tx, rx) = crossbeam_channel::bounded(1);
        let label = format!("Preparing {}", self.editor.doc.display_name());
        self.warm.superseded = false;
        let task = self
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Warm, label, move |ctx| {
                ctx.progress.detail(if zoom {
                    "Rebuilding shapes for this zoom"
                } else {
                    "Building shapes for smooth playback"
                });
                let out = warm::run_warm(
                    &jobs,
                    job,
                    &|| ctx.cancelled(),
                    &|done, total| ctx.progress.set(done, total),
                );
                let cancelled = out.report.cancelled;
                let _ = tx.send(out);
                if cancelled {
                    crate::tasks::TaskOutcome::Cancelled
                } else {
                    crate::tasks::TaskOutcome::Finished(String::new())
                }
            });
        self.warm.running = Some(warm::Running {
            task,
            rx,
            activation: self.warm.activation,
            level,
            zoom,
        });
    }

    /// Install a finished warm-up into the stage cache, size the buffers once
    /// (before the first frame is shown), and say what happened.
    fn finish_warm(&mut self, r: &crate::warm_open::Running, out: crate::warm_open::WarmOutput, pixels: f64) {
        // The cache must be home to be installed into.
        self.stop_encode_ahead();
        let report = crate::warm_open::apply(&mut self.lights, out, pixels);
        if let Some(active) = self.active.as_mut() {
            let before = active.gpu.buffer_scale;
            active.gpu.grow_to(self.lights.scale());
            // A new scale means new buffers: allocate them on the next frame.
            self.warm.prealloc = active.gpu.buffer_scale > before;
        }
        self.warm.warmed_level = Some(r.level);
        self.warm.needs = false;
        if r.zoom {
            self.warm.zoom_warms += 1;
            self.warm.last_zoom_warm_s = Some(report.wall_s);
        }
        let mib = report.stamp_bytes as f64 / (1024.0 * 1024.0);
        tracing::info!(
            zoom = r.zoom,
            frames = report.frames_total,
            covered = report.frames_covered,
            stamps = report.stamp_keys,
            mib = format!("{mib:.0}"),
            budget_mib = report.budget_bytes >> 20,
            by_budget = report.stopped_by_budget,
            fit_scale = report.fit_scale,
            peak_lines = report.peak_lines,
            wall_s = format!("{:.1}", report.wall_s),
            "warm-up finished"
        );
        self.editor.status = Some(if report.frames_covered >= report.frames_total {
            format!(
                "Prepared all {} frames ({} shapes cached, {mib:.0} MiB) in {:.1} s",
                report.frames_total, report.stamp_keys, report.wall_s
            )
        } else {
            format!(
                "Prepared the first {} of {} frames ({mib:.0} MiB of cached shapes, the memory limit); the rest is built as it plays",
                report.frames_covered, report.frames_total
            )
        });
        self.warm.last = Some(report);
    }

    /// The light-cache detail level, as the stage frame last judged it (see
    /// [`light_detail_of`]).
    fn light_detail(&self) -> buzz_render::document::LightDetail {
        light_detail_of(&self.pipeline, &self.lights)
    }

    fn render(&mut self) -> Result<()> {
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };

        let now = Instant::now();
        let elapsed = now.duration_since(active.last_frame);
        active.frame_ms = elapsed.as_secs_f32() * 1000.0;
        active.last_frame = now;

        // E0 (`docs/BIMARI5_LAG_PLAN.md` S5 revised): where a frame's time
        // goes on the render thread. Plain `Instant` reads; logged only for a
        // slow frame or when `BUZZ_FRAME_LOG=1`.
        let render_started = now;
        let encode_ms: f64;
        let mut render_call_ms = 0.0f64;
        let acquire_ms: f64;

        self.profiler.begin_frame();
        self.profiler.enter(crate::profile::Section::Ui);

        // Playback runs on wall-clock time, so the document plays at its
        // authored rate regardless of the display's refresh rate.
        //
        // Test Movie owns the transport while it is open: it follows the
        // audio clock the same way `advance_playback` does, but for the
        // whole film rather than one scene, so the editor's own playback is
        // skipped underneath it.
        if let Some(movie) = self.test_movie.as_mut() {
            movie.advance(elapsed.as_secs_f64(), self.editor.sound.playing_frame());
        } else {
            self.editor.advance_playback(elapsed.as_secs_f64());
        }
        // E4: tell the encode thread where the playhead is now.
        if self.pipeline.is_running() {
            self.pipeline.playhead(self.editor.current_frame);
        }
        // S9: collect a finished warm-up, or start one once the camera has
        // settled (reads the stage rectangle the last frame measured).
        if let Some((area_px, scale)) = self.warm_stage {
            self.tick_warm(area_px, scale);
        }
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
            // Test Movie, over everything drawn so far: a full-window scrim
            // and the film, so the stage beneath cannot be edited while it
            // plays.
            self.test_movie_overlay(ui.ctx());
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

        // The OS title tracks the document, including its dirty asterisk.
        // Set only when it changes: `set_title` on every frame is needless
        // window-manager traffic.
        let wanted_title = format!("{} - BuzzAnimate", self.editor.doc.display_name());
        if wanted_title != self.last_window_title {
            active.window.set_title(&wanted_title);
            self.last_window_title = wanted_title;
        }

        // **Window actions the title bar row asked for**, drained after the
        // frame that queued them rather than acted on while drawing it —
        // which is what keeps `WindowRequest` testable without a real window
        // underneath it.
        //
        // `Close` is only noted here, not acted on: `request_quit` needs the
        // whole of `self`, which `active` is still borrowed from for the
        // other requests in the same queue.
        let mut close_requested = false;
        {
            use crate::title_bar::WindowRequest;
            for request in self.window_requests.drain(..) {
                match request {
                    WindowRequest::StartDrag => {
                        let _ = active.window.drag_window();
                    }
                    WindowRequest::ToggleMaximize => {
                        active.window.set_maximized(!active.window.is_maximized());
                    }
                    WindowRequest::Minimize => active.window.set_minimized(true),
                    WindowRequest::Close => close_requested = true,
                    WindowRequest::Resize(direction) => {
                        let _ = active.window.drag_resize_window(direction);
                    }
                }
            }
        }
        // The same path the OS's own close button takes, so unsaved work is
        // still asked about rather than discarded.
        if close_requested {
            self.request_quit();
        }
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        self.window_maximized = active.window.is_maximized();

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
        // S9: what the warm-up at the top of the next frame warms for.
        self.warm_stage = (area_px.width() > 0.0 && area_px.height() > 0.0
            && area_px.width().is_finite()
            && area_px.height().is_finite())
        .then_some((area_px, scale));

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
        let cold = light_cache_empty_of(&self.pipeline, &self.lights)
            && self.editor.scene().lights().is_active()
            && light_detail_of(&self.pipeline, &self.lights)
                == buzz_render::document::LightDetail::Full;
        let building = self.shade_build.is_some();
        let bulk = cold || self.editor.is_gesturing();
        // Has the light come to rest? Nothing is recorded or built while it is
        // still moving — see where the batch is started, below.
        let settled = aim == self.shade_aim;
        self.shade_aim = aim;
        // Not while the cache is lent: the pipeline only runs when these are
        // already `INLINE_BUDGET` and `true` (see `can_encode_ahead`), and they
        // were set so before it was lent.
        if !self.pipeline.is_running() {
            self.lights.lights.set_inline_budget(if bulk || building {
                Duration::ZERO
            } else {
                buzz_render::lighting::INLINE_BUDGET
            });
            self.lights.lights.set_queue(!building && settled);
        }

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
            edit_path: self
                .editor
                .scene()
                .edit_path()
                .iter()
                .map(|s| s.0)
                .collect(),
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
        // **Never reused while the stage is drawn in bands.** `active.vello`
        // is not even what reaches the screen in that mode — see where
        // `stage_bands` is read below — so a stamp match would only mean the
        // wasted single-pass encode need not be built again, not that the
        // bands need not be. See Phase 4d.
        let reuse = self.retain_stage
            && !cold
            && !stale_encoding
            && !paints_into_scene
            && self.stage_bands == 0
            && self.stage_stamp.as_ref() == Some(&stamp);
        // Already banding, coming into this frame: the band encode below —
        // one `cache.begin`/`end` around all of them — is what actually draws
        // this frame, and `finish_stage_frame` runs once after it instead.
        // Skipping the whole-frame ladder here is `RENDER_FOLLOWUPS_PLAN.md`
        // item 2: run every frame, it used to encode the whole frame into
        // `active.vello` a second time, on top of the bands, for a scene
        // nothing downstream ever read.
        //
        // S3 (`docs/BIMARI5_LAG_PLAN.md`): not while the buffer scale can
        // still grow, and not on the frames `band_recheck_due` names — those
        // run the ladder, which grows the scale first and leaves band mode
        // (below) as soon as the whole frame fits.
        let banding_already = self.stage_bands > 0
            && !band_recheck_due(
                self.lights.scale(),
                self.lights.max_scale(),
                self.band_frames_since_ladder,
                self.editor.current_frame,
                self.band_last_frame,
            );
        // **An unchanged frame in band mode is not drawn again.** The bands
        // were composed into `active.target`, which nothing else writes, so
        // when the stamp, the target size and the pasteboard colour are the
        // ones the bands were drawn for, this frame is the picture already
        // there. Without this every redraw request — from egui, a tooltip, a
        // finished thumbnail — cost a full 16-band encode and render
        // (600–1300 ms on Bimari5) for a picture that could not have changed.
        // The ordinary single pass has the same rule in `reuse` above.
        let banded_background = pasteboard(&self.editor.workspace);
        let reuse_banded = banded_frame_reusable(
            banding_already && self.retain_stage,
            cold || stale_encoding || paints_into_scene,
            self.stage_stamp.as_ref() == Some(&stamp),
            self.banded_on_target,
            (
                active.surface_config.width,
                active.surface_config.height,
                banded_background,
            ),
        );
        // `finish_stage_frame` consumes `stamp` exactly once, either just
        // below (an ordinary frame) or once after the bands (a frame already
        // banding coming in) — never both, so `.take()` at each candidate
        // site is what lets the compiler see that without the two arms
        // having to share one `if`/`else`.
        let mut pending_stamp = Some(stamp);
        let encode_started = Instant::now();
        // **E4: while a plain stage plays, the encode thread has the frame.**
        let frame_interval = Duration::from_secs_f64(
            1.0 / f64::from(self.editor.scene().stage().frame_rate.max(1.0)),
        );
        let mut pipelined = false;
        let source = (Ahead {
            pipeline: &mut self.pipeline,
            lights: &mut self.lights,
            editor: &self.editor,
            stage_bands: self.stage_bands,
            test_movie_open: self.test_movie.is_some(),
            stage_stale: self.stage_stale,
            shade_building: self.shade_build.is_some(),
        })
        .source(
            pending_stamp.as_ref().expect("set just above"),
            scale,
            settled,
            frame_interval,
        );
        match source {
            PipeSource::Normal => {}
            // Nothing new to show: the scene already in `active.vello` is it
            // (`Shown`), or the frame did not come in time (`NotReady`, counted).
            PipeSource::Shown | PipeSource::NotReady => pipelined = true,
            PipeSource::Ready(mut prepared) => {
                pipelined = true;
                std::mem::swap(&mut active.vello, &mut prepared.scene);
                self.pipeline.recycle(prepared.scene);
                // What `finish_stage_frame` records, minus its lighting part
                // (misses are the home cache's business, and the pipeline only
                // runs while there are none): the stamp, and whether the shading
                // was provisional.
                self.stage_stamp = pending_stamp.take();
                self.stage_stale = prepared.light.stale;
                self.band_frames_since_ladder = 0;
                self.band_last_frame = self.editor.current_frame;
            }
        }
        if !pipelined && wants_whole_frame_ladder(reuse, banding_already) {
            // **E7: a jump (or a scrub) gets its stamps built on workers first**,
            // so the encode below is warm. Only a plain, not-playing stage on
            // the home cache with no bands; `BUZZ_NO_PREBUILD=1` says never.
            let seek_inputs = (!self.editor.playback.playing
                && self.stage_bands == 0
                && !self.pipeline.is_running()
                && stage::is_plain(&self.editor))
            .then(|| stage::StageInputs::from_editor(&self.editor, area_px, scale));
            let mut seek_misses = 0;
            if let Some(inputs) = &seek_inputs {
                let frame = self.editor.current_frame;
                let jump = frame.abs_diff(self.band_last_frame) > 1;
                let epoch = Ahead::epoch(
                    pending_stamp.as_ref().expect("set just above"),
                    scale,
                    inputs.buffer_scale,
                    self.lights.detail(),
                );
                self.seek.before_encode(epoch, &mut self.lights, inputs, jump);
                seek_misses =
                    self.lights.symbol_scenes.builds + self.lights.symbol_scenes.first_sightings;
            }
            stage::build_scene(
                &mut active.vello,
                &self.editor,
                area_px,
                scale,
                &mut self.lights,
            );
            if let Some(inputs) = &seek_inputs {
                let missed = self.lights.symbol_scenes.builds
                    + self.lights.symbol_scenes.first_sightings
                    > seek_misses;
                self.seek.after_encode(
                    &mut self.lights,
                    inputs,
                    self.editor.playing_length(),
                    missed,
                );
            }
            finish_stage_frame(
                &mut self.lights,
                &mut self.shade_build,
                &self.jobs,
                &active.window,
                aim,
                &mut self.stage_stamp,
                &mut self.stage_stale,
                pending_stamp.take().expect("set just above"),
            );

            // **A frame the ordinary ladder cannot fit even at
            // `LightDetail::Flat`, the bottom of the automatic ladder,** falls
            // back to drawing it in strips, rather than handing Vello a scene
            // it will silently drop. See `render_stage_bands` and Phase 4d in
            // `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
            //
            // S3: this branch also runs for a frame that *was* banding
            // (scale could still grow, or a re-check came due), so a frame
            // that now fits leaves band mode here — the whole-frame verdict
            // is the only exit.
            self.band_frames_since_ladder = 0;
            self.band_last_frame = self.editor.current_frame;
            if self.lights.over_budget() {
                self.stage_bands = self.lights.bands_needed();
            } else if self.stage_bands > 0 {
                self.stage_bands = 0;
            } else if !active.dragging && !self.editor.playback.playing {
                // V4 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): only a frame that
                // settled comfortably, outside a gesture or playback, ever
                // offers to give buffer scale back — never mid-drag, where
                // the renderer rebuild below would cost a hitch.
                self.lights
                    .note_shrink_headroom(area_px.width() * area_px.height(), Instant::now());
            }
        }
        // A plain stage that plays from here on is drawn ahead from the next
        // frame (the one just drawn above is the cache's warm starting point).
        if !pipelined && !self.pipeline.is_running() {
            (Ahead {
                pipeline: &mut self.pipeline,
                lights: &mut self.lights,
                editor: &self.editor,
                stage_bands: self.stage_bands,
                test_movie_open: self.test_movie.is_some(),
                stage_stale: self.stage_stale,
                shade_building: self.shade_build.is_some(),
            })
            .start(self.stage_stamp.as_ref(), area_px, scale, settled);
        }
        encode_ms = encode_started.elapsed().as_secs_f64() * 1e3;

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
        // Band mode: `finish_stage_frame` for this frame does not run until
        // after the bands are drawn, below — reading `stage_stale` here
        // first would still be reading last frame's. See the same two
        // checks just after `finish_stage_frame` in the banded path below.
        if !banding_already && self.stage_stale {
            WAKE_STALE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        if !banding_already && rig != self.last_rig {
            WAKE_RIG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        let acquire_started = Instant::now();
        let acquired = active.surface.get_current_texture();
        acquire_ms = acquire_started.elapsed().as_secs_f64() * 1e3;
        let frame = match acquired {
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

        // **The Export dialog's test render**, here for the same reason: this
        // is the only place the device, Vello and egui's renderer meet. The
        // whole strip in one go — six small pictures is a few milliseconds,
        // and a strip filling in one frame at a time would flicker its way
        // into existence while the user watched.
        if self.preview.pending() {
            let mut scratch = buzz_render::vello::Scene::new();
            self.preview.fulfil(
                &mut active.gpu,
                &mut active.egui_renderer,
                &mut scratch,
                &mut self.lights,
            );
            active.window.request_redraw();
        }

        // **Test Movie**, on the same budget and for the same reason: this is
        // the only place the device, Vello and egui's renderer meet.
        if let Some(movie) = self.test_movie.as_mut()
            && movie.needs_render()
        {
            let mut scratch = buzz_render::vello::Scene::new();
            movie.fulfil(
                &mut active.gpu,
                &mut active.egui_renderer,
                &mut scratch,
                &mut self.lights,
            );
            active.window.request_redraw();
        }
        for id in self.test_movie_retired.drain(..) {
            active.egui_renderer.free_texture(&id);
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
        // S7 (`docs/BIMARI5_LAG_PLAN.md`): while the film plays, the GPU is kept
        // out of its idle power states, so the first heavy frame after a run of
        // light ones is not drawn at idle clocks (a 10 ms frame took 47-122 ms).
        if self.editor.playback.playing {
            active.gpu.keep_warm();
        }
        // V3/V4 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): sync this window's
        // live Vello buffer scale to whatever `self.lights` just judged the
        // frame needs. Growing is always safe to do eagerly — it never
        // degrades quality, only allocates. Shrinking only ever happens here
        // because `self.lights.pending_shrink()` is only ever `Some` when
        // the ladder block above actually offered it, which it never does
        // mid-gesture or mid-playback.
        if let Some(half) = self.lights.pending_shrink() {
            if active.gpu.shrink_to(half).is_ok() {
                self.lights.commit_shrink(half);
            }
        }
        // The lent cache's scale comes from the frame just taken
        // (`Prepared::scale`).
        active.gpu.grow_to(light_scale_of(&self.pipeline, &self.lights));
        if reuse_banded {
            // Nothing changed since the bands were composed into the target:
            // the picture is already there. See `reuse_banded`.
        } else if self.stage_bands > 0 {
            if banding_already {
                self.band_frames_since_ladder = self.band_frames_since_ladder.saturating_add(1);
                self.band_last_frame = self.editor.current_frame;
            }
            // Half-written until it returns, so not reusable if it fails.
            self.banded_on_target = None;
            let render_started_bands = Instant::now();
            self.stage_bands = render_stage_bands(
                active,
                &self.editor,
                &mut self.lights,
                area_px,
                scale,
                banded_background,
                self.stage_bands,
            )?;
            // Band mode: planning (encode) and every band's render call are
            // inside this one span; `bands` in the log says so.
            render_call_ms = render_started_bands.elapsed().as_secs_f64() * 1e3;
            self.banded_on_target = Some((w, h, banded_background));
            // This frame's geometry — what `finish_stage_frame` needs — was
            // only just drawn, band by band, above; a frame that instead ran
            // the whole-frame ladder already ran it there, right after.
            if let Some(stamp) = pending_stamp.take() {
                finish_stage_frame(
                    &mut self.lights,
                    &mut self.shade_build,
                    &self.jobs,
                    &active.window,
                    aim,
                    &mut self.stage_stamp,
                    &mut self.stage_stale,
                    stamp,
                );
                // The two redraw requests skipped above for band mode,
                // read now that `finish_stage_frame` has actually set
                // `stage_stale` for this frame. See the comments on the
                // same checks above the ordinary path.
                if self.stage_stale {
                    active.window.request_redraw();
                }
                if rig != self.last_rig {
                    self.last_rig = rig;
                    active.window.request_redraw();
                }
            }
        } else {
            // The single pass overwrites whatever bands the target held.
            self.banded_on_target = None;
            let target = &active.target.as_ref().expect("ensure_target ran").view;
            // S9: the rasteriser's buffers are only really allocated by the
            // first render at a new scale (~200 ms at scale 8). A warm-up that
            // sized them pays that here, with an empty scene, so no frame of a
            // playback does.
            if std::mem::take(&mut self.warm.prealloc) {
                active.gpu.render(
                    &VelloScene::new(),
                    target,
                    w,
                    h,
                    pasteboard(&self.editor.workspace),
                )?;
            }
            let render_call_started = Instant::now();
            active
                .gpu
                .render(&active.vello, target, w, h, pasteboard(&self.editor.workspace))?;
            render_call_ms = render_call_started.elapsed().as_secs_f64() * 1e3;
        }
        // Read back out once either path above has finished writing into it —
        // the ordinary single pass rendered straight into it, banded mode
        // copied each strip into it in turn.
        let target = &active.target.as_ref().expect("ensure_target ran").view;

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

        let present_started = Instant::now();
        active
            .gpu
            .queue
            .submit(user_buffers.into_iter().chain([encoder.finish()]));
        frame.present();
        let present_ms = present_started.elapsed().as_secs_f64() * 1e3;

        for id in &output.textures_delta.free {
            active.egui_renderer.free_texture(id);
        }

        self.keep_crash_snapshot();
        self.poll_autosave();

        // Close the frame: store its section times and warn if one blew the
        // budget. This is the watchdog that catches an O(document) cost creeping
        // back onto the frame before it becomes a hang.
        self.profiler.end_frame();

        // S0 (`docs/BIMARI5_LAG_PLAN.md` §4): evidence for a frame heavy
        // enough to look like a hang. A frame this slow is rare enough that
        // one `tracing::warn!` per occurrence — not rate-limited the way
        // `FrameProfiler::end_frame`'s own per-section warning is — is still
        // cheap, and it is exactly the frame a hang report needs explained.
        // The release build has no console to have seen it in; `main.rs`
        // wires this subscriber to a log file for that build so a hang
        // still leaves evidence.
        let closed = self.profiler.last();
        if frame_log_enabled() {
            static FRAME_NO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = FRAME_NO.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::info!(
                "frame_log n {} frame {} encode_ms {:.2} render_call_ms {:.2} acquire_ms {:.2}                  present_ms {:.2} total_ms {:.2} scale {} bands {} lines {} why {}",
                n,
                self.editor.current_frame,
                encode_ms,
                render_call_ms,
                acquire_ms,
                present_ms,
                render_started.elapsed().as_secs_f64() * 1e3,
                light_scale_of(&self.pipeline, &self.lights),
                self.stage_bands,
                light_last_lines_of(&self.pipeline, &self.lights),
                self.redraw_reasons(),
            );
        }
        if closed.total() > Duration::from_millis(250) {
            let sections: Vec<String> = crate::profile::Section::ALL
                .into_iter()
                .map(|s| {
                    format!(
                        "{} {:.1}",
                        s.label(),
                        closed.section(s).as_secs_f64() * 1000.0
                    )
                })
                .collect();
            tracing::warn!(
                "slow frame: {:.1} ms ({}) — encode {:.1} render-call {:.1} acquire {:.1} present {:.1} ms — stage_bands {} lights scale {} frame {}",
                closed.total().as_secs_f64() * 1000.0,
                sections.join("  "),
                encode_ms,
                render_call_ms,
                acquire_ms,
                present_ms,
                self.stage_bands,
                light_scale_of(&self.pipeline, &self.lights),
                self.editor.current_frame,
            );
        }
        Ok(())
    }
}

impl ApplicationHandler<UserEvent> for App {
    /// The loop was woken by something off the UI thread — egui's repaint
    /// callback, or a background install. Draw a frame.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        WAKE_USER_EVENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        use crate::title_bar::TitleBar;

        let Some(active) = self.active.as_mut() else {
            return;
        };

        // **A press on a Custom title bar's resize border starts a native
        // resize instead of reaching egui.** Checked, and returned from,
        // before `on_window_event` sees the event at all: forwarding the
        // press first would let one of egui's own widgets claim it — the
        // stage's own pointer handling, mid-drag — and a resize that only
        // sometimes starts is worse than a border a pixel too broad.
        if self.title_bar == TitleBar::Custom
            && !active.window.is_maximized()
            && let WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } = &event
            && let Some(pos) = self.last_cursor
        {
            let size = active.window.inner_size();
            let border = 6.0 * active.window.scale_factor() as f32;
            let window_size = egui::vec2(size.width as f32, size.height as f32);
            if let Some(direction) = crate::title_bar::resize_direction(pos, window_size, border) {
                let _ = active.window.drag_resize_window(direction);
                return;
            }
        }

        let response = active.egui_state.on_window_event(&active.window, &event);
        // **Not for `RedrawRequested`:** egui-winit answers every one of those
        // with `repaint: true`, so asking for a redraw here made each frame
        // order the next one and an idle document re-rendered forever (Bimari5:
        // ~35 frames a second of 30 ms GPU work with nothing changing, found
        // 2026-10-01 with `BUZZ_FRAME_LOG`; `accesskit_winit` is the one
        // dependency added since this was harmless). This event *is* the
        // redraw; egui's own later repaints arrive through its wake callback
        // (`user_event`) and `egui_repaint`.
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            WAKE_EGUI_EVENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            // Tracked so a `MouseInput` press elsewhere in this same handler
            // can test the resize border without waiting on egui's own input
            // state, which lags a frame behind the raw OS event. Also sets
            // the resize cursor while hovering the border itself, Custom
            // mode only — egui's own per-frame cursor already covers every
            // widget's hover, and this only adds the border's own eight.
            WindowEvent::CursorMoved { position, .. } => {
                let pos = egui::pos2(position.x as f32, position.y as f32);
                self.last_cursor = Some(pos);
                if self.title_bar == TitleBar::Custom && !active.window.is_maximized() {
                    let size = active.window.inner_size();
                    let border = 6.0 * active.window.scale_factor() as f32;
                    let window_size = egui::vec2(size.width as f32, size.height as f32);
                    if let Some(direction) =
                        crate::title_bar::resize_direction(pos, window_size, border)
                    {
                        active.window.set_cursor(winit::window::CursorIcon::from(direction));
                    }
                }
            }
            WindowEvent::Resized(size) => {
                active.resize(size.width, size.height);
                self.window_maximized = active.window.is_maximized();
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
                    // Stop listening and take our entry out of the port ledger.
                    self.api_shutdown();
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

        // `BUZZ_AUTOPLAY=<seconds>` (diagnostics): start playback once, that
        // long after launch, so a real-app playback can be logged with
        // `BUZZ_FRAME_LOG` without anyone pressing a key.
        if let Some(after) = autoplay_after()
            && !AUTOPLAYED.load(std::sync::atomic::Ordering::Relaxed)
            && self.active.as_ref().is_some_and(|a| a.shown)
        {
            let started = *AUTOPLAY_FROM.get_or_init(Instant::now);
            if started.elapsed() >= after {
                AUTOPLAYED.store(true, std::sync::atomic::Ordering::Relaxed);
                if !self.editor.playback.playing {
                    self.editor.toggle_playback();
                }
                tracing::info!("autoplay: playback started");
                if let Some(active) = &self.active {
                    active.window.request_redraw();
                }
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(started + after));
                return;
            }
        }

        // **Minimised, with work for another app.** Windows sends a minimised
        // window no paint messages, so `request_redraw` never becomes a frame
        // — and the local API's requests, its export jobs and episode builds
        // are all served from the frame (`api_pump`, `pump_export_queue`).
        // The Studio or BuzzEdit driving a minimised BuzzAnimate would wait out
        // its timeout. So while there is such work, run the frame directly, a
        // few times a second; the surface declines to present and nothing is
        // drawn, but the UI pass — where the work happens — still runs.
        let minimised = self
            .active
            .as_ref()
            .is_some_and(|a| a.window.is_minimized() == Some(true));
        if minimised && (self.api.has_work() || !self.exports.is_idle()) {
            if let Err(e) = self.render() {
                eprintln!("frame failed: {e:?}");
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + std::time::Duration::from_millis(30),
            ));
            return;
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
    /// Dock a panel in a column above the section named, or at the bottom.
    docks: Vec<(
        buzz_ui::PanelId,
        buzz_ui::Dock,
        Option<buzz_ui::workspace::GroupId>,
    )>,
    /// Float a panel at a point on the screen.
    floats: Vec<(buzz_ui::PanelId, (f32, f32))>,
    /// Move a whole section to a side — every tab it carries, not just the
    /// one named.
    section_moves: Vec<(buzz_ui::PanelId, buzz_ui::Dock)>,
    /// Float a whole section at a point on the screen, as one window.
    section_floats: Vec<(buzz_ui::PanelId, (f32, f32))>,
    /// Dock a whole section in a column above the section named, or at the
    /// bottom.
    section_docks: Vec<(
        buzz_ui::PanelId,
        buzz_ui::Dock,
        Option<buzz_ui::workspace::GroupId>,
    )>,
    /// Tab a whole section in with another's, every member chained in.
    section_groups: Vec<(buzz_ui::PanelId, buzz_ui::PanelId)>,
}

/// What is being carried during a drag: the panels of a section picked up by
/// its grip, or the one panel of a tab.
struct DraggedPanels(Vec<buzz_ui::PanelId>);

/// A section as it was drawn this frame, for a dragged panel to land on.
#[derive(Debug, Clone, Copy)]
struct DropZone {
    dock: buzz_ui::Dock,
    group: buzz_ui::workspace::GroupId,
    front: buzz_ui::PanelId,
    rect: egui::Rect,
}

/// Where a dragged panel would land if released now.
enum DropTarget {
    /// As a tab of this section.
    Section {
        front: buzz_ui::PanelId,
        rect: egui::Rect,
    },
    /// In a section of its own in this column, above the section named.
    Column {
        dock: buzz_ui::Dock,
        before: Option<buzz_ui::workspace::GroupId>,
        line: egui::Rect,
        /// The section whose border highlights when approaching/docking near it
        highlight_rect: Option<egui::Rect>,
    },
    /// In a window of its own, here.
    Float(egui::Pos2),
}

/// Calculate the Euclidean distance from a point to a rectangle.
/// Returns 0.0 if the point is inside the rectangle.
fn distance_to_rect(pos: egui::Pos2, rect: egui::Rect) -> f32 {
    let dx = (rect.left() - pos.x).max(0.0).max(pos.x - rect.right());
    let dy = (rect.top() - pos.y).max(0.0).max(pos.y - rect.bottom());
    (dx * dx + dy * dy).sqrt()
}

/// How long a revealed panel's header stays lit.
const HIGHLIGHT_SECONDS: f64 = 2.5;

/// The colour of the drop indicator while a panel is being dragged.
///
/// A clear green, as Animate uses, rather than the blue selection accent: it has
/// to read as "here" on top of any theme's panels, and against a selection that
/// may itself be on screen. Fixed rather than a palette entry so it is the same
/// unmistakable green in every theme.
const DROP_ACCENT: egui::Color32 = egui::Color32::from_rgb(0x35, 0xC4, 0x6A);

impl App {
    /// How brightly this section's header is lit this frame, if at all.
    fn emphasis_for(&self, section: &buzz_ui::Section) -> Option<f32> {
        self.highlight_frame
            .filter(|(panel, _)| section.panels.contains(panel))
            .map(|(_, strength)| strength)
    }

    /// Where the pointer is over, in drop terms.
    ///
    /// Sections are checked with proximity detection: when the pointer is inside
    /// or comes close to a section (within 28 points), that section highlights its
    /// border as the docking candidate.
    fn drop_target(&self, pointer: egui::Pos2, dragged: &[buzz_ui::PanelId]) -> Option<DropTarget> {
        const PROXIMITY_MARGIN: f32 = 28.0;

        // Candidate drop zones that do not already contain the panel being carried.
        let candidates: Vec<&DropZone> = self
            .drop_zones
            .iter()
            .rev()
            .filter(|z| !dragged.contains(&z.front))
            .collect();

        // 1. Direct hit or closest zone within proximity.
        let target_zone = candidates
            .iter()
            .copied()
            .find(|z| z.rect.contains(pointer))
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .map(|z| (z, distance_to_rect(pointer, z.rect)))
                    .filter(|(_, d)| *d <= PROXIMITY_MARGIN)
                    .min_by(|(_, d1), (_, d2)| d1.total_cmp(d2))
                    .map(|(z, _)| z)
            });

        if let Some(zone) = target_zone {
            let r = zone.rect;
            if let Some(col) = self
                .dock_rects
                .iter()
                .find(|(d, _)| *d == zone.dock)
                .map(|(_, r)| *r)
            {
                // A generous edge band, so the line is easy to hit, but never so
                // wide it swallows the body of a short section.
                let band = (r.height() * 0.30).clamp(8.0, 32.0);
                if pointer.y < r.top() + band {
                    return Some(DropTarget::Column {
                        dock: zone.dock,
                        before: Some(zone.group),
                        line: Self::insertion_line(col, r.top()),
                        highlight_rect: Some(r),
                    });
                }
                if pointer.y > r.bottom() - band {
                    return Some(DropTarget::Column {
                        dock: zone.dock,
                        before: self.section_after(zone.dock, r),
                        line: Self::insertion_line(col, r.bottom()),
                        highlight_rect: Some(r),
                    });
                }
            }
            return Some(DropTarget::Section {
                front: zone.front,
                rect: r,
            });
        }

        // 2. Over or close to a dock column (in the gap past or between sections).
        if let Some((dock, rect)) = self
            .dock_rects
            .iter()
            .find(|(_, r)| r.contains(pointer) || distance_to_rect(pointer, *r) <= PROXIMITY_MARGIN)
        {
            let mut zones: Vec<&DropZone> =
                self.drop_zones.iter().filter(|z| z.dock == *dock).collect();
            zones.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
            let before = zones.iter().find(|z| z.rect.center().y > pointer.y);
            let y = before
                .map(|z| z.rect.top())
                .or_else(|| zones.last().map(|z| z.rect.bottom()))
                .unwrap_or(rect.top());
            let nearest_rect = before
                .map(|z| z.rect)
                .or_else(|| zones.last().map(|z| z.rect));
            return Some(DropTarget::Column {
                dock: *dock,
                before: before.map(|z| z.group),
                line: Self::insertion_line(*rect, y),
                highlight_rect: nearest_rect,
            });
        }

        Some(DropTarget::Float(pointer))
    }

    /// A horizontal insertion bar spanning a column, centred on `y`.
    fn insertion_line(col: egui::Rect, y: f32) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(col.left() + 2.0, y - 2.0),
            egui::pos2(col.right() - 2.0, y + 2.0),
        )
    }

    /// The group of the section immediately below `rect` in `dock`, or `None`
    /// when `rect` is the last — i.e. what "dock below this section" docks
    /// before, in [`Workspace::dock_at`] terms.
    fn section_after(
        &self,
        dock: buzz_ui::Dock,
        rect: egui::Rect,
    ) -> Option<buzz_ui::workspace::GroupId> {
        self.drop_zones
            .iter()
            .filter(|z| z.dock == dock && z.rect.top() > rect.top() + 1.0)
            .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
            .map(|z| z.group)
    }

    /// Carry a dragged panel, show where it would land, and land it.
    fn drag_docking(&self, ctx: &egui::Context, requests: &mut DockRequests) {
        let Some(dragged) = egui::DragAndDrop::payload::<DraggedPanels>(ctx) else {
            return;
        };
        let accent = DROP_ACCENT;
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("dock-drop"),
        ));
        let pointer = ctx.pointer_latest_pos();

        // The name travels with the pointer, so what is being carried is
        // never in doubt.
        if let Some(p) = pointer {
            let names = dragged
                .0
                .iter()
                .map(|id| id.tab_title())
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            egui::Area::new(egui::Id::new("dock-drag-label"))
                .order(egui::Order::Tooltip)
                .fixed_pos(p + egui::vec2(14.0, 14.0))
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(egui::RichText::new(names).small());
                    });
                });
        }

        let target = pointer.and_then(|p| self.drop_target(p, &dragged.0));
        match &target {
            Some(DropTarget::Section { rect, .. }) => {
                // Tabbing in: a filled green wash under a glowing clear green border,
                // so the whole section reads as the drop target.
                painter.rect_filled(*rect, 4.0, accent.gamma_multiply(0.22));
                painter.rect_stroke(
                    *rect,
                    4.0,
                    egui::Stroke::new(3.0, accent),
                    egui::StrokeKind::Inside,
                );
                painter.rect_stroke(
                    rect.expand(1.5),
                    5.0,
                    egui::Stroke::new(1.5, accent.gamma_multiply(0.40)),
                    egui::StrokeKind::Outside,
                );
            }
            Some(DropTarget::Column {
                line,
                highlight_rect,
                ..
            }) => {
                // Docking as a new row: highlight the adjacent section's border
                // AND draw the insertion line across the column.
                if let Some(rect) = highlight_rect {
                    painter.rect_filled(*rect, 4.0, accent.gamma_multiply(0.08));
                    painter.rect_stroke(
                        *rect,
                        4.0,
                        egui::Stroke::new(2.5, accent),
                        egui::StrokeKind::Inside,
                    );
                    painter.rect_stroke(
                        rect.expand(1.5),
                        5.0,
                        egui::Stroke::new(1.0, accent.gamma_multiply(0.35)),
                        egui::StrokeKind::Outside,
                    );
                }
                // Animate's insertion line: a rounded green bar across the column.
                painter.rect_filled(*line, 2.5, accent);
                painter.rect_stroke(
                    *line,
                    2.0,
                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
            }
            Some(DropTarget::Float(_)) | None => {}
        }
        ctx.request_repaint();

        if ctx.input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(ctx);
            // A grip carries a whole section — every panel in `dragged.0` — and
            // it must land as one: one section-level request for the first
            // panel, rather than one per-panel request that would split the
            // section back apart on the way down. A single tab dragged out
            // still goes through the old per-panel requests.
            let whole_section = dragged.0.len() > 1;
            match target {
                Some(DropTarget::Section { front, .. }) => {
                    if whole_section {
                        if dragged.0[0] != front {
                            requests.section_groups.push((dragged.0[0], front));
                        }
                    } else {
                        for id in &dragged.0 {
                            if *id != front {
                                requests.groups.push((*id, front));
                            }
                        }
                    }
                }
                Some(DropTarget::Column { dock, before, .. }) => {
                    if whole_section {
                        requests.section_docks.push((dragged.0[0], dock, before));
                    } else {
                        for id in &dragged.0 {
                            requests.docks.push((*id, dock, before));
                        }
                    }
                }
                Some(DropTarget::Float(pos)) => {
                    if whole_section {
                        requests
                            .section_floats
                            .push((dragged.0[0], (pos.x, pos.y)));
                    } else {
                        for id in &dragged.0 {
                            requests.floats.push((*id, (pos.x, pos.y)));
                        }
                    }
                }
                None => {}
            }
        }
    }
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
            || !self.collapses.is_empty()
            || !self.docks.is_empty()
            || !self.floats.is_empty()
            || !self.section_moves.is_empty()
            || !self.section_floats.is_empty()
            || !self.section_docks.is_empty()
            || !self.section_groups.is_empty();

        for id in self.selects {
            workspace.select_tab(id);
        }
        for (id, collapsed) in self.collapses {
            workspace.set_collapsed(id, collapsed);
        }
        // Section-level requests, before the per-panel ones below: a whole
        // section moved and then one of its tabs nudged should leave both in
        // effect, in that order.
        for (id, dock) in self.section_moves {
            workspace.move_section(id, dock);
        }
        for (id, pos) in self.section_floats {
            workspace.float_section_at(id, pos);
        }
        for (id, dock, before) in self.section_docks {
            workspace.dock_section_at(id, dock, before);
        }
        for (id, target) in self.section_groups {
            workspace.group_section_with(id, target);
        }
        for (id, target) in self.groups {
            workspace.group_with(id, target);
        }
        for id in self.ungroups {
            workspace.ungroup(id);
        }
        for (id, dock, before) in self.docks {
            workspace.dock_at(id, dock, before);
        }
        for (id, pos) in self.floats {
            workspace.float_at(id, pos);
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

/// A centred X — the close glyph every window manager draws the same way.
fn paint_x_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    const HALF: f32 = 5.0;
    let c = rect.center();
    let stroke = egui::Stroke::new(1.0, color);
    painter.line_segment(
        [c + egui::vec2(-HALF, -HALF), c + egui::vec2(HALF, HALF)],
        stroke,
    );
    painter.line_segment(
        [c + egui::vec2(-HALF, HALF), c + egui::vec2(HALF, -HALF)],
        stroke,
    );
}

/// A single horizontal line — minimise.
fn paint_minimize_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    const HALF: f32 = 5.0;
    let c = rect.center();
    painter.line_segment(
        [c + egui::vec2(-HALF, 0.0), c + egui::vec2(HALF, 0.0)],
        egui::Stroke::new(1.0, color),
    );
}

/// An open square outline — maximise.
fn paint_maximize_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    const HALF: f32 = 5.0;
    let square = egui::Rect::from_center_size(rect.center(), egui::vec2(HALF * 2.0, HALF * 2.0));
    painter.rect_stroke(
        square,
        0.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Outside,
    );
}

/// Two offset square outlines — restore, once the window is maximised.
///
/// egui has no boolean path ops to actually cut the corner where the two
/// squares cross, so the front one is filled with the row's own background
/// first, which reads the same at this size: an unbroken square in front of
/// a corner of another one behind it.
fn paint_restore_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    const HALF: f32 = 4.5;
    let stroke = egui::Stroke::new(1.0, color);
    let back = egui::Rect::from_center_size(
        rect.center() + egui::vec2(2.0, -2.0),
        egui::vec2(HALF * 2.0, HALF * 2.0),
    );
    let front = egui::Rect::from_center_size(
        rect.center() + egui::vec2(-1.5, 1.5),
        egui::vec2(HALF * 2.0, HALF * 2.0),
    );
    painter.rect_stroke(back, 0.0, stroke, egui::StrokeKind::Outside);
    painter.rect_filled(front, 0.0, Palette::bg());
    painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Outside);
}

/// A dock's name for "Group With", without the "Dock " every other label
/// carries — the parenthetical reads as "Library (Far Right)", not "Library
/// (Dock Far Right)".
fn dock_paren_label(dock: buzz_ui::Dock) -> &'static str {
    match dock {
        buzz_ui::Dock::Left => "Left",
        buzz_ui::Dock::Right => "Right",
        buzz_ui::Dock::RightOuter => "Far Right",
        buzz_ui::Dock::Bottom => "Bottom",
        buzz_ui::Dock::Float => "Float",
        buzz_ui::Dock::Hidden => "Hidden",
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
    workspace: &buzz_ui::Workspace,
    // Every visible section's front panel, on every dock — the offer for
    // "Group With", which used to be only the panels sharing this section's
    // own dock and so could never reach, say, the Library from the inspector.
    all_fronts: &[buzz_ui::PanelId],
    locked: bool,
    named: bool,
    collapsible: bool,
    // How brightly the header is lit, for a panel that has just been revealed.
    emphasis: Option<f32>,
    out: &mut DockRequests,
) -> Option<egui::Rect> {
    // The room kept on the Timeline's header line for its layer tools — see
    // `buzz_ui::timeline_panel_with_tools_slot`. Reserved here, where the
    // header knows how much of its line the tabs left; drawn into by the
    // panel's body, which owns the buttons' state.
    let mut tools_slot = None;
    // **A tab strip, not a header bar.** Stage 2 of
    // `docs/WORKSPACE_DESIGN_PLAN.md`: flat, no fill of its own — the panel
    // background shows straight through — with the front tab's own 2 px
    // accent underline as the only thing marking where one panel stops and
    // the next begins. The old filled, rounded bar this replaced is exactly
    // the "raised header" chrome the plan calls out by name.
    //
    // **Lit, when the panel has just been asked for.** A 2 px accent outline
    // around the strip, fading as `emphasis` falls — no fill wash, per the
    // Decision ("Emphasis (reveal) is a 2 px accent outline ... no fill").
    let accent = Palette::accent();
    let mut frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(
        buzz_ui::theme::Tokens::SPACE_1 as i8,
        0,
    ));
    if let Some(strength) = emphasis {
        frame = frame.stroke(egui::Stroke::new(2.0, accent.gamma_multiply(strength)));
    }

    frame.show(ui, |ui| {
        // A plain `horizontal`, not a fixed-height allocation: a column
        // holding every panel tabbed together can still wrap onto several
        // lines, and clamping the row to `Tokens::TAB_H` here left it
        // reporting a rectangle its own content had already grown past —
        // exactly the overrun `dock_geometry_tests` exists to catch. An
        // ordinary strip with one line of tabs still comes out at `TAB_H`
        // in practice, since nothing in it is taller than that.
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            // Every plain button in this strip — the grip, the roll-up
            // chevron, the "..." menu, "Dock"/"Float" — is a ghost button:
            // no fill at rest, `Palette::hover()` on hover (already the
            // theme's own `widgets.hovered.bg_fill`, untouched here), the
            // one thing this scope changes. Stage 2, item 3: "Rest: no
            // fill; hover: `hover`."
            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.spacing_mut().button_padding = egui::vec2(3.0, 1.0);
            ui.spacing_mut().item_spacing.x = buzz_ui::theme::Tokens::SPACE_1;

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Three bars, drawn as text: this one *is* in egui's bundled font,
                // unlike most symbols — and it is checked by a test rather than
                // assumed, because that assumption has been wrong twice.
                //
                // **Not widened to 24 px.** Tried, both unconditionally and
                // behind an "is there room" check the way the grip below
                // guards its own width: even a Timeline-width dock (roomy by
                // construction) tipped a *different* narrow column over the
                // edge of the window once every other panel shared it, which
                // is what `dock_geometry_tests::the_stage_never_reaches_into_a_dock_column`
                // caught. This stays text-tight; exempted by name in
                // `hit_target_tests`, whose own comment carries the rest of
                // this reasoning.
                let (panel_menu, panel_menu_inner) = egui::containers::menu::MenuButton::from_button(
                    egui::Button::new(egui::RichText::new(PANEL_MENU).small()),
                )
                .ui(ui, |ui| {
                    if locked {
                        ui.label(egui::RichText::new("The layout is locked").small().weak());
                        ui.separator();
                    }

                    // **The whole section moves.** Choosing a dock here used to
                    // send only the front tab, tearing a tabbed section apart —
                    // "Move Tab Only" below is that old behaviour, kept for when
                    // it is what is actually wanted.
                    for dock in buzz_ui::Dock::CHOICES {
                        if ui
                            .add_enabled(
                                !locked || dock == buzz_ui::Dock::Hidden,
                                egui::Button::new(dock.label()),
                            )
                            .clicked()
                        {
                            out.section_moves.push((section.front, dock));
                            ui.close();
                        }
                    }
                    ui.add_enabled_ui(!locked, |ui| {
                        ui.menu_button("Move Tab Only", |ui| {
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
                        })
                        .response
                        .on_hover_text("Move just this tab, leaving the rest of the section where it is");
                    });

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
                    // and a menu can be read, found and tested. The offer spans
                    // every dock — not only this section's own side — labelled
                    // with where each one lives, so the inspector can reach the
                    // Library on the far right just as easily as a neighbour.
                    ui.separator();
                    let elsewhere: Vec<buzz_ui::PanelId> = all_fronts
                        .iter()
                        .copied()
                        .filter(|id| !section.panels.contains(id))
                        .collect();
                    ui.add_enabled_ui(!locked && !elsewhere.is_empty(), |ui| {
                        ui.menu_button("Group With", |ui| {
                            for target in &elsewhere {
                                let dock = workspace.dock_of(*target);
                                let label = format!("{} ({})", target.title(), dock_paren_label(dock));
                                if ui.button(label).clicked() {
                                    out.section_groups.push((section.front, *target));
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
                            "Nothing else is open to group with"
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
                });
                let _ = panel_menu_inner;
                panel_menu.on_hover_text("Move, group, float or close this panel");

                // Quick Float or Dock button when room allows (leaves narrow columns untouched).
                let can_quick_toggle = ui.available_width() >= 140.0;
                if can_quick_toggle {
                    if !collapsible {
                        // In a floating window: offer 1-click Dock back to
                        // sidebar — to wherever it was docked before it
                        // floated, not always Right.
                        if ui
                            .add_enabled(
                                !locked,
                                egui::Button::new(egui::RichText::new("Dock").small()),
                            )
                            .on_hover_text("Dock this panel back into the sidebar")
                            .clicked()
                        {
                            let home = workspace
                                .slot(section.front)
                                .map(|s| s.home)
                                .filter(|d| d.is_docked())
                                .unwrap_or(buzz_ui::Dock::Right);
                            out.section_moves.push((section.front, home));
                        }
                    } else if section.front != buzz_ui::PanelId::Tools {
                        // In a docked panel: offer 1-click Float into a window
                        if ui
                            .add_enabled(
                                !locked,
                                egui::Button::new(egui::RichText::new("Float").small()),
                            )
                            .on_hover_text("Float this panel into a window over the stage")
                            .clicked()
                        {
                            out.section_floats.push((section.front, (340.0, 180.0)));
                        }
                    }
                }

                // Whatever the menu left over holds the roll-up triangle and
                // the tabs.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    // **The grip.** Drag it to carry the whole section — onto
                    // another panel to tab with it, into a column to dock
                    // there, or onto the stage to float it. A drag rather
                    // than the menu, because it is the gesture everyone
                    // tries first, and because where the pointer lands says
                    // where the panel goes without a list to read.
                    //
                    // **Only where there is room.** The tools column is 55
                    // points at its narrowest, and the menu, the triangle and
                    // a tab strip have to share that with the frame's margins;
                    // a grip on top of them left a truncated tab overrunning
                    // the column by two points, and an overrun is a column
                    // that reports itself wider than it was drawn (see
                    // `dock_geometry_tests`). A narrow column keeps its handle
                    // anyway: the roll-up triangle below drags too. A floating
                    // window has no triangle and plenty of width, so it always
                    // gets the grip.
                    let roomy = ui.available_width() >= 120.0;
                    if !collapsible || roomy {
                        let grip = ui
                            .scope(|ui| {
                                ui.spacing_mut().button_padding = egui::vec2(1.0, 1.0);
                                ui.add(
                                    egui::Button::new(egui::RichText::new(GRIP).small().weak())
                                        .frame(false)
                                        .sense(egui::Sense::drag()),
                                )
                            })
                            .inner
                            .on_hover_text(if locked {
                                "The layout is locked"
                            } else {
                                "Drag to move this section: drop it on a panel to \
                                 tab with it, between panels to dock there, or on \
                                 the stage to float it"
                            });
                        if !locked {
                            grip.dnd_set_drag_payload(DraggedPanels(section.panels.clone()));
                        }
                        // Where the grip was drawn, for the tests that drive a
                        // drag through the real window: a guess at where a
                        // twelve-point button landed is how a test tests the
                        // guess rather than the drag.
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new(("grip", section.front)), grip.rect);
                        });
                    }

                    // The roll-up triangle, where every collapsible thing keeps
                    // one. Only docked sections get it: a floating window
                    // already has a close button, and rolling one up would
                    // leave a title bar adrift over the stage.
                    if collapsible {
                        // Clicked, it rolls the section up or opens it.
                        // Dragged, it carries the section, which is the whole
                        // of the drag handle a column too narrow for the grip
                        // has. egui tells the two apart by whether the pointer
                        // moved.
                        let triangle = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(if section.collapsed {
                                        ROLLED_UP
                                    } else {
                                        OPEN
                                    })
                                    .small(),
                                )
                                .frame(false)
                                // **Not widened to 24 px.** Tried first, and
                                // reverted: the Tools column is 55 pt at its
                                // narrowest (see the grip's own comment,
                                // above), and this triangle sharing that row
                                // with the grip and the panel menu overran the
                                // column the moment its width grew past its
                                // text — exactly the overrun
                                // `dock_geometry_tests::the_stage_never_reaches_into_a_dock_column`
                                // exists to catch. Exempted by name (`ROLLED_UP`/
                                // `OPEN`) in `hit_target_tests`, whose own
                                // comment carries the rest of this reasoning.
                                .sense(egui::Sense::click_and_drag()),
                            )
                            .on_hover_text(if section.collapsed {
                                "Rolled up \u{2014} click to open, or drag to move"
                            } else {
                                "Roll up, and keep the tabs \u{2014} or drag to move"
                            });
                        if triangle.clicked() {
                            out.collapses.push((section.front, !section.collapsed));
                        }
                        if !locked {
                            triangle.dnd_set_drag_payload(DraggedPanels(section.panels.clone()));
                        }
                    }

                    if section.is_tabbed() || !collapsible {
                        // **Tabs.** Wrapped, because five of them do not fit a
                        // 216-point column on one line and a tab that does not
                        // fit is one nobody can click.
                        // Floating windows always show tabs so they can be dragged out to dock.
                        ui.horizontal_wrapped(|ui| {
                            for id in &section.panels {
                                let front = *id == section.front;
                                // Stage 2, item 1: a text tab, not a filled
                                // selectable button — the front tab is told
                                // apart by its own colour and, once drawn, a
                                // 2 px accent underline. The button keeps its
                                // frame (unlike the grip and the roll-up
                                // triangle below) so it still picks up
                                // `hover` on hover — the ghost-button scope
                                // above already leaves it transparent at
                                // rest, so a resting tab paints nothing extra.
                                let label = egui::RichText::new(id.tab_title())
                                    .size(buzz_ui::theme::Tokens::TEXT_SMALL)
                                    .color(if front {
                                        Palette::text()
                                    } else {
                                        Palette::text_dim()
                                    });
                                // A tab can be dragged out on its own, which
                                // is how one panel leaves a group or docks from float.
                                let tab = ui
                                    .add(
                                        egui::Button::new(label)
                                            .truncate()
                                            .sense(egui::Sense::click_and_drag()),
                                    )
                                    .on_hover_text(if !collapsible {
                                        "Drag tab to dock this panel into any section or column"
                                    } else {
                                        id.title()
                                    });
                                if front {
                                    ui.painter().hline(
                                        tab.rect.x_range(),
                                        tab.rect.bottom(),
                                        egui::Stroke::new(2.0, accent),
                                    );
                                }
                                if tab.clicked() {
                                    out.selects.push(*id);
                                }
                                if !locked {
                                    tab.dnd_set_drag_payload(DraggedPanels(vec![*id]));
                                }
                                tab.context_menu(|ui| {
                                    if !locked {
                                        if !collapsible {
                                            if ui.button("Dock to Left").clicked() {
                                                out.moves.push((*id, buzz_ui::Dock::Left));
                                                ui.close();
                                            }
                                            if ui.button("Dock to Right").clicked() {
                                                out.moves.push((*id, buzz_ui::Dock::Right));
                                                ui.close();
                                            }
                                            if ui.button("Dock to Bottom").clicked() {
                                                out.moves.push((*id, buzz_ui::Dock::Bottom));
                                                ui.close();
                                            }
                                        } else {
                                            if ui.button("Float Panel").clicked() {
                                                out.floats.push((*id, (340.0, 180.0)));
                                                ui.close();
                                            }
                                        }
                                        ui.separator();
                                        if ui.button("Close Panel").clicked() {
                                            out.moves.push((*id, buzz_ui::Dock::Hidden));
                                            ui.close();
                                        }
                                    }
                                });
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(egui::Id::new(("header-drag", *id)), tab.rect);
                                });
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
                        //
                        // **And the name drags the section**, so the title bar is
                        // a grab handle the size of the whole strip, not just the
                        // two-colon grip — the gesture a hand tries first.
                        let room = ui.available_width().max(1.0);
                        let title = ui
                            .add_sized(
                                egui::vec2(room, ui.spacing().interact_size.y),
                                egui::Label::new(
                                    egui::RichText::new(section.front.title())
                                        .small()
                                        // The one panel here is always the
                                        // section's front (and only) tab, so
                                        // this reads the same as the tabbed
                                        // branch above: the front tab's name
                                        // is `text`, not `text_dim`.
                                        .color(Palette::text()),
                                )
                                .truncate()
                                .sense(egui::Sense::click_and_drag()),
                            )
                            .on_hover_text(if locked {
                                "The layout is locked"
                            } else {
                                "Drag the title to move this section"
                            });
                        if !locked {
                            title.dnd_set_drag_payload(DraggedPanels(section.panels.clone()));
                        }
                        // Where a title-bar drag can be started, for the tests
                        // that drive one through the real window.
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new(("header-drag", section.front)),
                                title.rect,
                            );
                        });
                    }

                    // **Whatever the row leaves over drags the section too.**
                    //
                    // The tabs, the name and the handles rarely fill the strip;
                    // the rest of it is empty, and in Animate that empty title
                    // bar is itself the grab handle. Allocating exactly the space
                    // that is left keeps the promise that nothing here overflows
                    // the column. The menu keeps its click and each tab keeps its
                    // own drag, because those sit on top of this.
                    //
                    // **Except the Timeline's layer tools**, which sit at the
                    // left of that empty run (user 2026-10-02: the strip they
                    // had under the grid was "eating space"). Only when the
                    // whole row fits; otherwise the panel keeps its strip.
                    let inline = buzz_ui::timeline_panel::LAYER_TOOLS_INLINE_WIDTH;
                    if section.front == buzz_ui::PanelId::Timeline
                        && !section.collapsed
                        && ui.available_width() >= inline
                    {
                        let height = ui.available_height().max(buzz_ui::theme::Tokens::CONTROL_H);
                        let (slot, _) =
                            ui.allocate_exact_size(egui::vec2(inline, height), egui::Sense::hover());
                        tools_slot = Some(slot);
                    }
                    let rest = ui.available_size();
                    if rest.x > 1.0 {
                        let (_, drag) = ui.allocate_exact_size(rest, egui::Sense::click_and_drag());
                        if !locked {
                            drag.dnd_set_drag_payload(DraggedPanels(section.panels.clone()));
                        }
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new(("header-drag", section.front)), drag.rect);
                        });
                    }
                });
            });
        });
    });
    tools_slot
}

/// The panel menu's label.
///
/// Three dots, not the hamburger every docking interface uses: that
/// character has **no glyph** in egui's bundled font and would draw as an
/// empty box. `theme::font_has` said so before it reached a screenshot,
/// which is the whole reason that check exists.
const PANEL_MENU: &str = "...";

/// The drag handle on every section header. Two colons: in the bundled font
/// beyond doubt, and it reads as a grip.
const GRIP: &str = "::";

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

    /// **Three columns and a stage must add up to the window.**
    ///
    /// The sequel to the test below, and the same shape of defect from the
    /// other end. Every column reports itself honestly now — but `egui` gives
    /// each side to whichever panel asks first and clamps it to what is left,
    /// so when the three of them together want more than the window has, the
    /// last one asked is placed *over* its neighbour. Dragging the stage's
    /// left boundary outwards is the quickest way to arrange that: the left
    /// column grows, the stage is squeezed to a sliver, and then the
    /// Properties column is drawn across the far-right one.
    ///
    /// So the widths are fitted to the window before anything is drawn, and
    /// this lays the panels out exactly as `docks` does — bottom, left, far
    /// right, right — and measures that no two of them share a point.
    #[test]
    fn the_columns_never_overlap_however_wide_they_are_dragged() {
        // A wide window, a laptop, and a window too narrow for the columns'
        // own minimums — with the boundaries dragged to their extremes.
        for room in [1920.0f32, 1280.0, 1000.0, 700.0, 420.0] {
            for (left, right, outer) in [
                (
                    *buzz_ui::workspace::LEFT_WIDTH_RANGE.end(),
                    *buzz_ui::workspace::COLUMN_WIDTH_RANGE.end(),
                    *buzz_ui::workspace::COLUMN_WIDTH_RANGE.end(),
                ),
                (60.0, 300.0, 240.0),
                (*buzz_ui::workspace::LEFT_WIDTH_RANGE.end(), 300.0, 240.0),
            ] {
                let mut workspace = buzz_ui::Workspace::animate();
                // `Dock::RightOuter` is empty by default now that the
                // inspector's one tab stack holds everything it used to
                // share the side with — but it is still a real column a
                // panel can be dragged into (the AI panel's own home), and
                // this test is about what happens when all three columns
                // are actually in use, dragged to their extremes.
                workspace.move_to(buzz_ui::PanelId::Ai, buzz_ui::Dock::RightOuter);
                workspace.left_width = left;
                workspace.right_width = right;
                workspace.right_outer_width = outer;

                let ctx = egui::Context::default();
                buzz_ui::theme::apply(&ctx);
                let mut edges: Vec<(&str, egui::Rangef)> = Vec::new();

                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::pos2(0.0, 0.0),
                            egui::vec2(room, 1040.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let available = ui.available_rect_before_wrap().width();
                        let columns = workspace.column_widths(available);
                        // The order in `docks`: left first, then the far-right
                        // column, then the one beside the stage.
                        let left = egui::Panel::left("left")
                            .resizable(false)
                            .exact_size(columns.left)
                            .show(ui, |ui| {
                                ui.label("tools");
                            });
                        let outer = egui::Panel::right("outer")
                            .resizable(false)
                            .exact_size(columns.right_outer)
                            .show(ui, |ui| {
                                ui.label("library");
                            });
                        let right = egui::Panel::right("right")
                            .resizable(false)
                            .exact_size(columns.right)
                            .show(ui, |ui| {
                                ui.label("properties");
                            });
                        let stage = ui.available_rect_before_wrap();
                        edges.push(("left column", left.response.rect.x_range()));
                        edges.push(("stage", stage.x_range()));
                        edges.push(("right column", right.response.rect.x_range()));
                        edges.push(("far-right column", outer.response.rect.x_range()));
                    },
                );

                // Laid out left to right, each one starts where the last ended.
                for pair in edges.windows(2) {
                    let (before, after) = (&pair[0], &pair[1]);
                    assert!(
                        after.1.min >= before.1.max - 1.0,
                        "in a {room}-point window with columns dragged to                          {left}/{right}/{outer}, the {} ({:?}) is drawn over                          the {} ({:?})",
                        after.0,
                        after.1,
                        before.0,
                        before.1,
                    );
                }
                // And nothing hangs off the end of the window.
                let last = edges.last().expect("panels were laid out");
                assert!(
                    last.1.max <= room + 1.0,
                    "the {} runs {:.0} points past the edge of a {room}-point                      window",
                    last.0,
                    last.1.max - room
                );
            }
        }
    }

    /// **The stage must stop where the panels start.**
    ///
    /// The report was that the stage's scrollbar was drawn across the
    /// Properties panel, and that the panel would not shrink. Both are one
    /// fact, and it is a subtle one: a panel's *reported* rectangle in `egui`
    /// is its **contents'**, not the box it was placed in. A right-hand column
    /// whose rows are wider than the column does not simply overflow — `egui`
    /// trims the reported rect back to the panel's width from the **right**,
    /// so the rect slides bodily rightwards by however much the contents
    /// overran. The parent then hands the central panel everything left of
    /// that slid edge, and the stage is laid out *through* the column: ruler,
    /// scroll bars and zoom pill painted over the panel, the artwork ending
    /// 170 points short of where the stage says it ends.
    ///
    /// Measured against the whole application rather than a stand-in, at the
    /// narrowest a column can be dragged to, because the defect only appears
    /// once something real in the column does not fit.
    #[test]
    fn the_stage_never_reaches_into_a_dock_column() {
        let mut app = App::new(GpuPreference::default());
        // Every panel there is, in the one column — which is a layout a user
        // can arrange, and the one that asks the most of its width.
        for id in buzz_ui::PanelId::ALL {
            if id != buzz_ui::PanelId::Timeline {
                app.editor.workspace.move_to(id, buzz_ui::Dock::Right);
            }
        }
        // The narrowest the splitters allow, which is where a row is most
        // likely not to fit.
        app.editor.workspace.right_width = *buzz_ui::workspace::COLUMN_WIDTH_RANGE.start();
        app.editor.workspace.right_outer_width = *buzz_ui::workspace::COLUMN_WIDTH_RANGE.start();

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        // Twice: the first frame has no scroll state and the second does, and
        // it was the settled frame that drifted.
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(1226.0, 1002.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let stage = app.build_ui(ui);
                    for (dock, rect) in &app.dock_rects {
                        match dock {
                            buzz_ui::Dock::Right | buzz_ui::Dock::RightOuter => assert!(
                                stage.right() <= rect.left() + 1.0,
                                "the stage runs to {:.0} but the {dock:?} column                                  starts at {:.0} — {:.0} points of stage drawn                                  over the panel",
                                stage.right(),
                                rect.left(),
                                stage.right() - rect.left(),
                            ),
                            buzz_ui::Dock::Left => assert!(
                                stage.left() >= rect.right() - 1.0,
                                "the stage starts at {:.0} but the left column                                  runs to {:.0}",
                                stage.left(),
                                rect.right(),
                            ),
                            _ => {}
                        }
                    }
                    // And the columns still do not reach past the window.
                    for (_, rect) in &app.dock_rects {
                        assert!(
                            rect.right() <= 1227.0,
                            "a column runs {:.0} points off the right of the window",
                            rect.right() - 1226.0
                        );
                    }
                },
            );
        }
    }

    /// **Two rows given back to the stage and the timeline** (user
    /// 2026-10-02). The layer tools (Folder, Camera, Parent, Depth, the size
    /// sliders) sit on the Timeline's own header line, beside its tab, not
    /// in a strip under the grid; and the zoom readout sits in the menu row,
    /// not in a status bar along the bottom of the window.
    ///
    /// Measured from what is actually painted: where the "Folder", "Timeline"
    /// and "Zoom" text lands in a full-HD window of the whole application.
    #[test]
    fn the_layer_tools_and_zoom_share_rows_that_were_already_there() {
        fn find(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<egui::Rect> {
            fn walk(shape: &egui::Shape, clip: egui::Rect, text: &str, out: &mut Vec<egui::Rect>) {
                match shape {
                    egui::Shape::Text(t) if t.galley.text() == text => {
                        let rect = t.visual_bounding_rect();
                        if clip.intersects(rect) {
                            out.push(rect);
                        }
                    }
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, text, out)),
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for c in shapes {
                walk(&c.shape, c.clip_rect, text, &mut out);
            }
            out
        }

        let mut app = App::new(GpuPreference::default());
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let mut shapes = Vec::new();
        let mut stage = egui::Rect::NOTHING;
        for _ in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1920.0, 1040.0),
                    )),
                    ..Default::default()
                },
                |ui| stage = app.build_ui(ui),
            );
            shapes = output.shapes;
        }

        let tab = find(&shapes, "Timeline");
        let folder = find(&shapes, "Folder");
        let zoom = find(&shapes, "Zoom");
        assert_eq!(folder.len(), 1, "the Folder button is drawn once: {folder:?}");
        assert_eq!(zoom.len(), 1, "the Zoom label is drawn once: {zoom:?}");
        let tab = tab
            .iter()
            .find(|r| (r.center().y - folder[0].center().y).abs() < 4.0)
            .unwrap_or_else(|| {
                panic!("no Timeline tab on the Folder button's line: tabs {tab:?}, Folder {folder:?}")
            });
        assert!(
            folder[0].left() > tab.right(),
            "the layer tools start right of the Timeline tab ({tab:?}, {folder:?})"
        );
        assert!(
            zoom[0].bottom() <= stage.top(),
            "the Zoom readout ({:?}) sits above the stage ({stage:?}), in the menu row",
            zoom[0]
        );
        assert!(zoom[0].top() < 40.0, "the Zoom readout is in the top row: {:?}", zoom[0]);
    }

    /// **The breadcrumb renders at every depth**, including the form it takes
    /// inside a symbol: the scene name a link back to the main timeline, the
    /// scene menu folded onto the caret beside it, and each open ancestor its
    /// own link. A smoke test — it catches id collisions, missing glyphs and
    /// layout panics in the new branch, and confirms the strip raises no command
    /// on its own.
    #[test]
    fn the_breadcrumb_renders_at_the_root_and_inside_nested_symbols() {
        let mut app = App::new(GpuPreference::default());

        // Two symbols, to be opened one inside the other for a two-level path.
        let mut ids = None;
        app.editor.doc.edit("symbols", |scene| {
            let outer = scene.add_symbol("Outer", buzz_scene::SymbolKind::Graphic, None);
            let inner = scene.add_symbol("Inner", buzz_scene::SymbolKind::MovieClip, None);
            ids = Some((outer, inner));
        });
        let (outer, inner) = ids.expect("the symbols");

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1226.0, 1002.0),
            )),
            ..Default::default()
        };

        // At the main timeline the strip is the scene crumb alone.
        let _ = ctx.run_ui(input(), |ui| {
            assert!(app.editor.scene().edit_path().is_empty());
            assert!(app.breadcrumb(ui).is_none());
        });

        // Open both symbols, deepest last.
        app.editor.doc.edit_view(|scene| {
            scene.enter_symbol(outer);
            scene.enter_symbol(inner);
        });
        assert_eq!(
            app.editor.scene().edit_path().to_vec(),
            vec![outer, inner],
            "both symbols are on the edit path, outermost first"
        );

        // The two-level strip renders without panic and raises nothing itself.
        let _ = ctx.run_ui(input(), |ui| {
            assert!(app.breadcrumb(ui).is_none());
        });
    }

    /// **Deleting an asset takes two clicks**, because a file on disk has no
    /// undo. The first arms the confirm and leaves the file; the second removes
    /// it.
    #[test]
    fn deleting_an_asset_needs_a_second_click_to_confirm() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut app = App::new(GpuPreference::default());
        app.editor.assets = buzz_doc::AssetLibrary::at(dir.path());
        let asset = app
            .editor
            .assets
            .save("Oak", "", &buzz_scene::Scene::default())
            .expect("save");
        assert_eq!(app.editor.assets.len(), 1);

        // First click: armed, nothing deleted.
        app.apply_asset_action(buzz_ui::AssetAction::Delete(asset.clone()));
        assert_eq!(
            app.editor.assets.len(),
            1,
            "the first click must not delete"
        );
        assert!(
            app.editor.assets_panel.confirm_delete_asset.is_some(),
            "the first click arms the confirmation"
        );

        // Second click: confirmed and gone, arming cleared.
        app.apply_asset_action(buzz_ui::AssetAction::Delete(asset));
        assert_eq!(app.editor.assets.len(), 0, "the second click deletes");
        assert!(
            app.editor.assets_panel.confirm_delete_asset.is_none(),
            "and the arming is cleared"
        );
    }

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
                            egui::ScrollArea::vertical()
                                .id_salt("probe")
                                .show(ui, |ui| {
                                    let mut requests = DockRequests::default();
                                    let workspace = buzz_ui::Workspace::animate();
                                    let all_fronts: Vec<buzz_ui::PanelId> =
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
                                            &workspace,
                                            &all_fronts,
                                            false,
                                            true,
                                            true,
                                            None,
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
                                        &workspace,
                                        &all_fronts,
                                        false,
                                        true,
                                        true,
                                        None,
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

    /// S0 (`docs/BIMARI5_LAG_PLAN.md`): the status bar's "ms" is this frame's
    /// profiled work, not the wall-clock gap since the last one.
    ///
    /// This `App` has never had a window (`active` is `None`, as it is for
    /// every headless test in this file), so the old readout —
    /// `active.frame_ms`, which on a real window is however long
    /// `ControlFlow::Wait` sat idle before this frame was asked for, easily
    /// seconds — had nothing to read and silently fell back to `0.0`. A
    /// document that draws in a few ms but is only asked to redraw once every
    /// couple of seconds is exactly the case the old number lied about, and
    /// exactly what a genuine 250 ms+ hang looks like next to it: `active`
    /// makes that gap real, `profiler` never sees it, because nothing between
    /// `begin_frame` and `end_frame` ever waits. This test never opens a
    /// window either; it drives `profiler` directly and checks the readout
    /// tracks it exactly, small, regardless of any such gap.
    #[test]
    fn frame_ms_display_reads_the_profilers_work_not_the_idle_gap() {
        let app = App::new(GpuPreference::Automatic);
        assert!(app.active.is_none(), "this test is headless: no window, no gap to read");

        let mut app = app;
        app.profiler.begin_frame();
        app.profiler.enter(crate::profile::Section::Ui);
        std::thread::sleep(Duration::from_millis(3));
        app.profiler.enter(crate::profile::Section::Present);
        std::thread::sleep(Duration::from_millis(3));
        app.profiler.end_frame();

        let work_ms = app.profiler.last().total().as_secs_f64() * 1000.0;
        let displayed = f64::from(app.frame_ms_display());
        assert!(
            (displayed - work_ms).abs() < 0.05,
            "displayed {displayed} ms should equal the profiler's own total {work_ms} ms"
        );
        // A stand-in for the kind of gap `active.frame_ms` used to report:
        // this frame's actual sections took a handful of milliseconds, not
        // the ~2000 ms an idle wait between frames could easily be.
        assert!(
            displayed < 500.0,
            "displayed {displayed} ms should be this frame's small work total, not a multi-second idle gap"
        );
    }

    /// Stage 2, item 6 of `docs/WORKSPACE_DESIGN_PLAN.md`: "a headless
    /// `build_ui` test asserting no widget uses the removed colour roles";
    /// the second half of that fence — the count of call sites still using
    /// them — is `theme::tests::deprecated_colour_roles_are_a_tracked_list_not_a_growing_one`
    /// in `buzz-ui`, which is where every one of them actually lives. This
    /// half is the "headless build_ui" part: the tab-strip `section_header`
    /// rewrite, ghost buttons and the flat dock chrome draw at all, in every
    /// theme, with a document open, tabbed sections, a rolled-up section and
    /// a floating one — the paths through the header that were rewritten —
    /// without panicking. The snapshot that shows what it looks like is
    /// `ui_snapshot::tests::ui_snapshots`'s `dark_default.png`: Essentials
    /// (Stage 4) does not exist yet, so there is no separate
    /// "Dark/Essentials" snapshot to take.
    #[test]
    fn the_tab_strip_header_draws_in_every_theme() {
        let was = buzz_ui::theme::theme();
        for t in buzz_ui::theme::Theme::ALL {
            buzz_ui::theme::set_theme(t);
            let mut app = crate::ui_snapshot::fixture_app();
            // A second section on the right column, sharing Properties'
            // group, so a tabbed strip is actually drawn rather than only
            // ever the single-panel, named-title path.
            app.editor
                .workspace
                .group_with(buzz_ui::PanelId::Filters, buzz_ui::PanelId::Properties);
            // And one rolled up, so the collapsed chevron-before-tabs path
            // runs too.
            if let Some(slot) = app.editor.workspace.slot_mut(buzz_ui::PanelId::Layers) {
                slot.collapsed = true;
            }
            let ctx = egui::Context::default();
            buzz_ui::theme::apply(&ctx);
            let screen =
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
            for _ in 0..2 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        app.build_ui(ui);
                    },
                );
            }
        }
        buzz_ui::theme::set_theme(was);
    }

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
                // has focus, by `script_shortcuts` — `TestMovie` shares
                // `RunScript`'s chord and is resolved there the same way.
                Command::RunScript | Command::TestMovie | Command::ToggleActionsPanel
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

    /// **D1, repro first.** Ctrl+S on a new document resolves off the UI
    /// thread — the picker's own thread hands the chosen path back over a
    /// channel, and `poll_picker` picks it up. By the time that happens, the
    /// title row for *this* frame has already been drawn from the document
    /// as it stood before the save. If nothing then asks for another frame,
    /// egui goes idle the moment the picker and the task queue are both
    /// clear, and the stale "Untitled" sits on screen until some unrelated
    /// mouse or key event happens to redraw the window — exactly the "title
    /// bar row is only repainted on input" mechanism `docs/DOCUMENTS_PLAN.md`
    /// names. `Document::save_as` itself renames the document correctly
    /// (`app.editor.doc.title()` below); what this test is really pinning
    /// down is that a repaint is asked for *this* frame instead of one being
    /// merely possible in the abstract.
    #[test]
    fn saving_as_asks_for_a_repaint_so_the_title_does_not_go_stale() {
        let mut app = App::new(GpuPreference::Automatic);
        assert_eq!(app.editor.doc.title(), "Untitled");

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
        let frame = |app: &mut App, t: f64| {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(t),
                    ..Default::default()
                },
                |ui| {
                    app.build_ui(ui);
                },
            );
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.repaint_delay)
                .unwrap_or(std::time::Duration::MAX)
        };

        // Warm up: a freshly-shown `ScrollArea` (the Tools column) flashes its
        // scrollbar for a few tenths of a second regardless of the document,
        // and keeps asking for a repaint for as long as that animation runs.
        // Settle it first, on real time, so it does not mask the one repaint
        // this test actually cares about.
        for i in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame(&mut app, i as f64 * 0.02);
        }
        let settled = frame(&mut app, 0.42);
        assert_eq!(
            settled,
            std::time::Duration::MAX,
            "the window did not go idle even before any save — the picker/task \
             repaint this test checks for would be masked by this"
        );

        // Ctrl+S on a new document resolves off the UI thread: the dialog's
        // own thread hands the path back over a channel, and `poll_picker`
        // (called from inside `build_ui`, after the title row has already
        // drawn) picks it up.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Scene A.buzz");
        app.picker = crate::dialogs::Pending::resolved(Pick::SaveAs, Some(path.clone()));
        let repaint_delay = frame(&mut app, 0.44);

        // The save really happened — this is the completion handler calling
        // `save_to` on the same document, not a stray copy of it.
        assert_eq!(app.editor.doc.title(), "Scene A");
        assert_eq!(app.editor.doc.path(), Some(path.as_path()));
        assert!(!app.editor.doc.is_dirty());

        // And it asked for an immediate repaint, so the title just painted
        // stale does not sit there until the user happens to move the mouse.
        assert_eq!(
            repaint_delay,
            std::time::Duration::ZERO,
            "saving did not ask for an immediate repaint; the title drawn earlier \
             this frame would sit stale until unrelated input redraws the window"
        );
    }

    /// A tiny PNG on disk, `color` all over — enough for `Editor::import_image`
    /// to decode without needing a real picture.
    fn tiny_png(dir: &std::path::Path, name: &str, color: [u8; 4]) -> std::path::PathBuf {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba(color));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("encode png");
        let path = dir.join(name);
        std::fs::write(&path, &bytes).expect("write png");
        path
    }

    /// **D3.** Three pictures chosen at once for File ▸ Import Image all
    /// land — each its own object on the stage, each its own entry in the
    /// library — and the one status line says so.
    #[test]
    fn importing_three_images_at_once_adds_three_to_the_library() {
        let mut app = App::new(GpuPreference::Automatic);
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = vec![
            tiny_png(dir.path(), "red.png", [220, 30, 30, 255]),
            tiny_png(dir.path(), "green.png", [30, 220, 30, 255]),
            tiny_png(dir.path(), "blue.png", [30, 30, 220, 255]),
        ];

        app.import_images_from(paths);

        assert_eq!(
            app.editor.doc.scene().images().len(),
            3,
            "all three should have library entries"
        );
        assert_eq!(app.editor.status.as_deref(), Some("Imported 3 files"));
    }

    /// **D3.** One unreadable file among good ones reports the failure and
    /// still imports the rest — a bad file never stops the batch.
    #[test]
    fn one_bad_image_among_good_ones_reports_the_failure_and_imports_the_rest() {
        let mut app = App::new(GpuPreference::Automatic);
        let dir = tempfile::tempdir().expect("temp dir");
        let good_one = tiny_png(dir.path(), "red.png", [220, 30, 30, 255]);
        let bad = dir.path().join("not-a-picture.png");
        std::fs::write(&bad, b"this is not a picture").expect("write");
        let good_two = tiny_png(dir.path(), "blue.png", [30, 30, 220, 255]);

        app.import_images_from(vec![good_one, bad, good_two]);

        assert_eq!(
            app.editor.doc.scene().images().len(),
            2,
            "the two good pictures should still have arrived"
        );
        let status = app.editor.status.expect("a summary line");
        assert!(
            status.starts_with("Imported 3 files, 1 failed: not-a-picture.png"),
            "got: {status}"
        );
    }

    /// **D3.** Several images land staggered rather than stacked exactly on
    /// top of one another.
    #[test]
    fn a_batch_of_images_lands_staggered_not_stacked() {
        let mut app = App::new(GpuPreference::Automatic);
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = vec![
            tiny_png(dir.path(), "a.png", [220, 30, 30, 255]),
            tiny_png(dir.path(), "b.png", [30, 220, 30, 255]),
        ];

        app.import_images_from(paths);

        let frame = app.editor.current_frame;
        let scene = app.editor.doc.scene();
        let centers: Vec<_> = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(frame).iter())
            .map(|o| scene.resolved_bounds(o).center())
            .collect();
        assert_eq!(centers.len(), 2);
        assert!(
            centers[0].distance(centers[1]) > 1.0,
            "the two pictures landed on top of each other: {centers:?}"
        );
    }

    /// An imported document is named after the file it came from, even though
    /// it has no save path of its own — and Save offers a `.buzz` beside it.
    #[test]
    fn opening_an_import_takes_its_name() {
        let mut app = App::new(GpuPreference::Automatic);
        let imported = crate::import::Imported {
            scene: buzz_scene::Scene::default(),
            summary: String::new(),
            unsupported: Vec::new(),
            scene_name: None,
            more_scenes: Vec::new(),
        };
        app.finish_open_imported(std::path::Path::new("C:/x/bhatta2.fla"), imported);

        assert_eq!(app.editor.doc.title(), "bhatta2");
        assert!(app.editor.doc.path().is_none());

        let request = app.save_request();
        assert_eq!(request.suggested_file_name(), Some("bhatta2.buzz"));
        assert_eq!(
            request.suggested_directory(),
            Some(std::path::Path::new("C:/x"))
        );
    }

    /// **`docs/HARD_LIGHT_PLAN.md`'s Stage A, the defaults path.** The app
    /// opens on a brand-new document, and a brand-new document has nothing
    /// drawn against the old model to keep looking the way it did — so it
    /// starts on `Form`, not on the `Crescent` a file predating this plan
    /// loads as (`buzz-doc`'s own `a_file_without_a_model_loads_as_crescent`
    /// proves that half).
    #[test]
    fn a_brand_new_document_starts_on_the_form_model() {
        let app = App::new(GpuPreference::Automatic);
        assert_eq!(
            app.editor.scene().lights().model,
            buzz_scene::ShadeModel::Form,
            "the app's own new document did not start on Form"
        );
    }

    /// **The same defaults path, for an import.** `crate::import::read` never
    /// touches `lights` — an imported `.fla` is a new document as far as
    /// lighting is concerned, and `finish_open_imported` must not quietly
    /// hand it the old model either.
    #[test]
    fn an_imported_document_also_starts_on_the_form_model() {
        let mut app = App::new(GpuPreference::Automatic);
        let imported = crate::import::Imported {
            scene: buzz_scene::Scene::default(),
            summary: String::new(),
            unsupported: Vec::new(),
            scene_name: None,
            more_scenes: Vec::new(),
        };
        app.finish_open_imported(std::path::Path::new("C:/x/bhatta2.fla"), imported);

        assert_eq!(
            app.editor.scene().lights().model,
            buzz_scene::ShadeModel::Form,
            "an imported document did not start on Form"
        );
    }

    fn ctrl_enter() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }
    }

    fn test_movie_screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0))
    }

    /// Ctrl+Enter opens Test Movie when the Actions code editor does not have
    /// focus — Animate's own shortcut, repurposed the moment there is
    /// nothing to run.
    #[test]
    fn ctrl_enter_opens_test_movie_outside_the_script_editor() {
        let mut app = App::new(GpuPreference::Automatic);
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let input = egui::RawInput {
            events: vec![ctrl_enter()],
            screen_rect: Some(test_movie_screen()),
            ..Default::default()
        };

        let _ = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });

        assert!(
            app.test_movie.is_some(),
            "Ctrl+Enter outside the code editor should open Test Movie"
        );
    }

    /// The same chord runs the script instead when the Actions code editor
    /// has focus — the one place Ctrl+Enter already meant something.
    #[test]
    fn ctrl_enter_runs_the_script_when_the_code_editor_has_focus() {
        let mut app = App::new(GpuPreference::Automatic);
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        ctx.memory_mut(|m| {
            m.request_focus(egui::Id::new(buzz_ui::actions_panel::CODE_EDITOR_ID));
        });
        let input = egui::RawInput {
            events: vec![ctrl_enter()],
            screen_rect: Some(test_movie_screen()),
            ..Default::default()
        };

        let _ = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });

        assert!(
            app.test_movie.is_none(),
            "Ctrl+Enter with the code editor focused must not open Test Movie"
        );
        assert_eq!(
            app.editor.status.as_deref(),
            Some("Write a script in the Actions panel first"),
            "RunScript should have run instead of Test Movie"
        );
    }

    /// Esc closes Test Movie.
    #[test]
    fn escape_closes_test_movie() {
        let mut app = App::new(GpuPreference::Automatic);
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        app.dispatch(Command::TestMovie);
        assert!(app.test_movie.is_some(), "Test Movie should have opened");

        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            screen_rect: Some(test_movie_screen()),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
            app.import_report_window(ui.ctx());
            app.test_movie_overlay(ui.ctx());
        });

        assert!(
            app.test_movie.is_none(),
            "Escape should have closed Test Movie"
        );
    }

    /// Follow-up 2026-09-26, item 3: View > Pasteboard Colour…'s override
    /// wins over the theme once set, and "Match theme" (`None`, the
    /// default) falls back to `Palette::pasteboard()` — see `pasteboard`'s
    /// own doc comment.
    #[test]
    fn the_pasteboard_override_wins_over_the_theme() {
        let mut app = App::new(GpuPreference::Automatic);
        let theme_colour = Palette::pasteboard();
        assert_eq!(
            pasteboard(&app.editor.workspace),
            Color::from_rgba8(theme_colour.r(), theme_colour.g(), theme_colour.b(), 255),
            "no override: matches the theme"
        );

        app.editor.workspace.pasteboard = Some([0x11, 0x22, 0x33, 0xFF]);
        assert_eq!(
            pasteboard(&app.editor.workspace),
            Color::from_rgba8(0x11, 0x22, 0x33, 255),
            "an override wins over the theme"
        );
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

    #[test]
    fn ai_models_dialog_opens_and_renders_through_ui() {
        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        assert!(!app.editor.ai_models.open);
        app.editor.run(buzz_ui::Command::AiModels);
        assert!(app.editor.ai_models.open);

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });
        buzz_ui::ai_models_dialog(&ctx, &mut app.editor.ai_models, &mut app.editor.workspace.ai);
        assert!(app.editor.ai_models.open);
    }

    #[test]
    fn ai_character_dialog_opens_and_renders_through_ui() {
        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        assert!(!app.editor.ai_character.open);
        app.editor.run(buzz_ui::Command::AiCharacterFromImage);
        assert!(app.editor.ai_character.open);

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });
        let _ = buzz_ui::ai_character_dialog(&ctx, &mut app.editor.ai_character);
        assert!(app.editor.ai_character.open);
    }

    /// F7.0: every AI command lands in the AI panel — the three that open a
    /// dialog body (Models, Character from Image, Identify & Rename) and the
    /// three the shell runs itself (Direct Story, Auto-Rig, Scene from
    /// Image, whose file picker is not opened here) — and the panel then
    /// draws inside the real window, every section title on screen.
    #[test]
    fn every_ai_command_reveals_the_ai_panel_and_it_draws_through_the_ui() {
        for command in [
            buzz_ui::Command::AiModels,
            buzz_ui::Command::AiCharacterFromImage,
            buzz_ui::Command::AiIdentifyAssets,
            buzz_ui::Command::AiDirectStory,
            buzz_ui::Command::AiRigVectorCharacter,
        ] {
            let mut app = App::new(GpuPreference::Automatic);
            app.recovery.found.clear();
            assert!(!app.editor.workspace.is_open(buzz_ui::PanelId::Ai), "hidden by default");
            app.dispatch(command);
            assert!(
                app.editor.workspace.is_open(buzz_ui::PanelId::Ai),
                "{command:?} did not reveal the AI panel"
            );
        }

        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();
        app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let mut texts = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| {
                app.build_ui(ui);
            });
            texts = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>();
        }
        let all = texts.join("\n");
        // The column scrolls, so only the top of the panel is on screen; the
        // unit test `ai_panel_renders_every_section_headlessly` draws all
        // eight sections.
        for want in ["Models", "Claude API server", "Test connection", "Voice", "Allow all"] {
            assert!(all.contains(want), "{want} not on screen");
        }
        assert!(!all.contains("sk-local-d"), "a stored key reached the screen");
    }

    /// F7.0/F7.9: a finished transcription waits in the panel, and Apply
    /// writes a Captions layer (one undo step family) — the path the Transcribe
    /// section drives, without the model.
    #[test]
    fn a_transcript_applies_as_captions_from_the_ai_panel() {
        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();
        let wav = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../buzz-ai/tests/data/speech_short.wav"
        ));
        let name = app.editor.import_sound(&wav).expect("imports the clip");
        let (send, receive) = crossbeam_channel::bounded(1);
        let id = app
            .tasks
            .spawn_thread(crate::tasks::TaskKind::Ai, "AI", |_| crate::tasks::TaskOutcome::Finished(String::new()));
        let _ = send.send(AiResult::Transcribed {
            sound: name.clone(),
            transcript: Box::new(buzz_ai::transcribe::Transcript {
                segments: vec![
                    buzz_ai::transcribe::Segment { start: 0.0, end: 3.28, text: "Ramu, bring the water to the old man.".into() },
                    buzz_ai::transcribe::Segment { start: 3.28, end: 5.28, text: "The rain is coming tonight.".into() },
                ],
                audio_seconds: 5.5,
                load_ms: 1127,
                run_ms: 2159,
                provider: "DirectML".into(),
            }),
        });
        app.ai_results.insert(id, receive);
        app.collect_ai_result(id);
        let t = app.ai_panel.transcript.as_ref().expect("the transcript waits in the panel");
        assert_eq!(t.lines.len(), 2);
        assert!(app.editor.workspace.is_open(buzz_ui::PanelId::Ai));
        app.ai_panel.transcribe_speaker = "Ramu".into();
        app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::TranscriptApply);
        let captions = app
            .editor
            .doc
            .scene()
            .layers()
            .iter()
            .find(|l| l.name.to_lowercase().contains("caption"))
            .map(|l| l.frames.keyframes().iter().filter(|k| k.label.as_deref() == Some("Ramu")).count());
        assert_eq!(captions, Some(2), "status: {:?}", app.editor.status);
        assert!(app.ai_panel.transcript.is_none());
    }

    /// The AI character dialog's **Layers** kind: the character arrives as a
    /// layer rig (no armature, parts on followed layers) in one undo step, and
    /// the Cast copy is the same layer rig.
    #[test]
    fn an_ai_character_created_as_layers_is_a_layer_rig() {
        let png_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../buzz-ai/tests/data/pujariFront_0.png"
        );
        if !std::path::Path::new(png_path).exists() {
            eprintln!("Skipping test: {png_path} not found");
            return;
        }
        let mut app = App::new(GpuPreference::Automatic);
        let assets = tempfile::tempdir().expect("temp asset folder");
        app.editor.assets = buzz_doc::AssetLibrary::at(assets.path());
        app.recovery.found.clear();

        let assembled = assembled_test_character("Lay");
        let undo_before = app.editor.doc.history().undo_depth();
        app.editor.create_ai_character_as(&assembled, buzz_rig::RigKind::Layers);
        assert_eq!(app.editor.doc.history().undo_depth(), undo_before + 1);

        let is_layer_rig = |scene: &buzz_scene::Scene| {
            let armatures = scene
                .layers()
                .iter()
                .flat_map(|l| l.all_objects())
                .filter(|o| matches!(o.kind, buzz_scene::ObjectKind::Armature(_)))
                .count();
            let followers = scene.layers().iter().filter(|l| l.follows.is_some()).count();
            (armatures, followers)
        };
        let (armatures, followers) = is_layer_rig(app.editor.doc.scene());
        assert_eq!(armatures, 0, "status: {:?}", app.editor.status);
        assert!(followers > 0, "no followed layers");

        let cast = app
            .editor
            .assets
            .assets()
            .iter()
            .find(|a| a.folder == "Cast")
            .cloned()
            .expect("a Cast asset");
        let saved = app.editor.assets.load(&cast).expect("the Cast asset loads");
        let (armatures, saved_followers) = is_layer_rig(&saved);
        assert_eq!(armatures, 0);
        assert_eq!(saved_followers, followers);
    }

    /// LR2: the Assistant's character job commits as the call's `rig` says.
    /// Layers (the default): `rig_kind` "layers", no armature, and the root
    /// part's id to perform with. Bones: the armature id, as before.
    #[test]
    fn the_agent_character_job_reports_its_rig_kind_and_root_element() {
        let png_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../buzz-ai/tests/data/pujariFront_0.png"
        );
        if !std::path::Path::new(png_path).exists() {
            eprintln!("Skipping test: {png_path} not found");
            return;
        }
        let new_app = || {
            let mut app = App::new(GpuPreference::Automatic);
            let assets = tempfile::tempdir().expect("temp asset folder");
            app.editor.assets = buzz_doc::AssetLibrary::at(assets.path());
            app.recovery.found.clear();
            (app, assets)
        };

        let (mut app, _assets) = new_app();
        let layered = app.commit_agent_character(&assembled_test_character("Lay"), buzz_rig::RigKind::Layers);
        assert_eq!(layered["rig_kind"], serde_json::json!("layers"), "{layered}");
        assert!(layered["armature_element"].is_null(), "{layered}");
        let root = layered["rig_root_element"].as_u64().expect("a root element");
        assert!(
            buzz_act::layer_rig::rig_of(app.editor.doc.scene(), buzz_scene::ObjectId(root))
                .is_some_and(|r| matches!(r, buzz_act::layer_rig::CharacterRig::Layers(v) if v.body.0 == root)),
            "the root element is not the layer rig's root part: {layered}"
        );
        assert_eq!(layered["undo_steps_added"], serde_json::json!(1));

        let (mut app, _assets) = new_app();
        let boned = app.commit_agent_character(&assembled_test_character("Bone"), buzz_rig::RigKind::Bones);
        assert_eq!(boned["rig_kind"], serde_json::json!("bones"), "{boned}");
        assert!(boned["armature_element"].is_u64(), "{boned}");
        assert_eq!(boned["rig_root_element"], boned["armature_element"]);
    }

    /// Builds an `AssembledCharacter` the same way the non-ignored test in
    /// `buzz-ai/tests/assemble_character.rs` does: traditional CV only (no
    /// ONNX models), from the shared test fixture image and its known
    /// joints, under a chosen `name` so two calls give two independently
    /// named characters.
    fn assembled_test_character(name: &str) -> buzz_ai::character::AssembledCharacter {
        let png_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../buzz-ai/tests/data/pujariFront_0.png"
        );
        let img = image::open(png_path)
            .unwrap_or_else(|e| panic!("open {png_path}: {e}"))
            .to_rgba8();
        let matte = buzz_ai::segment::matte(&img, None).expect("matte");

        let joints_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../buzz-ai/tests/data/pujariFront_0.joints.json"
        );
        let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
        let true_joints: Vec<(String, (f64, f64))> =
            serde_json::from_str(&joints_str).expect("parse joints");

        let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
        for (joint_name, (x, y)) in &true_joints {
            let (xf, yf) = (*x as f32, *y as f32);
            match joint_name.as_str() {
                "Head" => keypoints[0] = (xf, yf, 0.95),
                "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
                "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
                "Elbow L" => keypoints[7] = (xf, yf, 0.95),
                "Elbow R" => keypoints[8] = (xf, yf, 0.95),
                "Hand L" => keypoints[9] = (xf, yf, 0.95),
                "Hand R" => keypoints[10] = (xf, yf, 0.95),
                "Thigh L" => keypoints[11] = (xf, yf, 0.95),
                "Thigh R" => keypoints[12] = (xf, yf, 0.95),
                "Knee L" => keypoints[13] = (xf, yf, 0.95),
                "Knee R" => keypoints[14] = (xf, yf, 0.95),
                "Foot L" => keypoints[15] = (xf, yf, 0.95),
                "Foot R" => keypoints[16] = (xf, yf, 0.95),
                _ => {}
            }
        }

        let person = buzz_ai::pose::Person {
            bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
            keypoints,
        };
        let skel = buzz_ai::character::skeleton(&person);
        let parts = buzz_ai::character::part_masks(&img, &matte, &skel).expect("part masks");
        let completed = buzz_ai::character::complete_parts(&img, &matte, &skel, &parts)
            .expect("complete parts");
        let traced =
            buzz_ai::character::trace_character(&completed, Some(2.0)).expect("trace character");

        buzz_ai::character::assemble_character(
            &skel,
            &traced,
            name,
            Some((img.width() as f64, img.height() as f64)),
            None,
        )
        .expect("assemble character")
    }

    /// **Creating an AI character must not overwrite the Library, and must
    /// place the rigged character on stage — as one undo step.**
    ///
    /// Every AI character is built the same way, so every one allocates the
    /// same fixed symbol ids: 1 for the character symbol, 100+i for its
    /// parts. Inserting those straight into the document's Library — which
    /// replaces by id — would silently destroy whatever already lived at
    /// those ids: the user's own symbols on the first character, and the
    /// first character's own symbols on the second. This seeds the document
    /// with symbols at exactly those ids, creates two characters, and checks
    /// that everything survives, both characters land on stage, and each
    /// creation is exactly one undo step.
    #[test]
    fn creating_an_ai_character_merges_instead_of_overwriting_the_library() {
        let png_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../buzz-ai/tests/data/pujariFront_0.png"
        );
        if !std::path::Path::new(png_path).exists() {
            eprintln!("Skipping test: {png_path} not found");
            return;
        }

        let mut app = App::new(GpuPreference::Automatic);
        // `create_ai_character` also files the character in the Cast, as the
        // wizard does. Without a scratch library this test wrote a new
        // "Ana N" and "Bo N" into the user's real asset folder on every run.
        let assets = tempfile::tempdir().expect("temp asset folder");
        app.editor.assets = buzz_doc::AssetLibrary::at(assets.path());
        app.recovery.found.clear();

        // Seed the document with user symbols at exactly the fixed ids every
        // AI character allocates (1, and 100..=130).
        app.editor.doc.edit("Seed user symbols", |scene| {
            scene.library_mut().insert(buzz_scene::Symbol::new(
                buzz_scene::SymbolId(1),
                "user_1".to_string(),
                buzz_scene::SymbolKind::Graphic,
            ));
            for i in 100..=130u64 {
                scene.library_mut().insert(buzz_scene::Symbol::new(
                    buzz_scene::SymbolId(i),
                    format!("user_{i}"),
                    buzz_scene::SymbolKind::Graphic,
                ));
            }
            scene.reserve_ids_above(130);
        });
        let baseline_names: Vec<String> = app
            .editor
            .doc
            .scene()
            .library()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(baseline_names.len(), 32, "1 plus 100..=130");

        let assembled_a = assembled_test_character("Ana");
        let assembled_b = assembled_test_character("Bo");
        let per_character_symbols = assembled_a.scene.library().len();

        let undo_before = app.editor.doc.history().undo_depth();
        app.editor.create_ai_character(&assembled_a);
        assert_eq!(
            app.editor.doc.history().undo_depth(),
            undo_before + 1,
            "creating a character is one undo step"
        );
        app.editor.create_ai_character(&assembled_b);
        assert_eq!(
            app.editor.doc.history().undo_depth(),
            undo_before + 2,
            "creating a character is one undo step"
        );

        let scene = app.editor.doc.scene();

        // Every original user symbol survived, under its own name.
        for name in &baseline_names {
            assert!(
                scene.library().iter().any(|s| &s.name == name),
                "user symbol {name} must survive creating two AI characters"
            );
        }

        // Both characters' whole symbol sets arrived too.
        assert_eq!(
            scene.library().len(),
            baseline_names.len() + per_character_symbols * 2,
            "library grew by exactly two characters' worth of symbols"
        );

        // Both rigged characters landed on stage.
        let armature_count = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .filter(|o| matches!(o.kind, buzz_scene::ObjectKind::Armature(_)))
            .count();
        assert_eq!(armature_count, 2, "both characters must be placed on stage");
    }

    /// Runs `build_ui` frames — which polls the AI Character wizard's
    /// background channels the same way a real frame loop would — until its
    /// step reaches `target`, or it reports an error, or `timeout` elapses.
    fn drive_ai_character_wizard_to(
        app: &mut App,
        ctx: &egui::Context,
        screen: egui::Rect,
        target: buzz_ui::AiCharacterStep,
        step_name: &str,
        timeout: Duration,
    ) -> Duration {
        let started = Instant::now();
        let deadline = started + timeout;
        loop {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                app.build_ui(ui);
            });
            if app.editor.ai_character.step == target {
                return started.elapsed();
            }
            if let Some((msg, true)) = &app.editor.ai_character.status_message {
                panic!("AI Character wizard failed during {step_name}: {msg}");
            }
            assert!(
                Instant::now() < deadline,
                "AI Character wizard step {step_name:?} timed out after {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Renders the stage at its current frame and writes it to `path`.
    fn render_ai_character_stage(app: &App, exporter: &mut buzz_export::Exporter, path: &std::path::Path) {
        let scene = app.editor.doc.scene();
        let settings = buzz_export::ExportSettings::for_stage(scene);
        let picture = exporter
            .render(scene, app.editor.current_frame, &settings)
            .unwrap_or_else(|e| panic!("render {path:?}: {e}"));
        picture
            .write_png(path)
            .unwrap_or_else(|e| panic!("write {path:?}: {e}"));
    }

    /// **"AI ▸ Character from Image" works end to end, through the real
    /// wizard UI state — not by calling the assembly pipeline directly.**
    ///
    /// Drives `App::new` + `build_ui` the way the app itself runs: opens the
    /// dialog the menu command opens, sets the image path the way
    /// "Browse…" would have left it (no real OS file dialog), and presses
    /// each wizard button by calling exactly what its click handler calls —
    /// `start_skeleton_detection`, `start_parts_processing`, and
    /// `create_character` in `crates/buzz-ui/src/ai_character_dialog.rs`,
    /// made `pub` for this test. A real pointer click at the buttons' screen
    /// rects was impractical: the joint editor's canvas and the preview's
    /// palette swatches both resize around the detected character, so the
    /// buttons below them do not sit at a rect known ahead of time.
    ///
    /// Needs the local ONNX models (matte, DWPose, part segmentation)
    /// installed, so it is `#[ignore]`d like the rest of the AI suite.
    #[test]
    #[ignore = "needs ONNX models"]
    fn character_from_image_end_to_end_through_the_ui() {
        for image_name in ["pujariFront_0.png", "OldmanfrontWalk_0.png"] {
            character_from_image_end_to_end_through_the_ui_for(image_name);
        }
    }

    fn character_from_image_end_to_end_through_the_ui_for(image_name: &str) {
        let png_path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../buzz-ai/tests/data/"
        ))
        .join(image_name);
        assert!(png_path.exists(), "missing fixture {png_path:?}");
        let stem = png_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("character")
            .to_string();

        let (img_w, img_h) = image::image_dimensions(&png_path)
            .unwrap_or_else(|e| panic!("read dimensions of {png_path:?}: {e}"));

        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();

        // The user's own Cast folder must not see this run's output.
        let assets_dir = tempfile::tempdir().expect("a temp dir for the asset library");
        app.editor.assets = buzz_doc::AssetLibrary::at(assets_dir.path());

        // Widen the stage so the character — and its posed silhouette — land
        // inside the frame the exporter renders, rather than being cropped
        // against the default 550x400 stage.
        let stage_w = img_w as f64 * 1.8;
        let stage_h = img_h as f64 * 1.5;
        app.editor.doc.edit("Widen Stage For Test", |scene| {
            scene.stage_mut().size = Size::new(stage_w, stage_h);
        });
        app.editor.camera.center = Point::new(stage_w / 2.0, stage_h / 2.0);

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 900.0));

        // The menu command: open the dialog.
        app.editor.run(buzz_ui::Command::AiCharacterFromImage);
        assert!(app.editor.ai_character.open, "the dialog did not open");

        // Set the path the way "Browse…" would have left it.
        app.editor.ai_character.image_path_input = png_path.display().to_string();
        app.editor.ai_character.character_name = format!("{stem} E2E");

        // Step 1 -> 2: "Detect Pose & Skeleton →"
        buzz_ui::ai_character_dialog::start_skeleton_detection(&mut app.editor.ai_character, &ctx);
        let detect_time = drive_ai_character_wizard_to(
            &mut app,
            &ctx,
            screen,
            buzz_ui::AiCharacterStep::JointEditor,
            "detect",
            Duration::from_secs(180),
        );

        // Step 2: accept the detected joints unchanged.
        assert_eq!(
            app.editor.ai_character.edited_joints,
            app.editor.ai_character.original_joints,
            "joints must start out unedited"
        );

        // Step 2 -> 3: "Next: Segment & Trace Parts →"
        buzz_ui::ai_character_dialog::start_parts_processing(&mut app.editor.ai_character, &ctx);
        let process_time = drive_ai_character_wizard_to(
            &mut app,
            &ctx,
            screen,
            buzz_ui::AiCharacterStep::Preview,
            "process",
            Duration::from_secs(180),
        );

        // Step 3 -> Create: "Create Character in Library & Cast"
        let baseline_library_len = app.editor.doc.scene().library().len();
        let undo_before = app.editor.doc.history().undo_depth();
        let create_started = Instant::now();
        let assembled =
            match buzz_ui::ai_character_dialog::create_character(&mut app.editor.ai_character) {
                buzz_ui::AiCharacterDialogAction::Created(assembled, _) => {
                    app.editor.create_ai_character(&assembled);
                    assembled
                }
                _ => panic!("Create Character in Library & Cast produced nothing"),
            };
        let create_time = create_started.elapsed();

        assert_eq!(
            app.editor.doc.history().undo_depth(),
            undo_before + 1,
            "creating the character must be exactly one undo step"
        );

        let scene = app.editor.doc.scene();
        assert_eq!(
            scene.library().len(),
            baseline_library_len + assembled.scene.library().len(),
            "the library must gain exactly this character's symbols"
        );

        let armatures: Vec<(buzz_scene::ObjectId, usize)> = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(app.editor.current_frame).iter())
            .filter_map(|o| match &o.kind {
                buzz_scene::ObjectKind::Armature(rig) => Some((o.id, rig.armature.bones.len())),
                _ => None,
            })
            .collect();
        assert_eq!(armatures.len(), 1, "exactly one Armature object must land on stage");
        let (armature_id, bone_count) = armatures[0];
        assert_eq!(bone_count, 11, "a biped rig has 11 bones");

        let cast_saved = app
            .editor
            .assets
            .assets()
            .iter()
            .any(|a| a.folder == "Cast" && a.name.contains(&stem));
        assert!(cast_saved, "the character must be saved as a Cast asset");

        // The face rig must actually be on the face: the eyes and mouth are
        // placed by `left_eye_center`/`right_eye_center`/`mouth_center` in
        // the same image-space coordinates as the Head part's own artwork,
        // so they must fall inside the Head part's (generously inflated)
        // bounds rather than floating off wherever a bad skeleton put them —
        // the regression `apply_edited_joints` (buzz-ui/src/ai_character_dialog.rs)
        // fixed.
        let head_symbol = assembled
            .part_symbols
            .iter()
            .find(|s| s.name.to_lowercase().ends_with("_head"))
            .unwrap_or_else(|| panic!("no Head part symbol among {:?}", assembled.part_symbols.iter().map(|s| &s.name).collect::<Vec<_>>()));
        let head_local_bounds = head_symbol
            .bounds()
            .unwrap_or_else(|| panic!("the Head symbol {:?} has no artwork", head_symbol.name));
        let reg = head_symbol.registration;
        let head_bounds = buzz_geom::Rect::new(
            head_local_bounds.x0 + reg.x,
            head_local_bounds.y0 + reg.y,
            head_local_bounds.x1 + reg.x,
            head_local_bounds.y1 + reg.y,
        );
        let margin = head_bounds.width().max(head_bounds.height()) * 0.2;
        let generous_head_bounds = head_bounds.inflate(margin, margin);
        let face = assembled
            .face
            .as_ref()
            .unwrap_or_else(|| panic!("[{image_name}] the assembled character must have a face rig"));
        for (label, center) in [
            ("left eye", face.left_eye_center),
            ("right eye", face.right_eye_center),
            ("mouth", face.mouth_center),
        ] {
            assert!(
                generous_head_bounds.contains(center),
                "[{image_name}] {label} at {center:?} must land on the head (head bounds {head_bounds:?}, generous {generous_head_bounds:?})"
            );
        }

        println!(
            "[{image_name}] detect {:.2}s, process {:.2}s, create {:.2}s",
            detect_time.as_secs_f64(),
            process_time.as_secs_f64(),
            create_time.as_secs_f64()
        );

        // Render the stage at rest, then posed, into the scratch directory.
        let out_dir = std::path::Path::new(
            "C:/Users/singh/AppData/Local/Temp/claude/B--youtubeProjects-Buzzcaf-Media-BuzzAnimate/ebc03a0c-3f7d-453b-95a6-b355d818e370/scratchpad/f6ui",
        );
        std::fs::create_dir_all(out_dir).expect("scratch output dir");

        let mut exporter =
            buzz_export::Exporter::new(&GpuPreference::Automatic).expect("a GPU to render with");

        let render_started = Instant::now();
        render_ai_character_stage(&app, &mut exporter, &out_dir.join(format!("{stem}_rest.png")));
        println!(
            "[{image_name}] render rest {:.2}s",
            render_started.elapsed().as_secs_f64()
        );

        for pose_name in ["walk", "wave"] {
            app.editor.doc.edit(format!("Test Pose: {pose_name}"), |scene| {
                let found = scene.update_object(armature_id, |target| {
                    if let buzz_scene::ObjectKind::Armature(rig) = &mut target.kind {
                        let applied = buzz_ai::motion::PoseLibrary::standard()
                            .apply_to_armature(pose_name, rig);
                        assert!(applied, "the standard library must know {pose_name:?}");
                    }
                });
                assert!(found, "the armature must still be on stage to pose");
            });
            let render_started = Instant::now();
            render_ai_character_stage(
                &app,
                &mut exporter,
                &out_dir.join(format!("{stem}_{pose_name}.png")),
            );
            println!(
                "[{image_name}] render {pose_name} {:.2}s",
                render_started.elapsed().as_secs_f64()
            );
        }
    }

    /// Spins `poll_tasks` until every background task has reported back, or
    /// gives up — a hung AI task would otherwise hang the test suite too.
    fn wait_for_ai_task(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.poll_tasks();
            if app.tasks.is_empty() {
                return;
            }
            assert!(Instant::now() < deadline, "a background AI task never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A 1×1 PNG — enough bytes to read as "a file". Its contents never
    /// matter in these tests: the backend below refuses the connection
    /// before any image would leave the machine.
    const MINIMAL_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// **AI commands never block the UI thread.**
    ///
    /// Direct Story's LLM call is seconds of network I/O; dispatching the
    /// command has to return long before that, and — pointed at a backend
    /// nothing is listening on — the task that follows has to fail cleanly:
    /// a status message, and the document exactly as it was, undo depth and
    /// all.
    #[test]
    fn ai_direct_story_dispatch_returns_immediately_and_fails_without_touching_the_document() {
        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();
        app.editor.workspace.ai.backend = Some(buzz_ai::llm::Backend::OpenAiCompatible {
            base_url: "http://127.0.0.1:9".into(),
        });
        app.editor.story.draft = "A hero walks across the clearing.".to_string();

        let undo_before = app.editor.doc.history().undo_depth();

        let started = Instant::now();
        app.dispatch(Command::AiDirectStory);
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "dispatching must not wait on the LLM"
        );

        wait_for_ai_task(&mut app);

        assert_eq!(
            app.editor.doc.history().undo_depth(),
            undo_before,
            "a failed AI task must not touch the document"
        );
        let status = app.editor.status.clone().unwrap_or_default();
        assert!(
            status.to_lowercase().contains("fail"),
            "expected a failure status, got {status:?}"
        );
    }

    /// The Scene from Image half of the same guarantee.
    ///
    /// `ai_scene_from_image_from` is the background-launching call
    /// `poll_picker` makes once a reference image is chosen, so it is
    /// exercised directly here rather than through a real OS file dialog,
    /// which a test must never open on the machine it runs on.
    #[test]
    fn ai_scene_from_image_runs_in_the_background_and_fails_without_touching_the_document() {
        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();
        app.editor.workspace.ai.backend = Some(buzz_ai::llm::Backend::OpenAiCompatible {
            base_url: "http://127.0.0.1:9".into(),
        });

        let tmp = tempfile::Builder::new()
            .suffix(".png")
            .tempfile()
            .expect("temp file");
        std::fs::write(tmp.path(), MINIMAL_PNG).expect("write a tiny png");

        let undo_before = app.editor.doc.history().undo_depth();

        let started = Instant::now();
        app.ai_scene_from_image_from(tmp.path().to_path_buf());
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "starting the background task must not wait on the VLM"
        );

        wait_for_ai_task(&mut app);

        assert_eq!(
            app.editor.doc.history().undo_depth(),
            undo_before,
            "a failed AI task must not touch the document"
        );
        let status = app.editor.status.clone().unwrap_or_default();
        assert!(
            status.to_lowercase().contains("fail"),
            "expected a failure status, got {status:?}"
        );
    }

    /// **Panels can be dragged where they should go.**
    ///
    /// Driven through the real `build_ui` with pointer events, because the
    /// question is never whether `dock_at` works — the workspace tests cover
    /// that — but whether picking a section up by its grip, carrying it and
    /// letting go lands it where the pointer is: a section becomes a tab
    /// there, a gap in a column docks there, the stage floats it.
    mod dock_drag_tests {
        use super::*;

        struct Drive {
            app: App,
            ctx: egui::Context,
            screen: egui::Rect,
        }

        impl Drive {
            fn new() -> Self {
                let mut app = App::new(GpuPreference::Automatic);
                // Explicit Animator layout: these tests drag panels
                // (Layers, Library, Filters, ...) that Essentials hides.
                app.editor.workspace = buzz_ui::Workspace::animate();
                app.recovery.found.clear();
                let ctx = egui::Context::default();
                buzz_ui::theme::apply(&ctx);
                // Tall enough that every section in the Right column's one
                // big tab stack — Color/Swatches and the utility stack below
                // it — still has a grip on screen: the inspector's six tabs
                // share Properties' own (fairly tall) body, and a shorter
                // window pushed everything after it down past the bottom
                // dock, where a click lands on the Timeline instead.
                let screen =
                    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1400.0));
                let mut drive = Self { app, ctx, screen };
                // Two frames to lay the window out and settle its scroll state.
                drive.frame(vec![]);
                drive.frame(vec![]);
                drive
            }

            fn frame(&mut self, events: Vec<egui::Event>) {
                let input = egui::RawInput {
                    events,
                    screen_rect: Some(self.screen),
                    ..Default::default()
                };
                let app = &mut self.app;
                let _ = self.ctx.run_ui(input, |ui| {
                    app.build_ui(ui);
                });
            }

            /// Where a section was drawn on the last frame.
            fn zone(&self, front: buzz_ui::PanelId) -> egui::Rect {
                self.app
                    .drop_zones
                    .iter()
                    .find(|z| z.front == front)
                    .unwrap_or_else(|| panic!("{front:?} was not drawn"))
                    .rect
            }

            /// The grip of a section, where the header drew it.
            fn grip(&self, front: buzz_ui::PanelId) -> egui::Pos2 {
                self.ctx
                    .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("grip", front))))
                    .unwrap_or_else(|| panic!("{front:?} has no grip on screen"))
                    .center()
            }

            /// A point on a section's title bar that is not the grip — the name
            /// or the empty part of the strip, which drags the whole section too.
            fn header(&self, front: buzz_ui::PanelId) -> egui::Pos2 {
                self.ctx
                    .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("header-drag", front))))
                    .unwrap_or_else(|| panic!("{front:?} has no title-bar drag zone"))
                    .center()
            }

            /// Press at `from`, carry to `to`, let go.
            fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
                let mods = egui::Modifiers::default();
                self.frame(vec![egui::Event::PointerMoved(from)]);
                self.frame(vec![egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: mods,
                }]);
                // In steps, as a hand moves, so the drag threshold is crossed
                // and the target has frames to be found in.
                for step in 1..=6 {
                    let t = step as f32 / 6.0;
                    self.frame(vec![egui::Event::PointerMoved(from + (to - from) * t)]);
                }
                assert!(
                    egui::DragAndDrop::has_payload_of_type::<DraggedPanels>(&self.ctx),
                    "nothing was picked up"
                );
                self.frame(vec![egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: mods,
                }]);
                self.frame(vec![]);
            }
        }

        /// Dropped on a section: a tab of it.
        #[test]
        fn dropping_a_section_on_another_makes_it_a_tab_there() {
            let mut d = Drive::new();
            let from = d.grip(buzz_ui::PanelId::Layers);
            let to = d.zone(buzz_ui::PanelId::Properties).center();

            d.drag(from, to);

            let section = d
                .app
                .editor
                .workspace
                .section_of(buzz_ui::PanelId::Layers)
                .expect("still on screen");
            assert!(
                section.panels.contains(&buzz_ui::PanelId::Properties),
                "Layers did not join Properties: {:?}",
                section.panels
            );
            assert_eq!(
                section.front,
                buzz_ui::PanelId::Layers,
                "the dropped tab comes to the front"
            );
        }

        /// Dropped in the gap between two sections: docked there, whole — every
        /// tab the grip carried, not just the one it happened to be gripped
        /// from.
        #[test]
        fn dropping_a_section_between_two_others_docks_it_there() {
            let mut d = Drive::new();
            // The Color section — still its own section, tabbed with
            // Swatches, since the inspector's one big stack absorbed Library
            // — carried within the same column: just above the Properties
            // section, in the gap between it and what is above.
            let from = d.grip(buzz_ui::PanelId::Color);
            let target = d.zone(buzz_ui::PanelId::Properties);
            let to = egui::pos2(target.center().x, target.top() - 1.0);

            d.drag(from, to);

            let workspace = &d.app.editor.workspace;
            assert_eq!(
                workspace.dock_of(buzz_ui::PanelId::Color),
                buzz_ui::Dock::Right
            );
            let side = workspace.on(buzz_ui::Dock::Right);
            let color = side
                .iter()
                .position(|p| *p == buzz_ui::PanelId::Color)
                .expect("in the right column");
            let properties = side
                .iter()
                .position(|p| *p == buzz_ui::PanelId::Properties)
                .expect("Properties is still there");
            assert!(
                color < properties && properties - color <= 3,
                "not just above Properties: {side:?}"
            );
            let section = workspace
                .section_of(buzz_ui::PanelId::Color)
                .expect("still on screen");
            assert_eq!(
                section.panels,
                vec![buzz_ui::PanelId::Color, buzz_ui::PanelId::Swatches,],
                "the grip carries the whole section, not just Color"
            );
            assert_eq!(section.front, buzz_ui::PanelId::Color);
        }

        /// **The whole title bar re-docks, not only the `::` grip.** Dragging a
        /// section by the empty part of its header carries it just as the grip
        /// does — the grab handle Animate gives you.
        #[test]
        fn dragging_a_sections_title_bar_re_docks_it() {
            let mut d = Drive::new();
            let from = d.header(buzz_ui::PanelId::Layers);
            let to = d.zone(buzz_ui::PanelId::Properties).center();

            d.drag(from, to);

            let section = d
                .app
                .editor
                .workspace
                .section_of(buzz_ui::PanelId::Layers)
                .expect("still on screen");
            assert!(
                section.panels.contains(&buzz_ui::PanelId::Properties),
                "dragging the title bar did not move Layers onto Properties: {:?}",
                section.panels
            );
        }

        /// Dropped on a section's lower edge: a new row below it, Animate's
        /// insertion-line behaviour — not a tab inside it.
        #[test]
        fn dropping_on_a_sections_lower_edge_docks_a_row_below_it() {
            let mut d = Drive::new();
            // Color — still its own section — since Library now tabs with
            // Properties rather than sitting apart from it.
            let from = d.grip(buzz_ui::PanelId::Color);
            let target = d.zone(buzz_ui::PanelId::Properties);
            // Just inside the bottom edge, where the green insertion line shows.
            let to = egui::pos2(target.center().x, target.bottom() - 2.0);

            d.drag(from, to);

            let workspace = &d.app.editor.workspace;
            let section = workspace
                .section_of(buzz_ui::PanelId::Color)
                .expect("still on screen");
            assert!(
                !section.panels.contains(&buzz_ui::PanelId::Properties),
                "Color tabbed into Properties instead of docking below it: {:?}",
                section.panels
            );
            let dock = workspace.dock_of(buzz_ui::PanelId::Properties);
            assert_eq!(
                workspace.dock_of(buzz_ui::PanelId::Color),
                dock,
                "Color landed in the same column as Properties"
            );
            let side = workspace.on(dock);
            let color = side
                .iter()
                .position(|p| *p == buzz_ui::PanelId::Color)
                .expect("in the column");
            let properties = side
                .iter()
                .position(|p| *p == buzz_ui::PanelId::Properties)
                .expect("in the column");
            assert!(
                color > properties,
                "Color must sit below Properties: {side:?}"
            );
        }

        /// Dropped on the stage: a window of its own, there.
        #[test]
        fn dropping_a_section_on_the_stage_floats_it_there() {
            let mut d = Drive::new();
            let from = d.grip(buzz_ui::PanelId::Layers);
            let to = egui::pos2(700.0, 450.0);

            d.drag(from, to);

            let workspace = &d.app.editor.workspace;
            assert_eq!(
                workspace.dock_of(buzz_ui::PanelId::Layers),
                buzz_ui::Dock::Float
            );
            assert_eq!(
                workspace.slot(buzz_ui::PanelId::Layers).unwrap().float_pos,
                (700.0, 450.0)
            );
            // And the window is drawn where it was dropped, not where egui
            // last remembered one.
            d.frame(vec![]);
            let rect = d.zone(buzz_ui::PanelId::Layers);
            assert!(
                (rect.left() - 700.0).abs() < 2.0 && (rect.top() - 450.0).abs() < 2.0,
                "the window was drawn at {:?}",
                rect.min
            );
        }

        /// A locked layout picks nothing up.
        #[test]
        fn a_locked_layout_cannot_be_dragged() {
            let mut d = Drive::new();
            d.app.editor.workspace.locked = true;
            d.frame(vec![]);
            let before = d.app.editor.workspace.clone();
            let from = d.grip(buzz_ui::PanelId::Layers);
            let to = d.zone(buzz_ui::PanelId::Properties).center();
            let mods = egui::Modifiers::default();
            d.frame(vec![egui::Event::PointerMoved(from)]);
            d.frame(vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: mods,
            }]);
            d.frame(vec![egui::Event::PointerMoved(to)]);
            assert!(!egui::DragAndDrop::has_payload_of_type::<DraggedPanels>(
                &d.ctx
            ));
            d.frame(vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: mods,
            }]);
            d.frame(vec![]);
            assert_eq!(d.app.editor.workspace.slots, before.slots);
        }

        /// A floating panel can be dragged back onto a docked section and
        /// becomes a tab of it — the whole point of floating windows carrying
        /// draggable tabs.
        #[test]
        fn dragging_a_floating_panel_to_dock_into_another_section() {
            let mut d = Drive::new();

            // Float the Layers panel first.
            d.app
                .editor
                .workspace
                .float_at(buzz_ui::PanelId::Layers, (500.0, 300.0));
            // Several frames to let the floating window render and register
            // its drop zones and header-drag rects.
            for _ in 0..4 {
                d.frame(vec![]);
            }

            // The floating window's header or tab should be stored at the
            // header-drag key. Find the drag handle for the floating panel.
            let from = d
                .ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        "header-drag",
                        buzz_ui::PanelId::Layers,
                    )))
                })
                .or_else(|| {
                    d.ctx.data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new((
                            "grip",
                            buzz_ui::PanelId::Layers,
                        )))
                    })
                })
                .expect("floating Layers has a drag handle")
                .center();

            let to = d.zone(buzz_ui::PanelId::Properties).center();
            d.drag(from, to);

            let section = d
                .app
                .editor
                .workspace
                .section_of(buzz_ui::PanelId::Layers)
                .expect("Layers is still on screen");
            assert!(
                section.panels.contains(&buzz_ui::PanelId::Properties),
                "Layers did not tab into Properties after dragging from float: {:?}",
                section.panels
            );
            assert_ne!(
                d.app.editor.workspace.dock_of(buzz_ui::PanelId::Layers),
                buzz_ui::Dock::Float,
                "Layers should no longer be floating"
            );
        }

        /// Placing the pointer just outside a section (within the 28-px
        /// proximity margin) still selects it as the drop target, and the
        /// target carries a `highlight_rect` for the border glow.
        #[test]
        fn proximity_border_highlight_when_close_to_section() {
            let mut d = Drive::new();
            // Need a drag in progress for drop_target to be meaningful.
            // Color, not Library: Library now tabs inside the same section
            // as Properties, so it has no grip of its own to start from.
            let from = d.grip(buzz_ui::PanelId::Color);
            let target_rect = d.zone(buzz_ui::PanelId::Properties);

            // A point 10 px to the left of the Properties section — outside
            // its rect but well within the 28-px proximity margin.
            let near = egui::pos2(target_rect.left() - 10.0, target_rect.center().y);

            // Start the drag (sets the payload) but do not release.
            let mods = egui::Modifiers::default();
            d.frame(vec![egui::Event::PointerMoved(from)]);
            d.frame(vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: mods,
            }]);
            for step in 1..=6 {
                let t = step as f32 / 6.0;
                d.frame(vec![egui::Event::PointerMoved(from + (near - from) * t)]);
            }

            // Ask drop_target where this pointer would land.
            let dragged = vec![buzz_ui::PanelId::Color];
            let target = d.app.drop_target(near, &dragged);
            assert!(
                target.is_some(),
                "the pointer near a section must select a target"
            );
            match target.unwrap() {
                DropTarget::Section { front, .. } => {
                    assert_eq!(
                        front,
                        buzz_ui::PanelId::Properties,
                        "proximity should target the nearby Properties section"
                    );
                }
                DropTarget::Column { highlight_rect, .. } => {
                    assert!(
                        highlight_rect.is_some(),
                        "a Column target near a section must carry a highlight_rect"
                    );
                }
                DropTarget::Float(_) => {
                    panic!("a pointer 10 px from a section should not be Float");
                }
            }
        }

        /// **Grip-dragging the inspector onto Color tabs the whole section
        /// in.** All eight panels end up sharing one section, on Color's own
        /// dock, with Properties still in front. Library used to sit apart
        /// from the inspector and stood in for "another section" here; now
        /// it tabs alongside Properties already, so Color — still its own
        /// section — is the one left to drag onto.
        #[test]
        fn grip_dragging_the_inspector_onto_the_library_tabs_it_there() {
            let mut d = Drive::new();
            let from = d.grip(buzz_ui::PanelId::Properties);
            let to = d.zone(buzz_ui::PanelId::Color).center();

            d.drag(from, to);

            let workspace = &d.app.editor.workspace;
            assert_eq!(
                workspace.dock_of(buzz_ui::PanelId::Properties),
                buzz_ui::Dock::Right
            );
            let section = workspace
                .section_of(buzz_ui::PanelId::Properties)
                .expect("still on screen");
            assert_eq!(section.front, buzz_ui::PanelId::Properties);
            for id in [
                buzz_ui::PanelId::Library,
                buzz_ui::PanelId::Assets,
                buzz_ui::PanelId::Cast,
                buzz_ui::PanelId::Properties,
                buzz_ui::PanelId::Filters,
                buzz_ui::PanelId::ToolOptions,
                buzz_ui::PanelId::Color,
                buzz_ui::PanelId::Swatches,
            ] {
                assert!(
                    section.panels.contains(&id),
                    "{id:?} missing from the merged section: {:?}",
                    section.panels
                );
            }
            assert_eq!(section.panels.len(), 8);
        }

        /// **Grip-dragging the inspector onto the stage floats one window**,
        /// not six — the whole section, not just the panel that was gripped.
        /// Six, not three: the inspector's one tab stack now holds
        /// Properties, Filters and Tool Options alongside Library, Assets
        /// and Cast.
        #[test]
        fn grip_dragging_the_inspector_onto_the_stage_floats_one_window() {
            let mut d = Drive::new();
            let from = d.grip(buzz_ui::PanelId::Properties);
            let to = egui::pos2(700.0, 450.0);

            d.drag(from, to);

            let workspace = &d.app.editor.workspace;
            assert_eq!(
                workspace.dock_of(buzz_ui::PanelId::Properties),
                buzz_ui::Dock::Float
            );
            let section = workspace
                .section_of(buzz_ui::PanelId::Properties)
                .expect("still on screen");
            assert_eq!(
                section.panels.len(),
                6,
                "the whole inspector should have floated together: {:?}",
                section.panels
            );
            assert_eq!(section.front, buzz_ui::PanelId::Properties);
            for id in [
                buzz_ui::PanelId::Properties,
                buzz_ui::PanelId::Filters,
                buzz_ui::PanelId::ToolOptions,
                buzz_ui::PanelId::Library,
                buzz_ui::PanelId::Assets,
                buzz_ui::PanelId::Cast,
            ] {
                assert_eq!(workspace.slot(id).unwrap().float_pos, (700.0, 450.0));
            }
        }

        /// **"Group With" reaches across docks.** The inspector's own section
        /// used to offer only its Right-column neighbours; `all_fronts` spans
        /// every dock, so Tools on the left column is offered too, labelled
        /// with where it lives. Library used to stand in for this — a
        /// section apart from the inspector's — but it now tabs alongside
        /// Properties, so it is no longer "elsewhere" to offer.
        #[test]
        fn group_with_menu_offers_the_library() {
            let workspace = buzz_ui::Workspace::animate();
            let all_fronts: Vec<buzz_ui::PanelId> = {
                let mut docks = buzz_ui::Dock::COLUMNS.to_vec();
                docks.push(buzz_ui::Dock::Float);
                docks
                    .into_iter()
                    .flat_map(|d| workspace.sections(d))
                    .map(|s| s.front)
                    .collect()
            };
            let section = workspace
                .section_of(buzz_ui::PanelId::Properties)
                .expect("on screen");
            let offered: Vec<buzz_ui::PanelId> = all_fronts
                .iter()
                .copied()
                .filter(|id| !section.panels.contains(id))
                .collect();
            assert!(
                offered.contains(&buzz_ui::PanelId::Tools),
                "Group With should reach Tools on the left column: {offered:?}"
            );
            assert_eq!(
                dock_paren_label(workspace.dock_of(buzz_ui::PanelId::Tools)),
                "Left"
            );
        }
    }

    /// **The reported bug, reproduced through the real input path.** egui
    /// does not call `Editor::pointer_down_at` at the pixel the mouse
    /// actually went down at — only once a drag has moved far enough for
    /// `Response::dragged()` to turn true (`egui`'s own click/drag
    /// disambiguation, `max_click_dist`, 6 points), and at that point it
    /// hands over wherever the pointer *now* is (`PointerState::
    /// interact_pos`), not where the press began (`PointerState::
    /// press_origin`). A corner grab — `TRANSFORM_GRAB_PX` is only 8 screen
    /// px — starting a shrink drifts inward past its own handle before the
    /// editor ever hears about the press, and the gesture `ToolMachine`
    /// builds from that drifted point is a move, not a scale: exactly the
    /// user's own words, "once it resizes once, it just moves".
    mod free_transform_dispatch_tests {
        use super::*;

        struct StageDrive {
            app: App,
            ctx: egui::Context,
            screen: egui::Rect,
        }

        impl StageDrive {
            fn new() -> Self {
                let mut app = App::new(GpuPreference::Automatic);
                app.recovery.found.clear();
                let ctx = egui::Context::default();
                buzz_ui::theme::apply(&ctx);
                let screen =
                    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1400.0));
                let mut drive = Self { app, ctx, screen };
                // Settle the layout before anything is measured against it.
                drive.frame(vec![]);
                drive.frame(vec![]);
                drive
            }

            fn frame(&mut self, events: Vec<egui::Event>) {
                let input = egui::RawInput {
                    events,
                    screen_rect: Some(self.screen),
                    ..Default::default()
                };
                let app = &mut self.app;
                let _ = self.ctx.run_ui(input, |ui| {
                    app.build_ui(ui);
                });
            }

            /// Where a document point is on screen right now — the same
            /// conversion `handle_stage_input`'s own `local` closure inverts.
            fn at(&self, doc: buzz_geom::Point) -> egui::Pos2 {
                let screen = self.app.editor.camera.doc_to_screen(doc);
                egui::pos2(
                    self.app.stage_area_min.x + screen.x as f32,
                    self.app.stage_area_min.y + screen.y as f32,
                )
            }
        }

        /// A 100x100 square, seeded directly on the document's default
        /// layer.
        fn seed_square(d: &mut StageDrive) -> buzz_scene::ObjectId {
            use buzz_geom::Shape as _;
            let mut id = buzz_scene::ObjectId(90_000);
            d.app.editor.doc.edit("Seed", |scene| {
                let layer = scene.layers().iter().next().expect("a default layer").id;
                id = scene.next_object_id();
                let shape = buzz_scene::Object::shape(
                    id,
                    buzz_scene::ShapeData::filled(
                        buzz_geom::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9),
                        Color::WHITE,
                    ),
                );
                scene.edit_layers().update(layer, |l| {
                    l.frames.set_objects(0, vec![Arc::new(shape)]);
                });
            });
            id
        }

        /// A corner drag that egui only reports to the editor once its own
        /// click/drag threshold is crossed must still scale the selection —
        /// not translate it.
        #[test]
        fn a_corner_drag_across_eguis_click_threshold_still_resizes() {
            let mut d = StageDrive::new();
            let id = seed_square(&mut d);
            d.app.editor.selection.select_one(id);
            d.app.editor.set_tool(ToolId::FreeTransform);
            d.app.editor.camera.set_zoom_percent(100.0);
            d.app.editor.camera.center = Point::new(50.0, 50.0);
            d.frame(vec![]);

            let bounds0 = d
                .app
                .editor
                .selection_bounds_drawn()
                .expect("a selection box");
            let transform_before = d
                .app
                .editor
                .scene()
                .find_object(id)
                .expect("still there")
                .1
                .transform;

            let corner_doc = Point::new(bounds0.x1, bounds0.y1);
            let corner_pos = d.at(corner_doc);
            let mods = egui::Modifiers::default();

            d.frame(vec![egui::Event::PointerMoved(corner_pos)]);
            d.frame(vec![egui::Event::PointerButton {
                pos: corner_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: mods,
            }]);
            // Below egui's own click/drag threshold (`max_click_dist`, 6
            // points): still a click in egui's eyes, so nothing has reached
            // the editor yet.
            d.frame(vec![egui::Event::PointerMoved(
                corner_pos + egui::vec2(2.0, 2.0),
            )]);
            // Past it, inward — a shrink. This is the first move egui calls
            // a drag, and it hands the editor *this* position, not the
            // corner the finger actually landed on.
            let shrink_to = corner_pos + egui::vec2(-15.0, -15.0);
            d.frame(vec![egui::Event::PointerMoved(shrink_to)]);
            // Carry it further in, as a real shrink drag would.
            let shrink_more = corner_pos + egui::vec2(-30.0, -30.0);
            for step in 1..=4 {
                let t = step as f32 / 4.0;
                d.frame(vec![egui::Event::PointerMoved(
                    shrink_to + (shrink_more - shrink_to) * t,
                )]);
            }
            d.frame(vec![egui::Event::PointerButton {
                pos: shrink_more,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: mods,
            }]);
            d.frame(vec![]);

            let transform_after = d
                .app
                .editor
                .scene()
                .find_object(id)
                .expect("still there")
                .1
                .transform;
            assert_ne!(
                transform_after, transform_before,
                "the drag should have changed the artwork"
            );
            let scale_after = {
                let c = transform_after.as_coeffs();
                (c[0] * c[0] + c[1] * c[1]).sqrt()
            };
            assert!(
                scale_after < 0.98,
                "a corner drag toward the pivot should have shrunk the artwork, \
                 got scale {scale_after} (transform {transform_after:?})"
            );
            let translation = {
                let c = transform_after.as_coeffs();
                (c[4] * c[4] + c[5] * c[5]).sqrt()
            };
            assert!(
                translation < 40.0,
                "the far (anchor) corner should stay roughly put, not have the \
                 whole shape translate: moved by {translation}"
            );
        }
    }

    /// **I4b: Space-drag, the middle mouse button and the Hand tool track
    /// the pointer 1:1** — through the real window path (`App` + `build_ui`,
    /// egui's own click/drag threshold and event dispatch), at every zoom
    /// and display scale, whether the pointer reports several moves a frame
    /// or one. See `docs/INTERACTION_FIXES_PLAN.md` I4b.
    mod pan_ratio_tests {
        use super::*;

        struct PanDrive {
            app: App,
            ctx: egui::Context,
            screen: egui::Rect,
            native_ppp: f32,
        }

        impl PanDrive {
            fn new(native_ppp: f32) -> Self {
                let mut app = App::new(GpuPreference::Automatic);
                app.recovery.found.clear();
                let ctx = egui::Context::default();
                buzz_ui::theme::apply(&ctx);
                let screen =
                    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1400.0));
                let mut drive = Self { app, ctx, screen, native_ppp };
                // Settle the layout before anything is measured against it.
                drive.frame(vec![]);
                drive.frame(vec![]);
                drive
            }

            fn frame(&mut self, events: Vec<egui::Event>) {
                let mut input = egui::RawInput {
                    events,
                    screen_rect: Some(self.screen),
                    ..Default::default()
                };
                if let Some(vp) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
                    vp.native_pixels_per_point = Some(self.native_ppp);
                }
                let app = &mut self.app;
                let _ = self.ctx.run_ui(input, |ui| {
                    app.build_ui(ui);
                });
            }

            fn key(&mut self, key: egui::Key, pressed: bool) {
                self.frame(vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }]);
            }

            fn button(&mut self, pos: egui::Pos2, button: egui::PointerButton, pressed: bool) {
                self.frame(vec![egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                }]);
            }

            fn moves(&mut self, positions: &[egui::Pos2]) {
                let events = positions
                    .iter()
                    .map(|p| egui::Event::PointerMoved(*p))
                    .collect();
                self.frame(events);
            }

            /// A stage-local point (as `handle_stage_input`'s own `local`
            /// closure produces) turned back into an absolute window one.
            fn to_window(&self, local: Point) -> egui::Pos2 {
                egui::pos2(
                    self.app.stage_area_min.x + local.x as f32,
                    self.app.stage_area_min.y + local.y as f32,
                )
            }
        }

        /// How far the view actually moved a document point, versus how far
        /// the pointer was carried — 1.0 is a perfect hand-drag.
        struct GestureResult {
            ratio: f64,
            /// Drift off the drag's own axis, in points; should be ~0.
            perpendicular: f64,
        }

        /// Drags from the centre of the stage by `total_dx` screen points
        /// along X, through egui's real click/drag threshold, and reports
        /// how far the view actually followed.
        ///
        /// The first three positions (`+3`, `+5`, `+8` points from the
        /// press) each land in their own frame, so egui's own
        /// click/drag disambiguation (`max_click_dist`, 6 points) falls
        /// exactly where a real slow-starting drag would put it: two
        /// frames that still read as a plain click, then the frame that
        /// crosses the threshold. An implementation that only reads a
        /// per-frame pointer delta on frames already recognised as a drag
        /// drops the motion from the first two. The rest of the distance is
        /// grouped `moves_per_frame` positions to a frame, as asked.
        fn measure_pan(
            zoom_pct: f64,
            native_ppp: f32,
            total_dx: f64,
            moves_per_frame: usize,
            hand_tool: bool,
        ) -> GestureResult {
            let mut drive = PanDrive::new(native_ppp);
            if hand_tool {
                drive.app.editor.set_tool(ToolId::Hand);
            }
            drive.app.editor.camera.set_zoom_percent(zoom_pct);
            drive.app.editor.camera.center = Point::new(0.0, 0.0);
            drive.frame(vec![]);

            let vp = drive.app.editor.camera.viewport;
            let start_local = Point::new(vp.width / 2.0, vp.height / 2.0);
            let start = drive.to_window(start_local);
            let doc_under_cursor = drive.app.editor.camera.screen_to_doc(start_local);

            if !hand_tool {
                drive.key(egui::Key::Space, true);
            }
            drive.button(start, egui::PointerButton::Primary, true);

            let mut offsets = vec![3.0_f64, 5.0, 8.0];
            let mut d = 8.0_f64;
            while d < total_dx {
                d = (d + 12.0).min(total_dx);
                offsets.push(d);
            }
            if *offsets.last().unwrap() < total_dx {
                offsets.push(total_dx);
            }

            let split = 3.min(offsets.len());
            let (pre, bulk) = offsets.split_at(split);
            for &dx in pre {
                drive.moves(&[egui::pos2(start.x + dx as f32, start.y)]);
            }
            let mut i = 0;
            while i < bulk.len() {
                let end = (i + moves_per_frame).min(bulk.len());
                let events: Vec<egui::Pos2> = bulk[i..end]
                    .iter()
                    .map(|dx| egui::pos2(start.x + *dx as f32, start.y))
                    .collect();
                drive.moves(&events);
                i = end;
            }

            let final_pos = egui::pos2(start.x + total_dx as f32, start.y);
            drive.button(final_pos, egui::PointerButton::Primary, false);
            if !hand_tool {
                drive.key(egui::Key::Space, false);
            }
            drive.frame(vec![]);

            let now_screen = drive.app.editor.camera.doc_to_screen(doc_under_cursor);
            let moved = now_screen - start_local;
            GestureResult {
                ratio: moved.x / total_dx,
                perpendicular: moved.y,
            }
        }

        fn assert_1_to_1(result: GestureResult, label: &str) {
            assert!(
                (result.ratio - 1.0).abs() < 0.01,
                "{label}: expected a 1:1 pan (\u{b1}1%), got ratio {:.4}",
                result.ratio
            );
            assert!(
                result.perpendicular.abs() < 1.0,
                "{label}: the pan drifted {:.3} points off its own axis",
                result.perpendicular
            );
        }

        #[test]
        fn hand_tool_pans_1_to_1_at_100pct_zoom_one_move_per_frame() {
            assert_1_to_1(
                measure_pan(100.0, 1.0, 300.0, 1, true),
                "Hand tool, 100% zoom, 1.0 ppp, one move/frame",
            );
        }

        #[test]
        fn hand_tool_pans_1_to_1_at_300pct_zoom_150pct_display_several_moves_per_frame() {
            assert_1_to_1(
                measure_pan(300.0, 1.5, 300.0, 5, true),
                "Hand tool, 300% zoom, 1.5 ppp, several moves/frame",
            );
        }

        #[test]
        fn space_drag_pans_1_to_1_at_100pct_zoom_one_move_per_frame() {
            assert_1_to_1(
                measure_pan(100.0, 1.0, 300.0, 1, false),
                "Space-drag, 100% zoom, 1.0 ppp, one move/frame",
            );
        }

        #[test]
        fn space_drag_pans_1_to_1_at_300pct_zoom_150pct_display_several_moves_per_frame() {
            assert_1_to_1(
                measure_pan(300.0, 1.5, 300.0, 5, false),
                "Space-drag, 300% zoom, 1.5 ppp, several moves/frame",
            );
        }

        /// The middle mouse button is the other half of `pan_override`, and
        /// shares its code path with Space — a quick check that it was
        /// fixed along with it rather than assumed.
        #[test]
        fn middle_button_drag_pans_1_to_1() {
            let mut drive = PanDrive::new(1.0);
            drive.app.editor.camera.set_zoom_percent(100.0);
            drive.app.editor.camera.center = Point::new(0.0, 0.0);
            drive.frame(vec![]);
            let vp = drive.app.editor.camera.viewport;
            let start_local = Point::new(vp.width / 2.0, vp.height / 2.0);
            let start = drive.to_window(start_local);
            let doc_under_cursor = drive.app.editor.camera.screen_to_doc(start_local);

            drive.button(start, egui::PointerButton::Middle, true);
            drive.moves(&[egui::pos2(start.x + 3.0, start.y)]);
            drive.moves(&[egui::pos2(start.x + 5.0, start.y)]);
            drive.moves(&[egui::pos2(start.x + 8.0, start.y)]);
            drive.moves(&[egui::pos2(start.x + 150.0, start.y)]);
            let end = egui::pos2(start.x + 300.0, start.y);
            drive.moves(&[end]);
            drive.button(end, egui::PointerButton::Middle, false);
            drive.frame(vec![]);

            let now_screen = drive.app.editor.camera.doc_to_screen(doc_under_cursor);
            let moved = now_screen - start_local;
            assert!(
                (moved.x / 300.0 - 1.0).abs() < 0.01,
                "middle-button pan should track the pointer 1:1, moved {moved:?}"
            );
        }

        /// **The pan must not stop when the pointer strays outside the stage
        /// rect mid-drag** — over a docked panel, or off the window
        /// entirely. `response.dragged()` stays true for as long as the
        /// button that started the drag is down, wherever the pointer goes;
        /// this pins that down against a regression.
        #[test]
        fn space_drag_continues_when_the_pointer_leaves_the_stage_rect() {
            let mut drive = PanDrive::new(1.0);
            drive.app.editor.camera.set_zoom_percent(100.0);
            drive.app.editor.camera.center = Point::new(0.0, 0.0);
            drive.frame(vec![]);
            let vp = drive.app.editor.camera.viewport;
            let start_local = Point::new(vp.width / 2.0, vp.height / 2.0);
            let start = drive.to_window(start_local);
            let doc_under_cursor = drive.app.editor.camera.screen_to_doc(start_local);

            drive.key(egui::Key::Space, true);
            drive.button(start, egui::PointerButton::Primary, true);
            drive.moves(&[egui::pos2(start.x + 8.0, start.y)]);
            // Far outside the stage rect altogether — above the window,
            // past any dock.
            let outside = egui::pos2(start.x + 150.0, -500.0);
            drive.moves(&[outside]);
            drive.moves(&[egui::pos2(outside.x + 50.0, outside.y + 50.0)]);
            let end = egui::pos2(start.x + 300.0, start.y);
            drive.moves(&[end]);
            drive.button(end, egui::PointerButton::Primary, false);
            drive.key(egui::Key::Space, false);
            drive.frame(vec![]);

            let now_screen = drive.app.editor.camera.doc_to_screen(doc_under_cursor);
            let moved = now_screen - start_local;
            assert!(
                (moved.x - 300.0).abs() < 3.0,
                "panning should have kept tracking the pointer even after it \
                 left the stage rect, moved {moved:?}"
            );
        }

        /// **Tool Options' Pan speed multiplies the Hand tool's own drag**,
        /// and does so without touching the wheel (`ToolAction::PanView`
        /// from the scroll handler is built at its own call site, which this
        /// multiplier never runs over).
        #[test]
        fn pan_speed_option_scales_the_hand_tools_drag() {
            let mut drive = PanDrive::new(1.0);
            drive.app.editor.set_tool(ToolId::Hand);
            drive.app.editor.camera.set_zoom_percent(100.0);
            drive.app.editor.camera.center = Point::new(0.0, 0.0);
            drive.app.editor.workspace.pan_speed = 2.0;
            drive.frame(vec![]);
            let vp = drive.app.editor.camera.viewport;
            let start_local = Point::new(vp.width / 2.0, vp.height / 2.0);
            let start = drive.to_window(start_local);
            let doc_under_cursor = drive.app.editor.camera.screen_to_doc(start_local);

            drive.button(start, egui::PointerButton::Primary, true);
            drive.moves(&[egui::pos2(start.x + 8.0, start.y)]);
            let end = egui::pos2(start.x + 108.0, start.y);
            drive.moves(&[end]);
            drive.button(end, egui::PointerButton::Primary, false);
            drive.frame(vec![]);

            let now_screen = drive.app.editor.camera.doc_to_screen(doc_under_cursor);
            let moved = now_screen - start_local;
            // 108 points of pointer travel at 2x should move the view ~216.
            assert!(
                (moved.x - 216.0).abs() < 4.0,
                "pan speed 2x should double the Hand tool's pan, moved {moved:?}"
            );
        }

        /// The same multiplier, applied to a Space-drag instead of the Hand
        /// tool — the other call site the fix touches.
        #[test]
        fn pan_speed_option_scales_a_space_drag_too() {
            let mut drive = PanDrive::new(1.0);
            drive.app.editor.camera.set_zoom_percent(100.0);
            drive.app.editor.camera.center = Point::new(0.0, 0.0);
            drive.app.editor.workspace.pan_speed = 1.5;
            drive.frame(vec![]);
            let vp = drive.app.editor.camera.viewport;
            let start_local = Point::new(vp.width / 2.0, vp.height / 2.0);
            let start = drive.to_window(start_local);
            let doc_under_cursor = drive.app.editor.camera.screen_to_doc(start_local);

            drive.key(egui::Key::Space, true);
            drive.button(start, egui::PointerButton::Primary, true);
            drive.moves(&[egui::pos2(start.x + 8.0, start.y)]);
            let end = egui::pos2(start.x + 108.0, start.y);
            drive.moves(&[end]);
            drive.button(end, egui::PointerButton::Primary, false);
            drive.key(egui::Key::Space, false);
            drive.frame(vec![]);

            let now_screen = drive.app.editor.camera.doc_to_screen(doc_under_cursor);
            let moved = now_screen - start_local;
            // 108 points of pointer travel at 1.5x should move the view ~162.
            assert!(
                (moved.x - 162.0).abs() < 4.0,
                "pan speed 1.5x should scale a Space-drag too, moved {moved:?}"
            );
        }
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
        app.editor
            .workspace
            .move_to(buzz_ui::PanelId::Depth, buzz_ui::Dock::Hidden);
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

    /// H2: the timeline's "Highlight" menu, and "Remove Highlight", both
    /// travel through `apply_timeline` into one `doc.edit`, labelled so the
    /// History panel says what happened rather than "Layer Properties".
    #[test]
    fn a_layers_highlight_is_set_and_cleared_through_the_response_path() {
        let mut app = App::new(GpuPreference::Automatic);
        let layer = app.editor.doc.scene().layers().iter().next().unwrap().id;
        let mut commands = Vec::new();

        let color = peniko::Color::from_rgb8(0xC0, 0x50, 0x50);
        app.apply_timeline(
            buzz_ui::TimelineResponse {
                set_highlight: Some((vec![layer], Some(color))),
                ..Default::default()
            },
            &mut commands,
        );
        assert_eq!(
            app.editor.doc.scene().layers().get(layer).unwrap().highlight,
            Some(color)
        );
        assert_eq!(app.editor.doc.history().next_undo_label(), Some("Highlight Layer"));

        app.apply_timeline(
            buzz_ui::TimelineResponse {
                set_highlight: Some((vec![layer], None)),
                ..Default::default()
            },
            &mut commands,
        );
        assert_eq!(
            app.editor.doc.scene().layers().get(layer).unwrap().highlight,
            None,
            "Remove Highlight clears it"
        );
        assert_eq!(app.editor.doc.history().next_undo_label(), Some("Highlight Layer"));
    }

    /// I2: the disclosure triangle travels through `apply_timeline` into one
    /// undoable `doc.edit`, so opening and closing a folder is a thing the
    /// undo history can say and step back through like anything else.
    #[test]
    fn a_folders_collapsed_state_toggles_through_the_response_path() {
        let mut app = App::new(GpuPreference::Automatic);
        let mut folder = None;
        app.editor.doc.edit("New Folder", |scene| {
            folder = Some(scene.add_layer("Folder", buzz_scene::LayerKind::Folder));
        });
        let folder = folder.unwrap();
        let mut commands = Vec::new();

        assert!(!app.editor.doc.scene().layers().get(folder).unwrap().collapsed);

        app.apply_timeline(
            buzz_ui::TimelineResponse {
                toggle_collapsed: Some(folder),
                ..Default::default()
            },
            &mut commands,
        );
        assert!(
            app.editor.doc.scene().layers().get(folder).unwrap().collapsed,
            "the disclosure triangle collapses the folder"
        );
        assert_eq!(
            app.editor.doc.history().next_undo_label(),
            Some("Expand/Collapse Folder")
        );

        app.apply_timeline(
            buzz_ui::TimelineResponse {
                toggle_collapsed: Some(folder),
                ..Default::default()
            },
            &mut commands,
        );
        assert!(
            !app.editor.doc.scene().layers().get(folder).unwrap().collapsed,
            "clicking it again expands the folder"
        );
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

    /// `RENDER_FOLLOWUPS_PLAN.md` item 2: once a frame is already drawn in
    /// bands, the next one must not also run the whole-frame
    /// `stage::build_scene` ladder — the band encode is what actually draws
    /// it, and running the ladder too would encode the frame twice for an
    /// `active.vello` nothing downstream reads. `App::render` needs a live
    /// GPU window and cannot be stepped here, so this steps the two frames'
    /// worth of state through the exact decision it makes at its one call
    /// site: `wants_whole_frame_ladder(reuse, banding_already)`.
    #[test]
    fn a_second_banded_frame_skips_the_whole_frame_ladder() {
        // Frame 1: nothing banding yet — `self.stage_bands` starts at 0 for
        // every session. Even a frame that turns out to be over budget still
        // runs the ladder this once: that is what discovers it and picks a
        // band count.
        assert!(
            wants_whole_frame_ladder(false, false),
            "an ordinary, or first over-budget, frame must run the ladder"
        );

        // Frame 2: `self.stage_bands` came back non-zero from frame 1, so
        // this frame starts already banding.
        assert!(
            !wants_whole_frame_ladder(false, true),
            "a frame already banding must not run the ladder too"
        );

        // A retained encoding is never reused while banding either way (see
        // `reuse`'s own `stage_bands == 0` term) — both must still refuse
        // the ladder.
        assert!(!wants_whole_frame_ladder(true, false));
        assert!(!wants_whole_frame_ladder(true, true));
    }

    // ---- S3 (`docs/BIMARI5_LAG_PLAN.md`): band mode must not trap the stage ----

    /// A square stage tiled with `instances` of one heavy symbol — the same
    /// fixture `tests/stage_band_budget.rs` calibrates: about 15,000 lines per
    /// instance, so 170 of them overrun the 1.8M line budget at scale 1 but
    /// fit at scale 2.
    fn dense_symbol_scene(instances: usize) -> Editor {
        use buzz_geom::{Affine, BezPath, PathEl, Point};
        use buzz_scene::{LayerKind, ShapeData, SymbolKind};

        let mut scene = buzz_scene::Scene::default();
        scene.stage_mut().background = Color::WHITE;
        scene.stage_mut().size = Size::new(1200.0, 1200.0);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let filler = scene.add_symbol("Filler", SymbolKind::Graphic, None);
        scene.library_mut().update(filler, |symbol| {
            let Some(inner) = symbol.layers.iter().next().map(|l| l.id) else {
                return;
            };
            let mut path = BezPath::new();
            for s in 0..3000 {
                let t = f64::from(s) / 3000.0 * std::f64::consts::TAU;
                let p = Point::new(25.0 * t.cos(), 25.0 * t.sin());
                path.push(if s == 0 { PathEl::MoveTo(p) } else { PathEl::LineTo(p) });
            }
            path.push(PathEl::ClosePath);
            symbol.layers.update(inner, |l| {
                l.frames.push_object(
                    0,
                    std::sync::Arc::new(buzz_scene::Object::shape(
                        buzz_scene::ObjectId(9001),
                        ShapeData::filled(path, Color::from_rgb8(0x20, 0x40, 0x80)),
                    )),
                );
            });
        });
        let across = (instances as f64).sqrt().ceil() as usize;
        let step = 1200.0 / across as f64;
        for i in 0..instances {
            let cx = (i % across) as f64 * step + step * 0.5;
            let cy = (i / across) as f64 * step + step * 0.5;
            scene.add_instance_at(layer, 0, filler, Affine::translate((cx, cy)));
        }
        let mut editor = Editor::new(Document::new(scene));
        editor.camera.viewport = Size::new(1200.0, 1200.0);
        editor.camera.center = Point::new(600.0, 600.0);
        editor.camera.zoom = 1.0;
        editor
    }

    /// `App::render`'s stage decisions, stepped without a window: the same
    /// three pure decisions (`band_recheck_due`, `wants_whole_frame_ladder`,
    /// the ladder's own verdict) on a real `DrawCache` and a real encode.
    struct BandSim {
        cache: buzz_render::document::DrawCache,
        stage_bands: u32,
        since_ladder: u32,
        last_frame: u32,
        ladder_runs: u32,
        planned: Vec<PlannedBand>,
    }

    impl BandSim {
        fn new(scale: u32, max_scale: u32, stage_bands: u32) -> Self {
            let mut cache = buzz_render::document::DrawCache::new();
            cache.set_scale(scale);
            cache.set_max_scale(max_scale);
            Self {
                cache,
                stage_bands,
                since_ladder: 0,
                last_frame: 0,
                ladder_runs: 0,
                planned: Vec::new(),
            }
        }

        fn frame(&mut self, editor: &Editor, frame: u32) {
            let area = buzz_geom::Rect::new(0.0, 0.0, 1200.0, 1200.0);
            let banding_already = self.stage_bands > 0
                && !band_recheck_due(
                    self.cache.scale(),
                    self.cache.max_scale(),
                    self.since_ladder,
                    frame,
                    self.last_frame,
                );
            if wants_whole_frame_ladder(false, banding_already) {
                let mut scene = VelloScene::new();
                stage::build_scene(&mut scene, editor, area, 1.0, &mut self.cache);
                self.since_ladder = 0;
                self.last_frame = frame;
                self.ladder_runs += 1;
                self.stage_bands = if self.cache.over_budget() {
                    self.cache.bands_needed()
                } else {
                    0
                };
            }
            if self.stage_bands > 0 {
                if banding_already {
                    self.since_ladder += 1;
                    self.last_frame = frame;
                }
                let ceiling = self.cache.segment_ceiling(1200.0 * 1200.0);
                self.planned = plan_stage_bands(
                    editor,
                    &mut self.cache,
                    area,
                    1.0,
                    ceiling,
                    1200,
                    1200,
                    self.stage_bands,
                );
                self.stage_bands = (self.planned.len() as u32).clamp(1, 16);
            }
        }
    }

    /// **Root cause 1.** Opening a document or switching tabs replaces the
    /// `DrawCache`; the replacement used to come back at the default
    /// `max_scale` of 1, so `reconsider` could never grow again and every
    /// heavy frame went to bands for good.
    #[test]
    fn opening_a_document_keeps_the_buffer_scale_limits() {
        let mut app = App::new(GpuPreference::Automatic);
        app.lights.set_scale(2);
        app.lights.set_max_scale(4);
        app.stage_bands = 16;
        app.banded_on_target = Some((10, 10, Color::WHITE));

        let (_old, _view) = app.activate_document(
            Document::new(buzz_scene::Scene::default()),
            Default::default(),
        );

        assert_eq!(app.lights.max_scale(), 4, "the ceiling must survive the swap");
        assert_eq!(app.lights.scale(), 2, "so must the live scale");
        assert_eq!(app.stage_bands, 0, "a new document starts unbanded");
        assert!(app.banded_on_target.is_none(), "and holds no banded picture");
    }

    /// Over budget at scale 1, fits at 2: a frame that *started in bands*
    /// leaves them on the first ladder run, within 2 frames.
    #[test]
    fn a_scene_that_fits_after_growing_leaves_band_mode_within_two_frames() {
        let editor = dense_symbol_scene(170);
        let mut sim = BandSim::new(1, 4, 16);
        sim.frame(&editor, 0);
        sim.frame(&editor, 0);
        assert_eq!(sim.stage_bands, 0, "single pass once the scale has grown");
        assert!(sim.cache.scale() >= 2, "it must have grown, got {}", sim.cache.scale());
        assert!(!sim.cache.over_budget());
    }

    /// Already at the ceiling and banded with a frame that now fits: bands
    /// hold for 30 unchanged frames, then one ladder run lets it out; a jump
    /// of more than one frame lets it out at once.
    #[test]
    fn a_banded_frame_that_now_fits_leaves_on_the_recheck() {
        let editor = dense_symbol_scene(60);
        let mut sim = BandSim::new(1, 1, 4);
        sim.last_frame = 5;
        for _ in 0..BAND_LADDER_RECHECK_FRAMES {
            sim.frame(&editor, 5);
            assert!(sim.stage_bands > 0, "held in bands between re-checks");
        }
        assert_eq!(sim.ladder_runs, 0, "no whole-frame encode between re-checks");
        sim.frame(&editor, 5);
        assert_eq!(sim.ladder_runs, 1);
        assert_eq!(sim.stage_bands, 0, "the ladder found the whole frame fits");

        let mut sim = BandSim::new(1, 1, 4);
        sim.last_frame = 5;
        sim.frame(&editor, 5);
        assert_eq!(sim.ladder_runs, 0);
        sim.frame(&editor, 9);
        assert_eq!(sim.ladder_runs, 1, "a scrub re-runs the ladder immediately");
        assert_eq!(sim.stage_bands, 0);
    }

    /// Over budget even at the largest scale: it grows, then stays banded,
    /// and no band it draws is over either budget (the H0 guarantee).
    #[test]
    fn a_scene_over_budget_at_the_largest_scale_stays_banded_with_no_blank_bands() {
        let editor = dense_symbol_scene(1500);
        let mut sim = BandSim::new(1, 8, 0);
        for _ in 0..3 {
            sim.frame(&editor, 0);
        }
        assert_eq!(sim.cache.scale(), 8, "grown all the way first");
        assert!(sim.stage_bands > 0, "still banded at the ceiling");
        assert!(!sim.planned.is_empty());
        let ceiling = sim.cache.segment_ceiling(1200.0 * 1200.0);
        for band in &sim.planned {
            assert!(band.lines <= sim.cache.line_budget(), "blank band (lines): {}", band.lines);
            assert!(band.segments <= ceiling, "blank band (segments): {}", band.segments);
        }
    }

    /// Band mode with nothing changing asks for nothing: the loop sleeps, the
    /// ladder is not due, and an unchanged banded frame is not drawn again.
    #[test]
    fn unchanged_band_mode_does_not_redraw_or_recheck() {
        let mut app = App::new(GpuPreference::Automatic);
        app.splash.dismiss();
        app.stage_bands = 16;
        assert_eq!(app.wants_frame(), Redraw::Idle, "band mode alone must not keep the loop awake");

        // At the ceiling, same frame, few frames since the ladder: not due.
        assert!(!band_recheck_due(8, 8, 1, 7, 7));
        assert!(!band_recheck_due(8, 8, BAND_LADDER_RECHECK_FRAMES - 1, 8, 7));
        // Every way it *is* due.
        assert!(band_recheck_due(4, 8, 1, 7, 7), "scale can still grow");
        assert!(band_recheck_due(8, 8, BAND_LADDER_RECHECK_FRAMES, 7, 7));
        assert!(band_recheck_due(8, 8, 1, 9, 7), "moved by more than one frame");

        let bg = Color::WHITE;
        let here = Some((1200, 800, bg));
        assert!(banded_frame_reusable(true, false, true, here, (1200, 800, bg)));
        assert!(!banded_frame_reusable(false, false, true, here, (1200, 800, bg)), "ladder due");
        assert!(!banded_frame_reusable(true, true, true, here, (1200, 800, bg)), "provisional");
        assert!(!banded_frame_reusable(true, false, false, here, (1200, 800, bg)), "stamp moved");
        assert!(!banded_frame_reusable(true, false, true, None, (1200, 800, bg)), "no bands on target");
        assert!(!banded_frame_reusable(true, false, true, here, (1300, 800, bg)), "resized");
        assert!(
            !banded_frame_reusable(true, false, true, here, (1200, 800, Color::BLACK)),
            "background changed"
        );
    }

    /// A WAV of two wandering partials, so a viseme analysis sees changing
    /// shapes rather than one held vowel for the whole clip — the same
    /// fixture `dialogue_to_mouths.rs` uses.
    fn directed_dialogue_wav(seconds: f64) -> Vec<u8> {
        let rate = 44_100.0_f64;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut out, spec).expect("writer");
            let n = (seconds * rate) as usize;
            for i in 0..n {
                let t = i as f64 / rate;
                let v = (t * 180.0 * std::f64::consts::TAU).sin() * 0.5
                    + (t * (700.0 + 400.0 * (t * 3.0).sin()) * std::f64::consts::TAU).sin() * 0.3;
                writer.write_sample((v * 26_000.0) as i16).expect("sample");
            }
            writer.finalize().expect("finalize");
        }
        out.into_inner()
    }

    /// The frames on `layer` that carry a mouth instance — the same helper
    /// `dialogue_to_mouths.rs` checks an imported subtitle's lip sync with.
    fn mouth_frames(app: &App, layer: buzz_scene::LayerId) -> Vec<u32> {
        let scene = app.editor.doc.scene();
        let Some(l) = scene.layers().get(layer) else {
            return Vec::new();
        };
        l.frames
            .keyframes()
            .iter()
            .map(|k| k.start)
            .filter(|at| {
                l.frames
                    .resolved_at(*at)
                    .iter()
                    .any(|o| matches!(o.kind, buzz_scene::ObjectKind::Instance(_)))
            })
            .collect()
    }

    /// **The AI Director's dialogue reaches the caption + lip-sync join a
    /// subtitle import uses.**
    ///
    /// Committing an `AiResult::Directed` with two actors and two dialogue
    /// lines must produce the same observable result
    /// `dialogue_to_mouths.rs` asserts for an imported `.srt`: a Captions
    /// layer, and each speaker's own mouth animated only over their own
    /// lines.
    #[test]
    fn directed_story_dialogue_is_captioned_and_lip_synced() {
        let mut app = App::new(GpuPreference::Automatic);

        // A soundtrack under the dialogue, and a mouth for each speaker —
        // the same setup `dialogue_to_mouths.rs`'s `conversation()` builds.
        let dir = tempfile::tempdir().expect("temp dir");
        let wav = dir.path().join("vo.wav");
        std::fs::write(&wav, directed_dialogue_wav(9.0)).expect("write wav");
        app.editor.import_sound(&wav).expect("the dialogue imports");
        app.editor.doc.edit("Mouths", |scene| {
            buzz_act::lipsync::placeholder_mouth(scene, "Ana");
            buzz_act::lipsync::placeholder_mouth(scene, "Ben");
        });

        let script = buzz_ai::director::DirectorScript {
            setting: "day".into(),
            scenery: "none".into(),
            cast: vec!["Ana".into(), "Ben".into()],
            beats: vec![
                buzz_ai::director::DirectorBeat {
                    actor: "Ana".into(),
                    verb: buzz_ai::director::DirectorVerb::Talk,
                    target: Some("Ben".into()),
                    dialogue: Some("We should go before it gets dark.".into()),
                    gaze: None,
                    seconds: None,
                },
                buzz_ai::director::DirectorBeat {
                    actor: "Ben".into(),
                    verb: buzz_ai::director::DirectorVerb::Talk,
                    target: Some("Ana".into()),
                    dialogue: Some("I am not going anywhere.".into()),
                    gaze: None,
                    seconds: None,
                },
            ],
        };

        // Committed exactly as `ai_direct_story_async`'s background half
        // hands it to `collect_ai_result`.
        let (send, receive) = crossbeam_channel::bounded(1);
        let _ = send.send(AiResult::Directed {
            script: Box::new(script),
            beat_count: 2,
        });
        let id = crate::tasks::TaskId(u64::MAX);
        app.ai_results.insert(id, receive);
        app.collect_ai_result(id);

        let scene = app.editor.doc.scene();
        assert!(
            scene.layers().iter().any(|l| l.name == "Captions"),
            "no Captions layer was created: {:?}",
            app.editor.status
        );
        let ana = scene
            .layers()
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case("Ana"))
            .map(|l| l.id)
            .expect("a layer for Ana");
        let ben = scene
            .layers()
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case("Ben"))
            .map(|l| l.id)
            .expect("a layer for Ben");

        assert!(
            !mouth_frames(&app, ana).is_empty(),
            "Ana has no mouth keys: {:?}",
            app.editor.status
        );
        assert!(
            !mouth_frames(&app, ben).is_empty(),
            "Ben has no mouth keys: {:?}",
            app.editor.status
        );
    }

    /// **A layer built from several shapes rigs as a whole.**
    ///
    /// The old code took only a layer's first object as its part, silently
    /// dropping the rest; `rig_parts_for_symbol` must carry every object on
    /// a layer through as one part, and skip an empty layer outright rather
    /// than standing in with a dummy group.
    #[test]
    fn rig_parts_for_symbol_carries_every_object_of_a_multi_object_layer() {
        let mut symbol = buzz_scene::Symbol::new(
            buzz_scene::SymbolId(9000),
            "TwoShapeBody",
            buzz_scene::SymbolKind::Graphic,
        );

        let mut body = buzz_scene::Layer::normal(buzz_scene::LayerId(9010), "Body");
        body.push_object_at(0, Arc::new(buzz_scene::Object::group(buzz_scene::ObjectId(1), Vec::new())));
        body.push_object_at(0, Arc::new(buzz_scene::Object::group(buzz_scene::ObjectId(2), Vec::new())));
        symbol.layers.push_front(body);

        // An empty layer must not produce a placeholder part.
        symbol.layers.push_front(buzz_scene::Layer::normal(buzz_scene::LayerId(9011), "Empty"));

        let mut scratch = buzz_scene::Scene::default();
        let parts = rig_parts_for_symbol(&mut scratch, &symbol);

        assert_eq!(
            parts.len(),
            1,
            "the empty layer must not produce a part, got {:?}",
            parts.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
        assert_eq!(parts[0].name, "Body");
        match &parts[0].object.kind {
            buzz_scene::ObjectKind::Group(children) => {
                assert_eq!(children.len(), 2, "both of the layer's objects must be carried");
                let ids: Vec<u64> = children.iter().map(|c| c.id.0).collect();
                assert!(ids.contains(&1) && ids.contains(&2), "got ids {ids:?}");
            }
            other => panic!("expected a Group wrapping both objects, got {other:?}"),
        }
    }

    /// **Auto-Rig Vector Character replaces the instance it rigged, instead
    /// of duplicating it.**
    ///
    /// The old, synchronous version left the original instance exactly
    /// where it was and added the armature on a brand new layer beside it,
    /// so the drawing appeared on stage twice. Committing an
    /// `AiResult::Rigged` for a selected stage instance — the "canned"
    /// `ReriggedCharacter` here is the real geometric algorithm run against
    /// a small hand-built skeleton and this symbol's own parts, not the
    /// full render-and-detect pipeline, just the piece `collect_ai_result`
    /// commits — must leave exactly one object where the instance was: the
    /// armature, in the same layer, and no extra layer.
    #[test]
    fn ai_rig_replaces_the_selected_instance_instead_of_duplicating_it() {
        use buzz_geom::{Rect, Shape as _};
        use buzz_scene::ShapeData;

        let mut app = App::new(GpuPreference::Automatic);
        app.recovery.found.clear();

        let sym_id = buzz_scene::SymbolId(9100);
        let mut symbol =
            buzz_scene::Symbol::new(sym_id, "TestBody", buzz_scene::SymbolKind::Graphic);
        let mut torso = buzz_scene::Layer::normal(buzz_scene::LayerId(9110), "Torso");
        torso.push_object_at(
            0,
            Arc::new(buzz_scene::Object::shape(
                buzz_scene::ObjectId(9210),
                ShapeData::filled(Rect::new(-20.0, -60.0, 20.0, 20.0).to_path(1e-6), Color::WHITE),
            )),
        );
        symbol.layers.push_front(torso);
        let mut legs = buzz_scene::Layer::normal(buzz_scene::LayerId(9111), "Legs");
        legs.push_object_at(
            0,
            Arc::new(buzz_scene::Object::shape(
                buzz_scene::ObjectId(9211),
                ShapeData::filled(Rect::new(-20.0, 20.0, 20.0, 80.0).to_path(1e-6), Color::WHITE),
            )),
        );
        symbol.layers.push_front(legs);

        let skel = buzz_ai::character::Skeleton {
            joints: vec![
                buzz_ai::character::Joint { name: "Head".into(), x: 0.0, y: -80.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Chest".into(), x: 0.0, y: -40.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Hips".into(), x: 0.0, y: 20.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Shoulder L".into(), x: -15.0, y: -40.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Elbow L".into(), x: -22.0, y: 0.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Shoulder R".into(), x: 15.0, y: -40.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Elbow R".into(), x: 22.0, y: 0.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Thigh L".into(), x: -10.0, y: 20.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Knee L".into(), x: -10.0, y: 50.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Thigh R".into(), x: 10.0, y: 20.0, confidence: 1.0 },
                buzz_ai::character::Joint { name: "Knee R".into(), x: 10.0, y: 50.0, confidence: 1.0 },
            ],
            bones: Vec::new(),
            face: Default::default(),
            character_height: 160.0,
        };
        let parts = {
            let mut scratch = buzz_scene::Scene::default();
            rig_parts_for_symbol(&mut scratch, &symbol)
        };
        assert!(!parts.is_empty(), "the test symbol has artwork to rig");
        let rerigged = buzz_ai::rerig::rerig_vector_character(&skel, &parts, "TestBody", None)
            .expect("rerig_vector_character should succeed against this fixture");

        let mut placed = None;
        app.editor.doc.edit("Seed", |scene| {
            scene.library_mut().insert(symbol);
            let layer = scene.layers().iter().next().expect("a default layer").id;
            let transform = buzz_geom::Affine::translate((300.0, 200.0));
            let obj_id = scene
                .add_instance_at(layer, 0, sym_id, transform)
                .expect("place the instance");
            placed = Some((layer, obj_id, transform));
        });
        let (stage_layer, instance_id, instance_transform) = placed.expect("seeded an instance");
        app.editor.selection.set([instance_id]);

        let target = RigTarget::ReplaceInstance {
            layer: stage_layer,
            object: instance_id,
            transform: instance_transform,
        };
        let (send, receive) = crossbeam_channel::bounded(1);
        let _ = send.send(AiResult::Rigged {
            target,
            sym_name: "TestBody".into(),
            rerigged: Box::new(rerigged),
        });
        let id = crate::tasks::TaskId(u64::MAX);
        app.ai_results.insert(id, receive);

        let layer_count_before = app.editor.doc.scene().layers().iter().count();
        app.collect_ai_result(id);

        let scene = app.editor.doc.scene();
        assert_eq!(
            scene.layers().iter().count(),
            layer_count_before,
            "no extra layer should be added when the instance is replaced in place"
        );
        assert!(
            scene.find_object(instance_id).is_none(),
            "the original instance id should be gone, replaced rather than left beside the armature"
        );
        let stage_objects = scene
            .layers()
            .get(stage_layer)
            .expect("the layer survives")
            .objects_at(0);
        assert_eq!(
            stage_objects.len(),
            1,
            "exactly one object should stand where the instance was: {:?}",
            stage_objects.iter().map(|o| &o.kind).collect::<Vec<_>>()
        );
        assert!(
            matches!(stage_objects[0].kind, buzz_scene::ObjectKind::Armature(_)),
            "the replacement must be the armature, got {:?}",
            stage_objects[0].kind
        );
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

/// The open character's loaded document and what the Cast panel shows of it.
///
/// Cached because a character is a whole document: enumerating its colours or
/// reading its poses is a walk of the lot, and doing that every frame would
/// make the panel lurch. Rebuilt when the open character changes, and after a
/// recolour edits it. See [`App::cast_panel`].
struct CastSheet {
    /// The asset's path on disk, the key the sheet is matched against.
    path: std::path::PathBuf,
    /// The character's document, recoloured in place as the wardrobe changes
    /// and written back to disk.
    scene: buzz_scene::Scene,
    /// Every colour the character wears, most-worn first — the wardrobe.
    palette: Vec<(peniko::Color, usize)>,
    /// The named poses the character carries.
    poses: Vec<String>,
}

/// Is this asset folder the cast folder, or a folder within it?
fn is_cast(folder: &str) -> bool {
    folder == buzz_ui::CAST_FOLDER || folder.starts_with(&format!("{}/", buzz_ui::CAST_FOLDER))
}

/// The named poses a character carries, in the order first seen and without
/// repeats. Poses live on the rig, so a character built from more than one
/// rigged figure offers each of their pose libraries.
fn poses_of(scene: &buzz_scene::Scene) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for layer in scene.layers().iter() {
        for object in layer.objects_at(0).iter() {
            collect_poses(object, &mut names);
        }
    }
    names
}

/// Gather the pose names on `object` and anything nested inside it.
fn collect_poses(object: &buzz_scene::Object, out: &mut Vec<String>) {
    match &object.kind {
        buzz_scene::ObjectKind::Armature(rig) => {
            for pose in &rig.poses {
                if !out.iter().any(|n| n == &pose.name) {
                    out.push(pose.name.clone());
                }
            }
        }
        buzz_scene::ObjectKind::Group(children) => {
            for child in children.iter() {
                collect_poses(child, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod timeline_splitter_tests {
    use super::*;

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        }
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// The timeline's rect as the splitter sees it: clipped to the window.
    fn timeline(app: &App) -> egui::Rect {
        app.dock_rects
            .iter()
            .find(|(dock, _)| *dock == buzz_ui::Dock::Bottom)
            .map(|(_, rect)| *rect)
            .expect("the timeline was drawn")
    }

    /// Drags the splitter from the timeline's top edge, `dy` points down.
    fn drag_splitter(app: &mut App, ctx: &egui::Context, dy: f32) {
        let from = egui::pos2(800.0, timeline(app).top());
        let run = |app: &mut App, events| {
            let _ = ctx.run_ui(input(events), |ui| {
                app.build_ui(ui);
            });
        };
        run(app, vec![egui::Event::PointerMoved(from), click(from, true)]);
        for step in 1..=8 {
            let pos = from + egui::vec2(0.0, dy * step as f32 / 8.0);
            run(app, vec![egui::Event::PointerMoved(pos)]);
        }
        run(app, vec![click(from + egui::vec2(0.0, dy), false)]);
        run(app, vec![]);
        run(app, vec![]);
    }

    /// **Dragging the timeline all the way down is not a one-way trip.** At
    /// the splitter's 80-point floor the timeline's contents no longer fit,
    /// and egui used to place the overflowing panel below the window: the
    /// timeline vanished, its edge with it, and nothing could bring it back.
    #[test]
    fn timeline_dragged_to_its_floor_can_be_dragged_back() {
        let mut app = App::new(GpuPreference::Automatic);
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        for _ in 0..2 {
            let _ = ctx.run_ui(input(vec![]), |ui| {
                app.build_ui(ui);
            });
        }
        let start = app.editor.workspace.bottom_height;

        drag_splitter(&mut app, &ctx, 600.0);
        let floor = *buzz_ui::workspace::BOTTOM_HEIGHT_RANGE.start();
        assert_eq!(app.editor.workspace.bottom_height, floor);
        let squeezed = timeline(&app);
        assert!(
            squeezed.height() >= floor - 1.0 && squeezed.bottom() <= 1000.0,
            "the squeezed timeline should still be on screen, {floor} points tall: {squeezed:?}"
        );

        drag_splitter(&mut app, &ctx, -(start - floor));
        assert_eq!(
            app.editor.workspace.bottom_height, start,
            "dragging the edge back up should restore the timeline"
        );
    }
}

#[cfg(test)]
mod title_bar_tests {
    use super::*;
    use crate::title_bar::{TitleBar, WindowRequest, resize_direction};

    fn screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0))
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            events,
            screen_rect: Some(screen()),
            ..Default::default()
        }
    }

    fn custom_app() -> App {
        let mut app = App::new(GpuPreference::Automatic);
        app.title_bar = TitleBar::Custom;
        app
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// **The menu shares the title row** with the logo, the drag zone and
    /// the window buttons — one 32-point strip, not the old menu row under a
    /// separate OS title bar.
    #[test]
    fn menus_share_the_title_row() {
        let mut app = custom_app();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let mut stage_area = egui::Rect::NOTHING;

        let _ = ctx.run_ui(input(vec![]), |ui| {
            stage_area = app.build_ui(ui);
        });

        let drag = app.title_rects.drag.expect("the drag zone was drawn");
        let close = app.title_rects.close.expect("the close button was drawn");
        assert!(
            drag.top() < 8.0,
            "the drag zone should start at the very top of the window: {drag:?}"
        );
        assert!(
            (drag.top() - close.top()).abs() < 1.0,
            "the drag zone and the window buttons should share one row: {drag:?} vs {close:?}"
        );
        assert!(
            stage_area.top() >= close.bottom() - 1.0
                && stage_area.top() < close.bottom() + 40.0,
            "the stage should start just below the title row, not after a \
             gap the size of a second one: stage {stage_area:?}, row bottom {}",
            close.bottom()
        );
    }

    /// Clicking Close queues a `Close` request — the same path the OS's own
    /// close button takes, asked about rather than acted on immediately.
    #[test]
    fn close_button_asks_to_quit() {
        let mut app = custom_app();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        let _ = ctx.run_ui(input(vec![]), |ui| {
            app.build_ui(ui);
        });
        let close = app.title_rects.close.expect("the close button was drawn");
        let at = close.center();

        let _ = ctx.run_ui(
            input(vec![egui::Event::PointerMoved(at), click(at, true), click(at, false)]),
            |ui| {
                app.build_ui(ui);
            },
        );

        assert!(
            app.window_requests.contains(&WindowRequest::Close),
            "clicking Close should have queued a Close request"
        );
    }

    /// Double-clicking the drag zone toggles maximised, the same as
    /// double-clicking any native title bar.
    #[test]
    fn double_click_on_the_bar_toggles_maximize() {
        let mut app = custom_app();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        let _ = ctx.run_ui(input(vec![]), |ui| {
            app.build_ui(ui);
        });
        let at = app
            .title_rects
            .drag
            .expect("the drag zone was drawn")
            .center();

        // Two press-release cycles at the same point: egui's own clock here
        // is the deterministic frame-to-frame `predicted_dt`, not the wall
        // clock, so two frames are always well inside the double-click
        // window.
        let _ = ctx.run_ui(
            input(vec![egui::Event::PointerMoved(at), click(at, true), click(at, false)]),
            |ui| {
                app.build_ui(ui);
            },
        );
        let _ = ctx.run_ui(input(vec![click(at, true), click(at, false)]), |ui| {
            app.build_ui(ui);
        });

        assert!(
            app.window_requests.contains(&WindowRequest::ToggleMaximize),
            "double-clicking the drag zone should toggle maximise"
        );
    }

    /// Dragging the bar starts a window drag — the grab handle every native
    /// title bar offers, drawn here instead.
    #[test]
    fn dragging_the_bar_starts_a_window_drag() {
        let mut app = custom_app();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        let _ = ctx.run_ui(input(vec![]), |ui| {
            app.build_ui(ui);
        });
        let from = app
            .title_rects
            .drag
            .expect("the drag zone was drawn")
            .center();
        let to = from + egui::vec2(40.0, 0.0);

        let _ = ctx.run_ui(
            input(vec![egui::Event::PointerMoved(from), click(from, true)]),
            |ui| {
                app.build_ui(ui);
            },
        );
        for step in 1..=6 {
            let t = step as f32 / 6.0;
            let pos = from + (to - from) * t;
            let _ = ctx.run_ui(input(vec![egui::Event::PointerMoved(pos)]), |ui| {
                app.build_ui(ui);
            });
        }

        assert!(
            app.window_requests.contains(&WindowRequest::StartDrag),
            "dragging the title bar should have started a window drag"
        );
    }

    /// Presses at `from` on a Custom-mode app `width` points wide, drags 40
    /// points right, and reports whether a window drag was asked for.
    fn drags_window_from(width: f32, from: egui::Pos2) -> bool {
        let mut app = custom_app();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let raw = |events| egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw(vec![]), |ui| {
            app.build_ui(ui);
        });
        let _ = ctx.run_ui(
            raw(vec![egui::Event::PointerMoved(from), click(from, true)]),
            |ui| {
                app.build_ui(ui);
            },
        );
        for step in 1..=6 {
            let pos = from + egui::vec2(40.0 * step as f32 / 6.0, 0.0);
            let _ = ctx.run_ui(raw(vec![egui::Event::PointerMoved(pos)]), |ui| {
                app.build_ui(ui);
            });
        }
        app.window_requests.contains(&WindowRequest::StartDrag)
    }

    /// **The bar's bare space drags too**, not just the gap around the
    /// name: right of the menus, well clear of the name's zone, and on a
    /// window narrow enough that the name's zone is not drawn at all.
    #[test]
    fn dragging_bare_bar_anywhere_starts_a_window_drag() {
        assert!(
            drags_window_from(1600.0, egui::pos2(700.0, 18.0)),
            "bare bar between the menus and the name should drag the window"
        );
        assert!(
            drags_window_from(1100.0, egui::pos2(600.0, 18.0)),
            "a narrow window's bare bar should still drag the window"
        );
    }

    /// **The frame time changing does not move the bar.** The readout is
    /// redrawn every frame, and its width used to set where the menu ended,
    /// so Reset Layout (pinned to the menu's right edge) and the name's zone
    /// shook whenever the pointer over the bar kept frames coming.
    #[test]
    fn frame_time_changes_do_not_shift_the_bar() {
        let mut app = custom_app();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        let mut drag_zone_after = |app: &mut App, ms: u64| {
            app.profiler.begin_frame();
            app.profiler.enter(crate::profile::Section::Ui);
            std::thread::sleep(Duration::from_millis(ms));
            app.profiler.end_frame();
            for _ in 0..2 {
                let _ = ctx.run_ui(input(vec![]), |ui| {
                    app.build_ui(ui);
                });
            }
            app.title_rects.drag.expect("the drag zone was drawn")
        };

        let fast = drag_zone_after(&mut app, 1);
        let slow = drag_zone_after(&mut app, 120);
        assert_eq!(
            fast, slow,
            "a '1.x ms' and a '12x.x ms' readout should leave the bar laid out identically"
        );
    }

    /// Pressing a window button and sliding off cancels the press; it does
    /// not fall through to the background and drag the window away.
    #[test]
    fn dragging_off_a_window_button_does_not_move_the_window() {
        let close = {
            let mut app = custom_app();
            let ctx = egui::Context::default();
            buzz_ui::theme::apply(&ctx);
            let _ = ctx.run_ui(input(vec![]), |ui| {
                app.build_ui(ui);
            });
            app.title_rects.close.expect("the close button was drawn")
        };
        assert!(
            !drags_window_from(screen().width(), close.center() - egui::vec2(20.0, 0.0)),
            "a press on the close button should not start a window drag"
        );
    }

    /// Native mode draws none of this: no drag zone, no window buttons, the
    /// menu row exactly as it always was.
    #[test]
    fn native_mode_draws_no_window_buttons() {
        let mut app = App::new(GpuPreference::Automatic);
        app.title_bar = TitleBar::Native;
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        let _ = ctx.run_ui(input(vec![]), |ui| {
            app.build_ui(ui);
        });

        assert!(app.title_rects.drag.is_none());
        assert!(app.title_rects.close.is_none());
        assert!(app.title_rects.minimize.is_none());
        assert!(app.title_rects.maximize.is_none());
    }

    fn win(w: f32, h: f32) -> egui::Vec2 {
        egui::vec2(w, h)
    }

    #[test]
    fn resize_direction_leaves_the_interior_alone() {
        assert_eq!(resize_direction(egui::pos2(400.0, 300.0), win(800.0, 600.0), 6.0), None);
    }

    #[test]
    fn resize_direction_finds_the_four_edges() {
        use winit::window::ResizeDirection::{East, North, South, West};
        let size = win(800.0, 600.0);
        assert_eq!(resize_direction(egui::pos2(0.0, 300.0), size, 6.0), Some(West));
        assert_eq!(resize_direction(egui::pos2(800.0, 300.0), size, 6.0), Some(East));
        assert_eq!(resize_direction(egui::pos2(400.0, 0.0), size, 6.0), Some(North));
        assert_eq!(resize_direction(egui::pos2(400.0, 600.0), size, 6.0), Some(South));
    }

    #[test]
    fn resize_direction_finds_the_four_corners() {
        use winit::window::ResizeDirection::{NorthEast, NorthWest, SouthEast, SouthWest};
        let size = win(800.0, 600.0);
        assert_eq!(
            resize_direction(egui::pos2(0.0, 0.0), size, 6.0),
            Some(NorthWest)
        );
        assert_eq!(
            resize_direction(egui::pos2(800.0, 0.0), size, 6.0),
            Some(NorthEast)
        );
        assert_eq!(
            resize_direction(egui::pos2(0.0, 600.0), size, 6.0),
            Some(SouthWest)
        );
        assert_eq!(
            resize_direction(egui::pos2(800.0, 600.0), size, 6.0),
            Some(SouthEast)
        );
    }

    /// A corner is tested before the edges it overlaps.
    #[test]
    fn resize_direction_prefers_corners_over_edges() {
        use winit::window::ResizeDirection::NorthWest;
        let size = win(800.0, 600.0);
        assert_eq!(
            resize_direction(egui::pos2(3.0, 3.0), size, 6.0),
            Some(NorthWest)
        );
    }

    /// The border is in physical pixels, so it scales with the display.
    #[test]
    fn resize_direction_scales_the_border_with_the_display() {
        use winit::window::ResizeDirection::West;
        let size = win(800.0, 600.0);
        let border = 6.0 * 1.5;
        assert_eq!(
            resize_direction(egui::pos2(8.0, 300.0), size, border),
            Some(West)
        );
        assert_eq!(resize_direction(egui::pos2(10.0, 300.0), size, border), None);
    }
}

#[cfg(test)]
mod hit_target_tests {
    use super::*;

    /// **Stage 6, item 3 of `docs/WORKSPACE_DESIGN_PLAN.md`.**
    ///
    /// "A headless audit test walks the `egui::Context` accesskit tree ... and
    /// fails on any clickable smaller than 24 × 24 at scale 1." Built the same
    /// way `ui_snapshot`'s screenshots and `dock_drag_tests::Drive` are: a
    /// real `App::build_ui`, two frames (the first lays the dock out; egui's
    /// own `ScrollArea`s and column widths are a frame late), no window and no
    /// GPU — `Context::enable_accesskit` builds the tree into
    /// `FullOutput::platform_output.accesskit_update` regardless of whether a
    /// screen reader is actually attached to read it.
    ///
    /// **The two exemptions the plan names**, both applied by the dock that
    /// holds them rather than by node role or label — a raw
    /// `ui.interact(rect, id, Sense::click())` frame cell or swatch chip
    /// carries neither (see `buzz_ui::widgets::icon_button`'s own doc comment
    /// on why nothing here can assume a widget painted its own text):
    /// - **The timeline's frame grid**: `timeline_panel.rs` draws a cell per
    ///   frame at `state.frame_width` × the row height, deliberately as dense
    ///   as Animate's own. Exempted by the whole Timeline dock's rect —
    ///   transport and footer included — both of which were already brought
    ///   to `Tokens::CONTROL_H` elsewhere in this same change, so nothing
    ///   there currently *relies* on the exemption to pass; it is here so a
    ///   cell at the default 12 px frame width does not fail the run.
    /// - **The colour swatch grid**: `panels::swatch_chip` is a fixed 16×16.
    ///   Exempted by whichever dock holds `PanelId::Swatches` or
    ///   `PanelId::Color`. Essentials hides both, so this exemption does not
    ///   fire in the layout this test actually builds — it is here for
    ///   Animator and Classic, which show them, and is exercised by
    ///   `theme::tests::ordinary_controls_already_meet_the_minimum_hit_target`'s
    ///   sibling numeric check rather than a second live render.
    #[test]
    fn every_clickable_in_essentials_clears_the_minimum_hit_target() {
        use egui::accesskit::{Action, Role};

        let mut app = App::new(GpuPreference::Automatic)
            .with_document(crate::ui_snapshot::fixture_document());
        app.editor.workspace = buzz_ui::Workspace::essentials();
        app.recovery.found.clear();

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        ctx.enable_accesskit();

        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
        let input = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let _ = ctx.run_ui(input(), |ui| {
            app.build_ui(ui);
        });
        let output = ctx.run_ui(input(), |ui| {
            app.build_ui(ui);
        });

        let update = output
            .platform_output
            .accesskit_update
            .expect("enable_accesskit was called, so a tree update must exist");

        let exempt_docks: Vec<egui::Rect> = [
            buzz_ui::Dock::Bottom,
            app.editor.workspace.dock_of(buzz_ui::PanelId::Swatches),
            app.editor.workspace.dock_of(buzz_ui::PanelId::Color),
        ]
        .into_iter()
        .flat_map(|dock| {
            app.dock_rects
                .iter()
                .filter(move |(d, _)| *d == dock)
                .map(|(_, r)| *r)
        })
        .collect();

        const MIN: f32 = buzz_ui::theme::Tokens::MIN_TARGET;
        let mut min_seen = f32::MAX;
        let mut violations = Vec::new();

        for (id, node) in &update.nodes {
            if !node.supports_action(Action::Click) {
                continue;
            }

            // **A third, principled exclusion, beyond the plan's two named
            // grids**: roles that are not "a control a pointer aims at" in
            // the sense a hit-target audit means, each already covered by a
            // more specific test of its own rather than this blanket one:
            // - `Label`: egui makes ordinary body text focusable and
            //   click-sensitive so a screen reader (and a mouse, for cursor
            //   placement) can select it — every `text_dim` field label this
            //   change's `panels::property_row` draws shows up here. Reading
            //   a sentence does not need a 24 px target the way pressing a
            //   button does.
            // - `TextInput`: a text field's target is its whole width, which
            //   is what a pointer actually has to land in to place a caret —
            //   `Tokens::CONTROL_H` governs the *row* these sit in; the field
            //   itself (e.g. the hex box beside a colour swatch) is judged by
            //   whether it fits its text, not by a fixed square.
            // - `Unknown`: a dock's resize handle — a 6–9 px strip the length
            //   of the whole column, not a button; the long axis is what a
            //   pointer actually has to hit, and it already spans hundreds of
            //   points.
            // - `ScrollBar`: `theme::apply` sets `scroll.bar_width = 6.0`
            //   deliberately (Stage 1, item 5) so it does not cover a
            //   column's own right-edge controls; a scrollbar is dragged
            //   along its long axis, not aimed at like a button.
            if matches!(
                node.role(),
                Role::Label | Role::TextInput | Role::Unknown | Role::ScrollBar
            ) {
                continue;
            }

            // **A fourth exclusion, by name rather than role or region**:
            // `section_header`'s own roll-up triangle and "..." panel menu,
            // in a column too narrow to widen them without overrunning it —
            // the Tools column is 55 pt at its narrowest, shared there with
            // the drag grip, and `dock_geometry_tests::the_stage_never_reaches_into_a_dock_column`
            // already fails the moment any of the three grows past its text.
            // Both stay at `Tokens::CONTROL_H` (24) tall; only the width is
            // exempted, and only for these two exact glyphs.
            if matches!(node.label(), Some(PANEL_MENU) | Some(ROLLED_UP) | Some(OPEN)) {
                continue;
            }

            let Some(b) = node.bounds() else { continue };
            let (w, h) = ((b.x1 - b.x0) as f32, (b.y1 - b.y0) as f32);
            if w <= 0.0 || h <= 0.0 {
                // A handful of structural nodes (the root window itself)
                // carry a click action with no meaningful size of their own —
                // not a control anybody can miss with a pointer.
                continue;
            }
            min_seen = min_seen.min(w.min(h));
            if w >= MIN && h >= MIN {
                continue;
            }
            let rect = egui::Rect::from_min_max(
                egui::pos2(b.x0 as f32, b.y0 as f32),
                egui::pos2(b.x1 as f32, b.y1 as f32),
            );
            let exempt = exempt_docks.iter().any(|d| d.contains_rect(rect));
            if !exempt {
                violations.push(format!(
                    "{id:?}: {w:.1}x{h:.1} at {rect:?} role={:?} label={:?}",
                    node.role(),
                    node.label()
                ));
            }
        }

        assert!(
            violations.is_empty(),
            "clickable(s) smaller than {MIN}x{MIN} outside the timeline frame grid \
             and the colour swatch grid:\n{}",
            violations.join("\n")
        );

        // Not asserted on — a debug breadcrumb for whoever reads this test's
        // output back, same reason `ordinary_controls_already_meet_the_minimum_hit_target`
        // names the tokens it checks rather than only asserting `>= 24.0`.
        println!("smallest checked clickable: {min_seen:.1} px");
    }
}

/// **F7.10: the end-to-end AI report, through the app** (`docs/AI_FEATURES_REPORT.md`).
///
/// Every flow runs the way the AI panel runs it — `App::new` + `build_ui`
/// frames, the panel's own actions, the task registry, `collect_ai_result`,
/// Build/Apply — on the user's real characters, and every picture is an
/// export of the resulting document or a screenshot of the window.
///
/// `BUZZ_AI_REPORT_BACKEND=local|claude cargo test --release -p buzz-app --lib
/// ai_features_report_pass -- --ignored --nocapture`. Writes PNGs and
/// `timings.json` under `docs/images/ai/<backend>/`. The Claude pass reads
/// its key from `ai.json` (never written anywhere by this test).
#[cfg(test)]
mod ai_report_tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const BHATTA2: &str = "B:/youtubeProjects/Khayal3Baje/stories/bhatta/animate/bhatta2.fla";
    const CHARACTER_PNG: &str = "C:/Users/singh/AppData/Local/Temp/claude/B--youtubeProjects-Buzzcaf-Media-BuzzAnimate/bd7c677d-48be-421f-8a2e-89b1993271b7/scratchpad/f7report/ThakurBhaiSide_copy_2_0.png";

    fn out_dir(backend: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/images/ai").join(backend);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn screen() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            ..Default::default()
        }
    }

    fn frame(app: &mut App, ctx: &egui::Context) {
        let _ = ctx.run_ui(screen(), |ui| {
            app.build_ui(ui);
        });
        app.poll_tasks();
    }

    /// Frames until no AI task is running and every result is collected.
    fn wait_ai(app: &mut App, ctx: &egui::Context, what: &str, limit: Duration) -> f64 {
        let t = Instant::now();
        loop {
            frame(app, ctx);
            let running = app.tasks.running().any(|t| t.kind == crate::tasks::TaskKind::Ai);
            if !running && app.ai_results.is_empty() {
                return t.elapsed().as_secs_f64();
            }
            assert!(t.elapsed() < limit, "{what} did not finish in {limit:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn render(scene: &buzz_scene::Scene, frame_no: u32, scale: f64, path: &Path) -> image::RgbaImage {
        let mut ex = buzz_export::Exporter::new(&GpuPreference::Automatic).expect("GPU");
        let f = ex.render(scene, frame_no, &buzz_export::ExportSettings::scaled(scene, scale)).expect("render");
        let _ = f.write_png(path);
        image::RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
    }

    fn strip(images: &[image::RgbaImage], height: u32, path: &Path) {
        let scaled: Vec<image::RgbaImage> = images
            .iter()
            .map(|i| {
                let w = ((i.width() as f64) * height as f64 / i.height().max(1) as f64).round().max(1.0) as u32;
                image::imageops::resize(i, w, height, image::imageops::FilterType::Triangle)
            })
            .collect();
        let total: u32 = scaled.iter().map(|i| i.width() + 8).sum();
        let mut out = image::RgbaImage::from_pixel(total.max(1), height, image::Rgba([255, 255, 255, 255]));
        let mut x = 0;
        for i in &scaled {
            image::imageops::overlay(&mut out, i, x as i64, 0);
            x += i.width() + 8;
        }
        out.save(path).unwrap();
    }

    fn bhatta2_app() -> App {
        let imported = crate::import::read(Path::new(BHATTA2)).expect("bhatta2 imports");
        let mut app = App::new(GpuPreference::Automatic).with_document(buzz_doc::Document::new(imported.scene));
        app.recovery.found.clear();
        app
    }

    fn set_backend(app: &mut App, backend: &str) -> (String, String) {
        let ai = &mut app.editor.workspace.ai;
        ai.low_reasoning = true;
        if backend == "claude" {
            ai.backend = Some(buzz_ai::llm::Backend::default_claude_api());
            ai.model = Some("sonnet".into());
            ai.vision_model = Some("sonnet".into());
        } else {
            ai.backend = Some(buzz_ai::llm::Backend::default_openai());
            // The Models section's recommendation (F7.3): qwen3.8-27b timed
            // out at 900 s on this brief inside the app (F7.10 run 2), so the
            // report's local pass uses gemma-4-12b for text too.
            ai.model = Some(std::env::var("BUZZ_AI_REPORT_TEXT_MODEL").unwrap_or_else(|_| "google/gemma-4-12b".into()));
            ai.vision_model = Some("google/gemma-4-12b".into());
        }
        (ai.model.clone().unwrap(), ai.vision_model.clone().unwrap())
    }

    #[test]
    #[ignore = "real models, GPU, bhatta2/barish1; writes docs/images/ai"]
    fn ai_features_report_pass() {
        let backend = std::env::var("BUZZ_AI_REPORT_BACKEND").unwrap_or_else(|_| "local".into());
        let out = out_dir(&backend);
        // Sections to run (`BUZZ_AI_REPORT_ONLY=rename,transcribe`); all by
        // default. Timings merge into the existing file so a partial rerun
        // keeps the rest.
        let only = std::env::var("BUZZ_AI_REPORT_ONLY").unwrap_or_default();
        let run = |name: &str| only.is_empty() || only.split(',').any(|x| x == name);
        let mut timings: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(out.join("timings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let mut note = |k: &str, v: serde_json::Value| {
            println!("{k}: {v}");
            timings.insert(k.to_string(), v);
        };
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        // ---------------- Character from Image (ONNX; the same in both passes)
        if backend == "local" && run("character") {
            let mut app = App::new(GpuPreference::Automatic);
            app.recovery.found.clear();
            let assets = tempfile::tempdir().unwrap();
            app.editor.assets = buzz_doc::AssetLibrary::at(assets.path());
            app.editor.doc.edit("Stage", |s| s.stage_mut().size = buzz_geom::Size::new(1400.0, 1300.0));
            app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
            let _ = std::fs::copy(CHARACTER_PNG, out.join("character_input.png"));
            app.editor.ai_character.image_path_input = CHARACTER_PNG.into();
            app.editor.ai_character.character_name = "ThakurBhai AI".into();
            // The report measures the bone rig and its pose sheet (Layers is
            // the default since LR2).
            app.editor.ai_character.rig_kind = buzz_rig::RigKind::Bones;
            let t = Instant::now();
            buzz_ui::ai_character_dialog::start_skeleton_detection(&mut app.editor.ai_character, &ctx);
            while app.editor.ai_character.step != buzz_ui::AiCharacterStep::JointEditor {
                frame(&mut app, &ctx);
                if let Some((m, true)) = &app.editor.ai_character.status_message {
                    panic!("detect: {m}");
                }
                assert!(t.elapsed() < Duration::from_secs(180));
                std::thread::sleep(Duration::from_millis(30));
            }
            note("character_detect_s", t.elapsed().as_secs_f64().into());
            let t = Instant::now();
            buzz_ui::ai_character_dialog::start_parts_processing(&mut app.editor.ai_character, &ctx);
            while app.editor.ai_character.step != buzz_ui::AiCharacterStep::Preview {
                frame(&mut app, &ctx);
                if let Some((m, true)) = &app.editor.ai_character.status_message {
                    panic!("process: {m}");
                }
                assert!(t.elapsed() < Duration::from_secs(300));
                std::thread::sleep(Duration::from_millis(30));
            }
            note("character_process_s", t.elapsed().as_secs_f64().into());
            let t = Instant::now();
            match buzz_ui::ai_character_dialog::create_character(&mut app.editor.ai_character) {
                buzz_ui::AiCharacterDialogAction::Created(a, _) => app.editor.ai_character.assembled = Some(a),
                _ => panic!("create produced nothing"),
            }
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::CharacterCreated);
            note("character_create_s", t.elapsed().as_secs_f64().into());
            let pose_s = wait_ai(&mut app, &ctx, "pose sheet", Duration::from_secs(120));
            note("character_pose_sheet_s", pose_s.into());
            frame(&mut app, &ctx);
            let poses = app.ai_pose_images.take();
            // `draw_ai_panel` has already uploaded them; render again for the file.
            let scene = app.editor.doc.scene().clone();
            let arm = scene
                .layers()
                .iter()
                .flat_map(|l| l.objects_at(0).to_vec())
                .find(|o| matches!(o.kind, buzz_scene::ObjectKind::Armature(_)))
                .expect("an armature on stage");
            let sheet = poses.unwrap_or_else(|| render_pose_sheet(&scene, &arm).unwrap());
            let imgs: Vec<image::RgbaImage> = sheet.into_iter().map(|(_, i)| i).collect();
            strip(&imgs, 360, &out.join("character_pose_sheet.png"));
            note("character_bones", serde_json::json!(match &arm.kind {
                buzz_scene::ObjectKind::Armature(r) => r.armature.bones.len(),
                _ => 0,
            }));
            app.ai_panel.focus = Some(1);
            let _ = crate::ui_snapshot::write_ui_snapshot(
                &mut app,
                crate::ui_snapshot::SnapshotConfig { width: 1600, height: 1000, scale: 1.0 },
                &out.join("panel_character.png"),
            );
        }

        // ---------------- Auto-Rig (ONNX)
        if backend == "local" && run("autorig") {
            let mut app = bhatta2_app();
            app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
            let sym = app.editor.doc.scene().library().iter().find(|s| s.name == "sadhuSide").map(|s| s.id);
            app.editor.library.selected = sym;
            let t = Instant::now();
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::AutoRigDetect);
            wait_ai(&mut app, &ctx, "auto-rig", Duration::from_secs(180));
            note("autorig_detect_s", t.elapsed().as_secs_f64().into());
            let rig = app.ai_panel.rig.clone().expect("a mapping to review");
            note("autorig_mapping", serde_json::json!(rig.mapping));
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::AutoRigApply);
            let scene = app.editor.doc.scene().clone();
            let arm = scene
                .layers()
                .iter()
                .flat_map(|l| l.objects_at(app.editor.current_frame).to_vec())
                .find(|o| matches!(o.kind, buzz_scene::ObjectKind::Armature(_)))
                .expect("the rig landed");
            let sheet = render_pose_sheet(&scene, &arm).unwrap();
            let imgs: Vec<image::RgbaImage> = sheet.into_iter().map(|(_, i)| i).collect();
            strip(&imgs, 360, &out.join("autorig_pose_sheet.png"));
        }

        // ---------------- Scene from Image
        if run("scene") {
            let mut app = bhatta2_app();
            let (_, vision) = set_backend(&mut app, &backend);
            app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
            let scene = app.editor.doc.scene().clone();
            let reference = out.join("scene_reference.png");
            render(&scene, 0, 0.5, &reference);
            let original: Vec<buzz_scene::LayerId> = scene.stage_layers().iter().map(|l| l.id).collect();
            let t = Instant::now();
            app.ai_next_from_panel = true;
            app.ai_scene_from_image_from(reference.clone());
            wait_ai(&mut app, &ctx, "scene from image", Duration::from_secs(900));
            note("scene_analyse_s", t.elapsed().as_secs_f64().into());
            note("scene_model", vision.into());
            let review = app.ai_panel.scene.clone().expect("rows to review");
            note(
                "scene_rows",
                serde_json::json!(review
                    .rows
                    .iter()
                    .map(|r| format!(
                        "{} -> {}",
                        r.label,
                        r.chosen.and_then(|c| r.shortlist.get(c)).map(|(n, v)| format!("{n} {v:.2}")).unwrap_or("-".into())
                    ))
                    .collect::<Vec<_>>()),
            );
            app.ai_panel.focus = Some(3);
            let _ = crate::ui_snapshot::write_ui_snapshot(
                &mut app,
                crate::ui_snapshot::SnapshotConfig { width: 1600, height: 1000, scale: 1.0 },
                &out.join("panel_scene.png"),
            );
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::SceneBuild);
            app.editor.doc.edit("Report: hide the original", |s| {
                for id in &original {
                    s.update_layer(*id, |l| l.visible = false);
                }
                s.camera_mut().enabled = false;
            });
            render(app.editor.doc.scene(), 0, 0.5, &out.join("scene_rebuilt.png"));
            note("scene_status", serde_json::json!(app.editor.status.clone()));
        }

        // ---------------- Direct Story (dialogue, captions, lip sync)
        if run("story") {
            let mut app = App::new(GpuPreference::Automatic);
            app.recovery.found.clear();
            let (text, _) = set_backend(&mut app, &backend);
            app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
            let brief = "Morning. Village. Ramu walks in from the left and waves. Thakur turns to him and says, \"The rain is late this year.\" Ramu listens, then says, \"Then we pray tonight.\" Ramu walks off.";
            note("story_brief", brief.into());
            let t = Instant::now();
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::DirectStory(brief.into()));
            wait_ai(&mut app, &ctx, "direct story", Duration::from_secs(900));
            note("story_parse_s", t.elapsed().as_secs_f64().into());
            note("story_model", text.into());
            let Some(review) = app.ai_panel.story.clone() else {
                note("story_error", serde_json::json!(app.editor.status.clone()));
                std::fs::write(out.join("timings.json"), serde_json::to_string_pretty(&timings).unwrap()).unwrap();
                panic!("no story to review: {:?}", app.editor.status);
            };
            note("story_beats", serde_json::json!(review.beats));
            note("story_lines", serde_json::json!(review.cues));
            app.ai_panel.focus = Some(4);
            let _ = crate::ui_snapshot::write_ui_snapshot(
                &mut app,
                crate::ui_snapshot::SnapshotConfig { width: 1600, height: 1000, scale: 1.0 },
                &out.join("panel_story.png"),
            );
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::StoryApply);
            note("story_status", serde_json::json!(app.editor.status.clone()));
            let scene = app.editor.doc.scene().clone();
            let n = scene.frame_count().max(1);
            let caption_frames: Vec<u32> = scene
                .layers()
                .iter()
                .find(|l| l.name.to_lowercase().contains("caption"))
                .map(|l| l.frames.keyframes().iter().filter(|k| k.label.is_some()).map(|k| k.start + 10).collect())
                .unwrap_or_default();
            note("story_caption_keys", serde_json::json!(caption_frames.len()));
            let mut frames = vec![0, n / 3];
            frames.extend(caption_frames.iter().copied());
            let imgs: Vec<image::RgbaImage> = frames
                .iter()
                .map(|f| render(&scene, *f, 0.5, &out.join(format!("story_f{f}.png"))))
                .collect();
            strip(&imgs, 300, &out.join("story_strip.png"));
        }

        // ---------------- Identify & Rename
        if run("rename") {
            // The whole of bhatta2 is 589 symbols — over 30 minutes of VLM
            // batches on either backend (F7.10 run 3). The report renames the
            // placeable top-level symbols that have artwork of their own and
            // nest nothing (props, scenery, whole drawings): 32 of them, with
            // the full library kept for drawing.
            let mut app = bhatta2_app();
            let full = app.editor.doc.scene().clone();
            let nests = |sym: &buzz_scene::Symbol| {
                sym.layers.iter().any(|l| l.all_objects().any(|o| !matches!(o.kind, buzz_scene::ObjectKind::Shape(_))))
            };
            let keep: Vec<buzz_scene::SymbolId> = buzz_ai::scene_builder::placeable_symbols(&full)
                .into_iter()
                .filter(|id| full.library().get(*id).is_some_and(|s| !nests(s)))
                .take(32)
                .collect();
            let (_, vision) = set_backend(&mut app, &backend);
            app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
            let t = Instant::now();
            {
                let mut trimmed = full.clone();
                let drop: Vec<buzz_scene::SymbolId> =
                    trimmed.library().iter().map(|s| s.id).filter(|id| !keep.contains(id)).collect();
                for id in drop {
                    trimmed.library_mut().remove(id);
                }
                let backend_v = app.editor.workspace.ai.backend.clone().unwrap();
                let model = app.editor.workspace.ai.vision_model.clone().unwrap();
                app.editor.ai_rename.open = true;
                app.editor.ai_rename.start_for_scene(&trimmed, backend_v, model);
            }
            loop {
                frame(&mut app, &ctx);
                if !app.editor.ai_rename.is_running {
                    break;
                }
                assert!(t.elapsed() < Duration::from_secs(1800), "rename took too long");
                std::thread::sleep(Duration::from_millis(100));
            }
            note("rename_s", t.elapsed().as_secs_f64().into());
            note("rename_model", vision.into());
            let proposals = app.editor.ai_rename.proposals.clone();
            note("rename_count", serde_json::json!(proposals.len()));
            note(
                "rename_sample",
                serde_json::json!(proposals.iter().take(24).map(|p| format!("{} -> {}", p.old_name, p.new_name)).collect::<Vec<_>>()),
            );
            app.ai_panel.focus = Some(5);
            let _ = crate::ui_snapshot::write_ui_snapshot(
                &mut app,
                crate::ui_snapshot::SnapshotConfig { width: 1600, height: 1000, scale: 1.0 },
                &out.join("panel_rename.png"),
            );
        }

        // ---------------- Transcribe (ONNX Whisper; the same in both passes)
        if backend == "local" && run("transcribe") {
            let mut app = App::new(GpuPreference::Automatic);
            app.recovery.found.clear();
            app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
            let wav = Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzz-ai/tests/data/speech_short.wav");
            let name = app.editor.import_sound(&wav).unwrap();
            // Somebody to lip-sync: a symbol named after the speaker.
            app.editor.doc.edit("Ramu", |s| {
                s.add_symbol("Ramu", buzz_scene::SymbolKind::Graphic, None);
            });
            let t = Instant::now();
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::Transcribe(name.clone()));
            wait_ai(&mut app, &ctx, "transcribe", Duration::from_secs(300));
            note("transcribe_s", t.elapsed().as_secs_f64().into());
            let tr = app.ai_panel.transcript.clone().expect("lines to review");
            note("transcribe_lines", serde_json::json!(tr.lines.iter().map(|(a, b, x)| format!("[{a:.2}-{b:.2}] {x}")).collect::<Vec<_>>()));
            note("transcribe_provider", tr.provider.clone().into());
            app.ai_panel.transcribe_speaker = "Ramu".into();
            app.ai_panel.transcribe_sound = Some(name);
            app.ai_panel.focus = Some(6);
            let _ = crate::ui_snapshot::write_ui_snapshot(
                &mut app,
                crate::ui_snapshot::SnapshotConfig { width: 1600, height: 1000, scale: 1.0 },
                &out.join("panel_transcribe.png"),
            );
            app.ai_panel_action(buzz_ui::ai_panel::AiPanelAction::TranscriptApply);
            note("transcribe_status", serde_json::json!(app.editor.status.clone()));
            let scene = app.editor.doc.scene().clone();
            let imgs = vec![
                render(&scene, 10, 1.0, &out.join("transcribe_f10.png")),
                render(&scene, 90, 1.0, &out.join("transcribe_f90.png")),
            ];
            strip(&imgs, 300, &out.join("transcribe_strip.png"));
        }

        // ---------------- The Models section, with this backend chosen
        if run("models") {
            let mut app = App::new(GpuPreference::Automatic);
            app.recovery.found.clear();
            set_backend(&mut app, &backend);
            app.editor.workspace.reveal(buzz_ui::PanelId::Ai);
            if backend == "claude" {
                app.editor.ai_models.test_claude_connection();
            }
            app.ai_panel.focus = Some(0);
            let ctx = egui::Context::default();
            buzz_ui::theme::apply(&ctx);
            for _ in 0..40 {
                frame(&mut app, &ctx);
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = crate::ui_snapshot::write_ui_snapshot(
                &mut app,
                crate::ui_snapshot::SnapshotConfig { width: 1600, height: 1000, scale: 1.0 },
                &out.join("panel_models.png"),
            );
        }

        std::fs::write(out.join("timings.json"), serde_json::to_string_pretty(&timings).unwrap()).unwrap();
    }
}

/// E4/E5 (`docs/BIMARI5_LAG_PLAN.md`, S5 revised): the encode thread's hand-over
/// with the render thread, driven headlessly - the same decisions `App::render`
/// makes (`Ahead::source`, `Ahead::start`), without a window or a GPU.
#[cfg(test)]
mod encode_ahead_tests {
    use super::*;
    use std::sync::Arc;

    fn rect_shape(id: u64, r: kurbo::Rect, rgb: (u8, u8, u8)) -> Arc<buzz_scene::Object> {
        Arc::new(buzz_scene::Object::shape(
            buzz_scene::ObjectId(id),
            buzz_scene::ShapeData::filled(
                buzz_geom::Shape::to_path(&r, 1e-9),
                Color::from_rgb8(rgb.0, rgb.1, rgb.2),
            ),
        ))
    }

    /// A plain, animated 12-frame document.
    fn animated_app() -> App {
        let mut scene = buzz_scene::Scene::default();
        scene.stage_mut().background = Color::WHITE;
        scene.stage_mut().size = Size::new(400.0, 300.0);
        let layer = scene.add_layer("Art", buzz_scene::LayerKind::Normal);
        scene.update_layer(layer, |l| {
            l.frames
                .push_object(0, rect_shape(1, kurbo::Rect::new(10.0, 10.0, 60.0, 60.0), (200, 40, 40)));
            l.frames.insert_keyframe(4);
            l.frames
                .push_object(4, rect_shape(2, kurbo::Rect::new(80.0, 10.0, 160.0, 90.0), (40, 40, 200)));
            l.frames.insert_keyframe(8);
            l.frames
                .push_object(8, rect_shape(3, kurbo::Rect::new(200.0, 20.0, 300.0, 120.0), (40, 160, 40)));
            l.frames.insert_frame(11);
        });
        let mut app = App::new(GpuPreference::Automatic);
        app.editor = Editor::new(Document::new(scene));
        app.editor.camera.viewport = Size::new(400.0, 300.0);
        app.editor.camera.center = buzz_geom::Point::new(200.0, 150.0);
        app.editor.camera.zoom = 1.0;
        app.editor.playback.playing = true;
        app.editor.playback.looping = true;
        app
    }

    fn area() -> buzz_geom::Rect {
        buzz_geom::Rect::new(0.0, 0.0, 400.0, 300.0)
    }

    /// The stamp `App::render` builds for the frame now showing.
    fn stamp_of(app: &App) -> StageStamp {
        let c = &app.editor.camera;
        let a = area();
        StageStamp {
            revision: app.editor.scene().revision(),
            frame: app.editor.current_frame,
            camera: [
                c.center.x.to_bits(),
                c.center.y.to_bits(),
                c.zoom.to_bits(),
                c.rotation.to_bits(),
            ],
            area: [a.x0.to_bits(), a.y0.to_bits(), a.x1.to_bits(), a.y1.to_bits()],
            edit_path: Vec::new(),
            onion: (false, false, 0, 0),
            edit_multiple: false,
            lights_generation: app.lights_generation,
            lights: 0,
            painted_preview: false,
        }
    }

    /// The render thread's normal path for the frame now showing, then the
    /// hand-over `render` makes after it.
    fn draw_normally_then_start(app: &mut App) {
        let mut vello = VelloScene::new();
        stage::build_scene(&mut vello, &app.editor, area(), 1.0, &mut app.lights);
        app.stage_stamp = Some(stamp_of(app));
        let stamp = app.stage_stamp.clone();
        app.ahead().start(stamp.as_ref(), area(), 1.0, true);
    }

    fn take(app: &mut App, frame: u32) -> PipeSource {
        app.editor.current_frame = frame;
        app.pipeline.playhead(frame);
        let stamp = stamp_of(app);
        app.ahead().source(&stamp, 1.0, true, Duration::from_secs(10))
    }

    #[test]
    fn a_playing_plain_stage_is_drawn_ahead_with_the_cache_lent_out() {
        let mut app = animated_app();
        draw_normally_then_start(&mut app);
        assert!(app.pipeline.is_running(), "a plain stage that plays is drawn ahead");
        // The real cache is on the thread: this one is the stand-in, which has
        // judged no frame.
        assert_eq!(app.lights.last_lines(), 0);

        for f in 1..=10u32 {
            match take(&mut app, f) {
                PipeSource::Ready(p) => {
                    assert_eq!(p.frame, f);
                    assert_eq!(p.epoch.revision, app.editor.scene().revision());
                    app.pipeline.recycle(p.scene);
                }
                PipeSource::Normal => panic!("frame {f}: the pipeline stopped under a plain play"),
                PipeSource::Shown => panic!("frame {f}: reads as already shown"),
                PipeSource::NotReady => panic!("frame {f}: not ready in 10 s"),
            }
        }
        // The lent cache's judgements reach the render thread through the
        // pipeline, not through the stand-in.
        assert!(light_last_lines_of(&app.pipeline, &app.lights) > 0);
        assert_eq!(app.lights.last_lines(), 0);
        assert_eq!(app.pipeline.stats.not_ready, 0);
        app.stop_encode_ahead();
    }

    #[test]
    fn an_edit_during_play_shows_no_old_epoch_frame() {
        let mut app = animated_app();
        draw_normally_then_start(&mut app);
        let before = app.editor.scene().revision();
        for f in 1..=4u32 {
            if let PipeSource::Ready(p) = take(&mut app, f) {
                assert_eq!(p.epoch.revision, before);
                app.pipeline.recycle(p.scene);
            } else {
                panic!("frame {f} not ready");
            }
        }

        // An edit lands while the thread has frames drawn ahead under the old
        // revision: the next frame's source must not be one of them.
        app.editor
            .doc
            .edit("test", |s| s.stage_mut().background = Color::BLACK);
        let after = app.editor.scene().revision();
        assert_ne!(before, after);
        match take(&mut app, 5) {
            PipeSource::Normal => {}
            PipeSource::Ready(p) => panic!("an old-epoch frame {} was handed over", p.frame),
            _ => panic!("the pipeline must stop and hand the frame to the render thread"),
        }
        assert!(!app.pipeline.is_running(), "the cache is home");
        assert!(app.lights.last_lines() > 0, "and it is the real one, not the stand-in");

        // Drawn on the render thread under the new revision, then ahead again:
        // every frame handed over is of the new epoch.
        draw_normally_then_start(&mut app);
        assert!(app.pipeline.is_running());
        for f in 6..=11u32 {
            match take(&mut app, f) {
                PipeSource::Ready(p) => {
                    assert_eq!(p.epoch.revision, after, "frame {f}");
                    app.pipeline.recycle(p.scene);
                }
                _ => panic!("frame {f}: expected a prepared frame"),
            }
        }
        app.stop_encode_ahead();
    }

    #[test]
    fn stopping_playback_takes_the_cache_home() {
        let mut app = animated_app();
        draw_normally_then_start(&mut app);
        assert!(matches!(take(&mut app, 1), PipeSource::Ready(_)));
        app.editor.playback.playing = false;
        let started = Instant::now();
        assert!(matches!(take(&mut app, 1), PipeSource::Normal));
        assert!(
            started.elapsed() < Duration::from_millis(10),
            "the cache must be back within 10 ms of the stop"
        );
        assert!(!app.pipeline.is_running());
        assert!(app.lights.last_lines() > 0, "the real cache is back");
    }

    #[test]
    fn what_may_be_drawn_ahead() {
        let mut app = animated_app();
        assert!(app.ahead().can(true), "a plain stage that plays");
        app.editor.playback.playing = false;
        assert!(!app.ahead().can(true), "not while stopped");
        app.editor.playback.playing = true;
        app.stage_bands = 16;
        assert!(!app.ahead().can(true), "not while the stage is drawn in bands");
        app.stage_bands = 0;
        assert!(app.ahead().can(true));
    }

    #[test]
    fn opening_a_document_while_drawing_ahead_takes_the_real_cache_home_first() {
        let mut app = animated_app();
        app.lights.set_scale(2);
        app.lights.set_max_scale(4);
        draw_normally_then_start(&mut app);
        assert!(app.pipeline.is_running());

        let (_old, _view) =
            app.activate_document(Document::new(buzz_scene::Scene::default()), Default::default());

        assert!(!app.pipeline.is_running());
        assert_eq!(app.lights.max_scale(), 4);
        assert_eq!(app.lights.scale(), 2);
    }
}

/// S9: the warm-up at document open, driven headlessly through `tick_warm`
/// (what `render` calls at the top of every frame): it starts once the camera
/// has settled, shows as a Tasks panel entry with progress, installs the
/// film's stamps and sizes the cache once, re-warms when the zoom settles
/// somewhere new, and is cancelled by a document switch.
#[cfg(test)]
mod warm_open_tests {
    use super::*;
    use std::sync::Arc;

    fn shape(id: u64, r: kurbo::Rect, rgb: (u8, u8, u8)) -> Arc<buzz_scene::Object> {
        Arc::new(buzz_scene::Object::shape(
            buzz_scene::ObjectId(id),
            buzz_scene::ShapeData::filled(
                buzz_geom::Shape::to_path(&r, 1e-9),
                Color::from_rgb8(rgb.0, rgb.1, rgb.2),
            ),
        ))
    }

    /// A 60-frame film of animated characters made of an animated part.
    fn film_app() -> App {
        use buzz_scene::{LayerKind, Object, ObjectId, SymbolKind};
        let mut scene = buzz_scene::Scene::default();
        scene.stage_mut().background = Color::WHITE;
        scene.stage_mut().size = Size::new(400.0, 300.0);
        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        scene.library_mut().update(part, |s| {
            let inner = s.layers.iter().next().map(|l| l.id).expect("a layer");
            s.layers.update(inner, |l| {
                l.frames.push_object(0, shape(1, kurbo::Rect::new(0.0, 0.0, 12.0, 12.0), (200, 60, 60)));
                l.frames.insert_keyframe(3);
                l.frames.push_object(3, shape(2, kurbo::Rect::new(14.0, 0.0, 30.0, 12.0), (60, 60, 200)));
            });
        });
        let character = scene.add_symbol("character", SymbolKind::Graphic, None);
        scene.library_mut().update(character, |s| {
            let inner = s.layers.iter().next().map(|l| l.id).expect("a layer");
            s.layers.update(inner, |l| {
                for i in 0..3u64 {
                    l.frames.push_object(
                        0,
                        Arc::new(
                            Object::instance_of(ObjectId(100 + i), part)
                                .with_transform(buzz_geom::Affine::translate((0.0, i as f64 * 16.0))),
                        ),
                    );
                }
                l.frames.insert_keyframe(5);
                l.frames.push_object(5, shape(3, kurbo::Rect::new(40.0, 0.0, 52.0, 48.0), (40, 160, 40)));
            });
        });
        let layer = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..6 {
            scene.add_instance_at(
                layer,
                0,
                character,
                buzz_geom::Affine::translate((20.0 + i as f64 * 60.0, 40.0 + (i % 2) as f64 * 100.0)),
            );
        }
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(59);
        });
        let mut app = App::new(GpuPreference::Automatic);
        let mut doc = Document::new(scene);
        // As an opened file is: clean.
        doc.mark_clean();
        app.editor = Editor::new(doc);
        app.editor.camera.viewport = Size::new(400.0, 300.0);
        app.editor.camera.center = buzz_geom::Point::new(200.0, 150.0);
        app.editor.camera.zoom = 1.0;
        app.lights.set_max_scale(8);
        app.warm.on = true;
        app.warm.needs = true;
        app
    }

    fn area() -> buzz_geom::Rect {
        buzz_geom::Rect::new(0.0, 0.0, 400.0, 300.0)
    }

    /// Tick until `done` holds, or fail after a minute.
    fn tick_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
        let t = Instant::now();
        while !done(app) {
            assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for: {what}");
            app.tick_warm(area(), 1.0);
            app.poll_tasks();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_document_is_warmed_after_the_camera_settles_behind_a_progress_task() {
        let mut app = film_app();
        assert!(!app.editor.doc.is_dirty(), "an opened document is clean");
        // The camera has not been still long enough: nothing starts.
        app.tick_warm(area(), 1.0);
        assert!(app.warm.running.is_none() && app.tasks.is_empty());
        assert!(app.warm.busy(), "the settle timer wants the next frame");

        tick_until(&mut app, "a warm-up to start", |a| a.warm.running.is_some());
        let task_listed = app
            .tasks
            .running()
            .any(|t| t.kind == crate::tasks::TaskKind::Warm);
        assert!(task_listed, "the warm-up is a task in the Tasks panel");
        assert_eq!(crate::tasks::TaskKind::Warm.label(), "Prepare");

        tick_until(&mut app, "the warm-up to finish", |a| a.warm.last.is_some());
        let report = app.warm.last.clone().unwrap();
        assert_eq!(report.frames_total, 60);
        assert_eq!(report.frames_covered, 60, "every frame is warm");
        assert!(report.stamp_keys > 0 && app.lights.stamp_count() == report.stamp_keys);
        assert!(app.lights.stamp_budget() >= report.budget_bytes);
        assert!(!app.warm.needs && app.warm.running.is_none());
        assert!(
            app.editor.status.as_deref().is_some_and(|s| s.starts_with("Prepared all 60 frames")),
            "status: {:?}",
            app.editor.status
        );

        // Playing the film afterwards builds nothing and draws nothing live first.
        let (builds, first) =
            (app.lights.symbol_scenes.builds, app.lights.symbol_scenes.first_sightings);
        for f in 0..60 {
            app.editor.current_frame = f;
            let mut scene = VelloScene::new();
            stage::build_scene(&mut scene, &app.editor, area(), 1.0, &mut app.lights);
        }
        assert_eq!(app.lights.symbol_scenes.builds, builds);
        assert_eq!(app.lights.symbol_scenes.first_sightings, first);
        assert_eq!(app.lights.stamp_count(), report.stamp_keys, "nothing evicted");

        // Warmed once: another tick starts nothing.
        app.tick_warm(area(), 1.0);
        app.tick_warm(area(), 1.0);
        assert!(app.warm.running.is_none() && !app.warm.busy());
    }

    #[test]
    fn a_zoom_that_settles_somewhere_new_is_warmed_again_and_a_moving_zoom_is_not() {
        let mut app = film_app();
        tick_until(&mut app, "the open warm-up", |a| a.warm.last.is_some());
        let first = app.lights.stamp_count();
        assert!(first > 0);

        // A zoom gesture: the level moves every tick, so nothing starts.
        for k in 1..=5 {
            app.editor.camera.zoom = 1.0 + 0.1 * f64::from(k);
            app.tick_warm(area(), 1.0);
            assert!(app.warm.running.is_none(), "not while the zoom is still moving");
        }
        // It settles: a re-warm for the new level starts and finishes.
        tick_until(&mut app, "the zoom re-warm", |a| a.warm.zoom_warms == 1);
        assert!(app.warm.last_zoom_warm_s.is_some(), "its cost is recorded");
        assert!(app.warm.warmed_level.is_some_and(|l| crate::warm_open::same_level(l, 1.5)));
        // Stamps for the old zoom were dropped, the new ones are all hits.
        let (builds, first_seen) =
            (app.lights.symbol_scenes.builds, app.lights.symbol_scenes.first_sightings);
        for f in 0..60 {
            app.editor.current_frame = f;
            let mut scene = VelloScene::new();
            stage::build_scene(&mut scene, &app.editor, area(), 1.0, &mut app.lights);
        }
        assert_eq!(app.lights.symbol_scenes.builds, builds, "the new zoom is warm");
        assert_eq!(app.lights.symbol_scenes.first_sightings, first_seen);
    }

    #[test]
    fn switching_documents_cancels_the_warm_up_and_prepares_the_next_one() {
        let mut app = film_app();
        tick_until(&mut app, "a warm-up to start", |a| a.warm.running.is_some());
        let before = app.warm.activation;
        let (_old, _view) =
            app.activate_document(Document::new(buzz_scene::Scene::default()), Default::default());
        assert_eq!(app.warm.activation, before + 1);
        assert!(app.warm.superseded, "the app stopped it, not the user");
        assert!(app.warm.needs && app.warm.last.is_none());
        tick_until(&mut app, "the old warm-up to be gone", |a| a.warm.running.is_none());
        assert!(!app.warm.declined, "a switch is not a refusal");
        assert!(app.lights.stamp_count() == 0, "the old film's stamps never reached the new cache");
    }

    #[test]
    fn stopping_the_warm_up_from_the_tasks_panel_declines_it_for_this_document() {
        let mut app = film_app();
        tick_until(&mut app, "a warm-up to start", |a| a.warm.running.is_some());
        let id = app.warm.running.as_ref().unwrap().task;
        app.tasks.cancel(id);
        tick_until(&mut app, "it to be gone", |a| a.warm.running.is_none());
        // It may have finished before the cancel was seen; either way it is not
        // started again for this document unless it completed.
        if app.warm.last.is_none() {
            assert!(app.warm.declined);
            for _ in 0..5 {
                app.tick_warm(area(), 1.0);
            }
            assert!(app.warm.running.is_none());
        }
    }
}

#[cfg(test)]
mod every_menu_command_tests {
    use super::*;

    /// Left out, each because it reaches outside the program: a native file
    /// picker on the user's screen, a background AI job, a second OS window,
    /// or a write into the user's own templates and snapshots. Everything
    /// else a menu can raise is run.
    const REACHES_OUTSIDE: &[Command] = &[
        Command::Open,
        Command::Save,
        Command::SaveAs,
        Command::SaveAsTemplate,
        Command::SaveSnapshot,
        Command::TestMovie,
        Command::ImportSound,
        Command::SetMasterAudio,
        Command::ImportCaptions,
        Command::ExportCaptions,
        Command::ImportVideoReference,
        Command::ImportSequenceFolder,
        Command::ImportImage,
        Command::ImportToStage,
        Command::ImportToLibrary,
        Command::ExportFla,
        Command::RunScript,
        Command::AiSceneFromImage,
        Command::AiDirectStory,
        Command::AiRigVectorCharacter,
        // These ask the machine's own BuzzEdit and Buzzcaf Studio
        // (`crate::estate`, whose tests use fake ones).
        Command::SendToBuzzEdit,
        Command::TranscribeWithBuzzEdit,
        Command::ImportScriptFromStudio,
    ];

    /// The whole menu catalogue, plus one of every data-carrying command the
    /// menus build on the fly — every panel, theme, size, preset, tool and
    /// camera move.
    fn every_command() -> Vec<Command> {
        let mut all = buzz_ui::command::palette_commands();
        all.extend(buzz_ui::PanelId::ALL.map(Command::TogglePanel));
        all.extend(buzz_ui::theme::Theme::ALL.map(Command::SetTheme));
        all.extend([100, 125, 150, 175, 200].map(Command::SetUiScale));
        all.extend(buzz_ui::workspace::WorkspacePreset::ALL.map(Command::SetWorkspacePreset));
        all.extend(buzz_ui::tools::all_tools().into_iter().map(Command::SelectTool));
        all.extend(buzz_ui::align::Distribute::ALL.map(Command::Distribute));
        all.extend(buzz_ui::align::MatchSize::ALL.map(Command::MatchSize));
        all.extend(
            [
                buzz_scene::CameraMove::PushIn,
                buzz_scene::CameraMove::PullOut,
                buzz_scene::CameraMove::PanLeft,
                buzz_scene::CameraMove::PanRight,
                buzz_scene::CameraMove::Reveal,
                buzz_scene::CameraMove::Drift,
            ]
            .map(Command::AddCameraMove),
        );
        all.retain(|c| !REACHES_OUTSIDE.contains(c));
        all.dedup();
        all
    }

    /// A headless window: one egui context kept for the whole walk, because a
    /// fresh one rebuilds the font atlas — seconds per command in a debug
    /// build, which turned a two-minute walk into an hour.
    struct Window {
        ctx: egui::Context,
        app: App,
    }

    impl Window {
        fn new() -> Self {
            let ctx = egui::Context::default();
            buzz_ui::theme::apply(&ctx);
            let mut app = App::new(GpuPreference::Automatic);
            app.recovery.found.clear();
            Self { ctx, app }
        }

        fn draw_twice(&mut self) {
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1600.0, 900.0),
                )),
                ..Default::default()
            };
            for _ in 0..2 {
                let _ = self.ctx.run_ui(input(), |ui| {
                    self.app.build_ui(ui);
                });
            }
        }

        /// A new document, as File ▸ New would leave it: the artwork fixture,
        /// or the empty page the program opens on.
        fn open(&mut self, with_art: bool) {
            self.app.editor = if with_art {
                Editor::new(crate::ui_snapshot::fixture_document())
            } else {
                let mut editor = Editor::default();
                let size = editor.workspace.new_document;
                editor.create_document(size);
                editor.status = None;
                editor
            };
        }
    }

    fn message(panic: Box<dyn std::any::Any + Send>) -> String {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    }

    /// **Every menu command runs, and the window draws after it.**
    ///
    /// Each command is run twice, on a fresh document each time: once with
    /// artwork and everything selected, and once on an empty page with
    /// nothing selected — the two states a menu item is most often chosen
    /// from, and the second the one least often tried by hand. A panic is
    /// collected rather than stopping the walk, so one run names every bad
    /// command, and the window is rebuilt after one so the next command
    /// starts clean.
    ///
    /// `forget_crash_snapshot` around each command: `App::new` installs the
    /// crash guard, and a panic caught here must not write a "recovered"
    /// document into the user's own recovery folder.
    #[test]
    fn every_menu_command_runs_and_the_window_still_draws() {
        let commands = every_command();
        assert!(commands.len() > 150, "only {} commands", commands.len());

        let mut window = Window::new();
        let mut failures = Vec::new();
        for command in &commands {
            for with_art in [true, false] {
                eprintln!("TMPMENU {command:?} art={with_art} t={:?}", std::time::Instant::now());
                buzz_doc::autosave::forget_crash_snapshot();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    window.open(with_art);
                    window.draw_twice();
                    if with_art {
                        window.app.dispatch(Command::SelectAll);
                    }
                    window.app.dispatch(*command);
                    buzz_doc::autosave::forget_crash_snapshot();
                    window.draw_twice();
                }));
                if let Err(panic) = outcome {
                    let on = if with_art { "artwork" } else { "empty" };
                    failures.push(format!("{command:?} on {on}: {}", message(panic)));
                    window = Window::new();
                }
            }
        }
        buzz_doc::autosave::forget_crash_snapshot();
        assert!(failures.is_empty(), "commands that crashed:\n{}", failures.join("\n"));
    }

    /// **The Sequence and Shot menus, in the order a film is made with them.**
    ///
    /// Their items carry the setup or shot they act on, so they cannot be run
    /// cold like the rest: this makes a setup, stamps a shot from it, and then
    /// runs every Shot-menu item on that shot, drawing the window after each,
    /// exactly as `docs/SEQUENCE_GUIDE.md` walks through it.
    #[test]
    fn the_sequence_and_shot_menus_run_end_to_end() {
        let mut window = Window::new();
        window.open(true);
        window.draw_twice();

        window.app.dispatch(Command::MakeSetupFromScene);
        window.draw_twice();
        let setups = window.app.editor.sequence_menu_info().setups;
        let (setup, _) = *setups.first().expect("Make Setup made a setup");

        window.app.dispatch(Command::NewShotFromSetup(setup));
        window.draw_twice();
        let shot = window
            .app
            .editor
            .sequence_menu_info()
            .active_shot
            .expect("New Shot from Setup opens the new shot");

        let steps: Vec<Box<dyn Fn(&App) -> Option<Command>>> = vec![
            Box::new(|_| Some(Command::NextFrame)),
            Box::new(|_| Some(Command::SplitAtPlayhead)),
            Box::new(move |_| Some(Command::DuplicateShot(shot))),
            Box::new(move |_| Some(Command::TryAnotherSetup(shot, setup))),
            Box::new(move |_| Some(Command::AddMissingSetupLayers(shot))),
            Box::new(move |app| {
                let info = app.editor.sequence_menu_info();
                let now = info.active_shot?;
                let earlier = info.continue_from_choices.first()?.0;
                Some(Command::ContinueFromShot(now, earlier))
            }),
            Box::new(move |_| Some(Command::CutOnDialogue(setup))),
            Box::new(|_| Some(Command::FlattenToMasterScene)),
            Box::new(move |_| Some(Command::EditSetup(setup))),
        ];
        for step in steps {
            if let Some(command) = step(&window.app) {
                window.app.dispatch(command);
                window.draw_twice();
            }
        }
    }
}

/// **Joint limits through the app** (docs/RIG_CONSTRAINTS_AND_LAYER_RIG_PLAN.md
/// R2): the edits the joint widget raises, applied the way the Rig panel and
/// Tool Options apply them, with a real UI frame drawn over the result. No
/// clicks — the widget's own output is built the way the widget builds it.
#[cfg(test)]
mod joint_limit_tests {
    use super::*;
    use buzz_ui::joint_widget::{JointEdit, limits_about, spec_of};

    /// A rig of four bones: a root, a two-bone limb hanging off it, and a
    /// sibling of the limb — so a chain has something beside it to leave alone.
    fn seed_rig(app: &mut App) -> buzz_scene::ObjectId {
        use buzz_geom::Shape as _;
        let mut id = buzz_scene::ObjectId(90_000);
        app.editor.doc.edit("Seed", |scene| {
            let layer = scene.layers().iter().next().expect("a default layer").id;
            id = scene.next_object_id();
            let shape = buzz_scene::Object::shape(
                id,
                buzz_scene::ShapeData::filled(
                    buzz_geom::Rect::new(0.0, -60.0, 260.0, 80.0).to_path(1e-9),
                    Color::WHITE,
                ),
            );
            scene.edit_layers().update(layer, |l| {
                l.frames.set_objects(0, vec![Arc::new(shape)]);
            });
        });
        let p = buzz_geom::Point::new;
        app.editor.doc.edit("Rig", |scene| {
            assert!(rigging::rig_object(scene, 0, id, p(0.0, 20.0), p(100.0, 20.0)));
            rigging::add_bone(scene, 0, id, Some(0), p(100.0, 20.0), p(180.0, 60.0));
            rigging::add_bone(scene, 0, id, Some(1), p(180.0, 60.0), p(240.0, 20.0));
            rigging::add_bone(scene, 0, id, Some(0), p(100.0, 20.0), p(140.0, -40.0));
        });
        app.editor.selection.set([id]);
        id
    }

    fn bones(app: &App, rig: buzz_scene::ObjectId) -> Vec<buzz_rig::Bone> {
        let (_, object) = app.editor.scene().find_object(rig).expect("the rig");
        match &object.kind {
            buzz_scene::ObjectKind::Armature(data) => data.armature.bones.clone(),
            other => panic!("not an armature: {other:?}"),
        }
    }

    /// One whole UI frame, so the panels and the stage's joint wedges draw
    /// over whatever the edit left behind.
    fn draw(app: &mut App) {
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });
    }

    /// Locked is Animate's "Enable rotation" off: the bone holds the angle it
    /// was drawn at, stored as a range of zero width *at the rest angle* — not
    /// at zero, which is where the old checkbox put every new limit.
    #[test]
    fn locking_a_joint_stores_min_equals_max_at_rest() {
        let mut app = App::new(GpuPreference::Automatic);
        let rig = seed_rig(&mut app);
        let before = bones(&app, rig)[1].clone();
        assert!(before.rest_angle.abs() > 0.1, "the test needs a bone drawn off zero");

        // What the widget sends when Locked is picked for bone 1.
        let mut spec = spec_of(&before);
        spec.0 = buzz_rig::RotationMode::Locked;
        let edit = JointEdit::Set {
            index: 1,
            limits: limits_about(before.rest_angle, spec),
        };
        app.apply_joint_edit(rig, edit);
        draw(&mut app);

        let after = bones(&app, rig)[1].clone();
        let limits = after.limits.expect("a locked joint has limits");
        assert!(
            (limits.min - after.rest_angle).abs() < 1e-12
                && (limits.max - after.rest_angle).abs() < 1e-12,
            "locked at {limits:?}, rest {}",
            after.rest_angle
        );
        assert_eq!(after.rotation_mode(), buzz_rig::RotationMode::Locked);

        app.editor.doc.undo();
        assert!(bones(&app, rig)[1].limits.is_none(), "locking was not one undo step");
    }

    /// Apply to chain reaches the bone and everything hanging off it — each
    /// with the same range about *its own* rest angle — and nothing else.
    #[test]
    fn apply_to_chain_reaches_descendants_only() {
        let mut app = App::new(GpuPreference::Automatic);
        let rig = seed_rig(&mut app);
        let start = bones(&app, rig);
        assert!(
            (start[1].rest_angle - start[2].rest_angle).abs() > 0.1,
            "the test needs the chain's bones drawn at different angles"
        );

        let (min, max) = ((-150.0f64).to_radians(), 0.0);
        let edit = JointEdit::ApplyToChain {
            index: 1,
            limits: Some(buzz_rig::JointLimits::from_relative(start[1].rest_angle, min, max)),
        };
        app.apply_joint_edit(rig, edit);
        draw(&mut app);

        let after = bones(&app, rig);
        assert!(after[0].limits.is_none(), "the parent of the chain was limited");
        assert!(after[3].limits.is_none(), "a sibling of the chain was limited");
        for index in [1, 2] {
            let bone = &after[index];
            let limits = bone.limits.expect("a bone of the chain has no limits");
            let (lo, hi) = limits.relative_to(bone.rest_angle);
            assert!(
                (lo - min).abs() < 1e-9 && (hi - max).abs() < 1e-9,
                "bone {index}: {lo}..{hi} about its rest, wanted {min}..{max}"
            );
            assert_eq!(
                limits.direction(bone.rest_angle),
                buzz_rig::Direction::CounterClockwise
            );
        }

        app.editor.doc.undo();
        assert!(
            bones(&app, rig).iter().all(|b| b.limits.is_none()),
            "the chain was not one undo step"
        );
    }

    /// Every piece of text one whole UI frame painted.
    fn painted_text(app: &mut App) -> Vec<String> {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });
        let mut out = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut out);
        }
        out
    }

    /// With the Bone tool in hand, pressing a bone puts that bone's joint
    /// widget in Tool Options; a selected rig with no bone pressed shows the
    /// hint instead; selecting something else forgets the bone.
    #[test]
    fn tool_options_shows_the_joint_widget_for_a_selected_bone() {
        let mut app = App::new(GpuPreference::Automatic);
        let rig = seed_rig(&mut app);
        app.editor.set_tool(buzz_ui::ToolId::Bone);
        app.editor.workspace.reveal(buzz_ui::PanelId::ToolOptions);

        // A rig selected, no bone pressed yet: the hint.
        let text = painted_text(&mut app);
        assert!(
            text.iter().any(|t| t == "Click a bone to set its limits"),
            "no hint for a rig with no bone: {text:?}"
        );
        assert!(!text.iter().any(|t| t == "Rotation"), "a widget with no bone: {text:?}");

        // Press the middle of bone 1 (100,20)→(180,60), as the Bone tool would.
        let at = app.editor.camera.doc_to_screen(buzz_geom::Point::new(140.0, 40.0));
        app.editor.pointer_down(at, Default::default());
        app.editor.pointer_up(at);
        assert_eq!(app.editor.selected_bone, Some((rig, 1)), "the press did not pick bone 1");

        let text = painted_text(&mut app);
        assert!(
            text.iter().any(|t| t == "Rotation"),
            "no joint widget for the pressed bone: {text:?}"
        );
        assert!(
            text.iter().any(|t| t.starts_with("Joint limits") && t.ends_with("Bone 2")),
            "the widget is not for bone 1 (\"Bone 2\"): {text:?}"
        );

        // An edit from it lands on that bone as one undo step.
        let bone = bones(&app, rig)[1].clone();
        let mut spec = spec_of(&bone);
        spec.0 = buzz_rig::RotationMode::Locked;
        app.apply_joint_edit(
            rig,
            JointEdit::Set {
                index: 1,
                limits: limits_about(bone.rest_angle, spec),
            },
        );
        assert_eq!(bones(&app, rig)[1].rotation_mode(), buzz_rig::RotationMode::Locked);
        app.editor.doc.undo();
        assert!(bones(&app, rig)[1].limits.is_none());

        // Selecting nothing forgets the bone.
        app.editor.selection.clear();
        let text = painted_text(&mut app);
        assert_eq!(app.editor.selected_bone, None);
        assert!(!text.iter().any(|t| t == "Rotation"), "widget with nothing selected: {text:?}");
    }

    /// BM: the stage paints the skeleton (one tapered polygon per bone, in the
    /// bone fill) only while the Bone tool is the active tool and its "Show
    /// bones" is on. Counted over one whole `build_ui` frame.
    #[test]
    fn the_stage_paints_bones_only_while_the_bone_tool_is_active() {
        fn bone_shapes(app: &mut App) -> usize {
            let fill = egui::Color32::from_rgba_unmultiplied(255, 210, 90, 60);
            fn count(shape: &egui::Shape, fill: egui::Color32) -> usize {
                match shape {
                    egui::Shape::Path(path) if path.fill == fill => 1,
                    egui::Shape::Vec(shapes) => shapes.iter().map(|s| count(s, fill)).sum(),
                    _ => 0,
                }
            }
            let ctx = egui::Context::default();
            buzz_ui::theme::apply(&ctx);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1600.0, 1000.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                app.build_ui(ui);
            });
            output.shapes.iter().map(|c| count(&c.shape, fill)).sum()
        }

        let mut app = App::new(GpuPreference::Automatic);
        let rig = seed_rig(&mut app);
        app.editor.workspace.rig = buzz_ui::RigOptions::default();

        app.editor.set_tool(buzz_ui::ToolId::Bone);
        assert_eq!(bone_shapes(&mut app), bones(&app, rig).len(), "the Bone tool shows every bone");
        app.editor.workspace.rig.mode = buzz_ui::BoneMode::Create;
        assert_eq!(bone_shapes(&mut app), bones(&app, rig).len(), "in either mode");

        for tool in [buzz_ui::ToolId::Selection, buzz_ui::ToolId::FreeTransform, buzz_ui::ToolId::Brush] {
            app.editor.set_tool(tool);
            assert_eq!(bone_shapes(&mut app), 0, "{tool:?} painted bones");
        }

        app.editor.set_tool(buzz_ui::ToolId::Bone);
        app.editor.workspace.rig.show_bones = false;
        assert_eq!(bone_shapes(&mut app), 0, "hidden bones were painted");
    }

    /// **Default limits** offers one row per slot of the pattern on screen, in
    /// slot order, read from the saved table the Rig panel loads when drawn.
    #[test]
    fn default_limits_table_lists_every_slot_of_the_pattern() {
        let mut app = App::new(GpuPreference::Automatic);
        app.editor.workspace.reveal(buzz_ui::PanelId::Rig);
        draw(&mut app);
        draw(&mut app);
        let table = app
            .editor
            .rig_panel
            .default_limits
            .clone()
            .expect("the Rig panel was drawn and loaded the saved table");

        for pattern in buzz_rig::RigPattern::builtin() {
            let rows = buzz_ui::rig_panel::default_limit_rows(&pattern, &table);
            let names: Vec<&str> = rows.iter().map(|(name, _)| name.as_str()).collect();
            let slots: Vec<&str> = pattern.slots.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(names, slots, "{}: the table's rows are not its slots", pattern.name);
        }

        // The built-in rows come through as they are: a knee bends one way.
        let rows = buzz_ui::rig_panel::default_limit_rows(
            &buzz_rig::RigPattern::biped_full(),
            &buzz_rig::JointLimitDefaults::anatomical(),
        );
        let knee = rows.iter().find(|(name, _)| name == "Knee L").expect("a left knee");
        assert_eq!(knee.1.0, buzz_rig::RotationMode::Limited);
        assert_eq!(knee.1.1, buzz_rig::Direction::CounterClockwise);
    }
}

/// L2 of `docs/RIG_CONSTRAINTS_AND_LAYER_RIG_PLAN.md`: the Rig panel's kind
/// radio, the Convert buttons and the Free Transform hint, headless.
#[cfg(test)]
mod layer_rig_ui_tests {
    use super::*;
    use buzz_geom::Shape as _;

    const LIMB: Color = Color::from_rgb8(0xC8, 0x8A, 0x5A);

    /// A chest, a left upper arm and a left forearm as bars on one layer, in
    /// the Biped's slots.
    fn three_parts(app: &mut App) -> Vec<Option<buzz_scene::ObjectId>> {
        let pattern = buzz_rig::RigPattern::biped();
        let mut layer = None;
        app.editor.doc.edit("Layer", |scene| {
            layer = Some(scene.add_layer("Character", buzz_scene::LayerKind::Normal));
        });
        let layer = layer.expect("a layer");
        let mut slots = vec![None; pattern.slots.len()];
        for (slot, bar) in [
            ("Chest", buzz_geom::Rect::new(200.0, 0.0, 260.0, 120.0)),
            ("Shoulder L", buzz_geom::Rect::new(140.0, 0.0, 200.0, 20.0)),
            ("Elbow L", buzz_geom::Rect::new(80.0, 0.0, 140.0, 20.0)),
        ] {
            let mut id = None;
            app.editor.doc.edit("Draw", |scene| {
                id = scene.add_shape(layer, buzz_scene::ShapeData::filled(bar.to_path(1e-9), LIMB));
            });
            slots[pattern.slot_named(slot).expect("a biped slot")] = id;
        }
        slots
    }

    fn armatures(app: &App) -> Vec<(buzz_scene::ObjectId, Vec<buzz_rig::Bone>)> {
        let frame = app.editor.frame();
        app.editor
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(frame).iter())
            .filter_map(|o| match &o.kind {
                buzz_scene::ObjectKind::Armature(data) => Some((o.id, data.armature.bones.clone())),
                _ => None,
            })
            .collect()
    }

    /// `(name, follows-name)` of every layer that follows or is followed.
    fn links(app: &App) -> Vec<(String, Option<String>)> {
        let layers = app.editor.scene().layers();
        let name = |id: buzz_scene::LayerId| layers.get(id).map(|l| l.name.clone());
        let mut out: Vec<(String, Option<String>)> = layers
            .iter()
            .filter(|l| l.follows.is_some() || !layers.followers_of(l.id).is_empty())
            .map(|l| (l.name.clone(), l.follows.and_then(name)))
            .collect();
        out.sort();
        out
    }

    fn painted_text(app: &mut App) -> Vec<String> {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            app.build_ui(ui);
        });
        let mut out = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut out);
        }
        out
    }

    fn depth(app: &App) -> usize {
        app.editor.doc.history().undo_depth()
    }

    /// The radio decides what **Rig Character** builds: an armature with
    /// Bones, three followed layers and no armature with Layers — each one
    /// undo step.
    #[test]
    fn rig_kind_radio_changes_what_build_produces() {
        // Bones (picked: Layers is the default since LR2).
        let mut app = App::new(GpuPreference::Automatic);
        let slots = three_parts(&mut app);
        assert_eq!(app.editor.rig_panel.kind, buzz_rig::RigKind::Layers);
        app.editor.rig_panel.kind = buzz_rig::RigKind::Bones;
        let before = depth(&app);
        app.rig_character("Biped", &slots);
        assert_eq!(depth(&app), before + 1);
        assert_eq!(armatures(&app).len(), 1, "Bones did not build an armature");
        assert!(links(&app).is_empty(), "Bones made layer links: {:?}", links(&app));

        // Layers: the radio is on screen with its hint.
        let mut app = App::new(GpuPreference::Automatic);
        let slots = three_parts(&mut app);
        app.editor.workspace.reveal(buzz_ui::PanelId::Rig);
        app.editor.rig_panel.slots = slots.clone();
        app.editor.rig_panel.kind = buzz_rig::RigKind::Bones;
        let text = painted_text(&mut app);
        assert!(text.iter().any(|t| t == "Rig kind:"), "no kind radio: {text:?}");
        assert!(!text.iter().any(|t| t == buzz_rig::RigKind::LAYERS_HINT));
        app.editor.rig_panel.kind = buzz_rig::RigKind::Layers;
        let text = painted_text(&mut app);
        assert!(
            text.iter().any(|t| t == buzz_rig::RigKind::LAYERS_HINT),
            "no layer-rig hint: {text:?}"
        );

        let before = depth(&app);
        app.rig_character("Biped", &slots);
        assert_eq!(depth(&app), before + 1, "a layer rig is not one undo step");
        assert!(armatures(&app).is_empty(), "Layers built an armature");
        assert_eq!(
            links(&app),
            vec![
                ("Chest".to_string(), None),
                ("Elbow L".to_string(), Some("Shoulder L".to_string())),
                ("Shoulder L".to_string(), Some("Chest".to_string())),
            ]
        );
        app.editor.doc.undo();
        assert!(links(&app).is_empty(), "undo left layer links");
        assert_eq!(app.editor.scene().shape_count(), 3);
    }

    /// Bones → Layers → Bones from the Rig panel's Convert buttons: one undo
    /// step each, and each undoes cleanly back to what was there.
    #[test]
    fn converting_bones_to_layers_and_back_is_one_undo_step_each() {
        let mut app = App::new(GpuPreference::Automatic);
        let slots = three_parts(&mut app);
        app.editor.rig_panel.kind = buzz_rig::RigKind::Bones;
        app.rig_character("Biped", &slots);
        let (rig, bones) = armatures(&app).pop().expect("a bone rig");
        app.editor.workspace.reveal(buzz_ui::PanelId::Rig);
        app.editor.selection.set([rig]);
        let text = painted_text(&mut app);
        assert!(text.iter().any(|t| t == "Convert to layer rig"), "no Convert button: {text:?}");
        assert!(!text.iter().any(|t| t == "Convert to bone rig"));

        // Bones → Layers.
        let start = depth(&app);
        app.convert_to_layer_rig(rig);
        assert_eq!(depth(&app), start + 1, "Convert to layer rig is not one undo step");
        assert!(armatures(&app).is_empty(), "the armature is still there");
        let layered = links(&app);
        assert_eq!(layered.len(), 3, "{layered:?}");
        let layers = app.selected_layer_rig().expect("the selection is a layer rig");
        assert_eq!(layers.len(), 3);
        let text = painted_text(&mut app);
        assert!(text.iter().any(|t| t == "Convert to bone rig"), "no Convert back: {text:?}");

        // Free Transform on a part that carries others says so.
        let chest = app
            .editor
            .scene()
            .layers()
            .iter()
            .find(|l| l.name == "Chest")
            .and_then(|l| l.objects_at(0).first().map(|o| o.id))
            .expect("the chest part");
        app.editor.set_tool(buzz_ui::ToolId::FreeTransform);
        app.editor.workspace.reveal(buzz_ui::PanelId::ToolOptions);
        app.editor.selection.set([chest]);
        let text = painted_text(&mut app);
        assert!(
            text.iter().any(|t| t == "Rotating this part carries the parts that follow it"),
            "no Free Transform hint: {text:?}"
        );

        // Layers → Bones.
        app.convert_to_bone_rig(&layers);
        assert_eq!(depth(&app), start + 2, "Convert to bone rig is not one undo step");
        let (_, back) = armatures(&app).pop().expect("a bone rig again");
        assert_eq!(
            back.iter().map(|b| &b.name).collect::<Vec<_>>(),
            bones.iter().map(|b| &b.name).collect::<Vec<_>>()
        );
        for (a, b) in bones.iter().zip(&back) {
            assert!((a.length - b.length).abs() < 0.5, "{}", a.name);
            assert!((a.rest_angle - b.rest_angle).abs().to_degrees() < 0.5, "{}", a.name);
        }
        assert!(links(&app).is_empty(), "the bone rig kept layer links");

        // Undo, undo: the layer rig, then the original bone rig.
        assert!(app.editor.doc.undo());
        assert!(armatures(&app).is_empty());
        assert_eq!(links(&app), layered, "undo did not bring the layer rig back intact");
        assert!(app.editor.doc.undo());
        assert!(links(&app).is_empty());
        let (id, again) = armatures(&app).pop().expect("the first bone rig");
        assert_eq!(id, rig);
        assert_eq!(again, bones);
    }

    /// A skinned rig refuses: a status message and no change, no undo step.
    #[test]
    fn a_skinned_rig_refuses_to_convert() {
        let mut app = App::new(GpuPreference::Automatic);
        let mut id = None;
        app.editor.doc.edit("Draw", |scene| {
            let layer = scene.layers().iter().next().expect("a layer").id;
            id = scene.add_shape(
                layer,
                buzz_scene::ShapeData::filled(
                    buzz_geom::Rect::new(0.0, 0.0, 200.0, 20.0).to_path(1e-9),
                    LIMB,
                ),
            );
        });
        let id = id.expect("a shape");
        app.editor.doc.edit("Rig", |scene| {
            assert!(rigging::rig_object(
                scene,
                0,
                id,
                buzz_geom::Point::new(0.0, 10.0),
                buzz_geom::Point::new(200.0, 10.0),
            ));
        });
        let start = depth(&app);
        app.convert_to_layer_rig(id);
        assert_eq!(depth(&app), start, "a refused conversion left an undo step");
        assert_eq!(armatures(&app).len(), 1);
        assert!(
            app.editor.status.as_deref().is_some_and(|s| s.contains("skinned")),
            "{:?}",
            app.editor.status
        );
    }

    /// **L3**: a Cast folder holding one bone rig and one layer rig — the
    /// scan reports each kind, and the Cast panel's rows say "bones" and
    /// "layers" in a full `build_ui` frame.
    #[test]
    fn the_cast_panel_row_shows_each_characters_rig_kind() {
        let dir = tempfile::tempdir().expect("temp asset folder");
        let mut library = buzz_doc::AssetLibrary::at(dir.path());

        let mut boned = App::new(GpuPreference::Automatic);
        let slots = three_parts(&mut boned);
        boned.editor.rig_panel.kind = buzz_rig::RigKind::Bones;
        boned.rig_character("Biped", &slots);
        assert_eq!(armatures(&boned).len(), 1);
        library.save("Boned", "Cast", boned.editor.scene()).expect("saved");

        let mut layered = App::new(GpuPreference::Automatic);
        let slots = three_parts(&mut layered);
        let pattern = buzz_rig::RigPattern::biped();
        layered.editor.doc.edit("Rig with Layers", |scene| {
            crate::rigging::rig_character_layers(scene, 0, &pattern, &slots).expect("a layer rig");
        });
        library.save("Layered", "Cast", layered.editor.scene()).expect("saved");

        let library = buzz_doc::AssetLibrary::at(dir.path());
        let kind = |name: &str| {
            library
                .assets()
                .iter()
                .find(|a| a.name == name)
                .and_then(|a| a.rig_kind)
        };
        assert_eq!(kind("Boned"), Some(buzz_rig::RigKind::Bones));
        assert_eq!(kind("Layered"), Some(buzz_rig::RigKind::Layers));

        let mut app = App::new(GpuPreference::Automatic);
        app.editor.assets = library;
        app.editor.workspace.reveal(buzz_ui::PanelId::Cast);
        let text = painted_text(&mut app);
        assert!(text.iter().any(|t| t == "Boned"), "no Cast row: {text:?}");
        assert!(text.iter().any(|t| t == "bones"), "no bones label: {text:?}");
        assert!(text.iter().any(|t| t == "layers"), "no layers label: {text:?}");
    }

    /// LR2 changed only what *new* characters get: a bone rig saved to Cast
    /// before is still a bone rig when an app with the Layers default scans
    /// and loads it.
    #[test]
    fn a_saved_bone_rig_cast_asset_still_loads_as_bones() {
        let dir = tempfile::tempdir().expect("temp asset folder");
        let mut library = buzz_doc::AssetLibrary::at(dir.path());
        let mut boned = App::new(GpuPreference::Automatic);
        let slots = three_parts(&mut boned);
        boned.editor.rig_panel.kind = buzz_rig::RigKind::Bones;
        boned.rig_character("Biped", &slots);
        library.save("Old Rig", "Cast", boned.editor.scene()).expect("saved");

        let mut app = App::new(GpuPreference::Automatic);
        assert_eq!(app.editor.rig_panel.kind, buzz_rig::RigKind::Layers);
        app.editor.assets = buzz_doc::AssetLibrary::at(dir.path());
        let asset = app
            .editor
            .assets
            .assets()
            .iter()
            .find(|a| a.name == "Old Rig")
            .cloned()
            .expect("the Cast asset");
        assert_eq!(asset.rig_kind, Some(buzz_rig::RigKind::Bones));
        let saved = app.editor.assets.load(&asset).expect("the Cast asset loads");
        assert_eq!(buzz_doc::rig_kind_of(&saved), Some(buzz_rig::RigKind::Bones));
        assert!(saved.layers().iter().all(|l| l.follows.is_none()), "a bone rig grew layer links");
    }
}

#[cfg(test)]
mod symbol_set_tests {
    use super::*;
    use buzz_scene::{ObjectId, Scene, SymbolId, SymbolKind};

    /// A 30-frame stage with the set `idle` (10 frames) + `wave` (4) + `sit`
    /// (20) and an instance of `idle` placed on frame 0.
    fn set_scene() -> (Scene, buzz_scene::LayerId, ObjectId, [SymbolId; 3]) {
        let mut scene = Scene::default();
        scene.set_frame_count(30);
        let layer = scene.layers().iter().next().unwrap().id;
        let mut ids = [SymbolId(0); 3];
        for (slot, (name, length)) in [("idle", 10), ("wave", 4), ("sit", 20)].into_iter().enumerate() {
            let id = scene.add_symbol(name, SymbolKind::Graphic, None);
            let inner = scene.library().get(id).unwrap().layers.iter().next().unwrap().id;
            scene.library_mut().update(id, |s| {
                s.layers.update(inner, |l| {
                    while l.frames.length() < length {
                        l.frames.insert_frame(l.frames.length());
                    }
                });
            });
            ids[slot] = id;
        }
        assert!(scene.library_mut().include_in_set(ids[0], ids[1]));
        assert!(scene.library_mut().include_in_set(ids[0], ids[2]));
        let id = scene
            .add_instance_at(layer, 0, ids[0], buzz_geom::Affine::IDENTITY)
            .unwrap();
        (scene, layer, id, ids)
    }

    fn symbol_on(app: &App, layer: buzz_scene::LayerId, frame: u32, id: ObjectId) -> SymbolId {
        app.editor
            .doc
            .scene()
            .layers()
            .get(layer)
            .unwrap()
            .frames
            .objects_at(frame)
            .iter()
            .find(|o| o.id == id)
            .and_then(|o| o.instance())
            .expect("the instance is on that frame")
            .symbol
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        let modifiers = egui::Modifiers::default();
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            },
        ]
    }

    /// **Picking from the Set dropdown in Properties, through the real app.**
    /// An instance of `idle` is selected with the playhead on frame 6; the
    /// Set menu is opened and `wave` clicked. Frame 6 on shows `wave`, frame 5
    /// still `idle`, and one undo puts frame 6 back to `idle`.
    #[test]
    fn picking_a_set_member_in_properties_keys_it_at_the_playhead_and_undoes() {
        use egui::accesskit::{Action, Role};

        let (scene, layer, id, [idle, wave, _]) = set_scene();
        let mut app = App::new(GpuPreference::Automatic);
        app.editor = Editor::new(Document::new(scene));
        app.editor.workspace = buzz_ui::Workspace::essentials();
        app.recovery.found.clear();
        app.editor.set_frame(6);
        app.editor.selection.set([id]);

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        ctx.enable_accesskit();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 2400.0));
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.build_ui(ui);
                },
            )
        };
        let tree = |output: &egui::FullOutput| {
            output
                .platform_output
                .accesskit_update
                .clone()
                .expect("accesskit is on, so every frame has a tree")
        };
        let centre = |b: egui::accesskit::Rect| {
            egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
        };

        let _ = frame(&mut app, Vec::new());
        let closed = frame(&mut app, Vec::new());
        let closed_tree = tree(&closed);
        let combos: Vec<_> = closed_tree
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::ComboBox && n.value() == Some("idle"))
            .filter_map(|(_, n)| n.bounds())
            .collect();
        assert_eq!(combos.len(), 1, "exactly one Set dropdown shows idle");
        let _ = frame(&mut app, click(centre(combos[0])));
        // A popup spends its first frame measuring itself.
        let opened = frame(&mut app, Vec::new());
        let before: std::collections::HashSet<_> =
            closed_tree.nodes.iter().map(|(id, _)| *id).collect();
        let items: Vec<_> = tree(&opened)
            .nodes
            .iter()
            .filter(|(nid, n)| {
                !before.contains(nid)
                    && n.supports_action(Action::Click)
                    && n.label() == Some("wave")
            })
            .filter_map(|(_, n)| n.bounds())
            .collect();
        assert_eq!(items.len(), 1, "the open Set menu offers wave once");
        let _ = frame(&mut app, click(centre(items[0])));
        let _ = frame(&mut app, Vec::new());

        assert_eq!(symbol_on(&app, layer, 6, id), wave, "the playhead shows the pick");
        assert_eq!(symbol_on(&app, layer, 29, id), wave, "and every frame after it");
        assert_eq!(symbol_on(&app, layer, 5, id), idle, "earlier frames keep idle");

        assert!(app.editor.doc.undo(), "the pick is an undo step");
        assert_eq!(symbol_on(&app, layer, 6, id), idle, "undo restores idle");
        assert_eq!(
            app.editor.doc.scene().layers().get(layer).unwrap().keyframe_count(),
            1,
            "and removes the keyframe the pick made"
        );
    }

    /// **Any other instance edit after a pick keeps the pick.** The panel used
    /// to read the earliest copy of the instance (still `idle`) and write all
    /// its fields to the keyframe under the playhead, so ticking a box on the
    /// frame where `wave` was picked put `idle` back.
    #[test]
    fn editing_an_instance_after_a_pick_does_not_undo_the_pick() {
        use egui::accesskit::Role;

        let (mut scene, layer, id, [idle, wave, _]) = set_scene();
        assert!(scene.pick_from_set(6, id, wave, buzz_scene::SetStart::SameFrame));
        let mut app = App::new(GpuPreference::Automatic);
        app.editor = Editor::new(Document::new(scene));
        app.editor.workspace = buzz_ui::Workspace::essentials();
        app.recovery.found.clear();
        app.editor.set_frame(6);
        app.editor.selection.set([id]);

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);
        ctx.enable_accesskit();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 2400.0));
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.build_ui(ui);
                },
            )
        };

        let _ = frame(&mut app, Vec::new());
        let shown = frame(&mut app, Vec::new());
        let tree = shown.platform_output.accesskit_update.expect("accesskit is on");
        assert!(
            tree.nodes
                .iter()
                .any(|(_, n)| n.role() == Role::ComboBox && n.value() == Some("wave")),
            "the Set dropdown names the symbol at the playhead"
        );
        let boxes: Vec<_> = tree
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("Match the stage frame"))
            .filter_map(|(_, n)| n.bounds())
            .collect();
        assert_eq!(boxes.len(), 1, "one Sync to root checkbox");
        let b = boxes[0];
        let _ = frame(
            &mut app,
            click(egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)),
        );
        let _ = frame(&mut app, Vec::new());

        let synced = |frame: u32| {
            app.editor.doc.scene().layers().get(layer).unwrap().frames.objects_at(frame)
                .iter()
                .find(|o| o.id == id)
                .and_then(|o| o.instance())
                .unwrap()
                .sync_to_root
        };
        assert!(synced(6), "the tick landed on the playhead's keyframe");
        assert_eq!(symbol_on(&app, layer, 6, id), wave, "and the pick survived it");
        assert_eq!(symbol_on(&app, layer, 5, id), idle);
    }
}

/// **Move Frames by dragging** — `docs/TIMELINE_FRAME_MOVE_PLAN.md` M3/M5/M6,
/// driven headlessly: one `egui::Context`, pointer events fed through
/// `App::build_ui`, cells found from the rows' own recorded geometry.
#[cfg(test)]
mod frame_move_drag_tests {
    use super::*;
    use buzz_scene::{FrameKind, LayerId};

    struct Drive {
        app: App,
        ctx: egui::Context,
    }

    impl Drive {
        fn new() -> Self {
            let mut app = App::new(GpuPreference::Automatic);
            app.recovery.found.clear();
            let ctx = egui::Context::default();
            buzz_ui::theme::apply(&ctx);
            ctx.enable_accesskit();
            let mut drive = Self { app, ctx };
            drive.frame(vec![]);
            drive.frame(vec![]);
            drive
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                ..Default::default()
            };
            let app = &mut self.app;
            let _ = self.ctx.run_ui(input, |ui| {
                app.build_ui(ui);
            });
        }

        /// The centre of `frame`'s cell on `layer`'s row, as last drawn.
        fn cell(&self, layer: LayerId, frame: u32) -> egui::Pos2 {
            let (rect, grid_left, cell_width) = self
                .ctx
                .data(|d| {
                    d.get_temp::<(egui::Rect, f32, f32)>(egui::Id::new((
                        buzz_ui::timeline_panel::ROW_GEOMETRY,
                        layer,
                    )))
                })
                .expect("the layer's row was drawn");
            egui::pos2(grid_left + (frame as f32 + 0.5) * cell_width, rect.center().y)
        }

        /// Press at `from`, carry the pointer to `to` over several frames,
        /// let go — a hand drag, through egui's own drag threshold.
        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            self.frame(vec![egui::Event::PointerMoved(from), button(from, true)]);
            for step in 1..=8 {
                let t = step as f32 / 8.0;
                self.frame(vec![egui::Event::PointerMoved(from + (to - from) * t)]);
            }
            self.frame(vec![button(to, false)]);
            self.frame(vec![]);
        }

        fn layer(&self, id: LayerId) -> buzz_scene::Layer {
            (**self.app.editor.doc.scene().layers().get(id).unwrap()).clone()
        }
    }

    /// Layers `length` frames long, each with one drawing on a keyframe at
    /// frame 2, added to the app's document. Returned top row first.
    fn layers_with_a_drawing_at_2(drive: &mut Drive, count: usize, length: u32) -> Vec<LayerId> {
        let mut made = Vec::new();
        drive.app.editor.doc.edit("Setup", |s| {
            for i in 0..count {
                let layer = s.add_layer(format!("Drawn {i}"), buzz_scene::LayerKind::Normal);
                s.update_layer(layer, |l| {
                    l.frames.insert_frame(length - 1);
                    l.frames.insert_blank_keyframe(2);
                });
                let shape = buzz_scene::ShapeData::filled(
                    kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, 40.0, 40.0), 0.1),
                    peniko::Color::WHITE,
                );
                s.add_shape_at(layer, 2, shape);
                made.push(layer);
            }
        });
        let order: Vec<LayerId> =
            drive.app.editor.doc.scene().layers().iter().map(|l| l.id).collect();
        made.sort_by_key(|id| order.iter().position(|o| o == id));
        drive.frame(vec![]);
        drive.frame(vec![]);
        made
    }

    /// Select frames 2–4 on a layer, press inside the selection, drag three
    /// cells right, let go: frames 5–7 hold the drawing and frame 2 is a
    /// blank keyframe.
    #[test]
    fn dragging_a_frame_selection_right_moves_the_frames() {
        let mut drive = Drive::new();
        let layer = layers_with_a_drawing_at_2(&mut drive, 1, 6)[0];
        drive
            .app
            .editor
            .selection
            .set_timeline(buzz_ui::TimelineSelection::new([layer], 2, 4, layer, 2));
        drive.frame(vec![]);

        let (from, to) = (drive.cell(layer, 3), drive.cell(layer, 6));
        drive.drag(from, to);

        let moved = drive.layer(layer);
        assert_eq!(moved.frames.frame_kind(2), FrameKind::BlankKeyframe, "the source is blank");
        for frame in 5..=7 {
            assert_eq!(moved.objects_at(frame).len(), 1, "frame {frame} holds the drawing");
        }
        let selection = drive.app.editor.selection.timeline().unwrap();
        assert_eq!((selection.first_frame, selection.last_frame), (5, 7));
    }

    /// A press **outside** the selection still sweeps a new selection and
    /// moves nothing.
    #[test]
    fn a_drag_outside_the_selection_still_selects() {
        let mut drive = Drive::new();
        let layer = layers_with_a_drawing_at_2(&mut drive, 1, 10)[0];
        drive
            .app
            .editor
            .selection
            .set_timeline(buzz_ui::TimelineSelection::new([layer], 2, 4, layer, 2));
        drive.frame(vec![]);
        let before = drive.layer(layer).frames.clone();

        let (from, to) = (drive.cell(layer, 6), drive.cell(layer, 8));
        drive.drag(from, to);

        assert_eq!(drive.layer(layer).frames, before, "nothing moved");
        let selection = drive.app.editor.selection.timeline().unwrap();
        // The sweep starts where egui decided it was a drag, which can be a
        // cell past the press (the selection rectangle reads the pointer then,
        // not the press) — today's behaviour, kept as it was.
        assert!(
            (6..=7).contains(&selection.first_frame) && selection.last_frame == 8,
            "a new selection: {selection:?}"
        );
    }

    /// M6: a selection dragged down one row and right two cells lands on the
    /// next layer.
    #[test]
    fn dragging_a_selection_down_a_row_moves_it_onto_the_next_layer() {
        let mut drive = Drive::new();
        let rows = layers_with_a_drawing_at_2(&mut drive, 2, 8);
        let (top, below) = (rows[0], rows[1]);
        drive
            .app
            .editor
            .selection
            .set_timeline(buzz_ui::TimelineSelection::new([top], 2, 3, top, 2));
        drive.frame(vec![]);
        assert!(
            drive.cell(top, 2).y < drive.cell(below, 2).y,
            "the first layer is the upper row"
        );

        let (from, to) = (drive.cell(top, 2), drive.cell(below, 4));
        drive.drag(from, to);

        assert_eq!(drive.layer(top).frames.frame_kind(2), FrameKind::BlankKeyframe);
        let target = drive.layer(below);
        assert_eq!(target.frames.frame_kind(4), FrameKind::Keyframe);
        assert_eq!(target.objects_at(4).len(), 1);
        assert_eq!(target.objects_at(5).len(), 1);
        let selection = drive.app.editor.selection.timeline().unwrap();
        assert!(selection.layers.contains(&below));
        assert_eq!(selection.first_frame, 4);
    }
}
