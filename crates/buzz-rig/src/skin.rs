//! Binding artwork to bones, and deforming it when they move.
//!
//! # What binding is
//!
//! Every point of a path — anchors *and* Bézier control points — is given a
//! weight for each bone, worked out once at bind time from how close it lies
//! to that bone in the rest pose. Posing the armature then moves each point by
//! the weighted blend of its bones' transforms. Near the middle of a bone the
//! weight is effectively 1 and the artwork moves rigidly with it; across a
//! joint the weights of the two bones meet and the artwork bends instead of
//! tearing.
//!
//! Control points are weighted like any other point, which is what keeps a
//! curve a curve: weighting only the anchors would drag the ends of a segment
//! while its handles stayed behind, and the curve would fold through itself.
//!
//! # Why weights are stored rather than recomputed
//!
//! A distance-based weight recomputed from the *posed* skeleton would change
//! as the character moves — artwork would swap allegiance from one bone to
//! another mid-animation and visibly pop. Binding once, against the rest pose,
//! is what makes the deformation stable, and it is what Animate does when it
//! says the artwork is bound to the armature.

use buzz_geom::{BezPath, Point};
use serde::{Deserialize, Serialize};

use crate::{Armature, distance_to_segment};

/// How sharply influence falls off with distance from a bone.
///
/// Two is the usual choice: influence as the inverse square of distance is
/// firm enough that a limb moves as a limb, and soft enough that a joint bends
/// rather than creasing.
const FALLOFF: f64 = 2.0;

/// Weights below this are dropped.
///
/// A point kept under the influence of every bone in the rig costs a transform
/// per bone per frame for a contribution that cannot be seen. Dropping them
/// makes deformation proportional to the bones that actually matter.
const MIN_WEIGHT: f64 = 0.02;

/// At most this many bones may move one point.
///
/// Four is the number every real-time skinning implementation settles on, for
/// the same reason: a point genuinely influenced by five bones is a rig
/// problem, not a weighting problem.
const MAX_INFLUENCES: usize = 4;

/// What each point of a path is attached to.
///
/// Parallel to the path's own point order, so applying it is a single pass and
/// no lookup. A binding is only valid for the path it was made from — the
/// length is checked on use, and a mismatch deforms nothing rather than
/// producing garbage from mismatched indices.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkinBinding {
    /// Per point: `(bone index, weight)`, weights summing to one.
    pub points: Vec<Vec<(usize, f64)>>,
}

impl SkinBinding {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// How many bones influence the most-influenced point.
    pub fn max_influences(&self) -> usize {
        self.points.iter().map(|p| p.len()).max().unwrap_or(0)
    }
}

/// Work out weights for every point of `path` against the armature's rest pose.
pub fn bind_path(path: &BezPath, armature: &Armature) -> SkinBinding {
    let rest = armature.at_rest();
    let segments = rest.joints();
    if segments.is_empty() {
        return SkinBinding::default();
    }

    let points = path_points(path)
        .into_iter()
        .map(|point| weights_for(point, &segments))
        .collect();

    SkinBinding { points }
}

/// Weights for one point, nearest bones first.
fn weights_for(point: Point, segments: &[(Point, Point)]) -> Vec<(usize, f64)> {
    let mut raw: Vec<(usize, f64)> = segments
        .iter()
        .enumerate()
        .map(|(i, (head, tip))| {
            let distance = distance_to_segment(point, *head, *tip);
            // A point sitting exactly on a bone would divide by zero; the
            // epsilon makes that the strongest possible influence instead of
            // an infinity that poisons the normalisation.
            let weight = 1.0 / (distance.powf(FALLOFF) + 1e-6);
            (i, weight)
        })
        .collect();

    raw.sort_by(|a, b| b.1.total_cmp(&a.1));
    raw.truncate(MAX_INFLUENCES);

    let total: f64 = raw.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return vec![(0, 1.0)];
    }

    let mut normalised: Vec<(usize, f64)> = raw
        .into_iter()
        .map(|(i, w)| (i, w / total))
        .filter(|(_, w)| *w >= MIN_WEIGHT)
        .collect();

    // Dropping the small ones leaves the rest summing to less than one, which
    // would shrink the artwork towards the origin. Re-normalise.
    let kept: f64 = normalised.iter().map(|(_, w)| w).sum();
    if kept > 0.0 {
        for (_, w) in &mut normalised {
            *w /= kept;
        }
    } else {
        normalised = vec![(0, 1.0)];
    }
    normalised
}

