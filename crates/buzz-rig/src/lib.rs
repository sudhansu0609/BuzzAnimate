//! Rigging: armatures, inverse kinematics, skinning and mesh warping.
//!
//! This is Animate's Bone tool and Asset Warp tool, underneath. Nothing here
//! knows about documents, layers or undo — an [`Armature`] is bones and angles,
//! and the document model wraps it. That split is what lets the solver be
//! tested exhaustively without building a scene, and it is why the solver can
//! run across every core without dragging document state along with it.
//!
//! # The angle convention, stated once
//!
//! The stage measures **y downwards**, so a positive angle turns *clockwise*
//! on screen. Every angle here is in radians and is **relative to the parent
//! bone**, which is what makes a pose portable: bending an elbow is one
//! number, whatever the shoulder is doing.
//!
//! # What a pose is
//!
//! Each bone carries a `rest_angle` and an `angle`. The rest pose is the shape
//! the artwork was drawn in and what skin weights are bound against; the pose
//! is where the bone is now. A document keyframe stores angles, so tweening
//! between poses is interpolating numbers rather than rebuilding a rig.

pub mod ik;
pub mod skin;
pub mod warp;

use buzz_geom::{Affine, Point, Vec2};
use serde::{Deserialize, Serialize};

pub use ik::{IkOptions, IkOutcome, solve_to};
pub use skin::{SkinBinding, bind_path, deform_path};
pub use warp::{WarpHandle, warp_path};

/// One bone: a segment with a joint at its head.
///
/// Animate's vocabulary throughout — a bone has a *head* (the joint it turns
/// about) and a *tip* (where the next bone starts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    /// Index of the parent bone, or `None` for a root bone.
    ///
    /// An index rather than an id because an armature is small, always walked
    /// whole, and stored as one unit: a map from ids would add a lookup to
    /// every step of every solve for no gain.
    pub parent: Option<usize>,
    /// Length in document units.
    pub length: f64,
    /// The angle this bone was drawn at, relative to its parent.
    pub rest_angle: f64,
    /// Where the bone is now, relative to its parent.
    pub angle: f64,
    /// Animate's joint rotation limits, relative to the parent. `None` lets
    /// the joint turn freely.
    pub limits: Option<JointLimits>,
    /// A pinned joint stays where it is: IK solves *up to* it and no further.
    pub pinned: bool,
}

impl Bone {
    /// A bone of `length`, at `angle` relative to its parent.
    pub fn new(name: impl Into<String>, parent: Option<usize>, length: f64, angle: f64) -> Self {
        Self {
            name: name.into(),
            parent,
            length: length.max(0.0),
            rest_angle: angle,
            angle,
            limits: None,
            pinned: false,
        }
    }

    /// Restrict the joint, in radians relative to the parent.
    pub fn with_limits(mut self, min: f64, max: f64) -> Self {
        self.limits = Some(JointLimits::new(min, max));
        self
    }

    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    /// The pose angle, clamped into the joint's limits.
    fn constrain(&self, angle: f64) -> f64 {
        match &self.limits {
            Some(limits) => limits.clamp(angle),
            None => angle,
        }
    }
}

/// How far a joint may turn, relative to its parent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JointLimits {
    pub min: f64,
    pub max: f64,
}

impl JointLimits {
    /// Ordered on construction, so limits typed backwards restrict the joint
    /// rather than locking it solid at whichever value happened to be first.
    pub fn new(min: f64, max: f64) -> Self {
        Self {
            min: min.min(max),
            max: min.max(max),
        }
    }

    pub fn clamp(&self, angle: f64) -> f64 {
        // Wrapped into the same turn as the limits before clamping: an angle
        // of 3.2 rad and a limit of -3.0..-2.9 rad describe nearby directions,
        // and clamping the raw numbers would swing the bone the long way
        // round to a place the user did not ask for.
        let centre = (self.min + self.max) * 0.5;
        let wrapped = centre + wrap_pi(angle - centre);
        wrapped.clamp(self.min, self.max)
    }

    pub fn span(&self) -> f64 {
        self.max - self.min
    }
}

/// Bring an angle into `-π..=π`.
pub fn wrap_pi(angle: f64) -> f64 {
    let two_pi = std::f64::consts::TAU;
    let mut a = angle % two_pi;
    if a > std::f64::consts::PI {
        a -= two_pi;
    } else if a < -std::f64::consts::PI {
        a += two_pi;
    }
    a
}

