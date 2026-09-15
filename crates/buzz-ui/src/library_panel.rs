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
use crate::theme::Palette;

/// Edge of a symbol's picture in the list, in points.
///
/// Matches the row height a symbol already had, so adding pictures did not
/// make the library taller per entry.
const THUMBNAIL: f32 = 20.0;

/// Room kept on the right of a symbol row for its use count.
const USE_COUNT: f32 = 26.0;

/// A symbol being dragged out of the Library, on its way to the stage.
///
/// A newtype rather than a bare `SymbolId` because egui keys a drag payload by
/// its *type*: anything else that ever wants to be dragged must be
/// distinguishable from this, or the stage would place a symbol when a swatch
/// was dropped on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraggedSymbol(pub SymbolId);

/// Which items a panel is showing: everything, only what moves, or only stills.
///
/// The same three-way choice serves the Library and the Assets panels — an
/// animated symbol and an animated asset are one idea at two scales — so it
/// lives in one place and both toggles drive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionFilter {
    /// Animated and static together, each under its own heading.
    #[default]
    All,
    /// Only what moves.
    Animated,
    /// Only stills.
    Static,
}

impl MotionFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Animated, Self::Static];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Animated => "Animated",
            Self::Static => "Static",
        }
    }

    /// Does an item with this motion pass the filter?
    pub fn accepts(self, animated: bool) -> bool {
        match self {
            Self::All => true,
            Self::Animated => animated,
            Self::Static => !animated,
        }
    }

    /// Draw the three-way toggle, updating `current`. Shared by both panels so
    /// they read and behave the same.
    pub fn toggle(ui: &mut Ui, current: &mut Self) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Show").small().weak());
            for option in Self::ALL {
                if ui
                    .selectable_label(*current == option, option.label())
                    .clicked()
                {
                    *current = option;
                }
            }
        });
    }
}

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
    /// Whether the list shows every symbol, only animated ones, or only stills.
    pub motion: MotionFilter,
    /// Folder paths the user has opened.
    expanded: BTreeSet<String>,
    /// Rename buffer, live only while a rename is in progress.
    renaming: Option<(SymbolId, String)>,
    /// The search string, lower-cased, recomputed only when `search` changes.
    ///
    /// `matches` is called once per symbol; lower-casing the needle inside it
    /// allocated a `String` per symbol per frame, which on a large library is a
    /// per-frame cost that scales with the whole library. Cached here instead.
    search_lower: String,
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

    /// Recompute the cached lower-cased needle. Called once at the top of a
    /// frame — one allocation a frame, rather than one per symbol in `matches`.
    fn sync_search(&mut self) {
        self.search_lower = self.search.trim().to_lowercase();
    }

    fn matches(&self, name: &str) -> bool {
        self.search_lower.is_empty() || name.to_lowercase().contains(&self.search_lower)
    }
}

/// Looks up the picture for a symbol, and remembers that it was asked for.
///
/// A closure rather than a type, because the pictures live on the GPU and this
/// crate has no device — the shell owns them and hands in a way to ask.
pub type ThumbnailSource<'a> = &'a mut dyn FnMut(SymbolId) -> Option<egui::TextureId>;

/// Draw the Library panel.
///
impl LibraryState {
    /// **Start naming a symbol**, as though its name had been double-clicked.
    ///
    /// Called the moment one is made. Convert to Symbol used to name it
    /// "Symbol" and leave it at that: Animate asks for the name as part of the
    /// gesture, and here there was nowhere to say it — the artwork became
    /// "Symbol", then "Symbol 2", and a library of them had to be renamed
    /// afterwards one at a time, if you remembered which was which.
    ///
    /// A field with the name in it, focused, is the same offer without a modal
    /// in the way: type over it, or press Enter and keep the default.
    pub fn start_naming(&mut self, id: SymbolId, name: impl Into<String>) {
        self.renaming = Some((id, name.into()));
    }

    /// The symbol whose name is being typed, if one is. For tests and for
    /// anything that needs to know a field has the keyboard.
    pub fn naming(&self) -> Option<SymbolId> {
        self.renaming.as_ref().map(|(id, _)| *id)
    }
}

