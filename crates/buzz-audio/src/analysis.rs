//! Lip sync: turning recorded speech into mouth shapes, one per frame.
//!
//! # What this is, and what it is not
//!
//! Animate's automatic lip sync runs a trained phoneme recogniser. There is no
//! such model here, and pretending otherwise would be worse than useless — the
//! failure mode of a bad recogniser is a character mouthing nonsense, which an
//! animator has to find by watching and then fix by hand.
//!
//! What this does instead is honest signal analysis: it measures **loudness**,
//! **where the energy sits in the spectrum**, and **how noisy the waveform
//! is**, and maps those to Preston Blair's mouth set — the ten shapes Animate
//! uses. That distinguishes the things the *shape of the mouth* actually
//! depends on:
//!
//! * silence and closures from speech, which is most of what reads on screen;
//! * open vowels from closed ones, by where the energy sits;
//! * fricatives (`f`, `s`, `th`) from vowels, by their noisiness;
//! * rounded vowels (`o`, `u`) from spread ones (`e`, `i`), by their dark
//!   spectrum.
//!
//! It does **not** distinguish `p` from `b` from `m`, or `l` from `n` — those
//! differ in ways an amplitude spectrum cannot see. They land on the closed
//! and tongue shapes respectively, which is where an animator would put them
//! anyway.
//!
//! The result is a starting point that lands the timing right and the openness
//! roughly right, editable frame by frame afterwards. That is what makes it
//! worth having: the timing is the tedious part.

use serde::{Deserialize, Serialize};

use crate::Clip;

/// Preston Blair's mouth shapes, in the order Animate's lip-sync mapping uses.
///
/// The numbering matters: a mouth symbol holds one shape per frame, and a
/// viseme selects the frame. Keeping this order means a symbol drawn for
/// Animate works here without being redrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Viseme {
    /// Closed, at rest. Silence.
    Rest,
    /// `p`, `b`, `m` — lips pressed together.
    MBP,
    /// `f`, `v` — lower lip under the teeth.
    FV,
    /// Wide open: `a`, `i` as in "eye".
    Ai,
    /// `e` — spread and half open.
    E,
    /// `o` — rounded.
    O,
    /// `u`, `w`, `q` — small and rounded.
    U,
    /// `l` — tongue up, mouth open.
    L,
    /// `w`, `q` consonants.
    WQ,
    /// Everything else: `c`, `d`, `g`, `k`, `n`, `r`, `s`, `th`, `y`, `z`.
    Etc,
}

impl Viseme {
    /// The frame of a mouth symbol this shape lives on, 0-based.
    ///
    /// Animate's mouth symbols are drawn one shape per frame in this order, so
    /// an instance set to this frame shows the right mouth.
    pub fn frame(self) -> u32 {
        match self {
            Viseme::Rest => 0,
            Viseme::Ai => 1,
            Viseme::E => 2,
            Viseme::O => 3,
            Viseme::U => 4,
            Viseme::L => 5,
            Viseme::WQ => 6,
            Viseme::MBP => 7,
            Viseme::FV => 8,
            Viseme::Etc => 9,
        }
    }

    /// How many frames a mouth symbol needs to cover every shape.
    pub const COUNT: u32 = 10;

    pub fn label(self) -> &'static str {
        match self {
            Viseme::Rest => "Rest",
            Viseme::MBP => "MBP",
            Viseme::FV => "FV",
            Viseme::Ai => "Ai",
            Viseme::E => "E",
            Viseme::O => "O",
            Viseme::U => "U",
            Viseme::L => "L",
            Viseme::WQ => "WQ",
            Viseme::Etc => "Etc",
        }
    }

    /// Is the mouth closed in this shape?
    pub fn is_closed(self) -> bool {
        matches!(self, Viseme::Rest | Viseme::MBP)
    }
}

/// A mouth shape per animation frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VisemeTrack {
    pub frames: Vec<Viseme>,
    /// Frames per second the analysis was run at, so a document can check.
    pub fps: f64,
}

impl VisemeTrack {
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn at(&self, frame: u32) -> Viseme {
        self.frames
            .get(frame as usize)
            .copied()
            .unwrap_or(Viseme::Rest)
    }

