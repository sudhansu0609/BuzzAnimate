//! **Sets from the user's own assets** (AS3): the placing half. The plan —
//! which pieces, where, why — is `buzz_ai::asset_sets::plan_set`; this loads
//! each chosen record (the whole asset, or only one symbol or one part inside
//! it: AS4), places it with the same merge the Assets panel uses, scales it
//! onto its line (or over the stage, for a sky or a room), gives its layers
//! their depth and puts them in the stack behind everything already staged.
//!
//! Used by the episode builder (`episode::build_scene`, when the casting gives
//! no `background`/`scenery` or says `"scenery": "assets"`), the Story panel's
//! Set the scene and Direct (with **Use my assets** on), and the agent's
//! `dress_set_from_assets`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use buzz_ai::asset_index::{self as ai_index, AssetIndex, AssetKind, IndexRecord};
use buzz_ai::asset_sets::{self as sets, SetPiece, SetPlan, SetQuery, SetRole};
use buzz_doc::AssetLibrary;
use buzz_geom::Rect;
use buzz_scene::{LayerId, Scene, SymbolId};

use crate::episode::{cover, place_scene, stand_on, transform_layers};

/// A record's colour for the planner's colour gate (AS5): measured from its
/// cached thumbnail (no re-render), once per thumbnail per run, kept in
/// memory only — the index file is never written here.
fn record_swatch(root: &Path, record: &IndexRecord) -> Option<sets::Swatch> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, Option<sets::Swatch>>>> = OnceLock::new();
    if record.thumb.is_empty() {
        return None;
    }
    let path = root.join(&record.thumb);
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(known) = memo.lock().ok().and_then(|m| m.get(&path).copied()) {
        return known;
    }
    let swatch = image::open(&path).ok().and_then(|img| sets::thumb_swatch(&img.to_rgba8()));
    if let Ok(mut m) = memo.lock() {
        m.insert(path, swatch);
    }
    swatch
}

/// Whether the library has an asset index (the Assets panel's Arrange by type
/// & style… builds it).
pub fn has_index(library: &AssetLibrary) -> bool {
    library
        .root()
        .is_some_and(|root| AssetIndex::path_in(root).is_file())
}

/// The library's own index, ready for planning (see [`ready`]); `None` when
/// there is none or it holds no records.
pub fn index_for(library: &AssetLibrary) -> Option<AssetIndex> {
    let root = library.root()?;
    load_index_file(&AssetIndex::path_in(root))
}

/// An index read from any file — the library's, or a copy elsewhere (its
/// thumbnails are read from beside it, as `.buzz-index/thumbs/`).
pub fn load_index_file(path: &Path) -> Option<AssetIndex> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut index: AssetIndex = serde_json::from_str(&text).ok()?;
    if index.version != ai_index::INDEX_VERSION || index.records.is_empty() {
        return None;
    }
    let dir: PathBuf = path.parent().map(Path::to_path_buf).unwrap_or_default();
    ready(&mut index, &dir);
    Some(index)
}

/// Measure, from the cached thumbnails under `dir`, the fill of every record
/// the planner might use that an older index has not measured.
pub fn ready(index: &mut AssetIndex, dir: &Path) {
    for r in index.records.iter_mut() {
        let used = matches!(
            r.kind,
            AssetKind::Sky | AssetKind::Interior | AssetKind::House | AssetKind::Shop | AssetKind::Tree | AssetKind::Bush
        );
        if used
            && (r.fill.is_none() || r.tone.is_none())
            && let Some(t) = crate::asset_index::load_thumb(dir, &r.thumb)
        {
            r.fill.get_or_insert_with(|| ai_index::thumb_fill(&t));
            r.tone.get_or_insert_with(|| ai_index::thumb_tone(&t));
        }
    }
}

/// Where the set goes in the stack.
#[derive(Debug, Clone, Copy, Default)]
pub struct SetFrame {
    /// The procedural sky (`staging::build`'s backdrop): a sky piece goes
    /// directly in front of it.
    pub backdrop: Option<LayerId>,
    /// The procedural ground: every other piece goes in front of it (and so
    /// behind anything staged after it).
    pub ground: Option<LayerId>,
}

impl SetFrame {
    /// The layers `Set the Scene` makes, found by name.
    pub fn by_name(scene: &Scene) -> Self {
        let find = |name: &str| scene.stage_layers().iter().find(|l| l.name == name).map(|l| l.id);
        Self { backdrop: find("Sky"), ground: find("Ground") }
    }
}

