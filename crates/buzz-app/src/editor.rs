//! Editor state and the operations the UI raises against it.
//!
//! Everything the application can do to a document funnels through here, so
//! undo labelling, layer locking and Animate's merge-shape rules live in one
//! place rather than being re-implemented — and eventually forgotten — in each
//! tool and menu handler.

use std::sync::Arc;

use buzz_doc::Document;
use buzz_geom::{Affine, BezPath, Camera, Point, Rect, Shape as _, Size, Vec2};
use buzz_scene::{
    FillSpec, LayerId, LayerKind, Object, ObjectId, ObjectKind, Scene, ShapeData, StrokeSpec, Tween,
};
use buzz_ui::{Command, DrawStyle, DrawingMode, LibraryState, Selection, ToolId, ViewSettings};
use peniko::Color;

use crate::tools::{Mods, Preview, ToolAction, ToolContext, ToolMachine};

/// How close a click must come to count as hitting a stroke, in screen pixels.
const PICK_TOLERANCE_PX: f64 = 4.0;

/// Upper bound on the playhead. Roughly 11 hours at 24 fps — far past anything
/// real, but finite so a stray value cannot produce an absurd timeline.
const MAX_FRAME: u32 = 999_999;

/// The whole editor.
pub struct Editor {
    pub doc: Document,
    pub camera: Camera,
    pub selection: Selection,
    pub style: DrawStyle,
    pub view: ViewSettings,
    pub machine: ToolMachine,
    /// The playhead. Everything the user sees and edits is at this frame.
    pub current_frame: u32,
    /// Playback state.
    pub playback: Playback,
    /// Onion skinning.
    pub onion: Onion,
    /// Library panel state: what is selected, what is open, what is typed in
    /// the search box. View state, so it lives here and not in the document.
    pub library: LibraryState,
    /// The fidelity report from the last import, while it is still on screen.
    ///
    /// View state, not document state: dismissing it is not an edit, and it
    /// must not be saved or undone.
    pub import_summary: Option<crate::import::ImportSummary>,
    /// Set when the user asks to quit.
    pub should_quit: bool,
    /// Transient message for the status bar.
    pub status: Option<String>,
}

/// Playback state.
///
/// Playback advances on *elapsed time*, not on frames rendered, so a document
/// plays at its authored rate whether the display runs at 60 Hz or 144 Hz, and
/// a slow frame drops rather than stretching the animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playback {
    pub playing: bool,
    pub looping: bool,
    /// Seconds accumulated towards the next frame.
    accumulator: f64,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            playing: false,
            // Animate loops by default in the timeline.
            looping: true,
            accumulator: 0.0,
        }
    }
}

/// Onion skinning: ghosts of neighbouring frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onion {
    pub enabled: bool,
    /// Draw ghosts as outlines rather than faded artwork.
    pub outlines: bool,
    /// Frames shown before the playhead.
    pub before: u32,
    /// Frames shown after it.
    pub after: u32,
}

impl Default for Onion {
    fn default() -> Self {
        Self {
            enabled: false,
            outlines: false,
            before: 2,
            after: 2,
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(Document::default())
    }
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        let stage = doc.scene().stage().stage_rect();
        let mut camera = Camera::new(stage.center(), 1.0, Size::new(1280.0, 720.0));
        camera.fit_to_rect(stage, 1.2);

        let mut selection = Selection::new();
        selection.ensure_active_layer(doc.scene());

        Self {
            doc,
            camera,
            selection,
            style: DrawStyle::default(),
            view: ViewSettings::default(),
            machine: ToolMachine::new(ToolId::Selection),
            current_frame: 0,
            playback: Playback::default(),
            onion: Onion::default(),
            library: LibraryState::default(),
            import_summary: None,
            should_quit: false,
            status: None,
        }
    }

    /// The frame the user is editing.
    pub fn frame(&self) -> u32 {
        self.current_frame
    }

    /// Move the playhead.
    ///
    /// The playhead may go **past the end of the document**, because that is
    /// how you extend it: click an empty frame in the timeline and press F5 or
    /// F6. Clamping to the current length would make the document impossible
    /// to lengthen. The bound is only there to stop an absurd value.
    pub fn set_frame(&mut self, frame: u32) {
        self.current_frame = frame.min(MAX_FRAME);
        // A selection made on another frame refers to objects that may not be
        // present here.
        self.selection.prune_to_frame(self.doc.scene(), self.current_frame);
    }

    pub fn step_frame(&mut self, delta: i64) {
        let target = (self.current_frame as i64 + delta).max(0) as u32;
        self.set_frame(target);
    }

    /// Advance playback by `elapsed` seconds. Call once per frame.
    pub fn advance_playback(&mut self, elapsed: f64) {
        if !self.playback.playing {
            return;
        }
        let fps = self.doc.scene().stage().frame_rate.max(0.01);
        self.playback.accumulator += elapsed.max(0.0);

        // Frames advanced is capped rather than elapsed time, so a normal
        // long-ish frame still advances the right number of frames. Clamping
        // the elapsed time instead silently played the document slowly.
        const MAX_CATCH_UP: u32 = 240;

        // Divide rather than subtract in a loop. Repeated subtraction
        // accumulates rounding error — half a second at 24 fps came out as 11
        // frames instead of 12 — and it is O(frames) besides.
        let per_frame = 1.0 / fps;
        let whole = (self.playback.accumulator / per_frame).floor();
        let advanced = whole.clamp(0.0, MAX_CATCH_UP as f64) as u32;

        if whole >= MAX_CATCH_UP as f64 {
            // Gave up catching up after a stall; drop the backlog.
            self.playback.accumulator = 0.0;
        } else {
            self.playback.accumulator -= advanced as f64 * per_frame;
        }
        if advanced == 0 {
            return;
        }

        let count = self.doc.scene().frame_count();
        let next = self.current_frame + advanced;
        self.current_frame = if next >= count {
            if self.playback.looping {
                next % count
            } else {
                self.playback.playing = false;
                count.saturating_sub(1)
            }
        } else {
            next
        };
    }

    pub fn toggle_playback(&mut self) {
        self.playback.playing = !self.playback.playing;
        self.playback.accumulator = 0.0;
    }

    /// Frames to draw as onion-skin ghosts, nearest first.
    pub fn onion_frames(&self) -> Vec<u32> {
        if !self.onion.enabled || self.playback.playing {
            // Ghosts during playback would just be visual noise.
            return Vec::new();
        }
        let count = self.doc.scene().frame_count();
        let mut frames = Vec::new();
        for back in 1..=self.onion.before {
            if let Some(f) = self.current_frame.checked_sub(back) {
                frames.push(f);
            }
        }
        for forward in 1..=self.onion.after {
            let f = self.current_frame + forward;
            if f < count {
                frames.push(f);
            }
        }
        frames
    }

    pub fn scene(&self) -> &Scene {
        self.doc.scene()
    }

    pub fn tool(&self) -> ToolId {
        self.machine.tool()
    }

    pub fn set_tool(&mut self, tool: ToolId) {
        if tool.is_ready() {
            self.machine.set_tool(tool);
        } else {
            self.status = Some(format!("{} is not available yet", tool.name()));
        }
    }

