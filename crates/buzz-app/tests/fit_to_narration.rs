//! **Laying a timeline out to a voice-over, end to end.**
//!
//! The phrase finder is unit-tested in `buzz-audio` against synthetic envelopes.
//! What that cannot show is the thing an animator actually depends on: import a
//! narration, run one command, and come back to a timeline that is the right
//! length with a keyframe at the start of every line — and that running it
//! again after a re-record does not throw away what was drawn to the lines that
//! did not move.

use buzz_app::editor::Editor;
use buzz_scene::LayerId;
use buzz_ui::Command;

/// A narration: `spec` is a list of (speaking?, seconds), written as a real WAV
/// so the whole import path is exercised rather than a back door.
fn narration_wav(spec: &[(bool, f64)]) -> Vec<u8> {
    let rate = 44_100.0_f64;
    let wav = hound::WavSpec {
        channels: 1,
        sample_rate: rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, wav).expect("writer");
        let mut at = 0.0_f64;
        for &(speaking, seconds) in spec {
            let n = (seconds * rate) as usize;
            for _ in 0..n {
                let t = at / rate;
                // A voice-ish tone rather than a pure sine: two partials, so
                // the loudness envelope is not perfectly flat.
                let v = if speaking {
                    (t * 220.0 * std::f64::consts::TAU).sin() * 0.6
                        + (t * 660.0 * std::f64::consts::TAU).sin() * 0.2
                } else {
                    0.0
                };
                writer.write_sample((v * 28_000.0) as i16).expect("sample");
                at += 1.0;
            }
        }
        writer.finalize().expect("finalize");
    }
    out.into_inner()
}

/// An editor with a three-line narration imported: 1.5s, pause, 1.0s, pause,
/// 2.0s, over about six seconds.
fn editor_with_narration() -> (Editor, LayerId, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("vo.wav");
    std::fs::write(
        &path,
        narration_wav(&[
            (false, 0.3),
            (true, 1.5),
            (false, 0.5),
            (true, 1.0),
            (false, 0.6),
            (true, 2.0),
            (false, 0.3),
        ]),
    )
    .expect("write wav");

    let mut editor = Editor::default();
    editor.import_sound(&path).expect("the narration imports");
    // Draw on a layer of the animator's own, not the one the sound landed on.
    let mut layer = LayerId(0);
    editor.doc.edit("Art", |scene| {
        layer = scene.add_layer("Art", buzz_scene::LayerKind::Normal);
    });
    editor.select_layer(layer);
    (editor, layer, dir)
}

fn keyframes(editor: &Editor, layer: LayerId) -> Vec<u32> {
    editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .map(|l| l.frames.keyframes().iter().map(|k| k.start).collect())
        .unwrap_or_default()
}

fn length(editor: &Editor, layer: LayerId) -> u32 {
    editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .map(|l| l.frames.length())
        .unwrap_or(0)
}

/// **Three lines in, three lines out**, and the film long enough to hold them.
#[test]
fn a_narration_becomes_a_timeline() {
    let (mut editor, layer, _dir) = editor_with_narration();

    editor.run(Command::FitToNarration);

    // Every new layer already has its own keyframe at frame 0; the lines are
    // the ones added on top of it.
    let keys = keyframes(&editor, layer);
    let lines = editor.ruler_marks.clone();
    assert_eq!(
        lines.len(),
        3,
        "expected a line per phrase, got {lines:?} \u{2014} {:?}",
        editor.status
    );
    for start in &lines {
        assert!(
            keys.contains(start),
            "line at frame {start} got no keyframe; the layer has {keys:?}"
        );
    }
    // The narration runs about 6.2 seconds; at 24fps that is ~150 frames.
    assert!(
        length(&editor, layer) >= 140,
        "the film is only {} frames, shorter than the narration",
        length(&editor, layer)
    );
    // The lines start at roughly 0.3s, 2.3s and 3.9s.
    let seconds: Vec<f64> = lines.iter().map(|f| *f as f64 / 24.0).collect();
    assert!((seconds[0] - 0.3).abs() < 0.3, "first line at {:.2}s", seconds[0]);
    assert!((seconds[1] - 2.3).abs() < 0.4, "second line at {:.2}s", seconds[1]);
    assert!((seconds[2] - 3.9).abs() < 0.4, "third line at {:.2}s", seconds[2]);
}

/// **The lines are marked on the ruler**, so they can be seen as well as drawn
/// against.
#[test]
fn the_lines_are_marked_on_the_ruler() {
    let (mut editor, layer, _dir) = editor_with_narration();
    editor.run(Command::FitToNarration);
    let keys = keyframes(&editor, layer);
    assert!(!editor.ruler_marks.is_empty(), "nothing was marked");
    for start in &editor.ruler_marks {
        assert!(
            keys.contains(start),
            "the ruler marks frame {start} but no keyframe was put there"
        );
    }
}

/// **Running it twice does not throw away the drawing.**
///
/// The usual reason to run it again is a re-record, and an animator who has
/// drawn to the first two lines must keep that work when the third moves.
#[test]
fn running_it_again_keeps_what_was_drawn() {
    let (mut editor, layer, _dir) = editor_with_narration();
    editor.run(Command::FitToNarration);
    let first = keyframes(&editor, layer);
    assert!(!first.is_empty());

    // Draw something on the first line's keyframe.
    let frame = first[0];
    editor.doc.edit("draw", |scene| {
        use buzz_geom::{Rect, Shape as _};
        scene.add_shape_at(
            layer,
            frame,
            buzz_scene::ShapeData::filled(
                Rect::new(10.0, 10.0, 60.0, 60.0).to_path(1e-9),
                peniko::Color::from_rgb8(0x30, 0x60, 0xC0),
            ),
        );
    });
    let drawn_before = editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .map(|l| l.frames.resolved_at(frame).iter().count())
        .unwrap_or(0);
    assert!(drawn_before > 0, "nothing was drawn to check");

    editor.run(Command::FitToNarration);

    assert_eq!(keyframes(&editor, layer), first, "the lines moved on their own");
    let drawn_after = editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .map(|l| l.frames.resolved_at(frame).iter().count())
        .unwrap_or(0);
    assert_eq!(
        drawn_after, drawn_before,
        "the second run discarded the drawing on the first line"
    );
}

/// **No soundtrack says so**, rather than doing nothing without explanation.
#[test]
fn no_narration_explains_itself() {
    let mut editor = Editor::default();
    editor.run(Command::FitToNarration);
    let status = editor.status.clone().unwrap_or_default();
    assert!(
        status.contains("soundtrack"),
        "the reason should name what is missing; it said {status:?}"
    );
}

/// **One undo takes the whole layout back.** Thirty keyframes is thirty things
/// to remove by hand if it is not one step.
#[test]
fn laying_out_a_narration_is_one_undo_step() {
    let (mut editor, layer, _dir) = editor_with_narration();
    editor.run(Command::FitToNarration);
    let after = keyframes(&editor, layer).len();
    assert!(after > 1, "nothing was laid out to undo");

    editor.run(Command::Undo);

    assert!(
        keyframes(&editor, layer).len() < after,
        "the layout did not come back in one step: {:?}",
        keyframes(&editor, layer)
    );
}
