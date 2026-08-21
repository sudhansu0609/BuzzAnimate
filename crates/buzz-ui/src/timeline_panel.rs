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
    /// Loudness per frame, `0.0..=1.0`.
    pub levels: std::sync::Arc<Vec<f32>>,
}

/// Width reserved for the layer-name column.
///
/// Wider than it was by the width of the three switch columns, so that adding
/// them took the room from the window rather than from the layer names.
const LAYER_COLUMN: f32 = 210.0;

/// The layer-names column background — dark, like Animate's. It is opaque so the
/// horizontally scrolling frame grid never shows through the pinned names.
const NAMES_BG: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);

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

            // The camera sits above every layer, as it does in Animate — but
            // only on the document's own timeline. See [`shows_camera_row`].
            if shows_camera_row(scene) {
                camera_row(ui, scene, columns, state, &mut response);
            }

            // The node table is rebuilt every frame the view is on: rows come
            // and go with the scroll, and a stale position would draw a
            // connector to where a layer used to be.
            if state.parenting_view {
                parenting::clear_nodes(ui.ctx());
            }

            let layer_ids: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
            for id in layer_ids {
                let Some(layer) = scene.layers().get(id) else {
                    continue;
                };
                layer_row(ui, scene, layer, columns, state, &mut response);
            }

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
                egui::Button::new("Parent")
                    .small()
                    .selected(state.parenting_view),
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
                egui::Button::new("Depth")
                    .small()
                    .selected(state.depth_view),
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

/// The transport strip's symbols, drawn like Animate's — icon buttons, not text.
#[derive(Clone, Copy)]
enum Glyph {
    First,
    Prev,
    Play,
    Pause,
    Next,
    Last,
    Onion,
    AutoKey,
    EditMultiple,
    Loop,
    InsertFrame,
    InsertKey,
    InsertBlank,
}

/// A small square icon button, highlighted blue when its mode is on.
fn icon_button(ui: &mut Ui, active: bool, tip: &str, glyph: Glyph) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::click());
    let bg = if active {
        Palette::active()
    } else if resp.hovered() {
        Palette::hover()
    } else {
        Palette::raised()
    };
    ui.painter().rect_filled(rect, 3.0, bg);
    let ink = if active { Color32::WHITE } else { Palette::text() };
    draw_glyph(ui.painter(), rect.shrink(6.0), glyph, ink);
    resp.on_hover_text(tip).clicked()
}

fn draw_glyph(p: &egui::Painter, r: egui::Rect, g: Glyph, c: Color32) {
    let s = Stroke::new(1.5, c);
    let at = |x: f32, y: f32| egui::pos2(r.min.x + r.width() * x, r.min.y + r.height() * y);
    let tri = |a: egui::Pos2, b: egui::Pos2, d: egui::Pos2| {
        p.add(egui::Shape::convex_polygon(vec![a, b, d], c, Stroke::NONE));
    };
    let bar = |x0: f32, x1: f32| {
        p.rect_filled(egui::Rect::from_min_max(at(x0, 0.08), at(x1, 0.92)), 0.0, c);
    };
    match g {
        Glyph::Play => tri(at(0.15, 0.08), at(0.15, 0.92), at(0.9, 0.5)),
        Glyph::Pause => {
            bar(0.22, 0.4);
            bar(0.6, 0.78);
        }
        Glyph::First => {
            bar(0.12, 0.24);
            tri(at(0.92, 0.1), at(0.92, 0.9), at(0.34, 0.5));
        }
        Glyph::Last => {
            bar(0.76, 0.88);
            tri(at(0.08, 0.1), at(0.08, 0.9), at(0.66, 0.5));
        }
        Glyph::Prev => tri(at(0.82, 0.1), at(0.82, 0.9), at(0.22, 0.5)),
        Glyph::Next => tri(at(0.18, 0.1), at(0.18, 0.9), at(0.78, 0.5)),
        // Two overlapping frames — the onion.
        Glyph::Onion => {
            p.rect_stroke(
                egui::Rect::from_min_max(at(0.1, 0.2), at(0.6, 0.8)),
                1.0,
                s,
                StrokeKind::Inside,
            );
            p.rect_stroke(
                egui::Rect::from_min_max(at(0.4, 0.2), at(0.9, 0.8)),
                1.0,
                s,
                StrokeKind::Inside,
            );
        }
        // A key, as a filled diamond.
        Glyph::AutoKey => {
            tri(at(0.5, 0.1), at(0.12, 0.5), at(0.5, 0.9));
            tri(at(0.5, 0.1), at(0.88, 0.5), at(0.5, 0.9));
        }
        // Several frames at once — three columns.
        Glyph::EditMultiple => {
            for x in [0.16, 0.44, 0.72] {
                p.rect_filled(egui::Rect::from_min_max(at(x, 0.15), at(x + 0.12, 0.85)), 0.0, c);
            }
        }
        // A loop — a ring with an arrowhead.
        Glyph::Loop => {
            p.circle_stroke(r.center(), r.width() * 0.3, s);
            tri(at(0.62, 0.02), at(0.98, 0.14), at(0.66, 0.32));
        }
        // Insert Frame / Keyframe / Blank Keyframe — a frame box, a filled dot,
        // a hollow dot: Animate's own marks for the three.
        Glyph::InsertFrame => {
            p.rect_stroke(
                egui::Rect::from_min_max(at(0.2, 0.18), at(0.8, 0.82)),
                0.0,
                s,
                StrokeKind::Inside,
            );
        }
        Glyph::InsertKey => {
            p.circle_filled(r.center(), r.width() * 0.24, c);
        }
        Glyph::InsertBlank => {
            p.circle_stroke(r.center(), r.width() * 0.24, s);
        }
    }
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
        if icon_button(ui, false, "Previous frame (,)", Glyph::Prev) {
            out.step -= 1;
        }
        let play = if state.playing { Glyph::Pause } else { Glyph::Play };
        if icon_button(ui, state.playing, "Play or pause (Enter)", play) {
            out.toggle_play = true;
        }
        if icon_button(ui, false, "Next frame (.)", Glyph::Next) {
            out.step += 1;
        }
        if icon_button(ui, false, "Go to last frame", Glyph::Last) {
            out.go_to_end = true;
        }

        ui.separator();

        if icon_button(ui, state.onion_enabled, "Onion skinning", Glyph::Onion) {
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
        for (action, glyph) in [
            (FrameAction::InsertFrame, Glyph::InsertFrame),
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

    draw_loop_band(&painter, rect, grid_left, columns, region, cell_width);
    draw_playhead(&painter, rect, grid_left, state.current_frame, cell_width);

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
        head.text(
            column.center(),
            Align2::CENTER_CENTER,
            icon.heading(),
            FontId::proportional(8.0),
            if hovered {
                Palette::text()
            } else {
                Palette::ruler_text()
            },
        );
        if response.clicked()
            && ui
                .ctx()
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| column.contains(p))
        {
            out.toggle_all = Some(icon);
        }
    }

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
    painter.rect_filled(head, 0.0, PLAYHEAD);
}

