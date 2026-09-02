//! **Applying a procedural texture to a shape.**
//!
//! The tile generation is unit-tested in `buzz-scene`; this checks the editor
//! wiring: `apply_texture` bakes a tile, adds it to the image library, and turns
//! the selected shape's fill into a *tiling* image fill — one undo step, and a
//! no-op when nothing is selected.

use buzz_app::editor::Editor;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerId, ObjectId, Paint, ShapeData, TextureKind};
use peniko::Color;

fn editor_with_a_shape() -> (Editor, LayerId, ObjectId) {
    let mut editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    let mut id = None;
    editor.doc.edit("setup", |scene| {
        id = scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 200.0, 120.0).to_path(1e-9),
                Color::from_rgb8(0x33, 0x66, 0x99),
            ),
        );
    });
    (editor, layer, id.unwrap())
}

fn fill_paint(editor: &Editor, id: ObjectId) -> Option<Paint> {
    match &editor.doc.scene().find_object(id)?.1.kind {
        buzz_scene::ObjectKind::Shape(s) => s.fill.as_ref().map(|f| f.paint.clone()),
        _ => None,
    }
}

#[test]
fn apply_texture_makes_a_tiling_image_fill() {
    let (mut editor, _layer, id) = editor_with_a_shape();
    let images_before = editor.doc.scene().images().len();
    editor.selection.select_one(id);

    editor.apply_texture(TextureKind::Checker);

    // The fill is now a tiling image, backed by a freshly added asset.
    match fill_paint(&editor, id) {
        Some(Paint::Image(fill)) => assert!(fill.tile, "the texture fill must tile"),
        other => panic!("expected a tiling image fill, got {other:?}"),
    }
    assert_eq!(
        editor.doc.scene().images().len(),
        images_before + 1,
        "the baked tile was added to the library"
    );

    // One undo step brings the solid fill back.
    editor.doc.undo();
    match fill_paint(&editor, id) {
        Some(Paint::Solid(_)) => {}
        other => panic!("undo should restore the solid fill, got {other:?}"),
    }
}

#[test]
fn apply_texture_needs_a_selection() {
    let (mut editor, _layer, id) = editor_with_a_shape();
    // Nothing selected.
    editor.apply_texture(TextureKind::Dots);
    assert!(
        matches!(fill_paint(&editor, id), Some(Paint::Solid(_))),
        "with no selection the fill is untouched"
    );
}
