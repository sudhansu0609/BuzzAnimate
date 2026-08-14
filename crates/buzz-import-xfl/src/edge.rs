//! Animate's compact **Edge** path format.
//!
//! This is the crux of importing real `.fla` artwork: every shape Animate
//! saves is a string in this format, and if it is parsed wrongly the drawing
//! comes in mangled or not at all.
//!
//! # The format
//!
//! An edge string is a sequence of commands, each a punctuation mark followed
//! by coordinate pairs:
//!
//! | Command | Meaning |
//! |---|---|
//! | `!x y` | move to |
//! | `\|x y` | line to |
//! | `/x y` | line to (a "hidden" edge, drawn identically) |
//! | `[cx cy x y` | quadratic curve, control point then anchor |
//! | `]cx cy x y` | quadratic curve (hidden variant) |
//! | `Sn` | selection state; carries no geometry |
//!
//! # Coordinates
//!
//! Two representations appear, and both must be handled:
//!
//! * **Decimal twips.** `1234` means 1234/20 = 61.7 pixels. Animate inherited
//!   twips from Flash, which is also why its zoom stops at 2000%.
//! * **Signed hex fixed point.** `#3C2.8` is hex `3C2` twips plus hex `8`/256
//!   of a twip. The fractional part is optional.
//!
//! Mixing the two inside one string is legal, and real files do it.

use buzz_geom::{BezPath, Point};

/// Twips per pixel. Flash's unit, inherited by Animate.
const TWIPS_PER_PIXEL: f64 = 20.0;

/// Why an edge string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdgeError {
    #[error("unexpected character {found:?} at byte {at}")]
    Unexpected { found: char, at: usize },
    #[error("expected a number at byte {at}")]
    ExpectedNumber { at: usize },
    #[error("edge data ended in the middle of a command")]
    UnexpectedEnd,
}

/// Parse an Animate edge string into a path.
///
/// Coordinates come out in **pixels**, converted from twips.
pub fn parse_edges(edges: &str) -> Result<BezPath, EdgeError> {
    let mut parser = Parser {
        bytes: edges.as_bytes(),
        at: 0,
    };
    let mut path = BezPath::new();
    let mut started = false;
    // Animate emits a `moveTo` per contour; without tracking the current point
    // a curve command at the start of a string would have no origin.
    let mut cursor = Point::ORIGIN;

    loop {
        parser.skip_space();
        let Some(command) = parser.peek() else { break };

        match command {
            b'!' => {
                parser.at += 1;
                let p = parser.point()?;
                path.move_to(p);
                cursor = p;
                started = true;
            }
            b'|' | b'/' => {
                parser.at += 1;
                let p = parser.point()?;
                if !started {
                    path.move_to(cursor);
                    started = true;
                }
                path.line_to(p);
                cursor = p;
            }
            b'[' | b']' => {
                parser.at += 1;
                let control = parser.point()?;
                let anchor = parser.point()?;
                if !started {
                    path.move_to(cursor);
                    started = true;
                }
                path.quad_to(control, anchor);
                cursor = anchor;
            }
            b'S' => {
                // Selection state: `S` followed by digits. No geometry.
                parser.at += 1;
                parser.skip_digits();
            }
            other => {
                return Err(EdgeError::Unexpected {
                    found: other as char,
                    at: parser.at,
                });
            }
        }
    }

    Ok(path)
}

/// Parse edge data, closing each contour.
///
/// Animate stores a filled shape's outline without an explicit close, relying
/// on the fill rule. Closing explicitly makes the path behave the same way in
/// a renderer that does not assume it.
pub fn parse_edges_closed(edges: &str) -> Result<BezPath, EdgeError> {
    let path = parse_edges(edges)?;
    if path.elements().is_empty() {
        return Ok(path);
    }

    let mut out = BezPath::new();
    let mut has_content = false;
    for element in path.elements() {
        if matches!(element, kurbo::PathEl::MoveTo(_)) && has_content {
            out.close_path();
        }
        out.push(*element);
        has_content = true;
    }
    if has_content {
        out.close_path();
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Shape assembly
//
// **A `DOMShape` is not a set of closed outlines.** It is a soup of boundary
// pieces, and each piece says which fill lies on its *left* (`fillStyle1`) and
// which on its *right* (`fillStyle0`). Animate writes them in whatever order
// the drawing was made in, split across several `<Edge>` elements, and a piece
// is usually two points long — a real bush arrives as several hundred of them.
//
// Treating one `<Edge>` as one filled outline, which is the obvious reading,
// produces exactly what it sounds like: hundreds of two-point slivers instead
// of a bush. The pieces have to be *reassembled* — for each fill, take every
// piece with that fill on its left, plus every piece with it on the right
// turned round, and chain them end to start into closed loops. That is what
// this section does, and it is the difference between an Animate document
// importing and an Animate document arriving as streaks.
// ---------------------------------------------------------------------------

/// One step along a boundary piece.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Line(Point),
    /// Control point, then anchor.
    Quad(Point, Point),
}

impl Step {
    fn anchor(self) -> Point {
        match self {
            Self::Line(p) | Self::Quad(_, p) => p,
        }
    }
}

/// A boundary piece: where it starts, and the steps that follow it.
///
/// One `!` command and everything up to the next one.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: Point,
    pub steps: Vec<Step>,
}

