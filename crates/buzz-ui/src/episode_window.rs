//! **File ▸ AI ▸ New Episode…** (ANIMATE_COPILOT_PLAN B7): one guided window for
//! making an episode from a story, in five steps shown in a left rail:
//! Story, Cast, Audio, Look, Produce. Each step says whether it is done; Next
//! stays off until the step's inputs are valid.
//!
//! The window only gathers choices and shows what the app tells it. Every click
//! that needs work is returned as an [`EpisodeAction`]; the app carries it out
//! (the same code the old Produce / Story / Casting dialogs run) and writes the
//! answer back into [`EpisodeState`].

use std::collections::HashMap;
use std::path::PathBuf;

use crossbeam_channel::Receiver;
use egui::{Color32, Context, RichText, Ui};

use crate::casting_dialog::{self, CastingAction, CastingDialogState};
use crate::produce_dialog::STYLE_PRESETS;

/// The five steps, in order.
pub const STEPS: [&str; 5] = ["Story", "Cast", "Audio", "Look", "Produce"];

/// The producer's steps in plain words, in order (`buzz_app::produce::Step::ALL`).
pub const PRODUCE_STEPS: [&str; 8] = [
    "Reading the screenplay",
    "Planning scenes and shots",
    "Casting your characters",
    "Drawing the backgrounds",
    "Placing the voices",
    "Building the animation",
    "Rendering the video",
    "Writing the report",
];

/// One audio part found in the parts folder.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PartInfo {
    pub n: usize,
    pub file: String,
    pub seconds: f64,
}

/// One row of the part -> scenes table (after alignment, from `parts.json`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanRow {
    pub n: usize,
    pub file: String,
    pub seconds: f64,
    pub scenes: String,
}

/// What the user did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EpisodeAction {
    #[default]
    None,
    Close,
    /// Story -> screenplay with the model.
    Convert,
    /// The chosen file already is a screenplay: load it as it is.
    UseAsScreenplay,
    /// Write the edited screenplay text back to its file.
    SaveScreenplay,
    CastLoad,
    CastSave,
    FixRig,
    /// Read the parts folder (durations) and `parts.json`.
    ListParts,
    /// A series was picked or typed: load its remembered Look.
    SeriesChanged,
    /// A Look option changed: remember it for the series.
    SaveLook,
    /// A workflow file was picked in the Look step (`EpisodeState::pictures_pick`).
    PicturesChanged,
    Produce,
    CancelProduce,
    /// Open the produced episode in the Producer panel.
    OpenInProducer,
}

/// Which Browse button is waiting for the OS dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Story,
    Out,
    StoryDir,
    Parts,
    Full,
}

/// The window's choices and what the app has told it.
pub struct EpisodeState {
    pub open: bool,
    /// 0..5, an index into [`STEPS`].
    pub step: usize,
    // 1 Story
    pub story: String,
    pub out: String,
    pub series: String,
    /// The "or a new series" box: a name typed for a series that does not exist yet.
    pub new_series: String,
    /// Series with a ComfyUI profile (the app fills it).
    pub series_list: Vec<String>,
    /// The screenplay file in use; empty until converted or loaded.
    pub screenplay_path: String,
    pub screenplay_text: String,
    pub screenplay_dirty: bool,
    pub cast_lines: Vec<String>,
    pub converting: bool,
    // 2 Cast
    /// The story's own folder (`<dir>/animate/*.fla` are its characters).
    pub story_dir: String,
    pub allow_generated: bool,
    // 3 Audio
    pub audio_parts: String,
    pub audio_full: String,
    /// "No recording yet": the episode is timed from the words.
    pub no_audio: bool,
    pub parts: Vec<PartInfo>,
    pub plan: Vec<PlanRow>,
    pub uncovered: Vec<usize>,
    // 4 Look
    pub world: String,
    /// An index into [`STYLE_PRESETS`].
    pub style: usize,
    pub style_text: String,
    pub captions: bool,
    pub objects: bool,
    pub angles: bool,
    /// "Pictures: <flow> (<LoRA files>)" for the series (the app fills it).
    pub pictures: String,
    /// "Characters from: Cast (Raju, ...)" for the series (the app fills it; read-only).
    pub sources: String,
    /// The workflow files in the series' profile folder (empty: no profile).
    pub picture_flows: Vec<String>,
    /// The flow just picked with "Change...", for the app to write.
    pub pictures_pick: Option<String>,
    // 5 Produce
    pub whole: bool,
    /// 1-based, when `whole` is off.
    pub part: usize,
    pub busy: bool,
    /// The producer step running (index into [`PRODUCE_STEPS`]), when one is.
    pub running_step: Option<usize>,
    pub detail: String,
    pub finished: bool,
    pub stills: Vec<PathBuf>,
    pub status: Option<String>,
    /// Rows in the shared casting sheet (kept by the drawing function).
    cast_rows: usize,
    textures: HashMap<PathBuf, Option<egui::TextureHandle>>,
    picking: Option<(Pick, Receiver<Option<PathBuf>>)>,
}

