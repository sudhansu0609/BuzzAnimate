//! Gradients — a fill or stroke whose colour varies across the shape.
//!
//! # Why a unit gradient and a matrix, rather than points on the stage
//!
//! A linear gradient could be stored as the two points its ramp runs between,
//! and a radial one as a centre and a radius. That is the obvious model and it
//! is not the one used here, for three reasons that all point the same way:
//!
//! * **It is what every format we read already does.** SWF stores a gradient
//!   as a matrix mapping a fixed square; XFL stores the same matrix, because
//!   XFL is Animate writing out what it would have compiled into a SWF. A
//!   reader that has to solve for endpoints is a reader that can get it wrong.
//! * **It is what Animate's Gradient Transform tool edits.** The tool has a
//!   centre handle, a width handle and a rotation handle — that is a matrix,
//!   presented as three grips. Storing endpoints would mean converting to a
//!   matrix to drag and back again to store, twice per mouse move.
//! * **Squashing a radial gradient is free.** An ellipse is a circle with a
//!   non-uniform scale in its matrix. With a centre and a radius it would need
//!   a second radius, and then a rotation, and at that point the matrix has
//!   been rebuilt out of named parts.
//!
//! So a gradient is defined in **unit space** — the ramp runs from `x = -1` to
//! `x = 1` for a linear gradient, and a radial one is the unit circle about the
//! origin — and [`Gradient::transform`] puts it where it belongs in the
//! object's own coordinates.

use buzz_geom::{Affine, Point, Rect};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// Animate allows fifteen colours in a gradient; so does SWF's `GRADIENT`
/// record. Bounding it keeps a corrupt or hostile file from asking for a
/// million stops, and matches what the formats can express anyway.
pub const MAX_STOPS: usize = 15;

/// One colour, at one place along the ramp.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// Where on the ramp this colour sits, `0.0` to `1.0`.
    pub offset: f64,
    pub color: Color,
}

impl GradientStop {
    pub fn new(offset: f64, color: Color) -> Self {
        Self {
            offset: sane_offset(offset),
            color,
        }
    }
}

/// An offset from a file, made safe.
///
/// **`f64::clamp` is not enough**, and that is the whole reason this exists:
/// `clamp` *propagates* NaN rather than replacing it, so a damaged file's NaN
/// offset passes straight through and lands in the model. From there it makes
/// `partition_point` return whatever it likes and the ramp is unpredictable.
/// Zero is the start of the ramp, which is where a stop with no stated position
/// belongs.
fn sane_offset(offset: f64) -> f64 {
    if offset.is_finite() {
        offset.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The shape of the ramp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GradientKind {
    /// Colours run along a line: in unit space, from `(-1, 0)` to `(1, 0)`.
    #[default]
    Linear,
    /// Colours radiate outwards: in unit space, the circle of radius 1 about
    /// the origin.
    Radial,
}

impl GradientKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Radial => "Radial",
        }
    }
}

/// What happens outside the ramp — Animate's Extend, Reflect and Repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GradientSpread {
    /// The end colours continue outwards. Animate's default, and the only one
    /// that cannot show a seam.
    #[default]
    Pad,
    /// The ramp mirrors back and forth.
    Reflect,
    /// The ramp starts again.
    Repeat,
}

impl GradientSpread {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pad => "Extend",
            Self::Reflect => "Reflect",
            Self::Repeat => "Repeat",
        }
    }
}

/// A gradient, in unit space, plus where it goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub kind: GradientKind,
    /// Ordered by offset, at least two, at most [`MAX_STOPS`].
    stops: Vec<GradientStop>,
    /// Unit space to the object's own space.
    pub transform: Affine,
    pub spread: GradientSpread,
    /// For a radial gradient, where the "hot spot" sits along the unit x axis,
    /// `-1.0` to `1.0`. Animate calls this the focal point and drags it as a
    /// small triangle; zero is a plain concentric gradient. Ignored by a linear
    /// gradient, which has no centre to move.
    pub focal: f64,
}

