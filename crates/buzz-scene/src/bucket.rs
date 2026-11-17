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
use std::collections::{HashMap, HashSet};

/// How large a gap in the outline the bucket will bridge before flooding.
///
/// Animate's Gap Size, with two more stops at each end so line art of any
/// weight has a setting that closes it without swallowing detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
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
    /// The fill is even-odd: where its contours overlap is a hole (traced
    /// line art often marks its holes so), not more ink.
    pub even_odd: bool,
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
            even_odd: shape.fill.as_ref().is_some_and(|f| f.rule == FillMode::EvenOdd),
        }
    }
}

/// Cap on the working grid's larger dimension, so a huge canvas still floods in
/// bounded time and memory.
const MAX_GRID: f64 = 2048.0;
/// The resolution the bucket works at wherever the grid allows: eight pixels
/// a document unit, so a hole two units wide (a door handle's slot in the
/// user's traced line art, 2026-11-17) is sixteen pixels across.
const MAX_PPU: f64 = 8.0;
/// Half the first window's side, in document units: most fills (an eye, a
/// sleeve) are enclosed well inside it, at full resolution.
const FIRST_WINDOW: f64 = 96.0;
/// How thick, in grid pixels, the thinnest wall is drawn. At a pixel and a
/// half every row and column a line crosses holds a pixel centre of it, so the
/// line is an unbroken wall to a flood that steps between edge neighbours.
const MIN_WALL_PX: f64 = 1.5;

/// Flood-fill the region containing `seed`, bounded by `boundaries`.
///
/// Returns the filled region as a path in **document space**, or `None` when
/// the seed sits on a boundary, or the region is not enclosed (the flood
/// reached the edge of the artwork) — which is Animate's behaviour: it refuses
/// to fill what is not closed rather than flooding the stage.
///
/// **Worked out where the click is, at full detail** (user 2026-11-17: the
/// bucket "only worked for close large gaps"). It used to rasterise every line
/// on the layer into one grid at most 900 pixels across: on a whole scene that
/// is two or three document units a pixel, so thin lines and the narrow ends
/// of brush strokes fell between pixel centres, the flood ran out through the
/// holes, and only a gap setting large enough to thicken every wall over them
/// filled anything. Now the flood starts in a window around the click at four
/// pixels a unit, and only a flood that reaches the window's edge tries again
/// in one four times the size, until the window holds all the artwork. Every
/// wall is drawn at least [`MIN_WALL_PX`] thick, so no line breaks up at any
/// resolution.
pub fn fill_region(boundaries: &[Boundary], seed: Point, gap: GapSize) -> Option<BezPath> {
    // Everything the boundaries cover, plus the seed, with a margin so a fill
    // can reach the outermost lines and reaching the edge means "leaked past
    // the artwork".
    let mut all = Rect::from_points(seed, seed);
    for b in boundaries {
        all = all.union(b.path.bounding_box());
    }
    if all.width() <= 0.0 || all.height() <= 0.0 {
        return None;
    }
    let margin = (all.width().max(all.height()) * 0.05).max(8.0);
    let all = all.inflate(margin, margin);

    // The gap closing needs room around the region to grow its walls in.
    let mut half = FIRST_WINDOW.max(gap.document_units() * 8.0);
    loop {
        let window = Rect::new(seed.x - half, seed.y - half, seed.x + half, seed.y + half).intersect(all);
        let whole = window.x0 <= all.x0 && window.y0 <= all.y0 && window.x1 >= all.x1 && window.y1 >= all.y1;
        match flood_window(boundaries, seed, gap.document_units(), window) {
            Flood::Filled(path) => return Some(path),
            Flood::Nothing => return None,
            // Out through the window's edge: look further, unless this was
            // already all the artwork — then the region is open.
            Flood::Escaped if whole => return None,
            Flood::Escaped => half *= 4.0,
        }
    }
}

/// The nearest pixel to `from` that the grown walls leave open, reached
/// through no real wall and no further than `reach` pixels: the click moved
/// off the grown edge of a line it was next to.
fn open_space_near(walls: &Grid, blocked: &dyn Fn(usize, usize) -> bool, from: (usize, usize), reach: f64) -> Option<(usize, usize)> {
    let (w, h) = (walls.w, walls.h);
    let mut seen = Grid::new(w, h);
    let mut queue = std::collections::VecDeque::from([from]);
    seen.set(from.0, from.1);
    while let Some((x, y)) = queue.pop_front() {
        if !blocked(x, y) {
            return Some((x, y));
        }
        let near = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)];
        for (nx, ny) in near {
            if nx >= w || ny >= h || seen.get(nx, ny) || walls.get(nx, ny) {
                continue;
            }
            let (dx, dy) = (nx as f64 - from.0 as f64, ny as f64 - from.1 as f64);
            if dx * dx + dy * dy > reach * reach {
                continue;
            }
            seen.set(nx, ny);
            queue.push_back((nx, ny));
        }
    }
    None
}

enum Flood {
    Filled(BezPath),
    /// The seed is on a wall, or nothing could be traced.
    Nothing,
    /// The flood reached the window's edge.
    Escaped,
}

/// Whether `p` is on the ink of any boundary: inside a fill (by its rule),
/// or within half a stroke of its line.
fn on_ink(boundaries: &[Boundary], p: Point) -> bool {
    boundaries.iter().any(|b| {
        let near = b.path.bounding_box().inflate(b.stroke_width.unwrap_or(0.0), b.stroke_width.unwrap_or(0.0));
        if !near.contains(p) {
            return false;
        }
        if b.filled {
            let w = b.path.winding(p);
            if if b.even_odd { w % 2 != 0 } else { w != 0 } {
                return true;
            }
        }
        b.stroke_width.is_some_and(|width| {
            let half = width / 2.0;
            subpaths(&b.path).iter().any(|sub| {
                let band = outline_stroke(sub, StrokeStyle::new(width), half.min(0.05));
                band.bounding_box().contains(p) && band.winding(p) != 0
            })
        })
    })
}

