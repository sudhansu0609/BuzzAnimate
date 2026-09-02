//! Damped-spring integration for secondary motion.
//!
//! # What this is
//!
//! One thing: a critically-dampable spring, integrated forward in time. Give it
//! a *target* each frame and it produces a value that chases the target and
//! overshoots it — the lag and the settle that read as weight. That is the whole
//! of the arithmetic behind follow-through and overlap; the rig knowledge (which
//! bone drives which, what the joint limits are) lives in `buzz-rig`, and the
//! reading and writing of keyframes lives in `buzz-act`. Keeping the integrator
//! alone in here is what lets it be tested without a skeleton and reused later
//! for a wiggling camera or a squashing bounce.
//!
//! # Why a spring, and not a filter
//!
//! A moving average would *smooth* motion; a spring *responds* to it. Hair does
//! not lag its head by averaging — it is pulled back to where it wants to hang
//! and swings past on the way, and only a second-order system (a position with a
//! velocity) does that. The two parameters are exactly the two an animator has
//! an intuition for: how hard it is pulled back ([`Spring::stiffness`]) and how
//! much of the swing survives ([`Spring::damping`]).
//!
//! # Determinism
//!
//! Fixed sub-stepping, plain `f64`, no clock and no randomness: the same target
//! series and the same spring give the same output on every machine and every
//! run. That is what lets a baked result — or, one day, a live one — be
//! reproducible frame for frame.

pub mod wiggle;

pub use wiggle::{Wiggle, WiggleSample, wiggle_at};

use std::f64::consts::{PI, TAU};

/// How many sub-steps each frame is integrated in.
///
/// Semi-implicit Euler is stable for a spring only while `stiffness * dt²` stays
/// small; at 24fps a stiff hair spring would ring and blow up in one step per
/// frame. Four sub-steps keep every preset here well inside that bound without
/// the cost of a proper implicit solver.
const SUBSTEPS: usize = 4;

/// A spring's feel: how strongly it is pulled back, and how quickly the swing
/// dies.
///
/// `stiffness` is the spring constant — larger catches up faster and oscillates
/// quicker. `damping` bleeds off velocity — larger overshoots less. Roughly,
/// `damping ≈ 2·√stiffness` is critical (no overshoot); below it the response
/// is bouncy, which is what hair and tails want.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub stiffness: f64,
    pub damping: f64,
}

impl Spring {
    pub const fn new(stiffness: f64, damping: f64) -> Self {
        Self { stiffness, damping }
    }

    /// Loose and bouncy: a ponytail that swings and takes a moment to hang.
    pub const fn hair() -> Self {
        Self::new(120.0, 12.0)
    }

    /// Springier and slower to settle: a tail or an antenna with a life of its
    /// own.
    pub const fn tail() -> Self {
        Self::new(80.0, 6.0)
    }

    /// Quick to catch up with barely a swing: heavier cloth, or a subtle touch
    /// of overlap on a limb.
    pub const fn stiff() -> Self {
        Self::new(300.0, 34.0)
    }
}

impl Default for Spring {
    fn default() -> Self {
        Self::hair()
    }
}

/// A spring's live state: where it is, and how fast it is moving.
///
/// Carried across frames by the caller — the whole point of secondary motion is
/// that this frame remembers the last one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringState {
    pub value: f64,
    pub velocity: f64,
}

impl SpringState {
    /// At rest at `value`, not moving. Start a chain here at its first-frame
    /// pose so the motion begins without a jolt.
    pub fn settle(value: f64) -> Self {
        Self {
            value,
            velocity: 0.0,
        }
    }

    /// Advance one frame of duration `dt` toward a **linear** `target` (a length,
    /// an amount — anything that does not wrap). For angles use [`Self::step_angle`].
    pub fn step(&mut self, target: f64, spring: Spring, dt: f64) {
        let h = dt / SUBSTEPS as f64;
        for _ in 0..SUBSTEPS {
            let accel = spring.stiffness * (target - self.value) - spring.damping * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
        }
    }

