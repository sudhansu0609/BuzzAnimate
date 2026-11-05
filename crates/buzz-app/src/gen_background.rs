//! **AI ▸ Generate Background…** (E1 of `docs/SIMPLE_EPISODE_FLOW_PLAN.md`):
//! a prompt becomes 1–4 pictures in the series look (the same ComfyUI profile
//! the Producer draws its plates with: the Khayal3Baje LoRA flow and its style
//! text), shown as thumbnails; the picked one lands on a new bottom layer
//! `Background`, and can go straight on to Trace (bold outlines) + Split into
//! layers. This file holds the dialog's state, the drawing of the dialog and
//! the off-thread generation; the document side is in `App` / `Editor`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::comfy::ComfyClient;
use crate::comfy_profile::{self, Kind};

/// The series whose look the dialog draws in unless told otherwise.
pub const DEFAULT_SERIES: &str = "Khayal3Baje";
/// Most tries one run draws.
pub const MAX_TRIES: usize = 4;
const GENERATE_TIMEOUT: Duration = Duration::from_secs(900);
/// Thumbnails are this wide (px).
const THUMB_WIDTH: u32 = 288;

/// One drawn picture.
pub struct Try {
    pub path: PathBuf,
    pub seed: u64,
    texture: Option<egui::TextureHandle>,
}

impl Try {
    pub fn new(path: PathBuf, seed: u64) -> Self {
        Self { path, seed, texture: None }
    }

    /// Whether the dialog has drawn this try's thumbnail yet.
    pub fn has_thumbnail(&self) -> bool {
        self.texture.is_some()
    }
}

/// The dialog's state.
pub struct GenBackground {
    pub open: bool,
    pub prompt: String,
    pub series: String,
    pub seed: u64,
    pub tries: usize,
    /// Trace (bold outlines) + split into layers right after the pick is placed.
    pub trace_split: bool,
    pub results: Vec<Try>,
    pub picked: Option<usize>,
    pub busy: bool,
    pub status: Option<String>,
}

impl Default for GenBackground {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64 % 1_000_000_000)
            .unwrap_or(1);
        Self {
            open: false,
            prompt: String::new(),
            series: DEFAULT_SERIES.into(),
            seed,
            tries: 2,
            trace_split: true,
            results: Vec::new(),
            picked: None,
            busy: false,
            status: None,
        }
    }
}

/// What the dialog asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenBackgroundAction {
    None,
    Generate,
    /// Place the picture at this index (and trace + split when the box is ticked).
    Use(usize),
}

/// Draw the dialog. Does nothing while it is closed.
pub fn dialog(ctx: &egui::Context, state: &mut GenBackground) -> GenBackgroundAction {
    if !state.open {
        return GenBackgroundAction::None;
    }
    let mut action = GenBackgroundAction::None;
    let mut open = true;
    egui::Window::new("Generate Background")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Draw a background in the series look; the pick goes on a new bottom layer named Background.");
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::multiline(&mut state.prompt)
                    .hint_text("A village lane at dusk, mud walls, a lone lamp\u{2026}")
                    .desired_rows(3)
                    .desired_width(520.0),
            );
            ui.horizontal(|ui| {
                ui.label("Series");
                ui.add(egui::TextEdit::singleline(&mut state.series).desired_width(120.0));
                ui.label("Seed");
                ui.add(egui::DragValue::new(&mut state.seed).speed(1.0));
                ui.label("Tries");
                ui.add(egui::DragValue::new(&mut state.tries).range(1..=MAX_TRIES));
            });
            ui.checkbox(&mut state.trace_split, "Then Trace (bold outlines) + Split into layers");
            ui.horizontal(|ui| {
                let can = !state.busy && !state.prompt.trim().is_empty();
                if ui.add_enabled(can, egui::Button::new("Generate")).clicked() {
                    action = GenBackgroundAction::Generate;
                }
                if state.busy {
                    ui.spinner();
                    ui.label("Drawing off the UI thread (progress card, bottom right)");
                }
            });
            if !state.results.is_empty() {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    for (i, t) in state.results.iter_mut().enumerate() {
                        ui.vertical(|ui| {
                            if t.texture.is_none() {
                                t.texture = load_thumb(ui.ctx(), &t.path, i);
                            }
                            if let Some(tex) = &t.texture {
                                let aspect = tex.size()[1] as f32 / tex.size()[0].max(1) as f32;
                                let size = egui::vec2(THUMB_WIDTH as f32, THUMB_WIDTH as f32 * aspect);
                                let selected = state.picked == Some(i);
                                let r = ui.add(egui::Image::new(tex).fit_to_exact_size(size).sense(egui::Sense::click()));
                                if selected {
                                    ui.painter().rect_stroke(r.rect, 0.0, egui::Stroke::new(3.0, egui::Color32::LIGHT_BLUE), egui::StrokeKind::Outside);
                                }
                                if r.clicked() {
                                    state.picked = Some(i);
                                }
                            } else {
                                ui.label("(picture unreadable)");
                            }
                            ui.small(format!("seed {}", t.seed));
                        });
                    }
                });
                ui.horizontal(|ui| {
                    let can = !state.busy && state.picked.is_some();
                    let label = if state.trace_split { "Use picked + Trace + Split" } else { "Use picked" };
                    if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                        if let Some(i) = state.picked {
                            action = GenBackgroundAction::Use(i);
                        }
                    }
                });
            }
            if let Some(s) = &state.status {
                ui.label(s);
            }
        });
    if !open {
        state.open = false;
    }
    action
}

