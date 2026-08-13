//! Perspective, as a 3×3 homography.
//!
//! # Why an affine is not enough
//!
//! Everything in this renderer has been an [`Affine`] until now, and for good
//! reason: an affine is exact, cheap, closed under composition, and it maps a
//! Bézier to a Bézier by moving its control points. Layer depth lives happily
//! inside one — a layer twice as far away is drawn at half scale, which is a
//! scale factor and nothing more.
//!
//! A camera that **tilts** is not. Pitch a camera down over a flat layer and
//! its far edge is further away than its near edge, so the rectangle becomes a
//! *trapezoid*: parallel lines converge. No affine does that, because an affine
//! preserves parallelism by definition.
//!
//! What does do it is a **homography** — a 3×3 matrix acting on homogeneous
//! coordinates with a divide at the end:
//!
//! ```text
//! [x']   [a b c] [x]                 x' / w'
//! [y'] = [d e f] [y]      screen  =  y' / w'
//! [w']   [g h i] [1]
//! ```
//!
//! With `g = h = 0` and `i = 1` the divide does nothing and this *is* an
//! affine, which is why it can replace one everywhere without changing a
//! single pixel of a document that never tilts. That property is load-bearing:
//! it is what makes this safe to put in the render path.
//!
//! # Why a plane is all we need
//!
//! A general 3D scene needs a 4×4 matrix and a depth buffer. This is not a
//! general 3D scene: every layer is **flat**, at one depth, and the perspective
//! image of a plane is exactly a homography of that plane. So one 3×3 per layer
//! describes the whole thing — no depth buffer, no z-fighting, no sorting
//! beyond the layer order the timeline already gives.
//!
//! # What it costs
//!
//! A projective map sends straight lines to straight lines but a *cubic* to a
//! rational cubic, which kurbo has no representation for. So [`Projection::map_path`]
//! flattens first and maps the points. Straight edges stay exactly straight —
//! the property that matters for a tilted rectangle — and curves are subdivided
//! finely enough that the error stays under the tolerance asked for.

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as _, Vec2};

/// A perspective transform of a plane.
///
/// Row-major, acting on `(x, y, 1)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    m: [f64; 9],
}

/// How close to the camera plane a point may come before it counts as behind
/// it.
///
/// The divide blows up at zero; a hair in front, a point is magnified by
/// thousands and swamps the frame. Everything at or beyond this is simply not
/// drawn, which is what [`crate::Camera`]'s near plane does for depth and for
/// the same reason.
const NEAR_W: f64 = 1e-6;

impl Default for Projection {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Projection {
    pub const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    /// Build one from its nine coefficients, row-major.
    pub fn new(m: [f64; 9]) -> Self {
        Self { m }
    }

    pub fn coeffs(&self) -> [f64; 9] {
        self.m
    }

    /// Every affine is a homography; this is the one it is.
    pub fn from_affine(a: Affine) -> Self {
        let [xx, yx, xy, yy, x0, y0] = a.as_coeffs();
        Self {
            m: [xx, xy, x0, yx, yy, y0, 0.0, 0.0, 1.0],
        }
    }

    /// Divide through by the bottom-right term.
    ///
    /// A homography is only defined **up to scale** — `[1000,0,0, 0,1000,0,
    /// 0,0,1000]` is the identity, and reading its bottom row literally says
    /// otherwise. Normalising keeps that from mattering anywhere else, and
    /// keeps the numbers near unity, which is worth having in a matrix that is
    /// about to be inverted.
    fn normalised(self) -> Self {
        let w = self.m[8];
        if w == 0.0 || w == 1.0 || !w.is_finite() {
            return self;
        }
        let mut m = self.m;
        for value in &mut m {
            *value /= w;
        }
        Self { m }
    }

    /// Is the divide a no-op, so this is really an affine?
    ///
    /// The whole render path takes a faster, exactly-as-before route when this
    /// is true, which is every document that does not tilt its camera.
    pub fn is_affine(&self) -> bool {
        self.m[6] == 0.0 && self.m[7] == 0.0 && self.m[8] != 0.0
    }

    /// The affine this is, if it is one.
    pub fn as_affine(&self) -> Option<Affine> {
        if !self.is_affine() {
            return None;
        }
        let w = self.m[8];
        Some(Affine::new([
            self.m[0] / w,
            self.m[3] / w,
            self.m[1] / w,
            self.m[4] / w,
            self.m[2] / w,
            self.m[5] / w,
        ]))
    }

