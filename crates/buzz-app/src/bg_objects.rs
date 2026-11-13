//! **Background objects as symbols** (ANIMATED_CHANNELS_PLAN AN3).
//!
//! After a background is split into plates, the VLM lists its objects (house, tree,
//! shop, pole, cart, wall...) with boxes; SAM 2 masks each; each masked crop is traced
//! with the plates' own tracer options into a symbol of its own, assigned to the plate
//! its bottom edge sits in, and the hole it leaves in that plate is filled from the
//! surrounding pixels, so moving the symbol never shows a hole. Any failure skips that
//! object with a note; the background still comes out as plates, exactly as without.

use std::sync::Arc;

use buzz_geom::Affine;
use buzz_scene::{LayerKind, Object, ObjectKind, Scene, ShapeData};
use image::{GrayImage, RgbaImage};

use crate::backdrop::{BackdropOptions, Plates, STAGE};

/// The plates an object can sit in (the sky holds no objects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateId {
    Far,
    Mid,
    Ground,
}

impl PlateId {
    /// The layer name `backdrop::trace_plates` gives the plate.
    pub fn name(self) -> &'static str {
        match self {
            PlateId::Far => "Far",
            PlateId::Mid => "Mid",
            PlateId::Ground => "Ground",
        }
    }
    fn of(self, plates: &Plates) -> &RgbaImage {
        match self {
            PlateId::Far => &plates.far,
            PlateId::Mid => &plates.mid,
            PlateId::Ground => &plates.ground,
        }
    }
}

/// How objects are cut.
#[derive(Debug, Clone)]
pub struct ObjectOptions {
    /// At most this many objects per picture, largest and most central first.
    pub max_objects: usize,
    /// Keep small far objects too (a back row of houses): lower box and mask floors, ranked
    /// without favouring big central ones, and the VLM is told to list every far building.
    pub small_objects: bool,
    /// The tracer options of the plates.
    pub trace: BackdropOptions,
    /// Ask the VLM again even when a saved object list matches the picture, and save the new
    /// list over the old one (Shift on the menu command, `redetect` for the agent).
    pub redetect: bool,
    /// An indoor picture (`scene_is_indoor`): its furniture and props are asked for instead of
    /// buildings, and the split makes one Room layer instead of Sky / Far / Land.
    pub indoor: bool,
}

impl Default for ObjectOptions {
    fn default() -> Self {
        Self { max_objects: 8, small_objects: false, trace: BackdropOptions::default(), redetect: false, indoor: false }
    }
}

/// One object cut out of a background.
pub struct CutObject {
    /// `House`, `Tree`...
    pub label: String,
    /// 1-based, per label.
    pub n: usize,
    /// `[x0, y0, x1, y1]` in picture pixels.
    pub bbox: [u32; 4],
    pub depth_plate: PlateId,
    /// The object's matte, full picture size (0 / 255).
    pub mask: GrayImage,
    /// The traced crop: one layer, shapes in crop-local pixels (origin at the bbox corner).
    pub symbol_scene: Scene,
}

/// The objects and the plates with their holes filled.
pub struct ObjectCut {
    pub objects: Vec<CutObject>,
    pub filled_plates: Plates,
    /// Why anything was skipped, one line each.
    pub notes: Vec<String>,
}

/// Where an object was placed in the saved background.
#[derive(Debug, Clone)]
pub struct Placed {
    pub label: String,
    pub n: usize,
    pub plate: &'static str,
    pub bbox: [u32; 4],
    pub symbol: String,
}

/// Whether objects are cut for `series`: the request's flag, else the series
/// profile's `objects_as_symbols`, else on for Chirailiya / Originals.
pub fn enabled(request: Option<bool>, series: &str) -> bool {
    if let Some(b) = request {
        return b;
    }
    if let Some(b) = crate::comfy_profile::profile_for(series).and_then(|p| p.objects_as_symbols) {
        return b;
    }
    matches!(series.trim().to_lowercase().as_str(), "chirailiya" | "originals")
}

// ------------------------------------------------------------------ labels

/// `banyan_tree` -> `Tree`, `tea stall` -> `Tea Stall`, anything else Title Case (two words at most).
pub fn normalise_label(raw: &str) -> String {
    let l = raw.to_lowercase().replace(['_', '-'], " ");
    let table: [(&str, &str); 22] = [
        ("tea", "Tea Stall"),
        ("stall", "Stall"),
        ("shop", "Shop"),
        ("store", "Shop"),
        ("house", "House"),
        ("hut", "House"),
        ("home", "House"),
        ("building", "House"),
        ("temple", "Temple"),
        ("tree", "Tree"),
        ("pole", "Pole"),
        ("cart", "Cart"),
        ("wall", "Wall"),
        ("fence", "Wall"),
        ("well", "Well"),
        ("gate", "Gate"),
        ("bench", "Bench"),
        ("tower", "Tower"),
        ("bush", "Bush"),
        ("haystack", "Haystack"),
        ("sign", "Signboard"),
        ("lamp", "Pole"),
    ];
    for (key, name) in table {
        if l.split_whitespace().any(|w| w.starts_with(key)) {
            return name.to_string();
        }
    }
    short_label(&l)
}

/// An indoor object's label (`scene_is_indoor`): the VLM's own words, shortened. The street
/// names of [`normalise_label`] do not apply indoors (a lamp is no pole, a kettle no tea stall).
pub fn normalise_indoor_label(raw: &str) -> String {
    short_label(&raw.to_lowercase().replace(['_', '-'], " "))
}

/// The first two words of `l` before any joining word ("Door with iron bars" -> "Door"),
/// each capitalised; "Object" when that leaves nothing.
fn short_label(l: &str) -> String {
    let label = l
        .split_whitespace()
        .take_while(|w| !matches!(*w, "with" | "of" | "and" | "on" | "in" | "for" | "behind" | "near"))
        .take(2)
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    if label.is_empty() { "Object".into() } else { label }
}

fn is_person_or_animal(label: &str, kind: &str) -> bool {
    if matches!(kind, "character" | "animal" | "effect" | "background") {
        return true;
    }
    let l = label.to_lowercase();
    ["person", "people", "man", "woman", "boy", "girl", "child", "villager", "kid", "dog", "cow", "bird", "goat", "buffalo", "cat", "crowd"]
        .iter()
        .any(|w| l.split(|c: char| !c.is_alphabetic()).any(|t| t == *w || t == format!("{w}s")))
}

// ------------------------------------------------------------------ plates

/// The plate an object's bottom edge sits in: the front-most plate holding the
/// mask's lowest three rows, else by the row against nothing known (Ground).
pub fn plate_at_bottom(plates: &Plates, mask: &GrayImage) -> PlateId {
    let (w, h) = mask.dimensions();
    let Some(bottom) = (0..h).rev().find(|&y| (0..w).any(|x| mask.get_pixel(x, y)[0] > 127)) else {
        return PlateId::Ground;
    };
    let mut votes = [0usize; 3];
    let order = [PlateId::Ground, PlateId::Mid, PlateId::Far];
    for y in bottom.saturating_sub(2)..=bottom {
        for x in 0..w {
            if mask.get_pixel(x, y)[0] <= 127 {
                continue;
            }
            for (i, p) in order.iter().enumerate() {
                let plate = p.of(plates);
                if x < plate.width() && y < plate.height() && plate.get_pixel(x, y)[3] > 128 {
                    votes[i] += 1;
                    break;
                }
            }
        }
    }
    let best = (0..3).max_by_key(|&i| (votes[i], 3 - i)).unwrap_or(0);
    if votes[best] == 0 {
        return if bottom <= plates.horizon { PlateId::Far } else { PlateId::Ground };
    }
    order[best]
}

