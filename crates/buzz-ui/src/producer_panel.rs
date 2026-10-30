//! **Window ▸ Producer** (ANIMATE_COPILOT_PLAN B4): the co-pilot's panel for a
//! produced episode's output folder. A tree of scenes -> shots (camera,
//! background thumbnail) -> lines (speaker, time, pose), each with the buttons
//! of the co-pilot actions.
//!
//! The panel only gathers a choice: it hands the app a [`ProducerRequest`]
//! (`action` + `args`, exactly the assistant tool of the same name), the app
//! runs it as a Task with Cancel (`buzz_app::copilot::run_tool`) and refreshes
//! the view and the stage when it is done. The tree itself is the JSON
//! `buzz_app::copilot::view` makes.

use std::collections::HashMap;
use std::path::PathBuf;

use crossbeam_channel::Receiver;
use egui::{Color32, RichText};
use serde_json::{Value, json};

use crate::casting_dialog::CastingPick;

/// One action to run: the tool name and its args (without `out`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProducerRequest {
    pub action: String,
    pub args: Value,
}

/// What the user did.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ProducerAction {
    #[default]
    None,
    /// Run this action as a Task.
    Run(ProducerRequest),
    /// (Re)load the tree from the folder.
    Load,
}

/// What the panel is showing the actions of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Selection {
    #[default]
    None,
    Scene(u32),
    /// Scene, shot number.
    Shot(u32, usize),
    /// Scene, line item.
    Line(u32, u64),
}

/// The panel's state.
pub struct ProducerState {
    pub open: bool,
    /// The episode's output folder.
    pub out: String,
    /// `buzz_app::copilot::view`, once loaded.
    pub view: Option<Value>,
    /// An action is running (the buttons wait for it).
    pub busy: bool,
    pub status: Option<String>,
    /// The scene, shot or line whose actions are on show.
    pub selected: Selection,
    /// What the last action changed, in a sentence.
    pub last_change: Option<String>,
    /// Seconds a nudge button moves a line.
    pub nudge: f64,
    /// The creator's characters (the Cast step's catalogue), for "Play someone else".
    pub catalogue: Vec<CastingPick>,
    /// Scroll the detail pane to this offset (the snapshot example uses it).
    pub detail_scroll: Option<f32>,
    recast_search: String,
    prompts: HashMap<String, String>,
    poses: HashMap<String, String>,
    cut_line: HashMap<u32, usize>,
    cut_camera: HashMap<u32, String>,
    move_who: HashMap<u32, String>,
    move_xy: HashMap<u32, (f64, f64)>,
    recast_member: HashMap<u32, String>,
    recast_asset: HashMap<u32, String>,
    textures: HashMap<String, Option<egui::TextureHandle>>,
    picking: Option<Receiver<Option<PathBuf>>>,
}

impl Default for ProducerState {
    fn default() -> Self {
        Self {
            open: false,
            out: String::new(),
            view: None,
            busy: false,
            status: None,
            selected: Selection::None,
            last_change: None,
            nudge: 0.1,
            catalogue: Vec::new(),
            detail_scroll: None,
            recast_search: String::new(),
            prompts: HashMap::new(),
            poses: HashMap::new(),
            cut_line: HashMap::new(),
            cut_camera: HashMap::new(),
            move_who: HashMap::new(),
            move_xy: HashMap::new(),
            recast_member: HashMap::new(),
            recast_asset: HashMap::new(),
            textures: HashMap::new(),
            picking: None,
        }
    }
}

impl ProducerState {
    /// New tree after a load or a finished action; thumbnails are read again.
    pub fn set_view(&mut self, view: Value) {
        self.view = Some(view);
        self.textures.clear();
        self.prompts.clear();
    }

    /// Pre-set the "Move a character" choice (who, and where on the stage, 0 to 1).
    pub fn preset_move(&mut self, scene: u32, who: &str, x: f64, y: f64) {
        self.move_who.insert(scene, who.to_string());
        self.move_xy.insert(scene, (x, y));
    }

    /// Pre-set the "Play someone else" choice (the person, and one of the creator's characters).
    pub fn preset_recast(&mut self, scene: u32, member: &str, asset: &str) {
        self.recast_member.insert(scene, member.to_string());
        self.recast_asset.insert(scene, asset.to_string());
    }

