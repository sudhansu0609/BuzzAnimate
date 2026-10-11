//! **An auto-rigged character's joint limits never stop the director.**
//!
//! `RigPattern::biped()` fills every slot with default limits (relative to the
//! rest angle), and `assemble` turns them into the bone's absolute limits. The
//! director's verbs are the ground truth those defaults are written against:
//! a limit that clamps a verb is a wrong limit, so every verb's pose over a
//! whole cycle must land on the rig exactly as asked — on a rig drawn either
//! way round, since the gestures that place a hand are solved on the rig's own
//! geometry and bend its elbows whichever way that geometry needs. And a joint
//! the animator *has* narrowed must hold against a verb that asks past it.

use std::sync::Arc;

use buzz_act::{Action, Joint, Performance, assemble, perform, pose_on};
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_rig::{JointLimits, RigPattern, match_parts, wrap_pi};
use buzz_scene::{LayerKind, Object, ObjectId, ObjectKind, Scene, ShapeData};
use peniko::Color;

/// One limb, drawn as a bar from `head` to `tip` (as in `rig_a_drawn_character`).
fn limb(id: u64, name: &str, head: Point, tip: Point, width: f64) -> Arc<Object> {
    let along = tip - head;
    let length = along.hypot();
    let bar = Rect::new(0.0, -width * 0.5, length, width * 0.5);
    let place = Affine::translate(head.to_vec2()) * Affine::rotate(along.y.atan2(along.x));

    let mut object = Object::shape(
        ObjectId(id),
        ShapeData::filled(
            place * bar.to_path(1e-9),
            Color::from_rgb8(0x99, 0x77, 0x55),
        ),
    );
    object.name = Some(name.to_string());
    Arc::new(object)
}

/// The same tidily named, upright character `rig_a_drawn_character` rigs: a
/// front view, the character's right arm on the picture's left — the Ep1 Cast
/// rigs' handedness. `mirror` draws it the other way round, the right arm on
/// the picture's right, which is the procedural puppet's.
fn drawn_character(mirror: bool) -> Vec<Arc<Object>> {
    let s = if mirror { -1.0 } else { 1.0 };
    let p = |x: f64, y: f64| Point::new(s * x, y);
    let hips = p(0.0, -470.0);
    let chest = p(0.0, -700.0);
    let shoulder = p(0.0, -820.0);
    let crown = p(0.0, -1000.0);

    vec![
        limb(1, "R_thigh", hips, p(-20.0, -235.0), 60.0),
        limb(2, "R.shin", p(-20.0, -235.0), p(-24.0, 0.0), 46.0),
        limb(3, "rightArm", shoulder, p(-60.0, -680.0), 44.0),
        limb(4, "R_forearm", p(-60.0, -680.0), p(-78.0, -540.0), 38.0),
        limb(5, "hips", hips, chest, 130.0),
        limb(6, "torso", chest, shoulder, 150.0),
        limb(7, "L_thigh", hips, p(20.0, -235.0), 60.0),
        limb(8, "L.shin", p(20.0, -235.0), p(24.0, 0.0), 46.0),
        limb(9, "leftArm", shoulder, p(60.0, -680.0), 44.0),
        limb(10, "L_forearm", p(60.0, -680.0), p(78.0, -540.0), 38.0),
        limb(11, "head", shoulder, crown, 120.0),
    ]
}

fn auto_rig(mirror: bool) -> (RigPattern, buzz_scene::ArmatureData) {
    let drawings = drawn_character(mirror);
    let pattern = RigPattern::biped();
    let names: Vec<String> = drawings
        .iter()
        .map(|d| d.name.clone().unwrap_or_default())
        .collect();
    let filled = match_parts(&pattern, &names);
    let mut parts: Vec<(usize, Arc<Object>)> = Vec::new();
    for (index, drawing) in drawings.iter().enumerate() {
        if let Some(slot) = filled.iter().position(|f| *f == Some(index)) {
            parts.push((slot, drawing.clone()));
        }
    }
    let rig = assemble(&pattern, &parts).expect("a rig");
    (pattern, rig)
}

/// Both handednesses: the Cast rigs' (`false`) and the puppet's (`true`).
const HANDS: [bool; 2] = [false, true];

/// Every verb the director can give.
const VERBS: [Action; 15] = [
    Action::Walk,
    Action::Run,
    Action::Talk,
    Action::Idle,
    Action::Sit,
    Action::Stand,
    Action::Turn,
    Action::Point,
    Action::Reach,
    Action::React,
    Action::Wave,
    Action::Nod,
    Action::HeadShake,
    Action::Shrug,
    Action::Namaste,
];

/// The two drawings really are the two handednesses the director tells
/// apart, or the rest of this file proves half of what it says.
#[test]
fn the_two_drawings_are_the_two_handednesses() {
    for mirror in HANDS {
        let (_, rig) = auto_rig(mirror);
        assert_eq!(
            buzz_act::perform::arms_mirrored(&rig.armature.at_rest()),
            !mirror,
            "mirror {mirror}"
        );
    }
}

