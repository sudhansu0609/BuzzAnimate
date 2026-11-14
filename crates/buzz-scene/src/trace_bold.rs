//! **Bold outlines** (`docs/TRACE_PICTURE_PLAN.md` T15): thick dark outlines
//! like the FLA files (`ghonsaar2.fla`, `ghonsaar5.fla`; measured in T15a).
//!
//! The FLAs outline object silhouettes and major edges in black, about 3 px
//! at 1920 px wide (1-1.5 px for detail), and leave flat shading bands inside
//! a wall or the ground without a line. A traced picture gets the same: its
//! ink as strokes of that weight, plus a stroke where two neighbouring colour
//! regions differ by more than the outline contrast ([`edge_strokes`], and
//! `trace::build_outline_strokes` for a trace in progress). Small closed
//! loops (beads, bulbs) get none, thin or short lines get the detail weight.

use crate::object::{Paint, ShapeData};
use crate::trace::{TraceOptions, srgb_to_lab};
use buzz_geom::{Affine, Point, Rect};
use peniko::Color;
use std::collections::HashMap;

/// The FLAs' outline weight, in pixels of a picture 1920 px wide (T15a).
pub const BOLD_WEIGHT_PX: f64 = 3.0;
/// ... and the detail weight, as a share of it (the FLAs' p10 of 1 px).
const DETAIL_SHARE: f64 = 0.5;

/// The line colour of the FLAs.
pub fn bold_colour() -> Color {
    Color::from_rgb8(0, 0, 0)
}

/// The bold weight for a picture whose long edge is `long_edge` px.
pub fn bold_width(long_edge: f64) -> f64 {
    BOLD_WEIGHT_PX * long_edge.max(1.0) / 1920.0
}

/// Trace Bold draws its lines at this share of [`bold_width`] (the user, 2026-11-14: the FLA weight
/// was too heavy on traced backgrounds). Even Lines keeps the full weight.
pub const TRACE_BOLD_SHARE: f64 = 0.6;

/// The thin weight for detail.
pub fn detail_width(width: f64) -> f64 {
    width * DETAIL_SHARE
}

/// A line this long (picture px) at weight `width` is worth drawing, and at
/// which weight: `None` for a speck of a loop or a stub, the detail weight for a
/// short one, else `width`.
pub fn edge_width(length: f64, closed: bool, width: f64) -> Option<f64> {
    let (drop_below, detail_below) = if closed { (12.0, 25.0) } else { (3.0, 10.0) };
    if length < drop_below * width {
        None
    } else if length < detail_below * width {
        Some(detail_width(width))
    } else {
        Some(width)
    }
}

/// A region thinner than this (2 x area / perimeter, px) is a thin thing (a
/// garland string, a pole) whose edges get the detail weight.
pub fn is_thin_limit(width: f64) -> f64 {
    4.0 * width
}

/// The length of a path, in its own units.
pub fn path_length(path: &buzz_geom::BezPath) -> f64 {
    let mut length = 0.0;
    let mut last: Option<Point> = None;
    kurbo::flatten(path.iter(), 0.5, |el| match el {
        kurbo::PathEl::MoveTo(p) => last = Some(p),
        kurbo::PathEl::LineTo(p) => {
            if let Some(l) = last {
                length += (p - l).hypot();
            }
            last = Some(p);
        }
        _ => {}
    });
    length
}

/// Fill `shape` (non-zero winding) into `labels` at `scale` from `origin`.
fn fill_shape(labels: &mut [u16], w: usize, h: usize, origin: Point, scale: f64, shape: &ShapeData, label: u16) {
    let to_raster = Affine::scale(scale) * Affine::translate((-origin.x, -origin.y));
    let path = to_raster * shape.path.clone();
    let bbox = kurbo::Shape::bounding_box(&path);
    let (y0, y1) = ((bbox.y0.floor().max(0.0)) as usize, (bbox.y1.ceil().min(h as f64)) as usize);
    if y1 <= y0 {
        return;
    }
    // Crossings per row: (x, winding direction).
    let mut rows: Vec<Vec<(f64, i32)>> = vec![Vec::new(); y1 - y0];
    let mut start = Point::ORIGIN;
    let mut last = Point::ORIGIN;
    let mut edge = |a: Point, b: Point| {
        if (a.y - b.y).abs() < 1e-12 {
            return;
        }
        let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
        let first = ((lo.y - 0.5).ceil().max(y0 as f64)) as usize;
        let mut y = first;
        while (y as f64 + 0.5) < hi.y && y < y1 {
            let t = ((y as f64 + 0.5) - lo.y) / (hi.y - lo.y);
            rows[y - y0].push((lo.x + (hi.x - lo.x) * t, dir));
            y += 1;
        }
    };
    kurbo::flatten(path.iter(), 0.25, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if last != start {
                edge(last, start);
            }
            start = p;
            last = p;
        }
        kurbo::PathEl::LineTo(p) => {
            edge(last, p);
            last = p;
        }
        kurbo::PathEl::ClosePath => {
            edge(last, start);
            last = start;
        }
        _ => {}
    });
    if last != start {
        edge(last, start);
    }
    for (i, crossings) in rows.iter_mut().enumerate() {
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0;
        for k in 0..crossings.len() {
            winding += crossings[k].1;
            if winding != 0 && k + 1 < crossings.len() {
                let xa = (crossings[k].0 - 0.5).ceil().max(0.0) as usize;
                let xb = ((crossings[k + 1].0 - 0.5).ceil().max(0.0) as usize).min(w);
                for x in xa..xb {
                    labels[(y0 + i) * w + x] = label;
                }
            }
        }
    }
}

