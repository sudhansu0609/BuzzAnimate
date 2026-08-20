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
        Lasso => lasso(&d),
        MagicWand => magic_wand(&d),
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

/// A rope loop with its tail hanging, which is what Animate draws and what
/// the gesture actually is: you throw a loop round something.
fn lasso(d: &Draw) {
    // The loop, an ellipse leaning slightly as a thrown rope does.
    let steps = 26;
    let pts: Vec<(f32, f32)> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32 * std::f32::consts::TAU;
            (0.50 + 0.24 * t.cos(), 0.36 + 0.17 * t.sin())
        })
        .collect();
    d.curve(&pts);
    // The tail, falling from where the loop crosses itself.
    d.curve(&[
        (0.36, 0.49),
        (0.32, 0.64),
        (0.38, 0.76),
        (0.34, 0.86),
    ]);
}

/// A wand at an angle with sparks off its tip.
fn magic_wand(d: &Draw) {
    d.line((0.28, 0.80), (0.63, 0.38));
    // The tip, and three sparks leaving it — enough to read as "magic" and few
    // enough to stay legible at sixteen pixels.
    d.circle((0.66, 0.34), 0.05, true);
    for (a, b) in [
        ((0.66, 0.20), (0.66, 0.11)),
        ((0.78, 0.30), (0.86, 0.26)),
        ((0.76, 0.19), (0.83, 0.13)),
    ] {
        d.hairline(a, b);
    }
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

/// A gradient in a box, with the handle that stretches it.
///
/// The ramp is drawn as bands thinning left to right, because a fill here
/// cannot itself be a gradient. Few and inside the box: a column of even bars
/// reads as a barcode, which is what this looked like when there were more.
fn gradient_transform(d: &Draw) {
    // The box the gradient fills.
    d.outline(&[(0.22, 0.22), (0.78, 0.22), (0.78, 0.60), (0.22, 0.60)]);
    // Solid at the left, thinning to nothing at the right.
    d.solid(&[(0.24, 0.24), (0.40, 0.24), (0.40, 0.58), (0.24, 0.58)]);
    d.line((0.48, 0.24), (0.48, 0.58));
    d.hairline((0.58, 0.24), (0.58, 0.58));
    d.hairline((0.68, 0.30), (0.68, 0.52));

    // The handle: drag it and the ramp follows.
    d.line((0.24, 0.78), (0.76, 0.78));
    d.circle((0.24, 0.78), 0.07, false);
    d.circle((0.76, 0.78), 0.07, true);
}

/// A fountain-pen nib: a tapered blade with a vent hole and the slit running
/// from it to the point.
///
/// **Outlined rather than filled.** A solid nib is a diamond — the slit and
/// the vent are what make it a nib, and they are absences. Drawn on top of a
/// fill they are the same colour as the fill and simply are not there, which
/// is exactly how this icon came to read as another arrowhead.
fn pen(d: &Draw) {
    // The blade: narrow at the shoulder, narrowing again to the point.
    d.outline(&[
        (0.42, 0.18),
        (0.58, 0.18),
        (0.64, 0.50),
        (0.50, 0.86),
        (0.36, 0.50),
    ]);
    // The vent hole, and the slit from it down to the point.
    d.circle((0.50, 0.48), 0.06, false);
    d.hairline((0.50, 0.54), (0.50, 0.84));
    // The shoulder, where the nib meets the holder.
    d.hairline((0.42, 0.26), (0.58, 0.26));
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

/// A paintbrush: handle, ferrule, and a bristle head that comes to a point.
/// The old shape read as a shovel, which is the wrong tool entirely.
fn brush(d: &Draw) {
    // The handle, running out to the top right.
    d.line((0.60, 0.36), (0.84, 0.14));
    // The ferrule: the metal band, solid.
    d.solid(&[(0.46, 0.42), (0.62, 0.28), (0.70, 0.36), (0.54, 0.50)]);
    // The bristles, a solid head tapering to the tip that paints.
    d.solid(&[(0.54, 0.50), (0.46, 0.42), (0.24, 0.76), (0.20, 0.84)]);
    d.solid(&[(0.24, 0.76), (0.20, 0.84), (0.30, 0.82)]);
}

/// A bone: a shaft with a knuckle at each end. Animate's Bone tool, and the
/// shape everybody draws for a skeleton — the old one was a lopsided blob.
fn bone(d: &Draw) {
    // The shaft.
    d.solid(&[(0.34, 0.40), (0.66, 0.60), (0.62, 0.68), (0.30, 0.48)]);
    // The knuckles, two lobes at each end.
    d.circle((0.28, 0.36), 0.10, true);
    d.circle((0.36, 0.28), 0.09, true);
    d.circle((0.72, 0.64), 0.10, true);
    d.circle((0.64, 0.72), 0.09, true);
}

/// A mesh with a pin in it: Animate's Asset Warp puts handles on artwork and
/// bends it between them, and a grid is what says "this deforms".
fn warp(d: &Draw) {
    // A bowed grid, so it reads as something already deformed.
    d.curve(&[(0.22, 0.30), (0.50, 0.22), (0.78, 0.30)]);
    d.curve(&[(0.22, 0.52), (0.50, 0.44), (0.78, 0.52)]);
    d.curve(&[(0.22, 0.74), (0.50, 0.66), (0.78, 0.74)]);
    d.curve(&[(0.22, 0.30), (0.18, 0.52), (0.22, 0.74)]);
    d.curve(&[(0.50, 0.22), (0.50, 0.44), (0.50, 0.66)]);
    d.curve(&[(0.78, 0.30), (0.82, 0.52), (0.78, 0.74)]);
    // The pins.
    d.circle((0.22, 0.30), 0.07, true);
    d.circle((0.78, 0.74), 0.07, true);
}

/// A bucket tipped to pour, with a drop leaving the lip. Animate's Paint
/// Bucket, and the shape reads as a bucket only when the sides taper — the
/// old parallelogram did not.
fn bucket(d: &Draw) {
    // The body, tapering downwards and tilted to pour.
    d.solid(&[(0.24, 0.34), (0.58, 0.22), (0.66, 0.52), (0.40, 0.62)]);
    // The rim, a touch proud of the body so the opening reads.
    d.line((0.22, 0.33), (0.60, 0.20));
    // The handle over the top.
    d.curve(&[(0.28, 0.28), (0.40, 0.10), (0.58, 0.18)]);
    // The pour, and the drop that has already left.
    d.curve(&[(0.64, 0.46), (0.74, 0.60), (0.76, 0.70)]);
    d.circle((0.78, 0.80), 0.07, true);
}

/// An ink bottle with ink in it and a drop leaving: the Ink Bottle changes a
/// *stroke*, and the drop is what it leaves behind.
///
/// The body is outlined and the ink inside it filled. Stacking solid neck on
/// solid body on solid stopper merged the three into one tower with no bottle
/// anywhere in it.
fn ink_bottle(d: &Draw) {
    // The body, shouldered out from the neck.
    d.outline(&[(0.30, 0.46), (0.70, 0.46), (0.74, 0.80), (0.26, 0.80)]);
    // The ink standing in it.
    d.solid(&[(0.32, 0.62), (0.72, 0.62), (0.735, 0.785), (0.265, 0.785)]);
    // The neck.
    d.outline(&[(0.44, 0.28), (0.56, 0.28), (0.56, 0.46), (0.44, 0.46)]);
    // The stopper.
    d.solid(&[(0.41, 0.18), (0.59, 0.18), (0.59, 0.28), (0.41, 0.28)]);
    // And the drop it has let go.
    d.circle((0.84, 0.58), 0.07, true);
}

/// A dropper: a squeezed bulb, a barrel, and a fine tip. The old one was a
/// bare diagonal with a dot, which reads as nothing in particular.
fn eyedropper(d: &Draw) {
    // The bulb.
    d.circle((0.70, 0.28), 0.13, true);
    // The barrel running down to the tip.
    d.solid(&[(0.60, 0.36), (0.70, 0.46), (0.34, 0.78), (0.26, 0.70)]);
    // The tip, a fine point where the colour is taken from.
    d.solid(&[(0.26, 0.70), (0.34, 0.78), (0.18, 0.86)]);
}

/// A block eraser, tilted, with its working face showing. Animate draws the
/// face because that is what tells an eraser from a plain quadrilateral.
fn eraser(d: &Draw) {
    // The body.
    d.solid(&[(0.28, 0.62), (0.56, 0.26), (0.78, 0.40), (0.50, 0.76)]);
    // The face it rubs with, outlined so it reads as a separate surface.
    d.outline(&[(0.28, 0.62), (0.50, 0.76), (0.44, 0.86), (0.22, 0.72)]);
    // And the crumbs it leaves.
    d.hairline((0.60, 0.80), (0.70, 0.80));
    d.hairline((0.66, 0.88), (0.78, 0.88));
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

/// **A contact sheet of every tool symbol, as JSON.**
///
/// Drawn icons cannot be judged by reading their coordinates, and no test can
/// tell a convincing paintbrush from an unconvincing one. This dumps the
/// geometry so it can be rasterised and *looked at*, which is the only way to
/// answer "does this read as Animate's toolbar".
///
/// Ignored by default; it writes a file and answers a question no assertion
/// can:
///
/// ```text
/// cargo test -p buzz-ui --lib dump_tool_icons -- --ignored --nocapture
/// ```
#[cfg(test)]
mod contact_sheet {
    use super::*;

    #[test]
    #[ignore = "writes a file for a person to look at"]
    fn dump_tool_icons() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        let mut out = String::from("{\n");
        let tools = crate::tools::all_tools();
        for (i, tool) in tools.iter().enumerate() {
            let shapes = tool_shapes(area, *tool, Color32::BLACK);
            out.push_str(&format!("  \"{:?}\": [\n", tool));
            let mut parts: Vec<String> = Vec::new();
            for shape in &shapes {
                parts.push(describe(shape));
            }
            out.push_str(&parts.join(",\n"));
            out.push_str("\n  ]");
            if i + 1 < tools.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("}\n");

        let path = std::env::temp_dir().join("buzz-tool-icons.json");
        std::fs::write(&path, out).expect("write the sheet");
        println!("wrote {}", path.display());
    }

    fn describe(shape: &Shape) -> String {
        match shape {
            Shape::LineSegment { points, stroke } => format!(
                "    {{\"kind\":\"line\",\"w\":{},\"pts\":[[{},{}],[{},{}]]}}",
                stroke.width, points[0].x, points[0].y, points[1].x, points[1].y
            ),
            Shape::Path(path) => {
                let pts: Vec<String> = path
                    .points
                    .iter()
                    .map(|p| format!("[{},{}]", p.x, p.y))
                    .collect();
                format!(
                    "    {{\"kind\":\"path\",\"fill\":{},\"w\":{},\"closed\":{},\"pts\":[{}]}}",
                    path.fill != Color32::TRANSPARENT,
                    path.stroke.width,
                    path.closed,
                    pts.join(",")
                )
            }
            Shape::Circle(c) => format!(
                "    {{\"kind\":\"circle\",\"fill\":{},\"w\":{},\"c\":[{},{}],\"r\":{}}}",
                c.fill != Color32::TRANSPARENT,
                c.stroke.width,
                c.center.x,
                c.center.y,
                c.radius
            ),
            Shape::Rect(r) => format!(
                "    {{\"kind\":\"rect\",\"fill\":{},\"w\":{},\"pts\":[[{},{}],[{},{}]]}}",
                r.fill != Color32::TRANSPARENT,
                r.stroke.width,
                r.rect.min.x,
                r.rect.min.y,
                r.rect.max.x,
                r.rect.max.y
            ),
            other => format!("    {{\"kind\":\"other\",\"debug\":{:?}}}", format!("{other:?}").len()),
        }
    }
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
