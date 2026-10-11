//! Warping a path through a four-cornered map — Animate's Distort.
//!
//! # What Distort has to mean
//!
//! Dragging one corner of Free Transform's box with Ctrl held moves *that
//! corner only*; the other three stay. No affine map does that — it would take
//! a perspective-like warp — so the artwork is carried through the **bilinear**
//! map from the box as it was to the box as it is now: every point keeps its
//! place in the box (how far across, how far down) and lands at that place in
//! the new quadrilateral.
//!
//! A bilinear map bends straight lines, and a Bézier segment is only *close* to
//! the map's image of itself when it is short. So a path is first cut into small
//! pieces ([`subdivide`]) and every point of every piece is mapped: the
//! result is a cubic path that follows the warped outline to within a hair.

use kurbo::{BezPath, CubicBez, ParamCurve, PathEl, Point, QuadBez, Vec2};

/// Cut every segment of `path` into `pieces` cubic Béziers that together trace
/// exactly what the segment did.
///
/// Lines and quadratics come out as the cubics they are equal to, so a warp
/// can bend them: a straight side of a rectangle is one cubic whose controls
/// would only be carried to *thirds of the warped line*, which is not the
/// warped line; eight pieces of it are close enough that nobody can tell.
///
/// The closing line a `ClosePath` draws without saying so is cut like any
/// other. A `pieces` of zero is taken as one.
pub fn subdivide(path: &BezPath, pieces: usize) -> BezPath {
    let pieces = pieces.max(1);
    let mut out = BezPath::new();
    let mut start = Point::ORIGIN;
    let mut current = Point::ORIGIN;

    let cut = |out: &mut BezPath, cubic: CubicBez| {
        for i in 0..pieces {
            let (a, b) = (i as f64 / pieces as f64, (i + 1) as f64 / pieces as f64);
            let part = cubic.subsegment(a..b);
            out.curve_to(part.p1, part.p2, part.p3);
        }
    };
    let line = |from: Point, to: Point| {
        CubicBez::new(
            from,
            from + (to - from) / 3.0,
            from + (to - from) * (2.0 / 3.0),
            to,
        )
    };

    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.move_to(p);
                start = p;
                current = p;
            }
            PathEl::LineTo(p) => {
                cut(&mut out, line(current, p));
                current = p;
            }
            PathEl::QuadTo(c, p) => {
                cut(&mut out, QuadBez::new(current, c, p).raise());
                current = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                cut(&mut out, CubicBez::new(current, c1, c2, p));
                current = p;
            }
            PathEl::ClosePath => {
                if (current - start).hypot() > 1e-9 {
                    cut(&mut out, line(current, start));
                }
                out.close_path();
                current = start;
            }
        }
    }
    out
}

/// Every point of `path` — anchors and controls alike — through `map`.
pub fn map_points(path: &BezPath, map: impl Fn(Point) -> Point) -> BezPath {
    BezPath::from_vec(
        path.elements()
            .iter()
            .map(|el| match *el {
                PathEl::MoveTo(p) => PathEl::MoveTo(map(p)),
                PathEl::LineTo(p) => PathEl::LineTo(map(p)),
                PathEl::QuadTo(c, p) => PathEl::QuadTo(map(c), map(p)),
                PathEl::CurveTo(c1, c2, p) => PathEl::CurveTo(map(c1), map(c2), map(p)),
                PathEl::ClosePath => PathEl::ClosePath,
            })
            .collect(),
    )
}

/// The bilinear map from one quadrilateral to another. See [`bilinear_warp`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BilinearWarp {
    from: [Point; 4],
    to: [Point; 4],
}

/// The map that carries the quadrilateral `from` onto `to`, corner for
/// corresponding corner — taken in order round the shape, say top-left,
/// top-right, bottom-right, bottom-left.
///
/// A point's place in `from` is found by inverting the bilinear map there, and
/// that place is evaluated in `to`. So the corners land on `to`'s **exactly**
/// and `from` onto itself is the identity. The map is exact for points outside
/// `from` too, which matters because a path's control points can lie outside
/// the box round its artwork.
///
/// `None` if `from` has no area, or any corner is not finite: there is no
/// "place in the box" to keep.
pub fn bilinear_warp(from: [Point; 4], to: [Point; 4]) -> Option<BilinearWarp> {
    let finite = |p: &Point| p.x.is_finite() && p.y.is_finite();
    if !from.iter().chain(to.iter()).all(finite) {
        return None;
    }
    let [a, b, _, d] = from;
    if (b - a).cross(d - a).abs() <= f64::EPSILON * (b - a).hypot2().max((d - a).hypot2()) {
        return None;
    }
    Some(BilinearWarp { from, to })
}

impl BilinearWarp {
    /// Where `p` lands.
    pub fn map(&self, p: Point) -> Point {
        let (u, v) = self.place_of(p);
        let [a, b, c, d] = self.to;
        let top = a + (b - a) * u;
        let bottom = d + (c - d) * u;
        top + (bottom - top) * v
    }

