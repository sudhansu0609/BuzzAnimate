//! CP-6.2 on the real machine: render a document, encode it, and read the
//! result back with ffmpeg to check it is what was asked for.
//!
//! **A file being written is not the claim.** ffmpeg will happily produce a
//! zero-frame MP4 from a broken pipe, and a video that is the wrong length, the
//! wrong size or the wrong colour is worse than one that failed — because
//! nobody finds out until they watch it. So these tests probe the finished file
//! with `ffprobe` and decode a frame back out of it.
//!
//! Skips cleanly when there is no GPU or no ffmpeg, so it is safe in CI.

use std::path::Path;
use std::process::Command;

use buzz_export::{
    AudioTrack, ExportSettings, VideoContainer, VideoSettings, export_video, ffmpeg_available,
};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

const RED: Color = Color::from_rgb8(0xFF, 0x00, 0x00);

/// A stage with a red square that moves, so a decoded frame can be checked
/// against the frame it claims to be.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    // Even in both axes: H.264 subsamples chroma by two.
    scene.stage_mut().size = buzz_geom::Size::new(320.0, 240.0);
    scene.stage_mut().frame_rate = 24.0;

    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(20.0, 20.0, 140.0, 140.0).to_path(1e-9), RED),
    );
    scene
}

fn can_run() -> bool {
    if !ffmpeg_available() {
        eprintln!("skipping video test: no ffmpeg on PATH");
        return false;
    }
    true
}

/// Ask ffprobe one thing about a file.
fn probe(path: &Path, entry: &str) -> Option<String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", entry])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn export(
    dir: &Path,
    name: &str,
    frames: std::ops::Range<u32>,
    video: VideoSettings,
) -> Option<(std::path::PathBuf, buzz_export::VideoReport)> {
    let scene = document();
    let settings = ExportSettings::for_stage(&scene);
    let path = dir.join(format!("{name}.{}", video.container.extension()));

    match export_video(
        &buzz_export::Reel::single(&scene),
        frames,
        &path,
        &settings,
        &video,
        &GpuPreference::Automatic,
        &[],
        |_, _| true,
    ) {
        Ok(report) => Some((path, report)),
        Err(e) => {
            // No GPU on the build machine is a skip, not a failure. Anything
            // else is a real defect and must fail loudly.
            let message = format!("{e:#}");
            if message.contains("adapter") || message.contains("GPU") || message.contains("device")
            {
                eprintln!("skipping video test: {message}");
                None
            } else {
                panic!("video export failed: {message}");
            }
        }
    }
}

/// The whole claim of CP-6.2: movement comes out, as a file a player opens.
#[test]
fn a_video_is_written_with_the_frames_and_the_size_asked_for() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let Some((path, report)) = export(dir.path(), "movement", 0..12, VideoSettings::default())
    else {
        return;
    };

    assert!(path.exists(), "no file was written");
    assert_eq!(report.frames, 12);
    assert!(
        std::fs::metadata(&path).unwrap().len() > 0,
        "the file is empty"
    );

    // The .part file must be gone: an export writes to a temporary name and
    // renames, so a leftover means the rename never happened.
    assert!(
        !path.with_extension("mp4.part").exists(),
        "a partial file was left behind"
    );

    assert_eq!(
        probe(&path, "stream=width").as_deref(),
        Some("320"),
        "the video is the wrong width"
    );
    assert_eq!(
        probe(&path, "stream=height").as_deref(),
        Some("240"),
        "the video is the wrong height"
    );

    // Twelve frames at 24 fps is half a second. ffprobe counts frames exactly
    // for a file it can decode, which is also a check that it *is* decodable.
    let counted = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-count_frames", "-show_entries", "stream=nb_read_frames"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(&path)
        .output()
        .expect("ffprobe runs");
    let counted = String::from_utf8_lossy(&counted.stdout).trim().to_string();
    assert_eq!(counted, "12", "the video holds the wrong number of frames");
}

/// The frame rate in the file is the document's, not ffmpeg's default of 25.
/// A film exported at 24 and played at 25 is four per cent fast, which is
/// exactly the kind of error nobody catches until the sound is out of step.
#[test]
fn the_frame_rate_is_the_documents() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let Some((path, _)) = export(dir.path(), "rate", 0..6, VideoSettings::default()) else {
        return;
    };

    let rate = probe(&path, "stream=r_frame_rate").expect("a frame rate");
    assert_eq!(rate, "24/1", "the document's 24 fps did not reach the file");
}

