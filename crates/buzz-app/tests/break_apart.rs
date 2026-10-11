//! **Breaking artwork apart one level** — Animate's Modify ▸ Break Apart.
//!
//! An image-filled shape bakes its transform so it can be cut directly, a
//! group frees its children, an instance drops to its symbol's contents,
//! text drops its editable link, and a warp bakes to what its handles
//! currently make. Fixture idiom from `trace_bitmap.rs` and `bitmap_tools.rs`.

use std::sync::Arc;

use buzz_app::editor::Editor;
use buzz_app::tools::ToolAction;
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_scene::{
    FillSpec, ImageAsset, ImageFill, ImageId, LayerKind, ObjectId, ObjectKind, Paint, PaintBlend,
    Scene, ShapeData,
};
use buzz_ui::Command;

/// A red disc on blue ground — the same stand-in `bitmap_tools.rs` uses.
fn disc_picture(size: u32, radius: f64) -> Arc<ImageAsset> {
    let c = f64::from(size) / 2.0;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((f64::from(x) + 0.5 - c).powi(2) + (f64::from(y) + 0.5 - c).powi(2)).sqrt();
            if d <= radius {
                pixels.extend_from_slice(&[220, 30, 30, 255]);
            } else {
                pixels.extend_from_slice(&[40, 80, 200, 255]);
            }
        }
    }
    Arc::new(ImageAsset::from_pixels(
        ImageId(1),
        "Photo",
        size,
        size,
        Arc::new(pixels),
    ))
}

/// An editor holding one bitmap laid across `area`, with `transform` applied
/// on top — the `trace_bitmap.rs` fixture idiom, extended with a transform
/// since baking one away is the whole point of Break Apart on a picture.
fn editor_with_transformed_picture(
    area: Rect,
    asset: Arc<ImageAsset>,
    transform: Affine,
) -> (Editor, ObjectId) {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Photo", LayerKind::Normal);
    let fill = ImageFill::new(asset, area);
    let id = scene
        .add_shape(
            layer,
            ShapeData {
                path: area.to_path(1e-9),
                fill: Some(FillSpec::image(fill)),
                stroke: None,
                blend: PaintBlend::Normal,
            },
        )
        .expect("on the stage");
    scene.update_object(id, |o| o.transform = transform);
    (Editor::new(Document::new(scene)), id)
}

/// Every object on the first layer of the current frame.
fn objects(editor: &Editor) -> Vec<(ObjectId, Rect)> {
    let scene = editor.doc.scene();
    scene
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(editor.current_frame).iter())
        .map(|o| (o.id, scene.resolved_bounds(o)))
        .collect()
}

fn close(a: Rect, b: Rect, tol: f64) -> bool {
    (a.x0 - b.x0).abs() < tol
        && (a.y0 - b.y0).abs() < tol
        && (a.x1 - b.x1).abs() < tol
        && (a.y1 - b.y1).abs() < tol
}

// -- 1. an image shape bakes its transform -----------------------------------

