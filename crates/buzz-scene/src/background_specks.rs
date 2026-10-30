//! **Remove Background Specks** (A6b of `docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md`):
//! the geometry. A character traced before A6a carries the sheet's paper,
//! trapped between hair curls and body-hair strands, as small white or cream
//! fills. This finds them in vector art: the same rule as the tracer's
//! (`trace::background_pockets`), on the character's painted picture.
//!
//! The picture is scan-converted, top shape per pixel. A *light patch* is a
//! 4-connected set of pixels whose top shape's fill lies within 2 × `max_de`
//! of the paper colour; it is a speck when its mean colour is within `max_de`,
//! its area is at most `max_area`, none of its shapes is protected, and it lies
//! wholly within `reach` of the outside of the picture (the uncovered pixels
//! reachable from its edge; a hole inside is not the outside) or touches it. A shape
//! loses each of its pieces (an outer outline with its holes) whose visible
//! pixels all lie in specks; a piece nothing shows of is kept.
//!
//! **A6d: ink–paper rims.** The source's anti-aliasing of an outline against
//! the paper, traced as thin grey fills along hair tips, goes the same way: a
//! pixel within `rim_reach` of the outside, reachable from it through such
//! pixels (4-connected), whose top shape's fill lies on the blend from its ink
//! (the darkest top-shape colour within [`RIM_INK_UNITS`], when at most
//! [`RIM_INK_MAX_L`]) to the paper (`trace::ink_paper_blend`: at least
//! `rim_min_t` towards the paper, at most `rim_de` off the segment) is a rim
//! pixel, and a piece whose visible pixels are all specks or rim pixels goes.
//! Where a shape keeps other visible pixels, its rim pixels are cut out of it
//! instead ([`SpeckCut::clip`]: those grid pixels, grown [`RIM_CLIP_GROW_PX`] into whatever
//! does not show the shape, so the cut runs under the ink beside the rim).
//!
//! Lengths and areas are in the shapes' own units. Deterministic.

use crate::bucket::{Grid, rasterise_fill, trace_contours};
use crate::object::ShapeData;
use crate::trace::{delta_e, ink_paper_blend, srgb_to_lab};
use crate::trace_lines::edt;
use buzz_geom::{BezPath, PathEl, Point, Rect, Shape as _};
use rayon::prelude::*;

/// The longest side of the scan-conversion grid, in pixels.
const MAX_GRID: f64 = 2048.0;
/// The most grid pixels per unit.
const MAX_PPU: f64 = 4.0;
/// A patch touching the outside is at most this far (grid px) from it.
const TOUCH_PX: f64 = 1.5;
/// With no paper colour given, a fill counts towards guessing it when it is
/// this light (L*) …
const GUESS_MIN_L: f64 = 85.0;
/// … and this grey (chroma), and touches the outside.
const GUESS_MAX_CHROMA: f64 = 20.0;
/// … and of those, the ones within this L* of the lightest tenth.
const GUESS_LIGHTEST_L: f64 = 5.0;
/// A6d: a rim pixel's ink is the darkest top-shape colour within this
/// distance (units, Chebyshev) …
const RIM_INK_UNITS: f64 = 6.0;
/// … when that is at most this light (L*).
const RIM_INK_MAX_L: f64 = 45.0;
/// A6d: a rim cut grows this many grid pixels into what does not show the
/// shape (under the ink beside it), so no sliver of the fill peeks out.
const RIM_CLIP_GROW_PX: usize = 2;
/// No light patch here.
const NONE: u32 = u32::MAX;

/// The settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeckOptions {
    /// The sheet's paper colour; `None` guesses it: the per-channel median of
    /// the small (at most `max_area`) light, grey fills that touch the outside,
    /// the lightest of them (within 5 L* of their 90th percentile).
    pub colour: Option<[u8; 3]>,
    /// ΔE from the paper colour a speck's mean colour may be.
    pub max_de: f64,
    /// A speck lies wholly within this distance of the outside (or touches it).
    pub reach: f64,
    /// The largest speck, in square units.
    pub max_area: f64,
    /// A6d: the width (units) of the edge band where a fill coloured between
    /// the outline's ink and the paper is a rim; 0 keeps every rim.
    pub rim_reach: f64,
    /// A6d: ΔE a rim's colour may be off the ink-to-paper blend.
    pub rim_de: f64,
    /// A6d: how far towards the paper (0 = the ink, 1 = the paper) a rim's
    /// colour must be.
    pub rim_min_t: f64,
}

