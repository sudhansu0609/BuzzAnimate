//! Deterministic procedural jitter.
//!
//! # What it is
//!
//! A smooth wandering offset, the same at a given `(seed, time)` on every machine
//! and every run. Add it to a camera for a handheld shake, to a held character
//! for an idle sway, to a sign for a breeze — the small, constant, unrepeating
//! motion that keeps a still drawing from looking dead.
//!
//! # Why summed sines, not a random walk
//!
//! A random walk drifts and never comes back; a single sine is a metronome you
//! can read. A few sines of different frequency (a small fractal sum) wander like
//! neither — no beat to lock onto and no drift away from home — and they are
//! cheap, seamless, and exactly reproducible from the seed, which a stochastic
//! process is not. The seed decorrelates one object from the next so a crowd does
//! not sway in unison.

use std::f64::consts::TAU;

/// How much a wiggle moves, and how fast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wiggle {
    /// Peak offset, in the units of whatever it drives (document units for a
    /// position).
    pub amplitude: f64,
    /// Roughly how many times a second it wanders through its range.
    pub frequency: f64,
}

impl Wiggle {
    pub const fn new(amplitude: f64, frequency: f64) -> Self {
        Self { amplitude, frequency }
    }

    /// A barely-there drift: a held pose that breathes rather than freezes.
    pub const fn breath() -> Self {
        Self::new(6.0, 0.5)
    }

    /// A handheld camera's unsteadiness.
    pub const fn handheld() -> Self {
        Self::new(14.0, 1.6)
    }

    /// A sharp shake — an impact, a shiver.
    pub const fn shake() -> Self {
        Self::new(40.0, 6.0)
    }
}

impl Default for Wiggle {
    fn default() -> Self {
        Self::breath()
    }
}

/// A wiggle's offset at one instant: an x/y displacement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WiggleSample {
    pub dx: f64,
    pub dy: f64,
}

/// The offset of `wiggle` at `t_seconds`, for a source identified by `seed`.
///
/// The two axes are independent noise channels, so the motion traces a wandering
/// blob rather than a diagonal line.
pub fn wiggle_at(wiggle: Wiggle, seed: u64, t_seconds: f64) -> WiggleSample {
    WiggleSample {
        dx: wiggle.amplitude * fbm(seed, 0, wiggle.frequency, t_seconds),
        dy: wiggle.amplitude * fbm(seed, 1, wiggle.frequency, t_seconds),
    }
}

/// A small fractal sum of sines in `-1..=1`: a base frequency plus two faster,
/// quieter octaves, each with a seed-derived phase.
fn fbm(seed: u64, channel: u64, frequency: f64, t: f64) -> f64 {
    const OCTAVES: u32 = 3;
    let mut sum = 0.0;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    for k in 0..OCTAVES {
        let f = frequency * (1 << k) as f64;
        let phase = phase_for(seed, channel, k as u64);
        sum += amplitude * (TAU * f * t + phase).sin();
        total += amplitude;
        amplitude *= 0.5;
    }
    sum / total
}

/// A stable phase in `0..TAU` for one channel and octave of one seed.
fn phase_for(seed: u64, channel: u64, octave: u64) -> f64 {
    let mixed = splitmix64(seed ^ (channel << 40) ^ (octave << 20).wrapping_add(0x1234_5678));
    (mixed as f64 / u64::MAX as f64) * TAU
}

/// A fast, well-distributed integer hash (SplitMix64's finalizer).
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wiggle_stays_within_its_amplitude() {
        let w = Wiggle::new(10.0, 2.0);
        for i in 0..2000 {
            let s = wiggle_at(w, 42, i as f64 / 60.0);
            assert!(s.dx.abs() <= 10.0 + 1e-9, "dx {} exceeded amplitude", s.dx);
            assert!(s.dy.abs() <= 10.0 + 1e-9, "dy {} exceeded amplitude", s.dy);
        }
    }

    #[test]
    fn it_is_deterministic() {
        let w = Wiggle::handheld();
        assert_eq!(wiggle_at(w, 7, 1.234), wiggle_at(w, 7, 1.234));
    }

    #[test]
    fn different_seeds_do_not_move_together() {
        // Two sources with different seeds should not trace the same path, or a
        // crowd would sway in unison.
        let w = Wiggle::handheld();
        let mut same = 0;
        for i in 0..600 {
            let t = i as f64 / 60.0;
            let a = wiggle_at(w, 1, t);
            let b = wiggle_at(w, 2, t);
            if (a.dx - b.dx).abs() < 1e-6 && (a.dy - b.dy).abs() < 1e-6 {
                same += 1;
            }
        }
        assert!(same < 10, "seeds 1 and 2 moved together on {same} of 600 frames");
    }

    #[test]
    fn it_is_smooth_frame_to_frame() {
        // No jumps: consecutive frames at 60fps are close, so the motion reads as
        // drift rather than jitter.
        // A step change would be on the order of the amplitude; the fastest
        // octave still moves a good deal per frame, so the guard is against a
        // real discontinuity, not against speed.
        let w = Wiggle::new(20.0, 2.0);
        let mut prev = wiggle_at(w, 5, 0.0);
        for i in 1..600 {
            let s = wiggle_at(w, 5, i as f64 / 60.0);
            assert!(
                (s.dx - prev.dx).abs() < 8.0 && (s.dy - prev.dy).abs() < 8.0,
                "a jump between frames: {:?} -> {:?}",
                prev,
                s
            );
            prev = s;
        }
    }

    #[test]
    fn zero_amplitude_is_no_motion() {
        let s = wiggle_at(Wiggle::new(0.0, 3.0), 9, 2.5);
        assert_eq!(s, WiggleSample { dx: 0.0, dy: 0.0 });
    }
}