    /// `other` after `self`.
    pub fn then(&self, other: &Self) -> Self {
        let (a, b) = (&other.m, &self.m);
        let mut m = [0.0; 9];
        for row in 0..3 {
            for col in 0..3 {
                m[row * 3 + col] = (0..3).map(|k| a[row * 3 + k] * b[k * 3 + col]).sum();
            }
        }
        Self { m }.normalised()
    }

    /// An affine applied after this.
    pub fn then_affine(&self, a: Affine) -> Self {
        self.then(&Self::from_affine(a))
    }

    /// An affine applied before this — the usual case, since an object's own
    /// placement is affine and happens in the plane.
    pub fn pre_affine(&self, a: Affine) -> Self {
        Self::from_affine(a).then(self)
    }

    /// Where a point lands, or `None` if it is at or behind the camera.
    pub fn map_point(&self, p: Point) -> Option<Point> {
        let m = &self.m;
        let w = m[6] * p.x + m[7] * p.y + m[8];
        if w <= NEAR_W {
            return None;
        }
        Some(Point::new(
            (m[0] * p.x + m[1] * p.y + m[2]) / w,
            (m[3] * p.x + m[4] * p.y + m[5]) / w,
        ))
    }

    /// The homogeneous denominator at a point: positive in front of the camera,
    /// zero on the horizon, negative behind.
    pub fn depth_at(&self, p: Point) -> f64 {
        self.m[6] * p.x + self.m[7] * p.y + self.m[8]
    }

    /// How much this magnifies at a point — the square root of the Jacobian's
    /// determinant.
    ///
    /// Used to pick a flattening tolerance: the near edge of a steeply tilted
    /// plane is magnified, and a curve flattened at the source tolerance would
    /// come out visibly faceted there.
    pub fn scale_at(&self, p: Point) -> f64 {
        let m = &self.m;
        let w = self.depth_at(p);
        if w <= NEAR_W {
            return f64::INFINITY;
        }
        // Rows of the Jacobian of (u/w, v/w).
        let u = m[0] * p.x + m[1] * p.y + m[2];
        let v = m[3] * p.x + m[4] * p.y + m[5];
        let dux = (m[0] * w - u * m[6]) / (w * w);
        let duy = (m[1] * w - u * m[7]) / (w * w);
        let dvx = (m[3] * w - v * m[6]) / (w * w);
        let dvy = (m[4] * w - v * m[7]) / (w * w);
        (dux * dvy - duy * dvx).abs().sqrt()
    }

    /// The inverse, or `None` if this is singular — a plane seen exactly
    /// edge-on, which projects to a line and cannot be un-projected.
    pub fn inverse(&self) -> Option<Self> {
        let m = &self.m;
        let c = [
            m[4] * m[8] - m[5] * m[7],
            m[2] * m[7] - m[1] * m[8],
            m[1] * m[5] - m[2] * m[4],
            m[5] * m[6] - m[3] * m[8],
            m[0] * m[8] - m[2] * m[6],
            m[2] * m[3] - m[0] * m[5],
            m[3] * m[7] - m[4] * m[6],
            m[1] * m[6] - m[0] * m[7],
            m[0] * m[4] - m[1] * m[3],
        ];
        let determinant = m[0] * c[0] + m[1] * c[3] + m[2] * c[6];
        if determinant.abs() < 1e-12 {
            return None;
        }
        let mut inv = [0.0; 9];
        for (i, value) in c.iter().enumerate() {
            inv[i] = value / determinant;
        }
        Some(Self { m: inv })
    }

    /// Map a path.
    ///
    /// Affine transforms are applied exactly, control point by control point.
    /// Anything else is flattened to line segments first — a projective map
    /// turns a cubic into a rational cubic, which there is no way to store —
    /// and the parts of the path behind the camera are clipped away rather
    /// than being folded back into the picture inside out.
    ///
    /// `tolerance` is the greatest allowed deviation **in the output**.
    pub fn map_path(&self, path: &BezPath, tolerance: f64) -> BezPath {
        if let Some(affine) = self.as_affine() {
            return affine * path.clone();
        }
        if path.elements().is_empty() {
            return BezPath::new();
        }

        // Flatten in *source* units, tight enough that magnification on the
        // near side of the plane does not turn a curve into a polygon.
        let bounds = path.bounding_box();
        let magnify = self.worst_scale(bounds);
        let source_tolerance = if magnify.is_finite() && magnify > 0.0 {
            (tolerance / magnify).max(1e-9)
        } else {
            tolerance.max(1e-9)
        };

        let mut out = BezPath::new();
        // The last point *in source space*, and whether it was in front.
        let mut cursor: Option<Point> = None;
        let mut start: Option<Point> = None;
        let mut pen_down = false;

        let emit = |from: Option<Point>, to: Point, out: &mut BezPath, pen_down: &mut bool| {
            self.emit_segment(from, to, out, pen_down);
        };

        kurbo::flatten(path.iter(), source_tolerance, |el| match el {
            PathEl::MoveTo(p) => {
                cursor = Some(p);
                start = Some(p);
                pen_down = false;
            }
            PathEl::LineTo(p) => {
                emit(cursor, p, &mut out, &mut pen_down);
                cursor = Some(p);
            }
            PathEl::ClosePath => {
                if let (Some(from), Some(to)) = (cursor, start) {
                    emit(Some(from), to, &mut out, &mut pen_down);
                    cursor = Some(to);
                }
                if pen_down {
                    out.close_path();
                    pen_down = false;
                }
            }
            // `flatten` yields only these three.
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
        });

        out
    }

