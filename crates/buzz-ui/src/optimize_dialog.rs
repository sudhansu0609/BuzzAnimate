//! **Modify ▸ Shape ▸ Optimize Shapes…** (`docs/TRACE_PICTURE_PLAN.md` T13):
//! Smoothing (curve-fit tolerance), Merge same colour and Remove tiny, with a
//! live preview on the stage while it is open.
//!
//! The dialog only edits its settings and raises intentions; the shell does
//! the work off the UI thread, previews it (exactly what Apply commits) and
//! applies it as one undo step. Cancel puts the artwork back.

use egui::{Context, RichText};

/// What the dialog asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizeAction {
    None,
    /// A setting changed: optimize again for the preview.
    Changed,
    Apply,
    Cancel,
}

/// The dialog's state.
#[derive(Debug, Clone)]
pub struct OptimizeDialogState {
    pub open: bool,
    /// Curve-fit tolerance, px (0: curves untouched).
    pub smoothing: f64,
    pub merge_same_colour: bool,
    /// Solid shapes under this many px² are absorbed (0: none).
    pub remove_tiny: f64,
    /// The preview's counts.
    pub shapes_before: usize,
    pub shapes_after: usize,
    pub segments_before: usize,
    pub segments_after: usize,
    /// The work runs off the UI thread: Apply waits.
    pub working: bool,
}

impl Default for OptimizeDialogState {
    fn default() -> Self {
        Self {
            open: false,
            smoothing: 0.75,
            merge_same_colour: true,
            remove_tiny: 1.0,
            shapes_before: 0,
            shapes_after: 0,
            segments_before: 0,
            segments_after: 0,
            working: false,
        }
    }
}

/// The id under which the last frame's rect of a dialog widget is kept in the
/// context's temp data, so a headless drive can click it: `"apply"`,
/// `"cancel"`, `"smoothing"`, `"merge"`, `"tiny"`.
pub fn widget_id(what: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    egui::Id::new(("optimize_shapes", what))
}

fn remember(ui: &egui::Ui, id: egui::Id, rect: egui::Rect) {
    ui.ctx().data_mut(|d| d.insert_temp(id, rect));
}

/// Draw the dialog. Returns what was asked for.
pub fn optimize_dialog(ctx: &Context, state: &mut OptimizeDialogState) -> OptimizeAction {
    if !state.open {
        return OptimizeAction::None;
    }
    let mut action = OptimizeAction::None;
    let mut open = state.open;
    egui::Window::new("Optimize Shapes")
        .open(&mut open)
        .default_width(360.0)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            action = body(ui, state);
        });
    if !open {
        action = OptimizeAction::Cancel;
    }
    if action == OptimizeAction::Cancel || action == OptimizeAction::Apply {
        open = false;
    }
    state.open = open;
    action
}

fn body(ui: &mut egui::Ui, state: &mut OptimizeDialogState) -> OptimizeAction {
    let mut action = OptimizeAction::None;
    ui.label(
        RichText::new(
            "Fewer curve segments and shapes for the selection (else the active layer's frame), \
             like Modify \u{25b8} Shape \u{25b8} Optimize in Animate.",
        )
        .small()
        .weak(),
    );
    ui.add_space(4.0);
    egui::Grid::new("optimize_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Smoothing");
        let smoothing = ui
            .add(egui::Slider::new(&mut state.smoothing, 0.0..=4.0).step_by(0.05).suffix(" px"))
            .on_hover_text("How far a curve may move to use fewer segments; 0 leaves the curves alone");
        remember(ui, widget_id("smoothing"), smoothing.rect);
        if smoothing.changed() {
            action = OptimizeAction::Changed;
        }
        ui.end_row();

        ui.label("Remove tiny");
        let tiny = ui
            .add(egui::Slider::new(&mut state.remove_tiny, 0.0..=50.0).step_by(0.5).suffix(" px\u{b2}"))
            .on_hover_text("Solid shapes smaller than this take their largest neighbour's colour; 0 keeps them");
        remember(ui, widget_id("tiny"), tiny.rect);
        if tiny.changed() {
            action = OptimizeAction::Changed;
        }
        ui.end_row();

        ui.label("");
        let merge = ui
            .checkbox(&mut state.merge_same_colour, "Merge same colour")
            .on_hover_text("Touching shapes of one colour become one shape");
        remember(ui, widget_id("merge"), merge.rect);
        if merge.changed() {
            action = OptimizeAction::Changed;
        }
        ui.end_row();
    });
    if state.working {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Working\u{2026}");
        });
    } else {
        ui.label(format!(
            "{} \u{2192} {} shapes, {} \u{2192} {} segments",
            state.shapes_before, state.shapes_after, state.segments_before, state.segments_after
        ));
    }
    ui.separator();
    ui.horizontal(|ui| {
        let apply = ui
            .add_enabled(!state.working, egui::Button::new("Apply"))
            .on_hover_text("Replace the shapes with these as one undo step");
        remember(ui, widget_id("apply"), apply.rect);
        if apply.clicked() {
            action = OptimizeAction::Apply;
        }
        let cancel = ui.button("Cancel");
        remember(ui, widget_id("cancel"), cancel.rect);
        if cancel.clicked() {
            action = OptimizeAction::Cancel;
        }
    });
    action
}
