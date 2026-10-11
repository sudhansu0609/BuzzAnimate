//! **Tracing a scan of a drawing into centreline strokes**, rather than the
//! filled regions [`crate::trace`] finds.
//!
//! A pencil line is not a region: it is ink laid down *along a path*, and the
//! honest trace of one is a stroke down its middle, not a long thin filled
//! blob. This module finds that middle.
//!
//! # The pipeline
//!
//! A. Composite the picture over white and threshold it to an ink/paper mask.
//! B. Clean speckle and pinhole noise off the mask.
//! C. Measure, at every pixel, the exact distance to the nearest paper.
//! D. Pull out anything too fat to be a line — a filled blob — and trace it
//!    as a mass instead of skeletonising it.
//! E. Thin what is left to a one-pixel skeleton (Zhang-Suen).
//! F. Turn the skeleton into a graph: nodes where lines meet or end, edges
//!    between them.
//! G. Prune whiskers too short to be real, and simplify pass-through nodes.
//! H. At a real junction, carry each line **through** rather than splitting
//!    it — a crossing reads as two strokes crossing, not four spokes.
//! I. Drop what is left too short to be worth keeping.
//! J. Measure each stroke's width along its length, smooth it, and fit a
//!    curve.
//! K. Build the shape — a stroke or a variable-width brush fill.
//! L. Optionally trace the paper the lines enclose, as fills tucked under
//!    the ink.
//!
//! Everything here is deterministic: no randomness, and no dependency on
//! iteration order that could change from one run to the next.

use crate::bucket::Grid;
use crate::object::ShapeData;
use crate::trace::{InkDetect, LineStyle, TraceOptions, TraceReport, outline};
use buzz_geom::{
    BezPath, BrushBudget, BrushProfile, Point, StrokeSample, TaperEnds, TipShape, Vec2,
    WidthResponse, catmull_rom, fluid_outline,
};
use peniko::Color;
use rayon::prelude::*;

/// Pixel counts above this are box-downsampled before tracing, and the
/// result scaled back up — see [`trace_lines`].
const BUDGET_PIXELS: usize = 6_000_000;
/// The most chains one trace will produce, however busy the picture.
const MAX_CHAINS: usize = 20_000;

/// **Trace `pixels` into centreline strokes**, in pixel space. The entry
/// point [`crate::trace::trace`] dispatches here for `TraceMode::LineArt`.
pub(crate) fn trace_lines(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> TraceReport {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return TraceReport {
            message: "There is nothing in that picture to trace".into(),
            ..Default::default()
        };
    }

    // **N. Budget.** Downsample a huge picture, trace at the smaller size,
    // and scale the result back up.
    let total = w * h;
    if total > BUDGET_PIXELS {
        let k = ((total as f64 / BUDGET_PIXELS as f64).sqrt()).ceil() as usize;
        let k = k.max(2);
        let (dw, dh, down) = box_downsample(w, h, pixels, k);
        let mut report = trace_core(dw, dh, &down, options);
        let scale = buzz_geom::Affine::scale(k as f64);
        for shape in &mut report.shapes {
            shape.path = scale * shape.path.clone();
            if let Some(s) = shape.stroke.as_mut() {
                s.width *= k as f64;
            }
        }
        return report;
    }

    trace_core(w, h, pixels, options)
}

/// Decide between LineArt and colour by measuring ink: the fraction of dark
/// pixels whose distance-transform radius is <= max_line_radius. More than 60%
/// of dark pixels thin means inked line art; otherwise colour art.
pub(crate) fn is_ink_drawing(w: usize, h: usize, pixels: &[u8], options: &TraceOptions) -> bool {
    let mut ink = ink_mask_with(w, h, pixels, options);
    clean(&mut ink, w, h, options.speckle.max(1));
    let total_dark = ink.iter().filter(|&&v| v).count();
    if total_dark == 0 {
        return false;
    }
    let r = edt(&ink, w, h);
    let max_radius = options.max_line_radius.max(0.5);
    let thin_dark = ink
        .iter()
        .enumerate()
        .filter(|&(i, is_ink)| *is_ink && r[i] <= max_radius)
        .count();
    (thin_dark as f64 / total_dark as f64) > 0.60
}

fn trace_core(w: usize, h: usize, pixels: &[u8], options: &TraceOptions) -> TraceReport {
    // With `ink_masses: false` the mass part of the ink is already stripped
    // here (see `traced_ink`), so only the strokes are left to skeletonise.
    let ink = traced_ink(w, h, pixels, options);

    if !ink.iter().any(|&v| v) {
        return TraceReport {
            message: "No ink found in that picture".into(),
            ..Default::default()
        };
    }

    let r = edt(&ink, w, h);

    // **D. Masses.** Anything too fat to be a line is traced as a filled
    // region instead, and excluded from the skeleton. (Not with
    // `ink_masses: false`: they are dropped from the ink above.)
    let max_radius = options.max_line_radius.max(0.5);
    let (masses, mass_pixels) = if options.ink_masses {
        find_masses(&ink, &r, w, h, max_radius, pixels, options)
    } else {
        (Vec::new(), Vec::new())
    };
    let mut skeleton_source = ink.clone();
    for &p in &mass_pixels {
        skeleton_source[p] = false;
    }

    // **E. Skeleton.**
    zhang_suen(&mut skeleton_source, w, h);
    staircase_cleanup(&mut skeleton_source, w, h);

    // **F. Graph, G. Prune, H. Continuation.**
    let (nodes, edges, mut closed) = skeleton_graph(&skeleton_source, w, h);
    let edges = prune(nodes.clone(), edges, &r, options.min_line_length.max(0.0));
    let mut chains = pair_junctions(&nodes, edges, &r);

    // Pruning and pairing can collapse a whole loop down to one edge whose
    // two ends are the same node — a ring with no real junction anywhere,
    // once the skeletonisation noise around it has been joined away. That is
    // a closed chain wearing an edge's clothes, not an open one.
    chains.retain(|c| {
        if c.len() > 2 && c[0].point == c[c.len() - 1].point {
            let mut loop_points = c.clone();
            loop_points.pop();
            closed.push(loop_points);
            false
        } else {
            true
        }
    });

    // **I. Drop short chains**, and **J/K build shapes**.
    let mut lines: Vec<(f64, ShapeData)> = Vec::new();
    for chain in chains {
        let length = arc_length(&chain);
        if length < options.min_line_length.max(0.0) {
            continue;
        }
        if let Some(shape) = chain_to_shape(&chain, false, &r, w, pixels, options) {
            lines.push((length, shape));
        }
    }
    for chain in closed {
        let length = arc_length(&chain);
        if let Some(shape) = chain_to_shape(&chain, true, &r, w, pixels, options) {
            lines.push((length, shape));
        }
    }
    // Longest first.
    lines.sort_by(|a, b| b.0.total_cmp(&a.0));
    let strokes = lines.len();

    // **L. Enclosed fills.**
    let mut fills: Vec<(usize, ShapeData)> = Vec::new();
    if options.fill_enclosed && !matches!(options.mode, crate::trace::TraceMode::Outline) {
        fills = enclosed_fills(&ink, w, h, pixels, &r, options.speckle.max(1), options);
    }
    fills.sort_by(|a, b| b.0.cmp(&a.0));
    let fill_count = fills.len();

    // **M. Order.** Enclosed fills, then masses, then lines, longest first.
    let mut shapes: Vec<ShapeData> = Vec::new();
    shapes.extend(fills.into_iter().map(|(_, s)| s));
    let mass_count = masses.len();
    shapes.extend(masses.into_iter().map(|(_, s)| s));
    shapes.extend(lines.into_iter().map(|(_, s)| s));

    let mut truncated = false;
    if shapes.len() > MAX_CHAINS {
        shapes.truncate(MAX_CHAINS);
        truncated = true;
    }

    let message = match (strokes, mass_count, fill_count) {
        (0, 0, 0) => "Nothing in that picture came out as a line".to_string(),
        _ => {
            let mut parts = Vec::new();
            if strokes > 0 {
                parts.push(format!("{strokes} lines"));
            }
            if mass_count > 0 {
                parts.push(format!("{mass_count} filled shapes"));
            }
            if fill_count > 0 {
                parts.push(format!("{fill_count} enclosed fills"));
            }
            let mut msg = format!("Traced {}", parts.join(", "));
            if truncated {
                msg.push_str(" \u{2014} stopped at the chain limit");
            }
            msg
        }
    };

    TraceReport {
        shapes,
        regions: mass_count,
        specks: 0,
        strokes,
        fills: fill_count,
        message,
    }
}

// ---------------------------------------------------------------------------
// A. Ink mask
// ---------------------------------------------------------------------------

/// Composite `pixels` over white, blur, and threshold into an ink/paper mask.
/// `true` is ink.
pub(crate) fn ink_mask(w: usize, h: usize, pixels: &[u8], ink_threshold: Option<u8>) -> Vec<bool> {
    let blurred = blurred_luma(w, h, pixels);
    let threshold = match ink_threshold {
        Some(t) => t as f64,
        None => otsu(&blurred) as f64,
    };

    blurred.iter().map(|&l| l < threshold).collect()
}

