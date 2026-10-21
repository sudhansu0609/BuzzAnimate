//! **Clear All Keyframes…** (Part F of `docs/MASTER_AUDIO_AND_PROTECT_PLAN.md`):
//! which pose to keep, and whether nested parts get the same treatment.
//!
//! The dialog only collects the choice; the editor applies
//! `Scene::clear_symbol_keyframes` as one undo step.

use buzz_scene::{KeepPose, SymbolId};

/// Dialog state.
#[derive(Debug, Clone, Default)]
pub struct ClearKeysState {
    pub open: bool,
    /// The symbol being cleared.
    pub symbol: Option<SymbolId>,
    /// Its name, for the heading.
    pub name: String,
    /// The inner frame "at playhead" means, when there is one.
    pub playhead: Option<u32>,
    /// Keep the pose at `playhead` rather than the first frame.
    pub keep_playhead: bool,
    /// Clear every symbol reachable inside too.
    pub nested: bool,
}

/// Draw the dialog. Returns `(symbol, pose, nested)` when OK is pressed.
pub fn clear_keys_dialog(
    ctx: &egui::Context,
    state: &mut ClearKeysState,
) -> Option<(SymbolId, KeepPose, bool)> {
    if !state.open {
        return None;
    }
    let mut still_open = true;
    let mut ok = false;
    let mut cancel = false;
    egui::Window::new("Clear All Keyframes")
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.label(format!(
                "Every layer of \u{201C}{}\u{201D} keeps one keyframe; the rest become \
                 normal frames. Lengths stay the same.",
                state.name
            ));
            ui.add_space(6.0);
            ui.label("Keep pose:");
            ui.radio_value(&mut state.keep_playhead, false, "First frame");
            let at = match state.playhead {
                Some(f) => format!("Frame at playhead (frame {})", f + 1),
                None => "Frame at playhead".to_string(),
            };
            ui.add_enabled_ui(state.playhead.is_some(), |ui| {
                ui.radio_value(&mut state.keep_playhead, true, at);
            });
            ui.add_space(4.0);
            ui.checkbox(&mut state.nested, "Include nested parts");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
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
    if !ok {
        return None;
    }
    let symbol = state.symbol?;
    let keep = match (state.keep_playhead, state.playhead) {
        (true, Some(f)) => KeepPose::Frame(f),
        _ => KeepPose::First,
    };
    Some((symbol, keep, state.nested))
}
