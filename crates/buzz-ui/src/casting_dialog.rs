//! **File ▸ AI ▸ Casting…** (ANIMATE_COPILOT_PLAN B2): who plays whom, from the
//! creator's OWN characters.
//!
//! Each person of the story is a row: the thumbnail of the character chosen for
//! them (or suggested), up to five alternatives from the shortlist, a Lock box,
//! and the rig-readiness line. Picking an alternative swaps it in. The dialog
//! only holds and edits the rows; the app fills them (`Load`) from the series'
//! `casting.json` and the shortlist, and writes them back (`Save`).

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Context, Ui};

/// One character of his, offered for a person of the story.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CastingPick {
    pub name: String,
    pub folder: String,
    /// Relative to the library root (what `casting.json` keeps).
    pub path: String,
    /// The thumbnail PNG, when the asset index has one.
    pub thumb: Option<PathBuf>,
    pub score: f32,
}

/// One person of the story.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CastingRow {
    /// `casting.json`'s key for them.
    pub key: String,
    pub member: String,
    pub role: String,
    pub chosen: Option<CastingPick>,
    /// Why (the model's sentence, or "the name matches").
    pub reason: String,
    pub locked: bool,
    /// Shown as a suggestion: nothing was kept for them yet.
    pub suggested: bool,
    /// `Woman3Symbol: mouth ok, arms ok` for the chosen character.
    pub readiness: String,
    /// Up to five more from the shortlist.
    pub alternatives: Vec<CastingPick>,
}

impl CastingRow {
    /// The readiness line says the chosen character has no mouth.
    pub fn no_mouth(&self) -> bool {
        self.readiness.contains("no mouth")
    }
}

/// What the user did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CastingAction {
    #[default]
    None,
    /// Read the cast and the choices.
    Load,
    /// Write `casting.json`.
    Save,
    /// The chosen character has no mouth: run the AI Layer Rig action on it.
    FixRig,
    Close,
}

#[derive(Default)]
pub struct CastingDialogState {
    pub open: bool,
    pub series: String,
    /// The episode's output folder, where the story converter put `cast.json`.
    pub out: String,
    pub rows: Vec<CastingRow>,
    pub status: Option<String>,
    /// The rows were changed since they were loaded or saved.
    pub dirty: bool,
    textures: HashMap<PathBuf, Option<egui::TextureHandle>>,
}

impl std::fmt::Debug for CastingDialogState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CastingDialogState").field("open", &self.open).field("series", &self.series).field("rows", &self.rows.len()).finish()
    }
}

impl CastingDialogState {
    fn texture(&mut self, ctx: &Context, path: &PathBuf) -> Option<egui::TextureHandle> {
        if let Some(t) = self.textures.get(path) {
            return t.clone();
        }
        let loaded = image::open(path).ok().map(|d| {
            let rgba = d.thumbnail(160, 160).to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            ctx.load_texture(path.display().to_string(), egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), Default::default())
        });
        self.textures.insert(path.clone(), loaded.clone());
        loaded
    }
}

/// Make `alt` (the `index`th alternative) the chosen character of `row`; the one
/// it replaces becomes an alternative.
pub fn choose_alternative(row: &mut CastingRow, index: usize) {
    if index >= row.alternatives.len() {
        return;
    }
    let picked = row.alternatives.remove(index);
    if let Some(old) = row.chosen.replace(picked) {
        row.alternatives.insert(0, old);
    }
    row.suggested = false;
    row.reason = "picked by you".into();
    row.readiness.clear();
}

/// Draw the dialog, editing `state` in place.
pub fn casting_dialog(ctx: &Context, state: &mut CastingDialogState) -> CastingAction {
    if !state.open {
        return CastingAction::None;
    }
    let mut action = CastingAction::None;
    let mut open = true;
    egui::Window::new("Casting")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(640.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| body(ui, ctx, state, &mut action));
    if !open {
        action = CastingAction::Close;
    }
    if action == CastingAction::Close {
        state.open = false;
    }
    action
}

