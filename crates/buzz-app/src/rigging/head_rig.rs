//! **H1: Rig Head** (`docs/HEAD_RIG_PLAN.md`): a head symbol whose face
//! parts are already separate (layers, or named objects on a layer) turned
//! into the user's head structure, as [`HeadStyle`] holds it. Top to bottom:
//! `eyeLid` (a 7-frame blink), `eyeMask` (a mask layer: the eye whites' fill
//! without their outline), `eyeBall` (masked by it: both irises/pupils, an
//! iris cut by the eye's edge completed as a full circle), `eyeWhite` (both
//! whites with their outlines), the brows, extras (moustache), `mouth` (a
//! symbol), the nose, hair and `base`. No layer parenting inside the head.
//!
//! The eye parts are split out of each eye drawing by colour and place: the
//! white is the light fill region (with everything under it and its
//! outline), an eyeball shape is drawn over the white and lies mostly inside
//! it. The lid frames are made from the skin colour sampled around the eye
//! and a lid line in the eye outline's ink colour; their lower edge closes
//! from the top of the eye to its bottom, each frame covering the share of
//! the white the style's blink curve says. The head rests open: the lid
//! instance shows frame 0 (empty), single frame, so it never blinks by
//! itself.
//!
//! The rest picture does not change: the eye shapes keep their paint order
//! within each new layer, kept layers keep their timelines, and the mouth
//! drawing is moved into a symbol at the same place.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, BoolOp, BooleanOptions, FillMode, Point, Rect, Shape as _};
use buzz_rig::head_style::{HeadLayerKind, HeadPart, HeadStyle};
use buzz_scene::{
    ColorTransform, FillSpec, Keyframe, Layer, LayerId, LayerKind, LayerStack, LayerTimeline, LoopMode, Object,
    ObjectKind, Paint, Scene, ShapeData, SymbolId, SymbolKind,
};
use peniko::Color;

/// What Rig Head did to one head symbol.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeadRigReport {
    pub symbol: Option<SymbolId>,
    /// "front" or "side".
    pub view: String,
    /// The head's layers after, top first.
    pub layers: Vec<String>,
    /// The head already had an eye mask over masked eyeballs: nothing done.
    pub already_rigged: bool,
    pub eyes: usize,
    /// Irises cut by the eye's edge, completed as full circles.
    pub irises_completed: usize,
    pub eye_white: Option<SymbolId>,
    pub eye_mask: Option<SymbolId>,
    pub eye_ball: Option<SymbolId>,
    pub eye_lid: Option<SymbolId>,
    /// The mouth's symbol (an existing mouth-shape symbol kept, or the mouth
    /// drawing made into one).
    pub mouth: Option<SymbolId>,
    /// H2: the head was one drawing, cut into its parts first.
    pub cut: bool,
    /// H2: eyes made from see-through eye holes (the head's eyes sat behind
    /// it): a white, an outline where the edge had none and a round iris.
    pub eye_holes: usize,
    /// The lid's skin colour (sampled around the eyes) and its line colour.
    pub skin: [u8; 4],
    pub ink: [u8; 4],
    pub notes: Vec<String>,
}

/// What a head layer (or a named object on one) is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Sound,
    Eye,
    Lid,
    Brow,
    Moustache,
    Mouth,
    Nose,
    Hair,
    Base,
    Extra,
}

pub(crate) fn classify(name: &str) -> Option<Part> {
    let n = name.to_lowercase();
    let has = |w: &str| n.contains(w);
    Some(if has("sound") || has("audio") || has("voice") {
        Part::Sound
    } else if has("brow") {
        Part::Brow
    } else if has("lid") || has("lash") || has("blink") {
        Part::Lid
    } else if has("eye") || has("pupil") || has("iris") || has("ball") {
        Part::Eye
    } else if has("moust") || has("mustach") || has("mooch") || has("beard") {
        Part::Moustache
    } else if has("mouth") || has("lip") || has("jaw") || has("teeth") {
        Part::Mouth
    } else if has("nose") {
        Part::Nose
    } else if has("hair") || has("pagdi") || has("turban") || has("topi") || has("cap") || has("hat") {
        Part::Hair
    } else if has("head") || has("face") || has("base") || has("skin") || has("skull") {
        Part::Base
    } else {
        return None;
    })
}

/// A layer, or some of the objects on one, going to one part.
#[derive(Clone)]
struct Unit {
    layer: Arc<Layer>,
    /// `Some`: these objects of the layer's frame-0 keyframe only.
    objects: Option<Vec<Arc<Object>>>,
    part: Part,
    /// Top-first index of the layer it came from.
    index: usize,
}

impl Unit {
    fn rest_objects(&self) -> Vec<Arc<Object>> {
        match &self.objects {
            Some(objects) => objects.clone(),
            None => self.layer.objects_at(0).to_vec(),
        }
    }
}

fn object_label(scene: &Scene, object: &Object) -> String {
    let mut label = object.name.clone().unwrap_or_default();
    if let ObjectKind::Instance(i) = &object.kind {
        if let Some(s) = scene.library().get(i.symbol) {
            label.push(' ');
            label.push_str(&s.name);
        }
    }
    label
}

// ---------------------------------------------------------------------------
// Flattening what is drawn at rest into shapes in the head's space
// ---------------------------------------------------------------------------

fn transformed(shape: &ShapeData, world: Affine, colours: &[ColorTransform]) -> ShapeData {
    let mut s = shape.clone();
    s.path = world * shape.path.clone();
    let recolour = |p: &Paint| {
        let mut p = p.transformed(world);
        for ct in colours {
            if !ct.is_identity() {
                p = p.map_colors(|c| ct.apply(c));
            }
        }
        p
    };
    if let Some(fill) = &mut s.fill {
        fill.paint = recolour(&fill.paint);
    }
    if let Some(stroke) = &mut s.stroke {
        stroke.paint = recolour(&stroke.paint);
        if !stroke.hairline {
            stroke.width *= world.determinant().abs().sqrt();
        }
    }
    s
}

/// A stroke's filled outline (round caps and joins).
pub(super) fn stroke_outline(path: &BezPath, width: f64) -> BezPath {
    let style = kurbo::Stroke::new(width.max(0.1)).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round);
    kurbo::stroke(path.iter(), &style, &kurbo::StrokeOpts::default(), 0.05)
}

/// The region a shape paints: its fill, or its stroke's outline.
pub(super) fn painted_region(s: &ShapeData) -> (BezPath, FillMode) {
    match (&s.fill, &s.stroke) {
        (Some(f), _) => (s.path.clone(), f.rule),
        (None, Some(k)) => (stroke_outline(&s.path, if k.hairline { 1.0 } else { k.width }), FillMode::NonZero),
        _ => (BezPath::new(), FillMode::NonZero),
    }
}

pub(super) fn clip_shape(s: &ShapeData, clip: &BezPath, op: BoolOp) -> Option<ShapeData> {
    let opts = BooleanOptions::for_shape_size(s.path.bounding_box().union(clip.bounding_box()).size().to_vec2().hypot());
    let mut out = s.clone();
    if s.fill.is_some() {
        let mut o = opts;
        o.fill = s.fill.as_ref().map(|f| f.rule).unwrap_or_default();
        out.path = buzz_geom::boolean(&s.path, clip, op, o);
        if s.stroke.is_some() {
            out.stroke = None;
        }
    } else if let Some(k) = &s.stroke {
        let outline = stroke_outline(&s.path, k.width);
        out.path = buzz_geom::boolean(&outline, clip, op, opts);
        out.fill = Some(FillSpec { paint: k.paint.clone(), rule: FillMode::NonZero, swatch: None });
        out.stroke = None;
    }
    (!out.path.elements().is_empty()).then_some(out)
}

