//! Dialog for managing local ONNX models and LLM backends (LM Studio / Ollama).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use crossbeam_channel::{Receiver, Sender};
use egui::{Align2, Context, RichText, Ui};
use buzz_ai::llm::{detect, Backend};
use buzz_ai::models::{ensure, models_dir, status, ModelId, ModelStatus, REGISTRY};
use crate::workspace::AiSettings;

/// Progress and status updates during background model downloads.
#[derive(Debug, Clone)]
pub enum ModelDownloadProgress {
    Progress {
        id: ModelId,
        downloaded: u64,
        total: u64,
    },
    Done {
        id: ModelId,
    },
    Error {
        id: ModelId,
        error: String,
    },
}

/// State of the AI Models dialog.
pub struct AiModelsState {
    pub open: bool,
    pub detected_backends: Option<Vec<(Backend, Vec<String>)>>,
    pub is_detecting: bool,
    pub rx_detect: Option<Receiver<Vec<(Backend, Vec<String>)>>>,
    pub downloading: Option<ModelId>,
    pub download_progress: f32,
    pub cancel_flag: Arc<AtomicBool>,
    pub rx_download: Option<Receiver<ModelDownloadProgress>>,
    pub status_message: Option<(String, bool)>, // (message, is_error)

    // --- The Claude API server (F7.8) ---
    /// Base URL field, e.g. `http://127.0.0.1:8787/v1`.
    pub claude_url: String,
    /// What the user typed into the (masked) key field. Never pre-filled
    /// with the stored key: the stored one is only ever shown masked.
    pub claude_key_input: String,
    /// Whether `ai.json` / `BUZZ_CLAUDE_API_KEY` already hold a key.
    pub claude_key_stored: bool,
    /// Models the server listed on the last successful Test connection.
    pub claude_models: Vec<String>,
    pub claude_status: Option<(String, bool)>,
    /// Model saved to ai.json (default `ai_config::CLAUDE_DEFAULT_MODEL`, "opus").
    pub claude_model: String,
    /// "Use Claude for the Assistant" (ai.json `use_claude`).
    pub use_claude: bool,
    pub claude_testing: bool,
    pub rx_claude_test: Option<Receiver<Result<(Vec<String>, u128), String>>>,
}

impl Default for AiModelsState {
    fn default() -> Self {
        Self {
            open: false,
            detected_backends: None,
            is_detecting: false,
            rx_detect: None,
            downloading: None,
            download_progress: 0.0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            rx_download: None,
            status_message: None,
            claude_url: buzz_ai::ai_config::claude_api_base_url(),
            claude_key_input: String::new(),
            claude_key_stored: buzz_ai::ai_config::claude_api_key().is_some(),
            claude_models: Vec::new(),
            claude_status: None,
            claude_model: buzz_ai::ai_config::claude_model(),
            use_claude: buzz_ai::ai_config::use_claude(),
            claude_testing: false,
            rx_claude_test: None,
        }
    }
}

impl std::fmt::Debug for AiModelsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiModelsState")
            .field("open", &self.open)
            .field("downloading", &self.downloading)
            .field("download_progress", &self.download_progress)
            .finish()
    }
}

/// Human name of where the key is stored ("BuzzAnimate\.env", "ai.json", ...).
fn key_where() -> &'static str {
    match buzz_ai::ai_config::key_location() {
        ".env" => "BuzzAnimate\\.env",
        other => other,
    }
}

/// "Key: sk-local-… in BuzzAnimate\.env" (masked; never the raw key).
fn key_status_line() -> String {
    match buzz_ai::ai_config::claude_api_key() {
        Some(k) => format!("Key: {} in {}", buzz_ai::ai_config::mask_key(&k), key_where()),
        None => format!("Key: (none) - will be saved to {}", key_where()),
    }
}

