//! **How high the voice is, moment to moment** — YIN, every ten milliseconds.
//!
//! Everything the voice engine does is measured against this: PSOLA places a
//! grain on every glottal pulse, so it has to know how far apart the pulses
//! are, and the intonation knob scales each moment's pitch around the speaker's
//! average, so it has to know both.
//!
//! The analysis runs on a 16 kHz copy. Speech pitch lives far below 8 kHz, and
//! YIN's cost grows with the square of the longest period in samples — at
//! 48 kHz a single second of audio would cost ten times as much for no better
//! an answer.

/// The pitch of a recording, one value per hop.
#[derive(Debug, Clone, PartialEq)]
pub struct PitchTrack {
    /// Seconds between values.
    pub hop_seconds: f64,
    /// Hz for each hop; `0.0` where the sound is unvoiced or silent.
    pub f0: Vec<f32>,
}

impl PitchTrack {
    /// The pitch at `seconds`, from the nearest hop; `0.0` when unvoiced.
    pub fn at(&self, seconds: f64) -> f32 {
        if self.f0.is_empty() {
            return 0.0;
        }
        let i = (seconds / self.hop_seconds).round().max(0.0) as usize;
        self.f0[i.min(self.f0.len() - 1)]
    }

    /// The speaker's centre: the geometric mean of every voiced hop, which is
    /// what "his voice is about this high" means on a log-frequency ear.
    pub fn mean(&self) -> Option<f32> {
        let voiced: Vec<f32> = self.f0.iter().copied().filter(|f| *f > 0.0).collect();
        if voiced.is_empty() {
            return None;
        }
        let log_mean = voiced.iter().map(|f| (*f as f64).ln()).sum::<f64>() / voiced.len() as f64;
        Some(log_mean.exp() as f32)
    }

    /// The share of hops that are voiced.
    pub fn voiced_share(&self) -> f32 {
        if self.f0.is_empty() {
            return 0.0;
        }
        self.f0.iter().filter(|f| **f > 0.0).count() as f32 / self.f0.len() as f32
    }
}

/// The lowest and highest pitch looked for. A deep man's fry dips to ~60 Hz; a
/// child's squeal or a scream reaches ~800.
pub const MIN_HZ: f32 = 60.0;
pub const MAX_HZ: f32 = 800.0;

const ANALYSIS_RATE: u32 = 16_000;
const HOP_SECONDS: f64 = 0.01;
/// YIN's absolute threshold on the normalised difference.
const THRESHOLD: f32 = 0.15;
/// Above this even the best dip is not a pitch.
const UNVOICED: f32 = 0.35;

