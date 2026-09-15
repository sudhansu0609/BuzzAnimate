//! Writing Animate's compact **Edge** path format.
//!
//! The exact inverse of `buzz_import_xfl::edge`, which documents the format;
//! this writes what that reads. Getting it wrong is the difference between a
//! `.fla` Animate opens and one it opens empty, so the two are held together
//! by a round trip rather than by hope — see the tests at the foot of this
//! file and `super::tests`.
//!
//! # The one real conversion
//!
//! Our paths are **cubic** Béziers; Animate's edge format has no cubic. It has
//! `[cx cy x y`, a single quadratic. So every cubic is split into quadratics
//! that stay within a tolerance of it, which kurbo does properly — subdividing
//! where the curvature demands rather than at a fixed count.
//!
//! The tolerance is in **twips**, the unit the format stores, and is a
//! twentieth of a pixel: finer than Animate's own coordinate resolution, so
//! the approximation cannot be the thing that loses precision.

use buzz_geom::{BezPath, Point};
use kurbo::PathEl;

/// Twips per pixel. Flash's unit, inherited by Animate.
const TWIPS_PER_PIXEL: f64 = 20.0;

/// How far a quadratic may stray from the cubic it stands in for, in pixels.
///
/// A twentieth of a pixel is one twip — the smallest difference the file can
/// record — so nothing is lost that the format could have kept.
const CURVE_TOLERANCE: f64 = 1.0 / TWIPS_PER_PIXEL;

/// One coordinate, in twips, as Animate writes them.
///
/// Whole twips print as integers because that is what Animate's own files are
/// full of and it keeps them diffable; anything else keeps enough decimals to
/// come back unchanged.
fn twips(value: f64) -> String {
    let t = value * TWIPS_PER_PIXEL;
    if (t - t.round()).abs() < 1e-9 {
        format!("{}", t.round() as i64)
    } else {
        let text = format!("{t:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn point(p: Point) -> String {
    format!("{} {}", twips(p.x), twips(p.y))
}

/// Turn a path into an Animate edge string.
///
/// Returns `None` for a path with nothing in it: an `<Edge>` with no geometry
/// is not something to write, and Animate treats an empty `edges` attribute as
/// a malformed shape rather than an empty one.
pub fn write_edges(path: &BezPath) -> Option<String> {
    let mut out = String::new();
    // Where the current subpath began, so `ClosePath` can be written as the
    // line back that Animate's format actually stores — it has no close
    // command of its own.
    let mut start: Option<Point> = None;
    let mut here = Point::ZERO;

    for element in path.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                out.push('!');
                out.push_str(&point(p));
                start = Some(p);
                here = p;
            }
            PathEl::LineTo(p) => {
                out.push('|');
                out.push_str(&point(p));
                here = p;
            }
            // `[` takes **four** numbers — control point then anchor — with no
            // separator between the pairs. A `|` in there is a different
            // command, and the reader stops on it.
            PathEl::QuadTo(c, p) => {
                out.push('[');
                out.push_str(&point(c));
                out.push(' ');
                out.push_str(&point(p));
                here = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                let cubic = kurbo::CubicBez::new(here, c1, c2, p);
                for (_, _, quad) in cubic.to_quads(CURVE_TOLERANCE) {
                    out.push('[');
                    out.push_str(&point(quad.p1));
                    out.push(' ');
                    out.push_str(&point(quad.p2));
                }
                here = p;
            }
            PathEl::ClosePath => {
                // The format has no close: a closed outline is one whose last
                // point is its first. Written only when it is not already
                // there, so a path that closes itself is not given a
                // zero-length edge Animate would have to discard.
                if let Some(first) = start
                    && (first - here).hypot() > 1e-9
                {
                    out.push('|');
                    out.push_str(&point(first));
                    here = first;
                }
            }
        }
    }

    (!out.is_empty()).then_some(out)
}