impl Default for SpeckOptions {
    fn default() -> Self {
        Self { colour: None, max_de: 10.0, reach: 12.0, max_area: 600.0, rim_reach: 3.0, rim_de: 8.0, rim_min_t: 0.3 }
    }
}

/// One shape of the picture, in the picture's space, bottom to top.
#[derive(Debug, Clone)]
pub struct SpeckShape {
    pub shape: ShapeData,
    /// Never a speck (an eye, a mouth).
    pub protected: bool,
}

/// What to take out of one shape.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeckCut {
    /// Index into the shapes given.
    pub shape: usize,
    /// The subpaths (in path order, counted from 0) to drop.
    pub subpaths: Vec<usize>,
    /// Every subpath goes: delete the shape.
    pub whole: bool,
    /// Square units taken out (visible).
    pub area: f64,
    /// A6d: a region (the picture's space) to cut out of what is left of the
    /// shape: its ink–paper rim where the shape shows elsewhere too.
    pub clip: Option<BezPath>,
}

/// The result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpeckReport {
    /// The paper colour used (`None`: none given and none found, nothing done).
    pub colour: Option<[u8; 3]>,
    pub cuts: Vec<SpeckCut>,
}

/// The subpaths of `path`, each as its own path.
pub fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) || out.is_empty() {
            out.push(BezPath::new());
        }
        out.last_mut().unwrap().push(*el);
    }
    out
}

/// `path` without the subpaths at `drop` (indices as [`subpaths`]).
pub fn without_subpaths(path: &BezPath, drop: &[usize]) -> BezPath {
    let mut out = BezPath::new();
    for (k, sub) in subpaths(path).into_iter().enumerate() {
        if !drop.contains(&k) {
            for el in sub.elements() {
                out.push(*el);
            }
        }
    }
    out
}

/// The pieces of a filled path: each outer subpath with the holes inside it
/// (subpaths wound against the largest one, assigned to the smallest outer
/// that contains their first point). Indices as [`subpaths`].
fn pieces(path: &BezPath) -> Vec<Vec<usize>> {
    let subs = subpaths(path);
    let areas: Vec<f64> = subs.iter().map(|s| s.area()).collect();
    let Some(big) = (0..subs.len()).max_by(|&a, &b| areas[a].abs().total_cmp(&areas[b].abs())) else {
        return Vec::new();
    };
    let sign = areas[big].signum();
    let outer: Vec<usize> = (0..subs.len()).filter(|&k| areas[k].signum() == sign || areas[k] == 0.0).collect();
    let mut out: Vec<Vec<usize>> = outer.iter().map(|&k| vec![k]).collect();
    for k in 0..subs.len() {
        if outer.contains(&k) {
            continue;
        }
        let first = subs[k].elements().iter().find_map(|el| match el {
            PathEl::MoveTo(p) => Some(*p),
            _ => None,
        });
        let host = first.and_then(|p| {
            outer
                .iter()
                .enumerate()
                .filter(|(_, o)| subs[**o].bounding_box().contains(p) && subs[**o].winding(p) != 0)
                .min_by(|a, b| areas[*a.1].abs().total_cmp(&areas[*b.1].abs()))
                .map(|(i, _)| i)
        });
        match host {
            Some(i) => out[i].push(k),
            None => out.push(vec![k]),
        }
    }
    out
}

