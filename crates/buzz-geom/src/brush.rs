//! Brushes: turning a dragged pointer into artwork.
//!
//! Three things live here, in the order a stroke passes through them.
//!
//! 1. **Conditioning.** Raw pointer samples are noisy, unevenly spaced, and
//!    arrive far faster than the artwork needs. [`condition`] decimates and
//!    smooths them into something a curve can be built on.
//! 2. **The fluid brush.** [`fluid_outline`] builds a filled outline whose
//!    width varies along the stroke — with pressure when the device reports
//!    it, and with speed when it does not. This is what makes a stroke look
//!    drawn rather than extruded.
//! 3. **Pattern and art brushes.** [`stamp_along`] repeats a source shape
//!    along the stroke, oriented to its tangent, or stretches one copy over
//!    the whole length.
//!
//! # Why a Catmull-Rom centreline, not a curve fit
//!
//! Curves here are built by converting the sample points to cubic Béziers with
//! the Catmull-Rom construction: a local, closed-form formula that passes
//! exactly through every input point. It is not a *fit*, so it cannot diverge
//! from its input — which matters, because CP-1.1c found kurbo's fitter
//! turning a correct polygon spanning `-5..105` into a curve spanning
//! `-5..1071` when handed a path that was not smooth. Freehand input is never
//! smooth. A construction that cannot overshoot is worth more here than one
//! that produces fewer segments.
//!
//! # Budgets are not optional
//!
//! Every entry point takes a [`BrushBudget`] and respects it. A stroke dragged
//! across the pasteboard at a spacing of 0.1 units asks for a million stamps;
//! generating them would freeze the window, and drawing them every frame after
//! that would keep it frozen. So the budget *widens the spacing* rather than
//! refusing or truncating — the user gets the whole stroke, at the density the
//! machine can carry, and [`BrushOutput`] says what was changed so the caller
//! can tell them.

use kurbo::{BezPath, ParamCurve, ParamCurveArclen, PathSeg, Point, Shape as _, Vec2};

/// One sample from the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeSample {
    pub point: Point,
    /// `0.0..=1.0`. Devices without a pressure sensor report `1.0`.
    pub pressure: f64,
    /// Seconds since the stroke began. Used for speed, so it must be real
    /// elapsed time rather than a frame count — a stroke drawn during a stall
    /// is not a fast stroke.
    pub time: f64,
}

impl StrokeSample {
    /// A sample from a device with no pressure sensor.
    pub fn new(point: Point, time: f64) -> Self {
        Self {
            point,
            pressure: 1.0,
            time,
        }
    }

    pub fn with_pressure(point: Point, pressure: f64, time: f64) -> Self {
        Self {
            point,
            pressure: pressure.clamp(0.0, 1.0),
            time,
        }
    }
}

/// What drives the width along a stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidthResponse {
    /// Constant width, which is what Animate's plain Brush does.
    Uniform,
    /// Follow the device's pressure.
    Pressure,
    /// Follow speed: fast strokes thin out, slow ones stay fat. This is the
    /// fallback on a mouse, which has no pressure to report, and it is what
    /// makes a mouse-drawn line look drawn rather than extruded.
    ///
    /// `reference_speed` is the speed, in document units per second, at which
    /// the stroke reaches its narrowest.
    Speed { reference_speed: f64 },
}

/// How a brush turns a stroke into artwork.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushProfile {
    /// Width at full pressure, in document units.
    pub width: f64,
    /// Narrowest width as a fraction of [`Self::width`].
    pub min_ratio: f64,
    pub response: WidthResponse,
    /// `0.0` follows the pointer exactly; `1.0` smooths hard. Animate's
    /// Smoothing setting is the same idea.
    pub smoothing: f64,
    /// How much of the stroke, as a fraction of its length, tapers to a point
    /// at each end. `0.0` gives round caps instead.
    pub taper: f64,
}

impl Default for BrushProfile {
    fn default() -> Self {
        Self {
            width: 10.0,
            min_ratio: 0.35,
            response: WidthResponse::Speed {
                reference_speed: 900.0,
            },
            smoothing: 0.5,
            taper: 0.12,
        }
    }
}

impl BrushProfile {
    /// The width this brush paints for a given pressure and speed.
    fn width_at(&self, pressure: f64, speed: f64) -> f64 {
        let min = self.min_ratio.clamp(0.0, 1.0);
        let factor = match self.response {
            WidthResponse::Uniform => 1.0,
            WidthResponse::Pressure => pressure.clamp(0.0, 1.0),
            WidthResponse::Speed { reference_speed } => {
                let reference = reference_speed.max(1e-6);
                // Fast is thin. Clamped so an implausible speed spike — the
                // first sample after a stall — cannot invert the width.
                1.0 - (speed / reference).clamp(0.0, 1.0)
            }
        };
        self.width * (min + (1.0 - min) * factor)
    }
}

/// Limits that keep a brush from generating more work than the machine can do.
///
/// These are not tuning knobs to be raised when something looks sparse. They
/// are the difference between a slow stroke and a frozen window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushBudget {
    /// Samples kept after decimation.
    pub max_samples: usize,
    /// Stamps a pattern brush may place along one stroke.
    pub max_stamps: usize,
    /// Path elements one stroke may produce.
    pub max_elements: usize,
    /// Samples closer together than this are dropped, in document units.
    pub min_spacing: f64,
}