/// **Outline strokes for flat colour shapes** (Even Out Lines, bold): `fills`
/// painted in order (opaque solid-ish fills, in one space) are rasterised, the
/// pixels of `ink` (shapes about to become strokes) are painted over from their
/// neighbours, and a stroke of `width` and `colour` is drawn wherever two
/// regions differ by more than `contrast` (CIELAB ΔE). In the shapes' space.
pub fn edge_strokes(fills: &[ShapeData], ink: &[ShapeData], bounds: Rect, width: f64, colour: Color, contrast: f64) -> Vec<ShapeData> {
    let long = bounds.width().max(bounds.height());
    if long < 8.0 || fills.is_empty() {
        return Vec::new();
    }
    // One raster pixel per unit up to 1920 px, no more.
    let scale = (1920.0 / long).min(1.0);
    let (w, h) = ((bounds.width() * scale).ceil() as usize + 1, (bounds.height() * scale).ceil() as usize + 1);
    let origin = Point::new(bounds.x0, bounds.y0);

    // Palette: the fills' own colours (a trace has a few dozen); 5 bits a channel past 250.
    let mut ids: HashMap<[u8; 3], u16> = HashMap::new();
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut crush = false;
    for shape in fills {
        let c = shape.fill.as_ref().map(|f| f.color().to_rgba8()).map(|c| [c.r, c.g, c.b]).unwrap_or([0; 3]);
        if !ids.contains_key(&c) {
            ids.insert(c, palette.len() as u16);
            palette.push(c);
            if palette.len() > 250 {
                crush = true;
                break;
            }
        }
    }
    if crush {
        ids.clear();
        palette.clear();
    }
    let key = |c: [u8; 3]| if crush { [c[0] & 0xf8 | 4, c[1] & 0xf8 | 4, c[2] & 0xf8 | 4] } else { c };
    let none = u16::MAX;
    let mut labels = vec![none; w * h];
    for shape in fills {
        let c = shape.fill.as_ref().map(|f| f.color().to_rgba8()).map(|c| [c.r, c.g, c.b]).unwrap_or([0; 3]);
        let c = key(c);
        let id = *ids.entry(c).or_insert_with(|| {
            palette.push(c);
            (palette.len() - 1) as u16
        });
        if palette.len() > 255 {
            continue;
        }
        fill_shape(&mut labels, w, h, origin, scale, shape, id);
    }

    // The ink's pixels (and a little round them) are not a region of their own:
    // they take the nearest region's label, up to the ink's own reach.
    let mut hole = vec![false; w * h];
    let mut mask = vec![u16::MAX; w * h];
    for shape in ink {
        fill_shape(&mut mask, w, h, origin, scale, shape, 0);
    }
    for i in 0..w * h {
        if mask[i] == 0 {
            hole[i] = true;
            labels[i] = none;
        }
    }
    let reach = ((2.5 * width * scale).ceil() as usize).clamp(2, 24);
    let mut frontier: Vec<usize> = (0..w * h).filter(|&i| hole[i]).collect();
    for _ in 0..reach {
        if frontier.is_empty() {
            break;
        }
        let mut filled: Vec<(usize, u16)> = Vec::new();
        let mut rest: Vec<usize> = Vec::new();
        for &at in &frontier {
            let (x, y) = (at % w, at / w);
            let mut got = none;
            for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let l = labels[ny as usize * w + nx as usize];
                if l != none && !hole[ny as usize * w + nx as usize] {
                    got = l;
                    break;
                }
            }
            if got != none {
                filled.push((at, got));
            } else {
                rest.push(at);
            }
        }
        if filled.is_empty() {
            break;
        }
        for (at, l) in filled {
            labels[at] = l;
            hole[at] = false;
        }
        frontier = rest;
    }

    // Connected regions of one label, for `build_outline_strokes`.
    let mut region_list: Vec<(u8, Vec<usize>)> = Vec::new();
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if seen[start] || labels[start] == none {
            continue;
        }
        let label = labels[start];
        let mut cells = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(at) = stack.pop() {
            cells.push(at);
            let (x, y) = (at % w, at / w);
            for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let n = ny as usize * w + nx as usize;
                if !seen[n] && labels[n] == label {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        region_list.push((label.min(255) as u8, cells));
    }
    let palette_rgb: Vec<[u8; 3]> = palette.iter().take(256).copied().collect();
    let palette_lab: Vec<[f64; 3]> = palette_rgb.iter().map(|&c| srgb_to_lab(c)).collect();
    let options = TraceOptions {
        bold_outlines: true,
        uniform_width: Some(width * scale),
        line_colour: Some(colour),
        outline_contrast: contrast,
        ..TraceOptions::default()
    };
    let strokes = crate::trace::build_outline_strokes(&region_list, w, h, &palette_rgb, &palette_lab, &options, 1.0);
    let back = Affine::translate((origin.x, origin.y)) * Affine::scale(1.0 / scale);
    strokes
        .into_iter()
        .map(|mut s| {
            s.path = back * s.path;
            if let Some(stroke) = s.stroke.as_mut() {
                stroke.width /= scale;
                if let Paint::Solid(_) = stroke.paint {
                    stroke.paint = Paint::Solid(colour);
                }
            }
            s
        })
        .collect()
}
