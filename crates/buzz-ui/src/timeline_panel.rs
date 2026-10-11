//! The timeline panel.
//!
//! Layer rows on the left, a frame grid on the right, sharing a vertical
//! scroll. The grid uses Animate's drawing conventions exactly, because they
//! are how an animator reads a timeline at a glance:
//!
//! | Drawn | Meaning |
//! |---|---|
//! | Filled circle | Keyframe with artwork |
//! | Hollow circle | Blank keyframe |
//! | Shaded cell | Continues the keyframe before it |
//! | Hollow rectangle | Last frame of a span |
//! | Empty cell | The layer does not reach here |

use buzz_scene::{FrameKind, LayerId, LayerKind, Light, LightId, LoopRegion, MAX_REPEATS, Scene};
use egui::{Align2, Color32, FontId, Sense, Shape, Stroke, StrokeKind, Ui};

use crate::theme::{Metrics, Palette, Tokens};

/// What the user did in the timeline.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TimelineResponse {
    /// The playhead was moved here.
    pub scrub_to: Option<u32>,
    /// The camera row was clicked: select the camera, as Animate does when you
    /// click its layer.
    pub select_camera: bool,
    /// The camera row's eye switch was clicked — the same on/off `Command::ToggleCamera`
    /// raises from the layer-tools strip. C1.
    pub toggle_camera: bool,
    /// The camera row's padlock switch was clicked. C1.
    pub toggle_camera_lock: bool,
    /// A layer row was clicked.
    pub select_layer: Option<LayerId>,
    /// Multiple layers were selected (e.g. from vertical drag across frames on a column).
    pub select_layers: Option<Vec<LayerId>>,
    /// I1: the range anchor should move to this layer — a plain click or a
    /// Ctrl/Cmd+click, either of which becomes what the *next* Shift+click
    /// sweeps from. `None` when the click was a Shift+click, which sweeps
    /// from the existing anchor and leaves it where it was.
    pub set_range_anchor: Option<LayerId>,
    /// I2: a folder's disclosure triangle was clicked — collapse it if it was
    /// expanded, expand it if it was collapsed.
    pub toggle_collapsed: Option<LayerId>,
    /// The exact rectangular region selected in the frame grid.
    pub select_cells: Option<crate::TimelineSelection>,
    /// The frame selection was dragged somewhere new: move it there (or copy
    /// it, with Alt), along its rows and onto other layers. See
    /// `docs/TIMELINE_FRAME_MOVE_PLAN.md` M3 and M6.
    pub move_frames: Option<MoveFrames>,
    /// A layer was dragged to a new position in the layer stack: (layer_to_move, to_index).
    ///
    /// Superseded by [`Self::move_layer`] for the timeline's own name-column
    /// drag (F1), which carries a folder as well as a row; kept for any other
    /// producer that only ever meant a plain reorder.
    pub reorder_layer: Option<(LayerId, usize)>,
    /// A layer — and everything filed under it — was dragged to a new row
    /// and parent folder: `(layer_to_move, to_index, new_parent)`. F1. See
    /// [`buzz_scene::LayerStack::move_to`].
    pub move_layer: Option<(LayerId, usize, Option<LayerId>)>,
    /// A light's channel was clicked: make it the selected light.
    pub select_light: Option<LightId>,
    /// A light's row was dragged to a new place in the layer stack: directly
    /// above this layer, or (`None`) at the top. See `Scene::place_light`.
    pub place_light: Option<(LightId, Option<LayerId>)>,
    /// Add a keyframe to this light at this frame (from the channel's menu).
    pub light_key: Option<(LightId, u32)>,
    /// Remove this light's keyframe at this frame.
    pub light_unkey: Option<(LightId, u32)>,
    /// The parenting view was switched on or off.
    pub toggle_parenting: bool,
    /// The depth view was switched on or off.
    pub toggle_depth: bool,
    /// A layer's depth was dragged in the depth view.
    pub set_depth: Option<(LayerId, f64)>,
    /// A parent link was made or broken in the parenting view: the child, and
    /// the layer it should now follow (`None` to detach it).
    pub set_follows: Option<(LayerId, Option<LayerId>)>,
    /// One of the three switches beside a layer's name was clicked.
    ///
    /// The timeline carries Animate's eye, padlock and outline columns beside
    /// its layer names, and they are the same three switches the Layers panel
    /// draws — the same painted icons, the same words, and the same edit. Two
    /// places to reach them, one meaning.
    pub toggle_layer: Option<(LayerId, crate::panels::LayerIcon)>,
    /// A column heading above the layers was clicked: flip that switch on
    /// **every** layer at once — Animate's show/hide-all, lock-all, outline-all.
    pub toggle_all: Option<crate::panels::LayerIcon>,
    /// A highlight colour was set (`Some`) or cleared (`None`, "Remove
    /// Highlight") on every layer named here — Animate's Highlight Layer.
    /// One entry for a single layer's own "Highlight" submenu; every
    /// selected layer for "Highlight Selected Layers". H2.
    pub set_highlight: Option<(Vec<LayerId>, Option<peniko::Color>)>,
    /// A frame operation was requested.
    pub action: Option<FrameAction>,
    /// A tween was created or removed from the frame menu.
    pub tween: Option<TweenRequest>,
    pub toggle_play: bool,
    pub toggle_onion: bool,
    pub toggle_auto_keyframe: bool,
    pub toggle_edit_multiple: bool,
    /// The onion markers were changed: frames before, frames after.
    pub set_onion_range: Option<(u32, u32)>,
    pub go_to_start: bool,
    pub go_to_end: bool,
    pub step: i64,
    /// The looping section was changed. Carries the whole region rather than a
    /// delta, so one undo step covers whatever the user just did to it.
    pub set_loop: Option<LoopRegion>,
    /// The document was made longer or shorter, in frames.
    pub set_frame_count: Option<u32>,
    /// A command raised by the timeline's own buttons — the layer tools under
    /// the layer names. Carried as a command so it runs through the same path
    /// as the menu item and the shortcut, undo included.
    pub command: Option<crate::Command>,
    /// The frame cells were made wider or narrower.
    ///
    /// Not undoable, and deliberately: how big the timeline is drawn is not a
    /// change to the film, and having it in the undo history would mean Ctrl+Z
    /// after a zoom undid the zoom rather than the last edit.
    pub set_frame_width: Option<f32>,
    /// The rows were made taller or shorter.
    pub set_row_scale: Option<f32>,
}

/// Frame operations, matching Animate's shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameAction {
    /// F5
    InsertFrame,
    /// Shift+F5
    RemoveFrame,
    /// F6
    InsertKeyframe,
    /// F7
    InsertBlankKeyframe,
    /// Shift+F6
    ClearKeyframe,
}

impl FrameAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::InsertFrame => "Insert Frame",
            Self::RemoveFrame => "Remove Frame",
            Self::InsertKeyframe => "Insert Keyframe",
            Self::InsertBlankKeyframe => "Insert Blank Keyframe",
            Self::ClearKeyframe => "Clear Keyframe",
        }
    }

    pub fn shortcut_text(self) -> &'static str {
        match self {
            Self::InsertFrame => "F5",
            Self::RemoveFrame => "Shift+F5",
            Self::InsertKeyframe => "F6",
            Self::InsertBlankKeyframe => "F7",
            Self::ClearKeyframe => "Shift+F6",
        }
    }
}

/// A tween asked for from the frame menu.
///
/// Kept separate from [`crate::Command`] for the same reason [`FrameAction`]
/// is: the panel describes what the user did to a frame, and the shell decides
/// which command that becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TweenRequest {
    Motion,
    Shape,
    Classic,
    Remove,
}

impl TweenRequest {
    pub fn label(self) -> &'static str {
        match self {
            Self::Motion => "Create Motion Tween",
            Self::Shape => "Create Shape Tween",
            Self::Classic => "Create Classic Tween",
            Self::Remove => "Remove Tween",
        }
    }
}

/// State of an active vertical drag across frames on a column.
#[derive(Clone, Copy, Debug, Default)]
struct FrameVerticalDrag {
    start_y: f32,
    start_frame: u32,
    current_frame: u32,
    anchor_layer: Option<LayerId>,
}

/// Where a dragged frame selection should land: [`TimelineResponse::move_frames`].
/// See `docs/TIMELINE_FRAME_MOVE_PLAN.md` M3 and M6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveFrames {
    /// The frame the selection's first frame lands on.
    pub to: u32,
    /// Layer rows down the timeline (negative: up) — onto other layers.
    pub rows: i32,
    /// Alt was held on release: copy, leaving the source as it was.
    pub copy: bool,
}

/// Temp-data key, with a [`LayerId`], of a drawn layer row's geometry:
/// `(row rect, x of frame 0's left edge, cell width)`. Recorded every frame a
/// row is drawn so a headless test can press on a given frame cell.
pub const ROW_GEOMETRY: &str = "timeline-row-geometry";

/// Temp-data key of the [`FrameMoveDrag`] in progress.
const FRAME_MOVE_DRAG: &str = "timeline_frame_move_drag";

/// State of a **frame move drag**: a press inside the frame selection,
/// dragged — Animate's way of moving frames. Kept apart from
/// [`FrameVerticalDrag`], which is the selection rectangle.
#[derive(Clone, Copy, Debug)]
struct FrameMoveDrag {
    /// The frame the button went down on.
    press_frame: u32,
    /// The frame under the pointer now.
    current_frame: u32,
    /// The row the button went down on.
    press_layer: LayerId,
    /// The selection's first frame, which `to` is measured from.
    first_frame: u32,
    /// Layer rows between the press and the row under the pointer, kept
    /// while the pointer crosses a light row or a gap.
    rows: i32,
}

impl FrameMoveDrag {
    /// The frame the selection's first frame would land on: moved by the
    /// pointer's offset, never before frame 0.
    fn destination(&self) -> u32 {
        let offset = self.current_frame as i64 - self.press_frame as i64;
        (self.first_frame as i64 + offset).max(0) as u32
    }
}

/// The timeline's layer rows, in display order — top to bottom, skipping a
/// row folded away inside a collapsed folder. The rows a frame drag counts.
fn display_rows(scene: &Scene) -> Vec<LayerId> {
    scene
        .layers()
        .iter()
        .map(|l| l.id)
        .filter(|id| !scene.layers().is_row_hidden(*id))
        .collect()
}

/// **M6**: where each selected layer's frames go when a frame drag moves
/// them `rows` layer rows down (negative: up) — `(source, target)` pairs.
///
/// Sources that cannot move — a folder, a locked layer, one that is gone —
/// are left out, as every frame operation skips them. Along the same rows
/// (`rows == 0`) every source is its own target. Otherwise each target is the
/// display row `rows` away, and it must exist, hold frames and be unlocked:
/// if **any** is not, the whole move is refused with the reason, because a
/// move that lands on some layers and not others is worse than none. The
/// timeline draws its ghost from this and the editor moves by it, so the two
/// cannot disagree.
pub fn frame_move_targets(
    scene: &Scene,
    layers: impl IntoIterator<Item = LayerId>,
    rows: i32,
) -> Result<Vec<(LayerId, LayerId)>, String> {
    let stack = scene.layers();
    let sources: Vec<LayerId> = layers
        .into_iter()
        .filter(|&id| {
            stack.get(id).is_some_and(|l| l.kind.holds_artwork())
                && !stack.is_effectively_locked(id)
        })
        .collect();
    if rows == 0 {
        return Ok(sources.into_iter().map(|id| (id, id)).collect());
    }
    let display = display_rows(scene);
    let mut pairs = Vec::with_capacity(sources.len());
    for source in sources {
        let Some(index) = display.iter().position(|id| *id == source) else {
            return Err("a selected layer is folded away in a collapsed folder".into());
        };
        let target = index as i64 + rows as i64;
        let Some(&target) = usize::try_from(target).ok().and_then(|i| display.get(i)) else {
            return Err("there is no layer row there".into());
        };
        let Some(layer) = stack.get(target) else {
            return Err("there is no layer row there".into());
        };
        if !layer.kind.holds_artwork() {
            return Err(format!("\u{201C}{}\u{201D} is a folder, which holds no frames", layer.name));
        }
        if stack.is_effectively_locked(target) {
            return Err(format!("\u{201C}{}\u{201D} is locked", layer.name));
        }
        pairs.push((source, target));
    }
    Ok(pairs)
}

/// One layer row's part in a [`FrameMoveDrag`]: follow the pointer, draw the
/// ghost where this row's frames would land, and on release say where the
/// selection goes.
///
/// The ghost is outlined in the accent colour, or in the danger colour when
/// [`frame_move_targets`] refuses the move — the editor then refuses it too,
/// with the reason on the status line.
#[allow(clippy::too_many_arguments)]
fn frame_move_drag(
    ui: &Ui,
    scene: &Scene,
    grid: &egui::Painter,
    layer: LayerId,
    rect: egui::Rect,
    grid_left: f32,
    cell_width: f32,
    columns: u32,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    let id = egui::Id::new(FRAME_MOVE_DRAG);
    let Some(mut drag) = ui.ctx().data(|d| d.get_temp::<FrameMoveDrag>(id)) else {
        return;
    };
    let Some(selection) = state.timeline_selection.as_ref() else {
        return;
    };
    let display = display_rows(scene);
    let row_of = |id: LayerId| display.iter().position(|l| *l == id);
    if let Some(pos) = ui
        .ctx()
        .input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()))
    {
        drag.current_frame = (((pos.x - grid_left) / cell_width).floor().max(0.0) as u32)
            .min(columns.saturating_sub(1));
        if pos.y >= rect.min.y
            && pos.y < rect.max.y
            && let (Some(here), Some(pressed)) = (row_of(layer), row_of(drag.press_layer))
        {
            drag.rows = here as i32 - pressed as i32;
        }
        ui.ctx().data_mut(|d| d.insert_temp(id, drag));
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);

    let to = drag.destination();
    let moving = to != drag.first_frame || drag.rows != 0;
    // The ghost goes on this row when a selected layer's frames land here.
    let lands_here = row_of(layer)
        .and_then(|here| usize::try_from(here as i64 - drag.rows as i64).ok())
        .and_then(|from| display.get(from))
        .is_some_and(|from| selection.layers.contains(from));
    if moving && lands_here {
        let refused =
            frame_move_targets(scene, selection.layers.iter().copied(), drag.rows).is_err();
        let count = selection.last_frame - selection.first_frame + 1;
        let ghost = egui::Rect::from_min_size(
            egui::pos2(grid_left + to as f32 * cell_width, rect.min.y),
            egui::vec2(cell_width * count as f32, rect.height()),
        );
        let colour = if refused {
            Palette::danger()
        } else {
            Palette::active()
        };
        grid.rect_stroke(ghost, 0.0, Stroke::new(2.0, colour), StrokeKind::Inside);
    }

    if ui.input(|i| i.pointer.any_released()) {
        if moving {
            let copy = ui.input(|i| i.modifiers.alt);
            out.move_frames = Some(MoveFrames {
                to,
                rows: drag.rows,
                copy,
            });
        } else if layer == drag.press_layer {
            // Let go where it started: a plain click on that cell, as before.
            out.select_cells = Some(crate::TimelineSelection::cell(layer, drag.press_frame));
            out.scrub_to = Some(drag.press_frame);
        }
    }
}

/// **M1**: what a layer row's context menu is about, captured the instant it
/// opens (`response.secondary_clicked()`) rather than read from the live
/// pointer inside the menu closure, which egui re-runs every frame the popup
/// stays open — so a pointer travelling toward the popup itself no longer
/// re-picks the frame or the column under it. See
/// `docs/TIMELINE_LIGHTS_FOLDERS_PLAN.md` M1.
#[derive(Clone, Copy, Debug, Default)]
struct MenuTarget {
    /// The frame under the pointer at open, if it was over the frame grid.
    frame: Option<u32>,
    /// The pointer was over the pinned layer-name column, not the grid.
    in_name_column: bool,
}

/// State the panel needs but does not own.
pub struct TimelineState {
    pub current_frame: u32,
    pub active_layer: Option<LayerId>,
    /// All layers currently selected in the timeline.
    pub selected_layers: Vec<LayerId>,
    /// I1: the last plain-clicked layer, which a Shift+click ranges from. See
    /// `crate::selection::Selection::range_anchor`.
    pub range_anchor: Option<LayerId>,
    pub timeline_selection: Option<crate::TimelineSelection>,
    /// The camera row is the selected one, so the camera is what the Camera
    /// menu's keyframe commands act on.
    pub camera_selected: bool,
    /// Which light's channel is highlighted — the one the light keyframe
    /// commands act on, and the one whose row is always shown so it can be keyed.
    pub selected_light: Option<LightId>,
    pub playing: bool,
    pub onion_enabled: bool,
    /// Auto Keyframe: changing artwork at a frame with no keyframe of its own
    /// makes one first.
    pub auto_keyframe: bool,
    /// Edit Multiple Frames: every keyframe in the onion range is editable.
    pub edit_multiple: bool,
    /// The onion markers: frames covered before and after the playhead. Shared
    /// by onion skinning and Edit Multiple Frames, as in Animate.
    pub onion_before: u32,
    pub onion_after: u32,
    /// How wide one frame cell is drawn, and how tall a row is relative to the
    /// standard one. Both come from the workspace, so they survive a restart.
    pub frame_width: f32,
    pub row_scale: f32,
    /// Show Animate's **parenting view** in the layer column: a node per layer,
    /// wired to the layer it follows.
    pub parenting_view: bool,
    /// Show the **layer depth** in the layer column: how far each layer sits
    /// from the camera, on a scale against the focal distance.
    pub depth_view: bool,
    /// The camera's focal distance, which is what a layer's depth is measured
    /// against. Supplied so the column can draw the scale.
    pub focal_distance: f64,
    /// As near the camera as a layer may be dragged —
    /// [`buzz_scene::CameraTrack::nearest_depth`]. Supplied rather than derived
    /// so the column and the Layer Depth panel cannot disagree about it.
    pub nearest_depth: f64,
    /// Loudness per frame for each layer carrying a sound, so the timeline can
    /// draw the waveform where the sound actually sits.
    ///
    /// Supplied by the editor rather than read from the document, because the
    /// document stores the *file* and the envelope only exists once it has
    /// been decoded — which is view state, and belongs outside the model.
    pub waveforms: std::collections::BTreeMap<LayerId, Waveform>,
    /// Detected beat frames, drawn as ticks on the ruler. Empty until the user
    /// runs Detect Beats.
    pub beats: Vec<u32>,
    /// **The active scene's playing length, if it is a Shot.** `None` for
    /// every other kind of scene, which plays its whole `frame_count`.
    ///
    /// A Shot's frames past this are kept, not deleted — moving a cut or
    /// shortening a shot never throws animation away, see
    /// `docs/SEQUENCE_SETUPS_PLAN.md` "Setups are stamped, never linked" —
    /// so the timeline still shows them, greyed, past an end marker at this
    /// frame, and playback stops or loops here rather than running on.
    pub shot_end: Option<u32>,
}

/// A sound's envelope, positioned on the timeline.
///
/// `levels` is an `Arc` so the editor's waveform cache can hand the same
/// envelope to the panel every frame without re-deriving or copying it — the
/// per-frame cost this timeline used to pay from raw PCM. See
/// `Editor::waveforms`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Waveform {
    /// The frame the sound starts on.
    pub start_frame: u32,
    /// Loudness per frame of the **whole clip**, `0.0..=1.0`. Shared across
    /// layers and memoised, so it is not sliced to the trim — `level_start` and
    /// `level_end` say which part of it to draw instead.
    pub levels: std::sync::Arc<Vec<f32>>,
    /// First level to draw — the in-point, in clip-frames. The strip is laid
    /// out from `start_frame`, reading `levels[level_start..]`.
    pub level_start: usize,
    /// One past the last level to draw — the out-point in clip-frames, or the
    /// clip's end when `None`.
    pub level_end: Option<usize>,
}

/// Width reserved for the layer-name column.
///
/// Wider than it was by the width of the three switch columns, so that adding
/// them took the room from the window rather than from the layer names.
const LAYER_COLUMN: f32 = 210.0;

/// H2: the width of a highlighted layer's left-edge bar, in both the
/// timeline's name column and the Layers panel's rows.
const HIGHLIGHT_BAR_WIDTH: f32 = 4.0;

/// I2: width reserved for the folder disclosure triangle, left of every
/// row's name. Reserved on every row, not only folders', so a folder's name
/// and a plain layer's name still line up.
const DISCLOSURE_WIDTH: f32 = 12.0;

/// I2: the disclosure triangle itself — right-pointing when collapsed, down
/// when expanded, Animate's own convention — painted at `rect`'s centre and
/// sensing the click that toggles it. Returns whether it was clicked this
/// frame; drawn on folder rows only, but `rect` is reserved on every row (see
/// [`DISCLOSURE_WIDTH`]) so the call site can skip drawing without leaving a
/// gap where the name shifts.
fn disclosure_triangle(ui: &Ui, painter: &egui::Painter, rect: egui::Rect, id: egui::Id, collapsed: bool) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    let color = if response.hovered() {
        Palette::text()
    } else {
        Palette::text_dim()
    };
    let c = rect.center();
    let pts = if collapsed {
        // Pointing right: there is more here than the row shows.
        vec![
            egui::pos2(c.x - 3.0, c.y - 4.0),
            egui::pos2(c.x - 3.0, c.y + 4.0),
            egui::pos2(c.x + 4.0, c.y),
        ]
    } else {
        // Pointing down: everything under this folder is already shown.
        vec![
            egui::pos2(c.x - 4.0, c.y - 3.0),
            egui::pos2(c.x + 4.0, c.y - 3.0),
            egui::pos2(c.x, c.y + 4.0),
        ]
    };
    painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
    response.clicked()
}

/// H2: the alpha of a highlighted layer's colour wash — roughly 22% of
/// `0xFF`, muted enough to mark a row without drowning what is drawn on it.
const HIGHLIGHT_WASH_ALPHA: u8 = 56;
/// The alpha of the selected layer's wash across its frames — fainter than a
/// highlight colour's, so a highlighted *and* selected row still shows both.
const ACTIVE_ROW_WASH_ALPHA: u8 = 34;

/// The height of the New Layer / New Folder / Delete strip under the layer
/// names — see its own `egui::Panel::bottom` call in [`timeline_panel`] for
/// why this is claimed before the frame grid's scroll area rather than after
/// (defects brief item 3b: a footer reserved too late overlapped the last
/// visible layer). Module level, not local to the function, so a test can
/// check the grid never renders into the room this reserves.
const LAYER_TOOLS_HEIGHT: f32 = Tokens::CONTROL_H + 4.0;

/// The layer-names column background — the panel's own colour, so the pinned
/// column reads as chrome that belongs to whichever theme is current rather
/// than a fixed dark block sitting on a light interface. It is opaque so the
/// horizontally scrolling frame grid never shows through the pinned names.
fn names_bg() -> Color32 {
    Palette::panel()
}

