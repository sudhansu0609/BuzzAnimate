//! **File ▸ AI ▸ Produce Episode…** (PRODUCER_PLAN P6): the form behind
//! [`crate::command::Command::ProduceEpisode`].
//!
//! The dialog only gathers the choices; the app turns them into a
//! `buzz_app::produce::ProduceRequest` and runs it as one AI task in the Tasks
//! panel (progress per step, Cancel), opening the `.buzz` when it is done.

use std::path::PathBuf;

use crossbeam_channel::Receiver;
use egui::Ui;

/// The style presets, by the name `buzz_app::backdrop::Style::by_name` knows.
pub const STYLE_PRESETS: [&str; 3] = ["comic", "painterly", "inked"];

/// Where a series lives, for its background prompts, when the dialog or the
/// Assistant is not told: the Originals series (Chirailiya) is set in rural
/// Bihar; any other series has none.
pub fn default_world(series: &str) -> &'static str {
    if series.trim().eq_ignore_ascii_case("chirailiya") { "rural Bihar, India, late 1990s" } else { "" }
}

/// Which field a Browse… button fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickField {
    Script,
    Audio,
    Characters,
    Out,
}

/// What the user did with the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProduceDialogAction {
    #[default]
    None,
    /// Start producing with the choices as they stand.
    Produce,
    /// Close.
    Cancel,
}

/// The dialog's choices.
#[derive(Debug)]
pub struct ProduceDialogState {
    pub open: bool,
    pub script: String,
    /// Optional.
    pub audio: String,
    /// Optional: `<dir>/<Name>/*.png`.
    pub characters: String,
    /// An index into [`STYLE_PRESETS`].
    pub style: usize,
    /// Words added to the preset's.
    pub style_text: String,
    pub series: String,
    /// Where and when the story lives: ends every background prompt.
    pub world: String,
    pub out: String,
    /// `4,5` or `4-6`; empty for every scene.
    pub scenes: String,
    pub backgrounds: bool,
    pub launch_comfy: bool,
    pub mp4: bool,
    /// A run is going (the button waits for it).
    pub busy: bool,
    /// The last thing to say: an input error, or that a run started.
    pub status: Option<String>,
    picking: Option<(PickField, Receiver<Option<PathBuf>>)>,
}

impl Default for ProduceDialogState {
    fn default() -> Self {
        Self {
            open: false,
            script: String::new(),
            audio: String::new(),
            characters: String::new(),
            style: 0,
            style_text: String::new(),
            series: String::new(),
            world: String::new(),
            out: String::new(),
            scenes: String::new(),
            backgrounds: true,
            launch_comfy: true,
            mp4: true,
            busy: false,
            status: None,
            picking: None,
        }
    }
}

impl ProduceDialogState {
    /// The scene numbers typed, `None` for every scene.
    pub fn scene_numbers(&self) -> Result<Option<Vec<u32>>, String> {
        let text = self.scenes.trim();
        if text.is_empty() {
            return Ok(None);
        }
        let mut out = Vec::new();
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let bad = || format!("scenes: {part:?} is not a number or a range like 4-6");
            if let Some((a, b)) = part.split_once('-') {
                let (a, b): (u32, u32) = (a.trim().parse().map_err(|_| bad())?, b.trim().parse().map_err(|_| bad())?);
                out.extend(a..=b);
            } else {
                out.push(part.parse().map_err(|_| bad())?);
            }
        }
        Ok(Some(out))
    }

    fn poll(&mut self) {
        let Some((field, rx)) = &self.picking else { return };
        let Ok(picked) = rx.try_recv() else { return };
        let field = *field;
        self.picking = None;
        if let Some(path) = picked {
            let text = path.display().to_string();
            match field {
                PickField::Script => self.script = text,
                PickField::Audio => self.audio = text,
                PickField::Characters => self.characters = text,
                PickField::Out => self.out = text,
            }
        }
    }
}

/// Draw the dialog, editing `state` in place.
pub fn produce_dialog(ctx: &egui::Context, state: &mut ProduceDialogState) -> ProduceDialogAction {
    if !state.open {
        return ProduceDialogAction::None;
    }
    state.poll();
    let mut action = ProduceDialogAction::None;
    let mut open = true;
    egui::Window::new("Produce Episode")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| body(ui, state, &mut action));
    if !open {
        action = ProduceDialogAction::Cancel;
    }
    if action == ProduceDialogAction::Cancel {
        state.open = false;
    }
    action
}

