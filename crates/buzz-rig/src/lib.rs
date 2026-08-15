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

    /// **Remove a bone, and give its children to its parent.**
    ///
    /// Building a rig was additive only: one bone in the wrong place meant
    /// starting the skeleton again, which is why nobody rigged the second
    /// character. Deleting has to keep the rest of the tree standing, so a
    /// bone's children are adopted by its parent rather than deleted with it —
    /// removing a shoulder should not silently take the hand.
    ///
    /// **Every index after the removed one shifts down by one**, and parent
    /// indices are rewritten to match. That is the whole difficulty: an
    /// armature stores parents as positions, so a removal that did not renumber
    /// would leave bones pointing at whichever bone slid into the gap.
    ///
    /// Returns `false` if there is no such bone.
    pub fn remove_bone(&mut self, index: usize) -> bool {
        if index >= self.bones.len() {
            return false;
        }
        let orphan_parent = self.bones[index].parent;

        // The removed bone's own turn is inherited by its children, so a limb
        // does not snap to a new direction when a joint above it goes.
        let inherited = self.bones[index].angle;
        let inherited_rest = self.bones[index].rest_angle;
        for bone in self.bones.iter_mut() {
            if bone.parent == Some(index) {
                bone.parent = orphan_parent;
                bone.angle += inherited;
                bone.rest_angle += inherited_rest;
            }
        }

        self.bones.remove(index);

        // Renumber: anything that pointed past the hole moves down with it.
        for bone in self.bones.iter_mut() {
            if let Some(parent) = bone.parent {
                bone.parent = match parent.cmp(&index) {
                    std::cmp::Ordering::Greater => Some(parent - 1),
                    // A parent that *was* the removed bone has already been
                    // rewritten above; anything still equal is a root now.
                    std::cmp::Ordering::Equal => None,
                    std::cmp::Ordering::Less => Some(parent),
                };
            }
        }
        self.mend_order();
        true
    }

    /// **Point a bone at a different parent.**
    ///
    /// Refused when it would make a cycle — a bone cannot become its own
    /// ancestor — and when the new parent does not exist. `None` makes the
    /// bone a root.
    ///
    /// Returns `false` if the change was refused, so a caller can say why
    /// rather than appearing to do nothing.
    pub fn reparent_bone(&mut self, index: usize, parent: Option<usize>) -> bool {
        if index >= self.bones.len() {
            return false;
        }
        if let Some(p) = parent
            && (p >= self.bones.len() || p == index || self.is_descendant(p, index))
        {
            return false;
        }
        self.bones[index].parent = parent;
        self.mend_order();
        true
    }

    /// Is `candidate` somewhere below `ancestor` in the tree?
    fn is_descendant(&self, candidate: usize, ancestor: usize) -> bool {
        let mut current = Some(candidate);
        // Bounded by the bone count: a malformed tree must not spin here.
        for _ in 0..=self.bones.len() {
            match current {
                Some(i) if i == ancestor => return true,
                Some(i) => current = self.bones.get(i).and_then(|b| b.parent),
                None => return false,
            }
        }
        false
    }

    /// Restore the parents-first invariant after an edit.
    ///
    /// Every walk here is a single forward pass, which is only sound while a
    /// bone's parent sits before it. Reparenting can break that — attaching an
    /// early bone to a late one — so the bones are reordered and every index
    /// rewritten to match.
    fn mend_order(&mut self) {
        if self.is_ordered() {
            return;
        }
        // Depth-first from the roots: a parent is always emitted before its
        // children, which is exactly the invariant.
        let mut order: Vec<usize> = Vec::with_capacity(self.bones.len());
        let mut placed = vec![false; self.bones.len()];
        let mut progress = true;
        while progress {
            progress = false;
            for i in 0..self.bones.len() {
                if placed[i] {
                    continue;
                }
                let ready = match self.bones[i].parent {
                    None => true,
                    Some(p) => placed.get(p).copied().unwrap_or(false),
                };
                if ready {
                    placed[i] = true;
                    order.push(i);
                    progress = true;
                }
            }
        }
        // Anything left is part of a cycle that should not exist; making it a
        // root is better than dropping it.
        for (i, done) in placed.iter().enumerate() {
            if !done {
                order.push(i);
            }
        }

        let mut position = vec![0usize; self.bones.len()];
        for (new, &old) in order.iter().enumerate() {
            position[old] = new;
        }
        let mut moved: Vec<Bone> = order.iter().map(|&i| self.bones[i].clone()).collect();
        for bone in &mut moved {
            bone.parent = bone.parent.and_then(|p| position.get(p).copied());
            if bone.parent == Some(usize::MAX) {
                bone.parent = None;
            }
        }
        // A parent that still lands after its child is a cycle; cut it.
        for (i, bone) in moved.iter_mut().enumerate() {
            if bone.parent.is_some_and(|p| p >= i) {
                bone.parent = None;
            }
        }
        self.bones = moved;
    }

    fn is_ordered(&self) -> bool {
        self.bones
            .iter()
            .enumerate()
            .all(|(i, b)| b.parent.is_none_or(|p| p < i))
    }

    /// **The same pose, facing the other way.**
    ///
    /// Every joint angle is reflected about the vertical, which turns a pose
    /// reaching right into the same pose reaching left. It halves the work of
    /// building a pose set — and a set is what makes a pose library worth
    /// having, so this is not a convenience so much as the other half of the
    /// feature.
    ///
    /// # What it does not do
    ///
    /// It does not swap a left arm's bones with a right arm's. Doing that
    /// needs to know which bones are a pair, and nothing here records that:
    /// Animate infers it from names, which is a guess that is wrong quietly.
    /// Reflecting the angles is the part that is always right, and for a rig
    /// drawn symmetrically it is the whole answer.
    pub fn mirrored_pose(&self) -> Vec<f64> {
        // A bone's angle is relative to its parent, and reflecting a chain
        // about a line negates every relative turn in it.
        self.bones
            .iter()
            .map(|bone| {
                // Reflect about the vertical: an angle measured from +x goes
                // to pi - angle. For a child, that is simply the negation,
                // because its parent has already been reflected.
                let mirrored = if bone.parent.is_some() {
                    -bone.angle
                } else {
                    std::f64::consts::PI - bone.angle
                };
                bone.constrain(wrap_pi(mirrored))
            })
            .collect()
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
        armature.push_dragged(
            "fore",
            Some(0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 60.0),
        );

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

        assert_eq!(
            arm.bones[0].angle, 1.0,
            "an unlimited joint takes the value"
        );
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
        assert!(
            (wrap_pi(end[1] - 0.1)).abs() < 1e-12,
            "and arrive at the target"
        );
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
        assert!(close(
            t * Point::new(100.0, 100.0),
            Point::new(100.0, 100.0)
        ));
        // A point at the tip swings a quarter turn about it.
        assert!(close(
            t * Point::new(150.0, 100.0),
            Point::new(100.0, 150.0)
        ));
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

#[cfg(test)]
mod mirror_tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    fn arm() -> Armature {
        let mut armature = Armature::new(Point::new(0.0, 0.0));
        armature.push(Bone::new("upper", None, 50.0, 0.0));
        armature.push(Bone::new("fore", Some(0), 40.0, 0.0));
        armature
    }

    /// **The mirrored pose reaches the other way.** An arm out to the right
    /// ends up out to the left, the same distance from the root.
    #[test]
    fn mirroring_reaches_the_other_side() {
        let mut a = arm();
        let tip_right = a.tip(1);
        assert!(tip_right.x > 0.0, "the test arm should start pointing right");

        let flipped = a.mirrored_pose();
        a.set_pose(&flipped);
        let tip_left = a.tip(1);

        assert!(tip_left.x < 0.0, "it should now point left, not {tip_left:?}");
        assert!(
            (tip_left.x + tip_right.x).abs() < 1e-9,
            "and the same distance out: {tip_right:?} vs {tip_left:?}"
        );
        assert!((tip_left.y - tip_right.y).abs() < 1e-9, "height is unchanged");
    }

    /// Mirroring twice is the pose you started with.
    #[test]
    fn mirroring_twice_is_where_it_started() {
        let mut a = arm();
        a.set_pose(&[FRAC_PI_4, -FRAC_PI_2]);
        let start = a.tip(1);

        let once = a.mirrored_pose();
        a.set_pose(&once);
        let twice = a.mirrored_pose();
        a.set_pose(&twice);

        let back = a.tip(1);
        assert!(
            (back - start).hypot() < 1e-9,
            "{start:?} became {back:?} after two mirrors"
        );
    }

    /// A bent elbow stays bent by the same amount — the shape of the pose
    /// survives, only its handedness changes.
    #[test]
    fn mirroring_keeps_the_bend() {
        let mut a = arm();
        a.set_pose(&[0.3, 0.9]);
        let bend = a.bones[1].angle.abs();

        let flipped = a.mirrored_pose();
        a.set_pose(&flipped);
        assert!((a.bones[1].angle.abs() - bend).abs() < 1e-9);
    }

    /// Joint limits still hold: a mirrored angle outside them is clamped, not
    /// smuggled past.
    #[test]
    fn mirroring_respects_joint_limits() {
        let mut a = arm();
        a.bones[1].limits = Some(JointLimits::new(0.0, PI / 2.0));
        a.set_pose(&[0.0, 1.0]);

        let flipped = a.mirrored_pose();
        a.set_pose(&flipped);
        let angle = a.bones[1].angle;
        assert!(
            (0.0..=PI / 2.0).contains(&angle),
            "{angle} escaped its limits"
        );
    }

    /// An empty rig has an empty pose, and mirroring it is not a panic.
    #[test]
    fn mirroring_an_empty_rig_is_empty() {
        let a = Armature::new(Point::ZERO);
        assert!(a.mirrored_pose().is_empty());
    }
}

