//! **Remove Background Specks** (A6b of `docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md`):
//! a character traced before A6a carries the sheet's paper, trapped between
//! hair curls and body-hair strands, as small white or cream fills near its
//! outline. This takes them out of existing art without a re-trace: on every
//! keyframe of every layer of the scope (the selection: shapes, or instances →
//! their symbols and every symbol inside; else the timeline being edited), each
//! fill piece that [`buzz_scene::background_specks`] finds is a speck of paper
//! is deleted (or, for a shape with other pieces, cut out of it). Everything
//! else, hand edits included, is left as it is. A6d: the same goes for the
//! thin fills coloured between the outline's ink and the paper along the
//! outside (the source's anti-aliasing, grey dashes on hair tips); where such a
//! fill is part of a shape that shows elsewhere too, that rim is cut out of it.
//!
//! The picture judged is the scope root drawn at rest (frame 0, symbols at the
//! frame they rest on); a keyframe not shown at rest is judged drawn over that
//! picture in place of its layer's rest keyframe, at every place its timeline
//! is shown. Eyes, eyelids and mouths (layers, symbols or objects named so)
//! are never touched.

use super::EvenLinesScope;
use super::head_rig::{Part, classify};
use buzz_geom::{Affine, BezPath};
use buzz_scene::background_specks::{SpeckOptions, SpeckShape, find_background_specks, without_subpaths};
use buzz_scene::{Layer, LayerId, LayerKind, LayerStack, Object, ObjectId, ObjectKind, Scene, ShapeData, SymbolId};
use std::sync::Arc;

/// Whose timeline a keyframe is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Stage,
    Symbol(SymbolId),
}

/// Where a flattened shape came from: a shape on a keyframe, on its own or
/// inside a group (A6e: a Cast layer rig's parts are groups of shapes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag {
    owner: Owner,
    layer: LayerId,
    start: u32,
    /// The object on the keyframe (the shape, or the group holding it).
    object: ObjectId,
    /// The shape itself.
    shape: ObjectId,
}

/// The keyframe a flattened object is on, and the object on it that holds it.
type Key = (Owner, LayerId, u32, ObjectId);

/// Whether an object is, or a group (at any depth) holds, a shape.
fn holds_shape(object: &Object) -> bool {
    match &object.kind {
        ObjectKind::Shape(_) => true,
        ObjectKind::Group(children) => children.iter().any(|c| holds_shape(c)),
        _ => false,
    }
}

/// A timeline drawn flat.
#[derive(Default)]
struct Flat {
    shapes: Vec<SpeckShape>,
    tags: Vec<Option<Tag>>,
    /// Each shape's own space → the picture's.
    worlds: Vec<Affine>,
    /// Every timeline shown inside, and where (its space → the root's).
    placed: Vec<(Owner, Affine)>,
    /// The keyframes shown: (owner, layer, start).
    shown: Vec<(Owner, LayerId, u32)>,
}

/// What Remove Background Specks did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpecksReport {
    /// Shapes deleted whole.
    pub shapes_removed: usize,
    /// Shapes that lost some pieces.
    pub shapes_clipped: usize,
    /// Square units taken out (as seen).
    pub area: f64,
    /// Keyframes changed.
    pub keyframes: usize,
    /// The paper colour used (`None`: none given and none found).
    pub colour: Option<[u8; 3]>,
    /// Keyframes not shown anywhere at rest (a timeline never placed), so not judged.
    pub keyframes_not_judged: usize,
}

fn is_protected_name(name: &str) -> bool {
    matches!(classify(name), Some(Part::Eye | Part::Lid | Part::Mouth))
}

