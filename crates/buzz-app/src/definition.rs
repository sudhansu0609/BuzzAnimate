//! **Definition and trace quality** (ANIMATED_CHANNELS_PLAN AN7, AN8).
//!
//! AN7: every generated picture is upscaled through ComfyUI (RealESRGAN) to the
//! trace size, then judged: sharpness (variance of the Laplacian on a 512-wide
//! grey copy), size at least the trace size, and the people check.
//! AN8: after tracing, the vector is rendered and compared with the picture
//! (mean colour error and a block SSIM on luma, 480-wide copies); a poor trace
//! is redone with the next option set, the best is kept.

use std::path::Path;
use std::time::Duration;

use image::{GrayImage, RgbaImage, imageops};
use serde_json::{Value, json};

use crate::backdrop::{BackdropOptions, Plates, trace_plates};
use crate::comfy::ComfyClient;

/// The size a picture must have before it is traced (the stage).
pub const TRACE_SIZE: (u32, u32) = (1920, 1080);
/// Sharpness below this is a soft picture (variance of the Laplacian, 512-wide grey).
/// Calibrated 2026-10-29 on the proof pictures in scratchpad/produce/p3 (see the plan's Status table).
pub const SHARPNESS_MIN: f64 = 40.0;
/// A trace whose mean absolute colour error (0..255) is above this is poor.
pub const TRACE_MAE_MAX: f64 = 28.0;
/// A trace whose luma SSIM is below this is poor.
pub const TRACE_SSIM_MIN: f64 = 0.55;

// ------------------------------------------------------------------ sharpness

/// Variance of the Laplacian of a 512-wide grey copy: high = crisp, low = soft.
pub fn sharpness(picture: &RgbaImage) -> f64 {
    let (w, h) = (picture.width().max(1), picture.height().max(1));
    let small = if w > 512 {
        imageops::resize(picture, 512, ((h as f64 * 512.0 / w as f64).round() as u32).max(1), imageops::FilterType::Triangle)
    } else {
        picture.clone()
    };
    let g: GrayImage = imageops::grayscale(&small);
    let (gw, gh) = (g.width() as usize, g.height() as usize);
    if gw < 3 || gh < 3 {
        return 0.0;
    }
    let px = |x: usize, y: usize| g.as_raw()[y * gw + x] as f64;
    let (mut sum, mut sum2, mut n) = (0.0, 0.0, 0.0);
    for y in 1..gh - 1 {
        for x in 1..gw - 1 {
            let l = px(x - 1, y) + px(x + 1, y) + px(x, y - 1) + px(x, y + 1) - 4.0 * px(x, y);
            sum += l;
            sum2 += l * l;
            n += 1.0;
        }
    }
    let mean = sum / n;
    sum2 / n - mean * mean
}

// ------------------------------------------------------------------ upscale

/// Upscale models tried in order (the first the ComfyUI install has is used).
const UPSCALE_MODELS: &[&str] = &["RealESRGAN_x4plus.safetensors", "RealESRGAN_x2plus.pth"];

/// The size `size` must reach: the trace size at the picture's own aspect.
pub fn target_size(size: (u32, u32)) -> (u32, u32) {
    if size.0 >= TRACE_SIZE.0 && size.1 >= TRACE_SIZE.1 {
        return size;
    }
    let scale = (TRACE_SIZE.0 as f64 / size.0.max(1) as f64).max(TRACE_SIZE.1 as f64 / size.1.max(1) as f64);
    ((size.0 as f64 * scale).ceil() as u32, (size.1 as f64 * scale).ceil() as u32)
}

fn upscale_workflow(uploaded: &str, model: &str, target: (u32, u32)) -> Value {
    json!({
        "1": {"class_type": "LoadImage", "inputs": {"image": uploaded}},
        "2": {"class_type": "UpscaleModelLoader", "inputs": {"model_name": model}},
        "3": {"class_type": "ImageUpscaleWithModel", "inputs": {"upscale_model": ["2", 0], "image": ["1", 0]}},
        "4": {"class_type": "ImageScale", "inputs": {"image": ["3", 0], "upscale_method": "lanczos", "width": target.0, "height": target.1, "crop": "disabled"}},
        "5": {"class_type": "SaveImage", "inputs": {"images": ["4", 0], "filename_prefix": "buzzanimate-up"}}
    })
}