    /// One flattened segment, clipped at the horizon.
    fn emit_segment(&self, from: Option<Point>, to: Point, out: &mut BezPath, pen_down: &mut bool) {
        let Some(from) = from else {
            // A `LineTo` with nothing before it: treat the point as a start.
            if let Some(mapped) = self.map_point(to) {
                out.move_to(mapped);
                *pen_down = true;
            }
            return;
        };

        let (wa, wb) = (self.depth_at(from), self.depth_at(to));
        let (a_in, b_in) = (wa > NEAR_W, wb > NEAR_W);

        match (a_in, b_in) {
            (false, false) => {
                // Wholly behind the camera: nothing to draw, and the pen lifts
                // so the next visible piece starts a fresh subpath rather than
                // being joined across the horizon.
                *pen_down = false;
            }
            (true, true) => {
                self.line_to(from, to, out, pen_down);
            }
            (true, false) => {
                let cut = crossing(from, to, wa, wb);
                self.line_to(from, cut, out, pen_down);
                *pen_down = false;
            }
            (false, true) => {
                let cut = crossing(from, to, wa, wb);
                *pen_down = false;
                self.line_to(cut, to, out, pen_down);
            }
        }
    }

    fn line_to(&self, from: Point, to: Point, out: &mut BezPath, pen_down: &mut bool) {
        let (Some(a), Some(b)) = (self.map_point(from), self.map_point(to)) else {
            *pen_down = false;
            return;
        };
        if !*pen_down {
            out.move_to(a);
            *pen_down = true;
        }
        out.line_to(b);
    }

    /// The greatest magnification anywhere on a rectangle, judged at its
    /// corners and centre. An estimate, deliberately: the exact worst case
    /// needs an optimisation, and this is being used to choose how finely to
    /// flatten a curve.
    fn worst_scale(&self, bounds: Rect) -> f64 {
        [
            Point::new(bounds.x0, bounds.y0),
            Point::new(bounds.x1, bounds.y0),
            Point::new(bounds.x0, bounds.y1),
            Point::new(bounds.x1, bounds.y1),
            bounds.center(),
        ]
        .iter()
        .map(|p| self.scale_at(*p))
        .filter(|s| s.is_finite())
        .fold(1.0f64, f64::max)
    }

    /// Where the four corners of a rectangle land, for anything that needs the
    /// shape rather than a bounding box — a gizmo, a hit-test hull.
    ///
    /// `None` if any corner is behind the camera, because a quadrilateral with
    /// a corner behind the viewer is not a quadrilateral.
    pub fn map_rect(&self, rect: Rect) -> Option<[Point; 4]> {
        Some([
            self.map_point(Point::new(rect.x0, rect.y0))?,
            self.map_point(Point::new(rect.x1, rect.y0))?,
            self.map_point(Point::new(rect.x1, rect.y1))?,
            self.map_point(Point::new(rect.x0, rect.y1))?,
        ])
    }

    /// The axis-aligned box the mapped rectangle occupies.
    pub fn map_rect_bounds(&self, rect: Rect) -> Option<Rect> {
        let corners = self.map_rect(rect)?;
        let mut bounds = Rect::from_points(corners[0], corners[1]);
        for corner in &corners[2..] {
            bounds = bounds.union_pt(*corner);
        }
        Some(bounds)
    }

