//! **Rotoscoping over a video**, end to end.
//!
//! A short clip is made with ffmpeg, imported, and must land as one frame of
//! artwork per frame of the film on a guide layer — the layer kind that is drawn
//! while working and never exported.
//!
//! Skips when there is no ffmpeg, like the export tests: this drives the one on
//! the machine rather than shipping its own.

use buzz_app::editor::Editor;
use buzz_scene::LayerKind;

/// Make a two-second test clip with ffmpeg's own pattern generator.
fn a_test_clip(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = dir.join("clip.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=12:duration=2",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .status()
        .ok()?;
    ok.success().then_some(path)
}

fn guide_layers(editor: &Editor) -> Vec<buzz_scene::LayerId> {
    editor
        .doc
        .scene()
        .layers()
        .iter()
        .filter(|l| l.kind == LayerKind::Guide)
        .map(|l| l.id)
        .collect()
}

#[test]
fn a_video_lands_a_frame_at_a_time_on_a_guide_layer() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(clip) = a_test_clip(dir.path()) else {
        eprintln!("skipping video reference test: no usable ffmpeg");
        return;
    };

    let mut editor = Editor::default();
    // Twelve a second for two seconds, at the document's own rate.
    editor.doc.edit("rate", |scene| {
        scene.stage_mut().frame_rate = 12.0;
    });

    editor
        .import_video_reference(&clip)
        .expect("the clip imports");

    let guides = guide_layers(&editor);
    assert_eq!(guides.len(), 1, "one reference layer");
    let layer = guides[0];

    // A frame of artwork on each of the film's frames, not one picture held.
    let scene = editor.doc.scene();
    let frames = scene.layers().get(layer).expect("the layer").frames.length();
    assert!(
        frames >= 20,
        "two seconds at twelve should be about 24 frames, got {frames}"
    );

    let mut with_art = 0;
    for frame in 0..frames {
        if scene
            .layers()
            .get(layer)
            .expect("the layer")
            .frames
            .resolved_at(frame)
            .iter()
            .next()
            .is_some()
        {
            with_art += 1;
        }
    }
    assert!(
        with_art >= 20,
        "every frame should carry a frame of the video, got {with_art} of {frames}"
    );

    // And they are *different* pictures — a reference that held one frame
    // would be no use to trace a movement over.
    let picture = |frame: u32| {
        let object = scene
            .layers()
            .get(layer)
            .expect("the layer")
            .frames
            .resolved_at(frame)
            .iter()
            .next()
            .cloned()
            .expect("artwork");
        let buzz_scene::ObjectKind::Shape(shape) = &object.kind else {
            panic!("expected a shape")
        };
        match &shape.fill.as_ref().expect("a fill").paint {
            buzz_scene::Paint::Image(img) => img.asset.pixels.clone(),
            other => panic!("expected a picture, got {other:?}"),
        }
    };
    assert_ne!(
        picture(0),
        picture(10),
        "two different moments of the clip should be two different pictures"
    );
}

/// A guide layer is never in the film, which is what makes it a *reference*.
#[test]
fn the_reference_is_not_exported() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let Some(clip) = a_test_clip(dir.path()) else {
        eprintln!("skipping video reference test: no usable ffmpeg");
        return;
    };

    let mut editor = Editor::default();
    editor
        .import_video_reference(&clip)
        .expect("the clip imports");

    let layer = guide_layers(&editor)[0];
    let kind = editor.doc.scene().layers().get(layer).expect("the layer").kind;
    assert!(
        !kind.paints_to_output(),
        "a reference layer must stay out of the film"
    );
}
