//! The export queue: many exports, one at a time, none of them in the way.
//!
//! # Why a queue, and why serial
//!
//! Before this an export was a single slot on `App`: starting one while another
//! ran was simply refused. That is the wrong answer for the thing exports are
//! for — rendering a batch of shots overnight — so "export while exporting" now
//! means "joins the queue".
//!
//! The queue runs **one at a time on purpose**, not as a limitation. Each export
//! builds its own second wgpu device and Vello renderer
//! (`buzz-export/src/lib.rs`), so four at once would quadruple VRAM and fight
//! NVENC's session limit to finish the same total work no sooner. Serial is the
//! correct scheduling, not a shortcut.
//!
//! # Why it lives here and runs on the task registry
//!
//! Each export is a [`crate::tasks::TaskRegistry`] thread, so it shows up in the
//! Tasks panel beside every other long job, its progress survives `File ▸ New`
//! (the bug that first motivated the registry), and quitting with one running
//! raises a prompt rather than throwing a half-written file away.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use buzz_export::{ExportSettings, GifSettings, VideoSettings, WebpSettings};
use buzz_render::GpuPreference;
use buzz_scene::Scene;

use crate::tasks::{TaskCtx, TaskId, TaskOutcome};

/// What an export writes, and where.
pub enum ExportTarget {
    /// One frame, as a PNG.
    Image { frame: u32, path: PathBuf },
    /// A numbered PNG per frame, into a folder.
    Sequence { directory: PathBuf, base_name: String },
    /// An MP4 or MOV.
    Video { path: PathBuf, video: VideoSettings },
    /// An animated GIF.
    Gif { path: PathBuf, gif: GifSettings },
    /// An animated WebP.
    Webp { path: PathBuf, webp: WebpSettings },
}

impl ExportTarget {
    /// What to open when the user asks to see the result — the file itself, or
    /// the folder a sequence filled.
    fn reveal(&self) -> PathBuf {
        match self {
            Self::Image { path, .. }
            | Self::Video { path, .. }
            | Self::Gif { path, .. }
            | Self::Webp { path, .. } => path.clone(),
            Self::Sequence { directory, .. } => directory.clone(),
        }
    }
}

/// One export, everything it needs owned outright so it can cross to its thread.
pub struct ExportRequest {
    /// **The film's scenes, in the order they play.**
    ///
    /// Copy-on-write snapshots — pointer copies, not the artwork — so the user
    /// can keep editing while it renders. A film of one scene is the ordinary
    /// case and behaves exactly as a single-scene export always did.
    pub scenes: Vec<Scene>,
    pub settings: ExportSettings,
    /// The frames to render, for everything but a single image.
    pub range: std::ops::Range<u32>,
    pub target: ExportTarget,
    pub gpu: GpuPreference,
    /// What the Tasks panel calls it — usually the file name.
    pub label: String,
}

impl ExportRequest {
    /// The file or folder to open when the finished export is revealed.
    pub fn reveal_path(&self) -> PathBuf {
        self.target.reveal()
    }
}

/// An export that has finished, for the Tasks panel to list and reveal.
#[derive(Debug, Clone)]
pub struct Finished {
    pub label: String,
    /// The file or folder to reveal.
    pub reveal: PathBuf,
    pub ok: bool,
    pub message: String,
}

/// The export currently running.
struct Active {
    task: TaskId,
    reveal: PathBuf,
    label: String,
}

/// The export queue on `App`.
#[derive(Default)]
pub struct ExportQueue {
    pending: VecDeque<ExportRequest>,
    active: Option<Active>,
    /// Most recent first, capped so a long session does not grow it without
    /// bound.
    finished: Vec<Finished>,
}

/// How many finished exports to remember.
const REMEMBERED: usize = 12;

impl ExportQueue {
    /// Add an export. It starts when whatever is ahead of it has finished.
    pub fn enqueue(&mut self, request: ExportRequest) {
        self.pending.push_back(request);
    }

