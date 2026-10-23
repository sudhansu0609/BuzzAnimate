//! **S1/S2: a merged drawing cut the way a [`CutStyle`] says** (see
//! `docs/STYLE_CUT_AND_OUTPAINT_PLAN.md`).
//!
//! R4's generic split hands each point to the bone capsule it lies nearest
//! and patches the joints with overlap copies, caps and underlays. This one
//! cuts like the user: the drawing's own lines (a hem, a cuff, a jaw line)
//! are the cut lines, each fill panel between them goes whole to one part by
//! the style's garment rules (the sleeve with the arm, the collar and neck
//! with the body and the head ending at the chin, the shoe with the foot,
//! midCloth = the centre fold between the thighs, extras with the body), and
//! a panel that really runs across a joint (one colour, no line) is cut
//! straight, square to the bone, at the style's cut position. A line goes
//! with the piece stacked over its neighbour (it is that piece's outline).
//!
//! Then each piece is **completed** (S2) the way the user draws it: a tucked
//! limb is continued under its parent along its two silhouette edges and
//! closed with the style's end shape, filled from its nearest visible
//! colours and outlined in the drawing's line colour and width; the torso is
//! continued (its hull) under the arms and head. Hidden art is kept to what
//! the pieces stacked over it cover at rest, so the rest picture does not
//! change. Pivots go where the style puts them.

use super::*;
use buzz_rig::cut_style::{CutStyle, PivotPlace, Stacking};
use kurbo::Shape as _;

const NONE: u32 = u32::MAX;

/// The part a slot's joint turns from in the cut: the head, arms and thighs
/// from the body; midCloth (the hips slot) from nothing.
fn cut_parent(slot: usize) -> Option<usize> {
    match slot {
        2 | 3 | 5 | 7 | 9 => Some(1),
        4 => Some(3),
        6 => Some(5),
        11 => Some(4),
        12 => Some(6),
        8 => Some(7),
        10 => Some(9),
        13 => Some(8),
        14 => Some(10),
        _ => None,
    }
}

/// The cut style's part key for a `biped_full` slot.
fn part_key(slot: usize) -> &'static str {
    match slot {
        0 => "Hips",
        1 => "Chest",
        2 => "Head",
        3 | 5 => "Shoulder",
        4 | 6 => "Elbow",
        7 | 9 => "Thigh",
        8 | 10 => "Knee",
        11 | 12 => "Wrist",
        13 | 14 => "Ankle",
        _ => "",
    }
}

/// Every grid cell whose centre `path` (filled by `rule`) covers.
fn raster(g: &BoneGrid, path: &buzz_geom::BezPath, rule: buzz_geom::FillMode, mut f: impl FnMut(usize)) {
    let (cols, rows, cell, origin) = (g.cols, g.rows, g.cell, g.origin);
    let edges = path_edges(path, cell * 0.25);
    let bbox = path.bounding_box();
    let r0 = ((bbox.y0 - origin.y) / cell - 0.5).ceil().max(0.0);
    let r1 = ((bbox.y1 - origin.y) / cell - 0.5).floor().min(rows as f64 - 1.0);
    if r1 < r0 {
        return;
    }
    let mut xs: Vec<(f64, i32)> = Vec::new();
    for r in r0 as usize..=r1 as usize {
        let y = origin.y + (r as f64 + 0.5) * cell;
        xs.clear();
        for (a, b) in &edges {
            if (a.y <= y) != (b.y <= y) {
                let t = (y - a.y) / (b.y - a.y);
                xs.push((a.x + t * (b.x - a.x), if b.y > a.y { 1 } else { -1 }));
            }
        }
        xs.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut wind = 0;
        for k in 0..xs.len().saturating_sub(1) {
            wind += xs[k].1;
            let inside = match rule {
                buzz_geom::FillMode::EvenOdd => k % 2 == 0,
                buzz_geom::FillMode::NonZero => wind != 0,
            };
            if !inside {
                continue;
            }
            let c0 = ((xs[k].0 - origin.x) / cell - 0.5).ceil().max(0.0);
            let c1 = ((xs[k + 1].0 - origin.x) / cell - 0.5).floor().min(cols as f64 - 1.0);
            if c1 < c0 {
                continue;
            }
            for c in c0 as usize..=c1 as usize {
                f(r * cols + c);
            }
        }
    }
}

/// The cells of `mask` as one path (rectangles merged).
fn mask_path(g: &BoneGrid, mask: &[bool], opts: buzz_geom::BooleanOptions) -> buzz_geom::BezPath {
    let mut runs = buzz_geom::BezPath::new();
    let (mut c0, mut c1, mut r0, mut r1) = (usize::MAX, 0, usize::MAX, 0);
    for r in 0..g.rows {
        let y0 = g.origin.y + r as f64 * g.cell;
        let mut start: Option<usize> = None;
        for c in 0..=g.cols {
            let on = c < g.cols && mask[r * g.cols + c];
            match (on, start) {
                (true, None) => start = Some(c),
                (false, Some(s)) => {
                    let rect = buzz_geom::Rect::new(
                        g.origin.x + s as f64 * g.cell,
                        y0,
                        g.origin.x + c as f64 * g.cell,
                        y0 + g.cell,
                    );
                    runs.extend(rect.to_path(0.1));
                    c0 = c0.min(s);
                    c1 = c1.max(c);
                    r0 = r0.min(r);
                    r1 = r1.max(r + 1);
                    start = None;
                }
                _ => {}
            }
        }
    }
    if runs.elements().is_empty() {
        return runs;
    }
    let whole = buzz_geom::Rect::new(
        g.origin.x + c0 as f64 * g.cell,
        g.origin.y + r0 as f64 * g.cell,
        g.origin.x + c1 as f64 * g.cell,
        g.origin.y + r1 as f64 * g.cell,
    )
    .inflate(g.cell, g.cell)
    .to_path(0.1);
    buzz_geom::boolean(&runs, &whole, buzz_geom::BoolOp::Intersect, buzz_geom::BooleanOptions { fill: buzz_geom::FillMode::NonZero, ..opts })
}

/// The convex hull of `points` (Andrew's monotone chain), counter-clockwise.
fn convex_hull(mut points: Vec<Point>) -> Vec<Point> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: Point, a: Point, b: Point| (a - o).cross(b - o);
    let mut lower: Vec<Point> = Vec::new();
    for &p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn polygon(points: &[Point]) -> buzz_geom::BezPath {
    let mut path = buzz_geom::BezPath::new();
    for (k, p) in points.iter().enumerate() {
        if k == 0 {
            path.move_to(*p);
        } else {
            path.line_to(*p);
        }
    }
    if !points.is_empty() {
        path.close_path();
    }
    path
}

/// The drawing on the grid, cut by the style (S1).
struct Styled {
    partition: BonePartition,
    /// The top paint per cell.
    cover: Vec<u32>,
    /// Per paint: a line (ink, fold, mark).
    line: Vec<bool>,
    /// Per bone: its place in the stack, back to front.
    rank: Vec<usize>,
    /// Per bone: the bone its joint turns from in the cut.
    parent: Vec<Option<usize>>,
    /// Per bone: a limb laid over its parent, its drawn shape (S1's dome).
    limb_shape: Vec<Option<buzz_geom::BezPath>>,
    /// The extra bone a scarf over the body was given (an extra of the body).
    scarf: Option<usize>,
}

/// The slots back to front as the style stacks them (a front view): the
/// legs (screen-left leg under the right; thigh over shin over foot as the
/// style tucks them), midCloth over the legs, the body, the arms over it
/// (screen-left arm under the right; within an arm as the style tucks it),
/// the head on top.
fn stack_order(base: &BonePartition, style: &CutStyle) -> Vec<usize> {
    let x = |slot: usize| base.bones.iter().find(|b| b.0 == slot).map_or(0.0, |b| b.1.x);
    let under = |slot: usize| style.slot(part_key(slot)).is_none_or(|s| s.stacking != Stacking::OverParent);
    let chain = |root: usize, mid: usize, end: usize| -> Vec<usize> {
        let mut v = vec![root];
        for (child, parent) in [(mid, root), (end, mid)] {
            let at = v.iter().position(|s| *s == parent).unwrap_or(0);
            if under(child) {
                v.insert(at, child);
            } else {
                v.insert(at + 1, child);
            }
        }
        v
    };
    let pair = |a: Vec<usize>, b: Vec<usize>, ax: f64, bx: f64| if ax <= bx { [a, b].concat() } else { [b, a].concat() };
    let legs = pair(chain(7, 8, 13), chain(9, 10, 14), x(7), x(9));
    let arms = pair(chain(3, 4, 11), chain(5, 6, 12), x(3), x(5));
    let mut order = legs;
    order.push(0);
    if style.arms_over_body_front {
        order.push(1);
        order.extend(arms);
    } else {
        order.extend(arms);
        order.push(1);
    }
    if style.slot("Head").is_none_or(|s| s.stacking == Stacking::OverParent) {
        order.push(2);
    } else {
        let at = order.iter().position(|s| *s == 1).unwrap_or(0);
        order.insert(at, 2);
    }
    order
}

