//! **Colour fills under the ink** — `TraceMode::InkAndColour`, stage A of
//! section I4 of `docs/INK_TRACE_PLAN.md`.
//!
//! [`crate::trace_ink`] traces the dark ink of a character sheet as smooth
//! filled shapes; this traces the *colour* between the lines as flat regions
//! that run under that ink, and lays the ink on top.
//!
//! # The pipeline
//!
//! 1. **Ink.** `trace_ink`'s coverage field `F`; the native ink mask is
//!    `F ≥ ink_level`, grown by `ColourTuning::ink_grow_px` so the anti-aliased rim counts
//!    as ink too.
//! 2. **Under-paint.** Every ink pixel takes the colour of the nearest non-ink
//!    opaque pixel ([`under_paint`]), so the fills run *under* the lines.
//!    **2b. Colour lines** ([`colour_line_coverage`]): thin strokes that differ
//!    from their local background colour (a plaid thread) are found on that
//!    picture and under-painted too, from the original, so the fill under a
//!    thread is the plain cloth; they are traced as their own shapes
//!    ([`colour_line_shapes`]).
//! 3. **Flatten.** Two passes of a bilateral filter in CIELAB
//!    ([`bilateral`]): airbrushed shading inside a surface collapses to one
//!    tone while every real colour edge stays put.
//! 4. **Palettes per surface** ([`surface_labels`]). A *surface* is a
//!    4-connected component of opaque non-ink pixels ([`surfaces`]); each one
//!    of at least [`SURFACE_MIN_PX2`] gets its own palette
//!    ([`surface_palette`]: supported farthest-point seeding, Lloyd, centres
//!    closer than `ColourTuning::palette_merge_de` merged), a smaller uniform one a single
//!    colour, the rest the global k-means palette ([`palette`]). Non-ink
//!    pixels take their surface palette's nearest centre; ink pixels take
//!    their under-paint donor's label.
//! 5. **Regions.** 8-connected components per label, then small regions
//!    (except high-contrast ones) and soft (gradient) boundaries merged away
//!    ([`merge_regions`]).
//! 6. **Sub-pixel label map at `U×`** ([`cell_labels`]).
//! 7. **Vectorisation** — stage A's simple stand-in: each region's own `U×`
//!    mask, grown by [`REGION_GROW_CELLS`] so neighbours overlap and leave no
//!    crack, traced and smoothed exactly as the ink is ([`region_path`]). (The
//!    watertight shared-edge vectorisation is stage B.)
//! 8. **Colour.** The per-channel median of each region's interior in the
//!    under-painted original ([`region_colours`]); the background region is
//!    not emitted when `drop_background`.
//! 9. **Output.** Fills, largest first, then the colour lines (largest
//!    first), then `trace_ink`'s shapes on top.
//!
//! Everything here is deterministic: no `HashMap` iteration order and no
//! parallel floating-point reductions reach the output.

use crate::bucket::{Grid, trace_contours};
use crate::object::ShapeData;
use crate::trace::{TraceOptions, TraceReport, srgb_to_lab};
use crate::trace_ink::{
    MAX_HOLE_PX2, SEED_LEVEL, fill_holes, ink_analysis, ink_window, label_components, loop_component, loop_path,
    native_ink_mask, require_core, threshold_mask, trace_ink_with, upsample_for,
};
use crate::trace_lines::{edt, local_background};
use crate::trace_tuning::{ColourTuning, InkTuning};
use buzz_geom::BezPath;
use peniko::Color;
use rayon::prelude::*;

// ---------------------------------------------------------------------------
// Constants (all from the I4 design)
// ---------------------------------------------------------------------------

/// A pixel is opaque when its alpha is above this; transparent pixels are
/// never painted, never donate a colour and are never traced.
const OPAQUE_MIN_ALPHA: u8 = 128;
// How deep under-painting reaches into the ink is
// `ColourTuning::under_paint_max_layers`.
/// An ink mass's core is left out of the ink (drawn as its own fill with an ink
/// rim) only from this many px²: a shoe (≈ 2000), a belt, the hair. A brow bar
/// (50 × 12 px → a 264 px² core), a pupil or a button stays one solid ink mass.
const MASS_CORE_MIN_PX2: usize = 600;
// The ink mask's growth is `ColourTuning::ink_grow_px`.
// Bilateral flatten (step 3): `ColourTuning::bilateral_passes` passes...
/// ...over a `(2·r + 1)²` window (7×7)...
const BILATERAL_RADIUS: usize = 3;
/// ...with a Gaussian spatial weight of this σ (px)...
const BILATERAL_SPATIAL_SIGMA: f32 = 3.0;
// ...and a Gaussian range weight of `ColourTuning::bilateral_range_sigma` (ΔE).
/// k-means seeding (step 4): the first seed is the mean of the most frequent
/// Lab bin, bins this many ΔE on a side.
const SEED_BIN_DE: f32 = 4.0;
/// k-means works on every this-many-th opaque pixel, for speed.
const KMEANS_SAMPLE_STRIDE: usize = 4;
/// Lloyd iterations (stops early when the assignment no longer changes).
const KMEANS_ITERATIONS: usize = 20;
// Palette centres closer than `ColourTuning::palette_merge_de` (ΔE) are merged.
/// **Palettes per surface** (A3): a surface — a 4-connected component of
/// opaque non-ink pixels ([`surfaces`]) — of at least this many px² gets its
/// own palette ([`surface_palette`]); a global palette mislabels a broad
/// ΔE-12 plaid band inconsistently along its length (Guddu's shirt).
const SURFACE_MIN_PX2: usize = 400;
/// A surface's farthest-point seeding stops once every pixel lies within this
/// ΔE of a centre...
const SURFACE_COVER_DE: f32 = 8.0;
// ...or once it has `ColourTuning::surface_max_k` centres.
// **Textured surfaces** are those whose median source gradient is at least
// `ColourTuning::textured_median_gradient`; their palette has at most
// `ColourTuning::surface_max_k_textured` centres.
/// On a textured surface a region smaller than this share of its surface is
/// a patch of the wash and merges into its most similar neighbour...
const TEXTURED_MIN_SHARE: f64 = 0.02;
/// ...unless it differs from every neighbour by at least this (ΔE): a real
/// mark. Also the soft-merge colour allowance between two textured regions.
const TEXTURED_MERGE_MAX_DE: f32 = 30.0;
/// A seeding candidate becomes a centre only when at least
/// `max(SURFACE_MIN_SUPPORT, surface px / SURFACE_SUPPORT_SHARE)` pixels of
/// the surface lie within [`SURFACE_SUPPORT_DE`] of it — a lone stray pixel
/// never gets a colour of its own...
const SURFACE_MIN_SUPPORT: usize = 24;
/// ...the share of the surface (one pixel in this many) that floor grows by...
const SURFACE_SUPPORT_SHARE: usize = 1000;
/// ...and the support radius (ΔE).
const SURFACE_SUPPORT_DE: f32 = 4.0;
/// Lloyd iterations over a surface's pixels after its seeding (stops early
/// when the assignment no longer changes, which gives the same centres).
const SURFACE_LLOYD_ITERATIONS: usize = 10;
/// A surface under [`SURFACE_MIN_PX2`] takes one colour, its per-channel Lab
/// median, when this percentile of its pixels' ΔE to that median...
const SMALL_SURFACE_PERCENTILE: f64 = 0.95;
/// ...is under this (ΔE); otherwise the global palette labels it.
const SMALL_SURFACE_MAX_DE: f32 = 8.0;
// A region smaller than `ColourTuning::min_region_px2` is merged into its most
// similar 8-neighbour...
/// ...unless it is fully enclosed by ink (an eye glint, a tooth), where the
/// floor is this.
const MIN_ENCLOSED_REGION_PX2: usize = 4;
/// **High-contrast small regions survive** (A3): a region under
/// `ColourTuning::min_region_px2` of at least this many px² whose mean Lab is
/// at least `ColourTuning::contrast_region_min_de` from every 8-neighbour
/// region (a green leaf on a yellow sari, a glint) is never merged for being
/// small.
const MIN_CONTRAST_REGION_PX2: usize = 4;
// **Soft-boundary merge** (anti-banding): adjacent regions whose mean colours
// are closer than `ColourTuning::soft_merge_max_de` (ΔE)...
/// ...and whose shared boundary has a mean source gradient below this (ΔE per
/// px, on the under-painted original) are one surface with a gradient painted
/// into it.
const SOFT_MERGE_MAX_GRADIENT: f32 = 4.0;
/// ...or below this multiple of the two regions' own mean interior gradient
/// (their texture): on a painterly wash the boundary is no sharper than the
/// grain, on a cel edge it is many times sharper. See `merge_regions`.
const SOFT_MERGE_TEXTURE_RATIO: f32 = 1.5;
// ...but never above `ColourTuning::soft_merge_gradient_cap` (ΔE per px).
/// **Thin stripes** (a plaid thread, a sari pleat): a region whose distance to
/// its own edge never exceeds this (px) — under 3 px wide everywhere — and
/// whose area is at least [`THIN_STRIPE_MIN_PX2`] is never merged for being
/// small.
const THIN_STRIPE_MAX_DEPTH_PX: f64 = 1.5;
/// ...a thin stripe must also cover at least this many px² (a stripe at least
/// ~2 × 10 px): every blob under ~3 px² has a depth ≤ 1.5 too, and without this
/// floor every such speck was exempt from the small-region merge (Woman1 came
/// out as 3857 fills).
const THIN_STRIPE_MIN_PX2: usize = 20;
/// Merge rounds are repeated until nothing merges, at most this many times.
const MERGE_ROUNDS: usize = 6;
/// A region's colour is the median of its interior (eroded by 1 px) when it
/// has at least this many interior pixels, else of all its pixels.
const MIN_INTERIOR_PX: usize = 8;
/// Each region's `U×` mask is grown by this many cells before it is traced,
/// so neighbouring fills overlap by `1/U` px and leave no hairline crack.
const REGION_GROW_CELLS: usize = 1;
/// A segmenter-fringe pixel (A2) has ink within this distance (px): the skirt
/// of a line, not the edge of a line-less fill (a hair top, a white collar
/// touching the cut-out edge), which stays a fill.
const FRINGE_INK_PX: f64 = 3.0;
/// The distance transforms of A1 / A2 pad the picture with this many px of
/// "no target", so the picture's border does not count as exterior, ink or
/// interior (`edt` alone treats it as a target): exact up to this distance.
const EDGE_EDT_PAD: usize = 32;
/// Traced loops with fewer points than this are dropped (as the ink does).
const MIN_LOOP_POINTS: usize = 8;
/// **Colour lines** (step 2b): a pixel's colour-line coverage is its ΔE to its
/// local background colour over this, clamped to `0..=1` — ΔE 10 is the 0.5
/// crossing, ΔE 14 the 0.7 core.
const LINE_DE: f32 = 16.0;
/// A connected colour line whose deepest pixel lies further than this (px)
/// from its edge is not a line but a differently coloured *surface* near its
/// edge (an eye white, a tooth, a bindi): it is dropped whole and stays a
/// region.
const COLOUR_LINE_MAX_HALF_WIDTH_PX: f64 = 2.5;
/// The local background colour is a median over 8-bit-quantised Lab: `L`
/// (0–100) is scaled by this into `0..=255`...
const LAB_L_QUANT: f32 = 2.55;
/// ...and `a`, `b` are offset by this.
const LAB_AB_OFFSET: f32 = 128.0;
/// The median: the 50th percentile of each channel's window histogram.
const MEDIAN_PERCENTILE: f64 = 0.5;
/// A colour line's colour is the median of its core pixels (`Fc ≥
/// SEED_LEVEL`) when it has at least this many, else of all its pixels.
const MIN_LINE_CORE_PX: usize = 8;
/// "No label": a transparent pixel or cell.
const NONE: u32 = u32::MAX;

type Lab = [f32; 3];

fn lab_of(rgb: [u8; 3]) -> Lab {
    let l = srgb_to_lab(rgb);
    [l[0] as f32, l[1] as f32, l[2] as f32]
}

fn dist2(a: Lab, b: Lab) -> f32 {
    let (d0, d1, d2) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    d0 * d0 + d1 * d1 + d2 * d2
}

/// The 8 neighbours of `(x, y)` inside a `w`×`h` picture, as indices.
fn neighbours8(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    (-1i64..=1).flat_map(move |dy| {
        (-1i64..=1).filter_map(move |dx| {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            ((dx != 0 || dy != 0) && nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h)
                .then(|| ny as usize * w + nx as usize)
        })
    })
}

// ---------------------------------------------------------------------------
// 1. Ink
// ---------------------------------------------------------------------------

/// `F ≥ ink_level`, grown by `c.ink_grow_px` (square), opaque pixels only.
fn grown_ink(core: &[bool], opaque: &[bool], w: usize, h: usize, c: &ColourTuning) -> Vec<bool> {
    let r = c.ink_grow_px as i64;
    let mut out = vec![false; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, cell) in row.iter_mut().enumerate() {
            if !opaque[y * w + x] {
                continue;
            }
            *cell = (-r..=r).any(|dy| {
                (-r..=r).any(|dx| {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && core[ny as usize * w + nx as usize]
                })
            });
        }
    });
    out
}

// ---------------------------------------------------------------------------
// 1b. The figure's edge (A1 / A2)
// ---------------------------------------------------------------------------

/// Euclidean distance (px) from every pixel to the nearest pixel where
/// `target` holds; the outside of the picture is no target (exact up to
/// [`EDGE_EDT_PAD`] px past the border).
fn distance_to(target: &[bool], w: usize, h: usize) -> Vec<f64> {
    let p = EDGE_EDT_PAD;
    let (pw, ph) = (w + 2 * p, h + 2 * p);
    let mut mask = vec![true; pw * ph];
    for y in 0..h {
        for x in 0..w {
            mask[(y + p) * pw + x + p] = !target[y * w + x];
        }
    }
    let d = edt(&mask, pw, ph);
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        let row = (y + p) * pw + p;
        out[y * w..(y + 1) * w].copy_from_slice(&d[row..row + w]);
    }
    out
}

