//! **The asset index builder** (AS1) and the arrange helpers the Assets
//! panel and the agent share (AS2). The pure half — records, heuristics,
//! clustering, the plan, apply and undo — is `buzz_ai::asset_index`; this
//! half owns what needs a GPU and the models: thumbnails, SigLIP, the VLM.
//!
//! Built as a Tasks-panel task (`App::start_arrange`), and headlessly by
//! `examples/asset_index.rs`. Incremental: an asset whose file's mtime is
//! unchanged keeps its records; a run with the VLM also upgrades records that
//! were only `guessed` from names.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use buzz_ai::asset_index::{
    self as ai_index, AssetIndex, IndexRecord, KindItem, INDEX_DIR, VLM_BATCH,
};
use buzz_ai::llm::{Backend, LlmError};
use buzz_doc::{Asset, AssetLibrary};
use buzz_geom::{Point, Rect};
use buzz_scene::{LayerKind, Object, Scene, SymbolId};

/// Thumbnail edge, pixels.
pub const THUMB: u32 = 256;
/// Most symbol records one asset contributes.
pub const MAX_SYMBOLS_PER_ASSET: usize = 12;
/// White, for flattening transparent thumbnails before SigLIP and the VLM.
pub const FLAT_BG: [u8; 3] = [255, 255, 255];

/// What a build should do.
#[derive(Clone, Default)]
pub struct BuildOptions {
    /// Work on at most this many assets that need work (the rest keep what
    /// they have, or stay unindexed until the next run).
    pub limit: Option<usize>,
    /// The VLM to classify with; `None` = name heuristics only.
    pub vlm: Option<(Backend, String)>,
}

/// What a build did.
#[derive(Debug, Clone, Default)]
pub struct BuildReport {
    pub assets_total: usize,
    pub assets_rendered: usize,
    pub assets_kept: usize,
    pub assets_failed: Vec<String>,
    pub records: usize,
    pub symbol_records: usize,
    pub guessed: usize,
    /// Seconds per VLM batch, in order.
    pub vlm_batches: Vec<f64>,
    pub vlm_records: usize,
    pub vlm_error: Option<String>,
    pub embed_error: Option<String>,
    pub render_seconds: f64,
    pub embed_seconds: f64,
}

/// Seconds-since-epoch mtime of a file.
pub fn mtime_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------------------------------------ distinct symbols

/// The asset's "main timeline": the stage, or — when the stage holds one
/// instance and nothing else, as every imported Animate asset does — that
/// symbol's own timeline. Returns the symbol whose timeline it is, if any.
fn main_wrapper(scene: &Scene) -> Option<SymbolId> {
    let mut found = None;
    for layer in scene.stage_layers().iter() {
        for o in layer.objects_at(0) {
            if !o.visible {
                continue;
            }
            match o.instance() {
                Some(i) if found.is_none() => found = Some(i.symbol),
                _ => return None,
            }
        }
    }
    found
}

/// **The distinct visual symbols inside an asset** (AS1): library symbols
/// placed on the asset's main timeline as separate instances (deduplicated,
/// the largest copy kept), big enough to be a thing of their own — at least
/// 12 % of the timeline's height and 3 % of its width. A timeline laid out
/// like a puppet (four or more layers, most holding exactly one instance) is
/// a rig, whose parts are not things of their own, and yields none;
/// Illustrator's `<Path>`/`<Group>` symbols are skipped. Listed only when
/// there are two or more, or one that covers under half the asset.
pub fn distinct_symbols(scene: &Scene) -> Vec<(SymbolId, String, Rect)> {
    let layers: Vec<&buzz_scene::Layer> = match main_wrapper(scene) {
        Some(sym) => match scene.library().get(sym) {
            Some(s) => s.layers.iter().map(|l| l.as_ref()).collect(),
            None => return Vec::new(),
        },
        None => scene.stage_layers().iter().map(|l| l.as_ref()).collect(),
    };
    let painted: Vec<&&buzz_scene::Layer> = layers.iter().filter(|l| !l.objects_at(0).is_empty()).collect();
    let single = painted
        .iter()
        .filter(|l| {
            let objs = l.objects_at(0);
            objs.len() == 1 && objs[0].instance().is_some()
        })
        .count();
    if painted.len() >= 4 && single * 10 >= painted.len() * 6 {
        return Vec::new();
    }
    let mut all: Option<Rect> = None;
    let mut inst: HashMap<SymbolId, Rect> = HashMap::new();
    for layer in &painted {
        for o in layer.objects_at(0) {
            let b = scene.resolved_bounds(o);
            if !(b.width().is_finite() && b.height().is_finite()) {
                continue;
            }
            all = Some(all.map_or(b, |a| a.union(b)));
            if let Some(i) = o.instance() {
                let keep = inst.get(&i.symbol).is_none_or(|old| old.area() < b.area());
                if keep {
                    inst.insert(i.symbol, b);
                }
            }
        }
    }
    let Some(all) = all else { return Vec::new() };
    let mut out: Vec<(SymbolId, String, Rect)> = inst
        .into_iter()
        .filter_map(|(id, b)| {
            let name = scene.library().get(id)?.name.clone();
            let junk = name.contains("&#060") || name.starts_with('<');
            let big = b.height() >= 0.12 * all.height()
                && b.width() >= 0.03 * all.width()
                && b.width() >= 24.0
                && b.height() >= 24.0;
            (!junk && big).then_some((id, name, b))
        })
        .collect();
    out.sort_by(|a, b| b.2.area().total_cmp(&a.2.area()).then(a.1.cmp(&b.1)));
    out.truncate(MAX_SYMBOLS_PER_ASSET);
    match out.len() {
        0 => out,
        1 if out[0].2.area() < 0.5 * all.area() => out,
        1 => Vec::new(),
        _ => out,
    }
}

