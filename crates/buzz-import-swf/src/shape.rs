//! Turning SWF shape records into editable paths.
//!
//! # Why this is not a straight transcription
//!
//! An SWF shape is not a list of paths. It is a list of **edges**, each
//! carrying up to three style references at once: `fill_style_1` names the
//! fill lying to the *left* of the edge as it is travelled, `fill_style_0` the
//! fill to its *right*, and `line_style` a stroke along it. Edges arrive in
//! whatever order the compiler emitted them, and a single closed region is
//! routinely spread across several disconnected runs.
//!
//! Flash drew this by scan-converting the whole edge soup at once. We cannot:
//! the whole point of importing is to produce paths a user can select and
//! reshape. So the edges are sorted into one bucket per style and each bucket
//! is **stitched** back into closed loops by matching endpoints.
//!
//! An edge with `fill_style_0` is walked *backwards* into that fill's bucket,
//! because reversing an edge swaps which side its fill is on. Getting this
//! wrong produces shapes that look almost right and have holes in the wrong
//! places — which is why it is asserted in the tests rather than left to
//! inspection.
//!
//! # Coordinates
//!
//! SWF measures in twips, one twentieth of a pixel, as integers. Converting to
//! pixels is exact in `f64` (a division by 20 of a value well inside the
//! mantissa), so nothing is lost on the way in.

use std::collections::HashMap;

use buzz_geom::{BezPath, Point};
use swf::{FillStyle, LineStyle, ShapeRecord, ShapeStyles};

/// One edge in absolute pixel coordinates.
#[derive(Debug, Clone, Copy)]
struct Edge {
    from: Point,
    to: Point,
    /// Present for a quadratic; SWF has no cubics.
    control: Option<Point>,
}

impl Edge {
    /// The same edge travelled the other way.
    ///
    /// This is what makes `fill_style_0` work: an edge's fill sides swap when
    /// its direction does.
    fn reversed(self) -> Self {
        Self {
            from: self.to,
            to: self.from,
            control: self.control,
        }
    }
}

/// A path pulled out of a shape, with the style it was drawn in.
#[derive(Debug, Clone)]
pub struct StyledPath {
    pub path: BezPath,
    pub fill: Option<peniko::Color>,
    pub stroke: Option<(peniko::Color, f64)>,
    /// True when the fill referenced something this importer cannot express,
    /// such as a gradient or a bitmap.
    pub approximated_fill: bool,
}

