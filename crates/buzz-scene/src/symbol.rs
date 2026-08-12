//! Symbols and the library.
//!
//! # What a symbol is
//!
//! A symbol is reusable artwork with **its own timeline**, stored once in the
//! library and placed on the stage as any number of *instances*. Editing the
//! symbol updates every instance — that is the whole point, and it is what
//! makes an Animate document a fraction of the size of its rendered output.
//!
//! Animate has three kinds, and the difference is entirely about how their
//! timelines behave:
//!
//! | Kind | Timeline behaviour |
//! |---|---|
//! | [`SymbolKind::Graphic`] | Locked to the parent's. Scrubbing the parent scrubs it. |
//! | [`SymbolKind::MovieClip`] | Independent. On the authoring stage it shows only its first frame. |
//! | [`SymbolKind::Button`] | Four fixed frames: Up, Over, Down, Hit. |
//!
//! Getting the Graphic/MovieClip distinction wrong is immediately obvious to a
//! user: scrubbing the timeline would either animate nothing or animate
//! everything.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use buzz_geom::Rect;
use peniko::Color;
use serde::{Deserialize, Serialize};

use crate::layer::LayerStack;
use crate::object::Object;

/// Stable identity for a library symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SymbolId(pub u64);

/// The three symbol kinds Animate offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Timeline locked to the parent's.
    #[default]
    Graphic,
    /// Independent timeline; shows its first frame while authoring.
    MovieClip,
    /// Up / Over / Down / Hit.
    Button,
}

impl SymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Graphic => "Graphic",
            Self::MovieClip => "Movie Clip",
            Self::Button => "Button",
        }
    }

    /// Does this symbol's timeline advance with the parent's playhead?
    ///
    /// Only graphics do. A movie clip runs independently, and Animate shows
    /// just its first frame while you author.
    pub fn follows_parent_timeline(self) -> bool {
        matches!(self, Self::Graphic)
    }

    /// Button frame names, in order.
    pub const BUTTON_FRAMES: [&'static str; 4] = ["Up", "Over", "Down", "Hit"];
}

/// How a graphic instance plays its timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoopMode {
    /// Repeat from the start. Animate's default.
    #[default]
    Loop,
    /// Play once and hold the last frame.
    PlayOnce,
    /// Freeze on a single frame.
    SingleFrame,
}

impl LoopMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loop => "Loop",
            Self::PlayOnce => "Play Once",
            Self::SingleFrame => "Single Frame",
        }
    }
}

/// Animate's per-instance colour effect.
///
/// Stored as multiply-then-add per channel, which is exactly what Animate's
/// Advanced colour effect exposes and what Tint, Alpha and Brightness reduce
/// to. Keeping one representation means tweening a colour effect is just
/// interpolating six numbers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorTransform {
    pub multiply: [f32; 4],
    /// Added afterwards, in 0..1 units.
    pub add: [f32; 4],
}

impl Default for ColorTransform {
    fn default() -> Self {
        Self {
            multiply: [1.0; 4],
            add: [0.0; 4],
        }
    }
}

impl ColorTransform {
    pub fn is_identity(&self) -> bool {
        self.multiply == [1.0; 4] && self.add == [0.0; 4]
    }

    /// Animate's Alpha effect.
    pub fn alpha(amount: f32) -> Self {
        Self {
            multiply: [1.0, 1.0, 1.0, amount.clamp(0.0, 1.0)],
            add: [0.0; 4],
        }
    }

    /// Animate's Tint: blend towards `color` by `amount`.
    pub fn tint(color: Color, amount: f32) -> Self {
        let a = amount.clamp(0.0, 1.0);
        let [r, g, b, _] = color.to_rgba8().to_u8_array();
        Self {
            multiply: [1.0 - a, 1.0 - a, 1.0 - a, 1.0],
            add: [
                r as f32 / 255.0 * a,
                g as f32 / 255.0 * a,
                b as f32 / 255.0 * a,
                0.0,
            ],
        }
    }

    /// Animate's Brightness, from -1 (black) to 1 (white).
    pub fn brightness(amount: f32) -> Self {
        let a = amount.clamp(-1.0, 1.0);
        if a >= 0.0 {
            Self {
                multiply: [1.0 - a, 1.0 - a, 1.0 - a, 1.0],
                add: [a, a, a, 0.0],
            }
        } else {
            Self {
                multiply: [1.0 + a, 1.0 + a, 1.0 + a, 1.0],
                add: [0.0; 4],
            }
        }
    }