    /// A camera looking at **any plane in space**.
    ///
    /// The general form, and the one everything else here is written in terms
    /// of. A plane is an origin and two basis vectors: the point at plane
    /// coordinates `(s, t)` is `origin + s·ex + t·ey`. `to_origin` is the
    /// vector from the camera to that origin.
    ///
    /// Why bother with the general case: a **layer** is a plane facing the
    /// camera, and an **object rotated in space** is a plane that is not. Both
    /// are one matrix, built the same way, and neither needs a depth buffer —
    /// the perspective image of a plane is a homography of that plane, whatever
    /// angle it sits at.
    ///
    /// `pitch` and `yaw` are the camera's, and `focal` is its lens.
    pub fn plane_in_view(
        ex: [f64; 3],
        ey: [f64; 3],
        to_origin: [f64; 3],
        focal: f64,
        pitch: f64,
        yaw: f64,
    ) -> Option<Self> {
        if !focal.is_finite() || focal <= 0.0 {
            return None;
        }
        for v in [&ex, &ey, &to_origin] {
            if !v.iter().all(|c| c.is_finite()) {
                return None;
            }
        }

        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();

        // The camera's axes in world terms — the rows of its rotation
        // transposed, which is what carries a world offset into its frame.
        let right = [cy, 0.0, -sy];
        let up = [sy * sp, cp, cy * sp];
        let forward = [sy * cp, -sp, cy * cp];

        let dot = |a: &[f64; 3], b: &[f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

        let m = [
            focal * dot(&right, &ex),
            focal * dot(&right, &ey),
            focal * dot(&right, &to_origin),
            focal * dot(&up, &ex),
            focal * dot(&up, &ey),
            focal * dot(&up, &to_origin),
            dot(&forward, &ex),
            dot(&forward, &ey),
            dot(&forward, &to_origin),
        ];
        // Exactly edge-on, the plane projects to a line and there is nothing
        // to draw. **Past** edge-on is allowed: that is the back of the plane,
        // and a card turned right round should show its back — mirrored, which
        // is what the negative determinant gives — rather than vanish.
        let normal = [
            ex[1] * ey[2] - ex[2] * ey[1],
            ex[2] * ey[0] - ex[0] * ey[2],
            ex[0] * ey[1] - ex[1] * ey[0],
        ];
        if dot(&forward, &normal).abs() <= NEAR_W {
            return None;
        }

        let projection = Self { m };
        // And the plane's origin has to be in front of the camera at all.
        (projection.depth_at(Point::ZERO) > NEAR_W).then(|| projection.normalised())
    }

    /// A camera **looking at** a plane from `distance` away.
    ///
    /// `pitch` tilts it down, `yaw` turns it to the right, and `focal` is the
    /// lens. All in the plane's own units, with the origin at the point the
    /// camera is pointed at.
    ///
    /// # The camera orbits its target; it does not swivel in place
    ///
    /// These are two different cameras and the difference is the whole feel of
    /// the tool. A camera that *swivels* keeps its position and turns, so the
    /// thing it was looking at slides out of frame: tilt, and the shot leaves.
    /// A camera that *orbits* stays pointed at its target and moves round it,
    /// so tilting tips the plane away while what matters stays in the middle
    /// of the frame — which is what an animator means by "tilt the camera",
    /// and what Blender's viewport does.
    ///
    /// The first version of this swivelled, and the artwork disappeared off
    /// the top of every test frame. Building the camera *behind* its target —
    /// at `-distance` along its own view axis — is the whole fix, and it makes
    /// the arithmetic simpler as well: the vector from camera to a plane point
    /// is `(x, y, 0) + distance · forward`, and carrying that into the
    /// camera's frame leaves `distance` alone on the depth row.
    ///
    /// With no tilt it reduces exactly to a scale of `focal / distance`, which
    /// is the factor layer depth has always used — so this generalises what
    /// was there rather than replacing it.
    pub fn look_at_plane(distance: f64, focal: f64, pitch: f64, yaw: f64) -> Option<Self> {
        if !(distance.is_finite() && focal.is_finite()) || distance <= NEAR_W || focal <= 0.0 {
            return None;
        }

        // The layer's plane faces the camera, so its basis is the plain one,
        // and the camera sits `distance` behind its target along its own view
        // axis — which is what makes it orbit rather than swivel.
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let forward = [sy * cp, -sp, cy * cp];
        let to_origin = [
            distance * forward[0],
            distance * forward[1],
            distance * forward[2],
        ];

        // A **layer** must face the camera. Past edge-on means the camera has
        // swung round behind the artwork, which for a whole layer is the shot
        // going wrong rather than an effect anybody asked for — unlike a single
        // rotated card, which is allowed to show its back.
        if forward[2] <= NEAR_W {
            return None;
        }

        Self::plane_in_view([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], to_origin, focal, pitch, yaw)
    }

    /// The basis of a plane rotated about its own axes.
    ///
    /// `x` tips it away from the viewer, `y` turns it about the vertical, `z`
    /// spins it in its own plane. Applied in that order — the order Animate's
    /// 3D Rotation tool uses, so a value copied from there means the same
    /// thing here.
    pub fn rotated_basis(x: f64, y: f64, z: f64) -> ([f64; 3], [f64; 3]) {
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();

        // R = Ry · Rx · Rz, applied to the plane's own axes.
        let column = |a: f64, b: f64, c: f64| -> [f64; 3] {
            // Rz first.
            let (a, b, c) = (a, b, c);
            // Rx about the horizontal.
            let (a, b, c) = (a, b * cx - c * sx, b * sx + c * cx);
            // Ry about the vertical.
            (a * cy + c * sy, b, -a * sy + c * cy).into()
        };

        (column(cz, sz, 0.0), column(-sz, cz, 0.0))
    }
}

impl std::ops::Mul<Point> for Projection {
    type Output = Option<Point>;