fn load_thumb(ctx: &egui::Context, path: &Path, index: usize) -> Option<egui::TextureHandle> {
    let img = image::open(path).ok()?.to_rgba8();
    let h = ((img.height() as f64 * THUMB_WIDTH as f64 / img.width().max(1) as f64).round() as u32).max(1);
    let small = image::imageops::resize(&img, THUMB_WIDTH, h, image::imageops::FilterType::Triangle);
    let color = egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw());
    Some(ctx.load_texture(format!("gen_background_{index}_{}", path.display()), color, Default::default()))
}

/// The size to draw at for a stage: its own, long edge capped at 1920, both
/// sides a multiple of 8 (what the latent needs).
pub fn draw_size(stage: (f64, f64)) -> (u32, u32) {
    let long = stage.0.max(stage.1).max(1.0);
    let s = (1920.0 / long).min(1.0);
    let snap = |v: f64| (((v * s / 8.0).round() as u32) * 8).max(64);
    (snap(stage.0), snap(stage.1))
}

/// Where this run's pictures are written.
pub fn out_dir() -> PathBuf {
    std::env::temp_dir().join("buzzanimate_generate_background")
}

/// Draw `tries` pictures of `prompt` (seeds `seed`, `seed + 1`, ...) at `size`
/// with the active series' ComfyUI profile (the Producer's plate path: the
/// series workflow with its LoRA, the style text appended to the prompt), or
/// the Z-Image default when the series has none. `progress` hears (done,
/// total, note); `cancelled` stops between tries. Returns (png, seed) per try.
pub fn generate_tries(
    series: &str,
    prompt: &str,
    seed: u64,
    tries: usize,
    size: (u32, u32),
    progress: &mut dyn FnMut(u64, u64, &str),
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<(PathBuf, u64)>, String> {
    let tries = tries.clamp(1, MAX_TRIES);
    progress(0, tries as u64 + 1, "Finding ComfyUI");
    let client = ComfyClient::ensure_running(true, Duration::from_secs(600))?;
    let dir = out_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..tries {
        if cancelled() {
            break;
        }
        let s = seed.wrapping_add(i as u64);
        progress(i as u64 + 1, tries as u64 + 1, &format!("Drawing picture {} of {tries}", i + 1));
        let png = dir.join(format!("bg_{stamp}_{s}.png"));
        draw_one(&client, series, prompt, s, size, &png)?;
        out.push((png, s));
    }
    if out.is_empty() {
        return Err("cancelled".into());
    }
    Ok(out)
}

fn draw_one(client: &ComfyClient, series: &str, prompt: &str, seed: u64, size: (u32, u32), out_png: &Path) -> Result<(), String> {
    let workflow = match comfy_profile::bound_for_series(client, series, Kind::Background, prompt, seed, Some(size)) {
        Ok(Some(w)) => w,
        Err(e) if e.starts_with(comfy_profile::MISSING_PREFIX) => return Err(e),
        other => {
            if let Err(e) = other {
                tracing::warn!("series ComfyUI profile unusable, using the Z-Image default: {e}");
            }
            crate::backdrop::generate(client, prompt, seed, out_png)?;
            return Ok(());
        }
    };
    let id = client.queue(workflow)?;
    let outputs = client.wait(&id, GENERATE_TIMEOUT)?;
    let first = outputs.first().ok_or("ComfyUI returned no picture")?;
    let bytes = client.view(first)?;
    let picture = image::load_from_memory(&bytes).map_err(|e| format!("{}: {e}", first.filename))?;
    picture.to_rgba8().save(out_png).map_err(|e| format!("{}: {e}", out_png.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_size_follows_the_stage_in_multiples_of_eight() {
        assert_eq!(draw_size((1920.0, 1080.0)), (1920, 1080));
        assert_eq!(draw_size((3840.0, 2160.0)), (1920, 1080));
        let (w, h) = draw_size((1000.0, 777.0));
        assert!(w % 8 == 0 && h % 8 == 0);
    }
}