/// **A2: the segmenter's anti-alias fringe** — opaque non-ink pixels
/// reachable from a transparent pixel through opaque non-ink pixels (4-
/// connected, so an ink line is a wall) within `fringe_px` of the
/// transparency, with ink within [`FRINGE_INK_PX`]. The caller treats them as
/// transparent. Never a pixel whose loss would leave an ink component with no
/// opaque non-ink neighbour (its donor). Empty when `fringe_px` is 0 or the
/// picture has no transparent pixel.
fn segmenter_fringe(opaque: &[bool], ink: &[bool], w: usize, h: usize, fringe_px: f32) -> Vec<bool> {
    let mut fringe = vec![false; w * h];
    if !(fringe_px > 0.0) || opaque.iter().all(|&o| o) {
        return fringe;
    }
    let exterior: Vec<bool> = opaque.iter().map(|&o| !o).collect();
    let d_ext = distance_to(&exterior, w, h);
    let d_ink = distance_to(ink, w, h);
    let reach = fringe_px as f64;
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h).filter(|&i| exterior[i]).collect();
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let n4 = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
        for j in n4.into_iter().flatten() {
            if !seen[j] && opaque[j] && !ink[j] && d_ext[j] <= reach {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    for i in 0..w * h {
        fringe[i] = seen[i] && d_ink[i] <= FRINGE_INK_PX;
    }
    if fringe.iter().any(|&f| f) {
        let (labels, n) = label_components(ink, w, h);
        let mut kept = vec![false; n as usize + 1];
        for i in 0..w * h {
            let l = labels[i] as usize;
            if l != 0 && !kept[l] && neighbours8(i % w, i / w, w, h).any(|j| opaque[j] && !ink[j] && !fringe[j]) {
                kept[l] = true;
            }
        }
        for i in 0..w * h {
            let l = labels[i] as usize;
            if l != 0 && !kept[l] {
                for j in neighbours8(i % w, i / w, w, h) {
                    fringe[j] = false;
                }
            }
        }
    }
    fringe
}

/// A6a: a patch touching the outside is at most this far (px, pixel centre to
/// pixel centre) from an exterior pixel.
const POCKET_TOUCH_PX: f64 = 1.5;

/// A6a: the border ring of an opaque sheet counts as its background colour
/// only when at least this share of it is opaque…
const SHEET_BORDER_OPAQUE_SHARE: f64 = 0.9;
/// … and this share of it lies within [`SHEET_BORDER_DE`] of its median.
const SHEET_BORDER_UNIFORM_SHARE: f64 = 0.8;
const SHEET_BORDER_DE: f64 = 6.0;

/// **The sheet's background colour** (A6a): the per-channel median of the
/// picture's 1 px border ring, when that ring is opaque
/// ([`SHEET_BORDER_OPAQUE_SHARE`]) and plain ([`SHEET_BORDER_UNIFORM_SHARE`]
/// within ΔE [`SHEET_BORDER_DE`] of the median): a character sheet's paper.
/// `None` for a cut-out (transparent border) or a photograph.
pub fn sheet_background(width: u32, height: u32, pixels: &[u8]) -> Option<[u8; 3]> {
    let (w, h) = (width as usize, height as usize);
    if w < 2 || h < 2 || pixels.len() < w * h * 4 {
        return None;
    }
    let mut ring: Vec<usize> = Vec::with_capacity(2 * (w + h));
    ring.extend((0..w).map(|x| x));
    ring.extend((0..w).map(|x| (h - 1) * w + x));
    ring.extend((1..h - 1).map(|y| y * w));
    ring.extend((1..h - 1).map(|y| y * w + w - 1));
    let opaque: Vec<[u8; 3]> = ring
        .iter()
        .filter(|&&i| pixels[i * 4 + 3] > OPAQUE_MIN_ALPHA)
        .map(|&i| [pixels[i * 4], pixels[i * 4 + 1], pixels[i * 4 + 2]])
        .collect();
    if (opaque.len() as f64) < SHEET_BORDER_OPAQUE_SHARE * ring.len() as f64 {
        return None;
    }
    let median = |c: usize| {
        let mut v: Vec<u8> = opaque.iter().map(|p| p[c]).collect();
        v.sort_unstable();
        v[v.len() / 2]
    };
    let bg = [median(0), median(1), median(2)];
    let bg_lab = srgb_to_lab(bg);
    let plain = opaque.iter().filter(|&&p| crate::trace::delta_e(srgb_to_lab(p), bg_lab) <= SHEET_BORDER_DE).count();
    ((plain as f64) >= SHEET_BORDER_UNIFORM_SHARE * opaque.len() as f64).then_some(bg)
}

/// **A6a: background pockets** — the sheet's paper showing through inside
/// the figure (between hair curls, between body-hair strands), as a mask of
/// pixels to treat as transparent.
///
/// A *light patch* is a 4-connected set of opaque pixels within
/// 2 × `background_pocket_de` of `background` (so a vest's highlight joins the
/// rest of the vest rather than standing alone). It is a pocket when its mean
/// colour is within `background_pocket_de` of `background`, it holds at most
/// `background_pocket_max_px2` pixels, no pixel of it is `protect`ed, it does
/// not touch the picture's border (that is the sheet itself) and either it
/// lies wholly within `background_pocket_reach_px` of the exterior or it
/// touches the exterior ([`POCKET_TOUCH_PX`]). The exterior is the
/// transparent pixels plus the light patches that touch the picture's border.
/// All `false` when `background_pocket_de` is 0.
pub fn background_pockets(
    width: u32,
    height: u32,
    pixels: &[u8],
    background: [u8; 3],
    protect: Option<&[bool]>,
    ct: &ColourTuning,
) -> Vec<bool> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![false; w * h];
    let de = ct.background_pocket_de as f64;
    if !(de > 0.0) || w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return out;
    }
    let bg = srgb_to_lab(background);
    let opaque: Vec<bool> = (0..w * h).map(|i| pixels[i * 4 + 3] > OPAQUE_MIN_ALPHA).collect();
    let lab: Vec<[f64; 3]> = (0..w * h)
        .into_par_iter()
        .map(|i| srgb_to_lab([pixels[i * 4], pixels[i * 4 + 1], pixels[i * 4 + 2]]))
        .collect();
    let light: Vec<bool> =
        (0..w * h).map(|i| opaque[i] && crate::trace::delta_e(lab[i], bg) <= 2.0 * de).collect();
    // 4-connected light patches.
    let mut label = vec![NONE; w * h];
    let mut patches: Vec<Vec<usize>> = Vec::new();
    for s in 0..w * h {
        if !light[s] || label[s] != NONE {
            continue;
        }
        let id = patches.len() as u32;
        let mut px = vec![s];
        label[s] = id;
        let mut k = 0;
        while k < px.len() {
            let i = px[k];
            k += 1;
            let (x, y) = (i % w, i / w);
            let n4 = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for j in n4.into_iter().flatten() {
                if light[j] && label[j] == NONE {
                    label[j] = id;
                    px.push(j);
                }
            }
        }
        patches.push(px);
    }
    let on_border = |i: usize| {
        let (x, y) = (i % w, i / w);
        x == 0 || y == 0 || x + 1 == w || y + 1 == h
    };
    let sheet: Vec<bool> = patches.iter().map(|px| px.iter().any(|&i| on_border(i))).collect();
    let exterior: Vec<bool> = (0..w * h).map(|i| !opaque[i] || (label[i] != NONE && sheet[label[i] as usize])).collect();
    if !exterior.iter().any(|&e| e) {
        return out;
    }
    let d_ext = distance_to(&exterior, w, h);
    let reach = ct.background_pocket_reach_px as f64;
    let max_px = ct.background_pocket_max_px2;
    // `BUZZ_TRACE_POCKETS_AT=x,y`: why the light patch at that pixel is or is not a pocket.
    if let Some((x, y)) = std::env::var("BUZZ_TRACE_POCKETS_AT").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse::<usize>().ok()?, b.trim().parse::<usize>().ok()?))
    }) {
        let i = y.min(h - 1) * w + x.min(w - 1);
        if label[i] == NONE {
            eprintln!("pockets at ({x}, {y}): not light (opaque {}, dE {:.1})", opaque[i], crate::trace::delta_e(lab[i], bg));
        } else {
            let px = &patches[label[i] as usize];
            let mut mean = [0.0f64; 3];
            let (mut d_min, mut d_max) = (f64::INFINITY, 0.0f64);
            for &j in px {
                for c in 0..3 {
                    mean[c] += lab[j][c] / px.len() as f64;
                }
                d_min = d_min.min(d_ext[j]);
                d_max = d_max.max(d_ext[j]);
            }
            eprintln!(
                "pockets at ({x}, {y}): patch of {} px, sheet {}, protected {}, mean dE {:.1}, d_ext {d_min:.1}..{d_max:.1}",
                px.len(),
                sheet[label[i] as usize],
                protect.is_some_and(|m| px.iter().any(|&j| m[j])),
                crate::trace::delta_e(mean, bg)
            );
        }
    }
    for (p, px) in patches.iter().enumerate() {
        if sheet[p] || px.len() > max_px {
            continue;
        }
        if protect.is_some_and(|m| px.iter().any(|&i| m[i])) {
            continue;
        }
        let mut mean = [0.0f64; 3];
        let (mut d_min, mut d_max) = (f64::INFINITY, 0.0f64);
        for &i in px {
            for c in 0..3 {
                mean[c] += lab[i][c];
            }
            d_min = d_min.min(d_ext[i]);
            d_max = d_max.max(d_ext[i]);
        }
        let n = px.len() as f64;
        let mean = [mean[0] / n, mean[1] / n, mean[2] / n];
        if crate::trace::delta_e(mean, bg) > de {
            continue;
        }
        if d_max <= reach || d_min <= POCKET_TOUCH_PX {
            if std::env::var_os("BUZZ_TRACE_POCKETS_LOG").is_some_and(|v| v == "2") {
                let (x, y) = (px[0] % w, px[0] / w);
                eprintln!(
                    "pocket at ({x}, {y}): {} px, mean dE {:.1}, d_ext {d_min:.1}..{d_max:.1}",
                    px.len(),
                    crate::trace::delta_e(mean, bg)
                );
            }
            for &i in px {
                out[i] = true;
            }
        }
    }
    out
}

/// A6d: an edge pixel's ink is the darkest pixel within this distance (px,
/// Chebyshev) …
const BLEND_RIM_INK_PX: usize = 3;
/// … when that is at most this light (L*): ink, not a dark fill's shading.
const BLEND_RIM_INK_MAX_L: f64 = 45.0;

/// **Where `c` lies on the blend from `ink` to `paper`** (Lab): `(t, d)`, `t`
/// the share of the way towards the paper (0 = the ink, 1 = the paper,
/// clamped) and `d` the ΔE from `c` to that point of the segment. The source's
/// anti-aliasing of a line drawn on paper lies on it (A6d).
pub fn ink_paper_blend(c: [f64; 3], ink: [f64; 3], paper: [f64; 3]) -> (f64, f64) {
    let seg = [paper[0] - ink[0], paper[1] - ink[1], paper[2] - ink[2]];
    let len2 = seg[0] * seg[0] + seg[1] * seg[1] + seg[2] * seg[2];
    let t = if len2 > 1e-9 {
        (((c[0] - ink[0]) * seg[0] + (c[1] - ink[1]) * seg[1] + (c[2] - ink[2]) * seg[2]) / len2).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let p = [ink[0] + t * seg[0], ink[1] + t * seg[1], ink[2] + t * seg[2]];
    (t, crate::trace::delta_e(c, p))
}

/// **A6d: ink–paper rims** — the source's anti-aliasing of an outline
/// against the paper at the figure's outer edge (light-grey dashes along hair
/// tips), as a mask of pixels to treat as transparent. A rim pixel is opaque,
/// not `protect`ed, within `blend_rim_px` of the exterior (`!opaque`) and
/// reachable from it through rim pixels (4-connected), and its colour lies on
/// the blend from its ink to `background` ([`ink_paper_blend`]: at least
/// `blend_rim_min_t` towards the paper, at most `blend_rim_de` off the
/// segment), its ink being the darkest pixel within [`BLEND_RIM_INK_PX`] when
/// that is at most [`BLEND_RIM_INK_MAX_L`]. All `false` when `blend_rim_px`
/// is 0.
pub fn blend_rims(
    width: u32,
    height: u32,
    pixels: &[u8],
    opaque: &[bool],
    background: [u8; 3],
    protect: Option<&[bool]>,
    ct: &ColourTuning,
) -> Vec<bool> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![false; w * h];
    let reach = ct.blend_rim_px as f64;
    if !(reach > 0.0) || w == 0 || h == 0 || pixels.len() < w * h * 4 || opaque.len() != w * h {
        return out;
    }
    let exterior: Vec<bool> = opaque.iter().map(|&o| !o).collect();
    if !exterior.iter().any(|&e| e) {
        return out;
    }
    let d_ext = distance_to(&exterior, w, h);
    let near: Vec<usize> = (0..w * h).filter(|&i| opaque[i] && d_ext[i] <= reach).collect();
    if near.is_empty() {
        return out;
    }
    let paper = srgb_to_lab(background);
    let rgb = |i: usize| [pixels[i * 4], pixels[i * 4 + 1], pixels[i * 4 + 2]];
    let r = BLEND_RIM_INK_PX as i64;
    let (de, min_t) = (ct.blend_rim_de as f64, ct.blend_rim_min_t as f64);
    let rim_colour: Vec<(usize, bool)> = near
        .par_iter()
        .map(|&i| {
            if protect.is_some_and(|m| m[i]) {
                return (i, false);
            }
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let mut darkest: Option<[f64; 3]> = None;
            for dy in -r..=r {
                for dx in -r..=r {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !opaque[j] {
                        continue;
                    }
                    let l = srgb_to_lab(rgb(j));
                    if darkest.is_none_or(|d| l[0] < d[0]) {
                        darkest = Some(l);
                    }
                }
            }
            let Some(ink) = darkest.filter(|d| d[0] <= BLEND_RIM_INK_MAX_L) else { return (i, false) };
            let (t, d) = ink_paper_blend(srgb_to_lab(rgb(i)), ink, paper);
            (i, t >= min_t && d <= de)
        })
        .collect();
    let mut cand = vec![false; w * h];
    for (i, c) in rim_colour {
        cand[i] = c;
    }
    let mut stack: Vec<usize> = (0..w * h).filter(|&i| exterior[i]).collect();
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let n4 = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
        for j in n4.into_iter().flatten() {
            if cand[j] && !out[j] {
                out[j] = true;
                stack.push(j);
            }
        }
    }
    out
}

/// **A1: fills stop at the outer ink's centreline.** `exterior` is
/// transparent, or a non-ink pixel of the dropped background region (ink is
/// never exterior, whichever region donated it); an ink pixel that is not a
/// mass core keeps its region iff `d_int <= reach * (d_int + d_ext)` (`d_ext`:
/// distance to the exterior, `d_int`: to the nearest opaque non-ink pixel that
/// is not exterior), evaluated per `U×` cell near the split (see
/// [`ExteriorGate::keeps`]).
struct ExteriorGate {
    exterior: Vec<bool>,
    gated: Vec<bool>,
    d_int: Vec<f64>,
    d_ext: Vec<f64>,
    reach: f64,
    w: usize,
    h: usize,
}

impl ExteriorGate {
    /// `None` when `reach` is 1 (the old behaviour, exactly) or nothing is
    /// exterior.
    #[allow(clippy::too_many_arguments)]
    fn new(
        opaque: &[bool],
        ink: &[bool],
        mass_core: &[bool],
        region: &[u32],
        skip: Option<u32>,
        w: usize,
        h: usize,
        reach: f32,
    ) -> Option<Self> {
        let reach = reach.clamp(0.0, 1.0) as f64;
        if !(reach < 1.0) {
            return None;
        }
        let exterior: Vec<bool> = (0..w * h).map(|i| !opaque[i] || (Some(region[i]) == skip && !ink[i])).collect();
        if !exterior.iter().any(|&e| e) {
            return None;
        }
        let interior: Vec<bool> = (0..w * h).map(|i| opaque[i] && !ink[i] && !exterior[i]).collect();
        let d_ext = distance_to(&exterior, w, h);
        let d_int = distance_to(&interior, w, h);
        let gated: Vec<bool> = (0..w * h).map(|i| ink[i] && !mass_core[i]).collect();
        Some(Self { exterior, gated, d_int, d_ext, reach, w, h })
    }

    /// Whether the fill may cover the cell centred at native `(sx, sy)` over
    /// ink pixel `i`: the inequality on `i`'s own distances, or — within 1 px
    /// of the split — on the two distances bilinearly interpolated at the
    /// cell centre, so the fill's edge is smooth at `U×`.
    fn keeps(&self, i: usize, sx: f64, sy: f64) -> bool {
        if !self.gated[i] {
            return true;
        }
        let margin = |di: f64, de: f64| di - self.reach * (di + de);
        let at = margin(self.d_int[i], self.d_ext[i]);
        if at.abs() > 1.0 {
            return at <= 0.0;
        }
        let di = sample_bilinear(&self.d_int, self.w, self.h, sx, sy);
        let de = sample_bilinear(&self.d_ext, self.w, self.h, sx, sy);
        margin(di, de) <= 0.0
    }
}