    /// Runs of the same shape: `(first frame, viseme, length)`.
    ///
    /// This is what becomes keyframes — one per change, not one per frame,
    /// because a keyframe on every frame is unreadable and unusable in a
    /// timeline.
    pub fn runs(&self) -> Vec<(u32, Viseme, u32)> {
        let mut out: Vec<(u32, Viseme, u32)> = Vec::new();
        for (i, viseme) in self.frames.iter().enumerate() {
            match out.last_mut() {
                Some((_, previous, length)) if previous == viseme => *length += 1,
                _ => out.push((i as u32, *viseme, 1)),
            }
        }
        out
    }
}

/// How eagerly to open the mouth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LipSyncOptions {
    /// Loudness below which the mouth is closed, `0.0..=1.0`.
    ///
    /// Room tone is never truly silent, so a fixed floor of zero would leave
    /// the mouth flapping through every pause. This is measured against the
    /// clip's own loudness, so a quiet recording is not treated as silence
    /// throughout.
    pub silence: f32,
    /// Shortest run of one shape, in frames.
    ///
    /// Below about two frames at 24 fps the eye reads a mouth as flickering
    /// rather than speaking, and phoneme boundaries are noisy enough to
    /// produce exactly that.
    pub hold: u32,
}

impl Default for LipSyncOptions {
    fn default() -> Self {
        Self {
            silence: 0.06,
            hold: 2,
        }
    }
}

/// **Find the beats in a clip**, as animation-frame indices.
///
/// Honest signal analysis, like the lip sync above and with the same caveat: no
/// trained tempo model, just onset detection. It measures how sharply the
/// loudness *rises* frame to frame (a beat is an attack, not just loudness),
/// and picks the peaks that stand out against their neighbourhood, spaced so two
/// beats cannot land on top of each other. The result is a starting set of beat
/// markers an animator can key action to — right far more often than not, and
/// easy to ignore where it is not.
pub fn detect_beats(clip: &Clip, fps: f64) -> Vec<u32> {
    if fps <= 0.0 || clip.is_empty() {
        return Vec::new();
    }
    beats_from_levels(&clip.frame_levels(fps), fps)
}

/// The beat picker, over a per-frame loudness envelope. Split out so it can be
/// tested against a synthetic envelope without decoding audio.
pub fn beats_from_levels(levels: &[f32], fps: f64) -> Vec<u32> {
    if levels.len() < 3 {
        return Vec::new();
    }
    // Onset strength: the positive rise in loudness, so a sustained loud note is
    // one beat at its attack, not a beat every frame it holds.
    let flux: Vec<f32> = (0..levels.len())
        .map(|i| if i == 0 { 0.0 } else { (levels[i] - levels[i - 1]).max(0.0) })
        .collect();

    let window = ((fps * 0.2).round() as usize).max(3); // ~200 ms neighbourhood
    let min_gap = ((fps / 8.0).round() as u32).max(2); // no faster than 8 a second
    let mut beats = Vec::new();
    let mut last: Option<u32> = None;
    for i in 1..flux.len() - 1 {
        // A local peak in the onset strength.
        if flux[i] <= flux[i - 1] || flux[i] < flux[i + 1] {
            continue;
        }
        // Standing clear of the local average, with a small floor so near-silence
        // does not manufacture beats out of noise.
        let lo = i.saturating_sub(window);
        let hi = (i + window + 1).min(flux.len());
        let mean = flux[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
        if flux[i] < mean * 1.5 + 0.02 {
            continue;
        }
        let frame = i as u32;
        if let Some(l) = last {
            if frame - l < min_gap {
                continue;
            }
        }
        beats.push(frame);
        last = Some(frame);
    }
    beats
}

/// Work out a mouth shape for every frame of `clip`.
pub fn analyse_visemes(clip: &Clip, fps: f64, options: &LipSyncOptions) -> VisemeTrack {
    if fps <= 0.0 || clip.is_empty() {
        return VisemeTrack::default();
    }

    let levels = clip.frame_levels(fps);
    // Loudness is judged against this clip rather than against an absolute:
    // dialogue recorded quietly should still open the mouth.
    let loudest = levels.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    let silence = options.silence * loudest.max(0.05);

    let mono = clip.mono();
    let per_frame = (clip.sample_rate as f64 / fps).max(1.0);

    let mut frames: Vec<Viseme> = levels
        .iter()
        .enumerate()
        .map(|(i, level)| {
            if *level <= silence {
                return Viseme::Rest;
            }

            let start = (i as f64 * per_frame) as usize;
            let end = (((i + 1) as f64 * per_frame) as usize).min(mono.len());
            if start >= end {
                return Viseme::Rest;
            }

            let features = Features::measure(&mono[start..end], clip.sample_rate);
            features.viseme(*level / loudest)
        })
        .collect();

    close_the_mouth_between_words(&mut frames, &levels, silence, options.hold);
    hold(&mut frames, options.hold.max(1));

    VisemeTrack { frames, fps }
}

/// What one frame of sound looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Features {
    /// Fraction of energy above 2 kHz — high for fricatives, low for vowels.
    brightness: f32,
    /// Sign changes per sample: noisy consonants are high, vowels are low.
    noisiness: f32,
    /// Where the energy's centre of mass sits, in hertz.
    centroid: f32,
}

