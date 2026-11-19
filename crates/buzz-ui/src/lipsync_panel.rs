//! The Lip Sync dialog.
//!
//! Animate's asks three things: which sound, which mouth symbol, and which
//! layer the mouth goes on. This asks the same three, and shows the one thing
//! Animate does not — **which soundtrack it is about to analyse**, by name and
//! duration. That matters here because the sound comes from the document's own
//! timeline while you may be several symbols deep, and being sure you are
//! syncing to the take you think you are is worth a line of text.

use egui::{RichText, Ui};

/// **Adobe Animate's twelve visemes**, in its Lip Syncing dialog's order — the
/// same list as `buzz_audio::ANIMATE_VISEMES`, which this crate does not depend on.
pub const ANIMATE_VISEMES: [&str; 12] = ["Neutral", "Ah", "D", "Ee", "F", "L", "M", "Oh", "R", "S", "Uh", "W-Oo"];

/// How big a mouth drawing is shown in Set Visemes, in points.
pub const VISEME_THUMB: f32 = 44.0;

/// How many of a mouth chart's drawings Set Visemes shows to a row.
const VISEME_ROW: usize = 8;

/// A choice offered in the dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub id: u64,
    pub name: String,
    /// A second line — a duration, a frame count, a warning.
    pub detail: String,
    /// Can this be chosen? A mouth symbol that is too short cannot.
    pub usable: bool,
}

/// Everything the dialog remembers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LipSyncState {
    pub open: bool,
    /// The mouth symbol, by symbol id.
    pub mouth: Option<u64>,
    /// The layer the mouth goes on, by layer id.
    pub layer: Option<u64>,
    /// How loud a frame must be to open the mouth, `0.0..=1.0`.
    pub silence: f32,
    /// Shortest run of one shape, in frames.
    pub hold: u32,
    /// What the last run reported.
    pub result: Option<String>,
    /// **The mouth was picked on the stage**: offer only its own layer, so
    /// the keys cannot be sent anywhere else by accident.
    pub only_layer: bool,
    /// **Set Visemes**: the frame of the mouth chart (0-based) for each of
    /// Animate's twelve visemes ([`ANIMATE_VISEMES`]). Filled by
    /// the app for the chosen mouth; stored on it when the sync runs.
    pub visemes: Option<[u32; 12]>,
    /// The mouth `visemes` were filled for, and how many frames it has.
    pub visemes_for: Option<u64>,
    pub chart_frames: u32,
    /// The viseme whose frame is being picked from the thumbnails, if one.
    pub picking: Option<usize>,
}