/// The ink mask `options.ink_detect` asks for. `Global(None)` defers to
/// `options.ink_threshold` (so a caller that only sets that keeps working).
pub(crate) fn ink_mask_with(w: usize, h: usize, pixels: &[u8], options: &TraceOptions) -> Vec<bool> {
    match options.ink_detect {
        InkDetect::Global(Some(t)) => ink_mask(w, h, pixels, Some(t)),
        InkDetect::Global(None) => ink_mask(w, h, pixels, options.ink_threshold),
        InkDetect::LocalContrast { ratio, max_luma, window, max_delta_e_to_line } => {
            let line = options.line_colour.map(|c| {
                let c = c.to_rgba8();
                crate::trace::srgb_to_lab([c.r, c.g, c.b])
            });
            ink_mask_local(w, h, pixels, ratio, max_luma, window, line.map(|l| (l, max_delta_e_to_line)))
        }
    }
}

/// **The ink this trace skeletonises**: the detected mask, cleaned of specks; with
/// `ink_masses: false` only the LINE components of it (T9e, see [`classify_ink`]:
/// compactness decides line vs mass, and `max_line_radius` is the upper bound on how
/// fat any line may be).
pub(crate) fn traced_ink(w: usize, h: usize, pixels: &[u8], options: &TraceOptions) -> Vec<bool> {
    if !options.ink_masses {
        return classify_ink(w, h, pixels, options).line;
    }
    let mut ink = ink_mask_with(w, h, pixels, options);
    clean(&mut ink, w, h, options.speckle.max(1));
    ink
}

/// A component is a MASS when its `area / skeleton length` (its mean width along the
/// skeleton) is more than this many times the typical pen width.
const MASS_WIDTH_FACTOR: f64 = 2.5;
/// The typical pen width (px, source resolution) is clamped into this range.
const LINE_WIDTH_RANGE: (f64, f64) = (1.5, 6.0);

/// The ink of a picture split by compactness (T9e); see [`classify_ink`].
pub(crate) struct ClassifiedInk {
    /// The ink of the line components (no mass, nothing fatter than `max_line_radius`).
    pub(crate) line: Vec<bool>,
    /// The rest of the ink: the mass components, and the fat cores cut out of lines.
    pub(crate) mass: Vec<bool>,
    /// The typical pen width in px, measured off the picture.
    pub(crate) line_width_typical: f64,
}

/// **Line vs mass by compactness** (T9e). The detected ink (cleaned of specks) is
/// split into 8-connected components. For each: `area`, and `skeleton_len` = the
/// pixels of its Zhang-Suen skeleton (the whole mask is thinned once, the skeleton
/// pixels counted by component label). `line_width_typical` = the median over
/// components of `2 x` the median distance-transform radius of the component's pixels,
/// clamped to [1.5, 6]: the artist's pen. A component is a MASS iff
/// `area / skeleton_len > 2.5 x line_width_typical` (a component whose skeleton is under
/// 3 px uses `area / max(bbox_w, bbox_h)`); a line stays a line even where it is
/// pressed thick. A 12 px brow, a pupil, hair and a shoe are masses; a 3 px outline is
/// a line however long, however it loops. `max_line_radius` is only the upper bound
/// on a line's thickness: the cores deeper than it (see [`strip_masses`]) are cut out
/// of the line components and count as mass.
pub(crate) fn classify_ink(w: usize, h: usize, pixels: &[u8], options: &TraceOptions) -> ClassifiedInk {
    let speckle = options.speckle.max(1);
    let max_radius = options.max_line_radius.max(0.5);
    let mut ink = ink_mask_with(w, h, pixels, options);
    clean(&mut ink, w, h, speckle);
    fill_thin_slits(&mut ink, w, h, SLIT_HALF_WIDTH_FRACTION * max_radius);
    classify_components(&ink, w, h, max_radius, speckle)
}

/// An enclosed paper slit is filled when no pixel of it is further than this fraction
/// of `max_line_radius` from the ink (so its width is under twice that).
const SLIT_HALF_WIDTH_FRACTION: f64 = 0.35;

/// **Fill the thin paper slits between fused lines** (T9e, the hand loops). Where several
/// lines run a pen-width apart (the folded fingers of a hand) the ink mask is one thick
/// band pierced by long thin slits of paper, and the skeleton of that band is a tangle
/// of loops round the slits, which the brush turns into a scribble of crossing ribbons.
/// Every enclosed paper component (4-connected, not reaching the picture's edge) whose
/// widest point is within `max_half_width` of the ink is turned to ink: the band is one
/// solid line again. A real ring (an eye outline, a bangle) encloses far more than a slit
/// and is untouched.
fn fill_thin_slits(ink: &mut [bool], w: usize, h: usize, max_half_width: f64) {
    if w == 0 || h == 0 {
        return;
    }
    // Paper reachable from the border (4-connected) is outside.
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        for y in [0, h - 1] {
            let i = y * w + x;
            if !ink[i] && !outside[i] {
                outside[i] = true;
                stack.push(i);
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            let i = y * w + x;
            if !ink[i] && !outside[i] {
                outside[i] = true;
                stack.push(i);
            }
        }
    }
    while let Some(at) = stack.pop() {
        for (nx, ny) in neighbours4(at % w, at / w, w, h) {
            let n = ny * w + nx;
            if !ink[n] && !outside[n] {
                outside[n] = true;
                stack.push(n);
            }
        }
    }
    if (0..w * h).all(|i| ink[i] || outside[i]) {
        return;
    }
    let to_ink = distance_to_set(ink, w, h);
    let mut visited = vec![false; w * h];
    for start in 0..w * h {
        if ink[start] || outside[start] || visited[start] {
            continue;
        }
        let comp = flood(ink, w, h, start, &mut visited, false, 4);
        let widest = comp.iter().map(|&p| to_ink[p]).fold(0.0f64, f64::max);
        if widest <= max_half_width {
            for &p in &comp {
                ink[p] = true;
            }
        }
    }
}

fn classify_components(ink: &[bool], w: usize, h: usize, max_radius: f64, speckle: usize) -> ClassifiedInk {
    let n = w * h;
    let r = edt(ink, w, h);
    // Label the 8-connected components; per component its area, bounding box and
    // pen width (twice the median radius).
    let mut label = vec![u32::MAX; n];
    let mut visited = vec![false; n];
    // (area, bbox_w, bbox_h, pen width)
    let mut stats: Vec<(usize, usize, usize, f64)> = Vec::new();
    for start in 0..n {
        if !ink[start] || visited[start] {
            continue;
        }
        let comp = flood(ink, w, h, start, &mut visited, true, 8);
        let id = stats.len() as u32;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        let mut radii = Vec::with_capacity(comp.len());
        for &p in &comp {
            label[p] = id;
            let (x, y) = (p % w, p / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            radii.push(r[p]);
        }
        let mid = radii.len() / 2;
        let (_, median, _) = radii.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
        stats.push((comp.len(), x1 - x0 + 1, y1 - y0 + 1, 2.0 * *median));
    }
    let widths: Vec<f64> = stats.iter().map(|s| s.3).collect();
    let line_width_typical = median_of(&widths).clamp(LINE_WIDTH_RANGE.0, LINE_WIDTH_RANGE.1);

    // The skeleton of the whole mask, counted per component.
    let mut skeleton = ink.to_vec();
    zhang_suen(&mut skeleton, w, h);
    staircase_cleanup(&mut skeleton, w, h);
    let mut skeleton_len = vec![0usize; stats.len()];
    for p in 0..n {
        if skeleton[p] && label[p] != u32::MAX {
            skeleton_len[label[p] as usize] += 1;
        }
    }
    let is_mass: Vec<bool> = stats
        .iter()
        .zip(&skeleton_len)
        .map(|(&(area, bw, bh, _), &len)| {
            let ratio = if len < 3 { area as f64 / bw.max(bh) as f64 } else { area as f64 / len as f64 };
            ratio > MASS_WIDTH_FACTOR * line_width_typical
        })
        .collect();

    let mut line: Vec<bool> = (0..n).map(|i| ink[i] && !is_mass[label[i] as usize]).collect();
    strip_masses(&mut line, w, h, max_radius);
    clean(&mut line, w, h, speckle);
    for i in 0..n {
        line[i] &= ink[i];
    }
    let mass: Vec<bool> = (0..n).map(|i| ink[i] && !line[i]).collect();
    ClassifiedInk { line, mass, line_width_typical }
}

/// **Local-contrast ink**: a pixel is ink when it is opaque (alpha > 128),
/// its 3x3-blurred luma (the same blur [`ink_mask`] uses) is below
/// `ratio * background` and below `max_luma`, where `background` is the
/// 80th-percentile blurred luma of the OPAQUE pixels in the `window` x `window`
/// box centred on it (the box is clipped at the picture's edge; an even `window`
/// is widened by one so the box has a centre).
///
/// The percentile is exact: a 256-bin histogram of the box, slid along each row
/// (add the entering column, remove the leaving one), one row per rayon task —
/// not a separable approximation. Deterministic: each row is computed alone.
///
/// `line` = `(line colour in CIELAB, max ΔE)`: the test above runs first; then each
/// 8-connected component of the candidate mask is kept iff its MEDIAN CIELAB ΔE to
/// the line colour is within that limit (a saturated dark red line is a fill, rejected
/// whole; a brown line keeps its anti-aliased edge). `None` skips the test.
pub(crate) fn ink_mask_local(
    w: usize,
    h: usize,
    pixels: &[u8],
    ratio: f32,
    max_luma: u8,
    window: usize,
    line: Option<([f64; 3], f64)>,
) -> Vec<bool> {
    let blurred = blurred_luma(w, h, pixels);
    let bins: Vec<u8> = blurred.iter().map(|&l| l.round().clamp(0.0, 255.0) as u8).collect();
    let opaque: Vec<bool> = (0..w * h).map(|i| pixels[i * 4 + 3] > 128).collect();
    let mut ink = vec![false; w * h];
    if w == 0 || h == 0 {
        return ink;
    }
    // Every opaque pixel's own window always contains itself, so its
    // background is never the "no opaque pixel in this window" sentinel —
    // the `count > 0` guard the old inline loop carried is implied here too.
    let background = local_background(w, h, &bins, &opaque, window, 0.8);
    for i in 0..w * h {
        if opaque[i] && blurred[i] < max_luma as f64 {
            ink[i] = blurred[i] < ratio as f64 * background[i] as f64;
        }
    }
    if let Some((line_lab, max_de)) = line {
        if max_de.is_finite() {
            keep_ink_coloured_lines(&mut ink, w, h, pixels, line_lab, max_de);
        }
    }
    ink
}

/// **Local background luma**: the `percentile`-th percentile luma of the
/// opaque pixels in a `window`×`window` box centred on each pixel (clamped at
/// the picture edge), by a per-row sliding histogram. `255` where the window
/// has no opaque pixel at all. `bins` is the per-pixel luma already rounded
/// to `0..=255`; the caller decides whether that luma was blurred first.
pub(crate) fn local_background(
    w: usize,
    h: usize,
    bins: &[u8],
    opaque: &[bool],
    window: usize,
    percentile: f64,
) -> Vec<u8> {
    let half = window.max(1) / 2;
    let mut background = vec![255u8; w * h];
    if w == 0 || h == 0 {
        return background;
    }
    background.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (y0, y1) = (y.saturating_sub(half), (y + half).min(h - 1));
        let mut hist = [0u32; 256];
        let mut count = 0u32;
        let add = |x: usize, hist: &mut [u32; 256], count: &mut u32| {
            for yy in y0..=y1 {
                let i = yy * w + x;
                if opaque[i] {
                    hist[bins[i] as usize] += 1;
                    *count += 1;
                }
            }
        };
        for x in 0..=half.min(w - 1) {
            add(x, &mut hist, &mut count);
        }
        for x in 0..w {
            if count > 0 {
                // The smallest luma bin whose cumulative share reaches `percentile`.
                let target = (percentile * count as f64).ceil() as u32;
                let (mut cum, mut bg) = (0u32, 255usize);
                for (b, &n) in hist.iter().enumerate() {
                    cum += n;
                    if cum >= target {
                        bg = b;
                        break;
                    }
                }
                row[x] = bg as u8;
            }
            // Slide the box one column right.
            if x + half + 1 < w {
                add(x + half + 1, &mut hist, &mut count);
            }
            if x >= half {
                let leaving = x - half;
                for yy in y0..=y1 {
                    let j = yy * w + leaving;
                    if opaque[j] {
                        hist[bins[j] as usize] -= 1;
                        count -= 1;
                    }
                }
            }
        }
    });
    background
}

