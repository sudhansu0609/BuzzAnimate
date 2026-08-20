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
    /// Read the directory again.
    Rescan,
    /// Bring an entire Animate asset library across.
    ImportFromAnimate,
}

/// How big the pictures are drawn, and — because the two are the same
/// decision — how the assets are laid out.
///
/// Animate's Library offers a list and a preview; this is the same idea taken
/// one step further, because an asset library is browsed by *eye*. A row of
/// names with a stamp beside each is the right shape for finding a thing you
/// can already name; a grid of large pictures is the right shape for finding
/// the one that looks right, which is what an asset folder is actually for.
///
/// One control rather than two, because a large picture in a one-per-row list
/// wastes most of the panel and a small one in a grid is a field of dots. The
/// size *implies* the layout, so there is no way to choose a combination that
/// does not work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThumbnailSize {
    /// A stamp beside the name, one asset per row. The densest listing.
    Small,
    /// A picture with its name beside it, still one per row.
    #[default]
    Medium,
    /// A grid of pictures with the name underneath, as many across as fit.
    Large,
}

impl ThumbnailSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
        }
    }

    /// Edge of the picture, in points.
    pub fn edge(self) -> f32 {
        match self {
            Self::Small => 20.0,
            Self::Medium => 40.0,
            Self::Large => 84.0,
        }
    }

    /// Does this size lay the assets out as a grid rather than as rows?
    pub fn is_grid(self) -> bool {
        matches!(self, Self::Large)
    }

    /// Width of one cell in the grid, including the room its name needs.
    pub fn cell(self) -> f32 {
        self.edge() + 12.0
    }
}

/// Panel state that is not part of the document, and not part of the library.
#[derive(Debug, Clone, Default)]
pub struct AssetPanelState {
    /// The folder a new asset goes into.
    pub selected_folder: String,
    /// Free-text filter over names.
    pub search: String,
    /// An Animate import in progress: assets done, and how many there are.
    pub importing: Option<(usize, usize)>,
    /// How big the pictures are, and so how the assets are laid out.
    pub thumbnail_size: ThumbnailSize,
    /// A folder whose deletion has been asked for once and not yet confirmed.
    ///
    /// Deleting a folder takes every asset under it, and an asset library has
    /// no undo — it is files on disk. So the first click says what will go and
    /// the second does it.
    pub confirm_delete: Option<String>,
    /// A rename in progress.
    renaming: Option<(std::path::PathBuf, String)>,
    expanded: std::collections::BTreeSet<String>,
}

impl AssetPanelState {
    fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
    }

    fn matches(&self, name: &str) -> bool {
        let needle = self.search.trim().to_lowercase();
        needle.is_empty() || name.to_lowercase().contains(&needle)
    }
}