    fn poll(&mut self) {
        let Some(rx) = &self.picking else { return };
        let Ok(picked) = rx.try_recv() else { return };
        self.picking = None;
        if let Some(p) = picked {
            self.out = p.display().to_string();
        }
    }
}

const THUMB: f32 = 96.0;

fn thumb(ctx: &egui::Context, state: &mut ProducerState, path: &str, loaded: &mut u32) -> Option<egui::TextureHandle> {
    thumb_sized(ctx, state, path, loaded, THUMB as u32 * 2)
}

/// A picture of up to `px` pixels on its longer side (big ones are kept under their own key).
fn thumb_sized(ctx: &egui::Context, state: &mut ProducerState, path: &str, loaded: &mut u32, px: u32) -> Option<egui::TextureHandle> {
    let key = if px > THUMB as u32 * 2 { format!("{path}#{px}") } else { path.to_string() };
    if let Some(t) = state.textures.get(&key) {
        return t.clone();
    }
    if *loaded >= 8 {
        ctx.request_repaint();
        return None;
    }
    *loaded += 1;
    let texture = image::open(path).ok().map(|img| {
        let small = img.thumbnail(px, px).to_rgba8();
        let size = [small.width() as usize, small.height() as usize];
        ctx.load_texture(format!("producer-{path}"), egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw()), Default::default())
    });
    state.textures.insert(key, texture.clone());
    texture
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, current: &str, options: &[String]) -> Option<String> {
    let mut chosen = None;
    egui::ComboBox::from_id_salt(id).selected_text(if current.is_empty() { "-" } else { current }).width(110.0).show_ui(ui, |ui| {
        for o in options {
            if ui.selectable_label(o == current, o).clicked() && o != current {
                chosen = Some(o.clone());
            }
        }
    });
    chosen
}

/// Draw the window. The returned action is for the app to carry out.
pub fn producer_panel(ctx: &egui::Context, state: &mut ProducerState) -> ProducerAction {
    if !state.open {
        return ProducerAction::None;
    }
    state.poll();
    let mut action = ProducerAction::None;
    let mut open = true;
    egui::Window::new("Producer").open(&mut open).default_size([760.0, 640.0]).resizable(true).show(ctx, |ui| {
        panel_body(ui, state, &mut action);
    });
    if !open {
        state.open = false;
    }
    action
}

fn panel_body(ui: &mut egui::Ui, state: &mut ProducerState, action: &mut ProducerAction) {
    ui.horizontal(|ui| {
        ui.label("Episode folder");
        ui.add(egui::TextEdit::singleline(&mut state.out).desired_width(380.0)).on_hover_text("The folder a produced episode was made in");
        if ui.add_enabled(state.picking.is_none(), egui::Button::new("Browse\u{2026}")).clicked() {
            let (tx, rx) = crossbeam_channel::bounded(1);
            state.picking = Some(rx);
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send(rfd::FileDialog::new().pick_folder());
                ctx.request_repaint();
            });
        }
        if ui.add_enabled(!state.out.trim().is_empty() && !state.busy, egui::Button::new("Load")).on_disabled_hover_text("Choose the episode folder first").clicked() {
            *action = ProducerAction::Load;
        }
        ui.label("nudge");
        ui.add(egui::DragValue::new(&mut state.nudge).speed(0.01).range(0.01..=2.0).suffix(" s")).on_hover_text("How far the start / end buttons move a line");
    });
    if let Some(s) = &state.status {
        ui.label(RichText::new(s.as_str()).weak());
    }
    if state.busy {
        ui.label("Working: see the Tasks panel (Cancel is there).");
    }
    ui.separator();
    let Some(view) = state.view.clone() else {
        ui.label("Choose the folder a produced episode was made in, then Load. Click a scene, a shot or a line to change it; only the scene it touches is rebuilt.");
        return;
    };
    if state.selected == Selection::None {
        if let Some(n) = view["scenes"].as_array().and_then(|a| a.first()).and_then(|s| s["number"].as_u64()) {
            state.selected = Selection::Scene(n as u32);
        }
    }
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(300.0);
            ui.label(RichText::new("Pick something to change").weak());
            egui::ScrollArea::vertical().id_salt("producer-tree").max_height(520.0).auto_shrink([false, false]).show(ui, |ui| tree(ui, state, &view));
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_min_width(430.0);
            let mut area = egui::ScrollArea::vertical().id_salt("producer-detail").max_height(560.0).auto_shrink([false, false]);
            if let Some(off) = state.detail_scroll {
                area = area.vertical_scroll_offset(off);
            }
            area.show(ui, |ui| detail(ui, state, action, &view));
        });
    });
}