impl Segment {
    pub fn end(&self) -> Point {
        self.steps.last().map(|s| s.anchor()).unwrap_or(self.start)
    }

    /// The same piece walked the other way.
    ///
    /// Needed because a fill on a piece's *right* becomes a fill on its left
    /// once the piece is reversed, and a loop can only be chained out of
    /// pieces that all run the same way round.
    pub fn reversed(&self) -> Self {
        let mut steps = Vec::with_capacity(self.steps.len());
        let mut anchor = self.end();
        for (index, step) in self.steps.iter().enumerate().rev() {
            let previous = if index == 0 {
                self.start
            } else {
                self.steps[index - 1].anchor()
            };
            steps.push(match step {
                // A quadratic reversed keeps its control point and swaps ends.
                Step::Quad(control, _) => Step::Quad(*control, previous),
                Step::Line(_) => Step::Line(previous),
            });
            anchor = previous;
        }
        let _ = anchor;
        Self {
            start: self.end(),
            steps,
        }
    }

    fn append(&self, path: &mut BezPath) {
        for step in &self.steps {
            match *step {
                Step::Line(p) => path.line_to(p),
                Step::Quad(c, p) => path.quad_to(c, p),
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Which way the piece sets off from its start.
    fn out_tangent(&self) -> buzz_geom::Vec2 {
        let Some(step) = self.steps.first() else {
            return buzz_geom::Vec2::ZERO;
        };
        let target = match *step {
            Step::Line(p) => p,
            // The control point leads, unless it sits exactly on the start.
            Step::Quad(c, p) => {
                if (c - self.start).hypot() > 1e-9 {
                    c
                } else {
                    p
                }
            }
        };
        target - self.start
    }

    /// Which way it is travelling as it arrives at its end.
    fn in_tangent(&self) -> buzz_geom::Vec2 {
        let Some(step) = self.steps.last() else {
            return buzz_geom::Vec2::ZERO;
        };
        let previous = if self.steps.len() >= 2 {
            self.steps[self.steps.len() - 2].anchor()
        } else {
            self.start
        };
        match *step {
            Step::Line(p) => p - previous,
            Step::Quad(c, p) => {
                if (p - c).hypot() > 1e-9 {
                    p - c
                } else {
                    p - previous
                }
            }
        }
    }
}

/// How far anticlockwise `to` lies from `from`, in `0..2π`.
///
/// Used to pick which boundary to follow where several meet, so a loop turns
/// the corner rather than setting off along whichever piece happened to be
/// stored first.
fn turn(from: buzz_geom::Vec2, to: buzz_geom::Vec2) -> f64 {
    let angle = to.y.atan2(to.x) - from.y.atan2(from.x);
    let full = std::f64::consts::TAU;
    ((angle % full) + full) % full
}

/// Points are matched by their exact stored value.
///
/// Coordinates arrive as twips, or as hex fixed point in 256ths of a twip, so
/// this key is lossless for both — and exactness matters: chaining by
/// approximate position would join pieces that only nearly touch, and the loop
/// would wander off across the drawing.
type Key = (i64, i64);

fn key(p: Point) -> Key {
    const UNITS: f64 = TWIPS_PER_PIXEL * 256.0;
    ((p.x * UNITS).round() as i64, (p.y * UNITS).round() as i64)
}

/// One `<Edge>` element: its styles, and the pieces it carries.
#[derive(Debug, Clone)]
pub struct EdgeRecord {
    /// `fillStyle1` — the fill on the left of the piece as it is written.
    pub fill_left: Option<u32>,
    /// `fillStyle0` — the fill on its right.
    pub fill_right: Option<u32>,
    pub stroke: Option<u32>,
    pub segments: Vec<Segment>,
}

/// Split an edge string into its boundary pieces.
pub fn parse_segments(edges: &str) -> Result<Vec<Segment>, EdgeError> {
    let mut parser = Parser {
        bytes: edges.as_bytes(),
        at: 0,
    };
    let mut out: Vec<Segment> = Vec::new();
    let mut current: Option<Segment> = None;
    // A curve command before any `!` continues from the origin, as in
    // `parse_edges`.
    let mut cursor = Point::ORIGIN;

    loop {
        parser.skip_space();
        let Some(command) = parser.peek() else { break };

        match command {
            b'!' => {
                parser.at += 1;
                let p = parser.point()?;
                if let Some(segment) = current.take()
                    && !segment.is_empty()
                {
                    out.push(segment);
                }
                cursor = p;
                current = Some(Segment {
                    start: p,
                    steps: Vec::new(),
                });
            }
            b'|' | b'/' => {
                parser.at += 1;
                let p = parser.point()?;
                current
                    .get_or_insert_with(|| Segment {
                        start: cursor,
                        steps: Vec::new(),
                    })
                    .steps
                    .push(Step::Line(p));
                cursor = p;
            }
            b'[' | b']' => {
                parser.at += 1;
                let control = parser.point()?;
                let anchor = parser.point()?;
                current
                    .get_or_insert_with(|| Segment {
                        start: cursor,
                        steps: Vec::new(),
                    })
                    .steps
                    .push(Step::Quad(control, anchor));
                cursor = anchor;
            }
            b'S' => {
                parser.at += 1;
                parser.skip_digits();
            }
            other => {
                return Err(EdgeError::Unexpected {
                    found: other as char,
                    at: parser.at,
                });
            }
        }
    }

    if let Some(segment) = current
        && !segment.is_empty()
    {
        out.push(segment);
    }
    Ok(out)
}

/// Every fill in the shape, as a closed path, in ascending style order.
///
/// Ascending because Animate draws its fills in style order and overlapping
/// fills must land in the same order they did there.
pub fn assemble_fills(records: &[EdgeRecord]) -> Vec<(u32, BezPath)> {
    assemble_fills_counted(records).0
}

/// The same, and how many loops had to be closed across a gap.
///
/// A fill's boundary in a well-formed file closes on itself; one that does not
/// is either damaged or beyond this reader, and closing it draws a straight
/// line across the artwork. The count is how that gets noticed rather than
/// shipped.
pub fn assemble_fills_counted(records: &[EdgeRecord]) -> (Vec<(u32, BezPath)>, usize) {
    let mut by_fill: std::collections::BTreeMap<u32, Vec<Segment>> =
        std::collections::BTreeMap::new();

    for record in records {
        for segment in &record.segments {
            if let Some(fill) = record.fill_left.filter(|f| *f != 0) {
                by_fill.entry(fill).or_default().push(segment.clone());
            }
            if let Some(fill) = record.fill_right.filter(|f| *f != 0) {
                // On the right of this piece is on the left of its reverse.
                by_fill.entry(fill).or_default().push(segment.reversed());
            }
        }
    }

    let mut gaps = 0;
    let paths = by_fill
        .into_iter()
        .filter_map(|(fill, segments)| {
            let (path, open) = chain_counted(segments, true);
            gaps += open;
            (!path.elements().is_empty()).then_some((fill, path))
        })
        .collect();
    (paths, gaps)
}

/// Every stroke in the shape, as an open path, in ascending style order.
pub fn assemble_strokes(records: &[EdgeRecord]) -> Vec<(u32, BezPath)> {
    let mut by_stroke: std::collections::BTreeMap<u32, Vec<Segment>> =
        std::collections::BTreeMap::new();

    for record in records {
        let Some(stroke) = record.stroke.filter(|s| *s != 0) else {
            continue;
        };
        for segment in &record.segments {
            by_stroke.entry(stroke).or_default().push(segment.clone());
        }
    }

    by_stroke
        .into_iter()
        .filter_map(|(stroke, segments)| {
            let path = chain(segments, false);
            (!path.elements().is_empty()).then_some((stroke, path))
        })
        .collect()
}

/// Chain pieces end-to-start into as few subpaths as possible.
///
/// Greedy, and deliberately so: at a point where three boundaries meet — which
/// is every place two filled shapes touch — any of them continues the loop,
/// and picking one and going on is both what Animate's own renderer does and
/// the only way to stay linear on a drawing with a hundred thousand pieces.
/// A piece that leads nowhere ends its subpath, which is closed if it is a
/// fill and left open if it is a stroke.
fn chain(segments: Vec<Segment>, close: bool) -> BezPath {
    chain_counted(segments, close).0
}

/// The same, and how many subpaths ended somewhere other than where they
/// started.
fn chain_counted(segments: Vec<Segment>, close: bool) -> (BezPath, usize) {
    use std::collections::HashMap;

    let mut open_loops = 0;
    let mut from: HashMap<Key, Vec<usize>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        from.entry(key(segment.start)).or_default().push(index);
    }

    let mut used = vec![false; segments.len()];
    let mut path = BezPath::new();

    for start_index in 0..segments.len() {
        if used[start_index] {
            continue;
        }
        used[start_index] = true;

        let first = &segments[start_index];
        let opening = key(first.start);
        path.move_to(first.start);
        first.append(&mut path);

        let mut at = key(first.end());
        let mut arriving = first.in_tangent();
        while at != opening {
            // **Which boundary to follow where several meet.** Blades of grass
            // drawn from a common root, two shapes sharing an edge, a leaf on
            // a stem: at that point three or four pieces start, and taking
            // whichever was stored first walks the loop off into the
            // neighbouring shape and back, which draws as a long thin spike
            // across the artwork.
            //
            // The piece to take is the one that turns furthest *back* towards
            // where the loop came from — the tightest turn that keeps the fill
            // on the same side all the way round. That is the standard rule
            // for tracing a face of a planar subdivision, and it is what makes
            // a hundred grass blades come out as a hundred grass blades.
            let candidates = from.get(&at);
            let next = candidates.and_then(|list| {
                let unused = list.iter().copied().filter(|i| !used[*i]);
                if list.len() <= 1 {
                    return unused.clone().next();
                }
                let back = -arriving;
                unused.min_by(|a, b| {
                    let ta = turn(back, segments[*a].out_tangent());
                    let tb = turn(back, segments[*b].out_tangent());
                    ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
                })
            });
            let Some(next) = next else {
                break;
            };
            used[next] = true;
            segments[next].append(&mut path);
            arriving = segments[next].in_tangent();
            at = key(segments[next].end());
        }

        if at != opening {
            open_loops += 1;
        }
        if close {
            path.close_path();
        }
    }

    (path, open_loops)
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    fn skip_digits(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.at += 1;
        }
    }

    fn point(&mut self) -> Result<Point, EdgeError> {
        let x = self.number()?;
        let y = self.number()?;
        Ok(Point::new(x, y))
    }

    /// One coordinate, in either representation, converted to pixels.
    fn number(&mut self) -> Result<f64, EdgeError> {
        self.skip_space();
        let start = self.at;

        let negative = match self.peek() {
            Some(b'-') => {
                self.at += 1;
                true
            }
            Some(b'+') => {
                self.at += 1;
                false
            }
            _ => false,
        };

        let twips = if self.peek() == Some(b'#') {
            self.at += 1;
            self.hex_fixed_point(start)?
        } else {
            self.decimal(start)?
        };

        if self.at == start {
            return Err(EdgeError::ExpectedNumber { at: start });
        }
        let signed = if negative { -twips } else { twips };
        Ok(signed / TWIPS_PER_PIXEL)
    }

    /// `#RRRR.FF` — hex twips, signed, optionally with a hex fraction in
    /// 256ths.
    fn hex_fixed_point(&mut self, start: usize) -> Result<f64, EdgeError> {
        let integer_start = self.at;
        while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
            self.at += 1;
        }
        if self.at == integer_start {
            return Err(EdgeError::ExpectedNumber { at: start });
        }
        let digits = std::str::from_utf8(&self.bytes[integer_start..self.at]).unwrap_or("0");
        let raw =
            i64::from_str_radix(digits, 16).map_err(|_| EdgeError::ExpectedNumber { at: start })?;

        // **Hex coordinates are two's complement, and Animate writes the sign
        // by using the full width.** `#FFFFFA.21` is not eight hundred
        // thousand pixels away; it is minus six twips. Read as unsigned, one
        // such point stretched a character's shin to four hundred thousand
        // units wide and its leg across the stage — which is exactly what a
        // real film looked like.
        //
        // Short forms stay positive: `#82` is 130 twips, not -126. Animate
        // only writes the leading `F`s when it means a negative number, so the
        // width *is* the sign, and six digits is where it starts.
        let bits = digits.len() * 4;
        let integer = if digits.len() >= 6 && bits <= 63 {
            let span = 1i64 << bits;
            if raw >= span / 2 { raw - span } else { raw }
        } else {
            raw
        };

        let mut fraction = 0.0;
        if self.peek() == Some(b'.') {
            self.at += 1;
            let fraction_start = self.at;
            while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                self.at += 1;
            }
            if self.at > fraction_start {
                let digits =
                    std::str::from_utf8(&self.bytes[fraction_start..self.at]).unwrap_or("0");
                let value = i64::from_str_radix(digits, 16).unwrap_or(0);
                // Each hex digit is a further 1/16th.
                let scale = 16f64.powi(digits.len() as i32);
                fraction = value as f64 / scale;
            }
        }

