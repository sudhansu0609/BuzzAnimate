//! CP-6.2 — MP4 and MOV, encoded on the GPU.
//!
//! # Why ffmpeg is a child process rather than a linked library
//!
//! The alternatives were a Rust binding to libav (`ffmpeg-next`), which means
//! shipping and linking a large C library and matching its version to whatever
//! the machine has, and `ffmpeg-sidecar`, which downloads an ffmpeg build over
//! the network on first use. A creative tool that quietly fetches an executable
//! and runs it is not something to build on, and the licensing of a bundled
//! ffmpeg is its own question — H.264 in particular.
//!
//! So this drives the ffmpeg **already on the machine**, over a pipe. It is the
//! same thing `ffmpeg-sidecar` does once its download is finished; what is
//! skipped is the download. If there is no ffmpeg, that is said plainly, with
//! the one line needed to fix it — rather than the export failing with a
//! process-spawn error.
//!
//! # Why frames are piped rather than written and re-read
//!
//! A 500-frame 1080p export is 4 GB of PNG on the way past. Writing that to
//! disk so ffmpeg can read it back doubles the I/O and needs somewhere to put
//! it. Raw frames go straight down ffmpeg's stdin as they are rendered, so the
//! export holds one frame in memory and the disk sees only the finished file.
//!
//! # NVENC, and what happens when it is not there
//!
//! The 5060 Ti encodes H.264, HEVC and AV1 in hardware, which is why this
//! project chose the card. But an export that *fails* because a machine has no
//! NVIDIA card would be a bad trade for the speed, so each hardware encoder
//! names a software one to fall back to, and the fallback is reported rather
//! than hidden — an animator who thinks they are using the GPU and is not
//! should be told.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use buzz_render::GpuPreference;

use crate::{ExportSettings, Exporter};

/// Which encoder to ask ffmpeg for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoCodec {
    /// H.264. Plays everywhere, which is why it is the default.
    #[default]
    H264,
    /// HEVC — smaller at the same quality, and not universally playable.
    Hevc,
    /// AV1. Smaller again, newer, and needs a recent player.
    Av1,
    /// Apple ProRes 4444 — keeps a real alpha channel, so the work composites
    /// over anything. Large files, and always a `.mov`.
    ProRes4444,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC (H.265)",
            Self::Av1 => "AV1",
            Self::ProRes4444 => "ProRes 4444 (alpha)",
        }
    }

    /// The NVENC encoder for this codec. ProRes has none, so it names its own.
    pub fn hardware_encoder(self) -> &'static str {
        match self {
            Self::H264 => "h264_nvenc",
            Self::Hevc => "hevc_nvenc",
            Self::Av1 => "av1_nvenc",
            Self::ProRes4444 => "prores_ks",
        }
    }

    /// What to use when NVENC is not available.
    pub fn software_encoder(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::Hevc => "libx265",
            // libaom is very slow; SVT-AV1 is what a modern ffmpeg ships and
            // what anyone encoding AV1 on a CPU actually uses.
            Self::Av1 => "libsvtav1",
            Self::ProRes4444 => "prores_ks",
        }
    }

    /// Whether this codec carries an alpha channel — so the export renders on a
    /// transparent background and ffmpeg is told a pixel format that keeps it.
    pub fn is_alpha(self) -> bool {
        matches!(self, Self::ProRes4444)
    }
}

/// The container to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoContainer {
    #[default]
    Mp4,
    Mov,
}

impl VideoContainer {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Mov => "MOV",
        }
    }
}

/// How a video export should be encoded.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    pub container: VideoContainer,
    /// Constant-quality level. Lower is better; ffmpeg's own scale, where 0 is
    /// lossless and 51 is unwatchable. 20 is visually clean for line artwork.
    pub quality: u32,
    /// Use NVENC where it is available.
    pub hardware: bool,
    /// Mux the document's soundtrack in, if it has one.
    pub audio: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            codec: VideoCodec::default(),
            container: VideoContainer::default(),
            // Animate's H.264 presets land around here, and flat vector artwork
            // shows banding long before it shows blocking.
            quality: 20,
            hardware: true,
            audio: true,
        }
    }
}

/// One sound to mux in, and where it sits in the finished film.
///
/// The file rather than the samples, because a `SoundAsset` keeps the bytes as
/// they were imported and ffmpeg would rather decode an MP3 itself than be
/// handed our decode of it.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub path: PathBuf,
    /// Seconds from the **start of the exported range**, not from the start of
    /// the document. A range beginning at frame 100 puts a sound cued to frame
    /// 100 at zero; measuring from the document instead would leave four
    /// seconds of silence at the head of every partial export.
    pub offset_seconds: f64,
    /// `0.0..=1.0`, the cue's own volume.
    pub volume: f32,
}

