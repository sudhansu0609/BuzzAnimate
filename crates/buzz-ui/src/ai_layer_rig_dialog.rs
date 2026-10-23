//! **AI ▸ Rig Character as Layers…** review (plan R3 of
//! `docs/AI_LAYER_RIG_AND_MOTION_COPY_PLAN.md`): the piece → slot table the
//! AI worked out — old layer name, new name (editable), slot (a dropdown of
//! the `biped_full` display names plus "Extra / follow parent") — the nested
//! symbol renames, each with a checkbox, and the notes, then Apply / Cancel.
//!
//! The dialog only edits its rows and raises intentions; the shell owns the
//! plan, works the leads and names out again when a slot changes, and applies
//! it as one undo step.

use egui::{Context, RichText, ScrollArea};

/// The dropdown entry that keeps a piece on its slot without leading it: it
/// follows the slot's lead layer.
pub const EXTRA_LABEL: &str = "Extra / follow parent";

/// One piece of the character.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRigRow {
    /// The layer it is on now.
    pub old: String,
    /// The layer it will get.
    pub new_name: String,
    /// Its slot, an index into [`AiLayerRigState::slots`].
    pub slot: usize,
    /// "Extra / follow parent": never its slot's lead.
    pub extra: bool,
    /// Its slot's lead.
    pub lead: bool,
    /// Named in the notes (matched no body part, or a side call): drawn in
    /// the warning colour.
    pub flagged: bool,
    /// The reviewer typed this name; kept when the names are worked out again.
    pub name_edited: bool,
    /// The reviewer chose this slot (or "Extra"); kept when the plan is made
    /// again ("Split merged drawing" toggled).
    pub slot_edited: bool,
}

/// One nested symbol the plan renames.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRigRenameRow {
    pub from: String,
    pub to: String,
    pub keep: bool,
}

/// What the dialog asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiLayerRigAction {
    None,
    /// Row `n`'s slot (or its extra flag) changed: work the leads and names
    /// out again.
    SlotChanged(usize),
    /// "Split merged drawing" ticked (`true`) or cleared: plan again.
    SplitChanged(bool),
    Apply,
    Cancel,
}

/// The dialog's state.
#[derive(Debug, Clone, Default)]
pub struct AiLayerRigState {
    pub open: bool,
    /// The character symbol's name.
    pub symbol: String,
    /// Where the joints came from ("DWPose" or "fitted to the drawings").
    pub source: String,
    /// 0–1.
    pub confidence: f64,
    pub low_confidence: bool,
    /// The `biped_full` slots' display names.
    pub slots: Vec<String>,
    pub rows: Vec<LayerRigRow>,
    pub renames: Vec<LayerRigRenameRow>,
    pub notes: Vec<String>,
    pub side_calls: Vec<String>,
    /// Vector pieces found merged (one drawing over several body parts).
    pub merged_found: usize,
    /// "Split merged drawing": the plan cut them along the skeleton.
    pub split_merged: bool,
    /// "Rig head like my characters" unticked (it is on by default): Apply
    /// leaves the head symbol's layers as they are (`docs/HEAD_RIG_PLAN.md` H1).
    pub no_head_rig: bool,
}

/// The id under which the last frame's rect of a dialog widget is kept in
/// the context's temp data, so a headless drive can click it: `"apply"`,
/// `"cancel"`, `"split"`, `("name", row)`, `("slot", row)`, `("option", row, k)` with
/// `k` = slot index, or `slots.len()` for [`EXTRA_LABEL`].
pub fn widget_id(what: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    egui::Id::new(("ai_layer_rig", what))
}

fn remember(ui: &egui::Ui, id: egui::Id, rect: egui::Rect) {
    ui.ctx().data_mut(|d| d.insert_temp(id, rect));
}

/// Draw the dialog. Returns what was asked for.
pub fn ai_layer_rig_dialog(ctx: &Context, state: &mut AiLayerRigState) -> AiLayerRigAction {
    if !state.open {
        return AiLayerRigAction::None;
    }
    let mut action = AiLayerRigAction::None;
    let mut open = state.open;
    egui::Window::new("Rig Character as Layers")
        .open(&mut open)
        .default_size([640.0, 560.0])
        .resizable(true)
        .show(ctx, |ui| {
            action = body(ui, state);
        });
    if !open {
        action = AiLayerRigAction::Cancel;
    }
    if action == AiLayerRigAction::Cancel {
        open = false;
    }
    state.open = open;
    action
}

