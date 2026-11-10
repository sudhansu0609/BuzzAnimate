//! **Character voices** made from the creator's own performance.
//!
//! He records every line himself. Each character has a [`VoiceProfile`] —
//! pitch, formants, intonation, tempo, breath, rasp, tone, level, room — and a
//! line is re-voiced by its speaker's profile and nobody else's. See
//! `docs/VOICES_AND_ACTING_PLAN.md`.
//!
//! # A line belongs to one speaker
//!
//! [`voice_dialogue`] is the call the rest of the program makes. It takes the
//! recording and the lines in it — each a span with a speaker, and an emotion —
//! and re-voices **each span with its own speaker's voice only**, leaving the
//! gaps between lines as they were. It hands back where every line now sits,
//! because a voice with a slower tempo makes its lines longer, and lip sync,
//! captions and acting must be laid against the new times, each on its own
//! speaker and nobody else.
//!
//! # No model
//!
//! Everything here is signal processing — YIN for pitch, TD-PSOLA for
//! re-voicing, textbook filters for tone. Deterministic, offline, instant
//! enough to scrub sliders against, and testable. Cloning a voice from a
//! sample is a separate, later backend (plan stage V7) that runs *before* this
//! chain, so a clone can still be aged, roughened or put on the radio.

pub mod fx;
pub mod pitch;
pub mod profile;
pub mod psola;

pub use pitch::{PitchTrack, track as track_pitch};
pub use profile::{Emotion, Preset, Push, VoiceProfile};

use std::collections::BTreeMap;

/// Re-voice one mono recording with `profile`.
pub fn render(samples: &[f32], sample_rate: u32, profile: &VoiceProfile) -> Vec<f32> {
    let p = profile.clamped();
    let mut noise = Noise::new(0xb022);
    let mut x = if p.moves_pitch_or_time() {
        revoice(samples, sample_rate, &p)
    } else {
        samples.to_vec()
    };
    fx::breathe(&mut x, sample_rate, p.breathiness, &mut noise);
    if p.warmth.abs() > 0.05 {
        fx::Biquad::low_shelf(220.0, p.warmth, sample_rate).run(&mut x);
    }
    if p.brightness.abs() > 0.05 {
        fx::Biquad::high_shelf(3_800.0, p.brightness, sample_rate).run(&mut x);
    }
    if p.nasal.abs() > 0.05 {
        fx::Biquad::peak(1_100.0, 2.2, p.nasal, sample_rate).run(&mut x);
    }
    fx::telephone(&mut x, sample_rate, p.telephone);
    fx::compress(&mut x, sample_rate, p.compression);
    fx::tremolo(&mut x, sample_rate, p.tremolo, 5.5);
    let mut x = fx::room(&x, sample_rate, p.room);
    fx::level(&mut x, p.gain, p.normalize);
    x
}

/// Pitch, formants, intonation, tempo, vibrato and rasp: the PSOLA half.
///
/// The formant shift is a resample — played faster, every frequency in the
/// voice rises together, throat and pitch alike — followed by PSOLA putting
/// the pitch and the duration back where the sliders want them. So pitch and
/// throat size are two independent knobs.
fn revoice(samples: &[f32], sample_rate: u32, p: &VoiceProfile) -> Vec<f32> {
    let f = p.formant as f64;
    let shifted;
    let y: &[f32] = if (f - 1.0).abs() > 0.005 {
        let to = ((sample_rate as f64) / f).round() as u32;
        shifted = buzz_audio::resample(samples, sample_rate, to);
        &shifted
    } else {
        samples
    };
    let track = pitch::track(y, sample_rate);
    // The performer's own centre, in the original's terms.
    let centre = track.mean().map(|m| m as f64 / f).unwrap_or(120.0);
    let semis = 2f64.powf(p.pitch as f64 / 12.0);
    let range = p.intonation as f64;
    let (vib, rate) = (p.vibrato as f64, p.vibrato_rate as f64);
    let target = move |seconds: f64, f0_y: f32| -> f32 {
        let f0 = f0_y as f64 / f;
        let wobble = 2f64.powf(vib * (std::f64::consts::TAU * rate * seconds * f).sin() / 12.0);
        // The output plays at the original rate, so the pitch asked for is
        // in the original's Hz — PSOLA divides it by the resampled pitch.
        (centre * (f0 / centre).powf(range) * semis * wobble) as f32
    };
    let plan = psola::Plan {
        time_ratio: f / p.tempo as f64,
        target_f0: &target,
        roughness: p.roughness,
    };
    psola::render(y, sample_rate, &track, &plan)
}