/// `v` bilinearly sampled at native `(x, y)`, clamped into the picture (the
/// sampling of [`sample_flat`], over every pixel).
fn sample_bilinear(v: &[f64], w: usize, h: usize, x: f64, y: f64) -> f64 {
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f64, y - y0 as f64);
    v[y0 * w + x0] * (1.0 - tx) * (1.0 - ty)
        + v[y0 * w + x1] * tx * (1.0 - ty)
        + v[y1 * w + x0] * (1.0 - tx) * ty
        + v[y1 * w + x1] * tx * ty
}

// ---------------------------------------------------------------------------
// 2. Under-paint
// ---------------------------------------------------------------------------

/// The picture's RGB with every opaque ink pixel repainted by the colour of
/// the nearest opaque non-ink pixel: a multi-source breadth-first search in
/// (8-connected) distance layers from the non-ink pixels bordering the ink,
/// each ink pixel taking the colour of the first seed to reach it, passed on
/// unchanged (never a blend). Transparent pixels never donate or carry; an ink
/// pixel no seed reaches keeps its own colour.
///
/// Also returns each painted pixel's **donor** — the index of the non-ink
/// pixel whose colour it took ([`NONE`] where nothing was painted), so an ink
/// pixel can join its donor's surface and label (A3).
fn under_paint(
    pixels: &[u8],
    ink: &[bool],
    opaque: &[bool],
    w: usize,
    h: usize,
    c: &ColourTuning,
) -> (Vec<[u8; 3]>, Vec<u32>) {
    let mut out: Vec<[u8; 3]> = (0..w * h).map(|i| [pixels[i * 4], pixels[i * 4 + 1], pixels[i * 4 + 2]]).collect();
    let mut donor = vec![NONE; w * h];
    // Only opaque ink pixels are to be reached.
    let mut seen: Vec<bool> = (0..w * h).map(|i| !(opaque[i] && ink[i])).collect();
    let mut frontier: Vec<(usize, u32)> = (0..w * h)
        .filter(|&i| opaque[i] && !ink[i] && neighbours8(i % w, i / w, w, h).any(|j| !seen[j]))
        .map(|i| (i, i as u32))
        .collect();
    let mut layers = 0usize;
    while !frontier.is_empty() && layers < c.under_paint_max_layers {
        layers += 1;
        let mut next: Vec<(usize, u32)> = Vec::new();
        for &(i, d) in &frontier {
            let rgb = out[i];
            for j in neighbours8(i % w, i / w, w, h) {
                if !seen[j] {
                    seen[j] = true;
                    out[j] = rgb;
                    donor[j] = d;
                    next.push((j, d));
                }
            }
        }
        frontier = next;
    }
    (out, donor)
}

// ---------------------------------------------------------------------------
// 2b. Colour lines
// ---------------------------------------------------------------------------

/// One Lab channel (`c` = 0 for `L`, 1 / 2 for `a` / `b`) quantised to
/// `0..=255` (see [`LAB_L_QUANT`], [`LAB_AB_OFFSET`]).
fn quantise_lab(c: usize, v: f32) -> u8 {
    let q = if c == 0 { v * LAB_L_QUANT } else { v + LAB_AB_OFFSET };
    q.round().clamp(0.0, 255.0) as u8
}

/// **Local background colour** per pixel: the per-channel median of the
/// 8-bit-quantised Lab of the opaque pixels in a `W×W` box
/// ([`ink_window`]), one sliding-histogram pass per channel
/// ([`local_background`], rows in parallel), back in Lab.
fn local_background_lab(lab: &[Lab], opaque: &[bool], w: usize, h: usize) -> Vec<Lab> {
    let window = ink_window(w, h);
    let channels: Vec<Vec<u8>> = (0..3usize)
        .into_par_iter()
        .map(|c| {
            let bins: Vec<u8> = lab.par_iter().map(|p| quantise_lab(c, p[c])).collect();
            local_background(w, h, &bins, opaque, window, MEDIAN_PERCENTILE)
        })
        .collect();
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            [
                channels[0][i] as f32 / LAB_L_QUANT,
                channels[1][i] as f32 - LAB_AB_OFFSET,
                channels[2][i] as f32 - LAB_AB_OFFSET,
            ]
        })
        .collect()
}

/// **Colour-line coverage `Fc`** per native pixel, over the ink-under-painted
/// picture's `lab`: `clamp(ΔE(lab, local background) / LINE_DE, 0, 1)`, `0` on
/// ink and transparent pixels; then the ink's hysteresis ([`require_core`]: a
/// pixel only counts with a `Fc ≥ SEED_LEVEL` pixel in its 3×3 neighbourhood);
/// then every 8-connected component of `Fc ≥ ink_level` whose deepest pixel is
/// further than [`COLOUR_LINE_MAX_HALF_WIDTH_PX`] from its edge, or which is
/// under `speckle` px², is zeroed whole. What is left at `≥ ink_level` are the
/// colour-line pixels.
fn colour_line_coverage(
    lab: &[Lab],
    ink: &[bool],
    opaque: &[bool],
    w: usize,
    h: usize,
    speckle: usize,
    t: &InkTuning,
) -> Vec<f32> {
    let bg = local_background_lab(lab, opaque, w, h);
    let fc: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            if !opaque[i] || ink[i] {
                0.0
            } else {
                (dist2(lab[i], bg[i]).sqrt() / LINE_DE).clamp(0.0, 1.0)
            }
        })
        .collect();
    drop(bg);
    thin_line_coverage(fc, w, h, speckle, t)
}

/// **Residual pattern lines**: what the fills get wrong, as lines. Per opaque,
/// non-ink pixel of a region: its ΔE in the ink-under-painted original (`lab`)
/// to its region's fill colour, over `LINE_DE`; zero within one pixel of
/// another region or of ink (the anti-aliased seam between two fills is not a
/// line). Then the same hysteresis, thinness and speck rules as
/// [`colour_line_coverage`]. A plaid thread or a sari pleat the local-median
/// detector missed shows up here against the flat base it was merged into;
/// a shadow or a wash patch is thick and stays merged.
fn residual_line_coverage(
    lab: &[Lab],
    region: &[u32],
    fill_lab: &[Lab],
    ink: &[bool],
    opaque: &[bool],
    w: usize,
    h: usize,
    speckle: usize,
    t: &InkTuning,
    c: &ColourTuning,
) -> Vec<f32> {
    let fr: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let r = region[i];
            if !opaque[i] || ink[i] || r == NONE {
                return 0.0;
            }
            let (x, y) = (i % w, i / w);
            let seam = neighbours8(x, y, w, h).any(|j| ink[j] || region[j] != r);
            if seam {
                return 0.0;
            }
            (dist2(lab[i], fill_lab[r as usize]).sqrt() / c.residual_line_de).clamp(0.0, 1.0)
        })
        .collect();
    thin_parts_coverage(fr, w, h, speckle, t)
}

// The residual's `LINE_DE` is `ColourTuning::residual_line_de`.

/// Hysteresis ([`require_core`]), then only the THIN PARTS of the `≥ ink_level`
/// mask are kept: its morphological opening by a disc of radius
/// [`COLOUR_LINE_MAX_HALF_WIDTH_PX`] (every pixel within that radius of a pixel
/// at least that deep inside the mask), grown by one more pixel, is removed.
/// In a plaid the thin threads and the broad bands are one connected web, so
/// judging whole components (as [`thin_line_coverage`] does) threw the threads
/// away with the bands; here the bands go and the threads stay. Components
/// under `speckle` px² are dropped.
fn thin_parts_coverage(mut fc: Vec<f32>, w: usize, h: usize, speckle: usize, t: &InkTuning) -> Vec<f32> {
    require_core(&mut fc, w, h);
    let mask: Vec<bool> = fc.iter().map(|&v| v >= t.ink_level).collect();
    if !mask.iter().any(|&m| m) {
        return fc;
    }
    let depth = edt(&mask, w, h);
    let r = COLOUR_LINE_MAX_HALF_WIDTH_PX;
    // `edt` measures, inside its `true` set, the distance to the nearest
    // `false` pixel: on the complement of the seeds that is the distance to
    // the nearest seed.
    let not_seed: Vec<bool> = depth.iter().map(|&d| d <= r).collect();
    let to_seed = edt(&not_seed, w, h);
    let thin: Vec<bool> = (0..w * h).map(|i| mask[i] && to_seed[i] > r + 1.0).collect();
    let (labels, n) = label_components(&thin, w, h);
    let mut area = vec![0usize; n as usize + 1];
    for &l in &labels {
        area[l as usize] += 1;
    }
    for i in 0..w * h {
        let l = labels[i] as usize;
        if l == 0 || area[l] < speckle {
            fc[i] = 0.0;
        }
    }
    fc
}

/// Hysteresis ([`require_core`]), then every 8-connected component of
/// `≥ ink_level` that is deeper than [`COLOUR_LINE_MAX_HALF_WIDTH_PX`] or
/// smaller than `speckle` px² is zeroed whole.
fn thin_line_coverage(mut fc: Vec<f32>, w: usize, h: usize, speckle: usize, t: &InkTuning) -> Vec<f32> {
    require_core(&mut fc, w, h);
    let mask: Vec<bool> = fc.iter().map(|&v| v >= t.ink_level).collect();
    let (labels, n) = label_components(&mask, w, h);
    if n == 0 {
        return fc;
    }
    let depth = edt(&mask, w, h);
    let mut deepest = vec![0.0f64; n as usize + 1];
    let mut area = vec![0usize; n as usize + 1];
    for i in 0..w * h {
        let l = labels[i] as usize;
        if l != 0 {
            area[l] += 1;
            if depth[i] > deepest[l] {
                deepest[l] = depth[i];
            }
        }
    }
    for i in 0..w * h {
        let l = labels[i] as usize;
        if l != 0 && (deepest[l] > COLOUR_LINE_MAX_HALF_WIDTH_PX || area[l] < speckle) {
            fc[i] = 0.0;
        }
    }
    fc
}

/// **The colour lines as shapes**, vectorised exactly as the ink is: `fc`
/// bilinearly upsampled to `U×` and thresholded at `ink_level`
/// ([`threshold_mask`]), holes of at most [`MAX_HOLE_PX2`] filled, no bold,
/// [`trace_contours`] + [`loop_path`]; one filled shape per 8-connected `U×`
/// component. Its colour is the per-channel median, in `painted` (the
/// ink-under-painted original), of the native pixels with `fc ≥ SEED_LEVEL`
/// whose cell centre lies in the component; with fewer than
/// [`MIN_LINE_CORE_PX`] of those, of the native pixels under all its cells.
/// Largest `|area|` first (component order on a tie).
fn colour_line_shapes(
    fc: &[f32],
    painted: &[[u8; 3]],
    w: usize,
    h: usize,
    line_smooth: f64,
    t: &InkTuning,
) -> Vec<ShapeData> {
    if !fc.iter().any(|&v| v >= t.ink_level) {
        return Vec::new();
    }
    let u = upsample_for(w, h, t);
    let (uw, uh) = (w * u, h * u);
    let mut mask = threshold_mask(w, h, fc, u, t.ink_level);
    let small_max = (MAX_HOLE_PX2 * (u * u) as f64) as usize;
    fill_holes(&mut mask, uw, uh, |cells| cells.len() <= small_max);
    let (labels, n) = label_components(&mask, uw, uh);
    if n == 0 {
        return Vec::new();
    }
    let n = n as usize;

    let mut grid = Grid::new(uw, uh);
    for (i, &m) in mask.iter().enumerate() {
        if m {
            grid.set(i % uw, i / uw);
        }
    }
    let mut per_component: Vec<Vec<Vec<(usize, usize)>>> = vec![Vec::new(); n + 1];
    for lp in trace_contours(&grid) {
        if lp.len() < MIN_LOOP_POINTS {
            continue;
        }
        if let Some(c) = loop_component(&lp, &labels, uw, uh) {
            per_component[c as usize].push(lp);
        }
    }

    // Colour samples: core pixels by their cell centre, all pixels by cell.
    let mut core: Vec<Vec<[u8; 3]>> = vec![Vec::new(); n + 1];
    let mut all: Vec<Vec<[u8; 3]>> = vec![Vec::new(); n + 1];
    for y in 0..h {
        let cy = (u * y + u / 2).min(uh - 1);
        for x in 0..w {
            let i = y * w + x;
            let cx = (u * x + u / 2).min(uw - 1);
            let l = labels[cy * uw + cx] as usize;
            if l != 0 && fc[i] >= SEED_LEVEL {
                core[l].push(painted[i]);
            }
        }
    }
    for (k, &l) in labels.iter().enumerate() {
        if l != 0 {
            let (cx, cy) = (k % uw, k / uw);
            all[l as usize].push(painted[(cy / u) * w + cx / u]);
        }
    }

    let mut shapes: Vec<(f64, usize, ShapeData)> = (1..=n)
        .into_par_iter()
        .filter_map(|c| {
            let loops = &per_component[c];
            if loops.is_empty() {
                return None;
            }
            let mut path = BezPath::new();
            for raw in loops {
                for el in loop_path(raw, u, line_smooth, t).elements() {
                    path.push(*el);
                }
            }
            if path.elements().is_empty() {
                return None;
            }
            let s = if core[c].len() >= MIN_LINE_CORE_PX { &core[c] } else { &all[c] };
            if s.is_empty() {
                return None;
            }
            let rgb = [median_channel(s, 0), median_channel(s, 1), median_channel(s, 2)];
            let area = buzz_geom::Shape::area(&path).abs();
            Some((area, c, ShapeData::filled(path, Color::from_rgb8(rgb[0], rgb[1], rgb[2]))))
        })
        .collect();
    shapes.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    shapes.into_iter().map(|(_, _, s)| s).collect()
}

// ---------------------------------------------------------------------------
// 3. Flatten
// ---------------------------------------------------------------------------

/// One bilateral pass in Lab over the opaque pixels (transparent pixels
/// neither contribute nor change), rows in parallel.
fn bilateral(src: &[Lab], opaque: &[bool], w: usize, h: usize, c: &ColourTuning) -> Vec<Lab> {
    let r = BILATERAL_RADIUS as i64;
    let side = (2 * r + 1) as usize;
    let spatial: Vec<f32> = (-r..=r)
        .flat_map(|dy| {
            (-r..=r).map(move |dx| {
                (-((dx * dx + dy * dy) as f32) / (2.0 * BILATERAL_SPATIAL_SIGMA * BILATERAL_SPATIAL_SIGMA)).exp()
            })
        })
        .collect();
    let inv_range = 1.0 / (2.0 * c.bilateral_range_sigma * c.bilateral_range_sigma);
    let mut out = src.to_vec();
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, cell) in row.iter_mut().enumerate() {
            let i = y * w + x;
            if !opaque[i] {
                continue;
            }
            let c = src[i];
            let mut acc = [0.0f32; 3];
            let mut wsum = 0.0f32;
            for dy in -r..=r {
                let ny = y as i64 + dy;
                if ny < 0 || ny as usize >= h {
                    continue;
                }
                for dx in -r..=r {
                    let nx = x as i64 + dx;
                    if nx < 0 || nx as usize >= w {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !opaque[j] {
                        continue;
                    }
                    let s = src[j];
                    let wt = spatial[(dy + r) as usize * side + (dx + r) as usize] * (-dist2(c, s) * inv_range).exp();
                    acc[0] += wt * s[0];
                    acc[1] += wt * s[1];
                    acc[2] += wt * s[2];
                    wsum += wt;
                }
            }
            if wsum > 0.0 {
                *cell = [acc[0] / wsum, acc[1] / wsum, acc[2] / wsum];
            }
        }
    });
    out
}