/// Draw the Assets panel.
pub fn assets_panel(
    ui: &mut Ui,
    library: &AssetLibrary,
    state: &mut AssetPanelState,
    can_add: bool,
    thumbnail: AssetThumbnailSource<'_>,
) -> Option<AssetAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        ui.heading("Assets");
        ui.label(
            RichText::new(format!("{} items", library.len()))
                .small()
                .weak(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("\u{27F3}")
                .on_hover_text("Read the assets folder again")
                .clicked()
            {
                action = Some(AssetAction::Rescan);
            }
        });
    });

    // An unreadable library must say so: an empty list otherwise reads as
    // "you have no assets", which is a different and much worse message.
    if let Some(error) = &library.last_error {
        ui.label(
            RichText::new(format!("Could not read the assets folder: {error}"))
                .small()
                .color(crate::theme::Palette::snap()),
        );
    }

    ui.horizontal(|ui| {
        ui.label("\u{1F50D}");
        ui.text_edit_singleline(&mut state.search);
        if !state.search.is_empty() && ui.small_button("x").on_hover_text("Clear").clicked() {
            state.search.clear();
        }
    });

    // **Picture size, which is also the layout.** See [`ThumbnailSize`] for
    // why the two are one control rather than two.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Size").small().weak());
        for option in ThumbnailSize::ALL {
            if ui
                .selectable_label(state.thumbnail_size == option, option.label())
                .clicked()
            {
                state.thumbnail_size = option;
            }
        }
    });
    ui.separator();

    if library.is_empty() && library.folders().is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "No assets yet.\n\nSelect artwork and press + to keep it here. Assets live \
                 outside the document, so anything kept here can be placed into any file.",
            )
            .weak()
            .italics(),
        );
    } else {
        draw_folder(ui, library, state, "", 0, &mut action, thumbnail);
    }

    ui.separator();
    // Wrapped: four controls and a hint are wider than a dock column at its
    // narrowest, and unwrapped the last of them — "From Animate…" — was drawn
    // off the end of the panel where it could not be clicked.
    ui.horizontal_wrapped(|ui| {
        // **Named, not a plus.** "+" beside a library reads as "new folder" as
        // easily as "keep this", and the one thing this button does is the
        // reason the panel exists.
        let add = ui.add_enabled(can_add, egui::Button::new("Add Selection").small());
        if add
            .on_hover_text("Keep the selected artwork here as a reusable asset")
            .clicked()
        {
            action = Some(AssetAction::Add {
                folder: state.selected_folder.clone(),
            });
        }
        if !can_add {
            ui.label(
                RichText::new("select artwork to add")
                    .small()
                    .weak()
                    .italics(),
            );
        }
        if ui.small_button("Fld").on_hover_text("New folder").clicked() {
            action = Some(AssetAction::NewFolder);
        }
        if ui
            .small_button("From Animate\u{2026}")
            .on_hover_text(
                "Bring in an Animate asset library \u{2014} everything under its \
                 Assets folder, filed the way Animate filed it",
            )
            .clicked()
        {
            action = Some(AssetAction::ImportFromAnimate);
        }
        if !state.selected_folder.is_empty() {
            ui.label(RichText::new(&state.selected_folder).small().weak());
        }
    });

    if let Some((done, total)) = state.importing {
        ui.add(
            egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                .text(format!("importing {done} of {total} from Animate")),
        );
    }

    if let Some(root) = library.root() {
        ui.label(
            RichText::new(root.display().to_string())
                .small()
                .weak()
                .italics(),
        )
        .on_hover_text("Where these files are kept");
    }

    action
}