// ------------------------------------------------------------ parts (AS4)

/// Most part records one asset contributes.
pub const MAX_PARTS_PER_ASSET: usize = 12;
/// Two pieces of artwork closer than this fraction of the asset's longer side
/// belong to one part.
pub const PART_GAP: f64 = 0.015;

/// The layers of the asset's main timeline (see [`main_wrapper`]).
fn main_layers(scene: &Scene) -> Option<Vec<&buzz_scene::Layer>> {
    Some(match main_wrapper(scene) {
        Some(sym) => scene.library().get(sym)?.layers.iter().map(|l| l.as_ref()).collect(),
        None => scene.stage_layers().iter().map(|l| l.as_ref()).collect(),
    })
}

/// The subpaths of a shape (split at each move-to).
fn subpaths(path: &kurbo::BezPath) -> Vec<kurbo::BezPath> {
    let mut out: Vec<kurbo::BezPath> = Vec::new();
    for el in path.elements() {
        if matches!(el, kurbo::PathEl::MoveTo(_)) || out.is_empty() {
            out.push(kurbo::BezPath::new());
        }
        out.last_mut().expect("pushed").push(*el);
    }
    out
}

/// The boxes an object's artwork is clustered by: one per subpath of a shape
/// (an imported shape is often one colour across several drawings: every
/// bush's dark green in one path), one for anything else. In the space the
/// object sits in.
fn piece_boxes(scene: &Scene, o: &Object) -> Vec<Rect> {
    match &o.kind {
        buzz_scene::ObjectKind::Shape(s) => subpaths(&s.path)
            .iter()
            .map(|p| buzz_scene::object::transform_rect(o.transform, kurbo::Shape::bounding_box(p)))
            .collect(),
        _ => vec![scene.resolved_bounds(o)],
    }
}

fn usable(b: &Rect) -> bool {
    b.width().is_finite() && b.height().is_finite() && (b.width() > 0.0 || b.height() > 0.0)
}

/// **The parts of an asset made of loose shapes** (AS4): several drawings
/// side by side in raw shapes have no symbol to tell them apart, and one
/// shape is often one colour across all of them. Only drawings that do not
/// touch come apart: `BushNew1`, a hedge whose fills run its whole length,
/// is one connected group and has no parts. The
/// artwork on the main timeline's frame 0 (each subpath of a shape, each
/// other object whole) is grouped by its boxes ([`ai_index::cluster_boxes`],
/// gap [`PART_GAP`] of the longer side); groups whose boxes still overlap
/// are merged, so a part is cut
/// out by its box alone. Each group big enough to be a thing of its own (>= 15 % of the
/// height, >= 1 % of the area, >= 24 units each way) is a part. Listed only
/// when there are two or more, for an asset with no distinct symbols that is
/// not a rig. Returns `(part number, box)` left to right, in the main
/// timeline's space.
pub fn distinct_parts(scene: &Scene) -> Vec<(u32, Rect)> {
    if !distinct_symbols(scene).is_empty() {
        return Vec::new();
    }
    let Some(layers) = main_layers(scene) else { return Vec::new() };
    let mut found: Vec<Rect> = Vec::new();
    for layer in &layers {
        if !layer.visible {
            continue;
        }
        for o in layer.objects_at(0) {
            if !o.visible {
                continue;
            }
            if matches!(o.kind, buzz_scene::ObjectKind::Armature(_)) {
                return Vec::new();
            }
            found.extend(piece_boxes(scene, o).into_iter().filter(usable));
        }
    }
    if found.len() < 2 {
        return Vec::new();
    }
    let all = found.iter().copied().reduce(|a, b| a.union(b)).expect("two or more");
    let boxes: Vec<[f64; 4]> = found.iter().map(|b| [b.x0, b.y0, b.x1, b.y1]).collect();
    let gap = PART_GAP * all.width().max(all.height());
    let labels = ai_index::cluster_boxes(&boxes, gap);
    let n = labels.iter().copied().max().map_or(0, |m| m + 1);
    let mut groups: Vec<Option<Rect>> = vec![None; n];
    for (b, &l) in boxes.iter().zip(&labels) {
        let r = Rect::new(b[0], b[1], b[2], b[3]);
        groups[l] = Some(groups[l].map_or(r, |g| g.union(r)));
    }
    // Groups whose boxes overlap (a long stroke curling round another) are
    // one thing too: merged until no two parts' boxes come within the gap,
    // so every part can be cut out by its box alone.
    let mut groups: Vec<Rect> = groups.into_iter().flatten().collect();
    loop {
        let boxes: Vec<[f64; 4]> = groups.iter().map(|g| [g.x0, g.y0, g.x1, g.y1]).collect();
        let labels = ai_index::cluster_boxes(&boxes, gap);
        let n = labels.iter().copied().max().map_or(0, |m| m + 1);
        if n == groups.len() {
            break;
        }
        let mut merged: Vec<Option<Rect>> = vec![None; n];
        for (g, &l) in groups.iter().zip(&labels) {
            merged[l] = Some(merged[l].map_or(*g, |m| m.union(*g)));
        }
        groups = merged.into_iter().flatten().collect();
    }
    let parts: Vec<(u32, Rect)> = groups
        .into_iter()
        .filter(|g| {
            g.height() >= 0.15 * all.height()
                && g.area() >= 0.01 * all.area()
                && g.width() >= 24.0
                && g.height() >= 24.0
        })
        .take(MAX_PARTS_PER_ASSET)
        .enumerate()
        .map(|(i, g)| (i as u32, g))
        .collect();
    if parts.len() >= 2 { parts } else { Vec::new() }
}