impl Default for BrushBudget {
    fn default() -> Self {
        Self {
            // A stroke across a 4K stage at one sample per unit is ~4000; ten
            // times that is generous and still bounded.
            max_samples: 40_000,
            max_stamps: 4_000,
            max_elements: 200_000,
            min_spacing: 0.75,
        }
    }
}

impl BrushBudget {
    /// The budget for a live preview, which is redrawn every frame while the
    /// user is still dragging and so must be much cheaper than the real thing.
    pub fn preview() -> Self {
        Self {
            max_samples: 4_000,
            max_stamps: 120,
            max_elements: 8_000,
            min_spacing: 1.5,
        }
    }
}

/// What a brush produced, and what the budget had to change to get it.
#[derive(Debug, Clone, PartialEq)]
pub struct BrushOutput {
    pub path: BezPath,
    /// Stamps actually placed. Zero for the fluid brush.
    pub stamps: usize,
    /// Set when the requested spacing was too fine for the budget and had to
    /// be widened. The stroke still covers its whole length.
    pub spacing_widened: bool,
    /// Set when output was cut short at [`BrushBudget::max_elements`]. Unlike
    /// widened spacing this *does* lose part of the stroke, so a caller that
    /// reports anything should report this.
    pub truncated: bool,
}

impl BrushOutput {
    fn plain(path: BezPath) -> Self {
        Self {
            path,
            stamps: 0,
            spacing_widened: false,
            truncated: false,
        }
    }

    /// Did the budget change what the user asked for?
    pub fn is_exact(&self) -> bool {
        !self.spacing_widened && !self.truncated
    }
}

// ---------------------------------------------------------------------------
// Conditioning
// ---------------------------------------------------------------------------

/// Decimate and smooth raw pointer samples.
///
/// Two separate jobs, done in this order for a reason. Decimation first,
/// because a pointer reporting at 1000 Hz produces runs of near-identical
/// samples whose tangents are numerical noise; smoothing that noise averages
/// it into the result instead of removing it. Once the samples are spaced,
/// smoothing has real motion to work on.
pub fn condition(
    samples: &[StrokeSample],
    smoothing: f64,
    budget: &BrushBudget,
) -> Vec<StrokeSample> {
    let decimated = decimate(samples, budget);
    smooth_samples(&decimated, smoothing)
}

/// Drop samples that are too close to the one before to carry information.
///
/// The last sample is always kept: it is where the user let go, and a stroke
/// that stops short of the pointer feels broken.
pub fn decimate(samples: &[StrokeSample], budget: &BrushBudget) -> Vec<StrokeSample> {
    if samples.len() < 2 {
        return samples.to_vec();
    }

    let mut out = Vec::with_capacity(samples.len().min(budget.max_samples));
    out.push(samples[0]);

    for sample in &samples[1..] {
        let last = out.last().expect("just pushed");
        if (sample.point - last.point).hypot() >= budget.min_spacing {
            out.push(*sample);
        }
        if out.len() >= budget.max_samples {
            break;
        }
    }

    // Keep the true end point, replacing the last kept sample if it is a
    // near-duplicate rather than appending a zero-length segment.
    let final_sample = *samples.last().expect("checked non-empty");
    match out.last() {
        Some(last) if (final_sample.point - last.point).hypot() < budget.min_spacing => {
            if out.len() > 1 {
                *out.last_mut().expect("non-empty") = final_sample;
            }
        }
        _ => out.push(final_sample),
    }

    out
}

/// Smooth the sample positions, leaving pressure and time alone.
///
/// A symmetric weighted average of each point with its neighbours, applied
/// twice at full strength. Symmetric matters: a one-sided filter lags the
/// pointer, and a brush that lags feels broken in a way that is hard to name
/// but immediately obvious. The endpoints are never moved, so the stroke still
/// starts and ends where the user put it.
pub fn smooth_samples(samples: &[StrokeSample], smoothing: f64) -> Vec<StrokeSample> {
    let strength = smoothing.clamp(0.0, 1.0);
    if samples.len() < 3 || strength <= 0.0 {
        return samples.to_vec();
    }

    let passes = if strength > 0.66 {
        3
    } else if strength > 0.33 {
        2
    } else {
        1
    };
    // Within a pass, strength sets how far each point moves toward the average
    // of its neighbours.
    let alpha = strength.min(1.0);

    let mut current = samples.to_vec();
    for _ in 0..passes {
        let previous = current.clone();
        for i in 1..previous.len() - 1 {
            let neighbour_mean =
                (previous[i - 1].point.to_vec2() + previous[i + 1].point.to_vec2()) / 2.0;
            let moved = previous[i].point.to_vec2().lerp(neighbour_mean, alpha);
            current[i].point = moved.to_point();
        }
    }
    current
}

// ---------------------------------------------------------------------------
// Centreline
// ---------------------------------------------------------------------------

/// A smooth curve through every sample, for previews and for pattern brushes.
///
/// Uses the Catmull-Rom construction, so the curve passes exactly through the
/// input points and cannot overshoot the way a fitted curve can.
pub fn centreline(samples: &[StrokeSample]) -> BezPath {
    let points: Vec<Point> = samples.iter().map(|s| s.point).collect();
    catmull_rom(&points)
}