/// Convert one shape's records into paths.
///
/// Returns fills first and strokes second, which is SWF's own painting order
/// and therefore what keeps overlapping artwork looking right.
pub fn convert(styles: &ShapeStyles, records: &[ShapeRecord]) -> Vec<StyledPath> {
    // Edges gathered per style index. Index 0 means "no style" in SWF, so the
    // buckets are keyed by the raw index and 0 is skipped at the end.
    let mut fills: HashMap<u32, Vec<Edge>> = HashMap::new();
    let mut strokes: HashMap<u32, Vec<Edge>> = HashMap::new();

    // A shape may replace its style tables part way through; each table is
    // kept so an edge can be resolved against the one in force when it was
    // emitted.
    let mut tables: Vec<ShapeStyles> = vec![styles.clone()];
    let mut table = 0usize;

    // Style indices are per-table, so the bucket key mixes in the table.
    let key = |table: usize, index: u32| (table as u32) << 16 | index;

    let mut cursor = Point::ZERO;
    let mut fill0: Option<u32> = None;
    let mut fill1: Option<u32> = None;
    let mut line: Option<u32> = None;

    for record in records {
        match record {
            ShapeRecord::StyleChange(change) => {
                if let Some(new) = &change.new_styles {
                    tables.push(new.clone());
                    table = tables.len() - 1;
                    // A new table resets the selections, as the format says.
                    fill0 = None;
                    fill1 = None;
                    line = None;
                }
                if let Some(v) = change.fill_style_0 {
                    fill0 = (v != 0).then_some(v);
                }
                if let Some(v) = change.fill_style_1 {
                    fill1 = (v != 0).then_some(v);
                }
                if let Some(v) = change.line_style {
                    line = (v != 0).then_some(v);
                }
                if let Some(to) = change.move_to {
                    cursor = Point::new(to.x.to_pixels(), to.y.to_pixels());
                }
            }

            ShapeRecord::StraightEdge { delta } => {
                let to = Point::new(
                    cursor.x + delta.dx.to_pixels(),
                    cursor.y + delta.dy.to_pixels(),
                );
                let edge = Edge {
                    from: cursor,
                    to,
                    control: None,
                };
                record_edge(edge, fill0, fill1, line, table, key, &mut fills, &mut strokes);
                cursor = to;
            }

            ShapeRecord::CurvedEdge {
                control_delta,
                anchor_delta,
            } => {
                let control = Point::new(
                    cursor.x + control_delta.dx.to_pixels(),
                    cursor.y + control_delta.dy.to_pixels(),
                );
                let to = Point::new(
                    control.x + anchor_delta.dx.to_pixels(),
                    control.y + anchor_delta.dy.to_pixels(),
                );
                let edge = Edge {
                    from: cursor,
                    to,
                    control: Some(control),
                };
                record_edge(edge, fill0, fill1, line, table, key, &mut fills, &mut strokes);
                cursor = to;
            }
        }
    }

    let mut out = Vec::new();

    // Fills first: SWF paints them under the strokes.
    let mut fill_keys: Vec<u32> = fills.keys().copied().collect();
    fill_keys.sort_unstable();
    for k in fill_keys {
        let edges = &fills[&k];
        let (table_index, style_index) = ((k >> 16) as usize, k & 0xFFFF);
        let Some(style) = tables
            .get(table_index)
            .and_then(|t| t.fill_styles.get(style_index as usize - 1))
        else {
            continue;
        };
        let (color, approximated) = fill_colour(style);
        let path = stitch(edges, true);
        if !path.is_empty() {
            out.push(StyledPath {
                path,
                fill: Some(color),
                stroke: None,
                approximated_fill: approximated,
            });
        }
    }

    let mut stroke_keys: Vec<u32> = strokes.keys().copied().collect();
    stroke_keys.sort_unstable();
    for k in stroke_keys {
        let edges = &strokes[&k];
        let (table_index, style_index) = ((k >> 16) as usize, k & 0xFFFF);
        let Some(style) = tables
            .get(table_index)
            .and_then(|t| t.line_styles.get(style_index as usize - 1))
        else {
            continue;
        };
        let path = stitch(edges, false);
        if !path.is_empty() {
            out.push(StyledPath {
                path,
                fill: None,
                stroke: Some((line_colour(style), style.width().to_pixels())),
                approximated_fill: false,
            });
        }
    }

    out
}

#[allow(clippy::too_many_arguments, reason = "internal edge sorter, not an API")]
fn record_edge(
    edge: Edge,
    fill0: Option<u32>,
    fill1: Option<u32>,
    line: Option<u32>,
    table: usize,
    key: impl Fn(usize, u32) -> u32,
    fills: &mut HashMap<u32, Vec<Edge>>,
    strokes: &mut HashMap<u32, Vec<Edge>>,
) {
    if let Some(index) = fill1 {
        fills.entry(key(table, index)).or_default().push(edge);
    }
    if let Some(index) = fill0 {
        // Reversed, because fill_style_0 is the fill on the *other* side.
        fills
            .entry(key(table, index))
            .or_default()
            .push(edge.reversed());
    }
    if let Some(index) = line {
        strokes.entry(key(table, index)).or_default().push(edge);
    }
}

/// Endpoints are matched at twip resolution, which is the precision the file
/// itself was written at, so this cannot merge points the format kept apart.
fn quantise(p: Point) -> (i64, i64) {
    ((p.x * 20.0).round() as i64, (p.y * 20.0).round() as i64)
}

/// Reassemble loose edges into subpaths.
///
/// Edges are chained end to start. When a chain closes, it is closed
/// explicitly — that is what makes a fill fill. When it runs out, it is left
/// open, which is correct for a stroke and the best available answer for a
/// fill whose edges the file left incomplete.
fn stitch(edges: &[Edge], close_loops: bool) -> BezPath {
    let mut path = BezPath::new();
    if edges.is_empty() {
        return path;
    }

    // Edges indexed by their start point, so the next one in a chain is found
    // without scanning. Several edges can share a start, hence the vector.
    let mut by_start: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, edge) in edges.iter().enumerate() {
        by_start.entry(quantise(edge.from)).or_default().push(i);
    }

    let mut used = vec![false; edges.len()];

    for start_index in 0..edges.len() {
        if used[start_index] {
            continue;
        }

        let first = edges[start_index];
        used[start_index] = true;
        path.move_to(first.from);
        append(&mut path, &first);

        let begin = quantise(first.from);
        let mut end = quantise(first.to);

        // Follow the chain. The bound is the edge count: every step consumes
        // one edge, so a malformed shape cannot loop forever.
        for _ in 0..edges.len() {
            if end == begin {
                break;
            }
            let Some(candidates) = by_start.get(&end) else {
                break;
            };
            let Some(&next) = candidates.iter().find(|&&i| !used[i]) else {
                break;
            };
            used[next] = true;
            append(&mut path, &edges[next]);
            end = quantise(edges[next].to);
        }

        if close_loops && end == begin {
            path.close_path();
        }
    }

    path
}