/// **S1.** The plane split by the drawing and the style. See the module
/// docs; `paints` are every paint of the split layers in the figure's space,
/// back to front.
fn styled_partition(
    base: &BonePartition,
    paints: &[(buzz_geom::BezPath, buzz_geom::FillMode, buzz_scene::FillSpec)],
    style: &CutStyle,
) -> Styled {
    let g = base.grid();
    let (cols, rows, cell, origin) = (g.cols, g.rows, g.cell, g.origin);
    let n = cols * rows;
    let centre = |i: usize| {
        Point::new(origin.x + ((i % cols) as f64 + 0.5) * cell, origin.y + ((i / cols) as f64 + 0.5) * cell)
    };
    let bones = base.bones.len();
    let bone_of = |slot: usize| base.bones.iter().position(|b| b.0 == slot);
    let neighbours = |i: usize| {
        let (c, r) = (i % cols, i / cols);
        [
            (c > 0).then(|| i - 1),
            (c + 1 < cols).then(|| i + 1),
            (r > 0).then(|| i - cols),
            (r + 1 < rows).then(|| i + cols),
        ]
    };

    // Stacking.
    let order = stack_order(base, style);
    let mut rank = vec![0usize; bones];
    let mut next = 0;
    for slot in &order {
        if let Some(b) = bone_of(*slot) {
            rank[b] = next;
            next += 1;
        }
    }
    for b in 0..bones {
        if !order.contains(&base.bones[b].0) {
            rank[b] = next;
            next += 1;
        }
    }
    let parent: Vec<Option<usize>> = (0..bones).map(|b| cut_parent(base.bones[b].0).and_then(bone_of)).collect();

    // 1. The top paint per cell, and which paints are lines.
    let mut cover = vec![NONE; n];
    let mut line = vec![false; paints.len()];
    for (index, (path, rule, _)) in paints.iter().enumerate() {
        let edges = path_edges(path, cell * 0.25);
        let perimeter: f64 = edges.iter().map(|(a, b)| (*b - *a).hypot()).sum();
        line[index] = perimeter <= 1e-9 || 2.0 * path.area().abs() / perimeter < LINE_CELLS * cell;
        raster(g, path, *rule, |i| cover[i] = index as u32);
    }
    let fill = |i: usize| cover[i] != NONE && !line[cover[i] as usize];
    // Small marks: lines no longer than a twentieth of the figure.
    let mark: Vec<bool> = paints
        .iter()
        .zip(&line)
        .map(|((path, _, _), l)| {
            let b = path.bounding_box();
            *l && b.width().hypot(b.height()) < 0.05 * base.bounds.height()
        })
        .collect();

    // 2. Panels: fills joined cell to cell with no line between.
    let mut panel = vec![NONE; n];
    let mut panels: Vec<Vec<usize>> = Vec::new();
    let mut queue: Vec<usize> = Vec::new();
    for start in 0..n {
        if !fill(start) || panel[start] != NONE {
            continue;
        }
        let id = panels.len() as u32;
        let mut cells = Vec::new();
        panel[start] = id;
        queue.push(start);
        while let Some(i) = queue.pop() {
            cells.push(i);
            for j in neighbours(i).into_iter().flatten() {
                if fill(j) && panel[j] == NONE {
                    panel[j] = id;
                    queue.push(j);
                }
            }
        }
        panels.push(cells);
    }
    let centroid = |cells: &[usize]| {
        let mut sum = Vec2::ZERO;
        for &i in cells {
            sum += centre(i).to_vec2();
        }
        (sum / cells.len().max(1) as f64).to_point()
    };

    // 3. The capsules' labels, the hips' given to the body: the body runs
    // down to its hem; the hips slot (midCloth) is only the centre fold.
    let mut labels = g.labels.clone();
    let (hips, chest) = (bone_of(0), bone_of(1));
    if let (Some(h), Some(c)) = (hips, chest) {
        for l in labels.iter_mut() {
            if *l as usize == h {
                *l = c as u8;
            }
        }
    }

    // 4. midCloth: a panel between the thighs, below the hips and above the
    // knees, narrower than the gap between the thigh bones and taller than
    // wide.
    let mut fixed = vec![false; panels.len()];
    if let (Some(h), Some(tl), Some(tr)) = (hips, bone_of(7), bone_of(9)) {
        let (_, hl, kl, _) = base.bones[tl];
        let (_, hr, kr, _) = base.bones[tr];
        let (top, knee) = ((hl.y + hr.y) * 0.5, (kl.y + kr.y) * 0.5);
        let x_at = |a: Point, b: Point, y: f64| if (b.y - a.y).abs() < 1e-9 { a.x } else { a.x + (b.x - a.x) * (y - a.y) / (b.y - a.y) };
        for (id, cells) in panels.iter().enumerate() {
            let c = centroid(cells);
            if !(c.y > top && c.y < knee) {
                continue;
            }
            let (xl, xr) = (x_at(hl, kl, c.y), x_at(hr, kr, c.y));
            let gap = (xr - xl).abs();
            if gap <= cell {
                continue;
            }
            let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
            for &i in cells {
                let p = centre(i);
                x0 = x0.min(p.x);
                x1 = x1.max(p.x);
                y0 = y0.min(p.y);
                y1 = y1.max(p.y);
            }
            if (c.x - (xl + xr) * 0.5).abs() < 0.2 * gap && x1 - x0 < 0.7 * gap && y1 - y0 > x1 - x0 {
                for &i in cells {
                    labels[i] = h as u8;
                }
                fixed[id] = true;
            }
        }
    }

    // 5. At each joint, root first. A panel the limb's bone runs through
    // past the style's cut line is the limb crossing the joint (one colour,
    // no line at the joint): a limb tucked under its parent is cut straight
    // there, square to the bone, across the limb's width; a limb laid over
    // its parent (the upper arm over the body) takes the part of the panel
    // inside its own drawn shape (its edges carried up to the style's drawn
    // tip, closed with its end shape). Any other panel across the joint, or
    // small and next to it, goes whole: to the parent when its centre lies
    // before the cut line, else to the side holding most of it.
    let depth = |b: usize| {
        let (mut d, mut at) = (0, b);
        while let Some(p) = parent[at] {
            d += 1;
            at = p;
            if d > bones {
                break;
            }
        }
        d
    };
    let subtree = |c: usize| -> Vec<bool> {
        (0..bones)
            .map(|b| {
                let mut at = Some(b);
                for _ in 0..=bones {
                    match at {
                        Some(x) if x == c => return true,
                        Some(x) => at = parent[x],
                        None => return false,
                    }
                }
                false
            })
            .collect()
    };
    let cell_at = |p: Point| -> Option<usize> {
        let c = ((p.x - origin.x) / cell).floor();
        let r = ((p.y - origin.y) / cell).floor();
        (c >= 0.0 && r >= 0.0 && (c as usize) < cols && (r as usize) < rows).then(|| r as usize * cols + c as usize)
    };
    let mut limb_shape: Vec<Option<buzz_geom::BezPath>> = vec![None; bones];
    let mut joints: Vec<usize> = (0..bones).filter(|b| parent[*b].is_some()).collect();
    joints.sort_by_key(|b| (depth(*b), *b));
    for c in joints {
        let Some(p) = parent[c] else { continue };
        let slot_c = base.bones[c].0;
        let sub = subtree(c);
        let (_, head, tip, _) = base.bones[c];
        let len = (tip - head).hypot();
        if len <= 1e-9 {
            continue;
        }
        // A foot is cut across the shin, along the shin.
        let u = match slot_c {
            13 | 14 => {
                let (_, ph, pt, _) = base.bones[p];
                let d = pt - ph;
                if d.hypot() > 1e-9 { d / d.hypot() } else { (tip - head) / len }
            }
            _ => (tip - head) / len,
        };
        let nrm = Vec2::new(-u.y, u.x);
        let st = |q: Point| ((q - head).dot(u), (q - head).dot(nrm));
        // A limb folded back along its parent (a forearm up behind the upper
        // arm) is hidden by it: what the drawing shows there is the parent.
        let (_, ph, pt, _) = base.bones[p];
        let up = pt - ph;
        if matches!(slot_c, 4 | 6 | 8 | 10 | 11 | 12) && up.hypot() > 1e-9 && u.dot(up / up.hypot()) < -0.5 {
            for cells in panels.iter() {
                if cells.iter().any(|&i| labels[i] as usize == p) {
                    for &i in cells {
                        if sub[labels[i] as usize] {
                            labels[i] = p as u8;
                        }
                    }
                }
            }
            continue;
        }
        let cut = style.slot(part_key(slot_c));
        let s_star = cut.map_or(0.0, |s| s.cut_pos) * len;
        let over = rank[c] > rank[p];
        let limbish = slot_c != 2;
        for id in 0..panels.len() {
            if fixed[id] {
                continue;
            }
            let cells = &panels[id];
            // The panel or a small mark over it (texture scratches, short
            // folds): a run across the limb walks on through those, not
            // through an outline.
            let in_panel_or_line =
                |q: Point| cell_at(q).is_some_and(|i| panel[i] == id as u32 || (cover[i] != NONE && mark[cover[i] as usize]));
            // Does the bone run through the panel past the cut, most of its
            // length (a long shirt over the hips stops at its hem)? A limb
            // laid over its parent needs only its start: a short sleeve
            // drawn in one with the shirt is the arm's.
            let samples = 16;
            let span = if over { 0.35 } else { 0.8 };
            let on_axis = (0..=samples)
                .filter(|k| in_panel_or_line(head + u * (s_star + span * len * *k as f64 / samples as f64)))
                .count();
            if limbish && on_axis * 10 >= (samples + 1) * 6 {
                // The limb's run across the bone, bin by bin.
                let step = 2.0 * cell;
                let mut runs: Vec<(f64, f64, f64)> = Vec::new();
                let mut s = s_star;
                while s <= s_star + 0.7 * len {
                    let at = head + u * s;
                    if in_panel_or_line(at) {
                        let walk = |dir: f64| {
                            let mut t = 0.0;
                            while t < 2.0 * len && in_panel_or_line(at + nrm * (dir * (t + 0.5 * cell))) {
                                t += 0.5 * cell;
                            }
                            dir * t
                        };
                        runs.push((s, walk(-1.0), walk(1.0)));
                    }
                    s += step;
                }
                if runs.len() < 3 {
                    continue;
                }
                // The limb's own width: the narrower runs (a run that meets
                // the body at the armpit is not the sleeve).
                let mut widths: Vec<f64> = runs.iter().map(|r| r.2 - r.1).collect();
                widths.sort_by(f64::total_cmp);
                let w = widths[(widths.len() * 3 / 10).min(widths.len() - 1)].max(cell);
                let fit_runs: Vec<(f64, f64, f64)> = runs.iter().copied().filter(|r| r.2 - r.1 <= 1.3 * w).collect();
                let fit = |pick: &dyn Fn(&(f64, f64, f64)) -> f64, from: f64, to: f64| -> (f64, f64) {
                    let pts: Vec<(f64, f64)> =
                        fit_runs.iter().filter(|r| r.0 >= from && r.0 <= to).map(|r| (r.0, pick(r))).collect();
                    let pts = if pts.len() >= 3 { pts } else { fit_runs.iter().map(|r| (r.0, pick(r))).collect() };
                    let m = pts.len().max(1) as f64;
                    let (ms, mt) = (pts.iter().map(|q| q.0).sum::<f64>() / m, pts.iter().map(|q| q.1).sum::<f64>() / m);
                    let (mut sxx, mut sxy) = (0.0, 0.0);
                    for (a, b) in &pts {
                        sxx += (a - ms) * (a - ms);
                        sxy += (a - ms) * (b - mt);
                    }
                    let slope = if pts.len() >= 3 && sxx > 1e-9 { (sxy / sxx).clamp(-0.3, 0.3) } else { 0.0 };
                    (mt - slope * ms, slope)
                };
                if over {
                    // Laid over the parent: the limb's drawn shape.
                    let (from, to) = (s_star + 0.25 * len, s_star + 0.7 * len);
                    let (la, lb) = fit(&|r| r.1, from, to);
                    let (ra, rb) = fit(&|r| r.2, from, to);
                    let tip_s = -cut.map_or(0.0, |s| s.tip_beyond_joint) * len;
                    let bottom = to;
                    let (wl, wr) = (la + lb * tip_s, ra + rb * tip_s);
                    let wt = (wr - wl).max(cell);
                    let d = (cut.map_or(1.0, |s| s.end_roundness) * wt * 0.5).min(bottom - tip_s);
                    let base_s = tip_s + d;
                    let mut pts: Vec<Point> = vec![
                        head + u * bottom + nrm * (la + lb * bottom),
                        head + u * base_s + nrm * (la + lb * base_s),
                    ];
                    let (tc, half) =
                        ((la + lb * base_s + ra + rb * base_s) * 0.5, (ra + rb * base_s - la - lb * base_s) * 0.5);
                    for k in 1..24 {
                        let phi = std::f64::consts::PI * k as f64 / 24.0;
                        pts.push(head + u * (base_s - d * phi.sin()) + nrm * (tc - half * phi.cos()));
                    }
                    pts.push(head + u * base_s + nrm * (ra + rb * base_s));
                    pts.push(head + u * bottom + nrm * (ra + rb * bottom));
                    let shape = polygon(&pts);
                    let mut inside = vec![false; n];
                    raster(g, &shape, buzz_geom::FillMode::NonZero, |i| inside[i] = true);
                    if limb_shape[c].is_none() {
                        limb_shape[c] = Some(shape.clone());
                    }
                    let outside = cells.iter().filter(|&&i| !inside[i] && st(centre(i)).0 < bottom).count();
                    let whole = (outside as f64) < 0.3 * cells.len() as f64;
                    // The parent lies on one side of the limb: what is
                    // outside the limb's shape on the other side is still
                    // the limb's (a cuff's corner), not the parent's.
                    let body_side = st(base.bones[p].1).1.signum();
                    for &i in cells {
                        let (s, t) = st(centre(i));
                        let l = labels[i] as usize;
                        // Only this joint's two sides change hands.
                        if !(sub[l] || l == p) {
                            continue;
                        }
                        if whole || inside[i] {
                            if !sub[l] {
                                labels[i] = c as u8;
                            }
                        } else if s < bottom {
                            let edge = if body_side >= 0.0 { ra + rb * s } else { la + lb * s };
                            if (t - edge) * body_side > 0.0 {
                                if sub[l] {
                                    labels[i] = p as u8;
                                }
                            } else if !sub[l] {
                                labels[i] = c as u8;
                            }
                        }
                    }
                } else {
                    // Tucked under the parent: cut straight at the cut line,
                    // across the limb's width (and a margin).
                    let (la, lb) = fit(&|r| r.1, s_star, s_star + 0.35 * len);
                    let (ra, rb) = fit(&|r| r.2, s_star, s_star + 0.35 * len);
                    for &i in cells {
                        let (s, t) = st(centre(i));
                        if s < s_star - 0.5 * len || s > s_star + 0.7 * len {
                            continue;
                        }
                        let sc = s.max(s_star);
                        let (a, b) = (la + lb * sc, ra + rb * sc);
                        let margin = 0.3 * (b - a).abs();
                        if t < a - margin || t > b + margin {
                            continue;
                        }
                        let l = labels[i] as usize;
                        if s < s_star && sub[l] {
                            labels[i] = p as u8;
                        } else if s >= s_star && l == p {
                            labels[i] = c as u8;
                        }
                    }
                }
                continue;
            }
            let on_sub = cells.iter().filter(|&&i| sub[labels[i] as usize]).count();
            let straddles = on_sub > 0 && on_sub < cells.len();
            let mut count = vec![0usize; bones];
            let (mut s_sum, mut s_lo, mut s_hi) = (0.0, f64::INFINITY, f64::NEG_INFINITY);
            for &i in cells {
                let s = st(centre(i)).0;
                s_sum += s;
                s_lo = s_lo.min(s);
                s_hi = s_hi.max(s);
                count[labels[i] as usize] += 1;
            }
            let s_c = s_sum / cells.len() as f64;
            let c_pt = centroid(cells);
            // A small panel at the joint (a rolled cuff, a strap).
            let local =
                limbish && on_sub > 0 && s_hi - s_lo < 0.6 * len && (c_pt - (head + u * s_star)).hypot() < 0.6 * len;
            if !(straddles || local) {
                continue;
            }
            let to = if s_c < s_star {
                p
            } else if local && !straddles {
                continue;
            } else {
                (0..bones).filter(|b| sub[*b] || *b == p).max_by_key(|b| (count[*b], usize::MAX - b)).unwrap_or(p)
            };
            // Only this joint's two sides change hands: a limb an earlier
            // joint cut out of the panel keeps its cells.
            for &i in cells {
                let l = labels[i] as usize;
                if sub[l] || l == p {
                    labels[i] = to as u8;
                }
            }
        }
    }

    // 5b. A limb piece showing almost nothing of itself (a foot behind the
    // other leg, a hand behind the body) is its parent's: a sliver of the
    // parent cut off at the joint would turn on its own.
    for c in 0..bones {
        let Some(p) = parent[c] else { continue };
        if !matches!(base.bones[c].0, 4 | 6 | 8 | 10 | 11 | 12 | 13 | 14) {
            continue;
        }
        let (_, head, tip, _) = base.bones[c];
        let expected = (tip - head).hypot().max(cell) * 2.0 * base.radius[c];
        let have = labels.iter().zip(&cover).filter(|(l, k)| **l as usize == c && **k != NONE).count() as f64 * cell * cell;
        if have > 0.0 && have < 0.12 * expected {
            for (l, k) in labels.iter_mut().zip(&cover) {
                if *l as usize == c && *k != NONE {
                    *l = p as u8;
                }
            }
        }
    }

    // 6. The head ends at the chin: a head panel whose centre lies below the
    // face's lowest point is the neck or collar, the body's. The face is the
    // largest head panel in the skin's colour (the hands' main fill).
    if let (Some(hd), Some(ch)) = (bone_of(2), chest) {
        let mut skin: Vec<(usize, usize)> = Vec::new();
        for slot in [11, 12, 13, 14] {
            let Some(b) = bone_of(slot) else { continue };
            for i in 0..n {
                if labels[i] as usize == b && fill(i) {
                    let k = cover[i] as usize;
                    match skin.iter_mut().find(|(p, _)| paints[*p].2.paint == paints[k].2.paint) {
                        Some(e) => e.1 += 1,
                        None => skin.push((k, 1)),
                    }
                }
            }
            if !skin.is_empty() && slot == 12 {
                break;
            }
        }
        let skin = skin.iter().max_by_key(|e| e.1).map(|e| paints[e.0].2.paint.clone());
        let (_, head, tip, _) = base.bones[hd];
        let len = (tip - head).hypot();
        if let Some(skin) = skin
            && len > 1e-9
        {
            let u = (tip - head) / len;
            let face = panels
                .iter()
                .enumerate()
                .filter(|(id, cells)| {
                    !fixed[*id]
                        && cells.iter().filter(|&&i| labels[i] as usize == hd).count() * 2 > cells.len()
                        && {
                            let mut by: Vec<(usize, usize)> = Vec::new();
                            for &i in cells.iter() {
                                let k = cover[i] as usize;
                                match by.iter_mut().find(|(q, _)| paints[*q].2.paint == paints[k].2.paint) {
                                    Some(e) => e.1 += 1,
                                    None => by.push((k, 1)),
                                }
                            }
                            by.iter().max_by_key(|e| e.1).is_some_and(|e| paints[e.0].2.paint == skin)
                        }
                })
                .max_by_key(|(_, cells)| cells.len())
                .map(|(id, _)| id);
            if let Some(face) = face {
                let chin = panels[face].iter().map(|&i| (centre(i) - head).dot(u)).fold(f64::INFINITY, f64::min);
                for (id, cells) in panels.iter().enumerate() {
                    if fixed[id] || id == face || cells.iter().filter(|&&i| labels[i] as usize == hd).count() * 2 <= cells.len() {
                        continue;
                    }
                    if (centroid(cells) - head).dot(u) < chin {
                        for &i in cells {
                            labels[i] = ch as u8;
                        }
                    }
                }
            }
        }
    }

    // 6b. A scarf (gamcha) over the body: panels on the body in a colour of
    // their own (no other part's main colour), together a tenth or less of
    // the figure's width... hanging from above the shoulders well down the
    // body, taller than wide. It follows the body as an extra piece of its
    // own, stacked over the arms (under the head).
    let mut rank = rank;
    let mut parent = parent;
    let mut scarf: Option<usize> = None;
    if let Some(ch) = chest {
        let dominant = |cells: &[usize]| -> Option<usize> {
            let mut by: Vec<(usize, usize)> = Vec::new();
            for &i in cells {
                let k = cover[i] as usize;
                match by.iter_mut().find(|(q, _)| paints[*q].2.paint == paints[k].2.paint) {
                    Some(e) => e.1 += 1,
                    None => by.push((k, 1)),
                }
            }
            by.into_iter().max_by_key(|e| e.1).map(|e| e.0)
        };
        // Per panel: its label (all its cells share one by now, but for cut
        // limbs) and its main paint.
        let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut elsewhere: Vec<(usize, usize)> = Vec::new();
        let mut chest_cells = 0usize;
        for (id, cells) in panels.iter().enumerate() {
            let Some(k) = dominant(cells) else { continue };
            let on_chest = cells.iter().filter(|&&i| labels[i] as usize == ch).count();
            chest_cells += on_chest;
            if on_chest * 2 >= cells.len() && !fixed[id] {
                match groups.iter_mut().find(|(q, _)| paints[*q].2.paint == paints[k].2.paint) {
                    Some(e) => e.1.push(id),
                    None => groups.push((k, vec![id])),
                }
            }
            if on_chest < cells.len() {
                match elsewhere.iter_mut().find(|(q, _)| paints[*q].2.paint == paints[k].2.paint) {
                    Some(e) => e.1 += cells.len() - on_chest,
                    None => elsewhere.push((k, cells.len() - on_chest)),
                }
            }
        }
        let shoulder_y = [3usize, 5]
            .iter()
            .filter_map(|s| bone_of(*s).map(|b| base.bones[b].1.y))
            .fold(f64::NEG_INFINITY, f64::max);
        let (_, chest_head, _, _) = base.bones[ch];
        let main = groups.iter().max_by_key(|(_, ids)| ids.iter().map(|id| panels[*id].len()).sum::<usize>()).map(|g| g.0);
        let figure_w = base.bounds.width();
        for (k, ids) in &groups {
            if Some(*k) == main {
                continue;
            }
            let size: usize = ids.iter().map(|id| panels[*id].len()).sum();
            let other = elsewhere.iter().find(|(q, _)| paints[*q].2.paint == paints[*k].2.paint).map_or(0, |e| e.1);
            if (size as f64) < 0.04 * chest_cells as f64 || other * 10 > size {
                continue;
            }
            let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
            for id in ids {
                for &i in &panels[*id] {
                    let p = centre(i);
                    x0 = x0.min(p.x);
                    x1 = x1.max(p.x);
                    y0 = y0.min(p.y);
                    y1 = y1.max(p.y);
                }
            }
            if y0 < shoulder_y && y1 > chest_head.y && y1 - y0 > 1.5 * (x1 - x0) && x1 - x0 < 0.35 * figure_w {
                let s = *scarf.get_or_insert(bones);
                for id in ids {
                    for &i in &panels[*id] {
                        labels[i] = s as u8;
                    }
                    fixed[*id] = true;
                }
            }
        }
        if let Some(s) = scarf {
            // Over the arms, under the head.
            let at = bone_of(2).map_or(bones, |h| rank[h]);
            for r in rank.iter_mut() {
                if *r >= at {
                    *r += 1;
                }
            }
            rank.push(at);
            parent.push(None);
            limb_shape.push(None);
            debug_assert_eq!(s, bones);
        }
    }
    let bones = bones + scarf.is_some() as usize;

    // 7. Line cells: a line is the outline of the piece stacked over its
    // neighbour, so of the fills it joins (through line cells, within twice
    // the nearest's distance and a little), the front-most piece's.
    let mut dmin = vec![u32::MAX; n];
    {
        let mut front: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for i in 0..n {
            if fill(i) {
                dmin[i] = 0;
                front.push_back(i);
            }
        }
        while let Some(i) = front.pop_front() {
            for j in neighbours(i).into_iter().flatten() {
                if cover[j] != NONE && dmin[j] == u32::MAX {
                    dmin[j] = dmin[i] + 1;
                    front.push_back(j);
                }
            }
        }
    }
    let fill_labels = labels.clone();
    let mut best: Vec<Option<(usize, u32)>> = vec![None; n];
    let mut dist = vec![u32::MAX; n];
    for b in 0..bones {
        let mut front: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        let mut touched: Vec<usize> = Vec::new();
        for i in 0..n {
            if fill(i) && fill_labels[i] as usize == b {
                dist[i] = 0;
                touched.push(i);
                front.push_back(i);
            }
        }
        while let Some(i) = front.pop_front() {
            let d = dist[i] + 1;
            if d as i64 > LINE_WINDOW {
                continue;
            }
            for j in neighbours(i).into_iter().flatten() {
                if cover[j] != NONE && !fill(j) && dist[j] == u32::MAX {
                    dist[j] = d;
                    touched.push(j);
                    front.push_back(j);
                }
            }
        }
        for &i in &touched {
            if fill(i) {
                continue;
            }
            let window = (2 * dmin[i] + 3).min(LINE_WINDOW as u32);
            if dist[i] <= window && best[i].is_none_or(|(r, d)| rank[b] > r || (rank[b] == r && dist[i] < d)) {
                best[i] = Some((rank[b], dist[i]));
                labels[i] = b as u8;
            }
        }
        for &i in &touched {
            dist[i] = u32::MAX;
        }
    }

    // 8. Bare cells next to paint: the paint's label.
    let mut steps = vec![u32::MAX; n];
    let mut front: std::collections::VecDeque<usize> = (0..n).filter(|i| cover[*i] != NONE).collect();
    for &i in &front {
        steps[i] = 0;
    }
    while let Some(i) = front.pop_front() {
        if steps[i] >= BARE_CELLS {
            continue;
        }
        for j in neighbours(i).into_iter().flatten() {
            if cover[j] == NONE && steps[j] == u32::MAX {
                steps[j] = steps[i] + 1;
                labels[j] = labels[i];
                front.push_back(j);
            }
        }
    }

    let mut all_bones = base.bones.clone();
    let mut radius = base.radius.clone();
    if scarf.is_some()
        && let Some(ch) = chest
    {
        let (_, h, t, _) = base.bones[ch];
        all_bones.push((1, h, t, None));
        radius.push(base.radius[ch]);
    }
    let partition = BonePartition {
        bones: all_bones,
        radius,
        penalty: base.penalty,
        bounds: base.bounds,
        grid: std::cell::OnceCell::from(BoneGrid { origin, cell, cols, rows, labels }),
        regions: std::cell::OnceCell::new(),
    };
    Styled { partition, cover, line, rank, parent, limb_shape, scarf }
}

