//! **A character somebody drew, rigged by pressing one button, then walking.**
//!
//! This is the path the Rigging panel exists for, end to end and without a
//! screen: eleven drawings come off an import with the names an artist gave
//! them, [`match_parts`] works out which limb each one is, [`assemble`] turns
//! them into a skeleton fitted to the artwork, and a walk cycle written against
//! `Joint::ThighL` moves the right leg.
//!
//! Each of those steps has unit tests of its own. What none of them can check
//! is that the four fit together — that the slot the matcher chose is the bone
//! the assembler built is the joint the performance addresses — and that is the
//! only way this feature can be wrong while every part of it is right.

use std::sync::Arc;

use buzz_act::{Action, Joint, Performance, assemble, figure, perform};
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_rig::{RigPattern, match_parts};
use buzz_scene::{LayerKind, Object, ObjectId, ObjectKind, Scene, ShapeData};
use peniko::Color;

/// One limb, drawn as a bar from `head` to `tip`.
///
/// Bars rather than capsules because what is being checked is where the *bone*
/// ends up, and a bar has an unambiguous long axis to put it on.
fn limb(id: u64, name: &str, head: Point, tip: Point, width: f64) -> Arc<Object> {
    let along = tip - head;
    let length = along.hypot();
    let bar = Rect::new(0.0, -width * 0.5, length, width * 0.5);
    let place = Affine::translate(head.to_vec2()) * Affine::rotate(along.y.atan2(along.x));

    let mut object = Object::shape(
        ObjectId(id),
        ShapeData::filled(place * bar.to_path(1e-9), Color::from_rgb8(0x99, 0x77, 0x55)),
    );
    object.name = Some(name.to_string());
    Arc::new(object)
}

/// A character as a tidy export leaves it: one drawing per limb, each with the
/// name the artist typed, standing upright with `(0, 0)` between the feet.
///
/// The names are deliberately four different spellings of the same convention —
/// `L_`, `left`, camel case and a dot — because that is what a real file looks
/// like once two people have worked on it.
fn drawn_character() -> Vec<Arc<Object>> {
    // Landmarks for a figure a thousand units tall, measured up from the ground.
    let hips = Point::new(0.0, -470.0);
    let chest = Point::new(0.0, -700.0);
    let shoulder = Point::new(0.0, -820.0);
    let crown = Point::new(0.0, -1000.0);

    vec![
        // Painted back to front, which is the order they come off the stack.
        limb(1, "R_thigh", hips, Point::new(-20.0, -235.0), 60.0),
        limb(2, "R.shin", Point::new(-20.0, -235.0), Point::new(-24.0, 0.0), 46.0),
        limb(3, "rightArm", shoulder, Point::new(-60.0, -680.0), 44.0),
        limb(4, "R_forearm", Point::new(-60.0, -680.0), Point::new(-78.0, -540.0), 38.0),
        limb(5, "hips", hips, chest, 130.0),
        limb(6, "torso", chest, shoulder, 150.0),
        limb(7, "L_thigh", hips, Point::new(20.0, -235.0), 60.0),
        limb(8, "L.shin", Point::new(20.0, -235.0), Point::new(24.0, 0.0), 46.0),
        limb(9, "leftArm", shoulder, Point::new(60.0, -680.0), 44.0),
        limb(10, "L_forearm", Point::new(60.0, -680.0), Point::new(78.0, -540.0), 38.0),
        limb(11, "head", shoulder, crown, 120.0),
    ]
}

/// Sort the drawings by name and build the rig, exactly as the panel's
/// Auto button and Rig Character button do between them.
fn auto_rig() -> (RigPattern, buzz_scene::ArmatureData) {
    let drawings = drawn_character();
    let pattern = RigPattern::biped();

    let names: Vec<String> = drawings
        .iter()
        .map(|d| d.name.clone().unwrap_or_default())
        .collect();
    let filled = match_parts(&pattern, &names);

    // Back to front, each naming the slot it landed in — the order the panel
    // hands over, and the one that keeps the paint order.
    let mut parts: Vec<(usize, Arc<Object>)> = Vec::new();
    for (index, drawing) in drawings.iter().enumerate() {
        if let Some(slot) = filled.iter().position(|f| *f == Some(index)) {
            parts.push((slot, drawing.clone()));
        }
    }

    let rig = assemble(&pattern, &parts).expect("a rig");
    (pattern, rig)
}

/// **The whole point.** Every limb reaches the slot its name describes, across
/// all four spellings, with no help from the animator.
#[test]
fn a_tidily_named_character_rigs_itself() {
    let (pattern, rig) = auto_rig();

    let full: Vec<bool> = (0..pattern.slots.len())
        .map(|slot| {
            rig.parts
                .iter()
                .any(|p| matches!(p.binding, buzz_scene::RigBinding::Rigid(b) if b == slot))
        })
        .collect();

    assert!(
        pattern.missing_required(&full).is_empty(),
        "the button left slots empty: {:?}",
        pattern.missing_required(&full)
    );
    assert_eq!(rig.parts.len(), 11, "a drawing was dropped or used twice");
}

/// A drawing goes in one slot. Two bones wearing the same picture would draw it
/// twice and pull it in two directions at once.
#[test]
fn no_drawing_ends_up_on_two_bones() {
    let (_, rig) = auto_rig();
    let mut ids: Vec<u64> = rig.parts.iter().map(|p| p.artwork.id.0).collect();
    ids.sort_unstable();
    let unique = {
        let mut seen = ids.clone();
        seen.dedup();
        seen.len()
    };
    assert_eq!(ids.len(), unique, "a drawing was bound twice: {ids:?}");
}

