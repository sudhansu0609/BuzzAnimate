//! **SAM 3 image model** (concept segmentation) on ONNX Runtime: a noun phrase ("table",
//! "shelf unit") gives every instance of it in a picture, each with its whole mask and a score.
//!
//! The usls export (github.com/jamjamjon/assets, release `sam3`): `vision-encoder.onnx`
//! (`images` [1,3,1008,1008], -1..1 -> `fpn_feat_0/1/2`, `fpn_pos_2`), `text-encoder.onnx`
//! (CLIP ids `input_ids` / `attention_mask` [n,32] -> `text_features`, `text_mask`) and
//! `decoder.onnx` (the four maps + the prompt -> `pred_masks` [1,200,288,288] logits,
//! `pred_boxes` [1,200,4] xyxy 0..1, `pred_logits` [1,200], `presence_logits` [1,1]). An
//! instance's score is sigmoid(logit) x sigmoid(presence). The picture is encoded once; each
//! phrase costs a text encode and a decode.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use image::{GrayImage, Luma, RgbaImage};

use crate::models::{self, ModelId};
use crate::onnx::OnnxSession;

/// The encoder's input side (the picture is stretched to it, not letterboxed).
const SIZE: u32 = 1008;
/// CLIP's sequence length.
const SEQ: usize = 32;
const START: i64 = 49406;
const END: i64 = 49407;

/// A picture as SAM 3's decoder needs it.
pub struct Sam3Image {
    maps: [(Vec<usize>, Vec<f32>); 4],
    size: (u32, u32),
}

/// One instance of a phrase.
pub struct Sam3Instance {
    /// sigmoid(logit) x sigmoid(presence), 0..1.
    pub score: f32,
    /// Picture size, 0 / 255.
    pub mask: GrayImage,
}

fn session(id: ModelId) -> Option<std::sync::Arc<std::sync::Mutex<OnnxSession>>> {
    // CoreML refuses these graphs like SAM 2's; the CPU runs them.
    if cfg!(target_os = "macos") {
        crate::onnx::pooled_cpu(id)
    } else {
        crate::onnx::pooled(id)
    }
}

/// Are the three models (and the tokenizer files) installed?
pub fn sam3_installed() -> bool {
    [ModelId::Sam3Vision, ModelId::Sam3Text, ModelId::Sam3Decoder].iter().all(|&id| models::status(id).is_ready())
}

/// Encode `img` once for any number of phrases. `None` when the vision model is not installed.
pub fn sam3_encode(img: &RgbaImage) -> Result<Option<Sam3Image>> {
    let Some(session) = session(ModelId::Sam3Vision) else {
        return Ok(None);
    };
    let small = image::imageops::resize(img, SIZE, SIZE, image::imageops::FilterType::Triangle);
    let n = (SIZE * SIZE) as usize;
    let mut chw = vec![0f32; 3 * n];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            chw[c * n + i] = p[c] as f32 / 127.5 - 1.0;
        }
    }
    let input = ndarray::Array4::from_shape_vec((1, 3, SIZE as usize, SIZE as usize), chw).context("SAM 3 image tensor")?;
    let input = ort::value::Tensor::from_array(input).map_err(|e| anyhow::anyhow!("SAM 3 image tensor: {e}"))?;
    let mut sess = session.lock().unwrap();
    let out = sess.inner_mut().run(ort::inputs!["images" => input]).map_err(|e| anyhow::anyhow!("SAM 3 vision encoder: {e}"))?;
    let map = |name: &str| -> Result<(Vec<usize>, Vec<f32>)> {
        let (shape, data) = out[name].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("SAM 3 {name}: {e}"))?;
        Ok((shape.iter().map(|&d| d.max(0) as usize).collect(), data.to_vec()))
    };
    let maps = [map("fpn_feat_0")?, map("fpn_feat_1")?, map("fpn_feat_2")?, map("fpn_pos_2")?];
    Ok(Some(Sam3Image { maps, size: img.dimensions() }))
}

