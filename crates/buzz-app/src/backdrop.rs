//! **Backgrounds from ComfyUI** (PRODUCER_PLAN § P3): a scene description →
//! a Z-Image Turbo picture → four depth plates (Sky, Far, Mid, Ground) →
//! traced vector layers at the AS3 set depths → an asset.
//!
//! The workflow files live with BuzzEdit (`<estate>/BuzzEdit/workflows/`) and
//! are read at run time, never copied into the repo.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use buzz_ai::asset_sets::SetRole;
use buzz_doc::{Asset, AssetLibrary};
use buzz_geom::{Affine, Size};
use buzz_scene::{
    FillSpec, Gradient, GradientKind, GradientStop, LayerKind, MAX_STOPS, Paint, Scene, ShapeData, TraceMode,
    TraceOptions,
};
use kurbo::Shape as _;
use peniko::Color;
use image::RgbaImage;
use rayon::prelude::*;
use serde_json::Value;

use crate::comfy::ComfyClient;

/// The size the workflow is asked for; the hq workflow's 1.5× pass makes it 1920×1080.
pub const BASE_SIZE: (u32, u32) = (1280, 720);
/// The stage the traced backdrop covers.
pub const STAGE: (f64, f64) = (1920.0, 1080.0);
/// How long one generation may take (a cold model load included).
const GENERATE_TIMEOUT: Duration = Duration::from_secs(900);

// ------------------------------------------------------------------ style

/// A look for the backgrounds: words appended to every prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub name: String,
    pub suffix: String,
}

impl Style {
    /// The house style (the user's Animate films): flat cel colours, bold
    /// clean black ink outlines. The default.
    pub fn inked() -> Self {
        Self {
            name: "inked".into(),
            suffix: format!(
                "2D animated-series background in a clean vector cartoon style, professional illustration, \
                 bold clean black ink outlines on every shape, flat cel-shaded colours with at most one soft \
                 shade per surface, simple readable shapes, layered foreground foliage, consistent perspective, {FRAMING}"
            ),
        }
    }

    /// `inked`, warmer and lighter: the Originals series.
    pub fn comic() -> Self {
        let inked = Self::inked();
        Self {
            name: "comic".into(),
            suffix: format!("{}, warm cheerful palette, light-hearted", inked.suffix),
        }
    }

    pub fn painterly() -> Self {
        Self {
            name: "painterly".into(),
            suffix: format!(
                "2D animated-film background painting, professional illustration, painterly flat colour \
                 fields with crisp edges, rich atmospheric light, {FRAMING}"
            ),
        }
    }

    /// `inked` / `comic` / `painterly`.
    pub fn by_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "inked" => Some(Self::inked()),
            "comic" => Some(Self::comic()),
            "painterly" => Some(Self::painterly()),
            _ => None,
        }
    }

    /// Does this look ask for black ink outlines? (Prompt wording only: every
    /// style traces as colour regions, see [`BackdropOptions::for_style`].)
    pub fn is_inked(&self) -> bool {
        self.name != "painterly"
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::inked()
    }
}

/// Where the camera stands and how much of the frame the set takes: in every
/// style's suffix (P6 review: a stall filling the frame left the cast no
/// ground to stand on).
pub const FRAMING: &str = "wide establishing shot from eye level, the main structure in the middle distance taking \
                           about half the frame width, open flat ground across the foreground where characters will stand";

/// What every background prompt starts with: Z-Image weighs the first words most.
pub const EMPTY_LEAD: &str = "An empty location with nobody in it: no people, no characters, no humans, no animals. ";

/// The prompt for one background: nobody in it first, then the scene, the
/// style (with [`FRAMING`], added when a free-text style lacks it), and the
/// exclusions again at the end (the workflow zeroes the negative, so they
/// ride in the prose).
pub fn background_prompt(scene: &str, style: &Style) -> String {
    let framing = if style.suffix.contains(FRAMING) { String::new() } else { format!(", {FRAMING}") };
    format!(
        "{EMPTY_LEAD}{}. {}{framing}, wide establishing shot, consistent perspective, an empty set: no people, \
         no characters, no animals, no text, no watermark, no frame",
        scene.trim().trim_end_matches('.'),
        style.suffix
    )
}

// ------------------------------------------------------------------ generate

/// `COMFY_WORKFLOWS` (a folder) when set, else `<estate>/BuzzEdit/workflows/`:
/// `zimage1_hq.json`, else `zimage1.json`.
pub fn workflow_path() -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = std::env::var("COMFY_WORKFLOWS")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain(crate::estate::estate_root().map(|r| r.join("BuzzEdit").join("workflows")))
        .collect();
    dirs.iter()
        .flat_map(|d| [d.join("zimage1_hq.json"), d.join("zimage1.json")])
        .find(|p| p.is_file())
}

/// Bind the prompt, size, seed and file prefix into a Z-Image workflow.
pub fn bind_workflow(workflow: &mut Value, prompt: &str, seed: u64, size: (u32, u32)) -> Result<(), String> {
    let mut set = |node: &str, key: &str, value: Value, required: bool| -> Result<(), String> {
        match workflow.get_mut(node).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
            Some(inputs) => {
                inputs.insert(key.into(), value);
                Ok(())
            }
            None if required => Err(format!("the workflow has no node '{node}'")),
            None => Ok(()),
        }
    };
    set("70", "text", Value::from(prompt), true)?;
    set("66", "width", Value::from(size.0), true)?;
    set("66", "height", Value::from(size.1), true)?;
    set("69", "seed", Value::from(seed), true)?;
    set("hires", "seed", Value::from(seed), false)?;
    set("9", "filename_prefix", Value::from("buzzanimate-bg"), true)?;
    Ok(())
}

