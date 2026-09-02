//! **An effect stroke renders**, on the real GPU.
//!
//! The effect brushes commit ordinary artwork — vector shapes, gradient
//! glows, bitmap fills, some of it compositing additively, all of it inside a
//! group. Every piece of that chain has its own unit tests; what none of them
//! can say is that the *renderer* honours the combination — an additive
//! image fill inside a group on a normal layer is exactly the kind of shape
//! that can quietly disappear. So this draws the two extreme effects on a
//! dark stage and reads the pixels back.
//!
//! Skips with no GPU, like every other headless test here.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::Point;
use buzz_render::GpuPreference;
use buzz_scene::{
    EffectKind, ArtPiece, EffectStroke, LayerKind, Object, Scene, ShapeData, effect_artwork,
};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping effect-brush test: no usable GPU ({e})"),
    }
}

/// A dark stage with one effect stroke on it, committed the way the editor
/// commits one: bitmap pieces through the image library, everything grouped.
fn document(kind: EffectKind, color: Color) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Effect", LayerKind::Normal);

    let samples: Vec<buzz_geom::StrokeSample> = (0..60)
        .map(|i| {
            let t = i as f64 / 59.0;
            buzz_geom::StrokeSample::new(Point::new(80.0 + t * 380.0, 200.0), t)
        })
        .collect();
    let pieces = effect_artwork(
        kind,
        &EffectStroke {
            samples: &samples,
            size: 30.0,
            color,
            conditioning: buzz_geom::Conditioning::smoothing(0.5),
        },
    );
    assert!(!pieces.is_empty(), "{kind:?} made nothing to draw");

    let shapes: Vec<ShapeData> = pieces
        .iter()
        .map(|piece| match piece {
            ArtPiece::Shape(shape) => shape.clone(),
            ArtPiece::Painting {
                canvas,
                brush,
                blend,
            } => {
                let id = scene.next_image_id();
                let asset = scene
                    .images_mut()
                    .insert(canvas.to_asset(id, "Effect", brush));
                let area = canvas.area();
                let mut fill = buzz_scene::ImageFill::new(asset, area);
                fill.smooth = false;
                ShapeData {
                    path: buzz_geom::Shape::to_path(&area, 1e-9),
                    fill: Some(buzz_scene::FillSpec::image(fill)),
                    stroke: None,
                    blend: *blend,
                }
            }
        })
        .collect();

    let children: Vec<Arc<Object>> = shapes
        .into_iter()
        .map(|s| {
            let id = scene.next_object_id();
            Arc::new(Object::shape(id, s))
        })
        .collect();
    let id = scene.next_object_id();
    scene.add_object(layer, Object::group(id, children));
    scene
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64
}

/// Mean brightness of the band the stroke ran through.
fn along_the_stroke(frame: &Frame) -> f64 {
    let mut sum = 0.0;
    let mut n = 0.0f64;
    for y in 170..230u32 {
        for x in 80..460u32 {
            let i = ((y * frame.width + x) * 4) as usize;
            if i + 3 < frame.pixels.len() {
                sum += luma(&frame.pixels[i..i + 4]);
                n += 1.0;
            }
        }
    }
    sum / n.max(1.0)
}

/// Diffused light is the chain's worst case — painted pixels, in an image
/// fill, compositing *additively*, inside a group — and it must visibly
/// brighten a dark stage.
#[test]
fn a_wash_of_diffused_light_brightens_a_dark_stage() {
    with_exporter(|exporter| {
        let bare = {
            let mut s = Scene::default();
            s.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
            s
        };
        let settings = ExportSettings::for_stage(&bare);
        let dark = exporter.render(&bare, 0, &settings).expect("bare frame");

        let lit = document(
            EffectKind::DiffusedLight,
            Color::from_rgb8(0xFF, 0xD9, 0x9A),
        );
        let glow = exporter.render(&lit, 0, &settings).expect("lit frame");

        let (a, b) = (along_the_stroke(&dark), along_the_stroke(&glow));
        assert!(
            b > a + 8.0,
            "the wash should visibly brighten the stroke's path: bare {a:.2}, lit {b:.2}"
        );
    });
}

/// Snow is the other extreme — hundreds of little vector fills in depth
/// buckets — and enough of it must land to read as weather, not as three
/// stray dots.
#[test]
fn a_snow_stroke_scatters_visible_flakes() {
    with_exporter(|exporter| {
        let scene = document(EffectKind::Snow, Color::from_rgb8(0xF0, 0xF4, 0xFF));
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("snow frame");

        let mut bright = 0usize;
        for y in 120..280u32 {
            for x in 40..500u32 {
                let i = ((y * frame.width + x) * 4) as usize;
                if i + 3 < frame.pixels.len() && luma(&frame.pixels[i..i + 4]) > 90.0 {
                    bright += 1;
                }
            }
        }
        assert!(
            bright > 300,
            "a snow stroke should leave hundreds of bright pixels, found {bright}"
        );
    });
}