/// **Ink colour per connected line** (T9d): labels the 8-connected components of the
/// candidate `ink` mask and keeps a component whole iff the MEDIAN CIELAB ΔE of its
/// pixels to `line_lab` is at most `max_de` (a component of fewer than 4 pixels uses the
/// mean). A brown line keeps its anti-aliased edge pixels (they drift from the line
/// colour one by one, but the line as a whole is on it); a red line is rejected whole.
fn keep_ink_coloured_lines(ink: &mut [bool], w: usize, h: usize, pixels: &[u8], line_lab: [f64; 3], max_de: f64) {
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let mut component: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !ink[start] || seen[start] {
            continue;
        }
        component.clear();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            component.push(i);
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if ink[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        let mut des: Vec<f64> = component
            .iter()
            .map(|&i| {
                let lab = crate::trace::srgb_to_lab([pixels[i * 4], pixels[i * 4 + 1], pixels[i * 4 + 2]]);
                crate::trace::delta_e(lab, line_lab)
            })
            .collect();
        let judged = if des.len() < 4 {
            des.iter().sum::<f64>() / des.len() as f64
        } else {
            let mid = des.len() / 2;
            let (_, m, _) = des.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
            *m
        };
        if judged > max_de {
            for &i in &component {
                ink[i] = false;
            }
        }
    }
}

/// How much further than `max_radius` [`strip_masses`] dilates the deep core of
/// a mass, in pixels: the rounded corners an exact opening leaves on a square
/// mass are slivers up to `0.41 * max_radius` thick; this slack thins them under
/// the speckle size so the caller's clean removes them.
const MASS_SLACK: f64 = 1.5;

/// **Remove the masses from `ink`**: every pixel within `max_radius + MASS_SLACK`
/// of a pixel deeper than `max_radius` inside the ink — the morphological opening
/// of the ink by a disc of that radius, slightly generous — is cleared. What
/// survives is what a disc that size cannot sit in: strokes and outlines (plus a
/// stray sliver hugging a mass's corner, which the caller's speckle clean takes
/// off).
fn strip_masses(ink: &mut [bool], w: usize, h: usize, max_radius: f64) {
    let r = edt(ink, w, h);
    let core: Vec<bool> = r.iter().map(|&d| d > max_radius).collect();
    if !core.iter().any(|&c| c) {
        return;
    }
    let near_core = distance_to_set(&core, w, h);
    for i in 0..w * h {
        if ink[i] && near_core[i] <= max_radius + MASS_SLACK {
            ink[i] = false;
        }
    }
}

/// Exact distance from every pixel to the nearest `true` pixel of `set`
/// (outside the picture is NOT in the set, unlike [`edt`]'s paper border).
/// Every distance is huge when `set` is empty.
fn distance_to_set(set: &[bool], w: usize, h: usize) -> Vec<f64> {
    let mut tmp = vec![0.0f64; w * h];
    let mut col = vec![0.0f64; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = if set[y * w + x] { 0.0 } else { EDT_INF };
        }
        let d = dt_1d(&col);
        for y in 0..h {
            tmp[y * w + x] = d[y];
        }
    }
    let mut out = vec![0.0f64; w * h];
    let mut row = vec![0.0f64; w];
    for y in 0..h {
        row.copy_from_slice(&tmp[y * w..(y + 1) * w]);
        let d = dt_1d(&row);
        for x in 0..w {
            out[y * w + x] = d[x].sqrt();
        }
    }
    out
}

/// The luma threshold [`ink_mask`] finds by itself (Otsu's method over the
/// blurred, composited-over-white luma) when no threshold is given.
pub(crate) fn auto_ink_threshold(w: usize, h: usize, pixels: &[u8]) -> u8 {
    otsu(&blurred_luma(w, h, pixels))
}

/// `pixels` composited over white, as luma, 3x3-blurred.
fn blurred_luma(w: usize, h: usize, pixels: &[u8]) -> Vec<f64> {
    let mut luma = vec![255.0f64; w * h];
    for i in 0..w * h {
        let px = &pixels[i * 4..i * 4 + 4];
        let a = px[3] as f64 / 255.0;
        let l = 0.299 * px[0] as f64 + 0.587 * px[1] as f64 + 0.114 * px[2] as f64;
        luma[i] = l * a + 255.0 * (1.0 - a);
    }

    // 3x3 blur, [1,2,1]⊗[1,2,1]/16, replicating the border.
    let at = |x: i64, y: i64| -> f64 {
        let cx = x.clamp(0, w as i64 - 1) as usize;
        let cy = y.clamp(0, h as i64 - 1) as usize;
        luma[cy * w + cx]
    };
    let mut blurred = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as i64, y as i64);
            let sum = at(xi - 1, yi - 1)
                + 2.0 * at(xi, yi - 1)
                + at(xi + 1, yi - 1)
                + 2.0 * at(xi - 1, yi)
                + 4.0 * at(xi, yi)
                + 2.0 * at(xi + 1, yi)
                + at(xi - 1, yi + 1)
                + 2.0 * at(xi, yi + 1)
                + at(xi + 1, yi + 1);
            blurred[y * w + x] = sum / 16.0;
        }
    }
    blurred
}

/// Otsu's method: the threshold that best splits a 256-bin histogram into
/// two classes. `128` if the histogram is degenerate (every sample the same).
fn otsu(values: &[f64]) -> u8 {
    let mut hist = [0u32; 256];
    for &v in values {
        let bin = v.round().clamp(0.0, 255.0) as usize;
        hist[bin] += 1;
    }
    let total: u32 = hist.iter().sum();
    if total == 0 {
        return 128;
    }
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &c)| i as f64 * c as f64)
        .sum();

    let mut sum_bg = 0.0f64;
    let mut weight_bg = 0u32;
    let mut best_t = 0u8;
    let mut best_variance = -1.0f64;
    let mut nonzero_bins = 0usize;
    for &c in &hist {
        if c > 0 {
            nonzero_bins += 1;
        }
    }
    if nonzero_bins <= 1 {
        return 128;
    }

    for t in 0..255usize {
        weight_bg += hist[t];
        if weight_bg == 0 {
            continue;
        }
        let weight_fg = total - weight_bg;
        if weight_fg == 0 {
            break;
        }
        sum_bg += t as f64 * hist[t] as f64;
        let mean_bg = sum_bg / weight_bg as f64;
        let mean_fg = (sum_all - sum_bg) / weight_fg as f64;
        let variance =
            weight_bg as f64 * weight_fg as f64 * (mean_bg - mean_fg) * (mean_bg - mean_fg);
        if variance > best_variance {
            best_variance = variance;
            best_t = (t + 1).min(255) as u8;
        }
    }
    best_t
}

// ---------------------------------------------------------------------------
// B. Clean
// ---------------------------------------------------------------------------

