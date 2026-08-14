//! Clipping paths for rendering at extreme zoom.
//!
//! # The problem this solves
//!
//! Vello flattens curves to line segments at *screen-space* tolerance. At
//! 1e12× zoom a 300-unit circle spans ~3e14 pixels and would need on the order
//! of 3e7 segments — enough to hang the frame. Phase 0 worked around this by
//! *culling* anything far larger than the viewport, which is wrong: a huge
//! shape whose edge genuinely crosses the view would simply vanish, and a
//! background rectangle larger than the stage would disappear the moment you
//! zoomed in.
//!
//! # Two quantities must be bounded
//!
//! 1. **Segment count** — otherwise flattening cost explodes.
//! 2. **Coordinate magnitude** — otherwise the `f32` handoff to the GPU loses
//!    precision, which is the very thing [`crate::camera`] exists to prevent.
//!
//! Clipping to a rectangle bounds both.
//!
//! # How it works
//!
//! A recursive walk over the path, using the Bézier convex-hull property: a
//! curve lies entirely within the convex hull of its control points, so the
//! control-point bounding box is a cheap conservative test.
//!
//! * **Hull entirely inside the clip bounds** → emit the segment untouched.
//!   This is the common case and costs nothing.
//! * **Hull does not meet the bounds** → the segment is invisible. Emit a
//!   single straight line to its (clamped) endpoint. Connectivity and winding
//!   are preserved, so fills stay correct, but the invisible detail costs one
//!   segment instead of thousands.
//! * **Otherwise** → subdivide and recurse, down to a depth and budget limit.
//!
//! Every emitted coordinate is clamped into the bounds, which caps magnitude.
//!
//! # Why clamping is safe
//!
//! [`RenderClip`] expands the visible rectangle by a generous margin before
//! clipping, so clamped points land well outside what the user can see.
//! Coverage *inside the visible area* is preserved; the approximation is
//! confined to the region beyond the margin. This is a render-path operation
//! only — the document is never modified, and clipping is not used for
//! editing, hit-testing, or export geometry.

use kurbo::{BezPath, CubicBez, PathEl, Point, Rect, Shape};

/// How far beyond the visible rectangle geometry is preserved exactly,
/// as a multiple of the visible size on each side.
const MARGIN_FACTOR: f64 = 1.0;

/// Recursion limit. 2^14 is far more subdivision than any real path needs.
const MAX_DEPTH: u8 = 14;

/// Hard cap on emitted segments, so a pathological input still terminates.
const MAX_SEGMENTS: usize = 20_000;

/// Clips paths into a bounded region for rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderClip {
    /// The clip region: the visible rect plus margin.
    bounds: Rect,
}

impl RenderClip {
    /// Build a clip for the given visible document rectangle.
    pub fn new(visible: Rect) -> Self {
        let mx = visible.width() * MARGIN_FACTOR;
        let my = visible.height() * MARGIN_FACTOR;
        Self {
            bounds: Rect::new(
                visible.x0 - mx,
                visible.y0 - my,
                visible.x1 + mx,
                visible.y1 + my,
            ),
        }
    }

    /// The region within which geometry is preserved exactly.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Whether this clip can be applied at all.
    ///
    /// A camera in a degenerate state (zero-size viewport, non-finite zoom)
    /// produces an unusable rect; clipping is then skipped rather than
    /// producing garbage.
    fn is_usable(&self) -> bool {
        let b = self.bounds;
        b.x0.is_finite()
            && b.y0.is_finite()
            && b.x1.is_finite()
            && b.y1.is_finite()
            && b.width() > 0.0
            && b.height() > 0.0
    }