/// Animate's playhead blue, sampled from the reference.
const PLAYHEAD: Color32 = Color32::from_rgb(0x36, 0x79, 0xC1);

/// Whether the camera row belongs on the timeline right now.
///
/// It films the whole document, so it appears only on the document's own
/// timeline and only once the camera is switched on. Inside a symbol the
/// timeline shows that symbol's layers; a camera row there would suggest each
/// symbol carries a camera of its own, which it does not — the film has one.
fn shows_camera_row(scene: &Scene) -> bool {
    scene.camera().enabled && scene.edit_path().is_empty()
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
    let tint = Color32::from_rgb(0x3A, 0x4A, 0x60);

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

        draw_frame_cell(&painter, cell, kind, tween, frame == state.current_frame);
    }

    // The pinned "Camera" name cell, over the grid.
    let names = ui.painter_at(name_row.intersect(ui.clip_rect()));
    names.rect_filled(
        name_row,
        0.0,
        if state.camera_selected {
            Color32::from_rgb(0x35, 0x61, 0x91)
        } else {
            NAMES_BG
        },
    );
    names.text(
        egui::pos2(name_row.min.x + 4.0, name_row.center().y),
        Align2::LEFT_CENTER,
        "Camera",
        FontId::proportional(11.0),
        Palette::text(),
    );

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
/// The range of frame columns that fall inside the visible viewport.
///
/// The timeline grid is virtualized: a row still *allocates* its full width — so
/// clicking, context menus and the scroll extent are exactly as before — but it
/// only *paints* the cells the user can actually see. On a document with
/// thousands of frames and hundreds of layers that is the difference between a
/// few thousand shapes a frame and several million. `clip` is the visible
/// rectangle (`ui.clip_rect()`); it is padded by a column on each side so a cell
/// half-scrolled off an edge is not missing.
fn visible_columns(clip: egui::Rect, grid_left: f32, cell_width: f32, columns: u32) -> std::ops::Range<u32> {
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

    // -- frame grid, clipped to the area right of the pinned column -------
    let grid = ui.painter_at(
        egui::Rect::from_min_max(egui::pos2(pinned_left + LAYER_COLUMN, rect.min.y), rect.max)
            .intersect(ui.clip_rect()),
    );
    let visible = visible_columns(ui.clip_rect(), grid_left, cell_width, columns);
    for frame in visible.clone() {
        let x = grid_left + frame as f32 * cell_width;
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(cell_width, height));
        draw_frame_cell(
            &grid,
            cell,
            layer.frame_kind(frame),
            tween_cell(layer, frame, length),
            frame == state.current_frame,
        );
    }
    // A sound layer draws its waveform across the frames the sound covers, over
    // the cells and translucently, so an animator finds the accents by looking.
    if let Some(waveform) = state.waveforms.get(&layer.id) {
        draw_waveform(&grid, waveform, grid_left, rect, height, &visible, cell_width);
    }

    // -- name column, pinned and drawn over the grid ----------------------
    let painter = ui.painter_at(name_row.intersect(ui.clip_rect()));
    let active = state.active_layer == Some(layer.id);
    if active {
        // Animate marks the selected layer with a blue name panel and its own
        // outline colour as a line along the bottom of the row.
        painter.rect_filled(name_row, 0.0, Color32::from_rgb(0x35, 0x61, 0x91));
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
        painter.rect_filled(name_row, 0.0, NAMES_BG);
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
        let indent = if layer.parent.is_some() { 12.0 } else { 0.0 };
        let mark = match layer.kind {
            LayerKind::Folder => "F ",
            LayerKind::Mask => "M ",
            LayerKind::InverseMask => "iM ",
            LayerKind::Guide => "G ",
            LayerKind::Masked | LayerKind::Guided => ". ",
            LayerKind::Normal => "",
        };
        // **Clipped to the name area.** A painted string does not wrap or
        // truncate itself, and a long layer name would otherwise be drawn
        // straight through the three switches to its right.
        painter
            .with_clip_rect(name_area(name_row).intersect(name_row))
            .text(
                egui::pos2(name_row.min.x + 4.0 + indent, name_row.center().y),
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

    // The grid begins to the right of the pinned column, not at its scrolled
    // origin, so a click on the pinned names is never read as a frame click.
    let grid_edge = pinned_left + LAYER_COLUMN;
    let clicked_frame = |ui: &Ui| -> Option<u32> {
        let pos = ui.ctx().input(|i| i.pointer.interact_pos())?;
        (pos.x >= grid_edge).then(|| {
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
            .and_then(|pos| switch_at(name_row, pos));
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
        .is_some_and(|pos| pos.x < grid_edge);
    response.context_menu(|ui| {
        out.select_layer = Some(layer.id);
        if in_name_column {
            layer_context_menu(ui, scene, layer, out);
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
    let target = dragging.is_some_and(|from| {
        from != layer.id && scene.layers().can_follow(from, layer.id)
    });
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
        egui::Stroke::new(if source || (target && hovered) { 2.0 } else { 1.0 }, ring),
    );

    // The line the pointer is dragging, drawn from the node it started on.
    if source && let Some(pointer) = ui.ctx().input(|i| i.pointer.interact_pos()) {
        painter.line_segment(
            [centre, pointer],
            egui::Stroke::new(1.5, Palette::active()),
        );
    }

    // **The drop.** Read on the *source* node's response, because that is the
    // widget egui gives the drag to for its whole life — the node under the
    // pointer at the end never sees a release of its own.
    if response.drag_stopped() {
        let dropped_on = ui
            .ctx()
            .input(|i| i.pointer.interact_pos())
            .and_then(|p| {
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
    let parent_name = layer.follows.and_then(|id| {
        scene
            .layers()
            .get(id)
            .map(|l| l.name.clone())
    });
    response.context_menu(|ui| {
        match &parent_name {
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
    visible: &std::ops::Range<u32>,
    cell_width: f32,
) {
    let middle = row.center().y;
    let half = (height * 0.5 - 2.0).max(1.0);
    let colour = Color32::from_rgba_unmultiplied(120, 215, 255, 150);

    for (i, level) in waveform.levels.iter().enumerate() {
        let frame = waveform.start_frame + i as u32;
        // Only the bars in view — the strip is virtualized with the cells.
        if frame >= visible.end {
            break;
        }
        if frame < visible.start {
            continue;
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
    // Animate's frame grid: an occupied frame is a light grey, an empty one the
    // dark panel; a tween tints its whole span so the three kinds read apart at
    // a glance. Sampled from Animate: occupied #909090, empty #252525.
    let occupied = Color32::from_rgb(0x90, 0x90, 0x90);
    let empty = Color32::from_rgb(0x25, 0x25, 0x25);
    let background = match (kind, tween) {
        (FrameKind::Empty, _) => empty,
        (_, Some(t)) => t.tint,
        // A blank keyframe still holds its span open, so its cell is occupied.
        _ => occupied,
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
        Color32::from_rgb(0x33, 0x33, 0x33)
    } else {
        Color32::from_rgb(0x7C, 0x7C, 0x7C)
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

    if let Some(t) = tween {
        draw_tween_mark(painter, cell, t);
    }

    let centre = cell.center();
    let dot = 3.0;
    // On Animate's light grid the marks are near-black.
    let mark = Color32::from_rgb(0x1E, 0x1E, 0x1E);

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
        // wash of the playhead blue, not just an outline.
        painter.rect_filled(
            cell,
            0.0,
            Color32::from_rgba_unmultiplied(0x36, 0x79, 0xC1, 0x40),
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
        assert_eq!(visible_columns(far_left, grid_left, cell_width, columns).start, 0);
        let far_right =
            egui::Rect::from_min_max(egui::pos2(1e9, 0.0), egui::pos2(1e9 + 100.0, 1.0));
        let r = visible_columns(far_right, grid_left, cell_width, columns);
        assert!(r.start <= columns && r.end <= columns);
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
            parenting_view: false,
            depth_view: false,
            focal_distance: 1000.0,
            nearest_depth: -900.0,
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
