//! **A guide layer is reference, and never reaches the film.**
//!
//! # The defect this pins
//!
//! `LayerKind::paints_to_output` has said since it was written that a guide
//! does not paint into the output, and it had its own unit tests saying so.
//! Nothing read it. The render walk asked `paints_on_stage` — the *authoring*
//! question, which a guide answers yes to, because being visible while you draw
//! is the whole point of one — so a guide was drawn into the export as well,
//! faded to about a third.
//!
//! Faded is why it lasted: it read as a bit of dim background rather than as a
//! mistake. What it actually means is that a photograph put on a guide layer to
//! trace over is *delivered*, at 35%, over the drawing made from it.
//!
//! Found the honest way: a script filed a moon in the asset library, hid the
//! drawing it was made from on a guide layer, and the moon turned up in the
//! daylit forest shot anyway.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping guide test: no usable GPU ({e})"),
    }
}

/// A white stage with one scarlet block on it, on a layer of the given kind.
fn stage(kind: LayerKind) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(400.0, 300.0);
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_stage_layer("Reference", kind);
    scene.add_shape(
        layer,
        ShapeData::filled(
            Rect::new(100.0, 80.0, 300.0, 220.0).to_path(1e-9),
            Color::from_rgb8(0xE0, 0x10, 0x10),
        ),
    );
    scene
}

/// How much red is anywhere in the frame, above the white background.
fn reddest(frame: &Frame) -> i32 {
    frame
        .pixels
        .chunks(4)
        .map(|p| p[0] as i32 - p[2] as i32)
        .max()
        .unwrap_or(0)
}

/// **The control**: on an ordinary layer the block is obviously there, so the
/// measurement is measuring something.
#[test]
fn an_ordinary_layer_is_in_the_film() {
    with_exporter(|exporter| {
        let scene = stage(LayerKind::Normal);
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("a frame");
        assert!(
            reddest(&frame) > 100,
            "the block should be plainly in the picture, got {}",
            reddest(&frame)
        );
    });
}

/// **And on a guide layer it is not**, at any strength at all.
#[test]
fn a_guide_layer_is_not_in_the_film() {
    with_exporter(|exporter| {
        let scene = stage(LayerKind::Guide);
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("a frame");
        let red = reddest(&frame);
        assert!(
            red < 8,
            "a guide layer reached the film (redness {red}) — it is reference \
             geometry and the audience must never see it"
        );
    });
}

/// **The stage still shows it**, because that is what a guide is for. Rendered
/// through `render_with`, which is the authoring pass, with guides on.
#[test]
fn the_stage_still_shows_a_guide() {
    with_exporter(|exporter| {
        let scene = stage(LayerKind::Guide);
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter
            .render_with(
                &scene,
                0,
                &settings,
                &buzz_render::document::FrameOptions {
                    lit: true,
                    guides: true,
                    ..buzz_render::document::FrameOptions::default()
                },
            )
            .expect("a frame");
        assert!(
            reddest(&frame) > 20,
            "the stage stopped showing guides, which is what they are for"
        );
    });
}