    fn mul(self, p: Point) -> Option<Point> {
        self.map_point(p)
    }
}

/// Where the segment from `a` to `b` crosses the horizon, pulled just in front
/// of it so the mapped point is finite.
fn crossing(a: Point, b: Point, wa: f64, wb: f64) -> Point {
    let span = wa - wb;
    let t = if span.abs() < f64::EPSILON {
        0.5
    } else {
        ((wa - NEAR_W) / span).clamp(0.0, 1.0)
    };
    a + (b - a) * t
}

/// Convenience: a `Vec2` from the origin to a point.
pub fn to_vec(p: Point) -> Vec2 {
    p.to_vec2()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> BezPath {
        Rect::new(-50.0, -50.0, 50.0, 50.0).to_path(1e-9)
    }

    /// The promise the whole render path depends on: with no tilt, this is an
    /// affine, and it is applied as one.
    #[test]
    fn an_untilted_camera_is_exactly_an_affine() {
        let projection = Projection::look_at_plane(1000.0, 1000.0, 0.0, 0.0).expect("in front");
        assert!(projection.is_affine(), "{projection:?}");

        let affine = projection.as_affine().expect("affine");
        // Focal 1000 at distance 1000 is unit scale.
        assert!((affine.as_coeffs()[0] - 1.0).abs() < 1e-12);
        assert!((affine.as_coeffs()[3] - 1.0).abs() < 1e-12);
    }

    /// And it reduces to exactly the depth scale that layer depth has always
    /// used, or a document with depth would shift the day this landed.
    #[test]
    fn it_reproduces_the_depth_scale() {
        for depth in [0.0, 250.0, 1000.0, 4000.0] {
            let focal = 1000.0;
            let projection =
                Projection::look_at_plane(focal + depth, focal, 0.0, 0.0).expect("in front");
            let scale = projection.as_affine().unwrap().as_coeffs()[0];
            assert!(
                (scale - focal / (focal + depth)).abs() < 1e-12,
                "depth {depth} gave {scale}"
            );
        }
    }

    #[test]
    fn an_affine_round_trips() {
        let affine = Affine::translate((10.0, -4.0)) * Affine::rotate(0.7) * Affine::scale(2.0);
        let projection = Projection::from_affine(affine);
        assert_eq!(projection.as_affine().unwrap().as_coeffs(), affine.as_coeffs());

        let p = Point::new(3.0, 5.0);
        let by_affine = affine * p;
        let by_projection = projection.map_point(p).unwrap();
        assert!((by_affine - by_projection).hypot() < 1e-12);
    }

    /// The whole point of the exercise: a tilted camera turns a rectangle into
    /// a trapezoid, with the far edge shorter than the near one.
    #[test]
    fn pitching_the_camera_makes_a_trapezoid() {
        let projection =
            Projection::look_at_plane(1000.0, 1000.0, 0.5, 0.0).expect("in front");
        assert!(!projection.is_affine(), "a tilt must not stay affine");

        let corners = projection.map_rect(Rect::new(-100.0, -100.0, 100.0, 100.0)).unwrap();
        let top = (corners[1] - corners[0]).hypot();
        let bottom = (corners[2] - corners[3]).hypot();
        assert!(
            (top - bottom).abs() > 1.0,
            "the two edges should differ: {top} vs {bottom}"
        );
    }

    /// Straight stays straight — the property that makes a trapezoid a
    /// trapezoid rather than a bulge.
    #[test]
    fn straight_lines_stay_straight() {
        let projection = Projection::look_at_plane(800.0, 1000.0, 0.6, 0.3).expect("in front");

        let a = Point::new(-80.0, -40.0);
        let b = Point::new(90.0, 60.0);
        let (ma, mb) = (
            projection.map_point(a).unwrap(),
            projection.map_point(b).unwrap(),
        );

        for t in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let on_line = a + (b - a) * t;
            let mapped = projection.map_point(on_line).unwrap();
            // Distance from the mapped point to the line through the mapped
            // ends. Zero, up to arithmetic.
            let along = mb - ma;
            let offset = mapped - ma;
            let area = (along.x * offset.y - along.y * offset.x).abs();
            let distance = area / along.hypot();
            assert!(distance < 1e-6, "t={t} bent by {distance}");
        }
    }

