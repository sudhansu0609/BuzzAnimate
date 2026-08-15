//! The Library panel.
//!
//! Animate's Library is where every symbol in a document lives, organised into
//! folders. This panel keeps that model: a folder tree, a search box that
//! filters across the whole library at once, a use count per symbol so you can
//! see what is safe to delete, and the same set of operations on the strip
//! along the bottom.
//!
//! # Why the folder tree is derived, not stored
//!
//! [`buzz_scene::Library`] stores a symbol's folder as a path string plus a set
//! of folder paths, exactly as XFL does. That means the tree shown here is
//! computed each frame from those two facts rather than held as a parallel
//! structure that could drift out of step with the symbols. Rebuilding it is
//! cheap — a library with thousands of symbols is a handful of string
//! comparisons per row, and only visible rows are built.

use std::collections::BTreeSet;

use buzz_scene::{Scene, SymbolId, SymbolKind};
use egui::{RichText, Ui};

use crate::command::Command;

/// Panel state that is not part of the document.
///
/// Which folders are open and what is typed in the search box are view state:
/// they must survive a frame but must never end up in the saved file or in the
/// undo history.
#[derive(Debug, Clone, Default)]
pub struct LibraryState {
    /// The symbol whose properties the panel is showing.
    pub selected: Option<SymbolId>,
    /// The folder currently highlighted, for "new symbol goes here".
    pub selected_folder: Option<String>,
    /// Free-text filter. Matches symbol names, case-insensitively.
    pub search: String,
    /// What kind of symbol New Symbol and Convert to Symbol produce.
    ///
    /// Animate asks in a dialog; this is the same choice, kept as a sticky
    /// setting so a run of conversions does not need one click each.
    pub new_symbol_kind: SymbolKind,
    /// Folder paths the user has opened.
    expanded: BTreeSet<String>,
    /// Rename buffer, live only while a rename is in progress.
    renaming: Option<(SymbolId, String)>,
}

impl LibraryState {
    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
    }

    /// Open every folder, which is what a search needs in order to show hits
    /// buried several levels down.
    pub fn expand_all(&mut self, scene: &Scene) {
        self.expanded = scene.library().folders().cloned().collect();
    }

    /// Is the panel filtering?
    pub fn is_searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    fn matches(&self, name: &str) -> bool {
        let needle = self.search.trim().to_lowercase();
        needle.is_empty() || name.to_lowercase().contains(&needle)
    }
}

/// Draw the Library panel.
///
/// Takes the scene mutably because renaming a symbol and moving it between
/// folders are edits in their own right; the caller wraps the call in an undo
/// step, and a frame where nothing changed records nothing.
pub fn library_panel(ui: &mut Ui, scene: &mut Scene, state: &mut LibraryState) -> Option<Command> {
    let mut command = None;

    ui.horizontal(|ui| {
        ui.heading("Library");
        ui.label(
            RichText::new(format!("{} items", scene.library().len()))
                .small()
                .weak(),
        );
    });

    // Search. Typing opens every folder, because a hit inside a closed folder
    // that the user cannot see is worse than no hit at all.
    ui.horizontal(|ui| {
        ui.label("🔍");
        if ui.text_edit_singleline(&mut state.search).changed() && state.is_searching() {
            state.expand_all(scene);
        }
        if !state.search.is_empty() && ui.small_button("x").on_hover_text("Clear").clicked() {
            state.search.clear();
        }
    });
    ui.separator();

    let usage = scene.symbol_usage();

    // **A fixed height, not the room available.**
    //
    // This panel keeps a scroll area of its own, because a library of three
    // hundred symbols has to be scrollable without carrying the whole dock
    // column with it. But it sits *inside* the column's scroll area, and there
    // `available_height` is not the height of the window — it is however much
    // room the column is willing to promise, which is effectively all of it.
    // Asking for that meant the library grew to fill the column and pushed the
    // Assets panel below it clean off the bottom of the screen, where nobody
    // was ever going to find it.
    //
    // So: a definite number of rows. Enough to browse in, and never enough to
    // hide what comes after it.
    egui::ScrollArea::vertical()
        .id_salt("library-items")
        .auto_shrink([false, true])
        .max_height(300.0)
        .show(ui, |ui| {
            if scene.library().is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "The library is empty.\n\n\
                         Select artwork and press F8 to convert it to a symbol, \
                         or use File > Import to Library to bring in an Animate document.",
                    )
                    .weak()
                    .italics(),
                );
                return;
            }

            // Root level first, then its folders, recursively.
            draw_folder_contents(ui, scene, state, None, 0, &usage, &mut command);
        });

    ui.separator();
    draw_footer(ui, scene, state, &usage, &mut command);

    command
}

