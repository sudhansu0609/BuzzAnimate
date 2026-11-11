//! **A colour from anywhere on the screen**, and the system's own colour panel.
//!
//! The Eyedropper tool reads the paint of the artwork under it, which is the
//! right thing for vector art but cannot take a colour from a placed picture,
//! from a reference image open in another program, or from a web page. macOS
//! has a sampler for exactly that — the magnifying loupe the system Colors
//! window uses — which reads the real pixel under the pointer anywhere on any
//! display. [`pick`] starts it; the colour arrives on the main thread a few
//! frames later and [`take`] hands it to the swatch that asked.
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

/// Start the screen loupe for the swatch `target`. The pick, if one is made,
/// is collected with [`take`].
pub fn pick(ctx: &egui::Context, target: Id) {
    #[cfg(target_os = "macos")]
    mac::pick(ctx, target);
    #[cfg(not(target_os = "macos"))]
    let _ = (ctx, target);
}

/// The colour picked from the screen for `target`, once.
pub fn take(target: Id) -> Option<Color> {
    let mut picked = PICKED.lock().ok()?;
    match *picked {
        Some((id, color)) if id == target => {
            *picked = None;
            Some(color)
        }
        _ => None,
    }
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

/// The last pick, waiting for its swatch to be drawn.
static PICKED: std::sync::Mutex<Option<(Id, Color)>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::Mutex;

    use egui::Id;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSColor, NSColorPanel, NSColorSampler, NSColorSpace};
    use peniko::Color;

    use super::PICKED;

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

    pub fn pick(ctx: &egui::Context, target: Id) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        let ctx = ctx.clone();
        // Called on the main thread when the loupe closes: with the colour,
        // or with nil when it was cancelled with Escape.
        let handler = block2::RcBlock::new(move |picked: *mut NSColor| {
            let Some(picked) = (unsafe { picked.as_ref() }) else {
                return;
            };
            if let Some(bytes) = rgba8(picked)
                && let Ok(mut slot) = PICKED.lock()
            {
                *slot = Some((target, to_color(bytes)));
                ctx.request_repaint();
            }
        });
        let sampler = NSColorSampler::new();
        // SAFETY: on the main thread, as AppKit requires; the sampler retains
        // itself, and the block, until the session ends.
        unsafe { sampler.showSamplerWithSelectionHandler(&handler) };
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
        *PICKED.lock().unwrap() = Some((Id::new(("color-row", "brush-fill")), red));

        let mut other = Color::BLACK;
        let mut fill = Color::BLACK;
        let mut changed = (false, false);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            changed.0 = crate::panels::color_row(ui, "line-stroke", &mut other);
            changed.1 = crate::panels::color_row(ui, "brush-fill", &mut fill);
        });
        assert_eq!(changed, (false, true));
        assert_eq!(other, Color::BLACK);
        assert_eq!(fill.to_rgba8(), red.to_rgba8(), "exactly the sampled bytes");
        assert!(take(Id::new(("color-row", "brush-fill"))).is_none(), "taken once");
    }
}
