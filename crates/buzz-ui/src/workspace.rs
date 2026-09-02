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
    /// Everything the program is doing in the background: exports, imports,
    /// asset scans. Global rather than per-document; hidden until asked for.
    Tasks,
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
            Self::Rig => "Rigging",
            Self::Filters => "Filters",
            Self::Lighting => "Lighting",
            Self::Sound => "Sound",
            Self::Library => "Library",
            Self::Assets => "Assets",
            Self::Timeline => "Timeline",
            Self::Actions => "Actions",
            Self::Tasks => "Tasks",
        }
    }

    /// The name on a tab, which has a column to share rather than fill.
    ///
    /// Five tabs in a 216-point section is the arrangement this was added for,
    /// and "Layer Depth" and "Rigging" spelled out do not fit in it. Animate
    /// abbreviates its own tabs for the same reason. Only the four long ones
    /// differ; the rest are already short, and a second name for a panel that
    /// does not need one is just something else to keep in step.
    pub fn tab_title(self) -> &'static str {
        match self {
            Self::Depth => "Depth",
            Self::Rig => "Rig",
            Self::Lighting => "Light",
            Self::Properties => "Props",
            other => other.title(),
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

    pub const ALL: [PanelId; 15] = [
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
        PanelId::Tasks,
    ];
}

/// Where a panel lives.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
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
    /// Rolled up to its title bar.
    ///
    /// A dock column holds nine panels in the default layout, and a column is
    /// as tall as the window. Without this, reaching the ninth means scrolling
    /// past the other eight every time — so the ones an animator opens
    /// occasionally start rolled up, and the ones they live in start open.
    #[serde(default)]
    pub collapsed: bool,
    /// Which tab stack this panel is part of, within its dock.
    ///
    /// Panels sharing a dock **and** a group are one *section*: one strip of
    /// tabs, one set of chrome, and only the front tab's contents drawn. This
    /// is Animate's panel group, and it is the answer to a column that is
    /// taller than the window — five occasional panels in one section cost the
    /// height of one.
    ///
    /// A number rather than a list of members, because the membership has to
    /// survive a panel being moved, hidden and brought back, and a list would
    /// need mending at every one of those. Nothing outside this module should
    /// mint one: [`Workspace::group_with`] and [`Workspace::ungroup`] are the
    /// only ways in and out.
    #[serde(default)]
    pub group: GroupId,
    /// Is this the front tab of its section?
    ///
    /// Exactly one panel in each section carries this; [`Workspace::sections`]
    /// falls back to the first member if a hand-edited file carries none or
    /// several, so a damaged layout shows a panel rather than an empty box.
    #[serde(default)]
    pub selected: bool,
}

/// Identifies a tab stack within one dock. See [`Slot::group`].
pub type GroupId = u32;

/// One tab stack: the panels in it, and which one is at the front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub group: GroupId,
    /// Every panel in the stack, in tab order. Never empty.
    pub panels: Vec<PanelId>,
    /// The one whose contents are drawn.
    pub front: PanelId,
    /// Rolled up to its tab strip.
    pub collapsed: bool,
}

impl Section {
    /// Is this a real stack, or a lone panel wearing the same chrome?
    pub fn is_tabbed(&self) -> bool {
        self.panels.len() > 1
    }
}

/// The whole arrangement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Which build's idea of a layout this file holds.
    ///
    /// Not a file format version — the layout is plain JSON and every field
    /// has a default. This is for the one thing defaults cannot express: a
    /// change to what a *sensible* arrangement looks like. A saved layout from
    /// before panels could be rolled up has every panel open, which is
    /// indistinguishable from a user who opened them all, and is also exactly
    /// the wall of nine panels the roll-up was added to fix. Bumping this
    /// adopts the new arrangement once, and never touches it again.
    #[serde(default)]
    pub version: u32,
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
    /// How big the Assets panel draws its pictures, and so how it lays them
    /// out. A way of looking at the library rather than part of any film.
    #[serde(default)]
    pub asset_thumbnail_size: crate::assets_panel::ThumbnailSize,
    /// The timeline's layer column shows the **parenting view** — Animate's
    /// node graph — instead of layer names.
    ///
    /// A way of looking at the film rather than part of it, so it lives here
    /// with the other view preferences and survives a restart.
    #[serde(default)]
    pub parenting_view: bool,
    /// The timeline's layer column shows **layer depth** instead of names.
    /// Mutually exclusive with the parenting view: one column, one question.
    #[serde(default)]
    pub depth_view: bool,
}

/// Bounds for the timeline's two zooms.
///
/// Below the minimum a cell is thinner than the line drawn around it and the
/// grid turns to mush; above the maximum a handful of frames fills the panel.
pub const FRAME_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 4.0..=40.0;
pub const ROW_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.6..=2.5;

/// How wide a dock column may be dragged.
///
/// # Why the minimum is not smaller
///
/// It was 120, which is narrower than the *contents* of every panel that goes
/// in one of these columns. A Layers row is three switches, a colour chip and a
/// name; a Properties row is a label and two number fields. Dragged down to
/// 120 the column still drew — it just drew everything with its right-hand end
/// cut off, which is indistinguishable from a panel whose controls are
/// missing, and is exactly how it was reported. This number is the width at
/// which those rows are whole, and the scroll bar's width is on top of it.
pub const COLUMN_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 216.0..=900.0;
/// The tool strip, which holds icons rather than rows.
///
/// One 30-point tool button, the panel frame's margins either side of it, and
/// the scroll bar's own width. It was 46, which was one button and its margins
/// and nothing else — correct until the dock's scroll bars stopped floating
/// over the content and started reserving space, at which point the buttons no
/// longer fitted the strip they were measured for.
pub const LEFT_WIDTH_RANGE: std::ops::RangeInclusive<f32> =
    (46.0 + crate::theme::Metrics::SCROLL_BAR)..=400.0;