// ---------------------------------------------------------------------------
// 4. Palette
// ---------------------------------------------------------------------------

fn nearest(centres: &[Lab], c: Lab) -> u32 {
    let mut best = (f32::INFINITY, 0u32);
    for (k, &centre) in centres.iter().enumerate() {
        let d = dist2(c, centre);
        if d < best.0 {
            best = (d, k as u32);
        }
    }
    best.1
}

/// k-means in Lab over every [`KMEANS_SAMPLE_STRIDE`]-th opaque pixel of the
/// flattened picture: farthest-point seeding (first seed the most frequent Lab
/// bin's mean, then repeatedly the sample farthest from its nearest centre),
/// [`KMEANS_ITERATIONS`] Lloyd iterations, then the closest pair of centres
/// merged (weighted by membership) while closer than
/// `ColourTuning::palette_merge_de`. Empty when there is no opaque pixel.
fn palette(flat: &[Lab], opaque: &[bool], k: usize, ct: &ColourTuning) -> Vec<Lab> {
    let samples: Vec<Lab> = flat
        .iter()
        .zip(opaque)
        .filter(|(_, o)| **o)
        .map(|(c, _)| *c)
        .step_by(KMEANS_SAMPLE_STRIDE)
        .collect();
    if samples.is_empty() {
        return Vec::new();
    }
    let k = k.max(1);

    // Seed 1: the most frequent bin.
    let l_bins = (100.0 / SEED_BIN_DE).ceil() as usize + 1;
    let ab_bins = (256.0 / SEED_BIN_DE).ceil() as usize + 1;
    let bin_of = |c: Lab| -> usize {
        let l = ((c[0] / SEED_BIN_DE).floor().max(0.0) as usize).min(l_bins - 1);
        let a = (((c[1] + 128.0) / SEED_BIN_DE).floor().max(0.0) as usize).min(ab_bins - 1);
        let b = (((c[2] + 128.0) / SEED_BIN_DE).floor().max(0.0) as usize).min(ab_bins - 1);
        (l * ab_bins + a) * ab_bins + b
    };
    let mut counts = vec![0u32; l_bins * ab_bins * ab_bins];
    for &s in &samples {
        counts[bin_of(s)] += 1;
    }
    let mut top = 0usize;
    for (b, &n) in counts.iter().enumerate() {
        if n > counts[top] {
            top = b;
        }
    }
    let mut sum = [0.0f64; 3];
    let mut n = 0.0f64;
    for &s in samples.iter().filter(|&&s| bin_of(s) == top) {
        sum[0] += s[0] as f64;
        sum[1] += s[1] as f64;
        sum[2] += s[2] as f64;
        n += 1.0;
    }
    let mut centres: Vec<Lab> = vec![[(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32]];

    // Farthest-point seeds.
    let mut mind: Vec<f32> = samples.par_iter().map(|&s| dist2(s, centres[0])).collect();
    while centres.len() < k {
        let mut far = 0usize;
        for (i, &d) in mind.iter().enumerate() {
            if d > mind[far] {
                far = i;
            }
        }
        if mind[far] <= 0.0 {
            break;
        }
        let c = samples[far];
        centres.push(c);
        mind.par_iter_mut().zip(samples.par_iter()).for_each(|(m, &s)| {
            let d = dist2(s, c);
            if d < *m {
                *m = d;
            }
        });
    }

    // Lloyd: assignment in parallel, accumulation sequential (deterministic sums).
    let mut assign: Vec<u32> = Vec::new();
    let mut members = vec![0usize; centres.len()];
    for _ in 0..KMEANS_ITERATIONS {
        let next: Vec<u32> = samples.par_iter().map(|&s| nearest(&centres, s)).collect();
        let changed = next != assign;
        assign = next;
        let mut sums = vec![[0.0f64; 3]; centres.len()];
        members = vec![0usize; centres.len()];
        for (s, &a) in samples.iter().zip(&assign) {
            let a = a as usize;
            sums[a][0] += s[0] as f64;
            sums[a][1] += s[1] as f64;
            sums[a][2] += s[2] as f64;
            members[a] += 1;
        }
        for (c, centre) in centres.iter_mut().enumerate() {
            if members[c] > 0 {
                let m = members[c] as f64;
                *centre = [(sums[c][0] / m) as f32, (sums[c][1] / m) as f32, (sums[c][2] / m) as f32];
            }
        }
        if !changed {
            break;
        }
    }

    merge_centres(&centres, &members, ct)
}

/// Empty centres dropped, then the closest pair merged (weighted by
/// membership) while closer than `ColourTuning::palette_merge_de`.
fn merge_centres(centres: &[Lab], members: &[usize], ct: &ColourTuning) -> Vec<Lab> {
    let mut kept: Vec<(Lab, f64)> = centres
        .iter()
        .zip(members)
        .filter(|(_, m)| **m > 0)
        .map(|(c, m)| (*c, *m as f64))
        .collect();
    let bar = ct.palette_merge_de * ct.palette_merge_de;
    loop {
        let mut best: Option<(f32, usize, usize)> = None;
        for i in 0..kept.len() {
            for j in i + 1..kept.len() {
                let d = dist2(kept[i].0, kept[j].0);
                if d < bar && best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        let (ci, wi) = kept[i];
        let (cj, wj) = kept[j];
        let wsum = wi + wj;
        let mix = |a: f32, b: f32| ((a as f64 * wi + b as f64 * wj) / wsum) as f32;
        kept[i] = ([mix(ci[0], cj[0]), mix(ci[1], cj[1]), mix(ci[2], cj[2])], wsum);
        kept.remove(j);
    }
    kept.into_iter().map(|(c, _)| c).collect()
}

// ---------------------------------------------------------------------------
// 4. Palettes per surface (A3)
// ---------------------------------------------------------------------------

/// **Surfaces**: the 4-connected components of opaque non-ink pixels (the
/// grown ink splits a figure into shirt body, sleeves, face, trouser legs…),
/// in raster order of their first pixel, each as its pixel indices in raster
/// order.
fn surfaces(ink: &[bool], opaque: &[bool], w: usize, h: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; w * h];
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if seen[start] || !opaque[start] || ink[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut px: Vec<usize> = Vec::new();
        while let Some(at) = stack.pop() {
            px.push(at);
            let (x, y) = (at % w, at / w);
            let four = [
                (x > 0).then(|| at - 1),
                (x + 1 < w).then(|| at + 1),
                (y > 0).then(|| at - w),
                (y + 1 < h).then(|| at + w),
            ];
            for j in four.into_iter().flatten() {
                if !seen[j] && opaque[j] && !ink[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        px.sort_unstable();
        out.push(px);
    }
    out
}

/// The number of [`SEED_BIN_DE`] Lab bins along `L` and along `a` / `b`.
fn seed_bin_counts() -> (usize, usize) {
    ((100.0 / SEED_BIN_DE).ceil() as usize + 1, (256.0 / SEED_BIN_DE).ceil() as usize + 1)
}

/// The `(L, a, b)` bin coordinates of `c` (as [`palette`] bins its seeds).
fn seed_bin(c: Lab) -> [usize; 3] {
    let (l_bins, ab_bins) = seed_bin_counts();
    [
        ((c[0] / SEED_BIN_DE).floor().max(0.0) as usize).min(l_bins - 1),
        (((c[1] + 128.0) / SEED_BIN_DE).floor().max(0.0) as usize).min(ab_bins - 1),
        (((c[2] + 128.0) / SEED_BIN_DE).floor().max(0.0) as usize).min(ab_bins - 1),
    ]
}

fn seed_bin_index(b: [usize; 3]) -> usize {
    let (_, ab_bins) = seed_bin_counts();
    (b[0] * ab_bins + b[1]) * ab_bins + b[2]
}

/// `max(SURFACE_MIN_SUPPORT, px / SURFACE_SUPPORT_SHARE)`.
fn surface_min_support(px: usize) -> usize {
    SURFACE_MIN_SUPPORT.max(px / SURFACE_SUPPORT_SHARE)
}

/// A small surface's one colour: the per-channel median of `cols`, when the
/// [`SMALL_SURFACE_PERCENTILE`] (nearest rank) of their ΔE to it is under
/// [`SMALL_SURFACE_MAX_DE`]; `None` otherwise.
fn small_surface_colour(cols: &[Lab]) -> Option<Lab> {
    let n = cols.len();
    if n == 0 {
        return None;
    }
    let median = |c: usize| -> f32 {
        let mut v: Vec<f32> = cols.iter().map(|p| p[c]).collect();
        *v.select_nth_unstable_by(n / 2, |a, b| a.total_cmp(b)).1
    };
    let m = [median(0), median(1), median(2)];
    let mut d: Vec<f32> = cols.iter().map(|&c| dist2(c, m).sqrt()).collect();
    let rank = ((n as f64 * SMALL_SURFACE_PERCENTILE).ceil() as usize).clamp(1, n) - 1;
    let p = *d.select_nth_unstable_by(rank, |a, b| a.total_cmp(b)).1;
    (p < SMALL_SURFACE_MAX_DE).then_some(m)
}

/// **One surface's palette** over the flattened Lab of its pixels `px`.
///
/// With at least [`SURFACE_MIN_PX2`] pixels: greedy farthest-point seeding —
/// the first centre is the mean of the surface's most frequent
/// [`SEED_BIN_DE`] bin; then the pixel farthest from its nearest centre is
/// offered, and becomes a centre only when [`surface_min_support`] of the
/// surface's pixels lie within [`SURFACE_SUPPORT_DE`] of it (otherwise it is
/// never offered again and the next farthest is), until the farthest offered
/// pixel is under [`SURFACE_COVER_DE`] or there are `max_k` centres;
/// then [`SURFACE_LLOYD_ITERATIONS`] Lloyd iterations over all its pixels and
/// [`merge_centres`]. Smaller: one colour ([`small_surface_colour`]), or
/// `None` — the global palette labels it. Sequential, so deterministic.
fn surface_palette(px: &[usize], flat: &[Lab], max_k: usize, ct: &ColourTuning) -> Option<Vec<Lab>> {
    let cols: Vec<Lab> = px.iter().map(|&i| flat[i]).collect();
    let n = cols.len();
    if n < SURFACE_MIN_PX2 {
        return small_surface_colour(&cols).map(|c| vec![c]);
    }

    // The pixels sorted by bin (then index): the most frequent bin is the
    // longest run, and the pixels near a colour lie in the bins around it.
    let mut by_bin: Vec<(usize, u32)> = cols
        .iter()
        .enumerate()
        .map(|(k, &c)| (seed_bin_index(seed_bin(c)), k as u32))
        .collect();
    by_bin.sort_unstable();
    let (mut top, mut top_len) = (0usize, 0usize);
    let mut s = 0usize;
    while s < n {
        let mut e = s;
        while e < n && by_bin[e].0 == by_bin[s].0 {
            e += 1;
        }
        if e - s > top_len {
            (top, top_len) = (s, e - s);
        }
        s = e;
    }
    let mut sum = [0.0f64; 3];
    for &(_, k) in &by_bin[top..top + top_len] {
        let c = cols[k as usize];
        sum[0] += c[0] as f64;
        sum[1] += c[1] as f64;
        sum[2] += c[2] as f64;
    }
    let m = top_len as f64;
    let mut centres: Vec<Lab> = vec![[(sum[0] / m) as f32, (sum[1] / m) as f32, (sum[2] / m) as f32]];

    // Support: the surface's pixels within SURFACE_SUPPORT_DE of `c`, counted
    // over the bins that ball can reach, stopping once there are enough.
    let need = surface_min_support(n);
    let reach = (SURFACE_SUPPORT_DE / SEED_BIN_DE).ceil() as i64;
    let (l_bins, ab_bins) = seed_bin_counts();
    let support_bar = SURFACE_SUPPORT_DE * SURFACE_SUPPORT_DE;
    let supported = |c: Lab| -> bool {
        let b = seed_bin(c);
        let mut count = 0usize;
        for dl in -reach..=reach {
            let l = b[0] as i64 + dl;
            if l < 0 || l >= l_bins as i64 {
                continue;
            }
            for da in -reach..=reach {
                let a = b[1] as i64 + da;
                if a < 0 || a >= ab_bins as i64 {
                    continue;
                }
                // The b-bins of one (L, a) column are contiguous.
                let b0 = (b[2] as i64 - reach).max(0) as usize;
                let b1 = ((b[2] as i64 + reach) as usize).min(ab_bins - 1);
                let lo_key = seed_bin_index([l as usize, a as usize, b0]);
                let hi_key = seed_bin_index([l as usize, a as usize, b1]);
                let lo = by_bin.partition_point(|e| e.0 < lo_key);
                let hi = by_bin.partition_point(|e| e.0 <= hi_key);
                for e in &by_bin[lo..hi] {
                    if dist2(cols[e.1 as usize], c) <= support_bar {
                        count += 1;
                        if count >= need {
                            return true;
                        }
                    }
                }
            }
        }
        false
    };

    // Farthest-point seeding with the support test.
    let cover_bar = SURFACE_COVER_DE * SURFACE_COVER_DE;
    let mut mind: Vec<f32> = cols.iter().map(|&c| dist2(c, centres[0])).collect();
    let mut offered = vec![false; n];
    while centres.len() < max_k {
        let mut far: Vec<u32> = (0..n as u32)
            .filter(|&k| !offered[k as usize] && mind[k as usize] >= cover_bar)
            .collect();
        far.sort_by(|&a, &b| mind[b as usize].total_cmp(&mind[a as usize]).then(a.cmp(&b)));
        let mut pick: Option<usize> = None;
        for k in far {
            let k = k as usize;
            offered[k] = true;
            if supported(cols[k]) {
                pick = Some(k);
                break;
            }
        }
        let Some(k) = pick else { break };
        let c = cols[k];
        centres.push(c);
        for (d, &p) in mind.iter_mut().zip(&cols) {
            let e = dist2(p, c);
            if e < *d {
                *d = e;
            }
        }
    }

    // Lloyd over all the surface's pixels (sequential sums).
    let mut assign: Vec<u32> = Vec::new();
    let mut members = vec![0usize; centres.len()];
    for _ in 0..SURFACE_LLOYD_ITERATIONS {
        let next: Vec<u32> = cols.iter().map(|&c| nearest(&centres, c)).collect();
        let changed = next != assign;
        assign = next;
        let mut sums = vec![[0.0f64; 3]; centres.len()];
        members = vec![0usize; centres.len()];
        for (c, &a) in cols.iter().zip(&assign) {
            let a = a as usize;
            sums[a][0] += c[0] as f64;
            sums[a][1] += c[1] as f64;
            sums[a][2] += c[2] as f64;
            members[a] += 1;
        }
        for (k, centre) in centres.iter_mut().enumerate() {
            if members[k] > 0 {
                let m = members[k] as f64;
                *centre = [(sums[k][0] / m) as f32, (sums[k][1] / m) as f32, (sums[k][2] / m) as f32];
            }
        }
        if !changed {
            break;
        }
    }
    Some(merge_centres(&centres, &members, ct))
}

/// **Labels** (step 4): the palette — every surface's own centres in surface
/// order, then the global palette ([`palette`], `k` colours) when anything
/// needs it — and every opaque pixel's label, an index into it. A non-ink
/// pixel takes the nearest centre of its surface's palette
/// ([`surface_palette`]), or of the global palette when its surface has none;
/// an ink pixel takes its under-paint `donor`'s label, so it belongs to its
/// donor's surface, or the global palette's nearest when no donor reached it.
/// Labels never repeat across surfaces, so regions only cross surfaces
/// through ink pixels.
/// Per pixel and per surface, what the region merges need to treat a
/// painterly wash differently from flat colour (see
/// `ColourTuning::textured_median_gradient`).
struct SurfaceInfo {
    /// The surface each opaque pixel lies on (an ink pixel: its under-paint
    /// donor's), or [`NONE`].
    of: Vec<u32>,
    /// Per surface: textured (a wash) or not.
    textured: Vec<bool>,
    /// Per surface: its pixel count.
    area: Vec<usize>,
}

fn surface_labels(
    flat: &[Lab],
    grad: &[f32],
    opaque: &[bool],
    ink: &[bool],
    donor: &[u32],
    w: usize,
    h: usize,
    k: usize,
    ct: &ColourTuning,
) -> (Vec<Lab>, Vec<u32>, SurfaceInfo) {
    let surfaces = surfaces(ink, opaque, w, h);
    let textured: Vec<bool> = surfaces
        .par_iter()
        .map(|px| {
            if px.len() < SURFACE_MIN_PX2 {
                return false;
            }
            let mut g: Vec<f32> = px.iter().map(|&i| grad[i]).collect();
            let mid = g.len() / 2;
            *g.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1 >= ct.textured_median_gradient
        })
        .collect();
    let palettes: Vec<Option<(Vec<Lab>, Vec<u32>)>> = surfaces
        .par_iter()
        .zip(&textured)
        .map(|(px, &tex)| {
            let max_k = if tex { ct.surface_max_k_textured } else { ct.surface_max_k };
            surface_palette(px, flat, max_k, ct).map(|p| {
                let labels: Vec<u32> = px.iter().map(|&i| nearest(&p, flat[i])).collect();
                (p, labels)
            })
        })
        .collect();

    let mut centres: Vec<Lab> = Vec::new();
    let mut pal = vec![NONE; w * h];
    let mut of = vec![NONE; w * h];
    let mut global_px: Vec<usize> = Vec::new();
    for (s, (px, own)) in surfaces.iter().zip(palettes).enumerate() {
        for &i in px {
            of[i] = s as u32;
        }
        match own {
            Some((p, labels)) => {
                let offset = centres.len() as u32;
                for (&i, &l) in px.iter().zip(&labels) {
                    pal[i] = offset + l;
                }
                centres.extend(p);
            }
            None => global_px.extend_from_slice(px),
        }
    }
    let area: Vec<usize> = surfaces.iter().map(|px| px.len()).collect();
    global_px.extend((0..w * h).filter(|&i| opaque[i] && ink[i] && donor[i] == NONE));
    if !global_px.is_empty() {
        let global = palette(flat, opaque, k, ct);
        let offset = centres.len() as u32;
        for &i in &global_px {
            pal[i] = offset + nearest(&global, flat[i]);
        }
        centres.extend(global);
    }
    for i in 0..w * h {
        if opaque[i] && ink[i] && donor[i] != NONE {
            pal[i] = pal[donor[i] as usize];
            of[i] = of[donor[i] as usize];
        }
    }
    (centres, pal, SurfaceInfo { of, textured, area })
}

// ---------------------------------------------------------------------------
// 5. Regions
// ---------------------------------------------------------------------------

/// 8-connected components of equal label, numbered `0..n` in raster order of
/// their first pixel; [`NONE`] where the label is [`NONE`].
fn components_by_label(labels: &[u32], w: usize, h: usize) -> (Vec<u32>, usize) {
    let mut region = vec![NONE; w * h];
    let mut n = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if labels[start] == NONE || region[start] != NONE {
            continue;
        }
        let l = labels[start];
        region[start] = n;
        stack.push(start);
        while let Some(at) = stack.pop() {
            for j in neighbours8(at % w, at / w, w, h) {
                if labels[j] == l && region[j] == NONE {
                    region[j] = n;
                    stack.push(j);
                }
            }
        }
        n += 1;
    }
    (region, n as usize)
}

/// `|∇Lab|` per opaque pixel: the larger of the two axis central differences
/// (ΔE per px; one-sided at an edge or next to a transparent pixel).
fn gradient(lab: &[Lab], opaque: &[bool], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, g) in row.iter_mut().enumerate() {
            let i = y * w + x;
            if !opaque[i] {
                continue;
            }
            let axis = |a: Option<usize>, b: Option<usize>| -> f32 {
                let a = a.filter(|&j| opaque[j]);
                let b = b.filter(|&j| opaque[j]);
                match (a, b) {
                    (Some(a), Some(b)) => dist2(lab[a], lab[b]).sqrt() / 2.0,
                    (Some(a), None) => dist2(lab[a], lab[i]).sqrt(),
                    (None, Some(b)) => dist2(lab[b], lab[i]).sqrt(),
                    (None, None) => 0.0,
                }
            };
            let gx = axis((x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1));
            let gy = axis((y > 0).then(|| i - w), (y + 1 < h).then(|| i + w));
            *g = gx.max(gy);
        }
    });
    out
}

/// Per region: pixel count, mean Lab (of `lab`), and inclusive bounding box
/// `(x0, y0, x1, y1)`.
struct RegionStats {
    area: Vec<usize>,
    mean: Vec<Lab>,
    bbox: Vec<(usize, usize, usize, usize)>,
    /// The mean source gradient over the region's pixels: its own texture.
    grad_mean: Vec<f32>,
    /// Most of the region lies on a textured surface (a wash).
    textured: Vec<bool>,
    /// The pixel count of the largest surface the region lies on.
    surface_area: Vec<usize>,
}

fn region_stats(region: &[u32], n: usize, lab: &[Lab], grad: &[f32], info: &SurfaceInfo, w: usize, h: usize) -> RegionStats {
    let mut area = vec![0usize; n];
    let mut sums = vec![[0.0f64; 3]; n];
    let mut grad_sums = vec![0.0f64; n];
    let mut textured_px = vec![0usize; n];
    let mut surface_area = vec![0usize; n];
    let mut bbox = vec![(usize::MAX, usize::MAX, 0usize, 0usize); n];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let r = region[i];
            if r == NONE {
                continue;
            }
            let r = r as usize;
            area[r] += 1;
            sums[r][0] += lab[i][0] as f64;
            sums[r][1] += lab[i][1] as f64;
            sums[r][2] += lab[i][2] as f64;
            grad_sums[r] += grad[i] as f64;
            let s = info.of[i];
            if s != NONE {
                let s = s as usize;
                if info.textured[s] {
                    textured_px[r] += 1;
                }
                surface_area[r] = surface_area[r].max(info.area[s]);
            }
            let b = &mut bbox[r];
            b.0 = b.0.min(x);
            b.1 = b.1.min(y);
            b.2 = b.2.max(x);
            b.3 = b.3.max(y);
        }
    }
    let mean = (0..n)
        .map(|r| {
            let a = area[r].max(1) as f64;
            [(sums[r][0] / a) as f32, (sums[r][1] / a) as f32, (sums[r][2] / a) as f32]
        })
        .collect();
    let grad_mean = (0..n).map(|r| (grad_sums[r] / area[r].max(1) as f64) as f32).collect();
    let textured = (0..n).map(|r| 2 * textured_px[r] > area[r]).collect();
    RegionStats { area, mean, bbox, grad_mean, textured, surface_area }
}