impl AiModelsState {
    /// Polls background download and backend detection channels.
    pub fn poll(&mut self, ai_settings: &mut AiSettings) {
        if let Some(rx) = &self.rx_detect {
            if let Ok(results) = rx.try_recv() {
                self.detected_backends = Some(results);
                self.is_detecting = false;
                self.rx_detect = None;

                // If no backend is chosen yet and we found backends, default to first (LM Studio preferred)
                if ai_settings.backend.is_none() {
                    if let Some(first) = self.detected_backends.as_ref().and_then(|b| b.first()) {
                        ai_settings.backend = Some(first.0.clone());
                        if ai_settings.model.is_none() {
                            ai_settings.model = first.1.first().cloned();
                        }
                    }
                }
            }
        }

        let mut finished = false;
        if let Some(rx) = &self.rx_download {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ModelDownloadProgress::Progress { id, downloaded, total } => {
                        self.downloading = Some(id);
                        if total > 0 {
                            self.download_progress = downloaded as f32 / total as f32;
                        }
                    }
                    ModelDownloadProgress::Done { id } => {
                        self.downloading = None;
                        self.download_progress = 0.0;
                        finished = true;
                        self.status_message = Some((
                            format!("{} downloaded and verified successfully.", id.name()),
                            false,
                        ));
                    }
                    ModelDownloadProgress::Error { id, error } => {
                        self.downloading = None;
                        self.download_progress = 0.0;
                        finished = true;
                        self.status_message = Some((
                            format!("Failed to download {}: {error}", id.name()),
                            true,
                        ));
                    }
                }
            }
        }
        if finished {
            self.rx_download = None;
        }

        if let Some(rx) = &self.rx_claude_test {
            if let Ok(res) = rx.try_recv() {
                self.rx_claude_test = None;
                self.claude_testing = false;
                match res {
                    Ok((models, ms)) => {
                        self.claude_status = Some((
                            format!(
                                "Claude proxy online at {} (model: {}). {} models, {ms} ms.",
                                self.claude_url.trim(),
                                self.claude_model.trim(),
                                models.len()
                            ),
                            false,
                        ));
                        self.claude_models = models;
                    }
                    Err(e) => self.claude_status = Some((e, true)),
                }
            }
        }
    }

    /// The Claude API server backend as the fields describe it. The key is
    /// left empty here unless one was just typed, so it resolves from
    /// `ai.json` at call time and never reaches `workspace.json`.
    pub fn claude_backend(&self) -> Backend {
        Backend::ClaudeApiServer {
            base_url: self.claude_url.trim().to_string(),
            key: self.claude_key_input.trim().to_string(),
        }
    }

    /// "Save key": writes the URL and key to `ai.json` (owner-only), then
    /// clears the field, so the secret does not sit in the UI state.
    pub fn save_claude_key(&mut self) {
        let key = if self.claude_key_input.trim().is_empty() {
            buzz_ai::ai_config::claude_api_key().unwrap_or_default()
        } else {
            self.claude_key_input.trim().to_string()
        };
        match buzz_ai::ai_config::store_claude_api(&self.claude_url, &key) {
            Ok(()) => {
                self.claude_key_input.clear();
                self.claude_key_stored = !key.is_empty();
                self.claude_status = Some((
                    format!(
                        "Saved (key {} in {})",
                        buzz_ai::ai_config::mask_key(&key),
                        key_where()
                    ),
                    false,
                ));
            }
            Err(e) => self.claude_status = Some((format!("Could not save the key: {e}"), true)),
        }
    }

    /// "Copy key from Buzzcaf Studio": file IO only, shows the masked key.
    pub fn copy_key_from_studio(&mut self) {
        match buzz_ai::ai_config::import_from_studio() {
            Ok(rep) => {
                self.claude_url = buzz_ai::ai_config::claude_api_base_url();
                self.claude_model = buzz_ai::ai_config::claude_model();
                self.claude_key_input.clear();
                self.claude_key_stored = true;
                self.claude_status = Some((
                    format!("Copied from Buzzcaf Studio (key {} in {})", rep.masked_key, key_where()),
                    false,
                ));
            }
            Err(e) => self.claude_status = Some((e, true)),
        }
    }

    /// Saves the model field to ai.json.
    pub fn save_claude_model(&mut self) {
        if let Err(e) = buzz_ai::ai_config::store_claude_model(&self.claude_model) {
            self.claude_status = Some((format!("Could not save ai.json: {e}"), true));
        }
    }

    /// Saves the "use Claude" switch to ai.json.
    pub fn save_use_claude(&mut self) {
        if let Err(e) = buzz_ai::ai_config::store_use_claude(self.use_claude) {
            self.claude_status = Some((format!("Could not save ai.json: {e}"), true));
        }
    }

    /// "Test connection": `GET /models` on a thread.
    pub fn test_claude_connection(&mut self) {
        if self.claude_testing {
            return;
        }
        self.claude_testing = true;
        self.claude_status = None;
        let backend = self.claude_backend();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.rx_claude_test = Some(rx);
        let url = self.claude_url.trim().to_string();
        thread::spawn(move || {
            let t = std::time::Instant::now();
            let res = backend
                .list_models()
                .map(|m| (m, t.elapsed().as_millis()))
                .map_err(|e| claude_probe_message(&url, e));
            let _ = tx.send(res);
        });
    }

    /// Triggers asynchronous detection of running local LLM servers.
    pub fn detect_backends(&mut self) {
        if self.is_detecting {
            return;
        }
        self.is_detecting = true;
        let (tx, rx) = crossbeam_channel::unbounded();
        self.rx_detect = Some(rx);
        thread::spawn(move || {
            let res = detect();
            let _ = tx.send(res);
        });
    }

    /// Starts downloading a model in the background.
    pub fn start_download(&mut self, id: ModelId) {
        if self.downloading.is_some() {
            return;
        }
        self.downloading = Some(id);
        self.download_progress = 0.0;
        self.status_message = None;

        self.cancel_flag.store(false, Ordering::SeqCst);
        let cancel_flag = Arc::clone(&self.cancel_flag);

        let (tx, rx): (Sender<ModelDownloadProgress>, Receiver<ModelDownloadProgress>) =
            crossbeam_channel::unbounded();
        self.rx_download = Some(rx);

        thread::spawn(move || {
            let res = ensure(
                id,
                |downloaded, total| {
                    let _ = tx.send(ModelDownloadProgress::Progress {
                        id,
                        downloaded,
                        total,
                    });
                },
                || cancel_flag.load(Ordering::SeqCst),
            );

            match res {
                Ok(_) => {
                    let _ = tx.send(ModelDownloadProgress::Done { id });
                }
                Err(err) => {
                    let _ = tx.send(ModelDownloadProgress::Error {
                        id,
                        error: err.to_string(),
                    });
                }
            }
        });
    }

    /// Cancels active download if any.
    pub fn cancel_download(&mut self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        self.downloading = None;
        self.download_progress = 0.0;
        self.rx_download = None;
        self.status_message = Some(("Download cancelled.".into(), false));
    }
}