impl Default for EpisodeState {
    fn default() -> Self {
        Self {
            open: false,
            step: 0,
            story: String::new(),
            out: String::new(),
            series: String::new(),
            new_series: String::new(),
            series_list: Vec::new(),
            screenplay_path: String::new(),
            screenplay_text: String::new(),
            screenplay_dirty: false,
            cast_lines: Vec::new(),
            converting: false,
            story_dir: String::new(),
            allow_generated: false,
            audio_parts: String::new(),
            audio_full: String::new(),
            no_audio: false,
            parts: Vec::new(),
            plan: Vec::new(),
            uncovered: Vec::new(),
            world: String::new(),
            style: 0,
            style_text: String::new(),
            captions: false,
            objects: false,
            angles: true,
            pictures: String::new(),
            sources: String::new(),
            picture_flows: Vec::new(),
            pictures_pick: None,
            whole: true,
            part: 1,
            busy: false,
            running_step: None,
            detail: String::new(),
            finished: false,
            stills: Vec::new(),
            status: None,
            cast_rows: 0,
            textures: HashMap::new(),
            picking: None,
        }
    }
}

impl std::fmt::Debug for EpisodeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpisodeState").field("open", &self.open).field("step", &self.step).field("series", &self.series).finish()
    }
}

impl EpisodeState {
    /// Is this step's work finished? (The rail's tick.)
    pub fn done(&self, step: usize) -> bool {
        match step {
            0 => !self.screenplay_path.trim().is_empty() && !self.screenplay_text.trim().is_empty(),
            1 => self.cast_rows > 0 && !self.screenplay_path.trim().is_empty(),
            2 => self.can_next(2),
            3 => self.can_next(3) && self.step > 3,
            4 => self.finished,
            _ => false,
        }
    }

    /// May the creator go on from this step? Next stays off until it holds.
    pub fn can_next(&self, step: usize) -> bool {
        match step {
            // A screenplay is in hand and there is a folder to work in.
            0 => !self.screenplay_path.trim().is_empty() && !self.screenplay_text.trim().is_empty() && !self.out.trim().is_empty() && !self.converting,
            // Somebody plays the people, or the run may draw them.
            1 => self.cast_rows > 0 || self.allow_generated,
            // A recording, or the explicit choice to go without.
            2 => self.no_audio || !self.audio_parts.trim().is_empty() || !self.audio_full.trim().is_empty(),
            // A series to remember the look under.
            3 => !self.series.trim().is_empty(),
            _ => false,
        }
    }

    /// May Produce start?
    pub fn can_produce(&self) -> bool {
        !self.busy
            && self.can_next(0)
            && self.can_next(2)
            && self.can_next(3)
            && (self.whole || (self.part >= 1 && self.part <= self.parts.len().max(self.plan.len())))
    }

