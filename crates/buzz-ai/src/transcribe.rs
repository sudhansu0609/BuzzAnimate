//! F7.9 — speech to text on a local ONNX Whisper (base.en).
//!
//! Audio comes in through `buzz-audio` (any format it decodes), is mixed to
//! mono and resampled to 16 kHz ([`buzz_audio::Clip::resampled_mono`]), cut
//! into 30-second windows, turned into Whisper's 80-bin log-mel spectrogram,
//! encoded once per window and decoded greedily **with timestamps**, so every
//! segment carries its own start and end. The segments become an `.srt`
//! ([`to_srt`]) and [`crate::director::DialogueCue`]s — the same shape the AI
//! director's dialogue uses — so captions and lip sync go through the
//! existing `Editor::sync_directed_dialogue` join.
//!
//! The decoder is `decoder_model.onnx` (no KV cache): each step re-runs the
//! tokens so far. That is simpler than the merged/with-past graphs and fast
//! enough for dialogue lines (measured in the plan's Status table).

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};

use crate::models::{get_entry, models_dir, ModelId};
use crate::onnx::OnnxSession;

/// Whisper's input rate.
pub const SAMPLE_RATE: u32 = 16_000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 80;
const CHUNK_SAMPLES: usize = 30 * SAMPLE_RATE as usize;
const N_FRAMES: usize = CHUNK_SAMPLES / HOP; // 3000

// base.en special tokens (English-only vocabulary, 51 864 entries).
const EOT: i64 = 50256;
const SOT: i64 = 50257;
const NO_TIMESTAMPS: i64 = 50362;
const TS_BEGIN: i64 = 50363;
/// Seconds per timestamp token.
const TS_STEP: f64 = 0.02;
const MAX_TOKENS: usize = 224;
/// `suppress_tokens` from the model's generation config.
const SUPPRESS: &[i64] = &[
    1, 2, 7, 8, 9, 10, 14, 25, 26, 27, 28, 29, 31, 58, 59, 60, 61, 62, 63, 90, 91, 92, 93, 357,
    366, 438, 532, 685, 705, 796, 930, 1058, 1220, 1267, 1279, 1303, 1343, 1377, 1391, 1635, 1782,
    1875, 2162, 2361, 2488, 3467, 4008, 4211, 4600, 4808, 5299, 5855, 6329, 7203, 9609, 9959,
    10563, 10786, 11420, 11709, 11907, 13163, 13697, 13700, 14808, 15306, 16410, 16791, 17992,
    19203, 19510, 20724, 22305, 22935, 27007, 30109, 30420, 33409, 34949, 40283, 40493, 40549,
    47282, 49146, 50257, 50357, 50358, 50359, 50360, 50361,
];

/// One timed line of speech.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    /// Seconds from the start of the sound.
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// A whole transcription and what it cost.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Transcript {
    pub segments: Vec<Segment>,
    pub audio_seconds: f64,
    pub load_ms: u128,
    pub run_ms: u128,
    /// The execution provider the encoder ran on.
    pub provider: String,
}

impl Transcript {
    /// All the text, one segment per line.
    pub fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

// ---------------------------------------------------------------- features

fn hann(n: usize) -> Vec<f32> {
    // Periodic Hann, as `torch.hann_window(400)`.
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos())
        .collect()
}

fn hz_to_mel(f: f64) -> f64 {
    // Slaney scale (librosa default, htk=False).
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if f >= min_log_hz {
        min_log_mel + (f / min_log_hz).ln() / logstep
    } else {
        f / f_sp
    }
}

fn mel_to_hz(m: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if m >= min_log_mel {
        min_log_hz * (logstep * (m - min_log_mel)).exp()
    } else {
        f_sp * m
    }
}

/// librosa.filters.mel(sr=16000, n_fft=400, n_mels=80), Slaney-normalised:
/// `N_MELS × (N_FFT/2+1)`.
fn mel_filters() -> &'static Vec<f32> {
    static F: OnceLock<Vec<f32>> = OnceLock::new();
    F.get_or_init(|| {
        let n_bins = N_FFT / 2 + 1;
        let fft_freqs: Vec<f64> = (0..n_bins)
            .map(|i| i as f64 * SAMPLE_RATE as f64 / N_FFT as f64)
            .collect();
        let (mmin, mmax) = (hz_to_mel(0.0), hz_to_mel(SAMPLE_RATE as f64 / 2.0));
        let pts: Vec<f64> = (0..N_MELS + 2)
            .map(|i| mel_to_hz(mmin + (mmax - mmin) * i as f64 / (N_MELS + 1) as f64))
            .collect();
        let mut w = vec![0f32; N_MELS * n_bins];
        for m in 0..N_MELS {
            let (lo, c, hi) = (pts[m], pts[m + 1], pts[m + 2]);
            let enorm = 2.0 / (hi - lo);
            for (k, &f) in fft_freqs.iter().enumerate() {
                let up = (f - lo) / (c - lo);
                let down = (hi - f) / (hi - c);
                let v = up.min(down).max(0.0);
                w[m * n_bins + k] = (v * enorm) as f32;
            }
        }
        w
    })
}

