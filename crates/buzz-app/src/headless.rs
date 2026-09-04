//! **A brief in at midnight, an mp4 out at breakfast.**
//!
//! # What this is for
//!
//! Everything needed to make a film without a person had been built and tested
//! separately — the director, the staging, the scenery, the performances, the
//! reel, the encoders — and there was no way to reach any of it without opening
//! a window and clicking. That is the difference between a program that *helps
//! you animate* and one you can hand a brief to and walk away from, and closing
//! it is almost entirely wiring: nothing here is new machinery.
//!
//! ```text
//! buzzanimate --brief story.txt --render out.mp4
//! buzzanimate film.buzz --render out.mp4 --height 1080
//! ```
//!
//! # It opens no window, and it says so on the way
//!
//! The GUI is never constructed. That matters for more than tidiness: an
//! overnight render on a machine nobody is sitting at must not be waiting on an
//! event loop, and a render that failed must exit non-zero so whatever
//! scheduled it can tell.
//!
//! Progress goes to stderr rather than a panel, because that is where a person
//! looks when a terminal has been running for an hour.
//!
//! # It reuses the export the window uses
//!
//! [`crate::export_service::run_export`] is the same call the Tasks panel makes,
//! with the same `ExportRequest`. There is no second encoder path and no second
//! set of settings to drift: what comes out of an overnight render is what would
//! have come out of the dialog.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use buzz_doc::Document;
use buzz_export::{ExportSettings, PresetFormat, VideoSettings};
use buzz_render::GpuPreference;

use crate::export_service::{ExportRequest, ExportTarget, run_export};
use crate::tasks::{ProgressSink, TaskCtx, TaskOutcome};
use buzz_jobs::CancelToken;

/// What to render, and where to put it.
#[derive(Debug, Clone)]
pub struct RenderJob {
    /// A `.buzz` (or any importable) document to open, if there is one.
    pub document: Option<PathBuf>,
    /// A file of prose to direct into a film, if there is one.
    ///
    /// Both may be given: the brief is directed *into* the opened document,
    /// which is how a scripted pipeline adds shots to a set somebody built by
    /// hand.
    pub brief: Option<PathBuf>,
    /// Where the film goes. The extension chooses the format.
    pub output: PathBuf,
    /// Target height in pixels; the width follows the document's aspect.
    /// `None` keeps the stage's own size.
    pub height: Option<u32>,
    pub gpu: GpuPreference,
}

/// **Run a render job to completion**, with no window and no event loop.
///
/// Returns what happened, for the caller to print. Errors are for the things
/// that mean there is no film to make at all — a missing brief, an
/// unrecognisable output format — rather than for a frame that came out wrong.
pub fn render(job: &RenderJob) -> Result<String> {
    let mut doc = match &job.document {
        Some(path) => {
            let (scenes, _) = buzz_doc::format::load_scenes(path)
                .with_context(|| format!("opening {}", path.display()))?;
            Document::from_scenes(scenes)
        }
        None => Document::default(),
    };

    // **The brief, directed into whatever is open.** `direct_sequence` lives on
    // the editor because it needs the document's scene list; the editor needs
    // no window, so it is built here and thrown away.
    let mut directed = 0usize;
    if let Some(path) = &job.brief {
        let prose = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        if prose.trim().is_empty() {
            bail!("{} is empty", path.display());
        }
        let mut editor = crate::editor::Editor::new(doc);
        directed = editor.direct_sequence(&prose);
        if directed == 0 {
            bail!(
                "nothing in {} could be directed{}",
                path.display(),
                editor
                    .status
                    .as_deref()
                    .map(|s| format!(" \u{2014} {s}"))
                    .unwrap_or_default()
            );
        }
        doc = editor.doc;
    }

    if job.document.is_none() && job.brief.is_none() {
        bail!("nothing to render: give a document, a brief, or both");
    }

    // Every scene, in the order they play — the same snapshots the Tasks panel
    // would have taken.
    let scenes: Vec<buzz_scene::Scene> = doc.film();
    let Some(lead) = scenes.first() else {
        bail!("that document has no scenes in it");
    };
    let frames = buzz_export::Reel::of(scenes.iter()).frames();
    if frames == 0 {
        bail!("that film is zero frames long");
    }

    let format = format_for(&job.output)?;
    let settings = sized(lead, job.height);
    let target = target_for(format, &job.output)?;

    let label = job
        .output
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "film".to_string());

    eprintln!(
        "Rendering {} scene(s), {frames} frames, {}x{} \u{2192} {}",
        scenes.len(),
        settings.width,
        settings.height,
        job.output.display()
    );

    // The same call the Tasks panel makes. A cancel token nobody will cancel,
    // and a progress sink that prints instead of drawing a bar.
    let ctx = TaskCtx {
        cancel: CancelToken::new(),
        progress: ProgressSink::detached(),
    };
    let request = ExportRequest {
        scenes,
        settings,
        range: 0..frames,
        target,
        gpu: job.gpu.clone(),
        label,
    };

    match run_export(request, &ctx) {
        TaskOutcome::Finished(message) => Ok(match directed {
            0 => message,
            1 => format!("Directed one shot. {message}"),
            n => format!("Directed {n} shots. {message}"),
        }),
        TaskOutcome::Failed(why) => bail!("{why}"),
        TaskOutcome::Cancelled => bail!("the render was cancelled"),
    }
}