    /// Why Produce is off, in plain words.
    pub fn produce_blocker(&self) -> Option<&'static str> {
        if self.busy {
            Some("An episode is being made: watch the steps below, or Cancel.")
        } else if !self.can_next(0) {
            Some("Step 1: choose or convert a story first.")
        } else if !self.can_next(2) {
            Some("Step 3: choose the audio, or tick \"No recording yet\".")
        } else if !self.can_next(3) {
            Some("Step 4: choose a series.")
        } else if !self.whole && !(self.part >= 1 && self.part <= self.parts.len().max(self.plan.len())) {
            Some("\"Part only\" needs the audio parts folder from step 3.")
        } else {
            None
        }
    }

    /// The screenplay is loaded (converted or chosen): put it on screen.
    pub fn set_screenplay(&mut self, path: &str, text: String, cast: Vec<String>) {
        self.screenplay_path = path.to_string();
        self.screenplay_text = text;
        self.screenplay_dirty = false;
        self.cast_lines = cast;
    }

    fn poll(&mut self) -> Option<Pick> {
        let (field, rx) = self.picking.as_ref()?;
        let picked = rx.try_recv().ok()?;
        let field = *field;
        self.picking = None;
        let path = picked?;
        let text = path.display().to_string();
        match field {
            Pick::Story => {
                if self.out.trim().is_empty() {
                    if let Some(dir) = path.parent() {
                        self.out = dir.display().to_string();
                    }
                }
                self.story = text;
            }
            Pick::Out => self.out = text,
            Pick::StoryDir => self.story_dir = text,
            Pick::Parts => {
                self.audio_parts = text;
                self.no_audio = false;
            }
            Pick::Full => {
                self.audio_full = text;
                self.no_audio = false;
            }
        }
        Some(field)
    }

    fn texture(&mut self, ctx: &Context, path: &PathBuf) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.get(path) {
            return t.clone();
        }
        let loaded = image::open(path).ok().map(|d| {
            let rgba = d.thumbnail(320, 180).to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            ctx.load_texture(path.display().to_string(), egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), Default::default())
        });
        self.textures.insert(path.clone(), loaded.clone());
        loaded
    }
}

/// A short "m:ss" for a length in seconds.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Draw the window. The returned action is for the app to carry out.
pub fn episode_window(ctx: &Context, state: &mut EpisodeState, casting: &mut CastingDialogState) -> EpisodeAction {
    if !state.open {
        return EpisodeAction::None;
    }
    state.cast_rows = casting.rows.len();
    let mut action = match state.poll() {
        Some(Pick::Parts) => EpisodeAction::ListParts,
        _ => EpisodeAction::None,
    };
    let mut open = true;
    egui::Window::new("New Episode")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([940.0, 620.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| body(ui, ctx, state, casting, &mut action));
    if !open {
        action = EpisodeAction::Close;
    }
    if action == EpisodeAction::Close {
        state.open = false;
    }
    action
}

fn body(ui: &mut Ui, ctx: &Context, state: &mut EpisodeState, casting: &mut CastingDialogState, action: &mut EpisodeAction) {
    ui.set_min_size(egui::vec2(900.0, 560.0));
    ui.horizontal_top(|ui| {
        // The left rail.
        ui.vertical(|ui| {
            ui.set_width(150.0);
            ui.label(RichText::new("Steps").weak());
            for (i, name) in STEPS.iter().enumerate() {
                let mark = if state.done(i) { "\u{2714}" } else { "\u{25CB}" };
                let colour = if state.done(i) { Color32::from_rgb(90, 190, 110) } else { Color32::GRAY };
                let label = RichText::new(format!("{mark}  {}. {name}", i + 1)).color(if state.step == i { Color32::WHITE } else { colour });
                if ui
                    .add_sized([146.0, 28.0], egui::Button::selectable(state.step == i, label))
                    .on_hover_text(if state.done(i) { "Done" } else { "Still to do" })
                    .clicked()
                {
                    go_to(state, i, action);
                }
            }
            ui.add_space(8.0);
            if let Some(s) = &state.status {
                ui.label(RichText::new(s.as_str()).small().weak());
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_min_width(720.0);
            match state.step {
                0 => story_step(ui, state, action),
                1 => cast_step(ui, ctx, state, casting, action),
                2 => audio_step(ui, state, action),
                3 => look_step(ui, state, action),
                _ => produce_step(ui, ctx, state, action),
            }
            ui.add_space(6.0);
            ui.separator();
            nav(ui, state, action);
        });
    });
}

fn go_to(state: &mut EpisodeState, step: usize, action: &mut EpisodeAction) {
    if state.step == step {
        return;
    }
    state.step = step;
    match step {
        1 => *action = EpisodeAction::CastLoad,
        2 => *action = EpisodeAction::ListParts,
        _ => {}
    }
}

fn nav(ui: &mut Ui, state: &mut EpisodeState, action: &mut EpisodeAction) {
    ui.horizontal(|ui| {
        if ui.add_enabled(state.step > 0, egui::Button::new("< Back")).clicked() {
            let to = state.step - 1;
            go_to(state, to, action);
        }
        if state.step < STEPS.len() - 1 {
            let ok = state.can_next(state.step);
            let tip = if ok { "Go to the next step".to_string() } else { next_blocker(state.step).to_string() };
            if ui.add_enabled(ok, egui::Button::new("Next >")).on_hover_text(tip.clone()).on_disabled_hover_text(tip).clicked() {
                let to = state.step + 1;
                go_to(state, to, action);
            }
            if !ok {
                ui.label(RichText::new(next_blocker(state.step)).weak());
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                *action = EpisodeAction::Close;
            }
        });
    });
}

/// Why Next is off on a step, in plain words.
pub fn next_blocker(step: usize) -> &'static str {
    match step {
        0 => "Convert your story (or load a screenplay) and choose the output folder to go on.",
        1 => "Load the cast, or allow generated characters, to go on.",
        2 => "Choose the audio, or tick \"No recording yet\", to go on.",
        3 => "Choose a series to go on.",
        _ => "",
    }
}

