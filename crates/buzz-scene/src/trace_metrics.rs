//! How closely a rendered picture matches the source it was traced from: the
//! numbers of `docs/TRACE_QUALITY_PLAN.md`, shared by the `trace_png` and
//! `rig_vs_source` examples so both measure the same way.
//!
//! Every function takes straight RGBA8 buffers of `w` x `h` pixels. `src`'s
//! alpha says which pixels are the picture (`> 128`); `traced` is the render,
//! compared pixel for pixel.

use crate::trace::{delta_e, sobel_magnitude, srgb_to_lab};

/// Mean CIE76 delta E between `src` and `traced` over the picture's flat
/// interior (opaque, away from the silhouette, gradient under 15).
pub fn compute_mean_delta_e(src: &[u8], traced: &[u8], w: usize, h: usize) -> f64 {
    let mut luma_src = vec![0.0f64; w * h];
    for i in 0..w * h {
        let px = &src[i * 4..i * 4 + 4];
        if px[3] > 128 {
            luma_src[i] = srgb_to_lab([px[0], px[1], px[2]])[0];
        }
    }
    let grad = sobel_magnitude(&luma_src, w, h);
    let mut sum_de = 0.0;
    let mut count = 0;
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            let spx = &src[i * 4..i * 4 + 4];
            if spx[3] <= 128 {
                continue;
            }
            let near_trans = [
                src[(i - 1) * 4 + 3],
                src[(i + 1) * 4 + 3],
                src[(i - w) * 4 + 3],
                src[(i + w) * 4 + 3],
            ]
            .iter()
            .any(|&a| a <= 128);
            if near_trans || grad[i] > 15.0 {
                continue;
            }
            let tpx = &traced[i * 4..i * 4 + 4];
            let lab_src = srgb_to_lab([spx[0], spx[1], spx[2]]);
            let lab_traced = srgb_to_lab([tpx[0], tpx[1], tpx[2]]);
            sum_de += delta_e(lab_src, lab_traced);
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum_de / count as f64 }
}

/// (mean, p95) distance in pixels from every edge pixel of `traced` to the
/// nearest edge pixel of `src`.
pub fn compute_edge_distance(src: &[u8], traced: &[u8], w: usize, h: usize) -> (f64, f64) {
    let mut luma_src = vec![0.0f64; w * h];
    let mut luma_traced = vec![0.0f64; w * h];
    for i in 0..w * h {
        let spx = &src[i * 4..i * 4 + 4];
        let tpx = &traced[i * 4..i * 4 + 4];
        luma_src[i] = srgb_to_lab([spx[0], spx[1], spx[2]])[0];
        luma_traced[i] = srgb_to_lab([tpx[0], tpx[1], tpx[2]])[0];
    }
    let grad_src = sobel_magnitude(&luma_src, w, h);
    let grad_traced = sobel_magnitude(&luma_traced, w, h);

    const EDGE_THRESH: f64 = 20.0;
    let mut src_edge_pts: Vec<(usize, usize)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if grad_src[y * w + x] > EDGE_THRESH {
                src_edge_pts.push((x, y));
            }
        }
    }
    if src_edge_pts.is_empty() {
        return (0.0, 0.0);
    }

    let grid_size = 16usize;
    let gw = (w + grid_size - 1) / grid_size;
    let gh = (h + grid_size - 1) / grid_size;
    let mut buckets: Vec<Vec<(usize, usize)>> = vec![Vec::new(); gw * gh];
    for &(x, y) in &src_edge_pts {
        let gx = x / grid_size;
        let gy = y / grid_size;
        buckets[gy * gw + gx].push((x, y));
    }

    let mut distances: Vec<f64> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if grad_traced[y * w + x] > EDGE_THRESH {
                let gx = x / grid_size;
                let gy = y / grid_size;
                let mut min_d2 = f64::INFINITY;
                let mut radius = 0isize;
                while radius <= 50 {
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            if dx.abs() != radius && dy.abs() != radius {
                                continue;
                            }
                            let bgx = gx as isize + dx;
                            let bgy = gy as isize + dy;
                            if bgx >= 0 && bgy >= 0 && (bgx as usize) < gw && (bgy as usize) < gh {
                                for &(sx, sy) in &buckets[bgy as usize * gw + bgx as usize] {
                                    let d2 = (x as f64 - sx as f64).powi(2) + (y as f64 - sy as f64).powi(2);
                                    if d2 < min_d2 {
                                        min_d2 = d2;
                                    }
                                }
                            }
                        }
                    }
                    if min_d2.sqrt() <= (radius as f64) * (grid_size as f64) {
                        break;
                    }
                    radius += 1;
                }
                if min_d2.is_finite() {
                    distances.push(min_d2.sqrt());
                }
            }
        }
    }

    if distances.is_empty() {
        return (0.0, 0.0);
    }
    distances.sort_by(|a, b| a.total_cmp(b));
    let mean = distances.iter().sum::<f64>() / distances.len() as f64;
    let p95_idx = ((distances.len() as f64 * 0.95).round() as usize).min(distances.len() - 1);
    let p95 = distances[p95_idx];
    (mean, p95)
}

