//! Bringing a photograph in and taking it apart.
//!
//! The workflow this file is about, in the animator's words: *import a picture,
//! cut out the bit I want, throw the rest away, keep what is left as an asset.*
//!
//! Every test here drives the editor the way the pointer does — a tool
//! selected, a gesture made — rather than calling the geometry underneath, so
//! what is proved is that the **tool** works, not that a boolean does.

use std::sync::Arc;

use buzz_app::editor::Editor;
use buzz_app::tools::{Mods, ToolAction};
use buzz_doc::Document;
use buzz_geom::{BezPath, Point, Rect, Shape as _};
use buzz_scene::{
    FillSpec, ImageAsset, ImageFill, ImageId, LayerKind, ObjectKind, Paint, PaintBlend, Scene,
    ShapeData,
};
use buzz_ui::{Command, ToolId};
use peniko::Color;

/// A picture with a red disc on a blue ground — a stand-in for "the subject and
/// the sky behind it", which is the only distinction any of these tests need.
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

/// An editor holding one bitmap laid across `area`, already broken apart.
fn editor_with_picture(area: Rect, asset: Arc<ImageAsset>) -> Editor {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Photo", LayerKind::Normal);
    let fill = ImageFill::new(asset, area);
    scene.add_shape(
        layer,
        ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::image(fill)),
            stroke: None,
            blend: PaintBlend::Normal,
        },
    );
    Editor::new(Document::new(scene))
}

/// Every object on the first layer of the current frame.
fn objects(editor: &Editor) -> Vec<(buzz_scene::ObjectId, Rect)> {
    let scene = editor.doc.scene();
    scene
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(editor.current_frame).iter())
        .map(|o| (o.id, scene.resolved_bounds(o)))
        .collect()
}

/// A closed polygon through the given document points.
fn region(points: &[(f64, f64)]) -> BezPath {
    let mut path = BezPath::new();
    path.move_to(Point::new(points[0].0, points[0].1));
    for (x, y) in &points[1..] {
        path.line_to(Point::new(*x, *y));
    }
    path.close_path();
    path
}

// -- the Lasso ---------------------------------------------------------------

/// **A lasso round part of a drawing makes that part a thing.**
///
/// The claim in full: after the gesture there are two objects where there was
/// one, the selected one is the part that was enclosed, and the part that was
/// not enclosed is still there and still the same size it was.
#[test]
fn the_lasso_cuts_the_part_it_encloses_out_of_the_artwork() {
    let area = Rect::new(0.0, 0.0, 400.0, 200.0);
    let mut editor = editor_with_picture(area, disc_picture(64, 20.0));
    assert_eq!(objects(&editor).len(), 1);

    // Round the left third.
    editor.apply(ToolAction::PickInRegion {
        region: region(&[
            (-10.0, -10.0),
            (130.0, -10.0),
            (130.0, 210.0),
            (-10.0, 210.0),
        ]),
        additive: false,
    });

    let after = objects(&editor);
    assert_eq!(after.len(), 2, "the lasso should have made a second object");
    assert_eq!(
        editor.selection.len(),
        1,
        "exactly the piece that was caught is selected"
    );

    let selected = editor.selection.iter().next().expect("a selection");
    let caught = after
        .iter()
        .find(|(id, _)| *id == selected)
        .expect("the caught piece")
        .1;
    assert!(
        (caught.width() - 130.0).abs() < 1.0 && (caught.height() - 200.0).abs() < 1.0,
        "the caught piece should be the left third, and is {caught:?}"
    );

    let kept = after
        .iter()
        .find(|(id, _)| *id != selected)
        .expect("the rest")
        .1;
    assert!(
        (kept.width() - 270.0).abs() < 1.0,
        "the rest should be what is left, and is {kept:?}"
    );
}