/// Cubic Béziers through a point sequence.
///
/// The tangent at each point is half the vector between its neighbours; the
/// control points sit a third of the way along those tangents. That is the
/// standard uniform Catmull-Rom-to-Bézier conversion, and it is exact — no
/// iteration, no error metric, no possibility of divergence.
pub fn catmull_rom(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    match points.len() {
        0 => return path,
        1 => {
            path.move_to(points[0]);
            return path;
        }
        2 => {
            path.move_to(points[0]);
            path.line_to(points[1]);
            return path;
        }
        _ => {}
    }

    path.move_to(points[0]);
    for i in 0..points.len() - 1 {
        // Neighbours, with the ends duplicated so the first and last segments
        // have a tangent to work with.
        let p0 = points[i.saturating_sub(1)];
        let p1 = points[i];
        let p2 = points[i + 1];
        let p3 = points[(i + 2).min(points.len() - 1)];

        let c1 = p1 + (p2 - p0) / 6.0;
        let c2 = p2 - (p3 - p1) / 6.0;
        path.curve_to(c1, c2, p2);
    }
    path
}

// ---------------------------------------------------------------------------
// The fluid brush
// ---------------------------------------------------------------------------

/// Build a filled outline whose width varies along the stroke.
///
/// The result is a single closed path, filled with the non-zero rule. A stroke
/// that doubles back on itself therefore *merges* with itself rather than
/// cancelling out, which is what paint does and what every drawing application
/// does. No attempt is made to resolve those self-intersections: doing so
/// would cost a boolean operation per stroke and would make no visible
/// difference under a non-zero fill.
pub fn fluid_outline(
    samples: &[StrokeSample],
    profile: &BrushProfile,
    budget: &BrushBudget,
) -> BrushOutput {
    let conditioned = condition(samples, profile.smoothing, budget);
    if conditioned.len() < 2 {
        // A tap, not a drag. Animate paints a dot, so we do too.
        if let Some(sample) = conditioned.first() {
            let radius = profile.width_at(sample.pressure, 0.0) / 2.0;
            if radius > 0.0 {
                return BrushOutput::plain(kurbo::Circle::new(sample.point, radius).to_path(1e-3));
            }
        }
        return BrushOutput::plain(BezPath::new());
    }

    let widths = widths_along(&conditioned, profile);
    let normals = normals_along(&conditioned);

    // Both sides of the stroke, as point sequences.
    let mut left = Vec::with_capacity(conditioned.len());
    let mut right = Vec::with_capacity(conditioned.len());
    for i in 0..conditioned.len() {
        let half = widths[i] / 2.0;
        let offset = normals[i] * half;
        left.push(conditioned[i].point + offset);
        right.push(conditioned[i].point - offset);
    }

    // Down one side and back the other. `right` is reversed so the outline is
    // a single continuous loop rather than two crossing strands.
    right.reverse();

    let mut path = catmull_rom(&left);
    let back = catmull_rom(&right);

    // Join the two sides. With a taper the ends have collapsed to a point and
    // a straight join is invisible; without one this is where a cap belongs.
    if profile.taper <= 0.0 {
        append_cap(
            &mut path,
            conditioned[conditioned.len() - 1].point,
            left[left.len() - 1],
            right[0],
        );
    }
    append_without_move(&mut path, &back);
    if profile.taper <= 0.0 {
        append_cap(
            &mut path,
            conditioned[0].point,
            right[right.len() - 1],
            left[0],
        );
    }
    path.close_path();

    let truncated = path.elements().len() > budget.max_elements;
    if truncated {
        path = truncate(path, budget.max_elements);
    }

    BrushOutput {
        path,
        stamps: 0,
        spacing_widened: false,
        truncated,
    }
}

/// Width at every sample, including the end tapers.
fn widths_along(samples: &[StrokeSample], profile: &BrushProfile) -> Vec<f64> {
    let n = samples.len();
    let mut widths = Vec::with_capacity(n);

    // Cumulative distance, so the taper is measured in length rather than in
    // sample count — a slow stroke has more samples per unit and would
    // otherwise taper over a different distance.
    let mut distances = Vec::with_capacity(n);
    let mut total = 0.0;
    distances.push(0.0);
    for i in 1..n {
        total += (samples[i].point - samples[i - 1].point).hypot();
        distances.push(total);
    }

    for i in 0..n {
        // Speed from the samples either side, so a single late sample does not
        // produce a spike.
        let speed = if n < 2 {
            0.0
        } else {
            let (a, b) = (samples[i.saturating_sub(1)], samples[(i + 1).min(n - 1)]);
            let dt = b.time - a.time;
            let dd = (b.point - a.point).hypot();
            if dt > 1e-6 { dd / dt } else { 0.0 }
        };

        let mut width = profile.width_at(samples[i].pressure, speed);

        // Taper both ends towards a point.
        if profile.taper > 0.0 && total > 0.0 {
            let taper_length = (profile.taper.clamp(0.0, 0.5)) * total;
            if taper_length > 0.0 {
                let from_start = distances[i];
                let from_end = total - distances[i];
                let nearest = from_start.min(from_end);
                if nearest < taper_length {
                    // Square root rather than linear: a linear taper reads as a
                    // wedge, and a brush end is closer to an ellipse.
                    width *= (nearest / taper_length).sqrt();
                }
            }
        }

        widths.push(width.max(0.0));
    }
    widths
}

/// Unit normals at every sample.
fn normals_along(samples: &[StrokeSample]) -> Vec<Vec2> {
    let n = samples.len();
    let mut normals = Vec::with_capacity(n);
    for i in 0..n {
        // Central difference, which keeps the normal continuous through a
        // corner instead of flipping between the two incident edges.
        let before = samples[i.saturating_sub(1)].point;
        let after = samples[(i + 1).min(n - 1)].point;
        let tangent = after - before;
        let length = tangent.hypot();
        normals.push(if length > 1e-9 {
            // Perpendicular, left-hand side.
            Vec2::new(-tangent.y / length, tangent.x / length)
        } else {
            Vec2::new(0.0, 1.0)
        });
    }
    normals
}

