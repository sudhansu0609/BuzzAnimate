//! **Even Out Lines** (`docs/EVEN_LINES_PLAN.md` E1): filled ink shapes, whose
//! thickness follows the drawing (an Ink trace's brush look), turned into real
//! **strokes of one width along each line** — the line-tool look.
//!
//! The ink is scan-converted into a mask at a scale where its typical line is
//! several pixels wide, and the mask goes through the line tracer's own
//! centreline pipeline ([`crate::trace_lines`]): skeleton, junction pairing,
//! pruning, one measured width per line. Ink too fat to be a line comes back as
//! a filled area and is kept as one. Everything is mapped back to the shapes'
//! own space.
//!
//! Deterministic: no randomness, and no hash-map order in the output.

use crate::bucket::{Grid, rasterise_fill};
use crate::object::{Paint, ShapeData};
use crate::trace::{LineStyle, TraceMode, TraceOptions, delta_e, srgb_to_lab};
use crate::trace_lines::{Straighten, closed_catmull_rom, edt, simplify_points, trace_core_opened};
use buzz_geom::{Affine, BezPath, PathEl, Point, Rect, Shape as _, Vec2, catmull_rom};
use rayon::prelude::*;
use peniko::Color;

/// The longest side of any mask, in pixels.
const MAX_GRID: usize = 4096;
/// The most mask pixels per document unit.
const MAX_PPU: f64 = 16.0;
/// The typical line is rasterised at least this wide, in pixels.
const TARGET_LINE_PX: f64 = 8.0;
/// … or this wide for a draft (a preview while a slider drags).
const DRAFT_LINE_PX: f64 = 3.0;
/// The longest side of the cheap measuring mask.
const MEASURE_GRID: usize = 1024;
/// Paper kept round the ink in a mask, in pixels.
const PAD_PX: f64 = 4.0;
/// Ink shapes closer than this in colour (CIELAB ΔE) are done together.
const GROUP_DELTA_E: f64 = 3.0;

/// What [`even_lines`] is asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvenLinesOptions {
    /// The target line width, in the shapes' own units.
    pub width: f64,
    /// **Keep thick and thin**: each line is `width × m / median(m)` (its own
    /// measured width `m`), clamped to 0.5–2× `width`; otherwise every line is
    /// exactly `width`. Either way a line is even along its length.
    pub keep_weight: bool,
    /// `0` the tracer's own line; up to `1` simplified harder, and from `0.5`
    /// built from straight segments with the corners kept.
    pub straighten: f64,
    /// Ink deeper than this radius (the tracer's `max_line_radius`, in the
    /// shapes' own units) stays a filled area.
    pub solid_above: f64,
    /// Work at a lower raster scale (a preview while a slider drags).
    pub draft: bool,
}

impl Default for EvenLinesOptions {
    fn default() -> Self {
        Self {
            width: 2.0,
            keep_weight: true,
            straighten: 0.0,
            solid_above: 8.0,
            draft: false,
        }
    }
}

/// What [`even_lines`] made.
#[derive(Debug, Clone, Default)]
pub struct EvenLines {
    /// The filled solid areas first, then the strokes, longest first.
    pub shapes: Vec<ShapeData>,
    /// How many of `shapes` are strokes (the last `strokes`).
    pub strokes: usize,
    /// How many of `shapes` are solid areas (the first `solids`).
    pub solids: usize,
    /// The median measured width of the strokes, in the shapes' own units.
    pub measured_median: Option<f64>,
    /// Each stroke's own measured width `m`, in `shapes[solids..]` order — so
    /// [`EvenLines::set_widths`] can re-width a preview without tracing again.
    pub measured: Vec<f64>,
    /// The deepest ink's distance-field radius, in the shapes' own units: a
    /// `solid_above` at or over it leaves no solid area, so the result is the
    /// same for any such `solid_above`.
    pub deepest: f64,
    /// E1e: how many pieces of ink are kept as drawn (among the solid areas):
    /// a line outside the silhouette on both sides, or a piece that yields
    /// neither a stroke nor a solid area.
    pub kept: usize,
    /// E1d: each stroke's silhouette pin, in `shapes[solids..]` order (empty
    /// when no silhouette was given), so [`EvenLines::set_widths`] keeps an
    /// outer line's outer edge where the old ink's was at any width.
    pins: Vec<Option<Pin>>,
}

impl EvenLines {
    /// Give every stroke its width for `width` / `keep_weight` (see
    /// [`EvenLinesOptions`]).
    pub fn set_widths(&mut self, width: f64, keep_weight: bool) {
        let width = width.max(0.01);
        let median = self.measured_median.unwrap_or(width).max(1e-9);
        for (i, (shape, &m)) in self.shapes[self.solids..].iter_mut().zip(&self.measured).enumerate() {
            if let Some(stroke) = shape.stroke.as_mut() {
                stroke.width = if keep_weight {
                    (width * m / median).clamp(0.5 * width, 2.0 * width)
                } else {
                    width
                };
                if let Some(pin) = self.pins.get(i).and_then(|p| p.as_ref()) {
                    shape.path = pin.build(stroke.width).unwrap_or_else(|| pin.original.clone());
                }
            }
        }
    }
}

/// The colour of a solidly filled shape.
fn fill_colour(shape: &ShapeData) -> Option<Color> {
    match &shape.fill.as_ref()?.paint {
        Paint::Solid(c) => Some(*c),
        _ => None,
    }
}

fn lab_of(c: Color) -> [f64; 3] {
    let c = c.to_rgba8();
    srgb_to_lab([c.r, c.g, c.b])
}