/// What a dressing did.
#[derive(Debug, Clone, Default)]
pub struct Dressed {
    pub plan: SetPlan,
    /// One line per piece placed: role, name, record, why.
    pub placed: Vec<String>,
    /// Pieces that could not be placed, with why.
    pub failed: Vec<String>,
    /// Every layer the set brought.
    pub layers: Vec<LayerId>,
    /// The front layer of the sky that was placed: procedural scenery laid
    /// as well goes in front of it.
    pub sky_front: Option<LayerId>,
}

impl Dressed {
    /// Nothing from the index was placed except perhaps a sky.
    pub fn needs_procedural(&self) -> bool {
        self.plan.procedural || !self.placed.iter().any(|l| !l.starts_with("sky "))
    }

    /// A one-paragraph summary for a status line or a report.
    pub fn summary(&self) -> String {
        let mut by_role: Vec<(SetRole, Vec<&str>)> = Vec::new();
        for p in &self.plan.pieces {
            if self.failed.iter().any(|f| f.starts_with(&p.id)) {
                continue;
            }
            match by_role.iter_mut().find(|(r, _)| *r == p.role) {
                Some((_, v)) => v.push(&p.name),
                None => by_role.push((p.role, vec![&p.name])),
            }
        }
        let mut s = by_role
            .iter()
            .map(|(r, v)| format!("{}: {}", r.label(), v.join(", ")))
            .collect::<Vec<_>>()
            .join("; ");
        for n in &self.plan.notes {
            s.push_str(&format!(" ({n})"));
        }
        s
    }
}

/// The asset file a record lives in.
fn asset_of<'a>(library: &'a AssetLibrary, rel: &str) -> Option<&'a buzz_doc::Asset> {
    let root = library.root()?;
    library.assets().iter().find(|a| ai_index::relative(root, &a.path) == rel)
}

/// **Load what an index record stands for** (AS4): the whole asset, or only
/// one symbol inside it (with the symbols it needs), or only one part of an
/// asset of loose shapes.
pub fn record_scene(library: &AssetLibrary, record: &IndexRecord) -> Result<Scene, String> {
    let asset = asset_of(library, &record.path).ok_or_else(|| format!("{}: not in the library", record.path))?;
    let scene = library.load(asset).map_err(|e| format!("{}: {e}", record.path))?;
    if let Some(sym) = record.symbol {
        return crate::asset_index::symbol_scene(&scene, SymbolId(sym), &record.name)
            .ok_or_else(|| format!("{}: symbol {sym} is not in it any more", record.path));
    }
    if record.part.is_some() {
        let b = record.bounds;
        return crate::asset_index::part_scene(&scene, Rect::new(b[0], b[1], b[2], b[3]), &record.name)
            .ok_or_else(|| format!("{}: nothing left where part {:?} was", record.path, record.part));
    }
    Ok(scene)
}

/// **What an object draws on the frame it shows**: a symbol is measured by
/// the artwork of the frame its instance shows, not (as `resolved_bounds`
/// does, for hit testing) over every frame — an animated figure that stands
/// up later measured as tall as its standing pose, and a sitting one placed
/// at a height came out a third of the size.
fn drawn_at(scene: &Scene, o: &buzz_scene::Object, elapsed: u32, depth: u32) -> Option<Rect> {
    use buzz_scene::object::transform_rect;
    if !o.visible || depth > 24 {
        return None;
    }
    let usable = |b: Rect| b.width().is_finite() && b.height().is_finite() && (b.width() > 0.0 || b.height() > 0.0);
    match &o.kind {
        buzz_scene::ObjectKind::Instance(i) => {
            let sym = scene.library().get(i.symbol)?;
            let f = i.resolve_frame(sym.kind, elapsed, sym.length().max(1));
            let mut acc: Option<Rect> = None;
            for layer in sym.layers.iter() {
                if !layer.visible || matches!(layer.kind, buzz_scene::LayerKind::Folder | buzz_scene::LayerKind::Mask) {
                    continue;
                }
                let follows = sym.layers.inherited_transform(layer.id, f);
                for c in layer.objects_at(f) {
                    if let Some(b) = drawn_at(scene, c, f, depth + 1) {
                        let b = transform_rect(follows, b);
                        acc = Some(acc.map_or(b, |a| a.union(b)));
                    }
                }
            }
            acc.map(|a| transform_rect(o.transform, a)).filter(|b| usable(*b))
        }
        buzz_scene::ObjectKind::Group(children) => children
            .iter()
            .filter_map(|c| drawn_at(scene, c, elapsed, depth + 1))
            .map(|b| transform_rect(o.transform, b))
            .reduce(|a, b| a.union(b)),
        _ => Some(scene.resolved_bounds(o)).filter(|b| usable(*b)),
    }
}

