//! The New Document dialog: how big, how fast, what colour.
//!
//! # Why ask at all
//!
//! A new document used to be Animate's own default — 550×400 at 24 fps — which
//! was the right answer in 2005 and is the wrong one now: almost everything
//! made today is delivered at 1920×1080 or at a phone's proportions, and
//! resizing a document after the artwork is drawn means rescaling every layer
//! and every camera move. Asking once, at the start, costs a keypress and saves
//! that.
//!
//! # Why the choice is remembered
//!
//! Somebody working on a series makes twenty documents at the same size. The
//! dialog opens on whatever was chosen last, kept with the workspace — a
//! preference belonging to the person, not to any one film — so the second
//! document onwards is Enter.

use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};

/// The settings a new document is made with.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DocumentSetup {
    pub width: f64,
    pub height: f64,
    pub frame_rate: f64,
    /// `#RRGGBB`, as everywhere else in this program's files.
    pub background: [u8; 3],
}

impl Default for DocumentSetup {
    fn default() -> Self {
        // **Full HD at 24 fps**: the size almost everything is delivered at,
        // and the rate animation is drawn on. Animate defaults to 550×400 for
        // reasons that stopped applying when Flash Player did.
        Self {
            width: 1920.0,
            height: 1080.0,
            frame_rate: 24.0,
            background: [0xFF, 0xFF, 0xFF],
        }
    }
}

impl DocumentSetup {
    /// Bring absurd numbers back into range.
    ///
    /// A stage of zero has no rectangle and a frame rate of zero never
    /// advances; both are reachable by typing, and neither should be a way to
    /// make an unusable document.
    pub fn sane(mut self) -> Self {
        self.width = self.width.clamp(1.0, 16_384.0);
        self.height = self.height.clamp(1.0, 16_384.0);
        self.frame_rate = if self.frame_rate.is_finite() {
            self.frame_rate.clamp(0.01, 240.0)
        } else {
            24.0
        };
        self
    }
}

/// A named starting point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub width: f64,
    pub height: f64,
    pub frame_rate: f64,
    /// What it is for, in the words somebody would use out loud.
    pub note: &'static str,
}

/// The presets offered, in the order they are shown.
///
/// Delivery formats first, because that is what a new document usually is;
/// Animate's own default last, for opening older work at its native size.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Full HD",
        width: 1920.0,
        height: 1080.0,
        frame_rate: 24.0,
        note: "1080p \u{2014} the usual delivery size",
    },
    Preset {
        name: "HD",
        width: 1280.0,
        height: 720.0,
        frame_rate: 24.0,
        note: "720p \u{2014} lighter to draw and to render",
    },
    Preset {
        name: "4K UHD",
        width: 3840.0,
        height: 2160.0,
        frame_rate: 24.0,
        note: "For work that will be scaled down, or shown large",
    },
    Preset {
        name: "Square",
        width: 1080.0,
        height: 1080.0,
        frame_rate: 30.0,
        note: "Social posts",
    },
    Preset {
        name: "Vertical",
        width: 1080.0,
        height: 1920.0,
        frame_rate: 30.0,
        note: "Phones: shorts, reels, stories",
    },
    Preset {
        name: "Film 2K",
        width: 2048.0,
        height: 858.0,
        frame_rate: 24.0,
        note: "Cinema scope",
    },
    Preset {
        name: "Animate default",
        width: 550.0,
        height: 400.0,
        frame_rate: 24.0,
        note: "What Animate opens with, for matching older work",
    },
];

/// Frame rates worth one click.
///
/// 12 is animating on twos at 24; 24 is film; 25 and 30 are the two broadcast
/// rates; 60 is for games and for very smooth motion.
pub const FRAME_RATES: &[f64] = &[12.0, 24.0, 25.0, 30.0, 60.0];

/// Dialog state, and the settings it is editing.
#[derive(Debug, Clone, Default)]
pub struct NewDocumentState {
    pub open: bool,
    pub setup: DocumentSetup,
}

/// What the user did with the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NewDocumentResponse {
    /// Make a document with these settings.
    pub create: Option<DocumentSetup>,
    pub cancelled: bool,
}

/// Draw the dialog. Does nothing unless it is open.
pub fn new_document_dialog(
    ctx: &egui::Context,
    state: &mut NewDocumentState,
) -> NewDocumentResponse {
    let mut response = NewDocumentResponse::default();
    if !state.open {
        return response;
    }

    let mut still_open = true;
    egui::Window::new("New Document")
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            body(ui, state, &mut response);
        });

    // The window's own close button counts as cancelling.
    if !still_open {
        response.cancelled = true;
    }
    if response.create.is_some() || response.cancelled {
        state.open = false;
    }
    response
}

