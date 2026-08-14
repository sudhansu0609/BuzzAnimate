//! Named colours, in folders — the document's own palette.
//!
//! # Why a colour needs a name
//!
//! A recent-colours row remembers *what you last used*, which is useful for a
//! minute and worthless the next day. A production needs the opposite: the
//! colours of a show, agreed once and reachable by name. "Hero Skin Shadow" is
//! a decision; `#C08A6E` is a number that looks like three other numbers on a
//! swatch strip, and the wrong one gets picked at four in the morning.
//!
//! So swatches are part of the **document**, they have names, and they live in
//! folders the way symbols do — a character's palette in one, the backgrounds
//! in another. Animate's Swatches panel is a flat grid of unnamed chips plus
//! `.clr` import; this keeps the grid and adds the two things every animator
//! ends up faking with a note file.

use std::collections::{BTreeMap, BTreeSet};

use peniko::Color;
use serde::{Deserialize, Serialize};

/// Identifies a swatch for the life of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SwatchId(pub u64);

/// A named colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Swatch {
    pub id: SwatchId,
    /// What it is called. Never empty in practice — the panel gives a new
    /// swatch a name — but not enforced, because a half-typed rename is a
    /// legitimate intermediate state.
    pub name: String,
    pub color: Color,
    /// Folder, `/`-separated. `None` means the palette's root, exactly as in
    /// the symbol library.
    pub folder: Option<String>,
}

impl Swatch {
    pub fn new(id: SwatchId, name: impl Into<String>, color: Color) -> Self {
        Self {
            id,
            name: name.into(),
            color,
            folder: None,
        }
    }

