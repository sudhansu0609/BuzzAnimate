//! **Storyboards** (`docs/STORYBOARD_PLAN.md` S1–S3): the plan of an episode
//! as panels the user can see and edit before anything is animated.
//!
//! * **S1, the data.** [`Board`] / [`Panel`], saved as `storyboard.json` beside
//!   the casting file. Made from the breakdown's shots and the script's lines
//!   ([`board_for`]): one panel per camera shot, and an extra panel where a shot
//!   holds a big action change (an entrance, an exit, a gesture an action line
//!   asks for). Panels are keyed by scene + shot + line, and [`merge`] keeps the
//!   user's edits when the board is made again.
//! * **S2, the pictures.** Look A ([`render_real`]): the panel's scene, built
//!   headless the way `episode_build` builds it (same rigs, set and lights),
//!   at the panel's key moment, through the shot's camera. Look B
//!   ([`render_sketch`]): a painted storyboard frame from ComfyUI. A panel that
//!   asks for B when ComfyUI does not answer falls back to A and **says so on
//!   the sheet**; nothing is replaced silently.
//! * **S3, the sheet.** [`write_sheets`]: pages of six panels (3 × 2), a caption
//!   block under each, PNG per page, and one PDF of them ([`write_pdf`]).
//!
//! [`make`] does all of it for a script, a casting file and a list of scenes
//! (`examples/storyboard.rs`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use buzz_act::screenplay::{Dialogue, Item, Screenplay, ScriptScene, Voice};
use buzz_act::script_timing::{Schedule, Timing};
use buzz_ai::breakdown::{self, Breakdown, BreakdownHints, SceneBrief};
use buzz_doc::AssetLibrary;
use buzz_scene::{Object, Scene, ShapeData};
use kurbo::Shape;
use peniko::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::comfy::ComfyClient;
use crate::episode::{self, BuildOptions, Casting, SceneReport, ShotCut, ShotSpec, Shots};

/// A panel's picture size (S2): half of 1080p.
pub const PANEL_SIZE: (u32, u32) = (960, 540);

// ------------------------------------------------------------------ S1: the data

/// One episode's storyboard.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Board {
    pub title: String,
    pub panels: Vec<Panel>,
    /// Ids of made panels the user deleted: a re-run does not bring them back.
    pub removed: Vec<String>,
}

/// Which picture a panel gets.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    /// A: the real scene setup, rendered.
    Real,
    /// B: an AI sketch.
    Sketch,
    /// A when every character in the panel has a Cast rig and the set exists;
    /// B otherwise.
    #[default]
    Auto,
}

/// How the camera moves over a panel.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CameraMove {
    #[default]
    Hold,
    Pan,
    Push,
    Pull,
}

/// Where the camera looks: its centre on the stage and its zoom.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CameraFrame {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

/// A panel's camera.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PanelCamera {
    #[serde(rename = "move")]
    pub movement: CameraMove,
    pub from: Option<CameraFrame>,
    /// Where a push, pull or pan ends; `None` for a hold.
    pub to: Option<CameraFrame>,
}

/// One character's place in one panel, overriding the casting file's.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PlaceOverride {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub h: Option<f64>,
    /// `left`, `right` or `front`.
    pub facing: Option<String>,
}

/// One panel of the board.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Panel {
    /// `s<scene>-sh<shot>-l<line>-<beat>`: what a re-run matches on.
    pub id: String,
    /// The script's scene number.
    pub scene: u32,
    /// The camera shot it belongs to, from 0.
    pub shot: usize,
    /// Why the panel is there: `shot`, `entrance`, `exit` or `gesture`.
    pub beat: String,
    /// `wide`, `medium` or `close`.
    pub kind: String,
    /// The character a medium or close is on; empty for a wide.
    pub on: String,
    /// The first of the scene's dialogue lines (every cue, voice-over too) the
    /// panel covers, and one past its last: `from_line == to_line` has none.
    pub from_line: usize,
    pub to_line: usize,
    /// Where the panel starts in its scene, and the frame its picture shows.
    pub start_frame: u32,
    pub frame: u32,
    pub duration_s: f64,
    /// The script's action lines inside the panel.
    pub action: String,
    /// `(speaker, text)`; a voice-over speaker is written `NAME (V.O.)`.
    pub dialogue: Vec<(String, String)>,
    pub camera: PanelCamera,
    /// Per character, where they stand in this panel when not where the
    /// casting file puts them.
    pub places: BTreeMap<String, PlaceOverride>,
    /// A pose or gesture hint: `enters`, `Nod`, ….
    pub pose: String,
    pub look: Look,
    pub notes: String,
    /// The panel as it was made, for [`merge`]: a field that differs from this
    /// was edited by the user and is kept. `None` on a panel the user added.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated: Option<Value>,
}

impl Board {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| format!("{}: not a storyboard", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", path.display()))
    }
}

/// The fields of a panel as JSON, without its `generated` snapshot.
fn fields_of(panel: &Panel) -> serde_json::Map<String, Value> {
    let mut p = panel.clone();
    p.generated = None;
    match serde_json::to_value(&p) {
        Ok(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    }
}

/// Stamp each panel with its own snapshot, as made.
fn stamp(board: &mut Board) {
    for p in &mut board.panels {
        p.generated = Some(Value::Object(fields_of(p)));
    }
}

/// **Make the board again without losing the user's work.** `fresh` is the
/// board as made now; `old` the one on disk.
///
/// * A panel in both (same id): every field the user changed (differs from the
///   old panel's `generated` snapshot) keeps the user's value; every other
///   field takes the fresh one.
/// * A panel the user added (no snapshot) is kept, after the panel it followed.
/// * A made panel the user deleted (listed in `removed`) stays deleted.
pub fn merge(old: &Board, mut fresh: Board) -> Board {
    stamp(&mut fresh);
    let mut panels: Vec<Panel> = Vec::new();
    for mut p in fresh.panels {
        if old.removed.contains(&p.id) {
            continue;
        }
        if let Some(prev) = old.panels.iter().find(|o| o.id == p.id)
            && let Some(Value::Object(made)) = &prev.generated
        {
            let user = fields_of(prev);
            let mut now = fields_of(&p);
            for (key, value) in &user {
                if made.get(key) != Some(value) {
                    now.insert(key.clone(), value.clone());
                }
            }
            let snapshot = p.generated.take();
            if let Ok(mut merged) = serde_json::from_value::<Panel>(Value::Object(now)) {
                merged.generated = snapshot;
                p = merged;
            } else {
                p.generated = snapshot;
            }
        }
        panels.push(p);
    }
    // The user's own panels, after the one they followed (or first).
    for (i, added) in old.panels.iter().enumerate().filter(|(_, p)| p.generated.is_none()) {
        let after = old.panels[..i].iter().rev().find_map(|q| panels.iter().position(|p| p.id == q.id));
        let at = after.map_or(0, |a| a + 1);
        panels.insert(at.min(panels.len()), added.clone());
    }
    Board {
        title: if old.title.trim().is_empty() { fresh.title } else { old.title.clone() },
        panels,
        removed: old.removed.clone(),
    }
}

/// Upper-case, trimmed: how the casting file matches names.
fn norm(name: &str) -> String {
    name.trim().to_uppercase()
}

/// One scene's makings, for [`board_for`].
pub struct SceneInput<'a> {
    pub src: &'a ScriptScene,
    pub brief: Option<&'a SceneBrief>,
    pub report: &'a SceneReport,
}

/// A split inside a shot: where, why, and the hint for it.
struct Split {
    frame: u32,
    beat: &'static str,
    pose: String,
    /// The action item it comes from.
    item: usize,
}

/// **The board for some scenes** (S1), from the script, the built scenes'
/// shots (or the breakdown's, or one wide) and the timing the build used.
pub fn board_for(title: &str, scenes: &[SceneInput], opts: &BuildOptions) -> Board {
    let timing = Timing { fps: opts.fps, ..opts.timing };
    let fps = opts.fps.max(1.0);
    let mut panels = Vec::new();
    for input in scenes {
        let src = input.src;
        let schedule = timing.schedule(src);
        panels.extend(scene_panels(src, input.brief, input.report, &schedule, fps));
    }
    Board { title: title.to_string(), panels, removed: Vec::new() }
}

