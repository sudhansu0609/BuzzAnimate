//! CP-6.3 — animated GIF and animated WebP.
//!
//! Built the same way as the video export ([`crate::video`]): frames are
//! rendered and piped straight into the ffmpeg already on the machine, so the
//! disk sees only the finished file and memory holds one frame at a time. What
//! differs is what ffmpeg is asked to do with them.
//!
//! # Why a GIF needs a palette, and why it is one pass
//!
//! A GIF has at most 256 colours. Left to itself ffmpeg builds that palette
//! from the *first* frame, so a character who walks into a differently coloured
//! room comes out with the wrong colours for the rest of the film. The fix is
//! to look at every frame first and choose a palette that suits the whole clip
//! — normally a two-pass job, writing a palette file and reading it back.
//!
//! It can be done in **one** pass with a split filter: the stream is duplicated,
//! one copy generates the palette while the other waits, and the second copy is
//! quantised against it. ffmpeg buffers the waiting copy itself, so the frames
//! are still piped exactly once:
//!
//! ```text
//! [0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer
//! ```
//!
//! `stats_mode=diff` weights the palette towards what *changes* between frames,
//! which is what makes a GIF of a character over a flat background spend its 256
//! colours on the character rather than on the background it shares with every
//! other frame.
//!
//! *Rejected gifski*, which dithers better: it is a second dependency with its
//! own thread pool, and ffmpeg is already shipped, already tested, and already
//! being piped to.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use buzz_render::GpuPreference;

use crate::{ExportSettings, Exporter};

/// Which animated image to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimatedFormat {
    #[default]
    Gif,
    Webp,
}

impl AnimatedFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Gif => "Animated GIF",
            Self::Webp => "Animated WebP",
        }
    }
}

/// How to dither a GIF's colours down to its palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dither {
    /// No dithering — flat, small, and prone to banding on gradients.
    None,
    /// Ordered dithering. Cheap, and it does not crawl between frames the way
    /// error-diffusion does — which matters for an animation, where a
    /// shimmering background is worse than a little visible pattern.
    #[default]
    Bayer,
    /// Floyd–Steinberg error diffusion. Smoothest on a still, but it *crawls*
    /// frame to frame, so it is offered rather than default.
    FloydSteinberg,
}

impl Dither {
    /// The `paletteuse` argument for this mode.
    fn arg(self) -> &'static str {
        match self {
            // Scale 3 is a middle setting: finer patterns show less than the
            // coarse default but hold gradients better than the finest.
            Self::Bayer => "dither=bayer:bayer_scale=3",
            Self::FloydSteinberg => "dither=floyd_steinberg",
            Self::None => "dither=none",
        }
    }
}

/// How a GIF should be built.
///
/// The defaults — ordered dithering, loop forever — are what a GIF is almost
/// always for, so [`Dither::default`] and `loops: 0` fall straight out of the
/// derive.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GifSettings {
    pub dither: Dither,
    /// How many times it loops. `0` is forever.
    pub loops: u32,
}

/// How an animated WebP should be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebpSettings {
    /// `0..=100`, higher is better. Ignored when `lossless` is set.
    pub quality: u32,
    /// Keep every pixel exactly, at the cost of size. Right for flat vector
    /// artwork, which compresses well losslessly and shows every artefact.
    pub lossless: bool,
    /// Loops, `0` for forever.
    pub loops: u32,
}

impl Default for WebpSettings {
    fn default() -> Self {
        Self {
            quality: 90,
            lossless: false,
            loops: 0,
        }
    }
}

/// What an animated-image export produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimatedReport {
    pub path: PathBuf,
    pub frames: u32,
    pub format: AnimatedFormat,
}

