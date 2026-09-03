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

// ---------------------------------------------------------------------------
// Parametric textures: the recipe stays with the tile
// ---------------------------------------------------------------------------

/// The recipe travels with the asset, which is what makes a texture something
/// you can come back to rather than a one-way bake.
#[test]
fn an_applied_texture_remembers_how_it_was_made() {
    let (mut editor, _layer, id) = editor_with_a_shape();
    editor.selection.select_one(id);
    editor.apply_texture(TextureKind::Bricks);

    let (recipe, _cell, _angle) = editor
        .selected_texture()
        .expect("the selection wears a texture it can describe");
    assert_eq!(recipe.kind, TextureKind::Bricks);
    assert_eq!(
        recipe.detail,
        TextureKind::Bricks.default_detail(),
        "and at the detail that suits a wall"
    );
}

/// Re-tuning changes the picture without going through undo and re-apply.
#[test]
fn retexturing_changes_the_tile_in_place() {
    let (mut editor, _layer, id) = editor_with_a_shape();
    editor.selection.select_one(id);
    editor.apply_texture(TextureKind::Checker);
    let before = match fill_paint(&editor, id) {
        Some(Paint::Image(img)) => img.asset.pixels.clone(),
        other => panic!("expected an image fill, got {other:?}"),
    };

    let (recipe, cell, angle) = editor.selected_texture().expect("a texture");
    let coarser = buzz_scene::TextureRecipe {
        detail: 2,
        ..recipe
    };
    editor.retexture(coarser, Some((cell, angle)));

    let after = match fill_paint(&editor, id) {
        Some(Paint::Image(img)) => img.asset.pixels.clone(),
        other => panic!("expected an image fill, got {other:?}"),
    };
    assert_ne!(before, after, "a coarser checker is a different tile");
    assert_eq!(
        editor.selected_texture().expect("still textured").0.detail,
        2,
        "and the shape now says so"
    );
}

/// One texture on many shapes is one tile, not one tile each: the library is
/// searched for the recipe before anything is baked.
#[test]
fn the_same_recipe_is_baked_once() {
    let (mut editor, layer, first) = editor_with_a_shape();
    let mut second = None;
    editor.doc.edit("second shape", |scene| {
        second = scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(300.0, 0.0, 460.0, 120.0).to_path(1e-9),
                Color::from_rgb8(0x99, 0x66, 0x33),
            ),
        );
    });
    let second = second.expect("a second shape");

    let before = editor.doc.scene().images().len();
    editor.selection.select_one(first);
    editor.apply_texture(TextureKind::Stripes);
    editor.selection.select_one(second);
    editor.apply_texture(TextureKind::Stripes);

    assert_eq!(
        editor.doc.scene().images().len(),
        before + 1,
        "the second shape should wear the tile the first one made"
    );
}

/// Two shapes wearing different textures have no single recipe to show, and the
/// panel must not offer one — nudging a slider would silently retexture both.
#[test]
fn a_mixed_selection_has_no_one_texture() {
    let (mut editor, layer, first) = editor_with_a_shape();
    let mut second = None;
    editor.doc.edit("second shape", |scene| {
        second = scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(300.0, 0.0, 460.0, 120.0).to_path(1e-9),
                Color::WHITE,
            ),
        );
    });
    let second = second.expect("a second shape");

    editor.selection.select_one(first);
    editor.apply_texture(TextureKind::Dots);
    editor.selection.select_one(second);
    editor.apply_texture(TextureKind::Wood);

    editor.selection.select_one(first);
    editor.selection.add(second);
    assert!(
        editor.selected_texture().is_none(),
        "two different textures have no single answer"
    );
}

/// **Drawing straight into a texture.** With the Texture fill chosen, a new
/// shape comes out textured rather than flat — no select-then-apply.
#[test]
fn a_new_shape_can_be_drawn_with_a_texture_fill() {
    let mut editor = Editor::default();
    editor.style.fill_kind = buzz_ui::FillKind::Texture;
    editor.style.fill_texture =
        buzz_scene::TextureRecipe::new(TextureKind::Hatch, Color::BLACK, Color::WHITE);
    editor.ensure_fill_texture();

    let asset = editor
        .style
        .fill_texture_asset
        .as_ref()
        .expect("the tile was baked into the document");
    assert_eq!(asset.recipe.map(|r| r.kind), Some(TextureKind::Hatch));

    let paint = editor
        .style
        .fill_for_new_shape(Rect::new(0.0, 0.0, 100.0, 100.0))
        .expect("a fill");
    match paint {
        Paint::Image(img) => assert!(img.tile, "a texture fill tiles"),
        other => panic!("expected a texture fill, got {other:?}"),
    }
}

/// **The on-stage gizmo.** A texture fill is scaled and turned by the same
/// three grips a gradient offers — Animate's Gradient Transform tool works a
/// bitmap fill too, and this is that.
#[test]
fn a_texture_fill_offers_transform_grips() {
    let (mut editor, _layer, id) = editor_with_a_shape();
    editor.selection.select_one(id);
    editor.apply_texture(TextureKind::Checker);

    let (handles, kind) = editor
        .selected_gradient_handles()
        .expect("a texture fill has grips, as a gradient does");
    assert_eq!(
        kind,
        buzz_scene::GradientKind::Linear,
        "reported linear so no focus grip is drawn — an image has no hot spot"
    );
    // The grips are the matrix's own parts, so they are not all the same point.
    assert_ne!(handles.center, handles.end);
    assert_ne!(handles.center, handles.width);
}

/// Dragging the end grip turns and scales the tile, and the shape keeps the
/// same texture while it does.
#[test]
fn dragging_a_grip_turns_the_texture() {
    use buzz_geom::Point;
    let (mut editor, _layer, id) = editor_with_a_shape();
    editor.selection.select_one(id);
    editor.apply_texture(TextureKind::Stripes);

    let (_, before_cell, before_angle) = editor.selected_texture().expect("a texture");
    let handles = editor.selected_gradient_handles().expect("grips").0;

    // Swing the first axis a quarter turn out and make it longer.
    let turned = Point::new(handles.center.x, handles.center.y + before_cell * 2.0);
    editor.apply(buzz_app::tools::ToolAction::DragGradient {
        grip: buzz_app::tools::GradientGrip::End,
        to: turned,
    });

    let (recipe, cell, angle) = editor
        .selected_texture()
        .expect("still the same texture, moved");
    assert_eq!(recipe.kind, TextureKind::Stripes, "the pattern is unchanged");
    assert!(cell > before_cell * 1.5, "the tile grew: {before_cell} to {cell}");
    assert!(
        (angle - before_angle).abs() > 1.0,
        "and turned: {before_angle} to {angle}"
    );
}
