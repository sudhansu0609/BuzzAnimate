//! Where the panels are — Animate's workspace, and the layout the user makes
//! of it.
//!
//! # Why this exists rather than a docking library
//!
//! egui has no docking, and the crate that adds it targets an egui this project
//! cannot move to (the GPU stack is pinned to wgpu 29 — see the README). It
//! would also be more machinery than the problem needs: an animator wants a
//! panel on the other side, floating, or gone, and wants the arrangement to
//! still be there tomorrow. That is a list of panels with a side each, which is
//! what this is.
//!
//! # The model
//!
//! Every panel names a [`Dock`]: a side of the window, a floating window, or
//! hidden. Panels docked to the same side stack in `order`. The stage is
//! whatever is left over, exactly as before — it is not a panel and cannot be
//! moved, because the thing you are drawing on is not furniture.
//!
//! **Locking** is Animate's: with the layout locked, nothing can be dragged,
//! resized or moved between sides. It is one flag rather than a per-panel one,
//! because the point of locking is that you have finished arranging *the
//! workspace* and want to stop knocking it out of place.

use serde::{Deserialize, Serialize};

/// Every panel the window can show.
///
/// The stage is deliberately absent: it is what the panels leave behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PanelId {
    Tools,
    Layers,
    Properties,
    Color,
    Swatches,
    Depth,
    Rig,
    Filters,
    Lighting,
    Sound,
    Library,
    Assets,
    Timeline,
    Actions,
}

impl PanelId {
    /// The name on the panel's title bar and in the Window menu.
    pub fn title(self) -> &'static str {
        match self {
            Self::Tools => "Tools",
            Self::Layers => "Layers",
            Self::Properties => "Properties",
            Self::Color => "Color",
            Self::Swatches => "Swatches",
            Self::Depth => "Layer Depth",
            Self::Rig => "Armature",
            Self::Filters => "Filters",
            Self::Lighting => "Lighting",
            Self::Sound => "Sound",
            Self::Library => "Library",
            Self::Assets => "Assets",
            Self::Timeline => "Timeline",
            Self::Actions => "Actions",
        }
    }

    /// Does this panel draw its own name?
    ///
    /// Most do — the heading is part of the panel. The two that do not would
    /// otherwise be nameless strips, so the dock chrome names them, and the
    /// rest are left alone rather than labelled twice.
    pub fn draws_own_title(self) -> bool {
        !matches!(self, Self::Tools | Self::Timeline)
    }

    pub const ALL: [PanelId; 14] = [
        PanelId::Tools,
        PanelId::Layers,
        PanelId::Properties,
        PanelId::Color,
        PanelId::Swatches,
        PanelId::Depth,
        PanelId::Rig,
        PanelId::Filters,
        PanelId::Lighting,
        PanelId::Sound,
        PanelId::Library,
        PanelId::Assets,
        PanelId::Timeline,
        PanelId::Actions,
    ];
}

/// Where a panel lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Dock {
    Left,
    #[default]
    Right,
    /// The outer right column. Animate's default workspace has two, and a long
    /// library wants to scroll without dragging the properties above it along.
    RightOuter,
    Bottom,
    /// A window of its own, over the stage.
    Float,
    /// Not shown. Kept rather than removed so its place is still there when it
    /// comes back.
    Hidden,
}

impl Dock {
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Dock Left",
            Self::Right => "Dock Right",
            Self::RightOuter => "Dock Far Right",
            Self::Bottom => "Dock Bottom",
            Self::Float => "Float",
            Self::Hidden => "Close",
        }
    }

    /// Is this one of the window's edges?
    pub fn is_docked(self) -> bool {
        matches!(
            self,
            Self::Left | Self::Right | Self::RightOuter | Self::Bottom
        )
    }

    /// The choices offered on a panel's own menu, in the order Animate lists
    /// them.
    pub const CHOICES: [Dock; 6] = [
        Dock::Left,
        Dock::Right,
        Dock::RightOuter,
        Dock::Bottom,
        Dock::Float,
        Dock::Hidden,
    ];
}