/// Export a range of frames as an animated GIF.
///
/// `progress` is called with frames finished and frames total; returning
/// `false` cancels, and a cancelled export removes the partial file — half a
/// GIF is no more use than half an MP4.
pub fn export_gif(
    reel: &crate::Reel<'_>,
    frames: std::ops::Range<u32>,
    path: &Path,
    settings: &ExportSettings,
    gif: &GifSettings,
    preference: &GpuPreference,
    progress: impl FnMut(u32, u32) -> bool,
) -> Result<AnimatedReport> {
    let dither = gif.dither.arg().to_string();
    let loops = gif.loops;
    encode(
        reel,
        frames,
        path,
        settings,
        preference,
        AnimatedFormat::Gif,
        progress,
        move |command, width, height, fps, output| {
            let filter = format!(
                "[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse={dither}"
            );
            feed(command, width, height, fps);
            command
                .args(["-filter_complex", &filter])
                // A GIF's clock is per-frame delay in hundredths of a second, so
                // its rate is set on the output rather than carried from the raw
                // input stream.
                .args(["-r", &format!("{fps}")])
                .args(["-loop", &loops.to_string()])
                .args(["-f", "gif"])
                .arg(output.as_os_str());
        },
    )
}

/// Export a range of frames as an animated WebP.
pub fn export_webp(
    reel: &crate::Reel<'_>,
    frames: std::ops::Range<u32>,
    path: &Path,
    settings: &ExportSettings,
    webp: &WebpSettings,
    preference: &GpuPreference,
    progress: impl FnMut(u32, u32) -> bool,
) -> Result<AnimatedReport> {
    let webp = webp.clone();
    encode(
        reel,
        frames,
        path,
        settings,
        preference,
        AnimatedFormat::Webp,
        progress,
        move |command, width, height, fps, output| {
            feed(command, width, height, fps);
            command
                .args(["-c:v", "libwebp"])
                // libwebp keeps the alpha the exporter hands out, so a WebP over
                // a transparent stage stays transparent.
                .args(["-pix_fmt", "yuva420p"])
                .args(["-lossless", if webp.lossless { "1" } else { "0" }])
                .args(["-quality", &webp.quality.min(100).to_string()])
                .args(["-loop", &webp.loops.to_string()])
                .args(["-r", &format!("{fps}")])
                .args(["-f", "webp"])
                .arg(output.as_os_str());
        },
    )
}

/// The shared input arguments: raw straight-alpha RGBA at a given size and rate.
fn feed(command: &mut Command, width: u32, height: u32, fps: f64) {
    command
        .arg("-hide_banner")
        .arg("-y")
        .args(["-f", "rawvideo"])
        // **Straight (unpremultiplied) RGBA**, which is what the exporter hands
        // out — the same fringe-avoiding choice the video path makes.
        .args(["-pix_fmt", "rgba"])
        .args(["-s", &format!("{width}x{height}")])
        .args(["-r", &format!("{fps}")])
        .args(["-i", "-"]);
}