/// Every symbol `object` refers to, however deeply, into `out`.
fn symbols_of(object: &Object, out: &mut Vec<SymbolId>) {
    match &object.kind {
        buzz_scene::ObjectKind::Instance(i) => out.push(i.symbol),
        buzz_scene::ObjectKind::Group(children) => {
            for c in children {
                symbols_of(c, out);
            }
        }
        buzz_scene::ObjectKind::Armature(rig) => {
            for part in &rig.parts {
                symbols_of(&part.artwork, out);
            }
        }
        buzz_scene::ObjectKind::Shape(_) | buzz_scene::ObjectKind::Warp(_) => {}
    }
}

/// Drop every library symbol nothing on the stage needs (directly, nested in
/// another symbol on any frame, or in the set of one that is).
fn prune_library(scene: &mut Scene) {
    let mut keep: HashSet<SymbolId> = HashSet::new();
    let mut todo: Vec<SymbolId> = Vec::new();
    for layer in scene.stage_layers().iter() {
        for k in layer.frames.keyframes() {
            for o in k.objects.iter() {
                symbols_of(o, &mut todo);
            }
        }
    }
    while let Some(id) = todo.pop() {
        if !keep.insert(id) {
            continue;
        }
        let Some(sym) = scene.library().get(id) else { continue };
        todo.extend(sym.set.iter().copied());
        for layer in sym.layers.iter() {
            for k in layer.frames.keyframes() {
                for o in k.objects.iter() {
                    symbols_of(o, &mut todo);
                }
            }
        }
    }
    let drop: Vec<SymbolId> = scene.library().iter().map(|s| s.id).filter(|id| !keep.contains(id)).collect();
    for id in drop {
        scene.library_mut().remove(id);
    }
}

/// A copy of `scene` whose stage is one layer holding `objects` (fresh ids),
/// with only the symbols they need.
fn scene_holding(scene: &Scene, name: &str, objects: Vec<Object>) -> Scene {
    let mut mini = scene.clone();
    mini.edit_document();
    for id in mini.stage_layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        mini.remove_layer(id);
    }
    let layer = mini.add_stage_layer(name, LayerKind::Normal);
    for mut o in objects {
        renumber(&mut o, &mut mini);
        mini.add_object(layer, o);
    }
    prune_library(&mut mini);
    mini
}

fn renumber(o: &mut Object, scene: &mut Scene) {
    o.id = scene.next_object_id();
    if let buzz_scene::ObjectKind::Group(children) = &mut o.kind {
        for c in children.iter_mut() {
            renumber(std::sync::Arc::make_mut(c), scene);
        }
    }
}

/// **One symbol of an asset, alone** (AS4): a scene whose stage holds one
/// instance of `symbol` and whose library holds only what it needs. Placing
/// it brings that symbol, not the whole asset.
pub fn symbol_scene(scene: &Scene, symbol: SymbolId, name: &str) -> Option<Scene> {
    scene.library().get(symbol)?;
    let probe = scene.clone().next_object_id();
    Some(scene_holding(scene, name, vec![Object::instance_of(probe, symbol)]))
}

