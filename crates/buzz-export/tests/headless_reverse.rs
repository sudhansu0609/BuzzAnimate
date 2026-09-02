//! **A turnaround shows the back, on the real GPU.**
//!
//! An object with a red front and a blue reverse: facing the camera it renders
//! red, turned a half-turn it renders blue — the whole point of the feature is a
//! *different drawing* when the object faces away, not the front mirrored. The
//! projection already does the perspective; this proves the swap reaches pixels.
//!
//! Skips with no GPU, like every other headless test here.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Object, ObjectId, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping reverse test: no usable GPU ({e})"),
    }
}

/// A dark stage with one big square centred on it: red front, blue back,
/// yawed by `rotation_y`.
fn turnaround(rotation_y: f64) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Turn", LayerKind::Normal);

    let square = || Rect::new(-120.0, -120.0, 120.0, 120.0).to_path(1e-9);
    let mut front = Object::shape(
        ObjectId(1),
        ShapeData::filled(square(), Color::from_rgb8(0xE0, 0x20, 0x20)),
    );
    front.transform = Affine::translate((275.0, 200.0));
    front.spatial.rotation_y = rotation_y;
    front.reverse = Some(Arc::new(Object::shape(
        ObjectId(2),
        ShapeData::filled(square(), Color::from_rgb8(0x20, 0x40, 0xE0)),
    )));
    scene.add_object(layer, front).expect("the object on a layer");
    scene
}

/// Count strongly-red and strongly-blue pixels in the frame.
fn colour_tally(frame: &Frame) -> (usize, usize) {
    let (mut red, mut blue) = (0, 0);
    for px in frame.pixels.chunks(4) {
        let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
        if r > 120 && r > g + 40 && r > b + 40 {
            red += 1;
        } else if b > 120 && b > r + 40 && b > g + 40 {
            blue += 1;
        }
    }
    (red, blue)
}

#[test]
fn a_facing_object_shows_its_front_and_a_turned_one_its_back() {
    with_exporter(|exporter| {
        let facing = turnaround(0.0);
        let settings = ExportSettings::for_stage(&facing);
        let (red_front, blue_front) = colour_tally(&exporter.render(&facing, 0, &settings).unwrap());
        assert!(
            red_front > 2000 && red_front > blue_front,
            "facing the camera should be red (front): {red_front} red, {blue_front} blue"
        );

        // A half-turn: the object faces away, so the blue back shows instead.
        let turned = turnaround(std::f64::consts::PI);
        let settings = ExportSettings::for_stage(&turned);
        let (red_back, blue_back) = colour_tally(&exporter.render(&turned, 0, &settings).unwrap());
        assert!(
            blue_back > 2000 && blue_back > red_back,
            "turned around should be blue (the back): {red_back} red, {blue_back} blue"
        );
    });
}
