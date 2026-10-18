//! **The producer** (PRODUCER_PLAN § P6): a script, a recording and a folder
//! of character art in, a finished episode out.
//!
//! One call joins the stages built before it:
//!
//! 1. **parse** the screenplay (`buzz_act::screenplay`);
//! 2. **breakdown** (`buzz_ai::breakdown`, P1): places, settings, background
//!    prompts, who stands where, lights — through the model when one answers,
//!    else the script's own rules (`breakdown.json`);
//! 3. **cast** (`crate::cast_build`, P4): a layer rig per character of the
//!    chosen scenes, from the folder, kept when the library has one, drawn by
//!    ComfyUI when there is no picture (`cast.json`);
//! 4. **backgrounds** (`crate::backdrop`, P3): one Z-Image picture per
//!    location key, split into plates, traced, saved as
//!    `Backgrounds/<series>/<key>` (`backgrounds.json`, `backgrounds/`);
//! 5. **audio** (`buzz_act::audio_align`, P2): the recording's lines found
//!    (`audio.json`);
//! 6. **build**: the casting file (`casting.json`: the breakdown's, the cast,
//!    the background assets, `"shots": "auto"`, the lighting, title cards)
//!    and `episode::build_episode` into `<series>.buzz`;
//! 7. **render**: a preview MP4 per scene through `headless::render` (each
//!    with its sound), joined into `<series>.mp4`, and stills when asked;
//! 8. **report** (`report.md`).
//!
//! **Resumable.** `<out>/produce.json` records each finished step with a hash
//! of its inputs; a run skips a step whose inputs are unchanged and whose
//! outputs are still there. `fresh` redoes everything. Cancel is checked
//! between steps, and between the scenes, characters and backgrounds inside
//! one.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use buzz_act::audio_align::{self, AudioLine, AudioPlan, Heard};
use buzz_act::screenplay::{Item, Screenplay, ScriptScene, Voice};
use buzz_ai::breakdown::{self, Breakdown, BreakdownHints, CharacterBrief};
use buzz_doc::{AssetLibrary, Document};
use buzz_render::GpuPreference;
use serde_json::{Value, json};

use crate::backdrop::{self, BackdropOptions, Style};
use crate::cast_build::{self, CastRequest};
use crate::comfy::ComfyClient;
use crate::episode::{BuildOptions, Casting, build_episode, report_markdown, scene_file_stem};
use crate::headless::{RenderJob, render};

/// What to produce.
#[derive(Debug, Clone)]
pub struct ProduceRequest {
    pub script: PathBuf,
    /// The dialogue recording of the chosen scenes, in script order.
    pub audio: Option<PathBuf>,
    /// `<dir>/<Name>/*.png` and the like (`cast_build::scan`).
    pub characters_dir: Option<PathBuf>,
    pub style: Style,
    pub series: String,
    /// Where the story lives ("rural Bihar, India, late 1990s"): ends every
    /// background prompt.
    pub world: String,
    pub out: PathBuf,
    /// The script's scene numbers; `None`: every scene.
    pub scenes: Option<Vec<u32>>,
    /// The asset library; `None`: the user's.
    pub assets: Option<PathBuf>,
    /// An asset index to dress sets from (AS3), read only; `None`: the library's own.
    pub index: Option<PathBuf>,
    /// The stage, in document units.
    pub size: (f64, f64),
    pub fps: f64,
    /// Generate backgrounds with ComfyUI (true), or dress sets from the assets only.
    pub backgrounds: bool,
    pub mp4: bool,
    /// Rendered height of the MP4s and stills.
    pub height: u32,
    /// Evenly spaced stills per scene into `<out>/stills/` (0: none).
    pub stills: u32,
    /// Start the portable ComfyUI when nothing answers.
    pub launch_comfy: bool,
    /// No model for the breakdown: the script's own rules.
    pub offline: bool,
    /// Redo every step.
    pub fresh: bool,
    /// Redo this step and every later one (the earlier ones are skipped when
    /// unchanged).
    pub redo: Option<Step>,
}

impl ProduceRequest {
    /// The defaults the dialog starts from.
    pub fn new(script: PathBuf, out: PathBuf) -> Self {
        Self {
            script,
            audio: None,
            characters_dir: None,
            style: Style::comic(),
            series: String::new(),
            world: String::new(),
            out,
            scenes: None,
            assets: None,
            index: None,
            size: (1920.0, 1080.0),
            fps: 24.0,
            backgrounds: true,
            mp4: true,
            height: 1080,
            stills: 0,
            launch_comfy: true,
            offline: false,
            fresh: false,
            redo: None,
        }
    }
}

/// The producer's steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Parse,
    Breakdown,
    Cast,
    Backgrounds,
    Audio,
    Build,
    Render,
    Report,
}

impl Step {
    pub const ALL: [Step; 8] = [
        Step::Parse,
        Step::Breakdown,
        Step::Cast,
        Step::Backgrounds,
        Step::Audio,
        Step::Build,
        Step::Render,
        Step::Report,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Step::Parse => "Parse",
            Step::Breakdown => "Breakdown",
            Step::Cast => "Cast",
            Step::Backgrounds => "Background",
            Step::Audio => "Audio",
            Step::Build => "Build",
            Step::Render => "Render",
            Step::Report => "Report",
        }
    }

    /// The key in `produce.json` and in [`ProduceReport::seconds`].
    pub fn key(self) -> &'static str {
        match self {
            Step::Parse => "parse",
            Step::Breakdown => "breakdown",
            Step::Cast => "cast",
            Step::Backgrounds => "backgrounds",
            Step::Audio => "audio",
            Step::Build => "build",
            Step::Render => "render",
            Step::Report => "report",
        }
    }

    /// The step named by its [`Step::key`].
    pub fn from_key(key: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.key().eq_ignore_ascii_case(key.trim()))
    }

    pub fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// What a run made.
#[derive(Debug, Clone, Default)]
pub struct ProduceReport {
    pub buzz: PathBuf,
    pub mp4: Option<PathBuf>,
    pub scene_mp4s: Vec<PathBuf>,
    pub stills: Vec<PathBuf>,
    pub report: PathBuf,
    /// Seconds per step key (0 for a skipped step).
    pub seconds: BTreeMap<String, f64>,
    /// Steps skipped because their inputs were unchanged.
    pub skipped_steps: Vec<String>,
    pub breakdown_backend: String,
    /// Script name → Cast rig.
    pub cast: BTreeMap<String, String>,
    pub generated_characters: Vec<String>,
    /// Location keys whose background was generated (or reused) as an asset.
    pub backgrounds: Vec<String>,
    pub lines_from_audio: usize,
    pub lines_from_text: usize,
    /// Camera cuts written (the opening wide not counted).
    pub cuts: usize,
    pub warnings: Vec<String>,
}

