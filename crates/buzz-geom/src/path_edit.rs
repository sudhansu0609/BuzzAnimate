//! Editing individual anchor points on a path.
//!
//! This is what Animate's Subselection tool does: show a path's anchors, let
//! the user drag one, and keep the surrounding curve attached to it.
//!
//! # What "moving an anchor" has to mean
//!
//! A naive implementation moves only the anchor point and leaves the Bézier
//! control handles where they were, which visibly kinks the curve on both
//! sides. Moving the adjacent handles by the same delta keeps the tangents
//! intact, so the curve slides rather than deforming — which is what the user
//! expects and what Animate does.

use kurbo::{BezPath, PathEl, Point, Vec2};

/// An editable anchor on a path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// Index into `BezPath::elements`.
    pub element: usize,
    pub point: Point,
}

/// Every on-curve anchor, in path order.
///
/// Control points are deliberately excluded: Animate's Subselection tool shows
/// handles only for the anchor currently being edited, and listing them here
/// would make them all selectable at once.
pub fn anchors(path: &BezPath) -> Vec<Anchor> {
    path.elements()
        .iter()
        .enumerate()
        .filter_map(|(element, el)| {
            let point = match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => *p,
                PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => *p,
                PathEl::ClosePath => return None,
            };
            Some(Anchor { element, point })
        })
        .collect()
}

/// The anchor nearest `point`, if one is within `tolerance`.
pub fn nearest_anchor(path: &BezPath, point: Point, tolerance: f64) -> Option<Anchor> {
    anchors(path)
        .into_iter()
        .map(|a| (a, (a.point - point).hypot()))
        .filter(|(_, d)| *d <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(a, _)| a)
}

/// Move the anchor at `element` by `delta`, carrying its handles with it.
///
/// Returns false if the index does not name an anchor.
pub fn move_anchor(path: &mut BezPath, element: usize, delta: Vec2) -> bool {
    if !delta.x.is_finite() || !delta.y.is_finite() {
        return false;
    }

    let elements = path.elements().to_vec();
    let Some(target) = elements.get(element) else {
        return false;
    };
    if matches!(target, PathEl::ClosePath) {
        return false;
    }

    let mut updated = elements.clone();

    // The anchor itself, plus the incoming control point that shapes the curve
    // arriving at it.
    updated[element] = match elements[element] {
        PathEl::MoveTo(p) => PathEl::MoveTo(p + delta),
        PathEl::LineTo(p) => PathEl::LineTo(p + delta),
        PathEl::QuadTo(c, p) => PathEl::QuadTo(c + delta, p + delta),
        PathEl::CurveTo(c1, c2, p) => PathEl::CurveTo(c1, c2 + delta, p + delta),
        PathEl::ClosePath => return false,
    };

    // The outgoing control point lives on the *next* element, and must follow
    // too or the curve leaving this anchor will kink.
    if let Some(next) = elements.get(element + 1) {
        updated[element + 1] = match *next {
            PathEl::QuadTo(c, p) => PathEl::QuadTo(c + delta, p),
            PathEl::CurveTo(c1, c2, p) => PathEl::CurveTo(c1 + delta, c2, p),
            other => other,
        };
    }

    // A closed subpath's first and last anchors are the same point on screen,
    // so moving one must move the other or the shape tears open.
    if matches!(elements.last(), Some(PathEl::ClosePath))
        && let Some(PathEl::MoveTo(start)) = elements.first()
        && element != 0
        && let Some(anchor_point) = anchor_of(&elements[element])
        && (anchor_point - *start).hypot() < 1e-9
    {
        updated[0] = PathEl::MoveTo(*start + delta);
    }

    *path = BezPath::from_vec(updated);
    true
}