    /// How far across (`u`) and down (`v`) `p` is in `from`.
    ///
    /// The inverse of the bilinear map `a + u·e + v·f + u·v·g`, which is a
    /// quadratic in `v`. For the usual box — a parallelogram, `g` zero — it is
    /// linear and the quadratic term vanishes.
    fn place_of(&self, p: Point) -> (f64, f64) {
        let [a, b, c, d] = self.from;
        let e = b - a;
        let f = d - a;
        let g = (a - b) + (c - d);
        let h = p - a;

        let k2 = g.cross(f);
        let k1 = e.cross(f) + h.cross(g);
        let k0 = h.cross(e);
        let scale = e.cross(f).abs().max(f64::MIN_POSITIVE);

        let v = if k2.abs() <= 1e-9 * scale {
            -k0 / k1
        } else {
            let disc = (k1 * k1 - 4.0 * k0 * k2).max(0.0).sqrt();
            let (v1, v2) = ((-k1 - disc) / (2.0 * k2), (-k1 + disc) / (2.0 * k2));
            // The root that lies in the box, or nearest it.
            let outside = |v: f64| (v - v.clamp(0.0, 1.0)).abs();
            if outside(v1) <= outside(v2) { v1 } else { v2 }
        };
        // Solve for `u` along whichever axis the denominator is larger on.
        let den = |axis: fn(Vec2) -> f64| axis(e) + axis(g) * v;
        let (dx, dy) = (den(|w| w.x), den(|w| w.y));
        let u = if dx.abs() >= dy.abs() {
            (h.x - f.x * v) / dx
        } else {
            (h.y - f.y * v) / dy
        };
        (u, v)
    }
}

