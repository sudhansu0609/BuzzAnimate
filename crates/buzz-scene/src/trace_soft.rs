//! **Soft tones** (`docs/TRACE_PICTURE_PLAN.md` T9): the bands a colour trace
//! cuts a haze or a glow into become one shape with a gradient fill.
//!
//! Quantising a smooth ramp to a palette leaves a stack of neighbouring
//! regions whose shared edge is not an edge of the picture at all: the source
//! changes by a level or two across it. Such regions are merged into a soft
//! group, and the group gets a gradient fitted by least squares to its source
//! pixels (colour and alpha), linear along the best axis or radial about the
//! extremum, whichever fits better. The group is only kept when the gradient
//! cuts its mean error by a quarter; otherwise the bands stay as they were.

use crate::gradient::{Gradient, GradientKind, GradientStop};
use buzz_geom::Affine;
use peniko::Color;
use std::collections::HashMap;

/// A boundary is soft when the source changes by less than this (0-255, mean
/// over the boundary, the largest of the four channels) from one side to the other.
/// A group that no gradient fits is cut again at each softer level in turn.
const STEP_LEVELS: [f64; 5] = [14.0, 8.0, 5.0, 3.0, 2.0];
/// ... and the two regions' mean colours are within this of each other.
const MAX_MEAN_STEP: f64 = 60.0;
/// A pixel is badly off by this much (sum over the colour channels); the gradient
/// must also cut the share of such pixels by the same fraction as the mean error.
const BAD_ERROR: f64 = 24.0;
/// A boundary shorter than this many pixel pairs is not evidence.
const MIN_BOUNDARY: u32 = 4;
/// A group smaller than this (in picture pixels) is not worth a gradient.
const MIN_GROUP_PX: f64 = 48.0;
/// The gradient must cut the bands' mean error by at least this fraction
/// (T16: was a quarter; a ramp that is mostly right is still better than bands).
const MIN_GAIN: f64 = 0.15;
/// **Keep textures** (T16): a soft group of at least [`plan`]'s `texture_px` whose
/// best description (gradient, or flat bands when no gradient fits) is still off
/// by this much on average (sum of the colour channels' absolute errors, 0-765;
/// 6 is about 2 per channel, a ΔE of roughly 3) ...
const TEXTURE_MEAN_ERR: f64 = 6.0;
/// ... or has this share of pixels badly off ([`BAD_ERROR`]) is a sky, a wall or
/// a ground painted with texture: it is filled with the source pixels instead.
const TEXTURE_BAD_SHARE: f64 = 0.12;

/// One soft group [`plan`] found.
pub(crate) struct SoftGroup {
    /// The member regions (indices into `plan`'s `regions`).
    pub members: Vec<usize>,
    /// The fitted gradient, or `None` for a textured group (see `textured`).
    pub gradient: Option<Gradient>,
    /// The region a haze group is painted over.
    pub under: Option<usize>,
    /// Too textured or graded for any gradient: to be filled with the source pixels.
    pub textured: bool,
}

/// Whether a fit's (or the flat bands') error says the group is textured.
pub(crate) fn is_textured(mean_err: f64, bad_share: f64) -> bool {
    mean_err > TEXTURE_MEAN_ERR || bad_share > TEXTURE_BAD_SHARE
}

/// How far the source over `cells` is from the flat colour `rgb`: the mean error
/// (sum of the colour channels' absolute differences) and the share of pixels off
/// by more than [`BAD_ERROR`]. Strided to at most [`MAX_SAMPLES`] pixels.
pub(crate) fn flat_error(cells: &[usize], pixels: &[u8], rgb: [u8; 3]) -> (f64, f64) {
    let stride = (cells.len() / MAX_SAMPLES).max(1);
    let (mut sum, mut bad, mut n) = (0.0f64, 0usize, 0usize);
    for &at in cells.iter().step_by(stride) {
        let px = &pixels[at * 4..at * 4 + 3];
        let e: f64 = (0..3).map(|c| (px[c] as f64 - rgb[c] as f64).abs()).sum();
        sum += e;
        bad += (e > BAD_ERROR) as usize;
        n += 1;
    }
    let n = n.max(1) as f64;
    (sum / n, bad as f64 / n)
}
/// Stops per gradient.
const KNOTS: usize = 4;
/// The most source pixels a fit looks at.
const MAX_SAMPLES: usize = 30_000;

