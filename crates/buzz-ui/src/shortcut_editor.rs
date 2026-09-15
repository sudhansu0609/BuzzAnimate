//! The keyboard-shortcut editor.
//!
//! Every command already carries a default shortcut; this lets the user change
//! them. An override lives on the [`Workspace`] keymap, so it belongs to the
//! person and survives restarts — the same place the theme and layout live. The
//! editor writes straight into that keymap; the caller saves the workspace when
//! this reports a change.
//!
//! Rebinding is a capture: press **Set** on a row, then the next key combination
//! becomes that command's shortcut. It is deliberately live rather than a text
//! field — nobody knows how to *spell* their own keyboard, and typing "Ctrl+K"
//! into a box is a different thing from pressing it.

use egui::{Event, Key, RichText};

use crate::command::{palette_commands, Command};
use crate::workspace::{KeyChord, Workspace};

/// The editor's open state and what it is doing.
#[derive(Debug, Default)]
pub struct ShortcutEditorState {
    pub open: bool,
    filter: String,
    /// The command whose next keypress is being captured, if any.
    capturing: Option<Command>,
}

impl ShortcutEditorState {
    pub fn open(&mut self) {
        self.open = true;
        self.filter.clear();
        self.capturing = None;
    }
}

/// Draw the editor if open, writing any change into `workspace`. Returns whether
/// the keymap changed this frame, so the caller can persist it.
pub fn shortcut_editor(
    ctx: &egui::Context,
    state: &mut ShortcutEditorState,
    workspace: &mut Workspace,
) -> bool {
    if !state.open {
        return false;
    }
    let mut changed = false;

    // If a row is waiting for a key, take the first real key pressed this frame
    // (Escape cancels the capture rather than binding it).
    if let Some(command) = state.capturing {
        let captured = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        if let Some((key, modifiers)) = captured {
            if key != Key::Escape {
                workspace.rebind(command, KeyChord::from_shortcut(egui::KeyboardShortcut::new(modifiers, key)));
                changed = true;
            }
            state.capturing = None;
        }
    }

    let mut open = true;
    egui::Window::new("Keyboard Shortcuts")
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Filter");
                ui.text_edit_singleline(&mut state.filter);
                if ui.button("Reset all").on_hover_text("Return every shortcut to its default").clicked() {
                    workspace.reset_all_shortcuts();
                    changed = true;
                }
            });
            ui.separator();

            let query = state.filter.to_lowercase();
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                egui::Grid::new("shortcut-grid")
                    .num_columns(3)
                    .striped(true)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for command in palette_commands() {
                            let label = command.label();
                            if !query.is_empty() && !label.to_lowercase().contains(&query) {
                                continue;
                            }
                            ui.label(label);

                            // The current binding, or a prompt while capturing.
                            let capturing = state.capturing == Some(command);
                            let shown = if capturing {
                                RichText::new("press a key…").italics()
                            } else {
                                match workspace.shortcut_for(command).map(KeyChord::from_shortcut) {
                                    Some(chord) => RichText::new(chord.label()).monospace(),
                                    None => RichText::new("—").weak(),
                                }
                            };
                            let overridden = workspace.has_shortcut_override(command);
                            let shown = if overridden { shown.strong() } else { shown };
                            ui.label(shown);

                            ui.horizontal(|ui| {
                                let set = if capturing { "…" } else { "Set" };
                                if ui.small_button(set).clicked() {
                                    state.capturing = Some(command);
                                }
                                if ui.small_button("Clear").on_hover_text("No shortcut").clicked() {
                                    workspace.unbind(command);
                                    state.capturing = None;
                                    changed = true;
                                }
                                if ui
                                    .add_enabled(overridden, egui::Button::new("Reset").small())
                                    .on_hover_text("Back to the default")
                                    .clicked()
                                {
                                    workspace.reset_shortcut(command);
                                    changed = true;
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        });

    if !open {
        state.open = false;
        state.capturing = None;
    }
    changed
}