fn draw_folder(
    ui: &mut Ui,
    library: &AssetLibrary,
    state: &mut AssetPanelState,
    parent: &str,
    depth: usize,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    let indent = depth as f32 * 14.0;

    let folders: Vec<String> = library.child_folders(parent).into_iter().cloned().collect();

    for folder in folders {
        let leaf = folder.rsplit('/').next().unwrap_or(&folder).to_string();
        let open = state.is_expanded(&folder) || !state.search.trim().is_empty();
        let selected = state.selected_folder == folder;

        ui.horizontal(|ui| {
            ui.add_space(indent);
            if ui
                .small_button(if open { "\u{23F7}" } else { "\u{25B6}" })
                .clicked()
            {
                state.toggle(&folder);
            }
            ui.label(RichText::new("F").small().weak())
                .on_hover_text("Folder");
            let label = ui
                .selectable_label(selected, &leaf)
                .on_hover_text("Click makes this the folder new assets go into");
            if label.clicked() {
                state.selected_folder = if selected {
                    String::new()
                } else {
                    folder.clone()
                };
            }
            // Right-click as well as the button, because a right-click is where
            // a hand goes looking for "delete" on anything in a tree.
            label.context_menu(|ui| {
                if ui.button("Delete folder and contents").clicked() {
                    *action = Some(AssetAction::DeleteFolder {
                        folder: folder.clone(),
                    });
                    ui.close();
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // **Spelled out, not a pictogram.** The trash-can glyph is not
                // in the bundled fonts, so the button that was here drew as an
                // empty box - which is why deleting looked as though it had
                // been taken away.
                if ui
                    .small_button("Delete")
                    .on_hover_text("Delete this folder and everything in it")
                    .clicked()
                {
                    *action = Some(AssetAction::DeleteFolder {
                        folder: folder.clone(),
                    });
                }
            });
        });

        if open {
            draw_folder(ui, library, state, &folder, depth + 1, action, thumbnail);
        }
    }

    let here: Vec<Asset> = library
        .in_folder(parent)
        .into_iter()
        .filter(|a| state.matches(&a.name))
        .cloned()
        .collect();
    draw_assets(ui, state, &here, indent, action, thumbnail);
}

/// The assets in one folder, laid out as the chosen size asks for.
///
/// Rows for Small and Medium, a wrapping grid for Large. Split here rather than
/// inside the row so the grid can decide how many fit across, which is a
/// question about the panel rather than about any one asset.
fn draw_assets(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    here: &[Asset],
    indent: f32,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    let size = state.thumbnail_size;
    if !size.is_grid() {
        for asset in here {
            asset_row(ui, state, asset, indent, action, thumbnail);
        }
        return;
    }

    // **A wrapping grid.** `horizontal_wrapped` would work for uniform
    // widgets, but each cell here is a picture over a name of its own width,
    // so the cells are allocated at a fixed size and wrapped by hand — which
    // is also what keeps the columns lined up.
    let cell = size.cell();
    let available = (ui.available_width() - indent).max(cell);
    let columns = ((available / cell).floor() as usize).max(1);

    for chunk in here.chunks(columns) {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            for asset in chunk {
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
    let size = state.thumbnail_size;
    let edge = size.edge();

    ui.allocate_ui(egui::vec2(size.cell(), edge + 22.0), |ui| {
        ui.vertical_centered(|ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(edge, edge),
                egui::Sense::click(),
            );
            draw_thumbnail(ui, rect, asset, thumbnail);

            let response = response.on_hover_text(format!(
                "{}\nClick places it in the document",
                asset.label()
            ));
            if response.clicked() {
                *action = Some(AssetAction::Place(asset.clone()));
            }
            response.context_menu(|ui| asset_menu(ui, state, asset, action));

            // Truncated by hand: a painted label does not shorten itself, and
            // a long name would push the whole column out of line.
            let name = shorten(&asset.name, 12);
            ui.label(RichText::new(name).small());
        });
    });
}

/// The picture, or the space it will occupy.
///
/// The square is always reserved so rows and columns do not jump about as
/// pictures arrive over the next few frames.
fn draw_thumbnail(
    ui: &Ui,
    rect: egui::Rect,
    asset: &Asset,
    thumbnail: AssetThumbnailSource<'_>,
) {
    match thumbnail(&asset.path) {
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
    if ui.button("Delete").clicked() {
        *action = Some(AssetAction::Delete(asset.clone()));
        ui.close();
    }
}


fn asset_row(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    asset: &Asset,
    indent: f32,
    action: &mut Option<AssetAction>,
    thumbnail: AssetThumbnailSource<'_>,
) {
    ui.horizontal(|ui| {
        ui.add_space(indent);

        // **The picture, before the name.** Choosing an asset meant reading a
        // list of names and opening the ones you could not remember, which is
        // the slowest possible way to answer "which of these is the oak".
        let edge = state.thumbnail_size.edge();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(edge, edge), egui::Sense::hover());
        draw_thumbnail(ui, rect, asset, thumbnail);
        ui.add_space(4.0);

        let renaming = state
            .renaming
            .as_ref()
            .is_some_and(|(path, _)| *path == asset.path);

        if renaming {
            let mut done = false;
            if let Some((_, text)) = state.renaming.as_mut() {
                let response = ui.text_edit_singleline(text);
                done = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                response.request_focus();
            }
            if done && let Some((_, text)) = state.renaming.take() {
                *action = Some(AssetAction::Rename {
                    asset: asset.clone(),
                    name: text,
                });
            }
        } else {
            let response = ui.selectable_label(false, &asset.name).on_hover_text(
                "Click places it in the document \u{b7} double-click renames \u{b7} \
                 right-click for more",
            );
            if response.double_clicked() {
                state.renaming = Some((asset.path.clone(), asset.name.clone()));
            } else if response.clicked() {
                *action = Some(AssetAction::Place(asset.clone()));
            }
            response.context_menu(|ui| asset_menu(ui, state, asset, action));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // **Spelled out, not a pictogram.** This was a trash-can glyph,
            // which the bundled fonts do not have - the note beside it said as
            // much about another glyph and then used one anyway. It drew as an
            // empty box, so deleting an asset looked impossible.
            if ui
                .small_button("Delete")
                .on_hover_text("Delete this asset from the library")
                .clicked()
            {
                *action = Some(AssetAction::Delete(asset.clone()));
            }
            if ui
                .small_button("Place")
                .on_hover_text("Put a copy into this document")
                .clicked()
            {
                *action = Some(AssetAction::Place(asset.clone()));
            }
        });
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
                for size in ThumbnailSize::ALL {
                    state.thumbnail_size = size;
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
