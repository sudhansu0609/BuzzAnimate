//! **Ink traced as smooth filled shapes** — vectorizer.ai style, for
//! `TraceMode::Ink`.
//!
//! Where [`crate::trace`] finds *regions of flat colour* and
//! [`crate::trace_lines`] finds a pencil stroke's *centreline*, this finds
//! the ink itself — the dark line or mass a character sheet is drawn with —
//! and traces its own boundary as a filled shape with a sub-pixel edge:
//! same width as the source (plus a chosen bold), continuous through every
//! junction, no staircase.
//!
//! # The pipeline
//!
//! A. Build the ink coverage field `F` at native resolution: a thin dark
//!    line against its *local* background (skin, paper, cloth) scores
//!    alongside a dark mass (hair, a pupil, a shoe) scored against a fixed
//!    luma — [`ink_coverage`].
//! B. Threshold `F` into a binary mask at `U`× the native resolution (bilinear,
//!    sub-pixel), clean specks and pinholes, and bold it.
//! C. Trace the mask's contours (reusing [`crate::bucket`]) and group the
//!    loops by connected component.
//! D. Smooth each loop with corners kept sharp.
//! E. Fit a Bézier curve through the smoothed points, falling back to
//!    Douglas–Peucker where the fit misbehaves.
//! F. Colour each component from its own darkest pixels, and build the
//!    shapes, largest first.
//!
//! Everything here is deterministic: no dependency on `HashMap` iteration
//! order anywhere in the output.

use crate::bucket::{Grid, trace_contours};
use crate::object::ShapeData;
use crate::trace::{TraceOptions, TraceReport};
use crate::trace_tuning::InkTuning;
use buzz_geom::{BezPath, Point, Shape as _};
use peniko::Color;
use rayon::prelude::*;

// ---------------------------------------------------------------------------
// A. Ink coverage field
// ---------------------------------------------------------------------------

// The local background's percentile and the band half-width are
// `InkTuning::bg_percentile` / `InkTuning::band_half_width_px`.

/// A pixel lighter than this luma AND more saturated (RGB max − min) than
/// `COLOUR_PIXEL_MIN_CHROMA` never counts as ink by local contrast: a red
/// bangle (luma 88, chroma 160) on skin. Anti-aliased mixes of dark ink and
/// skin reach chroma ≈ 120 (a bar of 100 ate the lighter half of Raju's
/// dark red cheek line), so the bar sits at 140.
const COLOUR_PIXEL_MIN_LUMA: f64 = 60.0;
const COLOUR_PIXEL_MIN_CHROMA: f64 = 140.0;
/// The absolute-darkness rule only makes ink of near-neutral pixels (RGB
/// max − min at most this): hair, shoes, a belt, pupils are all ≤ 50; the
/// shaded mid-tones of a red bangle (138, 12, 11) are 128.
const MASS_MAX_CHROMA: f64 = 100.0;

// (Two ways of taming the shadowed waistband under Guddu's vest, luma 39–50,
// which straddles the dark ramp and traces as a band of spikes, were tried
// and reverted: dropping absolute-dark regions by their share of truly dark
// pixels punched white gaps into Raju's shoes and hair, which are the same
// darkness; a 3×3 median on the darkness input cost thin lines their
// absolute-darkness coverage, after which the band rule swallowed them with
// the cloth edges they touch. The colour stage paints that band over.)

/// Zero `f_local` over every 8-connected region of local-only ink (`f_local
/// ≥ ink_level`, `f_global < ink_level`) whose deepest pixel is further than
/// `band_half_width_px` from the region's edge. See
/// [`InkTuning::band_half_width_px`].
fn remove_bands(f_local: &mut [f32], f_global: &[f32], w: usize, h: usize, t: &InkTuning) {
    let mask: Vec<bool> = f_local
        .iter()
        .zip(f_global)
        .map(|(&l, &g)| l >= t.ink_level && g < t.ink_level)
        .collect();
    let (labels, n) = label_components(&mask, w, h);
    if n == 0 {
        return;
    }
    let depth = crate::trace_lines::edt(&mask, w, h);
    let mut deepest = vec![0.0f64; n as usize + 1];
    for i in 0..w * h {
        let l = labels[i] as usize;
        if l != 0 && depth[i] > deepest[l] {
            deepest[l] = depth[i];
        }
    }
    for i in 0..w * h {
        let l = labels[i] as usize;
        if l != 0 && deepest[l] > t.band_half_width_px {
            f_local[i] = 0.0;
        }
    }
}
// Coverage at or above `InkTuning::ink_level` is ink (step B5).
/// **Hysteresis.** A pixel only counts as ink when a pixel within its 3×3
/// neighbourhood has coverage at or above this: every real line has a solid
/// core right next to its anti-aliased edge, while the darker threads of a
/// plaid shirt, a stain on a vest or a crease barely drawn into cloth peak at
/// 0.55–0.65 and have no core at all, so they drop out whole. (Raising
/// `ink_level` to 0.65 instead also lost the lighter half of a faint but real
/// line — Raju's cheek line — and the thin outlines of shirt buttons.)
pub(crate) const SEED_LEVEL: f32 = 0.7;

/// Zero the coverage of every pixel that has no `SEED_LEVEL` pixel in its 3×3
/// neighbourhood. See [`SEED_LEVEL`].
pub(crate) fn require_core(f: &mut [f32], w: usize, h: usize) {
    let seeded: Vec<bool> = f.iter().map(|&v| v >= SEED_LEVEL).collect();
    let mut keep = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if !seeded[y * w + x] {
                continue;
            }
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    keep[yy * w + xx] = true;
                }
            }
        }
    }
    for (v, &k) in f.iter_mut().zip(&keep) {
        if !k {
            *v = 0.0;
        }
    }
}
// The local-contrast floor and the dark-mass luma and ramp are
// `InkTuning::min_contrast` / `dark_luma` / `dark_ramp`.
/// The ink's own core luma (step A.3) is the 1st percentile of luma over the
/// opaque pixels.
const CORE_PERCENTILE: f64 = 0.01;

/// **The ink coverage field**: `F` per native pixel, `0` (paper) to `1`
/// (solid ink). See [`crate::trace::ink_field`], which this is behind.
pub(crate) fn ink_coverage(width: u32, height: u32, pixels: &[u8], t: &InkTuning) -> Vec<f32> {
    ink_analysis(width, height, pixels, t).f
}

/// The coverage field with the measurements it was made from, for the
/// per-component decisions later in the trace.
pub(crate) struct InkAnalysis {
    /// Coverage `0..=1` per native pixel.
    pub(crate) f: Vec<f32>,
    /// Luma over white per native pixel.
    luma: Vec<f64>,
    /// The local background luma per native pixel (step A2).
    bg: Vec<u8>,
}

