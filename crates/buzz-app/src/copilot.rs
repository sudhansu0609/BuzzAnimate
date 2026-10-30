//! **The co-pilot** (ANIMATE_COPILOT_PLAN B4): after the producer's first pass
//! the creator keeps directing. Every action here writes an *override* into
//! `<out>/overrides.json`, then rebuilds only the scene(s) it touches (a
//! background change re-runs that one background, then its scenes).
//!
//! **Overrides always win.** [`apply_to_casting`], [`apply_to_plan`],
//! [`apply_to_play`] and [`apply_to_keys`] are called by the producer itself
//! ([`crate::produce`]) on a full rebuild, after the model's choices and the
//! stage pins, so nothing the creator set is lost. The overrides' text is also
//! part of the build hash.
//!
//! The expensive half (ComfyUI, the build, the render) sits behind
//! [`Rebuilder`]; the tests use a stub, the Producer panel and the assistant
//! tools use [`RealRebuilder`]. Both reach the actions through [`run_with`] /
//! [`run_tool`], so the panel and the tools are the same thing.
//!
//! Addressing: a scene is its script number; a shot is its 1-based place in
//! the scene's shot list (what [`view`] shows); a line is `"scene:item"`, the
//! item being its index among the scene's items (what `audio.json` uses).
//! Limits, said plainly: a scene is rebuilt alone, so what it carries over
//! from the scene before it is not carried (positions are in the casting);
//! a picked try or a re-angle is kept in the current files but a later
//! `--redo backgrounds` honours only the seed and the prompt.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use buzz_act::audio_align::AudioPlan;
use buzz_act::screenplay::{Item, Screenplay};
use buzz_ai::breakdown::{Breakdown, CAMERAS, kind_of_camera};
use buzz_doc::{AssetLibrary, Document};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backdrop::{self, BackdropOptions, Style};
use crate::episode::{BuildOptions, Casting, build_episode, scene_file_stem};
use crate::stage_pins;

// ------------------------------------------------------------------ the overrides

/// One shot of a scene's list, once the creator has touched it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShotOv {
    /// Index among the scene's dialogue lines (every cue); the cut starts there.
    pub at_line: usize,
    /// `wide`, `medium` or `close` (what the builder frames).
    pub kind: String,
    /// One of `buzz_ai::breakdown::CAMERAS`.
    pub camera: String,
    /// The character a medium or close is on; empty: the line's speaker.
    pub on: String,
}