/// Drop ink specks smaller than `speckle`, and fill paper pinholes of 4px or
/// less fully enclosed by ink.
pub(crate) fn clean(ink: &mut [bool], w: usize, h: usize, speckle: usize) {
    // Small ink components become paper.
    let mut visited = vec![false; w * h];
    for start in 0..w * h {
        if !ink[start] || visited[start] {
            continue;
        }
        let comp = flood(ink, w, h, start, &mut visited, true, 8);
        if comp.len() < speckle {
            for &p in &comp {
                ink[p] = false;
            }
        }
    }

    // Paper reachable from the border (4-connected) is "outside"; anything
    // else is enclosed.
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        for y in [0, h - 1] {
            let idx = y * w + x;
            if !ink[idx] && !outside[idx] {
                outside[idx] = true;
                stack.push(idx);
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            let idx = y * w + x;
            if !ink[idx] && !outside[idx] {
                outside[idx] = true;
                stack.push(idx);
            }
        }
    }
    while let Some(at) = stack.pop() {
        let (x, y) = (at % w, at / w);
        for (nx, ny) in neighbours4(x, y, w, h) {
            let n = ny * w + nx;
            if !ink[n] && !outside[n] {
                outside[n] = true;
                stack.push(n);
            }
        }
    }

    let mut visited2 = vec![false; w * h];
    for start in 0..w * h {
        if ink[start] || outside[start] || visited2[start] {
            continue;
        }
        let comp = flood(ink, w, h, start, &mut visited2, false, 4);
        if comp.len() <= 4 {
            for &p in &comp {
                ink[p] = true;
            }
        }
    }
}

fn neighbours4(x: usize, y: usize, w: usize, h: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(4);
    if x > 0 {
        out.push((x - 1, y));
    }
    if x + 1 < w {
        out.push((x + 1, y));
    }
    if y > 0 {
        out.push((x, y - 1));
    }
    if y + 1 < h {
        out.push((x, y + 1));
    }
    out
}

fn neighbours8(x: usize, y: usize, w: usize, h: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(8);
    let (xi, yi) = (x as i64, y as i64);
    for dy in -1..=1i64 {
        for dx in -1..=1i64 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (xi + dx, yi + dy);
            if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                out.push((nx as usize, ny as usize));
            }
        }
    }
    out
}

/// Flood-fill the component of `start`, over pixels where `mask[p] == want`.
fn flood(
    mask: &[bool],
    w: usize,
    h: usize,
    start: usize,
    visited: &mut [bool],
    want: bool,
    connectivity: u8,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![start];
    visited[start] = true;
    while let Some(at) = stack.pop() {
        out.push(at);
        let (x, y) = (at % w, at / w);
        let neighbours = if connectivity == 8 {
            neighbours8(x, y, w, h)
        } else {
            neighbours4(x, y, w, h)
        };
        for (nx, ny) in neighbours {
            let n = ny * w + nx;
            if !visited[n] && mask[n] == want {
                visited[n] = true;
                stack.push(n);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// C. Exact Euclidean distance transform
// ---------------------------------------------------------------------------

const EDT_INF: f64 = 1e18;

/// Exact distance from every pixel to the nearest paper (non-ink) pixel,
/// treating outside the image as paper too (Felzenszwalb & Huttenlocher).
pub(crate) fn edt(ink: &[bool], w: usize, h: usize) -> Vec<f64> {
    let pw = w + 2;
    let ph = h + 2;
    let mut g = vec![EDT_INF; pw * ph];
    for y in 0..h {
        for x in 0..w {
            let idx = (y + 1) * pw + (x + 1);
            g[idx] = if ink[y * w + x] { EDT_INF } else { 0.0 };
        }
    }
    for x in 0..pw {
        g[x] = 0.0;
        g[(ph - 1) * pw + x] = 0.0;
    }
    for y in 0..ph {
        g[y * pw] = 0.0;
        g[y * pw + pw - 1] = 0.0;
    }

    let mut tmp = vec![0.0; pw * ph];
    let mut col = vec![0.0; ph];
    for x in 0..pw {
        for y in 0..ph {
            col[y] = g[y * pw + x];
        }
        let d = dt_1d(&col);
        for y in 0..ph {
            tmp[y * pw + x] = d[y];
        }
    }
    let mut out = vec![0.0; pw * ph];
    let mut row = vec![0.0; pw];
    for y in 0..ph {
        for x in 0..pw {
            row[x] = tmp[y * pw + x];
        }
        let d = dt_1d(&row);
        for x in 0..pw {
            out[y * pw + x] = d[x];
        }
    }

    let mut r = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            r[y * w + x] = out[(y + 1) * pw + (x + 1)].sqrt();
        }
    }
    r
}

/// One dimension of the exact distance transform of a sampled function, by
/// the lower-envelope-of-parabolas construction.
pub(crate) fn dt_1d(f: &[f64]) -> Vec<f64> {
    let n = f.len();
    let mut d = vec![0.0; n];
    if n == 0 {
        return d;
    }
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, slot) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        let dx = q as f64 - p as f64;
        *slot = dx * dx + f[p];
    }
    d
}

// ---------------------------------------------------------------------------
// D. Masses
// ---------------------------------------------------------------------------

fn find_masses(
    ink: &[bool],
    r: &[f64],
    w: usize,
    h: usize,
    max_radius: f64,
    pixels: &[u8],
    options: &TraceOptions,
) -> (Vec<(usize, ShapeData)>, Vec<usize>) {
    let mut visited = vec![false; w * h];
    let mut masses = Vec::new();
    let mut mass_pixels = Vec::new();
    let mut scratch = Grid::new(w, h);
    for start in 0..w * h {
        if !ink[start] || visited[start] {
            continue;
        }
        let comp = flood(ink, w, h, start, &mut visited, true, 8);
        let max_r = comp.iter().fold(0.0f64, |m, &p| m.max(r[p]));
        if max_r <= max_radius {
            continue;
        }
        for &p in &comp {
            scratch.set(p % w, p / w);
        }
        let path = outline(&scratch, options);
        for &p in &comp {
            scratch.unset(p % w, p / w);
        }
        if let Some(path) = path {
            let colour = mean_colour(pixels, &comp);
            masses.push((comp.len(), ShapeData::filled(path, colour)));
        }
        mass_pixels.extend(comp);
    }
    (masses, mass_pixels)
}

fn mean_colour(pixels: &[u8], indices: &[usize]) -> Color {
    if indices.is_empty() {
        return Color::BLACK;
    }
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for &i in indices {
        let px = &pixels[i * 4..i * 4 + 4];
        r += px[0] as u64;
        g += px[1] as u64;
        b += px[2] as u64;
    }
    let n = indices.len() as u64;
    Color::from_rgb8((r / n) as u8, (g / n) as u8, (b / n) as u8)
}

// ---------------------------------------------------------------------------
// E. Skeleton
// ---------------------------------------------------------------------------

