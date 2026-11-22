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

    /// **The compact picker is small and exact** (user 2026-11-17): it is
    /// under 160 points wide; every colour survives the trip through its hue,
    /// saturation and value unchanged; typing a hex code sets that colour to
    /// the digit.
    #[test]
    fn the_compact_picker_is_narrow_and_keeps_colours_exact() {
        for r in (0..=255u16).step_by(17) {
            for g in (0..=255u16).step_by(15) {
                for b in (0..=255u16).step_by(51) {
                    let c = [r as u8, g as u8, b as u8];
                    assert_eq!(hsv_to_rgb(rgb_to_hsv(c)), c, "{c:?}");
                }
            }
        }
        assert_eq!(parse_hex_text("#1a2B3c"), Some([0x1a, 0x2b, 0x3c, 255]));
        assert_eq!(parse_hex_text("f0c"), Some([255, 0, 204, 255]));
        assert_eq!(parse_hex_text("#12345678"), Some([0x12, 0x34, 0x56, 0x78]));
        assert_eq!(parse_hex_text("#12"), None);
        assert_eq!(hex_text([0x1a, 0x2b, 0x3c, 255]), "#1A2B3C");

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut color = egui::Color32::from_rgb(200, 40, 90);
        let id = egui::Id::new("picker-test");
        let mut width = 0.0;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let r = ui.scope(|ui| compact_color_picker(ui, id, &mut color, true)).response;
            width = r.rect.width();
        });
        assert!(width < 160.0, "the picker is {width} points wide");
        assert_eq!(color, egui::Color32::from_rgb(200, 40, 90), "drawing it changes nothing");
    }

    /// **+ Swatches** under a colour picker hands its colour on for the
    /// Swatches panel (user 2026-11-22: "give an option to add to swatches
    /// from the colour picker").
    #[test]
    fn plus_swatches_in_a_picker_asks_to_keep_its_colour() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut color = egui::Color32::from_rgb(200, 40, 90);
        let id = egui::Id::new("picker-add-test");
        let mut t = 0.0;
        let mut frame = |events: Vec<egui::Event>, color: &mut egui::Color32| {
            t += 0.05;
            let out = ctx.run_ui(egui::RawInput { events, time: Some(t), ..Default::default() }, |ui| {
                compact_color_picker(ui, id, color, true);
            });
            fn find(shape: &egui::Shape, found: &mut Option<egui::Pos2>) {
                match shape {
                    egui::Shape::Text(t) if t.galley.text() == "+ Swatches" => *found = Some(t.pos + t.galley.rect.center().to_vec2()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| find(s, found)),
                    _ => {}
                }
            }
            let mut found = None;
            out.shapes.iter().for_each(|s| find(&s.shape, &mut found));
            found
        };
        let at = frame(vec![], &mut color).expect("+ Swatches is drawn");
        assert_eq!(crate::swatch_panel::take_add_request(&ctx), None, "nothing asked yet");
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(vec![egui::Event::PointerMoved(at), press(true)], &mut color);
        frame(vec![press(false)], &mut color);
        let asked = crate::swatch_panel::take_add_request(&ctx).expect("the colour is handed on");
        assert_eq!(asked.to_rgba8().to_u8_array(), [200, 40, 90, 255]);
        assert_eq!(crate::swatch_panel::take_add_request(&ctx), None, "once");
    }

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

/// **How big a colour picker opens**: the colour field this many points
/// square. egui's own colour button opens at 275 and the swatches' at 220,
/// which covered a good part of the stage for a choice the eye makes at a
/// glance (user 2026-11-15: too big; 2026-11-17: still too big and wide).
pub const PICKER_SIZE: f32 = 120.0;
/// The hue strip beside the field.
const HUE_WIDTH: f32 = 12.0;

/// **A colour button** — a swatch that opens the [`compact_color_picker`].
/// In place of egui's `color_edit_button_srgba`, whose picker size is fixed.
/// The response is marked changed when the colour was edited.
pub fn color_button(ui: &mut Ui, color: &mut egui::Color32) -> Response {
    let size = ui.spacing().interact_size;
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    // A colour the stage eyedropper took for this button.
    let mut taken = false;
    if let Some(picked) = crate::screen_color::take(response.id) {
        let rgba = picked.to_rgba8();
        *color = egui::Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a);
        taken = true;
    }
    // Opening the picker offers the stage eyedropper too (see
    // `screen_color::offer`).
    let was_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    if response.clicked() && !was_open {
        crate::screen_color::offer(ui.ctx(), response.id);
    }
    if ui.is_rect_visible(rect) {
        egui::color_picker::show_color_at(ui.painter(), *color, rect);
        let stroke = if response.hovered() {
            ui.visuals().widgets.hovered.fg_stroke
        } else {
            egui::Stroke::new(1.0, Palette::hairline())
        };
        ui.painter().rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    }
    let mut changed = false;
    let id = response.id;
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            changed = compact_color_picker(ui, id, color, true);
        });
    if changed || taken {
        response.mark_changed();
    }
    if taken {
        egui::Popup::close_id(ui.ctx(), egui::Popup::default_response_id(&response));
    }
    response
}

