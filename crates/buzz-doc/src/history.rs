//! Undo and redo.
//!
//! # The mechanism is free; the policy is the work
//!
//! [`buzz_scene::Scene`] is an immutable snapshot, so "undo" is just holding on
//! to an earlier one. There are no inverse operations to write and none to get
//! wrong — the usual source of undo bugs simply does not exist here.
//!
//! What does need care is the *policy*:
//!
//! * **Coalescing.** Dragging an object emits an edit per mouse move. Without
//!   coalescing, one drag becomes two hundred undo steps and Ctrl+Z becomes
//!   useless. Consecutive edits sharing a [`UndoLabel`] merge while they keep
//!   arriving within [`COALESCE_WINDOW`].
//! * **Labelling.** Animate's History panel names each step ("Move", "Delete",
//!   "Draw Rectangle"), so entries carry a label rather than a bare snapshot.
//! * **Memory.** Snapshots share structure, so they are cheap — but not free,
//!   and an unbounded stack in a long session is a leak. The depth limit
//!   matches Animate's default of 100.

use std::time::{Duration, Instant};

use buzz_scene::Scene;

/// Animate's default undo depth.
pub const DEFAULT_DEPTH: usize = 100;

/// Consecutive same-labelled edits merge while they arrive this quickly.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(600);

/// What an edit was, for the History panel and for coalescing.
///
/// Two edits merge only if their labels are equal, so anything that should
/// stay separately undoable needs a distinct label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoLabel(pub String);

impl UndoLabel {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for UndoLabel {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for UndoLabel {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// One undoable step: the document *before* the edit, and what the edit was.
#[derive(Debug, Clone)]
struct Step {
    scene: Scene,
    label: UndoLabel,
    at: Instant,
}

/// Undo and redo stacks.
#[derive(Debug, Clone)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    depth: usize,
    /// Set when the current state matches what is on disk.
    saved_revision: Option<u64>,
}

impl Default for History {
    fn default() -> Self {
        Self::with_depth(DEFAULT_DEPTH)
    }
}

impl History {
    pub fn with_depth(depth: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            depth: depth.max(1),
            saved_revision: None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Label of the step Ctrl+Z would reverse, for the menu item.
    pub fn next_undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn next_redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// Every step, oldest first, for the History panel.
    pub fn labels(&self) -> Vec<&str> {
        self.undo.iter().map(|s| s.label.as_str()).collect()
    }

    /// Record the state *before* an edit.
    ///
    /// Call this with the pre-edit snapshot, then apply the edit. Recording
    /// beforehand is what lets undo restore exactly what the user had.
    pub fn record(&mut self, before: Scene, label: impl Into<UndoLabel>) {
        let label = label.into();
        let now = Instant::now();

        // Coalesce a continuing gesture: keep the *earliest* pre-edit state so
        // one Ctrl+Z reverses the whole drag rather than one mouse move of it.
        if let Some(last) = self.undo.last()
            && last.label == label
            && now.duration_since(last.at) <= COALESCE_WINDOW
        {
            if let Some(last) = self.undo.last_mut() {
                last.at = now;
            }
            self.redo.clear();
            return;
        }

        self.undo.push(Step {
            scene: before,
            label,
            at: now,
        });

        // Any new edit invalidates the redo branch, as everywhere else.
        self.redo.clear();

        if self.undo.len() > self.depth {
            // Drop the oldest. `remove(0)` is O(n) but n is 100 and this
            // happens once per edit past the limit.
            self.undo.remove(0);
        }
    }

    /// Step back. Pass the current scene; receive the one to restore.
    pub fn undo(&mut self, current: Scene) -> Option<Scene> {
        let step = self.undo.pop()?;
        self.redo.push(Step {
            scene: current,
            label: step.label.clone(),
            at: Instant::now(),
        });
        Some(step.scene)
    }

    /// Step forward again.
    pub fn redo(&mut self, current: Scene) -> Option<Scene> {
        let step = self.redo.pop()?;
        self.undo.push(Step {
            scene: current,
            label: step.label.clone(),
            at: Instant::now(),
        });
        Some(step.scene)
    }

    /// Forget everything, as opening a document does.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.saved_revision = None;
    }

