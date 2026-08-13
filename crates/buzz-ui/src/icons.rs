//! Tool symbols, drawn rather than typed.
//!
//! # Why these are paths and not characters
//!
//! Every icon here could in principle be a character — there are Unicode
//! codepoints for a pencil, a paintbrush and a magnifier. They do not render:
//! egui's bundled fonts carry a small subset of Unicode, and a missing glyph
//! comes out as an empty box. That has bitten this project twice already (`▼`
//! in the Library and the Actions panel), and it is invisible to every test —
//! only a screenshot shows it.
//!
//! So each tool is drawn with the painter, from lines and polygons in a unit
//! square. It cannot fail to render, it is sharp at any button size, and it
//! costs a handful of vertices.
//!
//! The shapes follow Animate's toolbar, because an animator coming from there
//! should be able to find the Ink Bottle without reading a tooltip.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, vec2};

use crate::tools::ToolId;

/// Draw `tool`'s symbol, centred in `rect` and drawn in `color`.
pub fn tool_icon(painter: &Painter, rect: Rect, tool: ToolId, color: Color32) {
    painter.extend(tool_shapes(rect, tool, color));
}

/// `tool`'s symbol, as shapes.
///
/// Separate from the painting so a test can look at what a tool would draw: a
/// symbol that silently paints nothing is exactly the failure this module
/// exists to prevent, and a `Painter` keeps no record of what went into it.
pub fn tool_shapes(rect: Rect, tool: ToolId, color: Color32) -> Vec<Shape> {
    // A square drawing area in the middle of the button, so a wide button does
    // not stretch the symbol.
    let side = rect.width().min(rect.height());
    let area = Rect::from_center_size(rect.center(), vec2(side, side));
    let d = Draw {
        out: std::cell::RefCell::new(Vec::new()),
        area,
        color,
        weight: (side * 0.09).clamp(1.0, 2.0),
    };

    use ToolId::*;
    match tool {
        Selection => arrow(&d, true),
        Subselection => arrow(&d, false),
        FreeTransform => free_transform(&d),
        GradientTransform => gradient_transform(&d),
        Pen => pen(&d),
        Text => text(&d),
        Line => line_tool(&d),
        Rectangle => rectangle(&d),
        Oval => oval(&d),
        PolyStar => star(&d),
        Pencil => pencil(&d),
        Brush => brush(&d),
        Bone => bone(&d),
        AssetWarp => warp(&d),
        PaintBucket => bucket(&d),
        InkBottle => ink_bottle(&d),
        Eyedropper => eyedropper(&d),
        Eraser => eraser(&d),
        Camera => camera(&d),
        Hand => hand(&d),
        Zoom => zoom(&d),
    }

    d.out.into_inner()
}

/// The drawing surface, in unit coordinates: `(0,0)` top-left, `(1,1)` bottom
/// right of the icon's square.
struct Draw {
    out: std::cell::RefCell<Vec<Shape>>,
    area: Rect,
    color: Color32,
    weight: f32,
}

impl Draw {
    fn push(&self, shape: Shape) {
        self.out.borrow_mut().push(shape);
    }

    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.area.lerp_inside(vec2(x, y))
    }

    fn stroke(&self) -> Stroke {
        Stroke::new(self.weight, self.color)
    }

    fn thin(&self) -> Stroke {
        Stroke::new(self.weight * 0.7, self.color)
    }

    fn line(&self, a: (f32, f32), b: (f32, f32)) {
        self.push(Shape::line_segment(
            [self.at(a.0, a.1), self.at(b.0, b.1)],
            self.stroke(),
        ));
    }

    fn hairline(&self, a: (f32, f32), b: (f32, f32)) {
        self.push(Shape::line_segment(
            [self.at(a.0, a.1), self.at(b.0, b.1)],
            self.thin(),
        ));
    }

    /// A closed outline through the given unit points.
    fn outline(&self, points: &[(f32, f32)]) {
        let mut pts: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        pts.push(pts[0]);
        self.push(Shape::line(pts, self.stroke()));
    }

    /// A filled polygon through the given unit points.
    fn solid(&self, points: &[(f32, f32)]) {
        let pts: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.push(Shape::convex_polygon(pts, self.color, Stroke::NONE));
    }

    fn circle(&self, centre: (f32, f32), radius: f32, filled: bool) {
        let c = self.at(centre.0, centre.1);
        let r = radius * self.area.width();
        if filled {
            self.push(Shape::circle_filled(c, r, self.color));
        } else {
            self.push(Shape::circle_stroke(c, r, self.stroke()));
        }
    }

    /// An ellipse outline, which egui has no primitive for.
    fn ellipse(&self, centre: (f32, f32), rx: f32, ry: f32) {
        let steps = 28;
        let pts: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32 * std::f32::consts::TAU;
                self.at(centre.0 + rx * t.cos(), centre.1 + ry * t.sin())
            })
            .collect();
        self.push(Shape::line(pts, self.stroke()));
    }

    /// An open curve through the given unit points.
    fn curve(&self, points: &[(f32, f32)]) {
        let pts: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.push(Shape::line(pts, self.stroke()));
    }
}

