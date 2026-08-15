//! The paint bucket's flood fill: colouring the enclosed region under the
//! pointer, the way Animate's bucket fills the area between lines.
//!
//! # Why raster, not vector
//!
//! Finding "the region bounded by these strokes and fills, containing this
//! point" is a planar-subdivision problem that is miserable to do exactly on
//! arbitrary overlapping Béziers. The honest, robust way — and the way paint
//! tools have always done it — is to rasterise the boundaries into a grid,
//! flood the empty pixels from the seed, and trace the result back to a path.
//! It runs on the CPU in a few milliseconds and has no pathological cases.
//!
//! # Gap closing
//!
//! Hand-drawn line art rarely closes perfectly: two strokes almost meet, and a
//! naïve flood leaks out through the gap and fills the whole canvas. Animate's
//! **Gap Size** dilates the walls before flooding so a small gap is bridged; we
//! do the same, then grow the fill back to the real lines so the colour still
//! meets the ink rather than stopping short of it.

use crate::object::ShapeData;
use buzz_geom::{Affine, BezPath, FillMode, Point, Rect, Shape as _, StrokeStyle, outline_stroke};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How large a gap in the outline the bucket will bridge before flooding.
///
/// Animate's Gap Size, with two more stops at each end so line art of any
/// weight has a setting that closes it without swallowing detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GapSize {
    /// Fill only a perfectly closed region — leak out of any gap. Animate's
    /// "Don't Close Gaps".
    #[default]
    None,
    ExtraSmall,
    Small,
    Medium,
    Large,
    ExtraLarge,
}

impl GapSize {
    /// The bridging distance, in **document units**. A gap up to twice this is
    /// closed, because both of its sides grow inward by this much.
    pub fn document_units(self) -> f64 {
        match self {
            GapSize::None => 0.0,
            GapSize::ExtraSmall => 2.0,
            GapSize::Small => 4.0,
            GapSize::Medium => 8.0,
            GapSize::Large => 16.0,
            GapSize::ExtraLarge => 32.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GapSize::None => "Don't close gaps",
            GapSize::ExtraSmall => "Extra small",
            GapSize::Small => "Small",
            GapSize::Medium => "Medium",
            GapSize::Large => "Large",
            GapSize::ExtraLarge => "Extra large",
        }
    }

    /// Every setting, for a menu.
    pub const ALL: [GapSize; 6] = [
        GapSize::None,
        GapSize::ExtraSmall,
        GapSize::Small,
        GapSize::Medium,
        GapSize::Large,
        GapSize::ExtraLarge,
    ];
}

/// One boundary the fill must respect: a shape's filled area, its stroke, or
/// both, in document space.
pub struct Boundary {
    pub path: BezPath,
    /// The shape has a fill, so its whole interior blocks the flood.
    pub filled: bool,
    /// The shape has a stroke of this width, whose ink blocks the flood.
    pub stroke_width: Option<f64>,
}

impl Boundary {
    /// The boundaries a layer's shapes present to the bucket. Instances, groups
    /// and rigs are left out: the bucket fills flat drawing, which is where the
    /// question even makes sense.
    pub fn from_shape(shape: &ShapeData, transform: Affine) -> Self {
        Self {
            path: transform * shape.path.clone(),
            filled: shape.fill.is_some(),
            stroke_width: shape.stroke.as_ref().map(|s| s.width.max(0.75)),
        }
    }
}

/// Cap on the working grid's larger dimension, so a huge canvas still floods in
/// bounded time and memory.
const MAX_GRID: u32 = 900;
/// Cap on resolution, so a tiny region does not build an enormous grid.
const MAX_PPU: f64 = 4.0;

