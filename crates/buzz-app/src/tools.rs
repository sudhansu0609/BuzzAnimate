//! Tool behaviour.
//!
//! [`buzz_ui::tools`] is the catalogue — names, glyphs, shortcuts. This is what
//! the tools actually *do*.
//!
//! # Shape of the design
//!
//! A gesture is three events: press, drag, release. [`ToolAction`] is what a
//! tool asks for, and the editor carries it out — tools never touch the
//! document directly. That keeps undo labelling, layer locking and the
//! merge-shape rules in one place instead of repeated in every tool, where one
//! of them would inevitably be forgotten.

use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _, Vec2};
use buzz_scene::ShapeData;
use buzz_ui::{DrawStyle, ToolId};
use kurbo::{Circle, Ellipse, Line};
use peniko::Color;

/// Keyboard modifiers during a gesture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// What a tool wants the editor to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolAction {
    /// Nothing yet; the gesture is still in progress.
    None,
    /// Add this shape to the active layer, honouring the drawing mode.
    AddShape {
        shape: ShapeData,
        label: &'static str,
    },
    /// Replace the selection with whatever is under the point.
    PickAt { point: Point, additive: bool },
    /// Select everything intersecting the rectangle.
    PickInRect { rect: Rect, additive: bool },
    /// Move the current selection.
    MoveSelection { delta: Vec2 },
    /// Scale the selection about a fixed corner.
    TransformSelection { transform: Affine },
    /// Drag one anchor of the selected path — Animate's Subselection tool.
    MoveAnchor { element: usize, delta: Vec2 },
    /// Erase within a stroked path.
    Erase { path: BezPath, width: f64 },
    /// Fill whatever is under the point with the current fill colour.
    BucketFill { point: Point },
    /// Apply the current stroke to whatever is under the point.
    ApplyStroke { point: Point },
    /// Adopt the colour under the point.
    SampleColor { point: Point },
    /// Pan the view.
    PanView { delta_screen: Vec2 },
    /// Move the document camera. Unlike `PanView` this changes the animation.
    MoveCamera { delta_doc: Vec2 },
    /// Zoom about a screen point.
    ZoomView { factor: f64, at_screen: Point },
    /// Clear the selection.
    Deselect,
}

/// Live feedback while a gesture is in progress.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    None,
    /// Outline of the shape being drawn.
    Shape(BezPath),
    /// Rubber-band selection rectangle.
    Marquee(Rect),
    /// Freehand stroke so far.
    Stroke {
        path: BezPath,
        width: f64,
    },
    /// Brush artwork as it will actually be painted, in its real colour.
    ///
    /// Drawn by the artwork renderer rather than the chrome, because for a
    /// brush the preview is the result.
    Ink {
        path: BezPath,
        color: Color,
    },
}

/// State shared with a tool for the duration of a gesture.
pub struct ToolContext<'a> {
    pub style: &'a DrawStyle,
    /// Document units per screen pixel, for size-independent thresholds.
    pub zoom: f64,
    pub selection_bounds: Option<Rect>,
    /// Anchors of the single selected shape, in document space.
    ///
    /// Supplied by the editor because only it can see the scene; empty unless
    /// exactly one shape is selected.
    pub anchors: &'a [buzz_geom::Anchor],
}

/// A gesture in progress.
#[derive(Debug, Clone, PartialEq)]
enum Gesture {
    Idle,
    /// Press-drag-release from a fixed origin.
    Dragging {
        origin: Point,
        current: Point,
        mods: Mods,
    },
    /// Accumulating freehand samples.
    ///
    /// Samples rather than bare points because a brush needs *when* as well as
    /// where: the width of a fluid stroke follows how fast it was drawn.
    Freehand {
        samples: Vec<buzz_geom::StrokeSample>,
    },
    /// Dragging one anchor of a path.
    Anchor {
        element: usize,
        origin: Point,
        current: Point,
    },
}

/// Where stroke timing comes from.
///
/// Wall time in the running application. A test needs the other arm: a brush
/// whose width follows *speed* cannot be tested against a clock that runs at
/// whatever pace the machine happens to manage, so a test supplies the times
/// itself and gets the same answer every run.
#[derive(Debug, Clone, Copy)]
enum Clock {
    Wall(std::time::Instant),
    Manual(f64),
}

impl Clock {
    fn now(&self) -> f64 {
        match self {
            Self::Wall(start) => start.elapsed().as_secs_f64(),
            Self::Manual(t) => *t,
        }
    }
}

/// Drives one tool through a gesture.
#[derive(Debug, Clone)]
pub struct ToolMachine {
    tool: ToolId,
    gesture: Gesture,
    /// Screen-space position, for the navigation tools.
    last_screen: Point,
    clock: Clock,
}

/// A drag shorter than this is a click, not a drag.
///
/// In *screen* pixels, converted using the zoom, so the threshold feels the
/// same at every magnification.
const CLICK_SLOP_PX: f64 = 3.0;

/// How close, in screen pixels, a click must come to grab an anchor.
const ANCHOR_GRAB_PX: f64 = 7.0;

impl ToolMachine {
    pub fn new(tool: ToolId) -> Self {
        Self {
            tool,
            gesture: Gesture::Idle,
            last_screen: Point::ORIGIN,
            clock: Clock::Wall(std::time::Instant::now()),
        }
    }

    /// Drive stroke timing from a supplied clock instead of wall time.
    ///
    /// For tests of speed-dependent behaviour, which cannot use a real clock
    /// and stay reproducible.
    pub fn set_time(&mut self, seconds: f64) {
        self.clock = Clock::Manual(seconds);
    }

    pub fn tool(&self) -> ToolId {
        self.tool
    }

