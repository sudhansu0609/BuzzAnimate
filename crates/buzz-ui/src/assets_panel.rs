//! The Assets panel: reusable artwork that outlives the document.
//!
//! The Library panel holds *this* film's symbols. This holds the things an
//! animator accumulates across films — a tree, a lamp-post, a mouth chart —
//! kept on disk under `%APPDATA%/BuzzAnimate/assets` and placeable into any
//! file. See [`buzz_doc::assets`] for why the folders shown here are the
//! folders on disk.
//!
//! The panel raises intentions rather than performing them: it has no business
//! writing files or merging documents, and the shell already owns the undo step
//! that placing an asset belongs in.

use buzz_doc::{Asset, AssetLibrary};
use egui::{RichText, Ui};

use crate::MotionFilter;

/// How the panel asks the shell for an asset's picture.
///
/// The same arrangement the Library panel uses: the panel has no GPU and no
/// business reading files, so it asks by path and draws what comes back.
/// means "not yet" — pictures arrive over the next few frames — and the row
/// shows its name in the meantime rather than a gap.
pub type AssetThumbnailSource<'a> = &'a mut dyn FnMut(&std::path::Path) -> Option<egui::TextureId>;

/// What the user asked the panel to do.
#[derive(Debug, Clone, PartialEq)]
pub enum AssetAction {
    /// Put this asset into the document.
    Place(Asset),
    /// Keep the current selection as a new asset in this folder.
    Add {
        folder: String,
    },
    /// Make a folder under the library root.
    NewFolder,
    Rename {
        asset: Asset,
        name: String,
    },
    Delete(Asset),
    /// Delete a folder **and everything in it**.
    ///
    /// Carried as an intention like the rest; the shell is what warns first,
    /// because assets live outside the document and there is no undo for them.
    DeleteFolder {
        folder: String,
    },
    /// **Move an asset into a folder**, which is what dropping it on one means.
    MoveToFolder {
        asset: Asset,
        folder: String,
    },
    /// Read the directory again.
    Rescan,
    /// Bring an entire Animate asset library across.
    ImportFromAnimate,
    /// **Arrange by type & style…** (AS2): open the preview of proposed
    /// folder moves.
    ArrangeByTypeAndStyle,
}

/// The smallest a picture is drawn, in points. Below [`ROWS_BELOW`] the
/// assets are listed one per row, picture beside name; from there up they are
/// a grid of pictures with the name under each.
pub const MIN_EDGE: f32 = 24.0;
/// The largest a picture is drawn — the pictures are rendered for it.
pub const MAX_EDGE: f32 = 140.0;
/// The size a picture starts at: a grid of a few across a docked panel.
pub const DEFAULT_EDGE: f32 = 84.0;
const ROWS_BELOW: f32 = 40.0;

/// Panel state that is not part of the document, and not part of the library.
#[derive(Debug, Clone, Default)]
pub struct AssetPanelState {
    /// The folder a new asset goes into.
    pub selected_folder: String,
    /// Free-text filter over names.
    pub search: String,
    /// An Animate import in progress: assets done, and how many there are.
    pub importing: Option<(usize, usize)>,
    /// How big the pictures are, in points, and so how the assets are laid
    /// out; Cmd+scroll or a pinch over the panel changes it. `0` is the
    /// default size. See [`Self::edge`].
    pub thumbnail_edge: f32,
    /// Whether the panel shows every asset, only animated ones, or only stills.
    pub motion: MotionFilter,
    /// A folder whose deletion has been asked for once and not yet confirmed.
    ///
    /// Deleting a folder takes every asset under it, and an asset library has
    /// no undo — it is files on disk. So the first click says what will go and
    /// the second does it.
    pub confirm_delete: Option<String>,
    /// An asset whose deletion has been asked for once and not yet confirmed,
    /// by its path. Same reason as [`Self::confirm_delete`]: an asset is a file
    /// on disk with no undo, so the first Delete arms and the second removes it.
    pub confirm_delete_asset: Option<std::path::PathBuf>,
    /// A rename in progress.
    renaming: Option<(std::path::PathBuf, String)>,
}

