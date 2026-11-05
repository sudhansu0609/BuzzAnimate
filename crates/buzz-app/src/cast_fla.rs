//! **Cast from FLA** (ALAXMI_PRODUCTION_PLAN B1a): the creator's own cast file
//! (`stories/<story>/BuzzAnimate/cast.fla`) turned into locked Cast assets.
//!
//! The FLA has one wrapper symbol per role on the stage (`deepak`, `bhushan`,
//! `mishra`, `radhe`, `padosi`, `padosi2`), holding that role's pose symbols
//! (Front, Side, Back, 3-4 — long holds with one key per layer) and walk cycles
//! (39-40 frames, real cycles). Each inner symbol is classified by the tokens
//! of its name, then saved as its own asset under `Cast/<Series>/`:
//!
//! * `<Role>`, `<Role> Side`, `<Role> Back`, `<Role> 3-4`: the pose symbols
//!   (what `produce.rs` calls views), cut to one frame with Start Fresh;
//! * `<Role> Pose walk_<view>`: the walk cycles, every frame kept. The
//!   producer walks a rig procedurally (`buzz_act::perform`); it never plays a
//!   cycle symbol, so these are filed as pose variants and are not played by it.
//!
//! The symbol's layers (the creator's own layer parenting, named after the
//! parts) are lifted onto the asset's stage, which is what a layer rig is
//! ([`buzz_act::layer_rig`]). When the mouth or arms are not found that way,
//! the AI Layer Rig route (`rigging::plan_layer_rig_auto`) is tried on the
//! symbol first. The FLA is only ever read; the Cast folder and the series'
//! `casting.json` are backed up before anything is written there, and an asset
//! this import did not make is never overwritten.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use buzz_doc::{Asset, AssetLibrary};
use buzz_geom::{Affine, Rect};
use buzz_scene::{Keyframe, KeepPose, LayerTimeline, LoopMode, ObjectKind, Scene, SymbolId};
use serde_json::{json, Value};

/// The wrapper symbols on the stage and the role each one plays.
pub const ROLES: [(&str, &str); 6] = [
    ("deepak", "DEEPAK"),
    ("bhushan", "BHUSHAN"),
    ("mishra", "MISHRA JI"),
    ("radhe", "RADHE"),
    ("padosi", "PADOSI 1"),
    ("padosi2", "PADOSI 2"),
];

/// Where the import's own assets are listed, beside `casting.json`.
const STAMP_FILE: &str = "cast_fla_import.json";

/// Which way a character faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum View {
    Front,
    Side,
    Back,
    ThreeQuarter,
}

impl View {
    /// The producer's suffix on the Cast rig name (`produce.rs` `view_rank`).
    pub fn suffix(self) -> &'static str {
        match self {
            View::Front => "",
            View::Side => " Side",
            View::Back => " Back",
            View::ThreeQuarter => " 3-4",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            View::Front => "front",
            View::Side => "side",
            View::Back => "back",
            View::ThreeQuarter => "3-4",
        }
    }
}

/// `(view, is a walk cycle)` from the tokens of a symbol's name.
pub fn classify(name: &str) -> (View, bool) {
    let n = name.to_lowercase();
    let walk = n.contains("walk");
    let view = if n.contains("3-4") || n.contains("34") {
        View::ThreeQuarter
    } else if n.contains("back") {
        View::Back
    } else if n.contains("side") {
        View::Side
    } else {
        View::Front
    };
    (view, walk)
}

/// The Cast asset name of a role's `view` (a pose), or of its walk.
pub fn asset_name(role: &str, view: View, walk: bool) -> String {
    if walk {
        crate::cast_build::pose_rig_name(role, &format!("walk_{}", view.word()))
    } else {
        format!("{role}{}", view.suffix())
    }
}

/// One inner symbol of a role's wrapper.
#[derive(Debug, Clone)]
pub struct Piece {
    pub symbol: SymbolId,
    pub name: String,
    pub view: View,
    pub walk: bool,
    pub layers: usize,
}

/// One role of the FLA.
#[derive(Debug, Clone)]
pub struct FoundRole {
    pub role: &'static str,
    /// The stage wrapper's name (`deepak`).
    pub wrapper: String,
    /// One symbol per (view, walk), the one with the most layers.
    pub chosen: BTreeMap<(View, bool), Piece>,
    /// Same (view, walk) symbols that were not taken.
    pub extra: Vec<String>,
}

fn kids(scene: &Scene, id: SymbolId) -> Vec<SymbolId> {
    let mut out: Vec<SymbolId> = Vec::new();
    let Some(symbol) = scene.library().get(id) else { return out };
    for layer in symbol.layers.iter() {
        for key in layer.frames.keyframes() {
            for o in key.objects.iter() {
                if let ObjectKind::Instance(i) = &o.kind
                    && !out.contains(&i.symbol)
                {
                    out.push(i.symbol);
                }
            }
        }
    }
    out
}

