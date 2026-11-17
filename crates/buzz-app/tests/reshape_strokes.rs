//! **Smooth, Straighten and Curvier on a drawn stroke**, as an animator uses
//! them: draw with the fluid brush, select the stroke with the Selection
//! tool, press a button — and press it again to go further.
//!
//! A fluid brush stroke is a filled outline that keeps the spine it was drawn
//! from, so it is reshaped down its middle and stays a brush stroke that can
//! be reshaped again. A pencil line is a path with a stroke width, and is
//! bent as a path.

use std::sync::Arc;

use buzz_app::editor::Editor;
use buzz_geom::{BezPath, BrushProfile, BrushSpine, Point, Shape as _, StrokeSample};
use buzz_scene::{ObjectId, ObjectKind, ShapeData};
use buzz_ui::Command;
use peniko::Color;

const INK: Color = Color::from_rgb8(0x10, 0x10, 0x10);

/// A 400-wide arc sagging 60, drawn with a shaky hand.
fn wobbly_arc() -> Vec<StrokeSample> {
    use std::f64::consts::{PI, TAU};
    (0..=600)
        .map(|i| {
            let t = f64::from(i) / 600.0;
            let x = 100.0 + 400.0 * t;
            let wobble = 4.0 * (x / 35.0 * TAU).sin() + 1.5 * (x / 9.0 * TAU).sin();
            StrokeSample {
                point: Point::new(x, 300.0 - 60.0 * (PI * t).sin() + wobble),
                pressure: 1.0,
                time: t * 0.6,
            }
        })
        .collect()
}

/// Lay a fluid brush stroke down the way the brush does: its outline, with
/// its spine kept on it.
fn brush_stroke(e: &mut Editor) -> ObjectId {
    let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
    let spine = BrushSpine::new(
        wobbly_arc(),
        BrushProfile {
            width: 8.0,
            ..Default::default()
        },
    );
    let mut id = ObjectId(0);
    e.doc.edit("Brush", |scene| {
        id = scene
            .add_shape(layer, ShapeData::filled(spine.outline(), INK))
            .expect("a stroke");
        scene.update_object(id, |o| o.brush = Some(Arc::new(spine.clone())));
    });
    id
}

fn spine(e: &Editor, id: ObjectId) -> BrushSpine {
    let (_, object) = e.doc.scene().find_object(id).expect("the stroke");
    object
        .brush_spine()
        .expect("still a brush stroke, spine and all")
        .clone()
}

/// How far the stroke's middle bows above the line between its ends.
fn sag(spine: &BrushSpine) -> f64 {
    let ends = spine.samples[0]
        .point
        .y
        .min(spine.samples.last().unwrap().point.y);
    ends - spine
        .samples
        .iter()
        .map(|s| s.point.y)
        .fold(f64::MAX, f64::min)
}

/// How far, on average, the stroke strays from the arc it was meant to be.
fn wobble(spine: &BrushSpine) -> f64 {
    let sum: f64 = spine
        .samples
        .iter()
        .map(|s| {
            let t = (s.point.x - 100.0) / 400.0;
            (s.point.y - (300.0 - 60.0 * (std::f64::consts::PI * t).sin())).powi(2)
        })
        .sum();
    (sum / spine.samples.len() as f64).sqrt()
}

/// **Smooth evens a shaky stroke out in one press**, keeps its bend and its
/// spine, is one undo step, and says what it did.
#[test]
fn smooth_evens_out_a_shaky_brush_stroke() {
    let mut e = Editor::default();
    let id = brush_stroke(&mut e);
    let before = spine(&e, id);
    e.selection.select_one(id);

    e.run(Command::SmoothSelection);

    let after = spine(&e, id);
    assert!(
        wobble(&after) < wobble(&before) * 0.25,
        "{} from {}",
        wobble(&after),
        wobble(&before)
    );
    assert!(
        (sag(&after) - 60.0).abs() < 3.0,
        "the bend stays: {}",
        sag(&after)
    );
    assert_eq!(e.status.as_deref(), Some("Smooth: one stroke"));

    e.run(Command::Undo);
    assert_eq!(
        spine(&e, id).samples,
        before.samples,
        "one press is one undo step"
    );
}