/// Generate `prompt` at `seed` into `out_png`. Cached: an existing file is reused.
pub fn generate(client: &ComfyClient, prompt: &str, seed: u64, out_png: &Path) -> Result<PathBuf, String> {
    if out_png.is_file() {
        return Ok(out_png.to_path_buf());
    }
    let path = workflow_path().ok_or("no Z-Image workflow found (COMFY_WORKFLOWS or <estate>/BuzzEdit/workflows)")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut workflow: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    bind_workflow(&mut workflow, prompt, seed, BASE_SIZE)?;
    let id = client.queue(workflow)?;
    let outputs = client.wait(&id, GENERATE_TIMEOUT)?;
    let first = outputs.first().ok_or("ComfyUI returned no picture")?;
    let bytes = client.view(first)?;
    let picture = image::load_from_memory(&bytes).map_err(|e| format!("{}: {e}", first.filename))?;
    if let Some(dir) = out_png.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    picture.to_rgba8().save(out_png).map_err(|e| format!("{}: {e}", out_png.display()))?;
    Ok(out_png.to_path_buf())
}

// ------------------------------------------------------------------ people check

/// A person box at least this tall (a fraction of the picture's height) means
/// the generator drew somebody.
pub const PERSON_MIN_HEIGHT: f32 = 0.08;

/// The heights (fractions of the picture's height) of the people DWPose finds
/// in `picture` that are at least [`PERSON_MIN_HEIGHT`] tall, tallest first.
/// The detector's stand-in box when it finds nobody (the whole picture inside
/// a 2 % margin) is not a person.
///
/// P8-C4: DWPose also looks at the left and the right half, each upscaled to
/// the full width (a group of small figures on a bench merged into one box,
/// or went unseen, at the whole picture's scale). A hit in any of the three
/// counts; boxes found more than once (mostly overlapping) count once.
pub fn people_in(picture: &RgbaImage) -> Result<Vec<f32>, String> {
    let h = picture.height() as f32;
    // Person boxes in `img`, mapped back to the picture by `scale` and `dx`.
    let boxes_in = |img: &RgbaImage, scale: f32, dx: f32| -> Result<Vec<[f32; 4]>, String> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let persons = buzz_ai::pose::detect(img).map_err(|e| format!("pose: {e}"))?;
        let fallback = [w * 0.02, h * 0.02, w - w * 0.02, h - h * 0.02];
        Ok(persons
            .iter()
            .filter(|p| !p.bbox.iter().zip(fallback).all(|(a, b)| (a - b).abs() < 1.5))
            .map(|p| {
                let b = p.bbox;
                [b[0].min(b[2]) / scale + dx, b[1].min(b[3]) / scale, b[0].max(b[2]) / scale + dx, b[1].max(b[3]) / scale]
            })
            .collect())
    };
    let mut boxes = boxes_in(picture, 1.0, 0.0)?;
    let half = picture.width() / 2;
    if half >= 8 && picture.height() >= 8 {
        let up = |x0: u32| {
            let crop = image::imageops::crop_imm(picture, x0, 0, half, picture.height()).to_image();
            image::imageops::resize(&crop, half * 2, picture.height() * 2, image::imageops::FilterType::Triangle)
        };
        for x0 in [0, half] {
            // A half the detector cannot read does not hide the whole's hits.
            if let Ok(found) = boxes_in(&up(x0), 2.0, x0 as f32) {
                boxes.extend(found);
            }
        }
    }
    // The same figure seen in the whole and in a half (boxes overlapping by
    // half their union): one person. A whole-picture box round a merged
    // group overlaps each single figure far less, so both are kept.
    let area = |b: &[f32; 4]| ((b[2] - b[0]) * (b[3] - b[1])).max(1e-6);
    let mut people: Vec<[f32; 4]> = Vec::new();
    for b in boxes {
        let dup = people.iter().any(|p| {
            let iw = (p[2].min(b[2]) - p[0].max(b[0])).max(0.0);
            let ih = (p[3].min(b[3]) - p[1].max(b[1])).max(0.0);
            let i = iw * ih;
            i / (area(p) + area(&b) - i).max(1e-6) > 0.5
        });
        if !dup {
            people.push(b);
        }
    }
    let mut heights: Vec<f32> = people
        .iter()
        .map(|b| (b[3] - b[1]) / h.max(1.0))
        .filter(|f| *f >= PERSON_MIN_HEIGHT)
        .collect();
    heights.sort_by(|a, b| b.total_cmp(a));
    Ok(heights)
}

/// One try of [`generate_checked`].
#[derive(Debug, Clone)]
pub struct PeopleTry {
    pub seed: u64,
    pub png: PathBuf,
    /// Person heights found (fractions of the picture's height); `None` when
    /// the check could not run.
    pub people: Option<Vec<f32>>,
}

impl PeopleTry {
    pub fn describe(&self, n: usize) -> String {
        match &self.people {
            Some(p) if p.is_empty() => format!("try {n} (seed {}) found 0 people", self.seed),
            Some(p) => format!(
                "try {n} (seed {}) found {} {} ({})",
                self.seed,
                p.len(),
                if p.len() == 1 { "person" } else { "people" },
                p.iter().map(|f| format!("{:.0} %", f * 100.0)).collect::<Vec<_>>().join(", ")
            ),
            None => format!("try {n} (seed {}): people check unavailable", self.seed),
        }
    }
}