/// Takes the scene mutably because renaming a symbol and moving it between
/// folders are edits in their own right; the caller wraps the call in an undo
/// step, and a frame where nothing changed records nothing.
pub fn library_panel(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut LibraryState,
    // Use counts per symbol. Computed off the UI thread by the shell and passed
    // in, because walking every object in a large document to count instances
    // was a per-frame cost that scaled with the whole file — see the caller.
    usage: &std::collections::BTreeMap<SymbolId, usize>,
    thumbnail: ThumbnailSource<'_>,
) -> Option<Command> {
    let mut command = None;
    // Lower-case the search needle once, not once per symbol in `matches`.
    state.sync_search();

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

    // Animated or still. On "All" the two are shown apart, under their own
    // headings; the other two narrow the list to one or the other.
    MotionFilter::toggle(ui, &mut state.motion);
    ui.separator();

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
    if scene.library().is_empty() {
        egui::ScrollArea::vertical()
            .id_salt("library-items")
            .auto_shrink([false, true])
            .max_height(300.0)
            .show(ui, |ui| {
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
            });
    } else {
        // **Virtualized.** The tree is flattened to a list and only the rows the
        // scroll area can show are turned into widgets, so a ten-thousand-symbol
        // library costs a screenful of rows a frame, not ten thousand.
        let rows = flatten_rows(scene, state);
        let row_height = THUMBNAIL + 2.0;
        egui::ScrollArea::vertical()
            .id_salt("library-items")
            .auto_shrink([false, true])
            .max_height(300.0)
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    // Each row is built inside a fixed-height slot so the
                    // virtualized layout stays aligned with its scrollbar.
                    let (slot, _) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), egui::Sense::hover());
                    let mut row_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(slot)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    match &rows[i] {
                        Row::Folder { path, leaf, depth } => {
                            draw_folder_row(&mut row_ui, state, path, leaf, *depth);
                        }
                        Row::Group { label, depth } => {
                            draw_group_row(&mut row_ui, label, *depth);
                        }
                        Row::Symbol {
                            id,
                            name,
                            kind,
                            depth,
                        } => {
                            let indent = *depth as f32 * 14.0;
                            draw_symbol_row(
                                &mut row_ui, scene, state, *id, name, *kind, indent, usage,
                                thumbnail, &mut command,
                            );
                        }
                    }
                }
            });
    }

    ui.separator();
    draw_footer(ui, scene, state, usage, &mut command);

    command
}

/// One level of the tree: the folders directly inside `parent`, then the
/// symbols that sit in `parent` itself.
///
/// Folders come first because that is where Animate puts them, and because a
/// long symbol list would otherwise push the folders off the top.
/// One line of the flattened Library tree.
///
/// The tree — folders, expanded into their contents, and the symbols at each
/// level — is flattened into this list once per frame, cheaply (no widgets),
/// and then only the rows the scroll area can actually show are built into
/// widgets. Without that, a flat library of ten thousand symbols built ten
/// thousand rows a frame, which was tens of milliseconds of pure layout and a
/// hang in its own right.
enum Row {
    Folder {
        path: String,
        leaf: String,
        depth: usize,
    },
    /// An "Animated" or "Static" heading over the symbols that follow, shown
    /// only when the filter is [`MotionFilter::All`] and that group has members.
    Group {
        label: &'static str,
        depth: usize,
    },
    Symbol {
        id: SymbolId,
        name: String,
        kind: SymbolKind,
        depth: usize,
    },
}

