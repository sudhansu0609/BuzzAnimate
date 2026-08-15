//! The compositor reaches the exported film, not only the stage.
//!
//! The parity guarantee in `ARCHITECTURE.md` is that the window and the
//! exporter run the same `Compositor::run`. This proves the exporter half: a
//! document with a vignette exports a frame whose corners are darker than its
//! centre, and a document with the look switched off exports the raw artwork
//! unchanged. Skips cleanly with no GPU.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use buzz_geom::{Rect, Shape as _};
use peniko::Color;

const MID: Color = Color::from_rgb8(0xB4, 0xB4, 0xB4);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping post test: no usable GPU ({e})"),
    }
}

/// A mid-grey stage, filled edge to edge so a vignette has something to darken.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = MID;
    scene.stage_mut().size = buzz_geom::Size::new(200.0, 200.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(0.0, 0.0, 200.0, 200.0).to_path(1e-9), MID),
    );
    scene
}

fn render(exporter: &mut Exporter, scene: &Scene) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * frame.width + x) * 4) as usize;
    [
        frame.pixels[i],
        frame.pixels[i + 1],
        frame.pixels[i + 2],
        frame.pixels[i + 3],
    ]
}

#[test]
fn a_vignette_reaches_the_exported_frame() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.stage_mut().post.enabled = true;
        scene.stage_mut().post.vignette.enabled = true;
        scene.stage_mut().post.vignette.amount = 0.9;
        scene.stage_mut().post.vignette.softness = 0.8;

        let frame = render(exporter, &scene);
        let centre = pixel(&frame, frame.width / 2, frame.height / 2)[0];
        let corner = pixel(&frame, 1, 1)[0];
        assert!(
            corner < centre,
            "the exported corner ({corner}) should be darker than the centre ({centre})"
        );
    });
}

#[test]
fn no_effects_exports_the_raw_artwork() {
    with_exporter(|exporter| {
        let scene = document();
        let frame = render(exporter, &scene);
        let centre = pixel(&frame, frame.width / 2, frame.height / 2);
        // A plain document is untouched: the mid-grey survives to the film.
        assert_eq!(centre[0], 0xB4, "the artwork must not be graded when off");
        assert_eq!(centre[1], 0xB4);
        assert_eq!(centre[2], 0xB4);
    });
}