/// DFT basis (cos, sin) for the 201 real bins of a 400-point frame.
fn dft_basis() -> &'static (Vec<f32>, Vec<f32>) {
    static B: OnceLock<(Vec<f32>, Vec<f32>)> = OnceLock::new();
    B.get_or_init(|| {
        let n_bins = N_FFT / 2 + 1;
        let mut c = vec![0f32; n_bins * N_FFT];
        let mut s = vec![0f32; n_bins * N_FFT];
        for k in 0..n_bins {
            for n in 0..N_FFT {
                let a = -2.0 * std::f64::consts::PI * (k * n) as f64 / N_FFT as f64;
                c[k * N_FFT + n] = a.cos() as f32;
                s[k * N_FFT + n] = a.sin() as f32;
            }
        }
        (c, s)
    })
}

/// Whisper's log-mel features for one window of up to 30 s of 16 kHz mono
/// audio (zero-padded): `80 × 3000`, row-major.
pub fn log_mel(samples: &[f32]) -> Vec<f32> {
    let mut audio = samples[..samples.len().min(CHUNK_SAMPLES)].to_vec();
    audio.resize(CHUNK_SAMPLES, 0.0);
    // center=True: reflect-pad n_fft/2 on both sides.
    let pad = N_FFT / 2;
    let mut padded = Vec::with_capacity(audio.len() + 2 * pad);
    for i in (1..=pad).rev() {
        padded.push(audio[i]);
    }
    padded.extend_from_slice(&audio);
    for i in 0..pad {
        padded.push(audio[audio.len() - 2 - i]);
    }
    let window = hann(N_FFT);
    let (cb, sb) = dft_basis();
    let filters = mel_filters();
    let n_bins = N_FFT / 2 + 1;
    let mut mel = vec![0f32; N_MELS * N_FRAMES];
    let mut frame = vec![0f32; N_FFT];
    let mut power = vec![0f32; n_bins];
    // STFT frames 0..3000 (the 3001st is dropped, as Whisper does).
    for t in 0..N_FRAMES {
        let start = t * HOP;
        for n in 0..N_FFT {
            frame[n] = padded[start + n] * window[n];
        }
        for k in 0..n_bins {
            let (row_c, row_s) = (&cb[k * N_FFT..(k + 1) * N_FFT], &sb[k * N_FFT..(k + 1) * N_FFT]);
            let (mut re, mut im) = (0f32, 0f32);
            for n in 0..N_FFT {
                re += frame[n] * row_c[n];
                im += frame[n] * row_s[n];
            }
            power[k] = re * re + im * im;
        }
        for m in 0..N_MELS {
            let row = &filters[m * n_bins..(m + 1) * n_bins];
            let v: f32 = row.iter().zip(&power).map(|(a, b)| a * b).sum();
            mel[m * N_FRAMES + t] = v.max(1e-10).log10();
        }
    }
    let max = mel.iter().cloned().fold(f32::MIN, f32::max);
    for v in mel.iter_mut() {
        *v = ((*v).max(max - 8.0) + 4.0) / 4.0;
    }
    mel
}

// ---------------------------------------------------------------- tokens

/// GPT-2 byte-level vocabulary, id → bytes.
struct Vocab {
    bytes: Vec<Vec<u8>>,
    /// Ids at or above this are special tokens, never text (base.en: 50256).
    limit: i64,
}

impl Vocab {
    fn load(path: &Path) -> Result<Self> {
        Self::load_with_limit(path, EOT)
    }

    fn load_with_limit(path: &Path, limit: i64) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
        let map: std::collections::HashMap<String, usize> = serde_json::from_str(&text)?;
        // bytes_to_unicode, inverted.
        let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32)
            .chain(0xA1..=0xAC)
            .chain(0xAE..=0xFF)
            .collect();
        let mut cs = bs.clone();
        let mut n = 0;
        for b in 0..256u32 {
            if !bs.contains(&b) {
                bs.push(b);
                cs.push(256 + n);
                n += 1;
            }
        }
        let decoder: std::collections::HashMap<char, u8> = cs
            .iter()
            .zip(&bs)
            .map(|(&c, &b)| (char::from_u32(c).unwrap(), b as u8))
            .collect();
        let size = map.values().copied().max().unwrap_or(0) + 1;
        let mut bytes = vec![Vec::new(); size];
        for (tok, id) in map {
            bytes[id] = tok.chars().filter_map(|c| decoder.get(&c).copied()).collect();
        }
        Ok(Self { bytes, limit })
    }

    fn decode(&self, ids: &[i64]) -> String {
        let mut out = Vec::new();
        for &id in ids {
            if id >= 0 && (id as usize) < self.bytes.len() && id < self.limit {
                out.extend_from_slice(&self.bytes[id as usize]);
            }
        }
        String::from_utf8_lossy(&out).trim().to_string()
    }
}