/// **Every instance of `phrase`** scoring at least `threshold`, best first, at most `max`.
/// `None` when the text or decoder model is not installed.
pub fn sam3_find(image: &Sam3Image, phrase: &str, threshold: f32, max: usize) -> Result<Option<Vec<Sam3Instance>>> {
    let (Some(text), Some(decoder), Some(tok)) = (session(ModelId::Sam3Text), session(ModelId::Sam3Decoder), tokenizer()) else {
        return Ok(None);
    };
    let ids = tok.encode(phrase);
    let mask: Vec<i64> = (0..SEQ).map(|i| (i < ids.len()) as i64).collect();
    let ids: Vec<i64> = (0..SEQ).map(|i| ids.get(i).copied().unwrap_or(END)).collect();
    let ids = ort::value::Tensor::from_array(ndarray::Array2::from_shape_vec((1, SEQ), ids)?).map_err(|e| anyhow::anyhow!("SAM 3 ids: {e}"))?;
    let mask = ort::value::Tensor::from_array(ndarray::Array2::from_shape_vec((1, SEQ), mask)?).map_err(|e| anyhow::anyhow!("SAM 3 mask: {e}"))?;
    let (feat_shape, feat, prompt_mask) = {
        let mut sess = text.lock().unwrap();
        let out = sess.inner_mut().run(ort::inputs!["input_ids" => ids, "attention_mask" => mask]).map_err(|e| anyhow::anyhow!("SAM 3 text encoder: {e}"))?;
        let (shape, feat) = out["text_features"].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("SAM 3 text_features: {e}"))?;
        let (_, m) = out["text_mask"].try_extract_tensor::<bool>().map_err(|e| anyhow::anyhow!("SAM 3 text_mask: {e}"))?;
        (shape.iter().map(|&d| d.max(0) as usize).collect::<Vec<_>>(), feat.to_vec(), m.to_vec())
    };
    let tensor4 = |(shape, data): &(Vec<usize>, Vec<f32>)| -> Result<ort::value::Tensor<f32>> {
        let a = ndarray::Array4::from_shape_vec((shape[0], shape[1], shape[2], shape[3]), data.clone())?;
        ort::value::Tensor::from_array(a).map_err(|e| anyhow::anyhow!("SAM 3 map: {e}"))
    };
    let [f0, f1, f2, p2] = &image.maps;
    let prompt = ort::value::Tensor::from_array(ndarray::Array3::from_shape_vec((feat_shape[0], feat_shape[1], feat_shape[2]), feat)?).map_err(|e| anyhow::anyhow!("SAM 3 prompt: {e}"))?;
    let prompt_mask = ort::value::Tensor::from_array(ndarray::Array2::from_shape_vec((1, prompt_mask.len()), prompt_mask)?).map_err(|e| anyhow::anyhow!("SAM 3 prompt mask: {e}"))?;
    let mut sess = decoder.lock().unwrap();
    let out = sess
        .inner_mut()
        .run(ort::inputs![
            "fpn_feat_0" => tensor4(f0)?,
            "fpn_feat_1" => tensor4(f1)?,
            "fpn_feat_2" => tensor4(f2)?,
            "fpn_pos_2" => tensor4(p2)?,
            "prompt_features" => prompt,
            "prompt_mask" => prompt_mask,
        ])
        .map_err(|e| anyhow::anyhow!("SAM 3 decoder: {e}"))?;
    let (mshape, masks) = out["pred_masks"].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("SAM 3 pred_masks: {e}"))?;
    let (_, logits) = out["pred_logits"].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("SAM 3 pred_logits: {e}"))?;
    let (_, presence) = out["presence_logits"].try_extract_tensor::<f32>().map_err(|e| anyhow::anyhow!("SAM 3 presence_logits: {e}"))?;
    let sigmoid = |x: f32| 1.0 / (1.0 + (-x).exp());
    let present = sigmoid(presence.first().copied().unwrap_or(0.0));
    let (mh, mw) = (mshape[2].max(0) as usize, mshape[3].max(0) as usize);
    let mut ranked: Vec<(usize, f32)> = logits.iter().enumerate().map(|(q, &l)| (q, sigmoid(l) * present)).filter(|(_, s)| *s >= threshold).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(max);
    let (w, h) = image.size;
    Ok(Some(
        ranked
            .into_iter()
            .map(|(q, score)| Sam3Instance { score, mask: upsample(&masks[q * mh * mw..(q + 1) * mh * mw], (mw, mh), (w, h)) })
            .collect(),
    ))
}