/// Gap between the three switch columns, and from the frame grid.
///
/// Five rather than three because the *headings* are what set this: `show` and
/// `lock` at eight points are each about as wide as the fifteen-point column
/// they name, and at a three-point gap they ran together into `showlock`.
const SWITCH_GAP: f32 = 5.0;

/// **Where Animate's three layer columns sit in the timeline's name column.**
///
/// Returned in [`LayerIcon::ALL`] order — eye, padlock, outline — running left
/// to right and ending just short of the frame grid, which is where Animate
/// rules them and where the hand goes looking.
///
/// One function rather than arithmetic repeated in the row, the heading and the
/// hit test: three copies of a layout that has to agree to the pixel is three
/// chances for a switch to be drawn somewhere other than where clicking it
/// works.
fn switch_columns(row: egui::Rect) -> [egui::Rect; 3] {
    let side = crate::panels::LAYER_SWITCH;
    // The rightmost column ends a gap short of the grid.
    let right = row.min.x + LAYER_COLUMN - SWITCH_GAP;
    let top = row.center().y - side * 0.5;
    std::array::from_fn(|i| {
        // `i` counts from the left, and there are three of them.
        let x = right - (3 - i) as f32 * (side + SWITCH_GAP) + SWITCH_GAP;
        egui::Rect::from_min_size(egui::pos2(x, top), egui::vec2(side, side))
    })
}

/// Which switch is under this point, if any.
fn switch_at(row: egui::Rect, pos: egui::Pos2) -> Option<crate::panels::LayerIcon> {
    switch_columns(row)
        .iter()
        .zip(crate::panels::LayerIcon::ALL)
        .find(|(rect, _)| rect.contains(pos))
        .map(|(_, icon)| icon)
}

/// Where the layer's name may be drawn: everything left of the switches.
fn name_area(row: egui::Rect) -> egui::Rect {
    let switches = switch_columns(row);
    egui::Rect::from_min_max(
        row.min,
        egui::pos2(
            switches[0].left() - crate::panels::CHIP_WIDTH - SWITCH_GAP * 2.0,
            row.max.y,
        ),
    )
}

/// How many folders deep a layer is filed — Animate indents each level under
/// its folder by the same amount, so a layer inside a folder inside a folder
/// reads two steps in, not one. Distinct from [`parenting::generation`],
/// which walks [`buzz_scene::Layer::follows`] (the rig) rather than
/// [`buzz_scene::Layer::parent`] (the timeline's own filing) and is used only
/// by the parenting view.
///
/// Cycle-guarded the same way `parenting::generation` is: `parent` is data in
/// the scene file, and a corrupted chain that loops back on itself must stop
/// rather than hang the timeline.
fn folder_depth(scene: &Scene, id: LayerId) -> usize {
    let layers = scene.layers();
    let mut seen = Vec::new();
    let mut current = layers.get(id).and_then(|l| l.parent);
    for _ in 0..layers.len() {
        let Some(next) = current else { break };
        if seen.contains(&next) {
            break;
        }
        seen.push(next);
        current = layers.get(next).and_then(|l| l.parent);
    }
    seen.len()
}

/// **F1: which of a row's three drop bands a dragged layer's pointer is
/// over**, deciding where it lands. See `docs/TIMELINE_LIGHTS_FOLDERS_PLAN.md`
/// F1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropZone {
    /// Above this row, at its own nesting level.
    Above,
    /// Below this row, at its own nesting level (or, dropped below a
    /// **Folder**, as its first child — see `drop_zone`).
    Below,
    /// Into this row — a **Folder** only.
    Into,
}

/// **F1's drop-zone decision, as a pure fn:** the top and bottom quarters of
/// a row always mean "beside it"; the middle half means "into it" for a
/// folder, and the nearest of above/below for anything else, since a plain
/// layer has nothing to hold a drop.
///
/// `fraction` is how far down the row's own height the pointer sits,
/// `0.0` at the top to `1.0` at the bottom.
fn drop_zone(fraction: f32, is_folder: bool) -> DropZone {
    if fraction < 0.25 {
        DropZone::Above
    } else if fraction >= 0.75 {
        DropZone::Below
    } else if is_folder {
        DropZone::Into
    } else if fraction < 0.5 {
        DropZone::Above
    } else {
        DropZone::Below
    }
}

/// **Animate's parenting view.**
///
/// The layer column stops being a list of names and becomes a node graph: one
/// node per layer, indented by how deep it sits in its parent chain, with a
/// connector running from each child to the layer it follows. Dragging one node
/// onto another parents it; dropping on empty column detaches it.
///
/// # Why a graph rather than the dropdown
///
/// A rig is a *shape* — a spine with limbs hanging off it — and the question an
/// animator asks of it is "what moves when I move this?". A per-layer dropdown
/// can answer that only one layer at a time, so reading a twelve-layer
/// character means opening twelve menus and holding the answer in your head.
/// The whole point of Animate's view is that the shape is on screen.
mod parenting {
    use super::*;

    /// How far in from the left the first node sits.
    pub const INSET: f32 = 14.0;
    /// How much further right each generation is drawn.
    pub const STEP: f32 = 18.0;
    pub const RADIUS: f32 = 5.0;

    /// How many layers this one follows, up to the root.
    ///
    /// Bounded by the layer count: a file edited by hand can hold a cycle, and
    /// this must terminate rather than hang the timeline.
    pub fn generation(scene: &Scene, id: LayerId) -> usize {
        let layers = scene.layers();
        let mut seen = Vec::new();
        let mut current = layers.get(id).and_then(|l| l.follows);
        for _ in 0..layers.len() {
            let Some(next) = current else { break };
            if seen.contains(&next) {
                break;
            }
            seen.push(next);
            current = layers.get(next).and_then(|l| l.follows);
        }
        seen.len()
    }

    /// Where a layer's node is drawn within its row.
    pub fn node_centre(name_row: egui::Rect, generation: usize) -> egui::Pos2 {
        egui::pos2(
            name_row.min.x + INSET + generation as f32 * STEP,
            name_row.center().y,
        )
    }

    /// The link being dragged, remembered across rows and frames.
    ///
    /// In egui's own store rather than in [`TimelineState`] because it is
    /// transient interaction state that belongs to the widget: the app has no
    /// use for a half-finished drag, and threading it through the response
    /// would make every caller carry it.
    pub fn dragging(ctx: &egui::Context) -> Option<LayerId> {
        ctx.data(|d| d.get_temp::<LayerId>(egui::Id::new("timeline-parent-drag")))
    }

    pub fn set_dragging(ctx: &egui::Context, layer: Option<LayerId>) {
        let id = egui::Id::new("timeline-parent-drag");
        match layer {
            Some(l) => ctx.data_mut(|d| {
                d.insert_temp(id, l);
            }),
            None => ctx.data_mut(|d| d.remove::<LayerId>(id)),
        }
    }

    /// Every node drawn this frame, so the connectors can be drawn over them
    /// once the rows are done. A row scrolled out of view is not in here, and
    /// a connector to it is simply not drawn — it is off screen anyway.
    pub fn nodes(ctx: &egui::Context) -> Vec<(LayerId, egui::Pos2)> {
        ctx.data(|d| {
            d.get_temp::<Vec<(LayerId, egui::Pos2)>>(egui::Id::new("timeline-parent-nodes"))
                .unwrap_or_default()
        })
    }

    pub fn record_node(ctx: &egui::Context, layer: LayerId, at: egui::Pos2) {
        let id = egui::Id::new("timeline-parent-nodes");
        ctx.data_mut(|d| {
            let mut all = d
                .get_temp::<Vec<(LayerId, egui::Pos2)>>(id)
                .unwrap_or_default();
            all.retain(|(other, _)| *other != layer);
            all.push((layer, at));
            d.insert_temp(id, all);
        });
    }

    pub fn clear_nodes(ctx: &egui::Context) {
        ctx.data_mut(|d| {
            d.insert_temp::<Vec<(LayerId, egui::Pos2)>>(
                egui::Id::new("timeline-parent-nodes"),
                Vec::new(),
            )
        });
    }

    /// What a drop means.
    ///
    /// `None` for *nothing happened*; `Some(None)` to detach; `Some(Some(id))`
    /// to follow that layer. A pure decision so the destructive half of it can
    /// be tested without driving a pointer.
    pub fn drop_decision(
        layers: &buzz_scene::LayerStack,
        dragged: LayerId,
        dropped_on: Option<LayerId>,
    ) -> Option<Option<LayerId>> {
        match dropped_on {
            // **Back where it started.** A press that barely moves still ends
            // as a drag, and reading that as "detach" meant a click on a node
            // cut the very link it was drawn to show.
            Some(other) if other == dragged => None,
            Some(other) if layers.can_follow(dragged, other) => Some(Some(other)),
            // An illegal target, or open column: detach. Dropping a node into
            // space is how a limb comes off a rig.
            _ => Some(None),
        }
    }

    /// The elbow from a child's node up to its parent's.
    ///
    /// Drawn as a curve rather than a straight line so that two links crossing
    /// the same rows stay tellable apart, which is the whole reason to draw
    /// them at all.
    pub fn connector(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, colour: Color32) {
        let stroke = egui::Stroke::new(1.5, colour);
        let bend = ((to.y - from.y).abs() * 0.4).clamp(6.0, 26.0);
        let points = [
            from,
            egui::pos2(from.x, from.y - bend.copysign(from.y - to.y)),
            egui::pos2(to.x, to.y + bend.copysign(from.y - to.y)),
            to,
        ];
        painter.add(egui::Shape::CubicBezier(
            egui::epaint::CubicBezierShape::from_points_stroke(
                points,
                false,
                Color32::TRANSPARENT,
                stroke,
            ),
        ));
    }
}

/// Draw the timeline.
pub fn timeline_panel(ui: &mut Ui, scene: &Scene, state: &TimelineState) -> TimelineResponse {
    timeline_panel_with_tools_slot(ui, scene, state, None)
}

/// The least width the layer tools need on one line: wider than the row
/// they draw ([`layer_tools`] at the theme's own sizes — checked by
/// `the_layer_tools_fit_the_header_slot_they_ask_for`), so a slot this wide
/// never clips the Reset button off its end.
pub const LAYER_TOOLS_INLINE_WIDTH: f32 = 860.0;

/// [`timeline_panel`], with the layer tools (New Layer, Folder, Camera,
/// Parent, Depth, the two size sliders) drawn into `tools_slot` rather than
/// a strip of their own under the grid.
///
/// The slot is the empty run of the panel's own header line, right of the
/// "Timeline" tab (user 2026-10-02: the strip at the bottom was "eating
/// space"). Handing it over gives the frame grid back a whole row. A slot
/// narrower than [`LAYER_TOOLS_INLINE_WIDTH`] — a narrow window, a timeline
/// docked in a column — falls back to the strip at the bottom, so the tools
/// are never clipped out of reach.
pub fn timeline_panel_with_tools_slot(
    ui: &mut Ui,
    scene: &Scene,
    state: &TimelineState,
    tools_slot: Option<egui::Rect>,
) -> TimelineResponse {
    let mut response = TimelineResponse::default();

    transport(ui, scene, state, &mut response);
    ui.separator();

    let frame_count = scene.frame_count().max(1);
    // Always offer some empty frames past the end so the span can be extended
    // by clicking, as in Animate.
    let columns = (frame_count + 40).min(9_999);

    // Animate keeps New Layer, New Folder and Delete under the layer names, at
    // the bottom left of the timeline, and that is where the hand goes looking
    // for them. Claimed before the grid so the grid takes what is left:
    // reserving it afterwards would leave the strip fighting the scroll area
    // for the last row of pixels.
    //
    // **A height that starts right rather than one guessed at.** An
    // `egui::Panel` with no explicit size asks `PanelState` for last frame's
    // rect and falls back to a plain guess on the first frame it is ever
    // shown — so for a frame or two (a fresh document, a resized dock, this
    // very screenshot harness's warm-up frame) the reserved strip could be
    // shorter than the row it is about to draw, and the grid above claimed
    // the difference: at a 220 px timeline that was enough to draw the last
    // layer's row half under this one. `default_size`/`min_size` at the row's
    // own height fixes the common case immediately rather than waiting for a
    // frame to settle. The tallest control on the row is a button or slider
    // at `Tokens::CONTROL_H`; `+ 4.0` is this panel's own default vertical
    // margin (`Frame::side_top_panel`'s `Margin::symmetric(8, 2)`, top and
    // bottom). Not `exact_size`: `layer_tools` wraps onto a second line on a
    // narrow window (see its own comment), and an exact height would clip
    // that second line rather than let the panel grow to fit it — egui
    // persists the *drawn* rect every frame regardless, so growing here still
    // takes only the one frame the rest of this dock already budgets for.
    match tools_slot.filter(|slot| slot.width() >= LAYER_TOOLS_INLINE_WIDTH) {
        Some(slot) => {
            let mut tools = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("timeline-layer-tools-inline")
                    .max_rect(slot)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            tools.set_clip_rect(slot);
            layer_tools(&mut tools, scene, state, &mut response);
        }
        None => {
            egui::Panel::bottom("timeline-layer-tools")
                .resizable(false)
                .default_size(LAYER_TOOLS_HEIGHT)
                .min_size(LAYER_TOOLS_HEIGHT)
                .show(ui, |ui| layer_tools(ui, scene, state, &mut response));
        }
    }

    // **A timeline scroll bar you can see** (user 2026-10-01: "too thin and
    // dark"). The theme's bar is 6 px and its thumb takes the widget fill,
    // dark grey on a near-black track in the dark theme. Here only: 10 px, and
    // the thumb in the text colour (light on dark, dark on light), so it has
    // the contrast of the labels around it. Put back after, so nothing nested
    // and no other panel changes.
    let theme_scroll = ui.spacing().scroll;
    ui.spacing_mut().scroll.bar_width = 10.0;
    ui.spacing_mut().scroll.foreground_color = true;
    egui::ScrollArea::both()
        .id_salt("timeline-grid")
        .auto_shrink([false, false])
        // Rows scroll rather than being silently clipped when a short
        // timeline (Essentials' 220 px default among them) cannot show every
        // layer at once — the bar stays on screen rather than only on hover,
        // so a layer past the fold is a scrollbar away rather than invisible.
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            // **No gaps between the rows.** egui puts its standard item spacing
            // between anything laid out one after another, which here was a
            // stripe of panel background between every layer — Animate's rows
            // touch, and the space was costing a row of layers for every three
            // shown. The cells' own outlines are what separate them.
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

            // Where the rows start, left and top, for the playhead line drawn
            // down through all of them once they are laid out (below).
            let rows_left = ui.cursor().min.x;
            ruler(ui, scene, columns, *scene.looping(), state, &mut response);
            let rows_top = ui.cursor().min.y;

            // The camera sits above every layer, as it does in Animate — but
            // only on the document's own timeline. See [`shows_camera_row`].
            if shows_camera_row(scene) {
                camera_row(ui, scene, columns, state, &mut response);
            }

            // A channel per light that is already keyed, plus the one currently
            // selected so it can be keyed — on the document timeline only, since
            // lights are global. Cloned ids first to avoid holding a scene borrow
            // across the row draws.
            //
            // **Every light is a row, in its place in the stack.** A light at
            // the top lights everything and sits here, above the layers; one
            // dragged down between two layers sits there and lights only what
            // is below it. See `Scene::place_light`.
            let lights_in = |slot: Option<LayerId>| -> Vec<LightId> {
                if !scene.edit_path().is_empty() {
                    return Vec::new();
                }
                scene
                    .lights()
                    .lights
                    .iter()
                    .filter(|l| scene.light_slot(l.id) == slot)
                    .map(|l| l.id)
                    .collect()
            };
            for id in lights_in(None) {
                if let Some(light) = scene.lights().get(id) {
                    light_row(ui, scene, light, columns, state, &mut response);
                }
            }

            // The node table is rebuilt every frame the view is on: rows come
            // and go with the scroll, and a stale position would draw a
            // connector to where a layer used to be.
            if state.parenting_view {
                parenting::clear_nodes(ui.ctx());
            }

            let layer_ids: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
            for id in layer_ids {
                // I2: a row nested inside a collapsed folder is not drawn —
                // neither its own row nor a light keyed to it — though the
                // folder's own row always is. Collapse never touches the
                // stage; see `LayerStack::is_row_hidden`.
                if scene.layers().is_row_hidden(id) {
                    continue;
                }
                for light_id in lights_in(Some(id)) {
                    if let Some(light) = scene.lights().get(light_id) {
                        light_row(ui, scene, light, columns, state, &mut response);
                    }
                }
                let Some(layer) = scene.layers().get(id) else {
                    continue;
                };
                layer_row(ui, scene, layer, columns, state, &mut response);
            }

            // **The playhead runs down through every row.** Animate's playhead
            // is one line from the ruler to the bottom of the timeline, so the
            // current frame can be read on any layer; the per-cell wash alone
            // marked only the camera, the lights and the selected layer. Drawn
            // over the rows, clipped to the frame grid so it never crosses the
            // pinned layer names, and down to the bottom of the view even when
            // the layers end above it.
            draw_playhead_line(ui, rows_left, rows_top, state.current_frame, state.frame_width);

            // **Connectors last**, over every row, because a link crosses the
            // rows between its two ends and would otherwise be painted over by
            // whichever row was drawn next.
            if state.parenting_view {
                let nodes = parenting::nodes(ui.ctx());
                let at = |id: LayerId| nodes.iter().find(|(l, _)| *l == id).map(|(_, p)| *p);
                let painter = ui.painter_at(ui.clip_rect());
                for (id, from) in &nodes {
                    let Some(parent) = scene.layers().get(*id).and_then(|l| l.follows) else {
                        continue;
                    };
                    // A parent scrolled out of view has no end to draw to.
                    let Some(to) = at(parent) else { continue };
                    let colour = scene
                        .layers()
                        .get(parent)
                        .map(|l| {
                            let [r, g, b, _] = l.color.to_rgba8().to_u8_array();
                            Color32::from_rgb(r, g, b)
                        })
                        .unwrap_or_else(Palette::text_dim);
                    parenting::connector(&painter, *from, to, colour);
                }
            }

            // **A Shot's end marker.** Frames past a shot's own playing
            // length are kept, never deleted (see
            // `docs/SEQUENCE_SETUPS_PLAN.md` "Setups are stamped, never
            // linked"), so they still show here — greyed, past a marker at
            // the end — rather than vanish. Drawn last, over every row, the
            // same reason connectors are.
            if let Some(end) = state.shot_end
                && end < columns
            {
                let cell_width = state.frame_width;
                let grid_left = ui.min_rect().min.x + LAYER_COLUMN;
                let x0 = grid_left + end as f32 * cell_width;
                let x1 = grid_left + columns as f32 * cell_width;
                let area = egui::Rect::from_min_max(
                    egui::pos2(x0, ui.min_rect().min.y),
                    egui::pos2(x1, ui.min_rect().max.y),
                );
                let painter = ui.painter_at(ui.clip_rect());
                painter.rect_filled(area, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
                painter.line_segment(
                    [egui::pos2(x0, area.min.y), egui::pos2(x0, area.max.y)],
                    egui::Stroke::new(2.0, Palette::warning()),
                );
            }
        });

    ui.spacing_mut().scroll = theme_scroll;
    if ui.input(|i| i.pointer.any_released()) {
        ui.ctx().data_mut(|d| {
            d.remove_temp::<FrameVerticalDrag>(egui::Id::new("timeline_frame_vertical_drag"));
            d.remove::<FrameMoveDrag>(egui::Id::new(FRAME_MOVE_DRAG));
        });
    }

    response
}

/// New Layer, New Folder and Delete, under the layer names.
///
/// The same three commands the Insert menu and the Layers panel raise — the
/// buttons are a third door onto one room, not a third implementation. Delete
/// is disabled at one layer, because a document with no layers is not a state
/// Animate lets you reach and not one worth supporting here.
fn layer_tools(ui: &mut Ui, scene: &Scene, state: &TimelineState, out: &mut TimelineResponse) {
    // `horizontal_wrapped`, not `horizontal`: Stage 6, item 3 brought every
    // button on this row up to `Tokens::CONTROL_H`/24 px wide-or-taller,
    // which is wider, in total, than the narrowest legal window — wrapping
    // onto a second line there is what `panel_bottom`'s own comment (above)
    // means by letting this panel grow to fit it, the same trade the
    // Library's action row already makes for the same reason.
    ui.horizontal_wrapped(|ui| {
        ui.add_space(4.0);
        // Stage 6, item 3: `.small()`/`small_button` opt out of the theme's
        // `interact_size.y` minimum (`Tokens::CONTROL_H`, 24 px) — fine for
        // dense inline text, wrong for the row's clickable buttons, which the
        // hit-target audit walks like any other. `LAYER_TOOLS_HEIGHT` above
        // already reserves the room for them at full size.
        if crate::widgets::sheet_icon_button(
            ui,
            crate::icon_sheet::SheetIcon::NewLayer,
            crate::Command::NewLayer.label(),
        )
        .clicked()
        {
            out.command = Some(crate::Command::NewLayer);
        }
        // F1: spelled out — "Fld" read as a typo or an abbreviation nobody
        // had agreed on, on a button that makes the one thing a folder
        // button ought to say plainly.
        if ui
            .button("Folder")
            .on_hover_text(crate::Command::NewLayerFolder.label())
            .clicked()
        {
            out.command = Some(crate::Command::NewLayerFolder);
        }

        let can_delete = scene.layers().len() > 1 && state.active_layer.is_some();
        if ui
            .add_enabled_ui(can_delete, |ui| {
                crate::widgets::sheet_icon_button(
                    ui,
                    crate::icon_sheet::SheetIcon::DeleteLayer,
                    crate::Command::DeleteLayer.label(),
                )
            })
            .inner
            .on_disabled_hover_text(if state.active_layer.is_some() {
                "The last layer cannot be deleted"
            } else {
                "Select a layer first"
            })
            .clicked()
        {
            out.command = Some(crate::Command::DeleteLayer);
        }

        // **C1**: the camera row's own on/off, reachable from the layer
        // tools the same way a layer's own eye is reachable from its row —
        // Animate keeps "Show/Hide Camera" beside its layer buttons rather
        // than making it a menu a user has to already know exists.
        if ui
            .add(egui::Button::new("Camera").selected(scene.camera().enabled))
            .on_hover_text("Add/Remove Camera")
            .clicked()
        {
            out.command = Some(crate::Command::ToggleCamera);
        }

        // **Animate's parenting view**, on the same strip as the layer tools
        // because it is one: it changes what the layer column is for. A toggle
        // rather than a mode you have to leave — the frame grid and every
        // switch keep working while it is on.
        if ui
            .add(
                // **Named, not a pictogram.** This was three box-drawing
                // characters meant to look like two nodes on a wire. The
                // bundled fonts do not have them, so the one control that turns
                // on the whole parenting view drew as an empty box — which is
                // why it could not be found.
                egui::Button::new("Parent").selected(state.parenting_view),
            )
            .on_hover_text(
                "Parenting view: the layer column becomes a node graph. Drag a \
                 layer's node onto another to make it follow that layer, or onto \
                 empty space to detach it.",
            )
            .clicked()
        {
            out.toggle_parenting = true;
        }

        // **Layer depth, in the timeline.** Animate keeps this beside the layer
        // names because depth is a property of a *layer*, read down the stack:
        // the question is always "which of these is in front", and a panel on
        // the far side of the window cannot answer it next to the layer it is
        // about.
        if ui
            .add(
                egui::Button::new("Depth").selected(state.depth_view),
            )
            .on_hover_text(
                "Depth view: the layer column shows how far each layer sits \
                 from the camera. Drag a number to move that layer in space.",
            )
            .clicked()
        {
            out.toggle_depth = true;
        }

        // How many, on the same line: the count is the one thing about the
        // layer stack that is not visible when the list is scrolled.
        ui.label(
            egui::RichText::new(format!("{} layers", scene.layers().len()))
                .small()
                .weak(),
        );

        // **The two zooms.** Animate has a Tiny-to-Large menu for the frames
        // and a Short/Normal/Tall one for the rows; these are the same two
        // things as sliders, because the useful size depends on the film. A
        // four-thousand-frame timeline wants narrow cells and a twelve-frame
        // cycle wants wide ones, and neither is a setting anybody sets once.
        ui.separator();
        let mut width = state.frame_width;
        let mut rows = state.row_scale;

        // **The numbers are shown.** A bare slider says a size can be changed
        // and not what it currently is, which is the one thing somebody
        // matching two documents needs.
        ui.label(egui::RichText::new("frame width").small().weak());
        if ui
            .add(
                egui::Slider::new(&mut width, crate::workspace::FRAME_WIDTH_RANGE)
                    .suffix(" px")
                    .fixed_decimals(0)
                    .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 }),
            )
            .on_hover_text("How wide one frame is drawn")
            .changed()
        {
            out.set_frame_width = Some(width);
        }

        ui.label(egui::RichText::new("row height").small().weak());
        if ui
            .add(
                egui::Slider::new(&mut rows, crate::workspace::ROW_SCALE_RANGE)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                    .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 }),
            )
            .on_hover_text("How tall each layer's row is")
            .changed()
        {
            out.set_row_scale = Some(rows);
        }

        if ui
            .button("Reset")
            .on_hover_text("Back to the standard frame and row size")
            .clicked()
        {
            out.set_frame_width = Some(Metrics::FRAME_WIDTH);
            out.set_row_scale = Some(1.0);
        }
    });
}

