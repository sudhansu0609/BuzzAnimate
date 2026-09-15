//! **A pose you can put a character back into, in any scene.**
//!
//! A pose used to be a fact about one keyframe: to reuse it you posed the rig
//! again by hand, every time, and that is the thing the studio reported as
//! slow. These tests drive the whole path — save, apply, mirror, key, and
//! travel with the character through the clipboard — through the document
//! rather than through the panel, so what is proved is that the *work* is
//! reusable and not merely that some buttons exist.

use buzz_app::{editor::Editor, rigging};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_scene::{LayerKind, NamedPose, ObjectId, ObjectKind, Scene, ShapeData};
use peniko::Color;

const LIMB: Color = Color::from_rgb8(0xC8, 0x8A, 0x5A);

/// A document with one limb, rigged with a two-bone arm — the same calls the
/// Bone tool makes when the user drags.
fn rigged_editor() -> (Editor, ObjectId) {
    let mut editor = Editor::default();
    editor.doc.edit("Layer", |scene| {
        scene.add_layer("Character", LayerKind::Normal);
    });
    editor
        .selection
        .ensure_active_layer(&editor.doc.scene().clone());
    let layer = editor.selection.active_layer().expect("a layer");

    let mut id = None;
    editor.doc.edit("Draw", |scene| {
        id = scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(100.0, 190.0, 300.0, 210.0).to_path(1e-9), LIMB),
        );
    });
    let id = id.expect("the limb");

    editor.doc.edit("Create Armature", |scene| {
        rigging::rig_object(scene, 0, id, Point::new(100.0, 200.0), Point::new(200.0, 200.0));
    });
    editor.doc.edit("Add Bone", |scene| {
        rigging::add_bone(
            scene,
            0,
            id,
            Some(0),
            Point::new(200.0, 200.0),
            Point::new(300.0, 200.0),
        );
    });

    (editor, id)
}

fn rig_of(scene: &Scene, frame: u32, id: ObjectId) -> buzz_scene::ArmatureData {
    let object = scene
        .layers()
        .iter()
        .flat_map(|l| l.frames.resolved_at(frame).iter().cloned().collect::<Vec<_>>())
        .find(|o| o.id == id)
        .expect("the rigged object");
    match &object.kind {
        ObjectKind::Armature(rig) => rig.clone(),
        other => panic!("expected an armature, found {other:?}"),
    }
}

/// Pose the rig by dragging its tip, the way an animator does.
fn pose_to(editor: &mut Editor, id: ObjectId, target: Point) {
    editor.doc.edit("Pose", |scene| {
        rigging::pose_bone(scene, 0, id, 1, target);
    });
}

fn save_pose(editor: &mut Editor, id: ObjectId, name: &str) {
    editor.doc.edit("Save Pose", |scene| {
        scene.update_object(id, |target| {
            if let ObjectKind::Armature(rig) = &mut target.kind {
                let angles = rig.armature.pose();
                rig.poses.push(NamedPose {
                    name: name.to_string(),
                    angles,
                });
            }
        });
    });
}

fn apply_pose(editor: &mut Editor, id: ObjectId, index: usize) {
    editor.doc.edit("Apply Pose", |scene| {
        scene.update_object(id, |target| {
            if let ObjectKind::Armature(rig) = &mut target.kind
                && let Some(pose) = rig.poses.get(index)
            {
                let angles = pose.angles.clone();
                rig.armature.set_pose(&angles);
            }
        });
    });
}

/// **Save a pose, move the rig, put it back.** The whole feature in one test.
#[test]
fn a_saved_pose_puts_the_character_back_where_it_was() {
    let (mut editor, id) = rigged_editor();

    pose_to(&mut editor, id, Point::new(200.0, 120.0));
    let reached = rig_of(editor.doc.scene(), 0, id).armature.pose();
    save_pose(&mut editor, id, "Reach");

    // Somewhere else entirely.
    pose_to(&mut editor, id, Point::new(140.0, 300.0));
    let moved = rig_of(editor.doc.scene(), 0, id).armature.pose();
    assert!(
        moved.iter().zip(&reached).any(|(a, b)| (a - b).abs() > 0.05),
        "the second pose should differ from the first"
    );

    apply_pose(&mut editor, id, 0);
    let back = rig_of(editor.doc.scene(), 0, id).armature.pose();
    for (a, b) in back.iter().zip(&reached) {
        assert!((a - b).abs() < 1e-9, "{back:?} is not {reached:?}");
    }
}

