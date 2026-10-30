//! **Window ▸ Storyboard** (`docs/STORYBOARD_PLAN.md` S3): the episode's board
//! in the app — a grid of panel pictures with their captions, the chosen
//! panel's fields beside it (action, dialogue, notes, duration, kind, who it
//! is on, look), up/down, add, split, delete, Regenerate, Open scene, and
//! Export sheets.
//!
//! Every edit is written to `storyboard.json` at once, so a board made again
//! later keeps it by the merge rule ([`crate::storyboard::merge`]). Making the
//! board, drawing pictures and writing the sheets run on one worker thread
//! that keeps the scenes it built ([`Workbench`]), so the panel never waits on
//! a render: the worker sends back small thumbnails and the panel shows what
//! has arrived.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::storyboard::{self, Board, CameraFrame, ExportOutcome, Look, LookChoice, PANEL_SIZE, Panel, StoryboardRequest, Workbench};

/// Thumbnail size the worker sends back.
const THUMB: (u32, u32) = (320, 180);

enum Job {
    /// Make the board (merged with the one on disk) and, with `render`, draw
    /// every panel.
    Open { req: StoryboardRequest, render: bool },
    /// Draw one panel as it is now.
    Picture { panel: Panel, req: StoryboardRequest },
    /// Sheets and PDF of the board as it is now.
    Export { board: Board, out: PathBuf },
    /// The pictures already on disk, as thumbnails.
    Thumbs { out: PathBuf, ids: Vec<String> },
}

enum Reply {
    Progress(String),
    Opened(Board, PathBuf),
    Picture {
        id: String,
        thumb: Option<egui::ColorImage>,
        look_line: String,
        framing: String,
        camera: Option<CameraFrame>,
    },
    Exported(ExportOutcome),
    Failed(String),
    /// One job finished.
    Idle,
}

/// One panel's picture, as the grid shows it.
struct Thumb {
    image: Arc<egui::ColorImage>,
    texture: Option<egui::TextureHandle>,
    look_line: String,
    framing: String,
}

struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Reply>,
}

/// What the panel asks of the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum StoryboardAction {
    /// Show this scene of the open document at this frame.
    OpenScene { scene: u32, frame: u32 },
}

/// The Storyboard panel's state.
pub struct StoryboardPanel {
    /// The episode folder: `script.txt`, and `casting.json` in it or in its
    /// `buzz` folder.
    pub episode: String,
    /// `4,5` or `1-3`; empty for every scene.
    pub scenes: String,
    /// Where the board lives; empty for `<casting folder>/storyboard`.
    pub out: String,
    /// A breakdown to use as it is (no model asked); empty to make one.
    pub breakdown: String,
    pub look: LookChoice,
    pub offline: bool,
    pub board: Option<Board>,
    pub board_path: Option<PathBuf>,
    pub selected: usize,
    pub status: String,
    pub last_export: Option<ExportOutcome>,
    thumbs: HashMap<String, Thumb>,
    worker: Option<Worker>,
    pending: usize,
    ctx: Arc<Mutex<Option<egui::Context>>>,
    /// The request the board was made with, for Regenerate.
    last_req: Option<StoryboardRequest>,
    errors: Vec<String>,
}

impl Default for StoryboardPanel {
    fn default() -> Self {
        Self {
            episode: String::new(),
            scenes: String::new(),
            out: String::new(),
            breakdown: String::new(),
            look: LookChoice::Auto,
            offline: false,
            board: None,
            board_path: None,
            selected: 0,
            status: String::new(),
            last_export: None,
            thumbs: HashMap::new(),
            worker: None,
            pending: 0,
            ctx: Arc::new(Mutex::new(None)),
            last_req: None,
            errors: Vec::new(),
        }
    }
}

fn parse_scenes(text: &str) -> Result<Option<Vec<u32>>, String> {
    let mut out = Vec::new();
    for part in text.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
        let bad = |_| format!("scenes: {part:?} is not a scene number");
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u32, u32) = (a.trim().parse().map_err(bad)?, b.trim().parse().map_err(bad)?);
            out.extend(a..=b);
        } else {
            out.push(part.parse().map_err(bad)?);
        }
    }
    Ok(if out.is_empty() { None } else { Some(out) })
}