impl Features {
    fn measure(window: &[f32], sample_rate: u32) -> Self {
        let spectrum = spectrum(window);
        let bin_hz = sample_rate as f32 / (spectrum.len() * 2).max(1) as f32;

        let mut total = 0.0f32;
        let mut high = 0.0f32;
        let mut weighted = 0.0f32;
        for (i, magnitude) in spectrum.iter().enumerate() {
            let hz = i as f32 * bin_hz;
            total += magnitude;
            weighted += magnitude * hz;
            if hz >= 2_000.0 {
                high += magnitude;
            }
        }

        let crossings = window
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();

        Self {
            brightness: if total > 0.0 { high / total } else { 0.0 },
            noisiness: crossings as f32 / window.len().max(1) as f32,
            centroid: if total > 0.0 { weighted / total } else { 0.0 },
        }
    }

    /// The mouth shape these features suggest.
    ///
    /// The order of the tests is the point: noise is decided before vowel
    /// colour, because a fricative's centroid looks like a bright vowel's and
    /// choosing the vowel would leave the mouth wide open on every `s`.
    fn viseme(self, loudness: f32) -> Viseme {
        // Noisy and bright: a fricative. `f`/`v` are the quiet ones, `s`/`sh`
        // the loud ones, and they take different shapes.
        if self.noisiness > 0.22 && self.brightness > 0.45 {
            return if loudness < 0.35 {
                Viseme::FV
            } else {
                Viseme::Etc
            };
        }

        // Voiced, so the shape follows where the energy sits. These bands are
        // the vowel formants an amplitude spectrum can actually separate:
        // rounded vowels are dark, spread vowels are bright, open vowels are
        // loud and broad.
        match self.centroid {
            hz if hz < 600.0 => {
                if loudness > 0.55 {
                    Viseme::O
                } else {
                    Viseme::U
                }
            }
            hz if hz < 1_100.0 => {
                if loudness > 0.6 {
                    Viseme::Ai
                } else {
                    Viseme::L
                }
            }
            hz if hz < 1_900.0 => Viseme::E,
            _ => Viseme::WQ,
        }
    }
}

/// Close the mouth in the silence *before* each word.
///
/// A mouth that snaps from resting to wide open in one frame reads as a
/// glitch. Real speech — and every animator drawing it — closes the lips
/// first, and does so *before* the sound starts: the closure is anticipation.
///
/// So the closure is written into the last silent frames rather than over the
/// first voiced one. Putting it on the first voiced frame instead would delay
/// every word by a frame, which is worse than the problem it solves; and
/// making it `hold` frames long is what lets it survive the smoothing pass
/// that follows.
fn close_the_mouth_between_words(frames: &mut [Viseme], levels: &[f32], silence: f32, hold: u32) {
    let closure = hold.max(1) as usize;

    for i in 1..frames.len() {
        let was_silent = levels.get(i - 1).is_some_and(|l| *l <= silence);
        let is_speaking = levels.get(i).is_some_and(|l| *l > silence);
        if !(was_silent && is_speaking) {
            continue;
        }
        for back in 1..=closure {
            let Some(slot) = i.checked_sub(back) else {
                break;
            };
            // Only silence becomes a closure: a closure written over the tail
            // of the previous word would swallow it.
            if frames[slot] != Viseme::Rest {
                break;
            }
            frames[slot] = Viseme::MBP;
        }
    }
}

