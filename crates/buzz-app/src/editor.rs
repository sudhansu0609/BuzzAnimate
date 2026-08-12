//! Editor state and the operations the UI raises against it.
//!
//! Everything the application can do to a document funnels through here, so
//! undo labelling, layer locking and Animate's merge-shape rules live in one
//! place rather than being re-implemented â€” and eventually forgotten â€” in each
//! tool and menu handler.

use std::sync::Arc;

use buzz_doc::Document;
use buzz_geom::{Affine, BezPath, Camera, Point, Rect, Shape as _, Size};
use buzz_scene::{
    FillSpec, LayerId, LayerKind, Object, ObjectId, ObjectKind, Scene, ShapeData, StrokeSpec,
};
use buzz_ui::{Command, DrawStyle, DrawingMode, Selection, ToolId, ViewSettings};
use peniko::Color;

use crate::tools::{Mods, Preview, ToolAction, ToolContext, ToolMachine};

/// How close a click must come to count as hitting a stroke, in screen pixels.
const PICK_TOLERANCE_PX: f64 = 4.0;

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
        let stage = doc.scene().stage.stage_rect();
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
            should_quit: false,
            status: None,
        }
    }

    /// The frame the user is editing.
    pub fn frame(&self) -> u32 {
        self.current_frame
    }

    /// Move the playhead, clamped to the document.
    pub fn set_frame(&mut self, frame: u32) {
        let last = self.doc.scene().frame_count().saturating_sub(1);
        self.current_frame = frame.min(last);
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
        let fps = self.doc.scene().stage.frame_rate.max(0.01);
        self.playback.accumulator += elapsed.clamp(0.0, 0.25);

        let per_frame = 1.0 / fps;
        let mut advanced = 0u32;
        while self.playback.accumulator >= per_frame {
            self.playback.accumulator -= per_frame;
            advanced += 1;
            // A long stall must not spin here catching up indefinitely.
            if advanced > 240 {
                self.playback.accumulator = 0.0;
                break;
            }
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
                if object_contains(object, point, tolerance) {
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
                let stage = self.doc.scene().stage.stage_rect();
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

            SelectTool(tool) => self.set_tool(tool),
        }
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
        let stage = self.doc.scene().stage.stage_rect();
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
fn object_contains(object: &Object, point: Point, tolerance: f64) -> bool {
    // Cheap rejection first.
    if !object.bounds().inflate(tolerance, tolerance).contains(point) {
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
            .any(|c| object_contains(c, local, tolerance)),
    }
}

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
                    ObjectKind::Group(_) => None,
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
