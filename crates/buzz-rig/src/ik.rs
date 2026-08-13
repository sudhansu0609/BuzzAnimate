//! Inverse kinematics: dragging a hand and having the arm follow.
//!
//! # FABRIK, then angles
//!
//! The solver is FABRIK — Forward And Backward Reaching Inverse Kinematics.
//! It works on **joint positions** rather than angles: reach backwards from
//! the target to the base keeping every bone its own length, then forwards
//! from the base again. Two passes, no Jacobian, no matrix inversion, and it
//! converges in a handful of iterations for the chain lengths a character rig
//! actually has.
//!
//! Joint limits are then imposed by **converting the reached positions back to
//! angles, clamping, and running forward kinematics again**. Constraining
//! inside FABRIK's own backward pass is the textbook variant and it is only
//! approximate — it moves a joint to a legal place, and the *next* pass moves
//! it out again. Reading the angles out and clamping them is exact, and
//! forward kinematics then guarantees every bone keeps its length, which is
//! the property a skeleton cannot be allowed to lose. A rig that stretches
//! looks broken in a way that a rig which merely fails to reach does not.
//!
//! # What a pin means
//!
//! A pinned joint does not move, so a solve stops there: the chain runs from
//! the nearest pinned ancestor (or the root) down to the bone being dragged.
//! That is Animate's behaviour and it is what lets an animator hold a foot on
//! the ground while moving the hips.

use buzz_geom::{Point, Vec2};

use crate::{Armature, wrap_pi};

/// How hard to work at a solve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IkOptions {
    /// Give up after this many passes.
    pub max_iterations: usize,
    /// Close enough, in document units.
    pub tolerance: f64,
    /// Let the whole armature move when the chain reaches its root.
    ///
    /// Off by default, which is Animate: dragging a hand should not slide the
    /// character across the stage.
    pub allow_root_translation: bool,
}

impl Default for IkOptions {
    fn default() -> Self {
        Self {
            // Ten passes settles a five-bone chain to well inside a pixel.
            // The cost of the cap is a slightly short reach for one frame of a
            // drag, which is invisible; the cost of no cap is a hang on a
            // target that cannot be reached at all.
            max_iterations: 10,
            tolerance: 0.01,
            allow_root_translation: false,
        }
    }
}

/// What a solve achieved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IkOutcome {
    /// How far the tip ended up from the target.
    pub distance: f64,
    pub iterations: usize,
    /// Did it get within tolerance?
    pub reached: bool,
    /// Bones the solve was free to move.
    pub chain_length: usize,
}

/// Drag bone `index`'s tip towards `target`, moving the chain above it.
///
/// The armature is posed in place. Bones below `index` keep their own angles
/// and are carried along, which is what makes a hand stay attached to a wrist.
pub fn solve_to(
    armature: &mut Armature,
    index: usize,
    target: Point,
    options: &IkOptions,
) -> IkOutcome {
    let chain = chain_to(armature, index);
    if chain.is_empty() {
        return IkOutcome {
            distance: f64::INFINITY,
            iterations: 0,
            reached: false,
            chain_length: 0,
        };
    }

    // Positions: the head of every bone in the chain, then the final tip.
    let joints = armature.joints();
    let base = joints[chain[0]].0;
    let mut points: Vec<Point> = chain.iter().map(|i| joints[*i].0).collect();
    points.push(joints[*chain.last().expect("non-empty")].1);

    let lengths: Vec<f64> = chain.iter().map(|i| armature.bones[*i].length).collect();
    let reach: f64 = lengths.iter().sum();

    let mut iterations = 0;
    for _ in 0..options.max_iterations {
        iterations += 1;

        // Out of reach: there is nothing to iterate towards, so lay the chain
        // straight at the target and stop. Iterating would inch towards a
        // point it can never touch, burning the whole budget every frame of a
        // drag that has simply gone too far.
        if (target - base).hypot() > reach + options.tolerance {
            straighten_towards(&mut points, base, target, &lengths);
            apply_positions(armature, &chain, &points);
            let distance = (armature.tip(index) - target).hypot();
            return IkOutcome {
                distance,
                iterations,
                reached: false,
                chain_length: chain.len(),
            };
        }

        // Backward: from the target down to the base.
        let last = points.len() - 1;
        points[last] = target;
        for i in (0..last).rev() {
            points[i] = towards(points[i + 1], points[i], lengths[i]);
        }

        // Forward: put the base back where it belongs and rebuild.
        points[0] = if options.allow_root_translation {
            points[0]
        } else {
            base
        };
        for i in 0..last {
            points[i + 1] = towards(points[i], points[i + 1], lengths[i]);
        }

        // Positions to angles, clamped, then forward kinematics — which is
        // what actually enforces the bone lengths.
        apply_positions(armature, &chain, &points);

        let distance = (armature.tip(index) - target).hypot();
        if distance <= options.tolerance {
            return IkOutcome {
                distance,
                iterations,
                reached: true,
                chain_length: chain.len(),
            };
        }

        // Clamping may have moved joints, so the next pass starts from where
        // the armature really is rather than from where FABRIK wished it were.
        let joints = armature.joints();
        for (slot, bone) in chain.iter().enumerate() {
            points[slot] = joints[*bone].0;
        }
        points[last] = armature.tip(index);
    }

    let distance = (armature.tip(index) - target).hypot();
    IkOutcome {
        distance,
        iterations,
        reached: distance <= options.tolerance,
        chain_length: chain.len(),
    }
}

