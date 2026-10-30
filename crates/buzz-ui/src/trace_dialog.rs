//! The Trace Bitmap dialog — Animate's Modify ▸ Bitmap ▸ Trace Bitmap…,
//! opened from [`crate::command::Command::TraceBitmapOptions`].
//!
//! `TraceBitmap` and `TraceLineArt` already trace with sensible defaults and
//! need no dialog at all; this is for tuning those defaults, or reaching for
//! `Both` — regions and lines in one pass — which nothing else offers.

use std::path::PathBuf;

use crossbeam_channel::Receiver;
use egui::Ui;

use buzz_scene::{LineStyle, TraceMode, TraceOptions, TraceTuning};

/// What the user did with the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TraceDialogAction {
    /// Still open; nothing decided yet.
    #[default]
    None,
    /// Trace with the options as they now stand.
    Trace,
    /// Close without tracing.
    Cancel,
}

/// Draw the dialog, editing `options` in place.
pub fn trace_dialog(ctx: &egui::Context, options: &mut TraceOptions) -> TraceDialogAction {
    let mut action = TraceDialogAction::None;
    egui::Window::new("Trace Bitmap")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| body(ui, options, &mut action));
    action
}

fn body(ui: &mut Ui, options: &mut TraceOptions, action: &mut TraceDialogAction) {
    ui.set_min_width(320.0);

    recommended_row(ui, || *options = TraceOptions::ink_colour());
    ui.separator();

    ui.label("Mode");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut options.mode, TraceMode::Ink, "Ink");
        ui.selectable_value(&mut options.mode, TraceMode::InkAndColour, "Ink + colour");
        ui.selectable_value(&mut options.mode, TraceMode::Auto, "Auto");
        ui.selectable_value(&mut options.mode, TraceMode::Outline, "Outline fills");
        ui.selectable_value(&mut options.mode, TraceMode::ColourLines, "Colour lines");
        ui.selectable_value(&mut options.mode, TraceMode::LineArt, "Line art");
        ui.selectable_value(&mut options.mode, TraceMode::Both, "Both");
    });
    ui.separator();

    egui::Grid::new("trace-dialog-grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            let ink_mode = matches!(options.mode, TraceMode::Ink | TraceMode::InkAndColour);
            if ink_mode {
                ui.label("Bold");
                ui.add(egui::Slider::new(&mut options.ink_bold, -1.0..=3.0).step_by(0.05).suffix(" px"))
                    .on_hover_text("px added to each side of every line; 0 = the source's weight, negative = thinner");
                ui.end_row();

                ui.label("Smoothness");
                ui.add(egui::Slider::new(&mut options.line_smooth, 0.0..=1.0))
                    .on_hover_text("How much a traced line is smoothed");
                ui.end_row();
            }

            if options.mode == TraceMode::InkAndColour {
                ui.label("Colours");
                ui.add(egui::Slider::new(&mut options.colours, 2..=64))
                    .on_hover_text("How many colours the fills under the ink are reduced to");
                ui.end_row();
            }

            if matches!(options.mode, TraceMode::ColourLines | TraceMode::Auto | TraceMode::Both) {
                ui.label("Outline contrast");
                ui.add(egui::Slider::new(&mut options.outline_contrast, 1.0..=50.0).suffix(" \u{0394}E"))
                    .on_hover_text("Minimum colour contrast (\u{0394}E) to draw an outline between regions");
                ui.end_row();

                ui.label("Outline width");
                let mut width = options.uniform_width.unwrap_or(0.0);
                let suffix = if width <= 0.0 { " (Auto)" } else { " px" };
                let resp = ui
                    .add(
                        egui::Slider::new(&mut width, 0.0..=20.0).suffix(suffix),
                    )
                    .on_hover_text("Outline stroke width. 0 means automatic based on image size");
                if resp.changed() {
                    options.uniform_width = if width <= 0.0 { None } else { Some(width) };
                }
                ui.end_row();
            }

            if options.mode != TraceMode::Outline
                && options.mode != TraceMode::ColourLines
                && !ink_mode
            {
                ui.label("Smoothness");
                ui.add(egui::Slider::new(&mut options.line_smooth, 0.0..=1.0))
                    .on_hover_text("How much a traced line is smoothed");
                ui.end_row();

                ui.label("Shortest line");
                ui.add(
                    egui::Slider::new(&mut options.min_line_length, 0.0..=100.0).suffix(" px"),
                )
                .on_hover_text("Lines shorter than this are dropped as noise");
                ui.end_row();

                ui.label("Line style");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut options.line_style, LineStyle::Stroke, "Plain line");
                    ui.selectable_value(
                        &mut options.line_style,
                        LineStyle::Brush,
                        "Brush stroke with variable width",
                    );
                });
                ui.end_row();

                ui.label("Fill enclosed areas");
                ui.checkbox(&mut options.fill_enclosed, "")
                    .on_hover_text("Trace the paper a closed line encloses as fills too");
                ui.end_row();

                ui.label("Thickest line");
                let mut diameter = options.max_line_radius * 2.0;
                if ui
                    .add(egui::Slider::new(&mut diameter, 2.0..=40.0).suffix(" px"))
                    .on_hover_text(
                        "Ink wider than this is traced as a filled shape rather than a line",
                    )
                    .changed()
                {
                    options.max_line_radius = diameter / 2.0;
                }
                ui.end_row();
            }

            if options.mode != TraceMode::LineArt && !ink_mode {
                ui.label("Colours");
                ui.add(egui::Slider::new(&mut options.colours, 2..=64));
                ui.end_row();

                ui.label("Detail");
                ui.add(egui::Slider::new(&mut options.detail, 0.0..=4.0));
                ui.end_row();

                ui.label("Corner smoothing");
                ui.add(egui::Slider::new(&mut options.smooth, 0.0..=1.0));
                ui.end_row();

                ui.label("Drop background");
                ui.checkbox(&mut options.drop_background, "")
                    .on_hover_text("Throw away the colour that covers the most of the picture");
                ui.end_row();
            }

            ui.label("Speck size");
            ui.add(egui::Slider::new(&mut options.speckle, 1..=64).suffix(" px"))
                .on_hover_text("The smallest region or speck worth keeping");
            ui.end_row();
        });

    // The dialog traces on **Trace** with the options as they then stand, so
    // a tuning change needs nothing more than being written into them.
    tuning_section(ui, &mut options.tuning, options.mode == TraceMode::InkAndColour);

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            *action = TraceDialogAction::Cancel;
        }
        if ui.button("Trace").clicked() {
            *action = TraceDialogAction::Trace;
        }
    });
}