    fn tool_context(&self) -> ToolContext<'_> {
        ToolContext {
            style: &self.style,
            zoom: self.camera.zoom,
            selection_bounds: self.selection.bounds(self.doc.scene()),
            anchors: &[],
        }
    }

    /// Anchors of the single selected shape, in world space.
    ///
    /// Empty unless exactly one shape is selected, matching Animate: the
    /// Subselection tool edits one path at a time.
    pub fn selected_anchors(&self) -> Vec<buzz_geom::Anchor> {
        if self.selection.len() != 1 {
            return Vec::new();
        }
        let Some(id) = self.selection.iter().next() else {
            return Vec::new();
        };
        let Some((_, object)) = self.doc.scene().find_object(id) else {
            return Vec::new();
        };
        let ObjectKind::Shape(shape) = &object.kind else {
            return Vec::new();
        };

        // Reported in world space so hit-testing can compare against the
        // pointer without the caller knowing about the object's transform.
        buzz_geom::anchors(&shape.path)
            .into_iter()
            .map(|a| buzz_geom::Anchor {
                element: a.element,
                point: object.transform * a.point,
            })
            .collect()
    }

    pub fn preview(&self) -> Preview {
        self.machine.preview(&self.tool_context())
    }

    /// Document-space tolerance equivalent to a few screen pixels.
    fn pick_tolerance(&self) -> f64 {
        PICK_TOLERANCE_PX / self.camera.zoom.max(f64::MIN_POSITIVE)
    }

    // -- pointer input ------------------------------------------------------

    pub fn pointer_down(&mut self, screen: Point, mods: Mods) {
        let doc = self.camera.screen_to_doc(screen);
        let doc = self.snap(doc);

        let anchors = self.selected_anchors();
        let selection_bounds = self.selection.bounds(self.doc.scene());
        let zoom = self.camera.zoom;
        let ctx = ToolContext {
            style: &self.style,
            zoom,
            selection_bounds,
            anchors: &anchors,
        };
        self.machine.pointer_down(doc, screen, mods, &ctx);
    }

    pub fn pointer_move(&mut self, screen: Point, mods: Mods) {
        let doc = self.snap(self.camera.screen_to_doc(screen));
        let action = self.machine.pointer_move(doc, screen, mods);
        self.apply(action);
    }

    pub fn pointer_up(&mut self, screen: Point) {
        let doc = self.snap(self.camera.screen_to_doc(screen));

        // Built from disjoint fields rather than via `tool_context`, which
        // would borrow all of `self` and conflict with `&mut self.machine`.
        let anchors = self.selected_anchors();
        let selection_bounds = self.selection.bounds(self.doc.scene());
        let zoom = self.camera.zoom;
        let ctx = ToolContext {
            style: &self.style,
            zoom,
            selection_bounds,
            anchors: &anchors,
        };
        let action = self.machine.pointer_up(doc, screen, &ctx);

        self.apply(action);
        self.doc.end_gesture();
    }

    /// Snap a document point using the current view settings.
    fn snap(&self, point: Point) -> Point {
        if !self.view.snap.any() {
            return point;
        }
        // Only nearby geometry is worth considering as a snap target.
        let reach = 64.0 / self.camera.zoom.max(f64::MIN_POSITIVE);
        let around = Rect::new(
            point.x - reach,
            point.y - reach,
            point.x + reach,
            point.y + reach,
        );
        let frame = self.current_frame;
        let edges: Vec<Rect> = self
            .doc
            .scene()
            .layers()
            .drawable_at(frame)
            .flat_map(|l| l.objects_at(frame).iter())
            .map(|o| o.bounds())
            .filter(|b| b.overlaps(around))
            .take(256)
            .collect();

        self.view.snap_point(point, self.camera.zoom, &edges).point
    }

    // -- applying tool actions ----------------------------------------------

    pub fn apply(&mut self, action: ToolAction) {
        match action {
            ToolAction::None => {}

            ToolAction::AddShape { shape, label } => self.add_shape(shape, label),

            ToolAction::PickAt { point, additive } => {
                let tolerance = self.pick_tolerance();
                match self.object_at(point, tolerance) {
                    Some(id) => {
                        if additive {
                            self.selection.toggle(id);
                        } else {
                            self.selection.select_one(id);
                        }
                    }
                    None if !additive => self.selection.clear(),
                    None => {}
                }
            }

            ToolAction::PickInRect { rect, additive } => {
                let hits = self.objects_in(rect);
                if !additive {
                    self.selection.clear();
                }
                self.selection.extend(hits);
            }

            ToolAction::MoveSelection { delta } => {
                self.transform_selection(Affine::translate(delta), "Move");
            }

            ToolAction::TransformSelection { transform } => {
                self.transform_selection(transform, "Transform");
            }

            ToolAction::MoveAnchor { element, delta } => {
                let Some(id) = self.selection.iter().next() else {
                    return;
                };
                // The anchor was grabbed in world space, but the path lives in
                // the object's local space, so the delta has to come back
                // through the transform or a rotated shape would move wrongly.
                let local_delta = self
                    .doc
                    .scene()
                    .find_object(id)
                    .and_then(|(_, o)| invert(o.transform).map(|inv| inv.deref_vector(delta)))
                    .unwrap_or(delta);

                self.doc.edit("Move Anchor", |scene| {
                    update_shape(scene, id, |s| {
                        buzz_geom::move_anchor(&mut s.path, element, local_delta);
                    });
                });
            }

            ToolAction::Erase { path, width } => self.erase(path, width),

            ToolAction::BucketFill { point } => {
                let tolerance = self.pick_tolerance();
                let color = self.style.fill_color;
                if let Some(id) = self.object_at(point, tolerance) {
                    self.doc.edit("Paint Bucket", |scene| {
                        update_shape(scene, id, |s| s.fill = Some(FillSpec::solid(color)));
                    });
                }
            }

            ToolAction::ApplyStroke { point } => {
                let tolerance = self.pick_tolerance();
                let stroke = self.style.stroke_for_new_shape();
                if let (Some(id), Some((color, width, hairline))) =
                    (self.object_at(point, tolerance), stroke)
                {
                    self.doc.edit("Ink Bottle", |scene| {
                        update_shape(scene, id, |s| {
                            s.stroke = Some(StrokeSpec {
                                color,
                                width,
                                hairline,
                            })
                        });
                    });
                }
            }

            ToolAction::SampleColor { point } => {
                let tolerance = self.pick_tolerance();
                if let Some(id) = self.object_at(point, tolerance)
                    && let Some((_, object)) = self.doc.scene().find_object(id)
                    && let ObjectKind::Shape(shape) = &object.kind
                {
                    if let Some(fill) = shape.fill {
                        self.style.fill_color = fill.color;
                        self.style.fill_enabled = true;
                        self.style.remember(fill.color);
                    } else if let Some(stroke) = shape.stroke {
                        self.style.stroke_color = stroke.color;
                        self.style.stroke_enabled = true;
                        self.style.remember(stroke.color);
                    }
                }
            }

            ToolAction::PanView { delta_screen } => self.camera.pan_screen(delta_screen),

            ToolAction::MoveCamera { delta_doc } => self.nudge_camera(delta_doc),

            ToolAction::ZoomView { factor, at_screen } => {
                self.camera.zoom_by_at(factor, at_screen);
            }

            ToolAction::Deselect => self.selection.clear(),
        }
    }

    // -- document operations -------------------------------------------------

    /// The layer new artwork goes on, or `None` if none is usable.
    fn active_layer(&mut self) -> Option<LayerId> {
        let scene = self.doc.scene().clone();
        self.selection.ensure_active_layer(&scene)
    }

    fn add_shape(&mut self, shape: ShapeData, label: &'static str) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer available to draw on".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }
        if !self.style.can_draw() {
            self.status = Some("Set a stroke or fill colour first".into());
            return;
        }

        let merge = self.style.drawing_mode == DrawingMode::MergeShape;
        let frame = self.current_frame;
        let mut created: Option<ObjectId> = None;

        self.doc.edit(label, |scene| {
            created = if merge {
                merge_shape_into_layer(scene, layer, frame, shape)
            } else {
                scene.add_shape_at(layer, frame, shape)
            };
        });

        // Animate leaves a freshly drawn shape selected in object-drawing mode
        // and deselected in merge mode, because a merged shape may no longer
        // be a distinct object.
        if !merge && let Some(id) = created {
            self.selection.select_one(id);
        }
    }

    fn transform_selection(&mut self, transform: Affine, label: &'static str) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.ids();
        self.doc.edit(label, |scene| {
            for id in ids {
                update_object(scene, id, |o| o.transform = transform * o.transform);
            }
        });
    }

    fn erase(&mut self, path: BezPath, width: f64) {
        if path.elements().is_empty() {
            return;
        }
        let Some(layer) = self.active_layer() else {
            return;
        };

        let cutter = buzz_geom::outline_stroke(
            &path,
            buzz_geom::StrokeStyle::new(width.max(0.01)),
            (width / 40.0).max(1e-4),
        );
        if cutter.elements().is_empty() {
            return;
        }
        let opts = buzz_geom::BooleanOptions::for_shape_size(
            cutter.bounding_box().width().hypot(cutter.bounding_box().height()),
        );

        let frame = self.current_frame;
        self.doc.edit("Erase", |scene| {
            let ids: Vec<ObjectId> = scene
                .layers()
                .get(layer)
                .map(|l| l.objects_at(frame).iter().map(|o| o.id).collect())
                .unwrap_or_default();

            for id in ids {
                let mut became_empty = false;
                update_shape(scene, id, |s| {
                    s.path = buzz_geom::boolean(
                        &s.path,
                        &cutter,
                        buzz_geom::BoolOp::Difference,
                        opts,
                    );
                    became_empty = s.path.elements().is_empty();
                });
                if became_empty {
                    scene.remove_object(id);
                }
            }
        });
        self.selection.prune(self.doc.scene());
    }

    // -- hit testing ---------------------------------------------------------

    /// Topmost object under `point`, honouring layer locking and visibility.
    pub fn object_at(&self, point: Point, tolerance: f64) -> Option<ObjectId> {
        let scene = self.doc.scene();
        let frame = self.current_frame;
        let mut hit = None;
        // `selectable` yields back to front, so the last match is on top.
        for layer in scene.layers().selectable() {
            for object in layer.objects_at(frame) {
                if !object.visible || object.locked {
                    continue;
                }
                if object_contains(scene, object, point, tolerance, frame, 0) {
                    hit = Some(object.id);
                }
            }
        }
        hit
    }

    /// Objects fully inside `rect`, matching Animate's marquee.
    pub fn objects_in(&self, rect: Rect) -> Vec<ObjectId> {
        let frame = self.current_frame;
        self.doc
            .scene()
            .layers()
            .selectable()
            .flat_map(|l| l.objects_at(frame).iter())
            .filter(|o| o.visible && !o.locked)
            .filter(|o| rect.contains_rect(o.bounds()))
            .map(|o| o.id)
            .collect()
    }

    // -- commands ------------------------------------------------------------

    pub fn run(&mut self, command: Command) {
        use Command::*;
        match command {
            New => {
                self.doc = Document::default();
                self.selection = Selection::new();
                self.selection.ensure_active_layer(self.doc.scene());
                self.zoom_fit();
            }
            Save | SaveAs | Open | Close => {
                // File dialogs are host concerns; the shell handles them.
                self.status = Some(format!("{} is handled by the shell", command.label()));
            }
            Quit => self.should_quit = true,

            Undo => {
                if self.doc.undo() {
                    self.selection.prune(self.doc.scene());
                }
            }
            Redo => {
                if self.doc.redo() {
                    self.selection.prune(self.doc.scene());
                }
            }
            Delete => self.delete_selection(),
            SelectAll => {
                let frame = self.current_frame;
                let all: Vec<ObjectId> = self
                    .doc
                    .scene()
                    .layers()
                    .selectable()
                    .flat_map(|l| l.objects_at(frame).iter())
                    .filter(|o| o.visible && !o.locked)
                    .map(|o| o.id)
                    .collect();
                self.selection.set(all);
            }
            Deselect => self.selection.clear(),
            DuplicateSelection => self.duplicate_selection(),
            Cut => {
                self.delete_selection();
            }
            Copy | Paste => {
                self.status = Some("Clipboard arrives with the Phase 2 follow-up".into());
            }

            ZoomIn => self.zoom_by(2.0),
            ZoomOut => self.zoom_by(0.5),
            ZoomActual => self.camera.set_zoom_percent(100.0),
            ZoomFitInWindow => self.zoom_fit(),
            ZoomShowFrame => {
                let stage = self.doc.scene().stage().stage_rect();
                self.camera.fit_to_rect(stage, 1.05);
            }
            ZoomShowAll => {
                let bounds = self.doc.scene().fit_bounds();
                self.camera.fit_to_rect(bounds, 1.1);
            }

            ToggleRulers => self.view.show_rulers = !self.view.show_rulers,
            ToggleGrid => self.view.show_grid = !self.view.show_grid,
            ToggleGuides => self.view.show_guides = !self.view.show_guides,
            ToggleSnapping => self.view.snap.to_objects = !self.view.snap.to_objects,
            TogglePasteboard => self.view.show_pasteboard = !self.view.show_pasteboard,

            GroupSelection => self.group_selection(),
            UngroupSelection => self.ungroup_selection(),
            BringToFront => self.reorder_selection(Reorder::Front),
            BringForward => self.reorder_selection(Reorder::Forward),
            SendBackward => self.reorder_selection(Reorder::Backward),
            SendToBack => self.reorder_selection(Reorder::Back),

            ConvertLinesToFills => self.convert_lines_to_fills(),
            ExpandFill => self.expand_selection(2.0),
            SmoothSelection => self.reshape_selection(Reshape::Smooth),
            StraightenSelection => self.reshape_selection(Reshape::Straighten),

            NewLayer => {
                let id = self.doc_add_layer("Layer", LayerKind::Normal);
                self.selection.set_active_layer(Some(id));
            }
            NewLayerFolder => {
                self.doc_add_layer("Folder", LayerKind::Folder);
            }
            DeleteLayer => self.delete_active_layer(),

            // -- timeline ----------------------------------------------------
            InsertFrame => self.frame_op(FrameOp::InsertFrame),
            RemoveFrame => self.frame_op(FrameOp::RemoveFrame),
            InsertKeyframe => self.frame_op(FrameOp::InsertKeyframe),
            InsertBlankKeyframe => self.frame_op(FrameOp::InsertBlankKeyframe),
            ClearKeyframe => self.frame_op(FrameOp::ClearKeyframe),
            PlayPause => self.toggle_playback(),
            NextFrame => {
                // Stepping past the end extends nothing; use F5 for that.
                self.step_frame(1);
            }
            PreviousFrame => self.step_frame(-1),
            FirstFrame => self.set_frame(0),
            LastFrame => {
                let last = self.doc.scene().frame_count().saturating_sub(1);
                self.set_frame(last);
            }
            ToggleOnionSkin => self.onion.enabled = !self.onion.enabled,

            // -- camera ------------------------------------------------------
            ToggleCamera => self.toggle_camera(),
            AddCameraKeyframe => self.add_camera_key(),
            RemoveCameraKeyframe => {
                let frame = self.current_frame;
                self.doc.edit("Remove Camera Keyframe", |scene| {
                    scene.camera_mut().remove_key(frame);
                });
            }
            ResetCamera => {
                self.doc.edit("Reset Camera", |scene| {
                    scene.camera_mut().clear();
                });
            }

            // -- symbols and library -----------------------------------------
            ConvertToSymbol => self.convert_selection_to_symbol(),
            BrushFromSelection => self.brush_from_selection(),
            NewSymbol => self.new_symbol(),
            EditSymbol => self.edit_selected_symbol(),
            EditDocument => {
                if !self.doc.scene().edit_path().is_empty() {
                    self.doc.edit_view(|scene| scene.edit_document());
                    self.after_context_change();
                }
            }
            PlaceInstance => self.place_library_instance(),
            DuplicateSymbol => self.duplicate_library_symbol(),
            DeleteSymbol => self.delete_library_symbol(),
            NewLibraryFolder => self.new_library_folder(),

            // -- tweens ------------------------------------------------------
            CreateClassicTween => self.set_tween(Tween::classic()),
            CreateMotionTween => self.set_tween(Tween::motion()),
            CreateShapeTween => self.set_tween(Tween::shape()),
            RemoveTween => self.set_tween(Tween::default()),

            ImportToLibrary | ImportToStage => {
                // Handled by the shell, which owns the file dialog, as with
                // Open and Save. Reaching here means a code path raised the
                // command without going through `App::dispatch`.
                debug_assert!(false, "{command:?} must be dispatched by the shell");
            }

            SelectTool(tool) => self.set_tool(tool),
        }
    }

    // -- symbols -------------------------------------------------------------

    /// Everything the Library panel needs the editor to remember.
    pub fn library_selection(&self) -> Option<buzz_scene::SymbolId> {
        self.library.selected
    }

    /// Re-settle the editor after entering or leaving a symbol.
    ///
    /// The symbol's timeline is a different length and holds different
    /// objects, so a playhead and a selection from the old context would both
    /// be meaningless.
    fn after_context_change(&mut self) {
        self.selection.clear();
        self.selection.set_active_layer(None);
        self.selection.ensure_active_layer(self.doc.scene());
        self.set_frame(0);
        self.playback.playing = false;
    }

    /// Animate's *Create Brush From Selection*: adopt the selected artwork as
    /// the shape a pattern brush stamps.
    ///
    /// The shape is recentred on its own origin, because stamps are placed
    /// centred on the stroke — artwork drawn at (400, 300) would otherwise
    /// stamp 500 units away from the pointer.
    fn brush_from_selection(&mut self) {
        let mut combined = buzz_geom::BezPath::new();
        for id in self.selection.iter() {
            let Some((_, object)) = self.doc.scene().find_object(id) else {
                continue;
            };
            // Flattening resolves groups and applies each object's transform,
            // so a brush made from a group comes out as it looked on stage.
            let mut parts = Vec::new();
            object.flatten(buzz_geom::Affine::IDENTITY, &mut parts);
            for (transform, shape) in parts {
                for element in (transform * shape.path).elements() {
                    combined.push(*element);
                }
            }
        }

        if combined.elements().is_empty() {
            self.status = Some("Select some artwork to make a brush from".into());
            return;
        }

        let bounds = buzz_geom::Shape::bounding_box(&combined);
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            self.status = Some("That selection has no area to make a brush from".into());
            return;
        }
        let centred = buzz_geom::Affine::translate(-bounds.center().to_vec2()) * combined;

        // Selecting the brush too: making a brush and not being given it is a
        // step the user would always have to take next.
        self.style.brush.set_custom_pattern(centred);
        if !self.style.brush.kind.uses_pattern() {
            self.style.brush.kind = buzz_ui::BrushKind::Pattern;
        }
        self.set_tool(ToolId::Brush);
        self.status = Some("Brush created from the selection".into());
    }

    /// Animate's F8: replace the selection with an instance of a new symbol.
    ///
    /// The artwork moves *into* the symbol rather than being copied, which is
    /// what makes F8 a conversion rather than a duplication.
    fn convert_selection_to_symbol(&mut self) {
        if self.selection.is_empty() {
            self.status = Some("Select artwork first, then Convert to Symbol".into());
            return;
        }
        let Some(target_layer) = self.active_layer() else {
            return;
        };
        let ids: Vec<ObjectId> = self.selection.iter().collect();
        let frame = self.current_frame;
        let folder = self.library.selected_folder.clone();
        let kind = self.library.new_symbol_kind;

        // The instance's origin is the selection's top-left, so the artwork
        // does not jump when it is replaced.
        let origin = self
            .selection
            .bounds(self.doc.scene())
            .map(|b| b.origin())
            .unwrap_or(Point::ZERO);

        let mut placed = None;
        self.doc.edit("Convert to Symbol", |scene| {
            let symbol = scene.add_symbol("Symbol", kind, folder.as_deref());
            let Some(inner_layer) = scene
                .library()
                .get(symbol)
                .and_then(|s| s.layers.iter().next())
                .map(|l| l.id)
            else {
                return;
            };

            // Lift the artwork out, rebasing it so the symbol's registration
            // point sits at its top-left corner.
            let shift = Affine::translate((-origin.x, -origin.y));
            let mut lifted = Vec::new();
            for id in &ids {
                if let Some(object) = scene.remove_object(*id) {
                    let mut object = (*object).clone();
                    object.transform = shift * object.transform;
                    lifted.push(Arc::new(object));
                }
            }
            if lifted.is_empty() {
                return;
            }
            scene.library_mut().update(symbol, |s| {
                s.layers.update(inner_layer, |l| {
                    l.frames.set_objects(0, lifted);
                });
            });

            placed = scene.add_instance_at(
                target_layer,
                frame,
                symbol,
                Affine::translate((origin.x, origin.y)),
            );
        });

        match placed {
            Some(id) => {
                self.selection.set([id]);
                self.status = Some("Converted to symbol".into());
            }
            None => self.status = Some("Nothing was converted".into()),
        }
    }

    /// Animate's Ctrl+F8: an empty symbol, opened for editing.
    fn new_symbol(&mut self) {
        let folder = self.library.selected_folder.clone();
        let kind = self.library.new_symbol_kind;
        let mut created = None;
        self.doc.edit("New Symbol", |scene| {
            created = Some(scene.add_symbol("Symbol", kind, folder.as_deref()));
        });
        if let Some(id) = created {
            self.library.selected = Some(id);
            self.doc.edit_view(|scene| {
                scene.enter_symbol(id);
            });
            self.after_context_change();
        }
    }

    /// Open a symbol: the library selection if there is one, otherwise the
    /// selected instance's symbol. Animate's Ctrl+E does both.
    fn edit_selected_symbol(&mut self) {
        let from_instance = self
            .selection
            .iter()
            .next()
            .and_then(|id| self.doc.scene().find_object(id))
            .and_then(|(_, o)| o.instance())
            .map(|i| i.symbol);

        let Some(id) = self.library.selected.or(from_instance) else {
            self.status = Some("Select a symbol or an instance first".into());
            return;
        };

        let mut entered = false;
        self.doc.edit_view(|scene| entered = scene.enter_symbol(id));
        if entered {
            self.library.selected = Some(id);
            self.after_context_change();
        } else {
            self.status = Some("That symbol is no longer in the library".into());
        }
    }

    /// Place an instance of the library selection at the centre of the view.
    fn place_library_instance(&mut self) {
        let Some(symbol) = self.library.selected else {
            self.status = Some("Select a symbol in the Library first".into());
            return;
        };
        let Some(layer) = self.active_layer() else {
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }

        let at = self.camera.center;
        let frame = self.current_frame;
        let mut placed = None;
        self.doc.edit("Place Instance", |scene| {
            placed = scene.add_instance_at(layer, frame, symbol, Affine::translate((at.x, at.y)));
        });
        match placed {
            Some(id) => self.selection.set([id]),
            None => self.status = Some("Could not place the instance here".into()),
        }
    }

    fn duplicate_library_symbol(&mut self) {
        let Some(id) = self.library.selected else {
            return;
        };
        let mut created = None;
        self.doc.edit("Duplicate Symbol", |scene| {
            let Some(source) = scene.library().get(id).cloned() else {
                return;
            };
            let new_id = scene.add_symbol(
                source.name.clone(),
                source.kind,
                source.folder.as_deref(),
            );
            // Copying the layer stack wholesale shares every `Arc` inside it;
            // the artwork is only cloned if one of the two is edited.
            scene.library_mut().update(new_id, |s| {
                s.layers = source.layers.clone();
                s.registration = source.registration;
            });
            created = Some(new_id);
        });
        if let Some(new_id) = created {
            self.library.selected = Some(new_id);
        }
    }

    fn delete_library_symbol(&mut self) {
        let Some(id) = self.library.selected else {
            return;
        };
        // Animate warns before deleting a symbol that is still placed; the
        // count is the same one the panel shows, so the warning matches what
        // the user can already see.
        let uses = self.doc.scene().symbol_usage().get(&id).copied().unwrap_or(0);

        self.doc.edit("Delete Symbol", |scene| {
            // Leaving symbol editing first, or the editor would be pointed at
            // a timeline that no longer exists.
            scene.edit_document();
            scene.library_mut().remove(id);
        });
        self.library.selected = None;
        self.after_context_change();
        if uses > 0 {
            self.status = Some(format!(
                "Deleted a symbol that was placed {uses} time(s); those instances now draw nothing"
            ));
        }
    }

    fn new_library_folder(&mut self) {
        // Nest inside the selected folder, which is how a tree gets built
        // without a dialog.
        let parent = self.library.selected_folder.clone();
        let mut created = None;
        self.doc.edit("New Library Folder", |scene| {
            let base = match &parent {
                Some(p) => format!("{p}/Folder"),
                None => "Folder".to_string(),
            };
            // Distinct from any sibling, so two folders never collide.
            let mut path = base.clone();
            for n in 1..1000 {
                if !scene.library().folders().any(|f| *f == path) {
                    break;
                }
                path = format!("{base} {n}");
            }
            scene.library_mut().add_folder(&path);
            created = Some(path);
        });
        self.library.selected_folder = created;
    }

    /// Set the tween on the keyframe governing the playhead.
    fn set_tween(&mut self, tween: Tween) {
        let Some(layer) = self.active_layer() else {
            return;
        };
        let frame = self.current_frame;
        let mut ok = false;
        self.doc.edit("Tween", |scene| {
            scene.update_layer(layer, |l| ok = l.frames.set_tween(frame, tween));
        });
        if !ok {
            self.status = Some("Put the playhead on a keyframe to tween from it".into());
        } else if tween.is_active() && self.tween_span_length(layer, frame) < 2 {
            // A tween needs somewhere to go. Saying so beats leaving the user
            // wondering why nothing moves.
            self.status =
                Some("Tween set, but there is no following keyframe to tween towards".into());
        }
    }

    /// Frames between this keyframe and the next, for diagnostics.
    fn tween_span_length(&self, layer: LayerId, frame: u32) -> u32 {
        let Some(l) = self.doc.scene().layers().get(layer) else {
            return 0;
        };
        let Some(start) = l.frames.keyframe_start(frame) else {
            return 0;
        };
        let next = l
            .frames
            .keyframes()
            .iter()
            .map(|k| k.start)
            .find(|s| *s > start);
        next.map(|n| n - start).unwrap_or(0)
    }

    /// Apply a frame operation to the active layer.
    fn frame_op(&mut self, op: FrameOp) {
        let Some(layer) = self.active_layer() else {
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }

        let frame = self.current_frame;
        let label = op.label();
        let mut changed = false;

        self.doc.edit(label, |scene| {
            scene.update_layer(layer, |l| {
                changed = match op {
                    FrameOp::InsertFrame => l.frames.insert_frame(frame),
                    FrameOp::RemoveFrame => l.frames.remove_frame(frame),
                    FrameOp::InsertKeyframe => l.frames.insert_keyframe(frame),
                    FrameOp::InsertBlankKeyframe => l.frames.insert_blank_keyframe(frame),
                    FrameOp::ClearKeyframe => l.frames.clear_keyframe(frame),
                };
            });
        });

        if !changed {
            self.status = Some(format!("{label} did nothing here"));
        }
        // Removing frames can shorten the document past the playhead.
        self.set_frame(self.current_frame);
        self.selection.prune(self.doc.scene());
    }

    /// Turn the camera on, seeding a key so it has something to show.
    fn toggle_camera(&mut self) {
        let frame = self.current_frame;
        let centre = self.doc.scene().stage().stage_rect().center();
        self.doc.edit("Toggle Camera", |scene| {
            scene.camera_mut().enabled = !scene.camera().enabled;
            if scene.camera().enabled && scene.camera().is_empty() {
                // Without a key the camera would be enabled but inert, which
                // looks like a bug.
                scene.camera_mut().set_key(buzz_scene::CameraKey::new(frame, centre));
            }
        });
        let on = self.doc.scene().camera().enabled;
        self.status = Some(if on {
            "Camera enabled — use the Camera tool to move it".into()
        } else {
            "Camera disabled".to_string()
        });
    }

    /// Key the camera's current state at the playhead.
    fn add_camera_key(&mut self) {
        let frame = self.current_frame;
        let centre = self.doc.scene().stage().stage_rect().center();
        let current = self.doc.scene().camera().state_at(frame);
        self.doc.edit("Camera Keyframe", |scene| {
            scene.camera_mut().enabled = true;
            let key = current.map(|s| buzz_scene::CameraKey { frame, ..s }).unwrap_or(
                buzz_scene::CameraKey::new(frame, centre),
            );
            scene.camera_mut().set_key(key);
        });
    }

    /// Move the camera at the playhead, keying it if needed.
    ///
    /// The Camera tool drags the *view*, so dragging right moves the camera
    /// left — the same inversion a real camera has, and what Animate does.
    pub fn nudge_camera(&mut self, delta_doc: Vec2) {
        let frame = self.current_frame;
        let centre = self.doc.scene().stage().stage_rect().center();
        let current = self.doc.scene().camera().state_at(frame);

        self.doc.edit("Move Camera", |scene| {
            scene.camera_mut().enabled = true;
            let mut key = current
                .map(|s| buzz_scene::CameraKey { frame, ..s })
                .unwrap_or(buzz_scene::CameraKey::new(frame, centre));
            key.center -= delta_doc;
            scene.camera_mut().set_key(key);
        });
    }

    /// Zoom the camera at the playhead.
    pub fn zoom_camera(&mut self, factor: f64) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let frame = self.current_frame;
        let centre = self.doc.scene().stage().stage_rect().center();
        let current = self.doc.scene().camera().state_at(frame);

        self.doc.edit("Zoom Camera", |scene| {
            scene.camera_mut().enabled = true;
            let mut key = current
                .map(|s| buzz_scene::CameraKey { frame, ..s })
                .unwrap_or(buzz_scene::CameraKey::new(frame, centre));
            key.zoom = (key.zoom * factor).clamp(0.01, 1000.0);
            scene.camera_mut().set_key(key);
        });
    }

    fn doc_add_layer(&mut self, prefix: &str, kind: LayerKind) -> LayerId {
        let n = self.doc.scene().layers().len() + 1;
        let mut created = LayerId(0);
        self.doc.edit("New Layer", |scene| {
            created = scene.add_layer(format!("{prefix}_{n}"), kind);
        });
        created
    }

    fn delete_active_layer(&mut self) {
        let Some(layer) = self.selection.active_layer() else {
            return;
        };
        if self.doc.scene().layers().len() <= 1 {
            self.status = Some("A document must keep at least one layer".into());
            return;
        }
        self.doc.edit("Delete Layer", |scene| {
            scene.remove_layer(layer);
        });
        self.selection.set_active_layer(None);
        self.selection.prune(self.doc.scene());
        self.selection.ensure_active_layer(&self.doc.scene().clone());
    }

    fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.ids();
        self.doc.edit("Delete", |scene| {
            for id in ids {
                scene.remove_object(id);
            }
        });
        self.selection.clear();
    }

    fn duplicate_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.ids();
        let mut created = Vec::new();
        self.doc.edit("Duplicate", |scene| {
            for id in ids {
                let Some((layer, original)) = scene.find_object(id).map(|(l, o)| (l, o.clone()))
                else {
                    continue;
                };
                let new_id = scene.next_object_id();
                let mut copy = (*original).clone();
                copy.id = new_id;
                // Offset slightly so the duplicate is visible and grabbable.
                copy.transform = Affine::translate((10.0, 10.0)) * copy.transform;
                if scene.add_object(layer, copy).is_some() {
                    created.push(new_id);
                }
            }
        });
        self.selection.set(created);
    }

    fn group_selection(&mut self) {
        if self.selection.len() < 2 {
            return;
        }
        let ids = self.selection.ids();
        let mut group_id = None;
        self.doc.edit("Group", |scene| {
            let Some((layer, _)) = scene.find_object(ids[0]) else {
                return;
            };
            let mut children = Vec::new();
            for id in &ids {
                if let Some(object) = scene.remove_object(*id) {
                    children.push(object);
                }
            }
            if children.is_empty() {
                return;
            }
            let id = scene.next_object_id();
            let group = Object::group(id, children);
            if scene.add_object(layer, group).is_some() {
                group_id = Some(id);
            }
        });
        if let Some(id) = group_id {
            self.selection.select_one(id);
        }
    }

    fn ungroup_selection(&mut self) {
        let ids = self.selection.ids();
        let mut freed = Vec::new();
        self.doc.edit("Ungroup", |scene| {
            for id in ids {
                let Some((layer, object)) = scene.find_object(id).map(|(l, o)| (l, o.clone()))
                else {
                    continue;
                };
                let ObjectKind::Group(children) = &object.kind else {
                    freed.push(id);
                    continue;
                };
                let children = children.clone();
                scene.remove_object(id);
                for child in children {
                    let mut promoted = (*child).clone();
                    // Fold the group's transform into each child so nothing
                    // jumps when the group disappears.
                    promoted.transform = object.transform * promoted.transform;
                    promoted.id = scene.next_object_id();
                    let child_id = promoted.id;
                    if scene.add_object(layer, promoted).is_some() {
                        freed.push(child_id);
                    }
                }
            }
        });
        self.selection.set(freed);
    }

    fn reorder_selection(&mut self, how: Reorder) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.ids();
        let frame = self.current_frame;
        self.doc.edit("Arrange", |scene| {
            for id in ids {
                let Some((layer, _)) = scene.find_object(id) else {
                    continue;
                };
                scene.update_layer(layer, |l| {
                    let Some(objects) = l.frames.objects_at_mut(frame) else {
                        return;
                    };
                    let Some(from) = objects.iter().position(|o| o.id == id) else {
                        return;
                    };
                    let last = objects.len().saturating_sub(1);
                    let to = match how {
                        Reorder::Front => last,
                        Reorder::Back => 0,
                        Reorder::Forward => (from + 1).min(last),
                        Reorder::Backward => from.saturating_sub(1),
                    };
                    let item = objects.remove(from);
                    objects.insert(to, item);
                });
            }
        });
    }

    fn convert_lines_to_fills(&mut self) {
        let ids = self.selection.ids();
        self.doc.edit("Convert Lines to Fills", |scene| {
            for id in ids {
                update_shape(scene, id, |s| {
                    let Some(stroke) = s.stroke else { return };
                    let width = if stroke.hairline { 1.0 } else { stroke.width };
                    let outline = buzz_geom::outline_stroke(
                        &s.path,
                        buzz_geom::StrokeStyle::new(width.max(0.01)),
                        (width / 40.0).max(1e-4),
                    );
                    if outline.elements().is_empty() {
                        return;
                    }
                    s.path = outline;
                    s.fill = Some(FillSpec::solid(stroke.color));
                    s.stroke = None;
                });
            }
        });
    }

    fn expand_selection(&mut self, amount: f64) {
        let ids = self.selection.ids();
        self.doc.edit("Expand Fill", |scene| {
            for id in ids {
                update_shape(scene, id, |s| {
                    let bb = s.path.bounding_box();
                    let opts =
                        buzz_geom::BooleanOptions::for_shape_size(bb.width().hypot(bb.height()));
                    s.path = buzz_geom::expand_fill(&s.path, amount, opts);
                });
            }
        });
    }

    fn reshape_selection(&mut self, how: Reshape) {
        let ids = self.selection.ids();
        let label = match how {
            Reshape::Smooth => "Smooth",
            Reshape::Straighten => "Straighten",
        };
        self.doc.edit(label, |scene| {
            for id in ids {
                update_shape(scene, id, |s| {
                    let bb = s.path.bounding_box();
                    let amount = (bb.width().hypot(bb.height()) / 200.0).clamp(0.01, 10.0);
                    s.path = match how {
                        Reshape::Smooth => buzz_geom::smooth(&s.path, amount),
                        Reshape::Straighten => buzz_geom::straighten(&s.path, amount),
                    };
                });
            }
        });
    }

    // -- view ----------------------------------------------------------------

    pub fn zoom_by(&mut self, factor: f64) {
        let centre = Point::new(
            self.camera.viewport.width / 2.0,
            self.camera.viewport.height / 2.0,
        );
        self.camera.zoom_by_at(factor, centre);
    }

    pub fn zoom_fit(&mut self) {
        let stage = self.doc.scene().stage().stage_rect();
        self.camera.fit_to_rect(stage, 1.2);
    }
}

