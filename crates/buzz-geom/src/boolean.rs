//! Boolean operations on filled paths.
//!
//! # Why these exist
//!
//! Animate's **merge-shape** drawing model is built on them. Draw one filled
//! shape over another on the same layer and the overlap is destructively
//! merged; draw with a different colour and it cuts. Nothing else in the
//! product reproduces that behaviour, so booleans are a prerequisite for the
//! drawing tools in Phase 2, not an optional extra.
//!
//! # Approach, and its one trade-off
//!
//! Robust Bézier-native boolean operations are notoriously hard to get right:
//! the failure modes are tangencies, self-intersections and near-degenerate
//! curves, and a subtly wrong answer corrupts the user's artwork silently.
//! Rather than write one, this module flattens to polygons, delegates to
//! [`i_overlay`] — which is well tested and handles self-intersection, holes
//! and multiple contours — and then optionally refits Béziers to the result
//! with [`kurbo::simplify`].
//!
//! The trade-off is that curves make a round trip through line segments.
//! Flattening tolerance is therefore a real quality control, not a
//! micro-optimisation. [`BooleanOptions::for_shape_size`] derives one from the
//! geometry so callers do not have to guess.
//!
//! # Precision note
//!
//! `i_overlay` snaps coordinates to an integer grid internally. That is fine
//! here because booleans are a **document edit at authoring scale**, not a
//! render-path operation — they are never performed at 1e12× zoom, where the
//! `f64` work in [`crate::camera`] matters. Keep it that way: do not call these
//! on render-space geometry.

use i_overlay::core::fill_rule::FillRule as IFillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use kurbo::{BezPath, PathEl, Point, Shape};
use rayon::prelude::*;

/// Which boolean operation to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    /// Everything covered by either path. Animate: merging same-colour shapes.
    Union,
    /// Only what both cover.
    Intersect,
    /// The first path with the second removed. Animate: cutting a shape.
    Difference,
    /// Covered by exactly one of the two.
    Xor,
}

impl BoolOp {
    fn to_overlay_rule(self) -> OverlayRule {
        match self {
            Self::Union => OverlayRule::Union,
            Self::Intersect => OverlayRule::Intersect,
            Self::Difference => OverlayRule::Difference,
            Self::Xor => OverlayRule::Xor,
        }
    }
}

/// How overlapping sub-regions of a single path are filled.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
pub enum FillMode {
    /// Overlaps stay filled. The sane default for artwork and what Vello uses.
    #[default]
    NonZero,
    /// Overlaps punch holes.
    EvenOdd,
}

impl FillMode {
    fn to_fill_rule(self) -> IFillRule {
        match self {
            Self::NonZero => IFillRule::NonZero,
            Self::EvenOdd => IFillRule::EvenOdd,
        }
    }
}

/// Tuning for a boolean operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BooleanOptions {
    /// Flattening tolerance, in document units. Smaller is more faithful and
    /// slower.
    pub tolerance: f64,
    pub fill: FillMode,
    /// Refit Béziers to the polygonal result.
    ///
    /// On for edits the user will keep working with, so curves survive. Off
    /// when the result is immediately consumed (hit-testing, area queries),
    /// where fitting is wasted work.
    pub refit_curves: bool,
    /// Tangent of the turn angle above which a vertex is kept as a hard corner
    /// during refitting. See [`CORNER_TANGENT_THRESHOLD`].
    pub corner_threshold: f64,
}

impl Default for BooleanOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.05,
            fill: FillMode::NonZero,
            refit_curves: true,
            corner_threshold: CORNER_TANGENT_THRESHOLD,
        }
    }
}

impl BooleanOptions {
    /// Derive a tolerance from the size of the geometry involved.
    ///
    /// A fixed tolerance is wrong at both extremes: too coarse for a 2-unit
    /// glyph, needlessly fine for a 10 000-unit background. One ten-thousandth
    /// of the diagonal keeps the error imperceptible at any scale.
    pub fn for_shape_size(diagonal: f64) -> Self {
        let tolerance = if diagonal.is_finite() && diagonal > 0.0 {
            (diagonal / 10_000.0).clamp(1e-9, 1.0)
        } else {
            0.05
        };
        Self {
            tolerance,
            ..Default::default()
        }
    }

    /// Cheap variant for results that are measured rather than edited.
    pub fn fast(mut self) -> Self {
        self.refit_curves = false;
        self
    }
}

