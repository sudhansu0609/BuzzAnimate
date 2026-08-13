//! The Export dialog.
//!
//! Animate's Export Image asks three things — how large, with or without a
//! background, and (for a sequence) which frames. This asks the same three and
//! nothing else. The state is separated from the drawing so the arithmetic
//! that matters — keeping the aspect ratio, clamping a range to the timeline —
//! can be tested without a window.

use egui::{RichText, Ui};

/// Which of Animate's two export commands is being set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// One frame, as a PNG.
    Image,
    /// A numbered PNG per frame.
    Sequence,
}

impl ExportKind {
    pub fn title(self) -> &'static str {
        match self {
            ExportKind::Image => "Export Image",
            ExportKind::Sequence => "Export PNG Sequence",
        }
    }
}

/// Everything the dialog remembers.
///
/// View state: an export setting is not part of the artwork, is not saved with
/// it and is not undone.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportState {
    /// Open, and for which command.
    pub open: Option<ExportKind>,
    pub width: u32,
    pub height: u32,
    /// Export with no background.
    pub transparent: bool,
    /// Keep the stage's proportions when either size is edited.
    pub link_aspect: bool,
    /// First and last frame of a sequence, inclusive.
    pub from_frame: u32,
    pub to_frame: u32,
    /// The stage size the sizes were derived from, so the ratio survives.
    stage: (u32, u32),
    /// A run in progress: frames done, frames total.
    pub progress: Option<(u32, u32)>,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            open: None,
            width: 550,
            height: 400,
            transparent: false,
            link_aspect: true,
            from_frame: 0,
            to_frame: 0,
            stage: (550, 400),
            progress: None,
        }
    }
}

impl ExportState {
    /// Open the dialog, sized to this document.
    ///
    /// The size is reset to the stage each time rather than remembered: an
    /// export at yesterday's dimensions for a document that has since been
    /// resized is a silent way to produce the wrong file.
    pub fn open(&mut self, kind: ExportKind, stage: (u32, u32), frame_count: u32) {
        self.open = Some(kind);
        self.stage = (stage.0.max(1), stage.1.max(1));
        self.width = self.stage.0;
        self.height = self.stage.1;
        self.from_frame = 0;
        self.to_frame = frame_count.saturating_sub(1);
        self.progress = None;
    }

    pub fn close(&mut self) {
        self.open = None;
        self.progress = None;
    }

    /// Scale relative to the stage, for the "200%" readout.
    pub fn scale(&self) -> f64 {
        self.width as f64 / self.stage.0 as f64
    }

    /// Set the width, carrying the height with it when linked.
    pub fn set_width(&mut self, width: u32) {
        let width = width.clamp(1, MAX_SIDE);
        if self.link_aspect {
            let ratio = self.stage.1 as f64 / self.stage.0 as f64;
            self.height = ((width as f64 * ratio).round() as u32).clamp(1, MAX_SIDE);
        }
        self.width = width;
    }

    /// Set the height, carrying the width with it when linked.
    pub fn set_height(&mut self, height: u32) {
        let height = height.clamp(1, MAX_SIDE);
        if self.link_aspect {
            let ratio = self.stage.0 as f64 / self.stage.1 as f64;
            self.width = ((height as f64 * ratio).round() as u32).clamp(1, MAX_SIDE);
        }
        self.height = height;
    }

    /// Set both from a multiple of the stage size.
    pub fn set_scale(&mut self, factor: f64) {
        let scale = |v: u32| ((v as f64 * factor).round() as u32).clamp(1, MAX_SIDE);
        self.width = scale(self.stage.0);
        self.height = scale(self.stage.1);
    }

    /// The frame range, as a half-open range ready for the exporter.
    ///
    /// Ordered, so a range typed backwards exports those frames rather than
    /// nothing at all.
    pub fn range(&self) -> std::ops::Range<u32> {
        let first = self.from_frame.min(self.to_frame);
        let last = self.from_frame.max(self.to_frame);
        first..last + 1
    }
}

/// Nothing sensible comes of a side longer than this, and a GPU will refuse it
/// anyway. Bounded here so the field cannot be typed into an allocation that
/// fails much later and much less clearly.
const MAX_SIDE: u32 = 16_384;

/// What the user did in the dialog.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ExportResponse {
    /// Go ahead: pick a destination and export.
    pub confirmed: bool,
    /// Close without exporting.
    pub cancelled: bool,
}

/// Draw the dialog. Returns what the user chose.
pub fn export_dialog(ctx: &egui::Context, state: &mut ExportState) -> ExportResponse {
    let mut response = ExportResponse::default();
    let Some(kind) = state.open else {
        return response;
    };

    let mut still_open = true;
    egui::Window::new(kind.title())
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            if let Some((done, total)) = state.progress {
                progress_view(ui, done, total, &mut response);
                return;
            }
            settings_view(ui, kind, state, &mut response);
        });

    // The window's own close button counts as cancelling.
    if !still_open {
        response.cancelled = true;
    }
    if response.cancelled {
        state.close();
    }
    response
}

