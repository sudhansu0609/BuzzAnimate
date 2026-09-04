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
    ActionsState, Command, DrawStyle, DrawingMode, LibraryState, Selection, SymmetryMode,
    SymmetrySettings, ToolId, ViewSettings,
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
    /// The Armature panel's own state — what is being typed into the pose
    /// name box. View state, like the Library's.
    pub rig_panel: buzz_ui::RigPanelState,
    /// Stages saved as starting points. Rescanned when one is saved, so the
    /// File menu lists what is actually on disk.
    pub templates: buzz_doc::TemplateLibrary,
    /// Frames on the clipboard, from Cut Frames or Copy Frames.
    ///
    /// View state, not document state: a clipboard that was saved with the
    /// film and came back a week later would be a surprise, and one that
    /// travelled inside a `.buzz` file handed to somebody else would be a
    /// stranger one.
    /// **What Copy Frames took: one frame of every layer.**
    ///
    /// Per layer rather than for the active one alone, because a frame of a
    /// drawing is a frame of the *whole* drawing — the character, the
    /// background and the overlay are on three layers and copying one of them
    /// is never what anybody means. The layer each came from is kept so the
    /// paste puts it back where it belongs rather than piling every layer's
    /// artwork onto whichever one happens to be active.
    pub frame_clipboard: Option<Vec<(LayerId, Vec<std::sync::Arc<Object>>)>>,
    /// Artwork on the clipboard, from Cut or Copy.
    ///
    /// # Why a whole `Scene`
    ///
    /// The same reason an asset is one: an instance whose symbol was left
    /// behind draws nothing, so what is copied has to carry the definitions it
    /// depends on. `Scene::extract` already gathers those recursively and
    /// `Scene::merge` already renumbers every id on the way in, so the
    /// clipboard is those two functions with somewhere to put the result.
    ///
    /// **Kept on the `Editor`, which is replaced when a document is opened.**
    /// That is deliberate and it is the one thing to be careful about: see
    /// `App::take_clipboard`, which carries it across so that copy-here,
    /// open-that, paste-there works — the whole point of having a clipboard
    /// rather than Duplicate.
    pub clipboard: Option<buzz_scene::Scene>,
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
    /// The Motion Editor's view state (it holds none of the curve; that lives on
    /// the keyframe).
    pub motion_editor: buzz_ui::MotionEditorState,
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
    /// While scrubbing the playhead with sound, the moment the short burst of
    /// audio should stop if the drag has paused. `None` when not scrubbing.
    scrub_until: Option<std::time::Instant>,
    /// Beat frames detected from the soundtrack, shown as ticks on the ruler.
    /// View state, not saved — a guide the animator keys action to.
    /// **Frames marked on the timeline ruler.**
    ///
    /// Written by `Detect Beats` (where the music hits) and by
    /// `Fit to Narration` (where each line of the voice-over starts). One set
    /// of marks rather than two, because they are answers to the same question
    /// — *where in this soundtrack does something happen* — and two rows of
    /// ticks on one ruler would be unreadable. The last command run wins, and
    /// its status line says which.
    pub ruler_marks: Vec<u32>,
    /// The Lip Sync dialog.
    pub lip_sync: buzz_ui::LipSyncState,
    /// Whether the last sound import also landed on the timeline. See
    /// [`Self::sound_was_placed`].
    sound_placed: bool,
    /// The Set the Scene and Animate Selection dialogs. See `crate::staging`.
    pub staging: buzz_ui::StagingState,
    /// A motion path just drawn, waiting on the dialog: the curve and the object
    /// it will send along it, captured at draw time so a later change of
    /// selection cannot send the wrong thing. See `crate::staging`.
    pub pending_motion_path: Option<(buzz_geom::BezPath, ObjectId)>,
    /// Set when the user asks to quit.
    pub should_quit: bool,
    /// Transient message for the status bar.
    pub status: Option<String>,
    /// Memoised timeline waveforms, so the panel does not re-derive an envelope
    /// from raw PCM every frame. See [`Editor::waveforms`].
    waveform_cache: WaveformCache,
    /// Every symbol's resolved extent, memoised by document revision. Hit-testing
    /// and the selection chrome resolve an instance's bounds against this table
    /// rather than re-walking the library per object — the difference between a
    /// snappy click and a second's pause on a rig-heavy import.
    bounds_cache: std::cell::RefCell<Option<(u64, SymbolBounds)>>,
}

/// Every symbol's resolved extent, keyed by id. See [`Editor::symbol_bounds`].
type SymbolBounds = std::sync::Arc<std::collections::HashMap<buzz_scene::SymbolId, buzz_geom::Rect>>;

/// The waveform strip the timeline draws, cached against the document revision.
///
/// Two layers of memoisation, both from the patterns the codebase already uses:
/// the whole map is gated on [`buzz_scene::Scene::revision`] (the `cued_revision`
/// gate), so an unchanged document hands the panel the same `Arc`s every frame;
/// and each clip's per-frame levels are keyed by `(SoundId, fps)` with the clip's
/// pointer recorded, so even a *real* edit (which bumps the revision) reassembles
/// the map without recomputing an envelope whose sound did not change. The
/// pointer check catches a re-decode of the same id.
/// A memoised per-clip envelope: the clip's pointer (to catch a re-decode) and
/// the shared levels.
type CachedLevels = (usize, std::sync::Arc<Vec<f32>>);

