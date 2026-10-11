//! The dotted hatch a selected raw shape wears — Animate's selection pattern.
//!
//! # Why a hatch, and why this one
//!
//! Animate does not box a selected fill or line. It dots the artwork itself, so
//! what is selected is the *shape*, and a thin line or a hole in a ring reads as
//! exactly that. The box this replaced said "something in here" — and, round a
//! ring or an L, said it about a great deal of empty stage.
//!
//! # Why it is cheap
//!
//! Nothing here is a boolean operation, a tessellation or a bitmap. A fill is
//! **scanned**: the outline is flattened, each row of a screen-aligned lattice
//! asks which stretches of it lie inside the shape, and the lattice's dots in
//! those stretches are kept. The work is one pass over the outline's edges plus
//! one dot per dot drawn — and the dots are held to [`MAX_DOTS`] a frame by
//! widening the lattice, so a selection of a thousand shapes costs what a
//! selection of a few does.
//!
//! Everything is in **screen pixels**: the lattice is screen-aligned and a dot is
//! a dot at every zoom. The pattern is anchored to the screen, not to the shape,
//! as Animate's is, which is also what keeps it from needing to be rebuilt as a
//! shape is dragged — there is nothing to cache.
//!
//! The geometry is plain `Pos2`s in and out, and the module draws nothing; the
//! stage turns the dots into one mesh.

use buzz_geom::Point;
use egui::{Color32, Mesh, Pos2, Rect, Vec2};

use crate::editor::DrawnShape;

/// Distance between neighbouring dots, in screen pixels, on a hatch that is not
/// being thinned for the budget.
pub const PITCH: f32 = 4.0;

/// The side of one dot, in screen pixels.
pub const DOT: f32 = 1.5;

/// The most dots a frame's hatch will lay down. A dot is a quad (four vertices),
/// so this is also what bounds the mesh handed to the painter.
pub const MAX_DOTS: usize = 40_000;

/// The narrowest a stroke's hatched band is made, in screen pixels. A hairline
/// is one pixel wide and a one-pixel dotted line would be lost on it.
const MIN_STROKE_BAND: f32 = 3.0;

/// The lattice spacing that keeps hatching `area` square pixels inside
/// [`MAX_DOTS`]: the ordinary pitch, widened only as far as it has to be.
///
/// `area` is the pixels the selection's shapes cover (their boxes, clipped to
/// the view, is a fair upper bound), so a few large shapes and a great many
/// small ones are both held to the same total.
pub fn pitch_for(area: f32) -> f32 {
    if !area.is_finite() || area <= 0.0 {
        return PITCH;
    }
    PITCH.max((area / MAX_DOTS as f32).sqrt())
}