/// Playback controls and readouts.
fn transport(ui: &mut Ui, scene: &Scene, state: &TimelineState, out: &mut TimelineResponse) {
    // **Centred, and centred a frame late.** egui lays widgets out as it draws
    // them, so the row's width is not known until it has been drawn; the width
    // from the previous frame decides the leading space. The row's contents
    // change rarely — and when they do, by one button — so the correction
    // lands within a frame and is not visible.
    let id = ui.id().with("transport-width");
    let previous: f32 = ui.memory(|m| m.data.get_temp(id)).unwrap_or(0.0);
    let leading = ((ui.available_width() - previous) * 0.5).max(0.0);

    ui.horizontal(|ui| {
        ui.add_space(leading);
        let start = ui.cursor().min.x;
        transport_controls(ui, scene, state, out);
        let width = ui.cursor().min.x - start;
        if width > 0.0 {
            ui.memory_mut(|m| m.data.insert_temp(id, width));
        }
    });
}

/// The transport strip's symbols, drawn like Animate's — icon buttons, not
/// text. Every one of these — including the six the user's own icon sheet
/// once supplied a raster outline mask for (`Play`, `PreviousFrame`,
/// `NextFrame`, `OnionSkin`, `AddFrame`) — is now a hand-authored painter
/// path, the same choice `icons.rs` makes for the tool strip and for the
/// same reason: a mask cut from the sheet read as a blob at 20 px, and a
/// path drawn for the button's own size does not.
#[derive(Clone, Copy)]
enum Glyph {
    First,
    Pause,
    Last,
    AutoKey,
    EditMultiple,
    Loop,
    InsertKey,
    InsertBlank,
    Play,
    PreviousFrame,
    NextFrame,
    OnionSkin,
    AddFrame,
}

impl Glyph {
    /// Every variant — walked by the two tests below, the same way
    /// `icons.rs`'s `all_tools()` is walked by its own bounds and
    /// draws-something tests.
    #[cfg(test)]
    const ALL: [Glyph; 13] = [
        Glyph::First,
        Glyph::Pause,
        Glyph::Last,
        Glyph::AutoKey,
        Glyph::EditMultiple,
        Glyph::Loop,
        Glyph::InsertKey,
        Glyph::InsertBlank,
        Glyph::Play,
        Glyph::PreviousFrame,
        Glyph::NextFrame,
        Glyph::OnionSkin,
        Glyph::AddFrame,
    ];
}

/// A small square icon button, ghost-styled like every other icon-only
/// control (Stage 2, item 3 of `docs/WORKSPACE_DESIGN_PLAN.md`): no fill at
/// rest, `hover` on hover, `active` fill with the accent glyph when its mode
/// is on. Its glyph is painted rather than text — `Glyph` covers marks (go to
/// first/last frame, loop) with no settled Unicode codepoint — so it stays
/// its own small helper rather than calling `buzz_ui::widgets::icon_button`,
/// which draws `icon: &str`.
fn icon_button(ui: &mut Ui, active: bool, tip: &str, glyph: Glyph) -> bool {
    // **Tried at `Tokens::CONTROL_H` (24), reverted to 22.** A dozen of these
    // sit side by side in one un-wrapped row (`transport`, centred, not
    // wrapped — a playback strip whose buttons rearrange mid-row would be
    // its own usability problem); two points each was enough, over that many
    // buttons, to overrun the window in `dock_geometry_tests::the_stage_never_reaches_into_a_dock_column`'s
    // narrowest legal one. Left at 22 rather than solved properly (auditing
    // what the transport can drop first) in the time this pass had. Still
    // inside the Timeline dock's own hit-target exemption in
    // `hit_target_tests` — the same one the frame grid uses — so this is a
    // recorded gap, not a silently accepted one.
    let size = egui::vec2(22.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = if active {
        Some(Palette::active())
    } else if resp.hovered() {
        Some(Palette::hover())
    } else {
        None
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, 3.0, bg);
    }
    let ink = if active {
        Palette::accent()
    } else {
        crate::icon_sheet::ink()
    };
    // The user's icon sheet where it has the mark; the painted glyph where it
    // does not (first/last frame, loop, auto-key and the rest).
    match sheet_glyph(glyph) {
        Some(icon) => crate::icon_sheet::paint(ui.painter(), rect.shrink(4.0), icon, ink),
        None => draw_glyph(ui.painter(), rect.shrink(6.0), glyph, ink),
    }

    // Stage 6, item 4: this button paints its own glyph rather than text, so
    // it carries no accessible name unless one is set explicitly — the same
    // string the tooltip already shows.
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));

    resp.on_hover_text(tip).clicked()
}

/// The icon sheet's picture for a transport glyph, where it has one.
fn sheet_glyph(glyph: Glyph) -> Option<crate::icon_sheet::SheetIcon> {
    use crate::icon_sheet::SheetIcon as S;
    Some(match glyph {
        Glyph::Play => S::Play,
        Glyph::PreviousFrame => S::PreviousFrame,
        Glyph::NextFrame => S::NextFrame,
        Glyph::OnionSkin => S::OnionSkin,
        Glyph::AddFrame => S::AddFrame,
        Glyph::InsertKey => S::Keyframe,
        _ => return None,
    })
}

/// `glyph`'s shapes, positioned absolutely inside `r` — split out from
/// [`draw_glyph`] the same way `icons.rs`'s `tool_shapes` is split from
/// `tool_icon`: a `Vec<Shape>` is what a test can look at, and a painted mark
/// that silently drew nothing is exactly the failure worth catching.
fn glyph_shapes(r: egui::Rect, g: Glyph, c: Color32) -> Vec<Shape> {
    let mut out = Vec::new();
    let s = Stroke::new(1.5, c);
    let at = |x: f32, y: f32| egui::pos2(r.min.x + r.width() * x, r.min.y + r.height() * y);
    let tri = |a: egui::Pos2, b: egui::Pos2, d: egui::Pos2| -> Shape {
        Shape::convex_polygon(vec![a, b, d], c, Stroke::NONE)
    };
    let bar = |x0: f32, x1: f32| -> Shape {
        Shape::rect_filled(egui::Rect::from_min_max(at(x0, 0.08), at(x1, 0.92)), 0.0, c)
    };
    match g {
        Glyph::Pause => {
            out.push(bar(0.22, 0.4));
            out.push(bar(0.6, 0.78));
        }
        Glyph::First => {
            out.push(bar(0.12, 0.24));
            out.push(tri(at(0.92, 0.1), at(0.92, 0.9), at(0.34, 0.5)));
        }
        Glyph::Last => {
            out.push(bar(0.76, 0.88));
            out.push(tri(at(0.08, 0.1), at(0.08, 0.9), at(0.66, 0.5)));
        }
        // Previous/Next Frame carry the same bar-and-triangle mark as
        // First/Last — the user's own icon sheet draws its Previous Frame
        // and Next Frame icons that way, and the design brief groups all
        // four under one description, "bar + triangle".
        Glyph::PreviousFrame => {
            out.push(bar(0.12, 0.24));
            out.push(tri(at(0.92, 0.1), at(0.92, 0.9), at(0.34, 0.5)));
        }
        Glyph::NextFrame => {
            out.push(bar(0.76, 0.88));
            out.push(tri(at(0.08, 0.1), at(0.08, 0.9), at(0.66, 0.5)));
        }
        // Play: a plain triangle, no bar — one of the few solid accents the
        // design brief names outright.
        Glyph::Play => {
            out.push(tri(at(0.24, 0.14), at(0.24, 0.86), at(0.84, 0.5)));
        }
        // A key, as a filled diamond.
        Glyph::AutoKey => {
            out.push(tri(at(0.5, 0.1), at(0.12, 0.5), at(0.5, 0.9)));
            out.push(tri(at(0.5, 0.1), at(0.88, 0.5), at(0.5, 0.9)));
        }
        // Several frames at once — three columns.
        Glyph::EditMultiple => {
            for x in [0.16, 0.44, 0.72] {
                out.push(Shape::rect_filled(
                    egui::Rect::from_min_max(at(x, 0.15), at(x + 0.12, 0.85)),
                    0.0,
                    c,
                ));
            }
        }
        // A loop — a ring with an arrowhead.
        Glyph::Loop => {
            out.push(Shape::circle_stroke(r.center(), r.width() * 0.3, s));
            out.push(tri(at(0.62, 0.02), at(0.98, 0.14), at(0.66, 0.32)));
        }
        // Insert Keyframe / Blank Keyframe — a filled dot, a hollow dot:
        // Animate's own marks for the two.
        Glyph::InsertKey => {
            out.push(Shape::circle_filled(r.center(), r.width() * 0.24, c));
        }
        Glyph::InsertBlank => {
            out.push(Shape::circle_stroke(r.center(), r.width() * 0.24, s));
        }
        // Three offset rounded rectangles, stepping back and up-left, the
        // front one filled — a stack of frames rather than one frame, thrice.
        Glyph::OnionSkin => {
            out.push(Shape::rect_stroke(
                egui::Rect::from_min_max(at(0.10, 0.14), at(0.58, 0.62)),
                2.0,
                s,
                StrokeKind::Middle,
            ));
            out.push(Shape::rect_stroke(
                egui::Rect::from_min_max(at(0.22, 0.24), at(0.70, 0.72)),
                2.0,
                s,
                StrokeKind::Middle,
            ));
            out.push(Shape::rect_filled(
                egui::Rect::from_min_max(at(0.34, 0.34), at(0.86, 0.82)),
                2.0,
                c,
            ));
        }
        // A frame with a plus — a keyframe added.
        Glyph::AddFrame => {
            out.push(Shape::rect_stroke(
                egui::Rect::from_min_max(at(0.12, 0.12), at(0.88, 0.88)),
                2.0,
                s,
                StrokeKind::Middle,
            ));
            out.push(Shape::line_segment([at(0.5, 0.30), at(0.5, 0.70)], s));
            out.push(Shape::line_segment([at(0.30, 0.5), at(0.70, 0.5)], s));
        }
    }
    out
}

/// Paint [`glyph_shapes`] straight to `p`.
fn draw_glyph(p: &egui::Painter, r: egui::Rect, g: Glyph, c: Color32) {
    p.extend(glyph_shapes(r, g, c));
}

fn transport_controls(
    ui: &mut Ui,
    scene: &Scene,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    {
        if icon_button(ui, false, "Go to first frame", Glyph::First) {
            out.go_to_start = true;
        }
        if icon_button(ui, false, "Previous frame (,)", Glyph::PreviousFrame) {
            out.step -= 1;
        }
        // Play and Pause share one button, switching face with `playing` —
        // both painted glyphs now, the same as every other transport icon.
        let play_clicked = if state.playing {
            icon_button(ui, true, "Play or pause (Enter)", Glyph::Pause)
        } else {
            icon_button(ui, false, "Play or pause (Enter)", Glyph::Play)
        };
        if play_clicked {
            out.toggle_play = true;
        }
        if icon_button(ui, false, "Next frame (.)", Glyph::NextFrame) {
            out.step += 1;
        }
        if icon_button(ui, false, "Go to last frame", Glyph::Last) {
            out.go_to_end = true;
        }

        ui.separator();

        if icon_button(ui, state.onion_enabled, "Onion skinning", Glyph::OnionSkin) {
            out.toggle_onion = true;
        }
        if icon_button(
            ui,
            state.auto_keyframe,
            "Auto Keyframe \u{2014} changing anything at a frame that is not a \
             keyframe makes one first, so the change starts here rather than \
             reaching back to the start of the span",
            Glyph::AutoKey,
        ) {
            out.toggle_auto_keyframe = true;
        }
        if icon_button(
            ui,
            state.edit_multiple,
            "Edit Multiple Frames \u{2014} every keyframe inside the onion markers \
             is drawn solid, can be clicked, and moves together",
            Glyph::EditMultiple,
        ) {
            out.toggle_edit_multiple = true;
        }

        // The onion markers, which both modes above read. Animate draws them as
        // brackets on the ruler and lets you drag them; here they are two
        // numbers, shown only when something is actually using them so the
        // transport does not carry controls for a mode that is off.
        if state.onion_enabled || state.edit_multiple {
            let mut before = state.onion_before;
            let mut after = state.onion_after;
            let frames = scene.frame_count().max(1);

            ui.label(egui::RichText::new("markers").small().weak());
            ui.add(
                egui::DragValue::new(&mut before)
                    .range(0..=frames)
                    .speed(0.2),
            )
            .on_hover_text("Frames covered before the playhead");
            ui.add(
                egui::DragValue::new(&mut after)
                    .range(0..=frames)
                    .speed(0.2),
            )
            .on_hover_text("Frames covered after it");
            if ui
                .small_button("All")
                .on_hover_text("Cover the whole timeline \u{2014} Animate's Onion All")
                .clicked()
            {
                before = frames;
                after = frames;
            }

            if (before, after) != (state.onion_before, state.onion_after) {
                out.set_onion_range = Some((before, after));
            }
        }

        loop_controls(ui, scene, out);

        ui.separator();

        let fps = scene.stage().frame_rate.max(0.01);
        let elapsed = state.current_frame as f64 / fps;
        ui.label(format!("Frame {}", state.current_frame + 1));

        // **How long the film is.** F5 and Shift+F5 add and remove a frame on
        // one layer at a time, which is the right tool inside a scene and a
        // poor one for "make this shot four seconds". This is the length
        // itself: dragging it extends every layer, or trims them.
        ui.label(egui::RichText::new("of").small().weak());
        let mut frames = scene.frame_count().max(1);
        if ui
            .add(
                egui::DragValue::new(&mut frames)
                    .range(1..=16_000)
                    .speed(0.25),
            )
            .on_hover_text("How many frames the document is \u{2014} every layer follows")
            .changed()
        {
            out.set_frame_count = Some(frames);
        }
        ui.label(
            egui::RichText::new(format!(
                "{:.0} fps · {elapsed:.2} s of {:.2} s",
                fps,
                scene.duration_seconds()
            ))
            .small()
            .weak(),
        );

        // In the row rather than pinned to the right edge: a right-aligned
        // group takes all the width that is left, which would leave nothing to
        // centre the row within.
        ui.separator();
        // Insert Frame is the one of the three with its own icon — Remove
        // Frame's own button lives only in the frame context menu as a text
        // item, and nothing here is called "duplicate", so Delete Frame and
        // Duplicate Frame stay unwired: there is nowhere this panel already
        // draws an icon for either.
        {
            let action = FrameAction::InsertFrame;
            let tip = format!("{} ({})", action.label(), action.shortcut_text());
            if icon_button(ui, false, &tip, Glyph::AddFrame) {
                out.action = Some(action);
            }
        }
        for (action, glyph) in [
            (FrameAction::InsertKeyframe, Glyph::InsertKey),
            (FrameAction::InsertBlankKeyframe, Glyph::InsertBlank),
        ] {
            let tip = format!("{} ({})", action.label(), action.shortcut_text());
            if icon_button(ui, false, &tip, glyph) {
                out.action = Some(action);
            }
        }
    }
}

/// The looping section's controls.
///
/// Animate's loop is a preview toggle. This one is part of the document and
/// the export repeats it, so the count is here beside the range: what the
/// numbers say is what the finished film will contain, and the readout says so
/// in frames rather than leaving it to be worked out.
fn loop_controls(ui: &mut Ui, scene: &Scene, out: &mut TimelineResponse) {
    ui.separator();

    let region = *scene.looping();
    let mut edited = region;

    if icon_button(
        ui,
        region.enabled,
        "Loop — repeat a section in playback and in the export",
        Glyph::Loop,
    ) {
        edited.enabled = !region.enabled;
        // Switching it on with nothing set would loop a single frame, which
        // reads as a bug. Default to the whole timeline, which is at least
        // what "loop" plainly means.
        if edited.enabled && edited.end <= edited.start {
            edited.start = 0;
            edited.end = scene.frame_count().saturating_sub(1);
        }
    }

    if region.enabled {
        // Shown one-based, like every other frame number in the timeline.
        let mut first = region.start + 1;
        let mut last = region.end + 1;
        let mut repeats = region.repeats;
        let frames = scene.frame_count().max(1);

        ui.label(egui::RichText::new("from").small().weak());
        ui.add(
            egui::DragValue::new(&mut first)
                .range(1..=frames)
                .speed(0.2),
        )
        .on_hover_text("First frame of the section");
        ui.label(egui::RichText::new("to").small().weak());
        ui.add(egui::DragValue::new(&mut last).range(1..=frames).speed(0.2))
            .on_hover_text("Last frame, inclusive");
        ui.label(egui::RichText::new("\u{00D7}").small().weak());
        ui.add(
            egui::DragValue::new(&mut repeats)
                .range(1..=MAX_REPEATS)
                .speed(0.1),
        )
        .on_hover_text("How many times it plays in total");

        edited.start = first.saturating_sub(1);
        edited.end = last.saturating_sub(1);
        edited.repeats = repeats;

        let length = edited.clamped(frames).rendered_length(frames);
        ui.label(egui::RichText::new(format!("film {length}")).small().weak())
            .on_hover_text("Length of the exported film, in frames");
    }

    if edited != region {
        out.set_loop = Some(edited.clamped(scene.frame_count().max(1)));
    }
}

