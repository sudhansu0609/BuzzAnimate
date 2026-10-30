//! **Audio parts** (ANIMATE_COPILOT_PLAN B3): the creator's ~3-minute audio
//! parts (a folder, in order) and/or the full episode file, aligned to the
//! screenplay's spoken lines so a part can be produced alone for a quick
//! preview, and the full episode from the full file.
//!
//! * parts only: the full episode is the parts joined in order
//!   (`audio/full.wav`); line times are episode times (offset by the parts before);
//! * full only: the file is split into ~3-minute working parts at silences
//!   near each 3-minute mark (`audio/parts/part_NN.wav`);
//! * both: parts give `parts.json` and `--part N`; the full file gives the
//!   whole episode.
//!
//! The matcher is a greedy in-order word matcher with a window: Roman and
//! Devanagari words are normalised to one spelling and compared with a small
//! edit tolerance (Hinglish is spelled every way). Joining and splitting are
//! done in-process (the clips are already decoded for the transcript).

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use buzz_act::audio_align::{self, AudioLine, AudioPlan, Heard, SpokenRef};
use buzz_act::screenplay::ScriptScene;
use buzz_audio::Clip;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Target length of a working part, seconds.
pub const PART_TARGET_S: f64 = 180.0;

// ------------------------------------------------------------------ names

/// "part2" < "part10"; "01" < "02"; case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (mut p, mut q) = (String::new(), String::new());
                while let Some(&c) = x.peek().filter(|c| c.is_ascii_digit()) {
                    p.push(c);
                    x.next();
                }
                while let Some(&d) = y.peek().filter(|d| d.is_ascii_digit()) {
                    q.push(d);
                    y.next();
                }
                let (p, q) = (p.trim_start_matches('0'), q.trim_start_matches('0'));
                let o = p.len().cmp(&q.len()).then_with(|| p.cmp(q));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// The wav / mp3 / m4a files of a folder, naturally sorted.
pub fn list_parts(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| {
                    let e = e.to_string_lossy().to_lowercase();
                    e == "wav" || e == "mp3" || e == "m4a"
                })
        })
        .collect();
    files.sort_by(|a, b| {
        natural_cmp(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy())
    });
    Ok(files)
}

// ------------------------------------------------------------------ words

fn deva_to_roman(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let cons = match c {
            'क' | 'ख' => "k",
            'ग' | 'घ' => "g",
            'च' | 'छ' => "c",
            'ज' | 'झ' => "j",
            'ङ' | 'ञ' | 'ण' | 'न' => "n",
            'ट' | 'ठ' | 'त' | 'थ' => "t",
            'ड' | 'ढ' | 'द' | 'ध' => "d",
            'प' => "p",
            'फ' => "f",
            'ब' | 'भ' => "b",
            'म' => "m",
            'य' => "y",
            'र' => "r",
            'ल' => "l",
            'व' => "v",
            'श' | 'ष' | 'स' => "s",
            'ह' => "h",
            _ => "",
        };
        if !cons.is_empty() {
            out.push_str(cons);
            // The inherent "a" unless a vowel sign or a virama follows.
            let next = chars.get(i + 1).copied().unwrap_or(' ');
            let followed = matches!(next, '\u{093E}'..='\u{094D}' | '\u{0962}' | '\u{0963}');
            let skip_nukta = next == '\u{093C}';
            if !followed && !skip_nukta {
                out.push('a');
            }
            continue;
        }
        match c {
            'अ' | 'आ' | 'ा' => out.push('a'),
            'इ' | 'ई' | 'ि' | 'ी' => out.push('i'),
            'उ' | 'ऊ' | 'ु' | 'ू' => out.push('u'),
            'ए' | 'ऐ' | 'े' | 'ै' => out.push('e'),
            'ओ' | 'औ' | 'ो' | 'ौ' => out.push('o'),
            'ऋ' | 'ृ' => out.push_str("ri"),
            'ं' | 'ँ' => out.push('n'),
            '\u{093C}' => {
                // Nukta: the base consonant already wrote its "a"; nothing.
            }
            c if c.is_ascii_alphanumeric() => out.push(c.to_ascii_lowercase()),
            _ => {}
        }
    }
    out
}

