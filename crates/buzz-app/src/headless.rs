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
//! buzzanimate --script film.js --audio line.mp3 --from 12 --for 15 \
//!             --save film.buzz --render film.mp4
//! ```
//!
//! # The script runs here too, and it is the reason this grew
//!
//! `--script` used to need the window: the editor built the interpreter, so a
//! script could only run against a document somebody had open. That put the
//! most capable half of the automation on the wrong side of the very door this
//! module exists to open -- a brief can say *"Ana walks in from the left"*, and
//! nothing a brief can say will rig a face, mask a sky or lip-sync a take.
//!
//! So a render job can carry a script, and the script is handed the **whole
//! film** rather than one scene (see [`buzz_script::run_film`]). It can add
//! shots, switch between them, cast, rig and lip-sync, and what it leaves is an
//! ordinary document -- which `--save` writes and `--render` encodes.
//!
//! # Sound comes in through the host, not through the script
//!
//! A script cannot open a file, and that stays true. `--audio` opens one here,
//! `--from` and `--for` take the slice that is actually wanted out of it, and
//! the decoded clip is handed to the script by index. A four-minute take and a
//! fifteen-second film is the ordinary case, not the exotic one.
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
    /// **A script to run over the whole film**, after the brief is directed
    /// and before anything is written.
    ///
    /// Last, because it is the most specific: a brief stages a shot in broad
    /// strokes and a script edits what the brief produced. Reversing them would
    /// have the director paint over the script's work.
    pub script: Option<PathBuf>,
    /// **Dialogue and music**, decoded before the run and handed to the script
    /// by index. Each may carry a slice of the file rather than all of it.
    pub audio: Vec<AudioIn>,
    /// **Where to write a contact sheet**, if one is wanted.
    ///
    /// A test render: frames spread across the film, tiled into one PNG. See
    /// [`contact_sheet`].
    pub preview: Option<PathBuf>,
    /// Where to write the `.buzz` document, if it is wanted.
    ///
    /// Separate from the render, and either may be given alone: a document with
    /// no film is a set-up to open and carry on with, and a film with no
    /// document is the ordinary overnight job.
    pub save: Option<PathBuf>,
    /// Where the film goes. The extension chooses the format.
    pub output: Option<PathBuf>,
    /// Target height in pixels; the width follows the document's aspect.
    /// `None` keeps the stage's own size.
    pub height: Option<u32>,
    pub gpu: GpuPreference,
}

/// One sound to open on the way in, and how much of it is wanted.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioIn {
    pub path: PathBuf,
    /// Where the wanted part starts, in seconds. `None` is the beginning.
    pub from: Option<f64>,
    /// How much of it is wanted, in seconds. `None` is the rest of the file.
    pub length: Option<f64>,
}

/// **Run a render job to completion**, with no window and no event loop.
///
/// Returns what happened, for the caller to print. Errors are for the things
/// that mean there is no film to make at all — a missing brief, an
/// unrecognisable output format — rather than for a frame that came out wrong.
pub fn render(job: &RenderJob) -> Result<String> {
    // **The output format is checked before any work is done.** Directing a
    // brief and running a script take real time, and finding out afterwards
    // that nothing encodes a `.psd` is the worst possible moment.
    let format = job.output.as_deref().map(format_for).transpose()?;

    let mut report = Vec::new();
    let mut doc = build(job, &mut report)?;

    if let Some(path) = &job.save {
        doc.save_as(path)
            .with_context(|| format!("saving {}", path.display()))?;
        report.push(format!("Saved {}", path.display()));
    }

    if let Some(path) = &job.preview {
        let scenes = doc.film();
        let sheet = contact_sheet(&scenes, job.height, &job.gpu)
            .with_context(|| format!("previewing into {}", path.display()))?;
        sheet
            .write_png(path)
            .with_context(|| format!("writing {}", path.display()))?;
        report.push(format!(
            "Preview: {}x{} \u{2192} {}",
            sheet.width,
            sheet.height,
            path.display()
        ));
    }

    let Some(output) = job.output.clone() else {
        // A job that only builds a document is a complete job. Refusing it
        // because no film was asked for would make `--save` useless on its own.
        return Ok(report.join(" "));
    };

    // Every scene, in the order they play -- the same snapshots the Tasks panel
    // would have taken.
    let scenes: Vec<buzz_scene::Scene> = doc.film();
    let Some(lead) = scenes.first() else {
        bail!("that document has no scenes in it");
    };
    let frames = buzz_export::Reel::of(scenes.iter()).frames();
    if frames == 0 {
        bail!("that film is zero frames long");
    }

    let format = format.expect("checked at the top, and the output is still there");
    let settings = sized(lead, job.height);
    let target = target_for(format, &output)?;

    let label = output
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "film".to_string());

    eprintln!(
        "Rendering {} scene(s), {frames} frames, {}x{} \u{2192} {}",
        scenes.len(),
        settings.width,
        settings.height,
        output.display()
    );

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
        TaskOutcome::Finished(message) => {
            report.push(message);
            Ok(report.join(" "))
        }
        TaskOutcome::Failed(why) => bail!("{why}"),
        TaskOutcome::Cancelled => bail!("the render was cancelled"),
    }
}

