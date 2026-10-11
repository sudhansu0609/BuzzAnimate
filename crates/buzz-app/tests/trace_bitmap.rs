//! **Tracing a picture into artwork, through the editor.**
//!
//! The tracer itself is unit-tested in `buzz-scene`; what those tests cannot
//! show is that the result lands **on the stage, in the right place, at the
//! right size**, that the picture it replaces is really gone, and that one undo
//! puts the photograph back. Getting the placement wrong is the failure that
//! would look like the feature not working at all — artwork correct to the
//! pixel, sitting off the side of the stage.

use buzz_app::editor::Editor;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{ObjectId, ObjectKind};
use buzz_ui::Command;

/// A black disc on white paper, as a PNG — a scan of a drawing, in miniature.
fn disc_png(size: u32, radius: f64) -> Vec<u8> {
    let c = size as f64 / 2.0;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2)).sqrt();
            let v = if d < radius { 0u8 } else { 255 };
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&pixels).expect("png body");
    }
    out
}

/// A horizontal black line, `thickness` px, across white paper — a scan of a
/// pencil line, in miniature. Centred on a half-integer row so the requested
/// thickness rasterises symmetrically.
fn line_png(size: u32, thickness: f64) -> Vec<u8> {
    let half = thickness / 2.0;
    // A half-integer row, so an odd thickness rasterises symmetrically
    // instead of losing half a pixel to rounding against a row boundary.
    let mid = size as f64 / 2.0 + 0.5;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        let d = (y as f64 + 0.5 - mid).abs();
        let v = if d < half { 0u8 } else { 255 };
        for _ in 0..size {
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&pixels).expect("png body");
    }
    out
}

/// An editor with one imported picture on the stage, and that object's id.
fn with_picture() -> (Editor, ObjectId) {
    let mut editor = Editor::default();
    let layer = editor
        .doc
        .scene()
        .layers()
        .iter()
        .next()
        .expect("a layer")
        .id;
    let png = disc_png(64, 20.0);

    let mut id = ObjectId(0);
    editor.doc.edit("import", |scene| {
        let asset = scene.add_image("Scan", &png).expect("the picture decodes");
        let fill = buzz_scene::ImageFill::new(asset, Rect::new(100.0, 60.0, 356.0, 316.0));
        let shape = buzz_scene::ShapeData {
            path: Rect::new(100.0, 60.0, 356.0, 316.0).to_path(1e-9),
            fill: Some(buzz_scene::FillSpec::image(fill)),
            stroke: None,
            blend: Default::default(),
        };
        id = scene.add_shape(layer, shape).expect("on the stage");
    });
    (editor, id)
}

fn shapes_on_stage(editor: &Editor) -> Vec<(ObjectId, Rect)> {
    let scene = editor.doc.scene();
    let mut out = Vec::new();
    for layer in scene.layers().iter() {
        for object in layer.frames.resolved_at(0).iter() {
            if matches!(object.kind, ObjectKind::Shape(_)) {
                out.push((object.id, object.bounds()));
            }
        }
    }
    out
}

fn is_picture(editor: &Editor, id: ObjectId) -> bool {
    editor
        .doc
        .scene()
        .find_object(id)
        .and_then(|(_, o)| match &o.kind {
            ObjectKind::Shape(s) => s.fill.as_ref().map(|f| &f.paint),
            _ => None,
        })
        .is_some_and(|p| matches!(p, buzz_scene::Paint::Image(_)))
}

/// **The picture becomes artwork, and the picture is gone.** Leaving the
/// photograph underneath is the failure that makes every later selection and
/// bucket fill have to be aimed past it.
#[test]
fn tracing_replaces_the_picture_with_shapes() {
    let (mut editor, id) = with_picture();
    assert!(is_picture(&editor, id));

    editor.selection.select_one(id);
    editor.run(Command::TraceLineArt);

    assert!(
        editor.doc.scene().find_object(id).is_none(),
        "the bitmap is still on the stage: {:?}",
        editor.status
    );
    let shapes = shapes_on_stage(&editor);
    assert!(
        !shapes.is_empty(),
        "nothing was traced: {:?}",
        editor.status
    );
    for (new, _) in &shapes {
        assert!(
            !is_picture(&editor, *new),
            "a traced shape is still a bitmap"
        );
    }
}

/// **The artwork lands where the picture was.** The tracer works in the
/// picture's own pixels and knows nothing about the stage; if that mapping is
/// wrong the result is correct to the pixel and sitting off the side.
#[test]
fn the_artwork_lands_where_the_picture_was() {
    let (mut editor, id) = with_picture();
    editor.selection.select_one(id);
    editor.run(Command::TraceLineArt);

    let shapes = shapes_on_stage(&editor);
    assert_eq!(shapes.len(), 1, "line art should be the ink alone");
    let bb = shapes[0].1;

    // The picture occupied 100..356 by 60..316, and the disc is the middle
    // 5/8ths of it: centred at (228, 188), about 160 across.
    assert!(
        (bb.center().x - 228.0).abs() < 20.0 && (bb.center().y - 188.0).abs() < 20.0,
        "the traced disc is centred at {:?}, not where the picture was",
        bb.center()
    );
    assert!(
        bb.width() > 110.0 && bb.width() < 210.0,
        "the traced disc came out {} across, not about 160",
        bb.width()
    );
}

/// **One undo puts the photograph back.** That is what makes replacing it the
/// honest choice rather than a destructive one.
#[test]
fn one_undo_brings_the_picture_back() {
    let (mut editor, id) = with_picture();
    editor.selection.select_one(id);
    editor.run(Command::TraceLineArt);
    assert!(editor.doc.scene().find_object(id).is_none());

    editor.run(Command::Undo);

    assert!(
        is_picture(&editor, id),
        "the picture did not come back in one step"
    );
    assert_eq!(
        shapes_on_stage(&editor).len(),
        1,
        "the traced artwork is still there"
    );
}

