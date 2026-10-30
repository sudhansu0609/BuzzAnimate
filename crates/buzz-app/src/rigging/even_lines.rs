//! **Even Out Lines…** (`docs/EVEN_LINES_PLAN.md` E2): the filled ink shapes of
//! a scope — the selection (shapes, or instances → their symbols, recursively),
//! else the symbol being edited, else the whole stage — on **every keyframe of
//! every layer**, turned into even-width strokes by
//! [`buzz_scene::even_lines`]; face lines and body lines each at their own
//! width.
//!
//! Which shapes are ink: solidly filled, unstroked shapes on a layer named
//! "Ink" (any case). Only when the scope has no "Ink" layer at all (a plain
//! Trace Bitmap result), such shapes on other layers whose colour luma is at
//! most [`DARK_LUMA`] and that come out as line (a shape that yields only a
//! solid area is left alone) — E1b: dark colour art beside an Ink layer is
//! never touched. A shape is a face line when its layer, or any symbol
//! or layer on its path from the scope root, classifies as a face part
//! ([`super::head_rig::classify`]).
//!
//! Three steps, so the dialog can preview cheaply: [`plan_even_lines`] (find
//! the ink, measure it), [`trace_even_lines`] (the centrelines; only straighten
//! and solid-above need it again) and [`apply_even_lines`] (widths, written
//! into the scene from the plan's original keyframes, so it can be applied
//! again and again over its own preview).

use super::head_rig::{Part, classify, flatten_stack_placed};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_scene::even_lines::{EvenLines, EvenLinesOptions, even_lines_within, measured_line_width};
use buzz_scene::{Layer, LayerId, LayerStack, Object, ObjectId, ObjectKind, Paint, Scene, ShapeData, SymbolId};
use rayon::prelude::*;
use std::sync::Arc;

/// On a layer not named "Ink", a filled shape this dark or darker is ink.
pub const DARK_LUMA: f64 = 0.30;

/// Whose timeline a keyframe is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Stage,
    Symbol(SymbolId),
}

/// What Even Out Lines works on.
#[derive(Debug, Clone, Default)]
pub struct EvenLinesScope {
    /// These shapes on the timeline being edited (and nothing else of it).
    pub shapes: Vec<ObjectId>,
    /// These symbols, every keyframe of every layer, and the symbols inside.
    pub symbols: Vec<SymbolId>,
    /// The whole timeline being edited, and every symbol inside it.
    pub whole_timeline: bool,
}

/// The dialog's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvenLinesSettings {
    pub face_width: f64,
    pub body_width: f64,
    pub keep_weight: bool,
    pub straighten: f64,
    pub solid_above: f64,
}

/// What was done.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EvenLinesReport {
    pub face_lines: usize,
    pub body_lines: usize,
    pub solids: usize,
    pub face_width: f64,
    pub body_width: f64,
    /// E1e: ink kept as drawn (a line outside on both sides even in context,
    /// or a piece that yields neither a stroke nor a solid area).
    pub kept_as_drawn: usize,
}

/// Shapes traced together: every ink shape of an "Ink" layer's keyframe, or
/// one dark shape of another layer.
#[derive(Debug, Clone)]
struct SetJob {
    objects: Vec<ObjectId>,
    /// In the timeline's space (each object's transform applied).
    shapes: Vec<ShapeData>,
}

/// One keyframe with ink on it.
#[derive(Debug, Clone)]
struct KeyJob {
    owner: Owner,
    layer: LayerId,
    start: u32,
    face: bool,
    /// On a layer named "Ink" (else its sets are dark shapes of another layer).
    ink_layer: bool,
    original: Arc<Vec<Arc<Object>>>,
    sets: Vec<SetJob>,
    /// E1d: every filled shape of the timeline at this keyframe's time (all
    /// its layers), in the timeline's space: the character's silhouette.
    silhouette: Arc<Vec<ShapeData>>,
}