fn scene_panels(src: &ScriptScene, brief: Option<&SceneBrief>, report: &SceneReport, schedule: &Schedule, fps: f64) -> Vec<Panel> {
    let total = schedule.total_frames.max(1);
    // The dialogue lines: (item index, start, end).
    let lines: Vec<(usize, u32, u32, &Dialogue)> = src
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            Item::Dialogue(d) => schedule.slots.get(i).map(|s| (i, s.start, s.end(), d)),
            _ => None,
        })
        .collect();
    let line_at = |frame: u32| lines.iter().position(|(_, s, _, _)| *s >= frame).unwrap_or(lines.len());

    // The cuts: the build's, else the breakdown's, else one wide.
    let cuts: Vec<ShotCut> = if !report.shots.is_empty() {
        report.shots.clone()
    } else if let Some(b) = brief.filter(|b| !b.shots.is_empty()) {
        b.shots
            .iter()
            .map(|s| ShotCut {
                frame: if s.at_line == 0 { 0 } else { lines.get(s.at_line).map_or(0, |l| l.1) },
                kind: s.kind.clone(),
                on: Some(s.on.clone()).filter(|o| !o.is_empty()),
                ..Default::default()
            })
            .collect()
    } else {
        vec![ShotCut { frame: 0, kind: "wide".into(), ..Default::default() }]
    };

    // The action changes worth a panel of their own.
    let mut splits: Vec<Split> = Vec::new();
    for (i, item) in src.items.iter().enumerate() {
        let Item::Action(text) = item else { continue };
        let Some(slot) = schedule.slots.get(i) else { continue };
        let (beat, pose) = match episode::action_moves(text) {
            Some(true) => ("entrance", "enters".to_string()),
            Some(false) => ("exit", "exits".to_string()),
            None => match episode::gesture_for("", text) {
                Some((action, _)) => ("gesture", format!("{action:?}")),
                None => continue,
            },
        };
        splits.push(Split { frame: slot.start, beat, pose, item: i });
    }

    let min_gap = (2.0 * fps) as u32;
    let mut out: Vec<Panel> = Vec::new();
    for (shot, cut) in cuts.iter().enumerate() {
        let start = cut.frame.min(total - 1);
        let end = cuts.get(shot + 1).map_or(total, |c| c.frame).clamp(start + 1, total);
        // Sub-panels: the shot's start, then each split at least 2 s from the
        // last one and from the end. A split right at the start rides on it.
        let mut bounds: Vec<(u32, &'static str, String, Option<usize>)> = vec![(start, "shot", String::new(), None)];
        for s in splits.iter().filter(|s| s.frame >= start && s.frame < end) {
            let last = bounds.last().map_or(start, |b| b.0);
            if s.frame < last + min_gap {
                // Too close: the beat names the panel it falls in.
                if let Some(b) = bounds.last_mut()
                    && b.3.is_none()
                {
                    b.1 = s.beat;
                    b.2 = s.pose.clone();
                    b.3 = Some(s.item);
                }
                continue;
            }
            if end - s.frame < min_gap {
                continue;
            }
            bounds.push((s.frame, s.beat, s.pose.clone(), Some(s.item)));
        }
        for (k, (b_start, beat, pose, item)) in bounds.iter().enumerate() {
            let b_end = bounds.get(k + 1).map_or(end, |n| n.0);
            let from_line = line_at(*b_start);
            let to_line = line_at(b_end).max(from_line);
            let mine = &lines[from_line.min(lines.len())..to_line.min(lines.len())];
            let dialogue: Vec<(String, String)> = mine
                .iter()
                .map(|(_, _, _, d)| {
                    let who = if d.voice == Voice::VoiceOver { format!("{} (V.O.)", d.speaker) } else { d.speaker.clone() };
                    (who, d.text.clone())
                })
                .collect();
            let action: Vec<String> = src
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| match it {
                    Item::Action(t) => schedule.slots.get(i).filter(|s| s.start >= *b_start && s.start < b_end).map(|_| t.trim().to_string()),
                    _ => None,
                })
                .collect();
            // The key moment: the action for a beat, the middle of the lines
            // for a shot, the middle of the panel with neither.
            let frame = match (beat, item) {
                (&"gesture", _) => report
                    .gestures
                    .iter()
                    .map(|g| g.frame)
                    .find(|f| *f >= *b_start && *f < b_end)
                    .or_else(|| item.and_then(|i| schedule.slots.get(i)).map(|s| s.start + s.frames / 2)),
                (&"entrance" | &"exit", Some(i)) => schedule.slots.get(*i).map(|s| s.start + s.frames * 3 / 5),
                _ => None,
            }
            .or_else(|| match (mine.first(), mine.last()) {
                (Some(a), Some(z)) => Some((a.1 + z.2) / 2),
                _ => None,
            })
            .unwrap_or((b_start + b_end) / 2)
            .clamp(*b_start, b_end.saturating_sub(1).max(*b_start));
            let from = CameraFrame { x: cut.x, y: cut.y, zoom: if cut.scale > 0.0 { cut.scale } else { 1.0 } };
            let camera = match cut.push_to {
                Some(_) => PanelCamera {
                    movement: CameraMove::Push,
                    from: Some(from),
                    to: Some(CameraFrame { zoom: from.zoom * 1.03, ..from }),
                },
                None => PanelCamera { movement: CameraMove::Hold, from: Some(from), to: None },
            };
            let mut id = format!("s{}-sh{}-l{}-{}", src.number, shot, from_line, beat);
            while out.iter().any(|p| p.id == id) {
                id.push('b');
            }
            out.push(Panel {
                id,
                scene: src.number,
                shot,
                beat: beat.to_string(),
                kind: if cut.kind.is_empty() { "wide".into() } else { cut.kind.clone() },
                on: cut.on.clone().unwrap_or_default(),
                from_line,
                to_line,
                start_frame: *b_start,
                frame,
                duration_s: (b_end - b_start) as f64 / fps,
                action: action.join(" "),
                dialogue,
                camera,
                places: BTreeMap::new(),
                pose: pose.clone(),
                look: Look::Auto,
                notes: String::new(),
                generated: None,
            });
        }
    }
    out
}

// ------------------------------------------------------------------ S2: the pictures

/// The look a panel gets, and why, before ComfyUI is asked.
pub fn auto_look(panel: &Panel, report: &SceneReport, casting: &Casting) -> (Look, String) {
    if panel.look != Look::Auto {
        return (panel.look, "chosen on the panel".into());
    }
    if report.set.trim().is_empty() || report.skipped.iter().any(|s| s.starts_with("background")) {
        return (Look::Sketch, "the set is not built".into());
    }
    let voice_only = |name: &str| {
        let key = norm(name);
        casting
            .characters
            .iter()
            .find(|(k, e)| **k == key || e.aliases.iter().any(|a| norm(a) == key))
            .is_some_and(|(_, e)| e.voice_only)
    };
    let mut missing: Vec<String> = Vec::new();
    for c in &report.cast {
        if !c.played_by.starts_with("Cast") {
            missing.push(format!("{} ({})", c.name, c.played_by));
        }
    }
    let lower = panel.action.to_lowercase();
    for (who, _) in &panel.dialogue {
        if who.ends_with("(V.O.)") || voice_only(who) {
            continue;
        }
        if !report.cast.iter().any(|c| norm(&c.name) == norm(who)) && !missing.iter().any(|m| m.starts_with(&norm(who))) {
            missing.push(format!("{} (no Cast rig)", norm(who)));
        }
    }
    for s in &report.skipped {
        if let Some((name, _)) = s.split_once(':') {
            let n = name.trim();
            if !n.is_empty() && !voice_only(n) && lower.contains(&n.to_lowercase()) && !missing.iter().any(|m| m.starts_with(n)) {
                missing.push(format!("{n} (no Cast rig)"));
            }
        }
    }
    if missing.is_empty() {
        (Look::Real, "every character has a Cast rig and the set is built".into())
    } else {
        (Look::Sketch, format!("not built: {}", missing.join(", ")))
    }
}

/// How much of a standing figure a shot shows, from the top of the head
/// down: a medium to about the knees, a close the head and shoulders.
fn shows_of(kind: &str) -> Option<f64> {
    match kind.trim().to_lowercase().as_str() {
        "medium" => Some(0.74),
        "close" => Some(0.36),
        _ => None,
    }
}