/// The frame-number strip, which also scrubs.
fn ruler(
    ui: &mut Ui,
    scene: &Scene,
    columns: u32,
    region: LoopRegion,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    let cell_width = state.frame_width;
    let width = columns as f32 * cell_width;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LAYER_COLUMN + width, 16.0),
        Sense::click_and_drag(),
    );
    let grid_left = rect.min.x + LAYER_COLUMN;
    let pinned_left = ui.clip_rect().min.x;
    let font = FontId::proportional(9.0);

    // The numbers, loop band and playhead scroll with the grid, clipped to the
    // area right of the pinned layer-names column so none of it draws over the
    // headings.
    let painter = ui.painter_at(
        egui::Rect::from_min_max(egui::pos2(pinned_left + LAYER_COLUMN, rect.min.y), rect.max)
            .intersect(ui.clip_rect()),
    );
    painter.rect_filled(rect, 0.0, Palette::ruler_bg());

    // Label every fifth frame, as Animate does — or every tenth, or every
    // twentieth, once the cells are too narrow for the numbers to fit beside
    // each other. A ruler whose labels overlap is worse than one with fewer.
    let step = if cell_width >= 12.0 {
        5
    } else if cell_width >= 7.0 {
        10
    } else {
        20
    } as u32;
    // Label frame 1 and every fifth frame after — 1, 5, 10, 15 — the way
    // Animate numbers its ruler, on the one-based frame the animator sees.
    let visible = visible_columns(ui.clip_rect(), grid_left, cell_width, columns);
    for frame in visible.clone() {
        let number = frame + 1;
        if number != 1 && number % step != 0 {
            continue;
        }
        let x = grid_left + frame as f32 * cell_width;
        painter.text(
            egui::pos2(x + 1.0, rect.min.y + 2.0),
            Align2::LEFT_TOP,
            format!("{number}"),
            font.clone(),
            Palette::ruler_text(),
        );
    }

    // Beat ticks, under the numbers: a small mark at each detected beat so
    // action can be keyed to the music. Drawn before the playhead, which stays
    // the brightest thing on the strip.
    for &beat in &state.beats {
        if beat >= columns {
            continue;
        }
        let x = grid_left + (beat as f32 + 0.5) * cell_width;
        painter.line_segment(
            [egui::pos2(x, rect.max.y - 5.0), egui::pos2(x, rect.max.y)],
            // The theme's own guide colour: a beat tick is a marker on the
            // ruler the same way a dragged guide is a marker on the stage.
            egui::Stroke::new(1.5, Palette::guide()),
        );
    }

    draw_loop_band(&painter, rect, grid_left, columns, region, cell_width);
    draw_playhead(&painter, rect, grid_left, state.current_frame, cell_width);

    // **P1: the grid pages to keep the playhead in view.** Animate does the
    // same during playback and when stepping — it does not leave the
    // playhead to run off the edge of the window. A manual scroll is left
    // alone otherwise: this only fires when the frame just changed (or
    // playback is running) and the playhead is not already visible, and never
    // while the user is mid-drag on the grid or the ruler itself. See
    // `playhead_follow_target`.
    {
        let last_frame_id = egui::Id::new("timeline_last_shown_frame");
        let last_frame = ui.ctx().data(|d| d.get_temp::<u32>(last_frame_id));
        let dragging = response.dragged()
            || ui
                .ctx()
                .data(|d| {
                    d.get_temp::<FrameVerticalDrag>(egui::Id::new(
                        "timeline_frame_vertical_drag",
                    ))
                })
                .is_some();
        if let Some(target_x) = playhead_follow_target(
            last_frame != Some(state.current_frame),
            state.playing,
            dragging,
            grid_left + (state.current_frame as f32 + 0.5) * cell_width,
            pinned_left,
            ui.clip_rect().max.x,
            cell_width,
        ) {
            let clip = ui.clip_rect();
            ui.scroll_to_rect(
                egui::Rect::from_min_size(
                    egui::pos2(target_x, clip.min.y),
                    egui::vec2(1.0, clip.height().max(1.0)),
                ),
                Some(egui::Align::Min),
            );
        }
        ui.ctx()
            .data_mut(|d| d.insert_temp(last_frame_id, state.current_frame));
    }

    // The pinned header over the layer-names column: the eye/lock/outline
    // headings, sitting in the same columns their switches do on each row below.
    // Clicking a heading flips that switch on every layer — Animate's hide-all,
    // lock-all, outline-all.
    let name_row = egui::Rect::from_min_size(
        egui::pos2(pinned_left, rect.min.y),
        egui::vec2(LAYER_COLUMN, rect.height()),
    );
    let head = ui.painter_at(name_row.intersect(ui.clip_rect()));
    head.rect_filled(name_row, 0.0, Palette::ruler_bg());
    let hover = ui.ctx().input(|i| i.pointer.hover_pos());
    for (column, icon) in switch_columns(name_row)
        .iter()
        .zip(crate::panels::LayerIcon::ALL)
    {
        let hovered = hover.is_some_and(|p| column.contains(p) && response.hovered());
        // **H1**: the same painted eye/lock/outline every layer row draws,
        // rather than the 8 pt words, on when every layer already has it on
        // — the heading reads as the state of the whole stack, the way its
        // switches read the state of one layer. Neutral ink (the theme's own
        // text colour) rather than any one layer's, since the heading is not
        // about a layer.
        let all_on = scene.layers().iter().all(|l| match icon {
            crate::panels::LayerIcon::Eye => l.visible,
            crate::panels::LayerIcon::Lock => l.locked,
            crate::panels::LayerIcon::Outline => l.outline,
        });
        crate::panels::paint_layer_icon(
            &head,
            *column,
            icon,
            all_on,
            crate::panels::from_egui(Palette::text()),
            hovered,
        );
        if hovered {
            let tip = match icon {
                crate::panels::LayerIcon::Eye => "Show/Hide All Layers",
                crate::panels::LayerIcon::Lock => "Lock/Unlock All Layers",
                crate::panels::LayerIcon::Outline => "Outline All Layers",
            };
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                egui::Id::new(("timeline-heading-tip", icon.heading())),
                *column,
            )
            .gap(4.0)
            .show(|ui| {
                ui.label(tip);
            });
        }
        if response.clicked()
            && ui
                .ctx()
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| column.contains(p))
        {
            out.toggle_all = Some(icon);
        }
    }

    // Initiating drag downward from the ruler begins vertical column frame selection.
    if response.drag_started()
        && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
        && pos.x >= grid_left
    {
        let frame = ((pos.x - grid_left) / cell_width).floor().max(0.0) as u32;
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("timeline_frame_vertical_drag"),
                FrameVerticalDrag {
                    start_y: rect.min.y,
                    start_frame: frame.min(columns.saturating_sub(1)),
                    current_frame: frame.min(columns.saturating_sub(1)),
                    anchor_layer: scene.layers().iter().next().map(|l| l.id),
                },
            );
        });
    }

    let is_double_click = response.double_clicked();
    let is_shift_or_ctrl = (response.clicked() || response.dragged())
        && ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);

    // Clicking, dragging or double-clicking along the ruler scrubs.
    if (response.clicked() || response.dragged() || is_double_click)
        && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
        && pos.x >= grid_left
    {
        let frame = ((pos.x - grid_left) / cell_width).floor().max(0.0) as u32;
        let clamped = frame.min(columns.saturating_sub(1));
        out.scrub_to = Some(clamped);

        // Double-clicking or Shift/Ctrl-clicking a frame number on the ruler selects all layers at that frame.
        if is_double_click || is_shift_or_ctrl {
            let all_layers: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
            if let Some(anchor) = all_layers.first().copied() {
                out.select_cells = Some(crate::TimelineSelection::new(
                    all_layers, clamped, clamped, anchor, clamped,
                ));
            }
        }
    }

    // **M1: the frame is captured the instant the menu opens**, not read live
    // every frame it stays on screen — see `docs/TIMELINE_LIGHTS_FOLDERS_PLAN.md`.
    // Reading `interact_pos()` from inside the menu closure re-ran every frame
    // the popup was open, so the pointer travelling toward the popup itself
    // kept re-picking the frame under it.
    let ruler_menu_id = egui::Id::new("timeline_ruler_menu_frame");
    if response.secondary_clicked()
        && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
        && pos.x >= grid_left
    {
        let frame = ((pos.x - grid_left) / cell_width).floor().max(0.0) as u32;
        let clamped = frame.min(columns.saturating_sub(1));
        ui.ctx().data_mut(|d| d.insert_temp(ruler_menu_id, clamped));
    }
    response.context_menu(|ui| {
        let Some(clamped) = ui.ctx().data(|d| d.get_temp::<u32>(ruler_menu_id)) else {
            return;
        };
        ui.label(format!("Frame {}", clamped + 1));
        ui.separator();
        if ui.button("Select All Layers at This Frame").clicked() {
            let all_layers: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
            if let Some(anchor) = all_layers.first().copied() {
                out.select_cells = Some(crate::TimelineSelection::new(
                    all_layers, clamped, clamped, anchor, clamped,
                ));
            }
            out.scrub_to = Some(clamped);
            ui.close();
        }
    });
}

/// The looping section, marked along the ruler.
///
/// A bar under the numbers with a tick at each end — enough to see at a glance
/// which frames repeat, without covering the numbers themselves or competing
/// with the playhead, which stays the brightest thing on the strip.
fn draw_loop_band(
    painter: &egui::Painter,
    rect: egui::Rect,
    grid_left: f32,
    columns: u32,
    region: LoopRegion,
    cell_width: f32,
) {
    if !region.enabled || region.end < region.start {
        return;
    }
    let last = region.end.min(columns.saturating_sub(1));
    let left = grid_left + region.start as f32 * cell_width;
    let right = grid_left + (last + 1) as f32 * cell_width;
    let band = egui::Rect::from_min_max(
        egui::pos2(left, rect.max.y - 3.0),
        egui::pos2(right.max(left + 2.0), rect.max.y),
    );

    // Dimmed when the region would do nothing, so "on, but ×1" is visibly not
    // the same as a section that really repeats. The theme's own caution
    // colour — a loop is exactly that: a section that plays again, which is
    // worth a second look before exporting it.
    let color = if region.is_active() {
        Palette::warning()
    } else {
        Palette::warning().gamma_multiply(0.55)
    };
    painter.rect_filled(band, 0.0, color);
    for x in [left, right] {
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x - 1.0, rect.min.y + 2.0),
                egui::pos2(x + 1.0, rect.max.y),
            ),
            0.0,
            color,
        );
    }
}

/// **The playhead** (Stage 5, item 3 of `docs/WORKSPACE_DESIGN_PLAN.md`): a
/// 1 px accent line down the column, with a small triangle head at the top —
/// a needle pointing at a frame, rather than the solid cell-wide band this
/// used to paint in a colour of its own regardless of theme.
fn draw_playhead(
    painter: &egui::Painter,
    rect: egui::Rect,
    grid_left: f32,
    frame: u32,
    cell_width: f32,
) {
    let x = grid_left + (frame as f32 + 0.5) * cell_width;
    let accent = Palette::accent();
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(x - 0.5, rect.min.y),
            egui::pos2(x + 0.5, rect.max.y),
        ),
        0.0,
        accent,
    );
    let half = (cell_width * 0.35).min(6.0).max(2.0);
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - half, rect.min.y),
            egui::pos2(x + half, rect.min.y),
            egui::pos2(x, rect.min.y + half * 1.6),
        ],
        accent,
        egui::Stroke::NONE,
    ));
}

/// The playhead's line through the rows: [`draw_playhead`]'s 1 px line,
/// carried from the bottom of the ruler (`top`) to the bottom of the visible
/// timeline, in the same accent. `left` is the rows' own (scrolled) left edge,
/// so the line sits at the same x as the ruler's. Its rect is recorded under
/// [`PLAYHEAD_LINE`] for tests.
fn draw_playhead_line(ui: &Ui, left: f32, top: f32, frame: u32, cell_width: f32) {
    let clip = ui.clip_rect();
    let x = left + LAYER_COLUMN + (frame as f32 + 0.5) * cell_width;
    let grid_area = egui::Rect::from_min_max(egui::pos2(clip.min.x + LAYER_COLUMN, top.max(clip.min.y)), clip.max);
    if x < grid_area.min.x || x > grid_area.max.x || grid_area.height() <= 0.0 {
        return;
    }
    let line = egui::Rect::from_min_max(egui::pos2(x - 0.5, grid_area.min.y), egui::pos2(x + 0.5, grid_area.max.y));
    ui.painter_at(grid_area).rect_filled(line, 0.0, Palette::accent());
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(PLAYHEAD_LINE), line));
}

/// Where [`draw_playhead_line`] drew the playhead through the rows, for tests.
pub const PLAYHEAD_LINE: &str = "timeline-playhead-line";

/// **P1: whether the timeline should page to keep the playhead in view right
/// now, and the screen-space x it should scroll to if so.**
///
/// A pure decision, so the paging rule can be tested without a real
/// `ScrollArea`: `playhead_x`, `pinned_left` and `clip_right` are all in the
/// same screen-space coordinates `ruler` already computes them in (see
/// `draw_playhead`, `switch_columns`'s caller). Returns the x that should
/// land at the scroll area's own left edge — `playhead_x - LAYER_COLUMN`, so
/// the playhead itself ends up just past the pinned layer names, the way
/// Animate pages rather than creeping the playhead to the middle.
///
/// `None`, and nothing should move, when:
/// - the frame has not changed since the last draw and playback is not
///   running — a user's own manual scroll is left alone;
/// - the user is dragging the grid or the ruler;
/// - the playhead already sits inside
///   `[pinned_left + LAYER_COLUMN, clip_right - cell_width]`.
fn playhead_follow_target(
    frame_changed: bool,
    playing: bool,
    dragging: bool,
    playhead_x: f32,
    pinned_left: f32,
    clip_right: f32,
    cell_width: f32,
) -> Option<f32> {
    if dragging || !(frame_changed || playing) {
        return None;
    }
    let visible_left = pinned_left + LAYER_COLUMN;
    let visible_right = clip_right - cell_width;
    if playhead_x >= visible_left && playhead_x <= visible_right {
        return None;
    }
    Some(playhead_x - LAYER_COLUMN)
}

/// Whether the camera row belongs on the timeline right now.
///
/// It films the whole document, so it appears only on the document's own
/// timeline, never inside a symbol: a camera row there would suggest each
/// symbol carries a camera of its own, which it does not — the film has one.
///
/// **C1**: shown once the camera is switched on, *or* once it has any keys —
/// a camera someone switched off still has a shot worth seeing and a lock
/// worth reaching, the same way a hidden layer keeps its row rather than
/// vanishing from the timeline. Drawn dimmed while off; see `camera_row`.
fn shows_camera_row(scene: &Scene) -> bool {
    scene.edit_path().is_empty() && (scene.camera().enabled || !scene.camera().is_empty())
}

/// The camera row's interpolated-run tint: a blue distinct from the accent,
/// so a camera move reads apart from an ordinary tween. Lightened in a light
/// theme rather than left at the dark-theme value regardless of which theme is
/// current — the same "canvas follows the theme" rule the frame cells follow.
fn camera_tween_tint() -> Color32 {
    if crate::theme::theme().is_dark() {
        Color32::from_rgb(0x3A, 0x4A, 0x60)
    } else {
        Color32::from_rgb(0xC9, 0xD8, 0xEE)
    }
}

/// [`camera_tween_tint`], for a light's channel — a warm amber rather than the
/// camera's blue, so the two read apart on the same timeline.
fn light_tween_tint() -> Color32 {
    if crate::theme::theme().is_dark() {
        Color32::from_rgb(0x63, 0x54, 0x2A)
    } else {
        Color32::from_rgb(0xEE, 0xDD, 0xB8)
    }
}

/// The camera's row, above every layer.
///
/// Its cells are the camera's own keyframes: a key where the shot is set, a
/// tinted run between two keys where it is being interpolated. That is the
/// point of showing it at all — an animator reads a camera move off the
/// timeline the same way they read a character's.
///
/// **It is not a `Layer`.** It holds no objects and cannot be drawn on, and
/// smuggling it into the layer stack would mean every piece of code that walks
/// layers had to know to skip it. Animate shows it as a layer; underneath, it
/// is the camera.
fn camera_row(
    ui: &mut Ui,
    scene: &Scene,
    columns: u32,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    let height = Metrics::LAYER_ROW * state.row_scale;
    let cell_width = state.frame_width;
    let width = columns as f32 * cell_width;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(LAYER_COLUMN + width, height), Sense::click());
    let pinned_left = ui.clip_rect().min.x;
    let name_row = egui::Rect::from_min_size(
        egui::pos2(pinned_left, rect.min.y),
        egui::vec2(LAYER_COLUMN, height),
    );
    let grid_left = rect.min.x + LAYER_COLUMN;

    // The camera's keyframe cells scroll with the grid, clipped right of the
    // pinned name column.
    let painter = ui.painter_at(
        egui::Rect::from_min_max(egui::pos2(pinned_left + LAYER_COLUMN, rect.min.y), rect.max)
            .intersect(ui.clip_rect()),
    );
    let camera = scene.camera();
    let last = camera.last_frame();
    let tint = camera_tween_tint();

    for frame in visible_columns(ui.clip_rect(), grid_left, cell_width, columns) {
        let x = grid_left + frame as f32 * cell_width;
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(cell_width, height));

        let kind = if camera.has_key_at(frame) {
            FrameKind::Keyframe
        } else if !camera.is_empty() && frame < last {
            FrameKind::Span
        } else if !camera.is_empty() && frame == last {
            FrameKind::SpanEnd
        } else {
            FrameKind::Empty
        };
        let tween = (!camera.is_empty() && frame <= last && !camera.has_key_at(frame)).then_some(
            TweenCell {
                tint,
                complete: true,
                arrow: false,
            },
        );

        draw_frame_cell(
            &painter,
            cell,
            kind,
            tween,
            false,
            frame == state.current_frame,
            None,
        );
    }

    // The pinned "Camera" name cell, over the grid.
    let names = ui.painter_at(name_row.intersect(ui.clip_rect()));
    names.rect_filled(
        name_row,
        0.0,
        if state.camera_selected {
            Palette::active()
        } else {
            names_bg()
        },
    );
    // **C1**: dimmed while the camera is switched off — the same treatment a
    // hidden layer's own name gets — so a row shown only because it still
    // carries keys reads at a glance as "off".
    names
        .with_clip_rect(name_area(name_row).intersect(name_row))
        .text(
            egui::pos2(name_row.min.x + 4.0, name_row.center().y),
            Align2::LEFT_CENTER,
            "Camera",
            FontId::proportional(11.0),
            if camera.enabled {
                Palette::text()
            } else {
                Palette::text_dim()
            },
        );

    // **C1**: eye and lock switches, in the same columns a layer's row draws
    // them in. Eye is the camera's own on/off; lock refuses `MoveCamera`,
    // keyframe add/remove and the named camera moves — see `editor.rs`.
    let hover = ui.ctx().input(|i| i.pointer.hover_pos());
    let switches = switch_columns(name_row);
    let eye = switches[0];
    let lock = switches[1];
    let ink = crate::panels::from_egui(Palette::text());
    crate::panels::paint_layer_icon(
        &names,
        eye,
        crate::panels::LayerIcon::Eye,
        camera.enabled,
        ink,
        hover.is_some_and(|p| eye.contains(p) && response.hovered()),
    );
    crate::panels::paint_layer_icon(
        &names,
        lock,
        crate::panels::LayerIcon::Lock,
        camera.locked,
        ink,
        hover.is_some_and(|p| lock.contains(p) && response.hovered()),
    );

    if response.clicked() {
        // A switch first: clicking the eye or the lock does not also select
        // the camera and move the playhead — the same rule a layer's own
        // switches follow.
        let hit_pos = ui.ctx().input(|i| i.pointer.interact_pos());
        if hit_pos.is_some_and(|p| eye.contains(p)) {
            out.toggle_camera = true;
        } else if hit_pos.is_some_and(|p| lock.contains(p)) {
            out.toggle_camera_lock = true;
        } else {
            out.select_camera = true;
            if let Some(pos) = hit_pos
                && pos.x > grid_left
            {
                let frame = ((pos.x - grid_left) / cell_width) as u32;
                out.scrub_to = Some(frame.min(columns.saturating_sub(1)));
            }
        }
    }
}

/// The drag payload of a light's row — kept apart from a layer's
/// (`rig_panel::DraggedPart`) so dropping a light can never reorder a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraggedLight(pub LightId);

/// The kind of row a dragged light is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropRow {
    /// A layer's row: the top half puts the light above it, the bottom half
    /// above the layer below it.
    Layer(LayerId),
    /// Another light's row: the light joins that light's slot.
    Light(Option<LayerId>),
}

/// Where a light dropped on `row` lands: `Some(slot)` where `slot` is the
/// layer it will sit directly above (`None` = the top of the stack), or
/// `None` when the drop is not a place a light can go — the bottom half of
/// the last layer, since a light below every layer would light nothing.
pub(crate) fn light_drop_slot(
    scene: &Scene,
    row: DropRow,
    lower_half: bool,
) -> Option<Option<LayerId>> {
    match row {
        DropRow::Light(slot) => Some(slot),
        DropRow::Layer(id) if !lower_half => Some(Some(id)),
        DropRow::Layer(id) => {
            let at = scene.stage_layer_index(id)?;
            scene.stage_layers().iter().nth(at + 1).map(|l| Some(l.id))
        }
    }
}

/// While a light is being dragged over `rect`: draw the line where it would
/// land, and on release hand back which light goes where.
pub(crate) fn light_drop_target(
    ui: &Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    scene: &Scene,
    row: DropRow,
) -> Option<(LightId, Option<LayerId>)> {
    let dragged = egui::DragAndDrop::payload::<DraggedLight>(ui.ctx())?;
    let pos = ui.ctx().input(|i| i.pointer.hover_pos())?;
    if !rect.contains(pos) {
        return None;
    }
    let lower = pos.y >= rect.center().y && matches!(row, DropRow::Layer(_));
    let slot = light_drop_slot(scene, row, lower)?;
    let y = if lower { rect.bottom() } else { rect.top() };
    painter.line_segment(
        [egui::pos2(rect.min.x, y), egui::pos2(rect.max.x, y)],
        Stroke::new(2.0, light_tween_tint()),
    );
    if ui.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ui.ctx());
        return Some((dragged.0, slot));
    }
    None
}

/// A light's row: its keyframes on the timeline, the same way the camera's are.
///
/// Shown for any light that is keyed, plus the selected one so it can be keyed
/// in the first place. Clicking selects the light and scrubs; the right-click
/// menu adds or removes a key at the frame under the pointer.
fn light_row(
    ui: &mut Ui,
    scene: &Scene,
    light: &Light,
    columns: u32,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    let height = Metrics::LAYER_ROW * state.row_scale;
    let cell_width = state.frame_width;
    let width = columns as f32 * cell_width;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LAYER_COLUMN + width, height),
        Sense::click_and_drag(),
    );
    let pinned_left = ui.clip_rect().min.x;
    let name_row = egui::Rect::from_min_size(
        egui::pos2(pinned_left, rect.min.y),
        egui::vec2(LAYER_COLUMN, height),
    );
    let grid_left = rect.min.x + LAYER_COLUMN;
    let painter = ui.painter_at(
        egui::Rect::from_min_max(egui::pos2(pinned_left + LAYER_COLUMN, rect.min.y), rect.max)
            .intersect(ui.clip_rect()),
    );

    let track = light.track.as_ref();
    let has_key = |frame: u32| track.is_some_and(|t| t.has_key_at(frame));
    let is_empty = track.is_none_or(|t| t.is_empty());
    let last = track.map_or(0, |t| t.last_frame());
    // A warm amber for the interpolated run, distinct from the camera's blue.
    let tint = light_tween_tint();

    for frame in visible_columns(ui.clip_rect(), grid_left, cell_width, columns) {
        let x = grid_left + frame as f32 * cell_width;
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(cell_width, height));
        let kind = if has_key(frame) {
            FrameKind::Keyframe
        } else if !is_empty && frame < last {
            FrameKind::Span
        } else if !is_empty && frame == last {
            FrameKind::SpanEnd
        } else {
            FrameKind::Empty
        };
        let tween = (!is_empty && frame <= last && !has_key(frame)).then_some(TweenCell {
            tint,
            complete: true,
            arrow: false,
        });
        draw_frame_cell(
            &painter,
            cell,
            kind,
            tween,
            false,
            frame == state.current_frame,
            None,
        );
    }

    let names = ui.painter_at(name_row.intersect(ui.clip_rect()));
    names.rect_filled(
        name_row,
        0.0,
        if Some(light.id) == state.selected_light {
            Palette::active()
        } else {
            names_bg()
        },
    );
    // A small sun, so a light's row reads as a light and not as a layer.
    let sun = egui::pos2(name_row.min.x + 9.0, name_row.center().y);
    let ink = light_tween_tint();
    names.circle_filled(sun, 2.5, ink);
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let d = egui::vec2(a.cos(), a.sin());
        names.line_segment([sun + d * 4.0, sun + d * 5.5], Stroke::new(1.0, ink));
    }
    names.text(
        egui::pos2(name_row.min.x + 18.0, name_row.center().y),
        Align2::LEFT_CENTER,
        &light.name,
        FontId::proportional(11.0),
        Palette::text(),
    );

    // **Dragged by its name, like a layer**, to sit between any two layers.
    if response.drag_started()
        && response
            .interact_pointer_pos()
            .is_some_and(|p| name_row.contains(p))
    {
        egui::DragAndDrop::set_payload(ui.ctx(), DraggedLight(light.id));
    }
    if response.dragged() && egui::DragAndDrop::has_payload_of_type::<DraggedLight>(ui.ctx()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    let slot = scene.light_slot(light.id);
    if let Some(drop) = light_drop_target(ui, &names, name_row, scene, DropRow::Light(slot)) {
        out.place_light = Some(drop);
    }

    if response.clicked() {
        out.select_light = Some(light.id);
        if let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
            && pos.x > grid_left
        {
            let frame = ((pos.x - grid_left) / cell_width) as u32;
            out.scrub_to = Some(frame.min(columns.saturating_sub(1)));
        }
    }

    // **M1**: captured on open, not read live every frame the menu is open —
    // see `ruler`'s own fix above for why.
    let id = light.id;
    let menu_frame_id = ui.id().with(("timeline-light-menu-frame", id.0));
    if response.secondary_clicked() {
        let frame = response
            .interact_pointer_pos()
            .filter(|p| p.x > grid_left)
            .map(|p| (((p.x - grid_left) / cell_width) as u32).min(columns.saturating_sub(1)))
            .unwrap_or(state.current_frame);
        ui.ctx().data_mut(|d| d.insert_temp(menu_frame_id, frame));
    }
    response.context_menu(|ui| {
        let menu_frame = ui
            .ctx()
            .data(|d| d.get_temp::<u32>(menu_frame_id))
            .unwrap_or(state.current_frame);
        if ui.button("Add Keyframe Here").clicked() {
            out.light_key = Some((id, menu_frame));
            ui.close();
        }
        if has_key(menu_frame) && ui.button("Remove Keyframe").clicked() {
            out.light_unkey = Some((id, menu_frame));
            ui.close();
        }
    });
}