/// **One part of an asset, alone** (AS4): the artwork of the main timeline's
/// frame 0 that lies inside `bounds` (a part's box from [`distinct_parts`]):
/// whole objects, and of a shape only the subpaths inside, on one layer,
/// back to front as it was drawn, with only the symbols it needs.
pub fn part_scene(scene: &Scene, bounds: Rect, name: &str) -> Option<Scene> {
    let layers = main_layers(scene)?;
    let margin = 1e-3 * bounds.width().max(bounds.height()) + 0.5;
    let grown = bounds.inflate(margin, margin);
    let inside = |b: &Rect| usable(b) && grown.contains(b.origin()) && grown.contains(Point::new(b.x1, b.y1));
    let mut objects: Vec<Object> = Vec::new();
    // The stack lists the front layer first; draw order is back first.
    for layer in layers.iter().rev() {
        if !layer.visible {
            continue;
        }
        for o in layer.objects_at(0) {
            if !o.visible {
                continue;
            }
            match &o.kind {
                buzz_scene::ObjectKind::Shape(s) => {
                    let mut path = kurbo::BezPath::new();
                    for sub in subpaths(&s.path) {
                        let b = buzz_scene::object::transform_rect(o.transform, kurbo::Shape::bounding_box(&sub));
                        if inside(&b) {
                            for el in sub.elements() {
                                path.push(*el);
                            }
                        }
                    }
                    if !path.elements().is_empty() {
                        let mut copy = (**o).clone();
                        if let buzz_scene::ObjectKind::Shape(shape) = &mut copy.kind {
                            shape.path = path;
                        }
                        objects.push(copy);
                    }
                }
                _ => {
                    if inside(&scene.resolved_bounds(o)) {
                        objects.push((**o).clone());
                    }
                }
            }
        }
    }
    (!objects.is_empty()).then(|| scene_holding(scene, name, objects))
}

// ------------------------------------------------------------ rendering

/// `scene` with its camera off, no symbol open, and a `THUMB`² stage.
fn thumb_scene(scene: &Scene) -> Scene {
    let mut mini = scene.clone();
    mini.edit_document();
    mini.camera_mut().enabled = false;
    mini
}

/// A square region around `b`, a little larger, for the exporter.
fn square_around(b: Rect) -> Rect {
    let side = b.width().max(b.height()).max(1.0) * 1.08;
    let c = b.center();
    Rect::new(c.x - side / 2.0, c.y - side / 2.0, c.x + side / 2.0, c.y + side / 2.0)
}

fn render_once(exporter: &mut buzz_export::Exporter, scene: &Scene, region: Rect, frame: u32) -> Option<image::RgbaImage> {
    let mut settings = buzz_export::ExportSettings::for_stage(scene);
    settings.width = THUMB;
    settings.height = THUMB;
    settings.transparent = true;
    settings.region = Some(region);
    let frame = exporter.render(scene, frame, &settings).ok()?;
    image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
}

/// Rendered, then — when what was drawn fills well under the picture (the
/// computed bounds took in hidden or empty artwork) — rendered again framed
/// on what was actually drawn.
fn render_region(exporter: &mut buzz_export::Exporter, scene: &Scene, region: Rect) -> Option<image::RgbaImage> {
    render_region_at(exporter, scene, region, 0)
}

fn render_region_at(
    exporter: &mut buzz_export::Exporter,
    scene: &Scene,
    region: Rect,
    frame: u32,
) -> Option<image::RgbaImage> {
    let img = render_once(exporter, scene, region, frame)?;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return Some(img);
    }
    let side = (x1 - x0).max(y1 - y0);
    if side * 10 >= THUMB * 7 {
        return Some(img);
    }
    let scale = region.width() / f64::from(THUMB);
    let drawn = Rect::new(
        region.x0 + f64::from(x0) * scale,
        region.y0 + f64::from(y0) * scale,
        region.x0 + f64::from(x1) * scale,
        region.y0 + f64::from(y1) * scale,
    );
    render_once(exporter, scene, square_around(drawn), frame).or(Some(img))
}

/// Frames the asset animates over: its own timeline or its longest symbol's
/// (an imported clip is one stage frame holding a looping symbol), at most 240.
fn animation_length(scene: &Scene) -> u32 {
    let longest = scene.library().iter().map(|s| s.length()).max().unwrap_or(1);
    scene.frame_count().max(longest).min(240)
}

/// **The whole asset, transparent, framed on its artwork**, at frame 0 — or,
/// for an animation whose frame 0 is empty (a clip that fades or grows in),
/// at the first of up to 24 frames spread over its length that has ink.
/// Returns the thumbnail, the bounds and the frame it was taken at.
pub fn render_asset(exporter: &mut buzz_export::Exporter, scene: &Scene) -> Option<(image::RgbaImage, Rect, u32)> {
    let mini = thumb_scene(scene);
    let bounds = crate::thumbnails::scene_bounds(&mini)?;
    if !(bounds.width().is_finite() && bounds.height().is_finite()) || bounds.area() <= 0.0 {
        return None;
    }
    let region = square_around(bounds);
    let img = render_region(exporter, &mini, region)?;
    if !ai_index::thumb_is_empty(&img) {
        return Some((img, bounds, 0));
    }
    let length = animation_length(&mini);
    if length <= 1 {
        return Some((img, bounds, 0));
    }
    // Every stage layer runs the whole length, so a looping symbol plays.
    let mut long = mini.clone();
    let last = length - 1;
    for id in long.layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        long.update_layer(id, |l| {
            if l.frames.length() <= last {
                l.frames.insert_frame(last);
            }
        });
    }
    let step = (length / 24).max(1) as usize;
    for f in (step as u32..length).step_by(step) {
        if let Some(i) = render_region_at(exporter, &long, region, f)
            && !ai_index::thumb_is_empty(&i)
        {
            return Some((i, bounds, f));
        }
    }
    Some((img, bounds, 0))
}

