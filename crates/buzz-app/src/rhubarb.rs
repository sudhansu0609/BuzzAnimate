//! **Rhubarb Lip Sync** — the speech recogniser behind the Lip Sync window.
//!
//! Rhubarb (Daniel Wolf's open-source tool, the one the animator's Animate
//! `autoLipSyncImporter` shelf command runs) listens to a dialogue recording
//! and says which of its nine mouth shapes shows when. Its *phonetic*
//! recogniser hears sounds rather than English words, so it works on dialogue
//! in any language. That is the part the loudness analysis in `buzz_audio`
//! can only approximate: it tells an "m" from a "d" from an "s".
//!
//! The tool is a program of its own, kept in
//! `<config>/BuzzAnimate/tools/rhubarb/` with its `res/` speech models beside
//! it (or anywhere on `PATH`). The clip is written to a temporary WAV, Rhubarb
//! runs on it, and its JSON answer comes back as [`Cue`]s.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// One stretch of one mouth shape, in seconds from the clip's start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    /// Rhubarb's shape: `A`–`H`, or `X` for rest.
    pub shape: char,
}

/// The Rhubarb program to run: the app's own copy, else one on `PATH`.
pub fn binary() -> Option<PathBuf> {
    let own = buzz_doc::commands::commands_root()
        .parent()
        .map(|config| config.join("tools").join("rhubarb").join("rhubarb"))
        .filter(|p| p.is_file());
    own.or_else(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).map(|d| d.join("rhubarb")).find(|p| p.is_file())
        })
    })
}

/// **Which of Animate's twelve visemes** (`buzz_audio::ANIMATE_VISEMES`) a
/// Rhubarb shape is, by Rhubarb's own definitions: A closed (M, B, P), B
/// clenched teeth (most consonants, EE, IH), C open (EH, AE — the neutral
/// open "uh"), D wide (AA), E slightly rounded (AO, ER), F puckered (UW, OW,
/// W), G F/V, H L, X rest. Each lands on a viseme of its own, so a chart
/// that draws them differently shows them differently: C went to Ee once,
/// which most charts draw on the same teeth frame as S, and B/C speech then
/// held one frame for seconds.
pub fn animate_slot(shape: char) -> usize {
    match shape {
        'A' => 6,  // M
        'B' => 9,  // S
        'C' => 10, // Uh
        'D' => 1,  // Ah
        'E' => 7,  // Oh
        'F' => 11, // W-Oo
        'G' => 4,  // F
        'H' => 5,  // L
        _ => 0,    // X: Neutral
    }
}

