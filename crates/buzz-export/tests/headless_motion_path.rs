//! **An object baked along a motion path renders, and it travels**, on the
//! real GPU.
//!
//! `buzz_act::follow_path` has unit tests for every transform it writes; what
//! none of them can say is that the *renderer* then draws the object in those
//! places. A shape whose keyframes are right but which never reaches the stage,
//! or which is drawn once and held, is the failure this catches — and the only
//! place a moving object can be seen moving is in the pixels of two frames.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_act::{MotionPathOptions, follow_path};
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{Easing, LayerKind, Object, ObjectId, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping motion-path test: no usable GPU ({e})"),
    }
}

/// A dark stage with one bright square, baked travelling along `path` over
/// `frames`, committed the way the editor commits one: a keyframe on twos with
/// tweens between.
fn document(path: &BezPath, frames: u32, orient: bool) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Mover", LayerKind::Normal);

    // A 44-wide square centred on its own origin, so its placement is exactly
    // the path point.
    let mut object = Object::shape(
        ObjectId(1),
        ShapeData::filled(
            Rect::new(-22.0, -22.0, 22.0, 22.0).to_path(1e-9),
            Color::from_rgb8(0xF0, 0xF0, 0xF6),
        ),
    );
    object.transform = Affine::IDENTITY;
    let id = scene.add_object(layer, object).expect("object on a layer");

    let opts = MotionPathOptions {
        easing: Easing::Linear,
        orient_to_path: orient,
        step: 2,
        ..MotionPathOptions::new(0..frames)
    };
    let report = follow_path(&mut scene, id, path, &opts).expect("the path was baked");
    assert!(report.keyframes > 1, "a path of one keyframe is a hold");
    scene
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64
}

/// The mean x of the bright pixels, and how many there were — where the object
/// is on screen, and proof it is on screen at all.
fn bright_centroid_x(frame: &Frame) -> (f64, usize) {
    let (mut sum_x, mut n) = (0.0, 0usize);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let i = ((y * frame.width + x) * 4) as usize;
            if i + 3 < frame.pixels.len() && luma(&frame.pixels[i..i + 4]) > 80.0 {
                sum_x += x as f64;
                n += 1;
            }
        }
    }
    (sum_x / n.max(1) as f64, n)
}

/// A straight eastbound path across the middle of the stage.
fn eastbound() -> BezPath {
    let mut path = BezPath::new();
    path.move_to(Point::new(90.0, 200.0));
    path.line_to(Point::new(460.0, 200.0));
    path
}

/// **The object is drawn, and it is somewhere different at the end than at the
/// start.** A baked path that rendered the object once and held it would put
/// the same bright square in the same place on every frame.
#[test]
fn a_baked_object_travels_across_the_stage() {
    with_exporter(|exporter| {
        let scene = document(&eastbound(), 25, false);
        let settings = ExportSettings::for_stage(&scene);

        let first = exporter.render(&scene, 0, &settings).expect("frame 0");
        let last = exporter.render(&scene, 24, &settings).expect("the last frame");

        let (start_x, start_n) = bright_centroid_x(&first);
        let (end_x, end_n) = bright_centroid_x(&last);

        assert!(start_n > 400, "the object did not reach the stage at the start ({start_n} px)");
        assert!(end_n > 400, "the object was gone by the end ({end_n} px)");
        assert!(
            end_x - start_x > 150.0,
            "the object barely moved: centroid went from x={start_x:.0} to x={end_x:.0}"
        );
    });
}

/// The midpoint of a straight traverse is halfway along it — a still would have
/// the square in one place for all three renders.
#[test]
fn the_midpoint_frame_sits_between_the_ends() {
    with_exporter(|exporter| {
        let scene = document(&eastbound(), 25, false);
        let settings = ExportSettings::for_stage(&scene);

        let start_x = bright_centroid_x(&exporter.render(&scene, 0, &settings).unwrap()).0;
        let mid_x = bright_centroid_x(&exporter.render(&scene, 12, &settings).unwrap()).0;
        let end_x = bright_centroid_x(&exporter.render(&scene, 24, &settings).unwrap()).0;

        assert!(
            mid_x > start_x + 50.0 && mid_x < end_x - 50.0,
            "the middle frame ({mid_x:.0}) is not between the ends ({start_x:.0}, {end_x:.0})"
        );
    });
}