/// **The roles of the FLA**: each stage wrapper by name, a single-instance
/// wrapper looked through (`padosi` holds `oldManNew`), the pose-shaped
/// symbols (eight layers or more) classified.
pub fn find_roles(scene: &Scene) -> Vec<FoundRole> {
    let mut wrappers: Vec<(&'static str, SymbolId, String)> = Vec::new();
    for layer in scene.stage_layers().iter() {
        for key in layer.frames.keyframes() {
            for o in key.objects.iter() {
                let ObjectKind::Instance(i) = &o.kind else { continue };
                let Some(symbol) = scene.library().get(i.symbol) else { continue };
                let lower = symbol.name.to_lowercase();
                if let Some((_, role)) = ROLES.iter().find(|(w, _)| *w == lower)
                    && !wrappers.iter().any(|(r, _, _)| r == role)
                {
                    wrappers.push((role, symbol.id, symbol.name.clone()));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (role, wrapper, wrapper_name) in wrappers {
        let mut current = wrapper;
        for _ in 0..4 {
            let inside = kids(scene, current);
            let few_layers = scene.library().get(current).is_some_and(|s| s.layers.iter().count() <= 2);
            if inside.len() == 1 && few_layers {
                current = inside[0];
            } else {
                break;
            }
        }
        let mut chosen: BTreeMap<(View, bool), Piece> = BTreeMap::new();
        let mut extra = Vec::new();
        let mut inner = kids(scene, current);
        inner.sort_by_key(|id| scene.library().get(*id).map(|s| s.name.clone()).unwrap_or_default());
        for id in inner {
            let Some(symbol) = scene.library().get(id) else { continue };
            let layers = symbol.layers.iter().count();
            if layers < 8 {
                continue;
            }
            let (view, walk) = classify(&symbol.name);
            let piece = Piece { symbol: id, name: symbol.name.clone(), view, walk, layers };
            match chosen.get(&(view, walk)) {
                Some(old) if old.layers >= layers => extra.push(piece.name),
                Some(old) => {
                    extra.push(old.name.clone());
                    chosen.insert((view, walk), piece);
                }
                None => {
                    chosen.insert((view, walk), piece);
                }
            }
        }
        out.push(FoundRole { role, wrapper: wrapper_name, chosen, extra });
    }
    out
}

/// The box round the artwork of `scene`'s stage at `frame`, parenting included.
pub fn character_bounds(scene: &Scene, frame: u32) -> Option<Rect> {
    bounds_of(scene, frame, true)
}

fn bounds_of(scene: &Scene, frame: u32, skip_hidden: bool) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    for layer in scene.layers().iter() {
        if skip_hidden && !layer.visible {
            continue;
        }
        let inherited = scene.layers().inherited_transform(layer.id, frame);
        for o in layer.objects_at(frame).iter().filter(|o| o.visible) {
            let b = buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(o));
            if b.width() <= 0.0 && b.height() <= 0.0 {
                continue;
            }
            bounds = Some(bounds.map_or(b, |u| u.union(b)));
        }
    }
    bounds
}

/// **One asset's scene**: the symbol alone, its layers lifted onto the stage
/// (that is the layer rig), centred on the stage. A hold is cut to one frame
/// with Start Fresh; a walk keeps every frame. `ai_rig` runs the AI Layer Rig
/// on the symbol first (holds only); its note is returned.
pub fn build_scene(imported: &Scene, piece: &Piece, name: &str, hold: bool, ai_rig: bool) -> Result<(Scene, String), String> {
    let mut scene = crate::asset_index::symbol_scene(imported, piece.symbol, name).ok_or_else(|| format!("{}: not in the library", piece.name))?;
    let mut note = String::new();
    if hold {
        scene.start_fresh(KeepPose::First);
        // Start Fresh keeps the mouth and hand charts as they are but flattens
        // the eye charts (lids, whites, mask) to their first frame, which can
        // be a closed eye; they are put back as the FLA drew them.
        let eyes: Vec<SymbolId> = scene
            .library()
            .iter()
            .filter(|s| s.length() >= 2 && ["eye", "blink"].iter().any(|w| s.name.to_lowercase().contains(w)))
            .map(|s| s.id)
            .collect();
        for id in eyes {
            if let Some(original) = imported.library().get(id) {
                let layers = original.layers.clone();
                if let Some(stack) = scene.library_mut().layers_mut(id) {
                    *stack = layers;
                }
            }
        }
        if ai_rig {
            note = match ai_rig_symbol(&mut scene, piece.symbol) {
                Ok(text) => text,
                Err(e) => format!("AI Layer Rig not applied: {e}"),
            };
        }
    }
    let layers = scene.library().get(piece.symbol).ok_or("the symbol vanished")?.layers.clone();
    *scene.edit_stage_layers() = layers;
    if hold {
        let ids: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
        for id in ids {
            scene.update_layer(id, |layer| {
                let first = layer.frames.keyframes().first().map(|k| k.objects.clone()).unwrap_or_default();
                let mut kept = Keyframe::new(0);
                kept.objects = first;
                layer.frames = LayerTimeline::from_parts(vec![kept], 1);
            });
        }
    }
    if hold {
        let heads = head_symbols(&scene, scene.layers());
        for head in heads {
            for d in tidy_head_symbol(&mut scene, head) {
                note = if note.is_empty() { d } else { format!("{note}; {d}") };
            }
        }
    }
    // Spare parts parked outside the body are hidden (holds only), then the
    // character is centred on the stage: the symbol's own space puts its
    // origin on the stage's corner.
    let hidden = if hold { hide_spare_parts(&mut scene) } else { Vec::new() };
    if let Some(b) = bounds_of(&scene, 0, hold) {
        let size = scene.stage().size;
        let t = Affine::translate((size.width / 2.0 - b.center().x, size.height / 2.0 - b.center().y));
        let top: Vec<_> = scene.layers().iter().filter(|l| l.follows.is_none()).map(|l| l.id).collect();
        crate::episode::transform_layers(&mut scene, &top, t);
    }
    if hold {
        let eyes = open_eyes(&mut scene);
        for part in [eyes, if hidden.is_empty() { String::new() } else { format!("hidden spare parts: {}", hidden.join(", ")) }] {
            if !part.is_empty() {
                note = if note.is_empty() { part } else { format!("{note}; {part}") };
            }
        }
    }
    Ok((scene, note))
}

/// One eye chart instance the open-eye pass can set: a lid, mask or white
/// chart placed at the first key of a layer (on the stage when `owner` is
/// `None`, else inside that symbol).
#[derive(Debug, Clone)]
struct EyeSlot {
    owner: Option<SymbolId>,
    layer: buzz_scene::LayerId,
    index: usize,
    frames: u32,
    name: String,
}

fn eye_slots(scene: &Scene) -> Vec<EyeSlot> {
    let mut out = Vec::new();
    let mut visit = |owner: Option<SymbolId>, layers: &buzz_scene::LayerStack| {
        for layer in layers.iter() {
            let Some(key) = layer.frames.keyframes().first() else { continue };
            if key.start != 0 {
                continue;
            }
            for (index, o) in key.objects.iter().enumerate() {
                let ObjectKind::Instance(i) = &o.kind else { continue };
                let Some(target) = scene.library().get(i.symbol) else { continue };
                let frames = target.length();
                if !(2..=60).contains(&frames) {
                    continue;
                }
                let (l, s) = (layer.name.to_lowercase(), target.name.to_lowercase());
                if ["ball", "pupil"].iter().any(|w| l.contains(w) || s.contains(w)) {
                    continue;
                }
                if ["lid", "mask", "white", "blink", "lash"].iter().any(|w| l.contains(w) || s.contains(w)) {
                    out.push(EyeSlot { owner, layer: layer.id, index, frames, name: layer.name.clone() });
                }
            }
        }
    };
    visit(None, scene.layers());
    for symbol in scene.library().iter() {
        visit(Some(symbol.id), &symbol.layers);
    }
    out
}

fn set_eye_slot(scene: &mut Scene, slot: &EyeSlot, frame: u32) {
    let edit = |stack: &mut buzz_scene::LayerStack| {
        stack.update(slot.layer, |l| {
            if let Some(key) = l.frames.keyframes_mut().first_mut()
                && let Some(o) = Arc::make_mut(&mut key.objects).get_mut(slot.index)
                && let ObjectKind::Instance(i) = &mut Arc::make_mut(o).kind
            {
                i.loop_mode = LoopMode::SingleFrame;
                i.first_frame = frame;
            }
        });
    };
    match slot.owner {
        None => edit(scene.edit_stage_layers()),
        Some(id) => {
            if let Some(stack) = scene.library_mut().layers_mut(id) {
                edit(stack);
            }
        }
    }
}

/// White pixels in the top of the character (where the eyes are).
fn white_pixels(exporter: &mut buzz_export::Exporter, scene: &Scene, region: Rect) -> Result<usize, String> {
    let mut settings = buzz_export::ExportSettings::for_stage(scene);
    settings.width = 640;
    settings.height = ((640.0 * region.height() / region.width().max(1.0)).round() as u32).clamp(64, 1600);
    settings.transparent = true;
    settings.region = Some(region);
    let pic = exporter.render(scene, 0, &settings).map_err(|e| e.to_string())?;
    Ok(pic.pixels.chunks_exact(4).filter(|p| p[3] > 200 && p[0].min(p[1]).min(p[2]) >= 225).count())
}

/// **Open eyes on a hold** (ALAXMI_PRODUCTION_PLAN B1b): Start Fresh and the
/// FLA's frame 0 can land on a shut eye. Each lid / mask / white chart at the
/// first key is set, one chart at a time, to the frame whose render shows the
/// most white in the head area. Returns a note.
fn open_eyes(scene: &mut Scene) -> String {
    let slots = eye_slots(scene);
    if slots.is_empty() {
        return String::new();
    }
    let Some(b) = character_bounds(scene, 0) else { return String::new() };
    let head = Rect::new(b.x0, b.y0, b.x1, b.y0 + b.height() * 0.3);
    let mut exporter = match buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic) {
        Ok(e) => e,
        Err(e) => return format!("eyes not checked: {e}"),
    };
    let mut chosen: Vec<u32> = vec![u32::MAX; slots.len()];
    for pass in 0..2 {
        for (n, slot) in slots.iter().enumerate() {
            let mut best: Option<(usize, u32)> = None;
            for f in 0..slot.frames {
                set_eye_slot(scene, slot, f);
                match white_pixels(&mut exporter, scene, head) {
                    Ok(w) if best.is_none_or(|(bw, _)| w > bw) => best = Some((w, f)),
                    Ok(_) => {}
                    Err(e) => return format!("eyes not checked: {e}"),
                }
            }
            if let Some((_, f)) = best {
                set_eye_slot(scene, slot, f);
                chosen[n] = f;
            }
        }
        let _ = pass;
    }
    format!("eyes open: {}", slots.iter().zip(&chosen).map(|(s, f)| format!("{}@{f}", s.name)).collect::<Vec<_>>().join(", "))
}

/// **Spare parts hidden** (B1b): a hold's layers are grouped by touching
/// boxes (frame 0, parenting included); every group that does not touch the
/// group of the biggest layer (the body) is a spare part, e.g. a leg parked
/// beside the body in the FLA. Those layers are hidden, not deleted. Returns
/// their names.
fn hide_spare_parts(scene: &mut Scene) -> Vec<String> {
    let mut boxes: Vec<(buzz_scene::LayerId, String, Rect)> = Vec::new();
    for layer in scene.layers().iter() {
        let inherited = scene.layers().inherited_transform(layer.id, 0);
        let mut b: Option<Rect> = None;
        for o in layer.objects_at(0).iter().filter(|o| o.visible) {
            let r = buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(o));
            if r.width() <= 0.0 && r.height() <= 0.0 {
                continue;
            }
            b = Some(b.map_or(r, |u| u.union(r)));
        }
        if let Some(b) = b
            && layer.visible
        {
            boxes.push((layer.id, layer.name.clone(), b));
        }
    }
    let Some(whole) = boxes.iter().map(|x| x.2).reduce(|a, b| a.union(b)) else { return Vec::new() };
    let margin = whole.height() * 0.01;
    let n = boxes.len();
    let mut group: Vec<usize> = (0..n).collect();
    fn root(g: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while g[r] != r {
            r = g[r];
        }
        g[i] = r;
        r
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (boxes[i].2.inflate(margin, margin), boxes[j].2);
            if a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1 {
                let (ri, rj) = (root(&mut group, i), root(&mut group, j));
                group[ri] = rj;
            }
        }
    }
    let area = |r: &Rect| r.width() * r.height();
    let body = (0..n).max_by(|&a, &b| area(&boxes[a].2).total_cmp(&area(&boxes[b].2))).unwrap_or(0);
    let main = root(&mut group, body);
    let mut hidden = Vec::new();
    for i in 0..n {
        if root(&mut group, i) != main {
            let id = boxes[i].0;
            scene.update_layer(id, |l| l.visible = false);
            hidden.push(boxes[i].1.clone());
        }
    }
    hidden
}
/// The AI Layer Rig route on `symbol` (frame 0), applied. A note on what it did.
fn ai_rig_symbol(scene: &mut Scene, symbol: SymbolId) -> Result<String, String> {
    let profile = crate::rigging::naming_profile(None)?;
    let plan = crate::rigging::plan_layer_rig_auto(scene, symbol, 0, &profile, None).ok_or("no pieces to rig at frame 0")?;
    let applied = crate::rigging::apply_layer_rig_plan(scene, &plan).ok_or("the plan no longer matches the symbol")?;
    Ok(format!(
        "AI Layer Rig: {} pieces on {} layers ({} before){}",
        plan.pieces.len(),
        applied.layers_after,
        applied.layers_before,
        if plan.low_confidence { ", lower confidence" } else { "" }
    ))
}