/// A bone's frame for measuring a piece: origin, along, across, length.
#[derive(Clone, Copy)]
struct Frame {
    origin: Point,
    u: Vec2,
    n: Vec2,
    len: f64,
}

impl Frame {
    fn new(origin: Point, u: Vec2, len: f64) -> Self {
        let u = if u.hypot() > 1e-9 { u / u.hypot() } else { Vec2::new(0.0, 1.0) };
        Self { origin, u, n: Vec2::new(-u.y, u.x), len: len.max(1e-6) }
    }
    fn st(&self, p: Point) -> (f64, f64) {
        let v = p - self.origin;
        (v.dot(self.u), v.dot(self.n))
    }
    fn at(&self, s: f64, t: f64) -> Point {
        self.origin + self.u * s + self.n * t
    }
}

/// What S2 adds to one piece.
struct Completion {
    /// The piece's whole shape as wanted (before clipping), figure space.
    shape: buzz_geom::BezPath,
    /// The line's paint and width.
    ink: Option<(buzz_scene::FillSpec, f64)>,
}

/// **S2.** A tucked limb continued under its parent: its two silhouette
/// edges near the cut carried on along the bone past the joint to the
/// style's drawn tip, closed with the style's end shape.
#[allow(clippy::too_many_arguments)]
fn limb_extension(
    st: &Styled,
    paints: &[(buzz_geom::BezPath, buzz_geom::FillMode, buzz_scene::FillSpec)],
    b: usize,
    frame: Frame,
    border_with: &[usize],
    reach: f64,
    roundness: f64,
    inner: Option<f64>,
) -> Option<Completion> {
    let g = st.partition.grid();
    let (cols, rows, cell, origin) = (g.cols, g.rows, g.cell, g.origin);
    let n = cols * rows;
    let centre = |i: usize| {
        Point::new(origin.x + ((i % cols) as f64 + 0.5) * cell, origin.y + ((i / cols) as f64 + 0.5) * cell)
    };
    let labels = &g.labels;
    let mine = |i: usize| labels[i] as usize == b && st.cover[i] != NONE;
    let cells: Vec<usize> = (0..n).filter(|&i| mine(i)).collect();
    if cells.len() < 16 {
        return None;
    }
    let neighbours = |i: usize| {
        let (c, r) = (i % cols, i / cols);
        [
            (c > 0).then(|| i - 1),
            (c + 1 < cols).then(|| i + 1),
            (r > 0).then(|| i - cols),
            (r + 1 < rows).then(|| i + cols),
        ]
    };
    // The cut: where the piece meets its parent.
    let mut border: Vec<f64> = cells
        .iter()
        .filter(|&&i| {
            neighbours(i)
                .into_iter()
                .flatten()
                .any(|j| st.cover[j] != NONE && border_with.contains(&(labels[j] as usize)))
        })
        .map(|&i| frame.st(centre(i)).0)
        .collect();
    let mut all_s: Vec<f64> = cells.iter().map(|&i| frame.st(centre(i)).0).collect();
    all_s.sort_by(f64::total_cmp);
    let s_cut = if border.len() >= 3 {
        border.sort_by(f64::total_cmp);
        border[border.len() / 2]
    } else {
        all_s[all_s.len() / 33]
    };
    let s_far = all_s[all_s.len() - 1];

    // Rows across the bone near the cut: their ends, whether each end is the
    // limb's own silhouette (not where a piece stacked over it starts), and
    // the line at each end.
    let bin = 2.0 * cell;
    let span = (s_far - s_cut).max(bin);
    let lo = s_cut + (0.04 * frame.len).max(2.0 * cell);
    let hi = s_cut + (0.5 * frame.len).min(span);
    let nb = (((hi - lo) / bin).ceil().max(1.0)) as usize;
    let mut rows_t: Vec<Vec<(f64, usize)>> = vec![Vec::new(); nb];
    for &i in &cells {
        let (s, t) = frame.st(centre(i));
        if s < lo || s >= hi {
            continue;
        }
        let k = (((s - lo) / bin) as usize).min(nb - 1);
        rows_t[k].push((t, i));
    }
    let cell_at = |p: Point| -> Option<usize> {
        let c = ((p.x - origin.x) / cell).floor();
        let r = ((p.y - origin.y) / cell).floor();
        (c >= 0.0 && r >= 0.0 && (c as usize) < cols && (r as usize) < rows).then(|| r as usize * cols + c as usize)
    };
    let free = |s: f64, t: f64, outward: f64| -> bool {
        for k in 1..=3 {
            let p = frame.at(s, t + outward * k as f64 * cell);
            if let Some(j) = cell_at(p)
                && st.cover[j] != NONE
                && labels[j] as usize != b
                && st.rank[labels[j] as usize] > st.rank[b]
            {
                return false;
            }
        }
        true
    };
    let (mut left, mut right): (Vec<(f64, f64)>, Vec<(f64, f64)>) = (Vec::new(), Vec::new());
    let mut widths: Vec<f64> = Vec::new();
    let mut inks: Vec<(usize, f64)> = Vec::new();
    for (k, row) in rows_t.iter_mut().enumerate() {
        if row.len() < 3 {
            continue;
        }
        row.sort_by(|a, b| a.0.total_cmp(&b.0));
        let s = lo + (k as f64 + 0.5) * bin;
        let (t0, t1) = (row[0].0, row[row.len() - 1].0);
        if free(s, t0, -1.0) {
            left.push((s, t0 - 0.5 * cell));
        }
        if free(s, t1, 1.0) {
            right.push((s, t1 + 0.5 * cell));
        }
        widths.push(t1 - t0 + cell);
        // The line at each end: its paint and thickness.
        for end in [0usize, 1] {
            let seq: Vec<&(f64, usize)> = if end == 0 { row.iter().collect() } else { row.iter().rev().collect() };
            let first = seq[0];
            let k0 = st.cover[first.1] as usize;
            if !st.line[k0] {
                continue;
            }
            let fill_at = seq.iter().find(|(_, i)| !st.line[st.cover[*i] as usize]);
            if let Some((tf, _)) = fill_at {
                inks.push((k0, (tf - first.0).abs()));
            }
        }
    }
    if widths.is_empty() {
        return None;
    }
    widths.sort_by(f64::total_cmp);
    let w0 = widths[widths.len() / 2];
    // A straight fit `t = a + b s` through one edge's points, its slope kept
    // to a gentle taper.
    let fit = |pts: &[(f64, f64)]| -> Option<(f64, f64)> {
        if pts.is_empty() {
            return None;
        }
        let m = pts.len() as f64;
        let (ms, mt) = (pts.iter().map(|p| p.0).sum::<f64>() / m, pts.iter().map(|p| p.1).sum::<f64>() / m);
        let (mut sxx, mut sxy) = (0.0, 0.0);
        for (s, t) in pts {
            sxx += (s - ms) * (s - ms);
            sxy += (s - ms) * (t - mt);
        }
        let slope = if pts.len() >= 3 && sxx > 1e-9 { (sxy / sxx).clamp(-0.3, 0.3) } else { 0.0 };
        Some((mt - slope * ms, slope))
    };
    let (fl, fr) = match (fit(&left), fit(&right)) {
        (Some(l), Some(r)) => (l, r),
        // One edge hidden all along (a thigh's inner edge under midCloth):
        // the other mirrored about the bone, but never narrower than the
        // rows.
        // A thigh's inner edge runs on under midCloth to the body's middle.
        (Some(l), None) => match inner {
            Some(ti) if ti > l.0 => (l, (ti.max(l.0 + w0), 0.0)),
            _ => (l, ((-l.0).max(l.0 + w0), -l.1)),
        },
        (None, Some(r)) => match inner {
            Some(ti) if ti < r.0 => ((ti.min(r.0 - w0), 0.0), r),
            _ => (((-r.0).min(r.0 - w0), -r.1), r),
        },
        (None, None) => ((-0.5 * w0, 0.0), (0.5 * w0, 0.0)),
    };
    let tl = |s: f64| fl.0 + fl.1 * s;
    let tr = |s: f64| fr.0 + fr.1 * s;

    // The line: the commonest end paint, its median thickness.
    let ink = {
        let mut by: Vec<(usize, Vec<f64>)> = Vec::new();
        for (k, w) in &inks {
            match by.iter_mut().find(|(p, _)| paints[*p].2.paint == paints[*k].2.paint) {
                Some(e) => e.1.push(*w),
                None => by.push((*k, vec![*w])),
            }
        }
        by.into_iter().max_by_key(|e| e.1.len()).map(|(k, mut ws)| {
            ws.sort_by(f64::total_cmp);
            (paints[k].2.clone(), ws[ws.len() / 2].max(cell))
        })
    };
    let lw = ink.as_ref().map_or(cell, |i| i.1);

    let s0 = s_cut + (2.0 * lw).max(0.06 * frame.len);
    let s_tip = s_cut - reach;
    if s_tip >= s0 - cell {
        return None;
    }
    // Width kept between half and one and a half the rows' width.
    let clamp_w = |s: f64| {
        let (a, b) = (tl(s), tr(s));
        let (c, w) = ((a + b) * 0.5, (b - a).clamp(0.5 * w0, 1.5 * w0));
        (c - w * 0.5, c + w * 0.5)
    };
    let (a_tip, b_tip) = clamp_w(s_tip);
    let w_tip = b_tip - a_tip;
    let d = (roundness.max(0.0) * w_tip * 0.5).min(s0 - s_tip);
    let s_base = s_tip + d;
    let (a0, b0) = clamp_w(s0);
    let (ab, bb) = clamp_w(s_base);
    let mut pts: Vec<Point> = vec![frame.at(s0, a0), frame.at(s_base, ab)];
    let (tc, half) = ((ab + bb) * 0.5, (bb - ab) * 0.5);
    let steps = 24;
    for k in 1..steps {
        let phi = std::f64::consts::PI * k as f64 / steps as f64;
        pts.push(frame.at(s_base - d * phi.sin(), tc - half * phi.cos()));
    }
    pts.push(frame.at(s_base, bb));
    pts.push(frame.at(s0, b0));
    Some(Completion { shape: polygon(&pts), ink })
}

