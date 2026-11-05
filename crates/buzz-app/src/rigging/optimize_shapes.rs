//! **Optimize Shapes…** (Modify ▸ Shape, `docs/TRACE_PICTURE_PLAN.md` T13):
//! Animate's Modify ▸ Shape ▸ Optimize for a traced picture. A trace leaves
//! thousands of shapes with long runs of tiny curve segments; this gives the
//! same picture in far fewer.
//!
//! Three things, each optional:
//! - **Remove tiny**: a solid shape under N px² takes the colour of its
//!   largest touching neighbour (so the next step absorbs it), or is dropped
//!   when nothing touches it;
//! - **Merge same colour**: touching solid shapes of one colour (and one
//!   transform) are unioned into one with [`buzz_geom::boolean`];
//! - **Smoothing**: every path is refitted with fewer cubic segments within
//!   a tolerance in px, corners kept ([`buzz_geom::optimize_path`]).
//!
//! Pure geometry: the shapes in, the shapes out, in paint order; the editor
//! and the trace job write them back.

use buzz_geom::{Affine, BezPath, BoolOp, BooleanOptions, FillMode, PathEl, Rect, Shape as _, boolean_many, optimize_path};
use buzz_scene::{Paint, ShapeData};
use rayon::prelude::*;

/// A path longer than this is not refitted: the fitter's cost grows faster
/// than its length and a few huge outlines are not worth a stall.
const MAX_REFIT_SEGMENTS: usize = 60_000;
/// The least accuracy a merged shape is refitted at, px (a union leaves a
/// polygon, which must be fitted again to be any smaller).
const MERGED_FIT_PX: f64 = 0.15;

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptimizeSettings {
    /// Curve-fit tolerance, px; `0` leaves the curves alone.
    pub smoothing: f64,
    /// Union touching solid shapes of one colour.
    pub merge_same_colour: bool,
    /// Solid shapes under this many px² are absorbed (`0`: none).
    pub remove_tiny: f64,
}

impl Default for OptimizeSettings {
    /// The mild setting "Optimize after trace" uses.
    fn default() -> Self {
        Self { smoothing: 0.75, merge_same_colour: true, remove_tiny: 1.0 }
    }
}

/// One shape of the result.
#[derive(Debug, Clone)]
pub struct Piece {
    pub transform: Affine,
    pub shape: ShapeData,
    /// The input shapes it replaces (indices; the first is its paint-order
    /// slot). One entry unless several were merged.
    pub sources: Vec<usize>,
}

/// The optimized shapes, in paint order of their first source.
#[derive(Debug, Clone, Default)]
pub struct Optimized {
    pub pieces: Vec<Piece>,
    pub segments_before: usize,
    pub segments_after: usize,
    pub shapes_before: usize,
}

/// The curve segments of a path (moves and closes are not segments).
pub fn segment_count(path: &BezPath) -> usize {
    path.elements()
        .iter()
        .filter(|e| matches!(e, PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..)))
        .count()
}

fn solid(shape: &ShapeData) -> Option<[u8; 4]> {
    if shape.stroke.is_some() {
        return None;
    }
    match &shape.fill.as_ref()?.paint {
        Paint::Solid(c) => {
            let c = c.to_rgba8();
            Some([c.r, c.g, c.b, c.a])
        }
        _ => None,
    }
}

fn scale_of(t: &Affine) -> f64 {
    t.determinant().abs().sqrt().max(1e-9)
}

fn coeffs(t: &Affine) -> [u64; 6] {
    t.as_coeffs().map(f64::to_bits)
}