    /// Switch tools, abandoning any gesture in progress.
    pub fn set_tool(&mut self, tool: ToolId) {
        self.tool = tool;
        self.gesture = Gesture::Idle;
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.gesture, Gesture::Idle)
    }

    /// Abandon the current gesture, as Escape does.
    pub fn cancel(&mut self) {
        self.gesture = Gesture::Idle;
    }

    pub fn pointer_down(
        &mut self,
        doc: Point,
        screen: Point,
        mods: Mods,
        ctx: &ToolContext<'_>,
    ) -> ToolAction {
        self.last_screen = screen;
        match self.tool {
            ToolId::Pencil | ToolId::Brush | ToolId::Eraser => {
                self.gesture = Gesture::Freehand {
                    samples: vec![buzz_geom::StrokeSample::new(doc, self.clock.now())],
                };
            }
            // Subselection grabs an anchor if one is close enough; otherwise it
            // falls through to ordinary selection behaviour.
            ToolId::Subselection => {
                let tolerance = ANCHOR_GRAB_PX / ctx.zoom.max(f64::MIN_POSITIVE);
                let grabbed = ctx
                    .anchors
                    .iter()
                    .map(|a| (a, (a.point - doc).hypot()))
                    .filter(|(_, d)| *d <= tolerance)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(a, _)| *a);

                self.gesture = match grabbed {
                    Some(anchor) => Gesture::Anchor {
                        element: anchor.element,
                        origin: doc,
                        current: doc,
                    },
                    None => Gesture::Dragging {
                        origin: doc,
                        current: doc,
                        mods,
                    },
                };
            }
            _ => {
                self.gesture = Gesture::Dragging {
                    origin: doc,
                    current: doc,
                    mods,
                };
            }
        }
        ToolAction::None
    }

    pub fn pointer_move(&mut self, doc: Point, screen: Point, mods: Mods) -> ToolAction {
        let delta_screen = screen - self.last_screen;
        self.last_screen = screen;

        match &mut self.gesture {
            Gesture::Idle => ToolAction::None,
            Gesture::Anchor { current, .. } => {
                *current = doc;
                ToolAction::None
            }
            Gesture::Freehand { samples } => {
                // Drop samples that add nothing, so a slow drag does not build
                // a path with thousands of coincident vertices. The brush
                // decimates properly later; this is only to stop the list
                // growing without bound while the pointer sits still.
                if samples
                    .last()
                    .is_none_or(|s| (doc - s.point).hypot() > f64::EPSILON)
                {
                    samples.push(buzz_geom::StrokeSample::new(doc, self.clock.now()));
                }
                ToolAction::None
            }
            Gesture::Dragging {
                current, mods: m, ..
            } => {
                let previous = *current;
                *current = doc;
                *m = mods;
                match self.tool {
                    ToolId::Hand => ToolAction::PanView { delta_screen },
                    // The camera moves live so the user can see the framing
                    // they are choosing, rather than only on release.
                    ToolId::Camera => ToolAction::MoveCamera {
                        delta_doc: doc - previous,
                    },
                    _ => ToolAction::None,
                }
            }
        }
    }

    pub fn pointer_up(&mut self, doc: Point, screen: Point, ctx: &ToolContext<'_>) -> ToolAction {
        let gesture = std::mem::replace(&mut self.gesture, Gesture::Idle);
        self.last_screen = screen;

        match gesture {
            Gesture::Idle => ToolAction::None,
            Gesture::Anchor {
                element, origin, ..
            } => {
                let delta = doc - origin;
                if delta.hypot() <= f64::EPSILON {
                    ToolAction::None
                } else {
                    ToolAction::MoveAnchor { element, delta }
                }
            }
            Gesture::Freehand { mut samples } => {
                if samples.last().is_none_or(|s| s.point != doc) {
                    samples.push(buzz_geom::StrokeSample::new(doc, self.clock.now()));
                }
                self.finish_freehand(samples, ctx)
            }
            Gesture::Dragging { origin, mods, .. } => self.finish_drag(origin, doc, mods, ctx),
        }
    }

    /// The current preview, for drawing feedback on the stage.
    pub fn preview(&self, ctx: &ToolContext<'_>) -> Preview {
        match &self.gesture {
            Gesture::Idle => Preview::None,
            // The stage already draws anchors for the selected path; a
            // rubber-band line here would just add noise.
            Gesture::Anchor { .. } => Preview::None,
            Gesture::Freehand { samples } => match self.tool {
                ToolId::Eraser => Preview::Stroke {
                    path: centreline_of(samples),
                    width: ctx.style.stroke_width.max(1.0) * 4.0,
                },
                // The brush previews what it will actually paint, under the
                // *preview* budget. That budget is what keeps a long stroke
                // interactive: this runs on every pointer move, so it has to
                // cost a fraction of the committed geometry, and a pattern
                // brush at close spacing would otherwise place thousands of
                // stamps per frame.
                ToolId::Brush => {
                    let budget = buzz_geom::BrushBudget::preview();
                    let color = ctx
                        .style
                        .fill_for_new_shape()
                        .unwrap_or(Color::BLACK)
                        // Slightly transparent, so the preview reads as
                        // provisional without misrepresenting its shape.
                        .multiply_alpha(0.85);
                    match build_brush_path(samples, ctx.style, &budget) {
                        Some(path) => Preview::Ink { path, color },
                        None => Preview::Stroke {
                            path: centreline_of(samples),
                            width: ctx.style.brush.size.max(1.0),
                        },
                    }
                }
                _ => Preview::Stroke {
                    path: centreline_of(samples),
                    width: brush_width(self.tool, ctx.style),
                },
            },
            Gesture::Dragging {
                origin,
                current,
                mods,
            } => match self.tool {
                ToolId::Rectangle
                | ToolId::Oval
                | ToolId::PolyStar
                | ToolId::Line
                | ToolId::Pen => build_shape_path(self.tool, *origin, *current, *mods)
                    .map(Preview::Shape)
                    .unwrap_or(Preview::None),
                ToolId::Selection | ToolId::Lasso | ToolId::Subselection => {
                    Preview::Marquee(Rect::from_points(*origin, *current))
                }
                ToolId::Zoom => Preview::Marquee(Rect::from_points(*origin, *current)),
                _ => Preview::None,
            },
        }
    }

    fn finish_freehand(
        &self,
        samples: Vec<buzz_geom::StrokeSample>,
        ctx: &ToolContext<'_>,
    ) -> ToolAction {
        // A brush tap paints a dot, so one sample is enough for it; every
        // other freehand tool needs a real drag.
        let is_brush = self.tool == ToolId::Brush;
        if samples.len() < 2 && !is_brush {
            return ToolAction::None;
        }

        match self.tool {
            ToolId::Eraser => ToolAction::Erase {
                path: centreline_of(&samples),
                width: ctx.style.stroke_width.max(1.0) * 4.0,
            },
            ToolId::Brush => {
                // The brush paints a filled stroke, so its colour comes from
                // the fill swatch — as in Animate.
                let budget = buzz_geom::BrushBudget::default();
                let Some(path) = build_brush_path(&samples, ctx.style, &budget) else {
                    return ToolAction::None;
                };
                if path.elements().is_empty() {
                    return ToolAction::None;
                }
                ToolAction::AddShape {
                    shape: ShapeData::filled(
                        path,
                        ctx.style.fill_for_new_shape().unwrap_or(Color::BLACK),
                    )
                    .with_blend(ctx.style.brush.blend()),
                    label: "Brush",
                }
            }
            _ => {
                let (color, width, hairline) =
                    ctx.style
                        .stroke_for_new_shape()
                        .unwrap_or((Color::BLACK, 1.0, false));
                ToolAction::AddShape {
                    shape: ShapeData {
                        path: centreline_of(&samples),
                        fill: None,
                        stroke: Some(buzz_scene::StrokeSpec {
                            color,
                            width,
                            hairline,
                        }),
                        blend: buzz_scene::PaintBlend::Normal,
                    },
                    label: "Pencil",
                }
            }
        }
    }

    fn finish_drag(
        &self,
        origin: Point,
        end: Point,
        mods: Mods,
        ctx: &ToolContext<'_>,
    ) -> ToolAction {
        let slop = CLICK_SLOP_PX / ctx.zoom.max(f64::MIN_POSITIVE);
        let was_click = (end - origin).hypot() <= slop;

        match self.tool {
            ToolId::Selection | ToolId::Subselection => {
                if was_click {
                    ToolAction::PickAt {
                        point: end,
                        additive: mods.shift,
                    }
                } else if ctx.selection_bounds.is_some_and(|b| contains(b, origin)) {
                    // Started inside the selection: move it.
                    ToolAction::MoveSelection {
                        delta: end - origin,
                    }
                } else {
                    ToolAction::PickInRect {
                        rect: Rect::from_points(origin, end),
                        additive: mods.shift,
                    }
                }
            }

            ToolId::FreeTransform => match ctx.selection_bounds {
                Some(bounds) if !was_click => ToolAction::TransformSelection {
                    transform: scale_about_corner(bounds, origin, end, mods.shift),
                },
                Some(_) => ToolAction::None,
                None => ToolAction::PickAt {
                    point: end,
                    additive: false,
                },
            },

            ToolId::Rectangle | ToolId::Oval | ToolId::PolyStar | ToolId::Line | ToolId::Pen => {
                if was_click {
                    return ToolAction::None;
                }
                let Some(path) = build_shape_path(self.tool, origin, end, mods) else {
                    return ToolAction::None;
                };
                let filled = self.tool != ToolId::Line && self.tool != ToolId::Pen;
                ToolAction::AddShape {
                    shape: ShapeData {
                        path,
                        fill: filled
                            .then(|| ctx.style.fill_for_new_shape())
                            .flatten()
                            .map(buzz_scene::FillSpec::solid),
                        blend: buzz_scene::PaintBlend::Normal,
                        stroke: ctx
                            .style
                            .stroke_for_new_shape()
                            .map(|(color, width, hairline)| buzz_scene::StrokeSpec {
                                color,
                                width,
                                hairline,
                            }),
                    },
                    label: shape_label(self.tool),
                }
            }

            ToolId::PaintBucket => ToolAction::BucketFill { point: end },
            ToolId::InkBottle => ToolAction::ApplyStroke { point: end },
            ToolId::Eyedropper => ToolAction::SampleColor { point: end },

            ToolId::Zoom => {
                if was_click {
                    // Alt-click zooms out, as in Animate.
                    let factor = if mods.alt { 0.5 } else { 2.0 };
                    ToolAction::ZoomView {
                        factor,
                        at_screen: self.last_screen,
                    }
                } else {
                    ToolAction::None
                }
            }

            // Both act during the drag; there is nothing left to do on release.
            ToolId::Hand | ToolId::Camera => ToolAction::None,
            _ => ToolAction::None,
        }
    }
}

