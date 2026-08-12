//! Phase 0 diagnostics overlay.
//!
//! This exists to make the three headline claims *visible* rather than merely
//! asserted in a test: which GPU is actually in use, how deep the zoom really
//! goes, and whether the cores are genuinely working.

use buzz_geom::Camera;
use buzz_jobs::Utilisation;

/// Everything the overlay displays for one frame.
pub struct HudState {
    pub adapter: String,
    pub backend: String,
    pub frame_ms: f32,
    pub interactive_threads: usize,
    pub background_threads: usize,
    pub utilisation: Option<Utilisation>,
    pub generation: usize,
    pub generations: usize,
    pub scene_items: usize,
    /// Items encoded last frame.
    pub drawn: usize,
    /// Items skipped last frame (off-screen, sub-pixel, or too large).
    pub culled: usize,
    /// Detail generations actually on screen, e.g. `"9–11"`.
    pub visible_generations: String,
}

/// What the user asked for by clicking something.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HudActions {
    pub reset_view: bool,
    pub fit_view: bool,
    pub stress_cores: bool,
    /// Jump straight to a zoom level, as a percentage.
    pub goto_zoom_percent: Option<u64>,
}

/// Format a zoom percentage that may span twelve orders of magnitude.
///
/// A plain `{:.0}%` becomes unreadable past a few thousand, which is fine for
/// Animate and useless here.
pub fn format_zoom(percent: f64) -> String {
    if !percent.is_finite() {
        return "—".to_string();
    }
    if percent < 0.01 {
        format!("{percent:.2e} %")
    } else if percent < 10_000.0 {
        format!("{percent:.1} %")
    } else if percent < 1e6 {
        format!("{:.0} %", percent)
    } else {
        format!("{percent:.3e} %")
    }
}

/// Describe the precision budget in words the user can act on.
pub fn describe_precision(px: f64) -> (String, egui::Color32) {
    if px < 0.01 {
        ("exact".to_string(), egui::Color32::from_rgb(0x5B, 0xD6, 0x8A))
    } else if px < 0.25 {
        (
            format!("{px:.3} px steps"),
            egui::Color32::from_rgb(0xE8, 0xC8, 0x60),
        )
    } else {
        (
            format!("{px:.2} px steps"),
            egui::Color32::from_rgb(0xE0, 0x7B, 0x5B),
        )
    }
}

/// Draw the overlay. Returns whatever the user clicked.
pub fn draw(ctx: &egui::Context, camera: &Camera, state: &HudState) -> HudActions {
    let mut actions = HudActions::default();

    egui::Window::new("BuzzAnimate — Phase 0")
        .default_pos([16.0, 16.0])
        .resizable(false)
        .show(ctx, |ui| {
            ui.style_mut().spacing.item_spacing.y = 6.0;

            // ---- GPU -------------------------------------------------
            ui.heading("GPU");
            ui.label(
                egui::RichText::new(&state.adapter)
                    .strong()
                    .color(egui::Color32::from_rgb(0x76, 0xD1, 0x7C)),
            );
            ui.label(format!("backend: {}", state.backend));
            ui.label(format!(
                "frame: {:.2} ms  ({:.0} fps)",
                state.frame_ms,
                if state.frame_ms > 0.0 {
                    1000.0 / state.frame_ms
                } else {
                    0.0
                }
            ));

            ui.separator();

            // ---- Zoom ------------------------------------------------
            ui.heading("Zoom");
            ui.label(
                egui::RichText::new(format_zoom(camera.zoom_percent()))
                    .size(22.0)
                    .strong(),
            );

            let over = camera.zoom_percent() / 2000.0;
            if over > 1.0 {
                ui.label(
                    egui::RichText::new(format!("{over:.3e}× past Animate's 2000% cap"))
                        .color(egui::Color32::from_rgb(0x8A, 0xC6, 0xFF)),
                );
            }

            let (precision, colour) = describe_precision(camera.screen_precision_px());
            ui.label(egui::RichText::new(format!("precision: {precision}")).color(colour));
            ui.label(format!(
                "detail generation {} of {}",
                state.generation, state.generations
            ));
            ui.label(
                egui::RichText::new(format!(
                    "on screen: generations {}",
                    state.visible_generations
                ))
                .weak(),
            );

            ui.horizontal(|ui| {
                if ui.button("Reset").clicked() {
                    actions.reset_view = true;
                }
                if ui.button("Fit").clicked() {
                    actions.fit_view = true;
                }
            });
            ui.horizontal(|ui| {
                for (label, pct) in [
                    ("2 000%", 2_000u64),
                    ("1e6", 1_000_000),
                    ("1e9", 1_000_000_000),
                    ("1e12", 1_000_000_000_000),
                ] {
                    if ui.button(label).clicked() {
                        actions.goto_zoom_percent = Some(pct);
                    }
                }
            });

            ui.separator();

            // ---- CPU -------------------------------------------------
            ui.heading("CPU");
            ui.label(format!(
                "{} interactive + {} background threads",
                state.interactive_threads, state.background_threads
            ));

            match &state.utilisation {
                Some(u) if u.available => {
                    ui.label(format!("{:.1} cores busy", u.cores_busy));
                    draw_core_bars(ui, &u.per_worker);
                }
                _ => {
                    ui.label(
                        egui::RichText::new("per-thread CPU metrics unavailable")
                            .italics()
                            .weak(),
                    );
                }
            }

            if ui.button("Stress all cores").clicked() {
                actions.stress_cores = true;
            }

            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{} items · {} drawn · {} culled",
                    state.scene_items, state.drawn, state.culled
                ))
                .weak()
                .small(),
            );
            ui.label(
                egui::RichText::new("wheel = zoom at cursor · drag = pan · R = reset")
                    .weak()
                    .small(),
            );
        });

    actions
}