fn anchor_of(el: &PathEl) -> Option<Point> {
    match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
        PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => Some(*p),
        PathEl::ClosePath => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Rect, Shape};

    fn triangle() -> BezPath {
        let mut p = BezPath::new();
        p.move_to(Point::new(0.0, 0.0));
        p.line_to(Point::new(100.0, 0.0));
        p.line_to(Point::new(50.0, 80.0));
        p.close_path();
        p
    }

    fn curved() -> BezPath {
        let mut p = BezPath::new();
        p.move_to(Point::new(0.0, 0.0));
        p.curve_to(
            Point::new(20.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(100.0, 0.0),
        );
        p.curve_to(
            Point::new(120.0, -40.0),
            Point::new(180.0, -40.0),
            Point::new(200.0, 0.0),
        );
        p
    }

    #[test]
    fn anchors_are_the_on_curve_points_only() {
        let a = anchors(&triangle());
        assert_eq!(a.len(), 3, "close_path is not an anchor");
        assert_eq!(a[0].point, Point::new(0.0, 0.0));
        assert_eq!(a[2].point, Point::new(50.0, 80.0));
    }

    #[test]
    fn curves_report_their_endpoints_not_their_handles() {
        let a = anchors(&curved());
        assert_eq!(a.len(), 3);
        assert_eq!(a[1].point, Point::new(100.0, 0.0));
        assert_eq!(a[2].point, Point::new(200.0, 0.0));
    }

    #[test]
    fn the_nearest_anchor_is_found_within_tolerance() {
        let path = triangle();
        let hit = nearest_anchor(&path, Point::new(98.0, 3.0), 5.0).unwrap();
        assert_eq!(hit.point, Point::new(100.0, 0.0));

        assert!(
            nearest_anchor(&path, Point::new(50.0, 40.0), 5.0).is_none(),
            "the middle of an edge is not an anchor"
        );
    }

    #[test]
    fn moving_an_anchor_moves_it() {
        let mut path = triangle();
        assert!(move_anchor(&mut path, 1, Vec2::new(10.0, 5.0)));
        assert_eq!(anchors(&path)[1].point, Point::new(110.0, 5.0));
    }

    /// The property that separates a usable tool from a frustrating one: the
    /// curve must slide, not kink.
    #[test]
    fn moving_an_anchor_carries_both_adjacent_handles() {
        let mut path = curved();
        let delta = Vec2::new(0.0, 30.0);
        // Element 1 is the first curve; its endpoint is the middle anchor.
        assert!(move_anchor(&mut path, 1, delta));

        let elements = path.elements();
        // Incoming handle (c2 of element 1) must have moved.
        let PathEl::CurveTo(_, c2, p) = elements[1] else {
            panic!("expected a curve")
        };
        assert_eq!(p, Point::new(100.0, 30.0));
        assert_eq!(c2, Point::new(80.0, 70.0), "incoming handle should follow");

        // Outgoing handle (c1 of element 2) must have moved too.
        let PathEl::CurveTo(c1, _, end) = elements[2] else {
            panic!("expected a curve")
        };
        assert_eq!(c1, Point::new(120.0, -10.0), "outgoing handle should follow");
        assert_eq!(end, Point::new(200.0, 0.0), "the far anchor must not move");
    }

    /// Tangent continuity is the visible consequence of carrying the handles.
    #[test]
    fn the_tangent_through_a_moved_anchor_is_preserved() {
        let before = curved();
        let mut after = curved();
        move_anchor(&mut after, 1, Vec2::new(15.0, 25.0));

        let tangent = |path: &BezPath| {
            let PathEl::CurveTo(_, c2, p) = path.elements()[1] else {
                panic!()
            };
            (p - c2).normalize()
        };

        let a = tangent(&before);
        let b = tangent(&after);
        assert!(
            (a - b).hypot() < 1e-9,
            "the tangent changed: {a:?} -> {b:?}"
        );
    }

    /// A closed shape must not tear open when its first anchor moves.
    #[test]
    fn closed_paths_stay_closed() {
        let mut path = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9);
        let last = anchors(&path).len() - 1;
        assert!(move_anchor(&mut path, last, Vec2::new(20.0, 20.0)));

        assert!(
            path.elements()
                .iter()
                .any(|e| matches!(e, PathEl::ClosePath)),
            "the path should still be closed"
        );
        assert!(path.area().abs() > 0.0, "it should still enclose area");
    }

    #[test]
    fn moving_the_first_anchor_of_a_closed_path_works() {
        let mut path = triangle();
        assert!(move_anchor(&mut path, 0, Vec2::new(-10.0, -10.0)));
        assert_eq!(anchors(&path)[0].point, Point::new(-10.0, -10.0));
        assert!(path.elements().iter().any(|e| matches!(e, PathEl::ClosePath)));
    }

    #[test]
    fn a_bad_index_or_delta_is_refused() {
        let mut path = triangle();
        let before = path.elements().to_vec();

        assert!(!move_anchor(&mut path, 999, Vec2::new(1.0, 1.0)));
        assert!(!move_anchor(&mut path, 3, Vec2::new(1.0, 1.0)), "close_path");
        assert!(!move_anchor(&mut path, 0, Vec2::new(f64::NAN, 0.0)));
        assert!(!move_anchor(&mut path, 0, Vec2::new(0.0, f64::INFINITY)));

        assert_eq!(path.elements(), before, "the path must be untouched");
    }

    #[test]
    fn an_empty_path_has_no_anchors() {
        let path = BezPath::new();
        assert!(anchors(&path).is_empty());
        assert!(nearest_anchor(&path, Point::ORIGIN, 10.0).is_none());
    }
}
