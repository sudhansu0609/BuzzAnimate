//! **Sorting a character's drawings into a skeleton, through the document.**
//!
//! The Rigging panel offers three ways to fill a slot — read the layer names,
//! click a slot and then the stage, or drag a drawing onto it — and all three
//! end in the same two document edits: [`rigging::rig_character`] and
//! [`rigging::replace_part`]. These drive those, so what is proved is that the
//! *character* comes out rigged rather than that some buttons exist.
//!
//! The panel's own arithmetic — which name means which limb, which drawing is
//! in which slot — is tested where it lives, in `buzz-rig` and `buzz-ui`.

use buzz_app::{editor::Editor, rigging};
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_rig::{RigPattern, match_parts};
use buzz_scene::{LayerKind, ObjectId, ObjectKind, ShapeData};
use peniko::Color;

const LIMB: Color = Color::from_rgb8(0xC8, 0x8A, 0x5A);

/// A document with one drawing per limb, named the way a tidy export names
/// them, on one layer and in paint order.
///
/// Two layers rather than one would be more realistic; one is used because
/// what these tests are about is the *slots*, and the paint order within a
/// layer is the same fact as the order between them.
fn drawn_character() -> (Editor, Vec<ObjectId>) {
    let mut editor = Editor::default();
    editor.doc.edit("Layer", |scene| {
        scene.add_layer("Character", LayerKind::Normal);
    });
    editor
        .selection
        .ensure_active_layer(&editor.doc.scene().clone());
    let layer = editor.selection.active_layer().expect("a layer");

    // name, and the bar it is drawn as: x is across, y is up the screen and
    // therefore negative. Back to front, as they come off a stack.
    let parts: [(&str, Rect); 11] = [
        ("R_thigh", Rect::new(-50.0, -470.0, 10.0, -235.0)),
        ("R.shin", Rect::new(-47.0, -235.0, -1.0, 0.0)),
        ("rightArm", Rect::new(-82.0, -820.0, -38.0, -680.0)),
        ("R_forearm", Rect::new(-97.0, -680.0, -59.0, -540.0)),
        ("hips", Rect::new(-65.0, -700.0, 65.0, -470.0)),
        ("torso", Rect::new(-75.0, -820.0, 75.0, -700.0)),
        ("L_thigh", Rect::new(-10.0, -470.0, 50.0, -235.0)),
        ("L.shin", Rect::new(1.0, -235.0, 47.0, 0.0)),
        ("leftArm", Rect::new(38.0, -820.0, 82.0, -680.0)),
        ("L_forearm", Rect::new(59.0, -680.0, 97.0, -540.0)),
        ("head", Rect::new(-60.0, -1000.0, 60.0, -820.0)),
    ];

    let mut ids = Vec::new();
    for (name, bar) in parts {
        let mut id = None;
        editor.doc.edit("Draw", |scene| {
            id = scene.add_shape(layer, ShapeData::filled(bar.to_path(1e-9), LIMB));
        });
        let id = id.expect("a limb");
        editor.doc.edit("Name", |scene| {
            scene.update_object(id, |object| object.name = Some(name.to_string()));
        });
        ids.push(id);
    }
    (editor, ids)
}

/// What the panel's Auto button works out, as the slot list it hands over.
fn auto_assigned(editor: &Editor, pattern: &RigPattern) -> Vec<Option<ObjectId>> {
    let parts = rigging::loose_parts(editor.doc.scene(), 0);
    let names: Vec<String> = parts.iter().map(|p| p.name.clone()).collect();
    match_parts(pattern, &names)
        .into_iter()
        .map(|found| found.map(|index| parts[index].object))
        .collect()
}

/// Rig the character, returning the armature.
fn rig(editor: &mut Editor, pattern: &RigPattern) -> ObjectId {
    let slots = auto_assigned(editor, pattern);
    let mut built = None;
    editor.doc.edit("Rig Character", |scene| {
        built = rigging::rig_character(scene, 0, pattern, &slots);
    });
    built.expect("a rig")
}

/// The panel offers exactly the drawings, and offers them back to front.
#[test]
fn the_loose_parts_are_the_drawings_in_paint_order() {
    let (editor, ids) = drawn_character();
    let parts = rigging::loose_parts(editor.doc.scene(), 0);

    assert_eq!(
        parts.iter().map(|p| p.object).collect::<Vec<_>>(),
        ids,
        "the parts came back in a different order from the layer stack"
    );
    assert_eq!(parts[0].name, "R_thigh");
}

/// A drawing on a hidden or locked layer is not something a click can reach, so
/// it is not something the panel should offer either.
#[test]
fn a_locked_layer_offers_nothing_to_rig() {
    let (mut editor, _) = drawn_character();
    let layer = editor.selection.active_layer().expect("a layer");
    editor.doc.edit("Lock", |scene| {
        scene.update_layer(layer, |l| l.locked = true);
    });

    assert!(rigging::loose_parts(editor.doc.scene(), 0).is_empty());
}

