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
    /// The tracer options of the plates.
    pub trace: BackdropOptions,
}

impl Default for ObjectOptions {
    fn default() -> Self {
        Self { max_objects: 8, trace: BackdropOptions::default() }
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
    l.split_whitespace()
        .take(2)
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    let raw: &mut [u8] = &mut *plate;
    for i in 0..n {
        if hole[i] {
            raw[i * 4 + 3] = 0;
        }
    }
    buzz_ai::outpaint::nearest_visible_fill(raw, w, h, &hole, fallback);
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

/// Ask the local VLM for the picture's objects (people and animals dropped), best first.
fn candidates(picture: &RgbaImage, opts: &ObjectOptions, notes: &mut Vec<String>) -> Vec<(String, [u32; 4])> {
    let (w, h) = picture.dimensions();
    let Some(png) = png_bytes(picture) else {
        notes.push("objects: could not encode the picture for the VLM".into());
        return Vec::new();
    };
    let claude = buzz_ai::ai_config::use_claude();
    let backend = if claude { buzz_ai::llm::Backend::default_claude_api() } else { buzz_ai::llm::Backend::default_openai() };
    let model = if claude { Some(buzz_ai::ai_config::claude_model()) } else { crate::produce::lmstudio_model(&backend).map(|m| m.0) };
    let Some(model) = model.filter(|m| !m.is_empty()) else {
        notes.push("objects: no VLM available; plates only".into());
        return Vec::new();
    };
    let layout = match buzz_ai::scene_builder::decompose_scene_image(&backend, &model, &png) {
        Ok(l) => l,
        Err(e) => {
            notes.push(format!("objects: the VLM failed ({e}); plates only"));
            return Vec::new();
        }
    };
    let mut scored: Vec<(f64, String, [u32; 4])> = Vec::new();
    for o in &layout.objects {
        if is_person_or_animal(&o.label, &o.kind) {
            continue;
        }
        let b = o.bbox.map(|v| v.clamp(0.0, 1.0));
        let (x0, y0, x1, y1) = ((b[0] * w as f64) as u32, (b[1] * h as f64) as u32, (b[2] * w as f64) as u32, (b[3] * h as f64) as u32);
        if x1 <= x0 + 4 || y1 <= y0 + 4 {
            continue;
        }
        let area = ((x1 - x0) as f64 * (y1 - y0) as f64) / (w as f64 * h as f64);
        if !(0.004..=0.7).contains(&area) {
            notes.push(format!("objects: {} skipped (box covers {:.1}% of the picture)", o.label, area * 100.0));
            continue;
        }
        let cx = ((x0 + x1) as f64 / 2.0 / w as f64 - 0.5).abs() * 2.0;
        scored.push((area * (1.2 - 0.8 * cx), normalise_label(&o.label), [x0, y0, x1, y1]));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut kept: Vec<(String, [u32; 4])> = Vec::new();
    for (_, label, bbox) in scored {
        if kept.len() >= opts.max_objects {
            break;
        }
        if kept.iter().any(|(_, k)| iou(*k, bbox) > 0.6) {
            continue;
        }
        kept.push((label, bbox));
    }
    kept
}

/// The mask kept clean: closed, small holes filled, the largest piece only.
fn clean(mask: &GrayImage) -> Vec<u8> {
    let (w, h) = mask.dimensions();
    let bin: Vec<u8> = mask.as_raw().iter().map(|&v| if v > 127 { 255 } else { 0 }).collect();
    let closed = buzz_ai::segment::morph_close(&bin, w, h, 2);
    let filled = buzz_ai::segment::fill_small_holes(&closed, w, h, 0.02);
    buzz_ai::segment::largest_component(&filled, w, h)
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
    let found = candidates(picture, opts, &mut notes);
    cut.notes.append(&mut notes);
    if found.is_empty() {
        return cut;
    }
    let emb = match buzz_ai::segment::sam2_encode(picture) {
        Ok(Some(e)) => e,
        Ok(None) => {
            cut.notes.push("objects: the SAM 2 model is not installed; plates only".into());
            return cut;
        }
        Err(e) => {
            cut.notes.push(format!("objects: SAM 2 failed ({e}); plates only"));
            return cut;
        }
    };
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for (label, bbox) in found {
        let boxf = [bbox[0] as f32, bbox[1] as f32, bbox[2] as f32, bbox[3] as f32];
        let mask = match buzz_ai::segment::sam2_mask(&emb, &[], Some(boxf)) {
            Ok(Some((m, _))) if m.dimensions() == (w, h) => m,
            Ok(Some(_)) => {
                cut.notes.push(format!("objects: {label} skipped (mask has the wrong size)"));
                continue;
            }
            Ok(None) => {
                cut.notes.push("objects: the SAM 2 decoder is not installed; plates only".into());
                break;
            }
            Err(e) => {
                cut.notes.push(format!("objects: {label} skipped (SAM 2: {e})"));
                continue;
            }
        };
        let raw = clean(&mask);
        let area = raw.iter().filter(|&&v| v > 0).count() as f64 / (w as f64 * h as f64);
        if !(0.002..=0.6).contains(&area) {
            cut.notes.push(format!("objects: {label} skipped (mask covers {:.2}% of the picture)", area * 100.0));
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
        let tight = [x0, y0, x1, y1];
        let Some(symbol_scene) = trace_crop(picture, &mask, tight, opts) else {
            cut.notes.push(format!("objects: {label} skipped (nothing to trace)"));
            continue;
        };
        let depth_plate = plate_at_bottom(&cut.filled_plates, &mask);
        let n = counts.entry(label.clone()).and_modify(|c| *c += 1).or_insert(1);
        cut.objects.push(CutObject { label, n: *n, bbox: tight, depth_plate, mask, symbol_scene });
    }
    // The holes, after every object is cut so none is filled from another's pixels.
    for o in &cut.objects {
        let plate = match o.depth_plate {
            PlateId::Far => &mut cut.filled_plates.far,
            PlateId::Mid => &mut cut.filled_plates.mid,
            PlateId::Ground => &mut cut.filled_plates.ground,
        };
        fill_hole(plate, &o.mask, 2);
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba};

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