/// The bones follow the drawings rather than a default the artist never saw.
///
/// The left thigh in this character runs 236 units down from the hips; the
/// eight-heads default for a figure of this height is 235, so the check that
/// means something is the **angle**: the drawn thigh leans outwards by about
/// five degrees, and a bone laid on the pattern's own default would lean the
/// other way on the right leg and not at all on this one.
#[test]
fn the_bones_lie_along_the_drawings() {
    let (pattern, rig) = auto_rig();

    for (slot_name, head, tip) in [
        ("Thigh L", Point::new(0.0, -470.0), Point::new(20.0, -235.0)),
        ("Elbow L", Point::new(60.0, -680.0), Point::new(78.0, -540.0)),
        ("Head", Point::new(0.0, -820.0), Point::new(0.0, -1000.0)),
    ] {
        let index = pattern.slot_named(slot_name).expect(slot_name);
        let drawn = (tip - head).hypot();
        let bone = rig.armature.bones[index].length;
        assert!(
            (bone - drawn).abs() < drawn * 0.05,
            "{slot_name} came out {bone} long against a drawing of {drawn}"
        );

        let want = (tip - head).atan2();
        let got = rig.armature.world_angle(index);
        assert!(
            buzz_rig::wrap_pi(got - want).abs() < 0.1,
            "{slot_name} points {got} rad, the drawing points {want} rad"
        );
    }
}

/// Rigging is not moving. A character that jumped when it was rigged would
/// have to be put back by hand every time.
#[test]
fn the_character_does_not_move_when_it_is_rigged() {
    let drawings = drawn_character();
    let before = drawings
        .iter()
        .map(|d| d.bounds())
        .reduce(|a, b| a.union(b))
        .expect("artwork");

    let (_, rig) = auto_rig();
    let after = rig
        .posed()
        .iter()
        .map(|o| o.bounds())
        .reduce(|a, b| a.union(b))
        .expect("posed artwork");

    assert!(
        (after.x0 - before.x0).abs() < 1e-6
            && (after.y0 - before.y0).abs() < 1e-6
            && (after.x1 - before.x1).abs() < 1e-6
            && (after.y1 - before.y1).abs() < 1e-6,
        "the character moved: {before:?} became {after:?}"
    );
}

/// The join between this crate's two halves: a rig assembled from the pattern
/// is one a performance can drive, because the pattern's slots *are*
/// `figure::Joint`.
#[test]
fn an_auto_rigged_character_walks() {
    let (pattern, rig) = auto_rig();

    let mut scene = Scene::default();
    let layer = scene.add_layer("Character", LayerKind::Normal);
    let id = scene
        .add_object(
            layer,
            Object {
                kind: ObjectKind::Armature(rig),
                ..Object::shape(ObjectId(500), ShapeData::filled(
                    Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9),
                    Color::WHITE,
                ))
            },
        )
        .expect("the rig went on a layer");

    let (_, object) = scene.find_object(id).expect("there");
    assert!(
        figure::is_figure(object),
        "an assembled biped is not a figure a performance will touch"
    );
    let rest = figure::rest_pose(object).expect("a rest pose");

    let report = perform(&mut scene, id, &Performance::new(Action::Walk, 0..24))
        .expect("the walk was written");
    assert!(report.keyframes > 1, "a walk of one keyframe is a pose");

    // A quarter of the way through a stride the legs are at their furthest
    // apart, so this is where a walk that did nothing shows up.
    let thigh = Joint::ThighL.index();
    let knee = Joint::KneeR.index();
    let mid = scene
        .layers()
        .get(layer)
        .expect("the layer")
        .objects_at(6)
        .first()
        .cloned()
        .expect("the rig at frame 6");
    let ObjectKind::Armature(walking) = &mid.kind else {
        panic!("the walk replaced the rig with something else");
    };
    let pose = walking.armature.pose();

    assert!(
        (pose[thigh] - rest[thigh]).abs() > 0.05,
        "the left thigh did not swing: {} against a rest of {}",
        pose[thigh],
        rest[thigh]
    );
    // The knee is a different question: it is *straight* at contact, which is
    // where frame 6 happens to land, so it is checked across the stride rather
    // than at one instant. A knee that never bends anywhere in a walk is a
    // character on stilts.
    let mut widest: f64 = 0.0;
    for frame in 0..24 {
        let at = scene
            .layers()
            .get(layer)
            .expect("the layer")
            .objects_at(frame)
            .first()
            .cloned()
            .expect("the rig");
        if let ObjectKind::Armature(data) = &at.kind {
            let angles = data.armature.pose();
            widest = widest.max((angles[knee] - rest[knee]).abs());
        }
    }
    assert!(
        widest > 0.05,
        "the right knee never bent anywhere in the stride: {widest} rad at its most"
    );

    // And the slot list still describes the rig it came from, so the panel can
    // take a redrawn arm back into "Elbow L" a week later.
    assert_eq!(walking.pattern.as_deref(), Some(pattern.name.as_str()));
}

/// A character nobody named rigs into nothing rather than into a guess.
///
/// Twelve layers called "Layer 1" is the other file everyone has, and quietly
/// putting them somewhere plausible would be worse than an empty panel: the
/// animator would have to find the wrong ones before fixing them.
#[test]
fn an_unnamed_character_fills_no_slots() {
    let pattern = RigPattern::biped();
    let names: Vec<String> = (1..=12).map(|n| format!("Layer {n}")).collect();
    let filled = match_parts(&pattern, &names);

    assert!(
        filled.iter().all(Option::is_none),
        "a nameless layer was guessed into a slot: {filled:?}"
    );
}
