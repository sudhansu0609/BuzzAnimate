//! **Paint fuses with paint.**
//!
//! Animate's merge drawing model is that paint of one colour is one thing: draw
//! over what you drew and there is one shape afterwards, not two stacked. Every
//! other drawing tool here honoured that; the soft brush did not, so a face
//! built out of forty strokes was forty objects.
//!
//! What matters most is the rule that makes it safe: fusing collapses the
//! object count and **leaves the picture identical**, because the two are
//! composited exactly as stacking them already showed.

use buzz_app::editor::Editor;
use buzz_geom::Point;
use buzz_scene::{Canvas, LayerId, ObjectKind, Paint, SoftBrush};
use buzz_ui::DrawingMode;
use peniko::Color;

const INK: Color = Color::from_rgb8(0x20, 0x30, 0x40);
const OTHER: Color = Color::from_rgb8(0xC0, 0x30, 0x20);

fn editor() -> (Editor, LayerId) {
    let editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    (editor, layer)
}

fn brush(color: Color) -> SoftBrush {
    SoftBrush {
        radius: 10.0,
        hardness: 0.8,
        flow: 1.0,
        color,
    }
}

/// Paint a straight stroke from `a` to `b`.
fn paint(editor: &mut Editor, a: Point, b: Point, brush: &SoftBrush) {
    let points = [a, b];
    let canvas = Canvas::for_stroke(&points, brush).expect("the stroke covers something");
    editor.apply(buzz_app::tools::ToolAction::PaintRaster {
        canvas,
        brush: *brush,
    });
}

/// Every shape on the layer at frame 0.
fn shapes(editor: &Editor, layer: LayerId) -> Vec<buzz_scene::ObjectId> {
    editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .expect("the layer")
        .frames
        .resolved_at(0u32)
        .iter()
        .map(|o| o.id)
        .collect()
}

fn painted_area(editor: &Editor, id: buzz_scene::ObjectId) -> buzz_geom::Rect {
    let (_, object) = editor.doc.scene().find_object(id).expect("the shape");
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("not a shape")
    };
    match &shape.fill.as_ref().expect("a fill").paint {
        Paint::Image(img) => img.transform.transform_rect_bbox(buzz_geom::Rect::new(
            0.0, 0.0, 1.0, 1.0,
        )),
        other => panic!("expected paint, got {other:?}"),
    }
}

#[test]
fn two_strokes_of_one_colour_become_one_shape() {
    let (mut editor, layer) = editor();
    editor.style.drawing_mode = DrawingMode::MergeShape;
    let b = brush(INK);

    paint(&mut editor, Point::new(40.0, 40.0), Point::new(90.0, 40.0), &b);
    assert_eq!(shapes(&editor, layer).len(), 1, "the first stroke");

    // Straight over the first one.
    paint(&mut editor, Point::new(60.0, 40.0), Point::new(120.0, 40.0), &b);
    assert_eq!(
        shapes(&editor, layer).len(),
        1,
        "the second joins it rather than stacking on it"
    );

    // And the one shape now covers both.
    let area = painted_area(&editor, shapes(&editor, layer)[0]);
    assert!(
        area.x1 > 125.0,
        "the fused bitmap should reach the end of the second stroke, got {area:?}"
    );
}

