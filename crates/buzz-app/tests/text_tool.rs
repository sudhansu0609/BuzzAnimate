//! **Placing and re-typing vector text.**
//!
//! The glyph outlining is unit-tested in `buzz-text`; this checks the editor
//! wiring: a click places a text object that is an ordinary shape carrying its
//! string, and editing the string re-shapes the glyphs. Skips when the machine
//! has no system font (nothing gets placed), like the headless-GPU tests skip
//! without a GPU.

use buzz_app::editor::Editor;
use buzz_geom::Point;
use buzz_scene::{ObjectId, ObjectKind};

fn glyph_count(editor: &Editor, id: ObjectId) -> usize {
    match &editor.doc.scene().find_object(id).unwrap().1.kind {
        ObjectKind::Shape(shape) => shape.path.elements().len(),
        _ => 0,
    }
}

#[test]
fn placing_text_makes_an_editable_shape() {
    let mut editor = Editor::default();
    editor.place_text(Point::new(100.0, 100.0));

    let Some(id) = editor.selection.iter().next() else {
        eprintln!("skipping: no system font, nothing placed");
        return;
    };
    let (_, object) = editor.doc.scene().find_object(id).expect("the placed object");
    assert!(object.text.is_some(), "the string rides on the object");
    assert!(matches!(object.kind, ObjectKind::Shape(_)), "text is an ordinary shape");
    assert!(glyph_count(&editor, id) > 0, "the glyphs were shaped into a path");
    assert_eq!(object.text.as_ref().unwrap().content, "Text");
}

#[test]
fn editing_text_reshapes_the_glyphs() {
    let mut editor = Editor::default();
    editor.place_text(Point::new(0.0, 0.0));
    let Some(id) = editor.selection.iter().next() else {
        eprintln!("skipping: no system font");
        return;
    };

    let before = glyph_count(&editor, id);
    editor.set_text(id, "wwwwwwwwww".to_string(), 96.0);
    let after = glyph_count(&editor, id);

    assert_ne!(after, before, "a different, larger string should reshape the path");
    let (_, object) = editor.doc.scene().find_object(id).unwrap();
    assert_eq!(object.text.as_ref().unwrap().content, "wwwwwwwwww");
    assert_eq!(object.text.as_ref().unwrap().size, 96.0);
}