/// One panel's place in the layout.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub id: PanelId,
    pub dock: Dock,
    /// Where this panel belongs when it is reopened.
    ///
    /// [`Dock::Hidden`] says a panel is not on screen; it says nothing about
    /// where it goes when it comes back. Without this, closing the Actions
    /// panel and pressing F9 again put it in a floating window — which is
    /// where a panel with no home ends up.
    #[serde(default = "default_home")]
    pub home: Dock,
    /// Position within its side, low first.
    pub order: u32,
    /// Where a floating panel sits, and how big it is.
    pub float_pos: (f32, f32),
    pub float_size: (f32, f32),
}

/// The whole arrangement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// What the user called this arrangement.
    pub name: String,
    pub slots: Vec<Slot>,
    /// Nothing can be moved or resized while this is set.
    pub locked: bool,
    /// Directories documents have been opened from or saved to, most recent
    /// first, so a crash can be recovered from wherever the work was.
    ///
    /// Autosave writes its recovery copy *beside the document*, which is the
    /// right place for it and an impossible place to find again from a fresh
    /// launch — the program has no idea what was open. This is that memory,
    /// and it is capped because it is a convenience, not a document history.
    #[serde(default)]
    pub recovery_dirs: Vec<std::path::PathBuf>,
    /// What the last new document was made with, so the next one opens on it.
    ///
    /// Kept here for the same reason the theme is: it belongs to the person,
    /// not to any one film, and must not travel inside a `.buzz` file.
    #[serde(default)]
    pub new_document: crate::new_document::DocumentSetup,
    /// Dark or light chrome.
    ///
    /// Kept with the layout because it is the same kind of thing: a preference
    /// belonging to the person rather than to the film, and one that must not
    /// travel inside a `.buzz` file handed to somebody else.
    #[serde(default)]
    pub theme: crate::theme::Theme,
    /// Widths and heights of the four sides, in points.
    pub left_width: f32,
    pub right_width: f32,
    pub right_outer_width: f32,
    pub bottom_height: f32,
    /// How wide one frame cell is drawn, in points.
    ///
    /// Animate offers Tiny through Large from the timeline's menu; this is the
    /// same idea as a number, because the useful setting depends on the film —
    /// a four-thousand-frame timeline wants narrow cells and a twelve-frame
    /// cycle wants wide ones. Here rather than in the document because it is
    /// how somebody looks at the film, not part of it.
    #[serde(default = "default_frame_width")]
    pub frame_width: f32,
    /// Row height, as a multiple of the standard row.
    #[serde(default = "default_row_scale")]
    pub row_scale: f32,
}

/// Bounds for the timeline's two zooms.
///
/// Below the minimum a cell is thinner than the line drawn around it and the
/// grid turns to mush; above the maximum a handful of frames fills the panel.
pub const FRAME_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 4.0..=40.0;
pub const ROW_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.6..=2.5;

fn default_frame_width() -> f32 {
    crate::theme::Metrics::FRAME_WIDTH
}

fn default_row_scale() -> f32 {
    1.0
}

impl Default for Workspace {
    fn default() -> Self {
        Self::animate()
    }
}