/// `input` (back to front) optimized under `settings`. `cancelled` is polled
/// between steps; a cancelled run returns what it has (callers drop it).
pub fn optimize_shapes(input: &[(Affine, ShapeData)], settings: &OptimizeSettings, cancelled: &dyn Fn() -> bool) -> Optimized {
    let n = input.len();
    let segments_before: usize = input.iter().map(|(_, s)| segment_count(&s.path)).sum();
    let boxes: Vec<Rect> = input.par_iter().map(|(_, s)| s.path.bounding_box()).collect();
    let mut colour: Vec<Option<[u8; 4]>> = input.iter().map(|(_, s)| solid(s)).collect();
    let mut dropped = vec![false; n];

    // --- Remove tiny: recoloured to the largest touching neighbour, or gone.
    if settings.remove_tiny > 0.0 {
        let areas: Vec<f64> = input
            .par_iter()
            .map(|(t, s)| if solid(s).is_some() { s.path.area().abs() * t.determinant().abs() } else { f64::INFINITY })
            .collect();
        let tiny: Vec<usize> = (0..n).filter(|&i| areas[i] < settings.remove_tiny).collect();
        let found: Vec<Option<usize>> = tiny
            .par_iter()
            .map(|&i| {
                let pad = 1.0 / scale_of(&input[i].0);
                let b = boxes[i].inflate(pad, pad);
                let key = coeffs(&input[i].0);
                (0..n)
                    .filter(|&j| {
                        j != i
                            && areas[j] >= settings.remove_tiny
                            && areas[j].is_finite()
                            && coeffs(&input[j].0) == key
                            && !(boxes[j].x1 < b.x0 || boxes[j].x0 > b.x1 || boxes[j].y1 < b.y0 || boxes[j].y0 > b.y1)
                    })
                    .max_by(|&a, &b| areas[a].total_cmp(&areas[b]))
            })
            .collect();
        for (&i, near) in tiny.iter().zip(found) {
            match near {
                Some(j) => colour[i] = colour[j],
                None => dropped[i] = true,
            }
        }
    }
    let paint_of = |i: usize| -> ShapeData {
        let mut s = input[i].1.clone();
        if let (Some(c), Some(fill)) = (colour[i], s.fill.as_mut()) {
            fill.paint = Paint::Solid(peniko::Color::from_rgba8(c[0], c[1], c[2], c[3]));
        }
        s
    };
    if cancelled() {
        return Optimized { shapes_before: n, segments_before, ..Default::default() };
    }

    // --- Merge same colour: components of bounding-box-touching shapes.
    let mut group_of: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    if settings.merge_same_colour {
        let mut keyed: Vec<(([u8; 4], [u64; 6]), usize)> = (0..n)
            .filter(|&i| !dropped[i] && colour[i].is_some() && input[i].1.fill.as_ref().is_some_and(|f| f.rule == FillMode::NonZero))
            .map(|i| ((colour[i].unwrap_or_default(), coeffs(&input[i].0)), i))
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0).then(boxes[a.1].x0.total_cmp(&boxes[b.1].x0)));
        let mut start = 0;
        while start < keyed.len() {
            let mut end = start;
            while end < keyed.len() && keyed[end].0 == keyed[start].0 {
                end += 1;
            }
            let pad = 0.5 / scale_of(&input[keyed[start].1].0);
            // Sweep along x: each shape against those still reaching it.
            let mut active: Vec<usize> = Vec::new();
            for &(_, i) in &keyed[start..end] {
                active.retain(|&j| boxes[j].x1 + pad >= boxes[i].x0);
                for &j in &active {
                    if boxes[j].y1 + pad >= boxes[i].y0 && boxes[j].y0 - pad <= boxes[i].y1 {
                        let (a, b) = (find(&mut group_of, i), find(&mut group_of, j));
                        if a != b {
                            group_of[a.max(b)] = a.min(b);
                        }
                    }
                }
                active.push(i);
            }
            start = end;
        }
    }
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        if dropped[i] {
            continue;
        }
        let g = find(&mut group_of, i);
        members[g].push(i);
    }
    let comps: Vec<Vec<usize>> = members.into_iter().filter(|m| !m.is_empty()).collect();

    // --- One piece per component (unions in parallel), then the refit.
    let merged_fit = settings.smoothing.max(MERGED_FIT_PX);
    let mut built: Vec<(Vec<usize>, Affine, ShapeData, bool)> = Vec::with_capacity(comps.len());
    let shapes: Vec<(Vec<usize>, ShapeData)> = comps
        .iter()
        .map(|m| {
            let mut first = paint_of(m[0]);
            if m.len() > 1 {
                first.path = BezPath::new();
            }
            (m.clone(), first)
        })
        .collect();
    for (m, shape) in shapes {
        built.push((m.clone(), input[m[0]].0, shape, m.len() > 1));
    }
    built.par_iter_mut().for_each(|(m, _, shape, merged)| {
        if *merged {
            let paths: Vec<BezPath> = m.iter().map(|&i| input[i].1.path.clone()).collect();
            let diag = m
                .iter()
                .map(|&i| boxes[i])
                .reduce(|a, b| a.union(b))
                .map_or(1.0, |r| r.width().hypot(r.height()));
            let options = BooleanOptions { fill: FillMode::NonZero, ..BooleanOptions::for_shape_size(diag) }.fast();
            let union = boolean_many(&paths, BoolOp::Union, options);
            shape.path = if union.elements().is_empty() { input[m[0]].1.path.clone() } else { union };
        }
    });
    if cancelled() {
        return Optimized { shapes_before: n, segments_before, ..Default::default() };
    }
    built.par_iter_mut().for_each(|(_, transform, shape, merged)| {
        let fit = if *merged { merged_fit } else { settings.smoothing };
        if fit <= 0.0 || !(shape.fill.as_ref().is_none_or(|f| matches!(f.paint, Paint::Solid(_)))) {
            return;
        }
        let before = segment_count(&shape.path);
        if before == 0 || before > MAX_REFIT_SEGMENTS {
            return;
        }
        let fitted = optimize_path(&shape.path, fit / scale_of(transform));
        if segment_count(&fitted) < before || *merged {
            // A union is polygon or curve already; take the fit when it is smaller.
            if segment_count(&fitted) <= before {
                shape.path = fitted;
            }
        }
    });
    // A union that came out with more segments than its members had is not an
    // optimization: those stay as they were (each refitted alone).
    let mut pieces: Vec<Piece> = Vec::with_capacity(built.len());
    for (sources, transform, shape, merged) in built {
        let own: usize = sources.iter().map(|&i| segment_count(&input[i].1.path)).sum();
        if merged && segment_count(&shape.path) > own {
            let alone: Vec<Piece> = sources
                .par_iter()
                .map(|&i| {
                    let mut shape = paint_of(i);
                    let before = segment_count(&shape.path);
                    if settings.smoothing > 0.0 && before > 0 && before <= MAX_REFIT_SEGMENTS {
                        let fitted = optimize_path(&shape.path, settings.smoothing / scale_of(&input[i].0));
                        if segment_count(&fitted) < before {
                            shape.path = fitted;
                        }
                    }
                    Piece { transform: input[i].0, shape, sources: vec![i] }
                })
                .collect();
            pieces.extend(alone);
        } else {
            pieces.push(Piece { transform, shape, sources });
        }
    }
    pieces.sort_by_key(|p| p.sources[0]);
    let segments_after = pieces.iter().map(|p| segment_count(&p.shape.path)).sum();
    Optimized { pieces, segments_before, segments_after, shapes_before: n }
}