/// The safety rule: fusing changes how many objects there are, not what is on
/// the screen.
#[test]
fn fusing_leaves_the_picture_as_it_was() {
    let b = brush(INK);
    let (a1, a2) = (Point::new(40.0, 40.0), Point::new(90.0, 40.0));
    let (b1, b2) = (Point::new(60.0, 40.0), Point::new(120.0, 40.0));

    // Fused into one shape.
    let (mut fused, layer) = editor();
    fused.style.drawing_mode = DrawingMode::MergeShape;
    paint(&mut fused, a1, a2, &b);
    paint(&mut fused, b1, b2, &b);
    assert_eq!(shapes(&fused, layer).len(), 1);

    // Left as two, stacked.
    let (mut stacked, layer2) = editor();
    stacked.style.drawing_mode = DrawingMode::ObjectDrawing;
    paint(&mut stacked, a1, a2, &b);
    paint(&mut stacked, b1, b2, &b);
    assert_eq!(shapes(&stacked, layer2).len(), 2);

    // The fused bitmap must show, at every pixel, what the two stacked ones
    // showed: source-over of the second on the first.
    let fused_id = shapes(&fused, layer)[0];
    let (_, object) = fused.doc.scene().find_object(fused_id).expect("the shape");
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("not a shape")
    };
    let Paint::Image(img) = &shape.fill.as_ref().unwrap().paint else {
        panic!("not paint")
    };

    let mut checked = 0;
    let mut worst = 0i32;
    for y in 0..img.asset.height {
        for x in 0..img.asset.width {
            let here = Point::new(
                img.transform.translation().x + f64::from(x) + 0.5,
                img.transform.translation().y + f64::from(y) + 0.5,
            );
            // What the two stacked shapes would have shown at this point.
            let mut expected = 0.0f64;
            for id in shapes(&stacked, layer2) {
                let (_, o) = stacked.doc.scene().find_object(id).expect("a shape");
                let ObjectKind::Shape(s) = &o.kind else { continue };
                let Paint::Image(i) = &s.fill.as_ref().unwrap().paint else {
                    continue;
                };
                let origin = i.transform.translation();
                let (px, py) = (here.x - origin.x, here.y - origin.y);
                if px < 0.0 || py < 0.0 {
                    continue;
                }
                let a = i.asset.pixel(px as i64, py as i64)[3];
                let a = f64::from(a) / 255.0;
                expected = a + expected * (1.0 - a);
            }
            let got = f64::from(img.asset.pixel(i64::from(x), i64::from(y))[3]) / 255.0;
            let diff = ((got - expected) * 255.0).round() as i32;
            worst = worst.max(diff.abs());
            checked += 1;
        }
    }
    assert!(checked > 1000, "the comparison actually looked at pixels");
    assert!(
        worst <= 1,
        "fusing must not change the picture; worst pixel differs by {worst}"
    );
}

#[test]
fn object_drawing_leaves_every_stroke_its_own() {
    let (mut editor, layer) = editor();
    editor.style.drawing_mode = DrawingMode::ObjectDrawing;
    let b = brush(INK);
    paint(&mut editor, Point::new(40.0, 40.0), Point::new(90.0, 40.0), &b);
    paint(&mut editor, Point::new(60.0, 40.0), Point::new(120.0, 40.0), &b);
    assert_eq!(shapes(&editor, layer).len(), 2, "nothing fuses in this mode");
}

#[test]
fn a_different_colour_sits_on_top() {
    let (mut editor, layer) = editor();
    editor.style.drawing_mode = DrawingMode::MergeShape;
    paint(
        &mut editor,
        Point::new(40.0, 40.0),
        Point::new(90.0, 40.0),
        &brush(INK),
    );
    paint(
        &mut editor,
        Point::new(60.0, 40.0),
        Point::new(120.0, 40.0),
        &brush(OTHER),
    );
    assert_eq!(
        shapes(&editor, layer).len(),
        2,
        "another colour is another thing, as it is in Animate"
    );
}

#[test]
fn paint_at_the_other_end_of_the_stage_does_not_fuse() {
    let (mut editor, layer) = editor();
    editor.style.drawing_mode = DrawingMode::MergeShape;
    let b = brush(INK);
    paint(&mut editor, Point::new(40.0, 40.0), Point::new(90.0, 40.0), &b);
    paint(
        &mut editor,
        Point::new(600.0, 400.0),
        Point::new(650.0, 400.0),
        &b,
    );
    assert_eq!(
        shapes(&editor, layer).len(),
        2,
        "one bitmap spanning both would be mostly empty"
    );
}