#[derive(Debug, Clone, Copy)]
enum Reorder {
    Front,
    Forward,
    Backward,
    Back,
}

#[derive(Debug, Clone, Copy)]
enum Reshape {
    Smooth,
    Straighten,
}

/// Frame operations, matching Animate's function keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameOp {
    InsertFrame,
    RemoveFrame,
    InsertKeyframe,
    InsertBlankKeyframe,
    ClearKeyframe,
}

impl FrameOp {
    fn label(self) -> &'static str {
        match self {
            Self::InsertFrame => "Insert Frame",
            Self::RemoveFrame => "Remove Frame",
            Self::InsertKeyframe => "Insert Keyframe",
            Self::InsertBlankKeyframe => "Insert Blank Keyframe",
            Self::ClearKeyframe => "Clear Keyframe",
        }
    }
}

/// Edit an object in place.
fn update_object(scene: &mut Scene, id: ObjectId, f: impl FnOnce(&mut Object)) {
    let Some((layer, _)) = scene.find_object(id) else {
        return;
    };
    scene.update_layer(layer, |l| {
        // The object may live on any keyframe, so search them all rather than
        // assuming the current one.
        for keyframe in l.frames.keyframes_mut() {
            let objects = Arc::make_mut(&mut keyframe.objects);
            if let Some(slot) = objects.iter_mut().find(|o| o.id == id) {
                f(Arc::make_mut(slot));
                return;
            }
        }
    });
}

