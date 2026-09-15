//! **A wiggle baked onto an object's placement, on a real timeline.**
//!
//! The noise itself is unit-tested in `buzz-physics`; this checks the baker
//! rides it on top of the object's own transform, writes keyframes that stay
//! within the amplitude, actually move the object, and are reproducible.

use buzz_act::{Wiggle, wiggle_bake};
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_scene::{LayerId, LayerKind, Object, ObjectId, Scene, ShapeData};
use peniko::Color;

fn a_sign_at(place: Affine) -> (Scene, LayerId, ObjectId) {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Sign", LayerKind::Normal);
    let mut object = Object::shape(
        ObjectId(1),
        ShapeData::filled(Rect::new(-10.0, -10.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
    );
    object.transform = place;
    let id = scene.add_object(layer, object).expect("on a layer");
    (scene, layer, id)
}

fn translation_at(scene: &Scene, layer: LayerId, id: ObjectId, frame: u32) -> Point {
    let t = scene
        .layers()
        .get(layer)
        .unwrap()
        .frames
        .resolved_at(frame)
        .iter()
        .find(|o| o.id == id)
        .expect("the object")
        .transform
        .translation();
    Point::new(t.x, t.y)
}

#[test]
fn a_wiggle_moves_the_object_within_its_amplitude() {
    let base = Point::new(200.0, 150.0);
    let (mut scene, layer, id) = a_sign_at(Affine::translate(base.to_vec2()));
    let amp = 12.0;

    let report = wiggle_bake(&mut scene, id, Wiggle::new(amp, 2.0), 0..30, 1).expect("baked");
    assert!(report.keyframes > 1, "a wiggle of one keyframe is a hold");

    let mut moved_somewhere = false;
    for frame in 0..30 {
        let p = translation_at(&scene, layer, id, frame);
        let dx = p.x - base.x;
        let dy = p.y - base.y;
        assert!(
            dx.abs() <= amp + 1e-6 && dy.abs() <= amp + 1e-6,
            "frame {frame}: offset ({dx:.2},{dy:.2}) exceeded amplitude {amp}"
        );
        if dx.hypot(dy) > 1.0 {
            moved_somewhere = true;
        }
    }
    assert!(moved_somewhere, "the wiggle never actually moved the object");
}

#[test]
fn the_wiggle_rides_on_top_of_the_objects_own_placement() {
    // Placed off-origin: the baked frames must jitter *around* that placement,
    // not around the origin.
    let base = Point::new(500.0, 320.0);
    let (mut scene, layer, id) = a_sign_at(Affine::translate(base.to_vec2()));
    wiggle_bake(&mut scene, id, Wiggle::new(8.0, 1.5), 0..20, 1).expect("baked");

    for frame in 0..20 {
        let p = translation_at(&scene, layer, id, frame);
        assert!(
            (p.x - base.x).abs() <= 8.0 + 1e-6 && (p.y - base.y).abs() <= 8.0 + 1e-6,
            "frame {frame} strayed far from the placement: {p:?}"
        );
    }
}

#[test]
fn the_same_object_wiggles_the_same_way_every_time() {
    let place = Affine::translate((100.0, 100.0));
    let bake = || {
        let (mut scene, layer, id) = a_sign_at(place);
        wiggle_bake(&mut scene, id, Wiggle::handheld(), 0..24, 1).unwrap();
        translation_at(&scene, layer, id, 7)
    };
    assert_eq!(bake(), bake(), "the wiggle was not reproducible");
}

#[test]
fn an_empty_range_is_refused() {
    let (mut scene, _, id) = a_sign_at(Affine::IDENTITY);
    assert!(wiggle_bake(&mut scene, id, Wiggle::breath(), 5..5, 1).is_err());
}