/// The layers that draw cast member `on` in a built scene: the layer of its
/// body (an armature, or a layer rig's root part) and every layer that
/// follows that one, directly or not.
pub fn figure_layers(scene: &Scene, report: &SceneReport, on: &str) -> Vec<buzz_scene::LayerId> {
    let key = norm(on);
    let Some(body) = report.cast.iter().find(|c| norm(&c.name) == key).and_then(|c| c.body) else {
        return Vec::new();
    };
    let Some((layer, _)) = scene.find_object(body) else {
        return Vec::new();
    };
    let layers = scene.layers();
    let mut root = layer;
    for _ in 0..layers.len() {
        match layers.get(root).and_then(|l| l.follows) {
            Some(up) => root = up,
            None => break,
        }
    }
    let leads_to_root = |mut id: buzz_scene::LayerId| {
        for _ in 0..=layers.len() {
            if id == root {
                return true;
            }
            match layers.get(id).and_then(|l| l.follows) {
                Some(up) => id = up,
                None => return false,
            }
        }
        false
    };
    layers.iter().map(|l| l.id).filter(|id| leads_to_root(*id)).collect()
}

/// Where a figure is in a picture, measured from its own pixels: the
/// picture with the figure and without it, and what changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FigureBox {
    /// The bounding box of the figure's pixels, in picture pixels.
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    /// The mean x of the figure's pixels inside the band (a share of the box
    /// height from its top): the head and chest, not an arm held out.
    pub band_x: f64,
    pub pixels: usize,
}

/// The figure in `with` that `without` lacks: pixels that changed by more
/// than a little.
fn figure_box(with: &image::RgbaImage, without: &image::RgbaImage, band: (f64, f64)) -> Option<FigureBox> {
    let (w, h) = with.dimensions();
    if without.dimensions() != (w, h) {
        return None;
    }
    let mut mask = vec![false; (w * h) as usize];
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (u32::MAX, u32::MAX, 0u32, 0u32, 0usize);
    for y in 0..h {
        for x in 0..w {
            let a = with.get_pixel(x, y).0;
            let b = without.get_pixel(x, y).0;
            let d = (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0);
            // The figure's cast shadow darkens what is behind it evenly
            // (every channel by about the same share): not the figure.
            let ratio: Vec<f64> = (0..3).map(|c| (a[c] as f64 + 4.0) / (b[c] as f64 + 4.0)).collect();
            let (lo, hi) = ratio.iter().fold((f64::MAX, f64::MIN), |(l, h), r| (l.min(*r), h.max(*r)));
            let shadow = hi < 0.97 && hi - lo < 0.12;
            if d > 28 && !shadow {
                mask[(y * w + x) as usize] = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                n += 1;
            }
        }
    }
    // A few stray pixels (a shadow, antialiasing) are not a figure.
    if n < 40 {
        return None;
    }
    let bh = (y1 - y0 + 1) as f64;
    let (from, to) = ((y0 as f64 + band.0 * bh) as u32, (y0 as f64 + band.1 * bh).ceil() as u32);
    let (mut sum, mut count) = (0.0f64, 0usize);
    for y in from.max(y0)..=to.min(y1) {
        for x in x0..=x1 {
            if mask[(y * w + x) as usize] {
                sum += x as f64 + 0.5;
                count += 1;
            }
        }
    }
    let band_x = if count > 0 { sum / count as f64 } else { (x0 + x1 + 1) as f64 / 2.0 };
    Some(FigureBox { x0: x0 as f64, y0: y0 as f64, x1: x1 as f64 + 1.0, y1: y1 as f64 + 1.0, band_x, pixels: n })
}

fn render_rgba(exporter: &mut buzz_export::Exporter, scene: &Scene, frame: u32, size: (u32, u32)) -> Result<image::RgbaImage> {
    let mut settings = buzz_export::ExportSettings::for_stage(scene);
    settings.width = size.0;
    settings.height = size.1;
    let rendered = exporter.render(scene, frame, &settings)?;
    image::RgbaImage::from_raw(rendered.width, rendered.height, rendered.pixels).context("a rendered frame of the wrong size")
}

fn hidden(scene: &Scene, layers: &[buzz_scene::LayerId]) -> Scene {
    let mut s = scene.clone();
    for id in layers {
        s.update_layer(*id, |l| l.visible = false);
    }
    s
}

/// How well a framed panel shows its subject, measured on the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameCheck {
    /// How far the subject's centre (its head and chest) is from the middle
    /// of the picture, as a fraction of the picture's width; + is right.
    pub off_centre: f64,
    /// Space above the head, as a fraction of the picture's height.
    pub head_room: f64,
    /// Whether the figure runs off the bottom (it should, for a medium or close).
    pub cut_at_bottom: bool,
}

/// A look-A picture and the camera that made it.
pub struct Framed {
    pub image: image::RgbaImage,
    /// The camera the picture was taken through.
    pub camera: CameraFrame,
    /// What the framing did: `framed on GUDDU (head to knees)`, or why not.
    pub note: String,
    /// Measured after the render, when asked for.
    pub check: Option<FrameCheck>,
}

/// **Frame a medium or close on its subject** (S2): find `panel.on`'s figure
/// at the panel's frame from its own pixels (the whole stage rendered with the
/// figure and without it), then put the camera on it: centred on the head and
/// chest, the top of the head 8 % from the top, down to about the knees
/// (medium) or the shoulders (close). Kept inside the stage. `Err` with the
/// reason when the panel is a wide or the subject is not on stage then.
pub fn frame_subject(exporter: &mut buzz_export::Exporter, scene: &Scene, report: &SceneReport, panel: &Panel) -> Result<std::result::Result<CameraFrame, String>> {
    let Some(shows) = shows_of(&panel.kind) else {
        return Ok(Err(format!("{}: the whole stage", if panel.kind.is_empty() { "wide" } else { panel.kind.as_str() })));
    };
    if panel.on.trim().is_empty() {
        return Ok(Err(format!("{} on nobody: the shot's own camera", panel.kind)));
    }
    let layers = figure_layers(scene, report, &panel.on);
    if layers.is_empty() {
        return Ok(Err(format!("{} has no figure in the scene: the shot's own camera", panel.on)));
    }
    let stage = scene.stage().stage_rect();
    let probe = (640u32, ((640.0 * stage.height() / stage.width().max(1.0)).round() as u32).max(1));
    let mut open = scene.clone();
    open.camera_mut().enabled = false;
    let frame = panel.frame.min(scene.frame_count().saturating_sub(1));
    let with = render_rgba(exporter, &open, frame, probe)?;
    let without = render_rgba(exporter, &hidden(&open, &layers), frame, probe)?;
    let band = (0.0, (shows * 0.6).min(0.45));
    let Some(fig) = figure_box(&with, &without, band) else {
        return Ok(Err(format!("{} is not on stage at frame {}: the shot's own camera", panel.on, panel.frame)));
    };
    let k = stage.width() / probe.0 as f64;
    let (top, bottom) = (stage.y0 + fig.y0 * k, stage.y0 + fig.y1 * k);
    let height = (bottom - top).max(1.0);
    let zoom = (stage.height() / (shows * height / 0.90).max(1.0)).max(1.0);
    let frame_h = stage.height() / zoom;
    let frame_w = stage.width() / zoom;
    let cx = (stage.x0 + fig.band_x * k).clamp(stage.x0 + frame_w / 2.0, stage.x1 - frame_w / 2.0);
    let cy = (top - 0.08 * frame_h + frame_h / 2.0).clamp(stage.y0 + frame_h / 2.0, stage.y1 - frame_h / 2.0);
    Ok(Ok(CameraFrame { x: cx, y: cy, zoom }))
}

