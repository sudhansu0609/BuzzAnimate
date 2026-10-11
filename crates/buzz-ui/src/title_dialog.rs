//! **Insert ▸ Title Card…** (T in `docs/PRODUCTION_SPEED_PLAN.md`): the text,
//! its font, size and colour, an optional backing plate, how it animates on
//! and off, how long it holds and where it sits.
//!
//! The dialog only collects the settings; the app turns them into keyed layers
//! (`buzz_act::title`) as one undo step. Presets and positions are carried by
//! their snake_case ids, the same words the agent's `add_title_card` takes.

use egui::{RichText, Ui};

/// The five presets, `(id, label)`, in the order offered.
pub const TITLE_PRESETS: [(&str, &str); 5] = [
    ("typewriter", "Typewriter"),
    ("pop_words", "Pop words"),
    ("fade_up", "Fade up"),
    ("slide_in", "Slide in"),
    ("stamp", "Stamp"),
];

/// The three positions, `(id, label)`.
pub const TITLE_POSITIONS: [(&str, &str); 3] =
    [("centre", "Centre"), ("lower_third", "Lower third"), ("top", "Top")];

/// What a title card is made from, as the dialog holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleCardSettings {
    pub text: String,
    /// Font family; empty for the default.
    pub font: String,
    pub size: f64,
    pub color: [u8; 3],
    pub plate: bool,
    pub plate_color: [u8; 3],
    /// 0..1.
    pub plate_opacity: f64,
    /// One of [`TITLE_PRESETS`]' ids.
    pub preset: &'static str,
    pub hold_seconds: f64,
    /// One of [`TITLE_POSITIONS`]' ids.
    pub position: &'static str,
}

impl Default for TitleCardSettings {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: String::new(),
            size: 72.0,
            color: [0xFF, 0xFF, 0xFF],
            plate: false,
            plate_color: [0x14, 0x14, 0x1C],
            plate_opacity: 0.7,
            preset: "fade_up",
            hold_seconds: 2.0,
            position: "centre",
        }
    }
}

/// Dialog state: open or not, and the settings being edited (kept between
/// opens, so the second card starts from the first).
#[derive(Debug, Clone, Default)]
pub struct TitleCardState {
    pub open: bool,
    pub settings: TitleCardSettings,
}

/// What the user did with the dialog.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TitleCardResponse {
    /// Add a card with these settings.
    pub add: Option<TitleCardSettings>,
    pub cancelled: bool,
}

/// Draw the dialog. Does nothing unless it is open.
pub fn title_card_dialog(ctx: &egui::Context, state: &mut TitleCardState) -> TitleCardResponse {
    let mut response = TitleCardResponse::default();
    if !state.open {
        return response;
    }
    let mut still_open = true;
    egui::Window::new("Title Card")
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| body(ui, state, &mut response));
    if !still_open {
        response.cancelled = true;
    }
    if response.add.is_some() || response.cancelled {
        state.open = false;
    }
    response
}

fn colour_button(ui: &mut Ui, rgb: &mut [u8; 3]) {
    let mut colour = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    if ui.color_edit_button_srgba(&mut colour).changed() {
        *rgb = [colour.r(), colour.g(), colour.b()];
    }
}

fn body(ui: &mut Ui, state: &mut TitleCardState, out: &mut TitleCardResponse) {
    ui.set_min_width(380.0);
    let s = &mut state.settings;
    egui::Grid::new("title-card")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Text");
            ui.add(egui::TextEdit::multiline(&mut s.text).desired_rows(2).desired_width(260.0));
            ui.end_row();

            ui.label("Font");
            let shown = if s.font.is_empty() { "Default".to_string() } else { s.font.clone() };
            egui::ComboBox::from_id_salt("title-font")
                .selected_text(shown)
                .width(260.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.font, String::new(), "Default");
                    for family in buzz_text::font_families() {
                        let label = if family.devanagari {
                            format!("{} (Hindi)", family.name)
                        } else {
                            family.name.clone()
                        };
                        ui.selectable_value(&mut s.font, family.name.clone(), label);
                    }
                });
            ui.end_row();

            ui.label("Size");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.size).range(4.0..=1000.0).speed(1.0));
                colour_button(ui, &mut s.color);
            });
            ui.end_row();

            ui.label("Plate");
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.plate, "Backing plate");
                ui.add_enabled_ui(s.plate, |ui| {
                    colour_button(ui, &mut s.plate_color);
                    let mut percent = (s.plate_opacity * 100.0).round();
                    if ui
                        .add(egui::DragValue::new(&mut percent).range(0.0..=100.0).suffix("%"))
                        .changed()
                    {
                        s.plate_opacity = (percent / 100.0).clamp(0.0, 1.0);
                    }
                });
            });
            ui.end_row();

            ui.label("Animation");
            ui.horizontal_wrapped(|ui| {
                for (id, label) in TITLE_PRESETS {
                    if ui.selectable_label(s.preset == id, label).clicked() {
                        s.preset = id;
                    }
                }
            });
            ui.end_row();

            ui.label("Hold");
            ui.add(egui::DragValue::new(&mut s.hold_seconds).range(0.0..=60.0).speed(0.1).suffix(" s"));
            ui.end_row();

            ui.label("Position");
            ui.horizontal(|ui| {
                for (id, label) in TITLE_POSITIONS {
                    if ui.selectable_label(s.position == id, label).clicked() {
                        s.position = id;
                    }
                }
            });
            ui.end_row();
        });

    ui.label(
        RichText::new("Starts at the playhead, on a new top layer that ignores the lights.")
            .small()
            .weak(),
    );
    ui.add_space(8.0);
    let has_text = !s.text.trim().is_empty();
    ui.horizontal(|ui| {
        if ui.add_enabled(has_text, egui::Button::new("Add")).clicked() {
            out.add = Some(s.clone());
        }
        if ui.button("Cancel").clicked() {
            out.cancelled = true;
        }
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        out.cancelled = true;
    }
}