/// One spelling for a word, whichever script it came in.
pub fn norm_word(w: &str) -> String {
    let lower = w.to_lowercase();
    let mut s = if lower.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) { deva_to_roman(&lower) } else { lower };
    s.retain(|c| c.is_ascii_alphanumeric());
    for (from, to) in [("ph", "f"), ("ee", "i"), ("oo", "u"), ("ai", "e"), ("au", "o"), ("ei", "e"), ("w", "v"), ("z", "j"), ("q", "k"), ("x", "ks")] {
        s = s.replace(from, to);
    }
    // Aspirates: kh, gh, ch, sh, th, bh, dh -> the plain letter.
    let chars: Vec<char> = s.chars().collect();
    let mut t = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev_cons = i > 0 && !"aeiou".contains(chars[i - 1]) && chars[i - 1].is_ascii_alphabetic();
        if c == 'h' && prev_cons {
            continue;
        }
        if t.chars().last() == Some(c) {
            continue;
        }
        t.push(c);
    }
    if t.len() > 2 && t.ends_with('a') {
        t.pop();
    }
    t
}

/// Normalised words of a text (Roman or Devanagari, punctuation dropped).
pub fn norm_words(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || (!c.is_alphanumeric() && !('\u{0900}'..='\u{097F}').contains(&c)) || c == '\u{0964}')
        .map(norm_word)
        .filter(|w| !w.is_empty())
        .collect()
}

fn lev(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn words_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let m = a.chars().count().min(b.chars().count());
    (m >= 4 && lev(a, b) <= 1) || (m >= 7 && lev(a, b) <= 2)
}

// ------------------------------------------------------------------ alignment

/// One heard word and when.
struct HeardWord {
    word: String,
    start: f64,
    end: f64,
}

fn heard_words(heard: &[Heard]) -> Vec<HeardWord> {
    let mut out = Vec::new();
    for h in heard {
        let words = norm_words(&h.text);
        let n = words.len().max(1) as f64;
        for (i, w) in words.into_iter().enumerate() {
            let d = h.end - h.start;
            out.push(HeardWord { word: w, start: h.start + d * i as f64 / n, end: h.start + d * (i as f64 + 1.0) / n });
        }
    }
    out
}

/// Look for `line` in `hw` from `cursor`: `(first, last, score)` of the best
/// stretch (earliest wins a tie).
fn find_line(line: &[String], hw: &[HeardWord], cursor: usize) -> Option<(usize, usize, f64)> {
    let l = line.len();
    let window = if l <= 2 { 15 } else { 60 };
    let span = l + l / 2 + 3;
    let mut best: Option<(usize, usize, f64)> = None;
    for s in cursor..hw.len().min(cursor + window) {
        let (mut j, mut count, mut first, mut last) = (0usize, 0usize, None, s);
        for (k, h) in hw.iter().enumerate().skip(s).take(span) {
            if let Some(off) = line[j..line.len().min(j + 3)].iter().position(|w| words_match(w, &h.word)) {
                j += off + 1;
                count += 1;
                first.get_or_insert(k);
                last = k;
                if j >= l {
                    break;
                }
            }
        }
        let Some(first) = first else { continue };
        let score = count as f64 / l as f64;
        if best.is_none_or(|b| score > b.2 + 1e-9) {
            best = Some((first, last, score));
        }
    }
    let need = if l <= 2 { 0.99 } else { 0.5 };
    best.filter(|b| b.2 >= need)
}

/// A line found in a part, in the part's own seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartLine {
    /// 1-based number among the screenplay's spoken lines.
    pub line: usize,
    pub scene: u32,
    pub item: usize,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartRow {
    pub n: usize,
    pub file: String,
    pub duration: f64,
    /// Seconds of the episode before this part.
    pub offset: f64,
    pub first_line: Option<usize>,
    pub last_line: Option<usize>,
    pub scenes: Vec<u32>,
    pub lines: Vec<PartLine>,
}