/// One layer's row: name on the left, frames on the right.
/// The range of frame columns that fall inside the visible viewport.
///
/// The timeline grid is virtualized: a row still *allocates* its full width — so
/// clicking, context menus and the scroll extent are exactly as before — but it
/// only *paints* the cells the user can actually see. On a document with
/// thousands of frames and hundreds of layers that is the difference between a
/// few thousand shapes a frame and several million. `clip` is the visible
/// rectangle (`ui.clip_rect()`); it is padded by a column on each side so a cell
/// half-scrolled off an edge is not missing.
fn visible_columns(
    clip: egui::Rect,
    grid_left: f32,
    cell_width: f32,
    columns: u32,
) -> std::ops::Range<u32> {
    if cell_width <= 0.0 {
        return 0..columns;
    }
    let first = ((clip.min.x - grid_left) / cell_width).floor() - 1.0;
    let last = ((clip.max.x - grid_left) / cell_width).ceil() + 1.0;
    let first = first.max(0.0) as u32;
    let last = last.max(0.0) as u32;
    first.min(columns)..last.min(columns)
}

fn layer_row(
    ui: &mut Ui,
    scene: &Scene,
    layer: &buzz_scene::Layer,
    columns: u32,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    let height = Metrics::LAYER_ROW * state.row_scale * (layer.height.percent() as f32 / 100.0);
    let cell_width = state.frame_width;
    let width = columns as f32 * cell_width;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LAYER_COLUMN + width, height),
        Sense::click_and_drag(),
    );
    // A row scrolled out of view is allocated (above) but not painted — the
    // virtualization that keeps a 500-layer timeline responsive.
    if !rect.intersects(ui.clip_rect()) {
        return;
    }
    // The layer names column is **sticky**: it stays pinned to the left edge of
    // the viewport while the frames grid scrolls horizontally beside it, exactly
    // as Animate keeps its layer list in place. Everything on the left is drawn
    // at `name_row` (pinned to the viewport) rather than at the row's own
    // scrolled origin, and the grid is clipped to the area right of it so the two
    // never overlap.
    let pinned_left = ui.clip_rect().min.x;
    let name_row = egui::Rect::from_min_size(
        egui::pos2(pinned_left, rect.min.y),
        egui::vec2(LAYER_COLUMN, height),
    );
    let grid_left = rect.min.x + LAYER_COLUMN;
    let length = layer.length();
    // Where this row's frame cells are, for tests that drive the timeline
    // with pointer events the way a hand would (`ROW_GEOMETRY`). One small
    // entry per drawn row.
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new((ROW_GEOMETRY, layer.id)),
            (rect, grid_left, cell_width),
        )
    });

    // -- frame grid, clipped to the area right of the pinned column -------
    let grid = ui.painter_at(
        egui::Rect::from_min_max(egui::pos2(pinned_left + LAYER_COLUMN, rect.min.y), rect.max)
            .intersect(ui.clip_rect()),
    );
    let visible = visible_columns(ui.clip_rect(), grid_left, cell_width, columns);
    // H2: the highlight wash, carried into every cell of this layer's row —
    // "across the whole frame row" the same way the name cell gets one.
    let highlight_wash = layer.highlight.map(|h| {
        let [r, g, b, _] = h.to_rgba8().to_u8_array();
        Color32::from_rgba_unmultiplied(r, g, b, HIGHLIGHT_WASH_ALPHA)
    });
    for frame in visible.clone() {
        let x = grid_left + frame as f32 * cell_width;
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(cell_width, height));
        // A frame reads as blank when the keyframe governing it holds nothing
        // at all — no art *and* no sound — across its own cell and every frame
        // of its span, so the hold after a blank keyframe no longer looks
        // filled. A sound keyframe carries no art but is not blank: its cell
        // stays occupied and shows the waveform.
        let blank = layer
            .frames
            .keyframe_at(frame)
            .is_some_and(|k| k.is_blank() && k.sound.is_none());
        draw_frame_cell(
            &grid,
            cell,
            layer.frame_kind(frame),
            tween_cell(layer, frame, length),
            blank,
            state
                .timeline_selection
                .as_ref()
                .is_some_and(|s| s.contains(layer.id, frame))
                || (frame == state.current_frame
                    && state.timeline_selection.is_none()
                    && (state.active_layer == Some(layer.id)
                        || state.selected_layers.contains(&layer.id))),
            highlight_wash,
        );
    }
    // A sound layer draws its waveform across the frames the sound covers, over
    // the cells and translucently, so an animator finds the accents by looking.
    // It stops where the sound does — at the next keyframe on the layer, so a
    // blank keyframe cuts the waveform off just as it silences the audio.
    if let Some(waveform) = state.waveforms.get(&layer.id) {
        let sound_end = layer
            .frames
            .keyframes()
            .iter()
            .map(|k| k.start)
            .find(|s| *s > waveform.start_frame);
        draw_waveform(
            &grid, waveform, grid_left, rect, height, &visible, cell_width, sound_end,
        );
    }

    let active = state.active_layer == Some(layer.id) || state.selected_layers.contains(&layer.id);
    // **The selected layer is marked along its whole row**, not only on its
    // name: a faint wash of the theme's selection colour over the frames and
    // the layer's own colour as a line along the bottom of the row, carried on
    // from the name cell's, so the layer whose artwork is lit on the stage can
    // be followed across the timeline to any frame.
    if active {
        let [r, g, b, _] = Palette::active().to_array();
        let row = egui::Rect::from_min_max(egui::pos2(grid_left, rect.min.y), rect.max);
        grid.rect_filled(row, 0.0, Color32::from_rgba_unmultiplied(r, g, b, ACTIVE_ROW_WASH_ALPHA));
        let [r, g, b, _] = layer.color.to_rgba8().to_u8_array();
        grid.rect_filled(
            egui::Rect::from_min_max(egui::pos2(grid_left, rect.max.y - 2.0), rect.max),
            0.0,
            Color32::from_rgb(r, g, b),
        );
    }

    // -- name column, pinned and drawn over the grid ----------------------
    let painter = ui.painter_at(name_row.intersect(ui.clip_rect()));
    if active {
        // Animate marks the selected layer with a blue name panel and its own
        // outline colour as a line along the bottom of the row — the theme's
        // own "active/selected" role, not a fixed blue.
        painter.rect_filled(name_row, 0.0, Palette::active());
        let [r, g, b, _] = layer.color.to_rgba8().to_u8_array();
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(name_row.min.x, name_row.max.y - 2.0),
                name_row.max,
            ),
            0.0,
            Color32::from_rgb(r, g, b),
        );
    } else {
        // Opaque, so the scrolling grid never shows through the pinned names.
        painter.rect_filled(name_row, 0.0, names_bg());
        // H2: the highlight wash. Painted only when the row is not the
        // active one — the selected-layer blue above already owns the name
        // cell, and this must not tint it.
        if let Some(h) = layer.highlight {
            let [r, g, b, _] = h.to_rgba8().to_u8_array();
            painter.rect_filled(
                name_row,
                0.0,
                Color32::from_rgba_unmultiplied(r, g, b, HIGHLIGHT_WASH_ALPHA),
            );
        }
    }
    // H2: the highlight's own 4 px bar at the row's left edge, painted last
    // so it stays visible even on the active row, whose blue would otherwise
    // cover it.
    if let Some(h) = layer.highlight {
        let [r, g, b, _] = h.to_rgba8().to_u8_array();
        let bar = egui::Rect::from_min_max(
            name_row.min,
            egui::pos2(name_row.min.x + HIGHLIGHT_BAR_WIDTH, name_row.max.y),
        );
        painter.rect_filled(bar, 0.0, Color32::from_rgb(r, g, b));
    }

    // **The parenting view puts the graph where the names go.** Everything
    // right of it — the switches, the colour chip, the frame grid, and every
    // click they answer — is untouched, because parenting is a thing layers do
    // rather than a different set of layers.
    if state.depth_view {
        depth_cell(ui, layer, name_row, &painter, state, out);
    } else if state.parenting_view {
        parenting_node(ui, scene, layer, name_row, &painter, out);
    } else {
        // Defects brief item 3c: a flat 12 px for "has any parent at all"
        // drew a folder's grandchild at the same indent as its child — one
        // step per folder, so nesting reads as nesting.
        let indent = folder_depth(scene, layer.id) as f32 * 12.0;
        let mark = match layer.kind {
            LayerKind::Folder => "F ",
            LayerKind::Mask => "M ",
            LayerKind::InverseMask => "iM ",
            LayerKind::Guide => "G ",
            LayerKind::Masked | LayerKind::Guided => ". ",
            LayerKind::Normal => "",
        };
        let name_rect = name_area(name_row).intersect(name_row);

        // I2: the disclosure triangle, one column reserved on every row so a
        // folder's name and a plain layer's name still line up.
        let triangle_rect = egui::Rect::from_min_size(
            egui::pos2(name_row.min.x + 2.0 + indent, name_row.center().y - 5.0),
            egui::vec2((DISCLOSURE_WIDTH - 2.0).max(1.0), 10.0),
        );
        if layer.kind == LayerKind::Folder {
            let triangle_id = ui.id().with(("timeline-folder-triangle", layer.id.0));
            if disclosure_triangle(ui, &painter, triangle_rect, triangle_id, layer.collapsed) {
                out.toggle_collapsed = Some(layer.id);
            }
        }

        // The name's own click/drag zone starts past the triangle column, so
        // clicking the triangle never also selects or drags the row.
        let text_x = (name_row.min.x + indent + DISCLOSURE_WIDTH).min(name_rect.max.x);
        let name_click_rect =
            egui::Rect::from_min_max(egui::pos2(text_x, name_rect.min.y), name_rect.max);
        let name_id = ui.id().with(("timeline-layer-name", layer.id.0));
        let name_resp = ui.interact(name_click_rect, name_id, Sense::click_and_drag());
        if !state.depth_view && !state.parenting_view {
            name_resp.dnd_set_drag_payload(crate::rig_panel::DraggedPart::Layer(layer.id));
        }
        if name_resp.clicked() {
            let shift = ui.input(|i| i.modifiers.shift);
            let toggle = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
            if shift {
                // I1: sweep from the anchor (or the active layer, if there is
                // no anchor yet) to the layer just clicked, in stack order —
                // pulling in a collapsed folder's hidden children when the
                // folder itself falls inside the range. The anchor itself
                // does not move.
                let anchor = state.range_anchor.or(state.active_layer).unwrap_or(layer.id);
                let order = scene.layers().visible_row_order();
                let range = crate::selection::layer_range(&order, anchor, layer.id);
                out.select_layers =
                    Some(crate::selection::expand_collapsed_folders(scene, &range));
            } else if toggle {
                let mut current = state.selected_layers.clone();
                if !current.contains(&layer.id) {
                    current.push(layer.id);
                } else {
                    current.retain(|&id| id != layer.id);
                }
                out.select_layers = Some(current);
                // Ctrl/Cmd+click moves the anchor, so a Shift+click right
                // after sweeps from the layer just toggled.
                out.set_range_anchor = Some(layer.id);
            } else {
                out.select_layer = Some(layer.id);
            }
        }
        name_resp.context_menu(|ui| {
            out.select_layer = Some(layer.id);
            layer_context_menu(ui, scene, layer, state, out);
        });

        // **Clipped to the name area.** A painted string does not wrap or
        // truncate itself, and a long layer name would otherwise be drawn
        // straight through the three switches to its right.
        painter.with_clip_rect(name_rect).text(
            egui::pos2(text_x + 2.0, name_row.center().y),
            Align2::LEFT_CENTER,
            format!("{mark}{}", layer.name),
            FontId::proportional(11.0),
            if layer.visible {
                Palette::text()
            } else {
                Palette::text_dim()
            },
        );
    }

    // **Animate's three columns, beside the name** — eye, lock and outline,
    // painted rather than laid out as widgets because the whole grid is.
    let hover = ui.ctx().input(|i| i.pointer.hover_pos());
    let switches = switch_columns(name_row);
    for (column, icon) in switches.iter().zip(crate::panels::LayerIcon::ALL) {
        let on = match icon {
            crate::panels::LayerIcon::Eye => layer.visible,
            crate::panels::LayerIcon::Lock => layer.locked,
            crate::panels::LayerIcon::Outline => layer.outline,
        };
        let hovered = hover.is_some_and(|p| column.contains(p) && response.hovered());
        crate::panels::paint_layer_icon(&painter, *column, icon, on, layer.color, hovered);
    }

    // The layer's colour chip, which is also its outline-view colour.
    let chip = egui::Rect::from_min_size(
        egui::pos2(
            switches[0].left() - crate::panels::CHIP_WIDTH - SWITCH_GAP,
            name_row.center().y - 4.0,
        ),
        egui::vec2(7.0, 8.0),
    );
    let [r, g, b, a] = layer.color.to_rgba8().to_u8_array();
    painter.rect_filled(chip, 1.0, Color32::from_rgba_unmultiplied(r, g, b, a));

    // A light being dragged over this row lands above it, or above the next
    // layer down, and lights from there downwards.
    if scene.edit_path().is_empty()
        && let Some(drop) =
            light_drop_target(ui, &painter, name_row, scene, DropRow::Layer(layer.id))
    {
        out.place_light = Some(drop);
    }

    // **F1**: a layer being dragged over this row shows where it will land
    // and — with everything filed under it — moves it there on drop. See
    // `docs/TIMELINE_LIGHTS_FOLDERS_PLAN.md` F1: the top and bottom quarters
    // of the row mean "beside it, at this nesting level"; the middle half of
    // a **Folder** row means "into it"; the middle half of anything else is
    // the nearest of above/below. See `drop_zone`, the pure decision this
    // reads, and `buzz_scene::LayerStack::move_to`, which moves a folder's
    // whole contiguous block rather than just the one row.
    if let Some(dragged) = egui::DragAndDrop::payload::<crate::rig_panel::DraggedPart>(ui.ctx())
        && let crate::rig_panel::DraggedPart::Layer(dragged_layer_id) = *dragged
        && dragged_layer_id != layer.id
        && let Some(hover_pos) = ui.ctx().input(|i| i.pointer.hover_pos())
        && name_row.contains(hover_pos)
    {
        let fraction = ((hover_pos.y - name_row.top()) / name_row.height()).clamp(0.0, 1.0);
        let is_folder = layer.kind == LayerKind::Folder;
        let row_index = scene.layer_index(layer.id).unwrap_or(0);
        let zone = drop_zone(fraction, is_folder);
        let (target_index, target_parent) = match zone {
            DropZone::Above => (row_index, layer.parent),
            DropZone::Below if !is_folder => (row_index + 1, layer.parent),
            // Below, or into, a folder: the folder's own first child.
            DropZone::Below | DropZone::Into => (row_index + 1, Some(layer.id)),
        };
        match zone {
            DropZone::Into => {
                painter.rect_filled(name_row, 0.0, Palette::active().gamma_multiply(0.4));
            }
            DropZone::Above | DropZone::Below => {
                let line_y = if zone == DropZone::Above {
                    name_row.top()
                } else {
                    name_row.bottom()
                };
                painter.line_segment(
                    [
                        egui::pos2(name_row.min.x, line_y),
                        egui::pos2(name_row.max.x, line_y),
                    ],
                    egui::Stroke::new(2.0, Palette::active()),
                );
            }
        }
        if ui.input(|i| i.pointer.any_released())
            && let Some(dropped) =
                egui::DragAndDrop::take_payload::<crate::rig_panel::DraggedPart>(ui.ctx())
            && let crate::rig_panel::DraggedPart::Layer(dropped_id) = *dropped
        {
            out.move_layer = Some((dropped_id, target_index, target_parent));
        }
    }

    // The grid begins to the right of the pinned column, not at its scrolled
    // origin, so a click on the pinned names is never read as a frame click.
    let grid_edge = pinned_left + LAYER_COLUMN;
    let drag_id = egui::Id::new("timeline_frame_vertical_drag");

    let clicked_frame = |ui: &Ui| -> Option<u32> {
        let pos = ui.ctx().input(|i| i.pointer.interact_pos())?;
        (pos.x >= grid_edge).then(|| {
            let frame = ((pos.x - grid_left) / cell_width).floor().max(0.0) as u32;
            frame.min(columns.saturating_sub(1))
        })
    };

    // **Move Frames**: a press inside the current frame selection, dragged
    // sideways, picks the selected frames up rather than starting a new
    // selection — Animate's frame drag. Read from where the button went down,
    // not where the pointer is once egui decides it is a drag, or a quick
    // flick would miss the selection it started on. Shift and Ctrl keep their
    // sweep, so extending a selection from inside it still works.
    let move_press = response.drag_started().then(|| {
        let press = ui
            .ctx()
            .input(|i| i.pointer.press_origin().or(i.pointer.interact_pos()))?;
        let modified = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
        if press.x < grid_edge || modified {
            return None;
        }
        let frame = (((press.x - grid_left) / cell_width).floor().max(0.0) as u32)
            .min(columns.saturating_sub(1));
        let selection = state.timeline_selection.as_ref()?;
        selection.contains(layer.id, frame).then_some(FrameMoveDrag {
            press_frame: frame,
            current_frame: frame,
            press_layer: layer.id,
            first_frame: selection.first_frame,
            rows: 0,
        })
    });
    if let Some(Some(drag)) = move_press {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(FRAME_MOVE_DRAG), drag));
    }
    frame_move_drag(
        ui, scene, &grid, layer.id, rect, grid_left, cell_width, columns, state, out,
    );

    // Initiating vertical drag across frames on a column
    if response.drag_started()
        && !matches!(move_press, Some(Some(_)))
        && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
        && pos.x >= grid_edge
    {
        let frame = ((pos.x - grid_left) / cell_width).floor().max(0.0) as u32;
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                drag_id,
                FrameVerticalDrag {
                    start_y: pos.y,
                    start_frame: frame.min(columns.saturating_sub(1)),
                    current_frame: frame.min(columns.saturating_sub(1)),
                    anchor_layer: Some(layer.id),
                },
            );
        });
    }

    // When vertical drag is active, check whether this layer row falls in the vertical drag range
    let drag_active = ui.ctx().data(|d| d.get_temp::<FrameVerticalDrag>(drag_id));
    if let Some(mut drag) = drag_active {
        let cur_y = ui
            .ctx()
            .input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()))
            .map(|p| p.y)
            .unwrap_or(drag.start_y);
        let y_min = drag.start_y.min(cur_y);
        let y_max = drag.start_y.max(cur_y);
        if let Some(pos) = ui
            .ctx()
            .input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()))
        {
            drag.current_frame = (((pos.x - grid_left) / cell_width).floor().max(0.0) as u32)
                .min(columns.saturating_sub(1));
            ui.ctx().data_mut(|d| d.insert_temp(drag_id, drag));
        }

        if rect.min.y <= y_max && rect.max.y >= y_min {
            out.select_layers
                .get_or_insert_with(Vec::new)
                .push(layer.id);
            out.scrub_to = Some(drag.current_frame);

            // Highlight the dragged frame cell
            let first = drag.start_frame.min(drag.current_frame);
            let last = drag.start_frame.max(drag.current_frame);
            let x = grid_left + first as f32 * cell_width;
            let cell = egui::Rect::from_min_size(
                egui::pos2(x, rect.min.y),
                egui::vec2(cell_width * (last - first + 1) as f32, height),
            );
            let [r, g, b, _] = Palette::active().to_array();
            grid.rect_filled(cell, 0.0, Color32::from_rgba_unmultiplied(r, g, b, 0xA0));
            grid.rect_stroke(
                cell,
                0.0,
                egui::Stroke::new(1.5, Palette::active()),
                StrokeKind::Inside,
            );

            // Highlight the layer name panel
            painter.rect_filled(name_row, 0.0, Palette::active());
        }

        if ui.input(|i| i.pointer.any_released())
            && let Some(layers) = out.select_layers.clone()
            && let Some(anchor) = drag.anchor_layer.or_else(|| layers.first().copied())
        {
            out.select_cells = Some(crate::TimelineSelection::new(
                layers,
                drag.start_frame,
                drag.current_frame,
                anchor,
                drag.start_frame,
            ));
            ui.ctx()
                .data_mut(|d| d.remove_temp::<FrameVerticalDrag>(drag_id));
        }
    }

    if response.clicked() {
        // A switch first: clicking the eye hides the layer, it does not also
        // select it and move the playhead to frame one.
        let hit = ui
            .ctx()
            .input(|i| i.pointer.interact_pos())
            .and_then(|pos| switch_at(name_row, pos));
        if let Some(icon) = hit {
            out.toggle_layer = Some((layer.id, icon));
        } else {
            if let Some(frame) = clicked_frame(ui) {
                let shift = ui.input(|i| i.modifiers.shift);
                if shift {
                    if let Some(old) = &state.timeline_selection {
                        let order: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
                        let a = order
                            .iter()
                            .position(|id| *id == old.anchor_layer)
                            .unwrap_or(0);
                        let b = order.iter().position(|id| *id == layer.id).unwrap_or(a);
                        let layers = order[a.min(b)..=a.max(b)].iter().copied();
                        out.select_cells = Some(crate::TimelineSelection::new(
                            layers,
                            old.anchor_frame,
                            frame,
                            old.anchor_layer,
                            old.anchor_frame,
                        ));
                    } else {
                        out.select_cells = Some(crate::TimelineSelection::cell(layer.id, frame));
                    }
                } else {
                    out.select_cells = Some(crate::TimelineSelection::cell(layer.id, frame));
                }
                out.scrub_to = Some(frame);
            } else {
                out.select_layer = Some(layer.id);
            }
        }
    }

    // **Two right-click menus, decided by where the pointer is.**
    //
    // Animate has a frame menu on the frames and a layer menu on the name, and
    // they are different sets of commands about different things. The frame
    // menu opening over a layer's name was the only menu here, so hiding or
    // deleting a layer from the timeline had no route at all.
    //
    // **M1**: both `in_name_column` and the clicked frame are captured once,
    // on `response.secondary_clicked()` — the frame the menu opens — rather
    // than read from the live pointer inside the menu closure below, which
    // runs again every frame the popup stays open. Moving the pointer toward
    // the popup afterwards used to re-pick the column and re-select or
    // re-scrub on every one of those frames. See
    // `docs/TIMELINE_LIGHTS_FOLDERS_PLAN.md` M1.
    let menu_target_id = ui.id().with(("timeline-layer-menu-target", layer.id.0));
    if response.secondary_clicked() {
        let in_name_column = ui
            .ctx()
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| pos.x < grid_edge);
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                menu_target_id,
                MenuTarget {
                    frame: clicked_frame(ui),
                    in_name_column,
                },
            )
        });
    }
    response.context_menu(|ui| {
        let target = ui
            .ctx()
            .data(|d| d.get_temp::<MenuTarget>(menu_target_id))
            .unwrap_or_default();
        let clicked = target.frame;
        if !clicked.is_some_and(|f| {
            state
                .timeline_selection
                .as_ref()
                .is_some_and(|s| s.contains(layer.id, f))
        }) {
            if let Some(frame) = clicked {
                out.select_cells = Some(crate::TimelineSelection::cell(layer.id, frame));
            } else {
                out.select_layer = Some(layer.id);
            }
        }
        if target.in_name_column {
            layer_context_menu(ui, scene, layer, state, out);
            return;
        }
        // Opening the frame menu moves the playhead first, so the command that
        // follows lands on the frame the user pointed at rather than wherever
        // the playhead happened to be.
        if let Some(frame) = clicked
            && out.scrub_to.is_none()
        {
            out.scrub_to = Some(frame);
        }
        frame_context_menu(ui, scene, out);
    });
}

