//! Tweening between keyframes.
//!
//! # How a tween is anchored
//!
//! A tween lives on the keyframe that *starts* it and runs until the next
//! keyframe on the same layer. That is Animate's model — you select a span and
//! apply a tween to it — and it means a tween needs no separate object of its
//! own, which keeps the timeline model simple.
//!
//! # Matching objects across keyframes
//!
//! A classic tween has to know which object at the start corresponds to which
//! at the end. Matching by position in the list breaks the moment anything is
//! reordered. Matching by [`ObjectId`] is exact — and it works naturally
//! because pressing **F6** duplicates a keyframe by cloning the `Arc`, so both
//! keyframes hold objects with the *same* ids. Anything appearing in only one
//! of the two keyframes is left alone rather than being tweened from nothing.
//!
//! # Shape tweens are different
//!
//! Shapes have no ids to match and usually differ in segment count, so
//! geometry is resampled to a common number of points and interpolated
//! point-wise. That is an approximation — Animate uses shape hints to guide it,
//! which is exactly why shape hints exist — but it produces the right result
//! for shapes of similar structure and degrades predictably otherwise.

use buzz_geom::{Affine, BezPath, Point, Shape as _};
use kurbo::PathEl;
use serde::{Deserialize, Serialize};

use crate::object::{Object, ObjectKind};

/// Which kind of tween a span carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TweenKind {
    /// No interpolation; the span holds its keyframe.
    #[default]
    None,
    /// Animate's classic tween: interpolates transforms and colour effects.
    Classic,
    /// Animate's motion tween. Interpolates the same properties as classic;
    /// the difference in Animate is the authoring model, not the maths.
    Motion,
    /// Interpolates geometry between two shapes.
    Shape,
}

impl TweenKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Classic => "Classic Tween",
            Self::Motion => "Motion Tween",
            Self::Shape => "Shape Tween",
        }
    }

    pub fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Colour used for the span in the timeline, matching Animate.
    pub fn timeline_tint(self) -> Option<(u8, u8, u8)> {
        match self {
            Self::None => None,
            // Animate: motion blue, classic purple, shape green.
            Self::Motion => Some((0x6C, 0x8E, 0xBF)),
            Self::Classic => Some((0x9A, 0x7C, 0xC4)),
            Self::Shape => Some((0x7C, 0xB3, 0x7C)),
        }
    }
}

/// Easing applied across a tween span.
///
/// Animate exposes a single -100..100 slider plus custom curves. The slider
/// maps onto [`Easing::Strength`], which covers the overwhelming majority of
/// real use; [`Easing::CubicBezier`] carries an imported custom curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Easing {
    /// Constant rate.
    Linear,
    /// Animate's ease slider. Negative eases in, positive eases out.
    Strength(f64),
    /// A custom curve, as CSS and Animate's editor express it.
    CubicBezier { x1: f64, y1: f64, x2: f64, y2: f64 },
}

impl Default for Easing {
    fn default() -> Self {
        Self::Linear
    }
}

impl Easing {
    /// Map linear progress `t` in `0..=1` to eased progress.
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Strength(amount) => {
                // -100..100 in Animate's units; 0 is linear.
                let a = (amount / 100.0).clamp(-1.0, 1.0);
                if a.abs() < 1e-9 {
                    return t;
                }
                if a > 0.0 {
                    // Ease out: fast then slow.
                    let p = 1.0 + a;
                    1.0 - (1.0 - t).powf(p)
                } else {
                    // Ease in: slow then fast.
                    let p = 1.0 - a;
                    t.powf(p)
                }
            }
            Self::CubicBezier { x1, y1, x2, y2 } => cubic_bezier_ease(t, x1, y1, x2, y2),
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Linear => "Linear".to_string(),
            Self::Strength(a) if a > 0.0 => format!("Ease Out {a:.0}"),
            Self::Strength(a) if a < 0.0 => format!("Ease In {:.0}", -a),
            Self::Strength(_) => "Linear".to_string(),
            Self::CubicBezier { .. } => "Custom".to_string(),
        }
    }
}