/// [`generate`] with a **people check**: each picture is looked at by DWPose,
/// and one with a person [`PERSON_MIN_HEIGHT`] or taller is drawn again with
/// the next seed, up to `tries` pictures. The one with the fewest (then the
/// smallest) people is copied to `out_png`. Each try is cached beside it as
/// `<stem>.try<n>.png`; `client` is only needed for a try not yet on disk.
/// With `check` off this is one try, unchecked.
pub fn generate_checked(
    client: Option<&ComfyClient>,
    prompt: &str,
    seed: u64,
    out_png: &Path,
    tries: usize,
    check: bool,
    progress: &mut dyn FnMut(&str),
) -> Result<(PathBuf, Vec<PeopleTry>), String> {
    let stem = out_png.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "bg".into());
    let dir = out_png.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut done: Vec<PeopleTry> = Vec::new();
    for n in 0..tries.max(1) {
        let s = seed.wrapping_add(n as u64);
        let path = dir.join(format!("{stem}.try{}.png", n + 1));
        if !path.is_file() {
            let client = client.ok_or("ComfyUI is not running")?;
            progress(&format!("try {}: generating (seed {s})\u{2026}", n + 1));
            generate(client, prompt, s, &path)?;
        }
        let people = if check {
            match image::open(&path) {
                Ok(img) => people_in(&img.to_rgba8()).ok(),
                Err(_) => None,
            }
        } else {
            None
        };
        let t = PeopleTry { seed: s, png: path, people };
        progress(&t.describe(n + 1));
        let clean = t.people.as_ref().is_none_or(|p| p.is_empty());
        done.push(t);
        if !check || clean {
            break;
        }
    }
    let score = |t: &PeopleTry| match &t.people {
        Some(p) => (p.len(), p.iter().sum::<f32>()),
        None => (usize::MAX, 0.0),
    };
    let best = done
        .iter()
        .min_by(|a, b| {
            let (sa, sb) = (score(a), score(b));
            sa.0.cmp(&sb.0).then(sa.1.total_cmp(&sb.1))
        })
        .ok_or("no picture")?;
    std::fs::copy(&best.png, out_png).map_err(|e| format!("{}: {e}", out_png.display()))?;
    Ok((out_png.to_path_buf(), done))
}

// ------------------------------------------------------------------ plates

/// The picture split by depth. Each plate is the full picture's size,
/// transparent where it holds nothing.
pub struct Plates {
    pub sky: RgbaImage,
    pub far: RgbaImage,
    pub mid: RgbaImage,
    pub ground: RgbaImage,
    /// The horizon row, in pixels.
    pub horizon: u32,
}

const SKY: u8 = 0;
const FAR: u8 = 1;
const MID: u8 = 2;
const GROUND: u8 = 3;
const NONE: u8 = 255;

/// Sky flood tolerance (CIELAB ΔE76) to the top-row palette.
const SKY_DELTA: f32 = 18.0;
/// One step of a gradient sky (ΔE76 between neighbours).
const SKY_STEP: f32 = 2.0;
/// Only a light sky is followed by steps (a dark night sky beside dark trees is not).
const SKY_CHAIN_LIGHT: f32 = 45.0;
/// The mean texture (ΔE76 to a neighbour) of a top-row colour that may seed the sky.
const SKY_SMOOTH: f32 = 3.0;
/// How far a gradient sky may drift from the top-row palette (ΔE76).
const SKY_REACH: f32 = 25.0;
/// Back plates reach this far under the plates in front, so no seam shows.
const OVERLAP: u32 = 3;

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn lab(rgb: [u8; 3]) -> [f32; 3] {
    let l = buzz_scene::trace::srgb_to_lab(rgb);
    [l[0] as f32, l[1] as f32, l[2] as f32]
}