struct Sample {
    x: f64,
    y: f64,
    /// Straight RGBA, 0-255.
    v: [f64; 4],
    /// Weight of the colour channels (a faint pixel's colour counts for little).
    w: f64,
    /// Index of the member region this pixel belongs to.
    m: usize,
}

fn step(p: &[u8], q: &[u8]) -> f64 {
    let wgt = p[3].min(q[3]) as f64 / 255.0;
    let rgb = (0..3).map(|c| (p[c] as f64 - q[c] as f64).abs()).fold(0.0, f64::max) * wgt;
    rgb.max((p[3] as f64 - q[3] as f64).abs())
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The soft groups of `regions` (each of at least two region indices) that a
/// fitted gradient describes better than their flat bands, with that gradient
/// (in the traced grid's pixel space).
pub(crate) fn plan(
    regions: &[(u8, Vec<usize>)],
    region_of: &[u32],
    pixels: &[u8],
    w: usize,
    h: usize,
    factor: f64,
    texture_px: Option<f64>,
) -> Vec<SoftGroup> {
    let n = regions.len();
    if n < 2 {
        return Vec::new();
    }
    // Each region's mean RGBA (colour weighted by alpha).
    let means: Vec<[u8; 4]> = regions
        .iter()
        .map(|(_, cells)| {
            let mut sum = [0.0f64; 3];
            let (mut a_sum, mut wsum) = (0.0f64, 0.0f64);
            for &at in cells {
                let px = &pixels[at * 4..at * 4 + 4];
                let a = px[3] as f64;
                for c in 0..3 {
                    sum[c] += px[c] as f64 * a;
                }
                a_sum += a;
                wsum += 1.0;
            }
            let d = a_sum.max(1.0);
            [(sum[0] / d) as u8, (sum[1] / d) as u8, (sum[2] / d) as u8, (a_sum / wsum.max(1.0)) as u8]
        })
        .collect();

    // The source's own step across every boundary between two regions.
    let mut edges: HashMap<u64, (u32, f64)> = HashMap::new();
    let mut note = |a: u32, b: u32, i: usize, j: usize| {
        if a == b || a == u32::MAX || b == u32::MAX {
            return;
        }
        let key = ((a.min(b) as u64) << 32) | a.max(b) as u64;
        let e = edges.entry(key).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += step(&pixels[i * 4..i * 4 + 4], &pixels[j * 4..j * 4 + 4]);
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if x + 1 < w {
                note(region_of[i], region_of[i + 1], i, i + 1);
            }
            if y + 1 < h {
                note(region_of[i], region_of[i + w], i, i + w);
            }
        }
    }

    // Soft boundaries (with their step) and every boundary's length, for the
    // background a group sits on.
    let mut soft: Vec<(usize, usize, f64)> = Vec::new();
    let mut adjacent: Vec<(usize, usize, u32)> = Vec::new();
    let mut keys: Vec<u64> = edges.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let (count, sum) = edges[&key];
        let (a, b) = ((key >> 32) as usize, (key & 0xffff_ffff) as usize);
        adjacent.push((a, b, count));
        let mean_step = sum / count as f64;
        if count < MIN_BOUNDARY || mean_step > STEP_LEVELS[0] || step(&means[a], &means[b]) > MAX_MEAN_STEP {
            continue;
        }
        soft.push((a, b, mean_step));
    }
    // The pieces `members` falls into when only boundaries softer than `limit` join.
    let pieces_of = |members: &[usize], limit: f64| -> Vec<Vec<usize>> {
        let local: HashMap<usize, usize> = members.iter().enumerate().map(|(i, &m)| (m, i)).collect();
        let mut lp: Vec<usize> = (0..members.len()).collect();
        for &(a, b, st) in &soft {
            if st > limit {
                continue;
            }
            if let (Some(&la), Some(&lb)) = (local.get(&a), local.get(&b)) {
                let (ra, rb) = (find(&mut lp, la), find(&mut lp, lb));
                if ra != rb {
                    lp[ra.max(rb)] = ra.min(rb);
                }
            }
        }
        let mut by_root: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
        for (i, &m) in members.iter().enumerate() {
            by_root.entry(find(&mut lp, i)).or_default().push(m);
        }
        by_root.into_values().filter(|p| p.len() >= 2).collect()
    };

    let all: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    // A group no gradient fits is cut at the next softer level and its pieces
    // tried again, so a haze falls out of the chain of soft edges round it.
    let mut work: Vec<(Vec<usize>, usize)> = pieces_of(&all, STEP_LEVELS[0]).into_iter().map(|p| (p, 0)).collect();
    while let Some((members, level)) = work.pop() {
        let total: usize = members.iter().map(|&m| regions[m].1.len()).sum();
        if (total as f64) < MIN_GROUP_PX * factor * factor {
            continue;
        }
        let samples = gather(&members, regions, pixels, w);
        if samples.len() < 8 {
            continue;
        }
        let banded = mean_error(&samples, |s| {
            let m = &means[members[s.m]];
            [m[0] as f64, m[1] as f64, m[2] as f64, m[3] as f64]
        });
        let banded_bad = samples
            .iter()
            .filter(|s| {
                let m = &means[members[s.m]];
                sample_error(s, [m[0] as f64, m[1] as f64, m[2] as f64, m[3] as f64]) > BAD_ERROR
            })
            .count() as f64
            / samples.len() as f64;
        // The region it sits on: the one it shares the most boundary with.
        let in_group: std::collections::HashSet<usize> = members.iter().copied().collect();
        let mut touch: HashMap<usize, u32> = HashMap::new();
        for &(a, b, c) in &adjacent {
            match (in_group.contains(&a), in_group.contains(&b)) {
                (true, false) => *touch.entry(b).or_insert(0) += c,
                (false, true) => *touch.entry(a).or_insert(0) += c,
                _ => {}
            }
        }
        let bg_region = touch.iter().max_by_key(|&(&r, &c)| (c, usize::MAX - r)).map(|(&r, _)| r);
        let bg = bg_region.map(|b| [means[b][0] as f64, means[b][1] as f64, means[b][2] as f64]);
        let cap = banded_bad.max(0.01);
        let opaque = best_fit(&samples, cap, None);
        let over = bg.and_then(|b| best_fit(&samples, cap, Some(b)));
        let chosen = match (opaque, over) {
            (Some(a), Some(b)) => Some(if b.1 < a.1 { (b, bg_region) } else { (a, None) }),
            (Some(a), None) => Some((a, None)),
            (None, Some(b)) => Some((b, bg_region)),
            (None, None) => None,
        };
        let big = texture_px.is_some_and(|min| total as f64 >= min);
        match chosen {
            Some(((gradient, err, bad), under)) if err <= banded * (1.0 - MIN_GAIN) => {
                let textured = big && under.is_none() && is_textured(err, bad);
                out.push(SoftGroup { members, gradient: (!textured).then_some(gradient), under, textured })
            }
            _ if big && is_textured(banded, banded_bad) => {
                out.push(SoftGroup { members, gradient: None, under: None, textured: true })
            }
            _ if level + 1 < STEP_LEVELS.len() => {
                for piece in pieces_of(&members, STEP_LEVELS[level + 1]) {
                    if piece.len() < members.len() {
                        work.push((piece, level + 1));
                    }
                }
            }
            _ => {}
        }
    }
    // A group painted over a region that is itself part of a group has nothing
    // to reach under: leave it as bands.
    let grouped: std::collections::HashSet<usize> = out.iter().flat_map(|g: &SoftGroup| g.members.iter().copied()).collect();
    out.retain(|g| g.under.is_none_or(|b| !grouped.contains(&b)));
    out
}

