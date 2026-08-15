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

use buzz_scene::{FrameKind, LayerId, LayerKind, LoopRegion, MAX_REPEATS, Scene};
use egui::{Align2, Color32, FontId, Sense, Stroke, StrokeKind, Ui};

use crate::theme::{Metrics, Palette};

/// What the user did in the timeline.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TimelineResponse {
    /// The playhead was moved here.
    pub scrub_to: Option<u32>,
    /// The camera row was clicked: select the camera, as Animate does when you
    /// click its layer.
    pub select_camera: bool,
    /// A layer row was clicked.
    pub select_layer: Option<LayerId>,
    /// One of the three switches beside a layer's name was clicked.
    ///
    /// The timeline carries Animate's eye, padlock and outline columns beside
    /// its layer names, and they are the same three switches the Layers panel
    /// draws — the same painted icons, the same words, and the same edit. Two
    /// places to reach them, one meaning.
    pub toggle_layer: Option<(LayerId, crate::panels::LayerIcon)>,
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

/// State the panel needs but does not own.
pub struct TimelineState {
    pub current_frame: u32,
    pub active_layer: Option<LayerId>,
    /// The camera row is the selected one, so the camera is what the Camera
    /// menu's keyframe commands act on.
    pub camera_selected: bool,
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
    /// Loudness per frame for each layer carrying a sound, so the timeline can
    /// draw the waveform where the sound actually sits.
    ///
    /// Supplied by the editor rather than read from the document, because the
    /// document stores the *file* and the envelope only exists once it has
    /// been decoded — which is view state, and belongs outside the model.
    pub waveforms: std::collections::BTreeMap<LayerId, Waveform>,
}

/// A sound's envelope, positioned on the timeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Waveform {
    /// The frame the sound starts on.
    pub start_frame: u32,
    /// Loudness per frame, `0.0..=1.0`.
    pub levels: Vec<f32>,
}

/// Width reserved for the layer-name column.
///
/// Wider than it was by the width of the three switch columns, so that adding
/// them took the room from the window rather than from the layer names.
const LAYER_COLUMN: f32 = 210.0;

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

/// Draw the timeline.
pub fn timeline_panel(ui: &mut Ui, scene: &Scene, state: &TimelineState) -> TimelineResponse {
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
    egui::Panel::bottom("timeline-layer-tools")
        .resizable(false)
        .show(ui, |ui| layer_tools(ui, scene, state, &mut response));

    egui::ScrollArea::both()
        .id_salt("timeline-grid")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // **No gaps between the rows.** egui puts its standard item spacing
            // between anything laid out one after another, which here was a
            // stripe of panel background between every layer — Animate's rows
            // touch, and the space was costing a row of layers for every three
            // shown. The cells' own outlines are what separate them.
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

            ruler(ui, columns, *scene.looping(), state, &mut response);

            // The camera sits above every layer, as it does in Animate.
            if scene.camera().enabled {
                camera_row(ui, scene, columns, state, &mut response);
            }

            let layer_ids: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
            for id in layer_ids {
                let Some(layer) = scene.layers().get(id) else {
                    continue;
                };
                layer_row(ui, layer, columns, state, &mut response);
            }
        });

    response
}

