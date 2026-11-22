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
    /// The last swatch recoloured and how many fills it repainted, so the panel
    /// can say what just happened. A colour change that silently reaches across
    /// a whole film should say how far it reached.
    pub last_repaint: Option<(SwatchId, usize)>,
    /// Show the palette as a grid of chips rather than as named rows.
    ///
    /// The grid is Animate's presentation and is faster to pick from once the
    /// names are known; the list is how the names are read and edited. Both,
    /// because they are good at different moments.
    pub grid: bool,
    /// The colour the Add row holds: what **+ Add** puts in the palette.
    /// `None` until it is changed, so it follows the fill colour until then.
    adding: Option<peniko::Color>,
    /// A swatch just added, to bring into view once its row is drawn.
    reveal: Option<SwatchId>,
    /// The folder (by path) open for renaming, and the text so far.
    renaming_folder: Option<(String, String)>,
}

/// **Add to Swatches from any colour picker** (user 2026-11-22: "give an
/// option to add to swatches from the colour picker"). A picker cannot
/// reach the document, so its button leaves the colour here and the shell
/// adds it ([`take_add_request`], [`SwatchState::add_colour`]) in the same
/// frame, inside an undo step.
pub fn request_add(ctx: &egui::Context, colour: peniko::Color) {
    ctx.data_mut(|d| d.insert_temp(add_request_key(), Some(colour)));
    ctx.request_repaint();
}

/// The colour a picker asked to keep, once.
pub fn take_add_request(ctx: &egui::Context) -> Option<peniko::Color> {
    ctx.data_mut(|d| d.remove_temp::<Option<peniko::Color>>(add_request_key())).flatten()
}

fn add_request_key() -> egui::Id {
    egui::Id::new("swatches-add-request")
}

impl SwatchState {
    /// Keep `colour` in the palette — in the selected folder — named
    /// "Swatch N", brought into view with its name box open. Its id.
    pub fn add_colour(&mut self, scene: &mut Scene, colour: peniko::Color) -> SwatchId {
        let folder = self.selected_folder.clone();
        let id = scene.add_swatch("Swatch", colour, folder.clone());
        // Straight into a rename: a colour called "Swatch 7" is no better
        // than a hex value, and naming it later never happens. Enter on the
        // empty box keeps the given name.
        self.renaming = Some((id, String::new()));
        self.renaming_folder = None;
        self.reveal = Some(id);
        self.search.clear();
        if let Some(folder) = folder {
            self.expanded.insert(folder);
        }
        id
    }

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
pub fn swatch_panel(
    ui: &mut Ui,
    scene: &mut Scene,
    state: &mut SwatchState,
    style: &mut DrawStyle,
) {
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

    // What a colour change just reached, so a swatch that repainted forty
    // shapes across the film says so rather than doing it silently.
    if let Some((_, painted)) = state.last_repaint {
        ui.label(
            RichText::new(match painted {
                0 => "Nothing is linked to that swatch yet".to_string(),
                1 => "Repainted 1 fill".to_string(),
                n => format!("Repainted {n} fills"),
            })
            .small()
            .weak(),
        );
    }

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

    add_row(ui, scene, state, style);

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
                "No swatches yet.\n\nChoose a colour in the box above, then press + Add \
                 to name it and keep it with the document.",
            )
            .weak()
            .italics(),
        );
    } else {
        draw_folder(ui, scene, state, None, 0, &mut picked);
    }

    if let Some(folder) = state.selected_folder.clone() {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("New colours go in {folder}")).small().weak());
            if ui
                .small_button("Delete folder")
                .on_hover_text("Delete the selected folder, keeping its colours")
                .clicked()
            {
                scene.swatches_mut().remove_folder(&folder);
                state.selected_folder = None;
            }
        });
    }

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
            if ui
                .small_button(if open { "\u{23F7}" } else { "\u{25B6}" })
                .clicked()
            {
                state.toggle(&folder);
            }
            ui.label(RichText::new("F").small().weak())
                .on_hover_text("Folder");
            folder_name(ui, scene, state, &folder, &leaf, selected);
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
    let response = chip(ui, color, 18.0).on_hover_text(format!(
        "{name}\nClick sets fill \u{b7} Shift-click sets stroke"
    ));
    if response.clicked() {
        *picked = Some((color, ui.input(|i| i.modifiers.shift)));
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "internal row painter, not an API"
)]
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
    let row = ui.horizontal(|ui| {
        ui.add_space(indent);

        if chip(ui, color, 16.0)
            .on_hover_text("Click sets fill \u{b7} Shift-click sets stroke")
            .clicked()
        {
            *picked = Some((color, ui.input(|i| i.modifiers.shift)));
        }

        // The name gets what the controls on the right leave it, so a long
        // one is cut short rather than drawn under them.
        const CONTROLS: f32 = 150.0;
        let name_width = (ui.available_width() - CONTROLS).max(40.0);
        let height = ui.spacing().interact_size.y;
        if let Some((editing, text)) = state.renaming.as_mut().filter(|(e, _)| *e == id) {
            let editing = *editing;
            // A new swatch's box starts empty, its given name as the hint, so
            // typing names it rather than adding to "Swatch 3".
            let response = ui.add(egui::TextEdit::singleline(text).hint_text(name).desired_width(name_width));
            let done = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if done {
                let new_name = text.trim().to_string();
                if !new_name.is_empty() {
                    scene.swatches_mut().update(editing, |s| s.name = new_name);
                }
                state.renaming = None;
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                state.renaming = None;
            } else {
                response.request_focus();
            }
        } else {
            let label = ui
                .allocate_ui(egui::vec2(name_width, height), |ui| {
                    ui.add(egui::Label::new(name).truncate().sense(egui::Sense::click()))
                })
                .inner
                .on_hover_text(format!("{name}\nDouble-click to rename \u{b7} right-click for more"));
            if label.double_clicked() {
                state.renaming = Some((id, name.to_string()));
                state.renaming_folder = None;
            }
            row_menu(&label, scene, state, id, name);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // **Editing the colour is what makes a palette a palette.**
            //
            // Until this, a swatch could be renamed, moved and deleted but not
            // *changed* — so the palette was a list of colours somebody had
            // picked, and changing a character's coat meant finding every piece
            // of it by eye. Changing it here repaints every fill and stroke
            // linked to this swatch, everywhere in the document, in one step.
            let mut edited = color;
            if crate::panels::color_row(ui, &format!("swatch-{}", id.0), &mut edited) {
                let painted = scene.recolour_swatch(id, edited);
                state.last_repaint = Some((id, painted));
            }

            // The trash can, as the Layers panel uses: `✕` (U+2715) has no
            // glyph in egui's bundled fonts and draws as an empty box — this
            // project has been caught by it before, and `theme::font_has`
            // keeps a list of the characters that are not available.
            if ui
                .small_button("\u{1F5D1}")
                .on_hover_text("Delete")
                .clicked()
            {
                scene.swatches_mut().remove(id);
            }
        });
    });
    if state.reveal == Some(id) {
        row.response.scroll_to_me(Some(egui::Align::Center));
        state.reveal = None;
    }
}

