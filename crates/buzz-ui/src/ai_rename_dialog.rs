//! Review dialog for "AI → Identify & Rename Assets…":
//! - Renders thumbnails and sends them to a local VLM in batches of 8 with a JSON schema.
//! - Review table lists: checkbox, thumbnail, old name, new name (editable snake_case), category, view, tags.
//! - Applies all approved renames and tags in one undoable edit.
//!
//! # Why the thumbnails and the VLM calls both run on a thread of their own
//!
//! Rendering every symbol in a library and then querying a VLM about each
//! batch is easily seconds of work, and used to happen on the UI thread the
//! moment the dialog opened — the window did not repaint again until it was
//! done. Both now run on a background thread against an owned clone of the
//! scene, reporting progress into a shared slot the dialog reads every frame,
//! with `poll` requesting its own repaints so completion shows up even if the
//! mouse never moves again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crossbeam_channel::Receiver;
use egui::{Context, ScrollArea};

use buzz_ai::llm::Backend;
use buzz_ai::naming::{identify_all_assets, AssetThumbnailItem, RenameProposal};
use buzz_scene::{LayerKind, Object, Scene, Symbol};

/// What the background thread has gotten through, and what it is doing —
/// shared with the dialog so `poll` can move the progress bar and the status
/// line without the thread ever touching egui state.
type SharedProgress = Arc<Mutex<(usize, usize, String)>>;

/// Dialog state for AI Asset Identification & Renaming.
pub struct AiRenameState {
    pub open: bool,
    pub is_running: bool,
    pub status: String,
    pub progress: (usize, usize),
    pub proposals: Vec<RenameProposal>,
    pub search_query: String,
    rx: Option<Receiver<Result<Vec<RenameProposal>, String>>>,
    /// Read every frame while a job is running; `None` once it is not.
    shared: Option<SharedProgress>,
    /// Set by Cancel. There is no interruption point inside a VLM HTTP call,
    /// so the thread runs to completion regardless; this only says that
    /// nobody is going to read what it sends back — `poll` drops the result
    /// unread rather than applying a run the user already dismissed.
    cancelled: Option<Arc<AtomicBool>>,
}

impl Default for AiRenameState {
    fn default() -> Self {
        Self {
            open: false,
            is_running: false,
            status: String::new(),
            progress: (0, 0),
            proposals: Vec::new(),
            search_query: String::new(),
            rx: None,
            shared: None,
            cancelled: None,
        }
    }
}

impl std::fmt::Debug for AiRenameState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiRenameState")
            .field("open", &self.open)
            .field("is_running", &self.is_running)
            .field("proposals_count", &self.proposals.len())
            .finish()
    }
}

/// Action returned by the dialog when the user confirms or cancels.
pub enum AiRenameAction {
    None,
    Apply(Vec<RenameProposal>),
}