fn pick_button(ui: &mut Ui, state: &mut EpisodeState, field: Pick) {
    if !ui.add_enabled(state.picking.is_none(), egui::Button::new("Browse\u{2026}")).clicked() {
        return;
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    state.picking = Some((field, rx));
    let ctx = ui.ctx().clone();
    std::thread::spawn(move || {
        let dialog = rfd::FileDialog::new();
        let picked = match field {
            Pick::Story => dialog.add_filter("Story or screenplay", &["txt", "md", "fountain"]).pick_file(),
            Pick::Full => dialog.add_filter("Audio", &["wav", "mp3", "m4a", "flac", "ogg"]).pick_file(),
            Pick::Out | Pick::StoryDir | Pick::Parts => dialog.pick_folder(),
        };
        let _ = tx.send(picked);
        ctx.request_repaint();
    });
}

fn heading(ui: &mut Ui, title: &str, blurb: &str) {
    ui.heading(title);
    ui.label(RichText::new(blurb).weak());
    ui.add_space(6.0);
}

fn path_row(ui: &mut Ui, state: &mut EpisodeState, label: &str, field: Pick, hint: &str, tip: &str) {
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new(label));
        let text = match field {
            Pick::Story => &mut state.story,
            Pick::Out => &mut state.out,
            Pick::StoryDir => &mut state.story_dir,
            Pick::Parts => &mut state.audio_parts,
            Pick::Full => &mut state.audio_full,
        };
        ui.add(egui::TextEdit::singleline(text).desired_width(360.0).hint_text(hint)).on_hover_text(tip);
        pick_button(ui, state, field);
    });
}

// ------------------------------------------------------------------ 1 Story

