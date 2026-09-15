//! **A tiling texture reaches pixels, on the real GPU.**
//!
//! A rectangle filled with a two-colour procedural checker, tiled across it,
//! must render *both* colours in quantity — that only happens if the baked tile
//! became an image brush and `Extend::Repeat` tiled it over the shape. One solid
//! colour would mean the texture never took, or never repeated.
//!
//! Skips with no GPU, like every other headless test here.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    FillSpec, ImageAsset, ImageFill, ImageId, LayerKind, Object, ObjectId, Scene, ShapeData,
    TextureKind,
};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping texture test: no usable GPU ({e})"),
    }
}

/// A dark stage with one big rectangle carrying a red/blue checker, tiled small
/// enough that many cells fall inside it.
fn checkered() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Tex", LayerKind::Normal);

    let fg = Color::from_rgb8(0xE0, 0x20, 0x20);
    let bg = Color::from_rgb8(0x20, 0x40, 0xE0);
    let px = buzz_scene::texture::tile(TextureKind::Checker, 64, fg, bg);
    let asset = Arc::new(ImageAsset::from_pixels(ImageId(1), "checker", 64, 64, Arc::new(px)));

    let rect = Rect::new(-160.0, -110.0, 160.0, 110.0).to_path(1e-9);
    let mut shape = ShapeData::filled(rect, Color::WHITE);
    shape.fill = Some(FillSpec::image(ImageFill::tiled(asset, 56.0)));
    let mut obj = Object::shape(ObjectId(1), shape);
    obj.transform = Affine::translate((275.0, 200.0));
    scene.add_object(layer, obj).expect("the object on a layer");
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
fn a_tiling_texture_shows_both_of_its_colours() {
    with_exporter(|exporter| {
        let scene = checkered();
        let settings = ExportSettings::for_stage(&scene);
        let (red, blue) = colour_tally(&exporter.render(&scene, 0, &settings).unwrap());
        assert!(
            red > 1000 && blue > 1000,
            "a tiled checker should show plenty of both colours: {red} red, {blue} blue"
        );
    });
}
