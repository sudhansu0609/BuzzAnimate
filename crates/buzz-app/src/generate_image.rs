//! **AI ▸ Generate Image…**: the window over [`crate::prompt_image`].
//!
//! Positive and negative prompt, a strength slider for every LoRA the chosen
//! workflow loads, seed and size; the job runs on its own thread so the stage
//! keeps drawing while ComfyUI works, and the picture lands in the Library or
//! on the stage. What was typed is remembered between sessions.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::editor::Editor;
use crate::prompt_image::{self, Generated, LoraSlot, Request};

/// Where a finished picture goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Destination {
    /// Into the Library only, to place later.
    #[default]
    Library,
    /// Into the Library and onto the active layer at the playhead.
    Stage,
}

/// What is remembered between sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub workflow: String,
    pub url: String,
    pub positive: String,
    pub negative: String,
    pub seed: u64,
    pub random_seed: bool,
    pub width: u32,
    pub height: u32,
    pub skip_upscale: bool,
    /// Start the local ComfyUI (Comfy Desktop's install) when none answers.
    pub start_comfy: bool,
    /// Open ComfyUI's page on the workflow while it runs, to watch it there.
    pub show_in_comfy: bool,
    pub destination: Destination,
    /// Strengths by LoRA file name, so a slider survives reopening.
    pub lora_strengths: std::collections::BTreeMap<String, f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            workflow: String::new(),
            url: String::new(),
            positive: String::new(),
            negative: String::new(),
            seed: 1,
            random_seed: true,
            width: 1280,
            height: 720,
            skip_upscale: true,
            start_comfy: true,
            show_in_comfy: true,
            destination: Destination::Library,
            lora_strengths: Default::default(),
        }
    }
}

/// `~/.config/BuzzAnimate/generate_image.json`, beside the workspace.
fn settings_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("generate_image.json")
}

/// Where finished pictures are kept as files, beside the settings.
fn output_dir() -> PathBuf {
    settings_path().with_file_name("generated")
}

enum JobState {
    /// What the job is doing, and how far through the picture it is.
    Running(String, Option<f32>),
    Done(Result<(Generated, Vec<String>), String>),
}

/// What the window asks the shell to do, which it cannot do itself.
pub enum Action {
    /// Open a file dialog for the workflow.
    BrowseWorkflow,
}

pub struct GenerateImage {
    pub open: bool,
    settings: Settings,
    /// The workflow's LoRAs, read from the file named in `settings`.
    loras: Vec<LoraSlot>,
    /// Which file `loras` was read from, so editing the path rereads it.
    read_from: Option<String>,
    workflow_note: Option<String>,
    job: Option<Arc<Mutex<JobState>>>,
    status: Option<String>,
    preview: Option<(egui::TextureHandle, String)>,
    /// When the running job began: the window shows the seconds, so a cold
    /// model load that is quiet for minutes reads as working, not hung.
    job_began: Option<std::time::Instant>,
}

impl Default for GenerateImage {
    fn default() -> Self {
        let settings = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            open: false,
            settings,
            loras: Vec::new(),
            read_from: None,
            workflow_note: None,
            job: None,
            status: None,
            preview: None,
            job_began: None,
        }
    }
}

impl GenerateImage {
    /// A workflow chosen in the file dialog.
    pub fn set_workflow(&mut self, path: PathBuf) {
        self.settings.workflow = path.display().to_string();
        self.read_from = None;
        self.save();
    }

    pub fn is_running(&self) -> bool {
        self.job.as_ref().is_some_and(|j| matches!(*j.lock().unwrap(), JobState::Running(..)))
    }

