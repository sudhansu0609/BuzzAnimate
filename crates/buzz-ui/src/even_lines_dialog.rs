//! **Modify ▸ Shape ▸ Even Out Lines…** (`docs/EVEN_LINES_PLAN.md` E2): face
//! and body line widths, Keep thick and thin, Straighten and Solid above, with
//! a live preview on the stage while it is open.
//!
//! The dialog only edits its settings and raises intentions; the shell owns
//! the traced ink, previews it (exactly what Apply commits) and applies it as
//! one undo step. Cancel puts the artwork back.

use egui::{Context, RichText};

/// What the dialog asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvenLinesAction {
    None,
    /// A width or Keep thick and thin changed: re-width the preview.
    Widths,
    /// Straighten or Solid above changed: trace the preview again — at a
    /// lower scale (`draft`) while a slider drags, full scale on release.
    Retrace { draft: bool },
    Apply,
    Cancel,
}

/// The dialog's state.
#[derive(Debug, Clone)]
pub struct EvenLinesDialogState {
    pub open: bool,
    /// Face lines width, px.
    pub face_width: f64,
    /// Body lines width, px.
    pub body_width: f64,
    /// Keep thick and thin.
    pub keep_weight: bool,
    /// 0–1.
    pub straighten: f64,
    /// Ink deeper than this (px) stays a filled area.
    pub solid_above: f64,
    /// The preview's counts, for the line under the sliders.
    pub face_lines: usize,
    pub body_lines: usize,
    pub solids: usize,
    /// The scope has face ink / body ink (a slider without any is greyed).
    pub has_face: bool,
    pub has_body: bool,
}

impl Default for EvenLinesDialogState {
    fn default() -> Self {
        Self {
            open: false,
            face_width: 1.5,
            body_width: 2.0,
            keep_weight: true,
            straighten: 0.0,
            solid_above: 8.0,
            face_lines: 0,
            body_lines: 0,
            solids: 0,
            has_face: true,
            has_body: true,
        }
    }
}

/// The id under which the last frame's rect of a dialog widget is kept in the
/// context's temp data, so a headless drive can click it: `"apply"`,
/// `"cancel"`, `"face"`, `"body"`, `"keep"`, `"straighten"`, `"solid"`.
pub fn widget_id(what: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    egui::Id::new(("even_lines", what))
}

fn remember(ui: &egui::Ui, id: egui::Id, rect: egui::Rect) {
    ui.ctx().data_mut(|d| d.insert_temp(id, rect));
}

/// Draw the dialog. Returns what was asked for.
pub fn even_lines_dialog(ctx: &Context, state: &mut EvenLinesDialogState) -> EvenLinesAction {
    if !state.open {
        return EvenLinesAction::None;
    }
    let mut action = EvenLinesAction::None;
    let mut open = state.open;
    egui::Window::new("Even Out Lines")
        .open(&mut open)
        .default_width(360.0)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            action = body(ui, state);
        });
    if !open {
        action = EvenLinesAction::Cancel;
    }
    if action == EvenLinesAction::Cancel || action == EvenLinesAction::Apply {
        open = false;
    }
    state.open = open;
    action
}

fn body(ui: &mut egui::Ui, state: &mut EvenLinesDialogState) -> EvenLinesAction {
    let mut action = EvenLinesAction::None;
    ui.label(
        RichText::new(
            "Traced ink becomes strokes of one width along each line, like the Line tool. \
             Every keyframe of every layer in the scope changes.",
        )
        .small()
        .weak(),
    );
    ui.add_space(4.0);
    egui::Grid::new("even_lines_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Face lines");
        let face = ui.add_enabled(
            state.has_face,
            egui::Slider::new(&mut state.face_width, 0.25..=12.0).step_by(0.25).suffix(" px"),
        );
        remember(ui, widget_id("face"), face.rect);
        if face.changed() {
            action = EvenLinesAction::Widths;
        }
        ui.end_row();

        ui.label("Body lines");
        let body = ui.add_enabled(
            state.has_body,
            egui::Slider::new(&mut state.body_width, 0.25..=12.0).step_by(0.25).suffix(" px"),
        );
        remember(ui, widget_id("body"), body.rect);
        if body.changed() {
            action = EvenLinesAction::Widths;
        }
        ui.end_row();

        ui.label("");
        let keep = ui
            .checkbox(&mut state.keep_weight, "Keep thick and thin")
            .on_hover_text("A thick line stays thicker than a thin one (0.5\u{2013}2\u{d7} the width), each even along its length");
        remember(ui, widget_id("keep"), keep.rect);
        if keep.changed() {
            action = EvenLinesAction::Widths;
        }
        ui.end_row();

        ui.label("Straighten");
        let straighten = ui
            .add(egui::Slider::new(&mut state.straighten, 0.0..=1.0).step_by(0.05))
            .on_hover_text("0 keeps the traced curve; from 0.5 lines are built from straight segments, corners kept");
        remember(ui, widget_id("straighten"), straighten.rect);
        if let Some(a) = retrace(&straighten) {
            action = a;
        }
        ui.end_row();

        ui.label("Solid above");
        let solid = ui
            .add(egui::Slider::new(&mut state.solid_above, 0.5..=40.0).step_by(0.25).suffix(" px"))
            .on_hover_text("Ink deeper than this (from its edge to its middle) stays a filled area: hair, pupils, soles");
        remember(ui, widget_id("solid"), solid.rect);
        if let Some(a) = retrace(&solid) {
            action = a;
        }
        ui.end_row();
    });
    ui.label(format!(
        "{} face lines, {} body lines, {} solid areas kept",
        state.face_lines, state.body_lines, state.solids
    ));
    ui.separator();
    ui.horizontal(|ui| {
        let apply = ui.button("Apply").on_hover_text("Replace the ink with these lines as one undo step");
        remember(ui, widget_id("apply"), apply.rect);
        if apply.clicked() {
            action = EvenLinesAction::Apply;
        }
        let cancel = ui.button("Cancel");
        remember(ui, widget_id("cancel"), cancel.rect);
        if cancel.clicked() {
            action = EvenLinesAction::Cancel;
        }
    });
    action
}

/// A slider that needs the ink traced again: a draft while it drags, full
/// scale once it is let go (or changed without a drag).
fn retrace(response: &egui::Response) -> Option<EvenLinesAction> {
    if response.drag_stopped() {
        Some(EvenLinesAction::Retrace { draft: false })
    } else if response.changed() {
        Some(EvenLinesAction::Retrace { draft: response.dragged() })
    } else {
        None
    }
}
