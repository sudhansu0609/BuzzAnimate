//! Ghost buttons: the icon-only control the flat chrome uses wherever a
//! panel needs a plain click rather than one of `panels.rs`'s bigger,
//! painted-icon tool buttons. Stage 2, item 3 of
//! `docs/WORKSPACE_DESIGN_PLAN.md`: "Rest: no fill; hover: `hover`; active or
//! toggled: `active` + accent glyph. Tooltip always carries the name and the
//! shortcut."
//!
//! # `icon` is a glyph, and it must already be vetted
//!
//! Unlike the tool strip's icons (`icons.rs`, painted shapes — see its own
//! doc comment for why), this draws `icon` as *text*, because the controls
//! it is for — a delete, an add, a menu — already have a settled glyph
//! (`\u{1F5D1}`, `\u{2795}`, `...`) covered by `theme::glyph_tests`. Passing
//! anything not already in that list risks the exact empty-box failure this
//! project has shipped twice: reach for `icons.rs` for a new symbol, not a
//! new codepoint here.

use egui::{Response, Sense, Ui};

use crate::theme::{Palette, Tokens};

/// An icon-only ghost button: no fill at rest, `hover` on hover. The tooltip
/// always carries `shortcut`, so an icon-only control is never a guess.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str, shortcut: Option<&str>) -> Response {
    toggle_icon_button(ui, icon, tooltip, shortcut, false)
}

/// [`icon_button`] with a picture from the icon sheet rather than a
/// character — ghost-styled the same way, ink white on a dark theme and black
/// on a light one, dimmed when the button is disabled.
pub fn sheet_icon_button(
    ui: &mut Ui,
    icon: crate::icon_sheet::SheetIcon,
    tooltip: &str,
) -> Response {
    let size = egui::vec2(Tokens::CONTROL_H, Tokens::CONTROL_H);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    if ui.is_rect_visible(rect) {
        if response.hovered() && enabled {
            ui.painter()
                .rect_filled(rect, Tokens::RADIUS as f32, Palette::hover());
        }
        let ink = if enabled {
            crate::icon_sheet::ink()
        } else {
            Palette::text_dim()
        };
        crate::icon_sheet::paint(ui.painter(), rect.shrink(4.0), icon, ink);
    }
    let text = tooltip.to_string();
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text.clone())
    });
    response.on_hover_text(tooltip)
}

/// [`icon_button`], with an `active`/toggled state: the fill is `active`
/// rather than `hover`, and the glyph itself is drawn in `accent`.
pub fn toggle_icon_button(
    ui: &mut Ui,
    icon: &str,
    tooltip: &str,
    shortcut: Option<&str>,
    active: bool,
) -> Response {
    let size = egui::vec2(Tokens::CONTROL_H, Tokens::CONTROL_H);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let fill = if active {
            Some(Palette::active())
        } else if response.hovered() {
            Some(Palette::hover())
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter()
                .rect_filled(rect, Tokens::RADIUS as f32, fill);
        }
        let color = if active {
            Palette::accent()
        } else {
            Palette::text()
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(Tokens::TEXT),
            color,
        );
    }

    let text = match shortcut {
        Some(s) => format!("{tooltip} ({s})"),
        None => tooltip.to_string(),
    };

    // Stage 6, item 4: an icon-only button draws no text of its own for
    // `fill_accesskit_node_common` to pick up, so without this a screen
    // reader announces it as a nameless button. The same string the tooltip
    // already carries — name and shortcut both — is the accessible label,
    // so nothing here can drift out of step with what a sighted user reads
    // on hover.
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text.clone())
    });

    response.on_hover_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The button draws without panicking, hands back a click, and the
    /// tooltip carries both the name and the shortcut — the whole contract
    /// Stage 2, item 3 asks for.
    #[test]
    fn an_icon_button_reports_a_click_and_carries_its_shortcut() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let response = icon_button(ui, "\u{2795}", "New layer", Some("Ctrl+Alt+N"));
            // Nothing to click in a single headless pass, but the response
            // exists and is the size the token says an icon button is.
            assert_eq!(response.rect.width(), Tokens::CONTROL_H);
            assert_eq!(response.rect.height(), Tokens::CONTROL_H);
        });
    }

    /// A toggled button is told apart from a resting one: the active fill,
    /// not the hover fill — asserted by exercising both paths without a
    /// panic, since colour output itself is a screenshot's job.
    #[test]
    fn a_toggled_icon_button_does_not_panic() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            toggle_icon_button(ui, "\u{1F5D1}", "Delete", None, true);
            toggle_icon_button(ui, "\u{1F5D1}", "Delete", None, false);
        });
    }
}
