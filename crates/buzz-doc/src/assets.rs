//! An asset library that lives **outside** any one document.
//!
//! # Why this is not the Library panel
//!
//! [`buzz_scene::Library`] holds the symbols of *this* document, and dies with
//! it. What an animator accumulates over a series is different: a tree, a
//! lamp-post, a mouth chart, a walk cycle — things made once and dropped into
//! whatever file needs them next. Animate calls that the Assets panel; without
//! it, reuse is "open last week's file and copy".
//!
//! # Why the folders are the folders on disk
//!
//! An asset is a `.buzz` document with one thing in it, saved under
//! `%APPDATA%/BuzzAnimate/assets/`. The tree the panel shows **is** the
//! directory tree, and nothing indexes it. That means:
//!
//! * assets can be added, renamed, moved and shared by dragging files about in
//!   the file manager, which is what people do anyway;
//! * there is no index to drift out of step with what is on disk, and no
//!   repair path for when it does;
//! * an asset is a whole document, so it can be opened, edited and saved back
//!   with no separate format to maintain.
//!
//! Placing an asset goes through [`buzz_scene::Scene::merge`], which already
//! renumbers every id — the same path the importers use.

use std::path::{Path, PathBuf};

use buzz_scene::{ImportTarget, MergeReport, Scene};

use crate::format::{self, DocError};

/// Where assets are kept when nobody says otherwise.
///
/// `%APPDATA%/BuzzAnimate/assets`, beside the saved workspace layout — the same
/// rule and the same resolution order, so a user's BuzzAnimate things are in
/// one place and neither feature invents its own home.
pub fn default_root() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("assets")
}

/// One saved asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// Shown in the panel, and the file's stem on disk.
    pub name: String,
    /// Folder path relative to the library root, `/`-separated. Empty means
    /// the root, matching the convention the symbol library uses.
    pub folder: String,
    /// Where it actually is.
    pub path: PathBuf,
}

impl Asset {
    /// `Folder/Name`, for a flat listing.
    pub fn label(&self) -> String {
        if self.folder.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.folder, self.name)
        }
    }
}

/// Everything under one root directory.
///
/// Rescanned rather than watched: a library of a few hundred files is a
/// millisecond to walk, and a file watcher is a thread, a platform API and a
/// class of bug for something the user can refresh.
#[derive(Debug, Clone, Default)]
pub struct AssetLibrary {
    root: Option<PathBuf>,
    assets: Vec<Asset>,
    folders: Vec<String>,
    /// What went wrong last time, for the panel to show. An unreadable library
    /// must not be silent — an empty panel looks like "you have no assets".
    pub last_error: Option<String>,
}

