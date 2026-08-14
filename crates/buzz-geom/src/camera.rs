//! The rebasing camera: unbounded zoom without precision collapse.
//!
//! # Why Animate stops at 2000%
//!
//! Flash-lineage formats store coordinates as twips and rasterise in `f32`.
//! An `f32` carries a 24-bit mantissa (~7 significant decimal digits). Once the
//! composed view transform contains a large translation, the coordinates handed
//! to the rasteriser lose all sub-pixel meaning and edges visibly break up. The
//! 2000% cap is where that becomes unacceptable — it is a symptom of the number
//! format, not a deliberate product limit.
//!
//! # Why naive `f64` alone does not fix it
//!
//! The obvious fix — "store everything in `f64`" — is necessary but *not
//! sufficient*, because the GPU still consumes `f32`. Consider composing the
//! whole view into a single affine:
//!
//! ```text
//! A = translate(viewport_centre) · rotate(θ) · scale(z) · translate(−centre)
//! ```
//!
//! Applying `A` to a document point `p` evaluates, in effect,
//! `z·p − z·centre`. With `z = 1e9` and `centre ≈ 1e3`, both terms are ≈ `1e12`
//! while their *difference* is the handful of pixels we actually want. That is
//! catastrophic cancellation: the answer is the small difference of two large
//! numbers, and every bit of precision goes into representing magnitude that
//! immediately cancels. Downcasting either term to `f32` destroys the result
//! outright.
//!
//! # The fix: subtract first, scale second
//!
//! Split the transform in two and keep the order of operations honest:
//!
//! 1. **Rebase, in `f64`, on the CPU:** `q = p − anchor`. Both operands are
//!    document-scale (`≈1e3`), so the subtraction is well conditioned and `q`
//!    is exact to ~16 digits.
//! 2. **Project:** `s = viewport_centre + R·(z·q)`. Now the large factor `z`
//!    multiplies an already-small `q`, and the product lands in viewport range.
//!
//! Only step 2's operands ever reach `f32`, and by then everything is
//! viewport-sized. This is the floating-origin / relative-to-eye technique used
//! by planet-scale renderers, applied to a 2D canvas.
//!
//! **The order matters and is not associative in floating point.** Fusing the
//! two steps back into one affine reintroduces the cancellation. That is the
//! single most important invariant in this module, and
//! [`tests::rebasing_beats_naive_composition_at_extreme_zoom`] pins it down.

use kurbo::{Affine, Point, Rect, Size, Vec2};
use serde::{Deserialize, Serialize};

/// A viewport onto document space.
///
/// Zoom is deliberately **unclamped**. There is no maximum. See the module
/// documentation for why that is safe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// Point in document space at the centre of the viewport.
    pub center: Point,
    /// Document units per physical pixel. Unbounded above.
    pub zoom: f64,
    /// Viewport rotation, radians, clockwise.
    pub rotation: f64,
    /// Viewport size in physical pixels.
    pub viewport: Size,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: Point::ORIGIN,
            zoom: 1.0,
            rotation: 0.0,
            viewport: Size::new(1280.0, 720.0),
        }
    }
}

/// A view transform split into its two numerically-critical halves.
///
/// Consumers **must** apply [`Self::anchor`] to geometry in `f64` before
/// handing anything to the renderer, then pass [`Self::view`] as the transform.
/// Pre-multiplying the two together defeats the entire mechanism — see
/// [`RebasedTransform::fused_unsafe_for_gpu`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RebasedTransform {
    /// Subtract this from document-space coordinates first, in `f64`.
    pub anchor: Point,
    /// Maps anchor-relative coordinates to screen pixels. Safe to downcast to
    /// `f32`, because its inputs are small by construction.
    pub view: Affine,
}

impl RebasedTransform {
    /// Rebase a document-space point into anchor-relative space.
    ///
    /// This is the step that must happen in `f64`.
    #[inline]
    pub fn rebase(&self, p: Point) -> Point {
        (p - self.anchor).to_point()
    }

    /// Full document-space to screen-space projection, in `f64`.
    ///
    /// Correct at any zoom, but only usable on the CPU (hit-testing, UI
    /// placement, culling). GPU geometry must go through [`Self::rebase`].
    #[inline]
    pub fn project(&self, p: Point) -> Point {
        self.view * self.rebase(p)
    }

