//! **Setting and clearing a turnaround's back view.**
//!
//! The render swap is a GPU test elsewhere; this checks the editor commands:
//! Set Reverse takes two selected objects and folds the second into the first as
//! its back, and Clear Reverse takes it off — both one undo step.

use buzz_app::editor::Editor;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerId, LayerKind, ObjectId, ShapeData};
use buzz_ui::Command;
use peniko::Color;

fn two_shapes() -> (Editor, LayerId, ObjectId, ObjectId) {
    let mut editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    let (mut front, mut back) = (None, None);
    editor.doc.edit("setup", |scene| {
        front = scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                Color::from_rgb8(0xFF, 0, 0),
            ),
        );
        back = scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                Color::from_rgb8(0, 0, 0xFF),
            ),
        );
    });
    (editor, layer, front.unwrap(), back.unwrap())
}

fn has_reverse(editor: &Editor, id: ObjectId) -> bool {
    editor
        .doc
        .scene()
        .find_object(id)
        .is_some_and(|(_, o)| o.reverse.is_some())
}

#[test]
fn set_reverse_folds_the_back_into_the_front() {
    let (mut editor, _layer, front, back) = two_shapes();
    editor.selection.select_one(front);
    editor.selection.toggle(back);

    editor.run(Command::SetReverse);

    assert!(has_reverse(&editor, front), "the front carries the back view");
    assert!(
        editor.doc.scene().find_object(back).is_none(),
        "the back left the stage — it lives inside the front now"
    );

    editor.doc.undo();
    assert!(!has_reverse(&editor, front), "undo removes the reverse");
    assert!(editor.doc.scene().find_object(back).is_some(), "and brings the back back");
}

#[test]
fn clear_reverse_takes_the_back_off() {
    let (mut editor, _layer, front, back) = two_shapes();
    editor.selection.select_one(front);
    editor.selection.toggle(back);
    editor.run(Command::SetReverse);
    assert!(has_reverse(&editor, front));

    editor.selection.select_one(front);
    editor.run(Command::ClearReverse);
    assert!(!has_reverse(&editor, front), "the reverse is gone");
}

#[test]
fn set_reverse_needs_exactly_two() {
    let (mut editor, _layer, front, _back) = two_shapes();
    editor.selection.select_one(front); // only one
    editor.run(Command::SetReverse);
    assert!(!has_reverse(&editor, front), "one object is not enough to set a reverse");
}