/// The artwork really is in there, in the right colour and the right place.
///
/// Decoded back out as a PNG rather than trusted: everything up to this point
/// could be right while the pixels were shifted, swapped from RGBA to BGRA, or
/// premultiplied — and all three look like a rendering bug rather than an
/// export one.
#[test]
fn a_decoded_frame_holds_the_artwork_where_it_was_drawn() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let Some((path, _)) = export(dir.path(), "pixels", 0..4, VideoSettings::default()) else {
        return;
    };

    let out = dir.path().join("frame.png");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-y"])
        .args(["-i", &path.to_string_lossy()])
        .args(["-vframes", "1"])
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "the video could not be decoded");

    let decoded =
        std::io::BufReader::new(std::fs::File::open(&out).expect("the frame was written"));
    let mut reader = png::Decoder::new(decoded).read_info().expect("valid PNG");
    let mut buffer = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut buffer).expect("a frame");
    let channels = info.color_type.samples();

    let at = |x: usize, y: usize| {
        let i = (y * info.width as usize + x) * channels;
        [buffer[i], buffer[i + 1], buffer[i + 2]]
    };

    // The square covers 20..140 in both axes; everything outside it is white.
    // The tolerance is wide because this has been through 4:2:0 chroma
    // subsampling and a lossy encoder — the claim is "red here, white there",
    // not an exact value.
    let inside = at(80, 80);
    assert!(
        inside[0] > 180 && inside[1] < 80 && inside[2] < 80,
        "the square should be red at (80, 80), got {inside:?}"
    );

    let outside = at(260, 200);
    assert!(
        outside[0] > 200 && outside[1] > 200 && outside[2] > 200,
        "the background should be white at (260, 200), got {outside:?} \
         — the channels may be in the wrong order"
    );
}

/// MOV is a different container round the same encoder, and must also open.
#[test]
fn mov_is_written_as_well_as_mp4() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let settings = VideoSettings {
        container: VideoContainer::Mov,
        ..Default::default()
    };
    let Some((path, report)) = export(dir.path(), "quicktime", 0..4, settings) else {
        return;
    };

    assert_eq!(path.extension().unwrap(), "mov");
    assert_eq!(report.frames, 4);
    assert!(
        probe(&path, "stream=width").is_some(),
        "the MOV will not open"
    );
}

/// Software encoding is the fallback every machine has, and it must work on
/// its own — an export that only succeeds with an NVIDIA card is not a
/// fallback at all.
#[test]
fn software_encoding_produces_a_playable_file() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let settings = VideoSettings {
        hardware: false,
        ..Default::default()
    };
    let Some((path, report)) = export(dir.path(), "software", 0..4, settings) else {
        return;
    };

    assert_eq!(report.encoder, "libx264");
    assert!(
        !report.fell_back_to_software,
        "asking for software is not a fallback"
    );
    assert_eq!(probe(&path, "stream=width").as_deref(), Some("320"));
}

/// NVENC, where the machine has it. Reported honestly either way: an animator
/// who believes they are using the GPU and is not should be told.
#[test]
fn nvenc_is_used_when_the_machine_has_it() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let Some((path, report)) = export(dir.path(), "nvenc", 0..4, VideoSettings::default()) else {
        return;
    };

    if report.fell_back_to_software {
        eprintln!("no NVENC on this machine; fell back to {}", report.encoder);
    } else {
        assert_eq!(report.encoder, "h264_nvenc");
    }
    assert_eq!(probe(&path, "stream=width").as_deref(), Some("320"));
}

/// A cancelled export leaves **nothing**, unlike a PNG sequence where the
/// frames already written are worth keeping. Half an MP4 has no index and will
/// not open, so a file that looks like a render and is not would be worse than
/// no file at all.
#[test]
fn a_cancelled_export_leaves_no_file_behind() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let scene = document();
    let settings = ExportSettings::for_stage(&scene);
    let path = dir.path().join("cancelled.mp4");

    let result = export_video(
        &buzz_export::Reel::single(&scene),
        0..20,
        &path,
        &settings,
        &VideoSettings::default(),
        &GpuPreference::Automatic,
        &[],
        // Stop after two frames.
        |done, _| done < 2,
    );

    match result {
        Err(e) if format!("{e:#}").contains("cancel") => {}
        Err(e) => {
            let message = format!("{e:#}");
            if message.contains("adapter") || message.contains("GPU") || message.contains("device")
            {
                eprintln!("skipping: {message}");
                return;
            }
            panic!("expected a cancellation, got: {message}");
        }
        Ok(_) => panic!("the export should have been cancelled"),
    }

    assert!(!path.exists(), "a cancelled export left a file");
    assert!(
        !path.with_extension("mp4.part").exists(),
        "a cancelled export left a partial file"
    );
}