pub(crate) fn ink_analysis(width: u32, height: u32, pixels: &[u8], t: &InkTuning) -> InkAnalysis {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return InkAnalysis { f: vec![0.0; w * h], luma: vec![255.0; w * h], bg: vec![255; w * h] };
    }
    let (luma, opaque) = luma_over_white(pixels, w, h);
    let bins: Vec<u8> = luma.iter().map(|&l| l.round().clamp(0.0, 255.0) as u8).collect();
    let window = ink_window(w, h);
    let bg = crate::trace_lines::local_background(w, h, &bins, &opaque, window, t.bg_percentile);
    let core = global_percentile(&bins, &opaque, CORE_PERCENTILE) as f64;

    let mut f_local = vec![0.0f32; w * h];
    let mut f_global = vec![0.0f32; w * h];
    for i in 0..w * h {
        if !opaque[i] {
            continue;
        }
        let l = luma[i];
        let bg_i = bg[i] as f64;
        f_local[i] = if bg_i - core >= t.min_contrast {
            ((bg_i - l) / (bg_i - core)).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        // A bright, strongly saturated pixel (a red bangle, a sari motif) is
        // a colour, not ink, however much darker than its surroundings.
        let px = &pixels[i * 4..i * 4 + 4];
        let chroma = px[..3].iter().max().unwrap() - px[..3].iter().min().unwrap();
        if l > COLOUR_PIXEL_MIN_LUMA && chroma as f64 > COLOUR_PIXEL_MIN_CHROMA {
            f_local[i] = 0.0;
        }
        // Ink masses are near-neutral (hair, shoes, pupils: chroma ≤ 50). A
        // dark but saturated pixel — the shaded mid-tones of a red bangle at
        // luma 40–60, chroma 128 — is a colour, left for the colour stage.
        f_global[i] = if chroma as f64 > MASS_MAX_CHROMA {
            0.0
        } else {
            ((t.dark_luma - l) / t.dark_ramp).clamp(0.0, 1.0) as f32
        };
    }
    remove_bands(&mut f_local, &f_global, w, h, t);
    let mut f: Vec<f32> = f_local.iter().zip(&f_global).map(|(&l, &g)| l.max(g)).collect();
    require_core(&mut f, w, h);
    InkAnalysis { f, luma, bg }
}

/// `l = (0.299 R + 0.587 G + 0.114 B)·a + 255·(1 − a)`, `a = alpha/255`. No
/// blur: blur widens the anti-alias ramp and moves the edge. Returns the
/// luma and `alpha > 128`.
fn luma_over_white(pixels: &[u8], w: usize, h: usize) -> (Vec<f64>, Vec<bool>) {
    let mut luma = vec![255.0f64; w * h];
    let mut opaque = vec![false; w * h];
    for i in 0..w * h {
        let px = &pixels[i * 4..i * 4 + 4];
        let a = px[3] as f64 / 255.0;
        let l = 0.299 * px[0] as f64 + 0.587 * px[1] as f64 + 0.114 * px[2] as f64;
        luma[i] = l * a + 255.0 * (1.0 - a);
        opaque[i] = px[3] > 128;
    }
    (luma, opaque)
}

/// `W = odd(clamp(round(max(w,h)/55), 15, 41))` — the local-background
/// window's side, always odd.
pub(crate) fn ink_window(w: usize, h: usize) -> usize {
    let raw = (w.max(h) as f64 / 55.0).round();
    let clamped = raw.clamp(15.0, 41.0) as usize;
    if clamped % 2 == 0 { clamped + 1 } else { clamped }
}

/// The smallest luma bin whose cumulative share of the opaque pixels reaches
/// `percentile`. `255` when there is no opaque pixel at all.
fn global_percentile(bins: &[u8], opaque: &[bool], percentile: f64) -> u8 {
    let mut hist = [0u32; 256];
    let mut count = 0u32;
    for (i, &b) in bins.iter().enumerate() {
        if opaque[i] {
            hist[b as usize] += 1;
            count += 1;
        }
    }
    if count == 0 {
        return 255;
    }
    let target = (percentile * count as f64).ceil() as u32;
    let mut cum = 0u32;
    for (b, &n) in hist.iter().enumerate() {
        cum += n;
        if cum >= target {
            return b as u8;
        }
    }
    255
}

// ---------------------------------------------------------------------------
// B. Sub-pixel binary mask at U×
// ---------------------------------------------------------------------------

/// `U = 4` under 2.5 M source pixels, `2` under 10 M, else `1`.
pub(crate) fn upsample_factor(w: usize, h: usize) -> usize {
    let area = w * h;
    if area <= 2_500_000 {
        4
    } else if area <= 10_000_000 {
        2
    } else {
        1
    }
}

/// The `U` to trace at: [`upsample_factor`] when `t.upsample` is `None`
/// (Auto), else that value clamped to `1..=6`.
pub(crate) fn upsample_for(w: usize, h: usize, t: &InkTuning) -> usize {
    match t.upsample {
        Some(u) => u.clamp(1, 6),
        None => upsample_factor(w, h),
    }
}

/// `F` bilinearly resampled at `(x + 0.5)/U − 0.5` and thresholded at
/// `level` (the ink level, `0.5` by default), one row at a time in parallel.
/// Sample coordinates are clamped into the source picture, so the mask
/// replicates the edge rather than fading to paper past it.
pub(crate) fn threshold_mask(w: usize, h: usize, f: &[f32], u: usize, level: f32) -> Vec<bool> {
    let (uw, uh) = (w * u, h * u);
    let mut mask = vec![false; uw * uh];
    mask.par_chunks_mut(uw).enumerate().for_each(|(y, row)| {
        let sy = (y as f64 + 0.5) / u as f64 - 0.5;
        for (x, cell) in row.iter_mut().enumerate() {
            let sx = (x as f64 + 0.5) / u as f64 - 0.5;
            *cell = sample_bilinear(f, w, h, sx, sy) >= level;
        }
    });
    mask
}

fn sample_bilinear(f: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let (tx, ty) = (x - x0 as f64, y - y0 as f64);
    let f00 = f[y0 * w + x0] as f64;
    let f10 = f[y0 * w + x1] as f64;
    let f01 = f[y1 * w + x0] as f64;
    let f11 = f[y1 * w + x1] as f64;
    let top = f00 + (f10 - f00) * tx;
    let bot = f01 + (f11 - f01) * tx;
    (top + (bot - top) * ty) as f32
}