#[cfg(test)]
mod skeleton_edit_tests {
    use super::*;

    /// shoulder → elbow → wrist, a chain of three.
    fn chain() -> Armature {
        let mut a = Armature::new(Point::ZERO);
        a.push(Bone::new("shoulder", None, 50.0, 0.0));
        a.push(Bone::new("elbow", Some(0), 40.0, 0.2));
        a.push(Bone::new("wrist", Some(1), 20.0, 0.1));
        a
    }

    /// **Removing a joint must not take the limb below it.** The children are
    /// adopted by the removed bone's parent.
    #[test]
    fn removing_a_bone_gives_its_children_to_its_parent() {
        let mut a = chain();
        assert!(a.remove_bone(1), "the elbow should have gone");

        assert_eq!(a.len(), 2);
        assert_eq!(a.bones[0].name, "shoulder");
        assert_eq!(a.bones[1].name, "wrist", "the hand went with the elbow");
        assert_eq!(
            a.bones[1].parent,
            Some(0),
            "the wrist should now hang off the shoulder"
        );
    }

    /// **The limb does not snap to a new direction.** The removed joint's turn
    /// is inherited by what hung off it, so the hand still points where it
    /// pointed.
    ///
    /// Its *position* does move, and must: the chain is genuinely shorter by
    /// the length of the bone that went. Direction is the invariant here, not
    /// place.
    #[test]
    fn removing_a_bone_keeps_the_limb_pointing_where_it_was() {
        let mut a = chain();
        let before = a.world_angle(2);
        a.remove_bone(1);
        let after = a.world_angle(1);
        assert!(
            (before - after).abs() < 1e-9,
            "the hand turned from {before} to {after}"
        );
    }

