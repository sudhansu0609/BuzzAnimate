//! **Even lines on a colour trace** (`docs/TRACE_PICTURE_PLAN.md` T10): the
//! cartoon's ink drawn as strokes of one width over the flat colour, as if
//! drawn with the Line tool, instead of ragged dark fill regions.
//!
//! The ink is found by **local contrast** (a black-hat: the grey picture
//! closed at the widest line, minus itself), so a thin dark ridge is ink but a
//! dark doorway or a night sky, which no small closing fills, is not. The ink
//! pixels are painted over from their neighbours before the colours are
//! quantised, so no dark fill is left under the lines; the ink mask goes
//! through the line tracer's own centreline pipeline ([`crate::trace_lines`]),
//! and every stroke gets one width.

use crate::object::{Paint, ShapeData};
use crate::trace::{LineStyle, TraceMode, TraceOptions, TraceReport, delta_e, srgb_to_lab};
use crate::trace_lines::{Straighten, trace_core_with};
use buzz_geom::Point;
use peniko::Color;

/// Ink is at least this much darker (0-255 luma) than the picture closed over it.
const MIN_CONTRAST: f32 = 24.0;
/// ... and under this fraction of it.
const MAX_RATIO: f32 = 0.8;
/// ... and dark in itself: ink is never brighter than this (0-255 luma), so a lit
/// marigold garland between two lamps is not drawn over.
const MAX_INK_LUMA: f32 = 100.0;
/// No ink within this many pixels (at 1920 px) of bright or coloured detail.
const DETAIL_ZONE_PX: f64 = 5.0;
/// A stroke is at least this many widths long.
const MIN_LENGTH_PER_WIDTH: f64 = 4.0;
/// Dark places (closed luma below this) also take faint ridges of this contrast ...
const DARK_PLACE: f32 = 80.0;
const FAINT_CONTRAST: f32 = 7.0;
/// ... when they are at least this long (pixels at 1920 px).
const FAINT_MIN_LENGTH_PX: f64 = 24.0;
/// The widest line, in pixels at a 1920 px picture (scaled with the picture).
const LINE_MAX_PX: f64 = 8.0;
/// A stroke whose own colour is further than this (ΔE) from the picture's ink keeps its own.
const OWN_COLOUR_DE: f64 = 18.0;

/// Max (or min) over a square window of radius `r`, separably.
fn morph(src: &[f32], w: usize, h: usize, r: usize, max: bool) -> Vec<f32> {
    let pick = |a: f32, b: f32| if max { a.max(b) } else { a.min(b) };
    let mut tmp = src.to_vec();
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = row[lo..=hi].iter().copied().reduce(pick).unwrap_or(row[x]);
        }
    }
    let mut out = tmp.clone();
    for x in 0..w {
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            let mut v = tmp[lo * w + x];
            for yy in lo + 1..=hi {
                v = pick(v, tmp[yy * w + x]);
            }
            out[y * w + x] = v;
        }
    }
    out
}