/// The region a shape paints, and its colour (RGBA8): its fill, else its
/// stroke's outline.
fn painted(shape: &ShapeData) -> Option<(BezPath, [u8; 4])> {
    if let Some(fill) = &shape.fill {
        let c = fill.paint.color().to_rgba8();
        return Some((shape.path.clone(), [c.r, c.g, c.b, c.a]));
    }
    let stroke = shape.stroke.as_ref()?;
    let width = if stroke.hairline { 1.0 } else { stroke.width.max(0.1) };
    let style = kurbo::Stroke::new(width).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round);
    let outline = kurbo::stroke(shape.path.iter(), &style, &kurbo::StrokeOpts::default(), 0.05);
    let c = stroke.paint.color().to_rgba8();
    Some((outline, [c.r, c.g, c.b, c.a]))
}

/// The grid: origin, pixels per unit, size.
struct Frame {
    origin: Point,
    ppu: f64,
    w: usize,
    h: usize,
}

impl Frame {
    /// Grid pixels `path` covers (non-zero), as indices.
    fn cover(&self, path: &BezPath) -> Vec<usize> {
        let bb = path.bounding_box();
        if !(bb.width() > 0.0 && bb.height() > 0.0) {
            return Vec::new();
        }
        let x0 = (((bb.x0 - self.origin.x) * self.ppu).floor().max(0.0) as usize).min(self.w);
        let y0 = (((bb.y0 - self.origin.y) * self.ppu).floor().max(0.0) as usize).min(self.h);
        let x1 = (((bb.x1 - self.origin.x) * self.ppu).ceil().max(0.0) as usize).min(self.w);
        let y1 = (((bb.y1 - self.origin.y) * self.ppu).ceil().max(0.0) as usize).min(self.h);
        if x1 <= x0 || y1 <= y0 {
            return Vec::new();
        }
        let mut grid = Grid::new(x1 - x0, y1 - y0);
        let area = Rect::new(
            self.origin.x + x0 as f64 / self.ppu,
            self.origin.y + y0 as f64 / self.ppu,
            self.origin.x + x1 as f64 / self.ppu,
            self.origin.y + y1 as f64 / self.ppu,
        );
        rasterise_fill(path, &mut grid, area, self.ppu);
        let mut out = Vec::new();
        for y in 0..grid.h {
            for x in 0..grid.w {
                if grid.get(x, y) {
                    out.push((y0 + y) * self.w + x0 + x);
                }
            }
        }
        out
    }
}