/// `parts.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PartsPlan {
    pub parts: Vec<PartRow>,
    pub uncovered_lines: Vec<usize>,
    /// False when only the parts up to `--part N` were aligned.
    pub complete: bool,
}

/// What one part is, for the aligner.
pub struct PartInput {
    pub file: PathBuf,
    pub duration: f64,
    pub heard: Vec<Heard>,
}

/// **Align the parts to the screenplay's lines, in order.** Each part takes up
/// where the last matched line of the one before ended.
pub fn align_parts(refs: &[SpokenRef], inputs: &[PartInput]) -> PartsPlan {
    let line_words: Vec<Vec<String>> = refs.iter().map(|r| norm_words(&r.text)).collect();
    let mut next = 0usize;
    let mut offset = 0.0;
    let mut covered = vec![false; refs.len()];
    let mut parts = Vec::new();
    for (k, input) in inputs.iter().enumerate() {
        let hw = heard_words(&input.heard);
        let (mut cursor, mut li, mut misses) = (0usize, next, 0usize);
        let mut lines: Vec<PartLine> = Vec::new();
        while li < refs.len() && cursor < hw.len() && misses < 4 {
            if line_words[li].is_empty() {
                li += 1;
                continue;
            }
            match find_line(&line_words[li], &hw, cursor) {
                Some((a, b, _)) => {
                    lines.push(PartLine { line: li + 1, scene: refs[li].scene, item: refs[li].item, start: hw[a].start, end: hw[b].end });
                    covered[li] = true;
                    cursor = b + 1;
                    misses = 0;
                    next = li + 1;
                }
                None => misses += 1,
            }
            li += 1;
        }
        let mut scenes: Vec<u32> = Vec::new();
        for l in &lines {
            if !scenes.contains(&l.scene) {
                scenes.push(l.scene);
            }
        }
        parts.push(PartRow {
            n: k + 1,
            file: input.file.display().to_string(),
            duration: input.duration,
            offset,
            first_line: lines.first().map(|l| l.line),
            last_line: lines.last().map(|l| l.line),
            scenes,
            lines,
        });
        offset += input.duration;
    }
    PartsPlan { parts, uncovered_lines: (0..refs.len()).filter(|i| !covered[*i]).map(|i| i + 1).collect(), complete: true }
}

impl PartsPlan {
    pub fn part(&self, n: usize) -> Option<&PartRow> {
        self.parts.iter().find(|p| p.n == n)
    }

    /// The scenes that have a line in part `n`.
    pub fn part_scenes(&self, n: usize) -> Vec<u32> {
        self.part(n).map(|p| p.scenes.clone()).unwrap_or_default()
    }

    /// Every line at episode time (offset by the parts before).
    pub fn episode_lines(&self) -> Vec<AudioLine> {
        self.parts
            .iter()
            .flat_map(|p| p.lines.iter().map(move |l| AudioLine { scene: l.scene, item: l.item, start: p.offset + l.start, end: p.offset + l.end }))
            .collect()
    }

    /// Part `n`'s lines in that part's own seconds.
    pub fn part_lines(&self, n: usize) -> Vec<AudioLine> {
        self.part(n)
            .map(|p| p.lines.iter().map(|l| AudioLine { scene: l.scene, item: l.item, start: l.start, end: l.end }).collect())
            .unwrap_or_default()
    }