// ------------------------------------------------------------------ hashing and state

fn hash_parts(parts: &[&[u8]]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p.len().hash(&mut h);
        h.write(p);
    }
    format!("{:016x}", h.finish())
}

fn hash_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => hash_parts(&[path.to_string_lossy().as_bytes(), &bytes]),
        Err(_) => hash_parts(&[path.to_string_lossy().as_bytes(), b"missing"]),
    }
}

/// Names, sizes and times of every file two levels down: changes when a
/// picture is added, removed or redrawn.
fn hash_dir(dir: Option<&Path>) -> String {
    let Some(dir) = dir else {
        return hash_parts(&[b"none"]);
    };
    let mut lines: Vec<String> = Vec::new();
    let walk = |d: &Path, lines: &mut Vec<String>| {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if let Ok(m) = e.metadata() {
                    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                    lines.push(format!("{}|{}|{}|{}", p.display(), m.is_dir(), m.len(), t));
                }
            }
        }
    };
    walk(dir, &mut lines);
    let subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    for s in subdirs {
        walk(&s, &mut lines);
    }
    lines.sort();
    hash_parts(&[lines.join("\n").as_bytes()])
}

/// `<out>/produce.json`: each finished step, its inputs' hash and how long it took.
struct State {
    path: PathBuf,
    value: Value,
}

impl State {
    fn load(out: &Path, fresh: bool) -> Self {
        let path = out.join("produce.json");
        let value = if fresh {
            json!({"steps": {}})
        } else {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .filter(|v| v["steps"].is_object())
                .unwrap_or_else(|| json!({"steps": {}}))
        };
        Self { path, value }
    }

    fn done(&self, step: Step, hash: &str) -> bool {
        self.value["steps"][step.key()]["hash"].as_str() == Some(hash)
    }

    fn note(&self, step: Step) -> String {
        self.value["steps"][step.key()]["note"].as_str().unwrap_or_default().to_string()
    }

    fn finish(&mut self, step: Step, hash: &str, seconds: f64, note: &str) -> Result<()> {
        self.value["steps"][step.key()] = json!({
            "hash": hash,
            "seconds": (seconds * 10.0).round() / 10.0,
            "note": note,
        });
        std::fs::write(&self.path, serde_json::to_string_pretty(&self.value)?)
            .with_context(|| format!("writing {}", self.path.display()))
    }

    /// Forget a step (its outputs went missing, or it failed part way).
    fn forget(&mut self, step: Step) {
        if let Some(steps) = self.value["steps"].as_object_mut() {
            steps.remove(step.key());
        }
    }
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?).with_context(|| format!("writing {}", path.display()))
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
}

/// Lowercase letters and digits only: `SHARMA JI` and `Sharmaji` match.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// A file-safe name: `Chirailiya`, `My_Show`.
fn file_safe(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { "episode".into() } else { s }
}

// ------------------------------------------------------------------ ComfyUI, once

/// ComfyUI asked for once, when a step first needs it.
struct Comfy {
    launch: bool,
    tried: bool,
    client: Option<ComfyClient>,
    error: Option<String>,
}

impl Comfy {
    fn get(&mut self, progress: &mut dyn FnMut(Step, &str), step: Step) -> Option<ComfyClient> {
        if !self.tried {
            self.tried = true;
            progress(step, if self.launch { "finding ComfyUI (starting it if it is not running)\u{2026}" } else { "finding ComfyUI\u{2026}" });
            let started = Instant::now();
            match ComfyClient::ensure_running(self.launch, Duration::from_secs(600)) {
                Ok(c) => {
                    progress(step, &format!("ComfyUI at {} ({:.0} s to reach)", c.base_url, started.elapsed().as_secs_f64()));
                    self.client = Some(c);
                }
                Err(e) => {
                    progress(step, &format!("no ComfyUI: {e}"));
                    self.error = Some(e);
                }
            }
        }
        self.client.clone()
    }
}

// ------------------------------------------------------------------ the breakdown's model