/// Move `path` to match the armature's current pose.
///
/// The path must be the one the binding was made from. A binding of a
/// different length returns the path untouched rather than reading weights
/// past their end: unchanged artwork is a visible, correctable mistake, and
/// garbage indices are not.
///
/// **That check is a bounds guard, not proof of identity.** Two different
/// paths with the same number of points would pass it and deform wrongly.
/// Identity is guaranteed a level up instead, by the document model keeping a
/// path and its binding inside one object and rebinding whenever the path is
/// replaced — which is the only place that can actually know they belong
/// together. A fingerprint here would be a second, weaker answer to a question
/// already settled.
pub fn deform_path(path: &BezPath, binding: &SkinBinding, armature: &Armature) -> BezPath {
    if binding.is_empty() || armature.is_empty() {
        return path.clone();
    }
    let source = path_points(path);
    if source.len() != binding.points.len() {
        return path.clone();
    }

    // One transform per bone, not one per point: a 2 000-point path over a
    // 20-bone rig would otherwise rebuild the same twenty matrices two
    // thousand times.
    let transforms: Vec<buzz_geom::Affine> = (0..armature.len())
        .map(|i| armature.pose_transform(i))
        .collect();

    let moved: Vec<Point> = source
        .iter()
        .zip(&binding.points)
        .map(|(point, weights)| {
            let mut x = 0.0;
            let mut y = 0.0;
            for (bone, weight) in weights {
                let Some(transform) = transforms.get(*bone) else {
                    continue;
                };
                let p = *transform * *point;
                x += p.x * weight;
                y += p.y * weight;
            }
            Point::new(x, y)
        })
        .collect();

    rebuild_path(path, &moved)
}

/// Rebuild a path with every point — anchors and control points alike — put
/// through `move_point`.
///
/// Shared with the warp: both deformations differ only in where a point goes,
/// never in how a path is taken apart and put back together.
pub(crate) fn map_path_points(path: &BezPath, move_point: impl Fn(Point) -> Point) -> BezPath {
    let moved: Vec<Point> = path_points(path).into_iter().map(move_point).collect();
    rebuild_path(path, &moved)
}

/// Every point a path is built from, in order.
fn path_points(path: &BezPath) -> Vec<Point> {
    use kurbo::PathEl;
    let mut out = Vec::new();
    for element in path.elements() {
        match element {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => out.push(*p),
            PathEl::QuadTo(a, b) => out.extend([*a, *b]),
            PathEl::CurveTo(a, b, c) => out.extend([*a, *b, *c]),
            PathEl::ClosePath => {}
        }
    }
    out
}

