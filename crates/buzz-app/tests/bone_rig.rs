//! **The rig stays on the character.**
//!
//! # The defect this pins
//!
//! Dragging from a bone's *tip* extends the chain — that is how a skeleton is
//! built — and extending it called `Armature::set_rest_here`, which adopts the
//! pose the bones are in **now** as the pose they were drawn in.
//!
//! For a rig being built that is exactly right: the first bones are laid on
//! artwork that is not going anywhere. For a rig that is already finished and
//! posed it is a catastrophe. Every rigidly bound part is drawn through
//! `pose_transform`, which measures the bone against its rest — so re-resting a
//! posed skeleton makes every one of those transforms the identity, and the
//! artwork snaps back to where it was drawn while the bones stay where the
//! animator put them. The character and its skeleton come apart.
//!
//! And it is easy to hit, because **grabbing the end of a bone is the natural
//! way to move a limb**. The tip is what you reach for; the tip is what built a
//! new bone instead.

use buzz_act::figure::{self, FigureSpec};
use buzz_act::puppet::{self, PuppetSpec};
use buzz_geom::Point;
use buzz_scene::{LayerKind, ObjectKind, Scene};

/// A rigged figure standing at the origin, and the id of the armature.
fn rigged() -> (Scene, buzz_scene::ObjectId) {
    let mut scene = Scene::default();
    let layer = scene.add_stage_layer("Cast", LayerKind::Normal);
    let id = scene.next_object_id();
    let person = figure::build(&FigureSpec::default(), id, || scene.next_object_id());
    let placed = scene.add_object(layer, person).expect("the figure");
    (scene, placed)
}

/// Where the posed artwork actually is.
fn artwork_bounds(scene: &Scene, object: buzz_scene::ObjectId) -> buzz_geom::Rect {
    let (_, found) = scene.find_object(object).expect("the figure");
    match &found.kind {
        ObjectKind::Armature(rig) => rig
            .posed()
            .iter()
            .map(|part| part.bounds())
            .reduce(|a, b| a.union(b))
            .expect("artwork"),
        _ => panic!("not rigged"),
    }
}

/// Where the bones are.
fn bone_bounds(scene: &Scene, object: buzz_scene::ObjectId) -> buzz_geom::Rect {
    let (_, found) = scene.find_object(object).expect("the figure");
    match &found.kind {
        ObjectKind::Armature(rig) => rig.armature.bounds().expect("bones"),
        _ => panic!("not rigged"),
    }
}

/// **Adding a bone to a posed rig leaves the artwork where it is.**
#[test]
fn extending_a_posed_rig_does_not_tear_the_artwork_off_it() {
    let (mut scene, figure) = rigged();

    // Pose it, as an animator would before reaching for another bone.
    scene.update_object_at(0, figure, |object| {
        if let ObjectKind::Armature(rig) = &mut object.kind {
            let mut pose = rig.armature.pose();
            pose[buzz_act::figure::Joint::ShoulderL.index()] += 0.6;
            pose[buzz_act::figure::Joint::ElbowL.index()] += 0.5;
            rig.armature.set_pose(&pose);
        }
    });

    let art_before = artwork_bounds(&scene, figure);
    let bones_before = bone_bounds(&scene, figure);

    // Extend the chain from the left hand.
    let tip = {
        let (_, found) = scene.find_object(figure).expect("the figure");
        match &found.kind {
            ObjectKind::Armature(rig) => {
                rig.armature.tip(buzz_act::figure::Joint::ElbowL.index())
            }
            _ => panic!("not rigged"),
        }
    };
    buzz_app::rigging::add_bone(
        &mut scene,
        0,
        figure,
        Some(buzz_act::figure::Joint::ElbowL.index()),
        tip,
        tip + buzz_geom::Vec2::new(20.0, 10.0),
    );

    let art_after = artwork_bounds(&scene, figure);
    let bones_after = bone_bounds(&scene, figure);

    // The new bone legitimately grows the skeleton's extent a little; the
    // *artwork* has no reason to move at all.
    let moved = (art_after.center() - art_before.center()).hypot();
    assert!(
        moved < 1.0,
        "the artwork jumped {moved:.1} units when a bone was added \u{2014} \
         the rig has come off the character.\n  before {art_before:?}\n  after  {art_after:?}"
    );

    // And the two are still on top of each other, which is the thing a person
    // actually sees.
    let drift = (bones_after.center() - bones_before.center()).hypot();
    assert!(
        drift < 30.0,
        "the skeleton moved {drift:.1} units away from artwork that did not"
    );
}