/// **Then Delete removes it, and the rest stays.** This is the whole request:
/// take an area away and keep the rest to use.
#[test]
fn deleting_what_the_lasso_caught_leaves_the_rest_standing() {
    let area = Rect::new(0.0, 0.0, 400.0, 200.0);
    let mut editor = editor_with_picture(area, disc_picture(64, 20.0));

    editor.apply(ToolAction::PickInRegion {
        region: region(&[
            (-10.0, -10.0),
            (130.0, -10.0),
            (130.0, 210.0),
            (-10.0, 210.0),
        ]),
        additive: false,
    });
    editor.run(Command::Delete);

    let after = objects(&editor);
    assert_eq!(after.len(), 1, "one piece removed, one left");
    let kept = after[0].1;
    assert!(
        kept.min_x() > 129.0 && (kept.max_x() - 400.0).abs() < 1.0,
        "what is left should be the right-hand part, and is {kept:?}"
    );

    // And it is still a picture, not a flat colour: the fill survived the cut.
    let scene = editor.doc.scene();
    let object = scene.find_object(after[0].0).expect("the kept piece").1;
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("the kept piece stopped being a shape");
    };
    assert!(
        matches!(shape.fill.as_ref().map(|f| &f.paint), Some(Paint::Image(_))),
        "the picture was lost when the artwork was cut"
    );
}

/// A lasso that catches a whole shape selects it rather than cutting it in two
/// and leaving an empty husk behind.
#[test]
fn a_lasso_round_everything_selects_it_whole() {
    let area = Rect::new(0.0, 0.0, 400.0, 200.0);
    let mut editor = editor_with_picture(area, disc_picture(64, 20.0));

    editor.apply(ToolAction::PickInRegion {
        region: region(&[
            (-50.0, -50.0),
            (450.0, -50.0),
            (450.0, 250.0),
            (-50.0, 250.0),
        ]),
        additive: false,
    });

    assert_eq!(objects(&editor).len(), 1, "nothing was cut");
    assert_eq!(editor.selection.len(), 1, "and it is selected");
}

/// **The gesture itself works**, not only the action it raises: pressing,
/// dragging round and releasing with the Lasso cuts.
#[test]
fn drawing_a_loop_with_the_lasso_tool_cuts_the_artwork() {
    let area = Rect::new(0.0, 0.0, 400.0, 200.0);
    let mut editor = editor_with_picture(area, disc_picture(64, 20.0));
    editor.set_tool(ToolId::Lasso);

    let loop_points = [
        (-20.0, -20.0),
        (150.0, -20.0),
        (150.0, 100.0),
        (150.0, 220.0),
        (-20.0, 220.0),
    ];
    let camera = editor.camera;
    let screen = move |p: (f64, f64)| camera.doc_to_screen(Point::new(p.0, p.1));

    editor.pointer_down(screen(loop_points[0]), Mods::default());
    for p in &loop_points[1..] {
        editor.pointer_move(screen(*p), Mods::default());
    }
    editor.pointer_up(screen(loop_points[loop_points.len() - 1]));

    assert_eq!(
        objects(&editor).len(),
        2,
        "the drawn loop did not cut anything"
    );
    assert_eq!(editor.selection.len(), 1);
}

/// A stray click with the Lasso deselects. It does not carve a sliver out of
/// whatever happened to be under the pointer.
#[test]
fn a_click_with_the_lasso_takes_nothing() {
    let area = Rect::new(0.0, 0.0, 400.0, 200.0);
    let mut editor = editor_with_picture(area, disc_picture(64, 20.0));
    editor.set_tool(ToolId::Lasso);

    let at = editor.camera.doc_to_screen(Point::new(200.0, 100.0));
    editor.pointer_down(at, Mods::default());
    editor.pointer_up(at);

    assert_eq!(objects(&editor).len(), 1, "a click cut the artwork");
}

// -- the Magic Wand ----------------------------------------------------------