/// `true` when region `r` (of `area` px²) is at least [`THIN_STRIPE_MIN_PX2`]
/// and its distance to its own edge never exceeds [`THIN_STRIPE_MAX_DEPTH_PX`]
/// (see those constants).
fn is_thin(region: &[u32], r: u32, area: usize, bbox: (usize, usize, usize, usize), w: usize) -> bool {
    if area < THIN_STRIPE_MIN_PX2 {
        return false;
    }
    let (x0, y0, x1, y1) = bbox;
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mask: Vec<bool> = (0..bw * bh).map(|k| region[(y0 + k / bw) * w + x0 + k % bw] == r).collect();
    let depth = crate::trace_lines::edt(&mask, bw, bh);
    depth.iter().all(|&d| d <= THIN_STRIPE_MAX_DEPTH_PX)
}

/// A thin region whose mean colour lies within this (ΔE) of the straight line
/// between two of its neighbours' means — between them in colour as well as in
/// place — is the anti-aliased seam between those two (the grey 1 px strip
/// where a dark trouser leg meets the paper), not a stripe, and merges. A
/// plaid thread between two patches of the same cream is not on their segment.
const SEAM_MIX_MAX_DE: f32 = 6.0;

/// Whether `c` lies within [`SEAM_MIX_MAX_DE`] of the segment between some two
/// of `neighbours`' means (strictly between: projection in `0.1..=0.9`).
fn is_mix_of_neighbours(c: Lab, neighbours: &[u32], means: &[Lab]) -> bool {
    let bar2 = SEAM_MIX_MAX_DE * SEAM_MIX_MAX_DE;
    for (i, &a) in neighbours.iter().enumerate() {
        for &b in &neighbours[i + 1..] {
            let (pa, pb) = (means[a as usize], means[b as usize]);
            let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if len2 < 1e-6 {
                continue;
            }
            let v = [c[0] - pa[0], c[1] - pa[1], c[2] - pa[2]];
            let t = (v[0] * d[0] + v[1] * d[1] + v[2] * d[2]) / len2;
            if !(0.1..=0.9).contains(&t) {
                continue;
            }
            let proj = [pa[0] + t * d[0], pa[1] + t * d[1], pa[2] + t * d[2]];
            if dist2(c, proj) <= bar2 {
                return true;
            }
        }
    }
    false
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self { parent: (0..n as u32).collect() }
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let gp = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = gp;
            x = gp;
        }
        x
    }

    /// Join the sets of `a` and `b` (the smaller root wins); `false` when they
    /// were already one.
    fn union(&mut self, a: u32, b: u32) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        let (lo, hi) = (ra.min(rb), ra.max(rb));
        self.parent[hi as usize] = lo;
        true
    }
}

/// Step 5's merges, repeated until stable (at most [`MERGE_ROUNDS`]). Each
/// round, from the round's starting statistics:
///
/// - a region smaller than `ColourTuning::min_region_px2` (or
///   [`MIN_ENCLOSED_REGION_PX2`] when fully enclosed by ink: none of its
///   pixels is 4-adjacent to a non-ink pixel of another region) joins its
///   most similar 8-neighbour (ΔE of mean flattened Lab) — unless it is a
///   thin stripe ([`is_thin`]), or at least [`MIN_CONTRAST_REGION_PX2`] with a
///   mean at least `ColourTuning::contrast_region_min_de` from every
///   8-neighbour's (a leaf, a glint);
/// - two 4-adjacent regions with mean ΔE under
///   `ColourTuning::soft_merge_max_de` whose shared boundary pixels have mean
///   `grad` under [`SOFT_MERGE_MAX_GRADIENT`] become one.
///
/// Regions are renumbered in raster order of their first pixel after every
/// round. Returns the new region count.
fn merge_regions(
    region: &mut [u32],
    mut n: usize,
    flat: &[Lab],
    ink: &[bool],
    grad: &[f32],
    info: &SurfaceInfo,
    w: usize,
    h: usize,
    ct: &ColourTuning,
) -> usize {
    for _ in 0..MERGE_ROUNDS {
        if n == 0 {
            break;
        }
        let stats = region_stats(region, n, flat, grad, info, w, h);

        // Enclosed by ink, and adjacency (8 for the small merges, 4 with the
        // boundary gradient for the soft merges).
        let mut enclosed = vec![true; n];
        let mut pairs8: Vec<(u32, u32)> = Vec::new();
        let mut edges4: Vec<(u32, u32, f32)> = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let a = region[i];
                if a == NONE {
                    continue;
                }
                if enclosed[a as usize] {
                    let four = [
                        (x > 0).then(|| i - 1),
                        (x + 1 < w).then(|| i + 1),
                        (y > 0).then(|| i - w),
                        (y + 1 < h).then(|| i + w),
                    ];
                    for j in four {
                        match j {
                            None => enclosed[a as usize] = false,
                            Some(j) if region[j] != a && !ink[j] => enclosed[a as usize] = false,
                            _ => {}
                        }
                    }
                }
                // Forward neighbours only, so each pair of pixels is seen once:
                // right and down (4-adjacent), down-right and down-left.
                let forward = [
                    ((x + 1 < w).then(|| i + 1), true),
                    ((y + 1 < h).then(|| i + w), true),
                    ((x + 1 < w && y + 1 < h).then(|| i + w + 1), false),
                    ((x > 0 && y + 1 < h).then(|| i + w - 1), false),
                ];
                for (j, four) in forward {
                    let Some(j) = j else { continue };
                    let b = region[j];
                    if b == NONE || b == a {
                        continue;
                    }
                    let key = (a.min(b), a.max(b));
                    if pairs8.last() != Some(&key) {
                        pairs8.push(key);
                    }
                    if four {
                        edges4.push((key.0, key.1, grad[i] + grad[j]));
                    }
                }
            }
        }
        pairs8.sort_unstable();
        pairs8.dedup();
        // Stable sort: equal keys keep scan order, so the sums are deterministic.
        edges4.sort_by(|p, q| (p.0, p.1).cmp(&(q.0, q.1)));
        let mut soft: Vec<(u32, u32, f32)> = Vec::new();
        let mut k = 0usize;
        while k < edges4.len() {
            let (a, b) = (edges4[k].0, edges4[k].1);
            let mut sum = 0.0f32;
            let mut count = 0usize;
            while k < edges4.len() && edges4[k].0 == a && edges4[k].1 == b {
                sum += edges4[k].2;
                count += 2;
                k += 1;
            }
            soft.push((a, b, sum / count as f32));
        }
        let mut adjacent: Vec<Vec<u32>> = vec![Vec::new(); n];
        for &(a, b) in &pairs8 {
            adjacent[a as usize].push(b);
            adjacent[b as usize].push(a);
        }

        let mut uf = UnionFind::new(n);
        // Running Lab sums per union-find root, so a merge is judged against
        // the colour of what a region has already become: without this a
        // chain of pairwise-similar regions (sari → gold border → red blouse)
        // fused into one in a single round.
        let mut sums: Vec<[f64; 3]> =
            (0..n).map(|r| [stats.mean[r][0] as f64, stats.mean[r][1] as f64, stats.mean[r][2] as f64].map(|v| v * stats.area[r] as f64)).collect();
        let mut counts: Vec<f64> = (0..n).map(|r| stats.area[r] as f64).collect();
        let root_mean = |uf: &mut UnionFind, sums: &[[f64; 3]], counts: &[f64], r: u32| -> Lab {
            let root = uf.find(r) as usize;
            let c = counts[root].max(1.0);
            [(sums[root][0] / c) as f32, (sums[root][1] / c) as f32, (sums[root][2] / c) as f32]
        };
        let join = |uf: &mut UnionFind, sums: &mut [[f64; 3]], counts: &mut [f64], a: u32, b: u32| -> bool {
            let (ra, rb) = (uf.find(a) as usize, uf.find(b) as usize);
            if !uf.union(a, b) {
                return false;
            }
            let root = uf.find(a) as usize;
            let other = if root == ra { rb } else { ra };
            for k in 0..3 {
                sums[root][k] += sums[other][k];
            }
            counts[root] += counts[other];
            true
        };
        let mut merged = false;
        let contrast_bar = ct.contrast_region_min_de * ct.contrast_region_min_de;
        let textured_bar = TEXTURED_MERGE_MAX_DE * TEXTURED_MERGE_MAX_DE;
        for r in 0..n {
            // On a textured surface a patch is small until it is a real share
            // of the surface, and only a strongly contrasting one is a mark.
            let (floor, keep_bar) = if enclosed[r] {
                (MIN_ENCLOSED_REGION_PX2, contrast_bar)
            } else if stats.textured[r] {
                (ct.min_region_px2.max((TEXTURED_MIN_SHARE * stats.surface_area[r] as f64) as usize), textured_bar)
            } else {
                (ct.min_region_px2, contrast_bar)
            };
            if stats.area[r] >= floor || adjacent[r].is_empty() {
                continue;
            }
            if stats.area[r] >= MIN_CONTRAST_REGION_PX2
                && adjacent[r]
                    .iter()
                    .all(|&s| dist2(stats.mean[r], stats.mean[s as usize]) >= keep_bar)
            {
                continue;
            }
            if is_thin(region, r as u32, stats.area[r], stats.bbox[r], w)
                && !is_mix_of_neighbours(stats.mean[r], &adjacent[r], &stats.mean)
            {
                continue;
            }
            let mut best: Option<(f32, u32)> = None;
            let mut nbrs = adjacent[r].clone();
            nbrs.sort_unstable();
            for s in nbrs {
                let d = dist2(stats.mean[r], stats.mean[s as usize]);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, s));
                }
            }
            if let Some((_, s)) = best {
                merged |= join(&mut uf, &mut sums, &mut counts, r as u32, s);
            }
        }
        let de_bar = ct.soft_merge_max_de * ct.soft_merge_max_de;
        // Most similar pairs first, each judged against the merged colours so
        // far (see `sums`).
        let mut soft_sorted: Vec<(f32, u32, u32, f32)> = soft
            .iter()
            .map(|&(a, b, g)| (dist2(stats.mean[a as usize], stats.mean[b as usize]), a, b, g))
            .collect();
        soft_sorted.sort_by(|p, q| p.0.total_cmp(&q.0).then((p.1, p.2).cmp(&(q.1, q.2))));
        for &(_, a, b, mean_grad) in &soft_sorted {
            // Soft relative to the two regions' own texture: a painterly wash
            // has grain everywhere (Guddu's shirt, gradient ≈ 3.7 per px), so
            // its region boundaries are no sharper than its interiors; a cel
            // shadow on smooth skin (interior ≈ 0.7) has a boundary ten times
            // sharper. Capped, so a busy pattern never merges across a real edge.
            let texture = 0.5 * (stats.grad_mean[a as usize] + stats.grad_mean[b as usize]);
            let bar = SOFT_MERGE_MAX_GRADIENT.max(SOFT_MERGE_TEXTURE_RATIO * texture).min(ct.soft_merge_gradient_cap);
            if mean_grad >= bar {
                continue;
            }
            let ma = root_mean(&mut uf, &sums, &counts, a);
            let mb = root_mean(&mut uf, &sums, &counts, b);
            // Two patches of one wash may differ far more than two cel tones.
            let pair_bar = if stats.textured[a as usize] && stats.textured[b as usize] { textured_bar } else { de_bar };
            if dist2(ma, mb) < pair_bar {
                merged |= join(&mut uf, &mut sums, &mut counts, a, b);
            }
        }
        if !merged {
            break;
        }
        let mut remap = vec![NONE; n];
        let mut next = 0u32;
        for px in region.iter_mut() {
            if *px == NONE {
                continue;
            }
            let root = uf.find(*px) as usize;
            if remap[root] == NONE {
                remap[root] = next;
                next += 1;
            }
            *px = remap[root];
        }
        n = next as usize;
    }
    n
}