/// Track the pitch of `samples` (mono, `sample_rate` Hz).
pub fn track(samples: &[f32], sample_rate: u32) -> PitchTrack {
    let x = buzz_audio::resample(samples, sample_rate, ANALYSIS_RATE);
    let sr = ANALYSIS_RATE as f32;
    let tau_min = (sr / MAX_HZ).floor() as usize;
    let tau_max = (sr / MIN_HZ).ceil() as usize;
    let window = tau_max;
    let hop = (ANALYSIS_RATE as f64 * HOP_SECONDS) as usize;
    let frames = x.len() / hop + 1;

    // Silence is not unvoiced speech, it is nothing: gate on a share of the
    // loudest moment so a quiet room's hum is never given a pitch.
    let rms_of = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt();
    let loudest = (0..frames)
        .map(|i| {
            let c = i * hop;
            let lo = c.saturating_sub(window / 2);
            let hi = (c + window / 2).min(x.len());
            if hi > lo { rms_of(&x[lo..hi]) } else { 0.0 }
        })
        .fold(0.0f32, f32::max);
    let gate = loudest * 0.06;

    let mut f0 = vec![0.0f32; frames];
    let mut diff = vec![0.0f32; tau_max + 1];
    for (i, out) in f0.iter_mut().enumerate() {
        let start = (i * hop).saturating_sub(window / 2);
        if start + window + tau_max >= x.len() {
            continue;
        }
        let seg = &x[start..start + window + tau_max];
        if rms_of(&seg[..window]) < gate.max(1e-4) {
            continue;
        }
        // Difference function.
        for (tau, d) in diff.iter_mut().enumerate().skip(1) {
            let mut acc = 0.0f32;
            for j in 0..window {
                let e = seg[j] - seg[j + tau];
                acc += e * e;
            }
            *d = acc;
        }
        // Cumulative mean normalised difference.
        diff[0] = 1.0;
        let mut running = 0.0f32;
        for tau in 1..=tau_max {
            running += diff[tau];
            diff[tau] = if running > 0.0 { diff[tau] * tau as f32 / running } else { 1.0 };
        }
        // The first dip under the threshold, walked to its bottom; otherwise
        // the deepest dip, if it is deep enough to be a pitch at all.
        let mut best = None;
        let mut tau = tau_min;
        while tau < tau_max {
            if diff[tau] < THRESHOLD {
                while tau + 1 < tau_max && diff[tau + 1] < diff[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        let best = best.or_else(|| {
            let (t, v) = (tau_min..tau_max)
                .map(|t| (t, diff[t]))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            (v < UNVOICED).then_some(t)
        });
        let Some(t) = best else { continue };
        // Parabolic interpolation between neighbours for sub-sample accuracy.
        let refined = if t > 1 && t + 1 <= tau_max {
            let (a, b, c) = (diff[t - 1], diff[t], diff[t + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-9 { t as f32 + 0.5 * (a - c) / denom } else { t as f32 }
        } else {
            t as f32
        };
        *out = sr / refined;
    }

    smooth(&mut f0);
    PitchTrack { hop_seconds: HOP_SECONDS, f0 }
}

/// Median-of-five over voiced runs, and runs shorter than three hops dropped:
/// a single voiced hop in the middle of an `s` is a mistake, and an octave
/// jump for one hop is a mistake PSOLA would make audible.
fn smooth(f0: &mut [f32]) {
    let copy = f0.to_vec();
    for i in 0..f0.len() {
        if copy[i] <= 0.0 {
            continue;
        }
        let mut near: Vec<f32> = (i.saturating_sub(2)..(i + 3).min(copy.len()))
            .map(|j| copy[j])
            .filter(|v| *v > 0.0)
            .collect();
        near.sort_by(|a, b| a.total_cmp(b));
        f0[i] = near[near.len() / 2];
    }
    let mut i = 0;
    while i < f0.len() {
        if f0[i] <= 0.0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < f0.len() && f0[i] > 0.0 {
            i += 1;
        }
        if i - start < 3 {
            f0[start..i].iter_mut().for_each(|v| *v = 0.0);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A buzzy, voice-like test signal: a band-limited sawtooth at `hz`.
    pub fn saw(hz: f32, seconds: f32, sr: u32) -> Vec<f32> {
        let n = (seconds * sr as f32) as usize;
        let harmonics = ((sr as f32 / 2.0) / hz).floor().min(30.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let mut v = 0.0;
                for k in 1..=harmonics {
                    v += (std::f32::consts::TAU * hz * k as f32 * t).sin() / k as f32;
                }
                v * 0.3
            })
            .collect()
    }

    #[test]
    fn a_steady_tone_is_tracked_at_its_pitch() {
        for hz in [110.0, 220.0, 330.0] {
            let track = track(&saw(hz, 0.6, 44_100), 44_100);
            let mean = track.mean().expect("voiced");
            assert!((mean - hz).abs() < hz * 0.02, "{hz} Hz tracked as {mean}");
            assert!(track.voiced_share() > 0.7);
        }
    }

    #[test]
    fn silence_has_no_pitch() {
        let track = track(&vec![0.0; 22_050], 44_100);
        assert_eq!(track.mean(), None);
    }
}