// ---------------------------------------------------------------- model

/// A loaded Whisper (encoder, decoder, vocabulary).
pub struct Whisper {
    encoder: OnnxSession,
    decoder: OnnxSession,
    vocab: Vocab,
    /// Token ids of the multilingual model (Stage V); `None` for base.en.
    multi: Option<MultiTokens>,
}

/// Special-token ids of the multilingual Whisper, read from its
/// `added_tokens.json` rather than hard-coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MultiTokens {
    eot: i64,
    sot: i64,
    en: i64,
    hi: i64,
    transcribe: i64,
    no_ts: i64,
}

impl MultiTokens {
    fn parse(json: &str) -> Result<Self> {
        let map: std::collections::HashMap<String, i64> = serde_json::from_str(json)?;
        let get = |k: &str| map.get(k).copied().ok_or_else(|| anyhow!("added_tokens.json has no {k}"));
        let sot = get("<|startoftranscript|>")?;
        Ok(Self {
            // `<|endoftext|>` lives in vocab.json; it is the id just below SOT.
            eot: map.get("<|endoftext|>").copied().unwrap_or(sot - 1),
            sot,
            en: get("<|en|>")?,
            hi: get("<|hi|>")?,
            transcribe: get("<|transcribe|>")?,
            no_ts: get("<|notimestamps|>")?,
        })
    }
}

/// What a voice command sounded like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub text: String,
    /// `"en"` or `"hi"`: the language the model judged more likely.
    pub lang: &'static str,
}

impl Whisper {
    /// Loads the installed model; errors when it is not downloaded.
    pub fn load() -> Result<Self> {
        let entry = get_entry(ModelId::Whisper);
        if !crate::models::status(ModelId::Whisper).is_ready() {
            bail!("Whisper is not installed — download it in AI ▸ Models");
        }
        let dir = models_dir();
        let enc_path = dir.join(entry.filename);
        let dec_path = dir.join(entry.extra_files[0].filename);
        let vocab_path = dir.join(entry.extra_files[1].filename);
        // The encoder is the heavy half: try the GPU; the decoder's shapes
        // change every step, which DirectML handles poorly — keep it on CPU.
        let encoder = OnnxSession::load(&enc_path).or_else(|_| OnnxSession::load_cpu(&enc_path))?;
        let decoder = OnnxSession::load_cpu(&dec_path)?;
        Ok(Self { encoder, decoder, vocab: Vocab::load(&vocab_path)?, multi: None })
    }

    /// Loads the multilingual model (voice commands); errors when missing.
    pub fn load_multi() -> Result<Self> {
        let entry = get_entry(ModelId::WhisperMulti);
        if !crate::models::status(ModelId::WhisperMulti).is_ready() {
            bail!("Whisper multilingual is not installed — download it in AI ▸ Models");
        }
        let dir = models_dir();
        let tokens = MultiTokens::parse(&std::fs::read_to_string(dir.join(entry.extra_files[2].filename))?)?;
        // int8 graphs: keep both halves on the CPU, where they are exact.
        let encoder = OnnxSession::load_cpu(&dir.join(entry.filename))?;
        let decoder = OnnxSession::load_cpu(&dir.join(entry.extra_files[0].filename))?;
        let vocab = Vocab::load_with_limit(&dir.join(entry.extra_files[1].filename), tokens.eot)?;
        Ok(Self { encoder, decoder, vocab, multi: Some(tokens) })
    }