/// (matched, total, percent): the small features of `src` (12 to 400 pixel
/// single-colour components that differ from their ring by more than 20 dE)
/// that `traced` reproduces (at least half their pixels within 18 dE).
pub fn compute_small_features(src: &[u8], traced: &[u8], w: usize, h: usize) -> (usize, usize, f64) {
    let mut seen = vec![false; w * h];
    let mut components = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let start = y * w + x;
            if seen[start] || src[start * 4 + 3] <= 128 {
                continue;
            }
            let start_lab = srgb_to_lab([src[start * 4], src[start * 4 + 1], src[start * 4 + 2]]);
            let mut cells = Vec::new();
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(at) = stack.pop() {
                cells.push(at);
                let (cx, cy) = (at % w, at / w);
                let check = |nx: usize, ny: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
                    let ni = ny * w + nx;
                    if !seen[ni] && src[ni * 4 + 3] > 128 {
                        let c = srgb_to_lab([src[ni * 4], src[ni * 4 + 1], src[ni * 4 + 2]]);
                        if delta_e(start_lab, c) < 10.0 {
                            seen[ni] = true;
                            stack.push(ni);
                        }
                    }
                };
                if cx > 0 {
                    check(cx - 1, cy, &mut stack, &mut seen);
                }
                if cx + 1 < w {
                    check(cx + 1, cy, &mut stack, &mut seen);
                }
                if cy > 0 {
                    check(cx, cy - 1, &mut stack, &mut seen);
                }
                if cy + 1 < h {
                    check(cx, cy + 1, &mut stack, &mut seen);
                }
            }
            if cells.len() >= 12 && cells.len() <= 400 {
                let mut ring_colours = Vec::new();
                for &cell in &cells {
                    let (cx, cy) = (cell % w, cell / w);
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (cx as isize + dx, cy as isize + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                            let ni = ny as usize * w + nx as usize;
                            if !cells.contains(&ni) && src[ni * 4 + 3] > 128 {
                                ring_colours.push(srgb_to_lab([src[ni * 4], src[ni * 4 + 1], src[ni * 4 + 2]]));
                            }
                        }
                    }
                }
                if !ring_colours.is_empty() {
                    let ring_mean = [
                        ring_colours.iter().map(|c| c[0]).sum::<f64>() / ring_colours.len() as f64,
                        ring_colours.iter().map(|c| c[1]).sum::<f64>() / ring_colours.len() as f64,
                        ring_colours.iter().map(|c| c[2]).sum::<f64>() / ring_colours.len() as f64,
                    ];
                    if delta_e(start_lab, ring_mean) > 20.0 {
                        components.push((cells, start_lab));
                    }
                }
            }
        }
    }

    if components.is_empty() {
        return (0, 0, 100.0);
    }
    let total = components.len();
    let mut matched = 0;
    for (cells, comp_lab) in &components {
        let mut overlap_count = 0;
        for &cell in cells {
            let tpx = &traced[cell * 4..cell * 4 + 4];
            let t_lab = srgb_to_lab([tpx[0], tpx[1], tpx[2]]);
            if delta_e(*comp_lab, t_lab) < 18.0 {
                overlap_count += 1;
            }
        }
        if overlap_count as f64 / cells.len() as f64 >= 0.50 {
            matched += 1;
        }
    }
    let pct = (matched as f64 / total as f64) * 100.0;
    (matched, total, pct)
}

/// (covered, total, percent): **how continuous the outlines are.** `ink` is the
/// source's thin-ink mask (`trace::thin_ink_mask`: strokes only, no masses);
/// `traced` is the render (opaque RGBA8, over white). An ink pixel is covered
/// when the render has a dark pixel (CIELAB L* below 45) within 2 px of it, i.e.
/// the render's dark mask is dilated by a disc of radius 2 and `ink` is looked
/// up in it. A dashed or missing outline leaves its ink uncovered. `100.0` when
/// the source has no thin ink.
pub fn compute_ink_coverage(ink: &[bool], traced: &[u8], w: usize, h: usize) -> (usize, usize, f64) {
    const REACH: i64 = 2;
    let dark: Vec<bool> = (0..w * h)
        .map(|i| srgb_to_lab([traced[i * 4], traced[i * 4 + 1], traced[i * 4 + 2]])[0] < 45.0)
        .collect();
    let mut offsets: Vec<(i64, i64)> = Vec::new();
    for dy in -REACH..=REACH {
        for dx in -REACH..=REACH {
            if dx * dx + dy * dy <= REACH * REACH {
                offsets.push((dx, dy));
            }
        }
    }
    let (mut covered, mut total) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if !ink[y * w + x] {
                continue;
            }
            total += 1;
            let near_dark = offsets.iter().any(|&(dx, dy)| {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && dark[ny as usize * w + nx as usize]
            });
            if near_dark {
                covered += 1;
            }
        }
    }
    if total == 0 {
        return (0, 0, 100.0);
    }
    (covered, total, covered as f64 / total as f64 * 100.0)
}