/// A swatch's right-click menu: rename, the folder it lives in, delete.
/// The folder was a dropdown on every row, drawn over the name.
fn row_menu(label: &egui::Response, scene: &mut Scene, state: &mut SwatchState, id: SwatchId, name: &str) {
    label.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            state.renaming = Some((id, name.to_string()));
            state.renaming_folder = None;
            ui.close();
        }
        let current = scene.swatches().get(id).and_then(|s| s.folder.clone());
        let folders: Vec<String> = scene.swatches().folders().cloned().collect();
        ui.menu_button("Move to", |ui| {
            if ui.selectable_label(current.is_none(), "\u{2014} root").clicked() {
                scene.swatches_mut().update(id, |s| s.folder = None);
                ui.close();
            }
            for folder in folders {
                if ui.selectable_label(current.as_deref() == Some(folder.as_str()), &folder).clicked() {
                    scene.swatches_mut().update(id, |s| s.folder = Some(folder.clone()));
                    ui.close();
                }
            }
        });
        if ui.button("Delete").clicked() {
            scene.swatches_mut().remove(id);
            ui.close();
        }
    });
}

/// Make a folder under `parent` (the root for `None`), select it so new
/// colours go in, and open its name for typing.
fn new_folder(scene: &mut Scene, state: &mut SwatchState, parent: Option<&str>) {
    let path = scene.swatches().unique_folder(parent, "Folder");
    scene.swatches_mut().add_folder(&path);
    if let Some(parent) = parent {
        state.expanded.insert(parent.to_string());
    }
    state.selected_folder = Some(path.clone());
    state.renaming_folder = Some((path, String::new()));
    state.renaming = None;
    state.search.clear();
}

