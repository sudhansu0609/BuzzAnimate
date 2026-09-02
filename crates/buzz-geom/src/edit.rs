//! Path-editing operations that map to Animate menu commands.
//!
//! | BuzzAnimate | Adobe Animate |
//! |---|---|
//! | [`outline_stroke`] | Modify ▸ Shape ▸ Convert Lines to Fills |
//! | [`expand_fill`] | Modify ▸ Shape ▸ Expand Fill |
//! | [`smooth`] | Modify ▸ Shape ▸ Smooth (and the Selection tool's Smooth) |
//! | [`straighten`] | Modify ▸ Shape ▸ Straighten |
//!
//! These are **document edits at authoring scale**, like [`crate::boolean`] and
//! unlike [`crate::clip`]. They are never applied to render-space geometry.

use kurbo::{
    BezPath, Cap, Join, Line, ParamCurve, ParamCurveArclen, PathEl, PathSeg, Point, Shape, Stroke,
    StrokeOpts,
};

use crate::boolean::{BoolOp, BooleanOptions, boolean};

/// How the ends and corners of an outlined stroke are shaped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    pub width: f64,
    pub join: Join,
    pub start_cap: Cap,
    pub end_cap: Cap,
    pub miter_limit: f64,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        // Animate's defaults for a new stroke.
        Self {
            width: 1.0,
            join: Join::Round,
            start_cap: Cap::Round,
            end_cap: Cap::Round,
            miter_limit: 3.0,
        }
    }
}

impl StrokeStyle {
    pub fn new(width: f64) -> Self {
        Self {
            width,
            ..Default::default()
        }
    }

    fn to_kurbo(self) -> Stroke {
        Stroke::new(self.width.max(0.0))
            .with_join(self.join)
            .with_start_cap(self.start_cap)
            .with_end_cap(self.end_cap)
            .with_miter_limit(self.miter_limit)
    }
}

/// Convert a stroked path into the filled outline of that stroke.
///
/// Animate: **Convert Lines to Fills**. Once converted the outline can be
/// edited as a shape, which is how strokes get tapered or unevenly styled.
pub fn outline_stroke(path: &BezPath, style: StrokeStyle, tolerance: f64) -> BezPath {
    if path.elements().is_empty() || style.width <= 0.0 {
        return BezPath::new();
    }
    let tolerance = sane_tolerance(tolerance, path);
    kurbo::stroke(
        path.iter(),
        &style.to_kurbo(),
        &StrokeOpts::default(),
        tolerance,
    )
}

/// Grow (positive) or shrink (negative) a filled region by `amount`.
///
/// Animate: **Expand Fill**.
///
/// # Why this is built from stroking rather than curve offsetting
///
/// Offsetting a path directly means handling joins, caps, cusps and the
/// self-intersections that appear when the offset exceeds a local radius of
/// curvature — a well-known source of subtly broken geometry. Stroking the
/// boundary with width `2·|amount|` produces a band extending `|amount|` to
/// each side; unioning that band with the original grows the shape, and
/// subtracting it shrinks the shape. Both steps are already well tested here,
/// so this reuses correctness rather than re-deriving it.
pub fn expand_fill(path: &BezPath, amount: f64, opts: BooleanOptions) -> BezPath {
    if path.elements().is_empty() || !amount.is_finite() || amount == 0.0 {
        return path.clone();
    }

    let band = outline_stroke(
        path,
        StrokeStyle {
            width: 2.0 * amount.abs(),
            join: Join::Round,
            // Butt caps: a cap would bulge past the ends of an open subpath.
            start_cap: Cap::Butt,
            end_cap: Cap::Butt,
            ..Default::default()
        },
        opts.tolerance,
    );

    if band.elements().is_empty() {
        return path.clone();
    }

    let op = if amount > 0.0 {
        BoolOp::Union
    } else {
        BoolOp::Difference
    };
    boolean(path, &band, op, opts)
}

