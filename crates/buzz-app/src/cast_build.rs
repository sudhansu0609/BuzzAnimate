//! **Cast from a folder of character art** (PRODUCER_PLAN § P4).
//!
//! `series_setup`'s one-view pipeline, generalised: every character picture
//! (`<dir>/<Name>/*.png`, `<dir>/<Name>.png`, or `<dir>/<Name> back.png`) becomes
//! one layer rig, arms relaxed down from the T-pose and baked, saved to
//! `Cast/<Name>` (`<Name> 3-4`, `<Name> Side`, `<Name> Back` by the file name's
//! words). The trace is the recommended one (`TraceCharacterOptions::default()`,
//! what the AI Character dialog's Reset gives). Spill is measured as
//! `examples/rig_vs_source.rs` measures it, on the T-pose at the picture's own
//! size; a rig past the gate (> 150 px beyond 1 px, or > 3 px at the furthest)
//! is traced once more with `speckle` doubled, and the report says so. A wanted
//! character with no picture and importance ≤ 2 is drawn by ComfyUI first
//! (`<out>/characters/<Name>.png`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use buzz_ai::breakdown::CharacterBrief;
use buzz_ai::character::{
    assemble_character, complete_parts, part_masks, skeleton, trace_character_whole_with, AssembledCharacter,
    TraceCharacterOptions,
};
use buzz_doc::AssetLibrary;
use buzz_geom::Affine;
use buzz_render::GpuPreference;
use buzz_scene::{Object, ObjectId, ObjectKind, Scene};
use image::{imageops::FilterType, Rgba, RgbaImage};
use serde_json::{json, Value};

use crate::agent::{execute, AgentCall, AgentResult};
use crate::app::App;
use crate::backdrop::Style;
use crate::comfy::ComfyClient;

/// Spill gate: painted pixels more than 1 px outside the source figure.
pub const GATE_BEYOND_1PX: f64 = 150.0;
/// Spill gate: the furthest painted pixel outside the source figure.
pub const GATE_MAX_PX: f64 = 3.0;
/// How far below horizontal the relaxed arms hang (series_setup's `--hang`).
pub const HANG_DEGREES: f64 = 75.0;
/// One view's build may take this long; past it the view is skipped.
const BUILD_DEADLINE: Duration = Duration::from_secs(180);
/// One character sheet may take this long (a cold model load included).
const SHEET_TIMEOUT: Duration = Duration::from_secs(900);

/// What to cast.
#[derive(Debug, Clone)]
pub struct CastRequest {
    /// `<dir>/<Name>/*.png`, `<dir>/<Name>.png`, or files named `<Name> back.png`.
    pub characters_dir: Option<PathBuf>,
    /// From P1: names + descriptions. Empty: every character found in the folder.
    pub wanted: Vec<CharacterBrief>,
    pub style: Style,
    /// Rebuild a rig whose `Cast/<Name>` asset exists.
    pub replace: bool,
    /// To draw characters that have no picture.
    pub comfy: Option<ComfyClient>,
    /// `rigs/`, `previews/`, `characters/` go here.
    pub out: PathBuf,
}

/// What was cast.
#[derive(Debug, Clone, Default)]
pub struct CastReport {
    /// (script name, Cast rig name, preview PNG).
    pub built: Vec<(String, String, PathBuf)>,
    /// (script name, generated character sheet).
    pub generated: Vec<(String, PathBuf)>,
    /// (script name, why).
    pub skipped: Vec<(String, String)>,
    /// (Cast rig name, spill px beyond 1 px, furthest spill px), of the rig kept.
    pub spill: Vec<(String, f64, f64)>,
    /// Retries, gate results, timings: one line each.
    pub notes: Vec<String>,
    /// (Cast rig name, the picture chosen for it, why that one).
    pub views: Vec<(String, PathBuf, String)>,
}

// ------------------------------------------------------------------ the folder

/// Which way a picture faces: from its file name's words, else from its pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum View {
    Front,
    ThreeQuarter,
    Side,
    Back,
}

impl View {
    /// `back` / `side` / `3-4`, `34`, `three`; `None` when the name has none.
    pub fn of_file(stem: &str) -> Option<View> {
        let lower = stem.to_lowercase();
        let w = words(stem);
        if w.iter().any(|x| x == "back") {
            Some(View::Back)
        } else if w.iter().any(|x| x == "side") {
            Some(View::Side)
        } else if lower.contains("3-4") || w.iter().any(|x| x == "34" || x == "three") {
            Some(View::ThreeQuarter)
        } else {
            None
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            View::Front => "front",
            View::ThreeQuarter => "3-4",
            View::Side => "side",
            View::Back => "back",
        }
    }

    /// `Guddu`, `Guddu 3-4`, `Guddu Side`, `Guddu Back`.
    pub fn rig_name(self, name: &str) -> String {
        match self {
            View::Front => name.to_string(),
            View::ThreeQuarter => format!("{name} 3-4"),
            View::Side => format!("{name} Side"),
            View::Back => format!("{name} Back"),
        }
    }
}