/// The bones a solve for `index` is allowed to move, base first.
///
/// Walks up from `index` and stops **at** a pinned bone: a pin holds its own
/// joint still, so the pinned bone itself may still turn about it, but nothing
/// above it moves.
fn chain_to(armature: &Armature, index: usize) -> Vec<usize> {
    if index >= armature.bones.len() {
        return Vec::new();
    }

    let mut chain = vec![index];
    let mut current = index;
    let mut guard = 0;

    while let Some(parent) = armature.bones[current].parent {
        if armature.bones[current].pinned {
            break;
        }
        if parent >= armature.bones.len() {
            break;
        }
        chain.push(parent);
        current = parent;

        guard += 1;
        if guard > armature.bones.len() {
            break;
        }
    }
    chain.reverse();
    chain
}

/// A point `length` away from `from`, in the direction of `towards`.
fn towards(from: Point, toward: Point, length: f64) -> Point {
    let delta = toward - from;
    let distance = delta.hypot();
    if distance <= f64::EPSILON {
        // Degenerate: any direction will do, and picking one keeps the solve
        // moving rather than producing NaN that spreads through the pose.
        return from + Vec2::new(length, 0.0);
    }
    from + delta * (length / distance)
}

/// Lay the chain out straight from `base` towards an unreachable target.
fn straighten_towards(points: &mut [Point], base: Point, target: Point, lengths: &[f64]) {
    points[0] = base;
    for i in 0..lengths.len() {
        points[i + 1] = towards(points[i], target, lengths[i]);
    }
}