/// The (strided) source pixels of `members`, tagged with their member's position.
fn gather(members: &[usize], regions: &[(u8, Vec<usize>)], pixels: &[u8], w: usize) -> Vec<Sample> {
    let total: usize = members.iter().map(|&m| regions[m].1.len()).sum();
    let stride = (total / MAX_SAMPLES).max(1);
    let mut samples: Vec<Sample> = Vec::with_capacity(total / stride + 1);
    let mut k = 0usize;
    for (mi, &m) in members.iter().enumerate() {
        for &at in &regions[m].1 {
            k += 1;
            if k % stride != 0 {
                continue;
            }
            let px = &pixels[at * 4..at * 4 + 4];
            samples.push(Sample {
                x: (at % w) as f64 + 0.5,
                y: (at / w) as f64 + 0.5,
                v: [px[0] as f64, px[1] as f64, px[2] as f64, px[3] as f64],
                w: (px[3] as f64 / 255.0).max(0.02),
                m: mi,
            });
        }
    }
    samples
}

fn sample_error(s: &Sample, pred: [f64; 4]) -> f64 {
    s.w * ((s.v[0] - pred[0]).abs() + (s.v[1] - pred[1]).abs() + (s.v[2] - pred[2]).abs())
        + (s.v[3] - pred[3]).abs()
}