/// **The whole gesture.** Eleven drawings go in, one rigged character comes
/// out, and nothing is left behind on the layer.
#[test]
fn rigging_a_character_replaces_its_drawings_with_one_armature() {
    let (mut editor, _) = drawn_character();
    let pattern = RigPattern::biped();
    let before = editor.doc.scene().shape_count();

    let id = rig(&mut editor, &pattern);
    let scene = editor.doc.scene();
    let (_, object) = scene.find_object(id).expect("the rig");

    let ObjectKind::Armature(data) = &object.kind else {
        panic!("what was built is not an armature");
    };
    assert_eq!(data.armature.len(), pattern.slots.len());
    assert_eq!(data.parts.len(), 11, "a drawing was dropped");
    assert_eq!(data.pattern.as_deref(), Some("Biped"));

    // The drawings moved into the rig rather than being copied, so the layer
    // holds the armature and nothing else.
    let layer = editor.selection.active_layer().expect("a layer");
    let left = scene.layers().get(layer).expect("the layer").objects_at(0);
    assert_eq!(left.len(), 1, "loose drawings were left on the layer");
    assert_eq!(
        scene.shape_count(),
        before,
        "rigging duplicated the artwork instead of moving it"
    );
}

/// One Ctrl+Z, not eleven. Rigging is a single decision.
#[test]
fn rigging_a_character_is_one_undo_step() {
    let (mut editor, ids) = drawn_character();
    rig(&mut editor, &RigPattern::biped());

    editor.doc.undo();

    let scene = editor.doc.scene();
    for id in ids {
        assert!(
            scene.find_object(id).is_some(),
            "undo did not bring drawing {id:?} back"
        );
    }
    let layer = editor.selection.active_layer().expect("a layer");
    assert_eq!(scene.layers().get(layer).expect("layer").objects_at(0).len(), 11);
}

/// Clicking a slot and then the stage picks what is under the pointer, topmost
/// first — the same rule a selection follows.
#[test]
fn clicking_the_stage_finds_the_drawing_on_top() {
    let (editor, ids) = drawn_character();
    let scene = editor.doc.scene();

    // The head, which nothing overlaps.
    let head = rigging::part_at(scene, 0, Point::new(0.0, -900.0)).expect("the head");
    assert_eq!(head.name, "head");

    // Where the torso and the left arm overlap, the arm is in front: it is
    // later in the stack than the torso.
    let over = rigging::part_at(scene, 0, Point::new(60.0, -750.0)).expect("something");
    assert_eq!(over.name, "leftArm");

    // And past the character there is nothing, which is what cancels the mode.
    assert!(rigging::part_at(scene, 0, Point::new(900.0, -100.0)).is_none());
    assert_eq!(ids.len(), 11);
}

/// A slot nothing was put in still gets a bone, so a performance addressing it
/// by index moves the limb it means rather than the one next to it.
#[test]
fn a_half_sorted_character_still_gets_a_whole_skeleton() {
    let (mut editor, ids) = drawn_character();
    let pattern = RigPattern::biped();

    // Only the hips and the head, put in by hand.
    let mut slots = vec![None; pattern.slots.len()];
    slots[pattern.slot_named("Hips").unwrap()] = Some(ids[4]);
    slots[pattern.slot_named("Head").unwrap()] = Some(ids[10]);

    let mut built = None;
    editor.doc.edit("Rig Character", |scene| {
        built = rigging::rig_character(scene, 0, &pattern, &slots);
    });
    let id = built.expect("a rig");

    let (_, object) = editor.doc.scene().find_object(id).expect("the rig");
    let ObjectKind::Armature(data) = &object.kind else {
        panic!("not an armature");
    };
    assert_eq!(data.armature.len(), pattern.slots.len());
    assert_eq!(data.parts.len(), 2);
    assert!(
        data.armature.bones.iter().all(|b| b.length > 1.0),
        "an empty slot came out with no bone worth the name"
    );

    // The nine drawings that were not sorted are still on the layer, waiting.
    let layer = editor.selection.active_layer().expect("a layer");
    let left = editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .expect("layer")
        .objects_at(0);
    assert_eq!(left.len(), 10, "the unsorted drawings went somewhere");
}