/// The first object of a layer's first keyframe that has one.
fn first_object(l: &buzz_scene::Layer) -> Option<&Arc<buzz_scene::Object>> {
    l.frames.keyframes().iter().find_map(|k| k.objects.first())
}

/// The layer (and the symbol holding it) in `symbol`, or in a symbol it shows up to `depth`
/// deep, that `pick(layer name, chart name, chart frames)` accepts.
fn find_chart_layer(scene: &Scene, symbol: SymbolId, pick: &dyn Fn(&str, &str, u32) -> bool, depth: u32) -> Option<(SymbolId, buzz_scene::LayerId)> {
    let s = scene.library().get(symbol)?;
    for l in s.layers.iter() {
        let Some(chart) = l.frames.keyframes().iter().find_map(|k| k.objects.first()).and_then(|o| o.instance()).and_then(|i| scene.library().get(i.symbol)) else { continue };
        if pick(&l.name, &chart.name, chart.length()) {
            return Some((symbol, l.id));
        }
    }
    if depth == 0 {
        return None;
    }
    for l in s.layers.iter() {
        if let Some(o) = first_object(l) {
            if let Some(i) = o.instance()
                && i.symbol != symbol
                && let Some(found) = find_chart_layer(scene, i.symbol, pick, depth - 1)
            {
                return Some(found);
            }
        }
    }
    None
}