/// The given shapes split by colour (ΔE < 3), in order of first appearance.
fn group_by_colour(shapes: &[ShapeData]) -> Vec<(Color, Vec<&ShapeData>)> {
    let mut groups: Vec<([f64; 3], Color, Vec<&ShapeData>)> = Vec::new();
    for shape in shapes {
        let Some(colour) = fill_colour(shape) else {
            continue;
        };
        let lab = lab_of(colour);
        match groups.iter_mut().find(|g| delta_e(g.0, lab) < GROUP_DELTA_E) {
            Some(g) => g.2.push(shape),
            None => groups.push((lab, colour, vec![shape])),
        }
    }
    groups.into_iter().map(|(_, c, s)| (c, s)).collect()
}

fn bounds_of(shapes: &[&ShapeData]) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for s in shapes {
        let b = s.path.bounding_box();
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) {
            continue;
        }
        out = Some(match out {
            Some(o) => o.union(b),
            None => b,
        });
    }
    out.filter(|b| b.width() > 1e-9 || b.height() > 1e-9)
}

/// The scale that fits `bounds` (plus padding) into `cap` pixels on its long
/// side, and no more than [`MAX_PPU`].
fn cap_ppu(bounds: Rect, cap: usize, ppu: f64) -> f64 {
    let long = bounds.width().max(bounds.height()).max(1e-9);
    let fit = (cap as f64 - 2.0 * PAD_PX - 2.0).max(1.0) / long;
    ppu.min(fit).min(MAX_PPU).max(1e-6)
}

/// A mask of `shapes` at `ppu` pixels per unit: (`w`, `h`, ink, the area the
/// grid's origin stands for).
struct Mask {
    w: usize,
    h: usize,
    ink: Vec<bool>,
    area: Rect,
    ppu: f64,
}

fn rasterise(shapes: &[&ShapeData], bounds: Rect, ppu: f64) -> Mask {
    let pad = PAD_PX / ppu;
    let area = Rect::new(bounds.x0 - pad, bounds.y0 - pad, bounds.x1 + pad, bounds.y1 + pad);
    let w = ((area.width() * ppu).ceil() as usize).clamp(1, MAX_GRID);
    let h = ((area.height() * ppu).ceil() as usize).clamp(1, MAX_GRID);
    let mut grid = Grid::new(w, h);
    for s in shapes {
        rasterise_fill(&s.path, &mut grid, area, ppu);
    }
    let mut ink = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            ink[y * w + x] = grid.get(x, y);
        }
    }
    Mask { w, h, ink, area, ppu }
}

/// The median line width of a mask, in pixels: `2r - 1` of the distance field
/// along its ridge (the pixels at least as deep as all eight neighbours — the
/// middle of every line, without thinning the mask).
fn median_width_px(mask: &Mask) -> Option<f64> {
    let mut widths = ridge_widths_px(mask)?;
    widths.sort_by(|a, b| a.total_cmp(b));
    Some(widths[widths.len() / 2])
}

/// The width (`2r - 1`, in pixels) at every distance-field ridge pixel of
/// `mask`: the width along the middle of every line in it.
fn ridge_widths_px(mask: &Mask) -> Option<Vec<f64>> {
    if !mask.ink.iter().any(|&v| v) {
        return None;
    }
    let (w, h) = (mask.w, mask.h);
    let r = edt(&mask.ink, w, h);
    let mut widths: Vec<f64> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !mask.ink[i] {
                continue;
            }
            let mut ridge = true;
            'n: for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if (dx, dy) == (0, 0) || nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    if r[ny as usize * w + nx as usize] > r[i] {
                        ridge = false;
                        break 'n;
                    }
                }
            }
            if ridge {
                widths.push((2.0 * r[i] - 1.0).max(1.0));
            }
        }
    }
    (!widths.is_empty()).then_some(widths)
}

/// Step 2's cheap measure: the median line width of `shapes`, in their units.
fn measure(shapes: &[&ShapeData], bounds: Rect) -> Option<f64> {
    let ppu = cap_ppu(bounds, MEASURE_GRID, MAX_PPU);
    let mask = rasterise(shapes, bounds, ppu);
    let px = median_width_px(&mask)?;
    if px < 3.0 && ppu < MAX_PPU {
        // Too few pixels across a line to trust: once more, finer.
        let finer = cap_ppu(bounds, MAX_GRID / 2, ppu * 4.0 / px);
        if finer > ppu * 1.2 {
            let mask = rasterise(shapes, bounds, finer);
            if let Some(px) = median_width_px(&mask) {
                return Some(px / finer);
            }
        }
    }
    Some(px / ppu)
}

/// **The median line width of the given filled ink shapes** (step 2), in their
/// own units: what the Even Out Lines dialog offers as its default width.
/// `None` when there is no solid-filled ink among them.
pub fn measured_line_width(shapes: &[ShapeData]) -> Option<f64> {
    let refs: Vec<&ShapeData> = shapes.iter().filter(|s| fill_colour(s).is_some()).collect();
    let bounds = bounds_of(&refs)?;
    measure(&refs, bounds)
}

/// **Turn filled ink `shapes` into even-width strokes** (see the module doc
/// and `docs/EVEN_LINES_PLAN.md` E1). Shapes without a solid fill are ignored.
pub fn even_lines(shapes: &[ShapeData], opts: &EvenLinesOptions) -> EvenLines {
    even_lines_within(shapes, &[], opts)
}

