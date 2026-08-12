//! Document persistence, undo history and autosave.
//!
//! [`Document`] is what the editor actually holds: the current [`Scene`], the
//! undo [`History`], where the file lives, and the [`Autosave`] schedule. Every
//! mutation goes through [`Document::edit`], so no path can change the document
//! without also recording an undo step — the usual way undo quietly develops
//! holes.

pub mod autosave;
pub mod format;
pub mod history;
pub mod serial;

use std::path::{Path, PathBuf};

use buzz_scene::Scene;

pub use autosave::{Autosave, AutosavePlan, Recovery, find_recoveries};
pub use format::{DocError, EXTENSION, MIMETYPE, Meta};
pub use history::{History, UndoLabel};
pub use serial::{FORMAT_VERSION, SerialError};

/// An open document.
pub struct Document {
    scene: Scene,
    history: History,
    path: Option<PathBuf>,
    autosave: Autosave,
}

impl Default for Document {
    fn default() -> Self {
        Self::new(Scene::default())
    }
}

impl Document {
    /// A new, unsaved document.
    pub fn new(scene: Scene) -> Self {
        let autosave = Autosave::untitled(std::env::temp_dir());
        Self {
            scene,
            history: History::default(),
            path: None,
            autosave,
        }
    }

    /// Open a document from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocError> {
        let path = path.as_ref().to_path_buf();
        let scene = format::load(&path)?;
        let mut history = History::default();
        history.mark_saved(scene.revision());

        Ok(Self {
            autosave: Autosave::beside(&path),
            scene,
            history,
            path: Some(path),
        })
    }

    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Has the document changed since it was last saved?
    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty(self.scene.revision())
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
        let before = self.scene.clone();
        f(&mut self.scene);

        // Nothing actually changed, so there is nothing to undo.
        if self.scene.revision() == before.revision() {
            return;
        }
        self.history.record(before, label);
    }

    /// End the current gesture so the next edit starts a fresh undo step.
    pub fn end_gesture(&mut self) {
        self.history.break_coalescing();
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo(&mut self) -> bool {
        match self.history.undo(self.scene.clone()) {
            Some(scene) => {
                self.scene = scene;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.history.redo(self.scene.clone()) {
            Some(scene) => {
                self.scene = scene;
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
        format::save(&self.scene, &path)?;

        self.history.mark_saved(self.scene.revision());
        self.autosave.retarget(&path);
        // The recovery copy is now redundant, and leaving it would prompt an
        // unnecessary recovery on next launch.
        let _ = self.autosave.discard_recovery();
        self.path = Some(path);
        Ok(())
    }

    /// Build an autosave job if one is due.
    ///
    /// Returns a `Send` plan rather than writing here, so the caller can hand
    /// it to the background pool and keep editing.
    pub fn autosave_plan(&mut self) -> Option<AutosavePlan> {
        self.autosave.plan(&self.scene)
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
        assert!(!reopened.can_undo(), "a freshly opened document has no history");
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
        assert!(doc.save().is_err(), "an unsaved document has nowhere to save");
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
