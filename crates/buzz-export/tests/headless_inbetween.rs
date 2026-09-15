//! **Inbetweening reaches pixels.**
//!
//! The pairing is unit-tested in `buzz-scene`; this proves it survives the whole
//! render path. Two keyframes of a two-stroke drawing, the strokes drawn in
//! opposite orders — which is how hand-drawn frames really arrive — and the
//! frame halfway between them must show both strokes near where they were, not
//! one of them crossing the stage to become the other.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData, Tween};
use peniko::Color;

const BG: Color = Color::from_rgb8(0x00, 0x00, 0x00);
const ART: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping inbetween test: no usable GPU ({e})"),
    }
}

/// Two squares in one path: a small one at the left, a large one at the right.
/// `swapped` draws them in the other order, which is the whole point.
fn two_strokes(swapped: bool, shift: f64) -> buzz_geom::BezPath {
    let small = Rect::new(40.0 + shift, 80.0, 80.0 + shift, 120.0).to_path(1e-9);
    let large = Rect::new(240.0 + shift, 60.0, 340.0 + shift, 140.0).to_path(1e-9);
    let (first, second) = if swapped { (large, small) } else { (small, large) };
    let mut path = first;
    for element in second.elements() {
        path.push(*element);
    }
    path
}

/// A shape tween between the two drawings, over 20 frames.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = BG;
    scene.stage_mut().size = buzz_geom::Size::new(400.0, 200.0);

    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(layer, ShapeData::filled(two_strokes(false, 0.0), ART));

    scene.update_layer(layer, |l| {
        while l.frames.length() <= 20 {
            l.frames.insert_frame(l.frames.length());
        }
        l.frames.insert_blank_keyframe(20);
    });
    // The far keyframe: the same two strokes, barely moved, drawn the other way
    // round.
    scene.add_shape_at(
        layer,
        20,
        ShapeData::filled(two_strokes(true, 8.0), ART),
    );
    scene.update_layer(layer, |l| {
        l.frames.set_tween(0, Tween::shape());
    });
    scene
}

/// How much ink is in a vertical band of the frame.
fn ink_between(frame: &Frame, x0: u32, x1: u32) -> usize {
    let mut ink = 0;
    for y in 0..frame.height {
        for x in x0..x1.min(frame.width) {
            if frame.pixel(x, y)[0] > 120 {
                ink += 1;
            }
        }
    }
    ink
}

/// **Both strokes stay where they belong.** Paired by draw order, the small
/// stroke would set off across the stage to become the large one, and the
/// middle of the tween would have ink stranded between them.
#[test]
fn a_two_stroke_drawing_morphs_stroke_for_stroke() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        let middle = exporter.render(&scene, 10, &settings).expect("frame 10");

        let left = ink_between(&middle, 20, 120);
        let centre = ink_between(&middle, 130, 220);
        let right = ink_between(&middle, 230, 360);

        assert!(left > 500, "the small stroke is still on the left: {left}");
        assert!(right > 3000, "and the large one on the right: {right}");
        assert!(
            centre < left / 2,
            "and nothing is stranded in between: {centre} in the middle against {left} on the left"
        );
    });
}

/// The ends of the tween are the drawings themselves, whatever the pairing did
/// in between.
#[test]
fn the_ends_of_the_tween_are_the_drawings() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);

        let first = exporter.render(&scene, 0, &settings).expect("frame 0");
        let last = exporter.render(&scene, 20, &settings).expect("frame 20");

        // Both drawings have the same two strokes, so both ends carry ink at
        // each side and none stranded in the middle.
        for (name, frame) in [("first", &first), ("last", &last)] {
            assert!(
                ink_between(frame, 20, 120) > 500,
                "{name} frame has its small stroke"
            );
            assert!(
                ink_between(frame, 230, 360) > 3000,
                "{name} frame has its large stroke"
            );
        }
    });
}