    /// The "Parts" section of `report.md`.
    pub fn markdown(&self, refs: &[SpokenRef]) -> String {
        let mut o = String::from("\n## Parts\n\n");
        if !self.complete {
            o.push_str("_Only the parts up to the one asked for were aligned._\n\n");
        }
        o.push_str("| Part | File | Seconds | Lines | Scenes |\n|---|---|---|---|---|\n");
        for p in &self.parts {
            let lines = match (p.first_line, p.last_line) {
                (Some(a), Some(b)) => format!("{a}\u{2013}{b} ({} found)", p.lines.len()),
                _ => "none found".into(),
            };
            let name = Path::new(&p.file).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.file.clone());
            o.push_str(&format!("| {} | {name} | {:.1} | {lines} | {:?} |\n", p.n, p.duration, p.scenes));
        }
        if self.complete {
            o.push_str("\n### Lines no part covered\n\n");
            if self.uncovered_lines.is_empty() {
                o.push_str("None.\n");
            }
            for n in &self.uncovered_lines {
                let r = refs.get(n - 1);
                o.push_str(&format!(
                    "* line {n} (scene {}): {}\n",
                    r.map(|r| r.scene).unwrap_or(0),
                    r.map(|r| r.text.chars().take(80).collect::<String>()).unwrap_or_default()
                ));
            }
        }
        o
    }

    pub fn to_json(&self, refs: &[SpokenRef]) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["uncovered"] = json!(self
            .uncovered_lines
            .iter()
            .map(|n| json!({"line": n, "scene": refs.get(n - 1).map(|r| r.scene), "text": refs.get(n - 1).map(|r| r.text.clone())}))
            .collect::<Vec<_>>());
        v
    }
}

// ------------------------------------------------------------------ plans

/// An [`AudioPlan`] for `lines` in one file: the scenes' spans get a lead-in
/// and a tail, an overlap is split at the middle of the gap.
pub fn plan_from_lines(path: &Path, lines: Vec<AudioLine>, scene_order: &[u32], duration: f64, notes: Vec<String>) -> AudioPlan {
    let mut spans: Vec<(u32, f64, f64, f64, f64)> = Vec::new();
    for &scene in scene_order {
        let mine: Vec<&AudioLine> = lines.iter().filter(|l| l.scene == scene).collect();
        let (Some(first), Some(last)) = (mine.iter().map(|l| l.start).reduce(f64::min), mine.iter().map(|l| l.end).reduce(f64::max)) else {
            continue;
        };
        spans.push((scene, (first - 0.5).max(0.0), (last + 0.8).min(duration), first, last));
    }
    for k in 1..spans.len() {
        if spans[k - 1].2 > spans[k].1 {
            let mid = (spans[k - 1].4 + spans[k].3) / 2.0;
            spans[k - 1].2 = mid.max(spans[k - 1].4);
            spans[k].1 = mid.min(spans[k].3);
        }
    }
    AudioPlan {
        path: path.to_path_buf(),
        lines,
        scene_spans: spans.into_iter().map(|(n, s, e, _, _)| (n, (s, e))).collect(),
        scene_paths: Default::default(),
        notes,
    }
}

/// Cut points (seconds) for ~`target`-second parts: at each mark, the middle of
/// the silence nearest to it (within a third of `target`), else the mark itself.
pub fn split_points(utts: &[(f64, f64)], duration: f64, target: f64) -> Vec<f64> {
    let gaps: Vec<(f64, f64)> = utts.windows(2).map(|w| (w[0].1, w[1].0)).filter(|g| g.1 - g.0 > 0.05).collect();
    let mut cuts: Vec<f64> = Vec::new();
    let mut mark = target;
    while mark < duration {
        let reach = target / 3.0;
        let best = gaps
            .iter()
            .map(|g| ((g.0 + g.1) / 2.0, g.1 - g.0))
            .filter(|(mid, _)| (mid - mark).abs() <= reach && cuts.last().is_none_or(|c| *mid > *c + 30.0))
            .min_by(|a, b| (a.0 - mark).abs().partial_cmp(&(b.0 - mark).abs()).unwrap_or(Ordering::Equal));
        cuts.push(best.map(|b| b.0).unwrap_or(mark));
        mark = cuts.last().copied().unwrap_or(mark) + target;
    }
    // A stub of a last part joins the one before it.
    if let Some(&last) = cuts.last() {
        if duration - last < target * 0.4 {
            cuts.pop();
        }
    }
    cuts
}

// ------------------------------------------------------------------ the step

/// What the audio step is given.
pub struct AudioInputs<'a> {
    /// The full final episode file.
    pub audio: Option<&'a Path>,
    /// The folder of parts.
    pub parts_dir: Option<&'a Path>,
    /// `--part N` (1-based).
    pub part: Option<usize>,
    pub out: &'a Path,
}