/// Remove `mask` (grown by `grow` px) from `plate` and fill the hole from the
/// surrounding pixels; the plate stays transparent where it already was.
pub fn fill_hole(plate: &mut RgbaImage, mask: &GrayImage, grow: i32) {
    let (w, h) = plate.dimensions();
    if mask.dimensions() != (w, h) {
        return;
    }
    let grown = buzz_ai::segment::dilate(mask.as_raw(), w, h, grow.max(0));
    let n = (w * h) as usize;
    let mut hole = vec![false; n];
    let (mut sum, mut count) = ([0u64; 3], 0u64);
    for i in 0..n {
        let opaque = plate.as_raw()[i * 4 + 3] > 128;
        if opaque && grown[i] > 0 {
            hole[i] = true;
        } else if opaque {
            for c in 0..3 {
                sum[c] += plate.as_raw()[i * 4 + c] as u64;
            }
            count += 1;
        }
    }
    let fallback = if count > 0 { [(sum[0] / count) as u8, (sum[1] / count) as u8, (sum[2] / count) as u8] } else { [128, 128, 128] };
    let smooth = crate::comfy_profile::smooth_hole_fill_active();
    let raw: &mut [u8] = &mut *plate;
    for i in 0..n {
        if hole[i] {
            raw[i * 4 + 3] = 0;
        }
    }
    if smooth {
        smooth_fill(raw, w, h, &hole, fallback);
    } else {
        buzz_ai::outpaint::nearest_visible_fill(raw, w, h, &hole, fallback);
    }
}

/// A hole filled by push-pull: the known pixels averaged down a pyramid, then each empty pixel
/// takes the bilinear value of the coarser level. A smooth blend of the surroundings, no
/// directional streaks (the nearest-pixel fill's wedges, which a trace turns into shards).
fn smooth_fill(rgba: &mut [u8], w: u32, h: u32, hole: &[bool], fallback: [u8; 3]) {
    struct Level {
        w: usize,
        h: usize,
        rgb: Vec<[f32; 3]>,
        wt: Vec<f32>,
    }
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    let mut rgb = vec![[0f32; 3]; n];
    let mut wt = vec![0f32; n];
    for i in 0..n {
        if !hole[i] && rgba[i * 4 + 3] > 128 {
            rgb[i] = [rgba[i * 4] as f32, rgba[i * 4 + 1] as f32, rgba[i * 4 + 2] as f32];
            wt[i] = 1.0;
        }
    }
    let mut levels = vec![Level { w, h, rgb, wt }];
    while levels.last().is_some_and(|l| l.w > 1 || l.h > 1) {
        let l = levels.last().unwrap();
        let (nw, nh) = (l.w.div_ceil(2), l.h.div_ceil(2));
        let mut nrgb = vec![[0f32; 3]; nw * nh];
        let mut nwt = vec![0f32; nw * nh];
        for y in 0..l.h {
            for x in 0..l.w {
                let (i, j) = (y * l.w + x, (y / 2) * nw + x / 2);
                let wi = l.wt[i];
                if wi > 0.0 {
                    for c in 0..3 {
                        nrgb[j][c] += l.rgb[i][c] * wi;
                    }
                    nwt[j] += wi;
                }
            }
        }
        for j in 0..nw * nh {
            if nwt[j] > 0.0 {
                for c in 0..3 {
                    nrgb[j][c] /= nwt[j];
                }
                nwt[j] = nwt[j].min(1.0);
            }
        }
        levels.push(Level { w: nw, h: nh, rgb: nrgb, wt: nwt });
    }
    for li in (0..levels.len() - 1).rev() {
        let (fine, coarse) = levels.split_at_mut(li + 1);
        let (f, c) = (&mut fine[li], &coarse[0]);
        for y in 0..f.h {
            for x in 0..f.w {
                let i = y * f.w + x;
                if f.wt[i] > 0.0 {
                    continue;
                }
                let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (c.w - 1) as f32);
                let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (c.h - 1) as f32);
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(c.w - 1), (y0 + 1).min(c.h - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let mut acc = [0f32; 3];
                let mut total = 0f32;
                for (cx, cy, k) in [(x0, y0, (1.0 - tx) * (1.0 - ty)), (x1, y0, tx * (1.0 - ty)), (x0, y1, (1.0 - tx) * ty), (x1, y1, tx * ty)] {
                    let ci = cy * c.w + cx;
                    if c.wt[ci] > 0.0 && k > 0.0 {
                        for ch in 0..3 {
                            acc[ch] += c.rgb[ci][ch] * k;
                        }
                        total += k;
                    }
                }
                if total > 0.0 {
                    f.rgb[i] = [acc[0] / total, acc[1] / total, acc[2] / total];
                    f.wt[i] = 1.0;
                }
            }
        }
    }
    let base = &levels[0];
    for i in 0..n {
        if !hole[i] {
            continue;
        }
        let c = if base.wt[i] > 0.0 { base.rgb[i] } else { [fallback[0] as f32, fallback[1] as f32, fallback[2] as f32] };
        rgba[i * 4..i * 4 + 4].copy_from_slice(&[c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255]);
    }
}

/// Back to front over: sky, far, mid, ground.
fn flatten(plates: &Plates) -> RgbaImage {
    let mut out = plates.sky.clone();
    for p in [&plates.far, &plates.mid, &plates.ground] {
        image::imageops::overlay(&mut out, p, 0, 0);
    }
    out
}

impl ObjectCut {
    /// The picture with each object's pixels replaced by the filled plates: what
    /// the plate trace should be compared with.
    pub fn picture_without_objects(&self, picture: &RgbaImage) -> RgbaImage {
        let flat = flatten(&self.filled_plates);
        let mut out = picture.clone();
        if flat.dimensions() != out.dimensions() {
            return out;
        }
        for o in &self.objects {
            let grown = buzz_ai::segment::dilate(o.mask.as_raw(), out.width(), out.height(), 2);
            for (i, g) in grown.iter().enumerate() {
                let (x, y) = ((i as u32) % out.width(), (i as u32) / out.width());
                if *g > 0 && flat.get_pixel(x, y)[3] > 128 {
                    out.put_pixel(x, y, *flat.get_pixel(x, y));
                }
            }
        }
        out
    }
}

// ------------------------------------------------------------------ cutting

fn png_bytes(img: &RgbaImage) -> Option<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(buf.into_inner())
}

fn iou(a: [u32; 4], b: [u32; 4]) -> f64 {
    let (x0, y0, x1, y1) = (a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3]));
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    let inter = ((x1 - x0) as f64) * ((y1 - y0) as f64);
    let area = |r: [u32; 4]| ((r[2] - r[0]) as f64) * ((r[3] - r[1]) as f64);
    inter / (area(a) + area(b) - inter).max(1.0)
}

