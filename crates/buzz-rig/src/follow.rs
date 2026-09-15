//! Follow-through and overlap for a bone chain.
//!
//! # What it does
//!
//! Given a rig's *primary* animation — the pose the animator keyed, frame by
//! frame — this makes a chosen chain (a ponytail, a tail, a floppy sleeve) *lag*
//! that motion and swing past it, the way anything with weight does. It is the
//! single most tedious thing to animate by hand and it has a physical answer, so
//! the app gives it away.
//!
//! # How
//!
//! Each bone in the chain runs a damped spring ([`buzz_physics`]) whose target is
//! where the primary pose says the bone should point *relative to its parent's
//! actual, already-lagging direction*. Processed parents-first, that one rule
//! produces both effects at once: **follow-through** (a bone lags the pose) and
//! **overlap** (a child lags its parent, so the chain whips down its length).
//!
//! The chain is `subtree(root)` — the chosen bone and everything below it. A
//! bone *above* the chain that the animator moved is not sprung, but its motion
//! still drives the chain, because it is the chain root's parent and so sets the
//! root's target. That is why swaying the body makes the hair follow without the
//! hair itself being keyed.
//!
//! # Deterministic, and it does not touch the document
//!
//! Pure arithmetic on pose vectors: same primary in, same secondary out. Reading
//! those poses off a timeline and writing the result back as keyframes is a
//! separate job (`buzz_act`), exactly as it is for a walk.

use buzz_geom::{Affine, Vec2};
use buzz_physics::{Spring, SpringState};

use crate::{Armature, wrap_pi};

/// Apply spring follow-through to `subtree(root)` across a primary pose
/// sequence, without any whole-object coupling. See [`follow_through_coupled`].
///
/// `topology` supplies the skeleton (lengths, parents, rest); `primary[f]` is
/// the keyed pose at frame `f` (one angle per bone, as [`Armature::pose`]
/// returns). Returns a modified pose per frame, identical to the primary except
/// on the driven chain.
///
/// The returned angles are the spring's raw result; joint-limit clamping is left
/// to whoever writes them back through [`Armature::set_pose`], matching how IK
/// clamps only when it commits.
pub fn follow_through(
    topology: &Armature,
    root: usize,
    spring: Spring,
    primary: &[Vec<f64>],
    fps: f64,
) -> Vec<Vec<f64>> {
    follow_through_coupled(topology, root, spring, primary, &[], 0.0, fps)
}