/// A mask of logits at the model's grid, bilinear to the picture's size, set where above 0.
fn upsample(logits: &[f32], grid: (usize, usize), size: (u32, u32)) -> GrayImage {
    let (gw, gh) = grid;
    let (w, h) = size;
    let (sx, sy) = (gw as f32 / w as f32, gh as f32 / h as f32);
    GrayImage::from_fn(w, h, |x, y| {
        let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (gw - 1) as f32);
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (gh - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(gw - 1), (y0 + 1).min(gh - 1));
        let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| logits[y * gw + x];
        let top = at(x0, y0) * (1.0 - ax) + at(x1, y0) * ax;
        let bottom = at(x0, y1) * (1.0 - ax) + at(x1, y1) * ax;
        Luma([if top * (1.0 - ay) + bottom * ay > 0.0 { 255 } else { 0 }])
    })
}

// ------------------------------------------------------------------ CLIP tokenizer

/// CLIP's byte-level BPE (`vocab.json` + `merges.txt` beside the text encoder).
struct ClipTokenizer {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    bytes: [char; 256],
}

fn tokenizer() -> Option<&'static ClipTokenizer> {
    static TOK: OnceLock<Option<ClipTokenizer>> = OnceLock::new();
    TOK.get_or_init(|| {
        let dir = models::models_dir();
        let vocab = std::fs::read_to_string(dir.join(SAM3_VOCAB)).ok()?;
        let merges = std::fs::read_to_string(dir.join(SAM3_MERGES)).ok()?;
        ClipTokenizer::new(&vocab, &merges)
    })
    .as_ref()
}

/// The tokenizer files' names in the models folder.
pub const SAM3_VOCAB: &str = "sam3-vocab.json";
pub const SAM3_MERGES: &str = "sam3-merges.txt";

impl ClipTokenizer {
    fn new(vocab: &str, merges: &str) -> Option<Self> {
        let vocab: HashMap<String, i64> = serde_json::from_str(vocab).ok()?;
        let ranks = merges
            .lines()
            .filter(|l| !l.starts_with("#version") && !l.trim().is_empty())
            .enumerate()
            .filter_map(|(i, l)| l.split_once(' ').map(|(a, b)| ((a.to_string(), b.to_string()), i)))
            .collect();
        Some(Self { vocab, ranks, bytes: byte_chars() })
    }

    /// `<start>` + the phrase's tokens + `<end>`, at most [`SEQ`].
    fn encode(&self, text: &str) -> Vec<i64> {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let mut ids = vec![START];
        for word in pieces(&text) {
            let chars: String = word.bytes().map(|b| self.bytes[b as usize]).collect();
            for token in self.bpe(&chars) {
                if let Some(&id) = self.vocab.get(&token) {
                    ids.push(id);
                }
            }
        }
        ids.truncate(SEQ - 1);
        ids.push(END);
        ids
    }