/// `r = round(ink_bold · U)` cells. Collects every ink cell with a non-ink
/// 4-neighbour in the *undilated* mask, then sets every cell within `r` of
/// one of those around it — a disc grown from the boundary only, cheap next
/// to an EDT at `U`×.
fn bold_mask(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    if r == 0 {
        return mask.to_vec();
    }
    let mut boundary: Vec<(usize, usize)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !mask[i] {
                continue;
            }
            let edge = (x == 0 || !mask[i - 1])
                || (x + 1 == w || !mask[i + 1])
                || (y == 0 || !mask[i - w])
                || (y + 1 == h || !mask[i + w]);
            if edge {
                boundary.push((x, y));
            }
        }
    }
    let r2 = (r * r) as i64;
    let mut out = mask.to_vec();
    for (bx, by) in boundary {
        let (bx, by) = (bx as i64, by as i64);
        let x0 = (bx - r as i64).max(0);
        let x1 = (bx + r as i64).min(w as i64 - 1);
        let y0 = (by - r as i64).max(0);
        let y1 = (by + r as i64).min(h as i64 - 1);
        for yy in y0..=y1 {
            for xx in x0..=x1 {
                let (dx, dy) = (xx - bx, yy - by);
                if dx * dx + dy * dy <= r2 {
                    out[yy as usize * w + xx as usize] = true;
                }
            }
        }
    }
    out
}

/// **Bold or thin the `U`× mask by `bold` px per side** (`r = round(|bold| ·
/// U)` cells). Positive grows every shape ([`bold_mask`]). Negative erodes
/// it — [`bold_mask`] on the inverted mask — except every 8-connected
/// component whose deepest cell is less than `2·|bold|·U` from its edge,
/// which is left whole so no loop ever vanishes.
fn apply_bold(mask: Vec<bool>, w: usize, h: usize, bold: f64, u: usize) -> Vec<bool> {
    let r = (bold.abs() * u as f64).round() as usize;
    if r == 0 {
        return mask;
    }
    if bold > 0.0 {
        return bold_mask(&mask, w, h, r);
    }
    let (labels, n) = label_components(&mask, w, h);
    if n == 0 {
        return mask;
    }
    let depth = crate::trace_lines::edt(&mask, w, h);
    let mut deepest = vec![0.0f64; n as usize + 1];
    for i in 0..w * h {
        let l = labels[i] as usize;
        if l != 0 && depth[i] > deepest[l] {
            deepest[l] = depth[i];
        }
    }
    let floor = 2.0 * bold.abs() * u as f64;
    let inverted: Vec<bool> = mask.iter().map(|&m| !m).collect();
    let paper = bold_mask(&inverted, w, h, r);
    (0..w * h)
        .map(|i| mask[i] && (!paper[i] || deepest[labels[i] as usize] < floor))
        .collect()
}

