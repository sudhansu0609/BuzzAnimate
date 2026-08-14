//! Hit-testing: turning a click into a selection.
//!
//! Animate's Selection tool distinguishes clicking a shape's **fill** from
//! clicking its **stroke** — they select independently — so both tests are
//! needed, not just one.
//!
//! # Tolerance is not optional
//!
//! A hairline stroke is one pixel wide. Requiring the user to click within
//! half a document unit of it would make thin lines unselectable, so hit tests
//! take a tolerance expressed in *document units*, which callers derive from
//! the current zoom: a few screen pixels divided by `camera.zoom`. That is why
//! these functions never look at the camera themselves — the same geometry must
//! hit-test differently at different zoom levels.

use kurbo::{BezPath, ParamCurve, ParamCurveNearest, Point, Rect, Shape};
use rayon::prelude::*;

use crate::boolean::FillMode;

/// Where on a path the nearest point lies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearestPoint {
    /// The closest point on the path.
    pub point: Point,
    /// Distance from the query point, in document units.
    pub distance: f64,
    /// Index of the segment it lies on, as yielded by `BezPath::segments`.
    pub segment: usize,
    /// Parameter within that segment, in `0.0..=1.0`.
    pub t: f64,
}

/// Is `point` inside the filled region of `path`?
///
/// Open subpaths are treated as implicitly closed, which is what Animate does
/// when it fills one.
pub fn fill_contains(path: &BezPath, point: Point, fill: FillMode) -> bool {
    // A bounding-box rejection first: cheap, and the common answer is "no".
    if !path.bounding_box().contains(point) {
        return false;
    }
    let winding = path.winding(point);
    match fill {
        FillMode::NonZero => winding != 0,
        FillMode::EvenOdd => winding % 2 != 0,
    }
}

/// Nearest point on the path outline to `point`.
///
/// Used for stroke hit-testing, snapping, and placing a new anchor with the
/// subselection tool. Returns `None` only for a path with no segments.
pub fn nearest_on_path(path: &BezPath, point: Point, accuracy: f64) -> Option<NearestPoint> {
    let accuracy = if accuracy.is_finite() && accuracy > 0.0 {
        accuracy
    } else {
        1e-6
    };

    let mut best: Option<NearestPoint> = None;
    for (index, seg) in path.segments().enumerate() {
        let near = seg.nearest(point, accuracy);
        let distance = near.distance_sq.max(0.0).sqrt();
        if best.is_none_or(|b| distance < b.distance) {
            best = Some(NearestPoint {
                point: seg.eval(near.t),
                distance,
                segment: index,
                t: near.t,
            });
        }
    }
    best
}

/// Did the user click on the stroke of `path`?
///
/// `stroke_width` is the drawn width; `tolerance` is the extra slack in
/// document units, so a hairline is still clickable.
pub fn stroke_contains(path: &BezPath, point: Point, stroke_width: f64, tolerance: f64) -> bool {
    let reach = (stroke_width.max(0.0) * 0.5) + tolerance.max(0.0);
    if reach <= 0.0 {
        return false;
    }

    // Reject against the bounding box grown by the reach before doing any
    // per-segment root finding.
    if !path.bounding_box().inflate(reach, reach).contains(point) {
        return false;
    }

    // Accuracy finer than the reach, so the answer is not decided by the
    // solver's own error.
    let accuracy = (reach * 1e-3).max(1e-9);
    nearest_on_path(path, point, accuracy).is_some_and(|n| n.distance <= reach)
}

/// One candidate in a hit test, as the scene presents it.
#[derive(Debug, Clone, Copy)]
pub struct HitTarget<'a> {
    pub path: &'a BezPath,
    /// Whether the shape has a fill that can be clicked.
    pub filled: bool,
    /// Stroke width, if it has a clickable stroke.
    pub stroke_width: Option<f64>,
    /// Locked or hidden layers are skipped, matching Animate.
    pub selectable: bool,
}

impl<'a> HitTarget<'a> {
    pub fn new(path: &'a BezPath) -> Self {
        Self {
            path,
            filled: true,
            stroke_width: None,
            selectable: true,
        }
    }