/// Render, pipe, and encode — everything the two formats share.
#[allow(clippy::too_many_arguments, reason = "two call sites, all of it needed")]
fn encode(
    reel: &crate::Reel<'_>,
    frames: std::ops::Range<u32>,
    path: &Path,
    settings: &ExportSettings,
    preference: &GpuPreference,
    format: AnimatedFormat,
    mut progress: impl FnMut(u32, u32) -> bool,
    build: impl FnOnce(&mut Command, u32, u32, f64, &Path),
) -> Result<AnimatedReport> {
    if frames.is_empty() {
        bail!("there are no frames in that range to export");
    }
    if !crate::ffmpeg_available() {
        bail!(
            "no ffmpeg was found on this machine, and animated export needs one.\n\
             Install it and make sure `ffmpeg` is on your PATH — on Windows, \
             `winget install Gyan.FFmpeg`."
        );
    }

    if reel.is_empty() {
        bail!("there are no scenes to export");
    }
    let lead = reel.lead().expect("a non-empty reel has a lead scene");
    let fps = lead.stage().frame_rate.max(1.0);
    let mut exporter = Exporter::new(preference)?;
    let total = frames.len() as u32;

    // Rendered once before the pipe opens, so a size ffmpeg cannot take is
    // reported before a process is started.
    let numbers: Vec<u32> = frames.collect();
    let (first_scene, first_frame) = reel
        .at_clamped(numbers[0])
        .expect("a non-empty reel always has a frame to hold on");
    let probe = exporter.render(first_scene, first_frame, settings)?;
    let (width, height) = (probe.width, probe.height);

    // Written to a temporary name and renamed into place: an interrupted encode
    // must not leave a broken file where the render was.
    let temporary = path.with_extension(format!("{}.part", format.extension()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut command = Command::new("ffmpeg");
    build(&mut command, width, height, fps, &temporary);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child: Child = command.spawn().context("starting ffmpeg")?;

    let mut stdin = child.stdin.take().context("ffmpeg accepted no input pipe")?;

    let mut written = 0u32;
    let mut cancelled = false;
    let mut frame = probe;

    for (i, &index) in numbers.iter().enumerate() {
        if i > 0 {
            let (scene, at) = reel
                .at_clamped(index)
                .expect("a non-empty reel always has a frame to hold on");
            frame = exporter.render(scene, at, settings)?;
        }

        if stdin.write_all(&frame.pixels).is_err() {
            // A broken pipe means ffmpeg has given up; its message, read below,
            // is more useful than ours.
            break;
        }
        written += 1;

        if !progress(written, total) {
            cancelled = true;
            break;
        }
    }

    drop(stdin);

    let output = child.wait_with_output().context("waiting for ffmpeg")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&temporary);
        let message = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = message.lines().rev().take(6).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        bail!(
            "ffmpeg could not encode this {}:\n{}",
            format.label().to_lowercase(),
            tail.join("\n")
        );
    }

    if cancelled {
        let _ = std::fs::remove_file(&temporary);
        bail!("the export was cancelled");
    }

    std::fs::rename(&temporary, path)
        .with_context(|| format!("moving the finished file into {}", path.display()))?;

    Ok(AnimatedReport {
        path: path.to_path_buf(),
        frames: written,
        format,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::Scene;
    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::{LayerKind, ShapeData};
    use peniko::Color;

    fn document() -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(4.0, 4.0, 40.0, 40.0).to_path(1e-9), Color::WHITE),
        );
        scene.set_frame_count(4);
        scene
    }

    fn ready() -> bool {
        if !crate::ffmpeg_available() {
            eprintln!("skipping animated export test: no ffmpeg");
            return false;
        }
        match Exporter::new(&GpuPreference::Automatic) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("skipping animated export test: no usable GPU ({e})");
                false
            }
        }
    }

    #[test]
    fn a_gif_is_written() {
        if !ready() {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("clip.gif");
        let scene = document();
        let settings = ExportSettings::scaled(&scene, 0.25);

        let report = export_gif(
            &crate::Reel::single(&scene),
            0..4,
            &path,
            &settings,
            &GifSettings::default(),
            &GpuPreference::Automatic,
            |_, _| true,
        )
        .expect("the GIF should encode");

        assert!(path.exists(), "the file should be there");
        assert_eq!(report.frames, 4);
        assert_eq!(report.format, AnimatedFormat::Gif);
        // The `.part` temporary must have been renamed away.
        assert!(!dir.path().join("clip.gif.part").exists());
    }

    #[test]
    fn a_cancelled_gif_leaves_no_file() {
        if !ready() {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("clip.gif");
        let scene = document();
        let settings = ExportSettings::scaled(&scene, 0.25);

        let result = export_gif(
            &crate::Reel::single(&scene),
            0..4,
            &path,
            &settings,
            &GifSettings::default(),
            &GpuPreference::Automatic,
            // Stop after the first frame.
            |done, _| done < 1,
        );

        assert!(result.is_err(), "a cancelled export reports as much");
        assert!(!path.exists(), "nothing should be left behind");
        assert!(!dir.path().join("clip.gif.part").exists());
    }

    #[test]
    fn a_webp_is_written() {
        if !ready() {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("clip.webp");
        let scene = document();
        let settings = ExportSettings::scaled(&scene, 0.25);

        let report = export_webp(
            &crate::Reel::single(&scene),
            0..4,
            &path,
            &settings,
            &WebpSettings::default(),
            &GpuPreference::Automatic,
            |_, _| true,
        )
        .expect("the WebP should encode");

        assert!(path.exists());
        assert_eq!(report.frames, 4);
        assert_eq!(report.format, AnimatedFormat::Webp);
    }
}