/// **Fill the paper holes inside ink that `should_fill` says to** — every
/// 4-connected paper component that does not reach the picture's border is
/// handed over as its cell indices, and becomes ink when the predicate says
/// so. Used for pinholes (a speck of paper inside a shoe) and for the darker
/// gaps inside a mass (a lighter strand painted into hair): neither is a hole
/// worth a loop. An eye glint is paper-bright and stays.
pub(crate) fn fill_holes(mask: &mut [bool], w: usize, h: usize, should_fill: impl Fn(&[usize]) -> bool) {
    if w == 0 || h == 0 {
        return;
    }
    // Paper reachable from the border is outside.
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let seed = |i: usize, outside: &mut Vec<bool>, stack: &mut Vec<usize>| {
        if !mask[i] && !outside[i] {
            outside[i] = true;
            stack.push(i);
        }
    };
    for x in 0..w {
        seed(x, &mut outside, &mut stack);
        seed((h - 1) * w + x, &mut outside, &mut stack);
    }
    for y in 0..h {
        seed(y * w, &mut outside, &mut stack);
        seed(y * w + w - 1, &mut outside, &mut stack);
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if !mask[j] && !outside[j] {
                outside[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    // Every other paper cell is in a hole; measure each hole and fill the small ones.
    let mut seen = vec![false; w * h];
    let mut comp: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if mask[start] || outside[start] || seen[start] {
            continue;
        }
        comp.clear();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if !mask[j] && !outside[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if should_fill(&comp) {
            for &i in &comp {
                mask[i] = true;
            }
        }
    }
}

/// A paper hole of at most this many source px² inside ink is always filled
/// (see [`fill_holes`]).
pub(crate) const MAX_HOLE_PX2: f64 = 3.0;
// A larger hole is filled when it is dark: `InkTuning::dark_hole_max_px2` /
// `dark_hole_max_luma`.

/// **A faint mark is not ink.** A small component (under
/// `InkTuning::faint_max_px2`) whose core pixels stand out from their local
/// background by less than `InkTuning::faint_max_contrast` luma, and are not
/// themselves near-black (`> FAINT_MIN_LUMA`), is a crease barely drawn into
/// dark cloth: the eye hardly sees it in the source, so a solid black dash
/// for it is wrong.
const FAINT_MIN_LUMA: f64 = 30.0;

/// Per component: its size in cells, and the median luma and median local
/// contrast (`bg − luma`) of its core pixels (`F ≥ 0.9`) — `None` when it has
/// no core pixel at all.
struct ComponentStats {
    cells: usize,
    core_luma: Option<f64>,
    core_contrast: Option<f64>,
}

fn component_stats(
    w: usize,
    h: usize,
    u: usize,
    uw: usize,
    uh: usize,
    analysis: &InkAnalysis,
    labels: &[u32],
    n: u32,
) -> Vec<ComponentStats> {
    let mut cells = vec![0usize; n as usize + 1];
    for &l in labels {
        cells[l as usize] += 1;
    }
    let mut lumas: Vec<Vec<f64>> = vec![Vec::new(); n as usize + 1];
    let mut contrasts: Vec<Vec<f64>> = vec![Vec::new(); n as usize + 1];
    for y in 0..h {
        let cy = (u * y + u / 2).min(uh.saturating_sub(1));
        for x in 0..w {
            let i = y * w + x;
            if analysis.f[i] < 0.9 {
                continue;
            }
            let cx = (u * x + u / 2).min(uw.saturating_sub(1));
            let lbl = labels[cy * uw + cx] as usize;
            if lbl == 0 {
                continue;
            }
            lumas[lbl].push(analysis.luma[i]);
            contrasts[lbl].push(analysis.bg[i] as f64 - analysis.luma[i]);
        }
    }
    (1..=n as usize)
        .map(|c| ComponentStats {
            cells: cells[c],
            core_luma: median_f64(&mut lumas[c]),
            core_contrast: median_f64(&mut contrasts[c]),
        })
        .collect()
}

fn median_f64(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

fn is_faint_mark(stats: &ComponentStats, u: usize, t: &InkTuning) -> bool {
    let px2 = stats.cells as f64 / (u * u) as f64;
    match (stats.core_luma, stats.core_contrast) {
        (Some(luma), Some(contrast)) => {
            px2 < t.faint_max_px2 && contrast < t.faint_max_contrast && luma > FAINT_MIN_LUMA
        }
        _ => false,
    }
}

/// **A colour mark is not ink.** A component whose colour is lighter than this
/// luma *and* more saturated (max − min of RGB) than
/// `InkTuning::colour_mark_min_chroma` — a red bindi, a bangle, a sari motif,
/// a pen cap — passed the coverage field by contrast alone and is left for
/// the colour stage. Dark reddish lines (a mouth, a cheek line at luma ≈ 40)
/// are ink and stay.
const COLOUR_MARK_MIN_LUMA: f64 = 60.0;

fn is_colour_mark(rgb: [u8; 3], t: &InkTuning) -> bool {
    let luma = 0.299 * rgb[0] as f64 + 0.587 * rgb[1] as f64 + 0.114 * rgb[2] as f64;
    let max = rgb.iter().copied().max().unwrap_or(0) as f64;
    let min = rgb.iter().copied().min().unwrap_or(0) as f64;
    luma > COLOUR_MARK_MIN_LUMA && (max - min) > t.colour_mark_min_chroma
}

// ---------------------------------------------------------------------------
// C. Contours, grouped by component
// ---------------------------------------------------------------------------

/// 8-connected components of `mask`, raster-scan order (deterministic, no
/// `HashMap`). `0` is "not ink"; components are numbered `1..=n`.
pub(crate) fn label_components(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, u32) {
    let mut labels = vec![0u32; w * h];
    let mut next = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        next += 1;
        labels[start] = next;
        stack.push(start);
        while let Some(at) = stack.pop() {
            let (x, y) = (at % w, at / w);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if mask[n] && labels[n] == 0 {
                        labels[n] = next;
                        stack.push(n);
                    }
                }
            }
        }
    }
    (labels, next)
}

/// The component a traced loop belongs to: the ink cell on the right of its
/// first edge. `p = loop[0]`, heading `d` from `loop[1] − loop[0]` (`0`
/// right, `1` down, `2` left, `3` up, matching [`trace_contours`]'s own
/// convention). `None` when the loop does not land on a labelled cell —
/// should not happen for a real contour, but a loop is dropped rather than
/// panicking if it does.
pub(crate) fn loop_component(loop_pts: &[(usize, usize)], labels: &[u32], uw: usize, uh: usize) -> Option<u32> {
    let p = loop_pts[0];
    let q = loop_pts[1];
    let (dx, dy) = (q.0 as i64 - p.0 as i64, q.1 as i64 - p.1 as i64);
    let d = match (dx, dy) {
        (1, 0) => 0,
        (0, 1) => 1,
        (-1, 0) => 2,
        (0, -1) => 3,
        _ => return None,
    };
    let (px, py) = (p.0 as i64, p.1 as i64);
    let (cx, cy) = match d {
        0 => (px, py),
        1 => (px - 1, py),
        2 => (px - 1, py - 1),
        _ => (px, py - 1),
    };
    if cx < 0 || cy < 0 || cx as usize >= uw || cy as usize >= uh {
        return None;
    }
    let lbl = labels[cy as usize * uw + cx as usize];
    (lbl != 0).then_some(lbl)
}

// ---------------------------------------------------------------------------
// D. Smooth with corners kept
// ---------------------------------------------------------------------------

// A loop point turning more than `InkTuning::corner_deg` (step 13) is a corner.
/// A fitted run may leave the bounds of its points by at most this (px);
/// a cubic that bows out further is a bad fit and the run falls back to
/// Douglas–Peucker. See [`fit_run`].
const FIT_MARGIN_PX: f64 = 2.0;

/// Index `i` wrapped into `0..n`, for any `i` (positive or negative).
fn wrap(i: isize, n: usize) -> usize {
    i.rem_euclid(n as isize) as usize
}

/// The turning angle in degrees at index `i` of a closed loop of `n` points:
/// between `P[i] − P[i−k]` and `P[i+k] − P[i]`. `0` when either leg is
/// degenerate.
fn turning_angle_deg(points: &[Point], i: usize, k: usize, n: usize) -> f64 {
    let a = points[wrap(i as isize - k as isize, n)];
    let b = points[i];
    let c = points[wrap(i as isize + k as isize, n)];
    let v1 = b - a;
    let v2 = c - b;
    let (l1, l2) = (v1.hypot(), v2.hypot());
    if l1 < 1e-9 || l2 < 1e-9 {
        return 0.0;
    }
    let cos = (v1.dot(v2) / (l1 * l2)).clamp(-1.0, 1.0);
    cos.acos().to_degrees()
}

fn turning_angles(points: &[Point], k: usize) -> Vec<f64> {
    let n = points.len();
    (0..n).map(|i| turning_angle_deg(points, i, k, n)).collect()
}

/// A closed loop's corners: `angle > corner_deg` (70° by default) and the
/// maximum over `[i−k, i+k]`. Loops with `n < 4k` have none.
fn detect_corners(points: &[Point], k: usize, corner_deg: f64) -> Vec<bool> {
    let n = points.len();
    if n < 4 * k {
        return vec![false; n];
    }
    let angles = turning_angles(points, k);
    let mut out = vec![false; n];
    for i in 0..n {
        if angles[i] <= corner_deg {
            continue;
        }
        let mut is_max = true;
        for d in 1..=k {
            let b = wrap(i as isize - d as isize, n);
            let fwd = wrap(i as isize + d as isize, n);
            if angles[b] > angles[i] || angles[fwd] > angles[i] {
                is_max = false;
                break;
            }
        }
        if is_max {
            out[i] = true;
        }
    }
    out
}

/// A circular Gaussian over a closed sequence of points, kernel radius `3σ`.
/// `σ ≤ 0` leaves the points unchanged.
fn gaussian_smooth_circular(points: &[Point], sigma: f64) -> Vec<Point> {
    if sigma <= 0.0 || points.len() < 3 {
        return points.to_vec();
    }
    let n = points.len();
    let radius = (3.0 * sigma).ceil().max(1.0) as isize;
    let weights: Vec<f64> = (-radius..=radius)
        .map(|k| (-(k as f64 * k as f64) / (2.0 * sigma * sigma)).exp())
        .collect();
    let wsum: f64 = weights.iter().sum();
    (0..n)
        .map(|i| {
            let (mut sx, mut sy) = (0.0, 0.0);
            for (off, &wt) in weights.iter().enumerate() {
                let k = off as isize - radius;
                let p = points[wrap(i as isize + k, n)];
                sx += p.x * wt;
                sy += p.y * wt;
            }
            Point::new(sx / wsum, sy / wsum)
        })
        .collect()
}

/// A Gaussian over an *open* run (a loop's points between two corners,
/// inclusive), reflecting about each endpoint (`P[−j] = 2·P[0] − P[j]`) so
/// the endpoints themselves are never moved and the tangent there is kept.
/// `σ ≤ 0` leaves the points unchanged.
fn gaussian_smooth_open(points: &[Point], sigma: f64) -> Vec<Point> {
    let n = points.len();
    if sigma <= 0.0 || n < 3 {
        return points.to_vec();
    }
    let radius = (3.0 * sigma).ceil().max(1.0) as isize;
    let weights: Vec<f64> = (-radius..=radius)
        .map(|k| (-(k as f64 * k as f64) / (2.0 * sigma * sigma)).exp())
        .collect();
    let wsum: f64 = weights.iter().sum();
    let reflect = |j: isize| -> Point {
        if j < 0 {
            let m = ((-j) as usize).min(n - 1);
            let p0 = points[0];
            let pm = points[m];
            Point::new(2.0 * p0.x - pm.x, 2.0 * p0.y - pm.y)
        } else if j as usize >= n {
            let over = j as usize - (n - 1);
            let mirror = (n - 1).checked_sub(over).unwrap_or(0);
            let last = points[n - 1];
            let pm = points[mirror];
            Point::new(2.0 * last.x - pm.x, 2.0 * last.y - pm.y)
        } else {
            points[j as usize]
        }
    };
    let mut out = Vec::with_capacity(n);
    out.push(points[0]);
    for i in 1..n - 1 {
        let (mut sx, mut sy) = (0.0, 0.0);
        for (off, &wt) in weights.iter().enumerate() {
            let k = off as isize - radius;
            let p = reflect(i as isize + k);
            sx += p.x * wt;
            sy += p.y * wt;
        }
        out.push(Point::new(sx / wsum, sy / wsum));
    }
    out.push(points[n - 1]);
    out
}

/// The points from `start` to `end` inclusive, walking forward around a
/// closed loop of length `points.len()`, wrapping past the end back to the
/// start when the walk does not reach `end` directly.
fn ring_slice(points: &[Point], start: usize, end: usize) -> Vec<Point> {
    let n = points.len();
    let mut out = Vec::new();
    // `start == end` is a loop with a single corner: the run is the whole
    // loop, from the corner all the way round and back to it.
    let steps = if start == end { n } else { (end + n - start) % n };
    for s in 0..=steps {
        out.push(points[(start + s) % n]);
    }
    out
}

// ---------------------------------------------------------------------------
// E. Curve fit
// ---------------------------------------------------------------------------

fn open_path(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    if points.is_empty() {
        return path;
    }
    path.move_to(points[0]);
    for &p in &points[1..] {
        path.line_to(p);
    }
    path
}

fn path_el_finite(el: &buzz_geom::PathEl) -> bool {
    use buzz_geom::PathEl::*;
    let finite = |p: Point| p.x.is_finite() && p.y.is_finite();
    match el {
        MoveTo(p) | LineTo(p) => finite(*p),
        QuadTo(p1, p2) => finite(*p1) && finite(*p2),
        CurveTo(p1, p2, p3) => finite(*p1) && finite(*p2) && finite(*p3),
        ClosePath => true,
    }
}

/// Fit one run (an open polyline, possibly a whole corner-less loop) with
/// `kurbo::simplify`, falling back to Douglas–Peucker at `0.2` px as
/// `LineTo`s when the fit overshoots, grows the point count, or is not
/// finite.
fn fit_run(points: &[Point], t: &InkTuning) -> BezPath {
    if points.len() < 2 {
        return open_path(points);
    }
    let input = open_path(points);
    let input_len = input.elements().len();
    // kurbo's default `angle_thresh` (1e-3) splits the fit at every vertex
    // of a dense polyline (points `1/U` px apart turn more than that), so
    // nothing gets fit. Corners were already split off in step D; only a
    // turn sharper than the step-13 corner angle may split a run here.
    let options = kurbo::simplify::SimplifyOptions::default().angle_thresh(t.corner_deg.to_radians().tan());
    let fitted = kurbo::simplify::simplify_bezpath(input.iter(), t.fit_tolerance_px, &options);
    // The fit must also stay where the points are: kurbo's area-and-moment
    // fit can hand back a finite cubic that swings hundreds of pixels off
    // (seen as a stray line through Raju's face, path bbox 360 px below the
    // picture). The CURVE — sampled along every segment, since the control
    // points of a wide arc legitimately sit outside it — must stay within the
    // run's own bounds plus a small margin.
    let bounds = points
        .iter()
        .fold(buzz_geom::Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |r, p| {
            buzz_geom::Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y))
        })
        .inflate(FIT_MARGIN_PX, FIT_MARGIN_PX);
    let in_bounds = fitted.segments().all(|seg| {
        use kurbo::ParamCurve;
        [0.0, 0.2, 0.4, 0.5, 0.6, 0.8, 1.0].iter().all(|&t| bounds.contains(seg.eval(t)))
    });
    let ok = fitted.elements().len() <= input_len
        && fitted.elements().iter().all(path_el_finite)
        && in_bounds;
    if ok {
        fitted
    } else {
        let simplified = douglas_peucker(points, 0.2);
        open_path(&simplified)
    }
}

