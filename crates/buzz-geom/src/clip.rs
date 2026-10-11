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
//! * **Hull does not meet the bounds** → the segment is invisible. Emit its
//!   straight chord through `line_to_clipped`. Connectivity and winding are
//!   preserved, so fills stay correct, but the invisible detail costs a few
//!   segments instead of thousands.
//! * **Otherwise** → subdivide and recurse, down to a depth and budget limit
//!   (the leftover piece is emitted as a clipped chord).
//!
//! Every straight edge (LineTo, the implicit closing edge, collapsed chords)
//! goes through `line_to_clipped`: it is split at its crossings with the four
//! bound lines and each piece's endpoint is clamped. Within one of the 9 regions
//! around the rectangle clamp is affine, so the result is a continuous
//! projection that is the identity inside the bounds; geometry inside the
//! bounds is exact. Every emitted coordinate lies in the bounds, which caps
//! magnitude.
//!
//! # Why clamping is safe
//!
//! Geometry *inside the bounds* is unchanged and winding there is preserved.
//! [`RenderClip`] expands the visible rectangle by a margin, so the only
//! approximation is confined to the region beyond the margin. This is a render-path operation
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
                    self.line_to_clipped(cursor, p, &mut out);
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
                    // The implicit closing edge must be split like any other.
                    if cursor != subpath_start {
                        self.line_to_clipped(cursor, subpath_start, &mut out);
                    }
                    out.close_path();
                    cursor = subpath_start;
                }
            }
        }

        out
    }

    /// Emit the straight edge `from -> to` (both UNclamped) so that the part
    /// inside the bounds is exact.
    ///
    /// The edge is split at its crossings with the four bound lines; each piece
    /// lies in one of the 9 regions around the rectangle, where `clamp` is
    /// affine, so inside pieces are unchanged and outside pieces land on the
    /// border. The overall map is a continuous projection, the identity inside.
    fn line_to_clipped(&self, from: Point, to: Point, out: &mut BezPath) {
        let b = self.bounds;
        let inside = |p: Point| p.x >= b.x0 && p.x <= b.x1 && p.y >= b.y0 && p.y <= b.y1;
        if inside(from) && inside(to) {
            out.line_to(to);
            return;
        }
        // Both ends strictly beyond the same bound line: the whole edge is
        // outside and projects onto the border, so the endpoint suffices.
        if (from.x < b.x0 && to.x < b.x0)
            || (from.x > b.x1 && to.x > b.x1)
            || (from.y < b.y0 && to.y < b.y0)
            || (from.y > b.y1 && to.y > b.y1)
        {
            out.line_to(self.clamp(to));
            return;
        }
        let d = to - from;
        let mut ts = [0.0f64; 4];
        let mut n = 0;
        let mut add = |t: f64| {
            if t > 0.0 && t < 1.0 {
                ts[n] = t;
                n += 1;
            }
        };
        if d.x != 0.0 {
            add((b.x0 - from.x) / d.x);
            add((b.x1 - from.x) / d.x);
        }
        if d.y != 0.0 {
            add((b.y0 - from.y) / d.y);
            add((b.y1 - from.y) / d.y);
        }
        let ts = &mut ts[..n];
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for &t in ts.iter() {
            out.line_to(self.clamp(from + d * t));
        }
        out.line_to(self.clamp(to));
    }

    /// Emit one cubic, subdividing only where it meets the clip bounds.
    fn emit_cubic(&self, c: CubicBez, depth: u8, out: &mut BezPath, budget: &mut usize) {
        let hull = control_hull(c);

        // Wholly invisible: collapse to a single line. Keeping the endpoint
        // (rather than dropping the segment) is what preserves winding, so
        // fills that enclose the viewport still fill it.
        if !hull.overlaps(self.bounds) {
            self.line_to_clipped(c.p0, c.p3, out);
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
        // in which case emit the exact-clipped chord.
        if depth >= MAX_DEPTH || *budget == 0 {
            self.line_to_clipped(c.p0, c.p3, out);
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

    fn dist_to_path(path: &BezPath, p: Point) -> f64 {
        let mut best = f64::INFINITY;
        let mut cur = Point::ORIGIN;
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(q) => cur = q,
                PathEl::LineTo(q) => {
                    let (a, b) = (cur, q);
                    let ab = b - a;
                    let len2 = ab.hypot2();
                    let t = if len2 == 0.0 { 0.0 } else { (((p - a).x * ab.x + (p - a).y * ab.y) / len2).clamp(0.0, 1.0) };
                    best = best.min((a + ab * t - p).hypot());
                    cur = q;
                }
                _ => {}
            }
        }
        best
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
    }

    #[test]
    fn a_long_diagonal_line_is_exact_inside_the_bounds() {
        let clip = RenderClip::new(visible());
        let b = clip.bounds();
        let from = Point::new(-3e6, -1.7e6);
        let to = Point::new(3.0e6 + 1900.0, 1.7e6 + 1100.0);
        let mut path = BezPath::new();
        path.move_to(from);
        path.line_to(to);
        let clipped = clip.apply(&path);
        let mut checked = 0;
        for i in 0..=20_000 {
            let t = i as f64 / 20_000.0;
            let p = from + (to - from) * t;
            if p.x >= b.x0 && p.x <= b.x1 && p.y >= b.y0 && p.y <= b.y1 {
                checked += 1;
                let d = dist_to_path(&clipped, p);
                assert!(d < 1e-9, "point {p:?} is {d} off the clipped path");
            }
        }
        assert!(checked > 0, "the test line never entered the bounds");
    }

    #[test]
    fn winding_inside_the_bounds_is_preserved_for_far_polygons() {
        let clip = RenderClip::new(visible());
        let b = clip.bounds();
        let mut rng = Lcg(0x1234_5678_9abc_def0);
        for _ in 0..300 {
            let n = 3 + (rng.next() * 5.0) as usize;
            let mut path = BezPath::new();
            for k in 0..n {
                let far = if rng.next() < 0.7 { 1e5 } else { 3e3 };
                let p = Point::new((rng.next() - 0.5) * far, (rng.next() - 0.5) * far);
                if k == 0 { path.move_to(p) } else { path.line_to(p) }
            }
            path.close_path();
            let clipped = clip.apply(&path);
            for _ in 0..40 {
                let q = Point::new(
                    b.x0 + rng.next() * b.width(),
                    b.y0 + rng.next() * b.height(),
                );
                assert_eq!(
                    path.winding(q),
                    clipped.winding(q),
                    "winding changed at {q:?} for {path:?}"
                );
            }
        }
    }

    #[test]
    fn the_implicit_closing_edge_is_clipped_exactly() {
        let clip = RenderClip::new(visible());
        let b = clip.bounds();
        // Open-looking triangle whose closing edge crosses the bounds.
        let mut path = BezPath::new();
        path.move_to(Point::new(-1e6, 500.0));
        path.line_to(Point::new(-1e6, 3e6));
        path.line_to(Point::new(1e6, 3e6));
        path.line_to(Point::new(1e6, 700.0));
        path.close_path();
        let clipped = clip.apply(&path);
        let mut rng = Lcg(42);
        for _ in 0..500 {
            let q = Point::new(b.x0 + rng.next() * b.width(), b.y0 + rng.next() * b.height());
            assert_eq!(path.winding(q), clipped.winding(q), "closing edge at {q:?}");
        }
        // Closing edge (1e6,700)->(-1e6,500): its point at x = 960 lies on the output.
        let y = 500.0 + (960.0 + 1e6) / 2e6 * 200.0;
        assert!(dist_to_path(&clipped, Point::new(960.0, y)) < 1e-9);
    }
}