/// One line in a recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Who says it — the key into the voices map.
    pub speaker: String,
    /// Seconds into the recording.
    pub start: f64,
    pub end: f64,
    pub emotion: Emotion,
    /// 0 … 1.
    pub intensity: f32,
}

/// Where a line ended up after re-voicing.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub speaker: String,
    /// Seconds in the re-voiced recording.
    pub start: f64,
    pub end: f64,
    /// Where the line was in the original recording.
    pub from_start: f64,
    pub from_end: f64,
    pub emotion: Emotion,
    pub intensity: f32,
}

/// A recording re-voiced line by line.
#[derive(Debug, Clone, PartialEq)]
pub struct Voiced {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
    /// Every line, in order, at its new time.
    pub lines: Vec<Placed>,
}

impl Voiced {
    /// Where a moment of the original recording is now: shifted by every
    /// line before it that changed length, and stretched with the line it
    /// falls inside. What a scene's span or a hand-placed cue is moved with.
    pub fn map_time(&self, t: f64) -> f64 {
        let mut shift = 0.0;
        for line in &self.lines {
            if t < line.from_start {
                break;
            }
            if t <= line.from_end {
                let from = (line.from_end - line.from_start).max(1e-9);
                return line.start + (t - line.from_start) / from * (line.end - line.start);
            }
            shift = line.end - line.from_end;
        }
        t + shift
    }
}

/// **Re-voice a recording, each line in its own speaker's voice.**
///
/// `lines` may come in any order and are voiced in time order; overlapping
/// spans are trimmed to start where the previous one ended (one recording, one
/// performer — two lines cannot really overlap). A speaker with no entry in
/// `voices` keeps the recording as it is. Everything between lines — breaths,
/// room tone, pauses — is kept untouched, so the timing of the scene is his.
pub fn voice_dialogue(
    samples: &[f32],
    sample_rate: u32,
    lines: &[Line],
    voices: &BTreeMap<String, VoiceProfile>,
) -> Voiced {
    let sr = sample_rate as f64;
    let at = |s: f64| ((s * sr).round().max(0.0) as usize).min(samples.len());
    let mut ordered: Vec<&Line> = lines.iter().collect();
    ordered.sort_by(|a, b| a.start.total_cmp(&b.start));

    // Ten milliseconds of crossfade either side of a line, so a re-voiced
    // span never clicks against the untouched room tone around it.
    let fade = (sr * 0.01) as usize;
    let mut out: Vec<f32> = Vec::with_capacity(samples.len());
    let mut placed = Vec::with_capacity(lines.len());
    let mut cursor = 0usize;
    for line in ordered {
        let start = at(line.start).max(cursor);
        let end = at(line.end).max(start);
        out.extend_from_slice(&samples[cursor..start]);
        let new_start = out.len() as f64 / sr;
        let span = &samples[start..end];
        let voiced = match voices.get(&line.speaker) {
            Some(profile) => render(span, sample_rate, &profile.for_line(line.emotion, line.intensity)),
            None => span.to_vec(),
        };
        crossfade_in(&mut out, &voiced, fade);
        placed.push(Placed {
            speaker: line.speaker.clone(),
            start: new_start,
            end: new_start + voiced.len() as f64 / sr,
            from_start: start as f64 / sr,
            from_end: end as f64 / sr,
            emotion: line.emotion,
            intensity: line.intensity,
        });
        cursor = end;
    }
    out.extend_from_slice(&samples[cursor..]);
    Voiced { sample_rate, samples: out, lines: placed }
}

/// Append `next` to `out`, ramping its first and last `fade` samples so the
/// joins are silent.
fn crossfade_in(out: &mut Vec<f32>, next: &[f32], fade: usize) {
    let n = next.len();
    let fade = fade.min(n / 2);
    for (i, v) in next.iter().enumerate() {
        let ramp = if i < fade {
            i as f32 / fade as f32
        } else if i + fade >= n {
            (n - i) as f32 / fade as f32
        } else {
            1.0
        };
        out.push(v * ramp);
    }
}

/// A small, seeded noise source: the same take re-voiced twice is the same
/// file twice.
pub struct Noise(u64);

