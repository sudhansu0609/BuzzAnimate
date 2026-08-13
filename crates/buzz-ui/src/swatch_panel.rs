//! The Swatches panel: the document's named colours, in folders.
//!
//! Animate's Swatches panel is a grid of unnamed chips. This is the same grid
//! with two additions the model already carries — a name and a folder — because
//! a production palette is a set of *decisions* ("Hero Skin Shadow"), and a
//! decision that can only be identified by its hex value gets picked wrongly at
//! four in the morning.
//!
//! The folder tree is derived from the palette each frame, exactly as the
//! Library panel derives its own, so the two cannot drift out of step with the
//! colours they organise.

use buzz_scene::{Scene, SwatchId};
use egui::{RichText, Ui};

use crate::style::DrawStyle;
use crate::theme::Palette;

/// Panel state that is not part of the document.
#[derive(Debug, Clone, Default)]
pub struct SwatchState {
    /// The swatch whose row is open for renaming, and the text so far.
    renaming: Option<(SwatchId, String)>,
    /// The folder a new swatch goes into.
    pub selected_folder: Option<String>,
    /// Folders the user has opened. Empty means everything is closed, so a new
    /// document shows its palette at the root and nothing else.
    expanded: std::collections::BTreeSet<String>,
    /// Free-text filter over names.
    pub search: String,
    /// Show the palette as a grid of chips rather than as named rows.
    ///
    /// The grid is Animate's presentation and is faster to pick from once the
    /// names are known; the list is how the names are read and edited. Both,
    /// because they are good at different moments.
    pub grid: bool,
}

impl SwatchState {
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

/// Draw the Swatches panel.
///
/// Takes the scene mutably because naming a colour, moving it between folders
/// and deleting it are edits to the document; the caller wraps this in an undo
/// step, and a frame in which nothing changed records nothing.
pub fn swatch_panel(ui: &mut Ui, scene: &mut Scene, state: &mut SwatchState, style: &mut DrawStyle) {
    ui.horizontal(|ui| {
        ui.heading("Swatches");
        ui.label(
            RichText::new(format!("{} colors", scene.swatches().len()))
                .small()
                .weak(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .selectable_label(state.grid, "Grid")
                .on_hover_text("Show chips rather than named rows")
                .clicked()
            {
                state.grid = !state.grid;
            }
        });
    });

    // What the tools are set to now, and — the point of the whole panel —
    // which named colour that is.
    ui.horizontal(|ui| {
        let named = |scene: &Scene, color| {
            scene
                .swatches()
                .find_color(color)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "unnamed".to_string())
        };
        let fill = named(scene, style.fill_color);
        let stroke = named(scene, style.stroke_color);
        chip(ui, style.fill_color, 14.0);
        ui.label(RichText::new(fill).small().weak())
            .on_hover_text("The fill colour");
        chip(ui, style.stroke_color, 14.0);
        ui.label(RichText::new(stroke).small().weak())
            .on_hover_text("The stroke colour");
    });

    ui.horizontal(|ui| {
        ui.label("\u{1F50D}");
        ui.text_edit_singleline(&mut state.search);
        if !state.search.is_empty() && ui.small_button("x").on_hover_text("Clear").clicked() {
            state.search.clear();
        }
    });
    ui.separator();

    let mut picked: Option<(peniko::Color, bool)> = None;

    // **No scroll area of its own.** The dock column this panel sits in
    // already scrolls, and a scroll area nested in one gets whatever height is
    // left over — which was four rows. A palette that shows four colours at a
    // time is not a palette; the search box is what handles a long one.
    if scene.swatches().is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "No swatches yet.\n\nPick a colour, then press + to name it and keep it \
                 with the document.",
            )
            .weak()
            .italics(),
        );
    } else {
        draw_folder(ui, scene, state, None, 0, &mut picked);
    }

    ui.separator();
    footer(ui, scene, state, style);

    if let Some((color, to_stroke)) = picked {
        if to_stroke {
            style.stroke_color = color;
            style.stroke_enabled = true;
        } else {
            style.fill_color = color;
            style.fill_enabled = true;
        }
        style.remember(color);
    }
}