/// Upscale `png` in place to [`target_size`]. A picture already that size is
/// left alone; any failure (no model, ComfyUI error) keeps the original and
/// returns `Err` for the caller to log: it never fails the run.
pub fn upscale_in_place(client: &ComfyClient, png: &Path) -> Result<bool, String> {
    let img = image::open(png).map_err(|e| format!("{}: {e}", png.display()))?;
    let size = (img.width(), img.height());
    if size.0 >= TRACE_SIZE.0 && size.1 >= TRACE_SIZE.1 {
        return Ok(false);
    }
    let target = target_size(size);
    let bytes = std::fs::read(png).map_err(|e| format!("{}: {e}", png.display()))?;
    let uploaded = client.upload_image(&format!("buzzanimate-{}.png", crate::comfy_profile::short_hash(&bytes)), &bytes)?;
    let mut last = String::from("no upscale model");
    for model in UPSCALE_MODELS {
        let attempt = client
            .queue(upscale_workflow(&uploaded, model, target))
            .and_then(|id| client.wait(&id, Duration::from_secs(300)))
            .and_then(|outs| {
                let first = outs.first().ok_or("ComfyUI returned no picture")?;
                client.view(first)
            })
            .and_then(|b| image::load_from_memory(&b).map_err(|e| e.to_string()));
        match attempt {
            Ok(up) if up.width() >= TRACE_SIZE.0 && up.height() >= TRACE_SIZE.1 => {
                up.to_rgba8().save(png).map_err(|e| format!("{}: {e}", png.display()))?;
                return Ok(true);
            }
            Ok(up) => last = format!("{model}: result only {}x{}", up.width(), up.height()),
            Err(e) => last = format!("{model}: {e}"),
        }
    }
    Err(last)
}

// ------------------------------------------------------------------ trace compare

/// How well a rendered trace matches its picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceScore {
    /// Mean absolute colour error over R, G, B (0..255).
    pub mae: f64,
    /// Block SSIM on luma (0..1).
    pub ssim: f64,
}

impl TraceScore {
    /// One number to rank by: higher is better.
    pub fn score(&self) -> f64 {
        self.ssim - self.mae / 255.0
    }
    pub fn ok(&self) -> bool {
        self.mae <= TRACE_MAE_MAX && self.ssim >= TRACE_SSIM_MIN
    }
}

/// A 480-wide RGB copy, transparency flattened on white.
fn small_rgb(img: &RgbaImage, size: (u32, u32)) -> Vec<[f64; 3]> {
    let r = imageops::resize(img, size.0, size.1, imageops::FilterType::Triangle);
    r.pixels()
        .map(|p| {
            let a = p.0[3] as f64 / 255.0;
            let c = |v: u8| v as f64 * a + 255.0 * (1.0 - a);
            [c(p.0[0]), c(p.0[1]), c(p.0[2])]
        })
        .collect()
}

/// Compare a rendered trace with its picture on 480-wide copies.
pub fn trace_compare(picture: &RgbaImage, rendered: &RgbaImage) -> TraceScore {
    let w = 480u32;
    let h = ((picture.height() as f64 * w as f64 / picture.width().max(1) as f64).round() as u32).max(8);
    let a = small_rgb(picture, (w, h));
    let b = small_rgb(rendered, (w, h));
    let mae = a.iter().zip(&b).map(|(x, y)| (0..3).map(|i| (x[i] - y[i]).abs()).sum::<f64>() / 3.0).sum::<f64>() / a.len() as f64;
    let luma = |v: &[f64; 3]| 0.299 * v[0] + 0.587 * v[1] + 0.114 * v[2];
    let (la, lb): (Vec<f64>, Vec<f64>) = (a.iter().map(luma).collect(), b.iter().map(luma).collect());
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (mut total, mut blocks) = (0.0, 0.0);
    let (bw, bh) = (w as usize / 8, h as usize / 8);
    for by in 0..bh {
        for bx in 0..bw {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for y in 0..8 {
                for x in 0..8 {
                    let i = (by * 8 + y) * w as usize + bx * 8 + x;
                    let (p, q) = (la[i], lb[i]);
                    sa += p;
                    sb += q;
                    saa += p * p;
                    sbb += q * q;
                    sab += p * q;
                }
            }
            let (ma, mb) = (sa / 64.0, sb / 64.0);
            let (va, vb, cov) = (saa / 64.0 - ma * ma, sbb / 64.0 - mb * mb, sab / 64.0 - ma * mb);
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            blocks += 1.0;
        }
    }
    TraceScore { mae, ssim: if blocks > 0.0 { total / blocks } else { 0.0 } }
}