/// **Everything up to the document being finished**: open it, direct the brief
/// into it, run the script over it.
///
/// Split out from [`render`] because a job that only saves does exactly this
/// and stops, and because the order of the three is the part worth being able
/// to read in one place.
fn build(job: &RenderJob, report: &mut Vec<String>) -> Result<Document> {
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

    if job.document.is_none() && job.brief.is_none() && job.script.is_none() {
        bail!("nothing to do: give a document, a brief, a script, or any of them together");
    }

    match directed {
        0 => {}
        1 => report.push("Directed one shot.".to_string()),
        n => report.push(format!("Directed {n} shots.")),
    }

    // **The script, over the whole film.** See the note at the top of the file
    // on why it is handed every scene rather than one.
    if let Some(path) = &job.script {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let sounds = decode_all(&job.audio)?;
        for clip in &sounds {
            eprintln!(
                "Sound: {} ({:.2}s, {} Hz, {} channel(s))",
                clip.name,
                clip.duration_seconds(),
                clip.sample_rate,
                clip.channels
            );
        }

        let named = doc.scene_names();
        let mut film = buzz_script::Film {
            scenes: doc.film(),
            names: named,
            current: 0,
            sounds,
        };
        let context = buzz_script::ScriptContext {
            current_frame: 0,
            selection: Vec::new(),
            active_layer: None,
            config_dir: buzz_script::default_config_dir(),
            asset_root: buzz_doc::AssetLibrary::user().root().map(|p| p.to_path_buf()),
        };
        // **A generous budget, because nobody is waiting on it.** The five
        // seconds a script gets in the editor is right there -- a mistake must
        // be an annoyance rather than a hang -- and wrong here, where the script
        // is the film and the person who started it has gone to bed.
        let limits = buzz_script::Limits {
            time: std::time::Duration::from_secs(600),
            memory: 512 * 1024 * 1024,
            ..buzz_script::Limits::default()
        };
        let outcome = buzz_script::run_film(&mut film, context, &source, &limits, None);
        for line in &outcome.trace {
            eprintln!("{line}");
        }
        for line in &outcome.alerts {
            eprintln!("(asked) {line}");
        }
        if let Some(error) = &outcome.error {
            bail!("{}: {error}", path.display());
        }
        if film.scenes.is_empty() {
            bail!("{} left the film with no scenes in it", path.display());
        }
        let scenes: Vec<(String, buzz_scene::Scene)> = film
            .names
            .iter()
            .cloned()
            .zip(film.scenes.iter().cloned())
            .collect();
        doc = Document::from_scenes(scenes);
        report.push(format!(
            "Ran {} over {} scene(s).",
            path.display(),
            doc.scene_names().len()
        ));
    }

    Ok(doc)
}

/// How many frames a contact sheet holds, and how they are laid out.
///
/// Three across and two down: enough to cross a three-shot film and see a
/// camera move, and small enough that the sheet is a picture rather than a
/// wall.
const SHEET: (u32, u32) = (3, 2);