/// Write `clip`'s mono mix as a 16-bit PCM WAV — what Rhubarb reads.
pub fn write_wav(clip: &buzz_audio::Clip, path: &Path) -> Result<()> {
    let mono = clip.mono();
    let rate = clip.sample_rate;
    let data_len = (mono.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for s in mono.iter() {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// **Listen to `clip` with Rhubarb**: its mouth cues, in order. Blocks for as
/// long as Rhubarb takes (about a fifth of the clip's length): run it off the
/// UI thread. `cancelled` is polled while it runs.
pub fn cues(clip: &buzz_audio::Clip, cancelled: &dyn Fn() -> bool) -> Result<Vec<Cue>> {
    let program = binary().context("Rhubarb is not installed (tools/rhubarb)")?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let wav = std::env::temp_dir().join(format!("buzz_rhubarb_{}_{stamp}.wav", std::process::id()));
    write_wav(clip, &wav)?;
    let mut child = std::process::Command::new(&program)
        .args(["-r", "phonetic", "-f", "json", "--extendedShapes", "GHX", "-q"])
        .arg(&wav)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", program.display()))?;
    // Read stdout on its own thread so a long answer cannot fill the pipe
    // while this one waits.
    let mut stdout = child.stdout.take().context("no output from Rhubarb")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
        text
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancelled() {
            let _ = child.kill();
            let _ = std::fs::remove_file(&wav);
            bail!("cancelled");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let text = reader.join().unwrap_or_default();
    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut e, &mut stderr);
    }
    let _ = std::fs::remove_file(&wav);
    if !status.success() {
        bail!("Rhubarb failed: {}", stderr.lines().last().unwrap_or("no message"));
    }
    parse(&text)
}

/// Rhubarb's JSON: `{"mouthCues": [{"start": 0.0, "end": 0.2, "value": "X"}, ...]}`.
pub fn parse(json: &str) -> Result<Vec<Cue>> {
    let value: serde_json::Value = serde_json::from_str(json).context("Rhubarb's answer is not JSON")?;
    let cues = value.get("mouthCues").and_then(|c| c.as_array()).context("Rhubarb's answer has no mouthCues")?;
    Ok(cues
        .iter()
        .filter_map(|c| {
            Some(Cue {
                start: c.get("start")?.as_f64()?,
                end: c.get("end")?.as_f64()?,
                shape: c.get("value")?.as_str()?.chars().next()?,
            })
        })
        .collect())
}

/// The keys the cues make at `fps`: `(frame from the clip's start, Animate
/// viseme slot)`, one per change of viseme. Two cues landing on one frame keep
/// the later one, which is the shape that holds.
pub fn keys(cues: &[Cue], fps: f64) -> Vec<(u32, usize)> {
    let mut out: Vec<(u32, usize)> = Vec::new();
    for cue in cues {
        let frame = (cue.start * fps).round().max(0.0) as u32;
        let slot = animate_slot(cue.shape);
        match out.last_mut() {
            Some(last) if last.0 == frame => last.1 = slot,
            _ => out.push((frame, slot)),
        }
    }
    out.dedup_by(|b, a| a.1 == b.1);
    out
}

/// [`keys`], with **true silence closed**: Rhubarb sometimes leaves a
/// consonant shape over a pause, so wherever the recording is all but silent
/// (under 3% of how loud it speaks) for three frames or more the mouth rests,
/// and opens again on the shape Rhubarb heard when the sound comes back.
/// `levels` is the clip's loudness per frame (`Clip::frame_levels`).
pub fn keys_closing_silence(cues: &[Cue], fps: f64, levels: &[f32]) -> Vec<(u32, usize)> {
    let heard = keys(cues, fps);
    if heard.is_empty() || levels.is_empty() {
        return heard;
    }
    let mut audible: Vec<f32> = levels.iter().copied().filter(|l| *l > 1e-4).collect();
    audible.sort_by(|a, b| a.total_cmp(b));
    let speaking = audible.get(((audible.len().max(1) - 1) as f64 * 0.9) as usize).copied().unwrap_or(0.0);
    let quiet = speaking * 0.03;
    // The slot showing on every frame, from the keys.
    let mut slots = vec![0usize; levels.len()];
    let mut k = 0;
    for (f, slot) in slots.iter_mut().enumerate() {
        while k + 1 < heard.len() && heard[k + 1].0 as usize <= f {
            k += 1;
        }
        *slot = if heard[k].0 as usize <= f { heard[k].1 } else { 0 };
    }
    let mut f = 0;
    while f < levels.len() {
        if levels[f] > quiet {
            f += 1;
            continue;
        }
        let start = f;
        while f < levels.len() && levels[f] <= quiet {
            f += 1;
        }
        if f - start >= 3 {
            for slot in &mut slots[start..f] {
                *slot = 0;
            }
        }
    }
    articulate(&mut slots, levels, speaking);
    let mut out: Vec<(u32, usize)> = Vec::new();
    for (f, slot) in slots.iter().enumerate() {
        if out.last().is_none_or(|(_, s)| s != slot) {
            out.push((f as u32, *slot));
        }
    }
    out
}

/// The longest a mouth holds over sound before its softest beat is used.
const LONGEST_SYLLABLE: usize = 6;

/// Cut `from..to` at its quietest inner frame while it is longer than
/// [`LONGEST_SYLLABLE`] and that frame is a dip at all; the cuts go into
/// `cuts` in order (not `to` itself).
fn split_long(from: usize, to: usize, level: &impl Fn(usize) -> f32, cuts: &mut Vec<usize>) {
    if to - from <= LONGEST_SYLLABLE {
        return;
    }
    let Some(dip) = (from + 3..to - 2).min_by(|a, b| level(*a).total_cmp(&level(*b))) else {
        return;
    };
    let left = (from..dip).map(level).fold(0.0f32, f32::max);
    let right = (dip + 1..to).map(level).fold(0.0f32, f32::max);
    if level(dip) >= 0.95 * left.min(right) {
        return;
    }
    split_long(from, dip, level, cuts);
    cuts.push(dip);
    split_long(dip, to, level, cuts);
}

/// Animate viseme slots ([`buzz_audio::ANIMATE_VISEMES`]) the syllables are
/// given: shapes most charts draw on frames of their own.
const AH: usize = 1;
const S: usize = 9;
const UH: usize = 10;

/// **One mouth per syllable, inside Rhubarb's long shapes.**
///
/// Rhubarb's recogniser is trained on English. On Hindi dialogue it hears
/// whole stretches as one shape — B, the teeth, for eight to sixteen frames
/// over three or four syllables — and the chart held one drawing while the
/// waveform rose and fell under it. So a shape held over sound is cut at the
/// syllable beats in the loudness (a frame quieter than the loudest three
/// either side of it, as `buzz_audio`'s own analysis finds them). The first
/// syllable keeps the shape Rhubarb heard; each one after it takes the mouth
/// its own loudness calls for — wide when loud, "uh" when middling, teeth
/// when quiet — and never the same as the syllable before, so every syllable
/// is a new mouth. A steady sound with no beat in it keeps its one shape.
fn articulate(slots: &mut [usize], levels: &[f32], speaking: f32) {
    let level = |f: usize| levels.get(f).copied().unwrap_or(0.0);
    let n = slots.len();
    let mut i = 0;
    while i < n {
        let shape = slots[i];
        let start = i;
        while i < n && slots[i] == shape {
            i += 1;
        }
        let end = i;
        if shape == 0 || end - start < 5 {
            continue;
        }

        let mut beats: Vec<usize> = Vec::new();
        for f in start + 1..end.saturating_sub(1) {
            let here = level(f);
            if here > level(f - 1) || here > level(f + 1) {
                continue;
            }
            let left = (f.saturating_sub(3).max(start)..f).map(level).fold(0.0f32, f32::max);
            let right = (f + 1..(f + 4).min(end)).map(level).fold(0.0f32, f32::max);
            if here < 0.85 * left.min(right) && f >= start + 2 && beats.last().is_none_or(|b| f >= b + 3) {
                beats.push(f);
            }
        }

        // A piece still longer than a syllable usually is has a softer beat
        // in it: split it at its quietest frame, when that is quieter at all
        // than the sound either side of it.
        let mut cuts: Vec<usize> = Vec::new();
        let mut from = start;
        for to in beats.into_iter().chain(std::iter::once(end)) {
            split_long(from, to, &level, &mut cuts);
            cuts.push(to);
            from = to;
        }

        let mut previous = shape;
        let mut from = start;
        for to in cuts {
            if from > start {
                let peak = (from..to).map(level).fold(0.0f32, f32::max) / speaking.max(f32::MIN_POSITIVE);
                let wanted = if peak >= 0.6 {
                    AH
                } else if peak >= 0.3 {
                    UH
                } else {
                    S
                };
                let syllable = match (wanted == previous, wanted) {
                    (false, _) => wanted,
                    (true, AH) => UH,
                    (true, UH) if peak >= 0.45 => AH,
                    (true, UH) => S,
                    (true, _) => UH,
                };
                slots[from..to].fill(syllable);
                previous = syllable;
            }
            from = to;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rhubarbs_answer_becomes_keys_on_frames() {
        let json = r#"{"metadata": {}, "mouthCues": [
            {"start": 0.00, "end": 0.30, "value": "X"},
            {"start": 0.30, "end": 0.38, "value": "B"},
            {"start": 0.38, "end": 0.40, "value": "D"},
            {"start": 0.40, "end": 0.60, "value": "C"},
            {"start": 0.60, "end": 0.70, "value": "C"},
            {"start": 0.70, "end": 0.90, "value": "A"}
        ]}"#;
        let cues = parse(json).expect("parses");
        assert_eq!(cues.len(), 6);
        // At 24 fps: X@0, B@7, D@9 then C@10 (D and C land on frames 9 and
        // 10), C again is no change, A@17.
        assert_eq!(keys(&cues, 24.0), vec![(0, 0), (7, 9), (9, 1), (10, 10), (17, 6)]);
    }

    #[test]
    fn a_silent_stretch_rests_whatever_rhubarb_said() {
        let cues = [Cue { start: 0.0, end: 1.0, shape: 'B' }];
        // 24 frames: loud, then 6 silent, then loud again.
        let mut levels = vec![0.3f32; 24];
        for l in &mut levels[8..14] {
            *l = 0.0;
        }
        let keys = keys_closing_silence(&cues, 24.0, &levels);
        assert_eq!(keys, vec![(0, 9), (8, 0), (14, 9)]);
    }

    /// **Every syllable is a new mouth.** Rhubarb heard one B (teeth) over
    /// four syllables of Hindi — a loud one, a middling one, a loud one and a
    /// quiet one, with a dip between each — and the chart held the teeth
    /// across all four. Each syllable now changes the mouth, by how loud it
    /// is; a steady sound with no beat in it keeps Rhubarb's one shape.
    #[test]
    fn a_shape_held_over_several_syllables_changes_with_each() {
        let cues = [Cue { start: 0.0, end: 1.0, shape: 'B' }];
        let syllable = |peak: f32| [0.1 * peak, 0.6 * peak, peak, 0.7 * peak, 0.3 * peak];
        let mut levels: Vec<f32> = Vec::new();
        for peak in [1.0, 0.5, 1.0, 0.25] {
            levels.extend(syllable(peak));
        }
        levels.extend([0.0; 4]);
        let keys = keys_closing_silence(&cues, 24.0, &levels);
        let speech: Vec<usize> = keys.iter().filter(|(f, _)| *f < 20).map(|(_, s)| *s).collect();
        assert!(speech.len() >= 4, "one mouth per syllable: {keys:?}");
        assert!(speech.windows(2).all(|w| w[0] != w[1]), "{keys:?}");
        assert_eq!(speech[0], 9, "the first syllable keeps what Rhubarb heard: {keys:?}");
        assert!(speech.contains(&AH) && speech.contains(&UH), "loud syllables open wide: {keys:?}");

        let steady = keys_closing_silence(&cues, 24.0, &[0.5f32; 24]);
        assert_eq!(steady, vec![(0, 9)], "no beat, no change");
    }

    #[test]
    fn a_wav_is_written_rhubarb_can_read() {
        let clip = buzz_audio::Clip::new("t", 16_000, 1, vec![0.25; 1600]).expect("a clip");
        let path = std::env::temp_dir().join(format!("buzz_rhubarb_test_{}.wav", std::process::id()));
        write_wav(&clip, &path).expect("written");
        let back = buzz_audio::Clip::open(&path).expect("reads back");
        assert_eq!(back.sample_rate, 16_000);
        assert_eq!(back.mono().len(), 1600);
        let _ = std::fs::remove_file(&path);
    }
}
