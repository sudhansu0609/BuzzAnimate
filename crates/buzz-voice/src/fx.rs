//! Tone, texture and space: the effects after the re-voicing.
//!
//! Every filter here is a textbook one (the RBJ cookbook biquads, a Schroeder
//! reverb, a feed-forward compressor) because a voice designer's sliders have
//! to do what their names say, and the textbook versions are the ones whose
//! behaviour is known.

use std::f32::consts::{PI, TAU};

/// One second-order section, Direct Form I.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn new(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b: [b0 / a0, b1 / a0, b2 / a0], a: [a1 / a0, a2 / a0], x: [0.0; 2], y: [0.0; 2] }
    }

    fn omega(hz: f32, sr: u32) -> (f32, f32) {
        let w = TAU * (hz / sr as f32).min(0.49);
        (w.sin(), w.cos())
    }

    pub fn low_pass(hz: f32, q: f32, sr: u32) -> Self {
        let (s, c) = Self::omega(hz, sr);
        let alpha = s / (2.0 * q);
        Self::new((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn high_pass(hz: f32, q: f32, sr: u32) -> Self {
        let (s, c) = Self::omega(hz, sr);
        let alpha = s / (2.0 * q);
        Self::new((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn peak(hz: f32, q: f32, db: f32, sr: u32) -> Self {
        let (s, c) = Self::omega(hz, sr);
        let a = 10f32.powf(db / 40.0);
        let alpha = s / (2.0 * q);
        Self::new(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    pub fn low_shelf(hz: f32, db: f32, sr: u32) -> Self {
        let (s, c) = Self::omega(hz, sr);
        let a = 10f32.powf(db / 40.0);
        let alpha = s / 2.0 * 2f32.sqrt();
        let k = 2.0 * a.sqrt() * alpha;
        Self::new(
            a * ((a + 1.0) - (a - 1.0) * c + k),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - k),
            (a + 1.0) + (a - 1.0) * c + k,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - k,
        )
    }

    pub fn high_shelf(hz: f32, db: f32, sr: u32) -> Self {
        let (s, c) = Self::omega(hz, sr);
        let a = 10f32.powf(db / 40.0);
        let alpha = s / 2.0 * 2f32.sqrt();
        let k = 2.0 * a.sqrt() * alpha;
        Self::new(
            a * ((a + 1.0) + (a - 1.0) * c + k),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - k),
            (a + 1.0) - (a - 1.0) * c + k,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - k,
        )
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }

    pub fn run(mut self, samples: &mut [f32]) {
        samples.iter_mut().for_each(|s| *s = self.tick(*s));
    }
}

/// A follower of the signal's loudness, `ms` to rise and fall.
pub fn envelope(x: &[f32], sr: u32, ms: f32) -> Vec<f32> {
    let k = (-1.0 / (sr as f32 * ms / 1000.0)).exp();
    let mut e = 0.0f32;
    x.iter()
        .map(|v| {
            e = k * e + (1.0 - k) * v.abs();
            e
        })
        .collect()
}

/// **Breath**: high-passed noise riding the voice's own loudness, so it
/// breathes where he speaks and is silent where he is.
pub fn breathe(x: &mut [f32], sr: u32, amount: f32, noise: &mut crate::Noise) {
    if amount <= 0.0 {
        return;
    }
    let env = envelope(x, sr, 12.0);
    let mut hp = Biquad::high_pass(1_800.0, 0.7, sr);
    // A breathy voice is also a softer-edged one: take a little of the top
    // off the tone underneath the air.
    Biquad::high_shelf(3_500.0, -6.0 * amount, sr).run(x);
    for (v, e) in x.iter_mut().zip(env) {
        *v += hp.tick(noise.next()) * e * amount * 1.6;
    }
}

/// **Tremolo**: the level wobbling, as a frightened or very old voice does.
pub fn tremolo(x: &mut [f32], sr: u32, depth: f32, hz: f32) {
    if depth <= 0.0 {
        return;
    }
    for (i, v) in x.iter_mut().enumerate() {
        let t = i as f32 / sr as f32;
        *v *= 1.0 - depth * 0.5 * (1.0 + (TAU * hz * t).sin());
    }
}

/// **Telephone / radio**: everything outside 300–3400 Hz gone, mixed in by
/// `amount`.
pub fn telephone(x: &mut [f32], sr: u32, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let mut band = x.to_vec();
    for _ in 0..2 {
        Biquad::high_pass(300.0, 0.7, sr).run(&mut band);
        Biquad::low_pass(3_400.0, 0.7, sr).run(&mut band);
    }
    // A little drive, which is what makes it a speaker and not just an EQ.
    for (v, b) in x.iter_mut().zip(band) {
        let driven = (b * 2.5).tanh() / 2.5_f32.tanh();
        *v = *v * (1.0 - amount) + driven * amount * 1.4;
    }
}

/// **Compression**: the loud and the quiet brought closer, then made up —
/// a shouting voice that does not clip and a whisper that is still heard.
pub fn compress(x: &mut [f32], sr: u32, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let threshold = 10f32.powf((-12.0 - 18.0 * amount) / 20.0);
    let ratio = 1.0 + 5.0 * amount;
    let attack = (-1.0 / (sr as f32 * 0.004)).exp();
    let release = (-1.0 / (sr as f32 * 0.09)).exp();
    let mut env = 0.0f32;
    let mut peak_in = 0.0f32;
    let mut peak_out = 0.0f32;
    for v in x.iter_mut() {
        let level = v.abs();
        peak_in = peak_in.max(level);
        let k = if level > env { attack } else { release };
        env = k * env + (1.0 - k) * level;
        let gain = if env > threshold { (threshold / env).powf(1.0 - 1.0 / ratio) } else { 1.0 };
        *v *= gain;
        peak_out = peak_out.max(v.abs());
    }
    // Make-up: back to the loudness it came in at.
    if peak_out > 1e-6 {
        let makeup = peak_in / peak_out;
        x.iter_mut().for_each(|v| *v *= makeup);
    }
}

/// **Room**: a small Schroeder reverb — four combs in parallel, two
/// all-passes in series — mixed in by `amount`.
pub fn room(x: &[f32], sr: u32, amount: f32) -> Vec<f32> {
    if amount <= 0.0 {
        return x.to_vec();
    }
    let ms = |m: f32| ((m / 1000.0) * sr as f32).max(1.0) as usize;
    let tail = ms(400.0 + 900.0 * amount);
    let mut out = x.to_vec();
    out.resize(x.len() + tail, 0.0);
    let feedback = 0.72 + 0.18 * amount;
    let damp = 0.3;
    let mut wet = vec![0.0f32; out.len()];
    for delay in [29.7, 37.1, 41.1, 43.7].map(ms) {
        let mut buf = vec![0.0f32; delay];
        let mut low = 0.0f32;
        for (i, w) in wet.iter_mut().enumerate() {
            let input = out[i];
            let slot = i % delay;
            let y = buf[slot];
            low = y * (1.0 - damp) + low * damp;
            buf[slot] = input + low * feedback;
            *w += y * 0.25;
        }
    }
    for (delay, g) in [(ms(5.0), 0.7f32), (ms(1.7), 0.7)] {
        let mut buf = vec![0.0f32; delay];
        for (i, w) in wet.iter_mut().enumerate() {
            let slot = i % delay;
            let delayed = buf[slot];
            let y = -g * *w + delayed;
            buf[slot] = *w + g * y;
            *w = y;
        }
    }
    let mix = 0.45 * amount;
    for (o, w) in out.iter_mut().zip(wet) {
        *o = *o * (1.0 - mix * 0.5) + w * mix;
    }
    // Trim the tail where it has died away.
    let floor = 1e-4;
    while out.len() > x.len() && out.last().is_some_and(|v| v.abs() < floor) {
        out.pop();
    }
    out
}

/// Gain in dB, then a soft ceiling just under full scale so nothing a slider
/// does can clip the file.
pub fn level(x: &mut [f32], gain_db: f32, normalize: bool) {
    let g = 10f32.powf(gain_db / 20.0);
    x.iter_mut().for_each(|v| *v *= g);
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if normalize && peak > 1e-6 {
        let k = 0.89 / peak;
        x.iter_mut().for_each(|v| *v *= k);
    } else if peak > 0.98 {
        x.iter_mut().for_each(|v| *v = soft_clip(*v));
    }
}

fn soft_clip(v: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = v.abs();
    if a <= KNEE {
        v
    } else {
        let over = (a - KNEE) / (1.0 - KNEE);
        v.signum() * (KNEE + (1.0 - KNEE) * (over * PI / 2.0).tanh().min(1.0) * 0.98)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, sr: u32) -> Vec<f32> {
        (0..sr / 2).map(|i| (TAU * hz * i as f32 / sr as f32).sin() * 0.5).collect()
    }
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn shelves_lift_and_cut_their_own_end_of_the_spectrum() {
        let sr = 44_100;
        let mut low = tone(100.0, sr);
        let mut high = tone(8_000.0, sr);
        Biquad::low_shelf(250.0, 6.0, sr).run(&mut low);
        Biquad::low_shelf(250.0, 6.0, sr).run(&mut high);
        assert!(rms(&low) > 0.35 * 1.7, "a +6 dB low shelf should nearly double a 100 Hz tone");
        assert!((rms(&high) - 0.3535).abs() < 0.03, "and leave 8 kHz alone");
    }

    #[test]
    fn the_telephone_band_removes_the_bass() {
        let sr = 44_100;
        let mut x = tone(80.0, sr);
        telephone(&mut x, sr, 1.0);
        assert!(rms(&x[2000..]) < 0.08);
    }

    #[test]
    fn nothing_clips() {
        let mut x = tone(200.0, 44_100);
        level(&mut x, 18.0, false);
        assert!(x.iter().all(|v| v.abs() <= 1.0));
    }
}
