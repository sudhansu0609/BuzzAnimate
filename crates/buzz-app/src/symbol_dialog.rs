//! **Animate's Convert to Symbol / Create New Symbol dialog**: a name and a
//! type, asked as part of the gesture.
//!
//! F8 used to convert straight away, call the result "Symbol", and offer the
//! name for typing in the Library — which did nothing at all when the Library
//! was not on screen, and left the type to a sticky setting at the foot of
//! that panel. Here both are asked up front, as in Animate: the name field is
//! focused with the default selected, so typing replaces it and Enter accepts.
//!
//! Only the menu and keyboard come through here. `Editor::run` still converts
//! without asking, so scripts, the agent and tests are never stopped by a
//! modal.

use buzz_scene::{Scene, SymbolKind};

/// Which of the two commands raised the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// F8: the selection becomes an instance of the new symbol.
    Convert,
    /// Ctrl+F8: an empty symbol, opened for editing.
    New,
}

impl Purpose {
    fn title(self) -> &'static str {
        match self {
            Self::Convert => "Convert to Symbol",
            Self::New => "Create New Symbol",
        }
    }
}

/// The dialog while it is up.
#[derive(Debug, Clone)]
pub struct SymbolDialog {
    pub purpose: Purpose,
    pub name: String,
    pub kind: SymbolKind,
    /// The library folder the symbol will land in, shown so it is no surprise.
    pub folder: Option<String>,
    focused: bool,
}

/// What the person chose this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Cancelled,
    Accepted { name: String, kind: SymbolKind },
}

impl SymbolDialog {
    /// Open with Animate's default name: the first free "Symbol N".
    pub fn new(purpose: Purpose, scene: &Scene, kind: SymbolKind, folder: Option<String>) -> Self {
        let library = scene.library();
        let name = (1..)
            .map(|n| format!("Symbol {n}"))
            .find(|candidate| library.find_by_name(candidate).is_none())
            .unwrap_or_else(|| "Symbol".into());
        Self {
            purpose,
            name,
            kind,
            folder,
            focused: false,
        }
    }

    /// Draw the dialog and say what, if anything, was chosen.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Open;
        egui::Modal::new(egui::Id::new("symbol-dialog")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading(self.purpose.title());
            ui.add_space(8.0);

            let mut enter = false;
            egui::Grid::new("symbol-dialog-grid")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name:");
                    let edit = egui::TextEdit::singleline(&mut self.name)
                        .desired_width(f32::INFINITY)
                        .show(ui);
                    if !self.focused {
                        // Selected, so typing replaces the default outright.
                        edit.response.request_focus();
                        let all = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(self.name.chars().count()),
                        );
                        let mut state = edit.state;
                        state.cursor.set_char_range(Some(all));
                        state.store(ui.ctx(), edit.response.id);
                        self.focused = true;
                    }
                    enter = edit.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();

                    ui.label("Type:");
                    egui::ComboBox::from_id_salt("symbol-dialog-type")
                        .selected_text(self.kind.label())
                        .show_ui(ui, |ui| {
                            for kind in [SymbolKind::MovieClip, SymbolKind::Graphic, SymbolKind::Button] {
                                ui.selectable_value(&mut self.kind, kind, kind.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Folder:");
                    ui.label(
                        egui::RichText::new(self.folder.as_deref().unwrap_or("Library root")).weak(),
                    );
                    ui.end_row();
                });

            ui.add_space(12.0);
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let named = !self.name.trim().is_empty();
            ui.horizontal(|ui| {
                let ok = ui.add_enabled(named, egui::Button::new("OK"));
                if named && (ok.clicked() || enter) {
                    outcome = Outcome::Accepted {
                        name: self.name.trim().to_string(),
                        kind: self.kind,
                    };
                } else if enter {
                    // An empty name: keep the dialog, and the field, up.
                    self.focused = false;
                }
                if ui.button("Cancel").clicked() || escape {
                    outcome = Outcome::Cancelled;
                }
            });
        });
        // The keys that closed the dialog are its own, not the stage's: Enter
        // must not also start playback, nor Escape leave a symbol.
        if outcome != Outcome::Open {
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
        }
        outcome
    }
}
