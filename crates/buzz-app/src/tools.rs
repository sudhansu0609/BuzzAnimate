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
    /// Select everything inside a freehand region — and, where that region cuts
    /// across artwork, cut the artwork along it and select the part inside.
    ///
    /// The cutting is what makes this the Lasso rather than a bendy marquee,
    /// and it is what Animate's Lasso does to a shape.
    PickInRegion { region: BezPath, additive: bool },
    /// Take everything the colour of what is under the point — the Magic Wand.
    ///
    /// The region cannot be worked out here, because it depends on the pixels
    /// of whatever was hit, and this module cannot see the scene. So the
    /// editor is handed the click and does the flood fill itself.
    WandAt { point: Point, additive: bool },
    /// Move the current selection.
    MoveSelection { delta: Vec2 },
    /// Scale the selection about a fixed corner.
    TransformSelection { transform: Affine },
    /// Drag one anchor of the selected path — Animate's Subselection tool.
    MoveAnchor { element: usize, delta: Vec2 },
    /// Place a painted stroke: a bitmap, and the rectangle it fills.
    ///
    /// The tool cannot make this artwork itself, because a bitmap needs an id
    /// from the document's library and this module cannot see the document.
    PaintRaster {
        canvas: buzz_scene::Canvas,
        brush: buzz_scene::SoftBrush,
    },
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
    ///
    /// In **screen pixels**, like [`Self::PanView`], and for two reasons that
    /// both showed up as a shot that shook while it was being aimed.
    ///
    /// The pointer's document position is *snapped* — pulled to nearby artwork
    /// edges — which is right for drawing and nonsense for a camera: the shot
    /// jumped from edge to edge as the pointer crossed the stage. And the
    /// document position is measured *through the camera*, so moving the camera
    /// moves the frame the measurement is made in; each step was measured
    /// against a ruler the previous step had already shifted. Screen pixels are
    /// what the hand actually did, and neither problem can reach them.
    MoveCamera { delta_screen: Vec2 },
    /// Zoom about a screen point.
    ZoomView { factor: f64, at_screen: Point },
    /// Clear the selection.
    Deselect,
    /// Put the transformation point here — Animate's white circle, dragged.
    SetTransformPoint { at: Point },
    /// Put it back at the centre of the selection.
    ResetTransformPoint,
    /// Drag one grip of the selected shape's gradient — Animate's Gradient
    /// Transform tool.
    DragGradient { grip: GradientGrip, to: Point },
}