    /// Transcribes one short command (up to 30 s of 16 kHz mono): language
    /// chosen between English and Hindi from the language-token logits, task
    /// transcribe, no timestamps.
    pub fn transcribe_command(&mut self, samples: &[f32], cancel: &dyn Fn() -> bool) -> Result<Heard> {
        let t = self.multi.ok_or_else(|| anyhow!("not the multilingual model"))?;
        let (shape, enc) = self.encode(log_mel(samples))?;
        let first = self.step_logits(&[t.sot], &shape, &enc)?;
        let hindi = first[t.hi as usize] > first[t.en as usize];
        let lang_tok = if hindi { t.hi } else { t.en };
        let mut tokens = vec![t.sot, lang_tok, t.transcribe, t.no_ts];
        let mut out: Vec<i64> = Vec::new();
        for _ in 0..120 {
            if cancel() {
                bail!("cancelled");
            }
            let mut logits = self.step_logits(&tokens, &shape, &enc)?;
            // Only text and end-of-text may be sampled.
            let from = (t.eot as usize + 1).min(logits.len());
            for v in logits[from..].iter_mut() {
                *v = f32::NEG_INFINITY;
            }
            if out.is_empty() {
                logits[220] = f32::NEG_INFINITY;
                logits[t.eot as usize] = f32::NEG_INFINITY;
            }
            let next = argmax(&logits) as i64;
            if next == t.eot {
                break;
            }
            out.push(next);
            tokens.push(next);
            // A decoder stuck repeating itself ends the command.
            let n = out.len();
            if n >= 16 && out[n - 8..] == out[n - 16..n - 8] {
                out.truncate(n - 8);
                break;
            }
        }
        Ok(Heard { text: self.vocab.decode(&out), lang: if hindi { "hi" } else { "en" } })
    }

    pub fn provider(&self) -> &'static str {
        self.encoder.ep_name()
    }

    fn encode(&mut self, mel: Vec<f32>) -> Result<(Vec<i64>, Vec<f32>)> {
        let arr = ndarray::Array3::from_shape_vec((1, N_MELS, N_FRAMES), mel)?;
        let t = ort::value::Tensor::from_array(arr).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .encoder
            .inner_mut()
            .run(ort::inputs!["input_features" => t])
            .map_err(|e| anyhow!("Whisper encoder failed: {e}"))?;
        let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        Ok((shape.iter().copied().collect(), data.to_vec()))
    }

    fn step_logits(&mut self, tokens: &[i64], enc_shape: &[i64], enc: &[f32]) -> Result<Vec<f32>> {
        let ids = ndarray::Array2::from_shape_vec((1, tokens.len()), tokens.to_vec())?;
        let hs = ndarray::Array3::from_shape_vec(
            (enc_shape[0] as usize, enc_shape[1] as usize, enc_shape[2] as usize),
            enc.to_vec(),
        )?;
        let ids = ort::value::Tensor::from_array(ids).map_err(|e| anyhow!("{e}"))?;
        let hs = ort::value::Tensor::from_array(hs).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .decoder
            .inner_mut()
            .run(ort::inputs!["input_ids" => ids, "encoder_hidden_states" => hs])
            .map_err(|e| anyhow!("Whisper decoder failed: {e}"))?;
        let (shape, data) = out["logits"].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        let vocab = *shape.last().unwrap() as usize;
        let seq = shape[1] as usize;
        Ok(data[(seq - 1) * vocab..seq * vocab].to_vec())
    }

    /// Greedy decode of one window with Whisper's timestamp rules. Returns
    /// the sampled tokens after the prompt.
    fn decode_window(&mut self, enc_shape: &[i64], enc: &[f32], cancel: &dyn Fn() -> bool) -> Result<Vec<i64>> {
        let mut tokens = vec![SOT];
        let mut out: Vec<i64> = Vec::new();
        for _ in 0..MAX_TOKENS {
            if cancel() {
                bail!("cancelled");
            }
            let mut logits = self.step_logits(&tokens, enc_shape, enc)?;
            apply_timestamp_rules(&mut logits, &out);
            let next = argmax(&logits) as i64;
            if next == EOT {
                break;
            }
            out.push(next);
            tokens.push(next);
        }
        Ok(out)
    }

    /// Transcribes 16 kHz mono samples. `progress(done, total)` is called per
    /// 30-second window; `cancel` is polled every decoder step.
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        progress: &mut dyn FnMut(usize, usize),
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<Segment>> {
        let mut segments = Vec::new();
        let mut seek = 0usize;
        let total = samples.len().div_ceil(CHUNK_SAMPLES).max(1);
        let mut done = 0;
        while seek < samples.len() {
            let end = (seek + CHUNK_SAMPLES).min(samples.len());
            let window = &samples[seek..end];
            let offset = seek as f64 / SAMPLE_RATE as f64;
            let (shape, enc) = self.encode(log_mel(window))?;
            let toks = self.decode_window(&shape, &enc, cancel)?;
            let (segs, last_complete) = split_segments(&toks, &self.vocab, offset);
            segments.extend(segs);
            done += 1;
            progress(done, total);
            // Continue from the last complete segment when the window ended
            // mid-sentence; otherwise move on a whole window.
            let advance = match last_complete {
                Some(t) if end < samples.len() && t > 1.0 => (t * SAMPLE_RATE as f64) as usize,
                _ => window.len(),
            };
            seek += advance.max(1);
        }
        Ok(segments)
    }
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[best] {
            best = i;
        }
    }
    best
}

