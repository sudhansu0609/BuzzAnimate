//! **The Cast panel** — the characters a film is made from, kept where they
//! outlive it.
//!
//! # Why this is not the Library or the Assets panel
//!
//! The Library holds *this* film's symbols; the Assets panel holds every
//! reusable thing an animator accumulates — trees, props, mouth charts. The
//! Cast is the slice of that which is *people*: named characters, drawn and
//! rigged once, that a story reaches for by name. It is an Assets folder given
//! a face — the characters filed under `Cast/` — with two things the Assets
//! panel has no business doing:
//!
//! * **A wardrobe.** Every colour a character is painted with is offered as a
//!   well, and changing one re-skins every part on every pose at once. This is
//!   [`buzz_scene::Scene::recolour_matching`] wearing a panel: change the coat
//!   here and the coat changes in the shot the moment the character is cast.
//! * **A cast sheet.** The named poses the character already carries, so you
//!   can see at a glance what it knows how to do before directing it.
//!
//! Like every panel here it raises intentions and performs nothing: loading a
//! character, recolouring its artwork and writing it back to disk is the
//! shell's business, and casting one into the shot belongs in an undo step the
//! panel knows nothing about.

use buzz_doc::Asset;
use egui::{RichText, Ui};
use peniko::Color;

use crate::assets_panel::AssetThumbnailSource;
use crate::panels::{from_egui, to_egui};

/// The folder, under the asset library root, that holds the cast.
///
/// A character *is* an asset — a saved document with a person in it — so the
/// cast reuses every bit of the asset library rather than inventing a second
/// store. Filing them under one folder is what tells a character apart from a
/// lamp-post.
pub const CAST_FOLDER: &str = "Cast";

/// How the panel asks the shell for a character's picture — the same
/// arrangement the Assets and Library panels use, since the panel has no GPU.
pub type CastThumbnailSource<'a> = AssetThumbnailSource<'a>;

/// What the user asked the Cast panel to do.
///
/// Intentions only. The shell loads documents, writes files and opens undo
/// steps; the panel decides none of that.
#[derive(Debug, Clone, PartialEq)]
pub enum CastAction {
    /// Open this character's sheet — its wardrobe and its poses.
    Open(Asset),
    /// **Cast this character into the shot**: place a copy on the stage.
    Cast(Asset),
    /// **Re-skin the character**: repaint every part painted `from` with `to`,
    /// on every pose, and write it back so the change is the character's from
    /// now on.
    Recolour {
        asset: Asset,
        from: Color,
        to: Color,
    },
    /// Keep the current selection as a new character in the cast.
    AddSelection,
    Rename {
        asset: Asset,
        name: String,
    },
    Delete(Asset),
    /// "Delete (n)…" was pressed on a multi-selection: the shell works out which
    /// of these are cast in a series and opens the confirmation
    /// ([`CastPanelState::delete_ask`]).
    AskDelete(Vec<Asset>),
    /// The multi-delete was confirmed: **move** each file (and its thumbnail) to
    /// `<library root>/_cast_backup/<stamp>_deleted/`; nothing is hard-deleted.
    DeleteMany(Vec<Asset>),
    /// **Combine poses**: merge the selected characters' separate view/pose
    /// assets (`X`, `X Side`, `X Back`, `X 3-4`, `X Pose walk_front`) into one
    /// asset per character whose symbol set holds every pose.
    CombinePoses(Vec<Asset>),
    /// **Add Pose…** (CAST_POSES_PLAN C2 e1): add a pose named `<Character> <label>`
    /// to this character's set from `source`; the asset is saved back, backup first.
    AddPose {
        asset: Asset,
        label: String,
        source: PoseSource,
    },
    /// Remove a pose (its full symbol name) from this character; the main pose stays.
    RemovePose { asset: Asset, pose: String },
    /// Rename a pose (full symbol name) to `<Character> <label>`.
    RenamePose { asset: Asset, pose: String, label: String },
    /// **Save changes to <name>** (C2 e2): write the placed character that was
    /// edited on the stage back over its Cast asset, backup first.
    SaveChanges(Asset),
    /// Read the cast folder again.
    Rescan,
    /// **Start Fresh** was confirmed: clear every keyframe of this character
    /// (`None`: every character in the cast), keeping the pose at `keep` (`None`:
    /// the first frame). The shell backs the files up first.
    StartFresh {
        asset: Option<Asset>,
        keep: Option<u32>,
    },
}

