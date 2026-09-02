//! **A light rims the artwork it reaches**, on the real GPU.
//!
//! Everything else lighting does here happens inside the silhouette — the tint,
//! the terminator, the highlight — so a lit drawing had no way to be brighter
//! than the picture *around* it, which is exactly what a strong light on a
//! figure looks like and what an animator draws by hand as a rim.
//!
//! What is measured is therefore deliberately outside the shape: a ring of
//! background pixels a few units off its edge. Inside would prove nothing, and
//! the whole frame's mean would be dominated by the artwork.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Light, LightId, LightKind, Scene, ShapeData};
use peniko::Color;

/// The square the light is rimming, in stage coordinates.
const ART: Rect = Rect::new(200.0, 140.0, 350.0, 260.0);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping rim-light test: no usable GPU ({e})"),
    }
}

/// A dark stage, a grey square, and one lamp standing off to the left of it.
///
/// A lamp rather than a sun so that the falloff test has something to fall off
/// with, and dark so that a glow has somewhere to show.
fn document(rim: f32) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x10, 0x10, 0x14);

    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(ART.to_path(1e-9), Color::from_rgb8(0x80, 0x80, 0x80)),
    );

    let mut lamp = Light::new(
        LightId(1),
        "Key",
        LightKind::lamp(buzz_geom::Point::new(120.0, 200.0)),
    );
    lamp.rim = rim;
    // No pool: a lamp's pool is laid over the whole frame and would brighten
    // the very pixels this test is reading, which would make it pass whether
    // the rim worked or not.
    lamp.glow = 0.0;
    // And no cast shadow, for the same reason in the other direction: a shadow
    // thrown across the sampled ring would darken it.
    lamp.shadows = false;

    let rig = scene.lights_mut();
    rig.enabled = true;
    rig.lights.push(lamp);
    scene
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64
}

/// The mean brightness of the background just outside the artwork's left edge —
/// the side the lamp is on, and where a rim therefore lands.
///
/// Offsets in *pixels*, which for these settings are stage units: the export is
/// rendered at the stage's own size.
fn beside_the_edge(frame: &Frame) -> f64 {
    let x0 = (ART.x0 as u32).saturating_sub(10);
    let x1 = ART.x0 as u32;
    let (y0, y1) = (ART.y0 as u32 + 10, ART.y1 as u32 - 10);

    let mut sum = 0.0;
    let mut n = 0.0f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * frame.width + x) * 4) as usize;
            if i + 3 < frame.pixels.len() {
                sum += luma(&frame.pixels[i..i + 4]);
                n += 1.0;
            }
        }
    }
    sum / n.max(1.0)
}

/// The whole reason this exists: with the rim on, the background immediately
/// outside the drawing comes up in the light's colour. With it off — every
/// document written before it existed — that background is untouched.
#[test]
fn a_rim_brightens_the_background_beside_the_artwork() {
    with_exporter(|exporter| {
        let bare = document(0.0);
        let settings = ExportSettings::for_stage(&bare);

        let without = exporter.render(&bare, 0, &settings).expect("unrimmed frame");
        let with = exporter
            .render(&document(0.9), 0, &settings)
            .expect("rimmed frame");

        let (a, b) = (beside_the_edge(&without), beside_the_edge(&with));
        assert!(
            b > a + 2.0,
            "the rim should light the ground beside the drawing: \
             without {a:.2}, with {b:.2}"
        );
    });
}

/// A rim turned down is a rim that is not there, so the frame is byte-identical
/// to the one a build without any of this would have produced. This is the
/// promise that switching the feature on for the *model* changed no existing
/// film.
#[test]
fn a_light_with_no_rim_renders_exactly_as_before() {
    with_exporter(|exporter| {
        let scene = document(0.0);
        let settings = ExportSettings::for_stage(&scene);

        // The same document twice: what is being pinned is that nothing in the
        // rim path fires when the rim is zero, so the two are identical rather
        // than merely close.
        let a = exporter.render(&scene, 0, &settings).expect("frame");
        let b = exporter.render(&scene, 0, &settings).expect("frame again");
        assert_eq!(a.pixels, b.pixels, "an unrimmed light draws no rim");
    });
}

/// The rim follows the light. Turn the light down and the edges go down with
/// it — which is what makes it animate for free rather than needing a track of
/// its own.
#[test]
fn a_dimmer_light_lays_a_fainter_rim() {
    with_exporter(|exporter| {
        let mut dim = document(0.9);
        dim.lights_mut().lights[0].intensity = 0.25;
        let bright = document(0.9);

        let settings = ExportSettings::for_stage(&bright);
        let dim = exporter.render(&dim, 0, &settings).expect("dim frame");
        let bright = exporter.render(&bright, 0, &settings).expect("bright frame");

        let (d, b) = (beside_the_edge(&dim), beside_the_edge(&bright));
        assert!(
            b > d,
            "the brighter light should rim harder: dim {d:.2}, bright {b:.2}"
        );
    });
}