    /// Advance one frame toward an **angular** `target`, taking the shortest way
    /// round so a spring near ±π does not unwind the long way. The value is kept
    /// within `-π..=π` so a long sequence cannot drift out of range.
    pub fn step_angle(&mut self, target: f64, spring: Spring, dt: f64) {
        let h = dt / SUBSTEPS as f64;
        for _ in 0..SUBSTEPS {
            let error = wrap_pi(target - self.value);
            let accel = spring.stiffness * error - spring.damping * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
        }
        self.value = wrap_pi(self.value);
    }
}

/// Bring an angle into `-π..=π`, the shortest signed representation.
pub fn wrap_pi(angle: f64) -> f64 {
    let mut x = angle % TAU;
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 24.0;

    /// Drive the spring with a constant target and return the whole value series,
    /// starting from rest at 0.
    fn response(spring: Spring, target: f64, frames: usize) -> Vec<f64> {
        let mut s = SpringState::settle(0.0);
        (0..frames)
            .map(|_| {
                s.step(target, spring, DT);
                s.value
            })
            .collect()
    }

    #[test]
    fn a_spring_overshoots_its_target_then_settles() {
        let series = response(Spring::hair(), 1.0, 200);
        let peak = series.iter().cloned().fold(f64::MIN, f64::max);
        assert!(peak > 1.0, "a bouncy spring should overshoot 1.0, peaked at {peak}");
        let end = *series.last().unwrap();
        assert!((end - 1.0).abs() < 1e-3, "it should settle on the target, ended at {end}");
    }

    #[test]
    fn more_damping_means_less_overshoot() {
        let bouncy = Spring::new(120.0, 8.0);
        let firm = Spring::new(120.0, 30.0);
        let peak = |s: Spring| response(s, 1.0, 200).iter().cloned().fold(f64::MIN, f64::max);
        assert!(
            peak(firm) < peak(bouncy),
            "raising damping should reduce the overshoot: firm {}, bouncy {}",
            peak(firm),
            peak(bouncy)
        );
    }

    #[test]
    fn a_stiffer_spring_catches_up_sooner() {
        // At the same damping, more stiffness reaches the target in fewer frames.
        // (Compared at equal damping on purpose: a very loose, underdamped spring
        // crosses 90% early only because it is overshooting wildly past it, which
        // is not "catching up".)
        let reach = |s: Spring| {
            response(s, 1.0, 400)
                .iter()
                .position(|v| *v >= 0.9)
                .unwrap_or(usize::MAX)
        };
        let stiff = Spring::new(300.0, 20.0);
        let soft = Spring::new(60.0, 20.0);
        assert!(
            reach(stiff) < reach(soft),
            "a stiffer spring should reach the target sooner: stiff {}, soft {}",
            reach(stiff),
            reach(soft)
        );
    }

    #[test]
    fn the_integrator_is_deterministic() {
        let a = response(Spring::hair(), 0.7, 120);
        let b = response(Spring::hair(), 0.7, 120);
        assert_eq!(a, b, "same inputs must give the same output");
    }

    #[test]
    fn an_angle_spring_takes_the_short_way_round() {
        // Value just under +π, target just over -π: the shortest path is a small
        // step across the wrap, not most of the way round the circle.
        let mut s = SpringState::settle(PI - 0.05);
        let target = -PI + 0.05;
        s.step_angle(target, Spring::stiff(), DT);
        // One firm step should move it a little, and toward the target across the
        // seam — so it should wrap to just above +π (i.e. a negative value near
        // -π) rather than swinging down through zero.
        assert!(s.value.abs() > PI - 0.3, "the spring unwound the long way to {}", s.value);
    }

    #[test]
    fn a_spring_started_at_its_target_barely_moves() {
        // Settling at the target with zero velocity should stay put — the
        // no-jolt-at-frame-0 property the baker relies on.
        let mut s = SpringState::settle(0.5);
        s.step(0.5, Spring::hair(), DT);
        assert!((s.value - 0.5).abs() < 1e-9 && s.velocity.abs() < 1e-9);
    }
}
