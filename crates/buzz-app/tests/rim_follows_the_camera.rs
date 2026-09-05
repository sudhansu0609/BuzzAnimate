//! **A light's rim stays on the artwork when the camera moves.**
//!
//! # The defect this pins
//!
//! A key light lays a *rim* around a layer's artwork — Animate's Glow filter,
//! laid by a light instead of by hand. Filter strokes are drawn by handing the
//! rasteriser the geometry and, separately, a transform that shapes the **pen**
//! rather than the path: that is what lets a blur's round pen become the
//! ellipse it needs without distorting the outline.
//!
//! `draw_ops` cancelled that transform out of the geometry before handing it
//! over, and the stroker cancels it again internally — so the rim was drawn at
//! the path's own coordinates with the camera's transform divided back out. It
//! landed as a **second, hollow copy of every lit figure**, sitting where the
//! artwork would be if the camera were not there.
//!
//! It was invisible for as long as it existed, because with no camera and a
//! round pen the transform is the identity twice over. Every scene the director
//! makes has a camera — it always frames its shot — so every automatically
//! directed film was carrying ghost outlines of its cast.
//!
//! # How this catches it
//!
//! Two overlapping blocks (so the light treats them as one figure), a key light
//! that rims them, and a camera pushed well off centre so the artwork lands to
//! the right of where it was drawn. A row of pixels is then read straight
//! across **the coordinates the blocks were drawn at**, which the camera has
//! moved the artwork out of: flat ground, and nothing else, unless the ghost is
//! back.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{CameraKey, LayerKind, Scene, ShapeData};
use peniko::Color;

const STAGE_W: f64 = 1920.0;
const STAGE_H: f64 = 1080.0;
/// Where the blocks are drawn, in document units.
const BLOCKS: (f64, f64) = (900.0, 1020.0);
/// Where the camera is centred. Well off the middle, so the artwork moves
/// clear of the coordinates it was drawn at.
const CAMERA_X: f64 = 650.0;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping rim test: no usable GPU ({e})"),
    }
}

/// Two overlapping blocks on lit ground, framed from one side.
///
/// `camera` puts the shot off centre; `None` leaves the camera off, which is
/// the arrangement that was always correct and is here as the control.
fn stage(camera: Option<f64>) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(STAGE_W, STAGE_H);

    // Ground, wide enough that the row read below is a flat colour whatever the
    // camera does. Its own layer, behind the blocks.
    let ground = scene.add_stage_layer("Ground", LayerKind::Normal);
    scene.add_shape(
        ground,
        ShapeData::filled(
            Rect::new(-STAGE_W, 620.0, STAGE_W * 2.0, STAGE_H * 2.0).to_path(1e-9),
            Color::from_rgb8(0x3E, 0x5A, 0x36),
        ),
    );

    let layer = scene.add_stage_layer("Blocks", LayerKind::Normal);
    for (rect, colour) in [
        (
            Rect::new(BLOCKS.0, 600.0, BLOCKS.1, 960.0),
            Color::from_rgb8(0xC0, 0x50, 0x30),
        ),
        (
            Rect::new(940.0, 520.0, 1000.0, 660.0),
            Color::from_rgb8(0x30, 0x50, 0xC0),
        ),
    ] {
        scene.add_shape(layer, ShapeData::filled(rect.to_path(1e-9), colour));
    }

    // A sun high on the left. What matters is only that it lays a rim at all.
    let sun = scene.add_light(buzz_scene::LightKind::Sun {
        azimuth: -0.6,
        elevation: 0.9,
    });
    if let Some(light) = scene.lights_mut().get_mut(sun) {
        light.intensity = 1.2;
        // **The rim is the whole subject.** It is off by default, and a light
        // that lays none has nothing to lay in the wrong place -- so without
        // this the test passes on a scene the bug could never have touched.
        light.rim = 0.9;
        // No shadow: a shadow is a legitimate mark on the ground, and this test
        // is about marks that are not legitimate.
        light.shadows = false;
    }

    if let Some(centre) = camera {
        let track = scene.camera_mut();
        track.enabled = true;
        track.set_key(CameraKey::new(0, Point::new(centre, STAGE_H / 2.0)).clamped());
    }
    scene
}

/// The worst colour difference along one row of the frame, between `from` and
/// `to` as fractions of the width, measured against the row's first pixel.
fn variation_across(frame: &Frame, row: f64, from: f64, to: f64) -> u8 {
    let y = ((frame.height as f64 * row) as u32).min(frame.height - 1);
    let at = |x: u32| (((y * frame.width + x) * 4) as usize);
    let reference = frame.pixels[at(2)..at(2) + 3].to_vec();

    let x0 = (frame.width as f64 * from) as u32;
    let x1 = ((frame.width as f64 * to) as u32).min(frame.width - 1);
    let mut worst = 0u8;
    for x in x0..=x1 {
        for c in 0..3 {
            worst = worst.max(frame.pixels[at(x) + c].abs_diff(reference[c]));
        }
    }
    worst
}

/// The window the blocks were *drawn* in, as fractions of the frame — which is
/// where a ghost lands, and which the camera has moved the artwork out of.
fn ghost_window() -> (f64, f64) {
    (
        (BLOCKS.0 - 30.0) / STAGE_W,
        (BLOCKS.1 + 30.0) / STAGE_W,
    )
}

/// **Nothing is drawn where the artwork used to be.**
#[test]
fn a_rim_does_not_leave_a_ghost_at_the_artworks_own_coordinates() {
    with_exporter(|exporter| {
        let scene = stage(Some(CAMERA_X));
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("a frame");

        let (from, to) = ghost_window();
        // Low on the frame, through the blocks' own footprint and well clear of
        // where the camera has actually put them.
        let ink = variation_across(&frame, 0.85, from, to);
        assert!(
            ink <= 8,
            "something is drawn across the coordinates the blocks were drawn at \
             (worst channel difference {ink}) — the rim is being laid at the \
             artwork's own coordinates instead of where the camera puts it"
        );
    });
}

/// **The control**: the same row is flat when the camera is off, so the test
/// above is measuring a ghost and not the ground.
#[test]
fn that_row_is_flat_ground_when_nothing_is_lit() {
    with_exporter(|exporter| {
        let mut scene = stage(Some(CAMERA_X));
        scene.lights_mut().enabled = false;
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("a frame");

        let (from, to) = ghost_window();
        let ink = variation_across(&frame, 0.85, from, to);
        assert!(ink <= 8, "the unlit control is not flat either ({ink})");
    });
}

/// **And the camera really did move the artwork**, so the test above is not
/// passing because nothing happened.
#[test]
fn the_camera_actually_moves_the_artwork() {
    with_exporter(|exporter| {
        let settings = ExportSettings::for_stage(&stage(None));
        let still = exporter.render(&stage(None), 0, &settings).expect("still");
        let moved = exporter
            .render(&stage(Some(CAMERA_X)), 0, &settings)
            .expect("moved");

        let differing = still
            .pixels
            .chunks(4)
            .zip(moved.pixels.chunks(4))
            .filter(|(a, b)| a[0].abs_diff(b[0]) > 6 || a[2].abs_diff(b[2]) > 6)
            .count();
        assert!(
            differing > 1_000,
            "the camera moved almost nothing ({differing} pixels)"
        );
    });
}