/// LM Studio's model for a breakdown: a loaded Gemma, else any Gemma, else the
/// first chat model (`examples/breakdown.rs`).
fn lmstudio_model(backend: &buzz_ai::llm::Backend) -> Option<(String, &'static str)> {
    let chat: Vec<String> = backend
        .list_models()
        .ok()?
        .into_iter()
        .filter(|id| !id.to_lowercase().contains("embed"))
        .collect();
    let loaded: Vec<String> = ureq::get(&format!("{}/api/v0/models", backend.base_url().trim_end_matches("/v1")))
        .timeout(Duration::from_secs(10))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .and_then(|v| {
            v["data"].as_array().map(|list| {
                list.iter()
                    .filter(|m| m["state"].as_str() == Some("loaded"))
                    .filter_map(|m| m["id"].as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default();
    let gemma = |id: &&String| id.to_lowercase().contains("gemma");
    if let Some(m) = loaded.iter().filter(|id| chat.contains(id)).find(gemma) {
        Some((m.clone(), "a loaded Gemma"))
    } else if let Some(m) = chat.iter().find(gemma) {
        Some((m.clone(), "the first Gemma listed"))
    } else {
        chat.first().cloned().map(|m| (m, "the first chat model"))
    }
}

// ------------------------------------------------------------------ audio plan as JSON

fn plan_json(p: &AudioPlan) -> Value {
    json!({
        "path": p.path,
        "lines": p.lines.iter().map(|l| json!({"scene": l.scene, "item": l.item, "start": l.start, "end": l.end})).collect::<Vec<_>>(),
        "scene_spans": p.scene_spans.iter().map(|(k, (a, b))| (k.to_string(), json!([a, b]))).collect::<serde_json::Map<_, _>>(),
        "scene_paths": p.scene_paths.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
        "notes": p.notes,
    })
}

fn plan_from_json(v: &Value) -> Option<AudioPlan> {
    let mut plan = AudioPlan { path: PathBuf::from(v["path"].as_str()?), ..AudioPlan::default() };
    for l in v["lines"].as_array()? {
        plan.lines.push(AudioLine {
            scene: l["scene"].as_u64()? as u32,
            item: l["item"].as_u64()? as usize,
            start: l["start"].as_f64()?,
            end: l["end"].as_f64()?,
        });
    }
    for (k, span) in v["scene_spans"].as_object()? {
        plan.scene_spans.insert(k.parse().ok()?, (span[0].as_f64()?, span[1].as_f64()?));
    }
    if let Some(paths) = v["scene_paths"].as_object() {
        for (k, p) in paths {
            plan.scene_paths.insert(k.parse().ok()?, PathBuf::from(p.as_str()?));
        }
    }
    plan.notes = v["notes"].as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(String::from)).collect()).unwrap_or_default();
    Some(plan)
}

// ------------------------------------------------------------------ the producer

fn check(cancel: &dyn Fn() -> bool) -> Result<()> {
    if cancel() { Err(anyhow!("cancelled")) } else { Ok(()) }
}

/// The rig's view rank: front first, then 3-4, back, side.
fn view_rank(rig: &str) -> u8 {
    if rig.ends_with(" 3-4") {
        1
    } else if rig.ends_with(" Back") {
        2
    } else if rig.ends_with(" Side") {
        3
    } else {
        0
    }
}

fn base_name(rig: &str) -> &str {
    for suffix in [" 3-4", " Back", " Side"] {
        if let Some(b) = rig.strip_suffix(suffix) {
            return b;
        }
    }
    rig
}

/// **Produce an episode.** See the module note.
pub fn produce(req: &ProduceRequest, progress: &mut dyn FnMut(Step, &str), cancel: &dyn Fn() -> bool) -> Result<ProduceReport> {
    let out = &req.out;
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let mut state = State::load(out, req.fresh);
    if let Some(from) = req.redo {
        for step in Step::ALL.into_iter().filter(|s| *s >= from) {
            state.forget(step);
        }
    }
    let mut report = ProduceReport::default();
    let series = if req.series.trim().is_empty() { "Episode".to_string() } else { req.series.trim().to_string() };
    let series_file = file_safe(&series);
    let assets_root: PathBuf = match &req.assets {
        Some(dir) => dir.clone(),
        None => AssetLibrary::user().root().map(Path::to_path_buf).ok_or_else(|| anyhow!("there is no asset library folder on this machine"))?,
    };
    let mut comfy = Comfy { launch: req.launch_comfy, tried: false, client: None, error: None };

    // -- 1. parse ------------------------------------------------------------------------
    let t = Instant::now();
    progress(Step::Parse, &format!("reading {}", req.script.display()));
    let bytes = std::fs::read(&req.script).with_context(|| format!("reading {}", req.script.display()))?;
    let play = Screenplay::parse(&String::from_utf8_lossy(&bytes));
    if play.scenes.is_empty() {
        bail!("{}: no scenes found (they need '## SCENE n — TITLE' headings)", req.script.display());
    }
    let numbers: Vec<u32> = match &req.scenes {
        Some(list) if !list.is_empty() => list.clone(),
        _ => play.scenes.iter().map(|s| s.number).collect(),
    };
    for n in &numbers {
        if play.scene(*n).is_none() {
            bail!("the script has no scene {n}");
        }
    }
    let chosen: Vec<&ScriptScene> = numbers.iter().filter_map(|n| play.scene(*n)).collect();
    let script_hash = hash_parts(&[&bytes]);
    let scenes_text = format!("{numbers:?}");
    progress(Step::Parse, &format!("{} scenes in the script; producing {}", play.scenes.len(), scenes_text));
    report.seconds.insert("parse".into(), t.elapsed().as_secs_f64());
    state.finish(Step::Parse, &script_hash, t.elapsed().as_secs_f64(), "")?;
    check(cancel)?;

    // -- 2. breakdown --------------------------------------------------------------------
    let t = Instant::now();
    let art = req.characters_dir.as_deref().map(cast_build::scan).unwrap_or_default();
    let bd_hash = hash_parts(&[
        script_hash.as_bytes(),
        req.style.name.as_bytes(),
        series.as_bytes(),
        req.world.as_bytes(),
        if req.offline { b"offline" } else { b"model" },
    ]);
    let bd_path = out.join("breakdown.json");
    let existing: Option<Breakdown> = if state.done(Step::Breakdown, &bd_hash) {
        read_json(&bd_path).and_then(|v| serde_json::from_value(v).ok())
    } else {
        None
    };
    let bd = match existing {
        Some(mut b) => {
            // Tidied again: a breakdown written before a `tidy` rule (P6:
            // no people in background prompts) gets it; `tidy` is idempotent.
            let before = serde_json::to_value(&b).ok();
            breakdown::tidy(&mut b, &play);
            if serde_json::to_value(&b).ok() != before {
                write_json(&bd_path, &b)?;
            }
            report.breakdown_backend = state.note(Step::Breakdown);
            report.skipped_steps.push("breakdown".into());
            progress(Step::Breakdown, &format!("unchanged; {} kept ({})", bd_path.display(), report.breakdown_backend));
            report.seconds.insert("breakdown".into(), 0.0);
            b
        }
        None => {
            let hints = BreakdownHints {
                style: format!("{}, light humour", req.style.name),
                known_characters: art.iter().map(|a| a.name.clone()).collect(),
                series: series.clone(),
                world: req.world.clone(),
            };
            let total = play.scenes.len();
            let mut say = |line: &str| progress(Step::Breakdown, line);
            let (b, backend) = if req.offline {
                say(&format!("offline: the script's own rules, {total} scenes"));
                (breakdown::breakdown_offline(&play, &hints), "offline (asked)".to_string())
            } else {
                let claude = buzz_ai::ai_config::use_claude();
                let backend = if claude { buzz_ai::llm::Backend::default_claude_api() } else { buzz_ai::llm::Backend::default_openai() };
                let model = if claude {
                    Some((buzz_ai::ai_config::claude_model(), "the saved Claude model"))
                } else {
                    lmstudio_model(&backend)
                };
                match model {
                    Some((model, why)) if !model.is_empty() => {
                        buzz_ai::llm::set_low_reasoning(true);
                        say(&format!("asking {model} ({why}, {}) about {total} scenes, four at a time\u{2026}", backend.display_name()));
                        let mut done = 0usize;
                        let mut inner = |line: &str| {
                            if line.starts_with("scenes [") && line.contains("asking the model") {
                                done += 1;
                                let first = ((done - 1) * 4 + 1).min(total);
                                say(&format!("scenes {first}\u{2013}{} of {total}: {line}", (done * 4).min(total)));
                            } else {
                                say(line);
                            }
                        };
                        // P8-C2: Cancel gives up a model call in flight.
                        let Some(b) = breakdown::breakdown_cancellable(&backend, &model, &play, &hints, &mut inner, cancel) else {
                            bail!("cancelled");
                        };
                        (b, format!("{} {model}", backend.display_name()))
                    }
                    _ => {
                        say("no model answers (LM Studio / Claude); the script's own rules");
                        (breakdown::breakdown_offline(&play, &hints), "offline (no model answered)".to_string())
                    }
                }
            };
            write_json(&bd_path, &b)?;
            let secs = t.elapsed().as_secs_f64();
            report.seconds.insert("breakdown".into(), secs);
            report.breakdown_backend = backend.clone();
            state.finish(Step::Breakdown, &bd_hash, secs, &backend)?;
            b
        }
    };
    check(cancel)?;

    // The characters of the chosen scenes, and the scene briefs.
    let briefs: Vec<&breakdown::SceneBrief> = bd.scenes.iter().filter(|s| numbers.contains(&s.number)).collect();
    let mut on_stage: BTreeSet<String> = BTreeSet::new();
    for s in &briefs {
        on_stage.extend(s.present.iter().map(|n| norm(n)));
    }
    for sc in &chosen {
        for item in &sc.items {
            if let Item::Dialogue(d) = item
                && d.voice != Voice::VoiceOver
            {
                on_stage.insert(norm(&d.speaker));
            }
        }
    }
    let wanted: Vec<CharacterBrief> = bd
        .characters
        .iter()
        .filter(|c| !c.voice_only)
        .filter(|c| on_stage.contains(&norm(&c.name)) || c.aliases.iter().any(|a| on_stage.contains(&norm(a))))
        .cloned()
        .collect();

    // -- 3. cast -------------------------------------------------------------------------
    let t = Instant::now();
    let cast_hash = hash_parts(&[
        bd_hash.as_bytes(),
        hash_dir(req.characters_dir.as_deref()).as_bytes(),
        assets_root.to_string_lossy().as_bytes(),
        scenes_text.as_bytes(),
        req.style.name.as_bytes(),
    ]);
    let cast_path = out.join("cast.json");
    let cast_value: Value = match read_json(&cast_path).filter(|_| state.done(Step::Cast, &cast_hash)) {
        Some(v) => {
            report.skipped_steps.push("cast".into());
            progress(Step::Cast, "unchanged; cast.json kept");
            report.seconds.insert("cast".into(), 0.0);
            v
        }
        None => {
            let mut cast: BTreeMap<String, String> = BTreeMap::new();
            let mut views: BTreeMap<String, Vec<String>> = BTreeMap::new();
            let mut how: BTreeMap<String, String> = BTreeMap::new();
            let mut generated: Vec<(String, PathBuf)> = Vec::new();
            let mut skipped: Vec<(String, String)> = Vec::new();
            let mut spill: Vec<(String, f64, f64)> = Vec::new();
            let mut notes: Vec<String> = Vec::new();
            for (i, brief) in wanted.iter().enumerate() {
                check(cancel)?;
                let keys: BTreeSet<String> =
                    std::iter::once(&brief.name).chain(brief.aliases.iter()).map(|n| norm(n)).filter(|k| !k.is_empty()).collect();
                let library = AssetLibrary::at(&assets_root);
                let mut rigs: Vec<String> = library
                    .assets()
                    .iter()
                    .filter(|a| a.folder == "Cast" && keys.contains(&norm(base_name(&a.name))))
                    .map(|a| a.name.clone())
                    .collect();
                let mut why = "already in Cast (kept)".to_string();
                if rigs.is_empty() {
                    let has_art = art.iter().any(|a| keys.contains(&norm(&a.name)));
                    let client = if !has_art && brief.importance <= 2 { comfy.get(progress, Step::Cast) } else { None };
                    progress(
                        Step::Cast,
                        &format!(
                            "character {}/{}: {} \u{2014} {}\u{2026}",
                            i + 1,
                            wanted.len(),
                            brief.name,
                            if has_art { "building rigs from the folder" } else { "no picture; drawing one with ComfyUI" }
                        ),
                    );
                    let creq = CastRequest {
                        characters_dir: req.characters_dir.clone(),
                        wanted: vec![brief.clone()],
                        style: req.style.clone(),
                        replace: false,
                        comfy: client,
                        out: out.join("cast"),
                    };
                    let mut say = |line: &str| progress(Step::Cast, line);
                    let r = cast_build::build_cast(&library, &creq, &mut say);
                    rigs.extend(r.built.iter().map(|(_, rig, _)| rig.clone()));
                    // A rig kept because it exists is reported as skipped.
                    for (_, line) in &r.skipped {
                        if let Some(rest) = line.strip_prefix("kept: Cast/")
                            && let Some(rig) = rest.strip_suffix(" exists (replace to rebuild)")
                        {
                            rigs.push(rig.to_string());
                        }
                    }
                    generated.extend(r.generated.iter().cloned());
                    skipped.extend(r.skipped.iter().filter(|(_, l)| !l.starts_with("kept: ")).cloned());
                    spill.extend(r.spill.iter().cloned());
                    notes.extend(r.notes.iter().cloned());
                    why = if r.generated.is_empty() { "built from the folder".into() } else { "drawn by ComfyUI, then built".into() };
                } else {
                    progress(Step::Cast, &format!("character {}/{}: {} \u{2014} {} already in Cast", i + 1, wanted.len(), brief.name, rigs.join(", ")));
                }
                rigs.sort_by_key(|r| (view_rank(r), r.clone()));
                rigs.dedup();
                if let Some(first) = rigs.first() {
                    cast.insert(brief.name.clone(), first.clone());
                    how.insert(brief.name.clone(), why);
                    views.insert(brief.name.clone(), rigs.clone());
                } else {
                    skipped.push((brief.name.clone(), "no rig: no picture in the folder and none drawn".into()));
                }
            }
            let v = json!({
                "cast": cast,
                "views": views,
                "how": how,
                "generated": generated,
                "skipped": skipped,
                "spill": spill,
                "notes": notes,
            });
            write_json(&cast_path, &v)?;
            let secs = t.elapsed().as_secs_f64();
            report.seconds.insert("cast".into(), secs);
            state.finish(Step::Cast, &cast_hash, secs, "")?;
            v
        }
    };
    let cast_names: BTreeMap<String, String> = cast_value["cast"]
        .as_object()
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default();
    report.cast = cast_names.clone();
    report.generated_characters = cast_value["generated"]
        .as_array()
        .map(|a| a.iter().filter_map(|g| g[0].as_str().map(String::from)).collect())
        .unwrap_or_default();
    check(cancel)?;

    // -- 4. backgrounds ------------------------------------------------------------------
    let t = Instant::now();
    // The casting's prompts: place, time, the model's picture, the world.
    let casting_base = breakdown::casting_value(&bd, &cast_names);
    let mut keys: Vec<(String, String)> = Vec::new();
    for s in &briefs {
        let key = s.location_key.trim().to_string();
        if key.is_empty() || keys.iter().any(|(k, _)| *k == key) {
            continue;
        }
        let prompt = casting_base["scenes"][s.number.to_string()]["background_prompt"].as_str().unwrap_or(&s.background_prompt).to_string();
        keys.push((key, prompt));
    }
    let bg_hash = hash_parts(&[
        bd_hash.as_bytes(),
        req.style.suffix.as_bytes(),
        series.as_bytes(),
        scenes_text.as_bytes(),
        if req.backgrounds { b"generate" } else { b"assets" },
        assets_root.to_string_lossy().as_bytes(),
    ]);
    let bg_path = out.join("backgrounds.json");
    let asset_of = |key: &str| format!("Backgrounds/{series}/{key}");
    let library_has = |path: &str| {
        let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
        AssetLibrary::at(&assets_root).assets().iter().any(|a| a.folder == folder && a.name == name)
    };
    let kept_bg = read_json(&bg_path).filter(|v| {
        state.done(Step::Backgrounds, &bg_hash)
            && v["backgrounds"].as_object().is_some_and(|m| m.values().all(|e| e["asset"].as_str().is_none_or(library_has)))
    });
    let bg_value: Value = match kept_bg {
        Some(v) => {
            report.skipped_steps.push("backgrounds".into());
            progress(Step::Backgrounds, "unchanged; backgrounds.json kept");
            report.seconds.insert("backgrounds".into(), 0.0);
            v
        }
        None => {
            let mut done = serde_json::Map::new();
            if !req.backgrounds {
                progress(Step::Backgrounds, "off: the sets are dressed from your assets (AS3)");
            }
            for (i, (key, prompt)) in keys.iter().enumerate().filter(|_| req.backgrounds) {
                check(cancel)?;
                let started = Instant::now();
                let full = backdrop::background_prompt(prompt, &req.style);
                // The prompt's hash in the name: a changed prompt draws anew.
                let png = out.join("backgrounds").join(format!("{key}-{}.png", &hash_parts(&[full.as_bytes()])[..8]));
                let seed = u64::from_str_radix(&hash_parts(&[key.as_bytes()])[..12], 16).unwrap_or(1);
                let mut entry = json!({"prompt": full, "seed": seed, "png": png});
                // ComfyUI only when a try is not on disk yet.
                let first_try = png.with_file_name(format!("{}.try1.png", png.file_stem().unwrap_or_default().to_string_lossy()));
                let client = if first_try.is_file() { comfy.client.clone() } else { comfy.get(progress, Step::Backgrounds) };
                progress(Step::Backgrounds, &format!("{}/{}: {key} (ComfyUI, about 60 s a picture, then a people check)\u{2026}", i + 1, keys.len()));
                let label = format!("{}/{}: {key}: ", i + 1, keys.len());
                let mut say = |line: &str| progress(Step::Backgrounds, &format!("{label}{line}"));
                match backdrop::generate_checked(client.as_ref(), &full, seed, &png, 3, true, &mut say) {
                    Ok((_, tries)) => {
                        let notes: Vec<String> = tries.iter().enumerate().map(|(n, t)| t.describe(n + 1)).collect();
                        let kept = std::fs::read(&png).ok();
                        let best = tries.iter().find(|t| std::fs::read(&t.png).ok() == kept);
                        entry["people_check"] = json!(notes);
                        entry["seed"] = json!(best.map(|t| t.seed).unwrap_or(seed));
                        entry["people_kept"] = json!(best.and_then(|t| t.people.as_ref().map(|p| p.len())));
                    }
                    Err(e) => {
                        let why = comfy.error.as_deref().map(|c| format!(" ({c})")).unwrap_or_default();
                        entry["error"] = json!(format!("ComfyUI: {e}{why}"));
                        done.insert(key.clone(), entry);
                        continue;
                    }
                }
                let generated_s = started.elapsed().as_secs_f64();
                check(cancel)?;
                progress(Step::Backgrounds, &format!("{}/{}: {key}: splitting into plates and tracing\u{2026}", i + 1, keys.len()));
                let picture = image::open(&png).with_context(|| png.display().to_string())?.to_rgba8();
                let plates = backdrop::split_plates(&picture);
                let traced = backdrop::trace_plates(&plates, &BackdropOptions::for_style(&req.style));
                let mut library = AssetLibrary::at(&assets_root);
                match backdrop::save_asset(&traced, &mut library, &asset_of(key), Some(&png)) {
                    Ok(asset) => {
                        entry["asset"] = json!(asset_of(key));
                        entry["file"] = json!(asset.path);
                        entry["horizon_row"] = json!(plates.horizon);
                    }
                    Err(e) => entry["error"] = json!(e),
                }
                entry["generate_s"] = json!((generated_s * 10.0).round() / 10.0);
                entry["total_s"] = json!((started.elapsed().as_secs_f64() * 10.0).round() / 10.0);
                done.insert(key.clone(), entry);
            }
            let v = json!({"backgrounds": done});
            write_json(&bg_path, &v)?;
            let secs = t.elapsed().as_secs_f64();
            report.seconds.insert("backgrounds".into(), secs);
            let all_ok = done.values().all(|e| e.get("error").is_none());
            if all_ok {
                state.finish(Step::Backgrounds, &bg_hash, secs, "")?;
            } else {
                // A failed background is tried again next run.
                state.forget(Step::Backgrounds);
            }
            v
        }
    };
    let mut bg_assets: BTreeMap<String, String> = BTreeMap::new();
    if let Some(m) = bg_value["backgrounds"].as_object() {
        for (k, e) in m {
            if let Some(a) = e["asset"].as_str() {
                bg_assets.insert(k.clone(), a.to_string());
            }
            if let Some(err) = e["error"].as_str() {
                report.warnings.push(format!("background {k}: {err}; the set is dressed from your assets instead"));
            }
        }
    }
    report.backgrounds = bg_assets.keys().cloned().collect();
    check(cancel)?;

    // -- 5. audio ------------------------------------------------------------------------
    let t = Instant::now();
    let audio_hash = hash_parts(&[
        req.audio.as_deref().map(hash_file).unwrap_or_default().as_bytes(),
        script_hash.as_bytes(),
        scenes_text.as_bytes(),
    ]);
    let audio_path = out.join("audio.json");
    let plan: Option<AudioPlan> = match &req.audio {
        None => {
            progress(Step::Audio, "no recording: timing and lip sync from the words");
            report.seconds.insert("audio".into(), 0.0);
            None
        }
        Some(audio) => match read_json(&audio_path).filter(|_| state.done(Step::Audio, &audio_hash)).and_then(|v| plan_from_json(&v)) {
            Some(p) => {
                report.skipped_steps.push("audio".into());
                progress(Step::Audio, "unchanged; audio.json kept");
                report.seconds.insert("audio".into(), 0.0);
                Some(p)
            }
            None => {
                progress(Step::Audio, &format!("opening {}", audio.display()));
                let clip = buzz_audio::Clip::open(audio).with_context(|| format!("opening {}", audio.display()))?;
                let heard: Option<Vec<Heard>> = match buzz_ai::transcribe::installed_whisper() {
                    Ok(_) => {
                        progress(Step::Audio, &format!("Whisper is listening ({:.0} s of audio)\u{2026}", clip.duration_seconds()));
                        match buzz_ai::transcribe::transcribe_clip(&clip, &mut |_, _| {}, cancel) {
                            Ok(t) => Some(t.segments.into_iter().map(|s| Heard { start: s.start, end: s.end, text: s.text }).collect()),
                            Err(e) => {
                                check(cancel)?;
                                progress(Step::Audio, &format!("no transcript ({e}); durations only"));
                                None
                            }
                        }
                    }
                    Err(e) => {
                        progress(Step::Audio, &format!("no Whisper ({e}); durations only"));
                        None
                    }
                };
                progress(Step::Audio, "finding the lines in the recording\u{2026}");
                let p = audio_align::plan(&chosen, audio, &clip, heard.as_deref());
                let mut v = plan_json(&p);
                v["transcript"] = json!(heard.is_some());
                write_json(&audio_path, &v)?;
                let secs = t.elapsed().as_secs_f64();
                report.seconds.insert("audio".into(), secs);
                state.finish(Step::Audio, &audio_hash, secs, if heard.is_some() { "whisper + durations" } else { "durations" })?;
                Some(p)
            }
        },
    };
    if let Some(p) = &plan {
        for n in &p.notes {
            report.warnings.push(format!("audio: {n}"));
        }
        progress(Step::Audio, &format!("{} line(s) found in the recording", p.lines.len()));
    }
    check(cancel)?;

    // -- 6. casting + build --------------------------------------------------------------
    let t = Instant::now();
    let mut casting_v = casting_base.clone();
    casting_v["title_cards"] = json!(true);
    if let Some(scenes) = casting_v["scenes"].as_object_mut() {
        for s in &briefs {
            if let Some(set) = scenes.get_mut(&s.number.to_string()) {
                set["shots"] = json!("auto");
                if let Some(asset) = bg_assets.get(s.location_key.trim()) {
                    set["background"] = json!([asset]);
                }
            }
        }
    }
    let casting_path = out.join("casting.json");
    write_json(&casting_path, &casting_v)?;
    let casting = Casting::from_json(&casting_v.to_string()).map_err(anyhow::Error::msg)?;

    let buzz = out.join(format!("{series_file}.buzz"));
    let build_path = out.join("build.json");
    let build_hash = hash_parts(&[
        casting_v.to_string().as_bytes(),
        script_hash.as_bytes(),
        audio_hash.as_bytes(),
        cast_hash.as_bytes(),
        bg_hash.as_bytes(),
        format!("{:?} {} {:?}", req.size, req.fps, req.index).as_bytes(),
    ]);
    let mut doc: Option<Document> = None;
    let build_value: Value = match read_json(&build_path).filter(|_| state.done(Step::Build, &build_hash) && buzz.is_file()) {
        Some(v) => {
            report.skipped_steps.push("build".into());
            progress(Step::Build, &format!("unchanged; {} kept", buzz.display()));
            report.seconds.insert("build".into(), 0.0);
            v
        }
        None => {
            let assets = AssetLibrary::at(&assets_root);
            let mut opts = BuildOptions { width: req.size.0, height: req.size.1, fps: req.fps, ..BuildOptions::default() };
            if let Some(path) = &req.index {
                match crate::asset_sets::load_index_file(path) {
                    Some(index) => opts.index = Some(Arc::new(index)),
                    None => report.warnings.push(format!("{}: not an asset index; the library's own is used", path.display())),
                }
            }
            if let Some(p) = &plan {
                opts.audio = Some(Arc::new(p.clone()));
            }
            progress(Step::Build, &format!("building {} scene(s) from {} asset(s)\u{2026}", numbers.len(), assets.len()));
            let (mut built, reports) = build_episode(&play, &casting, &assets, &numbers, &opts);
            check(cancel)?;
            progress(Step::Build, &format!("saving {}", buzz.display()));
            built.save_as(&buzz).with_context(|| format!("saving {}", buzz.display()))?;
            let title = if play.title.is_empty() { series.as_str() } else { play.title.as_str() };
            let md = report_markdown(title, &reports, opts.fps);
            let mut scenes = Vec::new();
            let (mut from_audio, mut from_text, mut cuts) = (0usize, 0usize, 0usize);
            let mut warnings: Vec<String> = Vec::new();
            let mut skipped: Vec<String> = Vec::new();
            for r in &reports {
                from_audio += r.notes.iter().filter(|n| n.starts_with("from audio:")).count();
                from_text += r.notes.iter().filter(|n| n.starts_with("from text:")).count();
                cuts += r.shots.iter().filter(|s| s.frame > 0).count();
                warnings.extend(r.warnings.iter().map(|w| format!("scene {}: {w}", r.number)));
                skipped.extend(r.skipped.iter().map(|w| format!("scene {}: {w}", r.number)));
                scenes.push(json!({
                    "number": r.number,
                    "title": r.title,
                    "frames": r.frames,
                    "seconds": r.seconds,
                    "set": r.set,
                    "cuts": r.shots.iter().map(|s| json!({"frame": s.frame, "kind": s.kind, "on": s.on})).collect::<Vec<_>>(),
                    "audio_text_lines": r.lines,
                    "props": r.set_pieces.iter().filter(|p| p.starts_with("prop ")).collect::<Vec<_>>(),
                    "props_not_placed": r.notes.iter().filter(|n| n.starts_with("prop ")).collect::<Vec<_>>(),
                }));
            }
            let v = json!({
                "scenes": scenes,
                "lines_from_audio": from_audio,
                "lines_from_text": from_text,
                "cuts": cuts,
                "warnings": warnings,
                "skipped": skipped,
                "report_md": md,
            });
            write_json(&build_path, &v)?;
            let secs = t.elapsed().as_secs_f64();
            report.seconds.insert("build".into(), secs);
            state.finish(Step::Build, &build_hash, secs, "")?;
            doc = Some(built);
            v
        }
    };
    report.buzz = buzz.clone();
    report.lines_from_audio = build_value["lines_from_audio"].as_u64().unwrap_or(0) as usize;
    report.lines_from_text = build_value["lines_from_text"].as_u64().unwrap_or(0) as usize;
    report.cuts = build_value["cuts"].as_u64().unwrap_or(0) as usize;
    if let Some(w) = build_value["warnings"].as_array() {
        report.warnings.extend(w.iter().filter_map(|x| x.as_str().map(String::from)));
    }
    check(cancel)?;

    // -- 7. render -----------------------------------------------------------------------
    let t = Instant::now();
    let render_hash = hash_parts(&[build_hash.as_bytes(), format!("{} {} {}", req.height, req.mp4, req.stills).as_bytes()]);
    let built_scenes: Vec<(u32, String)> = build_value["scenes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|s| s["frames"].as_u64().unwrap_or(0) > 0)
                .map(|s| (s["number"].as_u64().unwrap_or(0) as u32, s["title"].as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();
    let stems: Vec<String> = built_scenes.iter().map(|(n, title)| scene_file_stem(*n, title)).collect();
    let film = out.join(format!("{series_file}.mp4"));
    let scene_mp4s: Vec<PathBuf> = stems.iter().map(|s| out.join("scenes").join(format!("{s}.mp4"))).collect();
    let listed_stills = |dir: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "png")).collect())
            .unwrap_or_default();
        v.sort();
        v
    };
    let outputs_there = (!req.mp4 || (film.is_file() && scene_mp4s.iter().all(|p| p.is_file())))
        && (req.stills == 0 || listed_stills(&out.join("stills")).len() >= stems.len() * req.stills as usize);
    if !req.mp4 && req.stills == 0 {
        progress(Step::Render, "off (no MP4, no stills asked for)");
        report.seconds.insert("render".into(), 0.0);
    } else if state.done(Step::Render, &render_hash) && outputs_there {
        report.skipped_steps.push("render".into());
        progress(Step::Render, "unchanged; the MP4s and stills kept");
        report.seconds.insert("render".into(), 0.0);
    } else {
        let doc = match doc {
            Some(d) => d,
            None => Document::open(&buzz).map_err(|e| anyhow!("opening {}: {e}", buzz.display()))?,
        };
        let names = doc.scene_names();
        let scenes = doc.film();
        if req.stills > 0 {
            let _ = std::fs::remove_dir_all(out.join("stills"));
        }
        for (index, scene) in scenes.iter().enumerate() {
            check(cancel)?;
            let Some(stem) = stems.get(index) else { break };
            let name = names.get(index).cloned().unwrap_or_else(|| stem.clone());
            if req.stills > 0 {
                progress(Step::Render, &format!("scene {}/{}: {} still(s)", index + 1, scenes.len(), req.stills));
                stills(scene, &out.join("stills"), stem, req.stills, req.height)?;
            }
            if req.mp4 {
                progress(Step::Render, &format!("scene {}/{}: {name} \u{2192} MP4 at {} px\u{2026}", index + 1, scenes.len(), req.height));
                preview(scene.clone(), &name, &scene_mp4s[index], req.height)?;
            }
        }
        if req.mp4 {
            check(cancel)?;
            progress(Step::Render, &format!("joining {} scene(s) into {}", scene_mp4s.len(), film.display()));
            buzz_export::film::concat_segments(&scene_mp4s, &film).with_context(|| format!("joining into {}", film.display()))?;
        }
        let secs = t.elapsed().as_secs_f64();
        report.seconds.insert("render".into(), secs);
        state.finish(Step::Render, &render_hash, secs, "")?;
    }
    if req.mp4 {
        report.mp4 = Some(film.clone());
        report.scene_mp4s = scene_mp4s.clone();
    }
    if req.stills > 0 {
        report.stills = listed_stills(&out.join("stills"));
    }
    check(cancel)?;

    // -- 8. report -----------------------------------------------------------------------
    let t = Instant::now();
    let report_path = out.join("report.md");
    let md = report_md(req, &series, &play, &numbers, &report, &cast_value, &bg_value, plan.as_ref(), &build_value);
    std::fs::write(&report_path, md).with_context(|| format!("writing {}", report_path.display()))?;
    report.report = report_path.clone();
    report.seconds.insert("report".into(), t.elapsed().as_secs_f64());
    progress(Step::Report, &format!("wrote {}", report_path.display()));
    Ok(report)
}

/// One scene's preview MP4, through the headless render (sounds included).
fn preview(scene: buzz_scene::Scene, name: &str, output: &Path, height: u32) -> Result<()> {
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // The render reads a document from disk: each scene is saved alone into a
    // temporary one, so the film it makes is this scene and no other.
    let temp = tempfile::tempdir()?;
    let mut solo = Document::from_scenes(vec![(name.to_string(), scene)]);
    let path = temp.path().join("scene.buzz");
    solo.save_as(&path).map_err(|e| anyhow!("saving a scene for its render: {e}"))?;
    render(&RenderJob {
        document: Some(path),
        brief: None,
        script: None,
        audio: Vec::new(),
        save: None,
        preview: None,
        output: Some(output.to_path_buf()),
        height: Some(height),
        gpu: GpuPreference::Automatic,
    })
    .with_context(|| format!("rendering {}", output.display()))?;
    Ok(())
}

/// `count` evenly spaced stills of a scene.
fn stills(scene: &buzz_scene::Scene, dir: &Path, stem: &str, count: u32, height: u32) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut settings = buzz_export::ExportSettings::for_stage(scene);
    let aspect = settings.width as f64 / settings.height.max(1) as f64;
    settings.height = height + height % 2;
    settings.width = ((height as f64 * aspect).round().max(2.0) as u32 + 1) & !1;
    let mut exporter = buzz_export::Exporter::new(&GpuPreference::Automatic)?;
    let frames = scene.frame_count().max(1);
    for i in 0..count {
        let frame = (((i as f64 + 0.5) / count as f64 * frames as f64) as u32).min(frames - 1);
        let rendered = exporter.render(scene, frame, &settings)?;
        rendered.write_png(&dir.join(format!("{stem}_f{frame:04}.png")))?;
    }
    Ok(())
}

/// The producer's report: what was cast, generated, aligned, skipped, and
/// every warning, then the build's own report.
#[allow(clippy::too_many_arguments)]
fn report_md(
    req: &ProduceRequest,
    series: &str,
    play: &Screenplay,
    numbers: &[u32],
    report: &ProduceReport,
    cast: &Value,
    bgs: &Value,
    plan: Option<&AudioPlan>,
    build: &Value,
) -> String {
    let mut o = String::new();
    o.push_str(&format!("# Produced: {series}\n\n"));
    o.push_str(&format!("* **Script:** {} (scenes {numbers:?})\n", req.script.display()));
    o.push_str(&format!(
        "* **Audio:** {}\n",
        req.audio.as_ref().map(|a| a.display().to_string()).unwrap_or_else(|| "none (timing from the words)".into())
    ));
    o.push_str(&format!("* **Style:** {} \u{2014} {}\n", req.style.name, req.style.suffix));
    o.push_str(&format!("* **Breakdown by:** {}\n", report.breakdown_backend));
    o.push_str(&format!("* **Document:** {}\n", report.buzz.display()));
    if let Some(m) = &report.mp4 {
        o.push_str(&format!("* **Film:** {}\n", m.display()));
    }
    o.push_str("\n## Steps\n\n| Step | Seconds | |\n|---|---|---|\n");
    for step in Step::ALL {
        let s = report.seconds.get(step.key()).copied().unwrap_or(0.0);
        let skipped = report.skipped_steps.iter().any(|k| k == step.key());
        o.push_str(&format!("| {} | {s:.1} | {} |\n", step.key(), if skipped { "unchanged, skipped" } else { "" }));
    }

    o.push_str("\n## Cast\n\n");
    if report.cast.is_empty() {
        o.push_str("Nobody cast.\n");
    }
    for (name, rig) in &report.cast {
        let views: Vec<String> = cast["views"][name].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        let view = match view_rank(rig) {
            0 => "front",
            1 => "3-4 (no front)",
            2 => "back (no front)",
            _ => "side (no front)",
        };
        o.push_str(&format!(
            "* **{name}** \u{2192} `Cast/{rig}` ({view}; {}); views in Cast: {}\n",
            cast["how"][name].as_str().unwrap_or(""),
            views.join(", ")
        ));
    }
    if let Some(g) = cast["generated"].as_array().filter(|a| !a.is_empty()) {
        o.push_str("\n**Characters drawn by ComfyUI:**\n");
        for e in g {
            o.push_str(&format!("* {} \u{2014} {}\n", e[0].as_str().unwrap_or(""), e[1].as_str().unwrap_or("")));
        }
    }
    if let Some(s) = cast["spill"].as_array().filter(|a| !a.is_empty()) {
        o.push_str("\n**Spill (px beyond 1 px / furthest px; gate 150 / 3):**\n");
        for e in s {
            o.push_str(&format!("* {}: {:.0} / {:.2}\n", e[0].as_str().unwrap_or(""), e[1].as_f64().unwrap_or(0.0), e[2].as_f64().unwrap_or(0.0)));
        }
    }
    if let Some(s) = cast["skipped"].as_array().filter(|a| !a.is_empty()) {
        o.push_str("\n**Skipped while casting:**\n");
        for e in s {
            o.push_str(&format!("* {}: {}\n", e[0].as_str().unwrap_or(""), e[1].as_str().unwrap_or("")));
        }
    }

    o.push_str("\n## Backgrounds\n\n");
    match bgs["backgrounds"].as_object().filter(|m| !m.is_empty()) {
        None => o.push_str(if req.backgrounds { "None generated.\n" } else { "Off: sets dressed from your assets (AS3).\n" }),
        Some(m) => {
            for (k, e) in m {
                match (e["asset"].as_str(), e["error"].as_str()) {
                    (Some(a), _) => {
                        o.push_str(&format!(
                            "* **{k}** \u{2192} `{a}` (seed {}, {} s to generate, {} s in all; horizon row {})\n  * prompt: {}\n",
                            e["seed"], e["generate_s"], e["total_s"], e["horizon_row"], e["prompt"].as_str().unwrap_or("")
                        ));
                        let notes: Vec<&str> = e["people_check"].as_array().map(|a| a.iter().filter_map(|n| n.as_str()).collect()).unwrap_or_default();
                        if !notes.is_empty() {
                            o.push_str(&format!("  * people check: {}\n", notes.join(", ")));
                        }
                    }
                    (None, Some(err)) => o.push_str(&format!("* **{k}**: FAILED \u{2014} {err}\n")),
                    _ => {}
                }
            }
            // P8-C1: the props the build stood in front of the pictures.
            let props: Vec<String> = build["scenes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .flat_map(|s| {
                            let n = s["number"].as_u64().unwrap_or(0);
                            s["props"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .filter_map(move |p| p.as_str().map(|p| format!("scene {n}: {p}")))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if props.is_empty() {
                o.push_str("\nProps on a generated background: none placed (no asset index, or nothing in it matched the breakdown's `props_wanted`).\n");
            } else {
                o.push_str("\n**Props from the asset index, on the ground behind the cast (one layout per location):**\n");
                for p in props {
                    o.push_str(&format!("* {p}\n"));
                }
            }
            let missed: Vec<String> = build["scenes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .flat_map(|s| {
                            let n = s["number"].as_u64().unwrap_or(0);
                            s["props_not_placed"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .filter_map(move |p| p.as_str().map(|p| format!("scene {n}: {p}")))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !missed.is_empty() {
                o.push_str("\n**Props not placed** (a handheld or counter thing needs a surface; name it in the casting's `props` with x/y):\n");
                for p in missed {
                    o.push_str(&format!("* {p}\n"));
                }
            }
        }
    }

    o.push_str("\n## Audio alignment\n\n");
    match plan {
        None => o.push_str("No recording: every line is timed from its words.\n"),
        Some(p) => {
            o.push_str(&format!(
                "{} line(s) timed from the recording, {} from their text.\n\n| Scene | Item | Speaker | Line | Start s | End s |\n|---|---|---|---|---|---|\n",
                report.lines_from_audio, report.lines_from_text
            ));
            for l in &p.lines {
                let (speaker, text) = match play.scene(l.scene).and_then(|s| s.items.get(l.item)) {
                    Some(Item::Dialogue(d)) => (d.speaker.clone(), d.text.split_whitespace().take(6).collect::<Vec<_>>().join(" ")),
                    _ => (String::new(), String::new()),
                };
                o.push_str(&format!("| {} | {} | {speaker} | {text} | {:.3} | {:.3} |\n", l.scene, l.item, l.start, l.end));
            }
            let from_text: Vec<String> = build["report_md"]
                .as_str()
                .unwrap_or("")
                .lines()
                .filter_map(|l| l.trim().strip_prefix("* from text: ").map(String::from))
                .collect();
            if !from_text.is_empty() {
                o.push_str("\n**Timed from their text (not found in the recording):**\n");
                for l in from_text {
                    o.push_str(&format!("* {l}\n"));
                }
            }
        }
    }

    o.push_str(&format!("\n## Camera\n\n{} cut(s) (`\"shots\": \"auto\"`).\n", report.cuts));
    if let Some(scenes) = build["scenes"].as_array() {
        for s in scenes {
            let cuts: Vec<String> = s["cuts"]
                .as_array()
                .map(|a| a.iter().map(|c| format!("f{} {} {}", c["frame"], c["kind"].as_str().unwrap_or(""), c["on"].as_str().unwrap_or(""))).collect())
                .unwrap_or_default();
            o.push_str(&format!("* scene {}: {}\n", s["number"], cuts.join("; ")));
        }
    }

    o.push_str("\n## Skipped names\n\n");
    let skipped: Vec<&str> = build["skipped"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    if skipped.is_empty() {
        o.push_str("None.\n");
    }
    for s in skipped {
        o.push_str(&format!("* {s}\n"));
    }

    o.push_str("\n## Warnings\n\n");
    if report.warnings.is_empty() {
        o.push_str("None.\n");
    }
    for w in &report.warnings {
        o.push_str(&format!("* {w}\n"));
    }

    o.push_str("\n---\n\n");
    o.push_str(build["report_md"].as_str().unwrap_or(""));
    o
}