/// Renders the AI Models dialog window.
pub fn ai_models_dialog(
    ctx: &Context,
    state: &mut AiModelsState,
    ai_settings: &mut AiSettings,
) {
    if !state.open {
        return;
    }

    state.poll(ai_settings);

    // Initial detection on opening if empty
    if state.detected_backends.is_none() && !state.is_detecting {
        state.detect_backends();
    }

    // **Movable, resizable and always closable.** The window used to be
    // anchored to the screen centre with nothing scrolling but the model list,
    // so it was as tall as everything in it: on a smaller screen the anchor
    // pushed the title bar — and its close button — off the top, and no drag
    // could make it smaller than its contents. Now it opens centred but is
    // free to move, is kept on screen, scrolls its whole body, and closes from
    // the ✕, the Close button or Escape.
    let screen = ctx.viewport_rect();
    let mut open = state.open;
    let mut close = false;
    egui::Window::new("Local AI & Models")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .constrain(true)
        .default_size([680.0, 520.0_f32.min(screen.height() - 80.0)])
        .min_size([360.0, 200.0])
        .max_height(screen.height() - 40.0)
        .default_pos(screen.center())
        .pivot(Align2::CENTER_CENTER)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("ai_models_dialog_body")
                .auto_shrink([false, false])
                .max_height(ui.available_height() - 36.0)
                .show(ui, |ui| {
                    render_dialog_body(ui, state, ai_settings);
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        });
    // Escape closes it, unless a field in it is being typed into — there it
    // only leaves the field, as everywhere else.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && ctx.memory(|m| m.focused().is_none()) {
        close = true;
    }

    state.open = open && !close;
}