/// A skeleton: a root position and a tree of bones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Armature {
    /// Where the first joint sits, in the artwork's own coordinates.
    pub root: Point,
    pub bones: Vec<Bone>,
}

impl Default for Armature {
    fn default() -> Self {
        Self {
            root: Point::ZERO,
            bones: Vec::new(),
        }
    }
}

impl Armature {
    pub fn new(root: Point) -> Self {
        Self {
            root,
            bones: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    pub fn len(&self) -> usize {
        self.bones.len()
    }

    /// Add a bone, returning its index.
    ///
    /// A parent index that does not exist, or that is not *before* this bone,
    /// is dropped to `None`. Bones are stored parents-first so every walk is a
    /// single forward pass; accepting a forward reference would allow a cycle,
    /// and a cycle in a skeleton is an infinite loop in the solver.
    pub fn push(&mut self, mut bone: Bone) -> usize {
        let index = self.bones.len();
        if bone.parent.is_some_and(|p| p >= index) {
            bone.parent = None;
        }
        self.bones.push(bone);
        index
    }

    /// Add a bone from head to tip, working out its length and angle.
    ///
    /// This is what the Bone tool does: the user drags, and the drag *is* the
    /// bone. The angle stored is relative to the parent, so the parent's own
    /// direction is subtracted here rather than being the caller's problem.
    pub fn push_dragged(
        &mut self,
        name: impl Into<String>,
        parent: Option<usize>,
        head: Point,
        tip: Point,
    ) -> usize {
        let parent = parent.filter(|p| *p < self.bones.len());
        let delta = tip - head;
        let absolute = delta.y.atan2(delta.x);
        let angle = match parent {
            Some(p) => wrap_pi(absolute - self.world_angle(p)),
            None => absolute,
        };
        if parent.is_none() {
            // A first bone defines where the armature starts.
            if self.bones.is_empty() {
                self.root = head;
            }
        }
        self.push(Bone::new(name, parent, delta.hypot(), angle))
    }

    /// The bone's direction in world space: its own angle plus every ancestor.
    pub fn world_angle(&self, index: usize) -> f64 {
        let mut angle = 0.0;
        let mut current = Some(index);
        let mut guard = 0;
        while let Some(i) = current {
            let Some(bone) = self.bones.get(i) else { break };
            angle += bone.angle;
            current = bone.parent;

            // A malformed armature — one that arrived from a file rather than
            // from `push` — must not hang the renderer.
            guard += 1;
            if guard > self.bones.len() {
                break;
            }
        }
        angle
    }

    /// Where a bone's head sits, in the artwork's coordinates.
    pub fn head(&self, index: usize) -> Point {
        match self.bones.get(index).and_then(|b| b.parent) {
            Some(parent) => self.tip(parent),
            None => self.root,
        }
    }

    /// Where a bone's tip sits.
    pub fn tip(&self, index: usize) -> Point {
        let Some(bone) = self.bones.get(index) else {
            return self.root;
        };
        let angle = self.world_angle(index);
        let head = self.head(index);
        head + Vec2::new(angle.cos(), angle.sin()) * bone.length
    }

    /// Head and tip of every bone, in one pass.
    ///
    /// Cheaper than asking per bone: `head`/`tip` each walk to the root, so a
    /// chain of `n` bones costs `O(n²)` that way and `O(n)` this way. The
    /// renderer and the hit test both want the whole set.
    pub fn joints(&self) -> Vec<(Point, Point)> {
        let mut out: Vec<(Point, Point)> = Vec::with_capacity(self.bones.len());
        let mut angles: Vec<f64> = Vec::with_capacity(self.bones.len());

        for (i, bone) in self.bones.iter().enumerate() {
            let (head, parent_angle) = match bone.parent {
                // Parents always come first, so this is already computed.
                Some(p) if p < i => (out[p].1, angles[p]),
                _ => (self.root, 0.0),
            };
            let angle = parent_angle + bone.angle;
            let tip = head + Vec2::new(angle.cos(), angle.sin()) * bone.length;
            angles.push(angle);
            out.push((head, tip));
        }
        out
    }

    /// The transform that carries the bone's rest position to its pose.
    ///
    /// This is what skinning applies: a point drawn near a bone moves with it,
    /// turning about the bone's head.
    pub fn pose_transform(&self, index: usize) -> Affine {
        let rest = self.at_rest();
        let rest_head = rest.head(index);
        let rest_angle = rest.world_angle(index);
        let head = self.head(index);
        let angle = self.world_angle(index);

        Affine::translate(head.to_vec2())
            * Affine::rotate(angle - rest_angle)
            * Affine::translate(-rest_head.to_vec2())
    }

    /// The same armature with every bone back at the angle it was drawn.
    pub fn at_rest(&self) -> Armature {
        let mut rest = self.clone();
        for bone in &mut rest.bones {
            bone.angle = bone.rest_angle;
        }
        rest
    }

    /// Adopt the current pose as the rest pose.
    pub fn set_rest_here(&mut self) {
        for bone in &mut self.bones {
            bone.rest_angle = bone.angle;
        }
    }

    /// Every bone's pose angle, for a keyframe.
    pub fn pose(&self) -> Vec<f64> {
        self.bones.iter().map(|b| b.angle).collect()
    }

    /// Adopt a pose, clamping each angle into its joint's limits.
    ///
    /// A pose of the wrong length is applied as far as it goes rather than
    /// refused: a rig edited after a pose was keyed should lose that bone's
    /// animation, not the whole keyframe's.
    pub fn set_pose(&mut self, pose: &[f64]) {
        for (bone, angle) in self.bones.iter_mut().zip(pose) {
            bone.angle = bone.constrain(*angle);
        }
    }

    /// Interpolate between two poses of this armature, for a tween.
    ///
    /// Each joint turns the **shortest way round**, so a bone at 350° moving
    /// to 10° turns forward 20° rather than backwards 340° — the same rule the
    /// camera and classic tweens already follow.
    pub fn tween_pose(from: &[f64], to: &[f64], t: f64) -> Vec<f64> {
        from.iter()
            .zip(to)
            .map(|(a, b)| a + wrap_pi(b - a) * t)
            .collect()
    }

    /// Which bone is nearest `point`, and how far away it is.
    ///
    /// Distance is to the bone *segment*, not to its head — a bone is picked
    /// by clicking anywhere along it, as in Animate.
    pub fn nearest_bone(&self, point: Point) -> Option<(usize, f64)> {
        self.joints()
            .iter()
            .enumerate()
            .map(|(i, (head, tip))| (i, distance_to_segment(point, *head, *tip)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The rectangle every joint falls inside.
    pub fn bounds(&self) -> Option<buzz_geom::Rect> {
        let joints = self.joints();
        if joints.is_empty() {
            return None;
        }
        let mut rect = buzz_geom::Rect::from_points(self.root, self.root);
        for (head, tip) in joints {
            rect = rect.union(buzz_geom::Rect::from_points(head, tip));
        }
        Some(rect)
    }

    /// Every descendant of `index`, including itself, nearest first.
    pub fn subtree(&self, index: usize) -> Vec<usize> {
        let mut out = vec![index];
        for (i, bone) in self.bones.iter().enumerate() {
            if let Some(parent) = bone.parent
                && out.contains(&parent)
                && !out.contains(&i)
            {
                out.push(i);
            }
        }
        out
    }
}

/// Distance from a point to a line segment.
pub(crate) fn distance_to_segment(point: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let length_squared = ab.hypot2();
    if length_squared <= f64::EPSILON {
        return (point - a).hypot();
    }
    let t = ((point - a).dot(ab) / length_squared).clamp(0.0, 1.0);
    (point - (a + ab * t)).hypot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    /// Two bones to the right, elbow straight: the classic test arm.
    fn arm() -> Armature {
        let mut armature = Armature::new(Point::new(100.0, 100.0));
        armature.push(Bone::new("upper", None, 50.0, 0.0));
        armature.push(Bone::new("fore", Some(0), 40.0, 0.0));
        armature
    }

    fn close(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    #[test]
    fn a_straight_chain_lays_end_to_end() {
        let arm = arm();
        assert!(close(arm.head(0), Point::new(100.0, 100.0)));
        assert!(close(arm.tip(0), Point::new(150.0, 100.0)));
        assert!(close(arm.head(1), Point::new(150.0, 100.0)), "the elbow");
        assert!(close(arm.tip(1), Point::new(190.0, 100.0)));
    }

    /// Angles are relative to the parent, which is the whole point: bending
    /// the shoulder must carry the forearm with it.
    #[test]
    fn a_child_angle_is_relative_to_its_parent() {
        let mut arm = arm();
        arm.bones[0].angle = FRAC_PI_2; // shoulder turns down the screen
        assert!(close(arm.tip(0), Point::new(100.0, 150.0)));
        // The forearm's own angle is still 0, so it continues in the same
        // direction rather than snapping back to horizontal.
        assert!(close(arm.tip(1), Point::new(100.0, 190.0)));

        arm.bones[1].angle = -FRAC_PI_2; // elbow bends back
        assert!(close(arm.tip(1), Point::new(140.0, 150.0)));
    }

    #[test]
    fn joints_agrees_with_walking_each_bone() {
        let mut arm = arm();
        arm.push(Bone::new("hand", Some(1), 15.0, 0.3));
        arm.bones[0].angle = 0.4;
        arm.bones[1].angle = -0.7;

        for (i, (head, tip)) in arm.joints().iter().enumerate() {
            assert!(close(*head, arm.head(i)), "head {i}");
            assert!(close(*tip, arm.tip(i)), "tip {i}");
        }
    }

    #[test]
    fn dragging_a_bone_records_the_angle_relative_to_its_parent() {
        let mut armature = Armature::new(Point::ZERO);
        armature.push_dragged("upper", None, Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        // Straight down from the first bone's tip: a quarter turn from the
        // parent's direction, not from the world's.
        armature.push_dragged("fore", Some(0), Point::new(100.0, 0.0), Point::new(100.0, 60.0));

        assert!((armature.bones[0].angle - 0.0).abs() < 1e-12);
        assert!((armature.bones[1].angle - FRAC_PI_2).abs() < 1e-12);
        assert!((armature.bones[1].length - 60.0).abs() < 1e-12);
        assert!(close(armature.tip(1), Point::new(100.0, 60.0)));
    }

    #[test]
    fn the_first_dragged_bone_sets_where_the_armature_starts() {
        let mut armature = Armature::default();
        armature.push_dragged("only", None, Point::new(30.0, 40.0), Point::new(30.0, 90.0));
        assert!(close(armature.root, Point::new(30.0, 40.0)));
        assert!(close(armature.tip(0), Point::new(30.0, 90.0)));
    }

    /// A file could name a parent that comes later, which would be a cycle.
    #[test]
    fn a_forward_parent_reference_is_refused() {
        let mut armature = Armature::new(Point::ZERO);
        let index = armature.push(Bone::new("bad", Some(5), 10.0, 0.0));
        assert_eq!(armature.bones[index].parent, None);
        // And it still resolves rather than hanging.
        assert!(close(armature.head(index), Point::ZERO));
    }

    #[test]
    fn joint_limits_are_ordered_and_clamp() {
        let limits = JointLimits::new(1.0, -1.0);
        assert_eq!((limits.min, limits.max), (-1.0, 1.0));
        assert_eq!(limits.clamp(2.0), 1.0);
        assert_eq!(limits.clamp(-2.0), -1.0);
        assert_eq!(limits.clamp(0.5), 0.5);
    }

    /// Angles near the wrap point must clamp to the nearest end of the range,
    /// not swing the long way round.
    #[test]
    fn limits_clamp_across_the_wrap_point() {
        let limits = JointLimits::new(PI - 0.2, PI - 0.1);
        // Just past +π is the same direction as just under -π.
        let clamped = limits.clamp(-PI + 0.05);
        assert!(
            (clamped - (PI - 0.1)).abs() < 1e-9,
            "expected the near end of the range, got {clamped}"
        );
    }

    #[test]
    fn setting_a_pose_respects_the_limits() {
        let mut arm = arm();
        arm.bones[1].limits = Some(JointLimits::new(-0.5, 0.5));
        arm.set_pose(&[1.0, 2.0]);

        assert_eq!(arm.bones[0].angle, 1.0, "an unlimited joint takes the value");
        assert_eq!(arm.bones[1].angle, 0.5, "a limited one is clamped");
    }

    /// A rig edited after a pose was keyed should lose that bone, not the pose.
    #[test]
    fn a_pose_of_the_wrong_length_is_applied_as_far_as_it_goes() {
        let mut arm = arm();
        arm.set_pose(&[0.3]);
        assert_eq!(arm.bones[0].angle, 0.3);
        assert_eq!(arm.bones[1].angle, 0.0, "untouched, not reset");

        arm.set_pose(&[0.1, 0.2, 0.9]);
        assert_eq!(arm.pose(), vec![0.1, 0.2]);
    }

    #[test]
    fn tweening_a_pose_turns_the_shortest_way_round() {
        let from = vec![0.0, 6.1]; // 6.1 rad is just short of a full turn
        let to = vec![1.0, 0.1];

        let half = Armature::tween_pose(&from, &to, 0.5);
        assert!((half[0] - 0.5).abs() < 1e-12);
        // 6.1 -> 0.1 is +0.28 the short way, not -6.0 the long way.
        assert!(half[1] > 6.1, "should carry on forwards, got {}", half[1]);

        let end = Armature::tween_pose(&from, &to, 1.0);
        assert!((wrap_pi(end[1] - 0.1)).abs() < 1e-12, "and arrive at the target");
    }

    #[test]
    fn the_rest_pose_is_where_the_bones_were_drawn() {
        let mut arm = arm();
        arm.bones[1].angle = 1.2;

        let rest = arm.at_rest();
        assert_eq!(rest.bones[1].angle, 0.0);
        assert_eq!(arm.bones[1].angle, 1.2, "the posed armature is untouched");

        arm.set_rest_here();
        assert_eq!(arm.at_rest().bones[1].angle, 1.2, "rest moved to the pose");
    }

    /// The transform skinning applies: at rest it must be the identity, or
    /// binding artwork would shift it the moment it was bound.
    #[test]
    fn the_pose_transform_is_the_identity_at_rest() {
        let arm = arm();
        for i in 0..arm.len() {
            let t = arm.pose_transform(i);
            let p = Point::new(123.0, -45.0);
            assert!(close(t * p, p), "bone {i} moved a point at rest");
        }
    }

    #[test]
    fn the_pose_transform_turns_about_the_bones_head() {
        let mut arm = arm();
        arm.bones[0].angle = FRAC_PI_2;

        let t = arm.pose_transform(0);
        // The head itself does not move.
        assert!(close(t * Point::new(100.0, 100.0), Point::new(100.0, 100.0)));
        // A point at the tip swings a quarter turn about it.
        assert!(close(t * Point::new(150.0, 100.0), Point::new(100.0, 150.0)));
    }

    #[test]
    fn the_nearest_bone_is_found_along_its_length_not_at_its_head() {
        let arm = arm();
        // Beside the middle of the upper arm.
        let (index, distance) = arm.nearest_bone(Point::new(125.0, 110.0)).expect("a bone");
        assert_eq!(index, 0);
        assert!((distance - 10.0).abs() < 1e-9, "got {distance}");

        // Beside the middle of the forearm.
        let (index, _) = arm.nearest_bone(Point::new(170.0, 104.0)).expect("a bone");
        assert_eq!(index, 1);
    }

    #[test]
    fn bounds_cover_every_joint() {
        let arm = arm();
        let bounds = arm.bounds().expect("bounds");
        assert!((bounds.x0 - 100.0).abs() < 1e-9);
        assert!((bounds.x1 - 190.0).abs() < 1e-9);
        assert!(Armature::default().bounds().is_none());
    }

    #[test]
    fn a_subtree_reaches_every_descendant() {
        let mut arm = arm();
        arm.push(Bone::new("hand", Some(1), 10.0, 0.0));
        arm.push(Bone::new("thumb", Some(2), 5.0, 0.5));
        arm.push(Bone::new("other", Some(0), 20.0, 1.0));

        let mut whole = arm.subtree(0);
        whole.sort();
        assert_eq!(whole, vec![0, 1, 2, 3, 4]);

        let mut hand = arm.subtree(2);
        hand.sort();
        assert_eq!(hand, vec![2, 3], "only the hand and its thumb");
    }

    #[test]
    fn wrapping_brings_angles_into_one_turn() {
        assert!((wrap_pi(0.5) - 0.5).abs() < 1e-12);
        assert!((wrap_pi(std::f64::consts::TAU + 0.5) - 0.5).abs() < 1e-12);
        assert!((wrap_pi(-std::f64::consts::TAU - 0.5) + 0.5).abs() < 1e-12);
        assert!(wrap_pi(PI + 0.1) < 0.0, "just past the top wraps negative");
    }
}