    fn save(&self) {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.settings) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Read the workflow's LoRAs and size when the path changes.
    fn refresh_workflow(&mut self) {
        if self.read_from.as_deref() == Some(self.settings.workflow.as_str()) {
            return;
        }
        self.read_from = Some(self.settings.workflow.clone());
        self.loras.clear();
        self.workflow_note = None;
        let path = self.settings.workflow.trim();
        if path.is_empty() {
            self.workflow_note = Some("Choose a ComfyUI workflow (.json, saved from ComfyUI)".into());
            return;
        }
        let parsed = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()));
        match parsed {
            Ok(v) => {
                self.loras = prompt_image::lora_slots(&v);
                for l in &mut self.loras {
                    if let Some(s) = self.settings.lora_strengths.get(&l.name) {
                        l.strength = *s;
                    }
                }
                if self.loras.is_empty() {
                    self.workflow_note = Some("This workflow loads no LoRA".into());
                }
            }
            Err(e) => self.workflow_note = Some(format!("Cannot read the workflow: {e}")),
        }
    }

    /// The workflow's own size, into the size boxes.
    fn size_from_workflow(&mut self) {
        let size = std::fs::read_to_string(self.settings.workflow.trim())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| prompt_image::latent_size(&v));
        if let Some((w, h)) = size {
            self.settings.width = w;
            self.settings.height = h;
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.settings.random_seed {
            self.settings.seed = random_seed();
        }
        self.save();
        let req = Request {
            workflow: PathBuf::from(self.settings.workflow.trim()),
            url: self.settings.url.clone(),
            positive: self.settings.positive.clone(),
            negative: self.settings.negative.clone(),
            loras: self.loras.clone(),
            seed: self.settings.seed,
            size: (self.settings.width.max(64), self.settings.height.max(64)),
            skip_upscale: self.settings.skip_upscale,
            start_comfy: self.settings.start_comfy,
            show_in_comfy: self.settings.show_in_comfy,
        };
        let shared = Arc::new(Mutex::new(JobState::Running("Starting…".into(), None)));
        self.job = Some(Arc::clone(&shared));
        self.job_began = Some(std::time::Instant::now());
        self.status = None;
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("generate-image".into())
            .spawn(move || {
                let progress = |text: &str, fraction: Option<f32>| {
                    *shared.lock().unwrap() = JobState::Running(text.to_string(), fraction);
                    ctx.request_repaint();
                };
                let result = prompt_image::generate(&req, &output_dir(), &progress);
                *shared.lock().unwrap() = JobState::Done(result);
                ctx.request_repaint();
            })
            .expect("spawn generate-image thread");
    }

    /// A finished job: into the document, and into the preview.
    fn finish(&mut self, ctx: &egui::Context, editor: &mut Editor) {
        let Some(job) = &self.job else { return };
        let done = {
            let mut state = job.lock().unwrap();
            match &mut *state {
                JobState::Running(..) => return,
                JobState::Done(r) => std::mem::replace(r, Err(String::new())),
            }
        };
        self.job = None;
        match done {
            Err(e) => self.status = Some(format!("Failed: {e}")),
            Ok((picture, notes)) => {
                let stem = picture.file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let placed = match self.settings.destination {
                    Destination::Stage => editor.import_image(&picture.file).map_err(|e| e.to_string()),
                    Destination::Library => {
                        let mut out = Err("not added".to_string());
                        editor.doc.edit("Generate Image", |scene| {
                            out = scene.add_image(&stem, &picture.bytes).map(|a| a.name.clone()).map_err(|e| e.to_string());
                        });
                        editor.doc.end_gesture();
                        out
                    }
                };
                let mut text = match &placed {
                    Ok(name) => match self.settings.destination {
                        Destination::Stage => format!("Placed \u{201c}{name}\u{201d} on the stage (seed {})", picture.seed),
                        Destination::Library => format!("Added \u{201c}{name}\u{201d} to the Library (seed {})", picture.seed),
                    },
                    Err(e) => format!("Generated, but could not add it: {e}"),
                };
                for n in notes {
                    text.push_str(&format!("\nNote: {n}"));
                }
                editor.status = Some(text.lines().next().unwrap_or("").to_string());
                self.status = Some(text);
                if let Ok(img) = image::load_from_memory(&picture.bytes) {
                    let rgba = img.thumbnail(512, 512).to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let colour = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let tex = ctx.load_texture("generate-image-preview", colour, egui::TextureOptions::LINEAR);
                    self.preview = Some((tex, picture.file.display().to_string()));
                }
            }
        }
    }

    /// Draw the window; returns what the shell must do for it.
    pub fn show(&mut self, ctx: &egui::Context, editor: &mut Editor) -> Option<Action> {
        // A job finishes whether or not the window is still open.
        self.finish(ctx, editor);
        if !self.open {
            return None;
        }
        self.refresh_workflow();
        let mut action = None;
        let mut open = true;
        let running = self.is_running();
        let before = self.settings.clone();
        egui::Window::new("Generate Image")
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            // Never taller than the window: with a picture in it the content
            // outgrows the screen, and it scrolls instead.
            .max_height(ctx.content_rect().height() - 60.0)
            .show(ctx, |ui| {
              egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("generate-image-source").num_columns(2).show(ui, |ui| {
                    ui.label("Workflow");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.settings.workflow).desired_width(300.0))
                            .on_hover_text("A workflow saved from ComfyUI (Workflow ▸ Save, or Export API)");
                        if ui.button("Browse\u{2026}").clicked() {
                            action = Some(Action::BrowseWorkflow);
                        }
                    });
                    ui.end_row();
                    ui.label("ComfyUI");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.url)
                            .hint_text("found automatically (8188\u{2013}8208, 8000)")
                            .desired_width(300.0),
                    )
                    .on_hover_text("e.g. http://127.0.0.1:8000 — leave empty to find it");
                    ui.end_row();
                });
                if let Some(note) = &self.workflow_note {
                    ui.label(egui::RichText::new(note).small().weak());
                }

                ui.separator();
                ui.label("Positive prompt");
                ui.add(
                    egui::TextEdit::multiline(&mut self.settings.positive)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("What the picture should show"),
                );
                ui.label("Negative prompt");
                ui.add(
                    egui::TextEdit::multiline(&mut self.settings.negative)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("What it must not show \u{2014} empty keeps the workflow's own"),
                );

                if !self.loras.is_empty() {
                    ui.separator();
                    ui.label("LoRA strength");
                    for l in &mut self.loras {
                        ui.horizontal(|ui| {
                            let short: String = l.name.trim_end_matches(".safetensors").chars().take(36).collect();
                            ui.label(short).on_hover_text(&l.name);
                            if ui
                                .add(egui::Slider::new(&mut l.strength, 0.0..=2.0).step_by(0.01).fixed_decimals(2))
                                .on_hover_text("0 turns the LoRA off; 1 is its full trained strength")
                                .changed()
                            {
                                self.settings.lora_strengths.insert(l.name.clone(), l.strength);
                            }
                        });
                    }
                }

                ui.separator();
                egui::Grid::new("generate-image-params").num_columns(2).show(ui, |ui| {
                    ui.label("Seed");
                    ui.horizontal(|ui| {
                        ui.add_enabled(!self.settings.random_seed, egui::DragValue::new(&mut self.settings.seed));
                        ui.checkbox(&mut self.settings.random_seed, "New seed each time");
                    });
                    ui.end_row();
                    ui.label("Size");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.settings.width).range(64..=4096).speed(8));
                        ui.label("\u{d7}");
                        ui.add(egui::DragValue::new(&mut self.settings.height).range(64..=4096).speed(8));
                        if ui.small_button("Workflow's").on_hover_text("The size the workflow itself asks for").clicked() {
                            self.size_from_workflow();
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.settings.skip_upscale, "Skip the workflow's upscaler")
                        .on_hover_text("A 4\u{d7} upscaler turns 1280\u{d7}720 into 5120\u{d7}2880 — usually far more than the stage needs");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.settings.show_in_comfy, "Show the run in ComfyUI")
                        .on_hover_text("Opens ComfyUI's page on this workflow, with your prompt, seed, size and LoRA strengths in it, and shows each node as it runs");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.settings.start_comfy, "Start ComfyUI if it isn't running")
                        .on_hover_text("Starts your Comfy Desktop install in the background (log: ~/.config/BuzzAnimate/comfyui.log). It keeps running after BuzzAnimate quits, so the next picture is quick");
                    ui.end_row();
                    ui.label("Put it in");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.settings.destination, Destination::Library, "Library");
                        ui.radio_value(&mut self.settings.destination, Destination::Stage, "Stage (active layer)");
                    });
                    ui.end_row();
                });

                ui.separator();
                ui.horizontal(|ui| {
                    let ready = !self.settings.positive.trim().is_empty() && !self.settings.workflow.trim().is_empty();
                    if running {
                        ui.spinner();
                        if ui.button("Cancel").clicked() {
                            let url = self.settings.url.clone();
                            std::thread::spawn(move || prompt_image::interrupt(&url));
                        }
                    } else if ui.add_enabled(ready, egui::Button::new("Generate")).clicked() {
                        self.start(ui.ctx());
                    }
                    if ui
                        .button("Open ComfyUI")
                        .on_hover_text("ComfyUI's own page, in your browser")
                        .clicked()
                    {
                        let url = self.settings.url.clone();
                        std::thread::spawn(move || {
                            if let Ok(client) = prompt_image::connect(&url, false, &|_, _| {}) {
                                crate::comfy::open_in_browser(&client.base_url);
                            }
                        });
                    }
                });
                // **Where the job is**: ComfyUI's own stage — loading, sampling
                // step by step, decoding — a bar for how far through the
                // picture it is, and the seconds so far.
                if let Some(job) = &self.job
                    && let JobState::Running(text, fraction) = &*job.lock().unwrap()
                {
                    let seconds = self.job_began.map_or(0, |b| b.elapsed().as_secs());
                    ui.label(format!("{text}  ({seconds} s)"));
                    let bar = match fraction {
                        Some(f) => egui::ProgressBar::new(*f).show_percentage(),
                        None => egui::ProgressBar::new(0.0).animate(true),
                    };
                    ui.add(bar.desired_width(ui.available_width()));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                }
                if let Some(status) = &self.status {
                    ui.label(egui::RichText::new(status).small());
                }
                if let Some((tex, file)) = &self.preview {
                    let size = tex.size_vec2();
                    let scale = (ui.available_width() / size.x).min(1.0);
                    ui.add(egui::Image::new(tex).fit_to_exact_size(size * scale)).on_hover_text(file);
                }
              });
            });
        if !open {
            self.open = false;
        }
        if self.settings.url != before.url || self.settings.workflow != before.workflow {
            self.save();
        }
        action
    }
}

fn random_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    // ComfyUI's seeds are up to 2^64 but its UI shows 15 digits; keep them readable.
    h.finish() % 1_000_000_000_000_000
}