/// OpenAI's `ApplyTimestampRules` + `SuppressTokens` + `SuppressBlank`, for
/// greedy decoding.
fn apply_timestamp_rules(logits: &mut [f32], sampled: &[i64]) {
    let n = logits.len() as i64;
    let neg = f32::NEG_INFINITY;
    for &t in SUPPRESS {
        if t < n {
            logits[t as usize] = neg;
        }
    }
    logits[NO_TIMESTAMPS as usize] = neg;
    if sampled.is_empty() {
        // Blank and EOT at the start; the first token must be a timestamp
        // near zero (max_initial_timestamp 1.0 s = 50 steps).
        logits[220] = neg;
        for v in logits[..TS_BEGIN as usize].iter_mut() {
            *v = neg;
        }
        for v in logits[(TS_BEGIN + 51) as usize..].iter_mut() {
            *v = neg;
        }
        return;
    }
    let is_ts = |t: i64| t >= TS_BEGIN;
    let last_ts = is_ts(*sampled.last().unwrap());
    let penult_ts = sampled.len() < 2 || is_ts(sampled[sampled.len() - 2]);
    if last_ts {
        if penult_ts {
            // A pair closed a segment: text must follow (or end).
            for v in logits[TS_BEGIN as usize..].iter_mut() {
                *v = neg;
            }
        } else {
            // A single timestamp after text: another timestamp or EOT.
            for v in logits[..EOT as usize].iter_mut() {
                *v = neg;
            }
        }
    }
    // Timestamps never go backwards.
    if let Some(&last) = sampled.iter().rev().find(|&&t| is_ts(t)) {
        let floor = if last_ts && !penult_ts { last } else { last + 1 };
        for v in logits[TS_BEGIN as usize..(floor.min(n)) as usize].iter_mut() {
            *v = neg;
        }
    }
    // If timestamps together are likelier than any text token, take one.
    let lse = |s: &[f32]| {
        let m = s.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        if m == f32::NEG_INFINITY {
            return m;
        }
        m + s.iter().map(|x| (x - m).exp()).sum::<f32>().ln()
    };
    let all = lse(logits);
    let ts_lp = lse(&logits[TS_BEGIN as usize..]) - all;
    let text_max = logits[..TS_BEGIN as usize].iter().cloned().fold(f32::NEG_INFINITY, f32::max) - all;
    if ts_lp > text_max {
        for v in logits[..TS_BEGIN as usize].iter_mut() {
            *v = neg;
        }
    }
}

/// Splits `<|t0|> text <|t1|>` runs into segments offset by `offset` s.
/// Also returns the end time (within the window) of the last closed segment.
fn split_segments(tokens: &[i64], vocab: &Vocab, offset: f64) -> (Vec<Segment>, Option<f64>) {
    let mut segs = Vec::new();
    let mut start: Option<f64> = None;
    let mut text: Vec<i64> = Vec::new();
    let mut last_end = None;
    for &t in tokens {
        if t >= TS_BEGIN {
            let time = (t - TS_BEGIN) as f64 * TS_STEP;
            match start {
                None => start = Some(time),
                Some(s) if !text.is_empty() => {
                    let words = vocab.decode(&text);
                    if !words.is_empty() {
                        segs.push(Segment { start: offset + s, end: offset + time, text: words });
                    }
                    text.clear();
                    last_end = Some(time);
                    start = None;
                }
                Some(_) => start = Some(time),
            }
        } else {
            if start.is_none() {
                start = Some(last_end.unwrap_or(0.0));
            }
            text.push(t);
        }
    }
    // Trailing text with no closing timestamp: close it at the window end.
    if !text.is_empty() {
        let words = vocab.decode(&text);
        if !words.is_empty() {
            let s = start.unwrap_or(0.0);
            segs.push(Segment { start: offset + s, end: offset + (s + 2.0).min(30.0), text: words });
        }
    }
    (segs, last_end)
}

// ---------------------------------------------------------------- API

static POOL: OnceLock<Mutex<Option<Arc<Mutex<Whisper>>>>> = OnceLock::new();

fn pooled() -> Result<(Arc<Mutex<Whisper>>, u128)> {
    let cell = POOL.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap();
    if let Some(w) = guard.as_ref() {
        return Ok((w.clone(), 0));
    }
    let t = Instant::now();
    let w = Arc::new(Mutex::new(Whisper::load()?));
    *guard = Some(w.clone());
    Ok((w, t.elapsed().as_millis()))
}