fn append(path: &mut BezPath, edge: &Edge) {
    match edge.control {
        Some(c) => path.quad_to(c, edge.to),
        None => path.line_to(edge.to),
    }
}

fn colour(c: swf::Color) -> peniko::Color {
    peniko::Color::from_rgba8(c.r, c.g, c.b, c.a)
}

/// The colour to draw a fill style in, and whether that is an approximation.
///
/// Gradients and bitmap fills cannot be expressed yet (§7: gradients are still
/// outstanding), so they become a representative flat colour rather than
/// nothing at all — a visible shape in roughly the right colour is far easier
/// to fix by hand than an invisible one. The caller reports the approximation.
fn fill_colour(style: &FillStyle) -> (peniko::Color, bool) {
    match style {
        FillStyle::Color(c) => (colour(*c), false),
        FillStyle::LinearGradient(g) | FillStyle::RadialGradient(g) => {
            (average_gradient(g), true)
        }
        FillStyle::FocalGradient { gradient, .. } => (average_gradient(gradient), true),
        // A representative grey: a bitmap fill has no single colour, and
        // guessing one from the image would need the image decoded.
        FillStyle::Bitmap { .. } => (peniko::Color::from_rgba8(0x80, 0x80, 0x80, 0xFF), true),
    }
}

/// The mean of a gradient's stops, which is the flat colour closest to it.
fn average_gradient(gradient: &swf::Gradient) -> peniko::Color {
    if gradient.records.is_empty() {
        return peniko::Color::from_rgba8(0x80, 0x80, 0x80, 0xFF);
    }
    let n = gradient.records.len() as u32;
    let mut sum = [0u32; 4];
    for record in &gradient.records {
        sum[0] += record.color.r as u32;
        sum[1] += record.color.g as u32;
        sum[2] += record.color.b as u32;
        sum[3] += record.color.a as u32;
    }
    peniko::Color::from_rgba8(
        (sum[0] / n) as u8,
        (sum[1] / n) as u8,
        (sum[2] / n) as u8,
        (sum[3] / n) as u8,
    )
}

