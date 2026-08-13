//! Running an export without freezing the window.
//!
//! # Why a thread and its own GPU
//!
//! A 500-frame sequence is minutes of work. Done on the UI thread it would
//! stop the window dead, with no progress, no cancel and no way to tell a slow
//! export from a hung one — and the user cannot even move the window to see
//! whether their disk is filling.
//!
//! So the export runs on its own thread with **its own GPU context**. Sharing
//! the window's would mean the export and the editor taking turns on one
//! device, which is exactly the stall being avoided; a second context costs a
//! few hundred milliseconds once and nothing thereafter. The document is a
//! copy-on-write snapshot, so handing it over is cheap and the user can keep
//! editing while the frames they asked for are written.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use buzz_export::ExportSettings;
use buzz_render::GpuPreference;
use buzz_scene::Scene;
use crossbeam_channel::{Receiver, unbounded};

/// What the exporting thread reports back.
#[derive(Debug, Clone)]
pub enum ExportEvent {
    /// Frames finished, frames wanted.
    Progress(u32, u32),
    /// Finished, with a line for the status bar.
    Finished(String),
    /// Failed, with a line for the status bar.
    Failed(String),
}

/// An export in flight.
pub struct ExportJob {
    events: Receiver<ExportEvent>,
    cancel: Arc<AtomicBool>,
    /// Frames done and wanted, as last reported.
    pub progress: (u32, u32),
}

impl ExportJob {
    /// Start writing one PNG.
    pub fn image(
        scene: Scene,
        frame: u32,
        path: PathBuf,
        settings: ExportSettings,
        gpu: GpuPreference,
    ) -> Self {
        Self::spawn(1, move |events, _cancel| {
            let mut exporter = buzz_export::Exporter::new(&gpu)?;
            let rendered = exporter.render(&scene, frame, &settings)?;
            rendered.write_png(&path)?;
            let _ = events.send(ExportEvent::Progress(1, 1));
            Ok(format!(
                "Exported {} ({} x {})",
                file_name(&path),
                rendered.width,
                rendered.height
            ))
        })
    }