/// Transcribes a decoded sound (any rate, any channels).
pub fn transcribe_clip(
    clip: &buzz_audio::Clip,
    progress: &mut dyn FnMut(usize, usize),
    cancel: &dyn Fn() -> bool,
) -> Result<Transcript> {
    let samples = clip.resampled_mono(SAMPLE_RATE);
    transcribe_samples(&samples, progress, cancel)
}

/// Transcribes 16 kHz mono samples with the shared, lazily loaded model.
pub fn transcribe_samples(
    samples: &[f32],
    progress: &mut dyn FnMut(usize, usize),
    cancel: &dyn Fn() -> bool,
) -> Result<Transcript> {
    let (model, load_ms) = pooled()?;
    let mut w = model.lock().unwrap();
    let t = Instant::now();
    let segments = w.transcribe(samples, progress, cancel)?;
    Ok(Transcript {
        segments,
        audio_seconds: samples.len() as f64 / SAMPLE_RATE as f64,
        load_ms,
        run_ms: t.elapsed().as_millis(),
        provider: w.provider().to_string(),
    })
}

static POOL_MULTI: OnceLock<Mutex<Option<Arc<Mutex<Whisper>>>>> = OnceLock::new();

/// Whether the multilingual voice model is downloaded.
pub fn multi_ready() -> bool {
    crate::models::status(ModelId::WhisperMulti).is_ready()
}

/// Transcribes a voice command (16 kHz mono) with the shared, lazily loaded
/// multilingual model. Blocks: call from a worker thread.
pub fn transcribe_command(samples: &[f32], cancel: &dyn Fn() -> bool) -> Result<Heard> {
    let model = {
        let cell = POOL_MULTI.get_or_init(|| Mutex::new(None));
        let mut guard = cell.lock().unwrap();
        match guard.as_ref() {
            Some(w) => w.clone(),
            None => {
                let w = Arc::new(Mutex::new(Whisper::load_multi()?));
                *guard = Some(w.clone());
                w
            }
        }
    };
    let mut w = model.lock().unwrap();
    w.transcribe_command(samples, cancel)
}

// ------------------------------------------------- the Assistant's `transcribe`

/// Which Whisper the Assistant's `transcribe` tool will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperChoice {
    /// base.en: English only, real per-line timestamps.
    BaseEn,
    /// small multilingual (English, Hindi, Hinglish): lines are whole windows.
    Multilingual,
}

impl WhisperChoice {
    /// The name the tool's result reports.
    pub fn label(self) -> &'static str {
        match self {
            Self::BaseEn => "whisper-base.en",
            Self::Multilingual => "whisper-small-multilingual",
        }
    }
}

/// The multilingual model when it is installed (it hears Hindi; base.en turns
/// Hindi into English-looking nonsense), else base.en, else an error that says
/// how to get one. Pure so it can be tested without any model on disk.
pub fn pick_whisper(multi_ready: bool, base_ready: bool) -> std::result::Result<WhisperChoice, String> {
    if multi_ready {
        Ok(WhisperChoice::Multilingual)
    } else if base_ready {
        Ok(WhisperChoice::BaseEn)
    } else {
        Err("No Whisper model is installed. Download \"Whisper base.en\" (English) or \"Whisper small multilingual\" (English, Hindi, Hinglish) in AI \u{25b8} Models, then call transcribe again.".to_string())
    }
}

/// [`pick_whisper`] for what is on this machine.
pub fn installed_whisper() -> std::result::Result<WhisperChoice, String> {
    pick_whisper(multi_ready(), crate::models::status(ModelId::Whisper).is_ready())
}

/// The multilingual decoder here has no timestamp tokens, and hears at most
/// 30 s at a time, so a long sound is cut into windows of about this length.
const MULTI_WINDOW: usize = 25 * SAMPLE_RATE as usize;
/// A window may end anywhere in its last stretch of this length: the quietest
/// spot is chosen, so a cut falls between words rather than through one.
const MULTI_SEARCH: usize = 5 * SAMPLE_RATE as usize;
/// A window quieter than this (RMS) is silence, and is not sent to Whisper,
/// which invents text for silence.
const SILENCE_RMS: f32 = 0.004;