/// **Click the subject, get the subject.**
///
/// The disc is a fifth of the picture's width across. Clicking it must produce
/// a piece the size of the disc — not the whole rectangle, and not a speck.
#[test]
fn the_wand_takes_the_colour_region_it_is_clicked_on() {
    // A 256-pixel picture with a disc of radius 60, laid out at 1:1 so the
    // arithmetic below is in the same units as the pixels.
    let area = Rect::new(100.0, 100.0, 356.0, 356.0);
    let mut editor = editor_with_picture(area, disc_picture(256, 60.0));

    editor.apply(ToolAction::WandAt {
        point: area.center(),
        additive: false,
    });

    let after = objects(&editor);
    assert_eq!(after.len(), 2, "the wand should have cut out the disc");
    let selected = editor.selection.iter().next().expect("a selection");
    let caught = after.iter().find(|(id, _)| *id == selected).unwrap().1;

    assert!(
        (caught.width() - 120.0).abs() < 4.0 && (caught.height() - 120.0).abs() < 4.0,
        "the caught region should be the disc, 120 across; it is {caught:?}"
    );
    assert!(
        (caught.center().x - area.center().x).abs() < 2.0,
        "and centred where the disc is"
    );
}

/// **Click the sky, delete the sky.** The other half of the same tool, and the
/// one the request was actually about.
#[test]
fn the_wand_takes_the_background_and_deleting_it_leaves_the_subject() {
    let area = Rect::new(0.0, 0.0, 256.0, 256.0);
    let mut editor = editor_with_picture(area, disc_picture(256, 60.0));

    // A corner: ground, not subject.
    editor.apply(ToolAction::WandAt {
        point: Point::new(6.0, 6.0),
        additive: false,
    });
    editor.run(Command::Delete);

    let after = objects(&editor);
    assert_eq!(
        after.len(),
        1,
        "the ground should be gone and the disc left"
    );
    let subject = after[0].1;
    assert!(
        (subject.width() - 120.0).abs() < 4.0,
        "what is left should be the disc alone, and is {subject:?}"
    );

    // And it is a cut-out of the original photograph, ready to be used as an
    // asset: same picture, same place in it.
    let scene = editor.doc.scene();
    let object = scene.find_object(after[0].0).unwrap().1;
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("not a shape");
    };
    let Some(Paint::Image(fill)) = shape.fill.as_ref().map(|f| &f.paint) else {
        panic!("the cut-out lost its picture");
    };
    // The centre of the cut-out still reads the middle of the disc.
    let pixel = fill.to_pixel(subject.center()).expect("a pixel");
    assert!(
        (pixel.x - 128.0).abs() < 4.0 && (pixel.y - 128.0).abs() < 4.0,
        "the picture slid inside the cut-out: centre reads pixel {pixel:?}"
    );
}

/// Tolerance is the user's dial, and it does what it says.
#[test]
fn a_tolerance_of_zero_takes_only_the_exact_colour() {
    // A picture with a subject in two very close shades: at a tight tolerance
    // the wand takes one, at a loose one it takes both.
    let mut pixels = Vec::new();
    for y in 0..64u32 {
        for _ in 0..64u32 {
            if y < 32 {
                pixels.extend_from_slice(&[100, 100, 100, 255]);
            } else {
                pixels.extend_from_slice(&[110, 110, 110, 255]);
            }
        }
    }
    let asset = Arc::new(ImageAsset::from_pixels(
        ImageId(9),
        "Shades",
        64,
        64,
        Arc::new(pixels),
    ));

    let area = Rect::new(0.0, 0.0, 64.0, 64.0);

    let mut tight = editor_with_picture(area, Arc::clone(&asset));
    tight.style.wand.tolerance = 0.0;
    tight.apply(ToolAction::WandAt {
        point: Point::new(32.0, 8.0),
        additive: false,
    });
    let caught = objects(&tight)
        .into_iter()
        .find(|(id, _)| Some(*id) == tight.selection.iter().next())
        .expect("a piece")
        .1;
    assert!(
        (caught.height() - 32.0).abs() < 2.0,
        "at zero tolerance only the top half matches, but {caught:?} was taken"
    );

    let mut loose = editor_with_picture(area, asset);
    loose.style.wand.tolerance = 32.0 / 255.0;
    loose.apply(ToolAction::WandAt {
        point: Point::new(32.0, 8.0),
        additive: false,
    });
    assert_eq!(
        objects(&loose).len(),
        1,
        "at a loose tolerance the whole picture matches, so there is nothing to cut"
    );
}