/// The nearest pixel to `from` that is no wall, at most `reach` pixels away.
fn nearest_open(walls: &Grid, from: (usize, usize), reach: usize) -> Option<(usize, usize)> {
    let (x0, y0) = (from.0 as isize, from.1 as isize);
    let mut best: Option<((usize, usize), isize)> = None;
    let r = reach as isize;
    for dy in -r..=r {
        for dx in -r..=r {
            let (x, y) = (x0 + dx, y0 + dy);
            if x < 0 || y < 0 || x as usize >= walls.w || y as usize >= walls.h || walls.get(x as usize, y as usize) {
                continue;
            }
            let d = dx * dx + dy * dy;
            if d <= r * r && best.is_none_or(|(_, b)| d < b) {
                best = Some(((x as usize, y as usize), d));
            }
        }
    }
    best.map(|(p, _)| p)
}

/// The empty pixels reached from `start` through pixels `blocked` leaves
/// open, stepping between edge neighbours; `None` when the flood reaches the
/// grid's edge.
fn flood(walls: &Grid, blocked: impl Fn(usize, usize) -> bool, start: (usize, usize)) -> Option<Grid> {
    let (w, h) = (walls.w, walls.h);
    let mut fill = Grid::new(w, h);
    let mut stack = vec![start];
    fill.set(start.0, start.1);
    while let Some((x, y)) = stack.pop() {
        if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
            return None;
        }
        for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
            if !fill.get(nx, ny) && !blocked(nx, ny) {
                fill.set(nx, ny);
                stack.push((nx, ny));
            }
        }
    }
    Some(fill)
}

/// **The area a gap-closed flood `core` stands for, out to its lines.**
///
/// The core stops `grow` short of every line. Growing it back by that much
/// rounds off every corner and loses every part narrower than the gap
/// setting (the round spots the user saw), so instead it flows back out
/// through the real empty space, sharing it with the cores of the
/// neighbouring areas: every empty pixel goes to whichever core reaches it
/// first. Corners are reached only from this one, so they fill; a gap is
/// reached from both sides, so the fill stops at its middle — where Animate
/// closes it. A pixel no further core claims is taken at most three times the
/// gap setting away (a sharp corner's tip), so a run of empty space with no
/// core of its own is not followed off round the drawing.
fn claim(walls: &Grid, to_walls: &[f64], grow: f64, core: &Grid) -> Grid {
    let (w, h) = (walls.w, walls.h);
    // 0 unclaimed, 1 this core, 2 another; and the steps it took.
    let mut owner = vec![0u8; w * h];
    let mut steps = vec![u32::MAX; w * h];
    let mut queue = std::collections::VecDeque::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if core.get(x, y) {
                owner[i] = 1;
                steps[i] = 0;
                queue.push_back((x, y));
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if owner[i] == 0 && to_walls[i] > grow {
                owner[i] = 2;
                steps[i] = 0;
                queue.push_back((x, y));
            }
        }
    }
    let cap = (grow * 3.0).ceil() as u32 + 2;
    while let Some((x, y)) = queue.pop_front() {
        let i = y * w + x;
        let (who, n) = (owner[i], steps[i]);
        if who == 1 && n >= cap {
            continue;
        }
        let near = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)];
        for (nx, ny) in near {
            if nx >= w || ny >= h || walls.get(nx, ny) {
                continue;
            }
            let j = ny * w + nx;
            if owner[j] == 0 {
                owner[j] = who;
                steps[j] = n + 1;
                queue.push_back((nx, ny));
            }
        }
    }
    let mut out = Grid::new(w, h);
    for y in 0..h {
        for x in 0..w {
            if owner[y * w + x] == 1 {
                out.set(x, y);
            }
        }
    }
    out
}

/// One flood of the window `area` at the finest resolution its size allows,
/// with walls grown by `gap` document units to close gaps.
fn flood_window(boundaries: &[Boundary], seed: Point, gap: f64, area: Rect) -> Flood {
    let extent = area.width().max(area.height());
    if extent <= 0.0 {
        return Flood::Nothing;
    }
    let ppu = (MAX_GRID / extent).min(MAX_PPU);
    let w = (area.width() * ppu).ceil() as usize + 1;
    let h = (area.height() * ppu).ceil() as usize + 1;
    if w < 3 || h < 3 {
        return Flood::Nothing;
    }
    let to_px = |p: Point| (((p.x - area.x0) * ppu) as isize, ((p.y - area.y0) * ppu) as isize);

    // ---- rasterise the walls --------------------------------------------------
    let hairline = MIN_WALL_PX / ppu;
    let reach = area.inflate(hairline, hairline);
    let mut walls = Grid::new(w, h);
    for b in boundaries {
        let bounds = b.path.bounding_box().inflate(b.stroke_width.unwrap_or(0.0), b.stroke_width.unwrap_or(0.0));
        if !bounds.overlaps(reach) {
            continue;
        }
        if b.filled {
            rasterise_fill_rule(&b.path, &mut walls, area, ppu, b.even_odd);
        }
        // **Each contour is stroked and rasterised on its own.**
        //
        // Stroking a whole path at once returns one outline holding the two
        // ring loops of every closed contour and the band of every open one.
        // Rasterising all of that together under the non-zero rule lets a
        // line's band *cancel* against a ring loop where the two cross — the
        // winding sums back to zero right at the junction and opens a
        // one-pixel gap the flood pours straight through. A contour is an
        // independent wall; stroked on its own and OR-ed into the grid, walls
        // only ever add and never rub each other out.
        //
        // A filled shape's edge is a wall too, a hairline thick: a brush
        // stroke is a filled outline, and where it tapers thinner than a pixel
        // its fill alone leaves holes.
        let width = match (b.stroke_width, b.filled) {
            (Some(width), _) => Some(width.max(hairline)),
            (None, true) => Some(hairline),
            (None, false) => None,
        };
        if let Some(width) = width {
            for sub in subpaths(&b.path) {
                let outline = outline_stroke(&sub, StrokeStyle::new(width), 0.25 / ppu);
                rasterise_fill(&outline, &mut walls, area, ppu);
            }
        }
    }

    // ---- flood the empty space ------------------------------------------------
    let (sx, sy) = to_px(seed);
    if sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
        return Flood::Nothing;
    }
    let (mut sx, mut sy) = (sx as usize, sy as usize);
    if walls.get(sx, sy) {
        // A click beside a line, not on its ink, lands on the pixel or two
        // every wall is thickened by (or on a pixel the line only grazes):
        // the flood starts from the open pixel nearest it. On the ink
        // itself, there is nothing to fill.
        if on_ink(boundaries, seed) {
            return Flood::Nothing;
        }
        match nearest_open(&walls, (sx, sy), 4) {
            Some((x, y)) => (sx, sy) = (x, y),
            None => return Flood::Nothing,
        }
    }

    // **A closed area is filled exactly, whatever the gap setting** (user
    // 2026-11-17: small areas came out as round spots). Gap closing is for
    // an outline that leaks; one that does not is filled to its lines, into
    // every corner and every narrow part.
    let fill = match flood(&walls, |x, y| walls.get(x, y), (sx, sy)) {
        Some(fill) => fill,
        None if gap <= 0.0 => return Flood::Escaped,
        None => {
            // Leaked: close the gaps. Walls grown by `grow` hold the flood
            // back from every gap up to twice that wide.
            let grow = gap * ppu;
            let to_walls = walls.distances();
            let blocked = |x: usize, y: usize| to_walls[y * w + x] <= grow;
            let (mut sx, mut sy) = (sx, sy);
            if blocked(sx, sy) {
                // **The gap closing never swallows the click.** Grown walls
                // cover a click near a line, and all of an area narrower than
                // the gap setting. Open space within reach, through no line,
                // means the first: the flood starts there. None means the
                // second: the area is closed with the largest gap it has
                // room for.
                match open_space_near(&walls, &blocked, (sx, sy), grow) {
                    Some((x, y)) => (sx, sy) = (x, y),
                    None => {
                        let room = to_walls[sy * w + sx] / ppu * 0.9;
                        if room * ppu < 1.0 {
                            return Flood::Escaped;
                        }
                        return flood_window(boundaries, seed, room, area);
                    }
                }
            }
            let Some(core) = flood(&walls, blocked, (sx, sy)) else {
                return Flood::Escaped;
            };
            claim(&walls, &to_walls, grow, &core)
        }
    };

    // **Exact**, cut from the lines themselves (`exact_fill`); the traced
    // pixels only where that cannot be done.
    let seed_at = Point::new(area.x0 + (sx as f64 + 0.5) / ppu, area.y0 + (sy as f64 + 0.5) / ppu);
    let seed_at = if on_ink(boundaries, seed) { seed_at } else { seed };
    if let Some(exact) = exact_fill(&grid_path(&fill, area, ppu), boundaries, seed_at, ppu) {
        return Flood::Filled(exact);
    }

    // One pixel more, **into the ink only**: the bucket's fill lies behind the
    // lines, so tucked under them no seam of paper shows between the two —
    // and it never crosses a line into the next area.
    let mut tucked = fill.clone();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            if walls.get(x, y) && (fill.get(x - 1, y) || fill.get(x + 1, y) || fill.get(x, y - 1) || fill.get(x, y + 1)) {
                tucked.set(x, y);
            }
        }
    }
    let path = grid_path(&tucked, area, ppu);
    if path.elements().is_empty() { Flood::Nothing } else { Flood::Filled(path) }
}