    /// Apply to a colour.
    pub fn apply(&self, color: Color) -> Color {
        let [r, g, b, a] = color.to_rgba8().to_u8_array();
        let ch = |v: u8, i: usize| {
            ((v as f32 / 255.0) * self.multiply[i] + self.add[i]).clamp(0.0, 1.0)
        };
        Color::from_rgba8(
            (ch(r, 0) * 255.0).round() as u8,
            (ch(g, 1) * 255.0).round() as u8,
            (ch(b, 2) * 255.0).round() as u8,
            (ch(a, 3) * 255.0).round() as u8,
        )
    }

    /// Apply `self` first, then `outer`, as a single transform.
    ///
    /// Nested symbols each carry an effect, so a tinted symbol inside a faded
    /// one needs both. Composing into one transform keeps the renderer's
    /// recursion carrying a value rather than a chain of closures.
    pub fn compose(&self, outer: &Self) -> Self {
        let mut multiply = [0.0; 4];
        let mut add = [0.0; 4];
        for i in 0..4 {
            multiply[i] = self.multiply[i] * outer.multiply[i];
            add[i] = self.add[i] * outer.multiply[i] + outer.add[i];
        }
        Self { multiply, add }
    }

    /// Interpolate, for tweening a colour effect.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        let mut multiply = [0.0; 4];
        let mut add = [0.0; 4];
        for i in 0..4 {
            multiply[i] = mix(self.multiply[i], other.multiply[i]);
            add[i] = mix(self.add[i], other.add[i]);
        }
        Self { multiply, add }
    }
}

/// A placed instance of a library symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolInstance {
    pub symbol: SymbolId,
    /// Which frame of the symbol's timeline the instance starts on.
    pub first_frame: u32,
    pub loop_mode: LoopMode,
    pub color: ColorTransform,
}

impl SymbolInstance {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            first_frame: 0,
            loop_mode: LoopMode::Loop,
            color: ColorTransform::default(),
        }
    }

    /// Which frame of the symbol to show, given the parent's frame.
    ///
    /// `elapsed` is how many frames the instance has been on stage.
    pub fn resolve_frame(&self, kind: SymbolKind, elapsed: u32, symbol_length: u32) -> u32 {
        let length = symbol_length.max(1);

        if !kind.follows_parent_timeline() {
            // A movie clip runs on its own timeline, so while authoring
            // Animate shows its first frame and nothing else.
            return self.first_frame.min(length - 1);
        }

        match self.loop_mode {
            LoopMode::SingleFrame => self.first_frame.min(length - 1),
            LoopMode::Loop => (self.first_frame + elapsed) % length,
            LoopMode::PlayOnce => (self.first_frame + elapsed).min(length - 1),
        }
    }
}

/// A library symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    /// Library folder, `/`-separated. `None` means the library root.
    pub folder: Option<String>,
    /// The symbol's own layers and frames.
    pub layers: LayerStack,
    /// Registration point, which instance transforms are relative to.
    pub registration: buzz_geom::Point,
}