// ---------------------------------------------------------------------------
// 8. Colour
// ---------------------------------------------------------------------------

fn median_channel(samples: &[[u8; 3]], c: usize) -> u8 {
    let mut v: Vec<u8> = samples.iter().map(|p| p[c]).collect();
    let mid = v.len() / 2;
    *v.select_nth_unstable(mid).1
}

/// The side of the RGB bins [`dominant_colour`] counts in.
const DOMINANT_BIN: u32 = 8;
/// ...and the RGB radius (Euclidean) round the densest bin's centre whose
/// pixels are averaged into the dominant colour.
const DOMINANT_RADIUS: f64 = 14.0;

/// **The dominant colour** of `samples`: the mean of the pixels within
/// [`DOMINANT_RADIUS`] of the centre of the densest [`DOMINANT_BIN`]-wide RGB
/// bin (lowest bin on a tie). For a painterly wash this is the ground the
/// pattern sits on, where the median is a mix of ground and pattern.
fn dominant_colour(samples: &[[u8; 3]]) -> [u8; 3] {
    let key = |p: &[u8; 3]| -> u32 {
        let b = |v: u8| v as u32 / DOMINANT_BIN;
        (b(p[0]) << 16) | (b(p[1]) << 8) | b(p[2])
    };
    let mut keys: Vec<u32> = samples.iter().map(key).collect();
    keys.sort_unstable();
    let (mut best_key, mut best_len) = (keys[0], 0usize);
    let mut s = 0usize;
    while s < keys.len() {
        let mut e = s;
        while e < keys.len() && keys[e] == keys[s] {
            e += 1;
        }
        if e - s > best_len {
            (best_key, best_len) = (keys[s], e - s);
        }
        s = e;
    }
    let centre = |k: u32, shift: u32| -> f64 { (((k >> shift) & 0xFF) * DOMINANT_BIN) as f64 + DOMINANT_BIN as f64 / 2.0 };
    let c = [centre(best_key, 16), centre(best_key, 8), centre(best_key, 0)];
    let r2 = DOMINANT_RADIUS * DOMINANT_RADIUS;
    let mut sum = [0.0f64; 3];
    let mut count = 0usize;
    for p in samples {
        let d = (0..3).map(|k| (p[k] as f64 - c[k]).powi(2)).sum::<f64>();
        if d <= r2 {
            for k in 0..3 {
                sum[k] += p[k] as f64;
            }
            count += 1;
        }
    }
    if count == 0 {
        return [c[0] as u8, c[1] as u8, c[2] as u8];
    }
    [
        (sum[0] / count as f64).round() as u8,
        (sum[1] / count as f64).round() as u8,
        (sum[2] / count as f64).round() as u8,
    ]
}

/// Per region: the per-channel median of its interior pixels (all 8
/// neighbours inside the picture and in the region) in `painted`; under
/// [`MIN_INTERIOR_PX`] interior pixels, of all its pixels. A textured region
/// (a wash) takes its [`dominant_colour`] instead: the ground, not the mix.
fn region_colours(region: &[u32], n: usize, painted: &[[u8; 3]], textured: &[bool], w: usize, h: usize) -> Vec<[u8; 3]> {
    let mut interior: Vec<Vec<[u8; 3]>> = vec![Vec::new(); n];
    let mut all: Vec<Vec<[u8; 3]>> = vec![Vec::new(); n];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let r = region[i];
            if r == NONE {
                continue;
            }
            all[r as usize].push(painted[i]);
            let inside = x > 0
                && y > 0
                && x + 1 < w
                && y + 1 < h
                && neighbours8(x, y, w, h).all(|j| region[j] == r);
            if inside {
                interior[r as usize].push(painted[i]);
            }
        }
    }
    (0..n)
        .map(|r| {
            let s = if interior[r].len() >= MIN_INTERIOR_PX { &interior[r] } else { &all[r] };
            if s.is_empty() {
                return [0, 0, 0];
            }
            if textured.get(r).copied().unwrap_or(false) {
                return dominant_colour(s);
            }
            [median_channel(s, 0), median_channel(s, 1), median_channel(s, 2)]
        })
        .collect()
}

/// The region covering the most opaque border pixels (lowest id on a tie);
/// `None` when no border pixel is opaque.
fn background_region(region: &[u32], n: usize, w: usize, h: usize) -> Option<u32> {
    let mut count = vec![0usize; n];
    for y in 0..h {
        for x in 0..w {
            if x != 0 && y != 0 && x + 1 != w && y + 1 != h {
                continue;
            }
            let r = region[y * w + x];
            if r != NONE {
                count[r as usize] += 1;
            }
        }
    }
    let mut best: Option<(usize, u32)> = None;
    for (r, &c) in count.iter().enumerate() {
        if c > 0 && best.is_none_or(|(bc, _)| c > bc) {
            best = Some((c, r as u32));
        }
    }
    best.map(|(_, r)| r)
}

// ---------------------------------------------------------------------------
// 6. Sub-pixel label map
// ---------------------------------------------------------------------------

/// `flat` bilinearly sampled at native `(x, y)`, over the opaque pixels only
/// (weights renormalised); `fallback` when none of the four is opaque.
fn sample_flat(flat: &[Lab], opaque: &[bool], w: usize, h: usize, x: f64, y: f64, fallback: Lab) -> Lab {
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let taps = [
        (y0 * w + x0, (1.0 - tx) * (1.0 - ty)),
        (y0 * w + x1, tx * (1.0 - ty)),
        (y1 * w + x0, (1.0 - tx) * ty),
        (y1 * w + x1, tx * ty),
    ];
    let mut acc = [0.0f32; 3];
    let mut wsum = 0.0f32;
    for (j, wt) in taps {
        if opaque[j] && wt > 0.0 {
            acc[0] += wt * flat[j][0];
            acc[1] += wt * flat[j][1];
            acc[2] += wt * flat[j][2];
            wsum += wt;
        }
    }
    if wsum > 0.0 { [acc[0] / wsum, acc[1] / wsum, acc[2] / wsum] } else { fallback }
}

/// The region of every `U×` cell (step 6): a cell over a transparent pixel is
/// [`NONE`]; over an ink pixel it keeps that pixel's (under-painted) region;
/// otherwise the flattened colour, bilinearly sampled at the cell centre,
/// picks the nearest palette colour among the labels present in the 3×3
/// native neighbourhood (the cell's own pixel first, so it wins ties), and the
/// cell takes the region of the pixel that colour came from. Rows in parallel.
/// With a `gate` (A1), an ink cell past the outer ink's split is [`NONE`].
#[allow(clippy::too_many_arguments)]
fn cell_labels(
    w: usize,
    h: usize,
    u: usize,
    flat: &[Lab],
    opaque: &[bool],
    ink: &[bool],
    pal: &[u32],
    centres: &[Lab],
    region: &[u32],
    gate: Option<&ExteriorGate>,
) -> Vec<u32> {
    let (uw, uh) = (w * u, h * u);
    let mut cells = vec![NONE; uw * uh];
    cells.par_chunks_mut(uw).enumerate().for_each(|(cy, row)| {
        let ny = cy / u;
        let sy = (cy as f64 + 0.5) / u as f64 - 0.5;
        for (cx, cell) in row.iter_mut().enumerate() {
            let nx = cx / u;
            let i = ny * w + nx;
            if !opaque[i] {
                continue;
            }
            if ink[i] {
                if gate.is_some_and(|g| !g.keeps(i, (cx as f64 + 0.5) / u as f64 - 0.5, sy)) {
                    continue;
                }
                *cell = region[i];
                continue;
            }
            let sx = (cx as f64 + 0.5) / u as f64 - 0.5;
            let c = sample_flat(flat, opaque, w, h, sx, sy, flat[i]);
            let mut best = (dist2(c, centres[pal[i] as usize]), region[i]);
            for j in neighbours8(nx, ny, w, h) {
                if !opaque[j] {
                    continue;
                }
                let d = dist2(c, centres[pal[j] as usize]);
                if d < best.0 {
                    best = (d, region[j]);
                }
            }
            *cell = best.1;
        }
    });
    cells
}

// ---------------------------------------------------------------------------
// 7. Vectorisation (stage A: per region)
// ---------------------------------------------------------------------------

/// Region `r`'s `U×` cells (which all lie over its native bounding box grown
/// by 1 px), grown by [`REGION_GROW_CELLS`] (square), traced with
/// [`trace_contours`] and every loop smoothed and fit by
/// [`crate::trace_ink::loop_path`] — one path, outer loops and holes. A cell
/// over a native `exterior` pixel (A1) is never grown into.
#[allow(clippy::too_many_arguments)]
fn region_path(
    cells: &[u32],
    uw: usize,
    uh: usize,
    u: usize,
    r: u32,
    bbox: (usize, usize, usize, usize),
    line_smooth: f64,
    t: &InkTuning,
    exterior: Option<&[bool]>,
) -> BezPath {
    let g = REGION_GROW_CELLS;
    let ux0 = bbox.0.saturating_sub(1) * u;
    let uy0 = bbox.1.saturating_sub(1) * u;
    let ux1 = ((bbox.2 + 2) * u).min(uw);
    let uy1 = ((bbox.3 + 2) * u).min(uh);
    let (gx0, gy0) = (ux0.saturating_sub(g), uy0.saturating_sub(g));
    let (gx1, gy1) = ((ux1 + g).min(uw), (uy1 + g).min(uh));
    let (gw, gh) = (gx1 - gx0, gy1 - gy0);
    let inside: Vec<bool> = (0..gw * gh).map(|k| cells[(gy0 + k / gw) * uw + gx0 + k % gw] == r).collect();
    let mut grid = Grid::new(gw, gh);
    let gi = g as i64;
    for y in 0..gh {
        for x in 0..gw {
            let hit = (-gi..=gi).any(|dy| {
                (-gi..=gi).any(|dx| {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    nx >= 0 && ny >= 0 && (nx as usize) < gw && (ny as usize) < gh && inside[ny as usize * gw + nx as usize]
                })
            });
            if hit && !exterior.is_some_and(|e| e[((gy0 + y) / u) * (uw / u) + (gx0 + x) / u]) {
                grid.set(x, y);
            }
        }
    }
    let mut path = BezPath::new();
    for lp in trace_contours(&grid) {
        if lp.len() < MIN_LOOP_POINTS {
            continue;
        }
        let shifted: Vec<(usize, usize)> = lp.iter().map(|&(x, y)| (x + gx0, y + gy0)).collect();
        for el in loop_path(&shifted, u, line_smooth, t).elements() {
            path.push(*el);
        }
    }
    path
}

// ---------------------------------------------------------------------------
// The trace
// ---------------------------------------------------------------------------