/// An odd size is refused with a message that says what to do, rather than
/// failing inside ffmpeg with a chroma error nobody can act on.
#[test]
fn an_odd_size_is_refused_with_a_reason() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let scene = document();
    let mut settings = ExportSettings::for_stage(&scene);
    settings.width = 321;

    let result = export_video(
        &buzz_export::Reel::single(&scene),
        0..2,
        &dir.path().join("odd.mp4"),
        &settings,
        &VideoSettings::default(),
        &GpuPreference::Automatic,
        &[],
        |_, _| true,
    );

    let Err(e) = result else {
        panic!("an odd width should have been refused")
    };
    let message = format!("{e:#}");
    if message.contains("adapter") || message.contains("GPU") || message.contains("device") {
        eprintln!("skipping: {message}");
        return;
    }
    assert!(
        message.contains("even"),
        "the message should say what is wrong: {message}"
    );
}

/// **§7 item 41.** A soundtrack is muxed in, at the right place and the right
/// length — the half of Phase 6 a PNG sequence cannot have by definition.
///
/// The audio is generated by ffmpeg rather than shipped as a fixture: a
/// committed WAV would be a binary blob in the repository for a test that only
/// needs "some sound, of a known length".
#[test]
fn a_soundtrack_is_muxed_in_at_its_cue() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");

    // One second of a 440 Hz tone.
    let tone = dir.path().join("tone.wav");
    let made = Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-y"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
        .arg(&tone)
        .status()
        .expect("ffmpeg runs");
    assert!(made.success(), "the test tone could not be made");

    let scene = document();
    let settings = ExportSettings::for_stage(&scene);
    let path = dir.path().join("with-sound.mp4");

    // Two seconds of film at 24 fps, with the tone cued half a second in.
    let report = match export_video(
        &buzz_export::Reel::single(&scene),
        0..48,
        &path,
        &settings,
        &VideoSettings::default(),
        &GpuPreference::Automatic,
        &[AudioTrack {
            path: tone.clone(),
            offset_seconds: 0.5,
            volume: 1.0,
        }],
        |_, _| true,
    ) {
        Ok(report) => report,
        Err(e) => {
            let message = format!("{e:#}");
            if message.contains("adapter") || message.contains("GPU") || message.contains("device")
            {
                eprintln!("skipping: {message}");
                return;
            }
            panic!("export failed: {message}");
        }
    };

    assert_eq!(report.audio_tracks, 1, "the soundtrack was not muxed");

    // There is an audio stream, and it is AAC.
    let codec = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0"])
        .args(["-show_entries", "stream=codec_name"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(&path)
        .output()
        .expect("ffprobe runs");
    let codec = String::from_utf8_lossy(&codec.stdout).trim().to_string();
    assert_eq!(codec, "aac", "no audio stream in the file");

    // **The delay is real.** The tone is one second long, cued half a second
    // in, so the audio runs to about 1.5 seconds — not 1.0. Getting the delay
    // wrong is silent in every other way: the file plays, the sound is there,
    // and it is simply in the wrong place.
    let duration = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0"])
        .args(["-show_entries", "stream=duration"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(&path)
        .output()
        .expect("ffprobe runs");
    let seconds: f64 = String::from_utf8_lossy(&duration.stdout)
        .trim()
        .parse()
        .expect("a duration");
    assert!(
        (seconds - 1.5).abs() < 0.2,
        "the sound should run to about 1.5 s with its half-second delay, got {seconds}"
    );
}

/// Turning the soundtrack off in the settings really leaves it out, even when
/// tracks are supplied — the checkbox has to mean something.
#[test]
fn audio_can_be_switched_off() {
    if !can_run() {
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let tone = dir.path().join("tone.wav");
    let made = Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-y"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
        .arg(&tone)
        .status()
        .expect("ffmpeg runs");
    assert!(made.success());

    let scene = document();
    let settings = ExportSettings::for_stage(&scene);
    let path = dir.path().join("silent.mp4");

    let video = VideoSettings {
        audio: false,
        ..Default::default()
    };
    let report = match export_video(
        &buzz_export::Reel::single(&scene),
        0..8,
        &path,
        &settings,
        &video,
        &GpuPreference::Automatic,
        &[AudioTrack {
            path: tone,
            offset_seconds: 0.0,
            volume: 1.0,
        }],
        |_, _| true,
    ) {
        Ok(report) => report,
        Err(e) => {
            let message = format!("{e:#}");
            if message.contains("adapter") || message.contains("GPU") || message.contains("device")
            {
                eprintln!("skipping: {message}");
                return;
            }
            panic!("export failed: {message}");
        }
    };

    assert_eq!(report.audio_tracks, 0);
    let streams = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a"])
        .args(["-show_entries", "stream=codec_name"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(&path)
        .output()
        .expect("ffprobe runs");
    assert!(
        String::from_utf8_lossy(&streams.stdout).trim().is_empty(),
        "audio was muxed in despite being switched off"
    );
}