fn flatten_into(scene: &Scene, object: &Object, parent: Affine, colours: &[ColorTransform], out: &mut Vec<ShapeData>, depth: usize) {
    flatten_into_placed(scene, object, parent, colours, out, &mut Vec::new(), depth);
}

/// [`flatten_into`], also noting where every symbol shown inside is placed:
/// (symbol, its space → the flattened space). E1e (`docs/EVEN_LINES_PLAN.md`).
fn flatten_into_placed(
    scene: &Scene,
    object: &Object,
    parent: Affine,
    colours: &[ColorTransform],
    out: &mut Vec<ShapeData>,
    placed: &mut Vec<(SymbolId, Affine)>,
    depth: usize,
) {
    if !object.visible || depth > 12 {
        return;
    }
    let world = parent * object.transform;
    match &object.kind {
        ObjectKind::Shape(s) => out.push(transformed(s, world, colours)),
        ObjectKind::Group(children) => {
            for c in children {
                flatten_into_placed(scene, c, world, colours, out, placed, depth + 1);
            }
        }
        ObjectKind::Instance(i) => {
            let Some(symbol) = scene.library().get(i.symbol) else { return };
            placed.push((i.symbol, world));
            let frame = i.resolve_frame(symbol.kind, 0, symbol.length());
            let mut chain = vec![i.color];
            chain.extend_from_slice(colours);
            flatten_stack_placed(scene, &symbol.layers, frame, world, &chain, out, placed, depth + 1);
        }
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                flatten_into_placed(scene, &part, world, colours, out, placed, depth + 1);
            }
        }
        ObjectKind::Warp(w) => out.push(transformed(&w.warped(), world, colours)),
    }
}

pub(super) fn flatten_stack(
    scene: &Scene,
    stack: &LayerStack,
    frame: u32,
    world: Affine,
    colours: &[ColorTransform],
    out: &mut Vec<ShapeData>,
    depth: usize,
) {
    flatten_stack_placed(scene, stack, frame, world, colours, out, &mut Vec::new(), depth);
}

/// [`flatten_stack`], also noting every symbol placement inside (see
/// [`flatten_into_placed`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn flatten_stack_placed(
    scene: &Scene,
    stack: &LayerStack,
    frame: u32,
    world: Affine,
    colours: &[ColorTransform],
    out: &mut Vec<ShapeData>,
    placed: &mut Vec<(SymbolId, Affine)>,
    depth: usize,
) {
    for layer in stack.paint_order() {
        if matches!(layer.kind, LayerKind::Folder | LayerKind::Guide | LayerKind::Mask | LayerKind::InverseMask)
            || !layer.is_drawable_at(frame)
            || !stack.is_effectively_visible(layer.id)
        {
            continue;
        }
        let mut shapes = Vec::new();
        for o in layer.objects_at(frame) {
            flatten_into_placed(scene, o, world, colours, &mut shapes, placed, depth);
        }
        if let Some(mask) = stack.mask_for(layer.id).and_then(|m| stack.get(m)) {
            let mut mask_shapes = Vec::new();
            for o in mask.objects_at(frame) {
                flatten_into(scene, o, world, colours, &mut mask_shapes, depth);
            }
            let paths: Vec<BezPath> = mask_shapes.iter().map(|s| painted_region(s).0).collect();
            let region = buzz_geom::union_all(&paths, buzz_geom::boolean::options_for(&paths));
            let op = if mask.kind == LayerKind::InverseMask { BoolOp::Difference } else { BoolOp::Intersect };
            shapes = shapes.iter().filter_map(|s| clip_shape(s, &region, op)).collect();
        }
        out.extend(shapes);
    }
}

/// **What `object` draws at rest**, as plain shapes in its parent's space
/// (`parent` applied), in paint order: groups opened, instances shown at
/// the frame they rest on, masks inside them applied (a masked shape
/// clipped to its mask), colour effects baked in.
pub fn flatten_rest(scene: &Scene, object: &Object, parent: Affine) -> Vec<ShapeData> {
    let mut out = Vec::new();
    flatten_into(scene, object, parent, &[], &mut out, 0);
    out
}

// ---------------------------------------------------------------------------
// Colour and sampled geometry
// ---------------------------------------------------------------------------

pub(super) fn rgba(c: Color) -> [f64; 4] {
    let p = c.to_rgba8();
    [p.r as f64 / 255.0, p.g as f64 / 255.0, p.b as f64 / 255.0, p.a as f64 / 255.0]
}

pub(super) fn paint_colour(s: &ShapeData) -> Option<Color> {
    s.fill.as_ref().map(|f| f.paint.color()).or_else(|| s.stroke.as_ref().map(|k| k.paint.color()))
}

pub(super) fn luma(c: Color) -> f64 {
    let [r, g, b, _] = rgba(c);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub(super) fn is_light(c: Color) -> bool {
    let [r, g, b, a] = rgba(c);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    a > 0.5 && luma(c) >= 0.62 && (max - min) <= 0.25 * max.max(1e-6)
}

pub(super) fn contains(path: &BezPath, rule: FillMode, p: Point) -> bool {
    if !path.bounding_box().contains(p) {
        return false;
    }
    let w = path.winding(p);
    match rule {
        FillMode::NonZero => w != 0,
        FillMode::EvenOdd => w % 2 != 0,
    }
}

/// A sample grid over a rectangle.
pub(super) struct Grid {
    pub(super) origin: Point,
    pub(super) cell: f64,
    pub(super) cols: usize,
    pub(super) rows: usize,
}

impl Grid {
    pub(super) fn over(r: Rect, cell: f64) -> Self {
        let cols = ((r.width() / cell).ceil() as usize).clamp(1, 600);
        let rows = ((r.height() / cell).ceil() as usize).clamp(1, 600);
        let cell = (r.width() / cols as f64).max(r.height() / rows as f64).max(1e-6);
        Self { origin: Point::new(r.x0, r.y0), cell, cols, rows }
    }
    pub(super) fn centre(&self, c: usize, r: usize) -> Point {
        Point::new(self.origin.x + (c as f64 + 0.5) * self.cell, self.origin.y + (r as f64 + 0.5) * self.cell)
    }
    pub(super) fn raster(&self, path: &BezPath, rule: FillMode) -> Vec<bool> {
        let bb = path.bounding_box();
        let mut out = vec![false; self.cols * self.rows];
        for r in 0..self.rows {
            for c in 0..self.cols {
                let p = self.centre(c, r);
                if p.y >= bb.y0 && p.y <= bb.y1 && p.x >= bb.x0 && p.x <= bb.x1 {
                    out[r * self.cols + c] = contains(path, rule, p);
                }
            }
        }
        out
    }
}

/// Share of `a` (cells) that is inside `b`, on a grid over `a`.
pub(super) fn share_inside(a: &(BezPath, FillMode), b: &(BezPath, FillMode)) -> f64 {
    let bb = a.0.bounding_box();
    if bb.area() <= 0.0 {
        return 0.0;
    }
    let g = Grid::over(bb, (bb.width().max(bb.height()) / 40.0).max(0.05));
    let ra = g.raster(&a.0, a.1);
    let rb = g.raster(&b.0, b.1);
    let n = ra.iter().filter(|x| **x).count();
    if n == 0 {
        return 0.0;
    }
    ra.iter().zip(&rb).filter(|(x, y)| **x && **y).count() as f64 / n as f64
}

pub(super) fn region_area(r: &(BezPath, FillMode)) -> f64 {
    let bb = r.0.bounding_box();
    if bb.area() <= 0.0 {
        return 0.0;
    }
    let g = Grid::over(bb, (bb.width().max(bb.height()) / 60.0).max(0.02));
    g.raster(&r.0, r.1).iter().filter(|x| **x).count() as f64 * g.cell * g.cell
}

/// Least-squares circle through `points` (Kåsa), as (centre, radius).
fn fit_circle(points: &[Point]) -> Option<(Point, f64)> {
    if points.len() < 6 {
        return None;
    }
    // x² + y² + D x + E y + F = 0.
    let (mut sxx, mut sxy, mut syy, mut sx, mut sy, n) = (0.0, 0.0, 0.0, 0.0, 0.0, points.len() as f64);
    let (mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0);
    for p in points {
        let z = p.x * p.x + p.y * p.y;
        sxx += p.x * p.x;
        sxy += p.x * p.y;
        syy += p.y * p.y;
        sx += p.x;
        sy += p.y;
        sxz += p.x * z;
        syz += p.y * z;
        sz += z;
    }
    let m = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]];
    let v = [-sxz, -syz, -sz];
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    if d.abs() < 1e-12 {
        return None;
    }
    let mut sol = [0.0; 3];
    for (k, out) in sol.iter_mut().enumerate() {
        let mut mk = m;
        for row in 0..3 {
            mk[row][k] = v[row];
        }
        *out = det(&mk) / d;
    }
    let c = Point::new(-sol[0] / 2.0, -sol[1] / 2.0);
    let r2 = c.x * c.x + c.y * c.y - sol[2];
    (r2 > 0.0).then(|| (c, r2.sqrt()))
}

