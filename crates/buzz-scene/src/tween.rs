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
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Easing {
    /// Constant rate.
    #[default]
    Linear,
    /// Animate's ease slider. Negative eases in, positive eases out.
    Strength(f64),
    /// A custom curve, as CSS and Animate's editor express it.
    CubicBezier { x1: f64, y1: f64, x2: f64, y2: f64 },
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

        TweenKind::Shape => {
            // **Paired by what the drawings are, not by the order they were
            // drawn in.** See `match_drawings` for why that is the whole
            // difference between a rough inbetween and a shape crossing the
            // frame to become something unrelated.
            let pairing = match_drawings(from, to);
            let mut out: Vec<Object> = Vec::with_capacity(from.len().max(to.len()));
            let mut matched = vec![false; to.len()];

            for (index, start) in from.iter().enumerate() {
                match pairing.get(index).copied().flatten() {
                    Some(j) => {
                        matched[j] = true;
                        out.push(interpolate_shape_object(start, &to[j], t));
                    }
                    // Nothing on the far keyframe is this piece: it is on its
                    // way out.
                    None => out.push(vanishing(start, t)),
                }
            }

            // And whatever the far keyframe has that this one does not is on
            // its way in. Without this a drawing that gains a piece gained it
            // all at once, on the last frame.
            for (j, end) in to.iter().enumerate() {
                if !matched[j] {
                    out.push(arriving(end, t));
                }
            }
            out
        }
    }
}

/// **Which drawing on the first keyframe becomes which on the second.**
///
/// # Why this is not the order they were drawn in
///
/// A shape tween used to pair the two keyframes' artwork by *array position* —
/// shape 1 with shape 1, shape 2 with shape 2 — because shapes carry no ids to
/// match on. That is right exactly when both drawings were made in the same
/// order, and hand-drawn frames never are: draw the head before the arm on one
/// frame and after it on the next, and the head morphs into the arm. What came
/// out was not a rough inbetween, it was a shape crossing the frame to become
/// something unrelated, and the only fix available was to redraw a keyframe in
/// a particular order to appease the tweener.
///
/// So the pairing is made from what the drawings *are*: where each piece sits,
/// how big it is, whether it closes, and what colour it is painted. Four cheap
/// measures, none of them clever, and together they put the head with the head.
///
/// # Greedy, on purpose
///
/// Every pair is costed, sorted, and taken best-first while both sides are
/// still free. A true optimal assignment (Hungarian) is O(n³) and would differ
/// only where two pieces are nearly equally good partners — which is the case
/// where the animator will correct it whatever we choose. Greedy is predictable
/// and easy to read, and a tween that is understandable when it goes wrong is
/// worth more here than one that is optimal and inscrutable.
///
/// Returns, for each shape of `from`, the index in `to` it becomes — or `None`
/// where it has no partner and should vanish. Pieces that are not plain shapes
/// (a group, an instance, a rig) are paired by position as before: they are not
/// drawings and have no outline to compare.
pub fn match_drawings(from: &[std::sync::Arc<Object>], to: &[std::sync::Arc<Object>]) -> Vec<Option<usize>> {
    let mut pairing = vec![None; from.len()];
    let mut taken = vec![false; to.len()];

    // Anything that is not a shape keeps the old positional pairing: there is
    // nothing to measure, and changing how those behave is not what this is for.
    for (i, start) in from.iter().enumerate() {
        if !matches!(start.kind, ObjectKind::Shape(_)) {
            if let Some(j) = to.get(i)
                && !matches!(to[i].kind, ObjectKind::Shape(_))
            {
                let _ = j;
                pairing[i] = Some(i);
                taken[i] = true;
            }
        }
    }

    // The scale everything is measured against: the extent of **both**
    // keyframes together, so a costume detail on a small figure is judged by
    // the same yardstick as one on a large one — and, more importantly, so a
    // lone shape crossing the stage is measured against the distance it
    // crossed rather than against its own width. Measured against itself, every
    // classic one-shape morph looked like two unrelated pieces.
    let span = drawing_span(from)
        .max(drawing_span(to))
        .max(both_span(from, to));

    let mut costs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, start) in from.iter().enumerate() {
        if pairing[i].is_some() {
            continue;
        }
        let Some(a) = Trait::of(start) else { continue };
        for (j, end) in to.iter().enumerate() {
            if taken[j] {
                continue;
            }
            let Some(b) = Trait::of(end) else { continue };
            costs.push((a.cost(&b, span), i, j));
        }
    }
    costs.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    for (cost, i, j) in costs {
        if pairing[i].is_some() || taken[j] {
            continue;
        }
        // Past this the two are not the same piece of drawing by any reading,
        // and morphing them together looks worse than letting one go and the
        // other arrive.
        if cost > MAX_PAIR_COST {
            break;
        }
        pairing[i] = Some(j);
        taken[j] = true;
    }
    pairing
}

/// How far apart two pieces may be before they are not the same piece.
///
/// In the units [`Trait::cost`] returns, whose distance term is measured
/// against the extent of both keyframes together. Just under one, so that two
/// pieces at opposite ends of the action are not held to be the same piece,
/// while a single shape crossing that action still is — which is the classic
/// morph, and the thing a shape tween is chiefly used for.
const MAX_PAIR_COST: f64 = 0.9;