    /// Start writing a numbered PNG per frame.
    pub fn sequence(
        scene: Scene,
        frames: std::ops::Range<u32>,
        directory: PathBuf,
        base_name: String,
        settings: ExportSettings,
        gpu: GpuPreference,
    ) -> Self {
        let total = frames.len() as u32;
        Self::spawn(total, move |events, cancel| {
            let report = buzz_export::export_sequence(
                &scene,
                frames,
                &directory,
                &base_name,
                &settings,
                &gpu,
                |done, total| {
                    let _ = events.send(ExportEvent::Progress(done, total));
                    !cancel.load(Ordering::Relaxed)
                },
            )?;

            if cancel.load(Ordering::Relaxed) {
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
        })
    }

    /// Start encoding a video.
    ///
    /// The soundtrack is written out first, into a temporary directory that
    /// lives as long as the encode: ffmpeg decodes the original files itself
    /// rather than being handed our decode of them, and a `SoundAsset` keeps
    /// the bytes exactly as they were imported for precisely this reason.
    pub fn video(
        scene: Scene,
        frames: std::ops::Range<u32>,
        path: PathBuf,
        settings: ExportSettings,
        video: buzz_export::VideoSettings,
        gpu: GpuPreference,
    ) -> Self {
        let total = frames.len() as u32;
        Self::spawn(total, move |events, cancel| {
            // Held for the whole encode; dropping it removes the files, which
            // is why it is bound rather than discarded.
            let scratch = tempfile::tempdir().context("making room for the soundtrack")?;
            let audio = if video.audio {
                soundtrack(&scene, &frames, scratch.path())?
            } else {
                Vec::new()
            };

            let report = buzz_export::export_video(
                &scene,
                frames,
                &path,
                &settings,
                &video,
                &gpu,
                &audio,
                |done, total| {
                    let _ = events.send(ExportEvent::Progress(done, total));
                    !cancel.load(Ordering::Relaxed)
                },
            )?;

            let where_encoded = if report.fell_back_to_software {
                format!(" on the CPU ({}) — no NVENC here", report.encoder)
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
        })
    }

    fn spawn(
        total: u32,
        work: impl FnOnce(
            &crossbeam_channel::Sender<ExportEvent>,
            &AtomicBool,
        ) -> anyhow::Result<String>
        + Send
        + 'static,
    ) -> Self {
        let (sender, events) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);

        std::thread::Builder::new()
            .name("buzz-export".into())
            .spawn(move || {
                let outcome = work(&sender, &thread_cancel);
                let event = match outcome {
                    Ok(message) => ExportEvent::Finished(message),
                    // The whole error chain, because "permission denied" without
                    // the path it was denied on helps nobody.
                    Err(e) => ExportEvent::Failed(format!("Export failed: {e:#}")),
                };
                let _ = sender.send(event);
            })
            .expect("spawning the export thread");

        Self {
            events,
            cancel,
            progress: (0, total),
        }
    }

    /// Ask the export to stop after the batch it is on.
    ///
    /// What has already been written stays: half a sequence is usually still
    /// worth having, and deleting the user's files because they changed their
    /// mind would be worse than leaving them.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Drain whatever the thread has said. Returns a closing message once the
    /// export has finished, at which point the job should be dropped.
    pub fn poll(&mut self) -> Option<String> {
        let mut finished = None;
        while let Ok(event) = self.events.try_recv() {
            match event {
                ExportEvent::Progress(done, total) => self.progress = (done, total),
                ExportEvent::Finished(message) | ExportEvent::Failed(message) => {
                    finished = Some(message);
                }
            }
        }
        finished
    }
}

/// Write out every sound the exported range should carry, as files ffmpeg can
/// read, with the offset each one sits at.
///
/// # Which sounds, and where they go
///
/// The document's cues, resolved through the same rules the player uses — so
/// what is in the file is what was heard while animating. Three things are
/// decided here and each of them is a real choice:
///
/// * **Offsets are measured from the start of the exported range**, not from
///   the start of the document. Exporting frames 100 onward with a sound cued
///   to frame 100 should begin with the sound, not with four seconds of
///   silence.
/// * **A cue before the range is dropped**, not clamped to zero. A line of
///   dialogue that finished before this shot began has no business in it, and
///   sliding it to the front would put it somewhere it never was. Trimming
///   into a cue that *straddles* the start is a real case and is not handled
///   — see PROGRESS.md §7.
/// * **Stop cues never arrive here at all**, because `stage_cues` drops them,
///   which is the same reason they never reach the player.
fn soundtrack(
    scene: &Scene,
    frames: &std::ops::Range<u32>,
    scratch: &std::path::Path,
) -> anyhow::Result<Vec<buzz_export::AudioTrack>> {
    let fps = scene.stage().frame_rate.max(1.0);
    let mut tracks = Vec::new();

    for cue in scene.stage_cues() {
        if cue.start_frame < frames.start || cue.start_frame >= frames.end {
            continue;
        }
        let Some(asset) = scene.sounds().get(cue.sound) else {
            continue;
        };

        // The bytes as imported, under their own name, so ffmpeg picks the
        // decoder from the extension exactly as it would for a file on disk.
        let path = scratch.join(asset.file_name());
        std::fs::write(&path, asset.data.as_slice())
            .with_context(|| format!("writing {} for the encoder", asset.name))?;

        tracks.push(buzz_export::AudioTrack {
            path,
            offset_seconds: f64::from(cue.start_frame - frames.start) / fps,
            volume: cue.volume,
        });
    }

    Ok(tracks)
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::{LayerKind, ShapeData};
    use peniko::Color;

    fn document() -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(10.0, 10.0, 100.0, 100.0).to_path(1e-9),
                Color::WHITE,
            ),
        );
        scene
    }

    /// Waits for a job to finish, with a bound so a wedged export fails the
    /// test rather than hanging the suite.
    fn finish(job: &mut ExportJob) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            if let Some(message) = job.poll() {
                return message;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "export never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Needs a GPU; skips cleanly without one, like the other render tests.
    fn gpu_available() -> bool {
        match buzz_export::Exporter::new(&GpuPreference::Automatic) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("skipping export job test: no usable GPU ({e})");
                false
            }
        }
    }

    #[test]
    fn an_image_job_writes_a_file_and_reports_it() {
        if !gpu_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("still.png");
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);

        let mut job = ExportJob::image(scene, 0, path.clone(), settings, GpuPreference::Automatic);
        let message = finish(&mut job);

        assert!(path.exists(), "the file should be there");
        assert!(message.contains("still.png"), "{message}");
        assert_eq!(job.progress, (1, 1));
    }

    /// A failure has to arrive as a message, not as a panic on a thread nobody
    /// is watching.
    #[test]
    fn a_failing_export_reports_why() {
        if !gpu_available() {
            return;
        }
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        // A directory that does not exist, inside a file that is not one.
        let path = std::path::Path::new("no-such-directory-here")
            .join("nested")
            .join("still.png");

        let mut job = ExportJob::image(scene, 0, path, settings, GpuPreference::Automatic);
        let message = finish(&mut job);

        assert!(message.starts_with("Export failed"), "{message}");
        assert!(
            message.contains("still.png"),
            "the path should be named: {message}"
        );
    }

    #[test]
    fn a_sequence_job_reports_progress_for_every_frame() {
        if !gpu_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let scene = document();
        let settings = ExportSettings::scaled(&scene, 0.25);

        let mut job = ExportJob::sequence(
            scene,
            0..3,
            dir.path().to_path_buf(),
            "shot".into(),
            settings,
            GpuPreference::Automatic,
        );
        let message = finish(&mut job);

        assert!(message.contains("3 frame"), "{message}");
        assert_eq!(job.progress, (3, 3));
        for frame in 0..3 {
            assert!(dir.path().join(format!("shot{frame:04}.png")).exists());
        }
    }
}