/// Does this path enclose anything?
///
/// A fill needs a closed outline; Animate draws an unclosed one as nothing at
/// all rather than closing it for you, so a shape whose path never returns to
/// its start is written as a stroke even when it carries a fill.
pub fn is_closed(path: &BezPath) -> bool {
    path.elements()
        .iter()
        .any(|el| matches!(el, PathEl::ClosePath))
        || {
            let points: Vec<Point> = path.elements().iter().filter_map(end_point).collect();
            match (points.first(), points.last()) {
                (Some(a), Some(b)) => points.len() > 2 && (*a - *b).hypot() < 1e-6,
                _ => false,
            }
        }
}

fn end_point(el: &PathEl) -> Option<Point> {
    match *el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
            Some(p)
        }
        PathEl::ClosePath => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests build paths from shapes; the writer takes them as given.
    use kurbo::Shape as _;

    /// **What we write, their parser reads.** The two live in different crates
    /// and must agree exactly; this is the join.
    fn round_trip(path: &BezPath) -> BezPath {
        let edges = write_edges(path).expect("something to write");
        buzz_import_xfl::parse_edges(&edges).expect("our own reader should read it")
    }

    fn close(a: &BezPath, b: &BezPath, tolerance: f64) {
        let sample = |p: &BezPath| -> Vec<Point> {
            let mut out = Vec::new();
            kurbo::flatten(p.iter(), 0.01, |el| {
                if let Some(point) = end_point(&el) {
                    out.push(point);
                }
            });
            out
        };
        let (left, right) = (sample(a), sample(b));
        assert!(
            !left.is_empty() && !right.is_empty(),
            "both paths should have geometry"
        );
        // Compared by extent rather than point for point: a cubic written as
        // quadratics is flattened into a different number of segments, and it
        // is the shape that has to survive, not the segment list.
        let extent = |points: &[Point]| {
            points.iter().fold(
                buzz_geom::Rect::from_points(points[0], points[0]),
                |r, p| r.union_pt(*p),
            )
        };
        let (l, r) = (extent(&left), extent(&right));
        for (a, b) in [
            (l.x0, r.x0),
            (l.y0, r.y0),
            (l.x1, r.x1),
            (l.y1, r.y1),
        ] {
            assert!(
                (a - b).abs() < tolerance,
                "the shape moved: {l:?} became {r:?}"
            );
        }
    }

    #[test]
    fn a_rectangle_survives_the_round_trip() {
        let rect = buzz_geom::Rect::new(10.0, 20.0, 110.0, 70.0).to_path(1e-9);
        close(&rect, &round_trip(&rect), 1e-6);
    }

    #[test]
    fn a_circle_of_cubics_survives_as_quadratics() {
        // Every segment here is a cubic, which the format cannot store — this
        // is the conversion that has to hold.
        let circle = kurbo::Circle::new(Point::new(50.0, 40.0), 30.0).to_path(1e-9);
        close(&circle, &round_trip(&circle), 0.1);
    }

    #[test]
    fn coordinates_are_written_in_twips() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(100.0, 50.0));
        let edges = write_edges(&path).expect("edges");
        // 100 pixels is 2000 twips, which is how Animate's own files read.
        assert_eq!(edges, "!0 0|2000 1000", "got {edges}");
    }

    /// A fraction of a pixel still has to come back as itself.
    #[test]
    fn a_fractional_coordinate_keeps_its_value() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(1.0 / 3.0, 0.0));
        let edges = write_edges(&path).expect("edges");
        let back = buzz_import_xfl::parse_edges(&edges).expect("read back");
        let last = back
            .elements()
            .iter()
            .filter_map(end_point)
            .next_back()
            .expect("a point");
        assert!(
            (last.x - 1.0 / 3.0).abs() < 1.0 / TWIPS_PER_PIXEL,
            "got {last:?}"
        );
    }

    /// An open path is not a fill, whatever it carries.
    #[test]
    fn closure_is_reported_honestly() {
        let mut open = BezPath::new();
        open.move_to(Point::new(0.0, 0.0));
        open.line_to(Point::new(10.0, 0.0));
        assert!(!is_closed(&open));

        let closed = buzz_geom::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9);
        assert!(is_closed(&closed));
    }
}