impl Gradient {
    /// A two-stop gradient across the unit space.
    ///
    /// Stops are sorted and bounded here rather than trusted, because both
    /// importers and the scripting API can supply them.
    pub fn new(kind: GradientKind, stops: Vec<GradientStop>) -> Self {
        let mut g = Self {
            kind,
            stops: Vec::new(),
            transform: Affine::IDENTITY,
            spread: GradientSpread::Pad,
            focal: 0.0,
        };
        g.set_stops(stops);
        g
    }

    /// A linear ramp between two colours, laid across `rect`.
    ///
    /// The common case, and the one the Color panel makes when a fill is
    /// switched from solid to gradient: the ramp runs left to right across the
    /// shape it is being applied to, which is where a user expects to find it.
    pub fn linear(from: Color, to: Color, rect: Rect) -> Self {
        let mut g = Self::new(
            GradientKind::Linear,
            vec![GradientStop::new(0.0, from), GradientStop::new(1.0, to)],
        );
        g.fit_to(rect);
        g
    }

    /// A radial ramp between two colours, filling `rect`.
    pub fn radial(from: Color, to: Color, rect: Rect) -> Self {
        let mut g = Self::new(
            GradientKind::Radial,
            vec![GradientStop::new(0.0, from), GradientStop::new(1.0, to)],
        );
        g.fit_to(rect);
        g
    }

    /// Place the gradient over a rectangle — the whole of it, in both axes.
    ///
    /// Unit space is `-1..1`, so the scale is *half* the extent. A radial
    /// gradient over a non-square rectangle becomes an ellipse that reaches the
    /// edges, which is what Animate does and what "fill this shape" means.
    pub fn fit_to(&mut self, rect: Rect) {
        let c = rect.center();
        // A zero-extent shape — a straight line, or a single click — would
        // otherwise give a singular matrix that renders as nothing at all.
        let hw = (rect.width() * 0.5).max(f64::MIN_POSITIVE);
        let hh = (rect.height() * 0.5).max(f64::MIN_POSITIVE);
        self.transform = Affine::translate(c.to_vec2()) * Affine::scale_non_uniform(hw, hh);
    }

    pub fn stops(&self) -> &[GradientStop] {
        &self.stops
    }

    /// Replace the stops, sorted, bounded and guaranteed to be at least two.
    ///
    /// A gradient with one stop is a solid colour that costs a gradient to
    /// draw, and one with none has no colour at all; rather than reject either
    /// — both arrive from real files — they are padded, so the result is always
    /// drawable.
    pub fn set_stops(&mut self, mut stops: Vec<GradientStop>) {
        stops.truncate(MAX_STOPS);
        for s in &mut stops {
            s.offset = sane_offset(s.offset);
        }
        // `total_cmp` rather than `partial_cmp().unwrap()`: a NaN offset from a
        // damaged file would panic the sort, and sorting is not the place to
        // discover that.
        stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
        match stops.len() {
            0 => {
                stops.push(GradientStop::new(0.0, Color::BLACK));
                stops.push(GradientStop::new(1.0, Color::WHITE));
            }
            1 => {
                let only = stops[0];
                stops.clear();
                stops.push(GradientStop::new(0.0, only.color));
                stops.push(GradientStop::new(1.0, only.color));
            }
            _ => {}
        }
        self.stops = stops;
    }

    /// The colour at a point along the ramp, `0.0` to `1.0`.
    ///
    /// Interpolated in straight (non-premultiplied) sRGB, which is what SWF,
    /// XFL and PDF all specify and therefore what makes an imported gradient
    /// look like it did in the tool that wrote it.
    pub fn sample(&self, t: f64) -> Color {
        let t = t.clamp(0.0, 1.0);
        let stops = &self.stops;
        // Before the first and after the last, the end colours hold — which is
        // Pad. Reflect and Repeat are the *renderer's* business, because they
        // describe what happens outside the ramp, not inside it.
        if t <= stops[0].offset {
            return stops[0].color;
        }
        let last = stops[stops.len() - 1];
        if t >= last.offset {
            return last.color;
        }
        let i = stops.partition_point(|s| s.offset <= t).max(1);
        let (a, b) = (stops[i - 1], stops[i]);
        let span = b.offset - a.offset;
        // Two stops at the same offset are a hard colour change, which is how a
        // stripe is drawn. Dividing by the zero span would give NaN.
        if span <= 0.0 {
            return b.color;
        }
        lerp_color(a.color, b.color, (t - a.offset) / span)
    }