/// **Animate's head, named the Animate way** (SIMPLE_EPISODE_FLOW_PLAN E4b): inside the head
/// symbol the mouth chart's layer is named `Mouth` and the lid chart's `Eyelid`, with the
/// lid's wrapper symbol (`eyeLidWrap`, a long clip holding the chart) taken away so the chart
/// is on the layer itself. Nothing moves on screen: the wrapper's and the chart's placements
/// are multiplied into the layer's one object. Returns what was done.
pub fn tidy_head_symbol(scene: &mut Scene, head: SymbolId) -> Vec<String> {
    let mut done = Vec::new();
    let mouth = find_chart_layer(
        scene,
        head,
        &|layer, chart, frames| {
            let (l, c) = (layer.to_lowercase(), chart.to_lowercase());
            let says = |n: &str| n.contains("mouth") || n.contains("lips");
            let not_face = |n: &str| n.contains("head") || n.contains("face") || n.contains("base");
            frames >= buzz_audio::Viseme::COUNT && (says(&l) || says(&c)) && !not_face(&c) && !not_face(&l)
        },
        3,
    );
    if let Some((owner, layer)) = mouth
        && let Some(stack) = scene.library_mut().layers_mut(owner)
    {
        if stack.get(layer).is_some_and(|l| l.name != "Mouth") {
            stack.update(layer, |l| l.name = "Mouth".to_string());
            done.push("Mouth layer named".to_string());
        }
    }
    let lid = find_chart_layer(scene, head, &|layer, _, frames| layer.to_lowercase().contains("lid") && !layer.to_lowercase().contains("mask") && frames >= 2, 3);
    if let Some((owner, layer)) = lid {
        let first = scene.library().get(owner).and_then(|s| s.layers.get(layer)).and_then(|l| first_object(l).cloned());
        if let Some(object) = first
            && let Some(inst) = object.instance()
        {
            let mut chart = inst.symbol;
            let mut placed = object.transform;
            let mut ok = buzz_act::pose::lid_chart_frames(scene, chart).is_some();
            for _ in 0..3 {
                if ok {
                    break;
                }
                // A wrapper: one instance inside it is the chart.
                let Some(inner) = scene.library().get(chart) else { break };
                let instances: Vec<_> = inner.layers.iter().filter_map(|l| first_object(l).cloned()).filter(|o| o.instance().is_some()).collect();
                let [one] = instances.as_slice() else { break };
                placed = placed * one.transform;
                chart = one.instance().map(|i| i.symbol).unwrap_or(chart);
                ok = buzz_act::pose::lid_chart_frames(scene, chart).is_some();
            }
            if !ok {
                done.push("lid chart not readable".to_string());
            } else if let Some(frames) = buzz_act::pose::lid_chart_frames(scene, chart) {
                let already = chart == inst.symbol && inst.loop_mode == buzz_scene::LoopMode::SingleFrame && inst.first_frame == frames[0];
                if let Some(stack) = scene.library_mut().layers_mut(owner) {
                    stack.update(layer, |l| {
                        if !already {
                            let keys: Vec<u32> = l.frames.keyframes().iter().filter(|k| !k.objects.is_empty()).map(|k| k.start).collect();
                            for start in keys {
                                let mut o = (*object).clone();
                                o.transform = placed;
                                if let ObjectKind::Instance(i) = &mut o.kind {
                                    i.symbol = chart;
                                    i.loop_mode = buzz_scene::LoopMode::SingleFrame;
                                    i.first_frame = frames[0];
                                }
                                l.frames.set_objects(start, vec![Arc::new(o)]);
                            }
                        }
                        if l.name != "Eyelid" {
                            l.name = "Eyelid".to_string();
                        }
                    });
                    if !already {
                        done.push("Eyelid chart unwrapped onto its layer".to_string());
                    }
                }
            }
        }
    }
    done
}

