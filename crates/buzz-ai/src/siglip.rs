//! SigLIP vision encoder: image embeddings for reference-to-asset visual
//! similarity (C10 scene-from-image matching).
//!
//! `ModelId::SigLip` (Xenova/siglip-base-patch16-224, quantized). Preprocessing
//! squash-resizes to 224x224 — SigLIP's own processor does not letterbox —
//! and normalises each RGB channel to `[-1, 1]`. The embedding is whichever
//! output the graph exposes for it (`pooler_output` when there is one,
//! otherwise the mean of `last_hidden_state` over the patch axis), then
//! L2-normalised so cosine similarity between two embeddings is a plain dot
//! product.

use anyhow::{Context, Result};
use image::{imageops::FilterType, RgbaImage};

use crate::models::{models_dir, ModelId};
use crate::onnx::OnnxSession;

const SIZE: u32 = 224;

/// RGBA -> `[1, 3, 224, 224]` NCHW f32. Bilinear resize to 224x224 (no
/// letterbox padding), RGB channels only (alpha dropped), each channel
/// `(x/255 - 0.5) / 0.5` so every value lands in `[-1, 1]`.
pub fn preprocess(img: &RgbaImage) -> Vec<f32> {
    let resized = image::imageops::resize(img, SIZE, SIZE, FilterType::Triangle);
    let plane = (SIZE * SIZE) as usize;
    let mut data = vec![0.0f32; 3 * plane];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let idx = (y * SIZE + x) as usize;
            let p = resized.get_pixel(x, y);
            data[idx] = (p[0] as f32 / 255.0 - 0.5) / 0.5;
            data[plane + idx] = (p[1] as f32 / 255.0 - 0.5) / 0.5;
            data[2 * plane + idx] = (p[2] as f32 / 255.0 - 0.5) / 0.5;
        }
    }
    data
}

fn l2_normalise(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Runs one preprocessed image through an already-loaded session, returning
/// its L2-normalised embedding.
///
/// Prefers `pooler_output` when the graph exposes one (inspected by name at
/// call time, since it costs nothing next to the inference itself);
/// otherwise falls back to the mean of `last_hidden_state` — or, failing
/// either name, the first output — pooled the same way over its token axis.
fn run_one(sess: &mut OnnxSession, nchw: &[f32]) -> Result<Vec<f32>> {
    let input_array =
        ndarray::Array4::from_shape_vec((1, 3, SIZE as usize, SIZE as usize), nchw.to_vec())
            .context("Failed to reshape image into (1, 3, 224, 224)")?;
    let input_tensor = ort::value::Tensor::from_array(input_array)
        .map_err(|e| anyhow::anyhow!("Failed to create tensor: {e}"))?;

    let has_pooler = sess.inner().outputs().iter().any(|o| o.name() == "pooler_output");

    let outputs = sess
        .inner_mut()
        .run(ort::inputs![input_tensor])
        .map_err(|e| anyhow::anyhow!("SigLIP inference failed: {e}"))?;

    let mut embedding = if has_pooler {
        let (_shape, slice) = outputs
            .get("pooler_output")
            .ok_or_else(|| anyhow::anyhow!("SigLIP: pooler_output missing from outputs"))?
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Failed to extract pooler_output: {e}"))?;
        slice.to_vec()
    } else {
        let value = outputs.get("last_hidden_state").unwrap_or(&outputs[0]);
        let (shape, slice) = value
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Failed to extract SigLIP output: {e}"))?;
        let hidden = shape.last().copied().unwrap_or(0).max(0) as usize;
        if hidden == 0 || slice.len() % hidden != 0 {
            // An unexpected shape: hand back the raw values rather than
            // guessing at a pooling axis that isn't there.
            slice.to_vec()
        } else {
            let seq_len = slice.len() / hidden;
            let mut mean = vec![0.0f32; hidden];
            for t in 0..seq_len {
                for h in 0..hidden {
                    mean[h] += slice[t * hidden + h];
                }
            }
            for v in mean.iter_mut() {
                *v /= seq_len as f32;
            }
            mean
        }
    };

    l2_normalise(&mut embedding);
    Ok(embedding)
}

/// Embeds each image with SigLIP, returning one L2-normalised vector per
/// image, in the same order.
///
/// Loaded on the **CPU** execution provider outright — see [`crate::onnx::pooled_cpu`].
/// This model's quantized graph was found (via the ignored
/// `tests/siglip_embed.rs`) to crash the whole process with a native access
/// violation when DirectML tries to *commit* it, not merely fail to run one
/// op, so there is no `Result` to fall back from the way `OnnxSession::load`
/// does for other models; CPU is the only execution provider proven safe for
/// it here.
///
/// Errors — rather than silently skipping — when the model is not
/// installed; callers that want a graceful skip should check
/// `buzz_ai::models::status(ModelId::SigLip)` first, as the Scene-from-Image
/// task does.
pub fn embed(images: &[RgbaImage]) -> Result<Vec<Vec<f32>>> {
    let session = match crate::onnx::pooled_cpu(ModelId::SigLip) {
        Some(s) => s,
        None => {
            let model_path = models_dir().join("vision_model_quantized.onnx");
            if !model_path.exists() {
                anyhow::bail!(
                    "SigLIP model is not installed. Download it first via AI Models dialog."
                );
            }
            std::sync::Arc::new(std::sync::Mutex::new(OnnxSession::load_cpu(&model_path)?))
        }
    };

    if std::env::var_os("BUZZ_SIGLIP_DEBUG_IO").is_some() {
        let guard = session.lock().unwrap();
        eprintln!("SigLIP session execution provider: {}", guard.ep_name());
        for inp in guard.inner().inputs() {
            eprintln!("  input  {} : {}", inp.name(), inp.dtype());
        }
        for out in guard.inner().outputs() {
            eprintln!("  output {} : {}", out.name(), out.dtype());
        }
    }

    let mut guard = session.lock().unwrap();
    images.iter().map(|img| run_one(&mut guard, &preprocess(img))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_produces_a_normalised_nchw_tensor() {
        let img = RgbaImage::from_fn(37, 51, |x, y| {
            image::Rgba([(x * 7 % 256) as u8, (y * 5 % 256) as u8, 128, 255])
        });
        let data = preprocess(&img);
        assert_eq!(data.len(), 3 * 224 * 224, "1x3x224x224 flattened");
        assert!(
            data.iter().all(|&v| (-1.0..=1.0).contains(&v)),
            "every value should land in [-1, 1]"
        );
    }
}