impl LipSyncState {
    pub fn opened() -> Self {
        Self {
            open: true,
            silence: 0.06,
            hold: 2,
            ..Self::default()
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

/// What the user chose.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct LipSyncResponse {
    /// Run it.
    pub confirmed: bool,
    pub cancelled: bool,
    /// Make a placeholder mouth symbol to draw over.
    pub make_mouth: bool,
    /// Fill the visemes from the animator's Animate mapping.
    pub visemes_mine: bool,
    /// Fill the visemes by reading the chart's drawings.
    pub visemes_drawings: bool,
    /// Keep the visemes shown on the mouth symbol now, without a sync.
    pub visemes_save: bool,
}

/// Draw the dialog.
///
/// `track` describes the soundtrack found on the document's timeline, if any.
/// `thumbs`: a picture of each frame of the chosen mouth chart, in frame
/// order, for Set Visemes to pick from — empty when none could be drawn, and
/// the frames are then picked by number alone.
pub fn lip_sync_dialog(
    ctx: &egui::Context,
    state: &mut LipSyncState,
    track: Option<&str>,
    mouths: &[Choice],
    layers: &[Choice],
    thumbs: &[egui::TextureHandle],
) -> LipSyncResponse {
    let mut response = LipSyncResponse::default();
    if !state.open {
        return response;
    }

    let mut still_open = true;
    egui::Window::new("Lip Sync")
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // The soundtrack, named. It comes from the main timeline whichever
            // symbol is open, which is exactly the thing worth confirming.
            match track {
                Some(name) => {
                    ui.label(RichText::new("Soundtrack").small().strong());
                    ui.label(name);
                    ui.label(
                        RichText::new("The open symbol's main sound layer, else the main timeline's")
                            .small()
                            .weak(),
                    );
                }
                None => {
                    ui.label(
                        RichText::new(
                            "There is no sound on the main timeline.\nImport one with \
                             File ▸ Import to Stage, and put it on a keyframe.",
                        )
                        .small()
                        .weak(),
                    );
                }
            }

            ui.separator();
            picker(ui, "Mouth symbol", mouths, &mut state.mouth, "mouth");
            if mouths.iter().all(|m| !m.usable) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("No symbol has a frame per shape.")
                            .small()
                            .weak(),
                    );
                    if ui.small_button("Make one").clicked() {
                        response.make_mouth = true;
                    }
                });
            }

            // **Set Visemes**, as Animate's Lip Syncing dialog has it: which
            // frame of this mouth chart draws each of its twelve visemes —
            // each shown by its drawing, and picked from the chart's frames
            // drawn small, or by number.
            if let Some(visemes) = state.visemes.as_mut() {
                ui.separator();
                egui::CollapsingHeader::new(RichText::new("Set Visemes").small().strong())
                    .id_salt("lip-sync-visemes")
                    .default_open(true)
                    .show(ui, |ui| {
                        let last = state.chart_frames.max(1);
                        if state.picking.is_some_and(|slot| slot >= ANIMATE_VISEMES.len()) {
                            state.picking = None;
                        }
                        egui::Grid::new("lip-sync-viseme-grid").num_columns(6).spacing([6.0, 4.0]).show(ui, |ui| {
                            for (slot, name) in ANIMATE_VISEMES.iter().enumerate() {
                                let picking = state.picking == Some(slot);
                                match thumbs.get(visemes[slot] as usize) {
                                    Some(thumb) => {
                                        let image = egui::Image::new((thumb.id(), egui::vec2(VISEME_THUMB, VISEME_THUMB)));
                                        if ui
                                            .add(egui::Button::image(image).selected(picking))
                                            .on_hover_text(format!("{name}: frame {} \u{2014} click to pick its drawing", visemes[slot] + 1))
                                            .clicked()
                                        {
                                            state.picking = if picking { None } else { Some(slot) };
                                        }
                                    }
                                    None => {
                                        ui.label("");
                                    }
                                }
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(*name).small().strong());
                                    let mut shown = visemes[slot] + 1;
                                    if ui
                                        .add(egui::DragValue::new(&mut shown).range(1..=last).prefix("frame "))
                                        .on_hover_text(format!("The frame of the mouth chart that draws {name}"))
                                        .changed()
                                    {
                                        visemes[slot] = shown.saturating_sub(1);
                                    }
                                });
                                if slot % 3 == 2 {
                                    ui.end_row();
                                }
                            }
                        });
                        // **The chart's frames, to pick from**: every drawing of
                        // the mouth, numbered; the one the viseme has now
                        // marked. A click gives it that frame.
                        if let Some(slot) = state.picking {
                            ui.add_space(4.0);
                            ui.label(RichText::new(format!("Pick the drawing for {}:", ANIMATE_VISEMES[slot])).small());
                            // Eight to a row, so the strip is no wider than the
                            // grid above rather than one row widening the
                            // window.
                            egui::ScrollArea::vertical().id_salt("lip-sync-viseme-frames").max_height(200.0).show(ui, |ui| {
                                for (row, chunk) in thumbs.chunks(VISEME_ROW).enumerate() {
                                    ui.horizontal(|ui| {
                                        for (i, thumb) in chunk.iter().enumerate() {
                                            let frame = row * VISEME_ROW + i;
                                            let chosen = visemes[slot] as usize == frame;
                                            ui.vertical(|ui| {
                                                let image = egui::Image::new((thumb.id(), egui::vec2(VISEME_THUMB, VISEME_THUMB)));
                                                if ui
                                                    .add(egui::Button::image(image).selected(chosen))
                                                    .on_hover_text(format!("Frame {} for {}", frame + 1, ANIMATE_VISEMES[slot]))
                                                    .clicked()
                                                {
                                                    visemes[slot] = frame as u32;
                                                }
                                                ui.label(RichText::new(format!("{}", frame + 1)).small().weak());
                                            });
                                        }
                                    });
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("My Animate mapping").on_hover_text("The mapping your Animate lip sync used (autoLipSyncImporter)").clicked() {
                                response.visemes_mine = true;
                            }
                            if ui.small_button("Read from drawings").on_hover_text("Guess each viseme's frame from how open, wide and round each drawing is").clicked() {
                                response.visemes_drawings = true;
                            }
                            if ui
                                .small_button("Save on mouth")
                                .on_hover_text("Keep these frames on the mouth symbol, so every lip sync uses them \u{2014} Sync here and the talking commands over talk ranges (Shift+A) alike")
                                .clicked()
                            {
                                response.visemes_save = true;
                            }
                        });
                    });
            }

            ui.separator();
            picker(ui, "On layer", layers, &mut state.layer, "layer");

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Silence");
                ui.add(egui::Slider::new(&mut state.silence, 0.0..=0.5).fixed_decimals(2))
                    .on_hover_text("Below this loudness the mouth stays closed");
            });
            ui.horizontal(|ui| {
                ui.label("Hold");
                ui.add(egui::Slider::new(&mut state.hold, 1..=6).suffix(" frames"))
                    .on_hover_text(
                        "Shortest a shape may last. Below two frames a mouth reads as \
                         flickering rather than speaking.",
                    );
            });

            if let Some(result) = &state.result {
                ui.separator();
                ui.label(RichText::new(result).small().weak());
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ready = track.is_some() && state.mouth.is_some() && state.layer.is_some();
                if ui
                    .add_enabled(ready, egui::Button::new("Sync"))
                    .on_hover_text("Analyse the soundtrack and key the mouth")
                    .clicked()
                {
                    response.confirmed = true;
                }
                if ui.button("Close").clicked() {
                    response.cancelled = true;
                }
            });
        });

    if !still_open {
        response.cancelled = true;
    }
    if response.cancelled {
        state.close();
    }
    response
}

