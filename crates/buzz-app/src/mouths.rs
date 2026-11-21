//! **The ear lip sync hears the dialogue with** (user 2026-11-21: "how does
//! it identify the sound and place the mouth shapes? Animate does it much
//! better than what we have").
//!
//! Every lip sync — the Lip Sync window, Shift+A's, the talk ranges', a
//! produced episode's — asks [`heard_keys`] for the mouth on each frame. It
//! listens with the **speech-sound model** (`buzz_ai::phonemes`, wav2vec2
//! XLSR-53, any language) when it is in the models folder, and with
//! **Rhubarb** otherwise. The model hears each sound — `m`, `aː`, `ʃ` — and the
//! sound is the viseme, as Animate's own Auto Lip-Sync hears it, where
//! Rhubarb (English only) heard Hindi as a few long shapes the loudness had
//! to break up.
//!
//! The answer is `(frame from the clip's start, Animate viseme slot)` keys,
//! one per change of mouth, in `buzz_audio::ANIMATE_VISEMES` order.

use anyhow::Result;

/// Animate viseme slots used here.
const NEUTRAL: usize = 0;
const D: usize = 2;
const F: usize = 4;
const L: usize = 5;
const M: usize = 6;
const R: usize = 8;
const S: usize = 9;

/// The tongue's consonants: barely seen on the lips, the first to give way.
fn tongue(slot: usize) -> bool {
    matches!(slot, D | L | R | S)
}

/// How many frames a mouth shape leads its sound by. The model marks a sound
/// about a frame after it starts, and a mouth read *with* its sound looks
/// late: animators (and Animate's Auto Lip-Sync) set it a frame early.
const LEAD_FRAMES: f64 = 1.0;

/// The longest one speech sound holds the mouth when nothing follows it
/// soon: a vowel at the end of a word, not the pause after it.
const LONGEST_SOUND_SECONDS: f64 = 0.35;

/// Whether lip sync can hear dialogue here: the model or Rhubarb.
pub fn available() -> bool {
    buzz_ai::phonemes::available() || crate::rhubarb::binary().is_some()
}

/// What lip sync listens with, for the status line.
pub fn ear() -> &'static str {
    if buzz_ai::phonemes::available() {
        "the speech-sound model"
    } else {
        "Rhubarb"
    }
}

/// **The mouth keys for `clip`** at `fps`: heard by the speech-sound model
/// when it is installed (Rhubarb if the model fails), else by Rhubarb.
/// Blocks while it listens: run it off the UI thread.
pub fn heard_keys(clip: &buzz_audio::Clip, fps: f64, cancelled: &dyn Fn() -> bool) -> Result<Vec<(u32, usize)>> {
    let levels = clip.frame_levels(fps);
    if buzz_ai::phonemes::available() {
        let samples = clip.resampled_mono(buzz_ai::phonemes::SAMPLE_RATE);
        match buzz_ai::phonemes::hear(&samples, cancelled) {
            Ok(phones) => return Ok(keys_from_phones(&phones, fps, &levels)),
            Err(e) if cancelled() => return Err(e),
            Err(e) if crate::rhubarb::binary().is_none() => return Err(e),
            Err(e) => tracing::warn!("the speech-sound model failed, Rhubarb listens instead: {e:#}"),
        }
    }
    let cues = crate::rhubarb::cues(clip, cancelled)?;
    Ok(crate::rhubarb::keys_closing_silence(&cues, fps, &levels))
}