fn mean_error(samples: &[Sample], pred: impl Fn(&Sample) -> [f64; 4]) -> f64 {
    samples.iter().map(|s| sample_error(s, pred(s))).sum::<f64>() / samples.len().max(1) as f64
}

/// The better of the linear and the two radial fits, with its mean error.
fn best_fit(samples: &[Sample], bad_cap: f64, bg: Option<[f64; 3]>) -> Option<(Gradient, f64, f64)> {
    let mut best: Option<(Gradient, f64, f64)> = None;
    let mut keep = |cand: Option<(Gradient, f64, f64)>| {
        if let Some(c) = cand
            && c.2 <= bad_cap
            && best.as_ref().is_none_or(|b| c.1 < b.1)
        {
            best = Some(c);
        }
    };
    keep(fit_linear(samples, bg));
    for &top in &[true, false] {
        if bg.is_some() && !top {
            break;
        }
        keep(fit_radial(samples, top, bg));
    }
    best
}

fn fit_linear(samples: &[Sample], bg: Option<[f64; 3]>) -> Option<(Gradient, f64, f64)> {
    // The colour's plane fit per channel; the axis is the dominant gradient direction.
    let n = samples.len() as f64;
    let (cx, cy) = (samples.iter().map(|s| s.x).sum::<f64>() / n, samples.iter().map(|s| s.y).sum::<f64>() / n);
    let (mut gxx, mut gxy, mut gyy) = (0.0, 0.0, 0.0);
    for c in 0..4 {
        let wt = |s: &Sample| if c == 3 { 1.0 } else { s.w };
        let sw: f64 = samples.iter().map(wt).sum::<f64>().max(1e-9);
        let mean = samples.iter().map(|s| wt(s) * s.v[c]).sum::<f64>() / sw;
        let (mut sxx, mut sxy, mut syy, mut sxv, mut syv) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for s in samples {
            let (dx, dy, dv, wv) = (s.x - cx, s.y - cy, s.v[c] - mean, wt(s));
            sxx += wv * dx * dx;
            sxy += wv * dx * dy;
            syy += wv * dy * dy;
            sxv += wv * dx * dv;
            syv += wv * dy * dv;
        }
        let det = sxx * syy - sxy * sxy;
        if det.abs() < 1e-9 {
            continue;
        }
        let a = (sxv * syy - syv * sxy) / det;
        let b = (syv * sxx - sxv * sxy) / det;
        gxx += a * a;
        gxy += a * b;
        gyy += b * b;
    }
    let theta = if gxx + gyy < 1e-12 { 0.0 } else { 0.5 * (2.0 * gxy).atan2(gxx - gyy) };
    let (dx, dy) = (theta.cos(), theta.sin());
    let ts: Vec<f64> = samples.iter().map(|s| (s.x - cx) * dx + (s.y - cy) * dy).collect();
    let (tmin, tmax) = ts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &t| (lo.min(t), hi.max(t)));
    let half = ((tmax - tmin) * 0.5).max(1e-3);
    let us: Vec<f64> = ts.iter().map(|&t| (t - tmin) / (2.0 * half)).collect();
    let (stops, err, bad) = ramp(samples, &us, bg, false);
    let mid = (tmin + tmax) * 0.5;
    let mut g = Gradient::new(GradientKind::Linear, stops);
    g.transform = Affine::translate((cx + dx * mid, cy + dy * mid)) * Affine::rotate(theta) * Affine::scale(half);
    Some((g, err, bad))
}

