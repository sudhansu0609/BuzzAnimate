//! ONNX Runtime session management and pre/post-processing helpers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use anyhow::{Context, Result};
use image::{imageops::FilterType, RgbaImage, Rgba};
use ort::session::Session;

use crate::models::ModelId;

/// Wrapper around an ONNX Runtime session.
pub struct OnnxSession {
    session: Session,
    ep_name: &'static str,
}

impl OnnxSession {
    /// Loads an ONNX model from disk, trying hardware acceleration then CPU fallback.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        tracing::info!("Loading ONNX model from {:?}", path);

        // Try DirectML on Windows, CoreML on macOS, fall back to CPU.
        #[cfg(target_os = "windows")]
        {
            let builder = Session::builder()
                .map_err(|e| anyhow::anyhow!("Failed to create ONNX session builder: {e}"))?;
            let res = builder
                .with_execution_providers([
                    ort::ep::DirectML::default().build(),
                    ort::ep::CPU::default().build(),
                ])
                .map_err(|e| anyhow::anyhow!("Failed to set execution providers: {e}"))
                .and_then(|mut b| {
                    b.commit_from_file(path)
                        .map_err(|e| anyhow::anyhow!("Failed to commit ONNX model from file: {e}"))
                });

            if let Ok(session) = res {
                tracing::info!("ONNX model {:?} loaded using DirectML", path);
                return Ok(Self {
                    session,
                    ep_name: "DirectML",
                });
            } else if let Err(e) = &res {
                tracing::warn!("DirectML failed for {:?}, falling back to CPU: {e}", path);
            }
        }

        #[cfg(target_os = "macos")]
        {
            let builder = Session::builder()
                .map_err(|e| anyhow::anyhow!("Failed to create ONNX session builder: {e}"))?;
            let res = builder
                .with_execution_providers([
                    ort::ep::CoreML::default().build(),
                    ort::ep::CPU::default().build(),
                ])
                .map_err(|e| anyhow::anyhow!("Failed to set execution providers: {e}"))
                .and_then(|mut b| {
                    b.commit_from_file(path)
                        .map_err(|e| anyhow::anyhow!("Failed to commit ONNX model from file: {e}"))
                });

            if let Ok(session) = res {
                tracing::info!("ONNX model {:?} loaded using CoreML", path);
                return Ok(Self {
                    session,
                    ep_name: "CoreML",
                });
            } else if let Err(e) = &res {
                tracing::warn!("CoreML failed for {:?}, falling back to CPU: {e}", path);
            }
        }

        let builder = Session::builder()
            .map_err(|e| anyhow::anyhow!("Failed to create ONNX session builder: {e}"))?;
        let session = builder
            .with_execution_providers([ort::ep::CPU::default().build()])
            .map_err(|e| anyhow::anyhow!("Failed to set CPU execution provider: {e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to commit ONNX model from file: {e}"))
            .with_context(|| format!("Failed to load ONNX model from {:?}", path))?;

        tracing::info!("ONNX model {:?} loaded using CPU", path);
        Ok(Self {
            session,
            ep_name: "CPU",
        })
    }

    /// Loads an ONNX model on the CPU execution provider only, skipping
    /// hardware acceleration entirely.
    ///
    /// Some graphs (LaMa's FFC/spectral-transform blocks, observed on this
    /// machine) commit fine under DirectML but fail *during* `run()` with an
    /// opaque DML error — a runtime incompatibility `load()`'s
    /// commit-time fallback can't catch. Callers that hit that use this to
    /// retry on a execution provider known to support every ONNX op.
    pub fn load_cpu(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let builder = Session::builder()
            .map_err(|e| anyhow::anyhow!("Failed to create ONNX session builder: {e}"))?;
        let session = builder
            .with_execution_providers([ort::ep::CPU::default().build()])
            .map_err(|e| anyhow::anyhow!("Failed to set CPU execution provider: {e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to commit ONNX model from file: {e}"))
            .with_context(|| format!("Failed to load ONNX model from {:?} (CPU)", path))?;

        tracing::info!("ONNX model {:?} loaded using CPU (forced)", path);
        Ok(Self {
            session,
            ep_name: "CPU",
        })
    }

    /// The name of the execution provider in use (e.g. "DirectML", "CoreML", "CPU").
    pub fn ep_name(&self) -> &'static str {
        self.ep_name
    }

    /// Access the underlying `ort::session::Session`.
    pub fn inner(&self) -> &Session {
        &self.session
    }

    /// Mutable access to the underlying `ort::session::Session`.
    pub fn inner_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