/// Flatten a path into closed contours of points.
///
/// Every contour is treated as closed: boolean operations are defined on
/// filled regions, so an open subpath is implicitly closed, which is also what
/// Animate does when it fills one.
fn to_contours(path: &BezPath, tolerance: f64) -> Vec<Vec<[f64; 2]>> {
    let mut contours: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();

    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            if current.len() >= 3 {
                contours.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            current.push([p.x, p.y]);
        }
        PathEl::LineTo(p) => current.push([p.x, p.y]),
        PathEl::ClosePath => {
            if current.len() >= 3 {
                contours.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
        // `flatten` emits only MoveTo, LineTo and ClosePath.
        _ => {}
    });

    if current.len() >= 3 {
        contours.push(current);
    }
    contours
}

/// Rebuild a path from `i_overlay`'s shape/contour output.
fn from_shapes(shapes: &[Vec<Vec<[f64; 2]>>]) -> BezPath {
    let mut out = BezPath::new();
    for shape in shapes {
        for contour in shape {
            if contour.len() < 3 {
                continue;
            }
            out.move_to(Point::new(contour[0][0], contour[0][1]));
            for p in &contour[1..] {
                out.line_to(Point::new(p[0], p[1]));
            }
            out.close_path();
        }
    }
    out
}

/// Tangent of the turn angle above which a vertex is a genuine corner.
///
/// kurbo's default is ~1 milliradian, which is far too strict for input that
/// has just been flattened: *every* vertex of a flattened curve exceeds it, so
/// the whole path is treated as corners and refitting returns pure lines.
///
/// Flattening with tolerance `t` on radius `r` leaves a turn of roughly
/// `2·√(2t/r)` at each vertex. [`BooleanOptions::for_shape_size`] keeps `t/r`
/// near `1/2500`, giving turns of about `0.057`. A threshold of `0.2`
/// (≈11°) therefore sits comfortably above the noise while still treating a
/// real corner — a square's 90°, say — as a corner.
const CORNER_TANGENT_THRESHOLD: f64 = 0.2;

/// Refit Béziers to a polygonal path, **verifying the result**.
///
/// # Why the output is checked
///
/// Curve fitting is a heuristic, and kurbo says so: its simplifier "works best
/// if the source path is very smooth", and warns results may be poor otherwise.
/// That is not a theoretical caveat. Expanding a 100×100 square by 5 produces a
/// correct polygon spanning `-5..105`, which the fitter turned into a path
/// spanning `-5..1071` — a tenfold overshoot on a rounded corner. Trusting it
/// blindly would silently corrupt the user's artwork.
///
/// So the fit is accepted only if it stays within the source bounds plus a
/// small slack and preserves area. Otherwise the polygon is kept. A correct
/// polygon always beats a corrupted curve; the cost of falling back is more
/// anchor points, not a wrong shape.
pub(crate) fn refit_checked(
    source: &BezPath,
    accuracy: f64,
    corner_threshold: f64,
    area_slack: f64,
) -> BezPath {
    if source.elements().is_empty() {
        return source.clone();
    }

    let options = kurbo::simplify::SimplifyOptions::default().angle_thresh(corner_threshold);
    let fitted = kurbo::simplify::simplify_bezpath(source.clone(), accuracy, &options);

    if fitted.elements().is_empty() {
        return source.clone();
    }

    let source_bb = source.bounding_box();
    let diagonal = source_bb.width().hypot(source_bb.height());
    let slack = (accuracy * 4.0).max(diagonal * 1e-4);

    if !source_bb.inflate(slack, slack).contains_rect(fitted.bounding_box()) {
        return source.clone();
    }

    let source_area = source.area().abs();
    if source_area > 0.0 {
        let drift = (fitted.area().abs() - source_area).abs() / source_area;
        if drift > area_slack {
            return source.clone();
        }
    }

    fitted
}

/// Refit for boolean results: geometry must not move perceptibly.
fn refit(path: BezPath, tolerance: f64, corner_threshold: f64) -> BezPath {
    refit_checked(&path, tolerance, corner_threshold, 0.02)
}