    #[inline]
    fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.bounds.x0, self.bounds.x1),
            p.y.clamp(self.bounds.y0, self.bounds.y1),
        )
    }

    /// Clip `path` for rendering.
    ///
    /// Returns the input unchanged when it already fits, so the common case
    /// allocates nothing beyond the clone.
    pub fn apply(&self, path: &BezPath) -> BezPath {
        if !self.is_usable() {
            return path.clone();
        }

        // Fast path: entirely inside, nothing to do.
        let bb = path.bounding_box();
        if self.bounds.contains_rect(bb) {
            return path.clone();
        }

        let mut out = BezPath::new();
        let mut cursor = Point::ORIGIN;
        let mut subpath_start = Point::ORIGIN;
        let mut budget = MAX_SEGMENTS;

        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    out.move_to(self.clamp(p));
                    cursor = p;
                    subpath_start = p;
                }
                PathEl::LineTo(p) => {
                    out.line_to(self.clamp(p));
                    cursor = p;
                    budget = budget.saturating_sub(1);
                }
                PathEl::QuadTo(c, p) => {
                    // Raise to cubic so there is one subdivision path to
                    // maintain rather than two.
                    let c1 = cursor + (c - cursor) * (2.0 / 3.0);
                    let c2 = p + (c - p) * (2.0 / 3.0);
                    self.emit_cubic(CubicBez::new(cursor, c1, c2, p), 0, &mut out, &mut budget);
                    cursor = p;
                }
                PathEl::CurveTo(c1, c2, p) => {
                    self.emit_cubic(CubicBez::new(cursor, c1, c2, p), 0, &mut out, &mut budget);
                    cursor = p;
                }
                PathEl::ClosePath => {
                    out.close_path();
                    cursor = subpath_start;
                }
            }
        }

        out
    }

    /// Emit one cubic, subdividing only where it meets the clip bounds.
    fn emit_cubic(&self, c: CubicBez, depth: u8, out: &mut BezPath, budget: &mut usize) {
        let hull = control_hull(c);

        // Wholly invisible: collapse to a single line. Keeping the endpoint
        // (rather than dropping the segment) is what preserves winding, so
        // fills that enclose the viewport still fill it.
        if !hull.overlaps(self.bounds) {
            out.line_to(self.clamp(c.p3));
            *budget = budget.saturating_sub(1);
            return;
        }

        // Wholly visible: keep the curve exactly as authored.
        if self.bounds.contains_rect(hull) {
            out.curve_to(c.p1, c.p2, c.p3);
            *budget = budget.saturating_sub(1);
            return;
        }

        // Straddling the boundary. Subdivide unless we have run out of room,
        // in which case emit a clamped approximation.
        if depth >= MAX_DEPTH || *budget == 0 {
            out.curve_to(self.clamp(c.p1), self.clamp(c.p2), self.clamp(c.p3));
            *budget = budget.saturating_sub(1);
            return;
        }

        let (a, b) = subdivide(c);
        self.emit_cubic(a, depth + 1, out, budget);
        self.emit_cubic(b, depth + 1, out, budget);
    }
}