/// [`even_lines`] **with the character's silhouette** (E1d): `silhouette` is
/// every filled shape of the ink's timeline (in the same space as `shapes`).
/// A line with a side outside it keeps that side's edge exactly where the old
/// ink's was (the stroke thins or grows inward); a line inside it on both
/// sides stays centred. Empty: no pinning (as [`even_lines`]).
pub fn even_lines_within(shapes: &[ShapeData], silhouette: &[ShapeData], opts: &EvenLinesOptions) -> EvenLines {
    let mut solids: Vec<ShapeData> = Vec::new();
    // E1e: original ink kept as drawn (among `solids`).
    let mut kept = 0usize;
    // (length, measured width, stroke, silhouette pin)
    let mut strokes: Vec<(f64, f64, ShapeData, Option<Pin>)> = Vec::new();
    let straighten = opts.straighten.clamp(0.0, 1.0);
    let mut deepest = 0.0f64;

    for (colour, group) in group_by_colour(shapes) {
        let Some(bounds) = bounds_of(&group) else {
            continue;
        };
        let Some(measured) = measure(&group, bounds) else {
            continue;
        };
        let target = if opts.draft { DRAFT_LINE_PX } else { TARGET_LINE_PX };
        let ppu = cap_ppu(bounds, MAX_GRID, target / measured.max(1e-6));
        let mask = rasterise(&group, bounds, ppu);
        if !mask.ink.iter().any(|&v| v) {
            continue;
        }
        let mut rgba = vec![255u8; mask.w * mask.h * 4];
        for (i, &ink) in mask.ink.iter().enumerate() {
            if ink {
                rgba[i * 4] = 0;
                rgba[i * 4 + 1] = 0;
                rgba[i * 4 + 2] = 0;
            }
        }
        let options = TraceOptions {
            mode: TraceMode::LineArt,
            line_style: LineStyle::Stroke,
            uniform_width: None,
            ink_threshold: Some(128),
            fill_enclosed: false,
            ink_masses: true,
            drop_background: true,
            min_line_length: 2.0 * ppu,
            max_line_radius: (opts.solid_above * ppu).max(0.5),
            speckle: 4,
            detail: 1.0,
            smooth: 0.5,
            line_smooth: 0.5,
            ..TraceOptions::default()
        };
        let hook = Straighten {
            extra_tolerance: straighten * 2.0 * ppu,
            polyline: straighten >= 0.5,
        };
        // Masses by opening (E1b): a line joined to a solid area stays a
        // line up to it, and runs on half a line width under it.
        let (report, deep_px) = trace_core_opened(mask.w, mask.h, &rgba, &options, hook, 0.5 * target);
        deepest = deepest.max(deep_px / mask.ppu);
        let back = Affine::translate((mask.area.x0, mask.area.y0)) * Affine::scale(1.0 / mask.ppu);
        // E1d: every stroke measured against the silhouette, in mask pixels.
        let mut pins: Vec<Option<Pin>> = Vec::new();
        if !silhouette.is_empty() {
            let probe = Probe::new(&mask, &group, silhouette, opts.draft, target);
            pins = report
                .shapes
                .par_iter()
                .map(|shape| {
                    shape.stroke.as_ref()?;
                    probe.pin(&shape.path, hook.polyline).map(|p| p.into_doc(back, mask.ppu))
                })
                .collect();
        }
        // E1e: ink pieces kept as drawn, and the traced shapes they replace.
        let mut dropped = vec![false; report.shapes.len()];
        if !silhouette.is_empty() {
            let fwd = Affine::scale(mask.ppu) * Affine::translate((-mask.area.x0, -mask.area.y0));
            let pieces = kept_pieces(&mask, &group, &report.shapes, &pins, fwd);
            for (index, d) in pieces.dropped.into_iter().enumerate() {
                dropped[index] = d;
            }
            kept += pieces.shapes.len();
            solids.extend(pieces.shapes);
        }
        for (index, mut shape) in report.shapes.into_iter().enumerate() {
            if dropped[index] {
                continue;
            }
            shape.path = back * shape.path.clone();
            match shape.stroke.as_mut() {
                Some(stroke) => {
                    let m = stroke.width / mask.ppu;
                    stroke.width = m;
                    stroke.paint = Paint::Solid(colour);
                    let length = shape.path.perimeter(0.1);
                    let pin = pins.get_mut(index).and_then(|p| p.take()).map(|mut p| {
                        p.original = shape.path.clone();
                        p
                    });
                    strokes.push((length, m, shape, pin));
                }
                None => {
                    if let Some(fill) = shape.fill.as_mut() {
                        fill.paint = Paint::Solid(colour);
                    }
                    solids.push(shape);
                }
            }
        }
    }

    // Longest first; the sort is stable, so ties keep the tracer's order.
    strokes.sort_by(|a, b| b.0.total_cmp(&a.0));
    let measured: Vec<f64> = strokes.iter().map(|s| s.1).collect();
    let measured_median = if measured.is_empty() {
        None
    } else {
        let mut m = measured.clone();
        m.sort_by(|a, b| a.total_cmp(b));
        Some(m[m.len() / 2])
    };
    let has_pins = strokes.iter().any(|s| s.3.is_some());
    let mut out = EvenLines {
        strokes: strokes.len(),
        solids: solids.len(),
        shapes: solids,
        measured_median,
        measured,
        deepest,
        kept,
        pins: Vec::new(),
    };
    for (_, _, shape, pin) in strokes {
        out.shapes.push(shape);
        if has_pins {
            out.pins.push(pin);
        }
    }
    out.set_widths(opts.width, opts.keep_weight);
    out
}

// ---------------------------------------------------------------------------
// E1e: ink kept as drawn
// ---------------------------------------------------------------------------

/// A stroke whose samples are outside the silhouette on both sides this
/// often (or more) is a line standing free of the character.
const BOTH_OUTSIDE_SHARE: f64 = 0.5;

/// What [`kept_pieces`] decided.
struct KeptPieces {
    /// The original ink of every kept piece (each source shape's subpaths in
    /// it, with the source's own fill).
    shapes: Vec<ShapeData>,
    /// Per traced shape: replaced by a kept piece.
    dropped: Vec<bool>,
}