/// **A folder's name: click selects it, double-click renames it, and
/// right-click offers Rename, New Folder Inside and Delete** (user
/// 2026-11-22: "I am not able to rename the swatch group and add folders").
/// A new folder opens with its name ready to type; Enter keeps it, an empty
/// box keeps the name it had, Esc leaves it.
fn folder_name(ui: &mut Ui, scene: &mut Scene, state: &mut SwatchState, folder: &str, leaf: &str, selected: bool) {
    if let Some((_, text)) = state.renaming_folder.as_mut().filter(|(f, _)| f == folder) {
        let response = ui.add(egui::TextEdit::singleline(text).hint_text(leaf).desired_width(140.0));
        let done = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if done {
            let name = text.trim().to_string();
            if !name.is_empty()
                && let Some(renamed) = scene.swatches_mut().rename_folder(folder, &name)
            {
                if state.selected_folder.as_deref() == Some(folder) {
                    state.selected_folder = Some(renamed.clone());
                }
                if state.expanded.remove(folder) {
                    state.expanded.insert(renamed);
                }
            }
            state.renaming_folder = None;
        } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.renaming_folder = None;
        } else {
            response.request_focus();
        }
        return;
    }
    let label = ui
        .selectable_label(selected, leaf)
        .on_hover_text("Click: new colours go in this folder \u{b7} double-click to rename \u{b7} right-click for more");
    if label.double_clicked() {
        // Its first click toggled the selection; the folder being renamed is
        // the one being worked in.
        state.selected_folder = Some(folder.to_string());
        state.renaming_folder = Some((folder.to_string(), leaf.to_string()));
        state.renaming = None;
    } else if label.clicked() {
        state.selected_folder = if selected { None } else { Some(folder.to_string()) };
    }
    label.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            state.renaming_folder = Some((folder.to_string(), leaf.to_string()));
            state.renaming = None;
            ui.close();
        }
        if ui.button("New folder inside").clicked() {
            new_folder(scene, state, Some(folder));
            ui.close();
        }
        if ui.button("Delete folder (keep its colours)").clicked() {
            scene.swatches_mut().remove_folder(folder);
            if state.selected_folder.as_deref().is_some_and(|f| f == folder || f.starts_with(&format!("{folder}/"))) {
                state.selected_folder = None;
            }
            ui.close();
        }
    });
}

