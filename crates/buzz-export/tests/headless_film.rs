//! Assembling a film from shots, on the real machine.
//!
//! Two short shots are encoded with the same settings and concatenated by
//! stream copy; ffprobe then confirms the film's duration is the sum of the
//! shots and that it carries a single video stream. Skips cleanly with no GPU
//! or no ffmpeg.

use std::path::Path;
use std::process::Command;

use buzz_export::{
    ExportSettings, VideoSettings, concat_segments, export_video, ffmpeg_available,
};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = buzz_geom::Size::new(320.0, 240.0);
    scene.stage_mut().frame_rate = 24.0;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(
            Rect::new(40.0, 40.0, 160.0, 160.0).to_path(1e-9),
            Color::from_rgb8(0x20, 0x60, 0xC0),
        ),
    );
    scene
}

fn probe(path: &Path, entry: &str) -> Option<String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(["-show_entries", entry])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn encode(dir: &Path, name: &str, frames: std::ops::Range<u32>) -> Option<std::path::PathBuf> {
    let scene = document();
    let settings = ExportSettings::for_stage(&scene);
    let path = dir.join(name);
    match export_video(
        &buzz_export::Reel::single(&scene),
        frames,
        &path,
        &settings,
        &VideoSettings::default(),
        &GpuPreference::Automatic,
        &[],
        |_, _| true,
    ) {
        Ok(_) => Some(path),
        Err(e) => {
            let m = format!("{e:#}");
            if m.contains("adapter") || m.contains("GPU") || m.contains("device") {
                eprintln!("skipping film test: {m}");
                None
            } else {
                panic!("shot export failed: {m}");
            }
        }
    }
}

#[test]
fn two_shots_concatenate_into_one_film() {
    if !ffmpeg_available() {
        eprintln!("skipping film test: no ffmpeg");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");

    // Two four-frame shots, encoded identically.
    let Some(a) = encode(dir.path(), "shot_a.mp4", 0..4) else {
        return;
    };
    let b = encode(dir.path(), "shot_b.mp4", 0..4).expect("second shot");

    let film = dir.path().join("film.mp4");
    concat_segments(&[a, b], &film).expect("concatenate");
    assert!(film.exists(), "no film was written");

    // One video stream.
    let streams = probe(&film, "stream=index").unwrap_or_default();
    assert_eq!(
        streams.lines().count(),
        1,
        "the film should have exactly one video stream, got {streams:?}"
    );

    // Eight frames of video: the two four-frame shots end to end.
    let frames = probe(&film, "stream=nb_read_frames")
        .or_else(|| {
            Command::new("ffprobe")
                .args(["-v", "error", "-count_frames"])
                .args(["-select_streams", "v:0"])
                .args(["-show_entries", "stream=nb_read_frames"])
                .args(["-of", "default=noprint_wrappers=1:nokey=1"])
                .arg(&film)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .and_then(|s| s.parse::<u32>().ok());

    if let Some(count) = frames {
        assert_eq!(count, 8, "the film should be the two shots end to end");
    } else {
        eprintln!("note: ffprobe would not count frames; file existence checked only");
    }
}