/// **Trace `pixels` into colour fills with the ink on top.** Dispatched from
/// [`crate::trace::trace`] for `TraceMode::InkAndColour`. See the module docs.
pub(crate) fn trace_ink_and_colour(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> TraceReport {
    trace_ink_and_colour_with(width, height, pixels, options, None, None)
}

/// [`trace_ink_and_colour`] with A6a's background pockets taken against
/// `background` (default: [`sheet_background`] of `pixels`), never inside
/// `protect`. Only with `drop_background`: a trace that keeps the paper keeps
/// its pockets too.
pub(crate) fn trace_ink_and_colour_with(
    width: u32,
    height: u32,
    pixels: &[u8],
    options: &TraceOptions,
    background: Option<[u8; 3]>,
    protect: Option<&[bool]>,
) -> TraceReport {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return TraceReport {
            message: "There is nothing in that picture to trace".into(),
            ..Default::default()
        };
    }
    let mut opaque: Vec<bool> = (0..w * h).map(|i| pixels[i * 4 + 3] > OPAQUE_MIN_ALPHA).collect();
    let (it, ct) = (&options.tuning.ink, &options.tuning.colour);

    // 1. Ink.
    let analysis = ink_analysis(width, height, pixels, it);
    // The ink the ink stage will actually emit: a dark-red plaid dash or a
    // bangle that it drops as a colour mark must stay a colour here, or it
    // is under-painted away and lost from both stages.
    let mut ink = grown_ink(&native_ink_mask(&analysis, pixels, w, h, it), &opaque, w, h, ct);
    drop(analysis);
    // A6a. The sheet's paper showing through inside the figure is transparent
    // from here on, like the outside (and A2 below then takes the soft rim
    // round it).
    if options.drop_background && ct.background_pocket_de > 0.0 {
        if let Some(bg) = background.or_else(|| sheet_background(width, height, pixels)) {
            let pockets = background_pockets(width, height, pixels, bg, protect, ct);
            let n = pockets.iter().filter(|&&p| p).count();
            if n > 0 {
                for (i, p) in pockets.into_iter().enumerate() {
                    if p {
                        opaque[i] = false;
                        ink[i] = false;
                    }
                }
            }
            if std::env::var_os("BUZZ_TRACE_POCKETS_LOG").is_some() {
                eprintln!("trace: background {bg:?}, {n} px of background pockets left out");
            }
        }
    }
    // A6d. The outline's anti-aliasing against the paper at the outer edge
    // (and round the pockets just left out) is transparent too.
    if options.drop_background && ct.blend_rim_px > 0.0 {
        if let Some(bg) = background.or_else(|| sheet_background(width, height, pixels)) {
            let rims = blend_rims(width, height, pixels, &opaque, bg, protect, ct);
            let mut n = 0usize;
            for (i, r) in rims.into_iter().enumerate() {
                if r {
                    opaque[i] = false;
                    ink[i] = false;
                    n += 1;
                }
            }
            if std::env::var_os("BUZZ_TRACE_POCKETS_LOG").is_some() {
                eprintln!("trace: {n} px of ink-paper rims left out");
            }
        }
    }
    // A2. The segmenter's anti-alias fringe outside the ink is transparent
    // from here on: it donates nothing and gets no cell.
    let fringe = segmenter_fringe(&opaque, &ink, w, h, ct.fringe_px);
    if let Ok(dir) = std::env::var("BUZZ_COLOUR_DEBUG_DIR") {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::create_dir_all(dir);
        let _ = image::save_buffer(dir.join(format!("input_{w}x{h}.png")), &pixels[..w * h * 4], w as u32, h as u32, image::ExtendedColorType::Rgba8);
        // edge.png: R = opaque after pockets, G = ink, B = A2 fringe.
        let mut e = vec![0u8; w * h * 3];
        for i in 0..w * h {
            e[i * 3] = if opaque[i] { 255 } else { 0 };
            e[i * 3 + 1] = if ink[i] { 255 } else { 0 };
            e[i * 3 + 2] = if fringe[i] { 255 } else { 0 };
        }
        let _ = image::save_buffer(dir.join(format!("edge_{w}x{h}.png")), &e, w as u32, h as u32, image::ExtendedColorType::Rgb8);
    }
    for (o, f) in opaque.iter_mut().zip(fringe) {
        if f {
            *o = false;
        }
    }

    // 2. Under-paint the ink.
    let (painted_ink, donor_ink) = under_paint(pixels, &ink, &opaque, w, h, ct);
    // The ink masses' cores: the ink the under-painting did not reach. They
    // keep their own colour and become regions below, so the ink stage leaves
    // the LARGE ones out and draws a rim round each (see `trace_ink_with`): a
    // shoe or a belt shows its own brown with a dark edge. A small core (a
    // brow, a pupil, a button) stays solid ink — its colour is the ink's
    // anyway, and the face logic wants a brow or a pupil as one mass.
    let mut mass_core: Vec<bool> = (0..w * h).map(|i| opaque[i] && ink[i] && donor_ink[i] == NONE).collect();
    {
        let (labels, n) = label_components(&mass_core, w, h);
        let mut area = vec![0usize; n as usize + 1];
        for &l in &labels {
            area[l as usize] += 1;
        }
        for (m, &l) in mass_core.iter_mut().zip(&labels) {
            if l == 0 || area[l as usize] < MASS_CORE_MIN_PX2 {
                *m = false;
            }
        }
    }
    let lab_ink: Vec<Lab> = painted_ink.par_iter().map(|&rgb| lab_of(rgb)).collect();

    // 2b. Colour lines, found on that picture, then under-painted too (from
    // the original: the nearest pixel that is neither ink nor line).
    let mut fc = colour_line_coverage(&lab_ink, &ink, &opaque, w, h, options.speckle, it);
    let covered: Vec<bool> = ink.iter().zip(&fc).map(|(&k, &v)| k || v >= it.ink_level).collect();
    let (painted, lab, donor) = if covered == ink {
        (painted_ink.clone(), lab_ink.clone(), donor_ink)
    } else {
        drop(donor_ink);
        let (p, d) = under_paint(pixels, &covered, &opaque, w, h, ct);
        let l: Vec<Lab> = p.par_iter().map(|&rgb| lab_of(rgb)).collect();
        (p, l, d)
    };
    drop(covered);

    // 3. Flatten.
    let mut flat = lab.clone();
    for _ in 0..ct.bilateral_passes {
        flat = bilateral(&flat, &opaque, w, h, ct);
    }

    // 4. Palettes per surface, and labels.
    let grad = gradient(&lab, &opaque, w, h);
    let (centres, pal, info) = surface_labels(&flat, &grad, &opaque, &ink, &donor, w, h, options.colours, ct);
    drop(donor);
    if std::env::var_os("BUZZ_COLOUR_DEBUG_DIR").is_some() {
        for (s, (&tex, &a)) in info.textured.iter().zip(&info.area).enumerate() {
            if a >= 2000 {
                eprintln!("colour debug: surface {s}: {a} px, textured {tex}");
            }
        }
    }
    let mut fills: Vec<(usize, u32, ShapeData)> = Vec::new();
    if !centres.is_empty() {
        // 5. Regions.
        let (mut region, n) = components_by_label(&pal, w, h);
        let n = merge_regions(&mut region, n, &flat, &ink, &grad, &info, w, h, ct);
        let stats = region_stats(&region, n, &flat, &grad, &info, w, h);

        // Debug: `BUZZ_COLOUR_DEBUG_DIR=<dir>` writes the flattened picture
        // (`flat.png`), the palette labels (`palette.png`) and the final
        // regions (`regions.png`, one hashed colour per region).
        if let Ok(dir) = std::env::var("BUZZ_COLOUR_DEBUG_DIR") {
            let dir = std::path::Path::new(&dir);
            let _ = std::fs::create_dir_all(dir);
            let hashed = |v: u32| -> [u8; 3] {
                let x = v.wrapping_mul(2654435761).rotate_left(13) ^ 0x9E37_79B9;
                [(x & 0xFF) as u8, ((x >> 8) & 0xFF) as u8, ((x >> 16) & 0xFF) as u8]
            };
            let mut flat_png = vec![255u8; w * h * 3];
            let mut pal_png = vec![255u8; w * h * 3];
            let mut reg_png = vec![255u8; w * h * 3];
            for i in 0..w * h {
                if !opaque[i] {
                    continue;
                }
                let rgb = crate::trace::lab_to_srgb([flat[i][0] as f64, flat[i][1] as f64, flat[i][2] as f64]);
                flat_png[i * 3..i * 3 + 3].copy_from_slice(&rgb);
                if pal[i] != NONE {
                    pal_png[i * 3..i * 3 + 3].copy_from_slice(&hashed(pal[i]));
                }
                if region[i] != NONE {
                    reg_png[i * 3..i * 3 + 3].copy_from_slice(&hashed(region[i]));
                }
            }
            for (name, buf) in [("flat.png", &flat_png), ("palette.png", &pal_png), ("regions.png", &reg_png)] {
                let _ = image::save_buffer(dir.join(name), buf, w as u32, h as u32, image::ExtendedColorType::Rgb8);
            }
            eprintln!("colour debug: {} palette centres, {n} regions", centres.len());
        }

        // 8. Colour (and the background left out).
        let colours = region_colours(&region, n, &painted, &stats.textured, w, h);
        let skip = if options.drop_background { background_region(&region, n, w, h) } else { None };

        // 2c. Residual pattern lines: what deviates from the fills, as lines
        // (see `residual_line_coverage`), added to the colour-line coverage.
        {
            let fill_lab: Vec<Lab> = colours.iter().map(|&c| lab_of(c)).collect();
            let residual =
                residual_line_coverage(&lab_ink, &region, &fill_lab, &ink, &opaque, w, h, options.speckle, it, ct);
            for (a, b) in fc.iter_mut().zip(&residual) {
                if *b > *a {
                    *a = *b;
                }
            }
        }

        // 6 + 7.
        let u = upsample_for(w, h, it);
        let (uw, uh) = (w * u, h * u);
        // A1. Fills stop at the outer ink's centreline.
        let gate = ExteriorGate::new(&opaque, &ink, &mass_core, &region, skip, w, h, ct.exterior_fill_reach);
        let cells = cell_labels(w, h, u, &flat, &opaque, &ink, &pal, &centres, &region, gate.as_ref());
        let exterior = gate.as_ref().map(|g| g.exterior.as_slice());
        fills = (0..n as u32)
            .into_par_iter()
            .filter(|&r| Some(r) != skip && stats.area[r as usize] > 0)
            .filter_map(|r| {
                let path =
                    region_path(&cells, uw, uh, u, r, stats.bbox[r as usize], options.line_smooth, it, exterior);
                if path.elements().is_empty() {
                    return None;
                }
                let [cr, cg, cb] = colours[r as usize];
                Some((stats.area[r as usize], r, ShapeData::filled(path, Color::from_rgb8(cr, cg, cb))))
            })
            .collect();
        fills.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }

    // 2b. The colour lines as shapes.
    let lines = colour_line_shapes(&fc, &painted_ink, w, h, options.line_smooth, it);

    // 9. Output: fills first, then the colour lines, the ink on top.
    let ink_report = trace_ink_with(width, height, pixels, options, Some(&mass_core));
    let n_fills = fills.len();
    let n_lines = lines.len();
    let n_ink = ink_report.shapes.len();
    let mut shapes: Vec<ShapeData> = fills.into_iter().map(|(_, _, s)| s).collect();
    shapes.extend(lines);
    shapes.extend(ink_report.shapes);
    TraceReport {
        shapes,
        regions: n_fills,
        specks: ink_report.specks,
        strokes: n_lines,
        fills: n_fills,
        message: format!("Traced {n_fills} fills, {n_lines} colour lines and {n_ink} ink shapes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;

    const INK: [f64; 3] = [40.0, 30.0, 25.0];
    const RED: [u8; 3] = [220, 40, 40];
    const BLUE: [u8; 3] = [40, 70, 210];

    fn finite(path: &BezPath) -> bool {
        use buzz_geom::PathEl::*;
        let ok = |p: buzz_geom::Point| p.x.is_finite() && p.y.is_finite();
        path.elements().iter().all(|el| match el {
            MoveTo(p) | LineTo(p) => ok(*p),
            QuadTo(a, b) => ok(*a) && ok(*b),
            CurveTo(a, b, c) => ok(*a) && ok(*b) && ok(*c),
            ClosePath => true,
        })
    }

    fn fill_rgb(shape: &ShapeData) -> [u8; 3] {
        let [r, g, b, _] = shape.fill.as_ref().unwrap().color().to_rgba8().to_u8_array();
        [r, g, b]
    }

    /// A red disc (centre (48, 48), radius 40) and a blue 60×60 square
    /// ([92, 152)²) on white, each outlined by a 3 px dark line just outside
    /// it; 4×4 supersampled.
    fn disc_and_square_png() -> (u32, u32, Vec<u8>) {
        const W: usize = 160;
        const H: usize = 160;
        const SS: usize = 4;
        let mut pixels = vec![255u8; W * H * 4];
        for py in 0..H {
            for px in 0..W {
                let mut acc = [0.0f64; 3];
                for sy in 0..SS {
                    for sx in 0..SS {
                        let x = px as f64 + (sx as f64 + 0.5) / SS as f64;
                        let y = py as f64 + (sy as f64 + 0.5) / SS as f64;
                        let d = ((x - 48.0).powi(2) + (y - 48.0).powi(2)).sqrt();
                        let in_square = (92.0..152.0).contains(&x) && (92.0..152.0).contains(&y);
                        let in_square_line = (89.0..155.0).contains(&x) && (89.0..155.0).contains(&y) && !in_square;
                        let c: [f64; 3] = if (40.0..=43.0).contains(&d) || in_square_line {
                            INK
                        } else if d < 40.0 {
                            [RED[0] as f64, RED[1] as f64, RED[2] as f64]
                        } else if in_square {
                            [BLUE[0] as f64, BLUE[1] as f64, BLUE[2] as f64]
                        } else {
                            [255.0; 3]
                        };
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                    }
                }
                let i = (py * W + px) * 4;
                for k in 0..3 {
                    pixels[i + k] = (acc[k] / (SS * SS) as f64).round() as u8;
                }
                pixels[i + 3] = 255;
            }
        }
        (W as u32, H as u32, pixels)
    }

    #[test]
    fn disc_and_square_fill_with_their_own_colours() {
        let (w, h, pixels) = disc_and_square_png();
        let report = trace_ink_and_colour(w, h, &pixels, &TraceOptions::ink_colour());
        assert!(report.fills >= 2, "{}", report.message);
        for shape in &report.shapes {
            assert!(finite(&shape.path));
        }
        let fills = &report.shapes[..report.fills];
        let closest = |target: [u8; 3]| {
            fills
                .iter()
                .min_by_key(|s| {
                    let c = fill_rgb(s);
                    (0..3).map(|k| (c[k] as i32 - target[k] as i32).abs()).sum::<i32>()
                })
                .unwrap()
        };
        for (target, expected_area) in [
            (RED, std::f64::consts::PI * 43.0 * 43.0),
            (BLUE, 66.0 * 66.0),
        ] {
            let shape = closest(target);
            let c = fill_rgb(shape);
            for k in 0..3 {
                assert!((c[k] as i32 - target[k] as i32).abs() <= 8, "{c:?} vs {target:?}");
            }
            let area = shape.path.area().abs();
            let err = (area - expected_area).abs() / expected_area;
            assert!(err < 0.10, "{target:?}: area {area} vs {expected_area} (err {err})");
        }
    }

    /// A 120×60 rectangle ([30, 150) × [30, 90)) holding a smooth horizontal
    /// gradient from (200, 150, 100) to (230, 180, 130), outlined by a 3 px
    /// dark line, on a white 180×120 canvas.
    fn gradient_rect_png() -> (u32, u32, Vec<u8>) {
        const W: usize = 180;
        const H: usize = 120;
        let mut pixels = vec![255u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let inside = (30..150).contains(&x) && (30..90).contains(&y);
                let line = (27..153).contains(&x) && (27..93).contains(&y) && !inside;
                let c: [u8; 3] = if inside {
                    let t = (x as f64 + 0.5 - 30.0) / 120.0;
                    let lerp = |a: f64, b: f64| (a + (b - a) * t).round() as u8;
                    [lerp(200.0, 230.0), lerp(150.0, 180.0), lerp(100.0, 130.0)]
                } else if line {
                    [INK[0] as u8, INK[1] as u8, INK[2] as u8]
                } else {
                    [255, 255, 255]
                };
                let i = (y * W + x) * 4;
                pixels[i..i + 3].copy_from_slice(&c);
                pixels[i + 3] = 255;
            }
        }
        (W as u32, H as u32, pixels)
    }

    #[test]
    fn smooth_gradient_is_one_region() {
        let (w, h, pixels) = gradient_rect_png();
        let report = trace_ink_and_colour(w, h, &pixels, &TraceOptions::ink_colour());
        assert_eq!(report.fills, 1, "{}", report.message);
        for shape in &report.shapes {
            assert!(finite(&shape.path));
        }
    }

    const CREAM: [u8; 3] = [245, 235, 205];
    const ORANGE: [u8; 3] = [240, 140, 40];

    /// A 200×200 cream canvas holding a 120×120 square ([40, 160)²) outlined
    /// by a 3 px dark line just outside it; inside the square, a plaid of 2 px
    /// orange lines every 20 px in both directions (x and y in [50, 52),
    /// [70, 72), …, [150, 152)).
    fn plaid_square_png() -> (u32, u32, Vec<u8>) {
        const W: usize = 200;
        const H: usize = 200;
        let thread = |v: usize| v >= 50 && (v - 50) % 20 < 2;
        let mut pixels = vec![255u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let inside = (40..160).contains(&x) && (40..160).contains(&y);
                let line = (37..163).contains(&x) && (37..163).contains(&y) && !inside;
                let c: [u8; 3] = if line {
                    [INK[0] as u8, INK[1] as u8, INK[2] as u8]
                } else if inside && (thread(x) || thread(y)) {
                    ORANGE
                } else {
                    CREAM
                };
                let i = (y * W + x) * 4;
                pixels[i..i + 3].copy_from_slice(&c);
                pixels[i + 3] = 255;
            }
        }
        (W as u32, H as u32, pixels)
    }

    #[test]
    fn plaid_threads_trace_as_colour_lines_over_a_plain_fill() {
        let (w, h, pixels) = plaid_square_png();
        // The canvas and the square are one cream surface once the outline is
        // under-painted, so the background is kept to have a base fill.
        let options = TraceOptions { drop_background: false, ..TraceOptions::ink_colour() };
        let report = trace_ink_and_colour(w, h, &pixels, &options);
        for shape in &report.shapes {
            assert!(finite(&shape.path));
        }
        let near = |c: [u8; 3], t: [u8; 3]| (0..3).all(|k| (c[k] as i32 - t[k] as i32).abs() <= 8);
        let fills = &report.shapes[..report.fills];
        let lines = &report.shapes[report.fills..report.fills + report.strokes];

        // The square's base fill (the topmost fill under a point between threads).
        let base = fills
            .iter()
            .rev()
            .find(|s| s.path.contains(buzz_geom::Point::new(60.5, 60.5)))
            .unwrap_or_else(|| panic!("no fill under the square: {}", report.message));
        assert!(near(fill_rgb(base), CREAM), "base {:?}: {}", fill_rgb(base), report.message);
        for s in fills {
            assert!(!near(fill_rgb(s), ORANGE), "an orange fill: {}", report.message);
        }
        assert!(!lines.is_empty(), "{}", report.message);
        for s in lines {
            assert!(near(fill_rgb(s), ORANGE), "line {:?}: {}", fill_rgb(s), report.message);
        }
        // A crossing plaid is ONE 8-connected component, so one shape (or a
        // few) is right; what matters is that the colour lines cover the
        // drawn threads: their area must be at least half the orange pixels.
        let orange_px = (0..(w * h) as usize)
            .filter(|&i| near([pixels[i * 4], pixels[i * 4 + 1], pixels[i * 4 + 2]], ORANGE))
            .count() as f64;
        let line_area: f64 = lines.iter().map(|s| s.path.area().abs()).sum();
        assert!(
            line_area >= 0.5 * orange_px,
            "colour lines cover {line_area:.0} px² of {orange_px:.0} orange px: {}",
            report.message
        );
    }

    const PLAID_CREAM: [u8; 3] = [235, 225, 205];
    const BAND_V: [u8; 3] = [190, 170, 150];
    const BAND_H: [u8; 3] = [215, 170, 130];

    /// `true` on a broad plaid band: 12 px wide every 40 px, panel offsets
    /// [9, 21), [49, 61), [89, 101), [129, 141) (canvas [34, 46) … [154, 166)).
    fn broad_band(v: usize) -> bool {
        (34..166).contains(&v) && (v - 34) % 40 < 12
    }

    /// A 200×200 cream canvas holding a 150×150 shirt panel ([25, 175)²)
    /// outlined by a 3 px dark line just outside it; inside, on the cream, a
    /// plaid of broad bands ([`broad_band`]): vertical in `BAND_V`, horizontal
    /// in `BAND_H`, the horizontal drawn over the vertical at the crossings.
    fn broad_plaid_png() -> (u32, u32, Vec<u8>) {
        const W: usize = 200;
        const H: usize = 200;
        let mut pixels = vec![255u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let inside = (25..175).contains(&x) && (25..175).contains(&y);
                let line = (22..178).contains(&x) && (22..178).contains(&y) && !inside;
                let c: [u8; 3] = if line {
                    [INK[0] as u8, INK[1] as u8, INK[2] as u8]
                } else if inside && broad_band(y) {
                    BAND_H
                } else if inside && broad_band(x) {
                    BAND_V
                } else {
                    PLAID_CREAM
                };
                let i = (y * W + x) * 4;
                pixels[i..i + 3].copy_from_slice(&c);
                pixels[i + 3] = 255;
            }
        }
        (W as u32, H as u32, pixels)
    }

    #[test]
    fn broad_plaid_bands_trace_as_strips() {
        let (w, h, pixels) = broad_plaid_png();
        let report = trace_ink_and_colour(w, h, &pixels, &TraceOptions::ink_colour());
        for shape in &report.shapes {
            assert!(finite(&shape.path));
        }
        let near = |c: [u8; 3], t: [u8; 3]| (0..3).all(|k| (c[k] as i32 - t[k] as i32).abs() <= 8);
        let in_panel = |s: &ShapeData| {
            let bb = s.path.bounding_box();
            bb.x0 >= 20.0 && bb.y0 >= 20.0 && bb.x1 <= 180.0 && bb.y1 <= 180.0
        };
        let fills: Vec<&ShapeData> = report.shapes[..report.fills].iter().filter(|s| in_panel(s)).collect();
        let bands = fills
            .iter()
            .filter(|s| near(fill_rgb(s), BAND_V) || near(fill_rgb(s), BAND_H))
            .count();
        for s in &fills {
            let area = s.path.area().abs();
            let c = s.path.bounding_box().center();
            let crossing = broad_band(c.x as usize) && broad_band(c.y as usize);
            assert!(
                area >= 40.0 || crossing,
                "a {area:.0} px² fill at {c:?} ({:?}): {}",
                fill_rgb(s),
                report.message
            );
        }
        // 4 horizontal strips run whole; the 4 vertical bands are cut by them
        // into 5 segments each: 4 + 20 = 24 strips. A blotched plaid gives
        // dozens of blobs more; a lost band set gives fewer than 20.
        assert!((20..=30).contains(&bands), "{bands} band fills: {}", report.message);
    }

    const LEAF_YELLOW: [u8; 3] = [221, 165, 65];
    const LEAF_GREEN: [u8; 3] = [60, 140, 60];

    /// A 120×120 yellow square (the whole canvas) holding nine 3×3 green
    /// leaves, their corners at x, y ∈ {20, 58, 96}.
    fn leaves_png() -> (u32, u32, Vec<u8>) {
        const W: usize = 120;
        const H: usize = 120;
        let leaf = |v: usize| [20usize, 58, 96].iter().any(|&o| (o..o + 3).contains(&v));
        let mut pixels = vec![255u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let c = if leaf(x) && leaf(y) { LEAF_GREEN } else { LEAF_YELLOW };
                let i = (y * W + x) * 4;
                pixels[i..i + 3].copy_from_slice(&c);
                pixels[i + 3] = 255;
            }
        }
        (W as u32, H as u32, pixels)
    }

    #[test]
    fn small_high_contrast_leaves_survive() {
        let (w, h, pixels) = leaves_png();
        let report = trace_ink_and_colour(w, h, &pixels, &TraceOptions::ink_colour());
        for shape in &report.shapes {
            assert!(finite(&shape.path));
        }
        let near = |c: [u8; 3], t: [u8; 3]| (0..3).all(|k| (c[k] as i32 - t[k] as i32).abs() <= 8);
        let greens = report.shapes[..report.fills]
            .iter()
            .filter(|s| near(fill_rgb(s), LEAF_GREEN))
            .count();
        assert_eq!(greens, 9, "{}", report.message);
    }

    #[test]
    fn smoke_pujari_front_ink_and_colour() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../buzz-ai/tests/data/pujariFront_0.png");
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return, // the fixture is optional for this crate's own tests
        };
        let img = ::image::load_from_memory(&bytes).expect("decode pujariFront_0.png").to_rgba8();
        let (w, h) = img.dimensions();
        let pixels = img.into_raw();

        let started = std::time::Instant::now();
        let report = trace_ink_and_colour(w, h, &pixels, &TraceOptions::ink_colour());
        let elapsed = started.elapsed();

        assert!(report.fills >= 1, "{}", report.message);
        for shape in &report.shapes {
            assert!(finite(&shape.path));
        }
        assert!(elapsed.as_secs_f64() < 4.0, "{elapsed:?}");
    }

    /// The ringed disc's centre, its red radius, and its dark ring's outer
    /// radius (the ring is [`RING_IN`, `RING_OUT`], centreline 41.5).
    const RING_C: f64 = 56.0;
    const RING_IN: f64 = 40.0;
    const RING_OUT: f64 = 43.0;

    /// A 112×112 red disc (centre (56, 56), radius 40) outlined by a 3 px
    /// dark ring (radius 40–43), optionally ringed again by a 1 px light-grey
    /// (luma 190, opaque) ring (43–44), on `background` (opaque) or on
    /// transparency (`None`: alpha is the 4×4-supersampled coverage, colour
    /// the mean of the covered samples).
    fn ringed_disc_png(background: Option<[u8; 3]>, grey_ring: bool) -> (u32, u32, Vec<u8>) {
        const W: usize = 112;
        const H: usize = 112;
        const SS: usize = 4;
        let mut pixels = vec![0u8; W * H * 4];
        for py in 0..H {
            for px in 0..W {
                let mut acc = [0.0f64; 3];
                let mut covered = 0usize;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let x = px as f64 + (sx as f64 + 0.5) / SS as f64;
                        let y = py as f64 + (sy as f64 + 0.5) / SS as f64;
                        let d = ((x - RING_C).powi(2) + (y - RING_C).powi(2)).sqrt();
                        let c: Option<[f64; 3]> = if d < RING_IN {
                            Some([RED[0] as f64, RED[1] as f64, RED[2] as f64])
                        } else if d <= RING_OUT {
                            Some(INK)
                        } else if grey_ring && d <= RING_OUT + 1.0 {
                            Some([190.0; 3])
                        } else {
                            background.map(|b| [b[0] as f64, b[1] as f64, b[2] as f64])
                        };
                        if let Some(c) = c {
                            covered += 1;
                            for k in 0..3 {
                                acc[k] += c[k];
                            }
                        }
                    }
                }
                let i = (py * W + px) * 4;
                if covered > 0 {
                    for k in 0..3 {
                        pixels[i + k] = (acc[k] / covered as f64).round() as u8;
                    }
                }
                pixels[i + 3] = (covered as f64 / (SS * SS) as f64 * 255.0).round() as u8;
            }
        }
        (W as u32, H as u32, pixels)
    }

    /// Every pixel centre of the ringed-disc canvas, with its distance from
    /// the disc's centre.
    fn ring_pixels() -> impl Iterator<Item = (buzz_geom::Point, f64)> {
        (0..112).flat_map(|y| {
            (0..112).map(move |x| {
                let p = buzz_geom::Point::new(x as f64 + 0.5, y as f64 + 0.5);
                (p, ((p.x - RING_C).powi(2) + (p.y - RING_C).powi(2)).sqrt())
            })
        })
    }

    /// A point-sampled raster: whether any of `shapes` covers `p`.
    fn painted(shapes: &[ShapeData], p: buzz_geom::Point) -> bool {
        shapes.iter().any(|s| s.path.contains(p))
    }

    /// The ringed disc traced with `ink_colour()`: asserts that no fill
    /// covers a pixel centre more than 0.5 px past the ring's centreline and
    /// that fills + ink leave no pixel inside the ring's inner radius bare;
    /// returns the farthest painted pixel centre's distance from the centre.
    fn assert_fills_stop_at_the_centreline(background: Option<[u8; 3]>) -> f64 {
        let (w, h, pixels) = ringed_disc_png(background, false);
        let report = trace_ink_and_colour(w, h, &pixels, &TraceOptions::ink_colour());
        assert!(report.fills >= 1, "{}", report.message);
        for shape in &report.shapes {
            assert!(finite(&shape.path));
        }
        let fills = &report.shapes[..report.fills];
        let centreline = (RING_IN + RING_OUT) / 2.0;
        let mut farthest = 0.0f64;
        for (p, d) in ring_pixels() {
            if painted(fills, p) {
                farthest = farthest.max(d);
                assert!(
                    d <= centreline + 0.5,
                    "a fill at {p:?}, {:.2} px past the centreline: {}",
                    d - centreline,
                    report.message
                );
            }
            if d < RING_IN {
                assert!(painted(&report.shapes, p), "bare at {p:?} ({d:.2} px from the centre): {}", report.message);
            }
        }
        farthest
    }

    #[test]
    fn fills_stop_at_the_outer_outlines_centreline() {
        let farthest = assert_fills_stop_at_the_centreline(None);
        assert!(farthest >= RING_IN, "the fill stops at {farthest:.2}, short of the ink");
    }

    /// Each route on its own stays within centreline + 0.5 px (asserted in
    /// [`assert_fills_stop_at_the_centreline`]); the two may differ by up to
    /// one native pixel: `grown_ink` grows only into opaque pixels, so on
    /// transparency the ink band is grown inward only and the split sits
    /// ≈ 0.5 px inside the ring's centre, while on white it is grown both
    /// ways (measured 2026-10-08: 41.19 vs 41.74).
    #[test]
    fn the_same_picture_on_white_with_drop_background_matches() {
        let on_transparent = assert_fills_stop_at_the_centreline(None);
        let on_white = assert_fills_stop_at_the_centreline(Some([255; 3]));
        assert!(
            (on_white - on_transparent).abs() <= 1.0,
            "the fill reaches {on_white:.2} on white, {on_transparent:.2} on transparency"
        );
    }

    #[test]
    fn the_segmenters_light_fringe_is_not_painted() {
        let (w, h, pixels) = ringed_disc_png(None, true);
        let report = trace_ink_and_colour(w, h, &pixels, &TraceOptions::ink_colour());
        assert!(report.fills >= 1, "{}", report.message);
        let fills = &report.shapes[..report.fills];
        for (p, d) in ring_pixels() {
            if (RING_OUT..=RING_OUT + 1.0).contains(&d) {
                assert!(!painted(fills, p), "the grey ring is painted at {p:?} ({d:.2}): {}", report.message);
            }
        }
        // The fill under the ink is still the disc's red (the donor is kept).
        let r = RING_IN + 0.5;
        for (dx, dy) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
            let p = buzz_geom::Point::new(RING_C + dx * r, RING_C + dy * r);
            let top = fills
                .iter()
                .rev()
                .find(|s| s.path.contains(p))
                .unwrap_or_else(|| panic!("no fill under the ink at {p:?}: {}", report.message));
            let c = fill_rgb(top);
            assert!((0..3).all(|k| (c[k] as i32 - RED[k] as i32).abs() <= 8), "{c:?} under the ink at {p:?}");
        }
    }
}
