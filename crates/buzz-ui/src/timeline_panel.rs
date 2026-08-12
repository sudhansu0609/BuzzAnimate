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

use buzz_scene::{FrameKind, LayerId, LayerKind, Scene};
use egui::{Align2, Color32, FontId, Sense, Stroke, StrokeKind, Ui};

use crate::theme::{Metrics, Palette};

/// What the user did in the timeline.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TimelineResponse {
    /// The playhead was moved here.
    pub scrub_to: Option<u32>,
    /// A layer row was clicked.
    pub select_layer: Option<LayerId>,
    /// A frame operation was requested.
    pub action: Option<FrameAction>,
    /// A tween was created or removed from the frame menu.
    pub tween: Option<TweenRequest>,
    pub toggle_play: bool,
    pub toggle_onion: bool,
    pub go_to_start: bool,
    pub go_to_end: bool,
    pub step: i64,
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
    pub playing: bool,
    pub onion_enabled: bool,
}

/// Width reserved for the layer-name column.
const LAYER_COLUMN: f32 = 190.0;

/// Draw the timeline.
pub fn timeline_panel(ui: &mut Ui, scene: &Scene, state: &TimelineState) -> TimelineResponse {
    let mut response = TimelineResponse::default();

    transport(ui, scene, state, &mut response);
    ui.separator();

    let frame_count = scene.frame_count().max(1);
    // Always offer some empty frames past the end so the span can be extended
    // by clicking, as in Animate.
    let columns = (frame_count + 40).min(9_999);

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ruler(ui, columns, state, &mut response);

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

/// Playback controls and readouts.
fn transport(ui: &mut Ui, scene: &Scene, state: &TimelineState, out: &mut TimelineResponse) {
    ui.horizontal(|ui| {
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

        ui.separator();

        let fps = scene.stage().frame_rate.max(0.01);
        let elapsed = state.current_frame as f64 / fps;
        ui.label(format!("Frame {}", state.current_frame + 1));
        ui.label(
            egui::RichText::new(format!(
                "{:.0} fps · {elapsed:.2} s of {:.2} s",
                fps,
                scene.duration_seconds()
            ))
            .small()
            .weak(),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for action in [
                FrameAction::InsertKeyframe,
                FrameAction::InsertBlankKeyframe,
                FrameAction::InsertFrame,
            ] {
                if ui
                    .small_button(action.shortcut_text())
                    .on_hover_text(action.label())
                    .clicked()
                {
                    out.action = Some(action);
                }
            }
        });
    });
}

/// The frame-number strip, which also scrubs.
fn ruler(ui: &mut Ui, columns: u32, state: &TimelineState, out: &mut TimelineResponse) {
    let width = columns as f32 * Metrics::FRAME_WIDTH;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LAYER_COLUMN + width, 16.0),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 0.0, Palette::RULER_BG);

    let grid_left = rect.min.x + LAYER_COLUMN;
    let font = FontId::proportional(9.0);

    // Label every fifth frame, as Animate does.
    for frame in (0..columns).step_by(5) {
        let x = grid_left + frame as f32 * Metrics::FRAME_WIDTH;
        painter.text(
            egui::pos2(x + 1.0, rect.min.y + 2.0),
            Align2::LEFT_TOP,
            format!("{}", frame + 1),
            font.clone(),
            Palette::RULER_TEXT,
        );
    }

    draw_playhead(&painter, rect, grid_left, state.current_frame);

    // Dragging along the ruler scrubs.
    if (response.clicked() || response.dragged())
        && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
        && pos.x >= grid_left
    {
        let frame = ((pos.x - grid_left) / Metrics::FRAME_WIDTH).floor().max(0.0) as u32;
        out.scrub_to = Some(frame.min(columns.saturating_sub(1)));
    }
}

fn draw_playhead(painter: &egui::Painter, rect: egui::Rect, grid_left: f32, frame: u32) {
    let x = grid_left + frame as f32 * Metrics::FRAME_WIDTH;
    let head = egui::Rect::from_min_size(
        egui::pos2(x, rect.min.y),
        egui::vec2(Metrics::FRAME_WIDTH, rect.height()),
    );
    painter.rect_filled(head, 0.0, Palette::SELECTION);
}