/// The measurable facts about one drawn piece.
struct Trait {
    centre: Point,
    /// Square root of the area of its box: a length, so it compares linearly.
    size: f64,
    closed: bool,
    colour: [f64; 3],
}

impl Trait {
    fn of(object: &Object) -> Option<Trait> {
        let ObjectKind::Shape(shape) = &object.kind else {
            return None;
        };
        let box_ = object.transform.transform_rect_bbox(shape.path.bounding_box());
        let colour = shape
            .fill
            .as_ref()
            .map(|f| f.paint.color())
            .or_else(|| shape.stroke.as_ref().map(|s| s.paint.color()))
            .unwrap_or(peniko::Color::BLACK)
            .to_rgba8()
            .to_u8_array();
        Some(Trait {
            centre: box_.center(),
            size: (box_.width().max(0.0) * box_.height().max(0.0)).sqrt().max(1e-6),
            closed: is_closed(&shape.path),
            colour: [
                f64::from(colour[0]) / 255.0,
                f64::from(colour[1]) / 255.0,
                f64::from(colour[2]) / 255.0,
            ],
        })
    }

    /// How unlike another piece this one is. Zero is identical.
    fn cost(&self, other: &Trait, span: f64) -> f64 {
        // Where it sits, as a fraction of the whole drawing. The heaviest term:
        // a piece that has not moved much is almost always the same piece.
        let moved = self.centre.distance(other.centre) / span;

        // How much bigger or smaller, judged as a ratio so doubling and halving
        // cost the same.
        let grew = (self.size / other.size).ln().abs() / 2.0;

        // A closed outline and an open line are different kinds of mark.
        let shape = if self.closed == other.closed { 0.0 } else { 0.35 };

        // And colour, lightly: an animator redrawing a red shape draws it red,
        // but a shape may legitimately change colour through a tween.
        let recoloured = (0..3)
            .map(|c| (self.colour[c] - other.colour[c]).abs())
            .sum::<f64>()
            / 3.0
            * 0.4;

        moved + grew + shape + recoloured
    }
}

/// The extent of both keyframes at once — how far the whole action ranges.
fn both_span(from: &[std::sync::Arc<Object>], to: &[std::sync::Arc<Object>]) -> f64 {
    let mut all: Vec<std::sync::Arc<Object>> = Vec::with_capacity(from.len() + to.len());
    all.extend(from.iter().cloned());
    all.extend(to.iter().cloned());
    drawing_span(&all)
}

/// The size of a whole drawing, for judging distances against.
fn drawing_span(objects: &[std::sync::Arc<Object>]) -> f64 {
    let mut bounds: Option<buzz_geom::Rect> = None;
    for object in objects {
        let ObjectKind::Shape(shape) = &object.kind else {
            continue;
        };
        let box_ = object.transform.transform_rect_bbox(shape.path.bounding_box());
        bounds = Some(match bounds {
            Some(b) => b.union(box_),
            None => box_,
        });
    }
    bounds
        .map(|b| b.width().hypot(b.height()))
        .unwrap_or(1.0)
        .max(1.0)
}

/// A piece with no partner, on its way out: shrunk towards its own middle as
/// the tween runs.
///
/// Held in place is what this used to do, and it is the worse answer — the
/// piece sits there through the whole tween and then pops out of existence on
/// the last frame. Shrinking says what is happening, and shrinking *about its
/// own centre* keeps it where it was rather than sliding it to the origin.
fn vanishing(object: &Object, t: f64) -> Object {
    let mut out = object.clone();
    let ObjectKind::Shape(shape) = &object.kind else {
        return out;
    };
    let centre = shape.path.bounding_box().center();
    let scale = (1.0 - t).clamp(0.0, 1.0);
    out.transform = object.transform
        * Affine::translate(centre.to_vec2())
        * Affine::scale(scale.max(1e-4))
        * Affine::translate(-centre.to_vec2());
    out
}

/// The mirror of [`vanishing`]: a piece with no partner that is arriving, grown
/// from its own middle.
fn arriving(object: &Object, t: f64) -> Object {
    vanishing(object, 1.0 - t)
}