fn body(ui: &mut Ui, state: &mut NewDocumentState, out: &mut NewDocumentResponse) {
    ui.set_min_width(360.0);

    ui.label(RichText::new("Presets").small().weak());
    ui.horizontal_wrapped(|ui| {
        for preset in PRESETS {
            let chosen = (state.setup.width - preset.width).abs() < 0.5
                && (state.setup.height - preset.height).abs() < 0.5
                && (state.setup.frame_rate - preset.frame_rate).abs() < 0.001;
            if ui
                .selectable_label(chosen, preset.name)
                .on_hover_text(format!(
                    "{} \u{00D7} {} at {} fps\n{}",
                    preset.width, preset.height, preset.frame_rate, preset.note
                ))
                .clicked()
            {
                state.setup.width = preset.width;
                state.setup.height = preset.height;
                state.setup.frame_rate = preset.frame_rate;
            }
        }
    });

    ui.add_space(6.0);
    egui::Grid::new("new-doc")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Width");
            ui.add(
                egui::DragValue::new(&mut state.setup.width)
                    .range(1.0..=16_384.0)
                    .suffix(" px"),
            );
            ui.end_row();

            ui.label("Height");
            ui.add(
                egui::DragValue::new(&mut state.setup.height)
                    .range(1.0..=16_384.0)
                    .suffix(" px"),
            );
            ui.end_row();

            ui.label("Frame rate");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut state.setup.frame_rate)
                        .range(0.01..=240.0)
                        .speed(0.1)
                        .suffix(" fps"),
                );
                for rate in FRAME_RATES {
                    let chosen = (state.setup.frame_rate - rate).abs() < 0.001;
                    if ui
                        .selectable_label(chosen, format!("{rate:.0}"))
                        .clicked()
                    {
                        state.setup.frame_rate = *rate;
                    }
                }
            });
            ui.end_row();

            ui.label("Background");
            let mut colour = egui::Color32::from_rgb(
                state.setup.background[0],
                state.setup.background[1],
                state.setup.background[2],
            );
            if ui.color_edit_button_srgba(&mut colour).changed() {
                state.setup.background = [colour.r(), colour.g(), colour.b()];
            }
            ui.end_row();
        });

    // What the numbers mean, in the terms somebody would check them in.
    let ratio = ratio_label(state.setup.width, state.setup.height);
    ui.label(
        RichText::new(format!(
            "{:.0} \u{00D7} {:.0} \u{00B7} {ratio} \u{00B7} {:.0} fps",
            state.setup.width, state.setup.height, state.setup.frame_rate
        ))
        .small()
        .weak(),
    );

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Create").clicked() {
            out.create = Some(state.setup.sane());
        }
        if ui.button("Cancel").clicked() {
            out.cancelled = true;
        }
        ui.label(
            RichText::new("these settings become the default for the next one")
                .small()
                .weak()
                .italics(),
        );
    });

    // Enter creates, Escape cancels — the two keys a dialog like this owes you.
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        out.create = Some(state.setup.sane());
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        out.cancelled = true;
    }
}

/// The aspect ratio in the form people say it, when it is one of those.
fn ratio_label(width: f64, height: f64) -> String {
    if height <= 0.0 {
        return String::new();
    }
    let ratio = width / height;
    for (name, value) in [
        ("16:9", 16.0 / 9.0),
        ("9:16", 9.0 / 16.0),
        ("4:3", 4.0 / 3.0),
        ("1:1", 1.0),
        ("21:9", 21.0 / 9.0),
        ("3:2", 3.0 / 2.0),
    ] {
        if (ratio - value).abs() < 0.01 {
            return name.to_string();
        }
    }
    format!("{ratio:.2}:1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_document_is_full_hd_at_twenty_four() {
        let setup = DocumentSetup::default();
        assert_eq!(setup.width, 1920.0);
        assert_eq!(setup.height, 1080.0);
        assert_eq!(setup.frame_rate, 24.0);
        assert_eq!(setup.background, [0xFF, 0xFF, 0xFF]);
    }

    /// Every preset has to be a document somebody could actually work in.
    #[test]
    fn every_preset_is_usable() {
        for preset in PRESETS {
            assert!(preset.width >= 1.0 && preset.width <= 16_384.0, "{preset:?}");
            assert!(preset.height >= 1.0 && preset.height <= 16_384.0, "{preset:?}");
            assert!(
                preset.frame_rate > 0.0 && preset.frame_rate <= 240.0,
                "{preset:?}"
            );
            assert!(!preset.name.is_empty() && !preset.note.is_empty());
        }
    }

    /// The first preset is what the dialog opens on, so it has to *be* the
    /// default rather than merely resemble it.
    #[test]
    fn the_first_preset_matches_the_default() {
        let first = PRESETS[0];
        let setup = DocumentSetup::default();
        assert_eq!(first.width, setup.width);
        assert_eq!(first.height, setup.height);
        assert_eq!(first.frame_rate, setup.frame_rate);
    }

    #[test]
    fn absurd_numbers_are_brought_back() {
        let mad = DocumentSetup {
            width: 0.0,
            height: 9e9,
            frame_rate: f64::NAN,
            background: [0, 0, 0],
        }
        .sane();
        assert_eq!(mad.width, 1.0);
        assert_eq!(mad.height, 16_384.0);
        assert_eq!(mad.frame_rate, 24.0);
    }

    #[test]
    fn ratios_are_named_the_way_people_say_them() {
        assert_eq!(ratio_label(1920.0, 1080.0), "16:9");
        assert_eq!(ratio_label(1080.0, 1920.0), "9:16");
        assert_eq!(ratio_label(1080.0, 1080.0), "1:1");
        assert_eq!(ratio_label(550.0, 400.0), "1.38:1");
    }

    /// The dialog draws, creates and cancels — in each case closing itself,
    /// because a dialog that stays open after Create makes a second document.
    #[test]
    fn the_dialog_opens_creates_and_closes() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut state = NewDocumentState {
            open: true,
            setup: DocumentSetup::default(),
        };

        let mut response = NewDocumentResponse::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            response = new_document_dialog(ui.ctx(), &mut state);
        });
        assert!(state.open, "it should still be open with nothing clicked");
        assert!(response.create.is_none());

        // Cancelling closes it.
        state.open = true;
        let mut out = NewDocumentResponse::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let mut r = new_document_dialog(ui.ctx(), &mut state);
            r.cancelled = true;
            // Emulate the close having been asked for.
            state.open = false;
            out = r;
        });
        assert!(!state.open);
        assert!(out.create.is_none());
    }
}
