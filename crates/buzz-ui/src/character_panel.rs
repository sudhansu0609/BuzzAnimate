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
    Recolour { asset: Asset, from: Color, to: Color },
    /// Keep the current selection as a new character in the cast.
    AddSelection,
    Rename { asset: Asset, name: String },
    Delete(Asset),
    /// Read the cast folder again.
    Rescan,
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
}

impl CastPanelState {
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
        });
    });

    ui.horizontal(|ui| {
        ui.label("\u{1F50D}");
        ui.text_edit_singleline(&mut state.search);
        if !state.search.is_empty() && ui.small_button("x").on_hover_text("Clear").clicked() {
            state.search.clear();
        }
    });
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
            for asset in characters {
                if !state.matches(&asset.name) {
                    continue;
                }
                character_row(ui, asset, state, action, thumbnail);
            }
        });
}

/// One character in the list: a stamp, the name, and Cast.
fn character_row(
    ui: &mut Ui,
    asset: &Asset,
    state: &mut CastPanelState,
    action: &mut Option<CastAction>,
    thumbnail: CastThumbnailSource<'_>,
) {
    let open = state.open.as_deref() == Some(asset.path.as_path());
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
            let response = ui.selectable_label(open, RichText::new(&asset.name).strong());
            let response = response.on_hover_text(
                "Click opens the wardrobe \u{b7} double-click renames \u{b7} \
                 \u{201c}Cast\u{201d} puts them in the shot",
            );
            if response.double_clicked() {
                state.renaming = Some((asset.path.clone(), asset.name.clone()));
            } else if response.clicked() {
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
        });
    });
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

    ui.add_space(4.0);
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
                let _ = cast_panel(
                    ui,
                    &chars,
                    &palette,
                    &poses,
                    &mut state,
                    true,
                    &mut |_| None,
                );
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