// -- the symbols -------------------------------------------------------------

/// The pointer. Solid is Selection, hollow is Subselection — which is exactly
/// how Animate distinguishes them, and how every other editor does too.
fn arrow(d: &Draw, solid: bool) {
    let body = [
        (0.30, 0.14),
        (0.30, 0.80),
        (0.45, 0.65),
        (0.55, 0.86),
        (0.66, 0.80),
        (0.56, 0.60),
        (0.74, 0.58),
    ];
    if solid {
        // Two convex halves, because the arrow is not convex and egui's
        // polygon fill assumes it is.
        d.solid(&[(0.30, 0.14), (0.30, 0.80), (0.45, 0.65), (0.74, 0.58)]);
        d.solid(&[(0.45, 0.65), (0.55, 0.86), (0.66, 0.80), (0.56, 0.60)]);
    }
    d.outline(&body);
}

/// A box with corner handles: the transform frame Animate puts round a
/// selection.
fn free_transform(d: &Draw) {
    // Dashed sides and solid corner handles, so this reads as a *frame around
    // something* rather than as the Rectangle tool's box.
    for (a, b) in [
        ((0.30, 0.22), (0.70, 0.22)),
        ((0.30, 0.78), (0.70, 0.78)),
        ((0.22, 0.30), (0.22, 0.70)),
        ((0.78, 0.30), (0.78, 0.70)),
    ] {
        d.hairline(a, b);
    }
    let h = 0.075;
    for (x, y) in [(0.22, 0.22), (0.78, 0.22), (0.78, 0.78), (0.22, 0.78)] {
        d.solid(&[
            (x - h, y - h),
            (x + h, y - h),
            (x + h, y + h),
            (x - h, y + h),
        ]);
    }
}

/// A disc shading from full to empty, with the handle that aims it.
fn gradient_transform(d: &Draw) {
    d.circle((0.5, 0.5), 0.30, false);
    // The lit half, as three chords rather than a fill: it reads as a ramp.
    for (i, t) in [0.10f32, 0.20, 0.30].iter().enumerate() {
        let half = (0.09 - 0.022 * i as f32).max(0.02);
        d.hairline((0.5 - t, 0.5 - half * 2.0), (0.5 - t, 0.5 + half * 2.0));
    }
    d.line((0.5, 0.5), (0.86, 0.5));
    d.circle((0.86, 0.5), 0.06, true);
}

/// A loop with a tail: freehand selection.
fn pen(d: &Draw) {
    d.outline(&[(0.5, 0.12), (0.70, 0.56), (0.5, 0.72), (0.30, 0.56)]);
    d.hairline((0.5, 0.30), (0.5, 0.72));
    d.line((0.5, 0.74), (0.5, 0.88));
}

/// A capital T, drawn rather than typed so it matches the weight of the rest.
fn text(d: &Draw) {
    d.line((0.24, 0.22), (0.76, 0.22));
    d.line((0.5, 0.22), (0.5, 0.80));
}

fn line_tool(d: &Draw) {
    d.line((0.22, 0.78), (0.78, 0.22));
    d.circle((0.22, 0.78), 0.06, true);
    d.circle((0.78, 0.22), 0.06, true);
}