/// The head symbols of a layer stack: what each non-folder layer shows first.
fn head_symbols(scene: &Scene, layers: &buzz_scene::LayerStack) -> Vec<SymbolId> {
    let mut out = Vec::new();
    for l in layers.iter().filter(|l| l.kind != buzz_scene::LayerKind::Folder) {
        if let Some(i) = first_object(l).and_then(|o| o.instance())
            && scene.library().get(i.symbol).is_some_and(|s| s.length() > 1)
            && !out.contains(&i.symbol)
        {
            out.push(i.symbol);
        }
    }
    out
}

/// [`tidy_head_symbol`] on every head symbol of a pose symbol; what was done, per head.
pub fn tidy_pose(scene: &mut Scene, pose: SymbolId) -> Vec<String> {
    let heads = scene.library().get(pose).map(|s| head_symbols(scene, &s.layers)).unwrap_or_default();
    let mut done = Vec::new();
    for head in heads {
        let name = scene.library().get(head).map(|s| s.name.clone()).unwrap_or_default();
        for d in tidy_head_symbol(scene, head) {
            done.push(format!("{name}: {d}"));
        }
        // The layer showing a head with a face is the rig's `head` part, whatever the FLA called it
        // (`old3`, `SideHEadBase`): only a layer named for a body part is a part of the rig.
        let has_face = find_chart_layer(scene, head, &|layer, _, _| layer == "Mouth" || layer == "Eyelid", 3).is_some();
        let layers = scene.library().get(pose).map(|s| s.layers.clone());
        if has_face
            && let Some(layers) = layers
            && !layers.iter().any(|l| buzz_rig::pattern::body_part(&l.name) == Some("Head"))
            && let Some(layer) = layers.iter().find(|l| l.kind != buzz_scene::LayerKind::Folder && first_object(l).and_then(|o| o.instance()).is_some_and(|i| i.symbol == head))
        {
            let from = layer.name.clone();
            if let Some(stack) = scene.library_mut().layers_mut(pose) {
                stack.update(layer.id, |l| l.name = "head".to_string());
            }
            done.push(format!("{name}: head layer {from:?} named head"));
        }
    }
    done
}

/// `name` with its "left" turned to "right" or the other way round, keeping the case of the first letter.
fn swap_side_word(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    for (from, to) in [("left", "right"), ("right", "left")] {
        if let Some(at) = lower.find(from) {
            let upper = name[at..].chars().next().is_some_and(|c| c.is_uppercase());
            let word = if upper { format!("{}{}", to[..1].to_uppercase(), &to[1..]) } else { to.to_string() };
            return Some(format!("{}{}{}", &name[..at], word, &name[at + from.len()..]));
        }
    }
    None
}