/// **An image-filled shape bakes its transform.** The path, the fill and the
/// object all move it; Break Apart folds all three into the path and the
/// fill, and turns tiling on, so the Lasso, Eraser and Wand can cut it
/// directly rather than needing another break first.
#[test]
fn breaking_apart_an_image_shape_bakes_its_transform() {
    let area = Rect::new(0.0, 0.0, 100.0, 60.0);
    let transform = Affine::rotate(0.3) * Affine::scale(1.5);
    let (mut editor, id) =
        editor_with_transformed_picture(area, disc_picture(64, 20.0), transform);

    let world_bounds_before = {
        let scene = editor.doc.scene();
        let object = scene.find_object(id).expect("the picture").1;
        scene.resolved_bounds(object)
    };
    let disc_world_center = transform * area.center();

    editor.selection.select_one(id);
    editor.run(Command::BreakApart);

    let after = objects(&editor);
    assert_eq!(after.len(), 1, "one shape in, one shape out");
    let (new_id, _) = after[0];

    let scene = editor.doc.scene();
    let object = scene.find_object(new_id).expect("the baked shape").1;
    assert_eq!(
        object.transform,
        Affine::IDENTITY,
        "the object's transform should be baked away"
    );

    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("not a shape")
    };
    let baked_bounds = shape.path.bounding_box();
    assert!(
        close(baked_bounds, world_bounds_before, 1e-6),
        "the path bounds should equal the old world bounds: {baked_bounds:?} vs {world_bounds_before:?}"
    );

    let Some(Paint::Image(fill)) = shape.fill.as_ref().map(|f| &f.paint) else {
        panic!("lost the picture")
    };
    assert!(fill.tile, "tiling should be turned on");
    // The fill transform is T*old: it should carry the unit square to the
    // same world placement the path bounds just landed at.
    let unit_square = Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9);
    let fill_bounds = (fill.transform * unit_square).bounding_box();
    assert!(
        close(fill_bounds, baked_bounds, 1e-6),
        "the fill transform should place the same world rect as the path: {fill_bounds:?} vs {baked_bounds:?}"
    );

    // The wand can cut the baked shape directly — the point Break Apart is
    // for — and what it finds is centred where the disc is, and smaller than
    // the whole baked rectangle, rather than the wand failing outright or
    // taking the whole shape as one region.
    editor.apply(ToolAction::WandAt {
        point: disc_world_center,
        additive: false,
    });
    let after = objects(&editor);
    assert_eq!(
        after.len(),
        2,
        "the wand should have cut the disc out of the baked shape"
    );
    let after_wand = after
        .into_iter()
        .find(|(oid, _)| editor.selection.contains(*oid))
        .map(|(_, b)| b)
        .expect("the wand's own piece should be selected");

    assert!(
        after_wand.width() < world_bounds_before.width()
            && after_wand.height() < world_bounds_before.height(),
        "the wand should have taken less than the whole baked shape: {after_wand:?} vs {world_bounds_before:?}"
    );
    assert!(
        (after_wand.center().x - disc_world_center.x).abs() < 15.0
            && (after_wand.center().y - disc_world_center.y).abs() < 15.0,
        "the wand's piece should be centred where the disc is: {after_wand:?} vs {disc_world_center:?}"
    );
}

// -- 2 & 3. cutting what came out of Break Apart ------------------------------

/// **A Lasso cut afterwards gives two objects, both still bitmaps.**
#[test]
fn a_lasso_cut_after_breaking_apart_keeps_the_picture_on_both_pieces() {
    let area = Rect::new(0.0, 0.0, 200.0, 100.0);
    let transform = Affine::scale(1.2);
    let (mut editor, id) =
        editor_with_transformed_picture(area, disc_picture(64, 20.0), transform);
    editor.selection.select_one(id);
    editor.run(Command::BreakApart);
    let baked = objects(&editor)[0].1;

    editor.apply(ToolAction::PickInRegion {
        region: {
            let mut path = buzz_geom::BezPath::new();
            let (mx, y0, y1) = (baked.center().x, baked.y0 - 10.0, baked.y1 + 10.0);
            path.move_to(Point::new(baked.x0 - 10.0, y0));
            path.line_to(Point::new(mx, y0));
            path.line_to(Point::new(mx, y1));
            path.line_to(Point::new(baked.x0 - 10.0, y1));
            path.close_path();
            path
        },
        additive: false,
    });

    let after = objects(&editor);
    assert_eq!(after.len(), 2, "the lasso should have cut the baked shape");
    let scene = editor.doc.scene();
    for (oid, _) in &after {
        let object = scene.find_object(*oid).expect("a piece").1;
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("a cut piece stopped being a shape")
        };
        assert!(
            matches!(shape.fill.as_ref().map(|f| &f.paint), Some(Paint::Image(_))),
            "a cut piece lost its picture"
        );
    }
}