fn tree(ui: &mut egui::Ui, state: &mut ProducerState, view: &Value) {
    for scene in view["scenes"].as_array().cloned().unwrap_or_default() {
        let n = scene["number"].as_u64().unwrap_or(0) as u32;
        let dirty = scene["dirty"].as_bool().unwrap_or(false);
        let title = format!("Scene {n} \u{2014} {}{}", str_of(&scene, "location"), if dirty { "  \u{2022} changed" } else { "" });
        let open_now = matches!(state.selected, Selection::Scene(s) | Selection::Shot(s, _) | Selection::Line(s, _) if s == n);
        let r = egui::CollapsingHeader::new(title).id_salt(("producer-scene", n)).default_open(open_now).show(ui, |ui| {
            for shot in scene["shots"].as_array().cloned().unwrap_or_default() {
                let i = shot["n"].as_u64().unwrap_or(1) as usize;
                let label = format!("Shot {i}  \u{00B7}  {}  \u{00B7}  from line {}", str_of(&shot, "camera"), shot["at_line"].as_u64().unwrap_or(0));
                if ui.selectable_label(state.selected == Selection::Shot(n, i), label).clicked() {
                    state.selected = Selection::Shot(n, i);
                }
            }
            for (li, line) in scene["lines"].as_array().cloned().unwrap_or_default().into_iter().enumerate() {
                let item = line["item"].as_u64().unwrap_or(0);
                let text: String = str_of(&line, "text").chars().take(34).collect();
                let label = format!("{}: {text}", str_of(&line, "speaker"));
                if ui.selectable_label(state.selected == Selection::Line(n, item), label).on_hover_text(format!("Scene {n}, line {}", li + 1)).clicked() {
                    state.selected = Selection::Line(n, item);
                }
            }
        });
        if r.header_response.clicked() {
            state.selected = Selection::Scene(n);
        }
    }
    let log = strings(view, "overrides");
    if !log.is_empty() {
        ui.separator();
        egui::CollapsingHeader::new(format!("Everything changed so far ({})", log.len())).id_salt("producer-overrides").show(ui, |ui| {
            for l in log.iter().rev().take(60) {
                ui.label(format!("\u{2022} {l}"));
            }
        });
    }
}

fn run(state: &ProducerState, action: &mut ProducerAction, name: &str, args: Value) {
    if !state.busy {
        *action = ProducerAction::Run(ProducerRequest { action: name.into(), args });
    }
}

/// A button that says why it is off: `blocker` is the reason (or `None` when it may be pressed).
fn act_button(ui: &mut egui::Ui, state: &ProducerState, label: &str, tip: &str, blocker: Option<&str>) -> bool {
    let reason = if state.busy { Some("Another change is still running (see the Tasks panel).") } else { blocker };
    let r = ui.add_enabled(reason.is_none(), egui::Button::new(label));
    match reason {
        Some(why) => r.on_disabled_hover_text(why).clicked(),
        None => r.on_hover_text(tip).clicked(),
    }
}

/// "standing (default)": the first pose on offer, the character's ordinary stance.
const STANDING: &str = "standing (default)";

/// The poses on offer for a line's speaker: standing first, then what the view lists.
fn pose_options(line: &Value) -> Vec<String> {
    let mut v = vec![STANDING.to_string()];
    for p in strings(line, "poses").into_iter().chain(Some(str_of(line, "pose").to_string())) {
        if !p.trim().is_empty() && !v.iter().any(|x| x.eq_ignore_ascii_case(&p)) && !p.to_lowercase().starts_with("standing") {
            v.push(p);
        }
    }
    v
}

/// "Scene 2, shot 2 (reverse)".
fn shot_name(n: u32, i: usize, camera: &str) -> String {
    if camera.is_empty() { format!("Scene {n}, shot {i}") } else { format!("Scene {n}, shot {i} ({camera})") }
}

