//! **H2: a head drawn in one piece** (`docs/HEAD_RIG_PLAN.md`): before Rig
//! Head (H1) can build the user's structure, the face parts are cut out of
//! the drawing by colour and place, and the head symbol's layers are replaced
//! by one layer per part (`Eyes`, `Brow R`/`Brow L`, `Moustache`, `Mouth`,
//! `Nose`, `Hair`, `Base`), which H1 then rigs.
//!
//! * **Eyes**: a light fill with a darker shape drawn over it and lying in
//!   it (the iris/pupil), both eyes a pair of similar size side by side (one
//!   eye: a side head, the far eye stays in the base). With them go what is
//!   drawn over the white near it (pupils, highlights, lashes) and the dark
//!   outline drawn under it.
//! * **Eye holes** (the Cast heads, whose eyes sit behind the head): an
//!   enclosed see-through hole of an eye's shape is the socket. The eye is
//!   made there: the hole filled white, an outline in the head's ink along its
//!   edge when the edge has none, and a round iris (the style's eyeball area,
//!   or the drawn eye's when the other eye is drawn) in the head's darkest
//!   ink, centred.
//! * **Brows**: a dark elongated fill above each eye, with what lies on it.
//! * **Moustache**: a wide, filled shape in the brows' colour above the
//!   mouth. **Mouth**: dark or red fills in the lower face, with the teeth
//!   and what lies on them. **Nose**: the shapes between the eyes and the
//!   mouth, only when they are drawn over the skin (a separate region), else
//!   they stay in the base. **Hair**: only when it is drawn over the skin of
//!   the head (like a separate hair or turban drawing), else it stays in the
//!   base.
//!
//! The rest picture does not change: every shape keeps its paint order inside
//! its part, and a shape of a lower layer drawn over a part is cut along the
//! part's outline, the piece over the part going with it. The base is filled
//! under each cut part with the skin colour found around it (no outline is
//! redrawn), at the bottom of the base, so moving a part shows skin, not a
//! hole.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, BoolOp, BooleanOptions, FillMode, Point, Rect, Shape as _};
use buzz_rig::head_style::{HeadPart, HeadStyle};
use buzz_scene::{Keyframe, Layer, LayerId, LayerKind, LayerTimeline, Object, Paint, Scene, ShapeData, SymbolId};
use peniko::Color;

use super::head_rig::{
    Grid, boundary_points, clip_shape, contains, flatten_stack, hull_path, is_light, luma, paint_colour, painted_region,
    region_area, rgba, share_inside, split_disjoint, stroke_outline,
};

/// What the cut did.
#[derive(Debug, Clone, Default)]
pub(super) struct HeadCut {
    /// Eyes made from see-through eye holes.
    pub eye_holes: usize,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    Eyes,
    BrowR,
    BrowL,
    Moustache,
    Mouth,
    Nose,
    Hair,
    Base,
}

impl Cut {
    fn layer_name(self) -> &'static str {
        match self {
            Cut::Eyes => "Eyes",
            Cut::BrowR => "Brow R",
            Cut::BrowL => "Brow L",
            Cut::Moustache => "Moustache",
            Cut::Mouth => "Mouth",
            Cut::Nose => "Nose",
            Cut::Hair => "Hair",
            Cut::Base => "Base",
        }
    }
}

/// A shape on its way to a part.
#[derive(Clone)]
struct Piece {
    shape: ShapeData,
    /// Paint order (bottom first); a cut piece keeps its shape's.
    order: f64,
    part: Cut,
    /// Found as the part itself (not a piece cut off a lower shape).
    seed: bool,
}

