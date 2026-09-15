//! **Re-weighting a drawing's outlines by eye.**
//!
//! Line weight is a decision an animator makes by comparing one line against
//! the rest of the picture, which is why this is a key you press until it looks
//! right rather than a number you type. What is tested here is the behaviour
//! that makes it safe to lean on: it moves only the outline, it moves every
//! selected outline by the same *proportion* so a drawing keeps its internal
//! weighting, it is reversible, and it never quietly does something to a shape
//! that has no outline at all.

use buzz_app::editor::Editor;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerId, ObjectId, ObjectKind, ShapeData};
use buzz_ui::Command;
use peniko::Color;

const INK: Color = Color::from_rgb8(0x10, 0x10, 0x10);
const PAINT: Color = Color::from_rgb8(0xC0, 0x30, 0x20);

fn editor() -> Editor {
    Editor::default()
}

fn stroked(editor: &mut Editor, layer: LayerId, x: f64, width: f64) -> ObjectId {
    let mut id = ObjectId(0);
    editor.doc.edit("draw", |scene| {
        id = scene
            .add_shape(
                layer,
                ShapeData::stroked(
                    Rect::new(x, 40.0, x + 100.0, 140.0).to_path(1e-9),
                    INK,
                    width,
                ),
            )
            .expect("a stroked shape");
    });
    id
}

fn filled(editor: &mut Editor, layer: LayerId, x: f64) -> ObjectId {
    let mut id = ObjectId(0);
    editor.doc.edit("draw", |scene| {
        id = scene
            .add_shape(
                layer,
                ShapeData::filled(Rect::new(x, 40.0, x + 100.0, 140.0).to_path(1e-9), PAINT),
            )
            .expect("a filled shape");
    });
    id
}

/// The outline width of a shape, or `None` if it has no outline.
fn width(editor: &Editor, id: ObjectId) -> Option<f64> {
    let (_, object) = editor.doc.scene().find_object(id)?;
    match &object.kind {
        ObjectKind::Shape(s) => s.stroke.as_ref().map(|k| k.width),
        _ => None,
    }
}

fn path_len(editor: &Editor, id: ObjectId) -> usize {
    let (_, object) = editor.doc.scene().find_object(id).expect("the object");
    match &object.kind {
        ObjectKind::Shape(s) => s.path.elements().len(),
        _ => 0,
    }
}

/// **Thicker is thicker, thinner is thinner**, and the shape itself does not
/// move. The path is the drawing; only its weight was asked about.
#[test]
fn thickening_changes_the_outline_and_not_the_drawing() {
    let mut e = editor();
    let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
    let id = stroked(&mut e, layer, 0.0, 4.0);
    let before = path_len(&e, id);

    e.selection.select_one(id);
    e.run(Command::ThickenStroke);
    let thick = width(&e, id).expect("still an outline");
    assert!(thick > 4.0, "the outline did not thicken: {thick}");
    assert_eq!(path_len(&e, id), before, "the path was reshaped");

    e.run(Command::ThinStroke);
    let back = width(&e, id).expect("still an outline");
    assert!(back < thick, "the outline did not thin again: {back}");
}

/// **A fill is not an outline.** Brush strokes in this program are filled
/// paths, and widening one is a different operation with a different name
/// (Expand Fill). Touching it here would silently reshape artwork.
#[test]
fn a_shape_with_no_outline_is_left_alone() {
    let mut e = editor();
    let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
    let id = filled(&mut e, layer, 0.0);
    let before = path_len(&e, id);

    e.selection.select_one(id);
    e.run(Command::ThickenStroke);

    assert_eq!(width(&e, id), None, "a fill was given an outline it never had");
    assert_eq!(path_len(&e, id), before, "a fill was reshaped");
}

/// **Every outline moves by the same proportion.**
///
/// The reason the step multiplies instead of adding: a drawing whose heavy
/// lines and fine lines were shifted by the same *amount* would come back with
/// its weighting flattened, which is the thing the animator was looking at.
#[test]
fn a_drawings_internal_weighting_survives() {
    let mut e = editor();
    let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
    let heavy = stroked(&mut e, layer, 0.0, 8.0);
    let fine = stroked(&mut e, layer, 200.0, 1.0);

    e.selection.select_one(heavy);
    e.selection.toggle(fine);
    e.run(Command::ThickenStroke);

    let (h, f) = (
        width(&e, heavy).expect("heavy"),
        width(&e, fine).expect("fine"),
    );
    assert!(
        ((h / f) - 8.0).abs() < 1e-9,
        "the eight-to-one weighting came back as {:.3} to one",
        h / f
    );
}

/// **One undo takes the whole press back**, however many lines it moved.
#[test]
fn re_weighting_is_one_undo_step() {
    let mut e = editor();
    let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
    let a = stroked(&mut e, layer, 0.0, 3.0);
    let b = stroked(&mut e, layer, 200.0, 3.0);

    e.selection.select_one(a);
    e.selection.toggle(b);
    e.run(Command::ThickenStroke);
    assert!(width(&e, a).unwrap() > 3.0 && width(&e, b).unwrap() > 3.0);

    e.run(Command::Undo);
    assert!((width(&e, a).unwrap() - 3.0).abs() < 1e-9, "the first line did not come back");
    assert!((width(&e, b).unwrap() - 3.0).abs() < 1e-9, "the second line did not come back");
}
