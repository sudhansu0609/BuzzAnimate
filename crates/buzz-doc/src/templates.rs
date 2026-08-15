//! Scene templates: a stage you set up once and start from again.
//!
//! # Why a template is just a `.buzz` file
//!
//! Everything a template has to carry — stage size, background, frame rate,
//! the camera, the lights, the layers, the symbols they use, the sounds — is
//! already exactly what a document carries. So a template *is* a document,
//! kept in a folder of its own, and starting from one is opening it and
//! forgetting where it came from. No second format, no partial copy that has
//! to be kept in step with what a document can hold, and a template can be
//! opened and edited like anything else because it is like anything else.
//!
//! This is the same argument the Assets library makes for storing an asset as
//! a `.buzz` (see [`crate::assets`]), and it is worth making twice: the
//! alternative — a bespoke "stage settings" record — would have to grow a
//! field every time a document gained one, and would silently drop whatever
//! nobody remembered to add.
//!
//! # What starting from a template does *not* do
//!
//! It does not remember the template. The new document is untitled, so Save
//! asks where to put it and cannot write back over the template by accident.
//! A template is a starting point, not a link.

use std::path::{Path, PathBuf};

use buzz_scene::Scene;

use crate::{DocError, Document};

/// Where templates live: beside the user's other application data, never
/// beside the film. A template belongs to the person.
pub fn default_root() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("templates")
}

/// One saved starting point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub path: PathBuf,
}

/// The templates on disk.
///
/// Rescanned rather than watched, like the Assets library before its watcher:
/// listing a folder is microseconds and the list is only wanted when a menu
/// opens.
#[derive(Debug, Clone, Default)]
pub struct TemplateLibrary {
    root: Option<PathBuf>,
    templates: Vec<Template>,
    /// What went wrong last time, for the caller to show. An unreadable folder
    /// must not look like an empty one.
    pub last_error: Option<String>,
}

impl TemplateLibrary {
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

    pub fn iter(&self) -> impl Iterator<Item = &Template> {
        self.templates.iter()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Read the folder again.
    pub fn rescan(&mut self) {
        self.templates.clear();
        self.last_error = None;
        let Some(root) = self.root.clone() else {
            return;
        };
        // A folder that is not there yet is not an error — it is a user who
        // has saved no templates.
        if !root.exists() {
            return;
        }
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) => {
                self.last_error = Some(e.to_string());
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(crate::EXTENSION))
                && let Some(name) = path.file_stem().map(|s| s.to_string_lossy().to_string())
            {
                self.templates.push(Template { name, path });
            }
        }
        // Sorted, so the menu does not reorder itself between launches.
        self.templates.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// A name nothing else is using, so saving twice does not overwrite.
    pub fn unique_name(&self, wanted: &str) -> String {
        let base = wanted.trim();
        let base = if base.is_empty() { "Template" } else { base };
        if !self.templates.iter().any(|t| t.name == base) {
            return base.to_string();
        }
        for n in 2.. {
            let candidate = format!("{base} {n}");
            if !self.templates.iter().any(|t| t.name == candidate) {
                return candidate;
            }
        }
        base.to_string()
    }

    /// Keep this document as a template under `name`.
    pub fn save(&mut self, name: &str, scene: &Scene) -> Result<Template, DocError> {
        let Some(root) = self.root.clone() else {
            return Err(DocError::Io(std::io::Error::other(
                "no templates folder is configured",
            )));
        };
        std::fs::create_dir_all(&root)?;

        let name = self.unique_name(name);
        let path = root.join(format!("{name}.{}", crate::EXTENSION));
        let mut doc = Document::new(scene.clone());
        doc.save_as(&path)?;

        self.rescan();
        Ok(Template { name, path })
    }

    /// Start a document from a template.
    ///
    /// The result **forgets the file it came from**, so Save asks where to put
    /// it: a template is a starting point, and writing a film back over it
    /// would destroy the starting point.
    pub fn start(&self, template: &Template) -> Result<Document, DocError> {
        let mut doc = Document::open(&template.path)?;
        doc.forget_path();
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::LayerKind;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "buzzanimate-templates-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A stage set up once comes back set up.
    #[test]
    fn a_template_carries_the_whole_stage() {
        let root = temp_root("stage");
        let mut library = TemplateLibrary::at(&root);

        let mut scene = Scene::default();
        scene.stage_mut().size = buzz_geom::Size::new(1920.0, 1080.0);
        scene.stage_mut().frame_rate = 12.0;
        scene.stage_mut().background = peniko::Color::from_rgb8(0x20, 0x30, 0x40);
        scene.add_layer("Background", LayerKind::Normal);
        scene.add_layer("Characters", LayerKind::Normal);

        let saved = library.save("Night Exterior", &scene).expect("save");
        assert_eq!(saved.name, "Night Exterior");

        let doc = library.start(&saved).expect("start");
        let back = doc.scene();
        assert_eq!(back.stage().size.width, 1920.0);
        assert_eq!(back.stage().frame_rate, 12.0);
        assert_eq!(back.stage().background, scene.stage().background);
        assert!(back.layers().iter().any(|l| l.name == "Characters"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A document started from a template is untitled**, so Save cannot
    /// write the film back over the starting point.
    #[test]
    fn starting_from_a_template_forgets_the_template() {
        let root = temp_root("forget");
        let mut library = TemplateLibrary::at(&root);
        let saved = library.save("Blank", &Scene::default()).expect("save");

        let doc = library.start(&saved).expect("start");
        assert!(doc.path().is_none(), "it still points at the template");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Saving the same name twice keeps both, rather than one quietly
    /// replacing the other.
    #[test]
    fn saving_twice_does_not_overwrite() {
        let root = temp_root("unique");
        let mut library = TemplateLibrary::at(&root);

        let first = library.save("Set", &Scene::default()).expect("first");
        let second = library.save("Set", &Scene::default()).expect("second");

        assert_eq!(first.name, "Set");
        assert_eq!(second.name, "Set 2");
        assert_eq!(library.len(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The list is sorted, so a menu does not shuffle between launches.
    #[test]
    fn templates_are_listed_in_a_stable_order() {
        let root = temp_root("order");
        let mut library = TemplateLibrary::at(&root);
        for name in ["Zulu", "Alpha", "Mike"] {
            library.save(name, &Scene::default()).expect("save");
        }

        let names: Vec<&str> = library.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Mike", "Zulu"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder that does not exist yet is somebody with no templates, not an
    /// error to report.
    #[test]
    fn an_absent_folder_is_simply_empty() {
        let library = TemplateLibrary::at(temp_root("absent"));
        assert!(library.is_empty());
        assert!(library.last_error.is_none());
    }
}