    /// `Folder/Name`, for a listing that has lost its indentation.
    pub fn path(&self) -> String {
        match &self.folder {
            Some(folder) if !folder.is_empty() => format!("{folder}/{}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// The document's palette.
///
/// Folders are kept as their own set, so an empty folder survives — the same
/// rule the symbol library follows, and for the same reason: a folder made in
/// advance of the artwork must not vanish when the file is saved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Swatches {
    swatches: BTreeMap<SwatchId, Swatch>,
    folders: BTreeSet<String>,
    /// Swatch ids come from here rather than from the document's allocator.
    /// A palette does not interoperate with objects or symbols, and taking
    /// from the shared allocator would shift every other id in a new document
    /// by however many colours the default palette happens to have.
    next: u64,
}

impl Swatches {
    pub fn len(&self) -> usize {
        self.swatches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.swatches.is_empty()
    }

    pub fn get(&self, id: SwatchId) -> Option<&Swatch> {
        self.swatches.get(&id)
    }

    /// Every swatch, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = &Swatch> {
        self.swatches.values()
    }

    /// Add a colour, with an id of its own and a name nothing else is using.
    pub fn add(&mut self, name: &str, color: Color, folder: Option<String>) -> SwatchId {
        let id = SwatchId(self.next.max(1));
        self.next = id.0 + 1;
        let mut swatch = Swatch::new(id, self.unique_name(name), color);
        swatch.folder = folder;
        self.insert(swatch)
    }

    /// Add a swatch that already has an id — loading a file, mostly.
    pub fn insert(&mut self, swatch: Swatch) -> SwatchId {
        let id = swatch.id;
        self.next = self.next.max(id.0 + 1);
        if let Some(folder) = &swatch.folder
            && !folder.is_empty()
        {
            self.add_folder(folder);
        }
        self.swatches.insert(id, swatch);
        id
    }

    pub fn remove(&mut self, id: SwatchId) -> Option<Swatch> {
        self.swatches.remove(&id)
    }

    pub fn update(&mut self, id: SwatchId, f: impl FnOnce(&mut Swatch)) -> bool {
        match self.swatches.get_mut(&id) {
            Some(swatch) => {
                f(swatch);
                if let Some(folder) = swatch.folder.clone()
                    && !folder.is_empty()
                {
                    self.add_folder(&folder);
                }
                true
            }
            None => false,
        }
    }

    /// Create a folder, including its parents.
    pub fn add_folder(&mut self, path: &str) {
        let mut accumulated = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if !accumulated.is_empty() {
                accumulated.push('/');
            }
            accumulated.push_str(part);
            self.folders.insert(accumulated.clone());
        }
    }

    /// Remove a folder, moving anything inside it to the root.
    ///
    /// Deleting colours because a folder was deleted would be a poor trade —
    /// the same rule the symbol library follows for artwork.
    pub fn remove_folder(&mut self, path: &str) {
        let prefix = format!("{path}/");
        self.folders
            .retain(|f| f != path && !f.starts_with(&prefix));

        let orphans: Vec<SwatchId> = self
            .swatches
            .values()
            .filter(|s| {
                s.folder
                    .as_deref()
                    .is_some_and(|f| f == path || f.starts_with(&prefix))
            })
            .map(|s| s.id)
            .collect();
        for id in orphans {
            self.update(id, |s| s.folder = None);
        }
    }

    /// Every folder, sorted, including empty ones.
    pub fn folders(&self) -> impl Iterator<Item = &String> {
        self.folders.iter()
    }

    /// Swatches directly inside `folder` (`None` for the root).
    pub fn in_folder(&self, folder: Option<&str>) -> Vec<&Swatch> {
        self.swatches
            .values()
            .filter(|s| s.folder.as_deref() == folder)
            .collect()
    }

    /// Direct child folders of `parent`.
    pub fn child_folders(&self, parent: Option<&str>) -> Vec<&String> {
        self.folders
            .iter()
            .filter(|f| match parent {
                Some(parent) => {
                    let prefix = format!("{parent}/");
                    f.starts_with(&prefix) && !f[prefix.len()..].contains('/')
                }
                None => !f.contains('/'),
            })
            .collect()
    }

    /// The first swatch of this exact colour, if there is one.
    ///
    /// Used to show, beside the fill and stroke colours, which named colour is
    /// in use — the reason for naming them in the first place.
    pub fn find_color(&self, color: Color) -> Option<&Swatch> {
        let same = |a: Color, b: Color| a.to_rgba8().to_u8_array() == b.to_rgba8().to_u8_array();
        self.swatches.values().find(|s| same(s.color, color))
    }

    /// A name not already taken, based on `wanted`.
    pub fn unique_name(&self, wanted: &str) -> String {
        if !self.swatches.values().any(|s| s.name == wanted) {
            return wanted.to_string();
        }
        for n in 2..10_000 {
            let candidate = format!("{wanted} {n}");
            if !self.swatches.values().any(|s| s.name == candidate) {
                return candidate;
            }
        }
        wanted.to_string()
    }
}

/// A palette of Animate's default colours, named.
pub fn default_swatches() -> Swatches {
    let mut swatches = Swatches::default();
    for (name, color) in default_palette() {
        swatches.add(name, color, None);
    }
    swatches
}

/// Animate's default swatch set, named.
///
/// The same ten colours the Color panel has always offered here, which are
/// Animate's own web-safe primaries — with the names an animator would use out
/// loud, so a new document's palette is immediately readable.
pub fn default_palette() -> Vec<(&'static str, Color)> {
    vec![
        ("Black", Color::BLACK),
        ("White", Color::WHITE),
        ("Red", Color::from_rgb8(0xFF, 0x00, 0x00)),
        ("Green", Color::from_rgb8(0x00, 0xFF, 0x00)),
        ("Blue", Color::from_rgb8(0x00, 0x00, 0xFF)),
        ("Yellow", Color::from_rgb8(0xFF, 0xFF, 0x00)),
        ("Cyan", Color::from_rgb8(0x00, 0xFF, 0xFF)),
        ("Magenta", Color::from_rgb8(0xFF, 0x00, 0xFF)),
        ("Grey", Color::from_rgb8(0x99, 0x99, 0x99)),
        ("Orange", Color::from_rgb8(0xFF, 0x99, 0x00)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Swatches {
        default_swatches()
    }

    /// Ids must not repeat, or renaming one colour would rename another.
    #[test]
    fn ids_are_unique_however_a_swatch_arrives() {
        let mut swatches = default_swatches();
        let loaded = SwatchId(500);
        swatches.insert(Swatch::new(loaded, "Loaded", Color::WHITE));
        let fresh = swatches.add("Fresh", Color::BLACK, None);

        assert_ne!(fresh, loaded);
        assert!(
            fresh.0 > loaded.0,
            "a later id must not collide with a loaded one"
        );
        let ids: std::collections::BTreeSet<SwatchId> = swatches.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), swatches.len());
    }

    #[test]
    fn a_new_palette_has_named_colours_at_the_root() {
        let swatches = palette();
        assert_eq!(swatches.len(), 10);
        assert_eq!(swatches.in_folder(None).len(), 10);
        assert!(swatches.iter().all(|s| !s.name.is_empty()));
    }

    #[test]
    fn a_swatch_can_be_put_in_a_folder() {
        let mut swatches = palette();
        let id = SwatchId(1);
        swatches.update(id, |s| s.folder = Some("Hero/Skin".into()));

        assert_eq!(swatches.get(id).unwrap().path(), "Hero/Skin/Black");
        assert_eq!(swatches.in_folder(Some("Hero/Skin")).len(), 1);
        assert_eq!(
            swatches.child_folders(None),
            vec![&"Hero".to_string()],
            "the parent folder should have been made too"
        );
        assert_eq!(
            swatches.child_folders(Some("Hero")),
            vec![&"Hero/Skin".to_string()]
        );
    }

    /// An empty folder is a legitimate thing to have made in advance.
    #[test]
    fn an_empty_folder_survives() {
        let mut swatches = Swatches::default();
        swatches.add_folder("Backgrounds");
        assert_eq!(swatches.folders().count(), 1);
    }

    /// Deleting a folder must not delete the colours in it.
    #[test]
    fn removing_a_folder_keeps_its_colours() {
        let mut swatches = palette();
        swatches.update(SwatchId(1), |s| s.folder = Some("Hero".into()));
        swatches.update(SwatchId(2), |s| s.folder = Some("Hero/Skin".into()));

        swatches.remove_folder("Hero");

        assert_eq!(swatches.len(), 10, "no colour should have been lost");
        assert!(swatches.iter().all(|s| s.folder.is_none()));
        assert_eq!(swatches.folders().count(), 0);
    }

    /// Naming is the point, so two swatches must not end up with one name by
    /// accident.
    #[test]
    fn names_are_made_unique() {
        let mut swatches = palette();
        assert_eq!(swatches.unique_name("Red"), "Red 2");
        swatches.add("Red", Color::from_rgb8(0xAA, 0, 0), None);
        assert_eq!(swatches.unique_name("Red"), "Red 3");
        assert_eq!(
            swatches.unique_name("Sky"),
            "Sky",
            "a free name is left alone"
        );
    }

    #[test]
    fn a_colour_can_be_looked_up_by_value() {
        let swatches = palette();
        assert_eq!(
            swatches
                .find_color(Color::from_rgb8(0xFF, 0x99, 0x00))
                .map(|s| s.name.as_str()),
            Some("Orange")
        );
        assert!(
            swatches
                .find_color(Color::from_rgb8(0x12, 0x34, 0x56))
                .is_none()
        );
    }
}
