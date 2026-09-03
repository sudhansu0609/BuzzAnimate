//! Depth of field on the real GPU: an aperture blurs a layer off the focus
//! plane, a pinhole camera leaves it sharp, and a keyed focus **pull** takes
//! the same layer from one to the other over the length of a shot.
//!
//! This is Wave 9c's geometric half — the blur reuses the per-shape filter blur
//! rather than a per-pixel pass, so "blur" here means the crisp edge of a shape
//! softens. The test measures edge sharpness by how many pixels along a
//! horizontal scan sit between the background and the shape. Skips with no GPU.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{FocusKey, LayerKind, Scene, ShapeData};
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
    // Hold the artwork past frame 24, so the frames a focus pull is keyed at
    // have something on them to be out of focus.
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(25);
    });
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

/// **The focus pull.** The lens starts focused on the layer at depth 600 and
/// travels to the stage plane, so the layer is sharp on frame 0 and soft on
/// frame 24 — with nothing on the stage moving and the aperture unchanged.
///
/// This is the whole feature end to end: keys on the camera, resolved in the
/// shared document walk, on the exporter's real GPU. Rendering the *same
/// document* at two frames is what makes it a pull rather than two settings.
#[test]
fn a_focus_pull_takes_a_layer_out_of_focus_over_time() {
    with_exporter(|exporter| {
        let mut scene = document(0.0);
        {
            let cam = scene.camera_mut();
            cam.set_focus_key(FocusKey {
                frame: 0,
                focus_depth: 600.0,
                aperture: 0.05,
            });
            cam.set_focus_key(FocusKey {
                frame: 24,
                focus_depth: 0.0,
                aperture: 0.05,
            });
        }
        let settings = ExportSettings::for_stage(&scene);

        let start = exporter.render(&scene, 0, &settings).expect("frame 0");
        let end = exporter.render(&scene, 24, &settings).expect("frame 24");

        let start_band = edge_band(&start);
        let end_band = edge_band(&end);
        assert!(
            end_band > start_band + 2,
            "the focus should leave this layer behind: frame 0 {start_band}, frame 24 {end_band}"
        );

        // And halfway through it is halfway there, so the pull is a move rather
        // than a switch that flips at the second key.
        let middle_band = edge_band(&exporter.render(&scene, 12, &settings).expect("frame 12"));
        assert!(
            middle_band > start_band && middle_band < end_band,
            "midway should be between the two: {start_band} / {middle_band} / {end_band}"
        );
    });
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