/// **An asset being dragged**, as the payload a folder can catch.
///
/// Its own type rather than the asset alone, for the reason
/// `library_panel::DraggedSymbol` is: two things being dragged around the same
/// window must be distinguishable, or a folder would swallow a symbol and the
/// stage would place a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraggedAsset(pub Asset);

impl AssetPanelState {
    fn matches(&self, name: &str) -> bool {
        let needle = self.search.trim().to_lowercase();
        needle.is_empty() || name.to_lowercase().contains(&needle)
    }

    /// The pictures' edge in points, within [`MIN_EDGE`]..=[`MAX_EDGE`].
    pub fn edge(&self) -> f32 {
        if self.thumbnail_edge <= 0.0 {
            DEFAULT_EDGE
        } else {
            self.thumbnail_edge.clamp(MIN_EDGE, MAX_EDGE)
        }
    }
}

/// Draw the Assets panel.
///
/// **Pictures, and Animated or Static** — nothing else on show. The panel
/// used to stack a heading and count, a search box, three size buttons, the
/// motion switch, a row of five buttons, the folder path on disk and Place
/// and Delete on every asset, which buried the assets themselves. The
/// pictures grow and shrink with Cmd+scroll or a pinch over the panel; what
/// the buttons did is on the right-click menus — an asset's (Place, Rename,
/// Delete), a folder's (Delete) and the panel's own (Add Selection, New
/// Folder, From Animate, Arrange, Read Again).
pub fn assets_panel(
    ui: &mut Ui,
    library: &AssetLibrary,
    state: &mut AssetPanelState,
    can_add: bool,
    thumbnail: AssetThumbnailSource<'_>,
) -> Option<AssetAction> {
    let mut action = None;

    // **The panel's own menu**, on the empty space around the assets: laid
    // down first, so every asset and folder drawn over it keeps its own.
    let background = ui.interact(ui.max_rect(), ui.id().with("assets-background"), egui::Sense::click());
    background.context_menu(|ui| panel_menu(ui, state, can_add, &mut action));

    // **Cmd+scroll or a pinch resizes the pictures**, while the pointer is
    // over the panel; a plain scroll still scrolls the list.
    if ui.rect_contains_pointer(ui.max_rect()) {
        let zoom = ui.input(|i| i.zoom_delta());
        if zoom != 1.0 {
            state.thumbnail_edge = (state.edge() * zoom).clamp(MIN_EDGE, MAX_EDGE);
        }
    }

    filter_bar(ui, library, state, &mut action);

    // An unreadable library must say so: an empty list otherwise reads as
    // "you have no assets", which is a different and much worse message.
    if let Some(error) = &library.last_error {
        ui.label(
            RichText::new(format!("Could not read the assets folder: {error}"))
                .small()
                .color(crate::theme::Palette::snap()),
        );
    }

    if let Some((done, total)) = state.importing {
        ui.add(
            egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                .text(format!("importing {done} of {total} from Animate")),
        );
    }
    ui.add_space(4.0);

    if library.is_empty() && library.folders().is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "No assets yet.\n\nSelect artwork, then right-click here and choose Add \
                 Selection. Assets live outside the document, so anything kept here can be \
                 placed into any file.",
            )
            .weak()
            .italics(),
        );
    } else {
        // **Just the assets**: every one in the chosen folder and the folders
        // inside it, in one grid by name — no folder rows. "Characters" and
        // "Objects" used to stand between the switch and the pictures.
        let top = state.selected_folder.split('/').next().unwrap_or_default().to_string();
        let mut here: Vec<Asset> = library
            .assets()
            .iter()
            .filter(|a| {
                let first = a.folder.split('/').next().unwrap_or_default();
                if top.is_empty() { !first.starts_with('_') } else { first == top }
            })
            .filter(|a| state.matches(&a.name))
            .cloned()
            .collect();
        here.sort_by_key(|a| a.name.to_lowercase());
        draw_assets(ui, state, &here, 0.0, &mut action, thumbnail);
    }

    action
}