fn story_step(ui: &mut Ui, state: &mut EpisodeState, action: &mut EpisodeAction) {
    heading(ui, "1. Your story", "Pick the story you wrote (or a screenplay that is already written). Your words are copied and checked, never rewritten.");
    path_row(ui, state, "Story file", Pick::Story, "a .txt or .md story", "A first-person story, or a screenplay with '## SCENE' headings");
    path_row(ui, state, "Output folder", Pick::Out, "where the episode is made", "Everything the run makes lands here; a second run picks up where it stopped");
    series_row(ui, state, action);
    ui.horizontal(|ui| {
        let ready = !state.story.trim().is_empty() && !state.out.trim().is_empty() && !state.converting;
        if ui
            .add_enabled(ready, egui::Button::new(if state.converting { "Converting\u{2026}" } else { "Convert story to screenplay" }))
            .on_hover_text("Writes screenplay.md and cast.json. Runs in the Tasks panel, where it can be cancelled")
            .on_disabled_hover_text("Choose the story file and the output folder first")
            .clicked()
        {
            *action = EpisodeAction::Convert;
        }
        if ui
            .add_enabled(!state.story.trim().is_empty() && !state.converting, egui::Button::new("It is already a screenplay: use it"))
            .on_hover_text("Load the file as it is, with no conversion")
            .on_disabled_hover_text("Choose the file first")
            .clicked()
        {
            *action = EpisodeAction::UseAsScreenplay;
        }
    });
    ui.add_space(6.0);
    if state.screenplay_path.trim().is_empty() {
        ui.label(RichText::new("The screenplay appears here when it is ready. You can edit it.").weak());
        return;
    }
    ui.horizontal(|ui| {
        ui.strong("Screenplay");
        ui.label(RichText::new(state.screenplay_path.as_str()).weak().small());
        if ui.add_enabled(state.screenplay_dirty, egui::Button::new("Save changes")).on_disabled_hover_text("Nothing changed yet").clicked() {
            *action = EpisodeAction::SaveScreenplay;
        }
    });
    ui.horizontal_top(|ui| {
        egui::ScrollArea::vertical().id_salt("episode-screenplay").max_height(250.0).max_width(470.0).show(ui, |ui| {
            if ui.add(egui::TextEdit::multiline(&mut state.screenplay_text).desired_width(450.0).desired_rows(14).font(egui::TextStyle::Monospace)).changed() {
                state.screenplay_dirty = true;
            }
        });
        ui.vertical(|ui| {
            ui.strong(format!("Cast ({})", state.cast_lines.len()));
            egui::ScrollArea::vertical().id_salt("episode-cast-lines").max_height(230.0).show(ui, |ui| {
                for line in &state.cast_lines {
                    ui.label(format!("\u{2022} {line}"));
                }
            });
        });
    });
}

fn series_row(ui: &mut Ui, state: &mut EpisodeState, action: &mut EpisodeAction) {
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new("Series"));
        let before = state.series.clone();
        let list = state.series_list.clone();
        egui::ComboBox::from_id_salt("episode-series").selected_text(if state.series.is_empty() { "choose\u{2026}" } else { state.series.as_str() }).width(180.0).show_ui(ui, |ui| {
            for s in &list {
                ui.selectable_value(&mut state.series, s.clone(), s);
            }
        });
        ui.label(RichText::new("or a new series:").weak());
        if state.series != before {
            state.new_series.clear();
            *action = EpisodeAction::SeriesChanged;
        }
        let typed = ui.add(egui::TextEdit::singleline(&mut state.new_series).desired_width(150.0).hint_text("type a new name"));
        typed.clone().on_hover_text("The cast and the look are remembered per series, so the same person keeps the same name and look");
        let name = state.new_series.trim().to_string();
        if typed.lost_focus() && !name.is_empty() && name != state.series {
            state.series = name;
            *action = EpisodeAction::SeriesChanged;
        }
    });
}

// ------------------------------------------------------------------ 2 Cast

