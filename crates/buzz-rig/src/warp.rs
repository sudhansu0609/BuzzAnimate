//! Puppet warp: Moving Least Squares mesh deformation.
//!
//! This is Animate's Asset Warp tool. The user drops handles on artwork and
//! drags them; everything between follows smoothly, without a skeleton.
//!
//! # Similarity MLS, in complex arithmetic
//!
//! For each point `v` of the artwork, the handles are weighted by `1/d^2α`
//! from `v` and the **best similarity transform** — rotation, uniform scale
//! and translation, no shear — is fitted to them by least squares. That
//! transform is then applied to `v`. Handles near `v` dominate, so the
//! deformation is local; far ones still contribute, so it is smooth.
//!
//! In two dimensions a similarity is exactly multiplication by one complex
//! number, so the least-squares fit has a closed form:
//!
//! ```text
//! c = Σ wᵢ · conj(p̂ᵢ) · q̂ᵢ  /  Σ wᵢ · |p̂ᵢ|²
//! f(v) = q* + c · (v − p*)
//! ```
//!
//! No matrix inversion and no degenerate cases to guard beyond every handle
//! sitting on one spot. Fitting an **affine** transform instead — the other
//! standard choice — needs a 2×2 inverse and lets the artwork shear and
//! stretch unevenly, which reads as melting rather than posing.
//!
//! # Why not just skin it
//!
//! Skinning needs a skeleton, and a skeleton implies joints. Warping is for
//! artwork that has no joints — a cloth, a leaf, a face — where what is wanted
//! is "move this bit and let the rest follow".

use buzz_geom::{BezPath, Point};
use kurbo::ParamCurve as _;
use serde::{Deserialize, Serialize};

/// One handle: where it was placed, and where it has been dragged to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WarpHandle {
    /// Where the handle sits on the undeformed artwork.
    pub rest: Point,
    /// Where the user has moved it.
    pub current: Point,
}

impl WarpHandle {
    /// A handle that has not been moved yet.
    pub fn new(at: Point) -> Self {
        Self {
            rest: at,
            current: at,
        }
    }

    pub fn is_moved(&self) -> bool {
        (self.current - self.rest).hypot() > f64::EPSILON
    }
}

/// How sharply a handle's influence falls off with distance.
///
/// One is the paper's default and behaves like soft clay. Higher makes each
/// handle's neighbourhood stiffer and the falloff faster.
pub const DEFAULT_RIGIDITY: f64 = 1.0;

/// Deform `path` so its handles move from `rest` to `current`.
///
/// Every point of the path is mapped, control points included, so curves stay
/// curves rather than having their ends dragged away from their handles.
pub fn warp_path(path: &BezPath, handles: &[WarpHandle], rigidity: f64) -> BezPath {
    if handles.is_empty() || !handles.iter().any(|h| h.is_moved()) {
        // Nothing has been dragged, so the artwork is exactly as drawn. Worth
        // short-circuiting: this is the common case while the tool is simply
        // selected, and it makes "handles placed" cost nothing.
        return path.clone();
    }

    let alpha = rigidity.max(0.05);
    crate::skin::map_path_points(path, |v| warp_point(v, handles, alpha))
}

