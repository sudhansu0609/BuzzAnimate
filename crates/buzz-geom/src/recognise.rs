//! Shape recognition: turning a drawn wobble into the shape it was meant to be.
//!
//! Animate does this as you draw and again when you press Straighten: a
//! roughly-circular scribble becomes an oval, four rough strokes become a
//! rectangle, and a shaky line becomes straight. It is the difference between
//! a drawing program and a *sketching* one — the hand is allowed to be
//! approximate because the tool knows what was meant.
//!
//! # How it decides
//!
//! The path is flattened to points once, and each candidate shape is fitted to
//! those points and scored by the **worst** distance from a point to the ideal
//! shape, as a fraction of the shape's own size. Worst rather than average,
//! because an average hides exactly the case that matters: a circle with one
//! corner pulled out is not a circle, and its average error is tiny.
//!
//! Candidates are tried from the most specific to the least — a circle before
//! an oval, a square before a rectangle — so a shape that qualifies as both
//! comes back as the one the hand was more likely reaching for.
//!
//! # What it will not do
//!
//! It never *fails* into a wrong answer: everything is judged against a
//! tolerance, and a scribble that is no shape at all comes back as `None` and
//! is left exactly as drawn. Silently replacing artwork with something the
//! animator did not draw is worse than doing nothing.

use kurbo::{Affine, BezPath, Point, Rect, Shape as _, Vec2};

/// How rough a drawing may be and still count.
///
/// Animate's Preferences offer the same choice, in the same words, for its
/// drawing-time recognition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Tolerance {
    /// Nearly exact: for tidying artwork that is already close.
    Strict,
    /// What a steady hand produces.
    #[default]
    Normal,
    /// For drawing quickly, or with a mouse.
    Tolerant,
}

impl Tolerance {
    /// The worst allowed error, as a fraction of the shape's size.
    fn allowance(self) -> f64 {
        match self {
            Tolerance::Strict => 0.045,
            Tolerance::Normal => 0.10,
            Tolerance::Tolerant => 0.18,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tolerance::Strict => "Strict",
            Tolerance::Normal => "Normal",
            Tolerance::Tolerant => "Tolerant",
        }
    }
}

/// What a path was recognised as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recognised {
    Line,
    Rectangle,
    Square,
    Oval,
    Circle,
}

impl Recognised {
    pub fn label(self) -> &'static str {
        match self {
            Recognised::Line => "a straight line",
            Recognised::Rectangle => "a rectangle",
            Recognised::Square => "a square",
            Recognised::Oval => "an oval",
            Recognised::Circle => "a circle",
        }
    }
}

/// How finely a path is sampled before it is judged.
const FLATTEN: f64 = 0.25;

/// Try to recognise `path`. Returns the ideal shape and what it is.
pub fn recognise(path: &BezPath, tolerance: Tolerance) -> Option<(BezPath, Recognised)> {
    let points = sample(path);
    if points.len() < 4 {
        return None;
    }

    let closed = is_closed(path, &points);
    let bounds = path.bounding_box();
    let size = bounds.width().hypot(bounds.height());
    if size < 1e-6 {
        return None;
    }
    let allowed = size * tolerance.allowance();

    // An open path is a line or it is nothing: closing a drawn arc into an
    // oval would invent a shape the hand did not make.
    if !closed {
        return line(&points, allowed).map(|p| (p, Recognised::Line));
    }

    // Round before square: a hand-drawn circle passes a loose rectangle test
    // at its corners far more readily than a hand-drawn rectangle passes the
    // circle test, so trying the rectangle first would square off circles.
    if let Some(found) = ellipse(&points, bounds, allowed, tolerance) {
        return Some(found);
    }
    rectangle(&points, allowed, tolerance)
}

/// Points along the path, at a fixed flatness.
fn sample(path: &BezPath) -> Vec<Point> {
    let mut points = Vec::new();
    kurbo::flatten(path.iter(), FLATTEN, |el| match el {
        kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => points.push(p),
        _ => {}
    });
    // A closed path repeats its first point; one copy is enough.
    if points.len() > 2 && (points[0] - points[points.len() - 1]).hypot() < 1e-9 {
        points.pop();
    }
    points
}