fn shape_label(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Rectangle => "Draw Rectangle",
        ToolId::Oval => "Draw Oval",
        ToolId::PolyStar => "Draw PolyStar",
        ToolId::Line => "Draw Line",
        ToolId::Pen => "Draw Path",
        _ => "Draw",
    }
}

fn brush_width(tool: ToolId, style: &DrawStyle) -> f64 {
    match tool {
        // Animate's brush is much fatter than the pencil at the same setting.
        ToolId::Brush => style.brush.size.max(2.0),
        _ => style.stroke_width.max(0.1),
    }
}

/// A smooth curve through the samples.
///
/// Used by the pencil, the eraser and every brush preview that is not painting
/// its own artwork. Smoothing is applied first, so the pencil gets the same
/// steadying the brush does — Animate's Pencil has a Smoothing setting for the
/// same reason.
fn centreline_of(samples: &[buzz_geom::StrokeSample]) -> BezPath {
    buzz_geom::centreline(samples)
}

/// Build what the brush will paint, whichever brush is selected.
///
/// Shared by the preview and the committed geometry so the two cannot drift
/// apart; they differ only in the budget they are given.
///
/// Returns `None` when a pattern brush has no source shape — a custom pattern
/// the user has not made yet — so the caller can fall back to something
/// visible rather than painting nothing.
fn build_brush_path(
    samples: &[buzz_geom::StrokeSample],
    style: &DrawStyle,
    budget: &buzz_geom::BrushBudget,
) -> Option<BezPath> {
    let settings = &style.brush;

    match settings.kind {
        buzz_ui::BrushKind::Fluid => {
            Some(buzz_geom::fluid_outline(samples, &settings.profile(), budget).path)
        }
        buzz_ui::BrushKind::Pattern | buzz_ui::BrushKind::Art => {
            let source = settings.pattern_path()?;
            // The stroke is conditioned first, so stamps follow the smoothed
            // curve rather than the jitter of the raw pointer.
            let conditioned = buzz_geom::brush::condition(samples, settings.smoothing, budget);
            if conditioned.len() < 2 {
                // A tap with a pattern brush lays down a single stamp, which
                // is what a stamp tool should do.
                let at = conditioned.first()?.point;
                return Some(kurbo::Affine::translate(at.to_vec2()) * source);
            }
            let spine = buzz_geom::centreline(&conditioned);
            Some(buzz_geom::stamp_along(&spine, &source, settings.fit(), budget).path)
        }
    }
}