/// What a video export produced.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoReport {
    pub path: PathBuf,
    pub frames: u32,
    /// The encoder ffmpeg actually used.
    pub encoder: String,
    /// True when NVENC was asked for and not available.
    pub fell_back_to_software: bool,
    /// How many sounds were muxed in.
    pub audio_tracks: usize,
}

/// Is there an ffmpeg to drive?
///
/// Checked before an export starts rather than discovered when the pipe breaks,
/// so the message can say what to do about it.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// **What a video file contains**, as ffmpeg reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// Frames per second, as the file declares them.
    pub fps: f64,
    /// Length in seconds. Zero if ffmpeg would not say.
    pub seconds: f64,
}

/// Ask ffmpeg what is in a video file.
///
/// # Why this lives in the *export* crate
///
/// Because this is where ffmpeg lives, and there should be one place that
/// knows how to drive it. Reading a video is not exporting, but the alternative
/// is a second module that finds ffmpeg, spawns it and interprets its output —
/// two copies of the awkward part, to keep a tidy name.
pub fn probe(path: &std::path::Path) -> Result<VideoInfo> {
    // `ffprobe` ships with ffmpeg but is a separate binary and is missing from
    // some minimal builds, so this asks ffmpeg itself and reads what it says
    // about the input on the way to doing nothing with it.
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .context("running ffmpeg to read the video")?;
    // ffmpeg exits non-zero when given no output file; what matters is what it
    // printed about the input first.
    let text = String::from_utf8_lossy(&out.stderr);

    let stream = text
        .lines()
        .find(|l| l.contains("Stream #") && l.contains("Video:"))
        .ok_or_else(|| anyhow::anyhow!("no video stream in {}", path.display()))?;

    // "1920x1080" somewhere in the stream line.
    let size = stream
        .split(|c: char| c == ',' || c == ' ')
        .filter_map(|token| {
            let (w, h) = token.split_once('x')?;
            // A trailing "[SAR ...]" and the like come off with the split.
            Some((w.parse::<u32>().ok()?, h.trim_end().parse::<u32>().ok()?))
        })
        .find(|(w, h)| *w > 0 && *h > 0)
        .ok_or_else(|| anyhow::anyhow!("could not read the size of {}", path.display()))?;

    // "24 fps" or "23.98 fps".
    let fps = stream
        .split(", ")
        .find_map(|part| part.strip_suffix(" fps")?.trim().parse::<f64>().ok())
        .filter(|f| *f > 0.0)
        .unwrap_or(24.0);

    // "Duration: 00:00:12.34,"
    let seconds = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Duration: "))
        .and_then(|d| {
            let clock = d.split(',').next()?;
            let mut parts = clock.split(':');
            let h: f64 = parts.next()?.trim().parse().ok()?;
            let m: f64 = parts.next()?.parse().ok()?;
            let sec: f64 = parts.next()?.parse().ok()?;
            Some(h * 3600.0 + m * 60.0 + sec)
        })
        .unwrap_or(0.0);

    Ok(VideoInfo {
        width: size.0,
        height: size.1,
        fps,
        seconds,
    })
}

/// **Pull a video apart into one PNG per frame**, at `fps`, no wider or taller
/// than `fit`, into `dir`. Returns the files written, in order.
///
/// `limit` caps how many frames are written: a reference layer is something to
/// draw over, and a document is not the place for ten minutes of somebody's
/// footage.
pub fn extract_frames(
    path: &std::path::Path,
    fps: f64,
    fit: (u32, u32),
    limit: u32,
    dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>> {
    if !ffmpeg_available() {
        bail!("ffmpeg is not on this machine, so a video cannot be read");
    }
    std::fs::create_dir_all(dir).context("making somewhere to put the frames")?;

    let pattern = dir.join("frame-%05d.png");
    // `decrease` never enlarges: a video smaller than the stage is left alone
    // rather than blown up into a blurry reference. The `-2`s keep the scaler
    // on even dimensions, which some filters insist on.
    let scale = format!(
        "scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease",
        fit.0.max(2),
        fit.1.max(2)
    );
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-vf", &format!("fps={fps},{scale}")])
        .args(["-frames:v", &limit.max(1).to_string()])
        .arg(&pattern)
        .stdin(Stdio::null())
        .status()
        .context("running ffmpeg to pull the video apart")?;
    if !status.success() {
        bail!("ffmpeg could not read {}", path.display());
    }

    let mut frames: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .context("reading the frames back")?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
        .collect();
    frames.sort();
    if frames.is_empty() {
        bail!("{} yielded no frames", path.display());
    }
    Ok(frames)
}

/// The encoders this machine's ffmpeg can actually use.
///
/// `ffmpeg -encoders` lists what was *compiled in*, which is not the same as
/// what will run: a build with `h264_nvenc` compiled in still fails on a
/// machine with no NVIDIA driver. Compiled-in is the cheap check and rules out
/// the common case; a real failure is caught when the process exits and is
/// reported with ffmpeg's own words.
fn compiled_encoders() -> Vec<String> {
    let Ok(out) = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1).map(str::to_string))
        .collect()
}