/// **Look A**: `scene` at the panel's key frame, captions hidden (the sheet
/// has its own), at `size`. A medium or close is framed on its subject
/// ([`frame_subject`]); a wide, or a shot whose subject is not there, goes
/// through the panel's camera (else the scene's). With `check`, the subject is
/// measured on the finished picture too (one more render).
pub fn render_real(exporter: &mut buzz_export::Exporter, scene: &Scene, report: Option<&SceneReport>, panel: &Panel, size: (u32, u32), check: bool) -> Result<Framed> {
    let mut scene = scene.clone();
    let subtitles: Vec<_> = scene.layers().iter().filter(|l| l.name == "Subtitles").map(|l| l.id).collect();
    for id in subtitles {
        scene.update_layer(id, |l| l.visible = false);
    }
    let frame = panel.frame.min(scene.frame_count().saturating_sub(1));
    let framed = match report {
        Some(r) => frame_subject(exporter, &scene, r, panel)?,
        None => Err("no build report: the shot's own camera".to_string()),
    };
    let (camera, note) = match framed {
        Ok(c) => {
            let what = if panel.kind.eq_ignore_ascii_case("close") { "head and shoulders" } else { "head to knees" };
            (Some(c), format!("framed on {} ({what})", panel.on))
        }
        Err(why) => (panel.camera.from, why),
    };
    if let Some(c) = camera {
        let cam = scene.camera_mut();
        cam.clear();
        cam.enabled = true;
        let mut key = buzz_scene::CameraKey::new(frame, buzz_geom::Point::new(c.x, c.y));
        key.zoom = if c.zoom > 0.0 { c.zoom } else { 1.0 };
        cam.set_key(key);
    }
    let image = render_rgba(exporter, &scene, frame, size)?;
    let used = camera.unwrap_or_else(|| {
        let stage = scene.stage().stage_rect();
        scene
            .camera()
            .state_at(frame)
            .map_or(CameraFrame { x: stage.center().x, y: stage.center().y, zoom: 1.0 }, |k| CameraFrame { x: k.center.x, y: k.center.y, zoom: k.zoom })
    });
    let mut out = Framed { image, camera: used, note, check: None };
    if check
        && let Some(r) = report
        && let Some(shows) = shows_of(&panel.kind)
    {
        let layers = figure_layers(&scene, r, &panel.on);
        if !layers.is_empty() {
            let without = render_rgba(exporter, &hidden(&scene, &layers), frame, size)?;
            // On the picture the box is cut at the bottom: the band is a
            // share of what shows.
            let band = ((shows * 0.6).min(0.45) / shows).min(1.0);
            if let Some(fig) = figure_box(&out.image, &without, (0.0, band)) {
                out.check = Some(FrameCheck {
                    off_centre: (fig.band_x - size.0 as f64 / 2.0) / size.0 as f64,
                    head_room: fig.y0 / size.1 as f64,
                    cut_at_bottom: fig.y1 >= size.1 as f64 - 1.0,
                });
            }
        }
    }
    Ok(out)
}

// The words for a look-B panel: the storyboard look of the user's reference
/// frame (`dataset2\storyboardChatth.png`: painted, flat gouache, soft ink
/// lines, warm muted colour), the scene's place, the shot, who is in it and
/// what happens.
pub fn sketch_prompt(panel: &Panel, brief: Option<&SceneBrief>, bd: Option<&Breakdown>) -> String {
    let mut who: Vec<String> = Vec::new();
    if let Some(b) = brief {
        for name in &b.present {
            let desc = bd
                .and_then(|bd| bd.characters.iter().find(|c| norm(&c.name) == norm(name)))
                .filter(|c| !c.voice_only)
                .map(|c| c.description.clone());
            if let Some(d) = desc {
                who.push(if d.trim().is_empty() { name.clone() } else { format!("{name}: {}", d.trim().trim_end_matches('.')) });
            }
        }
    }
    let place = brief.map(|b| b.background_prompt.trim().trim_end_matches('.').to_string()).unwrap_or_default();
    let shot = if panel.on.is_empty() { format!("{} shot", panel.kind) } else { format!("{} shot on {}", panel.kind, panel.on) };
    let world = bd.map(|b| b.world.trim().to_string()).filter(|w| !w.is_empty()).map(|w| format!(" {w}.")).unwrap_or_default();
    format!(
        "Storyboard concept frame for a 2D animated series, painted gouache illustration with soft ink outlines, \
         flat textured paper, warm muted colours, simple readable staging. {shot}. Setting: {place}. Characters: {}. \
         Action: {}.{world} No text, no captions, no speech bubbles, no panel border, no watermark.",
        if who.is_empty() { "none named".to_string() } else { who.join("; ") },
        if panel.action.trim().is_empty() { "they talk".to_string() } else { panel.action.trim().trim_end_matches('.').to_string() },
    )
}

/// **Look B**: the prompt through the Z-Image workflow ([`crate::backdrop::generate`],
/// cached by file), scaled to `size`.
pub fn render_sketch(client: &ComfyClient, prompt: &str, seed: u64, png: &Path, size: (u32, u32)) -> Result<image::RgbaImage, String> {
    let path = crate::backdrop::generate(client, prompt, seed, png)?;
    let picture = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
    Ok(image::imageops::resize(&picture, size.0, size.1, image::imageops::FilterType::Lanczos3))
}

/// A stable seed from a panel id.
fn seed_of(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)) & 0x7fff_ffff_ffff
}

// ------------------------------------------------------------------ S3: the sheet

/// One panel on a sheet: the panel, its picture and the look line under it.
pub struct SheetPanel<'a> {
    pub panel: &'a Panel,
    pub picture: PathBuf,
    /// `Look A — real scene setup`, or the fallback said out loud.
    pub look_line: String,
    /// Red when a look was asked for and not given.
    pub fell_back: bool,
}

const COLS: u32 = 3;
const ROWS: u32 = 2;
const MARGIN: f64 = 50.0;
const GAP: f64 = 40.0;
const HEADER: f64 = 110.0;
const CAPTION: f64 = 330.0;

/// The caption font: Nirmala UI (Latin and Devanagari both) when installed,
/// else the default the app's captions use.
fn caption_font() -> Option<String> {
    buzz_text::family("Nirmala UI").map(|f| f.name.clone())
}