/// Animate's right-click menu on a layer's name.
///
/// The same three switches the columns carry, spelled out — nothing on a
/// fifteen-point painted square says which one hides a layer — plus the layer
/// commands, which is where Delete belongs: a destructive action does not want
/// to be a fourth small square beside three toggles.
/// One layer's depth, in the layer column.
///
/// The name, the number, and a bar showing where the layer sits between the
/// camera and the focal plane — so the stack can be read at a glance rather
/// than one layer at a time.
///
/// # Why the bar is worth the room
///
/// A column of numbers says what the depths *are* and not how they relate,
/// which is the only thing anybody asks of depth: whether this layer is in
/// front of that one, and by how much. The bar answers it by being longer.
fn depth_cell(
    ui: &mut Ui,
    layer: &buzz_scene::Layer,
    name_row: egui::Rect,
    painter: &egui::Painter,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    let area = name_area(name_row).intersect(name_row);

    // The name first, narrowed to leave room for the number and the bar.
    let name_width = (area.width() * 0.42).clamp(40.0, 110.0);
    painter
        .with_clip_rect(egui::Rect::from_min_size(
            area.min,
            egui::vec2(name_width, area.height()),
        ))
        .text(
            egui::pos2(area.min.x + 4.0, area.center().y),
            Align2::LEFT_CENTER,
            layer.name.clone(),
            FontId::proportional(11.0),
            if layer.visible {
                Palette::text()
            } else {
                Palette::text_dim()
            },
        );

    // The number, draggable. Bounded just short of the focal distance: a layer
    // *at* the camera's focal point has no size on screen, and one past it is
    // behind the lens and not drawn at all.
    let focal = if state.focal_distance.is_finite() && state.focal_distance > 1.0 {
        state.focal_distance
    } else {
        1000.0
    };
    // The near side is the camera's own bound, so a drag here and a drag in the
    // Layer Depth panel stop in the same place; the far side matches it only so
    // that the bar below reads symmetrically about the focal plane.
    let limit = -state.nearest_depth;

    let number = egui::Rect::from_min_size(
        egui::pos2(area.min.x + name_width, area.center().y - 9.0),
        egui::vec2(54.0, 18.0),
    );

    // **Painted and interacted, not `put`.**
    //
    // `Ui::put` builds a child `Ui` and then advances the parent's cursor past
    // it — and this rect is pinned to the viewport's left edge rather than
    // sitting where the row was laid out, so every layer row grew by the height
    // of its own depth box and the rows drifted apart down the timeline. The
    // switches beside a layer's name are painted for exactly this reason; so is
    // this. `interact` claims the pointer without claiming any space.
    let response = ui.interact(
        number,
        egui::Id::new(("layer-depth", layer.id.0)),
        Sense::click_and_drag(),
    );

    // Dragging sideways changes the depth, at a speed set by the focal
    // distance so the control feels the same whatever the lens.
    if response.dragged() {
        let step = response.drag_delta().x as f64 * (focal / 400.0);
        if step != 0.0 {
            out.set_depth = Some((layer.id, (layer.depth + step).clamp(-limit, limit)));
        }
    }
    if response.double_clicked() {
        // Back to the stage, which is the value everything is measured from.
        out.set_depth = Some((layer.id, 0.0));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    let hot = response.hovered() || response.dragged();
    painter.rect_filled(
        number,
        2.0,
        if hot {
            Palette::chrome()
        } else {
            Color32::TRANSPARENT
        },
    );
    painter.text(
        egui::pos2(number.max.x - 4.0, number.center().y),
        Align2::RIGHT_CENTER,
        format!("{:.0}", layer.depth),
        FontId::proportional(11.0),
        if layer.depth == 0.0 {
            Palette::text_dim()
        } else {
            Palette::text()
        },
    );
    response.on_hover_text(
        "How far this layer sits from the focal plane. Drag sideways to move \
         it; double-click to put it back on the stage. Positive is further \
         away, and draws smaller; negative is nearer the camera.",
    );

    // The bar: the focal plane is the middle, and the layer's mark slides
    // either side of it.
    let track = egui::Rect::from_min_max(
        egui::pos2(number.max.x + 6.0, area.center().y - 3.0),
        egui::pos2(area.max.x - 2.0, area.center().y + 3.0),
    );
    if track.width() > 12.0 {
        painter.rect_filled(track, 2.0, Palette::chrome());
        // The focal plane, where depth is zero.
        let middle = track.center().x;
        painter.line_segment(
            [
                egui::pos2(middle, track.min.y - 2.0),
                egui::pos2(middle, track.max.y + 2.0),
            ],
            egui::Stroke::new(1.0, Palette::text_dim()),
        );
        let t = (layer.depth / limit).clamp(-1.0, 1.0) as f32;
        let x = middle + t * (track.width() * 0.5 - 2.0);
        let [r, g, b, _] = layer.color.to_rgba8().to_u8_array();
        painter.circle_filled(
            egui::pos2(x, track.center().y),
            3.5,
            Color32::from_rgb(r, g, b),
        );
    }
}

/// One layer's node in the parenting view, and the drag that rewires it.
///
/// Animate's gesture exactly: press the node of the layer you want to *move*,
/// drag to the layer you want it to follow, let go. Dropping anywhere that is
/// not another node detaches it, which is how a limb is taken off a rig without
/// hunting for a "none" entry in a menu.
fn parenting_node(
    ui: &mut Ui,
    scene: &Scene,
    layer: &buzz_scene::Layer,
    name_row: egui::Rect,
    painter: &egui::Painter,
    out: &mut TimelineResponse,
) {
    let ctx = ui.ctx().clone();
    let generation = parenting::generation(scene, layer.id);
    let centre = parenting::node_centre(name_row, generation);
    // Recorded before anything is drawn, so the connector pass below can find
    // both ends of a link however far apart their rows are.
    parenting::record_node(&ctx, layer.id, centre);

    // The name still has to be readable — a graph of anonymous dots says which
    // layers are linked but not which layers they are — so it runs from just
    // right of the node to where the switches begin.
    let text_left = centre.x + parenting::RADIUS + 5.0;
    painter
        .with_clip_rect(name_area(name_row).intersect(name_row))
        .text(
            egui::pos2(text_left, name_row.center().y),
            Align2::LEFT_CENTER,
            layer.name.clone(),
            FontId::proportional(11.0),
            if layer.visible {
                Palette::text()
            } else {
                Palette::text_dim()
            },
        );

    let hit = egui::Rect::from_center_size(centre, egui::vec2(18.0, 18.0));
    let response = ui.interact(
        hit,
        egui::Id::new(("parent-node", layer.id.0)),
        Sense::click_and_drag(),
    );

    if response.drag_started() {
        parenting::set_dragging(&ctx, Some(layer.id));
    }

    let dragging = parenting::dragging(&ctx);
    let source = dragging == Some(layer.id);
    // A node this drag could legally land on. Refusing the link here rather
    // than on release is what makes the illegal target *look* illegal.
    let target =
        dragging.is_some_and(|from| from != layer.id && scene.layers().can_follow(from, layer.id));
    let hovered = ui
        .ctx()
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| hit.contains(p));

    let [r, g, b, _] = layer.color.to_rgba8().to_u8_array();
    let colour = Color32::from_rgb(r, g, b);
    // The node the drag came from and the node it would land on are both lit,
    // because a link has two ends and the gesture is about both of them.
    let ring = if source || (target && hovered) {
        Palette::active()
    } else if layer.follows.is_some() {
        Palette::text()
    } else {
        Palette::text_dim()
    };
    painter.circle_filled(centre, parenting::RADIUS, colour);
    painter.circle_stroke(
        centre,
        parenting::RADIUS,
        egui::Stroke::new(
            if source || (target && hovered) {
                2.0
            } else {
                1.0
            },
            ring,
        ),
    );

    // The line the pointer is dragging, drawn from the node it started on.
    if source && let Some(pointer) = ui.ctx().input(|i| i.pointer.interact_pos()) {
        painter.line_segment([centre, pointer], egui::Stroke::new(1.5, Palette::active()));
    }

    // **The drop.** Read on the *source* node's response, because that is the
    // widget egui gives the drag to for its whole life — the node under the
    // pointer at the end never sees a release of its own.
    if response.drag_stopped() {
        let dropped_on = ui.ctx().input(|i| i.pointer.interact_pos()).and_then(|p| {
            parenting::nodes(&ctx)
                .into_iter()
                .find(|(_, at)| at.distance(p) <= 11.0)
                .map(|(id, _)| id)
        });
        out.set_follows = parenting::drop_decision(scene.layers(), layer.id, dropped_on)
            .map(|parent| (layer.id, parent));
        parenting::set_dragging(&ctx, None);
    }

    // **Detaching, spelled out.** Dropping a node in open space unlinks it,
    // but a gesture is not an option — there is nothing on screen that says it
    // is possible. A right-click on the node says so, and names the parent it
    // would break from so the answer is never guessed at.
    let parent_name = layer
        .follows
        .and_then(|id| scene.layers().get(id).map(|l| l.name.clone()));
    response.context_menu(|ui| match &parent_name {
        Some(name) => {
            if ui.button(format!("Detach from {name}")).clicked() {
                out.set_follows = Some((layer.id, None));
                ui.close();
            }
        }
        None => {
            ui.label(
                egui::RichText::new("Not parented\nDrag this node onto another to parent it")
                    .small()
                    .weak(),
            );
        }
    });

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        let tip = match &parent_name {
            Some(name) => format!(
                "Follows {name}\nDrag onto another layer's node to re-parent \u{b7} \
                 drag into empty space to detach \u{b7} right-click to detach"
            ),
            None => "Drag onto another layer's node to make this follow it".to_string(),
        };
        response.clone().on_hover_text(tip);
    }
}

fn layer_context_menu(
    ui: &mut Ui,
    scene: &Scene,
    layer: &buzz_scene::Layer,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    ui.label(egui::RichText::new(&layer.name).small().weak());
    ui.separator();

    for (icon, label) in [
        (
            crate::panels::LayerIcon::Eye,
            if layer.visible {
                "Hide Layer"
            } else {
                "Show Layer"
            },
        ),
        (
            crate::panels::LayerIcon::Lock,
            if layer.locked {
                "Unlock Layer"
            } else {
                "Lock Layer"
            },
        ),
        (
            crate::panels::LayerIcon::Outline,
            if layer.outline {
                "Show Layer Filled"
            } else {
                "Show Layer as Outlines"
            },
        ),
    ] {
        if ui.button(label).clicked() {
            out.toggle_layer = Some((layer.id, icon));
            ui.close();
        }
    }

    // H2: Animate's Highlight Layer — a colour to pick this layer out down
    // the track, independent of its automatic colour chip.
    ui.menu_button("Highlight", |ui| {
        crate::panels::highlight_menu_body(ui, layer.highlight, |picked| {
            out.set_highlight = Some((vec![layer.id], picked));
        });
    });
    if state.selected_layers.len() > 1 {
        let selected = state.selected_layers.clone();
        ui.menu_button("Highlight Selected Layers", |ui| {
            crate::panels::highlight_menu_body(ui, None, |picked| {
                out.set_highlight = Some((selected.clone(), picked));
            });
        });
    }

    // **F2: a way out of a folder that does not require dragging.** Placed
    // just after the folder's own block — the same place it would land if
    // it were dragged out and dropped directly below the folder.
    if let Some(folder) = layer.parent {
        if ui.button("Move Out of Folder").clicked() {
            let new_parent = scene.layers().get(folder).and_then(|f| f.parent);
            let mut block = vec![folder];
            block.extend(scene.layers().descendants_of(folder));
            let after = block
                .iter()
                .filter_map(|id| scene.layers().index_of(*id))
                .max()
                .map(|i| i + 1)
                .unwrap_or(0);
            out.move_layer = Some((layer.id, after, new_parent));
            ui.close();
        }
    }

    // **Parenting, without having to find the view.**
    //
    // The node graph is the way to *see* a rig, and the way to build one when
    // the layers are on screen together. This is the other way in: a layer, and
    // the list of layers it could follow, right where the layer is. Both make
    // the same edit.
    ui.separator();

    // **Detaching is one click, at the top level.** It was inside the "Parent
    // To" submenu, which is the right place to *choose* a parent and the wrong
    // place to look for a way out of one — nobody opens a menu called "parent
    // to" to stop parenting. Named for the parent, so it says what it breaks.
    if let Some(parent) = layer.follows {
        let name = scene
            .layers()
            .get(parent)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| "parent".to_string());
        if ui.button(format!("Detach from {name}")).clicked() {
            out.set_follows = Some((layer.id, None));
            ui.close();
        }
    }

    ui.menu_button("Parent To", |ui| {
        if ui
            .add_enabled(layer.follows.is_some(), egui::Button::new("None (detach)"))
            .clicked()
        {
            out.set_follows = Some((layer.id, None));
            ui.close();
        }
        ui.separator();
        let mut offered = false;
        for other in scene.layers().iter() {
            // A layer cannot follow itself, nor anything that already follows
            // it — that would be a cycle with nothing sensible to draw.
            if other.id == layer.id || !scene.layers().can_follow(layer.id, other.id) {
                continue;
            }
            offered = true;
            let already = layer.follows == Some(other.id);
            if ui.selectable_label(already, &other.name).clicked() {
                out.set_follows = Some((layer.id, Some(other.id)));
                ui.close();
            }
        }
        if !offered {
            ui.label(
                egui::RichText::new("No other layer can be its parent")
                    .small()
                    .weak(),
            );
        }
    });

    ui.separator();
    if ui.button("Select All Layers").clicked() {
        out.select_layers = Some(scene.layers().iter().map(|l| l.id).collect());
        ui.close();
    }
    for command in [
        crate::Command::NewLayer,
        crate::Command::NewLayerFolder,
        crate::Command::MoveLayerUp,
        crate::Command::MoveLayerDown,
        crate::Command::DeleteLayer,
    ] {
        if ui.button(command.label()).clicked() {
            out.command = Some(command);
            ui.close();
        }
    }
}

/// The tween drawn in one cell, if any.
///
/// `None` for a frame with no tween, which is the overwhelmingly common case
/// and costs one keyframe lookup.
fn tween_cell(layer: &buzz_scene::Layer, frame: u32, length: u32) -> Option<TweenCell> {
    let span = layer.frames.tween_span_at(frame)?;
    let (r, g, b) = span.tween.kind.timeline_tint()?;
    Some(TweenCell {
        tint: Color32::from_rgb(r, g, b),
        complete: span.is_complete(),
        // The arrowhead sits on the last frame the tween covers, so the span
        // reads as one arrow running between its two keyframes.
        arrow: frame == span.last_frame(length) && frame > span.start,
    })
}

/// How a single cell participates in a tween span.
#[derive(Debug, Clone, Copy)]
struct TweenCell {
    tint: Color32,
    /// False when the tween has no keyframe to run to. Drawn dashed, because
    /// nothing will actually move.
    complete: bool,
    /// Draw the arrowhead here.
    arrow: bool,
}

/// Animate's right-click menu on a frame.
fn frame_context_menu(ui: &mut Ui, scene: &Scene, out: &mut TimelineResponse) {
    if ui.button("Select All Layers at This Frame").clicked() {
        let layers: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
        if let Some(anchor) = layers.first().copied() {
            let frame = out.scrub_to.unwrap_or(0);
            out.select_cells = Some(crate::TimelineSelection::new(
                layers, frame, frame, anchor, frame,
            ));
        }
        ui.close();
    }
    ui.separator();

    for action in [
        FrameAction::InsertFrame,
        FrameAction::RemoveFrame,
        FrameAction::InsertKeyframe,
        FrameAction::InsertBlankKeyframe,
        FrameAction::ClearKeyframe,
    ] {
        let button = egui::Button::new(action.label()).shortcut_text(action.shortcut_text());
        if ui.add(button).clicked() {
            out.action = Some(action);
            ui.close();
        }
    }

    ui.separator();

    for tween in [
        TweenRequest::Motion,
        TweenRequest::Shape,
        TweenRequest::Classic,
        TweenRequest::Remove,
    ] {
        if ui.button(tween.label()).clicked() {
            out.tween = Some(tween);
            ui.close();
        }
    }
}

/// Draw a sound's envelope across the frames it covers.
///
/// Drawn as a bar per frame, mirrored about the middle — the shape of the
/// sound, in the same units as the frames beside it, so what you see lines up
/// with what you can key.
fn draw_waveform(
    painter: &egui::Painter,
    waveform: &Waveform,
    grid_left: f32,
    row: egui::Rect,
    height: f32,
    visible: &std::ops::Range<u32>,
    cell_width: f32,
    // The frame the sound stops on — the next keyframe on the layer — past
    // which no waveform is drawn, so a blank keyframe cuts the strip off.
    end: Option<u32>,
) {
    let middle = row.center().y;
    let half = (height * 0.5 - 2.0).max(1.0);
    let colour = Color32::from_rgba_unmultiplied(120, 215, 255, 150);

    // Only the trimmed section of the clip is drawn, laid out from the sound's
    // start frame: timeline frame `start_frame + i` shows clip level
    // `level_start + i`, up to the out-point (`level_end`) or the clip's end.
    let last_level = waveform
        .level_end
        .unwrap_or(waveform.levels.len())
        .min(waveform.levels.len());
    let mut i = 0u32;
    loop {
        let level_idx = waveform.level_start + i as usize;
        if level_idx >= last_level {
            break;
        }
        let frame = waveform.start_frame + i;
        i += 1;
        // The sound also stops at the next keyframe (a blank keyframe cuts it),
        // so the waveform does too.
        if end.is_some_and(|e| frame >= e) {
            break;
        }
        // Only the bars in view — the strip is virtualized with the cells.
        if frame >= visible.end {
            break;
        }
        if frame < visible.start {
            continue;
        }
        let level = waveform.levels[level_idx];
        let x = grid_left + frame as f32 * cell_width;
        let amplitude = (level.clamp(0.0, 1.0) * half).max(0.5);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x + 0.5, middle - amplitude),
                egui::pos2(x + cell_width - 0.5, middle + amplitude),
            ),
            0.0,
            colour,
        );
    }
}

/// Draw one frame cell using Animate's conventions.
fn draw_frame_cell(
    painter: &egui::Painter,
    cell: egui::Rect,
    kind: FrameKind,
    tween: Option<TweenCell>,
    blank: bool,
    playhead: bool,
    highlight_wash: Option<Color32>,
) {
    // The frame grid, brightest where there is the most to see: a frame that
    // **holds art** stands out light; a **blank** keyframe and the span it holds
    // open are the plainer grey; a frame with **no layer** under it reads as
    // the panel behind it — the theme's own background rather than a block
    // fixed to Dark's. `blank` is true for the whole span a blank keyframe
    // governs, not just its own cell. A tween tints its whole span so it
    // reads apart too. `filled`/`blank_fill` stay close to Animate's own
    // paper regardless of theme — see `Colors::timeline_cell_filled`.
    let filled = Palette::timeline_cell_filled();
    let blank_fill = Palette::timeline_cell_blank();
    let empty = Palette::panel();
    let background = match (kind, tween) {
        (FrameKind::Empty, _) => empty,
        (_, Some(t)) => t.tint,
        _ if blank => blank_fill,
        _ => filled,
    };
    painter.rect_filled(cell, 0.0, background);

    // **Ruled, not boxed.** A full rectangle round every cell draws each line
    // twice — once as one cell's right edge and again as its neighbour's left
    // — and at twelve pixels a cell that doubled ink was most of what the
    // timeline showed: rows looked separated by a gap they did not have, and
    // the frames themselves were hard to count. One hairline down the right
    // and one along the bottom is Animate's grid, and it is half the ink.
    // A subtle grid: a darker hairline on the light occupied cells, a lighter
    // one on the dark empty cells, so the columns read without boxing each frame.
    let on_empty = matches!(kind, FrameKind::Empty) && tween.is_none();
    let rule = if on_empty {
        Palette::timeline_rule_empty()
    } else {
        Palette::timeline_rule_filled()
    };
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(cell.max.x - 1.0, cell.min.y), cell.max),
        0.0,
        rule,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(cell.min.x, cell.max.y - 1.0), cell.max),
        0.0,
        rule,
    );

    // H2: the highlight wash — over the background and its ruling, under
    // every mark the frame draws next, exactly like the name cell's own.
    if let Some(wash) = highlight_wash {
        painter.rect_filled(cell, 0.0, wash);
    }

    if let Some(t) = tween {
        draw_tween_mark(painter, cell, t);
    }

    let centre = cell.center();
    let dot = 3.0;
    // On Animate's light grid the marks are near-black — the same ink a
    // transform handle's outline uses in every theme, since both have to read
    // against a paper-light fill regardless of which way the chrome runs.
    let mark = Palette::handle_stroke();

    match kind {
        FrameKind::Empty => {}
        FrameKind::Keyframe => {
            // Filled circle: a keyframe with artwork.
            painter.circle_filled(egui::pos2(centre.x, cell.max.y - 5.0), dot, mark);
        }
        FrameKind::BlankKeyframe => {
            // Hollow circle: a keyframe that deliberately shows nothing.
            painter.circle_stroke(
                egui::pos2(centre.x, cell.max.y - 5.0),
                dot,
                Stroke::new(1.0, mark),
            );
        }
        FrameKind::Span => {}
        FrameKind::SpanEnd => {
            // Hollow rectangle: the last frame of a span.
            let end = egui::Rect::from_center_size(
                egui::pos2(centre.x, cell.max.y - 5.0),
                egui::vec2(5.0, 5.0),
            );
            painter.rect_stroke(end, 0.0, Stroke::new(1.0, mark), StrokeKind::Inside);
        }
    }

    if playhead {
        // Animate marks the current frame's whole column with a translucent
        // wash, not just an outline — drawn in the theme's own accent rather
        // than a colour fixed to one hex value regardless of theme.
        let [r, g, b, _] = Palette::accent().to_array();
        painter.rect_filled(cell, 0.0, Color32::from_rgba_unmultiplied(r, g, b, 0x40));
    }
}

