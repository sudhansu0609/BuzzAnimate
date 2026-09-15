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

/// How long a document must sit unchanged before an edit is written anyway.
///
/// # Why an interval alone is not enough
///
/// On a fixed two-minute cadence, a crash costs up to two minutes of drawing —
/// and the *hard* kinds of crash, where the process is terminated outright,
/// take the in-memory snapshot with them, so nothing else can save it either.
/// An animator draws in bursts with pauses between them, and a pause is
/// exactly when writing is free. Five seconds of quiet and the work is on
/// disk; the cost is one small write per pause, and only when something has
/// actually changed.
pub const IDLE_INTERVAL: Duration = Duration::from_secs(5);

/// Marks a file as a recovery copy rather than a document.
const RECOVERY_SUFFIX: &str = ".recovery.buzz";

/// Where work that has never been saved is recovered from.
///
/// `%APPDATA%/BuzzAnimate/recovery`, beside the workspace layout and the asset
/// library — **not** the system temp directory, which is swept by the operating
/// system and by every cleanup tool going. An hour of drawing that was never
/// saved is exactly the work most worth keeping, and putting it somewhere
/// designed to be deleted would be a poor place to keep it.
///
/// `BUZZANIMATE_RECOVERY` overrides it, which is what tests use so that running
/// the suite cannot disturb the running user's recoveries.
pub fn recovery_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("BUZZANIMATE_RECOVERY") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("recovery")
}

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
    /// The revision last *seen*, and when it appeared, so a pause in drawing
    /// can be noticed.
    last_seen: Option<(u64, Instant)>,
    idle: Duration,
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
    ///
    /// **One slot per session.** Every unsaved document would otherwise share
    /// the name `untitled`, and the first autosave of a *new* session would
    /// overwrite the recovery a crashed one left behind — destroying the work
    /// while the prompt offering it was still on screen. Found exactly that
    /// way, by crashing the program and relaunching it.
    pub fn untitled(directory: impl Into<PathBuf>) -> Self {
        Self::new(directory.into(), format!("untitled-{}", std::process::id()))
    }

    /// A fresh slot in the same directory, for a document that has just become
    /// untitled again — a recovery that was opened, and must not be saved back
    /// over the file it came from.
    pub fn reset_to_untitled(&mut self, directory: impl Into<PathBuf>) {
        let fresh = Self::untitled(directory);
        self.directory = fresh.directory;
        self.stem = fresh.stem;
        self.last_revision = None;
        self.last_seen = None;
    }

    pub fn new(directory: PathBuf, stem: String) -> Self {
        Self {
            directory,
            stem,
            interval: DEFAULT_INTERVAL,
            last_written: None,
            last_revision: None,
            last_seen: None,
            idle: IDLE_INTERVAL,
        }
    }

    /// How long a pause counts as "stopped drawing".
    pub fn with_idle(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Where this document's recovery file lives.
    pub fn recovery_path(&self) -> PathBuf {
        self.directory
            .join(format!("{}{RECOVERY_SUFFIX}", self.stem))
    }

    /// Is a save due for `revision`?
    ///
    /// False when nothing has changed since the last write, so a document
    /// nobody is touching costs nothing at all.
    ///
    /// True when the interval has passed **or** the document has been sitting
    /// unchanged for [`IDLE_INTERVAL`] with an edit not yet written — the
    /// pause between two bursts of drawing, which is when writing is free and
    /// when the work is most worth having on disk.
    pub fn is_due(&self, revision: u64) -> bool {
        if self.last_revision == Some(revision) {
            return false;
        }
        let paused = match self.last_seen {
            Some((seen, at)) => seen == revision && at.elapsed() >= self.idle,
            None => false,
        };
        match self.last_written {
            None => true,
            Some(at) => paused || at.elapsed() >= self.interval,
        }
    }

    /// Note the document as it stands, so a pause can be recognised.
    ///
    /// Called on every poll, which is cheap: it compares two integers.
    pub fn observe(&mut self, revision: u64) {
        match self.last_seen {
            Some((seen, _)) if seen == revision => {}
            _ => self.last_seen = Some((revision, Instant::now())),
        }
    }

    /// Build the work item for a due save.
    ///
    /// Returns `None` when nothing needs doing. The returned plan is `Send`, so
    /// it can be moved onto the background pool along with the snapshot.
    pub fn plan(&mut self, scene: &Scene) -> Option<AutosavePlan> {
        self.plan_scenes(&[("Scene 1", scene)], 0, scene.revision())
    }

    /// Build the work item for a due save of a whole multi-scene document.
    ///
    /// `revision` is a combined fingerprint of every scene, so any edit in any
    /// scene makes a save due again; a document nobody is touching costs
    /// nothing. Recovery keeps *all* the scenes — writing only the active one
    /// would silently lose the rest.
    pub fn plan_scenes(
        &mut self,
        scenes: &[(&str, &Scene)],
        active: usize,
        revision: u64,
    ) -> Option<AutosavePlan> {
        self.observe(revision);
        if !self.is_due(revision) {
            return None;
        }
        // Recorded optimistically: if the write fails the next tick retries,
        // and repeatedly failing writes should not spin.
        self.last_written = Some(Instant::now());
        self.last_revision = Some(revision);

        Some(AutosavePlan {
            scenes: scenes
                .iter()
                .map(|(name, scene)| ((*name).to_string(), (*scene).clone()))
                .collect(),
            active,
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

/// A snapshot of every scene and where to write it. Safe to move to another
/// thread — scenes are trees of `Arc`s, so cloning copies pointers not artwork.
#[derive(Debug, Clone)]
pub struct AutosavePlan {
    scenes: Vec<(String, Scene)>,
    active: usize,
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
        let refs: Vec<(&str, &Scene)> = self
            .scenes
            .iter()
            .map(|(name, scene)| (name.as_str(), scene))
            .collect();
        let bytes = format::to_bytes_scenes(&refs, self.active)?;
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

/// The last snapshot worth keeping if the process dies, and where it goes.
///
/// A `Mutex` rather than anything cleverer because the only two things that
/// touch it are the frame loop, which replaces it, and the panic hook, which
/// reads it once and never returns.
static LAST_CHANCE: std::sync::Mutex<Option<AutosavePlan>> = std::sync::Mutex::new(None);

/// Remember what to write if the program dies before the next autosave.
///
/// # Why this exists next to autosave
///
/// Autosave runs every two minutes, so a crash can still cost two minutes of
/// drawing. This costs a pointer copy per frame — a [`Scene`] is a tree of
/// `Arc`s and cloning one copies pointers, not artwork — and turns those two
/// minutes into nothing.
pub fn remember_for_crash(scene: &Scene, path: PathBuf) {
    remember_scenes_for_crash(&[("Scene 1", scene)], 0, scene.revision(), path);
}

/// Remember every scene of a multi-scene document for crash recovery, so a
/// crash restores the whole document rather than only the scene on screen.
pub fn remember_scenes_for_crash(
    scenes: &[(&str, &Scene)],
    active: usize,
    revision: u64,
    path: PathBuf,
) {
    let plan = AutosavePlan {
        scenes: scenes
            .iter()
            .map(|(name, scene)| ((*name).to_string(), (*scene).clone()))
            .collect(),
        active,
        path,
        revision,
    };
    if let Ok(mut slot) = LAST_CHANCE.lock() {
        *slot = Some(plan);
    }
}

/// Stop offering to write anything: the document was saved, or closed.
pub fn forget_crash_snapshot() {
    if let Ok(mut slot) = LAST_CHANCE.lock() {
        *slot = None;
    }
}

/// Write whatever was last remembered. Returns where it went.
///
/// Public because the panic hook is not the only caller that might want it —
/// a graceful "something has gone wrong" path can use it too.
pub fn flush_crash_snapshot() -> Option<PathBuf> {
    // `lock` rather than `try_lock`: the frame loop holds this for the length
    // of a pointer copy, so contention is not a real risk, and a poisoned
    // mutex still hands back the data, which is the whole point here.
    let plan = match LAST_CHANCE.lock() {
        Ok(mut slot) => slot.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    }?;
    match plan.write() {
        Ok(()) => Some(plan.path),
        Err(_) => None,
    }
}

/// Write the current document if the process panics.
///
/// Installed once, wrapping whatever hook is already there so a backtrace is
/// still printed: the crash still needs reporting, it just should not also
/// cost the artwork.
pub fn install_crash_guard() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(path) = flush_crash_snapshot() {
                // Printed rather than logged: a panic may be taking the
                // logging subsystem with it, and this is the one line the
                // user needs.
                eprintln!(
                    "BuzzAnimate crashed. Your work was written to {}",
                    path.display()
                );
            }
            previous(info);
        }));
    });
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

    /// The crash slot is **one per process**, which is what it has to be —
    /// there is one panic hook and one thing to write. That makes the three
    /// tests below share a single piece of state, and `cargo test` runs them on
    /// separate threads: without this lock, `forgetting_leaves_nothing_to_write`
    /// can clear the slot between another test's two flushes, and the failure
    /// looks exactly like a bug in the code under test.
    static CRASH_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take the crash slot, surviving a panic in an earlier test.
    ///
    /// A failed assertion while holding the lock poisons it, and every later
    /// test would then fail on the lock rather than on its own assertion —
    /// turning one real failure into three misleading ones.
    fn crash_slot() -> std::sync::MutexGuard<'static, ()> {
        CRASH_SLOT.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The two minutes autosave cannot cover.** A crash between autosaves
    /// would cost everything drawn since the last one; the snapshot kept for
    /// that moment is written instead.
    #[test]
    fn the_crash_snapshot_is_written_when_asked_for() {
        let _slot = crash_slot();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("crash.recovery.buzz");
        let mut scene = Scene::default();
        scene.add_layer("Drawn just now", buzz_scene::LayerKind::Normal);

        remember_for_crash(&scene, path.clone());
        let written = flush_crash_snapshot().expect("it should have been written");

        assert_eq!(written, path);
        assert!(path.exists());
        let back = format::load(&path).expect("readable");
        assert!(
            back.layers().iter().any(|l| l.name == "Drawn just now"),
            "the recovered document should hold what was on screen"
        );
    }

    /// Written once. A second flush has nothing to say, which is what stops a
    /// panic inside the panic hook from writing twice.
    #[test]
    fn the_crash_snapshot_is_only_written_once() {
        let _slot = crash_slot();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("once.recovery.buzz");
        remember_for_crash(&Scene::default(), path);

        assert!(flush_crash_snapshot().is_some());
        assert!(flush_crash_snapshot().is_none());
    }

    /// Saving means there is nothing to recover: leaving a snapshot behind
    /// would offer to restore work the user already has on disk.
    #[test]
    fn forgetting_leaves_nothing_to_write() {
        let _slot = crash_slot();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("saved.recovery.buzz");
        remember_for_crash(&Scene::default(), path.clone());

        forget_crash_snapshot();

        assert!(flush_crash_snapshot().is_none());
        assert!(!path.exists());
    }

    /// The guard may be installed more than once \u2014 a test binary, a second
    /// window \u2014 without stacking hooks or losing the backtrace.
    #[test]
    fn installing_the_guard_twice_is_harmless() {
        install_crash_guard();
        install_crash_guard();
    }

    /// Recovery files for unsaved work live in the application's own directory
    /// rather than the system temp, which is swept.
    #[test]
    fn the_recovery_directory_is_the_applications_own() {
        // SAFETY: set and read in this test only, before the value is used.
        unsafe { std::env::set_var("BUZZANIMATE_RECOVERY", "B:/somewhere/else") };
        assert_eq!(recovery_dir(), PathBuf::from("B:/somewhere/else"));
        unsafe { std::env::remove_var("BUZZANIMATE_RECOVERY") };

        let path = recovery_dir();
        assert!(
            path.ends_with("BuzzAnimate/recovery") || path.ends_with("BuzzAnimate\\recovery"),
            "unexpected recovery directory: {}",
            path.display()
        );
    }

    /// **A pause in drawing is when the work gets written.** On the interval
    /// alone, a crash costs up to two minutes; an animator draws in bursts,
    /// and the gap between two of them is free.
    #[test]
    fn a_pause_after_an_edit_makes_a_save_due() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut autosave = Autosave::untitled(dir.path())
            .with_interval(Duration::from_secs(3600))
            .with_idle(Duration::ZERO);

        let mut scene = Scene::default();
        // The first write always happens.
        assert!(autosave.plan(&scene).is_some());

        scene.add_layer("Drawn", buzz_scene::LayerKind::Normal);
        // Seen once...
        autosave.observe(scene.revision());
        // ...and still there a moment later: that is a pause, and the hour
        // long interval has certainly not elapsed.
        assert!(
            autosave.plan(&scene).is_some(),
            "a pause should have written the edit"
        );
    }

    /// While the hand is still moving, nothing is written on the idle rule:
    /// each change resets the pause.
    #[test]
    fn a_document_still_being_drawn_is_not_written_on_every_change() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut autosave = Autosave::untitled(dir.path())
            .with_interval(Duration::from_secs(3600))
            .with_idle(Duration::from_secs(3600));

        let mut scene = Scene::default();
        assert!(autosave.plan(&scene).is_some(), "the first write");

        for i in 0..5 {
            scene.add_layer(format!("Stroke {i}"), buzz_scene::LayerKind::Normal);
            assert!(
                autosave.plan(&scene).is_none(),
                "mid-gesture writes should wait for the pause"
            );
        }
    }

    /// And an untouched document is never rewritten, however long it sits.
    #[test]
    fn a_pause_over_an_already_written_document_writes_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut autosave = Autosave::untitled(dir.path()).with_idle(Duration::ZERO);

        let scene = Scene::default();
        assert!(autosave.plan(&scene).is_some());
        assert!(autosave.plan(&scene).is_none());
        assert!(autosave.plan(&scene).is_none());
    }

    /// **The bug this was written for.** Unsaved work used to be filed as
    /// `untitled.recovery.buzz` in one shared directory, so relaunching after
    /// a crash overwrote the recovery *while the prompt offering it was still
    /// on screen*. Found by crashing the program on purpose and looking at the
    /// file afterwards.
    #[test]
    fn two_sessions_do_not_share_one_recovery_file() {
        let dir = tempfile::tempdir().expect("temp dir");

        // A previous session's recovery, written by hand under the old name.
        let stale = dir.path().join("untitled-999999.recovery.buzz");
        std::fs::write(&stale, b"an earlier session's work").expect("write");

        let mut autosave = Autosave::untitled(dir.path());
        assert_ne!(
            autosave.recovery_path(),
            stale,
            "this session must not write over another session's recovery"
        );

        autosave
            .plan(&Scene::default())
            .expect("due")
            .write()
            .expect("write");

        assert_eq!(
            std::fs::read(&stale).expect("still there"),
            b"an earlier session's work",
            "the earlier recovery was overwritten"
        );
    }

    /// A recovered document is untitled again: saving it must ask where to go
    /// rather than writing back over the recovery, and its own autosave must
    /// land somewhere else.
    #[test]
    fn a_recovery_that_is_reopened_gets_a_slot_of_its_own() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut autosave = Autosave::beside(&dir.path().join("Scene 1.buzz"));
        let from = autosave.recovery_path();

        autosave.reset_to_untitled(dir.path());

        assert_ne!(autosave.recovery_path(), from);
        assert!(
            !autosave
                .recovery_path()
                .to_string_lossy()
                .contains(".recovery.recovery"),
            "suffixes must not stack: {}",
            autosave.recovery_path().display()
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
