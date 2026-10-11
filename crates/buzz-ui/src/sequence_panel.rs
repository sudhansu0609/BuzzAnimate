//! **The Sequence strip.** See `docs/SEQUENCE_SETUPS_PLAN.md` "The Sequence
//! strip": the master audio's waveform across the whole film, with the shots
//! as coloured blocks on it, docked at the bottom beside the Timeline.
//!
//! Pure UI, like every other panel here: [`sequence_panel`] takes a
//! [`SequenceView`] built by the editor and returns a [`SequenceResponse`]
//! naming what the user did. Nothing here reaches into `buzz_doc::Document`
//! directly, so the drag arithmetic, the click picking and the colour
//! assignment can all be tested without a document at all.

use buzz_doc::SceneKey;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui};

use crate::theme::Palette;
use crate::timeline_panel::Waveform;

/// Height of the film-timecode ruler across the top of the strip.
const RULER_H: f32 = 18.0;
/// Height of the row of shot blocks under the ruler.
const BLOCK_H: f32 = 44.0;
/// How wide a boundary's drag handle is, either side of the cut.
const HANDLE_W: f32 = 6.0;

/// One shot's block on the strip — everything the panel needs to draw it and
/// to answer a click, gathered so the panel never reaches back into the
/// document.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotBlock {
    /// The shot scene this block stands for — the take currently in the cut.
    pub key: SceneKey,
    /// First film-audio frame this shot covers.
    pub start: u32,
    /// How many frames it plays for (`audio_out - audio_in`).
    pub length: u32,
    /// The setup this shot was stamped from, for the block's colour. `None`
    /// if it was never stamped from one, or that setup is gone.
    pub setup: Option<SceneKey>,
    /// The setup's name, for the block's label. Empty if `setup` is `None`.
    pub setup_name: String,
    /// This shot's ordinal among shots stamped from the same setup, for the
    /// label — "Village \u{b7} Shot 3".
    pub shot_number: usize,
    /// Every take of this shot, in take order (this block's own `key` is one
    /// of them) — what the takes switcher lists, shown only when there is
    /// more than one.
    pub takes: Vec<SceneKey>,
    /// Shots earlier in the sequence than this one, stamped from the same
    /// setup, by key and name — what this block's "Continue from Shot"
    /// context menu item lists. Empty if this shot has no setup on record,
    /// or is the first shot of its setup in the cut.
    pub continue_from_choices: Vec<(SceneKey, String)>,
}

/// What [`sequence_panel`] needs to draw itself. Built fresh by the editor
/// every frame from `Document::sequence` and the shots it names.
pub struct SequenceView {
    /// Every shot in the cut, in film order.
    pub blocks: Vec<ShotBlock>,
    /// The master audio's own waveform, if one is set — drawn behind the
    /// blocks the same way the Timeline draws a sound layer's.
    pub master_waveform: Option<Waveform>,
    /// How long the whole film is, in master-audio frames — what the strip
    /// is laid out against. At least `1`, so an empty sequence still draws a
    /// ruler rather than dividing by zero.
    pub master_frames: u32,
    /// The frame rate `master_frames` is counted in, for the ruler's
    /// timecode.
    pub fps: f64,
    /// The film playhead, in master-audio frames.
    pub playhead: u32,
    /// The shot scene currently open for editing, if any — outlined.
    pub active_shot: Option<SceneKey>,
    /// Every Setup scene, by key and name, for "Try Another Setup" to list.
    pub setups: Vec<(SceneKey, String)>,
}