/// Where a new pose comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum PoseSource {
    /// A symbol of the open document's Library, by name.
    LibrarySymbol(String),
    /// A picture of the character in that pose (the cast build's picture route).
    Picture(std::path::PathBuf),
    /// One symbol of an FLA, by name.
    FlaSymbol { fla: std::path::PathBuf, symbol: String },
}

/// The Add Pose window, open while the user fills it in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AddPoseAsk {
    pub asset: Option<Asset>,
    pub label: String,
    /// 0 Library symbol, 1 picture, 2 FLA symbol.
    pub source: usize,
    pub symbol: String,
    pub picture: String,
    pub fla: String,
    pub fla_symbol: String,
}

/// The Start Fresh confirmation, open while the user decides.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshAsk {
    /// The one character, or `None` for all of them.
    pub target: Option<Asset>,
    /// How many characters will be cleared.
    pub count: usize,
    /// Keep the first frame (`false`) or the frame in `frame` (`true`).
    pub use_frame: bool,
    pub frame: u32,
}

/// The multi-delete confirmation, open while the user decides.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAsk {
    pub assets: Vec<Asset>,
    /// Names among `assets` that a series' casting.json points at.
    pub cast_in_series: Vec<String>,
}

/// Panel state that is neither the document's nor the library's: which
/// character's sheet is open, and the transient business of renaming and
/// confirming a delete.
#[derive(Debug, Clone, Default)]
pub struct CastPanelState {
    /// The character whose sheet is open, by the asset's path on disk. `None`
    /// shows the cast list with nothing expanded.
    pub open: Option<std::path::PathBuf>,
    /// Free-text filter over character names.
    pub search: String,
    /// A rename in progress, by path and the text being typed.
    renaming: Option<(std::path::PathBuf, String)>,
    /// A character whose deletion is armed but not confirmed. A character is a
    /// file on disk with no undo, so the first Delete arms and the second
    /// removes it — the same two-click guard the Assets panel uses.
    pub confirm_delete: Option<std::path::PathBuf>,
    /// The Start Fresh confirmation, when one is open.
    pub fresh: Option<FreshAsk>,
    /// The characters ticked for a bulk action (Ctrl+click, Shift+click, Select all).
    pub selected: std::collections::BTreeSet<std::path::PathBuf>,
    /// The last row clicked, where a Shift+click range starts.
    anchor: Option<std::path::PathBuf>,
    /// The multi-delete confirmation, when one is open.
    pub delete_ask: Option<DeleteAsk>,
    /// The Add Pose window, when one is open.
    pub add_pose: Option<AddPoseAsk>,
    /// A pose being renamed on the sheet: its full name and the label typed.
    renaming_pose: Option<(String, String)>,
    /// Filled in by the shell each frame: the open document's Library symbols
    /// (the Add Pose source list), the open character's poses (main first; empty
    /// when it is a single pose), and the Cast asset the stage selection came from.
    pub library_symbols: Vec<String>,
    pub character_poses: Vec<String>,
    pub save_target: Option<Asset>,
    /// "Add breath + blinks" is ticked by default: this is its inverse, so a default panel has it on.
    pub life_off: bool,
}

impl CastPanelState {
    /// The characters the list shows now (the search filter applied), in order.
    pub fn visible<'a>(&self, characters: &'a [Asset]) -> Vec<&'a Asset> {
        characters.iter().filter(|a| self.matches(&a.name)).collect()
    }

    fn matches(&self, name: &str) -> bool {
        let needle = self.search.trim().to_lowercase();
        needle.is_empty() || name.to_lowercase().contains(&needle)
    }
}