/// The vision model objects are asked of: the Claude API when set up, else LM Studio's.
fn vlm() -> Option<(buzz_ai::llm::Backend, String)> {
    let claude = buzz_ai::ai_config::use_claude();
    let backend = if claude { buzz_ai::llm::Backend::default_claude_api() } else { buzz_ai::llm::Backend::default_openai() };
    let model = if claude { Some(buzz_ai::ai_config::claude_model()) } else { crate::produce::lmstudio_model(&backend).map(|m| m.0) };
    model.filter(|m| !m.is_empty()).map(|m| (backend, m))
}

/// Whether `picture` is an **indoor** scene with no sky (a room, the inside of a shop), asked of
/// the VLM at temperature 0. The answer is saved with the picture's object list and reused with
/// it (unless `opts.redetect`); for a list saved before the question existed it is asked.
/// Without a VLM, or when it fails, the picture is taken as outdoor (Sky / Far / Land).
pub fn scene_is_indoor(picture: &RgbaImage, opts: &ObjectOptions, notes: &mut Vec<String>) -> bool {
    use crate::split_cache as cache;
    let Some((backend, model)) = vlm() else { return false };
    if !opts.redetect {
        // A saved indoor list, else a saved list that was asked and found outdoor.
        if cache::find(picture, &model, opts.small_objects, Some(true)).is_some() {
            return true;
        }
        if cache::find(picture, &model, opts.small_objects, Some(false)).is_some_and(|s| s.indoor == Some(false)) {
            return false;
        }
    }
    let Some(png) = png_bytes(picture) else { return false };
    let sampling = buzz_ai::llm::ChatOpts { temperature: Some(cache::TEMPERATURE), seed: Some(cache::SEED), no_thinking: true };
    match buzz_ai::scene_builder::scene_is_indoor(&backend, &model, &png, sampling) {
        Ok(indoor) => indoor,
        Err(e) => {
            notes.push(format!("scene: could not ask whether it is indoors ({e}); split as outdoor"));
            false
        }
    }
}

/// Ask the local VLM for the picture's objects (people and animals dropped), best first.
///
/// **Same picture, same list** (SPLIT_PICTURE_PLAN S11): the VLM is asked at temperature 0 with
/// a fixed seed; the split artwork's second look is the left and right halves of the picture
/// (10 % overlap), asked again at full detail, boxes mapped back and unioned with the first
/// pass; the final list is saved (`split_cache`) and reused for the same picture unless
/// `opts.redetect`. Every call also makes a run record (`split_runs`) with `input.png` and
/// `objects.json`; the split that called adds `result.json` (`split_cache::take_last_run`).
/// Each box comes with whether it is a **fragment** (S14): a box from a half pass whose inner edge
/// sits on that half's cut line, so the building behind it may have been cut in two.
fn candidates(picture: &RgbaImage, opts: &ObjectOptions, notes: &mut Vec<String>) -> Vec<(String, [u32; 4], bool)> {
    use crate::split_cache as cache;
    let (w, h) = picture.dimensions();
    let Some(png) = png_bytes(picture) else {
        notes.push("objects: could not encode the picture for the VLM".into());
        return Vec::new();
    };
    let Some((backend, model)) = vlm() else {
        notes.push("objects: no VLM available; plates only".into());
        return Vec::new();
    };
    let run = cache::start_run(&png);
    cache::set_last_run(run.clone());
    let record = |value: serde_json::Value| {
        if let Some(dir) = &run {
            cache::write_json(dir, "objects.json", &value);
        }
    };
    let route = if opts.small_objects { "artwork" } else { "picture" };
    // No hidden reasoning (S17): gemma-4-12b at temperature 0 can spend its whole token budget thinking
    // and list nothing (194 s); without it the same request answers in about 12 s.
    let sampling = buzz_ai::llm::ChatOpts { temperature: Some(cache::TEMPERATURE), seed: Some(cache::SEED), no_thinking: true };
    // The local VLM server drops a request now and then: a pass that fails is asked once more, but
    // after the others are done (below), not in its own thread.
    let ask = |bytes: &[u8]| {
        if opts.indoor {
            buzz_ai::scene_builder::decompose_indoor_objects_opts(&backend, &model, bytes, sampling)
        } else {
            buzz_ai::scene_builder::decompose_background_objects_opts(&backend, &model, bytes, opts.small_objects, sampling)
        }
    };
    let saved = cache::find(picture, &model, opts.small_objects, Some(opts.indoor));
    let reuse = saved.as_ref().filter(|_| !opts.redetect);
    let mut passes: Vec<serde_json::Value> = Vec::new();
    let (objects, ground_y, cache_file) = if let Some(s) = reuse {
        notes.push("objects: same list as the last split of this picture (saved list reused; hold Shift to detect again)".into());
        (s.objects.clone(), s.ground_y, Some(s.path.clone()))
    } else {
        // The passes run at the same time (three threads); their answers are joined and unioned
        // in a fixed order (whole picture, left half, right half), so the list never depends on
        // which request finishes first. Split artwork: the second and third look are each half
        // of the picture at full detail (a building missed or boxed badly in the whole view is
        // usually right in the half; the parts and duplicates are merged after SAM).
        let halves: &[(&str, (f64, f64))] = if opts.small_objects { &[("left half", (0.0, 0.55)), ("right half", (0.45, 1.0))] } else { &[] };
        let (mut whole, mut half_results) = std::thread::scope(|s| {
            let whole = s.spawn(|| ask(&png));
            let spawned: Vec<_> = halves.iter().map(|(name, span)| (*name, *span, s.spawn(|| ask_half(picture, *span, &ask)))).collect();
            let whole = whole.join().unwrap_or_else(|p| std::panic::resume_unwind(p));
            let half_results: Vec<_> = spawned.into_iter().map(|(name, span, t)| (name, span, t.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))).collect();
            (whole, half_results)
        });
        // SPLIT_PICTURE_PLAN S15: the concurrent requests are what overload the server, so a retry
        // made inside the failing thread meets the same load. After the join each failed pass is
        // asked once more, one at a time, in the fixed order (whole picture, left half, right half).
        if whole.is_err() {
            whole = ask(&png);
        }
        for (_, span, result) in half_results.iter_mut() {
            if result.is_err() {
                *result = ask_half(picture, *span, &ask);
            }
        }
        let mut layout = match whole {
            Ok(l) => l,
            Err(e) => {
                notes.push(format!("objects: the VLM failed ({e}); plates only"));
                record(serde_json::json!({ "route": route, "picture": [w, h], "model": model, "error": e.to_string() }));
                return Vec::new();
            }
        };
        passes.push(serde_json::json!({ "pass": "whole picture", "objects": layout.objects.len() }));
        let mut complete = true;
        for (name, span, result) in half_results {
            match result {
                Ok(mapped) => {
                    passes.push(serde_json::json!({ "pass": name, "x_range": [span.0, span.1], "objects": mapped.len() }));
                    layout.objects.extend(mapped);
                }
                Err(e) => {
                    complete = false;
                    notes.push(format!("objects: the {name} VLM pass failed ({e}); that half is missing from the list"));
                }
            }
        }
        // Only a complete list is saved; a re-detect writes over the entry it matched.
        let file = if complete {
            cache::save(picture, &model, opts.small_objects, opts.indoor, &layout.objects, layout.ground_y, saved.as_ref().map(|s| s.path.as_path()))
        } else {
            None
        };
        (layout.objects, layout.ground_y, file)
    };
    let mut raw_list: Vec<serde_json::Value> = Vec::new();
    let mut scored: Vec<(f64, String, [u32; 4], bool)> = Vec::new();
    for o in &objects {
        raw_list.push(serde_json::json!({
            "label": o.label, "kind": o.kind, "bbox_norm": o.bbox,
            "bbox_px": [o.bbox[0] * w as f64, o.bbox[1] * h as f64, o.bbox[2] * w as f64, o.bbox[3] * h as f64],
        }));
        if is_person_or_animal(&o.label, &o.kind) {
            continue;
        }
        let b = o.bbox.map(|v| v.clamp(0.0, 1.0));
        let (x0, y0, x1, y1) = ((b[0] * w as f64) as u32, (b[1] * h as f64) as u32, (b[2] * w as f64) as u32, (b[3] * h as f64) as u32);
        if x1 <= x0 + 4 || y1 <= y0 + 4 {
            continue;
        }
        let area = ((x1 - x0) as f64 * (y1 - y0) as f64) / (w as f64 * h as f64);
        if !(if opts.small_objects { SMALL_BOX_MIN } else { 0.004 }..=0.7).contains(&area) {
            notes.push(format!("objects: {} skipped (box covers {:.1}% of the picture)", o.label, area * 100.0));
            continue;
        }
        let cx = ((x0 + x1) as f64 / 2.0 / w as f64 - 0.5).abs() * 2.0;
        let score = if opts.small_objects { area.sqrt() } else { area * (1.2 - 0.8 * cx) };
        // The left half ends at 0.55 w and the right half starts at 0.45 w (the passes above); the
        // saved list holds no pass names, so the cut line is found from the box itself.
        let tol = 0.01 * w as f64;
        let fragment = opts.small_objects && ((x1 as f64 - 0.55 * w as f64).abs() <= tol || (x0 as f64 - 0.45 * w as f64).abs() <= tol);
        let label = if opts.indoor { normalise_indoor_label(&o.label) } else { normalise_label(&o.label) };
        scored.push((score, label, [x0, y0, x1, y1], fragment));
    }
    if let Some(dir) = debug_dir() {
        let _ = std::fs::write(
            dir.join("vlm.json"),
            serde_json::to_string_pretty(&serde_json::json!({ "picture": [w, h], "objects": raw_list, "ground_y": ground_y })).unwrap_or_default(),
        );
    }
    // One box over a street (S16c): the VLM sometimes boxes the whole street as one house. Dropped
    // here, after the cache, so a saved list is cleaned the same way.
    if opts.small_objects {
        let street: Vec<bool> = scored
            .iter()
            .enumerate()
            .map(|(i, (_, label, b, _))| {
                let area = ((b[2] - b[0]) as f64 * (b[3] - b[1]) as f64) / (w as f64 * h as f64);
                let holds = scored
                    .iter()
                    .enumerate()
                    .filter(|(j, (_, other, o, _))| *j != i && is_building(other) && (b[0]..=b[2]).contains(&((o[0] + o[2]) / 2)) && (b[1]..=b[3]).contains(&((o[1] + o[3]) / 2)))
                    .count();
                is_building(label) && area > STREET_BOX_MIN && holds >= STREET_HOLDS
            })
            .collect();
        let mut at = 0;
        scored.retain(|(_, label, bbox, _)| {
            at += 1;
            if street[at - 1] {
                notes.push(format!("objects: {label} {bbox:?} dropped (one box over several buildings)"));
            }
            !street[at - 1]
        });
    }
    // Best first; equal scores in a fixed order (box, then label), not the order the VLM listed them.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.cmp(&b.2)).then_with(|| a.1.cmp(&b.1)));
    let mut kept: Vec<(String, [u32; 4], bool)> = Vec::new();
    for (_, label, bbox, fragment) in scored {
        if kept.len() >= opts.max_objects {
            break;
        }
        if kept.iter().any(|(_, k, _)| iou(*k, bbox) > 0.6) {
            continue;
        }
        kept.push((label, bbox, fragment));
    }
    record(serde_json::json!({
        "route": route,
        "indoor": opts.indoor,
        "picture": [w, h],
        "model": model,
        "prompt_version": buzz_ai::scene_builder::BACKGROUND_OBJECTS_PROMPT_VERSION,
        "temperature": cache::TEMPERATURE,
        "seed": cache::SEED,
        "redetect": opts.redetect,
        "cache_hit": reuse.is_some(),
        "cache_file": cache_file.map(|p| p.to_string_lossy().to_string()),
        "passes": passes,
        "ground_y": ground_y,
        "vlm_objects": raw_list,
        "kept": kept.iter().map(|(label, bbox, fragment)| serde_json::json!({ "label": label, "bbox": bbox, "fragment": fragment })).collect::<Vec<_>>(),
    }));
    kept
}

