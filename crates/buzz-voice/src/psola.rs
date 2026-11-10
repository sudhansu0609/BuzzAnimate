//! **TD-PSOLA**: the voice re-spaced, one glottal pulse at a time.
//!
//! A voiced sound is a train of pulses from the vocal folds, each one ringing
//! through the mouth and throat. The ring is the *formants* — what makes an
//! `aa` an `aa` and this voice this person's — and the spacing of the pulses is
//! the *pitch*. Cut the signal into two-period grains centred on each pulse,
//! lay them back down closer together and the pitch rises while every ring
//! stays as it was. Lay the same grain down twice, or skip one, and the line
//! gets longer or shorter without its pitch moving. Those two facts are the
//! whole of this module.
//!
//! Unvoiced stretches (`s`, `sh`, breath) have no pulses; they are cut into
//! short fixed grains and only ever re-timed, never re-pitched.

use crate::pitch::PitchTrack;

/// One grain's centre in the analysed signal.
#[derive(Debug, Clone, Copy)]
struct Mark {
    pos: usize,
    period: usize,
    /// Measured pitch, Hz; `0.0` for an unvoiced grain.
    f0: f32,
}

/// What to do with each grain.
pub struct Plan<'a> {
    /// Output length over input length.
    pub time_ratio: f64,
    /// The pitch a voiced grain should have, given the analysis time (seconds
    /// into *this* signal) and the pitch it was measured at. Returning the
    /// measured pitch leaves it alone.
    pub target_f0: &'a dyn Fn(f64, f32) -> f32,
    /// 0..1: alternate grains are weakened by this much and every period is
    /// jittered — the sub-harmonic rattle of a rough or growling voice.
    pub roughness: f32,
}

/// Re-voice `x` (mono, `sample_rate` Hz) to `plan`, using `track` (measured
/// on `x` itself).
pub fn render(x: &[f32], sample_rate: u32, track: &PitchTrack, plan: &Plan<'_>) -> Vec<f32> {
    if x.is_empty() {
        return Vec::new();
    }
    let marks = marks(x, sample_rate, track);
    let sr = sample_rate as f64;
    let out_len = ((x.len() as f64) * plan.time_ratio).round().max(1.0) as usize;
    let longest = marks.iter().map(|m| m.period).max().unwrap_or(1);
    let mut out = vec![0.0f32; out_len + 2 * longest + 2];
    let mut weight = vec![0.0f32; out.len()];
    let mut noise = crate::Noise::new(0x5eed);

    let mut ts = 0.0f64;
    let mut odd = false;
    while (ts as usize) < out_len {
        let ta = ts / plan.time_ratio;
        let k = nearest(&marks, ta);
        let m = marks[k];
        let p = m.period;
        let (step, gain) = if m.f0 > 0.0 {
            let target = (plan.target_f0)(ta / sr, m.f0).max(1.0);
            let ratio = (target / m.f0).clamp(0.35, 3.0) as f64;
            let jitter = 1.0 + plan.roughness as f64 * 0.04 * noise.next() as f64;
            odd = !odd;
            let gain = if odd { 1.0 - 0.55 * plan.roughness } else { 1.0 };
            (p as f64 / ratio * jitter, gain)
        } else {
            (p as f64, 1.0)
        };
        // A Hann window two periods long, centred on the pulse.
        let centre = ts.round() as isize + longest as isize;
        let span = 2 * p;
        for j in 0..=span {
            let src = m.pos as isize + j as isize - p as isize;
            let dst = centre + j as isize - p as isize;
            if src < 0 || src as usize >= x.len() || dst < 0 || dst as usize >= out.len() {
                continue;
            }
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * j as f32 / span as f32).cos();
            out[dst as usize] += x[src as usize] * w * gain;
            weight[dst as usize] += w;
        }
        ts += step.max(1.0);
    }

    // Grains crowded together for a higher pitch add up louder, and grains
    // spread apart for a lower one leave dips between them; dividing by the
    // window total keeps the level where the performance had it. The floor
    // stops the gaps of a very deep shift being blown up into noise.
    out.iter_mut()
        .zip(&weight)
        .for_each(|(v, w)| *v /= w.max(0.5));
    out.drain(..longest);
    out.truncate(out_len);
    out
}

/// Grain centres: one per pitch period through voiced sound, snapped to the
/// strongest sample near where the period says the next pulse is; a fixed
/// five-millisecond spacing elsewhere.
fn marks(x: &[f32], sample_rate: u32, track: &PitchTrack) -> Vec<Mark> {
    let sr = sample_rate as f64;
    let unvoiced = ((sr * 0.005) as usize).max(8);
    let mut marks = Vec::new();
    let mut t = 0usize;
    while t < x.len() {
        let f0 = track.at(t as f64 / sr);
        if f0 > 0.0 {
            let period = ((sr / f0 as f64).round() as usize).max(8);
            // Snap to the pulse: the largest magnitude within a fifth of a
            // period, so grains sit on the same point of every cycle.
            let reach = period / 5;
            let lo = t.saturating_sub(reach);
            let hi = (t + reach).min(x.len() - 1);
            let peak = (lo..=hi)
                .max_by(|a, b| x[*a].abs().total_cmp(&x[*b].abs()))
                .unwrap_or(t);
            let pos = if marks.last().is_some_and(|m: &Mark| peak <= m.pos) { t } else { peak };
            marks.push(Mark { pos, period, f0 });
            t = pos + period;
        } else {
            marks.push(Mark { pos: t, period: unvoiced, f0: 0.0 });
            t += unvoiced;
        }
    }
    if marks.is_empty() {
        marks.push(Mark { pos: 0, period: unvoiced, f0: 0.0 });
    }
    marks
}

fn nearest(marks: &[Mark], at: f64) -> usize {
    let at = at.max(0.0) as usize;
    match marks.binary_search_by_key(&at, |m| m.pos) {
        Ok(i) => i,
        Err(0) => 0,
        Err(i) if i >= marks.len() => marks.len() - 1,
        Err(i) => {
            if at - marks[i - 1].pos <= marks[i].pos - at { i - 1 } else { i }
        }
    }
}