fn nearest(palette: &[[f32; 3]], c: [f32; 3]) -> (usize, f32) {
    palette
        .iter()
        .enumerate()
        .map(|(i, p)| (i, dist2(*p, c)))
        .fold((0, f32::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
}

/// Multi-source 4-connected BFS: every pixel with `owner == NONE` that `open`
/// allows takes the owner of the nearest seeded pixel, up to `max` steps.
fn spread(owner: &mut [u8], w: usize, h: usize, open: impl Fn(usize) -> bool, max: u32) {
    let mut dist = vec![u32::MAX; w * h];
    let mut queue = VecDeque::new();
    for i in 0..w * h {
        if owner[i] != NONE {
            dist[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        if dist[i] >= max {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if owner[j] == NONE && dist[j] == u32::MAX && open(j) {
                owner[j] = owner[i];
                dist[j] = dist[i] + 1;
                queue.push_back(j);
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
}

/// Like [`spread`], carrying colour: every pixel `open` allows, up to `max`
/// steps, takes the colour of the nearest pixel where `has` is set.
fn spread_colour(rgba: &mut [[u8; 4]], has: &[bool], w: usize, h: usize, open: impl Fn(usize) -> bool, max: u32) {
    let mut dist = vec![u32::MAX; w * h];
    let mut queue = VecDeque::new();
    for i in 0..w * h {
        if has[i] {
            dist[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        if dist[i] >= max {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let c = rgba[i];
        let mut visit = |j: usize| {
            if dist[j] == u32::MAX && open(j) {
                rgba[j] = [c[0], c[1], c[2], 255];
                dist[j] = dist[i] + 1;
                queue.push_back(j);
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
}

/// Square erosion (`keep_min`) or dilation of a mask, radius `r`, separable.
fn morph(mask: &[bool], w: usize, h: usize, r: usize, erode: bool) -> Vec<bool> {
    let pass = |src: &[bool], horizontal: bool| -> Vec<bool> {
        let mut out = vec![false; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let mut acc = erode;
                for d in -(r as isize)..=(r as isize) {
                    let (sx, sy) = if horizontal { (x as isize + d, y as isize) } else { (x as isize, y as isize + d) };
                    let v = if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                        erode
                    } else {
                        src[sy as usize * w + sx as usize]
                    };
                    if erode {
                        acc &= v;
                    } else {
                        acc |= v;
                    }
                }
                row[x] = acc;
            }
        });
        out
    };
    let a = pass(mask, true);
    pass(&a, false)
}

/// **Split a picture into Sky, Far, Mid and Ground plates.**
///
/// Sky = a flood from the top rows within ΔE 18 of the top-row palette;
/// horizon = the lowest row where the sky still covers ≥ 30 % of the width
/// (clamped to 0.25..0.75 of the height); Ground = what lies below
/// horizon + 2 % and belongs to no colour component rising above that line;
/// the other components are Far (bottom above horizon + 3 %) or Mid. Thin
/// dark lines (ink) join the plate of the nearest non-ink pixel. The pixels
/// hidden behind Mid are filled into Far from the nearest Far/Sky colour, and
/// every back plate reaches a few pixels under the plates in front of it.
pub fn split_plates(rgba: &RgbaImage) -> Plates {
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let px: Vec<[u8; 4]> = rgba.pixels().map(|p| p.0).collect();
    let labs: Vec<[f32; 3]> = px.par_iter().map(|p| lab([p[0], p[1], p[2]])).collect();

    // Sky palette: the top strip quantised, colours covering ≥ 5 % of it.
    let strip_rows = (h / 100).max(2).min(h);
    let strip = &rgba.as_raw()[..w * strip_rows * 4];
    let candidates: Vec<[f32; 3]> = buzz_scene::quantise(strip, w, strip_rows, 8).into_iter().map(lab).collect();
    // Texture: the larger ΔE to the right / lower neighbour. A sky is smooth;
    // foliage reaching the top edge is not, and must not seed the palette.
    let texture = |i: usize| -> f32 {
        let (x, y) = (i % w, i / w);
        let right = if x + 1 < w { dist2(labs[i], labs[i + 1]) } else { 0.0 };
        let down = if y + 1 < h { dist2(labs[i], labs[i + w]) } else { 0.0 };
        right.max(down).sqrt()
    };
    let mut share = vec![0usize; candidates.len()];
    let mut rough = vec![0f32; candidates.len()];
    for i in 0..w * strip_rows {
        let k = nearest(&candidates, labs[i]).0;
        share[k] += 1;
        rough[k] += texture(i);
    }
    let palette: Vec<[f32; 3]> = candidates
        .iter()
        .enumerate()
        .filter(|(k, _)| share[*k] * 20 >= w * strip_rows && rough[*k] / (share[*k] as f32) < SKY_SMOOTH)
        .map(|(_, c)| *c)
        .collect();
    let near_sky = |i: usize| nearest(&palette, labs[i]).1 <= SKY_DELTA * SKY_DELTA;

    let mut owner = vec![NONE; w * h];
    let mut queue = VecDeque::new();
    for x in 0..w {
        if near_sky(x) {
            owner[x] = SKY;
            queue.push_back(x);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        // A gradient sky is followed down in small steps (≤ SKY_STEP from the
        // sky pixel next to it) while it stays within SKY_REACH of the palette.
        let mut visit = |j: usize| {
            if owner[j] == NONE
                && (near_sky(j)
                    || (labs[j][0] >= SKY_CHAIN_LIGHT
                        && dist2(labs[i], labs[j]) <= SKY_STEP * SKY_STEP
                        && nearest(&palette, labs[j]).1 <= SKY_REACH * SKY_REACH))
            {
                owner[j] = SKY;
                queue.push_back(j);
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

    let lowest = (0..h)
        .rev()
        .find(|&y| owner[y * w..(y + 1) * w].iter().filter(|o| **o == SKY).count() * 10 >= w * 3)
        .unwrap_or(0);
    let horizon = lowest.clamp(h / 4, h * 3 / 4);
    let ground_line = horizon + h * 2 / 100;
    let far_line = horizon + h * 3 / 100;

    // Thin dark lines: dark pixels an opening of radius 3 removes.
    let dark: Vec<bool> = (0..w * h).map(|i| owner[i] != SKY && labs[i][0] < 35.0).collect();
    let opened = morph(&morph(&dark, w, h, 3, true), w, h, 3, false);
    let line: Vec<bool> = (0..w * h).map(|i| dark[i] && !opened[i]).collect();

    // Colour components of everything else.
    let palette16: Vec<[f32; 3]> = buzz_scene::quantise(rgba.as_raw(), w, h, 16).into_iter().map(lab).collect();
    let label: Vec<u8> = labs.par_iter().map(|c| nearest(&palette16, *c).0 as u8).collect();
    let mut comp = vec![u32::MAX; w * h];
    let mut spans: Vec<(usize, usize)> = Vec::new(); // (top, bottom)
    let mut stack = Vec::new();
    for start in 0..w * h {
        if owner[start] != NONE || line[start] || comp[start] != u32::MAX {
            continue;
        }
        let id = spans.len() as u32;
        let (mut top, mut bottom) = (usize::MAX, 0);
        comp[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            top = top.min(y);
            bottom = bottom.max(y);
            let mut visit = |j: usize| {
                if comp[j] == u32::MAX && owner[j] == NONE && !line[j] && label[j] == label[i] {
                    comp[j] = id;
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
        spans.push((top, bottom));
    }
    for i in 0..w * h {
        if comp[i] == u32::MAX {
            continue;
        }
        let (top, bottom) = spans[comp[i] as usize];
        owner[i] = if top >= ground_line {
            GROUND
        } else if bottom < far_line {
            FAR
        } else {
            MID
        };
    }
    // Islands in the sky (a moon, a cloud) belong to it: a connected patch of
    // non-sky pixels, wholly surrounded by sky, made only of Far components
    // and ink, under 5 % of the picture.
    {
        let mut seen = vec![false; w * h];
        let mut stack = Vec::new();
        for start in 0..w * h {
            if owner[start] == SKY || seen[start] {
                continue;
            }
            let mut patch = Vec::new();
            let mut island = true;
            seen[start] = true;
            stack.push(start);
            while let Some(i) = stack.pop() {
                patch.push(i);
                if owner[i] != FAR && owner[i] != NONE {
                    island = false;
                }
                let (x, y) = (i % w, i / w);
                if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                    island = false;
                }
                let near = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < w).then(|| i + 1),
                    (y > 0).then(|| i - w),
                    (y + 1 < h).then(|| i + w),
                ];
                for j in near.into_iter().flatten() {
                    if owner[j] != SKY && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            if island && patch.len() * 20 < w * h {
                for i in patch {
                    owner[i] = SKY;
                }
            }
        }
    }
    // Ink joins the nearest plate.
    spread(&mut owner, w, h, |_| true, u32::MAX);

    let mut plates: Vec<Vec<[u8; 4]>> = (0..4u8)
        .map(|p| (0..w * h).map(|i| if owner[i] == p { px[i] } else { [0, 0, 0, 0] }).collect())
        .collect();

    // Behind Mid: the nearest Far/Sky colour, into Far.
    {
        let has: Vec<bool> = owner.iter().map(|o| *o == FAR || *o == SKY).collect();
        let mut source: Vec<[u8; 4]> =
            (0..w * h).map(|i| if has[i] { px[i] } else { [0, 0, 0, 0] }).collect();
        spread_colour(&mut source, &has, w, h, |j| owner[j] == MID, u32::MAX);
        for i in 0..w * h {
            if owner[i] == MID && source[i][3] == 255 {
                plates[FAR as usize][i] = source[i];
            }
        }
    }
    // Back plates reach under the ones in front.
    for p in [SKY, FAR, MID] {
        let plate = &mut plates[p as usize];
        let has: Vec<bool> = plate.iter().map(|c| c[3] > 0).collect();
        spread_colour(plate, &has, w, h, |j| owner[j] > p, OVERLAP);
    }

    let to_image = |v: &Vec<[u8; 4]>| {
        RgbaImage::from_raw(w as u32, h as u32, v.iter().flatten().copied().collect()).expect("plate size")
    };
    Plates {
        sky: to_image(&plates[0]),
        far: to_image(&plates[1]),
        mid: to_image(&plates[2]),
        ground: to_image(&plates[3]),
        horizon: horizon as u32,
    }
}

// ------------------------------------------------------------------ trace

/// How the plates are traced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackdropOptions {
    /// Palette size per plate.
    pub colours: usize,
    /// `TraceOptions::smooth`.
    pub smoothness: f64,
    /// `TraceOptions::speckle`: the smallest region kept, in px².
    pub speckle: usize,
    /// `TraceOptions::ink_bold` (ink only).
    pub bold: f64,
    /// Ink over colour (`InkAndColour`) when true, colour regions only (`Outline`) when false.
    pub ink: bool,
}

impl Default for BackdropOptions {
    fn default() -> Self {
        Self { colours: 48, smoothness: 0.8, speckle: 24, bold: 0.0, ink: false }
    }
}

impl BackdropOptions {
    /// The defaults, for every style: colour regions (`Outline`). Z-Image
    /// draws thin brown lines rather than bold black ink even when asked, and
    /// `InkAndColour` turns its output into blotches, so the inked styles keep
    /// their prompt wording but not the ink trace.
    pub fn for_style(_style: &Style) -> Self {
        Self::default()
    }

    pub fn trace_options(&self) -> TraceOptions {
        if self.ink {
            TraceOptions {
                colours: self.colours,
                smooth: self.smoothness,
                speckle: self.speckle,
                ink_bold: self.bold,
                drop_background: false,
                ..TraceOptions::ink_colour()
            }
        } else {
            TraceOptions {
                mode: TraceMode::Outline,
                colours: self.colours,
                smooth: self.smoothness,
                speckle: self.speckle,
                drop_background: false,
                ..TraceOptions::default()
            }
        }
    }
}

/// **Trace the plates into a 1920×1080 scene**: layers `Sky` (cover, unlit),
/// `Far` (depth 0.85), `Mid` (0.6), `Ground` (0), back to front, the picture
/// scaled to cover the stage.
pub fn trace_plates(plates: &Plates, opts: &BackdropOptions) -> Scene {
    let mut scene = Scene::default();
    let starter: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
    for id in starter {
        scene.remove_layer(id);
    }
    scene.stage_mut().size = Size::new(STAGE.0, STAGE.1);
    let (w, h) = (plates.sky.width() as f64, plates.sky.height() as f64);
    let s = (STAGE.0 / w).max(STAGE.1 / h);
    let place = Affine::translate(((STAGE.0 - w * s) / 2.0, (STAGE.1 - h * s) / 2.0)) * Affine::scale(s);
    let options = opts.trace_options();
    let rows: [(&str, &RgbaImage, SetRole, f64); 4] = [
        ("Sky", &plates.sky, SetRole::Sky, SetRole::Sky.depth()),
        ("Far", &plates.far, SetRole::Far, SetRole::Far.depth()),
        ("Mid", &plates.mid, SetRole::Mid, SetRole::Mid.depth()),
        ("Ground", &plates.ground, SetRole::Ground, 0.0),
    ];
    for (name, plate, role, depth) in rows {
        let layer = scene.add_layer(name, LayerKind::Normal);
        let shapes = if role == SetRole::Sky {
            // The sky as gradients (a vertical ramp, radial glows), with only
            // what they cannot explain traced, always as colour regions: the
            // sky carries no ink.
            let fit = fit_sky(plate);
            let sky_options = BackdropOptions { ink: false, ..*opts }.trace_options();
            let rest = buzz_scene::trace(fit.residual.width(), fit.residual.height(), fit.residual.as_raw(), &sky_options);
            fit.shapes.into_iter().chain(rest.shapes).collect::<Vec<_>>()
        } else {
            buzz_scene::trace(plate.width(), plate.height(), plate.as_raw(), &options).shapes
        };
        for mut shape in shapes {
            shape.path = place * shape.path;
            if let Some(stroke) = shape.stroke.as_mut() {
                stroke.width *= s;
            }
            if let Some(fill) = shape.fill.as_mut() {
                if let Paint::Gradient(g) = &fill.paint {
                    fill.paint = Paint::Gradient(std::sync::Arc::new(g.transformed(place)));
                }
            }
            scene.add_shape_at(layer, 0, shape);
        }
        scene.update_stage_layer(layer, |l| {
            l.depth = depth;
            l.casts_shadow = false;
            if role.covers() {
                l.unlit = true;
            }
        });
    }
    scene
}

// ------------------------------------------------------------------ sky fit

/// The Sky plate as gradients: what [`trace_plates`] draws for it.
pub struct SkyFit {
    /// Pixel space: the vertical ramp over the plate's bounds, then one
    /// radial glow per bright blob with a smooth falloff.
    pub shapes: Vec<ShapeData>,
    /// The sky pixels the gradients do not explain (ΔE > 6), in parts of at
    /// least 0.05 % of the picture; transparent elsewhere. Traced as colour
    /// regions over the gradients.
    pub residual: RgbaImage,
    /// Stops in the vertical ramp.
    pub stops: usize,
    /// Stops in each radial glow.
    pub glow_stops: Vec<usize>,
    /// Residual parts kept.
    pub parts: usize,
}

/// A residual pixel is this far (ΔE76) from the fitted sky.
const SKY_RESIDUAL: f32 = 6.0;
/// The smallest residual part kept, as a fraction of the picture.
const SKY_PART: f64 = 0.0005;
/// A glow's core stands at least this far (ΔE76) from the sky round it.
const GLOW_CONTRAST: f32 = 12.0;
/// A glow's falloff spans at least this fraction of the width.
const GLOW_SPAN: f64 = 0.03;

fn rgb_lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn lab_f(c: [f32; 3]) -> [f32; 3] {
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    lab([q(c[0]), q(c[1]), q(c[2])])
}

fn colour(c: [f32; 3], alpha: f32) -> Color {
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Color::from_rgba8(q(c[0]), q(c[1]), q(c[2]), q(alpha * 255.0))
}

fn rgb_of(c: Color) -> ([f32; 3], f32) {
    let c = c.to_rgba8().to_u8_array();
    ([c[0] as f32, c[1] as f32, c[2] as f32], c[3] as f32 / 255.0)
}

/// Indices into `values` for at most `max` stops: the two ends, then
/// wherever the curve leaves the straight line between its stops the most
/// (ΔE76), until it stays within `tol`.
fn fit_stops(values: &[[f32; 3]], max: usize, tol: f32) -> Vec<usize> {
    let n = values.len();
    if n < 2 {
        return vec![0; n.min(1)];
    }
    let mut idx = vec![0, n - 1];
    while idx.len() < max {
        let mut best = (0.0f32, 0usize);
        for pair in idx.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            for i in a + 1..b {
                let t = (i - a) as f32 / (b - a) as f32;
                let d = dist2(lab_f(values[i]), lab_f(rgb_lerp(values[a], values[b], t))).sqrt();
                if d > best.0 {
                    best = (d, i);
                }
            }
        }
        if best.0 < tol {
            break;
        }
        idx.push(best.1);
        idx.sort_unstable();
    }
    idx
}

/// **Fit the Sky plate**: a vertical linear gradient from the per-row mean
/// of the sky pixels (stops where the row-mean curve bends most); radial
/// glows for bright blobs whose brightness falls off smoothly over ≥ 3 % of
/// the width (centre colour → sky, alpha to 0 at the edge); and the residual
/// the gradients leave, for tracing.
pub fn fit_sky(plate: &RgbaImage) -> SkyFit {
    let (w, h) = (plate.width() as usize, plate.height() as usize);
    let px = plate.as_raw();
    let mask: Vec<bool> = (0..w * h).map(|i| px[i * 4 + 3] > 0).collect();
    let rgb = |i: usize| [px[i * 4] as f32, px[i * 4 + 1] as f32, px[i * 4 + 2] as f32];
    let Some(y0) = (0..h).find(|&y| (0..w).any(|x| mask[y * w + x])) else {
        return SkyFit {
            shapes: Vec::new(),
            residual: RgbaImage::new(w as u32, h as u32),
            stops: 0,
            glow_stops: Vec::new(),
            parts: 0,
        };
    };
    let y1 = (0..h).rev().find(|&y| (0..w).any(|x| mask[y * w + x])).unwrap_or(y0);
    let x0 = (0..w).find(|&x| (y0..=y1).any(|y| mask[y * w + x])).unwrap_or(0);
    let x1 = (0..w).rev().find(|&x| (y0..=y1).any(|y| mask[y * w + x])).unwrap_or(w - 1);

    // Per-row colour of the sky, a row with no sky taking the nearest row
    // above (or below). The per-channel median rather than the mean: the
    // plate also holds a moon's glow and canopy caught by the flood, and a
    // mean dragged by them left the clean sky either side "off" the ramp.
    let means: Vec<Option<[f32; 3]>> = (y0..=y1)
        .into_par_iter()
        .map(|y| {
            let mut chans: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for x in 0..w {
                let i = y * w + x;
                if mask[i] {
                    for (c, chan) in chans.iter_mut().enumerate() {
                        chan.push(px[i * 4 + c]);
                    }
                }
            }
            if chans[0].is_empty() {
                return None;
            }
            let mut out = [0f32; 3];
            for (c, chan) in chans.iter_mut().enumerate() {
                let mid = chan.len() / 2;
                out[c] = *chan.select_nth_unstable(mid).1 as f32;
            }
            Some(out)
        })
        .collect();
    let mut last = means.iter().flatten().next().copied().unwrap_or([0.0; 3]);
    let rows: Vec<[f32; 3]> = means
        .iter()
        .map(|m| {
            if let Some(c) = m {
                last = *c;
            }
            last
        })
        .collect();
    let n = rows.len();
    let idx = fit_stops(&rows, MAX_STOPS, 1.0);
    let span = (y1 + 1 - y0) as f64;
    let offset = |k: usize| ((k as f64 + 0.5) / span).clamp(0.0, 1.0);
    let stops: Vec<GradientStop> = idx.iter().map(|&k| GradientStop::new(offset(k), colour(rows[k], 1.0))).collect();
    let mut ramp = Gradient::new(GradientKind::Linear, stops);
    let rect = kurbo::Rect::new(x0 as f64, y0 as f64, (x1 + 1) as f64, (y1 + 1) as f64);
    // Unit x (−1..1) runs down the rectangle.
    ramp.transform = Affine::translate(rect.center().to_vec2())
        * Affine::rotate(std::f64::consts::FRAC_PI_2)
        * Affine::scale_non_uniform(rect.height() / 2.0, rect.width() / 2.0);
    let row_model: Vec<[f32; 3]> = (0..n).map(|k| rgb_of(ramp.sample(offset(k))).0).collect();
    let mut model: Vec<[f32; 3]> = (0..w * h)
        .map(|i| if (y0..=y1).contains(&(i / w)) { row_model[i / w - y0] } else { [0.0; 3] })
        .collect();
    let mut shapes = vec![ShapeData {
        path: rect.to_path(0.1),
        fill: Some(FillSpec::gradient(ramp)),
        stroke: None,
        blend: Default::default(),
    }];

    // Residual parts.
    let labs: Vec<[f32; 3]> = (0..w * h).into_par_iter().map(|i| if mask[i] { lab_f(rgb(i)) } else { [0.0; 3] }).collect();
    let off = |i: usize, model: &[[f32; 3]]| mask[i] && dist2(labs[i], lab_f(model[i])) > SKY_RESIDUAL * SKY_RESIDUAL;
    let residual: Vec<bool> = (0..w * h).into_par_iter().map(|i| off(i, &model)).collect();
    let min_part = ((w * h) as f64 * SKY_PART) as usize;
    let parts = parts_of(&residual, w, h, min_part);

    // Glows.
    let mut glow_stops = Vec::new();
    let mut discs: Vec<(f64, f64, f64)> = Vec::new();
    for part in &parts {
        // The centre: the centroid of the part's bright core — every pixel
        // above half-way between its median and its brightest 2 %.
        let mut by_light: Vec<usize> = part.clone();
        by_light.sort_by(|a, b| labs[*b][0].total_cmp(&labs[*a][0]));
        let top = (part.len() / 50).max(1);
        let peak = by_light[..top].iter().map(|&i| labs[i][0]).sum::<f32>() / top as f32;
        let median = labs[by_light[by_light.len() / 2]][0];
        let cut = (peak + median) / 2.0;
        let bright: Vec<usize> = by_light.into_iter().take_while(|&i| labs[i][0] >= cut).collect();
        let (sx, sy) = bright.iter().fold((0.0, 0.0), |(sx, sy), &i| (sx + (i % w) as f64, sy + (i / w) as f64));
        let (cx, cy) = (sx / bright.len() as f64, sy / bright.len() as f64);
        // The radial profile of every sky pixel, against the ramp under it.
        let reach = (w as f64 * 0.25) as usize;
        let mut sum = vec![([0f32; 3], [0f32; 3], 0f32); reach + 1];
        let (bx0, bx1) = ((cx - reach as f64).max(0.0) as usize, ((cx + reach as f64) as usize).min(w - 1));
        let (by0, by1) = ((cy - reach as f64).max(0.0) as usize, ((cy + reach as f64) as usize).min(h - 1));
        for y in by0..=by1 {
            for x in bx0..=bx1 {
                let i = y * w + x;
                let r = (x as f64 - cx).hypot(y as f64 - cy).round() as usize;
                if !mask[i] || r > reach {
                    continue;
                }
                let (c, m) = (rgb(i), model[i]);
                let e = &mut sum[r];
                e.0 = [e.0[0] + c[0], e.0[1] + c[1], e.0[2] + c[2]];
                e.1 = [e.1[0] + m[0], e.1[1] + m[1], e.1[2] + m[2]];
                e.2 += 1.0;
            }
        }
        let profile: Vec<Option<([f32; 3], [f32; 3])>> = sum
            .iter()
            .map(|(c, m, k)| (*k > 0.0).then(|| ([c[0] / k, c[1] / k, c[2] / k], [m[0] / k, m[1] / k, m[2] / k])))
            .collect();
        let delta = |r: usize| profile[r].map(|(p, b)| dist2(lab_f(p), lab_f(b)).sqrt());
        let light = |r: usize| profile[r].map(|(p, _)| lab_f(p)[0]);
        // Where the glow has faded into the ramp (or the sky runs out).
        let Some(r_end) = (1..=reach).find(|&r| delta(r).is_none_or(|d| d < 2.0)) else {
            continue;
        };
        // The disc's edge: where the lightness falls below half-way between
        // the core and the sky the glow fades into.
        let core = (0..r_end).filter_map(light).fold(f32::MIN, f32::max);
        let Some(outer) = light(r_end).or_else(|| (0..r_end).rev().find_map(light)) else {
            continue;
        };
        let half = (core + outer) / 2.0;
        let Some(r0) = (0..r_end).find(|&r| light(r).is_some_and(|l| l < half) && (0..r).any(|k| light(k).is_some())) else {
            continue;
        };
        let smooth = (r0 + 1..r_end).all(|r| match (light(r - 1), light(r)) {
            (Some(a), Some(b)) => b <= a + 1.0,
            _ => false,
        });
        if !smooth || ((r_end - r0) as f64) < GLOW_SPAN * w as f64 {
            continue;
        }
        if delta(r0).is_none_or(|d| d < GLOW_CONTRAST) {
            continue;
        }
        // The glow is its core colour (the profile at the disc's edge) laid
        // over the sky at the alpha that best explains each ring: the
        // projection of (ring − sky) onto (core − sky); 0 at the edge, where
        // the colour runs out to the sky's.
        let Some((core_colour, _)) = profile[r0] else {
            continue;
        };
        let glow: Vec<([f32; 3], f32)> = (0..=r_end)
            .map(|r| {
                if r >= r_end {
                    return (profile[r].map(|(_, b)| b).unwrap_or(core_colour), 0.0);
                }
                if r <= r0 {
                    return (core_colour, 1.0);
                }
                match profile[r] {
                    Some((p, b)) => {
                        let (u, v) = (
                            [p[0] - b[0], p[1] - b[1], p[2] - b[2]],
                            [core_colour[0] - b[0], core_colour[1] - b[1], core_colour[2] - b[2]],
                        );
                        let vv = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
                        let a = if vv > 0.0 { (u[0] * v[0] + u[1] * v[1] + u[2] * v[2]) / vv } else { 0.0 };
                        (core_colour, a.clamp(0.0, 1.0))
                    }
                    None => (core_colour, 0.0),
                }
            })
            .collect();
        // Fit on what the glow shows over mid-grey, so alpha counts.
        let shown: Vec<[f32; 3]> = glow.iter().map(|(c, a)| rgb_lerp([128.0; 3], *c, *a)).collect();
        let gidx = fit_stops(&shown, MAX_STOPS, 1.0);
        glow_stops.push(gidx.len());
        let gstops: Vec<GradientStop> =
            gidx.iter().map(|&r| GradientStop::new(r as f64 / r_end as f64, colour(glow[r].0, glow[r].1))).collect();
        let mut radial = Gradient::new(GradientKind::Radial, gstops);
        let circle = kurbo::Circle::new((cx, cy), r_end as f64);
        radial.fit_to(circle.bounding_box());
        // The model under the glow, for the residual.
        for y in by0..=by1 {
            for x in bx0..=bx1 {
                let i = y * w + x;
                let r = (x as f64 - cx).hypot(y as f64 - cy) / r_end as f64;
                if r >= 1.0 {
                    continue;
                }
                let (c, a) = rgb_of(radial.sample(r));
                model[i] = rgb_lerp(model[i], c, a);
            }
        }
        discs.push((cx, cy, r0 as f64));
        shapes.push(ShapeData {
            path: circle.to_path(0.1),
            fill: Some(FillSpec::gradient(radial)),
            stroke: None,
            blend: Default::default(),
        });
    }

    // What is left to trace: the kept parts, minus what the glows explain
    // (parts measured again), and each glow's disc.
    let in_disc = |i: usize| {
        let (x, y) = ((i % w) as f64, (i / w) as f64);
        discs.iter().any(|&(cx, cy, r0)| (x - cx).hypot(y - cy) <= r0)
    };
    let mut kept_part = vec![false; w * h];
    for part in &parts {
        for &i in part {
            kept_part[i] = true;
        }
    }
    let left: Vec<bool> = (0..w * h).into_par_iter().map(|i| kept_part[i] && off(i, &model) && !in_disc(i)).collect();
    // Each disc whole, 2 px inside its half-way edge: the glow's core
    // colour shows in that rim, as an anti-aliased edge would.
    let disc_px: Vec<usize> = (0..w * h)
        .filter(|&i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            mask[i] && discs.iter().any(|&(cx, cy, r0)| (x - cx).hypot(y - cy) <= r0 - 2.0)
        })
        .collect();
    let mut out = RgbaImage::new(w as u32, h as u32);
    let left_parts = parts_of(&left, w, h, min_part);
    let kept = left_parts.len() + discs.len();
    for i in left_parts.into_iter().flatten().chain(disc_px) {
        out.put_pixel((i % w) as u32, (i / w) as u32, image::Rgba([px[i * 4], px[i * 4 + 1], px[i * 4 + 2], 255]));
    }
    SkyFit { shapes, residual: out, stops: idx.len(), glow_stops, parts: kept }
}

/// 4-connected parts of `on` of at least `min` pixels.
fn parts_of(on: &[bool], w: usize, h: usize, min: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; w * h];
    let mut parts = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !on[start] || seen[start] {
            continue;
        }
        let mut part = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            part.push(i);
            let (x, y) = (i % w, i / w);
            let near = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            for j in near.into_iter().flatten() {
                if on[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        if part.len() >= min {
            parts.push(part);
        }
    }
    parts
}

/// Save the traced backdrop as `Backgrounds/<series>/<key>` (any `/`-path:
/// the last part is the name), with `png` copied beside it.
pub fn save_asset(scene: &Scene, library: &mut AssetLibrary, path: &str, png: Option<&Path>) -> Result<Asset, String> {
    let path = path.trim_matches('/');
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    let asset = library.save(name, folder, scene).map_err(|e| format!("saving {path}: {e}"))?;
    if let Some(png) = png {
        let beside = asset.path.with_extension("png");
        std::fs::copy(png, &beside).map_err(|e| format!("{}: {e}", beside.display()))?;
    }
    Ok(asset)
}