/// The line an animator reads a tween by: solid with an arrowhead when the
/// tween runs between two keyframes, dashed when it has nowhere to go.
fn draw_tween_mark(painter: &egui::Painter, cell: egui::Rect, tween: TweenCell) {
    let y = cell.center().y;
    // The accent, not plain text ink: the line is what says "this span is
    // tweened" at a glance, so it gets the one colour the theme reserves for
    // "the thing that is happening" rather than blending into the grid.
    let stroke = Stroke::new(1.0, Palette::accent());

    if tween.complete {
        painter.line_segment(
            [egui::pos2(cell.min.x, y), egui::pos2(cell.max.x, y)],
            stroke,
        );
    } else {
        // Animate's broken-tween dashes. Two short strokes per cell is enough
        // to read as dashed at this width without any dash-pattern support.
        let quarter = cell.width() / 4.0;
        for i in 0..2 {
            let x0 = cell.min.x + (i as f32 * 2.0 + 0.5) * quarter;
            painter.line_segment([egui::pos2(x0, y), egui::pos2(x0 + quarter, y)], stroke);
        }
    }

    if tween.arrow {
        let tip = egui::pos2(cell.max.x - 1.0, y);
        let back = 4.0;
        painter.add(egui::Shape::convex_polygon(
            vec![
                tip,
                egui::pos2(tip.x - back, y - 3.0),
                egui::pos2(tip.x - back, y + 3.0),
            ],
            Palette::accent(),
            Stroke::NONE,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::ShapeData;
    use kurbo::Rect;
    use peniko::Color;

    /// The visible-column range is the heart of the timeline's virtualization:
    /// only cells overlapping the clip rectangle are painted, padded a column
    /// each side. Pins plan 1.6's arithmetic.
    #[test]
    fn visible_columns_covers_the_clip_and_pads_by_one() {
        let grid_left = 100.0;
        let cell_width = 10.0;
        let columns = 9_999;

        // A clip showing content x 300..500: cells 20..40, padded to 19..41.
        let clip = egui::Rect::from_min_max(egui::pos2(300.0, 0.0), egui::pos2(500.0, 50.0));
        let range = visible_columns(clip, grid_left, cell_width, columns);
        assert_eq!(range, 19..41);
        // Far fewer than the whole grid — that is the whole point.
        assert!(range.len() < 30);

        // Clamped to the grid at both ends.
        let far_left = egui::Rect::from_min_max(egui::pos2(-1000.0, 0.0), egui::pos2(-500.0, 1.0));
        assert_eq!(
            visible_columns(far_left, grid_left, cell_width, columns).start,
            0
        );
        let far_right =
            egui::Rect::from_min_max(egui::pos2(1e9, 0.0), egui::pos2(1e9 + 100.0, 1.0));
        let r = visible_columns(far_right, grid_left, cell_width, columns);
        assert!(r.start <= columns && r.end <= columns);
    }

    /// Every transport glyph draws at least one shape — the same guarantee
    /// `icons.rs`'s own `every_tool_draws_a_symbol` gives the tool strip, so
    /// a forgotten `Glyph` arm cannot render as a silently empty button.
    #[test]
    fn every_transport_glyph_draws_something() {
        let r = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(22.0, 22.0));
        for g in Glyph::ALL {
            assert!(
                !glyph_shapes(r, g, Color32::WHITE).is_empty(),
                "a transport glyph drew nothing"
            );
        }
    }

    /// A glyph's shapes stay inside the button they are drawn in, at any
    /// button size — the same property `icons.rs`'s
    /// `a_symbol_stays_inside_its_button` checks for the tool strip.
    #[test]
    fn a_transport_glyph_stays_inside_its_button() {
        for side in [8.0f32, 22.0, 64.0] {
            let r = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(side, side));
            for g in Glyph::ALL {
                for shape in glyph_shapes(r, g, Color32::WHITE) {
                    let bounds = shape.visual_bounding_rect();
                    if !bounds.is_positive() {
                        continue;
                    }
                    assert!(
                        r.expand(3.0).contains_rect(bounds),
                        "a transport glyph at {side}px drew {bounds:?} outside {r:?}"
                    );
                }
            }
        }
    }

    fn scene_with_frames() -> Scene {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 50.0, 50.0).to_path(1e-9), Color::WHITE),
        );
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(23);
            l.frames.insert_keyframe(8);
            l.frames.insert_blank_keyframe(16);
        });
        scene.add_layer("Layer_2", LayerKind::Normal);
        scene
    }

    fn state() -> TimelineState {
        TimelineState {
            beats: Vec::new(),
            waveforms: Default::default(),
            current_frame: 4,
            active_layer: None,
            selected_layers: Vec::new(),
            range_anchor: None,
            timeline_selection: None,
            camera_selected: false,
            selected_light: None,
            playing: false,
            onion_enabled: false,
            auto_keyframe: false,
            edit_multiple: false,
            onion_before: 2,
            onion_after: 2,
            frame_width: Metrics::FRAME_WIDTH,
            row_scale: 1.0,
            parenting_view: false,
            depth_view: false,
            focal_distance: 1000.0,
            nearest_depth: -900.0,
            shot_end: None,
        }
    }

    /// **A node's indent is how deep it hangs in the rig.** This is the whole
    /// readability of the parenting view: a spine at the left, a hand three
    /// generations in from it.
    #[test]
    fn a_node_is_indented_by_its_generation() {
        let mut scene = Scene::default();
        let spine = scene.add_layer("Spine", LayerKind::Normal);
        let arm = scene.add_layer("Arm", LayerKind::Normal);
        let hand = scene.add_layer("Hand", LayerKind::Normal);
        scene.update_layer(arm, |l| l.follows = Some(spine));
        scene.update_layer(hand, |l| l.follows = Some(arm));

        assert_eq!(parenting::generation(&scene, spine), 0, "the root");
        assert_eq!(parenting::generation(&scene, arm), 1);
        assert_eq!(parenting::generation(&scene, hand), 2);

        // Indent follows from it, so the graph reads left to right.
        let row = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(210.0, 20.0));
        let x = |id| parenting::node_centre(row, parenting::generation(&scene, id)).x;
        assert!(x(spine) < x(arm) && x(arm) < x(hand));
    }

    /// Defects brief item 3c: a folder's grandchild indents one step further
    /// than its child, not the same flat 12 px every non-root layer used to
    /// get regardless of how many folders deep it actually was.
    #[test]
    fn a_layer_indents_once_per_folder_it_is_filed_under() {
        let mut scene = Scene::default();
        let outer = scene.add_layer("Folder 1", LayerKind::Folder);
        let inner = scene.add_layer("Folder 2", LayerKind::Folder);
        let leaf = scene.add_layer("Leaf", LayerKind::Normal);
        scene.update_layer(inner, |l| l.parent = Some(outer));
        scene.update_layer(leaf, |l| l.parent = Some(inner));

        assert_eq!(folder_depth(&scene, outer), 0, "the root folder");
        assert_eq!(folder_depth(&scene, inner), 1, "one folder down");
        assert_eq!(folder_depth(&scene, leaf), 2, "two folders down");
    }

    /// A `parent` chain that loops back on itself must stop rather than hang
    /// the timeline — the same guarantee `parenting::generation` gives its
    /// own (`follows`) chain.
    #[test]
    fn a_folder_cycle_does_not_hang_the_indent() {
        let mut scene = Scene::default();
        let a = scene.add_layer("A", LayerKind::Folder);
        let b = scene.add_layer("B", LayerKind::Folder);
        scene.update_layer(a, |l| l.parent = Some(b));
        scene.update_layer(b, |l| l.parent = Some(a));

        // Does not hang; the exact number only has to be finite and no larger
        // than the layer count.
        assert!(folder_depth(&scene, a) <= scene.layers().len());
    }

    /// Defects brief item 3a: the name column and the frame grid must share
    /// one row origin. Measured through egui's own painted output rather than
    /// trusted from the source: the layer-name background (`names_bg()`) and
    /// that row's own frame-0 cell (a blank keyframe's cell fill, distinct
    /// from both `names_bg()` and the ruler's background) must land at the
    /// same y for every layer row, in the same order top to bottom.
    ///
    /// `names_bg()` is [`Palette::panel`] now, the same fill the footer strip
    /// and an "empty" frame cell both paint — so a name row is told apart
    /// from those by its width, [`LAYER_COLUMN`] exactly, which neither of
    /// them share (the footer spans the whole panel; an empty cell is one
    /// frame wide).
    #[test]
    fn a_layer_rows_name_and_its_frame_cells_share_one_y_origin() {
        // `Scene::empty()`, not `::default()`: the default document's own
        // starter layer would be a fourth row this test does not ask for.
        // The camera defaults to disabled (`CameraTrack::default`), so there
        // is no camera row either — nothing but the ruler and three plain
        // layers, so every other cell fill in the output is one of theirs.
        let mut scene = Scene::empty();
        for name in ["One", "Two", "Three"] {
            scene.add_layer(name, LayerKind::Normal);
        }

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 600.0),
        ));
        let s = state();
        let output = ctx.run_ui(input, |ui| {
            let _ = timeline_panel(ui, &scene, &s);
        });

        // A freshly added layer has exactly one keyframe, blank, at frame 0
        // (`LayerTimeline::default`) — `draw_frame_cell` paints a blank
        // keyframe's cell in this colour, distinct from every other fill in
        // the panel, so matching it picks out exactly one cell per row: that
        // row's frame 0, the one the name column's own left edge lines up
        // with.
        let blank_keyframe_cell = Palette::timeline_cell_blank();
        let names_bg = names_bg();

        let mut name_ys: Vec<f32> = Vec::new();
        let mut grid_ys: Vec<f32> = Vec::new();
        for clipped in &output.shapes {
            if let egui::Shape::Rect(r) = &clipped.shape {
                if r.fill == names_bg && (r.rect.width() - LAYER_COLUMN).abs() < 0.5 {
                    name_ys.push(r.rect.min.y);
                } else if r.fill == blank_keyframe_cell {
                    grid_ys.push(r.rect.min.y);
                }
            }
        }

        name_ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        name_ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        assert_eq!(name_ys.len(), 3, "expected one name row per layer");

        grid_ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        grid_ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        assert_eq!(grid_ys.len(), 3, "expected one frame-0 cell per layer row");

        for (name_y, grid_y) in name_ys.iter().zip(grid_ys.iter()) {
            assert!(
                (name_y - grid_y).abs() < 0.5,
                "a name row at y={name_y} does not line up with its own \
                 frame grid row at y={grid_y}"
            );
        }
    }

    /// I1: **Shift+click sweeps a range; Ctrl/Cmd+click toggles one.** Drives
    /// real pointer clicks through [`timeline_panel`] itself, not just the
    /// pure `layer_range` function in `selection.rs` — so the modifier
    /// reading and the anchor bookkeeping actually in `layer_row`'s click
    /// handler are what is under test. `state()` is a plain fixture with no
    /// behaviour of its own, so each click's effect on the anchor and the
    /// selection is applied by hand between frames, exactly as
    /// `App::apply_timeline` would.
    /// The playhead is one line through every row, not a wash on two or three
    /// of them: with six layers it runs from the ruler past the last row.
    #[test]
    fn the_playhead_line_runs_down_through_every_row() {
        let mut scene = Scene::empty();
        for name in ["L1", "L2", "L3", "L4", "L5", "L6"] {
            scene.add_layer(name, LayerKind::Normal);
        }
        let ids: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut st = state();
        st.current_frame = 4;
        let screen = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(screen, |ui| {
            let _ = timeline_panel(ui, &scene, &st);
        });
        let line: egui::Rect = ctx
            .data(|d| d.get_temp(egui::Id::new(PLAYHEAD_LINE)))
            .expect("the playhead line is drawn");
        let row = |id: LayerId| -> (egui::Rect, f32, f32) {
            ctx.data(|d| d.get_temp(egui::Id::new((ROW_GEOMETRY, id)))).expect("row drawn")
        };
        let (first, grid_left, cell) = row(ids[0]);
        let (last, _, _) = row(*ids.last().unwrap());
        assert!(line.min.y <= first.min.y + 0.5, "starts at the first row: {line:?} vs {first:?}");
        assert!(line.max.y >= last.max.y - 0.5, "reaches past the last row: {line:?} vs {last:?}");
        let x = grid_left + 4.5 * cell;
        assert!((line.center().x - x).abs() < 0.6, "at frame 5's centre: {} vs {x}", line.center().x);
    }

    /// The selected layer's row is marked across the frames too, with its
    /// colour along the bottom — not only on its name cell.
    #[test]
    fn the_selected_layers_whole_row_is_marked() {
        let mut scene = Scene::empty();
        for name in ["L1", "L2", "L3"] {
            scene.add_layer(name, LayerKind::Normal);
        }
        let ids: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut st = state();
        st.active_layer = Some(ids[1]);
        let screen = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0))),
            ..Default::default()
        };
        let out = ctx.run_ui(screen, |ui| {
            let _ = timeline_panel(ui, &scene, &st);
        });
        let (row, grid_left, _) = ctx
            .data(|d| d.get_temp::<(egui::Rect, f32, f32)>(egui::Id::new((ROW_GEOMETRY, ids[1]))))
            .expect("row drawn");
        let [r, g, b, _] = scene.layers().get(ids[1]).unwrap().color.to_rgba8().to_u8_array();
        let colour = Color32::from_rgb(r, g, b);
        let underline = out.shapes.iter().any(|c| match &c.shape {
            egui::Shape::Rect(s) => {
                s.fill == colour
                    && (s.rect.min.x - grid_left).abs() < 0.5
                    && (s.rect.max.y - row.max.y).abs() < 0.5
                    && s.rect.height() <= 2.5
                    && s.rect.width() > 100.0
            }
            _ => false,
        });
        assert!(underline, "the layer's colour runs along the bottom of its frames");
    }

    #[test]
    fn shift_click_sweeps_a_range_and_ctrl_click_toggles_one() {
        let mut scene = Scene::empty();
        for name in ["L1", "L2", "L3", "L4", "L5", "L6"] {
            scene.add_layer(name, LayerKind::Normal);
        }
        let ids: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
        assert_eq!(ids.len(), 6);

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut st = state();

        let screen = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..Default::default()
        };

        // A plain render finds each row's own name-column rect, top to
        // bottom — the same fill and width `a_layer_rows_name...` keys on.
        // Row order here matches `ids`' order: both come from the same
        // front-to-back `scene.layers()` iteration.
        let names_bg = names_bg();
        let out = ctx.run_ui(screen(), |ui| {
            let _ = timeline_panel(ui, &scene, &st);
        });
        let mut rows: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r)
                    if r.fill == names_bg && (r.rect.width() - LAYER_COLUMN).abs() < 0.5 =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .collect();
        rows.sort_by(|a, b| a.min.y.partial_cmp(&b.min.y).unwrap());
        rows.dedup_by(|a, b| (a.min.y - b.min.y).abs() < 0.5);
        assert_eq!(rows.len(), 6, "expected one row per layer");
        let click_pos = |row: egui::Rect| egui::pos2(row.min.x + 60.0, row.center().y);

        let click = |pos: egui::Pos2, modifiers: egui::Modifiers, st: &TimelineState| -> TimelineResponse {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    }],
                    modifiers,
                    ..screen()
                },
                |ui| {
                    let _ = timeline_panel(ui, &scene, st);
                },
            );
            let mut response = TimelineResponse::default();
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers,
                    }],
                    modifiers,
                    ..screen()
                },
                |ui| {
                    response = timeline_panel(ui, &scene, st);
                },
            );
            response
        };

        // A plain click on row 2 becomes the anchor.
        let r = click(click_pos(rows[1]), egui::Modifiers::NONE, &st);
        assert_eq!(r.select_layer, Some(ids[1]));
        st.active_layer = Some(ids[1]);
        st.selected_layers = vec![ids[1]];
        st.range_anchor = Some(ids[1]);

        // Shift+click on row 6 sweeps rows 2 through 6 — five layers.
        let r = click(click_pos(rows[5]), egui::Modifiers::SHIFT, &st);
        let mut swept = r.select_layers.expect("a shift-click range");
        swept.sort();
        let mut expected: Vec<LayerId> = ids[1..=5].to_vec();
        expected.sort();
        assert_eq!(swept.len(), 5, "row 2 through row 6 is five layers");
        assert_eq!(swept, expected);
        assert_eq!(
            r.set_range_anchor, None,
            "a shift-click leaves the anchor where it was"
        );

        // Back to just row 2 selected; Ctrl+click row 4 toggles it in.
        st.selected_layers = vec![ids[1]];
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let r = click(click_pos(rows[3]), ctrl, &st);
        let mut toggled_in = r.select_layers.expect("a ctrl-click toggle");
        toggled_in.sort();
        let mut expected_in = vec![ids[1], ids[3]];
        expected_in.sort();
        assert_eq!(toggled_in, expected_in);
        assert_eq!(
            r.set_range_anchor,
            Some(ids[3]),
            "ctrl-click moves the anchor to the layer it toggled"
        );

        // Ctrl+click row 4 again removes it — a plain toggle, not a range.
        st.selected_layers = expected_in;
        let r = click(click_pos(rows[3]), ctrl, &st);
        assert_eq!(r.select_layers, Some(vec![ids[1]]));
    }

    /// **The frame grid's canvas follows the theme.** `draw_frame_cell` used
    /// to paint an "empty" cell — no layer reaching this frame — in a colour
    /// fixed to Dark's own tone, so the timeline stayed a dark block in Light
    /// and Contrast while every panel around it followed the theme. It now
    /// reads as [`Palette::panel`], the same background the rest of the dock
    /// sits on, so this renders the timeline under each of the three themes
    /// and checks that the plain grid cell painted past the name column is
    /// exactly the current theme's panel colour — and that Light's is
    /// markedly lighter than Dark's, not merely a different dark tone.
    #[test]
    fn the_frame_grid_canvas_tracks_the_panel_role() {
        use crate::theme::{self, Theme};

        fn luminance(c: Color32) -> f32 {
            0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32
        }

        let was = theme::theme();
        let mut luminances = std::collections::HashMap::new();
        for t in [Theme::Dark, Theme::Light, Theme::Contrast] {
            theme::set_theme(t);
            let ctx = egui::Context::default();
            theme::apply(&ctx);

            // One short layer: past its own frame it draws nothing, which is
            // exactly the "empty" cell whose colour this test is about.
            let mut scene = Scene::empty();
            scene.add_layer("Only", LayerKind::Normal);
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 300.0),
            ));
            let s = state();
            let output = ctx.run_ui(input, |ui| {
                let _ = timeline_panel(ui, &scene, &s);
            });

            let panel = Palette::panel();
            // Past the name column, wider than the 1 px grid rules, and
            // clipped to the frame grid's own region — not just anywhere past
            // x=260, which the transport row's separators and slider handles
            // also satisfy — so this can only be a cell's own background fill.
            let canvas = output.shapes.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(r)
                    if r.rect.min.x > LAYER_COLUMN + 50.0
                        && r.rect.width() > 1.0
                        && clipped.clip_rect.min.x >= LAYER_COLUMN - 1.0 =>
                {
                    Some(r.fill)
                }
                _ => None,
            });
            let canvas = canvas
                .unwrap_or_else(|| panic!("{t:?}: no frame-grid cell painted past the name column"));
            assert_eq!(
                canvas, panel,
                "{t:?}: an empty frame cell should read as the panel behind it, \
                 not a colour fixed to one theme"
            );
            luminances.insert(t, luminance(canvas));
        }
        theme::set_theme(was);

        assert!(
            luminances[&Theme::Light] > luminances[&Theme::Dark] + 100.0,
            "the light theme's timeline canvas is not light: {} against {}",
            luminances[&Theme::Light],
            luminances[&Theme::Dark]
        );
    }

    /// The room the Timeline's header line reserves for the layer tools
    /// ([`LAYER_TOOLS_INLINE_WIDTH`]) holds the whole row at the theme's own
    /// sizes — so drawn there, nothing (the Reset button last of all) is
    /// clipped off its end.
    #[test]
    fn the_layer_tools_fit_the_header_slot_they_ask_for() {
        let scene = scene_with_frames();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut width = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(4000.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                let mut out = TimelineResponse::default();
                let row = ui.horizontal(|ui| layer_tools(ui, &scene, &state(), &mut out));
                width = row.response.rect.width();
            },
        );
        assert!(width > 300.0, "the layer tools drew nothing measurable ({width})");
        assert!(
            width <= LAYER_TOOLS_INLINE_WIDTH,
            "the layer tools are {width:.0} points wide, more than the              {LAYER_TOOLS_INLINE_WIDTH} the header line reserves for them"
        );
    }

    /// Defects brief item 3b: the New Layer / New Folder / Delete strip is
    /// reserved *before* the frame grid's scroll area lays out (see
    /// [`timeline_panel`]'s own `egui::Panel::bottom` call), so the grid
    /// never has room to draw a row into where that strip sits — measured by
    /// checking that no drawn layer-name row reaches past the panel height
    /// [`LAYER_TOOLS_HEIGHT`] leaves for it, in a viewport short enough that
    /// this would fail immediately if the reservation were missing or
    /// ordered the other way round.
    #[test]
    fn the_footer_never_overlaps_the_layer_list() {
        let mut scene = Scene::empty();
        for i in 0..10 {
            scene.add_layer(format!("Layer {i}"), LayerKind::Normal);
        }

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let screen_height = 220.0;
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, screen_height),
        ));
        let s = state();
        let output = ctx.run_ui(input, |ui| {
            let _ = timeline_panel(ui, &scene, &s);
        });

        // The *visible* extent, not the shape's own unclipped geometry —
        // egui clips a shape at render time via `ClippedShape::clip_rect`
        // rather than truncating the geometry itself, so a row's rect can
        // legitimately extend past its scroll area's edge without a single
        // one of its pixels actually landing there.
        //
        // `names_bg()` is [`Palette::panel`], which the footer strip's own
        // background also paints — filtered out by width, since only a name
        // row is exactly [`LAYER_COLUMN`] wide.
        let names_bg = names_bg();
        let mut lowest_row_bottom = 0.0f32;
        for clipped in &output.shapes {
            if let egui::Shape::Rect(r) = &clipped.shape
                && r.fill == names_bg
                && (r.rect.width() - LAYER_COLUMN).abs() < 0.5
            {
                let visible = r.rect.intersect(clipped.clip_rect);
                lowest_row_bottom = lowest_row_bottom.max(visible.max.y);
            }
        }

        assert!(lowest_row_bottom > 0.0, "no layer row drew at all");
        let footer_top = screen_height - LAYER_TOOLS_HEIGHT;
        assert!(
            lowest_row_bottom <= footer_top + 1.0,
            "a layer row's bottom edge reached y={lowest_row_bottom}, past the \
             y={footer_top} the footer needs — the list overlaps the footer"
        );
    }

    /// **What a drop means**, including the case that used to destroy work: a
    /// click on a node registers as a tiny drag that ends where it began, and
    /// reading that as "detach" cut the link the node was drawn to show.
    #[test]
    fn dropping_a_node_back_on_itself_changes_nothing() {
        let mut scene = Scene::default();
        let spine = scene.add_layer("Spine", LayerKind::Normal);
        let arm = scene.add_layer("Arm", LayerKind::Normal);
        let hand = scene.add_layer("Hand", LayerKind::Normal);
        scene.update_layer(arm, |l| l.follows = Some(spine));
        scene.update_layer(hand, |l| l.follows = Some(arm));
        let layers = scene.layers();

        assert_eq!(
            parenting::drop_decision(layers, arm, Some(arm)),
            None,
            "a click that went nowhere must not unlink anything"
        );
        assert_eq!(
            parenting::drop_decision(layers, hand, Some(spine)),
            Some(Some(spine)),
            "a legal parent is taken"
        );
        assert_eq!(
            parenting::drop_decision(layers, arm, None),
            Some(None),
            "open column detaches"
        );
        // Spine already follows nothing, but arm follows it: parenting the
        // spine to the hand would close a loop, so the drop detaches instead
        // of building something undrawable.
        assert_eq!(
            parenting::drop_decision(layers, spine, Some(hand)),
            Some(None),
            "a cycle is refused"
        );
    }

    /// **A right-click on the stage must open its menu.**
    ///
    /// The stage's own widget senses click *and* drag, because the tools need
    /// the drag; this pins down that the two can coexist, which is the thing
    /// that was in doubt when the menu did not appear.
    #[test]
    fn a_secondary_click_opens_the_context_menu_on_a_draggable_widget() {
        let ctx = egui::Context::default();

        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let at = egui::pos2(200.0, 150.0);
        let mods = egui::Modifiers::default();

        let mut opened = false;
        let mut drive = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let response =
                    ui.interact(area, ui.id().with("probe"), egui::Sense::click_and_drag());
                response.context_menu(|ui| {
                    let _ = ui.button("Cut");
                });
                if response.context_menu_opened() {
                    opened = true;
                }
            });
        };

        drive(vec![egui::Event::PointerMoved(at)]);
        drive(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: mods,
        }]);
        drive(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: mods,
        }]);
        drive(vec![]);

        assert!(
            opened,
            "a secondary click on a click-and-drag widget should open its menu"
        );
    }

    /// P1's paging rule, as a pure decision: it fires only when the frame
    /// just moved (or playback is running), never mid-drag, and only when
    /// the playhead has actually left the visible band.
    #[test]
    fn playhead_follow_target_pages_only_when_the_frame_moved_and_is_out_of_view() {
        let pinned_left = 0.0;
        let clip_right = 1000.0;
        let cell_width = 10.0;

        // Off-screen to the right, but the frame did not just change and
        // playback is not running: a manual scroll is left alone.
        assert_eq!(
            playhead_follow_target(false, false, false, 2000.0, pinned_left, clip_right, cell_width),
            None
        );

        // The frame changed, and the playhead sits well past the right edge:
        // page to it.
        let target = playhead_follow_target(
            true, false, false, 2000.0, pinned_left, clip_right, cell_width,
        );
        assert_eq!(target, Some(2000.0 - LAYER_COLUMN));

        // Playback running has the same effect as a frame change.
        assert!(playhead_follow_target(false, true, false, 2000.0, pinned_left, clip_right, cell_width).is_some());

        // Already inside the visible band: nothing to do.
        let visible_x = pinned_left + LAYER_COLUMN + 20.0;
        assert_eq!(
            playhead_follow_target(true, false, false, visible_x, pinned_left, clip_right, cell_width),
            None
        );

        // Dragging the grid or ruler always wins, even off-screen.
        assert_eq!(
            playhead_follow_target(true, true, true, 2000.0, pinned_left, clip_right, cell_width),
            None
        );
    }

    /// **M1**: the ruler's context menu names the frame that was under the
    /// pointer when it opened, and keeps naming it even as the pointer then
    /// travels toward the popup — it must not re-read the live pointer and
    /// re-pick a frame under wherever the popup itself now sits.
    #[test]
    fn ruler_context_menu_frame_stays_fixed_as_the_pointer_moves_toward_it() {
        let ctx = egui::Context::default();
        let scene = Scene::default();
        let st = state();
        let columns = 200u32;
        let cell_width = st.frame_width;

        // Frame index 3 (shown as "Frame 4"), inside the ruler's own strip.
        let open_at = egui::pos2(LAYER_COLUMN + 3.0 * cell_width + 1.0, 8.0);
        // Far along the ruler — where the pointer travels once the menu is
        // open, well past the popup that opened at `open_at`.
        let later_at = egui::pos2(LAYER_COLUMN + 30.0 * cell_width + 1.0, 8.0);
        let mods = egui::Modifiers::default();

        let mut texts: Vec<String> = Vec::new();
        let mut drive = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(2000.0, 600.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| {
                let mut response = TimelineResponse::default();
                ruler(ui, &scene, columns, LoopRegion::default(), &st, &mut response);
            });
            texts = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        };

        drive(vec![egui::Event::PointerMoved(open_at)]);
        drive(vec![egui::Event::PointerButton {
            pos: open_at,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: mods,
        }]);
        drive(vec![egui::Event::PointerButton {
            pos: open_at,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: mods,
        }]);
        drive(vec![]);
        // The pointer now moves away, toward the popup — across a couple of
        // frames, as it would while actually travelling there.
        drive(vec![egui::Event::PointerMoved(later_at)]);
        drive(vec![]);

        assert!(
            texts.iter().any(|t| t == "Frame 4"),
            "the menu must still name the frame it opened on: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t == "Frame 31"),
            "the menu must not have re-picked the frame under the pointer's \
             later position: {texts:?}"
        );
    }

    /// The layer column has three things it can be, and each has to draw for
    /// a real document — including a layer pushed behind the camera, which is
    /// the value the depth scale has to clamp rather than divide by.
    #[test]
    fn every_layer_column_view_draws() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        let spine = scene.add_layer("Spine", LayerKind::Normal);
        let arm = scene.add_layer("Arm", LayerKind::Normal);
        scene.update_layer(arm, |l| l.follows = Some(spine));
        scene.update_layer(arm, |l| l.depth = 400.0);
        scene.update_layer(spine, |l| l.depth = -9_000.0);

        for (parenting, depth) in [(false, false), (true, false), (false, true)] {
            let _ = ctx.run_ui(Default::default(), |ui| {
                let mut st = state();
                st.parenting_view = parenting;
                st.depth_view = depth;
                let _ = timeline_panel(ui, &scene, &st);
            });
        }
    }

    /// **The layer column claims no space of its own.**
    ///
    /// Every row is one `allocate_exact_size` and then paint; anything in the
    /// name column that *allocates* adds its own height to the row. The depth
    /// cell used `Ui::put`, which builds a child `Ui` and advances the parent
    /// cursor past it — so every layer grew by the height of its depth box and
    /// the rows drifted apart down the timeline.
    #[test]
    fn the_layer_column_claims_no_space_of_its_own() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        let spine = scene.add_layer("Spine", LayerKind::Normal);
        let arm = scene.add_layer("Arm", LayerKind::Normal);
        scene.update_layer(arm, |l| l.follows = Some(spine));
        scene.update_layer(arm, |l| l.depth = 120.0);
        let layer = scene.layers().get(arm).expect("the layer").clone();

        for depth_view in [true, false] {
            let mut moved = egui::Vec2::ZERO;
            let _ = ctx.run_ui(Default::default(), |ui| {
                let row =
                    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(LAYER_COLUMN, 20.0));
                let painter = ui.painter().clone();
                let mut out = TimelineResponse::default();
                let mut st = state();
                st.depth_view = depth_view;
                st.parenting_view = !depth_view;

                let before = ui.cursor().min;
                if depth_view {
                    depth_cell(ui, &layer, row, &painter, &st, &mut out);
                } else {
                    parenting_node(ui, &scene, &layer, row, &painter, &mut out);
                }
                moved = ui.cursor().min - before;
            });
            assert_eq!(
                moved,
                egui::Vec2::ZERO,
                "depth_view={depth_view}: the column moved the cursor by {moved:?},                  which is a gap under every layer"
            );
        }
    }

    /// A file edited by hand can hold a follow cycle. The timeline must draw
    /// something and move on rather than spin, which is why `generation` is
    /// bounded by the layer count.
    #[test]
    fn a_follow_cycle_does_not_hang_the_parenting_view() {
        let mut scene = Scene::default();
        let a = scene.add_layer("A", LayerKind::Normal);
        let b = scene.add_layer("B", LayerKind::Normal);
        // `can_follow` refuses this, so it is written straight in — the point
        // is what happens when a *document* already contains one.
        scene.update_layer(a, |l| l.follows = Some(b));
        scene.update_layer(b, |l| l.follows = Some(a));

        assert!(parenting::generation(&scene, a) <= scene.layers().len());
        assert!(parenting::generation(&scene, b) <= scene.layers().len());
    }

    /// The camera row appears only once the camera is switched on — which is
    /// exactly when Animate adds it.
    #[test]
    fn the_camera_row_appears_with_the_camera() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        assert!(!scene.camera().enabled, "the camera starts off");

        // Off: the panel draws the layers and nothing else.
        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = timeline_panel(ui, &scene, &state());
        });

        scene.camera_mut().enabled = true;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = timeline_panel(ui, &scene, &state());
        });
    }

    /// **The camera belongs to the film, not to each symbol.** On the main
    /// timeline the enabled camera shows its row; step into a symbol and the
    /// row is gone, because a symbol has no camera of its own.
    #[test]
    fn the_camera_row_is_hidden_inside_a_symbol() {
        let mut scene = Scene::default();
        scene.camera_mut().enabled = true;
        assert!(shows_camera_row(&scene), "the camera row shows at the root");

        let symbol = scene.add_symbol("S", buzz_scene::SymbolKind::Graphic, None);
        scene.enter_symbol(symbol);
        assert!(
            !shows_camera_row(&scene),
            "the camera row must not appear inside a symbol"
        );

        scene.exit_symbol();
        assert!(
            shows_camera_row(&scene),
            "and it returns on the main timeline"
        );
    }

    /// **C1**: a camera switched off still shows its row once it carries any
    /// keys — a shot already cut, and a lock worth reaching, do not vanish
    /// from the timeline just because the eye is closed.
    #[test]
    fn the_camera_row_stays_once_it_has_keys_even_when_switched_off() {
        let mut scene = Scene::default();
        assert!(
            !shows_camera_row(&scene),
            "an empty, disabled camera has nothing to show"
        );

        scene
            .camera_mut()
            .set_key(buzz_scene::CameraKey::new(0, kurbo::Point::ZERO));
        scene.camera_mut().enabled = false;
        assert!(
            shows_camera_row(&scene),
            "a keyed camera keeps its row even switched off"
        );
    }

    /// **F1's drop-zone decision, as a pure fn:** the outer quarters are
    /// always "beside it"; the middle half is "into it" only for a folder.
    #[test]
    fn drop_zone_picks_into_only_for_a_folder_in_the_middle_band() {
        assert_eq!(drop_zone(0.0, true), DropZone::Above);
        assert_eq!(drop_zone(0.24, false), DropZone::Above);
        assert_eq!(drop_zone(0.5, true), DropZone::Into);
        assert_eq!(
            drop_zone(0.5, false),
            DropZone::Below,
            "a plain layer has nothing to hold a drop, so the middle picks a side"
        );
        assert_eq!(drop_zone(0.4, false), DropZone::Above);
        assert_eq!(drop_zone(0.75, true), DropZone::Below);
        assert_eq!(drop_zone(1.0, false), DropZone::Below);
    }

    /// With the camera row selected the panel still draws every layer: the
    /// camera is another row, not a mode.
    #[test]
    fn the_camera_row_draws_selected() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        scene.camera_mut().enabled = true;
        scene.camera_mut().set_key(buzz_scene::CameraKey::new(
            0,
            buzz_geom::Point::new(0.0, 0.0),
        ));
        scene.camera_mut().set_key(buzz_scene::CameraKey::new(
            12,
            buzz_geom::Point::new(80.0, 0.0),
        ));

        let selected = TimelineState {
            camera_selected: true,
            selected_light: None,
            ..state()
        };
        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = timeline_panel(ui, &scene, &selected);
        });
    }

    /// A response starts with nothing selected, so drawing the panel cannot
    /// silently steal the selection from a layer.
    #[test]
    fn drawing_the_camera_row_selects_nothing() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut scene = Scene::default();
        scene.camera_mut().enabled = true;

        let mut response = TimelineResponse::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            response = timeline_panel(ui, &scene, &state());
        });
        assert!(!response.select_camera);
    }

    #[test]
    fn frame_actions_carry_animates_shortcut_labels() {
        assert_eq!(FrameAction::InsertFrame.shortcut_text(), "F5");
        assert_eq!(FrameAction::InsertKeyframe.shortcut_text(), "F6");
        assert_eq!(FrameAction::InsertBlankKeyframe.shortcut_text(), "F7");
        assert_eq!(FrameAction::RemoveFrame.shortcut_text(), "Shift+F5");
        assert_eq!(FrameAction::ClearKeyframe.shortcut_text(), "Shift+F6");
        for action in [
            FrameAction::InsertFrame,
            FrameAction::RemoveFrame,
            FrameAction::InsertKeyframe,
            FrameAction::InsertBlankKeyframe,
            FrameAction::ClearKeyframe,
        ] {
            assert!(!action.label().is_empty());
        }
    }

    /// Animate's three tween colours are how an animator tells the kinds apart
    /// without clicking anything, so each must reach the cell distinctly.
    #[test]
    fn each_tween_kind_tints_its_span_in_its_own_colour() {
        use buzz_scene::{Tween, TweenKind};

        let mut seen = Vec::new();
        for (tween, kind) in [
            (Tween::motion(), TweenKind::Motion),
            (Tween::classic(), TweenKind::Classic),
            (Tween::shape(), TweenKind::Shape),
        ] {
            let mut scene = Scene::default();
            let id = scene.layers().iter().next().unwrap().id;
            scene.update_layer(id, |l| {
                l.frames.insert_frame(19);
                l.frames.insert_keyframe(10);
                l.frames.set_tween(0, tween);
            });
            let layer = scene.layers().get(id).unwrap();

            let cell = tween_cell(layer, 5, layer.length()).expect("frame 5 is inside the tween");
            assert!(cell.complete, "{kind:?} runs to the keyframe at 10");
            assert!(!cell.arrow, "the arrowhead belongs at the end of the span");

            assert!(
                tween_cell(layer, 10, layer.length()).is_none(),
                "the span stops at the next keyframe"
            );
            let end = tween_cell(layer, 9, layer.length()).expect("frame 9 ends the tween");
            assert!(end.arrow, "{kind:?} needs an arrowhead on its last frame");

            seen.push(cell.tint);
        }

        seen.dedup();
        assert_eq!(seen.len(), 3, "the three tween colours must all differ");
    }

    /// A tween that cannot run is drawn differently from one that can — this is
    /// the first thing to look at when a new tween appears to do nothing.
    #[test]
    fn a_tween_with_nowhere_to_go_is_marked_incomplete() {
        let mut scene = Scene::default();
        let id = scene.layers().iter().next().unwrap().id;
        scene.update_layer(id, |l| {
            l.frames.insert_frame(9);
            l.frames.set_tween(0, buzz_scene::Tween::classic());
        });
        let layer = scene.layers().get(id).unwrap();

        let cell = tween_cell(layer, 5, layer.length()).expect("the tween is set");
        assert!(!cell.complete, "there is no keyframe after frame 0");
    }

    #[test]
    fn an_untweened_frame_has_no_tween_mark() {
        let scene = scene_with_frames();
        let layer = scene.layers().iter().next().unwrap();
        for frame in 0..24 {
            assert!(
                tween_cell(layer, frame, layer.length()).is_none(),
                "frame {frame} carries no tween"
            );
        }
    }

    #[test]
    fn the_panel_draws_without_panicking() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let scene = scene_with_frames();

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = timeline_panel(ui, &scene, &state());
        });
    }

    #[test]
    fn an_empty_document_draws_a_usable_timeline() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let scene = Scene::default();

        let _ = ctx.run_ui(Default::default(), |ui| {
            let response = timeline_panel(ui, &scene, &state());
            assert!(response.action.is_none());
        });
    }

    /// A long timeline must not try to draw tens of thousands of columns.
    #[test]
    fn a_very_long_document_stays_bounded() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(50_000);
        });

        let started = std::time::Instant::now();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = timeline_panel(ui, &scene, &state());
        });
        assert!(
            started.elapsed().as_millis() < 2_000,
            "drawing a 50k-frame timeline took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn frame_kinds_come_through_from_the_document() {
        let scene = scene_with_frames();
        let layer = scene.layers().iter().last().unwrap();

        assert_eq!(layer.frame_kind(0), FrameKind::Keyframe);
        assert_eq!(layer.frame_kind(4), FrameKind::Span);
        assert_eq!(layer.frame_kind(8), FrameKind::Keyframe);
        assert_eq!(layer.frame_kind(16), FrameKind::BlankKeyframe);
        assert_eq!(layer.frame_kind(23), FrameKind::SpanEnd);
        assert_eq!(layer.frame_kind(24), FrameKind::Empty);
    }

    #[test]
    fn the_document_length_is_the_longest_layer() {
        let scene = scene_with_frames();
        assert_eq!(scene.frame_count(), 24);
        assert!(
            (scene.duration_seconds() - 1.0).abs() < 0.001,
            "24 frames at 24 fps"
        );
    }
}