/// Remove runs shorter than `hold` frames by extending the previous shape.
///
/// Phoneme boundaries are noisy, and a mouth that changes every single frame
/// reads as vibration rather than speech. Animate's own lip sync smooths for
/// the same reason.
fn hold(frames: &mut [Viseme], hold: u32) {
    if frames.is_empty() || hold <= 1 {
        return;
    }
    let mut i = 1;
    while i < frames.len() {
        let start = i;
        while i < frames.len() && frames[i] == frames[start] {
            i += 1;
        }
        let length = (i - start) as u32;
        if length < hold {
            let previous = frames[start - 1];
            for slot in &mut frames[start..i] {
                *slot = previous;
            }
        }
    }
}

/// Magnitude spectrum of a window, via a radix-2 FFT.
///
/// Written here rather than pulled in: this is the only transform the crate
/// needs, it is forty lines, and a dependency whose whole job is one textbook
/// algorithm is a dependency that will one day need upgrading for no benefit.
fn spectrum(window: &[f32]) -> Vec<f32> {
    let size = window.len().next_power_of_two().clamp(64, 2048);
    let mut real: Vec<f32> = Vec::with_capacity(size);
    let mut imaginary = vec![0.0f32; size];

    // Hann window: without it, chopping the signal at an arbitrary point
    // smears energy across every bin and the centroid becomes meaningless.
    for i in 0..size {
        let sample = window.get(i).copied().unwrap_or(0.0);
        let taper = 0.5 - 0.5 * ((std::f32::consts::TAU * i as f32) / size as f32).cos();
        real.push(sample * taper);
    }

    fft(&mut real, &mut imaginary);

    // Only the first half is meaningful; the rest mirrors it.
    (0..size / 2)
        .map(|i| (real[i] * real[i] + imaginary[i] * imaginary[i]).sqrt())
        .collect()
}