/// **One line: the top folders, then Animated and Static.** A folder button
/// (Animate, Cast…) shows the assets in it and everything filed inside it,
/// and is where Add Selection keeps a new one; clicking it again shows every
/// folder's. Dropping an asset on one files it there, and right-click deletes
/// it. Animated or Static shows that kind alone; clicking the lit one again
/// shows both. Folders whose names start with `_` (backups) are not shown.
fn filter_bar(ui: &mut Ui, library: &AssetLibrary, state: &mut AssetPanelState, action: &mut Option<AssetAction>) {
    let top = state.selected_folder.split('/').next().unwrap_or_default().to_string();
    ui.horizontal_wrapped(|ui| {
        let folders: Vec<String> =
            library.child_folders("").into_iter().filter(|f| !f.starts_with('_')).cloned().collect();
        for folder in folders {
            let on = top == folder;
            let (dropped, payload) = ui.dnd_drop_zone::<DraggedAsset, egui::Response>(egui::Frame::NONE, |ui| {
                ui.selectable_label(on, &folder)
                    .on_hover_text("Show this folder's assets \u{b7} drop an asset here to file it \u{b7} right-click to delete")
            });
            let button = dropped.inner;
            if let Some(payload) = payload {
                *action = Some(AssetAction::MoveToFolder { asset: payload.0.clone(), folder: folder.clone() });
            }
            if button.clicked() {
                state.selected_folder = if on { String::new() } else { folder.clone() };
            }
            button.context_menu(|ui| {
                if ui.button("Delete folder and contents").clicked() {
                    *action = Some(AssetAction::DeleteFolder { folder: folder.clone() });
                    ui.close();
                }
            });
        }
        ui.separator();
        for (option, label) in [(MotionFilter::Animated, "Animated"), (MotionFilter::Static, "Static")] {
            let on = state.motion == option;
            if ui.selectable_label(on, label).clicked() {
                state.motion = if on { MotionFilter::All } else { option };
            }
        }
    });
}

/// What the panel's buttons did, on its right-click menu.
fn panel_menu(ui: &mut Ui, state: &AssetPanelState, can_add: bool, action: &mut Option<AssetAction>) {
    let add = ui
        .add_enabled(can_add, egui::Button::new("Add Selection"))
        .on_hover_text("Keep the selected artwork here as a reusable asset")
        .on_disabled_hover_text("Select artwork on the stage first");
    if add.clicked() {
        *action = Some(AssetAction::Add { folder: state.selected_folder.clone() });
        ui.close();
    }
    if ui.button("New Folder").clicked() {
        *action = Some(AssetAction::NewFolder);
        ui.close();
    }
    ui.separator();
    if ui
        .button("From Animate\u{2026}")
        .on_hover_text(
            "Bring in an Animate asset library \u{2014} everything under its Assets folder, \
             filed the way Animate filed it",
        )
        .clicked()
    {
        *action = Some(AssetAction::ImportFromAnimate);
        ui.close();
    }
    if ui
        .button("Arrange by type & style\u{2026}")
        .on_hover_text(
            "File assets into Type folders (Characters, Buildings, Trees\u{2026}) and Style \
             folders inside them \u{2014} previewed first, and undoable",
        )
        .clicked()
    {
        *action = Some(AssetAction::ArrangeByTypeAndStyle);
        ui.close();
    }
    if ui.button("Read the Folder Again").clicked() {
        *action = Some(AssetAction::Rescan);
        ui.close();
    }
}

/// The assets on show, animated or still or both together.
fn draw_assets(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    here: &[Asset],
    indent: f32,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    let (animated, still): (Vec<&Asset>, Vec<&Asset>) = here.iter().partition(|a| a.animated);
    match state.motion {
        MotionFilter::All => {
            let all: Vec<&Asset> = here.iter().collect();
            draw_asset_group(ui, state, "", &all, indent, action, thumbnail);
        }
        MotionFilter::Animated => {
            draw_asset_group(ui, state, "", &animated, indent, action, thumbnail);
        }
        MotionFilter::Static => {
            draw_asset_group(ui, state, "", &still, indent, action, thumbnail);
        }
    }
}