/// On vector artwork the wand selects the shape. A region of one colour *is*
/// the shape someone drew, so there is nothing to trace.
#[test]
fn the_wand_on_a_drawn_shape_selects_that_shape() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
    let id = scene
        .add_shape(
            layer,
            ShapeData {
                path: rect.to_path(1e-9),
                fill: Some(FillSpec::solid(Color::from_rgb8(0x20, 0x80, 0x40))),
                stroke: None,
                blend: PaintBlend::Normal,
            },
        )
        .unwrap();
    let mut editor = Editor::new(Document::new(scene));

    editor.apply(ToolAction::WandAt {
        point: Point::new(50.0, 50.0),
        additive: false,
    });

    assert_eq!(objects(&editor).len(), 1, "nothing should have been cut");
    assert_eq!(editor.selection.iter().next(), Some(id));
}

/// A wand click on empty stage deselects rather than doing something surprising.
#[test]
fn the_wand_on_nothing_clears_the_selection() {
    let area = Rect::new(0.0, 0.0, 100.0, 100.0);
    let mut editor = editor_with_picture(area, disc_picture(64, 20.0));
    editor.apply(ToolAction::PickAt {
        point: Point::new(50.0, 50.0),
        additive: false,
    });
    assert_eq!(editor.selection.len(), 1);

    editor.apply(ToolAction::WandAt {
        point: Point::new(900.0, 900.0),
        additive: false,
    });
    assert!(editor.selection.is_empty());
}

// -- importing ---------------------------------------------------------------