/// Solve a CSS-style cubic-Bézier easing for `y` at a given `x`.
///
/// Newton's method with a bisection fallback: Newton converges in a few
/// iterations for well-behaved curves but can wander when the curve is nearly
/// vertical, and a hung frame is worse than a slightly imprecise ease.
fn cubic_bezier_ease(x: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let curve = |t: f64, a: f64, b: f64| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    let slope = |t: f64, a: f64, b: f64| {
        let u = 1.0 - t;
        3.0 * u * u * a + 6.0 * u * t * (b - a) + 3.0 * t * t * (1.0 - b)
    };

    let mut t = x;
    for _ in 0..8 {
        let error = curve(t, x1, x2) - x;
        if error.abs() < 1e-6 {
            return curve(t, y1, y2);
        }
        let d = slope(t, x1, x2);
        if d.abs() < 1e-9 {
            break;
        }
        t -= error / d;
    }

    // Bisection: slower but cannot diverge.
    let (mut low, mut high) = (0.0f64, 1.0f64);
    let mut t = x.clamp(0.0, 1.0);
    for _ in 0..24 {
        let value = curve(t, x1, x2);
        if (value - x).abs() < 1e-6 {
            break;
        }
        if value < x {
            low = t;
        } else {
            high = t;
        }
        t = (low + high) * 0.5;
    }
    curve(t, y1, y2)
}

/// A tween on a keyframe, running until the next keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tween {
    pub kind: TweenKind,
    pub easing: Easing,
    /// Rotate the object during the tween, in whole extra turns.
    pub extra_rotations: i32,
    /// Orient the object along its motion path.
    pub orient_to_path: bool,
}

impl Default for Tween {
    fn default() -> Self {
        Self {
            kind: TweenKind::None,
            easing: Easing::Linear,
            extra_rotations: 0,
            orient_to_path: false,
        }
    }
}

impl Tween {
    pub fn classic() -> Self {
        Self {
            kind: TweenKind::Classic,
            ..Default::default()
        }
    }

    pub fn motion() -> Self {
        Self {
            kind: TweenKind::Motion,
            ..Default::default()
        }
    }

    pub fn shape() -> Self {
        Self {
            kind: TweenKind::Shape,
            ..Default::default()
        }
    }

    pub fn is_active(&self) -> bool {
        self.kind.is_active()
    }
}

/// Interpolate a whole keyframe's worth of objects.
///
/// `progress` is raw linear progress; easing is applied here so callers cannot
/// forget it.
pub fn interpolate_objects(
    from: &[std::sync::Arc<Object>],
    to: &[std::sync::Arc<Object>],
    tween: &Tween,
    progress: f64,
) -> Vec<Object> {
    let t = tween.easing.apply(progress);

    match tween.kind {
        TweenKind::None => from.iter().map(|o| (**o).clone()).collect(),

        TweenKind::Classic | TweenKind::Motion => from
            .iter()
            .map(|start| {
                // Matched by id, which survives reordering and works because
                // F6 clones keyframes with their ids intact.
                match to.iter().find(|e| e.id == start.id) {
                    Some(end) => interpolate_object(start, end, tween, t),
                    // Present only at the start: hold it rather than tween
                    // from nothing.
                    None => (**start).clone(),
                }
            })
            .collect(),

        TweenKind::Shape => from
            .iter()
            .enumerate()
            .map(|(index, start)| {
                // Shapes have no ids to match on, so pair by position.
                match to.get(index) {
                    Some(end) => interpolate_shape_object(start, end, t),
                    None => (**start).clone(),
                }
            })
            .collect(),
    }
}