    /// How many are waiting, not counting the one running.
    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Nothing to do and nothing running.
    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.pending.is_empty()
    }

    pub fn finished(&self) -> &[Finished] {
        &self.finished
    }

    /// The id of the running export, if any.
    pub fn active_task(&self) -> Option<TaskId> {
        self.active.as_ref().map(|a| a.task)
    }

    /// Take the next request to run, if the queue is free.
    ///
    /// The caller spawns it and hands the id straight back to [`Self::started`];
    /// splitting the two keeps this crate's threading in `App`, where the task
    /// registry lives.
    pub fn next_to_start(&mut self) -> Option<ExportRequest> {
        if self.active.is_some() {
            return None;
        }
        self.pending.pop_front()
    }

    /// Record the task that was spawned for the request just taken.
    pub fn started(&mut self, task: TaskId, reveal: PathBuf, label: String) {
        self.active = Some(Active {
            task,
            reveal,
            label,
        });
    }

    /// Note that the running export ended, freeing the queue for the next.
    ///
    /// Ignores a task that is not the one running: a straggling completion from
    /// a job already accounted for must not free the queue out from under the
    /// export currently in flight.
    pub fn complete(&mut self, task: TaskId, ok: bool, message: String) {
        let Some(active) = self.active.take_if(|a| a.task == task) else {
            return;
        };
        self.finished.insert(
            0,
            Finished {
                label: active.label,
                reveal: active.reveal,
                ok,
                message,
            },
        );
        self.finished.truncate(REMEMBERED);
    }

    /// Everything still to do, for the quit prompt: the running export plus the
    /// count waiting behind it.
    pub fn outstanding(&self) -> usize {
        self.active.is_some() as usize + self.pending.len()
    }
}

/// Do the export. Runs on a task thread; reports through `ctx`.
pub fn run_export(request: ExportRequest, ctx: &TaskCtx) -> TaskOutcome {
    let ExportRequest {
        scenes,
        settings,
        range,
        target,
        gpu,
        label: _,
    } = request;

    // The film: every scene end to end, each still resolving its own looping
    // section. Built here rather than by the caller because it borrows the
    // scenes, and the scenes had to cross to this thread owned.
    let reel = buzz_export::Reel::of(scenes.iter());

    let result = match target {
        ExportTarget::Image { frame, path } => run_image(&reel, frame, &path, &settings, &gpu, ctx),
        ExportTarget::Sequence {
            directory,
            base_name,
        } => run_sequence(&reel, range, &directory, &base_name, &settings, &gpu, ctx),
        ExportTarget::Video { path, video } => {
            run_video(&reel, range, &path, &settings, &video, &gpu, ctx)
        }
        ExportTarget::Gif { path, gif } => {
            let report = buzz_export::export_gif(
                &reel,
                range,
                &path,
                &settings,
                &gif,
                &gpu,
                report_progress(ctx),
            );
            report.map(|r| format!("Exported {} \u{2014} {} frames", file_name(&r.path), r.frames))
        }
        ExportTarget::Webp { path, webp } => {
            let report = buzz_export::export_webp(
                &reel,
                range,
                &path,
                &settings,
                &webp,
                &gpu,
                report_progress(ctx),
            );
            report.map(|r| format!("Exported {} \u{2014} {} frames", file_name(&r.path), r.frames))
        }
    };

    match result {
        Ok(message) => TaskOutcome::Finished(message),
        Err(e) => {
            // A cancel comes back as an error from the encoders, but it is not a
            // failure — the user asked. Told apart by the token rather than by
            // the message.
            if ctx.cancelled() {
                TaskOutcome::Cancelled
            } else {
                TaskOutcome::Failed(format!("Export failed: {e:#}"))
            }
        }
    }
}

/// A progress callback that feeds the task's sink and honours its cancel.
fn report_progress(ctx: &TaskCtx) -> impl FnMut(u32, u32) -> bool + '_ {
    move |done, total| {
        ctx.progress.set(done as u64, total as u64);
        ctx.progress.detail(format!("frame {done} of {total}"));
        !ctx.cancelled()
    }
}