/// The box round what `layers` draw on frame 0.
fn drawn_box(scene: &Scene, layers: &[LayerId]) -> Option<Rect> {
    let mut acc: Option<Rect> = None;
    for id in layers {
        let Some(layer) = scene.stage_layers().get(*id) else { continue };
        let follows = scene.stage_layers().inherited_transform(*id, 0);
        for o in layer.objects_at(0) {
            if let Some(b) = drawn_at(scene, o, 0, 0) {
                let b = buzz_scene::object::transform_rect(follows, b);
                acc = Some(acc.map_or(b, |a| a.union(b)));
            }
        }
    }
    acc
}

/// Move `layer` to directly in front of `anchor` in the stage stack.
fn put_in_front_of(scene: &mut Scene, layer: LayerId, anchor: LayerId) {
    let (Some(l), Some(a)) = (scene.stage_layer_index(layer), scene.stage_layer_index(anchor)) else {
        return;
    };
    let to = if l < a { a - 1 } else { a };
    scene.reorder_stage_layer(layer, to);
}

/// The set roles whose pieces keep their cast shadow; every other piece is
/// placed with **Cast shadow** off (still lit and tinted).
const SET_PIECES_CAST: &[SetRole] = &[SetRole::Ground];

/// **Place one planned piece.** Returns the layers it brought (front first).
pub fn place_piece(
    scene: &mut Scene,
    library: &AssetLibrary,
    record: &IndexRecord,
    piece: &SetPiece,
    horizon_y: f64,
) -> Result<Vec<LayerId>, String> {
    let incoming = record_scene(library, record)?;
    let placed = place_scene(scene, &incoming);
    let Some(bounds) = drawn_box(scene, &placed.all)
        .or(placed.bounds)
        .filter(|b| b.width() > 0.0 && b.height() > 0.0)
    else {
        for id in &placed.all {
            scene.remove_layer(*id);
        }
        return Err(format!("{}: nothing to draw on its first frame", record.id));
    };
    let stage = scene.stage().stage_rect();
    let t = match piece.role {
        // Over the stage above the horizon (a little past it, under the
        // ground's edge), and a little wider than the stage.
        SetRole::Sky => cover(
            bounds,
            Rect::new(
                stage.x0 - stage.width() * 0.05,
                stage.y0,
                stage.x1 + stage.width() * 0.05,
                horizon_y + stage.height() * 0.04,
            ),
        ),
        SetRole::Room => cover(bounds, stage),
        _ => {
            stand_on(
                bounds,
                stage.height() * piece.height,
                stage.x0 + stage.width() * piece.x,
                stage.y0 + stage.height() * piece.base,
            )
            .0
        }
    };
    transform_layers(scene, &placed.top, t);
    let unlit = piece.role.covers();
    // Set pieces stay lit and tinted but throw no cast shadow (AS5): a hut or
    // a tree far behind the cast laid long dark streaks across the foreground
    // grass.
    let casts_shadow = SET_PIECES_CAST.contains(&piece.role);
    for id in &placed.all {
        scene.update_stage_layer(*id, |l| {
            l.depth = piece.depth;
            if unlit {
                l.unlit = true;
            }
            if !casts_shadow {
                l.casts_shadow = false;
            }
        });
    }
    for (k, id) in placed.top.iter().enumerate() {
        let name = if placed.top.len() == 1 {
            format!("Set {} \u{2014} {}", piece.role.label(), record.name)
        } else {
            format!("Set {} \u{2014} {} {}", piece.role.label(), record.name, k + 1)
        };
        scene.update_stage_layer(*id, |l| l.name = name);
    }
    Ok(placed.all)
}