/// One level of the tree: folders inside `parent`, then the colours in
/// `parent` itself.
fn draw_folder(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut SwatchState,
    parent: Option<&str>,
    depth: usize,
    picked: &mut Option<(peniko::Color, bool)>,
) {
    let indent = depth as f32 * 14.0;

    let folders: Vec<String> = scene
        .swatches()
        .child_folders(parent)
        .into_iter()
        .cloned()
        .collect();

    for folder in folders {
        let leaf = folder.rsplit('/').next().unwrap_or(&folder).to_string();
        let open = state.is_expanded(&folder) || !state.search.trim().is_empty();
        let selected = state.selected_folder.as_deref() == Some(folder.as_str());

        ui.horizontal(|ui| {
            ui.add_space(indent);
            // `⏷` and `▶`: the two arrows egui's bundled fonts actually have.
            if ui.small_button(if open { "\u{23F7}" } else { "\u{25B6}" }).clicked() {
                state.toggle(&folder);
            }
            ui.label(RichText::new("F").small().weak())
                .on_hover_text("Folder");
            if ui.selectable_label(selected, &leaf).clicked() {
                state.selected_folder = if selected { None } else { Some(folder.clone()) };
            }
        });

        if open {
            draw_folder(ui, scene, state, Some(&folder), depth + 1, picked);
        }
    }

    let here: Vec<(SwatchId, String, peniko::Color)> = scene
        .swatches()
        .in_folder(parent)
        .into_iter()
        .map(|s| (s.id, s.name.clone(), s.color))
        .collect();

    if state.grid {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(indent);
            for (_, name, color) in here.iter().filter(|(_, n, _)| state.matches(n)) {
                chip_button(ui, *color, name, picked);
            }
        });
        return;
    }

    for (id, name, color) in here {
        if !state.matches(&name) {
            continue;
        }
        swatch_row(ui, scene, state, id, &name, color, indent, picked);
    }
}

/// A colour square.
fn chip(ui: &mut Ui, color: peniko::Color, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    ui.painter()
        .rect_filled(rect, 2.0, egui::Color32::from_rgba_unmultiplied(r, g, b, a));
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, Palette::border()),
        egui::StrokeKind::Inside,
    );
    response
}

fn chip_button(
    ui: &mut Ui,
    color: peniko::Color,
    name: &str,
    picked: &mut Option<(peniko::Color, bool)>,
) {
    let response = chip(ui, color, 18.0)
        .on_hover_text(format!("{name}\nClick sets fill \u{b7} Shift-click sets stroke"));
    if response.clicked() {
        *picked = Some((color, ui.input(|i| i.modifiers.shift)));
    }
}

#[allow(clippy::too_many_arguments, reason = "internal row painter, not an API")]
fn swatch_row(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut SwatchState,
    id: SwatchId,
    name: &str,
    color: peniko::Color,
    indent: f32,
    picked: &mut Option<(peniko::Color, bool)>,
) {
    ui.horizontal(|ui| {
        ui.add_space(indent);

        if chip(ui, color, 16.0)
            .on_hover_text("Click sets fill \u{b7} Shift-click sets stroke")
            .clicked()
        {
            *picked = Some((color, ui.input(|i| i.modifiers.shift)));
        }

        if let Some((editing, text)) = state.renaming.as_mut().filter(|(e, _)| *e == id) {
            let editing = *editing;
            let response = ui.text_edit_singleline(text);
            let done = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if done {
                let new_name = text.trim().to_string();
                if !new_name.is_empty() {
                    scene.swatches_mut().update(editing, |s| s.name = new_name);
                }
                state.renaming = None;
            }
            response.request_focus();
        } else if ui
            .selectable_label(false, name)
            .on_hover_text("Double-click to rename")
            .double_clicked()
        {
            state.renaming = Some((id, name.to_string()));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // The trash can, as the Layers panel uses: `✕` (U+2715) has no
            // glyph in egui's bundled fonts and draws as an empty box — this
            // project has been caught by it before, and `theme::font_has`
            // keeps a list of the characters that are not available.
            if ui.small_button("\u{1F5D1}").on_hover_text("Delete").clicked() {
                scene.swatches_mut().remove(id);
            }

            // Which folder it lives in. A dropdown rather than drag and drop,
            // for the same reason the Library moves symbols that way: the drag
            // is a piece of work in its own right and this is the whole
            // behaviour without it.
            let folders: Vec<String> = scene.swatches().folders().cloned().collect();
            let current = scene
                .swatches()
                .get(id)
                .and_then(|s| s.folder.clone())
                .unwrap_or_default();
            let label = if current.is_empty() {
                "\u{2014}".to_string()
            } else {
                current.rsplit('/').next().unwrap_or("").to_string()
            };
            egui::ComboBox::from_id_salt(("swatch-folder", id.0))
                .selected_text(label)
                .width(64.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_empty(), "\u{2014} root").clicked() {
                        scene.swatches_mut().update(id, |s| s.folder = None);
                    }
                    for folder in folders {
                        if ui
                            .selectable_label(current == folder, &folder)
                            .clicked()
                        {
                            scene
                                .swatches_mut()
                                .update(id, |s| s.folder = Some(folder.clone()));
                        }
                    }
                });
        });
    });
}