/// One layer's row: name on the left, frames on the right.
fn layer_row(
    ui: &mut Ui,
    layer: &buzz_scene::Layer,
    columns: u32,
    state: &TimelineState,
    out: &mut TimelineResponse,
) {
    let height = Metrics::LAYER_ROW * (layer.height.percent() as f32 / 100.0);
    let width = columns as f32 * Metrics::FRAME_WIDTH;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LAYER_COLUMN + width, height),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);

    let active = state.active_layer == Some(layer.id);
    if active {
        painter.rect_filled(rect, 0.0, Palette::RAISED);
    }

    // -- name column ------------------------------------------------------
    let indent = if layer.parent.is_some() { 12.0 } else { 0.0 };
    let mark = match layer.kind {
        LayerKind::Folder => "F ",
        LayerKind::Mask => "M ",
        LayerKind::Guide => "G ",
        LayerKind::Masked | LayerKind::Guided => ". ",
        LayerKind::Normal => "",
    };
    painter.text(
        egui::pos2(rect.min.x + 4.0 + indent, rect.center().y),
        Align2::LEFT_CENTER,
        format!("{mark}{}", layer.name),
        FontId::proportional(11.0),
        if layer.visible {
            Palette::TEXT
        } else {
            Palette::TEXT_DIM
        },
    );

    // The layer's colour chip, which is also its outline-view colour.
    let chip = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - width - 12.0, rect.center().y - 4.0),
        egui::vec2(7.0, 8.0),
    );
    let [r, g, b, a] = layer.color.to_rgba8().to_u8_array();
    painter.rect_filled(chip, 1.0, Color32::from_rgba_unmultiplied(r, g, b, a));

    // -- frame grid --------------------------------------------------------
    let grid_left = rect.min.x + LAYER_COLUMN;
    let length = layer.length();
    for frame in 0..columns {
        let x = grid_left + frame as f32 * Metrics::FRAME_WIDTH;
        let cell = egui::Rect::from_min_size(
            egui::pos2(x, rect.min.y),
            egui::vec2(Metrics::FRAME_WIDTH, height),
        );
        draw_frame_cell(
            &painter,
            cell,
            layer.frame_kind(frame),
            tween_cell(layer, frame, length),
            frame == state.current_frame,
        );
    }

    let clicked_frame = |ui: &Ui| -> Option<u32> {
        let pos = ui.ctx().input(|i| i.pointer.interact_pos())?;
        (pos.x >= grid_left).then(|| {
            let frame = ((pos.x - grid_left) / Metrics::FRAME_WIDTH).floor().max(0.0) as u32;
            frame.min(columns.saturating_sub(1))
        })
    };

    if response.clicked() {
        out.select_layer = Some(layer.id);
        if let Some(frame) = clicked_frame(ui) {
            out.scrub_to = Some(frame);
        }
    }

    // Animate's right-click frame menu. Opening it moves the playhead first,
    // so the command that follows lands on the frame the user pointed at
    // rather than wherever the playhead happened to be.
    response.context_menu(|ui| {
        out.select_layer = Some(layer.id);
        if let Some(frame) = clicked_frame(ui)
            && out.scrub_to.is_none()
        {
            out.scrub_to = Some(frame);
        }
        frame_context_menu(ui, out);
    });
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

/// Draw one frame cell using Animate's conventions.
fn draw_frame_cell(
    painter: &egui::Painter,
    cell: egui::Rect,
    kind: FrameKind,
    tween: Option<TweenCell>,
    playhead: bool,
) {
    let grid = Stroke::new(1.0, Palette::BORDER);

    // Occupied frames get a lighter background than empty ones — unless a
    // tween covers them, in which case Animate tints the whole span in the
    // tween's colour, which is how you tell the three kinds apart at a glance.
    let background = match (kind, tween) {
        (FrameKind::Empty, _) => Palette::PANEL,
        (_, Some(t)) => t.tint,
        (FrameKind::BlankKeyframe, None) => Palette::PANEL,
        _ => Color32::from_rgb(0x44, 0x4A, 0x52),
    };
    painter.rect_filled(cell, 0.0, background);
    painter.rect_stroke(cell, 0.0, grid, StrokeKind::Inside);

    if let Some(t) = tween {
        draw_tween_mark(painter, cell, t);
    }

    let centre = cell.center();
    let dot = 3.0;

    match kind {
        FrameKind::Empty => {}
        FrameKind::Keyframe => {
            // Filled circle: a keyframe with artwork.
            painter.circle_filled(egui::pos2(centre.x, cell.max.y - 5.0), dot, Palette::TEXT);
        }
        FrameKind::BlankKeyframe => {
            // Hollow circle: a keyframe that deliberately shows nothing.
            painter.circle_stroke(
                egui::pos2(centre.x, cell.max.y - 5.0),
                dot,
                Stroke::new(1.0, Palette::TEXT_DIM),
            );
        }
        FrameKind::Span => {}
        FrameKind::SpanEnd => {
            // Hollow rectangle: the last frame of a span.
            let end = egui::Rect::from_center_size(
                egui::pos2(centre.x, cell.max.y - 5.0),
                egui::vec2(5.0, 5.0),
            );
            painter.rect_stroke(end, 0.0, Stroke::new(1.0, Palette::TEXT_DIM), StrokeKind::Inside);
        }
    }

    if playhead {
        painter.rect_stroke(
            cell,
            0.0,
            Stroke::new(1.0, Palette::SELECTION),
            StrokeKind::Inside,
        );
    }
}

/// The line an animator reads a tween by: solid with an arrowhead when the
/// tween runs between two keyframes, dashed when it has nowhere to go.
fn draw_tween_mark(painter: &egui::Painter, cell: egui::Rect, tween: TweenCell) {
    let y = cell.center().y;
    let stroke = Stroke::new(1.0, Palette::TEXT);

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
            painter.line_segment(
                [egui::pos2(x0, y), egui::pos2(x0 + quarter, y)],
                stroke,
            );
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
            Palette::TEXT,
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
            current_frame: 4,
            active_layer: None,
            playing: false,
            onion_enabled: false,
        }
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
        assert!((scene.duration_seconds() - 1.0).abs() < 0.001, "24 frames at 24 fps");
    }
}
