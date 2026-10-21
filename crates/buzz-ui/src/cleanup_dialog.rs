//! **Clean Up Layers…** (L2 of `docs/LAYER_MANAGEMENT_PLAN.md`): which passes
//! run, over which scope, with live counts.
//!
//! The dialog only collects the choice; the editor applies
//! `Scene::cleanup_layers` as one undo step. The counts come from
//! `Scene::cleanup_preview`, the same plan the apply runs, recomputed only
//! when the scene revision or a choice changes.

use buzz_scene::{CleanupOptions, CleanupReport, CleanupScope, Scene};

/// Dialog state.
#[derive(Debug, Clone, Default)]
pub struct CleanupState {
    pub open: bool,
    pub options: CleanupOptions,
    pub scope: CleanupScope,
    /// The preview, keyed by what it was computed for.
    preview: Option<((u64, Vec<buzz_scene::SymbolId>, CleanupScope, CleanupOptions), CleanupReport)>,
}

impl CleanupState {
    /// Open with every pass on and "This timeline" chosen.
    pub fn open_fresh(&mut self) {
        *self = Self {
            open: true,
            ..Self::default()
        };
    }
}

/// Draw the dialog. Returns `(scope, options)` when OK is pressed.
pub fn cleanup_dialog(
    ctx: &egui::Context,
    state: &mut CleanupState,
    scene: &Scene,
) -> Option<(CleanupScope, CleanupOptions)> {
    if !state.open {
        return None;
    }
    let mut still_open = true;
    let mut ok = false;
    let mut cancel = false;
    egui::Window::new("Clean Up Layers")
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.checkbox(&mut state.options.empty_layers, "Delete empty layers");
            ui.checkbox(&mut state.options.trim, "Trim trailing blank frames");
            ui.checkbox(&mut state.options.folders, "Remove empty folders");
            ui.add_space(6.0);
            ui.label("Scope:");
            ui.radio_value(&mut state.scope, CleanupScope::Timeline, "This timeline");
            ui.radio_value(&mut state.scope, CleanupScope::WholeScene, "Whole scene (stage and every symbol)");
            ui.add_space(6.0);
            let key = (
                scene.revision(),
                scene.edit_path().to_vec(),
                state.scope,
                state.options,
            );
            if state.preview.as_ref().is_none_or(|(k, _)| *k != key) {
                let report = scene.cleanup_preview(state.scope, state.options);
                state.preview = Some((key, report));
            }
            let report = state.preview.as_ref().map(|(_, r)| *r).unwrap_or_default();
            ui.label(format!(
                "{} layers deleted, {} frames trimmed, {} folders removed{}",
                report.layers_deleted,
                report.frames_trimmed,
                report.folders_removed,
                if report.symbols_touched > 0 {
                    format!(" ({} symbols)", report.symbols_touched)
                } else {
                    String::new()
                }
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!report.is_empty(), egui::Button::new("OK")).clicked() {
                    ok = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if !still_open || cancel || ok {
        state.open = false;
    }
    ok.then_some((state.scope, state.options))
}