/// Douglas–Peucker on `Point`s, `epsilon` in pixels. `bucket::douglas_peucker`
/// is private to that module, so this is its own tiny copy.
pub(crate) fn douglas_peucker(points: &[Point], epsilon: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    let mut index = 0;
    let mut max = 0.0;
    for (i, &p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let d = perpendicular_distance(p, first, last);
        if d > max {
            max = d;
            index = i;
        }
    }
    if max > epsilon {
        let mut left = douglas_peucker(&points[..=index], epsilon);
        let right = douglas_peucker(&points[index..], epsilon);
        left.pop();
        left.extend(right);
        left
    } else {
        vec![first, last]
    }
}

fn perpendicular_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len = ab.hypot();
    if len < f64::EPSILON {
        return (p - a).hypot();
    }
    ((p.x - a.x) * ab.y - (p.y - a.y) * ab.x).abs() / len
}

/// One loop, fully smoothed (D) and fit (E) into a closed subpath —
/// `MoveTo`, `LineTo`/curve elements, `ClosePath`.
pub(crate) fn loop_path(raw: &[(usize, usize)], u: usize, line_smooth: f64, t: &InkTuning) -> BezPath {
    let points: Vec<Point> = raw
        .iter()
        .map(|&(x, y)| Point::new(x as f64 / u as f64, y as f64 / u as f64))
        .collect();

    let sigma0 = 0.5 * u as f64;
    let p1 = gaussian_smooth_circular(&points, sigma0);
    let n = p1.len();
    let k = ((0.75 * u as f64).round() as usize).max(1);
    let corners = detect_corners(&p1, k, t.corner_deg);
    let corner_idx: Vec<usize> = (0..n).filter(|&i| corners[i]).collect();
    let sigma1 = (line_smooth * 2.0 * u as f64).max(0.0);

    let mut path = BezPath::new();
    if corner_idx.is_empty() {
        // Step 15: rotate the seam to the straightest part of the loop.
        let angles = turning_angles(&p1, k);
        let least = angles
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut rotated = p1;
        rotated.rotate_left(least);
        let smoothed = gaussian_smooth_circular(&rotated, sigma1);
        let run_path = fit_run(&smoothed, t);
        for el in run_path.elements() {
            path.push(*el);
        }
        path.close_path();
    } else {
        let mut started = false;
        for w in 0..corner_idx.len() {
            let start = corner_idx[w];
            let end = corner_idx[(w + 1) % corner_idx.len()];
            let run = ring_slice(&p1, start, end);
            if run.len() < 2 {
                continue;
            }
            let smoothed = gaussian_smooth_open(&run, sigma1);
            let run_path = fit_run(&smoothed, t);
            let elements = run_path.elements();
            if elements.is_empty() {
                continue;
            }
            if !started {
                path.push(elements[0]);
                started = true;
            }
            for el in &elements[1..] {
                path.push(*el);
            }
        }
        if started {
            path.close_path();
        }
    }
    if n >= 400 && std::env::var_os("BUZZ_INK_DEBUG_DIR").is_some() {
        let area = path.area();
        eprintln!(
            "  loop_path: n {n} corners {} -> elements {} area {:.0}",
            corner_idx.len(),
            path.elements().len(),
            area
        );
    }
    path
}