/// `quad` with the corner at `corner` dragged by `delta` — Distort's target
/// shape. The other three corners stay.
///
/// With `taper` (Shift) the corner on the edge **most parallel to the drag**
/// moves too, by the drag's component along that edge, turned round: so the
/// two ends of that edge slide apart or together by equal amounts, and the box
/// narrows or widens evenly about the middle of it — Animate's taper.
///
/// `corner` counts round the quad as its points are given, and wraps.
pub fn drag_corner(quad: [Point; 4], corner: usize, delta: Vec2, taper: bool) -> [Point; 4] {
    let i = corner % 4;
    let mut out = quad;
    out[i] = quad[i] + delta;
    if taper {
        let along = |j: usize| {
            let e = quad[j] - quad[i];
            let length = e.hypot();
            if length > 0.0 { e / length } else { Vec2::ZERO }
        };
        let parallelism = |j: usize| delta.dot(along(j)).abs();
        let neighbour = [(i + 3) % 4, (i + 1) % 4]
            .into_iter()
            .max_by(|a, b| parallelism(*a).total_cmp(&parallelism(*b)))
            .unwrap_or((i + 1) % 4);
        let e = along(neighbour);
        out[neighbour] = quad[neighbour] - e * delta.dot(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn dragging_a_corner_moves_that_corner_only() {
        let out = drag_corner(square(), 1, Vec2::new(30.0, -10.0), false);
        assert_eq!(out[1], Point::new(130.0, -10.0));
        assert_eq!([out[0], out[2], out[3]], [square()[0], square()[2], square()[3]]);
    }

    #[test]
    fn a_taper_moves_the_neighbour_along_the_nearer_edge_the_other_way() {
        // The top-right corner dragged right: the top edge is the one more
        // parallel to it, so the top-left corner goes left by as much.
        let out = drag_corner(square(), 1, Vec2::new(20.0, 0.0), true);
        assert_eq!(out[1], Point::new(120.0, 0.0));
        assert_eq!(out[0], Point::new(-20.0, 0.0));
        assert_eq!([out[2], out[3]], [square()[2], square()[3]]);

        // Dragged down instead: now the right edge is the nearer one, and the
        // bottom-right corner goes up by as much.
        let out = drag_corner(square(), 1, Vec2::new(0.0, 15.0), true);
        assert_eq!(out[1], Point::new(100.0, 15.0));
        assert_eq!(out[2], Point::new(100.0, 85.0));
        assert_eq!([out[0], out[3]], [square()[0], square()[3]]);
    }

    #[test]
    fn a_taper_of_nothing_changes_nothing() {
        assert_eq!(drag_corner(square(), 2, Vec2::ZERO, true), square());
    }

    fn square() -> [Point; 4] {
        [
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 100.0),
            Point::new(0.0, 100.0),
        ]
    }

    #[test]
    fn the_corners_land_on_the_new_corners_exactly() {
        let to = [
            Point::new(0.0, 0.0),
            Point::new(130.0, -20.0),
            Point::new(100.0, 100.0),
            Point::new(0.0, 100.0),
        ];
        let warp = bilinear_warp(square(), to).unwrap();
        for (from, to) in square().into_iter().zip(to) {
            assert!((warp.map(from) - to).hypot() < 1e-9, "{from:?}");
        }
    }

    #[test]
    fn an_unmoved_quad_is_the_identity() {
        let warp = bilinear_warp(square(), square()).unwrap();
        for p in [
            Point::new(0.0, 0.0),
            Point::new(37.0, 12.0),
            Point::new(100.0, 55.5),
            // Outside the box, as a control point can be.
            Point::new(-40.0, 170.0),
        ] {
            assert!((warp.map(p) - p).hypot() < 1e-9, "{p:?}");
        }
    }

    #[test]
    fn a_turned_box_is_the_identity_too() {
        let turn = kurbo::Affine::rotate(0.5) * kurbo::Affine::translate((30.0, 4.0));
        let quad = square().map(|p| turn * p);
        let warp = bilinear_warp(quad, quad).unwrap();
        let p = turn * Point::new(61.0, 17.0);
        assert!((warp.map(p) - p).hypot() < 1e-9);
    }

    #[test]
    fn a_parallelogram_onto_a_parallelogram_is_the_affine_map() {
        let to = [
            Point::new(10.0, 10.0),
            Point::new(210.0, 10.0),
            Point::new(210.0, 60.0),
            Point::new(10.0, 60.0),
        ];
        let warp = bilinear_warp(square(), to).unwrap();
        // The middle goes to the middle, a quarter across to a quarter across.
        assert!((warp.map(Point::new(50.0, 50.0)) - Point::new(110.0, 35.0)).hypot() < 1e-9);
        assert!((warp.map(Point::new(25.0, 0.0)) - Point::new(60.0, 10.0)).hypot() < 1e-9);
    }

    #[test]
    fn a_general_quad_round_trips_through_the_inverse() {
        // From a skewed, non-parallelogram quad onto a square: the edges'
        // midpoints must land on the square's.
        let from = [
            Point::new(0.0, 0.0),
            Point::new(120.0, 10.0),
            Point::new(90.0, 100.0),
            Point::new(-10.0, 80.0),
        ];
        let warp = bilinear_warp(from, square()).unwrap();
        let mid_top = from[0].midpoint(from[1]);
        assert!((warp.map(mid_top) - Point::new(50.0, 0.0)).hypot() < 1e-9);
        let mid_left = from[0].midpoint(from[3]);
        assert!((warp.map(mid_left) - Point::new(0.0, 50.0)).hypot() < 1e-9);
    }

    #[test]
    fn a_box_with_no_area_has_no_warp() {
        let flat = [
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(0.0, 0.0),
        ];
        assert!(bilinear_warp(flat, square()).is_none());
        assert!(
            bilinear_warp(
                square(),
                [Point::new(f64::NAN, 0.0), Point::ORIGIN, Point::ORIGIN, Point::ORIGIN]
            )
            .is_none()
        );
    }

    #[test]
    fn subdividing_traces_the_same_outline() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(100.0, 0.0));
        path.quad_to(Point::new(140.0, 50.0), Point::new(100.0, 100.0));
        path.curve_to(
            Point::new(60.0, 130.0),
            Point::new(20.0, 130.0),
            Point::new(0.0, 100.0),
        );
        path.close_path();
        let cut = subdivide(&path, 8);

        // Every segment became eight cubics, the closing line included.
        let curves = cut
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::CurveTo(..)))
            .count();
        assert_eq!(curves, 4 * 8);
        assert!(
            cut.elements()
                .iter()
                .all(|e| !matches!(e, PathEl::LineTo(_) | PathEl::QuadTo(..)))
        );
        assert!(matches!(cut.elements().last(), Some(PathEl::ClosePath)));
        // Same ink: area and extent agree.
        assert!((cut.area() - path.area()).abs() < 1e-6);
        let (a, b) = (path.bounding_box(), cut.bounding_box());
        assert!((a.x0 - b.x0).abs() < 1e-6 && (a.y1 - b.y1).abs() < 1e-6);
    }

    #[test]
    fn a_closed_path_that_ends_on_its_start_gets_no_extra_closing_piece() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(10.0, 0.0));
        path.line_to(Point::new(0.0, 0.0));
        path.close_path();
        assert_eq!(subdivide(&path, 4).elements().len(), 1 + 8 + 1);
    }

    #[test]
    fn warping_a_subdivided_path_moves_its_corners_and_bends_its_sides() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(100.0, 0.0));
        path.line_to(Point::new(100.0, 100.0));
        path.line_to(Point::new(0.0, 100.0));
        path.close_path();
        let mut to = square();
        to[1] = Point::new(160.0, -30.0);
        let warp = bilinear_warp(square(), to).unwrap();
        let warped = map_points(&subdivide(&path, 8), |p| warp.map(p));
        let PathEl::MoveTo(start) = warped.elements()[0] else {
            panic!("no start")
        };
        assert_eq!(start, Point::new(0.0, 0.0));
        // Element 8 ends the first side: the moved corner.
        let PathEl::CurveTo(_, _, corner) = warped.elements()[8] else {
            panic!("not a curve")
        };
        assert!((corner - to[1]).hypot() < 1e-9);
        // The bottom-left corner did not move.
        let PathEl::CurveTo(_, _, last) = warped.elements()[3 * 8] else {
            panic!("not a curve")
        };
        assert!((last - Point::new(0.0, 100.0)).hypot() < 1e-9);
    }

    #[test]
    fn a_degenerate_piece_count_is_one() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(10.0, 0.0));
        assert_eq!(subdivide(&path, 0).elements().len(), 2);
    }
}
