//! Named version snapshots: keep a copy of where the work is now, come back to
//! it later.
//!
//! A snapshot is the same idea as a scene template — a whole document saved as
//! an ordinary `.buzz` — so this composes [`crate::TemplateLibrary`] pointed at
//! a snapshots folder rather than repeating its folder-scanning. The one
//! difference is what restoring does: a template *starts a new, untitled*
//! document, but a snapshot is a past version of the document you are in, so
//! restoring hands back the **scene** for the caller to fold into the current
//! document as one undo step — keeping the file you are working on.

use std::path::PathBuf;

use buzz_scene::Scene;

use crate::{templates::Template, DocError, Document, TemplateLibrary};

/// Where snapshots live — beside the user's other application data, in a folder
/// of their own, never beside the film.
pub fn snapshots_root() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("snapshots")
}

/// The saved snapshots on disk, newest first.
#[derive(Debug, Clone, Default)]
pub struct SnapshotLibrary {
    inner: TemplateLibrary,
}

impl SnapshotLibrary {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { inner: TemplateLibrary::at(root) }
    }

    /// The library in the user's own data directory.
    pub fn user() -> Self {
        Self::at(snapshots_root())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Template> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn rescan(&mut self) {
        self.inner.rescan();
    }

    /// Keep the current scene as a snapshot under `name`. Saving the same name
    /// twice keeps both, so a snapshot is never quietly overwritten.
    pub fn save(&mut self, name: &str, scene: &Scene) -> Result<Template, DocError> {
        self.inner.save(name, scene)
    }

    /// The scene stored in a snapshot, to fold back into the current document.
    pub fn restore(&self, snapshot: &Template) -> Result<Scene, DocError> {
        Ok(Document::open(&snapshot.path)?.scene().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::LayerKind;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("buzzanimate-snapshots-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_snapshot_restores_the_scene_it_kept() {
        let root = temp_root("restore");
        let mut library = SnapshotLibrary::at(&root);

        let mut scene = Scene::default();
        scene.add_layer("Rough", LayerKind::Normal);
        let saved = library.save("Before cleanup", &scene).expect("save");
        assert_eq!(saved.name, "Before cleanup");

        let restored = library.restore(&saved).expect("restore");
        assert!(restored.layers().iter().any(|l| l.name == "Rough"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshots_of_the_same_name_both_survive() {
        let root = temp_root("twice");
        let mut library = SnapshotLibrary::at(&root);
        let a = library.save("v1", &Scene::default()).expect("first");
        let b = library.save("v1", &Scene::default()).expect("second");
        assert_eq!(a.name, "v1");
        assert_eq!(b.name, "v1 2");
        assert_eq!(library.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