/// The dots of the lattice that fall **inside a filled shape**, within `clip`.
///
/// `rings` are the shape's outlines flattened to polygons in screen pixels, one
/// per subpath; each is closed implicitly, as a fill closes it. `nonzero` is the
/// shape's fill rule: under it a ring wound against another makes a hole, under
/// even-odd any ring inside another does.
///
/// The lattice is the one [`PITCH`] describes: rows `pitch` apart, the dots in a
/// row `pitch` apart, and every other row shifted half a step so the dots read
/// as a dotted hatch rather than as a grid.
pub fn fill_dots(rings: &[Vec<Pos2>], nonzero: bool, clip: Rect, pitch: f32, out: &mut Vec<Pos2>) {
    if !(pitch > 0.0) || !pitch.is_finite() {
        return;
    }
    // The rows that can matter: those in the shape's own extent and the view's.
    let (mut top, mut bottom) = (f32::INFINITY, f32::NEG_INFINITY);
    for ring in rings {
        for p in ring {
            if p.x.is_finite() && p.y.is_finite() {
                top = top.min(p.y);
                bottom = bottom.max(p.y);
            }
        }
    }
    let top = top.max(clip.min.y);
    let bottom = bottom.min(clip.max.y);
    if !(top <= bottom) {
        return;
    }
    let first = (top / pitch).ceil() as i64;
    let last = (bottom / pitch).floor() as i64;
    if last < first {
        return;
    }

    // **Every crossing of a row with an edge, found once.** An edge is visited a
    // single time and drops a crossing on each lattice row it spans, so the work
    // is the outline's length in rows — not rows times edges. A row's edge is
    // half open (`lo.y <= y < hi.y`), so a vertex exactly on a row is counted
    // once by the two edges that meet there, and a horizontal edge is none.
    let mut crossings: Vec<(u32, f32, i32)> = Vec::new();
    for ring in rings {
        let n = ring.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            if !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite())
                || a.y == b.y
            {
                continue;
            }
            let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            let k0 = ((lo.y / pitch).ceil() as i64).max(first);
            let k1 = (((hi.y / pitch).ceil() as i64) - 1).min(last);
            for k in k0..=k1 {
                let y = k as f32 * pitch;
                let t = (y - lo.y) / (hi.y - lo.y);
                crossings.push(((k - first) as u32, lo.x + t * (hi.x - lo.x), dir));
            }
        }
    }
    crossings.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

    // Walk each row's crossings left to right, keeping the winding number; the
    // stretches where it says "inside" are where this row's dots go.
    let inside = |winding: i32| {
        if nonzero {
            winding != 0
        } else {
            winding % 2 != 0
        }
    };
    let mut at = 0;
    while at < crossings.len() {
        let row = crossings[at].0;
        let k = first + i64::from(row);
        let y = k as f32 * pitch;
        // Odd rows are shifted half a step.
        let shift = if k.rem_euclid(2) == 1 { pitch / 2.0 } else { 0.0 };

        let mut winding = 0;
        let mut start = 0.0_f32;
        while at < crossings.len() && crossings[at].0 == row {
            let (_, x, dir) = crossings[at];
            let was = inside(winding);
            winding += dir;
            let now = inside(winding);
            if !was && now {
                start = x;
            } else if was && !now {
                let from = start.max(clip.min.x);
                let to = x.min(clip.max.x);
                let mut j = ((from - shift) / pitch).ceil();
                loop {
                    let dot_x = j * pitch + shift;
                    if dot_x > to || out.len() >= MAX_DOTS {
                        break;
                    }
                    out.push(Pos2::new(dot_x, y));
                    j += 1.0;
                }
            }
            at += 1;
        }
        if out.len() >= MAX_DOTS {
            return;
        }
    }
}

/// The dots of a **stroke**: a dotted line along every centre line, as many rows
/// of it side by side as the line is wide, within `clip`.
///
/// `lines` are the flattened centre lines in screen pixels (a closed subpath
/// repeats its first point at the end) and `width` is how wide the stroke is
/// drawn on screen. The dots keep their spacing round corners and from one
/// segment to the next, so a bend does not bunch them.
pub fn stroke_dots(lines: &[Vec<Pos2>], width: f32, clip: Rect, pitch: f32, out: &mut Vec<Pos2>) {
    if !(pitch > 0.0) || !pitch.is_finite() {
        return;
    }
    let band = width.max(MIN_STROKE_BAND);
    let rows = ((band / pitch).ceil() as i32).max(1);
    // Anything further from the view than the band could reach is not needed.
    let reach = clip.expand(band);

    for line in lines {
        // Distance along the line to the next dot, carried between segments.
        let mut carry = 0.0_f32;
        for pair in line.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let along = b - a;
            let length = along.length();
            if !(length.is_finite() && length > 0.0 && a.x.is_finite() && a.y.is_finite()) {
                continue;
            }
            let dir = along / length;
            let normal = Vec2::new(-dir.y, dir.x);
            // The first dot at or after `from` on this segment's lattice.
            let first_at = |from: f32| carry + ((from - carry) / pitch).ceil().max(0.0) * pitch;

            if let Some((t0, t1)) = clipped_range(a, b, reach) {
                let mut d = first_at(t0 * length);
                let end = t1 * length;
                while d <= end && out.len() < MAX_DOTS {
                    let centre = a + dir * d;
                    for r in 0..rows {
                        let offset = (r as f32 - (rows - 1) as f32 / 2.0) * pitch;
                        let p = centre + normal * offset;
                        if clip.contains(p) {
                            out.push(p);
                        }
                    }
                    d += pitch;
                }
            }
            carry = first_at(length) - length;
            if out.len() >= MAX_DOTS {
                return;
            }
        }
    }
}