/// **S2.** The band between a torso's two sides: per row across the body,
/// its ends where no piece stacked over it lies beside them, a straight line
/// fitted through each side's (a side never seen free mirrors the other).
fn torso_band(st: &Styled, b: usize, frame: Frame) -> Option<(buzz_geom::BezPath, buzz_geom::BezPath)> {
    let g = st.partition.grid();
    let (cols, rows, cell, origin) = (g.cols, g.rows, g.cell, g.origin);
    let labels = &g.labels;
    let centre = |i: usize| {
        Point::new(origin.x + ((i % cols) as f64 + 0.5) * cell, origin.y + ((i / cols) as f64 + 0.5) * cell)
    };
    let pts: Vec<(f64, f64)> =
        (0..cols * rows).filter(|&i| labels[i] as usize == b && st.cover[i] != NONE).map(|i| frame.st(centre(i))).collect();
    if pts.len() < 16 {
        return None;
    }
    let s_min = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let s_max = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let bin = 2.0 * cell;
    let nb = (((s_max - s_min) / bin).ceil().max(1.0)) as usize + 1;
    let mut ext = vec![(f64::INFINITY, f64::NEG_INFINITY); nb];
    for (s, t) in &pts {
        let k = (((s - s_min) / bin) as usize).min(nb - 1);
        ext[k].0 = ext[k].0.min(*t);
        ext[k].1 = ext[k].1.max(*t);
    }
    let cell_at = |p: Point| -> Option<usize> {
        let c = ((p.x - origin.x) / cell).floor();
        let r = ((p.y - origin.y) / cell).floor();
        (c >= 0.0 && r >= 0.0 && (c as usize) < cols && (r as usize) < rows).then(|| r as usize * cols + c as usize)
    };
    // Free: nothing stacked over the torso just beyond the edge.
    let free = |s: f64, t: f64, outward: f64| -> bool {
        (1..=4).all(|k| {
            let p = frame.at(s, t + outward * k as f64 * cell);
            cell_at(p).is_none_or(|j| st.cover[j] == NONE || labels[j] as usize == b)
        })
    };
    let (mut left, mut right, mut mids) = (Vec::new(), Vec::new(), Vec::new());
    for (k, e) in ext.iter().enumerate() {
        if e.1 < e.0 {
            continue;
        }
        let s = s_min + (k as f64 + 0.5) * bin;
        mids.push((e.0 + e.1) * 0.5);
        if free(s, e.0, -1.0) {
            left.push((s, e.0 - 0.5 * cell));
        }
        if free(s, e.1, 1.0) {
            right.push((s, e.1 + 0.5 * cell));
        }
    }
    let fit = |pts: &[(f64, f64)]| -> Option<(f64, f64)> {
        if pts.len() < 3 {
            return None;
        }
        let m = pts.len() as f64;
        let (ms, mt) = (pts.iter().map(|p| p.0).sum::<f64>() / m, pts.iter().map(|p| p.1).sum::<f64>() / m);
        let (mut sxx, mut sxy) = (0.0, 0.0);
        for (s, t) in pts {
            sxx += (s - ms) * (s - ms);
            sxy += (s - ms) * (t - mt);
        }
        let slope = if sxx > 1e-9 { (sxy / sxx).clamp(-0.15, 0.15) } else { 0.0 };
        Some((mt - slope * ms, slope))
    };
    mids.sort_by(f64::total_cmp);
    let mid = mids.get(mids.len() / 2).copied().unwrap_or(0.0);
    // A side with nothing free (an arm hanging against it all the way): at
    // least as wide as the torso is seen below the shoulders.
    let low = s_min + 0.6 * (s_max - s_min);
    let seen_left = ext.iter().enumerate().filter(|(k, e)| e.1 >= e.0 && s_min + *k as f64 * bin < low).map(|(_, e)| e.0).fold(f64::INFINITY, f64::min);
    let seen_right =
        ext.iter().enumerate().filter(|(k, e)| e.1 >= e.0 && s_min + *k as f64 * bin < low).map(|(_, e)| e.1).fold(f64::NEG_INFINITY, f64::max);
    let (fl, fr) = match (fit(&left), fit(&right)) {
        (Some(l), Some(r)) => (l, r),
        (Some(l), None) => (l, ((2.0 * mid - l.0).max(seen_right), 0.0)),
        (None, Some(r)) => (((2.0 * mid - r.0).min(seen_left), 0.0), r),
        (None, None) if seen_left.is_finite() && seen_right.is_finite() => ((seen_left, 0.0), (seen_right, 0.0)),
        (None, None) => return None,
    };
    // The shoulders' height: the top of the torso away from its middle
    // (the neck rises above it).
    let mut widths: Vec<f64> = ext.iter().filter(|e| e.1 >= e.0).map(|e| e.1 - e.0).collect();
    widths.sort_by(f64::total_cmp);
    let half = widths.get(widths.len() / 2).copied().unwrap_or(0.0) * 0.5;
    let mut tops: Vec<f64> = pts.iter().filter(|p| (p.1 - mid).abs() > 0.5 * half).map(|p| p.0).collect();
    tops.sort_by(f64::total_cmp);
    let shoulder = tops.get(tops.len() * 95 / 100).copied().unwrap_or(s_max);
    let band = |a: f64, z: f64| {
        polygon(&[
            frame.at(a, fl.0 + fl.1 * a),
            frame.at(z, fl.0 + fl.1 * z),
            frame.at(z, fr.0 + fr.1 * z),
            frame.at(a, fr.0 + fr.1 * a),
        ])
    };
    let a = s_min - 2.0 * cell;
    Some((band(a, s_max + 2.0 * cell), band(a, shoulder)))
}