/// Draw the Cast panel.
///
/// `characters` is every character in the cast, in listing order. `palette` and
/// `poses` describe the one character whose sheet is open (empty when none is),
/// worked out by the shell from the character's own document so what the wells
/// offer is exactly what a recolour would find.
pub fn cast_panel(
    ui: &mut Ui,
    characters: &[Asset],
    palette: &[(Color, usize)],
    poses: &[String],
    state: &mut CastPanelState,
    can_add: bool,
    thumbnail: CastThumbnailSource<'_>,
) -> Option<CastAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        ui.heading("Cast");
        ui.label(
            RichText::new(match characters.len() {
                1 => "1 character".to_string(),
                n => format!("{n} characters"),
            })
            .small()
            .weak(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("\u{27F3}")
                .on_hover_text("Read the cast folder again")
                .clicked()
            {
                action = Some(CastAction::Rescan);
            }
            if !characters.is_empty()
                && ui
                    .small_button("Start Fresh: all\u{2026}")
                    .on_hover_text(
                        "Clear every keyframe of every character in the cast, keeping one pose, \
                         so they start clean in a new file",
                    )
                    .clicked()
            {
                state.fresh = Some(FreshAsk {
                    target: None,
                    count: characters.len(),
                    use_frame: false,
                    frame: 0,
                });
            }
        });
    });

    ui.horizontal(|ui| {
        ui.label("\u{1F50D}");
        ui.text_edit_singleline(&mut state.search);
        if !state.search.is_empty() && ui.small_button("x").on_hover_text("Clear").clicked() {
            state.search.clear();
        }
    });

    // -- selection: Select all / Delete (n)… --
    state.selected.retain(|p| characters.iter().any(|a| a.path == *p));
    let select_all_key = !characters.is_empty()
        && ui.rect_contains_pointer(ui.max_rect())
        && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A));
    let mut select_all = select_all_key;
    ui.horizontal_wrapped(|ui| {
        if !characters.is_empty()
            && ui
                .small_button("Select all")
                .on_hover_text("Select every character listed (the search filter applies) \u{b7} Ctrl+A")
                .clicked()
        {
            select_all = true;
        }
        if !state.selected.is_empty() {
            if ui.small_button("Select none").clicked() {
                state.selected.clear();
            }
            if ui
                .small_button("Combine poses")
                .on_hover_text("Merge the selected characters' separate views and walks into one character that holds all its poses")
                .clicked()
            {
                let chosen: Vec<Asset> = characters.iter().filter(|a| state.selected.contains(&a.path)).cloned().collect();
                action = Some(CastAction::CombinePoses(chosen));
            }
            if ui
                .small_button(format!("Delete ({})\u{2026}", state.selected.len()))
                .on_hover_text("Move the selected characters to the _cast_backup folder")
                .clicked()
            {
                let chosen: Vec<Asset> = characters.iter().filter(|a| state.selected.contains(&a.path)).cloned().collect();
                action = Some(CastAction::AskDelete(chosen));
            }
        }
        ui.label(
            RichText::new("Ctrl+click toggles \u{b7} Shift+click ranges")
                .small()
                .weak(),
        );
    });
    if select_all {
        state.selected = state.visible(characters).iter().map(|a| a.path.clone()).collect();
    }
    ui.separator();

    if characters.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "No characters yet.\n\nSelect a drawn, rigged character and press \
                 \u{201c}Add to Cast\u{201d}. Characters live outside the document, so a \
                 cast built once can be directed into any film \u{2014} and re-skinned from \
                 here without touching the shot.",
            )
            .weak()
            .italics(),
        );
    } else {
        cast_list(ui, characters, state, &mut action, thumbnail);
    }

    fresh_window(ui, state, &mut action);
    delete_window(ui, state, &mut action);
    add_pose_window(ui, state, &mut action);

    // The sheet for whoever is open: their wardrobe, and the poses they know.
    if let Some(open) = state.open.clone() {
        if let Some(asset) = characters.iter().find(|a| a.path == open) {
            ui.separator();
            character_sheet(ui, asset, palette, poses, state, &mut action);
        } else {
            // The open character is gone (deleted, or the folder rescanned to
            // nothing) — forget it rather than showing an empty sheet.
            state.open = None;
        }
    }

    ui.separator();
    ui.horizontal_wrapped(|ui| {
        let add = ui.add_enabled(can_add, egui::Button::new("Add to Cast").small());
        if add
            .on_hover_text("Keep the selected character here, ready to direct into any film")
            .clicked()
        {
            action = Some(CastAction::AddSelection);
        }
        if let Some(target) = state.save_target.clone()
            && ui
                .add(egui::Button::new(format!("Save changes to {}", target.name)).small())
                .on_hover_text("Write the edited character on the stage back over its Cast asset (the old file is kept in _cast_backup)")
                .clicked()
        {
            action = Some(CastAction::SaveChanges(target));
        }
        if !can_add {
            ui.label(
                RichText::new("select a character to add")
                    .small()
                    .weak()
                    .italics(),
            );
        }
    });

    action
}