/// **An Eraser drag reduces the area and the fill stays a picture.**
#[test]
fn an_eraser_drag_after_breaking_apart_shrinks_it_and_keeps_the_picture() {
    let area = Rect::new(0.0, 0.0, 200.0, 100.0);
    let transform = Affine::scale(1.2);
    let (mut editor, id) =
        editor_with_transformed_picture(area, disc_picture(64, 20.0), transform);
    editor.selection.select_one(id);
    editor.run(Command::BreakApart);
    let (baked_id, baked_bounds) = objects(&editor)[0];
    let area_before = baked_bounds.width() * baked_bounds.height();

    let mut cutter = buzz_geom::BezPath::new();
    cutter.move_to(Point::new(baked_bounds.x0 - 20.0, baked_bounds.center().y));
    cutter.line_to(Point::new(baked_bounds.x1 + 20.0, baked_bounds.center().y));
    editor.apply(ToolAction::Erase {
        path: cutter,
        width: baked_bounds.height() * 0.6,
    });

    let after = objects(&editor);
    let area_after: f64 = after.iter().map(|(_, b)| b.width() * b.height()).sum();
    assert!(
        area_after < area_before * 0.9,
        "the eraser did not reduce the area: {area_before} -> {area_after} \
         (fix the eraser if a plain drag through a baked bitmap does not cut it)"
    );

    let scene = editor.doc.scene();
    let still_has_picture = after.iter().any(|(oid, _)| {
        matches!(
            scene.find_object(*oid).map(|(_, o)| &o.kind),
            Some(ObjectKind::Shape(s)) if matches!(s.fill.as_ref().map(|f| &f.paint), Some(Paint::Image(_)))
        )
    });
    assert!(still_has_picture, "the erased artwork should still be a picture");
    let _ = baked_id;
}

// -- 4. a symbol of an image ---------------------------------------------------

/// **A symbol of a picture expands back to the picture**, with no instance
/// left, and one undo brings the instance back.
#[test]
fn breaking_apart_a_symbol_of_an_image_expands_it_and_undo_restores_the_instance() {
    let area = Rect::new(0.0, 0.0, 100.0, 60.0);
    let mut scene = Scene::default();
    let layer = scene.add_layer("Photo", LayerKind::Normal);
    // Already broken apart, `Scene::place_image`'s own convention: identity
    // transform, tiling on.
    let mut fill = ImageFill::new(disc_picture(64, 20.0), area);
    fill.tile = true;
    let id = scene
        .add_shape(
            layer,
            ShapeData {
                path: area.to_path(1e-9),
                fill: Some(FillSpec::image(fill)),
                stroke: None,
                blend: PaintBlend::Normal,
            },
        )
        .expect("on the stage");
    let mut editor = Editor::new(Document::new(scene));
    let before_bounds = objects(&editor)[0].1;

    editor.selection.select_one(id);
    editor.run(Command::ConvertToSymbol);
    let instance_id = objects(&editor)[0].0;
    assert_ne!(
        instance_id, id,
        "Convert to Symbol replaces the artwork with a fresh instance"
    );
    {
        let scene = editor.doc.scene();
        let object = scene.find_object(instance_id).expect("the instance").1;
        assert!(matches!(object.kind, ObjectKind::Instance(_)));
    }

    editor.selection.select_one(instance_id);
    editor.run(Command::BreakApart);

    let after = objects(&editor);
    assert_eq!(after.len(), 1, "one picture should have come back");
    let (expanded_id, expanded_bounds) = after[0];
    {
        let scene = editor.doc.scene();
        let object = scene.find_object(expanded_id).expect("the expanded shape").1;
        assert!(
            !matches!(object.kind, ObjectKind::Instance(_)),
            "no instance should remain"
        );
    }
    assert!(
        close(expanded_bounds, before_bounds, 1.0),
        "the bounds should match: {expanded_bounds:?} vs {before_bounds:?}"
    );

    editor.run(Command::Undo);
    let scene = editor.doc.scene();
    let restored = scene.find_object(instance_id);
    assert!(restored.is_some(), "one undo should bring the instance back");
    assert!(matches!(restored.unwrap().1.kind, ObjectKind::Instance(_)));
}

// -- 5. a group -----------------------------------------------------------------

/// **A group frees its children.**
#[test]
fn breaking_apart_a_group_frees_its_children() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let a = scene
        .add_shape(
            layer,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 20.0, 20.0).to_path(1e-9),
                peniko::Color::from_rgb8(0xC0, 0x20, 0x20),
            ),
        )
        .expect("shape a");
    let b = scene
        .add_shape(
            layer,
            ShapeData::filled(
                Rect::new(40.0, 0.0, 60.0, 20.0).to_path(1e-9),
                peniko::Color::from_rgb8(0x20, 0xC0, 0x20),
            ),
        )
        .expect("shape b");
    let mut editor = Editor::new(Document::new(scene));
    editor.selection.set(vec![a, b]);
    editor.run(Command::GroupSelection);
    let group_id = objects(&editor)[0].0;
    {
        let scene = editor.doc.scene();
        let object = scene.find_object(group_id).expect("the group").1;
        assert!(matches!(object.kind, ObjectKind::Group(_)));
    }

    editor.selection.select_one(group_id);
    editor.run(Command::BreakApart);

    let after = objects(&editor);
    assert_eq!(after.len(), 2, "the group's two children should be free");
    let scene = editor.doc.scene();
    for (oid, _) in &after {
        let object = scene.find_object(*oid).expect("a freed child").1;
        assert!(
            matches!(object.kind, ObjectKind::Shape(_)),
            "a freed child is not a shape"
        );
    }
}

