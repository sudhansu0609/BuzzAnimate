//! Pictures of symbols, for the Library and the Assets panel.
//!
//! # Why this exists
//!
//! Both panels identified a character by its **name**. Choosing one meant
//! opening symbols to find out what they were, which is the slowest possible
//! way to answer "which of these is the hero's left arm", and it is the thing
//! that made assembling a scene from parts you already own take so long. A
//! picture is the whole answer.
//!
//! # Why it renders on the UI thread, and why that is not a hang
//!
//! The obvious design is the one an export uses: a second GPU device on a
//! worker thread, rendering to a buffer and reading it back. It is wrong here
//! twice over. Creating a `GpuContext` costs a few hundred milliseconds, so
//! either every thumbnail pays it or a whole device is kept alive for pictures
//! the size of a postage stamp; and reading back from the GPU means blocking
//! on `device.poll`, which is the one thing that must not happen near a frame.
//!
//! So there is no readback at all. Vello draws the symbol into a small texture
//! on the device the window already owns, and that texture is handed to egui
//! as one it can sample — `egui_wgpu` calls this a *native* texture, and it is
//! how a program shows something it rendered itself. The cost is a GPU pass
//! over a 96-pixel square, and **the CPU never waits for it**.
//!
//! What keeps that honest is the budget: at most [`PER_FRAME`] new pictures
//! are drawn in any one frame, however many the panel asked for. A library of
//! three hundred symbols fills in over a few frames instead of stalling one,
//! and scrolling never costs more than scrolling.
//!
//! # Why the key is a pointer
//!
//! A thumbnail is stale when its symbol changes, and the document's own
//! revision is no use: it moves when *anything* changes, so every picture in
//! the library would be redrawn every time a line was drawn on the stage.
//!
//! The library holds `Arc<Symbol>`, and every edit goes through
//! `Arc::make_mut`, so an edited symbol is a **different allocation**. Its
//! address is therefore an identity that changes exactly when the symbol does.
//! The cache holds the `Arc` alongside the picture so the address it is keyed
//! on cannot be reused by something else while the entry lives. This is the
//! same trick `buzz_render::lighting::LightCache` plays on objects, and it is
//! sound for the same reason.

use std::collections::HashMap;

use buzz_geom::{Camera, Rect, Size};
use buzz_scene::{Scene, SymbolId};

/// Edge of a thumbnail, in physical pixels.
///
/// Big enough to tell two characters apart at a glance, small enough that a
/// hundred of them are a few megabytes of texture.
pub const SIZE: u32 = 96;

/// How many new pictures may be drawn in one frame.
///
/// The whole no-hang argument in one number. Four 96-pixel passes are nothing;
/// three hundred would be a stutter.
const PER_FRAME: usize = 4;

/// Margin around the artwork, as the multiplier `Camera::fit_to_rect` takes.
/// A little air, so shapes that run to their own edge do not touch the frame.
const MARGIN: f64 = 1.15;

struct Entry {
    /// Held so the address this entry is keyed on stays valid and unreused.
    symbol: std::sync::Arc<buzz_scene::Symbol>,
    /// **Held, not read.** egui samples this texture every frame through the
    /// id below; dropping it here would leave that id pointing at nothing.
    /// Owning them is the whole job of this struct.
    #[allow(dead_code, reason = "kept alive for egui to sample")]
    texture: buzz_render::wgpu::Texture,
    #[allow(dead_code, reason = "kept alive for egui to sample")]
    view: buzz_render::wgpu::TextureView,
    id: egui::TextureId,
}

/// Pictures of symbols, drawn once and kept until the symbol changes.
#[derive(Default)]
pub struct Thumbnails {
    entries: HashMap<SymbolId, Entry>,
    /// What the panels asked for while the frame was being built, in the order
    /// they asked. Drained after the UI, when the GPU is reachable.
    wanted: Vec<SymbolId>,
}

impl Thumbnails {
    /// Throw away pictures whose symbols have changed.
    ///
    /// Called once a frame, **before** the panels are built. Staleness is
    /// checked here rather than inside [`Self::get`] so that asking for a
    /// picture needs no borrow of the document: the panel that wants one is
    /// already holding the scene mutably, and two borrows of it would not
    /// compile.
    pub fn invalidate_stale(&mut self, scene: &Scene) {
        self.entries.retain(|id, entry| {
            scene
                .library()
                .get(*id)
                // Still the same allocation, so still the same artwork.
                .is_some_and(|current| std::sync::Arc::ptr_eq(current, &entry.symbol))
        });
    }

    /// The picture for a symbol, if there is one, and a note that it was asked
    /// for if there is not.
    ///
    /// Called from panel code, which has no GPU: asking is all it can do.
    pub fn get(&mut self, symbol: SymbolId) -> Option<egui::TextureId> {
        if let Some(entry) = self.entries.get(&symbol) {
            return Some(entry.id);
        }
        if !self.wanted.contains(&symbol) {
            self.wanted.push(symbol);
        }
        None
    }

    /// Is anything waiting to be drawn? The caller repaints while there is, so
    /// a library fills in without the pointer having to move.
    pub fn pending(&self) -> bool {
        !self.wanted.is_empty()
    }

    /// Draw up to [`PER_FRAME`] of the pictures that were asked for.
    ///
    /// Called once a frame from the render path, which is the only place the
    /// device, the Vello renderer and egui's renderer are all reachable.
    pub fn fulfil(
        &mut self,
        gpu: &mut buzz_render::GpuContext,
        egui_renderer: &mut egui_wgpu::Renderer,
        vello: &mut vello::Scene,
        scene: &Scene,
        cache: &mut buzz_render::document::DrawCache,
    ) {
        let wanted: Vec<SymbolId> = self.wanted.drain(..).take(PER_FRAME).collect();
        for symbol in wanted {
            // Gone since it was asked for — a symbol deleted while the panel
            // was being drawn is not an error, it is just nothing to draw.
            let Some(arc) = scene.library().get(symbol).cloned() else {
                self.entries.remove(&symbol);
                continue;
            };
            if let Some(entry) = self.draw(gpu, egui_renderer, vello, scene, symbol, arc, cache) {
                self.entries.insert(symbol, entry);
            }
        }
    }

