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
use buzz_geom::{Affine, BezPath, Rect, Shape as _};
use buzz_scene::even_lines::{EvenLines, EvenLinesOptions, even_lines_within_boxed, measured_line_width, Coverage};
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
    /// T15: **bold outlines** like the FLA files: every ink line one width in
    /// black (`body_width`, the plan's [`EvenLinesPlan::bold_width`] to start
    /// with), a short one at half of it, and on a picture an outline wherever
    /// two neighbouring colours differ.
    pub bold: bool,
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
    /// The measured line width of `shapes` (set by the plan).
    measured: Option<f64>,
    /// The box of `shapes` (set by the plan).
    bounds: Option<Rect>,
    /// 2 x area / perimeter of `shapes`: a ribbon's width, a blob's bulk (set
    /// by the plan; a ridge median alone calls a jagged crescent a line).
    thickness: f64,
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
    /// The silhouette shapes' bounding boxes (measured once, not per set).
    silhouette_boxes: Arc<Vec<Rect>>,
    /// Where the silhouette covers, to leave it out for ink deep inside it.
    coverage: Option<Arc<Coverage>>,
    /// T12c: the shapes beside the ink with a stroke that traps under it.
    trapped: Arc<std::collections::HashMap<ObjectId, Arc<Object>>>,
    /// T15: the outlines between colour regions of this keyframe's picture, at
    /// the plan's bold width (found the first time bold outlines are asked for).
    edges: Arc<std::sync::OnceLock<Arc<Vec<ShapeData>>>>,
}

/// T15: the colour difference (CIELAB delta E) that draws an outline between two regions.
const BOLD_CONTRAST: f64 = 18.0;