/// Edit a shape in place, ignoring groups.
fn update_shape(scene: &mut Scene, id: ObjectId, f: impl FnOnce(&mut ShapeData)) {
    update_object(scene, id, |o| {
        if let ObjectKind::Shape(shape) = &mut o.kind {
            f(shape);
        }
    });
}

/// Precise hit test against an object, including its transform.
///
/// `scene` is needed because a symbol instance's real geometry lives in the
/// library, not in the object; without it an instance could only be tested
/// against a placeholder rectangle. `frame` is the timeline position the
/// object is being tested at, which nested graphic symbols inherit.
fn object_contains(
    scene: &Scene,
    object: &Object,
    point: Point,
    tolerance: f64,
    frame: u32,
    depth: usize,
) -> bool {
    // Cheap rejection first, resolved through the library so an instance is
    // rejected on its artwork's extents rather than a placeholder.
    if !scene
        .resolved_bounds(object)
        .inflate(tolerance, tolerance)
        .contains(point)
    {
        return false;
    }

    let Some(inverse) = invert(object.transform) else {
        return false;
    };
    let local = inverse * point;

    match &object.kind {
        ObjectKind::Shape(shape) => {
            if shape.fill.is_some()
                && buzz_geom::hit::fill_contains(&shape.path, local, buzz_geom::FillMode::NonZero)
            {
                return true;
            }
            match shape.stroke {
                Some(stroke) => {
                    let width = if stroke.hairline { 0.0 } else { stroke.width };
                    buzz_geom::hit::stroke_contains(&shape.path, local, width, tolerance)
                }
                None => false,
            }
        }
        ObjectKind::Group(children) => children
            .iter()
            .any(|c| object_contains(scene, c, local, tolerance, frame, depth)),

        ObjectKind::Instance(instance) => {
            // Same cycle guard the renderer uses.
            if depth >= MAX_SYMBOL_DEPTH {
                return false;
            }
            let Some(symbol) = scene.library().get(instance.symbol) else {
                return false;
            };
            let inner = instance.resolve_frame(symbol.kind, frame, symbol.length());
            // Hit the artwork, not the bounding box: clicking the hole in a
            // ring selects what is behind it, as it does in Animate.
            symbol
                .layers
                .selectable()
                .flat_map(|l| l.objects_at(inner))
                .any(|c| object_contains(scene, c, local, tolerance, inner, depth + 1))
        }
    }
}