/// **Dress the set** in `scene` from `index` for `query`: plan it, place every
/// piece, sky in front of `frame.backdrop`, the rest back to front in front
/// of `frame.ground` (behind anything staged after it), every layer as long
/// as the scene.
pub fn dress_set(
    scene: &mut Scene,
    library: &AssetLibrary,
    index: &AssetIndex,
    query: &SetQuery,
    frame: &SetFrame,
) -> Dressed {
    let stage = scene.stage().stage_rect();
    let root = library.root().map(Path::to_path_buf);
    let colour = |r: &IndexRecord| root.as_deref().and_then(|root| record_swatch(root, r));
    let plan = sets::plan_set_coloured(index, query, stage.width() / stage.height().max(1e-6), &colour);
    let horizon_y = stage.y0 + stage.height() * query.horizon;
    let mut out = Dressed { plan: plan.clone(), ..Default::default() };
    let mut rows_anchor = frame.ground.or(frame.backdrop);
    // Each piece's layers (front first), in the order placed (back first).
    let mut blocks: Vec<Vec<LayerId>> = Vec::new();
    for piece in &plan.pieces {
        let Some(record) = index.records.iter().find(|r| r.id == piece.id) else {
            out.failed.push(format!("{}: not in the index", piece.id));
            continue;
        };
        match place_piece(scene, library, record, piece, horizon_y) {
            Ok(layers) => {
                // Back-most first, each in front of the last: the block keeps
                // its own order and lands directly in front of the anchor.
                let anchor = if piece.role == SetRole::Sky { frame.backdrop } else { rows_anchor };
                if let Some(mut a) = anchor {
                    for id in layers.iter().rev() {
                        put_in_front_of(scene, *id, a);
                        a = *id;
                    }
                    if piece.role == SetRole::Sky {
                        out.sky_front = Some(a);
                    } else {
                        rows_anchor = Some(a);
                    }
                }
                out.placed.push(format!(
                    "{} \u{2014} {} ({}) \u{2014} {}",
                    piece.role.label(),
                    piece.name,
                    piece.id,
                    piece.why
                ));
                out.layers.extend(layers.iter().copied());
                blocks.push(layers);
            }
            Err(e) => out.failed.push(format!("{}: {e}", piece.id)),
        }
    }
    // No staged sky or ground to stand in front of: the whole set goes to
    // the very back, behind everything already there, in its own order.
    if frame.ground.is_none() && frame.backdrop.is_none() {
        for block in blocks.iter().rev() {
            for id in block {
                let last = scene.stage_layers().len().saturating_sub(1);
                scene.reorder_stage_layer(*id, last);
            }
        }
    }
    // As long as the scene, so nothing vanishes after frame one.
    let last = scene.frame_count().saturating_sub(1);
    let ours: HashSet<LayerId> = out.layers.iter().copied().collect();
    for id in ours {
        scene.update_stage_layer(id, |l| {
            if l.frames.length() <= last {
                l.frames.insert_frame(last);
            }
        });
    }
    out
}

/// The time of day a staging setting means, for choosing a sky.
pub fn time_of(setting: buzz_act::Setting) -> sets::TimeOfDay {
    match setting {
        buzz_act::Setting::Night | buzz_act::Setting::Storm => sets::TimeOfDay::Night,
        buzz_act::Setting::Sunset => sets::TimeOfDay::Evening,
        _ => sets::TimeOfDay::Day,
    }
}

/// **Dress a staged or directed shot** (Story panel's Set the scene and
/// Direct with **Use my assets** on): the request's words and setting as a
/// query, the cast standing where the staging put them.
pub fn dress_request(
    scene: &mut Scene,
    library: &AssetLibrary,
    index: &AssetIndex,
    request: &buzz_act::SetRequest,
) -> Dressed {
    let horizon = request.horizon.clamp(0.15, 0.95);
    let query = SetQuery {
        text: format!("{} {}", request.scenery.label(), request.text),
        interior: request.setting == buzz_act::Setting::Interior,
        time: time_of(request.setting),
        style: None,
        seed: sets::seed_of(&format!("{:?} {}", request.setting, request.text)),
        marks: request.marks.clone(),
        horizon,
        // Where `staging::build` stands its nearest figure.
        ground: horizon + (1.0 - horizon) * 0.72,
    };
    dress_set(scene, library, index, &query, &SetFrame { backdrop: request.backdrop, ground: request.ground })
}

/// Where the rigs on the stage stand, `0..1` across: marks for the planner.
pub fn rig_marks(scene: &Scene, frame: u32) -> Vec<f64> {
    let stage = scene.stage().stage_rect();
    let mut seen: HashSet<buzz_scene::ObjectId> = HashSet::new();
    let mut out = Vec::new();
    for layer in scene.stage_layers().iter() {
        for o in layer.objects_at(frame) {
            if let Some(rig) = buzz_act::layer_rig::rig_of(scene, o.id)
                && seen.insert(rig.body())
            {
                let b = scene.resolved_bounds(o);
                if b.width().is_finite() {
                    out.push((b.center().x - stage.x0) / stage.width().max(1e-6));
                }
            }
        }
    }
    out
}