impl Workspace {
    /// The layout this program opens with: Animate's, near enough — tools down
    /// the left, properties and the library on the right, timeline along the
    /// bottom.
    pub fn animate() -> Self {
        let slot = |id: PanelId, dock: Dock, order: u32, home: Dock| Slot {
            id,
            dock,
            home,
            order,
            // Floating panels start over the stage rather than at the origin,
            // where they would sit under the menu bar.
            float_pos: (320.0 + order as f32 * 24.0, 140.0 + order as f32 * 24.0),
            float_size: (300.0, 380.0),
        };

        Self {
            name: "Animator".into(),
            slots: vec![
                slot(PanelId::Tools, Dock::Left, 0, Dock::Left),
                slot(PanelId::Layers, Dock::Right, 0, Dock::Right),
                slot(PanelId::Properties, Dock::Right, 1, Dock::Right),
                slot(PanelId::Color, Dock::Right, 2, Dock::Right),
                // Beside the colour controls, which is where a palette is
                // reached for. Animate docks Swatches with Color for the same
                // reason.
                slot(PanelId::Swatches, Dock::Right, 3, Dock::Right),
                slot(PanelId::Depth, Dock::Right, 4, Dock::Right),
                slot(PanelId::Rig, Dock::Right, 5, Dock::Right),
                slot(PanelId::Filters, Dock::Right, 6, Dock::Right),
                slot(PanelId::Lighting, Dock::Right, 7, Dock::Right),
                slot(PanelId::Sound, Dock::Right, 8, Dock::Right),
                slot(PanelId::Library, Dock::RightOuter, 0, Dock::RightOuter),
                // Beside the Library, which is the panel it is most often
                // compared with: one holds this film's symbols, the other
                // what outlives the film.
                slot(PanelId::Assets, Dock::RightOuter, 1, Dock::RightOuter),
                slot(PanelId::Timeline, Dock::Bottom, 0, Dock::Bottom),
                // Closed until F9 asks for it — and it belongs at the bottom,
                // under the stage, where Animate keeps it.
                slot(PanelId::Actions, Dock::Hidden, 1, Dock::Bottom),
            ],
            locked: false,
            theme: crate::theme::Theme::default(),
            new_document: crate::new_document::DocumentSetup::default(),
            recovery_dirs: Vec::new(),
            // One tool column and its padding — the strip is Animate's
            // narrow one, and the extra half-column of empty space beside it
            // was the first thing anybody noticed.
            left_width: 46.0,
            right_width: 300.0,
            right_outer_width: 240.0,
            bottom_height: 170.0,
            frame_width: default_frame_width(),
            row_scale: default_row_scale(),
        }
    }

    /// Every panel on one side, in order.
    pub fn on(&self, dock: Dock) -> Vec<PanelId> {
        let mut found: Vec<&Slot> = self.slots.iter().filter(|s| s.dock == dock).collect();
        found.sort_by_key(|s| s.order);
        found.iter().map(|s| s.id).collect()
    }

    /// Note that documents live here, for crash recovery to look at later.
    pub fn remember_directory(&mut self, directory: impl Into<std::path::PathBuf>) {
        /// Enough to cover the projects somebody is actually moving between.
        const KEEP: usize = 8;

        let directory = directory.into();
        if directory.as_os_str().is_empty() {
            return;
        }
        self.recovery_dirs.retain(|d| *d != directory);
        self.recovery_dirs.insert(0, directory);
        self.recovery_dirs.truncate(KEEP);
    }

    pub fn slot(&self, id: PanelId) -> Option<&Slot> {
        self.slots.iter().find(|s| s.id == id)
    }