/// The VLM's objects for one vertical slice of `picture` (`span` = its left and right edge as
/// shares of the width, the full height), asked at the slice's own resolution; boxes mapped back
/// to the whole picture (normalised).
fn ask_half(
    picture: &RgbaImage,
    span: (f64, f64),
    ask: &(dyn Fn(&[u8]) -> Result<buzz_ai::scene_builder::DetectedSceneLayout, buzz_ai::llm::LlmError> + Sync),
) -> Result<Vec<buzz_ai::scene_builder::DetectedSceneObject>, String> {
    let (w, h) = picture.dimensions();
    let x0 = ((span.0 * w as f64).round() as u32).min(w.saturating_sub(1));
    let x1 = ((span.1 * w as f64).round() as u32).clamp(x0 + 1, w);
    let cw = x1 - x0;
    let crop = image::imageops::crop_imm(picture, x0, 0, cw, h).to_image();
    let png = png_bytes(&crop).ok_or("could not encode the half")?;
    let layout = ask(&png).map_err(|e| e.to_string())?;
    let (scale, offset) = (cw as f64 / w as f64, x0 as f64 / w as f64);
    Ok(layout
        .objects
        .into_iter()
        .map(|mut o| {
            o.bbox = [offset + o.bbox[0] * scale, o.bbox[1], offset + o.bbox[2] * scale, o.bbox[3]];
            o
        })
        .collect())
}