/// Flood-fill the region containing `seed`, bounded by `boundaries`.
///
/// Returns the filled region as a path in **document space**, or `None` when
/// the seed sits on a boundary, or the region is not enclosed (the flood
/// reached the edge of the working area) — which is Animate's behaviour: it
/// refuses to fill what is not closed rather than flooding the stage.
pub fn fill_region(boundaries: &[Boundary], seed: Point, gap: GapSize) -> Option<BezPath> {
    // The working area: everything the boundaries cover, plus the seed, with a
    // margin so a fill can reach the outermost lines and the border test means
    // "leaked past the artwork".
    let mut area = Rect::from_points(seed, seed);
    for b in boundaries {
        area = area.union(b.path.bounding_box());
    }
    if area.width() <= 0.0 || area.height() <= 0.0 {
        return None;
    }
    let margin = (area.width().max(area.height()) * 0.05).max(8.0);
    let area = area.inflate(margin, margin);

    // Resolution: fit the larger extent into MAX_GRID, but never magnify a small
    // region past MAX_PPU.
    let extent = area.width().max(area.height());
    let ppu = (MAX_GRID as f64 / extent).min(MAX_PPU);
    let w = (area.width() * ppu).ceil() as usize + 1;
    let h = (area.height() * ppu).ceil() as usize + 1;
    if w == 0 || h == 0 || w * h > (MAX_GRID as usize + 2).pow(2) {
        return None;
    }

    let to_px = |p: Point| {
        (
            ((p.x - area.x0) * ppu) as isize,
            ((p.y - area.y0) * ppu) as isize,
        )
    };

    // ---- rasterise the walls --------------------------------------------------
    let mut walls = Grid::new(w, h);
    for b in boundaries {
        if b.filled {
            rasterise_fill(&b.path, &mut walls, area, ppu);
        }
        if let Some(width) = b.stroke_width {
            let outline = outline_stroke(&b.path, StrokeStyle::new(width), 0.25 / ppu);
            rasterise_fill(&outline, &mut walls, area, ppu);
        }
    }

    // Gap closing: grow the walls so a gap up to twice the setting is bridged.
    let grow = (gap.document_units() * ppu).round() as usize;
    let flood_walls = if grow > 0 {
        walls.dilated(grow)
    } else {
        walls.clone()
    };

    // ---- flood the empty space ------------------------------------------------
    let (sx, sy) = to_px(seed);
    if sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
        return None;
    }
    let (sx, sy) = (sx as usize, sy as usize);
    if flood_walls.get(sx, sy) {
        // Clicked on a line, or inside a gap the closing has now sealed as wall.
        return None;
    }

    let mut fill = Grid::new(w, h);
    let mut touched_border = false;
    let mut stack = vec![(sx, sy)];
    fill.set(sx, sy);
    while let Some((x, y)) = stack.pop() {
        if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
            touched_border = true;
        }
        let push = |nx: usize, ny: usize, fill: &mut Grid, stack: &mut Vec<(usize, usize)>| {
            if !fill.get(nx, ny) && !flood_walls.get(nx, ny) {
                fill.set(nx, ny);
                stack.push((nx, ny));
            }
        };
        if x > 0 {
            push(x - 1, y, &mut fill, &mut stack);
        }
        if x < w - 1 {
            push(x + 1, y, &mut fill, &mut stack);
        }
        if y > 0 {
            push(x, y - 1, &mut fill, &mut stack);
        }
        if y < h - 1 {
            push(x, y + 1, &mut fill, &mut stack);
        }
    }

    // Not enclosed: the flood escaped to the edge of the working area, so there
    // is no region to fill. Animate refuses here too.
    if touched_border {
        return None;
    }

    // Grow the fill back to the real lines: the dilated walls held the flood
    // short of the ink by `grow`, so grow the fill by the same and then clear
    // the original walls, leaving colour that meets the lines.
    let fill = if grow > 0 {
        let mut grown = fill.dilated(grow);
        grown.subtract(&walls);
        grown
    } else {
        fill
    };

    // ---- trace the region back to a path -------------------------------------
    let loops = trace_contours(&fill);
    if loops.is_empty() {
        return None;
    }

    let px_to_doc = |gx: usize, gy: usize| {
        Point::new(area.x0 + gx as f64 / ppu, area.y0 + gy as f64 / ppu)
    };

    let mut path = BezPath::new();
    for loop_pts in loops {
        let simplified = simplify(&loop_pts, 0.75);
        if simplified.len() < 3 {
            continue;
        }
        let mut points = simplified.into_iter().map(|(gx, gy)| px_to_doc(gx, gy));
        if let Some(first) = points.next() {
            path.move_to(first);
            for p in points {
                path.line_to(p);
            }
            path.close_path();
        }
    }

    (!path.elements().is_empty()).then_some(path)
}

/// The default fill rule for a bucket result: even-odd, so a traced hole reads
/// as a hole rather than being filled over.
pub const FILL_RULE: FillMode = FillMode::EvenOdd;

// ---------------------------------------------------------------------------
// A 1-bit grid
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Grid {
    w: usize,
    h: usize,
    bits: Vec<bool>,
}