/// Where one point lands.
pub fn warp_point(v: Point, handles: &[WarpHandle], alpha: f64) -> Point {
    // Sitting exactly on a handle: that handle wins outright. Without this the
    // weight is an infinity and the whole sum becomes NaN.
    for handle in handles {
        if (v - handle.rest).hypot() < 1e-9 {
            return handle.current;
        }
    }

    let mut total = 0.0;
    let mut p_star = (0.0, 0.0);
    let mut q_star = (0.0, 0.0);
    let mut weights = Vec::with_capacity(handles.len());

    for handle in handles {
        let distance2 = (v - handle.rest).hypot2();
        let w = 1.0 / distance2.powf(alpha);
        weights.push(w);
        total += w;
        p_star.0 += handle.rest.x * w;
        p_star.1 += handle.rest.y * w;
        q_star.0 += handle.current.x * w;
        q_star.1 += handle.current.y * w;
    }

    if total <= 0.0 || !total.is_finite() {
        return v;
    }
    let p_star = Point::new(p_star.0 / total, p_star.1 / total);
    let q_star = Point::new(q_star.0 / total, q_star.1 / total);

    // c = Σ w conj(p̂) q̂ / Σ w |p̂|², the least-squares similarity.
    let (mut num_re, mut num_im, mut denominator) = (0.0, 0.0, 0.0);
    for (handle, w) in handles.iter().zip(&weights) {
        let px = handle.rest.x - p_star.x;
        let py = handle.rest.y - p_star.y;
        let qx = handle.current.x - q_star.x;
        let qy = handle.current.y - q_star.y;

        // conj(p) * q
        num_re += w * (px * qx + py * qy);
        num_im += w * (px * qy - py * qx);
        denominator += w * (px * px + py * py);
    }

    if denominator <= 1e-12 {
        // Every handle at one place: there is no rotation or scale to fit, so
        // this is a pure translation, which is exactly what one dragged handle
        // should do.
        let shift = q_star - p_star;
        return v + shift;
    }

    let c_re = num_re / denominator;
    let c_im = num_im / denominator;

    let dx = v.x - p_star.x;
    let dy = v.y - p_star.y;
    Point::new(
        q_star.x + c_re * dx - c_im * dy,
        q_star.y + c_im * dx + c_re * dy,
    )
}

/// Split every segment of a path into `parts`, without changing its shape.
///
/// # Why a warp needs this
///
/// The deformation moves **points**, and a rectangle has four. Dragging a
/// handle in the middle of one therefore does nothing at all: there is no
/// geometry between the corners for the warp to act on, so the outline stays
/// perfectly straight while the handle moves away from it. Any path drawn with
/// few points — a rectangle, a triangle, a straight-sided limb — behaves the
/// same way, which reads as the tool being broken.
///
/// Subdividing gives the warp something to bend. It is **exact**: each piece
/// is a sub-segment of the original curve through de Casteljau, so the path
/// before any handle is dragged is geometrically identical to the one drawn.
/// The cost is more points to carry, which is why this happens once, when the
/// artwork becomes warpable, rather than on every frame.
pub fn subdivide_path(path: &BezPath, parts: usize) -> BezPath {
    let parts = parts.max(1);
    if parts == 1 {
        return path.clone();
    }

    let mut out = BezPath::new();
    let mut started = false;

    for segment in path.segments() {
        if !started {
            out.move_to(segment_start(&segment));
            started = true;
        }
        for i in 0..parts {
            let t0 = i as f64 / parts as f64;
            let t1 = (i + 1) as f64 / parts as f64;
            match segment.subsegment(t0..t1) {
                kurbo::PathSeg::Line(line) => out.line_to(line.p1),
                kurbo::PathSeg::Quad(quad) => out.quad_to(quad.p1, quad.p2),
                kurbo::PathSeg::Cubic(cubic) => out.curve_to(cubic.p1, cubic.p2, cubic.p3),
            }
        }
    }

    // `segments()` closes a closed path with a line back to the start, so the
    // outline is already complete; the marker keeps it a filled region rather
    // than an open one.
    if path
        .elements()
        .iter()
        .any(|e| matches!(e, kurbo::PathEl::ClosePath))
    {
        out.close_path();
    }
    out
}

fn segment_start(segment: &kurbo::PathSeg) -> Point {
    match segment {
        kurbo::PathSeg::Line(l) => l.p0,
        kurbo::PathSeg::Quad(q) => q.p0,
        kurbo::PathSeg::Cubic(c) => c.p0,
    }
}

/// How finely artwork is divided when it becomes warpable.
///
/// Eight pieces per segment is enough for a rectangle to bend smoothly and
/// still leaves a 100-segment drawing under a thousand points.
pub const WARP_SUBDIVISION: usize = 8;