/// Greedy word wrap to `width`, at most `max` rows, the last ending `…` when cut.
fn wrap(text: &str, size: f64, font: Option<&str>, style: buzz_text::FontStyle, width: f64, max: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    let fits = |s: &str| buzz_text::measure_styled(s, size, font, style).0 <= width;
    for word in text.split_whitespace() {
        let tried = if row.is_empty() { word.to_string() } else { format!("{row} {word}") };
        if fits(&tried) || row.is_empty() {
            row = tried;
        } else {
            rows.push(std::mem::take(&mut row));
            row = word.to_string();
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    if rows.len() > max {
        rows.truncate(max);
        if let Some(last) = rows.last_mut() {
            while !last.is_empty() && !fits(&format!("{last} …")) {
                last.pop();
            }
            last.push_str(" …");
        }
    }
    rows
}

/// Text and boxes for one page, drawn as a scene so the app's own shaping
/// (Devanagari included) sets the words.
struct PageDraw {
    scene: Scene,
    objects: Vec<Arc<Object>>,
    font: Option<String>,
}

impl PageDraw {
    fn new(width: f64, height: f64) -> Self {
        let mut scene = Scene::default();
        scene.stage_mut().size = buzz_geom::Size::new(width, height);
        scene.stage_mut().background = Color::from_rgb8(0xf4, 0xf1, 0xea);
        Self { scene, objects: Vec::new(), font: caption_font() }
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, colour: Color) {
        let path = kurbo::Rect::new(x, y, x + w, y + h).to_path(0.1);
        let id = self.scene.next_object_id();
        self.objects.push(Arc::new(Object::shape(id, ShapeData::filled(path, colour))));
    }

    /// One row of text with its baseline at `y`.
    fn text(&mut self, s: &str, x: f64, y: f64, size: f64, bold: bool, colour: Color) {
        let style = buzz_text::FontStyle::new(bold, false);
        let Some(path) = buzz_text::outline_styled(s, size, self.font.as_deref(), style, buzz_text::TextAlign::Left) else {
            return;
        };
        let id = self.scene.next_object_id();
        let mut o = Object::shape(id, ShapeData::filled(path, colour));
        o.transform = buzz_geom::Affine::translate((x, y));
        self.objects.push(Arc::new(o));
    }

    fn finish(mut self, exporter: &mut buzz_export::Exporter) -> Result<image::RgbaImage> {
        let layer = self.scene.add_stage_layer("Sheet", buzz_scene::LayerKind::Normal);
        let objects = std::mem::take(&mut self.objects);
        self.scene.update_layer(layer, |l| {
            l.frames.set_objects(0, objects);
        });
        let settings = buzz_export::ExportSettings::for_stage(&self.scene);
        let f = exporter.render(&self.scene, 0, &settings)?;
        image::RgbaImage::from_raw(f.width, f.height, f.pixels).context("a page of the wrong size")
    }
}

/// The page size for panels of `size`.
pub fn page_size(size: (u32, u32)) -> (u32, u32) {
    let (pw, ph) = (size.0 as f64, size.1 as f64);
    let w = MARGIN * 2.0 + pw * COLS as f64 + GAP * (COLS - 1) as f64;
    let h = HEADER + (ph + CAPTION) * ROWS as f64 + GAP * (ROWS - 1) as f64 + MARGIN;
    (w as u32, h as u32)
}

/// **The board sheet** (S3): pages of six panels with a caption block under
/// each, `board_p01.png`… in `dir`. Returns the pages.
pub fn write_sheets(exporter: &mut buzz_export::Exporter, title: &str, items: &[SheetPanel], dir: &Path, size: (u32, u32)) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let (w, h) = page_size(size);
    let (pw, ph) = (size.0 as f64, size.1 as f64);
    let per = (COLS * ROWS) as usize;
    let pages = items.len().div_ceil(per).max(1);
    let ink = Color::from_rgb8(0x1c, 0x1c, 0x22);
    let soft = Color::from_rgb8(0x55, 0x55, 0x60);
    let mut out = Vec::new();
    for (page, chunk) in items.chunks(per).enumerate() {
        let mut d = PageDraw::new(w as f64, h as f64);
        let scenes: Vec<String> = {
            let mut s: Vec<u32> = chunk.iter().map(|i| i.panel.scene).collect();
            s.dedup();
            s.iter().map(|n| n.to_string()).collect()
        };
        d.text(&format!("{title} \u{2014} storyboard"), MARGIN, 70.0, 44.0, true, ink);
        let right = format!("scene {} \u{00b7} page {} of {pages}", scenes.join(", "), page + 1);
        let rw = buzz_text::measure_styled(&right, 30.0, d.font.as_deref(), buzz_text::FontStyle::REGULAR).0;
        d.text(&right, w as f64 - MARGIN - rw, 70.0, 30.0, false, soft);
        for (k, item) in chunk.iter().enumerate() {
            let (c, r) = (k as u32 % COLS, k as u32 / COLS);
            let x = MARGIN + c as f64 * (pw + GAP);
            let y = HEADER + r as f64 * (ph + CAPTION + GAP);
            d.rect(x - 3.0, y - 3.0, pw + 6.0, ph + 6.0, ink);
            d.rect(x - 3.0, y + ph + 3.0, pw + 6.0, CAPTION - 12.0, Color::WHITE);
            caption(&mut d, item, x + 14.0, y + ph + 3.0, pw - 28.0, CAPTION - 12.0, ink, soft);
        }
        let mut sheet = d.finish(exporter)?;
        for (k, item) in chunk.iter().enumerate() {
            let (c, r) = (k as u32 % COLS, k as u32 / COLS);
            let x = (MARGIN + c as f64 * (pw + GAP)) as i64;
            let y = (HEADER + r as f64 * (ph + CAPTION + GAP)) as i64;
            let pic = image::open(&item.picture).with_context(|| format!("reading {}", item.picture.display()))?.to_rgba8();
            let pic = if pic.dimensions() == size { pic } else { image::imageops::resize(&pic, size.0, size.1, image::imageops::FilterType::Lanczos3) };
            image::imageops::overlay(&mut sheet, &pic, x, y);
        }
        let path = dir.join(format!("board_p{:02}.png", page + 1));
        sheet.save(&path).with_context(|| format!("writing {}", path.display()))?;
        out.push(path);
    }
    Ok(out)
}

/// The caption block under one panel: scene/shot and kind, the look, the
/// camera and duration, the action, then the dialogue as room allows.
#[allow(clippy::too_many_arguments)]
fn caption(d: &mut PageDraw, item: &SheetPanel, x: f64, top: f64, width: f64, height: f64, ink: Color, soft: Color) {
    let p = item.panel;
    let font = d.font.clone();
    let body = 23.0;
    let lh = body * 1.32;
    let mut y = top + 36.0;
    let bottom = top + height - 8.0;

    let kind = if p.on.is_empty() { p.kind.to_uppercase() } else { format!("{} on {}", p.kind.to_uppercase(), p.on) };
    let beat = if p.beat == "shot" { String::new() } else { format!(" \u{00b7} {}", p.beat.to_uppercase()) };
    d.text(&format!("Sc {} \u{00b7} Shot {}{beat} \u{00b7} {kind}", p.scene, p.shot + 1), x, y, 28.0, true, ink);
    let dur = format!("{:.1} s", p.duration_s);
    let dw = buzz_text::measure_styled(&dur, 28.0, font.as_deref(), buzz_text::FontStyle::new(true, false)).0;
    d.text(&dur, x + width - dw, y, 28.0, true, ink);
    y += 34.0;

    let look_colour = if item.fell_back {
        Color::from_rgb8(0xc0, 0x30, 0x20)
    } else if item.look_line.starts_with("Look B") {
        Color::from_rgb8(0x6a, 0x3c, 0xa8)
    } else {
        Color::from_rgb8(0x1f, 0x7a, 0x3c)
    };
    for row in wrap(&item.look_line, 21.0, font.as_deref(), buzz_text::FontStyle::new(true, false), width, 2) {
        d.text(&row, x, y, 21.0, true, look_colour);
        y += 26.0;
    }

    let cam = match (&p.camera.from, p.camera.movement) {
        (Some(f), CameraMove::Hold) => format!("Camera: hold, {:.0}% at ({:.0}, {:.0})", f.zoom * 100.0, f.x, f.y),
        (Some(f), m) => format!(
            "Camera: {} {:.0}% \u{2192} {:.0}%",
            format!("{m:?}").to_lowercase(),
            f.zoom * 100.0,
            p.camera.to.map_or(f.zoom, |t| t.zoom) * 100.0
        ),
        (None, m) => format!("Camera: {}", format!("{m:?}").to_lowercase()),
    };
    let lines = match p.to_line.saturating_sub(p.from_line) {
        0 => "no lines".to_string(),
        1 => format!("line {}", p.to_line),
        _ => format!("lines {}\u{2013}{}", p.from_line + 1, p.to_line),
    };
    let cam = format!("{cam} \u{00b7} frame {} \u{00b7} {lines}", p.frame);
    d.text(&cam, x, y, 20.0, false, soft);
    y += lh;

    if !p.action.trim().is_empty() {
        let rows = wrap(&format!("Action: {}", p.action.trim()), body, font.as_deref(), buzz_text::FontStyle::new(false, true), width, 2);
        for row in rows {
            d.text(&row, x, y, body, false, soft);
            y += lh;
        }
    }
    let room = ((bottom - y) / lh).floor().max(0.0) as usize;
    let mut rows: Vec<(String, bool)> = Vec::new();
    for (who, text) in &p.dialogue {
        for (i, row) in wrap(&format!("{who}: {text}"), body, font.as_deref(), buzz_text::FontStyle::REGULAR, width, 3).into_iter().enumerate() {
            rows.push((row, i == 0));
        }
    }
    let shown = if rows.len() > room { room.saturating_sub(1) } else { rows.len() };
    for (row, _) in rows.iter().take(shown) {
        d.text(row, x, y, body, false, ink);
        y += lh;
    }
    if shown < rows.len() {
        let more = p.dialogue.len().saturating_sub(rows.iter().take(shown).filter(|r| r.1).count());
        d.text(&format!("\u{2026} {more} more line(s) in the board"), x, y, 20.0, false, soft);
    }
}

/// **One PDF of the pages**: each page a JPEG image filling an A4-wide page.
/// `lopdf` writes it; the JPEG goes in as-is (`DCTDecode`).
pub fn write_pdf(pages: &[PathBuf], out: &Path) -> Result<()> {
    use lopdf::{Document, Object as PdfObject, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids: Vec<PdfObject> = Vec::new();
    for page in pages {
        let rgb = image::open(page).with_context(|| format!("reading {}", page.display()))?.to_rgb8();
        let (w, h) = rgb.dimensions();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90).encode_image(&rgb)?;
        let img = doc.add_object(
            Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => w as i64,
                    "Height" => h as i64,
                    "ColorSpace" => "DeviceRGB",
                    "BitsPerComponent" => 8,
                    "Filter" => "DCTDecode",
                },
                jpeg,
            )
            .with_compression(false),
        );
        let pw = 842.0f32;
        let ph = pw * h as f32 / w as f32;
        let content = doc.add_object(Stream::new(dictionary! {}, format!("q {pw} 0 0 {ph} 0 0 cm /Im0 Do Q").into_bytes()).with_compression(false));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), pw.into(), ph.into()],
            "Contents" => content,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => img } },
        });
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, PdfObject::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    doc.save(out).with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

// ------------------------------------------------------------------ the whole run

/// Which look the run gives every panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LookChoice {
    /// Each panel's own `look` (Auto by default).
    #[default]
    Auto,
    A,
    B,
}