fn run_image(
    reel: &buzz_export::Reel<'_>,
    frame: u32,
    path: &Path,
    settings: &ExportSettings,
    gpu: &GpuPreference,
    ctx: &TaskCtx,
) -> anyhow::Result<String> {
    // A film frame, so a still taken from the third scene is the third
    // scene — see `ExportTarget::Image`.
    let (scene, at) = reel
        .at_clamped(frame)
        .context("there are no frames to export")?;
    let mut exporter = buzz_export::Exporter::new(gpu)?;
    let rendered = exporter.render(scene, at, settings)?;
    rendered.write_png(path)?;
    ctx.progress.set(1, 1);
    Ok(format!(
        "Exported {} ({} x {})",
        file_name(path),
        rendered.width,
        rendered.height
    ))
}

fn run_sequence(
    reel: &buzz_export::Reel<'_>,
    range: std::ops::Range<u32>,
    directory: &Path,
    base_name: &str,
    settings: &ExportSettings,
    gpu: &GpuPreference,
    ctx: &TaskCtx,
) -> anyhow::Result<String> {
    let report = buzz_export::export_sequence(
        reel,
        range,
        directory,
        base_name,
        settings,
        gpu,
        report_progress(ctx),
    )?;
    if ctx.cancelled() {
        return Ok(format!(
            "Export stopped after {} frame(s); what was written was kept",
            report.frames
        ));
    }
    Ok(format!(
        "Exported {} frame(s) to {}",
        report.frames,
        directory.display()
    ))
}

fn run_video(
    reel: &buzz_export::Reel<'_>,
    range: std::ops::Range<u32>,
    path: &Path,
    settings: &ExportSettings,
    video: &VideoSettings,
    gpu: &GpuPreference,
    ctx: &TaskCtx,
) -> anyhow::Result<String> {
    // Held for the whole encode; dropping it removes the soundtrack files.
    let scratch = tempfile::tempdir().context("making room for the soundtrack")?;
    let audio = if video.audio {
        soundtrack(reel, &range, scratch.path())?
    } else {
        Vec::new()
    };

    let report = buzz_export::export_video(
        reel,
        range,
        path,
        settings,
        video,
        gpu,
        &audio,
        report_progress(ctx),
    )?;

    let where_encoded = if report.fell_back_to_software {
        format!(" on the CPU ({}) \u{2014} no NVENC here", report.encoder)
    } else if report.encoder.ends_with("_nvenc") {
        " on the GPU".to_string()
    } else {
        String::new()
    };
    let sound = match report.audio_tracks {
        0 => String::new(),
        1 => ", with sound".to_string(),
        n => format!(", with {n} sounds"),
    };
    Ok(format!(
        "Exported {} frame(s) to {}{where_encoded}{sound}",
        report.frames,
        file_name(&report.path)
    ))
}