/// What the user did on the strip. See the field docs for what each maps to
/// in `buzz_doc::Document`; the caller (the editor) is the one that actually
/// calls them; this panel only ever reports.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SequenceResponse {
    /// A block was clicked: select it, and switch to editing it.
    pub select: Option<SceneKey>,
    /// A block was double-clicked: open it for editing (the same as
    /// `select` acts on, named separately so a caller can tell which
    /// gesture asked).
    pub open: Option<SceneKey>,
    /// The ruler was clicked: move the film playhead here.
    pub scrub_to: Option<u32>,
    /// A boundary between two adjacent shots was dragged to a new film
    /// frame: `(left, right, at)` for `Document::move_cut`.
    pub move_cut: Option<(SceneKey, SceneKey, u32)>,
    /// The takes switcher picked a different take: `(currently active,
    /// chosen take)` for `Document::set_active_take`.
    pub switch_take: Option<(SceneKey, SceneKey)>,
    /// "Duplicate Shot" was chosen from a block's context menu.
    pub duplicate_shot: Option<SceneKey>,
    /// "Try Another Setup \u{25b8} <setup>" was chosen: `(shot, new setup)`.
    pub try_another_setup: Option<(SceneKey, SceneKey)>,
    /// "Add Missing Setup Layers" was chosen from a block's context menu.
    pub add_missing_setup_layers: Option<SceneKey>,
    /// "Edit Setup" was chosen: the setup scene to switch to.
    pub edit_setup: Option<SceneKey>,
    /// "Continue from Shot \u{25b8} <shot>" was chosen: `(this shot, the
    /// earlier shot to seed frame 0 from)`.
    pub continue_from: Option<(SceneKey, SceneKey)>,
    /// `S` was pressed while the strip had the pointer's attention — split
    /// the shot under the film playhead in two.
    pub split_at_playhead: bool,
}

/// A stable, readable colour for a setup, from its key alone — so the same
/// setup always draws the same colour, block after block, without the panel
/// keeping a colour table of its own.
fn setup_colour(setup: Option<SceneKey>) -> Color32 {
    let Some(key) = setup else {
        // No setup on record: a plain neutral grey, which reads as "nothing
        // assigned yet" beside the coloured blocks around it.
        return Color32::from_rgb(120, 120, 120);
    };
    // A small, fixed palette of readable hues, picked by the key rather than
    // hashed to an arbitrary one — few enough setups in a real film that a
    // cycled palette is more legible than a hash's occasional near-duplicate.
    const HUES: [Color32; 8] = [
        Color32::from_rgb(210, 110, 90),
        Color32::from_rgb(100, 160, 210),
        Color32::from_rgb(140, 180, 100),
        Color32::from_rgb(200, 160, 80),
        Color32::from_rgb(170, 120, 190),
        Color32::from_rgb(90, 180, 170),
        Color32::from_rgb(200, 120, 150),
        Color32::from_rgb(150, 150, 90),
    ];
    HUES[key.0 as usize % HUES.len()]
}

