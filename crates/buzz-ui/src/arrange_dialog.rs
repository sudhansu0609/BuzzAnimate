//! **Assets ▸ Arrange by type & style…** (AS2): the preview of the proposed
//! folder moves — thumbnail, name, old → new folder, a checkbox each and a
//! select-all per destination folder — with Apply and Undo arrange.
//!
//! The dialog raises intentions; the shell builds the index and plan in a
//! Tasks-panel task, applies the moves (`AssetLibrary::move_to_folder` plus a
//! move log) and undoes them from that log.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use buzz_ai::asset_index::ArrangeMove;
use egui::{Context, RichText, ScrollArea};

/// Pictures decoded per frame, so opening a big folder never stalls a frame.
const LOADS_PER_FRAME: usize = 12;
const PIC: f32 = 48.0;

/// What the dialog asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrangeAction {
    None,
    /// Build (or refresh) the index and the plan again.
    Refresh,
    /// Move these (the checked rows).
    Apply(Vec<ArrangeMove>),
    /// Put back everything the last arrange moved.
    Undo,
}

/// The dialog's state.
#[derive(Default)]
pub struct ArrangeDialogState {
    pub open: bool,
    /// The library root, where the thumbnails are.
    pub root: PathBuf,
    /// The plan being previewed.
    pub moves: Vec<ArrangeMove>,
    /// Work in progress (`Some(what)` while the task runs): the dialog shows
    /// it and disables Apply.
    pub busy: Option<String>,
    /// `(done, total)` of the work in progress.
    pub progress: (u64, u64),
    /// How many files the last arrange moved (0 = nothing to undo).
    pub undo_files: usize,
    /// A line under the heading: what the index is made of, or what went wrong.
    pub note: String,
    /// Assets the plan leaves alone because they are in `Cast/`.
    pub cast_kept: usize,
    collapsed: BTreeSet<String>,
    textures: HashMap<String, Option<egui::TextureHandle>>,
}

impl ArrangeDialogState {
    /// A new plan to show; every row starts checked.
    pub fn set_plan(&mut self, root: PathBuf, moves: Vec<ArrangeMove>, cast_kept: usize) {
        self.root = root;
        self.moves = moves;
        self.cast_kept = cast_kept;
        self.textures.clear();
        // Folders start folded: a whole library is hundreds of rows.
        self.collapsed = self.moves.iter().map(|m| m.to.clone()).collect();
    }