/// The format an output path asks for, by its extension.
///
/// By extension rather than by a flag, because the file name already says it
/// and a `--format mp4` that disagreed with `out.gif` would be a trap.
fn format_for(path: &Path) -> Result<PresetFormat> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    Ok(match ext.as_str() {
        "mp4" => PresetFormat::Mp4H264,
        "mov" => PresetFormat::MovHevc,
        "gif" => PresetFormat::Gif,
        "webp" => PresetFormat::Webp,
        "png" => PresetFormat::Png,
        "" => bail!("give the output a file extension so the format is clear"),
        other => bail!("nothing renders to .{other} \u{2014} try mp4, mov, gif, webp or png"),
    })
}

/// The stage's own size, or scaled to a target height keeping its aspect.
fn sized(lead: &buzz_scene::Scene, height: Option<u32>) -> ExportSettings {
    let mut settings = ExportSettings::for_stage(lead);
    let Some(want) = height.filter(|h| *h > 0) else {
        return settings;
    };
    let aspect = settings.width as f64 / settings.height.max(1) as f64;
    // Even, because H.264 and HEVC refuse odd dimensions — the same rounding
    // `ExportPreset::resolve_size` does, and for the same reason.
    let even = |v: u32| v + (v % 2);
    settings.height = even(want);
    settings.width = even((want as f64 * aspect).round().max(2.0) as u32);
    settings
}

fn target_for(format: PresetFormat, path: &Path) -> Result<ExportTarget> {
    Ok(match format {
        PresetFormat::Mp4H264
        | PresetFormat::Mp4Hevc
        | PresetFormat::Mp4Av1
        | PresetFormat::MovHevc => ExportTarget::Video {
            path: path.to_path_buf(),
            video: VideoSettings {
                codec: match format {
                    PresetFormat::Mp4Av1 => buzz_export::VideoCodec::Av1,
                    PresetFormat::Mp4Hevc | PresetFormat::MovHevc => {
                        buzz_export::VideoCodec::Hevc
                    }
                    _ => buzz_export::VideoCodec::H264,
                },
                ..VideoSettings::default()
            },
        },
        PresetFormat::Gif => ExportTarget::Gif {
            path: path.to_path_buf(),
            gif: buzz_export::GifSettings::default(),
        },
        PresetFormat::Webp => ExportTarget::Webp {
            path: path.to_path_buf(),
            webp: buzz_export::WebpSettings::default(),
        },
        // A still is the first frame: a `--render out.png` on a film is a
        // contact card for it, which is a thing people want and a thing that
        // would otherwise need the window.
        PresetFormat::Png => ExportTarget::Image {
            frame: 0,
            path: path.to_path_buf(),
        },
        PresetFormat::PngSequence => ExportTarget::Sequence {
            directory: path.to_path_buf(),
            base_name: "frame".into(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_picks_the_format() {
        assert!(matches!(
            format_for(Path::new("a.mp4")).unwrap(),
            PresetFormat::Mp4H264
        ));
        assert!(matches!(
            format_for(Path::new("a.MOV")).unwrap(),
            PresetFormat::MovHevc
        ));
        assert!(matches!(format_for(Path::new("a.gif")).unwrap(), PresetFormat::Gif));
        assert!(format_for(Path::new("a.txt")).is_err());
        assert!(format_for(Path::new("nameonly")).is_err());
    }

    /// **A target height keeps the aspect and comes out even**, because the
    /// video encoders refuse odd dimensions and a render that failed at the
    /// last step of an overnight job is the worst possible time to find out.
    #[test]
    fn a_target_height_keeps_the_aspect_and_stays_even() {
        let mut scene = buzz_scene::Scene::default();
        scene.stage_mut().size = buzz_geom::Size::new(1600.0, 900.0);

        let full = sized(&scene, None);
        assert_eq!((full.width, full.height), (1600, 900));

        let small = sized(&scene, Some(721));
        assert_eq!(small.height % 2, 0, "odd height: {}", small.height);
        assert_eq!(small.width % 2, 0, "odd width: {}", small.width);
        let aspect = small.width as f64 / small.height as f64;
        assert!((aspect - 16.0 / 9.0).abs() < 0.02, "aspect drifted to {aspect}");
    }

    /// **Nothing in, a reason out.** A job with neither a document nor a brief
    /// is a mistake worth naming rather than an empty film.
    #[test]
    fn a_job_with_nothing_to_render_says_so() {
        let job = RenderJob {
            document: None,
            brief: None,
            output: PathBuf::from("out.mp4"),
            height: None,
            gpu: GpuPreference::Automatic,
        };
        let err = render(&job).expect_err("nothing to render");
        assert!(err.to_string().contains("nothing to render"), "{err}");
    }

    /// **An empty brief says so**, rather than rendering a blank film.
    #[test]
    fn an_empty_brief_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let brief = dir.path().join("empty.txt");
        std::fs::write(&brief, "   \n\n").expect("write");
        let job = RenderJob {
            document: None,
            brief: Some(brief),
            output: dir.path().join("out.mp4"),
            height: None,
            gpu: GpuPreference::Automatic,
        };
        let err = render(&job).expect_err("an empty brief");
        assert!(err.to_string().contains("empty"), "{err}");
    }
}