/// The settings settled on for tracing characters (2026-10-07/08): Ink + colour,
/// bold 0 (the source's own line weight), smoothness 0.5, 24 colours, speck 12,
/// the built-in tuning. Shown at the top of both trace dialogs.
pub const RECOMMENDED_NOTE: &str =
    "Ink + colour, bold 0 px, smoothness 0.5, 24 colours, speck 12 px, built-in tuning";

/// **Reset to recommended**: a button that puts every setting of the dialog
/// back to [`RECOMMENDED_NOTE`]'s values — not the saved tuning file, so a
/// tuning saved by mistake is undone too. `reset` does the putting back.
pub fn recommended_row(ui: &mut Ui, reset: impl FnOnce()) {
    ui.horizontal(|ui| {
        if ui
            .button("\u{21ba} Reset to recommended")
            .on_hover_text(format!(
                "Every setting back to the ones we settled on: {RECOMMENDED_NOTE}. \
                 Ignores the saved tuning file."
            ))
            .clicked()
        {
            reset();
        }
    });
}

/// **Tuning**: one slider per [`TraceTuning`] knob (a combo box for
/// `upsample`), under a header closed by default, with **Save as default**,
/// **Reset** and **Load file…** below them. Shared by the Trace Bitmap dialog
/// and the Character from Image preview; `show_colour` adds the colour
/// stage's knobs. True when a value changed.
pub fn tuning_section(ui: &mut Ui, tuning: &mut TraceTuning, show_colour: bool) -> bool {
    tuning_section_open(ui, tuning, show_colour, None)
}