/// **File ▸ Import Image, end to end**: a real PNG on disk becomes artwork on
/// the stage that the wand can immediately cut.
#[test]
fn an_imported_png_arrives_as_artwork_the_wand_can_cut() {
    let png = disc_picture(128, 40.0).encode_png().expect("encode");
    let dir = std::env::temp_dir().join(format!("buzz-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("village.png");
    std::fs::write(&path, &png).expect("write");

    let mut scene = Scene::default();
    scene.add_layer("Art", LayerKind::Normal);
    let mut editor = Editor::new(Document::new(scene));

    let name = editor.import_image(&path).expect("import");
    assert_eq!(name, "village");
    assert_eq!(editor.doc.scene().images().len(), 1);

    let placed = objects(&editor);
    assert_eq!(placed.len(), 1, "the picture should be on the stage");
    assert_eq!(
        editor.selection.len(),
        1,
        "and selected, ready to be moved into place"
    );
    // Placed at its natural size: 128 square fits inside any sane stage.
    assert!(
        (placed[0].1.width() - 128.0).abs() < 1.0,
        "expected natural size, got {:?}",
        placed[0].1
    );

    // And it is artwork, so the wand works on it with no Break Apart step.
    editor.apply(ToolAction::WandAt {
        point: placed[0].1.center(),
        additive: false,
    });
    assert_eq!(
        objects(&editor).len(),
        2,
        "an imported picture should be cuttable straight away"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A picture bigger than the stage is scaled to fit it, keeping its shape.
#[test]
fn an_oversized_picture_is_scaled_to_fit_the_stage() {
    let png = disc_picture(512, 100.0).encode_png().expect("encode");
    let dir = std::env::temp_dir().join(format!("buzz-import-big-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("wide.png");
    std::fs::write(&path, &png).expect("write");

    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(320.0, 240.0);
    scene.add_layer("Art", LayerKind::Normal);
    let mut editor = Editor::new(Document::new(scene));

    editor.import_image(&path).expect("import");
    let placed = objects(&editor)[0].1;

    assert!(
        (placed.height() - 240.0).abs() < 1.0,
        "should have been fitted to the stage's shorter side, got {placed:?}"
    );
    assert!(
        (placed.width() - placed.height()).abs() < 1.0,
        "a square picture must stay square: {placed:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A file that is not a picture is refused, and leaves nothing behind.
#[test]
fn a_file_that_is_not_a_picture_is_refused_cleanly() {
    let dir = std::env::temp_dir().join(format!("buzz-import-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("notes.png");
    std::fs::write(&path, b"this is not a picture").expect("write");

    let mut scene = Scene::default();
    scene.add_layer("Art", LayerKind::Normal);
    let mut editor = Editor::new(Document::new(scene));

    assert!(editor.import_image(&path).is_err());
    assert_eq!(
        editor.doc.scene().images().len(),
        0,
        "a refused import must not leave a library entry"
    );
    assert!(objects(&editor).is_empty(), "and nothing on the stage");

    let _ = std::fs::remove_dir_all(&dir);
}

// -- cost --------------------------------------------------------------------

/// **The wand answers while the pointer is still down.**
///
/// A four-megapixel photograph is what a phone takes, and this is the tool most
/// likely to be reached for on one. The budget is generous — it is one click,
/// not a per-frame cost — but it has to be a budget, because the alternative to
/// measuring it is finding out during a session.
#[test]
fn the_wand_is_prompt_on_a_photograph() {
    let area = Rect::new(0.0, 0.0, 2048.0, 2048.0);
    let mut editor = editor_with_picture(area, disc_picture(2048, 700.0));

    let start = std::time::Instant::now();
    editor.apply(ToolAction::WandAt {
        point: area.center(),
        additive: false,
    });
    let took = start.elapsed();

    assert_eq!(objects(&editor).len(), 2, "the wand caught nothing");
    assert!(
        took.as_millis() < 3000,
        "the wand took {took:?} on a four-megapixel picture"
    );
    eprintln!("magic wand on 4 megapixels: {took:?}");
}

// -- the soft brush ----------------------------------------------------------

/// **A soft brush stroke lands as artwork, and it fades at its edge.**
///
/// The whole of the request, driven the way the pointer drives it.
#[test]
fn a_soft_brush_stroke_paints_a_fading_bitmap() {
    let mut scene = Scene::default();
    scene.add_layer("Paint", LayerKind::Normal);
    let mut editor = Editor::new(Document::new(scene));

    editor.set_tool(ToolId::Brush);
    editor.style.brush.kind = buzz_ui::BrushKind::Raster;
    editor.style.brush.size = 40.0;
    editor.style.brush.hardness = 0.3;
    editor.style.fill_color = Color::from_rgb8(0x00, 0x00, 0x00);

    let camera = editor.camera;
    let screen = move |x: f64, y: f64| camera.doc_to_screen(Point::new(x, y));
    editor.pointer_down(screen(200.0, 300.0), Mods::default());
    for i in 1..=20 {
        editor.pointer_move(screen(200.0 + f64::from(i) * 15.0, 300.0), Mods::default());
    }
    editor.pointer_up(screen(500.0, 300.0));

    let painted = objects(&editor);
    assert_eq!(painted.len(), 1, "the stroke should have made one object");
    assert_eq!(
        editor.selection.len(),
        1,
        "and be selected, as a drawn shape is"
    );

    let scene = editor.doc.scene();
    let object = scene.find_object(painted[0].0).unwrap().1;
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("a painted stroke should be a shape");
    };
    let Some(Paint::Image(fill)) = shape.fill.as_ref().map(|f| &f.paint) else {
        panic!("a soft stroke should be filled with the pixels it painted");
    };

    // Down the middle of the stroke: solid. Out past its edge: nothing. And in
    // between: neither — which is the soft edge, and the reason for all of it.
    let read = |x: f64, y: f64| {
        let p = fill.to_pixel(Point::new(x, y)).expect("a pixel");
        fill.asset.pixel(p.x as i64, p.y as i64)[3]
    };
    let middle = read(350.0, 300.0);
    let edge = read(350.0, 317.0);
    let outside = read(350.0, 340.0);

    assert!(
        middle > 240,
        "the middle of the stroke is solid, not {middle}"
    );
    assert!(
        edge > 10 && edge < 240,
        "the edge should be part-transparent, and is {edge}"
    );
    assert!(
        outside < 10,
        "past the brush there is nothing, not {outside}"
    );

    // The document holds the pixels, so saving keeps them.
    assert_eq!(scene.images().len(), 1);
}

/// A hard raster brush is hard: the same gesture, a different edge.
#[test]
fn hardness_changes_the_edge_of_a_painted_stroke() {
    let paint = |hardness: f64| -> Vec<u8> {
        let mut scene = Scene::default();
        scene.add_layer("Paint", LayerKind::Normal);
        let mut editor = Editor::new(Document::new(scene));
        editor.set_tool(ToolId::Brush);
        editor.style.brush.kind = buzz_ui::BrushKind::Raster;
        editor.style.brush.size = 60.0;
        editor.style.brush.hardness = hardness;

        let camera = editor.camera;
        let screen = move |x: f64, y: f64| camera.doc_to_screen(Point::new(x, y));
        editor.pointer_down(screen(200.0, 300.0), Mods::default());
        editor.pointer_move(screen(300.0, 300.0), Mods::default());
        editor.pointer_up(screen(400.0, 300.0));

        let scene = editor.doc.scene();
        let id = objects(&editor)[0].0;
        let object = scene.find_object(id).unwrap().1;
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("not a shape");
        };
        let Some(Paint::Image(fill)) = shape.fill.as_ref().map(|f| &f.paint) else {
            panic!("not painted");
        };
        // A column across the stroke, from its middle outwards.
        (0..30)
            .map(|dy| {
                let p = fill
                    .to_pixel(Point::new(300.0, 300.0 + f64::from(dy)))
                    .expect("a pixel");
                fill.asset.pixel(p.x as i64, p.y as i64)[3]
            })
            .collect()
    };

    let soft = paint(0.0);
    let hard = paint(1.0);

    // Two thirds of the way out, a hard brush is still solid and a soft one is
    // well on its way to nothing.
    assert!(
        hard[20] > 240,
        "a hard brush should be solid at 20, is {}",
        hard[20]
    );
    assert!(
        soft[20] < 128,
        "a soft brush should be fading at 20, is {}",
        soft[20]
    );
    // Both stop by the edge of the brush.
    assert!(hard[29] < 250 && soft[29] < 60);
}

/// Painting is one undo step, and undoing it takes the picture with it.
#[test]
fn a_painted_stroke_undoes_in_one_step() {
    let mut scene = Scene::default();
    scene.add_layer("Paint", LayerKind::Normal);
    let mut editor = Editor::new(Document::new(scene));
    editor.set_tool(ToolId::Brush);
    editor.style.brush.kind = buzz_ui::BrushKind::Raster;

    let camera = editor.camera;
    let screen = move |x: f64, y: f64| camera.doc_to_screen(Point::new(x, y));
    editor.pointer_down(screen(100.0, 100.0), Mods::default());
    for i in 1..=10 {
        editor.pointer_move(screen(100.0 + f64::from(i) * 10.0, 100.0), Mods::default());
    }
    editor.pointer_up(screen(200.0, 100.0));
    assert_eq!(objects(&editor).len(), 1);

    editor.run(Command::Undo);
    assert!(
        objects(&editor).is_empty(),
        "one Ctrl+Z should undo the whole stroke, not one pointer move of it"
    );
}

/// **The GPU is not asked to re-upload a picture that has not changed — and
/// is never handed two different pictures under one name.**
///
/// The renderer caches bitmaps by an identifier, and the obvious way of
/// building one hands out a fresh number every frame, which re-uploads a
/// four-megapixel photograph sixty times a second for as long as it is on
/// screen. The second obvious way — the bitmap's library id and a change
/// counter — is worse, and worse in a way that does not show up until it does:
/// two assets can carry the same library id and hold *different pixels*, and
/// the renderer's atlas keeps whichever it saw first and serves it to both.
/// That was caught by these tests failing intermittently, and only under load.
#[test]
fn a_bitmaps_identity_follows_its_pixels_and_nothing_else() {
    let asset = disc_picture(64, 20.0);
    let first = asset.blob_id();
    assert_eq!(first, asset.blob_id(), "asking twice gave two answers");

    // A copy is the same pixels, so it keeps the identity: this is what stops
    // the re-upload every frame, since the scene is copied on every edit.
    let copied = (*asset).clone();
    assert_eq!(copied.blob_id(), first, "a copy is the same picture");

    // **Two separately built pictures never share an identity**, however alike
    // they are and whatever library id they carry.
    let twin = disc_picture(64, 20.0);
    assert_ne!(
        twin.blob_id(),
        first,
        "two bitmaps built separately share an identity — the renderer would          draw the first of them for both"
    );

    let mut painted = (*asset).clone();
    painted.edit_pixels(|p| p[0] = 0);
    assert_ne!(
        painted.blob_id(),
        first,
        "painting on a bitmap left the renderer showing the old pixels"
    );
}

/// **A brush preview is never mistaken for the one before it.**
///
/// The other half of caching by identity: a picture rebuilt on every pointer
/// move must take a new identity each time, or the renderer keeps showing the
/// first frame of the stroke while the user goes on drawing. Nothing special is
/// done for it — identity is issued at construction, so a rebuilt preview is a
/// new picture by definition.
#[test]
fn a_rebuilt_preview_takes_a_fresh_identity_every_time() {
    let brush = buzz_scene::SoftBrush::default();
    let stroke = |to: f64| {
        buzz_scene::Canvas::for_stroke(&[Point::new(0.0, 0.0), Point::new(to, 0.0)], &brush)
            .expect("a stroke")
            .to_asset(ImageId(0), "preview", &brush)
            .blob_id()
    };
    assert_ne!(
        stroke(40.0),
        stroke(60.0),
        "two previews share an identity, so the second would not be drawn"
    );
}

/// **Paint survives saving and reopening.**
///
/// A painted stroke has no source file to write back — its pixels *are* the
/// document — so the container has to encode them. If it did not, a night's
/// painting would open blank.
#[test]
fn a_painted_stroke_survives_a_save_and_a_reopen() {
    let mut scene = Scene::default();
    scene.add_layer("Paint", LayerKind::Normal);
    let mut editor = Editor::new(Document::new(scene));
    editor.set_tool(ToolId::Brush);
    editor.style.brush.kind = buzz_ui::BrushKind::Raster;
    editor.style.brush.size = 30.0;
    editor.style.fill_color = Color::from_rgb8(0xC0, 0x20, 0x20);

    let camera = editor.camera;
    let screen = move |x: f64, y: f64| camera.doc_to_screen(Point::new(x, y));
    editor.pointer_down(screen(100.0, 100.0), Mods::default());
    for i in 1..=8 {
        editor.pointer_move(screen(100.0 + f64::from(i) * 20.0, 100.0), Mods::default());
    }
    editor.pointer_up(screen(260.0, 100.0));

    let before = editor.doc.scene().clone();
    let painted_id = objects(&editor)[0].0;
    let sample = {
        let object = before.find_object(painted_id).unwrap().1;
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("not a shape")
        };
        let Some(Paint::Image(fill)) = shape.fill.as_ref().map(|f| &f.paint) else {
            panic!("not painted")
        };
        let p = fill.to_pixel(Point::new(180.0, 100.0)).unwrap();
        fill.asset.pixel(p.x as i64, p.y as i64)
    };
    assert!(sample[3] > 200, "the sample point should be solid paint");

    let dir = std::env::temp_dir().join(format!("buzz-paint-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("painting.buzz");
    buzz_doc::format::save(&before, &path).expect("save");
    let reopened = buzz_doc::format::load(&path).expect("open");

    let object = reopened
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(0).iter())
        .next()
        .expect("the stroke came back");
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("the stroke stopped being a shape")
    };
    let Some(Paint::Image(fill)) = shape.fill.as_ref().map(|f| &f.paint) else {
        panic!("the paint was lost on save")
    };
    let p = fill.to_pixel(Point::new(180.0, 100.0)).unwrap();
    let after = fill.asset.pixel(p.x as i64, p.y as i64);
    assert_eq!(
        after, sample,
        "the pixels changed across a save and a reopen"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