    pub fn slot_mut(&mut self, id: PanelId) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.id == id)
    }

    pub fn dock_of(&self, id: PanelId) -> Dock {
        self.slot(id).map(|s| s.dock).unwrap_or(Dock::Hidden)
    }

    pub fn is_open(&self, id: PanelId) -> bool {
        self.dock_of(id) != Dock::Hidden
    }

    /// Move a panel to a side, putting it at the end of whatever is there.
    ///
    /// Does nothing while the layout is locked — that is what locking is for,
    /// and refusing here means every route to a move is covered rather than
    /// each caller having to remember.
    pub fn move_to(&mut self, id: PanelId, dock: Dock) {
        if self.locked {
            return;
        }
        let next = self
            .slots
            .iter()
            .filter(|s| s.dock == dock && s.id != id)
            .map(|s| s.order + 1)
            .max()
            .unwrap_or(0);
        if let Some(slot) = self.slot_mut(id) {
            slot.dock = dock;
            slot.order = next;
            // Somewhere it can be seen is somewhere it can come back to.
            if dock != Dock::Hidden {
                slot.home = dock;
            }
        }
    }

    /// Show a hidden panel, or hide a shown one.
    ///
    /// Allowed while locked: a locked *layout* is about where things are, and
    /// an animator still needs to open the Actions panel and close it again.
    pub fn toggle(&mut self, id: PanelId) {
        let hidden = self.dock_of(id) == Dock::Hidden;
        let locked = self.locked;
        self.locked = false;
        self.move_to(
            id,
            if hidden {
                self.default_dock(id)
            } else {
                Dock::Hidden
            },
        );
        self.locked = locked;
    }

    /// Where a panel goes when it is reopened: the last side it was on, or
    /// the one the default workspace gives it.
    fn default_dock(&self, id: PanelId) -> Dock {
        self.slot(id)
            .map(|s| s.home)
            .filter(|d| *d != Dock::Hidden)
            .unwrap_or(Dock::Float)
    }

    /// Move a panel up or down its own side.
    pub fn reorder(&mut self, id: PanelId, delta: i32) {
        if self.locked {
            return;
        }
        let Some(slot) = self.slot(id).copied() else {
            return;
        };
        let mut side = self.on(slot.dock);
        let Some(index) = side.iter().position(|p| *p == id) else {
            return;
        };
        let to = index as i32 + delta;
        if to < 0 || to as usize >= side.len() {
            return;
        }
        side.swap(index, to as usize);
        for (order, panel) in side.into_iter().enumerate() {
            if let Some(slot) = self.slot_mut(panel) {
                slot.order = order as u32;
            }
        }
    }

    /// Every panel this build knows, whether or not the saved layout mentioned
    /// it.
    ///
    /// A workspace saved by an older build has no slot for a panel added since,
    /// and that panel would be invisible with no way to reach it. Filling the
    /// gaps on load is what stops a new feature being lost behind an old
    /// layout — which is exactly the sort of thing nobody tests until a user
    /// reports that a panel "does not exist".
    pub fn fill_gaps(&mut self) {
        let defaults = Self::animate();
        for id in PanelId::ALL {
            if self.slot(id).is_none()
                && let Some(slot) = defaults.slot(id)
            {
                self.slots.push(*slot);
            }
        }
        // And drop anything unrecognised, which is what a *newer* build's
        // layout looks like from here.
        self.slots.retain(|s| PanelId::ALL.contains(&s.id));

        // Sizes from a corrupt file must not collapse the window.
        self.left_width = self.left_width.clamp(40.0, 400.0);
        self.right_width = self.right_width.clamp(120.0, 900.0);
        self.right_outer_width = self.right_outer_width.clamp(120.0, 900.0);
        self.bottom_height = self.bottom_height.clamp(80.0, 900.0);
    }
}

// ---------------------------------------------------------------------------
// Keeping it between runs
// ---------------------------------------------------------------------------

/// The home a slot takes when a saved layout predates the field.
fn default_home() -> Dock {
    Dock::Right
}

/// Where the layout is kept.
///
/// Beside the user's other application data, never beside the document: a
/// workspace belongs to the person, not to the film, and a `.buzz` file handed
/// to somebody else must not rearrange their window.
pub fn workspace_path() -> std::path::PathBuf {
    // An explicit override, which is what a test uses so that running the
    // suite cannot rewrite the layout of whoever is running it — and what
    // anybody wanting a second, separate arrangement can use too.
    if let Some(path) = std::env::var_os("BUZZANIMATE_WORKSPACE") {
        return std::path::PathBuf::from(path);
    }

    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("workspace.json")
}

impl Workspace {
    /// Read the saved layout, or start from the default one.
    ///
    /// **Never fails.** A missing, unreadable or corrupt file gives the default
    /// workspace: a window that will not open because its layout file is
    /// damaged would be an absurd way to lose a day's work.
    pub fn load() -> Self {
        Self::load_from(&workspace_path())
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        let mut workspace = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .unwrap_or_default();
        // A layout from another build may be missing panels or hold sizes that
        // would collapse the window.
        workspace.fill_gaps();
        workspace
    }