/// [`tuning_section`] with every header forced open (`Some(true)`) or left
/// to the user (`None`).
fn tuning_section_open(ui: &mut Ui, tuning: &mut TraceTuning, show_colour: bool, open: Option<bool>) -> bool {
    // `SliderClamping::Edits`: drawing a slider never moves its value (by
    // default egui re-snaps it to the step every frame, so a default 0.15
    // came back 0.15000000000000002 and a file's off-step value was lost
    // just by opening the section); `max_decimals` keeps a dragged value
    // free of float noise.
    let mut changed = false;
    egui::CollapsingHeader::new("Tuning").default_open(false).open(open).show(ui, |ui| {
        egui::CollapsingHeader::new("Ink").open(open).show(ui, |ui| {
            let ink = &mut tuning.ink;
            egui::Grid::new("trace-tuning-ink")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Ink level");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.ink_level, 0.2..=0.8).step_by(0.01).clamping(egui::SliderClamping::Edits).max_decimals(2))
                        .on_hover_text("Coverage at or above this is ink; higher keeps only the darker middle of a line, so lines come out thinner.")
                        .changed();
                    ui.end_row();

                    ui.label("Line contrast");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.min_contrast, 20.0..=120.0).step_by(1.0).clamping(egui::SliderClamping::Edits))
                        .on_hover_text("How much brighter than the darkest ink a line's surroundings must be for it to count; higher drops lines drawn on dark cloth or hair.")
                        .changed();
                    ui.end_row();

                    ui.label("Dark ink level");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.dark_luma, 20.0..=90.0).step_by(1.0).clamping(egui::SliderClamping::Edits))
                        .on_hover_text("A pixel this dark (luma, 0–255) or darker is ink outright; higher turns more dark cloth and shadow into solid ink.")
                        .changed();
                    ui.end_row();

                    ui.label("Dark ramp");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.dark_ramp, 2.0..=40.0).step_by(0.5).clamping(egui::SliderClamping::Edits).max_decimals(1))
                        .on_hover_text("How many luma steps below the dark-ink level it takes to become fully ink; higher softens the edge of dark masses.")
                        .changed();
                    ui.end_row();

                    ui.label("Background percentile");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.bg_percentile, 0.5..=0.95).step_by(0.005).clamping(egui::SliderClamping::Edits).max_decimals(3))
                        .on_hover_text("Which brightness of a pixel's neighbourhood counts as its background (0–1); higher finds lines against the lightest nearby colour.")
                        .changed();
                    ui.end_row();

                    ui.label("Band half-width");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.band_half_width_px, 1.0..=8.0).step_by(0.1).suffix(" px").clamping(egui::SliderClamping::Edits).max_decimals(1))
                        .on_hover_text("Half-width (px) beyond which a contrast-only dark band is a cloth edge, not a line; higher keeps thicker bands as ink.")
                        .changed();
                    ui.end_row();

                    ui.label("Corner angle");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.corner_deg, 30.0..=120.0).step_by(1.0).suffix("\u{00b0}").clamping(egui::SliderClamping::Edits))
                        .on_hover_text("A bend sharper than this many degrees stays a sharp corner; higher rounds off more corners.")
                        .changed();
                    ui.end_row();

                    ui.label("Curve fit tolerance");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.fit_tolerance_px, 0.05..=0.6).step_by(0.01).suffix(" px").clamping(egui::SliderClamping::Edits).max_decimals(2))
                        .on_hover_text("How far (px) the fitted curve may stray from the traced edge; higher gives fewer, looser curves.")
                        .changed();
                    ui.end_row();

                    ui.label("Upsample");
                    let before = ink.upsample;
                    egui::ComboBox::from_id_salt("trace-tuning-upsample")
                        .selected_text(match ink.upsample { None => "Auto".to_string(), Some(u) => u.to_string() })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut ink.upsample, None, "Auto");
                            for u in [1usize, 2, 4, 6] {
                                ui.selectable_value(&mut ink.upsample, Some(u), u.to_string());
                            }
                        })
                        .response
                        .on_hover_text("Sub-pixel precision of the edge: Auto, or 1–6 cells per pixel; higher is a finer edge and a slower trace.");
                    changed |= ink.upsample != before;
                    ui.end_row();

                    ui.label("Faint mark contrast");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.faint_max_contrast, 10.0..=80.0).step_by(1.0).clamping(egui::SliderClamping::Edits))
                        .on_hover_text("A small mark standing out from its surroundings by less than this (luma) is a faint crease and dropped; higher drops more faint marks.")
                        .changed();
                    ui.end_row();

                    ui.label("Faint mark size");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.faint_max_px2, 20.0..=600.0).step_by(10.0).suffix(" px\u{00b2}").clamping(egui::SliderClamping::Edits))
                        .on_hover_text("Only marks smaller than this (px²) can be dropped as faint; higher lets bigger faint marks go too.")
                        .changed();
                    ui.end_row();

                    ui.label("Dark gap size");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.dark_hole_max_px2, 20.0..=1000.0).step_by(10.0).suffix(" px\u{00b2}").clamping(egui::SliderClamping::Edits))
                        .on_hover_text("A gap inside ink up to this size (px²) is filled when it is dark; higher fills bigger dark gaps.")
                        .changed();
                    ui.end_row();

                    ui.label("Dark gap level");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.dark_hole_max_luma, 40.0..=200.0).step_by(2.0).clamping(egui::SliderClamping::Edits))
                        .on_hover_text("A gap inside ink darker than this (luma) counts as dark and is filled; higher fills lighter gaps too.")
                        .changed();
                    ui.end_row();

                    ui.label("Colour mark chroma");
                    changed |= ui
                        .add(egui::Slider::new(&mut ink.colour_mark_min_chroma, 20.0..=160.0).step_by(2.0).clamping(egui::SliderClamping::Edits))
                        .on_hover_text("A light mark more colourful than this is left for the colour fills, not ink; higher keeps more coloured marks as ink.")
                        .changed();
                    ui.end_row();
                });
        });
        if show_colour {
            egui::CollapsingHeader::new("Colour").open(open).show(ui, |ui| {
                let colour = &mut tuning.colour;
                egui::Grid::new("trace-tuning-colour")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Smoothing passes");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.bilateral_passes, 0..=4).clamping(egui::SliderClamping::Edits))
                            .on_hover_text("How many smoothing passes flatten shading before colours are picked; higher gives flatter, simpler fills.")
                            .changed();
                        ui.end_row();

                        ui.label("Smoothing range");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.bilateral_range_sigma, 3.0..=30.0).step_by(0.5).suffix(" \u{0394}E").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("How different (ΔE) two colours may be and still be smoothed together; higher merges more shading into one tone.")
                            .changed();
                        ui.end_row();

                        ui.label("Palette merge");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.palette_merge_de, 2.0..=16.0).step_by(0.2).suffix(" \u{0394}E").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("Palette colours closer than this (ΔE) become one; higher gives fewer colours.")
                            .changed();
                        ui.end_row();

                        ui.label("Colours per surface");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.surface_max_k, 2..=16).clamping(egui::SliderClamping::Edits))
                            .on_hover_text("Most colours one flat surface may be split into; higher keeps more shades.")
                            .changed();
                        ui.end_row();

                        ui.label("Wash grain");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.textured_median_gradient, 1.0..=8.0).step_by(0.1).clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("A surface this grainy (median ΔE per px) or more is treated as a painterly wash; higher treats fewer surfaces as washes.")
                            .changed();
                        ui.end_row();

                        ui.label("Colours per wash");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.surface_max_k_textured, 1..=8).clamping(egui::SliderClamping::Edits))
                            .on_hover_text("Most colours a painterly-wash surface may be split into; higher keeps more patches of the wash.")
                            .changed();
                        ui.end_row();

                        ui.label("Soft merge");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.soft_merge_max_de, 4.0..=30.0).step_by(0.5).suffix(" \u{0394}E").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("Neighbouring fills closer than this (ΔE) across a soft edge merge into one; higher merges more gradient bands.")
                            .changed();
                        ui.end_row();

                        ui.label("Soft merge edge cap");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.soft_merge_gradient_cap, 2.0..=15.0).step_by(0.2).clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("An edge sharper than this (ΔE per px) is never soft enough to merge across; higher lets busier patterns merge.")
                            .changed();
                        ui.end_row();

                        ui.label("Smallest fill");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.min_region_px2, 4..=400).step_by(4.0).suffix(" px\u{00b2}").clamping(egui::SliderClamping::Edits))
                            .on_hover_text("Fills smaller than this (px²) are merged into a neighbour; higher removes more small patches.")
                            .changed();
                        ui.end_row();

                        ui.label("Keep contrasting fill");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.contrast_region_min_de, 8.0..=40.0).step_by(0.5).suffix(" \u{0394}E").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("A small fill this different (ΔE) from all its neighbours is kept anyway; higher keeps fewer small marks.")
                            .changed();
                        ui.end_row();

                        ui.label("Pattern line contrast");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.residual_line_de, 5.0..=30.0).step_by(0.5).suffix(" \u{0394}E").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("How different (ΔE) from its fill a thin stripe must be to become a pattern line; higher finds fewer pattern lines.")
                            .changed();
                        ui.end_row();

                        ui.label("Under-paint depth");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.under_paint_max_layers, 0..=8).suffix(" px").clamping(egui::SliderClamping::Edits))
                            .on_hover_text("How many pixels deep the fills reach under the ink; higher runs fills under thicker lines.")
                            .changed();
                        ui.end_row();

                        ui.label("Ink grow");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.ink_grow_px, 0..=3).suffix(" px").clamping(egui::SliderClamping::Edits))
                            .on_hover_text("How many pixels the ink is widened before fills are painted under it; higher hides more of the line's soft rim.")
                            .changed();
                        ui.end_row();

                        ui.label("Edge fill reach");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.exterior_fill_reach, 0.0..=1.0).step_by(0.05).clamping(egui::SliderClamping::Edits).max_decimals(2))
                            .on_hover_text("How far under an outline on the figure's edge the fill may reach: 0 stops at the line's inner edge, 0.5 at its centre, 1 at its outer edge.")
                            .changed();
                        ui.end_row();

                        ui.label("Soft edge as background");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.fringe_px, 0.0..=4.0).step_by(0.1).suffix(" px").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("Width (px) of the segmenter's soft edge outside the outline that is treated as background; 0 keeps it as fill.")
                            .changed();
                        ui.end_row();

                        ui.label("Paper pockets: colour");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.background_pocket_de, 0.0..=30.0).step_by(0.5).suffix(" ΔE").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("How close (ΔE) to the sheet's background colour a patch inside the figure must be to count as paper showing through (between hair curls or body-hair strands); 0 keeps every patch.")
                            .changed();
                        ui.end_row();

                        ui.label("Paper pockets: reach");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.background_pocket_reach_px, 0.0..=40.0).step_by(0.5).suffix(" px").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("A paper-coloured patch lying wholly within this distance (px) of the figure's edge, or touching the outside, is left out as background.")
                            .changed();
                        ui.end_row();

                        ui.label("Paper pockets: largest");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.background_pocket_max_px2, 0..=10000).suffix(" px²").clamping(egui::SliderClamping::Edits))
                            .on_hover_text("Paper-coloured patches larger than this (px²) are always kept: a white shirt, a vest, a dhoti.")
                            .changed();
                        ui.end_row();

                        ui.label("Ink-paper rims: width");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.blend_rim_px, 0.0..=8.0).step_by(0.5).suffix(" px").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("Width (px) of the edge band where a colour between the outline's ink and the paper (the source's anti-aliasing, traced as grey dashes along hair tips) is left out as background; 0 keeps it.")
                            .changed();
                        ui.end_row();

                        ui.label("Ink-paper rims: colour");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.blend_rim_de, 0.0..=30.0).step_by(0.5).suffix(" ΔE").clamping(egui::SliderClamping::Edits).max_decimals(1))
                            .on_hover_text("How far (ΔE) off the ink-to-paper blend an edge pixel may be and still count as the outline's anti-aliasing against the paper.")
                            .changed();
                        ui.end_row();

                        ui.label("Ink-paper rims: lightness");
                        changed |= ui
                            .add(egui::Slider::new(&mut colour.blend_rim_min_t, 0.0..=1.0).step_by(0.05).clamping(egui::SliderClamping::Edits).max_decimals(2))
                            .on_hover_text("How far towards the paper (0 = the ink, 1 = the paper) an edge pixel's colour must be to count as the outline's anti-aliasing; darker stays with the ink.")
                            .changed();
                        ui.end_row();
                    });
            });
        }
        changed |= tuning_buttons(ui, tuning);
    });
    changed
}