/// The thin dark ridges of the picture: `true` is ink; and the faint ones found only
/// in dark places (a window divider), drawn in their own colour.
fn ink_ridges(w: usize, h: usize, pixels: &[u8]) -> (Vec<bool>, Vec<bool>) {
    let luma: Vec<f32> = (0..w * h)
        .map(|i| {
            let p = &pixels[i * 4..i * 4 + 4];
            let l = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            let a = p[3] as f32 / 255.0;
            l * a + 255.0 * (1.0 - a)
        })
        .collect();
    let scale = w.max(h) as f64 / 1920.0;
    let r = ((LINE_MAX_PX * scale / 2.0).round() as usize).clamp(2, 8);
    let closed = morph(&morph(&luma, w, h, r, true), w, h, r, false);
    // Small bright or coloured detail (garland flowers, bulbs, crackers): no ink
    // within two line widths of it, so the colour trace keeps its shapes.
    let bright: Vec<f32> = (0..w * h)
        .map(|i| {
            let p = &pixels[i * 4..i * 4 + 4];
            let (hi, lo) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
            if p[3] >= 128 && ((hi >= 140 && hi - lo >= 70) || luma[i] >= 170.0) { 1.0 } else { 0.0 }
        })
        .collect();
    let zr = ((DETAIL_ZONE_PX * scale).round() as usize).clamp(2, 12);
    let zone = morph(&morph(&bright, w, h, zr, true), w, h, 1, true);

    // The luma's brightest in `r` pixels each side, across the line: a line is a
    // ridge when it is darker than **both** sides, in either direction.
    let side_strength = |i: usize| -> f32 {
        let (x, y) = (i % w, i / w);
        let mut best = f32::MIN;
        for (dx, dy) in [(1i64, 0i64), (0, 1)] {
            let (mut a, mut b) = (f32::MIN, f32::MIN);
            for k in 1..=r as i64 {
                let (ax, ay) = (x as i64 - dx * k, y as i64 - dy * k);
                let (bx, by) = (x as i64 + dx * k, y as i64 + dy * k);
                if ax >= 0 && ay >= 0 {
                    a = a.max(luma[ay as usize * w + ax as usize]);
                }
                if (bx as usize) < w && (by as usize) < h {
                    b = b.max(luma[by as usize * w + bx as usize]);
                }
            }
            if a > f32::MIN && b > f32::MIN {
                best = best.max(a.min(b) - luma[i]);
            }
        }
        best
    };
    // Candidates: strong ridges, and faint ones in dark places (a window divider).
    let strong: Vec<bool> = (0..w * h)
        .map(|i| closed[i] - luma[i] >= MIN_CONTRAST && luma[i] < MAX_RATIO * closed[i] && luma[i] < MAX_INK_LUMA && pixels[i * 4 + 3] >= 128 && zone[i] < 0.5)
        .collect();
    let faint: Vec<bool> = (0..w * h)
        .map(|i| {
            !strong[i]
                && closed[i] < DARK_PLACE
                && luma[i] < MAX_INK_LUMA
                && zone[i] < 0.5
                && pixels[i * 4 + 3] >= 128
                && closed[i] - luma[i] >= FAINT_CONTRAST
                && luma[i] < MAX_RATIO * closed[i]
                && side_strength(i) >= FAINT_CONTRAST
        })
        .collect();

    // Keep long thin components only: dots and short dashes stay colour regions.
    let mut ink = vec![false; w * h];
    let mut faint_ink = vec![false; w * h];
    for (cand, faint_pass) in [(&strong, false), (&faint, true)] {
        let mut seen = vec![false; w * h];
        let mut stack: Vec<usize> = Vec::new();
        for start in 0..w * h {
            if !cand[start] || seen[start] {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
            let mut comp: Vec<usize> = Vec::new();
            seen[start] = true;
            stack.push(start);
            while let Some(at) = stack.pop() {
                comp.push(at);
                let (x, y) = (at % w, at / w);
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let n = ny as usize * w + nx as usize;
                        if cand[n] && !seen[n] {
                            seen[n] = true;
                            stack.push(n);
                        }
                    }
                }
            }
            let diag = ((x1 - x0) as f64).hypot((y1 - y0) as f64) + 1.0;
            let width = (comp.len() as f64 / diag).max(2.0);
            let long = diag >= MIN_LENGTH_PER_WIDTH * width && (!faint_pass || diag >= FAINT_MIN_LENGTH_PX * scale.max(1.0));
            if comp.len() >= 10 && long {
                for p in comp {
                    if faint_pass {
                        faint_ink[p] = true;
                    } else {
                        ink[p] = true;
                    }
                }
            }
        }
    }
    (ink, faint_ink)
}

/// `pixels` with every ink pixel (and the antialiasing round it) painted over
/// from the nearest pixels that are not ink.
fn inpaint(w: usize, h: usize, pixels: &[u8], ink: &[bool]) -> Vec<u8> {
    let mut hole = ink.to_vec();
    for y in 0..h {
        for x in 0..w {
            if ink[y * w + x] {
                for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        hole[ny * w + nx] = true;
                    }
                }
            }
        }
    }
    let mut out = pixels.to_vec();
    let mut done: Vec<bool> = hole.iter().map(|&h| !h).collect();
    let mut frontier: Vec<usize> = (0..w * h).filter(|&i| hole[i]).collect();
    while !frontier.is_empty() {
        let mut filled: Vec<(usize, [u8; 4])> = Vec::new();
        let mut rest: Vec<usize> = Vec::new();
        for &at in &frontier {
            let (x, y) = (at % w, at / w);
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = ny * w + nx;
                    if done[j] {
                        for c in 0..4 {
                            sum[c] += out[j * 4 + c] as u32;
                        }
                        n += 1;
                    }
                }
            }
            if n > 0 {
                filled.push((at, [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, (sum[3] / n) as u8]));
            } else {
                rest.push(at);
            }
        }
        if filled.is_empty() {
            break;
        }
        for (at, c) in filled {
            out[at * 4..at * 4 + 4].copy_from_slice(&c);
            done[at] = true;
        }
        frontier = rest;
    }
    out
}