/// `BUZZ_SPLIT_DEBUG_DIR`: where the VLM list, each SAM mask and each crop are dumped (off when unset).
fn debug_dir() -> Option<std::path::PathBuf> {
    let d = std::path::PathBuf::from(std::env::var_os("BUZZ_SPLIT_DEBUG_DIR")?);
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// The smallest share of its box a mask must cover to count as the object.
const MASK_MIN_OF_BOX: f64 = 0.35;
/// With `small_objects`: the smallest box and mask (shares of the picture) kept.
const SMALL_BOX_MIN: f64 = 0.0015;
const SMALL_MASK_MIN: f64 = 0.0008;
/// With `small_objects`: the share of its box a mask must cover (the VLM's box of a far
/// building often includes the neighbour in front).
const SMALL_MASK_MIN_OF_BOX: f64 = 0.2;
/// A building's mask is tried with more prompts until it covers this share of its box (S14: SAM
/// on the whole box of a house sometimes gives only its fence).
const BUILDING_MASK_OF_BOX: f64 = 0.5;
/// The most of a mask that may lie outside its box grown by 4 % on each side.
const OUTSIDE_MAX: f64 = 0.3;
/// With `small_objects`: a building box over more than this share of the picture that holds the
/// centres of `STREET_HOLDS` other building boxes is a whole street, not a building (S16c).
const STREET_BOX_MIN: f64 = 0.35;
const STREET_HOLDS: usize = 2;

/// The mask kept clean: closed, small holes filled, the largest piece plus the
/// pieces touching it (within 3 px).
fn clean(mask: &GrayImage) -> Vec<u8> {
    let (w, h) = mask.dimensions();
    let bin: Vec<u8> = mask.as_raw().iter().map(|&v| if v > 127 { 255 } else { 0 }).collect();
    let closed = buzz_ai::segment::morph_close(&bin, w, h, 2);
    let filled = buzz_ai::segment::fill_small_holes(&closed, w, h, 0.02);
    let main = buzz_ai::segment::largest_component(&filled, w, h);
    let near = buzz_ai::segment::dilate(&main, w, h, 3);
    let mut out = main;
    let (wi, hi) = (w as usize, h as usize);
    let mut seen = vec![false; wi * hi];
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..wi * hi {
        if filled[start] == 0 || seen[start] || out[start] > 0 {
            continue;
        }
        let mut piece = Vec::new();
        let mut touches = false;
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            piece.push(i);
            touches |= near[i] > 0;
            let (x, y) = (i % wi, i / wi);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= wi as i32 || ny >= hi as i32 {
                        continue;
                    }
                    let j = ny as usize * wi + nx as usize;
                    if filled[j] > 0 && !seen[j] && out[j] == 0 {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if touches {
            for i in piece {
                out[i] = 255;
            }
        }
    }
    out
}

/// A cleaned SAM 2 mask for `boxf`, and the share of the box it covers. Tried
/// with the box alone first; when that covers under `min_cover`, again
/// with the box plus its centre as a positive point, then (still short) with
/// four more points inside the box; the best one is kept. A mask with more than
/// `OUTSIDE_MAX` of its pixels outside the box grown by 4 % of its size on each side
/// (SAM took the neighbour) is passed over, unless every prompt gave such a mask.
fn mask_for_box(
    emb: &buzz_ai::segment::Sam2Embeddings,
    boxf: [f32; 4],
    size: (u32, u32),
    min_cover: f64,
) -> Result<Option<(Vec<u8>, f64)>, String> {
    let (w, h) = size;
    let box_area = ((boxf[2] - boxf[0]) as f64 * (boxf[3] - boxf[1]) as f64).max(1.0);
    let coverage = |m: &[u8]| -> f64 {
        let (x0, y0, x1, y1) = (boxf[0] as u32, boxf[1] as u32, (boxf[2] as u32).min(w), (boxf[3] as u32).min(h));
        let mut n = 0u64;
        for y in y0..y1 {
            n += m[(y * w + x0) as usize..(y * w + x1) as usize].iter().filter(|&&v| v > 0).count() as u64;
        }
        n as f64 / box_area
    };
    // The share of the mask's pixels outside the box grown by 4 % of its width and height.
    let outside = |m: &[u8]| -> f64 {
        let (gx, gy) = (0.04 * (boxf[2] - boxf[0]), 0.04 * (boxf[3] - boxf[1]));
        let x0 = (boxf[0] - gx).max(0.0) as u32;
        let y0 = (boxf[1] - gy).max(0.0) as u32;
        let x1 = ((boxf[2] + gx).ceil() as u32).min(w);
        let y1 = ((boxf[3] + gy).ceil() as u32).min(h);
        let (mut all, mut inside) = (0u64, 0u64);
        for y in 0..h {
            let row = &m[(y * w) as usize..((y + 1) * w) as usize];
            let n = row.iter().filter(|&&v| v > 0).count() as u64;
            all += n;
            if y >= y0 && y < y1 && x1 > x0 {
                inside += row[x0 as usize..x1 as usize].iter().filter(|&&v| v > 0).count() as u64;
            }
        }
        (all - inside) as f64 / all.max(1) as f64
    };
    // `fit` = the best mask that stays near the box; `any` = the best of all (when none stays).
    let mut fit: Option<(Vec<u8>, f64)> = None;
    let mut any: Option<(Vec<u8>, f64)> = None;
    let centre = ((boxf[0] + boxf[2]) / 2.0, (boxf[1] + boxf[3]) / 2.0, true);
    let at = |fx: f32, fy: f32| (boxf[0] + (boxf[2] - boxf[0]) * fx, boxf[1] + (boxf[3] - boxf[1]) * fy, true);
    for points in [vec![], vec![centre], vec![centre, at(0.3, 0.3), at(0.7, 0.3), at(0.3, 0.7), at(0.7, 0.7)]] {
        let mask = match buzz_ai::segment::sam2_mask(emb, &points, Some(boxf)) {
            Ok(Some((m, _))) if m.dimensions() == (w, h) => m,
            Ok(Some(_)) => return Err("mask has the wrong size".into()),
            Ok(None) => return Ok(None),
            Err(e) => return Err(format!("SAM 2: {e}")),
        };
        let raw = clean(&mask);
        let cov = coverage(&raw);
        let near = outside(&raw) <= OUTSIDE_MAX;
        let slot = if near { &mut fit } else { &mut any };
        if slot.as_ref().map_or(true, |b| cov > b.1) {
            *slot = Some((raw, cov));
        }
        if fit.as_ref().map_or(false, |b| b.1 >= min_cover) {
            break;
        }
    }
    Ok(fit.or(any))
}

/// Trace the masked crop of `picture` into a one-layer scene (crop-local pixels).
fn trace_crop(picture: &RgbaImage, mask: &GrayImage, bbox: [u32; 4], opts: &ObjectOptions) -> Option<Scene> {
    let (cw, ch) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    let mut crop = image::imageops::crop_imm(picture, bbox[0], bbox[1], cw, ch).to_image();
    for y in 0..ch {
        for x in 0..cw {
            let on = mask.get_pixel(bbox[0] + x, bbox[1] + y)[0] > 127;
            let p = crop.get_pixel_mut(x, y);
            p[3] = if on { 255 } else { 0 };
        }
    }
    let shapes = buzz_scene::trace(cw, ch, crop.as_raw(), &opts.trace.trace_options()).shapes;
    if shapes.is_empty() {
        return None;
    }
    let mut scene = Scene::default();
    let starter: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
    for id in starter {
        scene.remove_layer(id);
    }
    let layer = scene.add_layer("Object", LayerKind::Normal);
    for s in shapes {
        scene.add_shape_at(layer, 0, s);
    }
    Some(scene)
}

/// One object found in a picture: its label, tight box and matte (no tracing).
pub struct Detected {
    pub label: String,
    /// `[x0, y0, x1, y1]` in picture pixels, tight round the mask.
    pub bbox: [u32; 4],
    /// Full picture size (0 / 255).
    pub mask: GrayImage,
}

/// **Find the picture's objects**: the VLM's boxes, one SAM 2 mask each (retries and
/// coverage as in `mask_for_box`). Detection only; `extract_objects` traces them.
pub fn detect_objects(picture: &RgbaImage, opts: &ObjectOptions, notes: &mut Vec<String>) -> Vec<Detected> {
    detect_objects_with(picture, opts, notes, false).0
}

/// As [`detect_objects`], also handing back the SAM 2 embedding of `picture` so the caller can
/// prompt it again (S16: the sky and ground masks). With `always_encode` the picture is encoded
/// even when the VLM found nothing; the embedding is `None` when SAM 2 is missing or failed.
pub fn detect_objects_with(
    picture: &RgbaImage,
    opts: &ObjectOptions,
    notes: &mut Vec<String>,
    always_encode: bool,
) -> (Vec<Detected>, Option<buzz_ai::segment::Sam2Embeddings>) {
    let (w, h) = picture.dimensions();
    let mut out = Vec::new();
    // This detection's run record (if it makes one) is what `take_last_run` hands the caller.
    crate::split_cache::set_last_run(None);
    let found = candidates(picture, opts, notes);
    if found.is_empty() && !always_encode {
        return (out, None);
    }
    let emb = match buzz_ai::segment::sam2_encode(picture) {
        Ok(Some(e)) => e,
        Ok(None) => {
            notes.push("objects: the SAM 2 model is not installed; plates only".into());
            return (out, None);
        }
        Err(e) => {
            notes.push(format!("objects: SAM 2 failed ({e}); plates only"));
            return (out, None);
        }
    };
    let min_cover = if opts.small_objects { SMALL_MASK_MIN_OF_BOX } else { MASK_MIN_OF_BOX };
    // Per detection in `out`: its VLM box (before SAM) and whether it is a cut-line fragment.
    let mut vlm: Vec<([u32; 4], bool)> = Vec::new();
    for (label, bbox, fragment) in found {
        let boxf = [bbox[0] as f32, bbox[1] as f32, bbox[2] as f32, bbox[3] as f32];
        let want = if is_building(&label) { min_cover.max(BUILDING_MASK_OF_BOX) } else { min_cover };
        let (raw, cover) = match mask_for_box(&emb, boxf, (w, h), want) {
            Ok(Some(m)) => m,
            Ok(None) => {
                notes.push("objects: the SAM 2 decoder is not installed; plates only".into());
                break;
            }
            Err(e) => {
                notes.push(format!("objects: {label} skipped ({e})"));
                continue;
            }
        };
        if cover < min_cover {
            notes.push(format!("objects: {label} {bbox:?} skipped (mask covers {:.0}% of its box)", cover * 100.0));
            continue;
        }
        let area = raw.iter().filter(|&&v| v > 0).count() as f64 / (w as f64 * h as f64);
        if !(if opts.small_objects { SMALL_MASK_MIN } else { 0.002 }..=0.6).contains(&area) {
            notes.push(format!("objects: {label} skipped (mask covers {:.2}% of the picture)", area * 100.0));
            continue;
        }
        let Some(mask) = GrayImage::from_raw(w, h, raw) else { continue };
        // The tight box of the mask.
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
        for (x, y, p) in mask.enumerate_pixels() {
            if p[0] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        out.push(Detected { label, bbox: [x0, y0, x1, y1], mask });
        vlm.push((bbox, fragment));
    }
    if opts.small_objects {
        merge_fragments(&mut out, &mut vlm, notes);
        if opts.indoor {
            merge_halves(&mut out, notes);
        }
        drop_multi_building(&mut out, notes);
        merge_parts(&mut out, notes);
    }
    (out, Some(emb))
}

fn mask_area(m: &GrayImage) -> u64 {
    m.iter().filter(|&&v| v > 127).count() as u64
}

/// Is this label a whole building (house, shop...) rather than a part of one?
fn is_building(label: &str) -> bool {
    let l = label.to_lowercase();
    ["house", "shop", "building", "store", "hut", "home", "temple", "tower"].iter().any(|k| l.contains(k))
}

/// Split artwork: a **fragment** (a VLM box from a half pass that ends on the cut line) whose box
/// lies >= 60 % inside the VLM box of another detection of the same kind (both buildings, else
/// the same label) is the cut-off part of that one: it is merged into it (masks and boxes
/// united). The container with the biggest VLM box wins; repeats until nothing merges. `vlm` is
/// the VLM box and fragment flag of each detection in `out`, kept in step with it.
fn merge_fragments(out: &mut Vec<Detected>, vlm: &mut Vec<([u32; 4], bool)>, notes: &mut Vec<String>) {
    let area = |b: [u32; 4]| (b[2].saturating_sub(b[0]) as f64) * (b[3].saturating_sub(b[1]) as f64);
    'again: loop {
        for a in 0..out.len() {
            if !vlm[a].1 {
                continue;
            }
            let ba = vlm[a].0;
            let mut target: Option<(usize, f64)> = None;
            for b in 0..out.len() {
                let same_kind = (is_building(&out[a].label) && is_building(&out[b].label)) || out[a].label == out[b].label;
                if a == b || !same_kind {
                    continue;
                }
                let bb = vlm[b].0;
                let ix = ba[2].min(bb[2]).saturating_sub(ba[0].max(bb[0])) as f64;
                let iy = ba[3].min(bb[3]).saturating_sub(ba[1].max(bb[1])) as f64;
                if ix * iy >= 0.6 * area(ba).max(1.0) && target.is_none_or(|t| area(bb) > t.1) {
                    target = Some((b, area(bb)));
                }
            }
            let Some((b, _)) = target else { continue };
            let part = out.remove(a);
            vlm.remove(a);
            let b = if b > a { b - 1 } else { b };
            let big = &mut out[b];
            for (u, m) in big.mask.iter_mut().zip(part.mask.iter()) {
                if *m > 127 {
                    *u = 255;
                }
            }
            big.bbox = [big.bbox[0].min(part.bbox[0]), big.bbox[1].min(part.bbox[1]), big.bbox[2].max(part.bbox[2]), big.bbox[3].max(part.bbox[3])];
            notes.push(format!("objects: {} {:?} merged into {} (cut by a half pass)", part.label, part.bbox, big.label));
            continue 'again;
        }
        break;
    }
}

/// Indoors: one long thing (a counter, a desk, a shelf) found as two halves, one by each half
/// pass, is **joined**. Two objects of the same label are halves of one when they share a row
/// (each one's height overlaps the other's by >= 60 % of the shorter), their masks touch, and
/// they meet in the middle band of the picture where the half passes overlap (0.40..0.60 of the
/// width) — so two crates standing side by side elsewhere stay two. Repeats until nothing joins.
fn merge_halves(out: &mut Vec<Detected>, notes: &mut Vec<String>) {
    'again: loop {
        for a in 0..out.len() {
            for b in a + 1..out.len() {
                if out[a].label != out[b].label {
                    continue;
                }
                let (ba, bb) = (out[a].bbox, out[b].bbox);
                let (w, h) = out[a].mask.dimensions();
                let yo = ba[3].min(bb[3]) as f64 - ba[1].max(bb[1]) as f64;
                let short = (ba[3] - ba[1]).min(bb[3] - bb[1]).max(1) as f64;
                if yo < 0.6 * short {
                    continue;
                }
                // Where they meet: the stretch between the left one's right edge and the right one's left edge.
                let (left, right) = if ba[0] <= bb[0] { (ba, bb) } else { (bb, ba) };
                let (m0, m1) = (left[2].min(right[0]) as f64, left[2].max(right[0]) as f64);
                if m1 < 0.40 * w as f64 || m0 > 0.60 * w as f64 {
                    continue;
                }
                let near = buzz_ai::segment::dilate(out[b].mask.as_raw(), w, h, 3);
                if !out[a].mask.iter().zip(near.iter()).any(|(p, q)| *p > 127 && *q > 0) {
                    continue;
                }
                let part = out.remove(b);
                let whole = &mut out[a];
                for (u, m) in whole.mask.iter_mut().zip(part.mask.iter()) {
                    if *m > 127 {
                        *u = 255;
                    }
                }
                whole.bbox = [whole.bbox[0].min(part.bbox[0]), whole.bbox[1].min(part.bbox[1]), whole.bbox[2].max(part.bbox[2]), whole.bbox[3].max(part.bbox[3])];
                notes.push(format!("objects: {} {:?} joined to {} (two halves of one)", part.label, part.bbox, whole.label));
                continue 'again;
            }
        }
        break;
    }
}

/// Split artwork: one VLM box that held two buildings gives one mask over both. When two other
/// buildings (each >= 15 % of its area, >= 80 % inside it, overlapping each other <= 30 % of the
/// smaller) together cover >= 60 % of it, that mask is dropped: the two buildings stand instead.
fn drop_multi_building(out: &mut Vec<Detected>, notes: &mut Vec<String>) {
    'again: loop {
        let areas: Vec<u64> = out.iter().map(|d| mask_area(&d.mask)).collect();
        for b in 0..out.len() {
            if !is_building(&out[b].label) {
                continue;
            }
            let mine: Vec<usize> = (0..out.len())
                .filter(|&m| {
                    if m == b || !is_building(&out[m].label) || (areas[m] as f64) < 0.15 * areas[b] as f64 || areas[m] >= areas[b] {
                        return false;
                    }
                    let inside = out[m].mask.iter().zip(out[b].mask.iter()).filter(|(p, q)| **p > 127 && **q > 127).count() as f64;
                    inside >= 0.8 * areas[m].max(1) as f64
                })
                .collect();
            for (i, &m1) in mine.iter().enumerate() {
                for &m2 in &mine[i + 1..] {
                    let both = out[m1].mask.iter().zip(out[m2].mask.iter()).filter(|(p, q)| **p > 127 && **q > 127).count() as f64;
                    let either = out[m1].mask.iter().zip(out[m2].mask.iter()).filter(|(p, q)| **p > 127 || **q > 127).count() as f64;
                    if both <= 0.3 * areas[m1].min(areas[m2]) as f64 && either >= 0.6 * areas[b] as f64 {
                        let gone = out.remove(b);
                        notes.push(format!("objects: {} {:?} dropped (one box over two buildings)", gone.label, gone.bbox));
                        continue 'again;
                    }
                }
            }
        }
        break;
    }
}