fn cast_step(ui: &mut Ui, ctx: &Context, state: &mut EpisodeState, casting: &mut CastingDialogState, action: &mut EpisodeAction) {
    heading(ui, "2. Who plays whom", "Each person of the story is played by one of YOUR characters. Pick another from the list, or lock the ones that must never change.");
    path_row(ui, state, "Story folder", Pick::StoryDir, "optional: the story's own folder", "The story's folder: its animate/*.fla files are its own characters, tried first");
    ui.horizontal(|ui| {
        ui.add_space(134.0);
        ui.checkbox(&mut state.allow_generated, "Allow generated characters")
            .on_hover_text("When none of your characters fits a person, draw one with ComfyUI. Off: that person is listed in the report and left off stage.");
        ui.label(RichText::new("Off: nobody is drawn for you; a person nobody fits is left off stage.").weak().small());
    });
    ui.horizontal(|ui| {
        if ui.button("Load the cast").on_hover_text("Reads cast.json from the output folder and your remembered casting").clicked() {
            *action = EpisodeAction::CastLoad;
        }
        if let Some(s) = &casting.status {
            ui.label(RichText::new(s.as_str()).weak());
        }
    });
    ui.separator();
    let mut inner = CastingAction::None;
    casting_dialog::sheet(ui, ctx, casting, &mut inner);
    match inner {
        CastingAction::Save => *action = EpisodeAction::CastSave,
        CastingAction::FixRig => *action = EpisodeAction::FixRig,
        CastingAction::Load => *action = EpisodeAction::CastLoad,
        _ => {}
    }
}

// ------------------------------------------------------------------ 3 Audio

fn audio_step(ui: &mut Ui, state: &mut EpisodeState, action: &mut EpisodeAction) {
    heading(ui, "3. The voices", "Your recording of the story, in parts (about 3 minutes each) and/or as one full file. The producer finds which scenes each part covers.");
    path_row(ui, state, "Parts folder", Pick::Parts, "part1, part2, ... in order", "The ~3-minute parts, named so they sort in order (part1, part2, ... part10). wav, mp3 or m4a");
    path_row(ui, state, "Full file", Pick::Full, "optional: the whole episode in one file", "The full final recording. With no full file the parts are joined");
    ui.horizontal(|ui| {
        ui.add_space(134.0);
        if ui.checkbox(&mut state.no_audio, "No recording yet").on_hover_text("Make the episode without voices: lines are timed from the words").changed() && state.no_audio {
            state.audio_parts.clear();
            state.audio_full.clear();
            state.parts.clear();
        }
        if ui.add_enabled(!state.audio_parts.trim().is_empty(), egui::Button::new("Refresh")).on_hover_text("Read the parts folder again").clicked() {
            *action = EpisodeAction::ListParts;
        }
    });
    ui.add_space(6.0);
    if !state.parts.is_empty() {
        ui.strong(format!("Parts found ({})", state.parts.len()));
        let total: f64 = state.parts.iter().map(|p| p.seconds).sum();
        egui::Grid::new("episode-parts").striped(true).num_columns(3).show(ui, |ui| {
            for p in &state.parts {
                ui.label(format!("Part {}", p.n));
                ui.label(p.file.as_str());
                ui.label(clock(p.seconds));
                ui.end_row();
            }
            ui.label("");
            ui.label(RichText::new("Total").strong());
            ui.label(clock(total));
            ui.end_row();
        });
    } else if !state.audio_parts.trim().is_empty() {
        ui.label(RichText::new("Reading the parts\u{2026} (or no wav, mp3 or m4a files in that folder)").weak());
    }
    if !state.plan.is_empty() {
        ui.add_space(6.0);
        ui.strong("Which scenes each part covers");
        egui::Grid::new("episode-plan").striped(true).num_columns(3).show(ui, |ui| {
            for r in &state.plan {
                ui.label(format!("Part {}", r.n));
                ui.label(clock(r.seconds));
                ui.label(if r.scenes.is_empty() { "no scene matched".to_string() } else { format!("scenes {}", r.scenes) });
                ui.end_row();
            }
        });
        if state.uncovered.is_empty() {
            ui.colored_label(Color32::from_rgb(90, 190, 110), "Every spoken line is covered by a part.");
        } else {
            let lines: Vec<String> = state.uncovered.iter().take(20).map(ToString::to_string).collect();
            ui.colored_label(Color32::from_rgb(230, 160, 40), format!("\u{26A0} {} line(s) are in no part: {}", state.uncovered.len(), lines.join(", ")));
        }
    } else {
        ui.label(RichText::new("After the first produce run, the part -> scenes table appears here.").weak().small());
    }
}

// ------------------------------------------------------------------ 4 Look