/// Flatten the visible tree — expanded folders and matching symbols — into a
/// list of rows, in display order. Reads only; the widgets are built later.
fn flatten_rows(scene: &Scene, state: &LibraryState) -> Vec<Row> {
    fn walk(scene: &Scene, state: &LibraryState, parent: Option<&str>, depth: usize, rows: &mut Vec<Row>) {
        for folder in scene.library().child_folders(parent) {
            let leaf = folder.rsplit('/').next().unwrap_or(&folder).to_string();
            rows.push(Row::Folder {
                path: folder.clone(),
                leaf,
                depth,
            });
            if state.is_expanded(&folder) {
                walk(scene, state, Some(&folder), depth + 1, rows);
            }
        }
        // Split this level's symbols into moving and still, keeping only what
        // matches the search and the motion filter.
        let mut animated = Vec::new();
        let mut still = Vec::new();
        for symbol in scene.library().symbols_in(parent) {
            if !state.matches(&symbol.name) || !state.motion.accepts(symbol.is_animated()) {
                continue;
            }
            if symbol.is_animated() {
                &mut animated
            } else {
                &mut still
            }
            .push(symbol);
        }

        let mut emit = |label: &'static str, symbols: &[&std::sync::Arc<buzz_scene::Symbol>]| {
            if symbols.is_empty() {
                return;
            }
            // A heading only earns its row when both groups are on show; asked
            // for one alone, the list is already all of one kind.
            if state.motion == MotionFilter::All {
                rows.push(Row::Group { label, depth });
            }
            for symbol in symbols {
                rows.push(Row::Symbol {
                    id: symbol.id,
                    name: symbol.name.clone(),
                    kind: symbol.kind,
                    depth,
                });
            }
        };
        emit("Animated", &animated);
        emit("Static", &still);
    }
    let mut rows = Vec::new();
    walk(scene, state, None, 0, &mut rows);
    rows
}

/// Draw an "Animated" / "Static" heading over the symbols beneath it.
fn draw_group_row(ui: &mut Ui, label: &str, depth: usize) {
    let indent = depth as f32 * 14.0;
    ui.horizontal(|ui| {
        ui.add_space(indent + 18.0);
        ui.label(RichText::new(label).small().weak());
    });
}