impl Noise {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    /// −1 … 1.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::tests::saw;

    const SR: u32 = 44_100;

    fn mean_pitch(x: &[f32]) -> f32 {
        pitch::track(x, SR).mean().expect("voiced")
    }

    /// The share of energy above 1.5 kHz — rises as the formants do.
    fn brightness(x: &[f32]) -> f32 {
        let mut hp = x.to_vec();
        fx::Biquad::high_pass(1_500.0, 0.7, SR).run(&mut hp);
        let e = |v: &[f32]| v.iter().map(|s| s * s).sum::<f32>();
        e(&hp) / e(x).max(1e-9)
    }

    /// A vowel-ish test signal: a sawtooth through two resonances, so it has
    /// formants to keep or move.
    fn vowel(hz: f32) -> Vec<f32> {
        let mut x = saw(hz, 0.8, SR);
        fx::Biquad::peak(700.0, 4.0, 14.0, SR).run(&mut x);
        fx::Biquad::peak(1_200.0, 4.0, 10.0, SR).run(&mut x);
        fx::Biquad::low_pass(3_000.0, 0.7, SR).run(&mut x);
        let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        x.iter_mut().for_each(|v| *v *= 0.5 / peak);
        x
    }

    #[test]
    fn natural_leaves_the_performance_alone() {
        let x = vowel(130.0);
        let profile = VoiceProfile { normalize: false, ..Preset::Natural.profile() };
        assert_eq!(render(&x, SR, &profile), x);
    }

    #[test]
    fn an_octave_up_doubles_the_pitch_and_keeps_the_length() {
        let x = vowel(130.0);
        let y = render(&x, SR, &VoiceProfile { pitch: 12.0, ..VoiceProfile::default() });
        let got = mean_pitch(&y);
        assert!((got - 260.0).abs() < 260.0 * 0.04, "expected ~260 Hz, got {got}");
        assert!((y.len() as f32 - x.len() as f32).abs() < SR as f32 * 0.02);
    }

    #[test]
    fn the_formant_knob_moves_the_throat_not_the_pitch() {
        let x = vowel(130.0);
        let y = render(&x, SR, &VoiceProfile { formant: 1.25, ..VoiceProfile::default() });
        let got = mean_pitch(&y);
        assert!((got - 130.0).abs() < 130.0 * 0.04, "pitch moved to {got}");
        assert!(brightness(&y) > brightness(&x) * 1.2, "the formants did not rise");
        assert!((y.len() as f32 - x.len() as f32).abs() < SR as f32 * 0.03);
    }

    #[test]
    fn tempo_changes_the_length_and_not_the_pitch() {
        let x = vowel(150.0);
        let y = render(&x, SR, &VoiceProfile { tempo: 1.5, ..VoiceProfile::default() });
        let ratio = y.len() as f32 / x.len() as f32;
        assert!((ratio - 1.0 / 1.5).abs() < 0.03, "length ratio {ratio}");
        assert!((mean_pitch(&y) - 150.0).abs() < 6.0);
    }