fn body(ui: &mut egui::Ui, state: &mut AiLayerRigState) -> AiLayerRigAction {
    let mut action = AiLayerRigAction::None;
    let warn = ui.visuals().warn_fg_color;
    ui.label(RichText::new(&state.symbol).strong().size(16.0));
    ui.label(format!(
        "Joints: {}   Confidence: {:.0}%",
        state.source,
        state.confidence * 100.0
    ));
    if state.low_confidence {
        ui.label(RichText::new("Lower confidence: check the slots before applying").color(warn));
    }
    ui.label(
        RichText::new(
            "Every piece goes on a layer of its own inside the symbol, named after its body part and \
             linked by layer parenting. Nothing moves on any frame.",
        )
        .small()
        .weak(),
    );
    let mut split = state.split_merged;
    let split_box = ui
        .checkbox(&mut split, "Split merged drawing")
        .on_hover_text(
            "Cut a drawing that covers several body parts along the skeleton into one piece per part,              each overlapping its neighbour across the joint so bends show no gap",
        );
    remember(ui, widget_id("split"), split_box.rect);
    ui.label(
        RichText::new(match state.merged_found {
            0 => "No merged drawing found: the pieces are already separate".to_string(),
            1 => "1 merged drawing found".to_string(),
            n => format!("{n} merged drawings found"),
        })
        .small()
        .weak(),
    );
    let mut rig_head = !state.no_head_rig;
    let head_box = ui.checkbox(&mut rig_head, "Rig head like my characters").on_hover_text(
        "Rebuild the head symbol's face like your Animate heads: eyeLid blink, eyeMask over eyeBall, eyeWhite, brows, mouth symbol, base",
    );
    remember(ui, widget_id("rig_head"), head_box.rect);
    state.no_head_rig = !rig_head;
    if split != state.split_merged {
        state.split_merged = split;
        action = AiLayerRigAction::SplitChanged(split);
    }
    ui.separator();

    let extra_index = state.slots.len();
    let slots = state.slots.clone();
    ScrollArea::vertical()
        .id_salt("ai_layer_rig_rows")
        .max_height((ui.available_height() - 140.0).max(160.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("ai_layer_rig_grid").num_columns(4).striped(true).show(ui, |ui| {
                ui.label(RichText::new("Layer now").strong());
                ui.label("");
                ui.label(RichText::new("New name").strong());
                ui.label(RichText::new("Slot").strong());
                ui.end_row();
                for (index, row) in state.rows.iter_mut().enumerate() {
                    let mut old = RichText::new(&row.old);
                    if row.flagged {
                        old = old.color(warn);
                    }
                    ui.label(old);
                    ui.label("\u{2192}");
                    let edit = ui.add(egui::TextEdit::singleline(&mut row.new_name).desired_width(180.0));
                    remember(ui, widget_id(("name", index)), edit.rect);
                    if edit.changed() {
                        row.name_edited = true;
                    }
                    let shown = if row.extra {
                        EXTRA_LABEL.to_string()
                    } else {
                        let name = slots.get(row.slot).cloned().unwrap_or_default();
                        if row.lead { name } else { format!("{name} (follows)") }
                    };
                    let mut chosen = if row.extra { extra_index } else { row.slot };
                    let combo = egui::ComboBox::from_id_salt(("ai_layer_rig_slot", index))
                        .selected_text(shown)
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            for (k, name) in slots.iter().enumerate() {
                                let r = ui.selectable_value(&mut chosen, k, name);
                                remember(ui, widget_id(("option", index, k)), r.rect);
                            }
                            let r = ui.selectable_value(&mut chosen, extra_index, EXTRA_LABEL);
                            remember(ui, widget_id(("option", index, extra_index)), r.rect);
                        });
                    remember(ui, widget_id(("slot", index)), combo.response.rect);
                    let was = if row.extra { extra_index } else { row.slot };
                    if chosen != was {
                        row.slot_edited = true;
                        if chosen == extra_index {
                            row.extra = true;
                        } else {
                            row.extra = false;
                            row.slot = chosen;
                        }
                        action = AiLayerRigAction::SlotChanged(index);
                    }
                    ui.end_row();
                }
            });

            if !state.renames.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Symbol renames").strong());
                for rename in &mut state.renames {
                    ui.checkbox(&mut rename.keep, format!("{}  \u{2192}  {}", rename.from, rename.to));
                }
            }
            if !state.notes.is_empty() || !state.side_calls.is_empty() {
                ui.add_space(6.0);
                egui::CollapsingHeader::new(format!(
                    "Notes ({})",
                    state.notes.len() + state.side_calls.len()
                ))
                .id_salt("ai_layer_rig_notes")
                .default_open(false)
                .show(ui, |ui| {
                    for note in state.notes.iter().chain(&state.side_calls) {
                        ui.label(RichText::new(note).small());
                    }
                });
            }
        });

    ui.separator();
    let filled = {
        let mut seen: Vec<usize> = state.rows.iter().map(|r| r.slot).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    };
    ui.label(format!("{} pieces on {filled} slots", state.rows.len()));
    ui.horizontal(|ui| {
        let can_apply = !state.rows.is_empty() && state.rows.iter().all(|r| !r.new_name.trim().is_empty());
        let apply = ui
            .add_enabled(can_apply, egui::Button::new("Apply"))
            .on_hover_text("Split, rename and rig the character as one undo step");
        remember(ui, widget_id("apply"), apply.rect);
        if apply.clicked() {
            action = AiLayerRigAction::Apply;
        }
        let cancel = ui.button("Cancel");
        remember(ui, widget_id("cancel"), cancel.rect);
        if cancel.clicked() {
            action = AiLayerRigAction::Cancel;
        }
    });
    action
}