fn rectangle(d: &Draw) {
    d.outline(&[(0.20, 0.28), (0.80, 0.28), (0.80, 0.72), (0.20, 0.72)]);
}

fn oval(d: &Draw) {
    d.ellipse((0.5, 0.5), 0.30, 0.22);
}

fn star(d: &Draw) {
    let mut pts = Vec::new();
    for i in 0..10 {
        let r = if i % 2 == 0 { 0.32 } else { 0.14 };
        let t = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        pts.push((0.5 + r * t.cos(), 0.5 + r * t.sin()));
    }
    d.outline(&pts);
}

/// A pencil: barrel, collar and point, on Animate's diagonal.
fn pencil(d: &Draw) {
    d.outline(&[(0.30, 0.72), (0.62, 0.18), (0.78, 0.28), (0.46, 0.82)]);
    d.hairline((0.56, 0.22), (0.72, 0.32));
    // The point.
    d.solid(&[(0.46, 0.82), (0.30, 0.72), (0.26, 0.86)]);
}

/// A brush: a handle, a ferrule and a loaded tip that widens.
fn brush(d: &Draw) {
    d.line((0.68, 0.18), (0.44, 0.52));
    d.outline(&[(0.34, 0.50), (0.52, 0.62), (0.40, 0.80), (0.24, 0.70)]);
    d.solid(&[(0.30, 0.68), (0.40, 0.74), (0.32, 0.86), (0.22, 0.80)]);
}

/// Animate's bone: a joint, a taper, a tip.
fn bone(d: &Draw) {
    d.solid(&[(0.26, 0.62), (0.36, 0.46), (0.78, 0.30), (0.40, 0.72)]);
    d.circle((0.28, 0.64), 0.10, false);
    d.circle((0.76, 0.30), 0.055, true);
}

/// A square pushed out of shape, with the handles that did it.
fn warp(d: &Draw) {
    d.curve(&[
        (0.24, 0.28),
        (0.50, 0.20),
        (0.76, 0.28),
        (0.70, 0.52),
        (0.78, 0.74),
        (0.50, 0.80),
        (0.26, 0.74),
        (0.30, 0.50),
        (0.24, 0.28),
    ]);
    for (x, y) in [(0.24, 0.28), (0.76, 0.28), (0.78, 0.74), (0.26, 0.74)] {
        d.circle((x, y), 0.07, true);
    }
}

/// A tipped bucket with paint coming out of it.
fn bucket(d: &Draw) {
    d.outline(&[(0.22, 0.34), (0.62, 0.20), (0.72, 0.52), (0.36, 0.70)]);
    d.hairline((0.30, 0.30), (0.66, 0.36));
    // The handle, and the drip.
    d.curve(&[(0.30, 0.30), (0.36, 0.16), (0.54, 0.16)]);
    d.circle((0.76, 0.72), 0.075, true);
}

/// A bottle with a nib, which is how Animate draws its stroke tool.
fn ink_bottle(d: &Draw) {
    d.outline(&[(0.32, 0.44), (0.66, 0.44), (0.72, 0.82), (0.26, 0.82)]);
    d.line((0.42, 0.44), (0.42, 0.26));
    d.line((0.36, 0.26), (0.60, 0.16));
    d.circle((0.72, 0.30), 0.07, true);
}

/// A dropper: bulb, barrel, point.
fn eyedropper(d: &Draw) {
    d.line((0.68, 0.24), (0.38, 0.56));
    d.circle((0.72, 0.22), 0.10, false);
    d.solid(&[(0.40, 0.54), (0.46, 0.62), (0.24, 0.80)]);
}

/// A rubber block on its edge, with the band across it.
fn eraser(d: &Draw) {
    d.outline(&[(0.20, 0.62), (0.52, 0.24), (0.80, 0.42), (0.48, 0.80)]);
    // The working end, filled: an outline alone reads as a plain diamond, and
    // it is the two-tone block that says "eraser".
    d.solid(&[(0.20, 0.62), (0.34, 0.46), (0.64, 0.64), (0.48, 0.80)]);
}