impl Symbol {
    pub fn new(id: SymbolId, name: impl Into<String>, kind: SymbolKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            folder: None,
            layers: LayerStack::new(),
            registration: buzz_geom::Point::ORIGIN,
        }
    }

    /// Frames in this symbol's timeline.
    pub fn length(&self) -> u32 {
        self.layers.frame_count()
    }

    /// Artwork shown at `frame`, in paint order.
    pub fn objects_at(&self, frame: u32) -> Vec<&Arc<Object>> {
        self.layers
            .drawable_at(frame)
            .flat_map(|l| l.objects_at(frame).iter())
            .collect()
    }

    /// Bounds of everything in the symbol, across all frames.
    pub fn bounds(&self) -> Option<Rect> {
        self.layers
            .iter()
            .filter_map(|l| l.bounds())
            .reduce(|a, b| a.union(b))
    }

    /// Full library path, e.g. `characters/hero`.
    pub fn path(&self) -> String {
        match &self.folder {
            Some(f) if !f.is_empty() => format!("{f}/{}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// The document's library.
///
/// Folders are stored as paths on the symbols themselves plus an explicit
/// folder set, so an empty folder survives — Animate lets you make a folder
/// before putting anything in it, and losing it on save would be surprising.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    symbols: BTreeMap<SymbolId, Arc<Symbol>>,
    folders: BTreeSet<String>,
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    pub fn get(&self, id: SymbolId) -> Option<&Arc<Symbol>> {
        self.symbols.get(&id)
    }

    /// Symbols in insertion-independent order, so listings are stable.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Symbol>> {
        self.symbols.values()
    }

    pub fn insert(&mut self, symbol: Symbol) -> SymbolId {
        let id = symbol.id;
        if let Some(folder) = &symbol.folder
            && !folder.is_empty()
        {
            self.add_folder(folder);
        }
        self.symbols.insert(id, Arc::new(symbol));
        id
    }

    pub fn remove(&mut self, id: SymbolId) -> Option<Arc<Symbol>> {
        self.symbols.remove(&id)
    }

    /// Edit a symbol in place, cloning only if another snapshot shares it.
    pub fn update(&mut self, id: SymbolId, f: impl FnOnce(&mut Symbol)) -> bool {
        match self.symbols.get_mut(&id) {
            Some(slot) => {
                f(Arc::make_mut(slot));
                true
            }
            None => false,
        }
    }

    /// Find a symbol by its name, as scripts and importers do.
    pub fn find_by_name(&self, name: &str) -> Option<&Arc<Symbol>> {
        self.symbols.values().find(|s| s.name == name)
    }

    /// A name not already used, so duplicates do not collide.
    pub fn unique_name(&self, base: &str) -> String {
        if self.find_by_name(base).is_none() {
            return base.to_string();
        }
        for n in 1..10_000 {
            let candidate = format!("{base} {n}");
            if self.find_by_name(&candidate).is_none() {
                return candidate;
            }
        }
        format!("{base} {}", self.symbols.len() + 1)
    }

    /// Create a folder, including its parents.
    pub fn add_folder(&mut self, path: &str) {
        let path = path.trim_matches('/');
        if path.is_empty() {
            return;
        }
        // Register every ancestor, so `a/b/c` also creates `a` and `a/b`.
        let mut accumulated = String::new();
        for part in path.split('/') {
            if part.is_empty() {
                continue;
            }
            if !accumulated.is_empty() {
                accumulated.push('/');
            }
            accumulated.push_str(part);
            self.folders.insert(accumulated.clone());
        }
    }

    /// Remove a folder, moving anything inside it to the root.
    ///
    /// Deleting artwork because a folder was deleted would be a poor trade;
    /// Animate keeps the contents.
    pub fn remove_folder(&mut self, path: &str) {
        let prefix = format!("{path}/");
        self.folders
            .retain(|f| f != path && !f.starts_with(&prefix));

        let affected: Vec<SymbolId> = self
            .symbols
            .values()
            .filter(|s| {
                s.folder
                    .as_deref()
                    .is_some_and(|f| f == path || f.starts_with(&prefix))
            })
            .map(|s| s.id)
            .collect();
        for id in affected {
            self.update(id, |s| s.folder = None);
        }
    }

    /// Every folder, sorted, including empty ones.
    pub fn folders(&self) -> impl Iterator<Item = &String> {
        self.folders.iter()
    }

    /// Symbols directly inside `folder` (`None` for the root).
    pub fn symbols_in(&self, folder: Option<&str>) -> Vec<&Arc<Symbol>> {
        self.symbols
            .values()
            .filter(|s| s.folder.as_deref() == folder)
            .collect()
    }

    /// Direct child folders of `parent`.
    pub fn child_folders(&self, parent: Option<&str>) -> Vec<String> {
        self.folders
            .iter()
            .filter_map(|f| match parent {
                None => (!f.contains('/')).then(|| f.clone()),
                Some(p) => {
                    let prefix = format!("{p}/");
                    f.strip_prefix(&prefix)
                        .filter(|rest| !rest.contains('/'))
                        .map(|_| f.clone())
                }
            })
            .collect()
    }

    /// Move a symbol into a folder.
    pub fn move_to_folder(&mut self, id: SymbolId, folder: Option<&str>) -> bool {
        if let Some(f) = folder {
            self.add_folder(f);
        }
        let owned = folder.map(|f| f.trim_matches('/').to_string());
        self.update(id, |s| s.folder = owned)
    }

    /// Highest id in use, so an importer can resume allocation safely.
    pub fn max_id(&self) -> u64 {
        self.symbols.keys().map(|s| s.0).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        let mut lib = Library::new();
        lib.insert(Symbol::new(SymbolId(1), "hero", SymbolKind::Graphic));
        lib.insert(Symbol::new(SymbolId(2), "villain", SymbolKind::MovieClip));
        lib
    }

    #[test]
    fn symbol_kinds_differ_in_timeline_behaviour() {
        assert!(SymbolKind::Graphic.follows_parent_timeline());
        assert!(
            !SymbolKind::MovieClip.follows_parent_timeline(),
            "a movie clip runs independently"
        );
        assert!(!SymbolKind::Button.follows_parent_timeline());
        assert_eq!(SymbolKind::MovieClip.label(), "Movie Clip");
    }

    /// A graphic follows the parent playhead; a movie clip does not.
    #[test]
    fn a_graphic_advances_with_the_parent_but_a_movie_clip_does_not() {
        let instance = SymbolInstance::new(SymbolId(1));

        assert_eq!(instance.resolve_frame(SymbolKind::Graphic, 3, 10), 3);
        assert_eq!(
            instance.resolve_frame(SymbolKind::MovieClip, 3, 10),
            0,
            "a movie clip shows its first frame while authoring"
        );
    }

    #[test]
    fn loop_modes_behave_as_named() {
        let mut instance = SymbolInstance::new(SymbolId(1));

        instance.loop_mode = LoopMode::Loop;
        assert_eq!(instance.resolve_frame(SymbolKind::Graphic, 12, 5), 2);

        instance.loop_mode = LoopMode::PlayOnce;
        assert_eq!(
            instance.resolve_frame(SymbolKind::Graphic, 12, 5),
            4,
            "play once holds the last frame"
        );

        instance.loop_mode = LoopMode::SingleFrame;
        instance.first_frame = 2;
        assert_eq!(instance.resolve_frame(SymbolKind::Graphic, 12, 5), 2);
    }

    #[test]
    fn resolving_a_frame_never_exceeds_the_symbol() {
        let instance = SymbolInstance {
            first_frame: 99,
            ..SymbolInstance::new(SymbolId(1))
        };
        for kind in [SymbolKind::Graphic, SymbolKind::MovieClip] {
            for length in [1, 3, 10] {
                let f = instance.resolve_frame(kind, 500, length);
                assert!(f < length, "{kind:?} gave frame {f} for length {length}");
            }
        }
        // A zero-length symbol must not divide by zero.
        assert_eq!(instance.resolve_frame(SymbolKind::Graphic, 5, 0), 0);
    }

    #[test]
    fn colour_effects_reduce_to_multiply_and_add() {
        let identity = ColorTransform::default();
        assert!(identity.is_identity());
        assert_eq!(
            identity.apply(Color::from_rgb8(10, 20, 30)).to_rgba8().to_u8_array(),
            [10, 20, 30, 255]
        );

        // Alpha halves the alpha channel and leaves colour alone.
        let half = ColorTransform::alpha(0.5);
        let out = half.apply(Color::from_rgb8(200, 100, 50)).to_rgba8().to_u8_array();
        assert_eq!(&out[..3], &[200, 100, 50]);
        assert!((out[3] as i32 - 128).abs() <= 1, "alpha was {}", out[3]);
    }

    #[test]
    fn brightness_moves_towards_white_and_black() {
        let white = ColorTransform::brightness(1.0)
            .apply(Color::from_rgb8(100, 100, 100))
            .to_rgba8()
            .to_u8_array();
        assert_eq!(&white[..3], &[255, 255, 255]);

        let black = ColorTransform::brightness(-1.0)
            .apply(Color::from_rgb8(100, 100, 100))
            .to_rgba8()
            .to_u8_array();
        assert_eq!(&black[..3], &[0, 0, 0]);
    }

    #[test]
    fn tint_blends_towards_the_tint_colour() {
        let full = ColorTransform::tint(Color::from_rgb8(255, 0, 0), 1.0)
            .apply(Color::from_rgb8(0, 0, 255))
            .to_rgba8()
            .to_u8_array();
        assert_eq!(&full[..3], &[255, 0, 0], "full tint should replace the colour");
    }

    /// Composing must equal applying one then the other.
    #[test]
    fn composing_two_effects_matches_applying_them_in_turn() {
        let inner = ColorTransform::tint(Color::from_rgb8(255, 0, 0), 0.5);
        let outer = ColorTransform::alpha(0.5);
        let combined = inner.compose(&outer);

        for probe in [
            Color::from_rgb8(0, 0, 255),
            Color::from_rgb8(200, 130, 60),
            Color::WHITE,
        ] {
            let sequential = outer.apply(inner.apply(probe)).to_rgba8().to_u8_array();
            let composed = combined.apply(probe).to_rgba8().to_u8_array();
            for i in 0..4 {
                assert!(
                    composed[i].abs_diff(sequential[i]) <= 1,
                    "channel {i}: composed {composed:?} vs sequential {sequential:?}"
                );
            }
        }
    }

    #[test]
    fn composing_with_the_identity_changes_nothing() {
        let effect = ColorTransform::brightness(0.3);
        let composed = effect.compose(&ColorTransform::default());
        assert_eq!(composed.multiply, effect.multiply);
        assert_eq!(composed.add, effect.add);
    }

    #[test]
    fn colour_transforms_interpolate_for_tweening() {
        let a = ColorTransform::default();
        let b = ColorTransform::alpha(0.0);
        let mid = a.lerp(&b, 0.5);
        assert!((mid.multiply[3] - 0.5).abs() < 1e-6, "got {}", mid.multiply[3]);
    }

    #[test]
    fn the_library_stores_and_finds_symbols() {
        let lib = library();
        assert_eq!(lib.len(), 2);
        assert_eq!(lib.get(SymbolId(1)).unwrap().name, "hero");
        assert!(lib.find_by_name("villain").is_some());
        assert!(lib.find_by_name("nobody").is_none());
    }

    #[test]
    fn duplicate_names_are_made_unique() {
        let lib = library();
        assert_eq!(lib.unique_name("fresh"), "fresh");
        assert_eq!(lib.unique_name("hero"), "hero 1");
    }

    #[test]
    fn folders_are_created_with_their_parents() {
        let mut lib = Library::new();
        lib.add_folder("characters/heroes/main");

        let all: Vec<&String> = lib.folders().collect();
        assert_eq!(
            all,
            vec!["characters", "characters/heroes", "characters/heroes/main"],
            "every ancestor should exist"
        );
    }

    #[test]
    fn an_empty_folder_survives() {
        let mut lib = Library::new();
        lib.add_folder("empty");
        assert_eq!(lib.folders().count(), 1);
        assert!(lib.symbols_in(Some("empty")).is_empty());
    }

    #[test]
    fn symbols_can_be_moved_between_folders() {
        let mut lib = library();
        assert!(lib.move_to_folder(SymbolId(1), Some("characters")));

        assert_eq!(lib.symbols_in(Some("characters")).len(), 1);
        assert_eq!(lib.symbols_in(None).len(), 1);
        assert_eq!(lib.get(SymbolId(1)).unwrap().path(), "characters/hero");

        assert!(lib.move_to_folder(SymbolId(1), None));
        assert_eq!(lib.symbols_in(None).len(), 2);
    }

    /// Deleting a folder must not delete the artwork inside it.
    #[test]
    fn removing_a_folder_keeps_its_contents() {
        let mut lib = library();
        lib.move_to_folder(SymbolId(1), Some("characters/heroes"));
        lib.move_to_folder(SymbolId(2), Some("characters"));

        lib.remove_folder("characters");

        assert_eq!(lib.len(), 2, "no symbol should have been deleted");
        assert_eq!(lib.symbols_in(None).len(), 2, "both moved to the root");
        assert_eq!(lib.folders().count(), 0);
    }

    #[test]
    fn child_folders_are_listed_one_level_at_a_time() {
        let mut lib = Library::new();
        lib.add_folder("a/b/c");
        lib.add_folder("x");

        let mut roots = lib.child_folders(None);
        roots.sort();
        assert_eq!(roots, vec!["a", "x"]);

        assert_eq!(lib.child_folders(Some("a")), vec!["a/b"]);
        assert_eq!(lib.child_folders(Some("a/b")), vec!["a/b/c"]);
    }

    #[test]
    fn folder_paths_are_normalised() {
        let mut lib = Library::new();
        lib.add_folder("/leading/");
        assert_eq!(lib.folders().next().map(String::as_str), Some("leading"));

        lib.add_folder("");
        lib.add_folder("///");
        assert_eq!(lib.folders().count(), 1, "empty paths should be ignored");
    }

    #[test]
    fn editing_a_symbol_shares_the_rest_of_the_library() {
        let mut lib = library();
        let untouched = Arc::as_ptr(lib.get(SymbolId(2)).unwrap());

        lib.update(SymbolId(1), |s| s.name = "renamed".into());

        assert_eq!(lib.get(SymbolId(1)).unwrap().name, "renamed");
        assert_eq!(
            Arc::as_ptr(lib.get(SymbolId(2)).unwrap()),
            untouched,
            "an unrelated symbol should not be reallocated"
        );
    }

    #[test]
    fn max_id_lets_an_importer_resume_allocation() {
        let mut lib = Library::new();
        assert_eq!(lib.max_id(), 0);
        lib.insert(Symbol::new(SymbolId(500), "imported", SymbolKind::Graphic));
        assert_eq!(lib.max_id(), 500);
    }
}