/// Process-wide pool of loaded ONNX sessions, keyed by `ModelId`.
///
/// C1-C4 previously loaded a fresh `OnnxSession` (and paid the DirectML
/// commit cost) on every single call, so any timing included a cold load.
/// This pool loads each model once and reuses it, so per-call timings
/// measure inference, not initialization. `pooled` is safe to call from
/// several threads: concurrent misses for the same id may briefly load the
/// model twice, but the pool converges on one shared session.
static POOL: OnceLock<Mutex<HashMap<ModelId, Arc<Mutex<OnnxSession>>>>> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<ModelId, Arc<Mutex<OnnxSession>>>> {
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns the pooled session for `id`, loading it from `models_dir()` on
/// first use. Returns `None` (rather than erroring) when the model file
/// isn't present on disk, so callers can fall back to a non-AI path.
pub fn pooled(id: ModelId) -> Option<Arc<Mutex<OnnxSession>>> {
    if let Some(existing) = pool().lock().unwrap().get(&id) {
        return Some(Arc::clone(existing));
    }
    let entry = crate::models::get_entry(id);
    let path = crate::models::models_dir().join(entry.filename);
    if !path.exists() {
        return None;
    }
    let session = match OnnxSession::load(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Pool: failed to load {:?}: {e}", id);
            return None;
        }
    };
    let arc = Arc::new(Mutex::new(session));
    pool().lock().unwrap().insert(id, Arc::clone(&arc));
    Some(arc)
}

/// Like [`pooled`], but loads on the CPU execution provider outright rather
/// than trying hardware acceleration first.
///
/// For most models `load()`'s commit-time DirectML failure is a clean `Err`
/// it can fall back from. SigLIP's quantized graph (`ModelId::SigLip`) is
/// not one of those: `commit_from_file` itself takes down the whole process
/// with a native access violation under DirectML on this machine — a crash
/// `load()`'s `Result`-based fallback can never see, let alone catch, since
/// the crash happens *inside* the call that would return it. Use this for
/// any model id known to crash DirectML at commit time, so `pooled()`'s
/// first (and only) load attempt for it is never made.
pub fn pooled_cpu(id: ModelId) -> Option<Arc<Mutex<OnnxSession>>> {
    if let Some(existing) = pool().lock().unwrap().get(&id) {
        return Some(Arc::clone(existing));
    }
    let entry = crate::models::get_entry(id);
    let path = crate::models::models_dir().join(entry.filename);
    if !path.exists() {
        return None;
    }
    let session = match OnnxSession::load_cpu(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Pool: failed to load {:?} on CPU: {e}", id);
            return None;
        }
    };
    let arc = Arc::new(Mutex::new(session));
    pool().lock().unwrap().insert(id, Arc::clone(&arc));
    Some(arc)
}

/// Replaces the pooled session for `id` (e.g. after a caller retried a
/// failed run on a different execution provider via `load_cpu`), so later
/// `pooled()` calls reuse the working session instead of repeating the
/// failure.
pub fn replace_pooled(id: ModelId, session: OnnxSession) {
    pool().lock().unwrap().insert(id, Arc::new(Mutex::new(session)));
}