/// In-place iterative radix-2 Cooley-Tukey FFT.
fn fft(real: &mut [f32], imaginary: &mut [f32]) {
    let n = real.len();
    if n <= 1 {
        return;
    }
    debug_assert!(n.is_power_of_two(), "the FFT needs a power-of-two length");

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            real.swap(i, j);
            imaginary.swap(i, j);
        }
    }

    let mut length = 2;
    while length <= n {
        let angle = -std::f32::consts::TAU / length as f32;
        let (step_sin, step_cos) = angle.sin_cos();
        for start in (0..n).step_by(length) {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for k in 0..length / 2 {
                let (a, b) = (start + k, start + k + length / 2);
                let tr = real[b] * wr - imaginary[b] * wi;
                let ti = real[b] * wi + imaginary[b] * wr;
                real[b] = real[a] - tr;
                imaginary[b] = imaginary[a] - ti;
                real[a] += tr;
                imaginary[a] += ti;

                let next_wr = wr * step_cos - wi * step_sin;
                wi = wr * step_sin + wi * step_cos;
                wr = next_wr;
            }
        }
        length <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_land_on_the_attacks_of_a_click_track() {
        // A per-frame loudness envelope with a sharp spike every 12 frames — a
        // steady beat at 24 fps / 2 Hz. Between spikes it is near silent.
        let fps = 24.0;
        let mut levels = vec![0.02f32; 120];
        let hits: Vec<usize> = (12..120).step_by(12).collect();
        for &h in &hits {
            levels[h] = 0.9;
        }
        let beats = beats_from_levels(&levels, fps);
        // Every spike is found, and nothing spurious between them.
        assert_eq!(beats.len(), hits.len(), "beats: {beats:?}");
        for (b, h) in beats.iter().zip(&hits) {
            assert_eq!(*b as usize, *h);
        }
    }

    #[test]
    fn silence_has_no_beats() {
        assert!(beats_from_levels(&vec![0.0f32; 100], 24.0).is_empty());
    }

    /// A clip of one tone, so the spectrum is known exactly.
    fn tone(hz: f64, seconds: f64, amplitude: f32) -> Clip {
        let rate = 44_100;
        let frames = (seconds * rate as f64) as usize;
        let samples = (0..frames)
            .map(|i| {
                let t = i as f64 / rate as f64;
                (t * hz * std::f64::consts::TAU).sin() as f32 * amplitude
            })
            .collect();
        Clip::new("Tone", rate, 1, samples).expect("a clip")
    }

    fn noise(seconds: f64, amplitude: f32) -> Clip {
        let rate = 44_100;
        let frames = (seconds * rate as f64) as usize;
        // A deterministic pseudo-random sequence: a test that fails only on
        // some runs is worse than no test.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let samples = (0..frames)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 40) as f32 / 8_388_608.0 - 1.0) * amplitude
            })
            .collect();
        Clip::new("Noise", rate, 1, samples).expect("a clip")
    }

    fn silence(seconds: f64) -> Clip {
        let rate = 44_100;
        Clip::new(
            "Silence",
            rate,
            1,
            vec![0.0; (seconds * rate as f64) as usize],
        )
        .expect("a clip")
    }

    fn join(clips: &[&Clip]) -> Clip {
        let mut samples = Vec::new();
        for clip in clips {
            samples.extend_from_slice(&clip.samples);
        }
        Clip::new("Joined", clips[0].sample_rate, 1, samples).expect("a clip")
    }

    #[test]
    fn silence_closes_the_mouth() {
        let track = analyse_visemes(&silence(0.5), 24.0, &LipSyncOptions::default());
        assert!(!track.is_empty());
        assert!(
            track.frames.iter().all(|v| *v == Viseme::Rest),
            "silence should be Rest throughout, got {:?}",
            track.frames
        );
    }

    #[test]
    fn speech_opens_it() {
        let track = analyse_visemes(&tone(500.0, 0.5, 0.6), 24.0, &LipSyncOptions::default());
        assert!(
            track.frames.iter().any(|v| !v.is_closed()),
            "a loud tone should open the mouth"
        );
    }

    /// The order of the tests inside `viseme` matters: a fricative's spectrum
    /// looks like a bright vowel's, and choosing the vowel would leave the
    /// mouth wide open on every `s`.
    #[test]
    fn noise_reads_as_a_fricative_rather_than_a_vowel() {
        let track = analyse_visemes(&noise(0.5, 0.5), 24.0, &LipSyncOptions::default());
        let fricatives = track
            .frames
            .iter()
            .filter(|v| matches!(v, Viseme::FV | Viseme::Etc))
            .count();
        assert!(
            fricatives > track.len() / 2,
            "most of a noise burst should be a fricative shape, got {:?}",
            track.frames
        );
    }

    /// A dark vowel and a bright one must not produce the same mouth.
    #[test]
    fn low_and_high_vowels_take_different_shapes() {
        let options = LipSyncOptions {
            hold: 1,
            ..LipSyncOptions::default()
        };
        let low = analyse_visemes(&tone(300.0, 0.4, 0.7), 24.0, &options);
        let high = analyse_visemes(&tone(2_400.0, 0.4, 0.7), 24.0, &options);

        let common = |track: &VisemeTrack| {
            let mut counts = std::collections::BTreeMap::new();
            for v in &track.frames {
                *counts.entry(v.label()).or_insert(0) += 1;
            }
            counts.into_iter().max_by_key(|(_, n)| *n).map(|(l, _)| l)
        };
        assert_ne!(
            common(&low),
            common(&high),
            "a 300 Hz vowel and a 2.4 kHz one should not read the same"
        );
    }

    /// The lips close in the silence *before* a word — anticipation — and the
    /// word itself is not delayed by it.
    #[test]
    fn the_lips_close_before_a_word_rather_than_on_it() {
        let clip = join(&[&silence(0.3), &tone(700.0, 0.4, 0.7)]);
        let options = LipSyncOptions::default();
        let track = analyse_visemes(&clip, 24.0, &options);

        let first_shape = track
            .frames
            .iter()
            .position(|v| *v != Viseme::Rest)
            .expect("the tone should open the mouth");

        assert_eq!(
            track.frames[first_shape],
            Viseme::MBP,
            "the first shape should be a closure: {:?}",
            &track.frames[..first_shape + 4]
        );

        // And it sits in the silence: an open shape follows within the hold,
        // so the word is not pushed later by its own closure.
        let opens = track.frames[first_shape..]
            .iter()
            .position(|v| !v.is_closed())
            .expect("the mouth should open");
        // The closure is the hold, plus at most the one partial frame where
        // the sound begins mid-frame and is still too quiet to shape.
        assert!(
            opens <= options.hold as usize + 2,
            "the word was delayed by {opens} frames of closure"
        );
    }

    /// A mouth that changes every frame reads as vibration, not speech.
    #[test]
    fn shapes_are_held_for_at_least_two_frames() {
        let clip = join(&[
            &tone(300.0, 0.2, 0.7),
            &noise(0.05, 0.6),
            &tone(2_000.0, 0.2, 0.7),
            &noise(0.04, 0.5),
            &tone(500.0, 0.2, 0.7),
        ]);
        let track = analyse_visemes(&clip, 24.0, &LipSyncOptions::default());

        for (start, viseme, length) in track.runs() {
            // The first run may be short: it has nothing before it to extend.
            if start == 0 {
                continue;
            }
            assert!(
                length >= 2,
                "{:?} at frame {start} lasts only {length} frame(s)",
                viseme
            );
        }
    }

    #[test]
    fn runs_collapse_a_track_into_keyframes() {
        let track = VisemeTrack {
            frames: vec![
                Viseme::Rest,
                Viseme::Rest,
                Viseme::Ai,
                Viseme::Ai,
                Viseme::Ai,
                Viseme::O,
            ],
            fps: 24.0,
        };
        assert_eq!(
            track.runs(),
            vec![(0, Viseme::Rest, 2), (2, Viseme::Ai, 3), (5, Viseme::O, 1),]
        );
    }

    /// A quiet recording must still animate: loudness is judged against the
    /// clip, not against an absolute that only studio dialogue reaches.
    #[test]
    fn a_quietly_recorded_clip_still_opens_the_mouth() {
        let track = analyse_visemes(&tone(600.0, 0.5, 0.05), 24.0, &LipSyncOptions::default());
        assert!(
            track.frames.iter().any(|v| !v.is_closed()),
            "quiet speech should still animate: {:?}",
            track.frames
        );
    }

    #[test]
    fn every_viseme_has_its_own_frame_within_the_symbol() {
        let all = [
            Viseme::Rest,
            Viseme::Ai,
            Viseme::E,
            Viseme::O,
            Viseme::U,
            Viseme::L,
            Viseme::WQ,
            Viseme::MBP,
            Viseme::FV,
            Viseme::Etc,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for viseme in all {
            assert!(seen.insert(viseme.frame()), "{viseme:?} shares a frame");
            assert!(viseme.frame() < Viseme::COUNT);
        }
        assert_eq!(seen.len(), Viseme::COUNT as usize);
    }

    #[test]
    fn an_impossible_frame_rate_produces_nothing_rather_than_a_panic() {
        assert!(
            analyse_visemes(&tone(400.0, 0.2, 0.5), 0.0, &LipSyncOptions::default()).is_empty()
        );
    }

    /// The FFT is the one piece of maths here that is easy to get subtly
    /// wrong, so it is checked against a signal whose spectrum is known.
    #[test]
    fn the_fft_finds_the_frequency_it_is_given() {
        let rate = 8_000.0f32;
        let size = 512;
        let hz = 1_000.0f32;
        let window: Vec<f32> = (0..size)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / rate).sin())
            .collect();

        let spectrum = spectrum(&window);
        let bin_hz = rate / (spectrum.len() * 2) as f32;
        let peak = spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i as f32 * bin_hz)
            .expect("a peak");

        assert!(
            (peak - hz).abs() < bin_hz * 2.0,
            "expected a peak near {hz} Hz, found {peak} Hz"
        );
    }

    #[test]
    fn the_fft_leaves_silence_silent() {
        let spectrum = spectrum(&vec![0.0; 256]);
        assert!(spectrum.iter().all(|m| *m < 1e-6));
    }
}