    #[test]
    fn inverting_a_projection_undoes_it() {
        let projection = Projection::look_at_plane(900.0, 1200.0, -0.4, 0.25).expect("in front");
        let back = projection.inverse().expect("invertible");

        for p in [
            Point::new(0.0, 0.0),
            Point::new(120.0, -60.0),
            Point::new(-200.0, 180.0),
        ] {
            let there = projection.map_point(p).expect("in front");
            let again = back.map_point(there).expect("in front");
            assert!((again - p).hypot() < 1e-6, "{p:?} came back as {again:?}");
        }
    }

    /// A point behind the camera has no image, and must be refused rather than
    /// mapped to a mirrored ghost on the wrong side of the frame.
    #[test]
    fn points_behind_the_camera_have_no_image() {
        // Steeply pitched: the ground plane runs away to a horizon.
        let projection = Projection::look_at_plane(400.0, 1000.0, 1.2, 0.0).expect("in front");

        let far = Point::new(0.0, 100_000.0);
        assert!(
            projection.depth_at(far) <= 0.0 || projection.map_point(far).is_some(),
            "a point either has an image or is behind the camera"
        );

        // Somewhere definitely behind: reverse the sign of the depth term.
        let behind = Projection::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        assert!(behind.map_point(Point::new(0.0, -1.0)).is_none());
    }

    /// A path crossing the horizon is cut there, not folded back into the
    /// picture — which is what an unclipped divide does, and it looks like the
    /// world turning inside out.
    #[test]
    fn a_path_crossing_the_horizon_is_clipped() {
        let projection = Projection::look_at_plane(300.0, 1000.0, 1.3, 0.0).expect("in front");

        // A long strip running away from the camera, past the horizon.
        let strip = Rect::new(-50.0, -200.0, 50.0, 20_000.0).to_path(1e-9);
        let mapped = projection.map_path(&strip, 0.1);

        for element in mapped.elements() {
            let p = match element {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => *p,
                _ => continue,
            };
            assert!(
                p.x.is_finite() && p.y.is_finite(),
                "the horizon produced {p:?}"
            );
        }
    }

    /// Mapping a path with no tilt is the affine path, exactly — curves and
    /// all, with nothing flattened.
    #[test]
    fn an_affine_map_keeps_its_curves() {
        let projection = Projection::from_affine(Affine::scale(2.0));
        let circle = kurbo::Circle::new(Point::ZERO, 40.0).to_path(1e-9);
        let mapped = projection.map_path(&circle, 0.1);

        assert!(
            mapped
                .elements()
                .iter()
                .any(|e| matches!(e, PathEl::CurveTo(..))),
            "an affine map should not flatten anything"
        );
    }

    /// A tilted map has to flatten, and what comes out must still be a closed
    /// shape of the right size rather than a scatter of segments.
    #[test]
    fn a_tilted_map_produces_a_closed_shape() {
        let projection = Projection::look_at_plane(1000.0, 1000.0, 0.4, 0.2).expect("in front");
        let mapped = projection.map_path(&square(), 0.05);

        assert!(!mapped.elements().is_empty());
        assert!(
            mapped
                .elements()
                .iter()
                .any(|e| matches!(e, PathEl::ClosePath)),
            "a closed source should stay closed"
        );

        let bounds = mapped.bounding_box();
        assert!(bounds.width() > 1.0 && bounds.height() > 1.0, "{bounds:?}");
    }