    /// The two halves collapsed into one affine.
    ///
    /// Provided for tests and for comparison against the naive approach. Named
    /// to be hard to reach for by accident: feeding this to an `f32` consumer
    /// is precisely the bug this module exists to prevent.
    #[inline]
    pub fn fused_unsafe_for_gpu(&self) -> Affine {
        self.view * Affine::translate(-self.anchor.to_vec2())
    }
}

/// The view split for *rendering*, which goes one step further than
/// [`RebasedTransform`].
///
/// # Why the scale must also be applied on the CPU
///
/// [`RebasedTransform`] hands the renderer small anchor-relative coordinates
/// plus a view transform carrying the full magnification. That fixes precision,
/// but it leaves the multiplication `zoom · q` to be performed by the GPU in
/// `f32`. Measured on an RTX 5060 Ti, Vello's cost for an identical 70-item
/// scene rose from 0.8 ms to 25 ms between 1e10 % and 1e12 % zoom — the
/// geometry and item count were unchanged, only the transform magnitude grew.
///
/// So the scale is applied on the CPU too, in `f64`, leaving the GPU a
/// transform of unit scale and coordinates already in viewport range.
///
/// **The two CPU steps must stay separate.** Composing them into one affine
/// makes it evaluate `zoom·p − zoom·anchor`, restoring exactly the catastrophic
/// cancellation this design exists to avoid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSplit {
    /// Step 1: subtract from document coordinates, in `f64`.
    pub anchor: Point,
    /// Step 2: multiply the result by this, in `f64`. Output is viewport-sized.
    pub scale: f64,
    /// Step 3: hand to the GPU. Rotation and viewport offset only — unit scale,
    /// so nothing large ever reaches `f32`.
    pub gpu_view: Affine,
}

impl RenderSplit {
    /// Apply both CPU steps to a point, in the correct order.
    #[inline]
    pub fn to_render_space(&self, p: Point) -> Point {
        let q = p - self.anchor;
        (q * self.scale).to_point()
    }

    /// Convert a document-space length into render space.
    #[inline]
    pub fn scale_length(&self, len: f64) -> f64 {
        len * self.scale
    }
}

impl Camera {
    /// A camera looking at `center` with the given zoom.
    pub fn new(center: Point, zoom: f64, viewport: Size) -> Self {
        Self {
            center,
            zoom,
            rotation: 0.0,
            viewport,
        }
    }

    /// Viewport centre in physical pixels.
    #[inline]
    pub fn viewport_center(&self) -> Vec2 {
        Vec2::new(self.viewport.width * 0.5, self.viewport.height * 0.5)
    }

    /// Split view transform. This is what the renderer consumes.
    ///
    /// The anchor is the camera centre, which keeps rebased coordinates bounded
    /// by `viewport / zoom` — vanishingly small at high zoom, which is exactly
    /// when precision would otherwise fail.
    #[inline]
    pub fn rebased(&self) -> RebasedTransform {
        RebasedTransform {
            anchor: self.center,
            view: Affine::translate(self.viewport_center())
                * Affine::rotate(self.rotation)
                * Affine::scale(self.zoom),
        }
    }

    /// Split view for the renderer: two CPU steps plus a unit-scale GPU
    /// transform. See [`RenderSplit`].
    #[inline]
    pub fn render_split(&self) -> RenderSplit {
        RenderSplit {
            anchor: self.center,
            scale: self.zoom,
            gpu_view: Affine::translate(self.viewport_center()) * Affine::rotate(self.rotation),
        }
    }

    /// Document point to screen pixel, `f64` throughout.
    #[inline]
    pub fn doc_to_screen(&self, p: Point) -> Point {
        self.rebased().project(p)
    }

    /// Screen pixel to document point, `f64` throughout.
    #[inline]
    pub fn screen_to_doc(&self, s: Point) -> Point {
        let r = self.rebased();
        // Invert only the small-magnitude half, then undo the rebase in f64.
        let q = r.view.inverse() * s;
        self.center + q.to_vec2()
    }