    /// Forget everything. Used when the document is replaced, so pictures from
    /// the last film cannot be shown for symbols in this one that happen to
    /// have been given the same ids.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.wanted.clear();
    }

    #[allow(clippy::too_many_arguments, reason = "one call site, in the render path")]
    fn draw(
        &mut self,
        gpu: &mut buzz_render::GpuContext,
        egui_renderer: &mut egui_wgpu::Renderer,
        vello: &mut vello::Scene,
        scene: &Scene,
        symbol: SymbolId,
        arc: std::sync::Arc<buzz_scene::Symbol>,
        cache: &mut buzz_render::document::DrawCache,
    ) -> Option<Entry> {
        // An empty symbol has no bounds and needs no picture; the panel shows
        // its name, as it did before.
        let bounds = scene.symbol_bounds(symbol)?;
        if !(bounds.width().is_finite() && bounds.height().is_finite()) {
            return None;
        }

        let camera = fit(bounds, SIZE);
        vello.reset();
        let mut builder = buzz_render::SceneBuilder::new(vello, &camera);
        buzz_render::document::draw_symbol(
            &mut builder,
            scene,
            symbol,
            0,
            buzz_geom::Affine::IDENTITY,
            &buzz_render::document::FrameOptions::default(),
            cache,
        );

        let texture = gpu.device.create_texture(&buzz_render::wgpu::TextureDescriptor {
            label: Some("thumbnail"),
            size: buzz_render::wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: buzz_render::wgpu::TextureDimension::D2,
            format: buzz_render::RENDER_FORMAT,
            usage: buzz_render::wgpu::TextureUsages::STORAGE_BINDING
                | buzz_render::wgpu::TextureUsages::TEXTURE_BINDING
                | buzz_render::wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&buzz_render::wgpu::TextureViewDescriptor::default());

        // Transparent, so a thumbnail sits on the panel's own colour and works
        // in both themes without being told which one is on.
        gpu.render(vello, &view, SIZE, SIZE, peniko::Color::TRANSPARENT)
            .ok()?;

        let id = egui_renderer.register_native_texture(&gpu.device, &view, buzz_render::wgpu::FilterMode::Linear);
        Some(Entry {
            symbol: arc,
            texture,
            view,
            id,
        })
    }
}

/// A camera that fits `bounds` into a square of `size` pixels, centred.
///
/// `Camera::fit_to_rect` is what Zoom ▸ Show All already uses, so a thumbnail
/// frames its symbol by the same rule the stage frames the artwork — including
/// taking the smaller of the two scales, which is what stops a tall character
/// being squashed into a square box.
fn fit(bounds: Rect, size: u32) -> Camera {
    let size = f64::from(size);
    let mut camera = Camera::new(
        bounds.center(),
        1.0,
        Size::new(size, size),
    );
    // A symbol with no extent in either axis cannot be framed; leaving the
    // camera at its default is harmless, because there is nothing to draw.
    camera.fit_to_rect(bounds, MARGIN);
    camera
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    /// Whatever the artwork's shape, the whole of it lands inside the box.
    #[test]
    fn fitting_keeps_the_artwork_inside_the_box() {
        let size = f64::from(SIZE);
        for bounds in [
            rect(0.0, 0.0, 100.0, 100.0),
            rect(-500.0, -20.0, 500.0, 20.0), // very wide
            rect(10.0, -400.0, 30.0, 400.0),  // very tall
            rect(1000.0, 1000.0, 1002.0, 1002.0), // tiny, far from the origin
        ] {
            let camera = fit(bounds, SIZE);
            let visible = camera.visible_doc_rect();
            assert!(
                visible.x0 <= bounds.x0 + 1e-6
                    && visible.x1 >= bounds.x1 - 1e-6
                    && visible.y0 <= bounds.y0 + 1e-6
                    && visible.y1 >= bounds.y1 - 1e-6,
                "{bounds:?} is not wholly inside {visible:?}"
            );
            assert!(camera.zoom.is_finite() && camera.zoom > 0.0);
            let _ = size;
        }
    }

    /// The artwork ends up centred, wherever in the document it was drawn.
    #[test]
    fn fitting_centres_whatever_it_is_given() {
        for bounds in [
            rect(0.0, 0.0, 10.0, 10.0),
            rect(-9000.0, -9000.0, -8990.0, -8990.0),
        ] {
            let camera = fit(bounds, SIZE);
            assert!((camera.center.x - bounds.center().x).abs() < 1e-9);
            assert!((camera.center.y - bounds.center().y).abs() < 1e-9);
        }
    }

    /// A wide symbol and a tall one of the same extent get the same zoom —
    /// the box is square, so the longer side is what has to fit.
    #[test]
    fn fitting_uses_the_longer_side() {
        let wide = fit(rect(0.0, 0.0, 400.0, 100.0), SIZE);
        let tall = fit(rect(0.0, 0.0, 100.0, 400.0), SIZE);
        assert!((wide.zoom - tall.zoom).abs() < 1e-9);
    }

    /// A symbol with no extent must not produce an infinite zoom.
    #[test]
    fn fitting_survives_a_degenerate_symbol() {
        let camera = fit(rect(5.0, 5.0, 5.0, 5.0), SIZE);
        assert!(camera.zoom.is_finite() && camera.zoom > 0.0);
    }
}