#[derive(Default)]
struct WaveformCache {
    revision: Option<u64>,
    map: std::collections::BTreeMap<LayerId, buzz_ui::Waveform>,
    levels: std::collections::HashMap<(buzz_scene::SoundId, u64), CachedLevels>,
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
            clipboard: None,
            rig_panel: buzz_ui::RigPanelState::default(),
            templates: buzz_doc::TemplateLibrary::user(),
            new_document: buzz_ui::NewDocumentState::default(),
            about: buzz_ui::AboutState::default(),
            assets: buzz_doc::AssetLibrary::user(),
            assets_panel: buzz_ui::AssetPanelState::default(),
            actions: ActionsState::default(),
            export: buzz_ui::ExportState::default(),
            rig_gesture: None,
            light_panel: buzz_ui::LightPanelState::default(),
            filter_panel: buzz_ui::FilterPanelState::default(),
            motion_editor: buzz_ui::MotionEditorState::default(),
            camera_selected: false,
            workspace: buzz_ui::Workspace::load(),
            light_gesture: None,
            sound: crate::sound::SoundBank::new(stage_fps),
            scrub_until: None,
            ruler_marks: Vec::new(),
            lip_sync: buzz_ui::LipSyncState::default(),
            sound_placed: false,
            staging: buzz_ui::StagingState::default(),
            pending_motion_path: None,
            import_summary: None,
            should_quit: false,
            status: None,
            waveform_cache: WaveformCache::default(),
            bounds_cache: std::cell::RefCell::new(None),
        }
    }

    /// Every symbol's resolved extent, memoised by revision. Rebuilt only on an
    /// edit; a lookup for the whole document otherwise. See
    /// [`buzz_scene::Scene::symbol_bounds_table`].
    fn symbol_bounds(&self) -> SymbolBounds {
        let revision = self.doc.scene().revision();
        if let Some((r, table)) = self.bounds_cache.borrow().as_ref()
            && *r == revision
        {
            return std::sync::Arc::clone(table);
        }
        let table = std::sync::Arc::new(self.doc.scene().symbol_bounds_table());
        *self.bounds_cache.borrow_mut() = Some((revision, std::sync::Arc::clone(&table)));
        table
    }

    /// **Is a gesture in progress?**
    ///
    /// True from the press to the release of any drag that edits the document
    /// — a tool gesture, a bone being posed, a light being moved.
    ///
    /// This is the gate on work that is *derived* from the document but is not
    /// what the user is looking at while they drag. Moving artwork bumps the
    /// revision on every pointer move, and everything keyed on the revision
    /// treats that as "the document changed, redo your sums": the symbol use
    /// counts, the sound envelopes, the scrollable extent. None of them can
    /// have changed in a way that matters mid-drag, and recomputing them
    /// between frames is time taken directly out of the drag.
    pub fn is_gesturing(&self) -> bool {
        self.machine.is_active() || self.rig_gesture.is_some() || self.light_gesture.is_some()
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
                self.selection_bounds_drawn().map(|b| b.center())
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
    pub fn edit_object(&mut self, label: &'static str, id: ObjectId, f: impl FnMut(&mut Object)) {
        let at = self.edit_at();
        self.doc
            .edit(label, |scene| update_object(scene, at, id, f));
    }

    /// Put a document on screen and reset everything that belonged to the
    /// last one.
    ///
    /// Shared by "new from template" and anything else that swaps the document
    /// without replacing the whole `Editor`: the selection, the playhead and
    /// the view all belonged to the film that has gone.
    pub fn adopt(&mut self, doc: Document) {
        self.doc = doc;
        self.doc.mark_clean();
        self.selection = Selection::new();
        self.selection.ensure_active_layer(self.doc.scene());
        self.current_frame = 0;
        self.zoom_fit();
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

    /// **Play a short burst of the soundtrack while scrubbing the playhead.**
    ///
    /// Dragging the playhead over a scene with sound should be audible, the way
    /// it is in every editor — you find the beat by ear. This starts the audio
    /// at `frame` (or moves it there if it is already rolling from the drag) and
    /// arms a short deadline; [`Self::tick_scrub`] stops it once the drag pauses,
    /// so it is a scrub, not playback. A no-op while the transport is playing or
    /// the scene has no soundtrack.
    pub fn scrub_audio(&mut self, frame: u32) {
        if self.playback.playing {
            return;
        }
        let scene = self.doc.scene();
        if self.sound.stage_track(scene).is_none() {
            return;
        }
        if self.scrub_until.is_some() && self.sound.playing_frame().is_some() {
            self.sound.seek(frame);
        } else {
            self.sound.play(scene, frame);
        }
        self.scrub_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(140));
    }

    /// **Find the beats in the document's soundtrack** and mark them on the
    /// ruler, so action can be keyed to the music. Re-detects each time; clears
    /// the marks when there is no soundtrack.
    pub fn detect_beats(&mut self) {
        let scene = self.doc.scene();
        let fps = scene.stage().frame_rate.max(1.0);
        match self.sound.stage_track(scene) {
            Some((_, _, clip)) => {
                self.ruler_marks = buzz_audio::detect_beats(clip.as_ref(), fps);
                let n = self.ruler_marks.len();
                self.status = Some(format!("Found {n} beats — marked on the ruler"));
            }
            None => {
                self.ruler_marks.clear();
                self.status = Some("There is no soundtrack to find beats in".into());
            }
        }
    }

    /// **Lay the timeline out to the narration.**
    ///
    /// # The job this is
    ///
    /// A narrated film is timed by audio that already exists and cannot move.
    /// Every shot length and every cut is fitted to it, and fitting them by
    /// dragging keyframes against a waveform by eye is the single largest block
    /// of time in a week of that work. The soundtrack already says where the
    /// lines are; nothing was reading it.
    ///
    /// So: find the phrases ([`buzz_audio::detect_phrases`]), stretch the film
    /// to cover the narration, and put a **blank keyframe at the start of every
    /// line** on the active layer. What comes out is an ordinary timeline with
    /// ordinary keyframes, already the right length and already divided where
    /// the sentences are, waiting to be drawn on.
    ///
    /// # Why blank keyframes and not scenes
    ///
    /// A scene per line would be tidier to look at and wrong to work with: the
    /// soundtrack is cued on one scene, and cutting the film into thirty of
    /// them would leave twenty-nine with no audio under them. The narration
    /// stays whole and the timeline is divided instead.
    ///
    /// # Why the existing drawing is not disturbed
    ///
    /// A keyframe is only inserted where the layer does not already have one.
    /// Running this a second time — after a re-record, which is the usual
    /// reason — re-marks the ruler and adds the lines that moved, without
    /// throwing away anything drawn against the lines that did not.
    pub fn fit_to_narration(&mut self) {
        let scene = self.doc.scene();
        let fps = scene.stage().frame_rate.max(1.0);
        let Some((_, _, clip)) = self.sound.stage_track(scene) else {
            self.status = Some(
                "There is no soundtrack to fit to \u{2014} import the narration first".into(),
            );
            return;
        };

        let options = buzz_audio::PhraseOptions::default();
        let phrases = buzz_audio::detect_phrases(clip.as_ref(), fps, &options);
        if phrases.is_empty() {
            self.status =
                Some("Nothing in that soundtrack sounded like speech".into());
            return;
        }

        // The film has to be at least as long as the narration, or the lines
        // past the end are marked on a ruler that does not reach them.
        let needed = clip.duration_frames(fps).max(
            phrases.last().map(|p| p.end).unwrap_or(0),
        );
        let Some(layer) = self.active_layer() else {
            self.status = Some("There is no layer to lay the narration out on".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }

        let starts: Vec<u32> = phrases.iter().map(|p| p.start).collect();
        let mut added = 0usize;
        self.doc.edit("Fit to Narration", |scene| {
            scene.update_layer(layer, |l| {
                while l.frames.length() < needed {
                    l.frames.insert_frame(l.frames.length());
                }
                for &start in &starts {
                    // Only where there is not one already: a second run after a
                    // re-record must not discard what was drawn to the lines
                    // that did not move.
                    if !l.frames.is_keyframe(start) {
                        l.frames.insert_blank_keyframe(start);
                        added += 1;
                    }
                }
            });
        });

        // The ruler shows where the lines are, the way it shows beats.
        self.ruler_marks = starts;
        let lines = phrases.len();
        let seconds = needed as f64 / fps;
        self.status = Some(format!(
            "{lines} lines over {seconds:.1}s \u{2014} {added} keyframes added, \
             and the lines marked on the ruler"
        ));
    }

    /// **Read a subtitle file onto the timeline** as a caption layer.
    ///
    /// # Why this is the direction that matters
    ///
    /// The program can already hear *where* a narration speaks
    /// ([`Self::fit_to_narration`]) and cannot hear a word of *what* it says. A
    /// subtitle file is the one place both sit together, and nobody has to type
    /// it: every transcription tool writes this format, and so does YouTube.
    ///
    /// So this is the import that gives the document something it has never
    /// had — the words, on the frame they are spoken. Captions on the picture
    /// are the immediate use; handing a named character their own lines is the
    /// one worth having.
    ///
    /// # A layer of its own
    ///
    /// Captions are not artwork and do not want to be mixed in with it: they
    /// are re-imported wholesale every time the narration is re-cut, and
    /// throwing away a layer is a great deal safer than picking text objects
    /// out of a drawing. So a fresh **Captions** layer each time, and the old
    /// one is left alone rather than merged into.
    pub fn import_captions(&mut self, path: &std::path::Path) -> anyhow::Result<usize> {
        let text = std::fs::read_to_string(path)
            .or_else(|_| std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()))?;
        let caps = buzz_doc::srt::parse(&text);
        if caps.is_empty() {
            anyhow::bail!("nothing in that file looked like subtitles");
        }

        let fps = self.doc.scene().stage().frame_rate.max(1.0);
        let stage = self.doc.scene().stage().stage_rect();
        // Down where a subtitle goes, and sized to the picture rather than to a
        // number: the same file on a 4K stage should not arrive as a speck.
        let size = (stage.height() / 20.0).max(8.0);
        let baseline = stage.y1 - stage.height() * 0.12;
        let colour = self.style.fill_color;

        // The words are shaped before the document is touched, so a font that
        // cannot be found leaves no half-made layer behind.
        let mut drawn: Vec<(
            std::ops::Range<u32>,
            buzz_geom::BezPath,
            buzz_scene::TextData,
            f64,
            Option<String>,
        )> = Vec::new();
        for cue in &caps.cues {
            let data = buzz_scene::TextData {
                content: cue.text.clone(),
                size,
                font: None,
                style: buzz_text::FontStyle::REGULAR,
                align: buzz_text::TextAlign::Centre,
            };
            let Some(path) = buzz_text::outline_styled(
                &cue.text,
                size,
                None,
                data.style,
                data.align,
            ) else {
                continue;
            };
            let (width, _) = buzz_text::measure_styled(&cue.text, size, None, data.style);
            drawn.push((cue.frames(fps), path, data, width, cue.speaker.clone()));
        }
        if drawn.is_empty() {
            anyhow::bail!("no font available to draw those captions with");
        }

        let last = drawn.iter().map(|(r, ..)| r.end).max().unwrap_or(1);
        let mut layer = LayerId(0);
        self.doc.edit("Import Captions", |scene| {
            layer = scene.add_layer("Captions", LayerKind::Normal);
            scene.update_layer(layer, |l| {
                while l.frames.length() < last {
                    l.frames.insert_frame(l.frames.length());
                }
            });
            for (range, path, data, width, speaker) in &drawn {
                scene.update_layer(layer, |l| {
                    l.frames.insert_blank_keyframe(range.start);
                    // **And a blank one where it ends**, or the last caption of
                    // a scene hangs on the screen to the end of the film. Only
                    // where the next one does not already start there.
                    if !l.frames.is_keyframe(range.end) && range.end < l.frames.length() {
                        l.frames.insert_blank_keyframe(range.end);
                    }
                    // **The speaker goes on the keyframe's label.**
                    //
                    // Which is a frame label — Animate's own idea — and so it
                    // is visible in the timeline, editable by hand when the
                    // detection got somebody wrong, and saved with the
                    // document without a new field. It is also what
                    // `Lip Sync from Captions` reads to know whose line this
                    // is; a name buried in a struct nobody can see would have
                    // meant a mis-detected speaker was unfixable.
                    if let Some(who) = speaker {
                        l.frames.set_label(range.start, Some(who.clone()));
                    }
                });
                if let Some(id) = scene.add_shape_at(
                    layer,
                    range.start,
                    ShapeData::filled(path.clone(), colour),
                ) {
                    scene.update_object_at(range.start, id, |o| {
                        // **Centred by measurement, not by the align setting.**
                        //
                        // `TextAlign::Centre` lines the *rows of a block* up
                        // with each other; it does not put the block on a
                        // point. Trusting it to did exactly what you would
                        // expect: every caption started at the middle of the
                        // stage and ran off the right-hand edge. The figure in
                        // the guide is what showed it.
                        o.transform = Affine::translate((
                            stage.center().x - width / 2.0,
                            baseline,
                        ));
                        o.text = Some(data.clone());
                    });
                }
            }
        });
        self.doc.end_gesture();

        self.select_layer(layer);
        let n = caps.cues.len();
        let cast = caps.speakers();
        self.status = Some(match (caps.skipped, cast.len()) {
            (0, 0) => format!("{n} captions on their own layer"),
            (0, _) => format!("{n} captions, spoken by {}", cast.join(", ")),
            (s, 0) => format!("{n} captions \u{2014} {s} blocks could not be read"),
            (s, _) => format!(
                "{n} captions spoken by {} \u{2014} {s} blocks could not be read",
                cast.join(", ")
            ),
        });
        Ok(n)
    }

    /// **Lip-sync every character from the caption layer**, each over their own
    /// lines and nobody else's.
    ///
    /// # The gap this closes
    ///
    /// Lip sync could already turn a soundtrack into mouth shapes, but only
    /// *one mouth against the whole track*: run it on Ana and she mouths Ben's
    /// lines too. That is fine for a monologue and useless for a conversation,
    /// which is most of what a story is.
    ///
    /// What was missing was never the analysis. It was knowing **who is
    /// speaking and when** — and an imported subtitle file says exactly that.
    /// So this reads the caption layer's frame labels for the cast, slices the
    /// viseme track to each line's own frames, and writes each slice onto that
    /// character's own layer. Two people talking, each animated only while they
    /// are talking.
    ///
    /// # How a speaker is matched to a mouth
    ///
    /// **By name.** A speaker called `Ana` drives the library symbol whose name
    /// matches `Ana` — exactly, or as a word inside it, so `Ana Mouth` and
    /// `Ana_mouth` both work — and the keyframes go on a layer of that name,
    /// made if there is not one.
    ///
    /// Matching by name rather than asking is what makes this worth running: a
    /// dialog with a row per speaker is the same work as doing it by hand once
    /// there are more than about three of them. A speaker with no matching
    /// symbol is reported by name rather than skipped in silence, because the
    /// fix — rename the symbol — is one the message can state outright.
    ///
    /// # The mouth closes at the end of every line
    ///
    /// A rest shape is appended to each slice. Without it the last shape of a
    /// line holds until the character's next line, which leaves them frozen
    /// mid-vowel through everybody else's dialogue.
    pub fn lip_sync_from_captions(&mut self) -> anyhow::Result<usize> {
        let Some(captions) = self.active_layer() else {
            anyhow::bail!("there is no layer to take captions from");
        };
        let scene = self.doc.scene();
        let fps = scene.stage().frame_rate.max(1.0);

        let Some((_, sound_start, clip)) = self.sound.stage_track(scene) else {
            anyhow::bail!("there is no soundtrack to sync to");
        };

        // The lines, from the caption layer: a labelled keyframe that holds
        // text is somebody's line, and it runs to the next keyframe.
        let Some(layer) = scene.layers().get(captions) else {
            anyhow::bail!("there is no layer to take captions from");
        };
        let starts: Vec<u32> = layer.frames.keyframes().iter().map(|k| k.start).collect();
        let length = layer.frames.length();
        let mut lines: Vec<(String, std::ops::Range<u32>)> = Vec::new();
        for (i, key) in layer.frames.keyframes().iter().enumerate() {
            let Some(who) = key.label.clone() else { continue };
            let has_text = layer
                .frames
                .resolved_at(key.start)
                .iter()
                .any(|o| o.text.is_some());
            if !has_text {
                continue;
            }
            let end = starts.get(i + 1).copied().unwrap_or(length);
            lines.push((who, key.start..end.max(key.start + 1)));
        }
        if lines.is_empty() {
            anyhow::bail!(
                "no line on this layer names a speaker \u{2014} import captions that say \
                 who is talking, or label the keyframes yourself"
            );
        }

        // The whole track once, then sliced per line. Analysing per line would
        // re-window the audio at every cue boundary and give a different answer
        // at the seams than a single pass does.
        let track = buzz_audio::analyse_visemes(
            clip.as_ref(),
            fps,
            &buzz_audio::LipSyncOptions::default(),
        );
        if track.is_empty() {
            anyhow::bail!("that soundtrack analysed to nothing");
        }

        // Each speaker's mouth symbol, matched by name.
        let needed = buzz_audio::Viseme::COUNT;
        let mut cast: Vec<String> = Vec::new();
        for (who, _) in &lines {
            if !cast.iter().any(|c| c.eq_ignore_ascii_case(who)) {
                cast.push(who.clone());
            }
        }
        let mouth_for = |who: &str| -> Option<buzz_scene::SymbolId> {
            scene
                .library()
                .iter()
                .filter(|s| s.length() >= needed && name_mentions(&s.name, who))
                // The closest match wins, so `Ana` beats `Ana and Ben` when
                // both exist.
                .min_by_key(|s| s.name.chars().count())
                .map(|s| s.id)
        };

        let mut jobs: Vec<(String, buzz_scene::SymbolId, Vec<std::ops::Range<u32>>)> = Vec::new();
        let mut unmatched: Vec<String> = Vec::new();
        for who in &cast {
            match mouth_for(who) {
                Some(mouth) => {
                    let spans = lines
                        .iter()
                        .filter(|(w, _)| w.eq_ignore_ascii_case(who))
                        .map(|(_, r)| r.clone())
                        .collect();
                    jobs.push((who.clone(), mouth, spans));
                }
                None => unmatched.push(who.clone()),
            }
        }
        if jobs.is_empty() {
            anyhow::bail!(
                "no mouth symbol matches {} \u{2014} name a symbol after the speaker \
                 (at least {needed} frames, one per shape)",
                unmatched.join(" or ")
            );
        }

        // Where a mouth sits, if the character's layer already has one; the
        // middle of the stage otherwise, to be dragged into place.
        let stage_centre = scene.stage().stage_rect().center();

        let mut written = 0usize;
        let mut done: Vec<String> = Vec::new();
        self.doc.edit("Lip Sync from Captions", |scene| {
            for (who, mouth, spans) in &jobs {
                let existing = scene
                    .layers()
                    .iter()
                    .find(|l| l.name.eq_ignore_ascii_case(who))
                    .map(|l| l.id);
                let target = match existing {
                    Some(id) => id,
                    None => scene.add_layer(who.clone(), LayerKind::Normal),
                };

                // Keep whatever placement the character's mouth already has, so
                // running this twice does not move it back to the middle.
                let placement = scene
                    .layers()
                    .get(target)
                    .and_then(|l| {
                        let frames: Vec<u32> =
                            l.frames.keyframes().iter().map(|k| k.start).collect();
                        frames.into_iter().find_map(|at| {
                            l.frames
                                .resolved_at(at)
                                .iter()
                                .find(|o| matches!(o.kind, ObjectKind::Instance(_)))
                                .map(|o| o.transform)
                        })
                    })
                    .unwrap_or_else(|| Affine::translate(stage_centre.to_vec2()));

                for span in spans {
                    // The line's own frames, in the track's own numbering.
                    let from = span.start.saturating_sub(sound_start) as usize;
                    let to = (span.end.saturating_sub(sound_start) as usize).min(track.len());
                    if from >= to {
                        continue;
                    }
                    let mut frames = track.frames[from..to].to_vec();
                    // **Closed at the end of the line.** Without it the last
                    // shape holds until this character speaks again, leaving
                    // them frozen mid-vowel through everybody else's dialogue.
                    frames.push(buzz_audio::Viseme::Rest);
                    let slice = buzz_audio::VisemeTrack { frames, fps };

                    let report = crate::lipsync::write_track(
                        scene,
                        &slice,
                        span.start,
                        target,
                        *mouth,
                        placement,
                    );
                    written += report.keyframes as usize;
                }
                done.push(who.clone());
            }
        });
        self.doc.end_gesture();

        self.status = Some(match unmatched.is_empty() {
            true => format!("Lip sync: {written} keyframes for {}", done.join(", ")),
            false => format!(
                "Lip sync: {written} keyframes for {} \u{2014} no mouth symbol named {}",
                done.join(", "),
                unmatched.join(" or ")
            ),
        });
        Ok(written)
    }

    /// **Write the active layer's captions back out as `.srt`.**
    ///
    /// # Why the active layer and not every piece of text in the film
    ///
    /// Because a title card is text and a logo is text, and neither is a
    /// caption. There is no property that separates them — only which layer the
    /// animator put them on — so the rule is the one the animator can see and
    /// control. [`Self::import_captions`] leaves its own layer selected, so the
    /// round trip needs no thought.
    ///
    /// A cue runs from its keyframe to the next keyframe on that layer, which
    /// is exactly how the import laid it down and exactly what the timeline
    /// shows.
    pub fn export_captions(&mut self, path: &std::path::Path) -> anyhow::Result<usize> {
        let Some(layer) = self.active_layer() else {
            anyhow::bail!("there is no layer to take captions from");
        };
        let scene = self.doc.scene();
        let fps = scene.stage().frame_rate.max(1.0);
        let Some(l) = scene.layers().get(layer) else {
            anyhow::bail!("there is no layer to take captions from");
        };

        // Every keyframe that holds text, and where the next keyframe is.
        let starts: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).collect();
        let length = l.frames.length();
        let mut cues: Vec<buzz_doc::srt::Cue> = Vec::new();
        for (i, &start) in starts.iter().enumerate() {
            let end = starts.get(i + 1).copied().unwrap_or(length);
            // A keyframe may hold several text objects; they are one caption,
            // in the order they are painted, which is how a two-line subtitle
            // built by hand would read.
            let lines: Vec<String> = l
                .frames
                .resolved_at(start)
                .iter()
                .filter_map(|o| o.text.as_ref().map(|t| t.content.clone()))
                .filter(|s| !s.trim().is_empty())
                .collect();
            if lines.is_empty() {
                continue;
            }
            let to_ms = |frame: u32| ((frame as f64 / fps) * 1000.0).round() as u64;
            cues.push(buzz_doc::srt::Cue {
                start_ms: to_ms(start),
                end_ms: to_ms(end.max(start + 1)),
                text: lines.join("\n"),
                speaker: None,
            });
        }

        if cues.is_empty() {
            anyhow::bail!(
                "there is no text on the active layer \u{2014} select the caption layer first"
            );
        }
        std::fs::write(path, buzz_doc::srt::write(&cues))?;
        let n = cues.len();
        self.status = Some(format!("Wrote {n} captions to {}", path.display()));
        Ok(n)
    }

    /// Stop the scrub burst once the drag has paused. Call once per frame.
    pub fn tick_scrub(&mut self) {
        if self.playback.playing {
            self.scrub_until = None;
            return;
        }
        if let Some(deadline) = self.scrub_until {
            if std::time::Instant::now() >= deadline {
                self.sound.stop();
                self.scrub_until = None;
            }
        }
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
            selection_bounds: self.selection_bounds_drawn(),
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
        let paint = &shape.fill.as_ref()?.paint;

        // **A bitmap or texture fill has the same grips**, because it is the
        // same kind of matrix: Animate's Gradient Transform tool adjusts a
        // bitmap fill too, and this is how a texture is scaled and turned on the
        // stage rather than by typing numbers. Reported as a *linear* gradient
        // so no focus grip is drawn — an image has no hot spot.
        let (local, kind) = match paint {
            buzz_scene::Paint::Gradient(g) => (g.handles(), g.kind),
            buzz_scene::Paint::Image(image) => {
                (image.handles(), buzz_scene::GradientKind::Linear)
            }
            _ => return None,
        };
        Some((
            buzz_scene::GradientHandles {
                center: object.transform * local.center,
                end: object.transform * local.end,
                width: object.transform * local.width,
                focus: object.transform * local.focus,
            },
            kind,
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
                // A bitmap or texture fill is transformed by the same three
                // grips — see `ImageFill::handles`.
                if let Paint::Image(image) = &mut fill.paint {
                    match grip {
                        crate::tools::GradientGrip::Center => image.set_center(local),
                        crate::tools::GradientGrip::End => image.set_end(local),
                        crate::tools::GradientGrip::Width => image.set_width_handle(local),
                        // An image has no hot spot, and no grip is drawn for one.
                        crate::tools::GradientGrip::Focus => {}
                    }
                    return;
                }
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

    /// **The extra placements symmetry makes**, in the space a tool draws in.
    ///
    /// Empty when symmetry is off, so the ordinary path costs one comparison.
    /// The same list the commit uses is what the preview is drawn through, and
    /// that is the point of it being reachable from outside: a mirrored stroke
    /// that only appears on release is the stroke changing under the pointer,
    /// which is the one thing a drawing preview must never do.
    pub fn symmetry_mirrors(&self) -> Vec<Affine> {
        symmetry_transforms(self.style.symmetry, self.doc.scene().stage().size)
    }

    /// Will [`Self::preview`] paint into the Vello scene rather than the
    /// chrome?
    ///
    /// Answered without building the preview. The artwork itself is built
    /// once a frame, by the stage; this question is asked twice more — by the
    /// encode-reuse check and by the chrome, which draws nothing for such a
    /// preview — and a brush preview is rebuilt on every pointer move, so
    /// answering it by building the artwork two extra times per frame is
    /// exactly the lag the preview budgets exist to prevent.
    pub fn preview_paints_into_scene(&self) -> bool {
        if !self.machine.painting_preview() {
            return false;
        }
        // A pattern brush with no source shape falls back to a chrome-drawn
        // centreline — the one brush preview that is *not* painted artwork —
        // and skipping the chrome for it would leave the drag invisible.
        match self.style.brush.kind {
            buzz_ui::BrushKind::Pattern | buzz_ui::BrushKind::Art => {
                self.style.brush.pattern_path().is_some()
            }
            _ => true,
        }
    }

    /// Document-space tolerance equivalent to a few screen pixels.
    pub(crate) fn pick_tolerance(&self) -> f64 {
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
        let scene = self.doc.scene();
        // The workspace view first — where the user has scrolled and zoomed.
        let doc = self.camera.screen_to_doc(screen);

        // **Then back through the document camera.**
        //
        // The stage is drawn `camera * place * follows * object`
        // (`buzz_render::document`), and a click has to be carried back through
        // every one of those before it can be tested against geometry. This
        // step was missing: with a shot framed anywhere but dead centre, the
        // artwork was drawn where the camera put it while the click was tested
        // where the artwork would have been with no camera at all — so clicking
        // a character selected the one beside it, or nothing, by exactly how
        // far the camera had been moved.
        //
        // At depth zero, which is the plane the editor works on. Layers pushed
        // into the distance are carried the rest of the way by
        // `Scene::view_to_layer`, which is written to take a point in *this*
        // space — the comment there about "the space the rest of the editor
        // already works in" is only true once this has happened.
        //
        // Identity for a document with the camera off, which is every document
        // that has not asked for one.
        let doc = scene
            .camera_projection_at_depth(self.current_frame, 0.0)
            .and_then(|shot| shot.inverse())
            .and_then(|back| back.map_point(doc))
            // A shot with no inverse cannot be undone; the point as it stands
            // is the honest fallback.
            .unwrap_or(doc);

        match buzz_scene::invert_affine(scene.edit_place()) {
            Some(back) => back * doc,
            // A collapsed place cannot be undone; the document's own space is
            // the honest fallback.
            None => doc,
        }
    }

    /// The inverse of [`Self::screen_to_edit`]'s document half: from the space
    /// the tools work in, back onto the stage.
    ///
    /// A tool is handed points that have been carried back through the
    /// document camera and through the place of any symbol opened for editing,
    /// so that what it builds lands in the right coordinates when it is
    /// committed. Anything drawn from those points *before* they are committed
    /// — a live preview — has to be carried forward again, or it is drawn
    /// wherever those two transforms would have moved it.
    ///
    /// Identity for a document on its main timeline with no camera, which is
    /// most of them.
    pub fn edit_to_stage(&self) -> buzz_geom::Projection {
        let scene = self.doc.scene();
        let shot = scene
            .camera_projection_at_depth(self.current_frame, 0.0)
            .unwrap_or(buzz_geom::Projection::IDENTITY);
        shot.pre_affine(scene.edit_place())
    }

    pub fn pointer_down(&mut self, screen: Point, mods: Mods) {
        self.pointer_down_at(screen, mods, None, None);
    }

    /// A pointer press the window has timed. See [`Self::pointer_move_at`].
    pub fn pointer_down_at(
        &mut self,
        screen: Point,
        mods: Mods,
        time: Option<f64>,
        pressure: Option<f64>,
    ) {
        let doc = self.screen_to_edit(screen);
        let doc = self.snap_for_tool(doc);

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
        let selection_bounds = self.selection_bounds_drawn();
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
        self.machine
            .pointer_down_at(doc, screen, mods, &ctx, time, pressure);
    }

    pub fn pointer_move(&mut self, screen: Point, mods: Mods) {
        self.pointer_move_at(screen, mods, None, None);
    }

    /// A pointer move the window has timed.
    ///
    /// See [`crate::tools::ToolMachine::pointer_move_at`]: several moves arrive
    /// per frame and a brush's width is read off how far apart they were.
    pub fn pointer_move_at(
        &mut self,
        screen: Point,
        mods: Mods,
        time: Option<f64>,
        pressure: Option<f64>,
    ) {
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
        let doc = self.snap_for_tool(self.screen_to_edit(screen));
        let action = self
            .machine
            .pointer_move_at(doc, screen, mods, time, pressure);
        self.apply(action);
    }

    pub fn pointer_up(&mut self, screen: Point) {
        self.pointer_up_at(screen, None, None);
    }

    /// A pointer release the window has timed. See [`Self::pointer_move_at`].
    pub fn pointer_up_at(&mut self, screen: Point, time: Option<f64>, pressure: Option<f64>) {
        if self.rig_gesture.is_some() {
            self.finish_rig_gesture(self.screen_to_edit(screen));
            self.doc.end_gesture();
            return;
        }
        if let Some(gesture) = self.light_gesture.take() {
            let doc = self.screen_to_edit(screen);
            let frame = self.current_frame;
            self.doc.edit(gesture.label(), |scene| {
                crate::lights::drag(scene, gesture, doc);
                // If the light is already animated, the drag re-keys it at the
                // playhead so the new placement lands on this frame rather than
                // silently editing a base the animation overrides — the same
                // auto-key a camera drag does.
                if let Some(light) = scene.lights_mut().get_mut(gesture.light())
                    && light.track.as_ref().is_some_and(|t| t.animates())
                {
                    let key = buzz_scene::LightKey::from_light(frame, light);
                    if let Some(track) = light.track.as_mut() {
                        track.set_key(key);
                    }
                }
            });
            // One drag, one undo step — as with every other gesture.
            self.doc.end_gesture();
            return;
        }
        let doc = self.snap_for_tool(self.screen_to_edit(screen));

        // Built from disjoint fields rather than via `tool_context`, which
        // would borrow all of `self` and conflict with `&mut self.machine`.
        let anchors = self.selected_anchors();
        let selection_bounds = self.selection_bounds_drawn();
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
        let action = self.machine.pointer_up_at(doc, screen, &ctx, time, pressure);

        self.apply(action);
        self.doc.end_gesture();
    }

    /// Snap a point for the *active* tool, which for a freehand tool means not
    /// snapping it at all.
    ///
    /// **A drawn line is not a placed one.** Snapping exists so that a
    /// rectangle's corner meets the guide the animator put there, and it does
    /// that by yanking a point up to eight screen pixels sideways onto the
    /// nearest edge, guide or grid line. That is the right answer for one
    /// point chosen deliberately, and the wrong one for the hundreds a brush
    /// stroke is made of: every sample that passes within eight pixels of a
    /// shape already on the stage is pulled onto its bounding box, so a stroke
    /// drawn across a drawing comes out with flats and steps in it where the
    /// hand drew a curve, and object snapping is on by default. Animate snaps
    /// what you place and never what you draw.
    ///
    /// The pen, which places anchors one at a time, still snaps — that is a
    /// placed point in every sense.
    fn snap_for_tool(&self, point: Point) -> Point {
        match self.machine.tool() {
            ToolId::Brush | ToolId::Pencil | ToolId::Eraser | ToolId::Lasso => point,
            _ => self.snap(point),
        }
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
                        // Selecting an object makes its layer the active layer, so
                        // the timeline highlights the layer the selection lives on
                        // — and, with the playhead already on the current frame,
                        // its frame too. This is Animate's behaviour: click a thing
                        // on the stage and its row lights up in the timeline.
                        if let Some((layer, _)) = self.doc.scene().find_object(id) {
                            self.selection.set_active_layer(Some(layer));
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

            ToolAction::PickInRegion { region, additive } => self.pick_in_region(&region, additive),

            ToolAction::PaintRaster { canvas, brush } => self.paint_raster(&canvas, &brush),

            ToolAction::AddArtwork { pieces, label } => self.add_artwork(pieces, label),

            ToolAction::AddArtworkFrames { frames, label } => {
                self.add_artwork_frames(frames, label);
            }

            ToolAction::WandAt { point, additive } => self.wand_at(point, additive),

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
                // Clicking a shape that already has a fill recolours it — the
                // gradient is refitted to that shape's bounds. Clicking anywhere
                // else (an empty region between lines, or on a bare outline)
                // floods the enclosed area with a new fill, which is what a paint
                // bucket is actually for.
                let recolour = self
                    .object_at(point, tolerance)
                    .filter(|id| self.shape_has_fill(*id));
                if let Some(id) = recolour {
                    let at = self.edit_at();
                    self.doc.edit("Paint Bucket", |scene| {
                        update_shape(scene, at, id, |s| {
                            let bounds = buzz_geom::Shape::bounding_box(&s.path);
                            if let Some(paint) = style.fill_for_new_shape(bounds) {
                                s.fill = Some(FillSpec {
                                    paint,
                                    rule: buzz_geom::FillMode::NonZero,
                                    swatch: None,
                                });
                            }
                        });
                    });
                } else {
                    self.bucket_flood(point, &style);
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
                                swatch: None,
                            })
                        });
                    });
                }
            }

            ToolAction::SampleColor { point } => {
                // The topmost shape under the point, found through instances and
                // groups — an imported file is all instances, so the old
                // top-level-shape-only check sampled nothing. A gradient is taken
                // as the one colour it stands for rather than loading the ramp.
                if let Some((color, is_fill)) = self.sampled_paint_at(point) {
                    if is_fill {
                        self.style.fill_color = color;
                        self.style.fill_enabled = true;
                    } else {
                        self.style.stroke_color = color;
                        self.style.stroke_enabled = true;
                    }
                    self.style.remember(color);
                }
            }

            ToolAction::PanView { delta_screen } => self.camera.pan_screen(delta_screen),

            ToolAction::MoveCamera { delta_screen } => {
                // Screen pixels into document units, the same conversion
                // `Camera::pan_screen` makes for the view: undo the view's
                // rotation, then divide by its magnification, so a drag of an
                // inch moves the shot the same distance whatever the zoom.
                let unrotated =
                    Affine::rotate(-self.camera.rotation) * delta_screen.to_point();
                let zoom = self.camera.zoom;
                if zoom.is_finite() && zoom > 0.0 {
                    self.nudge_camera(unrotated.to_vec2() / zoom);
                }
            }

            ToolAction::ZoomView { factor, at_screen } => {
                self.camera.zoom_by_at(factor, at_screen);
            }

            ToolAction::Deselect => self.selection.clear(),

            ToolAction::SetTransformPoint { at } => self.set_pivot(at),
            ToolAction::ResetTransformPoint => self.reset_pivot(),
            ToolAction::DragGradient { grip, to } => self.drag_gradient(grip, to),
            ToolAction::DrawMotionPath { path } => self.begin_motion_path(path),
            ToolAction::PlaceText { at } => self.place_text(at),
        }
    }

    // -- document operations -------------------------------------------------

    /// The layer new artwork goes on, or `None` if none is usable.
    fn active_layer(&mut self) -> Option<LayerId> {
        let scene = self.doc.scene().clone();
        self.selection.ensure_active_layer(&scene)
    }

    /// Does this object carry a fill? The paint bucket recolours a filled shape
    /// and floods everything else.
    fn shape_has_fill(&self, id: ObjectId) -> bool {
        matches!(
            self.doc.scene().find_object(id),
            Some((_, o)) if matches!(&o.kind, ObjectKind::Shape(s) if s.fill.is_some())
        )
    }

    /// Flood-fill the enclosed region under `point` on the active layer.
    ///
    /// The boundaries are the layer's own shapes; the seed and the boundaries are
    /// taken in the layer's local space — the space the shape paths are stored in
    /// — so a layer moved by depth or parenting still fills where it is drawn.
    fn bucket_flood(&mut self, point: Point, style: &DrawStyle) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer available to fill".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }
        if !style.fill_enabled {
            self.status = Some("Set a fill colour first".into());
            return;
        }

        let frame = self.current_frame;
        let scene = self.doc.scene();

        // The click, moved into the active layer's local space — the reverse of
        // the depth and parenting that place the layer's artwork on screen.
        let Some(depth) = scene.layers().get(layer).map(|l| l.depth) else {
            return;
        };
        let Some(local) = scene.view_to_layer(frame, depth, point) else {
            return;
        };
        let seed = match invert(scene.layers().inherited_transform(layer, frame)) {
            Some(back) => back * local,
            None => local,
        };

        // Every shape on the layer at this frame becomes a wall.
        let mut boundaries = Vec::new();
        if let Some(layer_ref) = scene.layers().get(layer) {
            for object in layer_ref.frames.resolved_at(frame).iter() {
                collect_bucket_boundaries(object, buzz_geom::Affine::IDENTITY, &mut boundaries);
            }
        }

        let Some(path) = buzz_scene::fill_region(&boundaries, seed, style.gap_size) else {
            self.status = Some(
                "Nothing enclosed to fill here — raise the Gap Size if the outline has a gap"
                    .into(),
            );
            return;
        };

        let bounds = buzz_geom::Shape::bounding_box(&path);
        let Some(paint) = style.fill_for_new_shape(bounds) else {
            return;
        };
        let shape = ShapeData {
            path,
            fill: Some(FillSpec {
                paint,
                rule: buzz_scene::bucket::FILL_RULE,
                swatch: None,
            }),
            stroke: None,
            blend: buzz_scene::PaintBlend::default(),
        };

        let auto = self.auto_keyframe;
        self.doc.edit("Paint Bucket", |scene| {
            if auto {
                scene.ensure_keyframe(layer, frame);
            }
            scene.add_shape_behind_at(layer, frame, shape);
        });
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
        let mirrors = self.symmetry_mirrors();
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
            // Symmetry drawing lays down the mirror copies first (so the stroke
            // the user is watching stays the selected one), each a reflection or
            // rotation of the drawn shape about the stage centre.
            for t in &mirrors {
                let copy = mirror_shape(&shape, *t);
                if merge {
                    merge_shape_into_layer(scene, layer, frame, copy);
                } else {
                    scene.add_shape_at(layer, frame, copy);
                }
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

    /// **Use this interface theme**, and remember it.
    ///
    /// Both ways in land here — the Window menu picking one by name, and the
    /// shortcut stepping to the next — so the theme is stored, persisted and
    /// reported in one place rather than in each of them.
    fn set_theme(&mut self, theme: buzz_ui::theme::Theme) {
        buzz_ui::theme::set_theme(theme);
        self.workspace.theme = theme;
        self.workspace.save();
        // The context is restyled by the shell, which owns it; this records
        // what the chrome should now be.
        self.restyle = true;
        self.status = Some(format!("{} interface", theme.label()));
    }

    /// **Place a text object** at `at`. The glyph outlines are shaped here (the
    /// editor holds the font) and stored as an ordinary filled shape; the string
    /// rides along on `Object::text` so it stays editable.
    pub fn place_text(&mut self, at: Point) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer available to place text on".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }
        const DEFAULT: &str = "Text";
        let size = 48.0;
        let Some(path) = buzz_text::outline(DEFAULT, size, None) else {
            self.status = Some("No font available to draw text with".into());
            return;
        };

        let color = self.style.fill_color;
        let frame = self.current_frame;
        let auto = self.auto_keyframe;
        let mut created: Option<ObjectId> = None;
        self.doc.edit("Text", |scene| {
            if auto {
                scene.ensure_keyframe(layer, frame);
            }
            created = scene.add_shape_at(layer, frame, ShapeData::filled(path.clone(), color));
            if let Some(id) = created {
                scene.update_object_at(frame, id, |o| {
                    o.transform = Affine::translate(at.to_vec2());
                    o.text = Some(buzz_scene::TextData::new(DEFAULT, size, None));
                });
            }
        });
        if let Some(id) = created {
            self.selection.select_one(id);
            self.status = Some("Text placed \u{2014} edit it in Properties".into());
        }
    }

    /// **Re-type a text object**: re-shape its glyphs from `content`/`size`/`font`
    /// and keep the string on it. One undo step across a typing burst (no
    /// `end_gesture`, so consecutive edits coalesce).
    pub fn set_text(&mut self, id: ObjectId, content: String, size: f64, font: Option<String>) {
        let style = self.text_of(id).map(|t| t.style).unwrap_or_default();
        let align = self.text_of(id).map(|t| t.align).unwrap_or_default();
        self.set_text_styled(id, content, size, font, style, align);
    }

    /// The text data on an object, if it is text.
    pub fn text_of(&self, id: ObjectId) -> Option<buzz_scene::TextData> {
        self.doc
            .scene()
            .find_object(id)
            .and_then(|(_, o)| o.text.clone())
    }

    /// **Re-type a text object in a chosen cut and alignment.**
    ///
    /// The glyphs are shaped again from every part of the choice — the words,
    /// the size, the family, the cut, the alignment — because all five change
    /// the outlines, and the outlines *are* the artwork. One undo step across a
    /// typing burst, as [`Self::set_text`].
    pub fn set_text_styled(
        &mut self,
        id: ObjectId,
        content: String,
        size: f64,
        font: Option<String>,
        style: buzz_scene::FontStyle,
        align: buzz_scene::TextAlign,
    ) {
        let path = buzz_text::outline_styled(&content, size, font.as_deref(), style, align)
            .unwrap_or_default();
        self.doc.edit("Edit Text", |scene| {
            scene.update_object_across(0, u32::MAX, id, |o| {
                if let ObjectKind::Shape(shape) = &mut o.kind {
                    shape.path = path.clone();
                }
                o.text = Some(buzz_scene::TextData {
                    content: content.clone(),
                    size,
                    font: font.clone(),
                    style,
                    align,
                });
            });
        });
    }

    fn transform_selection(&mut self, transform: Affine, label: &'static str) {
        if self.selection.is_empty() {
            return;
        }

        // **The transformation point travels with the artwork.**
        //
        // For several objects at once it is the middle of what is selected,
        // worked out afresh each time it is asked for — and the middle of a
        // *union* of boxes is not where it was once the artwork has turned,
        // because a union of boxes is not symmetric about its own centre the
        // way one box is. So the point wandered off during a rotation: the one
        // control whose whole job is to be the fixed point of the turn was the
        // thing that moved.
        //
        // Carried through the same transform the artwork is, which leaves it
        // exactly where it was under a rotation about itself and moves it with
        // the selection under everything else.
        let ids = self.selection.ids();
        if ids.len() > 1
            && let Some(at) = self.pivot()
        {
            self.group_pivot = Some((ids.clone(), transform * at));
        }

        // **The gesture is in view space; an object's transform is not.**
        //
        // Layer parenting draws a child through its parent's motion, so a drag
        // of 100 to the right on a child whose parent is turned a quarter turn
        // must not add 100 to the child's own x — that lands the artwork 100
        // *down*, along the parent's rotated axis, and dragging a rigged limb
        // felt like fighting the rig. Carried back through the same transform
        // it will be drawn with, so what the pointer did is what the artwork
        // does.
        //
        // Worked out per object, because a selection can span layers with
        // different parents, and before the edit so the scene is not borrowed
        // while it is being changed. With no parenting `follows` is the
        // identity and this is exactly `transform * o.transform`.
        let frame = self.current_frame;
        let scene = self.doc.scene();
        let moves: Vec<(ObjectId, Affine)> = self
            .selection
            .ids()
            .into_iter()
            .filter_map(|id| {
                let (layer, _) = scene.find_object(id)?;
                let follows = scene.layers().inherited_transform(layer, frame);
                let local = match invert(follows) {
                    Some(back) => back * transform * follows,
                    // A parent collapsed to nothing cannot be undone; the
                    // object's own space is the honest fallback.
                    None => transform,
                };
                Some((id, local))
            })
            .collect();

        let at = self.edit_at();
        self.doc.edit(label, |scene| {
            for (id, local) in moves {
                update_object(scene, at, id, |o| o.transform = local * o.transform);
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

        // **Symmetry rubs where it draws.** A mirror you can draw through but
        // not correct through is worse than no mirror: the far half of the
        // drawing becomes read-only the moment you make a mistake on it.
        //
        // Each reflection cuts in its own pass rather than being merged into
        // one cutter, because a reflection reverses a path's orientation — a
        // stroke crossing the axis would have its two halves cancel under the
        // nonzero rule and rub out nothing at all where it mattered most.
        let cutters: Vec<BezPath> = std::iter::once(cutter.clone())
            .chain(self.symmetry_mirrors().into_iter().map(|t| t * cutter.clone()))
            .collect();

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
                // **What the rub divided becomes separate shapes.**
                //
                // A difference returns one path holding every piece that
                // survived, so rubbing through the middle of a drawing left the
                // two halves welded into one object: clicking either selected
                // both, and dragging one dragged the other. Splitting is what
                // an eraser is *for* — it is how you cut a shape in two — and
                // `split_disjoint` keeps each piece's holes with it rather than
                // turning them into discs.
                let mut pieces: Vec<buzz_geom::BezPath> = Vec::new();
                update_shape(scene, at, id, |s| {
                    let mut cut = s.path.clone();
                    for cutter in &cutters {
                        cut = buzz_geom::boolean(
                            &cut,
                            cutter,
                            buzz_geom::BoolOp::Difference,
                            opts,
                        );
                    }
                    became_empty = cut.elements().is_empty();
                    let mut parts = buzz_geom::split_disjoint(&cut);
                    // The first piece stays in the object that was already
                    // there, so it keeps its id, its name and its place in the
                    // stacking order; only the extra pieces are new.
                    s.path = if parts.is_empty() {
                        cut
                    } else {
                        parts.remove(0)
                    };
                    pieces = parts;
                });
                if became_empty {
                    scene.remove_object(id);
                    continue;
                }
                // The offcuts, as objects of their own, carrying the same paint
                // as the shape they came from.
                let template = scene
                    .find_object(id)
                    .and_then(|(_, o)| match &o.kind {
                        ObjectKind::Shape(s) => Some(s.clone()),
                        _ => None,
                    });
                if let Some(template) = template {
                    for path in pieces {
                        scene.add_shape_at(
                            layer,
                            frame,
                            ShapeData {
                                path,
                                ..template.clone()
                            },
                        );
                    }
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
        let table = self.symbol_bounds();
        if let Some(hit) = self.object_at_frame(self.current_frame, point, tolerance, &table) {
            return Some(hit);
        }
        self.multi_frames()
            .into_iter()
            .rev()
            .find_map(|frame| self.object_at_frame(frame, point, tolerance, &table))
    }

    /// Topmost object under `point` on one particular frame.
    fn object_at_frame(
        &self,
        frame: u32,
        point: Point,
        tolerance: f64,
        table: &std::collections::HashMap<buzz_scene::SymbolId, buzz_geom::Rect>,
    ) -> Option<ObjectId> {
        let scene = self.doc.scene();
        let mut hit = None;
        // `selectable` yields back to front, so the last match is on top.
        // Walked in the order the renderer paints, so the last match really is
        // what is on top. See `LayerStack::selectable_in_paint_order`.
        let by_depth = scene.stage().sort_by_depth;
        for layer in scene.layers().selectable_in_paint_order(by_depth) {
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
                if object_contains(scene, object, local, local_tolerance, frame, 0, table) {
                    hit = Some(object.id);
                }
            }
        }
        hit
    }

    /// The colour under `point` for the eyedropper — the topmost shape's fill or
    /// stroke, found through instances and groups. Mirrors [`Self::object_at`]'s
    /// layer walk, so it samples exactly what a click would select into.
    pub fn sampled_paint_at(&self, point: Point) -> Option<(Color, bool)> {
        let table = self.symbol_bounds();
        let scene = self.doc.scene();
        let tolerance = self.pick_tolerance();
        let frame = self.current_frame;
        let mut hit = None;
        // Walked in the order the renderer paints, so the last match really is
        // what is on top. See `LayerStack::selectable_in_paint_order`.
        let by_depth = scene.stage().sort_by_depth;
        for layer in scene.layers().selectable_in_paint_order(by_depth) {
            let Some(local) = scene.view_to_layer(frame, layer.depth, point) else {
                continue;
            };
            let local = match invert(scene.layers().inherited_transform(layer.id, frame)) {
                Some(back) => back * local,
                None => local,
            };
            for object in layer.objects_at(frame) {
                if !object.visible || object.locked {
                    continue;
                }
                let Some(local) = unturn(scene, object, frame, layer.depth, local) else {
                    continue;
                };
                if let Some(paint) = sampled_paint(scene, object, local, tolerance, frame, 0, &table)
                {
                    hit = Some(paint);
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
        // **On the frame it is drawn on, not on the playhead's.**
        //
        // Under Edit Multiple Frames the artwork of other keyframes is on
        // screen, and the renderer draws each of them through its *own* layer
        // parenting (`buzz_render::document`). Measuring here at the playhead's
        // frame instead applied a parent's motion that had not happened to
        // artwork that was not there yet, so the handles sat off the drawing by
        // exactly how far the parent had travelled between the two frames.
        //
        // `None` when the object is on no frame currently drawn — a selection
        // that has outlived its span. There is nothing on screen to put a
        // handle on, and drawing one anyway put a box over empty stage.
        let (frame, layer) = self.drawn_at(id)?;
        let object = scene
            .layers()
            .get(layer)?
            .objects_at(frame)
            .iter()
            .find(|o| o.id == id)?;
        let depth = scene.layers().get(layer).map(|l| l.depth).unwrap_or(0.0);

        // Resolved from the memoised table: this runs every frame the object is
        // selected, and re-measuring a rig through the library each time is what
        // made the frame after a selection crawl.
        let table = self.symbol_bounds();
        let bounds = scene.resolved_bounds_with(object, &table);
        let pivot = scene.pivot_of_with(object, &table);
        let projection = scene.camera().projection_for_object(
            frame,
            scene.stage().size,
            depth,
            pivot,
            &object.spatial,
        )?;

        // Layer parenting moves the artwork, and so does the place a symbol was
        // opened at — both in the plane, before the lens, and in that order.
        // This is the same chain `buzz_render::document` draws through
        // (`projection.pre_affine(place).pre_affine(follows)`); without the
        // place, chrome inside a symbol opened *in place* was drawn at the
        // origin the artwork is no longer at.
        let follows = scene.layers().inherited_transform(layer, frame);
        projection
            .pre_affine(scene.edit_place())
            .pre_affine(follows)
            .map_rect(bounds)
    }

    /// The selection's bounds **where the artwork is drawn**, on the focal
    /// plane the pointer works on.
    ///
    /// Worked out object by object rather than asking the selection for one
    /// answer, because under Edit Multiple Frames two selected objects can be
    /// on different frames, and on layers whose parents and depths differ. This
    /// is the space the pointer works in, so it is what the transform box, the
    /// transformation point, and the "did this drag start inside the
    /// selection?" test must all be built on.
    ///
    /// `None` when nothing in the selection is on a frame currently drawn.
    pub fn selection_bounds_drawn(&self) -> Option<Rect> {
        let scene = self.doc.scene();
        self.selection
            .iter()
            .filter_map(|id| {
                // **Measured once.** `object_quad` is where the artwork is
                // drawn — through the object's own turn in space, its layer's
                // depth and parenting, the place a symbol was opened at, and
                // the shot. Deriving the bounds from it rather than
                // re-measuring means the handle box and the outline cannot
                // drift apart, which is how they drifted before.
                let quad = self.object_quad(id)?;
                let (frame, _) = self.drawn_at(id)?;

                // Back to the focal plane, which is where the pointer is by the
                // time `screen_to_edit` has finished with it — and where the
                // "did this drag start inside the selection?" test has to ask
                // its question. The place comes off with it, for the same
                // reason it does there.
                let shot = buzz_geom::Projection::from_affine(scene.camera_transform(frame));
                let back = shot.inverse()?;
                let unplace = buzz_scene::invert_affine(scene.edit_place());

                let mut out: Option<Rect> = None;
                for corner in quad {
                    let at = back.map_point(corner)?;
                    let at = match unplace {
                        Some(u) => u * at,
                        None => at,
                    };
                    out = Some(match out {
                        Some(r) => r.union_pt(at),
                        None => Rect::from_points(at, at),
                    });
                }
                out
            })
            .reduce(|a, b| a.union(b))
    }

    /// The frame an object is currently drawn on, and the layer it is on.
    ///
    /// The playhead's frame when the object is there, which is the ordinary
    /// case and the cheap one. Otherwise whichever keyframe Edit Multiple
    /// Frames is showing it on. `None` when it is on neither.
    fn drawn_at(&self, id: ObjectId) -> Option<(u32, LayerId)> {
        let scene = self.doc.scene();
        let on = |frame: u32| {
            scene
                .layers()
                .iter()
                .find(|l| l.objects_at(frame).iter().any(|o| o.id == id))
                .map(|l| (frame, l.id))
        };
        on(self.current_frame).or_else(|| self.multi_frames().into_iter().find_map(on))
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
        // Resolved through the library, for the reason `Scene::symbol_bounds`
        // records: an instance cannot measure itself, so `Object::bounds` gives
        // it a two-unit placeholder about its own origin. Marqueeing against
        // that picked a character up only when the drag happened to cross the
        // dot its registration point sits on — which reads as a selection
        // offset by however far the artwork is drawn from that point, typically
        // most of a limb away from where the user dragged.
        let table = self.symbol_bounds();
        let mut out = Vec::new();
        // Walked in the order the renderer paints, so the last match really is
        // what is on top. See `LayerStack::selectable_in_paint_order`.
        let by_depth = scene.stage().sort_by_depth;
        for layer in scene.layers().selectable_in_paint_order(by_depth) {
            // The marquee carried back the way a click is: through the camera's
            // projection at this layer's depth, then through whatever the layer
            // is parented to. Both draw a layer's artwork somewhere other than
            // where its geometry says it is, so the rectangle has to be moved
            // the same way in reverse before it can be tested against that
            // geometry — the trip `object_at` and the Lasso already make.
            //
            // With no depth and no tilt this is the identity, so an ordinary
            // document tests exactly the rectangle that was dragged.
            let Some(in_layer) = self.marquee_in_layer(rect, frame, layer.depth, layer.id) else {
                // At or behind the camera: not drawn, so not selectable.
                continue;
            };
            for object in layer.objects_at(frame) {
                if !object.visible || object.locked {
                    continue;
                }
                let bounds = scene.resolved_bounds_with(object, &table);
                if in_layer.contains_rect(bounds) {
                    out.push(object.id);
                }
            }
        }
        out
    }

    /// A marquee rectangle moved from what the user sees into one layer's own
    /// coordinates.
    ///
    /// `None` when the layer is at or behind the camera. A tilted camera turns
    /// the rectangle into a trapezoid, and the extent of that trapezoid is what
    /// is returned: a marquee gesture has no way to express a quad, and the
    /// honest approximation is the box round the one the user dragged.
    fn marquee_in_layer(&self, rect: Rect, frame: u32, depth: f64, layer: LayerId) -> Option<Rect> {
        let scene = self.doc.scene();
        let back = invert(scene.layers().inherited_transform(layer, frame));
        let mut mapped: Option<Rect> = None;
        for corner in [
            Point::new(rect.x0, rect.y0),
            Point::new(rect.x1, rect.y0),
            Point::new(rect.x1, rect.y1),
            Point::new(rect.x0, rect.y1),
        ] {
            let point = scene.view_to_layer(frame, depth, corner)?;
            let point = match back {
                Some(back) => back * point,
                None => point,
            };
            mapped = Some(match mapped {
                Some(r) => r.union_pt(point),
                None => Rect::from_points(point, point),
            });
        }
        mapped
    }

    /// Place a soft-brush stroke as artwork.
    ///
    /// The painted pixels go into the document's bitmap library and the stroke
    /// becomes an ordinary rectangle filled with them — so from this moment it
    /// is a shape like any other: selectable, movable, tweenable, cuttable by
    /// the Lasso, and convertible to a symbol to be reused.
    ///
    /// # Why each stroke is its own bitmap
    ///
    /// The alternative is a stage-sized canvas per layer that every stroke
    /// paints into, which is what Photoshop does. It would cost a full-canvas
    /// copy on every pointer move — eight megabytes at 1080p, sixty times a
    /// second — because the document is copy-on-write and undo holds the
    /// previous state. A stroke-sized bitmap costs what was painted: a dab is
    /// a few kilobytes. What is given up is painting *across* strokes, which
    /// §7 records.
    fn paint_raster(&mut self, canvas: &buzz_scene::Canvas, brush: &buzz_scene::SoftBrush) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer available to paint on".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }

        let frame = self.current_frame;
        let auto = self.auto_keyframe;
        let blend = self.style.brush.blend();
        let mirrors = self.symmetry_mirrors();

        // **Paint fuses with paint.** In Merge Shape mode — Animate's default,
        // and what every other drawing tool here already honours — a stroke laid
        // over paint of the same colour becomes part of it rather than a second
        // object stacked on top. See `fusable_paint`.
        let fuse = (self.style.drawing_mode == buzz_ui::DrawingMode::MergeShape)
            .then(|| self.fusable_paint(layer, frame, canvas, brush))
            .flatten();

        if let Some((target, merged)) = fuse {
            let mut fused = None;
            self.doc.edit("Brush", |scene| {
                // A mirrored soft stroke reflects **this stroke's** pixels, not
                // the fused result. Fusing rewrites the paint already on the
                // layer, so reflecting that would mirror everything under the
                // brush again with every dab, and the far side of the stage
                // would thicken as you painted on the near one.
                if !mirrors.is_empty() {
                    let stroke = raster_shape(scene, canvas, brush, blend);
                    for t in &mirrors {
                        scene.add_shape_at(layer, frame, mirror_shape(&stroke, *t));
                    }
                }
                let id = scene.next_image_id();
                let name = scene.images().unique_name("Paint");
                let mut asset = buzz_scene::ImageAsset::from_pixels(
                    id,
                    name,
                    merged.width,
                    merged.height,
                    std::sync::Arc::new(merged.pixels.clone()),
                );
                asset.painted = true;
                let asset = scene.images_mut().insert(asset);

                let area = merged.area();
                let mut fill = buzz_scene::ImageFill::new(asset, area);
                fill.smooth = false;
                scene.update_object_at(frame, target, |o| {
                    if let ObjectKind::Shape(shape) = &mut o.kind {
                        shape.path = buzz_geom::Shape::to_path(&area, 1e-9);
                        shape.fill = Some(buzz_scene::FillSpec::image(fill.clone()));
                        shape.blend = blend;
                    }
                });
                fused = Some(target);
            });
            self.doc.end_gesture();
            if let Some(id) = fused {
                self.selection.set([id]);
            }
            return;
        }

        let mut painted = None;
        self.doc.edit("Brush", |scene| {
            if auto {
                scene.ensure_keyframe(layer, frame);
            }
            let shape = raster_shape(scene, canvas, brush, blend);
            // The copies first, so the stroke being watched stays selected.
            // Each reflection is carried by the fill's own transform, so the
            // pixels are stored once however many mirrors there are.
            for t in &mirrors {
                scene.add_shape_at(layer, frame, mirror_shape(&shape, *t));
            }
            painted = scene.add_shape_at(layer, frame, shape);
        });
        self.doc.end_gesture();
        match painted {
            Some(id) => self.selection.set([id]),
            None => self.status = Some("Could not paint on this frame".into()),
        }
    }

    /// **The paint this stroke should join**, and the bitmap they make together.
    ///
    /// `None` — meaning the stroke becomes its own shape, as it always did —
    /// unless every one of these holds:
    ///
    /// * a shape on this layer and frame carries **painted** pixels (an
    ///   imported photograph is not paint and merges with nothing);
    /// * it is in the **same colour**, because that is what Animate's merge
    ///   model fuses; a different colour is a different thing on top;
    /// * it uses the **same blend**, or fusing would change how the result sits
    ///   against what is under it;
    /// * their areas actually **overlap** — paint at the other end of the stage
    ///   is not the same stroke of paint, and one bitmap spanning both would be
    ///   mostly empty;
    /// * and the existing paint is still **square to the axes at its own pixel
    ///   scale**. A stroke that has since been rotated or scaled would have to
    ///   be resampled to merge, and resampling paint to fuse it would lose more
    ///   than the fusing gains.
    ///
    /// The newest such shape wins, which is the one the eye reads as "what I am
    /// painting on".
    fn fusable_paint(
        &self,
        layer: LayerId,
        frame: u32,
        canvas: &buzz_scene::Canvas,
        brush: &buzz_scene::SoftBrush,
    ) -> Option<(ObjectId, buzz_scene::MergedPaint)> {
        let scene = self.doc.scene();
        let blend = self.style.brush.blend();
        let ink = brush.color.to_rgba8().to_u8_array();

        // Newest first: the top of the stack is the paint the eye reads as the
        // one being painted on.
        let resolved = scene.layers().get(layer)?.frames.resolved_at(frame);
        let objects: Vec<_> = resolved.iter().collect();
        for object in objects.into_iter().rev() {
            let ObjectKind::Shape(shape) = &object.kind else {
                continue;
            };
            if shape.blend != blend {
                continue;
            }
            let Some(fill) = shape.fill.as_ref().and_then(|f| f.paint.image()) else {
                continue;
            };
            if !fill.asset.painted {
                continue;
            }

            // Same colour, or it is a different thing sitting on top.
            let Some(under_ink) = buzz_scene::painted_ink(&fill.asset) else {
                continue;
            };
            let under = under_ink.to_rgba8().to_u8_array();
            if under[0..3] != ink[0..3] {
                continue;
            }

            // Still one pixel to one document unit, square to the axes, and
            // where its own transform says: no resampling, no guessing.
            let placement = object.transform * fill.transform;
            let c = placement.as_coeffs();
            let square = c[1].abs() < 1e-6 && c[2].abs() < 1e-6;
            let unit = (c[0] - f64::from(fill.asset.width)).abs() < 1e-6
                && (c[3] - f64::from(fill.asset.height)).abs() < 1e-6;
            if !square || !unit {
                continue;
            }
            let under_origin = buzz_geom::Point::new(c[4], c[5]);
            let under_area = buzz_geom::Rect::new(
                under_origin.x,
                under_origin.y,
                under_origin.x + f64::from(fill.asset.width),
                under_origin.y + f64::from(fill.asset.height),
            );
            if !under_area.overlaps(canvas.area()) {
                continue;
            }

            let merged = buzz_scene::merge_over(&fill.asset, under_origin, canvas, brush)?;
            return Some((object.id, merged));
        }
        None
    }

    /// Place an effect stroke's artwork: vector shapes and painted bitmaps
    /// together, in one undo step.
    ///
    /// Bitmap pieces go through the document's image library exactly as a
    /// soft-brush stroke does; every piece then lands as an ordinary shape.
    /// More than one piece is committed as a **group**, because one stroke
    /// should be one thing — the gesture after "paint snow" is "move the
    /// snow", and asking the user to gather up nine shapes first would make
    /// the brush feel like a spill rather than a stroke.
    fn add_artwork(&mut self, pieces: Vec<buzz_scene::ArtPiece>, label: &'static str) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer available to draw on".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }
        if pieces.is_empty() {
            return;
        }

        let frame = self.current_frame;
        let auto = self.auto_keyframe;
        let mirrors = self.symmetry_mirrors();
        let mut created: Option<ObjectId> = None;
        self.doc.edit(label, |scene| {
            if auto {
                scene.ensure_keyframe(layer, frame);
            }
            // Painted pieces go into the document's image library, exactly as
            // a soft-brush stroke does — which is what makes their pixels
            // part of the file rather than of this session.
            let mut register = |canvas: &buzz_scene::Canvas, brush: &buzz_scene::SoftBrush| {
                let id = scene.next_image_id();
                let name = scene.images().unique_name(label);
                scene.images_mut().insert(canvas.to_asset(id, name, brush))
            };
            let mut shapes = buzz_scene::art::to_shapes(&pieces, &mut register);

            // **A brush can carry a bitmap in from somewhere else.** Artwork
            // captured as a brush keeps a *shared* handle on its texture, and
            // that brush outlives the document it was made in: paint with it
            // in a new file and the pixels would be referred to by an id that
            // file's library has never heard of, which saves as a picture
            // that is not there. Adopting them here costs a lookup per stroke
            // and nothing at all once they are in.
            for shape in &mut shapes {
                adopt_textures(scene, shape);
            }

            // Symmetry copies go down first, so the stroke the user was
            // watching stays the selected one — the rule `add_shape`
            // follows, and the reason an effect brush under a mirror still
            // leaves you holding the piece you drew.
            for t in &mirrors {
                let copies = shapes.iter().map(|s| mirror_shape(s, *t)).collect();
                place_artwork(scene, layer, frame, copies);
            }
            created = place_artwork(scene, layer, frame, shapes);
        });
        self.doc.end_gesture();
        match created {
            Some(id) => self.selection.select_one(id),
            None => self.status = Some("Could not draw on this frame".into()),
        }
    }

    /// Place one drawing per frame: a stroke that commits an **animation**.
    ///
    /// The Wave brush hands over a whole cycle — see [`buzz_scene::wave`] — and
    /// this lays it out from the current frame onwards, one keyframe each, as a
    /// single undo step. Each frame's pieces are grouped exactly as
    /// [`Self::add_artwork`] groups a still, so the result is one thing per
    /// frame rather than a scatter of shapes to gather up.
    ///
    /// # Why every keyframe is made before any artwork is
    ///
    /// The keyframes are inserted the way **F6** inserts one: carrying whatever
    /// the layer was already showing, so baking a plume over a held background
    /// does not blank the background on the frames it covers. That only works
    /// if the whole run is created *first* — insert a keyframe after the wave's
    /// first frame is down and it carries the wave with it, and every frame
    /// after the first ends up holding one more copy of the plume than the
    /// frame before.
    ///
    /// A frame that is already a keyframe is left as it is and drawn onto, so a
    /// wave can be laid over an animation that is already there.
    fn add_artwork_frames(&mut self, frames: Vec<Vec<buzz_scene::ArtPiece>>, label: &'static str) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer available to draw on".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }
        if frames.iter().all(Vec::is_empty) {
            return;
        }

        let start = self.current_frame;
        let count = frames.len() as u32;
        let mirrors = self.symmetry_mirrors();
        let mut created: Option<ObjectId> = None;
        self.doc.edit(label, |scene| {
            // Every keyframe first. See the doc comment: this order is the
            // difference between a plume and a plume stacked on itself.
            scene.update_layer(layer, |l| {
                for i in 0..count {
                    l.frames.insert_keyframe(start + i);
                }
            });

            for (i, pieces) in frames.into_iter().enumerate() {
                if pieces.is_empty() {
                    continue;
                }
                let frame = start + i as u32;

                // Painted pieces go into the document's image library, exactly
                // as a still's do.
                let mut register = |canvas: &buzz_scene::Canvas, brush: &buzz_scene::SoftBrush| {
                    let id = scene.next_image_id();
                    let name = scene.images().unique_name(label);
                    scene.images_mut().insert(canvas.to_asset(id, name, brush))
                };
                let mut shapes = buzz_scene::art::to_shapes(&pieces, &mut register);
                for shape in &mut shapes {
                    adopt_textures(scene, shape);
                }

                // Every frame of the cycle is mirrored, not just the first:
                // a wave drawn under a mirror has to loop on both sides or the
                // two halves drift apart as it plays.
                for t in &mirrors {
                    let copies = shapes.iter().map(|s| mirror_shape(s, *t)).collect();
                    place_artwork(scene, layer, frame, copies);
                }
                let placed = place_artwork(scene, layer, frame, shapes);
                // The first frame's drawing is the one selected afterwards:
                // it is the frame the user was looking at while drawing.
                if created.is_none() {
                    created = placed;
                }
            }
        });
        self.doc.end_gesture();
        match created {
            Some(id) => {
                self.selection.select_one(id);
                self.status = Some(format!("{label}: {count} frames"));
            }
            None => self.status = Some("Could not draw on this frame".into()),
        }
    }

    // -- region selection: the Lasso and the Magic Wand ----------------------

    /// Take everything inside a freehand region — **and cut the artwork along
    /// it**.
    ///
    /// # Why cutting, and not merely selecting
    ///
    /// This is what Animate's Lasso does to a shape, and it is the only reading
    /// that makes the tool useful. A lasso round the left half of a drawing
    /// cannot select "the left half" — no such object exists — so either the
    /// tool selects the whole thing, in which case it is a worse Selection
    /// tool, or it makes the half real. Making it real is what lets the next
    /// keystroke delete it, move it, colour it or convert it to a symbol.
    ///
    /// An instance or a group is not cut: it is picked if its middle is inside
    /// the region. Cutting one would mean cutting the symbol it refers to, and
    /// every other instance with it.
    fn pick_in_region(&mut self, region: &BezPath, additive: bool) {
        let scene = self.doc.scene().clone();
        let table = self.symbol_bounds();
        let frame = self.current_frame;
        let region_bounds = buzz_geom::Shape::bounding_box(region);

        // Worked out entirely before the document is touched: `doc.edit` gets a
        // list of things to do, so a lasso that catches nothing is not an undo
        // step and the scene is not borrowed while it is being inspected.
        let mut cuts: Vec<(LayerId, ObjectId, BezPath)> = Vec::new();
        let mut picks: Vec<ObjectId> = Vec::new();

        // Walked in the order the renderer paints, so the last match really is
        // what is on top. See `LayerStack::selectable_in_paint_order`.
        let by_depth = scene.stage().sort_by_depth;
        for layer in scene.layers().selectable_in_paint_order(by_depth) {
            // The region carried back the way a click is: through the camera's
            // projection at this layer's depth, then through whatever the layer
            // is parented to. A projective map takes straight lines to straight
            // lines, so moving the corners of a polygon moves the polygon.
            let Some(in_layer) = map_path(region, |p| {
                let p = scene.view_to_layer(frame, layer.depth, p)?;
                Some(
                    match invert(scene.layers().inherited_transform(layer.id, frame)) {
                        Some(back) => back * p,
                        None => p,
                    },
                )
            }) else {
                continue;
            };

            for object in layer.objects_at(frame) {
                if !object.visible || object.locked {
                    continue;
                }
                let bounds = scene.resolved_bounds_with(object, &table);
                if bounds.intersect(region_bounds).is_zero_area()
                    && !region_bounds.contains_rect(bounds)
                {
                    continue;
                }
                // An object turned in space is drawn on its own plane, so the
                // region travels onto that plane before it can cut anything.
                let Some(in_object) = map_path(&in_layer, |p| {
                    let p = unturn(&scene, object, frame, layer.depth, p)?;
                    invert(object.transform).map(|back| back * p)
                }) else {
                    continue;
                };

                match &object.kind {
                    ObjectKind::Shape(_) => cuts.push((layer.id, object.id, in_object)),
                    // Not cuttable: picked whole, if its middle is inside.
                    _ => {
                        if in_object.contains(
                            invert(object.transform)
                                .map_or(bounds.center(), |back| back * bounds.center()),
                        ) {
                            picks.push(object.id);
                        }
                    }
                }
            }
        }

        let taken = self.cut_and_collect(cuts, "Lasso");
        if !additive {
            self.selection.clear();
        }
        self.selection.extend(taken);
        self.selection.extend(picks);
    }

    /// Cut each shape along its region, and report what to select.
    ///
    /// A shape entirely inside its region is selected whole rather than cut in
    /// two, because cutting it would leave an empty husk behind.
    fn cut_and_collect(
        &mut self,
        cuts: Vec<(LayerId, ObjectId, BezPath)>,
        label: &'static str,
    ) -> Vec<ObjectId> {
        if cuts.is_empty() {
            return Vec::new();
        }
        let frame = self.current_frame;
        let mut taken = Vec::new();
        self.doc.edit(label, |scene| {
            for (layer, id, region) in cuts {
                let Some(object) = scene.find_object(id).map(|(_, o)| Arc::clone(o)) else {
                    continue;
                };
                let ObjectKind::Shape(shape) = &object.kind else {
                    continue;
                };
                let size = buzz_geom::Shape::bounding_box(&shape.path);
                let opts = buzz_geom::BooleanOptions::for_shape_size(
                    size.width().hypot(size.height()).max(1.0),
                );
                let inside =
                    buzz_geom::boolean(&shape.path, &region, buzz_geom::BoolOp::Intersect, opts);
                if inside.is_empty() {
                    continue;
                }
                let outside =
                    buzz_geom::boolean(&shape.path, &region, buzz_geom::BoolOp::Difference, opts);
                if outside.is_empty() {
                    // The whole shape was caught: nothing to cut.
                    taken.push(id);
                    continue;
                }

                // The part that stays keeps the object — and so keeps its
                // name, its filters and anything referring to it by id.
                scene.update_object(id, |o| {
                    if let ObjectKind::Shape(s) = &mut o.kind {
                        s.path = outside;
                    }
                });
                // The part that was taken becomes a new object with the same
                // paint. An image or gradient fill travels with it unchanged,
                // which is exactly why the picture does not slide about inside
                // a cut bitmap: the fill's transform is in this same space.
                let cut = ShapeData {
                    path: inside,
                    ..shape.clone()
                };
                if let Some(new_id) = scene.add_shape_at(layer, frame, cut) {
                    scene.update_object(new_id, |o| {
                        o.transform = object.transform;
                        o.blend = object.blend;
                        o.spatial = object.spatial;
                        o.filters = object.filters.clone();
                    });
                    taken.push(new_id);
                }
            }
        });
        self.doc.end_gesture();
        taken
    }

    /// The Magic Wand: take everything the colour of what was clicked.
    ///
    /// On a bitmap this is a flood fill through the pixels, traced to a path
    /// and cut out — see [`buzz_scene::wand`]. On ordinary vector artwork the
    /// answer is already a shape: a region of one colour *is* the shape that
    /// was drawn, so the wand selects it whole. Both readings are the same
    /// sentence — "everything joined to this that looks like this" — and a user
    /// who does not know which kind of artwork they clicked gets the right
    /// answer either way.
    fn wand_at(&mut self, point: Point, additive: bool) {
        let scene = self.doc.scene().clone();
        let frame = self.current_frame;
        let Some(id) = self.object_at(point, self.pick_tolerance()) else {
            if !additive {
                self.selection.clear();
            }
            return;
        };
        let Some((layer, object)) = scene.find_object(id).map(|(l, o)| (l, o.clone())) else {
            return;
        };

        // The click, carried into the object's own space the same way the hit
        // test carried it.
        let depth = scene
            .layers()
            .get(layer)
            .map(|l| l.depth)
            .unwrap_or_default();
        let local = scene
            .view_to_layer(frame, depth, point)
            .map(
                |p| match invert(scene.layers().inherited_transform(layer, frame)) {
                    Some(back) => back * p,
                    None => p,
                },
            )
            .and_then(|p| unturn(&scene, &object, frame, depth, p))
            .and_then(|p| invert(object.transform).map(|back| back * p));
        let Some(local) = local else { return };

        let region = match &object.kind {
            ObjectKind::Shape(shape) => match shape.fill.as_ref().map(|f| &f.paint) {
                Some(buzz_scene::Paint::Image(fill)) => {
                    buzz_scene::wand_region(fill, local, self.style.wand)
                }
                // Not a picture: one flat colour, and the shape is its own
                // region.
                _ => None,
            },
            _ => None,
        };

        match region {
            Some(region) => {
                let taken = self.cut_and_collect(vec![(layer, id, region)], "Magic Wand");
                if !additive {
                    self.selection.clear();
                }
                if taken.is_empty() {
                    self.status = Some("Nothing there matched closely enough".into());
                }
                self.selection.extend(taken);
            }
            None => {
                if additive {
                    self.selection.toggle(id);
                } else {
                    self.selection.select_one(id);
                }
            }
        }
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
            Save | SaveAs | Open | Close | SaveSnapshot | Snapshots | ShortcutEditor => {
                // File dialogs, the snapshots store and the shortcut editor are
                // host concerns; the shell handles them.
                self.status = Some(format!("{} is handled by the shell", command.label()));
            }

            SaveAsTemplate => {
                // Named after the document, so a template is called what the
                // film it came from was called. `unique_name` keeps a second
                // save from quietly replacing the first.
                let wanted = self
                    .doc
                    .path()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Template".to_string());
                let scene = self.doc.scene().clone();
                match self.templates.save(&wanted, &scene) {
                    Ok(template) => {
                        self.status = Some(format!(
                            "Saved the stage as a template called \"{}\"",
                            template.name
                        ));
                    }
                    Err(e) => self.status = Some(format!("Could not save the template: {e}")),
                }
            }

            NewFromTemplate(index) => {
                let Some(template) = self.templates.iter().nth(index).cloned() else {
                    self.status = Some("That template is no longer there".into());
                    return;
                };
                match self.templates.start(&template) {
                    Ok(doc) => {
                        let name = template.name.clone();
                        self.adopt(doc);
                        self.status = Some(format!("Started from \"{name}\""));
                    }
                    Err(e) => {
                        self.status = Some(format!("Could not open \"{}\": {e}", template.name))
                    }
                }
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
            SelectSameColour => self.select_same_colour(),
            RetargetPerformance => self.retarget_performance(),
            SwapSymbol => self.swap_selected_symbol(),
            PaintThrough => {
                // To the end of the layer: colouring is a pass over a whole
                // shot, not over the next frame or two.
                let last = self
                    .active_layer()
                    .and_then(|l| self.doc.scene().layers().get(l).map(|l| l.frames.length()))
                    .unwrap_or(0)
                    .saturating_sub(1);
                self.propagate_fills(last);
            }
            ExposeOnTwos => self.expose_on(2),
            ExposeOnThrees => self.expose_on(3),
            Deselect => self.selection.clear(),
            DuplicateSelection => self.duplicate_selection(),
            Align { op, to_stage } => self.align_selection(op, to_stage),
            Distribute(op) => self.distribute_selection(op),
            MatchSize(op) => self.match_selection_size(op),
            Nudge { x, y } => {
                if !self.selection.is_empty() {
                    // Through the same path a drag takes, so a nudge is an
                    // ordinary Move in the history and coalesces with the
                    // nudges either side of it — holding an arrow key down is
                    // one undo step, not forty.
                    self.transform_selection(
                        Affine::translate((f64::from(x), f64::from(y))),
                        "Move",
                    );
                }
            }
            Cut => {
                if self.copy_selection() {
                    self.delete_selection();
                }
            }
            Copy => {
                self.copy_selection();
            }
            Paste => self.paste_clipboard(),

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
            ThickenStroke => self.scale_selected_strokes(1.25),
            ThinStroke => self.scale_selected_strokes(0.8),
            TraceBitmap => self.trace_selection(buzz_scene::TraceOptions::default()),
            TraceLineArt => self.trace_selection(buzz_scene::TraceOptions::line_art()),
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
            NewReferenceLayer => {
                // A guide layer: drawn faded, never exported. Made active so the
                // next Import Image drops the reference art straight onto it.
                let id = self.doc_add_layer("Reference", LayerKind::Guide);
                self.selection.set_active_layer(Some(id));
                self.status =
                    Some("Reference layer added — Import Image onto it to trace over".into());
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
            AddCameraMove(movement) => self.add_camera_move(movement),

            // -- lights ------------------------------------------------------
            AddLightKeyframe => self.add_light_key(),
            RemoveLightKeyframe => self.remove_light_key(),

            // -- symbols and library -----------------------------------------
            ConvertToSymbol => self.convert_selection_to_symbol(),
            BrushFromSelection => self.brush_from_selection(),

            // -- lighting -----------------------------------------------------
            AddSun => self.add_light(buzz_scene::LightKind::sun()),
            AddSky => self.add_light(buzz_scene::LightKind::sky()),
            AddLamp => self.add_light(buzz_scene::LightKind::lamp(self.camera.center)),
            // The point it is given is thrown away by `add_light`, which aims a
            // gloom against the rig it is joining. Passing one at all is what
            // keeps every kind arriving by the same door.
            AddGloom => self.add_light(buzz_scene::LightKind::gloom(self.camera.center)),
            AddFire => self.add_fire(),
            AddStorm => self.add_storm(),

            // -- staging and performance --------------------------------------
            SetScene => {
                let frames = self.doc.scene().frame_count();
                self.staging.open_scene(frames);
            }
            DirectScene => self.staging.open_direct(),
            AddScene => {
                self.add_scene();
                let n = self.doc.active_scene() + 1;
                self.status = Some(format!("Scene {n} of {}", self.doc.scene_names().len()));
            }
            DuplicateScene => {
                let from = self.doc.active_scene();
                self.duplicate_scene(from);
                let name = self
                    .doc
                    .scene_names()
                    .get(self.doc.active_scene())
                    .cloned()
                    .unwrap_or_default();
                self.status = Some(format!("Duplicated as {name}"));
            }
            AddPerson => self.add_person(),
            Perform => {
                // Over the whole film from the playhead, which is what an
                // animator standing on frame 12 means by "animate this".
                let last = self.doc.scene().frame_count().saturating_sub(1);
                let from = self.current_frame.min(last);
                self.staging.open_perform(from, last);
            }
            AddFollowThrough => {
                // The chain is chosen by name, so the dialog needs the selected
                // rig's bones. Gathered here and passed in, the same way the
                // performance dialog is handed a frame range.
                let bones = {
                    let scene = self.doc.scene();
                    self.selection.iter().find_map(|id| {
                        scene.find_object(id).and_then(|(_, o)| match &o.kind {
                            ObjectKind::Armature(rig) => {
                                Some(rig.armature.bones.iter().map(|b| b.name.clone()).collect::<Vec<String>>())
                            }
                            _ => None,
                        })
                    })
                };
                match bones {
                    Some(names) => {
                        let last = self.doc.scene().frame_count().saturating_sub(1);
                        let from = self.current_frame.min(last);
                        self.staging.open_physics(from, last, names);
                    }
                    None => {
                        self.status =
                            Some("Select a rigged character to add follow-through to".into())
                    }
                }
            }
            AddWiggle => {
                let has_object = {
                    let scene = self.doc.scene();
                    self.selection
                        .iter()
                        .any(|id| scene.find_object(id).is_some())
                };
                if has_object {
                    let last = self.doc.scene().frame_count().saturating_sub(1);
                    let from = self.current_frame.min(last);
                    self.staging.open_wiggle(from, last);
                } else {
                    self.status = Some("Select an object to add a wiggle to".into());
                }
            }
            ClearModifiers => {
                let object = {
                    let scene = self.doc.scene();
                    self.selection.iter().find(|id| {
                        scene
                            .find_object(*id)
                            .is_some_and(|(_, o)| !o.modifiers.is_empty())
                    })
                };
                match object {
                    Some(id) => {
                        self.doc.edit("Clear Modifiers", |scene| {
                            scene.update_object_across(0, u32::MAX, id, |o| o.modifiers.clear());
                        });
                        self.doc.end_gesture();
                        self.status = Some("Cleared the object's live modifiers".into());
                    }
                    None => self.status = Some("The selection has no live modifiers".into()),
                }
            }
            BakeModifiers => {
                let object = {
                    let scene = self.doc.scene();
                    self.selection.iter().find(|id| {
                        scene
                            .find_object(*id)
                            .is_some_and(|(_, o)| !o.modifiers.is_empty())
                    })
                };
                match object {
                    Some(id) => self.bake_modifiers(id),
                    None => self.status = Some("The selection has no live modifiers".into()),
                }
            }
            SetReverse => self.set_reverse(),
            ClearReverse => self.clear_reverse(),
            AddProfileRight => self.add_turnaround_view(90.0),
            AddProfileLeft => self.add_turnaround_view(-90.0),
            AddThreeQuarterRight => self.add_turnaround_view(45.0),
            AddThreeQuarterLeft => self.add_turnaround_view(-45.0),
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
            ToggleTheme => self.set_theme(buzz_ui::theme::theme().next()),
            SetTheme(theme) => self.set_theme(theme),
            About => self.about.open = true,
            ResetWorkspace => {
                // The layout, and only the layout: the theme, the new-document
                // settings and the crash-recovery directories are preferences
                // that live in the same struct and are not what "reset the
                // layout" asks for. See `Workspace::reset_layout`.
                self.workspace.reset_layout();
                self.workspace.save();
                self.status =
                    Some("Every panel put back where it started".into());
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

            ExportFla | ExportImage | ExportSequence | ExportVideo | ExportGif | ExportWebp => {
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

            ImportSound | ImportImage | LipSync | ImportVideoReference
            | ImportSequenceFolder => {
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
            DetectBeats => self.detect_beats(),
            FitToNarration => self.fit_to_narration(),
            // The dialogs belong to the shell, which raises these back with a
            // path — the same route Import Sound and every export take.
            ImportCaptions | ExportCaptions => {}
            LipSyncFromCaptions => {
                if let Err(e) = self.lip_sync_from_captions() {
                    self.status = Some(format!("{e}"));
                }
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
    /// **The box a new light is sized against.**
    ///
    /// A lamp's reach and a gloom's throw are both derived from how big the
    /// picture is, because a fixed number of units is right for one document
    /// and wrong for every other. What they were derived from was
    /// [`Camera::visible_doc_rect`] alone, and that is not always a picture:
    ///
    /// * **Before the stage has been laid out** the viewport is empty, so the
    ///   visible rectangle is a *point*. A lamp built from it got the minimum
    ///   reach of forty units and a gloom got a throw of one — a light that
    ///   cannot be seen, on a stage hundreds of units across, which reads as
    ///   the light never having been added. Any moment the stage has no area
    ///   does this: the panel maximised over it, the window minimised, the very
    ///   first frame after a document opens.
    /// * **Zoomed in**, the visible rectangle is a detail rather than the shot.
    ///   A light sized to it lights the detail and dies before the edge of the
    ///   frame, so zooming back out shows a shot that is barely lit at all.
    ///
    /// So the view is used only when it is a real box, and it is unioned with
    /// the stage: a light belongs to the *shot*, and the shot is at least the
    /// stage. Where the light is put still follows the view, which is the half
    /// of "put it where the user is looking" that was always right.
    fn light_frame(&self) -> buzz_geom::Rect {
        let stage = self.doc.scene().stage().stage_rect();
        let seen = self.camera.visible_doc_rect();
        let usable = seen.x0.is_finite()
            && seen.y0.is_finite()
            && seen.x1.is_finite()
            && seen.y1.is_finite()
            && seen.width() > 1.0
            && seen.height() > 1.0;
        if !usable {
            return stage;
        }
        stage.union(seen)
    }

    /// Where to stand a new lamp so it is not on top of one already there.
    ///
    /// Up and to the left of `frame`, where a key light goes — and then stepped
    /// along the diagonal until it is clear of every lamp in the rig.
    ///
    /// **Two lamps in the same place look like one lamp.** The position was a
    /// fixed fraction of the view, so every lamp after the first landed on
    /// exactly the same point: adding a second changed the picture by almost
    /// nothing, which is indistinguishable from it not having been added. It is
    /// the same report as "I deleted the light and the next one did nothing",
    /// because the next one arrives where the last one was.
    fn free_lamp_spot(&self, frame: buzz_geom::Rect) -> Point {
        let start = Point::new(
            frame.x0 + frame.width() * 0.22,
            frame.y0 + frame.height() * 0.20,
        );
        // A step big enough that the two pools are visibly different lights,
        // small enough that the tenth one is still on the picture.
        let step = (frame.width().min(frame.height()) * 0.12).max(8.0);
        let clear = |at: Point| {
            !self
                .doc
                .scene()
                .lights()
                .lights
                .iter()
                .any(|light| match light.kind {
                    buzz_scene::LightKind::Lamp { position, .. } => {
                        (position - at).hypot() < step * 0.9
                    }
                    _ => false,
                })
        };
        (0..12)
            .map(|n| start + buzz_geom::Vec2::new(step * n as f64, step * n as f64 * 0.6))
            .find(|at| clear(*at))
            .unwrap_or(start)
    }

    /// A lamp arrives **in the view, and off to one side** — up and to the left,
    /// where a key light goes — whatever position the request carried. A lamp is
    /// the one light with a place on the stage, and one dropped off-screen (at
    /// the origin, say, which is the top-left corner of the artwork) looks
    /// exactly like nothing having happened.
    ///
    /// # Why not the middle of the view
    ///
    /// It used to arrive dead centre, which is the one position where a lamp
    /// does nothing you can see. Everything a lamp does that reads as *light*
    /// comes from the direction it lies in: the shaded crescent is the artwork
    /// minus itself shifted towards the lamp, and the shadow is the artwork
    /// projected away from it. A lamp directly over the middle of what it is
    /// lighting has no direction in the plane at all — so there is no crescent,
    /// the pool is symmetrical, and the shadow projects straight out from under
    /// the drawing and hides beneath it.
    ///
    /// Measured on a character at the centre of the stage: the lit side and the
    /// dark side came out within a couple of levels of each other, with no
    /// shadow on the floor. Which is exactly the report — a lamp that lights
    /// nothing, on a rig that works perfectly the moment the lamp is dragged an
    /// inch off centre.
    pub fn add_light(&mut self, kind: buzz_scene::LightKind) {
        let seen = self.light_frame();
        let kind = match kind {
            buzz_scene::LightKind::Lamp { height, radius, .. } => {
                // **Reach is measured against what is on screen, not against a
                // fixed number of pixels.**
                //
                // The default was 320 units whatever the document was. On the
                // 550-wide stage this was built against that crosses most of
                // the picture; on a 1920 film it is a sixth of the width, so a
                // new lamp fell off to nothing before it reached the character
                // standing in the middle of the shot — added, aimed, and
                // apparently doing nothing, which is how a lamp gets reported
                // as broken.
                //
                // Half the smaller side of the view puts the half-brightness
                // ring around the middle distance of whatever is being looked
                // at, so the falloff lands across the subject rather than
                // inside the lamp or beyond the frame. The number stays in the
                // Reach box for the animator to overrule.
                let reach = (seen.width().min(seen.height()) * 0.5).clamp(40.0, 3000.0);
                buzz_scene::LightKind::Lamp {
                    position: self.free_lamp_spot(seen),
                    height,
                    radius: if radius > 0.0 { reach } else { radius },
                }
            }
            // **A gloom is aimed, not placed.** Dropping one where the pointer
            // happens to be is the one thing that cannot be right: it is a wall
            // the width of the picture, and where it stands only means anything
            // relative to the light it is standing against. So the panel's
            // position is discarded and the rig is asked instead.
            buzz_scene::LightKind::Gloom { .. } => {
                self.doc.scene().lights().opposing_gloom(seen)
            }
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

    /// **Add a lamp and set it alight.**
    ///
    /// A fire is a lamp with a hearth colour and a gutter, so it arrives through
    /// the same door every other light does — placed in the view, sized to the
    /// shot, clear of the lamps already there — and is then made fire. Doing it
    /// as one command rather than two clicks is the whole point: a preset buried
    /// in a panel that has to be found, selected and scrolled to is a preset
    /// nobody uses.
    fn add_fire(&mut self) {
        self.add_light(buzz_scene::LightKind::lamp(self.camera.center));
        let Some(id) = self.light_panel.selected else {
            return;
        };
        self.doc.edit("Fire", |scene| {
            if let Some(light) = scene.lights_mut().get_mut(id) {
                light.make_fire();
            }
        });
        self.status = Some("Added a fire \u{2014} scrub the timeline to see it move".into());
    }

    /// **Add a sky and set it striking.**
    ///
    /// The counterpart of [`add_fire`](Self::add_fire), through the same door
    /// and for the same reason: lightning is an ordinary light with a violent
    /// envelope on it, and a preset buried in a panel is a preset nobody finds.
    ///
    /// A **sky**, not a sun: a sheet of lightning has no direction — it lights
    /// the whole stage at once, which is what makes the frame go white rather
    /// than one side of every figure. `make_storm` then turns the light itself
    /// right down, because a flash only reads against the dark.
    fn add_storm(&mut self) {
        self.add_light(buzz_scene::LightKind::sky());
        let Some(id) = self.light_panel.selected else {
            return;
        };
        self.doc.edit("Storm", |scene| {
            if let Some(light) = scene.lights_mut().get_mut(id) {
                light.make_storm();
            }
        });
        self.status =
            Some("Added a storm \u{2014} scrub the timeline to see it strike".into());
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
        let Some(gesture) = crate::lights::target_at(
            self.doc.scene(),
            doc,
            tolerance,
            self.light_panel.selected,
        ) else {
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

    /// Bring a sound file into the library, and onto the timeline.
    ///
    /// Returns the name it took in the library. Whether it was also *placed* is
    /// asked separately with [`Self::sound_was_placed`], so that this keeps
    /// answering the one question its callers ask it — folding "and it went on
    /// a layer" into the returned name would make the name unusable as a name.
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
        let mut placed = false;
        let fps = self.doc.scene().stage().frame_rate.max(1.0);
        self.doc.edit("Import Sound", |scene| {
            let id = scene.add_sound(&name, bytes, &format, rate, channels, length);
            imported = Some(id);
            placed = place_sound_on_stage(scene, id, fps);
        });
        self.doc.end_gesture();

        let scene = self.doc.scene().clone();
        self.sound.refresh(&scene);

        self.sound_placed = placed;
        Ok(imported
            .and_then(|id| scene.sounds().get(id).map(|s| s.name.clone()))
            .unwrap_or(name))
    }

    /// Did the last [`Self::import_sound`] also put the sound on the timeline?
    ///
    /// The shell says so in the status bar: a sound that arrived on a layer of
    /// its own has changed the timeline, and an animator who is not told that
    /// finds a layer they did not make.
    pub fn sound_was_placed(&self) -> bool {
        self.sound_placed
    }

    /// Animate's File ▸ Import Image, arriving already broken apart.
    ///
    /// Returns the name it took in the library.
    ///
    /// # Two deliberate departures from Animate
    ///
    /// **It arrives as artwork, not as a placed bitmap.** See
    /// [`buzz_scene::Scene::place_image`]: every interesting thing an animator
    /// does to a photograph needs it broken apart first, and breaking apart
    /// costs nothing here because a bitmap *is* a shape with an image fill.
    ///
    /// **A picture larger than the stage is scaled to fit it.** Animate places
    /// at natural size, so a phone photograph lands mostly off the pasteboard
    /// and the first thing anyone does is scale it down by hand. Aspect ratio
    /// is kept, and Free Transform undoes it in one drag if the natural size
    /// was wanted. A picture that already fits is placed untouched, at its own
    /// size, so pixel-exact artwork stays pixel-exact.
    pub fn import_image(&mut self, path: &std::path::Path) -> anyhow::Result<String> {
        // Read and decode before the document is touched, so a file that is
        // not really a picture leaves no library entry behind.
        let bytes = std::fs::read(path)?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Bitmap".to_string());

        let Some(layer) = self.active_layer() else {
            anyhow::bail!("no layer available to place it on");
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            anyhow::bail!("the active layer is locked");
        }

        let stage = self.doc.scene().stage().size;
        let centre = self.camera.center;
        let frame = self.current_frame;

        let mut placed: Option<(String, ObjectId)> = None;
        let mut failure: Option<String> = None;
        self.doc.edit("Import Image", |scene| {
            let asset = match scene.add_image(&name, &bytes) {
                Ok(a) => a,
                Err(e) => {
                    failure = Some(e.to_string());
                    return;
                }
            };
            let (w, h) = (asset.width as f64, asset.height as f64);
            let fit = (stage.width / w).min(stage.height / h).min(1.0);
            let (w, h) = (w * fit, h * fit);
            let rect = buzz_geom::Rect::new(
                centre.x - w / 2.0,
                centre.y - h / 2.0,
                centre.x + w / 2.0,
                centre.y + h / 2.0,
            );
            let asset_name = asset.name.clone();
            if let Some(id) = scene.place_image_in(layer, frame, asset, rect) {
                placed = Some((asset_name, id));
            }
        });

        if let Some(e) = failure {
            // The failed `add_image` left the revision bumped but nothing
            // usable; drop it so the user's undo stack is not littered with
            // imports that did not happen.
            self.doc.undo();
            anyhow::bail!(e);
        }
        let Some((name, id)) = placed else {
            anyhow::bail!("could not place it on this frame");
        };
        self.doc.end_gesture();
        self.selection.set([id]);
        Ok(name)
    }

    /// Put the most recently imported sound on the current keyframe.
    ///
    /// Put the newest import on the keyframe the playhead is on.
    ///
    /// The menu command's quick path, kept deliberately: the sound an animator
    /// has just brought in is the one they mean, and importing then attaching
    /// should not need a trip through a panel. The Sound panel is where a
    /// *different* clip, the sync mode, the volume and the repeat count are
    /// chosen — it edits what this creates.
    fn attach_sound_to_frame(&mut self) {
        let Some(layer) = self.selection.active_layer() else {
            self.status = Some("Select a layer to put the sound on".into());
            return;
        };
        let Some(sound) = self.doc.scene().sounds().iter().last().map(|s| s.id) else {
            self.status = Some("Import a sound first: File > Import Sound".into());
            return;
        };

        // **A sound inside a symbol is a sound nobody hears.**
        //
        // What plays and what is exported is `Scene::stage_cues`, which reads
        // the document's own timeline; a cue put on a layer of the symbol that
        // happened to be open would sit there for ever, silent on the stage and
        // absent from the film, with the status bar cheerfully reporting that
        // it was attached. Saying so is the only honest answer \u2014 attaching it
        // somewhere the animator did not ask for would be worse.
        if !self.doc.scene().edit_path().is_empty() {
            self.status = Some(
                "Sound goes on the main timeline. Leave this symbol first \
                 (the breadcrumb above the stage), then attach it."
                    .into(),
            );
            return;
        }

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
    ///
    /// **Cached.** The timeline draws this every frame; deriving an envelope from
    /// raw PCM each time was a per-frame cost that scaled with the soundtrack's
    /// length and turned a long document into a frozen one. Now the whole map is
    /// gated on the document revision, and each clip's levels are memoised, so an
    /// unchanged document is a handful of `Arc` clones and even a live edit only
    /// reassembles the map. See [`WaveformCache`].
    pub fn waveforms(&mut self) -> std::collections::BTreeMap<LayerId, buzz_ui::Waveform> {
        let revision = self.doc.scene().revision();
        if self.waveform_cache.revision == Some(revision) {
            return self.waveform_cache.map.clone();
        }
        // Mid-drag the revision moves every pointer move, and no drag of
        // artwork can change a sound. Hold the last answer; the release will
        // bring the revision to rest and this will rebuild once.
        if self.is_gesturing() && self.waveform_cache.revision.is_some() {
            return self.waveform_cache.map.clone();
        }

        let fps = self.doc.scene().stage().frame_rate;
        let fps_bits = fps.to_bits();
        let mut map = std::collections::BTreeMap::new();

        for layer in self.doc.scene().stage_layers().iter() {
            for keyframe in layer.frames.keyframes() {
                let Some(reference) = keyframe.sound else {
                    continue;
                };
                let Some(clip) = self.sound.clip(reference.sound) else {
                    continue;
                };
                let clip_ptr = std::sync::Arc::as_ptr(clip) as usize;
                let key = (reference.sound, fps_bits);
                // Reuse the memoised envelope unless the sound was re-decoded
                // (its clip lives at a new address).
                let levels = match self.waveform_cache.levels.get(&key) {
                    Some((ptr, levels)) if *ptr == clip_ptr => levels.clone(),
                    _ => {
                        let levels = std::sync::Arc::new(clip.frame_levels(fps));
                        self.waveform_cache
                            .levels
                            .insert(key, (clip_ptr, levels.clone()));
                        levels
                    }
                };
                map.insert(
                    layer.id,
                    buzz_ui::Waveform {
                        start_frame: keyframe.start,
                        levels,
                    },
                );
            }
        }

        self.waveform_cache.revision = Some(revision);
        self.waveform_cache.map = map.clone();
        map
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
                    config_dir: buzz_script::default_config_dir(),
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

    /// Switch which scene is being edited, and put the editor into a clean
    /// state for it: nothing selected, the active layer resolved, the playhead
    /// clamped, and the symbol-bounds cache dropped (a different scene reuses
    /// revision numbers, so a stale entry would give wrong bounds).
    pub fn switch_scene(&mut self, index: usize) {
        if index == self.doc.active_scene() {
            return;
        }
        self.doc.switch_scene(index);
        self.bounds_cache.borrow_mut().take();
        self.selection.clear();
        self.camera_selected = false;
        self.selection.ensure_active_layer(self.doc.scene());
        // A fresh scene may be shorter than where the playhead sat; prune what
        // is no longer on screen.
        self.set_frame(self.current_frame);
        self.playback.playing = false;
    }

    /// Add a new empty scene after the active one and edit it.
    pub fn add_scene(&mut self) {
        self.doc.add_scene();
        self.bounds_cache.borrow_mut().take();
        self.selection.clear();
        self.camera_selected = false;
        self.selection.ensure_active_layer(self.doc.scene());
        self.set_frame(0);
        self.playback.playing = false;
    }

    /// Duplicate a scene, whole, and edit the copy.
    ///
    /// The playhead goes back to the start, because what you do next is play
    /// the beat you have just copied and change it — see
    /// [`buzz_doc::Document::duplicate_scene`].
    pub fn duplicate_scene(&mut self, index: usize) {
        self.doc.duplicate_scene(index);
        self.bounds_cache.borrow_mut().take();
        self.selection.clear();
        self.camera_selected = false;
        self.selection.ensure_active_layer(self.doc.scene());
        self.set_frame(0);
        self.playback.playing = false;
    }

    /// Move a scene in the running order and follow it.
    pub fn move_scene(&mut self, from: usize, to: usize) {
        self.doc.move_scene(from, to);
        self.bounds_cache.borrow_mut().take();
        self.selection.clear();
        self.camera_selected = false;
        self.selection.ensure_active_layer(self.doc.scene());
        self.set_frame(self.current_frame);
        self.playback.playing = false;
    }

    /// Delete a scene. The last remaining scene cannot be removed.
    pub fn delete_scene(&mut self, index: usize) {
        self.doc.delete_scene(index);
        self.bounds_cache.borrow_mut().take();
        self.selection.clear();
        self.camera_selected = false;
        self.selection.ensure_active_layer(self.doc.scene());
        self.set_frame(self.current_frame);
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

    /// Animate's *Create Brush From Selection* — **with the artwork's paint**.
    ///
    /// Animate takes the shape and leaves the colours behind, so a brush made
    /// from a red leaf with a gradient down it comes out as a grey silhouette.
    /// What is captured here is the artwork: its fills, its gradients and its
    /// bitmaps, so the brush stamps the thing that was pointed at. See
    /// [`buzz_scene::BrushStamp`] for the normalisation and for what happens
    /// to that paint when the stamp is placed.
    ///
    /// The Artwork Colours switch in the tool options turns it back into
    /// Animate's behaviour for artwork drawn deliberately to be a nib.
    fn brush_from_selection(&mut self) {
        // Flattening resolves groups and applies each object's transform, so a
        // brush made from a group comes out as it looked on stage — and each
        // part keeps the shape data it was drawn with rather than only its
        // outline.
        let mut parts: Vec<(buzz_geom::Affine, ShapeData)> = Vec::new();
        for id in self.selection.iter() {
            let Some((_, object)) = self.doc.scene().find_object(id) else {
                continue;
            };
            object.flatten(buzz_geom::Affine::IDENTITY, &mut parts);
        }

        if parts.is_empty() {
            self.status = Some("Select some artwork to make a brush from".into());
            return;
        }
        let Some(stamp) = buzz_scene::BrushStamp::capture(&parts) else {
            self.status = Some("That selection has no area to make a brush from".into());
            return;
        };

        let painted = stamp.is_painted();
        // Selecting the brush too: making a brush and not being given it is a
        // step the user would always have to take next.
        self.style.brush.set_custom_stamp(stamp);
        if !self.style.brush.kind.uses_pattern() {
            self.style.brush.kind = buzz_ui::BrushKind::Pattern;
        }
        self.set_tool(ToolId::Brush);
        self.status = Some(if painted && self.style.brush.keep_source_paint {
            "Brush created from the selection, with its colours".into()
        } else {
            "Brush created from the selection".to_string()
        });
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
            .local_bounds(self.doc.scene())
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
        // The middle of the view, which is the best guess available when the
        // command carries no position of its own.
        let at = self.camera.center;
        self.place_symbol(symbol, at);
    }

    /// Place an instance where the pointer is, in **screen** coordinates.
    ///
    /// The drop end of dragging a symbol out of the Library. Screen rather than
    /// document coordinates because that is what a pointer has; the camera
    /// converts, so it lands under the cursor at any zoom or pan.
    pub fn place_symbol_at(&mut self, symbol: buzz_scene::SymbolId, screen: Point) {
        let at = self.camera.screen_to_doc(screen);
        self.place_symbol(symbol, at);
    }

    fn place_symbol(&mut self, symbol: buzz_scene::SymbolId, at: Point) {
        let Some(layer) = self.active_layer() else {
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }

        let frame = self.current_frame;
        let mut placed = None;
        self.doc.edit("Place Instance", |scene| {
            placed = scene.add_instance_at(layer, frame, symbol, Affine::translate((at.x, at.y)));
        });
        match placed {
            Some(id) => {
                // What was just placed is what the user wants to move.
                self.selection.set([id]);
                self.library.selected = Some(symbol);
            }
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

    /// Replace only the *easing* of the tween on the keyframe governing the
    /// playhead — the Motion Editor's write. Read-modify-write keeps the tween's
    /// kind, extra rotations and orient-to-path; no `end_gesture`, so a drag of
    /// the curve coalesces into one undo step.
    pub fn set_ease_curve(&mut self, easing: buzz_scene::Easing) {
        let Some(layer) = self.active_layer() else {
            return;
        };
        let frame = self.current_frame;
        self.doc.edit("Ease Curve", |scene| {
            scene.update_layer(layer, |l| {
                let mut tween = l.frames.tween_at(frame);
                tween.easing = easing;
                l.frames.set_tween(frame, tween);
            });
        });
    }

    /// **Make one object the reverse (back view) of another.** With two objects
    /// selected, the earlier-drawn one becomes the front and the other its back,
    /// shown when the front is turned to face away. The back is anchored to the
    /// front (stored relative to it) so it travels with it.
    fn set_reverse(&mut self) {
        self.set_turnaround_view(std::f64::consts::PI, "Set Reverse");
    }

    /// **Make one object the view of another from a given way round.**
    ///
    /// The same gesture as a back view, at any angle: select the front and the
    /// drawing, and that drawing becomes what is shown once the front has turned
    /// far enough to be nearer this angle than any other. A profile goes in at
    /// ninety degrees, a three-quarter at forty-five.
    pub fn add_turnaround_view(&mut self, degrees: f64) {
        self.set_turnaround_view(degrees.to_radians(), "Add View");
    }

    fn set_turnaround_view(&mut self, angle: f64, label: &'static str) {
        if self.selection.len() != 2 {
            self.status =
                Some("Select two: the front (drawn first) and its back (drawn second)".into());
            return;
        }
        let ids = self.selection.ids();
        let (front, back) = (ids[0], ids[1]);
        self.doc.edit(label, |scene| {
            let front_transform = scene.find_object(front).map(|(_, o)| o.transform);
            if let (Some(front_transform), Some(mut back_obj)) =
                (front_transform, scene.remove_object(back))
            {
                // Relative to the front, so it overlays where it was drawn and
                // follows the front when the front is moved.
                let relative = buzz_scene::invert_affine(front_transform)
                    .unwrap_or(Affine::IDENTITY)
                    * back_obj.transform;
                std::sync::Arc::make_mut(&mut back_obj).transform = relative;
                scene.update_object(front, |o| {
                    o.turnaround.set(angle, back_obj.clone());
                });
            }
        });
        self.doc.end_gesture();
        self.selection.select_one(front);
        self.status = Some(format!(
            "View at {:.0}° set \u{2014} turn the object to see it",
            angle.to_degrees()
        ));
    }

    /// Remove the selected object's reverse (back) drawing.
    fn clear_reverse(&mut self) {
        let target = {
            let scene = self.doc.scene();
            self.selection.iter().find(|id| {
                scene
                    .find_object(*id)
                    .is_some_and(|(_, o)| !o.turnaround.is_empty())
            })
        };
        match target {
            Some(id) => {
                self.doc.edit("Clear Reverse", |scene| {
                    scene.update_object(id, |o| o.turnaround = Default::default());
                });
                self.doc.end_gesture();
                self.status = Some("Turnaround removed".into());
            }
            None => self.status = Some("The selection has no other views".into()),
        }
    }

    /// **How many views the selected object has**, beyond its own front.
    pub fn selected_turnaround(&self) -> Option<Vec<f64>> {
        let scene = self.doc.scene();
        let id = self.selection.iter().next()?;
        let (_, object) = scene.find_object(id)?;
        (!object.turnaround.is_empty())
            .then(|| object.turnaround.views().iter().map(|v| v.angle).collect())
    }

    /// **Re-expose the active layer on twos** — or threes, or whatever `step`
    /// says.
    ///
    /// Over the selected span of frames when there is one, and over the whole
    /// layer otherwise, because "put this on twos" is nearly always a decision
    /// about a shot rather than about six frames of it.
    pub fn expose_on(&mut self, step: u32) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer to re-expose".into());
            return;
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return;
        }
        let (from, to) = self.multi_frame_range().unwrap_or_else(|| {
            let last = self
                .doc
                .scene()
                .layers()
                .get(layer)
                .map(|l| l.frames.length().saturating_sub(1))
                .unwrap_or(0);
            (0, last)
        });

        let mut dropped = 0;
        self.doc.edit("Expose on Twos", |scene| {
            scene.update_layer(layer, |l| {
                dropped = l.frames.expose_on(from, to, step);
            });
        });
        self.doc.end_gesture();
        self.status = Some(match dropped {
            0 => format!("Already on {step}s over frames {}–{}", from + 1, to + 1),
            1 => format!("On {step}s — one drawing now holds longer"),
            n => format!("On {step}s — {n} drawings folded into the ones they follow"),
        });
    }

    /// **Direct a whole sequence** — a page of prose becomes an animatic.
    ///
    /// # What this adds over directing a shot
    ///
    /// [`buzz_act::direct`] stages one scene and animates the cast in it, which
    /// is a *shot*. A story is several: the place changes, the cast changes, and
    /// the film cuts between them. This reads the brief the way the writing
    /// already separates its beats — a blank line, or a line that is only a
    /// setting — and gives each one a scene of its own, named after its own
    /// first words so the scene list reads like the brief.
    ///
    /// The document already knows how to hold several scenes and export them as
    /// one film; what was missing was anything to fill them.
    ///
    /// # It stops at the first shot it cannot read
    ///
    /// Rather than leaving half a film and no explanation. Everything directed
    /// before the failure stays — the same rule the scripting host follows, and
    /// far more useful than discarding an hour's brief because the last
    /// paragraph was a sentence fragment.
    pub fn direct_sequence(&mut self, brief: &str) -> usize {
        let shots = buzz_act::split_shots(brief);
        if shots.is_empty() {
            self.status = Some("There is nothing in that brief to direct".into());
            return 0;
        }

        let mut directed = 0;
        let mut trouble: Option<String> = None;

        for (index, shot) in shots.iter().enumerate() {
            // The first shot fills the scene that is open; each one after it
            // gets a scene of its own, which is what makes this a sequence.
            if index > 0 {
                self.doc.add_scene();
            }
            let mut failed = None;
            self.doc.edit("Direct", |scene| {
                if let Err(e) = buzz_act::direct(scene, &shot.story) {
                    failed = Some(format!("{e}"));
                }
            });
            match failed {
                None => {
                    let at = self.doc.active_scene();
                    self.doc.rename_scene(at, &shot.title);
                    directed += 1;
                }
                Some(message) => {
                    trouble = Some(format!("shot {}: {message}", index + 1));
                    break;
                }
            }
        }
        self.doc.end_gesture();

        self.status = Some(match (directed, trouble) {
            (0, Some(why)) => format!("Could not direct that brief — {why}"),
            (n, Some(why)) => format!("Directed {n} shot(s), then stopped at {why}"),
            (1, None) => "Directed one shot".to_string(),
            (n, None) => format!("Directed {n} shots, one scene each"),
        });
        directed
    }

    /// **Carry this frame's colouring onto the frames after it** — ink and paint.
    ///
    /// # The job this is
    ///
    /// Colouring is half the labour of drawn animation and almost none of the
    /// craft. The line art is redrawn every frame and the *colours are the
    /// same every frame*: the coat is the same red on frame 40 as on frame 1,
    /// and somebody has to click it forty times. Traditional pipelines have had
    /// a name for automating this for decades — ink and paint — and it is the
    /// single largest saving available in a program like this one.
    ///
    /// # How the regions are matched
    ///
    /// Not by comparing shapes: on the next frame the region is a *different
    /// enclosure*, drawn afresh, and there is nothing to compare it to. What is
    /// stable is roughly **where it is**. So each fill on this frame offers a
    /// point inside itself as a seed, and the next frame is flooded from that
    /// point with the same colour, through the same gap-aware bucket a person
    /// would have clicked with. A region that has moved less than its own size —
    /// which is nearly all of them, between two frames of the same drawing —
    /// still contains the seed and comes out the same colour.
    ///
    /// # Where it stops, and why it says so
    ///
    /// A seed that lands outside every enclosure on the next frame — the arm
    /// swung further than its own width, or the line has a gap the bucket cannot
    /// close — is **left uncoloured and counted**. It is not guessed at. A
    /// wrong colour looks deliberate and survives to the film; a missing one is
    /// visible immediately and is a click to fix, which is the right way round
    /// for something automatic.
    ///
    /// Frames that already carry a fill covering the seed are left alone, so
    /// running this twice does nothing the second time and colouring done by
    /// hand is never overwritten.
    pub fn propagate_fills(&mut self, through: u32) -> (usize, usize) {
        let Some(layer) = self.active_layer() else {
            self.status = Some("No layer to carry the colours along".into());
            return (0, 0);
        };
        if self.doc.scene().layers().is_effectively_locked(layer) {
            self.status = Some("The active layer is locked".into());
            return (0, 0);
        }
        let from = self.current_frame;
        let gap = self.style.gap_size;

        // What this frame has been coloured with: the bucket's own fills, each
        // with a point inside itself to seed the next frame from.
        let sources: Vec<(Point, buzz_scene::FillSpec)> = {
            let scene = self.doc.scene();
            let Some(layer_ref) = scene.layers().get(layer) else {
                return (0, 0);
            };
            layer_ref
                .frames
                .resolved_at(from)
                .iter()
                .filter_map(|object| {
                    let ObjectKind::Shape(shape) = &object.kind else {
                        return None;
                    };
                    let fill = shape.fill.as_ref()?;
                    // The bucket's own rule is what marks a fill as *paint*
                    // rather than as line art: a brush stroke is a filled shape
                    // too, and carrying those forward would draw the drawing
                    // twice.
                    if fill.rule != buzz_scene::bucket::FILL_RULE {
                        return None;
                    }
                    let path = object.transform * shape.path.clone();
                    interior_point(&path).map(|seed| (seed, fill.clone()))
                })
                .collect()
        };

        if sources.is_empty() {
            self.status =
                Some("This frame has no bucket fills to carry — colour one first".into());
            return (0, 0);
        }

        let last = through.max(from);
        let mut painted = 0;
        let mut missed = 0;

        self.doc.edit("Ink and Paint", |scene| {
            for frame in (from + 1)..=last {
                // Nothing drawn here means nothing to colour; a held frame is
                // the same drawing and already carries its own colour.
                let Some(layer_ref) = scene.layers().get(layer) else {
                    break;
                };
                if !layer_ref.frames.is_keyframe(frame) {
                    continue;
                }

                let mut boundaries = Vec::new();
                let mut already: Vec<buzz_geom::BezPath> = Vec::new();
                for object in layer_ref.frames.resolved_at(frame).iter() {
                    collect_bucket_boundaries(object, buzz_geom::Affine::IDENTITY, &mut boundaries);
                    if let ObjectKind::Shape(shape) = &object.kind
                        && shape
                            .fill
                            .as_ref()
                            .is_some_and(|f| f.rule == buzz_scene::bucket::FILL_RULE)
                    {
                        already.push(object.transform * shape.path.clone());
                    }
                }

                for (seed, fill) in &sources {
                    // Painted here already, by hand or by an earlier run.
                    if already.iter().any(|path| {
                        buzz_geom::fill_contains(path, *seed, buzz_scene::bucket::FILL_RULE)
                    }) {
                        continue;
                    }
                    match buzz_scene::fill_region(&boundaries, *seed, gap) {
                        Some(path) => {
                            let shape = ShapeData {
                                path,
                                fill: Some(fill.clone()),
                                stroke: None,
                                blend: buzz_scene::PaintBlend::default(),
                            };
                            // Behind the line art, as the bucket puts its own.
                            scene.add_shape_behind_at(layer, frame, shape);
                            painted += 1;
                        }
                        None => missed += 1,
                    }
                }
            }
        });
        self.doc.end_gesture();

        self.status = Some(match (painted, missed) {
            (0, 0) => "Nothing after this frame to colour".to_string(),
            (n, 0) => format!("Carried the colours onto {n} region(s)"),
            (0, m) => format!(
                "Could not place {m} region(s) — the drawing moved too far, or a line has a gap"
            ),
            (n, m) => format!(
                "Coloured {n} region(s); {m} could not be placed and are left for you"
            ),
        });
        (painted, missed)
    }

    // -- reuse -----------------------------------------------------------------

    /// **Make one character perform what another already performs.**
    ///
    /// # Why a pose transfers at all
    ///
    /// A pose here is one angle per bone, in bone order. Two rigs assembled from
    /// the same pattern have the same bones in the same order, so the same list
    /// of angles means the same thing on both — a walk authored once drives the
    /// whole cast, which is the multiplier a solo animator actually needs.
    ///
    /// # It refuses rather than mangling
    ///
    /// Two rigs with different skeletons have no shared meaning for angle four,
    /// and applying one to the other would produce a character folded through
    /// itself. Where the bone counts differ this says so and does nothing. Where
    /// they match but the patterns differ it goes ahead: an animator who built
    /// two five-bone rigs by hand knows what they meant better than a name does.
    ///
    /// With two armatures selected: the first performs, the second follows.
    pub fn retarget_performance(&mut self) {
        let ids = self.selection.ids();
        if ids.len() != 2 {
            self.status = Some(
                "Select two rigs: the one that performs, then the one to copy it onto".into(),
            );
            return;
        }
        let (source, target) = (ids[0], ids[1]);

        // Read the whole performance before anything is written, so a rig that
        // turns out not to match leaves the document untouched.
        let (layer, poses, bones) = {
            let scene = self.doc.scene();
            let Some((layer, _)) = scene.find_object(source) else {
                self.status = Some("The first selection is no longer there".into());
                return;
            };
            let Some(layer_ref) = scene.layers().get(layer) else {
                return;
            };
            let mut poses: Vec<(u32, Vec<f64>)> = Vec::new();
            let mut bones = 0usize;
            for keyframe in layer_ref.frames.keyframes() {
                let frame = keyframe.start;
                if let Some(pose) = layer_ref
                    .frames
                    .resolved_at(frame)
                    .iter()
                    .find(|o| o.id == source)
                    .and_then(|o| match &o.kind {
                        ObjectKind::Armature(rig) => Some(rig.armature.pose()),
                        _ => None,
                    })
                {
                    bones = bones.max(pose.len());
                    poses.push((frame, pose));
                }
            }
            (layer, poses, bones)
        };

        if poses.is_empty() {
            self.status = Some("The first selection is not a rig with any poses on it".into());
            return;
        }

        let target_bones = self
            .doc
            .scene()
            .find_object(target)
            .and_then(|(_, o)| match &o.kind {
                ObjectKind::Armature(rig) => Some(rig.armature.pose().len()),
                _ => None,
            });
        let Some(target_bones) = target_bones else {
            self.status = Some("The second selection is not a rig".into());
            return;
        };
        if target_bones != bones {
            self.status = Some(format!(
                "These rigs have different skeletons — {bones} bones and {target_bones} — so a pose                  from one means nothing on the other"
            ));
            return;
        }

        let _ = layer;
        let target_layer = self.doc.scene().find_object(target).map(|(l, _)| l);
        let last = poses.iter().map(|(f, _)| *f).max().unwrap_or(0);

        let mut written = 0;
        self.doc.edit("Retarget", |scene| {
            // The performance is as long as it is; a rig standing on a
            // one-frame layer has no frames to be posed on, and without this
            // every pose after the first landed nowhere.
            if let Some(target_layer) = target_layer {
                scene.update_layer(target_layer, |l| {
                    while l.frames.length() <= last {
                        l.frames.insert_frame(l.frames.length());
                    }
                });
            }
            for (frame, pose) in &poses {
                scene.ensure_keyframe_for(*frame, target);
                scene.update_object_at(*frame, target, |object| {
                    if let ObjectKind::Armature(rig) = &mut object.kind {
                        rig.armature.set_pose(pose);
                        written += 1;
                    }
                });
            }
        });
        self.doc.end_gesture();
        self.status = Some(match written {
            0 => "Nothing could be copied onto that rig".to_string(),
            n => format!("Copied {n} pose(s) onto the second rig"),
        });
    }

    /// **Point every instance of one symbol at another.**
    ///
    /// With two instances selected: everything wearing the first symbol now
    /// wears the second, keeping where it stands, how big it is and what colour
    /// effect it carries — only the drawing changes. A costume change across a
    /// whole film, in one step.
    pub fn swap_selected_symbol(&mut self) {
        let ids = self.selection.ids();
        let symbols: Vec<buzz_scene::SymbolId> = {
            let scene = self.doc.scene();
            ids.iter()
                .filter_map(|id| match &scene.find_object(*id)?.1.kind {
                    ObjectKind::Instance(instance) => Some(instance.symbol),
                    _ => None,
                })
                .collect()
        };
        if symbols.len() != 2 {
            self.status = Some(
                "Select two instances: the one to replace, then the one to replace it with".into(),
            );
            return;
        }

        let mut swapped = 0;
        self.doc.edit("Swap Symbol", |scene| {
            swapped = scene.swap_symbol(symbols[0], symbols[1]);
        });
        self.doc.end_gesture();
        self.status = Some(match swapped {
            0 => "Nothing was wearing that symbol".to_string(),
            1 => "One instance now wears the other symbol".to_string(),
            n => format!("{n} instances now wear the other symbol"),
        });
    }

    // -- colour --------------------------------------------------------------

    /// **Repaint every fill and stroke linked to a swatch.**
    ///
    /// The whole point of a palette that links: change the coat's colour once
    /// and the coat changes on every frame, in every symbol, everywhere in the
    /// film. One undo step for all of it.
    pub fn recolour_swatch(&mut self, swatch: buzz_scene::SwatchId, colour: Color) {
        let mut painted = 0;
        self.doc.edit("Recolour", |scene| {
            painted = scene.recolour_swatch(swatch, colour);
        });
        self.doc.end_gesture();
        self.status = Some(match painted {
            0 => "Swatch changed — nothing in the document is linked to it yet".into(),
            1 => "Swatch changed, and repainted 1 fill".to_string(),
            n => format!("Swatch changed, and repainted {n} fills"),
        });
    }

    /// **Link the selection to a swatch**, and take its colour.
    ///
    /// The step that makes the palette worth having: a shape painted this way
    /// remembers where its colour came from, so the next change to that swatch
    /// finds it. Applied across the whole film, because a fill is a property of
    /// the artwork rather than of the moment.
    pub fn link_selection_to_swatch(&mut self, swatch: buzz_scene::SwatchId) {
        let Some(colour) = self
            .doc
            .scene()
            .swatches()
            .get(swatch)
            .map(|s| s.color)
        else {
            self.status = Some("That swatch is no longer in the palette".into());
            return;
        };
        let ids: Vec<ObjectId> = self.selection.ids();
        if ids.is_empty() {
            self.status = Some("Select the artwork to paint from the swatch".into());
            return;
        }
        let mut painted = 0;
        self.doc.edit("Paint from Swatch", |scene| {
            for id in &ids {
                scene.update_object_across(0, u32::MAX, *id, |object| {
                    if let ObjectKind::Shape(shape) = &mut object.kind {
                        if let Some(fill) = &mut shape.fill {
                            fill.paint = buzz_scene::Paint::Solid(colour);
                            fill.swatch = Some(swatch);
                            painted += 1;
                        }
                        if let Some(stroke) = &mut shape.stroke {
                            stroke.paint = buzz_scene::Paint::Solid(colour);
                            stroke.swatch = Some(swatch);
                            painted += 1;
                        }
                    }
                });
            }
        });
        self.doc.end_gesture();
        self.status = Some(if painted == 0 {
            "Nothing selected has a fill or a stroke to paint".into()
        } else {
            format!("{painted} painted from the swatch, and linked to it")
        });
    }

    /// **Select everything painted the same colour as what is already selected.**
    ///
    /// The enabling primitive for recolouring a document that predates linked
    /// swatches — and for the ordinary job of finding every piece of a
    /// character's coat before doing anything to it. Matches on the *fill*
    /// colour, which is what "this colour" means when looking at a drawing.
    ///
    /// Searches this frame, on every unlocked layer, because a selection that
    /// reached frames you cannot see would be a selection you cannot check.
    pub fn select_same_colour(&mut self) {
        let Some(wanted) = self.selection_fill_colour() else {
            self.status = Some("Select a filled shape first, to say which colour".into());
            return;
        };
        let frame = self.current_frame;
        let found: Vec<ObjectId> = {
            let scene = self.doc.scene();
            scene
                .layers()
                .selectable()
                .flat_map(|layer| layer.objects_at(frame))
                .filter(|object| match &object.kind {
                    ObjectKind::Shape(shape) => shape
                        .fill
                        .as_ref()
                        .is_some_and(|f| same_colour(f.paint.color(), wanted)),
                    _ => false,
                })
                .map(|object| object.id)
                .collect()
        };
        let count = found.len();
        self.selection.set(found);
        self.status = Some(match count {
            0 => "Nothing else is painted that colour".into(),
            1 => "One shape is painted that colour".into(),
            n => format!("{n} shapes are painted that colour"),
        });
    }

    /// The fill colour of the selection, when they agree on one.
    fn selection_fill_colour(&self) -> Option<Color> {
        let scene = self.doc.scene();
        let mut found: Option<Color> = None;
        for id in self.selection.iter() {
            let (_, object) = scene.find_object(id)?;
            let ObjectKind::Shape(shape) = &object.kind else {
                continue;
            };
            let colour = shape.fill.as_ref()?.paint.color();
            match found {
                None => found = Some(colour),
                // Two colours selected is no answer at all.
                Some(existing) if same_colour(existing, colour) => {}
                Some(_) => return None,
            }
        }
        found
    }

    /// **Fill the selected shapes with a procedural texture.** The texture is
    /// baked to one seamless tile from the current fill colour (foreground) and
    /// stroke colour (background), added to the image library once, and set as a
    /// tiling image fill on every selected shape. One undo step; a no-op with
    /// nothing (or nothing shaped) selected.
    pub fn apply_texture(&mut self, kind: buzz_scene::TextureKind) {
        let recipe =
            buzz_scene::TextureRecipe::new(kind, self.style.fill_color, self.style.stroke_color);
        self.apply_texture_recipe(recipe, None);
    }

    /// **Keep the draw style's texture tile in step with its recipe.**
    ///
    /// A style cannot hold an image on its own — an image has to live in the
    /// document's library or it would not survive being saved — so the tile is
    /// baked here, put in the library, and handed back to the style. An asset
    /// already baked from the same recipe is reused, so choosing Texture, going
    /// away and coming back does not leave a trail of identical tiles.
    ///
    /// Cheap and idempotent: it returns immediately unless the recipe has
    /// actually changed, which is what lets the panel call it every frame.
    pub fn ensure_fill_texture(&mut self) {
        if self.style.fill_kind != buzz_ui::FillKind::Texture {
            return;
        }
        let recipe = self.style.fill_texture;
        let current = self
            .style
            .fill_texture_asset
            .as_ref()
            .and_then(|a| a.recipe);
        if current == Some(recipe) {
            return;
        }
        if let Some(existing) = self.doc.scene().images().find_by_recipe(&recipe) {
            self.style.fill_texture_asset = Some(existing);
            return;
        }
        let mut made = None;
        self.doc.edit("Texture", |scene| {
            let name = scene.images().unique_name(recipe.kind.label());
            let id = scene.next_image_id();
            let asset = buzz_scene::ImageAsset::from_recipe(id, name, recipe, 256);
            made = Some(scene.images_mut().insert(asset));
        });
        self.doc.end_gesture();
        self.style.fill_texture_asset = made;
    }

    /// **The texture on the selection, if they all wear the same one.**
    ///
    /// What the panel shows: the recipe to put in its controls, the tile size
    /// and the angle. `None` when nothing selected is textured, or when the
    /// selection wears two different textures — there is no one answer then, and
    /// showing either shape's would silently apply it to the other on the first
    /// nudge of a slider.
    pub fn selected_texture(&self) -> Option<(buzz_scene::TextureRecipe, f64, f64)> {
        let scene = self.doc.scene();
        let mut found: Option<(buzz_scene::TextureRecipe, f64, f64)> = None;
        for id in self.selection.iter() {
            let Some((_, object)) = scene.find_object(id) else {
                continue;
            };
            let ObjectKind::Shape(shape) = &object.kind else {
                continue;
            };
            let this = shape
                .fill
                .as_ref()
                .and_then(|f| f.paint.image())
                .and_then(|img| img.asset.recipe.map(|r| (r, img.cell(), img.rotation())))?;
            match found {
                None => found = Some(this),
                Some(existing) if existing.0 == this.0 => {}
                Some(_) => return None,
            }
        }
        found
    }

    /// **Re-tune the texture on the selection.** The panel's sliders come here:
    /// the tile is re-baked from `recipe` and worn by every selected shape, so
    /// changing a colour or coarsening a wall is a live edit rather than an
    /// undo and a re-apply.
    pub fn retexture(&mut self, recipe: buzz_scene::TextureRecipe, placement: Option<(f64, f64)>) {
        self.apply_texture_recipe(recipe, placement);
    }

    /// Fill the selected shapes from a recipe.
    ///
    /// `placement` is the tile size and angle to wear it at; `None` fits a few
    /// repeats across each shape, which is what applying a texture for the first
    /// time should do. An asset already baked from this exact recipe is reused
    /// rather than baked again, so a document that puts one texture on twenty
    /// shapes holds one tile and uploads it once.
    fn apply_texture_recipe(
        &mut self,
        recipe: buzz_scene::TextureRecipe,
        placement: Option<(f64, f64)>,
    ) {
        // Gather the shapes and a per-shape tile size (a few repeats across the
        // smaller side) under an immutable borrow, before the edit takes a
        // mutable one.
        let targets: Vec<(ObjectId, f64)> = {
            let scene = self.doc.scene();
            self.selection
                .iter()
                .filter_map(|id| match scene.find_object(id) {
                    Some((_, o)) => match &o.kind {
                        ObjectKind::Shape(s) => {
                            let bb = s.path.bounding_box();
                            Some((id, (bb.width().min(bb.height()) / 5.0).max(16.0)))
                        }
                        _ => None,
                    },
                    None => None,
                })
                .collect()
        };
        if targets.is_empty() {
            self.status = Some("Select a shape to apply a texture to".into());
            return;
        }
        let label = recipe.kind.label();
        self.doc.edit("Apply Texture", |scene| {
            // The same recipe means the same tile: reuse it rather than filling
            // the library with copies.
            let asset = match scene.images().find_by_recipe(&recipe) {
                Some(existing) => existing,
                None => {
                    let name = scene.images().unique_name(label);
                    let id = scene.next_image_id();
                    let asset = buzz_scene::ImageAsset::from_recipe(id, name, recipe, 256);
                    scene.images_mut().insert(asset)
                }
            };
            for &(id, fitted) in &targets {
                let fill = match placement {
                    Some((cell, rotation)) => {
                        buzz_scene::ImageFill::tiled(std::sync::Arc::clone(&asset), cell)
                            .with_cell_rotation(cell, rotation)
                    }
                    None => buzz_scene::ImageFill::tiled(std::sync::Arc::clone(&asset), fitted),
                };
                scene.update_object_across(0, u32::MAX, id, |o| {
                    if let ObjectKind::Shape(s) = &mut o.kind {
                        s.fill = Some(buzz_scene::FillSpec::image(fill.clone()));
                    }
                });
            }
        });
        self.doc.end_gesture();
        self.status = Some(format!("{label} texture applied"));
    }

    /// **Import a video to trace over**, one frame of it per frame of the film.
    ///
    /// # Why the frames are pulled apart rather than played
    ///
    /// A rotoscope reference has to be *scrubbable*: dragging the playhead back
    /// and forth over six frames while drawing is the whole activity, and no
    /// video decoder is good at that. Frames are pulled out once, up front, and
    /// become ordinary keyframed artwork on a guide layer — which means the
    /// reference scrubs at the speed of everything else, survives being saved,
    /// and needs no decoder at all in the drawing path.
    ///
    /// # What it costs, and what is done about it
    ///
    /// One picture per frame is a lot of pictures, so each is scaled to fit the
    /// stage — a reference is looked at, not exported, and there is no point
    /// keeping detail finer than the film — and the count is capped. The layer
    /// is a **guide**: drawn while working, never in the film.
    ///
    /// Frames are taken at the *document's* rate, so frame 1 of the film is the
    /// video a frame in, whatever rate the file was shot at.
    /// **Import a folder of numbered pictures as one drawing per frame.**
    ///
    /// # Why this was missing and shouldn't have been
    ///
    /// The exporter has written PNG sequences from the start, and the frame
    /// machinery to place many pictures on many frames arrived with the video
    /// reference layer — but importing a sequence took the pictures one file at
    /// a time. Scanned drawings, frames from another program, a render from
    /// somewhere else: all of them arrive as a numbered folder, and all of them
    /// had to be brought in by hand.
    ///
    /// # Ordered by their numbers, not by their names
    ///
    /// `frame2.png` comes before `frame10.png`, which sorting by name gets
    /// exactly backwards — and getting the order wrong on an import of two
    /// hundred drawings is not something anybody would enjoy discovering later.
    /// Files with no number in them keep their name order, after the numbered
    /// ones.
    ///
    /// Each picture lands on a **blank** keyframe of its own: an ordinary
    /// keyframe carries the frame before it forward, which would stack the whole
    /// sequence on its own last frame.
    pub fn import_image_sequence(&mut self, folder: &std::path::Path) -> anyhow::Result<usize> {
        let entries = anyhow::Context::with_context(std::fs::read_dir(folder), || {
            format!("reading {}", folder.display())
        })?;
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .is_some_and(|e| {
                        matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
                    })
            })
            .collect();
        if files.is_empty() {
            anyhow::bail!("no pictures in {}", folder.display());
        }
        files.sort_by_key(|p| {
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            (trailing_number(&name).is_none(), trailing_number(&name), name)
        });

        // Read and decode before the document is touched, so a folder with one
        // bad file in it does not leave half a sequence behind.
        let mut pictures = Vec::with_capacity(files.len());
        for file in &files {
            let bytes = anyhow::Context::with_context(std::fs::read(file), || {
                format!("reading {}", file.display())
            })?;
            let name = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Frame".to_string());
            pictures.push((name, bytes));
        }

        let folder_name = folder
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Sequence".to_string());
        let stage = self.doc.scene().stage().size;
        let layer = self.doc_add_layer(&folder_name, LayerKind::Normal);
        let count = pictures.len() as u32;
        let mut placed = 0usize;

        self.doc.edit("Import Sequence", |scene| {
            scene.update_layer(layer, |l| {
                while l.frames.length() < count {
                    l.frames.insert_frame(l.frames.length());
                }
            });

            for (index, (name, bytes)) in pictures.iter().enumerate() {
                let frame = index as u32;
                let id = scene.next_image_id();
                let Ok(asset) = buzz_scene::ImageAsset::decode(id, name.clone(), bytes) else {
                    continue;
                };
                let asset = scene.images_mut().insert(asset);

                // Centred on the stage at its own size, as an imported picture
                // is: a sequence is usually already the size it was rendered at.
                let (w, h) = (f64::from(asset.width), f64::from(asset.height));
                let rect = buzz_geom::Rect::new(
                    (stage.width - w) / 2.0,
                    (stage.height - h) / 2.0,
                    (stage.width + w) / 2.0,
                    (stage.height + h) / 2.0,
                );
                let fill = buzz_scene::ImageFill::new(asset, rect);

                scene.update_layer(layer, |l| {
                    l.frames.insert_blank_keyframe(frame);
                });
                if scene
                    .add_shape_at(
                        layer,
                        frame,
                        ShapeData {
                            path: buzz_geom::Shape::to_path(&rect, 1e-9),
                            fill: Some(buzz_scene::FillSpec::image(fill)),
                            stroke: None,
                            blend: buzz_scene::PaintBlend::Normal,
                            },
                    )
                    .is_some()
                {
                    placed += 1;
                }
            }
        });
        self.doc.end_gesture();
        self.selection.set_active_layer(Some(layer));
        self.status = Some(format!(
            "{placed} drawings from {folder_name}, one to a frame"
        ));
        Ok(placed)
    }

    pub fn import_video_reference(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        /// As many frames as a reference layer will take. Twenty seconds at
        /// twenty-four is a long shot to rotoscope in one go, and ten times
        /// this would be a document nobody could open.
        const MAX_FRAMES: u32 = 480;

        let info = buzz_export::video::probe(path)?;
        let stage = self.doc.scene().stage().size;
        let fps = self.doc.scene().stage().frame_rate.max(1.0);

        let scratch = anyhow::Context::context(tempfile::tempdir(), "making somewhere to put the frames")?;
        let files = buzz_export::video::extract_frames(
            path,
            fps,
            (
                stage.width.round().max(2.0) as u32,
                stage.height.round().max(2.0) as u32,
            ),
            MAX_FRAMES,
            scratch.path(),
        )?;

        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Video".to_string());

        // Decoded outside the edit: reading and decoding hundreds of PNGs is
        // not something to do while holding the document.
        let mut decoded = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let bytes = anyhow::Context::with_context(std::fs::read(file), || {
                format!("reading frame {}", index + 1)
            })?;
            decoded.push(bytes);
        }

        let layer = self.doc_add_layer(&name, LayerKind::Guide);
        let count = decoded.len() as u32;
        let mut placed = 0u32;
        self.doc.edit("Import Video Reference", |scene| {
            // The layer has to be as long as the footage before there are
            // frames to key on.
            scene.update_layer(layer, |l| {
                while l.frames.length() < count {
                    l.frames.insert_frame(l.frames.length());
                }
            });

            for (index, bytes) in decoded.iter().enumerate() {
                let frame = index as u32;
                let id = scene.next_image_id();
                let Ok(asset) = buzz_scene::ImageAsset::decode(id, format!("{name} {}", frame + 1), bytes)
                else {
                    continue;
                };
                let asset = scene.images_mut().insert(asset);

                // Centred on the stage at its own size — the scale ffmpeg was
                // asked for already fits it.
                let (w, h) = (f64::from(asset.width), f64::from(asset.height));
                let rect = buzz_geom::Rect::new(
                    (stage.width - w) / 2.0,
                    (stage.height - h) / 2.0,
                    (stage.width + w) / 2.0,
                    (stage.height + h) / 2.0,
                );
                let fill = buzz_scene::ImageFill::new(asset, rect);

                // A **blank** keyframe, not an ordinary one: an ordinary
                // keyframe carries the previous frame's artwork forward, so
                // every frame of the clip would arrive stacked on top of every
                // frame before it. Each frame of a video replaces the last.
                scene.update_layer(layer, |l| {
                    l.frames.insert_blank_keyframe(frame);
                });
                if scene
                    .add_shape_at(
                        layer,
                        frame,
                        ShapeData {
                            path: buzz_geom::Shape::to_path(&rect, 1e-9),
                            fill: Some(buzz_scene::FillSpec::image(fill)),
                            stroke: None,
                            blend: buzz_scene::PaintBlend::Normal,
                        },
                    )
                    .is_some()
                {
                    placed += 1;
                }
            }
        });
        self.doc.end_gesture();
        self.selection.set_active_layer(Some(layer));

        let capped = if count >= MAX_FRAMES {
            format!(
                " (the first {MAX_FRAMES} of {:.0})",
                info.seconds * info.fps
            )
        } else {
            String::new()
        };
        self.status = Some(format!(
            "{placed} frames of {name} on a reference layer{capped} — draw over it"
        ));
        Ok(())
    }

    /// **Fill the selected shapes with an image file.** Decodes it, adds it to
    /// the library, and sets it as each selected shape's fill: `tile` repeats a
    /// seamless texture across the shape, otherwise one copy is laid across the
    /// shape's bounds (what a photograph wants). One undo step.
    pub fn fill_selection_with_image(
        &mut self,
        path: &std::path::Path,
        tile: bool,
    ) -> anyhow::Result<()> {
        let bytes = std::fs::read(path)?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Bitmap".to_string());
        let targets: Vec<(ObjectId, buzz_geom::Rect)> = {
            let scene = self.doc.scene();
            self.selection
                .iter()
                .filter_map(|id| match scene.find_object(id) {
                    Some((_, o)) => match &o.kind {
                        ObjectKind::Shape(s) => Some((id, s.path.bounding_box())),
                        _ => None,
                    },
                    None => None,
                })
                .collect()
        };
        if targets.is_empty() {
            anyhow::bail!("select a shape to fill with the image");
        }
        let mut failure: Option<String> = None;
        self.doc.edit("Fill With Image", |scene| {
            let asset = match scene.add_image(&name, &bytes) {
                Ok(a) => a,
                Err(e) => {
                    failure = Some(e.to_string());
                    return;
                }
            };
            for &(id, bounds) in &targets {
                let fill = if tile {
                    let cell = (bounds.width().min(bounds.height()) / 3.0).max(24.0);
                    buzz_scene::ImageFill::tiled(std::sync::Arc::clone(&asset), cell)
                } else {
                    buzz_scene::ImageFill::new(std::sync::Arc::clone(&asset), bounds)
                };
                scene.update_object_across(0, u32::MAX, id, |o| {
                    if let ObjectKind::Shape(s) = &mut o.kind {
                        s.fill = Some(buzz_scene::FillSpec::image(fill.clone()));
                    }
                });
            }
        });
        if let Some(e) = failure {
            self.doc.undo();
            anyhow::bail!(e);
        }
        self.doc.end_gesture();
        Ok(())
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
    /// The artwork the playhead is standing on, **across every layer** — the
    /// character, the background and the overlay together, because that is
    /// what "this frame" means to the person looking at it. It used to take
    /// the active layer alone, which made copying a drawing a job of selecting
    /// each layer and repeating yourself.
    ///
    /// Animate copies a *selected span* of frames; there is still no span
    /// selection in the timeline here, so this is the one frame the playhead
    /// is on — recorded in §7.
    fn copy_frames(&mut self, and_clear: bool) {
        let frame = self.current_frame;
        let scene = self.doc.scene();
        let taken: Vec<(LayerId, Vec<std::sync::Arc<Object>>)> = scene
            .layers()
            .iter()
            .filter_map(|layer| {
                let contents = layer.frames.frame_contents(frame)?;
                Some((layer.id, contents))
            })
            .collect();

        if taken.is_empty() {
            self.status = Some("There is no frame here to copy".into());
            return;
        }
        let layers = taken.len();
        let count: usize = taken.iter().map(|(_, objects)| objects.len()).sum();
        self.frame_clipboard = Some(taken);

        if and_clear {
            // Cleared on every layer it was taken from, or a cut would leave
            // most of the drawing where it was.
            let layers: Vec<LayerId> = self
                .frame_clipboard
                .iter()
                .flatten()
                .map(|(layer, _)| *layer)
                .collect();
            self.doc.edit("Cut Frames", |scene| {
                for layer in layers {
                    if scene.layers().is_effectively_locked(layer) {
                        continue;
                    }
                    scene.update_layer(layer, |l| {
                        l.frames.clear_frames(frame);
                    });
                }
            });
            self.selection.prune(self.doc.scene());
        }
        self.status = Some(format!(
            "{} {count} object{} from frame {} of {layers} layer{}",
            if and_clear { "Cut" } else { "Copied" },
            if count == 1 { "" } else { "s" },
            frame + 1,
            if layers == 1 { "" } else { "s" },
        ));
    }

    /// **Paste Frames** onto the frame the playhead is on.
    ///
    /// Each layer's artwork goes back onto **the layer it came from**, so a
    /// copied drawing arrives assembled rather than flattened onto whichever
    /// layer happened to be active. A layer that has since been deleted is
    /// skipped rather than guessed at.
    ///
    /// A keyframe is made on each of them first, as Animate does: pasting into
    /// the middle of a span would otherwise change the artwork from wherever
    /// that span began. The objects are given fresh ids, so pasting twice
    /// gives two drawings rather than one shared between two frames.
    fn paste_frames(&mut self) {
        let Some(clipboard) = self.frame_clipboard.clone() else {
            self.status = Some("There are no frames on the clipboard".into());
            return;
        };

        let frame = self.current_frame;
        let mut pasted = 0usize;
        let mut layers = 0usize;
        let mut locked = 0usize;
        self.doc.edit("Paste Frames", |scene| {
            for (layer, objects) in &clipboard {
                if scene.layers().get(*layer).is_none() {
                    continue;
                }
                if scene.layers().is_effectively_locked(*layer) {
                    locked += 1;
                    continue;
                }
                scene.update_layer(*layer, |l| {
                    l.frames.insert_frame(frame);
                    l.frames.insert_blank_keyframe(frame);
                });
                for object in objects {
                    let mut copy = (**object).clone();
                    copy.id = scene.next_object_id();
                    if scene.add_object_at(*layer, frame, copy).is_some() {
                        pasted += 1;
                    }
                }
                layers += 1;
            }
        });

        if layers == 0 {
            // Nothing was undone here on purpose. The edit changed nothing, so
            // `Document::edit` recorded nothing, and calling `undo` would take
            // back whatever the user did *before* this — which is how a paste
            // that landed nowhere could delete a layer.
            self.status = Some(if locked > 0 {
                "Every layer on the clipboard is locked".into()
            } else {
                "The layers these frames came from are gone".to_string()
            });
            return;
        }
        self.status = Some(format!(
            "Pasted {pasted} object{} onto frame {} of {layers} layer{}{}",
            if pasted == 1 { "" } else { "s" },
            frame + 1,
            if layers == 1 { "" } else { "s" },
            if locked > 0 {
                format!(" \u{2014} {locked} locked layer(s) skipped")
            } else {
                String::new()
            }
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

    /// **Write a named camera move from the playhead to the end of the scene.**
    ///
    /// # Why it runs to the end and not for a fixed two seconds
    ///
    /// Because that is what a camera move in a shot nearly always does. A push
    /// in, a drift and a reveal are all the length of the shot they are in —
    /// they are not events inside it — and a fixed duration would be wrong
    /// every time the shot was not that long, in both directions. What comes
    /// out is two ordinary keys, so shortening the move is dragging the second
    /// one, which is a thing an animator can see and do; guessing a length they
    /// then have to *find* and correct is not.
    fn add_camera_move(&mut self, movement: buzz_scene::CameraMove) {
        let from = self.current_frame;
        let last = self.doc.scene().frame_count().saturating_sub(1);
        let stage = self.doc.scene().stage().stage_rect();

        if last <= from {
            self.status = Some(format!(
                "{} needs frames to move across \u{2014} the playhead is at the end of the scene",
                movement.label()
            ));
            return;
        }

        let mut wrote = false;
        self.doc.edit(movement.label(), |scene| {
            // Turning the camera on is part of the command: a move written to a
            // camera nobody has enabled would do nothing and say nothing.
            scene.camera_mut().enabled = true;
            wrote = scene.camera_mut().add_move(movement, from, last, stage);
        });

        self.status = Some(if wrote {
            format!(
                "{} over {} frames \u{2014} drag the second camera key to re-time it",
                movement.label(),
                last - from
            )
        } else {
            format!("Could not write a {}", movement.label())
        });
    }

    /// Key the selected light's current state at the playhead.
    fn add_light_key(&mut self) {
        let frame = self.current_frame;
        match self.light_panel.selected {
            Some(id) => {
                self.key_light_at(id, frame);
                self.status = Some("Light keyframe added".into());
            }
            None => self.status = Some("Select a light first".into()),
        }
    }

    /// Remove the selected light's key at the playhead.
    fn remove_light_key(&mut self) {
        let frame = self.current_frame;
        if let Some(id) = self.light_panel.selected {
            self.unkey_light_at(id, frame);
        }
    }

    /// Key one light's current state at `frame` — the shared path for the panel
    /// button, the menu command and the timeline channel. Keying turns the
    /// light's track on and snapshots its whole state, exactly as a camera key
    /// captures the whole camera.
    pub fn key_light_at(&mut self, id: buzz_scene::LightId, frame: u32) {
        self.doc.edit("Light Keyframe", |scene| {
            let key = scene
                .lights()
                .get(id)
                .map(|light| buzz_scene::LightKey::from_light(frame, light));
            if let Some(key) = key
                && let Some(light) = scene.lights_mut().get_mut(id)
            {
                let track = light.track.get_or_insert_with(buzz_scene::LightTrack::new);
                track.enabled = true;
                track.set_key(key);
            }
        });
        self.doc.end_gesture();
    }

    /// Remove one light's key at `frame`.
    pub fn unkey_light_at(&mut self, id: buzz_scene::LightId, frame: u32) {
        self.doc.edit("Remove Light Keyframe", |scene| {
            if let Some(light) = scene.lights_mut().get_mut(id)
                && let Some(track) = light.track.as_mut()
            {
                track.remove_key(frame);
            }
        });
        self.doc.end_gesture();
    }

    /// **Bake an object's live modifiers into keyframes**, then remove them —
    /// the inverse of adding a live modifier. Evaluates the modifier stack across
    /// the film and writes the result as ordinary keyframes on twos, so the
    /// motion becomes hand-editable and no longer re-computes.
    pub fn bake_modifiers(&mut self, id: ObjectId) {
        let Some((layer, _)) = self.doc.scene().find_object(id) else {
            return;
        };
        let span = self.doc.scene().frame_count().max(1);
        let last = span - 1;
        let step = 2u32;
        let mut frames: Vec<u32> = (0..span).step_by(step as usize).collect();
        if frames.last() != Some(&last) {
            frames.push(last);
        }

        // Phase 1: read the modified state at each frame, with the modifiers
        // still active. Collected before mutating so the borrow is done.
        let mut baked: Vec<(u32, Affine, Option<Vec<f64>>)> = Vec::new();
        {
            let scene = self.doc.scene();
            let Some(l) = scene.layers().get(layer) else {
                return;
            };
            for &frame in &frames {
                let resolved = l.frames.resolved_at(frame);
                let Some(obj) = resolved.iter().find(|o| o.id == id).cloned() else {
                    continue;
                };
                let Some(ev) = scene.modified_object_at(layer, &obj, frame) else {
                    continue;
                };
                let base = ev.object.as_ref().unwrap_or(&obj);
                let transform = ev.prepend * base.transform;
                let pose = match &base.kind {
                    ObjectKind::Armature(rig) => Some(rig.armature.pose()),
                    _ => None,
                };
                baked.push((frame, transform, pose));
            }
        }
        if baked.is_empty() {
            return;
        }

        // Phase 2: write the keyframes and drop the modifiers, so the baked
        // motion is not then applied a second time on top of itself.
        self.doc.edit("Bake Modifiers", |scene| {
            scene.update_layer(layer, |l| {
                if l.frames.length() <= last {
                    l.frames.insert_frame(last);
                }
            });
            for (frame, transform, pose) in &baked {
                scene.ensure_keyframe(layer, *frame);
                scene.update_object_at(*frame, id, |o| {
                    o.transform = *transform;
                    if let (Some(p), ObjectKind::Armature(rig)) = (pose.as_ref(), &mut o.kind) {
                        rig.armature.set_pose(p);
                    }
                });
                if *frame != last {
                    scene.update_layer(layer, |l| {
                        l.frames.set_tween(*frame, buzz_scene::Tween::motion());
                    });
                }
            }
            scene.update_object_across(0, u32::MAX, id, |o| o.modifiers.clear());
        });
        self.doc.end_gesture();
        self.status = Some("Baked the live modifiers into keyframes".into());
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

    /// Each selected object's id and its bounds on the current frame, in a
    /// stable order.
    ///
    /// Shared by align, distribute and match size, all of which are "measure
    /// everything, then move everything" and must measure against the state
    /// *before* any of them moved — a running fold would make the answer
    /// depend on the order.
    fn selection_bounds(&self) -> Vec<(buzz_scene::ObjectId, kurbo::Rect)> {
        let scene = self.doc.scene();
        let frame = self.current_frame;
        self.selection
            .ids()
            .into_iter()
            .filter_map(|id| {
                scene
                    .layers()
                    .iter()
                    .find_map(|l| l.objects_at(frame).iter().find(|o| o.id == id).cloned())
                    .or_else(|| scene.find_object(id).map(|(_, o)| o.clone()))
                    .map(|o| (id, o.bounds()))
            })
            .collect()
    }

    /// Move each selected object by its own offset, as one undo step.
    fn offset_selection(
        &mut self,
        offsets: &[(buzz_scene::ObjectId, buzz_geom::Vec2)],
        label: &'static str,
    ) {
        if offsets.iter().all(|(_, d)| d.x == 0.0 && d.y == 0.0) {
            return;
        }
        let offsets = offsets.to_vec();
        let at = self.edit_at();
        self.doc.edit(label, |scene| {
            for (id, delta) in offsets {
                update_object(scene, at, id, |o| {
                    o.transform = Affine::translate(delta) * o.transform;
                });
            }
        });
    }

    fn align_selection(&mut self, op: buzz_ui::Align, to_stage: bool) {
        let measured = self.selection_bounds();
        if measured.is_empty() {
            self.status = Some("Select artwork to align".into());
            return;
        }
        let stage = to_stage.then(|| self.doc.scene().stage().stage_rect());
        let bounds: Vec<kurbo::Rect> = measured.iter().map(|(_, r)| *r).collect();
        let offsets = buzz_ui::align::align_offsets(&bounds, op, stage);

        let moves: Vec<_> = measured
            .iter()
            .map(|(id, _)| *id)
            .zip(offsets)
            .collect();
        self.offset_selection(&moves, "Align");
    }

    fn distribute_selection(&mut self, op: buzz_ui::Distribute) {
        let measured = self.selection_bounds();
        if measured.len() < 3 {
            // Said rather than silently ignored: two objects look like they
            // ought to distribute, and nothing happening is indistinguishable
            // from a broken menu item.
            self.status = Some("Select three or more objects to distribute".into());
            return;
        }
        let bounds: Vec<kurbo::Rect> = measured.iter().map(|(_, r)| *r).collect();
        let offsets = buzz_ui::align::distribute_offsets(&bounds, op);

        let moves: Vec<_> = measured
            .iter()
            .map(|(id, _)| *id)
            .zip(offsets)
            .collect();
        self.offset_selection(&moves, "Distribute");
    }

    fn match_selection_size(&mut self, op: buzz_ui::MatchSize) {
        let measured = self.selection_bounds();
        if measured.len() < 2 {
            self.status = Some("Select two or more objects to match their size".into());
            return;
        }
        let bounds: Vec<kurbo::Rect> = measured.iter().map(|(_, r)| *r).collect();
        let scales = buzz_ui::align::match_size_scales(&bounds, op);

        let work: Vec<_> = measured
            .iter()
            .zip(scales)
            .filter(|(_, (sx, sy))| (sx - 1.0).abs() > 1e-9 || (sy - 1.0).abs() > 1e-9)
            .map(|((id, rect), scale)| (*id, rect.center(), scale))
            .collect();
        if work.is_empty() {
            return;
        }

        let at = self.edit_at();
        self.doc.edit("Match Size", |scene| {
            for (id, centre, (sx, sy)) in work {
                // About the object's own centre, so nothing wanders across the
                // stage while being resized.
                let about = Affine::translate(centre.to_vec2())
                    * Affine::scale_non_uniform(sx, sy)
                    * Affine::translate(-centre.to_vec2());
                update_object(scene, at, id, |o| o.transform = about * o.transform);
            }
        });
    }

    /// Put the selection on the clipboard. `false` when there was nothing to
    /// put there, which is what stops Cut deleting on an empty selection.
    ///
    /// The artwork is taken **as it is on the current frame**, for the reason
    /// `Scene::extract` gives: the same object appears on several keyframes
    /// with different transforms, and what you copied is what you were looking
    /// at.
    fn copy_selection(&mut self) -> bool {
        if self.selection.is_empty() {
            self.status = Some("Select artwork to copy".into());
            return false;
        }
        let ids = self.selection.ids();
        let lifted = self.doc.scene().extract(self.current_frame, &ids);
        let count = ids.len();
        self.clipboard = Some(lifted);
        self.status = Some(format!(
            "Copied {count} {}",
            if count == 1 { "object" } else { "objects" }
        ));
        true
    }

    /// Paste the clipboard onto the active layer at the playhead.
    ///
    /// The clipboard is **not** consumed: pasting the same character into four
    /// scenes is the thing this exists for.
    fn paste_clipboard(&mut self) {
        let Some(lifted) = self.clipboard.clone() else {
            self.status = Some("There is nothing on the clipboard".into());
            return;
        };
        let scene = self.doc.scene().clone();
        self.selection.ensure_active_layer(&scene);
        let Some(layer) = self.selection.active_layer() else {
            self.status = Some("There is no layer to paste onto".into());
            return;
        };

        // Offset like Duplicate does, and for the same reason: a copy landing
        // exactly on its original is indistinguishable from nothing happening.
        const OFFSET: (f64, f64) = (10.0, 10.0);
        let frame = self.current_frame;
        let before: Vec<buzz_scene::ObjectId> = scene
            .layers()
            .get(layer)
            .map(|l| l.objects_at(frame).iter().map(|o| o.id).collect())
            .unwrap_or_default();

        let mut report = None;
        self.doc.edit("Paste", |scene| {
            report = Some(scene.merge(
                &lifted,
                buzz_scene::ImportTarget::Onto {
                    layer,
                    frame,
                    offset: buzz_geom::Vec2::new(OFFSET.0, OFFSET.1),
                },
            ));
        });

        // What arrived is what the user now wants to move — the same rule
        // Duplicate and Place Asset follow.
        let arrived: Vec<buzz_scene::ObjectId> = self
            .doc
            .scene()
            .layers()
            .get(layer)
            .map(|l| {
                l.objects_at(frame)
                    .iter()
                    .map(|o| o.id)
                    .filter(|id| !before.contains(id))
                    .collect()
            })
            .unwrap_or_default();
        let count = arrived.len();
        self.selection.set(arrived);

        self.status = Some(match report {
            Some(report) if report.symbols > 0 => format!(
                "Pasted {count} {} and {} symbol{}",
                if count == 1 { "object" } else { "objects" },
                report.symbols,
                if report.symbols == 1 { "" } else { "s" }
            ),
            _ => format!(
                "Pasted {count} {}",
                if count == 1 { "object" } else { "objects" }
            ),
        });
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
                    let Some(stroke) = s.stroke.clone() else {
                        return;
                    };
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
                        swatch: None,
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
        self.transform_selection(Affine::translate(c) * scale * Affine::translate(-c), label);
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

    /// **Turn the selected bitmaps into artwork** — Animate's Trace Bitmap.
    ///
    /// # Why the picture is replaced rather than traced alongside
    ///
    /// Because the point of tracing is to stop having a bitmap. Leaving the
    /// photograph underneath means every later selection, bucket fill and
    /// nudge has to be aimed past it, and the document carries the pixels for
    /// ever. Animate replaces it, and one `Ctrl + Z` puts it back — which is
    /// the honest version of "you can always get it back".
    ///
    /// # Where the artwork lands
    ///
    /// [`buzz_scene::trace`] works in the picture's own pixels and knows
    /// nothing about the stage, so the paths come back at pixel scale. Getting
    /// them onto the stage is the composition of what the editor already knows:
    /// the unit square to the image's own space (the fill's transform), and
    /// that space to the layer (the object's).
    fn trace_selection(&mut self, options: buzz_scene::TraceOptions) {
        let ids = self.selection.ids();
        if ids.is_empty() {
            self.status = Some("Select a picture to trace first".into());
            return;
        }
        let at = self.edit_at();

        // Everything needed is read before the document is touched, so a
        // picture that turns out not to be traceable leaves no half-edit.
        let mut jobs: Vec<(LayerId, ObjectId, Affine, buzz_scene::TraceReport)> = Vec::new();
        let mut not_pictures = 0usize;
        for id in &ids {
            let Some((layer, object)) = self.doc.scene().find_object(*id) else {
                continue;
            };
            let ObjectKind::Shape(shape) = &object.kind else {
                not_pictures += 1;
                continue;
            };
            let Some(buzz_scene::Paint::Image(fill)) = shape.fill.as_ref().map(|f| &f.paint) else {
                not_pictures += 1;
                continue;
            };
            let asset = &fill.asset;
            if asset.width == 0 || asset.height == 0 {
                not_pictures += 1;
                continue;
            }
            let report = buzz_scene::trace(asset.width, asset.height, &asset.pixels, &options);
            if report.shapes.is_empty() {
                self.status = Some(report.message);
                return;
            }
            // Pixels → the unit square → the image's own space → the layer.
            let place = object.transform
                * fill.transform
                * Affine::scale_non_uniform(
                    1.0 / asset.width as f64,
                    1.0 / asset.height as f64,
                );
            jobs.push((layer, *id, place, report));
        }

        if jobs.is_empty() {
            self.status = Some(if not_pictures > 0 {
                "Nothing selected is a picture \u{2014} tracing turns an imported bitmap \
                 into shapes"
                    .to_string()
            } else {
                "Nothing there to trace".to_string()
            });
            return;
        }

        let mut made = 0usize;
        let mut specks = 0usize;
        let mut colours_note = String::new();
        let mut fresh: Vec<ObjectId> = Vec::new();
        self.doc.edit("Trace Bitmap", |scene| {
            for (layer, id, place, report) in &jobs {
                specks += report.specks;
                colours_note = report.message.clone();
                for shape in &report.shapes {
                    let mut shape = shape.clone();
                    shape.path = *place * shape.path.clone();
                    if let Some(new) = scene.add_shape_at(*layer, at.frame, shape) {
                        fresh.push(new);
                        made += 1;
                    }
                }
                // The picture goes last, so the artwork it became is already
                // standing in for it and nothing flickers through an empty
                // frame in between.
                scene.remove_object(*id);
            }
        });

        self.selection.clear();
        for id in fresh {
            self.selection.toggle(id);
        }
        self.status = Some(if specks > 0 {
            format!("{colours_note} \u{2014} {made} shapes on the stage")
        } else {
            format!("Traced {made} shapes")
        });
    }

    /// **Re-weight the outlines of everything selected**, and nothing else.
    ///
    /// # What it touches, and what it deliberately does not
    ///
    /// Only a shape's **stroke**. The path is untouched, the fill is untouched,
    /// and a shape with no stroke — which includes every brush stroke in this
    /// program, whose "line" is a filled outline rather than a stroked one — is
    /// left exactly as it was. Dilating a *fill* is a different operation with
    /// a different failure mode, and it already has a command of its own:
    /// `Modify ▸ Shape ▸ Expand Fill`.
    ///
    /// # Why multiply rather than add
    ///
    /// So one press means the same thing everywhere. Adding half a unit is
    /// invisible on a six-unit outline and doubles a half-unit one, which makes
    /// the command behave differently on different parts of the same drawing —
    /// exactly when an animator is pressing it repeatedly to match them.
    ///
    /// # Hairlines
    ///
    /// A hairline is one screen pixel at any zoom, so it has no width to
    /// scale. Thickening one therefore **makes it a real line first** at the
    /// width a hairline reads as, and then scales that; thinning one leaves it
    /// alone, because there is nothing thinner to become.
    fn scale_selected_strokes(&mut self, factor: f64) {
        let ids = self.selection.ids();
        if ids.is_empty() {
            self.status = Some("Select something with an outline first".into());
            return;
        }
        let at = self.edit_at();
        let label = if factor >= 1.0 { "Thicken Lines" } else { "Thin Lines" };

        let mut touched = 0usize;
        self.doc.edit(label, |scene| {
            for id in ids {
                update_shape(scene, at, id, |s| {
                    let Some(stroke) = s.stroke.as_mut() else {
                        return;
                    };
                    if stroke.hairline {
                        if factor < 1.0 {
                            // Already the thinnest a line can be drawn.
                            return;
                        }
                        stroke.hairline = false;
                        stroke.width = 1.0;
                    }
                    // Bounded at both ends: a width of zero is an invisible
                    // line that cannot be thickened back, and one the size of
                    // the stage is a fill nobody asked for.
                    stroke.width = (stroke.width * factor).clamp(0.05, 400.0);
                    touched += 1;
                });
            }
        });

        self.status = Some(match touched {
            0 => "Nothing in the selection has an outline \u{2014} brush strokes are fills, \
                  and Modify \u{25b8} Shape \u{25b8} Expand Fill is what widens those"
                .to_string(),
            1 => format!("{label}: one outline"),
            n => format!("{label}: {n} outlines"),
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
                    if recognise && let Some((path, _)) = buzz_geom::recognise(&s.path, tolerance) {
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
/// **Does `name` name `who`** — exactly, or as a word inside it?
///
/// `Ana` matches `Ana`, `Ana Mouth` and `Ana_mouth`, and does not match
/// `Anabel` or `Banana`. Word-bounded rather than a substring search, because
/// a substring match would have `Ana` driving `Anabel`'s mouth and the animator
/// would spend an hour looking for the reason.
fn name_mentions(name: &str, who: &str) -> bool {
    if name.eq_ignore_ascii_case(who) {
        return true;
    }
    let boundary = |c: char| !c.is_alphanumeric();
    name.split(boundary)
        .any(|word| word.eq_ignore_ascii_case(who))
}

fn update_shape(scene: &mut Scene, at: EditAt, id: ObjectId, mut f: impl FnMut(&mut ShapeData)) {
    update_object(scene, at, id, |o| {
        if let ObjectKind::Shape(shape) = &mut o.kind {
            f(shape);
        }
    });
}

/// **Make this document the home of any bitmap the shape paints with.**
///
/// A brush captured from artwork keeps a shared handle on that artwork's
/// texture, and a brush outlives the document it was made in. Painting with it
/// somewhere else would leave a picture referred to by an identity the new
/// file's library never issued: on screen while the handle is alive, gone the
/// moment it is saved and reopened.
fn adopt_textures(scene: &mut Scene, shape: &mut ShapeData) {
    if let Some(fill) = shape.fill.as_mut() {
        adopt_texture(scene, &mut fill.paint);
    }
    if let Some(stroke) = shape.stroke.as_mut() {
        adopt_texture(scene, &mut stroke.paint);
    }
}

/// See [`adopt_textures`]. Anything that is not a bitmap is left alone.
fn adopt_texture(scene: &mut Scene, paint: &mut buzz_scene::Paint) {
    let buzz_scene::Paint::Image(fill) = paint else {
        return;
    };
    match scene.images().get(fill.asset.id).map(|a| a.blob_id()) {
        // Already this document's own pixels: nothing to do, nothing copied.
        Some(blob) if blob == fill.asset.blob_id() => {}
        // The same id holding **different pixels** — a document opened twice,
        // an import run again, a brush carried in from another file. Re-homed
        // under an id this document has not issued, because the renderer's
        // atlas keeps whichever picture arrived under an identity first and
        // would serve it for both. See `ImageAsset::blob_id`.
        Some(_) => {
            let mut copy = (*fill.asset).clone();
            copy.id = scene.next_image_id();
            copy.name = scene.images().unique_name(&copy.name);
            fill.asset = scene.images_mut().insert(copy);
        }
        // Not here yet: adopt it under the id it already carries.
        None => {
            fill.asset = scene.images_mut().insert((*fill.asset).clone());
        }
    }
}

/// Collect the walls the paint bucket must respect from one object, recursing
/// into groups. Instances, rigs and warped artwork are left out: the bucket
/// fills flat drawing, which is the only place the question is well posed.
fn collect_bucket_boundaries(
    object: &buzz_scene::Object,
    transform: Affine,
    out: &mut Vec<buzz_scene::Boundary>,
) {
    let t = transform * object.transform;
    match &object.kind {
        ObjectKind::Shape(shape) => out.push(buzz_scene::Boundary::from_shape(shape, t)),
        ObjectKind::Group(children) => {
            for child in children {
                collect_bucket_boundaries(child, t, out);
            }
        }
        _ => {}
    }
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
/// Carry every point of a path through `f`.
///
/// For the Lasso, whose region is drawn in view space and has to be cut out of
/// artwork living several transforms further in. `None` if any point cannot be
/// carried — a layer behind the camera, or a collapsed transform — because half
/// a region is worse than none.
///
/// Control points go through the same map as the on-curve points, which is
/// exact for the affine and projective maps used here in the case that matters:
/// both the Lasso and the Magic Wand produce paths of straight lines, and a
/// straight line stays straight through both.
fn map_path(path: &BezPath, mut f: impl FnMut(Point) -> Option<Point>) -> Option<BezPath> {
    use buzz_geom::PathEl::*;
    let mut out = BezPath::new();
    for el in path.elements() {
        out.push(match *el {
            MoveTo(p) => MoveTo(f(p)?),
            LineTo(p) => LineTo(f(p)?),
            QuadTo(a, b) => QuadTo(f(a)?, f(b)?),
            CurveTo(a, b, c) => CurveTo(f(a)?, f(b)?, f(c)?),
            ClosePath => ClosePath,
        });
    }
    Some(out)
}

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
    let flat = scene
        .camera()
        .projection_at_depth(frame, stage, layer_depth)?;

    flat.then(&turned.inverse()?).map_point(point)
}

fn object_contains(
    scene: &Scene,
    object: &Object,
    point: Point,
    tolerance: f64,
    frame: u32,
    depth: usize,
    table: &std::collections::HashMap<buzz_scene::SymbolId, buzz_geom::Rect>,
) -> bool {
    // Cheap rejection first, resolved through the memoised bounds table so an
    // instance is rejected on its artwork's real extents — a lookup, not a
    // fresh recursive measure of its whole rig on every object of every click.
    if !scene
        .resolved_bounds_with(object, table)
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
            .any(|c| object_contains(scene, c, local, tolerance, frame, depth, table)),

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
            //
            // **Through the symbol's own layer parenting**, in reverse. A
            // character symbol is rigged inside itself, and the renderer draws
            // each part through the chain it follows; testing the parts where
            // they were drawn at rest meant a click on a raised arm missed it
            // and fell through to whatever was behind — which is the object
            // behind getting selected.
            symbol.layers.selectable().any(|l| {
                let follows = symbol.layers.inherited_transform(l.id, inner);
                let local = match invert(follows) {
                    Some(back) => back * local,
                    None => local,
                };
                l.objects_at(inner)
                    .iter()
                    .any(|c| object_contains(scene, c, local, tolerance, inner, depth + 1, table))
            })
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

/// The colour of the topmost shape under `point`, and whether it is the fill
/// (`true`) or the stroke — descending through instances and groups, so the
/// eyedropper picks up a rig's artwork, not the placeholder of the instance the
/// click first lands on. `None` when the point is over nothing paintable.
fn sampled_paint(
    scene: &Scene,
    object: &Object,
    point: Point,
    tolerance: f64,
    frame: u32,
    depth: usize,
    table: &std::collections::HashMap<buzz_scene::SymbolId, buzz_geom::Rect>,
) -> Option<(Color, bool)> {
    if !scene
        .resolved_bounds_with(object, table)
        .inflate(tolerance, tolerance)
        .contains(point)
    {
        return None;
    }
    let inverse = invert(object.transform)?;
    let local = inverse * point;

    match &object.kind {
        ObjectKind::Shape(shape) => {
            if let Some(fill) = &shape.fill
                && buzz_geom::hit::fill_contains(&shape.path, local, buzz_geom::FillMode::NonZero)
            {
                return Some((fill.color(), true));
            }
            if let Some(stroke) = &shape.stroke {
                let width = if stroke.hairline { 0.0 } else { stroke.width };
                if buzz_geom::hit::stroke_contains(&shape.path, local, width, tolerance) {
                    return Some((stroke.color(), false));
                }
            }
            None
        }
        // The last hit in draw order is the one on top.
        ObjectKind::Group(children) => {
            let mut hit = None;
            for child in children {
                if let Some(paint) = sampled_paint(scene, child, local, tolerance, frame, depth, table)
                {
                    hit = Some(paint);
                }
            }
            hit
        }
        ObjectKind::Instance(instance) => {
            if depth >= MAX_SYMBOL_DEPTH {
                return None;
            }
            let symbol = scene.library().get(instance.symbol)?;
            let inner = instance.resolve_frame(symbol.kind, frame, symbol.length());
            let mut hit = None;
            for layer in symbol.layers.selectable() {
                // The same reverse trip through the symbol's rigging that
                // `object_contains` makes; sampling a colour has to look where
                // the paint is drawn.
                let follows = symbol.layers.inherited_transform(layer.id, inner);
                let local = match invert(follows) {
                    Some(back) => back * local,
                    None => local,
                };
                for child in layer.objects_at(inner) {
                    if let Some(paint) =
                        sampled_paint(scene, child, local, tolerance, inner, depth + 1, table)
                    {
                        hit = Some(paint);
                    }
                }
            }
            hit
        }
        // Posed rig artwork is skipped for now — sampling its deformed fill is
        // recorded as a follow-up.
        ObjectKind::Armature(_) | ObjectKind::Warp(_) => None,
    }
}

/// How deep a symbol may nest before hit testing gives up.
///
/// Matches the renderer's limit in [`crate::stage`]: anything it will not draw
/// must not be clickable either.
const MAX_SYMBOL_DEPTH: usize = 12;

/// The extra placements a symmetry mode makes for one drawn shape — every copy
/// but the original, reflected or rotated about the centre of the stage.
fn symmetry_transforms(sym: SymmetrySettings, stage: buzz_geom::Size) -> Vec<Affine> {
    if !sym.is_on() {
        return Vec::new();
    }
    let c = buzz_geom::Vec2::new(stage.width / 2.0, stage.height / 2.0);
    // Reflect about a line through the centre: move to the origin, flip, move back.
    let reflect = |sx: f64, sy: f64| {
        Affine::translate(c) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-c)
    };
    match sym.mode {
        SymmetryMode::Off => Vec::new(),
        SymmetryMode::MirrorX => vec![reflect(-1.0, 1.0)],
        SymmetryMode::MirrorY => vec![reflect(1.0, -1.0)],
        SymmetryMode::Both => {
            vec![reflect(-1.0, 1.0), reflect(1.0, -1.0), reflect(-1.0, -1.0)]
        }
        SymmetryMode::Radial => {
            let n = sym.radial_count.clamp(2, 24);
            (1..n)
                .map(|k| {
                    let angle = std::f64::consts::TAU * k as f64 / n as f64;
                    Affine::translate(c) * Affine::rotate(angle) * Affine::translate(-c)
                })
                .collect()
        }
    }
}

/// This stroke's painted pixels as a shape, with its canvas registered in the
/// document's image library. See [`Editor::paint_raster`] for why a stroke is
/// its own bitmap rather than a corner of a layer-sized one.
fn raster_shape(
    scene: &mut Scene,
    canvas: &buzz_scene::Canvas,
    brush: &buzz_scene::SoftBrush,
    blend: buzz_scene::PaintBlend,
) -> ShapeData {
    let id = scene.next_image_id();
    let name = scene.images().unique_name("Paint");
    let asset = scene.images_mut().insert(canvas.to_asset(id, name, brush));
    let area = canvas.area();
    let mut fill = buzz_scene::ImageFill::new(asset, area);
    // The canvas is already at the document's own pixel scale, so it is drawn
    // one painted pixel to one document unit. Smoothing it would blur paint
    // against the grid it was painted on.
    fill.smooth = false;
    ShapeData {
        path: buzz_geom::Shape::to_path(&area, 1e-9),
        fill: Some(buzz_scene::FillSpec::image(fill)),
        stroke: None,
        blend,
    }
}

/// Lay a finished set of brush shapes down as **one thing**.
///
/// A single shape stays a shape; several become a group, so a stroke that
/// happens to be built from twenty pieces is still one click to select and one
/// step to undo. Shared by the still and the animated commit, and by the
/// symmetry copies of both, which is what keeps a mirrored effect stroke
/// grouped the same way the original is.
fn place_artwork(
    scene: &mut Scene,
    layer: LayerId,
    frame: u32,
    shapes: Vec<ShapeData>,
) -> Option<ObjectId> {
    if shapes.is_empty() {
        return None;
    }
    if shapes.len() == 1 {
        let shape = shapes.into_iter().next().expect("checked length");
        return scene.add_shape_at(layer, frame, shape);
    }
    let children: Vec<Arc<Object>> = shapes
        .into_iter()
        .map(|shape| {
            let id = scene.next_object_id();
            Arc::new(Object::shape(id, shape))
        })
        .collect();
    let id = scene.next_object_id();
    scene.add_object_at(layer, frame, Object::group(id, children))
}

/// One symmetry copy of a shape: its path and any gradient/image paint carried
/// through the mirror transform, so the copy is a true reflection, fill and all.
fn mirror_shape(shape: &ShapeData, t: Affine) -> ShapeData {
    let mut copy = shape.clone();
    copy.path.apply_affine(t);
    if let Some(fill) = &mut copy.fill {
        fill.paint = fill.paint.transformed(t);
    }
    if let Some(stroke) = &mut copy.stroke {
        stroke.paint = stroke.paint.transformed(t);
    }
    copy
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
/// Fuse an unfilled, stroked shape — a line — with the lines it crosses.
///
/// # Why concatenation rather than a boolean
///
/// A filled shape fuses by unioning two regions, which is well defined. Two
/// *lines* have no region: a union of two open paths is not a thing, and
/// outlining them into regions to union would turn editable centrelines into
/// closed outlines that can never be a line again. So the paths are simply
/// carried into one object, each keeping its own subpath. That is exactly what
/// "these are one thing now" has to mean here: one object to click, one to
/// drag, one to undo — and every line still its own curve underneath.
///
/// # What counts as touching
///
/// The centrelines are outlined at their own widths and intersected, so two
/// lines fuse when the **ink** meets, not when the bounding boxes do. A page of
/// separate parallel lines has boxes that cross constantly and no ink in
/// common; fusing those would make one object of a whole drawing, which is the
/// failure the filled path documents at length just below.
fn merge_stroke_into_layer(
    scene: &mut Scene,
    layer: LayerId,
    frame: u32,
    incoming: ShapeData,
) -> Option<ObjectId> {
    let Some(new_stroke) = incoming.stroke.clone() else {
        // Neither fill nor stroke: nothing that can fuse, and nothing visible
        // either. Placed as it is, and refused earlier by `can_draw` anyway.
        return scene.add_shape_at(layer, frame, incoming);
    };

    let bb = incoming.path.bounding_box();
    let opts = buzz_geom::BooleanOptions::for_shape_size(bb.width().hypot(bb.height()));
    let ink = |path: &BezPath, width: f64| {
        buzz_geom::outline_stroke(
            path,
            buzz_geom::StrokeStyle::new(width.max(0.01)),
            (width / 40.0).max(1e-4),
        )
    };
    let same_paint = |a: &Paint, b: &Paint| match (a, b) {
        (Paint::Solid(a), Paint::Solid(b)) => {
            a.to_rgba8().to_u8_array() == b.to_rgba8().to_u8_array()
        }
        (Paint::Gradient(a), Paint::Gradient(b)) => a == b,
        _ => false,
    };

    // Lines already on the frame that share this one's ink and could touch it.
    let candidates: Vec<(ObjectId, BezPath)> = scene
        .layers()
        .get(layer)
        .map(|l| {
            l.objects_at(frame)
                .iter()
                .filter(|o| o.visible && !o.locked)
                .filter_map(|o| match &o.kind {
                    // Only an unfilled line fuses with an unfilled line. A
                    // stroked *fill* is a shape with an outline: fusing its
                    // centreline into a line would throw its fill away.
                    ObjectKind::Shape(s) if s.fill.is_none() => {
                        let stroke = s.stroke.as_ref()?;
                        (same_paint(&stroke.paint, &new_stroke.paint)
                            && (stroke.width - new_stroke.width).abs() < 1e-9
                            && stroke.hairline == new_stroke.hairline)
                            .then(|| (o.id, s.path.clone()))
                    }
                    _ => None,
                })
                .filter(|(_, path)| path.bounding_box().overlaps(bb))
                .collect()
        })
        .unwrap_or_default();

    let mut merged = incoming.path.clone();
    let mut absorbed = Vec::new();
    let width = if new_stroke.hairline {
        // A hairline is one screen pixel however far in you are, so it has no
        // document width to outline. Its own length gives the scale at which
        // "these meet" is a sensible question.
        (bb.width().hypot(bb.height()) * 1e-3).max(0.05)
    } else {
        new_stroke.width
    };

    for (id, path) in candidates {
        // Measured against everything fused so far, so a chain of three
        // touching lines comes in as one even though the first and last never
        // meet — the same rule the filled path follows.
        let touches = buzz_geom::boolean(
            &ink(&merged, width),
            &ink(&path, width),
            buzz_geom::BoolOp::Intersect,
            opts.fast(),
        )
        .area()
        .abs()
            > 1e-9;
        if !touches {
            continue;
        }
        merged.extend(path.iter());
        absorbed.push(id);
    }

    for id in absorbed {
        scene.remove_object(id);
    }

    scene.add_shape_at(
        layer,
        frame,
        ShapeData {
            path: merged,
            fill: None,
            stroke: Some(new_stroke),
            blend: incoming.blend,
        },
    )
}

fn merge_shape_into_layer(
    scene: &mut Scene,
    layer: LayerId,
    frame: u32,
    incoming: ShapeData,
) -> Option<ObjectId> {
    let Some(new_fill) = incoming.fill.clone() else {
        // **A line has no fill, and lines fuse too.**
        //
        // Merge Shape used to send anything without a fill straight through as
        // its own object, so two pencil lines drawn across each other stayed
        // two objects: clicking the join selected one of them and dragging it
        // pulled it out of the drawing it plainly belonged to. In Merge Shape
        // what touches is one thing, and that has to hold for the strokes as
        // well as for the paint.
        return merge_stroke_into_layer(scene, layer, frame, incoming);
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
        // **The same ramp, wherever it was laid.** Two shapes drawn with one
        // gradient setting never carry the same gradient: a new fill is fitted
        // to the shape's own bounding box, so every stroke gets a different
        // transform. Comparing the whole gradient therefore answered "these
        // are different paints" for two strokes the user drew with the same
        // swatch — and different paints *cut*, so the second stroke took a
        // bite out of the first instead of joining it. Which is worse the
        // larger the brush, because the bite is bigger.
        //
        // The ramp is what "the same paint" means here; where it was placed is
        // a consequence of the shape, and the merged shape gets its own
        // placement below.
        (Paint::Gradient(a), Paint::Gradient(b)) => {
            a.kind == b.kind
                && a.spread == b.spread
                && (a.focal - b.focal).abs() < 1e-9
                && a.stops().len() == b.stops().len()
                && a.stops().iter().zip(b.stops()).all(|(x, y)| {
                    (x.offset - y.offset).abs() < 1e-9 && same_color(x.color, y.color)
                })
        }
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
            // A bounding-box overlap only says the two shapes are *near* each
            // other, not that they touch — two brush strokes can sit side by
            // side with their boxes crossing while their fills never meet. A
            // union does not care: it happily returns one path with two
            // disjoint contours, and that one path is one object from here
            // on. Draw a page full of separate same-coloured strokes and
            // every one of them near enough to another would silently fuse
            // into a single object, so a click on any of them selected the
            // whole cluster — exactly what Merge Shape is not supposed to do.
            // Checked against the fill accumulated so far, not just the
            // incoming stroke, so a chain of three touching shapes still
            // fuses as one even though the first and last never meet.
            let touches = buzz_geom::boolean(&merged, &path, buzz_geom::BoolOp::Intersect, opts.fast())
                .area()
                .abs()
                > 1e-9;
            if !touches {
                continue;
            }
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

    let fused = !absorbed.is_empty();
    for id in absorbed {
        scene.remove_object(id);
    }

    // **One shape, one ramp across it.** A gradient is laid across the shape it
    // fills, and the shape has just changed: keeping the incoming stroke's
    // placement would run the ramp across the width of the last stroke drawn
    // and repeat it over everything it fused with. Refitted only when
    // something actually fused, so a stroke that landed on its own keeps
    // exactly the fill it was drawn with.
    let mut new_fill = new_fill;
    if fused
        && let Paint::Gradient(gradient) = &new_fill.paint
    {
        let mut ramp = (**gradient).clone();
        ramp.fit_to(merged.bounding_box());
        new_fill.paint = Paint::Gradient(std::sync::Arc::new(ramp));
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

/// **Put a freshly imported sound on the document's own timeline.**
///
/// # Why importing places it
///
/// A sound in the library and nowhere else is silent, and \u2014 the report this
/// exists to answer \u2014 it is *not exported*, because what the exporter writes
/// is `Scene::stage_cues`: the sounds attached to keyframes on the document's
/// timeline. An animator who imports a dialogue track, hears nothing on
/// playback and finds no audio in the finished MP4 has every reason to conclude
/// that sound does not work. Animate leaves it in the library too, and Animate
/// is wrong about this in a way that costs a beginner an afternoon.
///
/// So the sound arrives on a layer of its own, named after itself, running from
/// the first frame and spanning its own length. Everything about that is
/// ordinary editing and one Ctrl+Z undoes the lot, including the import.
///
/// # On the stage timeline, whatever is open
///
/// Deliberately `edit_stage_layers` and not `edit_layers`. What plays and what
/// exports is the document's own timeline; a sound put inside whichever symbol
/// happened to be open for editing would be neither heard nor written, which is
/// the same silence by a different route.
///
/// `false` when there was nothing to place \u2014 a sound of no length.
fn place_sound_on_stage(scene: &mut buzz_scene::Scene, sound: buzz_scene::SoundId, fps: f64) -> bool {
    let frames = scene
        .sounds()
        .get(sound)
        .map(|asset| asset.duration_frames(fps))
        .unwrap_or(0);
    if frames == 0 {
        return false;
    }
    let name = scene
        .sounds()
        .get(sound)
        .map(|asset| asset.name.clone())
        .unwrap_or_else(|| "Sound".to_string());

    let id = scene.add_stage_layer(name, buzz_scene::LayerKind::Normal);
    scene.update_stage_layer(id, |layer| {
        // The span the sound covers, so the timeline shows how long it runs,
        // the waveform has somewhere to be drawn, and the export range \u2014 which
        // defaults to the length of the timeline \u2014 reaches the end of it.
        layer.frames.insert_frame(frames.saturating_sub(1));
        // Stream, which is what dialogue is: tied to the playhead, so scrubbing
        // moves the sound and the picture cannot drift from it.
        if let Some(keyframe) = layer.frames.keyframe_at_mut(0) {
            keyframe.sound = Some(buzz_scene::SoundRef::stream(sound));
        }
    });
    true
}

/// The number a file name ends with, for putting a sequence in its own order.
///
/// `frame2` before `frame10`, which sorting by name gets exactly backwards.
/// `None` for a name with no trailing number, which then sorts by name after
/// every numbered file.
fn trailing_number(name: &str) -> Option<u64> {
    let digits: String = name
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// **A point inside a filled region**, to seed the next frame's bucket from.
///
/// The centre of the box first, which is inside almost every shape anybody
/// draws. Where it is not — a crescent, a horseshoe, anything that wraps around
/// its own middle — the box is sampled on a small grid and the first point that
/// is really inside wins. `None` for a shape with no inside worth the name,
/// which is a hairline or an empty path.
fn interior_point(path: &buzz_geom::BezPath) -> Option<Point> {
    use buzz_geom::Shape as _;
    let box_ = path.bounding_box();
    if !(box_.width() > 0.0) || !(box_.height() > 0.0) {
        return None;
    }
    let rule = buzz_scene::bucket::FILL_RULE;

    let centre = box_.center();
    if buzz_geom::fill_contains(path, centre, rule) {
        return Some(centre);
    }

    // Odd fractions, so a sample never lands exactly on the axis of a shape
    // that is symmetric about its own middle — which is where a crescent's
    // hole is, and would fail every time.
    const STEPS: [f64; 5] = [0.3, 0.7, 0.5, 0.2, 0.8];
    for fy in STEPS {
        for fx in STEPS {
            let point = Point::new(
                box_.x0 + box_.width() * fx,
                box_.y0 + box_.height() * fy,
            );
            if buzz_geom::fill_contains(path, point, rule) {
                return Some(point);
            }
        }
    }
    None
}

/// Are these the same colour, as far as a person looking at a drawing is
/// concerned?
///
/// Compared as bytes rather than as floats: a colour that has been through a
/// round trip is not bit-identical to the one that was picked, and a Select
/// Same Colour that missed half the coat because of a rounding error would be
/// worse than not having it.
fn same_colour(a: Color, b: Color) -> bool {
    a.to_rgba8().to_u8_array() == b.to_rgba8().to_u8_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Vec2;
    use kurbo::Rect as KRect;

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        KRect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    #[test]
    fn a_reference_layer_is_a_guide_layer() {
        let mut e = editor();
        e.run(Command::NewReferenceLayer);
        let active = e.selection.active_layer().expect("the reference layer is active");
        let kind = e.doc.scene().layers().get(active).expect("the layer").kind;
        assert_eq!(kind, buzz_scene::LayerKind::Guide, "a reference layer is a guide layer");
    }

    #[test]
    fn symmetry_makes_the_right_number_of_copies() {
        let size = buzz_geom::Size::new(400.0, 300.0);
        let sym = |mode, radial| SymmetrySettings { mode, radial_count: radial };
        assert_eq!(symmetry_transforms(sym(SymmetryMode::Off, 6), size).len(), 0);
        assert_eq!(symmetry_transforms(sym(SymmetryMode::MirrorX, 6), size).len(), 1);
        assert_eq!(symmetry_transforms(sym(SymmetryMode::Both, 6), size).len(), 3);
        assert_eq!(symmetry_transforms(sym(SymmetryMode::Radial, 6), size).len(), 5);
        // Radial count is clamped into a sane range.
        assert_eq!(symmetry_transforms(sym(SymmetryMode::Radial, 100), size).len(), 23);
    }

    #[test]
    fn mirror_x_reflects_across_the_stage_centre() {
        let size = buzz_geom::Size::new(400.0, 300.0);
        let t = symmetry_transforms(
            SymmetrySettings { mode: SymmetryMode::MirrorX, radial_count: 6 },
            size,
        )[0];
        // A point 30 left of centre lands 30 right of it; the row is unchanged.
        let p = t * Point::new(170.0, 120.0); // centre x = 200
        assert!((p.x - 230.0).abs() < 1e-9, "x should mirror to 230, got {}", p.x);
        assert!((p.y - 120.0).abs() < 1e-9, "y should be unchanged, got {}", p.y);
    }

    /// Mirroring is a property of *drawing*, not of one tool. The vector
    /// brush honoured it because it commits through `add_shape`; the soft
    /// brush, the effect brushes and the wave brush each commit their own way
    /// and quietly drew one copy.
    #[test]
    fn every_brush_kind_mirrors_its_stroke() {
        for kind in [
            buzz_ui::BrushKind::Fluid,
            buzz_ui::BrushKind::Raster,
            buzz_ui::BrushKind::Effect,
        ] {
            let mut e = editor();
            e.style.drawing_mode = DrawingMode::ObjectDrawing;
            e.style.brush.kind = kind;
            e.set_tool(ToolId::Brush);

            let count = |e: &Editor| {
                e.scene()
                    .layers()
                    .iter()
                    .map(|l| l.objects_at(0).len())
                    .sum::<usize>()
            };

            // The same stroke twice: once plain, once mirrored left to right.
            let drag = |e: &mut Editor| {
                let points: Vec<Point> = (0..12)
                    .map(|i| Point::new(60.0 + f64::from(i) * 8.0, 120.0))
                    .collect();
                e.pointer_down(points[0], Mods::default());
                for p in &points[1..] {
                    e.pointer_move(*p, Mods::default());
                }
                e.pointer_up(*points.last().expect("points"));
            };

            drag(&mut e);
            let plain = count(&e);
            assert!(plain > 0, "{kind:?} drew nothing at all");

            e.style.symmetry.mode = SymmetryMode::MirrorX;
            drag(&mut e);
            let mirrored = count(&e) - plain;

            assert_eq!(
                mirrored,
                plain * 2,
                "{kind:?} under a left-right mirror should lay down twice what it \
                 lays down plain, got {mirrored} against {plain}"
            );
        }
    }

    /// And the copy is really on the other side, rather than a second stroke
    /// stacked on the first.
    #[test]
    fn a_mirrored_brush_stroke_lands_on_the_far_side() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.style.brush.kind = buzz_ui::BrushKind::Fluid;
        e.style.symmetry.mode = SymmetryMode::MirrorX;
        e.set_tool(ToolId::Brush);

        let centre = e.scene().stage().size.width / 2.0;
        let points: Vec<Point> = (0..12)
            .map(|i| Point::new(20.0 + f64::from(i) * 6.0, 120.0))
            .collect();
        assert!(
            points.iter().all(|p| p.x < centre),
            "the test stroke has to start on one side to have a far side"
        );
        e.pointer_down(points[0], Mods::default());
        for p in &points[1..] {
            e.pointer_move(*p, Mods::default());
        }
        e.pointer_up(*points.last().expect("points"));

        let mut left = 0;
        let mut right = 0;
        for layer in e.scene().layers().iter() {
            for object in layer.objects_at(0).iter() {
                let Some(quad) = e.object_quad(object.id) else {
                    continue;
                };
                let x = quad.iter().map(|p| p.x).sum::<f64>() / 4.0;
                if x < centre {
                    left += 1;
                } else {
                    right += 1;
                }
            }
        }
        assert_eq!((left, right), (1, 1), "one stroke each side of the mirror");
    }

    /// The mirror is drawn through as well as drawn with. Rubbing out on one
    /// side rubs out on the other, or the far half of a symmetric drawing
    /// becomes read-only the moment you make a mistake on it.
    #[test]
    fn the_eraser_rubs_through_the_mirror() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        // Two bars, one each side of the vertical centre line, mirror images.
        let centre = e.scene().stage().size.width / 2.0;
        for x in [centre - 90.0, centre + 50.0] {
            e.apply(ToolAction::AddShape {
                shape: ShapeData::filled(square(x, 100.0, 40.0), Color::BLACK),
                label: "Draw",
            });
        }
        let before = e
            .scene()
            .layers()
            .iter()
            .map(|l| l.objects_at(0).len())
            .sum::<usize>();
        assert_eq!(before, 2, "two bars to rub at");

        // A narrow rub straight down through the middle of the left bar only,
        // under a left-right mirror.
        e.style.symmetry.mode = SymmetryMode::MirrorX;
        let mut path = BezPath::new();
        path.move_to(Point::new(centre - 70.0, 90.0));
        path.line_to(Point::new(centre - 70.0, 150.0));
        e.apply(ToolAction::Erase { path, width: 8.0 });

        // Both bars are cut in two: four pieces where there were two shapes.
        let after = e
            .scene()
            .layers()
            .iter()
            .map(|l| l.objects_at(0).len())
            .sum::<usize>();
        assert_eq!(
            after, 4,
            "the rub should have cut both bars, not only the one under the pointer"
        );
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

    /// An effect stroke is many pieces — vector shapes and painted bitmaps —
    /// but it lands as **one** object, one undo step, selected. The gesture
    /// after "paint snow" is "move the snow", and that only works if the
    /// stroke came in whole.
    #[test]
    fn an_effect_stroke_commits_as_one_grouped_undoable_object() {
        let mut e = editor();
        let samples: Vec<buzz_geom::StrokeSample> = (0..60)
            .map(|i| {
                let t = i as f64 / 59.0;
                buzz_geom::StrokeSample::new(Point::new(t * 400.0, 200.0), t)
            })
            .collect();
        let pieces = buzz_scene::effect_artwork(
            buzz_scene::EffectKind::Snow,
            &buzz_scene::EffectStroke {
                samples: &samples,
                size: 24.0,
                color: Color::WHITE,
                conditioning: buzz_geom::Conditioning::smoothing(0.5),
            },
        );
        assert!(pieces.len() > 1, "snow should be several depth buckets");

        e.apply(ToolAction::AddArtwork {
            pieces,
            label: "Snow",
        });

        // One object on the layer: the group.
        let objects: Vec<ObjectId> = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter().map(|o| o.id))
            .collect();
        assert_eq!(objects.len(), 1, "the pieces should arrive grouped");
        assert_eq!(
            e.selection.ids(),
            objects,
            "the stroke should come in selected, like any other drawing"
        );

        // And leave as one undo step.
        assert!(e.doc.undo());
        assert_eq!(e.scene().shape_count(), 0, "undo should remove the whole stroke");
    }

    /// A wave stroke, as the Wave brush hands one over.
    fn wave_frames(kind: buzz_scene::WaveKind, frames: u32) -> Vec<Vec<buzz_scene::ArtPiece>> {
        let samples: Vec<buzz_geom::StrokeSample> = (0..60)
            .map(|i| {
                let t = i as f64 / 59.0;
                buzz_geom::StrokeSample::new(Point::new(200.0, 400.0 - t * 300.0), t)
            })
            .collect();
        let mut settings = kind.preset();
        settings.frames = frames;
        buzz_scene::wave_loop(
            kind,
            &buzz_scene::WaveStroke {
                samples: &samples,
                size: 24.0,
                color: Color::WHITE,
                conditioning: buzz_geom::Conditioning::smoothing(0.5),
                settings,
            },
        )
    }

    /// **One stroke, one animation.** A wave commits a whole cycle: a keyframe
    /// per frame, each holding that frame's drawing, and all of it one undo
    /// step.
    #[test]
    fn a_wave_stroke_bakes_a_keyframe_for_every_frame() {
        let mut e = editor();
        let frames = wave_frames(buzz_scene::WaveKind::Smoke, 8);
        assert_eq!(frames.len(), 8);

        e.apply(ToolAction::AddArtworkFrames {
            frames,
            label: "Smoke",
        });

        let layer = e.active_layer().expect("a layer");
        let timeline = &e.scene().layers().get(layer).expect("the layer").frames;
        assert_eq!(timeline.keyframe_count(), 8, "one keyframe per frame");
        assert!(timeline.length() >= 8, "the layer should reach the last frame");

        for frame in 0..8u32 {
            assert_eq!(
                timeline.objects_at(frame).len(),
                1,
                "frame {frame} should hold exactly one drawing"
            );
        }

        // And it leaves as one undo step, like every other stroke.
        assert!(e.doc.undo());
        let after = &e.scene().layers().get(layer).expect("the layer").frames;
        assert_eq!(after.keyframe_count(), 1, "undo should take the whole cycle");
    }

    /// The bug this order was chosen to avoid: every keyframe is made before
    /// any of the artwork, so frame *n* holds one plume rather than *n* of
    /// them.
    #[test]
    fn a_baked_wave_does_not_stack_up_frame_by_frame() {
        let mut e = editor();
        e.apply(ToolAction::AddArtworkFrames {
            frames: wave_frames(buzz_scene::WaveKind::Smoke, 6),
            label: "Smoke",
        });

        let layer = e.active_layer().expect("a layer");
        let timeline = &e.scene().layers().get(layer).expect("the layer").frames;
        for frame in 0..6u32 {
            assert_eq!(
                timeline.objects_at(frame).len(),
                1,
                "frame {frame} carries copies of the frames before it"
            );
        }
    }

    /// Baking a wave over a held drawing must not blank the frames it covers:
    /// the keyframes are inserted the way F6 inserts one, carrying what the
    /// layer was already showing.
    #[test]
    fn baking_a_wave_keeps_the_background_it_was_drawn_over() {
        let mut e = editor();
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 500.0), Color::from_rgb8(0x20, 0x30, 0x40)),
            label: "Background",
        });
        let layer = e.active_layer().expect("a layer");
        // Held for a while, as a background is.
        e.doc.edit("Extend", |scene| {
            scene.update_layer(layer, |l| {
                l.frames.insert_frame(11);
            });
        });

        e.apply(ToolAction::AddArtworkFrames {
            frames: wave_frames(buzz_scene::WaveKind::Smoke, 6),
            label: "Smoke",
        });

        let timeline = &e.scene().layers().get(layer).expect("the layer").frames;
        for frame in 0..6u32 {
            assert_eq!(
                timeline.objects_at(frame).len(),
                2,
                "frame {frame} lost the background it was drawn over"
            );
        }
    }

    /// **The eraser cuts a shape in two.** Rubbing through the middle left one
    /// object holding two disconnected halves, so clicking either selected
    /// both and dragging one dragged the other — which is the opposite of what
    /// an eraser is for.
    #[test]
    fn rubbing_through_a_shape_leaves_two_shapes() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.style.stroke_enabled = false;
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 200.0), Color::WHITE),
            label: "Draw",
        });
        assert_eq!(e.scene().shape_count_at(0), 1);

        // A rub straight down the middle, wide enough to part it.
        e.style.eraser_size = 20.0;
        e.apply(ToolAction::Erase {
            path: {
                let mut path = buzz_geom::BezPath::new();
                path.move_to(Point::new(100.0, -40.0));
                path.line_to(Point::new(100.0, 240.0));
                path
            },
            width: e.style.eraser_size,
        });

        assert_eq!(
            e.scene().shape_count_at(0),
            2,
            "a shape rubbed through the middle becomes two shapes"
        );
        // Each half is on its own side of the cut.
        let mut middles: Vec<f64> = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .map(|o| o.bounds().center().x)
            .collect();
        middles.sort_by(f64::total_cmp);
        assert!(
            middles[0] < 100.0 && middles[1] > 100.0,
            "the halves should sit either side of the rub: {middles:?}"
        );
    }

    /// A rub that only takes a bite leaves one shape — splitting must not
    /// invent pieces that are still joined.
    #[test]
    fn a_rub_that_does_not_part_a_shape_leaves_one() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.style.stroke_enabled = false;
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 200.0), Color::WHITE),
            label: "Draw",
        });
        e.apply(ToolAction::Erase {
            path: {
                let mut path = buzz_geom::BezPath::new();
                path.move_to(Point::new(100.0, -40.0));
                path.line_to(Point::new(100.0, 60.0));
                path
            },
            width: 20.0,
        });
        assert_eq!(e.scene().shape_count_at(0), 1, "a notch is still one shape");
    }

    /// **Two strokes drawn with one gradient fuse.** A new fill is fitted to
    /// its own shape's bounds, so every stroke carried a different gradient
    /// transform — and "different paint" *cuts* in Merge Shape, so the second
    /// stroke took a bite out of the first instead of joining it.
    #[test]
    fn strokes_sharing_a_gradient_merge_rather_than_cutting() {
        let mut e = editor();
        e.style.stroke_enabled = false;
        let ramp = |bounds: buzz_geom::Rect| {
            buzz_scene::Gradient::linear(Color::BLACK, Color::WHITE, bounds)
        };

        let first = square(0.0, 0.0, 100.0);
        e.apply(ToolAction::AddShape {
            shape: ShapeData {
                path: first.clone(),
                fill: Some(FillSpec::gradient(ramp(buzz_geom::Shape::bounding_box(
                    &first,
                )))),
                stroke: None,
                blend: buzz_scene::PaintBlend::Normal,
            },
            label: "Draw",
        });
        let second = square(50.0, 0.0, 100.0);
        e.apply(ToolAction::AddShape {
            shape: ShapeData {
                path: second.clone(),
                fill: Some(FillSpec::gradient(ramp(buzz_geom::Shape::bounding_box(
                    &second,
                )))),
                stroke: None,
                blend: buzz_scene::PaintBlend::Normal,
            },
            label: "Draw",
        });

        assert_eq!(
            e.scene().shape_count_at(0),
            1,
            "two overlapping strokes of the same gradient are one shape"
        );
        let fused = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .next()
            .expect("the fused shape")
            .bounds();
        assert!(
            fused.width() > 140.0,
            "and it spans both strokes rather than one biting the other: {fused:?}"
        );
    }

    /// **The scene commands are reachable from the menu.** The only way to
    /// make a scene used to be a menu on the breadcrumb above the stage, and
    /// that strip was drawn only while a symbol was open — so on the main
    /// timeline, where everybody works, a document's scenes could not be
    /// reached at all.
    #[test]
    fn scenes_can_be_added_and_duplicated_from_a_command() {
        let mut e = editor();
        assert_eq!(e.doc.scene_names().len(), 1);

        e.run(buzz_ui::Command::AddScene);
        assert_eq!(e.doc.scene_names().len(), 2, "Add Scene makes one");
        assert_eq!(e.doc.active_scene(), 1, "and opens it");

        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 20.0), Color::WHITE),
            label: "Draw",
        });
        e.run(buzz_ui::Command::DuplicateScene);
        assert_eq!(e.doc.scene_names().len(), 3);
        assert_eq!(
            e.scene().shape_count_at(0),
            1,
            "the duplicate carries the artwork with it"
        );
        assert!(
            e.status.as_deref().unwrap_or_default().contains("Duplicated"),
            "and says what it did: {:?}",
            e.status
        );
    }

    /// **The length the export dialog offers is the length the exporter
    /// renders.** Two crates count the film — `Document` for the range the
    /// user is given, and the exporter's reel for what is actually written —
    /// and if they ever disagreed the default range would quietly stop short
    /// of the end of the film or ask for frames that do not exist.
    #[test]
    fn the_films_length_agrees_with_what_the_exporter_will_render() {
        let mut e = editor();

        // Three scenes of different lengths, one of them looping.
        let lengths = [4u32, 7, 2];
        for (i, length) in lengths.iter().enumerate() {
            if i > 0 {
                e.doc.add_scene();
            }
            let last = length.saturating_sub(1);
            e.doc.edit("Lengthen", |scene| {
                let layer = scene.add_layer("Art", buzz_scene::LayerKind::Normal);
                scene.update_layer(layer, |l| {
                    l.frames.insert_frame(last);
                });
            });
        }
        e.doc.switch_scene(1);
        e.doc.edit("Loop", |scene| {
            *scene.looping_mut() = buzz_scene::LoopRegion {
                enabled: true,
                start: 1,
                end: 3,
                repeats: 3,
            };
        });

        let scenes = e.doc.film();
        assert_eq!(scenes.len(), 3);
        let reel = buzz_export::Reel::of(scenes.iter());

        assert_eq!(
            e.doc.film_frames(),
            reel.frames(),
            "the dialog and the exporter disagree about how long the film is"
        );
        for (index, (_, start)) in reel.scenes().enumerate() {
            assert_eq!(
                e.doc.film_start_of(index),
                start,
                "scene {index} starts in a different place for each of them"
            );
        }
        // And the loop really lengthened the film, so this is not a test of
        // three plain scenes.
        assert!(
            reel.frames() > lengths.iter().sum::<u32>(),
            "the looping scene should make the film longer than its timelines"
        );
    }

    /// Drag a freehand stroke through the tool, at an optional pen pressure.
    fn draw_freehand(e: &mut Editor, tool: ToolId, from: Point, to: Point, pressure: Option<f64>) {
        e.set_tool(tool);
        e.pointer_down_at(from, Mods::default(), Some(0.0), pressure);
        for i in 1..=12 {
            let t = f64::from(i) / 12.0;
            e.pointer_move_at(from.lerp(to, t), Mods::default(), Some(t * 0.4), pressure);
        }
        e.pointer_up_at(to, Some(0.4), pressure);
    }

    /// **Crossing lines are one thing.** In Merge Shape mode two pencil lines
    /// drawn across each other used to stay two objects, so clicking the join
    /// selected one of them and dragging pulled it out of the drawing it
    /// belonged to.
    #[test]
    fn lines_that_cross_fuse_into_one_object() {
        let mut e = editor();
        assert_eq!(e.style.drawing_mode, DrawingMode::MergeShape);

        draw_freehand(
            &mut e,
            ToolId::Pencil,
            Point::new(100.0, 100.0),
            Point::new(300.0, 100.0),
            None,
        );
        assert_eq!(e.scene().shape_count_at(0), 1, "the first line is drawn");

        draw_freehand(
            &mut e,
            ToolId::Pencil,
            Point::new(200.0, 20.0),
            Point::new(200.0, 200.0),
            None,
        );
        assert_eq!(
            e.scene().shape_count_at(0),
            1,
            "a line drawn across another should fuse with it, not sit beside it"
        );

        // And the one object really holds both arms, so selecting it takes
        // the whole cross and dragging moves it together.
        let fused = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .next()
            .expect("the fused line")
            .bounds();
        assert!(
            fused.width() > 100.0 && fused.height() > 50.0,
            "the fused object should span both arms of the cross: {fused:?}"
        );
    }

    /// Lines that never touch stay separate, or a page of parallel strokes
    /// would silently become one object — the same failure the filled merge
    /// guards against.
    #[test]
    fn lines_that_never_touch_stay_separate() {
        let mut e = editor();
        for y in [100.0, 160.0] {
            draw_freehand(
                &mut e,
                ToolId::Pencil,
                Point::new(100.0, y),
                Point::new(300.0, y),
                None,
            );
        }
        assert_eq!(
            e.scene().shape_count_at(0),
            2,
            "parallel lines share bounding boxes but no ink"
        );
    }

    /// Object Drawing means every stroke is its own object, crossing or not —
    /// the whole point of the mode.
    #[test]
    fn object_drawing_keeps_crossing_lines_apart() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_freehand(
            &mut e,
            ToolId::Pencil,
            Point::new(100.0, 100.0),
            Point::new(300.0, 100.0),
            None,
        );
        draw_freehand(
            &mut e,
            ToolId::Pencil,
            Point::new(200.0, 20.0),
            Point::new(200.0, 200.0),
            None,
        );
        assert_eq!(e.scene().shape_count_at(0), 2);
    }

    /// **Pen pressure reaches the artwork.** The window reports a force per
    /// sample and it was thrown away, so every stroke was recorded at full
    /// pressure and the Pressure setting appeared to do nothing at all.
    #[test]
    fn a_pens_pressure_reaches_the_stroke() {
        let width_at = |pressure: Option<f64>| -> f64 {
            let mut e = editor();
            e.style.brush.kind = buzz_ui::BrushKind::Fluid;
            e.style.brush.use_pressure = true;
            e.style.brush.size = 40.0;
            e.style.brush.min_ratio = 0.05;
            e.style.brush.taper = 0.0;
            e.style.brush.smoothing = 0.0;
            draw_freehand(
                &mut e,
                ToolId::Brush,
                Point::new(100.0, 200.0),
                Point::new(400.0, 200.0),
                pressure,
            );
            e.scene()
                .layers()
                .iter()
                .flat_map(|l| l.objects_at(0).iter())
                .map(|o| o.bounds().height())
                .fold(0.0, f64::max)
        };

        // Both well under full pressure, as a real pen stroke is: a stroke
        // that never once came in under maximum is treated as a device with
        // no sensor — see `brush_profile_for`, and the test below.
        let light = width_at(Some(0.15));
        let heavy = width_at(Some(0.9));
        assert!(light > 0.0 && heavy > 0.0, "both strokes drew something");
        assert!(
            heavy > light * 2.0,
            "a hard press should paint far wider than a light one: {heavy:.2} against {light:.2}"
        );
    }

    /// **A mouse still gets a fluid stroke.** With Pressure on and no sensor
    /// to answer it, every sample is full pressure — so the brush falls back
    /// to speed rather than painting a dead constant width, which is what made
    /// the setting look broken.
    #[test]
    fn a_device_with_no_pressure_falls_back_to_speed() {
        let mut style = DrawStyle::default();
        style.brush.kind = buzz_ui::BrushKind::Fluid;
        style.brush.use_pressure = true;

        let mouse: Vec<buzz_geom::StrokeSample> = (0..40)
            .map(|i| {
                let t = f64::from(i) / 39.0;
                buzz_geom::StrokeSample::new(Point::new(t * 300.0, 0.0), t * 0.5)
            })
            .collect();
        assert!(
            matches!(
                crate::tools::brush_profile_for(&mouse, &style.brush).response,
                buzz_geom::WidthResponse::Speed { .. }
            ),
            "a stroke that never came in under full pressure is not a pen"
        );

        // And a pen's stroke keeps the pressure response.
        let mut pen = mouse.clone();
        pen[10].pressure = 0.4;
        assert_eq!(
            crate::tools::brush_profile_for(&pen, &style.brush).response,
            buzz_geom::WidthResponse::Pressure
        );
    }

    /// **Copy Frames takes every layer.** A frame of a drawing is a frame of
    /// the whole drawing — character, background and overlay — and copying one
    /// layer of it was never what anybody meant.
    #[test]
    fn copying_a_frame_takes_every_layer_and_pastes_them_all_back() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        // Three layers, each with its own drawing on frame 0.
        let mut layers = vec![e.selection.active_layer().expect("a first layer")];
        for name in ["Middle", "Front"] {
            let id = e.doc_add_layer(name, buzz_scene::LayerKind::Normal);
            e.selection.set_active_layer(Some(id));
            layers.push(id);
        }
        for (i, layer) in layers.iter().enumerate() {
            e.selection.set_active_layer(Some(*layer));
            e.apply(ToolAction::AddShape {
                shape: ShapeData::filled(square(i as f64 * 50.0, 0.0, 30.0), Color::WHITE),
                label: "Draw",
            });
        }
        let on_frame = |e: &Editor, frame: u32, layer: LayerId| -> usize {
            e.scene()
                .layers()
                .get(layer)
                .map(|l| l.objects_at(frame).len())
                .unwrap_or(0)
        };
        for layer in &layers {
            assert_eq!(on_frame(&e, 0, *layer), 1, "each layer drew something");
        }

        e.run(buzz_ui::Command::CopyFrames);
        e.set_frame(10);
        e.run(buzz_ui::Command::PasteFrames);

        // Every layer got its own artwork back, on the layer it came from.
        for layer in &layers {
            assert_eq!(
                on_frame(&e, 10, *layer),
                1,
                "a layer's artwork did not come back onto its own layer"
            );
        }
        // And the originals are untouched.
        for layer in &layers {
            assert_eq!(on_frame(&e, 0, *layer), 1);
        }
    }

    /// Cut clears every layer it took from, or most of the drawing stays put.
    #[test]
    fn cutting_a_frame_clears_every_layer_it_took_from() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let first = e.selection.active_layer().expect("a layer");
        let second = e.doc_add_layer("Second", buzz_scene::LayerKind::Normal);
        for layer in [first, second] {
            e.selection.set_active_layer(Some(layer));
            e.apply(ToolAction::AddShape {
                shape: ShapeData::filled(square(0.0, 0.0, 20.0), Color::WHITE),
                label: "Draw",
            });
        }
        assert_eq!(e.scene().shape_count_at(0), 2);

        e.run(buzz_ui::Command::CutFrames);
        assert_eq!(
            e.scene().shape_count_at(0),
            0,
            "a cut should empty the frame on every layer, not just the active one"
        );
        assert!(e.frame_clipboard.is_some(), "and it is on the clipboard");

        e.set_frame(6);
        e.run(buzz_ui::Command::PasteFrames);
        assert_eq!(e.scene().shape_count_at(6), 2, "both layers came back");
    }

    /// Pasting when the layers are gone says so and leaves no empty step in
    /// the history for the user to undo past.
    #[test]
    fn pasting_frames_whose_layers_are_gone_says_so() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let layer = e.doc_add_layer("Doomed", buzz_scene::LayerKind::Normal);
        e.selection.set_active_layer(Some(layer));
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 20.0), Color::WHITE),
            label: "Draw",
        });
        e.run(buzz_ui::Command::CopyFrames);

        // Take every layer the clipboard refers to away.
        let all: Vec<LayerId> = e.scene().layers().iter().map(|l| l.id).collect();
        e.doc.edit("Delete Layers", |scene| {
            for id in all {
                scene.remove_layer(id);
            }
        });

        let before = e.scene().revision();
        e.set_frame(4);
        e.run(buzz_ui::Command::PasteFrames);
        let said = e.status.clone().unwrap_or_default();
        assert!(
            said.contains("gone") || said.contains("locked"),
            "it should say why nothing was pasted, and said {said:?}"
        );
        assert_eq!(
            e.scene().revision(),
            before,
            "and left the document exactly as it was"
        );
    }

    /// **A brush made from artwork keeps that artwork's colour.** The whole
    /// request: select something red, make a brush, and paint red — not a
    /// grey silhouette in whatever the fill swatch happened to be.
    #[test]
    fn a_brush_made_from_a_selection_keeps_its_colour() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let red = Color::from_rgb8(0xFF, 0x00, 0x00);
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(400.0, 300.0, 40.0), red),
            label: "Draw",
        });
        let drawn = e.selection.ids();
        assert_eq!(drawn.len(), 1, "the artwork is selected after drawing it");

        // The fill swatch is deliberately something else entirely, so a brush
        // that painted with the swatch could not pass by accident.
        e.style.fill_color = Color::from_rgb8(0x00, 0xFF, 0x00);
        e.run(buzz_ui::Command::BrushFromSelection);

        let brush = &e.style.brush;
        assert_eq!(brush.pattern, buzz_ui::PatternShape::Custom);
        assert!(brush.kind.uses_pattern(), "and it is a stamping brush");
        assert!(
            brush.stamps_its_own_paint(),
            "it should stamp the artwork's own paint"
        );
        let stamp = brush.pattern_stamp().expect("the captured artwork");
        assert_eq!(
            stamp.place(buzz_geom::Affine::scale(20.0))[0]
                .fill
                .as_ref()
                .expect("a fill")
                .paint
                .color(),
            red,
            "the brush stamps red, not the swatch"
        );
    }

    /// And the stroke it paints really lands in that colour, through the
    /// tool: press, drag, release, and read the artwork back off the layer.
    #[test]
    fn painting_with_a_captured_brush_lays_down_its_colours() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let blue = Color::from_rgb8(0x20, 0x40, 0xE0);
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 30.0), blue),
            label: "Draw",
        });
        e.run(buzz_ui::Command::BrushFromSelection);
        e.style.fill_color = Color::from_rgb8(0xFF, 0xFF, 0x00);
        e.selection.clear();

        let before: Vec<ObjectId> = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .map(|o| o.id)
            .collect();

        // A drag across the stage with the Brush.
        let ctx_points: Vec<Point> = (0..24)
            .map(|i| Point::new(100.0 + f64::from(i) * 12.0, 200.0))
            .collect();
        e.set_tool(ToolId::Brush);
        e.pointer_down(ctx_points[0], Mods::default());
        for p in &ctx_points[1..] {
            e.pointer_move(*p, Mods::default());
        }
        e.pointer_up(*ctx_points.last().unwrap());

        // Whatever arrived, every fill in it is the captured blue.
        let mut painted = Vec::new();
        for layer in e.scene().layers().iter() {
            for object in layer.objects_at(0).iter() {
                if !before.contains(&object.id) {
                    object.flatten(buzz_geom::Affine::IDENTITY, &mut painted);
                }
            }
        }
        assert!(!painted.is_empty(), "the brush painted nothing");
        for (_, shape) in &painted {
            assert_eq!(
                shape.fill.as_ref().expect("a fill").paint.color(),
                blue,
                "a stamp came out in the swatch colour instead of the brush's"
            );
        }
    }

    /// Turning Artwork Colours off is Animate's behaviour, and must still
    /// work: the silhouette, painted by the current swatch.
    #[test]
    fn a_captured_brush_can_still_be_painted_with_the_swatch() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(square(0.0, 0.0, 30.0), Color::from_rgb8(0xFF, 0, 0)),
            label: "Draw",
        });
        e.run(buzz_ui::Command::BrushFromSelection);

        e.style.brush.keep_source_paint = false;
        assert!(
            !e.style.brush.stamps_its_own_paint(),
            "with the switch off it stamps an outline for the swatch to paint"
        );
        assert!(
            e.style.brush.pattern_path().is_some(),
            "and the silhouette is still there to stamp"
        );
    }

    /// A painted effect piece — clouds — lands its pixels in the image
    /// library and its shape on the layer, exactly as a soft-brush stroke
    /// does, so every vector tool works on it afterwards.
    #[test]
    fn a_painted_effect_piece_becomes_an_ordinary_image_fill() {
        let mut e = editor();
        let samples: Vec<buzz_geom::StrokeSample> = (0..40)
            .map(|i| {
                let t = i as f64 / 39.0;
                buzz_geom::StrokeSample::new(Point::new(50.0 + t * 300.0, 100.0), t)
            })
            .collect();
        let pieces = buzz_scene::effect_artwork(
            buzz_scene::EffectKind::Clouds,
            &buzz_scene::EffectStroke {
                samples: &samples,
                size: 30.0,
                color: Color::WHITE,
                conditioning: buzz_geom::Conditioning::smoothing(0.5),
            },
        );
        assert!(
            pieces
                .iter()
                .any(|p| matches!(p, buzz_scene::ArtPiece::Painting { .. })),
            "clouds should carry painted pixels"
        );

        let images_before = e.scene().images().len();
        e.apply(ToolAction::AddArtwork {
            pieces,
            label: "Clouds",
        });
        assert!(
            e.scene().images().len() > images_before,
            "the painting should be in the image library"
        );
        assert!(e.scene().shape_count() >= 1);
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

    /// A bounding box is not the shape. An object with two contours far apart
    /// has a box spanning the gap between them, and a new stroke drawn inside
    /// that gap crosses the box without ever touching the artwork — merging
    /// it anyway would leave a single click selecting both, the same as if
    /// Object Drawing were on.
    #[test]
    fn a_shape_merely_inside_another_shapes_bounding_box_does_not_fuse_with_it() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::MergeShape;
        e.style.fill_color = Color::WHITE;
        e.style.stroke_enabled = false;

        // One object, two corners far apart: its bounding box is [0,0..220,220]
        // but nothing is actually drawn in the middle of it.
        let mut two_corners = square(0.0, 0.0, 20.0);
        two_corners.extend(square(200.0, 200.0, 20.0).iter());
        e.apply(ToolAction::AddShape {
            shape: ShapeData::filled(two_corners, Color::WHITE),
            label: "Draw",
        });
        assert_eq!(e.scene().shape_count(), 1);

        // Drawn well inside that bounding box, nowhere near either corner.
        draw_square(&mut e, 90.0, 90.0, 20.0, Color::WHITE);

        assert_eq!(
            e.scene().shape_count(),
            2,
            "a shape merely inside another's bounding box must not fuse with it"
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

    /// **A symbol instance is marqueed where its artwork is, not where its
    /// registration point is.**
    ///
    /// An instance cannot measure itself — `Object::bounds` gives it a
    /// two-unit placeholder about its own origin — so a marquee tested against
    /// that picked a character up only when the drag crossed the dot its
    /// registration point sits on, and refused it everywhere the character
    /// actually was. On an imported character, whose parts hang a long way off
    /// that point, the effect is a selection offset by most of a limb from
    /// wherever the user dragged.
    #[test]
    fn a_marquee_finds_an_instance_where_its_artwork_is_drawn() {
        let mut e = editor();
        let layer = e.active_layer().expect("a layer");
        e.doc.edit("Build", |scene| {
            let symbol = scene.add_symbol("Character", buzz_scene::SymbolKind::Graphic, None);
            let inner = scene
                .library()
                .get(symbol)
                .and_then(|s| s.layers.iter().next())
                .map(|l| l.id)
                .expect("a layer inside the symbol");
            // Artwork a long way from the symbol's registration point, as a
            // part of an imported character is.
            let id = scene.next_object_id();
            let art = Object::shape(
                id,
                ShapeData::filled(square(200.0, 200.0, 40.0), Color::WHITE),
            );
            scene.library_mut().update(symbol, |s| {
                s.layers.update(inner, |l| {
                    l.frames.set_objects(0, vec![Arc::new(art)]);
                });
            });
            scene.add_instance_at(layer, 0, symbol, Affine::IDENTITY);
        });

        // Round the artwork: selected.
        e.apply(ToolAction::PickInRect {
            rect: Rect::new(150.0, 150.0, 300.0, 300.0),
            additive: false,
        });
        assert_eq!(
            e.selection.len(),
            1,
            "a marquee round the drawing must take it"
        );

        // Round the registration point, with no artwork in it: not selected.
        e.apply(ToolAction::PickInRect {
            rect: Rect::new(-50.0, -50.0, 50.0, 50.0),
            additive: false,
        });
        assert!(
            e.selection.is_empty(),
            "a marquee round empty stage must take nothing"
        );

        // And still "fully enclosed": a marquee that clips the artwork misses.
        e.apply(ToolAction::PickInRect {
            rect: Rect::new(150.0, 150.0, 220.0, 220.0),
            additive: false,
        });
        assert!(
            e.selection.is_empty(),
            "a marquee that only half covers the drawing must not take it"
        );
    }

    /// **A parented layer's selection is measured where its artwork is drawn.**
    ///
    /// Layer parenting moves a child layer's artwork by however far its parent
    /// has travelled since its rest pose. The bounds everything interactive is
    /// built on ignored that, so on a rig they described the limb where it was
    /// drawn at rest: the transform box and handles drew off the artwork, the
    /// transformation point sat beside it, and pressing on the character to
    /// drag it read as "outside the selection" and rubber-banded a marquee
    /// instead of moving it.
    #[test]
    fn a_parented_layer_measures_its_selection_where_it_is_drawn() {
        let mut e = editor();
        let child_layer = e.active_layer().expect("a layer");

        let mut parent_layer = None;
        let mut child_object = None;
        e.doc.edit("Rig", |scene| {
            let parent = scene.add_layer("Parent", buzz_scene::LayerKind::Normal);
            parent_layer = Some(parent);

            // The parent's artwork: at rest on frame 0, moved 120 right by
            // frame 10. `motion_of` reads the difference between the two.
            let rest = scene.next_object_id();
            let rest = Object::shape(rest, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
            let moved = scene.next_object_id();
            let moved = Object::shape(moved, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE))
                .with_transform(Affine::translate((120.0, 0.0)));

            // The child's artwork, and the link that makes it follow.
            let art = scene.next_object_id();
            child_object = Some(art);
            let art = Object::shape(art, ShapeData::filled(square(300.0, 40.0, 20.0), Color::WHITE));

            let layers = scene.edit_layers();
            layers.update(parent, |l| {
                l.frames.set_objects(0, vec![Arc::new(rest)]);
                l.frames.insert_keyframe(10);
                l.frames.set_objects(10, vec![Arc::new(moved)]);
            });
            layers.update(child_layer, |l| {
                l.follows = Some(parent);
                l.frames.set_objects(0, vec![Arc::new(art)]);
            });
        });

        let parent_layer = parent_layer.expect("the parent was added");
        let child_object = child_object.expect("the child artwork was added");
        e.selection.select_one(child_object);

        // On frame 0 the parent is at rest, so nothing has moved.
        e.current_frame = 0;
        let at_rest = e
            .selection
            .bounds_at(e.scene(), 0)
            .expect("the selection has bounds");
        assert!(
            (at_rest.x0 - 300.0).abs() < 1e-9,
            "at rest it sits where it was drawn, got {at_rest:?}"
        );

        // On frame 10 the parent has carried it 120 to the right, and the
        // selection has to say so.
        e.current_frame = 10;
        let carried = e
            .selection
            .bounds_at(e.scene(), 10)
            .expect("the selection has bounds");
        assert!(
            (carried.x0 - 420.0).abs() < 1e-9,
            "the parent carried it 120 right, got {carried:?}"
        );
        assert!(
            (carried.y0 - at_rest.y0).abs() < 1e-9,
            "nothing moved it vertically"
        );

        // And that is the same place the artwork is drawn, which is what makes
        // a press on it read as a press *inside* the selection.
        let follows = e
            .scene()
            .layers()
            .inherited_transform(child_layer, 10);
        assert!(
            (follows.as_coeffs()[4] - 120.0).abs() < 1e-9,
            "the layer really is carried, {follows:?}"
        );
        let _ = parent_layer;
    }

    /// **A live move is still one undo step.** The artwork now travels with the
    /// pointer, which means an edit per pointer move — and if each of those
    /// were its own step, undoing a drag would take fifty presses of Ctrl+Z.
    #[test]
    fn dragging_a_selection_across_the_stage_is_one_undo_step() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 40.0, Color::WHITE).unwrap();
        e.selection.select_one(id);
        let before = e.scene().find_object(id).unwrap().1.bounds();

        // Pressed inside the artwork, walked across the stage a step at a time,
        // released — the gesture the tools actually receive.
        //
        // Pressed at (30, 30) rather than the middle: the transformation point
        // sits at the centre of the selection, and grabbing *that* is a
        // different gesture (see `finish_drag`).
        let camera = e.camera.clone();
        let screen = |p: Point| camera.doc_to_screen(p);
        e.pointer_down(screen(Point::new(30.0, 30.0)), Mods::default());
        for i in 1..=20 {
            let at = Point::new(30.0 + i as f64 * 5.0, 30.0);
            e.pointer_move(screen(at), Mods::default());
        }
        e.pointer_up(screen(Point::new(130.0, 30.0)));

        let after = e.scene().find_object(id).unwrap().1.bounds();
        assert!(
            (after.x0 - before.x0 - 100.0).abs() < 1e-6,
            "it should have travelled the whole drag, {before:?} -> {after:?}"
        );

        // One press of Ctrl+Z puts it all the way back.
        e.run(Command::Undo);
        let undone = e.scene().find_object(id).unwrap().1.bounds();
        assert!(
            (undone.x0 - before.x0).abs() < 1e-6,
            "one undo should reverse the whole drag, got {undone:?}"
        );
    }

    /// **What is drawn at a point is what a click there selects.**
    ///
    /// The one invariant the whole stage rests on, asserted against the two
    /// answers that must agree: `object_quad`, which is where the chrome draws
    /// an object, and `object_at`, which is what a click finds. Any transform
    /// applied by one and not the other shows up here as a selection that
    /// misses the artwork by exactly that transform — which is what "the
    /// selection has an offset" means.
    ///
    /// Built to look like an imported Animate character, because that is where
    /// every transform in the chain is in play at once: a symbol whose artwork
    /// sits far from its registration point, instanced with a transform of its
    /// own, on a layer parented to another layer that has moved.
    #[test]
    fn what_is_drawn_at_a_point_is_what_a_click_there_selects() {
        let mut e = editor();
        let child_layer = e.active_layer().expect("a layer");
        let mut instance = None;

        e.doc.edit("Character", |scene| {
            // A part symbol: artwork a long way from its registration point.
            let symbol = scene.add_symbol("Arm", buzz_scene::SymbolKind::Graphic, None);
            let inner = scene
                .library()
                .get(symbol)
                .and_then(|s| s.layers.iter().next())
                .map(|l| l.id)
                .expect("a layer inside the symbol");
            let art = scene.next_object_id();
            let art = Object::shape(
                art,
                ShapeData::filled(square(180.0, 140.0, 60.0), Color::WHITE),
            );
            scene.library_mut().update(symbol, |s| {
                s.layers.update(inner, |l| {
                    l.frames.set_objects(0, vec![Arc::new(art)]);
                });
            });

            // A parent layer that moves between its rest frame and frame 10.
            let parent = scene.add_layer("Body", buzz_scene::LayerKind::Normal);
            let rest = scene.next_object_id();
            let rest = Object::shape(rest, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
            let moved = scene.next_object_id();
            let moved = Object::shape(
                moved,
                ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE),
            )
            .with_transform(Affine::translate((90.0, 35.0)));
            scene.edit_layers().update(parent, |l| {
                l.frames.set_objects(0, vec![Arc::new(rest)]);
                l.frames.insert_keyframe(10);
                l.frames.set_objects(10, vec![Arc::new(moved)]);
            });

            // The instance, with a transform of its own, on the parented layer,
            // whose span has to reach the frame being tested — a layer that
            // does not is not drawn there and has nothing to select.
            instance = scene.add_instance_at(
                child_layer,
                0,
                symbol,
                Affine::translate((40.0, 25.0)),
            );
            scene.edit_layers().update(child_layer, |l| {
                l.follows = Some(parent);
                // F5: the span has to reach the frame being tested.
                l.frames.insert_frame(10);
            });
        });

        let instance = instance.expect("the instance was placed");
        e.current_frame = 10;

        // Where the chrome says the artwork is.
        let quad = e.object_quad(instance).expect("it is on screen");
        let drawn = quad
            .iter()
            .fold(Rect::from_points(quad[0], quad[0]), |r, p| r.union_pt(*p));

        // A click in the middle of it, taken the whole way round the loop the
        // pointer really travels: stage space -> screen -> back to edit space.
        let middle = drawn.center();
        let screen = e.camera.doc_to_screen(middle);
        let back = e.screen_to_edit(screen);
        assert!(
            (back - middle).hypot() < 1e-6,
            "the screen round trip moved the point by {:?}",
            back - middle
        );

        let hit = e.object_at(back, e.pick_tolerance());
        assert_eq!(
            hit,
            Some(instance),
            "clicking the middle of where it is drawn ({drawn:?}) must select it"
        );

        // And the selection it produces is measured in the same place, or the
        // handles draw off the artwork.
        e.selection.select_one(instance);
        let bounds = e
            .selection
            .bounds_at(e.scene(), e.current_frame)
            .expect("bounds");
        assert!(
            (bounds.center() - drawn.center()).hypot() < 1e-6,
            "the selection is measured at {:?} but drawn at {:?}",
            bounds.center(),
            drawn.center()
        );
    }

    /// **Chrome for another keyframe's artwork is drawn on that keyframe's
    /// terms.**
    ///
    /// Edit Multiple Frames puts other keyframes on the stage, and the renderer
    /// draws each through its own layer parenting. The selection chrome
    /// measured every object at the *playhead's* frame instead, so a parent
    /// that had moved between the two dragged the handles off the artwork by
    /// exactly that much — a selection with an offset, and one that only
    /// appears once a rig is animated.
    #[test]
    fn chrome_for_another_keyframe_uses_that_keyframes_parenting() {
        let mut e = editor();
        let child_layer = e.active_layer().expect("a layer");
        let mut art = None;

        e.doc.edit("Rig", |scene| {
            // A parent that is still at frame 0 and has moved on by frame 10.
            let parent = scene.add_layer("Body", buzz_scene::LayerKind::Normal);
            let rest = scene.next_object_id();
            let rest = Object::shape(rest, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
            let moved = scene.next_object_id();
            let moved =
                Object::shape(moved, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE))
                    .with_transform(Affine::translate((150.0, 0.0)));
            scene.edit_layers().update(parent, |l| {
                l.frames.set_objects(0, vec![Arc::new(rest)]);
                l.frames.insert_keyframe(10);
                l.frames.set_objects(10, vec![Arc::new(moved)]);
            });

            // The child's artwork exists on frame 0 only.
            let id = scene.next_object_id();
            art = Some(id);
            let shape =
                Object::shape(id, ShapeData::filled(square(200.0, 100.0, 30.0), Color::WHITE));
            scene.edit_layers().update(child_layer, |l| {
                l.follows = Some(parent);
                l.frames.set_objects(0, vec![Arc::new(shape)]);
            });
        });

        let art = art.expect("artwork");
        e.selection.select_one(art);

        // On frame 0 it is drawn where it was made: the parent is at rest.
        e.current_frame = 0;
        let quad = e.object_quad(art).expect("on screen at its own frame");
        let at_rest = quad
            .iter()
            .fold(Rect::from_points(quad[0], quad[0]), |r, p| r.union_pt(*p));
        assert!(
            (at_rest.x0 - 200.0).abs() < 1e-6,
            "at rest, got {at_rest:?}"
        );

        // Playhead at 10 with Edit Multiple Frames off: the artwork is not on
        // this frame at all, so there is no box to draw.
        e.current_frame = 10;
        e.edit_multiple = false;
        assert!(
            e.object_quad(art).is_none(),
            "a selection off the current frame must not draw a floating box"
        );
        assert!(e.selection_bounds_drawn().is_none());

        // Edit Multiple Frames on: the artwork *is* drawn — through frame 0's
        // parenting, because that is the frame it lives on. The chrome must
        // agree, and must not add the 150 the parent travelled afterwards.
        e.edit_multiple = true;
        e.onion.before = 20;
        e.onion.after = 20;
        assert!(
            e.multi_frames().contains(&0),
            "frame 0 should be one of the frames being shown"
        );
        let quad = e.object_quad(art).expect("drawn by Edit Multiple Frames");
        let shown = quad
            .iter()
            .fold(Rect::from_points(quad[0], quad[0]), |r, p| r.union_pt(*p));
        assert!(
            (shown.x0 - at_rest.x0).abs() < 1e-6,
            "it is drawn on frame 0's terms; chrome put it at {shown:?}, artwork is at {at_rest:?}"
        );

        let bounds = e.selection_bounds_drawn().expect("bounds");
        assert!(
            (bounds.center() - shown.center()).hypot() < 1e-6,
            "the handles are measured at {:?} but the artwork is at {:?}",
            bounds.center(),
            shown.center()
        );
    }

    /// **The round trip, in every mode that moves artwork.**
    ///
    /// Each of these draws the artwork somewhere other than where its geometry
    /// says it is, and each has to be undone before a click can be tested
    /// against that geometry. Any one that is applied on the way out and not on
    /// the way back shows up as a click that selects the shape *beside* the one
    /// under the pointer.
    #[test]
    fn a_click_finds_the_artwork_under_it_in_every_mode() {
        // Each case sets up one displacement and names itself, so a failure
        // says which mode is broken rather than which line.
        let cases: Vec<(&str, fn(&mut Editor, ObjectId))> = vec![
            ("plain", |_e, _id| {}),
            ("scene camera moved", |e, _id| {
                e.run(Command::ToggleCamera);
                e.doc.edit("Frame", |scene| {
                    let stage = scene.stage().size;
                    let mut key = scene
                        .camera()
                        .state_at(0)
                        .expect("enabling the camera seeds a key");
                    key.frame = 0;
                    key.center = Point::new(stage.width / 2.0 + 120.0, stage.height / 2.0);
                    scene.camera_mut().set_key(key);
                });
            }),
            ("layer pushed in depth", |e, _id| {
                let layer = e.active_layer().expect("a layer");
                e.run(Command::ToggleCamera);
                e.doc.edit("Depth", |scene| {
                    scene.update_layer(layer, |l| l.depth = 220.0);
                });
            }),
            ("object turned in space", |e, id| {
                e.doc.edit("Turn", |scene| {
                    let at = buzz_scene::EditAt::exact(0);
                    update_object(scene, at, id, |o| {
                        o.spatial.rotation_y = 0.6;
                    });
                });
            }),
            ("camera moved and layer in depth", |e, _id| {
                let layer = e.active_layer().expect("a layer");
                e.run(Command::ToggleCamera);
                e.doc.edit("Frame", |scene| {
                    let stage = scene.stage().size;
                    let mut key = scene
                        .camera()
                        .state_at(0)
                        .expect("enabling the camera seeds a key");
                    key.frame = 0;
                    key.center = Point::new(stage.width / 2.0 - 90.0, stage.height / 2.0 + 40.0);
                    key.zoom = 1.4;
                    scene.camera_mut().set_key(key);
                    scene.update_layer(layer, |l| l.depth = 160.0);
                });
            }),
        ];

        for (name, setup) in cases {
            let mut e = editor();
            e.style.drawing_mode = DrawingMode::ObjectDrawing;
            let id = draw_square(&mut e, 260.0, 180.0, 80.0, Color::WHITE)
                .expect("the square was drawn");
            setup(&mut e, id);

            let quad = e
                .object_quad(id)
                .unwrap_or_else(|| panic!("[{name}] nothing on screen to click"));
            let drawn = quad
                .iter()
                .fold(Rect::from_points(quad[0], quad[0]), |r, p| r.union_pt(*p));

            // The exact loop the pointer travels: stage space out to the
            // screen, and back in through the editor's own conversion.
            let screen = e.camera.doc_to_screen(drawn.center());
            let back = e.screen_to_edit(screen);
            let hit = e.object_at(back, e.pick_tolerance());
            assert_eq!(
                hit,
                Some(id),
                "[{name}] the artwork is drawn at {drawn:?}; clicking its middle found {hit:?}"
            );

            // The handle box and the object quad live in different spaces on
            // purpose — the quad is already through the camera, the bounds are
            // not — so they are compared where the chrome actually puts them,
            // which is the same trip `stage::draw_selection` makes.
            e.selection.select_one(id);
            let bounds = e
                .selection_bounds_drawn()
                .unwrap_or_else(|| panic!("[{name}] no bounds"));
            let shot = e
                .scene()
                .camera_projection_at_depth(e.current_frame, 0.0)
                .unwrap_or(buzz_geom::Projection::IDENTITY);
            let handles = shot
                .map_point(e.scene().edit_place() * bounds.center())
                .expect("the handle box is on screen");
            let slip = (handles - drawn.center()).hypot();
            assert!(
                slip < 1e-6,
                "[{name}] handles drawn at {handles:?}, artwork at {:?} - {slip:.1} out",
                drawn.center()
            );
        }
    }

    /// **A character rigged inside its own symbol is clicked where its parts
    /// are drawn.**
    ///
    /// A symbol's layers can follow each other — that is how an Animate
    /// character is built — and the renderer draws each part through the chain
    /// it follows. Hit testing tested the parts where they sat at rest, so a
    /// click on a raised arm missed it and fell through to whatever was behind:
    /// the object behind gets selected.
    #[test]
    fn a_part_rigged_inside_a_symbol_is_clicked_where_it_is_drawn() {
        let mut e = editor();
        let layer = e.active_layer().expect("a layer");
        let mut instance = None;
        let mut backdrop = None;

        e.doc.edit("Character", |scene| {
            let symbol = scene.add_symbol("Hero", buzz_scene::SymbolKind::Graphic, None);

            // Inside the symbol: a body layer that has moved between its rest
            // frame and frame 6, and an arm layer that follows it.
            let mut body = None;
            scene.library_mut().update(symbol, |s| {
                body = s.layers.iter().next().map(|l| l.id);
            });
            let body = body.expect("a symbol starts with a layer");
            // Ids inside a symbol come from the scene in the running program;
            // a test can name one, so long as it is not one already in use.
            let arm = buzz_scene::LayerId(9001);
            scene.library_mut().update(symbol, |s| {
                s.layers.push_front(buzz_scene::Layer::new(
                    arm,
                    "Arm",
                    buzz_scene::LayerKind::Normal,
                ));
            });

            let rest = scene.next_object_id();
            let rest = Object::shape(rest, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
            let moved = scene.next_object_id();
            let moved = Object::shape(moved, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE))
                .with_transform(Affine::translate((0.0, -130.0)));
            let limb = scene.next_object_id();
            let limb = Object::shape(
                limb,
                ShapeData::filled(square(20.0, 20.0, 40.0), Color::WHITE),
            );

            scene.library_mut().update(symbol, |s| {
                s.layers.update(body, |l| {
                    l.frames.set_objects(0, vec![Arc::new(rest)]);
                    l.frames.insert_keyframe(6);
                    l.frames.set_objects(6, vec![Arc::new(moved)]);
                });
                s.layers.update(arm, |l| {
                    l.follows = Some(body);
                    l.frames.set_objects(0, vec![Arc::new(limb)]);
                    l.frames.insert_frame(6);
                });
            });

            // A backdrop sitting exactly where the arm was drawn at rest, on a
            // layer below. If the click is tested at the rest position this is
            // what wins.
            let back = scene.next_object_id();
            let back = Object::shape(
                back,
                ShapeData::filled(square(0.0, 0.0, 200.0), Color::WHITE),
            );
            let under = scene.add_layer("Backdrop", buzz_scene::LayerKind::Normal);
            backdrop = Some(back.id);
            scene.edit_layers().update(under, |l| {
                l.frames.set_objects(0, vec![Arc::new(back)]);
                l.frames.insert_frame(6);
            });

            instance = scene.add_instance_at(layer, 0, symbol, Affine::IDENTITY);
            scene.edit_layers().update(layer, |l| {
                l.frames.insert_frame(6);
            });
        });

        let instance = instance.expect("the character was placed");
        let backdrop = backdrop.expect("the backdrop was placed");
        e.current_frame = 6;

        // The arm is drawn 130 above its rest position, carried there by the
        // body layer it follows. Its rest position is (20,20)-(60,60), so it is
        // drawn at (20,-110)-(60,-70).
        let on_the_arm = Point::new(40.0, -90.0);
        let screen = e.camera.doc_to_screen(on_the_arm);
        let hit = e.object_at(e.screen_to_edit(screen), e.pick_tolerance());
        assert_eq!(
            hit,
            Some(instance),
            "clicking the arm where it is drawn must take the character, not {hit:?}"
        );

        // And where the arm used to be, there is only the backdrop.
        let at_rest = Point::new(40.0, 40.0);
        let screen = e.camera.doc_to_screen(at_rest);
        let hit = e.object_at(e.screen_to_edit(screen), e.pick_tolerance());
        assert_eq!(
            hit,
            Some(backdrop),
            "the arm is no longer at its rest position, so the backdrop is what is there"
        );
    }

    /// **Dragging a selection on a real document has to stay interactive.**
    ///
    /// The artwork travels with the pointer, which means an edit per pointer
    /// move — and everything keyed on the document's revision is thrown away by
    /// each one. On a character built the way an imported one is (symbols
    /// inside symbols, a library of parts) that adds up to a re-measure of the
    /// whole library on every mouse move, which is what a drag that judders
    /// feels like.
    ///
    /// A budget rather than a stopwatch: this is measuring that the work per
    /// move is *bounded*, not how fast any particular machine is.
    #[test]
    fn dragging_a_selection_on_a_heavy_document_stays_interactive() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        // A library of parts, each a symbol with artwork of its own, and
        // instances of them on the stage — the shape of an imported character.
        const PARTS: usize = 400;
        let mut placed = Vec::new();
        e.doc.edit("Character", |scene| {
            let layer = scene
                .layers()
                .iter()
                .next()
                .map(|l| l.id)
                .expect("a layer");
            for part in 0..PARTS as i32 {
                let symbol = scene.add_symbol(
                    format!("Part {part}"),
                    buzz_scene::SymbolKind::Graphic,
                    None,
                );
                let inner = scene
                    .library()
                    .get(symbol)
                    .and_then(|s| s.layers.iter().next())
                    .map(|l| l.id)
                    .expect("a layer inside");
                let art: Vec<std::sync::Arc<Object>> = (0..12)
                    .map(|i| {
                        let id = scene.next_object_id();
                        std::sync::Arc::new(Object::shape(
                            id,
                            ShapeData::filled(
                                square(i as f64 * 9.0, part as f64 * 3.0, 14.0),
                                Color::WHITE,
                            ),
                        ))
                    })
                    .collect();
                scene.library_mut().update(symbol, |s| {
                    s.layers.update(inner, |l| {
                        l.frames.set_objects(0, art);
                    });
                });
                if let Some(id) = scene.add_instance_at(
                    layer,
                    0,
                    symbol,
                    Affine::translate((part as f64 * 11.0, 40.0)),
                ) {
                    placed.push(id);
                }
            }
        });
        assert_eq!(placed.len(), PARTS, "the character was built");

        // One part picked up and dragged across the stage, exactly as the
        // window drives it: press, a run of moves, release.
        let id = placed[20];
        e.selection.select_one(id);
        let camera = e.camera.clone();
        let screen = |p: Point| camera.doc_to_screen(p);
        let start = Point::new(220.0 + 5.0, 45.0);

        const MOVES: usize = 120;
        let began = std::time::Instant::now();
        e.pointer_down(screen(start), Mods::default());
        for step in 1..=MOVES {
            let at = Point::new(start.x + step as f64 * 2.0, start.y);
            e.pointer_move(screen(at), Mods::default());
            // The window asks for these every frame of the drag: the preview
            // for the stage, and the bounds for the transform box.
            let _ = e.preview();
            let _ = e.selection_bounds_drawn();
        }
        e.pointer_up(screen(Point::new(start.x + MOVES as f64 * 2.0, start.y)));
        let elapsed = began.elapsed();

        let per_move = elapsed.as_secs_f64() * 1000.0 / MOVES as f64;
        // A drag has to fit inside a frame with the rest of the window's work
        // still to do. Generous enough not to fail on a loaded CI machine, and
        // far below where a re-measure of the whole library per move lands.
        // The window re-encodes the stage after every one of those moves, and
        // that is what a drag actually waits on. Measured here with a move
        // between each encode, so anything keyed on the document revision is
        // as cold as it is during a real drag.
        let mut vello = buzz_render::vello::Scene::new();
        let mut cache = buzz_render::document::DrawCache::default();
        let area = buzz_geom::Rect::new(0.0, 0.0, 1600.0, 1000.0);
        const ENCODES: usize = 20;
        let encode_began = std::time::Instant::now();
        for step in 0..ENCODES {
            e.doc.edit("Move", |scene| {
                let at = buzz_scene::EditAt::exact(0);
                update_object(scene, at, id, |o| {
                    o.transform = Affine::translate((step as f64 * 0.5, 0.0)) * o.transform;
                });
            });
            crate::stage::build_scene(&mut vello, &e, area, 1.0, &mut cache);
        }
        let per_encode = encode_began.elapsed().as_secs_f64() * 1000.0 / ENCODES as f64;

        assert!(
            per_move < 2.0,
            "a pointer move cost {per_move:.2} ms; a drag cannot afford that"
        );
        // Generous enough not to fail on a loaded machine, and well under where
        // re-walking the whole library per move lands.
        assert!(
            per_encode < 5.0,
            "re-encoding the stage mid-drag cost {per_encode:.2} ms a frame"
        );
    }

    /// **A rotation turns about the transformation point the user moved.**
    ///
    /// Animate's white circle is not decoration: you put it on the shoulder and
    /// the arm swings from the shoulder. Setting it and then rotating are two
    /// gestures, and the second has to remember what the first did.
    #[test]
    fn a_rotation_turns_about_the_moved_transformation_point() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 100.0, 100.0, 100.0, Color::WHITE).expect("a square");
        e.selection.select_one(id);

        // By default it sits at the centre of the artwork.
        assert!(
            (e.pivot().expect("a pivot") - Point::new(150.0, 150.0)).hypot() < 1e-6,
            "it should start at the centre, got {:?}",
            e.pivot()
        );

        // Put it on the top-left corner, the way a shoulder joint is placed.
        let corner = Point::new(100.0, 100.0);
        e.apply(ToolAction::SetTransformPoint { at: corner });
        assert!(
            (e.pivot().expect("a pivot") - corner).hypot() < 1e-6,
            "the transformation point should have moved, got {:?}",
            e.pivot()
        );

        // Now rotate: the ring sits just *outside* a corner, which is where the
        // pointer turns into the rotation cursor.
        let camera = e.camera.clone();
        let screen = |p: Point| camera.doc_to_screen(p);
        let from = Point::new(214.0, 186.0);
        let to = Point::new(186.0, 214.0);
        e.pointer_down(screen(from), Mods::default());
        e.pointer_move(screen(to), Mods::default());
        e.pointer_up(screen(to));

        let after = e.scene().find_object(id).expect("still there").1.bounds();
        assert!(
            (after.x0 - 100.0).abs() > 1.0 || (after.y0 - 100.0).abs() > 1.0,
            "the drag should have rotated it at all, bounds are {after:?}"
        );

        // **Turning about a point leaves that point where it is.** Asked of the
        // transformation point itself rather than of the artwork's bounding
        // box, which a rotation changes the shape of whatever it turned about.
        let now = e.pivot().expect("still has one");
        assert!(
            (now - corner).hypot() < 1e-6,
            "the point it was turned about moved to {now:?}, so the rotation \
             turned about something else"
        );
    }

    /// **A transformation point parked on a joint must not swallow the
    /// rotate ring beside it.**
    ///
    /// This is how the control is actually used: the point goes on the
    /// shoulder — which is at the edge of the arm, not in the middle of it —
    /// and then you reach just outside the nearest corner for the ring. The
    /// circle is a forgiving target on purpose, so it claimed that reach too,
    /// and the arm could never be swung about the joint it had just been given.
    #[test]
    fn a_transformation_point_on_a_corner_still_leaves_room_to_rotate() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 100.0, 100.0, 100.0, Color::WHITE).expect("a square");
        e.selection.select_one(id);

        // The joint, right on the corner of the artwork.
        let joint = Point::new(200.0, 200.0);
        e.apply(ToolAction::SetTransformPoint { at: joint });
        assert!((e.pivot().expect("a pivot") - joint).hypot() < 1e-6);

        let before = e.scene().find_object(id).expect("there").1.transform;

        // The ring, just outside that same corner.
        let camera = e.camera.clone();
        let screen = |p: Point| camera.doc_to_screen(p);
        let from = Point::new(210.0, 210.0);
        let to = Point::new(190.0, 226.0);
        e.pointer_down(screen(from), Mods::default());
        e.pointer_move(screen(to), Mods::default());
        e.pointer_up(screen(to));

        let after = e.scene().find_object(id).expect("there").1.transform;
        assert!(
            before.as_coeffs() != after.as_coeffs(),
            "the drag should have turned the artwork, not moved the point"
        );
        let now = e.pivot().expect("still has one");
        assert!(
            (now - joint).hypot() < 1e-6,
            "and it should have turned about the joint, which is now {now:?}"
        );
    }

    /// **A transformation point set on a *group* survives to the rotation.**
    ///
    /// A character on the stage is several objects, so this is the case that
    /// matters: place the point, then swing the lot about it.
    #[test]
    fn a_group_rotates_about_its_moved_transformation_point() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 100.0, 100.0, 60.0, Color::WHITE).expect("a");
        let b = draw_square(&mut e, 200.0, 100.0, 60.0, Color::WHITE).expect("b");
        e.selection.set(vec![a, b]);

        let joint = Point::new(110.0, 110.0);
        e.apply(ToolAction::SetTransformPoint { at: joint });
        assert!(
            (e.pivot().expect("a pivot") - joint).hypot() < 1e-6,
            "the group point should have moved, got {:?}",
            e.pivot()
        );

        // Where the group's box is, and the ring just outside its far corner.
        let bounds = e.selection_bounds_drawn().expect("bounds");
        let camera = e.camera.clone();
        let screen = |p: Point| camera.doc_to_screen(p);
        let from = Point::new(bounds.x1 + 10.0, bounds.y1 + 10.0);
        let to = Point::new(bounds.x1 - 10.0, bounds.y1 + 26.0);

        let before = e.scene().find_object(a).expect("a").1.transform;
        e.pointer_down(screen(from), Mods::default());
        e.pointer_move(screen(to), Mods::default());
        e.pointer_up(screen(to));

        let after = e.scene().find_object(a).expect("a").1.transform;
        assert!(
            before.as_coeffs() != after.as_coeffs(),
            "the group should have turned"
        );

        // The point it turned about has not moved.
        let now = e.pivot().expect("still has one");
        assert!(
            (now - joint).hypot() < 1e-6,
            "the group turned about {now:?} instead of the point set at {joint:?}"
        );
    }

    /// **Move the wrist and the palm goes with it.**
    ///
    /// The whole point of layer parenting, and the case it is actually used in:
    /// a character posed on one keyframe, nothing animated yet, and a chain of
    /// limbs that has to move as a limb.
    #[test]
    fn moving_a_parent_layer_carries_its_children() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        let wrist_layer = e.active_layer().expect("a layer");
        let wrist = draw_square(&mut e, 100.0, 100.0, 30.0, Color::WHITE).expect("wrist");

        let mut palm_layer = None;
        let mut palm = None;
        e.doc.edit("Palm", |scene| {
            let layer = scene.add_layer("palm", buzz_scene::LayerKind::Normal);
            palm_layer = Some(layer);
            let id = scene.next_object_id();
            palm = Some(id);
            let art =
                Object::shape(id, ShapeData::filled(square(140.0, 100.0, 20.0), Color::WHITE));
            scene.edit_layers().update(layer, |l| {
                l.frames.set_objects(0, vec![Arc::new(art)]);
            });
            // Through the same call the timeline and the Layers panel make,
            // which records the pose the link was made at.
            scene.set_follows(layer, Some(wrist_layer), 0);
        });
        let palm = palm.expect("the palm");
        let palm_layer = palm_layer.expect("the palm layer");

        let where_is = |e: &Editor, id: ObjectId, layer: buzz_scene::LayerId| -> Point {
            let scene = e.scene();
            let object = scene
                .layers()
                .get(layer)
                .and_then(|l| l.objects_at(0).iter().find(|o| o.id == id).cloned())
                .expect("the artwork");
            let follows = scene.layers().inherited_transform(layer, 0);
            buzz_scene::object::transform_rect(follows, object.bounds()).origin()
        };

        let palm_before = where_is(&e, palm, palm_layer);

        // Drag the wrist 60 to the right, as the pointer would.
        e.selection.select_one(wrist);
        e.apply(ToolAction::MoveSelection {
            delta: Vec2::new(60.0, 0.0),
        });

        let wrist_after = where_is(&e, wrist, wrist_layer);
        assert!(
            (wrist_after.x - 160.0).abs() < 1e-6,
            "the wrist should have moved, it is at {wrist_after:?}"
        );

        let palm_after = where_is(&e, palm, palm_layer);
        assert!(
            (palm_after.x - palm_before.x - 60.0).abs() < 1e-6,
            "the palm should have gone with it: {palm_before:?} -> {palm_after:?}"
        );
    }

    /// **Both tools turn the artwork, and turn it as the pointer moves.**
    ///
    /// The rotation used to be drawn as an outline and committed on release,
    /// so the drawing sat still while a wireframe swung around it. Asked here
    /// through the whole editor, for the Selection tool and Free Transform
    /// alike — Free Transform is the one an animator reaches for to rotate, and
    /// it has to be the one that does.
    #[test]
    fn both_transform_tools_rotate_while_the_pointer_moves() {
        for tool in [buzz_ui::ToolId::Selection, buzz_ui::ToolId::FreeTransform] {
            let mut e = editor();
            e.style.drawing_mode = DrawingMode::ObjectDrawing;
            let id = draw_square(&mut e, 100.0, 100.0, 100.0, Color::WHITE).expect("a square");
            e.selection.select_one(id);
            e.set_tool(tool);

            let before = e.scene().find_object(id).expect("there").1.transform;
            let camera = e.camera.clone();
            let screen = |p: Point| camera.doc_to_screen(p);

            // Press on the ring just outside a corner, then walk the pointer
            // round it a step at a time.
            e.pointer_down(screen(Point::new(214.0, 186.0)), Mods::default());
            let mut seen_turning = false;
            for step in 1..=8 {
                let angle = (step as f64) * 0.08;
                let at = Point::new(
                    150.0 + 90.0 * angle.cos(),
                    150.0 + 90.0 * angle.sin() - 60.0,
                );
                e.pointer_move(screen(at), Mods::default());
                let now = e.scene().find_object(id).expect("there").1.transform;
                if now.as_coeffs() != before.as_coeffs() {
                    seen_turning = true;
                }
            }
            assert!(
                seen_turning,
                "[{tool:?}] the artwork should turn while the pointer moves,                  not only when it is released"
            );

            e.pointer_up(screen(Point::new(120.0, 220.0)));
            let after = e.scene().find_object(id).expect("there").1.transform;
            assert!(
                before.as_coeffs() != after.as_coeffs(),
                "[{tool:?}] and the turn should still be there afterwards"
            );
        }
    }

    /// **The transformation point stays where it is when you turn something.**
    ///
    /// With no point placed by hand it is derived from the artwork's bounding
    /// box — and the bounding box of a turned shape is not the box it was, so
    /// on anything that is not symmetric the point wandered off as the artwork
    /// turned. The one control whose whole job is to be the fixed point of the
    /// rotation was the thing that moved.
    #[test]
    fn the_transformation_point_does_not_drift_when_the_artwork_turns() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        // Deliberately not symmetric: a triangle's bounding box moves under a
        // rotation about its own centre, which a square's does not.
        let mut path = BezPath::new();
        path.move_to(Point::new(100.0, 100.0));
        path.line_to(Point::new(220.0, 130.0));
        path.line_to(Point::new(130.0, 200.0));
        path.close_path();
        let id = {
            let before: Vec<ObjectId> = e
                .scene()
                .layers()
                .iter()
                .flat_map(|l| l.objects_at(0).iter())
                .map(|o| o.id)
                .collect();
            e.apply(ToolAction::AddShape {
                shape: ShapeData::filled(path, Color::WHITE),
                label: "Draw",
            });
            e.scene()
                .layers()
                .iter()
                .flat_map(|l| l.objects_at(0).iter())
                .map(|o| o.id)
                .find(|id| !before.contains(id))
                .expect("the triangle")
        };
        e.selection.select_one(id);

        let pivot = e.pivot().expect("a transformation point");

        // Turn it about that point, the way the ring does.
        e.apply(ToolAction::TransformSelection {
            transform: Affine::translate(pivot.to_vec2())
                * Affine::rotate(0.6)
                * Affine::translate(-pivot.to_vec2()),
        });

        let after = e.pivot().expect("still has one");
        assert!(
            (after - pivot).hypot() < 1e-6,
            "the point turned about should not have moved: {pivot:?} -> {after:?}"
        );

        // **And the same for several objects at once**, which is the case that
        // actually drifts: one object's box turns about its own centre and
        // stays centred there, but the union of several boxes does not.
        let second = draw_square(&mut e, 320.0, 120.0, 40.0, Color::WHITE).expect("a square");
        e.selection.set(vec![id, second]);
        let group_pivot = e.pivot().expect("a group point");
        e.apply(ToolAction::TransformSelection {
            transform: Affine::translate(group_pivot.to_vec2())
                * Affine::rotate(0.5)
                * Affine::translate(-group_pivot.to_vec2()),
        });
        let after = e.pivot().expect("still has one");
        assert!(
            (after - group_pivot).hypot() < 1e-6,
            "the group's point should not have moved: {group_pivot:?} -> {after:?}"
        );
    }

    /// **A rotation turns and does nothing else.**
    ///
    /// A rotation, possibly with a uniform scale, always satisfies `a == d`
    /// and `b == -c`. Shear breaks both, so this measures the matrix directly
    /// rather than looking at the artwork: a drag is hundreds of small steps
    /// multiplied together, and anything that is not quite a rotation
    /// accumulates into a visible lean.
    #[test]
    fn turning_something_never_shears_it() {
        for tool in [buzz_ui::ToolId::Selection, buzz_ui::ToolId::FreeTransform] {
            let mut e = editor();
            e.style.drawing_mode = DrawingMode::ObjectDrawing;
            let id = draw_square(&mut e, 100.0, 100.0, 100.0, Color::WHITE).expect("a square");
            e.selection.select_one(id);
            e.set_tool(tool);

            let camera = e.camera.clone();
            let screen = |p: Point| camera.doc_to_screen(p);
            let pivot = e.pivot().expect("a point");

            // A long, finely-stepped drag right round the pivot — the worst
            // case for anything that drifts, and what a real drag is.
            e.pointer_down(screen(Point::new(214.0, 186.0)), Mods::default());
            for step in 1..=400 {
                let angle = (step as f64) * 0.015;
                let at = Point::new(
                    pivot.x + 92.0 * angle.cos(),
                    pivot.y + 92.0 * angle.sin(),
                );
                e.pointer_move(screen(at), Mods::default());
            }
            let end = Point::new(pivot.x + 92.0, pivot.y);
            e.pointer_up(screen(end));

            let c = e.scene().find_object(id).expect("there").1.transform.as_coeffs();
            let (a, b, cc, d) = (c[0], c[1], c[2], c[3]);
            assert!(
                (a - d).abs() < 1e-9 && (b + cc).abs() < 1e-9,
                "[{tool:?}] a rotation must leave a == d and b == -c; got                  a={a}, b={b}, c={cc}, d={d}"
            );
            // And it stayed the size it was: a rotation is not a scale either.
            let scale = (a * a + b * b).sqrt();
            assert!(
                (scale - 1.0).abs() < 1e-9,
                "[{tool:?}] the turn changed its size by {scale}"
            );
        }
    }

    /// **Resize, squeeze and skew are reachable, from either tool, and live.**
    ///
    /// The handles are drawn for the selection tools as well as Free
    /// Transform, so they have to work from both — a handle you can see and
    /// cannot use is worse than none.
    #[test]
    fn corners_resize_and_edges_skew_from_either_tool() {
        for tool in [buzz_ui::ToolId::Selection, buzz_ui::ToolId::FreeTransform] {
            // -- a corner scales ------------------------------------------
            let mut e = editor();
            e.style.drawing_mode = DrawingMode::ObjectDrawing;
            let id = draw_square(&mut e, 100.0, 100.0, 100.0, Color::WHITE).expect("a square");
            e.selection.select_one(id);
            e.set_tool(tool);
            let camera = e.camera.clone();
            let screen = |p: Point| camera.doc_to_screen(p);

            let before = e.scene().find_object(id).expect("there").1.bounds();
            // Grab the bottom-right corner and pull it out.
            e.pointer_down(screen(Point::new(200.0, 200.0)), Mods::default());
            e.pointer_move(screen(Point::new(260.0, 240.0)), Mods::default());
            let during = e.scene().find_object(id).expect("there").1.bounds();
            assert!(
                during.width() > before.width() + 1.0,
                "[{tool:?}] dragging a corner should resize it as the pointer moves"
            );
            e.pointer_up(screen(Point::new(260.0, 240.0)));

            // A scale leaves the matrix free of shear.
            let c = e.scene().find_object(id).expect("there").1.transform.as_coeffs();
            assert!(
                c[1].abs() < 1e-9 && c[2].abs() < 1e-9,
                "[{tool:?}] a resize must not shear: {c:?}"
            );

            // -- the top handle squeezes vertically -----------------------
            //
            // The gesture the gizmo used to have no answer for: press the top
            // down and the artwork gets shorter, with the bottom left where it
            // stands. Dragging straight down the top edge used to move nothing
            // at all, because every edge sheared and a horizontal edge shears
            // in x alone.
            let mut e = editor();
            e.style.drawing_mode = DrawingMode::ObjectDrawing;
            let id = draw_square(&mut e, 100.0, 100.0, 100.0, Color::WHITE).expect("a square");
            e.selection.select_one(id);
            e.set_tool(tool);

            let before = e.scene().find_object(id).expect("there").1.bounds();
            // The handle at the middle of the top edge, pressed halfway down.
            e.pointer_down(screen(Point::new(150.0, 100.0)), Mods::default());
            e.pointer_move(screen(Point::new(150.0, 150.0)), Mods::default());
            let during = e.scene().find_object(id).expect("there").1.bounds();
            assert!(
                during.height() < before.height() - 1.0,
                "[{tool:?}] pressing the top handle down should squash it as the pointer moves"
            );
            e.pointer_up(screen(Point::new(150.0, 150.0)));

            let after = e.scene().find_object(id).expect("there").1.bounds();
            assert!(
                (after.width() - before.width()).abs() < 1.0,
                "[{tool:?}] a vertical squeeze must not change the width: {after:?}"
            );
            assert!(
                (after.y1 - before.y1).abs() < 1.0,
                "[{tool:?}] the far edge is the anchor and must not move: {after:?}"
            );
            let c = e.scene().find_object(id).expect("there").1.transform.as_coeffs();
            assert!(
                c[1].abs() < 1e-9 && c[2].abs() < 1e-9,
                "[{tool:?}] a squeeze must not shear: {c:?}"
            );

            // -- an edge skews --------------------------------------------
            let mut e = editor();
            e.style.drawing_mode = DrawingMode::ObjectDrawing;
            let id = draw_square(&mut e, 100.0, 100.0, 100.0, Color::WHITE).expect("a square");
            e.selection.select_one(id);
            e.set_tool(tool);

            // The top edge *between* its handle and the corner, clear of both.
            e.pointer_down(screen(Point::new(175.0, 100.0)), Mods::default());
            e.pointer_move(screen(Point::new(215.0, 100.0)), Mods::default());
            e.pointer_up(screen(Point::new(215.0, 100.0)));

            let c = e.scene().find_object(id).expect("there").1.transform.as_coeffs();
            assert!(
                c[1].abs() > 1e-6 || c[2].abs() > 1e-6,
                "[{tool:?}] dragging an edge should skew it: {c:?}"
            );
        }
    }

    /// **Moving a lamp over a scene has to stay interactive.**
    ///
    /// Every shading crescent is a boolean difference — the most expensive
    /// thing the renderer does — and a lamp's geometry depends on where the
    /// lamp is, so dragging one invalidates all of it on every pointer move.
    /// Whether that is affordable is the whole question, and it is the one
    /// that decides whether the lighting tools can be used at all.
    #[test]
    fn dragging_a_lamp_over_a_scene_stays_interactive() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        // A street's worth of artwork for the lamp to fall on.
        // Curved outlines, not squares: a boolean's cost follows the segment
        // count, and real artwork is curves.
        const SHAPES: usize = 400;
        e.doc.edit("Street", |scene| {
            let layer = scene.layers().iter().next().map(|l| l.id).expect("a layer");
            let art: Vec<std::sync::Arc<Object>> = (0..SHAPES)
                .map(|i| {
                    let x = (i % 20) as f64 * 60.0;
                    let y = (i / 20) as f64 * 60.0;
                    let id = scene.next_object_id();
                    let path = kurbo::Circle::new(Point::new(x + 24.0, y + 24.0), 22.0)
                        .to_path(0.05);
                    std::sync::Arc::new(Object::shape(
                        id,
                        ShapeData::filled(path, Color::WHITE),
                    ))
                })
                .collect();
            scene.edit_layers().update(layer, |l| {
                l.frames.set_objects(0, art);
            });
        });

        // The lamp, casting shadows, as one placed from the Lights panel is.
        e.doc.edit("Light", |scene| {
            let rig = scene.lights_mut();
            rig.enabled = true;
            let mut lamp = buzz_scene::Light::new(
                buzz_scene::LightId(1),
                "Lamp",
                buzz_scene::LightKind::Lamp {
                    position: Point::new(400.0, 300.0),
                    height: 220.0,
                    radius: 900.0,
                },
            );
            lamp.shadows = true;
            rig.lights.push(lamp);
        });
        assert!(e.scene().lights().is_active(), "the rig should be on");

        let mut vello = buzz_render::vello::Scene::new();
        let mut cache = buzz_render::document::DrawCache::default();
        let area = buzz_geom::Rect::new(0.0, 0.0, 1600.0, 1000.0);

        // Warm it once, as the window does on the frame the light appears.
        crate::stage::build_scene(&mut vello, &e, area, 1.0, &mut cache);

        // **As the window does while a gesture is running.** A lamp's geometry
        // depends on where the lamp is, so every entry it owns misses on every
        // pointer move; built inline those misses are one boolean difference
        // per shape, on the frame thread. Deferred, they are handed off and the
        // frame draws with the shading it had.
        cache.lights.set_defer(true);

        // Now walk the lamp across the stage, re-encoding after each move
        // exactly as the window does.
        const MOVES: usize = 12;
        let began = std::time::Instant::now();
        for step in 1..=MOVES {
            let at = Point::new(400.0 + step as f64 * 20.0, 300.0);
            e.doc.edit("Move Light", |scene| {
                if let Some(light) = scene.lights_mut().get_mut(buzz_scene::LightId(1))
                    && let buzz_scene::LightKind::Lamp { position, .. } = &mut light.kind
                {
                    *position = at;
                }
            });
            crate::stage::build_scene(&mut vello, &e, area, 1.0, &mut cache);
            // The window hands these to a worker; here they are simply not
            // built on this thread, which is the point being measured.
            let _ = cache.lights.take_misses();
        }
        let per_move = began.elapsed().as_secs_f64() * 1000.0 / MOVES as f64;

        eprintln!("LAMP DRAG: {per_move:.2} ms a frame over {SHAPES} shapes");
        assert!(
            per_move < 16.0,
            "moving a lamp cost {per_move:.1} ms a frame; the window cannot \
             draw at that and the tool is unusable"
        );

        // **And the frame the drag ends on.**
        //
        // This is where the freeze went when deferring during the gesture was
        // added: the pointer comes up, nothing is deferring any more, every
        // crescent in the document is stale, and the frame built all of them
        // inline. Measured at 170 ms over three hundred shapes — a freeze that
        // had been moved rather than removed, and worse than the one before it,
        // because it landed at the moment the user expected to be finished.
        //
        // The window draws this frame with an inline budget, so what it may
        // build is a fixed handful whatever it finds stale.
        cache
            .lights
            .set_inline_budget(buzz_render::lighting::INLINE_BUDGET);
        cache.lights.set_queue(true);
        let began = std::time::Instant::now();
        crate::stage::build_scene(&mut vello, &e, area, 1.0, &mut cache);
        let ending = began.elapsed().as_secs_f64() * 1000.0;

        eprintln!("LAMP DROP: {ending:.2} ms on the frame the drag ends");
        // **Fourteen, not ten.** A lamp is drawn per pixel now — its falloff is a
        // gradient laid over the artwork rather than one tint per shape — so a
        // shape a lamp varies across goes through the composited path, which is
        // a few passes rather than one fill. On this scene that is every one of
        // the four hundred, because the lamp's reach is 900 over a 1600-wide
        // stage: the worst case there is, which is what a guard should measure.
        // It moved the frame from about 8 ms to about 9.5. What this exists to
        // catch is a *freeze* — it was written against 170 ms — and the budget
        // still sits inside a single 60 Hz frame, so it still catches one.
        assert!(
            ending < 14.0,
            "the frame the drag ended on cost {ending:.1} ms; the freeze was \
             moved to the end of the gesture rather than removed"
        );
        assert!(
            !cache.lights.take_misses().is_empty(),
            "what it could not build must still be queued, or the shading \
             never becomes exact"
        );
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

    // -- the clipboard -----------------------------------------------------

    /// **Copy then paste puts a second copy on the stage, and selects it.**
    ///
    /// The selection matters as much as the copy: what you just pasted is what
    /// you want to drag, which is the rule Duplicate and Place Asset follow.
    #[test]
    fn pasting_adds_a_copy_and_selects_it() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        e.run(Command::Copy);
        assert!(e.clipboard.is_some(), "nothing reached the clipboard");
        assert_eq!(e.scene().shape_count(), 1, "copying must not add artwork");

        e.run(Command::Paste);
        assert_eq!(e.scene().shape_count(), 2);
        assert_eq!(e.selection.len(), 1);
        assert!(
            !e.selection.contains(id),
            "the pasted copy should be selected, not the original"
        );
    }

    /// The clipboard is not consumed — pasting the same character into four
    /// scenes is the thing it exists for.
    #[test]
    fn pasting_twice_gives_two_copies() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);
        e.run(Command::Copy);

        e.run(Command::Paste);
        e.run(Command::Paste);
        assert_eq!(e.scene().shape_count(), 3);
    }

    /// **Cut copies before it deletes.** A Cut that deleted without copying
    /// would be Delete with a misleading name.
    #[test]
    fn cutting_removes_the_artwork_and_keeps_it_for_pasting() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);

        e.run(Command::Cut);
        assert_eq!(e.scene().shape_count(), 0, "cut should remove it");
        assert!(e.clipboard.is_some());

        e.run(Command::Paste);
        assert_eq!(e.scene().shape_count(), 1, "and it should come back");
    }

    /// Cut with nothing selected must not delete anything, and must not leave
    /// an empty clipboard behind that a later Paste would act on.
    #[test]
    fn cutting_nothing_does_nothing() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE);
        e.selection.clear();

        e.run(Command::Cut);
        assert_eq!(e.scene().shape_count(), 1);
        assert!(e.clipboard.is_none());
    }

    /// Pasting with an empty clipboard says so rather than doing something.
    #[test]
    fn pasting_an_empty_clipboard_explains_itself() {
        let mut e = editor();
        e.run(Command::Paste);
        assert_eq!(e.scene().shape_count(), 0);
        assert!(
            e.status
                .as_deref()
                .unwrap_or_default()
                .contains("clipboard"),
            "it should say why nothing happened"
        );
    }

    /// **A pasted instance brings its symbol with it.**
    ///
    /// This is the reason the clipboard holds a whole `Scene` rather than a
    /// list of objects: an instance whose symbol was left behind draws
    /// nothing. Simulated here the way the cross-document case works — the
    /// clipboard is carried into an editor whose document has never seen the
    /// symbol.
    #[test]
    fn a_pasted_instance_carries_its_symbol_into_another_document() {
        let mut source = editor();
        source.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut source, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        source.selection.select_one(id);
        source.run(Command::ConvertToSymbol);

        let instance = source.selection.ids();
        assert_eq!(instance.len(), 1, "convert should leave the instance selected");
        assert_eq!(source.scene().library().len(), 1);
        source.run(Command::Copy);

        // A different document, as `App::adopt_document` hands the clipboard on.
        let mut target = editor();
        target.clipboard = source.clipboard.clone();
        assert_eq!(target.scene().library().len(), 0);

        target.run(Command::Paste);
        assert_eq!(
            target.scene().library().len(),
            1,
            "the symbol should have come across with its instance"
        );
        assert_eq!(target.selection.len(), 1);
    }

    /// A paste is one undo step, and undoing it leaves the document as it was.
    #[test]
    fn pasting_is_one_undo_step() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);
        e.run(Command::Copy);
        e.run(Command::Paste);
        assert_eq!(e.scene().shape_count(), 2);

        e.run(Command::Undo);
        assert_eq!(e.scene().shape_count(), 1);
    }

    /// Dropping a symbol on the stage places it **where it was dropped**, at
    /// any zoom or pan — the point of dragging rather than pressing Place.
    #[test]
    fn dropping_a_symbol_places_it_under_the_pointer() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);
        e.run(Command::ConvertToSymbol);
        let symbol = e.scene().library().iter().next().expect("a symbol").id;

        // A pointer well away from the middle of the view.
        let screen = Point::new(700.0, 120.0);
        let expected = e.camera.screen_to_doc(screen);
        e.place_symbol_at(symbol, screen);

        let placed = e.selection.ids();
        assert_eq!(placed.len(), 1, "the new instance should be selected");
        // The instance's own origin lands on the drop point, so the artwork
        // arrives centred there rather than hanging off it by its corner.
        let centre = e.scene().find_object(placed[0]).unwrap().1.bounds().center();
        assert!(
            (centre.x - expected.x).abs() < 2.0 && (centre.y - expected.y).abs() < 2.0,
            "landed centred at {centre:?}, wanted {expected:?}"
        );
    }

    /// Placing from the menu, with no pointer, still lands in the middle of
    /// the view rather than nowhere.
    #[test]
    fn placing_from_the_command_uses_the_middle_of_the_view() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);
        e.run(Command::ConvertToSymbol);
        let symbol = e.scene().library().iter().next().expect("a symbol").id;
        e.library.selected = Some(symbol);

        let before = e.scene().shape_count();
        e.run(Command::PlaceInstance);
        assert!(e.scene().shape_count() >= before);
        assert_eq!(e.selection.len(), 1);
    }

    // -- align and distribute ----------------------------------------------

    /// Aligning moves the artwork on the stage, not just the numbers.
    #[test]
    fn aligning_left_lines_the_selection_up() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 10.0, 0.0, 20.0, Color::WHITE).unwrap();
        let b = draw_square(&mut e, 90.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.set(vec![a, b]);

        e.run(Command::Align {
            op: buzz_ui::Align::LeftEdges,
            to_stage: false,
        });

        let left_a = e.scene().find_object(a).unwrap().1.bounds().min_x();
        let left_b = e.scene().find_object(b).unwrap().1.bounds().min_x();
        assert!((left_a - left_b).abs() < 1e-9, "{left_a} vs {left_b}");
        assert!((left_a - 10.0).abs() < 1e-9, "the leftmost should not move");
    }

    /// Align to stage centres on the *stage*, which is the whole point of the
    /// distinction.
    #[test]
    fn aligning_to_the_stage_centres_on_the_stage() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 0.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);
        let stage = e.scene().stage().stage_rect();

        e.run(Command::Align {
            op: buzz_ui::Align::HorizontalCentres,
            to_stage: true,
        });

        let centre = e.scene().find_object(id).unwrap().1.bounds().center().x;
        assert!((centre - stage.center().x).abs() < 1e-6, "at {centre}");
    }

    /// Distributing needs three; with two it says so rather than doing
    /// nothing silently.
    #[test]
    fn distributing_two_objects_explains_itself() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 0.0, 0.0, 10.0, Color::WHITE).unwrap();
        let b = draw_square(&mut e, 100.0, 0.0, 10.0, Color::WHITE).unwrap();
        e.selection.set(vec![a, b]);

        e.run(Command::Distribute(
            buzz_ui::Distribute::HorizontalCentres,
        ));
        assert!(
            e.status.as_deref().unwrap_or_default().contains("three"),
            "it should say what is needed"
        );
    }

    #[test]
    fn distributing_three_objects_evens_them_out() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 0.0, 0.0, 10.0, Color::WHITE).unwrap();
        let b = draw_square(&mut e, 30.0, 0.0, 10.0, Color::WHITE).unwrap();
        let c = draw_square(&mut e, 100.0, 0.0, 10.0, Color::WHITE).unwrap();
        e.selection.set(vec![a, b, c]);

        e.run(Command::Distribute(
            buzz_ui::Distribute::HorizontalCentres,
        ));

        let centre = |id| e.scene().find_object(id).unwrap().1.bounds().center().x;
        let (x0, x1, x2) = (centre(a), centre(b), centre(c));
        assert!(((x1 - x0) - (x2 - x1)).abs() < 1e-6, "{x0} {x1} {x2}");
    }

    /// Match Size scales about each object's own centre, so nothing wanders.
    #[test]
    fn matching_size_grows_the_smaller_one_in_place() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let small = draw_square(&mut e, 0.0, 0.0, 10.0, Color::WHITE).unwrap();
        let large = draw_square(&mut e, 100.0, 0.0, 40.0, Color::WHITE).unwrap();
        e.selection.set(vec![small, large]);
        let before = e.scene().find_object(small).unwrap().1.bounds().center();

        e.run(Command::MatchSize(buzz_ui::MatchSize::Width));

        let after = e.scene().find_object(small).unwrap().1.bounds();
        assert!((after.width() - 40.0).abs() < 1e-6, "width {}", after.width());
        assert!(
            (after.center().x - before.x).abs() < 1e-6,
            "it should grow about its own centre"
        );
    }

    /// Aligning is one undo step however many objects moved.
    #[test]
    fn aligning_is_one_undo_step() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let a = draw_square(&mut e, 10.0, 0.0, 20.0, Color::WHITE).unwrap();
        let b = draw_square(&mut e, 90.0, 0.0, 20.0, Color::WHITE).unwrap();
        e.selection.set(vec![a, b]);
        let before = e.scene().find_object(b).unwrap().1.bounds().min_x();

        e.run(Command::Align {
            op: buzz_ui::Align::LeftEdges,
            to_stage: false,
        });
        e.run(Command::Undo);

        let after = e.scene().find_object(b).unwrap().1.bounds().min_x();
        assert!((after - before).abs() < 1e-9);
    }

    // -- the arrow keys ----------------------------------------------------

    /// A nudge moves the selection by exactly the distance asked for, in the
    /// direction asked for. Screen down is +y, as everywhere else here.
    #[test]
    fn nudging_moves_the_selection_by_whole_units() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 10.0, 10.0, 20.0, Color::WHITE).unwrap();
        e.selection.select_one(id);
        let before = e.scene().find_object(id).unwrap().1.bounds();

        e.run(Command::Nudge { x: 1, y: 0 });
        let after = e.scene().find_object(id).unwrap().1.bounds();
        assert!((after.min_x() - before.min_x() - 1.0).abs() < 1e-9);
        assert!((after.min_y() - before.min_y()).abs() < 1e-9);

        e.run(Command::Nudge { x: 0, y: 8 });
        let down = e.scene().find_object(id).unwrap().1.bounds();
        assert!((down.min_y() - after.min_y() - 8.0).abs() < 1e-9);
    }

    /// Nudging nothing changes nothing — and in particular does not record an
    /// undo step for a move that did not happen.
    #[test]
    fn nudging_with_nothing_selected_does_nothing() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let id = draw_square(&mut e, 10.0, 10.0, 20.0, Color::WHITE).unwrap();
        e.selection.clear();
        let before = e.scene().find_object(id).unwrap().1.bounds();

        e.run(Command::Nudge { x: 1, y: 0 });
        let after = e.scene().find_object(id).unwrap().1.bounds();
        assert!((after.min_x() - before.min_x()).abs() < 1e-9);
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
            shape
                .fill
                .as_ref()
                .unwrap()
                .color()
                .to_rgba8()
                .to_u8_array()[0],
            255
        );
    }

    /// Clicking inside a stroke-only outline creates a new fill shape, behind
    /// the lines — the paint bucket's actual job, not just recolouring.
    #[test]
    fn the_bucket_floods_an_empty_region_bounded_by_lines() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        // A square outline with a stroke and no fill: the inside is empty.
        let outline = ShapeData {
            path: square(0.0, 0.0, 80.0),
            fill: None,
            stroke: Some(StrokeSpec {
                paint: Paint::Solid(Color::BLACK),
                width: 2.0,
                hairline: false,
                swatch: None,
            }),
            blend: buzz_scene::PaintBlend::default(),
        };
        e.apply(ToolAction::AddShape {
            shape: outline,
            label: "Draw",
        });
        let before = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .count();

        e.style.fill_enabled = true;
        e.style.fill_color = Color::from_rgb8(0x00, 0xFF, 0x00);
        e.apply(ToolAction::BucketFill {
            point: Point::new(40.0, 40.0),
        });

        let objects: Vec<_> = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter().cloned())
            .collect();
        assert_eq!(
            objects.len(),
            before + 1,
            "the bucket should have created a fill shape"
        );
        // Behind everything (index 0) and a fill-only shape.
        let ObjectKind::Shape(first) = &objects[0].kind else {
            panic!("expected a shape behind the lines");
        };
        assert!(
            first.fill.is_some() && first.stroke.is_none(),
            "the bucket makes a fill-only shape under the outline"
        );
    }

    /// Clicking in an open (unclosed) outline fills nothing, unless a gap size
    /// bridges the opening.
    #[test]
    fn the_bucket_respects_gaps_until_the_gap_size_closes_them() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;

        // A C-shape: three sides of a box, leaving the right edge open.
        let mut path = BezPath::new();
        path.move_to(Point::new(80.0, 10.0));
        path.line_to(Point::new(10.0, 10.0));
        path.line_to(Point::new(10.0, 80.0));
        path.line_to(Point::new(80.0, 80.0));
        let outline = ShapeData {
            path,
            fill: None,
            stroke: Some(StrokeSpec {
                paint: Paint::Solid(Color::BLACK),
                width: 2.0,
                hairline: false,
                swatch: None,
            }),
            blend: buzz_scene::PaintBlend::default(),
        };
        e.apply(ToolAction::AddShape {
            shape: outline,
            label: "Draw",
        });
        let before = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .count();

        e.style.fill_enabled = true;
        e.style.gap_size = buzz_scene::GapSize::None;
        e.apply(ToolAction::BucketFill {
            point: Point::new(45.0, 45.0),
        });
        let after_open = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .count();
        assert_eq!(after_open, before, "an open outline must not fill");

        // The opening is 70 units — Extra Large (32) bridges up to 64, so widen
        // it to a gap the setting can close by using a genuinely small opening.
        // Redraw with a 12-unit gap and confirm Medium closes it.
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        let mut path = BezPath::new();
        path.move_to(Point::new(80.0, 10.0));
        path.line_to(Point::new(10.0, 10.0));
        path.line_to(Point::new(10.0, 80.0));
        path.line_to(Point::new(80.0, 80.0));
        path.line_to(Point::new(80.0, 52.0)); // stub up, leaving a ~12-unit gap
        let outline = ShapeData {
            path,
            fill: None,
            stroke: Some(StrokeSpec {
                paint: Paint::Solid(Color::BLACK),
                width: 2.0,
                hairline: false,
                swatch: None,
            }),
            blend: buzz_scene::PaintBlend::default(),
        };
        e.apply(ToolAction::AddShape {
            shape: outline,
            label: "Draw",
        });
        let before = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .count();
        e.style.fill_enabled = true;
        e.style.gap_size = buzz_scene::GapSize::ExtraLarge;
        e.apply(ToolAction::BucketFill {
            point: Point::new(45.0, 45.0),
        });
        let after_closed = e
            .scene()
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .count();
        assert_eq!(after_closed, before + 1, "the gap size should close the gap");
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
    fn the_text_tool_is_selectable() {
        // Text used to be refused as unavailable; it now places vector type, so
        // selecting it takes, like any other ready tool.
        let mut e = editor();
        e.set_tool(ToolId::Text);
        assert_eq!(e.tool(), ToolId::Text);
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
        assert!(
            e.playback.playing,
            "a looping section does not end playback"
        );
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
            e.status
                .as_deref()
                .unwrap_or_default()
                .contains("clipboard"),
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
        assert!(
            e.selection.active_layer().is_some(),
            "and has a layer to draw on"
        );
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
            path.line_to(Point::new(
                t * 9.0,
                (t * 0.9).sin() * 70.0 + (t * 2.7).cos() * 25.0,
            ));
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
            e.scene()
                .layers()
                .get(layer)
                .unwrap()
                .frames
                .keyframes()
                .len(),
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
            e.scene()
                .layers()
                .get(layer)
                .unwrap()
                .frames
                .keyframes()
                .len(),
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
            e.scene()
                .layers()
                .get(layer)
                .unwrap()
                .frames
                .keyframes()
                .len(),
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
        assert!(
            (after[2] - before[2]).abs() < 1e-9,
            "frame 10 must not move"
        );
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
            delta_screen: Vec2::new(50.0, 0.0),
        });

        let after = e.scene().camera().state_at(0).unwrap().center;
        assert!(
            after.x < before.x,
            "dragging right should move the camera left: {before:?} -> {after:?}"
        );
    }

    /// **Aiming the camera is steady.**
    ///
    /// Two things used to make it shake. The drag was measured from the
    /// pointer's *snapped* document position, so the shot jumped from artwork
    /// edge to artwork edge; and that position is measured *through the
    /// camera*, so each step was measured against a ruler the step before had
    /// already moved. Neither can reach a screen-pixel delta, and this holds
    /// the tool to that: an even sweep of the pointer must move the shot evenly.
    #[test]
    fn aiming_the_camera_is_steady_over_artwork() {
        let mut e = editor();
        e.style.drawing_mode = DrawingMode::ObjectDrawing;
        // Artwork all over the stage, so snapping has plenty to grab at.
        for i in 0..6 {
            draw_square(&mut e, i as f64 * 60.0, 40.0, 30.0, Color::WHITE);
        }
        // Turned on explicitly: the test editor starts from a saved workspace,
        // and this is the setting the jitter came from.
        e.view.snap.to_objects = true;

        e.run(Command::ToggleCamera);
        e.set_tool(buzz_ui::ToolId::Camera);

        // An even sweep across the stage, one pixel-sized step at a time.
        let start = Point::new(20.0, 60.0);
        e.pointer_down(start, Mods::default());
        let mut centres = Vec::new();
        for step in 1..=24 {
            let at = Point::new(start.x + step as f64 * 12.0, start.y);
            e.pointer_move(at, Mods::default());
            centres.push(e.scene().camera().state_at(0).unwrap().center.x);
        }
        e.pointer_up(Point::new(start.x + 24.0 * 12.0, start.y));

        // Every step moved the shot by the same amount. Jitter shows up here
        // as one step disagreeing with its neighbours.
        let steps: Vec<f64> = centres.windows(2).map(|w| w[1] - w[0]).collect();
        let first = steps[0];
        assert!(first.abs() > 0.0, "the shot should move at all");
        for (i, step) in steps.iter().enumerate() {
            assert!(
                (step - first).abs() < 1e-9,
                "step {i} moved the shot by {step} where every other step moved it {first}"
            );
        }
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
            e.selection.bounds_at(e.scene(), e.current_frame).is_some(),
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
        assert!(
            e.selection.is_empty(),
            "the Selection tool selected the layer"
        );
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
        for (x, y, size) in [
            (260.0, 150.0, 40.0),
            (200.0, 270.0, 30.0),
            (240.0, 210.0, 70.0),
        ] {
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

    /// **A lamp arrives in the view and off to one side.**
    ///
    /// In the view, because the origin is off the top-left of the stage and a
    /// lamp dropped there looks exactly like nothing having happened. Off to one
    /// side, because dead centre is the one place a lamp does nothing you can
    /// see: everything that reads as *light* — the shaded crescent, the
    /// highlight, the cast shadow — comes from the direction the lamp lies in,
    /// and a lamp over the middle of what it lights has no direction at all.
    #[test]
    fn a_lamp_arrives_in_the_view_and_off_to_one_side() {
        let mut e = editor();
        e.camera.center = Point::new(640.0, 360.0);
        let seen = e.camera.visible_doc_rect();
        e.run(Command::AddLamp);

        match e.scene().lights().lights[0].kind {
            buzz_scene::LightKind::Lamp { position, .. } => {
                assert!(
                    seen.contains(position),
                    "the lamp landed outside the view: {position:?} in {seen:?}"
                );
                assert!(
                    position.x < seen.center().x && position.y < seen.center().y,
                    "a new lamp belongs up and to one side, where a key light \
                     goes — not on top of what it is lighting: {position:?}"
                );
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

    /// **The brush preview is drawn where the brush will paint.**
    ///
    /// A tool receives points carried back through the document camera and
    /// through the place of any symbol opened for editing; the preview is built
    /// from those points and used to be drawn straight into document space, so
    /// with a shot framed off centre the ink appeared a fixed distance from the
    /// pointer and jumped back the moment the stroke was committed. The fix is
    /// one transform, and this is it: carrying a tool-space point forward has to
    /// undo exactly what `screen_to_edit` did to it.
    #[test]
    fn a_preview_maps_back_onto_the_stage_it_was_taken_from() {
        let mut e = Editor::default();

        // A shot framed well off centre — the case where the old code drew the
        // preview a couple of hundred units from the pointer.
        e.doc.edit("Camera", |scene| {
            let stage = scene.stage().size;
            scene.camera_mut().enabled = true;
            scene.camera_mut().set_key(buzz_scene::CameraKey::new(
                0,
                Point::new(stage.width / 2.0 + 200.0, stage.height / 2.0 - 60.0),
            ));
        });
        e.doc.end_gesture();

        let onto_stage = e.edit_to_stage();
        for screen in [
            Point::new(12.0, 34.0),
            Point::new(400.0, 220.0),
            Point::new(-80.0, 500.0),
        ] {
            let in_tool_space = e.screen_to_edit(screen);
            let back = onto_stage
                .map_point(in_tool_space)
                .expect("the point is in front of the lens");
            let straight = e.camera.screen_to_doc(screen);
            assert!(
                (back - straight).hypot() < 1e-6,
                "a tool-space point must come back to where the pointer was:                  {back:?} against {straight:?}"
            );
        }
    }

    /// **A drawn line is not a placed one, so it does not snap.**
    ///
    /// Object snapping is on by default and reaches eight screen pixels. Applied
    /// to a brush stroke it pulls every sample that passes near existing artwork
    /// onto that artwork's bounding box, which puts flats and steps into a
    /// curve the hand drew smoothly.
    #[test]
    fn a_freehand_stroke_is_never_snapped() {
        let mut e = Editor::default();
        // A square to snap to, and snapping left at its default (objects on).
        draw_square(&mut e, 0.0, 0.0, 100.0, Color::WHITE).unwrap();
        assert!(e.view.snap.to_objects, "the default this test is about");

        // Just inside the snap radius of the square's right-hand edge.
        let near_the_edge = Point::new(103.0, 50.0);
        let screen = e.camera.doc_to_screen(near_the_edge);

        // Compared with a tolerance because the point has been through the
        // view and back; what is being tested is that snapping moved it, and
        // snapping moves things by whole pixels.
        e.set_tool(ToolId::Brush);
        let drawn = e.snap_for_tool(e.screen_to_edit(screen));
        assert!(
            (drawn - near_the_edge).hypot() < 1e-6,
            "the brush draws where the hand went, got {drawn:?}"
        );

        // The pen places anchors deliberately, so it still snaps.
        e.set_tool(ToolId::Pen);
        let placed = e.snap_for_tool(e.screen_to_edit(screen));
        assert!(
            (placed.x - 100.0).abs() < 1e-6,
            "a placed point still snaps to the edge it is beside, got {placed:?}"
        );
    }


    /// **An imported sound is a sound the film carries.**
    ///
    /// It used to land in the library and nowhere else, so it was silent on
    /// playback and \u2014 the report \u2014 missing from the exported video, because
    /// what the exporter writes is `Scene::stage_cues`: sounds attached to
    /// keyframes on the document's own timeline. An empty cue list is an export
    /// with no audio in it, and nothing anywhere says why.
    #[test]
    fn an_imported_sound_lands_on_the_timeline_the_export_reads() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Dialogue.wav");
        std::fs::write(&path, test_wav(1.0)).expect("wrote the tone");

        let mut e = Editor::default();
        assert!(
            e.scene().stage_cues().is_empty(),
            "nothing is cued before the import"
        );

        e.import_sound(&path).expect("the sound imports");

        let cues = e.scene().stage_cues();
        assert_eq!(cues.len(), 1, "the import put it where the film can see it");
        assert_eq!(cues[0].start_frame, 0, "from the first frame");

        // And on the document's timeline rather than inside anything, which is
        // the only place `stage_cues` looks.
        assert!(
            e.scene()
                .stage_layers()
                .iter()
                .any(|l| l.name == "Dialogue"),
            "on a layer named after the sound"
        );

        // Long enough to hold the whole second of audio at the default rate.
        let fps = e.scene().stage().frame_rate;
        assert!(
            e.scene().frame_count() >= (fps.round() as u32),
            "the layer spans the sound, so the default export range reaches its end"
        );

        // One undo takes the import and the placement together.
        e.doc.undo();
        assert!(
            e.scene().stage_cues().is_empty() && e.scene().sounds().iter().count() == 0,
            "the import is one step in the history"
        );
    }

    /// A sound attached while a symbol is open would be neither heard nor
    /// exported, so it is refused with the reason rather than silently lost.
    #[test]
    fn sound_is_not_attached_inside_a_symbol() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Line.wav");
        std::fs::write(&path, test_wav(0.2)).expect("wrote the tone");

        let mut e = Editor::default();
        e.import_sound(&path).expect("the sound imports");

        let symbol = e.doc.scene().library().iter().next().map(|s| s.id);
        let symbol = match symbol {
            Some(id) => id,
            None => {
                let mut made = None;
                e.doc.edit("Symbol", |scene| {
                    made = Some(scene.add_symbol(
                        "Head",
                        buzz_scene::SymbolKind::Graphic,
                        None,
                    ));
                });
                e.doc.end_gesture();
                made.expect("a symbol")
            }
        };
        e.doc.edit("Enter", |scene| {
            scene.enter_symbol(symbol);
        });
        e.doc.end_gesture();

        let before = e.scene().stage_cues().len();
        e.attach_sound_to_frame();
        assert_eq!(
            e.scene().stage_cues().len(),
            before,
            "nothing was attached where nothing could be heard"
        );
        let said = e.status.clone().unwrap_or_default();
        assert!(
            said.contains("main timeline"),
            "and the reason was given, got {said:?}"
        );
    }

    /// A WAV of a 440 Hz tone, written rather than committed: a fixture would be
    /// a binary blob for a test that needs "some sound, of a known length".
    fn test_wav(seconds: f64) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut out, spec).expect("writer");
            for i in 0..(seconds * 44_100.0) as usize {
                let t = i as f64 / 44_100.0;
                let v = (t * 440.0 * std::f64::consts::TAU).sin() * 0.6;
                writer.write_sample((v * 32_000.0) as i16).expect("sample");
            }
            writer.finalize().expect("finalize");
        }
        out.into_inner()
    }

}