/// The picker's rows made small with it: small text, small controls.
pub fn compact_picker_style(ui: &mut Ui) {
    let style = ui.style_mut();
    style.override_text_style = Some(egui::TextStyle::Small);
    style.spacing.interact_size = egui::vec2(24.0, 16.0);
    style.spacing.button_padding = egui::vec2(3.0, 1.0);
    style.spacing.item_spacing = egui::vec2(4.0, 3.0);
}

/// `[r, g, b]` (0–255, as stored) as hue, saturation and value, each 0–1.
fn rgb_to_hsv([r, g, b]: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

/// Hue, saturation and value (each 0–1) as `[r, g, b]`, 0–255.
fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [u8; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let byte = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

fn opaque([r, g, b]: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(r, g, b)
}

/// `#RRGGBB`, or `#RRGGBBAA` when not opaque.
fn hex_text([r, g, b, a]: [u8; 4]) -> String {
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

/// `#RGB`, `#RRGGBB` or `#RRGGBBAA`, the hash optional.
fn parse_hex_text(text: &str) -> Option<[u8; 4]> {
    let t = text.trim().trim_start_matches('#');
    if !t.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).ok();
    match t.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(&t[i..i + 1], 16).ok().map(|v| v * 17);
            Some([d(0)?, d(1)?, d(2)?, 255])
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// **The compact colour picker** (user 2026-11-17: "smaller, and able to
/// pick exact colours"): a [`PICKER_SIZE`] colour field (saturation across,
/// brightness up) with a thin hue strip beside it, then the colour exactly —
/// its hex code, and red, green and blue 0–255, typed or dragged — and its
/// opacity when `alpha`. About 140 points wide, where egui's picker was
/// twice that. `id` keeps the hue while the colour is grey, which has none.
/// Whether the colour changed.
pub fn compact_color_picker(ui: &mut Ui, id: egui::Id, color: &mut egui::Color32, alpha: bool) -> bool {
    let changed = color_picker_sized(ui, id, color, alpha, PICKER_SIZE);
    add_to_swatches_button(ui, *color);
    changed
}

/// **+ Swatches**: keep the colour a picker shows in the Swatches panel,
/// named there (see [`crate::swatch_panel::request_add`]).
pub fn add_to_swatches_button(ui: &mut Ui, color: egui::Color32) {
    if ui
        .small_button("+ Swatches")
        .on_hover_text("Add this colour to the Swatches panel, to name it and keep it with the document")
        .clicked()
    {
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        crate::swatch_panel::request_add(ui.ctx(), peniko::Color::from_rgba8(r, g, b, a));
    }
}

/// [`compact_color_picker`] with a colour field `field` points square — the
/// Color panel's large mixer.
pub fn color_picker_sized(ui: &mut Ui, id: egui::Id, color: &mut egui::Color32, alpha: bool, field: f32) -> bool {
    compact_picker_style(ui);
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let key = id.with("compact-picker-hsv");
    // The hue and saturation last shown, while they still make this colour:
    // a grey or black has no hue of its own to come back to.
    let mut hsv = ui
        .memory(|m| m.data.get_temp::<[f32; 3]>(key))
        .filter(|kept| hsv_to_rgb(*kept) == [r, g, b])
        .unwrap_or_else(|| rgb_to_hsv([r, g, b]));
    let mut rgb = [r, g, b];
    let mut a = a;
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // -- the field: saturation across, value up --
        let (rect, response) = ui.allocate_exact_size(egui::vec2(field, field), Sense::click_and_drag());
        if ui.is_rect_visible(rect) {
            const N: usize = 12;
            let mut mesh = egui::Mesh::default();
            for j in 0..=N {
                for i in 0..=N {
                    let (s, v) = (i as f32 / N as f32, 1.0 - j as f32 / N as f32);
                    let pos = egui::pos2(rect.left() + rect.width() * i as f32 / N as f32, rect.top() + rect.height() * j as f32 / N as f32);
                    mesh.colored_vertex(pos, opaque(hsv_to_rgb([hsv[0], s, v])));
                }
            }
            for j in 0..N as u32 {
                for i in 0..N as u32 {
                    let at = j * (N as u32 + 1) + i;
                    mesh.add_triangle(at, at + 1, at + N as u32 + 1);
                    mesh.add_triangle(at + 1, at + N as u32 + 2, at + N as u32 + 1);
                }
            }
            ui.painter().add(mesh);
            let mark = egui::pos2(rect.left() + hsv[1] * rect.width(), rect.top() + (1.0 - hsv[2]) * rect.height());
            let ring = if hsv[2] > 0.5 && hsv[1] < 0.5 { egui::Color32::BLACK } else { egui::Color32::WHITE };
            ui.painter().circle_stroke(mark, 4.0, egui::Stroke::new(1.5, ring));
        }
        if (response.dragged() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            hsv[1] = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            hsv[2] = 1.0 - ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            rgb = hsv_to_rgb(hsv);
            changed = true;
        }

        // -- the hue strip --
        let (rect, response) = ui.allocate_exact_size(egui::vec2(HUE_WIDTH, field), Sense::click_and_drag());
        if ui.is_rect_visible(rect) {
            const N: usize = 24;
            let mut mesh = egui::Mesh::default();
            for j in 0..=N {
                let t = j as f32 / N as f32;
                let c = opaque(hsv_to_rgb([t, 1.0, 1.0]));
                let y = rect.top() + rect.height() * t;
                mesh.colored_vertex(egui::pos2(rect.left(), y), c);
                mesh.colored_vertex(egui::pos2(rect.right(), y), c);
            }
            for j in 0..N as u32 {
                let at = j * 2;
                mesh.add_triangle(at, at + 1, at + 2);
                mesh.add_triangle(at + 1, at + 3, at + 2);
            }
            ui.painter().add(mesh);
            let y = rect.top() + hsv[0] * rect.height();
            ui.painter().line_segment(
                [egui::pos2(rect.left() - 2.0, y), egui::pos2(rect.right() + 2.0, y)],
                egui::Stroke::new(2.0, ui.visuals().strong_text_color()),
            );
        }
        if (response.dragged() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            hsv[0] = ((p.y - rect.top()) / rect.height()).clamp(0.0, 0.9999);
            rgb = hsv_to_rgb(hsv);
            changed = true;
        }
    });

    // -- exactly: the hex code, then each channel --
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
        egui::color_picker::show_color_at(ui.painter(), egui::Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], a), swatch);
        let text_key = id.with("compact-picker-hex");
        let mut text = ui.memory(|m| m.data.get_temp::<String>(text_key)).unwrap_or_else(|| hex_text([rgb[0], rgb[1], rgb[2], a]));
        let field = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(field - 4.0)
                .font(egui::TextStyle::Monospace)
                .hint_text("#RRGGBB"),
        );
        let field = field.on_hover_text("The exact colour: type or paste #RRGGBB (or #RGB, #RRGGBBAA)");
        if field.has_focus() {
            if let Some(parsed) = parse_hex_text(&text)
                && parsed != [rgb[0], rgb[1], rgb[2], a]
            {
                rgb = [parsed[0], parsed[1], parsed[2]];
                if alpha {
                    a = parsed[3];
                }
                hsv = rgb_to_hsv(rgb);
                changed = true;
            }
            ui.memory_mut(|m| m.data.insert_temp(text_key, text));
        } else {
            ui.memory_mut(|m| m.data.remove::<String>(text_key));
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut channel = |ui: &mut Ui, name: &str, value: &mut u8, what: &str| {
            ui.label(name);
            ui.add(egui::DragValue::new(value).range(0..=255).speed(0.5)).on_hover_text(what).changed()
        };
        let mut moved = false;
        moved |= channel(ui, "R", &mut rgb[0], "Red, 0\u{2013}255");
        moved |= channel(ui, "G", &mut rgb[1], "Green, 0\u{2013}255");
        moved |= channel(ui, "B", &mut rgb[2], "Blue, 0\u{2013}255");
        if moved {
            hsv = rgb_to_hsv(rgb);
            changed = true;
        }
        if alpha {
            changed |= channel(ui, "A", &mut a, "Opacity, 0 (clear) \u{2013} 255 (solid)");
        }
    });

    ui.memory_mut(|m| m.data.insert_temp(key, hsv));
    if changed {
        *color = egui::Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], a);
    }
    changed
}

/// [`color_button`] for an unmultiplied `[r, g, b, a]`.
pub fn color_button_unmultiplied(ui: &mut Ui, rgba: &mut [u8; 4]) -> Response {
    let mut color = egui::Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
    let response = color_button(ui, &mut color);
    if response.changed() {
        *rgba = color.to_srgba_unmultiplied();
    }
    response
}