/// One small bar per worker thread.
fn draw_core_bars(ui: &mut egui::Ui, per_worker: &[f64]) {
    const BAR_W: f32 = 9.0;
    const BAR_H: f32 = 34.0;
    const GAP: f32 = 2.0;

    let count = per_worker.len().max(1);
    let width = count as f32 * (BAR_W + GAP);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, BAR_H), egui::Sense::hover());
    let painter = ui.painter();

    for (i, load) in per_worker.iter().enumerate() {
        let load = load.clamp(0.0, 1.0) as f32;
        let x = rect.left() + i as f32 * (BAR_W + GAP);

        let track = egui::Rect::from_min_size(
            egui::pos2(x, rect.top()),
            egui::vec2(BAR_W, BAR_H),
        );
        painter.rect_filled(track, 1.0, egui::Color32::from_gray(38));

        if load > 0.0 {
            let h = (BAR_H * load).max(1.0);
            let filled = egui::Rect::from_min_size(
                egui::pos2(x, rect.bottom() - h),
                egui::vec2(BAR_W, h),
            );
            // Green through amber to red as a core saturates.
            let colour = egui::Color32::from_rgb(
                (70.0 + 170.0 * load) as u8,
                (210.0 - 90.0 * load) as u8,
                90,
            );
            painter.rect_filled(filled, 1.0, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_formatting_stays_readable_across_the_whole_range() {
        assert_eq!(format_zoom(100.0), "100.0 %");
        assert_eq!(format_zoom(2000.0), "2000.0 %");
        // Past a few thousand a plain decimal becomes unreadable, so it
        // switches representation rather than printing 13 digits.
        assert!(format_zoom(1e12).contains('e'));
        assert!(format_zoom(1e-3).contains('e'));
        assert_eq!(format_zoom(f64::NAN), "—");
        assert_eq!(format_zoom(f64::INFINITY), "—");
    }

    #[test]
    fn precision_is_reported_as_exact_until_it_actually_matters() {
        assert_eq!(describe_precision(1e-9).0, "exact");
        assert!(describe_precision(0.05).0.contains("px"));
        assert!(describe_precision(2.0).0.contains("px"));
    }

    #[test]
    fn precision_wording_changes_colour_as_it_degrades() {
        let good = describe_precision(1e-9).1;
        let warn = describe_precision(0.1).1;
        let bad = describe_precision(5.0).1;
        assert_ne!(good, warn);
        assert_ne!(warn, bad);
    }
}