/// Smooth a path, removing small irregularities.
///
/// Animate: **Smooth**. Repeated application smooths further, as it does there.
///
/// `strength` is in document units — roughly the size of detail to remove.
pub fn smooth(path: &BezPath, strength: f64) -> BezPath {
    if path.elements().is_empty() {
        return path.clone();
    }
    let accuracy = sane_tolerance(strength, path);
    // A generous corner threshold: smoothing is explicitly meant to soften
    // joins, so only sharp corners should be preserved. The result is still
    // verified — smoothing may reshape a path, but it must never make it
    // diverge wildly. A looser area allowance than boolean refitting, because
    // changing the shape is the whole point here.
    crate::boolean::refit_checked(path, accuracy, 0.6, 0.10)
}

/// Replace curves that are already nearly straight with actual line segments.
///
/// Animate: **Straighten**. `tolerance` is the greatest deviation from the
/// chord that still counts as straight.
pub fn straighten(path: &BezPath, tolerance: f64) -> BezPath {
    if path.elements().is_empty() {
        return path.clone();
    }
    let tolerance = sane_tolerance(tolerance, path);

    let mut out = BezPath::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => out.move_to(p),
            PathEl::LineTo(p) => out.line_to(p),
            PathEl::ClosePath => out.close_path(),
            PathEl::QuadTo(c, p) => {
                let start = current_point(&out).unwrap_or(c);
                if quad_is_straight(start, c, p, tolerance) {
                    out.line_to(p);
                } else {
                    out.quad_to(c, p);
                }
            }
            PathEl::CurveTo(c1, c2, p) => {
                let start = current_point(&out).unwrap_or(c1);
                if cubic_is_straight(start, c1, c2, p, tolerance) {
                    out.line_to(p);
                } else {
                    out.curve_to(c1, c2, p);
                }
            }
        }
    }
    out
}

/// Greatest distance from the control points to the chord, which bounds how
/// far the curve itself can stray.
fn cubic_is_straight(
    p0: kurbo::Point,
    p1: kurbo::Point,
    p2: kurbo::Point,
    p3: kurbo::Point,
    tolerance: f64,
) -> bool {
    let chord = Line::new(p0, p3);
    distance_to_line(chord, p1).max(distance_to_line(chord, p2)) <= tolerance
}

fn quad_is_straight(p0: kurbo::Point, c: kurbo::Point, p1: kurbo::Point, tolerance: f64) -> bool {
    distance_to_line(Line::new(p0, p1), c) <= tolerance
}

fn distance_to_line(line: Line, p: kurbo::Point) -> f64 {
    let d = line.p1 - line.p0;
    let len = d.hypot();
    if len < f64::EPSILON {
        return (p - line.p0).hypot();
    }
    // Perpendicular distance via the 2D cross product.
    ((p - line.p0).cross(d) / len).abs()
}

/// Last point emitted so far, for operations that rebuild a path element by
/// element.
fn current_point(path: &BezPath) -> Option<kurbo::Point> {
    path.elements().last().and_then(|el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
            Some(*p)
        }
        PathEl::ClosePath => None,
    })
}

/// Guard against a caller passing a zero, negative or non-finite tolerance.
///
/// Falls back to something scaled to the path rather than a fixed constant, so
/// the result is sensible for both a glyph and a background.
fn sane_tolerance(tolerance: f64, path: &BezPath) -> f64 {
    if tolerance.is_finite() && tolerance > 0.0 {
        return tolerance;
    }
    let bb = path.bounding_box();
    let diagonal = bb.width().hypot(bb.height());
    if diagonal.is_finite() && diagonal > 0.0 {
        (diagonal / 10_000.0).clamp(1e-9, 1.0)
    } else {
        0.05
    }
}

/// Total arc length, useful for motion-path work in Phase 4.
pub fn path_length(path: &BezPath, accuracy: f64) -> f64 {
    let accuracy = sane_tolerance(accuracy, path);
    path.segments().map(|s| s.arclen(accuracy)).sum()
}

/// Point at a fraction along the path, for distributing objects on a motion
/// path.
pub fn point_at_fraction(path: &BezPath, fraction: f64, accuracy: f64) -> Option<kurbo::Point> {
    let segments: Vec<PathSeg> = path.segments().collect();
    if segments.is_empty() {
        return None;
    }
    let accuracy = sane_tolerance(accuracy, path);
    let total: f64 = segments.iter().map(|s| s.arclen(accuracy)).sum();
    if total <= 0.0 {
        return Some(segments[0].eval(0.0));
    }

    let target = (fraction.clamp(0.0, 1.0)) * total;
    let mut walked = 0.0;
    for seg in &segments {
        let len = seg.arclen(accuracy);
        if walked + len >= target {
            let t = seg.inv_arclen(target - walked, accuracy);
            return Some(seg.eval(t));
        }
        walked += len;
    }
    Some(segments[segments.len() - 1].eval(1.0))
}