/// One level of the tree: the folders directly inside `parent`, then the
/// symbols that sit in `parent` itself.
///
/// Folders come first because that is where Animate puts them, and because a
/// long symbol list would otherwise push the folders off the top.
fn draw_folder_contents(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    parent: Option<&str>,
    depth: usize,
    usage: &std::collections::BTreeMap<SymbolId, usize>,
    command: &mut Option<Command>,
) {
    let indent = depth as f32 * 14.0;

    for folder in scene.library().child_folders(parent) {
        let leaf = folder.rsplit('/').next().unwrap_or(&folder).to_string();
        let open = state.is_expanded(&folder);
        let selected = state.selected_folder.as_deref() == Some(folder.as_str());

        ui.horizontal(|ui| {
            ui.add_space(indent);
            // `⏷` rather than `▼`: the bundled fonts have a glyph for U+23F7
            // and none for U+25BC, so an expanded folder used to be marked
            // with an empty box. `▶` for a closed one does render. Found by
            // screenshot while checking the Actions panel, not by a test —
            // a missing glyph is invisible to every assertion we can write.
            if ui.small_button(if open { "⏷" } else { "▶" }).clicked() {
                state.toggle(&folder);
            }
            // "F", not a folder emoji: egui's bundled fonts have no glyph for
            // one, and it renders as an empty box. The Layers panel marks its
            // folders the same way.
            ui.label(RichText::new("F").small().weak())
                .on_hover_text("Folder");
            if ui.selectable_label(selected, &leaf).clicked() {
                // Selecting a folder deselects the symbol, so "new symbol"
                // has one unambiguous destination.
                state.selected_folder = Some(folder.clone());
                state.selected = None;
            }
        });

        if open {
            draw_folder_contents(ui, scene, state, Some(&folder), depth + 1, usage, command);
        }
    }

    let symbols: Vec<(SymbolId, String, SymbolKind)> = scene
        .library()
        .symbols_in(parent)
        .into_iter()
        .map(|s| (s.id, s.name.clone(), s.kind))
        .collect();

    for (id, name, kind) in symbols {
        if !state.matches(&name) {
            continue;
        }
        draw_symbol_row(ui, scene, state, id, &name, kind, indent, usage, command);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "internal row painter, not an API"
)]
fn draw_symbol_row(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    id: SymbolId,
    name: &str,
    kind: SymbolKind,
    indent: f32,
    usage: &std::collections::BTreeMap<SymbolId, usize>,
    command: &mut Option<Command>,
) {
    let uses = usage.get(&id).copied().unwrap_or(0);

    ui.horizontal(|ui| {
        ui.add_space(indent + 18.0);

        // A one-letter kind marker, as Animate's icon column does.
        let mark = match kind {
            SymbolKind::Graphic => "G",
            SymbolKind::MovieClip => "M",
            SymbolKind::Button => "B",
        };
        ui.label(RichText::new(mark).small().weak())
            .on_hover_text(kind.label());

        // A rename in progress replaces the label with a field.
        if let Some((renaming, buffer)) = &mut state.renaming
            && *renaming == id
        {
            let response = ui.text_edit_singleline(buffer);
            let commit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if commit {
                let new_name = buffer.trim().to_string();
                if !new_name.is_empty() && new_name != name {
                    // Uniqueness is the library's rule, not the panel's, so
                    // two symbols can never end up sharing a name.
                    let unique = scene.library().unique_name(&new_name);
                    scene.library_mut().update(id, |s| s.name = unique);
                }
                state.renaming = None;
            } else if response.lost_focus() {
                state.renaming = None;
            } else {
                response.request_focus();
            }
            return;
        }

        // Truncated rather than allowed to run on: a symbol name has no length
        // limit, and the use count on the right of this row is the one thing
        // that must not be pushed off the panel by one.
        let label = ui.add(
            egui::Button::selectable(state.selected == Some(id), name)
                .truncate()
                .min_size(egui::vec2(
                    // Room for the use count and its spacing on the right.
                    (ui.available_width() - 26.0).max(1.0),
                    0.0,
                )),
        );
        if label.clicked() {
            state.selected = Some(id);
            state.selected_folder = None;
        }
        if label.double_clicked() {
            // Double-click opens the symbol, exactly as in Animate.
            state.selected = Some(id);
            *command = Some(Command::EditSymbol);
        }
        label.context_menu(|ui| {
            for c in [
                Command::EditSymbol,
                Command::PlaceInstance,
                Command::DuplicateSymbol,
                Command::DeleteSymbol,
            ] {
                if ui.button(c.label()).clicked() {
                    state.selected = Some(id);
                    *command = Some(c);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Rename").clicked() {
                state.renaming = Some((id, name.to_string()));
                ui.close();
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Zero uses is worth noticing — it is the safe-to-delete signal.
            let text = RichText::new(format!("{uses}")).small();
            let text = if uses == 0 { text.weak() } else { text };
            ui.label(text).on_hover_text(if uses == 0 {
                "Not used anywhere".to_string()
            } else {
                format!("Used {uses} time(s), including inside other symbols")
            });
        });
    });
}

/// The action strip and the "move to folder" control.
fn draw_footer(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    usage: &std::collections::BTreeMap<SymbolId, usize>,
    command: &mut Option<Command>,
) {
    let selected = state.selected;

    // **Wrapped, not one long row.**
    //
    // Seven controls end to end need something over 250 points, and a dock
    // column can legitimately be narrower than that. Unwrapped, the ones at the
    // end — Place, Duplicate, Delete — were simply drawn off the edge of the
    // panel, which is the "the Library is hidden" report: not the panel, the
    // half of it that had nowhere to go. Wrapping puts them on a second line
    // instead, and costs nothing in a column wide enough for one.
    ui.horizontal_wrapped(|ui| {
        // The kind the next new symbol gets, in place of Animate's dialog.
        egui::ComboBox::from_id_salt("library_new_kind")
            .selected_text(RichText::new(state.new_symbol_kind.label()).small())
            .width(72.0)
            .show_ui(ui, |ui| {
                for kind in [
                    SymbolKind::Graphic,
                    SymbolKind::MovieClip,
                    SymbolKind::Button,
                ] {
                    ui.selectable_value(&mut state.new_symbol_kind, kind, kind.label());
                }
            });
        if ui
            .small_button("➕")
            .on_hover_text(Command::NewSymbol.label())
            .clicked()
        {
            *command = Some(Command::NewSymbol);
        }
        if ui
            .small_button("Fld")
            .on_hover_text(Command::NewLibraryFolder.label())
            .clicked()
        {
            *command = Some(Command::NewLibraryFolder);
        }
        ui.separator();
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Place").small())
            .on_hover_text("Place an instance on the stage")
            .clicked()
        {
            *command = Some(Command::PlaceInstance);
        }
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Dup").small())
            .on_hover_text(Command::DuplicateSymbol.label())
            .clicked()
        {
            *command = Some(Command::DuplicateSymbol);
        }
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("🗑").small())
            .on_hover_text(Command::DeleteSymbol.label())
            .clicked()
        {
            *command = Some(Command::DeleteSymbol);
        }
    });

    let Some(id) = selected else { return };
    let Some(symbol) = scene.library().get(id) else {
        return;
    };
    let (current, kind) = (symbol.folder.clone(), symbol.kind);
    let uses = usage.get(&id).copied().unwrap_or(0);

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Folder").small().weak());

        // The destination list is every folder plus the root, which is what
        // "organised in folders" needs in order to be usable without drag and
        // drop.
        let shown = current.clone().unwrap_or_else(|| "(root)".to_string());
        let mut target: Option<Option<String>> = None;

        egui::ComboBox::from_id_salt("library_folder")
            .selected_text(RichText::new(shown).small())
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "(root)").clicked() {
                    target = Some(None);
                }
                for folder in scene.library().folders().cloned().collect::<Vec<_>>() {
                    let is_current = current.as_deref() == Some(folder.as_str());
                    if ui.selectable_label(is_current, &folder).clicked() {
                        target = Some(Some(folder));
                    }
                }
            });

        if let Some(destination) = target {
            scene
                .library_mut()
                .move_to_folder(id, destination.as_deref());
        }

        ui.label(
            RichText::new(format!("{} · {uses} use(s)", kind.label()))
                .small()
                .weak(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::SymbolKind;

    fn library_scene() -> Scene {
        let mut scene = Scene::empty();
        scene.add_symbol("Hero Body", SymbolKind::Graphic, Some("Characters"));
        scene.add_symbol("Hero Arm", SymbolKind::Graphic, Some("Characters/Hero"));
        scene.add_symbol("Loop", SymbolKind::MovieClip, None);
        scene.library_mut().add_folder("Empty");
        scene
    }

    #[test]
    fn search_matches_case_insensitively_anywhere_in_the_name() {
        let state = LibraryState {
            search: "hero".to_string(),
            ..Default::default()
        };
        assert!(state.matches("Hero Body"));
        assert!(state.matches("SUPERHERO"));
        assert!(!state.matches("Loop"));
    }

    #[test]
    fn an_empty_search_matches_everything() {
        let state = LibraryState::default();
        assert!(state.matches("anything at all"));

        let state = LibraryState {
            search: "   ".to_string(),
            ..Default::default()
        };
        assert!(!state.is_searching(), "whitespace is not a search");
        assert!(state.matches("anything at all"));
    }

    #[test]
    fn expanding_a_folder_toggles_it() {
        let mut state = LibraryState::default();
        assert!(!state.is_expanded("Characters"));
        state.toggle("Characters");
        assert!(state.is_expanded("Characters"));
        state.toggle("Characters");
        assert!(!state.is_expanded("Characters"));
    }

    /// A search has to reach symbols nested several folders deep, so it opens
    /// every folder — including ones with nothing in them.
    #[test]
    fn searching_expands_every_folder_including_intermediates() {
        let scene = library_scene();
        let mut state = LibraryState::default();
        state.expand_all(&scene);

        for folder in ["Characters", "Characters/Hero", "Empty"] {
            assert!(state.is_expanded(folder), "{folder} should be open");
        }
    }

    /// The tree the panel walks must reach every symbol exactly once, or a
    /// symbol would be invisible in the panel while still being in the file.
    #[test]
    fn every_symbol_is_reachable_by_walking_the_folder_tree() {
        let scene = library_scene();

        fn walk(scene: &Scene, parent: Option<&str>, found: &mut Vec<SymbolId>) {
            for folder in scene.library().child_folders(parent) {
                walk(scene, Some(&folder), found);
            }
            found.extend(scene.library().symbols_in(parent).iter().map(|s| s.id));
        }

        let mut found = Vec::new();
        walk(&scene, None, &mut found);
        found.sort();

        let mut all: Vec<SymbolId> = scene.library().iter().map(|s| s.id).collect();
        all.sort();

        assert_eq!(found, all, "the tree walk must reach every symbol once");
    }
}