fn fit_radial(samples: &[Sample], top: bool, bg: Option<[f64; 3]>) -> Option<(Gradient, f64, f64)> {
    // The channel that varies most, and the centroid of its extreme 5 %.
    let n = samples.len() as f64;
    let mut best_c = 3;
    let mut best_var = -1.0;
    for c in 0..4 {
        let mean = samples.iter().map(|s| s.v[c]).sum::<f64>() / n;
        let var = samples.iter().map(|s| (s.v[c] - mean).powi(2)).sum::<f64>() / n * if c == 3 { 1.0 } else { 0.5 };
        if var > best_var {
            best_var = var;
            best_c = c;
        }
    }
    let mut order: Vec<usize> = (0..samples.len()).collect();
    let key = |i: usize| match bg {
        Some(b) => (0..3).map(|c| (samples[i].v[c] - b[c]).abs()).sum::<f64>() * samples[i].w,
        None => samples[i].v[best_c],
    };
    order.sort_by(|&a, &b| key(a).total_cmp(&key(b)));
    if top || bg.is_some() {
        order.reverse();
    }
    let take = (samples.len() / 20).max(1);
    let (mut cx, mut cy) = (0.0, 0.0);
    for &i in &order[..take] {
        cx += samples[i].x;
        cy += samples[i].y;
    }
    cx /= take as f64;
    cy /= take as f64;
    // An ellipse: axes along the pixels' own spread about the centre, reaching the furthest one.
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for s in samples {
        let (dx, dy) = (s.x - cx, s.y - cy);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (ct, st) = (theta.cos(), theta.sin());
    let uv: Vec<(f64, f64)> = samples
        .iter()
        .map(|s| {
            let (dx, dy) = (s.x - cx, s.y - cy);
            (dx * ct + dy * st, -dx * st + dy * ct)
        })
        .collect();
    let a = uv.iter().map(|p| p.0.abs()).fold(1e-3, f64::max);
    let b = uv.iter().map(|p| p.1.abs()).fold(1e-3, f64::max);
    let ds: Vec<f64> = uv.iter().map(|p| (p.0 / a).hypot(p.1 / b)).collect();
    let tmax = ds.iter().cloned().fold(1e-3, f64::max);
    let us: Vec<f64> = ds.iter().map(|&d| d / tmax).collect();
    let (stops, err, bad) = ramp(samples, &us, bg, true);
    let mut g = Gradient::new(GradientKind::Radial, stops);
    g.transform = Affine::translate((cx, cy)) * Affine::rotate(theta) * Affine::scale_non_uniform(a * tmax, b * tmax);
    Some((g, err, bad))
}

/// The gradient's stops for `us`. With `bg` (the colour of the region the group
/// sits on) the colour is one flat colour, the group's own extreme, and the ramp is
/// its opacity over `bg`: a haze that fades out into what is behind it.
fn ramp(samples: &[Sample], us: &[f64], bg: Option<[f64; 3]>, radial: bool) -> (Vec<GradientStop>, f64, f64) {
    let Some(bg) = bg else {
        let (v, err, bad) = fit_ramp(samples, us);
        return (to_stops(&v), err, bad);
    };
    // The colour furthest from the background, averaged over the furthest few.
    let dist = |s: &Sample| (0..3).map(|c| (s.v[c] - bg[c]).abs()).sum::<f64>();
    let mut order: Vec<usize> = (0..samples.len()).collect();
    order.sort_by(|&a, &b| dist(&samples[b]).total_cmp(&dist(&samples[a])));
    let take = (samples.len() / 20).max(1);
    let mut col = [0.0f64; 3];
    for &i in &order[..take] {
        for c in 0..3 {
            col[c] += samples[i].v[c] / take as f64;
        }
    }
    let axis: Vec<f64> = (0..3).map(|c| col[c] - bg[c]).collect();
    let norm = axis.iter().map(|a| a * a).sum::<f64>().max(1.0);
    // Each pixel's opacity: how far along bg -> colour it sits.
    let alpha_samples: Vec<Sample> = samples
        .iter()
        .map(|s| {
            let a = ((0..3).map(|c| (s.v[c] - bg[c]) * axis[c]).sum::<f64>() / norm).clamp(0.0, 1.0);
            Sample { x: s.x, y: s.y, v: [0.0, 0.0, 0.0, a * 255.0], w: 1.0, m: s.m }
        })
        .collect();
    let (mut v, _, _) = fit_ramp(&alpha_samples, us);
    if radial {
        // The glow's outer edge is clear.
        v[KNOTS - 1][3] = 0.0;
    }
    let stops: Vec<GradientStop> = v
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let q = |x: f64| x.round().clamp(0.0, 255.0) as u8;
            GradientStop::new(
                i as f64 / (KNOTS - 1) as f64,
                Color::from_rgba8(q(col[0]), q(col[1]), q(col[2]), q(k[3])),
            )
        })
        .collect();
    // Error of the composite over the background.
    let errs: Vec<f64> = samples
        .iter()
        .zip(us)
        .map(|(s, &u)| {
            let pos = u.clamp(0.0, 1.0) * (KNOTS - 1) as f64;
            let i = (pos.floor() as usize).min(KNOTS - 2);
            let f = pos - i as f64;
            let a = (v[i][3] * (1.0 - f) + v[i + 1][3] * f) / 255.0;
            let pred = [bg[0] * (1.0 - a) + col[0] * a, bg[1] * (1.0 - a) + col[1] * a, bg[2] * (1.0 - a) + col[2] * a, 255.0];
            sample_error(s, pred)
        })
        .collect();
    let n = errs.len().max(1) as f64;
    (stops, errs.iter().sum::<f64>() / n, errs.iter().filter(|&&e| e > BAD_ERROR).count() as f64 / n)
}