fn transformed(shape: &ShapeData, world: Affine) -> ShapeData {
    let mut out = shape.clone();
    out.path = world * shape.path.clone();
    if let Some(stroke) = &mut out.stroke {
        if !stroke.hairline {
            stroke.width *= world.determinant().abs().sqrt();
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn flatten_object(
    scene: &Scene,
    object: &Object,
    key: Option<Key>,
    parent: Affine,
    protected: bool,
    depth: usize,
    out: &mut Flat,
) {
    if !object.visible || depth > 12 {
        return;
    }
    let world = parent * object.transform;
    let protected = protected || object.name.as_deref().is_some_and(is_protected_name);
    match &object.kind {
        ObjectKind::Shape(s) => {
            out.shapes.push(SpeckShape { shape: transformed(s, world), protected });
            out.tags.push(key.map(|(owner, layer, start, top)| Tag { owner, layer, start, object: top, shape: object.id }));
            out.worlds.push(world);
        }
        ObjectKind::Group(children) => {
            for c in children {
                flatten_object(scene, c, key, world, protected, depth + 1, out);
            }
        }
        ObjectKind::Instance(i) => {
            let Some(symbol) = scene.library().get(i.symbol) else { return };
            out.placed.push((Owner::Symbol(i.symbol), world));
            let frame = i.resolve_frame(symbol.kind, 0, symbol.length());
            let protected = protected || is_protected_name(&symbol.name);
            flatten_stack(scene, Owner::Symbol(i.symbol), &symbol.layers, frame, world, protected, depth + 1, out);
        }
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                flatten_object(scene, &part, None, world, protected, depth + 1, out);
            }
        }
        ObjectKind::Warp(w) => {
            out.shapes.push(SpeckShape { shape: transformed(&w.warped(), world), protected });
            out.tags.push(None);
            out.worlds.push(world);
        }
    }
}

fn paints(layer: &Layer) -> bool {
    !matches!(layer.kind, LayerKind::Folder | LayerKind::Guide | LayerKind::Mask | LayerKind::InverseMask)
}

#[allow(clippy::too_many_arguments)]
fn flatten_stack(
    scene: &Scene,
    owner: Owner,
    stack: &LayerStack,
    frame: u32,
    world: Affine,
    protected: bool,
    depth: usize,
    out: &mut Flat,
) {
    for layer in stack.paint_order() {
        if !paints(layer) || !layer.is_drawable_at(frame) || !stack.is_effectively_visible(layer.id) {
            continue;
        }
        let Some(start) = layer.frames.keyframe_start(frame) else { continue };
        out.shown.push((owner, layer.id, start));
        let protected = protected || is_protected_name(&layer.name);
        for o in layer.objects_at(frame) {
            flatten_object(scene, o, Some((owner, layer.id, start, o.id)), world, protected, depth, out);
        }
    }
}

/// The layers of a timeline.
fn stack_of<'a>(scene: &'a Scene, owner: Owner) -> Option<&'a LayerStack> {
    match owner {
        Owner::Stage => Some(scene.stage_layers()),
        Owner::Symbol(id) => scene.library().get(id).map(|s| &s.layers),
    }
}

/// Whether a layer of `owner` (or the owner itself, or its symbol name) is
/// an eye / lid / mouth part, seen from the timeline alone.
fn layer_protected(scene: &Scene, owner: Owner, layer: &Layer) -> bool {
    is_protected_name(&layer.name)
        || matches!(owner, Owner::Symbol(id) if scene.library().get(id).is_some_and(|s| is_protected_name(&s.name)))
}

/// A6d: what two judgements both cut out of a shape (its own space).
fn both_clip(a: Option<BezPath>, b: &Option<BezPath>) -> Option<BezPath> {
    let (a, b) = (a?, b.as_ref()?);
    let opts = buzz_geom::boolean::options_for(&[a.clone(), b.clone()]);
    let c = buzz_geom::boolean(&a, b, buzz_geom::BoolOp::Intersect, opts);
    (!c.elements().is_empty()).then_some(c)
}

/// Cuts per tag: the subpaths to drop, in every judgement that saw the shape
/// (intersected), the area, and (A6d) the region to cut out of it in its own
/// space (intersected likewise).
#[derive(Default)]
struct Verdicts {
    seen: Vec<Verdict>,
}

impl Verdicts {
    /// One judgement of a flattened picture: `cuts` from the core, over `tags`
    /// (`worlds`: each shape's space → the picture's).
    fn add(
        &mut self,
        tags: &[Option<Tag>],
        worlds: &[Affine],
        cuts: &[buzz_scene::background_specks::SpeckCut],
        only: &dyn Fn(&Tag) -> bool,
    ) {
        let mut per: Vec<(Tag, Vec<usize>, f64, Option<BezPath>)> = Vec::new();
        for (k, tag) in tags.iter().enumerate() {
            let Some(tag) = tag else { continue };
            if !only(tag) {
                continue;
            }
            let (drop, area, clip) = match cuts.iter().find(|c| c.shape == k) {
                Some(c) => {
                    let local = c.clip.as_ref().and_then(|p| {
                        let world = worlds.get(k).copied().unwrap_or(Affine::IDENTITY);
                        (world.determinant().abs() > 1e-12).then(|| world.inverse() * p.clone())
                    });
                    (c.subpaths.clone(), c.area, local)
                }
                None => (Vec::new(), 0.0, None),
            };
            match per.iter_mut().find(|(t, _, _, _)| t == tag) {
                Some((_, d, a, c)) => {
                    d.retain(|s| drop.contains(s));
                    *a = a.min(area);
                    *c = both_clip(c.take(), &clip);
                }
                None => per.push((*tag, drop, area, clip)),
            }
        }
        for (tag, drop, area, clip) in per {
            match self.seen.iter_mut().find(|(t, _, _, _)| *t == tag) {
                Some((_, d, a, c)) => {
                    let mut kept = d.take().unwrap_or_default();
                    kept.retain(|s| drop.contains(s));
                    *d = Some(kept);
                    *a = a.min(area);
                    *c = both_clip(c.take(), &clip);
                }
                None => self.seen.push((tag, Some(drop), area, clip)),
            }
        }
    }
}

