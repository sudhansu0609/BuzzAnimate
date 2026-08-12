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

    /// `#RRRR.FF` — hex twips, optionally with a hex fraction in 256ths.
    fn hex_fixed_point(&mut self, start: usize) -> Result<f64, EdgeError> {
        let integer_start = self.at;
        while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
            self.at += 1;
        }
        if self.at == integer_start {
            return Err(EdgeError::ExpectedNumber { at: start });
        }
        let integer = i64::from_str_radix(
            std::str::from_utf8(&self.bytes[integer_start..self.at]).unwrap_or("0"),
            16,
        )
        .map_err(|_| EdgeError::ExpectedNumber { at: start })?;

        let mut fraction = 0.0;
        if self.peek() == Some(b'.') {
            self.at += 1;
            let fraction_start = self.at;
            while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                self.at += 1;
            }
            if self.at > fraction_start {
                let digits = std::str::from_utf8(&self.bytes[fraction_start..self.at])
                    .unwrap_or("0");
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
            closed.elements().iter().any(|e| matches!(e, PathEl::ClosePath)),
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