/// The dialog's content, drawn into any `Ui` — the AI panel's **Models**
/// section draws exactly this (F7.0).
pub fn models_body(ui: &mut Ui, state: &mut AiModelsState, ai_settings: &mut AiSettings) {
    state.poll(ai_settings);
    if state.detected_backends.is_none() && !state.is_detecting {
        state.detect_backends();
    }
    render_dialog_body(ui, state, ai_settings);
}

/// Which backend and model each AI flow will use, from the settings.
pub fn flow_backends(ai_settings: &AiSettings) -> Vec<(&'static str, String)> {
    let backend = ai_settings
        .backend
        .as_ref()
        .map(|b| b.display_name().to_string())
        .unwrap_or_else(|| "LM Studio / OpenAI-Compatible (default)".into());
    let text = ai_settings.model.clone().unwrap_or_else(|| "(choose a model)".into());
    let vision = ai_settings.vision_model.clone().unwrap_or_else(|| text.clone());
    vec![
        ("Character from Image", "local ONNX (ISNet, DWPose, SAM 2, LaMa)".into()),
        ("Auto-Rig", "local ONNX (DWPose)".into()),
        ("Scene from Image", format!("{backend} \u{00b7} {vision} + SigLIP (local)")),
        ("Direct Story", format!("{backend} \u{00b7} {text}")),
        ("Identify & Rename", format!("{backend} \u{00b7} {vision}")),
        ("Transcribe", "local ONNX (Whisper base.en)".into()),
    ]
}

/// Model names the claude-local-api proxy routes to Claude Code (Buzzcaf Studio's list).
pub const CLAUDE_MODELS: [&str; 7] =
    ["opus", "sonnet", "haiku", "claude-sonnet-5", "claude-haiku-4-5", "claude-opus-5", "default"];

/// Buzzcaf Studio's wording for the probe's failure states.
fn claude_probe_message(url: &str, e: buzz_ai::llm::LlmError) -> String {
    use buzz_ai::llm::LlmError;
    match e {
        LlmError::Http { status: 401 | 403, .. } => {
            "Claude proxy is running but rejected the API key.".into()
        }
        LlmError::Http { status, .. } => format!("Claude proxy answered HTTP {status}."),
        LlmError::Connection(_) | LlmError::Timeout(_) => format!(
            "Claude proxy not reachable at {url}. Start it with start-server.bat start in ../claudeAPI."
        ),
        other => other.to_string(),
    }
}