/// Which handle of a gradient is being dragged.
///
/// # Why these four, and why the end grip does two things
///
/// Animate draws four grips on a gradient and gives *scale* and *rotate* one
/// each, on the same line. Here the end of the ramp does both at once: dragging
/// it puts the ramp's end where the pointer is. It is one grip instead of two
/// adjacent ones a few pixels apart, and there is never a question of which was
/// grabbed. Recorded as a deviation in PROGRESS.md §7.
///
/// The grips are the matrix's own parts, which is what makes this exact rather
/// than a decomposition: the centre is its translation, the end is its first
/// column and the width is its second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientGrip {
    /// Move the whole gradient.
    Center,
    /// The end of the ramp: its direction and its length together.
    End,
    /// How thick the ramp is across its own axis. For a radial gradient this
    /// is what makes it an ellipse.
    Width,
    /// Radial only: the hot spot, sliding along the ramp's axis.
    Focus,
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
    /// Artwork as it will be painted, in its real paint.
    ///
    /// For the soft brush, whose result is a bitmap rather than an outline:
    /// what it lays down cannot be described by a silhouette, so the preview
    /// is the bitmap itself, filling the rectangle it will occupy.
    Painted {
        area: Rect,
        paint: buzz_scene::Paint,
    },
    /// **The selection as the transform in progress would leave it.**
    ///
    /// A rotate, scale or skew used to be applied on release, so the artwork
    /// sat still while the handles moved and you found out what you had done
    /// only afterwards. The maths was never the missing part: this carries the
    /// *same* affine the release will commit, built by the same functions, and
    /// the stage draws the selected outlines through it.
    ///
    /// Nothing is edited until the pointer comes up, so a drag is still one
    /// undo step rather than one per pixel.
    Transform(Affine),
    /// The transformation point being dragged, where the pointer has it.
    ///
    /// The point only *moves* when the drag ends — it is one edit, not one per
    /// pixel — so without this the circle sat still under a moving pointer and
    /// the gesture looked as though it had not taken.
    Pivot(Point),
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
    /// The selection's transformation point, in document space — what a
    /// rotation or a skew turns about. `None` when nothing is selected.
    pub pivot: Option<Point>,
    /// The grips of the selected shape's gradient fill, in document space.
    ///
    /// `None` unless exactly one shape is selected and its fill is a gradient,
    /// which is precisely when the Gradient Transform tool has something to do.
    /// Supplied by the editor for the same reason the anchors are: only it can
    /// see the scene.
    pub gradient: Option<(buzz_scene::GradientHandles, buzz_scene::GradientKind)>,
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
        /// This drag is **moving the selection**, decided when it began.
        ///
        /// Recorded rather than re-derived on every move because the artwork
        /// travels *as the pointer does*, so the selection's bounds are no
        /// longer where they were when the press landed — asking "did this
        /// start inside the selection?" at release would be asking about a
        /// selection that has since moved out from under the question. What the
        /// drag began on decides what it does, which is the rule the rest of
        /// this file already follows.
        moving: bool,
    },
    /// Accumulating freehand samples.
    ///
    /// Samples rather than bare points because a brush needs *when* as well as
    /// where: the width of a fluid stroke follows how fast it was drawn.
    Freehand {
        samples: Vec<buzz_geom::StrokeSample>,
        /// What was held when the gesture began.
        ///
        /// Only the Lasso reads it — Shift adds to the selection there, as it
        /// does for a marquee — but it costs nothing to record for all of them
        /// and saves a second gesture variant that differs in one field.
        mods: Mods,
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
            ToolId::Pencil | ToolId::Brush | ToolId::Eraser | ToolId::Lasso => {
                self.gesture = Gesture::Freehand {
                    samples: vec![buzz_geom::StrokeSample::new(doc, self.clock.now())],
                    mods,
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
                        moving: self.begins_a_move(doc, ctx),
                    },
                };
            }
            _ => {
                self.gesture = Gesture::Dragging {
                    origin: doc,
                    current: doc,
                    mods,
                    moving: self.begins_a_move(doc, ctx),
                };
            }
        }
        ToolAction::None
    }

    /// Does a press at `at` begin a move of the selection?
    ///
    /// The same question `finish_drag` used to ask on release, asked once at
    /// the start where the answer is still true. Only the tools that can move
    /// artwork by dragging it answer yes.
    fn begins_a_move(&self, at: Point, ctx: &ToolContext<'_>) -> bool {
        if !matches!(
            self.tool,
            ToolId::Selection | ToolId::Subselection | ToolId::FreeTransform
        ) {
            return false;
        }
        let Some(bounds) = ctx.selection_bounds else {
            return false;
        };
        // Which point counts as "the transformation point" is per tool, and
        // matches what `finish_drag` does with it. Free Transform always draws
        // one, falling back to the centre. The selection tools can only grab a
        // point that actually exists — without one the centre of a selection is
        // ordinary artwork, and a press there moves it like anywhere else.
        let pivot = match (self.tool, ctx.pivot) {
            (ToolId::FreeTransform, p) => p.unwrap_or_else(|| bounds.center()),
            (_, Some(p)) => p,
            (_, None) => return contains(bounds, at),
        };
        let grab = TRANSFORM_GRAB_PX / ctx.zoom.max(f64::MIN_POSITIVE);
        if !matches!(
            transform_zone(bounds, pivot, at, grab),
            TransformZone::Inside
        ) {
            return false;
        }
        // `Inside` means "on none of the handles", which is also true of empty
        // stage far from the selection — and a drag out there is a marquee.
        // Free Transform has no marquee, so anywhere off its handles moves.
        self.tool == ToolId::FreeTransform || contains(bounds, at)
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
            Gesture::Freehand { samples, .. } => {
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
                current,
                mods: m,
                moving,
                ..
            } => {
                let previous = *current;
                let moving = *moving;
                *current = doc;
                *m = mods;
                // **The artwork travels with the pointer.**
                //
                // A move used to be committed only on release, so the drag
                // showed a marquee stretched across the artwork and then the
                // artwork teleported — the gesture gave no feedback at all
                // about the thing it was doing. Applied by the step since the
                // last move, exactly as the camera already is, and collapsed
                // into a single undo step by `end_gesture` on release.
                if moving {
                    let delta = doc - previous;
                    return if delta.hypot() > 0.0 {
                        ToolAction::MoveSelection { delta }
                    } else {
                        ToolAction::None
                    };
                }
                match self.tool {
                    ToolId::Hand => ToolAction::PanView { delta_screen },
                    // The camera moves live so the user can see the framing
                    // they are choosing, rather than only on release.
                    ToolId::Camera => ToolAction::MoveCamera { delta_screen },
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
            Gesture::Freehand { mut samples, mods } => {
                if samples.last().is_none_or(|s| s.point != doc) {
                    samples.push(buzz_geom::StrokeSample::new(doc, self.clock.now()));
                }
                self.finish_freehand(samples, mods, ctx)
            }
            Gesture::Dragging {
                origin, mods, moving, ..
            } => self.finish_drag(origin, doc, mods, moving, ctx),
        }
    }

    /// The current preview, for drawing feedback on the stage.
    pub fn preview(&self, ctx: &ToolContext<'_>) -> Preview {
        match &self.gesture {
            Gesture::Idle => Preview::None,
            // The stage already draws anchors for the selected path; a
            // rubber-band line here would just add noise.
            Gesture::Anchor { .. } => Preview::None,
            Gesture::Freehand { samples, .. } => match self.tool {
                // The lasso previews the region it is enclosing, closed, so the
                // user can see what is about to be caught rather than only the
                // line they have drawn so far.
                ToolId::Lasso => match lasso_region(samples) {
                    Some(path) => Preview::Shape(path),
                    None => Preview::None,
                },
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
                // A soft brush previews its own pixels, because the pixels are
                // the point: an outline of where the paint would go says
                // nothing about how it fades.
                ToolId::Brush if ctx.style.brush.kind == buzz_ui::BrushKind::Raster => {
                    match paint_soft_stroke(samples, ctx.style) {
                        Some((canvas, brush)) => {
                            let area = canvas.area();
                            // A fresh identity every move, because every move
                            // really is new pixels — which `to_asset` gives
                            // without being asked, since a bitmap's identity is
                            // issued at construction rather than worked out.
                            let asset = std::sync::Arc::new(canvas.to_asset(
                                buzz_scene::ImageId(0),
                                "preview",
                                &brush,
                            ));
                            let mut fill = buzz_scene::ImageFill::new(asset, area);
                            fill.smooth = false;
                            Preview::Painted {
                                area,
                                paint: buzz_scene::Paint::Image(Box::new(fill)),
                            }
                        }
                        None => Preview::None,
                    }
                }
                ToolId::Brush => {
                    let budget = buzz_geom::BrushBudget::preview();
                    // The preview is drawn in one colour: it is redrawn on
                    // every pointer move, and the stroke it previews has no
                    // final bounds yet to lay a ramp across.
                    let color = ctx
                        .style
                        .fill_color_for_preview()
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
            // **A move previews nothing.** The artwork is already travelling
            // with the pointer (see `pointer_move`), and an outline over the
            // top of it would be a second, redundant answer to "where is this
            // going?" — drawn in the place the artwork already is.
            Gesture::Dragging { moving: true, .. } => Preview::None,
            Gesture::Dragging {
                origin,
                current,
                mods,
                ..
            } => match self.tool {
                ToolId::Rectangle
                | ToolId::Oval
                | ToolId::PolyStar
                | ToolId::Line
                | ToolId::Pen => build_shape_path(self.tool, *origin, *current, *mods)
                    .map(Preview::Shape)
                    .unwrap_or(Preview::None),
                ToolId::Zoom => Preview::Marquee(Rect::from_points(*origin, *current)),

                // What the drag *began* on decides what it previews, exactly
                // as it decides what the release commits — the pointer's shape
                // there is the promise, and the preview has to keep it.
                ToolId::FreeTransform | ToolId::Selection | ToolId::Subselection => {
                    let zone = match (ctx.pivot, ctx.selection_bounds) {
                        (Some(pivot), Some(bounds)) => {
                            let grab = TRANSFORM_GRAB_PX / ctx.zoom.max(f64::MIN_POSITIVE);
                            Some((pivot, bounds, transform_zone(bounds, pivot, *origin, grab)))
                        }
                        _ => None,
                    };
                    // A marquee everywhere the tools do not transform.
                    let marquee = || {
                        if self.tool == ToolId::FreeTransform {
                            Preview::None
                        } else {
                            Preview::Marquee(Rect::from_points(*origin, *current))
                        }
                    };
                    match zone {
                        Some((_, _, TransformZone::Pivot)) => Preview::Pivot(*current),

                        // **The live preview.** The affine is built by the same
                        // functions the release calls, so what is drawn is what
                        // will happen rather than a second approximation of it.
                        // Nothing is edited until the pointer comes up, so the
                        // whole drag is still one undo step.
                        Some((pivot, _, TransformZone::Rotate)) => Preview::Transform(
                            rotate_about(pivot, *origin, *current, mods.shift),
                        ),
                        Some((pivot, bounds, TransformZone::Corner))
                            if self.tool == ToolId::FreeTransform =>
                        {
                            Preview::Transform(if mods.alt {
                                scale_about(pivot, bounds, *origin, *current, mods.shift)
                            } else {
                                scale_about_corner(bounds, *origin, *current, mods.shift)
                            })
                        }
                        Some((pivot, bounds, TransformZone::Edge(horizontal)))
                            if self.tool == ToolId::FreeTransform =>
                        {
                            Preview::Transform(skew_about(
                                pivot, bounds, *origin, *current, horizontal,
                            ))
                        }

                        _ => marquee(),
                    }
                }
                _ => Preview::None,
            },
        }
    }

    fn finish_freehand(
        &self,
        samples: Vec<buzz_geom::StrokeSample>,
        mods: Mods,
        ctx: &ToolContext<'_>,
    ) -> ToolAction {
        // A brush tap paints a dot, so one sample is enough for it; every
        // other freehand tool needs a real drag.
        let is_brush = self.tool == ToolId::Brush;
        if samples.len() < 2 && !is_brush {
            return ToolAction::None;
        }

        match self.tool {
            ToolId::Lasso => match lasso_region(&samples) {
                Some(region) => ToolAction::PickInRegion {
                    region,
                    additive: mods.shift,
                },
                None => ToolAction::None,
            },
            ToolId::Eraser => ToolAction::Erase {
                path: centreline_of(&samples),
                width: ctx.style.stroke_width.max(1.0) * 4.0,
            },
            ToolId::Brush if ctx.style.brush.kind == buzz_ui::BrushKind::Raster => {
                match paint_soft_stroke(&samples, ctx.style) {
                    Some((canvas, brush)) => ToolAction::PaintRaster { canvas, brush },
                    None => ToolAction::None,
                }
            }
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
                let bounds = buzz_geom::Shape::bounding_box(&path);
                let paint = ctx
                    .style
                    .fill_for_new_shape(bounds)
                    .unwrap_or(buzz_scene::Paint::Solid(Color::BLACK));
                ToolAction::AddShape {
                    shape: ShapeData {
                        path,
                        fill: Some(buzz_scene::FillSpec {
                            paint,
                            rule: buzz_geom::FillMode::NonZero,
                        }),
                        stroke: None,
                        blend: ctx.style.brush.blend(),
                    },
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
                            paint: buzz_scene::Paint::Solid(color),
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
        moving: bool,
        ctx: &ToolContext<'_>,
    ) -> ToolAction {
        let slop = CLICK_SLOP_PX / ctx.zoom.max(f64::MIN_POSITIVE);
        let was_click = (end - origin).hypot() <= slop;

        // **A move is already done.** `pointer_move` applied it step by step,
        // so the release has nothing left to commit — re-applying the whole
        // delta here would move the artwork twice as far as the pointer went.
        // `end_gesture` still collapses the run into one undo step.
        //
        // Taken before the transformation-point branch below, which asks how
        // close the press was to the pivot: the pivot has travelled with the
        // selection, so on a long move it can end up under the point the drag
        // started from and claim a gesture that was never about it.
        if moving && !was_click {
            return ToolAction::None;
        }

        // **The transformation point can be dragged with the selection tools
        // too**, not only with Free Transform.
        //
        // A deviation from Animate, and a deliberate one: the point is what a
        // rotation, a skew and an Alt-scale all turn about, and having to
        // change tools to move it — then change back to carry on selecting —
        // is a step nobody thanks you for. Only a **drag** counts, so clicking
        // the middle of a shape still selects and moves it as before.
        if matches!(self.tool, ToolId::Selection | ToolId::Subselection)
            && !was_click
            && let Some(pivot) = ctx.pivot
            && ctx.selection_bounds.is_some()
            && (origin - pivot).hypot()
                <= (TRANSFORM_GRAB_PX / ctx.zoom.max(f64::MIN_POSITIVE)) * 1.5
        {
            return ToolAction::SetTransformPoint { at: end };
        }

        match self.tool {
            // Animate's Gradient Transform: grab a grip and the ramp follows.
            // With nothing gradient-filled selected it selects, so picking the
            // shape you meant to adjust does not need a trip back to the
            // Selection tool.
            ToolId::GradientTransform => {
                let grab = TRANSFORM_GRAB_PX / ctx.zoom.max(f64::MIN_POSITIVE);
                match ctx
                    .gradient
                    .and_then(|(h, kind)| grip_at(h, kind, origin, grab))
                {
                    Some(grip) => ToolAction::DragGradient { grip, to: end },
                    None => ToolAction::PickAt {
                        point: end,
                        additive: mods.shift,
                    },
                }
            }
            ToolId::Selection | ToolId::Subselection => {
                if was_click {
                    return ToolAction::PickAt {
                        point: end,
                        additive: mods.shift,
                    };
                }

                // **Rotating without changing tools.**
                //
                // `preview` has drawn this rotation for the whole drag — the
                // ring just outside a corner is the same one Free Transform
                // uses, and the pointer turns there — but the release used to
                // fall straight through to a marquee, so the artwork snapped
                // back and the selection was replaced by whatever the rubber
                // band had swept. A preview that promises something the release
                // does not do is worse than no preview, and this is the file
                // that says so.
                //
                // Committed by the same `rotate_about` the preview called, so
                // the two cannot drift apart.
                if let Some((pivot, bounds)) = ctx.pivot.zip(ctx.selection_bounds) {
                    let grab = TRANSFORM_GRAB_PX / ctx.zoom.max(f64::MIN_POSITIVE);
                    if matches!(
                        transform_zone(bounds, pivot, origin, grab),
                        TransformZone::Rotate
                    ) {
                        return ToolAction::TransformSelection {
                            transform: rotate_about(pivot, origin, end, mods.shift),
                        };
                    }
                }

                // A drag that began inside the selection has already returned
                // above; anything left is a marquee.
                ToolAction::PickInRect {
                    rect: Rect::from_points(origin, end),
                    additive: mods.shift,
                }
            }

            ToolId::FreeTransform => {
                let Some(bounds) = ctx.selection_bounds else {
                    return ToolAction::PickAt {
                        point: end,
                        additive: false,
                    };
                };
                let pivot = ctx.pivot.unwrap_or_else(|| bounds.center());
                let grab = TRANSFORM_GRAB_PX / ctx.zoom.max(f64::MIN_POSITIVE);

                // Which part of the gizmo the drag *started* on decides what
                // it does, exactly as in Animate: the pointer's shape there is
                // the promise, and changing the answer half way through a drag
                // would break it.
                match transform_zone(bounds, pivot, origin, grab) {
                    TransformZone::Pivot => {
                        if was_click {
                            // A click on the circle without moving it means
                            // "put it back", which is Animate's double-click.
                            ToolAction::ResetTransformPoint
                        } else {
                            ToolAction::SetTransformPoint { at: end }
                        }
                    }
                    // **A click on a handle is a mis-grab of the gizmo**, not a
                    // selection. The handles are furniture drawn over the
                    // artwork, and picking whatever happens to lie under a
                    // corner would take the selection away from the very thing
                    // the user was lining up to transform.
                    TransformZone::Corner | TransformZone::Rotate | TransformZone::Edge(_)
                        if was_click =>
                    {
                        ToolAction::None
                    }

                    // **A click anywhere else selects.**
                    //
                    // Every click used to answer `None` once anything was
                    // selected, so Free Transform locked onto the first object
                    // you picked: you could transform it forever and never
                    // reach another one without changing tools and back. A
                    // click is how you choose what to work on, and that has to
                    // keep working while a gizmo is on screen.
                    TransformZone::Inside if was_click => ToolAction::PickAt {
                        point: end,
                        additive: mods.shift,
                    },
                    TransformZone::Corner => ToolAction::TransformSelection {
                        transform: if mods.alt {
                            // Animate's Alt: scale about the transformation
                            // point rather than the opposite corner.
                            scale_about(pivot, bounds, origin, end, mods.shift)
                        } else {
                            scale_about_corner(bounds, origin, end, mods.shift)
                        },
                    },
                    TransformZone::Rotate => ToolAction::TransformSelection {
                        transform: rotate_about(pivot, origin, end, mods.shift),
                    },
                    TransformZone::Edge(horizontal) => ToolAction::TransformSelection {
                        transform: skew_about(pivot, bounds, origin, end, horizontal),
                    },
                    // Already applied move by move; see the early return above.
                    TransformZone::Inside => ToolAction::None,
                }
            }

            ToolId::Rectangle | ToolId::Oval | ToolId::PolyStar | ToolId::Line | ToolId::Pen => {
                if was_click {
                    return ToolAction::None;
                }
                let Some(path) = build_shape_path(self.tool, origin, end, mods) else {
                    return ToolAction::None;
                };
                let filled = self.tool != ToolId::Line && self.tool != ToolId::Pen;
                // The shape's own extent is what a gradient fill is laid
                // across, so it has to be measured before the shape is built.
                let bounds = buzz_geom::Shape::bounding_box(&path);
                ToolAction::AddShape {
                    shape: ShapeData {
                        path,
                        fill: filled
                            .then(|| ctx.style.fill_for_new_shape(bounds))
                            .flatten()
                            .map(|paint| buzz_scene::FillSpec {
                                paint,
                                rule: buzz_geom::FillMode::NonZero,
                            }),
                        blend: buzz_scene::PaintBlend::Normal,
                        stroke: ctx
                            .style
                            .stroke_for_new_shape()
                            .map(|(color, width, hairline)| buzz_scene::StrokeSpec {
                                paint: buzz_scene::Paint::Solid(color),
                                width,
                                hairline,
                            }),
                    },
                    label: shape_label(self.tool),
                }
            }

            // A wand click, wherever the pointer let go. There is nothing
            // useful a *drag* could mean for it, and treating a small
            // accidental drag as a miss would be a tool that ignores you.
            ToolId::MagicWand => ToolAction::WandAt {
                point: end,
                additive: mods.shift,
            },

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

/// The soft brush the current style describes.
fn soft_brush(style: &DrawStyle) -> buzz_scene::SoftBrush {
    buzz_scene::SoftBrush {
        // Size is a *width*, as it is for every other brush and as Animate
        // shows it; the raster brush works in radii.
        radius: (style.brush.size.max(0.5)) / 2.0,
        hardness: style.brush.hardness,
        flow: style.brush.flow,
        // The fill swatch, as the vector brush uses — a brush paints a filled
        // stroke, so its colour is the fill's.
        color: style.fill_color_for_preview(),
    }
}

/// Paint a soft-edged stroke, as artwork: a bitmap and the rectangle it fills.
///
/// `None` if the gesture painted nothing at all.
fn paint_soft_stroke(
    samples: &[buzz_geom::StrokeSample],
    style: &DrawStyle,
) -> Option<(buzz_scene::Canvas, buzz_scene::SoftBrush)> {
    let brush = soft_brush(style);
    let points: Vec<Point> = samples.iter().map(|s| s.point).collect();
    let canvas = buzz_scene::Canvas::for_stroke(&points, &brush)?;
    if canvas.is_blank() {
        return None;
    }
    Some((canvas, brush))
}

/// The closed region a lasso gesture has drawn.
///
/// The user is not asked to return to where they began: releasing the button
/// closes the loop with a straight line back to the start, which is what every
/// lasso in every editor does and what makes the tool usable at all.
///
/// `None` for a gesture too small to enclose anything — a stray click with the
/// Lasso selected should deselect, not cut a sliver out of the artwork under
/// the pointer.
fn lasso_region(samples: &[buzz_geom::StrokeSample]) -> Option<BezPath> {
    if samples.len() < 3 {
        return None;
    }
    let mut path = BezPath::new();
    path.move_to(samples[0].point);
    for s in &samples[1..] {
        path.line_to(s.point);
    }
    path.close_path();

    // Twice the enclosed area, by the shoelace formula. A gesture that went out
    // and came straight back encloses nothing, however long it was.
    let mut area = 0.0;
    for pair in samples.windows(2) {
        let (a, b) = (pair[0].point, pair[1].point);
        area += a.x * b.y - b.x * a.y;
    }
    let (first, last) = (samples[0].point, samples[samples.len() - 1].point);
    area += last.x * first.y - first.x * last.y;
    if (area / 2.0).abs() < 1.0 {
        return None;
    }
    Some(path)
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
        // Not geometry at all: a soft stroke is pixels, built by
        // `paint_soft_stroke`. Nothing here can describe it.
        buzz_ui::BrushKind::Raster => None,
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
/// How far from a handle a grab still counts, in screen pixels.
pub(crate) const TRANSFORM_GRAB_PX: f64 = 8.0;

/// Which part of the Free Transform gizmo a drag started on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformZone {
    /// The transformation point itself.
    Pivot,
    /// A corner handle: scale.
    Corner,
    /// Just outside a corner: rotate.
    Rotate,
    /// An edge: skew. `true` for a horizontal edge, which shears in x.
    Edge(bool),
    /// Anywhere else within the selection: move it.
    Inside,
}

/// Work out what a drag starting at `at` means.
///
/// The order matters and matches what is drawn: the circle wins over the box
/// it sits inside, a corner wins over the ring around it, and the ring wins
/// over the edges it overlaps at the ends — otherwise the corner of a small
/// selection would be three things at once.
/// Which gradient grip is within `grab` of `at`, if any.
///
/// **The centre is tested last**, deliberately. All four grips sit on the ramp
/// and the focus starts *on top of* the centre when the focal point is zero —
/// which is its default, so it is the usual case. Testing the centre first
/// would make the focus unreachable on every gradient that had not already been
/// adjusted. Nearest-wins would flicker between them when they coincide; an
/// explicit order does not.
pub fn grip_at(
    handles: buzz_scene::GradientHandles,
    kind: buzz_scene::GradientKind,
    at: Point,
    grab: f64,
) -> Option<GradientGrip> {
    let near = |p: Point| (at - p).hypot() <= grab;

    if kind == buzz_scene::GradientKind::Radial && near(handles.focus) {
        return Some(GradientGrip::Focus);
    }
    if near(handles.end) {
        return Some(GradientGrip::End);
    }
    if near(handles.width) {
        return Some(GradientGrip::Width);
    }
    if near(handles.center) {
        return Some(GradientGrip::Center);
    }
    None
}

fn transform_zone(bounds: Rect, pivot: Point, at: Point, grab: f64) -> TransformZone {
    // A little more forgiving than a handle: the circle is small, it is often
    // parked over artwork you are looking at rather than over a corner, and
    // missing it silently *moves the artwork* instead — which is the one
    // outcome worth spending a few pixels to avoid.
    if (at - pivot).hypot() <= grab * 1.5 {
        return TransformZone::Pivot;
    }

    let corners = [
        Point::new(bounds.x0, bounds.y0),
        Point::new(bounds.x1, bounds.y0),
        Point::new(bounds.x1, bounds.y1),
        Point::new(bounds.x0, bounds.y1),
    ];
    let nearest = corners
        .iter()
        .map(|c| (*c - at).hypot())
        .fold(f64::INFINITY, f64::min);
    if nearest <= grab {
        return TransformZone::Corner;
    }
    // Animate's rotate ring: just *outside* a corner, where the pointer turns
    // into the rotation cursor.
    if nearest <= grab * 3.0 && !contains(bounds, at) {
        return TransformZone::Rotate;
    }

    // An edge, but not near a corner: skew along it.
    let near_vertical_edge = ((at.x - bounds.x0).abs() <= grab || (at.x - bounds.x1).abs() <= grab)
        && at.y > bounds.y0 + grab
        && at.y < bounds.y1 - grab;
    let near_horizontal_edge = ((at.y - bounds.y0).abs() <= grab
        || (at.y - bounds.y1).abs() <= grab)
        && at.x > bounds.x0 + grab
        && at.x < bounds.x1 - grab;
    if near_horizontal_edge {
        return TransformZone::Edge(true);
    }
    if near_vertical_edge {
        return TransformZone::Edge(false);
    }

    TransformZone::Inside
}

/// Rotation about a point, by the angle the drag swept.
///
/// With Shift the angle snaps to 45°, as Animate does.
fn rotate_about(pivot: Point, origin: Point, end: Point, snap: bool) -> Affine {
    let from = origin - pivot;
    let to = end - pivot;
    // A drag that began on the pivot has no direction to measure from.
    if from.hypot() < 1e-9 || to.hypot() < 1e-9 {
        return Affine::IDENTITY;
    }
    let mut angle = to.y.atan2(to.x) - from.y.atan2(from.x);
    if snap {
        let step = std::f64::consts::FRAC_PI_4;
        angle = (angle / step).round() * step;
    }
    Affine::translate(pivot.to_vec2()) * Affine::rotate(angle) * Affine::translate(-pivot.to_vec2())
}

/// Scale about an arbitrary point rather than the opposite corner.
fn scale_about(pivot: Point, bounds: Rect, origin: Point, end: Point, uniform: bool) -> Affine {
    let before = origin - pivot;
    let after = end - pivot;
    let safe = |a: f64, b: f64, extent: f64| {
        // Dragging a handle that is *on* the pivot's own row or column has no
        // ratio to take; fall back on the selection's size so the drag still
        // does something predictable rather than nothing.
        if b.abs() > 1e-9 {
            a / b
        } else if extent.abs() > 1e-9 {
            1.0 + a / extent
        } else {
            1.0
        }
    };
    let mut sx = safe(after.x, before.x, bounds.width());
    let mut sy = safe(after.y, before.y, bounds.height());
    if uniform {
        let s = (sx.abs() + sy.abs()) / 2.0;
        sx = s * sx.signum();
        sy = s * sy.signum();
    }
    const MIN: f64 = 1e-4;
    if sx.abs() < MIN {
        sx = MIN * if sx < 0.0 { -1.0 } else { 1.0 };
    }
    if sy.abs() < MIN {
        sy = MIN * if sy < 0.0 { -1.0 } else { 1.0 };
    }
    Affine::translate(pivot.to_vec2())
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate(-pivot.to_vec2())
}

/// Skew about a point: dragging a horizontal edge shears in x, a vertical one
/// in y, in proportion to the distance from the transformation point.
fn skew_about(pivot: Point, bounds: Rect, origin: Point, end: Point, horizontal: bool) -> Affine {
    let extent = if horizontal {
        bounds.height()
    } else {
        bounds.width()
    };
    if extent.abs() < 1e-9 {
        return Affine::IDENTITY;
    }
    // The lever is how far the grabbed edge is from the pivot: an edge through
    // the transformation point cannot shear about it, which is the geometry
    // rather than a special case.
    let lever = if horizontal {
        origin.y - pivot.y
    } else {
        origin.x - pivot.x
    };
    if lever.abs() < 1e-9 {
        return Affine::IDENTITY;
    }
    let (shear_x, shear_y) = if horizontal {
        ((end.x - origin.x) / lever, 0.0)
    } else {
        (0.0, (end.y - origin.y) / lever)
    };

    const LIMIT: f64 = 20.0;
    let shear_x = shear_x.clamp(-LIMIT, LIMIT);
    let shear_y = shear_y.clamp(-LIMIT, LIMIT);

    Affine::translate(pivot.to_vec2())
        * Affine::new([1.0, shear_y, shear_x, 1.0, 0.0, 0.0])
        * Affine::translate(-pivot.to_vec2())
}

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
            pivot: None,
            gradient: None,
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

    // -- the Free Transform gizmo ------------------------------------------

    fn box_10() -> Rect {
        Rect::new(0.0, 0.0, 100.0, 100.0)
    }

    /// Which part of the gizmo a drag lands on. Getting this wrong means a
    /// rotation where the user asked for a scale, which is the sort of thing
    /// that loses work.
    #[test]
    fn the_gizmo_reads_the_zone_a_drag_starts_in() {
        let bounds = box_10();
        let pivot = Point::new(50.0, 50.0);
        let grab = 8.0;
        let zone = |x: f64, y: f64| transform_zone(bounds, pivot, Point::new(x, y), grab);

        assert_eq!(zone(50.0, 50.0), TransformZone::Pivot, "on the circle");
        assert_eq!(zone(0.0, 0.0), TransformZone::Corner, "on a corner");
        assert_eq!(
            zone(-12.0, -12.0),
            TransformZone::Rotate,
            "just outside a corner"
        );
        assert_eq!(zone(50.0, 0.0), TransformZone::Edge(true), "a top edge");
        assert_eq!(zone(0.0, 50.0), TransformZone::Edge(false), "a left edge");
        assert_eq!(zone(30.0, 30.0), TransformZone::Inside, "the middle");
    }

    /// The circle wins over everything under it: a transformation point parked
    /// on a corner must still be draggable.
    #[test]
    fn the_transformation_point_wins_over_the_handle_it_sits_on() {
        let bounds = box_10();
        let corner = Point::new(0.0, 0.0);
        assert_eq!(
            transform_zone(bounds, corner, corner, 8.0),
            TransformZone::Pivot
        );
    }

    /// A quarter turn about a point, and Shift snapping to 45°.
    #[test]
    fn rotation_turns_about_the_transformation_point() {
        let pivot = Point::new(0.0, 0.0);
        let turned = rotate_about(pivot, Point::new(10.0, 0.0), Point::new(0.0, 10.0), false);
        let moved = turned * Point::new(10.0, 0.0);
        assert!(
            (moved - Point::new(0.0, 10.0)).hypot() < 1e-9,
            "expected a quarter turn, got {moved:?}"
        );

        // 30° asked for, 45° snapped to.
        let snapped = rotate_about(
            pivot,
            Point::new(10.0, 0.0),
            Point::new(10.0_f64.to_radians().cos() * 10.0, 5.0),
            true,
        );
        let angle = {
            let p = snapped * Point::new(1.0, 0.0);
            p.y.atan2(p.x)
        };
        assert!(
            (angle - std::f64::consts::FRAC_PI_4).abs() < 1e-9 || angle.abs() < 1e-9,
            "expected a multiple of 45°, got {}",
            angle.to_degrees()
        );
    }

    /// Rotating about a point leaves that point exactly where it was — the one
    /// property the whole feature rests on.
    #[test]
    fn rotation_leaves_the_transformation_point_alone() {
        let pivot = Point::new(37.0, -11.0);
        let turned = rotate_about(pivot, Point::new(50.0, 0.0), Point::new(0.0, 50.0), false);
        assert!((turned * pivot - pivot).hypot() < 1e-9);
    }

    /// Skew shears along the edge that was grabbed, in proportion to the
    /// distance from the transformation point.
    #[test]
    fn skew_shears_about_the_transformation_point() {
        let bounds = box_10();
        let pivot = Point::new(50.0, 100.0);
        // Grab the top edge and pull it 50 to the right: the top leans over,
        // the bottom — which is on the pivot's own line — does not move.
        let sheared = skew_about(
            pivot,
            bounds,
            Point::new(50.0, 0.0),
            Point::new(100.0, 0.0),
            true,
        );

        let top = sheared * Point::new(50.0, 0.0);
        let bottom = sheared * Point::new(50.0, 100.0);
        assert!((top.x - 100.0).abs() < 1e-9, "the top should lean: {top:?}");
        assert!(
            (bottom - Point::new(50.0, 100.0)).hypot() < 1e-9,
            "the pivot's own line should not move: {bottom:?}"
        );
    }

    /// Scaling about the transformation point keeps it fixed, which is what
    /// Alt-dragging a handle is for.
    #[test]
    fn alt_scaling_keeps_the_transformation_point_fixed() {
        let bounds = box_10();
        let pivot = Point::new(50.0, 50.0);
        let scaled = scale_about(
            pivot,
            bounds,
            Point::new(100.0, 100.0),
            Point::new(150.0, 150.0),
            false,
        );
        assert!((scaled * pivot - pivot).hypot() < 1e-9);
        let corner = scaled * Point::new(100.0, 100.0);
        assert!((corner - Point::new(150.0, 150.0)).hypot() < 1e-9);
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
            pivot: None,
            gradient: None,
        };
        assert_eq!(drag(&mut m, from, to, &zoomed_out), ToolAction::None);

        // At 10x, the same 2 units is 20 px: a real drag.
        let mut m = ToolMachine::new(ToolId::Rectangle);
        let zoomed_in = ToolContext {
            style: &style,
            zoom: 10.0,
            selection_bounds: None,
            anchors: &[],
            pivot: None,
            gradient: None,
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
    ///
    /// The move arrives **while the pointer travels**, a step at a time, so the
    /// artwork is under the pointer the whole way rather than jumping to it on
    /// release. The release itself therefore has nothing left to commit.
    #[test]
    fn dragging_from_inside_the_selection_moves_it() {
        let style = DrawStyle::default();
        let c = ToolContext {
            style: &style,
            zoom: 1.0,
            selection_bounds: Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
            anchors: &[],
            pivot: None,
            gradient: None,
        };
        let mut m = ToolMachine::new(ToolId::Selection);
        m.pointer_down(
            Point::new(50.0, 50.0),
            Point::new(50.0, 50.0),
            Mods::default(),
            &c,
        );

        // Walked in two steps, so the deltas have to add up to the whole move
        // rather than each being measured from the origin.
        let mut total = buzz_geom::Vec2::new(0.0, 0.0);
        for at in [Point::new(60.0, 70.0), Point::new(80.0, 90.0)] {
            match m.pointer_move(at, at, Mods::default()) {
                ToolAction::MoveSelection { delta } => total += delta,
                other => panic!("mid-drag: got {other:?}"),
            }
        }
        assert!(
            (total.x - 30.0).abs() < 1e-9 && (total.y - 40.0).abs() < 1e-9,
            "the steps should sum to the drag, got {total:?}"
        );

        let end = Point::new(80.0, 90.0);
        assert!(
            matches!(m.pointer_up(end, end, &c), ToolAction::None),
            "the release must not move it a second time"
        );
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
            pivot: None,
            gradient: None,
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
            pivot: None,
            gradient: None,
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
            pivot: None,
            gradient: None,
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
            pivot: None,
            gradient: None,
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
        for tool in [ToolId::Bone, ToolId::Camera, ToolId::Text] {
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

    /// With nothing gradient-filled selected, the Gradient Transform tool
    /// selects — so clicking the shape you meant to adjust does not need a trip
    /// back to the Selection tool and back again.
    #[test]
    fn gradient_transform_selects_when_there_is_no_gradient_to_grab() {
        let style = DrawStyle::default();
        let mut m = ToolMachine::new(ToolId::GradientTransform);
        let action = drag(
            &mut m,
            Point::new(0.0, 0.0),
            Point::new(50.0, 50.0),
            &ctx(&style),
        );
        assert_eq!(
            action,
            ToolAction::PickAt {
                point: Point::new(50.0, 50.0),
                additive: false,
            }
        );
    }

    fn handles(
        centre: Point,
        end: Point,
        width: Point,
        focus: Point,
    ) -> buzz_scene::GradientHandles {
        buzz_scene::GradientHandles {
            center: centre,
            end,
            width,
            focus,
        }
    }

    /// Each grip is grabbed by starting the drag on it, and the drag reports
    /// where it ended.
    #[test]
    fn each_gradient_grip_can_be_grabbed() {
        let style = DrawStyle::default();
        let h = handles(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            Point::new(100.0, 200.0),
            Point::new(150.0, 100.0),
        );

        for (start, expected) in [
            (Point::new(100.0, 100.0), GradientGrip::Center),
            (Point::new(200.0, 100.0), GradientGrip::End),
            (Point::new(100.0, 200.0), GradientGrip::Width),
            (Point::new(150.0, 100.0), GradientGrip::Focus),
        ] {
            let mut c = ctx(&style);
            c.gradient = Some((h, buzz_scene::GradientKind::Radial));
            let mut m = ToolMachine::new(ToolId::GradientTransform);
            let action = drag(&mut m, start, Point::new(400.0, 400.0), &c);
            assert_eq!(
                action,
                ToolAction::DragGradient {
                    grip: expected,
                    to: Point::new(400.0, 400.0),
                },
                "starting at {start:?} should grab {expected:?}"
            );
        }
    }

    /// **The focus wins where it coincides with the centre**, which is where it
    /// sits on every gradient nobody has adjusted — its default is zero. Test
    /// the centre first and the focus can never be grabbed at all.
    #[test]
    fn the_focus_is_reachable_when_it_sits_on_the_centre() {
        let style = DrawStyle::default();
        let centre = Point::new(100.0, 100.0);
        let h = handles(
            centre,
            Point::new(200.0, 100.0),
            Point::new(100.0, 200.0),
            centre,
        );

        let mut c = ctx(&style);
        c.gradient = Some((h, buzz_scene::GradientKind::Radial));
        let mut m = ToolMachine::new(ToolId::GradientTransform);
        assert_eq!(
            drag(&mut m, centre, Point::new(160.0, 100.0), &c),
            ToolAction::DragGradient {
                grip: GradientGrip::Focus,
                to: Point::new(160.0, 100.0),
            }
        );

        // A *linear* gradient has no focus, so the same press grabs the centre.
        let mut c = ctx(&style);
        c.gradient = Some((h, buzz_scene::GradientKind::Linear));
        let mut m = ToolMachine::new(ToolId::GradientTransform);
        assert_eq!(
            drag(&mut m, centre, Point::new(160.0, 100.0), &c),
            ToolAction::DragGradient {
                grip: GradientGrip::Center,
                to: Point::new(160.0, 100.0),
            }
        );
    }
}

#[cfg(test)]
mod transform_preview_tests {
    use crate::tools::{Mods, Preview, ToolAction, ToolContext, ToolId, ToolMachine};
    use buzz_geom::{Point, Rect};
    use buzz_ui::DrawStyle;

    const BOX: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 100.0,
        y1: 80.0,
    };

    fn context(style: &DrawStyle) -> ToolContext<'_> {
        ToolContext {
            style,
            zoom: 1.0,
            selection_bounds: Some(BOX),
            anchors: &[],
            pivot: Some(BOX.center()),
            gradient: None,
        }
    }

    /// Drag from `from` to `to` and report what was previewed mid-drag and
    /// what was committed on release.
    fn drag(tool: ToolId, from: Point, to: Point, mods: Mods) -> (Preview, ToolAction) {
        let style = DrawStyle::default();
        let ctx = context(&style);
        let mut m = ToolMachine::new(tool);
        m.pointer_down(from, from, mods, &ctx);
        m.pointer_move(to, to, mods);
        let previewed = m.preview(&ctx);
        let committed = m.pointer_up(to, to, &ctx);
        (previewed, committed)
    }

    /// **What is drawn while dragging is what happens when you let go.**
    ///
    /// The preview and the commit are built by the same functions on purpose;
    /// this is what stops them drifting into two answers that disagree, which
    /// would be worse than having no preview at all.
    #[test]
    fn the_preview_matches_what_the_release_commits() {
        // A rotate handle sits outside a corner.
        let cases = [
            (ToolId::FreeTransform, Point::new(-8.0, -8.0), Point::new(60.0, -30.0)),
            (ToolId::FreeTransform, Point::new(100.0, 80.0), Point::new(160.0, 130.0)),
            (ToolId::FreeTransform, Point::new(50.0, 0.0), Point::new(90.0, 0.0)),
        ];

        for (tool, from, to) in cases {
            let (previewed, committed) = drag(tool, from, to, Mods::default());
            match (previewed, committed) {
                (Preview::Transform(shown), ToolAction::TransformSelection { transform }) => {
                    let a = shown.as_coeffs();
                    let b = transform.as_coeffs();
                    for (x, y) in a.iter().zip(b.iter()) {
                        assert!(
                            (x - y).abs() < 1e-9,
                            "dragging {from:?} to {to:?} previewed {a:?} but committed {b:?}"
                        );
                    }
                }
                // Not every grab point is a transform; those that are not must
                // preview no transform either, which is the other half of the
                // same promise.
                (other, ToolAction::TransformSelection { .. }) => {
                    panic!("committed a transform but previewed {other:?}")
                }
                (Preview::Transform(_), other) => {
                    panic!("previewed a transform but committed {other:?}")
                }
                _ => {}
            }
        }
    }

    /// Shift is honoured in the preview too — a constrained rotate that
    /// previewed unconstrained would be lying about where it will land.
    #[test]
    fn the_preview_honours_the_modifiers() {
        let from = Point::new(-8.0, -8.0);
        let to = Point::new(60.0, -30.0);
        let plain = drag(ToolId::FreeTransform, from, to, Mods::default());
        let shifted = drag(
            ToolId::FreeTransform,
            from,
            to,
            Mods {
                shift: true,
                ..Mods::default()
            },
        );

        match (&plain.0, &shifted.0) {
            (Preview::Transform(a), Preview::Transform(b)) => {
                assert!(
                    a.as_coeffs() != b.as_coeffs(),
                    "Shift changed nothing about the preview"
                );
            }
            other => panic!("expected two transform previews, got {other:?}"),
        }
        // And each still agrees with its own commit.
        for (previewed, committed) in [plain, shifted] {
            if let (Preview::Transform(shown), ToolAction::TransformSelection { transform }) =
                (previewed, committed)
            {
                for (x, y) in shown.as_coeffs().iter().zip(transform.as_coeffs().iter()) {
                    assert!((x - y).abs() < 1e-9);
                }
            }
        }
    }

    /// **Free Transform can still choose what to work on.**
    ///
    /// Every click answered `None` once anything was selected, so the tool
    /// locked onto the first object picked: you could transform it forever and
    /// never reach another without changing tools and back.
    #[test]
    fn free_transform_can_still_select_another_object() {
        let style = DrawStyle::default();
        let ctx = context(&style);
        let mut m = ToolMachine::new(ToolId::FreeTransform);

        // A click well inside the gizmo but off every handle: pick what is
        // under it, which is how another object is reached.
        let at = Point::new(30.0, 40.0);
        m.pointer_down(at, at, Mods::default(), &ctx);
        let committed = m.pointer_up(at, at, &ctx);
        assert!(
            matches!(committed, ToolAction::PickAt { .. }),
            "a click should select, got {committed:?}"
        );

        // A click on a corner handle is a mis-grab of the gizmo, not a change
        // of selection — it must not throw away what is being transformed.
        let corner = Point::new(0.0, 0.0);
        m.pointer_down(corner, corner, Mods::default(), &ctx);
        let committed = m.pointer_up(corner, corner, &ctx);
        assert!(
            matches!(committed, ToolAction::None),
            "a handle click must leave the selection alone, got {committed:?}"
        );
    }

    /// Dragging inside the selection moves it, and moving previews nothing —
    /// a move is shown by the artwork itself, not by an outline.
    ///
    /// Away from the centre on purpose: the transformation point sits there,
    /// and grabbing *it* is a third thing again.
    #[test]
    fn dragging_inside_still_moves_rather_than_transforming() {
        let style = DrawStyle::default();
        let ctx = context(&style);
        let mut m = ToolMachine::new(ToolId::FreeTransform);
        let (from, to) = (Point::new(25.0, 20.0), Point::new(45.0, 30.0));
        m.pointer_down(from, from, Mods::default(), &ctx);

        let moved = m.pointer_move(to, to, Mods::default());
        assert!(
            matches!(moved, ToolAction::MoveSelection { .. }),
            "the artwork should move as the pointer does, got {moved:?}"
        );
        assert!(
            matches!(m.preview(&ctx), Preview::None),
            "a move should not preview a transform"
        );
        assert!(
            matches!(m.pointer_up(to, to, &ctx), ToolAction::None),
            "the release must not move it a second time"
        );
    }
}