/// Format a film-frame position as `m:ss`, for the ruler.
fn timecode(frame: u32, fps: f64) -> String {
    let fps = fps.max(1.0);
    let seconds = (frame as f64 / fps).floor() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Draw the strip and collect the response.
pub fn sequence_panel(ui: &mut Ui, view: &SequenceView) -> SequenceResponse {
    let mut out = SequenceResponse::default();
    let total = view.master_frames.max(1);

    let width = ui.available_width().max(1.0);
    let desired = egui::vec2(width, RULER_H + BLOCK_H);
    // `Sense::hover()` only: this reserves the layout space and gives a
    // response to test "does the strip have the pointer's attention" for the
    // `S` shortcut, without competing with the ruler's and the blocks' own
    // `interact` calls below for clicks.
    let (rect, strip_response) = ui.allocate_exact_size(desired, Sense::hover());
    // Cloned rather than borrowed: `egui::Painter` is a cheap handle onto the
    // context, and the takes switcher below needs `ui` mutably while this is
    // still in scope.
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 0.0, Palette::panel());

    let px_per_frame = width / total as f32;
    let frame_x = |frame: u32| rect.min.x + frame as f32 * px_per_frame;

    // -- the ruler -----------------------------------------------------------
    let ruler_rect = Rect::from_min_size(rect.min, egui::vec2(width, RULER_H));
    painter.rect_filled(ruler_rect, 0.0, Palette::ruler_bg());
    // One tick roughly every 80 px, snapped to a whole number of seconds.
    let fps = view.fps.max(1.0);
    let seconds_per_tick =
        ((80.0 / px_per_frame.max(0.001) as f64 / fps).ceil() as u32).max(1);
    let frames_per_tick = (seconds_per_tick as f64 * fps).round().max(1.0) as u32;
    let mut tick = 0u32;
    while tick <= total {
        let x = frame_x(tick);
        painter.line_segment(
            [egui::pos2(x, ruler_rect.max.y - 6.0), egui::pos2(x, ruler_rect.max.y)],
            Stroke::new(1.0, Palette::ruler_tick()),
        );
        painter.text(
            egui::pos2(x + 2.0, ruler_rect.min.y),
            Align2::LEFT_TOP,
            timecode(tick, fps),
            FontId::proportional(9.0),
            Palette::ruler_text(),
        );
        tick = tick.saturating_add(frames_per_tick);
    }
    let ruler_response = ui.interact(ruler_rect, ui.id().with("sequence-ruler"), Sense::click());
    if ruler_response.clicked()
        && let Some(pos) = ruler_response.interact_pointer_pos()
    {
        let frame = (((pos.x - rect.min.x) / px_per_frame).round() as i64)
            .clamp(0, total as i64) as u32;
        out.scrub_to = Some(frame);
    }

    // -- the master waveform, behind the blocks ------------------------------
    let blocks_top = rect.min.y + RULER_H;
    let block_rect_full = Rect::from_min_max(
        egui::pos2(rect.min.x, blocks_top),
        egui::pos2(rect.max.x, rect.max.y),
    );
    if let Some(wave) = &view.master_waveform {
        draw_master_waveform(&painter, wave, block_rect_full, px_per_frame, total);
    }

    // -- the shot blocks -------------------------------------------------------
    for block in &view.blocks {
        let x0 = frame_x(block.start);
        let x1 = frame_x(block.start.saturating_add(block.length));
        let block_rect = Rect::from_min_max(egui::pos2(x0, blocks_top), egui::pos2(x1, rect.max.y));

        let colour = setup_colour(block.setup);
        painter.rect_filled(block_rect, 2.0, Color32::from_rgba_unmultiplied(
            colour.r(), colour.g(), colour.b(), 170,
        ));
        if view.active_shot == Some(block.key) {
            painter.rect_stroke(
                block_rect,
                2.0,
                Stroke::new(2.0, Palette::accent()),
                StrokeKind::Inside,
            );
        }

        let label = if block.setup_name.is_empty() {
            format!("Shot {}", block.shot_number)
        } else {
            format!("{} \u{b7} Shot {}", block.setup_name, block.shot_number)
        };
        painter.text(
            block_rect.left_top() + egui::vec2(4.0, 3.0),
            Align2::LEFT_TOP,
            label,
            FontId::proportional(11.0),
            Palette::text(),
        );

        let id = ui.id().with(("sequence-block", block.key.0));
        let block_response = ui.interact(block_rect, id, Sense::click());
        if block_response.double_clicked() {
            out.open = Some(block.key);
        } else if block_response.clicked() {
            out.select = Some(block.key);
        }
        block_response.context_menu(|ui| {
            if ui.button("Duplicate Shot").clicked() {
                out.duplicate_shot = Some(block.key);
                ui.close();
            }
            ui.menu_button("Try Another Setup", |ui| {
                if view.setups.is_empty() {
                    ui.label("No setups yet");
                }
                for (setup_key, name) in &view.setups {
                    if ui.button(name).clicked() {
                        out.try_another_setup = Some((block.key, *setup_key));
                        ui.close();
                    }
                }
            });
            if ui.button("Add Missing Setup Layers").clicked() {
                out.add_missing_setup_layers = Some(block.key);
                ui.close();
            }
            if ui
                .add_enabled(block.setup.is_some(), egui::Button::new("Edit Setup"))
                .clicked()
                && let Some(setup) = block.setup
            {
                out.edit_setup = Some(setup);
                ui.close();
            }
            ui.menu_button("Continue from Shot", |ui| {
                if block.continue_from_choices.is_empty() {
                    ui.label("No earlier shot of this setup");
                }
                for (source, name) in &block.continue_from_choices {
                    if ui.button(name).clicked() {
                        out.continue_from = Some((block.key, *source));
                        ui.close();
                    }
                }
            });
        });

        // The takes switcher: only shown when there is more than one take of
        // this shot to switch between.
        if block.takes.len() > 1 {
            let corner = Rect::from_min_size(
                egui::pos2(x1 - 22.0, blocks_top + 2.0),
                egui::vec2(18.0, 16.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(corner), |ui| {
                let current = block
                    .takes
                    .iter()
                    .position(|k| *k == block.key)
                    .map(|i| i + 1)
                    .unwrap_or(1);
                ui.menu_button(format!("T{current}"), |ui| {
                    for (i, take) in block.takes.iter().enumerate() {
                        if ui
                            .selectable_label(*take == block.key, format!("Take {}", i + 1))
                            .clicked()
                        {
                            out.switch_take = Some((block.key, *take));
                            ui.close();
                        }
                    }
                });
            });
        }
    }

    // -- boundary drag handles, between adjacent blocks in film order -------
    for pair in view.blocks.windows(2) {
        let (left, right) = (&pair[0], &pair[1]);
        let boundary = right.start;
        let x = frame_x(boundary);
        let handle_rect = Rect::from_min_max(
            egui::pos2(x - HANDLE_W * 0.5, blocks_top),
            egui::pos2(x + HANDLE_W * 0.5, rect.max.y),
        );
        let id = ui.id().with(("sequence-cut", left.key.0, right.key.0));
        let handle_response = ui.interact(handle_rect, id, Sense::drag());
        if handle_response.hovered() || handle_response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if handle_response.dragged()
            && let Some(pos) = handle_response.interact_pointer_pos()
        {
            let min_at = left.start.saturating_add(1);
            let max_at = right.start.saturating_add(right.length).saturating_sub(1);
            let raw = ((pos.x - rect.min.x) / px_per_frame).round() as i64;
            let frame = raw.clamp(min_at as i64, max_at.max(min_at) as i64) as u32;
            out.move_cut = Some((left.key, right.key, frame));
        }
    }

    // -- the film playhead ----------------------------------------------------
    let px = frame_x(view.playhead);
    painter.line_segment(
        [egui::pos2(px, rect.min.y), egui::pos2(px, rect.max.y)],
        Stroke::new(1.0, Palette::accent()),
    );

    // -- Split at Playhead (S), while the strip has the pointer's attention --
    if strip_response.hovered() && ui.input(|i| i.key_pressed(egui::Key::S)) {
        out.split_at_playhead = true;
    }

    out
}

/// The master audio's waveform, drawn across the whole strip the same way
/// the Timeline draws a sound layer's — see `timeline_panel`'s own
/// `draw_waveform`, which this deliberately mirrors rather than calls: that
/// one is laid out one cell per animation frame, this one is laid out at
/// `px_per_frame`, which is usually well under a pixel a frame across a
/// whole film.
fn draw_master_waveform(
    painter: &egui::Painter,
    wave: &Waveform,
    area: Rect,
    px_per_frame: f32,
    total: u32,
) {
    let middle = area.center().y;
    let half = (area.height() * 0.5 - 2.0).max(1.0);
    let colour = Color32::from_rgba_unmultiplied(120, 215, 255, 90);
    let last_level = wave.level_end.unwrap_or(wave.levels.len()).min(wave.levels.len());

    // Downsampled to one bar per pixel column rather than one per frame —
    // a film-length strip is thousands of frames across a few hundred
    // pixels, and a bar per frame would just draw the same column over and
    // over rather than showing more of the clip. Film frame `f` reads clip
    // level `level_start + f` — the master waveform starts at `start_frame`
    // 0, so a film frame *is* a clip frame here.
    let columns = area.width().max(1.0) as u32;
    for col in 0..columns {
        let frame = ((col as f32 / px_per_frame.max(0.001)) as u32).min(total);
        let level_idx = wave.level_start + frame as usize;
        if level_idx < wave.start_frame as usize || level_idx >= last_level {
            continue;
        }
        let level = wave.levels.get(level_idx).copied().unwrap_or(0.0);
        let amplitude = (level.clamp(0.0, 1.0) * half).max(0.5);
        let x = area.min.x + col as f32;
        painter.line_segment(
            [egui::pos2(x, middle - amplitude), egui::pos2(x, middle + amplitude)],
            Stroke::new(1.0, colour),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(key: u32, start: u32, length: u32, setup: u32) -> ShotBlock {
        ShotBlock {
            key: SceneKey(key),
            start,
            length,
            setup: Some(SceneKey(setup)),
            setup_name: format!("Setup {setup}"),
            shot_number: 1,
            takes: vec![SceneKey(key)],
            continue_from_choices: Vec::new(),
        }
    }

    /// **Clicking a block selects it.** The block spans frames 0..100 of a
    /// 200-frame film laid across a 1000 px strip, so its screen rect is
    /// `x in [RULER... , 500)` at 5 px/frame — the middle of the block,
    /// `x=250`, well clear of either edge.
    #[test]
    fn clicking_a_block_selects_it() {
        let view = SequenceView {
            blocks: vec![block(1, 0, 100, 10), block(2, 100, 100, 11)],
            master_waveform: None,
            master_frames: 200,
            fps: 24.0,
            playhead: 0,
            active_shot: None,
            setups: vec![],
        };
        let click_at = egui::pos2(250.0, 40.0);
        let mods = egui::Modifiers::default();

        // One shared context across every frame: egui only recognises a
        // click across a press-then-release pair addressed to the same
        // widget id, which only holds if the same context (and so the same
        // `Id` bookkeeping) sees every frame of the gesture.
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let drive = |events: Vec<egui::Event>| -> SequenceResponse {
            let input = egui::RawInput {
                events,
                screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 100.0))),
                ..Default::default()
            };
            let mut response = SequenceResponse::default();
            let _ = ctx.run_ui(input, |ui| {
                response = sequence_panel(ui, &view);
            });
            response
        };

        drive(vec![egui::Event::PointerMoved(click_at)]);
        drive(vec![egui::Event::PointerButton {
            pos: click_at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: mods,
        }]);
        let response = drive(vec![egui::Event::PointerButton {
            pos: click_at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: mods,
        }]);
        assert_eq!(response.select, Some(SceneKey(1)));
    }

    /// **Dragging the boundary between two blocks reports `move_cut`** at the
    /// frame under the pointer, clamped so neither shot drops to nothing.
    #[test]
    fn dragging_a_boundary_reports_move_cut() {
        let view = SequenceView {
            blocks: vec![block(1, 0, 100, 10), block(2, 100, 100, 11)],
            master_waveform: None,
            master_frames: 200,
            fps: 24.0,
            playhead: 0,
            active_shot: None,
            setups: vec![],
        };
        // The boundary sits at frame 100 -> x = 500.0 at 5 px/frame.
        let start = egui::pos2(500.0, 40.0);
        let dragged_to = egui::pos2(560.0, 40.0); // frame 112
        let mods = egui::Modifiers::default();

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let drive = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 100.0))),
                ..Default::default()
            };
            let mut response = SequenceResponse::default();
            let _ = ctx.run_ui(input, |ui| {
                response = sequence_panel(ui, &view);
            });
            response
        };

        drive(vec![egui::Event::PointerMoved(start)]);
        drive(vec![egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: mods,
        }]);
        let response = drive(vec![egui::Event::PointerMoved(dragged_to)]);

        let (left, right, at) = response.move_cut.expect("a boundary drag should report move_cut");
        assert_eq!((left, right), (SceneKey(1), SceneKey(2)));
        assert_eq!(at, 112);
    }
}