fn contains(rect: Rect, p: Point) -> bool {
    p.x >= rect.x0 && p.x <= rect.x1 && p.y >= rect.y0 && p.y <= rect.y1
}

/// Build the path for a drag-created shape.
///
/// Shift constrains: squares, circles, and lines to 45° steps — all Animate
/// behaviours a user will reach for without thinking.
fn build_shape_path(tool: ToolId, origin: Point, end: Point, mods: Mods) -> Option<BezPath> {
    let mut end = end;

    if mods.shift {
        match tool {
            ToolId::Rectangle | ToolId::Oval => {
                let dx = end.x - origin.x;
                let dy = end.y - origin.y;
                let size = dx.abs().max(dy.abs());
                end = Point::new(origin.x + size * dx.signum(), origin.y + size * dy.signum());
            }
            ToolId::Line | ToolId::Pen => {
                let d = end - origin;
                let angle = d.y.atan2(d.x);
                let step = std::f64::consts::FRAC_PI_4;
                let snapped = (angle / step).round() * step;
                let length = d.hypot();
                end = Point::new(
                    origin.x + length * snapped.cos(),
                    origin.y + length * snapped.sin(),
                );
            }
            _ => {}
        }
    }

    let rect = Rect::from_points(origin, end);
    if rect.width() <= 0.0 && rect.height() <= 0.0 {
        return None;
    }

    Some(match tool {
        ToolId::Rectangle => rect.to_path(1e-6),
        ToolId::Oval => {
            if mods.shift {
                Circle::new(rect.center(), rect.width().min(rect.height()) / 2.0).to_path(1e-3)
            } else {
                Ellipse::new(
                    rect.center(),
                    (rect.width() / 2.0, rect.height() / 2.0),
                    0.0,
                )
                .to_path(1e-3)
            }
        }
        ToolId::PolyStar => star_path(rect.center(), rect.width().min(rect.height()) / 2.0, 5),
        ToolId::Line | ToolId::Pen => Line::new(origin, end).to_path(1e-6),
        _ => return None,
    })
}

/// A five-pointed star, Animate's PolyStar default.
fn star_path(center: Point, radius: f64, points: usize) -> BezPath {
    let mut path = BezPath::new();
    let points = points.max(3);
    let inner = radius * 0.382; // The golden-ratio inner radius of a pentagram.
    let step = std::f64::consts::PI / points as f64;
    // Start at the top.
    let mut angle = -std::f64::consts::FRAC_PI_2;

    for i in 0..(points * 2) {
        let r = if i % 2 == 0 { radius } else { inner };
        let p = Point::new(center.x + r * angle.cos(), center.y + r * angle.sin());
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
        angle += step;
    }
    path.close_path();
    path
}