    pub fn with_stroke(mut self, width: f64) -> Self {
        self.stroke_width = Some(width);
        self
    }

    pub fn filled(mut self, filled: bool) -> Self {
        self.filled = filled;
        self
    }

    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }
}

/// Which part of a shape was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitPart {
    Fill,
    Stroke,
}

/// The result of a hit test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Index into the slice that was tested.
    pub index: usize,
    pub part: HitPart,
}

/// Test one target.
fn test_one(
    target: &HitTarget<'_>,
    point: Point,
    tolerance: f64,
    fill: FillMode,
) -> Option<HitPart> {
    if !target.selectable {
        return None;
    }
    // Stroke wins over fill: it sits on top visually, and on a shape whose
    // stroke and fill are separately selectable that is what the user means.
    if let Some(width) = target.stroke_width
        && stroke_contains(target.path, point, width, tolerance)
    {
        return Some(HitPart::Stroke);
    }
    if target.filled && fill_contains(target.path, point, fill) {
        return Some(HitPart::Fill);
    }
    None
}

/// Find the topmost shape under `point`.
///
/// Targets are in paint order, so **later entries are on top** and the highest
/// matching index wins — the same convention as the layer stack.
///
/// Runs in parallel. Selecting in a scene with thousands of objects is exactly
/// the interaction that feels slow in a single-threaded editor, and the work
/// is embarrassingly parallel.
pub fn hit_test_topmost(
    targets: &[HitTarget<'_>],
    point: Point,
    tolerance: f64,
    fill: FillMode,
) -> Option<Hit> {
    // Below this, the parallel split costs more than the work.
    const PARALLEL_THRESHOLD: usize = 64;

    if targets.len() < PARALLEL_THRESHOLD {
        return targets.iter().enumerate().rev().find_map(|(index, t)| {
            test_one(t, point, tolerance, fill).map(|part| Hit { index, part })
        });
    }

    targets
        .par_iter()
        .enumerate()
        .filter_map(|(index, t)| {
            test_one(t, point, tolerance, fill).map(|part| Hit { index, part })
        })
        .max_by_key(|h| h.index)
}

/// Every shape under `point`, topmost first.
///
/// Backs alt-click cycling through overlapping objects.
pub fn hit_test_all(
    targets: &[HitTarget<'_>],
    point: Point,
    tolerance: f64,
    fill: FillMode,
) -> Vec<Hit> {
    let mut hits: Vec<Hit> = targets
        .par_iter()
        .enumerate()
        .filter_map(|(index, t)| {
            test_one(t, point, tolerance, fill).map(|part| Hit { index, part })
        })
        .collect();
    // Topmost first, so callers can cycle through overlaps in paint order.
    hits.sort_unstable_by_key(|h| std::cmp::Reverse(h.index));
    hits
}

/// Indices whose shapes intersect `rect`.
///
/// Animate's marquee: `crossing` false selects only shapes wholly inside,
/// which is the default; true also takes shapes the rectangle merely touches.
pub fn select_in_rect(targets: &[HitTarget<'_>], rect: Rect, crossing: bool) -> Vec<usize> {
    targets
        .par_iter()
        .enumerate()
        .filter(|(_, t)| t.selectable)
        .filter(|(_, t)| {
            let bb = t.path.bounding_box();
            if crossing {
                // `Rect::intersect` clamps to a zero-size rect rather than
                // returning negative extents, so testing its width would
                // always pass. `overlaps` is the correct predicate.
                bb.overlaps(rect)
            } else {
                rect.contains_rect(bb)
            }
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Rect};

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        Rect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    #[test]
    fn fill_hit_testing_respects_the_boundary() {
        let sq = square(0.0, 0.0, 10.0);
        assert!(fill_contains(&sq, Point::new(5.0, 5.0), FillMode::NonZero));
        assert!(!fill_contains(
            &sq,
            Point::new(15.0, 5.0),
            FillMode::NonZero
        ));
        assert!(!fill_contains(
            &sq,
            Point::new(-1.0, 5.0),
            FillMode::NonZero
        ));
    }

    #[test]
    fn even_odd_and_nonzero_disagree_about_a_hole() {
        // Outer square with an inner square wound the same way.
        let mut path = square(0.0, 0.0, 20.0);
        path.extend(square(5.0, 5.0, 10.0).iter());

        let inside_inner = Point::new(10.0, 10.0);
        assert!(
            fill_contains(&path, inside_inner, FillMode::NonZero),
            "non-zero should keep the overlap filled"
        );
        assert!(
            !fill_contains(&path, inside_inner, FillMode::EvenOdd),
            "even-odd should punch a hole"
        );
    }

    #[test]
    fn a_thin_stroke_is_still_clickable_thanks_to_tolerance() {
        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(100.0, 0.0));

        // 2 units away from a hairline.
        let near = Point::new(50.0, 2.0);
        assert!(
            !stroke_contains(&line, near, 0.1, 0.0),
            "without tolerance a hairline should not be hit from 2 units away"
        );
        assert!(
            stroke_contains(&line, near, 0.1, 3.0),
            "with 3 units of tolerance it should be hit"
        );
    }

    #[test]
    fn stroke_hit_testing_uses_half_the_width() {
        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(100.0, 0.0));

        // Width 10 reaches 5 units either side.
        assert!(stroke_contains(&line, Point::new(50.0, 4.5), 10.0, 0.0));
        assert!(!stroke_contains(&line, Point::new(50.0, 5.5), 10.0, 0.0));
    }

    #[test]
    fn nearest_point_lands_on_the_path() {
        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(100.0, 0.0));

        let n = nearest_on_path(&line, Point::new(30.0, 12.0), 1e-9).unwrap();
        assert!((n.point.x - 30.0).abs() < 1e-6, "got {:?}", n.point);
        assert!((n.point.y - 0.0).abs() < 1e-6);
        assert!((n.distance - 12.0).abs() < 1e-6, "distance {}", n.distance);
        assert_eq!(n.segment, 0);
    }

    #[test]
    fn nearest_point_reports_the_right_segment() {
        let sq = square(0.0, 0.0, 10.0);
        // Just outside the right edge.
        let n = nearest_on_path(&sq, Point::new(12.0, 5.0), 1e-9).unwrap();
        assert!((n.distance - 2.0).abs() < 1e-6, "distance {}", n.distance);
        assert!((n.point.x - 10.0).abs() < 1e-6, "point {:?}", n.point);
    }

    #[test]
    fn nearest_on_an_empty_path_is_none() {
        assert!(nearest_on_path(&BezPath::new(), Point::ORIGIN, 1e-9).is_none());
    }

    #[test]
    fn topmost_wins_when_shapes_overlap() {
        let bottom = square(0.0, 0.0, 20.0);
        let top = square(5.0, 5.0, 20.0);
        let targets = vec![HitTarget::new(&bottom), HitTarget::new(&top)];

        // In the overlap, the later entry must win.
        let hit = hit_test_topmost(&targets, Point::new(10.0, 10.0), 0.0, FillMode::NonZero);
        assert_eq!(hit.map(|h| h.index), Some(1));

        // Only the bottom shape covers this point.
        let hit = hit_test_topmost(&targets, Point::new(2.0, 2.0), 0.0, FillMode::NonZero);
        assert_eq!(hit.map(|h| h.index), Some(0));
    }

    #[test]
    fn unselectable_targets_are_skipped() {
        let sq = square(0.0, 0.0, 20.0);
        let targets = vec![HitTarget::new(&sq).selectable(false)];
        assert!(
            hit_test_topmost(&targets, Point::new(10.0, 10.0), 0.0, FillMode::NonZero).is_none()
        );
    }

    #[test]
    fn stroke_takes_priority_over_fill() {
        let sq = square(0.0, 0.0, 20.0);
        let targets = vec![HitTarget::new(&sq).with_stroke(4.0)];

        // On the edge: the stroke.
        let hit =
            hit_test_topmost(&targets, Point::new(0.5, 10.0), 0.0, FillMode::NonZero).unwrap();
        assert_eq!(hit.part, HitPart::Stroke);

        // Well inside: the fill.
        let hit =
            hit_test_topmost(&targets, Point::new(10.0, 10.0), 0.0, FillMode::NonZero).unwrap();
        assert_eq!(hit.part, HitPart::Fill);
    }

    #[test]
    fn an_unfilled_shape_is_only_hit_on_its_stroke() {
        let sq = square(0.0, 0.0, 20.0);
        let targets = vec![HitTarget::new(&sq).filled(false).with_stroke(2.0)];

        assert!(
            hit_test_topmost(&targets, Point::new(10.0, 10.0), 0.0, FillMode::NonZero).is_none()
        );
        assert!(
            hit_test_topmost(&targets, Point::new(0.0, 10.0), 0.0, FillMode::NonZero).is_some()
        );
    }

    /// The parallel path must agree with the sequential one.
    #[test]
    fn the_parallel_and_sequential_paths_agree() {
        // Well over the threshold that switches strategies.
        let squares: Vec<BezPath> = (0..500).map(|i| square(i as f64, 0.0, 30.0)).collect();
        let targets: Vec<HitTarget<'_>> = squares.iter().map(HitTarget::new).collect();

        for x in [5.0, 100.0, 250.0, 480.0, 600.0] {
            let point = Point::new(x, 15.0);

            let parallel = hit_test_topmost(&targets, point, 0.0, FillMode::NonZero);
            let sequential = targets.iter().enumerate().rev().find_map(|(i, t)| {
                test_one(t, point, 0.0, FillMode::NonZero).map(|part| Hit { index: i, part })
            });

            assert_eq!(
                parallel.map(|h| h.index),
                sequential.map(|h| h.index),
                "parallel and sequential disagreed at x={x}"
            );
        }
    }

    #[test]
    fn hit_test_all_returns_overlaps_topmost_first() {
        let a = square(0.0, 0.0, 30.0);
        let b = square(5.0, 5.0, 30.0);
        let c = square(100.0, 100.0, 10.0);
        let targets = vec![HitTarget::new(&a), HitTarget::new(&b), HitTarget::new(&c)];

        let hits = hit_test_all(&targets, Point::new(10.0, 10.0), 0.0, FillMode::NonZero);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].index, 1, "topmost should come first");
        assert_eq!(hits[1].index, 0);
    }

    #[test]
    fn marquee_selection_distinguishes_enclosing_from_crossing() {
        let inside = square(10.0, 10.0, 10.0);
        let straddling = square(45.0, 10.0, 20.0);
        let outside = square(200.0, 200.0, 10.0);
        let targets = vec![
            HitTarget::new(&inside),
            HitTarget::new(&straddling),
            HitTarget::new(&outside),
        ];
        let marquee = Rect::new(0.0, 0.0, 50.0, 50.0);

        assert_eq!(select_in_rect(&targets, marquee, false), vec![0]);
        assert_eq!(select_in_rect(&targets, marquee, true), vec![0, 1]);
    }

    #[test]
    fn hit_testing_an_empty_scene_is_fine() {
        let targets: Vec<HitTarget<'_>> = Vec::new();
        assert!(hit_test_topmost(&targets, Point::ORIGIN, 1.0, FillMode::NonZero).is_none());
        assert!(hit_test_all(&targets, Point::ORIGIN, 1.0, FillMode::NonZero).is_empty());
        assert!(select_in_rect(&targets, Rect::new(0.0, 0.0, 10.0, 10.0), true).is_empty());
    }

    #[test]
    fn curved_shapes_hit_test_correctly() {
        let circle = Circle::new(Point::new(0.0, 0.0), 50.0).to_path(1e-9);

        assert!(fill_contains(
            &circle,
            Point::new(0.0, 0.0),
            FillMode::NonZero
        ));
        assert!(fill_contains(
            &circle,
            Point::new(35.0, 35.0),
            FillMode::NonZero
        ));
        // Inside the bounding box but outside the circle.
        assert!(!fill_contains(
            &circle,
            Point::new(45.0, 45.0),
            FillMode::NonZero
        ));

        let n = nearest_on_path(&circle, Point::new(100.0, 0.0), 1e-9).unwrap();
        assert!(
            (n.distance - 50.0).abs() < 0.01,
            "distance to the circle was {}",
            n.distance
        );
    }
}