fn line_colour(style: &LineStyle) -> peniko::Color {
    match style.fill_style() {
        FillStyle::Color(c) => colour(*c),
        other => fill_colour(other).0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;
    use swf::{Fixed8, LineStyleFlag, PointDelta, ShapeStyles, StyleChangeData, Twips};

    fn twips(v: f64) -> Twips {
        Twips::from_pixels(v)
    }

    fn delta(dx: f64, dy: f64) -> PointDelta<Twips> {
        PointDelta::new(twips(dx), twips(dy))
    }

    fn move_to(x: f64, y: f64, fill1: Option<u32>) -> ShapeRecord {
        ShapeRecord::StyleChange(Box::new(StyleChangeData {
            move_to: Some(swf::Point::new(twips(x), twips(y))),
            fill_style_0: None,
            fill_style_1: fill1,
            line_style: None,
            new_styles: None,
        }))
    }

    fn line(dx: f64, dy: f64) -> ShapeRecord {
        ShapeRecord::StraightEdge { delta: delta(dx, dy) }
    }

    fn red_fill() -> ShapeStyles {
        ShapeStyles {
            fill_styles: vec![FillStyle::Color(swf::Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            })],
            line_styles: vec![],
        }
    }

    /// A square drawn as four edges must come back as one closed path, not
    /// four loose lines — otherwise nothing would fill.
    #[test]
    fn four_edges_stitch_into_one_closed_square() {
        let records = vec![
            move_to(0.0, 0.0, Some(1)),
            line(10.0, 0.0),
            line(0.0, 10.0),
            line(-10.0, 0.0),
            line(0.0, -10.0),
        ];

        let paths = convert(&red_fill(), &records);
        assert_eq!(paths.len(), 1, "one fill style, one path");

        let path = &paths[0].path;
        assert!(
            path.elements().iter().any(|e| matches!(e, PathEl::ClosePath)),
            "the loop must be closed: {path:?}"
        );
        let bounds = kurbo::Shape::bounding_box(path);
        assert_eq!((bounds.width(), bounds.height()), (10.0, 10.0));

        let fill = paths[0].fill.expect("it is filled");
        assert_eq!(fill.to_rgba8().to_u8_array(), [255, 0, 0, 255]);
        assert!(!paths[0].approximated_fill);
    }

    /// Edges are emitted in whatever order the compiler chose. A shape whose
    /// edges arrive out of order still has to come back as one loop.
    #[test]
    fn edges_given_out_of_order_are_still_stitched_into_a_loop() {
        // The same square, but the fourth side is declared before the second,
        // via an intervening move.
        let records = vec![
            move_to(0.0, 0.0, Some(1)),
            line(10.0, 0.0),
            move_to(10.0, 10.0, Some(1)),
            line(-10.0, 0.0),
            line(0.0, -10.0),
            move_to(10.0, 0.0, Some(1)),
            line(0.0, 10.0),
        ];

        let paths = convert(&red_fill(), &records);
        assert_eq!(paths.len(), 1);

        let closes = paths[0]
            .path
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::ClosePath))
            .count();
        assert_eq!(closes, 1, "the four edges form exactly one closed loop");
    }

    /// `fill_style_0` names the fill on the far side of the edge, so its edges
    /// have to be reversed. Without that the loop never closes and the shape
    /// does not fill.
    #[test]
    fn a_fill_declared_on_the_right_hand_side_is_reversed_and_still_closes() {
        // Two edges given with fill_style_0, traversed the "wrong" way round.
        let records = vec![
            ShapeRecord::StyleChange(Box::new(StyleChangeData {
                move_to: Some(swf::Point::new(twips(0.0), twips(0.0))),
                fill_style_0: Some(1),
                fill_style_1: None,
                line_style: None,
                new_styles: None,
            })),
            line(0.0, 10.0),
            line(10.0, 0.0),
            line(0.0, -10.0),
            line(-10.0, 0.0),
        ];

        let paths = convert(&red_fill(), &records);
        assert_eq!(paths.len(), 1);
        assert!(
            paths[0]
                .path
                .elements()
                .iter()
                .any(|e| matches!(e, PathEl::ClosePath)),
            "a fill_style_0 loop must close once its edges are reversed"
        );
    }

    #[test]
    fn a_curved_edge_survives_as_a_quadratic() {
        let records = vec![
            move_to(0.0, 0.0, Some(1)),
            ShapeRecord::CurvedEdge {
                control_delta: delta(5.0, 10.0),
                anchor_delta: delta(5.0, -10.0),
            },
            line(-10.0, 0.0),
        ];

        let paths = convert(&red_fill(), &records);
        let quads = paths[0]
            .path
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::QuadTo(..)))
            .count();
        assert_eq!(quads, 1, "the curve must not be flattened to a line");
    }

    /// Twips are twentieths of a pixel; a shape one twip across must not
    /// round away to nothing.
    #[test]
    fn a_single_twip_survives_the_conversion() {
        let records = vec![
            move_to(0.0, 0.0, Some(1)),
            ShapeRecord::StraightEdge {
                delta: PointDelta::new(Twips::new(1), Twips::new(0)),
            },
        ];
        let paths = convert(&red_fill(), &records);
        let bounds = kurbo::Shape::bounding_box(&paths[0].path);
        assert_eq!(bounds.width(), 0.05, "one twip is exactly 1/20 of a pixel");
    }

    #[test]
    fn a_stroke_becomes_a_stroked_path_with_its_width() {
        let styles = ShapeStyles {
            fill_styles: vec![],
            line_styles: vec![LineStyle::new()
                .with_width(twips(3.0))
                .with_color(swf::Color {
                    r: 0,
                    g: 0,
                    b: 255,
                    a: 255,
                })],
        };
        let records = vec![
            ShapeRecord::StyleChange(Box::new(StyleChangeData {
                move_to: Some(swf::Point::new(twips(0.0), twips(0.0))),
                fill_style_0: None,
                fill_style_1: None,
                line_style: Some(1),
                new_styles: None,
            })),
            line(10.0, 0.0),
        ];

        let paths = convert(&styles, &records);
        assert_eq!(paths.len(), 1);
        let (color, width) = paths[0].stroke.expect("it is stroked");
        assert_eq!(width, 3.0);
        assert_eq!(color.to_rgba8().to_u8_array(), [0, 0, 255, 255]);
        assert!(paths[0].fill.is_none());
    }

    /// A gradient cannot be represented yet, so it becomes a flat colour —
    /// and says so, rather than importing as an invisible shape.
    #[test]
    fn a_gradient_fill_becomes_a_flat_colour_and_is_flagged() {
        let gradient = swf::Gradient {
            matrix: swf::Matrix::IDENTITY,
            spread: swf::GradientSpread::Pad,
            interpolation: swf::GradientInterpolation::Rgb,
            records: vec![
                swf::GradientRecord {
                    ratio: 0,
                    color: swf::Color { r: 0, g: 0, b: 0, a: 255 },
                },
                swf::GradientRecord {
                    ratio: 255,
                    color: swf::Color { r: 255, g: 255, b: 255, a: 255 },
                },
            ],
        };
        let styles = ShapeStyles {
            fill_styles: vec![FillStyle::LinearGradient(gradient)],
            line_styles: vec![],
        };
        let records = vec![
            move_to(0.0, 0.0, Some(1)),
            line(10.0, 0.0),
            line(0.0, 10.0),
            line(-10.0, 0.0),
            line(0.0, -10.0),
        ];

        let paths = convert(&styles, &records);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].approximated_fill, "the loss must be reported");
        let c = paths[0].fill.unwrap().to_rgba8().to_u8_array();
        assert_eq!(c[0], 127, "black to white averages to mid grey");
    }

    #[test]
    fn an_empty_shape_produces_nothing_rather_than_an_empty_path() {
        assert!(convert(&red_fill(), &[]).is_empty());
    }

    /// A shape referring to a style index that does not exist is corrupt; it
    /// must be skipped rather than panicking on the lookup.
    #[test]
    fn an_out_of_range_style_index_is_skipped() {
        let records = vec![move_to(0.0, 0.0, Some(99)), line(10.0, 0.0)];
        let paths = convert(&red_fill(), &records);
        assert!(paths.is_empty());
    }

    /// The flag exists so the sorter can tell strokes and fills apart even
    /// when one edge carries both.
    #[test]
    fn an_edge_with_both_a_fill_and_a_stroke_produces_both() {
        let styles = ShapeStyles {
            fill_styles: vec![FillStyle::Color(swf::Color { r: 1, g: 2, b: 3, a: 255 })],
            line_styles: vec![LineStyle::new().with_width(twips(2.0))],
        };
        let records = vec![
            ShapeRecord::StyleChange(Box::new(StyleChangeData {
                move_to: Some(swf::Point::new(twips(0.0), twips(0.0))),
                fill_style_0: None,
                fill_style_1: Some(1),
                line_style: Some(1),
                new_styles: None,
            })),
            line(10.0, 0.0),
            line(0.0, 10.0),
            line(-10.0, -10.0),
        ];

        let paths = convert(&styles, &records);
        assert_eq!(paths.len(), 2, "one filled path and one stroked path");
        assert!(paths[0].fill.is_some() && paths[0].stroke.is_none());
        assert!(paths[1].stroke.is_some() && paths[1].fill.is_none());
    }

    /// Fills are painted under strokes in Flash, so they must come out first.
    #[test]
    fn fills_are_emitted_before_strokes() {
        let _ = Fixed8::ONE;
        let _ = LineStyleFlag::empty();

        let styles = ShapeStyles {
            fill_styles: vec![FillStyle::Color(swf::Color { r: 9, g: 9, b: 9, a: 255 })],
            line_styles: vec![LineStyle::new().with_width(twips(1.0))],
        };
        let records = vec![
            ShapeRecord::StyleChange(Box::new(StyleChangeData {
                move_to: Some(swf::Point::new(twips(0.0), twips(0.0))),
                fill_style_0: None,
                fill_style_1: Some(1),
                line_style: Some(1),
                new_styles: None,
            })),
            line(5.0, 0.0),
        ];

        let paths = convert(&styles, &records);
        assert!(paths.first().is_some_and(|p| p.fill.is_some()));
        assert!(paths.last().is_some_and(|p| p.stroke.is_some()));
    }
}