/// **The Strength under the buttons is how far one press goes.**
#[test]
fn strength_sets_how_far_one_press_goes() {
    let pressed_at = |strength: f32| {
        let mut e = Editor::default();
        e.workspace.reshape_strength = strength;
        let id = brush_stroke(&mut e);
        e.selection.select_one(id);
        e.run(Command::SmoothSelection);
        wobble(&spine(&e, id))
    };
    assert!(
        pressed_at(0.0) > pressed_at(0.5) * 2.0,
        "{} against {}",
        pressed_at(0.0),
        pressed_at(0.5)
    );

    let bowed_at = |strength: f32| {
        let mut e = Editor::default();
        e.workspace.reshape_strength = strength;
        let id = brush_stroke(&mut e);
        e.selection.select_one(id);
        e.run(Command::CurveSelection);
        sag(&spine(&e, id))
    };
    assert!(
        bowed_at(1.0) > bowed_at(0.0) + 20.0,
        "{} against {}",
        bowed_at(1.0),
        bowed_at(0.0)
    );
}

/// **Curvier bows the stroke further, again and again**, its ends where
/// they were drawn.
#[test]
fn curvier_bows_a_brush_stroke_further_each_press() {
    let mut e = Editor::default();
    let id = brush_stroke(&mut e);
    e.selection.select_one(id);
    let ends = |s: &BrushSpine| (s.samples[0].point, s.samples.last().unwrap().point);
    let start = spine(&e, id);

    e.run(Command::CurveSelection);
    let once = spine(&e, id);
    e.run(Command::CurveSelection);
    let twice = spine(&e, id);
    assert!(
        sag(&once) > sag(&start) * 1.25,
        "{} from {}",
        sag(&once),
        sag(&start)
    );
    assert!(
        sag(&twice) > sag(&once) * 1.25,
        "pressing again goes further: {} from {}",
        sag(&twice),
        sag(&once)
    );
    assert_eq!(ends(&twice), ends(&start));
    assert_eq!(e.status.as_deref(), Some("Curvier: one stroke"));
}

/// **Straighten turns a shaky stroke that was meant to be straight into a
/// straight one**, still a brush stroke.
#[test]
fn straighten_pulls_a_shaky_stroke_into_a_line() {
    let mut e = Editor::default();
    let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
    let shaky: Vec<StrokeSample> = (0..=600)
        .map(|i| {
            let t = f64::from(i) / 600.0;
            let x = 100.0 + 400.0 * t;
            StrokeSample {
                point: Point::new(x, 300.0 + 2.0 * (x / 30.0 * std::f64::consts::TAU).sin()),
                pressure: 1.0,
                time: t * 0.6,
            }
        })
        .collect();
    let drawn = BrushSpine::new(
        shaky,
        BrushProfile {
            width: 8.0,
            ..Default::default()
        },
    );
    let mut id = ObjectId(0);
    e.doc.edit("Brush", |scene| {
        id = scene
            .add_shape(layer, ShapeData::filled(drawn.outline(), INK))
            .expect("a stroke");
        scene.update_object(id, |o| o.brush = Some(Arc::new(drawn.clone())));
    });
    e.selection.select_one(id);
    let shake = |e: &Editor| {
        let straight = spine(e, id);
        let (a, b) = (
            straight.samples[0].point,
            straight.samples.last().unwrap().point,
        );
        straight
            .samples
            .iter()
            .map(|s| ((b - a).cross(s.point - a) / (b - a).hypot()).abs())
            .fold(0.0, f64::max)
    };

    // **Every press goes further**, as in Animate — not done in one and
    // then nothing.
    let mut last = shake(&e);
    for press in 1..=3 {
        e.run(Command::StraightenSelection);
        let now = shake(&e);
        assert!(now < last * 0.8, "press {press}: {now} from {last}");
        last = now;
    }
    assert!(last < 0.5, "{last} off the line after three presses");
}

