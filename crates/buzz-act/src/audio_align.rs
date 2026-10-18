//! **A recording in, the script's lines found in it.**
//!
//! [`crate::script_timing`] times a scene from its words because there is no
//! clock to read. Once the dialogue is recorded there is one, and this module
//! reads it: the recording is cut into utterances (stretches of speech between
//! breaths, [`utterances`]), the utterances are matched to the script's lines in
//! order ([`align`]), and each scene gets a span of the recording
//! ([`AudioPlan::scene_spans`]) its soundtrack is cut from.
//!
//! # The alignment
//!
//! A monotone dynamic programme between the utterances and the lines. One
//! utterance may hold several consecutive lines (two speakers with no breath
//! between them): it is split by word share. Several utterances may be one line
//! (a speaker pausing mid-line): they are merged. An utterance may be skipped (a
//! cough, a door), and so may a line (a line the recording does not have — it
//! keeps its text timing).
//!
//! Without a transcript the cost of a match is `|ln(heard s / expected s)|`,
//! expected being [`Timing::dialogue_seconds`] scaled by the recording's own
//! pace (the sum of what was heard over the sum of what was expected), so a
//! slow reader is not a mismatch on every line. With a transcript ([`Heard`]
//! segments, converted by the caller from Whisper's), each line also pays for
//! the share of its words the transcript did not hear over its span — token
//! similarity. There is no language model here: `buzz-ai` depends on this
//! crate, so it cannot be asked from inside it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use buzz_audio::{Clip, PhraseOptions, detect_phrases};

use crate::screenplay::{Dialogue, Item, ScriptScene};
use crate::script_timing::{Schedule, Slot, Timing, word_count};

/// One spoken line of the script, as the aligner sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpokenRef {
    pub scene: u32,
    /// Index into [`ScriptScene::items`].
    pub item: usize,
    pub words: usize,
    /// The words, for the expected length and the transcript match.
    pub text: String,
}

/// Where one line was found in the recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioLine {
    pub scene: u32,
    pub item: usize,
    /// Seconds in the recording.
    pub start: f64,
    pub end: f64,
}

/// A stretch of transcript: what speech-to-text heard, and when.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The recording, matched to the script.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioPlan {
    /// The recording (or, for one file per scene, the folder).
    pub path: PathBuf,
    pub lines: Vec<AudioLine>,
    /// Per scene: first line start − lead-in … last line end + tail, in seconds
    /// of that scene's recording, never overlapping the next scene's.
    pub scene_spans: BTreeMap<u32, (f64, f64)>,
    /// A scene with a recording of its own (`--audio-dir`); every other scene
    /// is in [`Self::path`].
    pub scene_paths: BTreeMap<u32, PathBuf>,
    /// What was done, for the report: which path aligned, which lines have no
    /// audio.
    pub notes: Vec<String>,
}

impl AudioPlan {
    /// The file a scene's span is cut from.
    pub fn path_for(&self, scene: u32) -> &Path {
        self.scene_paths.get(&scene).map(PathBuf::as_path).unwrap_or(self.path.as_path())
    }

    /// The lines found for one scene, in order.
    pub fn scene_lines(&self, scene: u32) -> Vec<AudioLine> {
        self.lines.iter().filter(|l| l.scene == scene).copied().collect()
    }

    /// Fold a plan for other scenes (another file) into this one.
    pub fn merge(&mut self, other: AudioPlan) {
        for (scene, span) in other.scene_spans {
            self.scene_paths.insert(scene, other.path.clone());
            self.scene_spans.insert(scene, span);
        }
        self.lines.extend(other.lines);
        self.notes.extend(other.notes);
    }
}

/// Seconds of silence before the first line a scene's span reaches back for.
const LEAD_IN: f64 = 0.5;
/// Seconds after the last line a scene's span runs on for.
const TAIL: f64 = 0.8;
/// The rate utterances are measured at: centiseconds.
const UTT_FPS: f64 = 100.0;

/// **The utterances in a recording**, in seconds: [`detect_phrases`], gaps
/// under 0.25 s merged, anything under 0.3 s dropped.
///
/// The silence floor is half [`PhraseOptions`]' default: at the default a
/// one-word line ("Munna.", 0.35 s) loses its soft edges, measures under
/// 0.3 s and is dropped.
pub fn utterances(clip: &Clip) -> Vec<(f64, f64)> {
    let options = PhraseOptions {
        floor: 0.06,
        gap: 0.25,
        shortest: 0.05,
    };
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for p in detect_phrases(clip, UTT_FPS, &options) {
        let (s, e) = (p.start as f64 / UTT_FPS, p.end as f64 / UTT_FPS);
        match merged.last_mut() {
            Some(last) if s - last.1 < 0.25 => last.1 = e,
            _ => merged.push((s, e)),
        }
    }
    merged.retain(|(s, e)| e - s >= 0.3);
    merged
}

