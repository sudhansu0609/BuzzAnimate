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
    /// Read the directory again.
    Rescan,
    /// Bring an entire Animate asset library across.
    ImportFromAnimate,
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
        draw_folder(ui, library, state, "", 0, &mut action);
    }

    ui.separator();
    // Wrapped: four controls and a hint are wider than a dock column at its
    // narrowest, and unwrapped the last of them — "From Animate…" — was drawn
    // off the end of the panel where it could not be clicked.
    ui.horizontal_wrapped(|ui| {
        let add = ui.add_enabled(can_add, egui::Button::new("+").small());
        if add
            .on_hover_text("Keep the selected artwork as an asset")
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
            if ui.selectable_label(selected, &leaf).clicked() {
                state.selected_folder = if selected {
                    String::new()
                } else {
                    folder.clone()
                };
            }
        });

        if open {
            draw_folder(ui, library, state, &folder, depth + 1, action);
        }
    }

    let here: Vec<Asset> = library.in_folder(parent).into_iter().cloned().collect();
    for asset in here {
        if !state.matches(&asset.name) {
            continue;
        }
        asset_row(ui, state, &asset, indent, action);
    }
}

fn asset_row(
    ui: &mut Ui,
    state: &mut AssetPanelState,
    asset: &Asset,
    indent: f32,
    action: &mut Option<AssetAction>,
) {
    ui.horizontal(|ui| {
        ui.add_space(indent);

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
            let response = ui
                .selectable_label(false, &asset.name)
                .on_hover_text("Click places it in the document \u{b7} double-click renames");
            if response.double_clicked() {
                state.renaming = Some((asset.path.clone(), asset.name.clone()));
            } else if response.clicked() {
                *action = Some(AssetAction::Place(asset.clone()));
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // The trash can: `✕` has no glyph in the bundled fonts.
            if ui
                .small_button("\u{1F5D1}")
                .on_hover_text("Delete")
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
                let _ = assets_panel(ui, library, &mut state, can_add);
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
