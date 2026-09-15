//! **A render region crops to its rectangle, on the real GPU.**
//!
//! A small red square sits in one corner of a dark stage. Rendered whole, the
//! frame is mostly background with a little red; rendered with the region set to
//! the square's rectangle, the frame is almost entirely red — the region framed
//! and filled the output with just that corner. That is the whole feature.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Object, ObjectId, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping region test: no usable GPU ({e})"),
    }
}

fn red_fraction(frame: &Frame) -> f64 {
    let mut red = 0usize;
    let total = (frame.pixels.len() / 4).max(1);
    for px in frame.pixels.chunks(4) {
        if px[0] as i32 > 140 && px[0] as i32 > px[1] as i32 + 50 && px[0] as i32 > px[2] as i32 + 50
        {
            red += 1;
        }
    }
    red as f64 / total as f64
}

fn scene_with_a_corner_square() -> (Scene, Rect) {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let square = Rect::new(60.0, 60.0, 120.0, 120.0);
    let obj = Object::shape(
        ObjectId(1),
        ShapeData::filled(square.to_path(1e-9), Color::from_rgb8(0xE0, 0x20, 0x20)),
    );
    scene.add_object(layer, obj).expect("the object on a layer");
    (scene, square)
}

#[test]
fn a_region_renders_only_its_rectangle() {
    with_exporter(|exporter| {
        let (scene, square) = scene_with_a_corner_square();

        let full = ExportSettings::for_stage(&scene);
        let whole = red_fraction(&exporter.render(&scene, 0, &full).unwrap());
        assert!(whole < 0.2, "the square is a small part of the whole stage: {whole}");

        let region = ExportSettings { width: 100, height: 100, region: Some(square), ..full };
        let cropped = red_fraction(&exporter.render(&scene, 0, &region).unwrap());
        assert!(
            cropped > 0.8,
            "the region should be almost all the red square: {cropped}"
        );
    });
}