// ---------------------------------------------------------------------------
// F. Colour and output
// ---------------------------------------------------------------------------

/// Per component (`1..=n`): the per-channel median RGB of every native pixel
/// with `F ≥ 0.9` whose upsampled cell centre labels to it; under 8 samples,
/// the darkest pixel under the component instead.
fn component_colours(
    w: usize,
    h: usize,
    u: usize,
    uw: usize,
    uh: usize,
    f: &[f32],
    labels: &[u32],
    pixels: &[u8],
    n: u32,
) -> Vec<[u8; 3]> {
    let mut samples: Vec<Vec<[u8; 3]>> = vec![Vec::new(); n as usize + 1];
    let mut darkest: Vec<Option<(f64, [u8; 3])>> = vec![None; n as usize + 1];
    for y in 0..h {
        let cy = (u * y + u / 2).min(uh.saturating_sub(1));
        for x in 0..w {
            let i = y * w + x;
            let cx = (u * x + u / 2).min(uw.saturating_sub(1));
            let lbl = labels[cy * uw + cx];
            if lbl == 0 {
                continue;
            }
            let px = &pixels[i * 4..i * 4 + 4];
            let rgb = [px[0], px[1], px[2]];
            let luma = 0.299 * px[0] as f64 + 0.587 * px[1] as f64 + 0.114 * px[2] as f64;
            let c = lbl as usize;
            if darkest[c].is_none_or(|(dl, _)| luma < dl) {
                darkest[c] = Some((luma, rgb));
            }
            if f[i] >= 0.9 {
                samples[c].push(rgb);
            }
        }
    }
    (1..=n as usize)
        .map(|c| {
            if samples[c].len() >= 8 {
                median_rgb(&mut samples[c])
            } else {
                darkest[c].map(|(_, rgb)| rgb).unwrap_or([56, 43, 35])
            }
        })
        .collect()
}

fn median_rgb(samples: &mut [[u8; 3]]) -> [u8; 3] {
    let mut r: Vec<u8> = samples.iter().map(|p| p[0]).collect();
    let mut g: Vec<u8> = samples.iter().map(|p| p[1]).collect();
    let mut b: Vec<u8> = samples.iter().map(|p| p[2]).collect();
    [median_u8(&mut r), median_u8(&mut g), median_u8(&mut b)]
}

fn median_u8(v: &mut [u8]) -> u8 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// **The native-resolution ink mask after the component rules**: `F ≥
/// ink_level`, minus every 8-connected component that [`trace_ink`] would
/// drop as a colour mark ([`is_colour_mark`]: a red bangle, a dark-red plaid
/// dash) or a faint mark ([`is_faint_mark`]). For the colour stage, which
/// must not treat those as ink — under-painting them would make them vanish
/// from the trace altogether, since the ink stage drops them too.
pub(crate) fn native_ink_mask(analysis: &InkAnalysis, pixels: &[u8], w: usize, h: usize, t: &InkTuning) -> Vec<bool> {
    let mut mask: Vec<bool> = analysis.f.iter().map(|&v| v >= t.ink_level).collect();
    let (labels, n) = label_components(&mask, w, h);
    if n == 0 {
        return mask;
    }
    let colours = component_colours(w, h, 1, w, h, &analysis.f, &labels, pixels, n);
    let stats = component_stats(w, h, 1, w, h, analysis, &labels, n);
    let drop: Vec<bool> = (0..n as usize)
        .map(|c| is_colour_mark(colours[c], t) || is_faint_mark(&stats[c], 1, t))
        .collect();
    for (m, &l) in mask.iter_mut().zip(&labels) {
        if l != 0 && drop[l as usize - 1] {
            *m = false;
        }
    }
    mask
}