/// Combine two paths.
///
/// Returns an empty path when the result is empty (for example intersecting
/// disjoint shapes), which callers should treat as "the shape is gone".
pub fn boolean(a: &BezPath, b: &BezPath, op: BoolOp, opts: BooleanOptions) -> BezPath {
    let tolerance = if opts.tolerance.is_finite() && opts.tolerance > 0.0 {
        opts.tolerance
    } else {
        BooleanOptions::default().tolerance
    };

    let subj = to_contours(a, tolerance);
    let clip = to_contours(b, tolerance);

    // Shortcuts that also avoid handing the solver degenerate input.
    match (subj.is_empty(), clip.is_empty()) {
        (true, true) => return BezPath::new(),
        (true, false) => {
            return match op {
                BoolOp::Union | BoolOp::Xor => b.clone(),
                BoolOp::Intersect | BoolOp::Difference => BezPath::new(),
            };
        }
        (false, true) => {
            return match op {
                BoolOp::Union | BoolOp::Xor | BoolOp::Difference => a.clone(),
                BoolOp::Intersect => BezPath::new(),
            };
        }
        (false, false) => {}
    }

    let shapes = subj.overlay(&clip, op.to_overlay_rule(), opts.fill.to_fill_rule());
    let result = from_shapes(&shapes);

    if opts.refit_curves {
        refit(result, tolerance, opts.corner_threshold)
    } else {
        result
    }
}

/// Fold an operation across many paths, in parallel.
///
/// Uses a tree reduction rather than a left fold, so `n` paths combine in
/// `log n` dependent rounds across the pool instead of `n` sequential steps.
/// This is the shape of the work when the user selects a hundred objects and
/// merges them.
///
/// Only associative operations are meaningful here — [`BoolOp::Union`],
/// [`BoolOp::Intersect`] and [`BoolOp::Xor`]. [`BoolOp::Difference`] is not
/// associative, so it is applied as a left fold against the first path, which
/// is what "subtract all of these from that" means.
pub fn boolean_many(paths: &[BezPath], op: BoolOp, opts: BooleanOptions) -> BezPath {
    match paths.len() {
        0 => return BezPath::new(),
        1 => return paths[0].clone(),
        _ => {}
    }

    if op == BoolOp::Difference {
        // Order matters, so this one cannot be tree-reduced.
        return paths[1..]
            .iter()
            .fold(paths[0].clone(), |acc, p| boolean(&acc, p, op, opts));
    }

    paths
        .par_iter()
        .cloned()
        .reduce(BezPath::new, |a, b| {
            if a.elements().is_empty() {
                return b;
            }
            if b.elements().is_empty() {
                return a;
            }
            boolean(&a, &b, op, opts)
        })
}

/// Merge overlapping shapes into as few paths as possible.
///
/// This is what Animate does when shapes of the same colour touch on a layer.
pub fn union_all(paths: &[BezPath], opts: BooleanOptions) -> BezPath {
    boolean_many(paths, BoolOp::Union, opts)
}