/// The most lines the exact pass cuts the fill from. Past this (a fill
/// across a whole background of thousands of traced strokes) the traced
/// fill is kept: the booleans would take seconds, and at that size the
/// pixel edge does not show.
const MAX_EXACT_INK: usize = 3000;

/// `grid`'s set pixels as closed loops in document space, the staircase
/// smoothed.
fn grid_path(grid: &Grid, area: Rect, ppu: f64) -> BezPath {
    let px_to_doc = |gx: usize, gy: usize| Point::new(area.x0 + gx as f64 / ppu, area.y0 + gy as f64 / ppu);
    let mut path = BezPath::new();
    for loop_pts in trace_contours(grid) {
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
    path
}

/// **The fill cut from the lines themselves, as Animate fills** (user
/// 2026-11-17: "use the way Animate works"). The pixel flood says which area
/// was clicked and where gaps close; its edge is a staircase a pixel off the
/// ink, which in a small area is most of the area. So the flooded area,
/// grown two pixels to reach over the ink it stopped short of, has the exact
/// ink — every fill by its own rule, every line at its width — taken out of
/// it, and the piece holding `seed` is the fill: it meets the lines exactly,
/// into every corner, and no further. A hair of it is then tucked under the
/// ink (only where there is ink), so no seam of paper shows between fill
/// and line. `None` when there is too much ink near to cut exactly, or the
/// cut leaves nothing at the click: the traced fill is used.
fn exact_fill(raster: &BezPath, boundaries: &[Boundary], seed: Point, ppu: f64) -> Option<BezPath> {
    use buzz_geom::{BoolOp, BooleanOptions, FillMode, boolean, boolean_parts, union_all};
    let px = 1.0 / ppu;
    let tol = (0.2 * px).clamp(0.005, 0.05);
    let opts = BooleanOptions { tolerance: tol, fill: FillMode::NonZero, refit_curves: false, ..BooleanOptions::default() };
    let even_odd = BooleanOptions { fill: FillMode::EvenOdd, ..opts };

    // The flooded area grown by two pixels.
    let reach = 2.0 * px;
    let mut pieces: Vec<BezPath> = vec![boolean(raster, raster, BoolOp::Union, even_odd)];
    for sub in subpaths(raster) {
        pieces.push(outline_stroke(&sub, StrokeStyle::new(2.0 * reach), tol));
    }
    let grown = union_all(&pieces, opts);
    let near = grown.bounding_box().inflate(reach, reach);

    // The ink near it, exactly.
    let mut ink: Vec<BezPath> = Vec::new();
    for b in boundaries {
        let half = b.stroke_width.unwrap_or(0.0);
        if !b.path.bounding_box().inflate(half, half).overlaps(near) {
            continue;
        }
        if b.filled {
            ink.push(if b.even_odd { boolean(&b.path, &b.path, BoolOp::Union, even_odd) } else { b.path.clone() });
        }
        if let Some(width) = b.stroke_width {
            for sub in subpaths(&b.path) {
                ink.push(outline_stroke(&sub, StrokeStyle::new(width), tol));
            }
        }
        if ink.len() > MAX_EXACT_INK {
            return None;
        }
    }
    if ink.is_empty() {
        return None;
    }
    let ink = union_all(&ink, opts);

    // The piece the click is in.
    let parts = boolean_parts(&grown, &ink, BoolOp::Difference, opts);
    let part = parts.into_iter().find(|p| p.winding(seed) != 0)?;

    // Tucked under the ink, never past it.
    let tuck = (0.75 * px).clamp(0.02, 0.25);
    let band: Vec<BezPath> = subpaths(&part).iter().map(|s| outline_stroke(s, StrokeStyle::new(2.0 * tuck), tol)).collect();
    let band = union_all(&band, opts);
    let under = boolean(&band, &ink, BoolOp::Intersect, opts);
    let filled = if under.elements().is_empty() { part } else { boolean(&part, &under, BoolOp::Union, opts) };
    (!filled.elements().is_empty()).then_some(filled)
}

/// The default fill rule for a bucket result: even-odd, so a traced hole reads
/// as a hole rather than being filled over.
pub const FILL_RULE: FillMode = FillMode::EvenOdd;

// ---------------------------------------------------------------------------
// A 1-bit grid
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct Grid {
    pub(crate) w: usize,
    pub(crate) h: usize,
    bits: Vec<bool>,
}

impl Grid {
    pub(crate) fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![false; w * h],
        }
    }

    #[inline]
    pub(crate) fn get(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.w + x]
    }

    #[inline]
    pub(crate) fn set(&mut self, x: usize, y: usize) {
        self.bits[y * self.w + x] = true;
    }

    /// Unset one pixel, so a scratch grid can be reused across many regions
    /// without reallocating one the size of the picture for each.
    #[inline]
    pub(crate) fn unset(&mut self, x: usize, y: usize) {
        self.bits[y * self.w + x] = false;
    }

    /// **How far each pixel is from the nearest set one**, in pixels: a
    /// two-pass chamfer distance (steps of 1 and √2), within a few per cent of
    /// the true distance and linear in the grid's size whatever the reach.
    /// Zero on a set pixel.
    fn distances(&self) -> Vec<f64> {
        const DIAG: f64 = std::f64::consts::SQRT_2;
        let (w, h) = (self.w, self.h);
        let mut d: Vec<f64> = self.bits.iter().map(|b| if *b { 0.0 } else { f64::INFINITY }).collect();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let mut v = d[i];
                if x > 0 {
                    v = v.min(d[i - 1] + 1.0);
                }
                if y > 0 {
                    v = v.min(d[i - w] + 1.0);
                    if x > 0 {
                        v = v.min(d[i - w - 1] + DIAG);
                    }
                    if x + 1 < w {
                        v = v.min(d[i - w + 1] + DIAG);
                    }
                }
                d[i] = v;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                let i = y * w + x;
                let mut v = d[i];
                if x + 1 < w {
                    v = v.min(d[i + 1] + 1.0);
                }
                if y + 1 < h {
                    v = v.min(d[i + w] + 1.0);
                    if x + 1 < w {
                        v = v.min(d[i + w + 1] + DIAG);
                    }
                    if x > 0 {
                        v = v.min(d[i + w - 1] + DIAG);
                    }
                }
                d[i] = v;
            }
        }
        d
    }

}