/// Point **and unit tangent** at a fraction along the path, by arc length.
///
/// The position half is exactly [`point_at_fraction`]; the tangent is the
/// direction of travel there, which motion-path work uses to face an object
/// along its route. Returns `None` only for an empty path. At a cusp or a
/// degenerate control net where the derivative vanishes the tangent falls back
/// to `+x` rather than a zero vector, so a caller can always take `atan2`.
pub fn frame_at_fraction(
    path: &BezPath,
    fraction: f64,
    accuracy: f64,
) -> Option<(Point, kurbo::Vec2)> {
    let segments: Vec<PathSeg> = path.segments().collect();
    if segments.is_empty() {
        return None;
    }
    let accuracy = sane_tolerance(accuracy, path);
    let total: f64 = segments.iter().map(|s| s.arclen(accuracy)).sum();
    if total <= 0.0 {
        return Some((segments[0].eval(0.0), kurbo::Vec2::new(1.0, 0.0)));
    }

    let target = fraction.clamp(0.0, 1.0) * total;
    let mut walked = 0.0;
    for seg in &segments {
        let len = seg.arclen(accuracy);
        if walked + len >= target {
            let t = seg.inv_arclen(target - walked, accuracy);
            return Some((seg.eval(t), seg_tangent(seg, t)));
        }
        walked += len;
    }
    let last = &segments[segments.len() - 1];
    Some((last.eval(1.0), seg_tangent(last, 1.0)))
}