/// The timeline, which needs a couple of layer rows to be worth having.
pub const BOTTOM_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 80.0..=900.0;

/// The two sections the default arrangement ships grouped.
///
/// Numbered above any `order` so they cannot collide with the one-panel
/// sections, which take their group from their position.
const GROUP_UTILITY: GroupId = 100;
const GROUP_ASSETS: GroupId = 101;

/// Bring a size back inside its range — including a NaN out of a damaged file,
/// which `f32::clamp` panics on rather than fixing.
pub fn clamp_to(value: f32, range: std::ops::RangeInclusive<f32>) -> f32 {
    if value.is_nan() {
        return *range.start();
    }
    value.clamp(*range.start(), *range.end())
}

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
    /// **Put every panel back where it started, and change nothing else.**
    ///
    /// Panels can be docked, undocked, tabbed, rolled up, resized and closed,
    /// and a layout can end up in a state there is no obvious way back from —
    /// so there has to be one button that undoes all of it at once.
    ///
    /// # What it deliberately keeps
    ///
    /// This struct is where the *layout* lives, and it is also where a few
    /// things live that are not layout at all: the theme, the settings the
    /// next new document opens with, and the list of directories to look in
    /// for recovered work after a crash. Resetting by replacing the whole
    /// workspace — which is what this used to do — threw those away with it.
    /// Losing your dark theme is an irritation; losing the only record of
    /// where an autosave went is losing work, and nobody pressing "Reset
    /// Layout" is asking for either.
    /// # Everything else goes back
    ///
    /// Written as "the fresh layout, except these three", rather than as a
    /// list of what to reset. A field added to this struct later is then part
    /// of the reset by default, which is the safe way round: forgetting to add
    /// one here leaves a button called Reset Layout that does not reset the
    /// layout, and the mistake is invisible until somebody's panels will not
    /// go back.
    pub fn reset_layout(&mut self) {
        let recovery_dirs = std::mem::take(&mut self.recovery_dirs);
        let new_document = self.new_document.clone();
        let theme = self.theme;

        *self = Self {
            recovery_dirs,
            new_document,
            theme,
            ..Self::animate()
        };
    }

    /// The layout this program opens with: Animate's, near enough — tools down
    /// the left, properties and the library on the right, timeline along the
    /// bottom.
    pub fn animate() -> Self {
        // `group` is the section this panel shares; `front` marks the tab that
        // starts at the front of it.
        let tab = |id: PanelId, dock: Dock, order: u32, home: Dock, group: GroupId, front: bool| {
            Slot {
                id,
                dock,
                home,
                order,
                collapsed: false,
                group,
                selected: front,
                // Floating panels start over the stage rather than at the
                // origin, where they would sit under the menu bar.
                float_pos: (320.0 + order as f32 * 24.0, 140.0 + order as f32 * 24.0),
                float_size: (300.0, 380.0),
            }
        };
        // A panel in a section of its own, which is what most of them are.
        let slot = |id: PanelId, dock: Dock, order: u32, home: Dock| {
            tab(id, dock, order, home, order, true)
        };

        Self {
            version: LAYOUT_VERSION,
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
                // **One section, five tabs.** Each of these is reached for now
                // and then rather than all day. Rolled up they were five title
                // bars taking five rows and showing nothing; as tabs they take
                // one row and always show one of them. That is what the
                // grouping is *for*, and the default arrangement should
                // demonstrate it rather than leave it as something to discover.
                tab(PanelId::Depth, Dock::Right, 4, Dock::Right, GROUP_UTILITY, true),
                tab(PanelId::Rig, Dock::Right, 5, Dock::Right, GROUP_UTILITY, false),
                tab(
                    PanelId::Filters,
                    Dock::Right,
                    6,
                    Dock::Right,
                    GROUP_UTILITY,
                    false,
                ),
                tab(
                    PanelId::Lighting,
                    Dock::Right,
                    7,
                    Dock::Right,
                    GROUP_UTILITY,
                    false,
                ),
                tab(
                    PanelId::Sound,
                    Dock::Right,
                    8,
                    Dock::Right,
                    GROUP_UTILITY,
                    false,
                ),
                // The Library and the Assets panel share a section too: one
                // holds this film's symbols and the other what outlives the
                // film, which is the comparison an animator is making when
                // they reach for either.
                tab(
                    PanelId::Library,
                    Dock::RightOuter,
                    0,
                    Dock::RightOuter,
                    GROUP_ASSETS,
                    true,
                ),
                tab(
                    PanelId::Assets,
                    Dock::RightOuter,
                    1,
                    Dock::RightOuter,
                    GROUP_ASSETS,
                    false,
                ),
                slot(PanelId::Timeline, Dock::Bottom, 0, Dock::Bottom),
                // Closed until F9 asks for it — and it belongs at the bottom,
                // under the stage, where Animate keeps it.
                slot(PanelId::Actions, Dock::Hidden, 1, Dock::Bottom),
                // Hidden until there is something to watch; opened from the
                // Window menu, or by the shell when an export is enqueued.
                slot(PanelId::Tasks, Dock::Hidden, 2, Dock::Bottom),
            ],
            locked: false,
            theme: crate::theme::Theme::default(),
            new_document: crate::new_document::DocumentSetup::default(),
            recovery_dirs: Vec::new(),
            // One tool column, its padding and the scroll bar — the strip is
            // Animate's narrow one, and the extra half-column of empty space
            // beside it was the first thing anybody noticed.
            left_width: *LEFT_WIDTH_RANGE.start(),
            right_width: 300.0,
            right_outer_width: 240.0,
            bottom_height: 170.0,
            frame_width: default_frame_width(),
            row_scale: default_row_scale(),
            parenting_view: false,
            depth_view: false,
            asset_thumbnail_size: crate::assets_panel::ThumbnailSize::default(),
        }
    }

    /// Every panel on one side, in order.
    pub fn on(&self, dock: Dock) -> Vec<PanelId> {
        let mut found: Vec<&Slot> = self.slots.iter().filter(|s| s.dock == dock).collect();
        found.sort_by_key(|s| s.order);
        found.iter().map(|s| s.id).collect()
    }

    /// The sections on one side, in the order they are drawn down it.
    ///
    /// A section is a tab stack: every panel sharing this dock and a group.
    /// Sections are ordered by their earliest member, so grouping two panels
    /// puts the section where the first of them already was rather than
    /// shuffling the column.
    pub fn sections(&self, dock: Dock) -> Vec<Section> {
        let mut here: Vec<&Slot> = self.slots.iter().filter(|s| s.dock == dock).collect();
        here.sort_by_key(|s| s.order);

        let mut sections: Vec<Section> = Vec::new();
        for slot in here {
            match sections.iter_mut().find(|s| s.group == slot.group) {
                Some(section) => {
                    section.panels.push(slot.id);
                    if slot.selected {
                        section.front = slot.id;
                    }
                    // A section is rolled up if the layout says its front tab
                    // is; the two are kept in step by `set_collapsed`.
                    section.collapsed |= slot.collapsed && slot.selected;
                }
                None => sections.push(Section {
                    group: slot.group,
                    panels: vec![slot.id],
                    front: slot.id,
                    collapsed: slot.collapsed,
                }),
            }
        }

        // A file that names no front tab, or several, still has to draw one.
        for section in &mut sections {
            if !section.panels.contains(&section.front) {
                section.front = section.panels[0];
            }
            section.collapsed = self
                .slot(section.front)
                .is_some_and(|slot| slot.collapsed);
        }
        sections
    }

    /// The section a panel belongs to, if it is on screen.
    pub fn section_of(&self, id: PanelId) -> Option<Section> {
        let dock = self.slot(id)?.dock;
        self.sections(dock).into_iter().find(|s| s.panels.contains(&id))
    }

    /// Bring a panel to the front of its section.
    ///
    /// Allowed while the layout is locked, for the same reason opening and
    /// closing a panel is: locking is about where things are, and clicking a
    /// tab does not move anything.
    pub fn select_tab(&mut self, id: PanelId) {
        let Some(slot) = self.slot(id).copied() else {
            return;
        };
        // Whatever the outgoing front tab's roll-up state was, the section
        // keeps it — clicking a tab should not silently unroll the section, and
        // should not roll it up either.
        let collapsed = self
            .sections(slot.dock)
            .into_iter()
            .find(|s| s.group == slot.group)
            .is_some_and(|s| s.collapsed);

        for other in &mut self.slots {
            if other.dock == slot.dock && other.group == slot.group {
                other.selected = other.id == id;
                other.collapsed = collapsed;
            }
        }
    }

    /// Put `id` into `target`'s section, as the tab after it.
    ///
    /// The moved panel comes to the front, because a tab you have just asked
    /// for and cannot see is indistinguishable from one that did not move.
    pub fn group_with(&mut self, id: PanelId, target: PanelId) {
        if self.locked || id == target {
            return;
        }
        let Some(target_slot) = self.slot(target).copied() else {
            return;
        };
        if target_slot.dock == Dock::Hidden {
            return;
        }

        // Everything after the target on that side shuffles down one, so the
        // newcomer has a place in the tab order rather than sharing one.
        for slot in &mut self.slots {
            if slot.dock == target_slot.dock && slot.order > target_slot.order {
                slot.order += 1;
            }
        }
        if let Some(slot) = self.slot_mut(id) {
            slot.dock = target_slot.dock;
            slot.home = target_slot.dock;
            slot.group = target_slot.group;
            slot.order = target_slot.order + 1;
        }
        self.select_tab(id);
    }

    /// Take a panel out of its section into one of its own, directly below.
    pub fn ungroup(&mut self, id: PanelId) {
        if self.locked {
            return;
        }
        let Some(slot) = self.slot(id).copied() else {
            return;
        };
        // A panel already alone in its section has nothing to leave.
        if !self
            .slots
            .iter()
            .any(|s| s.dock == slot.dock && s.group == slot.group && s.id != id)
        {
            return;
        }

        let group = self.next_group(slot.dock);
        if let Some(slot) = self.slot_mut(id) {
            slot.group = group;
            slot.selected = true;
            slot.collapsed = false;
        }
        // The section it left needs a front tab again.
        self.mend_fronts();
    }

    /// Make every panel in a section agree about whether it is rolled up.
    ///
    /// The flag is per panel because that is what a saved layout can express,
    /// but what it *means* is a property of the section — so a file where two
    /// tabs of one stack disagree is mended to whatever the front tab says.
    fn mend_collapse(&mut self) {
        let states: Vec<(Dock, GroupId, bool)> = self
            .slots
            .iter()
            .filter(|s| s.selected)
            .map(|s| (s.dock, s.group, s.collapsed))
            .collect();
        for (dock, group, collapsed) in states {
            for slot in &mut self.slots {
                if slot.dock == dock && slot.group == group {
                    slot.collapsed = collapsed;
                }
            }
        }
    }

    /// A group number nothing on this side is using.
    fn next_group(&self, dock: Dock) -> GroupId {
        self.slots
            .iter()
            .filter(|s| s.dock == dock)
            .map(|s| s.group + 1)
            .max()
            .unwrap_or(0)
    }

    /// Make sure every section has exactly one front tab.
    ///
    /// Called after anything that can leave a section without one — a panel
    /// ungrouped, hidden, or moved to another side.
    fn mend_fronts(&mut self) {
        let keys: Vec<(Dock, GroupId)> = {
            let mut keys: Vec<(Dock, GroupId)> =
                self.slots.iter().map(|s| (s.dock, s.group)).collect();
            keys.sort();
            keys.dedup();
            keys
        };
        for (dock, group) in keys {
            if dock == Dock::Hidden {
                continue;
            }
            let mut members: Vec<(u32, PanelId, bool)> = self
                .slots
                .iter()
                .filter(|s| s.dock == dock && s.group == group)
                .map(|s| (s.order, s.id, s.selected))
                .collect();
            members.sort();
            if members.is_empty() || members.iter().filter(|(_, _, front)| *front).count() == 1 {
                continue;
            }
            let front = members[0].1;
            for slot in &mut self.slots {
                if slot.dock == dock && slot.group == group {
                    slot.selected = slot.id == front;
                }
            }
        }
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
        // **A panel sent to another side arrives in a section of its own.**
        // Joining whatever section happened to carry the same group number
        // over there would group two panels that nobody asked to group, and
        // the second one would vanish behind the first's tab.
        let group = self.next_group(dock);
        if let Some(slot) = self.slot_mut(id) {
            slot.dock = dock;
            slot.order = next;
            slot.group = group;
            slot.selected = true;
            // Somewhere it can be seen is somewhere it can come back to.
            if dock != Dock::Hidden {
                slot.home = dock;
            }
        }
        // The section it left may have lost its front tab.
        self.mend_fronts();
    }

    /// Show a hidden panel, or hide a shown one.
    ///
    /// Allowed while locked: a locked *layout* is about where things are, and
    /// an animator still needs to open the Actions panel and close it again.
    /// Roll a section up to its tab strip, or open it again.
    ///
    /// Whole sections, not single tabs: what is rolled up is the *body* below
    /// the tabs, and there is only one of those however many tabs sit above it.
    pub fn toggle_collapsed(&mut self, id: PanelId) {
        let collapsed = self.is_collapsed(id);
        self.set_collapsed(id, !collapsed);
    }

    /// Roll a section up or open it, whichever is asked for.
    pub fn set_collapsed(&mut self, id: PanelId, collapsed: bool) {
        let Some(slot) = self.slot(id).copied() else {
            return;
        };
        for other in &mut self.slots {
            if other.dock == slot.dock && other.group == slot.group {
                other.collapsed = collapsed;
            }
        }
    }

    /// Is the section this panel is in rolled up to its tabs?
    pub fn is_collapsed(&self, id: PanelId) -> bool {
        self.slot(id).is_some_and(|s| s.collapsed)
    }

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

        // A layout from before panels could be rolled up: adopt which ones
        // start rolled, once. Everything else the user arranged is left alone
        // — their docks, their widths, their order.
        if self.version < VERSION_ROLL_UPS {
            for slot in &mut self.slots {
                if let Some(default) = defaults.slot(slot.id) {
                    slot.collapsed = default.collapsed;
                }
            }
        }

        // **A layout from before panels could be grouped.**
        //
        // `group` defaults to zero, and zero for every panel means *one
        // section holding all of them* — a column that draws a single tab
        // strip and one panel, with everything else apparently gone. So the
        // migration is not optional and cannot be left to serde's default:
        // every saved slot takes the grouping the default arrangement gives
        // it, which puts the five occasional panels in one section and the
        // Library with the Assets panel, and leaves every other panel alone in
        // its own.
        if self.version < VERSION_TAB_GROUPS {
            for slot in &mut self.slots {
                match defaults.slot(slot.id) {
                    // Still where the default workspace put it: take the
                    // default grouping, tabs and all — including whether the
                    // section starts rolled up.
                    //
                    // The roll-up has to come across with the grouping or the
                    // migration produces the one state a tabbed section must
                    // never be in: five panels behind a strip of tabs, rolled
                    // up, showing nothing. The five that start grouped are
                    // exactly the five that used to start rolled up, so a file
                    // from the last build has them all `collapsed: true`.
                    Some(default) if default.dock == slot.dock => {
                        slot.group = default.group;
                        slot.selected = default.selected;
                        slot.collapsed = default.collapsed;
                    }
                    // Moved somewhere of their own: a section to itself, which
                    // is exactly what it was before groups existed.
                    _ => {
                        slot.group = slot.order;
                        slot.selected = true;
                    }
                }
            }
        }
        self.version = LAYOUT_VERSION;

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

        // A hand-edited or half-migrated file must still show a panel in every
        // section rather than an empty body under a tab strip.
        self.mend_fronts();
        self.mend_collapse();

        // Sizes from a corrupt file must not collapse the window — and a
        // column dragged narrower than its contents by an earlier build is
        // brought back to a width its panels fit in, which is the whole point
        // of having a minimum at all.
        self.left_width = clamp_to(self.left_width, LEFT_WIDTH_RANGE);
        self.right_width = clamp_to(self.right_width, COLUMN_WIDTH_RANGE);
        self.right_outer_width = clamp_to(self.right_outer_width, COLUMN_WIDTH_RANGE);
        self.bottom_height = clamp_to(self.bottom_height, BOTTOM_HEIGHT_RANGE);
    }
}

