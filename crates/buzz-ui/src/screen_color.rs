//! **Picking a colour for a swatch**, and the system's own colour panel.
//!
//! [`pick`] starts the stage eyedropper for one swatch: the shell turns the
//! pointer into the dropper, and the colour of the artwork under its tip goes
//! to that swatch ([`deliver`], then [`take`]). It used to start macOS's
//! screen sampler, whose large magnifying circle hid what was being picked
//! (user 2026-11-17: "it shall be in the shape of an eyedropper"); a colour
//! from another program is still to be had from the Colors window's own
//! sampler.
//!
//! [`open_panel`] shows the system Colors window (wheel, sliders, palettes,
//! its own loupe) linked to one swatch; [`poll_panel`] reads it back each
//! frame while it is up.
//!
//! On other platforms neither exists and [`available`] says so, so the
//! buttons are simply not drawn.

use egui::Id;
use peniko::Color;

/// Whether this platform has a screen sampler and a colour panel.
pub fn available() -> bool {
    cfg!(target_os = "macos")
}

/// Start the stage eyedropper for the swatch `target`. The pick, if one is
/// made, is collected with [`take`].
///
/// Asked of this window's own context, not of a slot every window shares: a
/// request left in a shared slot was taken by whichever window looked first.
pub fn pick(ctx: &egui::Context, target: Id) {
    ctx.data_mut(|d| d.insert_temp(dropper_key(), Some(target)));
    ctx.request_repaint();
}

/// The swatch that asked this window for the stage eyedropper, once — for
/// the shell, which runs the dropper.
pub fn dropper_request(ctx: &egui::Context) -> Option<Id> {
    ctx.data_mut(|d| d.remove_temp::<Option<Id>>(dropper_key())).flatten()
}

/// **A swatch's picker was just opened** (user 2026-11-21: "when I select
/// the brush or paint bucket and click the choose-a-colour panel, the
/// eyedropper shall automatically get active"): the shell arms the stage
/// eyedropper for `target` when the tool in hand paints with a colour.
pub fn offer(ctx: &egui::Context, target: Id) {
    ctx.data_mut(|d| d.insert_temp(offer_key(), Some(target)));
    ctx.request_repaint();
}

/// The swatch whose picker was opened this frame, once — for the shell.
pub fn dropper_offer(ctx: &egui::Context) -> Option<Id> {
    ctx.data_mut(|d| d.remove_temp::<Option<Id>>(offer_key())).flatten()
}

fn offer_key() -> Id {
    Id::new("screen-color-dropper-offer")
}

fn dropper_key() -> Id {
    Id::new("screen-color-dropper-request")
}

/// Hand the colour the eyedropper took to the swatch `target`.
pub fn deliver(target: Id, color: Color) {
    PICKED.with(|picked| *picked.borrow_mut() = Some((target, color)));
}

/// The colour picked from the screen for `target`, once.
pub fn take(target: Id) -> Option<Color> {
    PICKED.with(|picked| {
        let mut picked = picked.borrow_mut();
        match *picked {
            Some((id, color)) if id == target => {
                *picked = None;
                Some(color)
            }
            _ => None,
        }
    })
}

/// Show the system Colors window, starting from `color`, linked to `target`.
pub fn open_panel(target: Id, color: Color) {
    #[cfg(target_os = "macos")]
    mac::open_panel(target, color);
    #[cfg(not(target_os = "macos"))]
    let _ = (target, color);
}

/// What the Colors window has been moved to, if it is linked to `target` and
/// changed since last asked. Keeps the frames coming while it is up.
pub fn poll_panel(ctx: &egui::Context, target: Id) -> Option<Color> {
    #[cfg(target_os = "macos")]
    return mac::poll_panel(ctx, target);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ctx, target);
        None
    }
}

thread_local! {
    /// The last pick, waiting for its swatch to be drawn. **The window's
    /// thread's own**: the eyedropper delivers it and the swatch takes it on
    /// that one thread, and tests — each on a thread of its own, drawing the
    /// same swatches — no longer take each other's picks.
    static PICKED: std::cell::RefCell<Option<(Id, Color)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::Mutex;

    use egui::Id;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSColor, NSColorPanel, NSColorSpace};
    use peniko::Color;


    /// The swatch the Colors window writes to, and the colour last read from it.
    static LINKED: Mutex<Option<(Id, [u8; 4])>> = Mutex::new(None);

    /// An AppKit colour as sRGB bytes. The sampler hands back the display's
    /// own colour space; converting to sRGB, the space the stage is drawn in,
    /// is what makes a pick of the stage give back the colour it was painted.
    fn rgba8(color: &NSColor) -> Option<[u8; 4]> {
        let srgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some([
            byte(srgb.redComponent() as f64),
            byte(srgb.greenComponent() as f64),
            byte(srgb.blueComponent() as f64),
            byte(srgb.alphaComponent() as f64),
        ])
    }

    fn to_color([r, g, b, a]: [u8; 4]) -> Color {
        Color::from_rgba8(r, g, b, a)
    }

    pub fn open_panel(target: Id, color: Color) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let [r, g, b, a] = color.to_rgba8().to_u8_array();
        let panel = NSColorPanel::sharedColorPanel(mtm);
        panel.setShowsAlpha(true);
        panel.setContinuous(true);
        let start = NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
            f64::from(a) / 255.0,
        );
        panel.setColor(&start);
        if let Ok(mut linked) = LINKED.lock() {
            *linked = Some((target, [r, g, b, a]));
        }
        panel.orderFront(None);
    }

    pub fn poll_panel(ctx: &egui::Context, target: Id) -> Option<Color> {
        let mut linked = LINKED.lock().ok()?;
        let (id, last) = (*linked)?;
        if id != target {
            return None;
        }
        let mtm = MainThreadMarker::new()?;
        if !NSColorPanel::sharedColorPanelExists(mtm) {
            *linked = None;
            return None;
        }
        let panel = NSColorPanel::sharedColorPanel(mtm);
        if !panel.isVisible() {
            // Closed: the link goes with it.
            *linked = None;
            return None;
        }
        // The window is AppKit's, so its changes reach egui only if egui looks.
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        let now = rgba8(&panel.color())?;
        if now == last {
            return None;
        }
        *linked = Some((id, now));
        Some(to_color(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pick lands in the swatch that asked for it, once, and no other.
    #[test]
    fn a_screen_pick_reaches_only_the_swatch_that_asked() {
        let ctx = egui::Context::default();
        let red = Color::from_rgba8(0xE5, 0x12, 0x34, 0xFF);
        // Swatch names no panel uses: tests run side by side, and a real panel's
        // swatch drawn by another test would take the pick first.
        PICKED.with(|p| *p.borrow_mut() = Some((Id::new(("color-row", "screen-pick-test-fill")), red)));

        let mut other = Color::BLACK;
        let mut fill = Color::BLACK;
        let mut changed = (false, false);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            changed.0 = crate::panels::color_row(ui, "screen-pick-test-line", &mut other);
            changed.1 = crate::panels::color_row(ui, "screen-pick-test-fill", &mut fill);
        });
        assert_eq!(changed, (false, true));
        assert_eq!(other, Color::BLACK);
        assert_eq!(fill.to_rgba8(), red.to_rgba8(), "exactly the sampled bytes");
        assert!(take(Id::new(("color-row", "screen-pick-test-fill"))).is_none(), "taken once");
    }
}