impl AssetLibrary {
    /// A library rooted at `root`, scanned immediately.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let mut library = Self {
            root: Some(root.into()),
            ..Default::default()
        };
        library.rescan();
        library
    }

    /// The library in the user's own data directory.
    pub fn user() -> Self {
        Self::at(default_root())
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn len(&self) -> usize {
        self.assets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    pub fn assets(&self) -> &[Asset] {
        &self.assets
    }

    /// Every folder, sorted, including empty ones.
    pub fn folders(&self) -> &[String] {
        &self.folders
    }

    /// Assets directly inside `folder` (`""` for the root).
    pub fn in_folder(&self, folder: &str) -> Vec<&Asset> {
        self.assets.iter().filter(|a| a.folder == folder).collect()
    }

    /// Direct child folders of `parent` (`""` for the root).
    pub fn child_folders(&self, parent: &str) -> Vec<&String> {
        self.folders
            .iter()
            .filter(|f| {
                if parent.is_empty() {
                    !f.contains('/')
                } else {
                    let prefix = format!("{parent}/");
                    f.starts_with(&prefix) && !f[prefix.len()..].contains('/')
                }
            })
            .collect()
    }

    /// Walk the directory again.
    pub fn rescan(&mut self) {
        self.assets.clear();
        self.folders.clear();
        self.last_error = None;

        let Some(root) = self.root.clone() else {
            return;
        };
        if !root.exists() {
            // Not an error: a library nobody has put anything in yet.
            return;
        }
        if let Err(e) = self.walk(&root, "") {
            self.last_error = Some(e.to_string());
        }
        self.assets.sort_by_key(|a| a.label());
        self.folders.sort();
    }

    fn walk(&mut self, dir: &Path, folder: &str) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let child = if folder.is_empty() {
                    name.to_string()
                } else {
                    format!("{folder}/{name}")
                };
                self.folders.push(child.clone());
                self.walk(&path, &child)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some(format::EXTENSION)
                && let Some(name) = path.file_stem().and_then(|n| n.to_str())
            {
                self.assets.push(Asset {
                    name: name.to_string(),
                    folder: folder.to_string(),
                    path,
                });
            }
        }
        Ok(())
    }

    /// Save a scene as an asset called `name` inside `folder`.
    ///
    /// The scene is a whole document by construction — the caller builds one
    /// holding just the artwork being kept — so an asset can be opened and
    /// edited like anything else.
    pub fn save(&mut self, name: &str, folder: &str, scene: &Scene) -> Result<Asset, DocError> {
        let Some(root) = self.root.clone() else {
            return Err(DocError::Io(std::io::Error::other(
                "no assets directory is available on this machine",
            )));
        };
        let dir = if folder.is_empty() {
            root
        } else {
            root.join(folder.replace('/', std::path::MAIN_SEPARATOR_STR))
        };
        std::fs::create_dir_all(&dir).map_err(DocError::Io)?;

        let name = sanitise(name);
        let path = dir.join(format!("{name}.{}", format::EXTENSION));
        format::save(scene, &path)?;

        self.rescan();
        Ok(Asset {
            name,
            folder: folder.to_string(),
            path,
        })
    }

    /// Read an asset back as a document.
    pub fn load(&self, asset: &Asset) -> Result<Scene, DocError> {
        format::load(&asset.path)
    }

    /// Place an asset into `scene`, renumbering everything it brings with it.
    pub fn place(&self, asset: &Asset, scene: &mut Scene) -> Result<MergeReport, DocError> {
        let incoming = self.load(asset)?;
        Ok(scene.merge(&incoming, ImportTarget::Stage))
    }

    /// Delete an asset from disk.
    pub fn delete(&mut self, asset: &Asset) -> Result<(), DocError> {
        std::fs::remove_file(&asset.path).map_err(DocError::Io)?;
        self.rescan();
        Ok(())
    }

    /// Rename an asset, keeping it where it is.
    pub fn rename(&mut self, asset: &Asset, new_name: &str) -> Result<(), DocError> {
        let name = sanitise(new_name);
        if name.is_empty() {
            return Ok(());
        }
        let target = asset
            .path
            .with_file_name(format!("{name}.{}", format::EXTENSION));
        std::fs::rename(&asset.path, &target).map_err(DocError::Io)?;
        self.rescan();
        Ok(())
    }

    /// Make a folder, including its parents.
    pub fn create_folder(&mut self, folder: &str) -> Result<(), DocError> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let path = root.join(folder.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::create_dir_all(path).map_err(DocError::Io)?;
        self.rescan();
        Ok(())
    }

    /// Delete a folder and everything under it.
    ///
    /// # Why this takes the whole tree
    ///
    /// A folder in this library *is* a directory, and an animator who asks to
    /// delete "Trees" means the trees. Refusing while it has contents would
    /// leave the only way to remove a folder being to empty it by hand, one
    /// asset at a time — and the panel already has to warn before it calls
    /// this, because assets outlive documents and there is no undo out here.
    ///
    /// Refuses the root itself: an empty folder name would take the whole
    /// library with it, which is not a thing any button should be able to do
    /// by accident.
    pub fn delete_folder(&mut self, folder: &str) -> Result<(), DocError> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let folder = folder.trim_matches('/');
        if folder.is_empty() {
            return Ok(());
        }
        // Built the same way `create_folder` builds it, and checked to be
        // inside the root before anything is removed — a `..` that arrived
        // from a hand-edited config must not reach outside the library.
        let path = root.join(folder.replace('/', std::path::MAIN_SEPARATOR_STR));
        let inside = path
            .canonicalize()
            .ok()
            .zip(root.canonicalize().ok())
            .is_some_and(|(p, r)| p.starts_with(&r) && p != r);
        if !inside {
            return Ok(());
        }
        std::fs::remove_dir_all(path).map_err(DocError::Io)?;
        self.rescan();
        Ok(())
    }

    /// A name not already used in `folder`.
    pub fn unique_name(&self, wanted: &str, folder: &str) -> String {
        let taken = |name: &str| {
            self.assets
                .iter()
                .any(|a| a.folder == folder && a.name == name)
        };
        let wanted = sanitise(wanted);
        if !taken(&wanted) {
            return wanted;
        }
        for n in 2..10_000 {
            let candidate = format!("{wanted} {n}");
            if !taken(&candidate) {
                return candidate;
            }
        }
        wanted
    }
}