#[cfg(test)]
mod layer_switch_tests {
    use super::*;
    use crate::panels::LayerIcon;

    fn row() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(LAYER_COLUMN + 400.0, 20.0))
    }

    /// **A switch is clickable exactly where it is drawn.**
    ///
    /// The row is painted and hit-tested separately — there is no widget to
    /// keep the two in step — so the one thing that must hold is that the
    /// rectangle the eye is drawn in is the rectangle that toggles it. Off by
    /// a few points and the icon is decoration.
    #[test]
    fn every_switch_is_hit_where_it_is_drawn() {
        let row = row();
        for (column, icon) in switch_columns(row).iter().zip(LayerIcon::ALL) {
            assert_eq!(
                switch_at(row, column.center()),
                Some(icon),
                "{icon:?} is not clickable at its own centre"
            );
            // And the corners, because a hit test that only works in the middle
            // of a fifteen-point square is a fight.
            for corner in [
                column.min + egui::vec2(1.0, 1.0),
                column.max - egui::vec2(1.0, 1.0),
            ] {
                assert_eq!(
                    switch_at(row, corner),
                    Some(icon),
                    "{icon:?} misses a corner"
                );
            }
        }
    }

    /// The three columns sit inside the name column, in order, and do not
    /// overlap each other or run into the frame grid.
    #[test]
    fn the_switch_columns_fit_the_layer_name_column() {
        let row = row();
        let columns = switch_columns(row);
        let grid_left = row.min.x + LAYER_COLUMN;

        for column in &columns {
            assert!(
                column.left() >= row.min.x && column.right() <= grid_left,
                "a switch at {column:?} is outside the {LAYER_COLUMN}-point name column"
            );
        }
        for pair in columns.windows(2) {
            assert!(
                pair[0].right() <= pair[1].left(),
                "the switch columns overlap: {:?} and {:?}",
                pair[0],
                pair[1]
            );
        }
        // Eye, padlock, outline — left to right, as Animate rules them.
        assert!(columns[0].left() < columns[1].left());
        assert!(columns[1].left() < columns[2].left());
    }

    /// The name never runs under the switches: there is real room left for it,
    /// and the two areas do not overlap.
    #[test]
    fn the_name_has_room_of_its_own() {
        let row = row();
        let name = name_area(row);
        let first = switch_columns(row)[0];

        assert!(
            name.right() <= first.left(),
            "the name area runs into the switches"
        );
        // Enough for an ordinary layer name — "Right hand" and the like — at
        // the eleven points the row draws them in.
        assert!(
            name.width() >= 120.0,
            "only {:.0} points left for a layer name",
            name.width()
        );
    }

    /// A click anywhere else in the row is not a switch — otherwise selecting
    /// a layer by its name would silently hide it.
    #[test]
    fn the_rest_of_the_row_is_not_a_switch() {
        let row = row();
        for x in [2.0, 40.0, name_area(row).right() - 2.0, LAYER_COLUMN + 50.0] {
            assert_eq!(
                switch_at(row, egui::pos2(x, row.center().y)),
                None,
                "x = {x} was taken for a switch"
            );
        }
    }

    /// The timeline and the Layers panel draw the same three switches, in the
    /// same order, meaning the same things.
    #[test]
    fn the_two_places_agree_about_what_the_switches_are() {
        assert_eq!(
            LayerIcon::ALL,
            [LayerIcon::Eye, LayerIcon::Lock, LayerIcon::Outline]
        );
        for icon in LayerIcon::ALL {
            assert!(!icon.heading().is_empty());
            assert!(!icon.hint(true).is_empty());
            assert!(!icon.hint(false).is_empty());
        }
    }

    #[test]
    fn selecting_all_layers_via_timeline_response() {
        let mut scene = Scene::default();
        let l1 = scene.add_layer("L1", LayerKind::Normal);
        let l2 = scene.add_layer("L2", LayerKind::Normal);
        let response = TimelineResponse {
            select_layers: Some(vec![l1, l2]),
            scrub_to: Some(5),
            ..Default::default()
        };
        assert_eq!(response.select_layers, Some(vec![l1, l2]));
        assert_eq!(response.scrub_to, Some(5));
    }
}

#[cfg(test)]
mod light_drop_tests {
    use super::*;

    /// Where a dragged light lands on each part of each row.
    #[test]
    fn a_light_lands_above_the_row_half_it_is_dropped_on() {
        let mut scene = Scene::default();
        let a = scene.add_stage_layer("A", LayerKind::Normal);
        let b = scene.add_stage_layer("B", LayerKind::Normal);
        let rows: Vec<LayerId> = scene.stage_layers().iter().map(|l| l.id).collect();
        let (first, next, last) = (rows[0], rows[1], *rows.last().unwrap());
        assert!([a, b].contains(&first), "a new layer goes in front");

        // Top half of a layer: directly above it.
        assert_eq!(light_drop_slot(&scene, DropRow::Layer(first), false), Some(Some(first)));
        // Bottom half: above the layer beneath it.
        assert_eq!(light_drop_slot(&scene, DropRow::Layer(first), true), Some(Some(next)));
        // Bottom half of the last layer is not a place — it would light nothing.
        assert_eq!(light_drop_slot(&scene, DropRow::Layer(last), true), None);
        // Onto another light: that light's own slot, top of stack included.
        assert_eq!(light_drop_slot(&scene, DropRow::Light(None), true), Some(None));
        assert_eq!(light_drop_slot(&scene, DropRow::Light(Some(last)), false), Some(Some(last)));
    }
}