/// Unit tangent of a single segment at parameter `t`.
///
/// Computed from the segment's own derivative rather than a finite difference,
/// so it is exact at the ends. A vanishing derivative (a cusp, or coincident
/// control points) has no direction; we return `+x` there so the result is
/// always a usable unit vector.
fn seg_tangent(seg: &PathSeg, t: f64) -> kurbo::Vec2 {
    let derivative = match seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => (q.p1 - q.p0) * (2.0 * (1.0 - t)) + (q.p2 - q.p1) * (2.0 * t),
        PathSeg::Cubic(c) => {
            let u = 1.0 - t;
            (c.p1 - c.p0) * (3.0 * u * u)
                + (c.p2 - c.p1) * (6.0 * u * t)
                + (c.p3 - c.p2) * (3.0 * t * t)
        }
    };
    let length = derivative.hypot();
    if length > 1e-12 {
        derivative / length
    } else {
        kurbo::Vec2::new(1.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Point, Rect};

    fn area(p: &BezPath) -> f64 {
        p.area().abs()
    }

    #[test]
    fn outlining_a_stroke_produces_a_fill_of_the_right_area() {
        // A 100-long horizontal line stroked at width 10 encloses ~1000 plus
        // the two round caps (a full circle of radius 5, ~78.5).
        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(100.0, 0.0));

        let outline = outline_stroke(&line, StrokeStyle::new(10.0), 0.01);
        let expected = 1000.0 + std::f64::consts::PI * 25.0;
        assert!(
            (area(&outline) - expected).abs() < 15.0,
            "outline area {} should be near {expected}",
            area(&outline)
        );
    }

    #[test]
    fn outlining_degenerate_input_yields_nothing() {
        assert!(
            outline_stroke(&BezPath::new(), StrokeStyle::new(4.0), 0.01)
                .elements()
                .is_empty()
        );

        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(10.0, 0.0));
        assert!(
            outline_stroke(&line, StrokeStyle::new(0.0), 0.01)
                .elements()
                .is_empty()
        );
    }

    #[test]
    fn frame_along_a_straight_line_marches_evenly_with_a_fixed_tangent() {
        // A 100-long horizontal line: the point at fraction f sits at x = 100f,
        // and the tangent is +x everywhere.
        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(100.0, 0.0));

        for f in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let (p, tan) = frame_at_fraction(&line, f, 1e-6).unwrap();
            assert!((p.x - 100.0 * f).abs() < 1e-6, "x at {f} was {}", p.x);
            assert!(p.y.abs() < 1e-6, "y at {f} was {}", p.y);
            assert!((tan.x - 1.0).abs() < 1e-9 && tan.y.abs() < 1e-9, "tangent {tan:?}");
        }
    }

    #[test]
    fn frame_on_a_quarter_circle_stays_tangent_to_the_arc() {
        // A quarter arc of a unit circle centred at the origin, from (1,0) up to
        // (0,1). At any point the tangent is perpendicular to the radius, so
        // tangent · position ~= 0, and the endpoints land where they should.
        let arc = Circle::new(Point::ORIGIN, 1.0)
            .to_path(1e-9)
            .segments()
            .next()
            .map(|s| {
                let mut p = BezPath::new();
                p.move_to(s.eval(0.0));
                if let PathSeg::Cubic(c) = s {
                    p.curve_to(c.p1, c.p2, c.p3);
                }
                p
            })
            .unwrap();

        for f in [0.0, 0.3, 0.6, 1.0] {
            let (p, tan) = frame_at_fraction(&arc, f, 1e-7).unwrap();
            let radial = p.to_vec2().normalize();
            assert!(
                tan.dot(radial).abs() < 1e-3,
                "tangent {tan:?} should be perpendicular to radius {radial:?} at {f}"
            );
        }
        // Endpoints sit on the unit circle, whichever way kurbo wound the arc.
        for f in [0.0, 1.0] {
            let (p, _) = frame_at_fraction(&arc, f, 1e-7).unwrap();
            assert!((p.to_vec2().hypot() - 1.0).abs() < 1e-4, "endpoint {p:?} off circle");
        }
    }

    #[test]
    fn frame_on_an_empty_path_is_none() {
        assert!(frame_at_fraction(&BezPath::new(), 0.5, 1e-6).is_none());
    }

    /// Isolates the stroking stage of `expand_fill`, so a bad band is not
    /// mistaken for a bad boolean or a bad refit.
    #[test]
    fn stroking_a_square_boundary_produces_a_sane_band() {
        let sq = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9);
        let band = outline_stroke(
            &sq,
            StrokeStyle {
                width: 10.0,
                join: Join::Round,
                start_cap: Cap::Butt,
                end_cap: Cap::Butt,
                ..Default::default()
            },
            0.015,
        );
        let bb = band.bounding_box();
        assert!(
            (bb.width() - 110.0).abs() < 0.5 && (bb.height() - 110.0).abs() < 0.5,
            "band around a 100x100 square should be 110x110, got {}x{}",
            bb.width(),
            bb.height()
        );
    }

    /// Regression test for a real failure: refitting a correct `-5..105`
    /// polygon produced a path spanning `-5..1071`. The guard in
    /// `refit_checked` must reject a fit that diverges like that, whatever the
    /// fitter does internally.
    #[test]
    fn refitting_never_makes_a_shape_larger_than_it_was() {
        let sq = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9);
        let opts = BooleanOptions::for_shape_size(150.0);

        let plain = expand_fill(&sq, 5.0, opts.fast());
        let refitted = expand_fill(&sq, 5.0, opts);

        let a = plain.bounding_box();
        let b = refitted.bounding_box();
        assert!(
            (a.width() - b.width()).abs() < 1.0 && (a.height() - b.height()).abs() < 1.0,
            "refit changed the bounds from {a:?} to {b:?}"
        );
    }

    #[test]
    fn expanding_a_square_grows_it_by_the_right_amount() {
        let sq = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9);
        let opts = BooleanOptions::for_shape_size(150.0);

        let grown = expand_fill(&sq, 5.0, opts);

        // 110x110 with rounded corners: between the inscribed square and the
        // full square.
        assert!(
            area(&grown) > 100.0 * 100.0,
            "expanding should grow the area, got {}",
            area(&grown)
        );
        assert!(
            area(&grown) > 11_500.0 && area(&grown) < 12_100.0,
            "expected ~12100 minus corner rounding, got {}",
            area(&grown)
        );

        let bb = grown.bounding_box();
        assert!(
            (bb.width() - 110.0).abs() < 0.5,
            "expanded width was {}",
            bb.width()
        );
    }

    #[test]
    fn contracting_a_square_shrinks_it() {
        let sq = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9);
        let opts = BooleanOptions::for_shape_size(150.0);

        let shrunk = expand_fill(&sq, -5.0, opts);

        assert!(
            area(&shrunk) < 100.0 * 100.0,
            "contracting should reduce the area, got {}",
            area(&shrunk)
        );
        assert!(
            (area(&shrunk) - 8100.0).abs() < 400.0,
            "expected ~8100 (90x90), got {}",
            area(&shrunk)
        );
    }

    #[test]
    fn expanding_by_zero_or_nonsense_leaves_the_path_alone() {
        let sq = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9);
        let opts = BooleanOptions::default();
        for amount in [0.0, f64::NAN, f64::INFINITY] {
            let r = expand_fill(&sq, amount, opts);
            assert_eq!(
                r.elements(),
                sq.elements(),
                "amount {amount} should be a no-op"
            );
        }
    }

    #[test]
    fn straighten_replaces_nearly_flat_curves_with_lines() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        // Control points sit almost exactly on the chord.
        path.curve_to(
            Point::new(33.0, 0.01),
            Point::new(66.0, -0.01),
            Point::new(100.0, 0.0),
        );

        let out = straighten(&path, 0.1);
        assert!(
            out.elements()
                .iter()
                .all(|e| !matches!(e, PathEl::CurveTo(..))),
            "a nearly-straight curve should become a line"
        );
    }

    #[test]
    fn straighten_leaves_genuinely_curved_paths_alone() {
        let circle = Circle::new(Point::new(0.0, 0.0), 50.0).to_path(1e-6);
        let out = straighten(&circle, 0.1);
        assert!(
            out.elements()
                .iter()
                .any(|e| matches!(e, PathEl::CurveTo(..))),
            "a circle must not be straightened away"
        );
        assert!(
            (area(&out) - area(&circle)).abs() < 1.0,
            "straightening changed the area of a curved shape"
        );
    }

    #[test]
    fn smoothing_preserves_the_broad_shape() {
        let circle = Circle::new(Point::new(0.0, 0.0), 100.0).to_path(1e-6);
        let smoothed = smooth(&circle, 0.5);
        assert!(
            (area(&smoothed) - area(&circle)).abs() / area(&circle) < 0.05,
            "smoothing changed the area by more than 5%: {} vs {}",
            area(&smoothed),
            area(&circle)
        );
    }

    #[test]
    fn operations_tolerate_degenerate_tolerances() {
        let circle = Circle::new(Point::new(0.0, 0.0), 20.0).to_path(1e-6);
        for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
            assert!(!smooth(&circle, bad).elements().is_empty());
            assert!(!straighten(&circle, bad).elements().is_empty());
            assert!(
                !outline_stroke(&circle, StrokeStyle::new(2.0), bad)
                    .elements()
                    .is_empty()
            );
        }
    }

    #[test]
    fn empty_paths_pass_through_every_operation() {
        let empty = BezPath::new();
        assert!(smooth(&empty, 1.0).elements().is_empty());
        assert!(straighten(&empty, 1.0).elements().is_empty());
        assert!(
            expand_fill(&empty, 5.0, BooleanOptions::default())
                .elements()
                .is_empty()
        );
        assert_eq!(path_length(&empty, 0.01), 0.0);
        assert!(point_at_fraction(&empty, 0.5, 0.01).is_none());
    }

    #[test]
    fn path_length_matches_a_known_perimeter() {
        let sq = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9);
        assert!(
            (path_length(&sq, 1e-6) - 40.0).abs() < 0.01,
            "square perimeter was {}",
            path_length(&sq, 1e-6)
        );

        let circle = Circle::new(Point::new(0.0, 0.0), 10.0).to_path(1e-9);
        let expected = 2.0 * std::f64::consts::PI * 10.0;
        assert!(
            (path_length(&circle, 1e-6) - expected).abs() < 0.05,
            "circumference was {}",
            path_length(&circle, 1e-6)
        );
    }

    #[test]
    fn point_at_fraction_walks_the_path_evenly() {
        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(100.0, 0.0));

        let quarter = point_at_fraction(&line, 0.25, 1e-6).unwrap();
        assert!((quarter.x - 25.0).abs() < 0.01, "got {quarter:?}");

        // Out-of-range fractions clamp rather than panicking.
        assert!((point_at_fraction(&line, -1.0, 1e-6).unwrap().x - 0.0).abs() < 0.01);
        assert!((point_at_fraction(&line, 5.0, 1e-6).unwrap().x - 100.0).abs() < 0.01);
    }
}