/// **Speech sounds to mouth keys**, as an animator keys them:
///
/// - each sound shows its viseme from the frame it starts until the next
///   sound starts (no longer than [`LONGEST_SOUND_SECONDS`]), and a sound
///   with no shape of its own (`h`, a glottal stop) takes the next one's;
/// - true silence — under 3% of how loud the line is spoken, three frames or
///   more — closes the mouth to Neutral;
/// - each shape leads its sound by [`LEAD_FRAMES`], as an animator sets it;
/// - a tongue consonant (D, S, L, R) held one frame gives way to the vowel
///   beside it — never to the silence, which would keep the mouth shut while
///   the line has begun. Vowels and the lip closures (M, B, P and F, V) stay
///   even on one frame: they are what makes a line look spoken.
///
/// `levels` is the clip's loudness per frame (`Clip::frame_levels`), which
/// also sets how many frames there are.
pub fn keys_from_phones(phones: &[buzz_ai::phonemes::Phone], fps: f64, levels: &[f32]) -> Vec<(u32, usize)> {
    let frames = levels.len().max(phones.last().map_or(0, |p| (p.end * fps).ceil() as usize));
    if frames == 0 {
        return Vec::new();
    }
    let mut slots = vec![NEUTRAL; frames];
    // Each sound's viseme: its own, else the next sound's that has one.
    let visemes: Vec<Option<usize>> = phones.iter().map(|p| buzz_ai::phonemes::animate_viseme(&p.ipa)).collect();
    for (i, phone) in phones.iter().enumerate() {
        let Some(viseme) = visemes[i..].iter().flatten().next().copied() else { continue };
        let lead = LEAD_FRAMES / fps;
        let from = (((phone.start - lead) * fps).round().max(0.0) as usize).min(frames - 1);
        let next = phones.get(i + 1).map_or(f64::INFINITY, |n| n.start - lead);
        let until = next.min(phone.start - lead + LONGEST_SOUND_SECONDS).max(phone.end - lead);
        let to = ((until * fps).round() as usize).clamp(from + 1, frames);
        for slot in &mut slots[from..to] {
            *slot = viseme;
        }
    }
    // **Every lip closure shows**, painted last: an "m" heard on the same
    // frame as the vowel after it ("ma") would otherwise be painted over, and
    // the lips that close on M, B, P (and F, V) are what reads as speech.
    let lead = LEAD_FRAMES / fps;
    let closures: Vec<usize> = phones
        .iter()
        .zip(&visemes)
        .filter(|(_, v)| matches!(v, Some(M) | Some(F)))
        .map(|(p, _)| (((p.start - lead) * fps).round().max(0.0) as usize).min(frames - 1))
        .collect();
    for (phone, viseme) in phones.iter().zip(&visemes) {
        if let Some(v @ (M | F)) = viseme {
            let at = (((phone.start - lead) * fps).round().max(0.0) as usize).min(frames - 1);
            slots[at] = *v;
            // The vowel heard with it moves on a frame, when it had that one.
            if at + 1 < frames && slots[at + 1] == NEUTRAL && !closures.contains(&(at + 1)) {
                slots[at + 1] = slots.get(at + 2).copied().unwrap_or(NEUTRAL);
            }
        }
    }

    // True silence closes the mouth.
    let mut audible: Vec<f32> = levels.iter().copied().filter(|l| *l > 1e-4).collect();
    audible.sort_by(|a, b| a.total_cmp(b));
    let speaking = audible.get(((audible.len().max(1) - 1) as f64 * 0.9) as usize).copied().unwrap_or(0.0);
    let quiet = speaking * 0.03;
    let mut f = 0;
    while f < levels.len().min(frames) {
        if levels[f] > quiet {
            f += 1;
            continue;
        }
        let start = f;
        while f < levels.len().min(frames) && levels[f] <= quiet {
            f += 1;
        }
        if f - start >= 3 {
            slots[start..f].fill(NEUTRAL);
        }
    }

    // No one-frame flicker, but the lip closures stay.
    let mut runs: Vec<(usize, usize)> = Vec::new(); // (slot, length)
    for slot in slots {
        match runs.last_mut() {
            Some((s, n)) if *s == slot => *n += 1,
            _ => runs.push((slot, 1)),
        }
    }
    let mut i = 0;
    while i < runs.len() {
        let (slot, n) = runs[i];
        if n < 2 && tongue(slot) && runs.len() > 1 {
            // Given to a neighbour that is spoken: a vowel or lip shape first,
            // the silence never while there is another.
            let before = i.checked_sub(1).map(|b| runs[b].0);
            let after = runs.get(i + 1).map(|r| r.0);
            let rank = |s: Option<usize>| match s {
                None => 0,
                Some(NEUTRAL) => 1,
                Some(s) if tongue(s) => 2,
                Some(_) => 3,
            };
            let to_before = rank(before) >= rank(after);
            if to_before {
                runs[i - 1].1 += n;
            } else {
                runs[i + 1].1 += n;
            }
            runs.remove(i);
            // Two runs of one shape now side by side are one.
            if i > 0 && i < runs.len() && runs[i - 1].0 == runs[i].0 {
                runs[i - 1].1 += runs[i].1;
                runs.remove(i);
            }
            i = i.saturating_sub(1);
            continue;
        }
        i += 1;
    }
    let mut out = Vec::new();
    let mut at = 0u32;
    for (slot, n) in runs {
        if out.last().is_none_or(|(_, s): &(u32, usize)| *s != slot) {
            out.push((at, slot));
        }
        at += n as u32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_ai::phonemes::Phone;

    fn phone(start: f64, end: f64, ipa: &str) -> Phone {
        Phone { start, end, ipa: ipa.into() }
    }

    /// "maa" then a pause then "bas": M, Ah, a closed mouth over the pause,
    /// then M (b) and S — each a frame before its sound. At 24 fps.
    #[test]
    fn sounds_become_visemes_with_silence_closed() {
        let fps = 24.0;
        let mut levels = vec![0.5f32; 48];
        for l in &mut levels[14..30] {
            *l = 0.0;
        }
        let phones = [phone(0.0, 0.04, "m"), phone(0.08, 0.12, "aː"), phone(1.25, 1.29, "b"), phone(1.33, 1.37, "a"), phone(1.5, 1.54, "s")];
        let keys = keys_from_phones(&phones, fps, &levels);
        let names: Vec<(u32, &str)> = keys.iter().map(|(f, s)| (*f, buzz_audio::ANIMATE_VISEMES[*s])).collect();
        assert_eq!(names, vec![(0, "M"), (1, "Ah"), (9, "Neutral"), (30, "M"), (31, "Ah"), (35, "S"), (43, "Neutral")]);
    }

    /// A one-frame tongue consonant gives way to the vowel beside it; a
    /// one-frame M and a one-frame vowel stay.
    #[test]
    fn one_frame_tongue_sounds_go_but_lips_and_vowels_stay() {
        let fps = 24.0;
        let levels = vec![0.5f32; 30];
        // Ah, one frame of t, Ee one frame, Ah, one frame of m, Ah.
        let phones = [
            phone(0.0, 0.2, "a"), phone(0.25, 0.29, "t"), phone(0.29, 0.33, "i"), phone(0.34, 0.5, "a"),
            phone(0.55, 0.59, "m"), phone(0.6, 0.9, "a"),
        ];
        let keys = keys_from_phones(&phones, fps, &levels);
        let names: Vec<&str> = keys.iter().map(|(_, s)| buzz_audio::ANIMATE_VISEMES[*s]).collect();
        assert!(!names.contains(&"D"), "the one-frame t goes: {names:?}");
        assert!(names.contains(&"Ee") && names.contains(&"M"), "the vowel and the M stay: {names:?}");
    }

    /// **A phrase opens on its first sounds**: quick consonants after a
    /// silence go to the vowel after them, not to the closed mouth before.
    #[test]
    fn a_phrase_does_not_stay_shut_over_its_first_consonants() {
        let fps = 24.0;
        let mut levels = vec![0.5f32; 30];
        for l in &mut levels[..10] {
            *l = 0.0;
        }
        let phones = [phone(0.46, 0.5, "s"), phone(0.5, 0.54, "k"), phone(0.54, 0.7, "a")];
        let keys = keys_from_phones(&phones, fps, &levels);
        let open = keys.iter().find(|(_, s)| *s != NEUTRAL).map(|(f, _)| *f);
        assert_eq!(open, Some(10), "open from the first loud frame: {keys:?}");
    }

    /// "ma" heard on one frame: the M shows, then the Ah.
    #[test]
    fn an_m_heard_with_its_vowel_still_closes_the_lips() {
        let levels = vec![0.5f32; 24];
        let keys = keys_from_phones(&[phone(0.0, 0.3, "a"), phone(0.4, 0.42, "m"), phone(0.41, 0.7, "a")], 24.0, &levels);
        let names: Vec<&str> = keys.iter().map(|(_, s)| buzz_audio::ANIMATE_VISEMES[*s]).collect();
        assert!(names.windows(2).any(|w| w == ["M", "Ah"]), "{names:?}");
    }

    /// A sound heard on (or past) the last frame does not overrun the clip.
    #[test]
    fn a_sound_at_the_very_end_stays_inside() {
        let levels = vec![0.5f32; 10];
        let keys = keys_from_phones(&[phone(0.0, 0.1, "a"), phone(0.41, 0.45, "m"), phone(0.9, 0.95, "o")], 24.0, &levels);
        assert!(keys.iter().all(|(f, _)| *f < 24), "{keys:?}");
    }

    /// h takes the vowel after it, as the mouth already shapes it.
    #[test]
    fn h_takes_the_next_shape() {
        let levels = vec![0.5f32; 12];
        let keys = keys_from_phones(&[phone(0.0, 0.04, "h"), phone(0.2, 0.3, "oː")], 24.0, &levels);
        assert_eq!(keys.first().map(|(f, s)| (*f, buzz_audio::ANIMATE_VISEMES[*s])), Some((0, "Oh")));
    }
}