/// Pre-loads (warms) each of `ids` in its own background thread, returning
/// `(id, elapsed, loaded)` once every thread has finished. `loaded` is
/// `false` when the model file was missing or failed to load — the pool
/// stays empty for that id and later `pooled()` calls fall back.
pub fn warm(ids: &[ModelId]) -> Vec<(ModelId, std::time::Duration, bool)> {
    let handles: Vec<_> = ids
        .iter()
        .copied()
        .map(|id| {
            std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                let loaded = pooled(id).is_some();
                (id, t0.elapsed(), loaded)
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

/// Metadata describing how an image was letterboxed to fit a model's input dimensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LetterboxInfo {
    pub orig_width: u32,
    pub orig_height: u32,
    pub target_width: u32,
    pub target_height: u32,
    pub scale: f32,
    pub pad_x: f32,
    pub pad_y: f32,
}

impl LetterboxInfo {
    /// Maps a point (x, y) from letterboxed coordinates back to original image space.
    pub fn unletterbox_point(&self, x: f32, y: f32) -> (f32, f32) {
        if self.scale <= 0.0 {
            return (0.0, 0.0);
        }
        let unpad_x = (x - self.pad_x) / self.scale;
        let unpad_y = (y - self.pad_y) / self.scale;
        (
            unpad_x.clamp(0.0, self.orig_width as f32),
            unpad_y.clamp(0.0, self.orig_height as f32),
        )
    }

    /// Maps a point (x, y) from original image space to letterboxed coordinates.
    pub fn letterbox_point(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale + self.pad_x, y * self.scale + self.pad_y)
    }
}

/// Scales an image preserving aspect ratio to fit inside `(target_width, target_height)`,
/// centered with padding color `fill`.
pub fn letterbox(
    img: &RgbaImage,
    target_width: u32,
    target_height: u32,
    fill: [u8; 4],
) -> (RgbaImage, LetterboxInfo) {
    let orig_width = img.width();
    let orig_height = img.height();

    if orig_width == 0 || orig_height == 0 {
        let canvas = RgbaImage::from_pixel(target_width, target_height, Rgba(fill));
        let info = LetterboxInfo {
            orig_width,
            orig_height,
            target_width,
            target_height,
            scale: 1.0,
            pad_x: 0.0,
            pad_y: 0.0,
        };
        return (canvas, info);
    }

    let scale_x = target_width as f32 / orig_width as f32;
    let scale_y = target_height as f32 / orig_height as f32;
    let scale = scale_x.min(scale_y);

    let new_w = (orig_width as f32 * scale).round().max(1.0) as u32;
    let new_h = (orig_height as f32 * scale).round().max(1.0) as u32;

    let resized = image::imageops::resize(img, new_w, new_h, FilterType::Triangle);

    let pad_x = ((target_width as f32 - new_w as f32) / 2.0).floor();
    let pad_y = ((target_height as f32 - new_h as f32) / 2.0).floor();

    let mut canvas = RgbaImage::from_pixel(target_width, target_height, Rgba(fill));
    image::imageops::overlay(&mut canvas, &resized, pad_x as i64, pad_y as i64);

    let info = LetterboxInfo {
        orig_width,
        orig_height,
        target_width,
        target_height,
        scale,
        pad_x,
        pad_y,
    };

    (canvas, info)
}

/// Converts an RGBA image to a planar NCHW float buffer `[1, 3, H, W]`
/// with normalized values `(pixel / 255.0 - mean) / std`.
pub fn image_to_nchw(
    img: &RgbaImage,
    target_size: (u32, u32),
    mean: [f32; 3],
    std: [f32; 3],
    pad_fill: [u8; 4],
) -> (Vec<f32>, LetterboxInfo) {
    let (target_w, target_h) = target_size;
    let (letterboxed, info) = letterbox(img, target_w, target_h, pad_fill);

    let num_pixels = (target_w * target_h) as usize;
    let mut data = vec![0.0f32; 3 * num_pixels];

    for y in 0..target_h {
        for x in 0..target_w {
            let idx = (y * target_w + x) as usize;
            let pixel = letterboxed.get_pixel(x, y);

            let r = pixel[0] as f32 / 255.0;
            let g = pixel[1] as f32 / 255.0;
            let b = pixel[2] as f32 / 255.0;

            data[idx] = (r - mean[0]) / std[0];
            data[num_pixels + idx] = (g - mean[1]) / std[1];
            data[2 * num_pixels + idx] = (b - mean[2]) / std[2];
        }
    }

    (data, info)
}

/// Unletterboxes a 2D float mask (H x W) back to the original image dimensions.
pub fn unletterbox_mask(
    mask: &[f32],
    mask_w: u32,
    mask_h: u32,
    info: &LetterboxInfo,
) -> Vec<f32> {
    let orig_w = info.orig_width as usize;
    let orig_h = info.orig_height as usize;
    let mut out = vec![0.0f32; orig_w * orig_h];

    if orig_w == 0 || orig_h == 0 || info.scale <= 0.0 {
        return out;
    }

    for y in 0..orig_h {
        for x in 0..orig_w {
            let (lx, ly) = info.letterbox_point(x as f32, y as f32);
            let lx = lx.clamp(0.0, (mask_w - 1) as f32);
            let ly = ly.clamp(0.0, (mask_h - 1) as f32);

            let x0 = lx.floor() as usize;
            let y0 = ly.floor() as usize;
            let x1 = (x0 + 1).min(mask_w as usize - 1);
            let y1 = (y0 + 1).min(mask_h as usize - 1);

            let fx = lx - x0 as f32;
            let fy = ly - y0 as f32;

            let v00 = mask[y0 * mask_w as usize + x0];
            let v10 = mask[y0 * mask_w as usize + x1];
            let v01 = mask[y1 * mask_w as usize + x0];
            let v11 = mask[y1 * mask_w as usize + x1];

            let top = v00 * (1.0 - fx) + v10 * fx;
            let bottom = v01 * (1.0 - fx) + v11 * fx;
            out[y * orig_w + x] = top * (1.0 - fy) + bottom * fy;
        }
    }

    out
}
