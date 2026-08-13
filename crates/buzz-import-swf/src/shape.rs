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
use buzz_scene::{Gradient, GradientKind, GradientSpread, GradientStop, Paint};
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
    pub fill: Option<Paint>,
    pub stroke: Option<(Paint, f64)>,
    /// True when the fill referenced something this importer cannot express.
    /// Only a bitmap fill now: gradients come across as gradients.
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
        let (paint, approximated) = fill_paint(style);
        let path = stitch(edges, true);
        if !path.is_empty() {
            out.push(StyledPath {
                path,
                fill: Some(paint),
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
                stroke: Some((line_paint(style), style.width().to_pixels())),
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

/// Flash's gradients are declared in a fixed square 32 768 twips across —
/// 1 638.4 pixels, from −819.2 to +819.2 — which the style's matrix maps onto
/// the artwork. Our unit space is −1 to 1, so this is the factor between them.
///
/// The same number the XFL reader uses, and for the same reason: XFL is Animate
/// writing out what it would have compiled into a SWF.
const GRADIENT_HALF_BOX: f64 = 819.2;

/// The paint to draw a fill style in, and whether that is an approximation.
///
/// Gradients now come across as gradients. A **bitmap** fill still cannot be
/// expressed — no reader here imports bitmaps (§7 item 22) — so it becomes a
/// representative flat colour rather than nothing at all: a visible shape in
/// roughly the right colour is far easier to fix by hand than an invisible one.
/// The caller reports the approximation.
fn fill_paint(style: &FillStyle) -> (Paint, bool) {
    match style {
        FillStyle::Color(c) => (Paint::Solid(colour(*c)), false),
        FillStyle::LinearGradient(g) => (gradient(g, GradientKind::Linear, 0.0), false),
        FillStyle::RadialGradient(g) => (gradient(g, GradientKind::Radial, 0.0), false),
        FillStyle::FocalGradient {
            gradient: g,
            focal_point,
        } => (
            gradient(g, GradientKind::Radial, focal_point.to_f64()),
            false,
        ),
        // A representative grey: a bitmap fill has no single colour, and
        // guessing one from the image would need the image decoded.
        FillStyle::Bitmap { .. } => (
            Paint::Solid(peniko::Color::from_rgba8(0x80, 0x80, 0x80, 0xFF)),
            true,
        ),
    }
}

/// One of SWF's gradients, as one of ours.
fn gradient(g: &swf::Gradient, kind: GradientKind, focal: f64) -> Paint {
    let stops = g
        .records
        .iter()
        .map(|r| {
            // SWF's ratio is a byte across the whole ramp, so 255 is the end —
            // not 256. Dividing by the wrong one leaves the last stop a
            // fraction short of the end, which shows as a thin band of the
            // wrong colour at the rim of every radial gradient.
            GradientStop::new(f64::from(r.ratio) / 255.0, colour(r.color))
        })
        .collect();

    let mut out = Gradient::new(kind, stops);
    out.spread = match g.spread {
        swf::GradientSpread::Pad => GradientSpread::Pad,
        swf::GradientSpread::Reflect => GradientSpread::Reflect,
        swf::GradientSpread::Repeat => GradientSpread::Repeat,
    };
    out.focal = focal.clamp(-1.0, 1.0);
    // The style's matrix maps Flash's gradient square; ours maps the unit one.
    out.transform = to_affine(g.matrix) * buzz_geom::Affine::scale(GRADIENT_HALF_BOX);
    Paint::Gradient(std::sync::Arc::new(out))
}

/// SWF's matrix, in twips, as a transform in pixels.
///
/// The same conversion the timeline uses for a placement — kept here rather
/// than shared because a gradient's matrix is read while converting a shape,
/// before any placement exists.
fn to_affine(m: swf::Matrix) -> buzz_geom::Affine {
    buzz_geom::Affine::new([
        m.a.to_f64(),
        m.b.to_f64(),
        m.c.to_f64(),
        m.d.to_f64(),
        m.tx.to_pixels(),
        m.ty.to_pixels(),
    ])
}

/// A stroke's paint. SWF v4+ lets a line be filled with anything a shape can
/// be, gradients included.
fn line_paint(style: &LineStyle) -> Paint {
    fill_paint(style.fill_style()).0
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

        let fill = paths[0].fill.as_ref().expect("it is filled");
        assert_eq!(fill.color().to_rgba8().to_u8_array(), [255, 0, 0, 255]);
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
        let (color, width) = paths[0].stroke.as_ref().expect("it is stroked");
        assert_eq!(*width, 3.0);
        assert_eq!(color.color().to_rgba8().to_u8_array(), [0, 0, 255, 255]);
        assert!(paths[0].fill.is_none());
    }

    fn ramp(spread: swf::GradientSpread, matrix: swf::Matrix) -> swf::Gradient {
        swf::Gradient {
            matrix,
            spread,
            interpolation: swf::GradientInterpolation::Rgb,
            records: vec![
                swf::GradientRecord {
                    ratio: 0,
                    color: swf::Color { r: 0, g: 0, b: 0, a: 255 },
                },
                swf::GradientRecord {
                    ratio: 128,
                    color: swf::Color { r: 255, g: 0, b: 0, a: 255 },
                },
                swf::GradientRecord {
                    ratio: 255,
                    color: swf::Color { r: 255, g: 255, b: 255, a: 255 },
                },
            ],
        }
    }

    fn a_square() -> Vec<ShapeRecord> {
        vec![
            move_to(0.0, 0.0, Some(1)),
            line(10.0, 0.0),
            line(0.0, 10.0),
            line(-10.0, 0.0),
            line(0.0, -10.0),
        ]
    }

    /// A gradient arrives as a gradient, with its stops where the file put
    /// them and its spread mode intact.
    #[test]
    fn a_gradient_fill_arrives_as_a_gradient() {
        let styles = ShapeStyles {
            fill_styles: vec![FillStyle::LinearGradient(ramp(
                swf::GradientSpread::Reflect,
                swf::Matrix::IDENTITY,
            ))],
            line_styles: vec![],
        };

        let paths = convert(&styles, &a_square());
        assert_eq!(paths.len(), 1);
        assert!(
            !paths[0].approximated_fill,
            "a gradient is no longer an approximation"
        );

        let g = paths[0]
            .fill
            .as_ref()
            .unwrap()
            .gradient()
            .expect("it should be a gradient");
        assert_eq!(g.kind, GradientKind::Linear);
        assert_eq!(g.spread, GradientSpread::Reflect);
        assert_eq!(g.stops().len(), 3);

        // **SWF's ratio is a byte across the whole ramp, so 255 is the end.**
        // Dividing by 256 would leave the last stop a fraction short and put a
        // thin band of the wrong colour at the rim of every radial gradient.
        assert!((g.stops()[2].offset - 1.0).abs() < 1e-9, "{:?}", g.stops());
        assert!(
            (g.stops()[1].offset - 128.0 / 255.0).abs() < 1e-9,
            "{:?}",
            g.stops()
        );
        assert_eq!(g.stops()[1].color.to_rgba8().to_u8_array()[..3], [255, 0, 0]);
    }

    /// The gradient's matrix maps Flash's fixed square, 1 638.4 pixels across.
    /// It is the one number that decides whether an imported gradient is the
    /// right size.
    #[test]
    fn a_gradients_matrix_maps_flashs_gradient_box() {
        let mut matrix = swf::Matrix::IDENTITY;
        matrix.a = swf::Fixed16::from_f64(0.05);
        matrix.d = swf::Fixed16::from_f64(0.05);
        matrix.tx = Twips::from_pixels(30.0);
        matrix.ty = Twips::from_pixels(40.0);

        let styles = ShapeStyles {
            fill_styles: vec![FillStyle::RadialGradient(ramp(
                swf::GradientSpread::Pad,
                matrix,
            ))],
            line_styles: vec![],
        };

        let paths = convert(&styles, &a_square());
        let g = paths[0].fill.as_ref().unwrap().gradient().unwrap();
        assert_eq!(g.kind, GradientKind::Radial);

        let h = g.handles();
        assert!((h.center.x - 30.0).abs() < 1e-6, "centre {:?}", h.center);
        assert!((h.center.y - 40.0).abs() < 1e-6, "centre {:?}", h.center);
        // Half the gradient box, scaled by the matrix — read back from the
        // matrix rather than written out, because `Fixed16` cannot hold 0.05
        // exactly and the expected value has to be the one the file really
        // carries.
        let expected = 30.0 + 819.2 * matrix.a.to_f64();
        assert!(
            (h.end.x - expected).abs() < 1e-9,
            "the gradient box is the wrong size: end {:?}, expected {expected}",
            h.end
        );
    }

    /// A focal gradient's hot spot comes across. SWF stores it as a fixed-point
    /// ratio of the radius, which is what our unit space wants.
    #[test]
    fn a_focal_gradient_keeps_its_hot_spot() {
        let styles = ShapeStyles {
            fill_styles: vec![FillStyle::FocalGradient {
                gradient: ramp(swf::GradientSpread::Pad, swf::Matrix::IDENTITY),
                focal_point: Fixed8::from_f64(-0.75),
            }],
            line_styles: vec![],
        };

        let paths = convert(&styles, &a_square());
        let g = paths[0].fill.as_ref().unwrap().gradient().unwrap();
        assert_eq!(g.kind, GradientKind::Radial);
        assert!((g.focal + 0.75).abs() < 1e-6, "focal was {}", g.focal);
    }

    /// A **bitmap** fill is still an approximation, and still says so — no
    /// reader here imports bitmaps (PROGRESS.md §7 item 22).
    #[test]
    fn a_bitmap_fill_is_still_a_flat_colour_and_is_flagged() {
        let styles = ShapeStyles {
            fill_styles: vec![FillStyle::Bitmap {
                id: 7,
                matrix: swf::Matrix::IDENTITY,
                is_smoothed: false,
                is_repeating: false,
            }],
            line_styles: vec![],
        };

        let paths = convert(&styles, &a_square());
        assert_eq!(paths.len(), 1);
        assert!(paths[0].approximated_fill, "the loss must be reported");
        assert!(!paths[0].fill.as_ref().unwrap().is_gradient());
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