fn to_stops(v: &[[f64; 4]; KNOTS]) -> Vec<GradientStop> {
    v.iter()
        .enumerate()
        .map(|(i, c)| {
            let q = |x: f64| x.round().clamp(0.0, 255.0) as u8;
            GradientStop::new(i as f64 / (KNOTS - 1) as f64, Color::from_rgba8(q(c[0]), q(c[1]), q(c[2]), q(c[3])))
        })
        .collect()
}

/// Least-squares piecewise-linear ramp over `us` (0-1) with [`KNOTS`] knots, per channel,
/// and its mean error on the samples.
fn fit_ramp(samples: &[Sample], us: &[f64]) -> ([[f64; 4]; KNOTS], f64, f64) {
    let mut out = [[0.0f64; 4]; KNOTS];
    for c in 0..4 {
        let mut a = [[0.0f64; KNOTS]; KNOTS];
        let mut b = [0.0f64; KNOTS];
        let (mut sw, mut sv) = (0.0, 0.0);
        for (s, &u) in samples.iter().zip(us) {
            let wt = if c == 3 { 1.0 } else { s.w };
            let pos = u.clamp(0.0, 1.0) * (KNOTS - 1) as f64;
            let i = (pos.floor() as usize).min(KNOTS - 2);
            let f = pos - i as f64;
            let (b0, b1) = (1.0 - f, f);
            a[i][i] += wt * b0 * b0;
            a[i][i + 1] += wt * b0 * b1;
            a[i + 1][i] += wt * b0 * b1;
            a[i + 1][i + 1] += wt * b1 * b1;
            b[i] += wt * b0 * s.v[c];
            b[i + 1] += wt * b1 * s.v[c];
            sw += wt;
            sv += wt * s.v[c];
        }
        // A faint pull toward the mean keeps a knot nothing lands on sane.
        let mean = sv / sw.max(1e-9);
        let lam = 1e-3 * sw / KNOTS as f64 + 1e-9;
        for k in 0..KNOTS {
            a[k][k] += lam;
            b[k] += lam * mean;
        }
        let x = solve(a, b);
        for k in 0..KNOTS {
            out[k][c] = x[k].clamp(0.0, 255.0);
        }
    }
    let errs: Vec<f64> = samples
        .iter()
        .zip(us)
        .map(|(s, &u)| {
            let pos = u.clamp(0.0, 1.0) * (KNOTS - 1) as f64;
            let i = (pos.floor() as usize).min(KNOTS - 2);
            let f = pos - i as f64;
            let mut pred = [0.0; 4];
            for c in 0..4 {
                pred[c] = (out[i][c] * (1.0 - f) + out[i + 1][c] * f).round();
            }
            sample_error(s, pred)
        })
        .collect();
    let n = errs.len().max(1) as f64;
    let err = errs.iter().sum::<f64>() / n;
    let bad = errs.iter().filter(|&&e| e > BAD_ERROR).count() as f64 / n;
    (out, err, bad)
}

/// Gaussian elimination with partial pivoting.
fn solve(mut a: [[f64; KNOTS]; KNOTS], mut b: [f64; KNOTS]) -> [f64; KNOTS] {
    for col in 0..KNOTS {
        let piv = (col..KNOTS).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs())).unwrap_or(col);
        a.swap(col, piv);
        b.swap(col, piv);
        let d = a[col][col];
        if d.abs() < 1e-12 {
            continue;
        }
        for row in col + 1..KNOTS {
            let f = a[row][col] / d;
            for k in col..KNOTS {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; KNOTS];
    for row in (0..KNOTS).rev() {
        let mut s = b[row];
        for k in row + 1..KNOTS {
            s -= a[row][k] * x[k];
        }
        x[row] = if a[row][row].abs() < 1e-12 { 0.0 } else { s / a[row][row] };
    }
    x
}