/// The slot's limits arrive on the bone measured from the bone's own rest
/// angle, so a freshly rigged character is inside every one of them.
#[test]
fn an_auto_rigged_bone_is_limited_about_its_rest_angle() {
    for mirror in HANDS {
        let (pattern, rig) = auto_rig(mirror);
        for (slot, bone) in pattern.slots.iter().zip(&rig.armature.bones) {
            match (slot.limits, bone.limits) {
                (None, None) => {}
                (Some(relative), Some(absolute)) => {
                    let (min, max) = absolute.relative_to(bone.rest_angle);
                    assert!(
                        (min - relative.min).abs() < 1e-9 && (max - relative.max).abs() < 1e-9,
                        "{}: {min}..{max} about rest, the slot says {relative:?}",
                        slot.name
                    );
                    assert!(
                        (absolute.clamp(bone.rest_angle) - bone.rest_angle).abs() < 1e-9,
                        "{} starts outside its own limits",
                        slot.name
                    );
                }
                (want, got) => panic!("{}: slot {want:?}, bone {got:?}", slot.name),
            }
        }
    }
}

/// **No verb is ever clamped, on a rig drawn either way round.** Each verb's
/// pose, sampled across its whole cycle at the amount the director uses (1.0,
/// its largest), is worked out on the rig *with its limits taken off* — so
/// the hand-targeted gestures cannot quietly pre-clamp themselves — then set
/// on the limited rig and read back: every joint must hold exactly what was
/// asked. A failure names the verb, the slot and how far past the limit it
/// asked — widen that slot's default, never narrow the verb.
#[test]
fn the_directors_verbs_are_never_clamped() {
    let samples = 240;
    let mut clamped: Vec<String> = Vec::new();
    for mirror in HANDS {
        let (pattern, rig) = auto_rig(mirror);
        let rest = rig.armature.at_rest().pose();
        let mut free = rig.armature.at_rest();
        for bone in &mut free.bones {
            bone.limits = None;
        }
        for action in VERBS {
            let mut worst: Vec<f64> = vec![0.0; rest.len()];
            for step in 0..=samples {
                let phase = step as f64 / samples as f64;
                let beat = pose_on(action, phase, 1.0, &free);
                let asked: Vec<f64> = rest
                    .iter()
                    .zip(beat.joints.iter().chain(std::iter::repeat(&0.0)))
                    .map(|(rest, delta)| rest + delta)
                    .collect();
                let mut posed = rig.armature.clone();
                posed.set_pose(&asked);
                for (index, (got, want)) in posed.pose().iter().zip(&asked).enumerate() {
                    let off = wrap_pi(got - want).abs();
                    worst[index] = worst[index].max(off);
                }
            }
            for (index, off) in worst.iter().enumerate() {
                if *off > 1e-6 {
                    clamped.push(format!(
                        "{action:?} (mirror {mirror}) clamps {} by {:.2} deg",
                        pattern.slots[index].name,
                        off.to_degrees()
                    ));
                }
            }
        }
    }
    assert!(clamped.is_empty(), "verbs clamped by the defaults: {clamped:#?}");
}

/// Every frame's offset of `joint` from its rest angle, over a talk written
/// with `perform` onto `rig`.
fn offsets_in_a_talk(rig: buzz_scene::ArmatureData, joint: usize) -> Vec<f64> {
    let rest = rig.armature.bones[joint].rest_angle;
    let mut scene = Scene::default();
    let layer = scene.add_layer("Character", LayerKind::Normal);
    let id = scene
        .add_object(
            layer,
            Object {
                kind: ObjectKind::Armature(rig),
                ..Object::shape(
                    ObjectId(500),
                    ShapeData::filled(Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9), Color::WHITE),
                )
            },
        )
        .expect("the rig went on a layer");
    perform(&mut scene, id, &Performance::new(Action::Talk, 0..48)).expect("the talk");

    let mut offsets = Vec::new();
    for frame in 0..48 {
        let at = scene
            .layers()
            .get(layer)
            .expect("the layer")
            .objects_at(frame)
            .first()
            .cloned()
            .expect("the rig");
        let ObjectKind::Armature(data) = &at.kind else {
            panic!("the talk replaced the rig");
        };
        offsets.push(wrap_pi(data.armature.bones[joint].angle - rest));
    }
    offsets
}

/// **A verb asking a joint past an explicitly narrowed limit is clamped**,
/// on a rig drawn either way round. A talk bends each elbow one way; narrowed
/// to bend only the *other* way, the elbow stays inside that range on every
/// frame instead.
#[test]
fn a_verb_cannot_bend_a_narrowed_elbow_past_its_limit() {
    for mirror in HANDS {
        for joint in [Joint::ElbowL, Joint::ElbowR] {
            let elbow = joint.index();

            // With the defaults, the talk bends the elbow — and which way.
            let (_, rig) = auto_rig(mirror);
            let free = offsets_in_a_talk(rig, elbow)
                .into_iter()
                .fold(0.0f64, |a, b| if b.abs() > a.abs() { b } else { a });
            assert!(
                free.abs() > 0.05,
                "mirror {mirror}: the default {joint:?} did not bend in a talk: {free}"
            );

            // Narrowed to the other way only: every frame is held inside it.
            let (_, mut rig) = auto_rig(mirror);
            let rest = rig.armature.bones[elbow].rest_angle;
            let (min, max) = if free < 0.0 {
                (0.0, 150f64.to_radians())
            } else {
                (-150f64.to_radians(), 0.0)
            };
            rig.armature.bones[elbow].limits = Some(JointLimits::from_relative(rest, min, max));
            for (frame, held) in offsets_in_a_talk(rig, elbow).into_iter().enumerate() {
                assert!(
                    held >= min - 1e-9 && held <= max + 1e-9,
                    "mirror {mirror} frame {frame}: a narrowed {joint:?} bent {} deg, outside {}..{}",
                    held.to_degrees(),
                    min.to_degrees(),
                    max.to_degrees()
                );
            }
        }
    }
}