fn median(v: &mut [u8]) -> u8 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// The colour trace of `pixels` with the ink drawn over it as even strokes.
pub(crate) fn trace(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> TraceReport {
    let (w, h) = (width as usize, height as usize);
    let want = options.even_lines.unwrap_or(0.0);
    // T15: bold outlines draw the ink and the object edges in one weight and
    // colour ([`crate::trace_bold`]); the colour trace brings its own edges.
    let bold = options.bold_outlines.then(|| {
        let width = if want > 0.0 {
            want
        } else {
            options.uniform_width.filter(|&u| u > 0.0).unwrap_or_else(|| crate::trace_bold::bold_width(w.max(h) as f64) * crate::trace_bold::TRACE_BOLD_SHARE)
        };
        (width, options.line_colour.unwrap_or_else(crate::trace_bold::bold_colour))
    });
    let plain = match bold {
        Some((width, colour)) => TraceOptions {
            even_lines: None,
            mode: TraceMode::ColourLines,
            uniform_width: Some(width),
            line_colour: Some(colour),
            ..*options
        },
        None => TraceOptions { even_lines: None, ..*options },
    };
    if w < 8 || h < 8 || pixels.len() < w * h * 4 {
        return crate::trace::trace_regions_only(width, height, pixels, &plain);
    }
    let (ink, faint) = ink_ridges(w, h, pixels);
    if !ink.iter().any(|&v| v) && !faint.iter().any(|&v| v) {
        return crate::trace::trace_regions_only(width, height, pixels, &plain);
    }

    // The ink's own colour: the median over every ink pixel.
    let (mut rs, mut gs, mut bs) = (Vec::new(), Vec::new(), Vec::new());
    for i in (0..w * h).filter(|&i| ink[i]) {
        rs.push(pixels[i * 4]);
        gs.push(pixels[i * 4 + 1]);
        bs.push(pixels[i * 4 + 2]);
    }
    let global = [median(&mut rs), median(&mut gs), median(&mut bs)];
    let global_lab = srgb_to_lab(global);

    // Colour regions on the picture with the ink painted out.
    let both: Vec<bool> = (0..w * h).map(|i| ink[i] || faint[i]).collect();
    let clean = inpaint(w, h, pixels, &both);
    // The colour regions on the picture with the ink painted out, and the ink
    // as black on white through the centreline pipeline, side by side.
    let mut mask = vec![255u8; w * h * 4];
    for i in (0..w * h).filter(|&i| ink[i] || faint[i]) {
        mask[i * 4..i * 4 + 3].fill(0);
    }
    let mut lo = TraceOptions::line_art();
    lo.mode = TraceMode::LineArt;
    lo.line_style = LineStyle::Stroke;
    lo.uniform_width = None;
    lo.ink_detect = crate::trace::InkDetect::Global(Some(140));
    lo.speckle = 6;
    lo.fill_enclosed = false;
    lo.ink_masses = true;
    lo.min_line_length = options.min_line_length;
    lo.line_smooth = options.line_smooth;
    let (mut report, lines) = rayon::join(
        || crate::trace::trace_regions_only(width, height, &clean, &plain),
        || trace_core_with(w, h, &mask, &lo, Straighten::default()),
    );
    let mut strokes: Vec<ShapeData> = lines.shapes.into_iter().filter(|s| s.stroke.is_some()).collect();
    if strokes.is_empty() {
        return report;
    }

    // One width: the median measured, or what was asked for.
    let width_px = if let Some((width, _)) = bold {
        width
    } else if want > 0.0 {
        want
    } else {
        // The ink's area over the length of the strokes that draw it.
        let mut length = 0.0;
        for shape in &strokes {
            let mut last: Option<Point> = None;
            kurbo::flatten(shape.path.iter(), 0.5, |el| match el {
                kurbo::PathEl::MoveTo(p) => last = Some(p),
                kurbo::PathEl::LineTo(p) => {
                    if let Some(l) = last {
                        length += (p - l).hypot();
                    }
                    last = Some(p);
                }
                _ => {}
            });
        }
        let area = ink.iter().filter(|&&v| v).count() as f64;
        (area / length.max(1.0)).clamp(1.5, 6.0)
    };
    for shape in &mut strokes {
        // Own colour when clearly different (brown against black), else the picture's.
        let mut pts: Vec<Point> = Vec::new();
        kurbo::flatten(shape.path.iter(), 0.5, |el| match el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => pts.push(p),
            _ => {}
        });
        let (mut r, mut g, mut b) = (Vec::new(), Vec::new(), Vec::new());
        let mut faint_hits = 0usize;
        for p in &pts {
            let (x, y) = (p.x.floor() as i64, p.y.floor() as i64);
            if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                continue;
            }
            let i = y as usize * w + x as usize;
            if ink[i] || faint[i] {
                faint_hits += faint[i] as usize;
                r.push(pixels[i * 4]);
                g.push(pixels[i * 4 + 1]);
                b.push(pixels[i * 4 + 2]);
            }
        }
        let own = [median(&mut r), median(&mut g), median(&mut b)];
        // A faint line (dark on dark) always keeps its own colour.
        let faint_line = faint_hits * 2 > r.len();
        let rgb = if r.is_empty() || (!faint_line && delta_e(srgb_to_lab(own), global_lab) <= OWN_COLOUR_DE) { global } else { own };
        if let Some((width, colour)) = bold {
            // A short stroke is detail.
            let length = crate::trace_bold::path_length(&shape.path);
            if let Some(stroke) = shape.stroke.as_mut() {
                stroke.width = crate::trace_bold::edge_width(length, false, width).unwrap_or(crate::trace_bold::detail_width(width));
                stroke.paint = Paint::Solid(colour);
            }
        } else if let Some(stroke) = shape.stroke.as_mut() {
            stroke.width = width_px;
            stroke.paint = Paint::Solid(Color::from_rgb8(rgb[0], rgb[1], rgb[2]));
        }
    }

    let n = strokes.len();
    report.shapes.extend(strokes);
    report.strokes += n;
    report.message = format!("{} (+ {n} even lines, {width_px:.1} px)", report.message);
    report
}