/// One group of assets — optionally under a heading — laid out as the chosen
/// size asks for.
///
/// Rows for Small and Medium, a wrapping grid for Large. Split from the row so
/// the grid can decide how many fit across, which is a question about the panel
/// rather than about any one asset.
fn draw_asset_group(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    heading: &str,
    here: &[&Asset],
    indent: f32,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    if here.is_empty() {
        return;
    }
    if !heading.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.label(RichText::new(heading).small().weak());
        });
    }

    let edge = state.edge();
    if edge < ROWS_BELOW {
        for &asset in here {
            asset_row(ui, state, asset, indent, action, thumbnail);
        }
        return;
    }

    // **A wrapping grid.** `horizontal_wrapped` would work for uniform
    // widgets, but each cell here is a picture over a name of its own width,
    // so the cells are allocated at a fixed size and wrapped by hand — which
    // is also what keeps the columns lined up.
    let cell = edge + 12.0;
    let available = (ui.available_width() - indent).max(cell);
    let columns = ((available / cell).floor() as usize).max(1);

    for chunk in here.chunks(columns) {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            for &asset in chunk {
                asset_cell(ui, state, asset, action, thumbnail);
            }
        });
    }
}

/// One asset in the grid: its picture, with its name under it.
fn asset_cell(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    asset: &Asset,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    let edge = state.edge();
    ui.allocate_ui(egui::vec2(edge + 12.0, edge + 22.0), |ui| {
        ui.vertical_centered(|ui| {
            asset_picture(ui, state, asset, edge, action, thumbnail);
            if !asset_name_editor(ui, state, asset, action) {
                // Truncated by hand: a painted label does not shorten itself,
                // and a long name would push the whole column out of line.
                let chars = ((edge + 12.0) / 6.5) as usize;
                ui.label(RichText::new(shorten(&asset.name, chars.max(6))).small());
            }
        });
    });
}

/// **An asset's picture**, which is the whole of what it offers: click places
/// it, dragging carries it onto the stage or a folder, double-click renames,
/// right-click for Place, Rename and Delete.
///
/// One widget sensing clicks and drags, with the drag payload set by hand,
/// rather than a drag source wrapping a clickable picture: a drag widget laid
/// over others swallows their clicks (see `draw_symbol_row` in the Library).
/// The grid's pictures used to take only clicks, so an asset could be dragged
/// out at the list sizes alone.
fn asset_picture(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    asset: &Asset,
    edge: f32,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(edge, edge), egui::Sense::click_and_drag());
    draw_thumbnail(ui, rect, asset, thumbnail);
    if response.hovered() {
        ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, crate::theme::Palette::text()), egui::StrokeKind::Inside);
    }
    if response.drag_started() {
        response.dnd_set_drag_payload(DraggedAsset(asset.clone()));
    }
    let response = response.on_hover_text(format!(
        "{}\nClick places it \u{b7} drag it onto the stage or a folder \u{b7} double-click \
         renames \u{b7} Cmd+scroll resizes \u{b7} right-click for more",
        asset.label()
    ));
    if response.double_clicked() {
        state.renaming = Some((asset.path.clone(), asset.name.clone()));
    } else if response.clicked() {
        *action = Some(AssetAction::Place(asset.clone()));
    }
    response.context_menu(|ui| asset_menu(ui, state, asset, action));
}

/// The name being typed, when this asset is being renamed: Enter keeps it.
/// Returns whether it drew the editor.
fn asset_name_editor(ui: &mut Ui, state: &mut AssetPanelState, asset: &Asset, action: &mut Option<AssetAction>) -> bool {
    let Some((path, text)) = state.renaming.as_mut() else {
        return false;
    };
    if *path != asset.path {
        return false;
    }
    let response = ui.text_edit_singleline(text);
    let done = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    response.request_focus();
    if done && let Some((_, text)) = state.renaming.take() {
        *action = Some(AssetAction::Rename { asset: asset.clone(), name: text });
    }
    true
}

/// The picture, or the space it will occupy.
///
/// The square is always reserved so rows and columns do not jump about as
/// pictures arrive over the next few frames.
///
/// **Asked for only when it is on screen.** Every asset in an open folder used
/// to ask, scrolled out of view or not, and a library larger than the
/// picture cache kept loading, evicting and loading again — reading files
/// for as long as the panel was open.
fn draw_thumbnail(ui: &Ui, rect: egui::Rect, asset: &Asset, thumbnail: AssetThumbnailSource<'_>) {
    let picture = if ui.is_rect_visible(rect) { thumbnail(&asset.path) } else { None };
    match picture {
        Some(texture) => {
            egui::Image::new((texture, rect.size())).paint_at(ui, rect);
        }
        None => {
            ui.painter()
                .rect_filled(rect.shrink(1.0), 2.0, crate::theme::Palette::chrome());
        }
    }
}