impl AiRenameState {
    /// Polls the background job: pulls the latest progress across, and
    /// collects the result once it arrives.
    pub fn poll(&mut self, ctx: &Context) {
        if let Some(shared) = &self.shared {
            let (done, total, phase) = {
                let guard = shared.lock().unwrap_or_else(|p| p.into_inner());
                guard.clone()
            };
            self.progress = (done, total);
            if !phase.is_empty() {
                self.status = phase;
            }
        }

        if let Some(rx) = &self.rx {
            if let Ok(res) = rx.try_recv() {
                self.rx = None;
                self.shared = None;
                self.is_running = false;
                let cancelled = self
                    .cancelled
                    .take()
                    .is_some_and(|c| c.load(Ordering::Relaxed));
                if !cancelled {
                    match res {
                        Ok(proposals) => {
                            self.status = format!("Successfully identified {} assets.", proposals.len());
                            self.proposals = proposals;
                        }
                        Err(e) => {
                            self.status = format!("VLM analysis failed: {e}");
                        }
                    }
                }
            }
        }

        if self.is_running {
            // A modal with only a spinner in it would otherwise sit still
            // until the next mouse move noticed the thread was done — ask for
            // a repaint explicitly so the bar and the completion both show up
            // on their own.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Launches identification on all symbols in the given scene library.
    pub fn start_for_scene(&mut self, scene: &Scene, backend: Backend, model_name: String) {
        if self.is_running {
            return;
        }

        let symbol_count = scene.library().iter().count();
        if symbol_count == 0 {
            self.status = "No symbols found in current document library.".to_string();
            return;
        }

        self.is_running = true;
        self.status = format!("Rendering {symbol_count} thumbnails\u{2026}");
        self.progress = (0, symbol_count);
        self.proposals.clear();

        let shared: SharedProgress = Arc::new(Mutex::new((0, symbol_count, String::new())));
        self.shared = Some(Arc::clone(&shared));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled = Some(Arc::clone(&cancelled));

        let (tx, rx) = crossbeam_channel::unbounded();
        self.rx = Some(rx);

        // An owned clone of the whole scene — pointer copies of its `Arc`s,
        // not the artwork — so each symbol's mini scene below still has the
        // full library to resolve a nested instance against.
        let scene = scene.clone();

        thread::spawn(move || {
            // One exporter for the whole batch: creating a `GpuContext` costs
            // real time, and the old per-symbol exporter paid it once per
            // symbol in a library that can hold hundreds.
            let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic).ok();

            let mut items = Vec::with_capacity(symbol_count);
            for (done, sym) in scene.library().iter().enumerate() {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let png_bytes = match exporter.as_mut() {
                    Some(exporter) => render_symbol_thumbnail(exporter, &scene, sym),
                    None => fallback_thumbnail_png(),
                };
                items.push(AssetThumbnailItem {
                    id: sym.id.0.to_string(),
                    old_name: sym.name.clone(),
                    png_bytes,
                });
                set_progress(
                    &shared,
                    done + 1,
                    symbol_count,
                    format!("Rendering thumbnails\u{2026} ({}/{symbol_count})", done + 1),
                );
            }

            let res = identify_all_assets(&backend, &model_name, &items, |processed, total| {
                set_progress(
                    &shared,
                    processed,
                    total,
                    format!("Analyzing with local VLM\u{2026} ({processed}/{total})"),
                );
            })
            .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
    }

    /// Renders the modal window UI.
    pub fn show(&mut self, ctx: &Context) -> AiRenameAction {
        if !self.open {
            return AiRenameAction::None;
        }

        self.poll(ctx);

        let mut action = AiRenameAction::None;
        let mut is_open = self.open;

        egui::Window::new("AI → Identify & Rename Assets")
            .open(&mut is_open)
            .default_size([720.0, 520.0])
            .resizable(true)
            .show(ctx, |ui| {
                action = self.body(ui);
            });

        // `body` closes it on Apply/Cancel; the title bar's × closes it too.
        self.open = is_open && self.open;
        action
    }

    /// The dialog's content, drawn into any `Ui` — the AI panel's
    /// **Identify & Rename** section draws exactly this (F7.0).
    pub fn body(&mut self, ui: &mut egui::Ui) -> AiRenameAction {
        let mut action = AiRenameAction::None;
        ui.heading("VLM Asset Identification & Review");
        ui.add_space(4.0);
        ui.label("Review auto-generated snake_case names, categories, and tags. Uncheck any you wish to keep unchanged.");
        ui.add_space(8.0);

        if self.is_running {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&self.status);
                if self.progress.1 > 0 {
                    ui.label(format!("{}/{}", self.progress.0, self.progress.1));
                }
                if ui.button("Cancel").clicked() {
                    if let Some(cancelled) = &self.cancelled {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                    self.rx = None;
                    self.shared = None;
                    self.cancelled = None;
                    self.is_running = false;
                    self.status = "Cancelled.".to_string();
                }
            });
            ui.add_space(8.0);
        } else if !self.status.is_empty() {
            ui.label(egui::RichText::new(&self.status).italics().weak());
            ui.add_space(8.0);
        }

        if self.proposals.is_empty() && !self.is_running {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label("No identification results loaded.");
                ui.add_space(10.0);
            });
        } else if !self.proposals.is_empty() {
            // Header controls
            ui.horizontal(|ui| {
                if ui.button("Select All").clicked() {
                    for p in &mut self.proposals {
                        p.selected = true;
                    }
                }
                if ui.button("Deselect All").clicked() {
                    for p in &mut self.proposals {
                        p.selected = false;
                    }
                }
                ui.separator();
                ui.label("Filter:");
                ui.text_edit_singleline(&mut self.search_query);
            });

            ui.add_space(8.0);
            ui.separator();

            // Scrollable table of proposals
            ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                egui::Grid::new("rename_proposals_grid")
                    .num_columns(6)
                    .spacing([12.0, 8.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Apply").strong());
                        ui.label(egui::RichText::new("Current Name").strong());
                        ui.label(egui::RichText::new("New Name (snake_case)").strong());
                        ui.label(egui::RichText::new("Category").strong());
                        ui.label(egui::RichText::new("View").strong());
                        ui.label(egui::RichText::new("Tags").strong());
                        ui.end_row();

                        for prop in &mut self.proposals {
                            if !self.search_query.is_empty() {
                                let q = self.search_query.to_lowercase();
                                if !prop.old_name.to_lowercase().contains(&q)
                                    && !prop.new_name.to_lowercase().contains(&q)
                                {
                                    continue;
                                }
                            }

                            ui.checkbox(&mut prop.selected, "");
                            ui.label(&prop.old_name);
                            ui.text_edit_singleline(&mut prop.new_name);
                            ui.label(prop.category.as_str());
                            ui.label(prop.view.as_str());

                            let tag_str = prop.tags.join(", ");
                            ui.label(tag_str);
                            ui.end_row();
                        }
                    });
            });

            ui.add_space(12.0);
            ui.separator();

            // Footer actions
            ui.horizontal(|ui| {
                let selected_count = self.proposals.iter().filter(|p| p.selected).count();
                let apply_btn = egui::Button::new(
                    egui::RichText::new(format!("Apply {} Renames as One Edit", selected_count)).strong(),
                );
                if ui.add_enabled(selected_count > 0, apply_btn).clicked() {
                    action = AiRenameAction::Apply(self.proposals.clone());
                    self.open = false;
                }

                if ui.button("Cancel").clicked() {
                    self.open = false;
                }
            });
        }
        action
    }
}

