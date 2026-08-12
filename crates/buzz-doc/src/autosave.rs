//! Autosave and crash recovery.
//!
//! # What this does and does not do
//!
//! It owns the *policy* — when a save is due, where recovery files live, how
//! they are named and cleaned up — and the *mechanism*, which is an atomic
//! write. It deliberately does **not** own a thread. The caller takes a
//! [`buzz_scene::Scene`] snapshot, hands it to the background pool, and calls
//! [`AutosavePlan::write`] from there. Snapshots are immutable, so no locking
//! is involved and editing continues unaffected while the bytes are produced.
//!
//! # Why recovery files are separate from the document
//!
//! Autosaving over the user's file would silently commit changes they never
//! chose to save, and would destroy the on-disk version they might want back.
//! Recovery files sit beside the document under a distinct name, are offered
//! on next launch, and are deleted on a successful manual save.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use buzz_scene::Scene;

use crate::format::{self, DocError};

/// How often an autosave becomes due. Animate's default is ten minutes; this
/// is more frequent because the writes are cheap and off-thread.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(120);

/// Marks a file as a recovery copy rather than a document.
const RECOVERY_SUFFIX: &str = ".recovery.buzz";

/// Decides when an autosave is due and where it goes.
#[derive(Debug, Clone)]
pub struct Autosave {
    directory: PathBuf,
    /// Stem of the document being edited, or a placeholder for an unsaved one.
    stem: String,
    interval: Duration,
    last_written: Option<Instant>,
    /// Revision at the last successful write, so an idle document is not
    /// rewritten over and over.
    last_revision: Option<u64>,
}

impl Autosave {
    /// Autosave next to `document_path`.
    pub fn beside(document_path: &Path) -> Self {
        let directory = document_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let stem = document_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".into());
        Self::new(directory, stem)
    }

    /// Autosave into a directory, for a document that has never been saved.
    pub fn untitled(directory: impl Into<PathBuf>) -> Self {
        Self::new(directory.into(), "untitled".to_string())
    }

    pub fn new(directory: PathBuf, stem: String) -> Self {
        Self {
            directory,
            stem,
            interval: DEFAULT_INTERVAL,
            last_written: None,
            last_revision: None,
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Where this document's recovery file lives.
    pub fn recovery_path(&self) -> PathBuf {
        self.directory.join(format!("{}{RECOVERY_SUFFIX}", self.stem))
    }

    /// Is a save due for `revision`?
    ///
    /// False when nothing has changed since the last write, so an idle
    /// document costs nothing.
    pub fn is_due(&self, revision: u64) -> bool {
        if self.last_revision == Some(revision) {
            return false;
        }
        match self.last_written {
            None => true,
            Some(at) => at.elapsed() >= self.interval,
        }
    }

    /// Build the work item for a due save.
    ///
    /// Returns `None` when nothing needs doing. The returned plan is `Send`, so
    /// it can be moved onto the background pool along with the snapshot.
    pub fn plan(&mut self, scene: &Scene) -> Option<AutosavePlan> {
        let revision = scene.revision();
        if !self.is_due(revision) {
            return None;
        }
        // Recorded optimistically: if the write fails the next tick retries,
        // and repeatedly failing writes should not spin.
        self.last_written = Some(Instant::now());
        self.last_revision = Some(revision);

        Some(AutosavePlan {
            scene: scene.clone(),
            path: self.recovery_path(),
            revision,
        })
    }

    /// Called after a successful manual save: the recovery file is now stale
    /// and keeping it would prompt a pointless recovery on next launch.
    pub fn discard_recovery(&mut self) -> Result<(), DocError> {
        let path = self.recovery_path();
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        self.last_revision = None;
        Ok(())
    }

    /// Point autosave at a new location, after Save As.
    pub fn retarget(&mut self, document_path: &Path) {
        let fresh = Self::beside(document_path);
        self.directory = fresh.directory;
        self.stem = fresh.stem;
        self.last_revision = None;
    }
}

/// A snapshot and where to write it. Safe to move to another thread.
#[derive(Debug, Clone)]
pub struct AutosavePlan {
    scene: Scene,
    path: PathBuf,
    revision: u64,
}

impl AutosavePlan {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Serialise and write. Intended to run on the background pool.
    pub fn write(&self) -> Result<(), DocError> {
        let bytes = format::to_bytes(&self.scene)?;
        format::write_atomic(&self.path, &bytes)?;
        tracing::debug!(path = %self.path.display(), revision = self.revision, "autosaved");
        Ok(())
    }
}

/// A recovery file found on startup.
#[derive(Debug, Clone)]
pub struct Recovery {
    pub path: PathBuf,
    /// The document it belongs to, if that file still exists.
    pub document: Option<PathBuf>,
    pub modified: Option<std::time::SystemTime>,
}

/// Look for recovery files in `directory`.
///
/// Offered to the user on launch rather than opened automatically: silently
/// replacing a document with a recovery copy would be its own kind of data
/// loss.
pub fn find_recoveries(directory: impl AsRef<Path>) -> Vec<Recovery> {
    let directory = directory.as_ref();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut found: Vec<Recovery> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy().to_string();
            let stem = name.strip_suffix(RECOVERY_SUFFIX)?;

            let document = {
                let candidate = directory.join(format!("{stem}.{}", format::EXTENSION));
                candidate.exists().then_some(candidate)
            };

            Some(Recovery {
                modified: entry.metadata().ok().and_then(|m| m.modified().ok()),
                path,
                document,
            })
        })
        .collect();