    /// Write the layout out. Best effort: a workspace that cannot be saved is
    /// not worth interrupting anybody over.
    pub fn save(&self) {
        self.save_to(&workspace_path());
    }

    pub fn save_to(&self, path: &std::path::Path) {
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_workspace_places_every_panel() {
        let workspace = Workspace::animate();
        for id in PanelId::ALL {
            assert!(workspace.slot(id).is_some(), "{id:?} has nowhere to live");
        }
    }

    #[test]
    fn panels_come_back_in_order() {
        let workspace = Workspace::animate();
        let right = workspace.on(Dock::Right);
        assert_eq!(right.first(), Some(&PanelId::Layers));
        assert!(right.contains(&PanelId::Filters));
        assert_eq!(workspace.on(Dock::Left), vec![PanelId::Tools]);
        assert_eq!(workspace.on(Dock::Bottom), vec![PanelId::Timeline]);
    }

    #[test]
    fn a_panel_can_be_moved_to_another_side() {
        let mut workspace = Workspace::animate();
        workspace.move_to(PanelId::Lighting, Dock::Left);

        assert_eq!(workspace.dock_of(PanelId::Lighting), Dock::Left);
        assert!(!workspace.on(Dock::Right).contains(&PanelId::Lighting));
        assert_eq!(
            workspace.on(Dock::Left),
            vec![PanelId::Tools, PanelId::Lighting]
        );
    }

    /// The whole point of the lock: with it on, nothing moves.
    #[test]
    fn a_locked_workspace_refuses_to_be_rearranged() {
        let mut workspace = Workspace::animate();
        workspace.locked = true;
        let before = workspace.clone();

        workspace.move_to(PanelId::Library, Dock::Left);
        workspace.reorder(PanelId::Properties, -1);

        assert_eq!(workspace, before, "a locked layout moved");
    }

    /// Opening and closing a panel still works while locked: locking is about
    /// where things are, not about whether they are there.
    #[test]
    fn a_locked_workspace_can_still_open_and_close_panels() {
        let mut workspace = Workspace::animate();
        workspace.locked = true;

        assert!(!workspace.is_open(PanelId::Actions));
        workspace.toggle(PanelId::Actions);
        assert!(workspace.is_open(PanelId::Actions));
        assert!(workspace.locked, "toggling must not unlock the layout");

        workspace.toggle(PanelId::Actions);
        assert!(!workspace.is_open(PanelId::Actions));
    }

    /// A panel closed and reopened comes back where it belongs, not wherever
    /// it happened to be last.
    #[test]
    fn a_reopened_panel_returns_to_its_home_side() {
        let mut workspace = Workspace::animate();
        workspace.toggle(PanelId::Library);
        assert_eq!(workspace.dock_of(PanelId::Library), Dock::Hidden);

        workspace.toggle(PanelId::Library);
        assert_eq!(workspace.dock_of(PanelId::Library), Dock::RightOuter);
    }

    /// A panel moved somewhere, closed, and reopened comes back where the user
    /// last put it — not where it was born.
    #[test]
    fn a_reopened_panel_remembers_where_it_was_moved_to() {
        let mut workspace = Workspace::animate();
        workspace.move_to(PanelId::Library, Dock::Left);
        workspace.toggle(PanelId::Library);
        workspace.toggle(PanelId::Library);

        assert_eq!(workspace.dock_of(PanelId::Library), Dock::Left);
    }

    /// The Actions panel starts closed and belongs at the bottom, under the
    /// stage, where Animate keeps it. F9 must put it there rather than leaving
    /// it floating over the artwork.
    #[test]
    fn the_actions_panel_opens_along_the_bottom() {
        let mut workspace = Workspace::animate();
        assert!(!workspace.is_open(PanelId::Actions));

        workspace.toggle(PanelId::Actions);
        assert_eq!(workspace.dock_of(PanelId::Actions), Dock::Bottom);
    }

    /// Panels that draw their own heading must not be labelled twice.
    #[test]
    fn only_the_nameless_panels_are_named_by_the_dock() {
        assert!(!PanelId::Tools.draws_own_title());
        assert!(!PanelId::Timeline.draws_own_title());
        assert!(PanelId::Layers.draws_own_title());
        assert!(PanelId::Library.draws_own_title());
    }

    #[test]
    fn reordering_moves_a_panel_within_its_side() {
        let mut workspace = Workspace::animate();
        let before = workspace.on(Dock::Right);
        workspace.reorder(PanelId::Properties, -1);
        let after = workspace.on(Dock::Right);

        assert_eq!(after[0], PanelId::Properties);
        assert_eq!(after[1], before[0]);
        assert_eq!(after.len(), before.len());
    }

    #[test]
    fn reordering_off_the_end_does_nothing() {
        let mut workspace = Workspace::animate();
        let before = workspace.clone();
        workspace.reorder(PanelId::Layers, -1);
        workspace.reorder(PanelId::Tools, 1);
        assert_eq!(workspace, before);
    }

    /// A layout saved before a panel existed must not hide it for ever.
    #[test]
    fn a_layout_from_an_older_build_gains_the_panels_it_never_knew() {
        let mut workspace = Workspace::animate();
        workspace.slots.retain(|s| s.id != PanelId::Filters);
        assert!(workspace.slot(PanelId::Filters).is_none());

        workspace.fill_gaps();
        assert_eq!(workspace.dock_of(PanelId::Filters), Dock::Right);
    }

    /// And a corrupt one must not collapse the window.
    #[test]
    fn silly_sizes_are_brought_back_into_range() {
        let mut workspace = Workspace::animate();
        workspace.right_width = -4000.0;
        workspace.bottom_height = f32::INFINITY;
        workspace.fill_gaps();

        assert!(workspace.right_width >= 120.0);
        assert!(workspace.bottom_height <= 900.0);
    }

    #[test]
    fn a_workspace_survives_a_round_trip() {
        let mut workspace = Workspace::animate();
        workspace.name = "Mine".into();
        workspace.move_to(PanelId::Lighting, Dock::Float);
        workspace.locked = true;

        let json = serde_json::to_string(&workspace).expect("serialise");
        let back: Workspace = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, workspace);
    }