/// Handles laid out over a shape's bounding box, for "add handles" in the UI.
///
/// A grid rather than the outline: warping is driven by handles *inside* the
/// artwork as well as around it, and a ring of edge handles leaves the middle
/// with nothing holding it.
pub fn grid_handles(bounds: buzz_geom::Rect, columns: usize, rows: usize) -> Vec<WarpHandle> {
    let columns = columns.max(2);
    let rows = rows.max(2);
    let mut out = Vec::with_capacity(columns * rows);

    for row in 0..rows {
        for column in 0..columns {
            let u = column as f64 / (columns - 1) as f64;
            let v = row as f64 / (rows - 1) as f64;
            out.push(WarpHandle::new(Point::new(
                bounds.x0 + bounds.width() * u,
                bounds.y0 + bounds.height() * v,
            )));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _, Vec2};

    fn square() -> BezPath {
        Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9)
    }

    fn handles() -> Vec<WarpHandle> {
        vec![
            WarpHandle::new(Point::new(0.0, 0.0)),
            WarpHandle::new(Point::new(100.0, 0.0)),
            WarpHandle::new(Point::new(100.0, 100.0)),
            WarpHandle::new(Point::new(0.0, 100.0)),
        ]
    }

    #[test]
    fn untouched_handles_leave_the_artwork_alone() {
        let path = square();
        let warped = warp_path(&path, &handles(), DEFAULT_RIGIDITY);
        assert_eq!(warped.to_svg(), path.to_svg());
    }

    #[test]
    fn a_handle_lands_exactly_where_it_was_dragged() {
        let mut handles = handles();
        handles[0].current = Point::new(-30.0, -20.0);

        let moved = warp_point(handles[0].rest, &handles, DEFAULT_RIGIDITY);
        assert!(
            (moved - Point::new(-30.0, -20.0)).hypot() < 1e-9,
            "the handle did not land on its own target: {moved:?}"
        );
    }

    /// Points far from the dragged handle should barely move: the deformation
    /// has to be local, or dragging an ear moves the whole head.
    #[test]
    fn the_deformation_is_local() {
        let mut handles = handles();
        handles[0].current = Point::new(-40.0, -40.0);

        let near = warp_point(Point::new(10.0, 10.0), &handles, DEFAULT_RIGIDITY);
        let far = warp_point(Point::new(100.0, 100.0), &handles, DEFAULT_RIGIDITY);

        let near_moved = (near - Point::new(10.0, 10.0)).hypot();
        let far_moved = (far - Point::new(100.0, 100.0)).hypot();
        assert!(
            near_moved > far_moved * 3.0,
            "near moved {near_moved}, far moved {far_moved}"
        );
    }

    /// One handle dragged is a pure translation, which is the only sensible
    /// answer: one point cannot describe a rotation.
    #[test]
    fn a_single_handle_translates_everything() {
        let handles = vec![WarpHandle {
            rest: Point::new(50.0, 50.0),
            current: Point::new(70.0, 90.0),
        }];
        let shift = Vec2::new(20.0, 40.0);

        for probe in [Point::ZERO, Point::new(100.0, 0.0), Point::new(-40.0, 25.0)] {
            let moved = warp_point(probe, &handles, DEFAULT_RIGIDITY);
            assert!(
                (moved - (probe + shift)).hypot() < 1e-9,
                "{probe:?} moved to {moved:?}"
            );
        }
    }

    /// Moving every handle by the same amount must move the artwork by that
    /// amount and not deform it at all — the test that catches a normalisation
    /// mistake immediately.
    #[test]
    fn translating_every_handle_translates_the_artwork_rigidly() {
        let shift = Vec2::new(15.0, -25.0);
        let handles: Vec<WarpHandle> = handles()
            .into_iter()
            .map(|h| WarpHandle {
                rest: h.rest,
                current: h.rest + shift,
            })
            .collect();

        for probe in [
            Point::new(50.0, 50.0),
            Point::new(0.0, 0.0),
            Point::new(140.0, -60.0),
        ] {
            let moved = warp_point(probe, &handles, DEFAULT_RIGIDITY);
            assert!(
                (moved - (probe + shift)).hypot() < 1e-6,
                "{probe:?} should have moved rigidly, went to {moved:?}"
            );
        }
    }

    /// Rotating every handle about the centre must rotate the artwork — and
    /// *not* scale it, which is what "similarity rather than affine" buys.
    #[test]
    fn rotating_every_handle_rotates_the_artwork_without_scaling_it() {
        let centre = Point::new(50.0, 50.0);
        let quarter = std::f64::consts::FRAC_PI_2;
        let rotate = |p: Point| {
            let d = p - centre;
            centre + Vec2::new(
                d.x * quarter.cos() - d.y * quarter.sin(),
                d.x * quarter.sin() + d.y * quarter.cos(),
            )
        };
        let handles: Vec<WarpHandle> = handles()
            .into_iter()
            .map(|h| WarpHandle {
                rest: h.rest,
                current: rotate(h.rest),
            })
            .collect();

        for probe in [Point::new(20.0, 30.0), Point::new(80.0, 10.0)] {
            let moved = warp_point(probe, &handles, DEFAULT_RIGIDITY);
            assert!(
                (moved - rotate(probe)).hypot() < 1e-6,
                "{probe:?} should have rotated to {:?}, went to {moved:?}",
                rotate(probe)
            );
            // Distance from the centre is preserved: no scaling crept in.
            assert!(
                ((moved - centre).hypot() - (probe - centre).hypot()).abs() < 1e-6,
                "the artwork changed size"
            );
        }
    }

    #[test]
    fn warping_keeps_the_path_structure() {
        let mut handles = handles();
        handles[2].current = Point::new(160.0, 130.0);

        let path = square();
        let warped = warp_path(&path, &handles, DEFAULT_RIGIDITY);

        assert_eq!(warped.elements().len(), path.elements().len());
        assert_ne!(warped.to_svg(), path.to_svg(), "it should have moved");
        assert!(
            warped.bounding_box().width().is_finite(),
            "the warp produced non-finite geometry"
        );
    }

    #[test]
    fn a_grid_of_handles_covers_the_bounds_including_the_middle() {
        let grid = grid_handles(Rect::new(0.0, 0.0, 100.0, 50.0), 3, 3);
        assert_eq!(grid.len(), 9);
        assert!(grid.iter().any(|h| h.rest == Point::new(50.0, 25.0)), "a middle handle");
        assert!(grid.iter().any(|h| h.rest == Point::new(0.0, 0.0)));
        assert!(grid.iter().any(|h| h.rest == Point::new(100.0, 50.0)));

        // Degenerate requests still produce a usable grid rather than an empty
        // one or a division by zero.
        assert_eq!(grid_handles(Rect::new(0.0, 0.0, 10.0, 10.0), 0, 1).len(), 4);
    }

    /// Subdivision must not change the shape at all — only how many points it
    /// is made of.
    #[test]
    fn subdividing_preserves_the_outline_exactly() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(100.0, 0.0));
        path.curve_to(
            Point::new(140.0, 30.0),
            Point::new(140.0, 70.0),
            Point::new(100.0, 100.0),
        );
        path.close_path();

        let fine = subdivide_path(&path, 8);
        assert!(
            fine.elements().len() > path.elements().len() * 4,
            "it should have many more points"
        );

        // Same shape, same place: the outline and the area it encloses are
        // what a subdivision must leave alone.
        let before = path.bounding_box();
        let after = fine.bounding_box();
        assert!((before.x0 - after.x0).abs() < 1e-9);
        assert!((before.x1 - after.x1).abs() < 1e-9);
        assert!((before.y0 - after.y0).abs() < 1e-9);
        assert!((before.y1 - after.y1).abs() < 1e-9);
        assert!(
            (path.area() - fine.area()).abs() < 1e-6,
            "the enclosed area changed: {} vs {}",
            path.area(),
            fine.area()
        );
    }

    /// The defect subdivision exists to prevent: a four-point rectangle cannot
    /// bend, because there is nothing between its corners to move.
    #[test]
    fn a_rectangle_can_bend_once_it_is_subdivided() {
        let rect = Rect::new(0.0, 0.0, 100.0, 20.0).to_path(1e-9);
        let handles = vec![
            WarpHandle::new(Point::new(0.0, 10.0)),
            WarpHandle::new(Point::new(100.0, 10.0)),
            WarpHandle {
                rest: Point::new(50.0, 10.0),
                current: Point::new(50.0, 80.0),
            },
        ];

        let flat = warp_path(&rect, &handles, DEFAULT_RIGIDITY);
        let subdivided = warp_path(&subdivide_path(&rect, 8), &handles, DEFAULT_RIGIDITY);

        assert!(
            subdivided.bounding_box().y1 > flat.bounding_box().y1 + 10.0,
            "subdividing should let the middle follow the handle: {:?} vs {:?}",
            flat.bounding_box(),
            subdivided.bounding_box()
        );
    }

    #[test]
    fn no_handles_means_no_change() {
        let path = square();
        assert_eq!(warp_path(&path, &[], DEFAULT_RIGIDITY).to_svg(), path.to_svg());
    }
}