/// E1e: does this shape's fill count for the silhouette? "Out of the
/// character" is transparency, so only a fill more than half opaque does (a
/// faint shadow round a hand is not the character).
fn opaque_fill(shape: &ShapeData) -> bool {
    shape.fill.as_ref().is_some_and(|f| f.paint.color().to_rgba8().a > 127)
}

/// E1d: the filled shapes of every shown painting layer of `layers` at `frame`,
/// each object's transform applied.
fn silhouette_at(layers: &LayerStack, frame: u32) -> Vec<ShapeData> {
    let mut out = Vec::new();
    for layer in layers.iter() {
        if !layer.visible || !layer.kind.holds_artwork() || !layer.kind.paints_on_stage() {
            continue;
        }
        for object in layer.objects_at(frame) {
            if let ObjectKind::Shape(sd) = &object.kind
                && opaque_fill(sd)
            {
                out.push(ShapeData {
                    path: object.transform * sd.path.clone(),
                    ..sd.clone()
                });
            }
        }
    }
    out
}

/// E1e: **the character in context** — a scope root's whole drawing at rest
/// (every symbol inside flattened as the renderer places it, bone-rig parts
/// posed, masks applied), its filled shapes only, and where each timeline
/// inside is placed (its space → the root's).
struct Context {
    shapes: Vec<ShapeData>,
    boxes: Vec<Rect>,
    placed: Vec<(Owner, Affine)>,
}

fn context_of(scene: &Scene, owner: Owner, layers: &LayerStack) -> Context {
    let mut shapes = Vec::new();
    let mut placed = Vec::new();
    flatten_stack_placed(scene, layers, 0, Affine::IDENTITY, &[], &mut shapes, &mut placed, 0);
    shapes.retain(opaque_fill);
    let boxes = shapes.iter().map(|s| s.path.bounding_box()).collect();
    let mut out = vec![(owner, Affine::IDENTITY)];
    out.extend(placed.into_iter().map(|(id, a)| (Owner::Symbol(id), a)));
    Context { shapes, boxes, placed: out }
}

/// E1e: `key`'s silhouette in context: its own timeline's (E1d) and, through
/// every placement of its timeline in every root, the root's whole drawing
/// near the key's ink, mapped into the timeline's space (a symbol used twice
/// gets both contexts).
fn in_context(key: &KeyJob, contexts: &[Context]) -> Vec<ShapeData> {
    let mut out: Vec<ShapeData> = key.silhouette.as_ref().clone();
    let mut ink: Option<Rect> = None;
    for s in key.sets.iter().flat_map(|s| &s.shapes) {
        let b = s.path.bounding_box();
        ink = Some(ink.map_or(b, |i| i.union(b)));
    }
    let Some(ink) = ink else { return out };
    let margin = 0.1 * ink.width().max(ink.height()) + 1.0;
    let ink = ink.inflate(margin, margin);
    for c in contexts {
        for &(owner, place) in &c.placed {
            if owner != key.owner || place.determinant().abs() < 1e-12 {
                continue;
            }
            let back = place.inverse();
            let near = place.transform_rect_bbox(ink);
            for (shape, b) in c.shapes.iter().zip(&c.boxes) {
                if b.x1 < near.x0 || b.x0 > near.x1 || b.y1 < near.y0 || b.y0 > near.y1 {
                    continue;
                }
                out.push(ShapeData {
                    path: back * shape.path.clone(),
                    ..shape.clone()
                });
            }
        }
    }
    out
}

/// The ink of a scope, found and measured.
#[derive(Debug, Clone, Default)]
pub struct EvenLinesPlan {
    keys: Vec<KeyJob>,
    /// Median measured width of the face ink / body ink / all of it.
    pub face_measured: Option<f64>,
    pub body_measured: Option<f64>,
    pub ink_measured: Option<f64>,
    /// How many ink shapes the plan holds.
    pub ink_shapes: usize,
    /// How many ink shapes were left as drawn: filled forms clearly thicker
    /// than their set's lines (E1c b).
    pub kept_fills: usize,
}