/// "Scene 2, line 1": the line's place among the scene's spoken lines.
fn line_name(scene: &Value, n: u32, item: u64) -> String {
    let at = scene["lines"].as_array().and_then(|a| a.iter().position(|l| l["item"].as_u64() == Some(item))).map_or(1, |p| p + 1);
    format!("Scene {n}, line {at}")
}

/// The scene's still preview: one of the episode's stills, else its first shot's background picture.
fn scene_picture(ui: &egui::Ui, state: &mut ProducerState, scene: &Value) -> Option<egui::TextureHandle> {
    let mut loaded = 0u32;
    let path = scene["still"]
        .as_str()
        .filter(|p| !p.is_empty() && std::path::Path::new(p).is_file())
        .map(String::from)
        .or_else(|| scene["shots"].as_array().and_then(|a| a.first()).and_then(|s| s["background_png"].as_str()).filter(|p| !p.is_empty()).map(String::from))?;
    thumb_sized(ui.ctx(), state, &path, &mut loaded, 800)
}

fn detail(ui: &mut egui::Ui, state: &mut ProducerState, action: &mut ProducerAction, view: &Value) {
    let cameras = strings(view, "cameras");
    let speakers = strings(view, "speakers");
    let scenes = view["scenes"].as_array().cloned().unwrap_or_default();
    let (n, sel) = match state.selected {
        Selection::None => return,
        Selection::Scene(n) | Selection::Shot(n, _) | Selection::Line(n, _) => (n, state.selected),
    };
    let Some(scene) = scenes.iter().find(|s| s["number"].as_u64() == Some(n as u64)).cloned() else {
        ui.label("That scene is not in the episode any more.");
        return;
    };
    if let Some(c) = &state.last_change {
        ui.colored_label(Color32::from_rgb(90, 190, 110), format!("\u{2714} Last change: {c}"));
        ui.add_space(4.0);
    }
    let location = str_of(&scene, "location").to_string();
    let mut loaded = 0u32;
    match sel {
        Selection::Shot(_, i) => {
            if let Some(shot) = scene["shots"].as_array().and_then(|a| a.iter().find(|s| s["n"].as_u64() == Some(i as u64))).cloned() {
                shot_pane(ui, state, action, &shot, n, i, &location, &cameras, &mut loaded);
            }
        }
        Selection::Line(_, item) => {
            if let Some(line) = scene["lines"].as_array().and_then(|a| a.iter().find(|l| l["item"].as_u64() == Some(item))).cloned() {
                line_pane(ui, state, action, &line, &scene, n, &speakers);
            }
        }
        _ => scene_pane(ui, state, action, &scene, n, &location, &cameras, &speakers),
    }
}