/// Cut `samples` (16 kHz mono) into windows of at most 29 s for the
/// multilingual model: `(start, end)` sample ranges that cover it exactly.
pub fn plan_windows(samples: &[f32]) -> Vec<(usize, usize)> {
    let n = samples.len();
    let block = (SAMPLE_RATE / 20) as usize; // 50 ms
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        // The model takes 30 s; anything up to 29 s is sent whole.
        if n - start <= MULTI_WINDOW + MULTI_SEARCH - SAMPLE_RATE as usize {
            out.push((start, n));
            break;
        }
        let from = start + MULTI_WINDOW - MULTI_SEARCH;
        let to = start + MULTI_WINDOW;
        let mut best = (f32::INFINITY, to);
        let mut at = from;
        while at + block <= to {
            let energy: f32 = samples[at..at + block].iter().map(|s| s * s).sum();
            if energy < best.0 {
                best = (energy, at + block / 2);
            }
            at += block;
        }
        out.push((start, best.1));
        start = best.1;
    }
    out
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Transcribes a decoded sound with the multilingual model: one line per
/// window (about 25 s, cut at the quietest spot), each with the language the
/// model heard. Returns the transcript and the language (`"en"`, `"hi"`, or
/// `"mixed"` when the windows disagree). Blocks: call from a worker thread.
///
/// The line times are the window edges, **not** word-accurate: the voice-command
/// decoder has no timestamp tokens. The Assistant's result says so.
pub fn transcribe_clip_multi(
    clip: &buzz_audio::Clip,
    progress: &mut dyn FnMut(usize, usize),
    cancel: &dyn Fn() -> bool,
) -> Result<(Transcript, String)> {
    let samples = clip.resampled_mono(SAMPLE_RATE);
    let windows = plan_windows(&samples);
    let t = Instant::now();
    let mut segments = Vec::new();
    let mut langs: Vec<&'static str> = Vec::new();
    for (i, &(a, b)) in windows.iter().enumerate() {
        if cancel() {
            bail!("cancelled");
        }
        if rms(&samples[a..b]) >= SILENCE_RMS {
            let heard = transcribe_command(&samples[a..b], cancel)?;
            if !heard.text.trim().is_empty() {
                langs.push(heard.lang);
                segments.push(Segment {
                    start: a as f64 / SAMPLE_RATE as f64,
                    end: b as f64 / SAMPLE_RATE as f64,
                    text: heard.text,
                });
            }
        }
        progress(i + 1, windows.len());
    }
    let language = match langs.first() {
        None => "unknown".to_string(),
        Some(first) if langs.iter().all(|l| l == first) => first.to_string(),
        Some(_) => "mixed".to_string(),
    };
    let transcript = Transcript {
        segments,
        audio_seconds: samples.len() as f64 / SAMPLE_RATE as f64,
        load_ms: 0,
        run_ms: t.elapsed().as_millis(),
        provider: "cpu".to_string(),
    };
    Ok((transcript, language))
}

fn srt_time(s: f64) -> String {
    let ms = (s.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// The segments as a SubRip file, optionally with a speaker prefix
/// (`Name: line`, the form the caption importer reads as the speaker).
pub fn to_srt(segments: &[Segment], speaker: Option<&str>) -> String {
    let mut out = String::new();
    for (i, s) in segments.iter().enumerate() {
        let line = match speaker {
            Some(who) if !who.is_empty() => format!("{who}: {}", s.text),
            _ => s.text.clone(),
        };
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(s.start), srt_time(s.end), line));
    }
    out
}