/// **A test render: frames from across the film, tiled into one picture.**
///
/// # Why this exists next to `--render`
///
/// Rendering a film to find out whether it is right costs the whole film, and
/// the mistakes it catches are the cheap ones — the wrong range, a light left
/// off, a guide layer somebody forgot to hide. Six frames cost a second and
/// catch every one of them. `--render out.png` already gives the *first* frame,
/// which is the least informative part of a film: the camera has not moved,
/// nobody has walked anywhere, and no cut has happened.
///
/// Rendered through the exporter, so what the sheet shows is what the file
/// would hold rather than what the stage would draw.
pub fn contact_sheet(
    scenes: &[buzz_scene::Scene],
    height: Option<u32>,
    gpu: &GpuPreference,
) -> Result<buzz_export::Frame> {
    let reel = buzz_export::Reel::of(scenes.iter());
    let total = reel.frames();
    let Some(lead) = reel.lead() else {
        bail!("that document has no scenes in it");
    };
    if total == 0 {
        bail!("that film is zero frames long");
    }

    // A cell of the sheet, at the film's own aspect. Deliberately small: this
    // is a check, not a delivery.
    let settings = sized(lead, Some(height.unwrap_or(1080).min(2160) / SHEET.1.max(1)));
    let mut exporter = buzz_export::Exporter::new(gpu)?;

    let (cols, rows) = SHEET;
    let cells = cols * rows;
    let (cw, ch) = (settings.width, settings.height);
    let mut sheet = buzz_export::Frame {
        width: cw * cols,
        height: ch * rows,
        pixels: vec![0u8; (cw * cols * ch * rows * 4) as usize],
    };

    let last = total - 1;
    for i in 0..cells {
        // Spread across the whole film, ends included: the first frame and the
        // last are the two most worth looking at.
        let at = if cells <= 1 {
            0
        } else {
            (last as u64 * i as u64 / (cells as u64 - 1)) as u32
        };
        let Some((scene, local)) = reel.at_clamped(at) else {
            continue;
        };
        let frame = exporter.render(scene, local, &settings)?;
        blit(&mut sheet, &frame, (i % cols) * cw, (i / cols) * ch);
    }
    Ok(sheet)
}

/// Copy `from` into `into` with its top-left corner at `(x, y)`.
///
/// Row by row rather than pixel by pixel: the rows are contiguous in both, so
/// this is six memcpys per row and not a million bounds checks.
fn blit(into: &mut buzz_export::Frame, from: &buzz_export::Frame, x: u32, y: u32) {
    for row in 0..from.height {
        let dst_y = y + row;
        if dst_y >= into.height {
            break;
        }
        let width = from.width.min(into.width.saturating_sub(x));
        if width == 0 {
            break;
        }
        let src = (row * from.width * 4) as usize;
        let dst = ((dst_y * into.width + x) * 4) as usize;
        let bytes = (width * 4) as usize;
        into.pixels[dst..dst + bytes].copy_from_slice(&from.pixels[src..src + bytes]);
    }
}

/// Open every `--audio`, taking the slice each one asked for.
fn decode_all(wanted: &[AudioIn]) -> Result<Vec<buzz_audio::Clip>> {
    wanted
        .iter()
        .map(|want| {
            let clip = buzz_audio::Clip::open(&want.path)
                .with_context(|| format!("opening {}", want.path.display()))?;
            Ok(slice(&clip, want.from, want.length))
        })
        .collect()
}

/// **A part of a clip**, cut on whole sample frames.
///
/// A dialogue take is minutes long and a shot is seconds long, so the useful
/// unit is almost never the file. Cut on a frame boundary rather than on a raw
/// sample index: cutting a stereo file mid-frame swaps the channels for the
/// rest of the clip, which is audible and baffling.
fn slice(clip: &buzz_audio::Clip, from: Option<f64>, length: Option<f64>) -> buzz_audio::Clip {
    if from.is_none() && length.is_none() {
        return clip.clone();
    }
    let channels = clip.channels.max(1) as usize;
    let rate = clip.sample_rate.max(1) as f64;
    let frames = clip.len();

    let start = ((from.unwrap_or(0.0).max(0.0) * rate).round() as usize).min(frames);
    let end = match length {
        Some(seconds) if seconds > 0.0 => {
            (start + (seconds * rate).round() as usize).min(frames)
        }
        _ => frames,
    };
    if end <= start {
        return clip.clone();
    }

    let samples = clip.samples[start * channels..end * channels].to_vec();
    let name = format!("{} {:.1}s", clip.name, (end - start) as f64 / rate);
    buzz_audio::Clip::new(&name, clip.sample_rate, clip.channels, samples)
        .unwrap_or_else(|_| clip.clone())
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
            script: None,
            audio: Vec::new(),
            save: None,
            output: Some(PathBuf::from("out.mp4")),
            height: None,
            gpu: GpuPreference::Automatic,
        };
        let err = render(&job).expect_err("nothing to do");
        assert!(err.to_string().contains("nothing to do"), "{err}");
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
            script: None,
            audio: Vec::new(),
            save: None,
            output: Some(dir.path().join("out.mp4")),
            height: None,
            gpu: GpuPreference::Automatic,
        };
        let err = render(&job).expect_err("an empty brief");
        assert!(err.to_string().contains("empty"), "{err}");
    }
}