#[allow(clippy::too_many_arguments)]
fn scene_pane(ui: &mut egui::Ui, state: &mut ProducerState, action: &mut ProducerAction, scene: &Value, n: u32, location: &str, cameras: &[String], speakers: &[String]) {
    let dirty = scene["dirty"].as_bool().unwrap_or(false);
    ui.heading(format!("Scene {n}"));
    ui.label(RichText::new(location).weak());
    let picture = scene_picture(ui, state, scene);
    match &picture {
        Some(t) => {
            ui.add(egui::Image::new(t).max_width(300.0).maintain_aspect_ratio(true));
        }
        None => {
            ui.label(RichText::new("No picture of this scene yet.").weak());
        }
    }
    if dirty {
        ui.colored_label(Color32::from_rgb(230, 160, 40), "Changed but not rebuilt yet.");
    }
    if act_button(ui, state, "Rebuild this scene", "Make this scene again from the casting and your changes. Other scenes are left alone.", None) {
        run(state, action, "rebuild_scene", json!({"scene": n}));
    }
    ui.separator();
    ui.strong("Move a character");
    ui.horizontal(|ui| {
        let who = state.move_who.entry(n).or_default();
        if let Some(c) = combo(ui, ("producer-move", n), &who.clone(), speakers) {
            *who = c;
        }
        let xy = state.move_xy.entry(n).or_insert((0.5, 0.9));
        for (label, x) in [("Left", 0.2), ("Centre", 0.5), ("Right", 0.8), ("Door", 0.95)] {
            if ui.button(label).on_hover_text("Put the character here; click the picture to fine-tune").clicked() {
                xy.0 = x;
            }
        }
    });
    state.move_xy.entry(n).or_insert((0.5, 0.9));
    ui.label(RichText::new("Click on the picture where the character should stand (drag to adjust).").weak().small());
    let size = egui::vec2(410.0, 410.0 * 9.0 / 16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        match &picture {
            Some(t) => {
                egui::Image::new(t).fit_to_exact_size(size).paint_at(ui, rect);
            }
            None => {
                ui.painter().rect_filled(rect, 4.0, Color32::from_gray(40));
            }
        }
        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
    }
    let have_who = !state.move_who.get(&n).map_or(true, String::is_empty);
    if have_who && let Some(pos) = response.interact_pointer_pos().filter(|_| response.clicked() || response.dragged()) {
        let xy = state.move_xy.entry(n).or_insert((0.5, 0.9));
        xy.0 = (((pos.x - rect.left()) / rect.width()) as f64).clamp(0.0, 1.0);
        xy.1 = (((pos.y - rect.top()) / rect.height()) as f64).clamp(0.0, 1.0);
    }
    let who = state.move_who.get(&n).cloned().unwrap_or_default();
    let xy = state.move_xy[&n];
    if have_who {
        let at = egui::pos2(rect.left() + rect.width() * xy.0 as f32, rect.top() + rect.height() * xy.1 as f32);
        ui.painter().circle(at, 8.0, Color32::from_rgb(240, 150, 40), egui::Stroke::new(2.0, Color32::WHITE));
        ui.painter().text(at + egui::vec2(0.0, -12.0), egui::Align2::CENTER_BOTTOM, &who, egui::FontId::proportional(13.0), Color32::WHITE);
    }
    egui::CollapsingHeader::new(RichText::new("exact position").small()).id_salt(("producer-exact", n)).show(ui, |ui| {
        let xy = state.move_xy.entry(n).or_insert((0.5, 0.9));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut xy.0).speed(0.01).range(0.0..=1.0).prefix("across "));
            ui.add(egui::DragValue::new(&mut xy.1).speed(0.01).range(0.0..=1.0).prefix("down "));
        });
    });
    let xy = state.move_xy[&n];
    let blocker = who.is_empty().then_some("Choose which character to move.");
    if act_button(ui, state, "Move there", "Put the character at this spot in the first shot of the scene", blocker) {
        run(state, action, "move_character", json!({"scene": n, "shot": 1, "character": who, "x": xy.0, "y": xy.1}));
    }
    ui.separator();
    ui.strong("Play someone else");
    ui.horizontal(|ui| {
        let member = state.recast_member.entry(n).or_default();
        if let Some(c) = combo(ui, ("producer-recast", n), &member.clone(), speakers) {
            *member = c;
        }
        ui.add(egui::TextEdit::singleline(&mut state.recast_search).desired_width(180.0).hint_text("search your characters"));
    });
    let asset = state.recast_asset.entry(n).or_default().clone();
    let q = state.recast_search.trim().to_lowercase();
    let items: Vec<CastingPick> =
        state.catalogue.iter().filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q) || c.folder.to_lowercase().contains(&q)).take(150).cloned().collect();
    if items.is_empty() {
        ui.label(RichText::new(if state.catalogue.is_empty() { "None of your characters were found." } else { "No character matches that search." }).weak());
    }
    let mut picked: Option<String> = None;
    let mut loaded = 0u32;
    egui::ScrollArea::vertical().id_salt(("producer-recast-grid", n)).min_scrolled_height(230.0).max_height(230.0).auto_shrink([false, false]).show(ui, |ui| {
        for row in items.chunks(5) {
            ui.horizontal(|ui| {
                for c in row {
                    let selected = asset == c.name;
                    ui.vertical(|ui| {
                        ui.set_width(72.0);
                        let tex = c.thumb.as_ref().and_then(|p| thumb(ui.ctx(), state, &p.display().to_string(), &mut loaded));
                        let r = match tex {
                            Some(t) => ui.add(egui::Button::image(egui::Image::new(&t).fit_to_exact_size(egui::vec2(64.0, 64.0))).selected(selected)),
                            None => ui.add_sized([64.0, 64.0], egui::Button::new("?").selected(selected)),
                        };
                        if r.on_hover_text(c.name.as_str()).clicked() {
                            picked = Some(c.name.clone());
                        }
                        let short: String = c.name.chars().take(10).collect();
                        ui.label(RichText::new(short).small());
                    });
                }
            });
        }
    });
    if let Some(name) = picked {
        state.recast_asset.insert(n, name);
    }
    let (member, asset) = (state.recast_member[&n].clone(), state.recast_asset[&n].clone());
    if !asset.is_empty() {
        ui.label(format!("Chosen: {asset}"));
    }
    let blocker = if member.is_empty() {
        Some("Choose the person to recast.")
    } else if asset.trim().is_empty() {
        Some("Click one of your characters above to play them.")
    } else {
        None
    };
    ui.horizontal(|ui| {
        if act_button(ui, state, "In this scene", "Only this scene uses the new character", blocker) {
            run(state, action, "recast", json!({"member": member, "asset": asset, "scope": "scene", "scene": n}));
        }
        if act_button(ui, state, "In the whole episode", "Every scene uses the new character", blocker) {
            run(state, action, "recast", json!({"member": member, "asset": asset, "scope": "episode"}));
        }
    });
    ui.separator();
    ui.strong("Add a camera cut");
    ui.horizontal(|ui| {
        ui.label("at line");
        let line = state.cut_line.entry(n).or_insert(1);
        ui.add(egui::DragValue::new(line).range(0..=999));
        let cam = state.cut_camera.entry(n).or_insert_with(|| "close".into());
        if let Some(c) = combo(ui, ("producer-addcut", n), &cam.clone(), cameras) {
            *cam = c;
        }
    });
    let (line, cam) = (state.cut_line[&n], state.cut_camera[&n].clone());
    if act_button(ui, state, "Add the cut", "Switch to this camera from that line on", None) {
        run(state, action, "add_cut", json!({"scene": n, "at_line": line, "camera": cam}));
    }
}