impl EvenLinesPlan {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn has_face(&self) -> bool {
        self.keys.iter().any(|k| k.face)
    }
    pub fn has_body(&self) -> bool {
        self.keys.iter().any(|k| !k.face)
    }
}

/// Earlier results kept per set (E1b), at most this many.
const CACHE_PER_SET: usize = 8;

/// One set's result for the options that made it.
#[derive(Debug, Clone)]
struct Traced {
    straighten: f64,
    solid_above: f64,
    draft: bool,
    result: EvenLines,
}

impl Traced {
    /// Is this what tracing at these options would give? The same options, or
    /// both solid-above values at or over the set's deepest ink (no solid area
    /// either way).
    fn serves(&self, straighten: f64, solid_above: f64, draft: bool) -> bool {
        let deepest = self.result.deepest * (1.0 + 1e-9);
        self.draft == draft
            && self.straighten == straighten
            && (self.solid_above == solid_above || (self.solid_above >= deepest && solid_above >= deepest))
    }
}

/// The centrelines of a plan's ink, for one straighten / solid-above.
#[derive(Debug, Clone, Default)]
pub struct EvenLinesTrace {
    results: Vec<Vec<EvenLines>>,
    /// Every set's earlier results (newest last), so a set whose result cannot
    /// change, or a slider back at an earlier value, is not traced again.
    cache: Vec<Vec<Vec<Traced>>>,
    pub straighten: f64,
    pub solid_above: f64,
    pub draft: bool,
}

fn is_face(name: &str) -> bool {
    matches!(
        classify(name),
        Some(Part::Eye | Part::Lid | Part::Brow | Part::Moustache | Part::Mouth | Part::Nose | Part::Base)
    )
}

/// The colour of a solidly filled, unstroked shape.
fn ink_colour(shape: &ShapeData) -> Option<peniko::Color> {
    if shape.stroke.is_some() {
        return None;
    }
    match &shape.fill.as_ref()?.paint {
        Paint::Solid(c) => Some(*c),
        _ => None,
    }
}

fn luma(c: peniko::Color) -> f64 {
    let c = c.to_rgba8();
    (0.299 * c.r as f64 + 0.587 * c.g as f64 + 0.114 * c.b as f64) / 255.0
}