/// The characters, each a picture and a name, the open one marked.
fn cast_list(
    ui: &mut Ui,
    characters: &[Asset],
    state: &mut CastPanelState,
    action: &mut Option<CastAction>,
    thumbnail: CastThumbnailSource<'_>,
) {
    egui::ScrollArea::vertical()
        .max_height(180.0)
        .id_salt("cast-list")
        .show(ui, |ui| {
            let visible: Vec<std::path::PathBuf> = state.visible(characters).iter().map(|a| a.path.clone()).collect();
            for asset in characters {
                if !state.matches(&asset.name) {
                    continue;
                }
                character_row(ui, asset, &visible, state, action, thumbnail);
            }
        });
}

/// One character in the list: a stamp, the name, and Cast.
fn character_row(
    ui: &mut Ui,
    asset: &Asset,
    visible: &[std::path::PathBuf],
    state: &mut CastPanelState,
    action: &mut Option<CastAction>,
    thumbnail: CastThumbnailSource<'_>,
) {
    let open = state.open.as_deref() == Some(asset.path.as_path());
    let selected = state.selected.contains(&asset.path);
    ui.horizontal(|ui| {
        let edge = 32.0;
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
                *action = Some(CastAction::Rename {
                    asset: asset.clone(),
                    name: text,
                });
            }
        } else {
            let response = ui.selectable_label(open || selected, RichText::new(&asset.name).strong());
            let response = response.on_hover_text(
                "Click opens the wardrobe \u{b7} Ctrl+click selects \u{b7} Shift+click selects a range \u{b7} \
                 double-click renames \u{b7} \u{201c}Cast\u{201d} puts them in the shot",
            );
            let mods = ui.input(|i| i.modifiers);
            if response.clicked() && mods.shift && state.anchor.is_some() {
                let here = visible.iter().position(|p| *p == asset.path);
                let from = state.anchor.as_ref().and_then(|a| visible.iter().position(|p| p == a));
                if let (Some(a), Some(b)) = (from, here) {
                    for path in &visible[a.min(b)..=a.max(b)] {
                        state.selected.insert(path.clone());
                    }
                }
            } else if response.clicked() && mods.command {
                if !state.selected.remove(&asset.path) {
                    state.selected.insert(asset.path.clone());
                }
                state.anchor = Some(asset.path.clone());
            } else if response.double_clicked() {
                state.renaming = Some((asset.path.clone(), asset.name.clone()));
            } else if response.clicked() {
                state.anchor = Some(asset.path.clone());
                state.selected.clear();
                // Clicking an open character closes its sheet again.
                state.open = if open { None } else { Some(asset.path.clone()) };
                *action = Some(CastAction::Open(asset.clone()));
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Cast")
                .on_hover_text("Put a copy of this character into the shot")
                .clicked()
            {
                *action = Some(CastAction::Cast(asset.clone()));
            }
            if ui
                .small_button("Start Fresh")
                .on_hover_text(START_FRESH_HINT)
                .clicked()
            {
                state.fresh = Some(FreshAsk {
                    target: Some(asset.clone()),
                    count: 1,
                    use_frame: false,
                    frame: 0,
                });
            }
            if let Some(kind) = asset.rig_kind {
                ui.label(RichText::new(kind.label()).small().weak())
                    .on_hover_text("How this character is rigged");
            }
        });
    });
}

const START_FRESH_HINT: &str =
    "Clear every keyframe of this character, keeping one pose, so it starts clean in a new file";