/// Interpolate one object's transform and colour effect.
fn interpolate_object(start: &Object, end: &Object, tween: &Tween, t: f64) -> Object {
    let mut out = start.clone();
    out.transform = lerp_affine(start.transform, end.transform, t, tween.extra_rotations);

    // Instance colour effects tween too, which is how a symbol fades out.
    if let (ObjectKind::Instance(a), ObjectKind::Instance(b)) = (&start.kind, &end.kind)
        && let ObjectKind::Instance(target) = &mut out.kind
    {
        target.color = a.color.lerp(&b.color, t as f32);
    }
    out
}

/// Interpolate geometry for a shape tween.
fn interpolate_shape_object(start: &Object, end: &Object, t: f64) -> Object {
    let mut out = start.clone();
    out.transform = lerp_affine(start.transform, end.transform, t, 0);

    if let (ObjectKind::Shape(a), ObjectKind::Shape(b)) = (&start.kind, &end.kind)
        && let ObjectKind::Shape(target) = &mut out.kind
    {
        target.path = interpolate_path(&a.path, &b.path, t);
    }
    out
}

/// Interpolate two affines by decomposing them.
///
/// Interpolating the six matrix coefficients directly makes a rotating object
/// shrink through the turn, because the midpoint of two rotation matrices is
/// not a rotation matrix. Decomposing into translation, rotation and scale and
/// interpolating those separately keeps the object the right size all the way
/// round — which is very obvious when it is wrong.
pub fn lerp_affine(a: Affine, b: Affine, t: f64, extra_rotations: i32) -> Affine {
    let (ta, ra, sa) = decompose(a);
    let (tb, rb, sb) = decompose(b);

    let translation = buzz_geom::Vec2::new(ta.x + (tb.x - ta.x) * t, ta.y + (tb.y - ta.y) * t);

    // Shortest way round, then any whole extra turns the user asked for.
    let tau = std::f64::consts::TAU;
    let mut delta = (rb - ra) % tau;
    if delta > tau / 2.0 {
        delta -= tau;
    } else if delta < -tau / 2.0 {
        delta += tau;
    }
    delta += tau * extra_rotations as f64;
    let rotation = ra + delta * t;

    let scale_x = sa.0 + (sb.0 - sa.0) * t;
    let scale_y = sa.1 + (sb.1 - sa.1) * t;

    Affine::translate(translation)
        * Affine::rotate(rotation)
        * Affine::scale_non_uniform(scale_x, scale_y)
}

/// Split an affine into translation, rotation and scale.
fn decompose(a: Affine) -> (Point, f64, (f64, f64)) {
    let c = a.as_coeffs();
    let translation = Point::new(c[4], c[5]);
    let rotation = c[1].atan2(c[0]);
    let scale_x = (c[0] * c[0] + c[1] * c[1]).sqrt();
    // Signed, so a mirrored object stays mirrored through the tween.
    let determinant = c[0] * c[3] - c[1] * c[2];
    let scale_y_magnitude = (c[2] * c[2] + c[3] * c[3]).sqrt();
    let scale_y = if determinant < 0.0 {
        -scale_y_magnitude
    } else {
        scale_y_magnitude
    };
    (translation, rotation, (scale_x, scale_y))
}

/// Number of points a shape tween resamples to.
///
/// High enough to keep curves smooth, low enough that a tween across a long
/// span stays cheap.
const SHAPE_SAMPLES: usize = 96;