/// What the creator set for one background (keyed like `backgrounds.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BgOverride {
    pub seed: Option<u64>,
    pub prompt: Option<String>,
    pub pick: Option<usize>,
    pub reangled: bool,
    /// A picked try or a re-angled picture, kept as its own file: every later
    /// run (`--redo backgrounds`, a full run) uses it as it is, until a new
    /// seed or prompt is set.
    pub fixed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecastOv {
    /// Upper-case character key.
    pub member: String,
    pub asset: String,
    /// `None`: the whole episode; `Some(n)`: scene n only.
    pub scene: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoseOv {
    pub scene: u32,
    pub item: usize,
    pub character: String,
    pub pose: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveOv {
    pub scene: u32,
    pub shot: usize,
    pub character: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetimeOv {
    pub scene: u32,
    pub item: usize,
    /// Total seconds added to the line's start / end, over the recording's.
    pub start_delta: f64,
    pub end_delta: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerOv {
    pub scene: u32,
    pub item: usize,
    pub member: String,
}

/// `<out>/overrides.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overrides {
    pub backgrounds: BTreeMap<String, BgOverride>,
    pub recasts: Vec<RecastOv>,
    pub poses: Vec<PoseOv>,
    pub moves: Vec<MoveOv>,
    /// Scene number -> its whole shot list, once edited.
    pub shots: BTreeMap<String, Vec<ShotOv>>,
    pub retime: Vec<RetimeOv>,
    pub speakers: Vec<SpeakerOv>,
    /// Lines whose lip sync was redone (the note is what the report shows).
    pub lipsync: Vec<String>,
    /// Scenes written to but not rebuilt yet (a rebuild that failed or was cancelled).
    pub dirty: BTreeSet<u32>,
    /// One human line per change, newest last.
    pub log: Vec<String>,
}

pub fn overrides_path(out: &Path) -> PathBuf {
    out.join("overrides.json")
}

pub fn load(out: &Path) -> Overrides {
    std::fs::read_to_string(overrides_path(out))
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn save(out: &Path, ov: &Overrides) -> Result<()> {
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let path = overrides_path(out);
    std::fs::write(&path, serde_json::to_string_pretty(ov)?).with_context(|| format!("writing {}", path.display()))
}

/// The overrides' text, for the producer's build hash.
pub fn overrides_hash(out: &Path) -> String {
    std::fs::read_to_string(overrides_path(out)).unwrap_or_default()
}

fn key_of(name: &str) -> String {
    name.trim().to_uppercase()
}

// ------------------------------------------------------------------ applying them

/// A shot's background and horizon, as the producer writes `shot_backgrounds`.
fn shot_backgrounds(bg: &Value, location: &str, shots: &[ShotOv]) -> (Vec<Value>, Option<f64>) {
    let map = bg.get("backgrounds").unwrap_or(bg);
    let horizon_of = |camera: &str| -> Option<f64> {
        [backdrop::angle_key(location, camera), backdrop::angle_key(location, "wide")]
            .iter()
            .find_map(|k| map.get(k.as_str()).filter(|e| e["asset"].is_string()).map(|e| e["horizon_row"].as_f64()))
            .flatten()
    };
    let per_shot = shots
        .iter()
        .filter_map(|sh| {
            backdrop::background_for(bg, location, &sh.camera)
                .map(|a| json!({"at_line": sh.at_line, "camera": sh.camera, "background": a, "horizon_row": horizon_of(&sh.camera)}))
        })
        .collect();
    (per_shot, horizon_of("wide"))
}

/// Writes every override into a casting value: poses, recasts, moves, then the
/// edited shot lists with their backgrounds. Idempotent.
pub fn apply_to_casting(ov: &Overrides, casting: &mut Value, bg: &Value, pose_rigs: &BTreeMap<String, BTreeMap<String, String>>) {
    if !ov.poses.is_empty() {
        for p in &ov.poses {
            let spec = &mut casting["scenes"][p.scene.to_string()]["place"][key_of(&p.character).as_str()];
            if !spec.is_object() {
                *spec = json!({});
            }
            spec["pose"] = json!(p.pose);
        }
        buzz_ai::breakdown::apply_pose_rigs(casting, pose_rigs);
    }
    for r in &ov.recasts {
        let field = if r.asset.contains('/') { "asset" } else { "cast" };
        match r.scene {
            None => {
                let entry = &mut casting["characters"][r.member.as_str()];
                if !entry.is_object() {
                    *entry = json!({});
                }
                if let Some(o) = entry.as_object_mut() {
                    for k in ["cast", "asset", "puppet", "voice_only"] {
                        o.remove(k);
                    }
                }
                entry[field] = json!(r.asset);
                // A scene's own pose rig would still win over an episode-wide choice; drop it.
                if let Some(scenes) = casting["scenes"].as_object_mut() {
                    for set in scenes.values_mut() {
                        if let Some(spec) = set["place"][r.member.as_str()].as_object_mut() {
                            spec.remove("cast");
                            spec.remove("asset");
                        }
                    }
                }
            }
            Some(n) => {
                let spec = &mut casting["scenes"][n.to_string()]["place"][r.member.as_str()];
                if !spec.is_object() {
                    *spec = json!({});
                }
                if let Some(o) = spec.as_object_mut() {
                    o.remove("cast");
                    o.remove("asset");
                }
                spec[field] = json!(r.asset);
            }
        }
    }
    let pins: Vec<stage_pins::Pin> = ov
        .moves
        .iter()
        .map(|m| stage_pins::Pin { scene: m.scene, name: key_of(&m.character), x: m.x, y: Some(m.y), posed: false })
        .collect();
    stage_pins::apply_pins(casting, &pins);
    for (scene, shots) in &ov.shots {
        let set = &mut casting["scenes"][scene.as_str()];
        if !set.is_object() || shots.is_empty() {
            continue;
        }
        set["shots"] = Value::Array(
            shots
                .iter()
                .map(|s| {
                    let mut v = json!({"at_line": s.at_line, "kind": s.kind, "camera": s.camera});
                    if !s.on.is_empty() {
                        v["on"] = json!(s.on);
                    }
                    v
                })
                .collect(),
        );
        let location = set["location"].as_str().unwrap_or_default().trim().to_string();
        if !location.is_empty() {
            let (per_shot, wide) = shot_backgrounds(bg, &location, shots);
            set["shot_backgrounds"] = Value::Array(per_shot);
            set["wide_horizon"] = json!(wide);
        }
    }
}

/// [`apply_to_casting`] with the overrides read from `out`.
pub fn apply_to_casting_dir(out: &Path, casting: &mut Value, bg: &Value, pose_rigs: &BTreeMap<String, BTreeMap<String, String>>) {
    apply_to_casting(&load(out), casting, bg, pose_rigs);
}

/// Nudged lines keep their new times. Idempotent (the deltas are totals over the recording's).
pub fn apply_to_plan_with(ov: &Overrides, plan: &mut AudioPlan) {
    for r in &ov.retime {
        let Some(line) = plan.lines.iter_mut().find(|l| l.scene == r.scene && l.item == r.item) else { continue };
        line.start = (line.start + r.start_delta).max(0.0);
        line.end = (line.end + r.end_delta).max(line.start + 0.1);
        if let Some(span) = plan.scene_spans.get_mut(&r.scene) {
            span.0 = span.0.min(line.start);
            span.1 = span.1.max(line.end);
        }
    }
}

pub fn apply_to_plan(out: &Path, plan: &mut AudioPlan) {
    apply_to_plan_with(&load(out), plan);
}

/// A changed speaker replaces the script's cue.
pub fn apply_to_play_with(ov: &Overrides, play: &mut Screenplay) {
    for s in &ov.speakers {
        let Some(scene) = play.scenes.iter_mut().find(|x| x.number == s.scene) else { continue };
        if let Some(Item::Dialogue(d)) = scene.items.get_mut(s.item) {
            d.speaker = s.member.clone();
        }
    }
}

pub fn apply_to_play(out: &Path, play: &mut Screenplay) {
    apply_to_play_with(&load(out), play);
}

/// The producer's background list: a seed or prompt the creator set wins.
pub fn apply_to_keys(out: &Path, keys: &mut [(String, String, Option<u64>, Option<(String, String)>)]) {
    let ov = load(out);
    for (key, prompt, seed, angle_of) in keys.iter_mut() {
        let Some(b) = ov.backgrounds.get(key.as_str()) else { continue };
        if let Some(s) = b.seed {
            *seed = Some(s);
        }
        if angle_of.is_none()
            && let Some(p) = b.prompt.as_ref().filter(|p| !p.trim().is_empty())
        {
            *prompt = p.clone();
        }
    }
}

// ------------------------------------------------------------------ the report

/// `md` with an "Overrides" section listing what the creator changed.
pub fn with_overrides(md: String, out: &Path) -> String {
    let mut md = match md.find("\n## Overrides") {
        Some(i) => {
            let rest = &md[i + 1..];
            let end = rest[3..].find("\n## ").map(|e| i + 1 + 3 + e);
            match end {
                Some(e) => format!("{}{}", &md[..i], &md[e..]),
                None => md[..i].to_string(),
            }
        }
        None => md,
    };
    let ov = load(out);
    if ov.log.is_empty() {
        return md;
    }
    if !md.ends_with('\n') {
        md.push('\n');
    }
    md.push_str("\n## Overrides\n\nWhat the creator changed after the first pass (these win over every rebuild):\n\n");
    for line in &ov.log {
        md.push_str(&format!("- {line}\n"));
    }
    md
}

/// Rewrite `report.md` in place with the current Overrides section.
pub fn refresh_report(out: &Path) {
    let path = out.join("report.md");
    if let Ok(md) = std::fs::read_to_string(&path) {
        let _ = std::fs::write(&path, with_overrides(md, out));
    }
}

// ------------------------------------------------------------------ the producer's context

/// What the producer was asked, kept so a scene can be rebuilt without it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ctx {
    pub assets: PathBuf,
    pub size: (f64, f64),
    pub fps: f64,
    pub index: Option<PathBuf>,
    pub captions: bool,
    pub objects: bool,
    pub series: String,
    pub style_name: String,
    pub style_suffix: String,
    pub height: u32,
    pub mp4: bool,
    pub stills: u32,
}

impl Ctx {
    fn load(out: &Path) -> Result<Self> {
        let path = out.join("copilot_ctx.json");
        let text = std::fs::read_to_string(&path).with_context(|| format!("{}: produce the episode once first", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
    }
}

/// Called by the producer on every full run.
pub fn save_context(req: &crate::produce::ProduceRequest, series: &str, assets: &Path, captions: bool, objects: bool, script: &[u8]) {
    let ctx = Ctx {
        assets: assets.to_path_buf(),
        size: req.size,
        fps: req.fps,
        index: req.index.clone(),
        captions,
        objects,
        series: series.to_string(),
        style_name: req.style.name.clone(),
        style_suffix: req.style.suffix.clone(),
        height: req.height,
        mp4: req.mp4,
        stills: req.stills,
    };
    let out = &req.out;
    let _ = std::fs::write(out.join("copilot_script.md"), script);
    if let Ok(text) = serde_json::to_string_pretty(&ctx) {
        let _ = std::fs::write(out.join("copilot_ctx.json"), text);
    }
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
}

fn write_json(path: &Path, v: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(v)?).with_context(|| format!("writing {}", path.display()))
}

fn h8(parts: &[&str]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    format!("{:016x}", h.finish())[..8].to_string()
}

fn new_seed() -> u64 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    (n ^ (n >> 17)).wrapping_mul(0x9E37_79B9_7F4A_7C15) & 0xFFFF_FFFF_FFFF
}

// ------------------------------------------------------------------ scene facts

fn breakdown_of(out: &Path) -> Option<Breakdown> {
    read_json(&out.join("breakdown.json")).and_then(|v| serde_json::from_value(v).ok())
}

/// A scene's current shot list: the edited one, else the breakdown's.
pub fn shots_for(out: &Path, ov: &Overrides, scene: u32) -> Vec<ShotOv> {
    if let Some(list) = ov.shots.get(&scene.to_string()) {
        return list.clone();
    }
    let from_bd = breakdown_of(out)
        .and_then(|b| b.scenes.into_iter().find(|s| s.number == scene))
        .map(|s| {
            s.shots
                .into_iter()
                .map(|sh| {
                    let camera = if sh.camera.is_empty() { sh.kind.clone() } else { sh.camera.clone() };
                    ShotOv { at_line: sh.at_line, kind: sh.kind, camera, on: sh.on }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if from_bd.is_empty() {
        vec![ShotOv { at_line: 0, kind: "wide".into(), camera: "wide".into(), on: String::new() }]
    } else {
        from_bd
    }
}

fn location_of(out: &Path, scene: u32) -> String {
    read_json(&out.join("casting.json"))
        .and_then(|c| c["scenes"][scene.to_string()]["location"].as_str().map(|s| s.trim().to_string()))
        .or_else(|| breakdown_of(out).and_then(|b| b.scenes.into_iter().find(|s| s.number == scene)).map(|s| s.location_key.trim().to_string()))
        .unwrap_or_default()
}

fn scene_numbers(out: &Path) -> Vec<u32> {
    let from_casting: Vec<u32> = read_json(&out.join("casting.json"))
        .and_then(|c| c["scenes"].as_object().map(|m| m.keys().filter_map(|k| k.parse().ok()).collect()))
        .unwrap_or_default();
    if !from_casting.is_empty() {
        let mut v = from_casting;
        v.sort();
        return v;
    }
    breakdown_of(out).map(|b| b.scenes.iter().map(|s| s.number).collect()).unwrap_or_default()
}

fn audio_lines(out: &Path) -> Vec<(u32, usize)> {
    read_json(&out.join("audio.json"))
        .and_then(|v| v["lines"].as_array().map(|a| a.iter().filter_map(|l| Some((l["scene"].as_u64()? as u32, l["item"].as_u64()? as usize))).collect()))
        .unwrap_or_default()
}

/// A line: `"scene:item"`, `"scene.item"` or `{scene, item}`.
pub fn parse_line(v: &Value) -> Result<(u32, usize)> {
    if let Some(o) = v.as_object() {
        let s = o.get("scene").and_then(Value::as_u64);
        let i = o.get("item").and_then(Value::as_u64);
        if let (Some(s), Some(i)) = (s, i) {
            return Ok((s as u32, i as usize));
        }
    }
    if let Some(t) = v.as_str() {
        let mut it = t.trim().split([':', '.']);
        if let (Some(s), Some(i), None) = (it.next(), it.next(), it.next())
            && let (Ok(s), Ok(i)) = (s.trim().parse(), i.trim().parse())
        {
            return Ok((s, i));
        }
    }
    bail!("a line is \"scene:item\" (the item's index in the scene, as the Producer panel shows it) or {{scene, item}}")
}

// ------------------------------------------------------------------ the heavy half

/// What a background job does.
#[derive(Debug, Clone, PartialEq)]
pub enum BgKind {
    Redo { seed: u64 },
    Prompt(String),
    Pick(usize),
    Reangle { seed: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BgJob {
    pub location: String,
    /// `wide` or a camera.
    pub angle: String,
    pub kind: BgKind,
}

impl BgJob {
    pub fn key(&self) -> String {
        backdrop::angle_key(&self.location, &self.angle)
    }
}

/// The picture of one rebuilt scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Rebuilt {
    pub scene: u32,
    pub still: Option<PathBuf>,
}

/// The expensive half of an action. [`RealRebuilder`] is the real one.
pub trait Rebuilder {
    /// Re-run one background (generate/pick, trace, save the asset); what was done, in a line.
    fn background(&mut self, out: &Path, job: &BgJob, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<String>;
    /// Rebuild these scenes only: casting + overrides, `build_episode`, spliced into the `.buzz`, rendered.
    fn rebuild(&mut self, out: &Path, scenes: &[u32], say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<Vec<Rebuilt>>;
}

/// What an action returns.
#[derive(Debug, Clone, Default)]
pub struct ActionResult {
    pub action: String,
    pub changed: String,
    pub scenes: Vec<u32>,
    pub stills: Vec<(u32, PathBuf)>,
}

impl ActionResult {
    pub fn to_json(&self) -> Value {
        json!({
            "action": self.action,
            "changed": self.changed,
            "scenes_rebuilt": self.scenes,
            "still": self.stills.first().map(|(_, p)| p),
            "stills": self.stills.iter().map(|(s, p)| json!({"scene": s, "path": p})).collect::<Vec<_>>(),
        })
    }
}

/// Where an action runs.
pub struct Run<'a> {
    pub out: &'a Path,
    pub rb: &'a mut dyn Rebuilder,
    pub say: &'a mut dyn FnMut(&str),
    pub cancel: &'a dyn Fn() -> bool,
}

const LOG_MAX: usize = 300;

/// Save `ov` with `scenes` dirty, rebuild them, clear them, answer.
fn finish(run: &mut Run, mut ov: Overrides, action: &str, scenes: Vec<u32>, changed: String) -> Result<ActionResult> {
    let mut scenes = scenes;
    scenes.sort();
    scenes.dedup();
    ov.dirty.extend(scenes.iter().copied());
    ov.log.push(changed.clone());
    if ov.log.len() > LOG_MAX {
        let cut = ov.log.len() - LOG_MAX;
        ov.log.drain(..cut);
    }
    save(run.out, &ov)?;
    let built = run.rb.rebuild(run.out, &scenes, run.say, run.cancel)?;
    for s in &scenes {
        ov.dirty.remove(s);
    }
    save(run.out, &ov)?;
    refresh_report(run.out);
    Ok(ActionResult {
        action: action.into(),
        changed,
        scenes,
        stills: built.into_iter().filter_map(|r| r.still.map(|p| (r.scene, p))).collect(),
    })
}

/// The scenes that show a background: all at the location for the wide, those with a shot at that camera otherwise.
fn scenes_of_background(out: &Path, ov: &Overrides, location: &str, angle: &str) -> Vec<u32> {
    let at: Vec<u32> = scene_numbers(out).into_iter().filter(|n| location_of(out, *n) == location.trim()).collect();
    if angle == "wide" || angle.is_empty() {
        return at;
    }
    let with: Vec<u32> = at.iter().copied().filter(|n| shots_for(out, ov, *n).iter().any(|s| s.camera == angle)).collect();
    if with.is_empty() { at } else { with }
}

fn angle_arg(angle: &str) -> String {
    let a = angle.trim();
    if a.is_empty() { "wide".into() } else { a.to_string() }
}

fn background_action(run: &mut Run, action: &str, location: &str, angle: &str, kind: BgKind, change: impl FnOnce(&mut BgOverride)) -> Result<ActionResult> {
    let angle = angle_arg(angle);
    if location.trim().is_empty() {
        bail!("a location is needed (the key in backgrounds.json)");
    }
    let job = BgJob { location: location.trim().to_string(), angle: angle.clone(), kind };
    let key = job.key();
    let note = run.rb.background(run.out, &job, run.say, run.cancel)?;
    let mut ov = load(run.out);
    // A picked try or a re-angle persists; a new seed or prompt draws anew.
    let keep = matches!(action, "pick_background_try" | "reangle_background");
    let fixed = if keep { keep_picture(run.out, &key) } else { None };
    let entry = ov.backgrounds.entry(key.clone()).or_default();
    entry.fixed = fixed;
    change(entry);
    let scenes = scenes_of_background(run.out, &ov, &job.location, &angle);
    finish(run, ov, action, scenes, format!("background {key}: {note}"))
}

/// Copy the background's current picture (`backgrounds.json`) to `backgrounds/<key>.fixed.png`
/// and answer that path: the file a later run uses as it is.
fn keep_picture(out: &Path, key: &str) -> Option<String> {
    let bg = read_json(&out.join("backgrounds.json"))?;
    let png = PathBuf::from(bg["backgrounds"][key]["png"].as_str()?);
    let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let dir = out.join("backgrounds");
    std::fs::create_dir_all(&dir).ok()?;
    let to = dir.join(format!("{safe}.fixed.png"));
    std::fs::copy(&png, &to).ok()?;
    Some(to.display().to_string())
}

/// The picture the creator picked (a try, or a re-angle) for this background key, if its file is still there.
pub fn fixed_picture(out: &Path, key: &str) -> Option<PathBuf> {
    let ov = load(out);
    let p = PathBuf::from(ov.backgrounds.get(key)?.fixed.as_ref()?);
    p.is_file().then_some(p)
}

/// A new seed for a background (an angle is re-angled from the wide again).
pub fn redo_background(run: &mut Run, location: &str, angle: &str) -> Result<ActionResult> {
    let angle = angle_arg(angle);
    let seed = new_seed();
    let kind = if angle == "wide" { BgKind::Redo { seed } } else { BgKind::Reangle { seed } };
    background_action(run, "redo_background", location, &angle, kind, |b| b.seed = Some(seed))
}

pub fn set_background_prompt(run: &mut Run, location: &str, angle: &str, text: &str) -> Result<ActionResult> {
    if text.trim().is_empty() {
        bail!("the prompt text is empty");
    }
    let text = text.trim().to_string();
    let t = text.clone();
    background_action(run, "set_background_prompt", location, angle, BgKind::Prompt(text), move |b| b.prompt = Some(t))
}

pub fn pick_background_try(run: &mut Run, location: &str, angle: &str, n: usize) -> Result<ActionResult> {
    if n == 0 {
        bail!("tries are numbered from 1");
    }
    background_action(run, "pick_background_try", location, angle, BgKind::Pick(n), move |b| b.pick = Some(n))
}

/// Re-angle an angle picture from the location's wide (Qwen), with a new seed.
pub fn reangle_background(run: &mut Run, location: &str, angle: &str) -> Result<ActionResult> {
    let angle = angle_arg(angle);
    if angle == "wide" {
        bail!("re-angle needs an angle other than wide: low_angle, high_angle, over_shoulder or reverse");
    }
    let seed = new_seed();
    background_action(run, "reangle_background", location, &angle, BgKind::Reangle { seed }, move |b| {
        b.seed = Some(seed);
        b.reangled = true;
    })
}

/// Swap a cast member's character, for the episode (`scene` None) or one scene.
pub fn recast(run: &mut Run, member: &str, asset: &str, scene: Option<u32>) -> Result<ActionResult> {
    let (member, asset) = (key_of(member), asset.trim().to_string());
    if member.is_empty() || asset.is_empty() {
        bail!("recast needs a member and an asset (a Cast rig name, or a library asset path)");
    }
    let mut ov = load(run.out);
    ov.recasts.retain(|r| !(r.member == member && r.scene == scene));
    ov.recasts.push(RecastOv { member: member.clone(), asset: asset.clone(), scene });
    let scenes = match scene {
        Some(n) => vec![n],
        None => {
            let with: Vec<u32> = read_json(&run.out.join("casting.json"))
                .and_then(|c| c["scenes"].as_object().map(|m| m.iter().filter(|(_, s)| s["place"][member.as_str()].is_object()).filter_map(|(k, _)| k.parse().ok()).collect()))
                .unwrap_or_default();
            if with.is_empty() { scene_numbers(run.out) } else { with }
        }
    };
    let where_ = scene.map(|n| format!("scene {n}")).unwrap_or_else(|| "the whole episode".into());
    finish(run, ov, "recast", scenes, format!("{member} played by {asset} in {where_}"))
}

pub fn set_pose(run: &mut Run, line: (u32, usize), character: &str, pose: &str) -> Result<ActionResult> {
    let (character, pose) = (key_of(character), pose.trim().to_string());
    if character.is_empty() || pose.is_empty() {
        bail!("set_pose needs a character and a pose name");
    }
    let mut ov = load(run.out);
    ov.poses.retain(|p| !(p.scene == line.0 && p.character == character));
    ov.poses.push(PoseOv { scene: line.0, item: line.1, character: character.clone(), pose: pose.clone() });
    finish(run, ov, "set_pose", vec![line.0], format!("scene {} line {}: {character} posed {pose}", line.0, line.1))
}

/// Stand a character somewhere: writes a pin too, so the stage keeps it.
pub fn move_character(run: &mut Run, scene: u32, shot: usize, character: &str, x: f64, y: f64) -> Result<ActionResult> {
    let name = key_of(character);
    if name.is_empty() {
        bail!("move_character needs a character");
    }
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        bail!("x and y are fractions of the stage, 0 to 1");
    }
    let mut ov = load(run.out);
    ov.moves.retain(|m| !(m.scene == scene && m.character == name));
    ov.moves.push(MoveOv { scene, shot, character: name.clone(), x, y });
    let pins_path = run.out.join("pins.json");
    let old: Vec<stage_pins::Pin> = read_json(&pins_path).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let pins = stage_pins::merge_pins(old, vec![stage_pins::Pin { scene, name: name.clone(), x, y: Some(y), posed: false }]);
    std::fs::write(&pins_path, serde_json::to_string_pretty(&pins)?).with_context(|| format!("writing {}", pins_path.display()))?;
    finish(run, ov, "move_character", vec![scene], format!("scene {scene} shot {shot}: {name} moved to x {x:.2}, y {y:.2}"))
}

fn check_camera(camera: &str) -> Result<String> {
    let c = camera.trim().to_lowercase().replace([' ', '-'], "_");
    if CAMERAS.contains(&c.as_str()) {
        Ok(c)
    } else {
        bail!("camera '{camera}' is not one of {}", CAMERAS.join(", "))
    }
}

/// `shot` is 1-based.
pub fn set_shot_camera(run: &mut Run, scene: u32, shot: usize, camera: &str) -> Result<ActionResult> {
    let camera = check_camera(camera)?;
    let mut ov = load(run.out);
    let mut list = shots_for(run.out, &ov, scene);
    let Some(s) = shot.checked_sub(1).and_then(|i| list.get_mut(i)) else {
        bail!("scene {scene} has {} shot(s); there is no shot {shot}", list.len());
    };
    s.camera = camera.clone();
    s.kind = kind_of_camera(&camera).to_string();
    ov.shots.insert(scene.to_string(), list);
    finish(run, ov, "set_shot_camera", vec![scene], format!("scene {scene} shot {shot}: camera {camera}"))
}

/// A new cut at dialogue line `at_line`.
pub fn add_cut(run: &mut Run, scene: u32, at_line: usize, camera: &str) -> Result<ActionResult> {
    let camera = check_camera(camera)?;
    let mut ov = load(run.out);
    let mut list = shots_for(run.out, &ov, scene);
    list.retain(|s| s.at_line != at_line);
    list.push(ShotOv { at_line, kind: kind_of_camera(&camera).into(), camera: camera.clone(), on: String::new() });
    list.sort_by_key(|s| s.at_line);
    ov.shots.insert(scene.to_string(), list);
    finish(run, ov, "add_cut", vec![scene], format!("scene {scene}: cut to {camera} at line {at_line}"))
}

/// `shot` is 1-based; the last shot of a scene cannot be removed.
pub fn remove_cut(run: &mut Run, scene: u32, shot: usize) -> Result<ActionResult> {
    let mut ov = load(run.out);
    let mut list = shots_for(run.out, &ov, scene);
    if shot == 0 || shot > list.len() {
        bail!("scene {scene} has {} shot(s); there is no shot {shot}", list.len());
    }
    if list.len() == 1 {
        bail!("scene {scene} has one shot; a scene needs at least one");
    }
    let gone = list.remove(shot - 1);
    if gone.at_line == 0 {
        // The opening shot is always at line 0: the next one takes its place.
        list[0].at_line = 0;
    }
    ov.shots.insert(scene.to_string(), list);
    finish(run, ov, "remove_cut", vec![scene], format!("scene {scene}: shot {shot} ({}) removed", gone.camera))
}

/// Nudge one line's start and end, in seconds; the other lines do not move.
pub fn retime_line(run: &mut Run, line: (u32, usize), start_delta: f64, end_delta: f64) -> Result<ActionResult> {
    if !audio_lines(run.out).contains(&line) {
        bail!("scene {} item {} is not a line found in the recording (audio.json); retiming needs a recording", line.0, line.1);
    }
    let mut ov = load(run.out);
    match ov.retime.iter_mut().find(|r| r.scene == line.0 && r.item == line.1) {
        Some(r) => {
            r.start_delta += start_delta;
            r.end_delta += end_delta;
        }
        None => ov.retime.push(RetimeOv { scene: line.0, item: line.1, start_delta, end_delta }),
    }
    finish(run, ov, "retime_line", vec![line.0], format!("scene {} line {}: start {start_delta:+.2} s, end {end_delta:+.2} s", line.0, line.1))
}

pub fn set_speaker(run: &mut Run, line: (u32, usize), member: &str) -> Result<ActionResult> {
    let member = member.trim().to_string();
    if member.is_empty() {
        bail!("set_speaker needs a member");
    }
    let mut ov = load(run.out);
    ov.speakers.retain(|s| !(s.scene == line.0 && s.item == line.1));
    ov.speakers.push(SpeakerOv { scene: line.0, item: line.1, member: member.clone() });
    finish(run, ov, "set_speaker", vec![line.0], format!("scene {} line {}: spoken by {member}", line.0, line.1))
}

/// Lip sync is built from the line's timing and speaker: redoing it is a rebuild of that line's scene (or a part's scenes).
pub fn redo_lipsync(run: &mut Run, line: Option<(u32, usize)>, part: Option<usize>) -> Result<ActionResult> {
    let mut ov = load(run.out);
    let (scenes, what) = match (line, part) {
        (Some((s, i)), _) => (vec![s], format!("scene {s} line {i}")),
        (None, Some(p)) => {
            let plan: crate::produce_parts::PartsPlan = read_json(&run.out.join("parts.json"))
                .and_then(|v| serde_json::from_value(v).ok())
                .ok_or_else(|| anyhow!("there is no parts.json: the episode was not produced from audio parts"))?;
            let scenes = plan.part_scenes(p);
            if scenes.is_empty() {
                bail!("part {p} has no scenes");
            }
            (scenes, format!("part {p}"))
        }
        (None, None) => bail!("redo_lipsync needs a line or a part"),
    };
    ov.lipsync.push(what.clone());
    finish(run, ov, "redo_lipsync", scenes, format!("lip sync redone for {what}"))
}

pub fn rebuild_scene(run: &mut Run, scene: u32) -> Result<ActionResult> {
    let ov = load(run.out);
    finish(run, ov, "rebuild_scene", vec![scene], format!("scene {scene} rebuilt"))
}

// ------------------------------------------------------------------ the tools

/// The tool names, in the order the panel offers them.
pub const TOOLS: [&str; 14] = [
    "redo_background",
    "set_background_prompt",
    "pick_background_try",
    "reangle_background",
    "recast",
    "set_pose",
    "move_character",
    "set_shot_camera",
    "add_cut",
    "remove_cut",
    "retime_line",
    "set_speaker",
    "redo_lipsync",
    "rebuild_scene",
];

/// Args each tool takes besides `out`.
pub fn tool_args(tool: &str) -> &'static [&'static str] {
    match tool {
        "redo_background" | "reangle_background" => &["location", "angle"],
        "set_background_prompt" => &["location", "angle", "text"],
        "pick_background_try" => &["location", "angle", "n"],
        "recast" => &["member", "asset", "scope", "scene"],
        "set_pose" => &["line", "character", "pose"],
        "move_character" => &["scene", "shot", "character", "x", "y"],
        "set_shot_camera" => &["scene", "shot", "camera"],
        "add_cut" => &["scene", "at_line", "camera"],
        "remove_cut" => &["scene", "shot"],
        "retime_line" => &["line", "start_delta", "end_delta"],
        "set_speaker" => &["line", "member"],
        "redo_lipsync" => &["line", "part"],
        "rebuild_scene" => &["scene"],
        _ => &[],
    }
}

fn text(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

fn need_text(args: &Value, key: &str) -> Result<String> {
    text(args, key).ok_or_else(|| anyhow!("missing arg '{key}'"))
}

fn num(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
}

fn need_u(args: &Value, key: &str) -> Result<usize> {
    num(args, key).filter(|n| *n >= 0.0).map(|n| n as usize).ok_or_else(|| anyhow!("missing or bad arg '{key}' (a whole number)"))
}

/// Run one tool/action by name with its args.
pub fn run_with(run: &mut Run, tool: &str, args: &Value) -> Result<ActionResult> {
    if let Some(map) = args.as_object() {
        let valid = tool_args(tool);
        for key in map.keys() {
            if key != "out" && !valid.contains(&key.as_str()) {
                bail!("unknown arg '{key}' for {tool}; valid: out, {}", valid.join(", "));
            }
        }
    }
    let angle = text(args, "angle").unwrap_or_else(|| "wide".into());
    match tool {
        "redo_background" => redo_background(run, &need_text(args, "location")?, &angle),
        "set_background_prompt" => set_background_prompt(run, &need_text(args, "location")?, &angle, &need_text(args, "text")?),
        "pick_background_try" => pick_background_try(run, &need_text(args, "location")?, &angle, need_u(args, "n")?),
        "reangle_background" => reangle_background(run, &need_text(args, "location")?, &angle),
        "recast" => {
            let scope = text(args, "scope").unwrap_or_else(|| "episode".into()).to_lowercase();
            let scene = match scope.as_str() {
                "episode" => None,
                "scene" => Some(need_u(args, "scene")? as u32),
                other => Some(other.trim_start_matches("scene").trim_matches([':', ' ']).parse::<u32>().map_err(|_| anyhow!("scope is 'episode' or 'scene' (with scene: n)"))?),
            };
            recast(run, &need_text(args, "member")?, &need_text(args, "asset")?, scene)
        }
        "set_pose" => set_pose(run, parse_line(args.get("line").unwrap_or(&Value::Null))?, &need_text(args, "character")?, &need_text(args, "pose")?),
        "move_character" => {
            let (Some(x), Some(y)) = (num(args, "x"), num(args, "y")) else { bail!("missing x / y") };
            move_character(run, need_u(args, "scene")? as u32, num(args, "shot").map(|n| n as usize).unwrap_or(1), &need_text(args, "character")?, x, y)
        }
        "set_shot_camera" => set_shot_camera(run, need_u(args, "scene")? as u32, need_u(args, "shot")?, &need_text(args, "camera")?),
        "add_cut" => add_cut(run, need_u(args, "scene")? as u32, need_u(args, "at_line")?, &need_text(args, "camera")?),
        "remove_cut" => remove_cut(run, need_u(args, "scene")? as u32, need_u(args, "shot")?),
        "retime_line" => retime_line(
            run,
            parse_line(args.get("line").unwrap_or(&Value::Null))?,
            num(args, "start_delta").unwrap_or(0.0),
            num(args, "end_delta").unwrap_or(0.0),
        ),
        "set_speaker" => set_speaker(run, parse_line(args.get("line").unwrap_or(&Value::Null))?, &need_text(args, "member")?),
        "redo_lipsync" => redo_lipsync(run, args.get("line").filter(|v| !v.is_null()).map(parse_line).transpose()?, num(args, "part").map(|n| n as usize)),
        "rebuild_scene" => rebuild_scene(run, need_u(args, "scene")? as u32),
        other => bail!("unknown co-pilot action '{other}'"),
    }
}

/// An action with the real rebuilder (a Task or an assistant job runs this).
pub fn run_tool(out: &Path, tool: &str, args: &Value, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<ActionResult> {
    let mut rb = RealRebuilder::default();
    let mut run = Run { out, rb: &mut rb, say, cancel };
    run_with(&mut run, tool, args)
}

/// The episode's `.buzz` in `out` (None before the first produce).
pub fn buzz_path(out: &Path) -> Option<PathBuf> {
    let ctx = Ctx::load(out).ok()?;
    Some(out.join(format!("{}.buzz", crate::produce::file_safe(&ctx.series))))
}

/// Whether `dir` is a produced episode's output folder.
pub fn is_episode_dir(dir: &Path) -> bool {
    dir.join("copilot_ctx.json").is_file() && dir.join("breakdown.json").is_file()
}

// ------------------------------------------------------------------ the panel's view

/// Everything the Producer panel shows: scenes -> shots -> lines.
pub fn view(out: &Path) -> Result<Value> {
    let ov = load(out);
    let bd = breakdown_of(out).ok_or_else(|| anyhow!("{}: no breakdown.json; produce the episode first", out.display()))?;
    let bg = read_json(&out.join("backgrounds.json")).unwrap_or(json!({}));
    let bgmap = bg.get("backgrounds").unwrap_or(&bg).clone();
    let casting = read_json(&out.join("casting.json")).unwrap_or(json!({}));
    let ctx = Ctx::load(out).ok();
    let mut play = std::fs::read_to_string(out.join("copilot_script.md")).map(|t| Screenplay::parse(&t)).unwrap_or(Screenplay { title: String::new(), scenes: Vec::new() });
    apply_to_play_with(&ov, &mut play);
    let mut plan = read_json(&out.join("audio.json")).and_then(|v| crate::produce::plan_from_json(&v));
    if let Some(p) = plan.as_mut() {
        apply_to_plan_with(&ov, p);
    }
    let mut speakers: BTreeSet<String> = BTreeSet::new();
    let cast_poses = read_json(&out.join("cast.json")).unwrap_or(json!({}))["poses"].clone();
    let mut scenes = Vec::new();
    for s in &bd.scenes {
        let location = location_of(out, s.number);
        let shots: Vec<Value> = shots_for(out, &ov, s.number)
            .iter()
            .enumerate()
            .map(|(i, sh)| {
                let key = backdrop::angle_key(&location, &sh.camera);
                let have = bgmap.get(key.as_str()).filter(|e| e["asset"].is_string());
                let used = if have.is_some() { key.clone() } else { backdrop::angle_key(&location, "wide") };
                let e = bgmap.get(used.as_str());
                let png = e.and_then(|e| e["png"].as_str()).map(PathBuf::from);
                let tries: Vec<String> = png.as_ref().map(|p| tries_of(p)).unwrap_or_default();
                json!({
                    "n": i + 1, "at_line": sh.at_line, "kind": sh.kind, "camera": sh.camera, "on": sh.on,
                    "background_key": used, "background_png": png, "tries": tries,
                    "prompt": e.and_then(|e| e["prompt"].as_str()).unwrap_or_default(),
                    "seed": e.and_then(|e| e["seed"].as_u64()),
                })
            })
            .collect();
        let mut lines = Vec::new();
        if let Some(src) = play.scenes.iter().find(|x| x.number == s.number) {
            let mut cue = 0usize;
            for (item, it) in src.items.iter().enumerate() {
                let Item::Dialogue(d) = it else { continue };
                speakers.insert(d.speaker.clone());
                let times = plan.as_ref().and_then(|p| p.lines.iter().find(|l| l.scene == s.number && l.item == item)).map(|l| (l.start, l.end));
                let pose = ov
                    .poses
                    .iter()
                    .find(|p| p.scene == s.number && p.character == key_of(&d.speaker))
                    .map(|p| p.pose.clone())
                    .or_else(|| casting["scenes"][s.number.to_string()]["place"][key_of(&d.speaker).as_str()]["pose"].as_str().map(String::from))
                    .unwrap_or_default();
                lines.push(json!({
                    "item": item, "cue": cue, "speaker": d.speaker, "text": d.text,
                    "start": times.map(|t| t.0), "end": times.map(|t| t.1), "pose": pose,
                    "poses": poses_of(&cast_poses, &d.speaker),
                }));
                cue += 1;
            }
        }
        scenes.push(json!({"number": s.number, "location": location, "dirty": ov.dirty.contains(&s.number), "shots": shots, "lines": lines, "still": still_of(out, s.number)}));
    }
    for c in &bd.characters {
        speakers.insert(c.name.clone());
    }
    let series = ctx.as_ref().map(|c| c.series.clone()).unwrap_or_default();
    let buzz = out.join(format!("{}.buzz", crate::produce::file_safe(&series)));
    Ok(json!({
        "out": out, "series": series, "buzz": buzz,
        "scenes": scenes, "speakers": speakers, "cameras": CAMERAS,
        "overrides": ov.log,
    }))
}

/// The poses offered for a speaker: standing first, the character's pose-variant rigs
/// (`<Name> Pose <pose>`, as `cast.json` lists them), then the rig's own Sit.
fn poses_of(cast_poses: &Value, speaker: &str) -> Vec<String> {
    let mut v = vec!["standing (default)".to_string()];
    if let Some(o) = cast_poses[key_of(speaker).as_str()].as_object() {
        v.extend(o.keys().map(|k| k.replace('_', " ")));
    }
    if !v.iter().any(|p| p.eq_ignore_ascii_case("sit")) {
        v.push("Sit".into());
    }
    v
}

/// One of the episode's own stills for a scene (`scene02_..._f0012.png`), if the run made any.
fn still_of(out: &Path, scene: u32) -> Option<PathBuf> {
    let prefix = format!("scene{scene:02}_");
    ["copilot", "stills"].iter().find_map(|d| {
        let mut found: Vec<PathBuf> = std::fs::read_dir(out.join(d))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "png") && p.file_name().is_some_and(|f| f.to_string_lossy().starts_with(&prefix)))
            .collect();
        found.sort();
        found.into_iter().next()
    })
}

/// `<stem>.try1.png`, `.try2.png` ... next to a background's picture.
fn tries_of(png: &Path) -> Vec<String> {
    let stem = png.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let prefix = format!("{stem}.try");
    let mut found: Vec<(usize, String)> = png
        .parent()
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let n = name.strip_prefix(&prefix)?.strip_suffix(".png")?.parse::<usize>().ok()?;
                    Some((n, e.path().display().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

// ------------------------------------------------------------------ the real rebuilder

/// ComfyUI, the build and the render.
#[derive(Default)]
pub struct RealRebuilder {
    comfy: Option<crate::comfy::ComfyClient>,
}

impl RealRebuilder {
    fn client(&mut self, say: &mut dyn FnMut(&str)) -> Result<crate::comfy::ComfyClient> {
        if let Some(c) = &self.comfy {
            return Ok(c.clone());
        }
        say("finding ComfyUI (starting it if it is not running)\u{2026}");
        let c = crate::comfy::ComfyClient::ensure_running(true, Duration::from_secs(600)).map_err(|e| anyhow!("ComfyUI: {e}"))?;
        self.comfy = Some(c.clone());
        Ok(c)
    }
}

impl Rebuilder for RealRebuilder {
    fn background(&mut self, out: &Path, job: &BgJob, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<String> {
        let ctx = Ctx::load(out)?;
        let bg_path = out.join("backgrounds.json");
        let mut bg = read_json(&bg_path).ok_or_else(|| anyhow!("{}: no backgrounds.json", bg_path.display()))?;
        let key = job.key();
        let wide_key = job.location.clone();
        let is_angle = key != wide_key;
        let entry = bg["backgrounds"][key.as_str()].clone();
        let wide_png = bg["backgrounds"][wide_key.as_str()]["png"].as_str().map(PathBuf::from);
        let style = Style { name: ctx.style_name.clone(), suffix: ctx.style_suffix.clone() };
        let mut entry = if entry.is_object() { entry } else { json!({}) };
        let note: String;
        let png: PathBuf;
        let mut people: Option<Vec<backdrop::PeopleTry>> = None;
        match &job.kind {
            BgKind::Pick(n) => {
                let cur = entry["png"].as_str().map(PathBuf::from).ok_or_else(|| anyhow!("background {key} has no picture yet"))?;
                let stem = cur.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let src = cur.with_file_name(format!("{stem}.try{n}.png"));
                if !src.is_file() {
                    bail!("background {key} has no try {n} ({})", src.display());
                }
                std::fs::copy(&src, &cur).with_context(|| format!("copying {}", src.display()))?;
                note = format!("try {n} kept");
                png = cur;
            }
            BgKind::Redo { seed } | BgKind::Reangle { seed } => {
                check_cancel(cancel)?;
                let client = self.client(say)?;
                let (full, angle_src): (String, Option<(PathBuf, String)>) = if is_angle {
                    let wide = wide_png.clone().filter(|p| p.is_file()).ok_or_else(|| anyhow!("{wide_key} has no wide picture to re-angle"))?;
                    let missing = backdrop::missing_angle_models(&client).map_err(|e| anyhow!(e))?;
                    if !missing.is_empty() {
                        bail!("angle models missing: {}", missing.join("; "));
                    }
                    let left = h8(&[&wide_key]).as_bytes()[0] % 2 == 0;
                    let p = entry["prompt"]
                        .as_str()
                        .map(String::from)
                        .or_else(|| crate::comfy_profile::angle_prompt(&job.angle, left).map(String::from))
                        .ok_or_else(|| anyhow!("'{}' has no angle picture (low_angle, high_angle, over_shoulder, reverse do)", job.angle))?;
                    (p.clone(), Some((wide, p)))
                } else {
                    let p = entry["prompt"].as_str().map(String::from).ok_or_else(|| anyhow!("background {key} has no prompt on record"))?;
                    (p, None)
                };
                let wide_hash = angle_src.as_ref().map(|(w, _)| h8(&[&w.display().to_string()])).unwrap_or_default();
                let p = out.join("backgrounds").join(format!("{key}-{}.png", h8(&[&full, &seed.to_string(), &wide_hash])));
                let mut sayb = |l: &str| say(&format!("{key}: {l}"));
                let result = match &angle_src {
                    Some((w, ap)) => backdrop::generate_checked_by(Some(&client), *seed, &p, 3, true, &mut sayb, &|c, s, path| backdrop::generate_angle(c, w, ap, s, path).map(|_| ())),
                    None => backdrop::generate_checked(Some(&client), &full, *seed, &p, 3, true, &mut sayb),
                };
                let (_, tries) = result.map_err(|e| anyhow!("ComfyUI: {e}"))?;
                entry["prompt"] = json!(full);
                entry["seed"] = json!(seed);
                people = Some(tries);
                note = format!("new picture (seed {seed})");
                png = p;
            }
            BgKind::Prompt(t) => {
                check_cancel(cancel)?;
                let client = self.client(say)?;
                let (full, wide_for_angle) = if is_angle {
                    (t.clone(), Some(wide_png.clone().filter(|p| p.is_file()).ok_or_else(|| anyhow!("{wide_key} has no wide picture to re-angle"))?))
                } else {
                    (backdrop::background_prompt(t, &style), None)
                };
                let seed = entry["seed"].as_u64().unwrap_or_else(new_seed);
                let p = out.join("backgrounds").join(format!("{key}-{}.png", h8(&[&full, &seed.to_string()])));
                let mut sayb = |l: &str| say(&format!("{key}: {l}"));
                let result = match &wide_for_angle {
                    Some(w) => backdrop::generate_checked_by(Some(&client), seed, &p, 3, true, &mut sayb, &|c, s, path| backdrop::generate_angle(c, w, &full, s, path).map(|_| ())),
                    None => backdrop::generate_checked(Some(&client), &full, seed, &p, 3, true, &mut sayb),
                };
                let (_, tries) = result.map_err(|e| anyhow!("ComfyUI: {e}"))?;
                entry["prompt"] = json!(full);
                entry["seed"] = json!(seed);
                people = Some(tries);
                note = "new prompt drawn".to_string();
                png = p;
            }
        }
        if let Some(tries) = &people {
            let kept = std::fs::read(&png).ok();
            let best = tries.iter().find(|t| std::fs::read(&t.png).ok() == kept);
            entry["people_check"] = json!(tries.iter().enumerate().map(|(n, t)| t.describe(n + 1)).collect::<Vec<_>>());
            if let Some(b) = best {
                entry["seed"] = json!(b.seed);
            }
            entry["sharpness"] = json!(best.and_then(|t| t.sharpness).map(|v| (v * 10.0).round() / 10.0));
            entry["definition_ok"] = json!(best.is_some_and(|t| t.definition_ok()));
        }
        entry["png"] = json!(png);
        entry.as_object_mut().map(|o| o.remove("error"));
        check_cancel(cancel)?;
        let mut say2 = |l: &str| say(&format!("{key}: {l}"));
        retrace(&ctx, &key, &png, &mut entry, &mut say2)?;
        bg["backgrounds"][key.as_str()] = entry;
        write_json(&bg_path, &bg)?;
        Ok(note)
    }

    fn rebuild(&mut self, out: &Path, scenes: &[u32], say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<Vec<Rebuilt>> {
        rebuild_scenes(out, scenes, say, cancel)
    }
}

fn check_cancel(cancel: &dyn Fn() -> bool) -> Result<()> {
    if cancel() { Err(anyhow!("cancelled")) } else { Ok(()) }
}

/// Split the picture into plates, cut its objects, trace, save the asset: the producer's own steps for one background.
fn retrace(ctx: &Ctx, key: &str, png: &Path, entry: &mut Value, say: &mut dyn FnMut(&str)) -> Result<()> {
    say("splitting into plates and tracing\u{2026}");
    let picture = image::open(png).with_context(|| png.display().to_string())?.to_rgba8();
    let plates = backdrop::split_plates(&picture);
    let opts = BackdropOptions::for_style(&Style { name: ctx.style_name.clone(), suffix: ctx.style_suffix.clone() });
    let cut = ctx.objects.then(|| crate::bg_objects::extract_objects(&picture, &plates, &crate::bg_objects::ObjectOptions { trace: opts, ..Default::default() }));
    let (plates, compare_with) = match &cut {
        Some(c) => (
            backdrop::Plates { sky: c.filled_plates.sky.clone(), far: c.filled_plates.far.clone(), mid: c.filled_plates.mid.clone(), ground: c.filled_plates.ground.clone(), horizon: c.filled_plates.horizon },
            c.picture_without_objects(&picture),
        ),
        None => (plates, picture.clone()),
    };
    let mut render_check = crate::definition::RenderCheck::default();
    let checked = crate::definition::trace_checked(&plates, &compare_with, &opts, &mut render_check, say);
    let mut traced = checked.scene;
    entry["trace_notes"] = json!(checked.notes);
    entry["trace_tries"] = json!(checked.tries);
    if let Some(sc) = checked.score {
        entry["trace_score"] = json!({"colour_error": (sc.mae * 100.0).round() / 100.0, "ssim": (sc.ssim * 1000.0).round() / 1000.0, "ok": sc.ok()});
    }
    let mut library = AssetLibrary::at(&ctx.assets);
    let empty = crate::bg_objects::ObjectCut {
        objects: Vec::new(),
        filled_plates: backdrop::Plates { sky: plates.sky.clone(), far: plates.far.clone(), mid: plates.mid.clone(), ground: plates.ground.clone(), horizon: plates.horizon },
        notes: Vec::new(),
    };
    let asset_path = format!("Backgrounds/{}/{key}", ctx.series);
    let (asset, placed) = backdrop::save_asset_with_objects(&mut traced, cut.as_ref().unwrap_or(&empty), key, (picture.width(), picture.height()), &mut library, &asset_path, Some(png))
        .map_err(|e| anyhow!("saving the background asset: {e}"))?;
    if cut.is_some() {
        entry["objects"] = json!(placed.iter().map(|p| json!({"label": p.label, "n": p.n, "plate": p.plate, "bbox": p.bbox})).collect::<Vec<_>>());
    }
    entry["asset"] = json!(asset_path);
    entry["file"] = json!(asset.path);
    entry["horizon_row"] = json!(plates.horizon);
    Ok(())
}

/// Rebuild only `scenes`: the casting with the overrides on, `build_episode` for each alone, spliced into the
/// `.buzz`, `build.json` / `produce.json` brought up to date, rendered (MP4 + a still), the film joined again.
fn rebuild_scenes(out: &Path, scenes: &[u32], say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<Vec<Rebuilt>> {
    let ctx = Ctx::load(out)?;
    let ov = load(out);
    let script = std::fs::read_to_string(out.join("copilot_script.md")).context("copilot_script.md: produce the episode once first")?;
    let mut play = Screenplay::parse(&script);
    apply_to_play_with(&ov, &mut play);
    let mut casting_v = read_json(&out.join("casting.json")).ok_or_else(|| anyhow!("no casting.json"))?;
    let bg = read_json(&out.join("backgrounds.json")).unwrap_or(json!({}));
    let pose_rigs: BTreeMap<String, BTreeMap<String, String>> =
        read_json(&out.join("cast.json")).and_then(|c| serde_json::from_value(c["poses"].clone()).ok()).unwrap_or_default();
    apply_to_casting(&ov, &mut casting_v, &bg, &pose_rigs);
    write_json(&out.join("casting.json"), &casting_v)?;
    let casting = Casting::from_json(&casting_v.to_string()).map_err(anyhow::Error::msg)?;
    let mut opts = BuildOptions { width: ctx.size.0, height: ctx.size.1, fps: ctx.fps, captions: ctx.captions, ..BuildOptions::default() };
    if let Some(path) = &ctx.index
        && let Some(index) = crate::asset_sets::load_index_file(path)
    {
        opts.index = Some(Arc::new(index));
    }
    if let Some(mut plan) = read_json(&out.join("audio.json")).and_then(|v| crate::produce::plan_from_json(&v)) {
        apply_to_plan_with(&ov, &mut plan);
        opts.audio = Some(Arc::new(plan));
    }
    let assets = AssetLibrary::at(&ctx.assets);
    let buzz = out.join(format!("{}.buzz", crate::produce::file_safe(&ctx.series)));
    let mut doc = Document::open(&buzz).map_err(|e| anyhow!("opening {}: {e}", buzz.display()))?;
    let mut produce_state = read_json(&out.join("produce.json")).unwrap_or(json!({"steps": {}}));
    let mut written: Vec<stage_pins::Written> = serde_json::from_value(produce_state["written"].clone()).unwrap_or_default();
    let mut build_v = read_json(&out.join("build.json")).unwrap_or(json!({"scenes": []}));
    let mut rebuilt: Vec<(u32, String, buzz_scene::Scene, String)> = Vec::new();

    for &n in scenes {
        check_cancel(cancel)?;
        say(&format!("scene {n}: building\u{2026}"));
        let (built, reports) = build_episode(&play, &casting, &assets, &[n], &opts);
        let Some(r) = reports.first() else { bail!("scene {n}: nothing was built") };
        let Some(new_scene) = built.film().into_iter().next() else { bail!("scene {n}: nothing was built") };
        let prefix = format!("Scene {n} ");
        let Some(index) = doc.scene_names().iter().position(|name| name.starts_with(&prefix)) else {
            bail!("{} has no scene {n} to replace", buzz.display());
        };
        if !doc.replace_scene(index, new_scene.clone()) {
            bail!("could not replace scene {n}");
        }
        // The stage pins: what this build wrote for the scene is the new baseline.
        let mut bodies: Vec<(u32, String, u64, f64, f64)> = Vec::new();
        for c in &r.cast {
            let Some(body) = c.body else { continue };
            let key = c.name.trim().to_uppercase();
            let place = &casting_v["scenes"][n.to_string()]["place"][key.as_str()];
            bodies.push((n, key.clone(), body.0, place["x"].as_f64().unwrap_or(0.5), place["y"].as_f64().unwrap_or(opts.ground)));
        }
        written.retain(|w| w.scene != n);
        written.extend(stage_pins::written_for(&built, &bodies));
        // build.json: this scene's row.
        let row = json!({
            "number": r.number, "title": r.title, "frames": r.frames, "seconds": r.seconds, "set": r.set,
            "cuts": r.shots.iter().map(|s| json!({"frame": s.frame, "kind": s.kind, "on": s.on})).collect::<Vec<_>>(),
            "audio_text_lines": r.lines,
            "props": r.set_pieces.iter().filter(|p| p.starts_with("prop ")).collect::<Vec<_>>(),
            "props_not_placed": r.notes.iter().filter(|x| x.starts_with("prop ")).collect::<Vec<_>>(),
        });
        match build_v["scenes"].as_array_mut().and_then(|a| a.iter_mut().find(|s| s["number"].as_u64() == Some(n as u64))) {
            Some(slot) => *slot = row,
            None => {
                if let Some(a) = build_v["scenes"].as_array_mut() {
                    a.push(row);
                }
            }
        }
        for w in &r.warnings {
            say(&format!("scene {n}: {w}"));
        }
        rebuilt.push((n, scene_file_stem(n, &r.title), new_scene, doc.scene_names()[index].clone()));
    }
    say("saving the episode\u{2026}");
    doc.save_as(&buzz).with_context(|| format!("saving {}", buzz.display()))?;
    produce_state["written"] = json!(written);
    write_json(&out.join("produce.json"), &produce_state)?;
    let total_cuts: usize = build_v["scenes"].as_array().map(|a| a.iter().map(|s| s["cuts"].as_array().map(|c| c.iter().filter(|x| x["frame"].as_u64().unwrap_or(0) > 0).count()).unwrap_or(0)).sum()).unwrap_or(0);
    build_v["cuts"] = json!(total_cuts);
    write_json(&out.join("build.json"), &build_v)?;

    // Render: the scene's MP4 (if the episode has MP4s), its stills, one still to show.
    let mut results = Vec::new();
    for (n, stem, scene, name) in &rebuilt {
        check_cancel(cancel)?;
        if ctx.mp4 {
            say(&format!("scene {n}: rendering the MP4\u{2026}"));
            crate::produce::preview(scene.clone(), name, &out.join("scenes").join(format!("{stem}.mp4")), ctx.height)?;
        }
        if ctx.stills > 0 {
            let dir = out.join("stills");
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    if e.file_name().to_string_lossy().starts_with(&format!("{stem}_f")) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            crate::produce::stills(scene, &dir, stem, ctx.stills, ctx.height)?;
        }
        let dir = out.join("copilot");
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(&format!("{stem}_f")) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        crate::produce::stills(scene, &dir, stem, 1, ctx.height)?;
        let still = std::fs::read_dir(&dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().starts_with(&format!("{stem}_f")))));
        results.push(Rebuilt { scene: *n, still });
    }
    if ctx.mp4 {
        let stems: Vec<PathBuf> = build_v["scenes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s["frames"].as_u64().unwrap_or(0) > 0)
                    .map(|s| out.join("scenes").join(format!("{}.mp4", scene_file_stem(s["number"].as_u64().unwrap_or(0) as u32, s["title"].as_str().unwrap_or("")))))
                    .collect()
            })
            .unwrap_or_default();
        if !stems.is_empty() && stems.iter().all(|p| p.is_file()) {
            let film = out.join(format!("{}.mp4", crate::produce::file_safe(&ctx.series)));
            say("joining the scenes into the film\u{2026}");
            buzz_export::film::concat_segments(&stems, &film).with_context(|| format!("joining into {}", film.display()))?;
        }
    }
    Ok(results)
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Stub {
        fail: bool,
        rebuilt: Vec<Vec<u32>>,
    }

    impl Rebuilder for Stub {
        fn background(&mut self, _: &Path, _: &BgJob, _: &mut dyn FnMut(&str), _: &dyn Fn() -> bool) -> Result<String> {
            Ok("stub".into())
        }
        fn rebuild(&mut self, _: &Path, scenes: &[u32], _: &mut dyn FnMut(&str), _: &dyn Fn() -> bool) -> Result<Vec<Rebuilt>> {
            self.rebuilt.push(scenes.to_vec());
            if self.fail {
                bail!("stub rebuild fails");
            }
            Ok(scenes.iter().map(|s| Rebuilt { scene: *s, still: None }).collect())
        }
    }

    fn episode(dir: &Path) {
        let shots = |n: u32| {
            json!({"number": n, "location_key": "yard", "shots": [
                {"at_line": 0, "kind": "wide", "camera": "wide"},
                {"at_line": 2, "kind": "medium", "camera": "medium", "on": "RAJU"}]})
        };
        std::fs::write(dir.join("breakdown.json"), json!({"scenes": [shots(3), shots(4)]}).to_string()).unwrap();
        let bgs = json!({"backgrounds": {
            "yard": {"asset": "B/s/yard", "horizon_row": 0.4},
            "yard__low_angle": {"asset": "B/s/yard__low_angle", "horizon_row": 0.3}}});
        std::fs::write(dir.join("backgrounds.json"), bgs.to_string()).unwrap();
        let line = |item: usize, start: f64, end: f64| json!({"scene": 3, "item": item, "start": start, "end": end});
        std::fs::write(
            dir.join("audio.json"),
            json!({"path": "a.wav", "lines": [line(1, 1.0, 2.0), line(2, 3.0, 4.0)], "scene_spans": {"3": [0.0, 5.0]}, "scene_paths": {}, "notes": []}).to_string(),
        )
        .unwrap();
    }

    fn casting() -> Value {
        json!({"scenes": {"3": {"location": "yard", "shots": "auto"}, "4": {"location": "yard", "shots": "auto"}}})
    }

    fn act(dir: &Path, rb: &mut Stub, tool: &str, args: Value) -> Result<ActionResult> {
        let mut say = |_: &str| {};
        let mut run = Run { out: dir, rb, say: &mut say, cancel: &|| false };
        run_with(&mut run, tool, &args)
    }

    #[test]
    fn an_override_survives_a_full_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        episode(dir.path());
        let mut rb = Stub::default();
        act(dir.path(), &mut rb, "set_shot_camera", json!({"scene": 3, "shot": 2, "camera": "low_angle"})).unwrap();
        // A full rebuild starts again from the breakdown's casting; the overrides go on top.
        let bg = read_json(&dir.path().join("backgrounds.json")).unwrap();
        let mut fresh = casting();
        apply_to_casting_dir(dir.path(), &mut fresh, &bg, &BTreeMap::new());
        assert_eq!(fresh["scenes"]["3"]["shots"][1]["camera"], "low_angle");
        assert_eq!(fresh["scenes"]["3"]["shot_backgrounds"][1]["background"], "B/s/yard__low_angle");
        assert_eq!(fresh["scenes"]["4"]["shots"], "auto", "the other scene is untouched");
        assert!(with_overrides("# Report\n".into(), dir.path()).contains("## Overrides"));
    }

    #[test]
    fn a_picked_try_survives_a_backgrounds_redo() {
        let dir = tempfile::tempdir().unwrap();
        episode(dir.path());
        let png = dir.path().join("backgrounds").join("yard-aaaa.png");
        std::fs::create_dir_all(png.parent().unwrap()).unwrap();
        std::fs::write(&png, b"picked try").unwrap();
        let bg = json!({"backgrounds": {"yard": {"png": png}}});
        write_json(&dir.path().join("backgrounds.json"), &bg).unwrap();
        let mut rb = Stub::default();
        act(dir.path(), &mut rb, "pick_background_try", json!({"location": "yard", "angle": "wide", "n": 2})).unwrap();
        // A redo of the backgrounds step starts from nothing and draws a new picture under another name.
        std::fs::remove_file(dir.path().join("backgrounds.json")).unwrap();
        std::fs::write(&png, b"drawn again").unwrap();
        let fixed = fixed_picture(dir.path(), "yard").expect("the pick is kept");
        assert_eq!(std::fs::read(fixed).unwrap(), b"picked try");
        // A new seed draws anew.
        act(dir.path(), &mut rb, "redo_background", json!({"location": "yard", "angle": "wide"})).unwrap();
        assert!(fixed_picture(dir.path(), "yard").is_none());
    }

    #[test]
    fn retime_line_shifts_one_line_only() {
        let dir = tempfile::tempdir().unwrap();
        episode(dir.path());
        let mut rb = Stub::default();
        act(dir.path(), &mut rb, "retime_line", json!({"line": "3:2", "start_delta": 0.1, "end_delta": -0.1})).unwrap();
        let mut plan = read_json(&dir.path().join("audio.json")).and_then(|v| crate::produce::plan_from_json(&v)).unwrap();
        apply_to_plan(dir.path(), &mut plan);
        let (a, b) = (plan.lines[0], plan.lines[1]);
        assert_eq!((a.start, a.end), (1.0, 2.0), "the other line did not move");
        assert!((b.start - 3.1).abs() < 1e-9 && (b.end - 3.9).abs() < 1e-9);
        assert_eq!(rb.rebuilt, vec![vec![3]]);
    }

    #[test]
    fn a_camera_change_touches_one_shot_and_marks_one_scene_dirty() {
        let dir = tempfile::tempdir().unwrap();
        episode(dir.path());
        let mut rb = Stub { fail: true, ..Stub::default() };
        assert!(act(dir.path(), &mut rb, "set_shot_camera", json!({"scene": 3, "shot": 2, "camera": "close"})).is_err());
        let ov = load(dir.path());
        assert_eq!(ov.dirty, BTreeSet::from([3]), "only its scene is dirty (the rebuild failed, so it stays so)");
        assert_eq!(ov.shots.keys().collect::<Vec<_>>(), vec!["3"]);
        let list = &ov.shots["3"];
        assert_eq!(list[0].camera, "wide", "shot 1 is as it was");
        assert_eq!((list[1].camera.as_str(), list[1].kind.as_str(), list[1].on.as_str()), ("close", "close", "RAJU"));
        assert_eq!(rb.rebuilt, vec![vec![3]]);
    }
}