fn set_progress(shared: &SharedProgress, done: usize, total: usize, phase: String) {
    *shared.lock().unwrap_or_else(|p| p.into_inner()) = (done, total, phase);
}

/// How far past the symbol's own extent the thumbnail frames it — matches
/// the margin the Library and Assets panels' own thumbnails use, so this one
/// looks the same next to them.
const THUMBNAIL_MARGIN: f64 = 1.15;

/// Side length of the square stage a symbol is thumbnailed against.
const THUMBNAIL_STAGE: f64 = 256.0;

/// Renders a thumbnail PNG for one library symbol.
///
/// `base_scene` is cloned into a mini scene that keeps the *whole* library —
/// cheap, since symbols are `Arc`s — so an instance nested inside another
/// symbol still resolves. Only the scene's own stage layers are replaced,
/// with a single instance of `sym`.
///
/// The instance's transform is solved to fit the symbol's resolved bounds
/// into the stage with a margin, centred. Left at identity, as this used to
/// be, a symbol drawn any distance from the document origin renders blank —
/// the exporter draws exactly the stage rectangle, at 1:1, and an
/// unframed instance is wherever its artwork happens to sit.
pub fn render_symbol_thumbnail(exporter: &mut buzz_export::Exporter, base_scene: &Scene, sym: &Symbol) -> Vec<u8> {
    let mut mini_scene = base_scene.clone();
    // Whatever symbol the original document had open for editing is not
    // relevant here, and would otherwise redirect `add_layer`/`remove_layer`
    // at that symbol's own layers instead of the document's stage.
    mini_scene.edit_document();
    mini_scene.stage_mut().size = buzz_geom::Size::new(THUMBNAIL_STAGE, THUMBNAIL_STAGE);
    // **The document's camera must not frame the thumbnail** (F7 run,
    // 2026-09-26): bhatta2 has a camera, and every thumbnail came out blank
    // because it framed the film's shot instead of this 256² stage — so the
    // VLM named assets from their old names alone and SigLIP saw the same
    // empty picture for every candidate.
    mini_scene.camera_mut().enabled = false;

    for id in mini_scene.layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        mini_scene.remove_layer(id);
    }

    let layer = mini_scene.add_layer("Thumb", LayerKind::Normal);
    let probe_id = mini_scene.next_object_id();
    let mut inst = Object::instance_of(probe_id, sym.id);

    let raw_bounds = mini_scene.resolved_bounds(&inst);
    let center = raw_bounds.center();
    let half = THUMBNAIL_STAGE / 2.0;
    let scale = if raw_bounds.width() > 0.0 && raw_bounds.height() > 0.0 {
        (THUMBNAIL_STAGE / raw_bounds.width()).min(THUMBNAIL_STAGE / raw_bounds.height()) / THUMBNAIL_MARGIN
    } else {
        1.0
    };
    inst.transform = buzz_geom::Affine::translate(buzz_geom::Vec2::new(half, half))
        * buzz_geom::Affine::scale(scale)
        * buzz_geom::Affine::translate(buzz_geom::Vec2::new(-center.x, -center.y));

    mini_scene.add_object(layer, inst);

    let settings = buzz_export::ExportSettings::for_stage(&mini_scene);
    if let Ok(frame) = exporter.render(&mini_scene, 0, &settings) {
        if let Ok(png) = frame.to_png() {
            return png;
        }
    }

    fallback_thumbnail_png()
}