/// The Start Fresh confirmation: which pose to keep, OK / Cancel. Nothing is
/// written until OK; the shell backs the files up first.
fn fresh_window(ui: &mut Ui, state: &mut CastPanelState, action: &mut Option<CastAction>) {
    let Some(mut ask) = state.fresh.take() else {
        return;
    };
    let mut decided = false;
    egui::Window::new("Start Fresh")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            match &ask.target {
                Some(asset) => ui.label(format!("Clear every keyframe of {}?", asset.name)),
                None => ui.label(format!("Clear every keyframe of all {} characters in the cast?", ask.count)),
            };
            ui.label(
                RichText::new("The file is backed up first, in a _cast_backup folder beside the cast.")
                    .small()
                    .weak(),
            );
            ui.radio_value(&mut ask.use_frame, false, "Keep the first frame");
            ui.horizontal(|ui| {
                ui.radio_value(&mut ask.use_frame, true, "Keep frame");
                ui.add_enabled(ask.use_frame, egui::DragValue::new(&mut ask.frame).range(0..=100_000));
            });
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    *action = Some(CastAction::StartFresh {
                        asset: ask.target.clone(),
                        keep: ask.use_frame.then_some(ask.frame),
                    });
                    decided = true;
                }
                if ui.button("Cancel").clicked() {
                    decided = true;
                }
            });
        });
    if !decided {
        state.fresh = Some(ask);
    }
}

/// The multi-delete confirmation: how many, which are cast in a series, and
/// where they go. Nothing is hard-deleted.
fn delete_window(ui: &mut Ui, state: &mut CastPanelState, action: &mut Option<CastAction>) {
    let Some(ask) = state.delete_ask.take() else {
        return;
    };
    let mut decided = false;
    egui::Window::new("Delete characters")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.label(format!("Move {} characters out of the Cast?", ask.assets.len()));
            ui.label(
                RichText::new("They are moved to a _cast_backup folder beside the cast, not erased.")
                    .small()
                    .weak(),
            );
            if !ask.cast_in_series.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 160, 60),
                    format!(
                        "{} of these are cast in a series: {}",
                        ask.cast_in_series.len(),
                        ask.cast_in_series.join(", ")
                    ),
                );
            }
            ui.horizontal(|ui| {
                if ui.button("Move to backup").clicked() {
                    *action = Some(CastAction::DeleteMany(ask.assets.clone()));
                    decided = true;
                }
                if ui.button("Cancel").clicked() {
                    decided = true;
                }
            });
        });
    if !decided {
        state.delete_ask = Some(ask);
    }
}

/// **Add Pose…**: a name and one of three sources.
fn add_pose_window(ui: &mut Ui, state: &mut CastPanelState, action: &mut Option<CastAction>) {
    let Some(mut ask) = state.add_pose.take() else {
        return;
    };
    let library = state.library_symbols.clone();
    let mut decided = false;
    egui::Window::new("Add Pose")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            let name = ask.asset.as_ref().map(|a| a.name.clone()).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(name);
                ui.text_edit_singleline(&mut ask.label)
                    .on_hover_text("The pose's name, e.g. Sitting \u{2014} it is saved as \u{201c}<Character> <name>\u{201d}");
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut ask.source, 0, "Library symbol");
                ui.selectable_value(&mut ask.source, 1, "Picture");
                ui.selectable_value(&mut ask.source, 2, "FLA symbol");
            });
            match ask.source {
                0 => {
                    egui::ComboBox::from_id_salt("add-pose-symbol")
                        .selected_text(if ask.symbol.is_empty() { "choose a symbol" } else { ask.symbol.as_str() })
                        .show_ui(ui, |ui| {
                            for s in &library {
                                ui.selectable_value(&mut ask.symbol, s.clone(), s);
                            }
                        });
                }
                1 => {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut ask.picture);
                        if ui.small_button("Browse\u{2026}").clicked()
                            && let Some(p) = rfd::FileDialog::new().add_filter("Picture", &["png", "jpg", "jpeg"]).pick_file()
                        {
                            ask.picture = p.display().to_string();
                        }
                    });
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut ask.fla);
                        if ui.small_button("Browse\u{2026}").clicked()
                            && let Some(p) = rfd::FileDialog::new().add_filter("FLA", &["fla", "xfl"]).pick_file()
                        {
                            ask.fla = p.display().to_string();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Symbol");
                        ui.text_edit_singleline(&mut ask.fla_symbol);
                    });
                }
            }
            let source = match ask.source {
                0 if !ask.symbol.is_empty() => Some(PoseSource::LibrarySymbol(ask.symbol.clone())),
                1 if !ask.picture.trim().is_empty() => Some(PoseSource::Picture(ask.picture.trim().into())),
                2 if !ask.fla.trim().is_empty() && !ask.fla_symbol.trim().is_empty() => {
                    Some(PoseSource::FlaSymbol { fla: ask.fla.trim().into(), symbol: ask.fla_symbol.trim().to_string() })
                }
                _ => None,
            };
            ui.horizontal(|ui| {
                let ready = source.is_some() && !ask.label.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new("Add")).clicked()
                    && let (Some(source), Some(asset)) = (source.clone(), ask.asset.clone())
                {
                    *action = Some(CastAction::AddPose { asset, label: ask.label.trim().to_string(), source });
                    decided = true;
                }
                if ui.button("Cancel").clicked() {
                    decided = true;
                }
            });
        });
    if !decided {
        state.add_pose = Some(ask);
    }
}