fn render_claude_server(ui: &mut Ui, state: &mut AiModelsState, ai_settings: &mut AiSettings) {
    ui.label(RichText::new("Claude API server").strong());
    ui.label(
        RichText::new("Your own OpenAI-compatible server (claude-local-api). The key is stored only in the git-ignored .env in the BuzzAnimate folder, never in the layout or the film.")
            .small()
            .weak(),
    );
    ui.label(RichText::new(key_status_line()).small().weak());
    egui::Grid::new("claude_api_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Base URL");
        ui.add(egui::TextEdit::singleline(&mut state.claude_url).desired_width(260.0));
        ui.end_row();
        ui.label("Key");
        ui.horizontal(|ui| {
            let hint = if state.claude_key_stored { "sk-local-\u{2026} (stored)" } else { "paste key" };
            ui.add(
                egui::TextEdit::singleline(&mut state.claude_key_input)
                    .password(true)
                    .hint_text(hint)
                    .desired_width(180.0),
            );
            if ui.button("Save key").clicked() {
                state.save_claude_key();
            }
        });
        ui.end_row();
        ui.label("Model");
        ui.horizontal(|ui| {
            let before = state.claude_model.clone();
            ui.add(egui::TextEdit::singleline(&mut state.claude_model).desired_width(120.0));
            egui::ComboBox::from_id_salt("claude_saved_model")
                .selected_text("pick")
                .show_ui(ui, |ui| {
                    for m in CLAUDE_MODELS {
                        if ui.selectable_label(state.claude_model == m, m).clicked() {
                            state.claude_model = m.to_string();
                        }
                    }
                });
            if state.claude_model != before {
                state.save_claude_model();
            }
        });
        ui.end_row();
    });
    ui.horizontal(|ui| {
        if ui.button("Copy key from Buzzcaf Studio").clicked() {
            state.copy_key_from_studio();
        }
    });
    if ui.checkbox(&mut state.use_claude, "Use Claude for the Assistant").changed() {
        state.save_use_claude();
    }
    ui.label(
        RichText::new("When on, the Assistant's prompts leave this PC (sent through your Claude proxy to Claude).")
            .small()
            .weak(),
    );
    ui.horizontal(|ui| {
        if state.claude_testing {
            ui.spinner();
            ui.label("Testing\u{2026}");
        } else if ui.button("Test connection").clicked() {
            state.test_claude_connection();
        }
        let active = matches!(ai_settings.backend, Some(Backend::ClaudeApiServer { .. }));
        if ui
            .add_enabled(!active, egui::Button::new(if active { "In use" } else { "Use this server" }))
            .clicked()
        {
            ai_settings.backend = Some(Backend::ClaudeApiServer {
                base_url: state.claude_url.trim().to_string(),
                key: String::new(),
            });
            ai_settings.model = Some(buzz_ai::ai_config::CLAUDE_DEFAULT_MODEL.into());
            ai_settings.vision_model = Some(buzz_ai::ai_config::CLAUDE_DEFAULT_MODEL.into());
        }
    });
    if let Some((msg, err)) = &state.claude_status {
        let c = if *err { ui.visuals().error_fg_color } else { ui.visuals().hyperlink_color };
        ui.label(RichText::new(msg).color(c).small());
    }
    if matches!(ai_settings.backend, Some(Backend::ClaudeApiServer { .. })) && !state.claude_models.is_empty() {
        let models = state.claude_models.clone();
        for (label, slot, salt) in [
            ("Chat model", &mut ai_settings.model, "claude_model"),
            ("Vision model", &mut ai_settings.vision_model, "claude_vision"),
        ] {
            ui.horizontal(|ui| {
                ui.label(label);
                egui::ComboBox::from_id_salt(salt)
                    .selected_text(slot.as_deref().unwrap_or(buzz_ai::ai_config::CLAUDE_DEFAULT_MODEL))
                    .show_ui(ui, |ui| {
                        for m in &models {
                            if ui.selectable_label(slot.as_deref() == Some(m.as_str()), m).clicked() {
                                *slot = Some(m.clone());
                            }
                        }
                    });
            });
        }
    }
}