/// The spoken lines of a scene, in order.
pub fn spoken_refs(scene: &ScriptScene) -> Vec<SpokenRef> {
    scene
        .items
        .iter()
        .enumerate()
        .filter_map(|(item, it)| match it {
            Item::Dialogue(Dialogue { text, .. }) => Some(SpokenRef {
                scene: scene.number,
                item,
                words: word_count(text),
                text: text.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The share of `line`'s words the transcript heard between `s` and `e`.
fn similarity(line: &[String], heard: &[Heard], s: f64, e: f64) -> f64 {
    if line.is_empty() {
        return 0.0;
    }
    let mut words: Vec<String> = Vec::new();
    for h in heard {
        let overlap = h.end.min(e) - h.start.max(s);
        let len = (h.end - h.start).max(1e-6);
        if overlap > 0.0 && overlap / len >= 0.3 {
            words.extend(tokens(&h.text));
        }
    }
    let hit = line.iter().filter(|w| words.contains(w)).count();
    hit as f64 / line.len() as f64
}

/// Cost of an utterance that is no line's.
const SKIP_UTT: f64 = 1.2;
/// Cost of a line the recording does not have.
const SKIP_LINE: f64 = 2.0;
/// Per extra line packed into one utterance.
const PACKED: f64 = 0.35;
/// Per extra utterance merged into one line.
const MERGED: f64 = 0.05;
/// Weight of the transcript's miss share, per line.
const HEARD_W: f64 = 0.6;
const MAX_UTTS: usize = 8;
const MAX_LINES: usize = 4;

#[derive(Clone, Copy)]
enum Step {
    SkipUtt,
    SkipLine,
    /// One line over `k` utterances.
    Merge(usize),
    /// `k` lines in one utterance.
    Pack(usize),
}

/// **Match the lines to the utterances**, in order. Lines with no audio are
/// left out of the result.
pub fn align(lines: &[SpokenRef], utts: &[(f64, f64)], transcript: Option<&[Heard]>) -> Vec<AudioLine> {
    let (n, m) = (lines.len(), utts.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let timing = Timing::default();
    let raw: Vec<f64> = lines
        .iter()
        .map(|l| {
            if l.text.trim().is_empty() {
                (l.words as f64 * timing.per_word).max(timing.min_line)
            } else {
                timing.dialogue_seconds(&l.text)
            }
        })
        .collect();
    // The recording's own pace.
    let heard_total: f64 = utts.iter().map(|(s, e)| e - s).sum();
    let expected_total: f64 = raw.iter().sum();
    let pace = (heard_total / expected_total.max(1e-6)).clamp(0.4, 2.5);
    let expected: Vec<f64> = raw.iter().map(|x| x * pace).collect();
    let toks: Vec<Vec<String>> = lines.iter().map(|l| tokens(&l.text)).collect();
    let heard_cost = |line: usize, s: f64, e: f64| -> f64 {
        match transcript {
            Some(h) => HEARD_W * (1.0 - similarity(&toks[line], h, s, e)),
            None => 0.0,
        }
    };
    let ratio = |seconds: f64, want: f64| (seconds.max(0.05) / want.max(0.05)).ln().abs();
    // Split one utterance across lines by word share.
    let split = |first: usize, k: usize, (s, e): (f64, f64)| -> Vec<(f64, f64)> {
        let weights: Vec<f64> = (first..first + k).map(|i| lines[i].words.max(1) as f64).collect();
        let sum: f64 = weights.iter().sum();
        let mut at = s;
        weights
            .iter()
            .map(|w| {
                let len = (e - s) * w / sum;
                let piece = (at, at + len);
                at += len;
                piece
            })
            .collect()
    };

    let inf = f64::INFINITY;
    let mut cost = vec![vec![inf; m + 1]; n + 1];
    let mut back: Vec<Vec<Option<Step>>> = vec![vec![None; m + 1]; n + 1];
    cost[0][0] = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            let here = cost[i][j];
            if !here.is_finite() {
                continue;
            }
            let mut relax = |ni: usize, nj: usize, c: f64, step: Step| {
                if here + c < cost[ni][nj] {
                    cost[ni][nj] = here + c;
                    back[ni][nj] = Some(step);
                }
            };
            if j < m {
                relax(i, j + 1, SKIP_UTT, Step::SkipUtt);
            }
            if i < n {
                let missed = if transcript.is_some() { HEARD_W } else { 0.0 };
                relax(i + 1, j, SKIP_LINE + missed, Step::SkipLine);
            }
            if i < n {
                for k in 1..=MAX_UTTS.min(m - j) {
                    let (s, e) = (utts[j].0, utts[j + k - 1].1);
                    let c = ratio(e - s, expected[i]) + MERGED * (k - 1) as f64 + heard_cost(i, s, e);
                    relax(i + 1, j + k, c, Step::Merge(k));
                }
            }
            if j < m {
                for k in 2..=MAX_LINES.min(n - i) {
                    let want: f64 = expected[i..i + k].iter().sum();
                    let pieces = split(i, k, utts[j]);
                    let mut c = ratio(utts[j].1 - utts[j].0, want) + PACKED * (k - 1) as f64;
                    for (o, (s, e)) in pieces.iter().enumerate() {
                        c += heard_cost(i + o, *s, *e);
                    }
                    relax(i + k, j + 1, c, Step::Pack(k));
                }
            }
        }
    }

    let mut out: Vec<AudioLine> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let Some(step) = back[i][j] else { break };
        match step {
            Step::SkipUtt => j -= 1,
            Step::SkipLine => i -= 1,
            Step::Merge(k) => {
                let line = &lines[i - 1];
                out.push(AudioLine {
                    scene: line.scene,
                    item: line.item,
                    start: utts[j - k].0,
                    end: utts[j - 1].1,
                });
                i -= 1;
                j -= k;
            }
            Step::Pack(k) => {
                let pieces = split(i - k, k, utts[j - 1]);
                for (o, (s, e)) in pieces.into_iter().enumerate().rev() {
                    let line = &lines[i - k + o];
                    out.push(AudioLine {
                        scene: line.scene,
                        item: line.item,
                        start: s,
                        end: e,
                    });
                }
                i -= k;
                j -= 1;
            }
        }
    }
    out.reverse();
    out
}

/// **Plan one recording** that holds `scenes`, in order.
pub fn plan(scenes: &[&ScriptScene], path: &Path, clip: &Clip, transcript: Option<&[Heard]>) -> AudioPlan {
    let refs: Vec<SpokenRef> = scenes.iter().flat_map(|s| spoken_refs(s)).collect();
    let utts = utterances(clip);
    let lines = align(&refs, &utts, transcript);
    let duration = clip.duration_seconds();

    let mut notes = vec![format!(
        "{}: {} utterance(s), {} of {} line(s) aligned by {}",
        path.display(),
        utts.len(),
        lines.len(),
        refs.len(),
        if transcript.is_some() {
            "duration DP + transcript token similarity"
        } else {
            "duration DP"
        }
    )];
    for r in &refs {
        if !lines.iter().any(|l| l.scene == r.scene && l.item == r.item) {
            notes.push(format!("scene {} item {}: no audio, text timing", r.scene, r.item));
        }
    }

    // Spans: lead-in and tail, then any overlap with the next scene split at
    // the middle of the silence between them.
    let mut spans: Vec<(u32, f64, f64, f64, f64)> = Vec::new(); // scene, span s, e, first, last
    for scene in scenes {
        let mine: Vec<&AudioLine> = lines.iter().filter(|l| l.scene == scene.number).collect();
        let (Some(first), Some(last)) = (
            mine.iter().map(|l| l.start).reduce(f64::min),
            mine.iter().map(|l| l.end).reduce(f64::max),
        ) else {
            continue;
        };
        spans.push((
            scene.number,
            (first - LEAD_IN).max(0.0),
            (last + TAIL).min(duration),
            first,
            last,
        ));
    }
    for k in 1..spans.len() {
        let (prev_end, this_start) = (spans[k - 1].2, spans[k].1);
        if prev_end > this_start {
            let mid = (spans[k - 1].4 + spans[k].3) / 2.0;
            spans[k - 1].2 = mid.max(spans[k - 1].4);
            spans[k].1 = mid.min(spans[k].3);
        }
    }
    AudioPlan {
        path: path.to_path_buf(),
        lines,
        scene_spans: spans.into_iter().map(|(n, s, e, _, _)| (n, (s, e))).collect(),
        scene_paths: BTreeMap::new(),
        notes,
    }
}

/// A scene laid out on its recording.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSchedule {
    pub schedule: Schedule,
    /// Seconds of silence put before the span, so the action before the first
    /// line keeps its text length.
    pub pre_roll: f64,
    /// Seconds of silence after it, for the action after the last line.
    pub post_roll: f64,
    /// The span cut from the recording.
    pub span: (f64, f64),
    /// Items timed by the recording.
    pub from_audio: Vec<usize>,
    /// Dialogue items timed by their text.
    pub from_text: Vec<usize>,
}

/// **The schedule of a scene from its recording**: the lines where they were
/// heard (scene frame 0 = span start − pre-roll), everything else at its text
/// length, squeezed into the time between lines when it does not fit. `None`
/// when the plan has no lines for the scene.
pub fn schedule_scene(timing: &Timing, scene: &ScriptScene, plan: &AudioPlan) -> Option<AudioSchedule> {
    let span = *plan.scene_spans.get(&scene.number)?;
    let anchors: BTreeMap<usize, (f64, f64)> = plan
        .lines
        .iter()
        .filter(|l| l.scene == scene.number && l.item < scene.items.len())
        .map(|l| (l.item, (l.start - span.0, l.end - span.0)))
        .collect();
    let first = *anchors.keys().next()?;
    let last = *anchors.keys().next_back()?;
    let fps = timing.fps;
    let advance = |item: &Item| timing.item_seconds(item);
    let length = |item: &Item| match item {
        Item::Dialogue(Dialogue { text, .. }) => timing.dialogue_seconds(text),
        other => timing.item_seconds(other),
    };

    let need_before: f64 = scene.items[..first].iter().map(advance).sum();
    let need_after: f64 = scene.items[last + 1..].iter().map(advance).sum();
    let span_len = span.1 - span.0;
    let pre_roll = (need_before - anchors[&first].0).max(0.0);
    let post_roll = (need_after - (span_len - anchors[&last].1)).max(0.0);
    let total_seconds = pre_roll + span_len + post_roll;

    let mut slots: Vec<Slot> = Vec::with_capacity(scene.items.len());
    let mut pending: Vec<usize> = Vec::new();
    let mut cursor = 0.0f64;
    let lay = |slots: &mut Vec<Slot>, pending: &mut Vec<usize>, from: f64, to: f64| {
        let sum: f64 = pending.iter().map(|&i| advance(&scene.items[i])).sum();
        let room = (to - from).max(0.0);
        let squeeze = if sum > room && sum > 0.0 { room / sum } else { 1.0 };
        let mut clock = from;
        for &i in pending.iter() {
            let item = &scene.items[i];
            slots.push(Slot {
                item: i,
                start: (clock * fps).round() as u32,
                frames: timing.frames(length(item) * squeeze),
            });
            clock += advance(item) * squeeze;
        }
        pending.clear();
    };
    let mut from_audio = Vec::new();
    let mut from_text = Vec::new();
    for (index, item) in scene.items.iter().enumerate() {
        match anchors.get(&index) {
            Some(&(s, e)) => {
                let (s, e) = (s + pre_roll, e + pre_roll);
                lay(&mut slots, &mut pending, cursor, s);
                let start = (s * fps).round() as u32;
                let end = ((e * fps).round() as u32).max(start + 1);
                slots.push(Slot {
                    item: index,
                    start,
                    frames: end - start,
                });
                cursor = e;
                from_audio.push(index);
            }
            None => {
                if matches!(item, Item::Dialogue(_)) {
                    from_text.push(index);
                }
                pending.push(index);
            }
        }
    }
    lay(&mut slots, &mut pending, cursor, total_seconds);

    let total = (total_seconds * fps).round() as u32;
    let last_end = slots.iter().map(|s| s.start + s.frames).max().unwrap_or(0);
    Some(AudioSchedule {
        schedule: Schedule {
            slots,
            total_frames: total.max(last_end).max(1),
        },
        pre_roll,
        post_roll,
        span,
        from_audio,
        from_text,
    })
}

/// `clip` between `span` seconds, with `pre` and `post` seconds of silence
/// either side. `None` when nothing is left.
pub fn cut(clip: &Clip, span: (f64, f64), pre: f64, post: f64) -> Option<Clip> {
    let piece = clip.trimmed(span.0 as f32, Some(span.1 as f32));
    let channels = clip.channels.max(1) as usize;
    let pad = |seconds: f64| (seconds.max(0.0) * clip.sample_rate as f64).round() as usize * channels;
    let mut samples = vec![0.0f32; pad(pre)];
    samples.extend_from_slice(&piece.samples);
    samples.extend(std::iter::repeat_n(0.0f32, pad(post)));
    Clip::new(&clip.name, clip.sample_rate, clip.channels, samples).ok()
}

/// A clip as a 16-bit PCM WAV file, in memory.
pub fn wav_bytes(clip: &Clip) -> Vec<u8> {
    let channels = clip.channels.max(1);
    let data_len = (clip.samples.len() * 2) as u32;
    let byte_rate = clip.sample_rate * channels as u32 * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&clip.sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &clip.samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}