/// **Redrawing a limb.** Drop a new arm on "Elbow L" and it takes over that
/// bone, keeping every pose and every keyframe that was written against it.
#[test]
fn a_redrawn_part_replaces_the_one_on_its_bone() {
    let (mut editor, _) = drawn_character();
    let pattern = RigPattern::biped();
    let rig_id = rig(&mut editor, &pattern);
    let slot = pattern.slot_named("Elbow L").expect("a left elbow");

    // A saved pose, to prove the skeleton survives the swap.
    editor.doc.edit("Pose", |scene| {
        scene.update_object(rig_id, |object| {
            if let ObjectKind::Armature(data) = &mut object.kind {
                let angles = data.armature.pose();
                data.poses.push(buzz_scene::NamedPose {
                    name: "Reach".into(),
                    angles,
                });
            }
        });
    });
    let bone_before = {
        let (_, o) = editor.doc.scene().find_object(rig_id).expect("the rig");
        let ObjectKind::Armature(data) = &o.kind else {
            panic!()
        };
        data.armature.bones[slot].clone()
    };

    // A new forearm, drawn on the stage where the old one is.
    let layer = editor.selection.active_layer().expect("a layer");
    let mut redrawn = None;
    editor.doc.edit("Draw", |scene| {
        redrawn = scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(59.0, -680.0, 97.0, -540.0).to_path(1e-9),
                Color::from_rgb8(0x22, 0x88, 0x44),
            ),
        );
    });
    let redrawn = redrawn.expect("a new forearm");

    let mut done = false;
    editor.doc.edit("Replace Part", |scene| {
        done = rigging::replace_part(scene, rig_id, slot, redrawn);
    });
    assert!(done, "the drop did nothing");

    let scene = editor.doc.scene();
    let (_, object) = scene.find_object(rig_id).expect("the rig");
    let ObjectKind::Armature(data) = &object.kind else {
        panic!("not an armature");
    };

    assert_eq!(data.parts.len(), 11, "the swap changed the number of parts");
    let on_the_bone = data
        .parts
        .iter()
        .find(|p| matches!(p.binding, buzz_scene::RigBinding::Rigid(b) if b == slot))
        .expect("something on the left elbow");
    assert_eq!(on_the_bone.artwork.id, redrawn, "the old drawing is still on");

    // The skeleton is untouched: same bone, same pose library.
    assert_eq!(data.armature.bones[slot], bone_before);
    assert_eq!(data.poses.len(), 1);
    assert_eq!(data.poses[0].name, "Reach");

    // And the new drawing is inside the rig, not still loose on the layer.
    assert!(rigging::loose_parts(scene, 0).is_empty());
}

/// A rig that has been moved or scaled must not fling a dropped part across the
/// stage: parts live in the coordinates of the armature, not of the layer.
#[test]
fn a_part_dropped_into_a_moved_rig_lands_where_it_was_drawn() {
    let (mut editor, _) = drawn_character();
    let pattern = RigPattern::biped();
    let rig_id = rig(&mut editor, &pattern);
    let slot = pattern.slot_named("Head").expect("a head");

    editor.doc.edit("Move", |scene| {
        scene.update_object(rig_id, |object| {
            object.transform = Affine::translate((300.0, -40.0)) * Affine::scale(1.5);
        });
    });

    let layer = editor.selection.active_layer().expect("a layer");
    let drawn = Rect::new(-60.0, -1000.0, 60.0, -820.0);
    let mut redrawn = None;
    editor.doc.edit("Draw", |scene| {
        redrawn = scene.add_shape(
            layer,
            ShapeData::filled(drawn.to_path(1e-9), Color::from_rgb8(0x22, 0x88, 0x44)),
        );
    });
    let redrawn = redrawn.expect("a new head");

    editor.doc.edit("Replace Part", |scene| {
        rigging::replace_part(scene, rig_id, slot, redrawn);
    });

    // Where the new head ends up on the stage, once the rig's own transform is
    // applied to it again: exactly where it was drawn.
    let (_, object) = editor.doc.scene().find_object(rig_id).expect("the rig");
    let ObjectKind::Armature(data) = &object.kind else {
        panic!("not an armature");
    };
    let posed = data
        .posed()
        .into_iter()
        .find(|o| o.id == redrawn)
        .expect("the new head");
    let bounds = posed.bounds();
    let landed = buzz_geom::Rect::from_points(
        object.transform * Point::new(bounds.x0, bounds.y0),
        object.transform * Point::new(bounds.x1, bounds.y1),
    );

    assert!(
        (landed.x0 - drawn.x0).abs() < 1e-6 && (landed.y0 - drawn.y0).abs() < 1e-6,
        "the dropped part moved: drawn at {drawn:?}, landed at {landed:?}"
    );
}

/// Nothing sorted is not a rig. Building one anyway would leave an empty
/// armature on the layer for the animator to find and delete.
#[test]
fn nothing_sorted_builds_nothing() {
    let (mut editor, _) = drawn_character();
    let pattern = RigPattern::biped();
    let empty = vec![None; pattern.slots.len()];

    let mut built = None;
    editor.doc.edit("Rig Character", |scene| {
        built = rigging::rig_character(scene, 0, &pattern, &empty);
    });
    assert!(built.is_none());

    let layer = editor.selection.active_layer().expect("a layer");
    assert_eq!(
        editor
            .doc
            .scene()
            .layers()
            .get(layer)
            .expect("layer")
            .objects_at(0)
            .len(),
        11,
        "a refused rig disturbed the artwork"
    );
}
