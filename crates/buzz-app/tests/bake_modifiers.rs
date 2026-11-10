//! **Baking a live modifier down to keyframes.**
//!
//! The inverse of adding one: evaluate the modifier across the film, write the
//! result as ordinary keyframes, and drop the modifier so the motion is now
//! hand-editable and no longer re-computes.

use buzz_app::editor::Editor;
use buzz_geom::{Point, Rect, Shape as _};
use buzz_scene::{LayerId, Modifier, ObjectId, ShapeData};
use buzz_ui::Command;
use peniko::Color;

fn shape_with_wiggle() -> (Editor, LayerId, ObjectId) {
    let mut editor = Editor::default();
    let layer = editor
        .doc
        .scene()
        .layers()
        .iter()
        .next()
        .expect("a default layer")
        .id;

    let mut id = None;
    editor.doc.edit("Add Shape", |scene| {
        id = scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
        );
        // A span for the wiggle to play across.
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(20);
        });
    });
    let id = id.expect("a shape");
    editor.doc.edit("Add Wiggle", |scene| {
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Wiggle {
                amplitude: 20.0,
                frequency: 2.0,
            });
        });
    });
    editor.selection.select_one(id);
    (editor, layer, id)
}

fn translation(editor: &Editor, layer: LayerId, id: ObjectId, frame: u32) -> Point {
    let t = editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .unwrap()
        .frames
        .resolved_at(frame)
        .iter()
        .find(|o| o.id == id)
        .unwrap()
        .transform
        .translation();
    Point::new(t.x, t.y)
}

#[test]
fn baking_writes_keyframes_and_drops_the_modifier() {
    let (mut editor, layer, id) = shape_with_wiggle();
    assert_eq!(
        editor
            .doc
            .scene()
            .find_object(id)
            .unwrap()
            .1
            .modifiers
            .len(),
        1,
        "the wiggle is attached"
    );

    editor.run(Command::BakeModifiers);

    // The modifier is gone...
    assert!(
        editor
            .doc
            .scene()
            .find_object(id)
            .unwrap()
            .1
            .modifiers
            .is_empty(),
        "baking removes the live modifier"
    );
    // ...keys were written on twos...
    let l = editor.doc.scene().layers().get(layer).unwrap();
    assert!(l.frames.is_keyframe(0) && l.frames.is_keyframe(10));
    // ...and the baked motion actually differs frame to frame.
    let a = translation(&editor, layer, id, 0);
    let b = translation(&editor, layer, id, 10);
    assert!(
        a.distance(b) > 1.0,
        "the baked wiggle should move the object: {a:?} vs {b:?}"
    );
}

#[test]
fn baking_is_one_undo_step() {
    let (mut editor, _layer, id) = shape_with_wiggle();
    editor.run(Command::BakeModifiers);
    assert!(
        editor
            .doc
            .scene()
            .find_object(id)
            .unwrap()
            .1
            .modifiers
            .is_empty()
    );

    editor.doc.undo();
    assert_eq!(
        editor
            .doc
            .scene()
            .find_object(id)
            .unwrap()
            .1
            .modifiers
            .len(),
        1,
        "one Ctrl+Z restores the live modifier"
    );
}

/// **A baked pendulum lands exactly on the live swing at every key.**
///
/// Live and baked are one calculation delivered two ways; the baker keys every
/// second frame and tweens between, so the keyed frames must match what the
/// modifier evaluated, to the last bit of arithmetic.
#[test]
fn a_baked_pendulum_matches_the_live_swing_at_its_keys() {
    let (mut editor, layer, id) = shape_with_wiggle();
    editor.doc.edit("Swap for a pendulum", |scene| {
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers = vec![Modifier::Pendulum {
                period: 1.0,
                damping: 0.1,
                reaction: 1.0,
                initial_angle: 25.0,
                sway: 0.0,
                max_angle: 80.0,
                pivot_dx: 0.0,
                pivot_dy: 0.0,
            }];
        });
    });

    let live: Vec<(u32, buzz_geom::Affine)> = {
        let scene = editor.doc.scene();
        (0..=20u32)
            .step_by(2)
            .map(|frame| {
                let object = scene
                    .layers()
                    .get(layer)
                    .unwrap()
                    .frames
                    .resolved_at(frame)
                    .iter()
                    .find(|o| o.id == id)
                    .cloned()
                    .unwrap();
                let eval = scene.modified_object_at(layer, &object, frame).unwrap();
                (frame, eval.prepend * object.transform)
            })
            .collect()
    };
    assert!(
        live.iter().any(|(_, t)| (t.as_coeffs()[1]).abs() > 0.05),
        "the pendulum should actually be swinging"
    );

    editor.run(Command::BakeModifiers);
    assert!(editor.doc.scene().find_object(id).unwrap().1.modifiers.is_empty());

    for (frame, want) in live {
        let got = editor
            .doc
            .scene()
            .layers()
            .get(layer)
            .unwrap()
            .frames
            .resolved_at(frame)
            .iter()
            .find(|o| o.id == id)
            .unwrap()
            .transform;
        for (g, w) in got.as_coeffs().iter().zip(want.as_coeffs()) {
            assert!((g - w).abs() < 1e-9, "frame {frame}: baked {got:?} vs live {want:?}");
        }
    }
}

/// **Hair / cloth deforms the drawing, so baking keeps it live.**
///
/// A keyframe holds a placement; a bent drawing is not one. Baking an object
/// whose only modifier is a hair bend must leave it alone (no pointless column
/// of keys), and baking one that also has a wiggle bakes the wiggle and keeps
/// the bend.
#[test]
fn baking_leaves_hair_live_and_bakes_the_rest() {
    let (mut editor, layer, id) = shape_with_wiggle();
    editor.doc.edit("Hair only", |scene| {
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers = vec![Modifier::hair()];
        });
    });
    editor.run(Command::BakeModifiers);
    let object = editor.doc.scene().find_object(id).unwrap().1.clone();
    assert_eq!(object.modifiers, vec![Modifier::hair()], "the bend is still there");
    let l = editor.doc.scene().layers().get(layer).unwrap();
    assert!(!l.frames.is_keyframe(10), "nothing was keyed");

    editor.doc.edit("Add a wiggle too", |scene| {
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Wiggle { amplitude: 20.0, frequency: 2.0 });
        });
    });
    editor.run(Command::BakeModifiers);
    let object = editor.doc.scene().find_object(id).unwrap().1.clone();
    assert_eq!(object.modifiers, vec![Modifier::hair()], "the wiggle went into keys, the bend stayed");
    let l = editor.doc.scene().layers().get(layer).unwrap();
    assert!(l.frames.is_keyframe(10), "the wiggle was keyed");
}