/// The 8-connected pieces of `ink`: a label per pixel (0 = paper) and how
/// many.
fn label_pieces(ink: &[bool], w: usize, h: usize) -> (Vec<u32>, usize) {
    let mut label = vec![0u32; w * h];
    let mut count = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !ink[start] || label[start] != 0 {
            continue;
        }
        count += 1;
        label[start] = count as u32;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if ink[j] && label[j] == 0 {
                        label[j] = count as u32;
                        stack.push(j);
                    }
                }
            }
        }
    }
    (label, count)
}

/// The piece at or next to mask point `q` (0 = none).
fn piece_near(label: &[u32], w: usize, h: usize, q: Point) -> u32 {
    let (cx, cy) = (q.x.floor() as i64, q.y.floor() as i64);
    for (dx, dy) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1)] {
        let (x, y) = (cx + dx, cy + dy);
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
            let l = label[y as usize * w + x as usize];
            if l != 0 {
                return l;
            }
        }
    }
    0
}

/// The pieces a path (mask pixels) lies on.
fn pieces_of(label: &[u32], w: usize, h: usize, path: &BezPath) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    kurbo::flatten(path.iter(), 0.25, |el| {
        if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
            let l = piece_near(label, w, h, p);
            if l != 0 && !out.contains(&l) {
                out.push(l);
            }
        }
    });
    out
}

/// **E1e: ink kept as drawn.** A piece of ink (8-connected in `mask`) is kept
/// exactly as it was drawn (the traced shapes on it dropped) when it yields
/// neither a stroke nor a solid area (a bracelet's lobe the tracer prunes),
/// or when every stroke on it stands outside the silhouette on both sides
/// (no thinner line keeps both edges). `traced` and `pins` are the tracer's
/// shapes (mask pixels) and their pins; `fwd` maps the shapes' space to mask
/// pixels.
fn kept_pieces(mask: &Mask, group: &[&ShapeData], traced: &[ShapeData], pins: &[Option<Pin>], fwd: Affine) -> KeptPieces {
    let (w, h) = (mask.w, mask.h);
    let (label, count) = label_pieces(&mask.ink, w, h);
    let mut yields = vec![false; count + 1];
    let mut free = vec![true; count + 1];
    let mut lined = vec![false; count + 1];
    let on: Vec<Vec<u32>> = traced.iter().map(|s| pieces_of(&label, w, h, &s.path)).collect();
    for (index, (shape, pieces)) in traced.iter().zip(&on).enumerate() {
        let standing = shape.stroke.is_some()
            && pins.get(index).and_then(|p| p.as_ref()).is_some_and(|p| {
                let both = p.outer.iter().filter(|o| o[0] && o[1]).count();
                both as f64 >= BOTH_OUTSIDE_SHARE * p.outer.len().max(1) as f64
            });
        for &l in pieces {
            yields[l as usize] = true;
            if shape.stroke.is_some() {
                lined[l as usize] = true;
                free[l as usize] &= standing;
            } else {
                free[l as usize] = false;
            }
        }
    }
    let keep: Vec<bool> = (0..=count).map(|l| l != 0 && (!yields[l] || (lined[l] && free[l]))).collect();
    let dropped = on.iter().map(|pieces| !pieces.is_empty() && pieces.iter().all(|&l| keep[l as usize])).collect();
    let mut shapes = Vec::new();
    if keep.iter().any(|&k| k) {
        for source in group {
            // Its subpaths, each on the piece most of its outline lies on.
            let mut by_piece: Vec<(u32, BezPath)> = Vec::new();
            let mut sub = BezPath::new();
            let flush = |sub: &mut BezPath, by_piece: &mut Vec<(u32, BezPath)>| {
                if sub.elements().is_empty() {
                    return;
                }
                let mut votes: Vec<(u32, usize)> = Vec::new();
                kurbo::flatten((fwd * sub.clone()).iter(), 0.25, |el| {
                    if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
                        let l = piece_near(&label, w, h, p);
                        if l != 0 {
                            match votes.iter_mut().find(|v| v.0 == l) {
                                Some(v) => v.1 += 1,
                                None => votes.push((l, 1)),
                            }
                        }
                    }
                });
                if let Some(&(l, _)) = votes.iter().max_by_key(|v| v.1)
                    && keep[l as usize]
                {
                    match by_piece.iter_mut().find(|b| b.0 == l) {
                        Some(b) => b.1.extend(sub.iter()),
                        None => by_piece.push((l, sub.clone())),
                    }
                }
                *sub = BezPath::new();
            };
            for el in source.path.iter() {
                if matches!(el, PathEl::MoveTo(_)) {
                    flush(&mut sub, &mut by_piece);
                }
                sub.push(el);
            }
            flush(&mut sub, &mut by_piece);
            for (_, path) in by_piece {
                shapes.push(ShapeData { path, ..(*source).clone() });
            }
        }
    }
    KeptPieces { shapes, dropped }
}

// ---------------------------------------------------------------------------
// E1d: the silhouette does not move
// ---------------------------------------------------------------------------

/// How far past the old ink's edge (mask pixels) the silhouette is looked at
/// to tell an outer side of a line.
const OUTER_PROBE_PX: f64 = 1.0;
/// Marching step along a normal, in mask pixels.
const MARCH_PX: f64 = 0.5;
/// Spacing of a pinned stroke's samples, in mask pixels.
const SAMPLE_PX: f64 = 1.0;
/// Douglas-Peucker tolerance of a pinned stroke, in mask pixels.
const PIN_TOL_PX: f64 = 0.1;
/// A stroke's round end may cross the silhouette by at most this (mask px).
const END_SLACK_PX: f64 = 0.25;
/// Bisection steps when an edge is found on the exact outlines.
const BISECT_STEPS: usize = 9;
/// The directions across a round end that are checked, from straight on.
const FAN_DEG: [f64; 5] = [0.0, 30.0, -30.0, 60.0, -60.0];
/// E1e: the slanted directions each side of a line is also measured in, in
/// degrees from square to the line (either way along it): a round stroke can
/// cross a bumpy silhouette edge slantwise where it clears it square on (the
/// arm's bottom line at Guddu's bracelet).
const SIDE_DEG: [f64; 5] = [15.0, 30.0, 45.0, 60.0, 75.0];
/// How many slanted directions per side.
const SIDE_N: usize = 2 * SIDE_DEG.len();
/// A pinned stroke's offsets are smoothed over this many stroke widths of
/// its length (each way).
const SMOOTH_WIDTHS: f64 = 0.75;
/// Height of a band of the exact silhouette's edge index, in mask pixels.
const BAND_PX: f64 = 8.0;

