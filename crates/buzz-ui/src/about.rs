//! Help ▸ About: the banner, what this is, and what it is built on.
//!
//! Every program has one, and most of them are an afterthought. This one earns
//! its place by answering the two questions somebody actually opens it for:
//! *what version am I running* — which is the first thing anybody reporting a
//! problem is asked — and *what is this thing*.

use egui::{RichText, Ui};

/// The banner, as it ships. Decoded once and kept as a texture.
const BANNER: &[u8] = include_bytes!("../../../assets/banner-800.png");

/// Dialog state: open, and the texture once it has been uploaded.
#[derive(Default)]
pub struct AboutState {
    pub open: bool,
    banner: Option<egui::TextureHandle>,
}

impl std::fmt::Debug for AboutState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AboutState")
            .field("open", &self.open)
            .field("banner", &self.banner.is_some())
            .finish()
    }
}

/// Draw the About window. Does nothing while it is closed.
pub fn about_dialog(ctx: &egui::Context, state: &mut AboutState) {
    if !state.open {
        return;
    }

    // Uploaded on first use rather than at startup: a window nobody opens
    // should not cost a texture.
    if state.banner.is_none()
        && let Some(image) = decode(BANNER)
    {
        state.banner = Some(ctx.load_texture("about-banner", image, Default::default()));
    }

    let mut open = state.open;
    egui::Window::new("About BuzzAnimate")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| body(ui, state));
    state.open = open;
}

fn body(ui: &mut Ui, state: &AboutState) {
    if let Some(banner) = &state.banner {
        let width = 560.0;
        let size = banner.size_vec2();
        let height = width * size.y / size.x;
        ui.add(
            egui::Image::new(banner)
                .fit_to_exact_size(egui::vec2(width, height))
                .corner_radius(3),
        );
        ui.add_space(6.0);
    }

    ui.heading("BuzzAnimate");
    ui.label(
        RichText::new(format!("version {}", env!("CARGO_PKG_VERSION")))
            .small()
            .weak(),
    );
    ui.add_space(4.0);
    ui.label(
        "GPU-accelerated vector animation \u{2014} Animate's shape of a program, \
         without its ceiling: unbounded zoom, every core working, and \
         rasterisation on the graphics card.",
    );

    ui.add_space(6.0);
    ui.separator();
    ui.label(
        RichText::new("Buzzcaf Media \u{00B7} artwork from Spilled Coffee Studios")
            .small()
            .weak(),
    );
    ui.label(
        RichText::new(
            "Built clean-room: no Adobe code, assets, icons or trademarks. \
             Rust, wgpu, Vello, egui, kurbo.",
        )
        .small()
        .weak(),
    );
}

/// Decode a PNG into something egui can upload.
fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .ok()?;
    let mut pixels = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(info.buffer_size());

    let size = [info.width as usize, info.height as usize];
    match info.color_type {
        png::ColorType::Rgba => Some(egui::ColorImage::from_rgba_unmultiplied(size, &pixels)),
        png::ColorType::Rgb => Some(egui::ColorImage::from_rgb(size, &pixels)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The banner has to be a picture this program can actually decode — a
    /// broken or unsupported file would show an empty window, and nobody
    /// checks About until something is already wrong.
    #[test]
    fn the_banner_decodes() {
        let image = decode(BANNER).expect("the banner should decode");
        assert!(
            image.width() > 200 && image.height() > 100,
            "{:?}",
            image.size
        );
    }

    #[test]
    fn it_is_closed_until_it_is_asked_for() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut state = AboutState::default();

        let _ = ctx.run_ui(Default::default(), |ui| about_dialog(ui.ctx(), &mut state));
        assert!(!state.open);
        assert!(
            state.banner.is_none(),
            "a window nobody opened should not have uploaded a texture"
        );

        state.open = true;
        let _ = ctx.run_ui(Default::default(), |ui| about_dialog(ui.ctx(), &mut state));
        assert!(state.open);
        assert!(
            state.banner.is_some(),
            "opening it should upload the banner"
        );
    }
}