/// The part of the segment `a`→`b` inside `rect`, as a range of its parameter —
/// Liang–Barsky. `None` when none of it is.
fn clipped_range(a: Pos2, b: Pos2, rect: Rect) -> Option<(f32, f32)> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
    for (p, q) in [
        (-dx, a.x - rect.min.x),
        (dx, rect.max.x - a.x),
        (-dy, a.y - rect.min.y),
        (dy, rect.max.y - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// The hatch of a whole selection: the dots for every shape in `shapes`, laid
/// over where the stage puts them.
///
/// `to_screen` carries a point on the stage to the screen, and `zoom` is how many
/// screen pixels a stage unit is (a stroke is drawn that wide). Shapes wholly out
/// of `clip` cost one box test. The pitch is chosen once for the lot, from how
/// much of the view the selection covers — see [`pitch_for`] — so the total is
/// bounded however many shapes there are.
pub fn selection_dots(
    shapes: &[DrawnShape],
    zoom: f64,
    to_screen: &impl Fn(Point) -> Pos2,
    clip: Rect,
) -> Vec<Pos2> {
    // Each outline on screen, and the stroke's width there.
    struct OnScreen {
        fill: Option<(Vec<Vec<Pos2>>, bool)>,
        stroke: Option<(Vec<Vec<Pos2>>, f32)>,
    }
    let screen = |rings: &[Vec<Point>]| -> Vec<Vec<Pos2>> {
        rings
            .iter()
            .map(|ring| ring.iter().map(|p| to_screen(*p)).collect())
            .collect()
    };

    let mut visible = Vec::with_capacity(shapes.len());
    let mut area = 0.0_f32;
    for shape in shapes {
        let fill = shape.fill.as_ref().map(|(rings, nonzero)| (screen(rings), *nonzero));
        let stroke = shape
            .stroke
            .as_ref()
            .map(|(lines, width)| (screen(lines), (*width * zoom) as f32));

        let mut bounds = Rect::NOTHING;
        let reach = stroke.as_ref().map_or(0.0, |(_, w)| w.max(MIN_STROKE_BAND));
        for ring in fill
            .iter()
            .flat_map(|(r, _)| r.iter())
            .chain(stroke.iter().flat_map(|(l, _)| l.iter()))
        {
            for p in ring {
                if p.x.is_finite() && p.y.is_finite() {
                    bounds.extend_with(*p);
                }
            }
        }
        // A shape that is nowhere near the view (or has no finite point at all)
        // costs nothing more.
        if bounds.min.x > bounds.max.x || !bounds.expand(reach).intersects(clip) {
            continue;
        }
        let seen = bounds.intersect(clip);
        if fill.is_some() && seen.is_positive() {
            area += seen.width() * seen.height();
        }
        visible.push(OnScreen { fill, stroke });
    }

    let pitch = pitch_for(area);
    let mut dots = Vec::new();
    for shape in &visible {
        if let Some((rings, nonzero)) = &shape.fill {
            fill_dots(rings, *nonzero, clip, pitch, &mut dots);
        }
        if let Some((lines, width)) = &shape.stroke {
            stroke_dots(lines, *width, clip, pitch, &mut dots);
        }
        if dots.len() >= MAX_DOTS {
            break;
        }
    }
    dots
}

/// The dots as one mesh: a small square at each, in one colour.
///
/// One mesh however many dots, so the painter is handed a single shape and the
/// cost of a frame is the dots, not a shape apiece.
pub fn dots_mesh(dots: &[Pos2], color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.reserve_vertices(dots.len() * 4);
    mesh.reserve_triangles(dots.len() * 2);
    for dot in dots {
        mesh.add_colored_rect(Rect::from_center_size(*dot, Vec2::splat(DOT)), color);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big() -> Rect {
        Rect::from_min_max(Pos2::new(-1000.0, -1000.0), Pos2::new(1000.0, 1000.0))
    }

    /// A square ring from `(x0, y0)` to `(x1, y1)`; `reverse` winds it the other
    /// way, which is what a hole is under the non-zero rule.
    fn square(x0: f32, y0: f32, x1: f32, y1: f32, reverse: bool) -> Vec<Pos2> {
        let mut ring = vec![
            Pos2::new(x0, y0),
            Pos2::new(x1, y0),
            Pos2::new(x1, y1),
            Pos2::new(x0, y1),
        ];
        if reverse {
            ring.reverse();
        }
        ring
    }

    fn inside_square(p: &Pos2, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1
    }

    /// **A fill is dotted all over and nowhere else.** The count is the area over
    /// the square of the pitch, whatever the shape.
    #[test]
    fn a_fill_is_dotted_inside_itself_only() {
        let mut dots = Vec::new();
        fill_dots(
            &[square(10.0, 10.0, 110.0, 110.0, false)],
            true,
            big(),
            PITCH,
            &mut dots,
        );
        assert!(
            dots.iter().all(|p| inside_square(p, 10.0, 10.0, 110.0, 110.0)),
            "a dot fell outside the square"
        );
        // 100 x 100 at a pitch of 4 is 625 dots.
        assert!(
            (dots.len() as f32 - 625.0).abs() < 60.0,
            "expected about 625 dots, got {}",
            dots.len()
        );
        // Every other row is shifted half a step: it is a hatch, not a grid.
        // Rows are 4 apart, so y = 16 and y = 24 are alike and y = 20 is not.
        let first_x = |y: f32| -> f32 {
            dots.iter()
                .filter(|p| p.y == y)
                .map(|p| p.x)
                .fold(f32::INFINITY, f32::min)
        };
        assert!(
            (first_x(16.0) - first_x(24.0)).abs() < 1e-3,
            "rows two apart line up"
        );
        assert!(
            (first_x(16.0) - first_x(20.0)).abs() > 1.9,
            "neighbouring rows are offset: {} vs {}",
            first_x(16.0),
            first_x(20.0)
        );
    }

    /// **A ring's hole stays empty, and the fill rule says what a hole is.**
    #[test]
    fn a_hole_is_left_undotted() {
        let outer = square(0.0, 0.0, 120.0, 120.0, false);
        let hole = (40.0, 40.0, 80.0, 80.0);
        let in_hole = |p: &Pos2| inside_square(p, hole.0 + 0.1, hole.1 + 0.1, hole.2 - 0.1, hole.3 - 0.1);

        // Non-zero: wound against the outer ring, it is a hole.
        let mut dots = Vec::new();
        fill_dots(
            &[outer.clone(), square(hole.0, hole.1, hole.2, hole.3, true)],
            true,
            big(),
            PITCH,
            &mut dots,
        );
        assert!(!dots.is_empty() && !dots.iter().any(in_hole), "non-zero hole");

        // Non-zero, wound the same way: it is not a hole, and is dotted.
        let mut dots = Vec::new();
        fill_dots(
            &[outer.clone(), square(hole.0, hole.1, hole.2, hole.3, false)],
            true,
            big(),
            PITCH,
            &mut dots,
        );
        assert!(dots.iter().any(in_hole), "same winding is not a hole");

        // Even-odd: any ring inside another is, whichever way it is wound.
        let mut dots = Vec::new();
        fill_dots(
            &[outer, square(hole.0, hole.1, hole.2, hole.3, false)],
            false,
            big(),
            PITCH,
            &mut dots,
        );
        assert!(!dots.is_empty() && !dots.iter().any(in_hole), "even-odd hole");
    }

    /// A concave outline — an L — is not filled across the notch.
    #[test]
    fn a_concave_shape_is_not_filled_across_its_notch() {
        let l = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(40.0, 0.0),
            Pos2::new(40.0, 40.0),
            Pos2::new(100.0, 40.0),
            Pos2::new(100.0, 100.0),
            Pos2::new(0.0, 100.0),
        ];
        let mut dots = Vec::new();
        fill_dots(&[l], true, big(), PITCH, &mut dots);
        assert!(!dots.is_empty());
        assert!(
            !dots.iter().any(|p| p.x > 41.0 && p.y < 39.0),
            "a dot was laid in the notch"
        );
        assert!(dots.iter().any(|p| p.x > 60.0 && p.y > 60.0), "the foot is dotted");
    }

    /// Only the view is worked on: nothing outside the clip rectangle is made.
    #[test]
    fn the_hatch_stays_inside_the_view() {
        let clip = Rect::from_min_max(Pos2::new(20.0, 30.0), Pos2::new(60.0, 70.0));
        let mut dots = Vec::new();
        fill_dots(
            &[square(0.0, 0.0, 500.0, 500.0, false)],
            true,
            clip,
            PITCH,
            &mut dots,
        );
        assert!(!dots.is_empty());
        assert!(dots.iter().all(|p| clip.contains(*p)), "a dot fell outside the view");
        // And a shape wholly out of view makes none, however big.
        let mut none = Vec::new();
        fill_dots(
            &[square(-5000.0, -5000.0, -4000.0, -4000.0, false)],
            true,
            clip,
            PITCH,
            &mut none,
        );
        assert!(none.is_empty());
    }

    /// **A stroke is a dotted line along the line**, as wide as the line is.
    #[test]
    fn a_stroke_is_dotted_along_its_line() {
        let line = vec![vec![Pos2::new(10.0, 50.0), Pos2::new(110.0, 50.0)]];

        // A hairline: one row, on the line itself, a dot every pitch.
        let mut thin = Vec::new();
        stroke_dots(&line, 1.0, big(), PITCH, &mut thin);
        assert!(
            thin.iter().all(|p| (p.y - 50.0).abs() < 1e-3 && p.x >= 10.0 && p.x <= 110.0),
            "a hairline's dots lie on it"
        );
        assert!(
            (thin.len() as f32 - 26.0).abs() <= 2.0,
            "100 long at a pitch of 4 is about 26 dots, got {}",
            thin.len()
        );

        // A 12-wide stroke: three rows side by side, spanning its width.
        let mut wide = Vec::new();
        stroke_dots(&line, 12.0, big(), PITCH, &mut wide);
        let mut ys: Vec<f32> = wide.iter().map(|p| p.y).collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
        assert_eq!(ys.len(), 3, "three rows, got {ys:?}");
        assert!(ys.iter().all(|y| (y - 50.0).abs() <= 6.0));
    }

    /// The spacing carries round a corner: the dots do not restart, and bunch,
    /// at every vertex of a flattened curve.
    #[test]
    fn dots_keep_their_spacing_round_a_corner() {
        let bend = vec![vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(30.0, 0.0),
            Pos2::new(30.0, 30.0),
        ]];
        let mut dots = Vec::new();
        stroke_dots(&bend, 1.0, big(), PITCH, &mut dots);
        for pair in dots.windows(2) {
            let gap = (pair[1] - pair[0]).length();
            // Across the corner the straight distance is shorter than the path.
            assert!(gap > PITCH * 0.6, "dots bunched: {gap} apart");
            assert!(gap < PITCH * 1.2, "dots skipped: {gap} apart");
        }
        // The whole 60 long, a dot every 4.
        assert!((dots.len() as f32 - 16.0).abs() <= 2.0, "got {}", dots.len());
    }

    /// **A frame's hatch is bounded however much is selected.** A page of
    /// shapes widens the lattice rather than laying down a million dots.
    #[test]
    fn the_dot_budget_holds_however_large_the_selection() {
        let area = 4000.0 * 4000.0;
        let pitch = pitch_for(area);
        assert!(pitch > PITCH, "a huge selection thins the hatch: {pitch}");

        let mut dots = Vec::new();
        fill_dots(
            &[square(0.0, 0.0, 4000.0, 4000.0, false)],
            true,
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(4000.0, 4000.0)),
            pitch,
            &mut dots,
        );
        assert!(dots.len() <= MAX_DOTS, "{} dots", dots.len());
        // And not wastefully few: the thinned lattice still fills the budget.
        assert!(dots.len() > MAX_DOTS / 2, "{} dots", dots.len());

        // The hard stop holds even with a lattice too fine for the area.
        let mut capped = Vec::new();
        fill_dots(
            &[square(0.0, 0.0, 4000.0, 4000.0, false)],
            true,
            big(),
            PITCH,
            &mut capped,
        );
        assert_eq!(capped.len(), MAX_DOTS);

        // A small selection is not thinned.
        assert_eq!(pitch_for(200.0 * 200.0), PITCH);
    }

    /// Nothing non-finite or degenerate may panic or loop.
    #[test]
    fn degenerate_input_makes_no_dots_and_does_not_hang() {
        let mut dots = Vec::new();
        fill_dots(&[], true, big(), PITCH, &mut dots);
        fill_dots(&[vec![Pos2::new(1.0, 1.0)]], true, big(), PITCH, &mut dots);
        fill_dots(
            &[vec![
                Pos2::new(f32::NAN, 0.0),
                Pos2::new(10.0, f32::INFINITY),
                Pos2::new(0.0, 10.0),
            ]],
            true,
            big(),
            PITCH,
            &mut dots,
        );
        stroke_dots(&[vec![Pos2::new(1.0, 1.0)]], 2.0, big(), PITCH, &mut dots);
        stroke_dots(
            &[vec![Pos2::new(5.0, 5.0), Pos2::new(5.0, 5.0)]],
            2.0,
            big(),
            PITCH,
            &mut dots,
        );
        fill_dots(&[square(0.0, 0.0, 10.0, 10.0, false)], true, big(), 0.0, &mut dots);
        assert!(dots.is_empty());
    }

    #[test]
    fn the_dots_make_one_mesh() {
        let mesh = dots_mesh(&[Pos2::new(1.0, 1.0), Pos2::new(9.0, 9.0)], Color32::WHITE);
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.indices.len(), 12);
    }
}