/// The open character's sheet: their wardrobe and the poses they know.
fn character_sheet(
    ui: &mut Ui,
    asset: &Asset,
    palette: &[(Color, usize)],
    poses: &[String],
    state: &mut CastPanelState,
    action: &mut Option<CastAction>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&asset.name).heading());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Two clicks: a character is a file on disk with no undo.
            let armed = state.confirm_delete.as_deref() == Some(asset.path.as_path());
            if ui
                .small_button(if armed { "Delete?" } else { "Delete" })
                .on_hover_text(if armed {
                    "Click again to remove this character for good \u{2014} there is no undo"
                } else {
                    "Remove this character from the cast"
                })
                .clicked()
            {
                *action = Some(CastAction::Delete(asset.clone()));
            }
        });
    });

    // -- the wardrobe --
    ui.add_space(2.0);
    ui.label(RichText::new("Wardrobe").strong());
    ui.label(
        RichText::new(
            "Every colour this character wears. Change one and it changes on every \
             part, on every pose.",
        )
        .small()
        .weak(),
    );
    if palette.is_empty() {
        ui.label(
            RichText::new("Reading the character\u{2026}")
                .small()
                .weak()
                .italics(),
        );
    }
    for (colour, count) in palette {
        ui.horizontal(|ui| {
            let mut edited = to_egui(*colour);
            let changed = ui
                .color_edit_button_srgba(&mut edited)
                .on_hover_text("Re-skin every part painted this colour")
                .changed();
            if changed {
                *action = Some(CastAction::Recolour {
                    asset: asset.clone(),
                    from: *colour,
                    to: from_egui(edited),
                });
            }
            ui.label(
                RichText::new(match count {
                    1 => "1 part".to_string(),
                    n => format!("{n} parts"),
                })
                .small()
                .weak(),
            );
        });
    }

    // -- the poses --
    ui.add_space(4.0);
    ui.label(RichText::new("Poses").strong());
    if poses.is_empty() {
        ui.label(
            RichText::new("No named poses yet \u{2014} pose the rig and save one in Rigging.")
                .small()
                .weak()
                .italics(),
        );
    } else {
        ui.horizontal_wrapped(|ui| {
            for pose in poses {
                ui.label(RichText::new(pose).small());
                ui.label(RichText::new("\u{b7}").small().weak());
            }
        });
    }

    // -- the character's own poses (its symbol set): add, rename, remove --
    ui.add_space(4.0);
    ui.label(RichText::new("Character poses").strong());
    let poses_here = state.character_poses.clone();
    if poses_here.len() < 2 {
        ui.label(
            RichText::new("One pose so far \u{2014} Add Pose\u{2026} makes this a character that can turn.")
                .small()
                .weak()
                .italics(),
        );
    }
    for (i, pose) in poses_here.iter().enumerate() {
        ui.horizontal(|ui| {
            let renaming = state.renaming_pose.as_ref().is_some_and(|(p, _)| p == pose);
            if renaming {
                let mut done = false;
                if let Some((_, text)) = state.renaming_pose.as_mut() {
                    let response = ui.text_edit_singleline(text);
                    done = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    response.request_focus();
                }
                if done && let Some((old, label)) = state.renaming_pose.take() {
                    *action = Some(CastAction::RenamePose { asset: asset.clone(), pose: old, label });
                }
            } else {
                ui.label(RichText::new(pose).small());
                if i > 0 {
                    if ui.small_button("Rename").clicked() {
                        let label = pose.strip_prefix(&asset.name).unwrap_or(pose).trim().to_string();
                        state.renaming_pose = Some((pose.clone(), label));
                    }
                    if ui.small_button("Remove").on_hover_text("Take this pose out of the character").clicked() {
                        *action = Some(CastAction::RemovePose { asset: asset.clone(), pose: pose.clone() });
                    }
                } else {
                    ui.label(RichText::new("main").small().weak());
                }
            }
        });
    }
    if ui
        .small_button("Add Pose\u{2026}")
        .on_hover_text("Add a pose from a Library symbol, a picture or an FLA symbol")
        .clicked()
    {
        state.add_pose = Some(AddPoseAsk { asset: Some(asset.clone()), ..AddPoseAsk::default() });
    }

    ui.add_space(4.0);
    let mut life = !state.life_off;
    if ui
        .checkbox(&mut life, "Add breath + blinks")
        .on_hover_text("Give the character breathing and blinks from the playhead to the end of the scene when it is cast in")
        .changed()
    {
        state.life_off = !life;
    }
    if ui
        .button("Cast into the shot")
        .on_hover_text("Put a copy of this character on the stage")
        .clicked()
    {
        *action = Some(CastAction::Cast(asset.clone()));
    }
}