fn thumb_of(img: &image::RgbaImage) -> egui::ColorImage {
    let small = image::imageops::resize(img, THUMB.0, THUMB.1, image::imageops::FilterType::Triangle);
    egui::ColorImage::from_rgba_unmultiplied([THUMB.0 as usize, THUMB.1 as usize], small.as_raw())
}

fn run_worker(rx: Receiver<Job>, tx: Sender<Reply>, ctx: Arc<Mutex<Option<egui::Context>>>) {
    let wake = |ctx: &Arc<Mutex<Option<egui::Context>>>| {
        if let Ok(c) = ctx.lock()
            && let Some(c) = c.as_ref()
        {
            c.request_repaint();
        }
    };
    let send = |r: Reply| {
        let _ = tx.send(r);
        wake(&ctx);
    };
    let mut bench: Option<Workbench> = None;
    let mut exporter: Option<buzz_export::Exporter> = None;
    while let Ok(job) = rx.recv() {
        let result: anyhow::Result<()> = (|| {
            let mut progress = |line: &str| send(Reply::Progress(line.to_string()));
            match job {
                Job::Open { req, render } => {
                    let mut b = Workbench::open(&req, &mut progress)?;
                    send(Reply::Opened(b.board.clone(), b.board_path.clone()));
                    if render {
                        let exp = match &mut exporter {
                            Some(e) => e,
                            None => exporter.insert(buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?),
                        };
                        let panels = b.board.panels.clone();
                        for (k, panel) in panels.iter().enumerate() {
                            if !b.numbers.contains(&panel.scene) {
                                continue;
                            }
                            let pic = b.picture(exp, panel, req.look, false)?;
                            let img = image::open(&pic.png)?.to_rgba8();
                            send(Reply::Progress(format!("panel {}/{}: {}", k + 1, panels.len(), panel.id)));
                            send(Reply::Picture {
                                id: panel.id.clone(),
                                thumb: Some(thumb_of(&img)),
                                look_line: pic.look_line,
                                framing: pic.framing,
                                camera: pic.camera,
                            });
                        }
                    }
                    bench = Some(b);
                }
                Job::Picture { panel, mut req } => {
                    let fits = bench.as_ref().is_some_and(|b| b.req.out == req.out && b.scene(panel.scene).is_some());
                    if !fits {
                        let mut wanted: Vec<u32> = bench.as_ref().filter(|b| b.req.out == req.out).map(|b| b.numbers.clone()).or(req.scenes.clone()).unwrap_or_default();
                        if !wanted.contains(&panel.scene) {
                            wanted.push(panel.scene);
                        }
                        req.scenes = Some(wanted);
                        bench = Some(Workbench::open_for_pictures(&req, &mut progress)?);
                    }
                    let exp = match &mut exporter {
                        Some(e) => e,
                        None => exporter.insert(buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?),
                    };
                    let b = bench.as_mut().expect("opened above");
                    let pic = b.picture(exp, &panel, LookChoice::Auto, false)?;
                    let img = image::open(&pic.png)?.to_rgba8();
                    send(Reply::Picture { id: panel.id.clone(), thumb: Some(thumb_of(&img)), look_line: pic.look_line, framing: pic.framing, camera: pic.camera });
                }
                Job::Export { board, out } => {
                    let exp = match &mut exporter {
                        Some(e) => e,
                        None => exporter.insert(buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?),
                    };
                    let outcome = storyboard::export_sheets(exp, &board, &out, PANEL_SIZE)?;
                    send(Reply::Exported(outcome));
                }
                Job::Thumbs { out, ids } => {
                    for id in ids {
                        let png = out.join("panels").join(format!("{id}.png"));
                        let Ok(img) = image::open(&png) else { continue };
                        let note: serde_json::Value = std::fs::read_to_string(png.with_extension("look.json"))
                            .ok()
                            .and_then(|t| serde_json::from_str(&t).ok())
                            .unwrap_or(serde_json::Value::Null);
                        send(Reply::Picture {
                            id,
                            thumb: Some(thumb_of(&img.to_rgba8())),
                            look_line: note["look_line"].as_str().unwrap_or("").to_string(),
                            framing: note["framing"].as_str().unwrap_or("").to_string(),
                            camera: None,
                        });
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            send(Reply::Failed(format!("{e:#}")));
        }
        send(Reply::Idle);
    }
}

impl StoryboardPanel {
    fn worker(&mut self) -> &Worker {
        if self.worker.is_none() {
            let (job_tx, job_rx) = crossbeam_channel::unbounded();
            let (reply_tx, reply_rx) = crossbeam_channel::unbounded();
            let ctx = self.ctx.clone();
            std::thread::Builder::new()
                .name("storyboard".into())
                .spawn(move || run_worker(job_rx, reply_tx, ctx))
                .expect("spawning the storyboard worker");
            self.worker = Some(Worker { tx: job_tx, rx: reply_rx });
        }
        self.worker.as_ref().expect("made above")
    }

    fn send(&mut self, job: Job) {
        let _ = self.worker().tx.send(job);
        self.pending += 1;
    }

    /// Is the worker on something?
    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    /// The request the panel's fields make.
    pub fn request(&self) -> Result<StoryboardRequest, String> {
        let dir = PathBuf::from(self.episode.trim());
        if self.episode.trim().is_empty() {
            return Err("choose the episode folder (with script.txt)".into());
        }
        let script = dir.join("script.txt");
        if !script.is_file() {
            return Err(format!("no script.txt in {}", dir.display()));
        }
        let casting = [dir.join("buzz").join("casting.json"), dir.join("casting.json")]
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| format!("no casting.json in {} or its buzz folder", dir.display()))?;
        let out = if self.out.trim().is_empty() {
            casting.parent().unwrap_or(&dir).join("storyboard")
        } else {
            PathBuf::from(self.out.trim())
        };
        let breakdown = Some(self.breakdown.trim()).filter(|b| !b.is_empty()).map(PathBuf::from);
        if let Some(b) = &breakdown
            && !b.is_file()
        {
            return Err(format!("no breakdown file at {}", b.display()));
        }
        Ok(StoryboardRequest {
            script,
            casting,
            out,
            scenes: parse_scenes(&self.scenes)?,
            look: self.look,
            offline: self.offline,
            assets: None,
            world: String::new(),
            size: PANEL_SIZE,
            breakdown,
        })
    }

    /// **Make**: the board, then a picture per panel, on the worker.
    pub fn make(&mut self) -> Result<(), String> {
        let req = self.request()?;
        self.out = req.out.display().to_string();
        self.last_req = Some(req.clone());
        self.errors.clear();
        self.status = "making the board\u{2026}".into();
        self.send(Job::Open { req, render: true });
        Ok(())
    }

    /// **Load** `storyboard.json` (the panel's out folder's, or `path`) and
    /// the pictures already drawn for it.
    pub fn load(&mut self, path: Option<&Path>) -> Result<(), String> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => {
                let req = self.request()?;
                req.out.join("storyboard.json")
            }
        };
        let board = Board::load(&path).map_err(|e| format!("{e:#}"))?;
        let out = path.parent().map(Path::to_path_buf).unwrap_or_default();
        self.out = out.display().to_string();
        let ids: Vec<String> = board.panels.iter().map(|p| p.id.clone()).collect();
        self.status = format!("{} panel(s) from {}", board.panels.len(), path.display());
        self.board = Some(board);
        self.board_path = Some(path);
        self.selected = 0;
        self.thumbs.clear();
        self.send(Job::Thumbs { out, ids });
        Ok(())
    }

    fn out_dir(&self) -> Option<PathBuf> {
        self.board_path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf)
    }

    /// Write the board as it is now.
    pub fn save(&mut self) -> Result<(), String> {
        let (Some(board), Some(path)) = (&self.board, &self.board_path) else {
            return Err("no board yet".into());
        };
        board.save(path).map_err(|e| format!("{e:#}"))
    }

    fn edited(&mut self) {
        if let Err(e) = self.save() {
            self.status = format!("could not save the board: {e}");
        }
    }

    /// Move panel `index` to `to`.
    pub fn move_panel(&mut self, index: usize, to: usize) -> Result<usize, String> {
        let board = self.board.as_mut().ok_or("no board yet")?;
        if index >= board.panels.len() {
            return Err(format!("no panel {index}"));
        }
        let to = to.min(board.panels.len() - 1);
        let p = board.panels.remove(index);
        board.panels.insert(to, p);
        self.selected = to;
        self.edited();
        Ok(to)
    }

    pub fn add_after(&mut self, index: usize) -> Result<usize, String> {
        let board = self.board.as_mut().ok_or("no board yet")?;
        let p = storyboard::new_panel_after(board, board.panels.get(index));
        let at = (index + 1).min(board.panels.len());
        board.panels.insert(at, p);
        self.selected = at;
        self.edited();
        Ok(at)
    }

    pub fn split(&mut self, index: usize) -> Result<usize, String> {
        let fps = crate::episode::BuildOptions::default().fps;
        let board = self.board.as_mut().ok_or("no board yet")?;
        let at = storyboard::split_panel(board, index, fps).ok_or(format!("no panel {index}"))?;
        self.selected = at;
        self.edited();
        Ok(at)
    }

    pub fn delete(&mut self, index: usize) -> Result<Panel, String> {
        let board = self.board.as_mut().ok_or("no board yet")?;
        let gone = storyboard::delete_panel(board, index).ok_or(format!("no panel {index}"))?;
        self.selected = self.selected.min(board.panels.len().saturating_sub(1));
        self.thumbs.remove(&gone.id);
        self.edited();
        Ok(gone)
    }

    /// Change a panel's fields and save.
    pub fn edit(&mut self, index: usize, change: impl FnOnce(&mut Panel)) -> Result<(), String> {
        let board = self.board.as_mut().ok_or("no board yet")?;
        let p = board.panels.get_mut(index).ok_or(format!("no panel {index}"))?;
        change(p);
        self.edited();
        Ok(())
    }

    /// **Regenerate** panel `index` as it is now (its look, kind, subject).
    pub fn regenerate(&mut self, index: usize) -> Result<(), String> {
        let panel = self.board.as_ref().and_then(|b| b.panels.get(index)).cloned().ok_or(format!("no panel {index}"))?;
        let mut req = match &self.last_req {
            Some(r) => r.clone(),
            None => self.request()?,
        };
        if let Some(out) = self.out_dir() {
            req.out = out;
        }
        self.last_req = Some(req.clone());
        self.status = format!("drawing {}\u{2026}", panel.id);
        self.send(Job::Picture { panel, req });
        Ok(())
    }

    /// **Export sheets**: PNG pages and one PDF of the board as it is now.
    pub fn export(&mut self) -> Result<(), String> {
        let board = self.board.clone().ok_or("no board yet")?;
        let out = self.out_dir().ok_or("no board yet")?;
        self.status = "writing the sheets\u{2026}".into();
        self.send(Job::Export { board, out });
        Ok(())
    }

    /// Take in what the worker has sent.
    pub fn poll(&mut self) {
        let Some(w) = &self.worker else { return };
        let replies: Vec<Reply> = w.rx.try_iter().collect();
        for r in replies {
            self.apply(r);
        }
    }

    /// Wait (up to `timeout`) until the worker is idle. `Err` with the
    /// worker's errors, or on a timeout.
    pub fn wait(&mut self, timeout: Duration) -> Result<(), String> {
        let end = Instant::now() + timeout;
        while self.pending > 0 {
            let Some(w) = &self.worker else { break };
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("still working after {} s: {}", timeout.as_secs(), self.status));
            }
            match w.rx.recv_timeout(left.min(Duration::from_millis(200))) {
                Ok(r) => self.apply(r),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        if self.errors.is_empty() { Ok(()) } else { Err(std::mem::take(&mut self.errors).join("; ")) }
    }

    fn apply(&mut self, reply: Reply) {
        match reply {
            Reply::Progress(line) => self.status = line,
            Reply::Opened(board, path) => {
                let keep: Vec<String> = board.panels.iter().map(|p| p.id.clone()).collect();
                self.thumbs.retain(|id, _| keep.contains(id));
                self.selected = self.selected.min(board.panels.len().saturating_sub(1));
                self.board = Some(board);
                self.board_path = Some(path);
            }
            Reply::Picture { id, thumb, look_line, framing, camera } => {
                if let Some(c) = camera
                    && framing.starts_with("framed")
                    && let Some(board) = &mut self.board
                    && let Some(p) = board.panels.iter_mut().find(|p| p.id == id)
                {
                    storyboard::adopt_camera(p, c);
                    let _ = self.save();
                }
                if let Some(t) = thumb {
                    self.thumbs.insert(id, Thumb { image: Arc::new(t), texture: None, look_line, framing });
                }
            }
            Reply::Exported(outcome) => {
                self.status = format!(
                    "{} page(s){}",
                    outcome.pages.len(),
                    outcome.pdf.as_ref().map(|p| format!(" and {}", p.display())).unwrap_or_default()
                );
                self.last_export = Some(outcome);
            }
            Reply::Failed(e) => {
                self.status = format!("failed: {e}");
                self.errors.push(e);
            }
            Reply::Idle => {
                self.pending = self.pending.saturating_sub(1);
                let working = ["panel ", "board:", "building", "drawing", "making", "breakdown"].iter().any(|w| self.status.starts_with(w));
                if self.pending == 0 && working {
                    if let Some(b) = &self.board {
                        self.status = format!("{} panel(s), {} drawn", b.panels.len(), b.panels.iter().filter(|p| self.thumbs.contains_key(&p.id)).count());
                    }
                }
            }
        }
    }

    /// Drop the textures (a snapshot harness draws into a fresh egui context
    /// each time; the pictures stay).
    pub fn forget_textures(&mut self) {
        for t in self.thumbs.values_mut() {
            t.texture = None;
        }
    }

    /// The look line and framing of a panel's last picture.
    pub fn picture_note(&self, id: &str) -> Option<(String, String)> {
        self.thumbs.get(id).map(|t| (t.look_line.clone(), t.framing.clone()))
    }

    /// Draw the panel.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<StoryboardAction> {
        if let Ok(mut c) = self.ctx.lock()
            && c.is_none()
        {
            *c = Some(ui.ctx().clone());
        }
        self.poll();
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("Episode");
            ui.add(egui::TextEdit::singleline(&mut self.episode).desired_width(220.0).hint_text("folder with script.txt"));
            ui.label("Scenes");
            ui.add(egui::TextEdit::singleline(&mut self.scenes).desired_width(60.0).hint_text("all"));
            ui.label("Board");
            ui.add(egui::TextEdit::singleline(&mut self.out).desired_width(220.0).hint_text("<casting folder>/storyboard"));
            ui.label("Breakdown");
            ui.add(egui::TextEdit::singleline(&mut self.breakdown).desired_width(160.0).hint_text("made by the model"));
            egui::ComboBox::from_id_salt("storyboard_look")
                .selected_text(match self.look {
                    LookChoice::Auto => "Look: auto",
                    LookChoice::A => "Look: A real",
                    LookChoice::B => "Look: B sketch",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.look, LookChoice::Auto, "auto (A when built)");
                    ui.selectable_value(&mut self.look, LookChoice::A, "A, real scene setup");
                    ui.selectable_value(&mut self.look, LookChoice::B, "B, AI sketch");
                });
            ui.checkbox(&mut self.offline, "No model").on_hover_text("The breakdown from the script's own rules");
            let busy = self.busy();
            if ui.add_enabled(!busy, egui::Button::new("Make")).clicked()
                && let Err(e) = self.make()
            {
                self.status = e;
            }
            if ui.add_enabled(!busy, egui::Button::new("Load")).clicked()
                && let Err(e) = self.load(None)
            {
                self.status = e;
            }
            if ui.add_enabled(!busy && self.board.is_some(), egui::Button::new("Export sheets")).clicked()
                && let Err(e) = self.export()
            {
                self.status = e;
            }
        });
        ui.horizontal(|ui| {
            if self.busy() {
                ui.spinner();
            }
            ui.label(egui::RichText::new(&self.status).small().weak());
        });
        ui.separator();
        if self.board.is_none() {
            ui.label("No board yet: choose the episode folder and Make, or Load a storyboard.json.");
            return None;
        }
        let editor_width = 360.0f32.min(ui.available_width() * 0.45);
        let full = ui.available_rect_before_wrap();
        let split = (full.right() - editor_width - 8.0).max(full.left() + 120.0);
        let grid_rect = egui::Rect::from_min_max(full.min, egui::pos2(split - 8.0, full.bottom()));
        let editor_rect = egui::Rect::from_min_max(egui::pos2(split + 8.0, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(grid_rect), |ui| {
            egui::ScrollArea::vertical().id_salt("storyboard_grid").auto_shrink([false, false]).show(ui, |ui| {
                self.grid(ui);
            });
        });
        ui.painter().vline(split, full.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        ui.scope_builder(egui::UiBuilder::new().max_rect(editor_rect), |ui| {
            egui::ScrollArea::vertical().id_salt("storyboard_editor").auto_shrink([false, false]).show(ui, |ui| {
                action = self.editor(ui);
            });
        });
        ui.advance_cursor_after_rect(full);
        action
    }

    fn grid(&mut self, ui: &mut egui::Ui) {
        let Some(board) = &self.board else { return };
        let card = egui::vec2(200.0, 112.5);
        let mut clicked = None;
        let ctx = ui.ctx().clone();
        let panels: Vec<(String, String, String)> = board
            .panels
            .iter()
            .map(|p| {
                let kind = if p.on.is_empty() { p.kind.to_uppercase() } else { format!("{} on {}", p.kind.to_uppercase(), p.on) };
                let line = p.dialogue.first().map(|(w, t)| format!("{w}: {t}")).unwrap_or_else(|| p.action.clone());
                (p.id.clone(), format!("Sc {} \u{00b7} Shot {} \u{00b7} {kind} \u{00b7} {:.1} s", p.scene, p.shot + 1, p.duration_s), line)
            })
            .collect();
        let per_row = (((ui.available_width() + 10.0) / (card.x + 8.0 + 2.0 + 10.0)).floor() as usize).max(1);
        for (row, chunk) in panels.chunks(per_row).enumerate() {
          ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
            for (k, (id, head, line)) in chunk.iter().enumerate() {
                let i = row * per_row + k;
                let selected = i == self.selected;
                let frame = egui::Frame::new()
                    .stroke(egui::Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { ui.visuals().selection.stroke.color } else { ui.visuals().widgets.noninteractive.bg_stroke.color }))
                    .inner_margin(4.0)
                    .corner_radius(4.0);
                let r = frame.show(ui, |ui| {
                    ui.set_width(card.x);
                    ui.vertical(|ui| {
                        let thumb = self.thumbs.get_mut(id);
                        let pic = match thumb {
                            Some(t) => {
                                let tex = t.texture.get_or_insert_with(|| ctx.load_texture(format!("storyboard-{id}"), (*t.image).clone(), Default::default()));
                                ui.add(egui::Image::new(&*tex).fit_to_exact_size(card).sense(egui::Sense::click()))
                            }
                            None => {
                                let (rect, resp) = ui.allocate_exact_size(card, egui::Sense::click());
                                ui.painter().rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "no picture yet", egui::FontId::proportional(11.0), ui.visuals().weak_text_color());
                                resp
                            }
                        };
                        ui.label(egui::RichText::new(format!("{}. {head}", i + 1)).small().strong());
                        ui.add(egui::Label::new(egui::RichText::new(line).small()).truncate());
                        pic.clicked()
                    })
                    .inner
                });
                if r.inner || r.response.interact(egui::Sense::click()).clicked() {
                    clicked = Some(i);
                }
            }
          });
          ui.add_space(6.0);
        }
        if let Some(i) = clicked {
            self.selected = i;
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui) -> Option<StoryboardAction> {
        let mut action = None;
        let Some(board) = &self.board else { return None };
        let n = board.panels.len();
        if n == 0 {
            ui.label("The board has no panels.");
            if ui.button("Add a panel").clicked() {
                let _ = self.add_after(0);
            }
            return None;
        }
        let i = self.selected.min(n - 1);
        let names: Vec<String> = {
            let mut v: Vec<String> = board.panels.iter().map(|p| p.on.clone()).filter(|o| !o.is_empty()).collect();
            for p in &board.panels {
                for (w, _) in &p.dialogue {
                    if !w.ends_with("(V.O.)") {
                        v.push(w.clone());
                    }
                }
            }
            v.sort();
            v.dedup();
            v
        };
        let mut p = board.panels[i].clone();
        let before = p.clone();
        ui.label(egui::RichText::new(format!("Panel {} of {n} \u{00b7} {}", i + 1, p.id)).strong());
        ui.label(egui::RichText::new(format!("Scene {} \u{00b7} shot {} \u{00b7} {} \u{00b7} frame {}", p.scene, p.shot + 1, p.beat, p.frame)).small().weak());
        if let Some(t) = self.thumbs.get(&p.id) {
            if !t.look_line.is_empty() {
                ui.label(egui::RichText::new(&t.look_line).small());
            }
            if !t.framing.is_empty() {
                ui.label(egui::RichText::new(format!("Framing: {}", t.framing)).small().weak());
            }
        }
        egui::Grid::new("storyboard_fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Kind");
            egui::ComboBox::from_id_salt("storyboard_kind").selected_text(p.kind.clone()).show_ui(ui, |ui| {
                for k in ["wide", "medium", "close"] {
                    ui.selectable_value(&mut p.kind, k.to_string(), k);
                }
            });
            ui.end_row();
            ui.label("On");
            egui::ComboBox::from_id_salt("storyboard_on").selected_text(if p.on.is_empty() { "nobody".to_string() } else { p.on.clone() }).show_ui(ui, |ui| {
                ui.selectable_value(&mut p.on, String::new(), "nobody");
                for name in &names {
                    ui.selectable_value(&mut p.on, name.clone(), name);
                }
            });
            ui.end_row();
            ui.label("Duration");
            ui.add(egui::DragValue::new(&mut p.duration_s).range(0.1..=600.0).speed(0.1).suffix(" s").max_decimals(1));
            ui.end_row();
            ui.label("Look");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut p.look, Look::Auto, "Auto");
                ui.selectable_value(&mut p.look, Look::Real, "A real");
                ui.selectable_value(&mut p.look, Look::Sketch, "B sketch");
            });
            ui.end_row();
        });
        ui.label("Action");
        ui.add(egui::TextEdit::multiline(&mut p.action).desired_rows(2).desired_width(f32::INFINITY));
        ui.label("Dialogue");
        let mut drop_line = None;
        for (k, (who, text)) in p.dialogue.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(who).desired_width(90.0));
                ui.add(egui::TextEdit::singleline(text).desired_width((ui.available_width() - 30.0).max(60.0)));
                if ui.small_button("x").on_hover_text("Remove the line").clicked() {
                    drop_line = Some(k);
                }
            });
        }
        if let Some(k) = drop_line {
            p.dialogue.remove(k);
        }
        if ui.small_button("+ line").clicked() {
            p.dialogue.push((p.on.clone(), String::new()));
        }
        ui.label("Notes");
        ui.add(egui::TextEdit::multiline(&mut p.notes).desired_rows(2).desired_width(f32::INFINITY));
        if p != before
            && let Some(b) = &mut self.board
        {
            b.panels[i] = p.clone();
            self.edited();
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(i > 0, egui::Button::new("Up")).clicked() {
                let _ = self.move_panel(i, i - 1);
            }
            if ui.add_enabled(i + 1 < n, egui::Button::new("Down")).clicked() {
                let _ = self.move_panel(i, i + 1);
            }
            if ui.button("Add after").clicked() {
                let _ = self.add_after(i);
            }
            if ui.button("Split").on_hover_text("Two panels, the lines and the time shared out").clicked() {
                let _ = self.split(i);
            }
            if ui.button("Delete").clicked() {
                let _ = self.delete(i);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.busy(), egui::Button::new("Regenerate")).on_hover_text("Draw this panel again as it is now").clicked()
                && let Err(e) = self.regenerate(i)
            {
                self.status = e;
            }
            if ui.button("Open scene").on_hover_text("Show this panel's scene and frame on the stage").clicked() {
                action = Some(StoryboardAction::OpenScene { scene: p.scene, frame: p.frame });
            }
        });
        action
    }
}