    /// Region of document space currently visible, for culling.
    ///
    /// Accounts for rotation by taking the bounding box of the transformed
    /// viewport corners.
    pub fn visible_doc_rect(&self) -> Rect {
        let (w, h) = (self.viewport.width, self.viewport.height);
        let corners = [
            Point::new(0.0, 0.0),
            Point::new(w, 0.0),
            Point::new(w, h),
            Point::new(0.0, h),
        ];
        let mut r = Rect::from_points(
            self.screen_to_doc(corners[0]),
            self.screen_to_doc(corners[1]),
        );
        for c in &corners[2..] {
            r = r.union_pt(self.screen_to_doc(*c));
        }
        r
    }

    /// Multiply zoom by `factor`, keeping the document point currently under
    /// `screen_pivot` pinned to that same screen position.
    ///
    /// This is mouse-wheel zoom. Deriving `new_center` analytically (rather
    /// than iterating) keeps the pivot exact even at extreme zoom.
    pub fn zoom_by_at(&mut self, factor: f64, screen_pivot: Point) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let pinned_doc = self.screen_to_doc(screen_pivot);
        let new_zoom = self.zoom * factor;
        if !(new_zoom.is_finite() && new_zoom > 0.0) {
            return;
        }

        // Solve s = vc + R·(z'·(pinned − c')) for c'.
        let offset = screen_pivot.to_vec2() - self.viewport_center();
        let unrotated = Affine::rotate(-self.rotation) * offset.to_point();
        self.center = pinned_doc - (unrotated.to_vec2() / new_zoom);
        self.zoom = new_zoom;
    }

    /// Pan by a screen-space delta in pixels.
    pub fn pan_screen(&mut self, delta_px: Vec2) {
        let unrotated = Affine::rotate(-self.rotation) * delta_px.to_point();
        self.center -= unrotated.to_vec2() / self.zoom;
    }

    /// Size of one `f64` quantisation step, measured in screen pixels.
    ///
    /// Rebasing removes the *catastrophic* precision loss, but one hard floor
    /// remains and it is worth being honest about: a document coordinate is
    /// stored as an absolute `f64`, so its resolution is `|coord| · ε` document
    /// units (`ε ≈ 2.22e-16`). Magnified by `zoom`, that becomes
    ///
    /// ```text
    /// precision_px ≈ |coord| · ε · zoom
    /// ```
    ///
    /// For a document with coordinates around `1e3`:
    ///
    /// | zoom factor | shown as | precision |
    /// |------------:|---------:|----------:|
    /// | `20`        | 2 000%   | 4e-12 px  |
    /// | `1e10`      | 1e12 %   | 0.002 px  |
    /// | `1e12`      | 1e14 %   | 0.25 px   |
    ///
    /// So positions stay sub-pixel out to roughly `1e12 %`, about ten orders of
    /// magnitude past Animate's ceiling, and degrade linearly after that rather
    /// than collapsing. Surfacing this in the HUD lets a user see exactly where
    /// they are against that budget.
    #[inline]
    pub fn screen_precision_px(&self) -> f64 {
        let magnitude = self.center.x.abs().max(self.center.y.abs()).max(1.0);
        magnitude * f64::EPSILON * self.zoom
    }

    /// Zoom expressed the way the UI shows it. 100.0 means 1:1.
    ///
    /// Animate's field stops accepting input above 2000.0. This one does not.
    #[inline]
    pub fn zoom_percent(&self) -> f64 {
        self.zoom * 100.0
    }

    /// Set zoom from a UI percentage, keeping the viewport centre fixed.
    #[inline]
    pub fn set_zoom_percent(&mut self, percent: f64) {
        if percent.is_finite() && percent > 0.0 {
            self.zoom = percent / 100.0;
        }
    }

    /// Frame `rect` in the viewport with a margin factor (1.0 = tight fit).
    pub fn fit_to_rect(&mut self, rect: Rect, margin: f64) {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let sx = self.viewport.width / rect.width();
        let sy = self.viewport.height / rect.height();
        self.zoom = sx.min(sy) / margin.max(f64::MIN_POSITIVE);
        self.center = rect.center();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Size = Size::new(1920.0, 1080.0);

    /// Apply an affine after forcing it and its input through `f32`, the way a
    /// GPU pipeline would.
    fn apply_as_f32(a: Affine, p: Point) -> Point {
        let c = a.as_coeffs().map(|v| v as f32);
        let (x, y) = (p.x as f32, p.y as f32);
        Point::new(
            (c[0] * x + c[2] * y + c[4]) as f64,
            (c[1] * x + c[3] * y + c[5]) as f64,
        )
    }

    #[test]
    fn round_trip_holds_across_twelve_decades_of_zoom() {
        for decade in 0..=12 {
            let zoom = 10f64.powi(decade);
            let cam = Camera::new(Point::new(1234.5, -987.25), zoom, VIEWPORT);

            let screen = Point::new(640.0, 360.0);
            let doc = cam.screen_to_doc(screen);
            let back = cam.doc_to_screen(doc);

            // The floor is f64 storage of an absolute document coordinate, not
            // the rebasing. Allow a small multiple of the documented step to
            // cover the handful of operations in a round trip.
            let budget = (cam.screen_precision_px() * 8.0).max(1e-9);
            let err = (back - screen).hypot();

            assert!(
                err < budget,
                "round trip at zoom 1e{decade} drifted {err} px, budget {budget} px \
                 ({screen:?} -> {doc:?} -> {back:?})"
            );
        }
    }

    /// Guards the precision model itself: sub-pixel out to 1e12%, which is the
    /// figure quoted to the user, and graceful linear decay past it.
    #[test]
    fn precision_model_matches_the_advertised_budget() {
        let cam = Camera::new(Point::new(1000.0, 1000.0), 1e10, VIEWPORT);
        assert!(
            cam.screen_precision_px() < 0.01,
            "1e12% should stay well sub-pixel, got {} px",
            cam.screen_precision_px()
        );

        // Animate's ceiling, for contrast.
        let animate_max = Camera::new(Point::new(1000.0, 1000.0), 20.0, VIEWPORT);
        assert!(animate_max.screen_precision_px() < 1e-10);

        // Degrades linearly rather than falling off a cliff.
        let deep = Camera::new(Point::new(1000.0, 1000.0), 1e12, VIEWPORT);
        let ratio = deep.screen_precision_px() / cam.screen_precision_px();
        assert!(
            (ratio - 100.0).abs() < 1.0,
            "precision should scale linearly with zoom, ratio was {ratio}"
        );
    }

    /// The headline claim, as an executable proof.
    ///
    /// At 1e9× zoom the rebased pipeline stays sub-pixel accurate through an
    /// `f32` downcast, while composing the transform first produces garbage.
    #[test]
    fn rebasing_beats_naive_composition_at_extreme_zoom() {
        let zoom = 1e9;
        let center = Point::new(1000.0, 1000.0);
        let cam = Camera::new(center, zoom, VIEWPORT);
        let r = cam.rebased();

        // A point 1e-7 document units from the centre — 100 px away on screen.
        let p = Point::new(center.x + 1e-7, center.y + 1e-7);
        let exact = r.project(p);

        // Rebase in f64, then let only the small values meet f32.
        let rebased_f32 = apply_as_f32(r.view, r.rebase(p));

        // Naive: one fused affine, document coordinates straight into f32.
        let naive_f32 = apply_as_f32(r.fused_unsafe_for_gpu(), p);

        let rebased_err = (rebased_f32 - exact).hypot();
        let naive_err = (naive_f32 - exact).hypot();

        assert!(
            rebased_err < 0.01,
            "rebased path should stay sub-pixel, got {rebased_err} px"
        );
        assert!(
            naive_err > 1.0,
            "naive path was expected to fail here; if this now passes the \
             comparison has lost its meaning and the test needs rewriting \
             (got {naive_err} px)"
        );
        assert!(
            naive_err > rebased_err * 1000.0,
            "rebasing should be orders of magnitude better: \
             rebased={rebased_err} px, naive={naive_err} px"
        );
    }

    #[test]
    fn rebased_output_stays_finite_and_small_at_absurd_zoom() {
        // Far past anything a user will reach, and far past Animate's 20x.
        for exp in [6, 9, 12, 15, 18] {
            let zoom = 10f64.powi(exp);
            let cam = Camera::new(Point::new(500.0, 500.0), zoom, VIEWPORT);
            let r = cam.rebased();

            let visible = cam.visible_doc_rect();
            let corner = Point::new(visible.x0, visible.y0);
            let q = r.rebase(corner);
            let screen = r.project(corner);

            assert!(q.x.is_finite() && q.y.is_finite(), "rebase NaN at 1e{exp}");
            assert!(
                screen.x.is_finite() && screen.y.is_finite(),
                "projection NaN at 1e{exp}"
            );
            // The rebased coordinate must shrink as zoom grows; that shrinkage
            // is what protects the f32 downcast.
            assert!(
                q.x.abs() <= VIEWPORT.width / zoom * 2.0,
                "rebased coord did not shrink with zoom at 1e{exp}: {q:?}"
            );
        }
    }

    #[test]
    fn zoom_at_pivot_keeps_the_point_under_the_cursor() {
        let mut cam = Camera::new(Point::new(10.0, 20.0), 1.0, VIEWPORT);
        let pivot = Point::new(1500.0, 200.0);
        let doc_before = cam.screen_to_doc(pivot);

        for _ in 0..40 {
            cam.zoom_by_at(1.25, pivot);
        }

        let doc_after = cam.screen_to_doc(pivot);
        let drift = (doc_after - doc_before).hypot();
        assert!(
            drift < 1e-9,
            "pivot drifted by {drift} document units over 40 zoom steps \
             (final zoom {}%)",
            cam.zoom_percent()
        );
    }

    #[test]
    fn zoom_is_not_clamped_at_animates_ceiling() {
        let mut cam = Camera::new(Point::ORIGIN, 1.0, VIEWPORT);
        cam.set_zoom_percent(2000.0);
        assert_eq!(cam.zoom_percent(), 2000.0);

        // Animate refuses past here. We keep going.
        cam.set_zoom_percent(1e12);
        assert_eq!(cam.zoom_percent(), 1e12);
        assert!(cam.zoom.is_finite());
    }

    #[test]
    fn degenerate_zoom_input_is_rejected_not_propagated() {
        let mut cam = Camera::new(Point::ORIGIN, 4.0, VIEWPORT);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            cam.set_zoom_percent(bad);
            assert_eq!(cam.zoom, 4.0, "zoom was corrupted by input {bad}");
            cam.zoom_by_at(bad, Point::new(10.0, 10.0));
            assert_eq!(cam.zoom, 4.0, "zoom was corrupted by factor {bad}");
        }
    }

    #[test]
    fn pan_is_reversible_and_scales_with_zoom() {
        let mut cam = Camera::new(Point::new(3.0, 4.0), 250.0, VIEWPORT);
        let before = cam.center;
        cam.pan_screen(Vec2::new(120.0, -75.0));
        assert_ne!(cam.center, before);
        cam.pan_screen(Vec2::new(-120.0, 75.0));
        assert!((cam.center - before).hypot() < 1e-12);
    }

    #[test]
    fn rotation_round_trips_through_screen_space() {
        let mut cam = Camera::new(Point::new(50.0, 60.0), 3.0, VIEWPORT);
        cam.rotation = std::f64::consts::FRAC_PI_6;
        let p = Point::new(75.5, 12.25);
        let back = cam.screen_to_doc(cam.doc_to_screen(p));
        assert!(
            (back - p).hypot() < 1e-9,
            "rotated round trip failed: {back:?}"
        );
    }

    #[test]
    fn fit_to_rect_frames_the_whole_rect() {
        let mut cam = Camera::new(Point::ORIGIN, 1.0, VIEWPORT);
        let stage = Rect::new(-320.0, -240.0, 320.0, 240.0);
        cam.fit_to_rect(stage, 1.0);
        let visible = cam.visible_doc_rect();

        // Compare edges directly. `Rect::contains` is half-open, so a tight fit
        // legitimately puts the far corner exactly on the excluded boundary.
        let eps = 1e-9;
        assert!(
            visible.x0 <= stage.x0 + eps
                && visible.y0 <= stage.y0 + eps
                && visible.x1 >= stage.x1 - eps
                && visible.y1 >= stage.y1 - eps,
            "stage {stage:?} not framed by {visible:?}"
        );

        // A tight fit should touch the constraining axis, not overshoot both.
        assert!(
            (visible.height() - stage.height()).abs() < eps,
            "expected height to be the limiting axis for a 640x480 stage in a \
             1920x1080 viewport, got {visible:?}"
        );
    }
}
