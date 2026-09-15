//! **An object sent along a drawn curve, without a screen.**
//!
//! The animator draws an L-shaped path and picks a range; `follow_path` should
//! leave the object standing at the start of the curve on the first frame, at
//! the end on the last, and travelling along it — round the corner — in between.
//! Two things that could quietly go wrong are checked directly: that the
//! object's own size survives the move, and that orienting turns it to face the
//! way the path heads rather than leaving it staring one direction the whole
//! time.

use buzz_act::{MotionPathOptions, follow_path};
use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _};
use buzz_scene::{Easing, LayerKind, Object, ObjectId, Scene, ShapeData};
use peniko::Color;

/// A right-angled path: 100 units east, then 100 units north. Arc length 200,
/// so the corner at `(100, 0)` sits at exactly half way.
fn corner_path() -> BezPath {
    let mut path = BezPath::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(100.0, 0.0));
    path.line_to(Point::new(100.0, 100.0));
    path
}

/// A small square object carrying a deliberate rotation and 2x scale, so the
/// test can tell whether the move disturbed either.
fn scene_with_object(transform: Affine) -> (Scene, buzz_scene::LayerId, ObjectId) {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Mover", LayerKind::Normal);
    let mut object = Object::shape(
        ObjectId(1),
        ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
    );
    object.transform = transform;
    let id = scene.add_object(layer, object).expect("object went on a layer");
    (scene, layer, id)
}

fn translation(a: Affine) -> Point {
    let t = a.translation();
    Point::new(t.x, t.y)
}
fn angle(a: Affine) -> f64 {
    let c = a.as_coeffs();
    c[1].atan2(c[0])
}
fn scale_x(a: Affine) -> f64 {
    let c = a.as_coeffs();
    (c[0] * c[0] + c[1] * c[1]).sqrt()
}

/// The transform on the object at a keyed frame.
fn transform_at(scene: &Scene, layer: buzz_scene::LayerId, frame: u32) -> Affine {
    scene
        .layers()
        .get(layer)
        .expect("the layer")
        .objects_at(frame)
        .first()
        .expect("the object at that frame")
        .transform
}

#[test]
fn the_object_starts_and_ends_on_the_path() {
    let (mut scene, layer, id) = scene_with_object(Affine::IDENTITY);
    let path = corner_path();
    let opts = MotionPathOptions {
        easing: Easing::Linear,
        orient_to_path: false,
        ..MotionPathOptions::new(0..25)
    };

    let report = follow_path(&mut scene, id, &path, &opts).expect("the path was written");
    assert!(report.keyframes > 1, "a path of one keyframe is a hold");
    assert!((report.length - 200.0).abs() < 1.0, "arc length was {}", report.length);

    let start = translation(transform_at(&scene, layer, 0));
    assert!(start.distance(Point::new(0.0, 0.0)) < 1.0, "started at {start:?}");

    let end = translation(transform_at(&scene, layer, 24));
    assert!(end.distance(Point::new(100.0, 100.0)) < 1.0, "ended at {end:?}");

    // Half way through a linear traverse of a 200-long path is the corner.
    let mid = translation(transform_at(&scene, layer, 12));
    assert!(mid.distance(Point::new(100.0, 0.0)) < 1.5, "mid was {mid:?}, not the corner");
}

#[test]
fn a_linear_traverse_matches_arc_length_sampling() {
    // Every keyed frame should sit where sampling the path at that frame's
    // fraction says it should — this is the property the whole feature rests on.
    let (mut scene, layer, id) = scene_with_object(Affine::IDENTITY);
    let path = corner_path();
    let opts = MotionPathOptions {
        easing: Easing::Linear,
        orient_to_path: false,
        step: 2,
        ..MotionPathOptions::new(0..25)
    };
    follow_path(&mut scene, id, &path, &opts).expect("written");

    for frame in (0..25).step_by(2) {
        let want = buzz_geom::edit::point_at_fraction(&path, frame as f64 / 24.0, 0.05).unwrap();
        let got = translation(transform_at(&scene, layer, frame));
        assert!(
            got.distance(want) < 1.0,
            "frame {frame}: object at {got:?}, path says {want:?}"
        );
    }
}

#[test]
fn moving_keeps_the_object_its_own_size_and_facing() {
    // Rotated 0.5 rad, scaled 2x. Without orienting, the move must not touch
    // either — only the position.
    let base = Affine::translate((10.0, 10.0)) * Affine::rotate(0.5) * Affine::scale(2.0);
    let (mut scene, layer, id) = scene_with_object(base);
    let path = corner_path();
    let opts = MotionPathOptions {
        easing: Easing::Linear,
        orient_to_path: false,
        ..MotionPathOptions::new(0..25)
    };
    follow_path(&mut scene, id, &path, &opts).expect("written");

    for frame in [0u32, 12, 24] {
        let t = transform_at(&scene, layer, frame);
        assert!((angle(t) - 0.5).abs() < 1e-6, "frame {frame} turned to {}", angle(t));
        assert!((scale_x(t) - 2.0).abs() < 1e-6, "frame {frame} rescaled to {}", scale_x(t));
    }
}

#[test]
fn orienting_turns_the_object_to_face_along_the_path() {
    let (mut scene, layer, id) = scene_with_object(Affine::scale(2.0));
    let path = corner_path();
    let opts = MotionPathOptions {
        easing: Easing::Linear,
        orient_to_path: true,
        ..MotionPathOptions::new(0..25)
    };
    follow_path(&mut scene, id, &path, &opts).expect("written");

    // Frame 4 is on the eastbound leg (fraction 1/6): facing +x, angle ~0.
    let east = angle(transform_at(&scene, layer, 4));
    assert!(east.abs() < 0.05, "on the first leg it faced {east} rad, not east");

    // Frame 20 is on the northbound leg (fraction 5/6): facing +y, angle ~pi/2.
    let north = angle(transform_at(&scene, layer, 20));
    assert!(
        (north - std::f64::consts::FRAC_PI_2).abs() < 0.05,
        "on the second leg it faced {north} rad, not north"
    );

    // Orienting still preserves scale.
    assert!((scale_x(transform_at(&scene, layer, 4)) - 2.0).abs() < 1e-6, "orient rescaled");
}

#[test]
fn an_empty_range_or_path_is_refused_not_written() {
    let (mut scene, _, id) = scene_with_object(Affine::IDENTITY);
    let opts = MotionPathOptions::new(5..5);
    assert!(follow_path(&mut scene, id, &corner_path(), &opts).is_err());

    let opts = MotionPathOptions::new(0..25);
    let mut dot = BezPath::new();
    dot.move_to(Point::new(3.0, 3.0));
    assert!(follow_path(&mut scene, id, &dot, &opts).is_err());
}
