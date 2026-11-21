//! **Hearing the speech sounds of any language** — the ear behind lip sync
//! (user 2026-11-21: "how does it identify the sound and place the mouth
//! shapes? Animate does it much better than what we have").
//!
//! Rhubarb, the lip-sync tool used before, is trained on English: on Hindi
//! dialogue it heard whole sentences as one or two shapes, and the rest was
//! guessed from loudness. This runs Meta's **wav2vec2 XLSR-53 espeak**
//! phoneme recogniser (Apache-2.0; trained on speech in many languages,
//! answering in IPA) on the Mac itself, through ONNX Runtime: every 20 ms of
//! the recording it says which speech sound is there — `m`, `aː`, `ʈ`, `ʃ` —
//! and those become the mouth shapes.
//!
//! The model is the 4-bit ONNX export of `qnighy/wav2vec2-xlsr-53-espeak-cv-
//! ft-ONNX` (241 MB) with its `vocab.json`, kept in the models folder
//! ([`crate::models::models_dir`]) as [`MODEL_FILE`] and [`VOCAB_FILE`].

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow, bail};

use crate::onnx::OnnxSession;

/// The model, in the models folder.
pub const MODEL_FILE: &str = "wav2vec2-xlsr53-espeak-q4.onnx";
/// Its token list (`{"<pad>": 0, "n": 4, ...}`), beside it.
pub const VOCAB_FILE: &str = "wav2vec2-xlsr53-espeak-vocab.json";
/// The sample rate the model listens at.
pub const SAMPLE_RATE: u32 = 16_000;
/// The model is fed this much at a time; a whole film's track is many.
const CHUNK_SECONDS: usize = 20;

/// One speech sound heard: from `start` to `end`, seconds from the start of
/// what was heard, and the sound in IPA (`"aː"`, `"m"`, `"tʃ"`).
#[derive(Debug, Clone, PartialEq)]
pub struct Phone {
    pub start: f64,
    pub end: f64,
    pub ipa: String,
}

/// Where the model and its tokens are, when both are there.
pub fn model_paths() -> Option<(PathBuf, PathBuf)> {
    let dir = crate::models::models_dir();
    let (model, vocab) = (dir.join(MODEL_FILE), dir.join(VOCAB_FILE));
    (model.is_file() && vocab.is_file()).then_some((model, vocab))
}

/// Whether the recogniser can run here.
pub fn available() -> bool {
    model_paths().is_some()
}

/// The recogniser: the model, loaded once, and its tokens by id.
pub struct PhonemeRecognizer {
    session: OnnxSession,
    tokens: Vec<String>,
    blank: usize,
}

impl PhonemeRecognizer {
    pub fn load() -> Result<Self> {
        let (model, vocab) = model_paths().context("the phoneme model is not in the models folder")?;
        let text = std::fs::read_to_string(&vocab).with_context(|| format!("reading {}", vocab.display()))?;
        let map: std::collections::HashMap<String, usize> = serde_json::from_str(&text).context("the phoneme vocabulary is not JSON")?;
        let size = map.values().copied().max().map_or(0, |m| m + 1);
        let mut tokens = vec![String::new(); size];
        for (token, id) in map {
            tokens[id] = token;
        }
        let blank = tokens.iter().position(|t| t == "<pad>").unwrap_or(0);
        // The 4-bit file uses MatMulNBits, which the CPU provider runs and
        // CoreML does not: on the CPU, every op is supported.
        let session = OnnxSession::load_cpu(&model)?;
        Ok(Self { session, tokens, blank })
    }

    /// **The speech sounds in `samples`** (mono, [`SAMPLE_RATE`]), in order.
    /// `cancelled` is polled between chunks.
    pub fn hear(&mut self, samples: &[f32], cancelled: &dyn Fn() -> bool) -> Result<Vec<Phone>> {
        let mut out: Vec<Phone> = Vec::new();
        let chunk = CHUNK_SECONDS * SAMPLE_RATE as usize;
        let mut offset = 0usize;
        while offset < samples.len() {
            if cancelled() {
                bail!("cancelled");
            }
            let piece = &samples[offset..(offset + chunk).min(samples.len())];
            // Under a tenth of a second is too short for the model's window.
            if piece.len() < SAMPLE_RATE as usize / 10 {
                break;
            }
            let at = offset as f64 / SAMPLE_RATE as f64;
            let seconds = piece.len() as f64 / SAMPLE_RATE as f64;
            let ids = self.frames(piece)?;
            let step = seconds / ids.len().max(1) as f64;
            // CTC: a sound starts where its token first appears after a
            // different one (blanks between repeats separate them).
            let mut previous = self.blank;
            for (i, id) in ids.iter().copied().enumerate() {
                if id != previous && id != self.blank {
                    let ipa = self.tokens.get(id).cloned().unwrap_or_default();
                    if !ipa.starts_with('<') && !ipa.is_empty() {
                        out.push(Phone { start: at + i as f64 * step, end: at + (i + 1) as f64 * step, ipa });
                    }
                }
                if id != self.blank {
                    if let Some(last) = out.last_mut()
                        && id == previous
                    {
                        last.end = at + (i + 1) as f64 * step;
                    }
                }
                previous = id;
            }
            offset += chunk;
        }
        Ok(out)
    }