/// **Split a path into its separate pieces**, keeping every hole with the
/// piece it belongs to.
///
/// A boolean difference — an eraser stroke through the middle of a shape —
/// returns *one* path holding two disconnected contours. That is one object as
/// far as everything downstream is concerned, so the two halves stay welded
/// together: click either and you select both, drag one and the other follows.
/// Animate splits them, and so should we.
///
/// # Why it is not simply "one subpath, one piece"
///
/// A ring is two contours: its outside and the hole inside it. Splitting by
/// subpath would turn the hole into a solid disc sitting on top of its own
/// ring. So a contour is a *hole* when it lies inside an odd number of others,
/// and it belongs to the smallest contour containing it — which is exactly the
/// even-odd nesting rule a filled path is drawn with, applied to the question
/// of what belongs with what.
pub fn split_disjoint(path: &BezPath) -> Vec<BezPath> {
    let contours = subpaths(path);
    if contours.len() < 2 {
        return if contours.is_empty() {
            Vec::new()
        } else {
            vec![path.clone()]
        };
    }

    // A point actually on each contour, to ask what contains it. The start
    // point is on the boundary of its own contour, which a winding test is
    // ambiguous about, so a point just inside is used instead: the midpoint of
    // the contour's own bounding box is inside it for every shape a brush or
    // an eraser makes, and where it is not the contour simply keeps its own
    // nesting depth of zero and stands alone — which is the safe answer.
    let probes: Vec<Point> = contours
        .iter()
        .map(|c| interior_point(c).unwrap_or_else(|| c.bounding_box().center()))
        .collect();

    // How many *other* contours contain each one.
    let depth: Vec<usize> = probes
        .iter()
        .enumerate()
        .map(|(i, probe)| {
            contours
                .iter()
                .enumerate()
                .filter(|(j, other)| *j != i && other.winding(*probe) != 0)
                .count()
        })
        .collect();

    // Each hole joins the smallest contour that contains it.
    let mut pieces: Vec<BezPath> = Vec::new();
    let mut owner: Vec<Option<usize>> = vec![None; contours.len()];
    let mut outers: Vec<usize> = Vec::new();
    for i in 0..contours.len() {
        if depth[i] % 2 == 0 {
            owner[i] = Some(outers.len());
            outers.push(i);
        }
    }
    for i in 0..contours.len() {
        if depth[i] % 2 == 0 {
            continue;
        }
        let smallest = outers
            .iter()
            .filter(|o| contours[**o].winding(probes[i]) != 0)
            .min_by(|a, b| {
                area_of(&contours[**a])
                    .partial_cmp(&area_of(&contours[**b]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied();
        // A hole with nothing around it cannot be a hole; it stands alone
        // rather than being thrown away.
        owner[i] = match smallest {
            Some(o) => owner[o],
            None => {
                let slot = outers.len();
                outers.push(i);
                Some(slot)
            }
        };
    }

    pieces.resize(outers.len(), BezPath::new());
    for (i, contour) in contours.iter().enumerate() {
        if let Some(slot) = owner[i]
            && let Some(piece) = pieces.get_mut(slot)
        {
            piece.extend(contour.iter());
        }
    }
    pieces.retain(|p| !p.elements().is_empty());
    pieces
}

/// The subpaths of a path, each as a path of its own.
fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for element in path.elements() {
        if matches!(element, PathEl::MoveTo(_)) {
            out.push(BezPath::new());
        }
        if let Some(current) = out.last_mut() {
            current.push(*element);
        }
    }
    out.retain(|c| c.segments().count() > 0);
    out
}

fn area_of(contour: &BezPath) -> f64 {
    contour.area().abs()
}

/// A point inside a closed contour.
///
/// Cast a horizontal ray across the middle of the contour's box and take the
/// midpoint of the first span that is actually inside it. Sampling rather than
/// solving: a handful of probes settles it for the shapes this is used on, and
/// a contour it cannot find a point in falls back to its own centre.
fn interior_point(contour: &BezPath) -> Option<Point> {
    let bounds = contour.bounding_box();
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return None;
    }
    // Several heights, so a crescent whose middle row misses the shape is
    // still found.
    for row in [0.5, 0.35, 0.65, 0.2, 0.8] {
        let y = bounds.y0 + bounds.height() * row;
        let mut previous: Option<f64> = None;
        for step in 0..=64 {
            let x = bounds.x0 + bounds.width() * (step as f64 / 64.0);
            let inside = contour.winding(Point::new(x, y)) != 0;
            match (inside, previous) {
                (true, Some(start)) => return Some(Point::new((start + x) / 2.0, y)),
                (true, None) => previous = Some(x),
                (false, _) => previous = None,
            }
        }
    }
    None
}

#[cfg(test)]
mod split_tests {
    use super::*;
    use crate::boolean::{BoolOp, BooleanOptions, boolean};
    use kurbo::Rect;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(1e-9)
    }

    /// **The eraser's case.** A bar cut through the middle leaves two pieces,
    /// and they must be two shapes — one object holding both means clicking
    /// either selects both and dragging one drags the other.
    #[test]
    fn a_shape_cut_in_two_splits_into_two_pieces() {
        let bar = rect(0.0, 0.0, 100.0, 20.0);
        let cutter = rect(45.0, -10.0, 55.0, 30.0);
        let cut = boolean(&bar, &cutter, BoolOp::Difference, BooleanOptions::default());

        let pieces = split_disjoint(&cut);
        assert_eq!(pieces.len(), 2, "a bar cut through the middle is two bars");
        let mut widths: Vec<f64> = pieces.iter().map(|p| p.bounding_box().width()).collect();
        widths.sort_by(f64::total_cmp);
        for width in widths {
            assert!(
                width > 40.0 && width < 50.0,
                "each piece should be one side of the cut, got {width}"
            );
        }
    }

    /// **A hole stays with its ring.** Splitting by subpath alone would turn
    /// the hole into a solid disc sitting on top of the ring it was cut from.
    #[test]
    fn a_ring_stays_one_piece_with_its_hole() {
        let ring = boolean(
            &rect(0.0, 0.0, 100.0, 100.0),
            &rect(30.0, 30.0, 70.0, 70.0),
            BoolOp::Difference,
            BooleanOptions::default(),
        );
        let pieces = split_disjoint(&ring);
        assert_eq!(pieces.len(), 1, "a ring is one piece: {}", pieces.len());

        // And it is still a ring — the middle is still empty.
        assert_eq!(
            pieces[0].winding(Point::new(50.0, 50.0)),
            0,
            "the hole was filled in"
        );
        assert_ne!(pieces[0].winding(Point::new(10.0, 50.0)), 0);
    }

    /// Two rings side by side are two pieces, each keeping its own hole —
    /// the case that catches a splitter that hands every hole to the first
    /// outer contour it finds.
    #[test]
    fn two_rings_keep_one_hole_each() {
        let opts = BooleanOptions::default();
        let left = boolean(
            &rect(0.0, 0.0, 100.0, 100.0),
            &rect(30.0, 30.0, 70.0, 70.0),
            BoolOp::Difference,
            opts,
        );
        let right = boolean(
            &rect(200.0, 0.0, 300.0, 100.0),
            &rect(230.0, 30.0, 270.0, 70.0),
            BoolOp::Difference,
            opts,
        );
        let mut both = left.clone();
        both.extend(right.iter());

        let pieces = split_disjoint(&both);
        assert_eq!(pieces.len(), 2);
        for piece in &pieces {
            let centre = piece.bounding_box().center();
            assert_eq!(
                piece.winding(centre),
                0,
                "each ring should keep its own hole"
            );
        }
    }

    /// An undivided shape comes back as itself, and nothing comes back as
    /// nothing — the two cases the eraser hits most often.
    #[test]
    fn a_whole_shape_and_an_empty_one_survive_splitting() {
        let solid = rect(0.0, 0.0, 10.0, 10.0);
        let pieces = split_disjoint(&solid);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].bounding_box(), solid.bounding_box());

        assert!(split_disjoint(&BezPath::new()).is_empty());
    }
}