/// A semicircular cap from `from` to `to` around `centre`.
fn append_cap(path: &mut BezPath, centre: Point, from: Point, to: Point) {
    let radius = (from - centre).hypot();
    if radius <= 1e-9 {
        path.line_to(to);
        return;
    }
    // A half circle as two cubics, using the standard 4/3·tan(θ/4) handle
    // length for a quarter turn.
    let k = 0.5522847498307936 * radius;
    let out = (from - centre) / radius;
    let across = Vec2::new(-out.y, out.x);
    let mid = centre + across * radius;

    path.curve_to(from + across * k, mid + out * k, mid);
    path.curve_to(mid - out * k, to + across * k, to);
}

/// Append `other`, turning its leading `MoveTo` into a `LineTo` so the result
/// stays one subpath.
fn append_without_move(path: &mut BezPath, other: &BezPath) {
    for (i, element) in other.elements().iter().enumerate() {
        match (i, element) {
            (0, kurbo::PathEl::MoveTo(p)) => path.line_to(*p),
            (_, element) => path.push(*element),
        }
    }
}

/// Cut a path down to a maximum element count, keeping it closed.
fn truncate(path: BezPath, max: usize) -> BezPath {
    let mut out = BezPath::from_vec(path.elements().iter().take(max).copied().collect());
    if !matches!(out.elements().last(), Some(kurbo::PathEl::ClosePath)) {
        out.close_path();
    }
    out
}

// ---------------------------------------------------------------------------
// Pattern and art brushes
// ---------------------------------------------------------------------------

/// How a source shape is laid along a stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatternFit {
    /// Animate's **pattern brush**: repeat the source at a fixed spacing.
    Repeat {
        /// Distance between stamp origins, in document units.
        spacing: f64,
    },
    /// Animate's **art brush**: one copy, stretched over the whole stroke.
    Stretch,
}

/// Repeat or stretch a source shape along a path.
///
/// The source is interpreted in its own coordinates: its bounding box is
/// mapped so that x runs along the stroke and y across it, which is what makes
/// a source drawn left-to-right come out following the curve.
///
/// # Cost
///
/// This is the expensive brush, and the one most able to hang an application,
/// so the loop is built to avoid the obvious quadratic. Arc lengths for the
/// path's segments are measured **once** into a cumulative table; each stamp
/// then finds its position with a binary search over that table. The naive
/// approach — asking the path for the point at each fraction — re-measures
/// every segment for every stamp, which is `O(stamps x segments)` and is
/// exactly how this kind of feature ends up freezing the window on a long
/// stroke.
pub fn stamp_along(
    path: &BezPath,
    source: &BezPath,
    fit: PatternFit,
    budget: &BrushBudget,
) -> BrushOutput {
    let table = ArcTable::build(path);
    if table.total <= 0.0 || source.elements().is_empty() {
        return BrushOutput::plain(BezPath::new());
    }

    let bounds = source.bounding_box();
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return BrushOutput::plain(BezPath::new());
    }

    match fit {
        PatternFit::Stretch => {
            // One stamp covering the whole stroke. Bending the source along
            // the curve would need a warp; scaling it to the arc length and
            // orienting it to the overall direction is what an art brush on a
            // gentle curve looks like, and it is honest about not warping.
            let start = table.point_at(0.0);
            let end = table.point_at(table.total);
            let direction = end - start;
            let angle = if direction.hypot() > 1e-9 {
                direction.atan2()
            } else {
                0.0
            };

            let scale_x = table.total / bounds.width();
            let transform = kurbo::Affine::translate(start.to_vec2())
                * kurbo::Affine::rotate(angle)
                * kurbo::Affine::scale_non_uniform(scale_x, 1.0)
                * kurbo::Affine::translate(-bounds.origin().to_vec2());

            let mut out = BezPath::new();
            append_all(&mut out, &(transform * source.clone()));
            let truncated = out.elements().len() > budget.max_elements;
            if truncated {
                out = truncate(out, budget.max_elements);
            }
            BrushOutput {
                path: out,
                stamps: 1,
                spacing_widened: false,
                truncated,
            }
        }

        PatternFit::Repeat { spacing } => {
            let requested = spacing.max(1e-3);

            // The budget widens the spacing rather than dropping the tail, so
            // the stroke still runs its whole length.
            let wanted = (table.total / requested).floor() as usize + 1;
            let (spacing, widened) = if wanted > budget.max_stamps {
                (table.total / budget.max_stamps.max(1) as f64, true)
            } else {
                (requested, false)
            };

            let count = ((table.total / spacing).floor() as usize + 1).min(budget.max_stamps);
            let mut out = BezPath::new();
            let mut truncated = false;

            for i in 0..count {
                if out.elements().len() >= budget.max_elements {
                    truncated = true;
                    break;
                }
                let distance = i as f64 * spacing;
                let (point, tangent) = table.frame_at(distance);
                let angle = if tangent.hypot() > 1e-9 {
                    tangent.atan2()
                } else {
                    0.0
                };

                // Stamps are centred on the path, which is what makes a
                // pattern read as running *along* the stroke rather than
                // hanging off one side of it.
                let transform = kurbo::Affine::translate(point.to_vec2())
                    * kurbo::Affine::rotate(angle)
                    * kurbo::Affine::translate(-bounds.center().to_vec2());
                append_all(&mut out, &(transform * source.clone()));
            }

            BrushOutput {
                path: out,
                stamps: count,
                spacing_widened: widened,
                truncated,
            }
        }
    }
}