/// Interpolate two paths by resampling both to a common point count.
///
/// Approximate by nature: without shape hints there is no canonical
/// correspondence between two outlines. Resampling by arc length gives an even
/// distribution and a predictable result for shapes of similar structure.
pub fn interpolate_path(from: &BezPath, to: &BezPath, t: f64) -> BezPath {
    if t <= 0.0 {
        return from.clone();
    }
    if t >= 1.0 {
        return to.clone();
    }

    let a = resample(from, SHAPE_SAMPLES);
    let mut b = resample(to, SHAPE_SAMPLES);
    if a.is_empty() || b.is_empty() {
        return from.clone();
    }

    let closed = is_closed(from) && is_closed(to);

    // Two resampled outlines only correspond if they are traversed the same
    // way round and start at the same place. Without both, point *i* of one
    // shape pairs with an unrelated point of the other and the interpolation
    // collapses the shape rather than morphing it — measured as a midpoint
    // area of 351 where ~9000 was expected.
    if closed {
        if signed_area(&a) * signed_area(&b) < 0.0 {
            b.reverse();
        }
        let offset = best_alignment(&a, &b);
        b.rotate_left(offset);
    }

    let mut path = BezPath::new();
    for i in 0..SHAPE_SAMPLES {
        let pa = a[i % a.len()];
        let pb = b[i % b.len()];
        let p = Point::new(pa.x + (pb.x - pa.x) * t, pa.y + (pb.y - pa.y) * t);
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    // Closed if both inputs were, which keeps a filled shape filled.
    if is_closed(from) && is_closed(to) {
        path.close_path();
    }
    path
}

/// Twice the signed area of a closed polygon; the sign gives the winding.
fn signed_area(points: &[Point]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        sum += a.x * b.y - b.x * a.y;
    }
    sum
}

/// The cyclic offset of `b` that best lines it up with `a`.
///
/// Brute force over all offsets: with 96 samples that is ~9k distance
/// computations, negligible next to the rest of a frame, and it avoids the
/// wrong-corner pairing that makes a morph look like it folds in on itself.
fn best_alignment(a: &[Point], b: &[Point]) -> usize {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0;
    }
    let mut best = (f64::INFINITY, 0usize);
    for offset in 0..n {
        let mut total = 0.0;
        for i in 0..n {
            let pa = a[i];
            let pb = b[(i + offset) % n];
            total += (pa - pb).hypot2();
            // No point finishing a worse candidate.
            if total >= best.0 {
                break;
            }
        }
        if total < best.0 {
            best = (total, offset);
        }
    }
    best.1
}

fn is_closed(path: &BezPath) -> bool {
    path.elements()
        .iter()
        .any(|e| matches!(e, PathEl::ClosePath))
}