/// Strip what a filename cannot contain.
///
/// The name is the file's stem, so a slash in it would silently move the asset
/// into another folder and a colon would fail to save at all on Windows.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "Asset".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deleting a folder takes what is in it, and **cannot** take the library.
    ///
    /// The second half matters more than the first: an empty folder name would
    /// resolve to the root, and a button that can delete every asset a person
    /// owns by accident is not one worth having.
    #[test]
    fn a_folder_is_deleted_with_its_contents_but_the_root_is_safe() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        library.save("Oak", "Trees", &a_tree()).expect("save");
        library.save("Pine", "Trees/Conifers", &a_tree()).expect("save");
        library.save("Lamp", "", &a_tree()).expect("save");
        assert_eq!(library.len(), 3);

        // The root is refused, whatever it is spelled as.
        for name in ["", "/", "   "] {
            library.delete_folder(name).expect("refused, not failed");
            assert_eq!(library.len(), 3, "{name:?} must not empty the library");
        }

        // A real folder goes, and so does everything nested under it.
        library.delete_folder("Trees").expect("delete");
        assert_eq!(library.len(), 1, "only the lamp is left");
        assert_eq!(library.assets()[0].name, "Lamp");
        assert!(
            library.folders().iter().all(|f| !f.starts_with("Trees")),
            "the folder itself is gone too, got {:?}",
            library.folders()
        );
    }

    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::{LayerKind, ShapeData};
    use peniko::Color;

    fn a_tree() -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Tree", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 40.0, 90.0).to_path(1e-9),
                Color::from_rgb8(0x2E, 0x7D, 0x32),
            ),
        );
        scene
    }

    #[test]
    fn an_asset_is_saved_found_again_and_placed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        assert!(library.is_empty(), "a fresh library holds nothing");

        let asset = library.save("Oak", "Trees", &a_tree()).expect("save");
        assert_eq!(asset.label(), "Trees/Oak");

        // Found by a fresh scan, which is the claim that matters: the panel
        // reads the disk rather than a list it kept in memory.
        let found = AssetLibrary::at(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found.assets()[0].name, "Oak");
        assert_eq!(found.folders(), ["Trees"]);
        assert_eq!(found.in_folder("Trees").len(), 1);
        assert_eq!(found.in_folder("").len(), 0);

        // Placed into a different document, which keeps everything it had.
        let mut target = Scene::default();
        let layers_before = target.layers().len();
        let report = found.place(&found.assets()[0], &mut target).expect("place");
        assert!(report.layers > 0, "the asset brought a layer: {report:?}");
        assert!(target.layers().len() > layers_before);
    }

    /// **The whole journey**: a selection is lifted out of one document, kept
    /// as an asset, and placed into another — with the symbol it depends on
    /// travelling with it. An instance whose symbol was left behind draws
    /// nothing, and the asset would look empty on arrival.
    #[test]
    fn a_symbol_instance_survives_being_kept_and_placed_elsewhere() {
        use buzz_geom::Affine;
        use buzz_scene::SymbolKind;

        let mut source = Scene::default();
        let layer = source.layers().iter().next().expect("a layer").id;
        let symbol = source.add_symbol("Lamp Post", SymbolKind::Graphic, None);
        let instance = source
            .add_instance_at(layer, 0, symbol, Affine::translate((120.0, 40.0)))
            .expect("an instance");

        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        let asset = library
            .save("Lamp Post", "Props", &source.extract(0, &[instance]))
            .expect("save");

        let mut target = Scene::default();
        library.place(&asset, &mut target).expect("place");

        assert_eq!(
            target.library().len(),
            1,
            "the symbol should have come across"
        );
        let placed = target
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .find(|o| o.instance().is_some())
            .expect("the placed instance");
        let placed_symbol = placed.instance().expect("an instance").symbol;
        assert!(
            target.library().get(placed_symbol).is_some(),
            "the instance points at a symbol this document actually has"
        );
        assert_eq!(
            target.library().get(placed_symbol).map(|s| s.name.as_str()),
            Some("Lamp Post")
        );
    }

    /// Two assets of the same name in one folder would be one file.
    #[test]
    fn names_are_made_unique_within_a_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        library.save("Oak", "", &a_tree()).expect("save");

        assert_eq!(library.unique_name("Oak", ""), "Oak 2");
        assert_eq!(
            library.unique_name("Oak", "Trees"),
            "Oak",
            "a different folder is a different name space"
        );
    }

    /// A name typed with a slash in it must not silently move the file.
    #[test]
    fn a_name_that_cannot_be_a_filename_is_cleaned_up() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());

        let asset = library.save("Hero/Head: v2", "", &a_tree()).expect("save");
        assert_eq!(asset.name, "Hero-Head- v2");
        assert_eq!(asset.folder, "", "it stayed at the root");
        assert!(asset.path.exists());

        assert_eq!(sanitise("   "), "Asset", "an empty name still saves");
    }

    #[test]
    fn an_asset_can_be_renamed_and_deleted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        let asset = library.save("Oak", "Trees", &a_tree()).expect("save");

        library.rename(&asset, "Elm").expect("rename");
        assert_eq!(library.assets()[0].name, "Elm");
        assert!(!asset.path.exists(), "the old file should be gone");

        let renamed = library.assets()[0].clone();
        library.delete(&renamed).expect("delete");
        assert!(library.is_empty());
        assert_eq!(
            library.folders(),
            ["Trees"],
            "the folder outlives the asset, as an empty folder should"
        );
    }

    /// Nested folders are the directory tree, so the panel can walk it.
    #[test]
    fn folders_nest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        library
            .save("Shadow", "Hero/Props", &a_tree())
            .expect("save");

        assert_eq!(library.folders(), ["Hero", "Hero/Props"]);
        assert_eq!(library.child_folders(""), [&"Hero".to_string()]);
        assert_eq!(library.child_folders("Hero"), [&"Hero/Props".to_string()]);
        assert_eq!(library.in_folder("Hero/Props").len(), 1);
    }

    /// A library pointed at nothing must be empty and quiet, not a panic — the
    /// machine may have no data directory at all.
    #[test]
    fn a_library_with_no_root_is_empty_and_harmless() {
        let mut library = AssetLibrary::default();
        library.rescan();
        assert!(library.is_empty());
        assert!(library.last_error.is_none());
        assert!(library.create_folder("Trees").is_ok());
        assert!(library.save("Oak", "", &a_tree()).is_err());
    }

    /// An empty folder made in advance is a legitimate thing to have.
    #[test]
    fn an_empty_folder_is_listed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut library = AssetLibrary::at(dir.path());
        library.create_folder("Backgrounds").expect("folder");
        assert_eq!(library.folders(), ["Backgrounds"]);
        assert!(library.is_empty(), "and holds no assets");
    }
}