/// Write out every sound the exported range should carry, as files ffmpeg can
/// read, with the offset each one sits at.
///
/// The document's cues, resolved through the same rules the player uses — so
/// what is in the file is what was heard while animating. Offsets are measured
/// from the start of the exported range, a cue outside the range is dropped
/// rather than clamped, and stop cues never arrive here because `stage_cues`
/// drops them. (Moved here from the retired `export_job`; the reasoning is the
/// same, and is set out at length in PROGRESS.md §4 and §7.)
///
/// # Across scenes
///
/// **A cue belongs to its scene, not to the film.** A line of dialogue on
/// frame 3 of the third shot is heard when the third shot reaches its frame 3,
/// which is a long way into the film — so every cue is carried through the
/// reel to find where its own scene lands. Getting this wrong would play the
/// whole conversation on top of the opening shot.
///
/// Two scenes can name the same sound file, so the files are written under a
/// name that includes the scene: writing both to one path would leave whichever
/// was written last standing in for both.
fn soundtrack(
    reel: &buzz_export::Reel<'_>,
    frames: &std::ops::Range<u32>,
    scratch: &Path,
) -> anyhow::Result<Vec<buzz_export::AudioTrack>> {
    let fps = reel
        .lead()
        .map(|s| s.stage().frame_rate.max(1.0))
        .unwrap_or(24.0);
    let mut tracks = Vec::new();

    for (index, (scene, _start)) in reel.scenes().enumerate() {
        for cue in scene.stage_cues() {
            // Where this scene's own frame lands in the finished film.
            let Some(at) = reel.film_frame_of(index, cue.start_frame) else {
                continue;
            };
            if at < frames.start || at >= frames.end {
                continue;
            }
            let Some(asset) = scene.sounds().get(cue.sound) else {
                continue;
            };

            let path = scratch.join(format!("{index}-{}", asset.file_name()));
            std::fs::write(&path, asset.data.as_slice())
                .with_context(|| format!("writing {} for the encoder", asset.name))?;

            tracks.push(buzz_export::AudioTrack {
                path,
                offset_seconds: f64::from(at - frames.start) / fps,
                volume: cue.volume,
            });
        }
    }

    Ok(tracks)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(label: &str) -> ExportRequest {
        ExportRequest {
            scenes: vec![Scene::default()],
            settings: ExportSettings::for_stage(&Scene::default()),
            range: 0..1,
            target: ExportTarget::Image {
                frame: 0,
                path: PathBuf::from(format!("{label}.png")),
            },
            gpu: GpuPreference::Automatic,
            label: label.to_string(),
        }
    }

    #[test]
    fn an_empty_queue_is_idle() {
        let queue = ExportQueue::default();
        assert!(queue.is_idle());
        assert_eq!(queue.outstanding(), 0);
    }

    /// The queue is serial: with one running, the next does not start until the
    /// first completes.
    #[test]
    fn only_one_runs_at_a_time() {
        let mut queue = ExportQueue::default();
        queue.enqueue(request("a"));
        queue.enqueue(request("b"));
        assert_eq!(queue.waiting(), 2);

        // Start the first.
        let first = queue.next_to_start().expect("one to start");
        assert_eq!(first.label, "a");
        queue.started(TaskId(1), PathBuf::from("a.png"), "a".into());
        assert_eq!(queue.waiting(), 1, "the second is still waiting");

        // Nothing else may start while one is active.
        assert!(queue.next_to_start().is_none(), "serial: one at a time");
        assert_eq!(queue.outstanding(), 2, "one running plus one waiting");

        // Finish the first; now the second may start.
        queue.complete(TaskId(1), true, "done".into());
        let second = queue.next_to_start().expect("the second");
        assert_eq!(second.label, "b");
    }

    #[test]
    fn completing_records_a_finished_export() {
        let mut queue = ExportQueue::default();
        queue.enqueue(request("shot"));
        let _ = queue.next_to_start();
        queue.started(TaskId(7), PathBuf::from("out/shot.png"), "shot".into());
        queue.complete(TaskId(7), true, "Exported shot.png".into());

        assert_eq!(queue.finished().len(), 1);
        assert_eq!(queue.finished()[0].label, "shot");
        assert!(queue.finished()[0].ok);
        assert!(queue.is_idle());
    }

    /// A completion for a task that is not the active one — a straggler — must
    /// not free the queue out from under the export actually running.
    #[test]
    fn a_stray_completion_is_ignored() {
        let mut queue = ExportQueue::default();
        queue.enqueue(request("a"));
        let _ = queue.next_to_start();
        queue.started(TaskId(1), PathBuf::from("a.png"), "a".into());

        queue.complete(TaskId(999), true, String::new());
        assert!(queue.is_active(), "the real export is still running");
        assert!(queue.finished().is_empty());
    }

    #[test]
    fn finished_exports_are_capped() {
        let mut queue = ExportQueue::default();
        for i in 0..(REMEMBERED + 5) {
            let id = TaskId(i as u64 + 1);
            queue.enqueue(request(&format!("s{i}")));
            let _ = queue.next_to_start();
            queue.started(id, PathBuf::from("x.png"), format!("s{i}"));
            queue.complete(id, true, String::new());
        }
        assert_eq!(queue.finished().len(), REMEMBERED);
        // Most recent first.
        assert_eq!(queue.finished()[0].label, format!("s{}", REMEMBERED + 4));
    }
}