/// Draw one folder row of the flattened tree.
fn draw_folder_row(ui: &mut Ui, state: &mut LibraryState, path: &str, leaf: &str, depth: usize) {
    let indent = depth as f32 * 14.0;
    let open = state.is_expanded(path);
    let selected = state.selected_folder.as_deref() == Some(path);

    ui.horizontal(|ui| {
        ui.add_space(indent);
        // `⏷` rather than `▼`: the bundled fonts have a glyph for U+23F7 and
        // none for U+25BC, so an expanded folder used to be marked with an empty
        // box. `▶` for a closed one does render.
        if ui.small_button(if open { "⏷" } else { "▶" }).clicked() {
            state.toggle(path);
        }
        ui.label(RichText::new("F").small().weak())
            .on_hover_text("Folder");
        if ui.selectable_label(selected, leaf).clicked() {
            state.selected_folder = Some(path.to_string());
            state.selected = None;
        }
    });
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
    thumbnail: ThumbnailSource<'_>,
    command: &mut Option<Command>,
) {
    let uses = usage.get(&id).copied().unwrap_or(0);

    // **The whole row is a drag source.**
    //
    // Placing a symbol was: select it, find the Place button, press it, then
    // hunt for where the artwork landed and drag it there. Dragging the row
    // onto the stage is the same intent with the three middle steps removed,
    // and it is what every library in every drawing program does. The stage
    // picks the payload up; see `App::handle_stage_input`.
    let drag_id = ui.id().with(("library-drag", id.0));
    ui.horizontal(|ui| {
        ui.add_space(indent + 18.0);

        // **The picture, where the eye goes first.**
        //
        // A symbol identified only by its name means opening symbols to find
        // out what they are. The space is always claimed, whether or not the
        // picture has been drawn yet, so a library does not jiggle as its
        // thumbnails arrive over the next few frames.
        //
        // **And the picture is the handle**, not the whole row.
        //
        // `dnd_drag_source` interacts over everything it wraps, and a drag
        // widget laid over a row swallows the clicks meant for what is in it:
        // wrapping the row made its name unselectable, so nothing could ever
        // become the selected symbol — which is why Place, Duplicate and Delete
        // sat permanently greyed out and deleting a symbol looked impossible.
        // Measured: with the row wrapped, no click anywhere in the panel
        // selected a symbol; with only the thumbnail wrapped, every click on
        // the name does.
        //
        // Grabbing the picture is also what a library is expected to offer.
        let dragged = ui.dnd_drag_source(drag_id, DraggedSymbol(id), |ui| {
            let (slot, _) = ui.allocate_exact_size(
                egui::vec2(THUMBNAIL, THUMBNAIL),
                egui::Sense::hover(),
            );
            match thumbnail(id) {
                Some(texture) => {
                    egui::Image::new((texture, egui::vec2(THUMBNAIL, THUMBNAIL)))
                        .paint_at(ui, slot);
                }
                // Not drawn yet: a quiet frame, so the row reads as a row
                // rather than as a gap.
                None => {
                    ui.painter().rect_stroke(
                        slot,
                        2.0,
                        egui::Stroke::new(1.0, Palette::border()),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            slot
        });
        dragged
            .response
            .on_hover_text("Drag onto the stage to place an instance");

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
        // `add_sized`, not `min_size`: a minimum is only a floor, and
        // `truncate` still wraps against the *whole* remaining width, so the
        // button could grow into the room the use count needs. Sizing the
        // allocation is what actually bounds it.
        let room = (ui.available_width() - USE_COUNT).max(1.0);
        let label = ui.add_sized(
            egui::vec2(room, ui.spacing().interact_size.y),
            egui::Button::selectable(state.selected == Some(id), name).truncate(),
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
                Command::SymbolToAsset,
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
        // **Out of the document and onto the shelf.** A symbol could be kept as
        // an asset only by placing an instance of it, selecting that, keeping
        // it, and deleting the instance again. The Library is where symbols
        // are, so it is where "keep this one" is asked.
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Asset").small())
            .on_hover_text("Keep this symbol in the Assets library, to reuse in any document")
            .clicked()
        {
            *command = Some(Command::SymbolToAsset);
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
        let mut state = LibraryState {
            search: "hero".to_string(),
            ..Default::default()
        };
        // The needle is lower-cased once per frame; the panel calls this, so the
        // test does too.
        state.sync_search();
        assert!(state.matches("Hero Body"));
        assert!(state.matches("SUPERHERO"));
        assert!(!state.matches("Loop"));

        // An uppercase search still matches — the caching lower-cases both ends.
        state.search = "HERO".to_string();
        state.sync_search();
        assert!(state.matches("hero body"));
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

    /// On "All", symbols are split into an Animated group and a Static group,
    /// each under its own heading, so a browsing eye can tell the two apart.
    #[test]
    fn symbols_are_grouped_into_animated_and_static() {
        let scene = library_scene();
        let mut state = LibraryState::default();
        state.expand_all(&scene);

        let rows = flatten_rows(&scene, &state);

        // The movie clip sits at the root, under an "Animated" heading.
        let loop_id = scene.library().find_by_name("Loop").expect("the clip").id;
        let animated_over_loop = rows.windows(2).any(|w| {
            matches!(w[0], Row::Group { label: "Animated", .. })
                && matches!(&w[1], Row::Symbol { id, .. } if *id == loop_id)
        });
        assert!(animated_over_loop, "the movie clip is under an Animated heading");

        // The graphics are stills, so there is a Static heading too.
        assert!(
            rows.iter()
                .any(|r| matches!(r, Row::Group { label: "Static", .. })),
            "the graphics are under a Static heading"
        );
    }

    /// Asked for one kind, the list narrows to it and drops the headings — a
    /// list already all of one kind does not need to say so.
    #[test]
    fn the_motion_filter_narrows_the_list() {
        let scene = library_scene();
        let mut state = LibraryState {
            motion: MotionFilter::Animated,
            ..Default::default()
        };
        state.expand_all(&scene);

        let names = |rows: &[Row]| {
            let mut names: Vec<String> = rows
                .iter()
                .filter_map(|r| match r {
                    Row::Symbol { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect();
            names.sort();
            names
        };

        let rows = flatten_rows(&scene, &state);
        assert!(
            !rows.iter().any(|r| matches!(r, Row::Group { .. })),
            "no headings when only one kind is shown"
        );
        assert_eq!(names(&rows), ["Loop"], "only the movie clip is animated");

        state.motion = MotionFilter::Static;
        let rows = flatten_rows(&scene, &state);
        assert_eq!(names(&rows), ["Hero Arm", "Hero Body"], "the rest are stills");
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