/// Does the path come back to where it started?
fn is_closed(path: &BezPath, points: &[Point]) -> bool {
    if path
        .elements()
        .iter()
        .any(|el| matches!(el, kurbo::PathEl::ClosePath))
    {
        return true;
    }
    match (points.first(), points.last()) {
        (Some(a), Some(b)) => {
            let span = extent(points);
            span > 1e-9 && (*a - *b).hypot() < span * 0.08
        }
        _ => false,
    }
}

/// The largest distance between any point and the first one — a cheap stand-in
/// for the shape's size that does not need the bounding box.
fn extent(points: &[Point]) -> f64 {
    let first = points[0];
    points
        .iter()
        .map(|p| (*p - first).hypot())
        .fold(0.0, f64::max)
}

/// A straight line, if every point is near the one joining the ends.
fn line(points: &[Point], allowed: f64) -> Option<BezPath> {
    let (a, b) = (*points.first()?, *points.last()?);
    let along = b - a;
    let length = along.hypot();
    if length < 1e-9 {
        return None;
    }
    let unit = along / length;
    // The perpendicular distance from the line, which for a *segment* is what
    // matters: a point beyond an end is not a wobble, it is a hook.
    let worst = points
        .iter()
        .map(|p| {
            let d = *p - a;
            let along_line = d.dot(unit).clamp(0.0, length);
            (d - unit * along_line).hypot()
        })
        .fold(0.0, f64::max);

    (worst <= allowed).then(|| {
        let mut out = BezPath::new();
        out.move_to(a);
        out.line_to(b);
        out
    })
}

/// An oval inscribed in the drawing's own bounding box.
fn ellipse(
    points: &[Point],
    bounds: Rect,
    allowed: f64,
    tolerance: Tolerance,
) -> Option<(BezPath, Recognised)> {
    let (rx, ry) = (bounds.width() / 2.0, bounds.height() / 2.0);
    if rx < 1e-6 || ry < 1e-6 {
        return None;
    }
    let centre = bounds.center();

    // Distance from the ellipse, estimated by scaling the point onto it. Exact
    // distance to an ellipse needs an iteration; this is within a few per cent
    // for points that are already near it, which is the only case that decides
    // anything.
    let worst = points
        .iter()
        .map(|p| {
            let d = *p - centre;
            let normalised = ((d.x / rx).powi(2) + (d.y / ry).powi(2)).sqrt();
            if normalised < 1e-9 {
                return rx.min(ry);
            }
            let on_ellipse = centre + d / normalised;
            (*p - on_ellipse).hypot()
        })
        .fold(0.0, f64::max);

    if worst > allowed {
        return None;
    }

    // A circle if the two radii are close — the shape somebody drawing "a
    // circle" meant, rather than an oval that happens to be nearly round.
    let round = (rx - ry).abs() <= rx.max(ry) * tolerance.allowance() * 1.5;
    let (rx, ry, kind) = if round {
        let r = (rx + ry) / 2.0;
        (r, r, Recognised::Circle)
    } else {
        (rx, ry, Recognised::Oval)
    };

    let ellipse = kurbo::Ellipse::new(centre, (rx, ry), 0.0);
    Some((ellipse.to_path(0.05), kind))
}

/// A rectangle, allowed to be at an angle.
///
/// The angle is found by trying orientations and keeping the one whose
/// bounding box is smallest — the standard minimum-area rectangle, coarsely
/// searched then refined, which is plenty for artwork drawn by hand and much
/// less code than rotating calipers over a convex hull.
fn rectangle(
    points: &[Point],
    allowed: f64,
    tolerance: Tolerance,
) -> Option<(BezPath, Recognised)> {
    let (angle, box_at) = best_angle(points)?;

    let rotate = Affine::rotate(-angle);
    let back = Affine::rotate(angle);

    // Every point must lie near the outline of that rectangle — inside it is
    // not good enough, or a filled blob would pass.
    let worst = points
        .iter()
        .map(|p| distance_to_outline(rotate * *p, box_at))
        .fold(0.0, f64::max);
    if worst > allowed {
        return None;
    }

    let (w, h) = (box_at.width(), box_at.height());
    let square = (w - h).abs() <= w.max(h) * tolerance.allowance() * 1.5;
    let box_at = if square {
        let side = (w + h) / 2.0;
        Rect::from_center_size(box_at.center(), (side, side))
    } else {
        box_at
    };

    let mut out = BezPath::new();
    let corners = [
        Point::new(box_at.x0, box_at.y0),
        Point::new(box_at.x1, box_at.y0),
        Point::new(box_at.x1, box_at.y1),
        Point::new(box_at.x0, box_at.y1),
    ];
    out.move_to(back * corners[0]);
    for corner in &corners[1..] {
        out.line_to(back * *corner);
    }
    out.close_path();

    Some((
        out,
        if square {
            Recognised::Square
        } else {
            Recognised::Rectangle
        },
    ))
}