    /// Composition: an affine before the projection is the object's own
    /// placement, and it must land in the same place as transforming the
    /// points first.
    #[test]
    fn composing_with_an_affine_agrees_with_doing_it_by_hand() {
        let projection = Projection::look_at_plane(1000.0, 1000.0, 0.35, -0.2).expect("in front");
        let place = Affine::translate((30.0, -10.0)) * Affine::rotate(0.3);
        let composed = projection.pre_affine(place);

        let p = Point::new(12.0, 34.0);
        let by_hand = projection.map_point(place * p).unwrap();
        let by_composition = composed.map_point(p).unwrap();
        assert!((by_hand - by_composition).hypot() < 1e-9);
    }

    #[test]
    fn an_affine_after_the_projection_composes_too() {
        let projection = Projection::look_at_plane(1000.0, 1000.0, 0.35, 0.0).expect("in front");
        let after = Affine::translate((100.0, 50.0));
        let composed = projection.then_affine(after);

        let p = Point::new(-20.0, 40.0);
        let by_hand = after * projection.map_point(p).unwrap();
        assert!((by_hand - composed.map_point(p).unwrap()).hypot() < 1e-9);
    }

    /// A camera at or behind the plane has nothing to show.
    #[test]
    fn a_camera_on_the_plane_is_refused() {
        assert!(Projection::look_at_plane(0.0, 1000.0, 0.0, 0.0).is_none());
        assert!(Projection::look_at_plane(-500.0, 1000.0, 0.0, 0.0).is_none());
        // Turned so far that the plane is edge-on or behind.
        assert!(Projection::look_at_plane(1000.0, 1000.0, 2.0, 0.0).is_none());
    }

    /// Magnification rises as the denominator falls — the nearer half of a
    /// tilted plane is drawn bigger. This is what the flattening tolerance
    /// leans on, so it is asserted as the invariant rather than as a direction:
    /// which way `+y` leans depends on a sign convention, and the first version
    /// of this test guessed it wrong.
    /// A plane facing the camera, expressed the general way, is the same
    /// projection the layer case builds — so an object with no rotation is
    /// drawn exactly as it was before there was such a thing.
    #[test]
    fn an_unrotated_plane_matches_the_layer_case() {
        let distance = 1200.0;
        let focal = 1000.0;
        for (pitch, yaw) in [(0.0, 0.0), (0.4, 0.0), (0.0, -0.3), (0.25, 0.35)] {
            let layer = Projection::look_at_plane(distance, focal, pitch, yaw).expect("in front");

            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            let forward = [sy * cp, -sp, cy * cp];
            let to_origin = [
                distance * forward[0],
                distance * forward[1],
                distance * forward[2],
            ];
            let (ex, ey) = Projection::rotated_basis(0.0, 0.0, 0.0);
            let general =
                Projection::plane_in_view(ex, ey, to_origin, focal, pitch, yaw).expect("in front");

            for (a, b) in layer.coeffs().iter().zip(general.coeffs().iter()) {
                assert!(
                    (a - b).abs() < 1e-9,
                    "pitch {pitch} yaw {yaw}: {:?} vs {:?}",
                    layer.coeffs(),
                    general.coeffs()
                );
            }
        }
    }

    /// An unrotated basis is the plain one — the identity case has to be exact,
    /// because everything that does not use 3D goes through it.
    #[test]
    fn an_unrotated_basis_is_the_plain_one() {
        let (ex, ey) = Projection::rotated_basis(0.0, 0.0, 0.0);
        assert_eq!(ex, [1.0, 0.0, 0.0]);
        assert_eq!(ey, [0.0, 1.0, 0.0]);
    }

    /// Turning a card about the vertical tips one side away from the camera:
    /// its far edge is drawn shorter than its near one.
    #[test]
    fn a_card_turned_about_the_vertical_is_foreshortened() {
        let (ex, ey) = Projection::rotated_basis(0.0, 0.6, 0.0);
        let projection = Projection::plane_in_view(ex, ey, [0.0, 0.0, 1000.0], 1000.0, 0.0, 0.0)
            .expect("in front");

        assert!(!projection.is_affine(), "a turned card is not an affine");

        let corners = projection
            .map_rect(Rect::new(-100.0, -100.0, 100.0, 100.0))
            .expect("in front");
        let left = (corners[3] - corners[0]).hypot();
        let right = (corners[2] - corners[1]).hypot();
        assert!(
            (left - right).abs() > 4.0,
            "the two sides should differ: {left} vs {right}"
        );
    }