    /// One colour standing in for the whole gradient.
    ///
    /// Everything that needs a gradient to be a single colour goes through
    /// here: the lighting model, which shades a fill by tinting it; outline
    /// view; the Swatches panel; and both importers before this existed. It is
    /// the ramp's mean, weighted by how much of the ramp each span occupies, so
    /// a gradient that is red for nine tenths of its length averages to
    /// something close to red rather than to the midpoint of red and its far
    /// end.
    pub fn average_color(&self) -> Color {
        let stops = &self.stops;
        let (mut r, mut g, mut b, mut a, mut total) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
        for pair in stops.windows(2) {
            let (s0, s1) = (pair[0], pair[1]);
            let w = s1.offset - s0.offset;
            if w <= 0.0 {
                continue;
            }
            let (c0, c1) = (s0.color.components, s1.color.components);
            // The mean of a linear ramp is the mean of its endpoints, so each
            // span contributes its midpoint weighted by its length.
            r += 0.5 * f64::from(c0[0] + c1[0]) * w;
            g += 0.5 * f64::from(c0[1] + c1[1]) * w;
            b += 0.5 * f64::from(c0[2] + c1[2]) * w;
            a += 0.5 * f64::from(c0[3] + c1[3]) * w;
            total += w;
        }
        // Every stop at the same offset: no span has any length, so there is no
        // ramp to average and the first colour is the whole of it.
        if total <= 0.0 {
            return stops[0].color;
        }
        Color::new([
            (r / total) as f32,
            (g / total) as f32,
            (b / total) as f32,
            (a / total) as f32,
        ])
    }

    /// The same gradient with every stop passed through `f`.
    ///
    /// This is how an instance's colour effect, a filter's Adjust Color and the
    /// onion-skin ghost reach a gradient: they are defined as functions of a
    /// colour, and a gradient is a list of colours.
    pub fn map_colors(&self, f: impl Fn(Color) -> Color) -> Self {
        Self {
            kind: self.kind,
            stops: self
                .stops
                .iter()
                .map(|s| GradientStop {
                    offset: s.offset,
                    color: f(s.color),
                })
                .collect(),
            transform: self.transform,
            spread: self.spread,
            focal: self.focal,
        }
    }

    /// The same gradient moved by `t` — used when artwork is transformed, so a
    /// gradient travels with the shape it is painted on.
    pub fn transformed(&self, t: Affine) -> Self {
        let mut g = self.clone();
        g.transform = t * g.transform;
        g
    }

    /// The unit-space anchors the Gradient Transform tool grabs: the centre,
    /// the end of the ramp, and the point that sets its thickness.
    ///
    /// Returned in the object's own space, which is where the tool works.
    pub fn handles(&self) -> GradientHandles {
        GradientHandles {
            center: self.transform * Point::new(0.0, 0.0),
            end: self.transform * Point::new(1.0, 0.0),
            width: self.transform * Point::new(0.0, 1.0),
            focus: self.transform * Point::new(self.focal.clamp(-1.0, 1.0), 0.0),
        }
    }

    /// Move the centre of the gradient to `to`, carrying the ramp with it.
    ///
    /// The three operations below are all writes to one part of the matrix,
    /// because the grips **are** the matrix: the centre is its translation, the
    /// end of the ramp is its first column and the width handle is its second.
    /// Nothing is decomposed into an angle and a scale and put back together,
    /// so a gradient cannot lose its skew by being dragged.
    pub fn set_center(&mut self, to: Point) {
        let mut c = self.transform.as_coeffs();
        c[4] = to.x;
        c[5] = to.y;
        self.transform = Affine::new(c);
    }

    /// Put the end of the ramp at `to`, which sets its direction and its length
    /// together.
    pub fn set_end(&mut self, to: Point) {
        let mut c = self.transform.as_coeffs();
        let centre = Point::new(c[4], c[5]);
        let axis = to - centre;
        // A ramp of no length is a singular matrix, which renders as nothing.
        // Refusing the drag leaves the gradient as it was, which is what the
        // user can see and undo.
        if axis.hypot() <= f64::MIN_POSITIVE {
            return;
        }
        c[0] = axis.x;
        c[1] = axis.y;
        self.transform = Affine::new(c);
    }