/// One symbol of the asset alone, frame 0, transparent.
pub fn render_symbol(exporter: &mut buzz_export::Exporter, scene: &Scene, symbol: SymbolId) -> Option<image::RgbaImage> {
    let mut mini = thumb_scene(scene);
    for id in mini.layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        mini.remove_layer(id);
    }
    let layer = mini.add_layer("Thumb", LayerKind::Normal);
    let probe = mini.next_object_id();
    let inst = Object::instance_of(probe, symbol);
    let bounds = mini.resolved_bounds(&inst);
    if !(bounds.width().is_finite() && bounds.height().is_finite()) || bounds.area() <= 0.0 {
        return None;
    }
    mini.add_object(layer, inst);
    render_region(exporter, &mini, square_around(bounds))
}

/// PNG bytes of `img` laid on white (what the VLM sees).
pub fn flat_png(img: &image::RgbaImage) -> Vec<u8> {
    let flat = ai_index::flatten_on(img, FLAT_BG);
    let mut out = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(flat)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

/// A thumbnail file name for a record id (`/` and other path characters
/// replaced), relative to the library root.
fn thumb_rel(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    format!("{INDEX_DIR}/thumbs/{safe}.png")
}

/// Load a record's thumbnail (transparent RGBA).
pub fn load_thumb(root: &Path, record_thumb: &str) -> Option<image::RgbaImage> {
    if record_thumb.is_empty() {
        return None;
    }
    image::open(root.join(record_thumb)).ok().map(|d| d.to_rgba8())
}

// ------------------------------------------------------------ the build

fn guessed_record(
    id: String,
    path: &str,
    symbol: Option<u64>,
    name: &str,
    folder: &str,
    bounds: Rect,
    mtime: u64,
    fallback: Option<&ai_index::Guess>,
) -> IndexRecord {
    let mut g = ai_index::guess_from_name(name);
    // A symbol whose own name says nothing takes its asset's guess.
    if g.kind == ai_index::AssetKind::Other
        && let Some(f) = fallback
    {
        g = f.clone();
    }
    IndexRecord {
        thumb: thumb_rel(&id),
        id,
        path: path.to_string(),
        symbol,
        name: name.to_string(),
        folder: folder.to_string(),
        embedding: Vec::new(),
        category: g.category,
        kind: g.kind,
        view: g.view,
        tags: g.tags,
        label: None,
        bounds: [bounds.x0, bounds.y0, bounds.x1, bounds.y1],
        aspect: if bounds.height() > 0.0 { bounds.width() / bounds.height() } else { 1.0 },
        mtime,
        guessed: true,
        taxonomy: ai_index::TAXONOMY_VERSION,
        blank: None,
        part: None,
        thumb_frame: None,
        fill: None,
        tone: None,
    }
}

/// The blank and fill measures of a fresh thumbnail.
fn measure(rec: &mut IndexRecord, img: &image::RgbaImage) {
    rec.blank = Some(ai_index::thumb_is_blank(img));
    rec.fill = Some(ai_index::thumb_fill(img));
    rec.tone = Some(ai_index::thumb_tone(img));
}

/// Render, embed and (with a VLM) classify one asset's records. The old
/// records of this asset are replaced.
fn index_one(
    root: &Path,
    library: &AssetLibrary,
    asset: &Asset,
    exporter: &mut buzz_export::Exporter,
    report: &mut BuildReport,
) -> Result<Vec<IndexRecord>, String> {
    let rel = ai_index::relative(root, &asset.path);
    let mtime = mtime_of(&asset.path);
    let scene = library.load(asset).map_err(|e| e.to_string())?;
    let t = Instant::now();
    let (img, bounds, frame) = render_asset(exporter, &scene).ok_or("nothing to draw")?;
    let mut records = Vec::new();
    let mut images = Vec::new();
    let asset_guess = ai_index::guess_from_name(&asset.name);
    let mut rec = guessed_record(rel.clone(), &rel, None, &asset.name, &asset.folder, bounds, mtime, None);
    measure(&mut rec, &img);
    rec.thumb_frame = Some(frame);
    save_png(root, &rec.thumb, &img)?;
    records.push(rec);
    images.push(img);
    for (sym, name, b) in distinct_symbols(&scene) {
        let Some(img) = render_symbol(exporter, &scene, sym) else { continue };
        let id = format!("{rel}#{}", sym.0);
        let mut rec = guessed_record(id, &rel, Some(sym.0), &name, &asset.folder, b, mtime, Some(&asset_guess));
        measure(&mut rec, &img);
        save_png(root, &rec.thumb, &img)?;
        records.push(rec);
        images.push(img);
    }
    // AS4: an asset of loose shapes (a strip of bushes) lists its parts. Not
    // the Cast: a character is one thing, however it is drawn.
    let (part_recs, part_imgs) = part_records(root, &rel, asset, &scene, mtime, exporter)?;
    records.extend(part_recs);
    images.extend(part_imgs);
    report.render_seconds += t.elapsed().as_secs_f64();
    embed_into(&mut records, &images, report);
    Ok(records)
}

/// The part records (and thumbnails) of an asset of loose shapes; none for
/// the Cast or an asset with distinct symbols.
fn part_records(
    root: &Path,
    rel: &str,
    asset: &Asset,
    scene: &Scene,
    mtime: u64,
    exporter: &mut buzz_export::Exporter,
) -> Result<(Vec<IndexRecord>, Vec<image::RgbaImage>), String> {
    let mut records = Vec::new();
    let mut images = Vec::new();
    if ai_index::is_cast(&asset.folder) {
        return Ok((records, images));
    }
    let asset_guess = ai_index::guess_from_name(&asset.name);
    for (n, b) in distinct_parts(scene) {
        let name = format!("{} part {}", asset.name, n + 1);
        let Some(alone) = part_scene(scene, b, &name) else { continue };
        let Some((img, _, _)) = render_asset(exporter, &alone) else { continue };
        let id = format!("{rel}#part{n}");
        let mut rec = guessed_record(id, rel, None, &name, &asset.folder, b, mtime, Some(&asset_guess));
        rec.part = Some(n);
        measure(&mut rec, &img);
        save_png(root, &rec.thumb, &img)?;
        records.push(rec);
        images.push(img);
    }
    Ok((records, images))
}

/// SigLIP embeddings for `records` from their thumbnails (in order), unless
/// SigLIP already failed this run.
fn embed_into(records: &mut [IndexRecord], images: &[image::RgbaImage], report: &mut BuildReport) {
    if report.embed_error.is_none() && !images.is_empty() {
        let t = Instant::now();
        let flat: Vec<image::RgbaImage> = images.iter().map(|i| ai_index::flatten_on(i, FLAT_BG)).collect();
        match buzz_ai::siglip::embed(&flat) {
            Ok(embs) => {
                for (r, e) in records.iter_mut().zip(embs) {
                    r.embedding = e;
                }
            }
            Err(e) => report.embed_error = Some(e.to_string()),
        }
        report.embed_seconds += t.elapsed().as_secs_f64();
    }
}

fn save_png(root: &Path, rel: &str, img: &image::RgbaImage) -> Result<(), String> {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    img.save(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Is this error the server being away (stop asking), rather than one bad answer?
fn vlm_unreachable(e: &LlmError) -> bool {
    matches!(e, LlmError::Connection(_) | LlmError::Timeout(_))
        || matches!(e, LlmError::Http { status, .. } if *status == 404 || *status >= 500)
}

/// **Build or refresh the index** of the library at `root`.
///
/// `progress(done, total, what)`; `cancelled()` is checked between assets
/// and batches (what is done so far is saved).
pub fn build_index(
    root: &Path,
    opts: &BuildOptions,
    progress: &mut dyn FnMut(u64, u64, &str),
    cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<BuildReport> {
    let library = AssetLibrary::at(root);
    let mut index = AssetIndex::load(root);
    let mut report = BuildReport {
        assets_total: library.len(),
        ..Default::default()
    };

    // Records of assets that are gone are dropped; records of a moved asset
    // are simply rebuilt at the new path.
    let present: HashSet<String> = library.assets().iter().map(|a| ai_index::relative(root, &a.path)).collect();
    index.records.retain(|r| present.contains(&r.path));
    // Kinds decided under an older kind list are asked again; thumbnails not
    // yet measured for the Other/Check test are measured (no re-render).
    for r in index.records.iter_mut() {
        if r.taxonomy < ai_index::TAXONOMY_VERSION {
            r.guessed = true;
            r.taxonomy = ai_index::TAXONOMY_VERSION;
        }
        if (r.blank.is_none() || r.fill.is_none() || r.tone.is_none())
            && let Some(t) = load_thumb(root, &r.thumb)
        {
            r.blank.get_or_insert_with(|| ai_index::thumb_is_blank(&t));
            r.fill.get_or_insert_with(|| ai_index::thumb_fill(&t));
            r.tone.get_or_insert_with(|| ai_index::thumb_tone(&t));
        }
    }
    // An asset whose frame-0 thumbnail came out empty before thumbnails took
    // the first frame with ink (an animation that starts empty) is drawn again.
    let rethumb: HashSet<String> = index
        .records
        .iter()
        .filter(|r| r.is_asset() && r.thumb_frame.is_none() && r.blank == Some(true))
        .filter(|r| load_thumb(root, &r.thumb).is_none_or(|t| ai_index::thumb_is_empty(&t)))
        .map(|r| r.path.clone())
        .collect();
    // What was only guessed follows the current name heuristics.
    for r in index.records.iter_mut().filter(|r| r.guessed) {
        let mut g = ai_index::guess_from_name(&r.name);
        if !r.is_asset() && g.kind == ai_index::AssetKind::Other {
            g = ai_index::guess_from_name(&ai_index::stem_of(&r.path));
        }
        r.category = g.category;
        r.kind = g.kind;
        r.view = g.view;
        r.tags = g.tags;
    }

    let mut vlm = opts.vlm.clone();
    if let Some((backend, _)) = &vlm
        && let Err(e) = backend.list_models()
    {
        report.vlm_error = Some(format!("VLM not reachable ({e}); records marked guessed"));
        vlm = None;
    }

    // Which assets need work.
    let mut work: Vec<(Asset, bool)> = Vec::new(); // (asset, needs render)
    for a in library.assets() {
        let rel = ai_index::relative(root, &a.path);
        let fresh = index.asset_record(&rel).is_some_and(|r| r.mtime == mtime_of(&a.path)) && !rethumb.contains(&rel);
        let needs_vlm = vlm.is_some() && index.records.iter().any(|r| r.path == rel && r.guessed);
        if !fresh {
            work.push((a.clone(), true));
        } else if needs_vlm {
            work.push((a.clone(), false));
        } else {
            report.assets_kept += 1;
        }
    }
    if let Some(limit) = opts.limit {
        report.assets_kept += work.len().saturating_sub(limit);
        work.truncate(limit);
    }

    let total = work.len() as u64;
    let mut exporter = if work.iter().any(|(_, r)| *r) {
        Some(buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?)
    } else {
        None
    };
    let mut worked_paths: Vec<String> = Vec::new();
    for (n, (asset, render)) in work.iter().enumerate() {
        if cancelled() {
            break;
        }
        progress(n as u64, total, &format!("Indexing {}", asset.name));
        let rel = ai_index::relative(root, &asset.path);
        worked_paths.push(rel.clone());
        if !*render {
            continue;
        }
        let Some(exporter) = exporter.as_mut() else { break };
        match index_one(root, &library, asset, exporter, &mut report) {
            Ok(records) => {
                index.records.retain(|r| r.path != rel);
                index.records.extend(records);
                report.assets_rendered += 1;
            }
            Err(e) => report.assets_failed.push(format!("{rel}: {e}")),
        }
        if n % 25 == 24 {
            index.save(root)?;
        }
    }
    index.save(root)?;

    // AS4 on an index from before parts: the assets not looked at for parts
    // yet (no `thumb_frame`) get their part records added — loaded, not
    // re-rendered, and their own records (and any VLM answers) kept.
    let worked_now: HashSet<String> = worked_paths.iter().cloned().collect();
    let mut parts_todo: Vec<Asset> = library
        .assets()
        .iter()
        .filter(|a| !ai_index::is_cast(&a.folder))
        .filter(|a| {
            let rel = ai_index::relative(root, &a.path);
            !worked_now.contains(&rel)
                && index.asset_record(&rel).is_some_and(|r| r.thumb_frame.is_none())
                && index.symbol_records(&rel).next().is_none()
        })
        .cloned()
        .collect();
    if let Some(limit) = opts.limit {
        parts_todo.truncate(limit);
    }
    let parts_total = parts_todo.len() as u64;
    for (n, asset) in parts_todo.iter().enumerate() {
        if cancelled() {
            break;
        }
        progress(n as u64, parts_total, &format!("Looking for parts in {}", asset.name));
        let rel = ai_index::relative(root, &asset.path);
        let Ok(scene) = library.load(asset) else { continue };
        if !distinct_parts(&scene).is_empty() {
            if exporter.is_none() {
                exporter = Some(buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?);
            }
            let Some(ex) = exporter.as_mut() else { break };
            let t = Instant::now();
            let mtime = index.asset_record(&rel).map(|r| r.mtime).unwrap_or_else(|| mtime_of(&asset.path));
            match part_records(root, &rel, asset, &scene, mtime, ex) {
                Ok((mut recs, imgs)) => {
                    report.render_seconds += t.elapsed().as_secs_f64();
                    embed_into(&mut recs, &imgs, &mut report);
                    index.records.retain(|r| !(r.path == rel && r.part.is_some()));
                    index.records.extend(recs);
                    worked_paths.push(rel.clone());
                }
                Err(e) => report.assets_failed.push(format!("{rel}: parts: {e}")),
            }
        }
        if let Some(r) = index.records.iter_mut().find(|r| r.path == rel && r.is_asset()) {
            r.thumb_frame = Some(0);
        }
        if n % 50 == 49 {
            index.save(root)?;
        }
    }
    index.save(root)?;

    // The VLM, in batches of 8, over every still-guessed record of the
    // assets worked on this run.
    if let Some((backend, model)) = &vlm {
        let worked: HashSet<&str> = worked_paths.iter().map(String::as_str).collect();
        let todo: Vec<usize> = (0..index.records.len())
            .filter(|&i| index.records[i].guessed && worked.contains(index.records[i].path.as_str()))
            .collect();
        let batches = todo.len().div_ceil(VLM_BATCH);
        let mut failures = 0;
        for (b, chunk) in todo.chunks(VLM_BATCH).enumerate() {
            if cancelled() {
                break;
            }
            progress(b as u64, batches as u64, &format!("Identifying with the VLM ({}/{batches})", b + 1));
            let items: Vec<KindItem> = chunk
                .iter()
                .map(|&i| {
                    let r = &index.records[i];
                    KindItem {
                        id: r.id.clone(),
                        old_name: r.name.clone(),
                        png_bytes: load_thumb(root, &r.thumb).map(|t| flat_png(&t)).unwrap_or_default(),
                    }
                })
                .collect();
            let t = Instant::now();
            match ai_index::identify_kinds_batch(backend, model, &items) {
                Ok(answers) => {
                    report.vlm_batches.push(t.elapsed().as_secs_f64());
                    failures = 0;
                    for &i in chunk {
                        let r = &mut index.records[i];
                        if let Some(a) = answers.iter().find(|a| a.id == r.id) {
                            r.category = a.category;
                            r.kind = a.kind;
                            r.view = a.view;
                            r.tags = a.tags.clone();
                            r.label = Some(a.name.clone());
                            r.guessed = false;
                            report.vlm_records += 1;
                        }
                    }
                }
                Err(e) => {
                    failures += 1;
                    let unreachable = vlm_unreachable(&e);
                    report.vlm_error = Some(format!("batch {}: {e}", b + 1));
                    if unreachable || failures >= 3 {
                        break;
                    }
                }
            }
            index.save(root)?;
        }
    }
    index.save(root)?;

    report.records = index.records.len();
    report.symbol_records = index.records.iter().filter(|r| !r.is_asset()).count();
    report.guessed = index.records.iter().filter(|r| r.guessed).count();
    progress(total, total, "Index ready");
    Ok(report)
}

// ------------------------------------------------------------ arrange

/// `(relative path, name, folder)` of every asset.
pub fn library_listing(library: &AssetLibrary) -> Vec<(String, String, String)> {
    let Some(root) = library.root() else { return Vec::new() };
    library
        .assets()
        .iter()
        .map(|a| (ai_index::relative(root, &a.path), a.name.clone(), a.folder.clone()))
        .collect()
}

/// The arrange plan for `library` from its index.
pub fn plan_for(library: &AssetLibrary) -> ai_index::ArrangePlan {
    let Some(root) = library.root() else { return Default::default() };
    let index = AssetIndex::load(root);
    ai_index::plan_arrange(&index, &library_listing(library), ai_index::STYLE_COSINE)
}

/// Up to nine thumbnails on white, three to a row, as one PNG (the contact
/// sheet the VLM names a style from).
pub fn contact_sheet_png(thumbs: &[image::RgbaImage]) -> Vec<u8> {
    const GAP: u32 = 4;
    let thumbs = &thumbs[..thumbs.len().min(9)];
    let cols = (thumbs.len() as u32).clamp(1, 3);
    let rows = (thumbs.len() as u32).div_ceil(3).max(1);
    let mut sheet = image::RgbaImage::from_pixel(
        cols * THUMB + (cols + 1) * GAP,
        rows * THUMB + (rows + 1) * GAP,
        image::Rgba([0xC8, 0xC8, 0xC8, 255]),
    );
    for (k, t) in thumbs.iter().enumerate() {
        let flat = ai_index::flatten_on(t, FLAT_BG);
        let cell = if flat.dimensions() == (THUMB, THUMB) {
            flat
        } else {
            image::imageops::resize(&flat, THUMB, THUMB, image::imageops::FilterType::Triangle)
        };
        let (c, r) = (k as u32 % 3, k as u32 / 3);
        image::imageops::replace(
            &mut sheet,
            &cell,
            i64::from(GAP + c * (THUMB + GAP)),
            i64::from(GAP + r * (THUMB + GAP)),
        );
    }
    let mut out = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(sheet)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

/// Ask the VLM to name every style cluster of the plan that has no name yet,
/// from a contact sheet of up to nine members spread through the cluster;
/// names are kept in the index. Returns how many were named, or the error
/// that stopped it.
pub fn name_styles(
    root: &Path,
    plan: &ai_index::ArrangePlan,
    backend: &Backend,
    model: &str,
    progress: &mut dyn FnMut(u64, u64, &str),
    cancelled: &dyn Fn() -> bool,
) -> Result<usize, String> {
    let mut index = AssetIndex::load(root);
    let todo: Vec<&ai_index::StyleCluster> =
        plan.clusters.iter().filter(|c| !index.style_names.contains_key(&c.key)).collect();
    let mut named = 0;
    for (n, c) in todo.iter().enumerate() {
        if cancelled() {
            break;
        }
        progress(n as u64, todo.len() as u64, &format!("Naming styles ({}/{})", n + 1, todo.len()));
        let picks = c.members.len().min(9);
        let thumbs: Vec<image::RgbaImage> = (0..picks)
            .map(|k| &c.members[k * c.members.len() / picks])
            .filter_map(|p| index.asset_record(p))
            .filter_map(|r| load_thumb(root, &r.thumb))
            .collect();
        if thumbs.is_empty() {
            continue;
        }
        let sheet = contact_sheet_png(&thumbs);
        match ai_index::name_style(backend, model, &c.type_folder, &sheet, thumbs.len()) {
            Ok(name) if !name.is_empty() => {
                index.style_names.insert(c.key.clone(), name);
                named += 1;
                index.save(root).map_err(|e| e.to_string())?;
            }
            Ok(_) => {}
            Err(e) if vlm_unreachable(&e) => return Err(e.to_string()),
            Err(_) => {}
        }
    }
    Ok(named)
}