/// **A colour trace keeps the paper**; a line-art trace throws it away. The two
/// commands exist because those are two different jobs, and a trace that always
/// did one of them would be wrong half the time.
#[test]
fn colour_and_line_art_differ_in_what_they_keep() {
    let (mut editor, id) = with_picture();
    editor.selection.select_one(id);
    editor.run(Command::TraceBitmap);
    let colour = shapes_on_stage(&editor).len();

    let (mut editor, id) = with_picture();
    editor.selection.select_one(id);
    editor.run(Command::TraceLineArt);
    let ink = shapes_on_stage(&editor).len();

    assert!(
        colour > ink,
        "a colour trace kept {colour} shapes and line art kept {ink}; \
         line art should have dropped the paper"
    );
}

/// **Selecting something that is not a picture says so** rather than silently
/// doing nothing, which is the version of this that wastes an afternoon.
#[test]
fn tracing_a_drawing_explains_itself() {
    let mut editor = Editor::default();
    let layer = editor
        .doc
        .scene()
        .layers()
        .iter()
        .next()
        .expect("a layer")
        .id;
    let mut id = ObjectId(0);
    editor.doc.edit("draw", |scene| {
        id = scene
            .add_shape(
                layer,
                buzz_scene::ShapeData::filled(
                    Rect::new(0.0, 0.0, 50.0, 50.0).to_path(1e-9),
                    peniko::Color::from_rgb8(0x20, 0x80, 0x40),
                ),
            )
            .expect("a shape");
    });

    editor.selection.select_one(id);
    editor.run(Command::TraceBitmap);

    assert!(
        editor.doc.scene().find_object(id).is_some(),
        "a drawing must not be consumed by a trace"
    );
    let status = editor.status.clone().unwrap_or_default();
    assert!(
        status.contains("picture"),
        "the reason should name what is missing; it said {status:?}"
    );
}

/// **A stroke's width scales with the picture, not just its path.** The
/// centreline already moves with `place`; if the stroke's own width did not
/// follow, tracing a picture placed at twice its natural size would come back
/// with lines half as thick as they look.
#[test]
fn traced_line_art_strokes_scale_with_the_picture_and_undo_restores_it() {
    let mut editor = Editor::default();
    let layer = editor
        .doc
        .scene()
        .layers()
        .iter()
        .next()
        .expect("a layer")
        .id;
    // A 5px line in a 64px picture, placed across a 256-unit square: a 4x
    // scale, the same the disc fixture above uses.
    let png = line_png(64, 5.0);

    let mut id = ObjectId(0);
    editor.doc.edit("import", |scene| {
        let asset = scene.add_image("Line", &png).expect("the picture decodes");
        let fill = buzz_scene::ImageFill::new(asset, Rect::new(100.0, 60.0, 356.0, 316.0));
        let shape = buzz_scene::ShapeData {
            path: Rect::new(100.0, 60.0, 356.0, 316.0).to_path(1e-9),
            fill: Some(buzz_scene::FillSpec::image(fill)),
            stroke: None,
            blend: Default::default(),
        };
        id = scene.add_shape(layer, shape).expect("on the stage");
    });

    editor.selection.select_one(id);
    editor.trace_with(buzz_scene::TraceOptions {
        mode: buzz_scene::TraceMode::LineArt,
        line_style: buzz_scene::LineStyle::Stroke,
        ..buzz_scene::TraceOptions::line_art()
    });

    assert!(
        editor.doc.scene().find_object(id).is_none(),
        "the picture should have been replaced"
    );
    let mut strokes: Vec<(buzz_geom::Rect, f64)> = Vec::new();
    {
        let scene = editor.doc.scene();
        for layer in scene.layers().iter() {
            for object in layer.frames.resolved_at(0).iter() {
                let ObjectKind::Shape(s) = &object.kind else {
                    continue;
                };
                let Some(stroke) = s.stroke.as_ref() else {
                    continue;
                };
                assert!(s.fill.is_none(), "a Stroke-style trace has no fill");
                strokes.push((object.bounds(), stroke.width));
            }
        }
    }
    assert!(!strokes.is_empty(), "nothing was traced: {:?}", editor.status);

    // ~4x the pixel width (5px), within a quarter.
    for (_, width) in &strokes {
        assert!(
            (*width - 20.0).abs() < 5.0,
            "traced width {width}, expected about 20 (4x the 5px line)"
        );
    }
    // In the right place: the picture occupied 100..356 by 60..316.
    let bb = strokes[0].0;
    assert!(
        bb.x0 > 95.0 && bb.x1 < 361.0,
        "the traced line landed at {bb:?}, not where the picture was"
    );

    editor.run(Command::Undo);
    assert!(
        is_picture(&editor, id),
        "one undo should bring the picture back"
    );
}

/// **Trace Bitmap… opens a dialog and touches nothing else.** The direct
/// commands trace straight away; this one is for tuning the settings first.
#[test]
fn trace_bitmap_options_opens_the_dialog_without_tracing() {
    let (mut editor, id) = with_picture();
    editor.selection.select_one(id);
    let revision_before = editor.doc.scene().revision();

    editor.run(Command::TraceBitmapOptions);

    assert!(
        editor.trace_dialog.is_some(),
        "the dialog should have opened"
    );
    assert_eq!(
        editor.doc.scene().revision(),
        revision_before,
        "opening the dialog must not touch the document"
    );
    assert!(is_picture(&editor, id), "the picture is still there");
}