#[allow(clippy::too_many_arguments)]
fn shot_pane(ui: &mut egui::Ui, state: &mut ProducerState, action: &mut ProducerAction, shot: &Value, n: u32, i: usize, location: &str, cameras: &[String], loaded: &mut u32) {
    let camera = str_of(shot, "camera").to_string();
    let key = str_of(shot, "background_key").to_string();
    let angle = if key == location { "wide".to_string() } else { camera.clone() };
    ui.heading(shot_name(n, i, &camera));
    let png = shot["background_png"].as_str().unwrap_or_default().to_string();
    match (!png.is_empty()).then(|| thumb(ui.ctx(), state, &png, loaded)).flatten() {
        Some(t) => {
            ui.add(egui::Image::new(&t).max_height(150.0).maintain_aspect_ratio(true));
        }
        None => {
            ui.add_sized([200.0, 112.0], egui::Label::new("no picture yet"));
        }
    }
    ui.horizontal(|ui| {
        ui.label("Camera");
        if let Some(c) = combo(ui, ("producer-camera", n, i), &camera, cameras) {
            run(state, action, "set_shot_camera", json!({"scene": n, "shot": i, "camera": c}));
        }
        if act_button(ui, state, "Remove this cut", "Merge this shot into the one before it", None) {
            run(state, action, "remove_cut", json!({"scene": n, "shot": i}));
        }
    });
    ui.separator();
    ui.strong("The background picture");
    ui.horizontal(|ui| {
        if act_button(ui, state, "Draw it again", "Draws this background again with a new seed", None) {
            run(state, action, "redo_background", json!({"location": location, "angle": angle}));
        }
        let wide = angle == "wide";
        let blocker = wide.then_some("This is the wide picture itself. Only the other angles are redrawn from it.");
        if act_button(ui, state, "Redraw from this angle", "Re-angles the wide picture (needs the Qwen image-edit models in ComfyUI; you are told if they are missing)", blocker) {
            run(state, action, "reangle_background", json!({"location": location, "angle": angle}));
        }
    });
    let pkey = format!("{n}/{i}/{key}");
    let text = state.prompts.entry(pkey.clone()).or_insert_with(|| str_of(shot, "prompt").to_string());
    ui.add(egui::TextEdit::multiline(text).desired_width(410.0).desired_rows(3).hint_text("what the background should show"));
    let edited = state.prompts.get(&pkey).cloned().unwrap_or_default();
    let blocker = edited.trim().is_empty().then_some("Write what the background should show first.");
    if act_button(ui, state, "Use this description", "Draw the background again from this text", blocker) {
        run(state, action, "set_background_prompt", json!({"location": location, "angle": angle, "text": edited}));
    }
    let tries = strings(shot, "tries");
    if !tries.is_empty() {
        ui.add_space(4.0);
        ui.label("Earlier tries (click one to use it)");
        ui.horizontal_wrapped(|ui| {
            for (t, path) in tries.iter().enumerate() {
                if let Some(tex) = thumb(ui.ctx(), state, path, loaded) {
                    let r = ui.add(egui::Button::image(egui::Image::new(&tex).max_height(50.0).maintain_aspect_ratio(true)));
                    if r.on_hover_text(format!("Use try {}", t + 1)).clicked() {
                        run(state, action, "pick_background_try", json!({"location": location, "angle": angle, "n": t + 1}));
                    }
                }
            }
        });
    }
}