/// **The rest pose survives.** The direct statement of the same thing: a rig
/// that has been posed and then extended still knows what pose it was drawn
/// in, so it can be put back.
#[test]
fn extending_a_rig_leaves_the_pose_it_was_drawn_in_alone() {
    let (mut scene, figure) = rigged();
    let drawn = {
        let (_, found) = scene.find_object(figure).expect("the figure");
        match &found.kind {
            ObjectKind::Armature(rig) => rig.armature.at_rest().pose(),
            _ => panic!("not rigged"),
        }
    };

    scene.update_object_at(0, figure, |object| {
        if let ObjectKind::Armature(rig) = &mut object.kind {
            let mut pose = rig.armature.pose();
            for angle in &mut pose {
                *angle += 0.4;
            }
            rig.armature.set_pose(&pose);
        }
    });

    let head = Point::new(0.0, -100.0);
    buzz_app::rigging::add_bone(
        &mut scene,
        0,
        figure,
        None,
        head,
        head + buzz_geom::Vec2::new(0.0, -30.0),
    );

    let (_, found) = scene.find_object(figure).expect("the figure");
    let after = match &found.kind {
        ObjectKind::Armature(rig) => rig.armature.at_rest().pose(),
        _ => panic!("not rigged"),
    };
    for (i, (before, now)) in drawn.iter().zip(after.iter()).enumerate() {
        assert!(
            (before - now).abs() < 1e-9,
            "bone {i}'s drawn angle moved from {before} to {now}"
        );
    }
}

/// **Posing a bone moves the artwork with it**, which is the whole point of a
/// rig and is the behaviour the tip-grab was stealing.
#[test]
fn posing_a_bone_carries_its_artwork() {
    let (mut scene, figure) = rigged();
    let before = artwork_bounds(&scene, figure);

    // Drag the left hand a long way out to the side.
    let target = Point::new(220.0, -180.0);
    buzz_app::rigging::pose_bone(
        &mut scene,
        0,
        figure,
        buzz_act::figure::Joint::ElbowL.index(),
        target,
    );

    let after = artwork_bounds(&scene, figure);
    assert!(
        after.x1 > before.x1 + 20.0,
        "the arm did not reach: {before:?} -> {after:?}"
    );
    // The feet stay planted: an IK drag moves the chain above the bone, never
    // the whole character.
    assert!(
        (after.y1 - before.y1).abs() < 1.0,
        "the figure left the ground: {before:?} -> {after:?}"
    );
}

/// **Hiding the bones takes them out of the way of the pointer**, not just out
/// of the picture. A rig you cannot see and can still grab by accident is worse
/// than one you can see.
#[test]
fn hidden_bones_cannot_be_grabbed() {
    let mut scene = Scene::default();
    let layer = scene.add_stage_layer("Cast", LayerKind::Normal);
    let id = scene.next_object_id();
    let puppet = puppet::build(
        &mut scene,
        &PuppetSpec {
            at: Point::new(400.0, 700.0),
            ..PuppetSpec::default()
        },
    );
    let _ = (layer, id);

    // Somewhere along the spine, which is a bone.
    let on_a_bone = Point::new(400.0, 700.0 - 320.0 * 0.6);

    let shown = buzz_app::rigging::target_at(&scene, 0, on_a_bone, 8.0);
    assert!(
        matches!(
            shown,
            buzz_app::rigging::RigTarget::Bone(..) | buzz_app::rigging::RigTarget::BoneTip(..)
        ),
        "expected to find a bone with the rig shown, found {shown:?}"
    );

    let hidden = buzz_app::rigging::target_at_visible(&scene, 0, on_a_bone, 8.0, false);
    assert!(
        !matches!(
            hidden,
            buzz_app::rigging::RigTarget::Bone(..) | buzz_app::rigging::RigTarget::BoneTip(..)
        ),
        "a hidden bone was still grabbed: {hidden:?}"
    );
    let _ = puppet;
}