fn render_dialog_body(
    ui: &mut Ui,
    state: &mut AiModelsState,
    ai_settings: &mut AiSettings,
) {
    ui.spacing_mut().item_spacing.y = 8.0;

    ui.heading("Local LLM Backends");
    ui.label(
        RichText::new("Connect to LM Studio (127.0.0.1:1234) or Ollama (127.0.0.1:11434). Text and vision queries never leave your machine.")
            .small()
            .weak(),
    );

    ui.horizontal(|ui| {
        if state.is_detecting {
            ui.spinner();
            ui.label("Probing local ports...");
        } else {
            if ui.button("Refresh Backends").clicked() {
                state.detect_backends();
            }
        }

        if let Some(backends) = &state.detected_backends {
            if backends.is_empty() {
                ui.label(RichText::new("No local LLM detected on port 1234 or 11434").color(ui.visuals().warn_fg_color));
            } else {
                ui.label(format!("Found {} active backend(s)", backends.len()));
            }
        }
    });

    if let Some(backends) = &state.detected_backends {
        if !backends.is_empty() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Active Backend:");
                    let current_base = ai_settings.backend.as_ref().map(|b| b.base_url().to_string()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("ai_backend_select")
                        .selected_text(if let Some(b) = &ai_settings.backend {
                            format!("{} ({})", b.display_name(), b.base_url())
                        } else {
                            "Select backend...".into()
                        })
                        .show_ui(ui, |ui| {
                            for (backend, _) in backends {
                                let label = format!("{} ({})", backend.display_name(), backend.base_url());
                                let selected = backend.base_url() == current_base;
                                if ui.selectable_label(selected, label).clicked() {
                                    ai_settings.backend = Some(backend.clone());
                                }
                            }
                        });
                });

                // Find active backend's models
                let available_models = backends.iter()
                    .find(|(b, _)| Some(b.base_url()) == ai_settings.backend.as_ref().map(|b| b.base_url()))
                    .map(|(_, models)| models.as_slice())
                    .unwrap_or(&[]);

                ui.horizontal(|ui| {
                    ui.label("Default Model:");
                    egui::ComboBox::from_id_salt("ai_model_select")
                        .selected_text(ai_settings.model.as_deref().unwrap_or("Select model..."))
                        .show_ui(ui, |ui| {
                            for m in available_models {
                                let selected = ai_settings.model.as_deref() == Some(m.as_str());
                                if ui.selectable_label(selected, m).clicked() {
                                    ai_settings.model = Some(m.clone());
                                }
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("Vision Model:");
                    egui::ComboBox::from_id_salt("ai_vision_model_select")
                        .selected_text(ai_settings.vision_model.as_deref().unwrap_or("Auto / Same as model"))
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(ai_settings.vision_model.is_none(), "Auto / Same as model").clicked() {
                                ai_settings.vision_model = None;
                            }
                            for m in available_models {
                                let selected = ai_settings.vision_model.as_deref() == Some(m.as_str());
                                if ui.selectable_label(selected, m).clicked() {
                                    ai_settings.vision_model = Some(m.clone());
                                }
                            }
                        });
                });
            });
        }
    }

    ui.separator();
    render_claude_server(ui, state, ai_settings);

    ui.separator();
    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut ai_settings.low_reasoning, "Fast answers (no hidden reasoning)")
            .on_hover_text("Asks LM Studio for reasoning_effort \"none\" and Ollama for think=false. Reasoning models otherwise spend minutes per image batch.")
            .changed()
        {
            buzz_ai::llm::set_low_reasoning(ai_settings.low_reasoning);
        }
    });
    let current = ai_settings.model.clone().unwrap_or_default();
    if !current.is_empty()
        && !buzz_ai::llm::RECOMMENDED_FAST_MODELS.iter().any(|m| current.contains(m))
    {
        ui.label(
            RichText::new(format!(
                "\u{26a0} '{current}' is a reasoning model and slow here (27B, ~7 tokens/s). For speed pick google/gemma-4-12b (local) or sonnet/haiku (Claude API server)."
            ))
            .small()
            .color(ui.visuals().warn_fg_color),
        );
    }
    egui::Grid::new("ai_flow_backends").num_columns(2).striped(true).show(ui, |ui| {
        for (flow, what) in flow_backends(ai_settings) {
            ui.label(RichText::new(flow).small());
            ui.label(RichText::new(what).small().weak());
            ui.end_row();
        }
    });

    ui.separator();

    ui.heading("Pixel & Geometry Models (ONNX)");
    ui.label(
        RichText::new("Used for character segmentation, keypoint detection, SAM-2 parts, and inpainting. Downloaded into local application storage.")
            .small()
            .weak(),
    );

    if let Some((msg, is_err)) = &state.status_message {
        let color = if *is_err {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().hyperlink_color
        };
        ui.label(RichText::new(msg).color(color));
    }

    egui::ScrollArea::vertical()
        .id_salt("ai_models_list")
        .max_height(240.0)
        .show(ui, |ui| {
            for entry in REGISTRY {
                let st = status(entry.id);
                let is_current_dl = state.downloading == Some(entry.id);

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.strong(entry.id.name());
                                let mb = entry.bytes as f64 / 1_048_576.0;
                                ui.label(RichText::new(format!("({mb:.1} MB, {})", entry.licence)).small().weak());
                            });
                            ui.label(RichText::new(entry.description).small().weak());
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if is_current_dl {
                                if ui.button("Cancel").clicked() {
                                    state.cancel_download();
                                }
                                ui.add(
                                    egui::ProgressBar::new(state.download_progress)
                                        .show_percentage()
                                        .desired_width(120.0),
                                );
                            } else {
                                match &st {
                                    ModelStatus::Ready { .. } => {
                                        ui.label(RichText::new("✓ Ready").color(ui.visuals().hyperlink_color));
                                        if ui.button("Re-download").clicked() {
                                            state.start_download(entry.id);
                                        }
                                    }
                                    ModelStatus::Missing => {
                                        if ui.button("Download").clicked() {
                                            state.start_download(entry.id);
                                        }
                                        ui.label(RichText::new("Not Installed").weak());
                                    }
                                    ModelStatus::Corrupted(reason) => {
                                        if ui.button("Redownload").clicked() {
                                            state.start_download(entry.id);
                                        }
                                        ui.label(RichText::new(format!("Damaged ({reason})")).color(ui.visuals().error_fg_color));
                                    }
                                }
                            }
                        });
                    });
                });
            }
        });

    ui.separator();

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Storage path: {}", models_dir().display())).small().weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                state.open = false;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &Context, size: egui::Vec2, events: Vec<egui::Event>, state: &mut AiModelsState, ai: &mut AiSettings) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| ai_models_dialog(ui.ctx(), state, ai));
    }

    fn open_state() -> AiModelsState {
        // Detection already "done", so the test never probes real ports.
        AiModelsState { open: true, detected_backends: Some(Vec::new()), ..Default::default() }
    }

    /// On a short screen the window used to grow to its full contents and,
    /// anchored at the centre, push its title bar and close button off the
    /// top. It must stay on screen, so the ✕ is always reachable.
    #[test]
    fn the_window_fits_a_short_screen_so_its_close_button_is_reachable() {
        let ctx = Context::default();
        let (mut state, mut ai) = (open_state(), AiSettings::default());
        let size = egui::vec2(900.0, 420.0);
        for _ in 0..4 {
            frame(&ctx, size, Vec::new(), &mut state, &mut ai);
        }
        assert!(state.open);
        // The one window this draws: the largest area on screen.
        let rect = ctx
            .memory(|m| {
                m.areas()
                    .visible_layer_ids()
                    .into_iter()
                    .filter_map(|l| m.area_rect(l.id))
                    .max_by(|a, b| a.area().total_cmp(&b.area()))
            })
            .expect("the window was drawn");
        assert!(rect.min.y >= 0.0, "title bar above the screen: {rect:?}");
        assert!(rect.max.y <= size.y + 1.0, "window taller than the screen: {rect:?}");
    }

    #[test]
    fn escape_closes_the_window() {
        let ctx = Context::default();
        let (mut state, mut ai) = (open_state(), AiSettings::default());
        let size = egui::vec2(1280.0, 720.0);
        frame(&ctx, size, Vec::new(), &mut state, &mut ai);
        assert!(state.open);
        let esc = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, size, vec![esc], &mut state, &mut ai);
        assert!(!state.open, "Escape should close the dialog");
    }
}