/// What [`make`] is asked.
#[derive(Debug, Clone)]
pub struct StoryboardRequest {
    pub script: PathBuf,
    pub casting: PathBuf,
    /// `storyboard.json`, `breakdown.json`, `panels/`, `sketch/`, `board_p*.png`, `board.pdf`.
    pub out: PathBuf,
    pub scenes: Option<Vec<u32>>,
    pub look: LookChoice,
    /// No model for the breakdown: the script's own rules.
    pub offline: bool,
    pub assets: Option<PathBuf>,
    pub world: String,
    pub size: (u32, u32),
    /// A breakdown to use as it is (no model asked): the `breakdown.json` of
    /// an earlier run, or a bare breakdown.
    pub breakdown: Option<PathBuf>,
}

/// What [`make`] did.
#[derive(Debug, Clone, Default)]
pub struct StoryboardReport {
    pub board: PathBuf,
    pub breakdown_backend: String,
    /// Panels per scene number.
    pub panels: BTreeMap<u32, usize>,
    pub pages: Vec<PathBuf>,
    pub pdf: Option<PathBuf>,
    /// What happened to look B, per panel that asked for it.
    pub look_b: Vec<String>,
    /// How each look-A panel was framed, and what was measured on it.
    pub framing: Vec<String>,
    pub warnings: Vec<String>,
}

/// The chosen scenes and, for any that carries on from the scene before (no
/// slugline), the scenes back to one that has one: what the breakdown is run on.
fn breakdown_play(play: &Screenplay, numbers: &[u32]) -> Screenplay {
    let mut keep: Vec<u32> = Vec::new();
    for &n in numbers {
        let Some(at) = play.scenes.iter().position(|s| s.number == n) else { continue };
        let mut i = at;
        while i > 0 && play.scenes[i].slugline.is_none() {
            i -= 1;
        }
        keep.extend(play.scenes[i..=at].iter().map(|s| s.number));
    }
    Screenplay {
        title: play.title.clone(),
        scenes: play.scenes.iter().filter(|s| keep.contains(&s.number)).cloned().collect(),
    }
}

/// The breakdown as the producer makes it: LM Studio (or the saved Claude
/// model), else the script's own rules. Cached as `breakdown.json` in `out`.
fn breakdown_for(play: &Screenplay, hints: &BreakdownHints, offline: bool, out: &Path, progress: &mut dyn FnMut(&str)) -> Result<(Breakdown, String)> {
    let path = out.join("breakdown.json");
    let numbers: Vec<u32> = play.scenes.iter().map(|s| s.number).collect();
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(v) = serde_json::from_str::<Value>(&text)
        && let Ok(mut b) = serde_json::from_value::<Breakdown>(v["breakdown"].clone())
        && numbers.iter().all(|n| b.scenes.iter().any(|s| s.number == *n))
        && (v["offline"].as_bool() == Some(true)) == offline
    {
        breakdown::tidy(&mut b, play);
        let backend = v["backend"].as_str().unwrap_or("cached").to_string();
        progress(&format!("breakdown: kept {} ({backend})", path.display()));
        return Ok((b, backend));
    }
    let (b, backend) = if offline {
        (breakdown::breakdown_offline(play, hints), "offline (asked)".to_string())
    } else {
        let claude = buzz_ai::ai_config::use_claude();
        let llm = if claude { buzz_ai::llm::Backend::default_claude_api() } else { buzz_ai::llm::Backend::default_openai() };
        let model = if claude { Some((buzz_ai::ai_config::claude_model(), "the saved Claude model")) } else { crate::produce::lmstudio_model(&llm) };
        match model {
            Some((model, why)) if !model.is_empty() => {
                buzz_ai::llm::set_low_reasoning(true);
                progress(&format!("breakdown: asking {model} ({why}, {})", llm.display_name()));
                // The breakdown never fails: a batch the model got wrong keeps
                // its offline scenes. The backend says so rather than claim
                // the model's answer.
                let mut fell_back = Vec::new();
                let b = breakdown::breakdown(&llm, &model, play, hints, &mut |line| {
                    if line.contains("offline breakdown") || line.contains("no usable answer") {
                        fell_back.push(line.split(':').next().unwrap_or(line).trim().to_string());
                    }
                    progress(&format!("breakdown: {line}"))
                });
                let name = format!("{} {model}", llm.display_name());
                if fell_back.is_empty() {
                    (b, name)
                } else {
                    (b, format!("offline rules for {} ({name} did not answer)", fell_back.join(", ")))
                }
            }
            _ => {
                progress("breakdown: no model answers (LM Studio / Claude); the script's own rules");
                (breakdown::breakdown_offline(play, hints), "offline (no model answered)".to_string())
            }
        }
    };
    std::fs::write(&path, serde_json::to_string_pretty(&serde_json::json!({"backend": backend, "offline": offline, "breakdown": b}))?)?;
    Ok((b, backend))
}

/// A breakdown the caller already has (`--breakdown <path>`): the file
/// `breakdown_for` writes (`{backend, offline, breakdown}`) or a bare
/// breakdown. Nothing is asked of any model. Copied to `out/breakdown.json`.
fn breakdown_given(given: &Path, play: &Screenplay, out: &Path, progress: &mut dyn FnMut(&str)) -> Result<(Breakdown, String)> {
    let text = std::fs::read_to_string(given).with_context(|| format!("reading {}", given.display()))?;
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| format!("{}: not JSON", given.display()))?;
    let (inner, backend) = if v.get("breakdown").is_some() {
        (v["breakdown"].clone(), v["backend"].as_str().unwrap_or("given").to_string())
    } else {
        (v.clone(), "given".to_string())
    };
    let mut b: Breakdown = serde_json::from_value(inner).with_context(|| format!("{}: not a breakdown", given.display()))?;
    let missing: Vec<u32> = play.scenes.iter().map(|s| s.number).filter(|n| !b.scenes.iter().any(|s| s.number == *n)).collect();
    if !missing.is_empty() {
        bail!("{} has no scene(s) {missing:?}", given.display());
    }
    breakdown::tidy(&mut b, play);
    let backend = format!("{backend} (from {})", given.display());
    let path = out.join("breakdown.json");
    if path.canonicalize().ok() != given.canonicalize().ok() {
        std::fs::write(&path, serde_json::to_string_pretty(&serde_json::json!({"backend": backend, "offline": false, "breakdown": b}))?)?;
    }
    progress(&format!("breakdown: {backend}, no model asked"));
    Ok((b, backend))
}

/// One panel's picture, as [`Workbench::picture`] made it.
#[derive(Debug, Clone)]
pub struct PanelPicture {
    pub png: PathBuf,
    /// The look line the sheet prints.
    pub look_line: String,
    pub fell_back: bool,
    /// What look B did, when it was asked for.
    pub look_b: Option<String>,
    /// The camera a look-A picture was taken through.
    pub camera: Option<CameraFrame>,
    /// How the framing went (`framed on GUDDU (head to knees)`, or why not).
    pub framing: String,
    pub check: Option<FrameCheck>,
}

/// **A board ready to draw**: the breakdown, the scenes built headless with
/// the breakdown's shots, and the board merged with the one on disk (and
/// saved). [`make`] uses it for a whole run; the app's Storyboard panel keeps
/// one on its worker thread to redraw single panels.
pub struct Workbench {
    pub req: StoryboardRequest,
    pub casting: Casting,
    pub breakdown: Breakdown,
    pub breakdown_backend: String,
    /// The built scenes by number, and their reports.
    pub scenes: Vec<(u32, Scene)>,
    pub reports: Vec<SceneReport>,
    pub numbers: Vec<u32>,
    pub board: Board,
    pub board_path: PathBuf,
    pub warnings: Vec<String>,
    comfy: Option<Option<ComfyClient>>,
}

impl Workbench {
    /// Build the scenes and make the board, merged with the one on disk and
    /// saved.
    pub fn open(req: &StoryboardRequest, progress: &mut dyn FnMut(&str)) -> Result<Self> {
        Self::open_with(req, progress, true)
    }

    /// Build the scenes only, to draw panels of a board the caller holds:
    /// `storyboard.json` is read (or made, unsaved) but never written.
    pub fn open_for_pictures(req: &StoryboardRequest, progress: &mut dyn FnMut(&str)) -> Result<Self> {
        Self::open_with(req, progress, false)
    }