/// Suggested options for a set of paths, sized from their combined bounds.
pub fn options_for(paths: &[BezPath]) -> BooleanOptions {
    let bounds = paths
        .iter()
        .map(|p| p.bounding_box())
        .reduce(|a, b| a.union(b));

    match bounds {
        Some(b) if b.width().is_finite() && b.height().is_finite() => {
            BooleanOptions::for_shape_size(b.width().hypot(b.height()))
        }
        _ => BooleanOptions::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Rect};

    /// Unsigned area, which is what "how much is covered" means here.
    fn area(p: &BezPath) -> f64 {
        p.area().abs()
    }

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        Rect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    /// Two 10×10 squares overlapping in a 5×5 region.
    fn overlapping_pair() -> (BezPath, BezPath) {
        (square(0.0, 0.0, 10.0), square(5.0, 5.0, 10.0))
    }

    #[test]
    fn union_area_is_the_sum_less_the_overlap() {
        let (a, b) = overlapping_pair();
        let r = boolean(&a, &b, BoolOp::Union, BooleanOptions::default());
        // 100 + 100 - 25
        assert!(
            (area(&r) - 175.0).abs() < 0.5,
            "union area was {}, expected ~175",
            area(&r)
        );
    }

    #[test]
    fn intersect_yields_only_the_overlap() {
        let (a, b) = overlapping_pair();
        let r = boolean(&a, &b, BoolOp::Intersect, BooleanOptions::default());
        assert!(
            (area(&r) - 25.0).abs() < 0.5,
            "intersection area was {}, expected ~25",
            area(&r)
        );
    }

    #[test]
    fn difference_removes_the_second_shape() {
        let (a, b) = overlapping_pair();
        let r = boolean(&a, &b, BoolOp::Difference, BooleanOptions::default());
        assert!(
            (area(&r) - 75.0).abs() < 0.5,
            "difference area was {}, expected ~75",
            area(&r)
        );
    }

    #[test]
    fn xor_leaves_what_exactly_one_shape_covers() {
        let (a, b) = overlapping_pair();
        let r = boolean(&a, &b, BoolOp::Xor, BooleanOptions::default());
        // 175 total minus the 25 shared
        assert!(
            (area(&r) - 150.0).abs() < 0.5,
            "xor area was {}, expected ~150",
            area(&r)
        );
    }

    #[test]
    fn difference_is_not_commutative() {
        let (a, b) = overlapping_pair();
        let ab = boolean(&a, &b, BoolOp::Difference, BooleanOptions::default());
        let ba = boolean(&b, &a, BoolOp::Difference, BooleanOptions::default());
        // Same area here only because the squares are congruent; the shapes
        // must still differ.
        assert!(
            (ab.bounding_box().x0 - ba.bounding_box().x0).abs() > 1.0,
            "a-b and b-a produced the same geometry"
        );
    }

    #[test]
    fn disjoint_shapes_union_into_two_separate_regions() {
        let a = square(0.0, 0.0, 10.0);
        let b = square(100.0, 100.0, 10.0);
        let r = boolean(&a, &b, BoolOp::Union, BooleanOptions::default());

        assert!((area(&r) - 200.0).abs() < 0.5, "area was {}", area(&r));
        let moves = r
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::MoveTo(_)))
            .count();
        assert_eq!(moves, 2, "expected two separate contours");
    }

    #[test]
    fn intersecting_disjoint_shapes_gives_nothing() {
        let a = square(0.0, 0.0, 10.0);
        let b = square(100.0, 100.0, 10.0);
        let r = boolean(&a, &b, BoolOp::Intersect, BooleanOptions::default());
        assert!(
            area(&r) < 1e-6,
            "expected an empty result, got area {}",
            area(&r)
        );
    }

    #[test]
    fn subtracting_a_shape_from_itself_leaves_nothing() {
        let a = square(0.0, 0.0, 10.0);
        let r = boolean(&a, &a, BoolOp::Difference, BooleanOptions::default());
        assert!(area(&r) < 1e-6, "expected nothing, got area {}", area(&r));
    }

    #[test]
    fn a_hole_can_be_punched_and_measured() {
        let outer = square(0.0, 0.0, 20.0);
        let inner = square(5.0, 5.0, 10.0);
        let r = boolean(&outer, &inner, BoolOp::Difference, BooleanOptions::default());
        // 400 - 100
        assert!(
            (area(&r) - 300.0).abs() < 1.0,
            "ring area was {}, expected ~300",
            area(&r)
        );
        // The hole must be a second contour.
        let moves = r
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::MoveTo(_)))
            .count();
        assert_eq!(moves, 2, "expected an outer contour and a hole");
    }

    #[test]
    fn curved_shapes_survive_the_round_trip() {
        let a = Circle::new(Point::new(0.0, 0.0), 10.0).to_path(1e-9);
        let b = Circle::new(Point::new(8.0, 0.0), 10.0).to_path(1e-9);
        let opts = BooleanOptions::for_shape_size(40.0);

        let u = boolean(&a, &b, BoolOp::Union, opts);
        let expected_max = std::f64::consts::PI * 100.0 * 2.0;

        assert!(
            area(&u) > expected_max * 0.5 && area(&u) < expected_max,
            "union of two circles had area {}, outside plausible range",
            area(&u)
        );
        // Refitting should produce curves, not a polygon.
        assert!(
            u.elements()
                .iter()
                .any(|e| matches!(e, PathEl::CurveTo(..))),
            "expected refitted curves in the output"
        );
    }

    /// The risk introduced by raising the corner threshold: refitting must not
    /// round off genuine corners. An L-shape has 90° and 270° turns that have
    /// to survive.
    #[test]
    fn refitting_preserves_real_corners() {
        let a = square(0.0, 0.0, 20.0);
        let b = square(10.0, 10.0, 20.0);
        let l = boolean(&a, &b, BoolOp::Difference, BooleanOptions::default());

        // 400 - 100 overlap
        assert!(
            (area(&l) - 300.0).abs() < 0.5,
            "corners were rounded: area {} should be ~300",
            area(&l)
        );

        // The bounding box must stay exactly the original square.
        let bb = l.bounding_box();
        assert!(
            (bb.x0 - 0.0).abs() < 0.01
                && (bb.y0 - 0.0).abs() < 0.01
                && (bb.x1 - 20.0).abs() < 0.01
                && (bb.y1 - 20.0).abs() < 0.01,
            "corner rounding moved the bounds: {bb:?}"
        );
    }

    #[test]
    fn refitting_can_be_turned_off() {
        let a = Circle::new(Point::new(0.0, 0.0), 10.0).to_path(1e-9);
        let b = Circle::new(Point::new(8.0, 0.0), 10.0).to_path(1e-9);
        let opts = BooleanOptions::for_shape_size(40.0).fast();

        let u = boolean(&a, &b, BoolOp::Union, opts);
        assert!(
            !u.elements()
                .iter()
                .any(|e| matches!(e, PathEl::CurveTo(..))),
            "fast mode should leave a polygon"
        );
    }

    #[test]
    fn empty_inputs_are_handled_without_panicking() {
        let empty = BezPath::new();
        let a = square(0.0, 0.0, 10.0);
        let o = BooleanOptions::default();

        assert!((area(&boolean(&a, &empty, BoolOp::Union, o)) - 100.0).abs() < 0.5);
        assert!(area(&boolean(&a, &empty, BoolOp::Intersect, o)) < 1e-6);
        assert!((area(&boolean(&a, &empty, BoolOp::Difference, o)) - 100.0).abs() < 0.5);
        assert!((area(&boolean(&empty, &a, BoolOp::Union, o)) - 100.0).abs() < 0.5);
        assert!(area(&boolean(&empty, &a, BoolOp::Difference, o)) < 1e-6);
        assert!(area(&boolean(&empty, &empty, BoolOp::Union, o)) < 1e-6);
    }

    #[test]
    fn a_degenerate_tolerance_does_not_produce_garbage() {
        let (a, b) = overlapping_pair();
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let opts = BooleanOptions {
                tolerance: bad,
                ..Default::default()
            };
            let r = boolean(&a, &b, BoolOp::Union, opts);
            assert!(
                (area(&r) - 175.0).abs() < 1.0,
                "tolerance {bad} gave area {}",
                area(&r)
            );
        }
    }

    #[test]
    fn union_all_matches_a_sequential_fold() {
        let squares: Vec<BezPath> = (0..24)
            .map(|i| square(i as f64 * 4.0, 0.0, 10.0))
            .collect();
        let opts = options_for(&squares);

        let parallel = union_all(&squares, opts);
        let sequential = squares
            .iter()
            .skip(1)
            .fold(squares[0].clone(), |acc, p| {
                boolean(&acc, p, BoolOp::Union, opts)
            });

        assert!(
            (area(&parallel) - area(&sequential)).abs() < 1.0,
            "parallel {} vs sequential {}",
            area(&parallel),
            area(&sequential)
        );
    }

    #[test]
    fn boolean_many_handles_trivial_inputs() {
        let o = BooleanOptions::default();
        assert!(area(&boolean_many(&[], BoolOp::Union, o)) < 1e-6);

        let one = square(0.0, 0.0, 10.0);
        assert!((area(&boolean_many(std::slice::from_ref(&one), BoolOp::Union, o)) - 100.0).abs() < 0.5);
    }

    #[test]
    fn difference_across_many_subtracts_all_of_them() {
        let base = square(0.0, 0.0, 20.0);
        let cuts = vec![
            base.clone(),
            square(0.0, 0.0, 5.0),
            square(15.0, 15.0, 5.0),
        ];
        let r = boolean_many(&cuts, BoolOp::Difference, BooleanOptions::default());
        // 400 - 25 - 25
        assert!(
            (area(&r) - 350.0).abs() < 1.0,
            "expected ~350, got {}",
            area(&r)
        );
    }

    #[test]
    fn tolerance_scales_with_shape_size() {
        let small = BooleanOptions::for_shape_size(2.0);
        let large = BooleanOptions::for_shape_size(10_000.0);
        assert!(small.tolerance < large.tolerance);
        assert!(small.tolerance > 0.0 && large.tolerance.is_finite());

        // Degenerate sizes fall back rather than producing a zero tolerance.
        for bad in [0.0, -5.0, f64::NAN] {
            let o = BooleanOptions::for_shape_size(bad);
            assert!(o.tolerance > 0.0 && o.tolerance.is_finite());
        }
    }
}
