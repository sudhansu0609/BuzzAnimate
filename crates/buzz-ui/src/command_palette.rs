//! A command palette: press Ctrl+K, type a few letters, run any command.
//!
//! Everything a menu can do is already a [`Command`] with a label and a shortcut
//! — this is a search box over that catalogue, so a command buried three menus
//! deep is two keystrokes away. It runs whatever the list has highlighted; the
//! arrow keys move the highlight and Enter runs it.

use egui::{Key, Modifiers};

use crate::command::{palette_commands, Command};

/// The palette's open state and current query. Held by the shell across frames.
#[derive(Debug, Default)]
pub struct CommandPaletteState {
    pub open: bool,
    query: String,
    selected: usize,
    /// Set the frame the palette opens, so the search box takes focus once.
    just_opened: bool,
}

impl CommandPaletteState {
    /// Open (or re-open) the palette with an empty query.
    pub fn open(&mut self) {
        self.open = true;
        self.just_opened = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.open = false;
        } else {
            self.open();
        }
    }
}

/// Draw the palette if it is open, and return the command the user chose (if
/// any). `enabled` reports whether a command can run right now — disabled ones
/// are shown greyed and cannot be picked, so the palette never silently does
/// nothing.
pub fn command_palette(
    ctx: &egui::Context,
    state: &mut CommandPaletteState,
    enabled: impl Fn(Command) -> bool,
) -> Option<Command> {
    if !state.open {
        return None;
    }

    // Escape closes without running anything.
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        state.open = false;
        return None;
    }

    // The commands matching the query, in catalogue order.
    let query = state.query.to_lowercase();
    let matches: Vec<Command> = palette_commands()
        .into_iter()
        .filter(|c| query.is_empty() || c.label().to_lowercase().contains(&query))
        .collect();

    // Arrow keys move the highlight; wrap so it can't run off the ends.
    if !matches.is_empty() {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            state.selected = (state.selected + 1) % matches.len();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            state.selected = (state.selected + matches.len() - 1) % matches.len();
        }
    }
    state.selected = state.selected.min(matches.len().saturating_sub(1));

    let mut chosen: Option<Command> = None;

    egui::Window::new("Command palette")
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
        .fixed_size([420.0, 0.0])
        .show(ctx, |ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .hint_text("Run a command\u{2026}")
                    .desired_width(f32::INFINITY),
            );
            if state.just_opened {
                edit.request_focus();
                state.just_opened = false;
            }
            // Enter runs the highlighted command, if it is runnable.
            let submit = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));

            ui.separator();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (i, command) in matches.iter().enumerate() {
                    let runnable = enabled(*command);
                    let label = command.label();
                    let text = if runnable {
                        egui::RichText::new(label)
                    } else {
                        egui::RichText::new(label).weak()
                    };
                    let row = ui.selectable_label(i == state.selected, text);
                    if row.hovered() {
                        state.selected = i;
                    }
                    if runnable && row.clicked() {
                        chosen = Some(*command);
                    }
                }
            });

            if submit {
                if let Some(command) = matches.get(state.selected).copied() {
                    if enabled(command) {
                        chosen = Some(command);
                    }
                }
                // Keep focus on the box so typing continues to work if the
                // chosen command didn't close the palette.
                edit.request_focus();
            }
        });

    if chosen.is_some() {
        state.open = false;
    }
    chosen
}