    fn open_with(req: &StoryboardRequest, progress: &mut dyn FnMut(&str), save_board: bool) -> Result<Self> {
        std::fs::create_dir_all(&req.out)?;
        let source = std::fs::read_to_string(&req.script).with_context(|| format!("reading {}", req.script.display()))?;
        let play = Screenplay::parse(&source);
        if play.scenes.is_empty() {
            bail!("{} has no scenes in it", req.script.display());
        }
        let casting_text = std::fs::read_to_string(&req.casting).with_context(|| format!("reading {}", req.casting.display()))?;
        let mut casting = Casting::from_json(&casting_text).map_err(anyhow::Error::msg)?;
        let numbers: Vec<u32> = req.scenes.clone().unwrap_or_else(|| play.scenes.iter().map(|s| s.number).collect());

        // The breakdown.
        let hints = BreakdownHints {
            style: "comic, light humour".into(),
            known_characters: casting.characters.values().filter_map(|c| c.cast.clone()).collect(),
            series: play.title.clone(),
            world: req.world.clone(),
            ..Default::default()
        };
        let part = breakdown_play(&play, &numbers);
        let (bd, backend) = match &req.breakdown {
            Some(given) => breakdown_given(given, &part, &req.out, progress)?,
            None => breakdown_for(&part, &hints, req.offline, &req.out, progress)?,
        };

        // The breakdown's shots into the casting, for the camera.
        for n in &numbers {
            if let Some(b) = bd.scenes.iter().find(|s| s.number == *n)
                && !b.shots.is_empty()
            {
                let mut set = casting.scene_set(*n);
                if set.shots.is_none() {
                    set.shots = Some(Shots::List(
                        b.shots
                            .iter()
                            .map(|s| ShotSpec { at_line: s.at_line, kind: s.kind.clone(), on: Some(s.on.clone()).filter(|o| !o.is_empty()), hold: None })
                            .collect(),
                    ));
                    casting.scenes.insert(n.to_string(), set);
                }
            }
        }

        // The scenes, built headless.
        let assets = match &req.assets {
            Some(dir) => AssetLibrary::at(dir),
            None => AssetLibrary::user(),
        };
        let opts = BuildOptions::default();
        progress(&format!("building scenes {numbers:?} headless from {} asset(s)", assets.len()));
        let (doc, reports) = episode::build_episode(&play, &casting, &assets, &numbers, &opts);
        let built: Vec<u32> = reports.iter().filter(|r| r.frames > 0).map(|r| r.number).collect();
        let scenes: Vec<(u32, Scene)> = built.iter().copied().zip(doc.film()).collect();
        let mut warnings = Vec::new();
        for r in &reports {
            warnings.extend(r.warnings.iter().map(|w| format!("scene {}: {w}", r.number)));
        }

        // S1: the board, merged with the user's.
        let inputs: Vec<SceneInput> = numbers
            .iter()
            .filter_map(|n| {
                Some(SceneInput { src: play.scene(*n)?, brief: bd.scenes.iter().find(|s| s.number == *n), report: reports.iter().find(|r| r.number == *n)? })
            })
            .collect();
        let title = if play.title.is_empty() { "Storyboard".to_string() } else { play.title.clone() };
        let fresh = board_for(&title, &inputs, &opts);
        let board_path = req.out.join("storyboard.json");
        let board = match Board::load(&board_path) {
            Ok(old) if board_path.is_file() && !save_board => old,
            Ok(old) if board_path.is_file() => merge(&old, fresh),
            _ => {
                let mut b = fresh;
                stamp(&mut b);
                b
            }
        };
        if save_board {
            board.save(&board_path)?;
        }
        progress(&format!("board: {} panel(s) in {}", board.panels.len(), board_path.display()));
        Ok(Self {
            req: req.clone(),
            casting,
            breakdown: bd,
            breakdown_backend: backend,
            scenes,
            reports,
            numbers,
            board,
            board_path,
            warnings,
            comfy: None,
        })
    }

    pub fn scene(&self, number: u32) -> Option<&Scene> {
        self.scenes.iter().find(|(n, _)| *n == number).map(|(_, s)| s)
    }

    pub fn report(&self, number: u32) -> Option<&SceneReport> {
        self.reports.iter().find(|r| r.number == number)
    }

    /// Where a panel's picture is kept.
    pub fn png_of(&self, panel: &Panel) -> PathBuf {
        self.req.out.join("panels").join(format!("{}.png", panel.id))
    }

    /// **Draw one panel** (S2) as `look` asks (Auto: the panel's own look,
    /// and A or B by [`auto_look`]), save it to `panels/<id>.png`. A look-B
    /// panel ComfyUI cannot draw falls back to A and says so. With `check` a
    /// framed panel is measured on its picture.
    pub fn picture(&mut self, exporter: &mut buzz_export::Exporter, panel: &Panel, look: LookChoice, check: bool) -> Result<PanelPicture> {
        let png = self.png_of(panel);
        if let Some(dir) = png.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let report = self.reports.iter().find(|r| r.number == panel.scene);
        let (wanted, why) = match (look, report) {
            (LookChoice::A, _) => (Look::Real, "asked for".to_string()),
            (LookChoice::B, _) => (Look::Sketch, "asked for".to_string()),
            (LookChoice::Auto, Some(r)) => auto_look(panel, r, &self.casting),
            (LookChoice::Auto, None) => (Look::Real, format!("scene {} was not built", panel.scene)),
        };
        let mut out = PanelPicture {
            png: png.clone(),
            look_line: format!("Look A \u{2014} real scene setup ({why})"),
            fell_back: false,
            look_b: None,
            camera: None,
            framing: String::new(),
            check: None,
        };
        let mut picture: Option<image::RgbaImage> = None;
        if wanted == Look::Sketch {
            let client = self.comfy.get_or_insert_with(ComfyClient::discover);
            match client {
                None => {
                    out.look_line = format!("Look A \u{2014} B asked ({why}), ComfyUI not reachable: real setup shown");
                    out.fell_back = true;
                    out.look_b = Some(format!("{}: B asked ({why}); ComfyUI not reachable on 127.0.0.1:8188\u{2013}8208; fell back to A, said on the sheet", panel.id));
                }
                Some(c) => {
                    let prompt = sketch_prompt(panel, self.breakdown.scenes.iter().find(|s| s.number == panel.scene), Some(&self.breakdown));
                    let sketch = self.req.out.join("sketch").join(format!("{}.png", panel.id));
                    match render_sketch(c, &prompt, seed_of(&panel.id), &sketch, self.req.size) {
                        Ok(img) => {
                            out.look_line = format!("Look B \u{2014} AI sketch ({why})");
                            out.look_b = Some(format!("{}: sketched by ComfyUI ({})", panel.id, sketch.display()));
                            picture = Some(img);
                        }
                        Err(e) => {
                            out.look_line = format!("Look A \u{2014} B asked, ComfyUI failed ({e}): real setup shown");
                            out.fell_back = true;
                            out.look_b = Some(format!("{}: ComfyUI failed: {e}; fell back to A, said on the sheet", panel.id));
                        }
                    }
                }
            }
        }
        let picture = match picture {
            Some(p) => p,
            None => match self.scene(panel.scene) {
                Some(scene) => {
                    let framed = render_real(exporter, scene, report, panel, self.req.size, check)?;
                    out.camera = Some(framed.camera);
                    out.framing = framed.note;
                    out.check = framed.check;
                    framed.image
                }
                None => {
                    self.warnings.push(format!("{}: scene {} was not built; no picture", panel.id, panel.scene));
                    image::RgbaImage::from_pixel(self.req.size.0, self.req.size.1, image::Rgba([60, 60, 60, 255]))
                }
            },
        };
        picture.save(&png).with_context(|| format!("writing {}", png.display()))?;
        let note = serde_json::json!({"look_line": out.look_line, "fell_back": out.fell_back, "framing": out.framing});
        let _ = std::fs::write(look_note_of(&png), serde_json::to_string_pretty(&note).unwrap_or_default());
        Ok(out)
    }

    /// The sheets and the PDF of `items` (S3).
    pub fn sheets(&self, exporter: &mut buzz_export::Exporter, board: &Board, items: &[(usize, PanelPicture)]) -> Result<(Vec<PathBuf>, Option<PathBuf>, Vec<String>)> {
        let sheet: Vec<SheetPanel> = items
            .iter()
            .filter(|(i, _)| *i < board.panels.len())
            .map(|(i, p)| SheetPanel { panel: &board.panels[*i], picture: p.png.clone(), look_line: p.look_line.clone(), fell_back: p.fell_back })
            .collect();
        let pages = write_sheets(exporter, &board.title, &sheet, &self.req.out, self.req.size)?;
        let pdf = self.req.out.join("board.pdf");
        match write_pdf(&pages, &pdf) {
            Ok(()) => Ok((pages, Some(pdf), Vec::new())),
            Err(e) => Ok((pages, None, vec![format!("PDF: {e}")])),
        }
    }
}