    #[test]
    fn a_workspace_survives_being_written_and_read() {
        let dir = std::env::temp_dir().join("buzz-workspace-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("layout.json");

        let mut workspace = Workspace::animate();
        workspace.name = "Mine".into();
        workspace.locked = true;
        workspace.move_to(PanelId::Library, Dock::Float);
        workspace.locked = true;
        workspace.save_to(&path);

        let back = Workspace::load_from(&path);
        assert_eq!(back.name, "Mine");
        assert!(back.locked);
        let _ = std::fs::remove_file(&path);
    }

    /// A layout file that is missing, empty or nonsense must not stop the
    /// window opening.
    #[test]
    fn a_damaged_layout_falls_back_to_the_default() {
        let dir = std::env::temp_dir().join("buzz-workspace-test");
        let _ = std::fs::create_dir_all(&dir);

        let missing = dir.join("not-here.json");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(Workspace::load_from(&missing), Workspace::animate());

        let broken = dir.join("broken.json");
        std::fs::write(&broken, "{ not json at all").expect("write");
        assert_eq!(Workspace::load_from(&broken), Workspace::animate());
        let _ = std::fs::remove_file(&broken);
    }

    #[test]
    fn every_panel_and_dock_has_a_name() {
        for id in PanelId::ALL {
            assert!(!id.title().is_empty());
        }
        for dock in Dock::CHOICES {
            assert!(!dock.label().is_empty());
        }
    }
}