/// **Left/right duplicates told apart by position** (SIMPLE_EPISODE_FLOW_PLAN E4b): the FLA's
/// own names sometimes call both legs (or both arms) of a pose `frontLeftThigh`, so Rename
/// Body Layers leaves two layers with one name. Of such a pair, by where each stands against
/// the pose's body centre (unused: a pair is told apart by its own two x positions): in a Front, 3/4 or Side view the one on
/// the right of the screen is the character's left, in a Back view the one on the left is;
/// each gets its side's name, unless that name is already taken. Returns what was renamed.
pub fn fix_side_duplicates(scene: &mut Scene, pose: SymbolId, view: View) -> Vec<String> {
    let Some(symbol) = scene.library().get(pose) else { return Vec::new() };
    let stack = &symbol.layers;
    // (layer, name, x centre of its drawing at frame 0, area)
    let mut boxes: Vec<(buzz_scene::LayerId, String, f64, f64)> = Vec::new();
    for l in stack.iter().filter(|l| l.kind != buzz_scene::LayerKind::Folder) {
        let inherited = stack.inherited_transform(l.id, 0u32);
        let mut b: Option<Rect> = None;
        for o in l.objects_at(0).iter().filter(|o| o.visible) {
            let r = buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(o));
            if r.width() > 0.0 || r.height() > 0.0 {
                b = Some(b.map_or(r, |u| u.union(r)));
            }
        }
        if let Some(b) = b {
            boxes.push((l.id, l.name.clone(), b.center().x, b.width() * b.height()));
        }
    }
    if boxes.is_empty() {
        return Vec::new();
    }
    let mut renames: Vec<(buzz_scene::LayerId, String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (_, name, _, _) in &boxes {
        let key = name.to_lowercase();
        if seen.contains(&key) || !(key.contains("left") || key.contains("right")) {
            continue;
        }
        seen.push(key.clone());
        let same: Vec<&(buzz_scene::LayerId, String, f64, f64)> = boxes.iter().filter(|b| b.1.to_lowercase() == key).collect();
        let [a, b] = same.as_slice() else { continue };
        let Some(other) = swap_side_word(name) else { continue };
        if boxes.iter().any(|x| x.1.eq_ignore_ascii_case(&other)) {
            continue;
        }
        // Which of the two is the character's left, by x against the body centre.
        let (lo, hi) = if a.2 <= b.2 { (a, b) } else { (b, a) };
        if (hi.2 - lo.2).abs() < 1e-6 {
            continue;
        }
        let left_is_hi = view != View::Back;
        let (left, right) = if left_is_hi { (hi, lo) } else { (lo, hi) };
        let is_left_name = key.contains("left");
        let (left_name, right_name) = if is_left_name { (name.clone(), other.clone()) } else { (other.clone(), name.clone()) };
        for (layer, want) in [(left, left_name), (right, right_name)] {
            if layer.1 != want {
                renames.push((layer.0, layer.1.clone(), want));
            }
        }
    }
    let mut done = Vec::new();
    if let Some(stack) = scene.library_mut().layers_mut(pose) {
        for (id, from, to) in renames {
            if stack.get(id).is_some_and(|l| l.name == from) {
                stack.update(id, |l| l.name = to.clone());
                done.push(format!("{from}->{to}"));
            }
        }
    }
    done
}

/// What the readiness report says, with the eye charts.
fn readiness_text(library: &AssetLibrary, asset: &Asset) -> (crate::my_cast::Readiness, String) {
    let r = crate::my_cast::readiness(library, asset);
    let eyes = library
        .load(asset)
        .map(|s| s.library().iter().filter(|x| x.length() >= 2 && ["eye", "lid", "blink"].iter().any(|w| x.name.to_lowercase().contains(w))).count())
        .unwrap_or(0);
    let line = format!("{}; {eyes} eye/blink chart(s)", r.line());
    (r, line)
}

/// What was made for one asset.
#[derive(Debug, Clone)]
pub struct MadeAsset {
    pub name: String,
    pub path: String,
    pub view: View,
    pub walk: bool,
    pub source: String,
    pub frames: u32,
    pub readiness: String,
    pub mouth: bool,
    pub arms: bool,
    pub note: String,
}

/// One role's outcome.
#[derive(Debug, Clone, Default)]
pub struct RoleReport {
    pub role: String,
    pub wrapper: String,
    pub assets: Vec<MadeAsset>,
    pub missing: Vec<String>,
    /// CAST_POSES_PLAN C2: the poses of the one combined asset the separate ones were merged into.
    pub combined: Vec<String>,
}

/// Everything the import did.
#[derive(Debug, Clone, Default)]
pub struct CastFlaReport {
    pub roles: Vec<RoleReport>,
    pub backup: Option<PathBuf>,
    pub casting: Option<PathBuf>,
    pub choices: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

impl CastFlaReport {
    pub fn to_json(&self) -> Value {
        json!({
            "roles": self.roles.iter().map(|r| {
                let pose = |a: &&MadeAsset| !a.walk;
                let front = r.assets.iter().find(|a| !a.walk && a.view == View::Front);
                json!({
                    "role": r.role,
                    "source_symbol": r.wrapper,
                    "assets": r.assets.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
                    "views": r.assets.iter().filter(pose).map(|a| a.view.word()).collect::<Vec<_>>(),
                    "walks": r.assets.iter().filter(|a| a.walk).map(|a| a.name.clone()).collect::<Vec<_>>(),
                    "readiness": front.map(|a| a.readiness.clone()).unwrap_or_default(),
                    "per_asset": r.assets.iter().map(|a| json!({"name": a.name, "from": a.source, "frames": a.frames, "readiness": a.readiness, "note": a.note})).collect::<Vec<_>>(),
                    "combined_poses": r.combined,
                    "missing": r.missing.join("; "),
                })
            }).collect::<Vec<_>>(),
            "backup": self.backup.as_ref().map(|p| p.display().to_string()),
            "casting_json": self.casting.as_ref().map(|p| p.display().to_string()),
            "choices": self.choices,
            "notes": self.notes,
        })
    }