fn append_all(path: &mut BezPath, other: &BezPath) {
    for element in other.elements() {
        path.push(*element);
    }
}

/// Segment arc lengths, measured once.
///
/// Building this is `O(segments)`; every query afterwards is `O(log segments)`.
struct ArcTable {
    segments: Vec<PathSeg>,
    /// Cumulative length *after* each segment; `cumulative[i]` is the distance
    /// from the start of the path to the end of `segments[i]`.
    cumulative: Vec<f64>,
    total: f64,
    accuracy: f64,
}

impl ArcTable {
    fn build(path: &BezPath) -> Self {
        let segments: Vec<PathSeg> = path.segments().collect();
        // Relative to the path's own size, so a tiny stamp and a stage-sized
        // stroke both measure sensibly.
        let extent = path.bounding_box();
        let accuracy = (extent.width().max(extent.height()) * 1e-4).clamp(1e-6, 0.1);

        let mut cumulative = Vec::with_capacity(segments.len());
        let mut total = 0.0;
        for segment in &segments {
            total += segment.arclen(accuracy);
            cumulative.push(total);
        }

        Self {
            segments,
            cumulative,
            total,
            accuracy,
        }
    }

    /// Which segment contains `distance`, and how far into it.
    fn locate(&self, distance: f64) -> Option<(usize, f64)> {
        if self.segments.is_empty() {
            return None;
        }
        let distance = distance.clamp(0.0, self.total);
        let index = self
            .cumulative
            .partition_point(|end| *end < distance)
            .min(self.segments.len() - 1);
        let before = if index == 0 {
            0.0
        } else {
            self.cumulative[index - 1]
        };
        Some((index, distance - before))
    }

    fn point_at(&self, distance: f64) -> Point {
        match self.locate(distance) {
            Some((index, into)) => {
                let segment = self.segments[index];
                let t = segment.inv_arclen(into, self.accuracy);
                segment.eval(t)
            }
            None => Point::ZERO,
        }
    }