/// The hidden art for one piece: `shape` filled from the piece's nearest
/// visible colours, outlined with `ink`, both kept inside `keep` (what the
/// piece and the pieces over it cover at rest, a cell in).
#[allow(clippy::too_many_arguments)]
fn hidden_art(
    st: &Styled,
    paints: &[(buzz_geom::BezPath, buzz_geom::FillMode, buzz_scene::FillSpec)],
    b: usize,
    completion: &Completion,
    keep: &buzz_geom::BezPath,
    opts: buzz_geom::BooleanOptions,
) -> Vec<(buzz_geom::BezPath, buzz_scene::FillSpec)> {
    let g = st.partition.grid();
    let (cols, rows) = (g.cols, g.rows);
    let n = cols * rows;
    let labels = &g.labels;
    let wanted = buzz_geom::boolean(&completion.shape, keep, buzz_geom::BoolOp::Intersect, opts);
    if wanted.elements().is_empty() || wanted.area().abs() <= g.cell * g.cell {
        return Vec::new();
    }
    // The shape on the grid, and each of its cells' nearest fill of the
    // piece (through the shape).
    let mut inside = vec![false; n];
    raster(g, &completion.shape, buzz_geom::FillMode::NonZero, |i| inside[i] = true);
    let mut from = vec![NONE; n];
    let mut front: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for i in 0..n {
        if labels[i] as usize == b && st.cover[i] != NONE && !st.line[st.cover[i] as usize] {
            from[i] = st.cover[i];
            front.push_back(i);
        }
    }
    while let Some(i) = front.pop_front() {
        let (c, r) = (i % cols, i / cols);
        for j in [
            (c > 0).then(|| i - 1),
            (c + 1 < cols).then(|| i + 1),
            (r > 0).then(|| i - cols),
            (r + 1 < rows).then(|| i + cols),
        ]
        .into_iter()
        .flatten()
        {
            if inside[j] && from[j] == NONE {
                from[j] = from[i];
                front.push_back(j);
            }
        }
    }
    // One region per colour (paints of one colour together).
    let mut groups: Vec<(buzz_scene::FillSpec, Vec<bool>)> = Vec::new();
    for i in 0..n {
        if !inside[i] || from[i] == NONE {
            continue;
        }
        let spec = &paints[from[i] as usize].2;
        let k = match groups.iter().position(|(s, _)| s.paint == spec.paint) {
            Some(k) => k,
            None => {
                groups.push((spec.clone(), vec![false; n]));
                groups.len() - 1
            }
        };
        groups[k].1[i] = true;
    }
    let mut out = Vec::new();
    // Grown by a cell, so neighbouring colours meet with no gap; the last
    // painted wins where they overlap.
    for (spec, mask) in &groups {
        let region = buzz_geom::expand_fill(&mask_path(g, mask, opts), g.cell, opts);
        let part = buzz_geom::boolean(&region, &wanted, buzz_geom::BoolOp::Intersect, opts);
        if !part.elements().is_empty() && part.area().abs() > g.cell * g.cell * 0.5 {
            out.push((part, spec.clone()));
        }
    }
    if out.is_empty() {
        // No fill to take the colour from: the piece's main fill there is none.
        return Vec::new();
    }
    if let Some((spec, width)) = &completion.ink {
        let inner = buzz_geom::expand_fill(&completion.shape, -width, opts);
        let ring = buzz_geom::boolean(&completion.shape, &inner, buzz_geom::BoolOp::Difference, opts);
        let ring = buzz_geom::boolean(&ring, keep, buzz_geom::BoolOp::Intersect, opts);
        if !ring.elements().is_empty() && ring.area().abs() > g.cell * g.cell * 0.5 {
            out.push((ring, spec.clone()));
        }
    }
    out
}