    /// One line for a status bar.
    pub fn summary(&self) -> String {
        let assets: usize = self.roles.iter().map(|r| r.assets.len()).sum();
        format!("Cast from FLA: {} role(s), {assets} asset(s); casting.json updated; backup {}", self.roles.len(), self.backup.as_ref().map(|p| p.display().to_string()).unwrap_or_default())
    }
}

/// What to import.
#[derive(Debug, Clone)]
pub struct CastFlaRequest {
    pub fla: PathBuf,
    /// The asset library root.
    pub root: PathBuf,
    pub series: String,
    /// Try the AI Layer Rig on a view whose mouth or arms were not found.
    pub ai_rig: bool,
    /// Only these roles (empty: all six).
    pub only: Vec<String>,
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut files = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            files += copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
            files += 1;
        }
    }
    Ok(files)
}

/// Copy the Cast folder (which holds the series' `casting.json`) to
/// `<root>/_cast_backup/<stamp>/Cast`. Nothing is written before this.
pub fn backup_cast(root: &Path) -> Result<PathBuf, String> {
    let stamp = crate::cast_fresh::backup_stamp();
    let mut dest = root.join(crate::cast_fresh::BACKUP_FOLDER).join(&stamp);
    let mut again = 2;
    while dest.exists() {
        dest = root.join(crate::cast_fresh::BACKUP_FOLDER).join(format!("{stamp}_{again}"));
        again += 1;
    }
    let cast = root.join(buzz_ui::CAST_FOLDER);
    if cast.is_dir() {
        copy_dir(&cast, &dest.join(buzz_ui::CAST_FOLDER)).map_err(|e| format!("could not back up the Cast folder: {e}"))?;
    } else {
        std::fs::create_dir_all(&dest).map_err(|e| format!("could not make the backup folder: {e}"))?;
    }
    Ok(dest)
}