fn zhang_suen(ink: &mut [bool], w: usize, h: usize) {
    let max_iters = w.max(h) / 2 + 1;
    // P2..P9, clockwise from north.
    const OFFSETS: [(i64, i64); 8] = [
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
    ];
    let get = |ink: &[bool], x: i64, y: i64| -> bool {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            false
        } else {
            ink[y as usize * w + x as usize]
        }
    };

    for _ in 0..max_iters {
        let mut changed = false;
        for sub in 0..2 {
            let mut to_remove = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    let idx = y * w + x;
                    if !ink[idx] {
                        continue;
                    }
                    let (xi, yi) = (x as i64, y as i64);
                    let p: Vec<bool> = OFFSETS.iter().map(|&(dx, dy)| get(ink, xi + dx, yi + dy)).collect();
                    let b = p.iter().filter(|&&v| v).count();
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let mut a = 0;
                    for i in 0..8 {
                        if !p[i] && p[(i + 1) % 8] {
                            a += 1;
                        }
                    }
                    if a != 1 {
                        continue;
                    }
                    let (p2, p4, p6, p8) = (p[0], p[2], p[4], p[6]);
                    if sub == 0 {
                        if p2 && p4 && p6 {
                            continue;
                        }
                        if p4 && p6 && p8 {
                            continue;
                        }
                    } else {
                        if p2 && p4 && p8 {
                            continue;
                        }
                        if p2 && p6 && p8 {
                            continue;
                        }
                    }
                    to_remove.push(idx);
                }
            }
            if !to_remove.is_empty() {
                changed = true;
                for idx in to_remove {
                    ink[idx] = false;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// **Straighten what Zhang-Suen leaves double-wide.**
///
/// Zhang-Suen's own removal rule deliberately keeps a pixel that would cut a
/// diagonal-only bridge, which is correct for not severing the skeleton but
/// leaves exactly the 2px-wide staircases this cleans up: real topological
/// junctions (a true fork) have more than one transition around their ring of
/// neighbours and are never touched, so this cannot fork or sever a line —
/// only thin what is genuinely still doubled. Iterates to a fixed point,
/// capped the same way [`zhang_suen`] is.
fn staircase_cleanup(ink: &mut [bool], w: usize, h: usize) {
    const OFFSETS: [(i64, i64); 8] = [
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
    ];
    let get = |ink: &[bool], x: i64, y: i64| -> bool {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            false
        } else {
            ink[y as usize * w + x as usize]
        }
    };

    let max_iters = w.max(h) / 2 + 1;
    for _ in 0..max_iters {
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                if !ink[idx] {
                    continue;
                }
                let (xi, yi) = (x as i64, y as i64);
                let p: Vec<bool> = OFFSETS.iter().map(|&(dx, dy)| get(ink, xi + dx, yi + dy)).collect();
                let b = p.iter().filter(|&&v| v).count();
                if !(2..=6).contains(&b) {
                    continue;
                }
                let mut a = 0;
                for i in 0..8 {
                    if !p[i] && p[(i + 1) % 8] {
                        a += 1;
                    }
                }
                // A single 0-to-1 transition around the ring is what makes a
                // point safe to remove without disconnecting or forking its
                // neighbours — the same simple-point test Zhang-Suen itself
                // uses, without the extra bridge-preserving conditions that
                // are exactly what left this pixel behind.
                if a == 1 {
                    ink[idx] = false;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// F. Graph
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct SkelPoint {
    point: Point,
    pixel: usize,
}

#[derive(Debug, Clone)]
struct Node {
    pos: Point,
    pixel: usize,
}

#[derive(Debug, Clone)]
struct WorkEdge {
    points: Vec<SkelPoint>,
    node_a: usize,
    node_b: usize,
}

fn pixel_degree(skeleton: &[bool], w: usize, h: usize) -> Vec<u8> {
    let mut degree = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            if !skeleton[y * w + x] {
                continue;
            }
            let d = neighbours8(x, y, w, h)
                .into_iter()
                .filter(|&(nx, ny)| skeleton[ny * w + nx])
                .count();
            degree[y * w + x] = d as u8;
        }
    }
    degree
}

fn skeleton_graph(
    skeleton: &[bool],
    w: usize,
    h: usize,
) -> (Vec<Node>, Vec<WorkEdge>, Vec<Vec<SkelPoint>>) {
    let degree = pixel_degree(skeleton, w, h);
    let mut node_of: Vec<Option<usize>> = vec![None; w * h];
    let mut nodes: Vec<Node> = Vec::new();

    // Junction clusters.
    let mut visited = vec![false; w * h];
    for start in 0..w * h {
        if !skeleton[start] || visited[start] || degree[start] < 3 {
            continue;
        }
        let mut stack = vec![start];
        visited[start] = true;
        let mut members = Vec::new();
        while let Some(at) = stack.pop() {
            members.push(at);
            let (x, y) = (at % w, at / w);
            for (nx, ny) in neighbours8(x, y, w, h) {
                let n = ny * w + nx;
                if !visited[n] && skeleton[n] && degree[n] >= 3 {
                    visited[n] = true;
                    stack.push(n);
                }
            }
        }
        let (mut sx, mut sy) = (0.0, 0.0);
        for &p in &members {
            sx += (p % w) as f64 + 0.5;
            sy += (p / w) as f64 + 0.5;
        }
        let n = members.len() as f64;
        sx /= n;
        sy /= n;
        let id = nodes.len();
        nodes.push(Node {
            pos: Point::new(sx, sy),
            pixel: members[0],
        });
        for &p in &members {
            node_of[p] = Some(id);
        }
    }

    // Endpoints, each a singleton node.
    for i in 0..w * h {
        if skeleton[i] && degree[i] == 1 && node_of[i].is_none() {
            let id = nodes.len();
            let (x, y) = (i % w, i / w);
            nodes.push(Node {
                pos: Point::new(x as f64 + 0.5, y as f64 + 0.5),
                pixel: i,
            });
            node_of[i] = Some(id);
        }
    }

    // Walk edges from every node.
    let mut edges: Vec<WorkEdge> = Vec::new();
    let mut edge_consumed = vec![false; w * h]; // degree-2 pixels already walked
    let mut trivial_seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();

    for ni in 0..nodes.len() {
        let member_pixels: Vec<usize> = (0..w * h).filter(|&p| node_of[p] == Some(ni)).collect();
        for &mp in &member_pixels {
            let (x, y) = (mp % w, mp / w);
            for (nx, ny) in neighbours8(x, y, w, h) {
                let np = ny * w + nx;
                if !skeleton[np] || node_of[np] == Some(ni) {
                    continue;
                }
                if let Some(other) = node_of[np] {
                    // Trivial edge: adjacent nodes, no interior pixels.
                    let key = (mp.min(np), mp.max(np));
                    if trivial_seen.contains(&key) {
                        continue;
                    }
                    trivial_seen.insert(key);
                    edges.push(WorkEdge {
                        points: vec![
                            SkelPoint { point: nodes[ni].pos, pixel: mp },
                            SkelPoint { point: nodes[other].pos, pixel: np },
                        ],
                        node_a: ni,
                        node_b: other,
                    });
                    continue;
                }
                if edge_consumed[np] {
                    continue;
                }
                // Walk through degree-2 pixels to the next node. The path
                // starts at the *node's* position (its centroid, for a
                // junction cluster), not at the member pixel that happened to
                // have this neighbour, or a multi-pixel cluster's edges would
                // each start from a different point on its own rim.
                let mut path = vec![SkelPoint { point: nodes[ni].pos, pixel: mp }];
                let mut prev = mp;
                let mut cur = np;
                let end_node;
                loop {
                    edge_consumed[cur] = true;
                    let (cx, cy) = (cur % w, cur / w);
                    path.push(SkelPoint {
                        point: Point::new(cx as f64 + 0.5, cy as f64 + 0.5),
                        pixel: cur,
                    });
                    if let Some(node) = node_of[cur] {
                        end_node = Some(node);
                        break;
                    }
                    let next = neighbours8(cx, cy, w, h)
                        .into_iter()
                        .map(|(nx2, ny2)| ny2 * w + nx2)
                        .find(|&p| skeleton[p] && p != prev);
                    match next {
                        Some(p) => {
                            prev = cur;
                            cur = p;
                        }
                        None => {
                            end_node = None;
                            break;
                        }
                    }
                }
                if let Some(other) = end_node {
                    path.push(SkelPoint { point: nodes[other].pos, pixel: nodes[other].pixel });
                    edges.push(WorkEdge { points: path, node_a: ni, node_b: other });
                }
            }
        }
    }

    // Leftover degree-2 pixels with no node anywhere on their component: pure
    // cycles.
    let mut cycle_visited = vec![false; w * h];
    let mut closed_chains: Vec<Vec<SkelPoint>> = Vec::new();
    for start in 0..w * h {
        if !skeleton[start]
            || degree[start] != 2
            || node_of[start].is_some()
            || edge_consumed[start]
            || cycle_visited[start]
        {
            continue;
        }
        let mut chain = Vec::new();
        let (sx, sy) = (start % w, start / w);
        chain.push(SkelPoint { point: Point::new(sx as f64 + 0.5, sy as f64 + 0.5), pixel: start });
        cycle_visited[start] = true;
        let first_neighbour = neighbours8(sx, sy, w, h)
            .into_iter()
            .map(|(nx, ny)| ny * w + nx)
            .find(|&p| skeleton[p]);
        let Some(mut cur) = first_neighbour else { continue };
        let mut prev = start;
        loop {
            if cur == start {
                break;
            }
            if cycle_visited[cur] {
                break;
            }
            let (cx, cy) = (cur % w, cur / w);
            chain.push(SkelPoint { point: Point::new(cx as f64 + 0.5, cy as f64 + 0.5), pixel: cur });
            cycle_visited[cur] = true;
            let next = neighbours8(cx, cy, w, h)
                .into_iter()
                .map(|(nx2, ny2)| ny2 * w + nx2)
                .find(|&p| skeleton[p] && p != prev);
            prev = cur;
            match next {
                Some(p) => cur = p,
                None => break,
            }
        }
        if chain.len() >= 4 {
            closed_chains.push(chain);
        }
    }

    (nodes, edges, closed_chains)
}

// ---------------------------------------------------------------------------
// G. Prune
// ---------------------------------------------------------------------------

fn edge_arc_length(edge: &WorkEdge) -> f64 {
    edge.points
        .windows(2)
        .map(|w| (w[1].point - w[0].point).hypot())
        .sum()
}

fn prune(nodes: Vec<Node>, mut edges: Vec<WorkEdge>, r: &[f64], min_line_length: f64) -> Vec<WorkEdge> {
    let junction_r = |node: usize| -> f64 {
        let p = nodes[node].pixel;
        r[p.min(r.len() - 1)]
    };

    for _pass in 0..2 {
        let mut drop_indices: Vec<usize> = Vec::new();
        for (i, edge) in edges.iter().enumerate() {
            let deg_a = edges
                .iter()
                .filter(|e| e.node_a == edge.node_a || e.node_b == edge.node_a)
                .count();
            let deg_b = edges
                .iter()
                .filter(|e| e.node_a == edge.node_b || e.node_b == edge.node_b)
                .count();
            let leaf_a = deg_a == 1;
            let leaf_b = deg_b == 1;
            if leaf_a == leaf_b {
                // Both ends free (an isolated segment) or both ends
                // junctions (a real inter-junction segment): not a spur.
                continue;
            }
            let junction_node = if leaf_a { edge.node_b } else { edge.node_a };
            let limit = min_line_length.max(2.0 * junction_r(junction_node));
            if edge_arc_length(edge) < limit {
                drop_indices.push(i);
            }
        }
        if drop_indices.is_empty() {
            continue;
        }
        let drop: std::collections::HashSet<usize> = drop_indices.into_iter().collect();
        edges = edges
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !drop.contains(i))
            .map(|(_, e)| e)
            .collect();
    }

    // Join edges at nodes now left with exactly two.
    loop {
        let mut joined = false;
        'search: for a in 0..edges.len() {
            for b in (a + 1)..edges.len() {
                let shared = [
                    (edges[a].node_a, edges[b].node_a),
                    (edges[a].node_a, edges[b].node_b),
                    (edges[a].node_b, edges[b].node_a),
                    (edges[a].node_b, edges[b].node_b),
                ]
                .into_iter()
                .find(|&(x, y)| x == y);
                let Some((shared_node, _)) = shared else { continue };
                let degree = edges
                    .iter()
                    .filter(|e| e.node_a == shared_node || e.node_b == shared_node)
                    .count();
                if degree != 2 {
                    continue;
                }
                let merged = splice(&edges[a], &edges[b], shared_node);
                let mut next: Vec<WorkEdge> = edges
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != a && *i != b)
                    .map(|(_, e)| e.clone())
                    .collect();
                next.push(merged);
                edges = next;
                joined = true;
                break 'search;
            }
        }
        if !joined {
            break;
        }
    }

    edges
}