/// [`follow_through`], also driven by the whole object's motion.
///
/// `object_world[f]` is the object's own transform at frame `f` — its placement
/// on the stage. Two effects come from it: the chain **rotates with the body**,
/// so turning the character swings the hair, and it **trails the body's
/// acceleration**, so a character breaking into a run leaves its hair behind,
/// scaled by `coupling`. An empty slice and `coupling = 0.0` mean neither, which
/// is exactly what [`follow_through`] asks for.
pub fn follow_through_coupled(
    topology: &Armature,
    root: usize,
    spring: Spring,
    primary: &[Vec<f64>],
    object_world: &[Affine],
    coupling: f64,
    fps: f64,
) -> Vec<Vec<f64>> {
    let bone_count = topology.bones.len();
    if primary.is_empty() || root >= bone_count {
        return primary.to_vec();
    }

    let dt = 1.0 / fps.max(1.0);
    // Nearest-first, and because a parent's index is always below its child's,
    // a parent is listed before any child of it — so a single forward pass sees
    // each parent's sprung result already computed this frame.
    let driven = topology.subtree(root);
    let mut is_driven = vec![false; bone_count];
    for &i in &driven {
        is_driven[i] = true;
    }

    // The object's own rotation, and the pseudo-force its acceleration puts on
    // anything with weight (opposite the acceleration). Read off the per-frame
    // transform; an absent transform is a still object.
    let obj_rotation = |f: usize| -> f64 {
        object_world
            .get(f)
            .map_or(0.0, |a| {
                let c = a.as_coeffs();
                c[1].atan2(c[0])
            })
    };
    let obj_position = |f: usize| -> Vec2 {
        object_world.get(f).map_or(Vec2::ZERO, |a| {
            let c = a.as_coeffs();
            Vec2::new(c[4], c[5])
        })
    };
    let obj_accel = |f: usize| -> Vec2 {
        if object_world.len() < 3 {
            return Vec2::ZERO;
        }
        let last = object_world.len() - 1;
        let prev = obj_position(f.saturating_sub(1));
        let here = obj_position(f.min(last));
        let next = obj_position((f + 1).min(last));
        (next - here * 2.0 + prev) / (dt * dt)
    };

    // A scratch skeleton, re-posed to the primary each frame, is how we read the
    // primary world angles (a bone's absolute direction) without disturbing the
    // caller's topology.
    let mut scratch = topology.clone();

    // One spring per driven bone, started at rest at its frame-0 world angle
    // (including the object's rotation) so the chain begins hanging where it was
    // posed — no jolt on frame 0.
    scratch.set_pose(&primary[0]);
    let mut springs = vec![SpringState::settle(0.0); bone_count];
    let rot0 = obj_rotation(0);
    for &i in &driven {
        springs[i] = SpringState::settle(scratch.world_angle(i) + rot0);
    }

    let mut out = Vec::with_capacity(primary.len());
    for (f, pose) in primary.iter().enumerate() {
        scratch.set_pose(pose);
        let body_rot = obj_rotation(f);
        let force = -obj_accel(f);
        let mut actual_world = vec![0.0f64; bone_count];
        let mut result = pose.clone();

        for &i in &driven {
            if i >= pose.len() {
                continue;
            }
            let parent_world = match topology.bones[i].parent {
                // A sprung parent's lagging direction: already integrated above.
                Some(p) if is_driven[p] => actual_world[p],
                // An unsprung ancestor the animator moved: its primary direction
                // drives the chain, turned with the body.
                Some(p) => scratch.world_angle(p) + body_rot,
                // A root bone points in world space, plus the body's rotation.
                None => body_rot,
            };

            // Where the bone would point if it were rigid, plus a lean opposite
            // the body's acceleration: a torque from the pseudo-force across the
            // bone's own direction, so a hanging chain trails a body that speeds
            // up. Bounded so a violent move cannot fling a joint round.
            let mut target = parent_world + pose[i];
            if coupling != 0.0 && force != Vec2::ZERO {
                let dir = scratch.world_angle(i) + body_rot;
                let (dx, dy) = (dir.cos(), dir.sin());
                let torque = dx * force.y - dy * force.x;
                target += (coupling * torque).clamp(-1.0, 1.0);
            }

            springs[i].step_angle(target, spring, dt);
            let world = springs[i].value;
            actual_world[i] = world;
            result[i] = wrap_pi(world - parent_world);
        }
        out.push(result);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bone;
    use buzz_geom::Point;

    /// A straight chain of `n` bones pointing along +x from the origin, each
    /// parented to the one before.
    fn straight_chain(n: usize) -> Armature {
        let mut arm = Armature {
            root: Point::ORIGIN,
            bones: Vec::new(),
        };
        for i in 0..n {
            let parent = if i == 0 { None } else { Some(i - 1) };
            arm.bones.push(Bone::new(format!("b{i}"), parent, 40.0, 0.0));
        }
        arm
    }

    /// A primary animation that holds still, then swings the base bone (bone 0)
    /// by `swing` at `at`, and holds. The chain below is expected to lag.
    fn base_swing(n: usize, frames: usize, at: usize, swing: f64) -> Vec<Vec<f64>> {
        (0..frames)
            .map(|f| {
                let mut pose = vec![0.0; n];
                pose[0] = if f >= at { swing } else { 0.0 };
                pose
            })
            .collect()
    }

    fn world_angle_of(topology: &Armature, pose: &[f64], bone: usize) -> f64 {
        let mut a = topology.clone();
        a.set_pose(pose);
        a.world_angle(bone)
    }

    #[test]
    fn the_chain_lags_the_primary_then_settles() {
        let arm = straight_chain(4);
        let primary = base_swing(4, 60, 5, 0.8);
        let modified = follow_through(&arm, 1, Spring::tail(), &primary, 24.0);

        // Just after the base snaps, the tip bone has barely started to move —
        // it lags well behind where the rigid pose would have put it.
        let tip = 3;
        let primary_tip = world_angle_of(&arm, &primary[7], tip);
        let sprung_tip = world_angle_of(&arm, &modified[7], tip);
        assert!(
            (sprung_tip - primary_tip).abs() > 0.2,
            "the tip did not lag: primary {primary_tip:.3}, sprung {sprung_tip:.3}"
        );
        // The lag is *behind*: nearer the old direction (0.0) than the new one.
        assert!(
            sprung_tip.abs() < primary_tip.abs(),
            "the tip should trail toward its old direction, got {sprung_tip:.3}"
        );

        // Long after, held, it catches up.
        let primary_end = world_angle_of(&arm, primary.last().unwrap(), tip);
        let sprung_end = world_angle_of(&arm, modified.last().unwrap(), tip);
        assert!(
            (sprung_end - primary_end).abs() < 0.02,
            "the chain never settled: primary {primary_end:.3}, sprung {sprung_end:.3}"
        );
    }

    #[test]
    fn bones_outside_the_chain_are_untouched() {
        let arm = straight_chain(4);
        let primary = base_swing(4, 40, 5, 0.8);
        let modified = follow_through(&arm, 1, Spring::tail(), &primary, 24.0);
        // Bone 0 is above the chosen root (1), so its keyed angle must survive
        // exactly.
        for f in 0..primary.len() {
            assert_eq!(modified[f][0], primary[f][0], "bone 0 changed at frame {f}");
        }
    }

    #[test]
    fn overlap_runs_down_the_chain() {
        // The tip lags more than the bone just below the root: the whip grows
        // along the chain rather than every bone moving together.
        let arm = straight_chain(4);
        let primary = base_swing(4, 60, 5, 0.8);
        let modified = follow_through(&arm, 1, Spring::tail(), &primary, 24.0);

        let f = 8;
        let primary_1 = world_angle_of(&arm, &primary[f], 1);
        let sprung_1 = world_angle_of(&arm, &modified[f], 1);
        let primary_3 = world_angle_of(&arm, &primary[f], 3);
        let sprung_3 = world_angle_of(&arm, &modified[f], 3);
        assert!(
            (sprung_3 - primary_3).abs() > (sprung_1 - primary_1).abs(),
            "the tip should lag more than the root of the chain"
        );
    }

    #[test]
    fn a_still_primary_stays_still() {
        // Nothing moves, so nothing should swing — the baker relies on this to
        // leave a held pose alone.
        let arm = straight_chain(3);
        let primary: Vec<Vec<f64>> = (0..20).map(|_| vec![0.3, 0.0, 0.0]).collect();
        let modified = follow_through(&arm, 0, Spring::hair(), &primary, 24.0);
        for (f, pose) in modified.iter().enumerate() {
            for (b, angle) in pose.iter().enumerate() {
                assert!(angle.abs() < 1e-6 || b == 0, "frame {f} bone {b} drifted to {angle}");
            }
        }
    }

    #[test]
    fn a_bad_root_returns_the_primary_unchanged() {
        let arm = straight_chain(3);
        let primary = base_swing(3, 10, 2, 0.5);
        let modified = follow_through(&arm, 9, Spring::hair(), &primary, 24.0);
        assert_eq!(modified, primary);
    }

    #[test]
    fn rotating_the_body_swings_the_chain_even_with_no_bone_motion() {
        // The bones never move relative to each other (primary is all rest), but
        // the whole object turns. The chain should still lag that turn — which a
        // pose-only solver, blind to the object transform, could never do.
        let arm = straight_chain(3);
        let primary: Vec<Vec<f64>> = (0..40).map(|_| vec![0.0; 3]).collect();
        let object: Vec<Affine> = (0..40)
            .map(|f| Affine::rotate(if f >= 10 { 1.0 } else { f as f64 / 10.0 }))
            .collect();

        let modified = follow_through_coupled(&arm, 0, Spring::tail(), &primary, &object, 0.0, 24.0);
        // Mid-turn, the root lags the body's rotation: its angle *relative to the
        // turning body* is behind, so its own local angle dips below rest.
        assert!(
            modified[8][0] < -0.05,
            "the chain did not lag the body's rotation: {}",
            modified[8][0]
        );
        // Settled, it is back on rest relative to the (now still) body.
        assert!(
            modified.last().unwrap()[0].abs() < 0.02,
            "the chain never caught up with the body"
        );
    }

    #[test]
    fn a_chain_trails_the_body_it_is_dragged_behind() {
        // A chain hanging straight down (angle +y), while the object accelerates
        // along +x and then along -x. It should lean back the opposite way each
        // time — hair streaming behind a runner.
        let arm = straight_chain(3);
        let down = std::f64::consts::FRAC_PI_2;
        let primary: Vec<Vec<f64>> = (0..40).map(|_| vec![down, 0.0, 0.0]).collect();

        let accel_along = |sign: f64| -> Vec<Affine> {
            (0..40)
                .map(|f| {
                    let t = f as f64;
                    Affine::translate((sign * 0.5 * 0.01 * t * t, 0.0))
                })
                .collect()
        };

        let forward = follow_through_coupled(
            &arm,
            0,
            Spring::stiff(),
            &primary,
            &accel_along(1.0),
            0.05,
            24.0,
        );
        let backward = follow_through_coupled(
            &arm,
            0,
            Spring::stiff(),
            &primary,
            &accel_along(-1.0),
            0.05,
            24.0,
        );

        assert!(
            forward[8][0] > down + 0.05,
            "accelerating one way did not lean the chain: {}",
            forward[8][0]
        );
        assert!(
            backward[8][0] < down - 0.05,
            "accelerating the other way did not lean it back: {}",
            backward[8][0]
        );
    }
}