/// Choose the encoder, and say whether it is the one that was asked for.
fn choose_encoder(settings: &VideoSettings) -> (String, bool) {
    // ProRes has no hardware path; it is always the one software encoder.
    if settings.codec.is_alpha() {
        return (settings.codec.software_encoder().to_string(), false);
    }
    let available = compiled_encoders();
    let wanted = settings.codec.hardware_encoder();
    if settings.hardware && available.iter().any(|e| e == wanted) {
        return (wanted.to_string(), false);
    }
    (
        settings.codec.software_encoder().to_string(),
        settings.hardware,
    )
}

/// Export a range of frames as a video file.
///
/// `progress` is called with frames finished and frames total; returning
/// `false` cancels. A cancelled export **removes the partial file** — unlike a
/// PNG sequence, where half the frames are still worth having, half an MP4 with
/// no trailing index is a file that will not open.
#[allow(clippy::too_many_arguments, reason = "one call site, all of it needed")]
pub fn export_video(
    reel: &crate::Reel<'_>,
    frames: std::ops::Range<u32>,
    path: &Path,
    settings: &ExportSettings,
    video: &VideoSettings,
    preference: &GpuPreference,
    audio: &[AudioTrack],
    mut progress: impl FnMut(u32, u32) -> bool,
) -> Result<VideoReport> {
    if frames.is_empty() {
        bail!("there are no frames in that range to export");
    }
    if !ffmpeg_available() {
        bail!(
            "no ffmpeg was found on this machine, and video export needs one.\n\
             Install it and make sure `ffmpeg` is on your PATH — on Windows, \
             `winget install Gyan.FFmpeg`."
        );
    }

    if reel.is_empty() {
        bail!("there are no scenes to export");
    }

    // An alpha codec must render on a transparent background, or there is no
    // alpha to keep — force it whatever the checkbox says.
    let effective;
    let settings = if video.codec.is_alpha() && !settings.transparent {
        effective = ExportSettings { transparent: true, ..*settings };
        &effective
    } else {
        settings
    };

    // The same resolution through the reel that a PNG sequence uses, so scenes
    // follow one another and a looping section repeats in the video exactly as
    // it does in the frames.
    let lead = reel.lead().expect("a non-empty reel has a lead scene");
    let fps = lead.stage().frame_rate.max(1.0);
    let (encoder, fell_back) = choose_encoder(video);

    let mut exporter = Exporter::new(preference)?;
    let total = frames.len() as u32;

    // Rendered once before the pipe opens, so a size ffmpeg cannot take is
    // reported before a process is started rather than after.
    let numbers: Vec<u32> = frames.collect();
    let (first_scene, first_frame) = reel
        .at_clamped(numbers[0])
        .expect("a non-empty reel always has a frame to hold on");
    let probe = exporter.render(first_scene, first_frame, settings)?;
    let (width, height) = (probe.width, probe.height);

    // **H.264 and HEVC need even dimensions.** Their chroma is subsampled by
    // two, so an odd width has half a chroma sample at the edge and the encoder
    // refuses outright. Animate's own 550 x 400 default stage is even, but a
    // scaled export is not reliably so, and "550 works and 551 fails" is a
    // baffling thing to meet mid-deadline.
    if !video.codec.is_alpha() && (width % 2 != 0 || height % 2 != 0) {
        bail!(
            "video is {width} x {height}, and H.264 and HEVC need both \
             dimensions to be even. Adjust the export size by a pixel."
        );
    }

    // Written to a temporary name and renamed into place, exactly as an image
    // export is: an interrupted encode must not leave a broken file where the
    // user's render was.
    let temporary = path.with_extension(format!("{}.part", video.container.extension()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let tracks: &[AudioTrack] = if video.audio { audio } else { &[] };
    let mut child = spawn_ffmpeg(&encoder, video, width, height, fps, tracks, &temporary)?;
    let mut stdin = child
        .stdin
        .take()
        .context("ffmpeg accepted no input pipe")?;

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

        // A broken pipe means ffmpeg has already given up — on a bad codec
        // argument, usually. Its own message is far more useful than ours, so
        // the loop stops here and the error is read off the process below.
        if stdin.write_all(&frame.pixels).is_err() {
            break;
        }
        written += 1;

        if !progress(written, total) {
            cancelled = true;
            break;
        }
    }

    // Closing the pipe is what tells ffmpeg the stream has ended; without it,
    // waiting for the process deadlocks.
    drop(stdin);

    let output = child.wait_with_output().context("waiting for ffmpeg")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&temporary);
        let message = String::from_utf8_lossy(&output.stderr);
        // ffmpeg is verbose and the useful line is at the end.
        let tail: Vec<&str> = message.lines().rev().take(6).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        bail!("ffmpeg could not encode this video:\n{}", tail.join("\n"));
    }

    if cancelled {
        // Half an MP4 has no index and will not open, so there is nothing worth
        // keeping — unlike half a PNG sequence.
        let _ = std::fs::remove_file(&temporary);
        bail!("the export was cancelled");
    }

    std::fs::rename(&temporary, path)
        .with_context(|| format!("moving the finished video into {}", path.display()))?;

    Ok(VideoReport {
        path: path.to_path_buf(),
        frames: written,
        encoder,
        fell_back_to_software: fell_back,
        audio_tracks: tracks.len(),
    })
}