fn colour_distance(a: Color, b: Color) -> f64 {
    let (a, b) = (rgba(a), rgba(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Lips, tongue, the inside of a mouth: pink, red or purple (not the orange
/// of skin).
fn is_reddish(c: Color) -> bool {
    let [r, g, b, _] = rgba(c);
    (r > g * 1.15 && b >= g * 0.85 && r > 0.2) || (r > 2.0 * g && r > 2.0 * b && r > 0.25)
}

fn is_dark(c: Color) -> bool {
    luma(c) < 0.3
}

/// Skin: a warm mid tone.
fn is_skin_like(c: Color) -> bool {
    let [r, g, b, a] = rgba(c);
    a > 0.5 && !is_light(c) && (0.25..0.92).contains(&luma(c)) && r >= g && r > b + 0.06 && !is_reddish(c)
}

fn is_image(s: &ShapeData) -> bool {
    matches!(s.fill.as_ref().map(|f| &f.paint), Some(Paint::Image(_)))
}

/// A region as a non-overlapping path (an even-odd fill made plain), for
/// unions.
fn plain(region: &(BezPath, FillMode)) -> BezPath {
    match region.1 {
        FillMode::NonZero => region.0.clone(),
        FillMode::EvenOdd => {
            let mut o = BooleanOptions::for_shape_size(region.0.bounding_box().size().to_vec2().hypot()).fast();
            o.fill = FillMode::EvenOdd;
            buzz_geom::boolean(&region.0, &BezPath::new(), BoolOp::Union, o)
        }
    }
}

fn union_of(paths: &[BezPath]) -> BezPath {
    let paths: Vec<BezPath> = paths.iter().filter(|p| !p.elements().is_empty()).cloned().collect();
    match paths.len() {
        0 => BezPath::new(),
        1 => paths[0].clone(),
        _ => buzz_geom::union_all(&paths, buzz_geom::boolean::options_for(&paths).fast()),
    }
}

fn opts_for(r: Rect) -> BooleanOptions {
    BooleanOptions::for_shape_size(r.size().to_vec2().hypot()).fast()
}

/// Share of `inner`'s box inside `outer`.
fn box_inside(inner: Rect, outer: Rect) -> f64 {
    let a = inner.area();
    if a <= 0.0 {
        return if outer.contains(inner.center()) { 1.0 } else { 0.0 };
    }
    inner.intersect(outer).area() / a
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

// ---------------------------------------------------------------------------
// Scanline coverage
// ---------------------------------------------------------------------------

/// Marks the cells of `g` whose centres `path` covers.
fn scan(path: &BezPath, rule: FillMode, g: &Grid, out: &mut [bool]) {
    let mut edges: Vec<(f64, f64, f64, f64, i32)> = Vec::new();
    {
        let mut push = |a: Point, b: Point| {
            if (a.y - b.y).abs() < 1e-12 {
                return;
            }
            let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            edges.push((lo.y, hi.y, lo.x, (hi.x - lo.x) / (hi.y - lo.y), dir));
        };
        let (mut start, mut last): (Option<Point>, Option<Point>) = (None, None);
        kurbo::flatten(path.iter(), (g.cell * 0.1).max(1e-3), |el| match el {
            kurbo::PathEl::MoveTo(p) => {
                if let (Some(s), Some(l)) = (start, last) {
                    push(l, s);
                }
                start = Some(p);
                last = Some(p);
            }
            kurbo::PathEl::LineTo(p) => {
                if let Some(l) = last {
                    push(l, p);
                }
                last = Some(p);
            }
            kurbo::PathEl::ClosePath => {
                if let (Some(s), Some(l)) = (start, last) {
                    push(l, s);
                }
                last = start;
            }
            _ => {}
        });
        if let (Some(s), Some(l)) = (start, last) {
            push(l, s);
        }
    }
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut xs: Vec<(f64, i32)> = Vec::new();
    for r in 0..g.rows {
        let y = g.origin.y + (r as f64 + 0.5) * g.cell;
        while next < edges.len() && edges[next].0 <= y {
            active.push(next);
            next += 1;
        }
        active.retain(|&e| edges[e].1 > y);
        if active.is_empty() {
            if next >= edges.len() {
                break;
            }
            continue;
        }
        xs.clear();
        for &e in &active {
            let (y0, _, x0, k, d) = edges[e];
            xs.push((x0 + (y - y0) * k, d));
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut w = 0;
        for i in 0..xs.len() {
            w += xs[i].1;
            let inside = match rule {
                FillMode::NonZero => w != 0,
                FillMode::EvenOdd => w % 2 != 0,
            };
            if inside && i + 1 < xs.len() {
                let c0 = ((xs[i].0 - g.origin.x) / g.cell - 0.5).ceil().max(0.0) as usize;
                let c1 = (((xs[i + 1].0 - g.origin.x) / g.cell - 0.5).ceil().max(0.0) as usize).min(g.cols);
                for c in c0..c1 {
                    out[r * g.cols + c] = true;
                }
            }
        }
    }
}

fn coverage(regions: &[&(BezPath, FillMode)], g: &Grid) -> Vec<bool> {
    let mut out = vec![false; g.cols * g.rows];
    for r in regions {
        scan(&r.0, r.1, g, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// Eyes
// ---------------------------------------------------------------------------

/// An eye found in the drawing.
struct EyeFound {
    /// Its white's shapes (indices into the flattened shapes).
    white: Vec<usize>,
    bounds: Rect,
    area: f64,
    /// Made from an eye hole: the hole's outline.
    hole: Option<BezPath>,
    /// A darker shape (iris/pupil) is drawn over the white and lies in it.
    iris: bool,
    /// A see-through eye hole, its outline not worked out yet.
    socket: Option<Hole>,
}

/// Is a shape's region walled in by other paint all round (most points
/// just outside its edge covered by another shape)? An eye white is (by the
/// skin or its outline); a light patch on the drawing's edge (a scarf, a
/// collar) is not.
fn enclosed(own: usize, regions: &[(BezPath, FillMode)], boxes: &[Rect]) -> bool {
    let r = &regions[own];
    let b = boxes[own];
    let centre = b.center();
    let pts = boundary_points(&r.0, (b.width() + b.height()) / 60.0);
    let step = (b.width().max(b.height()) * 0.08).max(0.5);
    let (mut n, mut covered) = (0usize, 0usize);
    for p in pts.iter().step_by((pts.len() / 48).max(1)) {
        let v = *p - centre;
        let len = v.hypot();
        if len <= 1e-9 {
            continue;
        }
        let q = *p + v * (step / len);
        n += 1;
        if (0..regions.len()).any(|j| j != own && boxes[j].contains(q) && contains(&regions[j].0, regions[j].1, q)) {
            covered += 1;
        }
    }
    n > 0 && covered as f64 >= 0.9 * n as f64
}

/// The eye candidates: light fills of an eye's size and shape walled in by
/// the drawing, with a darker shape drawn over them and lying in them (the
/// iris/pupil) or blank (an eye with no pupil, `iris` false; at least 0.2 %
/// of the head), and see-through eye holes (`socket`).
fn find_eyes(shapes: &[ShapeData], regions: &[(BezPath, FillMode)], areas: &[f64], head: Rect) -> Vec<EyeFound> {
    let boxes: Vec<Rect> = regions.iter().map(|r| r.0.bounding_box()).collect();
    let mut cands: Vec<(usize, bool)> = Vec::new();
    for i in 0..shapes.len() {
        let s = &shapes[i];
        if s.fill.is_none() || !paint_colour(s).is_some_and(is_light) {
            continue;
        }
        let (a, b) = (areas[i], boxes[i]);
        if a < 0.0004 * head.area() || a > 0.06 * head.area() || b.area() <= 0.0 {
            continue;
        }
        let aspect = b.width() / b.height().max(1e-9);
        if a / b.area() < 0.35 || !(0.3..=6.0).contains(&aspect) {
            continue;
        }
        let iris = (i + 1..shapes.len()).any(|j| {
            shapes[j].fill.is_some()
                && paint_colour(&shapes[j]).is_some_and(|c| !is_light(c))
                && overlaps(boxes[j], b)
                && (0.02 * a..=0.95 * a).contains(&areas[j])
                && share_inside(&regions[j], &regions[i]) >= 0.5
        });
        if (iris || a >= 0.002 * head.area()) && enclosed(i, regions, &boxes) {
            cands.push((i, iris));
        }
    }
    // Pieces of one white (a traced eye) go together.
    let mut eyes: Vec<EyeFound> = Vec::new();
    for (i, iris) in cands {
        match eyes.iter_mut().find(|e| overlaps(e.bounds, boxes[i])) {
            Some(e) => {
                e.white.push(i);
                e.bounds = e.bounds.union(boxes[i]);
                e.area += areas[i];
                e.iris |= iris;
            }
            None => eyes.push(EyeFound { white: vec![i], bounds: boxes[i], area: areas[i], hole: None, iris, socket: None }),
        }
    }
    for h in find_holes(regions, head) {
        if eyes.iter().any(|e| overlaps(e.bounds, h.bounds)) {
            continue;
        }
        eyes.push(EyeFound { white: Vec::new(), bounds: h.bounds, area: h.area, hole: None, iris: false, socket: Some(h) });
    }
    eyes
}

/// The two eyes of a front head: the best pair (side by side, of a size;
/// a pair with no pupil at all also symmetric about the face's midline
/// `mid`), else the biggest eye with a pupil alone (a side head).
fn pick_eyes(mut eyes: Vec<EyeFound>, mid: f64) -> Vec<EyeFound> {
    let mut best: Option<(usize, usize, f64)> = None;
    for a in 0..eyes.len() {
        for b in a + 1..eyes.len() {
            if let Some(score) = pair_score(&eyes[a], &eyes[b], mid) {
                if best.is_none_or(|(_, _, s)| score > s) {
                    best = Some((a, b, score));
                }
            }
        }
    }
    match best {
        Some((a, b, _)) => {
            let eb = eyes.swap_remove(b);
            let ea = eyes.swap_remove(a);
            vec![ea, eb]
        }
        None => {
            eyes.retain(|e| e.iris);
            eyes.sort_by(|a, b| b.area.total_cmp(&a.area));
            eyes.truncate(1);
            eyes
        }
    }
}

fn pair_score(a: &EyeFound, b: &EyeFound, mid: f64) -> Option<f64> {
    let ratio = a.area.min(b.area) / a.area.max(b.area).max(1e-9);
    let (ca, cb) = (a.bounds.center(), b.bounds.center());
    let w = a.bounds.width().max(b.bounds.width());
    let h = a.bounds.height().max(b.bounds.height());
    let (dx, dy) = ((ca.x - cb.x).abs(), (ca.y - cb.y).abs());
    if !(ratio >= 0.3 && dy <= 0.75 * h + 0.1 * w && dx >= 0.8 * w && dx <= 6.0 * w) {
        return None;
    }
    // Off the face's midline, in eye spacings.
    let off = ((ca.x + cb.x) / 2.0 - mid).abs() / dx.max(1e-9);
    let irises = a.iris as u8 + b.iris as u8;
    if irises == 0 && off > 1.0 {
        return None;
    }
    Some((a.area + b.area) * (1.0 + 0.5 * irises as f64) / (1.0 + off))
}

/// An enclosed see-through hole in the drawing.
struct Hole {
    bounds: Rect,
    area: f64,
    /// Its convex hull, a cell larger all round.
    hull: BezPath,
}

fn find_holes(regions: &[(BezPath, FillMode)], head: Rect) -> Vec<Hole> {
    let g = Grid::over(head.inflate(2.0, 2.0), (head.width().max(head.height()) / 400.0).max(0.05));
    let refs: Vec<&(BezPath, FillMode)> = regions.iter().collect();
    let cover = coverage(&refs, &g);
    let (cols, rows) = (g.cols, g.rows);
    // 0 unknown, 1 outside, 2.. hole labels.
    let mut label = vec![0usize; cols * rows];
    let mut stack: Vec<usize> = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            if (r == 0 || c == 0 || r + 1 == rows || c + 1 == cols) && !cover[r * cols + c] {
                stack.push(r * cols + c);
            }
        }
    }
    let flood = |label: &mut Vec<usize>, stack: &mut Vec<usize>, mark: usize, members: &mut Vec<usize>| {
        while let Some(i) = stack.pop() {
            if label[i] != 0 || cover[i] {
                continue;
            }
            label[i] = mark;
            members.push(i);
            let (r, c) = (i / cols, i % cols);
            if r > 0 {
                stack.push(i - cols);
            }
            if r + 1 < rows {
                stack.push(i + cols);
            }
            if c > 0 {
                stack.push(i - 1);
            }
            if c + 1 < cols {
                stack.push(i + 1);
            }
        }
    };
    let mut outside = Vec::new();
    flood(&mut label, &mut stack, 1, &mut outside);
    let mut holes = Vec::new();
    let mut mark = 2;
    for start in 0..cols * rows {
        if label[start] != 0 || cover[start] {
            continue;
        }
        let mut members = Vec::new();
        stack.push(start);
        flood(&mut label, &mut stack, mark, &mut members);
        mark += 1;
        let (mut c0, mut c1, mut r0, mut r1) = (usize::MAX, 0, usize::MAX, 0);
        for &i in &members {
            let (r, c) = (i / cols, i % cols);
            c0 = c0.min(c);
            c1 = c1.max(c);
            r0 = r0.min(r);
            r1 = r1.max(r);
        }
        let n = members.len() as f64;
        let (bw, bh) = ((c1 - c0 + 1) as f64, (r1 - r0 + 1) as f64);
        let area = n * g.cell * g.cell;
        if area < 0.0005 * head.area() || area > 0.06 * head.area() || bw < 3.0 || bh < 3.0 {
            continue;
        }
        let aspect = bw / bh;
        if n / (bw * bh) < 0.4 || !(0.3..=4.0).contains(&aspect) {
            continue;
        }
        let mut corners = Vec::new();
        for &i in &members {
            let (r, c) = (i / cols, i % cols);
            let edge = r == 0
                || c == 0
                || r + 1 == rows
                || c + 1 == cols
                || label[i - cols] != label[i]
                || label[i + cols] != label[i]
                || label[i - 1] != label[i]
                || label[i + 1] != label[i];
            if edge {
                let p = g.centre(c, r);
                let k = g.cell * 1.5;
                corners.extend([
                    Point::new(p.x - k, p.y - k),
                    Point::new(p.x + k, p.y - k),
                    Point::new(p.x - k, p.y + k),
                    Point::new(p.x + k, p.y + k),
                ]);
            }
        }
        let hull = hull_path(&corners);
        if hull.elements().is_empty() {
            continue;
        }
        let o = g.origin;
        let bounds = Rect::new(
            o.x + c0 as f64 * g.cell,
            o.y + r0 as f64 * g.cell,
            o.x + (c1 + 1) as f64 * g.cell,
            o.y + (r1 + 1) as f64 * g.cell,
        );
        holes.push(Hole { bounds, area, hull });
    }
    holes
}

/// The hole's exact outline: its hull less every shape around it, the
/// biggest piece left.
fn hole_outline(hole: &Hole, regions: &[(BezPath, FillMode)]) -> Option<BezPath> {
    let hb = hole.hull.bounding_box();
    let mut path = hole.hull.clone();
    for r in regions.iter().filter(|r| overlaps(r.0.bounding_box(), hb)) {
        let o = opts_for(hb.union(r.0.bounding_box()));
        let p = plain(r);
        path = buzz_geom::boolean(&path, &p, BoolOp::Difference, o);
        if path.elements().is_empty() {
            return None;
        }
    }
    let pieces = split_disjoint(&ShapeData::filled(path, Color::WHITE));
    pieces
        .into_iter()
        .map(|s| {
            let a = region_area(&(s.path.clone(), FillMode::NonZero));
            (s.path, a)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, a)| *a >= 0.3 * hole.area)
        .map(|(p, _)| p)
}

/// The colour of the topmost filled shape at `q`.
fn colour_at(shapes: &[ShapeData], regions: &[(BezPath, FillMode)], q: Point) -> Option<Color> {
    (0..shapes.len())
        .rev()
        .find(|i| shapes[*i].fill.is_some() && contains(&regions[*i].0, regions[*i].1, q))
        .and_then(|i| paint_colour(&shapes[i]))
}

/// Is the hole's edge drawn in ink (dark just outside it)?
fn edge_has_ink(outline: &BezPath, centre: Point, shapes: &[ShapeData], regions: &[(BezPath, FillMode)]) -> bool {
    let b = outline.bounding_box();
    let pts = boundary_points(outline, (b.width() + b.height()) / 60.0);
    let step = (b.width().max(b.height()) * 0.04).max(0.5);
    let mut dark = 0;
    let mut n = 0;
    for p in pts.iter().step_by((pts.len() / 48).max(1)) {
        let v = *p - centre;
        let len = v.hypot();
        if len <= 1e-9 {
            continue;
        }
        let q = *p + v * (step / len);
        n += 1;
        if colour_at(shapes, regions, q).is_some_and(is_dark) {
            dark += 1;
        }
    }
    n > 0 && dark as f64 >= 0.6 * n as f64
}

// ---------------------------------------------------------------------------
// The cut
// ---------------------------------------------------------------------------

/// The flattened drawing of a head with no separate face parts.
struct Drawing {
    shapes: Vec<ShapeData>,
    /// Layers left as they are (not drawn: guides, hidden, sound).
    kept: Vec<Layer>,
    length: u32,
}

fn flatten_head(scene: &Scene, head: SymbolId) -> Result<Drawing, String> {
    let symbol = scene.library().get(head).ok_or("Rig Head: no such symbol")?;
    let mut kept = Vec::new();
    for layer in symbol.layers.iter() {
        match layer.kind {
            LayerKind::Folder | LayerKind::Mask | LayerKind::InverseMask => continue,
            _ => {}
        }
        let drawn = layer.kind != LayerKind::Guide && symbol.layers.is_effectively_visible(layer.id) && !layer.objects_at(0).is_empty();
        if !drawn {
            kept.push((**layer).clone());
        } else if layer.frames.keyframe_count() > 1 {
            return Err(format!("Rig Head: '{}' is drawn in one piece but its layer '{}' is animated; not cut", symbol.name, layer.name));
        }
    }
    let mut shapes = Vec::new();
    flatten_stack(scene, &symbol.layers, 0, Affine::IDENTITY, &[], &mut shapes, 0);
    if shapes.iter().any(is_image) {
        return Err(format!("Rig Head: '{}' is a bitmap; bitmap heads are not cut", symbol.name));
    }
    let shapes: Vec<ShapeData> = shapes.iter().flat_map(split_disjoint).filter(|s| !s.path.elements().is_empty()).collect();
    if shapes.is_empty() {
        return Err(format!("Rig Head: '{}' draws nothing", symbol.name));
    }
    Ok(Drawing { shapes, kept, length: symbol.length().max(1) })
}

/// The colour covering most area among `pred` fills.
fn dominant(shapes: &[ShapeData], areas: &[f64], pred: impl Fn(Color) -> bool) -> Option<Color> {
    let mut tally: Vec<(Color, f64)> = Vec::new();
    for (s, a) in shapes.iter().zip(areas) {
        if s.fill.is_none() {
            continue;
        }
        let Some(c) = paint_colour(s) else { continue };
        if !pred(c) {
            continue;
        }
        match tally.iter_mut().find(|(k, _)| colour_distance(*k, c) < 0.03) {
            Some((_, t)) => *t += a,
            None => tally.push((c, *a)),
        }
    }
    tally.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(c, _)| c)
}

/// How many eyes of `head` would be made from eye holes (a quick look, for
/// the AI rig review): 0 when the head has drawn eyes or no holes.
pub(super) fn eye_holes_in(scene: &Scene, head: SymbolId) -> usize {
    let Ok(d) = flatten_head(scene, head) else { return 0 };
    let regions: Vec<(BezPath, FillMode)> = d.shapes.iter().map(painted_region).collect();
    let areas: Vec<f64> = regions.iter().map(region_area).collect();
    let hb = regions.iter().map(|r| r.0.bounding_box()).reduce(|a, b| a.union(b)).unwrap_or_default();
    pick_eyes(find_eyes(&d.shapes, &regions, &areas, hb), hb.center().x).iter().filter(|e| e.socket.is_some()).count()
}

/// A brow drawn into the hair: the dark mass's lower edge found straight
/// above the eye (marching up from the eye's top, at most two eye heights,
/// across the eye's width), and a band a third of an eye high along it cut
/// off every dark shape there (hair, its ink, a brow joined with a strand).
/// Per shape cut: (index, the band, what is left of it). Empty when no dark
/// mass lies over most of the eye's width.
fn brow_band(
    e: &EyeFound,
    shapes: &[ShapeData],
    regions: &[(BezPath, FillMode)],
    areas: &[f64],
    boxes: &[Rect],
    part: &[Option<Cut>],
    remainder: &[Option<ShapeData>],
) -> Vec<(usize, ShapeData, Option<ShapeData>)> {
    let (w, h) = (e.bounds.width(), e.bounds.height());
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let step = (h / 40.0).max(0.02);
    let thick = 0.3 * h;
    let dark = |i: usize| paint_colour(&shapes[i]).is_some_and(|c| luma(c) < 0.35 && !is_reddish(c) && !is_skin_like(c));
    // Dark shapes there not taken as a brow of their own: the hair, its
    // ink, or a brow joined with a strand of it (no specks).
    let mass = |i: usize| part[i].is_none() && dark(i) && areas[i] >= 0.12 * e.area;
    let n = 24;
    let mut edge: Vec<Option<f64>> = Vec::with_capacity(n + 1);
    for k in 0..=n {
        let x = e.bounds.x0 - 0.05 * w + 1.1 * w * k as f64 / n as f64;
        let mut y = e.bounds.y0 - step;
        let mut found = None;
        // The topmost shape at a point.
        let topmost = |q: Point| {
            (0..shapes.len()).rev().find(|j| {
                (shapes[*j].fill.is_some() || shapes[*j].stroke.is_some())
                    && boxes[*j].contains(q)
                    && contains(&regions[*j].0, regions[*j].1, q)
            })
        };
        while y >= e.bounds.y0 - 2.0 * h {
            let (here, above) = (topmost(Point::new(x, y)), topmost(Point::new(x, y - 0.15 * h)));
            // A band, not a thin line (the eye's own rim in the ink).
            if here.is_some_and(mass) && above.is_some_and(dark) {
                found = Some(y);
                break;
            }
            y -= step;
        }
        edge.push(found);
    }
    // The longest run of columns with the mass over them.
    let (mut best, mut run) = ((0usize, 0usize), (0usize, 0usize));
    for (k, v) in edge.iter().enumerate() {
        if v.is_some() {
            run = if run.1 == 0 { (k, 1) } else { (run.0, run.1 + 1) };
            if run.1 > best.1 {
                best = run;
            }
        } else {
            run = (0, 0);
        }
    }
    if best.1 < n / 2 + 1 {
        return Vec::new();
    }
    let cols: Vec<(f64, f64)> = (best.0..best.0 + best.1)
        .map(|k| (e.bounds.x0 - 0.05 * w + 1.1 * w * k as f64 / n as f64, edge[k].unwrap_or(e.bounds.y0)))
        .collect();
    let mut strip = BezPath::new();
    strip.move_to(Point::new(cols[0].0, cols[0].1 + step));
    for (x, y) in cols.iter().skip(1) {
        strip.line_to(Point::new(*x, *y + step));
    }
    for (x, y) in cols.iter().rev() {
        strip.line_to(Point::new(*x, *y - thick));
    }
    strip.close_path();
    let sb = strip.bounding_box();
    let mut out = Vec::new();
    for i in 0..shapes.len() {
        if !mass(i) || !overlaps(boxes[i], sb) {
            continue;
        }
        let current = remainder[i].clone().unwrap_or_else(|| shapes[i].clone());
        if current.path.elements().is_empty() {
            continue;
        }
        let Some(band) = clip_shape(&current, &strip, BoolOp::Intersect) else { continue };
        if region_area(&painted_region(&band)) < 0.01 * region_area(&(strip.clone(), FillMode::NonZero)) {
            continue;
        }
        let rest = clip_shape(&current, &strip, BoolOp::Difference);
        out.push((i, band, rest));
    }
    out
}

/// **Cut** the head symbol `head`, drawn in one piece, into part layers that
/// [`super::rig_head`] then rigs (see the module doc). Errors leave `scene`
/// as it was.
pub(super) fn cut_merged_head(scene: &mut Scene, head: SymbolId, style: &HeadStyle) -> Result<HeadCut, String> {
    let name = scene.library().get(head).map(|s| s.name.clone()).unwrap_or_default();
    let drawing = flatten_head(scene, head)?;
    let shapes = &drawing.shapes;
    let regions: Vec<(BezPath, FillMode)> = shapes.iter().map(painted_region).collect();
    let areas: Vec<f64> = regions.iter().map(region_area).collect();
    let boxes: Vec<Rect> = regions.iter().map(|r| r.0.bounding_box()).collect();
    let hb = boxes.iter().copied().reduce(|a, b| a.union(b)).unwrap_or_default();
    let mut out = HeadCut::default();

    // The eyes: drawn (with or without a pupil) or see-through eye holes,
    // paired about the face's midline.
    let mut eyes: Vec<EyeFound> = Vec::new();
    for mut e in pick_eyes(find_eyes(shapes, &regions, &areas, hb), hb.center().x) {
        match e.socket.take() {
            Some(h) => {
                if let Some(outline) = hole_outline(&h, &regions) {
                    e.bounds = outline.bounding_box();
                    e.area = region_area(&(outline.clone(), FillMode::NonZero));
                    e.hole = Some(outline);
                    eyes.push(e);
                }
            }
            None => eyes.push(e),
        }
    }
    if eyes.is_empty() {
        return Err(format!(
            "Rig Head: '{name}' has no separate eye parts and no eyes found in its drawing (no pair of eye whites, no eye white with a pupil, no eye hole)"
        ));
    }
    eyes.sort_by(|a, b| a.bounds.center().x.total_cmp(&b.bounds.center().x));
    let side = eyes.len() == 1 || {
        let (lo, hi) = eyes.iter().fold((f64::MAX, 0.0f64), |(lo, hi), e| (lo.min(e.area), hi.max(e.area)));
        lo < 0.55 * hi
    };

    let skin = dominant(shapes, &areas, is_skin_like).unwrap_or(Color::from_rgba8(214, 140, 85, 255));
    let ink_colour = dominant(shapes, &areas, |c| luma(c) < 0.25).unwrap_or(Color::from_rgba8(20, 14, 10, 255));
    let darkest = shapes
        .iter()
        .zip(&areas)
        .filter(|(s, a)| s.fill.is_some() && **a >= 0.0005 * hb.area())
        .filter_map(|(s, _)| paint_colour(s))
        .min_by(|a, b| luma(*a).total_cmp(&luma(*b)))
        .unwrap_or(ink_colour);

    let mut part: Vec<Option<Cut>> = vec![None; shapes.len()];
    let mut generated: Vec<ShapeData> = Vec::new();

    // Each drawn eye's own shapes.
    for e in eyes.iter().filter(|e| e.hole.is_none()) {
        let zone = e.bounds.inflate(e.bounds.width() * 0.25, e.bounds.height() * 0.45);
        let first = *e.white.iter().min().unwrap_or(&0);
        let centre = e.bounds.center();
        for i in 0..shapes.len() {
            if part[i].is_some() {
                continue;
            }
            if e.white.contains(&i) {
                part[i] = Some(Cut::Eyes);
                continue;
            }
            let b = boxes[i];
            if box_inside(b, zone) < 0.9 || !overlaps(b, e.bounds) || areas[i] > 2.5 * e.area {
                continue;
            }
            let joins = if i < first {
                paint_colour(&shapes[i]).is_some_and(is_dark) && b.contains(centre)
            } else {
                true
            };
            if joins {
                part[i] = Some(Cut::Eyes);
            }
        }
    }
    // The drawn eye's measures, for an eye made beside it.
    let drawn = eyes.iter().find(|e| e.hole.is_none());
    let drawn_white = drawn.and_then(|e| e.white.first().and_then(|i| paint_colour(&shapes[*i])));
    let drawn_iris = drawn.and_then(|e| {
        let first = *e.white.iter().min()?;
        let white = &regions[e.white[0]];
        (first + 1..shapes.len())
            .filter(|j| part[*j] == Some(Cut::Eyes) && !e.white.contains(j))
            .filter(|j| shapes[*j].fill.is_some() && paint_colour(&shapes[*j]).is_some_and(|c| !is_light(c)))
            .filter(|j| share_inside(&regions[*j], white) >= 0.5)
            .max_by(|a, b| areas[*a].total_cmp(&areas[*b]))
            .map(|j| (areas[j] / e.area.max(1e-9), paint_colour(&shapes[j]).unwrap_or(darkest)))
    });
    for e in eyes.iter().filter(|e| e.hole.is_some()) {
        let Some(outline) = &e.hole else { continue };
        out.eye_holes += 1;
        let white_colour = drawn_white.unwrap_or(Color::WHITE);
        generated.push(ShapeData::filled(outline.clone(), white_colour));
        let (ratio, iris_colour) = drawn_iris.unwrap_or((style.eyes.eyeball_area, darkest));
        let r = (ratio.clamp(0.05, 0.6) * e.area / std::f64::consts::PI).sqrt().min(0.48 * e.bounds.height());
        let c = e.bounds.center();
        generated.push(ShapeData::filled(kurbo::Circle::new(c, r).to_path(0.02), iris_colour));
        if !edge_has_ink(outline, c, shapes, &regions) {
            let width = (e.bounds.height() * 0.07).clamp(0.8, 4.0);
            generated.push(ShapeData::stroked(outline.clone(), ink_colour, width));
        }
    }

    // The face's frame: between the eyes, and how far apart they are.
    let ec = eyes.iter().fold(Point::ZERO, |p, e| p + e.bounds.center().to_vec2() / eyes.len() as f64);
    let ew = eyes.iter().map(|e| e.bounds.width()).sum::<f64>() / eyes.len() as f64;
    let d = if eyes.len() >= 2 {
        eyes[0].bounds.center().distance(eyes[eyes.len() - 1].bounds.center())
    } else {
        2.2 * ew
    };
    let free = |part: &[Option<Cut>], i: usize| part[i].is_none();

    // Brows: a dark elongated fill above each eye, with what lies on it;
    // else (a brow drawn into the hair mass) a band cut off the dark mass's
    // lower edge right above the eye.
    let mut brow_colours: Vec<Color> = Vec::new();
    let mut brows: Vec<(f64, Vec<usize>, Vec<usize>)> = Vec::new();
    // Band pieces: (shape index, the band), and what is left of a shape a
    // band was cut off.
    let mut bands: Vec<(usize, ShapeData)> = Vec::new();
    let mut remainder: Vec<Option<ShapeData>> = vec![None; shapes.len()];
    for e in &eyes {
        let (cx, top, w) = (e.bounds.center().x, e.bounds.y0, e.bounds.width());
        let reach = e.bounds.height().max(0.35 * w);
        let seed = (0..shapes.len())
            .filter(|i| free(&part, *i) && (shapes[*i].fill.is_some() || shapes[*i].stroke.is_some()))
            .filter(|i| paint_colour(&shapes[*i]).is_some_and(|c| luma(c) < 0.5 && !is_reddish(c) && !is_skin_like(c)))
            .filter(|i| {
                let b = boxes[*i];
                let c = b.center();
                // Elongated: wide, or a thin arch (little of its box filled).
                (0.5 * w..=2.2 * w).contains(&b.width())
                    && (b.width() >= 1.4 * b.height() || (b.width() >= 0.8 * b.height() && areas[*i] <= 0.4 * b.area()))
                    && c.y <= top + 0.2 * e.bounds.height()
                    && c.y >= top - 1.5 * reach
                    && (c.x - cx).abs() <= 0.7 * w
                    && areas[*i] >= 0.12 * e.area
                    && areas[*i] <= 3.0 * e.area.max(1e-9)
                    && areas[*i] <= 0.03 * hb.area()
            })
            .max_by(|a, b| {
                let score = |i: usize| areas[i] / (1.0 + (boxes[i].center().y - (top - 0.8 * reach)).abs() / reach);
                score(*a).total_cmp(&score(*b))
            });
        let Some(seed) = seed else {
            let cut = brow_band(e, shapes, &regions, &areas, &boxes, &part, &remainder);
            if cut.is_empty() {
                continue;
            }
            let mut mine = Vec::new();
            let mut xs = Rect::ZERO;
            for (i, band, rest) in cut {
                if let Some(c) = paint_colour(&band) {
                    brow_colours.push(c);
                }
                let bb = band.path.bounding_box();
                xs = if mine.is_empty() { bb } else { xs.union(bb) };
                remainder[i] = Some(rest.unwrap_or_else(|| {
                    let mut empty = shapes[i].clone();
                    empty.path = BezPath::new();
                    empty
                }));
                mine.push(bands.len());
                bands.push((i, band));
            }
            brows.push((xs.center().x, Vec::new(), mine));
            continue;
        };
        let sb = boxes[seed];
        let grown = sb.inflate(sb.width() * 0.1, sb.height() * 0.25);
        let mut members = vec![seed];
        for i in 0..shapes.len() {
            if i == seed || !free(&part, i) || box_inside(boxes[i], grown) < 0.8 {
                continue;
            }
            let dark = paint_colour(&shapes[i]).is_some_and(|c| luma(c) < 0.5);
            if i > seed || dark {
                members.push(i);
            }
        }
        for &i in &members {
            part[i] = Some(Cut::BrowL); // sided below
            if let Some(c) = paint_colour(&shapes[i]) {
                brow_colours.push(c);
            }
        }
        brows.push((sb.center().x, members, Vec::new()));
    }
    // Screen sides, as H1 names them: the right brow is the one further right
    // (a lone brow: right of the head's middle).
    let head_mid = hb.center().x;
    let max_x = brows.iter().map(|(x, _, _)| *x).fold(f64::MIN, f64::max);
    let mut band_parts: Vec<Cut> = vec![Cut::BrowL; bands.len()];
    for (x, members, cut) in &brows {
        let right = if brows.len() >= 2 { *x >= max_x } else { *x >= head_mid };
        let side = if right { Cut::BrowR } else { Cut::BrowL };
        for &i in members {
            part[i] = Some(side);
        }
        for &k in cut {
            band_parts[k] = side;
        }
    }

    // The lower face.
    let below = |b: Rect, from: f64, to: f64, half: f64| {
        b.y0 >= ec.y + from * d && b.y1 <= ec.y + to * d && b.x0 >= ec.x - half * d && b.x1 <= ec.x + half * d
    };
    let mouthish = |c: Color| is_dark(c) || is_reddish(c);
    let mouth_cands: Vec<usize> = (0..shapes.len())
        .filter(|i| free(&part, *i))
        .filter(|i| paint_colour(&shapes[*i]).is_some_and(mouthish))
        .filter(|i| below(boxes[*i], 0.45, 2.1, 1.0) && boxes[*i].width() <= 1.3 * d && areas[*i] <= 0.03 * hb.area())
        .collect();
    // Moustache: wide, filled, in the brows' colour, above some of the mouth.
    let moustache_seed = (0..shapes.len())
        .filter(|i| free(&part, *i) && shapes[*i].fill.is_some())
        .filter(|i| {
            let b = boxes[*i];
            let c = paint_colour(&shapes[*i]);
            below(b, 0.3, 1.7, 1.0)
                && b.width() >= 0.6 * d
                && b.width() >= 2.2 * b.height()
                && b.area() > 0.0
                && areas[*i] / b.area() >= 0.3
                && c.is_some_and(|c| {
                    !is_reddish(c) && (luma(c) < 0.15 || brow_colours.iter().any(|k| colour_distance(*k, c) < 0.12))
                })
                && mouth_cands.iter().any(|j| *j != *i && boxes[*j].center().y > b.center().y && boxes[*j].width() < b.width())
        })
        .max_by(|a, b| areas[*a].total_cmp(&areas[*b]));
    if let Some(seed) = moustache_seed {
        // The shapes drawn with it, one after another (a moustache drawn as
        // a group), that lie on it; the first that does not (the mouth, the
        // brows) ends the run. Then anything else drawn over it.
        let sb = boxes[seed];
        let near = sb.inflate(sb.width() * 0.15, sb.height());
        let fits = |part: &[Option<Cut>], i: usize, zone: Rect| {
            free(part, i) && box_inside(boxes[i], zone) >= 0.8 && paint_colour(&shapes[i]).is_none_or(|c| !is_reddish(c))
        };
        let mut members = vec![seed];
        for i in (0..seed).rev() {
            if !fits(&part, i, near) {
                break;
            }
            members.push(i);
        }
        for i in seed + 1..shapes.len() {
            if !fits(&part, i, near) {
                break;
            }
            members.push(i);
        }
        let cluster = members.iter().map(|i| boxes[*i]).reduce(|a, b| a.union(b)).unwrap_or(sb);
        let grown = cluster.inflate(cluster.width() * 0.1, cluster.height() * 0.1);
        let first = *members.iter().min().unwrap_or(&seed);
        for i in first + 1..shapes.len() {
            if !members.contains(&i) && fits(&part, i, grown) {
                members.push(i);
            }
        }
        for i in members {
            part[i] = Some(Cut::Moustache);
        }
    }
    // Mouth: the biggest dark/red fill there, those by it, and what lies on
    // them (teeth, tongue).
    let mouth_cands: Vec<usize> = mouth_cands.into_iter().filter(|i| free(&part, *i)).collect();
    if let Some(&main) = mouth_cands.iter().max_by(|a, b| areas[**a].total_cmp(&areas[**b])) {
        let mb = boxes[main];
        let near = mb.inflate(mb.width() * 0.3, mb.height() * 0.6);
        let mut members: Vec<usize> = mouth_cands.iter().copied().filter(|i| overlaps(boxes[*i], near)).collect();
        let cluster = members.iter().map(|i| boxes[*i]).reduce(|a, b| a.union(b)).unwrap_or(mb);
        let first = *members.iter().min().unwrap_or(&main);
        let grown = cluster.inflate(cluster.width() * 0.1, cluster.height() * 0.1);
        for i in first + 1..shapes.len() {
            if free(&part, i) && !members.contains(&i) && box_inside(boxes[i], grown) >= 0.8 && areas[i] <= 0.03 * hb.area() {
                members.push(i);
            }
        }
        for i in members {
            part[i] = Some(Cut::Mouth);
        }
    }

    // The skin drawn first: what the nose and hair must lie over to be
    // separate regions.
    let skin_shapes: Vec<usize> =
        (0..shapes.len()).filter(|i| shapes[*i].fill.is_some() && paint_colour(&shapes[*i]).is_some_and(|c| colour_distance(c, skin) < 0.08)).collect();
    let over_skin = |members: &[usize]| -> f64 {
        let Some(first) = members.iter().min().copied() else { return 0.0 };
        let b = members.iter().map(|i| boxes[*i]).reduce(|a, b| a.union(b)).unwrap_or_default();
        if b.area() <= 0.0 {
            return 0.0;
        }
        let g = Grid::over(b, (b.width().max(b.height()) / 80.0).max(0.02));
        let own: Vec<&(BezPath, FillMode)> = members.iter().map(|i| &regions[*i]).collect();
        let under: Vec<&(BezPath, FillMode)> =
            skin_shapes.iter().filter(|i| **i < first && overlaps(boxes[**i], b)).map(|i| &regions[*i]).collect();
        let a = coverage(&own, &g);
        let u = coverage(&under, &g);
        let n = a.iter().filter(|x| **x).count();
        if n == 0 {
            return 0.0;
        }
        a.iter().zip(&u).filter(|(x, y)| **x && **y).count() as f64 / n as f64
    };
    let first_skin = skin_shapes.first().copied().unwrap_or(0);

    // Nose: between the eyes and the mouth, over the skin.
    let mouth_top = (0..shapes.len())
        .filter(|i| matches!(part[*i], Some(Cut::Mouth | Cut::Moustache)))
        .map(|i| boxes[i].y0)
        .fold(f64::MAX, f64::min);
    // (A moustache or mouth may be drawn over the nose's tip.)
    let nose_bottom = if mouth_top < f64::MAX { mouth_top + 0.3 * d } else { ec.y + 1.0 * d };
    let (nose_x0, nose_x1) = if side { (ec.x - 1.5 * ew, ec.x + 1.5 * ew) } else { (ec.x - 0.5 * d, ec.x + 0.5 * d) };
    let nose_top = if side { ec.y } else { ec.y - 0.25 * d };
    let nose: Vec<usize> = (0..shapes.len())
        .filter(|i| free(&part, *i) && *i > first_skin)
        .filter(|i| {
            let b = boxes[*i];
            b.x0 >= nose_x0 && b.x1 <= nose_x1 && b.y0 >= nose_top && b.y1 <= nose_bottom && areas[*i] <= 0.02 * hb.area()
        })
        .collect();
    let nose_fill = nose.iter().any(|i| shapes[*i].fill.is_some() && paint_colour(&shapes[*i]).is_some_and(|c| !is_dark(c)));
    let nose_share = over_skin(&nose);
    if !nose.is_empty() && nose_fill && nose_share >= 0.6 {
        for &i in &nose {
            part[i] = Some(Cut::Nose);
        }
    }

    // Hair: drawn over the skin of the head, above the eyes.
    let hair: Vec<usize> = (0..shapes.len())
        .filter(|i| free(&part, *i) && *i > first_skin && shapes[*i].fill.is_some())
        .filter(|i| boxes[*i].center().y < ec.y - 0.2 * d)
        .filter(|i| paint_colour(&shapes[*i]).is_some_and(|c| colour_distance(c, skin) >= 0.08))
        .collect();
    let hair_area: f64 = hair.iter().map(|i| areas[*i]).sum();
    let hair_share = if hair_area >= 0.05 * hb.area() { over_skin(&hair) } else { 0.0 };
    if hair_area >= 0.05 * hb.area() && hair_share >= 0.15 {
        for &i in &hair {
            part[i] = Some(Cut::Hair);
        }
    }

    // Everything else is the base.
    let mut pieces: Vec<Piece> = shapes
        .iter()
        .enumerate()
        .map(|(i, s)| Piece {
            shape: remainder[i].clone().unwrap_or_else(|| s.clone()),
            order: i as f64,
            part: part[i].unwrap_or(Cut::Base),
            seed: part[i].is_some(),
        })
        .collect();
    for ((i, band), p) in bands.into_iter().zip(band_parts) {
        pieces.push(Piece { shape: band, order: i as f64, part: p, seed: true });
    }
    for (k, s) in generated.into_iter().enumerate() {
        pieces.push(Piece { shape: s, order: 1e9 + k as f64, part: Cut::Eyes, seed: true });
    }

    // The layer order H1 builds (the style's), top first.
    let layers_style = if side { &style.side } else { &style.front };
    let mut order: Vec<Cut> = Vec::new();
    for l in layers_style {
        let c = match l.part {
            HeadPart::EyeLid | HeadPart::EyeMask | HeadPart::EyeBall | HeadPart::EyeWhite => Cut::Eyes,
            HeadPart::RightBrow => Cut::BrowR,
            HeadPart::LeftBrow => Cut::BrowL,
            HeadPart::Moustache => Cut::Moustache,
            HeadPart::Mouth => Cut::Mouth,
            HeadPart::Nose => Cut::Nose,
            HeadPart::Hair => Cut::Hair,
            HeadPart::Base => Cut::Base,
        };
        if !order.contains(&c) {
            order.push(c);
        }
    }
    for c in [Cut::Eyes, Cut::BrowR, Cut::BrowL, Cut::Moustache, Cut::Mouth, Cut::Nose, Cut::Hair, Cut::Base] {
        if !order.contains(&c) {
            order.insert(order.len().saturating_sub(1), c);
        }
    }
    let rank = |c: Cut| order.iter().position(|x| *x == c).unwrap_or(order.len());

    // A lower layer's shape drawn over a part: the piece over the part goes
    // with it.
    let mut conflicts = 0usize;
    for &p in order.iter().filter(|c| **c != Cut::Base) {
        let seeds: Vec<&Piece> = pieces.iter().filter(|q| q.part == p && q.seed).collect();
        if seeds.is_empty() {
            continue;
        }
        let t = seeds.iter().map(|q| q.order).fold(f64::MAX, f64::min);
        let paths: Vec<BezPath> = seeds.iter().map(|q| plain(&painted_region(&q.shape))).collect();
        let foot = union_of(&paths);
        let fb = foot.bounding_box();
        let foot_area = seeds.iter().map(|q| region_area(&painted_region(&q.shape))).sum::<f64>().max(1e-9);
        let mut add: Vec<Piece> = Vec::new();
        for q in pieces.iter_mut() {
            if q.order <= t || rank(q.part) <= rank(p) || q.shape.path.elements().is_empty() {
                continue;
            }
            if !overlaps(q.shape.path.bounding_box(), fb) {
                continue;
            }
            if q.seed {
                if share_inside(&painted_region(&q.shape), &(foot.clone(), FillMode::NonZero)) > 0.02 {
                    conflicts += 1;
                }
                continue;
            }
            let Some(inside) = clip_shape(&q.shape, &foot, BoolOp::Intersect) else { continue };
            if region_area(&painted_region(&inside)) < 1e-3 * foot_area {
                continue;
            }
            let outside = clip_shape(&q.shape, &foot, BoolOp::Difference);
            add.push(Piece { shape: inside, order: q.order, part: p, seed: false });
            match outside {
                Some(o) => q.shape = o,
                None => q.shape.path = BezPath::new(),
            }
        }
        pieces.extend(add);
    }
    pieces.retain(|q| !q.shape.path.elements().is_empty());
    if conflicts > 0 {
        out.notes.push(format!("{conflicts} part shape(s) drawn over a higher part: left in their part"));
    }

    // Skin under each cut part, at the bottom of the base.
    let grow = (hb.width().max(hb.height()) / 600.0).max(0.4);
    let base_pieces: Vec<(BezPath, Rect, Option<Color>, bool)> = pieces
        .iter()
        .filter(|q| q.part == Cut::Base)
        .map(|q| {
            let r = painted_region(&q.shape);
            let b = r.0.bounding_box();
            (plain(&r), b, paint_colour(&q.shape), q.shape.fill.is_some())
        })
        .collect();
    let mut patches: Vec<ShapeData> = Vec::new();
    for &p in &[Cut::Eyes, Cut::BrowR, Cut::BrowL, Cut::Moustache, Cut::Mouth, Cut::Nose] {
        let own: Vec<BezPath> = pieces
            .iter()
            .filter(|q| q.part == p && (q.order < 1e9))
            .map(|q| plain(&painted_region(&q.shape)))
            .collect();
        if own.is_empty() {
            continue;
        }
        let foot = union_of(&own);
        let fb = foot.bounding_box();
        if fb.area() <= 0.0 {
            continue;
        }
        let near = fb.inflate(grow * 3.0, grow * 3.0);
        let local: Vec<BezPath> = base_pieces.iter().filter(|(_, b, _, _)| overlaps(*b, near)).map(|(p, _, _, _)| p.clone()).collect();
        let ring = stroke_outline(&foot, 2.0 * grow);
        let mut patch = foot.clone();
        if !local.is_empty() {
            let under = union_of(&local);
            let extra = buzz_geom::boolean(&ring, &under, BoolOp::Intersect, opts_for(near));
            if !extra.elements().is_empty() {
                patch = union_of(&[patch, extra]);
            }
        }
        // The skin colour around the part, on the base.
        let mut tally: Vec<(Color, usize)> = Vec::new();
        let (cx, cy) = (fb.center().x, fb.center().y);
        for ring in [1.2, 1.45] {
            let (rx, ry) = (fb.width() / 2.0 * ring + 2.0 * grow, fb.height() / 2.0 * ring + 2.0 * grow);
            for k in 0..40 {
                let a = k as f64 / 40.0 * std::f64::consts::TAU;
                let q = Point::new(cx + rx * a.cos(), cy + ry * a.sin());
                let hit = base_pieces.iter().rev().find(|(path, b, _, filled)| *filled && b.contains(q) && contains(path, FillMode::NonZero, q));
                let Some(c) = hit.and_then(|h| h.2) else { continue };
                if luma(c) < 0.2 || is_light(c) {
                    continue;
                }
                match tally.iter_mut().find(|(k, _)| colour_distance(*k, c) < 0.03) {
                    Some((_, n)) => *n += 1,
                    None => tally.push((c, 1)),
                }
            }
        }
        let colour = tally
            .iter()
            .filter(|(c, _)| is_skin_like(*c))
            .max_by_key(|(_, n)| *n)
            .or_else(|| tally.iter().max_by_key(|(_, n)| *n))
            .map(|(c, _)| *c)
            .unwrap_or(skin);
        patches.push(ShapeData::filled(patch, colour));
    }
    for (k, s) in patches.into_iter().enumerate() {
        pieces.push(Piece { shape: s, order: -1e9 + k as f64, part: Cut::Base, seed: false });
    }

    // What was found, for the report.
    let count = |c: Cut| pieces.iter().filter(|q| q.part == c).count();
    let mut found = vec![format!("{} eye(s){}", eyes.len(), if out.eye_holes > 0 { format!(" ({} made from eye holes)", out.eye_holes) } else { String::new() })];
    let brows_n = [Cut::BrowR, Cut::BrowL].iter().filter(|c| count(**c) > 0).count();
    found.push(format!("{brows_n} brow(s)"));
    found.push(if count(Cut::Moustache) > 0 { "a moustache".into() } else { "no moustache".into() });
    found.push(if count(Cut::Mouth) > 0 { "the mouth".into() } else { "no mouth found (it stays in the base)".into() });
    found.push(if count(Cut::Nose) > 0 {
        "the nose".into()
    } else {
        format!("the nose stays in the base (over skin {:.0}%)", nose_share * 100.0)
    });
    found.push(if count(Cut::Hair) > 0 {
        format!("the hair (over skin {:.0}%)", hair_share * 100.0)
    } else {
        format!("the hair stays in the base (over skin {:.0}%)", hair_share * 100.0)
    });
    out.notes.insert(0, format!("'{name}' is one drawing: cut into {}", found.join(", ")));

    // The new layers, top first.
    let mut stack = buzz_scene::LayerStack::new();
    let mut rows: Vec<Layer> = drawing.kept.clone();
    for &c in &order {
        let mut mine: Vec<&Piece> = pieces.iter().filter(|q| q.part == c).collect();
        if mine.is_empty() {
            continue;
        }
        mine.sort_by(|a, b| a.order.total_cmp(&b.order));
        let objects: Vec<Arc<Object>> = mine.iter().map(|q| Arc::new(Object::shape(scene.next_object_id(), q.shape.clone()))).collect();
        let mut layer = Layer::new(LayerId(scene.next_object_id().0), c.layer_name(), LayerKind::Normal);
        let mut k = Keyframe::new(0);
        k.objects = Arc::new(objects);
        layer.frames = LayerTimeline::from_parts(vec![k], drawing.length);
        rows.push(layer);
    }
    for (i, layer) in rows.into_iter().enumerate() {
        stack.insert(i, layer);
    }
    scene.library_mut().update(head, |s| s.layers = stack);
    Ok(out)
}