/// Concatenate two edges that share `shared_node`, dropping the duplicate
/// joining point.
fn splice(a: &WorkEdge, b: &WorkEdge, shared_node: usize) -> WorkEdge {
    let (a_far, a_points) = if a.node_a == shared_node {
        (a.node_b, reversed(&a.points))
    } else {
        (a.node_a, a.points.clone())
    };
    let (b_far, b_points) = if b.node_a == shared_node {
        (b.node_b, b.points.clone())
    } else {
        (b.node_a, reversed(&b.points))
    };
    let mut points = a_points;
    // `a_points` ends at the shared node; `b_points` starts there too.
    points.extend(b_points.into_iter().skip(1));
    WorkEdge { points, node_a: a_far, node_b: b_far }
}

fn reversed(points: &[SkelPoint]) -> Vec<SkelPoint> {
    let mut out = points.to_vec();
    out.reverse();
    out
}

// ---------------------------------------------------------------------------
// H. Continuation at junctions
// ---------------------------------------------------------------------------

fn pair_junctions(nodes: &[Node], mut edges: Vec<WorkEdge>, r: &[f64]) -> Vec<Vec<SkelPoint>> {
    let mut junctions: Vec<usize> = (0..nodes.len())
        .filter(|&n| {
            edges.iter().filter(|e| e.node_a == n || e.node_b == n).count() >= 3
        })
        .collect();
    junctions.sort_unstable();

    for node in junctions {
        // How far along each edge to measure its tangent — far enough that
        // the junction's own noisy pixels do not dominate it, close enough
        // that it still describes the edge's real direction.
        let local_r = r.get(nodes[node].pixel).copied().unwrap_or(1.0);
        let k_at_node = ((2.0 * local_r).round() as i64 + 3).clamp(4, 12) as usize;
        loop {
            let incident: Vec<usize> = (0..edges.len())
                .filter(|&i| edges[i].node_a == node || edges[i].node_b == node)
                .collect();
            if incident.len() < 2 {
                break;
            }
            let tangent = |i: usize| -> Vec2 {
                let e = &edges[i];
                let from_a = e.node_a == node;
                let pts = if from_a { e.points.clone() } else { reversed(&e.points) };
                let k = k_at_node.min(pts.len().saturating_sub(1)).max(1);
                let far = pts[k.min(pts.len() - 1)].point;
                let near = pts[0].point;
                let v = far - near;
                let len = v.hypot();
                if len > 1e-9 { v / len } else { Vec2::new(1.0, 0.0) }
            };
            let tangents: Vec<Vec2> = incident.iter().map(|&i| tangent(i)).collect();

            let mut best: Option<(usize, usize, f64)> = None;
            for a in 0..incident.len() {
                for b in (a + 1)..incident.len() {
                    let dot = tangents[a].x * tangents[b].x + tangents[a].y * tangents[b].y;
                    if best.is_none_or(|(_, _, best_dot)| dot < best_dot) {
                        best = Some((a, b, dot));
                    }
                }
            }
            let Some((a, b, dot)) = best else { break };
            // More than 150 degrees apart: cos(150deg) = -0.866.
            if dot > -0.866 {
                break;
            }
            let ia = incident[a];
            let ib = incident[b];
            let merged = splice(&edges[ia], &edges[ib], node);
            let (lo, hi) = (ia.min(ib), ia.max(ib));
            let mut next: Vec<WorkEdge> = edges
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != lo && *i != hi)
                .map(|(_, e)| e.clone())
                .collect();
            next.push(merged);
            edges = next;
        }
    }

    edges.into_iter().map(|e| e.points).collect()
}

// ---------------------------------------------------------------------------
// J/K. Width, smoothing, fitting and the shape itself
// ---------------------------------------------------------------------------

fn arc_length(points: &[SkelPoint]) -> f64 {
    points
        .windows(2)
        .map(|w| (w[1].point - w[0].point).hypot())
        .sum()
}

fn median_filter_5(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    let mut out = vec![0.0; n];
    for i in 0..n {
        let lo = i.saturating_sub(2);
        let hi = (i + 2).min(n - 1);
        let mut window: Vec<f64> = values[lo..=hi].to_vec();
        window.sort_by(|a, b| a.total_cmp(b));
        out[i] = window[window.len() / 2];
    }
    out
}

fn mean_filter_5(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    let mut out = vec![0.0; n];
    for i in 0..n {
        let lo = i.saturating_sub(2);
        let hi = (i + 2).min(n - 1);
        let window = &values[lo..=hi];
        out[i] = window.iter().sum::<f64>() / window.len() as f64;
    }
    out
}

fn median_of(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 1.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Repeatedly smooth sample positions, endpoints pinned; a closed chain is
/// padded with wrapped samples first so the seam sees real neighbours.
fn smooth_chain(samples: &[StrokeSample], line_smooth: f64, closed: bool) -> Vec<StrokeSample> {
    let passes = 1 + (line_smooth * 3.0) as usize;
    let smooth = line_smooth.clamp(0.0, 1.0);
    if closed && samples.len() > 6 {
        let pad = 3;
        let mut padded = Vec::with_capacity(samples.len() + pad * 2);
        padded.extend_from_slice(&samples[samples.len() - pad..]);
        padded.extend_from_slice(samples);
        padded.extend_from_slice(&samples[..pad]);
        let mut out = padded;
        for _ in 0..passes {
            out = buzz_geom::brush::smooth_samples(&out, smooth);
        }
        out[pad..pad + samples.len()].to_vec()
    } else {
        let mut out = samples.to_vec();
        for _ in 0..passes {
            out = buzz_geom::brush::smooth_samples(&out, smooth);
        }
        out
    }
}

/// Douglas-Peucker over `f64` points.
fn simplify_points(points: &[Point], tol: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    fn perpendicular(p: Point, a: Point, b: Point) -> f64 {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            return (p - a).hypot();
        }
        ((p.x - a.x) * dy - (p.y - a.y) * dx).abs() / len
    }
    fn recurse(points: &[Point], tol: f64, out: &mut Vec<Point>) {
        let (first, last) = (points[0], points[points.len() - 1]);
        let mut index = 0;
        let mut max = 0.0;
        for (i, &p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
            let d = perpendicular(p, first, last);
            if d > max {
                max = d;
                index = i;
            }
        }
        if max > tol {
            recurse(&points[..=index], tol, out);
            out.pop();
            recurse(&points[index..], tol, out);
        } else {
            out.push(first);
            out.push(last);
        }
    }
    let mut out = Vec::new();
    recurse(points, tol, &mut out);
    out
}

/// A closed Catmull-Rom loop through `points` (which do not repeat the first
/// point at the end).
///
/// An open [`catmull_rom`] plus [`BezPath::close_path`] leaves a visible
/// straight seam and a flattened tangent right where the point list happens
/// to start, because the construction has no idea the two ends are actually
/// neighbours on a loop. Wrapping a couple of points from each side into the
/// fit, then keeping only the segments of the true loop, gives every point
/// — including the one the list starts on — a real neighbour on both sides.
fn closed_catmull_rom(points: &[Point]) -> BezPath {
    let n = points.len();
    if n < 3 {
        let mut path = catmull_rom(points);
        path.close_path();
        return path;
    }
    let pad = 2.min(n - 1);
    let mut padded = Vec::with_capacity(n + 2 * pad);
    padded.extend_from_slice(&points[n - pad..]);
    padded.extend_from_slice(points);
    padded.extend_from_slice(&points[..pad]);

    let full = catmull_rom(&padded);
    let elements = full.elements();
    // `elements[0]` is `MoveTo(padded[0])`; `elements[1 + i]` is the curve for
    // `padded[i] -> padded[i + 1]`. The true loop is `padded[pad]` (== the
    // first input point) through `padded[pad + n]` (== the same point,
    // having wrapped around) — segments `pad ..= pad + n - 1`.
    let start = pad + 1;
    let end = (pad + n).min(elements.len().saturating_sub(1));

    let mut out = BezPath::new();
    out.move_to(points[0]);
    if start <= end {
        for el in &elements[start..=end] {
            out.push(*el);
        }
    }
    out.close_path();
    out
}