/// The row under the tuning sliders. **Save as default** writes
/// [`TraceTuning::path`], which both trace dialogs start from; **Reset** goes
/// back to [`TraceTuning::default`]; **Load file…** reads a tuning file picked
/// in the OS dialog, which runs on a thread of its own (a blocking dialog has
/// no place on the UI thread). True when `tuning` changed.
fn tuning_buttons(ui: &mut Ui, tuning: &mut TraceTuning) -> bool {
    let mut changed = false;
    let load_id = ui.id().with("trace-tuning-load");
    let note_id = ui.id().with("trace-tuning-note");

    // The file "Load file…" picked, once the OS dialog has closed.
    let pending: Option<Receiver<Option<PathBuf>>> = ui.data(|d| d.get_temp(load_id));
    let mut picking = pending.is_some();
    if let Some(picked) = pending.and_then(|rx| rx.try_recv().ok()) {
        picking = false;
        ui.data_mut(|d| d.remove::<Receiver<Option<PathBuf>>>(load_id));
        if let Some(path) = picked {
            let note = match TraceTuning::load(&path) {
                Ok(loaded) => {
                    *tuning = loaded;
                    changed = true;
                    format!("Loaded {}", path.display())
                }
                Err(e) => e,
            };
            ui.data_mut(|d| d.insert_temp(note_id, note));
        }
    }

    ui.horizontal(|ui| {
        if ui
            .button("Save as default")
            .on_hover_ui(|ui| {
                ui.label(format!(
                    "Write these values to {}; both trace dialogs start from them",
                    TraceTuning::path().display()
                ));
            })
            .clicked()
        {
            let note = match tuning.save() {
                Ok(()) => format!("Saved to {}", TraceTuning::path().display()),
                Err(e) => format!("Could not save: {e}"),
            };
            ui.data_mut(|d| d.insert_temp(note_id, note));
        }
        if ui.button("Reset").on_hover_text("Back to the built-in values").clicked() {
            *tuning = TraceTuning::default();
            changed = true;
        }
        if ui
            .add_enabled(!picking, egui::Button::new(if picking { "Loading…" } else { "Load file…" }))
            .on_hover_text("Read a saved tuning file into these sliders")
            .clicked()
        {
            let (tx, rx) = crossbeam_channel::bounded(1);
            ui.data_mut(|d| d.insert_temp(load_id, rx));
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let picked = rfd::FileDialog::new().add_filter("Trace tuning", &["json"]).pick_file();
                let _ = tx.send(picked);
                ctx.request_repaint();
            });
        }
    });
    if let Some(note) = ui.data(|d| d.get_temp::<String>(note_id)) {
        ui.label(note);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sliders and combo boxes `tuning_section` puts on screen with every
    /// header open, counted from the accessibility tree.
    fn widgets(show_colour: bool) -> (usize, usize) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut tuning = TraceTuning::default();
        let out = ctx.run_ui(Default::default(), |ui| {
            tuning_section_open(ui, &mut tuning, show_colour, Some(true));
        });
        assert_eq!(tuning, TraceTuning::default(), "drawing the section changed a value");
        let update = out.platform_output.accesskit_update.expect("accesskit is on");
        let count = |role| update.nodes.iter().filter(|(_, n)| n.role() == role).count();
        (count(egui::accesskit::Role::Slider), count(egui::accesskit::Role::ComboBox))
    }

    /// Every `TraceTuning` field has its control: one slider each, bar
    /// `upsample`, which is a combo box. The fields are counted from the
    /// serialised defaults, so a knob added without a slider fails here.
    #[test]
    fn tuning_section_lists_every_field() {
        let json = serde_json::to_value(TraceTuning::default()).unwrap();
        let ink_fields = json["ink"].as_object().unwrap().len();
        let colour_fields = json["colour"].as_object().unwrap().len();

        let (sliders, combos) = widgets(true);
        assert_eq!(sliders, ink_fields - 1 + colour_fields);
        assert_eq!(combos, 1);

        let (sliders, combos) = widgets(false);
        assert_eq!(sliders, ink_fields - 1, "the colour knobs show only with colour");
        assert_eq!(combos, 1);
    }
}