type Verdict = (Tag, Option<Vec<usize>>, f64, Option<BezPath>);

/// `object` with its verdicts applied (`find`: the verdict on a shape, by its
/// id), recursing into groups; `None` when nothing of it is left.
fn cut_object<'v>(
    object: &Arc<Object>,
    find: &dyn Fn(ObjectId) -> Option<&'v Verdict>,
    report: &mut SpecksReport,
    changed: &mut bool,
) -> Option<Arc<Object>> {
    match &object.kind {
        ObjectKind::Shape(sd) => {
            let Some((_, Some(drop), area, clip)) = find(object.id) else { return Some(object.clone()) };
            if drop.is_empty() && clip.is_none() {
                return Some(object.clone());
            }
            let mut path = without_subpaths(&sd.path, drop);
            if let Some(clip) = clip.as_ref().filter(|_| !path.elements().is_empty()) {
                let opts = buzz_geom::BooleanOptions {
                    fill: sd.fill.as_ref().map_or(buzz_geom::FillMode::NonZero, |f| f.rule),
                    ..buzz_geom::boolean::options_for(&[path.clone(), clip.clone()])
                };
                path = buzz_geom::boolean(&path, clip, buzz_geom::BoolOp::Difference, opts);
            }
            *changed = true;
            report.area += area;
            if path.elements().is_empty() {
                report.shapes_removed += 1;
                return None;
            }
            report.shapes_clipped += 1;
            let mut o = (**object).clone();
            if let ObjectKind::Shape(s) = &mut o.kind {
                s.path = path;
            }
            Some(Arc::new(o))
        }
        ObjectKind::Group(children) => {
            let mut inner = false;
            let kept: Vec<Arc<Object>> =
                children.iter().filter_map(|c| cut_object(c, find, report, &mut inner)).collect();
            if !inner {
                return Some(object.clone());
            }
            *changed = true;
            if kept.is_empty() {
                return None;
            }
            let mut o = (**object).clone();
            o.kind = ObjectKind::Group(kept);
            Some(Arc::new(o))
        }
        _ => Some(object.clone()),
    }
}