/// Split a path into its subpaths, one per `MoveTo`, so each contour can be
/// stroked and rasterised as the independent wall it is.
fn subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    let mut current = BezPath::new();
    for el in path.elements() {
        if matches!(el, buzz_geom::PathEl::MoveTo(_)) && !current.elements().is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push(*el);
    }
    if !current.elements().is_empty() {
        out.push(current);
    }
    out
}

/// Scan-convert a filled path into the grid with the non-zero rule.
pub(crate) fn rasterise_fill(path: &BezPath, grid: &mut Grid, area: Rect, ppu: f64) {
    rasterise_fill_rule(path, grid, area, ppu, false);
}

/// [`rasterise_fill`] under the even-odd rule when `even_odd`.
pub(crate) fn rasterise_fill_rule(path: &BezPath, grid: &mut Grid, area: Rect, ppu: f64, even_odd: bool) {
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

    // Edges by the first row they cross, so each row looks only at the edges
    // spanning it rather than every edge of the drawing.
    edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut next = 0;
    let mut active: Vec<usize> = Vec::new();
    let mut xs: Vec<(f64, i32)> = Vec::new();
    for py in 0..grid.h {
        let y = py as f64 + 0.5;
        while next < edges.len() && edges[next].y0 <= y {
            active.push(next);
            next += 1;
        }
        active.retain(|&i| edges[i].y1 > y);
        // Crossings this scanline, with winding direction.
        xs.clear();
        for &i in &active {
            let e = &edges[i];
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
            let inside = if even_odd { winding % 2 != 0 } else { winding != 0 };
            if inside {
                // A pixel is in when its **centre** is, as the rows are
                // sampled. It used to take only pixels the span covered edge
                // to edge, so a line under two pixels thick could miss every
                // pixel of a row and leave a hole the bucket leaked through.
                let x_start = (pair[0].0 - 0.5).max(0.0).ceil() as usize;
                let x_end = (pair[1].0 - 0.5).min(grid.w as f64).ceil().max(0.0) as usize;
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
///
/// Winding: the fill is always on the walker's right, so an outer loop runs
/// clockwise on screen (positive shoelace area in y-down grid space) and a
/// hole the other way.
///
/// **Corner contacts.** Two set pixels that touch only at a corner leave that
/// grid corner with *two* outgoing edges (and two incoming). A one-edge-per-
/// corner map keeps only the later of them, so the walk loses an edge and
/// closes its loop with a straight chord across the shape. Each corner here
/// holds every outgoing direction, and the walk takes, at a corner, the
/// leftmost way on that is available (left turn, else straight, else right).
/// At a diagonal contact that always crosses from one pixel to the other, so
/// pixels of one region that touch at a corner stay one connected piece (as
/// the fill is 8-connected and the hole 4-connected, which keeps a hole a
/// hole), the choice depends only on the heading in so it cannot vary from run
/// to run, and corners without a contact have one way on and behave exactly
/// as before.
pub(crate) fn trace_contours(grid: &Grid) -> Vec<Vec<(usize, usize)>> {
    // Headings, clockwise on screen: 0 right, 1 down, 2 left, 3 up.
    const DX: [isize; 4] = [1, 0, -1, 0];
    const DY: [isize; 4] = [0, 1, 0, -1];

    // The directed edges leaving each grid corner, as a bit per heading, walking
    // the fill so the interior is on a consistent side.
    let mut out: HashMap<(usize, usize), u8> = HashMap::new();
    let filled = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && (x as usize) < grid.w
            && (y as usize) < grid.h
            && grid.get(x as usize, y as usize)
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
                *out.entry((x, y)).or_insert(0) |= 1 << 0; // top, left→right
            }
            if !filled(xi + 1, yi) {
                *out.entry((x + 1, y)).or_insert(0) |= 1 << 1; // right, top→bottom
            }
            if !filled(xi, yi + 1) {
                *out.entry((x + 1, y + 1)).or_insert(0) |= 1 << 2; // bottom, right→left
            }
            if !filled(xi - 1, yi) {
                *out.entry((x, y + 1)).or_insert(0) |= 1 << 3; // left, bottom→top
            }
        }
    }

    // **Loops are started in raster order, not in whatever order the map
    // happens to hand back.**
    //
    // `HashMap` iteration is seeded per instance, so taking the first entry
    // made the *same* picture trace differently between two runs: the loops
    // came out rotated to a different starting corner, and Douglas–Peucker
    // then kept a slightly different set of points. Nothing was visibly wrong
    // either time, which is what made it worth pinning down — a bucket fill,
    // and now a trace, has to be reproducible or the same document saved twice
    // is two different documents.
    let mut starts: Vec<((usize, usize), usize)> = Vec::new();
    for (&p, &mask) in &out {
        for d in 0..4 {
            if mask & (1 << d) != 0 {
                starts.push((p, d));
            }
        }
    }
    starts.sort_unstable_by_key(|&((x, y), d)| (y, x, d));

    // Edges already walked; `out` itself is left whole so the turn choice at a
    // corner never depends on what has been used.
    let mut used: HashSet<((usize, usize), usize)> = HashSet::new();
    let mut loops = Vec::new();
    for (start, start_d) in starts {
        if used.contains(&(start, start_d)) {
            continue;
        }
        let mut loop_pts = Vec::new();
        let (mut p, mut d) = (start, start_d);
        loop {
            used.insert((p, d));
            loop_pts.push(p);
            let n = (
                (p.0 as isize + DX[d]) as usize,
                (p.1 as isize + DY[d]) as usize,
            );
            let Some(&mask) = out.get(&n) else { break };
            // Leftmost way on: left turn, else straight, else right.
            let Some(nd) = [(d + 3) % 4, d, (d + 1) % 4]
                .into_iter()
                .find(|&nd| mask & (1 << nd) != 0)
            else {
                break;
            };
            p = n;
            d = nd;
            // Closed when the next edge is the one the loop began with — not
            // merely when the start *corner* is reached again, which at a
            // corner contact happens mid-loop.
            if (p, d) == (start, start_d) {
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
pub(crate) fn simplify(points: &[(usize, usize)], epsilon: f64) -> Vec<(usize, usize)> {
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
    use kurbo::Circle;

    /// A closed square outline: clicking inside fills, and the filled region is
    /// roughly the square's interior.
    #[test]
    fn a_closed_outline_fills_inside() {
        let square = Rect::new(20.0, 20.0, 120.0, 120.0);
        let boundary = Boundary {
            path: square.to_path(0.01),
            filled: false, even_odd: false,
            stroke_width: Some(2.0),
        };
        let region = fill_region(&[boundary], Point::new(70.0, 70.0), GapSize::None)
            .expect("a closed outline should fill");
        let bbox = region.bounding_box();
        // The fill lands inside the square, close to its full extent.
        assert!(bbox.x0 >= 18.0 && bbox.y0 >= 18.0, "{bbox:?}");
        assert!(bbox.x1 <= 122.0 && bbox.y1 <= 122.0, "{bbox:?}");
        assert!(
            bbox.width() > 80.0 && bbox.height() > 80.0,
            "too small: {bbox:?}"
        );
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
            filled: false, even_odd: false,
            stroke_width: Some(2.0),
        };

        // No closing: the flood escapes through the gap.
        assert!(
            fill_region(&[boundary(&path)], Point::new(70.0, 70.0), GapSize::None).is_none(),
            "an open outline must not fill"
        );

        // A large enough gap size bridges the ~30-unit opening.
        let filled = fill_region(
            &[boundary(&path)],
            Point::new(70.0, 70.0),
            GapSize::ExtraLarge,
        );
        assert!(filled.is_some(), "the gap should close at Extra Large");
    }

    /// **A divider welded into the outline still blocks the flood.**
    ///
    /// A boundary whose path holds the closed rectangle *and* an open line
    /// across it — as it does once an outline and a divider have merged into
    /// one shape — must fill only the half the seed is in. Stroking the whole
    /// path at once and rasterising it under the non-zero rule let the line's
    /// band cancel against the rectangle ring where they cross, opening a gap
    /// the flood escaped through into the far half; each contour is now stroked
    /// on its own so the walls only ever add.
    #[test]
    fn a_divider_welded_into_the_outline_still_blocks_the_flood() {
        let mut combined = Rect::new(100.0, 100.0, 300.0, 220.0).to_path(0.01);
        combined.move_to(Point::new(80.0, 160.0)); // a line clean across, past both edges
        combined.line_to(Point::new(320.0, 160.0));
        let boundary = Boundary {
            path: combined,
            filled: false, even_odd: false,
            stroke_width: Some(1.0),
        };
        let region = fill_region(&[boundary], Point::new(200.0, 130.0), GapSize::None)
            .expect("the top half is enclosed and should fill");
        // Only the top half: the divider held the flood back from the bottom.
        assert!(
            region.winding(Point::new(200.0, 130.0)) != 0,
            "the clicked half"
        );
        assert!(
            region.winding(Point::new(200.0, 190.0)) == 0,
            "the flood must not have leaked into the other half"
        );
        let bbox = region.bounding_box();
        assert!(
            bbox.y1 <= 162.0,
            "the fill reached past the divider: {bbox:?}"
        );
    }

    /// Clicking on the line itself fills nothing.
    #[test]
    fn clicking_on_the_boundary_fills_nothing() {
        let square = Rect::new(20.0, 20.0, 120.0, 120.0);
        let boundary = Boundary {
            path: square.to_path(0.01),
            filled: false, even_odd: false,
            stroke_width: Some(4.0),
        };
        assert!(
            fill_region(&[boundary], Point::new(20.0, 70.0), GapSize::None).is_none(),
            "the seed sat on the outline"
        );
    }

    /// The rest of a scene: a frame of line art 2000 units across, so the
    /// layer's artwork is as big as a real shot's.
    fn a_whole_scene() -> Boundary {
        Boundary { path: Rect::new(0.0, 0.0, 2000.0, 1100.0).to_path(0.01), filled: false, even_odd: false, stroke_width: Some(2.0) }
    }

    /// A closed ring of radius `r` at `c`, drawn as a brush stroke would be: a
    /// filled outline `width` thick, no stroke — with a gap of `gap` units cut
    /// out of its right side when `gap > 0`.
    fn brush_ring(c: Point, r: f64, width: f64, gap: f64) -> Boundary {
        let mut spine = BezPath::new();
        let open = if gap > 0.0 { (gap / 2.0 / r).asin() } else { 0.0 };
        let steps = 96;
        for i in 0..=steps {
            let a = open + (std::f64::consts::TAU - 2.0 * open) * i as f64 / steps as f64;
            let p = Point::new(c.x + r * a.cos(), c.y + r * a.sin());
            if i == 0 { spine.move_to(p) } else { spine.line_to(p) }
        }
        if gap <= 0.0 {
            spine.close_path();
        }
        Boundary { path: outline_stroke(&spine, StrokeStyle::new(width), 0.01), filled: true, even_odd: false, stroke_width: None }
    }

    /// **A small region in a whole scene fills at every gap size** (user
    /// 2026-11-17: the bucket "only worked for close large gaps"). An eye 12
    /// units across, ringed by a thin stroke or a brush stroke thinner than
    /// a unit, in a shot 2000 units wide: the old single 900-pixel grid was
    /// two units a pixel there, the ring fell apart and only Large or more
    /// filled. Every setting fills it now, and only inside the ring.
    #[test]
    fn a_small_thin_lined_region_in_a_whole_scene_fills_at_every_gap_size() {
        let eye = Point::new(1000.0, 500.0);
        let mut thin = Circle::new(eye, 6.0).to_path(0.01);
        thin.close_path();
        let rings = [
            ("a stroke half a unit wide", Boundary { path: thin, filled: false, even_odd: false, stroke_width: Some(0.5) }),
            ("a brush stroke 0.4 units thick", brush_ring(eye, 6.0, 0.4, 0.0)),
        ];
        for (what, ring) in rings {
            let walls = [a_whole_scene(), ring];
            for gap in GapSize::ALL {
                let region = fill_region(&walls, eye, gap).unwrap_or_else(|| panic!("{what}, {gap:?}: nothing filled"));
                let bbox = region.bounding_box();
                assert!(bbox.width() < 14.0 && bbox.height() < 14.0, "{what}, {gap:?}: it leaked past the ring: {bbox:?}");
                if gap <= GapSize::Small {
                    assert!(bbox.width() > 10.0, "{what}, {gap:?}: the fill should reach the ring: {bbox:?}");
                }
            }
        }
    }

    /// **The gap settings close what they say, in a whole scene**: a ring
    /// whose ink stops 6 units short (a 7.5-unit cut less its round ends)
    /// leaks with Don't Close Gaps and Extra Small (which bridges 4), and
    /// fills from Small (8) up — staying inside the ring.
    #[test]
    fn each_gap_size_closes_its_gap_in_a_whole_scene() {
        let c = Point::new(700.0, 400.0);
        let walls = [a_whole_scene(), brush_ring(c, 30.0, 1.5, 7.5)];
        for gap in GapSize::ALL {
            let region = fill_region(&walls, c, gap);
            if gap <= GapSize::ExtraSmall {
                // Leaked: out into the whole scene, which is what the frame encloses.
                assert!(region.as_ref().is_none_or(|r| r.bounding_box().width() > 1000.0), "{gap:?} should not close a 6-unit gap");
            } else {
                let bbox = region.unwrap_or_else(|| panic!("{gap:?} should close a 6-unit gap")).bounding_box();
                assert!(bbox.width() < 66.0, "{gap:?}: leaked: {bbox:?}");
            }
        }
    }

    /// **A big region still fills** — the sky of the whole frame, far wider
    /// than the first window — and a click outside all the artwork fills
    /// nothing.
    #[test]
    fn a_region_wider_than_the_first_window_fills_and_open_space_does_not() {
        let walls = [a_whole_scene(), brush_ring(Point::new(1000.0, 500.0), 6.0, 0.4, 0.0)];
        let started = std::time::Instant::now();
        let sky = fill_region(&walls, Point::new(300.0, 300.0), GapSize::Medium).expect("the frame's inside fills");
        let bbox = sky.bounding_box();
        assert!(bbox.width() > 1900.0 && bbox.height() > 1000.0, "{bbox:?}");
        assert!(sky.winding(Point::new(1000.0, 500.0)) == 0, "the eye is a hole in it, not painted over");
        assert!(started.elapsed().as_secs_f64() < 3.0, "took {:?}", started.elapsed());
        assert!(fill_region(&walls, Point::new(-50.0, -50.0), GapSize::None).is_none(), "outside everything");
    }

    /// The area a fill covers, counted on a fine grid inside `within`.
    fn covered(region: &BezPath, within: Rect) -> f64 {
        let step = 0.1;
        let mut n = 0usize;
        let mut y = within.y0 + step / 2.0;
        while y < within.y1 {
            let mut x = within.x0 + step / 2.0;
            while x < within.x1 {
                if region.winding(Point::new(x, y)) != 0 {
                    n += 1;
                }
                x += step;
            }
            y += step;
        }
        n as f64 * step * step
    }

    fn thin_lines(points: &[Point], closed: bool, width: f64) -> Boundary {
        let mut path = BezPath::new();
        path.move_to(points[0]);
        for p in &points[1..] {
            path.line_to(*p);
        }
        if closed {
            path.close_path();
        }
        Boundary { path, filled: false, even_odd: false, stroke_width: Some(width) }
    }

    /// **A tiny area fills right into its corners at every gap size** (user
    /// 2026-11-17: very small areas came out as round spots). A triangle 8
    /// units a side in a whole scene: the gap closing used to shrink it to a
    /// dot and grow that back round.
    #[test]
    fn a_tiny_sharp_cornered_area_fills_into_its_corners() {
        let (a, b, c) = (Point::new(1000.0, 500.0), Point::new(1008.0, 500.0), Point::new(1004.0, 507.0));
        let walls = [a_whole_scene(), thin_lines(&[a, b, c], true, 0.3)];
        let inside = Point::new(1004.0, 502.5);
        // The triangle's own area, less the half line width round it.
        let full = 0.5 * 8.0 * 7.0;
        for gap in GapSize::ALL {
            let region = fill_region(&walls, inside, gap).unwrap_or_else(|| panic!("{gap:?}: nothing filled"));
            let got = covered(&region, Rect::new(999.0, 499.0, 1009.0, 508.0));
            assert!(got > full * 0.85, "{gap:?}: only {got:.1} of {full} filled — corners left out");
            let bbox = region.bounding_box();
            assert!(bbox.x0 > 999.5 && bbox.x1 < 1008.5 && bbox.y0 > 499.5 && bbox.y1 < 507.5, "{gap:?}: spilled: {bbox:?}");
            // Near a corner, not only round the middle.
            assert!(region.winding(Point::new(1001.2, 500.5)) != 0, "{gap:?}: the left corner is empty");
        }
    }

    /// **A tiny area with a tiny gap fills, whole, and spills nowhere**: a
    /// 10-unit box whose side has a 1.5-unit opening, with Small gaps (which
    /// close 8): all of the box, nothing outside it but the gap's own mouth.
    #[test]
    fn a_tiny_area_with_a_gap_fills_whole_without_spilling() {
        let box_ = [Point::new(1010.0, 500.0), Point::new(1000.0, 500.0), Point::new(1000.0, 510.0), Point::new(1010.0, 510.0), Point::new(1010.0, 505.75)];
        let walls = [a_whole_scene(), thin_lines(&box_, false, 0.4), thin_lines(&[Point::new(1010.0, 504.25), Point::new(1010.0, 500.0)], false, 0.4)];
        let region = fill_region(&walls, Point::new(1005.0, 505.0), GapSize::Small).expect("the box fills");
        let got = covered(&region, Rect::new(1000.0, 500.0, 1010.0, 510.0));
        assert!(got > 100.0 * 0.85, "only {got:.1} of 100 filled");
        let bbox = region.bounding_box();
        assert!(bbox.x0 > 999.5 && bbox.y0 > 499.5 && bbox.y1 < 510.5 && bbox.x1 < 1011.0, "spilled out: {bbox:?}");
        assert!(region.winding(Point::new(1000.6, 509.4)) != 0, "a corner is empty");
    }

    /// **A thin line between two tiny cells keeps them apart**: filling one
    /// leaves the other empty, at every gap size.
    #[test]
    fn a_thin_line_keeps_two_tiny_cells_apart() {
        let outline = thin_lines(&[Point::new(1000.0, 500.0), Point::new(1012.0, 500.0), Point::new(1012.0, 506.0), Point::new(1000.0, 506.0)], true, 0.3);
        let divider = thin_lines(&[Point::new(1006.0, 500.0), Point::new(1006.0, 506.0)], false, 0.2);
        let walls = [a_whole_scene(), outline, divider];
        for gap in GapSize::ALL {
            let region = fill_region(&walls, Point::new(1003.0, 503.0), gap).unwrap_or_else(|| panic!("{gap:?}: nothing filled"));
            assert!(region.winding(Point::new(1009.0, 503.0)) == 0, "{gap:?}: spilled into the next cell");
            assert!(region.bounding_box().x1 < 1006.3, "{gap:?}: crossed the line: {:?}", region.bounding_box());
            assert!(covered(&region, Rect::new(1000.0, 500.0, 1006.0, 506.0)) > 36.0 * 0.8, "{gap:?}: the cell is not filled");
        }
    }

    /// **The fill meets the lines exactly** (user 2026-11-17: "use the way
    /// Animate works"): inside a ring drawn 2 units wide, the fill is the
    /// disc out to the ink's inner edge — its area right to within a per
    /// cent, at every gap size — and reaches no further than a hair under the
    /// ink.
    #[test]
    fn the_fill_meets_the_lines_exactly() {
        let c = Point::new(1000.0, 500.0);
        let walls = [a_whole_scene(), Boundary { path: Circle::new(c, 20.0).to_path(0.001), filled: false, even_odd: false, stroke_width: Some(2.0) }];
        let inner = std::f64::consts::PI * 19.0 * 19.0;
        for gap in GapSize::ALL {
            let region = fill_region(&walls, c, gap).unwrap_or_else(|| panic!("{gap:?}: nothing filled"));
            let area = region.area().abs();
            assert!(area > inner * 0.995 && area < inner * 1.04, "{gap:?}: {area:.1} where the inside is {inner:.1}");
            let bbox = region.bounding_box();
            assert!(bbox.width() < 38.6 && bbox.width() > 37.9, "{gap:?}: edge {bbox:?}");
        }
    }

    /// **Traced line art, the way the user's door is drawn**: the lines are
    /// one filled black shape whose holes are the small areas between them.
    /// A click in a hole 1.5 units wide — near its edge too — fills that
    /// hole exactly, at every gap size.
    #[test]
    fn a_small_hole_in_traced_line_art_fills_exactly() {
        let mut art = Rect::new(990.0, 490.0, 1030.0, 530.0).to_path(0.01);
        let hole = Rect::new(1000.0, 500.0, 1001.5, 515.0);
        // The hole wound the other way, as a traced outline's holes are.
        art.move_to(Point::new(hole.x0, hole.y0));
        art.line_to(Point::new(hole.x0, hole.y1));
        art.line_to(Point::new(hole.x1, hole.y1));
        art.line_to(Point::new(hole.x1, hole.y0));
        art.close_path();
        let walls = [a_whole_scene(), Boundary { path: art, filled: true, even_odd: false, stroke_width: None }];
        for gap in GapSize::ALL {
            for x in [1000.75, 1000.15, 1001.35] {
                let region = fill_region(&walls, Point::new(x, 507.0), gap).unwrap_or_else(|| panic!("{gap:?} at {x}: nothing filled"));
                let area = region.area().abs();
                // The hole, and at most a quarter-unit tuck under the ink round its 33-unit edge.
                assert!(area > 22.5 * 0.97 && area < 22.5 + 33.0 * 0.25, "{gap:?} at {x}: {area:.2} for a hole of 22.5");
                let bbox = region.bounding_box();
                assert!(bbox.x0 > 999.7 && bbox.x1 < 1001.8 && bbox.y0 > 499.7 && bbox.y1 < 515.3, "{gap:?} at {x}: {bbox:?}");
            }
        }
    }

    #[test]
    fn gap_sizes_increase() {
        let sizes = GapSize::ALL.map(|g| g.document_units());
        for pair in sizes.windows(2) {
            assert!(pair[1] > pair[0], "gap sizes must increase: {sizes:?}");
        }
    }

    // ---- contour tracing ------------------------------------------------------

    /// A grid from rows of text: `X` is a set pixel, anything else is unset.
    fn grid_from(rows: &[&str]) -> Grid {
        let w = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut grid = Grid::new(w, rows.len());
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.bytes().enumerate() {
                if c == b'X' {
                    grid.set(x, y);
                }
            }
        }
        grid
    }

    fn is_set(grid: &Grid, x: isize, y: isize) -> bool {
        x >= 0 && y >= 0 && (x as usize) < grid.w && (y as usize) < grid.h && grid.get(x as usize, y as usize)
    }

    /// Twice the signed (shoelace) area of a loop, in y-down grid space, so an
    /// outer loop is positive and a hole negative.
    fn twice_area(points: &[(usize, usize)]) -> i64 {
        let n = points.len();
        (0..n)
            .map(|i| {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % n];
                x0 as i64 * y1 as i64 - x1 as i64 * y0 as i64
            })
            .sum()
    }

    /// Trace `rows` and hold the result to the true pixel boundary: every step
    /// is one unit along a grid line with the set pixel on its right and an
    /// unset one on its left, no edge is used twice, every boundary edge of
    /// every set pixel is used, and so the signed areas add up to exactly the
    /// pixel count.
    fn trace_exact(rows: &[&str]) -> Vec<Vec<(usize, usize)>> {
        let grid = grid_from(rows);
        let loops = trace_contours(&grid);

        let mut seen = std::collections::HashSet::new();
        let mut edges = 0usize;
        let mut twice = 0i64;
        for lp in &loops {
            assert!(lp.len() >= 4, "a loop too short to be a shape: {lp:?}");
            for i in 0..lp.len() {
                let (a, b) = (lp[i], lp[(i + 1) % lp.len()]);
                let (ax, ay) = (a.0 as isize, a.1 as isize);
                let (dx, dy) = (b.0 as isize - ax, b.1 as isize - ay);
                assert_eq!(dx.abs() + dy.abs(), 1, "a chord, not a pixel edge: {a:?} -> {b:?}");
                let (right, left) = match (dx, dy) {
                    (1, 0) => ((ax, ay), (ax, ay - 1)),
                    (0, 1) => ((ax - 1, ay), (ax, ay)),
                    (-1, 0) => ((ax - 1, ay - 1), (ax - 1, ay)),
                    _ => ((ax, ay - 1), (ax - 1, ay - 1)),
                };
                assert!(is_set(&grid, right.0, right.1), "no fill on the right of {a:?} -> {b:?}");
                assert!(!is_set(&grid, left.0, left.1), "fill on the left of {a:?} -> {b:?}");
                assert!(seen.insert((a, b)), "edge {a:?} -> {b:?} used twice");
                edges += 1;
            }
            twice += twice_area(lp);
        }

        let mut boundary = 0usize;
        let mut count = 0i64;
        for y in 0..grid.h as isize {
            for x in 0..grid.w as isize {
                if !is_set(&grid, x, y) {
                    continue;
                }
                count += 1;
                for (nx, ny) in [(x, y - 1), (x + 1, y), (x, y + 1), (x - 1, y)] {
                    if !is_set(&grid, nx, ny) {
                        boundary += 1;
                    }
                }
            }
        }
        assert_eq!(edges, boundary, "a boundary edge was lost: {loops:?}");
        assert_eq!(twice, 2 * count, "area is not the pixel count: {loops:?}");
        loops
    }

    /// Two pixels of one region meeting only at a corner are one piece joined
    /// there, traced as one loop of eight edges — not a loop closed by a chord.
    #[test]
    fn a_diagonal_pair_of_pixels_traces_every_edge() {
        for rows in [["X.", ".X"], [".X", "X."]] {
            let loops = trace_exact(&rows);
            assert_eq!(loops.len(), 1, "{rows:?}: {loops:?}");
            assert_eq!(loops[0].len(), 8, "{rows:?}: {loops:?}");
        }
    }

    #[test]
    fn a_checkerboard_traces_every_edge() {
        trace_exact(&["X.X.", ".X.X", "X.X.", ".X.X"]);
        trace_exact(&[".X.X", "X.X.", ".X.X", "X.X."]);
    }

    /// An L of pixels with another piece touching its outer corner.
    #[test]
    fn an_l_meeting_another_piece_at_a_corner_traces_every_edge() {
        let loops = trace_exact(&["X...", "X...", "XX..", "..XX"]);
        assert_eq!(loops.len(), 1, "{loops:?}");
        // And the other way round, so both diagonals and both turn senses at
        // the pinch are met.
        trace_exact(&["...X", "...X", "..XX", "XX.."]);
        trace_exact(&["XX..", "..XX", "...X", "...X"]);
        trace_exact(&["..XX", "XX..", "X...", "X..."]);
    }

    /// A ring of pixels closed only by corner contacts: the outer boundary and
    /// the hole stay separate loops, the hole winding the other way.
    #[test]
    fn a_ring_touching_itself_at_corners_traces_every_edge() {
        let loops = trace_exact(&[".X.", "X.X", ".X."]);
        let mut areas: Vec<i64> = loops.iter().map(|l| twice_area(l)).collect();
        areas.sort_unstable();
        assert_eq!(areas, vec![-2, 10], "{loops:?}");

        // A thicker ring with one side closed by a corner contact only.
        trace_exact(&["XXXX.", "X..XX", "X..X.", "XXX.."]);
    }

    /// A region with a hole that reaches the outside only through a corner:
    /// the hole is still a hole, and the area is the pixel count.
    #[test]
    fn a_hole_touching_the_outside_at_a_corner_traces_every_edge() {
        let loops = trace_exact(&[".XXX.", ".X.X.", ".XX.."]);
        let mut areas: Vec<i64> = loops.iter().map(|l| twice_area(l)).collect();
        areas.sort_unstable();
        assert_eq!(areas, vec![-2, 16], "{loops:?}");
    }

    /// Without any corner contact the walk is what it always was: loops in
    /// raster order of their first corner, the outer loop clockwise and a hole
    /// the other way.
    #[test]
    fn a_region_without_corner_contacts_traces_as_before() {
        let loops = trace_exact(&["XXX", "X.X", "XXX"]);
        assert_eq!(
            loops,
            vec![
                vec![
                    (0, 0), (1, 0), (2, 0), (3, 0), (3, 1), (3, 2),
                    (3, 3), (2, 3), (1, 3), (0, 3), (0, 2), (0, 1),
                ],
                vec![(1, 1), (1, 2), (2, 2), (2, 1)],
            ]
        );
    }
}