    /// Note that `revision` is what is currently on disk.
    pub fn mark_saved(&mut self, revision: u64) {
        self.saved_revision = Some(revision);
    }

    /// Does the document differ from what was last saved?
    ///
    /// Revision-based rather than a dirty flag, so undoing back to the saved
    /// state correctly reports *clean* again — which a flag would get wrong.
    pub fn is_dirty(&self, current_revision: u64) -> bool {
        self.saved_revision != Some(current_revision)
    }

    /// Prevent the next edit from coalescing with the previous one.
    ///
    /// Called when a gesture ends — mouse up, tool change, focus loss — so a
    /// second drag is separately undoable however quickly it follows.
    pub fn break_coalescing(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.at = Instant::now() - COALESCE_WINDOW - Duration::from_millis(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{LayerId, LayerKind, ShapeData};
    use kurbo::Rect;
    use peniko::Color;

    fn scene_with(n: usize) -> Scene {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("L", LayerKind::Normal);
        for i in 0..n {
            scene.add_shape(
                layer,
                ShapeData::filled(
                    Rect::new(i as f64, 0.0, i as f64 + 1.0, 1.0).to_path(1e-9),
                    Color::WHITE,
                ),
            );
        }
        scene
    }

    fn add_one(scene: &mut Scene) {
        let layer = scene.layers().iter().next().map(|l| l.id).unwrap_or(LayerId(1));
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 50.0, 5.0, 55.0).to_path(1e-9), Color::WHITE),
        );
    }

    #[test]
    fn a_fresh_history_has_nothing_to_undo() {
        let h = History::default();
        assert!(!h.can_undo() && !h.can_redo());
        assert_eq!(h.next_undo_label(), None);
    }

    #[test]
    fn undo_restores_the_previous_document() {
        let mut history = History::default();
        let mut scene = scene_with(3);

        history.record(scene.clone(), "Draw");
        add_one(&mut scene);
        assert_eq!(scene.shape_count(), 4);

        let restored = history.undo(scene.clone()).unwrap();
        assert_eq!(restored.shape_count(), 3);
        assert!(history.can_redo());
    }

    #[test]
    fn redo_reapplies_what_undo_reversed() {
        let mut history = History::default();
        let mut scene = scene_with(3);

        history.record(scene.clone(), "Draw");
        add_one(&mut scene);
        let after_edit = scene.clone();

        let undone = history.undo(scene).unwrap();
        assert_eq!(undone.shape_count(), 3);

        let redone = history.redo(undone).unwrap();
        assert_eq!(redone.shape_count(), 4);
        assert_eq!(redone.shape_count(), after_edit.shape_count());
    }

    #[test]
    fn a_new_edit_discards_the_redo_branch() {
        let mut history = History::default();
        let mut scene = scene_with(2);

        history.record(scene.clone(), "First");
        add_one(&mut scene);
        let scene = history.undo(scene).unwrap();
        assert!(history.can_redo());

        history.record(scene.clone(), "Second");
        assert!(!history.can_redo(), "a new edit must invalidate redo");
    }

    /// The reason coalescing exists: one drag must be one undo step.
    #[test]
    fn a_continuous_drag_collapses_into_one_step() {
        let mut history = History::default();
        let mut scene = scene_with(1);
        let original = scene.clone();

        // 200 mouse-move edits, all labelled the same.
        for _ in 0..200 {
            history.record(scene.clone(), "Move");
            add_one(&mut scene);
        }

        assert_eq!(
            history.undo_depth(),
            1,
            "a single drag should leave exactly one undo step"
        );
        let restored = history.undo(scene).unwrap();
        assert_eq!(
            restored.shape_count(),
            original.shape_count(),
            "undo should return to before the drag began"
        );
    }