    fn bpe(&self, word: &str) -> Vec<String> {
        let chars: Vec<char> = word.chars().collect();
        let Some((last, rest)) = chars.split_last() else { return Vec::new() };
        let mut parts: Vec<String> = rest.iter().map(|c| c.to_string()).collect();
        parts.push(format!("{last}</w>"));
        while parts.len() > 1 {
            let best = (0..parts.len() - 1).filter_map(|i| self.ranks.get(&(parts[i].clone(), parts[i + 1].clone())).map(|&r| (r, i))).min();
            let Some((_, i)) = best else { break };
            let (a, b) = (parts[i].clone(), parts[i + 1].clone());
            // Merge every occurrence of the pair, left to right.
            let mut merged = Vec::with_capacity(parts.len());
            let mut k = 0;
            while k < parts.len() {
                if k + 1 < parts.len() && parts[k] == a && parts[k + 1] == b {
                    merged.push(format!("{a}{b}"));
                    k += 2;
                } else {
                    merged.push(parts[k].clone());
                    k += 1;
                }
            }
            parts = merged;
        }
        parts
    }
}

/// CLIP's pre-tokenizer: contractions, runs of letters, single digits, runs of other symbols.
fn pieces(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' {
            let rest: String = chars[i + 1..].iter().take(2).collect();
            if let Some(tail) = ["ll", "re", "ve", "s", "t", "m", "d"].iter().find(|t| rest.starts_with(**t)) {
                out.push(format!("'{tail}"));
                i += 1 + tail.len();
                continue;
            }
        }
        let start = i;
        if c.is_alphabetic() {
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
        } else if c.is_numeric() {
            i += 1;
        } else {
            while i < chars.len() && !chars[i].is_whitespace() && !chars[i].is_alphabetic() && !chars[i].is_numeric() {
                i += 1;
            }
        }
        out.push(chars[start..i].iter().collect());
    }
    out
}

/// GPT-2's byte -> printable char table.
fn byte_chars() -> [char; 256] {
    let mut table = ['\0'; 256];
    let printable = |b: u32| (33..=126).contains(&b) || (161..=172).contains(&b) || (174..=255).contains(&b);
    let mut extra = 0u32;
    for b in 0..256u32 {
        table[b as usize] = if printable(b) {
            char::from_u32(b).unwrap_or('?')
        } else {
            extra += 1;
            char::from_u32(255 + extra).unwrap_or('?')
        };
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_merged_by_rank_and_ends_with_the_word_mark() {
        let vocab = r#"{"t":1,"a":2,"b":3,"l":4,"e</w>":5,"ta":6,"bl":7,"table</w>":8,"tab":9,"le</w>":10}"#;
        let merges = "#version: 0.2\nt a\nl e</w>\nta b\ntab le</w>\n";
        let tok = ClipTokenizer::new(vocab, merges).unwrap();
        assert_eq!(tok.bpe("table"), vec!["table</w>".to_string()]);
        assert_eq!(tok.encode("  TABLE "), vec![START, 8, END]);
    }

    /// Against Hugging Face's CLIP tokenizer (`tokenizer.json` of the same export).
    #[test]
    fn the_installed_tokenizer_gives_clips_ids() {
        let Some(tok) = tokenizer() else { return };
        let cases: [(&str, &[i64]); 6] = [
            ("table", &[49406, 2175, 49407]),
            ("shelf unit", &[49406, 10955, 5695, 49407]),
            ("small cup/pot", &[49406, 2442, 1937, 270, 5168, 49407]),
            ("pile of folded cloth", &[49406, 13930, 539, 25233, 14559, 49407]),
            ("man's hut", &[49406, 786, 568, 16503, 49407]),
            ("2 boxes", &[49406, 273, 8350, 49407]),
        ];
        for (text, ids) in cases {
            assert_eq!(tok.encode(text), ids, "{text}");
        }
    }

    #[test]
    fn the_text_is_cut_into_words_digits_and_symbols() {
        assert_eq!(pieces("shelf unit"), vec!["shelf", "unit"]);
        assert_eq!(pieces("cup/pot 12"), vec!["cup", "/", "pot", "1", "2"]);
        assert_eq!(pieces("man's"), vec!["man", "'s"]);
    }

    #[test]
    fn bytes_map_to_printable_chars() {
        let t = byte_chars();
        assert_eq!(t[b'a' as usize], 'a');
        assert_eq!(t[b' ' as usize], '\u{120}');
    }
}
