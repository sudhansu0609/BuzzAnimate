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
        editor.doc.scene().find_object(id).unwrap().1.modifiers.len(),
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
    assert!(editor.doc.scene().find_object(id).unwrap().1.modifiers.is_empty());

    editor.doc.undo();
    assert_eq!(
        editor.doc.scene().find_object(id).unwrap().1.modifiers.len(),
        1,
        "one Ctrl+Z restores the live modifier"
    );
}