/// The segments as dialogue cues on the timeline, starting at `start_frame`,
/// for `Editor::sync_directed_dialogue` (captions + that speaker's lip sync).
pub fn to_dialogue_cues(
    segments: &[Segment],
    speaker: &str,
    fps: f64,
    start_frame: u32,
) -> Vec<crate::director::DialogueCue> {
    let fps = fps.max(1.0);
    segments
        .iter()
        .map(|s| {
            let a = start_frame + (s.start * fps).round() as u32;
            let b = (start_frame + (s.end * fps).round() as u32).max(a + 1);
            crate::director::DialogueCue {
                speaker: speaker.to_string(),
                text: s.text.clone(),
                start_frame: a,
                end_frame: b,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_filters_match_librosa_shape_and_normalisation() {
        let f = mel_filters();
        assert_eq!(f.len(), 80 * 201);
        // Every filter has some weight, and the lowest one sits in the first bins.
        for m in 0..80 {
            assert!(f[m * 201..(m + 1) * 201].iter().any(|&v| v > 0.0), "empty filter {m}");
        }
        assert!(f[1] > 0.0);
        // librosa's first filter peak (bin 1) is ≈ 0.0248 for these settings.
        assert!((f[1] - 0.02483).abs() < 0.002, "{}", f[1]);
    }

    #[test]
    fn log_mel_of_silence_is_flat_and_of_a_tone_peaks_in_the_right_band() {
        let silence = log_mel(&vec![0.0; 16_000]);
        assert_eq!(silence.len(), 80 * 3000);
        let tone: Vec<f32> = (0..16_000)
            .map(|i| (i as f32 / 16_000.0 * 1000.0 * std::f32::consts::TAU).sin() * 0.3)
            .collect();
        let m = log_mel(&tone);
        // Frame 50 (0.5 s): the loudest mel bin is near 1 kHz (mel bin ~ 23 of 80).
        let col: Vec<f32> = (0..80).map(|b| m[b * 3000 + 50]).collect();
        let peak = argmax(&col);
        assert!((18..=28).contains(&peak), "peak bin {peak}");
    }

    #[test]
    fn the_assistant_prefers_the_multilingual_model_and_explains_when_none_is_installed() {
        assert_eq!(pick_whisper(true, true), Ok(WhisperChoice::Multilingual));
        assert_eq!(pick_whisper(true, false), Ok(WhisperChoice::Multilingual));
        assert_eq!(pick_whisper(false, true), Ok(WhisperChoice::BaseEn));
        let none = pick_whisper(false, false).unwrap_err();
        assert!(none.contains("AI \u{25b8} Models") && none.contains("base.en"), "{none}");
    }

    #[test]
    fn multilingual_windows_cover_the_sound_and_cut_in_a_quiet_spot() {
        let sr = SAMPLE_RATE as usize;
        // Short sounds go whole; empty has nothing to hear.
        assert_eq!(plan_windows(&vec![0.1; 20 * sr]), vec![(0, 20 * sr)]);
        assert!(plan_windows(&[]).is_empty());
        // 70 s of loud noise with one silent half-second at 23 s.
        let mut s = vec![0.5f32; 70 * sr];
        for v in &mut s[23 * sr..23 * sr + sr / 2] {
            *v = 0.0;
        }
        let w = plan_windows(&s);
        assert_eq!(w.first().unwrap().0, 0);
        assert_eq!(w.last().unwrap().1, s.len());
        for pair in w.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "windows touch: {w:?}");
        }
        assert!(w.iter().all(|(a, b)| b - a <= 29 * sr), "{w:?}");
        let cut = w[0].1;
        assert!((23 * sr..23 * sr + sr / 2).contains(&cut), "cut {cut} falls in the silence");
    }

    #[test]
    fn multilingual_transcription_of_silence_needs_no_model_and_is_empty() {
        // Silence is never sent to Whisper, so this runs with no model installed.
        let clip = buzz_audio::Clip::new("quiet", 16_000, 1, vec![0.0; 16_000 * 3]).unwrap();
        let (t, lang) = transcribe_clip_multi(&clip, &mut |_, _| {}, &|| false).unwrap();
        assert!(t.segments.is_empty() && lang == "unknown");
        assert!((t.audio_seconds - 3.0).abs() < 0.01);
    }

    #[test]
    fn multilingual_special_tokens_come_from_added_tokens_json() {
        let json = r#"{"<|startoftranscript|>": 50258, "<|en|>": 50259, "<|hi|>": 50276,
            "<|transcribe|>": 50359, "<|notimestamps|>": 50363}"#;
        let t = MultiTokens::parse(json).unwrap();
        assert_eq!((t.sot, t.eot, t.en, t.hi, t.transcribe, t.no_ts), (50258, 50257, 50259, 50276, 50359, 50363));
        assert!(MultiTokens::parse("{}").is_err());
    }

    #[test]
    fn srt_and_cues_from_segments() {
        let segs = vec![
            Segment { start: 0.0, end: 1.5, text: "Hello there.".into() },
            Segment { start: 61.25, end: 63.0, text: "Rain.".into() },
        ];
        let srt = to_srt(&segs, Some("Ramu"));
        assert!(srt.starts_with("1\n00:00:00,000 --> 00:00:01,500\nRamu: Hello there.\n"), "{srt}");
        assert!(srt.contains("00:01:01,250 --> 00:01:03,000"));
        let cues = to_dialogue_cues(&segs, "Ramu", 24.0, 10);
        assert_eq!((cues[0].start_frame, cues[0].end_frame), (10, 46));
        assert_eq!(cues[1].speaker, "Ramu");
    }

    #[test]
    fn timestamp_rules_force_a_timestamp_first_and_pairs_after() {
        let mut l = vec![0.0f32; 51_864];
        l[100] = 10.0;
        apply_timestamp_rules(&mut l, &[]);
        let first = argmax(&l) as i64;
        assert!(first >= TS_BEGIN && first <= TS_BEGIN + 50);
        // After text then one timestamp: only a timestamp or EOT.
        let mut l = vec![0.0f32; 51_864];
        l[100] = 10.0;
        apply_timestamp_rules(&mut l, &[TS_BEGIN, 100, TS_BEGIN + 40]);
        let t = argmax(&l) as i64;
        assert!(t >= TS_BEGIN + 40 || t == EOT, "{t}");
    }
}