/// A pose is only meaningful against its own skeleton, so it lives on the rig
/// — which means it travels wherever the object does. This is the case that
/// matters: **into another document.**
#[test]
fn poses_travel_with_the_character_through_the_clipboard() {
    let (mut editor, id) = rigged_editor();
    pose_to(&mut editor, id, Point::new(200.0, 120.0));
    save_pose(&mut editor, id, "Reach");
    save_pose(&mut editor, id, "Wave");

    editor.selection.set([id]);
    editor.run(buzz_ui::Command::Copy);

    // A different document, as `App::adopt_document` hands the clipboard on.
    // Built and then given the clipboard, rather than by struct update: the
    // editor has private fields (its caches), so `..Editor::default()` cannot
    // name them from outside the crate.
    let mut other = Editor::default();
    other.clipboard = editor.clipboard.clone();
    other.run(buzz_ui::Command::Paste);

    let pasted = other.selection.ids();
    assert_eq!(pasted.len(), 1, "the rig should have arrived");
    let rig = rig_of(other.doc.scene(), 0, pasted[0]);
    assert_eq!(rig.poses.len(), 2, "its poses did not come with it");
    assert_eq!(rig.poses[0].name, "Reach");
    assert_eq!(rig.poses[1].name, "Wave");
}

/// **Two poses on two keyframes is an animation**, without anything being
/// posed by hand in between. This is what turns a pose list into a way of
/// animating rather than a way of posing.
#[test]
fn two_keyed_poses_tween_between_them() {
    let (mut editor, id) = rigged_editor();

    // Two poses worth moving between.
    pose_to(&mut editor, id, Point::new(200.0, 120.0));
    save_pose(&mut editor, id, "Up");
    pose_to(&mut editor, id, Point::new(200.0, 280.0));
    save_pose(&mut editor, id, "Down");

    editor.selection.set([id]);

    // **The layer has to reach frame 12 before anything can be keyed there.**
    // A one-frame layer shows nothing past frame zero (§7 item 156), which is
    // correct and is also the trap: extending the span is its own action, as
    // it is in Animate.
    editor.doc.edit("Extend", |scene| {
        scene.set_frame_count(24);
    });

    // Frame 0 takes the first pose.
    editor.set_frame(0);
    apply_pose(&mut editor, id, 0);

    // Frame 12 takes the second, on a keyframe of its own.
    editor.set_frame(12);
    editor.run(buzz_ui::Command::InsertKeyframe);
    apply_pose(&mut editor, id, 1);

    // A classic tween between them.
    editor.set_frame(0);
    editor.run(buzz_ui::Command::CreateClassicTween);

    let start = rig_of(editor.doc.scene(), 0, id).armature.pose();
    let middle = rig_of(editor.doc.scene(), 6, id).armature.pose();
    let end = rig_of(editor.doc.scene(), 12, id).armature.pose();

    assert!(
        start.iter().zip(&end).any(|(a, b)| (a - b).abs() > 0.05),
        "the two keys should differ, or there is nothing to tween"
    );
    // The frame between them is genuinely between them, not a copy of either.
    let differs_from_start = middle.iter().zip(&start).any(|(a, b)| (a - b).abs() > 1e-6);
    let differs_from_end = middle.iter().zip(&end).any(|(a, b)| (a - b).abs() > 1e-6);
    assert!(
        differs_from_start && differs_from_end,
        "frame 6 is {middle:?}, between {start:?} and {end:?}"
    );
}

/// Mirroring gives the other side of a pose, so a set is half the work.
#[test]
fn a_mirrored_pose_reaches_the_other_way() {
    let (mut editor, id) = rigged_editor();
    pose_to(&mut editor, id, Point::new(280.0, 130.0));

    let before = rig_of(editor.doc.scene(), 0, id).armature.tip(1);
    editor.doc.edit("Mirror Pose", |scene| {
        scene.update_object(id, |target| {
            if let ObjectKind::Armature(rig) = &mut target.kind {
                let flipped = rig.armature.mirrored_pose();
                rig.armature.set_pose(&flipped);
            }
        });
    });
    let after = rig_of(editor.doc.scene(), 0, id).armature.tip(1);

    let root = rig_of(editor.doc.scene(), 0, id).armature.root;
    assert!(
        (before.x - root.x).signum() != (after.x - root.x).signum(),
        "it reached {before:?} and now reaches {after:?}, from {root:?}"
    );
}

/// A rig edited after a pose was saved keeps the pose it can still use, rather
/// than refusing the whole thing — `set_pose`'s own rule.
#[test]
fn a_pose_saved_before_a_bone_was_added_still_applies() {
    let (mut editor, id) = rigged_editor();
    pose_to(&mut editor, id, Point::new(200.0, 120.0));
    save_pose(&mut editor, id, "Reach");
    let saved = rig_of(editor.doc.scene(), 0, id).poses[0].angles.clone();
    assert_eq!(saved.len(), 2);

    editor.doc.edit("Add Bone", |scene| {
        rigging::add_bone(
            scene,
            0,
            id,
            Some(1),
            Point::new(300.0, 200.0),
            Point::new(340.0, 200.0),
        );
    });
    assert_eq!(rig_of(editor.doc.scene(), 0, id).armature.len(), 3);

    apply_pose(&mut editor, id, 0);
    let now = rig_of(editor.doc.scene(), 0, id).armature.pose();
    assert_eq!(now.len(), 3, "the new bone should still be there");
    for (a, b) in now.iter().zip(&saved) {
        assert!((a - b).abs() < 1e-9, "the two bones it knew about moved");
    }
}