/// **Add a colour, at the top where it is always in reach** (user
/// 2026-11-22: "I am not able to manually create swatches and add a new
/// colour"). It was a `+` at the very bottom, under every colour — in the
/// icon strip's pop-out, below its edge — and it could only add the fill
/// colour already set somewhere else, at the bottom of the list, out of
/// sight. Now the colour to add is chosen right here (it starts as the fill
/// colour), **+ Add** keeps it, and the new swatch comes into view with its
/// name ready to type.
fn add_row(ui: &mut Ui, scene: &mut Scene, state: &mut SwatchState, style: &DrawStyle) {
    ui.horizontal(|ui| {
        let mut colour = state.adding.unwrap_or(style.fill_color);
        if crate::panels::color_row(ui, "swatch-new", &mut colour) {
            state.adding = Some(colour);
        }
        if ui
            .button("+ Add")
            .on_hover_text("Keep this colour in the palette, named \u{2014} choose it in the box first (it starts as the fill colour)")
            .clicked()
        {
            state.add_colour(scene, colour);
            state.adding = None;
        }
        if ui
            .button("New folder")
            .on_hover_text("A folder for colours, named as you type \u{2014} right-click a folder for one inside it")
            .clicked()
        {
            new_folder(scene, state, None);
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

    /// **+ Add keeps the colour chosen in the Add row, named as typed**
    /// (user 2026-11-22: "I am not able to manually create swatches and add
    /// a new colour"): clicked in a real frame of the panel, a new swatch of
    /// that colour appears with its name box open, and what is typed is its
    /// name. Enter on the empty box keeps the given name.
    #[test]
    fn plus_add_keeps_the_chosen_colour_under_the_typed_name() {
        let ctx = egui::Context::default();
        let mut scene = Scene::default();
        let mut state = SwatchState::default();
        let mut style = DrawStyle::default();
        let skin = peniko::Color::from_rgba8(0xE0, 0xB0, 0x90, 255);
        state.adding = Some(skin);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 900.0));
        let mut frame = |scene: &mut Scene, state: &mut SwatchState, events: Vec<egui::Event>| {
            let input = egui::RawInput { events, screen_rect: Some(screen), ..Default::default() };
            let out = ctx.run_ui(input, |ui| swatch_panel(ui, scene, state, &mut style));
            let mut found = None;
            fn find(shape: &egui::Shape, found: &mut Option<egui::Pos2>) {
                match shape {
                    egui::Shape::Text(t) if t.galley.text() == "+ Add" => *found = Some(t.pos + t.galley.rect.center().to_vec2()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| find(s, found)),
                    _ => {}
                }
            }
            out.shapes.iter().for_each(|s| find(&s.shape, &mut found));
            found
        };
        let add = frame(&mut scene, &mut state, vec![]).expect("+ Add is drawn");
        let before = scene.swatches().len();
        let press = |pressed| egui::Event::PointerButton { pos: add, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut scene, &mut state, vec![egui::Event::PointerMoved(add), press(true)]);
        frame(&mut scene, &mut state, vec![press(false)]);
        assert_eq!(scene.swatches().len(), before + 1, "+ Add adds a swatch");
        frame(&mut scene, &mut state, vec![]);
        frame(&mut scene, &mut state, vec![egui::Event::Text("Skin".into())]);
        frame(&mut scene, &mut state, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        let added = scene.swatches().iter().last().expect("the new swatch");
        assert_eq!(added.name, "Skin");
        assert_eq!(added.color.to_rgba8().to_u8_array(), skin.to_rgba8().to_u8_array());
        assert!(state.renaming.is_none(), "the name box closes on Enter");

        // Enter on the empty box keeps the name it was given.
        let add = frame(&mut scene, &mut state, vec![]).expect("+ Add is drawn");
        let press = |pressed| egui::Event::PointerButton { pos: add, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut scene, &mut state, vec![egui::Event::PointerMoved(add), press(true)]);
        frame(&mut scene, &mut state, vec![press(false)]);
        frame(&mut scene, &mut state, vec![]);
        frame(&mut scene, &mut state, vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        let added = scene.swatches().iter().last().expect("the second swatch");
        assert!(added.name.starts_with("Swatch"), "kept {:?}", added.name);
    }

    /// **New folder opens with its name ready to type, and a folder renames
    /// by double-click** (user 2026-11-22: "I am not able to rename the
    /// swatch group and add folders"), clicked in real frames of the panel.
    #[test]
    fn a_new_folder_is_named_as_typed_and_renames_by_double_click() {
        let ctx = egui::Context::default();
        let mut scene = Scene::default();
        let mut state = SwatchState::default();
        let mut style = DrawStyle::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 900.0));
        let mut t = 0.0;
        let mut frame = |scene: &mut Scene, state: &mut SwatchState, events: Vec<egui::Event>, text: &str| {
            t += 0.05;
            let input = egui::RawInput { events, screen_rect: Some(screen), time: Some(t), ..Default::default() };
            let out = ctx.run_ui(input, |ui| swatch_panel(ui, scene, state, &mut style));
            fn find(shape: &egui::Shape, text: &str, found: &mut Option<egui::Pos2>) {
                match shape {
                    egui::Shape::Text(t) if t.galley.text() == text => *found = Some(t.pos + t.galley.rect.center().to_vec2()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| find(s, text, found)),
                    _ => {}
                }
            }
            let mut found = None;
            out.shapes.iter().for_each(|s| find(&s.shape, text, &mut found));
            found
        };
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let enter = || egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };

        let at = frame(&mut scene, &mut state, vec![], "New folder").expect("New folder is drawn");
        frame(&mut scene, &mut state, vec![egui::Event::PointerMoved(at), button(at, true)], "");
        frame(&mut scene, &mut state, vec![button(at, false)], "");
        frame(&mut scene, &mut state, vec![], "");
        frame(&mut scene, &mut state, vec![egui::Event::Text("Skin".into())], "");
        frame(&mut scene, &mut state, vec![enter()], "");
        let folders: Vec<String> = scene.swatches().folders().cloned().collect();
        assert_eq!(folders, ["Skin"], "the new folder is named as typed");
        assert_eq!(state.selected_folder.as_deref(), Some("Skin"), "and new colours go in it");

        // Double-click its name to rename it — a moment later, or egui counts
        // the clicks with New folder's as a triple-click.
        for _ in 0..20 {
            frame(&mut scene, &mut state, vec![], "");
        }
        let at = frame(&mut scene, &mut state, vec![], "Skin").expect("the folder's name is drawn");
        for _ in 0..2 {
            frame(&mut scene, &mut state, vec![egui::Event::PointerMoved(at), button(at, true)], "");
            frame(&mut scene, &mut state, vec![button(at, false)], "");
        }
        frame(&mut scene, &mut state, vec![], "");
        assert!(state.renaming_folder.is_some(), "double-click opens the name (at {at:?}, selected {:?})", state.selected_folder);
        for key in [egui::Key::A] {
            frame(&mut scene, &mut state, vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }], "");
        }
        frame(&mut scene, &mut state, vec![egui::Event::Text("Face".into())], "");
        frame(&mut scene, &mut state, vec![enter()], "");
        let folders: Vec<String> = scene.swatches().folders().cloned().collect();
        assert_eq!(folders, ["Face"], "renamed by double-click");
        assert_eq!(state.selected_folder.as_deref(), Some("Face"));
    }

    /// Adding a colour names it and opens the rename box, because a palette of
    /// "Swatch 7" is a palette of hex values with extra steps.
    #[test]
    fn a_new_swatch_is_named_and_ready_to_rename() {
        let mut scene = Scene::default();
        let before = scene.swatches().len();
        let id = scene.add_swatch("Swatch", peniko::Color::WHITE, None);

        assert_eq!(scene.swatches().len(), before + 1);
        assert_eq!(
            scene.swatches().get(id).map(|s| s.name.as_str()),
            Some("Swatch")
        );
    }
}