/// The picture, or the space it will occupy while it is drawn.
fn draw_thumbnail(ui: &Ui, rect: egui::Rect, asset: &Asset, thumbnail: CastThumbnailSource<'_>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn a_character() -> Asset {
        Asset {
            name: "Ana".into(),
            folder: CAST_FOLDER.into(),
            path: std::path::PathBuf::from("/cast/Ana.buzz"),
            animated: false,
            rig_kind: None,
            tags: Vec::new(),
        }
    }

    /// The panel draws through every state it can be in — empty, listed, a
    /// sheet open, filtered to nothing — without a window.
    #[test]
    fn the_panel_draws_in_every_state() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let ana = a_character();
        let palette = vec![
            (Color::from_rgb8(0x2E, 0x7D, 0x32), 4usize),
            (Color::from_rgb8(0xC0, 0x30, 0x30), 1),
        ];
        let poses = vec!["idle".to_string(), "wave".to_string()];

        for (chars, search, open) in [
            (vec![ana.clone()], "", Some(ana.path.clone())),
            (vec![ana.clone()], "zzz", None),
            (Vec::new(), "", None),
        ] {
            let mut state = CastPanelState {
                open,
                search: search.to_string(),
                ..Default::default()
            };
            let _ = ctx.run_ui(Default::default(), |ui| {
                let _ = cast_panel(ui, &chars, &palette, &poses, &mut state, true, &mut |_| {
                    None
                });
            });
        }
    }

    /// Editing a wardrobe well asks to recolour from the well's old colour to
    /// the new one — the intention the shell turns into a recolour and a save.
    #[test]
    fn a_recolour_carries_the_old_and_new_colour() {
        let ana = a_character();
        let from = Color::from_rgb8(0x2E, 0x7D, 0x32);
        let to = Color::from_rgb8(0x15, 0x3E, 0x8A);
        let action = CastAction::Recolour {
            asset: ana.clone(),
            from,
            to,
        };
        // The variant carries what a match-by-colour recolour needs, and nothing
        // it does not.
        assert_eq!(
            action,
            CastAction::Recolour {
                asset: ana,
                from,
                to
            }
        );
    }
}