fn load_stamp(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["assets"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
        .unwrap_or_default()
}

/// **Import the cast FLA.** See the module note.
pub fn import_cast_fla(req: &CastFlaRequest, say: &mut dyn FnMut(&str)) -> Result<CastFlaReport, String> {
    let mut report = CastFlaReport::default();
    if !req.fla.is_file() {
        return Err(format!("no cast FLA at {}", req.fla.display()));
    }
    say(&format!("reading {}\u{2026}", req.fla.display()));
    let imported = crate::import::read(&req.fla)?;
    let mut roles = find_roles(&imported.scene);
    if !req.only.is_empty() {
        roles.retain(|r| req.only.iter().any(|o| o.eq_ignore_ascii_case(r.role)));
    }
    if roles.is_empty() {
        return Err("no role wrappers (deepak, bhushan, mishra, radhe, padosi, padosi2) found on the FLA's stage".into());
    }
    for (_, role) in ROLES {
        if req.only.is_empty() && !roles.iter().any(|r| r.role == role) {
            report.notes.push(format!("{role}: no wrapper symbol on the stage"));
        }
    }

    // Backup first: nothing is written to the library before this.
    say("backing up the Cast folder\u{2026}");
    let backup = backup_cast(&req.root)?;
    report.backup = Some(backup);

    let folder = format!("{}/{}", buzz_ui::CAST_FOLDER, req.series);
    let stamp_path = req.root.join(buzz_ui::CAST_FOLDER).join(&req.series).join(STAMP_FILE);
    let mut ours = load_stamp(&stamp_path);
    let mut library = AssetLibrary::at(&req.root);

    for found in &roles {
        let mut rr = RoleReport { role: found.role.to_string(), wrapper: found.wrapper.clone(), ..Default::default() };
        // Holds first; a front with no pose is the first frame of the front walk.
        let mut jobs: Vec<(View, bool, Piece, bool)> = Vec::new();
        for ((view, walk), piece) in &found.chosen {
            jobs.push((*view, *walk, piece.clone(), !*walk));
        }
        if !found.chosen.contains_key(&(View::Front, false))
            && let Some(walk) = found.chosen.get(&(View::Front, true))
        {
            rr.missing.push(format!("Front pose: none in the FLA; made from frame 0 of {}", walk.name));
            jobs.push((View::Front, false, walk.clone(), true));
        }
        for view in [View::Front, View::Side, View::Back] {
            if !jobs.iter().any(|(v, w, _, _)| *v == view && !*w) {
                rr.missing.push(format!("{} view: not in the FLA", view.word()));
            }
        }
        if !found.chosen.contains_key(&(View::ThreeQuarter, false)) {
            rr.missing.push("3-4 view: not in the FLA".into());
        }
        if !found.extra.is_empty() {
            rr.missing.push(format!("not used (a same-view symbol with more layers was taken): {}", found.extra.join(", ")));
        }
        jobs.sort_by_key(|(v, w, _, _)| (*w, *v));
        for (view, walk, piece, hold) in jobs {
            let name = asset_name(found.role, view, walk);
            let clash = library.assets().iter().any(|a| a.folder == folder && a.name.eq_ignore_ascii_case(&name));
            if clash && !ours.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
                rr.missing.push(format!("{name}: already in {folder} and not from this import (kept)"));
                continue;
            }
            say(&format!("{}: {name} from {}\u{2026}", found.role, piece.name));
            let (scene, build_note) = build_scene(&imported.scene, &piece, &name, hold, false)?;
            let mut asset = library.save(&name, &folder, &scene).map_err(|e| format!("{name}: {e}"))?;
            ours.insert(name.clone());
            let (mut r, mut line) = readiness_text(&library, &asset);
            let plain_note = build_note;
            let mut note = plain_note.clone();
            if hold && req.ai_rig && view != View::Back && !(r.mouth && r.arms) {
                say(&format!("{name}: mouth/arms not found; trying the AI Layer Rig\u{2026}"));
                let (ai_scene, ai_note) = build_scene(&imported.scene, &piece, &name, true, true)?;
                library.save(&name, &folder, &ai_scene).map_err(|e| format!("{name}: {e}"))?;
                let again = library.assets().iter().find(|a| a.folder == folder && a.name == name).cloned().unwrap_or_else(|| asset.clone());
                let (r2, line2) = readiness_text(&library, &again);
                let score = |x: &crate::my_cast::Readiness| (x.mouth as u8) + (x.arms as u8);
                if score(&r2) > score(&r) {
                    note = format!("{ai_note}; kept (was: {line})");
                    (r, line, asset) = (r2, line2, again);
                } else {
                    library.save(&name, &folder, &scene).map_err(|e| format!("{name}: {e}"))?;
                    note = format!("{plain_note}; AI Layer Rig tried: {ai_note}; no better ({line2}); the plain lift kept");
                }
            }
            let frames = scene.library().get(piece.symbol).map(|s| s.length()).unwrap_or(1);
            rr.assets.push(MadeAsset {
                name: name.clone(),
                path: buzz_ai::asset_index::relative(&req.root, &asset.path),
                view,
                walk,
                source: piece.name.clone(),
                frames: if hold { 1 } else { frames },
                readiness: line.clone(),
                mouth: r.mouth,
                arms: r.arms,
                note,
            });
            if view == View::Front && !walk && !(r.mouth && r.arms) {
                rr.missing.push(format!("{name}: {line}"));
            }
        }
        // One character, all its poses: the separate assets are merged into `<ROLE>`.
        if rr.assets.len() > 1 {
            let stamp = format!("{}_fla", crate::cast_fresh::backup_stamp());
            match crate::cast_combine::combine_role(&mut library, &folder, found.role, &stamp) {
                Ok(done) => {
                    say(&format!("{}: {} poses combined into one asset", found.role, done.poses.len()));
                    rr.combined = done.poses;
                }
                Err(e) => rr.missing.push(format!("{}: poses not combined: {e}", found.role)),
            }
        }
        report.roles.push(rr);
    }

    // The list of what this import made, then casting.json.
    if let Some(dir) = stamp_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{e}"))?;
    }
    let _ = std::fs::write(&stamp_path, serde_json::to_string_pretty(&json!({"fla": req.fla.display().to_string(), "assets": ours})).unwrap_or_default());
    let mut file = crate::my_cast::load_casting(&req.root, &req.series);
    for rr in &report.roles {
        let Some(front) = rr.assets.iter().find(|a| !a.walk && a.view == View::Front) else {
            report.notes.push(format!("{}: no front asset, not cast", rr.role));
            continue;
        };
        let member = crate::my_cast::Member { name: rr.role.clone(), ..Default::default() };
        let views: Vec<&str> = rr.assets.iter().filter(|a| !a.walk).map(|a| a.view.word()).collect();
        file.choices.insert(
            member.key(),
            crate::my_cast::Choice {
                asset: front.path.clone(),
                name: front.name.clone(),
                locked: true,
                reason: format!("your cast.fla: {} (views {}; walks {} kept as Pose walk_*)", rr.wrapper, views.join("/"), rr.assets.iter().filter(|a| a.walk).count()),
                member: rr.role.clone(),
            },
        );
        report.choices.insert(rr.role.clone(), front.path.clone());
    }
    crate::my_cast::save_casting(&req.root, &file).map_err(|e| format!("could not write casting.json: {e}"))?;
    report.casting = Some(crate::my_cast::casting_path(&req.root, &req.series));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_into_views_and_walks() {
        let cases = [
            ("shirtFront", View::Front, false),
            ("shirtFrontWalk", View::Front, true),
            ("raviSideWalk", View::Side, true),
            ("SHIRTbACK2", View::Back, false),
            ("man1walkingBack", View::Back, true),
            ("bhagat1", View::Front, false),
            ("bhagat1Walking", View::Front, true),
            ("MazdoorBack3-4", View::ThreeQuarter, false),
            ("MazdoorBack34Walk", View::ThreeQuarter, true),
            ("oldManWalkBack", View::Back, true),
            ("rameshFrontPose", View::Front, false),
        ];
        for (name, view, walk) in cases {
            assert_eq!(classify(name), (view, walk), "{name}");
        }
    }

    #[test]
    fn asset_names_are_what_the_producer_reads() {
        assert_eq!(asset_name("DEEPAK", View::Front, false), "DEEPAK");
        assert_eq!(asset_name("DEEPAK", View::Side, false), "DEEPAK Side");
        assert_eq!(asset_name("DEEPAK", View::Back, false), "DEEPAK Back");
        assert_eq!(asset_name("RADHE", View::ThreeQuarter, false), "RADHE 3-4");
        assert_eq!(asset_name("DEEPAK", View::Side, true), "DEEPAK Pose walk_side");
        assert!(crate::cast_build::pose_of_rig(&asset_name("DEEPAK", View::Side, true)).is_some());
    }
}