/// One straight piece of a silhouette outline, in mask pixels.
#[derive(Debug, Clone, Copy)]
struct SilEdge {
    y0: f64,
    y1: f64,
    x0: f64,
    dxdy: f64,
    dir: i32,
    shape: u32,
}

/// The silhouette's own outlines (flattened finely), indexed by bands of rows:
/// an exact inside test (non-zero per shape, union of the shapes).
struct ExactSil {
    bands: Vec<Vec<SilEdge>>,
    h: f64,
}

impl ExactSil {
    fn new(paths: &[BezPath], h: usize) -> Self {
        let nb = (h as f64 / BAND_PX).ceil() as usize + 1;
        let mut bands: Vec<Vec<SilEdge>> = vec![Vec::new(); nb];
        for (si, path) in paths.iter().enumerate() {
            let mut subs: Vec<Vec<Point>> = Vec::new();
            let mut cur: Vec<Point> = Vec::new();
            kurbo::flatten(path.iter(), 0.02, |el| match el {
                PathEl::MoveTo(p) => {
                    if cur.len() > 1 {
                        subs.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                    cur.push(p);
                }
                PathEl::LineTo(p) => cur.push(p),
                PathEl::ClosePath => {
                    if cur.len() > 1 {
                        subs.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                }
                _ => {}
            });
            if cur.len() > 1 {
                subs.push(cur);
            }
            for sub in &subs {
                for i in 0..sub.len() {
                    let (a, b) = (sub[i], sub[(i + 1) % sub.len()]);
                    if (a.y - b.y).abs() < f64::EPSILON {
                        continue;
                    }
                    let (top, bot, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
                    if bot.y < 0.0 || top.y >= h as f64 {
                        continue;
                    }
                    let e = SilEdge {
                        y0: top.y,
                        y1: bot.y,
                        x0: top.x,
                        dxdy: (bot.x - top.x) / (bot.y - top.y),
                        dir,
                        shape: si as u32,
                    };
                    let b0 = ((top.y.max(0.0) / BAND_PX) as usize).min(nb - 1);
                    let b1 = ((bot.y.min(h as f64) / BAND_PX) as usize).min(nb - 1);
                    for band in &mut bands[b0..=b1] {
                        band.push(e);
                    }
                }
            }
        }
        Self { bands, h: h as f64 }
    }

    /// The crossings of row `y` (shape, x, dir), sorted by shape then x.
    fn crossings(&self, y: f64, out: &mut Vec<(u32, f64, i32)>) {
        out.clear();
        if !(0.0..self.h).contains(&y) {
            return;
        }
        for e in &self.bands[(y / BAND_PX) as usize] {
            if y >= e.y0 && y < e.y1 {
                out.push((e.shape, e.x0 + (y - e.y0) * e.dxdy, e.dir));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    }

    fn contains(&self, p: Point) -> bool {
        if !(0.0..self.h).contains(&p.y) {
            return false;
        }
        let mut acc: Vec<(u32, i32)> = Vec::new();
        for e in &self.bands[(p.y / BAND_PX) as usize] {
            if p.y >= e.y0 && p.y < e.y1 && e.x0 + (p.y - e.y0) * e.dxdy < p.x {
                match acc.iter_mut().find(|a| a.0 == e.shape) {
                    Some(a) => a.1 += e.dir,
                    None => acc.push((e.shape, e.dir)),
                }
            }
        }
        acc.iter().any(|a| a.1 != 0)
    }

    /// A `w` × `h` mask of the silhouette, sampled at pixel centres.
    fn mask(&self, w: usize, h: usize) -> Vec<bool> {
        let mut out = vec![false; w * h];
        let mut xs: Vec<(u32, f64, i32)> = Vec::new();
        for py in 0..h {
            self.crossings(py as f64 + 0.5, &mut xs);
            let mut i = 0;
            while i < xs.len() {
                let mut j = i;
                while j < xs.len() && xs[j].0 == xs[i].0 {
                    j += 1;
                }
                let mut winding = 0;
                for k in i..j.saturating_sub(1) {
                    winding += xs[k].2;
                    if winding != 0 {
                        let a = (xs[k].1 - 0.5).ceil().max(0.0) as usize;
                        let b = ((xs[k + 1].1 - 0.5).ceil().max(0.0) as usize).min(w);
                        for px in a..b {
                            out[py * w + px] = true;
                        }
                    }
                }
                i = j;
            }
        }
        out
    }
}

/// What a stroke is measured against (all in one group's mask pixels).
struct Probe {
    w: usize,
    h: usize,
    ink: Vec<bool>,
    sil: Vec<bool>,
    /// E1e: each pixel's distance to the nearest pixel outside the
    /// silhouette: far inside, no edge is looked for.
    depth: Vec<f64>,
    exact: Option<ExactSil>,
    /// The farthest an edge is looked for, in mask pixels.
    cap: f64,
}

impl Probe {
    fn new(mask: &Mask, group: &[&ShapeData], silhouette: &[ShapeData], draft: bool, line_px: f64) -> Self {
        let fwd = Affine::scale(mask.ppu) * Affine::translate((-mask.area.x0, -mask.area.y0));
        let area = mask.area;
        let mut paths: Vec<BezPath> = Vec::new();
        for s in silhouette.iter().chain(group.iter().copied()) {
            if s.fill.is_none() {
                continue;
            }
            let b = s.path.bounding_box();
            if b.x1 < area.x0 || b.x0 > area.x1 || b.y1 < area.y0 || b.y0 > area.y1 {
                continue;
            }
            paths.push(fwd * s.path.clone());
        }
        let exact = ExactSil::new(&paths, mask.h);
        let mut sil = exact.mask(mask.w, mask.h);
        for (s, &i) in sil.iter_mut().zip(&mask.ink) {
            *s |= i;
        }
        let depth = edt(&sil, mask.w, mask.h);
        Self {
            w: mask.w,
            h: mask.h,
            ink: mask.ink.clone(),
            sil,
            depth,
            exact: (!draft).then_some(exact),
            cap: 3.0 * line_px + 2.0,
        }
    }

    fn at(&self, mask: &[bool], q: Point) -> bool {
        if q.x < 0.0 || q.y < 0.0 {
            return false;
        }
        let (x, y) = (q.x as usize, q.y as usize);
        x < self.w && y < self.h && mask[y * self.w + x]
    }

    /// How far along `d` from `p` the first step off `mask` is.
    fn first_out(&self, mask: &[bool], p: Point, d: Vec2) -> Option<f64> {
        let mut t = 0.0;
        while t <= self.cap {
            if !self.at(mask, p + d * t) {
                return Some(t);
            }
            t += MARCH_PX;
        }
        None
    }

    /// The distance along `d` from `p` to the silhouette's edge (exact unless
    /// drafting); `None` past [`Probe::cap`].
    fn sil_edge(&self, p: Point, d: Vec2) -> Option<f64> {
        let t = self.first_out(&self.sil, p, d)?;
        let approx = (t - 0.5 * MARCH_PX).max(0.0);
        let Some(ex) = &self.exact else {
            return Some(approx);
        };
        // Bracket the exact edge near the mask's: walk out while inside, or
        // back while outside, in quarter pixels.
        const STEP: f64 = 0.25;
        let (mut lo, mut hi);
        if ex.contains(p + d * approx) {
            lo = approx;
            hi = approx + STEP;
            while ex.contains(p + d * hi) {
                if hi > approx + 3.0 {
                    return Some(approx);
                }
                lo = hi;
                hi += STEP;
            }
        } else {
            hi = approx;
            lo = (approx - STEP).max(0.0);
            while !ex.contains(p + d * lo) {
                if lo <= 0.0 || lo < approx - 3.0 {
                    return Some(lo.max(0.0));
                }
                hi = lo;
                lo = (lo - STEP).max(0.0);
            }
        }
        for _ in 0..BISECT_STEPS {
            let mid = 0.5 * (lo + hi);
            if ex.contains(p + d * mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(0.5 * (lo + hi))
    }

    /// Measure the stroke whose centreline is `path` (mask pixels).
    fn pin(&self, path: &BezPath, polyline: bool) -> Option<Pin> {
        let (points, closed) = resample(path, SAMPLE_PX)?;
        let n = points.len();
        let mut arc = vec![0.0; n];
        for i in 1..n {
            arc[i] = arc[i - 1] + (points[i] - points[i - 1]).hypot();
        }
        let length = arc[n - 1] + if closed { (points[0] - points[n - 1]).hypot() } else { 0.0 };
        let tangent = |i: usize| -> Vec2 {
            let (a, b) = if closed {
                (points[(i + n - 1) % n], points[(i + 1) % n])
            } else {
                (points[i.saturating_sub(1)], points[(i + 1).min(n - 1)])
            };
            let t = b - a;
            let l = t.hypot();
            if l < 1e-12 { Vec2::new(1.0, 0.0) } else { t / l }
        };
        let mut normals = Vec::with_capacity(n);
        let mut e = Vec::with_capacity(n);
        let mut outer = Vec::with_capacity(n);
        let mut side = Vec::with_capacity(n);
        for (i, &p) in points.iter().enumerate() {
            let t = tangent(i);
            let nv = Vec2::new(-t.y, t.x);
            normals.push(nv);
            // E1e: slantwise too, unless the edge is out of reach all round.
            let mut fs = [[f64::INFINITY; SIDE_N]; 2];
            let (px, py) = (p.x.max(0.0) as usize, p.y.max(0.0) as usize);
            let deep = px < self.w && py < self.h && self.depth[py * self.w + px] > self.cap + 1.0;
            if !deep {
                for (s, d) in [nv, -nv].into_iter().enumerate() {
                    for (k, deg) in SIDE_DEG.iter().enumerate() {
                        let (sn, cs) = deg.to_radians().sin_cos();
                        for (j, along) in [t, -t].into_iter().enumerate() {
                            fs[s][2 * k + j] = self.sil_edge(p, d * cs + along * sn).unwrap_or(f64::INFINITY);
                        }
                    }
                }
            }
            side.push(fs);
            let mut es = [f64::INFINITY; 2];
            let mut os = [false; 2];
            for (s, d) in [nv, -nv].into_iter().enumerate() {
                if let Some(ink) = self.first_out(&self.ink, p, d) {
                    let edge = (ink - 0.5 * MARCH_PX).max(0.0);
                    os[s] = !self.at(&self.sil, p + d * (edge + OUTER_PROBE_PX));
                }
                if let Some(edge) = self.sil_edge(p, d) {
                    es[s] = edge;
                }
                os[s] &= es[s].is_finite();
            }
            e.push(es);
            outer.push(os);
        }
        // The round ends: how far each end's last samples are from the edge
        // across the cap (straight on past the end, and 30° / 60° either
        // side; square to the line is the pin's own `e`).
        let mut tips = [Vec::new(), Vec::new()];
        if !closed && n > 2 {
            let reach = self.cap.min(0.5 * length);
            let fan = |p: Point, u: Vec2| -> [f64; 5] {
                let side = Vec2::new(-u.y, u.x);
                FAN_DEG.map(|deg| {
                    let (s, c) = deg.to_radians().sin_cos();
                    self.sil_edge(p, u * c + side * s).unwrap_or(f64::INFINITY)
                })
            };
            let u0 = -tangent(0);
            for i in 0..n / 2 {
                if arc[i] > reach {
                    break;
                }
                tips[0].push(fan(points[i], u0));
            }
            let u1 = tangent(n - 1);
            for i in (n / 2..n).rev() {
                if arc[n - 1] - arc[i] > reach {
                    break;
                }
                tips[1].push(fan(points[i], u1));
            }
        }
        Some(Pin {
            points,
            normals,
            arc,
            length,
            e,
            outer,
            side,
            tips,
            closed,
            polyline,
            tol: PIN_TOL_PX,
            slack: END_SLACK_PX,
            original: path.clone(),
        })
    }
}

/// `path` flattened and resampled every `step` along its length (its first
/// subpath); whether it is closed.
fn resample(path: &BezPath, step: f64) -> Option<(Vec<Point>, bool)> {
    let mut poly: Vec<Point> = Vec::new();
    let mut closed = false;
    let mut started = false;
    kurbo::flatten(path.iter(), 0.05, |el| match el {
        PathEl::MoveTo(p) => {
            if !started {
                started = true;
                poly.push(p);
            } else {
                closed = true; // a second subpath: stop at the first
            }
        }
        PathEl::LineTo(p) if !closed => poly.push(p),
        PathEl::ClosePath => closed = true,
        _ => {}
    });
    let closed = closed && path.elements().iter().any(|e| matches!(e, PathEl::ClosePath));
    if closed && poly.len() > 1 && (poly[0] - poly[poly.len() - 1]).hypot() > 1e-9 {
        poly.push(poly[0]);
    }
    if poly.len() < 2 {
        return None;
    }
    let mut out = vec![poly[0]];
    let mut need = step;
    for seg in poly.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let l = (b - a).hypot();
        let mut at = 0.0;
        while l - at >= need {
            at += need;
            out.push(a + (b - a) * (at / l));
            need = step;
        }
        need -= l - at;
    }
    let last = poly[poly.len() - 1];
    if closed {
        // The loop's start is not repeated.
        if out.len() > 1 && (out[out.len() - 1] - out[0]).hypot() < 0.5 * step {
            out.pop();
        }
    } else if (out[out.len() - 1] - last).hypot() > 1e-6 {
        out.push(last);
    }
    (out.len() >= 2).then_some((out, closed))
}

/// **A stroke's silhouette pin** (E1d): its centreline samples, the normal at
/// each, how far the silhouette's edge is on either side (`e[i][0]` along the
/// normal, `[1]` against it), which sides are outer, and how far past each
/// open end the edge is. In the shapes' own units.
#[derive(Debug, Clone)]
struct Pin {
    points: Vec<Point>,
    normals: Vec<Vec2>,
    arc: Vec<f64>,
    length: f64,
    e: Vec<[f64; 2]>,
    outer: Vec<[bool; 2]>,
    /// E1e: per sample and side, the edge distances slantwise
    /// ([`SIDE_DEG`], each toward either end of the line).
    side: Vec<[[f64; SIDE_N]; 2]>,
    /// Per open end, its last samples' edge distances across the cap (in
    /// [`FAN_DEG`] directions from straight on; `+` turns toward the normal
    /// at the far end and away from it at the start).
    tips: [Vec<[f64; 5]>; 2],
    closed: bool,
    polyline: bool,
    tol: f64,
    slack: f64,
    /// The tracer's own centreline, kept when nothing needs pinning.
    original: BezPath,
}

impl Pin {
    fn into_doc(mut self, back: Affine, ppu: f64) -> Self {
        let k = 1.0 / ppu;
        for p in &mut self.points {
            *p = back * *p;
        }
        for a in &mut self.arc {
            *a *= k;
        }
        self.length *= k;
        for e in &mut self.e {
            e[0] *= k;
            e[1] *= k;
        }
        for f in self.side.iter_mut().flatten().flatten() {
            *f *= k;
        }
        for t in self.tips.iter_mut().flatten().flatten() {
            *t *= k;
        }
        self.tol *= k;
        self.slack *= k;
        self
    }

    /// The centreline for a stroke `width` wide: on an outer side, moved in so
    /// its edge lies on the old edge; inner stretches centred, with a ramp
    /// between; open ends pulled back until their round cap fits. `None` when
    /// nothing moves (the tracer's own line stands).
    fn build(&self, width: f64) -> Option<BezPath> {
        let n = self.points.len();
        let hw = 0.5 * width;
        let mut off = vec![0.0f64; n];
        let mut pinned = vec![false; n];
        let mut bounds = vec![(f64::NEG_INFINITY, f64::INFINITY); n];
        for i in 0..n {
            let [ep, em] = self.e[i];
            let (mut lo, mut hi) = (hw - em, ep - hw);
            // E1e: the round stroke clears the edge slantwise too. A centre
            // moved `o` along the normal is `hw` or more from the edge point
            // `f` away at `θ` from the normal while `o ≤ f cosθ − √(hw² − f² sin²θ)`.
            for (s, fs) in self.side[i].iter().enumerate() {
                for (j, &f) in fs.iter().enumerate() {
                    let (sn, cs) = SIDE_DEG[j / 2].to_radians().sin_cos();
                    let across = f * sn;
                    if !f.is_finite() || hw <= across {
                        continue;
                    }
                    let limit = f * cs - (hw * hw - across * across).sqrt();
                    if s == 0 {
                        hi = hi.min(limit);
                    } else {
                        lo = lo.max(-limit);
                    }
                }
            }
            bounds[i] = (lo, hi);
            let mid = 0.5 * (ep - em);
            let o = match (self.outer[i][0], self.outer[i][1]) {
                (true, true) => Some(mid),
                (true, false) => Some(if lo > hi { mid } else { hi }),
                (false, true) => Some(if lo > hi { mid } else { lo }),
                (false, false) => {
                    if lo > hi {
                        Some(mid)
                    } else if hi < 0.0 {
                        Some(hi)
                    } else if lo > 0.0 {
                        Some(lo)
                    } else {
                        None
                    }
                }
            };
            if let Some(o) = o {
                off[i] = o;
                pinned[i] = true;
            }
        }
        // Stretches between pinned samples: a short gap is bridged, a long
        // one ramps back to the centre over `2 × width`.
        let ramp = (2.0 * width).max(1e-9);
        let rounds = if self.closed { 2 } else { 1 };
        let mut prev: Vec<Option<(f64, f64)>> = vec![None; n];
        let mut next: Vec<Option<(f64, f64)>> = vec![None; n];
        let mut last: Option<(f64, f64)> = None;
        for r in 0..rounds {
            for i in 0..n {
                let s = self.arc[i] + r as f64 * self.length;
                if pinned[i] {
                    last = Some((s, off[i]));
                } else if let Some((a, o)) = last {
                    prev[i] = Some((s - a, o));
                }
            }
        }
        last = None;
        for r in 0..rounds {
            for i in (0..n).rev() {
                let s = self.arc[i] - r as f64 * self.length;
                if pinned[i] {
                    last = Some((s, off[i]));
                } else if let Some((b, o)) = last {
                    next[i] = Some((b - s, o));
                }
            }
        }
        for i in 0..n {
            if pinned[i] {
                continue;
            }
            let o = match (prev[i], next[i]) {
                (Some((a, oa)), Some((b, ob))) if a + b <= 2.0 * ramp => (oa * b + ob * a) / (a + b).max(1e-12),
                (a, b) => {
                    let w = |x: Option<(f64, f64)>| x.map_or(0.0, |(d, o)| o * (1.0 - d / ramp).max(0.0));
                    w(a) + w(b)
                }
            };
            off[i] = o;
        }
        // Smoothed along the chain (no kinks where a line leaves the edge or
        // an edge is noisy), then kept from crossing the silhouette.
        let reach = SMOOTH_WIDTHS * width;
        let mut smooth = vec![0.0f64; n];
        for i in 0..n {
            let (mut sum, mut weight) = (0.0, 0.0);
            let mut add = |j: usize, d: f64| {
                let w = 1.0 - d / reach;
                if w > 0.0 {
                    sum += w * off[j];
                    weight += w;
                }
            };
            add(i, 0.0);
            for step in 1..n {
                let (back, fwd) = if self.closed {
                    ((i + n - step % n) % n, (i + step) % n)
                } else {
                    (i.wrapping_sub(step), i + step)
                };
                let mut any = false;
                if back < n {
                    let d = if self.closed {
                        (self.arc[i] - self.arc[back]).rem_euclid(self.length)
                    } else {
                        self.arc[i] - self.arc[back]
                    };
                    if d < reach {
                        add(back, d);
                        any = true;
                    }
                }
                if fwd < n {
                    let d = if self.closed {
                        (self.arc[fwd] - self.arc[i]).rem_euclid(self.length)
                    } else {
                        self.arc[fwd] - self.arc[i]
                    };
                    if d < reach {
                        add(fwd, d);
                        any = true;
                    }
                }
                if !any || 2 * step >= n {
                    break;
                }
            }
            smooth[i] = if weight > 0.0 { sum / weight } else { off[i] };
        }
        for i in 0..n {
            let (lo, hi) = bounds[i];
            off[i] = if lo <= hi { smooth[i].clamp(lo, hi) } else { 0.5 * (lo + hi) };
        }
        // Open ends: pulled back while the round cap (round the moved
        // centre) would cross the silhouette.
        let (mut k0, mut k1) = (0usize, n);
        if !self.closed {
            let fits = |fan: &[f64; 5], o: f64, sign: f64| {
                FAN_DEG.iter().zip(fan).all(|(deg, &f)| hw + sign * o * deg.to_radians().sin() <= f + self.slack)
            };
            k0 = (0..self.tips[0].len()).find(|&j| fits(&self.tips[0][j], off[j], -1.0)).unwrap_or(self.tips[0].len());
            let pulled = (0..self.tips[1].len())
                .find(|&j| fits(&self.tips[1][j], off[n - 1 - j], 1.0))
                .unwrap_or(self.tips[1].len());
            k1 = n - pulled;
            if k1 < k0 + 2 {
                (k0, k1) = (0, n);
            }
        }
        if !pinned.iter().any(|&p| p) && k0 == 0 && k1 == n {
            return None;
        }
        let centre: Vec<Point> = (k0..k1).map(|i| self.points[i] + self.normals[i] * off[i]).collect();
        let simplified = simplify_points(&centre, self.tol);
        if simplified.len() < 2 {
            return None;
        }
        Some(if self.polyline {
            let mut path = BezPath::new();
            path.move_to(simplified[0]);
            for &p in &simplified[1..] {
                path.line_to(p);
            }
            if self.closed {
                path.close_path();
            }
            path
        } else if self.closed {
            closed_catmull_rom(&simplified)
        } else {
            catmull_rom(&simplified)
        })
    }
}