fn chain_to_shape(
    chain: &[SkelPoint],
    closed: bool,
    r: &[f64],
    img_w: usize,
    pixels: &[u8],
    options: &TraceOptions,
) -> Option<ShapeData> {
    if chain.len() < 2 {
        return None;
    }
    let n = chain.len();

    // Raw widths from the distance field.
    let raw_w: Vec<f64> = chain
        .iter()
        .map(|p| {
            let ri = r.get(p.pixel).copied().unwrap_or(1.0);
            (2.0 * ri - 1.0).max(1.0)
        })
        .collect();
    let filtered = mean_filter_5(&median_filter_5(&raw_w));
    let w_max = filtered.iter().cloned().fold(1.0f64, f64::max);
    let median_w = median_of(&filtered);

    // Arc length for the time field.
    let mut arc = vec![0.0; n];
    for i in 1..n {
        arc[i] = arc[i - 1] + (chain[i].point - chain[i - 1].point).hypot();
    }

    let samples: Vec<StrokeSample> = (0..n)
        .map(|i| StrokeSample::with_pressure(chain[i].point, filtered[i] / w_max, arc[i] / 1000.0))
        .collect();
    let smooth_line = options.line_smooth.clamp(0.0, 1.0);
    let smoothed = smooth_chain(&samples, smooth_line, closed);

    let colour = mean_colour(pixels, &chain.iter().map(|p| p.pixel).collect::<Vec<_>>());
    let _ = img_w;

    match options.line_style {
        LineStyle::Stroke => {
            let points: Vec<Point> = smoothed.iter().map(|s| s.point).collect();
            let tol = 0.35 + 1.15 * smooth_line;
            let simplified = simplify_points(&points, tol);
            if simplified.len() < 2 {
                return None;
            }
            let path = if closed {
                closed_catmull_rom(&simplified)
            } else {
                catmull_rom(&simplified)
            };
            let width = match options.uniform_width {
                Some(w) => w,
                None => median_w.max(1.0),
            };
            Some(ShapeData::stroked(path, colour, width))
        }
        LineStyle::Brush => {
            let profile = BrushProfile {
                width: w_max.max(1.0),
                min_ratio: 0.0,
                response: WidthResponse::Pressure,
                smoothing: 0.0,
                stabiliser: 0.0,
                taper: 0.0,
                taper_ends: TaperEnds::Neither,
                viscosity: 1.0,
                tip: TipShape::Round,
            };
            // A closed chain is wrapped a little further so the outline's
            // caps land hidden under stroke that is already there, rather
            // than showing a seam.
            let mut wrapped = smoothed.clone();
            if closed && smoothed.len() > 3 {
                wrapped.extend_from_slice(&smoothed[..3]);
            }
            let out = fluid_outline(&wrapped, &profile, &BrushBudget::default());
            if out.path.elements().is_empty() {
                return None;
            }
            Some(ShapeData::filled(out.path, colour))
        }
    }
}

// ---------------------------------------------------------------------------
// L. Enclosed fills
// ---------------------------------------------------------------------------

