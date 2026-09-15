//! **A rig's chain given follow-through, on a real timeline.**
//!
//! The spring solver is unit-tested in `buzz-rig`; what it cannot check is that
//! the baker reads the *tweened* primary motion off a document, drives the
//! solver with it, and writes the lagging result back as keyframes that leave
//! the rest of the animation alone. That join is the only way this can be wrong
//! while every part is right.

use buzz_act::{Spring, follow_through_bake};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_rig::{Armature, Bone};
use buzz_scene::{
    ArmatureData, LayerId, LayerKind, Object, ObjectId, ObjectKind, Scene, ShapeData, Tween,
};
use peniko::Color;

/// A straight 4-bone chain pointing along +x, as a scene object.
fn rigged_object() -> (Scene, LayerId, ObjectId, Armature) {
    let mut arm = Armature {
        root: Point::ORIGIN,
        bones: Vec::new(),
    };
    for i in 0..4 {
        let parent = if i == 0 { None } else { Some(i - 1) };
        arm.bones.push(Bone::new(format!("b{i}"), parent, 40.0, 0.0));
    }

    let mut scene = Scene::default();
    let layer = scene.add_layer("Rig", LayerKind::Normal);
    let object = Object {
        kind: ObjectKind::Armature(ArmatureData::new(arm.clone())),
        ..Object::shape(
            ObjectId(1),
            ShapeData::filled(Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9), Color::WHITE),
        )
    };
    let id = scene.add_object(layer, object).expect("rig on a layer");
    (scene, layer, id, arm)
}

/// Animate the base bone from 0 to `swing` between frame 0 and frame `at`, with a
/// motion tween, then hold that pose out to `hold_to` (extending the span so the
/// held pose actually resolves across the whole bake range).
fn animate_base(scene: &mut Scene, layer: LayerId, id: ObjectId, at: u32, swing: f64, hold_to: u32) {
    scene.update_layer(layer, |l| {
        if l.frames.length() <= at {
            l.frames.insert_frame(at);
        }
    });
    scene.ensure_keyframe(layer, at);
    scene.update_object_at(at, id, |o| {
        if let ObjectKind::Armature(r) = &mut o.kind {
            r.armature.set_pose(&[swing, 0.0, 0.0, 0.0]);
        }
    });
    scene.update_layer(layer, |l| {
        l.frames.set_tween(0, Tween::motion());
        // Extend the span past the last key so frames after `at` hold `swing`
        // rather than falling off the end of the animation.
        if l.frames.length() <= hold_to {
            l.frames.insert_frame(hold_to);
        }
    });
}

/// The resolved (tweened) pose of the rig at a frame.
fn pose_at(scene: &Scene, layer: LayerId, id: ObjectId, frame: u32) -> Vec<f64> {
    scene
        .layers()
        .get(layer)
        .unwrap()
        .frames
        .resolved_at(frame)
        .iter()
        .find(|o| o.id == id)
        .and_then(|o| match &o.kind {
            ObjectKind::Armature(r) => Some(r.armature.pose()),
            _ => None,
        })
        .expect("a rig at that frame")
}

fn tip_world(topology: &Armature, pose: &[f64]) -> f64 {
    let mut a = topology.clone();
    a.set_pose(pose);
    a.world_angle(3)
}

#[test]
fn baking_follow_through_makes_the_chain_lag_then_settle() {
    let (mut scene, layer, id, arm) = rigged_object();
    animate_base(&mut scene, layer, id, 12, 0.8, 47);

    // Capture the primary motion before baking overwrites it. After frame 12 the
    // base holds at 0.8, so the settled tip should point there too.
    let primary_mid = pose_at(&scene, layer, id, 8);
    let held_tip = tip_world(&arm, &[0.8, 0.0, 0.0, 0.0]);

    let report =
        follow_through_bake(&mut scene, id, 1, Spring::tail(), 0..48, 2, 0.0).expect("baked");
    assert!(report.keyframes > 1, "one keyframe is a hold, not follow-through");
    assert_eq!(report.bones, 3, "the chain from bone 1 is three bones");

    // Mid-move, the tip lags: its baked direction trails the primary's.
    let baked_mid = pose_at(&scene, layer, id, 8);
    assert!(
        (tip_world(&arm, &baked_mid) - tip_world(&arm, &primary_mid)).abs() > 0.1,
        "the tip did not lag the primary motion"
    );

    // Long after, held, it has caught up onto the held pose.
    let baked_end = pose_at(&scene, layer, id, 46);
    assert!(
        (tip_world(&arm, &baked_end) - held_tip).abs() < 0.05,
        "the chain never settled onto the held pose: tip {:.3}, held {:.3}",
        tip_world(&arm, &baked_end),
        held_tip
    );
}

#[test]
fn the_driving_bone_is_left_alone() {
    let (mut scene, layer, id, _arm) = rigged_object();
    animate_base(&mut scene, layer, id, 12, 0.8, 23);
    let primary_mid = pose_at(&scene, layer, id, 8);

    follow_through_bake(&mut scene, id, 1, Spring::tail(), 0..24, 2, 0.0).expect("baked");

    // Bone 0 is above the chain root (1); its keyed motion must survive.
    let baked_mid = pose_at(&scene, layer, id, 8);
    assert!(
        (baked_mid[0] - primary_mid[0]).abs() < 1e-6,
        "the driving bone was disturbed: {} vs {}",
        baked_mid[0],
        primary_mid[0]
    );
}

#[test]
fn keys_land_on_twos() {
    let (mut scene, layer, id, _arm) = rigged_object();
    animate_base(&mut scene, layer, id, 12, 0.8, 23);
    follow_through_bake(&mut scene, id, 1, Spring::tail(), 0..24, 2, 0.0).expect("baked");

    let l = scene.layers().get(layer).unwrap();
    for frame in (0..24).step_by(2) {
        assert!(l.frames.is_keyframe(frame), "no key on frame {frame}");
    }
}

#[test]
fn an_unrigged_object_is_refused() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let id = scene
        .add_object(
            layer,
            Object::shape(
                ObjectId(1),
                ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
            ),
        )
        .unwrap();
    assert!(follow_through_bake(&mut scene, id, 0, Spring::hair(), 0..24, 2, 0.0).is_err());
}