/// New Layer, New Folder and Delete, under the layer names.
///
/// The same three commands the Insert menu and the Layers panel raise — the
/// buttons are a third door onto one room, not a third implementation. Delete
/// is disabled at one layer, because a document with no layers is not a state
/// Animate lets you reach and not one worth supporting here.
fn layer_tools(ui: &mut Ui, scene: &Scene, state: &TimelineState, out: &mut TimelineResponse) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        if ui
            .small_button("➕")
            .on_hover_text(crate::Command::NewLayer.label())
            .clicked()
        {
            out.command = Some(crate::Command::NewLayer);
        }
        if ui
            .small_button("Fld")
            .on_hover_text(crate::Command::NewLayerFolder.label())
            .clicked()
        {
            out.command = Some(crate::Command::NewLayerFolder);
        }

        let can_delete = scene.layers().len() > 1 && state.active_layer.is_some();
        if ui
            .add_enabled(can_delete, egui::Button::new("🗑").small())
            .on_hover_text(crate::Command::DeleteLayer.label())
            .on_disabled_hover_text(if state.active_layer.is_some() {
                "The last layer cannot be deleted"
            } else {
                "Select a layer first"
            })
            .clicked()
        {
            out.command = Some(crate::Command::DeleteLayer);
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
            .small_button("Reset")
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

fn transport_controls(
    ui: &mut Ui,
    scene: &Scene,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    {
        if ui.button("|<").on_hover_text("Go to first frame").clicked() {
            out.go_to_start = true;
        }
        if ui.button("◀").on_hover_text("Previous frame (,)").clicked() {
            out.step -= 1;
        }
        let play_label = if state.playing { "||" } else { ">" };
        if ui
            .button(play_label)
            .on_hover_text("Play or pause (Enter)")
            .clicked()
        {
            out.toggle_play = true;
        }
        if ui.button("▶|").on_hover_text("Next frame (.)").clicked() {
            out.step += 1;
        }
        if ui.button(">|").on_hover_text("Go to last frame").clicked() {
            out.go_to_end = true;
        }

        ui.separator();

        if ui
            .selectable_label(state.onion_enabled, "Onion")
            .on_hover_text("Onion skinning")
            .clicked()
        {
            out.toggle_onion = true;
        }

        if ui
            .selectable_label(state.auto_keyframe, "Auto Key")
            .on_hover_text(
                "Auto Keyframe \u{2014} changing anything at a frame that is not a \
                 keyframe makes one first, so the change starts here rather than \
                 reaching back to the start of the span",
            )
            .clicked()
        {
            out.toggle_auto_keyframe = true;
        }

        if ui
            .selectable_label(state.edit_multiple, "Edit Multiple")
            .on_hover_text(
                "Edit Multiple Frames \u{2014} every keyframe inside the onion markers \
                 is drawn solid, can be clicked, and moves together",
            )
            .clicked()
        {
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
        for action in [
            FrameAction::InsertFrame,
            FrameAction::InsertKeyframe,
            FrameAction::InsertBlankKeyframe,
        ] {
            if ui
                .small_button(action.shortcut_text())
                .on_hover_text(action.label())
                .clicked()
            {
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

    if ui
        .selectable_label(region.enabled, "Loop")
        .on_hover_text("Repeat a section — in playback and in the export")
        .clicked()
    {
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
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 0.0, Palette::ruler_bg());

    let grid_left = rect.min.x + LAYER_COLUMN;
    let font = FontId::proportional(9.0);

    // Headings over the three switch columns, in the same place the switches
    // are: a painted eye is only obvious once you know what it is, and this is
    // the row Animate rules them under.
    for (column, icon) in switch_columns(rect)
        .iter()
        .zip(crate::panels::LayerIcon::ALL)
    {
        painter.text(
            column.center(),
            Align2::CENTER_CENTER,
            icon.heading(),
            FontId::proportional(8.0),
            Palette::ruler_text(),
        );
    }

    // Label every fifth frame, as Animate does — or every tenth, or every
    // twentieth, once the cells are too narrow for the numbers to fit beside
    // each other. A ruler whose labels overlap is worse than one with fewer.
    let step = if cell_width >= 12.0 {
        5
    } else if cell_width >= 7.0 {
        10
    } else {
        20
    };
    for frame in (0..columns).step_by(step) {
        let x = grid_left + frame as f32 * cell_width;
        painter.text(
            egui::pos2(x + 1.0, rect.min.y + 2.0),
            Align2::LEFT_TOP,
            format!("{}", frame + 1),
            font.clone(),
            Palette::ruler_text(),
        );
    }

    draw_loop_band(&painter, rect, grid_left, columns, region, cell_width);
    draw_playhead(&painter, rect, grid_left, state.current_frame, cell_width);

    // Dragging along the ruler scrubs.
    if (response.clicked() || response.dragged())
        && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
        && pos.x >= grid_left
    {
        let frame = ((pos.x - grid_left) / cell_width).floor().max(0.0) as u32;
        out.scrub_to = Some(frame.min(columns.saturating_sub(1)));
    }
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
    // the same as a section that really repeats.
    let color = if region.is_active() {
        Color32::from_rgb(0xE0, 0x9B, 0x3A)
    } else {
        Color32::from_rgb(0x7A, 0x63, 0x3C)
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

fn draw_playhead(
    painter: &egui::Painter,
    rect: egui::Rect,
    grid_left: f32,
    frame: u32,
    cell_width: f32,
) {
    let x = grid_left + frame as f32 * cell_width;
    let head = egui::Rect::from_min_size(
        egui::pos2(x, rect.min.y),
        egui::vec2(cell_width, rect.height()),
    );
    painter.rect_filled(head, 0.0, Palette::selection());
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
    let painter = ui.painter_at(rect);

    if state.camera_selected {
        painter.rect_filled(rect, 0.0, Palette::raised());
    }

    painter.text(
        egui::pos2(rect.min.x + 4.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Camera",
        FontId::proportional(11.0),
        Palette::text(),
    );

    let camera = scene.camera();
    let last = camera.last_frame();
    let grid_left = rect.min.x + LAYER_COLUMN;
    let tint = Color32::from_rgb(0x3A, 0x4A, 0x60);

    for frame in 0..columns {
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

        draw_frame_cell(&painter, cell, kind, tween, frame == state.current_frame);
    }

    if response.clicked() {
        out.select_camera = true;
        if let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
            && pos.x > grid_left
        {
            let frame = ((pos.x - grid_left) / cell_width) as u32;
            out.scrub_to = Some(frame.min(columns.saturating_sub(1)));
        }
    }
}

/// One layer's row: name on the left, frames on the right.
fn layer_row(
    ui: &mut Ui,
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
    let painter = ui.painter_at(rect);

    let active = state.active_layer == Some(layer.id);
    if active {
        painter.rect_filled(rect, 0.0, Palette::raised());
    }

    // -- name column ------------------------------------------------------
    let indent = if layer.parent.is_some() { 12.0 } else { 0.0 };
    let mark = match layer.kind {
        LayerKind::Folder => "F ",
        LayerKind::Mask => "M ",
        LayerKind::InverseMask => "iM ",
        LayerKind::Guide => "G ",
        LayerKind::Masked | LayerKind::Guided => ". ",
        LayerKind::Normal => "",
    };
    // **Clipped to the name area.** A painted string does not wrap or truncate
    // itself, and a long layer name would otherwise be drawn straight through
    // the three switches to its right.
    painter
        .with_clip_rect(name_area(rect).intersect(rect))
        .text(
            egui::pos2(rect.min.x + 4.0 + indent, rect.center().y),
            Align2::LEFT_CENTER,
            format!("{mark}{}", layer.name),
            FontId::proportional(11.0),
            if layer.visible {
                Palette::text()
            } else {
                Palette::text_dim()
            },
        );

    // **Animate's three columns, beside the name.**
    //
    // The Layers panel has had these for a while and the timeline had none of
    // them, which meant hiding a layer while working in the timeline — the
    // panel an animator is actually in — needed a trip to the other side of
    // the window. They are painted rather than laid out as widgets because the
    // whole grid is: see `paint_layer_icon`.
    let hover = ui.ctx().input(|i| i.pointer.hover_pos());
    let switches = switch_columns(rect);
    for (column, icon) in switches.iter().zip(crate::panels::LayerIcon::ALL) {
        let on = match icon {
            crate::panels::LayerIcon::Eye => layer.visible,
            crate::panels::LayerIcon::Lock => layer.locked,
            crate::panels::LayerIcon::Outline => layer.outline,
        };
        let hovered = hover.is_some_and(|p| column.contains(p) && response.hovered());
        crate::panels::paint_layer_icon(&painter, *column, icon, on, layer.color, hovered);
    }

    // The layer's colour chip, which is also its outline-view colour. Left of
    // the switches, beside the outline one it explains.
    let chip = egui::Rect::from_min_size(
        egui::pos2(
            switches[0].left() - crate::panels::CHIP_WIDTH - SWITCH_GAP,
            rect.center().y - 4.0,
        ),
        egui::vec2(7.0, 8.0),
    );
    let [r, g, b, a] = layer.color.to_rgba8().to_u8_array();
    painter.rect_filled(chip, 1.0, Color32::from_rgba_unmultiplied(r, g, b, a));

    // -- frame grid --------------------------------------------------------
    let grid_left = rect.min.x + LAYER_COLUMN;
    let length = layer.length();

    for frame in 0..columns {
        let x = grid_left + frame as f32 * cell_width;
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(cell_width, height));
        draw_frame_cell(
            &painter,
            cell,
            layer.frame_kind(frame),
            tween_cell(layer, frame, length),
            frame == state.current_frame,
        );
    }

    // A sound layer draws its waveform across the frames the sound covers.
    // This is what makes a soundtrack usable: an animator finds the accents by
    // *looking* at where the loud parts are, then keys on them.
    //
    // Drawn **over** the frame cells, translucently. Underneath it was
    // invisible: every cell paints its own background, so the envelope was
    // covered by the very frames it describes.
    if let Some(waveform) = state.waveforms.get(&layer.id) {
        draw_waveform(
            &painter, waveform, grid_left, rect, height, columns, cell_width,
        );
    }

    let clicked_frame = |ui: &Ui| -> Option<u32> {
        let pos = ui.ctx().input(|i| i.pointer.interact_pos())?;
        (pos.x >= grid_left).then(|| {
            let frame = ((pos.x - grid_left) / cell_width).floor().max(0.0) as u32;
            frame.min(columns.saturating_sub(1))
        })
    };

    if response.clicked() {
        // A switch first: clicking the eye hides the layer, it does not also
        // select it and move the playhead to frame one.
        let hit = ui
            .ctx()
            .input(|i| i.pointer.interact_pos())
            .and_then(|pos| switch_at(rect, pos));
        if let Some(icon) = hit {
            out.toggle_layer = Some((layer.id, icon));
        } else {
            out.select_layer = Some(layer.id);
            if let Some(frame) = clicked_frame(ui) {
                out.scrub_to = Some(frame);
            }
        }
    }

    // **Two right-click menus, decided by where the pointer is.**
    //
    // Animate has a frame menu on the frames and a layer menu on the name, and
    // they are different sets of commands about different things. The frame
    // menu opening over a layer's name was the only menu here, so hiding or
    // deleting a layer from the timeline had no route at all.
    let in_name_column = ui
        .ctx()
        .input(|i| i.pointer.interact_pos())
        .is_some_and(|pos| pos.x < grid_left);
    response.context_menu(|ui| {
        out.select_layer = Some(layer.id);
        if in_name_column {
            layer_context_menu(ui, layer, out);
            return;
        }
        // Opening the frame menu moves the playhead first, so the command that
        // follows lands on the frame the user pointed at rather than wherever
        // the playhead happened to be.
        if let Some(frame) = clicked_frame(ui)
            && out.scrub_to.is_none()
        {
            out.scrub_to = Some(frame);
        }
        frame_context_menu(ui, out);
    });
}

/// Animate's right-click menu on a layer's name.
///
/// The same three switches the columns carry, spelled out — nothing on a
/// fifteen-point painted square says which one hides a layer — plus the layer
/// commands, which is where Delete belongs: a destructive action does not want
/// to be a fourth small square beside three toggles.
fn layer_context_menu(ui: &mut Ui, layer: &buzz_scene::Layer, out: &mut TimelineResponse) {
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

    ui.separator();
    for command in [
        crate::Command::NewLayer,
        crate::Command::NewLayerFolder,
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
fn frame_context_menu(ui: &mut Ui, out: &mut TimelineResponse) {
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
    columns: u32,
    cell_width: f32,
) {
    let middle = row.center().y;
    let half = (height * 0.5 - 2.0).max(1.0);
    let colour = Color32::from_rgba_unmultiplied(120, 215, 255, 150);

    for (i, level) in waveform.levels.iter().enumerate() {
        let frame = waveform.start_frame + i as u32;
        if frame >= columns {
            break;
        }
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
    playhead: bool,
) {
    // Occupied frames get a lighter background than empty ones — unless a
    // tween covers them, in which case Animate tints the whole span in the
    // tween's colour, which is how you tell the three kinds apart at a glance.
    let background = match (kind, tween) {
        (FrameKind::Empty, _) => Palette::panel(),
        (_, Some(t)) => t.tint,
        (FrameKind::BlankKeyframe, None) => Palette::panel(),
        _ => Color32::from_rgb(0x44, 0x4A, 0x52),
    };
    painter.rect_filled(cell, 0.0, background);

    // **Ruled, not boxed.** A full rectangle round every cell draws each line
    // twice — once as one cell's right edge and again as its neighbour's left
    // — and at twelve pixels a cell that doubled ink was most of what the
    // timeline showed: rows looked separated by a gap they did not have, and
    // the frames themselves were hard to count. One hairline down the right
    // and one along the bottom is Animate's grid, and it is half the ink.
    let rule = Palette::border();
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

    if let Some(t) = tween {
        draw_tween_mark(painter, cell, t);
    }

    let centre = cell.center();
    let dot = 3.0;

    match kind {
        FrameKind::Empty => {}
        FrameKind::Keyframe => {
            // Filled circle: a keyframe with artwork.
            painter.circle_filled(egui::pos2(centre.x, cell.max.y - 5.0), dot, Palette::text());
        }
        FrameKind::BlankKeyframe => {
            // Hollow circle: a keyframe that deliberately shows nothing.
            painter.circle_stroke(
                egui::pos2(centre.x, cell.max.y - 5.0),
                dot,
                Stroke::new(1.0, Palette::text_dim()),
            );
        }
        FrameKind::Span => {}
        FrameKind::SpanEnd => {
            // Hollow rectangle: the last frame of a span.
            let end = egui::Rect::from_center_size(
                egui::pos2(centre.x, cell.max.y - 5.0),
                egui::vec2(5.0, 5.0),
            );
            painter.rect_stroke(
                end,
                0.0,
                Stroke::new(1.0, Palette::text_dim()),
                StrokeKind::Inside,
            );
        }
    }

    if playhead {
        painter.rect_stroke(
            cell,
            0.0,
            Stroke::new(1.0, Palette::selection()),
            StrokeKind::Inside,
        );
    }
}

/// The line an animator reads a tween by: solid with an arrowhead when the
/// tween runs between two keyframes, dashed when it has nowhere to go.
fn draw_tween_mark(painter: &egui::Painter, cell: egui::Rect, tween: TweenCell) {
    let y = cell.center().y;
    let stroke = Stroke::new(1.0, Palette::text());

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
            Palette::text(),
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
            waveforms: Default::default(),
            current_frame: 4,
            active_layer: None,
            camera_selected: false,
            playing: false,
            onion_enabled: false,
            auto_keyframe: false,
            edit_multiple: false,
            onion_before: 2,
            onion_after: 2,
            frame_width: Metrics::FRAME_WIDTH,
            row_scale: 1.0,
        }
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
            for corner in [column.min + egui::vec2(1.0, 1.0), column.max - egui::vec2(1.0, 1.0)] {
                assert_eq!(switch_at(row, corner), Some(icon), "{icon:?} misses a corner");
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
}