fn enclosed_fills(
    ink: &[bool],
    w: usize,
    h: usize,
    pixels: &[u8],
    r: &[f64],
    speckle: usize,
    options: &TraceOptions,
) -> Vec<(usize, ShapeData)> {
    // Flood "outside" from the border and from transparent pixels, over
    // paper, 4-connected.
    let transparent = |i: usize| pixels[i * 4 + 3] < 128;
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let seed = |idx: usize, outside: &mut Vec<bool>, stack: &mut Vec<usize>| {
        if !ink[idx] && !outside[idx] {
            outside[idx] = true;
            stack.push(idx);
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
    for i in 0..w * h {
        if transparent(i) {
            seed(i, &mut outside, &mut stack);
        }
    }
    while let Some(at) = stack.pop() {
        let (x, y) = (at % w, at / w);
        for (nx, ny) in neighbours4(x, y, w, h) {
            let n = ny * w + nx;
            if !ink[n] && !outside[n] {
                outside[n] = true;
                stack.push(n);
            }
        }
    }

    let mut visited = vec![false; w * h];
    let mut fills = Vec::new();
    let mut scratch = Grid::new(w, h);
    for start in 0..w * h {
        if ink[start] || outside[start] || visited[start] {
            continue;
        }
        let comp = flood(ink, w, h, start, &mut visited, false, 4);
        if comp.len() < speckle {
            continue;
        }
        let median_r = median_of(&comp.iter().map(|&p| r[p]).collect::<Vec<_>>());
        let dilation = (median_r.ceil() as i64 + 1).max(1);
        let mut dilated = std::collections::HashSet::new();
        for &p in &comp {
            dilated.insert(p);
        }
        // Dilate `dilation` pixels into the ink, 8-connected, so the fill
        // tucks under the line rather than stopping short of it.
        let mut frontier: Vec<usize> = comp.clone();
        for _ in 0..dilation {
            let mut next = Vec::new();
            for &p in &frontier {
                let (x, y) = (p % w, p / w);
                for (nx, ny) in neighbours8(x, y, w, h) {
                    let n = ny * w + nx;
                    if dilated.insert(n) {
                        next.push(n);
                    }
                }
            }
            frontier = next;
        }
        for &p in &dilated {
            scratch.set(p % w, p / w);
        }
        let path = outline(&scratch, options);
        for &p in &dilated {
            scratch.unset(p % w, p / w);
        }
        if let Some(path) = path {
            let colour = mean_colour(pixels, &comp);
            fills.push((comp.len(), ShapeData::filled(path, colour)));
        }
    }
    fills
}

// ---------------------------------------------------------------------------
// N. Budget
// ---------------------------------------------------------------------------

fn box_downsample(w: usize, h: usize, pixels: &[u8], k: usize) -> (usize, usize, Vec<u8>) {
    let dw = w.div_ceil(k);
    let dh = h.div_ceil(k);
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        for dx in 0..dw {
            let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
            let mut count = 0u64;
            for oy in 0..k {
                let y = dy * k + oy;
                if y >= h {
                    continue;
                }
                for ox in 0..k {
                    let x = dx * k + ox;
                    if x >= w {
                        continue;
                    }
                    let i = (y * w + x) * 4;
                    r += pixels[i] as u64;
                    g += pixels[i + 1] as u64;
                    b += pixels[i + 2] as u64;
                    a += pixels[i + 3] as u64;
                    count += 1;
                }
            }
            let count = count.max(1);
            let o = (dy * dw + dx) * 4;
            out[o] = (r / count) as u8;
            out[o + 1] = (g / count) as u8;
            out[o + 2] = (b / count) as u8;
            out[o + 3] = (a / count) as u8;
        }
    }
    (dw, dh, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::TraceMode;

    fn paper(w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![255u8; w * h * 4];
        for i in 0..w * h {
            out[i * 4 + 3] = 255;
        }
        out
    }

    fn set_ink(px: &mut [u8], w: usize, x: i64, y: i64) {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let i = (y * w + x) * 4;
        if i + 3 < px.len() {
            px[i] = 0;
            px[i + 1] = 0;
            px[i + 2] = 0;
            px[i + 3] = 255;
        }
    }

    /// A straight ink line, `t` px thick, from `a` to `b`.
    fn ink_line(px: &mut [u8], w: usize, h: usize, a: (f64, f64), b: (f64, f64), t: f64) {
        let half = t / 2.0;
        for y in 0..h {
            for x in 0..w {
                let p = (x as f64 + 0.5, y as f64 + 0.5);
                let d = distance_to_segment(p, a, b);
                if d < half {
                    set_ink(px, w, x as i64, y as i64);
                }
            }
        }
    }

    fn distance_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
        let (abx, aby) = (b.0 - a.0, b.1 - a.1);
        let len2 = abx * abx + aby * aby;
        let t = if len2 > 1e-9 {
            (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (cx, cy) = (a.0 + abx * t, a.1 + aby * t);
        ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
    }

    /// A ring: ink between radius `r - t/2` and `r + t/2` from the centre.
    fn ink_ring(px: &mut [u8], w: usize, h: usize, cx: f64, cy: f64, r: f64, t: f64) {
        let half = t / 2.0;
        for y in 0..h {
            for x in 0..w {
                let d = (((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt() - r).abs();
                if d < half {
                    set_ink(px, w, x as i64, y as i64);
                }
            }
        }
    }

    /// A wedge from `w0` px thick at `x0` to `w1` px thick at `x1`, along the
    /// horizontal centreline `y`.
    fn ink_wedge(px: &mut [u8], w: usize, h: usize, y: f64, x0: f64, x1: f64, w0: f64, w1: f64) {
        for py in 0..h {
            for pxl in 0..w {
                let x = pxl as f64 + 0.5;
                if x < x0 || x > x1 {
                    continue;
                }
                let t = (x - x0) / (x1 - x0);
                let thick = w0 + (w1 - w0) * t;
                if (py as f64 + 0.5 - y).abs() < thick / 2.0 {
                    set_ink(px, w, pxl as i64, py as i64);
                }
            }
        }
    }

    fn options_for(style: LineStyle) -> TraceOptions {
        TraceOptions {
            mode: TraceMode::LineArt,
            line_style: style,
            fill_enclosed: false,
            speckle: 4,
            min_line_length: 8.0,
            ..TraceOptions::line_art()
        }
    }

    fn shape_bounds(shape: &ShapeData) -> buzz_geom::Rect {
        use buzz_geom::Shape as _;
        shape.path.bounding_box()
    }

    // -- 1. a straight stroke ------------------------------------------------

    #[test]
    fn a_straight_line_gives_one_stroke_near_its_own_axis() {
        let (w, h) = (200, 120);
        let mut px = paper(w, h);
        // Centred on a pixel row (a half-integer y), so a 5px-wide band
        // rasterises symmetrically rather than losing half a pixel to
        // rounding against a row boundary.
        ink_line(&mut px, w, h, (20.0, 60.5), (180.0, 60.5), 5.0);
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));

        assert_eq!(report.strokes, 1, "{}", report.message);
        let shape = &report.shapes[0];
        assert!(shape.fill.is_none(), "a Stroke-style trace has no fill");
        let points = flatten_points(&shape.path);
        for p in &points {
            assert!((p.y - 60.5).abs() <= 0.75, "point {p:?} strays from the axis");
        }
        let xs: Vec<f64> = points.iter().map(|p| p.x).collect();
        let (min_x, max_x) = (
            xs.iter().cloned().fold(f64::INFINITY, f64::min),
            xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        );
        assert!((min_x - 20.0).abs() < 4.0, "start at {min_x}");
        assert!((max_x - 180.0).abs() < 4.0, "end at {max_x}");
        let width = shape.stroke.as_ref().expect("a stroke").width;
        assert!((width - 5.0).abs() < 1.5, "width {width}");
    }

    /// **`uniform_width` overrides the measured width exactly**, for a caller
    /// that already knows the line weight it wants rather than trusting what
    /// noisy scan pixels measure to.
    #[test]
    fn uniform_width_forces_an_exact_stroke_width() {
        let (w, h) = (200, 120);
        let mut px = paper(w, h);
        ink_line(&mut px, w, h, (20.0, 60.5), (180.0, 60.5), 5.0);
        let options = TraceOptions {
            uniform_width: Some(2.5),
            ..options_for(LineStyle::Stroke)
        };
        let report = trace_lines(w as u32, h as u32, &px, &options);

        assert_eq!(report.strokes, 1, "{}", report.message);
        let width = report.shapes[0].stroke.as_ref().expect("a stroke").width;
        assert_eq!(width, 2.5, "uniform_width should be exact, not just close");
    }

    // -- 2. a ring ------------------------------------------------------------

    #[test]
    fn a_ring_gives_one_closed_path_at_its_own_radius() {
        let (w, h) = (120, 120);
        let mut px = paper(w, h);
        // A little thicker than the bare minimum, so the ring's own radius
        // is large next to the roughly one-pixel jitter a discrete skeleton's
        // junction clusters land with — a thin ring makes that jitter a much
        // bigger fraction of its radius.
        ink_ring(&mut px, w, h, 60.0, 60.0, 40.0, 6.0);
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));

        assert_eq!(report.strokes, 1, "{}", report.message);
        let shape = &report.shapes[0];
        let points = flatten_points(&shape.path);
        for p in &points {
            let d = ((p.x - 60.0).powi(2) + (p.y - 60.0).powi(2)).sqrt();
            assert!((d - 40.0).abs() <= 1.5, "point {p:?} at radius {d}");
        }
    }

    // -- 3. a plus sign ---------------------------------------------------

    #[test]
    fn a_plus_sign_gives_two_strokes() {
        let (w, h) = (120, 120);
        let mut px = paper(w, h);
        ink_line(&mut px, w, h, (20.0, 60.0), (100.0, 60.0), 5.0);
        ink_line(&mut px, w, h, (60.0, 20.0), (60.0, 100.0), 5.0);
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));
        assert_eq!(report.strokes, 2, "{}", report.message);
    }

    // -- 4. a T -------------------------------------------------------------

    #[test]
    fn a_t_gives_two_strokes() {
        let (w, h) = (120, 120);
        let mut px = paper(w, h);
        ink_line(&mut px, w, h, (20.0, 30.0), (100.0, 30.0), 5.0);
        ink_line(&mut px, w, h, (60.0, 30.0), (60.0, 100.0), 5.0);
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));
        assert_eq!(report.strokes, 2, "{}", report.message);
    }

    // -- 5. a wedge -----------------------------------------------------------

    #[test]
    fn a_wedge_tapers_from_thin_to_thick() {
        let (w, h) = (200, 60);
        let mut px = paper(w, h);
        ink_wedge(&mut px, w, h, 30.0, 20.0, 180.0, 2.0, 12.0);
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Brush));
        assert_eq!(report.strokes, 1, "{}", report.message);
        let shape = &report.shapes[0];
        let extent_near = |x: f64| -> f64 {
            let points = flatten_points(&shape.path);
            let ys: Vec<f64> = points.iter().filter(|p| (p.x - x).abs() < 4.0).map(|p| p.y).collect();
            match (
                ys.iter().cloned().fold(f64::INFINITY, f64::min),
                ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            ) {
                (lo, hi) if lo.is_finite() && hi.is_finite() => hi - lo,
                _ => 0.0,
            }
        };
        let thin = extent_near(40.0);
        let thick = extent_near(160.0);
        assert!(
            thin < thick * 0.5,
            "the thin end ({thin}) should be under half the thick end ({thick})"
        );
    }

    // -- 6. specks ------------------------------------------------------------

    #[test]
    fn specks_alone_give_no_strokes() {
        let (w, h) = (40, 40);
        let mut px = paper(w, h);
        set_ink(&mut px, w, 10, 10);
        set_ink(&mut px, w, 11, 10);
        set_ink(&mut px, w, 30, 20);
        set_ink(&mut px, w, 31, 20);
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));
        assert_eq!(report.strokes, 0, "{}", report.message);
    }

    // -- 7. a square outline with fill_enclosed --------------------------------

    #[test]
    fn a_square_outline_with_fill_enclosed_gives_a_stroke_and_a_fill() {
        let (w, h) = (140, 140);
        let mut px = paper(w, h);
        for &(a, b) in &[
            ((20.0, 20.0), (120.0, 20.0)),
            ((120.0, 20.0), (120.0, 120.0)),
            ((120.0, 120.0), (20.0, 120.0)),
            ((20.0, 120.0), (20.0, 20.0)),
        ] {
            ink_line(&mut px, w, h, a, b, 4.0);
        }
        let mut options = options_for(LineStyle::Stroke);
        options.fill_enclosed = true;
        let report = trace_lines(w as u32, h as u32, &px, &options);

        assert_eq!(report.strokes, 1, "{}", report.message);
        assert_eq!(report.fills, 1, "{}", report.message);
        let fill = report
            .shapes
            .iter()
            .find(|s| s.fill.is_some())
            .expect("a fill shape");
        let bb = shape_bounds(fill);
        // The interior, out to the centreline of the square give or take.
        assert!(bb.x0 > 18.0 && bb.x0 < 24.0, "{bb:?}");
        assert!(bb.x1 > 116.0 && bb.x1 < 122.0, "{bb:?}");
    }

    // -- 8. a disc in line-art mode is a mass ----------------------------------

    #[test]
    fn a_disc_is_traced_as_a_mass_not_a_line() {
        let (w, h) = (64, 64);
        let mut px = paper(w, h);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f64 + 0.5 - 32.0).powi(2) + (y as f64 + 0.5 - 32.0).powi(2)).sqrt();
                if d < 20.0 {
                    set_ink(&mut px, w, x as i64, y as i64);
                }
            }
        }
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));
        assert_eq!(report.strokes, 0, "{}", report.message);
        assert_eq!(report.regions, 1, "{}", report.message);
    }

    // -- 9. a face --------------------------------------------------------------

    #[test]
    fn a_simple_face_gives_a_handful_of_strokes() {
        let (w, h) = (200, 200);
        let mut px = paper(w, h);
        ink_ring(&mut px, w, h, 100.0, 100.0, 80.0, 5.0);
        ink_line(&mut px, w, h, (70.0, 80.0), (85.0, 80.0), 4.0);
        ink_line(&mut px, w, h, (115.0, 80.0), (130.0, 80.0), 4.0);
        // A shallow arc for the mouth, approximated with short segments.
        let mut prev = None;
        for i in 0..=12 {
            let t = i as f64 / 12.0;
            let x = 70.0 + t * 60.0;
            let y = 140.0 + (t * std::f64::consts::PI).sin() * 15.0;
            if let Some(p) = prev {
                ink_line(&mut px, w, h, p, (x, y), 4.0);
            }
            prev = Some((x, y));
        }
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));
        assert!(
            report.strokes >= 4 && report.strokes <= 6,
            "expected 4 to 6 strokes, got {}: {}",
            report.strokes,
            report.message
        );
    }

    // -- 10. a bump -------------------------------------------------------------

    #[test]
    fn a_line_with_a_small_bump_is_still_one_stroke() {
        let (w, h) = (160, 60);
        let mut px = paper(w, h);
        for x in 0..w {
            let xf = x as f64 + 0.5;
            let bump = if (70.0..90.0).contains(&xf) {
                2.0 * (1.0 - ((xf - 80.0) / 10.0).abs())
            } else {
                0.0
            };
            let y = 30.0 + bump;
            for py in 0..h {
                if (py as f64 + 0.5 - y).abs() < 2.5 {
                    set_ink(&mut px, w, x as i64, py as i64);
                }
            }
        }
        let report = trace_lines(w as u32, h as u32, &px, &options_for(LineStyle::Stroke));
        assert_eq!(report.strokes, 1, "{}", report.message);
    }

    // -- 11. determinism --------------------------------------------------------

    #[test]
    fn tracing_a_line_is_deterministic() {
        let (w, h) = (140, 80);
        let mut px = paper(w, h);
        ink_line(&mut px, w, h, (20.0, 40.0), (120.0, 40.0), 5.0);
        let options = options_for(LineStyle::Brush);
        let a = trace_lines(w as u32, h as u32, &px, &options);
        let b = trace_lines(w as u32, h as u32, &px, &options);
        assert_eq!(a.strokes, b.strokes);
        assert_eq!(a.shapes.len(), b.shapes.len());
        for (x, y) in a.shapes.iter().zip(&b.shapes) {
            assert_eq!(x.path.elements().len(), y.path.elements().len());
            let (px, py) = (flatten_points(&x.path), flatten_points(&y.path));
            assert_eq!(px.len(), py.len());
            for (p, q) in px.iter().zip(&py) {
                assert!((p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9);
            }
        }
    }

    fn flatten_points(path: &buzz_geom::BezPath) -> Vec<Point> {
        let mut points = Vec::new();
        kurbo::flatten(path.iter(), 0.1, |el| match el {
            buzz_geom::PathEl::MoveTo(p) | buzz_geom::PathEl::LineTo(p) => points.push(p),
            _ => {}
        });
        points
    }
}