/// Sample `count` points evenly along a path by arc length.
fn resample(path: &BezPath, count: usize) -> Vec<Point> {
    if path.elements().is_empty() || count == 0 {
        return Vec::new();
    }

    // Flatten first: walking segments directly would weight each segment
    // equally regardless of length, bunching points on short segments.
    let bounds = path.bounding_box();
    let tolerance = (bounds.width().hypot(bounds.height()) / 2000.0).clamp(1e-6, 1.0);

    let mut points: Vec<Point> = Vec::new();
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push(p),
        _ => {}
    });
    if points.len() < 2 {
        return points;
    }

    // Cumulative length.
    let mut lengths = Vec::with_capacity(points.len());
    let mut total = 0.0;
    lengths.push(0.0);
    for pair in points.windows(2) {
        total += (pair[1] - pair[0]).hypot();
        lengths.push(total);
    }
    if total <= 0.0 {
        return vec![points[0]; count];
    }

    let mut out = Vec::with_capacity(count);
    let mut cursor = 0usize;
    for i in 0..count {
        let target = total * (i as f64 / count as f64);
        while cursor + 2 < points.len() && lengths[cursor + 1] < target {
            cursor += 1;
        }
        let span = lengths[cursor + 1] - lengths[cursor];
        let local = if span > 0.0 {
            (target - lengths[cursor]) / span
        } else {
            0.0
        };
        let a = points[cursor];
        let b = points[cursor + 1];
        out.push(Point::new(
            a.x + (b.x - a.x) * local,
            a.y + (b.y - a.y) * local,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{ObjectId, ShapeData};
    use kurbo::{Circle, Rect};
    use peniko::Color;
    use std::sync::Arc;

    fn square(x: f64, size: f64) -> BezPath {
        Rect::new(x, 0.0, x + size, size).to_path(1e-9)
    }

    fn object(id: u64, transform: Affine) -> Arc<Object> {
        Arc::new(
            Object::shape(ObjectId(id), ShapeData::filled(square(0.0, 10.0), Color::WHITE))
                .with_transform(transform),
        )
    }

    // -- easing -------------------------------------------------------------

    #[test]
    fn easing_always_spans_zero_to_one() {
        for easing in [
            Easing::Linear,
            Easing::Strength(100.0),
            Easing::Strength(-100.0),
            Easing::CubicBezier { x1: 0.42, y1: 0.0, x2: 0.58, y2: 1.0 },
        ] {
            assert!((easing.apply(0.0) - 0.0).abs() < 1e-6, "{easing:?} at 0");
            assert!((easing.apply(1.0) - 1.0).abs() < 1e-6, "{easing:?} at 1");
        }
    }

    #[test]
    fn easing_is_monotonic_and_bounded() {
        for easing in [
            Easing::Linear,
            Easing::Strength(80.0),
            Easing::Strength(-80.0),
            Easing::CubicBezier { x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0 },
        ] {
            let mut previous = -1.0;
            for i in 0..=50 {
                let v = easing.apply(i as f64 / 50.0);
                assert!((0.0..=1.0).contains(&v), "{easing:?} produced {v}");
                assert!(v >= previous - 1e-9, "{easing:?} went backwards at {i}");
                previous = v;
            }
        }
    }

    #[test]
    fn ease_out_starts_fast_and_ease_in_starts_slow() {
        assert!(
            Easing::Strength(100.0).apply(0.25) > 0.25,
            "ease out should be ahead early"
        );
        assert!(
            Easing::Strength(-100.0).apply(0.25) < 0.25,
            "ease in should be behind early"
        );
        assert!((Easing::Strength(0.0).apply(0.3) - 0.3).abs() < 1e-9);
    }

    /// A near-vertical curve must not hang the solver.
    #[test]
    fn a_pathological_easing_curve_still_terminates() {
        let nasty = Easing::CubicBezier { x1: 1.0, y1: 0.0, x2: 0.0, y2: 1.0 };
        let started = std::time::Instant::now();
        for i in 0..=100 {
            let v = nasty.apply(i as f64 / 100.0);
            assert!(v.is_finite());
        }
        assert!(started.elapsed().as_millis() < 100);
    }

    // -- transform interpolation ---------------------------------------------

    #[test]
    fn a_transform_tween_moves_halfway_at_halfway() {
        let a = Affine::translate((0.0, 0.0));
        let b = Affine::translate((100.0, 50.0));
        let mid = lerp_affine(a, b, 0.5, 0);
        let p = mid * Point::ORIGIN;
        assert!((p.x - 50.0).abs() < 1e-9 && (p.y - 25.0).abs() < 1e-9, "{p:?}");
    }

    /// Interpolating matrix coefficients directly shrinks a rotating object.
    /// Decomposition must not.
    #[test]
    fn a_rotating_object_keeps_its_size_through_the_turn() {
        let a = Affine::rotate(0.0);
        let b = Affine::rotate(std::f64::consts::PI);

        for i in 0..=10 {
            let t = i as f64 / 10.0;
            let m = lerp_affine(a, b, t, 0);
            let c = m.as_coeffs();
            let scale = (c[0] * c[0] + c[1] * c[1]).sqrt();
            assert!(
                (scale - 1.0).abs() < 1e-6,
                "scale drifted to {scale} at t={t}; coefficients were interpolated directly"
            );
        }
    }

    #[test]
    fn extra_rotations_add_whole_turns() {
        let a = Affine::rotate(0.0);
        let b = Affine::rotate(0.0);
        // One extra turn means halfway is half a turn.
        let mid = lerp_affine(a, b, 0.5, 1);
        let p = mid * Point::new(1.0, 0.0);
        assert!((p.x + 1.0).abs() < 1e-6, "expected a half turn, got {p:?}");
    }

    #[test]
    fn a_mirrored_object_stays_mirrored() {
        let a = Affine::scale_non_uniform(1.0, -1.0);
        let b = Affine::scale_non_uniform(2.0, -2.0);
        let mid = lerp_affine(a, b, 0.5, 0);
        let c = mid.as_coeffs();
        let determinant = c[0] * c[3] - c[1] * c[2];
        assert!(determinant < 0.0, "the mirror was lost: {determinant}");
    }

    // -- object interpolation -------------------------------------------------

    #[test]
    fn objects_are_matched_by_id_not_position() {
        let from = vec![
            object(1, Affine::translate((0.0, 0.0))),
            object(2, Affine::translate((0.0, 0.0))),
        ];
        // Reversed order, same ids.
        let to = vec![
            object(2, Affine::translate((200.0, 0.0))),
            object(1, Affine::translate((100.0, 0.0))),
        ];

        let out = interpolate_objects(&from, &to, &Tween::classic(), 0.5);
        let x = |id: u64| {
            out.iter()
                .find(|o| o.id == ObjectId(id))
                .map(|o| (o.transform * Point::ORIGIN).x)
                .unwrap()
        };
        assert!((x(1) - 50.0).abs() < 1e-9, "object 1 went to {}", x(1));
        assert!((x(2) - 100.0).abs() < 1e-9, "object 2 went to {}", x(2));
    }

    #[test]
    fn an_object_missing_from_the_end_keyframe_is_held() {
        let from = vec![object(1, Affine::IDENTITY), object(2, Affine::IDENTITY)];
        let to = vec![object(1, Affine::translate((100.0, 0.0)))];

        let out = interpolate_objects(&from, &to, &Tween::classic(), 0.5);
        assert_eq!(out.len(), 2, "the unmatched object should survive");
        let held = out.iter().find(|o| o.id == ObjectId(2)).unwrap();
        assert_eq!(held.transform.as_coeffs(), Affine::IDENTITY.as_coeffs());
    }

    #[test]
    fn no_tween_holds_the_starting_keyframe() {
        let from = vec![object(1, Affine::IDENTITY)];
        let to = vec![object(1, Affine::translate((100.0, 0.0)))];
        let out = interpolate_objects(&from, &to, &Tween::default(), 0.5);
        assert_eq!(out[0].transform.as_coeffs(), Affine::IDENTITY.as_coeffs());
    }

    #[test]
    fn easing_is_applied_by_the_interpolator() {
        let from = vec![object(1, Affine::translate((0.0, 0.0)))];
        let to = vec![object(1, Affine::translate((100.0, 0.0)))];

        let tween = Tween {
            easing: Easing::Strength(100.0),
            ..Tween::classic()
        };
        let out = interpolate_objects(&from, &to, &tween, 0.25);
        let x = (out[0].transform * Point::ORIGIN).x;
        assert!(x > 25.0, "ease out should be ahead of linear, got {x}");
    }

    // -- shape tweening --------------------------------------------------------

    #[test]
    fn a_shape_tween_produces_geometry_between_the_two() {
        let a = square(0.0, 100.0);
        let b = Circle::new(Point::new(50.0, 50.0), 50.0).to_path(0.05);

        let mid = interpolate_path(&a, &b, 0.5);
        assert!(!mid.elements().is_empty());

        let area = mid.area().abs();
        let (area_a, area_b) = (a.area().abs(), b.area().abs());
        let (low, high) = (area_a.min(area_b), area_a.max(area_b));
        assert!(
            area > low * 0.7 && area < high * 1.3,
            "midpoint area {area} should sit between {low} and {high}"
        );
    }

    /// Regression test for a real defect: without aligning winding direction
    /// and start point, point *i* of one outline paired with an unrelated
    /// point of the other and the shape collapsed. A square-to-circle morph
    /// came out with an area of 351 instead of about 9000.
    #[test]
    fn shape_tweening_aligns_winding_and_start_point() {
        let square = square(0.0, 100.0);
        let circle = Circle::new(Point::new(50.0, 50.0), 50.0).to_path(0.05);

        // Sample across the whole tween, not just the midpoint: a bad
        // alignment shows up as a collapse somewhere in the middle.
        for i in 1..10 {
            let t = i as f64 / 10.0;
            let area = interpolate_path(&square, &circle, t).area().abs();
            assert!(
                area > 6_000.0,
                "the shape collapsed at t={t} (area {area}); \
                 the two outlines are not aligned"
            );
        }
    }

    /// A reversed outline must be detected and flipped, not morphed through
    /// itself.
    #[test]
    fn opposite_winding_directions_are_reconciled() {
        let forward = square(0.0, 100.0);
        let mut reversed_points: Vec<Point> = Vec::new();
        kurbo::flatten(forward.iter(), 0.01, |el| match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => reversed_points.push(p),
            _ => {}
        });
        reversed_points.reverse();

        let mut reversed = BezPath::new();
        reversed.move_to(reversed_points[0]);
        for p in &reversed_points[1..] {
            reversed.line_to(*p);
        }
        reversed.close_path();

        let mid = interpolate_path(&forward, &reversed, 0.5).area().abs();
        assert!(
            mid > 6_000.0,
            "a shape morphing to its own reverse should stay itself, got {mid}"
        );
    }

    #[test]
    fn the_ends_of_a_shape_tween_are_the_originals() {
        let a = square(0.0, 50.0);
        let b = square(100.0, 50.0);
        assert_eq!(interpolate_path(&a, &b, 0.0).elements(), a.elements());
        assert_eq!(interpolate_path(&a, &b, 1.0).elements(), b.elements());
    }

    #[test]
    fn a_shape_tween_between_closed_shapes_stays_closed() {
        let a = square(0.0, 50.0);
        let b = Circle::new(Point::new(25.0, 25.0), 25.0).to_path(0.05);
        let mid = interpolate_path(&a, &b, 0.5);
        assert!(
            mid.elements().iter().any(|e| matches!(e, PathEl::ClosePath)),
            "a filled shape must not spring open mid-tween"
        );
    }

    #[test]
    fn shapes_with_very_different_segment_counts_still_tween() {
        let simple = square(0.0, 50.0);
        let complex = Circle::new(Point::new(25.0, 25.0), 25.0).to_path(1e-6);
        assert!(complex.elements().len() > simple.elements().len());

        let mid = interpolate_path(&simple, &complex, 0.5);
        assert!(mid.area().abs() > 0.0, "resampling should bridge the gap");
    }

    #[test]
    fn tweening_an_empty_path_does_not_panic() {
        let empty = BezPath::new();
        let square = square(0.0, 10.0);
        assert!(interpolate_path(&empty, &square, 0.5).elements().is_empty()
            || !interpolate_path(&empty, &square, 0.5).elements().is_empty());
        let _ = interpolate_path(&square, &empty, 0.5);
        let _ = interpolate_path(&empty, &empty, 0.5);
    }

    #[test]
    fn tween_kinds_carry_animates_timeline_colours() {
        assert!(TweenKind::None.timeline_tint().is_none());
        assert!(TweenKind::Motion.timeline_tint().is_some());
        assert!(TweenKind::Classic.timeline_tint().is_some());
        assert!(TweenKind::Shape.timeline_tint().is_some());
        assert_ne!(
            TweenKind::Motion.timeline_tint(),
            TweenKind::Shape.timeline_tint(),
            "the kinds must be distinguishable at a glance"
        );
    }

    #[test]
    fn a_shape_tween_of_many_shapes_stays_affordable() {
        let a = Circle::new(Point::new(0.0, 0.0), 100.0).to_path(1e-4);
        let b = Circle::new(Point::new(50.0, 50.0), 30.0).to_path(1e-4);

        let started = std::time::Instant::now();
        for i in 0..200 {
            let _ = interpolate_path(&a, &b, i as f64 / 200.0);
        }
        assert!(
            started.elapsed().as_millis() < 1_000,
            "200 shape-tween frames took {:?}",
            started.elapsed()
        );
    }
}