/// A list of choices, one selectable.
fn picker(ui: &mut Ui, label: &str, choices: &[Choice], chosen: &mut Option<u64>, salt: &str) {
    ui.label(RichText::new(label).small().strong());

    if choices.is_empty() {
        ui.label(RichText::new("nothing to choose").small().weak());
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt(salt)
        .max_height(110.0)
        .show(ui, |ui| {
            for choice in choices {
                let selected = *chosen == Some(choice.id);
                let text = if choice.detail.is_empty() {
                    choice.name.clone()
                } else {
                    format!("{}   —   {}", choice.name, choice.detail)
                };

                // An unusable choice is shown and disabled rather than hidden:
                // a mouth symbol missing from the list looks like a bug, where
                // one greyed out with "6 frames, needs 10" explains itself.
                let clicked = ui
                    .add_enabled_ui(choice.usable, |ui| ui.selectable_label(selected, text))
                    .inner
                    .clicked();
                if clicked {
                    *chosen = Some(choice.id);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(id: u64, usable: bool) -> Choice {
        Choice {
            id,
            name: format!("Item {id}"),
            detail: String::new(),
            usable,
        }
    }

    /// **Set Visemes picks a frame by its drawing**: with a viseme's picture
    /// clicked, the chart's frames are shown, and a click on one gives the
    /// viseme that frame.
    #[test]
    fn a_viseme_is_picked_by_its_drawing() {
        let ctx = egui::Context::default();
        let thumbs: Vec<egui::TextureHandle> = (0..6)
            .map(|i| ctx.load_texture(format!("t{i}"), egui::ColorImage::filled([4, 4], egui::Color32::WHITE), Default::default()))
            .collect();
        let mut state = LipSyncState::opened();
        state.mouth = Some(1);
        state.visemes = Some([0; 12]);
        state.chart_frames = 6;
        state.picking = Some(1);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 1000.0));
        let mut run = |events: Vec<egui::Event>, state: &mut LipSyncState| {
            let out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| {
                let _ = lip_sync_dialog(ui.ctx(), state, Some("Line"), &[], &[], &thumbs);
            });
            // Where frame 4's picture is drawn in the strip: just above its
            // number.
            fn find(shape: &egui::Shape, at: &mut Option<egui::Pos2>) {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == "4" => {
                        let r = text.visual_bounding_rect();
                        *at = Some(egui::pos2(r.center().x, r.top() - VISEME_THUMB / 2.0 - 6.0));
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| find(s, at)),
                    _ => {}
                }
            }
            let mut at = None;
            for clipped in &out.shapes {
                find(&clipped.shape, &mut at);
            }
            at
        };
        run(Vec::new(), &mut state);
        let at = run(Vec::new(), &mut state).expect("frame 4's drawing is offered");
        let m = egui::Modifiers::default();
        run(vec![egui::Event::PointerMoved(at)], &mut state);
        run(vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: m }], &mut state);
        run(vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: m }], &mut state);
        assert_eq!(state.visemes.unwrap()[1], 3, "Ah takes frame 4");
        assert_eq!(state.visemes.unwrap()[0], 0, "the others keep theirs");
    }

    #[test]
    fn a_dialog_starts_with_animates_defaults() {
        let state = LipSyncState::opened();
        assert!(state.open);
        assert_eq!(state.hold, 2, "two frames is the readable minimum");
        assert!(state.silence > 0.0 && state.silence < 0.2);
        assert!(state.mouth.is_none());
    }

    #[test]
    fn closing_clears_the_open_flag() {
        let mut state = LipSyncState::opened();
        state.close();
        assert!(!state.open);
    }

    #[test]
    fn a_closed_dialog_draws_nothing_and_reports_nothing() {
        let ctx = egui::Context::default();
        let mut state = LipSyncState::default();
        let mut response = LipSyncResponse::default();

        let _ = ctx.run_ui(Default::default(), |ui| {
            response = lip_sync_dialog(ui.ctx(), &mut state, Some("Line"), &[], &[], &[]);
        });
        assert_eq!(response, LipSyncResponse::default());
    }

    /// The dialog has to draw in every state it can be in, including the ones
    /// that are all warnings.
    #[test]
    fn the_dialog_draws_with_and_without_a_soundtrack() {
        let ctx = egui::Context::default();
        let mouths = vec![choice(1, true), choice(2, false)];
        let layers = vec![choice(10, true)];

        for track in [Some("Dialogue — 4.2 s"), None] {
            let mut state = LipSyncState::opened();
            state.mouth = Some(1);
            state.layer = Some(10);
            let _ = ctx.run_ui(Default::default(), |ui| {
                let _ = lip_sync_dialog(ui.ctx(), &mut state, track, &mouths, &layers, &[]);
            });
        }
    }
}
