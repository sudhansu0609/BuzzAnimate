//! Depth of field on the real GPU: an aperture blurs a layer off the focus
//! plane, and a pinhole camera leaves it sharp.
//!
//! This is Wave 9c's geometric half — the blur reuses the per-shape filter blur
//! rather than a per-pixel pass, so "blur" here means the crisp edge of a shape
//! softens. The test measures edge sharpness by how many pixels along a
//! horizontal scan sit between the background and the shape. Skips with no GPU.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

const BG: Color = Color::from_rgb8(0x00, 0x00, 0x00);
const ART: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping DOF test: no usable GPU ({e})"),
    }
}

/// A black stage with one white square on a layer pushed well off the focus
/// plane.
fn document(aperture: f64) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = BG;
    scene.stage_mut().size = buzz_geom::Size::new(300.0, 200.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.update_layer(layer, |l| l.depth = 600.0);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(100.0, 60.0, 200.0, 140.0).to_path(1e-9), ART),
    );
    let cam = scene.camera_mut();
    cam.aperture = aperture;
    cam.focus_depth = 0.0;
    scene
}

/// Count the pixels on the middle scanline whose luma is between the background
/// and the shape — the transition band whose width is the edge softness.
fn edge_band(frame: &Frame) -> usize {
    let y = frame.height / 2;
    let mut band = 0;
    for x in 0..frame.width {
        let i = ((y * frame.width + x) * 4) as usize;
        let l = frame.pixels[i]; // grey, so red channel is luma
        if l > 20 && l < 235 {
            band += 1;
        }
    }
    band
}

#[test]
fn an_aperture_softens_an_out_of_focus_layer() {
    with_exporter(|exporter| {
        let sharp = document(0.0);
        let blurred = document(0.05);
        let s = ExportSettings::for_stage(&sharp);

        let sharp_frame = exporter.render(&sharp, 0, &s).expect("sharp");
        let blur_frame = exporter.render(&blurred, 0, &s).expect("blurred");

        let sharp_band = edge_band(&sharp_frame);
        let blur_band = edge_band(&blur_frame);
        assert!(
            blur_band > sharp_band + 2,
            "an out-of-focus layer should have a wider edge: sharp {sharp_band}, blurred {blur_band}"
        );
    });
}