pub struct AudioOutcome {
    /// The plan the build uses: the whole episode, or part N's.
    pub plan: AudioPlan,
    pub parts: Option<PartsPlan>,
    pub transcript: bool,
}

fn stamp(path: &Path) -> String {
    let m = std::fs::metadata(path).ok();
    let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    format!("{}|{}|{t}", path.display(), m.map(|m| m.len()).unwrap_or(0))
}

/// A transcript, kept in `audio/transcripts/` until the file changes.
fn transcribe_cached(path: &Path, clip: &Clip, out: &Path, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<Option<Vec<Heard>>> {
    let dir = out.join("audio").join("transcripts");
    let key = stamp(path);
    let cache = dir.join(format!("{}.json", path.file_stem().unwrap_or_default().to_string_lossy()));
    if let Some(v) = std::fs::read_to_string(&cache).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if v["key"].as_str() == Some(key.as_str()) {
            if let Some(a) = v["segments"].as_array() {
                let heard: Vec<Heard> = a
                    .iter()
                    .filter_map(|s| Some(Heard { start: s["start"].as_f64()?, end: s["end"].as_f64()?, text: s["text"].as_str()?.to_string() }))
                    .collect();
                say(&format!("{}: transcript kept", path.display()));
                return Ok(Some(heard));
            }
        }
    }
    if let Err(e) = buzz_ai::transcribe::installed_whisper() {
        say(&format!("no Whisper ({e}); durations only"));
        return Ok(None);
    }
    say(&format!("Whisper is listening to {} ({:.0} s)\u{2026}", path.display(), clip.duration_seconds()));
    match buzz_ai::transcribe::transcribe_clip(clip, &mut |_, _| {}, cancel) {
        Ok(t) => {
            let heard: Vec<Heard> = t.segments.into_iter().map(|s| Heard { start: s.start, end: s.end, text: s.text }).collect();
            let _ = std::fs::create_dir_all(&dir);
            let segs: Vec<Value> = heard.iter().map(|h| json!({"start": h.start, "end": h.end, "text": h.text})).collect();
            let _ = std::fs::write(&cache, json!({"key": key, "segments": segs}).to_string());
            Ok(Some(heard))
        }
        Err(e) => {
            if cancel() {
                bail!("cancelled");
            }
            say(&format!("no transcript ({e}); durations only"));
            Ok(None)
        }
    }
}

fn write_clip(clip: &Clip, path: &Path) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    std::fs::write(path, audio_align::wav_bytes(clip)).with_context(|| format!("writing {}", path.display()))
}

/// Parts joined in order into one mono clip at the first part's rate.
fn join_clips(clips: &[Clip]) -> Result<Clip> {
    let rate = clips.first().map(|c| c.sample_rate).ok_or_else(|| anyhow!("no parts to join"))?;
    let mut samples: Vec<f32> = Vec::new();
    for c in clips {
        samples.extend(c.resampled_mono(rate));
    }
    Clip::new("full", rate, 1, samples)
}