impl Grid {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![false; w * h],
        }
    }

    #[inline]
    fn get(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.w + x]
    }

    #[inline]
    fn set(&mut self, x: usize, y: usize) {
        self.bits[y * self.w + x] = true;
    }

    /// Everything within `r` pixels of a set pixel, by `r` passes of a 4-neighbour
    /// grow. `r` is small (a gap size in pixels), so this stays cheap.
    fn dilated(&self, r: usize) -> Grid {
        let mut current = self.clone();
        for _ in 0..r {
            let mut next = current.clone();
            for y in 0..self.h {
                for x in 0..self.w {
                    if current.get(x, y) {
                        continue;
                    }
                    let n = (x > 0 && current.get(x - 1, y))
                        || (x + 1 < self.w && current.get(x + 1, y))
                        || (y > 0 && current.get(x, y - 1))
                        || (y + 1 < self.h && current.get(x, y + 1));
                    if n {
                        next.set(x, y);
                    }
                }
            }
            current = next;
        }
        current
    }

    /// Clear every pixel set in `other`.
    fn subtract(&mut self, other: &Grid) {
        for (b, o) in self.bits.iter_mut().zip(&other.bits) {
            if *o {
                *b = false;
            }
        }
    }
}

/// Scan-convert a filled path into the grid with the non-zero rule.
fn rasterise_fill(path: &BezPath, grid: &mut Grid, area: Rect, ppu: f64) {
    // Flatten into polylines, in grid coordinates.
    let tol = 0.2 / ppu;
    let mut subpaths: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    kurbo::flatten(path.iter(), tol, |el| match el {
        buzz_geom::PathEl::MoveTo(p) => {
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            current.push(p);
        }
        buzz_geom::PathEl::LineTo(p) => current.push(p),
        buzz_geom::PathEl::ClosePath => {
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
        _ => {}
    });
    if current.len() > 1 {
        subpaths.push(current);
    }

    // Edges, in grid space.
    struct Edge {
        y0: f64,
        y1: f64,
        x_at_y0: f64,
        dxdy: f64,
        dir: i32,
    }
    let mut edges: Vec<Edge> = Vec::new();
    let gx = |p: Point| (p.x - area.x0) * ppu;
    let gy = |p: Point| (p.y - area.y0) * ppu;
    for sub in &subpaths {
        for i in 0..sub.len() {
            let a = sub[i];
            let b = sub[(i + 1) % sub.len()];
            let (ax, ay) = (gx(a), gy(a));
            let (bx, by) = (gx(b), gy(b));
            if (ay - by).abs() < f64::EPSILON {
                continue;
            }
            let (top, bot, dir) = if ay < by {
                ((ax, ay), (bx, by), 1)
            } else {
                ((bx, by), (ax, ay), -1)
            };
            edges.push(Edge {
                y0: top.1,
                y1: bot.1,
                x_at_y0: top.0,
                dxdy: (bot.0 - top.0) / (bot.1 - top.1),
                dir,
            });
        }
    }
    if edges.is_empty() {
        return;
    }

    for py in 0..grid.h {
        let y = py as f64 + 0.5;
        // Crossings this scanline, with winding direction.
        let mut xs: Vec<(f64, i32)> = Vec::new();
        for e in &edges {
            if y >= e.y0 && y < e.y1 {
                xs.push((e.x_at_y0 + (y - e.y0) * e.dxdy, e.dir));
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0;
        for pair in xs.windows(2) {
            winding += pair[0].1;
            if winding != 0 {
                let x_start = pair[0].0.max(0.0).ceil() as usize;
                let x_end = (pair[1].0.min(grid.w as f64)).floor() as usize;
                for px in x_start..x_end.min(grid.w) {
                    grid.set(px, py);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Contour tracing
// ---------------------------------------------------------------------------

/// Trace the boundary of the set pixels into closed loops of grid points.
///
/// Directed boundary edges are collected — each set pixel contributes an edge
/// on every side where its neighbour is unset — and chained end-to-end into
/// loops. Interior holes fall out as their own loops.
fn trace_contours(grid: &Grid) -> Vec<Vec<(usize, usize)>> {
    // A directed edge from one grid corner to the next, walking the fill so the
    // interior is on a consistent side.
    let mut next: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    let filled = |x: isize, y: isize| {
        x >= 0 && y >= 0 && (x as usize) < grid.w && (y as usize) < grid.h && grid.get(x as usize, y as usize)
    };
    for y in 0..grid.h {
        for x in 0..grid.w {
            if !grid.get(x, y) {
                continue;
            }
            let (xi, yi) = (x as isize, y as isize);
            // Corners of this pixel, clockwise: (x,y) (x+1,y) (x+1,y+1) (x,y+1).
            // A side is a boundary when the neighbour across it is unset. Emit
            // it directed so the fill is on the right.
            if !filled(xi, yi - 1) {
                next.insert((x, y), (x + 1, y)); // top, left→right
            }
            if !filled(xi + 1, yi) {
                next.insert((x + 1, y), (x + 1, y + 1)); // right, top→bottom
            }
            if !filled(xi, yi + 1) {
                next.insert((x + 1, y + 1), (x, y + 1)); // bottom, right→left
            }
            if !filled(xi - 1, yi) {
                next.insert((x, y + 1), (x, y)); // left, bottom→top
            }
        }
    }

    let mut loops = Vec::new();
    while let Some((&start, _)) = next.iter().next() {
        let mut loop_pts = Vec::new();
        let mut p = start;
        loop {
            loop_pts.push(p);
            let Some(&n) = next.get(&p) else { break };
            next.remove(&p);
            p = n;
            if p == start {
                break;
            }
        }
        if loop_pts.len() >= 4 {
            loops.push(loop_pts);
        }
    }
    loops
}

/// Drop collinear points, then Douglas–Peucker to smooth the staircase the
/// pixel grid produces, with `epsilon` in grid pixels.
fn simplify(points: &[(usize, usize)], epsilon: f64) -> Vec<(usize, usize)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    // Collinear removal first — cheap, and it takes long straight runs down to
    // their endpoints before the recursive pass.
    let pts: Vec<(f64, f64)> = points.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
    let kept = douglas_peucker(&pts, epsilon);
    kept.into_iter()
        .map(|(x, y)| (x.round() as usize, y.round() as usize))
        .collect()
}

fn douglas_peucker(points: &[(f64, f64)], epsilon: f64) -> Vec<(f64, f64)> {
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

fn perpendicular_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f64::EPSILON {
        return ((p.0 - a.0).powi(2) + (p.1 - a.1).powi(2)).sqrt();
    }
    ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;

    /// A closed square outline: clicking inside fills, and the filled region is
    /// roughly the square's interior.
    #[test]
    fn a_closed_outline_fills_inside() {
        let square = Rect::new(20.0, 20.0, 120.0, 120.0);
        let boundary = Boundary {
            path: square.to_path(0.01),
            filled: false,
            stroke_width: Some(2.0),
        };
        let region = fill_region(&[boundary], Point::new(70.0, 70.0), GapSize::None)
            .expect("a closed outline should fill");
        let bbox = region.bounding_box();
        // The fill lands inside the square, close to its full extent.
        assert!(bbox.x0 >= 18.0 && bbox.y0 >= 18.0, "{bbox:?}");
        assert!(bbox.x1 <= 122.0 && bbox.y1 <= 122.0, "{bbox:?}");
        assert!(bbox.width() > 80.0 && bbox.height() > 80.0, "too small: {bbox:?}");
    }

    /// A square with a gap in one side leaks with no gap closing, and fills once
    /// the gap size covers it.
    #[test]
    fn a_gap_leaks_until_it_is_closed() {
        // Three sides and two stubs, leaving a ~10-unit gap in the right edge.
        let mut path = BezPath::new();
        path.move_to(Point::new(20.0, 20.0));
        path.line_to(Point::new(120.0, 20.0));
        path.line_to(Point::new(120.0, 55.0)); // stub down
        path.move_to(Point::new(120.0, 85.0)); // gap from 55 to 85
        path.line_to(Point::new(120.0, 120.0));
        path.line_to(Point::new(20.0, 120.0));
        path.line_to(Point::new(20.0, 20.0));

        let boundary = |p: &BezPath| Boundary {
            path: p.clone(),
            filled: false,
            stroke_width: Some(2.0),
        };

        // No closing: the flood escapes through the gap.
        assert!(
            fill_region(&[boundary(&path)], Point::new(70.0, 70.0), GapSize::None).is_none(),
            "an open outline must not fill"
        );

        // A large enough gap size bridges the ~30-unit opening.
        let filled = fill_region(&[boundary(&path)], Point::new(70.0, 70.0), GapSize::ExtraLarge);
        assert!(filled.is_some(), "the gap should close at Extra Large");
    }

    /// Clicking on the line itself fills nothing.
    #[test]
    fn clicking_on_the_boundary_fills_nothing() {
        let square = Rect::new(20.0, 20.0, 120.0, 120.0);
        let boundary = Boundary {
            path: square.to_path(0.01),
            filled: false,
            stroke_width: Some(4.0),
        };
        assert!(
            fill_region(&[boundary], Point::new(20.0, 70.0), GapSize::None).is_none(),
            "the seed sat on the outline"
        );
    }

    #[test]
    fn gap_sizes_increase() {
        let sizes = GapSize::ALL.map(|g| g.document_units());
        for pair in sizes.windows(2) {
            assert!(pair[1] > pair[0], "gap sizes must increase: {sizes:?}");
        }
    }
}