/// **Trace `pixels` into ink shapes.** Dispatched from [`crate::trace::trace`]
/// for `TraceMode::Ink`. See the module docs for the pipeline.
pub(crate) fn trace_ink(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> TraceReport {
    trace_ink_with(width, height, pixels, options, None)
}

/// [`trace_ink`] with `exclude`: native pixels (`width × height`, `true` =
/// leave out) whose coverage is zeroed before the trace. The colour stage
/// passes the CORES of the ink masses — the pixels its under-painting did not
/// reach, which are their own colour regions there — so a shoe or a belt is
/// drawn as its own fill with an ink rim round it, not as a solid black mass.
/// Cores small enough to be a pupil or a button are refilled by the ink's
/// hole rules and stay solid.
pub(crate) fn trace_ink_with(
    width: u32,
    height: u32,
    pixels: &[u8],
    options: &TraceOptions,
    exclude: Option<&[bool]>,
) -> TraceReport {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return TraceReport {
            message: "There is nothing in that picture to trace".into(),
            ..Default::default()
        };
    }

    let t = &options.tuning.ink;

    // A.
    let mut analysis = ink_analysis(width, height, pixels, t);
    if let Some(exclude) = exclude {
        for (v, &x) in analysis.f.iter_mut().zip(exclude) {
            if x {
                *v = 0.0;
            }
        }
    }
    let f = &analysis.f;

    // B.
    let u = upsample_for(w, h, t);
    let (uw, uh) = (w * u, h * u);
    let mut mask = threshold_mask(w, h, f, u, t.ink_level);
    crate::trace_lines::clean(&mut mask, uw, uh, (options.speckle * u * u).max(1));
    {
        let small_max = (MAX_HOLE_PX2 * (u * u) as f64) as usize;
        let dark_max = (t.dark_hole_max_px2 * (u * u) as f64) as usize;
        let luma = &analysis.luma;
        fill_holes(&mut mask, uw, uh, |cells| {
            if cells.len() <= small_max {
                return true;
            }
            if cells.len() > dark_max {
                return false;
            }
            let mut ls: Vec<f64> = cells.iter().map(|&i| luma[(i / uw) / u * w + (i % uw) / u]).collect();
            median_f64(&mut ls).is_some_and(|m| m < t.dark_hole_max_luma)
        });
    }
    mask = apply_bold(mask, uw, uh, options.ink_bold, u);

    // C.
    let (labels, max_label) = label_components(&mask, uw, uh);
    let mut grid = Grid::new(uw, uh);
    for y in 0..uh {
        for x in 0..uw {
            if mask[y * uw + x] {
                grid.set(x, y);
            }
        }
    }
    let raw_loops = trace_contours(&grid);

    // Debug: `BUZZ_INK_DEBUG_DIR=<dir>` writes the U× mask as `ink_mask.png`
    // and prints every loop's length, signed area and component.
    if let Ok(dir) = std::env::var("BUZZ_INK_DEBUG_DIR") {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::create_dir_all(dir);
        let buf: Vec<u8> = mask.iter().map(|&m| if m { 0u8 } else { 255u8 }).collect();
        let _ = image::save_buffer(
            dir.join("ink_mask.png"),
            &buf,
            uw as u32,
            uh as u32,
            image::ExtendedColorType::L8,
        );
        let comp1 = raw_loops.iter().filter(|lp| lp.len() >= 8 && loop_component(lp, &labels, uw, uh) == Some(1)).count();
        eprintln!(
            "ink debug: U={u} mask {uw}x{uh}, {} components, {} loops ({comp1} of them on component 1, >= 8 points)",
            max_label,
            raw_loops.len()
        );
        for (i, lp) in raw_loops.iter().enumerate() {
            let mut a = 0.0f64;
            for j in 0..lp.len() {
                let p = lp[j];
                let q = lp[(j + 1) % lp.len()];
                a += p.0 as f64 * q.1 as f64 - q.0 as f64 * p.1 as f64;
            }
            let comp = loop_component(lp, &labels, uw, uh);
            if lp.len() >= 400 || comp.is_none() {
                eprintln!(
                    "  loop {i}: len {} area {:.0} start ({}, {}) comp {:?}",
                    lp.len(),
                    a / 2.0 / (u * u) as f64,
                    lp[0].0,
                    lp[0].1,
                    comp
                );
            }
        }
    }

    let mut per_component: Vec<Vec<Vec<(usize, usize)>>> = vec![Vec::new(); max_label as usize + 1];
    let mut dropped = 0usize;
    for loop_pts in raw_loops {
        if loop_pts.len() < 8 {
            dropped += 1;
            continue;
        }
        let Some(comp) = loop_component(&loop_pts, &labels, uw, uh) else {
            dropped += 1;
            continue;
        };
        per_component[comp as usize].push(loop_pts);
    }

    // D + E.
    let mut built: Vec<(usize, f64, BezPath)> = Vec::new();
    for comp in 1..=max_label {
        let loops = &per_component[comp as usize];
        if loops.is_empty() {
            continue;
        }
        let mut path = BezPath::new();
        for raw in loops {
            let lp = loop_path(raw, u, options.line_smooth, t);
            for el in lp.elements() {
                path.push(*el);
            }
        }
        if path.elements().is_empty() {
            continue;
        }
        let area = path.area().abs();
        built.push((comp as usize, area, path));
    }

    // F.
    let colours = component_colours(w, h, u, uw, uh, f, &labels, pixels, max_label);
    let stats = component_stats(w, h, u, uw, uh, &analysis, &labels, max_label);
    let mut shapes: Vec<(f64, ShapeData)> = Vec::new();
    for (comp, area, path) in built {
        let rgb = colours.get(comp - 1).copied().unwrap_or([56, 43, 35]);
        if is_colour_mark(rgb, t) || stats.get(comp - 1).is_some_and(|s| is_faint_mark(s, u, t)) {
            dropped += 1;
            continue;
        }
        let colour = options.line_colour.unwrap_or_else(|| Color::from_rgb8(rgb[0], rgb[1], rgb[2]));
        shapes.push((area, ShapeData::filled(path, colour)));
    }
    shapes.sort_by(|a, b| b.0.total_cmp(&a.0));
    let shapes: Vec<ShapeData> = shapes.into_iter().map(|(_, s)| s).collect();
    let n = shapes.len();

    TraceReport {
        shapes,
        regions: n,
        specks: dropped,
        strokes: 0,
        fills: 0,
        message: format!("Traced {n} ink shapes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::TraceMode;

    /// Composite an annulus (centre `(cx, cy)`, `outer`/`inner` radius) and a
    /// straight line onto a white canvas, 4× supersampled coverage per
    /// pixel so the mask's own sub-pixel edge is not the thing under test.
    fn ring_and_line_png() -> (u32, u32, Vec<u8>) {
        const W: usize = 200;
        const H: usize = 200;
        const SS: i32 = 4;
        let (cx, cy) = (100.0f64, 100.0f64);
        let (outer, inner) = (40.0f64, 36.5f64);
        let ink = [56.0f64, 43.0, 35.0];
        let mut pixels = vec![255u8; W * H * 4];
        for py in 0..H {
            for px in 0..W {
                let mut hits = 0i32;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let x = px as f64 + (sx as f64 + 0.5) / SS as f64;
                        let y = py as f64 + (sy as f64 + 0.5) / SS as f64;
                        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                        let in_ring = d <= outer && d >= inner;
                        let in_line = y >= 180.0 && y <= 185.0 && x >= 20.0 && x <= 180.0;
                        if in_ring || in_line {
                            hits += 1;
                        }
                    }
                }
                let coverage = hits as f64 / (SS * SS) as f64;
                let i = (py * W + px) * 4;
                // Blend ink over white by `coverage`, straight alpha stays opaque.
                for c in 0..3 {
                    pixels[i + c] = (ink[c] * coverage + 255.0 * (1.0 - coverage)).round() as u8;
                }
                pixels[i + 3] = 255;
            }
        }
        (W as u32, H as u32, pixels)
    }

    #[test]
    fn ring_and_line_trace_cleanly() {
        let (w, h, pixels) = ring_and_line_png();
        let mut options = TraceOptions::ink();
        options.ink_bold = 0.0;
        let report = trace_ink(w, h, &pixels, &options);
        assert_eq!(report.shapes.len(), 2, "{:?}", report.message);

        // The ring is the larger shape (area close to the annulus); the line
        // the smaller one.
        let ring = report
            .shapes
            .iter()
            .max_by(|a, b| a.path.area().abs().total_cmp(&b.path.area().abs()))
            .unwrap();

        let move_tos = ring
            .path
            .elements()
            .iter()
            .filter(|e| matches!(e, buzz_geom::PathEl::MoveTo(_)))
            .count();
        assert_eq!(move_tos, 2, "the ring should trace as two loops (outer + inner)");

        let expected_area = std::f64::consts::PI * (40.0 * 40.0 - 36.5 * 36.5);
        let area = ring.path.area().abs();
        let err = (area - expected_area).abs() / expected_area;
        assert!(err < 0.06, "area {area} vs expected {expected_area} (err {err})");

        assert!(
            ring.path.elements().len() <= 40,
            "{} elements",
            ring.path.elements().len()
        );
        for el in ring.path.elements() {
            assert!(path_el_finite(el), "{el:?}");
        }

        let [r, g, b, _] = ring.fill.as_ref().unwrap().color().to_rgba8().to_u8_array();
        assert!((r as i32 - 56).abs() <= 6, "r={r}");
        assert!((g as i32 - 43).abs() <= 6, "g={g}");
        assert!((b as i32 - 35).abs() <= 6, "b={b}");
    }

    #[test]
    fn ink_bold_grows_the_ring() {
        let (w, h, pixels) = ring_and_line_png();
        let mut thin = TraceOptions::ink();
        thin.ink_bold = 0.0;
        let thin_report = trace_ink(w, h, &pixels, &thin);
        let mut bold = TraceOptions::ink();
        bold.ink_bold = 0.5;
        let bold_report = trace_ink(w, h, &pixels, &bold);

        let ring_area = |report: &TraceReport| -> f64 {
            report
                .shapes
                .iter()
                .map(|s| s.path.area().abs())
                .fold(0.0, f64::max)
        };
        let thin_area = ring_area(&thin_report);
        let bold_area = ring_area(&bold_report);
        let expected_growth = 0.5 * (2.0 * std::f64::consts::PI * 40.0 + 2.0 * std::f64::consts::PI * 36.5);
        let growth = bold_area - thin_area;
        let err = (growth - expected_growth).abs() / expected_growth;
        assert!(err < 0.20, "growth {growth} vs expected {expected_growth} (err {err})");
    }

    /// The ring's `|area|` and how many loops (`MoveTo`s) it has: the shape
    /// over the picture's centre (the line below it is nearly as large).
    fn ring_area_and_loops(report: &TraceReport) -> (f64, usize) {
        let ring = report
            .shapes
            .iter()
            .find(|s| s.path.bounding_box().contains(Point::new(100.0, 100.0)))
            .expect("a ring");
        let loops = ring.path.elements().iter().filter(|e| matches!(e, buzz_geom::PathEl::MoveTo(_))).count();
        (ring.path.area().abs(), loops)
    }

    #[test]
    fn negative_bold_thins_the_ring() {
        let (w, h, pixels) = ring_and_line_png();
        let mut options = TraceOptions::ink();
        options.ink_bold = 0.0;
        let (source_area, _) = ring_area_and_loops(&trace_ink(w, h, &pixels, &options));

        // −0.5 px per side: the 3.5 px ring (14 cells at U 4) is deep enough
        // to erode, and loses about half a pixel along both its edges.
        options.ink_bold = -0.5;
        let (thin_area, thin_loops) = ring_area_and_loops(&trace_ink(w, h, &pixels, &options));
        assert_eq!(thin_loops, 2, "the thinned ring must still be an outer and an inner loop");
        let expected_loss = 0.5 * (2.0 * std::f64::consts::PI * 40.0 + 2.0 * std::f64::consts::PI * 36.5);
        let loss = source_area - thin_area;
        let err = (loss - expected_loss).abs() / expected_loss;
        assert!(err < 0.25, "loss {loss} vs expected {expected_loss} (err {err})");

        // −1 px per side would take 2 of its 3.5 px: the ring is too thin to
        // survive that, so it is left exactly as it was — it never vanishes.
        options.ink_bold = -1.0;
        let (kept_area, kept_loops) = ring_area_and_loops(&trace_ink(w, h, &pixels, &options));
        assert_eq!(kept_loops, 2);
        assert!((kept_area - source_area).abs() < 1e-6, "{kept_area} vs {source_area}");
    }

    /// An anti-aliased ring `outer − inner` px wide on white, 200 × 200.
    fn ring_png(outer: f64, inner: f64) -> (u32, u32, Vec<u8>) {
        const W: usize = 200;
        const SS: i32 = 4;
        let ink = [56.0f64, 43.0, 35.0];
        let mut pixels = vec![255u8; W * W * 4];
        for py in 0..W {
            for px in 0..W {
                let mut hits = 0i32;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let x = px as f64 + (sx as f64 + 0.5) / SS as f64;
                        let y = py as f64 + (sy as f64 + 0.5) / SS as f64;
                        let d = ((x - 100.0).powi(2) + (y - 100.0).powi(2)).sqrt();
                        if d <= outer && d >= inner {
                            hits += 1;
                        }
                    }
                }
                let coverage = hits as f64 / (SS * SS) as f64;
                let i = (py * W + px) * 4;
                for c in 0..3 {
                    pixels[i + c] = (ink[c] * coverage + 255.0 * (1.0 - coverage)).round() as u8;
                }
                pixels[i + 3] = 255;
            }
        }
        (W as u32, W as u32, pixels)
    }

    #[test]
    fn bold_zero_keeps_the_source_width() {
        let (w, h, pixels) = ring_png(40.0, 37.0);
        let t = InkTuning::default();
        let (wn, hn) = (w as usize, h as usize);
        let u = upsample_for(wn, hn, &t);
        assert_eq!(u, 4);
        let (uw, uh) = (wn * u, hn * u);
        let analysis = ink_analysis(w, h, &pixels, &t);
        let mask = apply_bold(threshold_mask(wn, hn, &analysis.f, u, t.ink_level), uw, uh, 0.0, u);

        // The ridge of the distance transform: cells at least as deep as
        // their four neighbours. Twice their median depth is the line width.
        let depth = crate::trace_lines::edt(&mask, uw, uh);
        let mut ridge: Vec<f64> = Vec::new();
        for y in 1..uh - 1 {
            for x in 1..uw - 1 {
                let i = y * uw + x;
                if depth[i] > 0.0 && [i - 1, i + 1, i - uw, i + uw].iter().all(|&j| depth[j] <= depth[i]) {
                    ridge.push(depth[i]);
                }
            }
        }
        let width = 2.0 * median_f64(&mut ridge).expect("a ridge") / u as f64;
        assert!((width - 3.0).abs() <= 0.5, "ridge width {width} px");
    }

    #[test]
    fn smoke_pujari_front() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../buzz-ai/tests/data/pujariFront_0.png");
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return, // the fixture is optional for this crate's own tests
        };
        let img = ::image::load_from_memory(&bytes).expect("decode pujariFront_0.png").to_rgba8();
        let (w, h) = img.dimensions();
        let pixels = img.into_raw();

        let started = std::time::Instant::now();
        let mut options = TraceOptions::ink();
        options.mode = TraceMode::Ink;
        let report = trace_ink(w, h, &pixels, &options);
        let elapsed = started.elapsed();

        assert!(!report.shapes.is_empty(), "{}", report.message);
        for shape in &report.shapes {
            for el in shape.path.elements() {
                assert!(path_el_finite(el), "{el:?}");
            }
        }
        assert!(elapsed.as_secs_f64() < 2.0, "{elapsed:?}");
    }
}