    #[test]
    fn differently_labelled_edits_stay_separate() {
        let mut history = History::default();
        let mut scene = scene_with(1);

        for label in ["Move", "Scale", "Rotate"] {
            history.record(scene.clone(), label);
            add_one(&mut scene);
        }
        assert_eq!(history.undo_depth(), 3);
        assert_eq!(history.labels(), vec!["Move", "Scale", "Rotate"]);
    }

    #[test]
    fn ending_a_gesture_stops_the_next_one_merging_into_it() {
        let mut history = History::default();
        let mut scene = scene_with(1);

        history.record(scene.clone(), "Move");
        add_one(&mut scene);
        history.break_coalescing();
        history.record(scene.clone(), "Move");
        add_one(&mut scene);

        assert_eq!(
            history.undo_depth(),
            2,
            "two separate drags should be two undo steps"
        );
    }

    #[test]
    fn the_stack_is_bounded_and_drops_the_oldest() {
        let mut history = History::with_depth(10);
        let mut scene = scene_with(1);

        for i in 0..50 {
            history.record(scene.clone(), format!("Edit {i}"));
            add_one(&mut scene);
        }

        assert_eq!(history.undo_depth(), 10);
        assert_eq!(
            history.labels().first(),
            Some(&"Edit 40"),
            "the oldest steps should have been dropped"
        );
    }

    #[test]
    fn undoing_everything_then_redoing_everything_returns_the_same_document() {
        let mut history = History::default();
        let mut scene = scene_with(0);

        for i in 0..12 {
            history.record(scene.clone(), format!("Edit {i}"));
            add_one(&mut scene);
        }
        let final_count = scene.shape_count();

        let mut current = scene;
        while history.can_undo() {
            current = history.undo(current).unwrap();
        }
        assert_eq!(current.shape_count(), 0, "should be back to empty");

        while history.can_redo() {
            current = history.redo(current).unwrap();
        }
        assert_eq!(current.shape_count(), final_count, "redo should restore all");
    }

    #[test]
    fn undo_on_an_empty_history_is_a_no_op() {
        let mut history = History::default();
        let scene = scene_with(1);
        assert!(history.undo(scene.clone()).is_none());
        assert!(history.redo(scene).is_none());
    }

    /// Dirty state must be revision-based: undoing back to the saved document
    /// should report clean, which a boolean flag would get wrong.
    #[test]
    fn undoing_back_to_the_saved_state_reports_clean_again() {
        let mut history = History::default();
        let mut scene = scene_with(2);

        history.mark_saved(scene.revision());
        assert!(!history.is_dirty(scene.revision()));

        history.record(scene.clone(), "Draw");
        add_one(&mut scene);
        assert!(history.is_dirty(scene.revision()), "an edit makes it dirty");

        let restored = history.undo(scene).unwrap();
        assert!(
            !history.is_dirty(restored.revision()),
            "undoing back to the saved revision should be clean again"
        );
    }

    #[test]
    fn clearing_resets_everything() {
        let mut history = History::default();
        let mut scene = scene_with(1);
        history.record(scene.clone(), "Draw");
        add_one(&mut scene);

        history.clear();
        assert!(!history.can_undo() && !history.can_redo());
        assert!(history.is_dirty(scene.revision()));
    }

    /// History holds many snapshots; they must stay cheap.
    #[test]
    fn a_full_history_of_a_large_document_stays_affordable() {
        let mut history = History::with_depth(100);
        let mut scene = scene_with(10_000);

        let start = std::time::Instant::now();
        for i in 0..100 {
            history.record(scene.clone(), format!("Edit {i}"));
            add_one(&mut scene);
        }
        let elapsed = start.elapsed();

        assert_eq!(history.undo_depth(), 100);
        assert!(
            elapsed.as_millis() < 1_000,
            "100 undo steps over a 10k-object document took {elapsed:?}"
        );
    }
}