fn look_step(ui: &mut Ui, state: &mut EpisodeState, action: &mut EpisodeAction) {
    heading(ui, "4. The look", "How the episode is drawn. Remembered for the series, so the next episode starts the same way.");
    series_row(ui, state, action);
    ui.horizontal(|ui| {
        ui.add_space(134.0);
        let line = if state.pictures.is_empty() { "Pictures: the default Z-Image flow" } else { state.pictures.as_str() };
        ui.label(line).on_hover_text("Your ComfyUI flow and LoRA used for backgrounds and characters");
        if !state.picture_flows.is_empty() {
            let mut chosen = None;
            egui::ComboBox::from_id_salt("episode-pictures-flow").selected_text("Change\u{2026}").show_ui(ui, |ui| {
                for f in &state.picture_flows {
                    if ui.selectable_label(false, f).clicked() {
                        chosen = Some(f.clone());
                    }
                }
            });
            if chosen.is_some() {
                state.pictures_pick = chosen;
                *action = EpisodeAction::PicturesChanged;
            }
        }
    });
    if !state.sources.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(134.0);
            ui.label(state.sources.as_str()).on_hover_text("Casting draws only from these characters (set in the series' profile)");
        });
    }
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new("World"));
        changed |= ui
            .add(egui::TextEdit::singleline(&mut state.world).desired_width(480.0).hint_text("where and when, e.g. a semi-rural small town in North India, 2010s"))
            .on_hover_text("Where and when the story lives. It ends every background prompt, so the generator does not draw a generic street.")
            .changed();
    });
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new("Style"));
        for (i, name) in STYLE_PRESETS.iter().enumerate() {
            changed |= ui.selectable_value(&mut state.style, i, *name).on_hover_text(style_tip(name)).changed();
        }
        changed |= ui
            .add(egui::TextEdit::singleline(&mut state.style_text).desired_width(260.0).hint_text("more style words, e.g. warm dusty light"))
            .on_hover_text("Extra words added to the style for every background")
            .changed();
    });
    ui.add_space(4.0);
    changed |= ui.checkbox(&mut state.captions, "Captions on the video").on_hover_text("Show the spoken words as subtitles. Story channels usually keep this off.").changed();
    changed |= ui
        .checkbox(&mut state.objects, "Cut objects out of backgrounds")
        .on_hover_text("Turns the chairs, lamps and props of each background into separate pieces you can move. Slower.")
        .changed();
    changed |= ui
        .checkbox(&mut state.angles, "Draw other camera angles")
        .on_hover_text("Makes a separate picture for reverse, low, high and over-the-shoulder shots (needs the Qwen image-edit models). Off: those shots use the wide picture.")
        .changed();
    if changed && !state.series.trim().is_empty() {
        *action = EpisodeAction::SaveLook;
    }
    ui.add_space(4.0);
    ui.label(RichText::new(if state.series.trim().is_empty() { "Choose a series to remember these choices." } else { "These choices are saved for this series as you change them." }).weak().small());
}

fn style_tip(name: &str) -> &'static str {
    match name {
        "comic" => "Bold outlines and flat colour, like a comic book",
        "painterly" => "Soft brush strokes, like a painting",
        "inked" => "Black ink lines on a light wash",
        _ => "",
    }
}

// ------------------------------------------------------------------ 5 Produce