/// Renders a traced scene (the stage at half size) for the comparison. Starts
/// the GPU exporter on first use; without one, nothing is compared.
#[derive(Default)]
pub struct RenderCheck {
    exporter: Option<buzz_export::Exporter>,
    tried: bool,
}

impl RenderCheck {
    pub fn render(&mut self, scene: &buzz_scene::Scene) -> Option<RgbaImage> {
        if !self.tried {
            self.tried = true;
            match buzz_export::Exporter::new(&buzz_render::GpuPreference::default()) {
                Ok(e) => self.exporter = Some(e),
                Err(e) => tracing::warn!("trace check unavailable (no renderer): {e}"),
            }
        }
        let exporter = self.exporter.as_mut()?;
        let settings = buzz_export::ExportSettings::scaled(scene, 0.5);
        let f = exporter.render(scene, 0, &settings).ok()?;
        RgbaImage::from_raw(f.width, f.height, f.pixels)
    }
}

/// The option sets tried after the first one: more colours / lower speckle /
/// another smoothness (TRACE_TUNING_PLAN).
pub fn retrace_variants(base: &BackdropOptions) -> [BackdropOptions; 2] {
    [
        BackdropOptions { colours: (base.colours + 16).min(64), speckle: (base.speckle / 2).max(6), ..*base },
        BackdropOptions { colours: (base.colours + 24).min(64), speckle: (base.speckle / 4).max(4), smoothness: (base.smoothness - 0.2).max(0.3), ..*base },
    ]
}

/// What [`trace_checked`] did.
pub struct Traced {
    pub scene: buzz_scene::Scene,
    /// `None` when no renderer was available.
    pub score: Option<TraceScore>,
    pub tries: usize,
    pub notes: Vec<String>,
}

/// Trace `plates`, compare with `picture`, retrace (max 2 more) while poor, keep the best.
pub fn trace_checked(
    plates: &Plates,
    picture: &RgbaImage,
    base: &BackdropOptions,
    check: &mut RenderCheck,
    say: &mut dyn FnMut(&str),
) -> Traced {
    let variants = retrace_variants(base);
    let sets = [*base, variants[0], variants[1]];
    let mut best: Option<(buzz_scene::Scene, Option<TraceScore>)> = None;
    let mut notes = Vec::new();
    let mut tries = 0;
    for (n, opts) in sets.iter().enumerate() {
        if n > 0 {
            say(&format!("trace poor; retrace {n} ({} colours, speckle {}, smooth {:.1})\u{2026}", opts.colours, opts.speckle, opts.smoothness));
        }
        let scene = trace_plates(plates, opts);
        tries += 1;
        let score = check.render(&scene).map(|r| trace_compare(picture, &r));
        match &score {
            Some(s) => notes.push(format!("trace {} ({} colours): colour error {:.1}, SSIM {:.3}", n + 1, opts.colours, s.mae, s.ssim)),
            None => notes.push(format!("trace {}: not compared (no renderer)", n + 1)),
        }
        let better = match (&best, &score) {
            (None, _) => true,
            (Some((_, Some(b))), Some(s)) => s.score() > b.score(),
            _ => false,
        };
        let good = score.is_some_and(|s| s.ok());
        if better {
            best = Some((scene, score));
        }
        if good || score.is_none() {
            break;
        }
    }
    let (scene, score) = best.expect("at least one trace");
    Traced { scene, score, tries, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern() -> RgbaImage {
        RgbaImage::from_fn(640, 360, |x, y| {
            let on = ((x / 20) + (y / 20)) % 2 == 0;
            let v = if on { 230 } else { 30 };
            image::Rgba([v, (x % 200) as u8, 90, 255])
        })
    }

    #[test]
    fn a_blurred_copy_is_less_sharp() {
        let a = pattern();
        let blurred = imageops::blur(&a, 4.0);
        assert!(sharpness(&a) > sharpness(&blurred) * 2.0);
    }

    #[test]
    fn an_identical_render_scores_above_a_shifted_one() {
        let a = pattern();
        let shifted = RgbaImage::from_fn(640, 360, |x, y| *a.get_pixel((x + 17) % 640, (y + 11) % 360));
        let same = trace_compare(&a, &a);
        let off = trace_compare(&a, &shifted);
        assert!(same.score() > off.score());
        assert!(same.ok() && same.mae < 0.01);
    }
}
