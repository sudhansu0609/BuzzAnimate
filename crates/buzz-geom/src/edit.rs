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
    BezPath, Cap, Join, Line, ParamCurve, ParamCurveArclen, PathEl, PathSeg, Shape, Stroke,
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