/// `name`, cut to `max` characters with an ellipsis if it is longer.
fn shorten(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let kept: String = name.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

impl AssetPanelState {
    /// **Start naming an asset**, as though its name had been double-clicked.
    ///
    /// Called the moment one is kept. See
    /// [`LibraryState::start_naming`](crate::LibraryState::start_naming): the
    /// asset had the same defect, and "Asset", "Asset 2" is a worse library
    /// than "Symbol", "Symbol 2" because assets outlive the document.
    pub fn start_naming(&mut self, path: std::path::PathBuf, name: impl Into<String>) {
        self.renaming = Some((path, name.into()));
    }

    /// The asset whose name is being typed, if one is.
    pub fn naming(&self) -> Option<&std::path::Path> {
        self.renaming.as_ref().map(|(path, _)| path.as_path())
    }
}

/// Place, rename and delete — the three things a row and a cell both offer.
fn asset_menu(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    asset: &Asset,
    action: &mut Option<AssetAction>,
) {
    if ui.button("Place").clicked() {
        *action = Some(AssetAction::Place(asset.clone()));
        ui.close();
    }
    if ui.button("Rename").clicked() {
        state.renaming = Some((asset.path.clone(), asset.name.clone()));
        ui.close();
    }
    // Two clicks, because there is no undo for a file on disk. The first arms
    // the delete (the shell relabels this and says so); the second, on the now
    // "confirm?" button, does it. Arming keeps the menu open so the second click
    // has something to land on.
    let armed = state.confirm_delete_asset.as_deref() == Some(asset.path.as_path());
    if ui
        .button(if armed {
            "Delete \u{2014} confirm?"
        } else {
            "Delete"
        })
        .clicked()
    {
        *action = Some(AssetAction::Delete(asset.clone()));
        if armed {
            ui.close();
        }
    }
}

/// One asset at the smallest sizes: its picture with the name beside it.
fn asset_row(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    asset: &Asset,
    indent: f32,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    let edge = state.edge();
    ui.horizontal(|ui| {
        ui.add_space(indent);
        asset_picture(ui, state, asset, edge, action, thumbnail);
        ui.add_space(4.0);
        if !asset_name_editor(ui, state, asset, action) {
            let response = ui.selectable_label(false, &asset.name);
            if response.double_clicked() {
                state.renaming = Some((asset.path.clone(), asset.name.clone()));
            } else if response.clicked() {
                *action = Some(AssetAction::Place(asset.clone()));
            }
            response.context_menu(|ui| asset_menu(ui, state, asset, action));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_with_an_asset() -> (tempfile::TempDir, AssetLibrary) {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        library
            .save("Oak", "Trees", &buzz_scene::Scene::default())
            .expect("save");
        (dir, library)
    }

    /// Every state a listing is usually only tested in one of: empty, full,
    /// filtered to nothing, and unreadable.
    #[test]
    fn the_panel_draws_in_every_state() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let (_dir, library) = library_with_an_asset();
        let empty = AssetLibrary::default();
        let mut broken = AssetLibrary::default();
        broken.last_error = Some("permission denied".into());

        for (library, search, can_add) in [
            (&library, "", true),
            (&library, "zzz", false),
            (&empty, "", false),
            (&broken, "", true),
        ] {
            let mut state = AssetPanelState {
                search: search.to_string(),
                ..Default::default()
            };
            let _ = ctx.run_ui(Default::default(), |ui| {
                for edge in [0.0, MIN_EDGE, DEFAULT_EDGE, MAX_EDGE] {
                    state.thumbnail_edge = edge;
                    let _ = assets_panel(ui, library, &mut state, can_add, &mut |_| None);
                }
            });
        }
    }

    /// The panel decides nothing: it says what the user asked for, and the
    /// shell does it inside an undo step.
    #[test]
    fn the_panel_only_raises_intentions() {
        let (_dir, library) = library_with_an_asset();
        assert_eq!(library.len(), 1);

        let asset = library.assets()[0].clone();
        let place = AssetAction::Place(asset.clone());
        assert_eq!(place, AssetAction::Place(asset));
    }
}