/// **Smooth keeps working press after press** on a brush stroke: each one
/// changes it, and the bend is still there after three.
#[test]
fn every_press_of_smooth_does_something() {
    let mut e = Editor::default();
    let id = brush_stroke(&mut e);
    e.selection.select_one(id);
    let mut last = spine(&e, id);
    for press in 1..=3 {
        e.run(Command::SmoothSelection);
        let now = spine(&e, id);
        assert!(
            now.outline() != last.outline(),
            "press {press} changed nothing"
        );
        last = now;
    }
    assert!(sag(&last) > 50.0, "the bend stays: {}", sag(&last));
}

/// **A brush stroke with no spine** — merge paint fused it, or it was drawn
/// before strokes kept one, as most of a real film's are — still smooths,
/// straightens and bends, press after press, and stays one filled stroke.
#[test]
fn a_spineless_brush_stroke_reshapes_press_after_press() {
    for command in [
        Command::SmoothSelection,
        Command::StraightenSelection,
        Command::CurveSelection,
    ] {
        let mut e = Editor::default();
        let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
        let drawn = BrushSpine::new(
            wobbly_arc(),
            BrushProfile {
                width: 10.0,
                ..Default::default()
            },
        );
        let mut id = ObjectId(0);
        e.doc.edit("Brush", |scene| {
            id = scene
                .add_shape(layer, ShapeData::filled(drawn.outline(), INK))
                .expect("a stroke");
        });
        e.selection.select_one(id);
        let outline = |e: &Editor| {
            let (_, object) = e.doc.scene().find_object(id).expect("the stroke");
            assert!(object.brush_spine().is_none());
            let ObjectKind::Shape(shape) = &object.kind else {
                panic!("still a shape");
            };
            assert!(
                shape.fill.is_some() && shape.stroke.is_none(),
                "still a fill"
            );
            shape.path.clone()
        };
        let mut last = outline(&e);
        for press in 1..=3 {
            e.run(command);
            let now = outline(&e);
            assert!(now != last, "{command:?} press {press} changed nothing");
            assert!(
                buzz_geom::FillSpine::from_outline(&now).is_some(),
                "{command:?} press {press} left something that is no longer a stroke"
            );
            last = now;
        }
        assert_eq!(
            e.status.as_deref(),
            Some(format!("{}: one stroke", command.label()).as_str())
        );
    }
}

/// **A pencil line bends too** — as a path, ends kept.
#[test]
fn curvier_bends_a_pencil_line() {
    let mut e = Editor::default();
    let layer = e.doc.scene().layers().iter().next().expect("a layer").id;
    let mut line = BezPath::new();
    line.move_to(Point::new(0.0, 0.0));
    line.curve_to(
        Point::new(30.0, -30.0),
        Point::new(70.0, -30.0),
        Point::new(100.0, 0.0),
    );
    let mut id = ObjectId(0);
    e.doc.edit("Pencil", |scene| {
        id = scene
            .add_shape(layer, ShapeData::stroked(line.clone(), INK, 2.0))
            .expect("a line");
    });
    e.selection.select_one(id);

    e.run(Command::CurveSelection);

    let (_, object) = e.doc.scene().find_object(id).expect("the line");
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("still a shape");
    };
    assert!(
        shape.path.bounding_box().y0 < line.bounding_box().y0 - 5.0,
        "{:?} from {:?}",
        shape.path.bounding_box(),
        line.bounding_box()
    );
    assert!(shape.stroke.is_some(), "still a line");
}

/// Nothing selected: the buttons say how to select rather than doing
/// nothing in silence.
#[test]
fn with_nothing_selected_it_says_how_to_select() {
    let mut e = Editor::default();
    e.run(Command::CurveSelection);
    assert!(
        e.status
            .as_deref()
            .unwrap_or_default()
            .contains("Selection tool"),
        "{:?}",
        e.status
    );
}