#[allow(clippy::too_many_arguments)]
fn walk(
    scene: &Scene,
    owner: Owner,
    layers: &LayerStack,
    only: Option<&[ObjectId]>,
    face: bool,
    recurse: bool,
    seen: &mut Vec<SymbolId>,
    keys: &mut Vec<KeyJob>,
    ink_seen: &mut bool,
) {
    // **Mouth shapes stay as drawn**: the lip-sync shapes of a mouth symbol
    // are open mouths whose dark inside is a filled form, not a line. (A
    // mouth drawn straight into a head's ink is not told from a line by any
    // width measure: Guddu's open mouth is 1.4× a line's width, as thick for
    // as long as an outline where lines meet. E1c b.)
    if let Owner::Symbol(id) = owner {
        if scene.library().get(id).is_some_and(|s| classify(&s.name) == Some(Part::Mouth)) {
            return;
        }
    }
    for layer in layers.iter() {
        if !layer.kind.holds_artwork() || layer.kind.is_mask() {
            continue;
        }
        let face = face || is_face(&layer.name);
        let ink_layer = layer.name.trim().eq_ignore_ascii_case("ink");
        if ink_layer && only.is_none() {
            *ink_seen = true;
        }
        for kf in layer.frames.keyframes() {
            let mut ink = SetJob { objects: Vec::new(), shapes: Vec::new() };
            let mut sets: Vec<SetJob> = Vec::new();
            for object in kf.objects.iter() {
                if only.is_some_and(|only| !only.contains(&object.id)) {
                    continue;
                }
                if ink_layer {
                    *ink_seen = true;
                }
                match &object.kind {
                    ObjectKind::Shape(sd) => {
                        let Some(colour) = ink_colour(sd) else { continue };
                        let shape = ShapeData {
                            path: object.transform * sd.path.clone(),
                            ..sd.clone()
                        };
                        if ink_layer {
                            ink.objects.push(object.id);
                            ink.shapes.push(shape);
                        } else if luma(colour) <= DARK_LUMA {
                            sets.push(SetJob { objects: vec![object.id], shapes: vec![shape] });
                        }
                    }
                    _ if recurse => {
                        for symbol in nested_symbols(object) {
                            if seen.contains(&symbol) {
                                continue;
                            }
                            seen.push(symbol);
                            if let Some(s) = scene.library().get(symbol) {
                                let face = face || is_face(&s.name);
                                walk(scene, Owner::Symbol(symbol), &s.layers, None, face, true, seen, keys, ink_seen);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !ink.objects.is_empty() {
                sets.insert(0, ink);
            }
            if !sets.is_empty() {
                keys.push(KeyJob {
                    owner,
                    layer: layer.id,
                    start: kf.start,
                    face,
                    ink_layer,
                    original: kf.objects.clone(),
                    sets,
                    silhouette: Arc::new(silhouette_at(layers, kf.start)),
                });
            }
        }
    }
}

/// The symbols an object shows: an instance's own, and those of the
/// instances inside a group or a bone rig's parts, in order.
pub fn nested_symbols(object: &Object) -> Vec<SymbolId> {
    let mut out = Vec::new();
    fn visit(object: &Object, out: &mut Vec<SymbolId>) {
        match &object.kind {
            ObjectKind::Instance(i) => {
                if !out.contains(&i.symbol) {
                    out.push(i.symbol);
                }
            }
            ObjectKind::Group(children) => children.iter().for_each(|c| visit(c, out)),
            ObjectKind::Armature(rig) => rig.parts.iter().for_each(|p| visit(&p.artwork, out)),
            _ => {}
        }
    }
    visit(object, &mut out);
    out
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

/// **Find and measure the ink of `scope`** (see the module doc).
/// A shape whose own median width is over this many times its set's is a
/// filled form, not a line (E1c b).
const FILLED_FORM_RATIO: f64 = 1.8;

/// **Filled forms stay as drawn** (`docs/EVEN_LINES_PLAN.md` E1c b): the
/// shapes of `set` whose own median width is over [`FILLED_FORM_RATIO`] × the
/// set's line width (a nostril, a dark patch) leave the set, so the originals
/// are kept untouched. How many left. (A mouth is not caught by any width
/// measure; see `walk`.)
fn keep_filled_forms(set: &mut SetJob) -> usize {
    if set.shapes.len() < 2 {
        return 0;
    }
    let Some(line) = measured_line_width(&set.shapes) else {
        return 0;
    };
    let filled: Vec<bool> = set
        .shapes
        .par_iter()
        .map(|s| measured_line_width(std::slice::from_ref(s)).is_some_and(|m| m > FILLED_FORM_RATIO * line))
        .collect();
    let n = filled.iter().filter(|&&f| f).count();
    if n > 0 {
        let objects = std::mem::take(&mut set.objects);
        let shapes = std::mem::take(&mut set.shapes);
        for ((object, shape), filled) in objects.into_iter().zip(shapes).zip(filled) {
            if !filled {
                set.objects.push(object);
                set.shapes.push(shape);
            }
        }
    }
    n
}

pub fn plan_even_lines(scene: &Scene, scope: &EvenLinesScope) -> EvenLinesPlan {
    let mut keys = Vec::new();
    let mut seen: Vec<SymbolId> = Vec::new();
    // The timeline being edited, and the names on the way into it.
    let current = match scene.editing_symbol() {
        Some(id) => Owner::Symbol(id),
        None => Owner::Stage,
    };
    let path_face = scene
        .edit_path()
        .iter()
        .any(|id| scene.library().get(*id).is_some_and(|s| is_face(&s.name)));
    let mut ink_seen = false;
    if let Owner::Symbol(id) = current {
        seen.push(id);
    }
    // E1e: the roots the scope is seen in (the timeline being edited, and
    // each symbol of the scope).
    let mut contexts: Vec<Context> = Vec::new();
    if scope.whole_timeline || !scope.shapes.is_empty() {
        contexts.push(context_of(scene, current, scene.layers()));
    }
    for &symbol in &scope.symbols {
        if let Some(s) = scene.library().get(symbol) {
            contexts.push(context_of(scene, Owner::Symbol(symbol), &s.layers));
        }
    }
    if scope.whole_timeline {
        walk(scene, current, scene.layers(), None, path_face, true, &mut seen, &mut keys, &mut ink_seen);
    } else if !scope.shapes.is_empty() {
        let only = Some(&scope.shapes[..]);
        walk(scene, current, scene.layers(), only, path_face, false, &mut seen, &mut keys, &mut ink_seen);
    }
    for &symbol in &scope.symbols {
        if seen.contains(&symbol) {
            continue;
        }
        seen.push(symbol);
        if let Some(s) = scene.library().get(symbol) {
            let face = is_face(&s.name);
            walk(scene, Owner::Symbol(symbol), &s.layers, None, face, true, &mut seen, &mut keys, &mut ink_seen);
        }
    }
    // E1b: with an "Ink" layer anywhere in scope, only Ink layers are ink.
    if ink_seen {
        keys.retain(|k| k.ink_layer);
    }
    // E1c (b): filled forms stay as drawn.
    let kept_fills: usize = keys
        .par_iter_mut()
        .map(|k| {
            let n: usize = k.sets.par_iter_mut().map(keep_filled_forms).sum();
            k.sets.retain(|s| !s.objects.is_empty());
            n
        })
        .sum();
    keys.retain(|k| !k.sets.is_empty());
    // E1e: every key's silhouette seen in context.
    keys.par_iter_mut().for_each(|k| k.silhouette = Arc::new(in_context(k, &contexts)));

    let measured: Vec<(bool, Option<f64>)> = keys
        .par_iter()
        .flat_map_iter(|k| k.sets.iter().map(move |s| (k.face, s)))
        .map(|(face, s)| (face, measured_line_width(&s.shapes)))
        .collect();
    let pick = |want: Option<bool>| {
        median(
            measured
                .iter()
                .filter(|(f, _)| want.is_none_or(|w| *f == w))
                .filter_map(|(_, m)| *m)
                .collect(),
        )
    };
    EvenLinesPlan {
        face_measured: pick(Some(true)),
        body_measured: pick(Some(false)),
        ink_measured: pick(None),
        ink_shapes: keys.iter().flat_map(|k| &k.sets).map(|s| s.objects.len()).sum(),
        kept_fills,
        keys,
    }
}

/// The default "Solid above" for a plan: ink deeper than this radius stays a
/// filled area — 1.25 × the measured median width, the tracer's own line /
/// mass split (a mass is over 2.5 × the typical pen width across).
pub fn default_solid_above(plan: &EvenLinesPlan) -> f64 {
    plan.ink_measured.map_or(8.0, |m| 1.25 * m)
}

/// **The centrelines of a plan's ink** (in parallel; the order is the plan's).
pub fn trace_even_lines(plan: &EvenLinesPlan, straighten: f64, solid_above: f64, draft: bool) -> EvenLinesTrace {
    retrace_even_lines(plan, EvenLinesTrace::default(), straighten, solid_above, draft)
}

/// [`trace_even_lines`] again for new options, over `previous` (a trace of the
/// same plan): only the sets whose result can change are traced (E1b), the
/// rest come from `previous`'s cache.
pub fn retrace_even_lines(
    plan: &EvenLinesPlan,
    previous: EvenLinesTrace,
    straighten: f64,
    solid_above: f64,
    draft: bool,
) -> EvenLinesTrace {
    let options = EvenLinesOptions {
        width: 1.0,
        keep_weight: false,
        straighten,
        solid_above,
        draft,
    };
    let mut cache = previous.cache;
    cache.resize_with(plan.keys.len(), Vec::new);
    let done: Vec<Vec<(EvenLines, Vec<Traced>)>> = plan
        .keys
        .par_iter()
        .zip(cache.into_par_iter())
        .map(|(k, mut sets)| {
            sets.resize_with(k.sets.len(), Vec::new);
            k.sets
                .par_iter()
                .zip(sets.into_par_iter())
                .map(|(s, mut kept)| {
                    if let Some(t) = kept.iter().rev().find(|t| t.serves(straighten, solid_above, draft)) {
                        return (t.result.clone(), kept);
                    }
                    let result = even_lines_within(&s.shapes, &k.silhouette, &options);
                    kept.push(Traced { straighten, solid_above, draft, result: result.clone() });
                    if kept.len() > CACHE_PER_SET {
                        kept.remove(0);
                    }
                    (result, kept)
                })
                .collect()
        })
        .collect();
    let (results, cache) = done.into_iter().map(|sets| sets.into_iter().unzip()).unzip();
    EvenLinesTrace { results, cache, straighten, solid_above, draft }
}

/// **Write a traced plan into `scene`** at `settings`' widths: each set that
/// came out as line is replaced by its strokes (and solid areas), in the paint
/// order slot of its first shape; every other object is the plan's original.
pub fn apply_even_lines(
    scene: &mut Scene,
    plan: &EvenLinesPlan,
    trace: &mut EvenLinesTrace,
    settings: &EvenLinesSettings,
) -> EvenLinesReport {
    let mut report = EvenLinesReport {
        face_width: settings.face_width,
        body_width: settings.body_width,
        ..Default::default()
    };
    for (key, results) in plan.keys.iter().zip(trace.results.iter_mut()) {
        let width = if key.face { settings.face_width } else { settings.body_width };
        // Which set each replaced object belongs to.
        let mut member: Vec<(ObjectId, usize)> = Vec::new();
        for (index, (set, result)) in key.sets.iter().zip(results.iter_mut()).enumerate() {
            if result.strokes == 0 {
                continue;
            }
            result.set_widths(width, settings.keep_weight);
            if key.face {
                report.face_lines += result.strokes;
            } else {
                report.body_lines += result.strokes;
            }
            report.solids += result.solids - result.kept;
            report.kept_as_drawn += result.kept;
            member.extend(set.objects.iter().map(|&id| (id, index)));
        }
        let mut placed = vec![false; key.sets.len()];
        let mut out: Vec<Arc<Object>> = Vec::with_capacity(key.original.len());
        for object in key.original.iter() {
            match member.iter().find(|(id, _)| *id == object.id) {
                Some(&(_, index)) => {
                    if !placed[index] {
                        placed[index] = true;
                        for shape in &results[index].shapes {
                            let id = scene.next_object_id();
                            out.push(Arc::new(Object::shape(id, shape.clone())));
                        }
                    }
                }
                None => out.push(object.clone()),
            }
        }
        let start = key.start;
        let write = move |layer: &mut Layer| {
            if let Some(kf) = layer.frames.keyframes_mut().iter_mut().find(|kf| kf.start == start) {
                kf.objects = Arc::new(out);
            }
        };
        match key.owner {
            Owner::Stage => {
                scene.update_stage_layer(key.layer, write);
            }
            Owner::Symbol(symbol) => {
                scene.library_mut().update(symbol, |s| {
                    s.layers.update(key.layer, write);
                });
            }
        }
    }
    report
}