fn produce_step(ui: &mut Ui, ctx: &Context, state: &mut EpisodeState, action: &mut EpisodeAction) {
    heading(ui, "5. Make the episode", "Everything is ready: choose what to make and press Produce. It runs in the Tasks panel; you can keep working.");
    let parts = state.parts.len().max(state.plan.len());
    ui.horizontal(|ui| {
        ui.radio_value(&mut state.whole, true, "Whole episode").on_hover_text("Every scene, with the full audio");
        ui.radio_value(&mut state.whole, false, "One part only").on_hover_text("A quick preview: only the scenes of one audio part");
        ui.add_enabled_ui(!state.whole, |ui| {
            let label = if parts == 0 { "no parts".to_string() } else { format!("Part {}", state.part.clamp(1, parts)) };
            egui::ComboBox::from_id_salt("episode-part").selected_text(label).show_ui(ui, |ui| {
                for n in 1..=parts {
                    ui.selectable_value(&mut state.part, n, format!("Part {n}"));
                }
            });
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let blocker = state.produce_blocker();
        let r = ui.add_enabled(blocker.is_none(), egui::Button::new(if state.busy { "Producing\u{2026}" } else { "Produce" }));
        let r = match blocker {
            Some(why) => r.on_disabled_hover_text(why),
            None => r.on_hover_text("Runs in the Tasks panel, where it can be cancelled"),
        };
        if r.clicked() {
            *action = EpisodeAction::Produce;
        }
        if ui.add_enabled(state.busy, egui::Button::new("Cancel")).on_disabled_hover_text("Nothing is running").clicked() {
            *action = EpisodeAction::CancelProduce;
        }
        if let Some(why) = blocker.filter(|_| !state.busy) {
            ui.label(RichText::new(why).weak());
        }
    });
    ui.add_space(6.0);
    for (i, name) in PRODUCE_STEPS.iter().enumerate() {
        let (mark, colour) = match state.running_step {
            _ if state.finished => ("\u{2714}", Color32::from_rgb(90, 190, 110)),
            Some(r) if i < r => ("\u{2714}", Color32::from_rgb(90, 190, 110)),
            Some(r) if i == r && state.busy => ("\u{25CF}", Color32::from_rgb(120, 170, 255)),
            _ => ("\u{25CB}", Color32::GRAY),
        };
        ui.horizontal(|ui| {
            ui.colored_label(colour, format!("{mark}  {name}"));
            if state.busy && state.running_step == Some(i) && !state.detail.is_empty() {
                ui.label(RichText::new(state.detail.as_str()).weak().small());
            }
        });
    }
    if let Some(s) = &state.status {
        ui.add_space(4.0);
        ui.label(s.as_str());
    }
    if state.finished {
        ui.add_space(6.0);
        if !state.stills.is_empty() {
            ui.strong("A look at the result");
            egui::ScrollArea::horizontal().id_salt("episode-stills").max_height(120.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for p in state.stills.clone().iter().take(12) {
                        match state.texture(ctx, p) {
                            Some(t) => {
                                ui.add(egui::Image::new(&t).fit_to_exact_size(egui::vec2(160.0, 90.0)));
                            }
                            None => {
                                ui.add_sized([160.0, 90.0], egui::Label::new("no picture"));
                            }
                        }
                    }
                });
            });
        }
        if ui.button("Open in Producer panel").on_hover_text("Fix a scene, a shot or a line without making the whole episode again").clicked() {
            *action = EpisodeAction::OpenInProducer;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each step's Next stays off until the step's inputs are valid.
    #[test]
    fn next_is_disabled_until_inputs_are_valid() {
        let mut s = EpisodeState::default();
        assert!(!(0..4).any(|i| s.can_next(i)));
        // 1 Story: screenplay + folder.
        s.screenplay_path = "a/screenplay.md".into();
        s.screenplay_text = "## SCENE 1".into();
        assert!(!s.can_next(0));
        s.out = "a".into();
        assert!(s.can_next(0));
        s.converting = true;
        assert!(!s.can_next(0));
        // 2 Cast: loaded rows, or generated allowed.
        assert!(!s.can_next(1));
        s.cast_rows = 2;
        assert!(s.can_next(1));
        s.cast_rows = 0;
        s.allow_generated = true;
        assert!(s.can_next(1));
        // 3 Audio.
        assert!(!s.can_next(2));
        s.audio_parts = "parts".into();
        assert!(s.can_next(2));
        // 4 Look.
        assert!(!s.can_next(3));
        s.series = "Chirailiya".into();
        assert!(s.can_next(3));
        // Produce needs a part list for "Part only".
        s.converting = false;
        assert!(s.can_produce());
        s.whole = false;
        assert!(!s.can_produce());
        s.parts = vec![PartInfo { n: 1, file: "p1.wav".into(), seconds: 180.0 }];
        assert!(s.can_produce());
    }
}