fn browse(ui: &mut Ui, state: &mut ProduceDialogState, field: PickField) {
    let picking = state.picking.is_some();
    if !ui.add_enabled(!picking, egui::Button::new("Browse\u{2026}")).clicked() {
        return;
    }
    // The OS dialog blocks whatever thread calls it; not this one.
    let (tx, rx) = crossbeam_channel::bounded(1);
    state.picking = Some((field, rx));
    let ctx = ui.ctx().clone();
    std::thread::spawn(move || {
        let dialog = rfd::FileDialog::new();
        let picked = match field {
            PickField::Script => dialog.add_filter("Script", &["txt", "md", "fountain"]).pick_file(),
            PickField::Audio => dialog.add_filter("Audio", &["wav", "mp3", "m4a", "flac", "ogg"]).pick_file(),
            PickField::Characters | PickField::Out => dialog.pick_folder(),
        };
        let _ = tx.send(picked);
        ctx.request_repaint();
    });
}

fn body(ui: &mut Ui, state: &mut ProduceDialogState, action: &mut ProduceDialogAction) {
    ui.set_min_width(460.0);
    ui.label("A script, the dialogue recording and the character art in; a finished episode out.");
    ui.separator();
    egui::Grid::new("produce-dialog-grid").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
        let rows: [(&str, PickField, &str); 4] = [
            ("Script", PickField::Script, "The screenplay: '## SCENE n — TITLE' headings, cues, action"),
            ("Audio (optional)", PickField::Audio, "The dialogue recording of the chosen scenes, in script order"),
            ("Characters folder (optional)", PickField::Characters, "One sub-folder of pictures per character"),
            ("Output folder", PickField::Out, "Everything the run makes lands here; a second run picks up where it stopped"),
        ];
        for (label, field, hint) in rows {
            ui.label(label);
            let text = match field {
                PickField::Script => &mut state.script,
                PickField::Audio => &mut state.audio,
                PickField::Characters => &mut state.characters,
                PickField::Out => &mut state.out,
            };
            ui.add(egui::TextEdit::singleline(text).desired_width(280.0)).on_hover_text(hint);
            browse(ui, state, field);
            ui.end_row();
        }
        ui.label("Style");
        ui.horizontal(|ui| {
            for (i, name) in STYLE_PRESETS.iter().enumerate() {
                ui.selectable_value(&mut state.style, i, *name);
            }
        });
        ui.end_row();
        ui.label("More style words");
        ui.add(egui::TextEdit::singleline(&mut state.style_text).desired_width(280.0).hint_text("optional, e.g. warm dusty light"));
        ui.end_row();
        ui.label("Series name");
        let series = ui.add(egui::TextEdit::singleline(&mut state.series).desired_width(280.0));
        // The series' own world, filled in while the field is empty.
        if series.changed() && state.world.trim().is_empty() {
            state.world = default_world(&state.series).to_string();
        }
        ui.end_row();
        ui.label("World");
        ui.add(egui::TextEdit::singleline(&mut state.world).desired_width(280.0).hint_text("where and when, e.g. rural Bihar, India, late 1990s"))
            .on_hover_text("Ends every background prompt, so the generator does not draw a generic street");
        ui.end_row();
        ui.label("Scenes");
        ui.add(egui::TextEdit::singleline(&mut state.scenes).desired_width(280.0).hint_text("all; or 4,5 or 4-6"));
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.checkbox(&mut state.backgrounds, "Generate backgrounds with ComfyUI");
    ui.add_enabled(state.backgrounds, egui::Checkbox::new(&mut state.launch_comfy, "Start ComfyUI if it is not running"));
    ui.checkbox(&mut state.mp4, "Render MP4");
    if let Some(status) = &state.status {
        ui.add_space(4.0);
        ui.label(status.as_str());
    }
    ui.separator();
    ui.horizontal(|ui| {
        let ready = !state.script.trim().is_empty() && !state.out.trim().is_empty() && !state.busy;
        if ui
            .add_enabled(ready, egui::Button::new(if state.busy { "Producing\u{2026}" } else { "Produce" }))
            .on_hover_text("Runs in the Tasks panel, where it can be cancelled")
            .clicked()
        {
            *action = ProduceDialogAction::Produce;
        }
        if ui.button("Close").clicked() {
            *action = ProduceDialogAction::Cancel;
        }
    });
}