/// Split artwork: a part of a building (its awning, its upper floor) is **merged into** the
/// building instead of standing as an object of its own. A part is an object whose mask lies
/// mostly (>= 50 %) inside another's, or that touches another, shares its column (x-overlap
/// >= 60 % of the narrower) and is stacked above or below it, with a mask <= 35 % of the
/// bigger's and its box >= 70 % inside the bigger's box. Two buildings never merge. The union keeps the bigger label.
/// Repeats until nothing merges.
fn merge_parts(out: &mut Vec<Detected>, notes: &mut Vec<String>) {
    'again: loop {
        let areas: Vec<u64> = out.iter().map(|d| mask_area(&d.mask)).collect();
        for a in 0..out.len() {
            for b in 0..out.len() {
                if a == b || areas[a] > areas[b] || (areas[a] == areas[b] && a > b) {
                    continue;
                }
                // `a` is the smaller one: does it belong to `b`? Two buildings never merge.
                if is_building(&out[a].label) && is_building(&out[b].label) {
                    continue;
                }
                let (ma, mb) = (&out[a].mask, &out[b].mask);
                let inside = ma.iter().zip(mb.iter()).filter(|(p, q)| **p > 127 && **q > 127).count() as f64;
                let mut belongs = inside >= 0.5 * areas[a].max(1) as f64;
                if !belongs {
                    let (ba, bb) = (out[a].bbox, out[b].bbox);
                    let xo = ba[2].min(bb[2]) as f64 - ba[0].max(bb[0]) as f64;
                    let narrow = (ba[2] - ba[0]).min(bb[2] - bb[0]).max(1) as f64;
                    let yo = ba[3].min(bb[3]) as f64 - ba[1].max(bb[1]) as f64;
                    let short = (ba[3] - ba[1]).min(bb[3] - bb[1]).max(1) as f64;
                    let box_in = {
                        let ix = ba[2].min(bb[2]).saturating_sub(ba[0].max(bb[0])) as f64;
                        let iy = ba[3].min(bb[3]).saturating_sub(ba[1].max(bb[1])) as f64;
                        ix * iy >= 0.7 * ((ba[2] - ba[0]) as f64 * (ba[3] - ba[1]) as f64).max(1.0)
                    };
                    if xo >= 0.6 * narrow && yo < 0.5 * short && areas[a] as f64 <= 0.35 * areas[b] as f64 && box_in {
                        let (w, h) = mb.dimensions();
                        let near = buzz_ai::segment::dilate(mb.as_raw(), w, h, 3);
                        belongs = ma.iter().zip(near.iter()).any(|(p, q)| *p > 127 && *q > 0);
                    }
                }
                if !belongs {
                    continue;
                }
                let part = out.remove(a);
                let b = if b > a { b - 1 } else { b };
                let big = &mut out[b];
                for (u, m) in big.mask.iter_mut().zip(part.mask.iter()) {
                    if *m > 127 {
                        *u = 255;
                    }
                }
                big.bbox = [big.bbox[0].min(part.bbox[0]), big.bbox[1].min(part.bbox[1]), big.bbox[2].max(part.bbox[2]), big.bbox[3].max(part.bbox[3])];
                notes.push(format!("objects: {} {:?} merged into {}", part.label, part.bbox, big.label));
                continue 'again;
            }
        }
        break;
    }
}