    /// Put the width handle at `to`, which sets how far the ramp reaches across
    /// its own axis. For a radial gradient this is what makes it an ellipse.
    pub fn set_width_handle(&mut self, to: Point) {
        let mut c = self.transform.as_coeffs();
        let centre = Point::new(c[4], c[5]);
        let across = to - centre;
        if across.hypot() <= f64::MIN_POSITIVE {
            return;
        }
        c[2] = across.x;
        c[3] = across.y;
        self.transform = Affine::new(c);
    }

    /// Slide the focal point towards `to`.
    ///
    /// Only the component along the ramp's own axis counts: the focus runs on
    /// that line, so a drag away from it moves the hot spot as far along as the
    /// pointer reached and no further. Clamped to the ramp, because a focus
    /// outside the circle has no meaning.
    pub fn set_focus(&mut self, to: Point) {
        let c = self.transform.as_coeffs();
        let axis = buzz_geom::Vec2::new(c[0], c[1]);
        let len2 = axis.x * axis.x + axis.y * axis.y;
        if len2 <= f64::MIN_POSITIVE {
            return;
        }
        let from_centre = to - Point::new(c[4], c[5]);
        self.focal = ((from_centre.x * axis.x + from_centre.y * axis.y) / len2).clamp(-1.0, 1.0);
    }

    /// Whether interpolating towards `other` is meaningful.
    ///
    /// Two gradients tween stop by stop, which needs the same number of stops
    /// and the same kind. Anything else would have to invent colours, and a
    /// tween that invents colours is one an animator cannot predict.
    pub fn tweenable_with(&self, other: &Self) -> bool {
        self.kind == other.kind && self.stops.len() == other.stops.len()
    }

    /// Interpolate towards `other`. Falls back to whichever end is nearer when
    /// the two do not correspond, rather than producing a gradient neither
    /// keyframe contains.
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        if !self.tweenable_with(other) {
            return if t < 0.5 { self.clone() } else { other.clone() };
        }
        let stops = self
            .stops
            .iter()
            .zip(&other.stops)
            .map(|(a, b)| GradientStop {
                offset: a.offset + (b.offset - a.offset) * t,
                color: lerp_color(a.color, b.color, t),
            })
            .collect();
        let (m0, m1) = (self.transform.as_coeffs(), other.transform.as_coeffs());
        let mut c = [0.0; 6];
        for i in 0..6 {
            c[i] = m0[i] + (m1[i] - m0[i]) * t;
        }
        Self {
            kind: self.kind,
            stops,
            transform: Affine::new(c),
            // The spread is a mode, not a quantity: it switches at the halfway
            // point rather than interpolating into something in between.
            spread: if t < 0.5 { self.spread } else { other.spread },
            focal: self.focal + (other.focal - self.focal) * t,
        }
    }
}

/// Where the Gradient Transform tool's grips sit, in the object's own space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientHandles {
    pub center: Point,
    pub end: Point,
    pub width: Point,
    pub focus: Point,
}

