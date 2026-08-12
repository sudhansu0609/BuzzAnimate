//! Visual theme.
//!
//! Animate ships a dark interface with a mid-grey pasteboard and a white stage,
//! and creative tools generally follow that convention: a neutral surround
//! keeps the artwork's own colours readable. These values are chosen to match
//! that feel without copying Adobe's assets — colours and metrics only, no
//! icons or artwork.

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// Panel and chrome colours.
pub struct Palette;

impl Palette {
    /// Window chrome behind the panels.
    pub const CHROME: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
    /// Panel interiors.
    pub const PANEL: Color32 = Color32::from_rgb(0x2F, 0x2F, 0x2F);
    /// Slightly raised surfaces: toolbar buttons, headers.
    pub const RAISED: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
    /// Hovered control.
    pub const HOVER: Color32 = Color32::from_rgb(0x4A, 0x4A, 0x4A);
    /// Active or selected control.
    pub const ACTIVE: Color32 = Color32::from_rgb(0x2D, 0x6C, 0xB5);
    /// Separators and panel edges.
    pub const BORDER: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1C);

    pub const TEXT: Color32 = Color32::from_rgb(0xD8, 0xD8, 0xD8);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9A);

    /// The work area surrounding the stage. Objects here are editable but do
    /// not appear in published output.
    pub const PASTEBOARD: Color32 = Color32::from_rgb(0x53, 0x53, 0x53);
    /// Edge of the stage rectangle.
    pub const STAGE_BORDER: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);

    /// Rulers and their markings.
    pub const RULER_BG: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
    pub const RULER_TICK: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
    pub const RULER_TEXT: Color32 = Color32::from_rgb(0xAA, 0xAA, 0xAA);

    /// Guides dragged off the rulers. Animate uses cyan-green.
    pub const GUIDE: Color32 = Color32::from_rgb(0x00, 0xD0, 0xC8);
    pub const GUIDE_LOCKED: Color32 = Color32::from_rgb(0x8A, 0x8A, 0x60);
    /// The drawing grid.
    pub const GRID: Color32 = Color32::from_rgba_premultiplied(0x60, 0x60, 0x60, 0x60);

    /// Selection marquee and handles.
    pub const SELECTION: Color32 = Color32::from_rgb(0x00, 0xA8, 0xFF);
    pub const HANDLE_FILL: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    pub const HANDLE_STROKE: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);

    /// A snap indicator, shown while a drag is snapping to something.
    pub const SNAP: Color32 = Color32::from_rgb(0xFF, 0x40, 0x80);
}

/// Metrics shared across the chrome.
pub struct Metrics;

impl Metrics {
    /// Width of the ruler strips, in points.
    pub const RULER: f32 = 18.0;
    /// Toolbar button edge.
    pub const TOOL_BUTTON: f32 = 30.0;
    /// Timeline row height at 100%.
    pub const LAYER_ROW: f32 = 20.0;
    /// Frame cell width in the timeline.
    pub const FRAME_WIDTH: f32 = 12.0;
}

/// Apply the theme to an egui context.
///
/// The same dark palette is installed for *both* egui theme slots. BuzzAnimate
/// is a dark-only application, and leaving the light slot untouched would give
/// a half-styled window to anyone whose system theme is light.
pub fn apply(ctx: &egui::Context) {
    let visuals = build_visuals();
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.interact_size.y = 20.0;
    });
}

fn build_visuals() -> Visuals {
    let mut visuals = Visuals::dark();

    visuals.panel_fill = Palette::PANEL;
    visuals.window_fill = Palette::PANEL;
    visuals.extreme_bg_color = Palette::CHROME;
    visuals.faint_bg_color = Palette::RAISED;
    visuals.override_text_color = Some(Palette::TEXT);

    visuals.widgets.noninteractive.bg_fill = Palette::PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = Palette::PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Palette::BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Palette::TEXT_DIM);

    visuals.widgets.inactive.bg_fill = Palette::RAISED;
    visuals.widgets.inactive.weak_bg_fill = Palette::RAISED;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Palette::TEXT);

    visuals.widgets.hovered.bg_fill = Palette::HOVER;
    visuals.widgets.hovered.weak_bg_fill = Palette::HOVER;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    visuals.widgets.active.bg_fill = Palette::ACTIVE;
    visuals.widgets.active.weak_bg_fill = Palette::ACTIVE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

    visuals.selection.bg_fill = Palette::ACTIVE;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    // Square-ish corners read as a professional tool rather than a web app.
    let radius = CornerRadius::same(2);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    visuals.window_corner_radius = radius;

    visuals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_applies_to_both_theme_slots() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            assert_eq!(
                ctx.style_of(theme).visuals.panel_fill,
                Palette::PANEL,
                "{theme:?} slot was left unstyled"
            );
        }
    }

    /// The pasteboard must be clearly distinct from a white stage, or the
    /// boundary of the exported area becomes invisible.
    #[test]
    fn the_pasteboard_contrasts_with_the_stage() {
        let p = Palette::PASTEBOARD;
        let luminance = 0.299 * p.r() as f32 + 0.587 * p.g() as f32 + 0.114 * p.b() as f32;
        assert!(
            (40.0..180.0).contains(&luminance),
            "the pasteboard should be mid-grey, got luminance {luminance}"
        );
    }

    #[test]
    fn text_is_readable_against_panels() {
        let bg = Palette::PANEL;
        let fg = Palette::TEXT;
        let lum = |c: Color32| 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
        assert!(
            (lum(fg) - lum(bg)).abs() > 80.0,
            "insufficient contrast between text and panel background"
        );
    }
}