/// Scale the selection by dragging, anchored at the opposite corner.
fn scale_about_corner(bounds: Rect, origin: Point, end: Point, uniform: bool) -> Affine {
    // Whichever corner the drag started nearest stays put... its opposite does.
    let anchor = Point::new(
        if (origin.x - bounds.x0).abs() < (origin.x - bounds.x1).abs() {
            bounds.x1
        } else {
            bounds.x0
        },
        if (origin.y - bounds.y0).abs() < (origin.y - bounds.y1).abs() {
            bounds.y1
        } else {
            bounds.y0
        },
    );

    let before = origin - anchor;
    let after = end - anchor;

    let safe = |a: f64, b: f64| {
        if b.abs() < 1e-9 { 1.0 } else { a / b }
    };
    let mut sx = safe(after.x, before.x);
    let mut sy = safe(after.y, before.y);

    if uniform {
        let s = (sx.abs() + sy.abs()) / 2.0;
        sx = s * sx.signum();
        sy = s * sy.signum();
    }

    // Refuse to collapse geometry to nothing, which would be unrecoverable.
    const MIN: f64 = 1e-4;
    if sx.abs() < MIN {
        sx = MIN * if sx < 0.0 { -1.0 } else { 1.0 };
    }
    if sy.abs() < MIN {
        sy = MIN * if sy < 0.0 { -1.0 } else { 1.0 };
    }

    Affine::translate(anchor.to_vec2())
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate(-anchor.to_vec2())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(style: &DrawStyle) -> ToolContext<'_> {
        ToolContext {
            style,
            zoom: 1.0,
            selection_bounds: None,
            anchors: &[],
        }
    }

    fn drag(
        machine: &mut ToolMachine,
        from: Point,
        to: Point,
        ctx: &ToolContext<'_>,
    ) -> ToolAction {
        machine.pointer_down(from, from, Mods::default(), ctx);
        machine.pointer_move(to, to, Mods::default());
        machine.pointer_up(to, to, ctx)
    }

    #[test]
    fn dragging_the_rectangle_tool_creates_a_rectangle() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Rectangle);
        let action = drag(
            &mut m,
            Point::new(10.0, 10.0),
            Point::new(60.0, 40.0),
            &ctx(&style),
        );

        match action {
            ToolAction::AddShape { shape, label } => {
                assert_eq!(label, "Draw Rectangle");
                let bb = shape.path.bounding_box();
                assert!((bb.width() - 50.0).abs() < 1e-6, "{bb:?}");
                assert!((bb.height() - 30.0).abs() < 1e-6);
                assert!(shape.fill.is_some() && shape.stroke.is_some());
            }
            other => panic!("expected a shape, got {other:?}"),
        }
    }

    #[test]
    fn shift_constrains_a_rectangle_to_a_square() {
        let path = build_shape_path(
            ToolId::Rectangle,
            Point::new(0.0, 0.0),
            Point::new(100.0, 30.0),
            Mods {
                shift: true,
                ..Default::default()
            },
        )
        .unwrap();
        let bb = path.bounding_box();
        assert!(
            (bb.width() - bb.height()).abs() < 1e-6,
            "shift should give a square, got {bb:?}"
        );
    }

    #[test]
    fn shift_constrains_a_line_to_45_degree_steps() {
        let path = build_shape_path(
            ToolId::Line,
            Point::new(0.0, 0.0),
            // Nearly horizontal, so it should snap flat.
            Point::new(100.0, 12.0),
            Mods {
                shift: true,
                ..Default::default()
            },
        )
        .unwrap();
        let bb = path.bounding_box();
        assert!(bb.height() < 1e-6, "expected a horizontal line, got {bb:?}");
    }

    #[test]
    fn a_line_has_a_stroke_but_no_fill() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Line);
        match drag(
            &mut m,
            Point::new(0.0, 0.0),
            Point::new(50.0, 50.0),
            &ctx(&style),
        ) {
            ToolAction::AddShape { shape, .. } => {
                assert!(shape.stroke.is_some());
                assert!(shape.fill.is_none(), "a line must not be filled");
            }
            other => panic!("got {other:?}"),
        }
    }

    /// A click is not a drag; drawing tools must not leave zero-size shapes
    /// behind every time the user clicks the stage.
    #[test]
    fn a_click_does_not_create_a_shape() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Rectangle);
        let p = Point::new(20.0, 20.0);
        assert_eq!(drag(&mut m, p, p, &ctx(&style)), ToolAction::None);
    }

    /// The click threshold is in screen pixels, so it must shrink as you zoom.
    #[test]
    fn the_click_threshold_scales_with_zoom() {
        let style = DrawStyle::default();
        let from = Point::new(0.0, 0.0);
        let to = Point::new(2.0, 0.0);

        // At 1x, 2 document units is under the 3 px slop: a click.
        let mut m = ToolMachine::new(ToolId::Rectangle);
        let zoomed_out = ToolContext {
            style: &style,
            zoom: 1.0,
            selection_bounds: None,
            anchors: &[],
        };
        assert_eq!(drag(&mut m, from, to, &zoomed_out), ToolAction::None);

        // At 10x, the same 2 units is 20 px: a real drag.
        let mut m = ToolMachine::new(ToolId::Rectangle);
        let zoomed_in = ToolContext {
            style: &style,
            zoom: 10.0,
            selection_bounds: None,
            anchors: &[],
        };
        assert!(matches!(
            drag(&mut m, from, to, &zoomed_in),
            ToolAction::AddShape { .. }
        ));
    }

    #[test]
    fn clicking_with_the_selection_tool_picks_what_is_under_the_cursor() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Selection);
        let p = Point::new(30.0, 30.0);
        match drag(&mut m, p, p, &ctx(&style)) {
            ToolAction::PickAt { point, additive } => {
                assert_eq!(point, p);
                assert!(!additive);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn shift_click_extends_the_selection() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Selection);
        let p = Point::new(5.0, 5.0);
        let mods = Mods {
            shift: true,
            ..Default::default()
        };
        m.pointer_down(p, p, mods, &ctx(&style));
        match m.pointer_up(p, p, &ctx(&style)) {
            ToolAction::PickAt { additive, .. } => assert!(additive),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn dragging_empty_space_marquee_selects() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Selection);
        match drag(
            &mut m,
            Point::new(0.0, 0.0),
            Point::new(100.0, 80.0),
            &ctx(&style),
        ) {
            ToolAction::PickInRect { rect, .. } => {
                assert!((rect.width() - 100.0).abs() < 1e-9);
            }
            other => panic!("got {other:?}"),
        }
    }

    /// Dragging from *inside* the selection moves it instead of starting a new
    /// marquee — the behaviour that makes the Selection tool feel right.
    #[test]
    fn dragging_from_inside_the_selection_moves_it() {
        let style = DrawStyle::default();
        let c = ToolContext {
            style: &style,
            zoom: 1.0,
            selection_bounds: Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
            anchors: &[],
        };
        let mut m = ToolMachine::new(ToolId::Selection);
        match drag(&mut m, Point::new(50.0, 50.0), Point::new(80.0, 90.0), &c) {
            ToolAction::MoveSelection { delta } => {
                assert!((delta.x - 30.0).abs() < 1e-9 && (delta.y - 40.0).abs() < 1e-9);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn freehand_tools_accumulate_a_path() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Pencil);
        m.pointer_down(
            Point::new(0.0, 0.0),
            Point::ORIGIN,
            Mods::default(),
            &ctx(&style),
        );
        for i in 1..20 {
            let p = Point::new(i as f64, (i as f64).sin() * 5.0);
            m.pointer_move(p, p, Mods::default());
        }
        match m.pointer_up(Point::new(20.0, 0.0), Point::ORIGIN, &ctx(&style)) {
            ToolAction::AddShape { shape, label } => {
                assert_eq!(label, "Pencil");
                assert!(shape.path.elements().len() > 10);
                assert!(shape.stroke.is_some() && shape.fill.is_none());
            }
            other => panic!("got {other:?}"),
        }
    }

    /// The brush paints a filled outline, not a stroke — Animate's behaviour,
    /// and why it uses the fill colour.
    #[test]
    fn the_brush_produces_a_filled_outline() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Brush);
        m.pointer_down(
            Point::new(0.0, 0.0),
            Point::ORIGIN,
            Mods::default(),
            &ctx(&style),
        );
        for i in 1..10 {
            let p = Point::new(i as f64 * 5.0, 0.0);
            m.pointer_move(p, p, Mods::default());
        }
        match m.pointer_up(Point::new(50.0, 0.0), Point::ORIGIN, &ctx(&style)) {
            ToolAction::AddShape { shape, label } => {
                assert_eq!(label, "Brush");
                assert!(shape.fill.is_some(), "the brush fills");
                assert!(shape.stroke.is_none(), "the brush does not stroke");
                assert!(
                    shape.path.area().abs() > 0.0,
                    "the outline should enclose area"
                );
            }
            other => panic!("got {other:?}"),
        }
    }

    /// Drive a whole stroke through the machine, with controlled timing.
    ///
    /// `seconds` is how long the stroke takes, which is what a speed-driven
    /// brush answers to. Real wall time would make these tests depend on how
    /// busy the machine is.
    fn draw_stroke(
        machine: &mut ToolMachine,
        style: &DrawStyle,
        points: &[Point],
        seconds: f64,
    ) -> ToolAction {
        let last = points.len().saturating_sub(1).max(1) as f64;
        machine.set_time(0.0);
        machine.pointer_down(points[0], points[0], Mods::default(), &ctx(style));
        for (i, p) in points.iter().enumerate().skip(1) {
            machine.set_time(i as f64 / last * seconds);
            machine.pointer_move(*p, *p, Mods::default());
        }
        machine.set_time(seconds);
        let end = *points.last().expect("a stroke has points");
        machine.pointer_up(end, end, &ctx(style))
    }

    fn wavy(count: usize, length: f64) -> Vec<Point> {
        (0..count)
            .map(|i| {
                let t = i as f64 / (count - 1).max(1) as f64;
                Point::new(t * length, (t * 12.0).sin() * 40.0)
            })
            .collect()
    }

    /// The fluid brush's whole reason for existing: the same path drawn faster
    /// paints a thinner stroke.
    #[test]
    fn a_fast_brush_stroke_is_thinner_than_a_slow_one() {
        let style = DrawStyle::default();
        let points: Vec<Point> = (0..60).map(|i| Point::new(i as f64 * 10.0, 0.0)).collect();

        let area_for = |seconds: f64| -> f64 {
            let mut m = ToolMachine::new(ToolId::Brush);
            match draw_stroke(&mut m, &style, &points, seconds) {
                ToolAction::AddShape { shape, .. } => shape.path.area().abs(),
                other => panic!("got {other:?}"),
            }
        };

        let slow = area_for(6.0);
        let fast = area_for(0.3);
        assert!(
            fast < slow * 0.8,
            "a fast stroke should lay down less ink: fast {fast:.1} vs slow {slow:.1}"
        );
        assert!(fast > 0.0);
    }

    /// A pattern brush must produce many separate stamps, not one blob.
    #[test]
    fn the_pattern_brush_stamps_its_shape_along_the_stroke() {
        let mut style = DrawStyle::default();
        style.brush.kind = buzz_ui::BrushKind::Pattern;
        style.brush.pattern = buzz_ui::PatternShape::Star;
        style.brush.spacing = 20.0;
        style.brush.size = 14.0;

        let mut m = ToolMachine::new(ToolId::Brush);
        match draw_stroke(&mut m, &style, &wavy(60, 400.0), 1.0) {
            ToolAction::AddShape { shape, label } => {
                assert_eq!(label, "Brush");
                assert!(shape.fill.is_some(), "stamps are filled artwork");

                let stamps = shape
                    .path
                    .elements()
                    .iter()
                    .filter(|e| matches!(e, kurbo::PathEl::MoveTo(_)))
                    .count();
                assert!(
                    stamps > 10,
                    "a 400-unit stroke at 20 apart should stamp many times, got {stamps}"
                );
            }
            other => panic!("got {other:?}"),
        }
    }

    /// The art brush lays down exactly one stretched copy.
    #[test]
    fn the_art_brush_lays_down_a_single_stretched_shape() {
        let mut style = DrawStyle::default();
        style.brush.kind = buzz_ui::BrushKind::Art;
        style.brush.pattern = buzz_ui::PatternShape::Leaf;

        let mut m = ToolMachine::new(ToolId::Brush);
        match draw_stroke(&mut m, &style, &wavy(40, 300.0), 1.0) {
            ToolAction::AddShape { shape, .. } => {
                let stamps = shape
                    .path
                    .elements()
                    .iter()
                    .filter(|e| matches!(e, kurbo::PathEl::MoveTo(_)))
                    .count();
                assert_eq!(stamps, 1, "an art brush places one copy");
                assert!(
                    shape.path.bounding_box().width() > 250.0,
                    "stretched to fit"
                );
            }
            other => panic!("got {other:?}"),
        }
    }

    /// A pattern brush with no shape chosen must not paint an empty object
    /// into the document.
    #[test]
    fn a_pattern_brush_with_no_shape_yet_paints_nothing() {
        let mut style = DrawStyle::default();
        style.brush.kind = buzz_ui::BrushKind::Pattern;
        style.brush.pattern = buzz_ui::PatternShape::Custom; // never made

        let mut m = ToolMachine::new(ToolId::Brush);
        assert_eq!(
            draw_stroke(&mut m, &style, &wavy(20, 100.0), 1.0),
            ToolAction::None
        );
    }

    /// Tapping the brush leaves a dot, as Animate does. The other freehand
    /// tools still need a real drag.
    #[test]
    fn tapping_the_brush_paints_a_dot() {
        let style = DrawStyle::default();
        let p = Point::new(4.0, 4.0);

        let mut brush = ToolMachine::new(ToolId::Brush);
        brush.pointer_down(p, p, Mods::default(), &ctx(&style));
        match brush.pointer_up(p, p, &ctx(&style)) {
            ToolAction::AddShape { shape, .. } => {
                assert!(shape.path.area().abs() > 0.0, "a tap should leave a mark");
            }
            other => panic!("got {other:?}"),
        }
    }

    /// The live preview is what runs on every pointer move, so it is where a
    /// hang would actually show up. It must stay far inside a frame even when
    /// the stroke is already enormous and the spacing is absurd.
    #[test]
    fn the_live_preview_stays_within_a_frame_budget_on_a_huge_stroke() {
        let mut style = DrawStyle::default();
        style.brush.kind = buzz_ui::BrushKind::Pattern;
        style.brush.pattern = buzz_ui::PatternShape::Star;
        style.brush.spacing = 0.5; // asks for thousands of stamps

        let mut m = ToolMachine::new(ToolId::Brush);
        let points = wavy(6_000, 20_000.0);

        m.set_time(0.0);
        m.pointer_down(points[0], points[0], Mods::default(), &ctx(&style));
        for (i, p) in points.iter().enumerate().skip(1) {
            m.set_time(i as f64 * 0.001);
            m.pointer_move(*p, *p, Mods::default());
        }

        // The worst case: the preview for the longest the stroke ever gets.
        let started = std::time::Instant::now();
        let preview = m.preview(&ctx(&style));
        let elapsed = started.elapsed();

        assert!(matches!(preview, Preview::Ink { .. }), "got {preview:?}");
        assert!(
            elapsed.as_millis() < 16,
            "one preview frame of a 6000-sample pattern stroke took {elapsed:?}; \
             at 60 fps that is a stutter the user would feel"
        );
    }

    /// And the committed geometry, which runs once on release, must also stay
    /// well short of anything a user would call a freeze.
    #[test]
    fn committing_a_huge_pattern_stroke_does_not_hang() {
        let mut style = DrawStyle::default();
        style.brush.kind = buzz_ui::BrushKind::Pattern;
        style.brush.pattern = buzz_ui::PatternShape::Leaf;
        style.brush.spacing = 0.25;

        let mut m = ToolMachine::new(ToolId::Brush);
        let points = wavy(8_000, 40_000.0);

        let started = std::time::Instant::now();
        let action = draw_stroke(&mut m, &style, &points, 4.0);
        let elapsed = started.elapsed();

        match action {
            ToolAction::AddShape { shape, .. } => {
                assert!(!shape.path.elements().is_empty());
                // Bounded, so the document does not grow without limit either.
                assert!(
                    shape.path.elements().len() < 250_000,
                    "the committed path has {} elements",
                    shape.path.elements().len()
                );
            }
            other => panic!("got {other:?}"),
        }
        assert!(
            elapsed.as_millis() < 1_500,
            "committing a 40 000-unit pattern stroke took {elapsed:?}"
        );
    }

    /// Drawing a whole picture's worth of pattern strokes, one after another,
    /// with a preview on every move — the realistic worst case.
    #[test]
    fn drawing_many_pattern_strokes_in_a_row_stays_responsive() {
        let mut style = DrawStyle::default();
        style.brush.kind = buzz_ui::BrushKind::Pattern;
        style.brush.pattern = buzz_ui::PatternShape::Dot;
        style.brush.spacing = 6.0;

        let started = std::time::Instant::now();
        let mut painted = 0usize;

        for stroke in 0..120 {
            let mut m = ToolMachine::new(ToolId::Brush);
            let points: Vec<Point> = (0..80)
                .map(|i| {
                    let t = i as f64;
                    Point::new(t * 6.0, stroke as f64 * 4.0 + (t * 0.3).sin() * 25.0)
                })
                .collect();

            m.set_time(0.0);
            m.pointer_down(points[0], points[0], Mods::default(), &ctx(&style));
            for (i, p) in points.iter().enumerate().skip(1) {
                m.set_time(i as f64 * 0.01);
                m.pointer_move(*p, *p, Mods::default());
                // A preview every move, exactly as the window does.
                let _ = m.preview(&ctx(&style));
            }
            let end = *points.last().unwrap();
            m.set_time(1.0);
            if let ToolAction::AddShape { .. } = m.pointer_up(end, end, &ctx(&style)) {
                painted += 1;
            }
        }

        let elapsed = started.elapsed();
        assert_eq!(painted, 120, "every stroke should have painted something");
        assert!(
            elapsed.as_secs_f64() < 8.0,
            "120 pattern strokes with live previews took {elapsed:?}"
        );
    }

    #[test]
    fn a_single_point_freehand_gesture_produces_nothing() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Pencil);
        let p = Point::new(1.0, 1.0);
        m.pointer_down(p, p, Mods::default(), &ctx(&style));
        assert_eq!(m.pointer_up(p, p, &ctx(&style)), ToolAction::None);
    }

    #[test]
    fn the_hand_tool_pans_while_dragging() {
        let mut m = ToolMachine::new(ToolId::Hand);
        m.pointer_down(
            Point::ORIGIN,
            Point::new(100.0, 100.0),
            Mods::default(),
            &ctx(&DrawStyle::default()),
        );
        match m.pointer_move(Point::ORIGIN, Point::new(120.0, 90.0), Mods::default()) {
            ToolAction::PanView { delta_screen } => {
                assert!((delta_screen.x - 20.0).abs() < 1e-9);
                assert!((delta_screen.y + 10.0).abs() < 1e-9);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn the_zoom_tool_zooms_in_and_alt_zooms_out() {
        let style = DrawStyle::default();
        let p = Point::new(10.0, 10.0);

        let mut m = ToolMachine::new(ToolId::Zoom);
        m.pointer_down(p, p, Mods::default(), &ctx(&style));
        match m.pointer_up(p, p, &ctx(&style)) {
            ToolAction::ZoomView { factor, .. } => assert!(factor > 1.0),
            other => panic!("got {other:?}"),
        }

        let mut m = ToolMachine::new(ToolId::Zoom);
        let alt = Mods {
            alt: true,
            ..Default::default()
        };
        m.pointer_down(p, p, alt, &ctx(&style));
        match m.pointer_up(p, p, &ctx(&style)) {
            ToolAction::ZoomView { factor, .. } => assert!(factor < 1.0),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn previews_appear_while_dragging_and_vanish_after() {
        let style = DrawStyle::default();
        let c = ctx(&style);
        let mut m = ToolMachine::new(ToolId::Oval);
        assert_eq!(m.preview(&c), Preview::None);

        m.pointer_down(
            Point::new(0.0, 0.0),
            Point::ORIGIN,
            Mods::default(),
            &ctx(&style),
        );
        m.pointer_move(Point::new(40.0, 30.0), Point::ORIGIN, Mods::default());
        assert!(matches!(m.preview(&c), Preview::Shape(_)));

        m.pointer_up(Point::new(40.0, 30.0), Point::ORIGIN, &c);
        assert_eq!(m.preview(&c), Preview::None);
    }

    #[test]
    fn escape_cancels_a_gesture_without_creating_anything() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::Rectangle);
        m.pointer_down(
            Point::new(0.0, 0.0),
            Point::ORIGIN,
            Mods::default(),
            &ctx(&style),
        );
        m.pointer_move(Point::new(50.0, 50.0), Point::ORIGIN, Mods::default());
        assert!(m.is_active());

        m.cancel();
        assert!(!m.is_active());
        let style = DrawStyle::default();
        assert_eq!(
            m.pointer_up(Point::new(50.0, 50.0), Point::ORIGIN, &ctx(&style)),
            ToolAction::None
        );
    }

    #[test]
    fn changing_tool_abandons_the_gesture() {
        let mut m = ToolMachine::new(ToolId::Rectangle);
        m.pointer_down(
            Point::ORIGIN,
            Point::ORIGIN,
            Mods::default(),
            &ctx(&DrawStyle::default()),
        );
        m.set_tool(ToolId::Oval);
        assert!(!m.is_active());
        assert_eq!(m.tool(), ToolId::Oval);
    }

    #[test]
    fn a_star_is_closed_and_has_the_right_number_of_points() {
        let star = star_path(Point::new(0.0, 0.0), 50.0, 5);
        let lines = star
            .elements()
            .iter()
            .filter(|e| matches!(e, kurbo::PathEl::LineTo(_)))
            .count();
        assert_eq!(lines, 9, "five points means ten vertices");
        assert!(
            star.elements()
                .iter()
                .any(|e| matches!(e, kurbo::PathEl::ClosePath))
        );
        assert!(star.area().abs() > 0.0);
    }

    #[test]
    fn scaling_anchors_the_opposite_corner() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Drag the bottom-right corner outwards; the top-left must stay put.
        let t = scale_about_corner(
            bounds,
            Point::new(100.0, 100.0),
            Point::new(200.0, 200.0),
            false,
        );

        let top_left = t * Point::new(0.0, 0.0);
        assert!(
            top_left.to_vec2().hypot() < 1e-9,
            "anchor moved to {top_left:?}"
        );

        let bottom_right = t * Point::new(100.0, 100.0);
        assert!(
            (bottom_right.x - 200.0).abs() < 1e-6,
            "got {bottom_right:?}"
        );
    }

    #[test]
    fn uniform_scaling_keeps_the_aspect_ratio() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 50.0);
        let t = scale_about_corner(
            bounds,
            Point::new(100.0, 50.0),
            Point::new(300.0, 60.0),
            true,
        );
        let c = t.as_coeffs();
        assert!(
            (c[0] - c[3]).abs() < 1e-9,
            "uniform scaling should match both axes: {} vs {}",
            c[0],
            c[3]
        );
    }

    /// Collapsing a shape to zero size would be unrecoverable.
    #[test]
    fn scaling_refuses_to_collapse_geometry() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        let t = scale_about_corner(
            bounds,
            Point::new(100.0, 100.0),
            Point::new(0.0, 0.0),
            false,
        );
        let c = t.as_coeffs();
        assert!(c[0].abs() > 0.0 && c[3].abs() > 0.0);
        assert!(c[0].is_finite() && c[3].is_finite());
    }

    /// Subselection grabs a nearby anchor rather than starting a marquee.
    #[test]
    fn subselection_grabs_an_anchor_within_reach() {
        let style = DrawStyle::default();
        let anchors = [
            buzz_geom::Anchor {
                element: 1,
                point: Point::new(100.0, 0.0),
            },
            buzz_geom::Anchor {
                element: 2,
                point: Point::new(50.0, 80.0),
            },
        ];
        let ctx = ToolContext {
            style: &style,
            zoom: 1.0,
            selection_bounds: Some(Rect::new(0.0, 0.0, 100.0, 80.0)),
            anchors: &anchors,
        };

        let mut m = ToolMachine::new(ToolId::Subselection);
        let start = Point::new(102.0, 2.0);
        let end = Point::new(140.0, 20.0);
        m.pointer_down(start, start, Mods::default(), &ctx);
        m.pointer_move(end, end, Mods::default());

        match m.pointer_up(end, end, &ctx) {
            ToolAction::MoveAnchor { element, delta } => {
                assert_eq!(element, 1, "should have grabbed the nearest anchor");
                assert!((delta.x - 38.0).abs() < 1e-9, "delta was {delta:?}");
            }
            other => panic!("expected an anchor drag, got {other:?}"),
        }
    }

    #[test]
    fn subselection_falls_back_to_selection_away_from_anchors() {
        let style = DrawStyle::default();
        let anchors = [buzz_geom::Anchor {
            element: 1,
            point: Point::new(100.0, 0.0),
        }];
        let ctx = ToolContext {
            style: &style,
            zoom: 1.0,
            selection_bounds: None,
            anchors: &anchors,
        };

        let mut m = ToolMachine::new(ToolId::Subselection);
        let start = Point::new(300.0, 300.0);
        let end = Point::new(400.0, 400.0);
        m.pointer_down(start, start, Mods::default(), &ctx);
        m.pointer_move(end, end, Mods::default());

        assert!(
            matches!(m.pointer_up(end, end, &ctx), ToolAction::PickInRect { .. }),
            "far from any anchor it should marquee-select"
        );
    }

    /// The grab radius is in screen pixels, so it must tighten as you zoom in.
    #[test]
    fn the_anchor_grab_radius_scales_with_zoom() {
        let style = DrawStyle::default();
        let anchors = [buzz_geom::Anchor {
            element: 1,
            point: Point::new(100.0, 0.0),
        }];
        let start = Point::new(105.0, 0.0);
        let end = Point::new(120.0, 0.0);

        // At 1x, 5 document units is within the 7 px grab radius.
        let near = ToolContext {
            style: &style,
            zoom: 1.0,
            selection_bounds: None,
            anchors: &anchors,
        };
        let mut m = ToolMachine::new(ToolId::Subselection);
        m.pointer_down(start, start, Mods::default(), &near);
        assert!(matches!(
            m.pointer_up(end, end, &near),
            ToolAction::MoveAnchor { .. }
        ));

        // At 10x it is 50 px away, far outside the radius.
        let far = ToolContext {
            style: &style,
            zoom: 10.0,
            selection_bounds: None,
            anchors: &anchors,
        };
        let mut m = ToolMachine::new(ToolId::Subselection);
        m.pointer_down(start, start, Mods::default(), &far);
        assert!(!matches!(
            m.pointer_up(end, end, &far),
            ToolAction::MoveAnchor { .. }
        ));
    }

    #[test]
    fn unimplemented_tools_do_nothing_rather_than_misbehave() {
        let style = DrawStyle::default();
        for tool in [
            ToolId::Bone,
            ToolId::Camera,
            ToolId::Text,
            ToolId::GradientTransform,
        ] {
            let mut m = ToolMachine::new(tool);
            let action = drag(
                &mut m,
                Point::new(0.0, 0.0),
                Point::new(50.0, 50.0),
                &ctx(&style),
            );
            assert_eq!(action, ToolAction::None, "{tool:?} should be inert");
        }
    }
}