/// A minimal valid transparent PNG, for a symbol that could not be rendered —
/// no GPU exporter, or a render that failed. The VLM sees an empty picture
/// and falls back to keeping the symbol's old name, which is exactly what
/// should happen when there was nothing to look at.
fn fallback_thumbnail_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
        0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41,
        0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
        0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
        0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
        0x42, 0x60, 0x82,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{Layer, LayerId, ObjectId, ShapeData, SymbolId, SymbolKind};

    /// Direct unit test for the thumbnail framing fix: a symbol drawn far
    /// from the document origin must still land inside the thumbnail, not
    /// off in blank space the stage never covers.
    #[test]
    fn a_symbol_drawn_far_from_the_origin_still_frames_into_its_thumbnail() {
        let Ok(mut exporter) = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic) else {
            // No GPU available in this environment — nothing to measure.
            return;
        };

        let mut symbol = Symbol::new(SymbolId(1), "far_away", SymbolKind::Graphic);
        let path = buzz_geom::Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1);
        let square = Object::shape(ObjectId(1), ShapeData::filled(path, peniko::Color::BLACK))
            .with_transform(buzz_geom::Affine::translate(buzz_geom::Vec2::new(5000.0, 5000.0)));
        let mut layer = Layer::normal(LayerId(1), "Art");
        layer.push_object_at(0, std::sync::Arc::new(square));
        symbol.layers.push_front(layer);

        let mut scene = Scene::default();
        scene.library_mut().insert(symbol);
        // The document has a camera looking somewhere else entirely, zoomed
        // in — bhatta2 does; the thumbnail must ignore it.
        scene.camera_mut().enabled = true;
        let mut key = buzz_scene::CameraKey::new(0, buzz_geom::Point::new(-3000.0, -3000.0));
        key.zoom = 4.0;
        scene.camera_mut().set_key(key);
        let sym = scene.library().get(SymbolId(1)).expect("symbol exists").clone();

        let png = render_symbol_thumbnail(&mut exporter, &scene, &sym);
        let frame = image::load_from_memory(&png).expect("valid PNG").to_rgba8();

        let (w, h) = frame.dimensions();
        assert!(w > 1 && h > 1, "expected a real render, not the fallback pixel");

        let (cx, cy) = (w / 2, h / 2);
        let pixel = frame.get_pixel(cx, cy);
        assert!(
            pixel.0[3] > 0,
            "expected non-transparent pixels near the centre of the thumbnail, got alpha {}",
            pixel.0[3]
        );
    }
}