    /// The most likely token on each 20 ms frame of `piece`.
    fn frames(&mut self, piece: &[f32]) -> Result<Vec<usize>> {
        // The feature extractor's normalisation: zero mean, unit variance.
        let n = piece.len() as f32;
        let mean = piece.iter().sum::<f32>() / n;
        let var = piece.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / n;
        let scale = 1.0 / (var + 1e-7).sqrt();
        let input: Vec<f32> = piece.iter().map(|s| (s - mean) * scale).collect();
        let array = ndarray::Array2::from_shape_vec((1, input.len()), input)?;
        let tensor = ort::value::Tensor::from_array(array).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .session
            .inner_mut()
            .run(ort::inputs!["input_values" => tensor])
            .map_err(|e| anyhow!("the phoneme model failed: {e}"))?;
        let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        let vocab = *shape.last().context("no output")? as usize;
        let frames = data.len() / vocab.max(1);
        Ok((0..frames)
            .map(|f| {
                let row = &data[f * vocab..(f + 1) * vocab];
                row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(self.blank, |(i, _)| i)
            })
            .collect())
    }
}

/// **The recogniser, loaded once** and kept: loading 241 MB takes a few
/// seconds, every lip sync after the first starts at once.
pub fn hear(samples: &[f32], cancelled: &dyn Fn() -> bool) -> Result<Vec<Phone>> {
    static SHARED: OnceLock<Mutex<Option<PhonemeRecognizer>>> = OnceLock::new();
    let slot = SHARED.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().map_err(|_| anyhow!("the phoneme recogniser is busy elsewhere"))?;
    if guard.is_none() {
        *guard = Some(PhonemeRecognizer::load()?);
    }
    guard.as_mut().expect("loaded").hear(samples, cancelled)
}

/// **Which of Animate's twelve visemes an IPA sound shows** — in the order
/// of `buzz_audio::ANIMATE_VISEMES` (Neutral, Ah, D, Ee, F, L, M, Oh, R, S,
/// Uh, W-Oo). `None` for a sound the mouth does not shape on its own (`h`,
/// a glottal stop): it keeps the shape around it.
pub fn animate_viseme(ipa: &str) -> Option<usize> {
    const AH: usize = 1;
    const D: usize = 2;
    const EE: usize = 3;
    const F: usize = 4;
    const L: usize = 5;
    const M: usize = 6;
    const OH: usize = 7;
    const R: usize = 8;
    const S: usize = 9;
    const UH: usize = 10;
    const WOO: usize = 11;
    // Affricates and sibilant clusters first: their first letter alone (t, d)
    // would read as D.
    for (lead, viseme) in [("tʃ", S), ("dʒ", S), ("ts", S), ("dz", S), ("tɕ", S), ("dʑ", S), ("tS", S), ("dZ", S)] {
        if ipa.starts_with(lead) {
            return Some(viseme);
        }
    }
    let first = ipa.chars().next()?;
    Some(match first {
        'm' | 'b' | 'p' | 'ɱ' | 'ʙ' | 'ɓ' => M,
        'f' | 'v' | 'ʋ' | 'ɸ' | 'β' => F,
        'w' | 'u' | 'ʊ' | 'ʍ' | 'ɥ' | 'ʉ' | 'ɯ' | 'y' | 'ʏ' => WOO,
        'o' | 'ɔ' | 'ɒ' | 'ø' | 'œ' | 'ɵ' | 'ɶ' => OH,
        'a' | 'ɑ' | 'æ' | 'ä' => AH,
        'ə' | 'ʌ' | 'ɐ' | 'ɜ' | 'ɚ' | 'ɘ' | 'ɝ' | 'ɞ' => UH,
        'i' | 'ɪ' | 'e' | 'ɛ' | 'j' | 'ɨ' | 'ᵻ' | 'ʝ' | 'ç' | 'ɟ' | 'c' => EE,
        'l' | 'ɫ' | 'ɭ' | 'ɬ' | 'ʎ' | 'ɮ' | 'θ' | 'ð' => L,
        'r' | 'ɹ' | 'ɾ' | 'ɽ' | 'ʁ' | 'ɻ' | 'ʀ' | 'ɺ' => R,
        's' | 'z' | 'ʃ' | 'ʒ' | 'ʂ' | 'ʐ' | 'ɕ' | 'ʑ' | 'x' | 'ɣ' | 'χ' | 'S' | 'Z' => S,
        't' | 'd' | 'n' | 'ʈ' | 'ɖ' | 'ɳ' | 'k' | 'ɡ' | 'g' | 'ŋ' | 'q' | 'ɲ' | 'ɴ' | 'N' => D,
        // h, ɦ, ʔ, ħ, ʕ: no shape of their own.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_sounds_land_on_animates_visemes() {
        let v = |s: &str| animate_viseme(s).map(|i| ["Neutral", "Ah", "D", "Ee", "F", "L", "M", "Oh", "R", "S", "Uh", "W-Oo"][i]);
        for (ipa, want) in [
            ("m", "M"), ("b", "M"), ("pʰ", "M"), ("f", "F"), ("ʋ", "F"), ("uː", "W-Oo"), ("w", "W-Oo"),
            ("oː", "Oh"), ("aː", "Ah"), ("ɑ̃", "Ah"), ("ə", "Uh"), ("ɐ", "Uh"), ("iː", "Ee"), ("e", "Ee"),
            ("l", "L"), ("r", "R"), ("ɾ", "R"), ("s", "S"), ("ʃ", "S"), ("tʃ", "S"), ("dʒ", "S"),
            ("ʈ", "D"), ("ɖ", "D"), ("n", "D"), ("k", "D"), ("t", "D"),
        ] {
            assert_eq!(v(ipa), Some(want), "{ipa}");
        }
        assert_eq!(animate_viseme("h"), None, "h keeps the shape around it");
        assert_eq!(animate_viseme("ɦ"), None);
    }
}