/// New swatch, new folder, and the colour a new swatch would take.
fn footer(ui: &mut Ui, scene: &mut Scene, state: &mut SwatchState, style: &DrawStyle) {
    ui.horizontal(|ui| {
        if ui
            .small_button("+")
            .on_hover_text("Add the current fill colour to the palette")
            .clicked()
        {
            let folder = state.selected_folder.clone();
            let id = scene.add_swatch("Swatch", style.fill_color, folder);
            // Straight into a rename: a colour called "Swatch 7" is no better
            // than a hex value, and naming it later never happens.
            let name = scene
                .swatches()
                .get(id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            state.renaming = Some((id, name));
        }

        if ui
            .small_button("Fld")
            .on_hover_text("New folder")
            .clicked()
        {
            let base = "Folder";
            let mut name = base.to_string();
            let existing: Vec<String> = scene.swatches().folders().cloned().collect();
            for n in 2..1000 {
                if !existing.contains(&name) {
                    break;
                }
                name = format!("{base} {n}");
            }
            scene.swatches_mut().add_folder(&name);
            state.selected_folder = Some(name);
        }

        if let Some(folder) = state.selected_folder.clone() {
            if ui
                .small_button("Del Fld")
                .on_hover_text("Delete the selected folder, keeping its colours")
                .clicked()
            {
                scene.swatches_mut().remove_folder(&folder);
                state.selected_folder = None;
            }
            ui.label(RichText::new(folder).small().weak());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::Scene;

    /// The panel must not fall over on an empty palette, a deep tree or a
    /// search that matches nothing — the three states a listing is usually
    /// only tested in one of.
    #[test]
    fn the_panel_draws_in_every_state() {
        let ctx = egui::Context::default();
        for (empty, search) in [(true, ""), (false, ""), (false, "zzz"), (false, "red")] {
            let mut scene = if empty {
                Scene::empty()
            } else {
                Scene::default()
            };
            if !empty {
                scene.swatches_mut().add_folder("Hero/Skin");
                let id = scene.add_swatch("Shadow", peniko::Color::BLACK, Some("Hero/Skin".into()));
                assert!(scene.swatches().get(id).is_some());
            }
            let mut state = SwatchState {
                search: search.to_string(),
                ..Default::default()
            };
            let mut style = DrawStyle::default();

            // egui 0.35 roots the UI in a `Ui` rather than a `Context`.
            let _ = ctx.run_ui(Default::default(), |ui| {
                swatch_panel(ui, &mut scene, &mut state, &mut style);
            });
        }
    }

    /// Adding a colour names it and opens the rename box, because a palette of
    /// "Swatch 7" is a palette of hex values with extra steps.
    #[test]
    fn a_new_swatch_is_named_and_ready_to_rename() {
        let mut scene = Scene::default();
        let before = scene.swatches().len();
        let id = scene.add_swatch("Swatch", peniko::Color::WHITE, None);

        assert_eq!(scene.swatches().len(), before + 1);
        assert_eq!(scene.swatches().get(id).map(|s| s.name.as_str()), Some("Swatch"));
    }
}