/// Read joint positions back into bone angles, honouring joint limits.
fn apply_positions(armature: &mut Armature, chain: &[usize], points: &[Point]) {
    for (slot, &bone_index) in chain.iter().enumerate() {
        let direction = points[slot + 1] - points[slot];
        if direction.hypot() <= f64::EPSILON {
            continue;
        }
        let world = direction.y.atan2(direction.x);

        // Relative to the parent, which may itself have just moved — so it is
        // read from the armature as it now is, after the earlier bones in this
        // loop have been written.
        let parent_angle = match armature.bones[bone_index].parent {
            Some(parent) => armature.world_angle(parent),
            None => 0.0,
        };
        let relative = wrap_pi(world - parent_angle);
        let bone = &mut armature.bones[bone_index];
        bone.angle = bone.constrain(relative);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bone, JointLimits};
    use std::f64::consts::{FRAC_PI_2, PI};

    /// A two-bone arm reaching right: upper 50, forearm 40, from (100, 100).
    fn arm() -> Armature {
        let mut armature = Armature::new(Point::new(100.0, 100.0));
        armature.push(Bone::new("upper", None, 50.0, 0.0));
        armature.push(Bone::new("fore", Some(0), 40.0, 0.0));
        armature
    }

    fn solve(armature: &mut Armature, index: usize, target: Point) -> IkOutcome {
        solve_to(armature, index, target, &IkOptions::default())
    }

    #[test]
    fn a_reachable_target_is_reached() {
        let mut arm = arm();
        let target = Point::new(140.0, 140.0);
        let outcome = solve(&mut arm, 1, target);

        assert!(outcome.reached, "should reach: {outcome:?}");
        assert!((arm.tip(1) - target).hypot() < 0.01);
    }

    /// The property that must never be given up: bones do not stretch.
    #[test]
    fn bone_lengths_survive_every_solve() {
        let mut arm = arm();
        for target in [
            Point::new(140.0, 140.0),
            Point::new(60.0, 60.0),
            Point::new(100.0, 100.0),
            Point::new(1000.0, -1000.0),
        ] {
            solve(&mut arm, 1, target);
            assert!((arm.tip(0) - arm.head(0)).hypot() - 50.0 < 1e-9, "upper stretched");
            assert!((arm.tip(1) - arm.head(1)).hypot() - 40.0 < 1e-9, "forearm stretched");
            assert!(arm.pose().iter().all(|a| a.is_finite()), "NaN in the pose");
        }
    }

    /// The base joint stays put — dragging a hand must not slide the whole
    /// character across the stage.
    #[test]
    fn the_root_does_not_move() {
        let mut arm = arm();
        solve(&mut arm, 1, Point::new(200.0, 300.0));
        assert!((arm.head(0) - Point::new(100.0, 100.0)).hypot() < 1e-12);
    }

    #[test]
    fn an_unreachable_target_lays_the_chain_out_straight_towards_it() {
        let mut arm = arm();
        let target = Point::new(100.0, 1000.0);
        let outcome = solve(&mut arm, 1, target);

        assert!(!outcome.reached, "90 units cannot reach 900 away");
        // Straight down, at full extension.
        assert!((arm.tip(1) - Point::new(100.0, 190.0)).hypot() < 1e-6, "{:?}", arm.tip(1));
        assert!(
            outcome.iterations == 1,
            "an unreachable target should not burn the iteration budget"
        );
    }

    /// A limit that forbids the natural solution must hold, even at the cost
    /// of not reaching.
    #[test]
    fn a_joint_limit_is_never_exceeded() {
        let mut arm = arm();
        arm.bones[1].limits = Some(JointLimits::new(0.0, 0.2));

        solve(&mut arm, 1, Point::new(100.0, 40.0));

        let elbow = arm.bones[1].angle;
        assert!(
            (0.0..=0.2 + 1e-9).contains(&elbow),
            "the elbow bent to {elbow}, outside its limits"
        );
    }

    /// With both joints limited to a narrow range the target is simply out of
    /// the rig's reach — and the solver must say so rather than cheat.
    #[test]
    fn limits_that_make_a_target_impossible_report_failure() {
        let mut arm = arm();
        arm.bones[0].limits = Some(JointLimits::new(-0.05, 0.05));
        arm.bones[1].limits = Some(JointLimits::new(-0.05, 0.05));

        let outcome = solve(&mut arm, 1, Point::new(100.0, 190.0));
        assert!(!outcome.reached, "a locked arm cannot reach behind itself");
        assert!(arm.bones[0].angle.abs() <= 0.05 + 1e-9);
        assert!(arm.bones[1].angle.abs() <= 0.05 + 1e-9);
    }

    #[test]
    fn a_pin_stops_the_solve_from_climbing_past_it() {
        let mut arm = arm();
        arm.push(Bone::new("hand", Some(1), 20.0, 0.0));
        arm.bones[1].pinned = true;

        let shoulder_before = arm.bones[0].angle;
        let elbow_position = arm.head(1);

        let outcome = solve(&mut arm, 2, Point::new(160.0, 160.0));

        assert_eq!(outcome.chain_length, 2, "hand and forearm only");
        assert_eq!(arm.bones[0].angle, shoulder_before, "the shoulder is held");
        assert!(
            (arm.head(1) - elbow_position).hypot() < 1e-12,
            "the pinned joint moved"
        );
    }

    #[test]
    fn solving_the_root_bone_alone_just_turns_it() {
        let mut arm = arm();
        let outcome = solve(&mut arm, 0, Point::new(100.0, 150.0));

        assert_eq!(outcome.chain_length, 1);
        assert!((arm.bones[0].angle - FRAC_PI_2).abs() < 1e-6);
        assert!((arm.head(0) - Point::new(100.0, 100.0)).hypot() < 1e-12);
    }

    /// Bones below the one being dragged keep their own angles and are carried
    /// along — a hand stays attached to the wrist at the angle it was posed.
    #[test]
    fn children_below_the_dragged_bone_are_carried_along() {
        let mut arm = arm();
        arm.push(Bone::new("hand", Some(1), 20.0, 0.6));

        solve(&mut arm, 1, Point::new(120.0, 160.0));
        assert!((arm.bones[2].angle - 0.6).abs() < 1e-12, "the wrist angle changed");
    }

    #[test]
    fn dragging_to_where_the_tip_already_is_changes_nothing_much() {
        let mut arm = arm();
        let tip = arm.tip(1);
        let before = arm.pose();
        let outcome = solve(&mut arm, 1, tip);

        assert!(outcome.reached);
        for (a, b) in before.iter().zip(arm.pose()) {
            assert!((a - b).abs() < 1e-6, "the pose drifted: {a} vs {b}");
        }
    }

    #[test]
    fn a_target_on_top_of_the_root_does_not_produce_nan() {
        let mut arm = arm();
        let root = arm.root;
        solve(&mut arm, 1, root);
        assert!(arm.pose().iter().all(|a| a.is_finite()));
        assert!(arm.joints().iter().all(|(h, t)| h.is_finite() && t.is_finite()));
    }

    #[test]
    fn a_bone_that_does_not_exist_is_refused_rather_than_panicking() {
        let mut arm = arm();
        let outcome = solve(&mut arm, 99, Point::new(0.0, 0.0));
        assert_eq!(outcome.chain_length, 0);
        assert!(!outcome.reached);
    }

    /// A long chain still settles, and quickly: this is the case that decides
    /// whether dragging a tail or a spine feels alive or gluey.
    #[test]
    fn a_long_chain_settles_within_the_iteration_budget() {
        let mut armature = Armature::new(Point::ZERO);
        armature.push(Bone::new("b0", None, 20.0, 0.0));
        for i in 1..12 {
            armature.push(Bone::new(format!("b{i}"), Some(i - 1), 20.0, 0.0));
        }

        let target = Point::new(80.0, 90.0);
        let outcome = solve(&mut armature, 11, target);

        assert!(outcome.reached, "{outcome:?}");
        assert!(outcome.iterations <= 10);
        for i in 0..armature.len() {
            let length = (armature.tip(i) - armature.head(i)).hypot();
            assert!((length - 20.0).abs() < 1e-6, "bone {i} is {length} long");
        }
    }

    /// Rotation is the *shortest way round*, so a small drag never spins the
    /// bone the long way — the same rule the camera and tweens follow.
    #[test]
    fn a_small_drag_produces_a_small_rotation() {
        let mut arm = arm();
        arm.bones[0].angle = PI - 0.05; // pointing nearly left
        arm.bones[1].angle = 0.0;
        let before = arm.bones[0].angle;

        // A target a hair the other side of the wrap point.
        let target = arm.head(0) + Vec2::new(-90.0, -2.0);
        solve(&mut arm, 1, target);

        let moved = wrap_pi(arm.bones[0].angle - before).abs();
        assert!(moved < 0.5, "the shoulder swung {moved} rad for a tiny drag");
    }

    /// The exit criterion for CP-7.2: fifty armatures solved in parallel,
    /// inside one frame of a 24 fps document (41 ms).
    #[test]
    fn fifty_armatures_solve_in_parallel_within_a_frame() {
        use rayon::prelude::*;

        let make = |seed: usize| {
            let mut armature = Armature::new(Point::new(seed as f64, 0.0));
            armature.push(Bone::new("b0", None, 30.0, 0.1));
            for i in 1..6 {
                armature.push(Bone::new(format!("b{i}"), Some(i - 1), 25.0, 0.2));
            }
            armature
        };
        let mut rigs: Vec<Armature> = (0..50).map(make).collect();

        let started = std::time::Instant::now();
        let outcomes: Vec<IkOutcome> = rigs
            .par_iter_mut()
            .enumerate()
            .map(|(i, rig)| {
                let target = Point::new(i as f64 + 40.0, 60.0);
                solve_to(rig, 5, target, &IkOptions::default())
            })
            .collect();
        let elapsed = started.elapsed();

        assert_eq!(outcomes.len(), 50);
        assert!(
            outcomes.iter().all(|o| o.reached),
            "every rig should reach its target"
        );
        // Generous against a debug build; the point is that it is nowhere near
        // a frame, not to measure the machine.
        assert!(
            elapsed < std::time::Duration::from_millis(41),
            "fifty rigs took {elapsed:?}, which is more than one frame at 24 fps"
        );
    }
}