/// Start ffmpeg, reading raw frames on stdin.
fn spawn_ffmpeg(
    encoder: &str,
    video: &VideoSettings,
    width: u32,
    height: u32,
    fps: f64,
    audio: &[AudioTrack],
    output: &Path,
) -> Result<Child> {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        // Overwrite: the caller has already decided, and a prompt on stdin
        // from a child process would hang the export for ever.
        .arg("-y")
        .args(["-f", "rawvideo"])
        // **The exporter hands out straight (unpremultiplied) RGBA**, which is
        // what `rgba` means to ffmpeg. Naming the wrong one here gives a video
        // with dark fringes wherever the artwork is translucent — the same
        // defect CP-6.1 fixed on the PNG side, and it would come straight back.
        .args(["-pix_fmt", "rgba"])
        .args(["-s", &format!("{width}x{height}")])
        .args(["-r", &format!("{fps}")])
        .args(["-i", "-"]);

    for track in audio {
        command.args(["-i", &track.path.to_string_lossy()]);
    }

    // **Every sound is delayed to its own cue and the lot is mixed.** A film
    // has more than one: dialogue on one keyframe, a door on another. Muxing
    // only the first would silently drop the rest, and muxing them all without
    // delays would stack them at zero — every effect firing at once on the
    // first frame, which sounds like a fault rather than a missing feature.
    //
    // `adelay` is in milliseconds and `all=1` applies it to every channel, so a
    // stereo file does not need its delay written twice. `normalize=0` on the
    // mix keeps each sound at the level it was given: `amix` otherwise divides
    // by the number of inputs, so adding a footstep would quietly halve the
    // dialogue.
    if !audio.is_empty() {
        let mut graph = String::new();
        for (i, track) in audio.iter().enumerate() {
            let input = i + 1; // input 0 is the video.
            let delay_ms = (track.offset_seconds.max(0.0) * 1000.0).round() as u64;
            graph.push_str(&format!(
                "[{input}:a]adelay={delay_ms}:all=1,volume={:.4}[a{i}];",
                track.volume.clamp(0.0, 1.0)
            ));
        }
        for i in 0..audio.len() {
            graph.push_str(&format!("[a{i}]"));
        }
        graph.push_str(&format!("amix=inputs={}:normalize=0[aout]", audio.len()));

        command
            .args(["-filter_complex", &graph])
            .args(["-map", "0:v"])
            .args(["-map", "[aout]"]);
    }

    command.args(["-c:v", encoder]);

    if video.codec.is_alpha() {
        // ProRes 4444 keeps alpha: a 10-bit 4:4:4:4 pixel format and the 4444
        // profile. No CRF — ProRes is quality-by-profile, not by a rate factor.
        command
            .args(["-profile:v", "4444"])
            .args(["-pix_fmt", "yuva444p10le"]);
    } else {
        // yuv420p rather than a wider format: it is what every player and
        // every website accepts. A 4:4:4 export would be sharper on hard
        // vector edges and would not play in Safari or QuickTime.
        command.args(["-pix_fmt", "yuv420p"]);

        // Quality is spelled differently by each family of encoder, and asking
        // for the wrong one is an error rather than an ignored argument.
        if encoder.ends_with("_nvenc") {
            command
                .args(["-rc", "vbr"])
                .args(["-cq", &video.quality.to_string()])
                // NVENC's default preset is fast and soft; p5 is the middle of
                // the modern scale and is what a render should use.
                .args(["-preset", "p5"]);
        } else {
            command.args(["-crf", &video.quality.to_string()]);
        }
    }

    if !audio.is_empty() {
        command
            .args(["-c:a", "aac"])
            .args(["-b:a", "192k"])
            // **The video decides the length.** Without this a soundtrack
            // longer than the film leaves the last picture frozen on screen
            // for the rest of the audio, which is not what anyone means by
            // "export frames 1 to 100".
            .arg("-shortest");
    }

    if video.container == VideoContainer::Mp4 && !video.codec.is_alpha() {
        // Puts the index at the front, so the file starts playing before it has
        // finished downloading. Costs one extra pass over the finished file.
        command.args(["-movflags", "+faststart"]);
    }

    // **Name the container rather than letting ffmpeg guess it.** It guesses
    // from the output's extension, and the output here is a `.part` temporary
    // — written under a temporary name so an interrupted encode cannot leave a
    // broken file where the render was. ffmpeg has no idea what a `.part` is
    // and refuses to open it at all, which is a failure caused entirely by the
    // safety measure. Saying the format outright makes the two independent.
    command.args([
        "-f",
        // ProRes lives in a QuickTime .mov whatever the chosen container says.
        if video.codec.is_alpha() {
            "mov"
        } else {
            match video.container {
                VideoContainer::Mp4 => "mp4",
                VideoContainer::Mov => "mov",
            }
        },
    ]);

    command
        .arg(output.as_os_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    command.spawn().context("starting ffmpeg")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::Scene;

    #[test]
    fn prores_is_the_alpha_codec_and_chooses_its_own_encoder() {
        assert!(VideoCodec::ProRes4444.is_alpha());
        assert!(!VideoCodec::H264.is_alpha());
        // ProRes has no NVENC path, so it is always its one software encoder,
        // even when hardware encoding was asked for.
        let settings = VideoSettings {
            codec: VideoCodec::ProRes4444,
            hardware: true,
            ..VideoSettings::default()
        };
        let (encoder, fell_back) = choose_encoder(&settings);
        assert_eq!(encoder, "prores_ks");
        assert!(!fell_back, "ProRes was the codec asked for, not a fallback");
    }

    /// Every codec names both a hardware and a software encoder, and they are
    /// never the same string. A codec whose fallback *was* its hardware
    /// encoder would fail identically on the machines the fallback exists for.
    #[test]
    fn every_codec_has_a_distinct_software_fallback() {
        for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            let (hardware, software) = (codec.hardware_encoder(), codec.software_encoder());
            assert!(!hardware.is_empty() && !software.is_empty(), "{codec:?}");
            assert_ne!(hardware, software, "{codec:?} falls back to itself");
            assert!(
                hardware.ends_with("_nvenc"),
                "{codec:?} hardware encoder is not NVENC: {hardware}"
            );
        }
    }

    /// Asking for software explicitly is not a fallback, and must not be
    /// reported as one — the report is what tells an animator whether the GPU
    /// was really used.
    #[test]
    fn choosing_software_deliberately_is_not_reported_as_a_fallback() {
        let settings = VideoSettings {
            hardware: false,
            ..Default::default()
        };
        let (encoder, fell_back) = choose_encoder(&settings);
        assert_eq!(encoder, "libx264");
        assert!(!fell_back, "an explicit choice is not a fallback");
    }

    #[test]
    fn containers_have_their_own_extensions() {
        assert_eq!(VideoContainer::Mp4.extension(), "mp4");
        assert_eq!(VideoContainer::Mov.extension(), "mov");
    }

    /// The default is the one that plays everywhere. A tool whose default
    /// export will not open on a phone is a tool that gets blamed for it.
    #[test]
    fn the_default_is_h264_in_an_mp4_on_the_gpu() {
        let settings = VideoSettings::default();
        assert_eq!(settings.codec, VideoCodec::H264);
        assert_eq!(settings.container, VideoContainer::Mp4);
        assert!(settings.hardware);
        assert!(settings.audio);
    }
}