    fn texture(&mut self, ctx: &Context, rel: &str, budget: &mut usize) -> Option<egui::TextureHandle> {
        if rel.is_empty() {
            return None;
        }
        if let Some(t) = self.textures.get(rel) {
            return t.clone();
        }
        if *budget == 0 {
            ctx.request_repaint();
            return None;
        }
        *budget -= 1;
        let loaded = load_png(&self.root.join(rel)).map(|img| {
            let size = [img.width() as usize, img.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            ctx.load_texture(format!("arrange_{rel}"), color, Default::default())
        });
        self.textures.insert(rel.to_string(), loaded.clone());
        loaded
    }
}

fn load_png(path: &Path) -> Option<image::RgbaImage> {
    image::open(path).ok().map(|d| d.to_rgba8())
}

/// Draw the dialog. Returns what was asked for.
pub fn arrange_dialog(ctx: &Context, state: &mut ArrangeDialogState) -> ArrangeAction {
    if !state.open {
        return ArrangeAction::None;
    }
    let mut action = ArrangeAction::None;
    let mut open = state.open;
    egui::Window::new("Arrange Assets by Type & Style")
        .open(&mut open)
        .default_size([760.0, 560.0])
        .resizable(true)
        .show(ctx, |ui| {
            action = body(ui, state);
        });
    state.open = open;
    action
}

fn body(ui: &mut egui::Ui, state: &mut ArrangeDialogState) -> ArrangeAction {
    let mut action = ArrangeAction::None;
    ui.label(
        "Files every asset under a Type folder (Characters, Buildings/Houses, Nature/Trees…) and, \
         where a Type holds several looks, a Style folder inside it. Names do not change. \
         Cast and its folders are never moved.",
    );
    if !state.note.is_empty() {
        ui.label(RichText::new(&state.note).small().weak());
    }
    ui.add_space(4.0);

    if let Some(what) = &state.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(what);
            if state.progress.1 > 0 {
                ui.label(format!("{}/{}", state.progress.0, state.progress.1));
            }
        });
        ui.add_space(4.0);
    }

    let checked = state.moves.iter().filter(|m| m.selected).count();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} proposed moves, {checked} checked; {} Cast assets stay where they are",
                state.moves.len(),
                state.cast_kept
            ))
            .strong(),
        );
    });
    ui.horizontal(|ui| {
        if ui.button("Check all").clicked() {
            state.moves.iter_mut().for_each(|m| m.selected = true);
        }
        if ui.button("Uncheck all").clicked() {
            state.moves.iter_mut().for_each(|m| m.selected = false);
        }
        if ui
            .add_enabled(state.busy.is_none(), egui::Button::new("Refresh"))
            .on_hover_text("Index new and changed assets again, and plan again")
            .clicked()
        {
            action = ArrangeAction::Refresh;
        }
    });
    ui.separator();

    // Rows grouped by destination folder.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, m) in state.moves.iter().enumerate() {
        groups.entry(m.to.clone()).or_default().push(i);
    }
    let ctx = ui.ctx().clone();
    let mut budget = LOADS_PER_FRAME;
    ScrollArea::vertical().max_height(ui.available_height() - 48.0).auto_shrink([false, false]).show(ui, |ui| {
        for (folder, rows) in &groups {
            let all = rows.iter().all(|&i| state.moves[i].selected);
            let some = rows.iter().any(|&i| state.moves[i].selected);
            let folded = state.collapsed.contains(folder);
            ui.horizontal(|ui| {
                if ui.small_button(if folded { "\u{25B6}" } else { "\u{23F7}" }).clicked() {
                    if folded {
                        state.collapsed.remove(folder);
                    } else {
                        state.collapsed.insert(folder.clone());
                    }
                }
                let mut value = all;
                let label = format!("{folder}  ({} of {})", rows.iter().filter(|&&i| state.moves[i].selected).count(), rows.len());
                let response = ui.checkbox(&mut value, RichText::new(label).strong());
                if some && !all {
                    response.clone().on_hover_text("some checked");
                }
                if response.changed() {
                    for &i in rows {
                        state.moves[i].selected = value;
                    }
                }
            });
            if folded {
                continue;
            }
            ui.indent(folder.as_str(), |ui| {
                for &i in rows {
                    let thumb = state.moves[i].thumb.clone();
                    let texture = state.texture(&ctx, &thumb, &mut budget);
                    let m = &mut state.moves[i];
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut m.selected, "");
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(PIC, PIC), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, egui::Color32::WHITE);
                        if let Some(t) = &texture {
                            ui.painter().image(
                                t.id(),
                                rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                        ui.vertical(|ui| {
                            let mut name = RichText::new(&m.name).strong();
                            if m.guessed {
                                name = name.italics();
                            }
                            ui.label(name).on_hover_text(if m.guessed {
                                "type guessed from the name (the VLM did not see it)"
                            } else {
                                "type from the VLM"
                            });
                            let from = if m.from.is_empty() { "(top level)" } else { m.from.as_str() };
                            ui.label(RichText::new(format!("{from}  \u{2192}  {}", m.to)).small().weak());
                        });
                    });
                }
            });
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        let can_apply = state.busy.is_none() && checked > 0;
        if ui
            .add_enabled(can_apply, egui::Button::new(format!("Apply ({checked} moves)")))
            .on_hover_text("Move the checked assets; Undo arrange puts every one back")
            .clicked()
        {
            action = ArrangeAction::Apply(state.moves.iter().filter(|m| m.selected).cloned().collect());
        }
        let can_undo = state.busy.is_none() && state.undo_files > 0;
        let undo_label = if state.undo_files > 0 {
            format!("Undo arrange ({} files)", state.undo_files)
        } else {
            "Undo arrange".to_string()
        };
        if ui
            .add_enabled(can_undo, egui::Button::new(undo_label))
            .on_hover_text("Put back every file the last arrange moved")
            .clicked()
        {
            action = ArrangeAction::Undo;
        }
        if ui.button("Close").clicked() {
            state.open = false;
        }
    });
    action
}