    // Most recent first, which is what a recovery prompt should show.
    found.sort_by_key(|r| std::cmp::Reverse(r.modified));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{LayerKind, ShapeData};
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

    #[test]
    fn the_first_save_is_immediately_due() {
        let dir = tempfile::tempdir().unwrap();
        let autosave = Autosave::untitled(dir.path());
        assert!(autosave.is_due(1));
    }

    #[test]
    fn an_unchanged_document_is_never_re_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path()).with_interval(Duration::ZERO);
        let scene = scene_with(3);

        assert!(autosave.plan(&scene).is_some());
        assert!(
            autosave.plan(&scene).is_none(),
            "the same revision must not be written twice"
        );
    }

    #[test]
    fn an_edit_makes_a_save_due_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path()).with_interval(Duration::ZERO);
        let mut scene = scene_with(3);

        autosave.plan(&scene).unwrap();
        let layer = scene.layers().iter().next().unwrap().id;
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 9.0, 1.0, 10.0).to_path(1e-9), Color::WHITE),
        );

        assert!(autosave.plan(&scene).is_some(), "an edit should be saved");
    }

    #[test]
    fn the_interval_is_respected() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path()).with_interval(Duration::from_secs(3600));
        let mut scene = scene_with(1);

        autosave.plan(&scene).unwrap();
        let layer = scene.layers().iter().next().unwrap().id;
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 9.0, 1.0, 10.0).to_path(1e-9), Color::WHITE),
        );

        assert!(
            autosave.plan(&scene).is_none(),
            "an edit within the interval should still wait"
        );
    }

    /// The point of the design: serialising happens off the editing thread.
    #[test]
    fn a_plan_can_be_written_from_another_thread() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path()).with_interval(Duration::ZERO);
        let mut scene = scene_with(500);

        let plan = autosave.plan(&scene).expect("a save should be due");
        let expected = plan.path().to_path_buf();

        let worker = std::thread::spawn(move || plan.write());

        // Keep editing while the write is in flight.
        let layer = scene.layers().iter().next().unwrap().id;
        for i in 0..200 {
            scene.add_shape(
                layer,
                ShapeData::filled(
                    Rect::new(i as f64, 40.0, i as f64 + 1.0, 41.0).to_path(1e-9),
                    Color::WHITE,
                ),
            );
        }

        worker.join().unwrap().expect("the write should succeed");

        let recovered = format::load(&expected).unwrap();
        assert_eq!(
            recovered.shape_count(),
            500,
            "the recovery file should hold the snapshot, not the later edits"
        );
        assert_eq!(scene.shape_count(), 700);
    }

    #[test]
    fn recovery_files_are_found_and_linked_to_their_document() {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("drawing.buzz");
        format::save(&scene_with(2), &document).unwrap();

        let mut autosave = Autosave::beside(&document).with_interval(Duration::ZERO);
        autosave.plan(&scene_with(5)).unwrap().write().unwrap();

        let found = find_recoveries(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].document.as_deref(), Some(document.as_path()));
        assert!(found[0].modified.is_some());
    }

    #[test]
    fn a_recovery_without_its_document_is_still_offered() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path()).with_interval(Duration::ZERO);
        autosave.plan(&scene_with(3)).unwrap().write().unwrap();

        let found = find_recoveries(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].document, None,
            "an unsaved document's recovery has no source file"
        );
    }

    #[test]
    fn saving_manually_discards_the_recovery_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path()).with_interval(Duration::ZERO);
        autosave.plan(&scene_with(3)).unwrap().write().unwrap();
        assert_eq!(find_recoveries(dir.path()).len(), 1);

        autosave.discard_recovery().unwrap();
        assert!(
            find_recoveries(dir.path()).is_empty(),
            "a stale recovery would prompt pointlessly on next launch"
        );
    }

    #[test]
    fn discarding_a_recovery_that_does_not_exist_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path());
        assert!(autosave.discard_recovery().is_ok());
    }

    #[test]
    fn scanning_a_missing_directory_returns_nothing() {
        assert!(find_recoveries("no/such/directory").is_empty());
    }

    #[test]
    fn save_as_retargets_the_recovery_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path());
        let before = autosave.recovery_path();

        autosave.retarget(&dir.path().join("renamed.buzz"));
        let after = autosave.recovery_path();

        assert_ne!(before, after);
        assert!(after.to_string_lossy().contains("renamed"));
    }

    #[test]
    fn a_recovery_file_is_a_normal_loadable_document() {
        let dir = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::untitled(dir.path()).with_interval(Duration::ZERO);
        let scene = scene_with(7);
        let plan = autosave.plan(&scene).unwrap();
        plan.write().unwrap();

        let recovered = format::load(plan.path()).unwrap();
        assert_eq!(recovered.shape_count(), 7);
    }
}