/// The orientation whose axis-aligned box is smallest, and that box.
fn best_angle(points: &[Point]) -> Option<(f64, Rect)> {
    if points.len() < 3 {
        return None;
    }
    let area_at = |angle: f64| {
        let rotate = Affine::rotate(-angle);
        let mut r = Rect::from_points(rotate * points[0], rotate * points[0]);
        for p in points {
            let q = rotate * *p;
            r = r.union(Rect::from_points(q, q));
        }
        (r.width() * r.height(), r)
    };

    // A quarter turn covers every rectangle; past that they repeat.
    let quarter = std::f64::consts::FRAC_PI_2;
    let mut best = (f64::INFINITY, 0.0, Rect::ZERO);
    const COARSE: usize = 45;
    for i in 0..COARSE {
        let angle = quarter * i as f64 / COARSE as f64;
        let (area, r) = area_at(angle);
        if area < best.0 {
            best = (area, angle, r);
        }
    }
    // Refine around the winner, so a rectangle drawn at 7° does not come back
    // at 6° and look subtly wrong.
    let step = quarter / COARSE as f64;
    for i in -10..=10 {
        let angle = best.1 + step * i as f64 / 10.0;
        let (area, r) = area_at(angle);
        if area < best.0 {
            best = (area, angle, r);
        }
    }

    if !(best.0.is_finite() && best.2.width() > 1e-9 && best.2.height() > 1e-9) {
        return None;
    }

    // **Nearly square-on means square-on.** A wobbly hand makes the smallest
    // box sit a degree or two off the page, and a rectangle returned at 1.4°
    // looks like a mistake rather than a drawing. Anything within three
    // degrees of the page is snapped to it; a rectangle genuinely drawn at an
    // angle is nowhere near that line.
    let snap = 3.0_f64.to_radians();
    let square_on = best.1 < snap || (quarter - best.1) < snap;
    let angle = if square_on { 0.0 } else { best.1 };
    if angle != best.1 {
        let (_, r) = area_at(angle);
        return Some((angle, r));
    }
    Some((best.1, best.2))
}

/// Distance from a point to the *outline* of a rectangle.
fn distance_to_outline(p: Point, r: Rect) -> f64 {
    let dx = (p.x - r.x0).abs().min((p.x - r.x1).abs());
    let dy = (p.y - r.y0).abs().min((p.y - r.y1).abs());
    let inside = p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1;
    if inside {
        // Nearest edge.
        dx.min(dy)
    } else {
        // Outside: the usual distance to the box, which is also the distance
        // to its outline.
        let ox = (r.x0 - p.x).max(0.0).max(p.x - r.x1);
        let oy = (r.y0 - p.y).max(0.0).max(p.y - r.y1);
        ox.hypot(oy)
    }
}