/// **Remove the background specks of `scope`** from `scene` (see the module
/// doc). Every keyframe it changes is rewritten in place.
pub fn remove_background_specks(scene: &mut Scene, scope: &EvenLinesScope, options: &SpeckOptions) -> SpecksReport {
    let mut report = SpecksReport::default();
    let current = match scene.editing_symbol() {
        Some(id) => Owner::Symbol(id),
        None => Owner::Stage,
    };
    // The roots: (owner, its layers' space is the picture's), and which shapes may change.
    let mut roots: Vec<(Owner, bool)> = Vec::new();
    if scope.whole_timeline || !scope.shapes.is_empty() {
        roots.push((current, scope.whole_timeline));
    }
    for &s in &scope.symbols {
        roots.push((Owner::Symbol(s), true));
    }
    let mut options = *options;
    let mut verdicts = Verdicts::default();
    let mut judged_keys: Vec<(Owner, LayerId, u32)> = Vec::new();
    for &(root, whole) in &roots {
        let Some(stack) = stack_of(scene, root) else { continue };
        let mut flat = Flat::default();
        let root_protected = matches!(root, Owner::Symbol(id) if scene.library().get(id).is_some_and(|s| is_protected_name(&s.name)));
        flatten_stack(scene, root, stack, 0, Affine::IDENTITY, root_protected, 0, &mut flat);
        flat.placed.insert(0, (root, Affine::IDENTITY));
        let shapes_only = scope.shapes.clone();
        let in_scope = move |t: &Tag| whole || (t.owner == root && shapes_only.contains(&t.object));
        let result = find_background_specks(&flat.shapes, &options);
        if options.colour.is_none() {
            // One paper colour for the whole run: the first root's.
            options.colour = result.colour;
        }
        report.colour = report.colour.or(result.colour);
        if result.colour.is_none() {
            continue;
        }
        verdicts.add(&flat.tags, &flat.worlds, &result.cuts, &in_scope);
        judged_keys.extend(flat.shown.iter().copied());
        if !whole {
            continue;
        }
        // Keyframes not shown at rest, drawn over the rest picture in place of
        // their layer's rest keyframe, wherever their timeline is shown.
        let mut owners: Vec<Owner> = Vec::new();
        for (owner, _) in &flat.placed {
            if !owners.contains(owner) {
                owners.push(*owner);
            }
        }
        for owner in owners {
            let Some(stack) = stack_of(scene, owner) else { continue };
            for layer in stack.iter() {
                if !paints(layer) || !layer.kind.holds_artwork() {
                    continue;
                }
                for kf in layer.frames.keyframes() {
                    if flat.shown.contains(&(owner, layer.id, kf.start)) || kf.objects.is_empty() {
                        continue;
                    }
                    if !kf.objects.iter().any(|o| holds_shape(o)) {
                        continue;
                    }
                    let protected = layer_protected(scene, owner, layer);
                    let mut over = Flat::default();
                    for (k, shape) in flat.shapes.iter().enumerate() {
                        if flat.tags[k].is_some_and(|t| t.owner == owner && t.layer == layer.id) {
                            continue;
                        }
                        over.shapes.push(shape.clone());
                        over.tags.push(None);
                        over.worlds.push(flat.worlds[k]);
                    }
                    for (_, place) in flat.placed.iter().filter(|(o, _)| *o == owner) {
                        for o in kf.objects.iter() {
                            flatten_object(scene, o, Some((owner, layer.id, kf.start, o.id)), *place, protected, 1, &mut over);
                        }
                    }
                    let result = find_background_specks(&over.shapes, &options);
                    verdicts.add(&over.tags, &over.worlds, &result.cuts, &in_scope);
                    judged_keys.push((owner, layer.id, kf.start));
                }
            }
        }
    }
    // Keyframes in scope that no judgement reached.
    for &(root, whole) in &roots {
        if !whole {
            continue;
        }
        let mut todo = vec![root];
        let mut seen: Vec<Owner> = Vec::new();
        while let Some(owner) = todo.pop() {
            if seen.contains(&owner) {
                continue;
            }
            seen.push(owner);
            let Some(stack) = stack_of(scene, owner) else { continue };
            for layer in stack.iter() {
                for kf in layer.frames.keyframes() {
                    for o in kf.objects.iter() {
                        for s in super::nested_symbols(o) {
                            todo.push(Owner::Symbol(s));
                        }
                    }
                    if paints(layer)
                        && kf.objects.iter().any(|o| holds_shape(o))
                        && !judged_keys.contains(&(owner, layer.id, kf.start))
                    {
                        report.keyframes_not_judged += 1;
                    }
                }
            }
        }
    }

    // Write: per keyframe, the shapes cut.
    let mut keys: Vec<(Owner, LayerId, u32)> = Vec::new();
    for (tag, drop, _, clip) in &verdicts.seen {
        if (drop.as_ref().is_some_and(|d| !d.is_empty()) || clip.is_some())
            && !keys.contains(&(tag.owner, tag.layer, tag.start))
        {
            keys.push((tag.owner, tag.layer, tag.start));
        }
    }
    for (owner, layer_id, start) in keys {
        let Some(stack) = stack_of(scene, owner) else { continue };
        let Some(layer) = stack.get(layer_id) else { continue };
        let Some(kf) = layer.frames.keyframes().iter().find(|k| k.start == start) else { continue };
        let mut out: Vec<Arc<Object>> = Vec::with_capacity(kf.objects.len());
        let mut changed = false;
        for object in kf.objects.iter() {
            let find = |shape: ObjectId| {
                verdicts.seen.iter().find(|(t, _, _, _)| {
                    t.owner == owner && t.layer == layer_id && t.start == start && t.object == object.id && t.shape == shape
                })
            };
            if let Some(kept) = cut_object(object, &find, &mut report, &mut changed) {
                out.push(kept);
            }
        }
        if !changed {
            continue;
        }
        report.keyframes += 1;
        let write = move |layer: &mut Layer| {
            if let Some(kf) = layer.frames.keyframes_mut().iter_mut().find(|kf| kf.start == start) {
                kf.objects = Arc::new(out);
            }
        };
        match owner {
            Owner::Stage => {
                scene.update_stage_layer(layer_id, write);
            }
            Owner::Symbol(symbol) => {
                scene.library_mut().update(symbol, |s| {
                    s.layers.update(layer_id, write);
                });
            }
        }
    }
    report
}