/// **Find the background specks of a picture** (see the module doc).
pub fn find_background_specks(shapes: &[SpeckShape], options: &SpeckOptions) -> SpeckReport {
    let mut report = SpeckReport::default();
    let regions: Vec<Option<(BezPath, [u8; 4])>> =
        shapes.par_iter().map(|s| painted(&s.shape).filter(|(_, c)| c[3] > 127)).collect();
    let mut bounds: Option<Rect> = None;
    for (p, _) in regions.iter().flatten() {
        let b = p.bounding_box();
        if b.width() > 0.0 && b.height() > 0.0 && b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite() {
            bounds = Some(bounds.map_or(b, |r| r.union(b)));
        }
    }
    let Some(bounds) = bounds else { return report };
    let ppu = (MAX_GRID / bounds.width().max(bounds.height())).min(MAX_PPU);
    // A margin of outside all round, so the picture's edge is never its border.
    let margin = 4.0 / ppu;
    let bounds = bounds.inflate(margin, margin);
    let frame = Frame {
        origin: Point::new(bounds.x0, bounds.y0),
        ppu,
        w: (bounds.width() * ppu).ceil() as usize + 1,
        h: (bounds.height() * ppu).ceil() as usize + 1,
    };
    let (w, h) = (frame.w, frame.h);
    let covers: Vec<Vec<usize>> =
        regions.par_iter().map(|p| p.as_ref().map(|(path, _)| frame.cover(path)).unwrap_or_default()).collect();
    let mut top = vec![NONE; w * h];
    for (k, cover) in covers.iter().enumerate() {
        for &i in cover {
            top[i] = k as u32;
        }
    }
    // The outside: the uncovered pixels reachable from the grid's border (a
    // hole inside the picture, an empty eye symbol, is not the outside).
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h)
        .filter(|&i| {
            let (x, y) = (i % w, i / w);
            (x == 0 || y == 0 || x + 1 == w || y + 1 == h) && top[i] == NONE
        })
        .collect();
    for &i in &stack {
        outside[i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let n4 = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
        for j in n4.into_iter().flatten() {
            if !outside[j] && top[j] == NONE {
                outside[j] = true;
                stack.push(j);
            }
        }
    }
    let inside: Vec<bool> = outside.iter().map(|&o| !o).collect();
    let d_ext = edt(&inside, w, h);
    let lab: Vec<Option<[f64; 3]>> =
        regions.iter().map(|p| p.as_ref().map(|(_, c)| srgb_to_lab([c[0], c[1], c[2]]))).collect();

    // The paper colour: one vote per small light grey fill that touches the
    // outside (a vest or a shirt is one big fill and casts one vote; the
    // paper is many small ones).
    let colour = options.colour.or_else(|| {
        let mut votes: Vec<([u8; 3], usize)> = Vec::new();
        let small = options.max_area * ppu * ppu;
        for (k, p) in regions.iter().enumerate() {
            let (Some((_, c)), Some(l)) = (p, lab[k]) else { continue };
            if l[0] < GUESS_MIN_L || l[1].hypot(l[2]) > GUESS_MAX_CHROMA {
                continue;
            }
            let visible: Vec<usize> = covers[k].iter().copied().filter(|&i| top[i] == k as u32).collect();
            if !visible.is_empty() && visible.len() as f64 <= small && visible.iter().any(|&i| d_ext[i] <= TOUCH_PX) {
                votes.push(([c[0], c[1], c[2]], 1));
            }
        }
        if votes.is_empty() {
            return None;
        }
        // The paper is the lightest of them: the anti-aliased grey rim of a
        // line is light and touches the outside too. Keep the votes within
        // GUESS_LIGHTEST_L of the 90th percentile's L*.
        let mut ls: Vec<f64> = votes.iter().map(|(rgb, _)| srgb_to_lab(*rgb)[0]).collect();
        ls.sort_by(|a, b| a.total_cmp(b));
        let p90 = ls[((ls.len() - 1) as f64 * 0.9).round() as usize];
        votes.retain(|(rgb, _)| srgb_to_lab(*rgb)[0] >= p90 - GUESS_LIGHTEST_L);
        let total: usize = votes.iter().map(|v| v.1).sum();
        let mut out = [0u8; 3];
        for (c, slot) in out.iter_mut().enumerate() {
            let mut v: Vec<(u8, usize)> = votes.iter().map(|(rgb, n)| (rgb[c], *n)).collect();
            v.sort_unstable();
            let mut acc = 0;
            for (value, n) in v {
                acc += n;
                if 2 * acc >= total {
                    *slot = value;
                    break;
                }
            }
        }
        Some(out)
    });
    report.colour = colour;
    let Some(colour) = colour else { return report };
    let paper = srgb_to_lab(colour);
    let de = options.max_de.max(0.0);
    if de == 0.0 {
        return report;
    }
    let light: Vec<bool> = lab.iter().map(|l| l.is_some_and(|l| delta_e(l, paper) <= 2.0 * de)).collect();

    // Light patches, 4-connected, on the top shapes.
    let mut patch = vec![NONE; w * h];
    let mut qualifies: Vec<bool> = Vec::new();
    let reach = options.reach * ppu;
    let max_px = options.max_area * ppu * ppu;
    for s in 0..w * h {
        if top[s] == NONE || !light[top[s] as usize] || patch[s] != NONE {
            continue;
        }
        let id = qualifies.len() as u32;
        let mut px = vec![s];
        patch[s] = id;
        let mut k = 0;
        while k < px.len() {
            let i = px[k];
            k += 1;
            let (x, y) = (i % w, i / w);
            let n4 = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for j in n4.into_iter().flatten() {
                if top[j] != NONE && light[top[j] as usize] && patch[j] == NONE {
                    patch[j] = id;
                    px.push(j);
                }
            }
        }
        let mut mean = [0.0f64; 3];
        let (mut d_min, mut d_max) = (f64::INFINITY, 0.0f64);
        let mut protected = false;
        for &i in &px {
            let t = top[i] as usize;
            protected |= shapes[t].protected;
            let l = lab[t].unwrap_or(paper);
            for c in 0..3 {
                mean[c] += l[c];
            }
            d_min = d_min.min(d_ext[i]);
            d_max = d_max.max(d_ext[i]);
        }
        let n = px.len() as f64;
        let mean = [mean[0] / n, mean[1] / n, mean[2] / n];
        qualifies.push(
            !protected && n <= max_px && delta_e(mean, paper) <= de && (d_max <= reach || d_min <= TOUCH_PX),
        );
    }

    // A6d: the ink–paper rims along the outside.
    let mut rim = vec![false; w * h];
    let rim_reach = options.rim_reach * ppu;
    if rim_reach > 0.0 {
        let r = (RIM_INK_UNITS * ppu).ceil().max(1.0) as i64;
        let (rim_de, min_t) = (options.rim_de, options.rim_min_t);
        let cand: Vec<bool> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let t = top[i];
                if t == NONE || shapes[t as usize].protected || d_ext[i] > rim_reach {
                    return false;
                }
                let Some(c) = lab[t as usize] else { return false };
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                let mut darkest: Option<[f64; 3]> = None;
                for ny in (y - r).max(0)..=(y + r).min(h as i64 - 1) {
                    for nx in (x - r).max(0)..=(x + r).min(w as i64 - 1) {
                        let u = top[ny as usize * w + nx as usize];
                        if u == NONE {
                            continue;
                        }
                        if let Some(l) = lab[u as usize] {
                            if darkest.is_none_or(|d| l[0] < d[0]) {
                                darkest = Some(l);
                            }
                        }
                    }
                }
                let Some(ink) = darkest.filter(|d| d[0] <= RIM_INK_MAX_L) else { return false };
                let (t, d) = ink_paper_blend(c, ink, paper);
                t >= min_t && d <= rim_de
            })
            .collect();
        let mut stack: Vec<usize> = (0..w * h).filter(|&i| outside[i]).collect();
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let n4 = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for j in n4.into_iter().flatten() {
                if cand[j] && !rim[j] {
                    rim[j] = true;
                    stack.push(j);
                }
            }
        }
    }

    // Each light (or rim) shape's pieces.
    let speck = |i: usize| (patch[i] != NONE && qualifies[patch[i] as usize]) || rim[i];
    // `BUZZ_SPECKS_AT=x,y` (the picture's units): what the grid holds there.
    if let Some((px, py)) = std::env::var("BUZZ_SPECKS_AT").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?))
    }) {
        let gx = ((px - frame.origin.x) * ppu).floor().clamp(0.0, (w - 1) as f64) as usize;
        let gy = ((py - frame.origin.y) * ppu).floor().clamp(0.0, (h - 1) as f64) as usize;
        for y in gy.saturating_sub(2)..=(gy + 2).min(h - 1) {
            let row: Vec<String> = (gx.saturating_sub(3)..=(gx + 3).min(w - 1))
                .map(|x| {
                    let i = y * w + x;
                    match top[i] {
                        NONE => format!("{:>22}", if outside[i] { "out" } else { "hole" }),
                        t => {
                            let c = regions[t as usize].as_ref().map_or([0; 4], |r| r.1);
                            format!(
                                "{:>5}#{:02x}{:02x}{:02x}{}{}{:>3.1}",
                                t,
                                c[0],
                                c[1],
                                c[2],
                                if rim[i] { "R" } else { "." },
                                if speck(i) { "S" } else { "." },
                                d_ext[i]
                            )
                        }
                    }
                })
                .collect();
            eprintln!("specks at ({px}, {py}) row {y}: {}", row.join(" "));
        }
        let mut ts: Vec<u32> = Vec::new();
        for y in gy.saturating_sub(2)..=(gy + 2).min(h - 1) {
            for x in gx.saturating_sub(3)..=(gx + 3).min(w - 1) {
                let t = top[y * w + x];
                if t != NONE && !ts.contains(&t) {
                    ts.push(t);
                }
            }
        }
        for t in ts {
            let s = &shapes[t as usize].shape;
            eprintln!(
                "specks at: shape {t}: fill {:?}, stroke {:?}",
                s.fill.as_ref().map(|f| f.paint.color().to_rgba8()),
                s.stroke.as_ref().map(|k| (k.paint.color().to_rgba8(), k.width))
            );
        }
    }
    // A6d: shape `k`'s visible rim pixels, grown RIM_CLIP_GROW_PX into pixels that do not
    // show `k` (or show it as a speck), as a polygon in the picture's space,
    // with the count of visible rim pixels.
    let rim_clip = |k: usize| -> Option<(BezPath, usize)> {
        // Only a fill can be cut as a region (a stroke loses whole pieces).
        shapes[k].shape.fill.as_ref()?;
        let px: Vec<usize> = covers[k].iter().copied().filter(|&i| top[i] == k as u32 && rim[i]).collect();
        if px.is_empty() {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for &i in &px {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        // The growth and one pixel of margin round it.
        let g = RIM_CLIP_GROW_PX;
        let (gx0, gy0) = (x0.saturating_sub(g + 1), y0.saturating_sub(g + 1));
        let (gx1, gy1) = ((x1 + g + 1).min(w - 1), (y1 + g + 1).min(h - 1));
        let mut grid = Grid::new(gx1 - gx0 + 1, gy1 - gy0 + 1);
        for &i in &px {
            let (x, y) = (i % w, i / w);
            grid.set(x - gx0, y - gy0);
            for ny in y.saturating_sub(g)..=(y + g).min(h - 1) {
                for nx in x.saturating_sub(g)..=(x + g).min(w - 1) {
                    let j = ny * w + nx;
                    if top[j] != k as u32 || speck(j) {
                        grid.set(nx - gx0, ny - gy0);
                    }
                }
            }
        }
        let mut path = BezPath::new();
        for lp in trace_contours(&grid) {
            let pt = |&(x, y): &(usize, usize)| {
                Point::new(frame.origin.x + (x + gx0) as f64 / ppu, frame.origin.y + (y + gy0) as f64 / ppu)
            };
            path.move_to(pt(&lp[0]));
            for q in &lp[1..] {
                path.line_to(pt(q));
            }
            path.close_path();
        }
        (!path.elements().is_empty()).then_some((path, px.len()))
    };
    report.cuts = (0..shapes.len())
        .into_par_iter()
        .filter(|&k| !shapes[k].protected && covers[k].iter().any(|&i| top[i] == k as u32 && speck(i)))
        .filter_map(|k| {
            let path = &shapes[k].shape.path;
            let clip = rim_clip(k);
            let subs = subpaths(path);
            let pieces = if shapes[k].shape.fill.is_some() {
                pieces(path)
            } else {
                vec![(0..subs.len()).collect()]
            };
            let mut drop: Vec<usize> = Vec::new();
            let mut area = 0usize;
            for piece in &pieces {
                let mut piece_path = BezPath::new();
                for &s in piece {
                    for el in subs[s].elements() {
                        piece_path.push(*el);
                    }
                }
                let region = if shapes[k].shape.fill.is_some() {
                    piece_path
                } else {
                    match painted(&ShapeData { path: piece_path, ..shapes[k].shape.clone() }) {
                        Some((p, _)) => p,
                        None => continue,
                    }
                };
                let visible: Vec<usize> = frame.cover(&region).into_iter().filter(|&i| top[i] == k as u32).collect();
                if !visible.is_empty() && visible.iter().all(|&i| speck(i)) {
                    drop.extend(piece.iter().copied());
                    area += visible.len();
                }
            }
            if drop.is_empty() && clip.is_none() {
                return None;
            }
            drop.sort_unstable();
            let whole = drop.len() == subs.len();
            let (clip, clip_px) = match clip {
                Some((c, n)) if !whole => (Some(c), n),
                _ => (None, 0),
            };
            Some(SpeckCut { shape: k, subpaths: drop, whole, area: (area + clip_px) as f64 / (ppu * ppu), clip })
        })
        .collect();
    report
}