/// **The audio step.** See the module docs for the three input shapes.
pub fn run_audio(
    inputs: &AudioInputs,
    chosen: &[&ScriptScene],
    say: &mut dyn FnMut(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<AudioOutcome> {
    let refs: Vec<SpokenRef> = chosen.iter().flat_map(|s| audio_align::spoken_refs(s)).collect();
    let scene_order: Vec<u32> = chosen.iter().map(|s| s.number).collect();
    let out = inputs.out;
    let stop_at = inputs.part;
    if stop_at == Some(0) {
        bail!("--part counts from 1");
    }
    if stop_at.is_some() && inputs.audio.is_none() && inputs.parts_dir.is_none() {
        bail!("--part needs --audio-parts <folder> or --audio <full file>");
    }

    // 1. the parts: the folder's, or the full file split.
    let mut part_inputs: Vec<PartInput> = Vec::new();
    let mut part_clips: Vec<Clip> = Vec::new();
    let mut transcript_any = false;
    let mut full_clip: Option<Clip> = None;
    let mut full_heard: Option<Vec<Heard>> = None;
    if let Some(dir) = inputs.parts_dir {
        let files = list_parts(dir)?;
        if files.is_empty() {
            bail!("{}: no wav, mp3 or m4a parts found", dir.display());
        }
        say(&format!("{} part(s) in {}", files.len(), dir.display()));
        for (k, f) in files.iter().enumerate() {
            if cancel() {
                bail!("cancelled");
            }
            // Parts after the asked-for one are only read for the join.
            let needed = stop_at.is_none_or(|n| k < n) ;
            let clip = Clip::open(f).with_context(|| format!("opening {}", f.display()))?;
            let heard = if needed { transcribe_cached(f, &clip, out, say, cancel)? } else { None };
            transcript_any |= heard.is_some();
            part_inputs.push(PartInput { file: f.clone(), duration: clip.duration_seconds(), heard: heard.unwrap_or_default() });
            if stop_at.is_none() {
                part_clips.push(clip);
            }
        }
        if let Some(n) = stop_at {
            if n > files.len() {
                bail!("--part {n}: the folder has {} part(s)", files.len());
            }
            part_inputs.truncate(n);
        }
    }
    if let Some(path) = inputs.audio {
        say(&format!("opening {}", path.display()));
        let clip = Clip::open(path).with_context(|| format!("opening {}", path.display()))?;
        // A full file is transcribed when it is the episode (no --part) or the only source of parts.
        if stop_at.is_none() || inputs.parts_dir.is_none() {
            full_heard = transcribe_cached(path, &clip, out, say, cancel)?;
            transcript_any |= full_heard.is_some();
        }
        if inputs.parts_dir.is_none() {
            let duration = clip.duration_seconds();
            let utts = audio_align::utterances(&clip);
            let cuts = split_points(&utts, duration, PART_TARGET_S);
            let mut bounds = vec![0.0];
            bounds.extend(cuts);
            bounds.push(duration);
            say(&format!("splitting into {} working part(s) near every {:.0} s", bounds.len() - 1, PART_TARGET_S));
            for (k, w) in bounds.windows(2).enumerate() {
                let (a, b) = (w[0], w[1]);
                let file = out.join("audio").join("parts").join(format!("part_{:02}.wav", k + 1));
                write_clip(&clip.trimmed(a as f32, Some(b as f32)), &file)?;
                let heard: Vec<Heard> = full_heard
                    .iter()
                    .flatten()
                    .filter(|h| h.start >= a && h.start < b)
                    .map(|h| Heard { start: h.start - a, end: (h.end - a).min(b - a), text: h.text.clone() })
                    .collect();
                part_inputs.push(PartInput { file, duration: b - a, heard });
            }
            if let Some(n) = stop_at {
                if n > part_inputs.len() {
                    bail!("--part {n}: the full file splits into {} part(s)", part_inputs.len());
                }
            }
        }
        full_clip = Some(clip);
    }

    // 2. align the parts.
    let mut parts = align_parts(&refs, &part_inputs);
    if let Some(n) = stop_at {
        parts.parts.truncate(n);
        parts.complete = false;
        parts.uncovered_lines.clear();
    }
    let parts_complete = parts.complete;

    // 3. the plan the build uses.
    if let Some(n) = stop_at {
        let row = parts.part(n).ok_or_else(|| anyhow!("--part {n}: no such part"))?;
        if row.lines.is_empty() {
            bail!("--part {n}: no screenplay line was found in {} (is it transcribed? Whisper must be installed)", row.file);
        }
        let notes = vec![format!(
            "part {n} ({}): lines {}\u{2013}{} found, scenes {:?}",
            row.file,
            row.first_line.unwrap_or(0),
            row.last_line.unwrap_or(0),
            row.scenes
        )];
        let file = PathBuf::from(&row.file);
        let plan = plan_from_lines(&file, parts.part_lines(n), &scene_order, row.duration, notes);
        return Ok(AudioOutcome { plan, parts: Some(parts), transcript: transcript_any });
    }
    let plan = if let Some(path) = inputs.audio {
        // The full file is the episode: the ordinary aligner over all of it.
        let clip = full_clip.as_ref().ok_or_else(|| anyhow!("full audio not read"))?;
        audio_align::plan(chosen, path, clip, full_heard.as_deref())
    } else {
        // Parts only: joined in order; line times are episode times.
        let joined = join_clips(&part_clips)?;
        let full = out.join("audio").join("full.wav");
        write_clip(&joined, &full)?;
        say(&format!("{} part(s) joined into {}", part_clips.len(), full.display()));
        if parts.episode_lines().is_empty() {
            audio_align::plan(chosen, &full, &joined, None)
        } else {
            let notes = vec![format!(
                "{}: {} of {} line(s) aligned part by part (offsets of the parts before)",
                full.display(),
                parts.episode_lines().len(),
                refs.len()
            )];
            plan_from_lines(&full, parts.episode_lines(), &scene_order, joined.duration_seconds(), notes)
        }
    };
    let _ = parts_complete;
    Ok(AudioOutcome { plan, parts: Some(parts), transcript: transcript_any })
}

/// The spoken lines the parts are aligned against (for `parts.json` text).
pub fn spoken_lines(chosen: &[&ScriptScene]) -> Vec<SpokenRef> {
    chosen.iter().flat_map(|s| audio_align::spoken_refs(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(scene: u32, item: usize, text: &str) -> SpokenRef {
        SpokenRef { scene, item, words: text.split_whitespace().count(), text: text.into() }
    }

    fn h(start: f64, end: f64, text: &str) -> Heard {
        Heard { start, end, text: text.into() }
    }

    #[test]
    fn natural_sort_of_part_names() {
        let mut names = vec!["part10.wav", "part2.wav", "Part1.mp3", "part02b.wav"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["Part1.mp3", "part2.wav", "part02b.wav", "part10.wav"]);
        let mut n2 = vec!["10", "02", "01"];
        n2.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(n2, vec!["01", "02", "10"]);
    }

    #[test]
    fn two_parts_map_to_the_right_lines_with_offsets() {
        let refs = vec![
            r(1, 1, "Raat ke teen baje the aur gaon sunsaan tha"),
            r(1, 2, "Munna ne darwaza khola"),
            r(2, 1, "Kya tum ne kuch suna"),
            r(2, 2, "कोई नहीं था वहाँ"),
        ];
        let parts = vec![
            PartInput {
                file: "p1.wav".into(),
                duration: 100.0,
                heard: vec![h(1.0, 6.0, "raat ke 3 baje the aur gaon sunsan tha"), h(7.0, 9.0, "munna ne darvaja khola")],
            },
            PartInput {
                file: "p2.wav".into(),
                duration: 80.0,
                heard: vec![h(2.0, 4.0, "kya tumne kuch suna"), h(5.0, 8.0, "koi nahi tha vahan")],
            },
        ];
        let plan = align_parts(&refs, &parts);
        assert_eq!((plan.parts[0].first_line, plan.parts[0].last_line), (Some(1), Some(2)));
        assert_eq!((plan.parts[1].first_line, plan.parts[1].last_line), (Some(3), Some(4)));
        assert_eq!(plan.parts[1].scenes, vec![2]);
        assert!(plan.uncovered_lines.is_empty());
        let ep = plan.episode_lines();
        assert_eq!(ep.len(), 4);
        assert!((ep[2].start - 102.0).abs() < 0.6, "{}", ep[2].start);
        assert!((plan.part_lines(2)[0].start - 2.0).abs() < 0.6);
    }

    #[test]
    fn a_part_filters_to_its_scenes() {
        let plan = PartsPlan {
            parts: vec![
                PartRow { n: 1, file: "a".into(), duration: 10.0, offset: 0.0, first_line: Some(1), last_line: Some(2), scenes: vec![1, 2], lines: vec![] },
                PartRow { n: 2, file: "b".into(), duration: 10.0, offset: 10.0, first_line: Some(3), last_line: Some(3), scenes: vec![3], lines: vec![] },
            ],
            uncovered_lines: vec![],
            complete: true,
        };
        assert_eq!(plan.part_scenes(2), vec![3]);
        assert_eq!(plan.part_scenes(1), vec![1, 2]);
    }
}