pub(super) fn boundary_points(path: &BezPath, step: f64) -> Vec<Point> {
    let mut pts = Vec::new();
    let mut last: Option<Point> = None;
    kurbo::flatten(path.iter(), (step * 0.05).max(0.01), |el| {
        let p = match el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => p,
            _ => return,
        };
        if let Some(l) = last {
            let d = l.distance(p);
            let k = (d / step).floor() as usize;
            for i in 1..=k {
                pts.push(l.lerp(p, i as f64 * step / d));
            }
        }
        pts.push(p);
        last = Some(p);
    });
    pts
}

/// The full circle an iris (or the ring of its outline, or an iris with a
/// hole for the pupil) cut by the eye's edge was part of, through the points
/// of its outline that are not on the cut: a circle fitted to them, its
/// radius the fit's for a shape covering the centre, else the outer spread
/// of the points.
/// `None` when the shape is not cut by the eye's edge (a pupil drawn whole
/// inside the white is left as drawn, however unround), or the fit is not
/// believable.
fn completed_iris(iris: &(BezPath, FillMode), eye: &(BezPath, FillMode)) -> Option<(Point, f64)> {
    let white = eye.0.bounding_box();
    let bb = iris.0.bounding_box();
    let pts = boundary_points(&iris.0, (bb.width().max(bb.height()) / 80.0).max(0.05));
    let edge = boundary_points(&eye.0, (white.width().max(white.height()) / 200.0).max(0.05));
    let tol = (white.height() * 0.04).max(0.75);
    let near = |p: &Point| edge.iter().any(|q| q.distance(*p) < tol);
    let outside = pts.iter().filter(|p| !near(p) && !contains(&eye.0, eye.1, **p)).count();
    let arc: Vec<Point> = pts.iter().filter(|p| !near(p) && contains(&eye.0, eye.1, **p)).copied().collect();
    let along = pts.len() - arc.len() - outside;
    // Drawn over the eye's edge rather than cut by it: left as drawn.
    if (outside as f64) > 0.1 * pts.len() as f64 || (along as f64) < 0.15 * pts.len() as f64 || arc.len() < 8 {
        return None;
    }
    let (c, r) = fit_circle(&arc)?;
    let mut d: Vec<f64> = arc.iter().map(|p| p.distance(c)).collect();
    d.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| d[((d.len() - 1) as f64 * q).round() as usize];
    let area = region_area(iris);
    let disc = contains(&iris.0, iris.1, c);
    let (outer, inner) = if disc { (r, 0.0) } else { (at(0.9), at(0.1)) };
    if !disc && inner >= 0.85 * outer {
        return None;
    }
    let full = std::f64::consts::PI * (outer * outer - inner * inner);
    let believable = outer <= 0.75 * white.width().max(white.height()) && white.inflate(outer, outer).contains(c);
    if !(believable && full > area * 1.06) {
        return None;
    }
    Some((c, outer))
}

// ---------------------------------------------------------------------------
// The eyes
// ---------------------------------------------------------------------------

struct Eye {
    /// Index of the white body in the eye shapes.
    body: usize,
    white: (BezPath, FillMode),
    bounds: Rect,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    White,
    Ball,
}

struct EyeSplit {
    eyes: Vec<Eye>,
    /// Per eye shape: its role and its eye.
    roles: Vec<(Role, Option<usize>)>,
}

fn split_eyes(shapes: &[ShapeData]) -> Option<EyeSplit> {
    let regions: Vec<(BezPath, FillMode)> = shapes.iter().map(painted_region).collect();
    let mut light: Vec<(usize, f64)> = shapes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.fill.is_some() && paint_colour(s).is_some_and(is_light))
        .map(|(i, _)| (i, region_area(&regions[i])))
        .collect();
    light.sort_by(|a, b| b.1.total_cmp(&a.1));
    let biggest = light.first()?.1;
    let mut eyes: Vec<Eye> = Vec::new();
    for (i, area) in &light {
        if *area < 0.3 * biggest {
            break;
        }
        let bb = regions[*i].0.bounding_box();
        if eyes.iter().any(|e| e.bounds.intersect(bb).area() > 0.0) {
            continue;
        }
        eyes.push(Eye { body: *i, white: regions[*i].clone(), bounds: bb });
    }
    eyes.sort_by(|a, b| a.bounds.center().x.total_cmp(&b.bounds.center().x));
    let mut roles: Vec<(Role, Option<usize>)> = vec![(Role::White, None); shapes.len()];
    for (k, e) in eyes.iter().enumerate() {
        roles[e.body] = (Role::White, Some(k));
    }
    for i in 0..shapes.len() {
        if eyes.iter().any(|e| e.body == i) {
            continue;
        }
        let bb = regions[i].0.bounding_box();
        if bb.area() <= 0.0 && bb.width() + bb.height() <= 0.0 {
            continue;
        }
        // The eye it belongs to: the one its box overlaps most (grown by a
        // third), else the nearest.
        let k = eyes
            .iter()
            .enumerate()
            .map(|(k, e)| {
                let grown = e.bounds.inflate(e.bounds.width() / 3.0, e.bounds.height() / 3.0);
                (k, grown.intersect(bb).area(), e.bounds.center().distance(bb.center()))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(k, _, _)| k);
        let Some(k) = k else { continue };
        let eye = &eyes[k];
        let above = i > eye.body;
        let colour = paint_colour(&shapes[i]);
        // An outline drawn over the white's edge can lie mostly inside it;
        // it spans the eye's width, where an iris or pupil does not (an iris
        // can fill a narrow eye's height).
        let spans = bb.width() > 0.8 * eye.bounds.width();
        let role = if above && !spans && share_inside(&regions[i], &eye.white) >= 0.6 {
            if colour.is_some_and(is_light) {
                // A highlight on a pupil is part of the eyeball; a speck on
                // the white stays with the white.
                let centre = bb.center();
                // (By box: the highlight may sit in the iris's hole for the
                // pupil.)
                let on_ball = (0..i).any(|j| roles[j] == (Role::Ball, Some(k)) && regions[j].0.bounding_box().contains(centre));
                if on_ball { Role::Ball } else { Role::White }
            } else {
                Role::Ball
            }
        } else {
            Role::White
        };
        roles[i] = (role, Some(k));
    }
    // Drawn over an eyeball and lying in the white, a shape is part of the
    // eyeball however wide (a big iris over its own darker centre): left in
    // the white, it would end up under the eyeball.
    for i in 0..shapes.len() {
        let (Role::White, Some(k)) = roles[i] else { continue };
        if eyes[k].body == i || paint_colour(&shapes[i]).is_none_or(is_light) {
            continue;
        }
        let bb = regions[i].0.bounding_box();
        let over_ball = (0..i).any(|j| roles[j] == (Role::Ball, Some(k)) && regions[j].0.bounding_box().intersect(bb).area() > 0.0);
        if over_ball && share_inside(&regions[i], &eyes[k].white) >= 0.6 {
            roles[i] = (Role::Ball, Some(k));
        }
    }
    Some(EyeSplit { eyes, roles })
}