/// Beside `panels/<id>.png`: the look line the picture was made with.
fn look_note_of(png: &Path) -> PathBuf {
    png.with_extension("look.json")
}

/// What [`export_sheets`] wrote.
#[derive(Debug, Clone, Default)]
pub struct ExportOutcome {
    pub pages: Vec<PathBuf>,
    pub pdf: Option<PathBuf>,
    /// Panels left off the sheets (no picture yet), and a PDF that failed.
    pub warnings: Vec<String>,
}

/// **The sheets of a board as it is now** (S3), from the pictures already
/// drawn in `out/panels` (and the look each was drawn with): PNG pages and
/// one PDF in `out`. A panel with no picture yet is left off and named in
/// the warnings; nothing is drawn in its place.
pub fn export_sheets(exporter: &mut buzz_export::Exporter, board: &Board, out: &Path, size: (u32, u32)) -> Result<ExportOutcome> {
    let mut outcome = ExportOutcome::default();
    let mut items: Vec<SheetPanel> = Vec::new();
    for panel in &board.panels {
        let png = out.join("panels").join(format!("{}.png", panel.id));
        if !png.is_file() {
            outcome.warnings.push(format!("{}: no picture yet (Regenerate it); left off the sheets", panel.id));
            continue;
        }
        let note: Value = std::fs::read_to_string(look_note_of(&png)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let look_line = note["look_line"].as_str().map(String::from).unwrap_or_else(|| "Look A \u{2014} real scene setup".to_string());
        items.push(SheetPanel { panel, picture: png, look_line, fell_back: note["fell_back"].as_bool().unwrap_or(false) });
    }
    if items.is_empty() {
        bail!("no panel has a picture yet: make the board first");
    }
    // Last run's pages past the new count would read as part of this board.
    if let Ok(dir) = std::fs::read_dir(out) {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("board_p") && name.ends_with(".png") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    outcome.pages = write_sheets(exporter, &board.title, &items, out, size)?;
    let pdf = out.join("board.pdf");
    match write_pdf(&outcome.pages, &pdf) {
        Ok(()) => outcome.pdf = Some(pdf),
        Err(e) => outcome.warnings.push(format!("PDF: {e}")),
    }
    Ok(outcome)
}

/// A new panel the user adds after `after` (or first): its own id, a
/// two-second hold on the same moment, no snapshot (so a re-made board
/// keeps it after the panel it followed).
pub fn new_panel_after(board: &Board, after: Option<&Panel>) -> Panel {
    let mut n = 1;
    let id = loop {
        let id = format!("user-{n}");
        if !board.panels.iter().any(|p| p.id == id) {
            break id;
        }
        n += 1;
    };
    let mut p = after.cloned().unwrap_or_default();
    p.id = id;
    p.beat = "added".into();
    p.from_line = p.to_line;
    p.dialogue.clear();
    p.action.clear();
    p.notes.clear();
    p.pose.clear();
    p.duration_s = 2.0;
    p.start_frame = p.frame;
    p.generated = None;
    if p.kind.is_empty() {
        p.kind = "wide".into();
    }
    p
}

/// **Split a panel in two** at the middle of its time: the lines and the
/// duration are shared out, the first half keeps the id (and so the user's
/// edits), the second is a panel of the user's own after it. `fps` turns the
/// duration into frames for the second half's key moment.
pub fn split_panel(board: &mut Board, index: usize, fps: f64) -> Option<usize> {
    let p = board.panels.get(index)?.clone();
    let fps = fps.max(1.0);
    let mut second = new_panel_after(board, Some(&p));
    let half_lines = p.dialogue.len().div_ceil(2);
    let half_s = (p.duration_s / 2.0).max(0.1);
    let half_frames = (half_s * fps).round() as u32;
    let first = &mut board.panels[index];
    first.dialogue.truncate(half_lines);
    first.to_line = (p.from_line + half_lines).min(p.to_line.max(p.from_line));
    first.duration_s = half_s;
    first.frame = (p.start_frame + half_frames / 2).max(p.start_frame);
    second.beat = "split".into();
    second.dialogue = p.dialogue[half_lines.min(p.dialogue.len())..].to_vec();
    second.from_line = first.to_line;
    second.to_line = p.to_line.max(second.from_line);
    second.action = String::new();
    second.duration_s = (p.duration_s - half_s).max(0.1);
    second.start_frame = p.start_frame + half_frames;
    second.frame = second.start_frame + ((second.duration_s * fps) as u32) / 2;
    board.panels.insert(index + 1, second);
    Some(index + 1)
}

/// **Delete a panel**: a made one is remembered in `removed`, so making the
/// board again does not bring it back.
pub fn delete_panel(board: &mut Board, index: usize) -> Option<Panel> {
    if index >= board.panels.len() {
        return None;
    }
    let p = board.panels.remove(index);
    if p.generated.is_some() && !board.removed.contains(&p.id) {
        board.removed.push(p.id.clone());
    }
    Some(p)
}

/// **Keep the camera a panel was framed through** on the board, when the
/// user has not set the panel's camera by hand: the snapshot moves with it,
/// so a re-made board does not take it for an edit.
pub fn adopt_camera(panel: &mut Panel, camera: CameraFrame) {
    let made = panel.generated.as_ref().and_then(|g| g.get("camera")).cloned();
    let now = serde_json::to_value(&panel.camera).ok();
    if made.is_some() && made != now {
        return;
    }
    let mut cam = panel.camera.clone();
    let push = cam.movement == CameraMove::Push;
    cam.from = Some(camera);
    cam.to = if push { Some(CameraFrame { zoom: camera.zoom * 1.03, ..camera }) } else { None };
    if !push {
        cam.movement = CameraMove::Hold;
    }
    if let Some(Value::Object(g)) = &mut panel.generated
        && let Ok(v) = serde_json::to_value(&cam)
    {
        g.insert("camera".into(), v);
    }
    panel.camera = cam;
}

/// **Make a storyboard**: breakdown, headless build with the breakdown's shots,
/// the board (merged with the one on disk), a picture per panel, the sheets
/// and the PDF.
pub fn make(req: &StoryboardRequest, progress: &mut dyn FnMut(&str)) -> Result<StoryboardReport> {
    let mut bench = Workbench::open(req, progress)?;
    let first_warning = bench.warnings.len();
    let mut report = StoryboardReport {
        breakdown_backend: bench.breakdown_backend.clone(),
        board: bench.board_path.clone(),
        warnings: bench.warnings.clone(),
        ..Default::default()
    };

    // S2: a picture per panel.
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?;
    let mut board = bench.board.clone();
    let mut items: Vec<(usize, PanelPicture)> = Vec::new();
    for i in 0..board.panels.len() {
        let panel = board.panels[i].clone();
        if !bench.numbers.contains(&panel.scene) {
            continue;
        }
        *report.panels.entry(panel.scene).or_default() += 1;
        if bench.report(panel.scene).is_none() {
            continue;
        }
        let pic = bench.picture(&mut exporter, &panel, req.look, true)?;
        if let Some(c) = pic.camera {
            adopt_camera(&mut board.panels[i], c);
        }
        if let Some(b) = &pic.look_b {
            report.look_b.push(b.clone());
        }
        let check = pic
            .check
            .map(|c| format!("; measured: centre {:+.1} % of the width off, head room {:.1} %, {}", c.off_centre * 100.0, c.head_room * 100.0, if c.cut_at_bottom { "cut at the bottom" } else { "whole figure" }))
            .unwrap_or_default();
        let line = format!("{} {} on {:?}: {}{check}", panel.id, panel.kind, panel.on, pic.framing);
        progress(&format!("panel {} (frame {}): {}", panel.id, panel.frame, pic.look_line));
        progress(&format!("  framing: {line}"));
        report.framing.push(line);
        items.push((i, pic));
    }
    board.save(&bench.board_path)?;
    bench.board = board.clone();

    // S3: the sheets and the PDF.
    let (pages, pdf, warn) = bench.sheets(&mut exporter, &board, &items)?;
    report.pages = pages;
    report.pdf = pdf;
    report.warnings.extend(warn);
    report.warnings.extend(bench.warnings.iter().skip(first_warning).cloned());
    Ok(report)
}