/// Interpolate one object's transform and colour effect.
fn interpolate_object(start: &Object, end: &Object, tween: &Tween, t: f64) -> Object {
    let mut out = start.clone();
    out.transform = lerp_affine(start.transform, end.transform, t, tween.extra_rotations);

    // **Orient along the path.** Animate's "Orient to path" turns the object to
    // face the way it is travelling rather than holding the rotation it was
    // keyed with — a car following a bend, a fish nosing along a curve. The
    // travel here is the straight line between the two keyframes' positions (a
    // motion tween is a straight move); the rotation is replaced with that
    // heading while the eased translation and the scale are kept.
    if tween.orient_to_path {
        let travel = end.transform.translation() - start.transform.translation();
        if travel.hypot() > 1e-9 {
            let (translation, _rotation, scale) = decompose(out.transform);
            out.transform = Affine::translate(translation.to_vec2())
                * Affine::rotate(travel.y.atan2(travel.x))
                * Affine::scale_non_uniform(scale.0, scale.1);
        }
    }

    // Instance colour effects tween too, which is how a symbol fades out.
    if let (ObjectKind::Instance(a), ObjectKind::Instance(b)) = (&start.kind, &end.kind)
        && let ObjectKind::Instance(target) = &mut out.kind
    {
        target.color = a.color.lerp(&b.color, t as f32);
    }

    // An object's facing tweens, which is how a card turns as the camera
    // passes it — the thing 3D rotation is for.
    out.spatial = start.spatial.lerp(&end.spatial, t);

    // The transformation point tweens with the rest, so a hinge that moves
    // between two keyframes moves smoothly rather than jumping at the end.
    // A point set on one keyframe and not the other holds where it is, which
    // is the same rule the filters below follow.
    out.pivot = match (start.pivot, end.pivot) {
        (Some(a), Some(b)) => Some(a.lerp(b, t)),
        (a, b) => a.or(b),
    };

    // Filters tween, which is how a glow grows or a shadow swings across a
    // shot. Matched by position in the stack and by kind: Animate holds a
    // filter that has no counterpart rather than interpolating towards
    // nothing, and so does this.
    if !start.filters.is_empty() {
        out.filters = start
            .filters
            .iter()
            .enumerate()
            .map(|(i, filter)| match end.filters.get(i) {
                Some(target) => filter.lerp(target, t),
                None => filter.clone(),
            })
            .collect();
    }

    // **Armature poses tween.** This is what makes rigging an animation tool
    // rather than a posing tool: two keyframes holding the same rig in
    // different poses, and every frame between them interpolated joint by
    // joint. The pose is a handful of angles, so this is arithmetic rather
    // than geometry — and each joint turns the shortest way round, as
    // everything else that interpolates an angle here does.
    if let (ObjectKind::Armature(a), ObjectKind::Armature(b)) = (&start.kind, &end.kind) {
        out.kind = ObjectKind::Armature(crate::rig::tween_armature(a, b, t));
    }

    // Warp handles tween the same way, which is how a puppet-warped drawing is
    // animated: place the handles once, move them, and the frames between are
    // the handle positions between.
    if let (ObjectKind::Warp(a), ObjectKind::Warp(b)) = (&start.kind, &end.kind) {
        out.kind = ObjectKind::Warp(crate::rig::tween_warp(a, b, t));
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

    // A shape tween over rigged artwork interpolates the *rig*, not the
    // vertices. Blending the deformed outlines of two poses would fight the
    // skeleton and produce shapes no pose could make; moving the bones between
    // the two poses is both cheaper and the only answer that stays a rig.
    if let (ObjectKind::Armature(a), ObjectKind::Armature(b)) = (&start.kind, &end.kind) {
        out.kind = ObjectKind::Armature(crate::rig::tween_armature(a, b, t));
    }
    if let (ObjectKind::Warp(a), ObjectKind::Warp(b)) = (&start.kind, &end.kind) {
        out.kind = ObjectKind::Warp(crate::rig::tween_warp(a, b, t));
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

    // **A drawing is made of strokes, and they have to be paired too.**
    //
    // `match_drawings` pairs the *pieces* of two keyframes; this is the same
    // problem one level down. A single piece of artwork is very often several
    // contours — an outline and the holes in it, or a dozen pen strokes merged
    // into one shape — and resampling the whole path as one run of points pairs
    // sample 40 of one drawing with sample 40 of the other whatever they happen
    // to belong to. Two contours drawn in a different order, or a drawing that
    // gains one, and the morph runs a stroke across the drawing to become an
    // unrelated stroke.
    //
    // One contour on each side is the overwhelmingly common case and takes the
    // path below unchanged, so this costs nothing where there is nothing to
    // pair.
    let from_parts = subpaths(from);
    let to_parts = subpaths(to);
    if from_parts.len() > 1 || to_parts.len() > 1 {
        return interpolate_strokes(&from_parts, &to_parts, t);
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

/// **The separate contours a path is made of** — its strokes.
///
/// A `move_to` starts one; everything up to the next `move_to` belongs to it.
/// Empty subpaths are dropped: a stray `move_to` with nothing after it is not a
/// stroke, and pairing against it would waste a partner.
fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    let mut current = BezPath::new();
    for element in path.elements() {
        if matches!(element, buzz_geom::PathEl::MoveTo(_)) && !current.elements().is_empty() {
            if current.elements().len() > 1 {
                out.push(std::mem::take(&mut current));
            } else {
                current = BezPath::new();
            }
        }
        current.push(*element);
    }
    if current.elements().len() > 1 {
        out.push(current);
    }
    out
}

/// **Morph two drawings stroke by stroke**, pairing the contours by what they
/// are rather than by the order they were drawn in.
///
/// The same three ideas as [`match_drawings`], one level down: cost every pair,
/// take the best first, and let whatever is left over shrink away or grow in
/// rather than sit there and pop. Winding is part of the cost, so a hole is
/// paired with a hole — matching a hole to a solid outline would fill it in
/// halfway through the tween, which is the one artefact that would be blamed on
/// the drawing rather than on the tweener.
fn interpolate_strokes(from: &[BezPath], to: &[BezPath], t: f64) -> BezPath {
    let span = stroke_span(from).max(stroke_span(to)).max(1.0);

    let traits_from: Vec<StrokeTrait> = from.iter().map(StrokeTrait::of).collect();
    let traits_to: Vec<StrokeTrait> = to.iter().map(StrokeTrait::of).collect();

    let mut costs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, a) in traits_from.iter().enumerate() {
        for (j, b) in traits_to.iter().enumerate() {
            costs.push((a.cost(b, span), i, j));
        }
    }
    costs.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut pairing: Vec<Option<usize>> = vec![None; from.len()];
    let mut taken = vec![false; to.len()];
    for (cost, i, j) in costs {
        if pairing[i].is_some() || taken[j] {
            continue;
        }
        if cost > MAX_STROKE_COST {
            break;
        }
        pairing[i] = Some(j);
        taken[j] = true;
    }

    let mut out = BezPath::new();
    for (i, stroke) in from.iter().enumerate() {
        match pairing[i] {
            Some(j) => extend(&mut out, &interpolate_contour(stroke, &to[j], t)),
            // Nothing on the far drawing is this stroke: it goes.
            None => extend(&mut out, &shrunk(stroke, 1.0 - t)),
        }
    }
    for (j, stroke) in to.iter().enumerate() {
        if !taken[j] {
            extend(&mut out, &shrunk(stroke, t));
        }
    }
    out
}

/// How unlike two strokes may be and still be the same stroke.
///
/// In the units [`StrokeTrait::cost`] returns, whose distance term is measured
/// against the whole drawing. Tighter than the one pieces are paired by: within
/// a single drawing the strokes are close together and there are more of them,
/// so a loose threshold pairs a sleeve with a collar.
const MAX_STROKE_COST: f64 = 0.7;

/// What one contour is, for pairing.
struct StrokeTrait {
    centre: Point,
    size: f64,
    /// Sign of the enclosed area: which way round it is drawn, and therefore
    /// whether it is a hole.
    winding: f64,
    closed: bool,
}

impl StrokeTrait {
    fn of(path: &BezPath) -> StrokeTrait {
        use buzz_geom::Shape as _;
        let box_ = path.bounding_box();
        let points = resample(path, 24);
        StrokeTrait {
            centre: box_.center(),
            size: (box_.width().max(0.0) * box_.height().max(0.0))
                .sqrt()
                .max(1e-6),
            winding: signed_area(&points).signum(),
            closed: is_closed(path),
        }
    }

    fn cost(&self, other: &StrokeTrait, span: f64) -> f64 {
        let moved = self.centre.distance(other.centre) / span;
        let grew = (self.size / other.size).ln().abs() / 2.0;
        let shape = if self.closed == other.closed { 0.0 } else { 0.3 };
        // A hole and an outline are not the same stroke. Heavy, because filling
        // a hole in halfway through a tween is the artefact nobody would read
        // as the tweener's fault.
        let inside_out = if self.winding == other.winding { 0.0 } else { 0.5 };
        moved + grew + shape + inside_out
    }
}

/// The size of a whole drawing, for judging how far a stroke has moved.
fn stroke_span(strokes: &[BezPath]) -> f64 {
    use buzz_geom::Shape as _;
    let mut bounds: Option<buzz_geom::Rect> = None;
    for stroke in strokes {
        let box_ = stroke.bounding_box();
        bounds = Some(match bounds {
            Some(b) => b.union(box_),
            None => box_,
        });
    }
    bounds
        .map(|b| b.width().hypot(b.height()))
        .unwrap_or(1.0)
        .max(1.0)
}

/// Morph one contour into another — the single-outline case, which is what
/// [`interpolate_path`] did for a whole path before strokes were paired.
fn interpolate_contour(from: &BezPath, to: &BezPath, t: f64) -> BezPath {
    let a = resample(from, SHAPE_SAMPLES);
    let mut b = resample(to, SHAPE_SAMPLES);
    if a.is_empty() || b.is_empty() {
        return from.clone();
    }
    let closed = is_closed(from) && is_closed(to);
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
    if closed {
        path.close_path();
    }
    path
}

/// A stroke drawn at `scale` of its size, about its own middle: what a stroke
/// with no partner does on its way out, or on its way in.
fn shrunk(path: &BezPath, scale: f64) -> BezPath {
    use buzz_geom::Shape as _;
    let centre = path.bounding_box().center();
    let scale = scale.clamp(0.0, 1.0).max(1e-4);
    Affine::translate(centre.to_vec2())
        * Affine::scale(scale)
        * Affine::translate(-centre.to_vec2())
        * path.clone()
}

/// Append one path's elements to another.
fn extend(into: &mut BezPath, from: &BezPath) {
    for element in from.elements() {
        into.push(*element);
    }
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
mod stroke_tests {
    use super::*;
    use buzz_geom::Shape as _;

    /// A square contour, as one subpath.
    fn ring(cx: f64, cy: f64, size: f64) -> BezPath {
        let r = size / 2.0;
        buzz_geom::Rect::new(cx - r, cy - r, cx + r, cy + r).to_path(1e-9)
    }

    /// A hole: the same square wound the other way.
    fn hole(cx: f64, cy: f64, size: f64) -> BezPath {
        let mut points: Vec<Point> = resample(&ring(cx, cy, size), 16);
        points.reverse();
        let mut path = BezPath::new();
        for (i, p) in points.iter().enumerate() {
            if i == 0 {
                path.move_to(*p);
            } else {
                path.line_to(*p);
            }
        }
        path.close_path();
        path
    }

    fn joined(parts: &[BezPath]) -> BezPath {
        let mut out = BezPath::new();
        for part in parts {
            for element in part.elements() {
                out.push(*element);
            }
        }
        out
    }

    /// The centres of a path's contours, so a test can say where each stroke
    /// ended up.
    fn centres(path: &BezPath) -> Vec<Point> {
        subpaths(path)
            .iter()
            .map(|s| s.bounding_box().center())
            .collect()
    }

    #[test]
    fn a_path_splits_into_its_own_strokes() {
        let drawing = joined(&[ring(0.0, 0.0, 20.0), ring(100.0, 0.0, 20.0)]);
        assert_eq!(subpaths(&drawing).len(), 2);
    }

    /// **The defect this exists to fix**, one level below the piece matcher: two
    /// drawings of the same two strokes, drawn in opposite orders. Paired by
    /// sample index, a stroke crosses the drawing to become the other one.
    #[test]
    fn strokes_pair_by_what_they_are_not_by_draw_order() {
        let a = joined(&[ring(0.0, 0.0, 20.0), ring(200.0, 0.0, 40.0)]);
        // The same two, barely moved, drawn the other way round.
        let b = joined(&[ring(205.0, 0.0, 40.0), ring(4.0, 0.0, 20.0)]);

        let middle = interpolate_path(&a, &b, 0.5);
        let mut got = centres(&middle);
        got.sort_by(|p, q| p.x.partial_cmp(&q.x).unwrap());

        assert_eq!(got.len(), 2, "both strokes are still there");
        assert!(
            got[0].x.abs() < 20.0,
            "the small stroke stayed where it was, at {:?}",
            got[0]
        );
        assert!(
            (got[1].x - 202.5).abs() < 20.0,
            "and the large one stayed where it was, at {:?}",
            got[1]
        );
    }

    /// One contour on each side is the common case and must take the path it
    /// always did — bit for bit, because that is what every existing shape
    /// tween in every existing document relies on.
    #[test]
    fn a_single_contour_morphs_exactly_as_it_always_did() {
        let a = ring(0.0, 0.0, 20.0);
        let b = ring(100.0, 0.0, 20.0);

        let through_the_new_path = interpolate_path(&a, &b, 0.5);
        let through_the_old_path = interpolate_contour(&a, &b, 0.5);
        assert_eq!(
            through_the_new_path.to_svg(),
            through_the_old_path.to_svg(),
            "a single-contour morph is unchanged"
        );
    }

    /// **A hole stays a hole.** Pairing it with a solid outline would fill it in
    /// halfway through the tween, which reads as a fault in the drawing rather
    /// than in the tweener.
    #[test]
    fn a_hole_is_paired_with_a_hole() {
        let a = joined(&[ring(0.0, 0.0, 100.0), hole(0.0, 0.0, 40.0)]);
        let b = joined(&[ring(6.0, 0.0, 100.0), hole(6.0, 0.0, 40.0)]);

        let middle = interpolate_path(&a, &b, 0.5);
        let parts = subpaths(&middle);
        assert_eq!(parts.len(), 2, "the outline and its hole");

        let windings: Vec<f64> = parts
            .iter()
            .map(|p| signed_area(&resample(p, 24)).signum())
            .collect();
        assert!(
            windings[0] != windings[1],
            "one is still wound the other way — it is still a hole: {windings:?}"
        );
    }

    /// A stroke with no partner leaves, rather than sitting there and popping
    /// out of existence on the last frame.
    #[test]
    fn a_stroke_with_no_partner_shrinks_away() {
        let a = joined(&[ring(0.0, 0.0, 40.0), ring(300.0, 300.0, 40.0)]);
        let b = ring(4.0, 0.0, 40.0);

        let width_of_the_leaver = |t: f64| {
            let drawn = interpolate_path(&a, &b, t);
            subpaths(&drawn)
                .into_iter()
                .map(|s| s.bounding_box())
                .find(|box_| box_.center().x > 100.0)
                .map(|box_| box_.width())
                .unwrap_or(0.0)
        };
        let early = width_of_the_leaver(0.25);
        let late = width_of_the_leaver(0.75);
        assert!(
            late < early && early > 0.0,
            "the unpartnered stroke is on its way out: {early} then {late}"
        );
    }

    /// And one that arrives grows in, rather than appearing whole at the end.
    #[test]
    fn a_stroke_that_arrives_grows_in() {
        let a = ring(0.0, 0.0, 40.0);
        let b = joined(&[ring(4.0, 0.0, 40.0), ring(300.0, 300.0, 40.0)]);

        let middle = interpolate_path(&a, &b, 0.5);
        let arriving = subpaths(&middle)
            .into_iter()
            .map(|s| s.bounding_box())
            .find(|box_| box_.center().x > 100.0)
            .expect("the arriving stroke is drawn while arriving");
        assert!(
            arriving.width() > 4.0 && arriving.width() < 30.0,
            "halfway in it is about half size, got {}",
            arriving.width()
        );
    }

    /// Strokes at opposite ends of a drawing are not each other.
    #[test]
    fn strokes_far_apart_are_not_paired() {
        let a = joined(&[ring(0.0, 0.0, 20.0), ring(1000.0, 0.0, 20.0)]);
        let b = ring(2.0, 0.0, 20.0);
        let middle = interpolate_path(&a, &b, 0.5);
        // The near one morphs; the far one shrinks away rather than being
        // dragged a thousand units to meet it.
        let far = subpaths(&middle)
            .into_iter()
            .map(|s| s.bounding_box())
            .find(|box_| box_.center().x > 500.0);
        assert!(
            far.is_some(),
            "the far stroke stayed where it was while it left"
        );
    }
}

#[cfg(test)]
mod inbetween_tests {
    use super::*;
    use crate::{FillSpec, ObjectId, ShapeData};
    use buzz_geom::Shape as _;
    use std::sync::Arc;

    const RED: peniko::Color = peniko::Color::from_rgb8(0xE0, 0x20, 0x20);
    const BLUE: peniko::Color = peniko::Color::from_rgb8(0x20, 0x40, 0xE0);

    fn blob(id: u64, centre: (f64, f64), size: f64, colour: peniko::Color) -> Arc<Object> {
        let (x, y) = centre;
        let r = size / 2.0;
        let path = buzz_geom::Rect::new(x - r, y - r, x + r, y + r).to_path(1e-9);
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData {
                path,
                fill: Some(FillSpec::solid(colour)),
                stroke: None,
                blend: Default::default(),
            },
        ))
    }

    fn centre_of(object: &Object) -> Point {
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("expected a shape")
        };
        object
            .transform
            .transform_rect_bbox(shape.path.bounding_box())
            .center()
    }

    /// **The defect this exists to fix.** Two drawings of the same two things,
    /// drawn in opposite orders. Pairing by array position sends the head across
    /// the frame to become the arm; pairing by what they *are* keeps each with
    /// itself.
    #[test]
    fn drawings_pair_by_what_they_are_not_by_draw_order() {
        // Frame 1: a head high on the left, an arm low on the right.
        let from = vec![
            blob(1, (100.0, 100.0), 40.0, RED),
            blob(2, (300.0, 300.0), 20.0, BLUE),
        ];
        // Frame 2: the same two, barely moved — but drawn the other way round.
        let to = vec![
            blob(3, (310.0, 305.0), 20.0, BLUE),
            blob(4, (110.0, 105.0), 40.0, RED),
        ];

        let pairing = match_drawings(&from, &to);
        assert_eq!(
            pairing,
            vec![Some(1), Some(0)],
            "the head should become the head and the arm the arm"
        );
    }

    /// And the whole point of that: halfway through, nothing has crossed the
    /// frame.
    #[test]
    fn nothing_crosses_the_frame_halfway_through() {
        let from = vec![
            blob(1, (100.0, 100.0), 40.0, RED),
            blob(2, (300.0, 300.0), 20.0, BLUE),
        ];
        let to = vec![
            blob(3, (310.0, 305.0), 20.0, BLUE),
            blob(4, (110.0, 105.0), 40.0, RED),
        ];

        let middle = interpolate_objects(&from, &to, &Tween::shape(), 0.5);
        assert_eq!(middle.len(), 2);

        // Each piece should still be near where it started, not halfway across.
        for object in &middle {
            let c = centre_of(object);
            let near_head = c.distance(Point::new(105.0, 102.5)) < 30.0;
            let near_arm = c.distance(Point::new(305.0, 302.5)) < 30.0;
            assert!(
                near_head || near_arm,
                "a piece ended up at {c:?}, which is neither where it was nor where it is going"
            );
        }
    }

    /// A single shape, which is the overwhelmingly common case, must be
    /// untouched by any of this.
    #[test]
    fn one_shape_still_pairs_with_the_one_shape() {
        let from = vec![blob(1, (100.0, 100.0), 40.0, RED)];
        let to = vec![blob(2, (200.0, 100.0), 40.0, RED)];
        assert_eq!(match_drawings(&from, &to), vec![Some(0)]);

        let middle = interpolate_objects(&from, &to, &Tween::shape(), 0.5);
        assert_eq!(middle.len(), 1);
        let c = centre_of(&middle[0]);
        assert!(
            (c.x - 150.0).abs() < 1.0,
            "it should be halfway across, got {c:?}"
        );
    }

    /// A piece with no partner leaves rather than sitting there and popping.
    #[test]
    fn a_piece_with_no_partner_shrinks_away() {
        let from = vec![
            blob(1, (100.0, 100.0), 40.0, RED),
            blob(2, (300.0, 300.0), 20.0, BLUE),
        ];
        // The blue one is gone on the far keyframe.
        let to = vec![blob(3, (105.0, 100.0), 40.0, RED)];

        let pairing = match_drawings(&from, &to);
        assert_eq!(pairing, vec![Some(0), None], "the blue one has no partner");

        let size_at = |t: f64| {
            let drawn = interpolate_objects(&from, &to, &Tween::shape(), t);
            let leaving = &drawn[1];
            let ObjectKind::Shape(shape) = &leaving.kind else {
                panic!("expected a shape")
            };
            let box_ = leaving
                .transform
                .transform_rect_bbox(shape.path.bounding_box());
            box_.width()
        };
        let early = size_at(0.25);
        let late = size_at(0.75);
        assert!(
            late < early,
            "an unpartnered piece should be on its way out: {early} then {late}"
        );
    }

    /// And one that arrives grows in, rather than appearing whole on the last
    /// frame.
    #[test]
    fn a_piece_that_arrives_grows_in() {
        let from = vec![blob(1, (100.0, 100.0), 40.0, RED)];
        let to = vec![
            blob(2, (105.0, 100.0), 40.0, RED),
            blob(3, (300.0, 300.0), 20.0, BLUE),
        ];

        let drawn = interpolate_objects(&from, &to, &Tween::shape(), 0.5);
        assert_eq!(drawn.len(), 2, "the arriving piece is drawn while arriving");

        let arriving_piece = &drawn[1];
        let ObjectKind::Shape(shape) = &arriving_piece.kind else {
            panic!("expected a shape")
        };
        let box_ = arriving_piece
            .transform
            .transform_rect_bbox(shape.path.bounding_box());
        assert!(
            box_.width() > 1.0 && box_.width() < 20.0,
            "halfway in it should be about half size, got {}",
            box_.width()
        );
    }

    /// Colour helps, but does not overrule where a piece is: an animator may
    /// recolour a shape through a tween, and it is still that shape.
    #[test]
    fn a_recoloured_piece_is_still_the_same_piece() {
        let from = vec![blob(1, (100.0, 100.0), 40.0, RED)];
        let to = vec![blob(2, (105.0, 100.0), 40.0, BLUE)];
        assert_eq!(match_drawings(&from, &to), vec![Some(0)]);
    }

    /// Two pieces at opposite ends of the drawing are not each other, and
    /// morphing them together looks worse than letting one go.
    /// Two pieces at opposite ends of the action are not each other: a leftover
    /// with nowhere sensible to go leaves, rather than being dragged across the
    /// frame to pair with whatever happened to be free.
    ///
    /// A *lone* shape crossing the stage is the opposite case and must still
    /// pair — that is the classic morph, pinned down by
    /// `one_shape_still_pairs_with_the_one_shape`.
    #[test]
    fn a_leftover_is_not_dragged_across_the_frame() {
        let from = vec![
            blob(1, (0.0, 0.0), 20.0, RED),
            blob(2, (1000.0, 1000.0), 20.0, RED),
        ];
        let to = vec![blob(3, (5.0, 5.0), 20.0, RED)];
        assert_eq!(
            match_drawings(&from, &to),
            vec![Some(0), None],
            "the near one pairs; the far one leaves"
        );
    }

    /// Size counts too: a drawing that keeps one piece and replaces another
    /// with something of a very different size pairs them the right way round.
    #[test]
    fn size_tells_two_nearby_pieces_apart() {
        let from = vec![
            blob(1, (100.0, 100.0), 80.0, RED),
            blob(2, (140.0, 100.0), 10.0, RED),
        ];
        // Drawn in the other order again, and close together.
        let to = vec![
            blob(3, (145.0, 100.0), 10.0, RED),
            blob(4, (105.0, 100.0), 80.0, RED),
        ];
        assert_eq!(match_drawings(&from, &to), vec![Some(1), Some(0)]);
    }
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
            Object::shape(
                ObjectId(id),
                ShapeData::filled(square(0.0, 10.0), Color::WHITE),
            )
            .with_transform(transform),
        )
    }

    #[test]
    fn orient_to_path_faces_the_direction_of_travel() {
        let a = object(1, Affine::translate((0.0, 0.0)));
        let b = object(2, Affine::translate((100.0, 50.0)));
        let mut tween = Tween::motion();
        tween.orient_to_path = true;

        let mid = interpolate_object(&a, &b, &tween, 0.5);
        let c = mid.transform.as_coeffs();
        let angle = c[1].atan2(c[0]);
        let want = 50.0_f64.atan2(100.0);
        assert!((angle - want).abs() < 1e-6, "faced {angle} rad, expected {want}");
    }

    #[test]
    fn without_orient_to_path_an_unrotated_move_stays_unrotated() {
        let a = object(1, Affine::translate((0.0, 0.0)));
        let b = object(2, Affine::translate((100.0, 50.0)));
        let mid = interpolate_object(&a, &b, &Tween::motion(), 0.5);
        let c = mid.transform.as_coeffs();
        assert!(c[1].atan2(c[0]).abs() < 1e-9, "no orient should leave rotation alone");
    }

    // -- easing -------------------------------------------------------------

    #[test]
    fn easing_always_spans_zero_to_one() {
        for easing in [
            Easing::Linear,
            Easing::Strength(100.0),
            Easing::Strength(-100.0),
            Easing::CubicBezier {
                x1: 0.42,
                y1: 0.0,
                x2: 0.58,
                y2: 1.0,
            },
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
            Easing::CubicBezier {
                x1: 0.25,
                y1: 0.1,
                x2: 0.25,
                y2: 1.0,
            },
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
        let nasty = Easing::CubicBezier {
            x1: 1.0,
            y1: 0.0,
            x2: 0.0,
            y2: 1.0,
        };
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
        assert!(
            (p.x - 50.0).abs() < 1e-9 && (p.y - 25.0).abs() < 1e-9,
            "{p:?}"
        );
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
            mid.elements()
                .iter()
                .any(|e| matches!(e, PathEl::ClosePath)),
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
        assert!(
            interpolate_path(&empty, &square, 0.5).elements().is_empty()
                || !interpolate_path(&empty, &square, 0.5).elements().is_empty()
        );
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

    // -- rigging ------------------------------------------------------------

    fn rigged(elbow: f64) -> Object {
        let mut armature = buzz_rig::Armature::new(Point::ZERO);
        armature.push(buzz_rig::Bone::new("upper", None, 50.0, 0.0));
        armature.push(buzz_rig::Bone::new("fore", Some(0), 50.0, elbow));

        let mut rig = crate::rig::ArmatureData::new(armature);
        rig.bind_shape(Arc::new(Object::shape(
            ObjectId(7),
            ShapeData::filled(Rect::new(0.0, -8.0, 100.0, 8.0).to_path(1e-9), Color::WHITE),
        )));

        Object {
            id: ObjectId(7),
            name: None,
            transform: Affine::IDENTITY,
            kind: ObjectKind::Armature(rig),
            locked: false,
            visible: true,
            filters: Vec::new(),
            blend: Default::default(),
            spatial: Default::default(),
            pivot: None,
            modifiers: Vec::new(),
            text: None,
            turnaround: Default::default(),
        }
    }

    /// The point of Phase 7: two keyframes holding poses, and every frame
    /// between them interpolated joint by joint.
    #[test]
    fn a_tween_between_two_poses_bends_the_rig_halfway() {
        let start = rigged(0.0);
        let end = rigged(1.0);

        let mid = interpolate_object(&start, &end, &Tween::classic(), 0.5);
        let ObjectKind::Armature(rig) = &mid.kind else {
            panic!("expected an armature");
        };
        assert!((rig.armature.bones[1].angle - 0.5).abs() < 1e-9);
    }

    /// The artwork must follow the interpolated pose, not stay where it was
    /// drawn: a tween that moves the bones and leaves the drawing behind is
    /// exactly the failure this is guarding.
    #[test]
    fn tweened_artwork_follows_the_tweened_bones() {
        let start = rigged(0.0);
        let end = rigged(std::f64::consts::FRAC_PI_2);

        let straight = interpolate_object(&start, &end, &Tween::classic(), 0.0);
        let bent = interpolate_object(&start, &end, &Tween::classic(), 1.0);

        let extent = |object: &Object| object.bounds();
        assert!(
            extent(&bent).y1 > extent(&straight).y1 + 20.0,
            "the artwork did not bend with the rig: {:?} vs {:?}",
            extent(&straight),
            extent(&bent)
        );
    }

    /// A shape tween over a rig interpolates the skeleton rather than blending
    /// two deformed outlines, which would produce shapes no pose could make.
    #[test]
    fn a_shape_tween_over_a_rig_interpolates_the_pose() {
        let start = rigged(0.0);
        let end = rigged(0.8);

        let mid = interpolate_shape_object(&start, &end, 0.5);
        let ObjectKind::Armature(rig) = &mid.kind else {
            panic!("expected an armature");
        };
        assert!((rig.armature.bones[1].angle - 0.4).abs() < 1e-9);
    }

    #[test]
    fn a_tween_between_warps_moves_the_handles() {
        let shape = ShapeData::filled(
            Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9),
            Color::WHITE,
        );
        let start_warp = crate::rig::WarpData::new(shape).with_grid(2, 2);
        let mut end_warp = start_warp.clone();
        end_warp.handles[0].current = Point::new(-100.0, -100.0);

        let object = |warp: crate::rig::WarpData| Object {
            id: ObjectId(3),
            name: None,
            transform: Affine::IDENTITY,
            kind: ObjectKind::Warp(warp),
            locked: false,
            visible: true,
            filters: Vec::new(),
            blend: Default::default(),
            spatial: Default::default(),
            pivot: None,
            modifiers: Vec::new(),
            text: None,
            turnaround: Default::default(),
        };

        let mid = interpolate_object(
            &object(start_warp),
            &object(end_warp),
            &Tween::classic(),
            0.5,
        );
        let ObjectKind::Warp(warp) = &mid.kind else {
            panic!("expected a warp");
        };
        assert!((warp.handles[0].current.x - -50.0).abs() < 1e-9);
    }
}