/// One character's pictures.
#[derive(Debug, Clone)]
pub struct CharacterArt {
    /// The Cast name: the folder's (or file's) name, first letter capitalised.
    pub name: String,
    /// Sorted by file name.
    pub images: Vec<PathBuf>,
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Lowercase letters and digits only: `SHARMA JI` and `sharmaji` match.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// `CONDUCTOR` → `Conductor`, `SHARMA JI` → `Sharma Ji`.
fn display_name(script_name: &str) -> String {
    script_name
        .split_whitespace()
        .map(|w| capitalise(&w.to_lowercase()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_png(p: &Path) -> bool {
    p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

/// The characters in `dir`: each sub-folder holding PNGs is one, and each PNG
/// directly in `dir` belongs to the name its file name gives once the view
/// words are taken off (`Guddu back.png` → `Guddu`).
pub fn scan(dir: &Path) -> Vec<CharacterArt> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    entries.sort();
    let mut found: Vec<CharacterArt> = Vec::new();
    let add = |found: &mut Vec<CharacterArt>, name: String, path: PathBuf| {
        if name.is_empty() {
            return;
        }
        match found.iter_mut().find(|a| norm(&a.name) == norm(&name)) {
            Some(a) => a.images.push(path),
            None => found.push(CharacterArt { name, images: vec![path] }),
        }
    };
    for path in &entries {
        if path.is_dir() {
            let Some(folder) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
                continue;
            };
            let mut files: Vec<PathBuf> = std::fs::read_dir(path)
                .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| is_png(p)).collect())
                .unwrap_or_default();
            files.sort();
            for f in files {
                add(&mut found, capitalise(&folder), f);
            }
        } else if is_png(path) {
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let view_words = ["back", "side", "3-4", "34", "three", "front", "view"];
            let name: Vec<&str> = stem
                .split(|c: char| c.is_whitespace() || c == '_')
                .filter(|t| !t.is_empty() && !view_words.contains(&t.to_lowercase().as_str()))
                .collect();
            add(&mut found, capitalise(&name.join(" ")), path.clone());
        }
    }
    found
}

// ------------------------------------------------------------------ the cast

/// Build every wanted character's rigs into `Cast/` of `assets`.
pub fn build_cast(assets: &AssetLibrary, req: &CastRequest, progress: &mut dyn FnMut(&str)) -> CastReport {
    let mut report = CastReport::default();
    let Some(root) = assets.root() else {
        report.skipped.push(("*".into(), "the asset library has no folder".into()));
        return report;
    };
    let mut library = AssetLibrary::at(root);
    let art = req.characters_dir.as_deref().map(scan).unwrap_or_default();
    let wanted: Vec<CharacterBrief> = if req.wanted.is_empty() {
        art.iter().map(|a| CharacterBrief { name: a.name.clone(), importance: 1, ..Default::default() }).collect()
    } else {
        req.wanted.clone()
    };
    let scratch = req.out.join("scratch");

    for brief in &wanted {
        if brief.voice_only {
            report.skipped.push((brief.name.clone(), "voice only".into()));
            continue;
        }
        let keys: Vec<String> =
            std::iter::once(&brief.name).chain(brief.aliases.iter()).map(|n| norm(n)).filter(|k| !k.is_empty()).collect();
        let found = art.iter().find(|a| keys.contains(&norm(&a.name)));
        let (cast_name, images, generated) = match found {
            Some(a) => (a.name.clone(), a.images.clone(), false),
            None => {
                if brief.importance > 2 {
                    report.skipped.push((brief.name.clone(), "no picture, and an extra is not drawn".into()));
                    continue;
                }
                let Some(client) = &req.comfy else {
                    report.skipped.push((brief.name.clone(), "no picture, and no ComfyUI to draw one".into()));
                    continue;
                };
                let name = display_name(&brief.name);
                let png = req.out.join("characters").join(format!("{name}.png"));
                progress(&format!("{name}: drawing a character sheet with ComfyUI"));
                let started = Instant::now();
                match generate_sheet(client, &sheet_prompt(brief, &req.style), seed_for(&name), &png) {
                    Ok(p) => {
                        report.notes.push(format!("{name}: character sheet {} in {:.0} s", p.display(), started.elapsed().as_secs_f64()));
                        report.generated.push((brief.name.clone(), p.clone()));
                        (name, vec![p], true)
                    }
                    Err(e) => {
                        report.skipped.push((brief.name.clone(), format!("ComfyUI: {e}")));
                        continue;
                    }
                }
            }
        };

        // --- One picture per view, judged from its pose.
        let mut judged: Vec<Judged> = Vec::new();
        for image in &images {
            progress(&format!("{cast_name}: looking at {}", image.display()));
            match judge(image) {
                Ok(mut j) => {
                    if generated {
                        j.view = View::Front;
                        j.why = format!("generated front sheet; {}", j.why);
                    }
                    match &j.skip {
                        Some(why) => report.skipped.push((brief.name.clone(), format!("{}: {why}", image.display()))),
                        None => judged.push(j),
                    }
                }
                Err(e) => report.skipped.push((brief.name.clone(), format!("{}: {e}", image.display()))),
            }
        }
        let mut chosen: Vec<(View, Judged)> = Vec::new();
        for view in [View::Front, View::ThreeQuarter, View::Side, View::Back] {
            let mut of_view: Vec<Judged> = judged.iter().filter(|j| j.view == view).cloned().collect();
            if of_view.is_empty() {
                continue;
            }
            let total = of_view.len();
            let tposes = of_view.iter().filter(|j| j.tpose).count();
            if tposes == 0 && total > 1 {
                for j in &of_view {
                    report.skipped.push((
                        brief.name.clone(),
                        format!("{}: {} view, not a T-pose ({})", j.image.display(), view.label(), j.why),
                    ));
                }
                continue;
            }
            // T-poses first, then the largest figure.
            of_view.sort_by(|a, b| b.tpose.cmp(&a.tpose).then(b.height_px.total_cmp(&a.height_px)));
            let mut best = of_view.remove(0);
            for j in &of_view {
                let why = if j.tpose {
                    format!("another {} picture has a larger figure", view.label())
                } else {
                    "not a T-pose".to_string()
                };
                report.skipped.push((brief.name.clone(), format!("{}: {why} ({})", j.image.display(), j.why)));
            }
            best.why = format!(
                "{}; {}; largest of {tposes} T-pose(s) among {total} {} picture(s)",
                best.why,
                if best.tpose { "T-pose" } else { "not a T-pose (arms down), the only picture of this view" },
                view.label()
            );
            if !best.tpose {
                report
                    .notes
                    .push(format!("{}: not a T-pose (arms down), built as the only {} picture", view.rig_name(&cast_name), view.label()));
            }
            chosen.push((view, best));
        }

        // No front but a 3-4: the producer casts the 3-4, and the report says so.
        if !chosen.iter().any(|(v, _)| *v == View::Front) {
            if let Some((_, pick)) = chosen.iter_mut().find(|(v, _)| *v == View::ThreeQuarter) {
                pick.why = format!("{}; NO FRONT picture for {cast_name}: cast this 3-4 in its place", pick.why);
            }
        }

        for (view, pick) in chosen {
            let rig_name = view.rig_name(&cast_name);
            let image = pick.image.clone();
            report.views.push((rig_name.clone(), image.clone(), pick.why.clone()));
            let exists = library.assets().iter().any(|a| a.folder == "Cast" && a.name == rig_name);
            if exists && !req.replace {
                report.skipped.push((brief.name.clone(), format!("kept: Cast/{rig_name} exists (replace to rebuild)")));
                continue;
            }
            progress(&format!("{rig_name}: building from {}", image.display()));
            // On a worker with a deadline: a picture the character pipeline
            // hangs on is skipped, not the whole cast.
            let (tx, rx) = std::sync::mpsc::channel();
            {
                let (image, person, rig, out, scratch) =
                    (image.clone(), pick.person, rig_name.clone(), req.out.clone(), scratch.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(build_view(&image, person, &rig, &out, &scratch, generated, &mut |_| {}));
                });
            }
            let result = match rx.recv_timeout(BUILD_DEADLINE) {
                Ok(r) => r,
                Err(_) => Err(format!("timed out after {} s ({})", BUILD_DEADLINE.as_secs(), image.display())),
            };
            match result {
                Ok(v) => {
                    match library.save(&rig_name, "Cast", &v.scene) {
                        Ok(_) => {
                            report.built.push((brief.name.clone(), rig_name.clone(), v.preview));
                            report.spill.push((rig_name.clone(), v.beyond_1px, v.max_px));
                        }
                        Err(e) => report.skipped.push((brief.name.clone(), format!("{rig_name}: Cast save: {e}"))),
                    }
                    report.notes.extend(v.notes);
                    report.notes.push(format!("{rig_name}: built in {:.1} s", v.seconds));
                }
                Err(e) => report.skipped.push((brief.name.clone(), format!("{rig_name}: {e}"))),
            }
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    report
}

/// The ComfyUI prompt for a character with no picture.
pub fn sheet_prompt(brief: &CharacterBrief, style: &Style) -> String {
    let who = if brief.description.trim().is_empty() { display_name(&brief.name) } else { brief.description.trim().trim_end_matches('.').to_string() };
    // A character suffix, never the background one (P4 review): scenery words
    // there put foliage and a ground shadow round the figure.
    let colour = if style.name == "painterly" { "soft painted shading" } else { "flat cel colours" };
    format!(
        "{who}, full body character design sheet, standing T-pose with both arms straight out horizontally, \
         front view facing the camera, feet slightly apart, 2D animation character in a clean vector cartoon style, \
         professional illustration, bold clean black ink outlines, {colour}, plain pure white background, \
         no ground shadow, no scenery, no foliage, no props on the ground, no text,          the whole figure fully inside the picture with clear margin on every side, hard clean edges,          no glow, no outer light, no halo"
    )
}

/// A seed that stays the same for a name, so a rerun draws the same character.
fn seed_for(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h % 1_000_000_000_000
}

/// A portrait character sheet through the Z-Image workflow `backdrop` uses:
/// 1024×1024 base, so a T-pose fits (the hq workflow's 1.5× pass gives 1536²).
/// Cached: an existing file is reused.
pub fn generate_sheet(client: &ComfyClient, prompt: &str, seed: u64, out_png: &Path) -> Result<PathBuf, String> {
    if out_png.is_file() {
        return Ok(out_png.to_path_buf());
    }
    let path = crate::backdrop::workflow_path()
        .ok_or("no Z-Image workflow found (COMFY_WORKFLOWS or <estate>/BuzzEdit/workflows)")?;
    let size = (1024, 1024);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut workflow: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    crate::backdrop::bind_workflow(&mut workflow, prompt, seed, size)?;
    if let Some(inputs) = workflow.get_mut("9").and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        inputs.insert("filename_prefix".into(), Value::from("buzzanimate-char"));
    }
    let id = client.queue(workflow)?;
    let outputs = client.wait(&id, SHEET_TIMEOUT)?;
    let first = outputs.first().ok_or("ComfyUI returned no picture")?;
    let bytes = client.view(first)?;
    let picture = image::load_from_memory(&bytes).map_err(|e| format!("{}: {e}", first.filename))?;
    if let Some(dir) = out_png.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    picture.to_rgba8().save(out_png).map_err(|e| format!("{}: {e}", out_png.display()))?;
    Ok(out_png.to_path_buf())
}

// ------------------------------------------------------------------ judging a picture

/// What DWPose says about one picture.
#[derive(Debug, Clone)]
struct Judged {
    image: PathBuf,
    person: buzz_ai::pose::Person,
    view: View,
    tpose: bool,
    /// The figure's height in pixels.
    height_px: f32,
    /// Why this view (and, once chosen, why this picture).
    why: String,
    /// A sheet or chart, not one character: why.
    skip: Option<String>,
}

/// The matte's silhouette: (largest 4-connected component's share of the
/// matte pixels, its box w/h, its box height as a share of the picture's).
fn silhouette(matte: &image::GrayImage) -> (f32, f32, f32) {
    let (w, h) = (matte.width() as usize, matte.height() as usize);
    let on: Vec<bool> = matte.pixels().map(|p| p[0] > 128).collect();
    let total = on.iter().filter(|v| **v).count();
    if total == 0 {
        return (0.0, 0.0, 0.0);
    }
    let mut seen = vec![false; w * h];
    let (mut best, mut best_box) = (0usize, (0usize, 0usize, 0usize, 0usize));
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !on[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut n, mut x0, mut y0, mut x1, mut y1) = (0usize, usize::MAX, usize::MAX, 0usize, 0usize);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            n += 1;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            let mut push = |j: usize| {
                if on[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if n > best {
            best = n;
            best_box = (x0, y0, x1, y1);
        }
    }
    let (bw, bh) = ((best_box.2 - best_box.0 + 1) as f32, (best_box.3 - best_box.1 + 1) as f32);
    (best as f32 / total as f32, bw / bh, bh / h.max(1) as f32)
}

/// The picture's view and whether it is one figure in a T-pose, from the
/// silhouette: a full body is the matte's largest component holding ≥ 90 % of
/// it with a box ≥ 60 % of the picture's height (charts and sheets fail); a
/// T-pose is a full body with box w/h ≥ 0.65. More than one person detected is a
/// sheet. File-name words win for the view; else nose confidence < 0.3 or mean
/// face-landmark confidence < 0.35 is back, and the nose's offset from the
/// shoulders' middle (share of their width) gives front (< 15 %), 3-4 (15–40 %)
/// or side (> 40 %, skipped). Arm angles are logged only.
fn judge(image: &Path) -> Result<Judged, String> {
    let rgba = image::open(image).map_err(|e| format!("decode: {e}"))?.to_rgba8();
    let persons = buzz_ai::pose::detect(&rgba).map_err(|e| format!("pose: {e}"))?;
    let person = persons.first().cloned().ok_or("no person detected")?;
    let matte = buzz_ai::segment::matte(&rgba, None).map_err(|e| format!("matte: {e}"))?;
    let (largest, aspect, box_share) = silhouette(&matte);
    let k = &person.keypoints;
    let height_px = person.character_height();
    let stem = image.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let (sl, sr, nose) = (k[5], k[6], k[0]);
    let shoulder_w = (sl.0 - sr.0).abs().max(1.0);
    let offset = (nose.0 - (sl.0 + sr.0) / 2.0).abs() / shoulder_w;
    // Mean confidence of the 68 face landmarks: low on a back view even when
    // DWPose puts a nose there.
    let face = k[23..=90].iter().map(|p| p.2).sum::<f32>() / 68.0;
    let (view, why) = match View::of_file(&stem) {
        Some(v) => (v, format!("file name says {}", v.label())),
        None if nose.2 < 0.3 || face < 0.35 => (
            View::Back,
            format!("nose or face not seen (nose {:.2}, face landmarks {face:.2}): back", nose.2),
        ),
        None if offset > 0.40 => (View::Side, format!("nose {:.0} % of the shoulder width off centre: side", offset * 100.0)),
        None if offset >= 0.15 => {
            (View::ThreeQuarter, format!("nose {:.0} % of the shoulder width off centre: 3-4", offset * 100.0))
        }
        None => (View::Front, format!("nose {:.0} % of the shoulder width off centre: front", offset * 100.0)),
    };
    let arm = |sh: (f32, f32, f32), el: (f32, f32, f32)| {
        let (dx, dy) = ((el.0 - sh.0).abs(), (el.1 - sh.1).abs());
        dy.atan2(dx).to_degrees()
    };
    let (a1, a2) = (arm(sl, k[7]), arm(sr, k[8]));
    let why = format!(
        "{why}; silhouette: largest part {:.0} % of the matte, box w/h {aspect:.2}, {:.0} % of the height; arms {a1:.0}/{a2:.0} deg (logged only); figure {height_px:.0} px",
        largest * 100.0,
        box_share * 100.0
    );
    // One full-body figure; a T-pose when it is also wide. A full body that is
    // not a T-pose is kept for the "only picture of its view" rule.
    let full_body = largest >= 0.90 && box_share >= 0.60;
    let tpose = aspect >= 0.65;
    let mut skip = None;
    if persons.len() > 1 {
        skip = Some(format!("sheet/chart/partial: {} figures ({why})", persons.len()));
    } else if !full_body {
        skip = Some(format!("sheet/chart/partial: silhouette is not one full-body figure ({why})"));
    } else if view == View::Side {
        skip = Some(format!("side view: not supported by the rig pipeline ({why})"));
    }
    Ok(Judged { image: image.to_path_buf(), person, view, tpose, height_px, why, skip })
}

// ------------------------------------------------------------------ one view

struct ViewBuilt {
    scene: Scene,
    preview: PathBuf,
    beyond_1px: f64,
    max_px: f64,
    notes: Vec<String>,
    seconds: f64,
}

fn passes(beyond: usize, max: f64) -> bool {
    beyond as f64 <= GATE_BEYOND_1PX && max <= GATE_MAX_PX
}

/// The darkest a rim pixel may be (its smallest channel); ink is darker.
const RIM_MIN_CHANNEL: u8 = 140;
/// How far (px) a rim pixel may lie inside the matte: strips up to ~3 px wide.
const RIM_MAX_DEPTH: f32 = 2.0;
/// How far (px) to look for non-matte on both sides of a rim pixel.
const RIM_SIDE_REACH: i64 = 3;

/// How many px of light (not ink) pixels [`peel_sticker_rim`] goes through
/// before it accepts near-white only.
const PEEL_LIGHT_PX: u32 = 3;
/// The smallest channel of a near-white (paper) pixel for [`peel_sticker_rim`].
const PEEL_WHITE_MIN: u8 = 235;
/// How deep (px) [`peel_sticker_rim`] goes at most.
const PEEL_MAX_DEPTH: u32 = 10;

/// **The generated sheet's sticker rim, peeled** (P8-A A2; only for pictures the
/// cast build drew itself with ComfyUI). Those sheets have a faint grey line a
/// few px outside the ink with near-white paper between, and the segmenter's
/// matte follows the faint line (`trim_paper_halo`'s flood stops at it), so the
/// band plus line (4–8 px) is too thick for [`drop_light_rim`]. From the
/// exterior inward (4-connected): through up to [`PEEL_LIGHT_PX`] px of light
/// pixels (smallest channel ≥ [`RIM_MIN_CHANNEL`]), then near-white only
/// (≥ [`PEEL_WHITE_MIN`]), at most [`PEEL_MAX_DEPTH`] px deep. A peeled pixel
/// is removed only when the peel reached ink there: it lies within
/// [`PEEL_MAX_DEPTH`] steps (through peeled pixels) of a peeled pixel touching
/// ink (smallest channel < [`RIM_MIN_CHANNEL`]). An open light edge with no ink
/// line (a white shirt against the paper) is left as it was.
/// Returns the cleaned matte and how many pixels went.
fn peel_sticker_rim(rgba: &RgbaImage, matte: image::GrayImage) -> (image::GrayImage, usize) {
    let (w, h) = (matte.width() as usize, matte.height() as usize);
    if w < 8 || h < 8 || rgba.width() as usize != w || rgba.height() as usize != h {
        return (matte, 0);
    }
    let inside: Vec<bool> = matte.pixels().map(|p| p[0] > 128).collect();
    let min_channel: Vec<u8> = rgba.pixels().map(|p| p[0].min(p[1]).min(p[2])).collect();
    let neighbours = |i: usize| {
        let (x, y) = (i % w, i / w);
        [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then(|| i + w),
        ]
        .into_iter()
        .flatten()
    };
    // The exterior: non-matte pixels 4-connected to the picture's border.
    let mut exterior = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h)
        .filter(|&i| {
            let (x, y) = (i % w, i / w);
            (x == 0 || y == 0 || x + 1 == w || y + 1 == h) && !inside[i]
        })
        .collect();
    for &i in &stack {
        exterior[i] = true;
    }
    while let Some(i) = stack.pop() {
        for j in neighbours(i) {
            if !inside[j] && !exterior[j] {
                exterior[j] = true;
                stack.push(j);
            }
        }
    }
    // The peel, breadth first from the exterior; `depth` 0 = not peeled.
    let allowed = |i: usize, d: u32| min_channel[i] >= RIM_MIN_CHANNEL && (d <= PEEL_LIGHT_PX || min_channel[i] >= PEEL_WHITE_MIN);
    let mut depth = vec![0u32; w * h];
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for i in 0..w * h {
        if inside[i] && neighbours(i).any(|j| exterior[j]) && allowed(i, 1) {
            depth[i] = 1;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let d = depth[i];
        if d >= PEEL_MAX_DEPTH {
            continue;
        }
        for j in neighbours(i) {
            if inside[j] && depth[j] == 0 && allowed(j, d + 1) {
                depth[j] = d + 1;
                queue.push_back(j);
            }
        }
    }
    // Kept only where the peel reached ink: flood back from the peeled pixels
    // touching ink, through peeled pixels, at most PEEL_MAX_DEPTH steps.
    let mut reach = vec![u32::MAX; w * h];
    for i in 0..w * h {
        if depth[i] > 0 && neighbours(i).any(|j| inside[j] && depth[j] == 0 && min_channel[j] < RIM_MIN_CHANNEL) {
            reach[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let r = reach[i];
        if r >= PEEL_MAX_DEPTH {
            continue;
        }
        for j in neighbours(i) {
            if depth[j] > 0 && reach[j] == u32::MAX {
                reach[j] = r + 1;
                queue.push_back(j);
            }
        }
    }
    let mut out = matte;
    let mut removed = 0usize;
    for i in 0..w * h {
        if reach[i] != u32::MAX {
            out.put_pixel((i % w) as u32, (i / w) as u32, image::Luma([0]));
            removed += 1;
        }
    }
    (out, removed)
}

/// **The sticker rim off the matte** (P8-A A2). A generated character sheet
/// (and some drawings) has a faint grey line a few px outside the figure's ink,
/// with paper between: `segment::trim_paper_halo` takes the paper out, but the
/// line itself stays in the matte as thin strips and traces as a pale ghost
/// edge. Removed: matte pixels the matte's opening by 2 px drops (thin
/// structures), no deeper than [`RIM_MAX_DEPTH`] inside the matte, not ink-dark
/// (smallest channel ≥ [`RIM_MIN_CHANNEL`]), with non-matte within
/// [`RIM_SIDE_REACH`] px on two opposite sides (outside, and the paper gap on
/// the inward side), in pieces that touch the picture's exterior. Returns the
/// cleaned matte and how many pixels went.
fn drop_light_rim(rgba: &RgbaImage, matte: image::GrayImage) -> (image::GrayImage, usize) {
    let (w, h) = (matte.width() as usize, matte.height() as usize);
    if w < 8 || h < 8 || rgba.width() as usize != w || rgba.height() as usize != h {
        return (matte, 0);
    }
    let inside: Vec<bool> = matte.pixels().map(|p| p[0] > 128).collect();
    let mask: Vec<u8> = inside.iter().map(|&m| if m { 255 } else { 0 }).collect();
    let opened = buzz_ai::segment::morph_open(&mask, w as u32, h as u32, 2);
    let depth = buzz_ai::segment::distance_transform(&matte);
    // The exterior: non-matte pixels 4-connected to the picture's border.
    let mut exterior = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        if (x == 0 || y == 0 || x + 1 == w || y + 1 == h) && !inside[i] {
            exterior[i] = true;
            stack.push(i);
        }
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if !inside[j] && !exterior[j] {
                exterior[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    let outside_at = |x: i64, y: i64| x < 0 || y < 0 || x >= w as i64 || y >= h as i64 || !inside[y as usize * w + x as usize];
    let open_both_ways = |x: i64, y: i64| {
        [(1i64, 0i64), (0, 1), (1, 1), (1, -1)].iter().any(|&(dx, dy)| {
            let side = |s: i64| (1..=RIM_SIDE_REACH).any(|k| outside_at(x + s * k * dx, y + s * k * dy));
            side(1) && side(-1)
        })
    };
    let rim: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let p = rgba.get_pixel(x as u32, y as u32);
            let light = p[0].min(p[1]).min(p[2]) >= RIM_MIN_CHANNEL;
            let hit = inside[i] && opened[i] <= 128 && depth[i] <= RIM_MAX_DEPTH && light && open_both_ways(x, y);
            if hit { 255 } else { 0 }
        })
        .collect();
    let touches_exterior = |i: usize| {
        let (x, y) = (i % w, i / w);
        (x > 0 && exterior[i - 1]) || (x + 1 < w && exterior[i + 1]) || (y > 0 && exterior[i - w]) || (y + 1 < h && exterior[i + w])
    };
    let mut out = matte;
    let mut removed = 0usize;
    for piece in buzz_ai::segment::connected_components(&rim, w as u32, h as u32) {
        if !piece.iter().any(|&i| touches_exterior(i)) {
            continue;
        }
        for i in piece {
            out.put_pixel((i % w) as u32, (i / w) as u32, image::Luma([0]));
            removed += 1;
        }
    }
    (out, removed)
}

/// One picture → one baked, slimmed layer rig, saved to `<out>/rigs/<rig>.buzz`,
/// with `<out>/previews/<rig>.png` (rest) and `<rig>_poses.png` (T-pose as
/// drawn | rest, arms down | walk). `generated`: the picture is a sheet the
/// cast build drew itself (ComfyUI), whose sticker rim is peeled
/// ([`peel_sticker_rim`]).
fn build_view(
    image: &Path,
    person: buzz_ai::pose::Person,
    rig: &str,
    out: &Path,
    scratch: &Path,
    generated: bool,
    progress: &mut dyn FnMut(&str),
) -> Result<ViewBuilt, String> {
    let started = Instant::now();
    let rgba = image::open(image).map_err(|e| format!("decode: {e}"))?.to_rgba8();
    let figure = Figure::of(&rgba);
    let mut exporter = buzz_export::Exporter::new(&GpuPreference::default()).map_err(|e| format!("{e}"))?;
    let matte = buzz_ai::segment::matte(&rgba, None).map_err(|e| format!("matte: {e}"))?;
    let (matte, rim_px) = drop_light_rim(&rgba, matte);
    let (matte, peel_px) = if generated { peel_sticker_rim(&rgba, matte) } else { (matte, 0) };
    if std::env::var_os("BUZZ_RIM_LOG").is_some() {
        eprintln!("{rig}: sticker rim: {rim_px} matte px removed, {peel_px} peeled (generated sheet: {generated})");
    }
    let limits = crate::rig_limits::load_or_default();
    let slug = rig.replace(' ', "_");
    let scratch_assets = scratch.join(&slug);
    std::fs::create_dir_all(&scratch_assets).map_err(|e| e.to_string())?;
    let mut notes = Vec::new();

    let mut swap = false;
    loop {
        // --- The wizard's stages, with the recommended trace, gated on spill.
        let mut person = person.clone();
        if swap && !swap_crossed_wrists(&mut person) {
            return Err("the wrists could not be put back on their own arms".into());
        }
        let skel = skeleton(&person);
        let parts = part_masks(&rgba, &matte, &skel).map_err(|e| format!("parts: {e}"))?;
        let completed = complete_parts(&rgba, &matte, &skel, &parts).map_err(|e| format!("completion: {e}"))?;
        let mut options = TraceCharacterOptions::default();
        let assemble = |options: &TraceCharacterOptions| -> Result<AssembledCharacter, String> {
            let traced = trace_character_whole_with(&rgba, &completed, &skel, &matte, options)
                .map_err(|e| format!("trace: {e}"))?;
            assemble_character(&skel, &traced, rig, Some((rgba.width() as f64, rgba.height() as f64)), Some(&limits))
                .map_err(|e| format!("assembly: {e}"))
        };
        let mut assembled = assemble(&options)?;
        let (mut beyond, mut max) = spill_of(&mut exporter, &assembled.scene, &figure)?;
        if !passes(beyond, max) {
            options.trace.speckle *= 2;
            progress(&format!("{rig}: spill {beyond} px beyond 1 px, max {max:.1} px; retrying with speckle {}", options.trace.speckle));
            assembled = assemble(&options)?;
            let (b2, m2) = spill_of(&mut exporter, &assembled.scene, &figure)?;
            notes.push(format!(
                "{rig}: spill gate failed ({beyond} px beyond 1 px, max {max:.2} px); retried with speckle {}: {b2} px, max {m2:.2} px, {}",
                options.trace.speckle,
                if passes(b2, m2) { "passes" } else { "still fails" }
            ));
            beyond = b2;
            max = m2;
        }

        // --- Open it, check the arms are not crossed, relax them.
        let (mut app, element) = open_in_app(&assembled.scene, &slug, &scratch_assets)?;
        let bones = read_bones(&mut app, element)?;
        if let Some(bend) = crossed_arm(&bones) {
            if !swap {
                notes.push(format!("{rig}: an elbow bent {bend:.0} degrees off its upper arm; rebuilt with the wrists swapped back"));
                swap = true;
                continue;
            }
            return Err(format!("the corrected skeleton still has an elbow bent {bend:.0} degrees"));
        }
        let moves = relax_arms(&mut app, element, HANG_DEGREES)?;
        let lifted = moves.iter().filter(|m| m["note"] != json!("")).count();
        if lifted > 0 {
            notes.push(format!("{rig}: joint limits lifted on {lifted} arm(s) to hang them"));
        }

        // --- Baked, taken apart into layers, slimmed.
        let posed_path = scratch_assets.join(format!("{slug}.posed.buzz"));
        must(call(&mut app, "command", json!({"name": "save_as", "path": posed_path.to_string_lossy()})), "save_as")?;
        drop(app);
        let mut scene = buzz_doc::format::load(&posed_path).map_err(|e| format!("reload: {e}"))?;
        if bake_pose(&mut scene) == 0 {
            return Err("the saved document holds no armature".into());
        }
        let (converted, refused) = crate::rigging::convert_all_to_layers(&mut scene, 0);
        if refused > 0 || converted == 0 {
            return Err(format!("{refused} rig(s) have skinned artwork and cannot be one layer per bone"));
        }
        let view = layer_rig(&scene).ok_or("no layer rig after the conversion")?;
        slim_layer_rig(&mut scene, &view, rig);

        let rigs = out.join("rigs");
        let previews = out.join("previews");
        std::fs::create_dir_all(&rigs).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&previews).map_err(|e| e.to_string())?;
        buzz_doc::format::save(&scene, rigs.join(format!("{rig}.buzz"))).map_err(|e| format!("save rig: {e}"))?;

        // --- Previews.
        let rest = render_figure(&mut exporter, &scene, 900)?;
        let preview = previews.join(format!("{rig}.png"));
        rest.save(&preview).map_err(|e| format!("{}: {e}", preview.display()))?;
        let mut walk = scene.clone();
        let walk_cell = if pose_layer_rig(&mut walk, &view, "walk") {
            Some(render_figure(&mut exporter, &walk, 900)?)
        } else {
            notes.push(format!("{rig}: the walk pose did not apply"));
            None
        };
        let tpose = render_figure(&mut exporter, &assembled.scene, 900)?;
        let mut cells = vec![tpose, rest];
        cells.extend(walk_cell);
        let sheet = side_by_side(&cells, 900);
        let sheet_path = previews.join(format!("{rig}_poses.png"));
        sheet.save(&sheet_path).map_err(|e| format!("{}: {e}", sheet_path.display()))?;

        return Ok(ViewBuilt {
            scene,
            preview,
            beyond_1px: beyond as f64,
            max_px: max,
            notes,
            seconds: started.elapsed().as_secs_f64(),
        });
    }
}

/// The assembled rig saved to the scratch folder and opened in a headless `App`
/// (as `series_setup::build_corrected` does): the app and the armature's element.
fn open_in_app(scene: &Scene, slug: &str, scratch_assets: &Path) -> Result<(App, u64), String> {
    let file = scratch_assets.join(format!("{slug}.corrected.buzz"));
    buzz_doc::format::save(scene, &file).map_err(|e| format!("save corrected rig: {e}"))?;
    let mut app = App::new(GpuPreference::default());
    app.live_use_assets_at(scratch_assets);
    must(call(&mut app, "command", json!({"name": "open", "path": file.to_string_lossy()})), "open")?;
    let started = Instant::now();
    loop {
        let _ = call(&mut app, "wait_for_jobs", json!({"timeout_s": 1}));
        let objects = must(call(&mut app, "inspect", json!({"what": "objects"})), "inspect objects")?;
        if let Some(element) = objects["objects"]
            .as_array()
            .and_then(|o| o.iter().find(|o| o["kind"] == json!("armature")))
            .and_then(|o| o["id"].as_u64())
        {
            return Ok((app, element));
        }
        if started.elapsed() > Duration::from_secs(120) {
            return Err("the rig did not open with an armature on stage".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn layer_rig(scene: &Scene) -> Option<buzz_act::layer_rig::LayerRigView> {
    for layer in scene.layers().iter() {
        for o in layer.objects_at(0).iter() {
            if let Some(buzz_act::layer_rig::CharacterRig::Layers(view)) = buzz_act::layer_rig::rig_of(scene, o.id) {
                return Some(view);
            }
        }
    }
    None
}

// ------------------------------------------------------------------ agent tools (from series_setup)

fn call(app: &mut App, tool: &str, args: Value) -> AgentResult {
    execute(app, &AgentCall { tool: tool.to_string(), args })
}

fn must(r: AgentResult, what: &str) -> Result<Value, String> {
    if r.ok { Ok(r.value) } else { Err(format!("{what}: {}", r.error.unwrap_or_default())) }
}

fn wrap_deg(mut a: f64) -> f64 {
    while a > 180.0 {
        a -= 360.0;
    }
    while a <= -180.0 {
        a += 360.0;
    }
    a
}

#[derive(Debug, Clone)]
struct BoneInfo {
    name: String,
    parent: Option<usize>,
    angle: f64,
}

fn read_bones(app: &mut App, element: u64) -> Result<Vec<BoneInfo>, String> {
    let target = format!("element:{element}");
    let list = must(call(app, "edit_list", json!({"target": target, "list": "bones", "op": "get"})), "bones get")?;
    let props = must(call(app, "inspect", json!({"what": "properties", "target": target})), "inspect properties")?;
    let angle_of = |i: usize| {
        props["fields"]
            .as_array()
            .and_then(|f| f.iter().find(|f| f["field"] == json!(format!("bone.{i}.angle"))))
            .and_then(|f| f["value"].as_f64())
    };
    let items = list["items"].as_array().ok_or("bones get returned no items")?;
    items
        .iter()
        .enumerate()
        .map(|(i, b)| {
            Ok(BoneInfo {
                name: b["name"].as_str().unwrap_or("").to_string(),
                parent: b["parent"].as_u64().map(|p| p as usize),
                angle: angle_of(i).ok_or_else(|| format!("bone.{i}.angle is not listed"))?,
            })
        })
        .collect()
}

fn world_angle(bones: &[BoneInfo], index: usize) -> f64 {
    let mut total = 0.0;
    let mut current = Some(index);
    let mut guard = 0;
    while let Some(i) = current {
        total += bones[i].angle;
        current = bones[i].parent;
        guard += 1;
        if guard > bones.len() {
            break;
        }
    }
    wrap_deg(total)
}

fn is_upper_arm(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("shoulder") || n.contains("upper arm") || n.contains("upperarm") || n.contains("bicep")
}

/// Swing each upper arm to `hang` degrees below horizontal on its own side.
fn relax_arms(app: &mut App, element: u64, hang: f64) -> Result<Vec<Value>, String> {
    let target = format!("element:{element}");
    let bones = read_bones(app, element)?;
    let mut done = Vec::new();
    for (i, bone) in bones.iter().enumerate() {
        if !is_upper_arm(&bone.name) {
            continue;
        }
        let world = world_angle(&bones, i);
        let goes_right = world.to_radians().cos() >= 0.0;
        let wanted_world = if goes_right { hang } else { 180.0 - hang };
        let local = wrap_deg(bone.angle + wrap_deg(wanted_world - world));
        let mut r = call(app, "set_property", json!({"target": target, "field": format!("bone.{i}.angle"), "value": local}));
        let mut note = String::new();
        let mut now = read_bones(app, element)?;
        if (wrap_deg(world_angle(&now, i) - wanted_world)).abs() > 0.5 {
            let lifted = call(app, "set_property", json!({"target": target, "field": format!("bone.{i}.limited"), "value": false}));
            if lifted.ok {
                r = call(app, "set_property", json!({"target": target, "field": format!("bone.{i}.angle"), "value": local}));
                note = "joint limits lifted".into();
                now = read_bones(app, element)?;
            }
        }
        if !r.ok {
            return Err(format!("bone.{i}.angle: {}", r.error.unwrap_or_default()));
        }
        done.push(json!({"bone": i, "name": bone.name, "world_after": world_angle(&now, i), "note": note}));
    }
    if done.is_empty() {
        return Err(format!("no upper-arm bone found among {:?}", bones.iter().map(|b| b.name.clone()).collect::<Vec<_>>()));
    }
    Ok(done)
}

/// Adopt the pose the bones are in as the rig's drawn pose, in the artwork too;
/// a multi-frame face part holds its first frame.
fn bake_pose(scene: &mut Scene) -> usize {
    let lengths: std::collections::HashMap<buzz_scene::SymbolId, u32> =
        scene.library().iter().map(|s| (s.id, s.length())).collect();
    let ids: Vec<ObjectId> = scene
        .layers()
        .iter()
        .flat_map(|l| l.all_objects().filter(|o| matches!(o.kind, ObjectKind::Armature(_))).map(|o| o.id).collect::<Vec<_>>())
        .collect();
    for id in &ids {
        scene.update_object(*id, |o| {
            if let ObjectKind::Armature(rig) = &mut o.kind {
                let posed = rig.posed();
                for (part, p) in rig.parts.iter_mut().zip(posed) {
                    part.artwork = p;
                    let long = matches!(&part.artwork.kind, ObjectKind::Instance(i) if lengths.get(&i.symbol).is_some_and(|n| *n > 1));
                    if long && let ObjectKind::Instance(inst) = &mut Arc::make_mut(&mut part.artwork).kind {
                        inst.loop_mode = buzz_scene::LoopMode::SingleFrame;
                        inst.first_frame = 0;
                    }
                }
                rig.armature.set_rest_here();
                rig.rebind();
            }
        });
    }
    ids.len()
}

/// How far the worst elbow bends off its upper arm, if past 60 degrees.
fn crossed_arm(bones: &[BoneInfo]) -> Option<f64> {
    bones
        .iter()
        .filter(|b| {
            let n = b.name.to_lowercase();
            (n.contains("elbow") || n.contains("forearm")) && b.parent.is_some_and(|p| is_upper_arm(&bones[p].name))
        })
        .map(|b| wrap_deg(b.angle).abs())
        .filter(|a| *a > 60.0)
        .reduce(f64::max)
}

/// DWPose's wrists put back on the arm whose elbow they belong to.
fn swap_crossed_wrists(person: &mut buzz_ai::pose::Person) -> bool {
    let k = &mut person.keypoints;
    let along = |sh: (f32, f32, f32), el: (f32, f32, f32), wr: (f32, f32, f32)| {
        let (ax, ay) = (el.0 - sh.0, el.1 - sh.1);
        let (bx, by) = (wr.0 - el.0, wr.1 - el.1);
        ax * bx + ay * by
    };
    let straight =
        |k: &[(f32, f32, f32); 133], w9: usize, w10: usize| along(k[5], k[7], k[w9]) > 0.0 && along(k[6], k[8], k[w10]) > 0.0;
    if straight(k, 9, 10) {
        return false;
    }
    if !straight(k, 10, 9) {
        return false;
    }
    k.swap(9, 10);
    for i in 0..21 {
        k.swap(91 + i, 112 + i);
    }
    true
}

/// The episode's `slim_layer_rig`: each part drawn as a group or a shape becomes
/// an instance of a symbol made for it, so a key per part costs a few bytes.
fn slim_layer_rig(scene: &mut Scene, rig: &buzz_act::layer_rig::LayerRigView, label: &str) -> usize {
    let mut moved = 0usize;
    for (index, &(_, _, id)) in rig.parts.iter().enumerate() {
        let Some(art) = scene.find_object(id).map(|(_, o)| o.clone()) else {
            continue;
        };
        if !matches!(art.kind, ObjectKind::Group(_) | ObjectKind::Shape(_)) {
            continue;
        }
        let symbol = scene.add_symbol(format!("{label} part {index}"), buzz_scene::SymbolKind::Graphic, Some("Cast parts"));
        let Some(inner_layer) = scene.library().get(symbol).and_then(|s| s.layers.iter().next()).map(|l| l.id) else {
            continue;
        };
        let mut inner = (*art).clone();
        inner.transform = Affine::IDENTITY;
        inner.pivot = None;
        scene.library_mut().update(symbol, |s| {
            s.layers.update(inner_layer, |l| {
                l.frames.set_objects(0, vec![Arc::new(inner)]);
            });
        });
        let kind = Object::instance_of(id, symbol).kind;
        scene.update_object_across(0, u32::MAX, id, |o| o.kind = kind.clone());
        moved += 1;
    }
    moved
}

/// The app's `pose_layer_rig` at frame 0: the standard library's `pose` on the
/// rig's rest armature, every part layer keyed to where that pose puts it.
fn pose_layer_rig(scene: &mut Scene, view: &buzz_act::layer_rig::LayerRigView, pose: &str) -> bool {
    buzz_act::layer_rig::record_rests(scene, view);
    let Some((at_rest, parts)) = buzz_act::layer_rig::rest_armature(scene, view) else {
        return false;
    };
    let placed = buzz_act::layer_rig::placement(scene, view.body).unwrap_or(Affine::IDENTITY);
    let mut data = buzz_scene::ArmatureData::new(at_rest);
    if !buzz_ai::motion::PoseLibrary::standard().apply_to_armature(pose, &mut data) {
        return false;
    }
    let posed = &data.armature;
    let turns: Vec<Affine> = (0..posed.len()).map(|i| posed.pose_transform(i)).collect();
    for part in &parts {
        let Some(turn) = turns.get(part.slot) else {
            continue;
        };
        let world = placed * *turn * part.rest;
        scene.ensure_keyframe(part.layer, 0);
        let inherited = scene.layers().inherited_transform(part.layer, 0);
        let c = inherited.as_coeffs();
        let own = if (c[0] * c[3] - c[1] * c[2]).abs() > 1e-12 { inherited.inverse() * world } else { world };
        scene.update_object_at(0, part.object, |target| target.transform = own);
    }
    true
}

// ------------------------------------------------------------------ pictures

/// Frame 0 of `scene`, framed on its own drawing (layer parenting included) with
/// a 6 % margin, the longest side `longest` px, on the stage colour.
fn render_figure(exporter: &mut buzz_export::Exporter, scene: &Scene, longest: u32) -> Result<RgbaImage, String> {
    let mut union: Option<buzz_geom::Rect> = None;
    for layer in scene.layers().iter() {
        let inherited = scene.layers().inherited_transform(layer.id, 0);
        for o in layer.objects_at(0).iter() {
            let mut placed = (**o).clone();
            placed.transform = inherited * placed.transform;
            let b = scene.resolved_bounds(&placed);
            union = Some(union.map_or(b, |u| u.union(b)));
        }
    }
    let b = union.ok_or("nothing to preview")?;
    let margin = b.width().max(b.height()) * 0.06;
    let region = b.inflate(margin, margin);
    let scale = longest as f64 / region.width().max(region.height());
    let settings = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent: false,
        region: Some(region),
    };
    let frame = exporter.render(scene, 0, &settings).map_err(|e| format!("{e}"))?;
    let mut img = RgbaImage::from_raw(frame.width, frame.height, frame.pixels).ok_or("render size mismatch")?;
    for p in img.pixels_mut() {
        let [r, g, b] = over_white(p);
        *p = Rgba([r, g, b, 255]);
    }
    Ok(img)
}

/// The cells scaled to `height`, left to right on white, 16 px apart.
fn side_by_side(cells: &[RgbaImage], height: u32) -> RgbaImage {
    let scaled: Vec<RgbaImage> = cells
        .iter()
        .map(|c| {
            let w = ((c.width() as f64) * height as f64 / c.height().max(1) as f64).round().max(1.0) as u32;
            image::imageops::resize(c, w, height, FilterType::Lanczos3)
        })
        .collect();
    let gap = 16u32;
    let width = scaled.iter().map(|c| c.width() + gap).sum::<u32>().saturating_sub(gap).max(1);
    let mut sheet = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));
    let mut x = 0i64;
    for c in &scaled {
        image::imageops::overlay(&mut sheet, c, x, 0);
        x += (c.width() + gap) as i64;
    }
    sheet
}

// ------------------------------------------------------------------ spill (as rig_vs_source)

const MAGENTA: [u8; 3] = [255, 0, 255];

fn over_white(p: &Rgba<u8>) -> [u8; 3] {
    let a = p[3] as f32 / 255.0;
    let c = |v: u8| (v as f32 * a + 255.0 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    [c(p[0]), c(p[1]), c(p[2])]
}

/// (spill px beyond 1 px, furthest spill px) of the assembled rig's frame 0 on
/// magenta at the picture's size, as `rig_vs_source` measures it.
fn spill_of(exporter: &mut buzz_export::Exporter, scene: &Scene, figure: &Figure) -> Result<(usize, f64), String> {
    let mut scene = scene.clone();
    scene.stage_mut().background = peniko::Color::from_rgb8(MAGENTA[0], MAGENTA[1], MAGENTA[2]);
    let (w, h) = (figure.w as u32, figure.h as u32);
    let mut settings = buzz_export::ExportSettings::for_stage(&scene);
    settings.width = w + w % 2;
    settings.height = h + h % 2;
    let frame = exporter.render(&scene, 0, &settings).map_err(|e| format!("{e}"))?;
    let mut img = RgbaImage::from_raw(frame.width, frame.height, frame.pixels).ok_or("render size mismatch")?;
    if img.dimensions() != (w, h) {
        img = image::imageops::resize(&img, w, h, FilterType::Lanczos3);
    }
    for p in img.pixels_mut() {
        let [r, g, b] = over_white(p);
        *p = Rgba([r, g, b, 255]);
    }
    let (_, beyond, max) = figure.spill(&img);
    Ok((beyond, max))
}

/// The source's figure as `spill.py` builds it, and every pixel's distance to it.
struct Figure {
    w: usize,
    h: usize,
    /// Squared Euclidean distance to the nearest figure pixel (0 inside it).
    dist2: Vec<f64>,
}

impl Figure {
    /// Pixels differing from the one at (2, 2) by more than 20 in some RGB
    /// channel, closed by 2 with the 4-neighbour cross, holes filled.
    fn of(source: &RgbaImage) -> Self {
        let (w, h) = (source.width() as usize, source.height() as usize);
        let corner = *source.get_pixel(2.min(w as u32 - 1), 2.min(h as u32 - 1));
        let mut mask: Vec<bool> =
            source.pixels().map(|p| (0..3).any(|c| (p[c] as i32 - corner[c] as i32).abs() > 20)).collect();
        let step = |m: &[bool], dilate: bool| -> Vec<bool> {
            let mut out = vec![false; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let n = [
                        (x > 0).then(|| m[i - 1]),
                        (x + 1 < w).then(|| m[i + 1]),
                        (y > 0).then(|| m[i - w]),
                        (y + 1 < h).then(|| m[i + w]),
                    ];
                    out[i] = if dilate { m[i] || n.iter().any(|v| *v == Some(true)) } else { m[i] && n.iter().all(|v| *v == Some(true)) };
                }
            }
            out
        };
        for _ in 0..2 {
            mask = step(&mask, true);
        }
        for _ in 0..2 {
            mask = step(&mask, false);
        }
        let mut outside = vec![false; w * h];
        let mut stack = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if (x == 0 || y == 0 || x + 1 == w || y + 1 == h) && !mask[i] {
                    outside[i] = true;
                    stack.push(i);
                }
            }
        }
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let mut next = Vec::with_capacity(4);
            if x > 0 {
                next.push(i - 1);
            }
            if x + 1 < w {
                next.push(i + 1);
            }
            if y > 0 {
                next.push(i - w);
            }
            if y + 1 < h {
                next.push(i + w);
            }
            for j in next {
                if !mask[j] && !outside[j] {
                    outside[j] = true;
                    stack.push(j);
                }
            }
        }
        let figure: Vec<bool> = outside.iter().map(|o| !o).collect();
        Figure { w, h, dist2: squared_edt(&figure, w, h) }
    }

    /// (spill px, spill px more than 1 px from the figure, the furthest's distance)
    /// of a render on magenta: pixels more than 8 from magenta in some channel.
    fn spill(&self, render: &RgbaImage) -> (usize, usize, f64) {
        let (mut px, mut beyond, mut max2) = (0usize, 0usize, 0.0f64);
        for (i, p) in render.pixels().enumerate() {
            let painted = (0..3).any(|c| (p[c] as i32 - MAGENTA[c] as i32).abs() > 8);
            let d2 = self.dist2[i];
            if painted && d2 > 0.0 {
                px += 1;
                if d2 > 1.0 {
                    beyond += 1;
                }
                max2 = max2.max(d2);
            }
        }
        (px, beyond, max2.sqrt())
    }
}

/// Exact squared Euclidean distance to the nearest `seed` pixel (Felzenszwalb
/// and Huttenlocher, columns then rows).
fn squared_edt(seed: &[bool], w: usize, h: usize) -> Vec<f64> {
    const FAR: f64 = 1e10;
    let mut d: Vec<f64> = seed.iter().map(|&s| if s { 0.0 } else { FAR }).collect();
    let n = w.max(h);
    let (mut f, mut out, mut v, mut z) = (vec![0.0; n], vec![0.0; n], vec![0usize; n], vec![0.0; n + 1]);
    let pass = |f: &[f64], out: &mut [f64], v: &mut [usize], z: &mut [f64]| {
        let n = f.len();
        let mut k = 0usize;
        v[0] = 0;
        z[0] = f64::NEG_INFINITY;
        z[1] = f64::INFINITY;
        for q in 1..n {
            loop {
                let p = v[k];
                let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64);
                if s <= z[k] && k > 0 {
                    k -= 1;
                    continue;
                }
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f64::INFINITY;
                break;
            }
        }
        k = 0;
        for q in 0..n {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let p = v[k];
            out[q] = (q as f64 - p as f64).powi(2) + f[p];
        }
    };
    for x in 0..w {
        for y in 0..h {
            f[y] = d[y * w + x];
        }
        pass(&f[..h], &mut out[..h], &mut v, &mut z);
        for y in 0..h {
            d[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&d[y * w..y * w + w]);
        pass(&f[..w], &mut out[..w], &mut v, &mut z);
        d[y * w..y * w + w].copy_from_slice(&out[..w]);
    }
    d
}