    #[test]
    fn every_preset_produces_clean_audio() {
        let x = vowel(120.0);
        for preset in Preset::ALL {
            let y = render(&x, SR, &preset.profile());
            assert!(!y.is_empty(), "{preset:?} produced nothing");
            assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "{preset:?} clipped or broke");
        }
    }

    #[test]
    fn a_woman_from_a_man_is_higher_and_brighter() {
        let man = vowel(115.0);
        let woman = render(&man, SR, &Preset::Woman.profile());
        assert!(mean_pitch(&woman) > 115.0 * 1.35);
        assert!(brightness(&woman) > brightness(&man));
    }

    #[test]
    fn emotion_only_pushes_when_allowed() {
        let base = Preset::Man.profile();
        assert_eq!(base.for_line(Emotion::Angry, 1.0), base, "push is off by default");
        let pushed = VoiceProfile { emotion_push: 1.0, ..base.clone() }.for_line(Emotion::Scared, 1.0);
        assert!(pushed.pitch > base.pitch && pushed.vibrato > base.vibrato);
    }

    #[test]
    fn script_tags_are_read_in_english_and_hinglish() {
        assert_eq!(Emotion::parse("angry"), Some((Emotion::Angry, 0.65)));
        assert_eq!(Emotion::parse("very angry"), Some((Emotion::Angry, 1.0)));
        assert_eq!(Emotion::parse("horrified"), Some((Emotion::Scared, 1.0)));
        assert_eq!(Emotion::parse("thoda gusse mein"), Some((Emotion::Angry, 0.35)));
        assert_eq!(Emotion::parse("khush hokar"), Some((Emotion::Happy, 0.65)));
        assert_eq!(Emotion::parse("dheere se"), Some((Emotion::Whisper, 0.65)));
        assert_eq!(Emotion::parse("looking at the door"), None);
    }

    #[test]
    fn a_profile_survives_a_save_with_missing_knobs() {
        let json = serde_json::to_string(&Preset::OldWoman.profile()).unwrap();
        let back: VoiceProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Preset::OldWoman.profile());
        let sparse: VoiceProfile = serde_json::from_str(r#"{"name":"Raju","pitch":3.0}"#).unwrap();
        assert_eq!(sparse.formant, 1.0);
        assert_eq!(sparse.pitch, 3.0);
    }

    /// **Each line in its own speaker's voice, and only there.** Two lines
    /// from one performer: Raju's is raised to a woman's voice, Guddu's is
    /// left alone, and the pause between them is untouched.
    #[test]
    fn each_line_takes_its_own_speakers_voice() {
        let gap = vec![0.0f32; (SR as f32 * 0.3) as usize];
        let mut rec = vowel(120.0);
        let first_end = rec.len() as f64 / SR as f64;
        rec.extend_from_slice(&gap);
        let second_start = rec.len() as f64 / SR as f64;
        rec.extend(vowel(120.0));
        let second_end = rec.len() as f64 / SR as f64;

        let lines = vec![
            Line { speaker: "RAJU".into(), start: 0.0, end: first_end, emotion: Emotion::Neutral, intensity: 0.0 },
            Line { speaker: "GUDDU".into(), start: second_start, end: second_end, emotion: Emotion::Angry, intensity: 1.0 },
        ];
        let mut voices = BTreeMap::new();
        voices.insert("RAJU".to_string(), Preset::Woman.profile());
        let voiced = voice_dialogue(&rec, SR, &lines, &voices);

        assert_eq!(voiced.lines.len(), 2);
        let span = |p: &Placed| {
            let a = (p.start * SR as f64) as usize;
            let b = ((p.end * SR as f64) as usize).min(voiced.samples.len());
            voiced.samples[a..b].to_vec()
        };
        assert!(mean_pitch(&span(&voiced.lines[0])) > 160.0, "Raju's line was not re-voiced");
        let guddu = mean_pitch(&span(&voiced.lines[1]));
        assert!((guddu - 120.0).abs() < 6.0, "Guddu has no voice set and must stay himself: {guddu}");
        assert_eq!(voiced.lines[1].speaker, "GUDDU");
        assert!(voiced.lines[1].start >= voiced.lines[0].end, "lines must not overlap");
    }

    /// A slower voice makes its line longer, and every later line moves with
    /// it — the times handed back are where lip sync must go.
    #[test]
    fn a_slower_voice_moves_the_lines_after_it() {
        let mut rec = vowel(120.0);
        let first_end = rec.len() as f64 / SR as f64;
        rec.extend(vowel(120.0));
        let end = rec.len() as f64 / SR as f64;
        let lines = vec![
            Line { speaker: "A".into(), start: 0.0, end: first_end, emotion: Emotion::Neutral, intensity: 0.0 },
            Line { speaker: "B".into(), start: first_end, end, emotion: Emotion::Neutral, intensity: 0.0 },
        ];
        let mut voices = BTreeMap::new();
        voices.insert("A".to_string(), VoiceProfile { tempo: 0.8, ..VoiceProfile::default() });
        let voiced = voice_dialogue(&rec, SR, &lines, &voices);
        let a_len = voiced.lines[0].end - voiced.lines[0].start;
        assert!((a_len - first_end / 0.8).abs() < 0.03, "A's line should be 25% longer: {a_len}");
        assert!((voiced.lines[1].start - voiced.lines[0].end).abs() < 1e-6);
        // A moment after both lines moves by A's extra length; one inside A
        // stretches with it.
        let extra = voiced.lines[0].end - voiced.lines[0].from_end;
        assert!((voiced.map_time(end + 0.5) - (end + 0.5 + extra)).abs() < 0.02);
        assert!((voiced.map_time(first_end / 2.0) - a_len / 2.0).abs() < 0.02);
    }
}