/// **Cut the background's objects out** and fill their holes in `plates`.
pub fn extract_objects(picture: &RgbaImage, plates: &Plates, opts: &ObjectOptions) -> ObjectCut {
    let mut cut = ObjectCut {
        objects: Vec::new(),
        filled_plates: Plates {
            sky: plates.sky.clone(),
            far: plates.far.clone(),
            mid: plates.mid.clone(),
            ground: plates.ground.clone(),
            horizon: plates.horizon,
        },
        notes: Vec::new(),
    };
    let (w, h) = picture.dimensions();
    if plates.far.dimensions() != (w, h) {
        cut.notes.push("objects: the plates are not the picture's size; plates only".into());
        return cut;
    }
    let mut notes = Vec::new();
    let detected = detect_objects(picture, opts, &mut notes);
    cut.notes.append(&mut notes);
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for Detected { label, bbox: tight, mask } in detected {
        let Some(symbol_scene) = trace_crop(picture, &mask, tight, opts) else {
            cut.notes.push(format!("objects: {label} skipped (nothing to trace)"));
            continue;
        };
        let depth_plate = plate_at_bottom(&cut.filled_plates, &mask);
        let n = counts.entry(label.clone()).and_modify(|c| *c += 1).or_insert(1);
        cut.objects.push(CutObject { label, n: *n, bbox: tight, depth_plate, mask, symbol_scene });
    }
    // The holes, after every object is cut so none is filled from another's
    // pixels: in EVERY plate with pixels under a mask (the sky plate may hold a
    // building the depth split did not place), the union of the masks at once.
    if !cut.objects.is_empty() {
        let mut union = GrayImage::new(w, h);
        for o in &cut.objects {
            for (u, m) in union.iter_mut().zip(o.mask.iter()) {
                if *m > 127 {
                    *u = 255;
                }
            }
        }
        for plate in [&mut cut.filled_plates.sky, &mut cut.filled_plates.far, &mut cut.filled_plates.mid, &mut cut.filled_plates.ground] {
            fill_hole(plate, &union, 2);
        }
    }
    if let Some(dir) = debug_dir() {
        for (i, o) in cut.objects.iter().enumerate() {
            let name = format!("{i}_{}{}", o.label.replace(' ', "_"), o.n);
            let _ = o.mask.save(dir.join(format!("{name}_mask.png")));
            let mut crop = picture.clone();
            for (x, y, p) in o.mask.enumerate_pixels() {
                if p[0] <= 127 {
                    crop.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                }
            }
            let [x0, y0, x1, y1] = o.bbox;
            let _ = image::imageops::crop_imm(&crop, x0, y0, x1 - x0, y1 - y0).to_image().save(dir.join(format!("{name}_crop.png")));
        }
    }
    cut
}