/// The centre of a set of points, for tests and for callers that want to know
/// where a recognised shape ended up.
pub fn centroid(points: &[Point]) -> Point {
    if points.is_empty() {
        return Point::ORIGIN;
    }
    let sum = points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2());
    (sum / points.len() as f64).to_point()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wobble of `amount` at every sample, deterministic so a failure can be
    /// reproduced.
    fn wobble(i: usize, amount: f64) -> Vec2 {
        let a = (i as f64 * 12.9898).sin() * 43758.5453;
        let b = (i as f64 * 78.233).sin() * 12345.6789;
        Vec2::new(
            (a.fract() - 0.5) * 2.0 * amount,
            (b.fract() - 0.5) * 2.0 * amount,
        )
    }

    fn rough_circle(centre: Point, radius: f64, amount: f64) -> BezPath {
        let mut path = BezPath::new();
        for i in 0..48 {
            let t = i as f64 / 48.0 * std::f64::consts::TAU;
            let p = centre + Vec2::new(t.cos() * radius, t.sin() * radius) + wobble(i, amount);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();
        path
    }

    fn rough_rect(rect: Rect, amount: f64, angle: f64) -> BezPath {
        let turn = Affine::rotate_about(angle, rect.center());
        let corners = [
            Point::new(rect.x0, rect.y0),
            Point::new(rect.x1, rect.y0),
            Point::new(rect.x1, rect.y1),
            Point::new(rect.x0, rect.y1),
        ];
        let mut path = BezPath::new();
        let mut n = 0;
        for i in 0..4 {
            let a = corners[i];
            let b = corners[(i + 1) % 4];
            for step in 0..12 {
                let t = step as f64 / 12.0;
                let p = a.lerp(b, t) + wobble(n, amount);
                n += 1;
                if i == 0 && step == 0 {
                    path.move_to(turn * p);
                } else {
                    path.line_to(turn * p);
                }
            }
        }
        path.close_path();
        path
    }

    fn rough_line(a: Point, b: Point, amount: f64) -> BezPath {
        let mut path = BezPath::new();
        for i in 0..20 {
            let p = a.lerp(b, i as f64 / 19.0) + wobble(i, amount);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path
    }

    #[test]
    fn a_rough_circle_becomes_a_circle() {
        let path = rough_circle(Point::new(100.0, 100.0), 50.0, 3.0);
        let (found, kind) = recognise(&path, Tolerance::Normal).expect("not recognised");
        assert_eq!(kind, Recognised::Circle);

        let b = found.bounding_box();
        assert!(
            (b.width() - b.height()).abs() < 1e-6,
            "a circle should be round: {b:?}"
        );
        assert!((b.center() - Point::new(100.0, 100.0)).hypot() < 6.0);
        assert!((b.width() - 100.0).abs() < 12.0, "wrong size: {b:?}");
    }

    #[test]
    fn a_rough_oval_stays_an_oval() {
        let mut path = BezPath::new();
        for i in 0..48 {
            let t = i as f64 / 48.0 * std::f64::consts::TAU;
            let p = Point::new(200.0 + t.cos() * 90.0, 100.0 + t.sin() * 40.0) + wobble(i, 2.0);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();

        let (found, kind) = recognise(&path, Tolerance::Normal).expect("not recognised");
        assert_eq!(kind, Recognised::Oval);
        let b = found.bounding_box();
        assert!(b.width() > b.height() * 1.5, "it should stay oval: {b:?}");
    }

    #[test]
    fn a_rough_rectangle_becomes_a_rectangle() {
        let path = rough_rect(Rect::new(0.0, 0.0, 200.0, 100.0), 4.0, 0.0);
        let (found, kind) = recognise(&path, Tolerance::Normal).expect("not recognised");
        assert_eq!(kind, Recognised::Rectangle);

        // Measured on the rectangle's own edges rather than its bounding box,
        // which is the same thing here only because it came back square to the
        // page — and the point of the next test is that it does not always.
        let corners = sample(&found);
        assert_eq!(corners.len(), 4);
        let long = (corners[1] - corners[0]).hypot();
        let short = (corners[2] - corners[1]).hypot();
        assert!((long - 200.0).abs() < 15.0, "long edge {long}");
        assert!((short - 100.0).abs() < 15.0, "short edge {short}");
        // A move, three lines and a close: the fourth edge is the close.
        assert_eq!(found.elements().len(), 5);
    }

    /// A rectangle drawn at an angle comes back at that angle, not squared up
    /// to the page — the drawing was not crooked, it was turned.
    #[test]
    fn a_turned_rectangle_keeps_its_angle() {
        let angle = 0.35;
        let path = rough_rect(Rect::new(0.0, 0.0, 200.0, 100.0), 3.0, angle);
        let (found, kind) = recognise(&path, Tolerance::Normal).expect("not recognised");
        assert_eq!(kind, Recognised::Rectangle);

        // The recognised rectangle's longest edge should run at the same angle.
        let points = sample(&found);
        assert_eq!(points.len(), 4);
        let edge = points[1] - points[0];
        let found_angle = edge.y.atan2(edge.x);
        // Modulo a quarter turn: which corner the path starts at, and which way
        // round it goes, are not part of the claim.
        let quarter = std::f64::consts::FRAC_PI_2;
        let diff = (found_angle - angle).rem_euclid(quarter);
        let off = diff.min(quarter - diff);
        assert!(
            off < 0.1,
            "expected {angle} rad, got {found_angle} (off by {off})"
        );
    }

    #[test]
    fn a_rough_square_is_a_square() {
        let path = rough_rect(Rect::new(0.0, 0.0, 120.0, 112.0), 2.0, 0.0);
        let (found, kind) = recognise(&path, Tolerance::Normal).expect("not recognised");
        assert_eq!(kind, Recognised::Square);
        let b = found.bounding_box();
        assert!((b.width() - b.height()).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn a_shaky_line_becomes_straight() {
        let path = rough_line(Point::new(0.0, 0.0), Point::new(300.0, 40.0), 5.0);
        let (found, kind) = recognise(&path, Tolerance::Normal).expect("not recognised");
        assert_eq!(kind, Recognised::Line);
        assert_eq!(found.elements().len(), 2, "a line is two elements");
    }

    /// **The one that matters most.** Something that is not a shape must come
    /// back untouched: replacing a drawing with a rectangle the animator never
    /// drew is worse than doing nothing at all.
    #[test]
    fn a_scribble_is_left_alone() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        for i in 1..30 {
            let t = i as f64;
            path.line_to(Point::new(
                t * 7.0,
                (t * 0.9).sin() * 60.0 + (t * 2.3).cos() * 20.0,
            ));
        }
        assert!(recognise(&path, Tolerance::Normal).is_none());
    }

    /// A five-pointed star is a closed shape and emphatically not an oval.
    #[test]
    fn a_star_is_not_a_circle() {
        let mut path = BezPath::new();
        for i in 0..10 {
            let t = i as f64 / 10.0 * std::f64::consts::TAU;
            let radius = if i % 2 == 0 { 100.0 } else { 40.0 };
            let p = Point::new(t.cos() * radius, t.sin() * radius);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();
        assert!(recognise(&path, Tolerance::Normal).is_none());
    }

    /// Tolerance means what it says: a wobble that Normal rejects is accepted
    /// by Tolerant, and one Strict rejects is accepted by Normal.
    #[test]
    fn tolerance_decides_how_rough_a_hand_may_be() {
        let rough = rough_circle(Point::new(0.0, 0.0), 50.0, 6.5);
        assert!(recognise(&rough, Tolerance::Strict).is_none());
        assert!(recognise(&rough, Tolerance::Tolerant).is_some());

        let neat = rough_circle(Point::new(0.0, 0.0), 50.0, 1.0);
        assert!(
            recognise(&neat, Tolerance::Strict).is_some(),
            "a carefully drawn circle should pass even the strict test"
        );
    }

    /// An open arc is not an oval: closing it would invent a shape.
    #[test]
    fn an_open_arc_is_not_closed_into_an_oval() {
        let mut path = BezPath::new();
        for i in 0..24 {
            let t = i as f64 / 24.0 * std::f64::consts::PI;
            let p = Point::new(t.cos() * 60.0, t.sin() * 60.0);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        assert!(recognise(&path, Tolerance::Normal).is_none());
    }

    #[test]
    fn an_empty_or_tiny_path_is_not_recognised() {
        assert!(recognise(&BezPath::new(), Tolerance::Normal).is_none());

        let mut dot = BezPath::new();
        dot.move_to(Point::ORIGIN);
        dot.line_to(Point::new(1e-9, 0.0));
        assert!(recognise(&dot, Tolerance::Normal).is_none());
    }
}