    /// Removing a root leaves what was below it standing.
    #[test]
    fn removing_the_root_leaves_a_root_behind() {
        let mut a = chain();
        a.remove_bone(0);
        assert_eq!(a.len(), 2);
        assert_eq!(a.bones[0].name, "elbow");
        assert_eq!(a.bones[0].parent, None, "the elbow should be a root now");
        assert_eq!(a.bones[1].parent, Some(0));
    }

    /// Every parent index still points at the right bone after the renumber —
    /// the whole difficulty of removing from a list addressed by position.
    #[test]
    fn removing_a_bone_renumbers_every_parent() {
        let mut a = Armature::new(Point::ZERO);
        a.push(Bone::new("a", None, 10.0, 0.0));
        a.push(Bone::new("b", Some(0), 10.0, 0.0));
        a.push(Bone::new("c", Some(0), 10.0, 0.0));
        a.push(Bone::new("d", Some(2), 10.0, 0.0));

        a.remove_bone(1); // b, which has no children

        let names: Vec<&str> = a.bones.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["a", "c", "d"]);
        assert_eq!(a.bones[1].parent, Some(0), "c still hangs off a");
        assert_eq!(a.bones[2].parent, Some(1), "d still hangs off c");
    }

    #[test]
    fn removing_a_bone_that_is_not_there_does_nothing() {
        let mut a = chain();
        assert!(!a.remove_bone(9));
        assert_eq!(a.len(), 3);
    }

    /// Reparenting moves a bone onto another, and the tree stays walkable.
    #[test]
    fn reparenting_moves_a_bone_onto_another() {
        let mut a = chain();
        assert!(a.reparent_bone(2, Some(0)), "wrist onto shoulder");

        let wrist = a.bones.iter().position(|b| b.name == "wrist").unwrap();
        assert_eq!(a.bones[wrist].parent, Some(0));
        // Parents-first still holds, which is what every walk relies on.
        for (i, bone) in a.bones.iter().enumerate() {
            assert!(bone.parent.is_none_or(|p| p < i), "bone {i} points forward");
        }
    }

    /// **A bone cannot become its own ancestor.** A cycle in a skeleton is an
    /// infinite loop in the solver, so this is refused rather than clamped.
    #[test]
    fn reparenting_refuses_to_make_a_cycle() {
        let mut a = chain();
        assert!(!a.reparent_bone(0, Some(2)), "shoulder onto its own wrist");
        assert!(!a.reparent_bone(1, Some(1)), "onto itself");
        assert_eq!(a.bones[0].parent, None, "the tree should be untouched");
        assert_eq!(a.bones[1].parent, Some(0));
    }

    #[test]
    fn reparenting_to_nothing_makes_a_root() {
        let mut a = chain();
        assert!(a.reparent_bone(1, None));
        let elbow = a.bones.iter().position(|b| b.name == "elbow").unwrap();
        assert_eq!(a.bones[elbow].parent, None);
    }

    /// Reparenting an early bone onto a later one reorders the list so that
    /// parents still come first — and every other index survives it.
    #[test]
    fn reparenting_backwards_reorders_and_keeps_the_tree() {
        let mut a = Armature::new(Point::ZERO);
        a.push(Bone::new("a", None, 10.0, 0.0));
        a.push(Bone::new("b", None, 10.0, 0.0));
        a.push(Bone::new("c", Some(1), 10.0, 0.0));

        assert!(a.reparent_bone(0, Some(2)), "a onto c");

        for (i, bone) in a.bones.iter().enumerate() {
            assert!(bone.parent.is_none_or(|p| p < i), "bone {i} points forward");
        }
        let name_at = |i: usize| a.bones[i].name.as_str();
        let a_at = a.bones.iter().position(|x| x.name == "a").unwrap();
        let c_at = a.bones.iter().position(|x| x.name == "c").unwrap();
        assert!(c_at < a_at, "c must come before a now");
        assert_eq!(a.bones[a_at].parent, Some(c_at));
        assert_eq!(name_at(0), "b");
    }
}