fn thumb(ui: &mut Ui, ctx: &Context, state: &mut CastingDialogState, pick: &CastingPick, size: f32) {
    let texture = pick.thumb.as_ref().and_then(|p| state.texture(ctx, p));
    match texture {
        Some(t) => {
            ui.add(egui::Image::new(&t).fit_to_exact_size(egui::vec2(size, size)));
        }
        None => {
            ui.add_sized([size, size], egui::Label::new("no picture"));
        }
    }
}

fn body(ui: &mut Ui, ctx: &Context, state: &mut CastingDialogState, action: &mut CastingAction) {
    ui.set_min_width(620.0);
    ui.label("Each person of the story is played by one of YOUR characters. Pick another from the list, lock the ones that must never change; nothing is drawn for you unless the run allows it.");
    ui.separator();
    egui::Grid::new("casting-dialog-top").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Series");
        ui.add(egui::TextEdit::singleline(&mut state.series).desired_width(300.0)).on_hover_text("The choices are kept per series, in the asset library's Cast/<Series>/casting.json");
        ui.end_row();
        ui.label("Episode folder");
        ui.add(egui::TextEdit::singleline(&mut state.out).desired_width(300.0)).on_hover_text("Where cast.json is (the story converter's cast); empty: the series' remembered cast");
        ui.end_row();
    });
    if ui.button("Load cast").clicked() {
        *action = CastingAction::Load;
    }
    if let Some(status) = &state.status {
        ui.label(status.as_str());
    }
    ui.separator();
    sheet(ui, ctx, state, action);
    if ui.button("Close").clicked() {
        *action = CastingAction::Close;
    }
}

/// **The casting sheet**, without a window around it: the rows (thumbnail, the
/// model's reason, the alternatives, Lock, a no-mouth badge with Fix rig) and
/// Save. The Episode window's Cast step draws this too.
pub fn sheet(ui: &mut Ui, ctx: &Context, state: &mut CastingDialogState, action: &mut CastingAction) {
    if state.rows.is_empty() {
        ui.label("No cast loaded yet.");
    }
    let mut swap: Option<(usize, usize)> = None;
    let mut rows = std::mem::take(&mut state.rows);
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        for (r, row) in rows.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                match row.chosen.clone() {
                    Some(pick) => thumb(ui, ctx, state, &pick, 96.0),
                    None => {
                        ui.add_sized([96.0, 96.0], egui::Label::new("no character of yours fits"));
                    }
                }
                ui.vertical(|ui| {
                    let role = if row.role.is_empty() { String::new() } else { format!(" ({})", row.role) };
                    ui.strong(format!("{}{role}", row.member));
                    match &row.chosen {
                        Some(p) => {
                            let tag = if row.suggested { "suggested" } else { "chosen" };
                            ui.label(format!("{tag}: {}  [{}]", p.name, p.folder));
                        }
                        None => {
                            ui.label("left off stage");
                        }
                    }
                    if !row.reason.is_empty() {
                        ui.label(egui::RichText::new(row.reason.as_str()).weak());
                    }
                    if !row.readiness.is_empty() {
                        ui.label(row.readiness.as_str());
                    }
                    if row.no_mouth() {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(230, 160, 40), "\u{26A0} no mouth: this character cannot speak yet");
                            if ui.small_button("Fix rig").on_hover_text("Runs Rig Character as Layers on the open character so its mouth moves. Open the character's file first.").clicked() {
                                *action = CastingAction::FixRig;
                            }
                        });
                    }
                    if ui.checkbox(&mut row.locked, "Lock").on_hover_text("A locked choice is never changed by a later episode").changed() {
                        state.dirty = true;
                    }
                });
            });
            if !row.alternatives.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Instead:");
                    for (a, alt) in row.alternatives.clone().iter().enumerate() {
                        ui.vertical(|ui| {
                            thumb(ui, ctx, state, alt, 56.0);
                            if ui.small_button(alt.name.as_str()).on_hover_text(alt.folder.as_str()).clicked() {
                                swap = Some((r, a));
                            }
                        });
                    }
                });
            }
            ui.separator();
        }
    });
    state.rows = rows;
    if let Some((r, a)) = swap {
        choose_alternative(&mut state.rows[r], a);
        state.dirty = true;
    }
    if ui.add_enabled(!state.rows.is_empty(), egui::Button::new(if state.dirty { "Save casting*" } else { "Save casting" })).clicked() {
        *action = CastingAction::Save;
    }
}