        Ok(integer as f64 + fraction)
    }

    /// Plain decimal twips, possibly with a decimal fraction.
    fn decimal(&mut self, start: usize) -> Result<f64, EdgeError> {
        let begin = self.at;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.at += 1;
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if self.at == begin {
            return Err(EdgeError::ExpectedNumber { at: start });
        }
        std::str::from_utf8(&self.bytes[begin..self.at])
            .unwrap_or("0")
            .parse::<f64>()
            .map_err(|_| EdgeError::ExpectedNumber { at: start })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{PathEl, Shape};

    fn points(path: &BezPath) -> Vec<Point> {
        path.elements()
            .iter()
            .filter_map(|e| match e {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => Some(*p),
                PathEl::ClosePath => None,
            })
            .collect()
    }

    #[test]
    fn a_simple_triangle_parses() {
        // 0,0 -> 100,0 -> 50,80, in twips (x20).
        let path = parse_edges("!0 0|2000 0|1000 1600|0 0").unwrap();
        let p = points(&path);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], Point::new(0.0, 0.0));
        assert_eq!(p[1], Point::new(100.0, 0.0));
        assert_eq!(p[2], Point::new(50.0, 80.0));
    }

    /// Twips are the whole reason coordinates look like large integers.
    #[test]
    fn coordinates_convert_from_twips_to_pixels() {
        let path = parse_edges("!20 40").unwrap();
        assert_eq!(points(&path)[0], Point::new(1.0, 2.0));
    }

    /// **A hex coordinate is signed, and the width carries the sign.**
    ///
    /// `#FFFFFA.21` is minus six twips, not eight hundred thousand pixels.
    /// Read as unsigned, one point like this stretched a character's shin to
    /// four hundred thousand units and threw a spike across every frame it
    /// appeared in — the single most damaging thing in a real film's import.
    #[test]
    fn a_wide_hex_coordinate_is_negative() {
        let path = parse_edges("!0 0|#FFFFFA.21 #5C9.4B").unwrap();
        let p = points(&path)[1];
        // -6 twips plus 0x21/256 of a twip, in pixels.
        assert!((p.x - (-5.871_093_75 / 20.0)).abs() < 1e-9, "x was {}", p.x);
        assert!((p.y - (1481.293 / 20.0)).abs() < 0.01, "y was {}", p.y);
    }

    /// And a short one is not: `#82` is 130 twips, not minus 126. Animate
    /// writes the leading `F`s only when it means a negative number.
    #[test]
    fn a_short_hex_coordinate_stays_positive() {
        let path = parse_edges("!#82.5D #3AF.E1").unwrap();
        let p = points(&path)[0];
        assert!(p.x > 6.0 && p.x < 6.6, "x was {}", p.x);
        assert!(p.y > 47.0 && p.y < 47.3, "y was {}", p.y);
    }

    #[test]
    fn negative_coordinates_work() {
        let path = parse_edges("!-2000 -1000|0 0").unwrap();
        let p = points(&path);
        assert_eq!(p[0], Point::new(-100.0, -50.0));
        assert_eq!(p[1], Point::new(0.0, 0.0));
    }

    #[test]
    fn quadratic_curves_keep_their_control_point() {
        let path = parse_edges("!0 0[1000 2000 2000 0").unwrap();
        match path.elements()[1] {
            PathEl::QuadTo(control, anchor) => {
                assert_eq!(control, Point::new(50.0, 100.0));
                assert_eq!(anchor, Point::new(100.0, 0.0));
            }
            other => panic!("expected a quad, got {other:?}"),
        }
    }

    #[test]
    fn the_hidden_edge_variants_behave_the_same() {
        let visible = parse_edges("!0 0|2000 0").unwrap();
        let hidden = parse_edges("!0 0/2000 0").unwrap();
        assert_eq!(points(&visible), points(&hidden));

        let a = parse_edges("!0 0[1000 500 2000 0").unwrap();
        let b = parse_edges("!0 0]1000 500 2000 0").unwrap();
        assert_eq!(points(&a), points(&b));
    }

    /// Hex fixed point is common in real files and easy to get wrong.
    #[test]
    fn hex_fixed_point_coordinates_parse() {
        // #3E8 = 1000 twips = 50 px.
        let path = parse_edges("!#3E8 #1F4").unwrap();
        assert_eq!(points(&path)[0], Point::new(50.0, 25.0));
    }

    #[test]
    fn hex_fractions_are_sixteenths() {
        // #A.8 = 10 + 8/16 = 10.5 twips = 0.525 px.
        let path = parse_edges("!#A.8 #0").unwrap();
        let p = points(&path)[0];
        assert!((p.x - 0.525).abs() < 1e-9, "got {}", p.x);
    }

    #[test]
    fn negative_hex_coordinates_parse() {
        let path = parse_edges("!-#3E8 #0").unwrap();
        assert_eq!(points(&path)[0].x, -50.0);
    }

    /// Real files mix the two representations in one string.
    #[test]
    fn decimal_and_hex_can_be_mixed() {
        let path = parse_edges("!1000 #1F4|#3E8 500").unwrap();
        let p = points(&path);
        assert_eq!(p[0], Point::new(50.0, 25.0));
        assert_eq!(p[1], Point::new(50.0, 25.0));
    }

    #[test]
    fn selection_markers_are_skipped() {
        let path = parse_edges("!0 0S1|2000 0S3|1000 1600").unwrap();
        assert_eq!(points(&path).len(), 3, "S tokens carry no geometry");
    }

    #[test]
    fn whitespace_between_tokens_is_tolerated() {
        let path = parse_edges("  ! 0 0 | 2000 0  ").unwrap();
        assert_eq!(points(&path).len(), 2);
    }

    #[test]
    fn an_empty_string_gives_an_empty_path() {
        assert!(parse_edges("").unwrap().elements().is_empty());
        assert!(parse_edges("   ").unwrap().elements().is_empty());
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(parse_edges("!0").is_err(), "missing y");
        assert!(parse_edges("@1 2").is_err(), "unknown command");
        assert!(parse_edges("!abc def").is_err(), "not a number");
        assert!(parse_edges("[1 2").is_err(), "curve missing its anchor");
    }

    #[test]
    fn a_curve_before_any_move_still_produces_a_path() {
        // Defensive: some generators omit the leading move.
        let path = parse_edges("|2000 0").unwrap();
        assert!(!path.elements().is_empty());
        assert!(matches!(path.elements()[0], PathEl::MoveTo(_)));
    }

    #[test]
    fn closing_produces_a_fillable_shape() {
        let open = parse_edges("!0 0|2000 0|2000 2000|0 2000").unwrap();
        assert!(open.area().abs() > 0.0);

        let closed = parse_edges_closed("!0 0|2000 0|2000 2000|0 2000").unwrap();
        assert!(
            closed
                .elements()
                .iter()
                .any(|e| matches!(e, PathEl::ClosePath)),
            "a filled shape should be explicitly closed"
        );
        assert!((closed.area().abs() - 10_000.0).abs() < 1.0, "100x100 px");
    }

    #[test]
    fn multiple_contours_each_get_closed() {
        let path = parse_edges_closed("!0 0|2000 0|0 2000!4000 0|6000 0|4000 2000").unwrap();
        let closes = path
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::ClosePath))
            .count();
        assert_eq!(closes, 2, "each contour needs its own close");
    }

    #[test]
    fn a_large_edge_string_parses_quickly() {
        let mut edges = String::from("!0 0");
        for i in 1..20_000 {
            edges.push_str(&format!("|{} {}", i * 20, (i % 100) * 20));
        }

        let started = std::time::Instant::now();
        let path = parse_edges(&edges).unwrap();
        assert_eq!(path.elements().len(), 20_000);
        assert!(
            started.elapsed().as_millis() < 500,
            "parsing 20k edges took {:?}",
            started.elapsed()
        );
    }
}