// ---------------------------------------------------------------------------
// Keeping it between runs
// ---------------------------------------------------------------------------

/// Bumped when the *default arrangement* changes in a way a saved layout
/// should adopt. See [`Workspace::version`].
pub const LAYOUT_VERSION: u32 = VERSION_TAB_GROUPS;

/// Panels gained the roll-up, and five of them started rolled.
const VERSION_ROLL_UPS: u32 = 1;
/// Panels gained tab groups, and two sections started tabbed.
const VERSION_TAB_GROUPS: u32 = 2;

/// The home a slot takes when a saved layout predates the field.
fn default_home() -> Dock {
    Dock::Right
}

/// Set once by the application's `main`, so only the program the user launched
/// reads and writes the user's layout.
///
/// See [`claim_user_workspace`].
static IS_THE_APPLICATION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// **Claim the user's real layout file. Called by `main`, and by nothing else.**
///
/// # Why an opt-in rather than a test opt-out
///
/// An `Editor` loads the workspace when it is built and saves it whenever a
/// preference changes, and a *test* that builds one therefore reads and writes
/// the layout of whoever is running the suite. There was a guard for this, in
/// the unit tests' own helper — and it covered the unit tests only. Four
/// integration test binaries build an `Editor` without going near that helper,
/// so `cargo test` quietly rearranged the running user's panels; it was caught
/// when a test run left the Library and the Assets panel floating over the
/// stage.
///
/// Opting *out* in each test is the arrangement that failed: it has to be
/// remembered in every new test file, and forgetting is silent and destructive.
/// Opting *in*, from the one function that means "a person launched this", is a
/// thing you cannot forget, because everything that has not called it gets a
/// scratch file and nothing breaks.
pub fn claim_user_workspace() {
    IS_THE_APPLICATION.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Where the layout is kept.
///
/// Beside the user's other application data, never beside the document: a
/// workspace belongs to the person, not to the film, and a `.buzz` file handed
/// to somebody else must not rearrange their window.
pub fn workspace_path() -> std::path::PathBuf {
    // An explicit override: what a test uses when it wants a layout file of its
    // own to look at, and what anybody wanting a second, separate arrangement
    // can use too.
    if let Some(path) = std::env::var_os("BUZZANIMATE_WORKSPACE") {
        return std::path::PathBuf::from(path);
    }

    // Nobody has claimed the user's layout, so this is not the application:
    // a scratch file per process, which behaves exactly like the real one and
    // is nobody's to lose.
    if !IS_THE_APPLICATION.load(std::sync::atomic::Ordering::Relaxed) {
        return std::env::temp_dir().join(format!(
            "buzzanimate-scratch-workspace-{}.json",
            std::process::id()
        ));
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
        workspace.right_outer_width = f32::NAN;
        workspace.bottom_height = f32::INFINITY;
        workspace.fill_gaps();

        assert!(workspace.right_width >= *COLUMN_WIDTH_RANGE.start());
        assert!(workspace.right_outer_width >= *COLUMN_WIDTH_RANGE.start());
        assert!(workspace.bottom_height <= *BOTTOM_HEIGHT_RANGE.end());
    }

    /// **A column dragged narrower than its contents is widened on load.**
    ///
    /// The minimum used to be 120 points, which is narrower than a Layers row
    /// — so a user who had dragged their right column in was left with panels
    /// whose right-hand end, switches and menus included, was simply not on
    /// screen. Raising the minimum only helps if the layouts already saved
    /// adopt it, which is what this checks.
    #[test]
    fn a_column_saved_narrower_than_its_contents_is_widened() {
        let mut workspace = Workspace::animate();
        // What was actually in a workspace file when this was reported.
        workspace.right_width = 144.0;
        workspace.fill_gaps();

        assert_eq!(workspace.right_width, *COLUMN_WIDTH_RANGE.start());
    }

    /// The default arrangement must not itself be narrower than the minimum,
    /// or a fresh install starts in the state that was reported as broken.
    #[test]
    fn the_default_columns_are_wide_enough_for_what_is_in_them() {
        let workspace = Workspace::animate();
        for (what, width) in [
            ("right", workspace.right_width),
            ("far right", workspace.right_outer_width),
        ] {
            assert!(
                COLUMN_WIDTH_RANGE.contains(&width),
                "the default {what} column is {width} points"
            );
        }
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

    /// **Nothing but the application writes the user's layout.**
    ///
    /// An `Editor` loads the workspace when it is built and saves it whenever a
    /// preference changes, so any test that builds one used to read and write
    /// the layout of whoever was running the suite. It was caught by a test run
    /// leaving the Library and the Assets panel floating over the stage — the
    /// sort of damage that is invisible until somebody opens the program and
    /// finds their window rearranged.
    ///
    /// This test runs in a process that has never called `claim_user_workspace`,
    /// which is true of every test binary and false of `main`.
    #[test]
    fn a_test_process_never_touches_the_real_layout_file() {
        // The override still wins, because a test that wants to look at a file
        // has to be able to name one.
        assert!(
            std::env::var_os("BUZZANIMATE_WORKSPACE").is_some() || {
                let path = workspace_path();
                !path.ends_with("BuzzAnimate/workspace.json")
                    && !path.ends_with("BuzzAnimate\\workspace.json")
            },
            "a test process resolved to the user's own layout file: {}",
            workspace_path().display()
        );
    }

    /// **Reset puts every panel back.** Panels can be docked, undocked,
    /// tabbed, rolled up, resized, closed and locked in place, and there has
    /// to be one button that undoes all of it at once.
    #[test]
    fn resetting_the_layout_puts_every_panel_back() {
        let mut workspace = Workspace::animate();
        let fresh = Workspace::animate();

        // Make a mess of it: float a panel, hide another, roll one up, drag
        // the sides about and lock the lot.
        workspace.move_to(PanelId::Layers, Dock::Float);
        workspace.move_to(PanelId::Tools, Dock::Hidden);
        if let Some(slot) = workspace.slots.iter_mut().find(|s| s.id == PanelId::Color) {
            slot.collapsed = true;
        }
        workspace.left_width += 120.0;
        workspace.bottom_height += 90.0;
        workspace.locked = true;

        workspace.reset_layout();

        assert_eq!(workspace.locked, fresh.locked, "the lock comes off");
        assert_eq!(workspace.left_width, fresh.left_width);
        assert_eq!(workspace.bottom_height, fresh.bottom_height);
        for expected in &fresh.slots {
            let actual = workspace
                .slots
                .iter()
                .find(|s| s.id == expected.id)
                .unwrap_or_else(|| panic!("{:?} went missing", expected.id));
            assert_eq!(
                (actual.dock, actual.collapsed, actual.order),
                (expected.dock, expected.collapsed, expected.order),
                "{:?} did not go back where it started",
                expected.id
            );
        }
    }

    /// **And it keeps what is not layout.** The theme, the settings the next
    /// new document opens with, and the directories to look in for recovered
    /// work all live in this struct and are none of them layout. Resetting by
    /// replacing the whole workspace threw them away with it — losing a theme
    /// is an irritation, losing the only record of where an autosave went is
    /// losing work.
    #[test]
    fn resetting_the_layout_keeps_the_preferences_that_are_not_layout() {
        let mut workspace = Workspace::animate();
        workspace.theme = crate::theme::Theme::Light;
        workspace.recovery_dirs = vec![
            std::path::PathBuf::from("B:/films/one"),
            std::path::PathBuf::from("B:/films/two"),
        ];
        workspace.new_document.width = 1920.0;
        workspace.locked = true;

        workspace.reset_layout();

        assert_eq!(
            workspace.theme,
            crate::theme::Theme::Light,
            "resetting the layout must not change the theme"
        );
        assert_eq!(
            workspace.recovery_dirs.len(),
            2,
            "and must not forget where recovered work is"
        );
        assert_eq!(
            workspace.new_document.width, 1920.0,
            "and must not undo the new-document settings"
        );
        assert!(!workspace.locked, "while still resetting the layout itself");
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

#[cfg(test)]
mod migration_tests {
    use super::*;

    /// **A layout saved before panels could be rolled up adopts the new
    /// arrangement — once, and without disturbing anything else.**
    ///
    /// The failure this prevents is quiet: the user's file has every panel
    /// open, which is what a file from an older build looks like *and* what a
    /// user who opened them all looks like. Without a version to tell them
    /// apart, the fix would never reach anybody who had already run the
    /// program — which is everybody it was reported by.
    #[test]
    fn an_older_layout_adopts_the_new_default_roll_ups() {
        let mut saved = Workspace::animate();
        saved.version = 0;
        // As an older file has it: nothing rolled up, and no groups at all.
        for slot in &mut saved.slots {
            slot.collapsed = false;
            slot.group = 0;
            slot.selected = false;
        }
        // And an arrangement of their own, which must survive.
        saved.right_width = 444.0;
        saved.move_to(PanelId::Sound, Dock::Float);

        saved.fill_gaps();

        assert_eq!(saved.version, LAYOUT_VERSION);
        assert_eq!(saved.right_width, 444.0, "their column width was disturbed");
        assert_eq!(
            saved.dock_of(PanelId::Sound),
            Dock::Float,
            "their arrangement was disturbed"
        );
    }

    /// **A layout saved before panels could be grouped does not collapse into
    /// one section.**
    ///
    /// `group` defaults to zero, so an untouched older file says *every panel
    /// on this side is one tab stack* — a column that would draw a single strip
    /// of tabs and one panel, with the rest apparently gone. This is the one
    /// migration that cannot be skipped, and it is checked on the exact shape
    /// an older file has: no groups, no front tabs.
    #[test]
    fn an_older_layout_does_not_become_one_giant_tab_stack() {
        let mut saved = Workspace::animate();
        saved.version = VERSION_ROLL_UPS;
        for slot in &mut saved.slots {
            slot.group = 0;
            slot.selected = false;
        }

        saved.fill_gaps();

        let right = saved.sections(Dock::Right);
        assert!(
            right.len() > 1,
            "every panel on the right ended up in one section of {} tabs",
            right.first().map(|s| s.panels.len()).unwrap_or(0)
        );
        // And it adopted the arrangement this build ships: the occasional
        // panels as one tabbed section, everything else on its own.
        let utility = right
            .iter()
            .find(|s| s.panels.contains(&PanelId::Depth))
            .expect("the Depth panel is on the right");
        assert!(utility.panels.contains(&PanelId::Sound));
        assert!(utility.is_tabbed());
        assert_eq!(utility.front, PanelId::Depth);

        for id in [PanelId::Layers, PanelId::Properties] {
            let section = right
                .iter()
                .find(|s| s.panels.contains(&id))
                .expect("still on the right");
            assert!(!section.is_tabbed(), "{id:?} was grouped with something");
        }
    }

    /// **The migrated tab section is not rolled up.**
    ///
    /// The five panels that now share a section are exactly the five that used
    /// to start rolled up, so every saved layout from the last build has them
    /// `collapsed: true`. Carried across unchanged that gives a strip of tabs
    /// with nothing under it — the one state a tabbed section must never be in,
    /// and one that looks precisely like five panels that have gone missing.
    #[test]
    fn the_migrated_tab_section_is_not_left_rolled_up() {
        let mut saved = Workspace::animate();
        saved.version = VERSION_ROLL_UPS;
        // As a file from the previous build has it.
        for slot in &mut saved.slots {
            slot.group = 0;
            slot.selected = false;
            slot.collapsed = matches!(
                slot.id,
                PanelId::Depth
                    | PanelId::Rig
                    | PanelId::Filters
                    | PanelId::Lighting
                    | PanelId::Sound
            );
        }

        saved.fill_gaps();

        let section = saved
            .section_of(PanelId::Depth)
            .expect("Layer Depth is on screen");
        assert!(section.is_tabbed());
        assert!(
            !section.collapsed,
            "the new tabbed section arrived rolled up, so all five panels in it \
             show nothing at all"
        );
    }

    /// A panel the user had moved somewhere of its own is not swept into a
    /// group by the migration — it kept its place before, so it keeps it now.
    #[test]
    fn a_panel_the_user_moved_keeps_a_section_to_itself() {
        let mut saved = Workspace::animate();
        saved.version = VERSION_ROLL_UPS;
        for slot in &mut saved.slots {
            slot.group = 0;
            slot.selected = false;
        }
        // Their arrangement: the Filters panel dragged to the left column.
        if let Some(slot) = saved.slot_mut(PanelId::Filters) {
            slot.dock = Dock::Left;
            slot.order = 7;
        }

        saved.fill_gaps();

        let left = saved.sections(Dock::Left);
        let filters = left
            .iter()
            .find(|s| s.panels.contains(&PanelId::Filters))
            .expect("still on the left");
        assert!(
            !filters.is_tabbed(),
            "a panel the user had moved was grouped with the tools"
        );
    }

    /// And it happens once: a section the user rolls up stays rolled up.
    #[test]
    fn a_layout_already_migrated_is_left_alone() {
        let mut saved = Workspace::animate();
        saved.toggle_collapsed(PanelId::Depth); // the user rolls it up
        assert!(saved.is_collapsed(PanelId::Depth));
        // Their own grouping, too.
        saved.ungroup(PanelId::Sound);

        saved.fill_gaps();

        assert!(
            saved.is_collapsed(PanelId::Depth),
            "a section the user rolled up was opened again on the next launch"
        );
        assert!(
            saved
                .section_of(PanelId::Sound)
                .is_some_and(|s| !s.is_tabbed()),
            "a panel the user ungrouped was grouped again on the next launch"
        );
    }

    /// The Library and the Assets panel are both reachable in the default
    /// arrangement — which is the complaint that started this.
    #[test]
    fn the_library_and_the_assets_panel_share_a_column_and_neither_is_hidden() {
        let workspace = Workspace::animate();
        let outer = workspace.on(Dock::RightOuter);
        assert!(outer.contains(&PanelId::Library));
        assert!(outer.contains(&PanelId::Assets));
        for id in [PanelId::Library, PanelId::Assets] {
            assert!(
                !workspace.is_collapsed(id),
                "{id:?} starts rolled up, so it still looks missing"
            );
            assert_ne!(workspace.dock_of(id), Dock::Hidden);
        }
        // And they share it as tabs, which is what makes them one section.
        let section = workspace
            .section_of(PanelId::Library)
            .expect("the Library is on screen");
        assert!(section.panels.contains(&PanelId::Assets));
        assert_eq!(section.front, PanelId::Library);
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    /// The default arrangement puts the five occasional panels in one section,
    /// which is what turns a column of nine into a column of five.
    #[test]
    fn the_occasional_panels_share_one_section() {
        let workspace = Workspace::animate();
        let section = workspace
            .section_of(PanelId::Depth)
            .expect("Layer Depth is on screen");

        for id in [
            PanelId::Depth,
            PanelId::Rig,
            PanelId::Filters,
            PanelId::Lighting,
            PanelId::Sound,
        ] {
            assert!(section.panels.contains(&id), "{id:?} is not in the section");
        }
        assert!(section.is_tabbed());
        assert_eq!(section.front, PanelId::Depth, "no tab is at the front");
        assert!(
            !section.collapsed,
            "a tabbed section that starts rolled up shows nothing at all"
        );

        // Five panels, one section: the right column is five sections, not nine.
        assert_eq!(workspace.sections(Dock::Right).len(), 5);
    }

    /// **Every section has exactly one front tab, always.**
    ///
    /// A section with none draws an empty body under a strip of tabs, which is
    /// indistinguishable from a panel that has stopped working.
    #[test]
    fn every_section_shows_exactly_one_of_its_panels() {
        let mut workspace = Workspace::animate();
        // Put it through everything that can disturb a section.
        workspace.group_with(PanelId::Color, PanelId::Layers);
        workspace.select_tab(PanelId::Color);
        workspace.ungroup(PanelId::Rig);
        workspace.move_to(PanelId::Filters, Dock::Left);
        workspace.toggle(PanelId::Sound);
        workspace.toggle(PanelId::Sound);

        for dock in [Dock::Left, Dock::Right, Dock::RightOuter, Dock::Bottom] {
            for section in workspace.sections(dock) {
                assert!(!section.panels.is_empty(), "an empty section in {dock:?}");
                assert!(
                    section.panels.contains(&section.front),
                    "{dock:?} has a section whose front tab is not in it"
                );
                let fronts = section
                    .panels
                    .iter()
                    .filter(|id| workspace.slot(**id).is_some_and(|s| s.selected))
                    .count();
                assert_eq!(fronts, 1, "{:?} has {fronts} front tabs", section.panels);
            }
        }
    }

    /// Grouping puts one panel into another's section and brings it forward —
    /// a tab you asked for and cannot see is one that did not move.
    #[test]
    fn grouping_joins_a_section_and_comes_to_the_front() {
        let mut workspace = Workspace::animate();
        workspace.group_with(PanelId::Assets, PanelId::Layers);

        let section = workspace
            .section_of(PanelId::Layers)
            .expect("the Layers panel is on screen");
        assert!(section.panels.contains(&PanelId::Assets));
        assert_eq!(section.front, PanelId::Assets);
        assert_eq!(workspace.dock_of(PanelId::Assets), Dock::Right);

        // And the section it left still shows something.
        let outer = workspace.sections(Dock::RightOuter);
        assert_eq!(outer.len(), 1);
        assert_eq!(outer[0].front, PanelId::Library);
    }

    /// Ungrouping gives a tab a section of its own, and leaves the old one
    /// with a front tab.
    #[test]
    fn ungrouping_gives_a_panel_a_section_of_its_own() {
        let mut workspace = Workspace::animate();
        workspace.select_tab(PanelId::Sound);
        workspace.ungroup(PanelId::Sound);

        let sound = workspace.section_of(PanelId::Sound).expect("still on screen");
        assert!(!sound.is_tabbed());
        assert_eq!(sound.front, PanelId::Sound);

        let utility = workspace.section_of(PanelId::Depth).expect("still on screen");
        assert!(!utility.panels.contains(&PanelId::Sound));
        assert!(utility.panels.contains(&utility.front));
    }

    /// A panel alone in its section has nothing to leave, and asking must not
    /// disturb the layout.
    #[test]
    fn ungrouping_a_lone_panel_does_nothing() {
        let mut workspace = Workspace::animate();
        let before = workspace.clone();
        workspace.ungroup(PanelId::Layers);
        assert_eq!(workspace, before);
    }

    /// Clicking a tab is not moving a panel, so it works while locked — the
    /// same rule that lets a locked layout still open and close a panel.
    #[test]
    fn a_locked_layout_still_lets_tabs_be_clicked_but_not_regrouped() {
        let mut workspace = Workspace::animate();
        workspace.locked = true;

        workspace.select_tab(PanelId::Sound);
        assert_eq!(
            workspace.section_of(PanelId::Sound).map(|s| s.front),
            Some(PanelId::Sound),
            "a tab could not be brought forward in a locked layout"
        );

        let before = workspace.clone();
        workspace.group_with(PanelId::Layers, PanelId::Properties);
        workspace.ungroup(PanelId::Sound);
        assert_eq!(workspace, before, "a locked layout was regrouped");
    }

    /// Rolling up a section rolls up all of it: what is hidden is the one body
    /// under the tabs, and a section half rolled up is not a state that exists.
    #[test]
    fn rolling_up_a_section_takes_every_tab_with_it() {
        let mut workspace = Workspace::animate();
        workspace.toggle_collapsed(PanelId::Depth);

        for id in [PanelId::Depth, PanelId::Rig, PanelId::Sound] {
            assert!(workspace.is_collapsed(id), "{id:?} disagreed");
        }
        assert!(
            workspace
                .section_of(PanelId::Rig)
                .is_some_and(|s| s.collapsed)
        );

        // And clicking a tab in a rolled-up section leaves it rolled up.
        workspace.select_tab(PanelId::Sound);
        assert!(
            workspace
                .section_of(PanelId::Sound)
                .is_some_and(|s| s.collapsed),
            "clicking a tab silently opened the section"
        );
    }

    /// Moving a panel to another side gives it a section of its own there —
    /// it must not silently join whatever group number it happens to match.
    #[test]
    fn a_panel_moved_to_another_side_lands_in_its_own_section() {
        let mut workspace = Workspace::animate();
        workspace.move_to(PanelId::Sound, Dock::RightOuter);

        let section = workspace.section_of(PanelId::Sound).expect("on screen");
        assert!(
            !section.is_tabbed(),
            "it joined {:?} without being asked",
            section.panels
        );
        // The Library and Assets are still together, and still showing one.
        let library = workspace.section_of(PanelId::Library).expect("on screen");
        assert!(library.panels.contains(&PanelId::Assets));
    }

    /// A group survives being undocked: the section floats as one window.
    #[test]
    fn a_grouped_section_can_be_floated_together() {
        let mut workspace = Workspace::animate();
        workspace.group_with(PanelId::Assets, PanelId::Layers);
        for id in [PanelId::Layers, PanelId::Assets] {
            workspace.move_to(id, Dock::Float);
        }
        // Moved one at a time they each get their own section, which is right:
        // `move_to` is "send this panel there", not "send its section there".
        // Re-grouping them floats them as one window.
        workspace.group_with(PanelId::Assets, PanelId::Layers);

        let floating = workspace.sections(Dock::Float);
        assert_eq!(floating.len(), 1, "two windows, not one");
        assert_eq!(floating[0].panels.len(), 2);
    }

    /// Every panel is reachable: no panel may be left in a section that never
    /// shows it, and every open panel is in exactly one section.
    #[test]
    fn every_open_panel_belongs_to_exactly_one_section() {
        let workspace = Workspace::animate();
        for id in PanelId::ALL {
            if !workspace.is_open(id) {
                continue;
            }
            let found: Vec<_> = [Dock::Left, Dock::Right, Dock::RightOuter, Dock::Bottom]
                .into_iter()
                .flat_map(|d| workspace.sections(d))
                .filter(|s| s.panels.contains(&id))
                .collect();
            assert_eq!(found.len(), 1, "{id:?} is in {} sections", found.len());
        }
    }
}