fn line_pane(ui: &mut egui::Ui, state: &mut ProducerState, action: &mut ProducerAction, line: &Value, scene: &Value, n: u32, speakers: &[String]) {
    let item = line["item"].as_u64().unwrap_or(0);
    let id = format!("{n}:{item}");
    let speaker = str_of(line, "speaker").to_string();
    ui.heading(line_name(scene, n, item));
    let text = str_of(line, "text");
    if !text.is_empty() {
        ui.label(RichText::new(text).italics());
    }
    ui.horizontal(|ui| {
        ui.label("Spoken by");
        if let Some(c) = combo(ui, ("producer-speaker", n, item), &speaker, speakers) {
            run(state, action, "set_speaker", json!({"line": id, "member": c}));
        }
    });
    ui.separator();
    ui.strong("When it is said");
    let nudge = state.nudge;
    let timed = matches!((line["start"].as_f64(), line["end"].as_f64()), (Some(_), Some(_)));
    if let (Some(a), Some(b)) = (line["start"].as_f64(), line["end"].as_f64()) {
        ui.label(format!("{a:.2} s to {b:.2} s"));
    } else {
        ui.label(RichText::new("Timed from the words (no recording times).").weak());
    }
    let blocker = (!timed).then_some("This line has no recording time to move. Produce with the audio to time it.");
    ui.horizontal(|ui| {
        for (label, tip, sd, ed) in [
            ("Starts earlier", "Move the start earlier", -nudge, 0.0),
            ("Starts later", "Move the start later", nudge, 0.0),
            ("Ends earlier", "Move the end earlier", 0.0, -nudge),
            ("Ends later", "Move the end later", 0.0, nudge),
        ] {
            if act_button(ui, state, label, tip, blocker) {
                run(state, action, "retime_line", json!({"line": id, "start_delta": sd, "end_delta": ed}));
            }
        }
    });
    ui.label(RichText::new(format!("Each click moves it {nudge:.2} s (change the nudge at the top).")).weak().small());
    ui.separator();
    ui.strong("Pose");
    let options = pose_options(line);
    let current = state.poses.entry(id.clone()).or_insert_with(|| options.first().cloned().unwrap_or_default());
    if !options.contains(current) {
        *current = options[0].clone();
    }
    egui::ComboBox::from_id_salt(("producer-pose", n, item)).selected_text(current.as_str()).width(200.0).show_ui(ui, |ui| {
        for o in &options {
            ui.selectable_value(current, o.clone(), o);
        }
    });
    let pose = state.poses[&id].clone();
    let pose = if pose == STANDING { "standing".to_string() } else { pose };
    if act_button(ui, state, "Set the pose", "The speaker strikes this pose on this line", None) {
        run(state, action, "set_pose", json!({"line": id, "character": speaker, "pose": pose}));
    }
    ui.separator();
    if act_button(ui, state, "Redo the mouth movement", "Rebuilds this line's scene so its mouth is cut again", None) {
        run(state, action, "redo_lipsync", json!({"line": id}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pose dropdown lists the character's variants, with "standing (default)" first.
    #[test]
    fn pose_dropdown_lists_variants_with_standing_first() {
        let line = json!({"pose": "sitting", "poses": ["Sit", "pointing", "standing"]});
        assert_eq!(pose_options(&line), vec![STANDING, "Sit", "pointing", "sitting"]);
        assert_eq!(pose_options(&json!({})), vec![STANDING]);
    }
}