/// How deep a symbol may nest before hit testing gives up.
///
/// Matches the renderer's limit in [`crate::stage`]: anything it will not draw
/// must not be clickable either.
const MAX_SYMBOL_DEPTH: usize = 12;

/// Invert an affine, or `None` if it is singular.
fn invert(t: Affine) -> Option<Affine> {
    let c = t.as_coeffs();
    let determinant = c[0] * c[3] - c[1] * c[2];
    (determinant.abs() > 1e-12).then(|| t.inverse())
}

/// Extension for transforming a *direction* rather than a position.
trait DerefVector {
    /// Apply the linear part only, ignoring translation.
    fn deref_vector(&self, v: buzz_geom::Vec2) -> buzz_geom::Vec2;
}

impl DerefVector for Affine {
    fn deref_vector(&self, v: buzz_geom::Vec2) -> buzz_geom::Vec2 {
        let c = self.as_coeffs();
        buzz_geom::Vec2::new(c[0] * v.x + c[2] * v.y, c[1] * v.x + c[3] * v.y)
    }
}

/// Add a shape using Animate's **merge shape** rules.
///
/// Same-coloured fills fuse into one shape; a different colour cuts a hole.
/// This is the behaviour that makes raw shapes feel like paint rather than
/// objects, and it is why `buzz-geom`'s boolean operations exist.
///
/// Strokes are left alone: Animate merges fills, not strokes.
fn merge_shape_into_layer(
    scene: &mut Scene,
    layer: LayerId,
    frame: u32,
    incoming: ShapeData,
) -> Option<ObjectId> {
    let Some(new_fill) = incoming.fill else {
        // Nothing to merge with, so it behaves like an ordinary object.
        return scene.add_shape_at(layer, frame, incoming);
    };

    let bb = incoming.path.bounding_box();
    let opts = buzz_geom::BooleanOptions::for_shape_size(bb.width().hypot(bb.height()));
    let same_color = |a: Color, b: Color| a.to_rgba8().to_u8_array() == b.to_rgba8().to_u8_array();

    // Existing filled shapes that overlap the new one.
    let candidates: Vec<(ObjectId, Color, BezPath)> = scene
        .layers()
        .get(layer)
        .map(|l| {
            l.objects_at(frame)
                .iter()
                .filter(|o| o.visible && !o.locked)
                .filter_map(|o| match &o.kind {
                    ObjectKind::Shape(s) => s.fill.map(|f| (o.id, f.color, s.path.clone())),
                    // Merge-shape rules apply to raw shapes only. Groups and
                    // symbol instances are objects: in Animate they sit above
                    // the merge layer and never fuse with what they overlap.
                    ObjectKind::Group(_) | ObjectKind::Instance(_) => None,
                })
                .filter(|(_, _, path)| path.bounding_box().overlaps(bb))
                .collect()
        })
        .unwrap_or_default();

    let mut merged = incoming.path.clone();
    let mut absorbed = Vec::new();

    for (id, color, path) in candidates {
        if same_color(color, new_fill.color) {
            merged = buzz_geom::boolean(&merged, &path, buzz_geom::BoolOp::Union, opts);
            absorbed.push(id);
        } else {
            // Different colour: the new shape cuts into the old one.
            let mut emptied = false;
            update_shape(scene, id, |s| {
                s.path = buzz_geom::boolean(
                    &s.path,
                    &incoming.path,
                    buzz_geom::BoolOp::Difference,
                    opts,
                );
                emptied = s.path.elements().is_empty();
            });
            if emptied {
                scene.remove_object(id);
            }
        }
    }

    for id in absorbed {
        scene.remove_object(id);
    }

    scene.add_shape_at(
        layer,
        frame,
        ShapeData {
            path: merged,
            fill: Some(new_fill),
            stroke: incoming.stroke,
            // A merge takes the incoming shape's blend: it is the paint being
            // applied, and the result is one shape from here on.
            blend: incoming.blend,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Vec2;
    use kurbo::Rect as KRect;

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        KRect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    fn editor() -> Editor {
        let mut e = Editor::default();
        e.camera.viewport = Size::new(1000.0, 800.0);
        e.camera.set_zoom_percent(100.0);
        e.camera.center = Point::new(275.0, 200.0);
        e.view.snap = buzz_ui::SnapSettings {
            to_guides: false,
            to_grid: false,
            to_objects: false,
            to_pixels: false,
        };
        e
    }

    fn draw_square(e: &mut Editor, x: f64, y: f64, size: f64, color: Color) -> Option<ObjectId> {
        e.style.fill_color = color;
        e.style.stroke_enabled = false;
        let before: Vec<ObjectId> = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .map(|o| o.id)
            .collect();
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(x, y, size), color),
            label: "Draw",
        });
        e.scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .map(|o| o.id)
            .find(|id| !before.contains(id))
    }

    #[test]
    fn a_new_editor_has_a_layer_and_frames_the_stage() {
        let e = Editor::default();
        assert_eq!(e.scene().layers().len(), 1);
        assert!(e.selection.active_layer().is_some());
        assert!(e.camera.zoom > 0.0);
    }

    #[test]
    fn drawing_adds_a_shape_and_records_undo() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 50.0, Color::WHITE);

        assert_eq!(e.scene().shape_count(), 1);
        assert!(e.doc.can_undo());
        e.run(Command::Undo);
        assert_eq!(e.scene().shape_count(), 0);
    }

    #[test]
    fn drawing_on_a_locked_layer_is_refused_with_a_message() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Lock", |s| {
            s.update_layer(layer, |l| l.locked = true);
        });

        draw_square(&mut e, 0.0, 0.0, 50.0, Color::WHITE);
        assert_eq!(e.scene().shape_count(), 0);
        assert!(e.status.is_some(), "the user should be told why");
    }

    #[test]
    fn drawing_with_no_stroke_and_no_fill_is_refused() {
        let mut e = editor();
        e.style.stroke_enabled = false;
        e.style.fill_enabled = false;
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE),
            label: "Draw",
        });
        assert_eq!(e.scene().shape_count(), 0);
    }

    /// Animate's merge model: same colour fuses.
    #[test]
    fn overlapping_same_coloured_shapes_merge_into_one() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::MergeShape;

        draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE);
        draw_square(&mut e, 50.0, 0.0, 100.0, Color::WHITE);

        assert_eq!(
            e.scene().shape_count(),
            1,
            "two overlapping same-coloured shapes should become one"
        );
        let bounds = e.scene().content_bounds().unwrap();
        assert!((bounds.width() - 150.0).abs() < 1.0, "got {bounds:?}");
    }

    /// ...and a different colour cuts.
    #[test]
    fn a_different_coloured_shape_cuts_the_one_beneath() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::MergeShape;

        draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE);
        let area_before = shape_area(&e);

        draw_square(&mut e, 50.0, 50.0, 100.0, Color::BLACK);

        assert_eq!(e.scene().shape_count(), 2, "both shapes should remain");
        let white = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .find_map(|o| match &o.kind {
                ObjectKind::Shape(s) if s.fill.map(|f| f.color.to_rgba8().to_u8_array()[0]) == Some(255) => {
                    Some(s.path.area().abs())
                }
                _ => None,
            })
            .expect("the white shape should still exist");

        assert!(
            white < area_before - 100.0,
            "the white shape should have been cut: {white} vs {area_before}"
        );
    }

    #[test]
    fn object_drawing_mode_keeps_shapes_separate() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE);
        draw_square(&mut e, 50.0, 0.0, 100.0, Color::WHITE);

        assert_eq!(
            e.scene().shape_count(),
            2,
            "object drawing must not merge shapes"
        );
    }

    fn shape_area(e: &Editor) -> f64 {
        e.scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .filter_map(|o| match &o.kind {
                ObjectKind::Shape(s) => Some(s.path.area().abs()),
                _ => None,
            })
            .sum()
    }

    #[test]
    fn clicking_selects_the_shape_under_the_cursor() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE).unwrap();
        e.selection.clear();

        e.apply(ToolAction::PickAt {
            point: Point::new(50.0, 50.0),
            additive: false,
        });
        assert!(e.selection.contains(id));

        // Clicking empty space clears it.
        e.apply(ToolAction::PickAt {
            point: Point::new(500.0, 500.0),
            additive: false,
        });
        assert!(e.selection.is_empty());
    }

    #[test]
    fn a_locked_layer_cannot_be_selected_on() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE);
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Lock", |s| s.update_layer(layer, |l| l.locked = true).then_some(()).map(|_| ()).unwrap_or(()));
        e.selection.clear();

        e.apply(ToolAction::PickAt {
            point: Point::new(50.0, 50.0),
            additive: false,
        });
        assert!(e.selection.is_empty(), "a locked layer must not be clickable");
    }

    #[test]
    fn marquee_selection_takes_only_fully_enclosed_shapes() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE);
        draw_square(&mut e, 200.0, 0.0, 20.0, Color::WHITE);

        e.apply(ToolAction::PickInRect {
            rect: Rect::new(-10.0, -10.0, 100.0, 100.0),
            additive: false,
        });
        assert_eq!(e.selection.len(), 1);
    }

    #[test]
    fn moving_the_selection_shifts_it_and_is_undoable() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 50.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        let before = e.scene().find_object(id).unwrap().1.bounds();
        e.apply(ToolAction::MoveSelection {
            delta: Vec2::new(30.0, 20.0),
        });
        let after = e.scene().find_object(id).unwrap().1.bounds();

        assert!((after.x0 - before.x0 - 30.0).abs() < 1e-9);
        e.run(Command::Undo);
        assert!((e.scene().find_object(id).unwrap().1.bounds().x0 - before.x0).abs() < 1e-9);
    }

    #[test]
    fn grouping_and_ungrouping_round_trip() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        let b = draw_square(&mut e, 50.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.set([a, b]);

        e.run(Command::GroupSelection);
        assert_eq!(e.scene().shape_count(), 2, "the leaves still exist");
        let top_level = e.scene().layers().iter().map(|l| l.objects_at(0).len()).sum::<usize>();
        assert_eq!(top_level, 1, "they should be inside one group");

        e.run(Command::UngroupSelection);
        let top_level = e.scene().layers().iter().map(|l| l.objects_at(0).len()).sum::<usize>();
        assert_eq!(top_level, 2, "ungrouping should free both");
    }

    #[test]
    fn deleting_removes_the_selection_and_can_be_undone() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        e.run(Command::Delete);
        assert_eq!(e.scene().shape_count(), 0);
        assert!(e.selection.is_empty());

        e.run(Command::Undo);
        assert_eq!(e.scene().shape_count(), 1);
    }

    #[test]
    fn duplicating_offsets_the_copy_and_selects_it() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        e.run(Command::DuplicateSelection);
        assert_eq!(e.scene().shape_count(), 2);
        assert_eq!(e.selection.len(), 1);
        assert!(!e.selection.contains(id), "the copy should be selected");
    }

    #[test]
    fn select_all_skips_hidden_and_locked_layers() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE);

        let hidden = e.doc_add_layer("Hidden", LayerKind::Normal);
        e.selection.set_active_layer(Some(hidden));
        draw_square(&mut e, 100.0, 0.0, 20.0, Color::WHITE);
        e.doc.edit("Hide", |s| {
            s.update_layer(hidden, |l| l.visible = false);
        });

        e.run(Command::SelectAll);
        assert_eq!(e.selection.len(), 1, "the hidden layer's shape must be skipped");
    }

    #[test]
    fn the_bucket_recolours_the_shape_under_the_cursor() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 60.0, Color::WHITE).unwrap();

        e.style.fill_color = Color::from_rgb8(0xFF, 0x00, 0x00);
        e.apply(ToolAction::BucketFill {
            point: Point::new(30.0, 30.0),
        });

        let (_, object) = e.scene().find_object(id).unwrap();
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("expected a shape")
        };
        assert_eq!(shape.fill.unwrap().color.to_rgba8().to_u8_array()[0], 255);
    }

    #[test]
    fn the_eyedropper_adopts_the_colour_it_samples() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let target = Color::from_rgb8(0x11, 0x22, 0x33);
        draw_square(&mut e, 0.0, 0.0, 60.0, target);

        e.style.fill_color = Color::WHITE;
        e.apply(ToolAction::SampleColor {
            point: Point::new(30.0, 30.0),
        });
        assert_eq!(
            e.style.fill_color.to_rgba8().to_u8_array(),
            target.to_rgba8().to_u8_array()
        );
    }

    #[test]
    fn erasing_cuts_into_a_shape() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE);
        let before = shape_area(&e);

        let mut stroke = BezPath::new();
        stroke.move_to(Point::new(-10.0, 50.0));
        stroke.line_to(Point::new(110.0, 50.0));
        e.apply(ToolAction::Erase {
            path: stroke,
            width: 20.0,
        });

        let after = shape_area(&e);
        assert!(after < before, "erasing should remove area: {after} vs {before}");
    }

    #[test]
    fn zoom_commands_change_magnification_without_limit() {
        let mut e = editor();
        e.run(Command::ZoomActual);
        assert!((e.camera.zoom_percent() - 100.0).abs() < 1e-9);

        for _ in 0..40 {
            e.run(Command::ZoomIn);
        }
        assert!(
            e.camera.zoom_percent() > 1e12,
            "zoom should be unbounded, reached {}",
            e.camera.zoom_percent()
        );
        assert!(e.camera.zoom.is_finite());
    }

    #[test]
    fn view_toggles_flip_their_settings() {
        let mut e = editor();
        let before = e.view.show_grid;
        e.run(Command::ToggleGrid);
        assert_ne!(e.view.show_grid, before);

        let before = e.view.show_rulers;
        e.run(Command::ToggleRulers);
        assert_ne!(e.view.show_rulers, before);
    }

    #[test]
    fn a_document_always_keeps_one_layer() {
        let mut e = editor();
        e.run(Command::DeleteLayer);
        assert_eq!(e.scene().layers().len(), 1);
        assert!(e.status.is_some());
    }

    #[test]
    fn adding_and_deleting_layers_works() {
        let mut e = editor();
        e.run(Command::NewLayer);
        assert_eq!(e.scene().layers().len(), 2);

        e.run(Command::DeleteLayer);
        assert_eq!(e.scene().layers().len(), 1);
        assert!(e.selection.active_layer().is_some());
    }

    #[test]
    fn convert_lines_to_fills_turns_a_stroke_into_a_filled_outline() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.style.stroke_enabled = true;
        e.style.stroke_width = 6.0;

        let mut line = BezPath::new();
        line.move_to(Point::new(0.0, 0.0));
        line.line_to(Point::new(100.0, 0.0));
        let layer = e.selection.active_layer().unwrap();
        let mut id = None;
        e.doc.edit("Draw", |s| {
            id = s.add_shape(layer, ShapeData::stroked(line.clone(), Color::BLACK, 6.0));
        });
        let id = id.unwrap();
        e.selection.select_one(id);

        e.run(Command::ConvertLinesToFills);

        let (_, object) = e.scene().find_object(id).unwrap();
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!()
        };
        assert!(shape.fill.is_some(), "should now be filled");
        assert!(shape.stroke.is_none(), "the stroke should be gone");
        assert!(shape.path.area().abs() > 100.0, "the outline should enclose area");
    }

    #[test]
    fn undo_prunes_a_selection_that_referred_to_deleted_objects() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        e.run(Command::Undo);
        assert!(
            e.selection.is_empty(),
            "the selection must not keep pointing at a removed object"
        );
    }

    #[test]
    fn an_unavailable_tool_is_refused_with_a_message() {
        let mut e = editor();
        e.set_tool(ToolId::Bone);
        assert_ne!(e.tool(), ToolId::Bone);
        assert!(e.status.is_some());
    }

    #[test]
    fn anchors_are_reported_only_for_a_single_selected_shape() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 0.0, 0.0, 50.0, Color::WHITE).unwrap();
        let b = draw_square(&mut e, 100.0, 0.0, 50.0, Color::WHITE).unwrap();

        // Object drawing leaves the new shape selected, so clear it first.
        e.selection.clear();
        assert!(e.selected_anchors().is_empty(), "nothing selected");

        e.selection.select_one(a);
        assert!(!e.selected_anchors().is_empty(), "one shape gives anchors");

        e.selection.set([a, b]);
        assert!(
            e.selected_anchors().is_empty(),
            "Subselection edits one path at a time"
        );
    }

    /// Anchors are reported in world space, so a transformed object's points
    /// still line up with the cursor.
    #[test]
    fn anchors_account_for_the_object_transform() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 50.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        let before = e.selected_anchors();
        e.apply(ToolAction::MoveSelection {
            delta: Vec2::new(100.0, 0.0),
        });
        let after = e.selected_anchors();

        assert_eq!(before.len(), after.len());
        assert!(
            (after[0].point.x - before[0].point.x - 100.0).abs() < 1e-9,
            "anchors should follow the transform"
        );
    }

    #[test]
    fn dragging_an_anchor_reshapes_the_path_and_is_undoable() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 50.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        let before = e.scene().find_object(id).unwrap().1.bounds();
        let anchor = e.selected_anchors()[1];
        e.apply(ToolAction::MoveAnchor {
            element: anchor.element,
            delta: Vec2::new(40.0, 0.0),
        });

        let after = e.scene().find_object(id).unwrap().1.bounds();
        assert!(
            after.width() > before.width(),
            "moving a corner outwards should widen the shape: {before:?} -> {after:?}"
        );

        e.run(Command::Undo);
        let restored = e.scene().find_object(id).unwrap().1.bounds();
        assert!((restored.width() - before.width()).abs() < 1e-9);
    }

    // -- frames, playback and camera ----------------------------------------

    #[test]
    fn frame_operations_extend_and_key_the_active_layer() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();

        e.set_frame(0);
        e.run(Command::InsertFrame);
        assert_eq!(e.scene().layers().get(layer).unwrap().length(), 2);

        e.set_frame(5);
        e.run(Command::InsertKeyframe);
        assert!(e.scene().layers().get(layer).unwrap().frames.is_keyframe(5));
    }

    /// F6 duplicates, F7 does not — the distinction animators rely on.
    #[test]
    fn f6_carries_artwork_forward_and_f7_does_not() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 50.0, Color::WHITE);

        e.set_frame(0);
        e.run(Command::InsertFrame);
        e.set_frame(1);
        e.run(Command::InsertKeyframe);
        assert_eq!(e.scene().shape_count_at(1), 1, "F6 should copy the artwork");

        e.run(Command::InsertFrame);
        e.set_frame(2);
        e.run(Command::InsertBlankKeyframe);
        assert_eq!(e.scene().shape_count_at(2), 0, "F7 should be empty");
    }

    #[test]
    fn drawing_lands_on_the_keyframe_that_owns_the_current_frame() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(9);
            });
        });

        // Drawing on frame 7, inside the span that began at frame 0.
        e.set_frame(7);
        draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE);

        assert_eq!(e.scene().shape_count_at(0), 1, "the edit belongs to frame 0");
        assert_eq!(e.scene().shape_count_at(7), 1);
    }

    /// The playhead must be able to go past the end, or the document could
    /// never be lengthened.
    #[test]
    fn the_playhead_may_move_beyond_the_document_but_not_below_zero() {
        let mut e = editor();
        assert_eq!(e.scene().frame_count(), 1);

        e.set_frame(30);
        assert_eq!(e.current_frame, 30, "clicking an empty frame must work");

        // And F5 there extends the layer to reach it.
        e.run(Command::InsertFrame);
        assert_eq!(e.scene().frame_count(), 31);

        e.step_frame(-1000);
        assert_eq!(e.current_frame, 0, "must not go below zero");
    }

    /// Playback follows wall-clock time, not frames rendered.
    #[test]
    fn playback_advances_at_the_documents_frame_rate() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(23);
            });
        });
        assert_eq!(e.scene().frame_count(), 24);

        e.toggle_playback();
        assert!(e.playback.playing);

        // Half a second at 24 fps is 12 frames.
        e.advance_playback(0.5);
        assert_eq!(e.current_frame, 12, "expected 12 frames in half a second");
    }

    #[test]
    fn playback_loops_by_default_and_can_stop_at_the_end() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(9);
            });
        });

        e.toggle_playback();
        e.advance_playback(1.0); // 24 frames over a 10-frame document
        assert!(e.playback.playing, "looping should keep playing");
        assert!(e.current_frame < 10);

        e.playback.looping = false;
        e.set_frame(0);
        e.advance_playback(1.0);
        assert!(!e.playback.playing, "without looping it should stop");
        assert_eq!(e.current_frame, 9, "and rest on the last frame");
    }

    /// A stall must not make playback spin catching up.
    #[test]
    fn a_long_stall_does_not_cause_a_catch_up_storm() {
        let mut e = editor();
        e.toggle_playback();
        let started = std::time::Instant::now();
        e.advance_playback(10_000.0);
        assert!(
            started.elapsed().as_millis() < 100,
            "advancing after a huge stall took {:?}",
            started.elapsed()
        );
        assert!(e.current_frame < e.scene().frame_count());
    }

    #[test]
    fn onion_frames_surround_the_playhead_and_stay_in_range() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(19);
            });
        });

        assert!(e.onion_frames().is_empty(), "off by default");

        e.onion.enabled = true;
        e.set_frame(10);
        let mut frames = e.onion_frames();
        frames.sort_unstable();
        assert_eq!(frames, vec![8, 9, 11, 12]);

        // At frame 0 there is nothing before it.
        e.set_frame(0);
        let frames = e.onion_frames();
        assert!(frames.iter().all(|f| *f > 0));
    }

    #[test]
    fn onion_skinning_is_suppressed_during_playback() {
        let mut e = editor();
        e.onion.enabled = true;
        e.set_frame(0);
        e.playback.playing = true;
        assert!(
            e.onion_frames().is_empty(),
            "ghosts during playback would be noise"
        );
    }

    #[test]
    fn the_camera_is_off_until_enabled_and_then_has_a_key() {
        let mut e = editor();
        assert!(!e.scene().camera().enabled);
        assert_eq!(
            e.scene().camera_transform(0).as_coeffs(),
            Affine::IDENTITY.as_coeffs()
        );

        e.run(Command::ToggleCamera);
        assert!(e.scene().camera().enabled);
        assert!(
            !e.scene().camera().is_empty(),
            "enabling should seed a key so the camera is not inert"
        );
    }

    /// Dragging the camera right moves the view left, like a real camera.
    #[test]
    fn moving_the_camera_inverts_the_drag() {
        let mut e = editor();
        e.run(Command::ToggleCamera);
        let before = e.scene().camera().state_at(0).unwrap().center;

        e.apply(ToolAction::MoveCamera {
            delta_doc: Vec2::new(50.0, 0.0),
        });

        let after = e.scene().camera().state_at(0).unwrap().center;
        assert!(
            after.x < before.x,
            "dragging right should move the camera left: {before:?} -> {after:?}"
        );
    }

    #[test]
    fn camera_keyframes_interpolate_across_the_timeline() {
        let mut e = editor();
        e.run(Command::ToggleCamera);

        e.set_frame(0);
        e.nudge_camera(Vec2::new(0.0, 0.0));
        e.set_frame(10);
        e.nudge_camera(Vec2::new(-100.0, 0.0));

        let start = e.scene().camera().state_at(0).unwrap().center.x;
        let end = e.scene().camera().state_at(10).unwrap().center.x;
        let mid = e.scene().camera().state_at(5).unwrap().center.x;

        assert!((end - start).abs() > 50.0, "the camera should have moved");
        assert!(
            (mid - (start + end) / 2.0).abs() < 1e-6,
            "frame 5 should be halfway: {start} .. {mid} .. {end}"
        );
    }

    #[test]
    fn camera_edits_are_undoable() {
        let mut e = editor();
        e.run(Command::ToggleCamera);
        let before = e.scene().camera().state_at(0).unwrap().center;

        e.nudge_camera(Vec2::new(80.0, 40.0));
        assert_ne!(e.scene().camera().state_at(0).unwrap().center, before);

        e.run(Command::Undo);
        assert_eq!(e.scene().camera().state_at(0).unwrap().center, before);
    }

    #[test]
    fn resetting_the_camera_clears_its_keys() {
        let mut e = editor();
        e.run(Command::ToggleCamera);
        e.set_frame(5);
        e.run(Command::AddCameraKeyframe);
        assert!(e.scene().camera().keys().len() >= 2);

        e.run(Command::ResetCamera);
        assert!(e.scene().camera().is_empty());
    }

    #[test]
    fn the_camera_tool_is_available_now() {
        let mut e = editor();
        e.set_tool(ToolId::Camera);
        assert_eq!(e.tool(), ToolId::Camera);
    }

    #[test]
    fn frame_operations_are_refused_on_a_locked_layer() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Lock", |s| {
            s.update_layer(layer, |l| l.locked = true);
        });

        e.run(Command::InsertKeyframe);
        assert_eq!(e.scene().layers().get(layer).unwrap().frames.keyframe_count(), 1);
        assert!(e.status.is_some());
    }

    #[test]
    fn snapping_pulls_a_point_onto_a_guide() {
        let mut e = editor();
        e.view.snap.to_guides = true;
        e.view.add_guide(buzz_ui::Guide {
            position: 100.0,
            orientation: buzz_ui::Orientation::Vertical,
        });

        let snapped = e.snap(Point::new(102.0, 40.0));
        assert!((snapped.x - 100.0).abs() < 1e-9, "got {snapped:?}");
    }
}