/// **Put the cut objects into the traced background**: each a library symbol named
/// `<location> <Label> <n>`, on its own layer `BG <Label> <n>` directly above its
/// plate, at its original position.
pub fn add_to_scene(cut: &ObjectCut, scene: &mut Scene, location: &str, size: (u32, u32)) -> Vec<Placed> {
    let (w, h) = (size.0 as f64, size.1 as f64);
    let s = (STAGE.0 / w).max(STAGE.1 / h);
    let place = Affine::translate(((STAGE.0 - w * s) / 2.0, (STAGE.1 - h * s) / 2.0)) * Affine::scale(s);
    let mut placed = Vec::new();
    for o in &cut.objects {
        let Some(plate) = scene.layers().iter().find(|l| l.name == o.depth_plate.name()).map(|l| (l.id, l.depth)) else { continue };
        let Some(source) = o.symbol_scene.layers().iter().next() else { continue };
        let shapes: Vec<ShapeData> = source
            .objects_at(0)
            .iter()
            .filter_map(|ob| if let ObjectKind::Shape(sd) = &ob.kind { Some(sd.clone()) } else { None })
            .collect();
        if shapes.is_empty() {
            continue;
        }
        let name = format!("{location} {} {}", o.label, o.n);
        let symbol = scene.add_symbol(name.clone(), buzz_scene::SymbolKind::Graphic, Some("Background objects"));
        let Some(inner) = scene.library().get(symbol).and_then(|s| s.layers.iter().next()).map(|l| l.id) else { continue };
        let objects: Vec<Arc<Object>> = shapes.into_iter().map(|sd| Arc::new(Object::shape(scene.next_object_id(), sd))).collect();
        scene.library_mut().update(symbol, |sym| {
            sym.layers.update(inner, |l| {
                l.frames.set_objects(0, objects);
            });
        });
        let layer = scene.add_layer(format!("BG {} {}", o.label, o.n), LayerKind::Normal);
        if let Some(index) = scene.layers().index_of(plate.0) {
            scene.reorder_layer(layer, index);
        }
        scene.update_stage_layer(layer, |l| {
            l.depth = plate.1;
            l.casts_shadow = false;
        });
        let at = place * Affine::translate((o.bbox[0] as f64, o.bbox[1] as f64));
        scene.add_instance_at(layer, 0, symbol, at);
        placed.push(Placed { label: o.label.clone(), n: o.n, plate: o.depth_plate.name(), bbox: o.bbox, symbol: name });
    }
    placed
}

/// The run record's `result.json` for the picture route: the plates and the object layers
/// `add_to_scene` made, back to front (an object sits directly above its plate), and the notes.
pub fn result_json(placed: &[Placed], notes: &[String]) -> serde_json::Value {
    let mut layers: Vec<serde_json::Value> = vec![serde_json::json!({ "name": "Sky" })];
    for plate in [PlateId::Far, PlateId::Mid, PlateId::Ground] {
        layers.push(serde_json::json!({ "name": plate.name() }));
        for p in placed.iter().filter(|p| p.plate == plate.name()) {
            layers.push(serde_json::json!({ "name": format!("BG {} {}", p.label, p.n), "bbox": p.bbox }));
        }
    }
    serde_json::json!({
        "route": "picture",
        "layers_back_to_front": layers.iter().map(|l| l["name"].clone()).collect::<Vec<_>>(),
        "layers": layers,
        "objects": placed.iter().map(|p| serde_json::json!({ "label": p.label, "n": p.n, "plate": p.plate, "bbox": p.bbox })).collect::<Vec<_>>(),
        "notes": notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba};

    /// Indoor labels keep the VLM's own words, cut before a joining word; street names stay outdoors.
    #[test]
    fn indoor_labels_are_the_objects_own_words() {
        assert_eq!(normalise_indoor_label("Door with iron bars"), "Door");
        assert_eq!(normalise_indoor_label("hanging-lamp"), "Hanging Lamp");
        assert_eq!(normalise_indoor_label("Tea kettle on counter"), "Tea Kettle");
        assert_eq!(normalise_indoor_label("with nothing"), "Object");
        assert_eq!(normalise_label("Street lamp"), "Pole");
        assert_eq!(normalise_label("Counter with shelves"), "Counter");
    }

    fn plates(n: u32) -> Plates {
        let blank = || RgbaImage::from_pixel(n, n, Rgba([0, 0, 0, 0]));
        let mut p = Plates { sky: blank(), far: blank(), mid: blank(), ground: blank(), horizon: n / 3 };
        for y in 0..n {
            for x in 0..n {
                if y < 12 {
                    p.mid.put_pixel(x, y, Rgba([10, 120, 10, 255]));
                } else {
                    p.ground.put_pixel(x, y, Rgba([120, 90, 40, 255]));
                }
            }
        }
        p
    }

    fn mask(n: u32, rect: [u32; 4]) -> GrayImage {
        let mut m = GrayImage::new(n, n);
        for y in rect[1]..rect[3] {
            for x in rect[0]..rect[2] {
                m.put_pixel(x, y, Luma([255]));
            }
        }
        m
    }

    #[test]
    fn object_goes_to_the_plate_its_bottom_edge_sits_in() {
        let p = plates(20);
        assert_eq!(plate_at_bottom(&p, &mask(20, [4, 4, 10, 11])), PlateId::Mid);
        assert_eq!(plate_at_bottom(&p, &mask(20, [4, 9, 10, 17])), PlateId::Ground);
    }

    #[test]
    fn hole_fill_leaves_no_transparent_pixel_in_the_region() {
        let mut plate = RgbaImage::from_pixel(16, 16, Rgba([30, 60, 90, 255]));
        let m = mask(16, [5, 5, 10, 10]);
        for y in 5..10 {
            for x in 5..10 {
                plate.put_pixel(x, y, Rgba([200, 0, 0, 255]));
            }
        }
        fill_hole(&mut plate, &m, 1);
        assert!(plate.pixels().all(|p| p[3] > 128));
        assert_eq!(plate.get_pixel(7, 7)[0], 30);
    }
}