    /// Position and tangent at `distance`.
    fn frame_at(&self, distance: f64) -> (Point, Vec2) {
        match self.locate(distance) {
            Some((index, into)) => {
                let segment = self.segments[index];
                let t = segment.inv_arclen(into, self.accuracy);
                let point = segment.eval(t);
                // Differentiating a `PathSeg` needs a match per kind, and a
                // short chord is both simpler and stable at the ends where the
                // derivative can vanish.
                let step = 1e-3;
                let ahead = segment.eval((t + step).min(1.0));
                let behind = segment.eval((t - step).max(0.0));
                let tangent = ahead - behind;
                (
                    point,
                    if tangent.hypot() > 1e-12 {
                        tangent
                    } else {
                        Vec2::new(1.0, 0.0)
                    },
                )
            }
            None => (Point::ZERO, Vec2::new(1.0, 0.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn drag(from: Point, to: Point, count: usize) -> Vec<StrokeSample> {
        (0..count)
            .map(|i| {
                let t = i as f64 / (count - 1).max(1) as f64;
                StrokeSample::new(from.lerp(to, t), t * 0.5)
            })
            .collect()
    }

    fn unit_square() -> BezPath {
        kurbo::Rect::new(0.0, -5.0, 10.0, 5.0).to_path(1e-9)
    }

    // -- conditioning ------------------------------------------------------

    #[test]
    fn decimation_drops_crowded_samples_but_keeps_the_ends() {
        // 1000 samples over 10 units: far denser than the 0.75 minimum.
        let samples = drag(Point::ZERO, Point::new(10.0, 0.0), 1000);
        let kept = decimate(&samples, &BrushBudget::default());

        assert!(
            kept.len() < 30,
            "1000 samples over 10 units kept {}",
            kept.len()
        );
        assert_eq!(kept.first().unwrap().point, Point::ZERO);
        assert_eq!(
            kept.last().unwrap().point,
            Point::new(10.0, 0.0),
            "the point the user let go at must survive"
        );
    }

    /// The sample budget is a hard stop, not a suggestion.
    #[test]
    fn decimation_never_exceeds_the_sample_budget() {
        let samples = drag(Point::ZERO, Point::new(100_000.0, 0.0), 100_000);
        let budget = BrushBudget {
            max_samples: 500,
            ..BrushBudget::default()
        };
        let kept = decimate(&samples, &budget);
        assert!(kept.len() <= budget.max_samples + 1, "kept {}", kept.len());
    }

    /// Smoothing must not drag the stroke away from where it was drawn.
    #[test]
    fn smoothing_leaves_the_endpoints_exactly_where_they_were() {
        let mut samples = drag(Point::ZERO, Point::new(50.0, 0.0), 20);
        samples[10].point.y = 30.0; // a spike in the middle

        let smoothed = smooth_samples(&samples, 1.0);
        assert_eq!(
            smoothed.first().unwrap().point,
            samples.first().unwrap().point
        );
        assert_eq!(
            smoothed.last().unwrap().point,
            samples.last().unwrap().point
        );
        assert!(
            smoothed[10].point.y < 30.0,
            "the spike should have been pulled in, got {}",
            smoothed[10].point.y
        );
    }

    #[test]
    fn no_smoothing_changes_nothing() {
        let samples = drag(Point::ZERO, Point::new(50.0, 20.0), 12);
        assert_eq!(smooth_samples(&samples, 0.0), samples);
    }

    // -- centreline ---------------------------------------------------------

    /// The reason for choosing Catmull-Rom over a fit: it cannot overshoot.
    /// CP-1.1c found kurbo's fitter turning a path spanning -5..105 into one
    /// spanning -5..1071 on input that was not smooth, and freehand input
    /// never is.
    #[test]
    fn the_centreline_passes_through_its_points_and_never_overshoots() {
        let points = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 40.0),
            Point::new(20.0, 0.0),
            Point::new(30.0, 40.0),
            Point::new(40.0, 0.0),
        ];
        let path = catmull_rom(&points);
        let bounds = path.bounding_box();

        assert!(
            bounds.x0 >= -1.0 && bounds.x1 <= 41.0,
            "x should stay near the input range, got {bounds:?}"
        );
        assert!(
            bounds.y0 >= -12.0 && bounds.y1 <= 52.0,
            "y may overshoot a little at a cusp but not wildly, got {bounds:?}"
        );

        // And it is made of curves, not a polyline.
        assert!(
            path.elements()
                .iter()
                .any(|e| matches!(e, kurbo::PathEl::CurveTo(..))),
            "the centreline should be curved"
        );
    }

    #[test]
    fn a_two_point_centreline_is_a_line_and_a_one_point_one_is_empty_of_segments() {
        let line = catmull_rom(&[Point::ZERO, Point::new(5.0, 5.0)]);
        assert_eq!(line.segments().count(), 1);

        let dot = catmull_rom(&[Point::ZERO]);
        assert_eq!(dot.segments().count(), 0);

        assert!(catmull_rom(&[]).elements().is_empty());
    }

    // -- the fluid brush ----------------------------------------------------

    #[test]
    fn a_fluid_stroke_is_a_closed_filled_outline() {
        let samples = drag(Point::ZERO, Point::new(100.0, 0.0), 60);
        let out = fluid_outline(&samples, &BrushProfile::default(), &BrushBudget::default());

        assert!(out.is_exact());
        assert!(
            out.path
                .elements()
                .iter()
                .any(|e| matches!(e, kurbo::PathEl::ClosePath)),
            "a brush stroke is a filled outline, so it must close"
        );

        let bounds = out.path.bounding_box();
        assert!(
            bounds.width() >= 99.0,
            "it should span the drag: {bounds:?}"
        );
        assert!(bounds.height() > 0.0, "and have width");
    }

    /// The whole point of a *fluid* brush: a fast stroke is thinner than a
    /// slow one drawn over the same distance.
    #[test]
    fn a_fast_stroke_comes_out_thinner_than_a_slow_one() {
        let profile = BrushProfile {
            taper: 0.0,
            smoothing: 0.0,
            response: WidthResponse::Speed {
                reference_speed: 500.0,
            },
            ..BrushProfile::default()
        };

        let make = |seconds: f64| -> f64 {
            let samples: Vec<StrokeSample> = (0..80)
                .map(|i| {
                    let t = i as f64 / 79.0;
                    StrokeSample::new(Point::new(t * 200.0, 0.0), t * seconds)
                })
                .collect();
            fluid_outline(&samples, &profile, &BrushBudget::default())
                .path
                .bounding_box()
                .height()
        };

        let slow = make(4.0); // 50 units/s
        let fast = make(0.2); // 1000 units/s

        assert!(
            fast < slow * 0.75,
            "a fast stroke should be markedly thinner: fast {fast:.2} vs slow {slow:.2}"
        );
        assert!(fast > 0.0, "but never vanish");
    }

    #[test]
    fn pressure_drives_the_width_when_the_device_reports_it() {
        let profile = BrushProfile {
            response: WidthResponse::Pressure,
            taper: 0.0,
            smoothing: 0.0,
            width: 20.0,
            min_ratio: 0.1,
        };

        let at = |pressure: f64| -> f64 {
            let samples: Vec<StrokeSample> = (0..40)
                .map(|i| {
                    let t = i as f64 / 39.0;
                    StrokeSample::with_pressure(Point::new(t * 100.0, 0.0), pressure, t)
                })
                .collect();
            fluid_outline(&samples, &profile, &BrushBudget::default())
                .path
                .bounding_box()
                .height()
        };

        assert!(
            at(1.0) > at(0.5) * 1.5,
            "full pressure must be clearly fatter"
        );
        assert!(at(0.0) > 0.0, "and zero pressure still leaves a mark");
    }

    /// A tap with no drag should leave a dot, as Animate does — not nothing.
    #[test]
    fn a_single_tap_paints_a_dot() {
        let samples = vec![StrokeSample::new(Point::new(5.0, 5.0), 0.0)];
        let out = fluid_outline(&samples, &BrushProfile::default(), &BrushBudget::default());

        assert!(
            !out.path.elements().is_empty(),
            "a tap should paint something"
        );
        let bounds = out.path.bounding_box();
        assert!(bounds.width() > 0.0 && bounds.height() > 0.0);
    }

    #[test]
    fn tapered_ends_are_narrower_than_the_middle() {
        let profile = BrushProfile {
            taper: 0.25,
            smoothing: 0.0,
            response: WidthResponse::Uniform,
            width: 20.0,
            ..BrushProfile::default()
        };
        let samples = drag(Point::ZERO, Point::new(200.0, 0.0), 100);
        let out = fluid_outline(&samples, &profile, &BrushBudget::default());

        // Sample the outline's vertical extent near an end and in the middle.
        let extent_near = |x: f64| -> f64 {
            let ys: Vec<f64> = out
                .path
                .elements()
                .iter()
                .filter_map(|e| match e {
                    kurbo::PathEl::MoveTo(p)
                    | kurbo::PathEl::LineTo(p)
                    | kurbo::PathEl::CurveTo(_, _, p) => Some(*p),
                    _ => None,
                })
                .filter(|p| (p.x - x).abs() < 5.0)
                .map(|p| p.y)
                .collect();
            match (
                ys.iter().cloned().fold(f64::INFINITY, f64::min),
                ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            ) {
                (lo, hi) if lo.is_finite() && hi.is_finite() => hi - lo,
                _ => 0.0,
            }
        };

        assert!(
            extent_near(2.0) < extent_near(100.0),
            "the start should be narrower than the middle"
        );
        assert!(
            extent_near(198.0) < extent_near(100.0),
            "and so should the end"
        );
    }

    // -- pattern and art brushes -------------------------------------------

    #[test]
    fn a_pattern_brush_repeats_its_source_along_the_stroke() {
        let stroke = catmull_rom(&[
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(100.0, 0.0),
        ]);
        let out = stamp_along(
            &stroke,
            &unit_square(),
            PatternFit::Repeat { spacing: 10.0 },
            &BrushBudget::default(),
        );

        assert!(out.is_exact());
        assert_eq!(
            out.stamps, 11,
            "100 units at 10 apart, inclusive of both ends"
        );

        let bounds = out.path.bounding_box();
        assert!(
            bounds.width() >= 100.0,
            "the stamps should span the stroke: {bounds:?}"
        );
    }

    /// A stamp must turn to follow the stroke, or a pattern brush is just a
    /// row of identical shapes.
    #[test]
    fn stamps_rotate_to_follow_the_tangent() {
        // A stroke going straight down, so a source that is wide in x should
        // come out tall in y.
        let stroke = catmull_rom(&[
            Point::new(0.0, 0.0),
            Point::new(0.0, 50.0),
            Point::new(0.0, 100.0),
        ]);
        let source = kurbo::Rect::new(-10.0, -1.0, 10.0, 1.0).to_path(1e-9);

        let out = stamp_along(
            &stroke,
            &source,
            PatternFit::Repeat { spacing: 25.0 },
            &BrushBudget::default(),
        );
        let bounds = out.path.bounding_box();

        assert!(
            bounds.width() < 5.0,
            "a 20-wide source on a vertical stroke should be narrow in x: {bounds:?}"
        );
        assert!(bounds.height() > 100.0, "and tall in y: {bounds:?}");
    }

    #[test]
    fn an_art_brush_stretches_one_copy_over_the_whole_stroke() {
        let stroke = catmull_rom(&[
            Point::new(0.0, 0.0),
            Point::new(60.0, 0.0),
            Point::new(120.0, 0.0),
        ]);
        let out = stamp_along(
            &stroke,
            &unit_square(),
            PatternFit::Stretch,
            &BrushBudget::default(),
        );

        assert_eq!(out.stamps, 1, "an art brush places exactly one stamp");
        let bounds = out.path.bounding_box();
        assert!(
            bounds.width() >= 118.0,
            "the single copy should be stretched to the stroke: {bounds:?}"
        );
        assert!(
            (bounds.height() - 10.0).abs() < 1.0,
            "and keep its own thickness: {bounds:?}"
        );
    }

    // -- budgets ------------------------------------------------------------

    /// The requirement that matters: an absurd spacing must not try to place a
    /// million stamps. The spacing widens, the stroke still covers its length,
    /// and the caller is told.
    #[test]
    fn an_absurd_spacing_widens_rather_than_placing_a_million_stamps() {
        let stroke = catmull_rom(&[
            Point::new(0.0, 0.0),
            Point::new(5_000.0, 0.0),
            Point::new(10_000.0, 0.0),
        ]);
        let budget = BrushBudget::default();

        let started = Instant::now();
        let out = stamp_along(
            &stroke,
            &unit_square(),
            PatternFit::Repeat { spacing: 0.01 }, // asks for a million
            &budget,
        );
        let elapsed = started.elapsed();

        assert!(
            out.spacing_widened,
            "the budget should have widened the spacing"
        );
        assert!(
            out.stamps <= budget.max_stamps,
            "placed {} stamps against a budget of {}",
            out.stamps,
            budget.max_stamps
        );
        assert!(
            elapsed.as_millis() < 500,
            "a pathological request took {elapsed:?}; it must stay interactive"
        );

        // The whole stroke is still covered, which is why widening beats
        // truncating.
        let bounds = out.path.bounding_box();
        assert!(bounds.width() >= 9_900.0, "coverage: {bounds:?}");
    }

    /// The preview budget must be far cheaper than the real one, because it
    /// runs on every mouse move while the stroke is still being drawn.
    #[test]
    fn the_preview_budget_is_much_cheaper_than_the_committed_one() {
        let preview = BrushBudget::preview();
        let full = BrushBudget::default();

        assert!(preview.max_stamps * 10 <= full.max_stamps);
        assert!(preview.max_elements < full.max_elements);
        assert!(preview.min_spacing >= full.min_spacing);
    }

    #[test]
    fn an_element_budget_truncates_and_says_so() {
        let stroke = catmull_rom(&[
            Point::new(0.0, 0.0),
            Point::new(500.0, 0.0),
            Point::new(1000.0, 0.0),
        ]);
        let budget = BrushBudget {
            max_elements: 40,
            ..BrushBudget::default()
        };
        let out = stamp_along(
            &stroke,
            &unit_square(),
            PatternFit::Repeat { spacing: 5.0 },
            &budget,
        );

        assert!(out.truncated, "it should admit to cutting the stroke short");
        assert!(!out.is_exact());
        assert!(out.path.elements().len() <= budget.max_elements + 6);
    }

    // -- performance --------------------------------------------------------

    /// The stamping loop must not be quadratic in the stroke's segment count.
    /// A naive implementation asks the path for the point at each fraction,
    /// which re-measures every segment for every stamp; on a long stroke that
    /// is what freezes the window.
    #[test]
    fn stamping_stays_linear_as_the_stroke_gets_longer() {
        let build = |segments: usize| -> BezPath {
            let points: Vec<Point> = (0..segments)
                .map(|i| Point::new(i as f64 * 10.0, ((i % 7) as f64) * 4.0))
                .collect();
            catmull_rom(&points)
        };

        let time_for = |segments: usize| -> f64 {
            let stroke = build(segments);
            let started = Instant::now();
            let out = stamp_along(
                &stroke,
                &unit_square(),
                PatternFit::Repeat { spacing: 12.0 },
                &BrushBudget::default(),
            );
            assert!(out.stamps > 0);
            started.elapsed().as_secs_f64()
        };

        // Warm up, so the first measurement does not pay for lazy allocation.
        let _ = time_for(200);

        let small = time_for(400).max(1e-6);
        let large = time_for(1600).max(1e-6);

        // Four times the input should cost roughly four times as much, not
        // sixteen. A generous factor keeps this from failing on a busy machine
        // while still catching an accidental quadratic.
        assert!(
            large < small * 12.0,
            "4x the stroke took {:.1}x the time ({small:.6}s -> {large:.6}s); \
             the arc-length table is probably being rebuilt per stamp",
            large / small
        );
    }

    /// A whole drawing's worth of pattern strokes has to stay well inside a
    /// frame budget, or the application stops being usable exactly when a
    /// drawing gets interesting.
    #[test]
    fn hundreds_of_pattern_strokes_stay_within_a_frame_budget() {
        let source = unit_square();
        let budget = BrushBudget::default();

        let started = Instant::now();
        let mut total_stamps = 0usize;
        for stroke_index in 0..300 {
            let base = stroke_index as f64;
            let points: Vec<Point> = (0..40)
                .map(|i| {
                    let t = i as f64;
                    Point::new(t * 8.0, base * 3.0 + (t * 0.4).sin() * 20.0)
                })
                .collect();
            let out = stamp_along(
                &catmull_rom(&points),
                &source,
                PatternFit::Repeat { spacing: 8.0 },
                &budget,
            );
            total_stamps += out.stamps;
        }
        let elapsed = started.elapsed();

        assert!(total_stamps > 3_000, "the test should be doing real work");
        assert!(
            elapsed.as_millis() < 2_000,
            "300 pattern strokes ({total_stamps} stamps) took {elapsed:?}"
        );
    }

    /// A single very long stroke — the pasteboard-spanning drag — must also
    /// stay interactive.
    #[test]
    fn one_enormous_stroke_stays_interactive() {
        let samples: Vec<StrokeSample> = (0..50_000)
            .map(|i| {
                let t = i as f64;
                StrokeSample::new(Point::new(t * 0.5, (t * 0.01).sin() * 200.0), t * 0.001)
            })
            .collect();

        let started = Instant::now();
        let out = fluid_outline(&samples, &BrushProfile::default(), &BrushBudget::default());
        let elapsed = started.elapsed();

        assert!(!out.path.elements().is_empty());
        assert!(
            elapsed.as_millis() < 1_000,
            "a 50 000-sample stroke took {elapsed:?}"
        );
    }

    // -- robustness ---------------------------------------------------------

    #[test]
    fn degenerate_input_does_not_panic() {
        let budget = BrushBudget::default();
        let profile = BrushProfile::default();

        assert!(
            fluid_outline(&[], &profile, &budget)
                .path
                .elements()
                .is_empty()
        );

        // Every sample in the same place.
        let stacked = vec![StrokeSample::new(Point::new(1.0, 1.0), 0.0); 50];
        let _ = fluid_outline(&stacked, &profile, &budget);

        // A zero-length stroke, and an empty source.
        let empty = BezPath::new();
        assert!(
            stamp_along(&empty, &unit_square(), PatternFit::Stretch, &budget)
                .path
                .elements()
                .is_empty()
        );
        let dot = catmull_rom(&[Point::ZERO, Point::ZERO]);
        let _ = stamp_along(
            &dot,
            &unit_square(),
            PatternFit::Repeat { spacing: 1.0 },
            &budget,
        );
        let _ = stamp_along(&dot, &empty, PatternFit::Stretch, &budget);

        // Times that go backwards, which a paused clock can produce.
        let backwards: Vec<StrokeSample> = (0..20)
            .map(|i| StrokeSample::new(Point::new(i as f64 * 3.0, 0.0), -(i as f64)))
            .collect();
        let _ = fluid_outline(&backwards, &profile, &budget);
    }

    #[test]
    fn a_zero_width_brush_paints_nothing_rather_than_a_negative_outline() {
        let profile = BrushProfile {
            width: 0.0,
            ..BrushProfile::default()
        };
        let samples = drag(Point::ZERO, Point::new(50.0, 0.0), 20);
        let out = fluid_outline(&samples, &profile, &BrushBudget::default());
        assert!(out.path.bounding_box().height() < 1e-6);
    }
}