/// A boundary between two regions is soft when the source steps by no more than this
/// (mean over the boundary, as [`step`]).
const SOFT_PAIR_STEP: f64 = 6.0;
/// A region is blurred when at least this share of its boundary is soft.
const SOFT_SHARE: f64 = 0.7;
/// The blur radius, as a share of the measured transition width.
const BLUR_PER_WIDTH: f64 = 0.8;
/// Blur radius limits, in picture pixels.
const BLUR_MIN_PX: f64 = 1.0;
const BLUR_MAX_PX: f64 = 24.0;
/// Most blurred shapes per trace (a blur is a boolean per band, built once per
/// object and then cached by the renderer).
const MAX_BLURRED: usize = 12;

/// Each region's blur radius in grid pixels (0 = sharp): regions (not in `skip`)
/// of which at least [`SOFT_SHARE`] of the boundary is soft in the source. The
/// radius is the transition width, the colour step between the two sides over the
/// source's own step across the boundary.
pub(crate) fn blur_radii(
    regions: &[(u8, Vec<usize>)],
    region_of: &[u32],
    pixels: &[u8],
    w: usize,
    h: usize,
    factor: f64,
    skip: &[bool],
) -> Vec<f64> {
    let n = regions.len();
    let mut out = vec![0.0f64; n];
    if n < 2 {
        return out;
    }
    let mean_of = |cells: &[usize]| -> [f64; 4] {
        let mut sum = [0.0f64; 4];
        for &at in cells {
            for c in 0..4 {
                sum[c] += pixels[at * 4 + c] as f64;
            }
        }
        let k = cells.len().max(1) as f64;
        [sum[0] / k, sum[1] / k, sum[2] / k, sum[3] / k]
    };
    let means: Vec<[f64; 4]> = regions.iter().map(|(_, cells)| mean_of(cells)).collect();
    let mut edges: HashMap<u64, (u32, f64)> = HashMap::new();
    let mut note = |a: u32, b: u32, i: usize, j: usize| {
        if a == b || a == u32::MAX || b == u32::MAX {
            return;
        }
        let key = ((a.min(b) as u64) << 32) | a.max(b) as u64;
        let e = edges.entry(key).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += step(&pixels[i * 4..i * 4 + 4], &pixels[j * 4..j * 4 + 4]);
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if x + 1 < w {
                note(region_of[i], region_of[i + 1], i, i + 1);
            }
            if y + 1 < h {
                note(region_of[i], region_of[i + w], i, i + w);
            }
        }
    }
    // Per region: all boundary, soft boundary, and the soft boundary's width (count-weighted).
    let mut total = vec![0u64; n];
    let mut soft_len = vec![0u64; n];
    let mut width_sum = vec![0.0f64; n];
    for (&key, &(count, sum)) in &edges {
        let (a, b) = ((key >> 32) as usize, (key & 0xffff_ffff) as usize);
        let mean_step = sum / count as f64;
        let d = (0..3).map(|c| (means[a][c] - means[b][c]).abs()).fold(0.0, f64::max).max((means[a][3] - means[b][3]).abs());
        let soft = count >= MIN_BOUNDARY && mean_step <= SOFT_PAIR_STEP;
        let width = (d / mean_step.max(0.25)).clamp(BLUR_MIN_PX, BLUR_MAX_PX);
        for r in [a, b] {
            total[r] += count as u64;
            if soft {
                soft_len[r] += count as u64;
                width_sum[r] += width * count as f64;
            }
        }
    }
    let min_px = MIN_GROUP_PX * factor * factor;
    let mut picked: Vec<(usize, u64)> = (0..n)
        .filter(|&r| {
            !skip[r] && total[r] > 0 && soft_len[r] as f64 >= SOFT_SHARE * total[r] as f64 && regions[r].1.len() as f64 >= min_px
        })
        .map(|r| (r, soft_len[r]))
        .collect();
    picked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    picked.truncate(MAX_BLURRED);
    for (r, _) in picked {
        let width = width_sum[r] / soft_len[r] as f64;
        out[r] = (width * BLUR_PER_WIDTH).max(BLUR_MIN_PX) * factor;
    }
    out
}
