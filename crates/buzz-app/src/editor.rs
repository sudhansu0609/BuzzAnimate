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
    EditAt, FillSpec, LayerId, LayerKind, Object, ObjectId, ObjectKind, Paint, Scene, ShapeData,
    StrokeSpec, Tween,
};
use buzz_ui::{
    ActionsState, Command, DrawStyle, DrawingMode, LibraryState, Selection, ToolId, ViewSettings,
};
use peniko::Color;

use crate::tools::{Mods, Preview, ToolAction, ToolContext, ToolMachine};

/// How close a click must come to count as hitting a stroke, in screen pixels.
const PICK_TOLERANCE_PX: f64 = 4.0;

/// Upper bound on the playhead. Roughly 11 hours at 24 fps — far past anything
/// real, but finite so a stray value cannot produce an absurd timeline.
const MAX_FRAME: u32 = 999_999;

/// The shortest drag that counts as drawing a bone, in screen pixels.
///
/// A click with the Bone tool is far more likely to be a misfire than a
/// request for a zero-length bone — which would be a joint that can never be
/// grabbed again, because there is nothing of it to click.
const MIN_BONE_LENGTH: f64 = 6.0;

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
    /// Set when the interface theme changed and egui has to be restyled.
    ///
    /// The editor does not hold the egui context — the shell does — so a theme
    /// change is recorded here and acted on when the next frame is built.
    pub restyle: bool,
    /// The transformation point for a selection of **several** objects, and
    /// the objects it was set for.
    ///
    /// One object keeps its own point on itself, where it is saved and tweens.
    /// A group of them has nothing to keep it on, so it lives here for the
    /// session — and is ignored the moment the selection is a different set,
    /// which is cheaper and more predictable than clearing it from every place
    /// a selection can change.
    pub group_pivot: Option<(Vec<ObjectId>, Point)>,
    /// **Edit Multiple Frames.** Every keyframe inside the onion markers is
    /// drawn solid instead of ghosted, can be clicked, and is changed together
    /// — which is how a whole scene is shifted across without opening each
    /// drawing in turn. Animate's own mode, and it shares the onion markers
    /// with onion skinning exactly as Animate does.
    pub edit_multiple: bool,
    /// **Auto Keyframe.** With it on, changing artwork at a frame that has no
    /// keyframe of its own makes one first, so the change begins where the
    /// playhead is instead of reaching back to wherever the span started.
    /// Off by default: it is a mode, and a mode that silently adds keyframes
    /// must be one the user asked for.
    pub auto_keyframe: bool,
    /// Library panel state: what is selected, what is open, what is typed in
    /// the search box. View state, so it lives here and not in the document.
    pub library: LibraryState,
    /// Frames on the clipboard, from Cut Frames or Copy Frames.
    ///
    /// View state, not document state: a clipboard that was saved with the
    /// film and came back a week later would be a surprise, and one that
    /// travelled inside a `.buzz` file handed to somebody else would be a
    /// stranger one.
    pub frame_clipboard: Option<Vec<std::sync::Arc<Object>>>,
    /// The New Document dialog: how big, how fast, what colour.
    pub new_document: buzz_ui::NewDocumentState,
    /// Help ▸ About, and the banner it shows once it has been opened.
    pub about: buzz_ui::AboutState,
    /// Reusable artwork kept outside the document, and the panel's own state.
    ///
    /// Scanned from disk at startup rather than watched: a few hundred files
    /// is a millisecond to walk, and a file watcher is a thread and a class of
    /// bug for something a button can refresh.
    pub assets: buzz_doc::AssetLibrary,
    pub assets_panel: buzz_ui::AssetPanelState,
    /// The Swatches panel: which folders are open, what is being renamed,
    /// what is typed in its search box. View state — the palette itself is in
    /// the document.
    pub swatch_panel: buzz_ui::SwatchState,
    /// The Actions panel: the script being written and what the last run said.
    /// View state — a script is not part of the artwork.
    pub actions: ActionsState,
    /// The Export dialog: size, transparency and frame range. View state, and
    /// deliberately re-derived from the document each time it opens.
    pub export: buzz_ui::ExportState,
    /// The fidelity report from the last import, while it is still on screen.
    ///
    /// View state, not document state: dismissing it is not an edit, and it
    /// must not be saved or undone.
    pub import_summary: Option<crate::import::ImportSummary>,
    /// A rigging drag in progress: building a bone, posing one, or moving a
    /// warp handle. Held here rather than in the tool machine because it began
    /// with a question about the document.
    pub rig_gesture: Option<crate::rigging::RigGesture>,
    /// The Lighting panel: which light is being edited, and whether the
    /// on-stage handles are drawn. View state — which light you happen to have
    /// selected is not part of the artwork.
    pub light_panel: buzz_ui::LightPanelState,
    /// The Filters panel: which target and which row. View state.
    pub filter_panel: buzz_ui::FilterPanelState,
    /// The camera row is the selected one in the timeline.
    ///
    /// Animate's camera is shown as a layer and selected like one, but it is
    /// not a layer — so which row is current cannot live in `Selection`, which
    /// addresses layers and objects.
    pub camera_selected: bool,
    /// Where the panels are. View state, saved beside the preferences rather
    /// than in the document: a layout belongs to the person, not to the film.
    pub workspace: buzz_ui::Workspace,
    /// A light handle being dragged, for the same reason as `rig_gesture`: it
    /// began with a question about the document that the tool machine cannot
    /// ask.
    pub light_gesture: Option<crate::lights::LightGesture>,
    /// Decoded sound and the output stream.
    ///
    /// View state: which sounds happen to be decoded is not part of the
    /// document, and the document is the authority on what should be heard.
    pub sound: crate::sound::SoundBank,
    /// The Lip Sync dialog.
    pub lip_sync: buzz_ui::LipSyncState,
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
        let stage_fps = doc.scene().stage().frame_rate;

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
            edit_multiple: false,
            group_pivot: None,
            restyle: false,
            auto_keyframe: false,
            library: LibraryState::default(),
            swatch_panel: buzz_ui::SwatchState::default(),
            frame_clipboard: None,
            new_document: buzz_ui::NewDocumentState::default(),
            about: buzz_ui::AboutState::default(),
            assets: buzz_doc::AssetLibrary::user(),
            assets_panel: buzz_ui::AssetPanelState::default(),
            actions: ActionsState::default(),
            export: buzz_ui::ExportState::default(),
            rig_gesture: None,
            light_panel: buzz_ui::LightPanelState::default(),
            filter_panel: buzz_ui::FilterPanelState::default(),
            camera_selected: false,
            workspace: buzz_ui::Workspace::load(),
            light_gesture: None,
            sound: crate::sound::SoundBank::new(stage_fps),
            lip_sync: buzz_ui::LipSyncState::default(),
            import_summary: None,
            should_quit: false,
            status: None,
        }
    }

    /// The frame the user is editing.
    pub fn frame(&self) -> u32 {
        self.current_frame
    }

    /// The selection's transformation point, in document space.
    ///
    /// Animate's white circle: what a rotation, a skew and an Alt-scale turn
    /// about, and what the 3D rotation of a single object turns about too.
    /// `None` when nothing is selected and there is nothing to turn.
    pub fn pivot(&self) -> Option<Point> {
        let ids = self.selection.ids();
        match ids.as_slice() {
            [] => None,
            [id] => self
                .doc
                .scene()
                .find_object(*id)
                .map(|(_, object)| self.doc.scene().pivot_of(object)),
            many => {
                if let Some((for_ids, at)) = &self.group_pivot
                    && for_ids == many
                {
                    return Some(*at);
                }
                self.selection.bounds(self.doc.scene()).map(|b| b.center())
            }
        }
    }

    /// Put the transformation point at a document-space position.
    ///
    /// On the object when there is one — saved with the document, and it
    /// tweens — and in the editor when several are selected, because a group
    /// of objects has nothing to keep it on.
    pub fn set_pivot(&mut self, at: Point) {
        let ids = self.selection.ids();
        match ids.as_slice() {
            [] => {}
            [id] => {
                let id = *id;
                let frame = self.current_frame;
                self.group_pivot = None;
                self.doc.edit("Transformation Point", |scene| {
                    scene.set_pivot_at(frame, id, at);
                });
            }
            many => self.group_pivot = Some((many.to_vec(), at)),
        }
    }

    /// Put it back at the centre of the artwork — Animate's double-click.
    pub fn reset_pivot(&mut self) {
        let ids = self.selection.ids();
        self.group_pivot = None;
        if ids.is_empty() {
            return;
        }
        let frame = self.current_frame;
        self.doc.edit("Transformation Point", |scene| {
            for id in ids {
                scene.update_object_at(frame, id, |o| o.pivot = None);
            }
        });
    }

    /// Where the next edit lands: the playhead, and the two editing modes.
    pub fn edit_at(&self) -> EditAt {
        EditAt {
            frame: self.current_frame,
            auto_key: self.auto_keyframe,
            span: self.multi_frame_range(),
        }
    }

    /// The frames Edit Multiple Frames covers, or `None` when it is off.
    ///
    /// The onion markers, as in Animate: the mode is "edit what you can see
    /// ghosted", so the two must be the same range or the picture would stop
    /// telling you what an edit is about to touch.
    pub fn multi_frame_range(&self) -> Option<(u32, u32)> {
        if !self.edit_multiple {
            return None;
        }
        let last = self.doc.scene().frame_count().saturating_sub(1);
        Some((
            self.current_frame.saturating_sub(self.onion.before),
            (self.current_frame + self.onion.after).min(last),
        ))
    }

    /// Keyframes to draw solid under Edit Multiple Frames, excluding the one
    /// the playhead is on — which the live frame draws anyway.
    pub fn multi_frames(&self) -> Vec<u32> {
        let Some((first, last)) = self.multi_frame_range() else {
            return Vec::new();
        };
        let mut frames: Vec<u32> = self
            .doc
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.frames.keyframes())
            .map(|k| k.start)
            .filter(|f| *f >= first && *f <= last && *f != self.current_frame)
            .collect();
        frames.sort_unstable();
        frames.dedup();
        frames
    }

    /// Change one object from a panel, on the frame the playhead is inside.
    ///
    /// Panels address an object by id and have no idea which keyframe is
    /// showing, so without this they edit the earliest keyframe holding that
    /// id — which is frame 0's copy of artwork the user is looking at on
    /// frame 12.
    pub fn edit_object(
        &mut self,
        label: &'static str,
        id: ObjectId,
        f: impl FnMut(&mut Object),
    ) {
        let at = self.edit_at();
        self.doc.edit(label, |scene| update_object(scene, at, id, f));
    }

    /// Make a new document with these settings, and remember them.
    ///
    /// The remembering is the point of asking: somebody making a series makes
    /// twenty documents the same size, and the second onwards should be Enter.
    pub fn create_document(&mut self, setup: buzz_ui::DocumentSetup) {
        let setup = setup.sane();

        // The stage is set on the scene *before* the document is built around
        // it, so the size it was asked for is the size it was born at: a new
        // document must not open already dirty, and there is nothing to undo
        // back to.
        let mut scene = buzz_scene::Scene::default();
        {
            let stage = scene.stage_mut();
            stage.size = buzz_geom::Size::new(setup.width, setup.height);
            stage.frame_rate = setup.frame_rate;
            stage.background = peniko::Color::from_rgb8(
                setup.background[0],
                setup.background[1],
                setup.background[2],
            );
        }
        self.doc = Document::new(scene);
        self.doc.mark_clean();

        self.selection = Selection::new();
        self.selection.ensure_active_layer(self.doc.scene());
        self.current_frame = 0;
        self.zoom_fit();

        self.workspace.new_document = setup;
        self.workspace.save();
        self.status = Some(format!(
            "New document \u{2014} {:.0} \u{00D7} {:.0} at {:.0} fps",
            setup.width, setup.height, setup.frame_rate
        ));
    }

    /// Make the document longer or shorter, in frames.
    ///
    /// One undo label, so dragging the number is a single step rather than one
    /// per frame passed through \u2014 the labels coalesce.
    pub fn set_frame_count(&mut self, frames: u32) {
        self.doc.edit("Document Length", |scene| {
            scene.set_frame_count(frames);
        });
        // The playhead may have been left beyond the end.
        let last = self.doc.scene().frame_count().saturating_sub(1);
        if self.current_frame > last {
            self.set_frame(last);
        }
    }

    /// Set the section of the timeline that repeats.
    ///
    /// One undo label for the whole thing, so dragging the range is a single
    /// step rather than one per pixel — the labels coalesce.
    pub fn set_loop_region(&mut self, region: buzz_scene::LoopRegion) {
        let frames = self.doc.scene().frame_count().max(1);
        self.doc.edit("Loop Section", |scene| {
            *scene.looping_mut() = region.clamped(frames);
        });
    }

    /// The next frame during playback, honouring a looping section.
    ///
    /// A document with a loop region cycles inside it rather than running to
    /// the end — the same rule the exporter follows when it repeats those
    /// frames into the film, so what is previewed is what is published.
    fn next_playback_frame(&self, frame: u32) -> u32 {
        let region = *self.doc.scene().looping();
        if let Some(back) = region.wrap(frame) {
            return back;
        }
        frame
    }

    /// Move the playhead.
    ///
    /// The playhead may go **past the end of the document**, because that is
    /// how you extend it: click an empty frame in the timeline and press F5 or
    /// F6. Clamping to the current length would make the document impossible
    /// to lengthen. The bound is only there to stop an absurd value.
    pub fn set_frame(&mut self, frame: u32) {
        self.current_frame = frame.min(MAX_FRAME);
        self.prune_selection();
    }

    /// Drop selected artwork that is not on screen any more.
    ///
    /// Under Edit Multiple Frames "on screen" is the whole range, not the one
    /// frame — otherwise selecting a scene and nudging the playhead would
    /// silently throw most of the selection away.
    fn prune_selection(&mut self) {
        match self.multi_frame_range() {
            Some((first, last)) => {
                let kept: Vec<ObjectId> = self
                    .doc
                    .scene()
                    .objects_across(first, last)
                    .into_iter()
                    .map(|(_, id)| id)
                    .collect();
                self.selection.retain(|id| kept.contains(&id));
            }
            None => self
                .selection
                .prune_to_frame(self.doc.scene(), self.current_frame),
        }
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

        // **The audio clock wins while it is running.** A dropped video frame
        // must not nudge the dialogue: if the picture followed its own clock,
        // every stall would push the two apart a little further, and lip sync
        // drifting out over a long take is the one defect an audience always
        // notices. So when sound is playing the playhead is *told* where the
        // sound has reached.
        if let Some(frame) = self.sound.playing_frame() {
            let count = self.doc.scene().frame_count();
            if frame >= count {
                if self.playback.looping {
                    self.sound.seek(0);
                    self.current_frame = 0;
                } else {
                    self.playback.playing = false;
                    self.sound.stop();
                    self.current_frame = count.saturating_sub(1);
                }
            } else {
                self.current_frame = frame;
            }
            // A looping section cycles even while sound is driving the
            // transport — and the sound is *told* to go back with it, or the
            // dialogue would run on under a repeating picture.
            let looped = self.next_playback_frame(self.current_frame);
            if looped != self.current_frame {
                self.current_frame = looped;
                self.sound.seek(looped);
            }
            self.prune_selection();
            return;
        }

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

        // The looping section is checked first: it ends the run before the end
        // of the timeline, so reaching frame `count` never comes up while one
        // is set inside the document.
        if let Some(back) = self.doc.scene().looping().wrap(next) {
            self.current_frame = back;
            return;
        }

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

        // Sound follows the transport, and does so from the *document's*
        // timeline — so pressing Enter inside a character symbol plays the
        // root dialogue you are animating to, not silence.
        if self.playback.playing {
            let frame = self.current_frame;
            let scene = self.doc.scene().clone();
            self.sound.play(&scene, frame);
        } else {
            self.sound.stop();
        }
    }

    /// Move the sound to wherever the playhead now is.
    ///
    /// Scrubbing counts: dragging the playhead over dialogue should let you
    /// hear roughly where you are, and at minimum must not leave the sound
    /// playing from where it used to be.
    pub fn sync_sound_to_playhead(&mut self) {
        self.sound.seek(self.current_frame);
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
            // Free Transform needs something to put handles on. Picking it up
            // with nothing selected takes the active layer's artwork, so the
            // tool is usable the moment it is chosen — which is what going into
            // a symbol and pressing Q is for.
            if tool == ToolId::FreeTransform {
                self.select_active_layer_contents();
            }
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
            pivot: self.pivot(),
            gradient: self.selected_gradient_handles(),
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

    /// The grips of the selected shape's gradient fill, in world space.
    ///
    /// `None` unless exactly one shape is selected and its fill is a gradient
    /// — which is exactly when the Gradient Transform tool has anything to
    /// grab. Reported in world space for the same reason the anchors are: the
    /// tool compares them against the pointer and should not have to know about
    /// the object's own transform.
    pub fn selected_gradient_handles(
        &self,
    ) -> Option<(buzz_scene::GradientHandles, buzz_scene::GradientKind)> {
        if self.selection.len() != 1 {
            return None;
        }
        let id = self.selection.iter().next()?;
        let (_, object) = self.doc.scene().find_object(id)?;
        let ObjectKind::Shape(shape) = &object.kind else {
            return None;
        };
        let g = shape.fill.as_ref()?.paint.gradient()?;
        let local = g.handles();
        Some((
            buzz_scene::GradientHandles {
                center: object.transform * local.center,
                end: object.transform * local.end,
                width: object.transform * local.width,
                focus: object.transform * local.focus,
            },
            g.kind,
        ))
    }

    /// Apply a Gradient Transform drag to the selected shape.
    ///
    /// The pointer arrives in world space and the gradient lives in the
    /// object's own, so the drag is carried back through the object's inverse
    /// transform. Without that a gradient on a rotated or scaled shape would
    /// jump away from the pointer the moment it was grabbed.
    fn drag_gradient(&mut self, grip: crate::tools::GradientGrip, to: Point) {
        let Some(id) = self.selection.iter().next() else {
            return;
        };
        let Some((_, object)) = self.doc.scene().find_object(id) else {
            return;
        };
        // A shape scaled to nothing has no inverse, and there would be no
        // sensible place to put the grip anyway.
        let det = {
            let c = object.transform.as_coeffs();
            c[0] * c[3] - c[1] * c[2]
        };
        if det.abs() <= f64::MIN_POSITIVE {
            return;
        }
        let local = object.transform.inverse() * to;
        let at = self.edit_at();

        self.doc.edit("Gradient Transform", |scene| {
            update_shape(scene, at, id, |s| {
                let Some(fill) = &mut s.fill else { return };
                let Paint::Gradient(g) = &mut fill.paint else {
                    return;
                };
                // Copy-on-write: the gradient is shared with every snapshot
                // that still references this shape, exactly as the object
                // itself is.
                let g = std::sync::Arc::make_mut(g);
                match grip {
                    crate::tools::GradientGrip::Center => g.set_center(local),
                    crate::tools::GradientGrip::End => g.set_end(local),
                    crate::tools::GradientGrip::Width => g.set_width_handle(local),
                    crate::tools::GradientGrip::Focus => g.set_focus(local),
                }
            });
        });
    }

    pub fn preview(&self) -> Preview {
        self.machine.preview(&self.tool_context())
    }

    /// Document-space tolerance equivalent to a few screen pixels.
    fn pick_tolerance(&self) -> f64 {
        PICK_TOLERANCE_PX / self.camera.zoom.max(f64::MIN_POSITIVE)
    }

    // -- pointer input ------------------------------------------------------

    /// A screen position in the space of whatever is open for editing.
    ///
    /// The two differ only while a symbol is open **in place**: its contents
    /// are drawn through the transform of the instance that was opened, so a
    /// click has to be carried back through the same transform or every tool
    /// would work at an offset — drawing a line beside the pointer, picking
    /// artwork that is not under it.
    pub fn screen_to_edit(&self, screen: Point) -> Point {
        let doc = self.camera.screen_to_doc(screen);
        match buzz_scene::invert_affine(self.doc.scene().edit_place()) {
            Some(back) => back * doc,
            // A collapsed place cannot be undone; the document's own space is
            // the honest fallback.
            None => doc,
        }
    }

    pub fn pointer_down(&mut self, screen: Point, mods: Mods) {
        let doc = self.screen_to_edit(screen);
        let doc = self.snap(doc);

        // Rigging asks what is *under* the pointer before the drag begins —
        // a question the tool machine deliberately cannot answer, because it
        // cannot reach the document. So it is answered here first.
        //
        // **From the raw point, not the snapped one.** Snapping pulls a click
        // towards artwork edges, and a bone lies *inside* its artwork: with
        // snapping on, clicking a bone near the edge of the limb it drives
        // would silently jump the click onto that edge and miss the bone. What
        // is under the pointer is decided by where the pointer is.
        if self.begin_rig_gesture(self.screen_to_edit(screen)) {
            return;
        }

        // A light handle, for the same reason and unsnapped for the same
        // reason: a handle is where it is drawn, and a click that jumped to
        // the nearest artwork edge would miss it.
        if self.begin_light_gesture(self.screen_to_edit(screen)) {
            return;
        }

        let anchors = self.selected_anchors();
        let selection_bounds = self.selection.bounds(self.doc.scene());
        let pivot = self.pivot();
        let zoom = self.camera.zoom;
        let gradient = self.selected_gradient_handles();
        let ctx = ToolContext {
            style: &self.style,
            zoom,
            selection_bounds,
            anchors: &anchors,
            pivot,
            gradient,
        };
        self.machine.pointer_down(doc, screen, mods, &ctx);
    }

    pub fn pointer_move(&mut self, screen: Point, mods: Mods) {
        if self.rig_gesture.is_some() {
            // Unsnapped, for the same reason: an IK target that jumped to the
            // nearest edge would make posing feel like it was fighting back.
            self.update_rig_gesture(self.screen_to_edit(screen));
            return;
        }
        if let Some(gesture) = self.light_gesture {
            let doc = self.screen_to_edit(screen);
            self.doc.edit(gesture.label(), |scene| {
                crate::lights::drag(scene, gesture, doc);
            });
            return;
        }
        let doc = self.snap(self.screen_to_edit(screen));
        let action = self.machine.pointer_move(doc, screen, mods);
        self.apply(action);
    }

    pub fn pointer_up(&mut self, screen: Point) {
        if self.rig_gesture.is_some() {
            self.finish_rig_gesture(self.screen_to_edit(screen));
            self.doc.end_gesture();
            return;
        }
        if let Some(gesture) = self.light_gesture.take() {
            let doc = self.screen_to_edit(screen);
            self.doc.edit(gesture.label(), |scene| {
                crate::lights::drag(scene, gesture, doc);
            });
            // One drag, one undo step — as with every other gesture.
            self.doc.end_gesture();
            return;
        }
        let doc = self.snap(self.screen_to_edit(screen));

        // Built from disjoint fields rather than via `tool_context`, which
        // would borrow all of `self` and conflict with `&mut self.machine`.
        let anchors = self.selected_anchors();
        let selection_bounds = self.selection.bounds(self.doc.scene());
        let pivot = self.pivot();
        let zoom = self.camera.zoom;
        let gradient = self.selected_gradient_handles();
        let ctx = ToolContext {
            style: &self.style,
            zoom,
            selection_bounds,
            anchors: &anchors,
            pivot,
            gradient,
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

                let at = self.edit_at();
                self.doc.edit("Move Anchor", |scene| {
                    update_shape(scene, at, id, |s| {
                        buzz_geom::move_anchor(&mut s.path, element, local_delta);
                    });
                });
            }

            ToolAction::Erase { path, width } => self.erase(path, width),

            ToolAction::BucketFill { point } => {
                let tolerance = self.pick_tolerance();
                let style = self.style.clone();
                if let Some(id) = self.object_at(point, tolerance) {
                    let at = self.edit_at();
                    self.doc.edit("Paint Bucket", |scene| {
                        update_shape(scene, at, id, |s| {
                            // **The bucket is how a gradient reaches artwork
                            // that already exists**, so it fits the ramp to the
                            // shape it is poured into rather than to the shape
                            // the panel last drew. A gradient laid across
                            // somebody else's bounds shows one flat colour, and
                            // reads as the tool having done nothing.
                            let bounds = buzz_geom::Shape::bounding_box(&s.path);
                            if let Some(paint) = style.fill_for_new_shape(bounds) {
                                s.fill = Some(FillSpec {
                                    paint,
                                    rule: buzz_geom::FillMode::NonZero,
                                });
                            }
                        });
                    });
                }
            }

            ToolAction::ApplyStroke { point } => {
                let tolerance = self.pick_tolerance();
                let stroke = self.style.stroke_for_new_shape();
                if let (Some(id), Some((color, width, hairline))) =
                    (self.object_at(point, tolerance), stroke)
                {
                    let at = self.edit_at();
                    self.doc.edit("Ink Bottle", |scene| {
                        update_shape(scene, at, id, |s| {
                            s.stroke = Some(StrokeSpec {
                                paint: Paint::Solid(color),
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
                    // The eyedropper picks up a colour, so a gradient is
                    // sampled to the one colour it stands for rather than
                    // loading the whole ramp into the colour well. Animate's
                    // eyedropper does copy a gradient; ours does not yet, and
                    // it is recorded in PROGRESS.md §7.
                    if let Some(fill) = &shape.fill {
                        self.style.fill_color = fill.color();
                        self.style.fill_enabled = true;
                        self.style.remember(fill.color());
                    } else if let Some(stroke) = &shape.stroke {
                        self.style.stroke_color = stroke.color();
                        self.style.stroke_enabled = true;
                        self.style.remember(stroke.color());
                    }
                }
            }

            ToolAction::PanView { delta_screen } => self.camera.pan_screen(delta_screen),

            ToolAction::MoveCamera { delta_doc } => self.nudge_camera(delta_doc),

            ToolAction::ZoomView { factor, at_screen } => {
                self.camera.zoom_by_at(factor, at_screen);
            }

            ToolAction::Deselect => self.selection.clear(),

            ToolAction::SetTransformPoint { at } => self.set_pivot(at),
            ToolAction::ResetTransformPoint => self.reset_pivot(),
            ToolAction::DragGradient { grip, to } => self.drag_gradient(grip, to),
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
        let auto = self.auto_keyframe;
        let mut created: Option<ObjectId> = None;

        self.doc.edit(label, |scene| {
            // Auto Keyframe applies to *drawing* as much as to changing what is
            // already there: a stroke made on frame 12 of a span belongs to
            // frame 12, not to the keyframe on frame 0 where it would otherwise
            // land and appear from.
            if auto {
                scene.ensure_keyframe(layer, frame);
            }
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
        let at = self.edit_at();
        self.doc.edit(label, |scene| {
            for id in ids {
                update_object(scene, at, id, |o| o.transform = transform * o.transform);
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
            cutter
                .bounding_box()
                .width()
                .hypot(cutter.bounding_box().height()),
        );

        let frame = self.current_frame;
        let at = self.edit_at();
        self.doc.edit("Erase", |scene| {
            let ids: Vec<ObjectId> = scene
                .layers()
                .get(layer)
                .map(|l| l.objects_at(frame).iter().map(|o| o.id).collect())
                .unwrap_or_default();

            for id in ids {
                let mut became_empty = false;
                update_shape(scene, at, id, |s| {
                    s.path =
                        buzz_geom::boolean(&s.path, &cutter, buzz_geom::BoolOp::Difference, opts);
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
    ///
    /// Under Edit Multiple Frames the artwork of other keyframes is on screen
    /// too, so it has to be clickable — a mode that shows you a drawing you
    /// cannot select would be a worse trick than not showing it. The frame the
    /// playhead is on still wins where they overlap.
    pub fn object_at(&self, point: Point, tolerance: f64) -> Option<ObjectId> {
        if let Some(hit) = self.object_at_frame(self.current_frame, point, tolerance) {
            return Some(hit);
        }
        self.multi_frames()
            .into_iter()
            .rev()
            .find_map(|frame| self.object_at_frame(frame, point, tolerance))
    }

    /// Topmost object under `point` on one particular frame.
    fn object_at_frame(&self, frame: u32, point: Point, tolerance: f64) -> Option<ObjectId> {
        let scene = self.doc.scene();
        let mut hit = None;
        // `selectable` yields back to front, so the last match is on top.
        for layer in scene.layers().selectable() {
            // Depth draws a layer's artwork somewhere other than where its
            // geometry says it is, so the click has to be moved the same way
            // in reverse. Without this, a layer pushed into the distance is
            // visible but unclickable — and one pulled forward is selected by
            // clicking empty space beside it.
            let Some(local) = scene.view_to_layer(frame, layer.depth, point) else {
                // At or behind the camera: not drawn, so not selectable.
                continue;
            };
            // Layer parenting moves artwork for the same reason depth does, so
            // the click is moved back the same way.
            let local = match invert(scene.layers().inherited_transform(layer.id, frame)) {
                Some(back) => back * local,
                None => local,
            };
            // The tolerance is a distance, so it shrinks with the layer.
            let local_tolerance = match scene.camera().depth_scale(layer.depth) {
                Some(scale) if scale > 0.0 => tolerance / scale,
                _ => tolerance,
            };

            for object in layer.objects_at(frame) {
                if !object.visible || object.locked {
                    continue;
                }
                // An object turned in space is drawn on a plane of its own, so
                // the click has to be carried back onto that plane before it
                // can be tested against the artwork — the same reverse trip
                // depth and camera tilt already need, one level further in.
                let local = match unturn(scene, object, frame, layer.depth, local) {
                    Some(point) => point,
                    // Edge-on: nothing on screen to click.
                    None => continue,
                };
                if object_contains(scene, object, local, local_tolerance, frame, 0) {
                    hit = Some(object.id);
                }
            }
        }
        hit
    }

    /// The four corners of an object, where they are **drawn**.
    ///
    /// Through the camera *and* the object's own facing, so chrome sits on a
    /// turned object rather than on the rectangle it would occupy if it were
    /// flat. In stage space — the caller adds the view.
    ///
    /// `None` when the object is edge-on or behind the camera, and there is
    /// nothing on screen to put a handle on.
    pub fn object_quad(&self, id: ObjectId) -> Option<[Point; 4]> {
        let scene = self.doc.scene();
        let (layer, object) = scene.find_object(id)?;
        let depth = scene.layers().get(layer).map(|l| l.depth).unwrap_or(0.0);

        let bounds = scene.resolved_bounds(object);
        let pivot = scene.pivot_of(object);
        let projection = scene.camera().projection_for_object(
            self.current_frame,
            scene.stage().size,
            depth,
            pivot,
            &object.spatial,
        )?;

        // Layer parenting moves the artwork as well, and it happens in the
        // plane, before the lens.
        let follows = scene
            .layers()
            .inherited_transform(layer, self.current_frame);
        projection.pre_affine(follows).map_rect(bounds)
    }

    /// Objects fully inside `rect`, matching Animate's marquee.
    ///
    /// Sweeps every frame Edit Multiple Frames is showing, so a marquee round
    /// a whole scene picks up all of it — which is the gesture the mode exists
    /// for.
    pub fn objects_in(&self, rect: Rect) -> Vec<ObjectId> {
        let mut out = self.objects_in_frame(rect, self.current_frame);
        for frame in self.multi_frames() {
            for id in self.objects_in_frame(rect, frame) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// Objects fully inside `rect` on one particular frame.
    fn objects_in_frame(&self, rect: Rect, frame: u32) -> Vec<ObjectId> {
        let scene = self.doc.scene();
        let mut out = Vec::new();
        for layer in scene.layers().selectable() {
            // Marquee against where the artwork is drawn, which for a followed
            // layer is not where its geometry sits.
            let follows = scene.layers().inherited_transform(layer.id, frame);
            for object in layer.objects_at(frame) {
                if !object.visible || object.locked {
                    continue;
                }
                let bounds = buzz_scene::object::transform_rect(follows, object.bounds());
                if rect.contains_rect(bounds) {
                    out.push(object.id);
                }
            }
        }
        out
    }

    // -- commands ------------------------------------------------------------

    pub fn run(&mut self, command: Command) {
        use Command::*;
        match command {
            New => {
                // **Ask, rather than assume.** A document's size and rate are
                // painful to change once artwork exists — every layer and every
                // camera move has to be rescaled — and cost one keypress here.
                // The dialog opens on whatever was chosen last.
                self.new_document.setup = self.workspace.new_document;
                self.new_document.open = true;
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
                // Under Edit Multiple Frames, "all" means every drawing in the
                // range, not only the one showing — Select All then drag is
                // how a whole scene is repositioned.
                let all: Vec<ObjectId> = match self.multi_frame_range() {
                    Some((first, last)) => {
                        let mut ids: Vec<ObjectId> = Vec::new();
                        for (_, id) in self.doc.scene().objects_across(first, last) {
                            if !ids.contains(&id) {
                                ids.push(id);
                            }
                        }
                        ids
                    }
                    None => {
                        let frame = self.current_frame;
                        self.doc
                            .scene()
                            .layers()
                            .selectable()
                            .flat_map(|l| l.objects_at(frame).iter())
                            .filter(|o| o.visible && !o.locked)
                            .map(|o| o.id)
                            .collect()
                    }
                };
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
            RecogniseShape => self.recognise_selection(),
            FlipHorizontal => self.mirror_selection(true),
            FlipVertical => self.mirror_selection(false),
            RotateClockwise => self.turn_selection(std::f64::consts::FRAC_PI_2),
            RotateAnticlockwise => self.turn_selection(-std::f64::consts::FRAC_PI_2),

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
            ClearFrames => self.frame_op(FrameOp::ClearFrames),
            ReverseFrames => self.frame_op(FrameOp::ReverseFrames),
            CopyFrames => self.copy_frames(false),
            CutFrames => self.copy_frames(true),
            PasteFrames => self.paste_frames(),
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
            ToggleAutoKeyframe => self.auto_keyframe = !self.auto_keyframe,
            ToggleEditMultipleFrames => {
                self.edit_multiple = !self.edit_multiple;
                self.prune_selection();
            }

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

            // -- lighting -----------------------------------------------------
            AddSun => self.add_light(buzz_scene::LightKind::sun()),
            AddSky => self.add_light(buzz_scene::LightKind::sky()),
            AddLamp => self.add_light(buzz_scene::LightKind::lamp(self.camera.center)),
            TogglePanel(panel) => {
                self.workspace.toggle(panel);
                self.workspace.save();
            }
            ToggleLayoutLock => {
                self.workspace.locked = !self.workspace.locked;
                self.status = Some(
                    if self.workspace.locked {
                        "Layout locked"
                    } else {
                        "Layout unlocked"
                    }
                    .into(),
                );
                self.workspace.save();
            }
            ToggleTheme => {
                let next = buzz_ui::theme::theme().other();
                buzz_ui::theme::set_theme(next);
                self.workspace.theme = next;
                self.workspace.save();
                // The context is restyled by the shell, which owns it; this
                // records what the chrome should now be.
                self.restyle = true;
                self.status = Some(format!("{} interface", next.label()));
            }
            About => self.about.open = true,
            ResetWorkspace => {
                self.workspace = buzz_ui::Workspace::animate();
                self.workspace.save();
                self.status = Some("Layout reset".into());
            }

            ToggleLightGizmos => {
                self.light_panel.gizmos = !self.light_panel.gizmos;
            }
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

            ExportImage | ExportSequence => {
                // The shell owns the file dialog and the exporting thread, as
                // with Open and Save. Reaching here means a code path raised
                // the command without going through `App::dispatch`.
                debug_assert!(false, "{command:?} must be dispatched by the shell");
            }

            ImportToLibrary | ImportToStage => {
                // Handled by the shell, which owns the file dialog, as with
                // Open and Save. Reaching here means a code path raised the
                // command without going through `App::dispatch`.
                debug_assert!(false, "{command:?} must be dispatched by the shell");
            }

            ImportSound | LipSync => {
                // The shell owns the file dialog and the modal window, as with
                // Open and Export.
                debug_assert!(false, "{command:?} must be dispatched by the shell");
            }
            AttachSound => self.attach_sound_to_frame(),
            RemoveSound => self.remove_sound_from_frame(),
            NewMouthSymbol => {
                let symbol = self.new_mouth_symbol();
                self.lip_sync.mouth = Some(symbol.0);
            }

            ToggleActionsPanel => {
                self.workspace.toggle(buzz_ui::PanelId::Actions);
                self.workspace.save();
            }
            RunScript => {
                // Running from the menu or the keyboard while the panel is
                // closed would put the output somewhere the user cannot see it.
                if !self.workspace.is_open(buzz_ui::PanelId::Actions) {
                    self.workspace.toggle(buzz_ui::PanelId::Actions);
                }
                self.run_script();
            }
            ClearScriptOutput => self.actions.clear_output(),

            SelectTool(tool) => self.set_tool(tool),
        }
    }

    // -- lights ---------------------------------------------------------------

    /// Add a light, select it, and say so.
    ///
    /// Selecting it is the point: the Lighting panel then shows the new
    /// light's own settings rather than whichever one was there before.
    ///
    /// A lamp arrives in the middle of the view whatever position the request
    /// carried. A lamp is the one light with a place on the stage, and one
    /// dropped off-screen — at the origin, say, which is the top-left corner of
    /// the artwork — looks exactly like nothing having happened.
    pub fn add_light(&mut self, kind: buzz_scene::LightKind) {
        let kind = match kind {
            buzz_scene::LightKind::Lamp { height, radius, .. } => buzz_scene::LightKind::Lamp {
                position: self.camera.center,
                height,
                radius,
            },
            other => other,
        };

        let label = kind.label();
        let mut added = None;
        self.doc.edit(format!("Add {label}"), |scene| {
            added = Some(scene.add_light(kind));
        });
        self.light_panel.selected = added;
        // The handles come back on with a new light: adding one you cannot see
        // and cannot grab would look like nothing happened.
        self.light_panel.gizmos = true;
        self.status = Some(format!("Added a {}", label.to_lowercase()));
    }

    /// Start a light drag, if a handle is under the pointer.
    ///
    /// Only with the Selection tool. On-stage handles belong to Selection the
    /// way transform handles do; a lamp sitting over the canvas must not
    /// swallow a brush stroke aimed at the artwork beneath it.
    fn begin_light_gesture(&mut self, doc: Point) -> bool {
        // The same three conditions the stage draws under, so what can be
        // grabbed is exactly what can be seen.
        if !self.light_panel.gizmos
            || !self.doc.scene().lights().enabled
            || self.tool() != ToolId::Selection
        {
            return false;
        }

        let tolerance = crate::lights::GRAB_PX / self.camera.zoom.max(f64::MIN_POSITIVE);
        let Some(gesture) = crate::lights::target_at(self.doc.scene(), doc, tolerance) else {
            return false;
        };

        // Grabbing a light selects it, so the panel is already showing the one
        // being dragged by the time the drag ends.
        self.light_panel.selected = Some(gesture.light());
        self.light_gesture = Some(gesture);
        tracing::debug!(?doc, ?gesture, "light gesture");
        true
    }

    // -- rigging -------------------------------------------------------------

    /// Start a rigging drag, if the active tool and what is under the pointer
    /// call for one. Returns whether the gesture was taken.
    fn begin_rig_gesture(&mut self, doc: Point) -> bool {
        use crate::rigging::{RigGesture, RigTarget};

        let tool = self.tool();
        if !matches!(tool, ToolId::Bone | ToolId::AssetWarp) {
            return false;
        }

        let tolerance = crate::rigging::GRAB_PX / self.camera.zoom.max(f64::MIN_POSITIVE);
        let target =
            crate::rigging::target_at(self.doc.scene(), self.current_frame, doc, tolerance);
        // Rigging is the one gesture whose outcome depends on what was under
        // the pointer, so what it found is worth being able to see.
        tracing::debug!(?tool, ?doc, tolerance, ?target, "rig gesture");

        self.rig_gesture = match (tool, target) {
            // -- the Bone tool ---------------------------------------------
            (ToolId::Bone, RigTarget::BoneTip(object, bone)) => Some(RigGesture::Building {
                object: Some(object),
                parent: Some(bone),
                head: doc,
                current: doc,
            }),
            (ToolId::Bone, RigTarget::Bone(object, bone)) => {
                self.selection.set([object]);
                Some(RigGesture::Posing {
                    object,
                    bone,
                    current: doc,
                })
            }
            (ToolId::Bone, RigTarget::Artwork(object)) => Some(RigGesture::Building {
                object: Some(object),
                parent: None,
                head: doc,
                current: doc,
            }),
            (ToolId::Bone, RigTarget::Handle(..) | RigTarget::Nothing) => {
                self.status = Some("Draw a bone across some artwork to rig it".into());
                None
            }

            // -- the Asset Warp tool ---------------------------------------
            (ToolId::AssetWarp, RigTarget::Handle(object, handle)) => Some(RigGesture::Warping {
                object,
                handle,
                current: doc,
            }),
            (ToolId::AssetWarp, RigTarget::Artwork(object)) => {
                // Animate turns the artwork into a warp object the moment you
                // touch it with the tool, and puts a starting grid on it — a
                // tool that needed handles placed one at a time before doing
                // anything would look broken.
                let mut warped = false;
                let frame = self.current_frame;
                self.doc.edit("Add Warp Handles", |scene| {
                    warped = crate::rigging::warp_object(scene, frame, object, 3, 3);
                });
                self.status = Some(if warped {
                    "Drag a handle to warp the artwork".into()
                } else {
                    "Only a single shape can be warped; ungroup it first".to_string()
                });
                if warped {
                    self.selection.set([object]);
                }
                None
            }
            (ToolId::AssetWarp, _) => None,
            _ => None,
        };

        self.rig_gesture.is_some()
    }

    /// Follow the pointer during a rigging drag.
    ///
    /// Posing and warping are applied live, so the user sees the rig follow
    /// their hand. Both are `Document::edit`s with a stable label, so the
    /// hundreds of moves in one drag coalesce into a single undo step — the
    /// same mechanism that makes dragging a shape one Ctrl+Z.
    fn update_rig_gesture(&mut self, doc: Point) {
        use crate::rigging::RigGesture;

        match self.rig_gesture.clone() {
            Some(RigGesture::Building {
                object,
                parent,
                head,
                ..
            }) => {
                self.rig_gesture = Some(RigGesture::Building {
                    object,
                    parent,
                    head,
                    current: doc,
                });
            }
            Some(RigGesture::Posing { object, bone, .. }) => {
                let frame = self.current_frame;
                self.doc.edit("Pose", |scene| {
                    crate::rigging::pose_bone(scene, frame, object, bone, doc);
                });
                self.rig_gesture = Some(RigGesture::Posing {
                    object,
                    bone,
                    current: doc,
                });
            }
            Some(RigGesture::Warping { object, handle, .. }) => {
                let frame = self.current_frame;
                self.doc.edit("Warp", |scene| {
                    crate::rigging::move_handle(scene, frame, object, handle, doc);
                });
                self.rig_gesture = Some(RigGesture::Warping {
                    object,
                    handle,
                    current: doc,
                });
            }
            None => {}
        }
    }

    /// Commit a rigging drag.
    fn finish_rig_gesture(&mut self, doc: Point) {
        use crate::rigging::RigGesture;

        let Some(gesture) = self.rig_gesture.take() else {
            return;
        };

        if let RigGesture::Building {
            object,
            parent,
            head,
            ..
        } = gesture
        {
            // A click rather than a drag: too short to be a bone, and a
            // zero-length bone is a joint that can never be found again.
            if (doc - head).hypot() < MIN_BONE_LENGTH / self.camera.zoom.max(f64::MIN_POSITIVE) {
                self.status = Some("Drag to set the bone's length".into());
                return;
            }
            let Some(object) = object else { return };

            let is_new_rig = parent.is_none();
            let mut rigged = false;
            let frame = self.current_frame;
            self.doc.edit(
                if is_new_rig {
                    "Create Armature"
                } else {
                    "Add Bone"
                },
                |scene| {
                    rigged = if is_new_rig {
                        crate::rigging::rig_object(scene, frame, object, head, doc)
                    } else {
                        crate::rigging::add_bone(scene, frame, object, parent, head, doc);
                        true
                    };
                },
            );

            if rigged {
                self.selection.set([object]);
                self.status = Some(if is_new_rig {
                    "Armature created — drag from the bone's tip to add another".into()
                } else {
                    "Bone added".to_string()
                });
            } else {
                self.status = Some("That artwork cannot be rigged".into());
            }
        }
    }

    /// The rig being dragged out, for the stage to draw as a preview.
    pub fn rig_preview(&self) -> Option<(Point, Point)> {
        match &self.rig_gesture {
            Some(crate::rigging::RigGesture::Building { head, current, .. }) => {
                Some((*head, *current))
            }
            _ => None,
        }
    }

    // -- sound ---------------------------------------------------------------

    /// Bring a sound file into the library.
    pub fn import_sound(&mut self, path: &std::path::Path) -> anyhow::Result<String> {
        // Decoded once here to learn its shape and to fail *before* the
        // document is touched: a file that cannot be decoded should not leave
        // an entry in the library that plays nothing.
        let clip = buzz_audio::Clip::open(path)?;
        let bytes = std::sync::Arc::new(std::fs::read(path)?);
        let format = path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "wav".to_string());
        let name = clip.name.clone();
        let (rate, channels, length) = (clip.sample_rate, clip.channels, clip.len() as u64);

        let mut imported = None;
        self.doc.edit("Import Sound", |scene| {
            imported = Some(scene.add_sound(&name, bytes, &format, rate, channels, length));
        });
        self.doc.end_gesture();

        let scene = self.doc.scene().clone();
        self.sound.refresh(&scene);

        Ok(imported
            .and_then(|id| scene.sounds().get(id).map(|s| s.name.clone()))
            .unwrap_or(name))
    }

    /// Put the most recently imported sound on the current keyframe.
    ///
    /// Animate attaches a sound to the keyframe the playhead is on, chosen
    /// from the Properties panel. There is no sound picker yet (PROGRESS §7),
    /// so the newest import is used — which is the one an animator has just
    /// brought in.
    fn attach_sound_to_frame(&mut self) {
        let Some(layer) = self.selection.active_layer() else {
            self.status = Some("Select a layer to put the sound on".into());
            return;
        };
        let Some(sound) = self.doc.scene().sounds().iter().last().map(|s| s.id) else {
            self.status = Some("Import a sound first: File > Import Sound".into());
            return;
        };

        let frame = self.current_frame;
        let mut attached = false;
        self.doc.edit("Attach Sound", |scene| {
            attached =
                scene.set_frame_sound(layer, frame, Some(buzz_scene::SoundRef::stream(sound)));
        });
        self.doc.end_gesture();

        self.status = Some(if attached {
            "Sound attached - press Enter to play".into()
        } else {
            "That frame is not a keyframe; press F6 first".to_string()
        });
        let scene = self.doc.scene().clone();
        self.sound.refresh(&scene);
    }

    fn remove_sound_from_frame(&mut self) {
        let Some(layer) = self.selection.active_layer() else {
            return;
        };
        let frame = self.current_frame;
        self.doc.edit("Remove Sound", |scene| {
            scene.set_frame_sound(layer, frame, None);
        });
        self.doc.end_gesture();
        let scene = self.doc.scene().clone();
        self.sound.refresh(&scene);
        self.status = Some("Sound removed from the frame".into());
    }

    /// Waveforms for the timeline, one per layer carrying a sound.
    pub fn waveforms(&self) -> std::collections::BTreeMap<LayerId, buzz_ui::Waveform> {
        let fps = self.doc.scene().stage().frame_rate;
        let mut out = std::collections::BTreeMap::new();

        for layer in self.doc.scene().stage_layers().iter() {
            for keyframe in layer.frames.keyframes() {
                let Some(reference) = keyframe.sound else {
                    continue;
                };
                let Some(clip) = self.sound.clip(reference.sound) else {
                    continue;
                };
                out.insert(
                    layer.id,
                    buzz_ui::Waveform {
                        start_frame: keyframe.start,
                        levels: clip.frame_levels(fps),
                    },
                );
            }
        }
        out
    }

    /// Make a mouth symbol with a frame per shape.
    pub fn new_mouth_symbol(&mut self) -> buzz_scene::SymbolId {
        let mut made = None;
        self.doc.edit("New Mouth Symbol", |scene| {
            made = Some(crate::lipsync::placeholder_mouth(scene, "Mouth"));
        });
        self.doc.end_gesture();
        self.status = Some("Made a mouth symbol - draw each shape on its own frame".into());
        made.expect("the symbol was made inside the edit")
    }

    /// What the Lip Sync dialog should offer: the soundtrack, the mouth
    /// symbols, and the layers.
    pub fn lip_sync_choices(&self) -> (Option<String>, Vec<buzz_ui::Choice>, Vec<buzz_ui::Choice>) {
        let scene = self.doc.scene();

        // The soundtrack comes from the document's own timeline, whichever
        // symbol is open — which is the whole point, and is why it is named in
        // the dialog rather than left implicit.
        let track = self.sound.stage_track(scene).map(|(id, start, clip)| {
            let name = scene
                .sounds()
                .get(id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| clip.name.clone());
            format!(
                "{name} - {:.1}s, from frame {start}",
                clip.duration_seconds()
            )
        });

        let needed = buzz_audio::Viseme::COUNT;
        let mouths = scene
            .library()
            .iter()
            .map(|symbol| {
                let length = symbol.length();
                buzz_ui::Choice {
                    id: symbol.id.0,
                    name: symbol.name.clone(),
                    detail: if length >= needed {
                        format!("{length} frames")
                    } else {
                        format!("{length} frames, needs {needed}")
                    },
                    usable: length >= needed,
                }
            })
            .collect();

        // The layers offered are those of the timeline being *edited*: the
        // mouth goes where you are working, which may be several symbols deep.
        let layers = scene
            .layers()
            .iter()
            .map(|layer| buzz_ui::Choice {
                id: layer.id.0,
                name: layer.name.clone(),
                detail: String::new(),
                usable: layer.kind.holds_artwork(),
            })
            .collect();

        (track, mouths, layers)
    }

    /// Run lip sync with whatever the dialog has been set to.
    pub fn run_lip_sync(&mut self) {
        let scene = self.doc.scene().clone();
        let Some((_, start, clip)) = self.sound.stage_track(&scene) else {
            self.lip_sync.result = Some("There is no sound on the main timeline".into());
            return;
        };
        let (Some(mouth), Some(layer)) = (self.lip_sync.mouth, self.lip_sync.layer) else {
            self.lip_sync.result = Some("Choose a mouth symbol and a layer".into());
            return;
        };

        let options = buzz_audio::LipSyncOptions {
            silence: self.lip_sync.silence,
            hold: self.lip_sync.hold,
        };

        // The mouth is placed where the layer's existing artwork is, and only
        // falls back to the middle of the stage when there is none. A mouth
        // landing at the origin, off the character's face, would be the first
        // thing to fix by hand every single time.
        let placement = self
            .doc
            .scene()
            .layers()
            .get(buzz_scene::LayerId(layer))
            .and_then(|l| l.bounds_at(self.current_frame))
            .map(|b| Affine::translate(b.center().to_vec2()))
            .unwrap_or_else(|| {
                let stage = self.doc.scene().stage().stage_rect();
                Affine::translate(stage.center().to_vec2())
            });

        let mut outcome = None;
        self.doc.edit("Lip Sync", |scene| {
            outcome = Some(crate::lipsync::apply(
                scene,
                &clip,
                start,
                buzz_scene::LayerId(layer),
                buzz_scene::SymbolId(mouth),
                placement,
                &options,
            ));
        });
        self.doc.end_gesture();

        match outcome {
            Some(Ok(report)) => {
                self.lip_sync.result = Some(report.message.clone());
                self.status = Some(report.message);
            }
            Some(Err(e)) => {
                self.lip_sync.result = Some(e.to_string());
                self.status = Some(e.to_string());
            }
            None => {}
        }
    }

    // -- scripting -----------------------------------------------------------

    /// Run what is in the Actions panel against this document.
    ///
    /// # One run is one undo step
    ///
    /// The script works on a *clone* of the scene and the result is committed
    /// in a single [`Document::edit`], so a script that draws four hundred
    /// rectangles is one Ctrl+Z. `end_gesture` follows it, because two runs in
    /// quick succession share an undo label and would otherwise coalesce into
    /// one step — for a drag that is the point, but two runs of a script are
    /// two deliberate acts.
    ///
    /// # A failed script keeps what it managed
    ///
    /// Whatever the script did before it failed is committed too, and the error
    /// is reported alongside. Discarding an hour of generated artwork because
    /// the last line had a typo would be indefensible, and Animate does not do
    /// it either.
    ///
    /// # It edits what the user is looking at
    ///
    /// The scene answers `layers()` for the timeline currently open, so a
    /// script run inside a symbol edits that symbol. That is the same rule
    /// every tool and panel follows, and the breadcrumb above the stage is what
    /// says which one is open.
    pub fn run_script(&mut self) {
        if !self.actions.has_source() {
            self.status = Some("Write a script in the Actions panel first".into());
            return;
        }

        let source = self.actions.source.clone();
        let context = buzz_script::ScriptContext {
            current_frame: self.current_frame,
            selection: self.selection.ids(),
            active_layer: self.selection.active_layer(),
        };

        let mut working = self.doc.scene().clone();
        let outcome = buzz_script::run(
            &mut working,
            context,
            &source,
            &buzz_script::Limits::default(),
        );

        if outcome.changed {
            self.doc.edit("Run Script", |scene| *scene = working);
            self.doc.end_gesture();
        }

        let summary = outcome.summary();

        // The script's view of the editor becomes the editor's, so
        // `t.currentFrame = 5` moves the playhead the user can see and
        // `d.selectAll()` leaves the artwork actually selected.
        self.set_frame(outcome.context.current_frame);
        self.selection.set(outcome.context.selection);
        self.selection.prune(self.doc.scene());
        if let Some(layer) = outcome.context.active_layer {
            self.selection.set_active_layer(Some(layer));
        }
        self.selection.ensure_active_layer(self.doc.scene());

        self.status = Some(summary.clone());
        self.actions.report(outcome.trace, outcome.error, summary);
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

        // **Opening a symbol selects what is in it.** A symbol is usually one
        // drawing on one layer, and the reason for going in is almost always to
        // move, scale or turn that drawing. Arriving with nothing selected — and
        // no layer lit in the timeline — means a click before any work can
        // start, every time. A deviation from Animate, recorded in §7.
        if let Some(layer) = self.selection.active_layer() {
            self.selection
                .select_layer(self.doc.scene(), layer, self.current_frame);
        }
    }

    /// Make `layer` active and select the artwork on it.
    ///
    /// Every route to "the user clicked a layer" goes through here — the Layers
    /// panel, the timeline's layer column and the Layer Depth view — so all
    /// three agree about what a click does.
    pub fn select_layer(&mut self, layer: LayerId) {
        self.selection
            .select_layer(self.doc.scene(), layer, self.current_frame);
    }

    /// Give a transform tool something to work on.
    ///
    /// Free Transform with nothing selected has nothing to draw handles round,
    /// so reaching for it is always followed by a click on the artwork. Where
    /// the active layer has artwork and nothing is selected, that click is
    /// foregone: the layer's contents are selected, which is what the user was
    /// about to do. It only ever *adds* a selection — an existing one is left
    /// exactly as it is, so this can never take a chosen object away.
    fn select_active_layer_contents(&mut self) {
        if !self.selection.is_empty() {
            return;
        }
        self.selection
            .select_active_layer_contents(self.doc.scene(), self.current_frame);
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

    /// **Animate's double-click on the stage**: go into the instance under the
    /// pointer, or come back out if there is nothing there.
    ///
    /// This is how a nested character is opened — a head inside a body inside
    /// a scene — and without it the only way in was the Library, one symbol at
    /// a time, with no way to tell which of three like-named heads was the one
    /// on screen.
    pub fn enter_or_leave_at(&mut self, screen: Point) {
        let point = self.screen_to_edit(screen);
        let tolerance = self.pick_tolerance();

        // **Where it sits, as well as which symbol it is.** The instance's own
        // matrix and its layer's parenting together say where the symbol's
        // space lands on the timeline that is open now; opening it in place
        // means drawing its contents through exactly that.
        let frame = self.current_frame;
        let opened = self
            .object_at(point, tolerance)
            .and_then(|id| self.doc.scene().find_object(id))
            .and_then(|(layer, object)| {
                let instance = object.instance()?;
                let follows = self.doc.scene().layers().inherited_transform(layer, frame);
                Some((instance.symbol, follows * object.transform))
            });

        match opened {
            Some((id, place)) => {
                let mut entered = false;
                self.doc
                    .edit_view(|scene| entered = scene.enter_symbol_in_place(id, place));
                if entered {
                    self.library.selected = Some(id);
                    self.after_context_change();
                    let name = self
                        .doc
                        .scene()
                        .library()
                        .get(id)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    self.status = Some(format!("Editing {name}"));
                }
            }
            None => {
                // Nothing under the pointer: out one level, as Animate does.
                let mut left = false;
                self.doc.edit_view(|scene| left = scene.exit_symbol());
                if left {
                    self.library.selected = self.doc.scene().editing_symbol();
                    self.after_context_change();
                }
            }
        }
    }

    /// Open a symbol: the selected instance's symbol if one is selected,
    /// otherwise the library selection. Animate's Ctrl+E does both.
    ///
    /// **The selection leads.** The Library keeps its highlight for as long as
    /// the panel is open, so preferring it meant Ctrl+E on a chosen instance
    /// opened whatever was last clicked in the Library instead — which, in a
    /// document with three hundred symbols, is almost never the right one.
    fn edit_selected_symbol(&mut self) {
        let from_instance = self
            .selection
            .iter()
            .next()
            .and_then(|id| self.doc.scene().find_object(id))
            .and_then(|(_, o)| o.instance())
            .map(|i| i.symbol);

        let Some(id) = from_instance.or(self.library.selected) else {
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
            let new_id =
                scene.add_symbol(source.name.clone(), source.kind, source.folder.as_deref());
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
        let uses = self
            .doc
            .scene()
            .symbol_usage()
            .get(&id)
            .copied()
            .unwrap_or(0);

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
                    FrameOp::ClearFrames => l.frames.clear_frames(frame),
                    FrameOp::ReverseFrames => l.frames.reverse_frames(),
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

    /// **Copy Frames**, and **Cut Frames** when `and_clear`.
    ///
    /// The artwork of the keyframe the playhead is inside, taken as it is.
    /// Animate copies a *selected span* of frames; there is no span selection
    /// here yet, so this is the frame you are looking at — recorded in §7.
    fn copy_frames(&mut self, and_clear: bool) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer to copy from".into());
            return;
        };
        let frame = self.current_frame;
        let contents = self
            .doc
            .scene()
            .layers()
            .get(layer)
            .and_then(|l| l.frames.frame_contents(frame));

        let Some(contents) = contents else {
            self.status = Some("There is no frame here to copy".into());
            return;
        };
        let count = contents.len();
        self.frame_clipboard = Some(contents);

        if and_clear {
            self.frame_op(FrameOp::ClearFrames);
        }
        self.status = Some(format!(
            "{} {count} object{} from frame {}",
            if and_clear { "Cut" } else { "Copied" },
            if count == 1 { "" } else { "s" },
            frame + 1
        ));
    }

    /// **Paste Frames** onto the frame the playhead is on.
    ///
    /// Makes a keyframe there first, as Animate does: pasting into the middle
    /// of a span would otherwise change the artwork from wherever that span
    /// began. The objects are given fresh ids, so pasting twice gives two
    /// drawings rather than one shared between two frames.
    fn paste_frames(&mut self) {
        let Some(contents) = self.frame_clipboard.clone() else {
            self.status = Some("There are no frames on the clipboard".into());
            return;
        };
        let Some(layer) = self.active_layer() else {
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }

        let frame = self.current_frame;
        let count = contents.len();
        self.doc.edit("Paste Frames", |scene| {
            scene.update_layer(layer, |l| {
                l.frames.insert_frame(frame);
                l.frames.insert_blank_keyframe(frame);
            });
            for object in &contents {
                let mut copy = (**object).clone();
                copy.id = scene.next_object_id();
                scene.add_object_at(layer, frame, copy);
            }
        });
        self.status = Some(format!(
            "Pasted {count} object{} onto frame {}",
            if count == 1 { "" } else { "s" },
            frame + 1
        ));
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
                scene
                    .camera_mut()
                    .set_key(buzz_scene::CameraKey::new(frame, centre));
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
            let key = current
                .map(|s| buzz_scene::CameraKey { frame, ..s })
                .unwrap_or(buzz_scene::CameraKey::new(frame, centre));
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
        self.selection
            .ensure_active_layer(&self.doc.scene().clone());
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
        let at = self.edit_at();
        self.doc.edit("Convert Lines to Fills", |scene| {
            for id in ids {
                update_shape(scene, at, id, |s| {
                    let Some(stroke) = s.stroke.clone() else { return };
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
                    // The outline keeps the stroke's paint, gradient and all:
                    // Convert Lines to Fills turns a line into the shape of
                    // that line, and a gradient-stroked line becomes a
                    // gradient-filled outline of itself.
                    s.fill = Some(FillSpec {
                        paint: stroke.paint.clone(),
                        rule: buzz_geom::FillMode::NonZero,
                    });
                    s.stroke = None;
                });
            }
        });
    }

    fn expand_selection(&mut self, amount: f64) {
        let ids = self.selection.ids();
        let at = self.edit_at();
        self.doc.edit("Expand Fill", |scene| {
            for id in ids {
                update_shape(scene, at, id, |s| {
                    let bb = s.path.bounding_box();
                    let opts =
                        buzz_geom::BooleanOptions::for_shape_size(bb.width().hypot(bb.height()));
                    s.path = buzz_geom::expand_fill(&s.path, amount, opts);
                });
            }
        });
    }

    /// Modify ▸ Transform ▸ Flip.
    ///
    /// **About the selection's centre, not each object's.** Flipping two
    /// objects together swaps their places as well as mirroring each one,
    /// which is what Animate does and what anybody flipping a pair of ears
    /// means. Object by object would mirror each in place and leave them the
    /// wrong way round.
    fn mirror_selection(&mut self, horizontal: bool) {
        // About the **transformation point**, which is the selection's centre
        // until somebody moves it — so this is unchanged for anyone who never
        // touches the circle, and hinges where they put it for anyone who does.
        let Some(pivot) = self.pivot() else {
            self.status = Some("Nothing selected".into());
            return;
        };
        let c = pivot.to_vec2();
        let scale = if horizontal {
            Affine::scale_non_uniform(-1.0, 1.0)
        } else {
            Affine::scale_non_uniform(1.0, -1.0)
        };
        let label = if horizontal {
            "Flip Horizontal"
        } else {
            "Flip Vertical"
        };
        self.transform_selection(
            Affine::translate(c) * scale * Affine::translate(-c),
            label,
        );
    }

    /// Modify ▸ Transform ▸ Rotate 90°, about the selection's centre.
    fn turn_selection(&mut self, angle: f64) {
        let Some(pivot) = self.pivot() else {
            self.status = Some("Nothing selected".into());
            return;
        };
        let c = pivot.to_vec2();
        self.transform_selection(
            Affine::translate(c) * Affine::rotate(angle) * Affine::translate(-c),
            "Rotate",
        );
    }

    /// **Shape recognition** — Animate's, on the selection.
    ///
    /// A roughly circular scribble becomes a circle, four rough strokes become
    /// a rectangle, a shaky stroke becomes a straight line. Anything that is
    /// not a shape is left exactly as it was: replacing a drawing with
    /// something the animator did not draw is worse than doing nothing, so
    /// this reports what it did rather than changing artwork silently.
    fn recognise_selection(&mut self) {
        let ids = self.selection.ids();
        if ids.is_empty() {
            self.status = Some("Nothing selected".into());
            return;
        }
        let at = self.edit_at();
        let tolerance = self.view.shape_tolerance;
        let mut found: Vec<buzz_geom::Recognised> = Vec::new();

        self.doc.edit("Recognise Shape", |scene| {
            for id in ids {
                update_shape(scene, at, id, |s| {
                    if let Some((path, kind)) = buzz_geom::recognise(&s.path, tolerance) {
                        s.path = path;
                        found.push(kind);
                    }
                });
            }
        });

        self.status = Some(match found.as_slice() {
            [] => "Nothing here is a recognisable shape".to_string(),
            [one] => format!("Recognised {}", one.label()),
            many => format!("Recognised {} shapes", many.len()),
        });
    }

    fn reshape_selection(&mut self, how: Reshape) {
        let ids = self.selection.ids();
        let at = self.edit_at();
        let label = match how {
            Reshape::Smooth => "Smooth",
            Reshape::Straighten => "Straighten",
        };
        // **Straighten recognises shapes first**, as Animate's does: it is the
        // command an animator reaches for after drawing a rough circle, and
        // easing the curve of a circle that could have *been* a circle is a
        // worse answer than the one they wanted. Nothing recognisable falls
        // through to the ordinary straightening.
        let recognise = matches!(how, Reshape::Straighten);
        let tolerance = self.view.shape_tolerance;

        self.doc.edit(label, |scene| {
            for id in ids {
                update_shape(scene, at, id, |s| {
                    if recognise
                        && let Some((path, _)) = buzz_geom::recognise(&s.path, tolerance)
                    {
                        s.path = path;
                        return;
                    }
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
    ClearFrames,
    ReverseFrames,
}

impl FrameOp {
    fn label(self) -> &'static str {
        match self {
            Self::InsertFrame => "Insert Frame",
            Self::RemoveFrame => "Remove Frame",
            Self::InsertKeyframe => "Insert Keyframe",
            Self::InsertBlankKeyframe => "Insert Blank Keyframe",
            Self::ClearKeyframe => "Clear Keyframe",
            Self::ClearFrames => "Clear Frames",
            Self::ReverseFrames => "Reverse Frames",
        }
    }
}

/// Edit an object in place, on the keyframe the playhead is inside.
///
/// **The frame matters.** F6 duplicates a keyframe by cloning the `Arc` around
/// its objects, so one id legitimately appears on several keyframes. Editing
/// the first one found would move the artwork on frame 0 while the user was
/// looking at frame 12 — the change would seem to do nothing, and would
/// quietly damage a frame they were not editing.
fn update_object(scene: &mut Scene, at: EditAt, id: ObjectId, f: impl FnMut(&mut Object)) {
    scene.update_object_where(at, id, f);
}

/// Edit a shape in place, ignoring groups.
fn update_shape(scene: &mut Scene, at: EditAt, id: ObjectId, mut f: impl FnMut(&mut ShapeData)) {
    update_object(scene, at, id, |o| {
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
/// Carry a point from where a turned object is *drawn* back onto its own
/// plane.
///
/// Returns the point unchanged for the overwhelming majority of objects, which
/// are flat. `None` when the object is edge-on or behind the camera, and there
/// is nothing on screen to click.
fn unturn(
    scene: &Scene,
    object: &Object,
    frame: u32,
    layer_depth: f64,
    point: Point,
) -> Option<Point> {
    if object.spatial.is_flat() {
        return Some(point);
    }

    let stage = scene.stage().size;
    let pivot = scene.pivot_of(object);

    // Where the object is drawn, against where it *would* be drawn flat. The
    // difference between the two is what the click travels back through; the
    // turned projection alone would also undo the camera, which the caller has
    // already undone.
    let turned =
        scene
            .camera()
            .projection_for_object(frame, stage, layer_depth, pivot, &object.spatial)?;
    let flat = scene.camera().projection_at_depth(frame, stage, layer_depth)?;

    flat.then(&turned.inverse()?).map_point(point)
}

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
            match &shape.stroke {
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

        // Rigged artwork is hit **where it is drawn**, not where it was drawn.
        // The posed geometry is what the user sees, so it is what they must be
        // able to click: testing the rest pose would make a bent arm
        // selectable only along the straight one it started as.
        ObjectKind::Armature(_) | ObjectKind::Warp(_) => {
            let Some(paths) = buzz_scene::rig::posed_paths(&object.kind) else {
                return false;
            };
            paths.iter().any(|path| {
                buzz_geom::hit::fill_contains(path, local, buzz_geom::FillMode::NonZero)
                    || buzz_geom::hit::stroke_contains(path, local, 0.0, tolerance)
            })
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
    // Merge-shape fusion asks "is this the same paint?", and for a gradient
    // that cannot be answered by one colour. Two different ramps can share an
    // average — a red-to-blue and a blue-to-red have the same one — and fusing
    // them would silently throw one of the two away. So a gradient fuses only
    // with a gradient it matches outright: same stops, same placement, same
    // spread. A gradient and a solid never fuse.
    let same_paint = |a: &Paint, b: &Paint| match (a, b) {
        (Paint::Solid(a), Paint::Solid(b)) => same_color(*a, *b),
        (Paint::Gradient(a), Paint::Gradient(b)) => a == b,
        _ => false,
    };

    // Existing filled shapes that overlap the new one.
    let candidates: Vec<(ObjectId, Paint, BezPath)> = scene
        .layers()
        .get(layer)
        .map(|l| {
            l.objects_at(frame)
                .iter()
                .filter(|o| o.visible && !o.locked)
                .filter_map(|o| match &o.kind {
                    ObjectKind::Shape(s) => s
                        .fill
                        .as_ref()
                        .map(|f| (o.id, f.paint.clone(), s.path.clone())),
                    // Merge-shape rules apply to raw shapes only. Groups,
                    // symbol instances and rigged artwork are objects: in
                    // Animate they sit above the merge layer and never fuse
                    // with what they overlap. Merging a rig would be worse
                    // than useless — it would fuse the artwork away from the
                    // skeleton that deforms it.
                    ObjectKind::Group(_)
                    | ObjectKind::Instance(_)
                    | ObjectKind::Armature(_)
                    | ObjectKind::Warp(_) => None,
                })
                .filter(|(_, _, path)| path.bounding_box().overlaps(bb))
                .collect()
        })
        .unwrap_or_default();

    let mut merged = incoming.path.clone();
    let mut absorbed = Vec::new();

    for (id, paint, path) in candidates {
        if same_paint(&paint, &new_fill.paint) {
            merged = buzz_geom::boolean(&merged, &path, buzz_geom::BoolOp::Union, opts);
            absorbed.push(id);
        } else {
            // Different colour: the new shape cuts into the old one.
            let mut emptied = false;
            update_shape(scene, EditAt::exact(frame), id, |s| {
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
        // **Never write the running user's layout.** An editor saves its
        // workspace whenever a preference changes, and a test that made a
        // document would otherwise leave the next launch opening at whatever
        // size that test asked for.
        static ISOLATED: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        let path = ISOLATED.get_or_init(|| {
            // Per process, so one run cannot inherit what the last one saved.
            std::env::temp_dir().join(format!(
                "buzzanimate-test-workspace-{}.json",
                std::process::id()
            ))
        });
        // SAFETY: set once, to a constant, before any thread reads it — the
        // workspace path is only consulted while loading or saving.
        unsafe { std::env::set_var("BUZZANIMATE_WORKSPACE", path) };

        // And the layout itself starts from the default rather than from
        // whatever an earlier test in this run happened to save: a test that
        // opens a panel must not decide whether the next one finds it open.
        let mut e = Editor {
            workspace: buzz_ui::Workspace::animate(),
            ..Editor::default()
        };
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
                ObjectKind::Shape(s)
                    if s.fill
                        .as_ref()
                        .map(|f| f.color().to_rgba8().to_u8_array()[0])
                        == Some(255) =>
                {
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
        e.doc.edit("Lock", |s| {
            s.update_layer(layer, |l| l.locked = true)
                .then_some(())
                .map(|_| ())
                .unwrap_or(())
        });
        e.selection.clear();

        e.apply(ToolAction::PickAt {
            point: Point::new(50.0, 50.0),
            additive: false,
        });
        assert!(
            e.selection.is_empty(),
            "a locked layer must not be clickable"
        );
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
        let top_level = e
            .scene()
            .layers()
            .iter()
            .map(|l| l.objects_at(0).len())
            .sum::<usize>();
        assert_eq!(top_level, 1, "they should be inside one group");

        e.run(Command::UngroupSelection);
        let top_level = e
            .scene()
            .layers()
            .iter()
            .map(|l| l.objects_at(0).len())
            .sum::<usize>();
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
        assert_eq!(
            e.selection.len(),
            1,
            "the hidden layer's shape must be skipped"
        );
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
        assert_eq!(
            shape.fill.as_ref().unwrap().color().to_rgba8().to_u8_array()[0],
            255
        );
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

    /// Draw a gradient-filled square and select it, which is the state the
    /// Gradient Transform tool needs to do anything.
    fn draw_gradient_square(e: &mut Editor, size: f64) -> ObjectId {
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.style.stroke_enabled = false;
        let area = buzz_geom::Rect::new(0.0, 0.0, size, size);
        e.apply(ToolAction::AddShape {
            shape: ShapeData {
                path: square(0.0, 0.0, size),
                fill: Some(FillSpec::gradient(buzz_scene::Gradient::linear(
                    Color::BLACK,
                    Color::WHITE,
                    area,
                ))),
                stroke: None,
                blend: buzz_scene::PaintBlend::Normal,
            },
            label: "Draw",
        });
        let id = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .map(|o| o.id)
            .last()
            .expect("a shape");
        e.selection.select_one(id);
        id
    }

    fn gradient_of(e: &Editor, id: ObjectId) -> buzz_scene::Gradient {
        let (_, object) = e.scene().find_object(id).expect("the object");
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("expected a shape")
        };
        shape
            .fill
            .as_ref()
            .expect("filled")
            .paint
            .gradient()
            .expect("a gradient")
            .clone()
    }

    /// The Gradient Transform tool moves the ramp, and the move is one undo
    /// step like every other gesture.
    #[test]
    fn the_gradient_transform_tool_moves_the_ramp() {
        let mut e = editor();
        let id = draw_gradient_square(&mut e, 100.0);

        let before = gradient_of(&e, id).handles();
        assert!((before.center.x - 50.0).abs() < 1e-9, "{before:?}");

        e.apply(ToolAction::DragGradient {
            grip: crate::tools::GradientGrip::Center,
            to: Point::new(80.0, 20.0),
        });
        let after = gradient_of(&e, id).handles();
        assert!(
            (after.center - Point::new(80.0, 20.0)).hypot() < 1e-9,
            "the centre did not follow the drag: {after:?}"
        );

        e.doc.undo();
        let undone = gradient_of(&e, id).handles();
        assert!(
            (undone.center - before.center).hypot() < 1e-9,
            "the drag should undo in one step"
        );
    }

    /// **The drag is carried into the object's own space.** A gradient on a
    /// moved shape must land under the pointer, not offset by wherever the
    /// object happens to sit — which is what happens if the pointer's world
    /// coordinates are written into the gradient directly.
    #[test]
    fn a_gradient_drag_lands_under_the_pointer_on_a_moved_shape() {
        let mut e = editor();
        draw_gradient_square(&mut e, 100.0);

        // Move the object itself, so world space and the object's own space
        // are no longer the same thing.
        e.apply(ToolAction::MoveSelection {
            delta: buzz_geom::Vec2::new(300.0, 200.0),
        });

        let target = Point::new(340.0, 260.0);
        e.apply(ToolAction::DragGradient {
            grip: crate::tools::GradientGrip::Center,
            to: target,
        });

        // Reported back in world space, which is where the pointer was.
        let (handles, _) = e
            .selected_gradient_handles()
            .expect("the selected shape has a gradient");
        assert!(
            (handles.center - target).hypot() < 1e-9,
            "the grip should be where the pointer was, got {:?}",
            handles.center
        );
    }

    /// The handles are offered only when there is a gradient to grab.
    #[test]
    fn no_gradient_means_no_handles() {
        let mut e = editor();
        let id = draw_square(&mut e, 0.0, 0.0, 60.0, Color::WHITE).expect("a shape");
        e.selection.select_one(id);
        assert!(
            e.selected_gradient_handles().is_none(),
            "a solid fill has no gradient to transform"
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
        assert!(
            after < before,
            "erasing should remove area: {after} vs {before}"
        );
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
        assert!(
            shape.path.area().abs() > 100.0,
            "the outline should enclose area"
        );
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
        // Text is still to come; the Bone tool used to be here and arrived
        // with Phase 7.
        e.set_tool(ToolId::Text);
        assert_ne!(e.tool(), ToolId::Text);
        assert!(e.status.is_some());
    }

    #[test]
    fn the_rigging_tools_are_selectable() {
        let mut e = editor();
        e.set_tool(ToolId::Bone);
        assert_eq!(e.tool(), ToolId::Bone);
        e.set_tool(ToolId::AssetWarp);
        assert_eq!(e.tool(), ToolId::AssetWarp);
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

        assert_eq!(
            e.scene().shape_count_at(0),
            1,
            "the edit belongs to frame 0"
        );
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

    /// A looping section is a document setting, so playback has to honour it —
    /// what is previewed must be what the export writes, or the loop would
    /// come as a surprise in the finished film.
    #[test]
    fn playback_cycles_within_a_looping_section() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(19);
            });
        });
        e.set_loop_region(buzz_scene::LoopRegion {
            enabled: true,
            start: 4,
            end: 8,
            repeats: 3,
        });

        e.set_frame(4);
        e.toggle_playback();
        for _ in 0..40 {
            // A twelfth of a second: two frames at 24 fps, so the wrap is
            // stepped over rather than landed on exactly.
            e.advance_playback(1.0 / 12.0);
            assert!(
                e.current_frame >= 4 && e.current_frame <= 8,
                "playback left the section at frame {}",
                e.current_frame
            );
        }
        assert!(e.playback.playing, "a looping section does not end playback");
    }

    /// Setting the section is one undo step and survives being undone.
    #[test]
    fn the_looping_section_is_undoable() {
        let mut e = editor();
        e.set_loop_region(buzz_scene::LoopRegion {
            enabled: true,
            start: 0,
            end: 0,
            repeats: 5,
        });
        assert!(e.scene().looping().enabled);

        e.doc.undo();
        assert!(!e.scene().looping().enabled, "undo should take it back off");
    }

    // -- Animate's frame commands -------------------------------------------

    /// Clear Frames empties the drawing and keeps the frames. Clear Keyframe
    /// is the other one: it gives the frames back to the keyframe before it.
    #[test]
    fn clear_frames_empties_the_frame_without_shortening_the_layer() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(9);
            });
        });
        assert_eq!(e.scene().frame_count(), 10);

        e.run(Command::ClearFrames);

        assert_eq!(e.scene().frame_count(), 10, "the span must stay");
        assert!(
            e.scene()
                .layers()
                .get(layer)
                .unwrap()
                .objects_at(0)
                .is_empty(),
            "the artwork should be gone"
        );
    }

    /// Copy Frames then Paste Frames on another frame: the drawing arrives,
    /// as its own copy rather than as the same objects twice.
    #[test]
    fn frames_can_be_copied_and_pasted() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let original = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(9);
            });
        });

        e.run(Command::CopyFrames);
        e.set_frame(5);
        e.run(Command::PasteFrames);

        let pasted: Vec<ObjectId> = e
            .scene()
            .layers()
            .get(layer)
            .unwrap()
            .objects_at(5)
            .iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(pasted.len(), 1, "one object should have arrived");
        assert_ne!(
            pasted[0], original,
            "a pasted drawing is its own, not the same object on two frames"
        );
        assert_eq!(
            e.scene().layers().get(layer).unwrap().objects_at(0).len(),
            1,
            "and the frame it came from is untouched"
        );
    }

    /// Cut Frames is Copy Frames and then Clear Frames.
    #[test]
    fn cut_frames_takes_the_drawing_with_it() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();

        e.run(Command::CutFrames);

        assert!(e.frame_clipboard.is_some(), "it should be on the clipboard");
        assert!(
            e.scene()
                .layers()
                .get(layer)
                .unwrap()
                .objects_at(0)
                .is_empty(),
            "and gone from the frame"
        );
    }

    /// Pasting with an empty clipboard says so rather than doing something.
    #[test]
    fn pasting_nothing_says_so() {
        let mut e = editor();
        e.run(Command::PasteFrames);
        assert!(
            e.status.as_deref().unwrap_or_default().contains("clipboard"),
            "{:?}",
            e.status
        );
        assert!(!e.doc.can_undo(), "and records no edit");
    }

    /// Reverse Frames plays a layer's keyframes back to front, keeping their
    /// timing: what was on the first is on the last.
    #[test]
    fn reverse_frames_swaps_the_ends() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let first = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(9);
                l.frames.insert_blank_keyframe(9);
            });
        });
        e.set_frame(9);
        let second = draw_square(&mut e, 200.0, 0.0, 40.0, Color::BLACK)
            .or_else(|| {
                e.scene()
                    .layers()
                    .get(layer)
                    .unwrap()
                    .objects_at(9)
                    .first()
                    .map(|o| o.id)
            })
            .unwrap();

        e.run(Command::ReverseFrames);

        let on = |frame: u32| {
            e.scene()
                .layers()
                .get(layer)
                .unwrap()
                .objects_at(frame)
                .iter()
                .map(|o| o.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(on(0), vec![second], "the last drawing should now be first");
        assert_eq!(on(9), vec![first], "and the first should be last");
    }

    // -- a new document -----------------------------------------------------

    /// **New asks rather than assuming.** The document on screen is untouched
    /// until the dialog is answered, which is what makes it safe for New to be
    /// on Ctrl+N next to Ctrl+B.
    #[test]
    fn new_opens_the_dialog_and_changes_nothing_yet() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();

        e.run(Command::New);

        assert!(e.new_document.open, "the dialog should be open");
        assert!(
            e.scene().find_object(id).is_some(),
            "the artwork must still be there until the dialog is answered"
        );
    }

    /// The dialog opens on whatever was chosen last, which is the whole point
    /// of remembering it.
    #[test]
    fn the_dialog_opens_on_the_remembered_setup() {
        let mut e = editor();
        e.workspace.new_document = buzz_ui::DocumentSetup {
            width: 1080.0,
            height: 1920.0,
            frame_rate: 30.0,
            background: [0x10, 0x20, 0x30],
        };

        e.run(Command::New);

        assert_eq!(e.new_document.setup.width, 1080.0);
        assert_eq!(e.new_document.setup.height, 1920.0);
        assert_eq!(e.new_document.setup.frame_rate, 30.0);
    }

    /// Creating one applies the size, the rate and the colour, and leaves a
    /// document that is *not* already dirty.
    #[test]
    fn creating_a_document_uses_the_settings_asked_for() {
        let mut e = editor();
        e.create_document(buzz_ui::DocumentSetup {
            width: 1920.0,
            height: 1080.0,
            frame_rate: 25.0,
            background: [0x00, 0x00, 0x00],
        });

        let stage = e.scene().stage();
        assert_eq!(stage.size.width, 1920.0);
        assert_eq!(stage.size.height, 1080.0);
        assert_eq!(stage.frame_rate, 25.0);
        assert_eq!(stage.background.to_rgba8().to_u8_array()[..3], [0, 0, 0]);
        assert!(!e.doc.is_dirty(), "a new document opens clean");
        assert_eq!(e.current_frame, 0);
        assert!(e.selection.active_layer().is_some(), "and has a layer to draw on");
    }

    /// And the settings become the default for the next one — the second
    /// document of a series should be one keypress.
    #[test]
    fn the_settings_are_remembered_for_next_time() {
        let mut e = editor();
        let wanted = buzz_ui::DocumentSetup {
            width: 1280.0,
            height: 720.0,
            frame_rate: 12.0,
            background: [0xFF, 0xFF, 0xFF],
        };
        e.create_document(wanted);

        assert_eq!(e.workspace.new_document, wanted);
        e.run(Command::New);
        assert_eq!(e.new_document.setup, wanted);
    }

    /// A size typed as nonsense must not produce a document nobody can work
    /// in — the dialog clamps, and so does this.
    #[test]
    fn an_impossible_setup_is_brought_back_into_range() {
        let mut e = editor();
        e.create_document(buzz_ui::DocumentSetup {
            width: 0.0,
            height: 1e9,
            frame_rate: 0.0,
            background: [0xFF, 0xFF, 0xFF],
        });

        let stage = e.scene().stage();
        assert!(stage.size.width >= 1.0);
        assert!(stage.size.height <= 16_384.0);
        assert!(stage.frame_rate > 0.0);
    }

    // -- shape recognition --------------------------------------------------

    /// A closed, roughly circular path drawn straight onto the stage.
    fn draw_rough_circle(e: &mut Editor, radius: f64, wobble: f64) -> ObjectId {
        let mut path = BezPath::new();
        for i in 0..40 {
            let t = i as f64 / 40.0 * std::f64::consts::TAU;
            let jitter = ((i as f64 * 12.9898).sin() * 43758.5453).fract() - 0.5;
            let r = radius + jitter * 2.0 * wobble;
            let p = Point::new(150.0 + t.cos() * r, 150.0 + t.sin() * r);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();

        let layer = e.selection.active_layer().unwrap();
        let mut id = None;
        e.doc.edit("Draw", |s| {
            id = s.add_shape(layer, ShapeData::filled(path.clone(), Color::BLACK));
        });
        id.unwrap()
    }

    /// **Draw roughly, then ask for the shape.** The wobble goes and a circle
    /// is left, in the same place and at the same size.
    #[test]
    fn recognise_turns_a_rough_circle_into_a_circle() {
        let mut e = editor();
        let id = draw_rough_circle(&mut e, 60.0, 4.0);
        e.selection.select_one(id);
        let before = e.scene().find_object(id).unwrap().1.bounds();

        e.run(Command::RecogniseShape);

        let after = e.scene().find_object(id).unwrap().1.bounds();
        assert!(
            (after.width() - after.height()).abs() < 1.0,
            "not round: {after:?}"
        );
        assert!(
            (after.center() - before.center()).hypot() < 8.0,
            "it moved: {:?} to {:?}",
            before.center(),
            after.center()
        );
        assert!(
            e.status.as_deref().unwrap_or_default().contains("circle"),
            "the status should say what it found: {:?}",
            e.status
        );
    }

    /// Straighten recognises first, as Animate's does — so the command an
    /// animator actually reaches for after drawing a rough circle gives them a
    /// circle rather than a slightly tidier wobble.
    #[test]
    fn straighten_recognises_a_shape_before_easing_it() {
        let mut e = editor();
        let id = draw_rough_circle(&mut e, 60.0, 4.0);
        e.selection.select_one(id);

        e.run(Command::StraightenSelection);

        let after = e.scene().find_object(id).unwrap().1.bounds();
        assert!(
            (after.width() - after.height()).abs() < 1.0,
            "straighten should have recognised the circle: {after:?}"
        );
    }

    /// Nothing recognisable is left exactly as it was, and says so.
    #[test]
    fn recognise_leaves_a_scribble_alone() {
        let mut e = editor();
        let layer = e.selection.active_layer().unwrap();
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        for i in 1..24 {
            let t = i as f64;
            path.line_to(Point::new(t * 9.0, (t * 0.9).sin() * 70.0 + (t * 2.7).cos() * 25.0));
        }
        let mut id = None;
        e.doc.edit("Draw", |s| {
            id = s.add_shape(layer, ShapeData::stroked(path.clone(), Color::BLACK, 2.0));
        });
        let id = id.unwrap();
        e.selection.select_one(id);

        let before = e.scene().find_object(id).unwrap().1.bounds();
        e.run(Command::RecogniseShape);
        let after = e.scene().find_object(id).unwrap().1.bounds();

        assert_eq!(before, after, "a scribble must be left alone");
        assert!(
            e.status
                .as_deref()
                .unwrap_or_default()
                .contains("recognisable"),
            "{:?}",
            e.status
        );
    }

    /// Recognition is one undo step, like every other edit.
    #[test]
    fn recognising_a_shape_is_undoable() {
        let mut e = editor();
        let id = draw_rough_circle(&mut e, 60.0, 4.0);
        e.selection.select_one(id);
        let before = e.scene().find_object(id).unwrap().1.bounds();

        e.run(Command::RecogniseShape);
        e.doc.undo();

        assert_eq!(e.scene().find_object(id).unwrap().1.bounds(), before);
    }

    // -- the transformation point ------------------------------------------

    /// One object keeps its own point, and it is where the artwork's centre is
    /// until somebody moves it.
    #[test]
    fn the_transformation_point_starts_at_the_centre_of_the_selection() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        e.selection.select_one(id);

        assert_eq!(e.pivot(), Some(Point::new(20.0, 20.0)));
    }

    /// Moved, it is stored **on the object** — so it is saved with the
    /// document and comes back with it.
    #[test]
    fn moving_the_transformation_point_stores_it_on_the_object() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        e.selection.select_one(id);

        e.set_pivot(Point::new(0.0, 40.0));

        assert_eq!(e.pivot(), Some(Point::new(0.0, 40.0)));
        let (_, object) = e.scene().find_object(id).unwrap();
        assert_eq!(object.pivot, Some(Point::new(0.0, 40.0)));

        e.reset_pivot();
        assert_eq!(
            e.pivot(),
            Some(Point::new(20.0, 20.0)),
            "resetting goes back to the centre"
        );
    }

    /// **Rotation turns about it.** A door with its point on the hinge swings
    /// on the hinge: the hinge itself does not move.
    #[test]
    fn rotating_turns_about_the_transformation_point() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        e.selection.select_one(id);
        let hinge = Point::new(0.0, 0.0);
        e.set_pivot(hinge);

        e.run(Command::RotateClockwise);

        let (_, object) = e.scene().find_object(id).unwrap();
        let bounds = object.bounds();
        assert!(
            (e.pivot().unwrap() - hinge).hypot() < 1e-9,
            "the hinge moved: {:?}",
            e.pivot()
        );
        assert!(
            bounds.x0 < -1.0 && bounds.y0 > -1.0,
            "a quarter turn about the top-left corner should swing the square \
             to the left of it, not around its middle: {bounds:?}"
        );
    }

    /// Several objects have nothing to keep a point on, so the editor holds
    /// one for the session — and forgets it when the selection is a different
    /// set, rather than applying one selection's point to another's artwork.
    #[test]
    fn a_group_keeps_its_transformation_point_only_while_it_is_the_selection() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let b = draw_square(&mut e, 100.0, 0.0, 40.0, Color::BLACK).unwrap();
        e.selection.select_one(a);
        e.selection.add(b);

        e.set_pivot(Point::new(70.0, 20.0));
        assert_eq!(e.pivot(), Some(Point::new(70.0, 20.0)));
        assert!(
            e.scene().find_object(a).unwrap().1.pivot.is_none(),
            "a group's point must not be written onto its members"
        );

        e.selection.select_one(a);
        assert_eq!(
            e.pivot(),
            Some(Point::new(20.0, 20.0)),
            "a different selection gets its own point back"
        );
    }

    /// Setting it is undoable, because it changes the document.
    #[test]
    fn moving_the_transformation_point_is_undoable() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        e.selection.select_one(id);
        e.set_pivot(Point::new(0.0, 0.0));

        e.doc.undo();

        assert!(e.scene().find_object(id).unwrap().1.pivot.is_none());
    }

    // -- Auto Keyframe ------------------------------------------------------

    /// Artwork on frame 0, a span reaching frame 9, playhead on frame 5.
    fn span(e: &mut Editor) -> ObjectId {
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(9);
            });
        });
        e.set_frame(5);
        e.selection.select_one(id);
        id
    }

    /// Where the artwork sits on a given frame.
    fn at_frame(e: &Editor, frame: u32, id: ObjectId) -> Point {
        let layer = e.selection.active_layer().unwrap();
        let object = e
            .scene()
            .layers()
            .get(layer)
            .unwrap()
            .objects_at(frame)
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .expect("the artwork");
        object.bounds().center()
    }

    /// **Off — the behaviour every earlier phase had.** Moving artwork inside a
    /// span changes the keyframe that owns the span, so frame 0 moves too.
    #[test]
    fn without_auto_keyframe_an_edit_reaches_back_to_the_keyframe() {
        let mut e = editor();
        let id = span(&mut e);
        assert!(!e.auto_keyframe, "off by default");

        e.transform_selection(Affine::translate((100.0, 0.0)), "Move");

        let layer = e.selection.active_layer().unwrap();
        assert_eq!(
            e.scene().layers().get(layer).unwrap().frames.keyframes().len(),
            1,
            "no keyframe should have been made"
        );
        assert!(
            (at_frame(&e, 0, id).x - at_frame(&e, 5, id).x).abs() < 1e-9,
            "both frames show the same artwork, because they are the same keyframe"
        );
    }

    /// **On — the point of the mode.** The frame being edited gets a keyframe
    /// of its own first, so the change starts there and frame 0 is left alone.
    #[test]
    fn auto_keyframe_keys_the_frame_being_edited() {
        let mut e = editor();
        let id = span(&mut e);
        e.run(Command::ToggleAutoKeyframe);
        assert!(e.auto_keyframe);

        let before = at_frame(&e, 0, id);
        e.transform_selection(Affine::translate((100.0, 0.0)), "Move");

        let layer = e.selection.active_layer().unwrap();
        assert!(
            e.scene().layers().get(layer).unwrap().frames.is_keyframe(5),
            "frame 5 should now be a keyframe of its own"
        );
        assert!(
            (at_frame(&e, 5, id).x - before.x - 100.0).abs() < 1e-9,
            "the move should land on frame 5"
        );
        assert!(
            (at_frame(&e, 0, id).x - before.x).abs() < 1e-9,
            "frame 0 must not have moved"
        );
    }

    /// One keyframe, not one per object: moving three things together is one
    /// edit and must not leave the layer with three keyframes or three undo
    /// steps.
    #[test]
    fn auto_keyframe_makes_one_keyframe_for_a_whole_edit() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let first = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let second = draw_square(&mut e, 100.0, 100.0, 20.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(9);
            });
        });
        e.set_frame(5);
        e.selection.select_one(first);
        e.selection.add(second);
        e.auto_keyframe = true;

        let steps = e.doc.history().undo_depth();
        e.transform_selection(Affine::translate((10.0, 0.0)), "Move");

        let layer = e.selection.active_layer().unwrap();
        assert_eq!(
            e.scene().layers().get(layer).unwrap().frames.keyframes().len(),
            2,
            "frame 0 and frame 5, and nothing else"
        );
        assert_eq!(
            e.doc.history().undo_depth(),
            steps + 1,
            "the keyframe and the move are one undo step"
        );
    }

    /// Undo takes the keyframe back with the change that caused it. A mode
    /// that adds keyframes you then have to delete by hand would be worse than
    /// not having it.
    #[test]
    fn undoing_an_auto_keyframed_edit_removes_the_keyframe() {
        let mut e = editor();
        span(&mut e);
        e.auto_keyframe = true;
        e.transform_selection(Affine::translate((100.0, 0.0)), "Move");

        e.doc.undo();

        let layer = e.selection.active_layer().unwrap();
        assert!(
            !e.scene().layers().get(layer).unwrap().frames.is_keyframe(5),
            "the keyframe should have gone with the move"
        );
    }

    /// A frame that is already a keyframe is left exactly as it is.
    #[test]
    fn auto_keyframe_does_nothing_on_a_frame_that_is_already_keyed() {
        let mut e = editor();
        let id = span(&mut e);
        e.auto_keyframe = true;
        e.set_frame(0);

        e.selection.select_one(id);
        e.transform_selection(Affine::translate((5.0, 0.0)), "Move");

        let layer = e.selection.active_layer().unwrap();
        assert_eq!(
            e.scene().layers().get(layer).unwrap().frames.keyframes().len(),
            1
        );
    }

    // -- Edit Multiple Frames ----------------------------------------------

    /// One square, keyed on frames 0, 5 and 10, playhead in the middle.
    fn keyed_on_three_frames(e: &mut Editor) -> ObjectId {
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(10);
                l.frames.insert_keyframe(5);
                l.frames.insert_keyframe(10);
            });
        });
        e.set_frame(5);
        e.selection.select_one(id);
        id
    }

    /// Where the artwork sits on each of the three keyframes.
    fn x_on_each(e: &Editor, id: ObjectId) -> [f64; 3] {
        [0, 5, 10].map(|frame| at_frame(e, frame, id).x)
    }

    /// **Off — one keyframe changes.** Without the mode this is the ordinary
    /// behaviour, and it is what the mode is measured against.
    #[test]
    fn without_edit_multiple_only_the_current_keyframe_moves() {
        let mut e = editor();
        let id = keyed_on_three_frames(&mut e);
        let before = x_on_each(&e, id);

        e.transform_selection(Affine::translate((100.0, 0.0)), "Move");

        let after = x_on_each(&e, id);
        assert!((after[1] - before[1] - 100.0).abs() < 1e-9, "frame 5 moved");
        assert!((after[0] - before[0]).abs() < 1e-9, "frame 0 must not move");
        assert!((after[2] - before[2]).abs() < 1e-9, "frame 10 must not move");
    }

    /// **On — every keyframe in the range moves together.** The point of the
    /// mode: a scene is shifted across without opening each drawing in turn.
    #[test]
    fn edit_multiple_frames_moves_every_keyframe_in_range() {
        let mut e = editor();
        let id = keyed_on_three_frames(&mut e);
        e.onion.before = 10;
        e.onion.after = 10;
        e.run(Command::ToggleEditMultipleFrames);
        assert!(e.edit_multiple);
        let before = x_on_each(&e, id);

        e.transform_selection(Affine::translate((100.0, 0.0)), "Move");

        let after = x_on_each(&e, id);
        for (i, (a, b)) in after.iter().zip(before.iter()).enumerate() {
            assert!(
                (a - b - 100.0).abs() < 1e-9,
                "keyframe {i} did not move: {b} -> {a}"
            );
        }
    }

    /// The range is the onion markers, as in Animate — so a keyframe outside
    /// them is left alone even with the mode on.
    #[test]
    fn edit_multiple_frames_stops_at_the_onion_markers() {
        let mut e = editor();
        let id = keyed_on_three_frames(&mut e);
        e.onion.before = 1;
        e.onion.after = 1;
        e.edit_multiple = true;
        let before = x_on_each(&e, id);

        e.transform_selection(Affine::translate((100.0, 0.0)), "Move");

        let after = x_on_each(&e, id);
        assert!((after[1] - before[1] - 100.0).abs() < 1e-9, "frame 5 moved");
        assert!(
            (after[0] - before[0]).abs() < 1e-9 && (after[2] - before[2]).abs() < 1e-9,
            "frames 0 and 10 are outside the markers and must not move"
        );
    }

    /// The frames drawn solid are the other keyframes in range — not the one
    /// the playhead is on, which the live frame draws anyway, and not every
    /// frame of the span.
    #[test]
    fn the_frames_shown_are_the_other_keyframes_in_range() {
        let mut e = editor();
        keyed_on_three_frames(&mut e);
        assert!(e.multi_frames().is_empty(), "off by default");

        e.onion.before = 10;
        e.onion.after = 10;
        e.edit_multiple = true;
        assert_eq!(e.multi_frames(), vec![0, 10]);
    }

    /// Select All reaches artwork the playhead is not standing on, because
    /// "select the scene and drag it" is the gesture this mode is for.
    #[test]
    fn select_all_under_edit_multiple_reaches_every_frame() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let first = draw_square(&mut e, 0.0, 0.0, 40.0, Color::BLACK).unwrap();
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Frames", |s| {
            s.update_layer(layer, |l| {
                l.frames.insert_frame(10);
                l.frames.insert_blank_keyframe(10);
            });
        });
        e.set_frame(10);
        // A different drawing on frame 10, as hand-drawn animation has.
        // Found by asking frame 10 rather than through `draw_square`, which
        // looks at frame 0.
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(200.0, 0.0, 40.0), Color::BLACK),
            label: "Draw",
        });
        let second = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(10).iter())
            .map(|o| o.id)
            .find(|id| *id != first)
            .expect("frame 10's drawing");

        e.onion.before = 20;
        e.onion.after = 20;
        e.edit_multiple = true;
        e.run(Command::SelectAll);

        let ids = e.selection.ids();
        assert!(ids.contains(&first), "frame 0's drawing was not selected");
        assert!(ids.contains(&second), "frame 10's drawing was not selected");
    }

    /// Moving the playhead must not throw the selection away while the mode is
    /// on: the artwork is still on screen, so it is still selected.
    #[test]
    fn edit_multiple_keeps_a_selection_across_a_frame_change() {
        let mut e = editor();
        let id = keyed_on_three_frames(&mut e);
        e.onion.before = 10;
        e.onion.after = 10;
        e.edit_multiple = true;

        e.set_frame(7);
        assert!(
            e.selection.ids().contains(&id),
            "the selection should survive the move"
        );
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
        assert_eq!(
            e.scene()
                .layers()
                .get(layer)
                .unwrap()
                .frames
                .keyframe_count(),
            1
        );
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

    /// An editor with one square on a layer whose depth can be set.
    /// **The transformation point can be dragged**, with the Free Transform
    /// tool, from the pointer rather than to somewhere of its own choosing.
    #[test]
    fn the_transformation_point_follows_the_pointer() {
        let mut e = editor();
        let id = draw_square(&mut e, 100.0, 100.0, 200.0, Color::WHITE).expect("a square");
        e.selection.select_one(id);
        e.set_tool(ToolId::FreeTransform);

        let start = e.pivot().expect("a selection has a transformation point");
        assert_eq!(start, Point::new(200.0, 200.0), "it starts at the centre");

        // Grab the circle and drag it to a corner of the artwork.
        let target = Point::new(120.0, 120.0);
        e.pointer_down(e.camera.doc_to_screen(start), Mods::default());
        e.pointer_move(e.camera.doc_to_screen(target), Mods::default());
        e.pointer_up(e.camera.doc_to_screen(target));

        let moved = e.pivot().expect("still a transformation point");
        assert!(
            (moved - target).hypot() < 0.51,
            "the point should be where it was dragged, not {moved:?}"
        );

        // And a click on it without a drag puts it back, as Animate does.
        e.pointer_down(e.camera.doc_to_screen(moved), Mods::default());
        e.pointer_up(e.camera.doc_to_screen(moved));
        assert_eq!(
            e.pivot(),
            Some(Point::new(200.0, 200.0)),
            "clicking the point resets it to the centre"
        );
    }

    /// And with the **Selection** tool, which Animate does not allow — see
    /// §7. A drag that starts on the circle moves it; a click still selects.
    #[test]
    fn the_selection_tool_can_move_the_transformation_point_too() {
        let mut e = editor();
        let id = draw_square(&mut e, 100.0, 100.0, 200.0, Color::WHITE).expect("a square");
        e.selection.select_one(id);
        assert_eq!(e.tool(), ToolId::Selection, "the tool it opens with");

        let start = e.pivot().expect("a transformation point");
        let target = Point::new(140.0, 260.0);
        e.pointer_down(e.camera.doc_to_screen(start), Mods::default());
        e.pointer_move(e.camera.doc_to_screen(target), Mods::default());
        e.pointer_up(e.camera.doc_to_screen(target));

        let moved = e.pivot().expect("still there");
        assert!(
            (moved - target).hypot() < 0.51,
            "the selection tool should move it too, not {moved:?}"
        );

        // A click in the middle of the artwork still selects rather than
        // resetting the point — the thing that would make this deviation cost
        // more than it gives.
        let before = e.pivot();
        e.pointer_down(e.camera.doc_to_screen(moved), Mods::default());
        e.pointer_up(e.camera.doc_to_screen(moved));
        assert_eq!(e.pivot(), before, "a click must not disturb it");
    }

    /// **Clicking a layer selects its artwork**, through the editor's own
    /// entry point — which is what the Layers panel, the timeline and the
    /// Layer Depth view all call.
    #[test]
    fn selecting_a_layer_selects_the_artwork_on_it() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let first = draw_square(&mut e, 0.0, 0.0, 40.0, Color::WHITE).expect("a square");
        let second = draw_square(&mut e, 100.0, 0.0, 40.0, Color::WHITE).expect("a square");
        let layer = e.selection.active_layer().expect("a layer");

        e.selection.clear();
        e.select_layer(layer);

        assert!(e.selection.contains(first));
        assert!(e.selection.contains(second));
        assert_eq!(e.selection.active_layer(), Some(layer));
    }

    /// **Free Transform arrives with something to transform.** Reaching for Q
    /// with nothing selected used to draw no handles at all, so the tool did
    /// nothing until the artwork was clicked as well.
    #[test]
    fn free_transform_selects_the_active_layers_artwork() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 40.0, Color::WHITE).expect("a square");
        e.selection.clear();

        e.set_tool(ToolId::FreeTransform);

        assert!(
            e.selection.contains(id),
            "Q should have taken the layer's artwork"
        );
        assert!(
            e.selection.bounds(e.scene()).is_some(),
            "there should now be handles to draw"
        );
    }

    /// It only ever *adds* to an empty selection. Choosing Q after picking one
    /// leg of a character must not select the whole layer and transform the
    /// lot.
    #[test]
    fn free_transform_leaves_an_existing_selection_alone() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let first = draw_square(&mut e, 0.0, 0.0, 40.0, Color::WHITE).expect("a square");
        let _second = draw_square(&mut e, 100.0, 0.0, 40.0, Color::WHITE).expect("a square");

        e.selection.select_one(first);
        e.set_tool(ToolId::FreeTransform);

        assert_eq!(
            e.selection.ids(),
            vec![first],
            "the chosen object must stay the only one"
        );
    }

    /// Another tool does not grab the layer. Only Free Transform needs
    /// something to put handles on; a brush picking the artwork up would be
    /// baffling.
    #[test]
    fn other_tools_do_not_select_the_layer() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let _ = draw_square(&mut e, 0.0, 0.0, 40.0, Color::WHITE).expect("a square");
        e.selection.clear();

        e.set_tool(ToolId::Brush);
        assert!(e.selection.is_empty(), "the Brush selected the layer");

        e.set_tool(ToolId::Selection);
        assert!(e.selection.is_empty(), "the Selection tool selected the layer");
    }

    /// **Going into a symbol arrives with its artwork selected and its layer
    /// lit.** A symbol is usually one drawing on one layer, and the reason for
    /// going in is almost always to work on that drawing.
    #[test]
    fn opening_a_symbol_selects_what_is_inside_it() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 100.0, 100.0, 200.0, Color::WHITE).expect("a square");
        e.selection.select_one(id);
        e.run(Command::ConvertToSymbol);
        e.run(Command::EditSymbol);

        assert!(
            !e.scene().edit_path().is_empty(),
            "the symbol should have opened"
        );
        assert!(
            e.selection.active_layer().is_some(),
            "a layer should be lit in the timeline"
        );
        assert!(
            !e.selection.is_empty(),
            "the symbol's own artwork should be selected"
        );

        // And every selected object really is inside the symbol, not left over
        // from the stage.
        for object in e.selection.iter() {
            assert!(
                e.scene().find_object(object).is_some(),
                "{object:?} is not in the open symbol"
            );
        }
    }

    /// Every object on the stage's first frame, in order.
    fn stage_objects(e: &Editor) -> Vec<ObjectId> {
        e.scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter().map(|o| o.id))
            .collect()
    }

    /// Build an Animate-shaped character: a symbol whose layers hold
    /// part-symbols, which is how every rig out of Animate is put together.
    ///
    /// Built through the editor's own gestures — draw, F8, draw, F8, select the
    /// lot, F8 again — so it exercises the same path a user takes rather than
    /// a hand-assembled scene the real code never produces.
    fn place_nested_character(e: &mut Editor) -> ObjectId {
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        let mut parts = Vec::new();
        for (x, y, size) in [(260.0, 150.0, 40.0), (200.0, 270.0, 30.0), (240.0, 210.0, 70.0)] {
            let drawn = draw_square(e, x, y, size, Color::WHITE).expect("a square");
            e.selection.select_one(drawn);
            e.run(Command::ConvertToSymbol);
            // F8 replaces the artwork with an instance; it is the object that
            // is on the stage now and was not before.
            let now = stage_objects(e);
            let fresh = now
                .into_iter()
                .find(|id| !parts.contains(id))
                .expect("the instance that replaced the square");
            parts.push(fresh);
        }

        // The character: all three parts, made into one symbol.
        e.selection.set(parts.clone());
        e.run(Command::ConvertToSymbol);

        stage_objects(e)
            .into_iter()
            .next()
            .expect("the character instance")
    }

    /// **A character measures its artwork, not a placeholder.**
    ///
    /// `Object::bounds` returns a two-unit box for an instance, because an
    /// object cannot reach the library. A symbol whose layers hold *instances*
    /// — which is every Animate rig — was therefore measured as two units
    /// across, and hit-testing rejects a click outside those bounds before it
    /// tests any artwork. The character was clickable only at a dot on its own
    /// origin: it could not be selected, and could not be double-clicked into.
    #[test]
    fn a_character_of_nested_symbols_measures_its_real_extent() {
        let mut e = editor();
        let id = place_nested_character(&mut e);

        let (_, object) = e.scene().find_object(id).expect("the character");
        let bounds = e.scene().resolved_bounds(object);

        // **Not merely "wide enough".** Three placeholder dots at three
        // different positions already union into a wide box, so a width check
        // passes without the library being consulted at all. The claim is that
        // the bounds cover the artwork: this point is on the far corner of the
        // largest part and is nowhere near any part's origin.
        let far_corner = Point::new(305.0, 275.0);
        assert!(
            bounds.contains(far_corner),
            "the character measured {bounds:?}, which does not cover its own              artwork at {far_corner:?} — the library was not consulted"
        );
    }

    /// The consequence, through the editor's own hit-testing: a click anywhere
    /// on the character's artwork finds it, not only at its origin.
    #[test]
    fn a_character_can_be_clicked_anywhere_on_its_artwork() {
        let mut e = editor();
        let id = place_nested_character(&mut e);

        // Inside the largest part, and nowhere near the character's origin.
        let on_the_body = Point::new(270.0, 240.0);
        assert_eq!(
            e.object_at(on_the_body, 2.0),
            Some(id),
            "a click on the character's body should find the character"
        );

        assert_eq!(
            e.object_at(Point::new(700.0, 560.0), 2.0),
            None,
            "a click well away from it should still find nothing"
        );
    }

    /// **The whole complaint, end to end.** Double-click into a character, then
    /// click a part and have that part — not the character — selected.
    #[test]
    fn double_clicking_a_character_goes_inside_and_its_parts_can_be_selected() {
        let mut e = editor();
        place_nested_character(&mut e);

        let on_the_body = Point::new(270.0, 240.0);
        e.enter_or_leave_at(e.camera.doc_to_screen(on_the_body));

        assert!(
            !e.scene().edit_path().is_empty(),
            "double-clicking the character should have opened it"
        );

        // Inside, the parts are instances on their own layers. The symbol was
        // opened *in place*, so `screen_to_edit` carries a click back through
        // the instance's transform — the same point on screen is the same
        // point on the artwork.
        let inside = e.screen_to_edit(e.camera.doc_to_screen(on_the_body));
        let part = e
            .object_at(inside, 2.0)
            .expect("a part of the character should be clickable inside it");
        e.selection.select_one(part);

        assert_eq!(e.selection.len(), 1, "one part, not the whole character");
        assert!(
            e.scene().find_object(part).is_some(),
            "the selected part should be inside the open symbol"
        );
    }

    /// The same for a **symbol instance**, which is the case that matters:
    /// a character is rotated about a hip or a shoulder, and both are on an
    /// instance rather than on loose artwork.
    #[test]
    fn the_transformation_point_of_an_instance_moves_too() {
        let mut e = editor();
        let id = draw_square(&mut e, 100.0, 100.0, 200.0, Color::WHITE).expect("a square");
        e.selection.select_one(id);
        e.run(Command::ConvertToSymbol);

        let instance = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter().map(|o| o.id))
            .next()
            .expect("the instance that replaced the square");
        e.selection.select_one(instance);
        e.set_tool(ToolId::FreeTransform);

        let start = e.pivot().expect("an instance has a transformation point");
        let target = start + Vec2::new(-70.0, -70.0);
        e.pointer_down(e.camera.doc_to_screen(start), Mods::default());
        e.pointer_move(e.camera.doc_to_screen(target), Mods::default());
        e.pointer_up(e.camera.doc_to_screen(target));

        let moved = e.pivot().expect("still there");
        assert!(
            (moved - target).hypot() < 0.51,
            "an instance's point should move with the pointer, not {moved:?}"
        );
    }

    /// **A symbol opens where its instance stands** — Animate's Edit in Place.
    ///
    /// The instance's transform becomes the place its contents are drawn
    /// through, so a head opened from a character stays on the shoulders
    /// instead of jumping to the origin; and a click carried back through the
    /// same transform lands where the pointer is.
    #[test]
    fn a_symbol_opens_where_its_instance_stands() {
        let mut e = editor();
        let id = draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE).expect("a square");
        e.selection.select_one(id);
        e.run(Command::ConvertToSymbol);

        // Move the instance well away from the origin.
        let instance = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter().map(|o| o.id))
            .next()
            .expect("the instance");
        e.doc.edit("Move", |scene| {
            scene.update_object(instance, |o| {
                o.transform = Affine::translate(Vec2::new(400.0, 300.0)) * o.transform;
            });
        });

        let on_screen = e.camera.doc_to_screen(
            e.scene()
                .find_object(instance)
                .map(|(_, o)| o.bounds().center())
                .expect("its bounds"),
        );
        e.enter_or_leave_at(on_screen);

        assert!(!e.scene().edit_path().is_empty(), "it should have opened");
        let place = e.scene().edit_place().as_coeffs();
        assert!(
            (place[4] - 400.0).abs() < 1e-6 && (place[5] - 300.0).abs() < 1e-6,
            "the place should be the instance's own transform, got {:?}",
            (place[4], place[5])
        );

        // And a click at that spot now means the symbol's own origin area,
        // not a point four hundred units away.
        let inside = e.screen_to_edit(e.camera.doc_to_screen(Point::new(450.0, 350.0)));
        assert!(
            (inside - Point::new(50.0, 50.0)).hypot() < 1e-6,
            "a click should be carried back through the place, got {inside:?}"
        );
    }

    /// **Double-click goes into the symbol under the pointer, and out again.**
    ///
    /// Animate's whole navigation. Nothing about it is visible to the eye in a
    /// screenshot either — the stage draws the symbol's contents, which for a
    /// character that fills the frame looks much like the scene did — so the
    /// edit path is what gets asserted.
    #[test]
    fn double_clicking_an_instance_opens_it_and_empty_space_leaves() {
        let mut e = editor();
        let square = draw_square(&mut e, 100.0, 100.0, 120.0, Color::WHITE).expect("a square");
        e.selection.select_one(square);
        e.run(Command::ConvertToSymbol);

        let symbol = e.scene().library().iter().next().expect("a symbol").id;
        assert!(
            e.scene().edit_path().is_empty(),
            "converting does not open the symbol"
        );

        // The instance sits where the square was; aim at the middle of it.
        let screen = e.camera.doc_to_screen(Point::new(160.0, 160.0));
        e.enter_or_leave_at(screen);
        assert_eq!(
            e.scene().edit_path(),
            &[symbol],
            "double-clicking the instance should open it"
        );

        // Well away from the artwork: back out a level.
        let empty = e.camera.doc_to_screen(Point::new(-4000.0, -4000.0));
        e.enter_or_leave_at(empty);
        assert!(
            e.scene().edit_path().is_empty(),
            "double-clicking empty space should leave the symbol"
        );
    }

    fn editor_with_deep_square(depth: f64) -> (Editor, ObjectId) {
        let mut e = editor();
        let id =
            draw_square(&mut e, 200.0, 125.0, 150.0, Color::WHITE).expect("the square is placed");

        let layer = e.scene().layers().iter().next().unwrap().id;
        e.doc.edit("Depth", |scene| {
            scene.update_layer(layer, |l| l.depth = depth);
        });
        (e, id)
    }

    /// The bug depth-aware picking exists to prevent: a layer pushed into the
    /// distance is drawn smaller, so clicking where it *looks* has to select
    /// it, and clicking where its geometry used to be must not.
    #[test]
    fn a_layer_pushed_into_the_distance_is_clicked_where_it_is_drawn() {
        // Stage 550x400, so the centre is (275, 200). The square spans
        // 200..350 x 125..275, centred on the stage.
        let (deep, id) = editor_with_deep_square(1000.0); // half size
        let tolerance = 0.5;

        // Half size about the stage centre puts the square's corner at
        // (237.5, 162.5); its old corner at (200, 125) is now outside it.
        assert_eq!(
            deep.object_at(Point::new(245.0, 170.0), tolerance),
            Some(id),
            "the click should land where the shrunken square is drawn"
        );
        assert_eq!(
            deep.object_at(Point::new(205.0, 130.0), tolerance),
            None,
            "and not where its untransformed geometry sits"
        );

        // The centre is on the square at any depth, which is a useful control:
        // it shows the test is not simply missing everything.
        assert_eq!(
            deep.object_at(Point::new(275.0, 200.0), tolerance),
            Some(id)
        );
    }

    /// A layer on the focal plane must pick exactly as it always did.
    #[test]
    fn depth_zero_picking_is_unchanged() {
        let (flat, id) = editor_with_deep_square(0.0);
        assert_eq!(flat.object_at(Point::new(205.0, 130.0), 0.5), Some(id));
        assert_eq!(flat.object_at(Point::new(100.0, 100.0), 0.5), None);
    }

    /// A layer pulled in front of the camera is drawn larger, so it should be
    /// selectable well beyond where its geometry sits.
    #[test]
    fn a_layer_pulled_forward_is_selectable_over_its_larger_drawn_area() {
        let (near, id) = editor_with_deep_square(-500.0); // double size

        // Doubling about (275, 200) puts the corner at (125, 50), so a point
        // outside the original square is now inside the drawn one.
        assert_eq!(
            near.object_at(Point::new(150.0, 80.0), 0.5),
            Some(id),
            "the enlarged square should cover this"
        );
    }

    /// A layer at or behind the camera is not drawn, so it must not be
    /// selectable either — clicking empty space should not find it.
    #[test]
    fn a_layer_behind_the_camera_cannot_be_selected() {
        let (behind, _) = editor_with_deep_square(-1000.0);
        for probe in [
            Point::new(275.0, 200.0),
            Point::new(205.0, 130.0),
            Point::new(0.0, 0.0),
        ] {
            assert_eq!(behind.object_at(probe, 0.5), None, "at {probe:?}");
        }
    }

    // -- scripting ----------------------------------------------------------

    fn scripted(source: &str) -> Editor {
        let mut e = editor();
        e.actions.source = source.to_string();
        e.run(Command::RunScript);
        e
    }

    /// The promise the whole feature is sold on: however much a script draws,
    /// it is one Ctrl+Z.
    #[test]
    fn a_script_that_draws_forty_shapes_is_one_undo_step() {
        let mut e = scripted(
            "var d = fl.getDocumentDOM();
             d.setFillColor('#FF0000');
             for (var i = 0; i < 40; i++) {
                 d.addNewRectangle({left: i, top: 0, right: i + 5, bottom: 5});
             }",
        );

        assert_eq!(e.scene().shape_count(), 40);
        assert!(e.doc.can_undo());

        e.run(Command::Undo);
        assert_eq!(
            e.scene().shape_count(),
            0,
            "one undo should reverse all of it"
        );
    }

    /// Two runs are two deliberate acts, so they must not coalesce into one
    /// undo step the way the moves of a single drag do.
    #[test]
    fn two_runs_are_two_undo_steps() {
        let mut e =
            scripted("fl.getDocumentDOM().addNewRectangle({left:0, top:0, right:10, bottom:10});");
        e.run(Command::RunScript);
        assert_eq!(e.scene().shape_count(), 2);

        e.run(Command::Undo);
        assert_eq!(
            e.scene().shape_count(),
            1,
            "the second run should undo alone"
        );
    }

    /// The editor adopts what the script left behind, or `d.selectAll()` and
    /// `t.currentFrame = 3` would appear to do nothing.
    #[test]
    fn the_editor_adopts_the_selection_and_playhead_a_script_left() {
        let e = scripted(
            "var d = fl.getDocumentDOM();
             d.addNewRectangle({left:0, top:0, right:10, bottom:10});
             d.selectAll();
             d.getTimeline().insertFrames(5);
             d.getTimeline().currentFrame = 3;",
        );

        assert_eq!(
            e.selection.len(),
            1,
            "the drawn rectangle should be selected"
        );
        assert_eq!(e.frame(), 3);
    }

    /// A failing script keeps what it managed and says what went wrong, in the
    /// Output area rather than only in the status bar.
    #[test]
    fn a_failing_script_keeps_its_work_and_reports_the_error() {
        let e = scripted(
            "fl.trace('starting');
             fl.getDocumentDOM().addNewRectangle({left:0, top:0, right:10, bottom:10});
             throw new Error('deliberate');",
        );

        assert_eq!(e.scene().shape_count(), 1, "work before the error survives");
        assert_eq!(e.actions.output, vec!["starting".to_string()]);
        assert!(
            e.actions
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("deliberate"),
            "{:?}",
            e.actions.error
        );
    }

    /// Running from the menu with the panel shut would put the output where
    /// nobody can read it.
    #[test]
    fn running_opens_the_panel_and_an_empty_script_says_so() {
        let mut e = editor();
        assert!(
            !e.workspace.is_open(buzz_ui::PanelId::Actions),
            "the panel starts closed"
        );

        e.run(Command::RunScript);
        assert!(e.workspace.is_open(buzz_ui::PanelId::Actions));
        assert!(
            e.status.as_deref().unwrap_or_default().contains("Actions"),
            "{:?}",
            e.status
        );
        assert!(!e.doc.can_undo(), "an empty script is not an edit");
    }

    /// Reading the document is not editing it: a script that only traces must
    /// leave the document clean, or every inspection would mark it dirty.
    #[test]
    fn a_reading_script_leaves_the_document_unchanged() {
        let e = scripted("fl.trace(fl.getDocumentDOM().width);");

        assert!(!e.doc.can_undo());
        assert_eq!(e.actions.output, vec!["550".to_string()]);
    }

    /// F9 is Animate's key for the Actions panel, and it has to work both ways.
    #[test]
    fn the_actions_panel_toggles() {
        let mut e = editor();
        e.run(Command::ToggleActionsPanel);
        assert!(e.workspace.is_open(buzz_ui::PanelId::Actions));
        e.run(Command::ToggleActionsPanel);
        assert!(!e.workspace.is_open(buzz_ui::PanelId::Actions));
    }

    // -- rigging ------------------------------------------------------------

    /// Drag with the pointer, in document coordinates, as the window does.
    fn drag(e: &mut Editor, from: Point, to: Point) {
        let camera = e.camera;
        let screen = |p: Point| {
            let s = camera.doc_to_screen(p);
            Point::new(s.x, s.y)
        };
        let (a, b) = (screen(from), screen(to));
        e.pointer_down(a, Mods::default());
        e.pointer_move(b, Mods::default());
        e.pointer_up(b);
    }

    /// A limb to rig: a bar from (0, 90) to (200, 110).
    fn editor_with_limb() -> (Editor, ObjectId) {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.style.fill_color = Color::WHITE;
        e.style.stroke_enabled = false;
        let id = {
            let layer = e.selection.active_layer().expect("a layer");
            let mut made = None;
            e.doc.edit("Draw", |scene| {
                made = scene.add_shape(
                    layer,
                    ShapeData::filled(
                        buzz_geom::Rect::new(0.0, 90.0, 200.0, 110.0).to_path(1e-9),
                        Color::WHITE,
                    ),
                );
            });
            made.expect("a shape")
        };
        e.selection.clear();
        (e, id)
    }

    fn armature_of(e: &Editor, id: ObjectId) -> buzz_rig::Armature {
        let (_, object) = e.scene().find_object(id).expect("the object");
        match &object.kind {
            ObjectKind::Armature(rig) => rig.armature.clone(),
            other => panic!("expected an armature, found {other:?}"),
        }
    }

    /// The first gesture of Phase 7: drag the Bone tool across artwork and it
    /// becomes a rig.
    #[test]
    fn dragging_the_bone_tool_across_artwork_rigs_it() {
        let (mut e, id) = editor_with_limb();
        e.set_tool(ToolId::Bone);

        drag(&mut e, Point::new(0.0, 100.0), Point::new(100.0, 100.0));

        let armature = armature_of(&e, id);
        assert_eq!(armature.len(), 1);
        assert!(
            (armature.bones[0].length - 100.0).abs() < 1.0,
            "{armature:?}"
        );
        assert!(e.doc.can_undo(), "rigging must be undoable");
    }

    /// Building a chain: each drag from the previous bone's tip adds the next.
    #[test]
    fn dragging_from_a_bone_tip_adds_a_child_bone() {
        let (mut e, id) = editor_with_limb();
        e.set_tool(ToolId::Bone);

        drag(&mut e, Point::new(0.0, 100.0), Point::new(100.0, 100.0));
        drag(&mut e, Point::new(100.0, 100.0), Point::new(200.0, 100.0));

        let armature = armature_of(&e, id);
        assert_eq!(
            armature.len(),
            2,
            "the second drag should have added a bone"
        );
        assert_eq!(armature.bones[1].parent, Some(0));
    }

    /// Dragging a bone poses the rig, and the artwork follows.
    #[test]
    fn dragging_a_bone_poses_the_rig_and_moves_the_artwork() {
        let (mut e, id) = editor_with_limb();
        e.set_tool(ToolId::Bone);
        drag(&mut e, Point::new(0.0, 100.0), Point::new(100.0, 100.0));
        drag(&mut e, Point::new(100.0, 100.0), Point::new(200.0, 100.0));

        let before = e.scene().find_object(id).expect("there").1.bounds();
        // Grab the second bone in the middle and pull it downwards.
        drag(&mut e, Point::new(150.0, 100.0), Point::new(120.0, 190.0));
        let after = e.scene().find_object(id).expect("there").1.bounds();

        assert!(
            after.y1 > before.y1 + 20.0,
            "the artwork did not follow the pose: {before:?} then {after:?}"
        );
    }

    /// A whole posing drag is one undo step, like every other drag.
    #[test]
    fn a_posing_drag_is_a_single_undo_step() {
        let (mut e, id) = editor_with_limb();
        e.set_tool(ToolId::Bone);
        drag(&mut e, Point::new(0.0, 100.0), Point::new(100.0, 100.0));
        drag(&mut e, Point::new(100.0, 100.0), Point::new(200.0, 100.0));

        let posed_from = armature_of(&e, id).pose();

        // A drag with several moves in it, as a real one has.
        let camera = e.camera;
        let screen = |p: Point| {
            let s = camera.doc_to_screen(p);
            Point::new(s.x, s.y)
        };
        e.pointer_down(screen(Point::new(150.0, 100.0)), Mods::default());
        for y in [120.0, 150.0, 180.0, 190.0] {
            e.pointer_move(screen(Point::new(130.0, y)), Mods::default());
        }
        e.pointer_up(screen(Point::new(130.0, 190.0)));

        assert_ne!(armature_of(&e, id).pose(), posed_from, "nothing moved");
        e.run(Command::Undo);
        assert_eq!(
            armature_of(&e, id).pose(),
            posed_from,
            "one undo should reverse the whole drag"
        );
    }

    /// A click is not a bone: a zero-length bone is a joint that can never be
    /// grabbed again.
    #[test]
    fn a_click_with_the_bone_tool_does_not_make_a_bone() {
        let (mut e, id) = editor_with_limb();
        e.set_tool(ToolId::Bone);

        drag(&mut e, Point::new(50.0, 100.0), Point::new(50.2, 100.1));

        assert!(
            matches!(
                &e.scene().find_object(id).expect("there").1.kind,
                ObjectKind::Shape(_)
            ),
            "a click should have left the artwork alone"
        );
        assert!(e.status.is_some(), "and said why");
    }

    #[test]
    fn the_asset_warp_tool_puts_handles_on_artwork_and_drags_them() {
        let (mut e, id) = editor_with_limb();
        e.set_tool(ToolId::AssetWarp);

        // The first touch turns the shape into warped artwork with a grid.
        let camera = e.camera;
        let screen = |p: Point| {
            let s = camera.doc_to_screen(p);
            Point::new(s.x, s.y)
        };
        e.pointer_down(screen(Point::new(100.0, 100.0)), Mods::default());
        e.pointer_up(screen(Point::new(100.0, 100.0)));

        let handles = match &e.scene().find_object(id).expect("there").1.kind {
            ObjectKind::Warp(warp) => warp.handles.len(),
            other => panic!("expected warped artwork, found {other:?}"),
        };
        assert_eq!(handles, 9);

        // Now drag the middle handle, which sits at the centre of the artwork.
        let before = e.scene().find_object(id).expect("there").1.bounds();
        drag(&mut e, Point::new(100.0, 100.0), Point::new(100.0, 200.0));
        let after = e.scene().find_object(id).expect("there").1.bounds();

        assert!(
            after.y1 > before.y1 + 10.0,
            "the warp did not take: {before:?} then {after:?}"
        );
    }

    /// Rigging moves the artwork into the armature rather than copying it —
    /// two copies of one drawing, one rigged and one not, is not what anybody
    /// means by rigging.
    #[test]
    fn rigging_does_not_duplicate_the_artwork() {
        let (mut e, _) = editor_with_limb();
        let before = e.scene().shape_count();
        e.set_tool(ToolId::Bone);
        drag(&mut e, Point::new(0.0, 100.0), Point::new(100.0, 100.0));

        assert_eq!(e.scene().shape_count(), before);
    }

    /// A script run inside a symbol edits that symbol, because the scene
    /// answers `layers()` for whichever timeline is open. The document's own
    /// timeline must be left alone.
    #[test]
    fn a_script_run_inside_a_symbol_draws_into_the_symbol() {
        let mut e = editor();
        e.run(Command::NewSymbol);
        assert!(
            !e.scene().edit_path().is_empty(),
            "should be inside a symbol"
        );

        e.actions.source =
            "fl.getDocumentDOM().addNewRectangle({left:0, top:0, right:20, bottom:20});".into();
        e.run(Command::RunScript);

        assert_eq!(e.scene().shape_count(), 1, "drawn inside the symbol");

        e.run(Command::EditDocument);
        assert_eq!(
            e.scene().shape_count(),
            0,
            "the main timeline should be untouched"
        );
    }

    // -- lighting ------------------------------------------------------------

    fn sun_of(e: &Editor) -> (f64, f64) {
        match e.scene().lights().lights.first().expect("a light").kind {
            buzz_scene::LightKind::Sun { azimuth, elevation } => (azimuth, elevation),
            other => panic!("not a sun: {other:?}"),
        }
    }

    /// Adding a light from the menu puts one in the document, switches the rig
    /// on, and leaves it selected so the panel is already showing it.
    #[test]
    fn adding_a_sun_lights_the_document_and_selects_it() {
        let mut e = editor();
        assert!(
            !e.scene().lights().is_active(),
            "a new document has no lights"
        );

        e.run(Command::AddSun);

        assert_eq!(e.scene().lights().lights.len(), 1);
        assert!(e.scene().lights().is_active(), "the rig should switch on");
        assert!(
            e.light_panel.selected.is_some(),
            "the new light should be selected"
        );
        assert!(e.doc.can_undo(), "adding a light must be undoable");

        e.run(Command::Undo);
        assert!(e.scene().lights().lights.is_empty());
    }

    /// A lamp arrives where the user is looking, not at the origin — which is
    /// off the top-left of the stage and would look like nothing happened.
    #[test]
    fn a_lamp_arrives_in_the_middle_of_the_view() {
        let mut e = editor();
        e.camera.center = Point::new(640.0, 360.0);
        e.run(Command::AddLamp);

        match e.scene().lights().lights[0].kind {
            buzz_scene::LightKind::Lamp { position, .. } => {
                assert_eq!(position, Point::new(640.0, 360.0));
            }
            other => panic!("not a lamp: {other:?}"),
        }
    }

    /// The gesture the whole gizmo exists for: drag the sun's handle across
    /// the stage and the light now points from where it was dropped.
    #[test]
    fn dragging_the_sun_handle_swings_the_light() {
        let mut e = editor();
        e.run(Command::AddSun);
        e.set_tool(ToolId::Selection);

        let stage = e.scene().stage().stage_rect();
        let (azimuth, elevation) = sun_of(&e);
        let handle = crate::lights::sun_handle(stage, azimuth, elevation);

        // Straight left of the middle of the stage, half way to the rim: the
        // sun ends up in the west, half way up.
        let centre = stage.center();
        let target = Point::new(
            centre.x - stage.width().min(stage.height()) * 0.21,
            centre.y,
        );
        drag(&mut e, handle, target);

        let (azimuth, elevation) = sun_of(&e);
        assert!(
            (azimuth.abs() - std::f64::consts::PI).abs() < 1e-6,
            "the sun should now lie to the west: {azimuth}"
        );
        assert!(
            (elevation - std::f64::consts::FRAC_PI_2 * 0.5).abs() < 1e-6,
            "and half way up: {elevation}"
        );
    }

    /// One drag, one undo step — not one per mouse-move.
    #[test]
    fn aiming_the_sun_is_a_single_undo_step() {
        let mut e = editor();
        e.run(Command::AddSun);
        e.set_tool(ToolId::Selection);
        let stage = e.scene().stage().stage_rect();
        let (azimuth, elevation) = sun_of(&e);
        let handle = crate::lights::sun_handle(stage, azimuth, elevation);

        // A drag with several intermediate moves, as a real one has.
        let camera = e.camera;
        let screen = |p: Point| {
            let s = camera.doc_to_screen(p);
            Point::new(s.x, s.y)
        };
        e.pointer_down(screen(handle), Mods::default());
        for step in 1..=6 {
            let at = handle + Vec2::new(-8.0 * step as f64, 4.0 * step as f64);
            e.pointer_move(screen(at), Mods::default());
        }
        let end = handle + Vec2::new(-48.0, 24.0);
        e.pointer_up(screen(end));

        let after = sun_of(&e);
        e.run(Command::Undo);
        assert_ne!(
            sun_of(&e),
            after,
            "one undo should take the whole drag back"
        );
        assert_eq!(
            sun_of(&e),
            (azimuth, elevation),
            "and land exactly where the sun started"
        );
    }

    /// A light handle must not eat a brush stroke aimed at artwork beneath it.
    #[test]
    fn light_handles_are_only_grabbed_with_the_selection_tool() {
        let mut e = editor();
        e.run(Command::AddLamp);
        let at = match e.scene().lights().lights[0].kind {
            buzz_scene::LightKind::Lamp { position, .. } => position,
            other => panic!("{other:?}"),
        };

        e.set_tool(ToolId::Rectangle);
        drag(&mut e, at, at + Vec2::new(60.0, 60.0));

        match e.scene().lights().lights[0].kind {
            buzz_scene::LightKind::Lamp { position, .. } => {
                assert_eq!(position, at, "the rectangle drag moved the lamp");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(e.scene().shape_count(), 1, "and it should have drawn one");
    }

    /// Hiding the handles hides them from the pointer too: a hidden handle
    /// that still swallowed clicks would be a ghost.
    #[test]
    fn hidden_handles_cannot_be_grabbed() {
        let mut e = editor();
        e.run(Command::AddSun);
        e.set_tool(ToolId::Selection);
        e.run(Command::ToggleLightGizmos);
        assert!(!e.light_panel.gizmos);

        let stage = e.scene().stage().stage_rect();
        let before = sun_of(&e);
        let handle = crate::lights::sun_handle(stage, before.0, before.1);
        drag(&mut e, handle, stage.center());

        assert_eq!(sun_of(&e), before, "a hidden handle was still grabbed");
    }
}
