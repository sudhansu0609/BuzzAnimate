//! Document persistence, undo history and autosave.
//!
//! [`Document`] is what the editor actually holds: the current [`Scene`], the
//! undo [`History`], where the file lives, and the [`Autosave`] schedule. Every
//! mutation goes through [`Document::edit`], so no path can change the document
//! without also recording an undo step — the usual way undo quietly develops
//! holes.

pub mod assets;
pub mod autosave;
pub mod format;
pub mod history;
pub mod project;
pub mod serial;
pub mod templates;

use std::path::{Path, PathBuf};

use buzz_scene::Scene;

pub use assets::{Asset, AssetLibrary};
pub use autosave::{Autosave, AutosavePlan, Recovery, find_recoveries};
pub use format::{DocError, EXTENSION, MIMETYPE, Meta};
pub use history::{History, UndoLabel};
pub use project::{FilmError, PROJECT_EXTENSION, PROJECT_VERSION, Project, Shot};
pub use serial::{FORMAT_VERSION, SerialError};
pub use templates::{Template, TemplateLibrary};

/// An open document.
/// One scene of a document: an independent timeline with its own name and undo
/// history. A document has at least one, and edits, undo and view all address
/// whichever is [`Document::active_scene`].
struct SceneSlot {
    name: String,
    scene: Scene,
    history: History,
}

pub struct Document {
    /// Never empty — a document always has at least one scene.
    scenes: Vec<SceneSlot>,
    active: usize,
    path: Option<PathBuf>,
    autosave: Autosave,
}

impl Default for Document {
    fn default() -> Self {
        Self::new(Scene::default())
    }
}

impl Document {
    /// A new, unsaved document with a single scene.
    pub fn new(scene: Scene) -> Self {
        // Work that has never been saved recovers from the application's own
        // directory rather than the system temp, which is swept.
        let autosave = Autosave::untitled(crate::autosave::recovery_dir());
        Self {
            scenes: vec![SceneSlot {
                name: "Scene 1".to_string(),
                scene,
                history: History::default(),
            }],
            active: 0,
            path: None,
            autosave,
        }
    }

    /// A document from several named scenes, the first active.
    pub fn from_scenes(scenes: Vec<(String, Scene)>) -> Self {
        let mut doc = Self::new(Scene::default());
        if scenes.is_empty() {
            return doc;
        }
        doc.scenes = scenes
            .into_iter()
            .map(|(name, scene)| {
                let mut history = History::default();
                history.mark_saved(scene.revision());
                SceneSlot {
                    name,
                    scene,
                    history,
                }
            })
            .collect();
        doc.active = 0;
        doc
    }