/// Straight-alpha linear interpolation between two colours.
pub fn lerp_color(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0) as f32;
    let (x, y) = (a.components, b.components);
    Color::new([
        x[0] + (y[0] - x[0]) * t,
        x[1] + (y[1] - x[1]) * t,
        x[2] + (y[2] - x[2]) * t,
        x[3] + (y[3] - x[3]) * t,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color::from_rgba8(r, g, b, 255)
    }

    fn two(from: Color, to: Color) -> Gradient {
        Gradient::new(
            GradientKind::Linear,
            vec![GradientStop::new(0.0, from), GradientStop::new(1.0, to)],
        )
    }

    #[test]
    fn samples_run_from_the_first_colour_to_the_last() {
        let g = two(rgb(0, 0, 0), rgb(255, 255, 255));
        assert_eq!(g.sample(0.0), rgb(0, 0, 0));
        assert_eq!(g.sample(1.0), rgb(255, 255, 255));
        let mid = g.sample(0.5).components;
        assert!((mid[0] - 0.5).abs() < 1e-3, "midpoint was {mid:?}");
    }

    /// Outside the ramp the end colours hold. Reflect and Repeat describe what
    /// the *renderer* does beyond the ends; the ramp itself is only defined on
    /// `0..1`, and clamping here is what keeps `sample` total.
    #[test]
    fn sampling_outside_the_ramp_holds_the_end_colours() {
        let g = two(rgb(255, 0, 0), rgb(0, 0, 255));
        assert_eq!(g.sample(-5.0), rgb(255, 0, 0));
        assert_eq!(g.sample(5.0), rgb(0, 0, 255));
    }

    #[test]
    fn stops_are_sorted_however_they_arrive() {
        let g = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(1.0, rgb(0, 0, 255)),
                GradientStop::new(0.0, rgb(255, 0, 0)),
                GradientStop::new(0.5, rgb(0, 255, 0)),
            ],
        );
        let offsets: Vec<f64> = g.stops().iter().map(|s| s.offset).collect();
        assert_eq!(offsets, vec![0.0, 0.5, 1.0]);
        assert_eq!(g.sample(0.5), rgb(0, 255, 0));
    }

    /// A file can say anything. None of these may panic, and all must produce
    /// something drawable — a gradient that renders as nothing is a silently
    /// invisible shape, which is worse than one that renders flat.
    #[test]
    fn degenerate_stop_lists_are_repaired_rather_than_refused() {
        let none = Gradient::new(GradientKind::Linear, vec![]);
        assert_eq!(none.stops().len(), 2);

        let one = Gradient::new(
            GradientKind::Radial,
            vec![GradientStop::new(0.3, rgb(10, 20, 30))],
        );
        assert_eq!(one.stops().len(), 2);
        assert_eq!(one.sample(0.0), rgb(10, 20, 30));
        assert_eq!(one.sample(1.0), rgb(10, 20, 30));

        let many = Gradient::new(
            GradientKind::Linear,
            (0..40)
                .map(|i| GradientStop::new(f64::from(i) / 39.0, rgb(i as u8, 0, 0)))
                .collect(),
        );
        assert_eq!(many.stops().len(), MAX_STOPS);
    }

    /// A NaN offset arrives from a damaged file, and `sort_by` panics on a
    /// comparator that returns no ordering. `total_cmp` does not.
    #[test]
    fn a_nan_offset_does_not_panic_the_sort() {
        let g = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop {
                    offset: f64::NAN,
                    color: rgb(1, 2, 3),
                },
                GradientStop::new(0.5, rgb(4, 5, 6)),
            ],
        );
        assert_eq!(g.stops().len(), 2);
        for s in g.stops() {
            assert!(s.offset.is_finite(), "offset {} survived", s.offset);
        }
    }

    /// Two stops at one offset are how a hard stripe is drawn. The zero-length
    /// span must not divide by zero.
    #[test]
    fn coincident_stops_are_a_hard_edge_not_a_nan() {
        let g = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(0.0, rgb(255, 0, 0)),
                GradientStop::new(0.5, rgb(255, 0, 0)),
                GradientStop::new(0.5, rgb(0, 0, 255)),
                GradientStop::new(1.0, rgb(0, 0, 255)),
            ],
        );
        assert_eq!(g.sample(0.25), rgb(255, 0, 0));
        assert_eq!(g.sample(0.75), rgb(0, 0, 255));
        for c in g.sample(0.5).components {
            assert!(c.is_finite(), "sampling the seam gave {c}");
        }
    }

    /// The weighting is the point: a gradient that is red for nine tenths of
    /// its length must not average to the midpoint of red and blue.
    #[test]
    fn the_average_is_weighted_by_how_much_of_the_ramp_each_span_holds() {
        let g = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(0.0, rgb(255, 0, 0)),
                GradientStop::new(0.9, rgb(255, 0, 0)),
                GradientStop::new(1.0, rgb(0, 0, 255)),
            ],
        );
        let avg = g.average_color().components;
        assert!(
            avg[0] > 0.9 && avg[2] < 0.1,
            "expected nearly red, got {avg:?}"
        );

        // And an even two-stop ramp averages to its midpoint.
        let even = two(rgb(0, 0, 0), rgb(255, 255, 255));
        let mid = even.average_color().components;
        assert!((mid[0] - 0.5).abs() < 1e-3, "expected mid grey, got {mid:?}");
    }

    #[test]
    fn fitting_to_a_rect_puts_the_ramp_across_it() {
        let mut g = two(rgb(0, 0, 0), rgb(255, 255, 255));
        g.fit_to(Rect::new(100.0, 50.0, 300.0, 150.0));
        let h = g.handles();
        assert!((h.center.x - 200.0).abs() < 1e-9, "centre {:?}", h.center);
        assert!((h.center.y - 100.0).abs() < 1e-9, "centre {:?}", h.center);
        // The ramp reaches the right edge, not past it.
        assert!((h.end.x - 300.0).abs() < 1e-9, "end {:?}", h.end);
    }

    /// A straight horizontal line has no height. A singular matrix renders as
    /// nothing, so the shape would silently vanish when given a gradient.
    #[test]
    fn a_flat_rect_still_gives_an_invertible_matrix() {
        let mut g = two(rgb(0, 0, 0), rgb(255, 255, 255));
        g.fit_to(Rect::new(0.0, 10.0, 100.0, 10.0));
        let det = {
            let c = g.transform.as_coeffs();
            c[0] * c[3] - c[1] * c[2]
        };
        assert!(det != 0.0, "matrix was singular");
    }

    #[test]
    fn a_gradient_travels_with_the_shape_it_is_painted_on() {
        let mut g = two(rgb(0, 0, 0), rgb(255, 255, 255));
        g.fit_to(Rect::new(0.0, 0.0, 100.0, 100.0));
        let moved = g.transformed(Affine::translate((25.0, 0.0)));
        assert!((moved.handles().center.x - 75.0).abs() < 1e-9);
    }

    #[test]
    fn tweening_interpolates_stops_and_placement() {
        let mut a = two(rgb(0, 0, 0), rgb(255, 0, 0));
        a.fit_to(Rect::new(0.0, 0.0, 100.0, 100.0));
        let mut b = two(rgb(0, 0, 0), rgb(0, 0, 255));
        b.fit_to(Rect::new(200.0, 0.0, 300.0, 100.0));

        let half = a.lerp(&b, 0.5);
        let end = half.stops()[1].color.components;
        assert!(
            (end[0] - 0.5).abs() < 1e-3 && (end[2] - 0.5).abs() < 1e-3,
            "expected halfway between red and blue, got {end:?}"
        );
        assert!((half.handles().center.x - 150.0).abs() < 1e-6);
    }

    /// Interpolating between gradients with different structures would have to
    /// invent stops. It takes the nearer end instead, and says so.
    #[test]
    fn gradients_that_do_not_correspond_do_not_tween() {
        let a = two(rgb(255, 0, 0), rgb(0, 0, 255));
        let b = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(0.0, rgb(0, 255, 0)),
                GradientStop::new(0.5, rgb(255, 255, 0)),
                GradientStop::new(1.0, rgb(0, 255, 255)),
            ],
        );
        assert!(!a.tweenable_with(&b));
        assert_eq!(a.lerp(&b, 0.25), a);
        assert_eq!(a.lerp(&b, 0.75), b);

        // Kind counts too: a linear and a radial are not the same ramp.
        let radial = Gradient::new(
            GradientKind::Radial,
            vec![
                GradientStop::new(0.0, rgb(255, 0, 0)),
                GradientStop::new(1.0, rgb(0, 0, 255)),
            ],
        );
        assert!(!a.tweenable_with(&radial));
    }

    /// Each grip moves to where it was dragged, and moving one leaves the
    /// others where they were. That second half is what makes the tool
    /// predictable — a decomposed implementation that rebuilt the matrix from
    /// an angle and a scale would quietly straighten a skewed gradient every
    /// time any grip was touched.
    #[test]
    fn dragging_a_grip_moves_that_grip_and_leaves_the_others() {
        let mut g = two(rgb(0, 0, 0), rgb(255, 255, 255));
        g.fit_to(Rect::new(0.0, 0.0, 100.0, 100.0));

        let before = g.handles();
        g.set_center(Point::new(200.0, 300.0));
        let after = g.handles();
        assert!((after.center - Point::new(200.0, 300.0)).hypot() < 1e-9);
        // The ramp travelled with it: the axis is unchanged, so the end moved
        // by exactly the same amount.
        let moved = after.center - before.center;
        assert!((after.end - (before.end + moved)).hypot() < 1e-9);

        let mut g2 = g.clone();
        g2.set_end(Point::new(400.0, 300.0));
        assert!((g2.handles().end - Point::new(400.0, 300.0)).hypot() < 1e-9);
        assert!(
            (g2.handles().center - g.handles().center).hypot() < 1e-9,
            "dragging the end must not move the centre"
        );
        assert!(
            (g2.handles().width - g.handles().width).hypot() < 1e-9,
            "dragging the end must not change the thickness"
        );

        let mut g3 = g.clone();
        g3.set_width_handle(Point::new(200.0, 400.0));
        assert!((g3.handles().width - Point::new(200.0, 400.0)).hypot() < 1e-9);
        assert!(
            (g3.handles().end - g.handles().end).hypot() < 1e-9,
            "dragging the width must not move the ramp's end"
        );
    }

    /// A skewed gradient stays skewed when it is dragged.
    #[test]
    fn dragging_does_not_straighten_a_skewed_gradient() {
        let mut g = two(rgb(0, 0, 0), rgb(255, 255, 255));
        g.transform = Affine::new([80.0, 20.0, -30.0, 60.0, 10.0, 10.0]);
        let width_before = g.handles().width - g.handles().center;

        g.set_center(Point::new(500.0, 500.0));
        let width_after = g.handles().width - g.handles().center;

        assert!(
            (width_after - width_before).hypot() < 1e-9,
            "the shear was lost: {width_before:?} became {width_after:?}"
        );
    }

    /// A grip dragged onto the centre would make the matrix singular, and a
    /// singular gradient renders as nothing at all. The drag is refused rather
    /// than allowed to make the shape vanish.
    #[test]
    fn a_grip_dragged_onto_the_centre_is_refused() {
        let mut g = two(rgb(0, 0, 0), rgb(255, 255, 255));
        g.fit_to(Rect::new(0.0, 0.0, 100.0, 100.0));
        let before = g.transform;

        let centre = g.handles().center;
        g.set_end(centre);
        g.set_width_handle(centre);

        assert_eq!(g.transform, before, "a singular matrix was allowed through");
    }

    /// The focus slides along the ramp's own axis: a drag perpendicular to it
    /// moves the hot spot not at all, and one past the end clamps.
    #[test]
    fn the_focus_slides_along_the_ramp_and_clamps_to_it() {
        let mut g = Gradient::new(
            GradientKind::Radial,
            vec![
                GradientStop::new(0.0, rgb(255, 0, 0)),
                GradientStop::new(1.0, rgb(0, 0, 255)),
            ],
        );
        g.fit_to(Rect::new(0.0, 0.0, 200.0, 200.0));
        // Centre (100, 100), axis 100 long along x.

        g.set_focus(Point::new(150.0, 100.0));
        assert!((g.focal - 0.5).abs() < 1e-9, "focal was {}", g.focal);

        // Straight up from the centre: no movement along the axis at all.
        g.set_focus(Point::new(100.0, 20.0));
        assert!(g.focal.abs() < 1e-9, "focal was {}", g.focal);

        // Far past the rim: clamped, not run off the end.
        g.set_focus(Point::new(9000.0, 100.0));
        assert!((g.focal - 1.0).abs() < 1e-9, "focal was {}", g.focal);
    }

    #[test]
    fn mapping_colours_leaves_the_placement_alone() {
        let mut g = two(rgb(10, 20, 30), rgb(40, 50, 60));
        g.fit_to(Rect::new(0.0, 0.0, 80.0, 80.0));
        let dimmed = g.map_colors(|c| c.multiply_alpha(0.5));
        assert_eq!(dimmed.transform, g.transform);
        for s in dimmed.stops() {
            assert!((s.color.components[3] - 0.5).abs() < 1e-3);
        }
    }
}