/// `shape` as one shape per group of its subpaths that do not touch (by
/// their boxes): both eyes' whites drawn as one path become one white each.
/// Subpaths inside each other (a hole) stay together, so nothing draws
/// differently.
pub(super) fn split_disjoint(shape: &ShapeData) -> Vec<ShapeData> {
    let mut subs: Vec<BezPath> = Vec::new();
    for el in shape.path.elements() {
        if matches!(el, kurbo::PathEl::MoveTo(_)) || subs.is_empty() {
            subs.push(BezPath::new());
        }
        if let Some(last) = subs.last_mut() {
            last.push(*el);
        }
    }
    if subs.len() < 2 {
        return vec![shape.clone()];
    }
    let pad = shape.stroke.as_ref().map(|k| k.width).unwrap_or(0.0);
    let boxes: Vec<Rect> = subs.iter().map(|p| p.bounding_box().inflate(pad, pad)).collect();
    let mut group: Vec<usize> = (0..subs.len()).collect();
    fn root(g: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while g[i] != i {
            g[i] = g[g[i]];
            i = g[i];
        }
        i
    }
    for a in 0..subs.len() {
        for b in a + 1..subs.len() {
            if boxes[a].intersect(boxes[b]).area() > 0.0 || boxes[a].contains(boxes[b].center()) {
                let (ra, rb) = (root(&mut group, a), root(&mut group, b));
                group[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut order: Vec<usize> = Vec::new();
    for i in 0..subs.len() {
        let r = root(&mut group, i);
        if !order.contains(&r) {
            order.push(r);
        }
    }
    if order.len() < 2 {
        return vec![shape.clone()];
    }
    order
        .iter()
        .map(|r| {
            let mut s = shape.clone();
            let mut path = BezPath::new();
            for (i, sub) in subs.iter().enumerate() {
                if root(&mut group, i) == *r {
                    for el in sub.elements() {
                        path.push(*el);
                    }
                }
            }
            s.path = path;
            s
        })
        .collect()
}

/// The convex hull of `points` as a closed path (Andrew's monotone chain).
pub(super) fn hull_path(points: &[Point]) -> BezPath {
    let mut p: Vec<Point> = points.to_vec();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup_by(|a, b| a.distance(*b) < 1e-9);
    if p.len() < 3 {
        return BezPath::new();
    }
    let cross = |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut hull: Vec<Point> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Point>> = if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for q in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], *q) <= 0.0 {
                hull.pop();
            }
            hull.push(*q);
        }
        hull.pop();
    }
    let mut path = BezPath::new();
    for (i, q) in hull.iter().enumerate() {
        if i == 0 { path.move_to(*q) } else { path.line_to(*q) }
    }
    path.close_path();
    path
}

/// The column profile of a region: per column, the top and bottom of what
/// it covers.
struct Profile {
    x0: f64,
    step: f64,
    top: Vec<Option<f64>>,
    bottom: Vec<Option<f64>>,
}

fn profile(regions: &[(BezPath, FillMode)], bounds: Rect, step: f64) -> Profile {
    let g = Grid::over(bounds, step);
    let rasters: Vec<Vec<bool>> = regions.iter().map(|r| g.raster(&r.0, r.1)).collect();
    let mut top = vec![None; g.cols];
    let mut bottom = vec![None; g.cols];
    for c in 0..g.cols {
        for r in 0..g.rows {
            if rasters.iter().any(|ra| ra[r * g.cols + c]) {
                let y = g.centre(c, r).y;
                if top[c].is_none() {
                    top[c] = Some(y - g.cell / 2.0);
                }
                bottom[c] = Some(y + g.cell / 2.0);
            }
        }
    }
    Profile { x0: g.origin.x + g.cell / 2.0, step: g.cell, top, bottom }
}

/// The lid's lower edge closed `t` of the way (0 open, 1 shut), per column.
fn lid_edge(p: &Profile, t: f64) -> Vec<Option<f64>> {
    p.top.iter().zip(&p.bottom).map(|(a, b)| Some(a.as_ref()? + t * (b.as_ref()? - a.as_ref()?))).collect()
}