// -- 6. text ----------------------------------------------------------------

/// **Text drops its editable link; the glyph outlines stay.**
#[test]
fn breaking_apart_text_drops_the_link_and_keeps_the_outlines() {
    let mut scene = Scene::default();
    scene.add_layer("Art", LayerKind::Normal);
    let mut editor = Editor::new(Document::new(scene));
    editor.place_text(Point::new(100.0, 100.0));
    let Some(id) = editor.selection.iter().next() else {
        panic!("place_text did not select the text it made — no font available?");
    };

    let path_before = {
        let scene = editor.doc.scene();
        let object = scene.find_object(id).expect("the text").1;
        assert!(object.text.is_some(), "should have started as text");
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("not a shape")
        };
        shape.path.clone()
    };

    editor.selection.select_one(id);
    editor.run(Command::BreakApart);

    let scene = editor.doc.scene();
    let object = scene.find_object(id).expect("the same object, broken apart").1;
    assert!(object.text.is_none(), "the editable link should be gone");
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("not a shape")
    };
    assert_eq!(
        shape.path, path_before,
        "the glyph outlines should be unchanged"
    );
}

// -- 7. the shortcut ----------------------------------------------------------

/// **Ctrl+B maps to Break Apart**, and does not collide with anything else —
/// Ctrl+Shift+B is already Toggle Bone Rig.
#[test]
fn ctrl_b_maps_to_break_apart_with_no_duplicate_shortcuts() {
    let shortcut = Command::BreakApart
        .shortcut()
        .expect("Break Apart should have a shortcut");
    assert_eq!(shortcut.logical_key, egui::Key::B);
    assert!(shortcut.modifiers.command, "should be a Ctrl/Cmd shortcut");
    assert!(
        !shortcut.modifiers.shift,
        "not Ctrl+Shift+B, which is already Toggle Bone Rig"
    );

    let mut seen: std::collections::HashMap<String, Command> = std::collections::HashMap::new();
    for c in buzz_ui::command::palette_commands() {
        let Some(sc) = c.shortcut() else { continue };
        let key = format!("{:?}+{:?}", sc.modifiers, sc.logical_key);
        if let Some(previous) = seen.insert(key.clone(), c) {
            // RunScript and TestMovie deliberately share Ctrl+Enter:
            // `script_shortcuts` decides between them by whether the Actions
            // code editor has focus, so the chord never actually fires both.
            let allowed = matches!(
                (previous, c),
                (Command::ZoomShowAll, Command::TogglePasteboard)
                    | (Command::TogglePasteboard, Command::ZoomShowAll)
                    | (Command::RunScript, Command::TestMovie)
                    | (Command::TestMovie, Command::RunScript)
            );
            assert!(allowed, "{previous:?} and {c:?} both bind {key}");
        }
    }
}

// -- 8. a plain shape ---------------------------------------------------------

/// **A plain shape is a no-op**: a status message, and no undo step.
#[test]
fn breaking_apart_a_plain_shape_says_so_and_leaves_no_undo_step() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let id = scene
        .add_shape(
            layer,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 20.0, 20.0).to_path(1e-9),
                peniko::Color::from_rgb8(0xC0, 0x20, 0x20),
            ),
        )
        .expect("a shape");
    let mut editor = Editor::new(Document::new(scene));
    let revision_before = editor.doc.scene().revision();

    editor.selection.select_one(id);
    editor.run(Command::BreakApart);

    assert_eq!(
        editor.doc.scene().revision(),
        revision_before,
        "a plain shape must leave no undo step"
    );
    assert!(
        editor.status.is_some(),
        "a plain shape should still say something"
    );
}