/// Rebuild a path with the same structure and new points.
fn rebuild_path(path: &BezPath, points: &[Point]) -> BezPath {
    use kurbo::PathEl;
    let mut out = BezPath::new();
    let mut next = 0;
    let mut take = |n: usize| -> Option<&[Point]> {
        let slice = points.get(next..next + n)?;
        next += n;
        Some(slice)
    };

    for element in path.elements() {
        match element {
            PathEl::MoveTo(_) => match take(1) {
                Some([p]) => out.move_to(*p),
                _ => return path.clone(),
            },
            PathEl::LineTo(_) => match take(1) {
                Some([p]) => out.line_to(*p),
                _ => return path.clone(),
            },
            PathEl::QuadTo(_, _) => match take(2) {
                Some([a, b]) => out.quad_to(*a, *b),
                _ => return path.clone(),
            },
            PathEl::CurveTo(_, _, _) => match take(3) {
                Some([a, b, c]) => out.curve_to(*a, *b, *c),
                _ => return path.clone(),
            },
            PathEl::ClosePath => out.close_path(),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bone;
    use buzz_geom::{Rect, Shape as _};
    use std::f64::consts::FRAC_PI_2;

    /// A two-bone arm along the x axis from the origin, 100 units long.
    fn arm() -> Armature {
        let mut armature = Armature::new(Point::ZERO);
        armature.push(Bone::new("upper", None, 50.0, 0.0));
        armature.push(Bone::new("fore", Some(0), 50.0, 0.0));
        armature
    }

    /// A long thin rectangle lying along the arm, as artwork would.
    fn limb() -> BezPath {
        Rect::new(0.0, -10.0, 100.0, 10.0).to_path(1e-9)
    }

    #[test]
    fn weights_sum_to_one_at_every_point() {
        let binding = bind_path(&limb(), &arm());
        assert!(!binding.is_empty());
        for (i, weights) in binding.points.iter().enumerate() {
            let total: f64 = weights.iter().map(|(_, w)| w).sum();
            assert!((total - 1.0).abs() < 1e-9, "point {i} sums to {total}");
            assert!(!weights.is_empty(), "point {i} has no bone");
        }
    }

    #[test]
    fn a_point_beside_a_bone_belongs_mostly_to_that_bone() {
        let arm = arm();
        let path = BezPath::from_vec(vec![
            kurbo::PathEl::MoveTo(Point::new(10.0, 2.0)), // beside the upper arm
            kurbo::PathEl::LineTo(Point::new(90.0, 2.0)), // beside the forearm
        ]);
        let binding = bind_path(&path, &arm);

        let strongest = |weights: &Vec<(usize, f64)>| {
            weights
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| *i)
                .expect("a bone")
        };
        assert_eq!(strongest(&binding.points[0]), 0);
        assert_eq!(strongest(&binding.points[1]), 1);
    }

    #[test]
    fn no_point_is_influenced_by_more_bones_than_the_cap() {
        let mut armature = Armature::new(Point::ZERO);
        armature.push(Bone::new("b0", None, 10.0, 0.0));
        for i in 1..8 {
            armature.push(Bone::new(format!("b{i}"), Some(i - 1), 10.0, 0.0));
        }
        let binding = bind_path(&limb(), &armature);
        assert!(binding.max_influences() <= MAX_INFLUENCES);
    }

    /// The property everything else rests on: binding then posing at rest must
    /// leave the artwork exactly where it was drawn.
    #[test]
    fn deforming_at_rest_changes_nothing() {
        let arm = arm();
        let path = limb();
        let binding = bind_path(&path, &arm);
        let deformed = deform_path(&path, &binding, &arm);

        for (a, b) in path_points(&path).iter().zip(path_points(&deformed)) {
            assert!((*a - b).hypot() < 1e-9, "{a:?} moved to {b:?}");
        }
    }

    #[test]
    fn bending_the_elbow_carries_the_far_end_round() {
        let mut arm = arm();
        let path = limb();
        let binding = bind_path(&path, &arm);

        arm.bones[1].angle = FRAC_PI_2; // forearm turns down the screen

        let deformed = deform_path(&path, &binding, &arm);
        let bounds = deformed.bounding_box();

        // The far end has swung below the axis rather than staying to the right.
        assert!(
            bounds.y1 > 40.0,
            "the forearm did not swing down: {bounds:?}"
        );
        assert!(
            bounds.x1 < 95.0,
            "the artwork still reaches right: {bounds:?}"
        );
    }

    #[test]
    fn the_shape_of_the_path_is_preserved() {
        let arm = arm();
        // A path with a curve in it, to prove control points survive.
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.curve_to(
            Point::new(30.0, -20.0),
            Point::new(70.0, 20.0),
            Point::new(100.0, 0.0),
        );
        path.close_path();

        let binding = bind_path(&path, &arm);
        let deformed = deform_path(&path, &binding, &arm);

        assert_eq!(
            deformed.elements().len(),
            path.elements().len(),
            "the path structure changed"
        );
        assert!(matches!(
            deformed.elements().last(),
            Some(kurbo::PathEl::ClosePath)
        ));
    }

    /// A binding whose length does not match the path must not be read past
    /// its end. Applied to a path of a different size, it deforms nothing.
    #[test]
    fn a_binding_of_the_wrong_length_leaves_the_artwork_alone() {
        let mut arm = arm();
        let binding = bind_path(&limb(), &arm);
        arm.bones[0].angle = 1.0;

        // A triangle: three points where the binding expects a rectangle's.
        let mut other = BezPath::new();
        other.move_to(Point::new(0.0, 0.0));
        other.line_to(Point::new(10.0, 0.0));
        other.line_to(Point::new(5.0, 8.0));
        other.close_path();

        let deformed = deform_path(&other, &binding, &arm);
        assert_eq!(deformed.to_svg(), other.to_svg());
    }

    /// The limitation stated in `deform_path`'s documentation, pinned by a
    /// test so it is a known property rather than a surprise: the length check
    /// cannot tell two same-sized paths apart. What stops that happening is
    /// the document model keeping a path and its binding together, not this
    /// guard.
    #[test]
    fn a_same_length_binding_from_another_path_is_not_detected() {
        let mut arm = arm();
        let binding = bind_path(&limb(), &arm);
        arm.bones[1].angle = 1.0;

        let other = Rect::new(0.0, 0.0, 5.0, 5.0).to_path(1e-9);
        let deformed = deform_path(&other, &binding, &arm);
        assert_ne!(
            deformed.to_svg(),
            other.to_svg(),
            "if this ever passes, deform_path has gained an identity check and \
             its documentation needs updating"
        );
    }

    #[test]
    fn an_armature_with_no_bones_binds_nothing_and_deforms_nothing() {
        let empty = Armature::default();
        let path = limb();
        let binding = bind_path(&path, &empty);

        assert!(binding.is_empty());
        assert_eq!(deform_path(&path, &binding, &empty).to_svg(), path.to_svg());
    }

    /// Weights are bound against the *rest* pose, so binding a rig that is
    /// currently posed must not bake that pose into the weights.
    #[test]
    fn binding_uses_the_rest_pose_not_the_current_one() {
        let mut posed = arm();
        posed.bones[1].angle = FRAC_PI_2;

        let path = limb();
        let from_posed = bind_path(&path, &posed);
        let from_rest = bind_path(&path, &arm());

        assert_eq!(
            from_posed, from_rest,
            "the current pose leaked into the weights"
        );
    }

    /// A point on the far side of a joint is shared between the two bones, so
    /// the artwork bends there instead of tearing.
    #[test]
    fn artwork_across_a_joint_is_shared_between_both_bones() {
        let arm = arm();
        let path = BezPath::from_vec(vec![kurbo::PathEl::MoveTo(Point::new(50.0, 0.0))]);
        let binding = bind_path(&path, &arm);

        let weights = &binding.points[0];
        assert!(
            weights.len() >= 2,
            "the elbow should be shared: {weights:?}"
        );
        let smallest = weights.iter().map(|(_, w)| *w).fold(f64::MAX, f64::min);
        assert!(smallest > 0.2, "one bone dominates the joint: {weights:?}");
    }
}