/// Bounding box of a cubic's control points.
///
/// Conservative (it can be larger than the true curve bounds) but exact enough
/// for rejection, and far cheaper than solving for extrema.
#[inline]
fn control_hull(c: CubicBez) -> Rect {
    let xs = [c.p0.x, c.p1.x, c.p2.x, c.p3.x];
    let ys = [c.p0.y, c.p1.y, c.p2.y, c.p3.y];
    Rect::new(
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

/// De Casteljau split at t = 0.5.
#[inline]
fn subdivide(c: CubicBez) -> (CubicBez, CubicBez) {
    let mid = |a: Point, b: Point| Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);

    let p01 = mid(c.p0, c.p1);
    let p12 = mid(c.p1, c.p2);
    let p23 = mid(c.p2, c.p3);
    let p012 = mid(p01, p12);
    let p123 = mid(p12, p23);
    let p0123 = mid(p012, p123);

    (
        CubicBez::new(c.p0, p01, p012, p0123),
        CubicBez::new(p0123, p123, p23, c.p3),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Line};

    /// The viewport in these tests.
    fn visible() -> Rect {
        Rect::new(0.0, 0.0, 1920.0, 1080.0)
    }

    fn segment_count(p: &BezPath) -> usize {
        p.elements()
            .iter()
            .filter(|e| !matches!(e, PathEl::MoveTo(_) | PathEl::ClosePath))
            .count()
    }

    fn max_coord(p: &BezPath) -> f64 {
        let mut m: f64 = 0.0;
        for el in p.elements() {
            let pts: &[Point] = match el {
                PathEl::MoveTo(a) | PathEl::LineTo(a) => &[*a],
                PathEl::QuadTo(a, b) => &[*a, *b],
                PathEl::CurveTo(a, b, c) => &[*a, *b, *c],
                PathEl::ClosePath => &[],
            };
            for q in pts {
                m = m.max(q.x.abs()).max(q.y.abs());
            }
        }
        m
    }

    #[test]
    fn a_path_that_already_fits_is_returned_untouched() {
        let clip = RenderClip::new(visible());
        let path = Circle::new(Point::new(960.0, 540.0), 200.0).to_path(1e-9);
        let clipped = clip.apply(&path);
        assert_eq!(
            path.elements(),
            clipped.elements(),
            "an in-view path must not be altered"
        );
    }

    /// The Phase 0 failure, now fixed: a shape vastly larger than the viewport.
    #[test]
    fn an_enormous_circle_is_bounded_in_both_size_and_count() {
        let clip = RenderClip::new(visible());
        // Radius 3e14 px is what generation 0 reaches at 1e12x zoom.
        let huge = Circle::new(Point::new(960.0, 540.0), 3e14).to_path(1e-9);

        let clipped = clip.apply(&huge);

        assert!(
            segment_count(&clipped) <= MAX_SEGMENTS,
            "segment count {} exceeded the budget",
            segment_count(&clipped)
        );
        assert!(
            max_coord(&clipped) <= 1e5,
            "coordinates should be bounded by the clip region, saw {}",
            max_coord(&clipped)
        );
    }

    /// The correctness property that matters: what the user can see is
    /// unchanged. Winding number is checked directly, with no rasteriser
    /// involved.
    #[test]
    fn coverage_inside_the_viewport_is_preserved() {
        let clip = RenderClip::new(visible());
        let v = visible();

        let cases: Vec<(&str, BezPath)> = vec![
            (
                "circle enclosing the whole viewport",
                Circle::new(Point::new(960.0, 540.0), 5e9).to_path(1e-9),
            ),
            (
                "circle partly overlapping",
                Circle::new(Point::new(0.0, 0.0), 3e6).to_path(1e-9),
            ),
            (
                "circle fully inside",
                Circle::new(Point::new(700.0, 500.0), 250.0).to_path(1e-9),
            ),
            (
                "huge rectangle covering everything",
                Rect::new(-1e10, -1e10, 1e10, 1e10).to_path(1e-9),
            ),
            (
                "far-off shape that should stay invisible",
                Circle::new(Point::new(9e8, 9e8), 1000.0).to_path(1e-9),
            ),
        ];

        for (name, original) in cases {
            let clipped = clip.apply(&original);

            // Sample a grid strictly inside the visible rect.
            let mut mismatches = 0;
            let mut samples = 0;
            for iy in 1..12 {
                for ix in 1..12 {
                    let p = Point::new(
                        v.x0 + v.width() * (ix as f64 / 12.0),
                        v.y0 + v.height() * (iy as f64 / 12.0),
                    );
                    let before = original.winding(p) != 0;
                    let after = clipped.winding(p) != 0;
                    samples += 1;
                    if before != after {
                        mismatches += 1;
                    }
                }
            }
            assert_eq!(
                mismatches, 0,
                "{name}: coverage changed at {mismatches}/{samples} sampled points"
            );
        }
    }

    #[test]
    fn a_line_crossing_the_view_still_crosses_it() {
        let clip = RenderClip::new(visible());
        let mut path = BezPath::new();
        path.move_to(Point::new(-1e9, 540.0));
        path.line_to(Point::new(1e9, 540.0));

        let clipped = clip.apply(&path);

        assert!(max_coord(&clipped) <= 1e5, "coordinates must be bounded");
        // It must still span the full width of the viewport.
        let bb = clipped.bounding_box();
        assert!(
            bb.x0 <= visible().x0 && bb.x1 >= visible().x1,
            "the clipped line no longer crosses the viewport: {bb:?}"
        );
    }

    #[test]
    fn geometry_outside_the_view_stays_outside() {
        let clip = RenderClip::new(visible());
        let far = Circle::new(Point::new(5e7, 5e7), 100.0).to_path(1e-9);
        let clipped = clip.apply(&far);

        // Whatever it collapses to, it must not intrude on the visible area.
        for iy in 1..8 {
            for ix in 1..8 {
                let p = Point::new(
                    visible().width() * (ix as f64 / 8.0),
                    visible().height() * (iy as f64 / 8.0),
                );
                assert_eq!(
                    clipped.winding(p),
                    0,
                    "a far-off shape leaked into the viewport at {p:?}"
                );
            }
        }
    }

    #[test]
    fn open_subpaths_and_quads_survive_clipping() {
        let clip = RenderClip::new(visible());
        let mut path = BezPath::new();
        path.move_to(Point::new(-1e8, -1e8));
        path.quad_to(Point::new(960.0, 540.0), Point::new(1e8, 1e8));
        path.line_to(Point::new(500.0, 500.0));

        let clipped = clip.apply(&path);
        assert!(max_coord(&clipped) <= 1e5);
        assert!(segment_count(&clipped) > 0, "path was emptied");
    }

    #[test]
    fn degenerate_clip_regions_pass_the_path_through_unchanged() {
        let path = Circle::new(Point::new(0.0, 0.0), 10.0).to_path(1e-9);

        for bad in [
            Rect::new(0.0, 0.0, 0.0, 0.0),
            Rect::new(f64::NAN, 0.0, 1.0, 1.0),
            Rect::new(0.0, 0.0, f64::INFINITY, 1.0),
        ] {
            let clipped = RenderClip::new(bad).apply(&path);
            assert_eq!(
                path.elements(),
                clipped.elements(),
                "a degenerate clip {bad:?} must not corrupt the path"
            );
        }
    }

    #[test]
    fn an_empty_path_clips_to_an_empty_path() {
        let clip = RenderClip::new(visible());
        assert!(clip.apply(&BezPath::new()).elements().is_empty());
    }

    #[test]
    fn clipping_is_idempotent() {
        let clip = RenderClip::new(visible());
        let huge = Circle::new(Point::new(960.0, 540.0), 1e9).to_path(1e-9);
        let once = clip.apply(&huge);
        let twice = clip.apply(&once);
        assert_eq!(
            once.elements(),
            twice.elements(),
            "clipping an already-clipped path should change nothing"
        );
    }

    #[test]
    fn margin_keeps_clamped_points_well_outside_the_visible_area() {
        let clip = RenderClip::new(visible());
        let b = clip.bounds();
        assert!(b.x0 < visible().x0 && b.x1 > visible().x1);
        assert!(b.y0 < visible().y0 && b.y1 > visible().y1);
        // Line crossing at an angle: clamping must not pull it into view.
        let l = Line::new(Point::new(-1e9, -1e9), Point::new(-1e9, 1e9));
        let clipped = clip.apply(&l.to_path(1e-9));
        assert!(clipped.bounding_box().x1 < visible().x0);
    }
}