fn camera(d: &Draw) {
    d.outline(&[(0.18, 0.34), (0.62, 0.34), (0.62, 0.72), (0.18, 0.72)]);
    // The lens hood, which is what says "camera" rather than "box".
    d.solid(&[(0.64, 0.44), (0.84, 0.34), (0.84, 0.72), (0.64, 0.62)]);
    d.circle((0.38, 0.53), 0.09, false);
}

/// A hand: palm and four fingers, the panning tool.
fn hand(d: &Draw) {
    d.outline(&[
        (0.30, 0.84),
        (0.26, 0.54),
        (0.30, 0.42),
        (0.34, 0.52),
        (0.34, 0.24),
        (0.42, 0.24),
        (0.44, 0.48),
        (0.48, 0.22),
        (0.56, 0.22),
        (0.58, 0.48),
        (0.62, 0.28),
        (0.70, 0.30),
        (0.70, 0.66),
        (0.64, 0.84),
    ]);
}

fn zoom(d: &Draw) {
    d.circle((0.44, 0.44), 0.24, false);
    d.line((0.62, 0.62), (0.82, 0.82));
    d.hairline((0.32, 0.44), (0.56, 0.44));
    d.hairline((0.44, 0.32), (0.44, 0.56));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::all_tools;

    fn icon(tool: ToolId, side: f32) -> Vec<Shape> {
        tool_shapes(
            Rect::from_min_size(Pos2::ZERO, vec2(side, side)),
            tool,
            Color32::WHITE,
        )
    }

    /// Every tool draws something. A tool whose symbol was forgotten would come
    /// out as an empty button — exactly the failure this module exists to
    /// prevent, and one no rendering test would ever catch.
    #[test]
    fn every_tool_draws_a_symbol() {
        for tool in all_tools() {
            assert!(!icon(tool, 30.0).is_empty(), "{tool:?} drew nothing");
        }
    }

    /// The symbols are built in unit coordinates, so they must survive any
    /// button size — including silly ones — and stay inside the button.
    #[test]
    fn a_symbol_stays_inside_its_button() {
        for side in [4.0f32, 30.0, 240.0] {
            let button = Rect::from_min_size(Pos2::ZERO, vec2(side, side));
            for tool in all_tools() {
                for shape in tool_shapes(button, tool, Color32::WHITE) {
                    let bounds = shape.visual_bounding_rect();
                    if !bounds.is_positive() {
                        continue;
                    }
                    // A stroke is drawn centred on its path, so allow its
                    // half-width to fall outside the geometry.
                    assert!(
                        button.expand(3.0).contains_rect(bounds),
                        "{tool:?} at {side}px drew {bounds:?} outside {button:?}"
                    );
                }
            }
        }
    }

    /// A non-square button gets a square symbol, centred — not a stretched one.
    #[test]
    fn a_wide_button_does_not_stretch_the_symbol() {
        let bounds = |shapes: Vec<Shape>| {
            shapes
                .iter()
                .map(|s| s.visual_bounding_rect())
                .reduce(|a, b| a.union(b))
                .expect("shapes")
        };
        let wide = bounds(tool_shapes(
            Rect::from_min_size(Pos2::ZERO, vec2(90.0, 20.0)),
            ToolId::Rectangle,
            Color32::WHITE,
        ));
        let square = bounds(icon(ToolId::Rectangle, 20.0));
        assert!(
            (wide.width() - square.width()).abs() < 0.5
                && (wide.height() - square.height()).abs() < 0.5,
            "the symbol was stretched: {wide:?} vs {square:?}"
        );
    }

    /// No two tools draw the same thing, or the strip would be unreadable.
    #[test]
    fn no_two_tools_share_a_symbol() {
        // The whole shape, not its bounding box: a rectangle outline and an
        // ellipse of the same size share a box and look nothing alike.
        let sketch = |tool: ToolId| {
            icon(tool, 64.0)
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
        };
        let tools = all_tools();
        for (i, a) in tools.iter().enumerate() {
            for b in &tools[i + 1..] {
                assert_ne!(sketch(*a), sketch(*b), "{a:?} and {b:?} look the same");
            }
        }
    }
}