impl KeyJob {
    /// T15: the outline strokes between the colour regions of this keyframe's
    /// shapes at `width` (the ink's own pixels painted over from the colour
    /// beside them), found once. Nothing on an "Ink" layer: a character's
    /// colours are lined by its ink.
    fn bold_edges(&self, width: f64) -> Arc<Vec<ShapeData>> {
        self.edges
            .get_or_init(|| {
                if self.ink_layer {
                    return Arc::new(Vec::new());
                }
                let members: std::collections::HashSet<ObjectId> = self.sets.iter().flat_map(|s| s.objects.iter().copied()).collect();
                let fills: Vec<ShapeData> = self
                    .original
                    .iter()
                    .filter(|o| !members.contains(&o.id))
                    .filter_map(|o| match &o.kind {
                        ObjectKind::Shape(sd) if opaque_fill(sd) => Some(ShapeData { path: o.transform * sd.path.clone(), ..sd.clone() }),
                        _ => None,
                    })
                    .collect();
                let ink: Vec<ShapeData> = self.sets.iter().flat_map(|s| s.shapes.iter().cloned()).collect();
                let bounds = fills.iter().chain(&ink).map(|s| s.path.bounding_box()).reduce(|a, b| a.union(b));
                let Some(bounds) = bounds else { return Arc::new(Vec::new()) };
                Arc::new(buzz_scene::trace_bold::edge_strokes(&fills, &ink, bounds, width, buzz_scene::trace_bold::bold_colour(), BOLD_CONTRAST))
            })
            .clone()
    }
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
    /// T15: the bold line width for this scope's picture: 3 px per 1920 px of
    /// the longest side of its shapes.
    pub bold_width: f64,
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
    /// T15: find the outlines between the picture's colour regions now (they
    /// are found once, on the first Bold outlines preview otherwise).
    pub fn prepare_bold(&self) {
        self.keys.par_iter().for_each(|k| {
            k.bold_edges(self.bold_width);
        });
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
        // A layer name only says "face" inside a character's symbol: on the
        // stage a layer called "Base" or "Lid" is a picture's, not a head's.
        let face = face || (matches!(owner, Owner::Symbol(_)) && is_face(&layer.name));
        let ink_layer = layer.name.trim().eq_ignore_ascii_case("ink");
        if ink_layer && only.is_none() {
            *ink_seen = true;
        }
        for kf in layer.frames.keyframes() {
            let mut ink = SetJob { objects: Vec::new(), shapes: Vec::new(), measured: None, bounds: None, thickness: 0.0 };
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
                            sets.push(SetJob { objects: vec![object.id], shapes: vec![shape], measured: None, bounds: None, thickness: 0.0 });
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
                    silhouette_boxes: Arc::new(Vec::new()),
                    coverage: None,
                    trapped: Arc::default(),
                    edges: Arc::default(),
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
    let mut kept_fills: usize = keys
        .par_iter_mut()
        .map(|k| {
            let n: usize = k.sets.par_iter_mut().map(keep_filled_forms).sum();
            k.sets.retain(|s| !s.objects.is_empty());
            n
        })
        .sum();
    keys.retain(|k| !k.sets.is_empty());
    // E1e: every key's silhouette seen in context.
    keys.par_iter_mut().for_each(|k| {
        k.silhouette = Arc::new(in_context(k, &contexts));
        k.silhouette_boxes = Arc::new(k.silhouette.par_iter().map(|s| s.path.bounding_box()).collect());
        k.coverage = Coverage::new(&k.silhouette, &k.silhouette_boxes).map(Arc::new);
    });

    keys.par_iter_mut().for_each(|k| {
        k.sets.par_iter_mut().for_each(|s| {
            s.measured = measured_line_width(&s.shapes);
            s.bounds = s.shapes.iter().map(|sd| sd.path.bounding_box()).reduce(|a, b| a.union(b));
            let area: f64 = s.shapes.iter().map(|sd| sd.path.area().abs()).sum();
            let perimeter: f64 = s.shapes.iter().map(|sd| sd.path.perimeter(0.25)).sum();
            s.thickness = if perimeter > 0.0 { 2.0 * area / perimeter } else { 0.0 };
        });
    });
    // T12c: with no Ink layer every dark shape is a set of its own, and a big
    // dark region (a building, a doorway) is a filled form, not a line: one
    // whose width is over [`FILLED_FORM_RATIO`] x the picture's typical line
    // stays as drawn (E1c b measured against the whole scope, not its set).
    if !keys.iter().any(|k| k.ink_layer) {
        let line = median(keys.iter().flat_map(|k| k.sets.iter().filter_map(|s| s.measured)).collect());
        if let Some(line) = line {
            for k in keys.iter_mut() {
                let before = k.sets.len();
                k.sets.retain(|s| s.measured.is_none_or(|m| m <= FILLED_FORM_RATIO * line) && s.thickness <= FILLED_FORM_RATIO * line);
                kept_fills += before - k.sets.len();
            }
            keys.retain(|k| !k.sets.is_empty());
        }
    }
    // T12c: on a plain trace nothing lies under the ink, so a line thinned or
    // moved shows the paper. Every solid shape beside the ink gets a stroke of
    // its own colour as wide as the ink (it grows half that under the ink: a
    // trap), so what shows is the neighbour's own colour.
    keys.par_iter_mut().filter(|k| !k.ink_layer).for_each(|k| {
        let mut widths: Vec<f64> = k.sets.iter().filter_map(|s| s.measured).collect();
        widths.sort_by(|a, b| a.total_cmp(b));
        let Some(&width) = widths.get(widths.len() / 2) else { return };
        let width = width.clamp(0.5, 3.0);
        let members: std::collections::HashSet<ObjectId> = k.sets.iter().flat_map(|s| s.objects.iter().copied()).collect();
        let trapped = k
            .original
            .iter()
            .filter(|o| !members.contains(&o.id))
            .filter_map(|o| {
                let ObjectKind::Shape(sd) = &o.kind else { return None };
                if !opaque_fill(sd) || sd.stroke.is_some() {
                    return None;
                }
                let colour = ink_colour(sd)?;
                let mut object = (**o).clone();
                if let ObjectKind::Shape(sd) = &mut object.kind {
                    sd.stroke = Some(buzz_scene::StrokeSpec::new(colour, width));
                }
                Some((o.id, Arc::new(object)))
            })
            .collect();
        k.trapped = Arc::new(trapped);
    });
    let measured: Vec<(bool, Option<f64>)> = keys
        .iter()
        .flat_map(|k| k.sets.iter().map(move |s| (k.face, s.measured)))
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
    let long_edge = keys
        .iter()
        .flat_map(|k| k.silhouette_boxes.iter().copied())
        .reduce(|a, b| a.union(b))
        .map_or(1920.0, |b| b.width().max(b.height()));
    EvenLinesPlan {
        bold_width: buzz_scene::trace_bold::bold_width(long_edge),
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
                    // Ink with the silhouette well all round it has no edge to
                    // be measured against: the silhouette is left out.
                    let deep = s.bounds.zip(k.coverage.as_ref()).is_some_and(|(b, c)| {
                        c.covers(b, 4.0 * s.measured.unwrap_or(2.0) + 1.0)
                    });
                    let result = if deep {
                        even_lines_within_boxed(&s.shapes, &[], None, s.measured, &options)
                    } else {
                        even_lines_within_boxed(&s.shapes, &k.silhouette, Some(&k.silhouette_boxes), s.measured, &options)
                    };
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

/// T12c: do `result`'s strokes and solid areas cover at least [`MIN_COVER`] of
/// the ink they replace? On a plain trace a dark region wider than a line
/// comes out as a few thin strokes and a hole; that ink stays as drawn.
fn covers_ink(set: &SetJob, result: &EvenLines) -> bool {
    let ink: f64 = set.shapes.iter().map(|s| s.path.area().abs()).sum();
    if ink <= 0.0 {
        return true;
    }
    let got: f64 = result
        .shapes
        .iter()
        .map(|s| match &s.stroke {
            Some(stroke) => s.path.perimeter(0.5) * stroke.width,
            None => s.path.area().abs(),
        })
        .sum();
    got >= MIN_COVER * ink
}

/// The share of its ink a replaced set must cover (T12c).
const MIN_COVER: f64 = 0.4;

/// T15: a bold line: black, `width` wide, or the detail weight when a stub.
fn bold_stroke(shape: &mut ShapeData, width: f64) {
    if let Some(stroke) = shape.stroke.as_mut() {
        let length = buzz_scene::trace_bold::path_length(&shape.path);
        // Only a stub is detail: the plate's brick dashes are its ink.
        stroke.width = if length < 4.0 * width { buzz_scene::trace_bold::detail_width(width) } else { width };
        stroke.paint = Paint::Solid(buzz_scene::trace_bold::bold_colour());
    }
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
        let mut member: std::collections::HashMap<ObjectId, usize> = std::collections::HashMap::new();
        for (index, (set, result)) in key.sets.iter().zip(results.iter_mut()).enumerate() {
            if result.strokes == 0 {
                continue;
            }
            if settings.bold {
                result.set_widths_centred(width, false);
            } else {
                result.set_widths(width, settings.keep_weight);
            }
            if !key.ink_layer && !covers_ink(set, result) {
                continue;
            }
            if key.face {
                report.face_lines += result.strokes;
            } else {
                report.body_lines += result.strokes;
            }
            report.solids += result.solids - result.kept;
            report.kept_as_drawn += result.kept;
            member.extend(set.objects.iter().map(|&id| (id, index)));
        }
        // The trap only matters once some set is replaced by strokes.
        let none = std::collections::HashMap::new();
        let trapped = if member.is_empty() { &none } else { &*key.trapped };
        let mut placed = vec![false; key.sets.len()];
        let mut out: Vec<Arc<Object>> = Vec::with_capacity(key.original.len());
        for object in key.original.iter() {
            match member.get(&object.id) {
                Some(&index) => {
                    if !placed[index] {
                        placed[index] = true;
                        for shape in &results[index].shapes {
                            let id = scene.next_object_id();
                            let mut shape = shape.clone();
                            if settings.bold {
                                bold_stroke(&mut shape, width);
                            }
                            out.push(Arc::new(Object::shape(id, shape)));
                        }
                    }
                }
                None => out.push(trapped.get(&object.id).unwrap_or(object).clone()),
            }
        }
        // T15: the outlines between colour regions, on top of the ink's lines.
        if settings.bold && !member.is_empty() && !key.ink_layer {
            let found = key.bold_edges(plan.bold_width);
            let ratio = width / plan.bold_width.max(1e-9);
            for edge in found.iter() {
                let id = scene.next_object_id();
                let mut edge = edge.clone();
                if let Some(stroke) = edge.stroke.as_mut() {
                    stroke.width *= ratio;
                }
                out.push(Arc::new(Object::shape(id, edge)));
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