    /// Open a document from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocError> {
        let path = path.as_ref().to_path_buf();
        let (scenes, active) = format::load_scenes(&path)?;
        let mut doc = Self::from_scenes(scenes);
        doc.switch_scene(active);
        doc.path = Some(path.clone());
        doc.autosave = Autosave::beside(&path);
        Ok(doc)
    }

    fn active_slot(&self) -> &SceneSlot {
        &self.scenes[self.active]
    }

    fn active_slot_mut(&mut self) -> &mut SceneSlot {
        &mut self.scenes[self.active]
    }

    pub fn scene(&self) -> &Scene {
        &self.active_slot().scene
    }

    pub fn history(&self) -> &History {
        &self.active_slot().history
    }

    /// The names of every scene, in order.
    pub fn scene_names(&self) -> Vec<String> {
        self.scenes.iter().map(|s| s.name.clone()).collect()
    }

    /// Which scene is being edited.
    pub fn active_scene(&self) -> usize {
        self.active
    }

    /// **Every scene, in the order the film plays them.**
    ///
    /// Snapshots, for an export to take to its own thread. Copy-on-write, so
    /// this is a handful of pointer copies rather than a copy of the artwork.
    pub fn film(&self) -> Vec<Scene> {
        self.scenes.iter().map(|s| s.scene.clone()).collect()
    }

    /// How long the whole film is, in frames — every scene end to end, each
    /// counting its own looping section.
    ///
    /// What the export dialog's range runs over, so "all of it" means all of
    /// the film rather than all of whichever scene happened to be open.
    pub fn film_frames(&self) -> u32 {
        self.scenes
            .iter()
            .map(|s| s.scene.rendered_frame_count())
            .sum()
    }

    /// The first film frame of the scene at `index`.
    ///
    /// What turns "the frame I am looking at" into "the frame of the film I am
    /// looking at" — which is what an exported still, and a range typed
    /// against the film, both need.
    pub fn film_start_of(&self, index: usize) -> u32 {
        self.scenes
            .iter()
            .take(index)
            .map(|s| s.scene.rendered_frame_count())
            .sum()
    }

    /// Add a new empty scene after the active one and switch to it. The stage
    /// size, frame rate and background carry over, as a new scene in Animate
    /// inherits the document's, so the shots match without setting them again.
    pub fn add_scene(&mut self) {
        let mut scene = Scene::default();
        *scene.stage_mut() = *self.scene().stage();
        let name = self.unique_scene_name("Scene");
        self.active += 1;
        self.scenes.insert(
            self.active,
            SceneSlot {
                name,
                scene,
                history: History::default(),
            },
        );
    }

    /// **Duplicate a scene**, whole, and switch to the copy.
    ///
    /// The set, the cast, the lights, the camera track and every keyframe come
    /// with it. This is the operation a conversation is built out of: two
    /// people standing in a room is the *same* scene beat after beat, and only
    /// the performance differs — so the second shot starts as a copy of the
    /// first and is then changed, rather than being staged again or pasted
    /// frame by frame onto the end of the one before it.
    ///
    /// The copy gets its **own history**, not the original's. Sharing one
    /// would mean undoing in the new scene reaching back into the old one's
    /// past and quietly rewriting a shot that was already finished.
    pub fn duplicate_scene(&mut self, index: usize) {
        let Some(source) = self.scenes.get(index) else {
            return;
        };
        // Named after what it came from, so a duplicated "Kitchen" is
        // "Kitchen 2" rather than "Scene 4" — the name is how a director finds
        // a shot in a list of twenty.
        let stem = source
            .name
            .trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace());
        let stem = if stem.is_empty() { "Scene" } else { stem };
        let name = self.unique_scene_name(stem);
        let scene = source.scene.clone();

        self.active = index + 1;
        self.scenes.insert(
            self.active,
            SceneSlot {
                name,
                scene,
                history: History::default(),
            },
        );
    }

    /// Move a scene to another position, and follow it.
    ///
    /// Scenes are an *ordered* list — they are the order the film plays in —
    /// so being able to say "this beat comes before that one" is part of
    /// having them at all.
    pub fn move_scene(&mut self, from: usize, to: usize) {
        if from >= self.scenes.len() || to >= self.scenes.len() || from == to {
            return;
        }
        let slot = self.scenes.remove(from);
        self.scenes.insert(to, slot);
        self.active = to;
    }

    /// A scene name not already taken, e.g. `Scene 3`.
    ///
    /// Numbering starts at two when the bare stem is itself a scene, because
    /// a scene called "Kitchen" *is* the first Kitchen: the copy beside it
    /// should be "Kitchen 2", not "Kitchen 1" sitting after it.
    fn unique_scene_name(&self, stem: &str) -> String {
        let taken = |name: &str| self.scenes.iter().any(|s| s.name == name);
        let first = if taken(stem) { 2 } else { 1 };
        for n in first.. {
            let candidate = format!("{stem} {n}");
            if !taken(&candidate) {
                return candidate;
            }
        }
        unreachable!()
    }

    /// Switch which scene is being edited. Out-of-range indices are ignored.
    pub fn switch_scene(&mut self, index: usize) {
        if index < self.scenes.len() {
            self.active = index;
        }
    }

    /// Rename a scene. A blank name, or one already taken, is refused.
    pub fn rename_scene(&mut self, index: usize, name: impl Into<String>) {
        let name = name.into();
        let taken = self
            .scenes
            .iter()
            .enumerate()
            .any(|(i, s)| i != index && s.name == name);
        if index < self.scenes.len() && !name.trim().is_empty() && !taken {
            self.scenes[index].name = name;
        }
    }

    /// Delete a scene. The last remaining scene cannot be deleted — a document
    /// always has one.
    pub fn delete_scene(&mut self, index: usize) {
        if self.scenes.len() <= 1 || index >= self.scenes.len() {
            return;
        }
        self.scenes.remove(index);
        self.active = self.active.min(self.scenes.len() - 1);
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Has the document changed since it was last saved?
    /// Treat the document as it stands as unmodified.
    ///
    /// For a document that was *made* rather than opened: a new file is not a
    /// changed one, and there is nothing to save yet. Without this a brand new
    /// document reports unsaved changes from the moment it appears, because
    /// building a scene bumps its revision.
    pub fn mark_clean(&mut self) {
        for slot in &mut self.scenes {
            let revision = slot.scene.revision();
            slot.history.mark_saved(revision);
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.scenes
            .iter()
            .any(|s| s.history.is_dirty(s.scene.revision()))
    }

    /// Name for the title bar, with Animate's asterisk for unsaved changes.
    pub fn display_name(&self) -> String {
        let base = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        if self.is_dirty() {
            format!("{base}*")
        } else {
            base
        }
    }

    /// Apply an edit, recording it for undo.
    ///
    /// The single entry point for mutation. `label` names the step in the
    /// History panel and drives coalescing: consecutive edits sharing a label
    /// collapse into one undo step, so a drag is one Ctrl+Z rather than two
    /// hundred.
    pub fn edit(&mut self, label: impl Into<UndoLabel>, f: impl FnOnce(&mut Scene)) {
        let slot = &mut self.scenes[self.active];
        let before = slot.scene.clone();
        f(&mut slot.scene);

        // Nothing actually changed, so there is nothing to undo.
        if slot.scene.revision() == before.revision() {
            return;
        }
        slot.history.record(before, label);
    }

    /// Change view state on the scene without recording an undo step.
    ///
    /// Only one thing qualifies: which symbol is open for editing. It lives on
    /// [`Scene`] so that every panel and tool sees the same answer, but opening
    /// a symbol is navigation rather than an edit — it must not mark the
    /// document dirty, and Ctrl+Z must not take you back out of it.
    ///
    /// Nothing here may touch the artwork. `Scene`'s navigation methods do not
    /// bump the revision, so an accidental artwork change made through this
    /// path would go unrecorded; a debug assertion catches that.
    pub fn edit_view(&mut self, f: impl FnOnce(&mut Scene)) {
        let slot = &mut self.scenes[self.active];
        let before = slot.scene.revision();
        f(&mut slot.scene);
        debug_assert_eq!(
            before,
            slot.scene.revision(),
            "edit_view changed the document; use edit() so the change can be undone"
        );
    }

    /// End the current gesture so the next edit starts a fresh undo step.
    pub fn end_gesture(&mut self) {
        self.active_slot_mut().history.break_coalescing();
    }

    pub fn can_undo(&self) -> bool {
        self.active_slot().history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.active_slot().history.can_redo()
    }

    pub fn undo(&mut self) -> bool {
        let slot = &mut self.scenes[self.active];
        match slot.history.undo(slot.scene.clone()) {
            Some(scene) => {
                slot.scene = scene;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        let slot = &mut self.scenes[self.active];
        match slot.history.redo(slot.scene.clone()) {
            Some(scene) => {
                slot.scene = scene;
                true
            }
            None => false,
        }
    }

    /// Save to the current path. Fails if there is none — the caller should
    /// present Save As instead.
    pub fn save(&mut self) -> Result<(), DocError> {
        let Some(path) = self.path.clone() else {
            return Err(DocError::Io(std::io::Error::other(
                "this document has never been saved; use Save As",
            )));
        };
        self.save_as(path)
    }

    /// Save to `path` and adopt it as the document's location.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), DocError> {
        let path = path.as_ref().to_path_buf();
        {
            let refs: Vec<(&str, &Scene)> = self
                .scenes
                .iter()
                .map(|s| (s.name.as_str(), &s.scene))
                .collect();
            format::save_scenes(&refs, self.active, &path)?;
        }

        self.mark_clean();
        self.autosave.retarget(&path);
        // The recovery copy is now redundant, and leaving it would prompt an
        // unnecessary recovery on next launch.
        let _ = self.autosave.discard_recovery();
        self.path = Some(path);
        Ok(())
    }

    /// Forget where this document came from: it becomes untitled.
    ///
    /// For a recovered autosave. The recovery file is *evidence of a crash*,
    /// not a document the user chose to keep, and Save must ask where to put
    /// the result rather than writing back over it — and the new document's
    /// own autosave must go somewhere else, or it would overwrite the very
    /// file it was recovered from.
    pub fn forget_path(&mut self) {
        self.path = None;
        self.autosave
            .reset_to_untitled(crate::autosave::recovery_dir());
    }

    /// Build an autosave job if one is due.
    ///
    /// Returns a `Send` plan rather than writing here, so the caller can hand
    /// it to the background pool and keep editing.
    pub fn autosave_plan(&mut self) -> Option<AutosavePlan> {
        let revision = self.combined_revision();
        let refs: Vec<(&str, &Scene)> = self
            .scenes
            .iter()
            .map(|s| (s.name.as_str(), &s.scene))
            .collect();
        self.autosave.plan_scenes(&refs, self.active, revision)
    }

    /// A single fingerprint over every scene, so any edit anywhere makes an
    /// autosave due and an untouched document stays quiet.
    pub fn combined_revision(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.scenes.len().hash(&mut hasher);
        for slot in &self.scenes {
            slot.scene.revision().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Remember the whole document for crash recovery. A crash then restores
    /// every scene, not only the one on screen.
    pub fn remember_for_crash(&self) {
        let refs: Vec<(&str, &Scene)> = self
            .scenes
            .iter()
            .map(|s| (s.name.as_str(), &s.scene))
            .collect();
        crate::autosave::remember_scenes_for_crash(
            &refs,
            self.active,
            self.combined_revision(),
            self.recovery_path(),
        );
    }

    /// Where this document's recovery file would be written.
    pub fn recovery_path(&self) -> PathBuf {
        self.autosave.recovery_path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{LayerKind, ShapeData};
    use kurbo::Rect;
    use peniko::Color;

    fn add_shape(scene: &mut Scene) {
        let layer = scene
            .layers()
            .iter()
            .next()
            .map(|l| l.id)
            .expect("a document always has a layer");
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
        );
    }

    #[test]
    fn a_new_document_is_clean_and_named_untitled() {
        let doc = Document::default();
        assert!(!doc.can_undo());
        assert_eq!(doc.display_name(), "Untitled*");
        assert!(doc.is_dirty(), "an unsaved document counts as dirty");
    }

    #[test]
    fn editing_records_an_undo_step() {
        let mut doc = Document::default();
        doc.edit("Draw", add_shape);

        assert!(doc.can_undo());
        assert_eq!(doc.history().next_undo_label(), Some("Draw"));
        assert_eq!(doc.scene().shape_count(), 1);

        assert!(doc.undo());
        assert_eq!(doc.scene().shape_count(), 0);
        assert!(doc.redo());
        assert_eq!(doc.scene().shape_count(), 1);
    }

    /// **A document keeps several scenes**, each with its own artwork, undo
    /// history and name — and switching between them is not an edit.
    #[test]
    fn scenes_are_independent_and_switching_is_not_an_edit() {
        let mut doc = Document::default();
        doc.edit("Draw", add_shape); // one shape on Scene 1
        assert_eq!(doc.scene_names(), vec!["Scene 1"]);

        doc.add_scene(); // now on Scene 2, empty
        assert_eq!(doc.active_scene(), 1);
        assert_eq!(doc.scene_names(), vec!["Scene 1", "Scene 2"]);
        assert_eq!(doc.scene().shape_count(), 0, "a new scene starts empty");

        doc.edit("Draw", add_shape);
        doc.edit("Draw", add_shape); // two shapes on Scene 2

        // Each scene has its own undo stack.
        assert!(doc.can_undo());
        doc.switch_scene(0);
        assert_eq!(doc.scene().shape_count(), 1, "Scene 1 is untouched");

        doc.switch_scene(1);
        assert_eq!(doc.scene().shape_count(), 2, "Scene 2 kept its own artwork");
    }

    /// Every scene survives a save and reopen, in order, with its name.
    #[test]
    fn scenes_round_trip_through_a_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("film.buzz");

        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.add_scene();
        doc.rename_scene(1, "Chase");
        doc.edit("Draw", add_shape);
        doc.edit("Draw", add_shape);
        doc.save_as(&path).unwrap();

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene_names(), vec!["Scene 1", "Chase"]);
        assert_eq!(reopened.active_scene(), 1, "the active scene is remembered");
        assert_eq!(reopened.scene().shape_count(), 2);
    }

    /// **Duplicating a scene copies the whole thing.** The operation a
    /// conversation is built out of: the next beat is the same room and the
    /// same two people, so it starts as a copy rather than as an empty stage
    /// or as frames pasted onto the end of the shot before it.
    #[test]
    fn duplicating_a_scene_copies_all_of_it_and_opens_the_copy() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Kitchen");
        doc.edit("Draw", add_shape);
        doc.edit("Draw", add_shape);

        doc.duplicate_scene(0);

        assert_eq!(doc.active_scene(), 1, "the copy is opened for editing");
        assert_eq!(
            doc.scene_names(),
            vec!["Kitchen", "Kitchen 2"],
            "and is named after what it came from, next to it in the order"
        );
        assert_eq!(
            doc.scene().shape_count(),
            2,
            "the copy carries the whole set, not an empty stage"
        );

        // The two are now separate: changing the copy leaves the original.
        doc.edit("Draw", add_shape);
        assert_eq!(doc.scene().shape_count(), 3);
        doc.switch_scene(0);
        assert_eq!(doc.scene().shape_count(), 2, "the original is untouched");
    }

    /// The copy gets its **own** history. Sharing one would let an undo in the
    /// new shot reach back and rewrite the shot it was copied from.
    #[test]
    fn a_duplicated_scene_has_its_own_undo_history() {
        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.duplicate_scene(0);

        // Fresh history: there is nothing of the original's past to undo.
        assert!(
            !doc.can_undo(),
            "the copy starts with a clean history of its own"
        );

        doc.edit("Draw", add_shape);
        assert!(doc.undo(), "and undoes its own edits");
        assert_eq!(doc.scene().shape_count(), 1);
        doc.switch_scene(0);
        assert_eq!(doc.scene().shape_count(), 1, "the original never moved");
    }

    /// Scenes are the order the film plays in, so that order has to be
    /// changeable.
    #[test]
    fn scenes_can_be_reordered() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Opening");
        doc.add_scene();
        doc.rename_scene(1, "Chase");
        doc.add_scene();
        doc.rename_scene(2, "Ending");
        assert_eq!(doc.scene_names(), vec!["Opening", "Chase", "Ending"]);

        doc.move_scene(2, 0);
        assert_eq!(doc.scene_names(), vec!["Ending", "Opening", "Chase"]);
        assert_eq!(doc.active_scene(), 0, "the moved scene is followed");

        // Nonsense is refused rather than panicking or reshuffling.
        doc.move_scene(0, 9);
        doc.move_scene(9, 0);
        doc.move_scene(1, 1);
        assert_eq!(doc.scene_names(), vec!["Ending", "Opening", "Chase"]);
    }

    /// Duplicated scenes survive the file, in order, like any other.
    #[test]
    fn a_duplicated_scene_round_trips_through_a_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conversation.buzz");

        let mut doc = Document::default();
        doc.rename_scene(0, "Kitchen");
        doc.edit("Draw", add_shape);
        doc.duplicate_scene(0);
        doc.edit("Draw", add_shape);
        doc.save_as(&path).unwrap();

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene_names(), vec!["Kitchen", "Kitchen 2"]);
        assert_eq!(reopened.scene().shape_count(), 2);
    }

    /// The last scene cannot be deleted — a document always has one.
    #[test]
    fn the_final_scene_cannot_be_deleted() {
        let mut doc = Document::default();
        doc.delete_scene(0);
        assert_eq!(doc.scene_names().len(), 1, "a document is never scene-less");
    }

    /// An edit that changes nothing should not clutter the History panel.
    #[test]
    fn a_no_op_edit_records_nothing() {
        let mut doc = Document::default();
        doc.edit("Nothing", |_scene| {});
        assert!(
            !doc.can_undo(),
            "an edit that changed nothing must not create an undo step"
        );
    }

    #[test]
    fn a_drag_collapses_into_a_single_undo_step() {
        let mut doc = Document::default();
        for _ in 0..50 {
            doc.edit("Move", add_shape);
        }
        assert_eq!(doc.history().undo_depth(), 1);

        doc.end_gesture();
        doc.edit("Move", add_shape);
        assert_eq!(doc.history().undo_depth(), 2, "a new gesture is separate");
    }

    #[test]
    fn saving_and_reopening_preserves_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.edit("Add layer", |s| {
            s.add_layer("Second", LayerKind::Normal);
        });
        doc.save_as(&path).unwrap();

        assert!(!doc.is_dirty(), "saving should clear the dirty flag");
        assert_eq!(doc.display_name(), "drawing.buzz");

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene().shape_count(), doc.scene().shape_count());
        assert_eq!(reopened.scene().layers().len(), doc.scene().layers().len());
        assert!(!reopened.is_dirty());
        assert!(
            !reopened.can_undo(),
            "a freshly opened document has no history"
        );
    }

    #[test]
    fn editing_after_saving_marks_the_document_dirty_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.save_as(&path).unwrap();
        assert!(!doc.is_dirty());

        doc.edit("Draw", add_shape);
        assert!(doc.is_dirty());
        assert!(doc.display_name().ends_with('*'));

        // And undoing back to the saved state should read clean again.
        doc.undo();
        assert!(!doc.is_dirty(), "undo back to the saved revision is clean");
    }

    #[test]
    fn saving_without_a_path_asks_for_save_as() {
        let mut doc = Document::default();
        assert!(
            doc.save().is_err(),
            "an unsaved document has nowhere to save"
        );
    }

    #[test]
    fn save_then_save_again_uses_the_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.save_as(&path).unwrap();
        doc.edit("Draw", add_shape);
        doc.save().unwrap();

        assert_eq!(Document::open(&path).unwrap().scene().shape_count(), 1);
    }

    #[test]
    fn a_manual_save_clears_the_recovery_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.save_as(&path).unwrap();
        doc.edit("Draw", add_shape);

        // Force an autosave.
        if let Some(plan) = doc.autosave_plan() {
            plan.write().unwrap();
        }
        doc.save().unwrap();

        assert!(
            find_recoveries(dir.path()).is_empty(),
            "saving should remove the now-redundant recovery file"
        );
    }

    #[test]
    fn a_full_edit_save_crash_recover_cycle_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        // Work, save, then work more without saving.
        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.save_as(&path).unwrap();
        for _ in 0..3 {
            doc.end_gesture();
            doc.edit("Draw", add_shape);
        }
        assert_eq!(doc.scene().shape_count(), 4);

        // Autosave catches the unsaved work, then the process "crashes".
        let plan = doc.autosave_plan().expect("a save should be due");
        plan.write().unwrap();
        drop(doc);

        // On restart the recovery is offered and holds the later work, while
        // the saved document still holds only what was explicitly saved.
        let recoveries = find_recoveries(dir.path());
        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].document.as_deref(), Some(path.as_path()));

        assert_eq!(format::load(&recoveries[0].path).unwrap().shape_count(), 4);
        assert_eq!(Document::open(&path).unwrap().scene().shape_count(), 1);
    }
}