/// Share of the white's cells above the lid edge.
fn lid_cover(p: &Profile, white: &(BezPath, FillMode), t: f64) -> f64 {
    let edge = lid_edge(p, t);
    let bb = white.0.bounding_box();
    let g = Grid::over(bb, p.step);
    let ra = g.raster(&white.0, white.1);
    let (mut n, mut covered) = (0usize, 0usize);
    for r in 0..g.rows {
        for c in 0..g.cols {
            if !ra[r * g.cols + c] {
                continue;
            }
            n += 1;
            let q = g.centre(c, r);
            let col = ((q.x - p.x0) / p.step).round();
            if col >= 0.0 && (col as usize) < edge.len() && edge[col as usize].is_some_and(|y| q.y <= y) {
                covered += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { covered as f64 / n as f64 }
}

/// The lid closed `t` of the way: its skin and its line.
fn lid_shapes(p: &Profile, t: f64, margin: f64, skin: Color, ink: Color, line: f64) -> Vec<ShapeData> {
    let edge = lid_edge(p, t);
    let cols: Vec<usize> = (0..edge.len()).filter(|c| edge[*c].is_some() && p.top[*c].is_some()).collect();
    if cols.len() < 2 {
        return Vec::new();
    }
    let x = |c: usize| p.x0 + c as f64 * p.step;
    // Smooth the lower edge a little: the profile is on a grid.
    let lower: Vec<f64> = cols
        .iter()
        .enumerate()
        .map(|(k, _)| {
            let a = k.saturating_sub(1);
            let b = (k + 1).min(cols.len() - 1);
            (a..=b).map(|j| edge[cols[j]].unwrap_or(0.0)).sum::<f64>() / (b - a + 1) as f64
        })
        .collect();
    let first = cols[0];
    let last = *cols.last().unwrap_or(&first);
    let mut fill = BezPath::new();
    fill.move_to(Point::new(x(first) - margin, p.top[first].unwrap_or(0.0) - margin));
    for c in &cols {
        fill.line_to(Point::new(x(*c), p.top[*c].unwrap_or(0.0) - margin));
    }
    fill.line_to(Point::new(x(last) + margin, p.top[last].unwrap_or(0.0) - margin));
    fill.line_to(Point::new(x(last) + margin, lower[cols.len() - 1]));
    for (k, c) in cols.iter().enumerate().rev() {
        fill.line_to(Point::new(x(*c), lower[k]));
    }
    fill.line_to(Point::new(x(first) - margin, lower[0]));
    fill.close_path();
    let mut stroke = BezPath::new();
    stroke.move_to(Point::new(x(first) - margin * 0.5, lower[0]));
    for (k, c) in cols.iter().enumerate() {
        stroke.line_to(Point::new(x(*c), lower[k]));
    }
    stroke.line_to(Point::new(x(last) + margin * 0.5, lower[cols.len() - 1]));
    vec![ShapeData::filled(fill, skin), ShapeData::stroked(stroke, ink, line)]
}

/// The skin around the eyes: the colour most often found just outside
/// them, on the head's other parts (`under`, paint order), dark lines and
/// whites aside.
fn sample_skin(under: &[ShapeData], around: &[Rect]) -> Option<Color> {
    let regions: Vec<(BezPath, FillMode)> = under.iter().map(painted_region).collect();
    let mut counts: Vec<(Color, usize)> = Vec::new();
    for b in around {
        let (cx, cy) = (b.center().x, b.center().y);
        for ring in [1.15, 1.35] {
            let (rx, ry) = (b.width() / 2.0 * ring + 2.0, b.height() / 2.0 * ring + 2.0);
            for k in 0..32 {
                let a = k as f64 / 32.0 * std::f64::consts::TAU;
                let q = Point::new(cx + rx * a.cos(), cy + ry * a.sin());
                if q.y > cy + ry * 0.5 {
                    continue; // below the eye: cheeks, shading
                }
                let hit = (0..under.len()).rev().find(|i| under[*i].fill.is_some() && contains(&regions[*i].0, regions[*i].1, q));
                let Some(c) = hit.and_then(|i| paint_colour(&under[i])) else { continue };
                if luma(c) < 0.15 || is_light(c) {
                    continue;
                }
                match counts.iter_mut().find(|(k, _)| k.to_rgba8() == c.to_rgba8()) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((c, 1)),
                }
            }
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

fn shape_object(scene: &mut Scene, s: ShapeData) -> Arc<Object> {
    Arc::new(Object::shape(scene.next_object_id(), s))
}

/// A one-layer symbol holding `keys` (frame, shapes) in head space.
fn shapes_symbol(scene: &mut Scene, name: &str, folder: Option<&str>, keys: Vec<Vec<ShapeData>>) -> SymbolId {
    let id = scene.add_symbol(name, SymbolKind::Graphic, folder);
    let frames: Vec<Keyframe> = keys
        .into_iter()
        .enumerate()
        .map(|(f, shapes)| {
            let mut k = Keyframe::new(f as u32);
            k.objects = Arc::new(shapes.into_iter().map(|s| shape_object(scene, s)).collect());
            k
        })
        .collect();
    let length = frames.len().max(1) as u32;
    scene.library_mut().update(id, |s| {
        let first = s.layers.iter().next().map(|l| l.id);
        if let Some(layer) = first {
            s.layers.update(layer, |l| l.frames = LayerTimeline::from_parts(frames, length));
        }
    });
    id
}

fn new_layer(scene: &mut Scene, name: &str, kind: LayerKind, objects: Vec<Arc<Object>>, length: u32) -> Layer {
    let mut layer = Layer::new(LayerId(scene.next_object_id().0), name, kind);
    let mut k = Keyframe::new(0);
    k.objects = Arc::new(objects);
    layer.frames = LayerTimeline::from_parts(vec![k], length.max(1));
    layer
}

fn rest_instance(scene: &mut Scene, symbol: SymbolId, pivot: Point) -> Arc<Object> {
    let mut o = Object::instance_of(scene.next_object_id(), symbol);
    if let ObjectKind::Instance(i) = &mut o.kind {
        i.first_frame = 0;
        i.loop_mode = LoopMode::SingleFrame;
    }
    o.pivot = Some(pivot);
    Arc::new(o)
}

/// Is `stack` already a rigged head: an eye mask over masked eyeballs?
fn already_rigged(stack: &LayerStack) -> bool {
    stack.mask_groups().iter().any(|g| {
        !g.masked.is_empty()
            && g.masked.iter().any(|m| {
                stack.get(*m).is_some_and(|l| {
                    let n = l.name.to_lowercase();
                    n.contains("eye") || n.contains("ball") || n.contains("pupil")
                })
            })
    })
}

/// **Rig Head** on the head symbol `head`, by `style` (see the module doc).
/// Nothing changes when it fails or when the head is already rigged.
///
/// **H2**: a head with no separate eye parts is one drawing: its face parts
/// are cut out of it first ([`super::head_cut`]), eyes made from eye holes
/// when its eyes sit behind it, then rigged, all or nothing.
pub fn rig_head(scene: &mut Scene, head: SymbolId, style: &HeadStyle) -> Result<HeadRigReport, String> {
    rig_head_inner(scene, head, style, true)
}

/// Would Rig Head make `head`'s eyes from eye holes? A note for the AI rig
/// review ("eyes made from eye holes"), else `None`.
pub fn head_rig_flag(scene: &Scene, head: SymbolId) -> Option<String> {
    let symbol = scene.library().get(head)?;
    if already_rigged(&symbol.layers) || symbol.layers.iter().any(|l| classify(&l.name) == Some(Part::Eye)) {
        return None;
    }
    match super::head_cut::eye_holes_in(scene, head) {
        0 => None,
        n => Some(format!("'{}': eyes made from eye holes ({n}): check the head after Apply", symbol.name)),
    }
}

fn rig_head_inner(scene: &mut Scene, head: SymbolId, style: &HeadStyle, may_cut: bool) -> Result<HeadRigReport, String> {
    let symbol = scene.library().get(head).cloned().ok_or("Rig Head: no such symbol")?;
    let mut report = HeadRigReport { symbol: Some(head), ..Default::default() };
    if already_rigged(&symbol.layers) {
        report.already_rigged = true;
        report.layers = symbol.layers.iter().map(|l| l.name.clone()).collect();
        report.notes.push(format!("'{}' already has an eye mask over its eyeballs: left as it is", symbol.name));
        return Ok(report);
    }
    let length = symbol.length().max(1);

    // Units: each layer by its name; a layer whose name says nothing (or
    // holds several named parts) split by its objects' names.
    let mut units: Vec<Unit> = Vec::new();
    for (index, layer) in symbol.layers.iter().enumerate() {
        if matches!(layer.kind, LayerKind::Folder) {
            continue;
        }
        let own = classify(&layer.name);
        let objects = layer.objects_at(0);
        let named: Vec<Option<Part>> = objects.iter().map(|o| classify(&object_label(scene, o))).collect();
        let distinct = {
            let mut parts: Vec<Part> = named.iter().flatten().copied().collect();
            parts.dedup();
            parts.sort_by_key(|p| *p as u8);
            parts.dedup();
            parts.len()
        };
        let single_key = layer.frames.keyframe_count() <= 1;
        if single_key && objects.len() > 1 && distinct >= 2 && named.iter().all(|n| n.is_some()) {
            // One unit per run of objects of the same part, in paint order.
            let mut runs: Vec<(Part, Vec<Arc<Object>>)> = Vec::new();
            for (o, p) in objects.iter().zip(&named) {
                let p = p.unwrap_or(Part::Extra);
                match runs.last_mut() {
                    Some((q, v)) if *q == p => v.push(o.clone()),
                    _ => runs.push((p, vec![o.clone()])),
                }
            }
            // Top first, like the layers.
            for (p, v) in runs.into_iter().rev() {
                units.push(Unit { layer: layer.clone(), objects: Some(v), part: p, index });
            }
            continue;
        }
        let part = own
            .or_else(|| (objects.len() == 1).then(|| named[0]).flatten())
            .unwrap_or(Part::Extra);
        units.push(Unit { layer: layer.clone(), objects: None, part, index });
    }

    let eye_units: Vec<&Unit> = units.iter().filter(|u| matches!(u.part, Part::Eye | Part::Lid)).collect();
    if !units.iter().any(|u| u.part == Part::Eye) && may_cut {
        // H2: one drawing. Cut and rig a copy, so a failure changes nothing.
        let mut trial = scene.clone();
        let cut = super::head_cut::cut_merged_head(&mut trial, head, style)?;
        let mut report = rig_head_inner(&mut trial, head, style, false)?;
        *scene = trial;
        report.cut = true;
        report.eye_holes = cut.eye_holes;
        let mut notes = cut.notes;
        notes.append(&mut report.notes);
        report.notes = notes;
        return Ok(report);
    }
    if !units.iter().any(|u| u.part == Part::Eye) {
        return Err(format!(
            "Rig Head: '{}' has no separate eye parts (a layer or object named eye/pupil/white); a head drawn in one piece is cut by H2",
            symbol.name
        ));
    }
    if let Some(u) = eye_units.iter().find(|u| u.objects.is_none() && u.layer.frames.keyframe_count() > 1) {
        return Err(format!("Rig Head: the eye layer '{}' is animated (several keyframes); not rigged", u.layer.name));
    }

    // The eye drawings, flattened in paint order (bottom first).
    let mut eye_shapes: Vec<ShapeData> = Vec::new();
    for u in eye_units.iter().rev() {
        for o in u.rest_objects() {
            flatten_into(scene, &o, Affine::IDENTITY, &[], &mut eye_shapes, 0);
        }
    }
    let eye_shapes: Vec<ShapeData> = eye_shapes.iter().flat_map(split_disjoint).collect();
    let split = split_eyes(&eye_shapes).ok_or("Rig Head: no eye white (a light fill) in the eye parts")?;
    let regions: Vec<(BezPath, FillMode)> = eye_shapes.iter().map(painted_region).collect();
    report.eyes = split.eyes.len();

    // The view: one eye, or a far eye much smaller than the near one, is a
    // side head.
    let side = match split.eyes.len() {
        1 => true,
        _ => {
            let a: Vec<f64> = split.eyes.iter().map(|e| region_area(&e.white)).collect();
            let (lo, hi) = a.iter().fold((f64::MAX, 0.0f64), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
            lo < 0.55 * hi
        }
    };
    report.view = if side { "side" } else { "front" }.into();
    let layers_style = if side { &style.side } else { &style.front };
    let name_of = |part: HeadPart, fallback: &str| {
        layers_style.iter().find(|l| l.part == part).map(|l| l.name.clone()).unwrap_or_else(|| fallback.to_string())
    };

    let mut masks: Vec<ShapeData> = Vec::new();
    for eye in split.eyes.iter() {
        // The white's outer contours filled: a hole cut in the white under
        // the pupil is still eye, or the pupil would be clipped there.
        let opts = BooleanOptions::for_shape_size(eye.bounds.size().to_vec2().hypot());
        let mut contours: Vec<BezPath> = Vec::new();
        for el in eye.white.0.elements() {
            if matches!(el, kurbo::PathEl::MoveTo(_)) || contours.is_empty() {
                contours.push(BezPath::new());
            }
            if let Some(c) = contours.last_mut() {
                c.push(*el);
            }
        }
        let mut path = if contours.len() > 1 { buzz_geom::union_all(&contours, opts) } else { eye.white.0.clone() };
        // An eye white drawn with the iris cut out of it (a notch: well
        // short of its hull) is the whole eye under the iris, so the mask is
        // its hull there.
        let hull = hull_path(&boundary_points(&path, (eye.bounds.width().max(eye.bounds.height()) / 150.0).max(0.05)));
        let notched = !hull.elements().is_empty()
            && region_area(&(path.clone(), FillMode::NonZero)) < 0.9 * region_area(&(hull.clone(), FillMode::NonZero));
        if notched {
            path = hull;
        }
        // Less the outline drawn over the white's edge (anything dark over
        // it that reaches its edge); a dark shape inside the white, away
        // from its edge, is under the pupil and stays eye.
        let filled = (path.clone(), FillMode::NonZero);
        let edge = boundary_points(&path, (eye.bounds.width().max(eye.bounds.height()) / 200.0).max(0.05));
        let tol = (eye.bounds.height() * 0.04).max(0.75);
        let over: Vec<BezPath> = (eye.body + 1..eye_shapes.len())
            .filter(|i| split.roles[*i].0 == Role::White)
            .filter(|i| paint_colour(&eye_shapes[*i]).is_some_and(|c| !is_light(c)))
            .filter(|i| regions[*i].0.bounding_box().intersect(eye.bounds).area() > 0.0)
            .filter(|i| {
                let bb = regions[*i].0.bounding_box();
                let pts = boundary_points(&regions[*i].0, (bb.width().max(bb.height()) / 60.0).max(0.05));
                pts.iter().any(|p| !contains(&filled.0, filled.1, *p) || edge.iter().any(|q| q.distance(*p) < tol))
            })
            .map(|i| regions[i].0.clone())
            .collect();
        if !over.is_empty() {
            let cut = buzz_geom::union_all(&over, opts);
            let rest = buzz_geom::boolean(&path, &cut, BoolOp::Difference, opts);
            if !rest.elements().is_empty() {
                path = rest;
            }
        }
        let colour = paint_colour(&eye_shapes[eye.body]).unwrap_or(Color::WHITE);
        let m = ShapeData::filled(path, colour);
        masks.push(m);
    }

    // White, eyeball and mask shapes.
    let mut white: Vec<ShapeData> = Vec::new();
    let mut ball: Vec<ShapeData> = Vec::new();
    // Completed irises go under every eyeball shape, the widest lowest, and
    // only outside the mask at rest: the rest picture stays exactly as drawn
    // and the rest of the circle comes into view as the eyeball moves.
    let mut completions: Vec<(f64, ShapeData)> = Vec::new();
    for (i, s) in eye_shapes.iter().enumerate() {
        match split.roles[i].0 {
            Role::White => white.push(s.clone()),
            Role::Ball => {
                // An iris cut by the eye's edge: completed under itself.
                let k = split.roles[i].1.unwrap_or(0);
                let eye = &split.eyes[k];
                let big = region_area(&regions[i]) >= 0.04 * region_area(&eye.white);
                let dark = paint_colour(s).is_some_and(|c| !is_light(c));
                if big && dark && s.fill.is_some() {
                    if let Some((c, r)) = completed_iris(&regions[i], &eye.white) {
                        let mut circle = s.clone();
                        let disc = kurbo::Circle::new(c, r).to_path(0.02);
                        let opts = BooleanOptions::for_shape_size(eye.bounds.size().to_vec2().hypot());
                        circle.path = buzz_geom::boolean(&disc, &masks[k].path, BoolOp::Difference, opts);
                        circle.stroke = None;
                        if let Some(f) = &mut circle.fill {
                            f.rule = FillMode::NonZero;
                        }
                        completions.push((r, circle));
                        report.irises_completed += 1;
                    }
                }
                ball.push(s.clone());
            }
        }
    }
    completions.sort_by(|a, b| b.0.total_cmp(&a.0));
    let ball: Vec<ShapeData> = completions.into_iter().map(|(_, s)| s).chain(ball).collect();

    // The lid: skin sampled around the eyes on the head's other parts, the
    // line in the outline's ink.
    let mut under: Vec<ShapeData> = Vec::new();
    for u in units.iter().rev() {
        if matches!(u.part, Part::Base | Part::Extra | Part::Hair | Part::Nose) {
            for o in u.rest_objects() {
                flatten_into(scene, &o, Affine::IDENTITY, &[], &mut under, 0);
            }
        }
    }
    let eye_bounds: Vec<Rect> = split.eyes.iter().map(|e| e.bounds).collect();
    let skin = sample_skin(&under, &eye_bounds)
        .or_else(|| {
            under
                .iter()
                .filter(|s| s.fill.is_some())
                .filter_map(|s| paint_colour(s).map(|c| (c, region_area(&painted_region(s)))))
                .filter(|(c, _)| luma(*c) >= 0.15 && !is_light(*c))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(c, _)| c)
        })
        .unwrap_or(Color::from_rgba8(214, 140, 85, 255));
    let ink = white
        .iter()
        .filter_map(|s| paint_colour(s).map(|c| (c, region_area(&painted_region(s)))))
        .filter(|(c, _)| luma(*c) < 0.35)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c)
        .unwrap_or(Color::from_rgba8(20, 14, 10, 255));
    report.skin = { let p = skin.to_rgba8(); [p.r, p.g, p.b, p.a] };
    report.ink = { let p = ink.to_rgba8(); [p.r, p.g, p.b, p.a] };

    let mut lid_frames: Vec<Vec<ShapeData>> = vec![Vec::new(); style.lid.cover_per_frame.len().max(1)];
    for (k, eye) in split.eyes.iter().enumerate() {
        // The white with any other light pieces of it (a traced eye's white
        // can be several shapes): what the shut lid must cover.
        let near = eye.bounds.inflate(eye.bounds.width() * 0.3, eye.bounds.height() * 0.3);
        let lights: Vec<BezPath> = (0..eye_shapes.len())
            .filter(|i| split.roles[*i] == (Role::White, Some(k)))
            .filter(|i| paint_colour(&eye_shapes[*i]).is_some_and(is_light) && eye_shapes[*i].fill.is_some())
            .filter(|i| regions[*i].0.bounding_box().intersect(near).area() > 0.0)
            .map(|i| regions[i].0.clone())
            .collect();
        let whites = if lights.len() > 1 {
            let opts = BooleanOptions::for_shape_size(near.size().to_vec2().hypot()).fast();
            (buzz_geom::union_all(&lights, opts), FillMode::NonZero)
        } else {
            eye.white.clone()
        };
        let white_box = whites.0.bounding_box().union(eye.bounds);
        let reach = white_box.inflate(white_box.width() * 0.12, white_box.height() * 0.35);
        let own: Vec<(BezPath, FillMode)> = std::iter::once(whites.clone())
            .chain(
                (0..eye_shapes.len())
                    .filter(|i| split.roles[*i] == (Role::White, Some(k)) && *i != eye.body)
                    .filter(|i| regions[*i].0.bounding_box().intersect(reach).area() > 0.0)
                    .map(|i| regions[i].clone()),
            )
            .collect();
        let silhouette = own.iter().fold(white_box, |r, p| r.union(p.0.bounding_box())).intersect(reach);
        let step = (eye.bounds.height() / 60.0).clamp(0.1, 2.0);
        let prof = profile(&own, silhouette, step);
        let margin = (step * 1.5).max(1.0);
        let line = (eye.bounds.height() * 0.05).clamp(1.0, 3.0);
        for (f, cover) in style.lid.cover_per_frame.iter().enumerate() {
            if *cover <= 0.0 {
                continue;
            }
            let t = if *cover >= 0.999 {
                1.0
            } else {
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..24 {
                    let mid = (lo + hi) / 2.0;
                    if lid_cover(&prof, &whites, mid) < *cover { lo = mid } else { hi = mid }
                }
                (lo + hi) / 2.0
            };
            let mut p2 = Profile { x0: prof.x0, step: prof.step, top: prof.top.clone(), bottom: prof.bottom.clone() };
            if t >= 1.0 {
                // Shut: past the bottom, so no white shows at the edge.
                p2.bottom = p2.bottom.iter().map(|b| b.map(|y| y + margin * 0.5)).collect();
            }
            lid_frames[f].extend(lid_shapes(&p2, t, margin, skin, ink, line));
        }
    }

    // The symbols.
    let folder = symbol.folder.clone();
    let folder = folder.as_deref();
    let base_name = symbol.name.clone();
    let group = split.eyes.iter().fold(split.eyes[0].bounds, |r, e| r.union(e.bounds));
    let pivot = group.center();
    let white_sym = shapes_symbol(scene, &format!("{base_name} eyeWhite"), folder, vec![white]);
    let mask_sym = shapes_symbol(scene, &format!("{base_name} eyeMask"), folder, vec![masks]);
    let ball_sym = shapes_symbol(scene, &format!("{base_name} eyeBall"), folder, vec![ball]);
    let lid_sym = shapes_symbol(scene, &format!("{base_name} eyeLid"), folder, lid_frames);
    report.eye_white = Some(white_sym);
    report.eye_mask = Some(mask_sym);
    report.eye_ball = Some(ball_sym);
    report.eye_lid = Some(lid_sym);

    let mut eye_layers: Vec<(HeadPart, Layer)> = Vec::new();
    for (part, sym, kind) in [
        (HeadPart::EyeLid, lid_sym, LayerKind::Normal),
        (HeadPart::EyeMask, mask_sym, LayerKind::Mask),
        (HeadPart::EyeBall, ball_sym, LayerKind::Masked),
        (HeadPart::EyeWhite, white_sym, LayerKind::Normal),
    ] {
        let fallback = match part {
            HeadPart::EyeLid => "eyeLid",
            HeadPart::EyeMask => "eyeMask",
            HeadPart::EyeBall => "eyeBall",
            _ => "eyeWhite",
        };
        let style_kind = layers_style.iter().find(|l| l.part == part).map(|l| l.kind);
        let kind = match style_kind {
            Some(HeadLayerKind::Mask) => LayerKind::Mask,
            Some(HeadLayerKind::Masked) => LayerKind::Masked,
            Some(HeadLayerKind::Normal) => LayerKind::Normal,
            None => kind,
        };
        let inst = rest_instance(scene, sym, pivot);
        eye_layers.push((part, new_layer(scene, &name_of(part, fallback), kind, vec![inst], length)));
    }

    // Brows: the one further right on screen is the right brow.
    let brow_x = |u: &Unit| -> Option<f64> {
        let mut s = Vec::new();
        for o in u.rest_objects() {
            flatten_into(scene, &o, Affine::IDENTITY, &[], &mut s, 0);
        }
        s.iter().map(|x| x.path.bounding_box()).reduce(|a, b| a.union(b)).map(|r| r.center().x)
    };
    let head_centre = {
        let mut all = Vec::new();
        for u in &units {
            for o in u.rest_objects() {
                flatten_into(scene, &o, Affine::IDENTITY, &[], &mut all, 0);
            }
        }
        all.iter().map(|x| x.path.bounding_box()).reduce(|a, b| a.union(b)).map(|r| r.center().x).unwrap_or(0.0)
    };
    // Between the eyes of a front head; the head's middle for a side one.
    let eyes_centre = (split.eyes.len() >= 2).then(|| pivot.x);
    let brows: Vec<(usize, Option<f64>)> =
        units.iter().enumerate().filter(|(_, u)| u.part == Part::Brow).map(|(i, u)| (i, brow_x(u))).collect();
    let brow_part = |i: usize| -> HeadPart {
        // Screen geometry first; a brow with nothing drawn at rest goes by
        // the side its name says.
        let x = brows.iter().find(|(j, _)| *j == i).and_then(|(_, x)| *x);
        let others: Vec<f64> = brows.iter().filter(|(j, _)| *j != i).filter_map(|(_, x)| *x).collect();
        let right = match x {
            Some(x) if others.is_empty() => x >= eyes_centre.unwrap_or(head_centre),
            Some(x) => others.iter().all(|o| x >= *o),
            None => buzz_rig::pattern::name_side(&units[i].layer.name) != Some(buzz_rig::pattern::Side::Left),
        };
        if right { HeadPart::RightBrow } else { HeadPart::LeftBrow }
    };

    // The topmost base layer: what was below it stays below it.
    let base_index = units.iter().filter(|u| u.part == Part::Base).map(|u| u.index).min();
    let below_base = |u: &Unit| base_index.is_some_and(|b| u.index > b) && u.part != Part::Base;

    // Each kept unit as a layer, with its new name.
    let mut used_names: Vec<String> = Vec::new();
    let mut slots: Vec<(HeadPart, Layer)> = Vec::new();
    let mut sound: Vec<Layer> = Vec::new();
    let mut bottom: Vec<Layer> = Vec::new();
    let mut mouth_symbol = None;
    for (i, u) in units.iter().enumerate() {
        if matches!(u.part, Part::Eye | Part::Lid) {
            continue;
        }
        let part = match u.part {
            Part::Brow => brow_part(i),
            Part::Moustache | Part::Extra => HeadPart::Moustache,
            Part::Mouth => HeadPart::Mouth,
            Part::Nose => HeadPart::Nose,
            Part::Hair => HeadPart::Hair,
            _ => HeadPart::Base,
        };
        let wanted = match u.part {
            Part::Extra => u.layer.name.clone(),
            _ => name_of(part, &u.layer.name),
        };
        let name = if used_names.contains(&wanted) { u.layer.name.clone() } else { wanted };
        used_names.push(name.clone());
        let mut layer = match &u.objects {
            Some(objects) => new_layer(scene, &name, LayerKind::Normal, objects.clone(), length),
            None => {
                let mut l = (*u.layer).clone();
                l.name = name;
                l
            }
        };
        layer.parent = None;
        layer.follows = None;
        layer.follows_bone = None;
        layer.rest_pose = None;
        if u.part == Part::Mouth {
            // A mouth-shape symbol is kept; a drawing becomes one, its frame
            // 0 the mouth as it is.
            let rest = layer.objects_at(0).to_vec();
            let shapes_symbol_kept = match rest.as_slice() {
                [one] => one.instance().and_then(|i| scene.library().get(i.symbol).map(|s| (i.symbol, s.length()))),
                _ => None,
            };
            match shapes_symbol_kept {
                Some((id, len)) if len >= 2 || layer.frames.keyframe_count() > 1 => {
                    mouth_symbol = Some(id);
                }
                Some((id, _)) if scene.library().get(id).is_some_and(|s| s.name.to_lowercase().contains("mouth")) => {
                    mouth_symbol = Some(id);
                }
                _ if layer.frames.keyframe_count() <= 1 && !rest.is_empty() => {
                    let id = scene.add_symbol(format!("{base_name} mouth"), SymbolKind::Graphic, folder);
                    let inner: Vec<Arc<Object>> = rest.clone();
                    let bounds = rest
                        .iter()
                        .flat_map(|o| {
                            let mut s = Vec::new();
                            flatten_into(scene, o, Affine::IDENTITY, &[], &mut s, 0);
                            s
                        })
                        .map(|s| s.path.bounding_box())
                        .reduce(|a, b| a.union(b));
                    scene.library_mut().update(id, |s| {
                        let first = s.layers.iter().next().map(|l| l.id);
                        if let Some(l) = first {
                            s.layers.update(l, |l| {
                                let mut k = Keyframe::new(0);
                                k.objects = Arc::new(inner);
                                l.frames = LayerTimeline::from_parts(vec![k], 1);
                            });
                        }
                    });
                    let inst = rest_instance(scene, id, bounds.map(|b| b.center()).unwrap_or_default());
                    let mut k = Keyframe::new(0);
                    k.objects = Arc::new(vec![inst]);
                    layer.frames = LayerTimeline::from_parts(vec![k], length);
                    mouth_symbol = Some(id);
                }
                _ => report.notes.push(format!("the mouth layer '{}' is animated: kept as it is", u.layer.name)),
            }
        }
        if u.part == Part::Sound {
            sound.push(layer);
        } else if below_base(u) {
            bottom.push(layer);
        } else {
            slots.push((part, layer));
        }
    }
    report.mouth = mouth_symbol;
    if mouth_symbol.is_none() && !units.iter().any(|u| u.part == Part::Mouth) {
        report.notes.push("no separate mouth part: the mouth stays in the base".into());
    }

    // The new stack, top first: sound, then the style's order, then what was
    // under the base.
    let mut rows: Vec<Layer> = sound;
    for style_layer in layers_style {
        for (part, layer) in eye_layers.iter().filter(|(p, _)| *p == style_layer.part) {
            let _ = part;
            rows.push(layer.clone());
        }
        // Kept units in their original order within the slot.
        for (_, layer) in slots.iter().filter(|(p, _)| *p == style_layer.part) {
            rows.push(layer.clone());
        }
    }
    for (p, layer) in &slots {
        if !layers_style.iter().any(|l| l.part == *p) {
            rows.push(layer.clone());
        }
    }
    rows.extend(bottom);
    report.layers = rows.iter().map(|l| l.name.clone()).collect();
    let mut stack = LayerStack::new();
    for (i, layer) in rows.into_iter().enumerate() {
        stack.insert(i, layer);
    }
    scene.library_mut().update(head, |s| s.layers = stack);
    Ok(report)
}

/// The head symbol Rig Head works on for `target`: `target` itself when it
/// has separate face parts or an eye stack, else the head symbol on its
/// layers (`characterSetup.jsfl`'s rule).
pub fn head_symbol_for(scene: &Scene, target: SymbolId) -> Option<SymbolId> {
    let symbol = scene.library().get(target)?;
    let is_head = already_rigged(&symbol.layers)
        || symbol.layers.iter().any(|l| classify(&l.name) == Some(Part::Eye))
        || symbol.layers.iter().any(|l| {
            l.objects_at(0).iter().any(|o| classify(&object_label(scene, o)) == Some(Part::Eye))
        });
    if is_head {
        return Some(target);
    }
    // A head drawn in one piece (H2): named a head, with no head inside.
    super::head_symbol_on(scene, &symbol.layers).filter(|h| *h != target).or_else(|| {
        let n = symbol.name.to_lowercase();
        (n.contains("head") || n.contains("face")).then_some(target)
    })
}

/// What Rig Head is aimed at: the head in `symbol` (a selected instance's
/// symbol) when given, else in the symbol being edited, else the head
/// instance on the stage's layers.
pub fn head_target(scene: &Scene, symbol: Option<SymbolId>) -> Option<SymbolId> {
    symbol
        .or_else(|| scene.editing_symbol())
        .and_then(|s| head_symbol_for(scene, s))
        .or_else(|| super::head_symbol_on(scene, scene.layers()))
}
