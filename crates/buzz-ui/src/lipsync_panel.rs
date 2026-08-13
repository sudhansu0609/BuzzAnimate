//! The Lip Sync dialog.
//!
//! Animate's asks three things: which sound, which mouth symbol, and which
//! layer the mouth goes on. This asks the same three, and shows the one thing
//! Animate does not — **which soundtrack it is about to analyse**, by name and
//! duration. That matters here because the sound comes from the document's own
//! timeline while you may be several symbols deep, and being sure you are
//! syncing to the take you think you are is worth a line of text.

use egui::{RichText, Ui};

/// A choice offered in the dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub id: u64,
    pub name: String,
    /// A second line — a duration, a frame count, a warning.
    pub detail: String,
    /// Can this be chosen? A mouth symbol that is too short cannot.
    pub usable: bool,
}

/// Everything the dialog remembers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LipSyncState {
    pub open: bool,
    /// The mouth symbol, by symbol id.
    pub mouth: Option<u64>,
    /// The layer the mouth goes on, by layer id.
    pub layer: Option<u64>,
    /// How loud a frame must be to open the mouth, `0.0..=1.0`.
    pub silence: f32,
    /// Shortest run of one shape, in frames.
    pub hold: u32,
    /// What the last run reported.
    pub result: Option<String>,
}

impl LipSyncState {
    pub fn opened() -> Self {
        Self {
            open: true,
            silence: 0.06,
            hold: 2,
            ..Self::default()
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

/// What the user chose.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct LipSyncResponse {
    /// Run it.
    pub confirmed: bool,
    pub cancelled: bool,
    /// Make a placeholder mouth symbol to draw over.
    pub make_mouth: bool,
}

/// Draw the dialog.
///
/// `track` describes the soundtrack found on the document's timeline, if any.
pub fn lip_sync_dialog(
    ctx: &egui::Context,
    state: &mut LipSyncState,
    track: Option<&str>,
    mouths: &[Choice],
    layers: &[Choice],
) -> LipSyncResponse {
    let mut response = LipSyncResponse::default();
    if !state.open {
        return response;
    }

    let mut still_open = true;
    egui::Window::new("Lip Sync")
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // The soundtrack, named. It comes from the main timeline whichever
            // symbol is open, which is exactly the thing worth confirming.
            match track {
                Some(name) => {
                    ui.label(RichText::new("Soundtrack").small().strong());
                    ui.label(name);
                    ui.label(
                        RichText::new("From the main timeline, wherever you are editing")
                            .small()
                            .weak(),
                    );
                }
                None => {
                    ui.label(
                        RichText::new(
                            "There is no sound on the main timeline.\nImport one with \
                             File ▸ Import Sound, and put it on a keyframe.",
                        )
                        .small()
                        .weak(),
                    );
                }
            }

            ui.separator();
            picker(ui, "Mouth symbol", mouths, &mut state.mouth, "mouth");
            if mouths.iter().all(|m| !m.usable) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("No symbol has a frame per shape.")
                            .small()
                            .weak(),
                    );
                    if ui.small_button("Make one").clicked() {
                        response.make_mouth = true;
                    }
                });
            }

            ui.separator();
            picker(ui, "On layer", layers, &mut state.layer, "layer");

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Silence");
                ui.add(egui::Slider::new(&mut state.silence, 0.0..=0.5).fixed_decimals(2))
                    .on_hover_text("Below this loudness the mouth stays closed");
            });
            ui.horizontal(|ui| {
                ui.label("Hold");
                ui.add(egui::Slider::new(&mut state.hold, 1..=6).suffix(" frames"))
                    .on_hover_text(
                        "Shortest a shape may last. Below two frames a mouth reads as \
                         flickering rather than speaking.",
                    );
            });

            if let Some(result) = &state.result {
                ui.separator();
                ui.label(RichText::new(result).small().weak());
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ready = track.is_some() && state.mouth.is_some() && state.layer.is_some();
                if ui
                    .add_enabled(ready, egui::Button::new("Sync"))
                    .on_hover_text("Analyse the soundtrack and key the mouth")
                    .clicked()
                {
                    response.confirmed = true;
                }
                if ui.button("Close").clicked() {
                    response.cancelled = true;
                }
            });
        });

    if !still_open {
        response.cancelled = true;
    }
    if response.cancelled {
        state.close();
    }
    response
}

/// A list of choices, one selectable.
fn picker(ui: &mut Ui, label: &str, choices: &[Choice], chosen: &mut Option<u64>, salt: &str) {
    ui.label(RichText::new(label).small().strong());

    if choices.is_empty() {
        ui.label(RichText::new("nothing to choose").small().weak());
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt(salt)
        .max_height(110.0)
        .show(ui, |ui| {
            for choice in choices {
                let selected = *chosen == Some(choice.id);
                let text = if choice.detail.is_empty() {
                    choice.name.clone()
                } else {
                    format!("{}   —   {}", choice.name, choice.detail)
                };

                // An unusable choice is shown and disabled rather than hidden:
                // a mouth symbol missing from the list looks like a bug, where
                // one greyed out with "6 frames, needs 10" explains itself.
                let clicked = ui
                    .add_enabled_ui(choice.usable, |ui| ui.selectable_label(selected, text))
                    .inner
                    .clicked();
                if clicked {
                    *chosen = Some(choice.id);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(id: u64, usable: bool) -> Choice {
        Choice {
            id,
            name: format!("Item {id}"),
            detail: String::new(),
            usable,
        }
    }

    #[test]
    fn a_dialog_starts_with_animates_defaults() {
        let state = LipSyncState::opened();
        assert!(state.open);
        assert_eq!(state.hold, 2, "two frames is the readable minimum");
        assert!(state.silence > 0.0 && state.silence < 0.2);
        assert!(state.mouth.is_none());
    }

    #[test]
    fn closing_clears_the_open_flag() {
        let mut state = LipSyncState::opened();
        state.close();
        assert!(!state.open);
    }

    #[test]
    fn a_closed_dialog_draws_nothing_and_reports_nothing() {
        let ctx = egui::Context::default();
        let mut state = LipSyncState::default();
        let mut response = LipSyncResponse::default();

        let _ = ctx.run_ui(Default::default(), |ui| {
            response = lip_sync_dialog(ui.ctx(), &mut state, Some("Line"), &[], &[]);
        });
        assert_eq!(response, LipSyncResponse::default());
    }

    /// The dialog has to draw in every state it can be in, including the ones
    /// that are all warnings.
    #[test]
    fn the_dialog_draws_with_and_without_a_soundtrack() {
        let ctx = egui::Context::default();
        let mouths = vec![choice(1, true), choice(2, false)];
        let layers = vec![choice(10, true)];

        for track in [Some("Dialogue — 4.2 s"), None] {
            let mut state = LipSyncState::opened();
            state.mouth = Some(1);
            state.layer = Some(10);
            let _ = ctx.run_ui(Default::default(), |ui| {
                let _ = lip_sync_dialog(ui.ctx(), &mut state, track, &mouths, &layers);
            });
        }
    }
}