/// **S1/S2.** [`split_merged_drawings`] the style's way: see the module docs.
pub(super) fn split_merged_styled(
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    targets: Vec<(LayerId, ObjectId, PieceLabels)>,
    base: &BonePartition,
    capsules: &[buzz_ai::rerig::BoneCapsule],
    style: &CutStyle,
) -> Option<(Scene, MergedSplit)> {
    use std::collections::{HashMap, HashSet};
    let stack = &scene.library().get(symbol)?.layers;
    let target_ids: HashSet<(LayerId, ObjectId)> = targets.iter().map(|(l, o, _)| (*l, *o)).collect();
    let mut layer_ids: Vec<LayerId> = Vec::new();
    for (layer, _, _) in &targets {
        if !layer_ids.contains(layer) {
            layer_ids.push(*layer);
        }
    }
    let labelled: HashMap<(LayerId, ObjectId), PieceLabels> = targets.into_iter().map(|(l, o, p)| ((l, o), p)).collect();

    // Every paint on the layers, in the figure's space, back to front.
    let mut paints: Vec<(buzz_geom::BezPath, buzz_geom::FillMode, buzz_scene::FillSpec)> = Vec::new();
    for layer_id in &layer_ids {
        let layer = stack.get(*layer_id)?;
        let inherited = stack.inherited_transform(*layer_id, frame);
        for object in layer.objects_at(frame) {
            let read = match labelled.get(&(*layer_id, object.id)) {
                Some(p) => Some(p.leaves.iter().flat_map(|l| l.paints.iter().map(|p| (p.region.clone(), p.spec.rule, p.spec.clone()))).collect::<Vec<_>>()),
                None => {
                    let mut world = (**object).clone();
                    world.transform = inherited * world.transform;
                    piece_labels(scene, &world, base).map(|p| {
                        p.leaves.iter().flat_map(|l| l.paints.iter().map(|p| (p.region.clone(), p.spec.rule, p.spec.clone()))).collect()
                    })
                }
            };
            paints.extend(read.into_iter().flatten());
        }
    }
    let styled = styled_partition(base, &paints, style);
    let partition = &styled.partition;
    let bones_n = partition.bones.len();
    let seam = partition.seam();
    let nonzero = partition.opts(buzz_geom::FillMode::NonZero);
    let rank = &styled.rank;
    let mut order: Vec<usize> = (0..bones_n).collect();
    order.sort_by_key(|b| rank[*b]);

    // 1. Each layer read leaf by leaf against the style's plane: a leaf whole
    // on one bone, or cut (the parts of lower pieces reaching a cell under
    // the piece above, so no seam shows).
    struct LayerRead {
        id: LayerId,
        layer: Arc<buzz_scene::Layer>,
        shown: Vec<Arc<Object>>,
        inherited: Affine,
        by_bone: Vec<Vec<Object>>,
        kept: Vec<Arc<Object>>,
        parts_at: Option<usize>,
    }
    let mut reads: Vec<LayerRead> = Vec::new();
    // Each bone's visible paint, in the figure's space.
    let mut visible: Vec<Vec<buzz_geom::BezPath>> = vec![Vec::new(); bones_n];
    for layer_id in &layer_ids {
        let layer = stack.get(*layer_id)?.clone();
        let shown: Vec<Arc<Object>> = layer.objects_at(frame).to_vec();
        let inherited = stack.inherited_transform(*layer_id, frame);
        let mut read = LayerRead {
            id: *layer_id,
            layer: layer.clone(),
            shown: shown.clone(),
            inherited,
            by_bone: vec![Vec::new(); bones_n],
            kept: Vec::new(),
            parts_at: None,
        };
        let (_, grown) = partition.regions();
        for object in &shown {
            let mut world = (**object).clone();
            world.transform = inherited * world.transform;
            let Some(labels) = piece_labels(scene, &world, partition) else {
                read.kept.push(object.clone());
                continue;
            };
            read.parts_at.get_or_insert(read.kept.len());
            let world = inherited * object.transform;
            for l in labels.leaves {
                let back = invert(world * l.inner);
                let blend = match &l.object.kind {
                    ObjectKind::Shape(s) => s.blend,
                    _ => buzz_scene::PaintBlend::Normal,
                };
                let as_part = |path: buzz_geom::BezPath, spec: &buzz_scene::FillSpec| {
                    let mut o = (*l.object).clone();
                    o.transform = object.transform * l.inner;
                    o.kind = ObjectKind::Shape(buzz_scene::ShapeData {
                        path: back.unwrap_or(Affine::IDENTITY) * path,
                        fill: Some(spec.clone()),
                        stroke: None,
                        blend,
                    });
                    o
                };
                match (l.cut(), back) {
                    (Ok(_), Some(_)) => {
                        for paint in &l.paints {
                            if paint.parts.is_empty() {
                                let b = partition.bones_in(paint.region.bounding_box()).first().copied().unwrap_or(0);
                                visible[b].push(paint.region.clone());
                                read.by_bone[b].push(as_part(paint.region.clone(), &paint.spec));
                                continue;
                            }
                            let top = paint.parts.iter().map(|(b, _)| *b).max_by_key(|b| rank[*b]);
                            for (b, part) in &paint.parts {
                                let path = if Some(*b) != top {
                                    let reached = buzz_geom::boolean(
                                        &paint.region,
                                        &grown[*b],
                                        buzz_geom::BoolOp::Intersect,
                                        partition.opts(paint.spec.rule),
                                    );
                                    if reached.elements().is_empty() { part.clone() } else { reached }
                                } else {
                                    part.clone()
                                };
                                visible[*b].push(part.clone());
                                read.by_bone[*b].push(as_part(path, &paint.spec));
                            }
                        }
                    }
                    (cut, _) => {
                        let b = match cut {
                            Err(b) => b,
                            Ok(bones) => bones[0],
                        };
                        for paint in &l.paints {
                            visible[b].push(paint.region.clone());
                        }
                        let mut whole = (*l.object).clone();
                        whole.transform = object.transform * l.inner;
                        read.by_bone[b].push(whole);
                    }
                }
            }
        }
        reads.push(read);
    }
    let visible: Vec<buzz_geom::BezPath> =
        visible.iter().map(|v| if v.is_empty() { buzz_geom::BezPath::new() } else { buzz_geom::union_all(v, nonzero) }).collect();
    let present: Vec<bool> = visible.iter().map(|v| !v.elements().is_empty()).collect();

    // 2. S2: each piece completed under the pieces stacked over it.
    let g = partition.grid();
    let bone_of = |slot: usize| partition.bones.iter().position(|b| b.0 == slot);
    let frame_of = |b: usize| -> Frame {
        let (slot, head, tip, _) = partition.bones[b];
        let len = (tip - head).hypot();
        match slot {
            // The foot is continued up the shin, along the shin.
            13 | 14 => {
                let shin = bone_of(slot - 5).map(|k| partition.bones[k]);
                let u = shin.map_or(tip - head, |(_, h, t, _)| t - h);
                Frame::new(head, u, len)
            }
            // midCloth hangs from the body: down the body's bone.
            0 => {
                let down = bone_of(1).map_or(Vec2::new(0.0, 1.0), |c| partition.bones[c].1 - partition.bones[c].2);
                Frame::new(head, down, len)
            }
            _ => Frame::new(head, tip - head, len),
        }
    };
    let mut hidden: Vec<Vec<(buzz_geom::BezPath, buzz_scene::FillSpec)>> = vec![Vec::new(); bones_n];
    // The drawing's line: its darkest thin paint, and that paint's width.
    let drawing_ink: Option<(buzz_scene::FillSpec, f64)> = {
        let mut ink: Option<(buzz_scene::FillSpec, f64, f64)> = None;
        for (k, (path, _, spec)) in paints.iter().enumerate() {
            if !styled.line[k] {
                continue;
            }
            let buzz_scene::Paint::Solid(c) = &spec.paint else { continue };
            let c = c.to_rgba8();
            let luma = 0.299 * c.r as f64 + 0.587 * c.g as f64 + 0.114 * c.b as f64;
            let p: f64 = path_edges(path, g.cell * 0.25).iter().map(|(a, b)| (*b - *a).hypot()).sum();
            let w = if p > 1e-9 { 2.0 * path.area().abs() / p } else { 0.0 };
            if ink.as_ref().is_none_or(|(_, l, _)| luma < *l) {
                ink = Some((spec.clone(), luma, w));
            }
        }
        ink.map(|(s, _, w)| (s, w.max(g.cell)))
    };
    for b in 0..bones_n {
        if !present[b] || Some(b) == styled.scarf {
            continue;
        }
        let slot = partition.bones[b].0;
        let Some(cut) = style.slot(part_key(slot)) else { continue };
        // What covers this piece at rest: itself and every piece over it.
        let mut over: Vec<buzz_geom::BezPath> = vec![visible[b].clone()];
        for x in 0..bones_n {
            if present[x] && rank[x] > rank[b] {
                over.push(visible[x].clone());
            }
        }
        let keep = buzz_geom::expand_fill(&buzz_geom::union_all(&over, nonzero), -seam, nonzero);
        let completion = if slot == 1 {
            if !style.torso_complete_under_arms {
                continue;
            }
            // The torso continued under the arms and the head: its hull.
            let mut pts = Vec::new();
            for i in 0..g.labels.len() {
                if g.labels[i] as usize == b && styled.cover[i] != NONE {
                    let (c, r) = (i % g.cols, i / g.cols);
                    let p = Point::new(g.origin.x + c as f64 * g.cell, g.origin.y + r as f64 * g.cell);
                    pts.extend([p, p + Vec2::new(g.cell, 0.0), p + Vec2::new(0.0, g.cell), p + Vec2::new(g.cell, g.cell)]);
                }
            }
            let hull = convex_hull(pts);
            if hull.len() < 3 {
                continue;
            }
            // Kept between the torso's own sides: each side's silhouette
            // where nothing lies over it, carried straight up and down (an
            // arm hanging beside the torso covers its side, not more).
            // The torso is drawn whole between its sides, up to its
            // shoulders, under the arms laid over it (and its neck, from the
            // hull, under the head).
            let hull = match torso_band(&styled, b, frame_of(b)) {
                Some((band, capped)) => buzz_geom::boolean(
                    &buzz_geom::boolean(&polygon(&hull), &band, buzz_geom::BoolOp::Intersect, nonzero),
                    &capped,
                    buzz_geom::BoolOp::Union,
                    nonzero,
                ),
                None => polygon(&hull),
            };
            Completion { shape: hull, ink: drawing_ink.clone() }
        } else if !cut.tucked && matches!(slot, 3 | 5) {
            // An upper arm laid over the body: its drawn shape, where a
            // piece over it (a scarf) hides it.
            let Some(shape) = styled.limb_shape[b].clone() else { continue };
            Completion { shape, ink: drawing_ink.clone() }
        } else {
            if !cut.tucked || slot == 2 {
                continue;
            }
            let frame = frame_of(b);
            let border: Vec<usize> = match slot {
                0 => bone_of(1).into_iter().collect(),
                _ => styled.parent[b].into_iter().collect(),
            };
            // midCloth's length is its own, from the body's hem down.
            let frame = if slot == 0 {
                let ext = (0..g.labels.len())
                    .filter(|&i| g.labels[i] as usize == b && styled.cover[i] != NONE)
                    .map(|i| {
                        let p = Point::new(
                            g.origin.x + ((i % g.cols) as f64 + 0.5) * g.cell,
                            g.origin.y + ((i / g.cols) as f64 + 0.5) * g.cell,
                        );
                        frame.st(p).0
                    })
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, z), s| (a.min(s), z.max(s)));
                Frame { len: (ext.1 - ext.0).max(g.cell), ..frame }
            } else {
                frame
            };
            let reach = (cut.cut_pos + cut.tip_beyond_joint).max(0.0) * frame.len;
            let inner = match slot {
                7 | 9 => bone_of(0).map(|h| frame.st(partition.bones[h].1).1),
                _ => None,
            };
            let Some(c) = limb_extension(&styled, &paints, b, frame, &border, reach, cut.end_roundness, inner) else {
                continue;
            };
            c
        };
        hidden[b] = hidden_art(&styled, &paints, b, &completion, &keep, nonzero);
    }

    // 3. Pivots where the style puts them, from each piece's whole shape on
    // the grid (what it paints and its hidden art).
    let n = g.cols * g.rows;
    let mut full: Vec<Vec<bool>> = vec![Vec::new(); bones_n];
    for b in 0..bones_n {
        if !present[b] {
            continue;
        }
        let mut mask: Vec<bool> = (0..n).map(|i| g.labels[i] as usize == b && styled.cover[i] != NONE).collect();
        for (path, _) in &hidden[b] {
            raster(g, path, buzz_geom::FillMode::NonZero, |i| mask[i] = true);
        }
        full[b] = mask;
    }
    let centre = |i: usize| {
        Point::new(g.origin.x + ((i % g.cols) as f64 + 0.5) * g.cell, g.origin.y + ((i / g.cols) as f64 + 0.5) * g.cell)
    };
    let mut pivots: Vec<(usize, Point)> = Vec::new();
    for b in 0..bones_n {
        if !present[b] || Some(b) == styled.scarf {
            continue;
        }
        let slot = partition.bones[b].0;
        let Some(cut) = style.slot(part_key(slot)) else { continue };
        let frame = frame_of(b);
        let pivot = match cut.pivot {
            PivotPlace::OverlapCentre => {
                let parent = styled.parent[b].filter(|p| present[*p]);
                parent.and_then(|p| {
                    let (mut sum, mut count) = (Vec2::ZERO, 0usize);
                    for i in 0..n {
                        if full[b][i] && full[p][i] {
                            sum += centre(i).to_vec2();
                            count += 1;
                        }
                    }
                    (count > 0).then(|| (sum / count as f64).to_point())
                })
            }
            PivotPlace::FromTip(k) => {
                let pts: Vec<(f64, f64)> = (0..n).filter(|&i| full[b][i]).map(|i| frame.st(centre(i))).collect();
                (!pts.is_empty()).then(|| {
                    let s_min = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
                    let s_max = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
                    // The limb's width: the median row across its middle.
                    let (a, z) = (s_min + 0.3 * (s_max - s_min), s_min + 0.7 * (s_max - s_min));
                    let bins = 16usize;
                    let mut ext = vec![(f64::INFINITY, f64::NEG_INFINITY); bins];
                    for (s, t) in &pts {
                        if *s >= a && *s < z {
                            let k = (((s - a) / (z - a).max(1e-9)) * bins as f64) as usize;
                            let e = &mut ext[k.min(bins - 1)];
                            e.0 = e.0.min(*t);
                            e.1 = e.1.max(*t);
                        }
                    }
                    let mut ws: Vec<f64> = ext.iter().filter(|e| e.1 >= e.0).map(|e| e.1 - e.0).collect();
                    ws.sort_by(f64::total_cmp);
                    let w = ws.get(ws.len() / 2).copied().unwrap_or(0.0);
                    let s_p = s_min + k * w;
                    let row: Vec<f64> = pts.iter().filter(|p| (p.0 - s_p).abs() <= g.cell).map(|p| p.1).collect();
                    let t_c = if row.is_empty() {
                        0.0
                    } else {
                        (row.iter().copied().fold(f64::INFINITY, f64::min) + row.iter().copied().fold(f64::NEG_INFINITY, f64::max)) * 0.5
                    };
                    frame.at(s_p, t_c)
                })
            }
            PivotPlace::AlongBone(f) => {
                // The body: from the hips up to the neck.
                let hips = bone_of(0).map(|h| partition.bones[h].1);
                let neck = partition.bones[b].2;
                Some(match hips {
                    Some(h) => h + (neck - h) * f,
                    None => partition.bones[b].1 + (neck - partition.bones[b].1) * f,
                })
            }
        };
        if let Some(p) = pivot {
            pivots.push((slot, p));
        }
    }

    // 4. One part per slot on each layer, stacked as the style says: the
    // hidden art first, then what it paints. Hidden art goes on the first
    // layer holding the piece.
    let mut work = scene.clone();
    let mut originals: Vec<Arc<buzz_scene::Layer>> = Vec::new();
    let mut layers_out: Vec<buzz_scene::Layer> = Vec::new();
    let mut new_ids: Vec<ObjectId> = Vec::new();
    let mut parts_out: Vec<(ObjectId, usize)> = Vec::new();
    let mut overlaps: Vec<(usize, Point, f64)> = Vec::new();
    let mut unsplit_keys = 0;
    let mut hidden_placed = vec![false; bones_n];
    let mut extras: Vec<(ObjectId, String)> = Vec::new();
    for read in reads {
        let to_layer = invert(read.inherited).unwrap_or(Affine::IDENTITY);
        let mut by_bone = read.by_bone;
        let mut slot_parts: Vec<Arc<Object>> = Vec::new();
        for &b in &order {
            let mut run: Vec<Object> = Vec::new();
            if !by_bone[b].is_empty() && !hidden_placed[b] {
                hidden_placed[b] = true;
                for (path, spec) in &hidden[b] {
                    run.push(Object::shape(
                        ObjectId(0),
                        buzz_scene::ShapeData {
                            path: to_layer * path.clone(),
                            fill: Some(buzz_scene::FillSpec { rule: buzz_geom::FillMode::NonZero, ..spec.clone() }),
                            stroke: None,
                            blend: buzz_scene::PaintBlend::Normal,
                        },
                    ));
                }
                if !hidden[b].is_empty() {
                    let area: f64 = hidden[b].iter().map(|(p, _)| p.area().abs()).sum();
                    let at = pivots.iter().find(|(s, _)| *s == partition.bones[b].0).map_or(partition.bones[b].1, |p| p.1);
                    overlaps.push((partition.bones[b].0, at, (area / std::f64::consts::PI).sqrt()));
                }
            }
            run.append(&mut by_bone[b]);
            if run.is_empty() {
                continue;
            }
            let id = work.next_object_id();
            new_ids.push(id);
            let object = if run.len() == 1 {
                let mut o = run.remove(0);
                o.id = id;
                o
            } else {
                let kids: Vec<Arc<Object>> = run
                    .drain(..)
                    .map(|mut o| {
                        o.id = work.next_object_id();
                        new_ids.push(o.id);
                        Arc::new(o)
                    })
                    .collect();
                Object::group(id, kids)
            };
            parts_out.push((id, partition.bones[b].0));
            if Some(b) == styled.scarf {
                extras.push((id, "scarf".to_string()));
            }
            slot_parts.push(Arc::new(object));
        }
        let mut out = read.kept;
        let at = read.parts_at.unwrap_or(out.len()).min(out.len());
        out.splice(at..at, slot_parts);
        let mut edited = (*read.layer).clone();
        for key in edited.frames.keyframes_mut() {
            if *key.objects == read.shown {
                key.objects = Arc::new(out.clone());
            } else if key.objects.iter().any(|o| target_ids.contains(&(read.id, o.id))) {
                unsplit_keys += 1;
            }
        }
        originals.push(read.layer.clone());
        layers_out.push(edited);
    }
    if parts_out.is_empty() {
        return None;
    }
    let _ = capsules;
    let stack = work.library_mut().layers_mut(symbol)?;
    let mut layers = Vec::new();
    for layer in layers_out {
        let id = layer.id;
        let copy = layer.clone();
        stack.update(id, move |l| *l = copy);
        layers.push(Arc::new(layer));
    }
    let merged = target_ids.len();
    Some((
        work,
        MergedSplit {
            originals,
            layers,
            new_ids,
            merged,
            parts: parts_out,
            overlaps,
            unsplit_keys,
            underlays: Vec::new(),
            pivots,
            extras,
        },
    ))
}