    /// And about the horizontal, the top and bottom differ instead.
    #[test]
    fn a_card_tipped_about_the_horizontal_is_foreshortened() {
        let (ex, ey) = Projection::rotated_basis(0.6, 0.0, 0.0);
        let projection = Projection::plane_in_view(ex, ey, [0.0, 0.0, 1000.0], 1000.0, 0.0, 0.0)
            .expect("in front");

        let corners = projection
            .map_rect(Rect::new(-100.0, -100.0, 100.0, 100.0))
            .expect("in front");
        let top = (corners[1] - corners[0]).hypot();
        let bottom = (corners[2] - corners[3]).hypot();
        assert!((top - bottom).abs() > 4.0, "{top} vs {bottom}");
    }

    /// Spinning a card in its own plane is a *rotation*, not a perspective: it
    /// stays an affine, because the plane has not moved.
    #[test]
    fn spinning_a_card_in_its_own_plane_stays_affine() {
        let (ex, ey) = Projection::rotated_basis(0.0, 0.0, 0.7);
        let projection = Projection::plane_in_view(ex, ey, [0.0, 0.0, 1000.0], 1000.0, 0.0, 0.0)
            .expect("in front");
        assert!(projection.is_affine(), "{projection:?}");
    }

    /// A card turned exactly edge-on has no image at all.
    #[test]
    fn an_edge_on_card_is_refused() {
        let (ex, ey) = Projection::rotated_basis(0.0, std::f64::consts::FRAC_PI_2, 0.0);
        assert!(Projection::plane_in_view(ex, ey, [0.0, 0.0, 1000.0], 1000.0, 0.0, 0.0).is_none());
    }

    /// Turned past edge-on it shows its **back**, mirrored — which is what the
    /// back of a card looks like, and better than having it vanish.
    #[test]
    fn a_card_turned_right_round_shows_its_back() {
        let (ex, ey) = Projection::rotated_basis(0.0, 2.6, 0.0);
        let projection = Projection::plane_in_view(ex, ey, [0.0, 0.0, 1000.0], 1000.0, 0.0, 0.0)
            .expect("the back is still a picture");

        // Mirrored: the corner that was on the left is now on the right.
        let corners = projection
            .map_rect(Rect::new(-100.0, -100.0, 100.0, 100.0))
            .expect("in front");
        assert!(
            corners[0].x > corners[1].x,
            "the back of a card should be mirrored: {corners:?}"
        );
    }

    /// A card pushed away along its own depth is drawn smaller — which is what
    /// makes the near cards of a tree sweep past the far ones.
    #[test]
    fn a_card_pushed_back_is_drawn_smaller() {
        let (ex, ey) = Projection::rotated_basis(0.0, 0.0, 0.0);
        let size = |z: f64| {
            let projection =
                Projection::plane_in_view(ex, ey, [0.0, 0.0, 1000.0 + z], 1000.0, 0.0, 0.0)
                    .expect("in front");
            projection
                .map_rect_bounds(Rect::new(-100.0, -100.0, 100.0, 100.0))
                .expect("in front")
                .width()
        };
        assert!(size(400.0) < size(0.0), "further should be smaller");
        assert!(size(-400.0) > size(0.0), "nearer should be bigger");
    }

    #[test]
    fn magnification_rises_as_the_plane_comes_nearer() {
        let projection = Projection::look_at_plane(1000.0, 1000.0, 0.8, 0.0).expect("in front");

        let a = Point::new(0.0, -200.0);
        let b = Point::new(0.0, 200.0);
        let (wa, wb) = (projection.depth_at(a), projection.depth_at(b));
        let (sa, sb) = (projection.scale_at(a), projection.scale_at(b));

        assert!(sa.is_finite() && sb.is_finite());
        assert_ne!(wa, wb, "a pitched camera should not see both at one depth");
        if wa < wb {
            assert!(sa > sb, "the nearer point should magnify more: {sa} vs {sb}");
        } else {
            assert!(sb > sa, "the nearer point should magnify more: {sb} vs {sa}");
        }
    }

    /// Scale is defined up to the matrix's own scale, so a projection and the
    /// same one times a thousand must behave identically.
    #[test]
    fn a_scaled_matrix_is_the_same_projection() {
        let plain = Projection::new([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        let scaled = Projection::new([50.0, 0.0, 0.0, 0.0, 50.0, 0.0, 0.0, 0.0, 50.0]);

        assert!(scaled.is_affine());
        let p = Point::new(7.0, -3.0);
        assert!((plain.map_point(p).unwrap() - scaled.map_point(p).unwrap()).hypot() < 1e-12);
        assert_eq!(
            scaled.as_affine().unwrap().as_coeffs(),
            plain.as_affine().unwrap().as_coeffs()
        );
    }
}