fn settings_view(
    ui: &mut Ui,
    kind: ExportKind,
    state: &mut ExportState,
    response: &mut ExportResponse,
) {
    egui::Grid::new("export-size")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Width");
            let mut width = state.width;
            if ui
                .add(
                    egui::DragValue::new(&mut width)
                        .range(1..=MAX_SIDE)
                        .suffix(" px"),
                )
                .changed()
            {
                state.set_width(width);
            }
            ui.end_row();

            ui.label("Height");
            let mut height = state.height;
            if ui
                .add(
                    egui::DragValue::new(&mut height)
                        .range(1..=MAX_SIDE)
                        .suffix(" px"),
                )
                .changed()
            {
                state.set_height(height);
            }
            ui.end_row();

            ui.label("");
            ui.checkbox(&mut state.link_aspect, "Keep proportions");
            ui.end_row();
        });

    ui.horizontal(|ui| {
        ui.label(RichText::new("Scale").small().weak());
        for factor in [0.5, 1.0, 2.0, 4.0] {
            let label = format!("{}%", (factor * 100.0) as i32);
            if ui.small_button(label).clicked() {
                state.set_scale(factor);
            }
        }
        ui.label(
            RichText::new(format!("{:.0}%", state.scale() * 100.0))
                .small()
                .weak(),
        );
    });

    ui.add_space(4.0);
    ui.checkbox(&mut state.transparent, "Transparent background")
        .on_hover_text("Leave the stage colour out, so the artwork can be composited elsewhere");

    if kind == ExportKind::Sequence {
        ui.add_space(4.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Frames");
            ui.add(egui::DragValue::new(&mut state.from_frame).range(0..=u32::MAX));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut state.to_frame).range(0..=u32::MAX));
            let count = state.range().len();
            ui.label(RichText::new(format!("({count} files)")).small().weak());
        });
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Export…").clicked() {
            response.confirmed = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

fn progress_view(ui: &mut Ui, done: u32, total: u32, response: &mut ExportResponse) {
    let fraction = if total == 0 {
        0.0
    } else {
        done as f32 / total as f32
    };
    ui.label(format!("Exporting frame {done} of {total}"));
    ui.add(egui::ProgressBar::new(fraction).show_percentage());
    ui.add_space(6.0);
    if ui.button("Stop").clicked() {
        // Stopping keeps what has already been written: half a sequence is
        // usually still worth having, and deleting the user's files because
        // they changed their mind would be worse than leaving them.
        response.cancelled = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ExportState {
        let mut s = ExportState::default();
        s.open(ExportKind::Image, (550, 400), 10);
        s
    }

    #[test]
    fn opening_sizes_the_dialog_to_the_document() {
        let s = state();
        assert_eq!((s.width, s.height), (550, 400));
        assert_eq!(s.open, Some(ExportKind::Image));
        assert_eq!(s.to_frame, 9, "the last frame, not the count");
    }

    /// A document resized since the last export must not be exported at the
    /// old dimensions.
    #[test]
    fn reopening_takes_the_current_stage_size() {
        let mut s = state();
        s.set_scale(4.0);
        assert_eq!(s.width, 2200);

        s.open(ExportKind::Image, (1920, 1080), 5);
        assert_eq!((s.width, s.height), (1920, 1080));
    }

    #[test]
    fn linked_sizes_keep_the_stage_proportions() {
        let mut s = state();
        s.set_width(1100);
        assert_eq!(s.height, 800, "height should follow");

        s.set_height(200);
        assert_eq!(s.width, 275, "and width should follow back");
    }

    #[test]
    fn unlinked_sizes_move_independently() {
        let mut s = state();
        s.link_aspect = false;
        s.set_width(1000);
        assert_eq!(s.height, 400, "height should not have moved");
    }

    #[test]
    fn scale_buttons_set_both_sides_from_the_stage() {
        let mut s = state();
        s.set_scale(2.0);
        assert_eq!((s.width, s.height), (1100, 800));
        assert_eq!(s.scale(), 2.0);

        s.set_scale(0.5);
        assert_eq!((s.width, s.height), (275, 200));
    }

    /// Absurd sizes are refused at the field rather than at the GPU, where the
    /// failure would be an allocation error a long way from the cause.
    #[test]
    fn sizes_are_bounded() {
        let mut s = state();
        s.set_width(10_000_000);
        assert_eq!(s.width, MAX_SIDE);
        s.link_aspect = false;
        s.set_height(0);
        assert_eq!(s.height, 1);
    }

    #[test]
    fn a_frame_range_is_inclusive_and_survives_being_typed_backwards() {
        let mut s = state();
        s.from_frame = 2;
        s.to_frame = 5;
        assert_eq!(s.range(), 2..6);
        assert_eq!(s.range().len(), 4);

        s.from_frame = 5;
        s.to_frame = 2;
        assert_eq!(
            s.range(),
            2..6,
            "a backwards range still exports those frames"
        );

        s.from_frame = 3;
        s.to_frame = 3;
        assert_eq!(s.range(), 3..4, "a single frame is one file");
    }

    #[test]
    fn closing_clears_any_progress() {
        let mut s = state();
        s.progress = Some((3, 10));
        s.close();
        assert!(s.open.is_none());
        assert!(s.progress.is_none());
    }
}
