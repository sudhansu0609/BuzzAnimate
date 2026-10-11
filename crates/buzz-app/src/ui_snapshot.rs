//! Offscreen screenshot harness for the egui UI.
//!
//! Stage 0 of `docs/WORKSPACE_DESIGN_PLAN.md`: every later stage of that plan
//! ships PNGs an architect can look at rather than a description of what
//! changed. This renders exactly the same `App::build_ui` the real window
//! draws, through the same kind of headless GPU device the exporter brings up
//! ([`buzz_render::GpuPreference`]), and reads it back with the exporter's own
//! [`buzz_export::Frame`] — so a screenshot is never a second rendering path
//! that could quietly drift from what the window actually shows.
//!
//! # Why `egui_wgpu` rather than a CPU rasteriser
//!
//! `egui_wgpu::Renderer` is already linked into this binary for the real
//! window (`app.rs`), targets the identical `wgpu::Device`/`Queue` the
//! exporter's `GpuContext` brings up, and accepts an arbitrary render-target
//! texture — so pointing it at an offscreen texture instead of a surface is a
//! change of target, not a second renderer to keep faithful to the first.

use std::path::Path;

use anyhow::{Context, Result};
use buzz_export::Frame;
use buzz_render::{GpuContext, GpuPreference, RENDER_FORMAT, wgpu};

use crate::app::App;

/// One screenshot request: the window's logical size and its pixel density.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotConfig {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

impl SnapshotConfig {
    /// 1600 x 900 at 100%, the size every Stage 0 snapshot is taken at.
    pub const HD: Self = Self {
        width: 1600,
        height: 900,
        scale: 1.0,
    };

    /// The same window, at 150% — the plan's second required scale.
    pub const HD_150: Self = Self {
        width: 1600,
        height: 900,
        scale: 1.5,
    };
}

/// wgpu requires each row of a buffer a texture is copied into to be padded to
/// a multiple of this many bytes. Copied from `buzz_export`'s own constant
/// rather than re-exported, because a screenshot has no business depending on
/// the exporter's internals beyond the `Frame` it already publishes.
const COPY_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// An `egui::Color32` as a `wgpu::Color` for a render pass's clear colour.
/// Gamma-space values straight through: `RENDER_FORMAT` (`Rgba8Unorm`) is a
/// gamma-space target, the same convention egui's own vertex colours use, so
/// no linearisation belongs here.
fn clear_color(c: egui::Color32) -> wgpu::Color {
    wgpu::Color {
        r: c.r() as f64 / 255.0,
        g: c.g() as f64 / 255.0,
        b: c.b() as f64 / 255.0,
        a: 1.0,
    }
}

/// Render one frame of `app`'s UI offscreen and return it as pixels.
///
/// Two frames of `build_ui` run: the first lays the dock out (scroll areas and
/// first-frame layout settle a frame late in egui), the second is the one
/// actually painted — the same reason the dock-drag test harness in `app.rs`
/// runs a spare frame before it starts asserting anything.
pub fn render_ui(app: &mut App, config: SnapshotConfig) -> Result<Frame> {
    render_frame(config, |ui| {
        app.build_ui(ui);
    })
}

/// [`render_ui`], generalised to any egui drawing closure rather than a whole
/// `App` — what the icon contact sheet below renders with, since it has no
/// document, no dock and no use for either. `render_ui` is this plus
/// `app.build_ui`, kept as its own function so the one GPU-rendering path is
/// never duplicated between the two.
pub fn render_frame(config: SnapshotConfig, mut draw: impl FnMut(&mut egui::Ui)) -> Result<Frame> {
    let gpu = GpuContext::new_blocking(&GpuPreference::Automatic)
        .context("bringing up a headless GPU for a UI snapshot")?;

    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    ctx.set_zoom_factor(config.scale);

    let screen_rect = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(config.width as f32, config.height as f32),
    );
    let input = || egui::RawInput {
        screen_rect: Some(screen_rect),
        ..Default::default()
    };

    // The renderer is built *before* either frame runs, and fed every
    // frame's `textures_delta` — including the warm-up one — because the
    // font atlas is only ever in a frame's `textures_delta.set` the first
    // time egui uploads it. A renderer created after both frames never
    // learns the atlas exists: every mesh (glyphs and flat fills alike, both
    // sampling the atlas for `WHITE_UV`) then references a texture id the
    // renderer has never heard of and is silently skipped, which is a solid
    // clear colour rather than a UI — a mistake worth naming so it is not
    // made twice.
    let mut renderer =
        egui_wgpu::Renderer::new(&gpu.device, RENDER_FORMAT, egui_wgpu::RendererOptions::default());

    let warm_up = ctx.run_ui(input(), |ui| {
        draw(ui);
    });
    for (id, delta) in &warm_up.textures_delta.set {
        renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
    }
    for id in &warm_up.textures_delta.free {
        renderer.free_texture(id);
    }

    let output = ctx.run_ui(input(), |ui| {
        draw(ui);
    });

    let pixels_per_point = output.pixels_per_point;
    let paint_jobs = ctx.tessellate(output.shapes, pixels_per_point);

    let width_px = (config.width as f32 * pixels_per_point).round().max(1.0) as u32;
    let height_px = (config.height as f32 * pixels_per_point).round().max(1.0) as u32;

    let padded_row = (width_px * 4).div_ceil(COPY_ALIGNMENT) * COPY_ALIGNMENT;
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ui-snapshot-target"),
        size: wgpu::Extent3d {
            width: width_px,
            height: height_px,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: RENDER_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui-snapshot-readback"),
        size: (padded_row as u64) * (height_px as u64),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ui-snapshot"),
        });

    for (id, delta) in &output.textures_delta.set {
        renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
    }
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width_px, height_px],
        pixels_per_point,
    };
    let user_buffers =
        renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &paint_jobs, &screen);

    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui-snapshot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Cleared to the pasteboard colour, which is what the
                    // stage area sits on beneath the chrome egui draws — this
                    // harness paints the panels, not the Vello artwork, so
                    // the stage would otherwise read as a hole rather than as
                    // the working surface it is in the real window.
                    load: wgpu::LoadOp::Clear(clear_color(buzz_ui::Palette::pasteboard())),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
    }

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height_px),
            },
        },
        wgpu::Extent3d {
            width: width_px,
            height: height_px,
            depth_or_array_layers: 1,
        },
    );

    gpu.queue
        .submit(user_buffers.into_iter().chain([encoder.finish()]));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| {
        if let Err(e) = r {
            tracing::error!("UI snapshot readback failed: {e:?}");
        }
    });
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(60)),
        })
        .context("waiting for the GPU to finish a UI snapshot")?;

    let mut pixels = Vec::with_capacity((width_px * height_px * 4) as usize);
    {
        let mapped = slice.get_mapped_range();
        let row_bytes = (width_px * 4) as usize;
        for row in 0..height_px as usize {
            let start = row * padded_row as usize;
            pixels.extend_from_slice(&mapped[start..start + row_bytes]);
        }
    }
    readback.unmap();

    // The chrome is opaque everywhere it draws, but egui's antialiasing
    // leaves a fractional alpha along glyph and shape edges; a screenshot is
    // meant to be a flat picture, not a sprite composited over whatever
    // happens to load it, so alpha is forced to opaque here rather than
    // asking every viewer to composite it over black correctly.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[3] = 255;
    }

    Ok(Frame {
        width: width_px,
        height: height_px,
        pixels,
    })
}

/// [`render_ui`], written to `path` as a PNG. Parent directories are created
/// as needed, matching `target/ui-snapshots/` not existing until the first
/// snapshot is taken.
pub fn write_ui_snapshot(app: &mut App, config: SnapshotConfig, path: &Path) -> Result<()> {
    write_frame(config, path, |ui| {
        app.build_ui(ui);
    })
}

/// [`render_frame`], written to `path` as a PNG.
pub fn write_frame(
    config: SnapshotConfig,
    path: &Path,
    draw: impl FnMut(&mut egui::Ui),
) -> Result<()> {
    let frame = render_frame(config, draw)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    frame.write_png(path)
}

/// **The icon contact sheet.**
///
/// Every tool glyph in `buzz_ui::icons`, drawn at both sizes the design
/// brief cares about — 20 px, the icon size inside a real 28 px button, and
/// 28 px, the button's own full size — on a plain grid rather than inside
/// the chrome, so a reviewer is looking at the glyph and nothing else. Below
/// that, the timeline's own transport strip, rendered through the real
/// `buzz_ui::timeline_panel::timeline_panel` (not reimplemented here) so
/// what is captured is exactly what the app draws, at its own native size.
///
/// This replaces `tools/make_icon_outlines.py`'s old Pillow composite: that
/// script only ever composited the raster masks it had just written, which
/// is gone along with them. A Rust harness is the only thing left that can
/// draw a painter-path glyph, so it is the only thing left that can preview
/// one.
fn draw_icon_contact_sheet(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(buzz_ui::Palette::bg())
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                for size in [20.0_f32, 28.0] {
                    ui.label(
                        egui::RichText::new(format!("Tools — {size:.0} px"))
                            .color(buzz_ui::Palette::text())
                            .strong(),
                    );
                    ui.add_space(4.0);
                    egui::Grid::new(("icon-contact-sheet-tools", size as i32))
                        .spacing(egui::vec2(6.0, 6.0))
                        .show(ui, |ui| {
                            let cell = 28.0_f32.max(size + 8.0);
                            for (i, tool) in buzz_ui::all_tools().into_iter().enumerate() {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(cell, cell),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, 2.0, buzz_ui::Palette::panel());
                                let icon_rect =
                                    egui::Rect::from_center_size(rect.center(), egui::vec2(size, size));
                                buzz_ui::icons::tool_icon(
                                    ui.painter(),
                                    icon_rect,
                                    tool,
                                    buzz_ui::Palette::text(),
                                );
                                if (i + 1) % 8 == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                    ui.add_space(16.0);
                }

                ui.label(
                    egui::RichText::new("Timeline transport — native size")
                        .color(buzz_ui::Palette::text())
                        .strong(),
                );
                ui.add_space(4.0);
                let document = fixture_document();
                let scene = document.scene();
                let state = buzz_ui::timeline_panel::TimelineState {
                    current_frame: 0,
                    active_layer: None,
                    selected_layers: Vec::new(),
                    range_anchor: None,
                    timeline_selection: None,
                    camera_selected: false,
                    selected_light: None,
                    playing: false,
                    onion_enabled: false,
                    auto_keyframe: false,
                    edit_multiple: false,
                    onion_before: 0,
                    onion_after: 0,
                    frame_width: buzz_ui::Metrics::FRAME_WIDTH,
                    row_scale: 1.0,
                    parenting_view: false,
                    depth_view: false,
                    focal_distance: 500.0,
                    nearest_depth: 1.0,
                    waveforms: Default::default(),
                    beats: Vec::new(),
                    shot_end: None,
                };
                let _ = buzz_ui::timeline_panel::timeline_panel(ui, scene, &state);
            });
        });
}

/// [`draw_icon_contact_sheet`], written to `path` as a PNG.
pub fn write_icon_contact_sheet(path: &Path) -> Result<()> {
    write_frame(
        SnapshotConfig {
            width: 700,
            height: 760,
            scale: 1.0,
        },
        path,
        |ui| draw_icon_contact_sheet(ui),
    )
}

/// A small fixture document, built entirely from the `buzz_scene` API rather
/// than a file on disk: two layers, a rectangle on one, a symbol instance on
/// the other, and a classic tween across two keyframes of it — enough that
/// the Properties, Library, Layers and Timeline panels all have something to
/// draw when a snapshot is taken.
pub fn fixture_document() -> buzz_doc::Document {
    use buzz_geom::{Affine, Rect, Shape, Size, Vec2};
    use buzz_scene::{Layer, LayerId, LayerKind, Object, ObjectId, Scene, ShapeData, SymbolKind, Tween};
    use peniko::Color;

    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(960.0, 540.0);
    scene.set_frame_count(24);

    let background = scene.add_layer("Background", LayerKind::Normal);
    scene.add_object(
        background,
        Object::shape(
            ObjectId(1),
            ShapeData::filled(
                Rect::new(0.0, 0.0, 960.0, 540.0).to_path(1e-9),
                Color::from_rgb8(0x40, 0x40, 0x40),
            ),
        ),
    );

    let symbol = scene.add_symbol("Figure", SymbolKind::Graphic, None);
    scene.library_mut().update(symbol, |sym| {
        let art = std::sync::Arc::new(Object::shape(
            ObjectId(2),
            ShapeData::filled(
                Rect::new(-40.0, -60.0, 40.0, 60.0).to_path(1e-9),
                Color::from_rgb8(0x37, 0x8E, 0xF0),
            ),
        ));
        let mut inner = Layer::new(LayerId(100), "body", LayerKind::Normal);
        inner.frames.set_objects(0, vec![art]);
        sym.layers.push_front(inner);
    });

    let art = scene.add_layer("Art", LayerKind::Normal);
    scene.add_instance_at(art, 0, symbol, Affine::translate(Vec2::new(480.0, 300.0)));
    scene.update_layer(art, |l| {
        l.frames.insert_keyframe(12);
        l.frames.set_tween(0, Tween::classic());
    });

    buzz_doc::Document::new(scene)
}

/// Build an `App` with the fixture document open, ready for a snapshot: the
/// same construction the `dock_drag_tests::Drive` harness in `app.rs` uses,
/// so a snapshot never opens a recovery dialog over the picture.
pub fn fixture_app() -> App {
    let mut app = App::new(GpuPreference::Automatic).with_document(fixture_document());
    app.recovery.found.clear();
    app
}

/// A second fixture, for the timeline specifically: twelve layers, one of
/// them a folder holding three of the others. The workspace defects brief's
/// item 3 (row alignment, the footer, folder indentation) has nothing to
/// show in a screenshot of [`fixture_document`]'s two layers — none of them
/// scroll, and neither is filed under a folder.
pub fn fixture_document_with_many_layers() -> buzz_doc::Document {
    use buzz_scene::{LayerKind, Scene};

    let mut scene = Scene::default();
    scene.set_frame_count(24);

    // `Scene::default` already carries one starter layer ("Layer_1"); a
    // folder, three layers filed under it, and seven plain layers on top of
    // that make twelve.
    let folder = scene.add_layer("Folder 1", LayerKind::Folder);
    for i in 1..=3 {
        let child = scene.add_layer(format!("In Folder 1 \u{2014} {i}"), LayerKind::Normal);
        scene.update_layer(child, |l| l.parent = Some(folder));
    }
    for i in 1..=7 {
        scene.add_layer(format!("Layer {i}"), LayerKind::Normal);
    }

    buzz_doc::Document::new(scene)
}

/// [`fixture_app`], for [`fixture_document_with_many_layers`].
pub fn fixture_app_with_many_layers() -> App {
    let mut app =
        App::new(GpuPreference::Automatic).with_document(fixture_document_with_many_layers());
    app.recovery.found.clear();
    app
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_ui::theme::{self, Theme};

    /// Writes the four PNGs the plan's Stage 0 asks for, at
    /// `target/ui-snapshots/`. `#[ignore]`d because it needs a real GPU
    /// adapter, which is not guaranteed on every machine this suite runs on;
    /// run explicitly with `cargo test -p buzz-app ui_snapshot::tests::ui_snapshots -- --ignored`.
    #[test]
    #[ignore = "needs a GPU adapter; run explicitly to produce the review PNGs"]
    fn ui_snapshots() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-snapshots");

        let was = theme::theme();
        for (t, name) in [
            (Theme::Dark, "dark_default"),
            (Theme::Light, "light_default"),
            (Theme::Contrast, "contrast_default"),
        ] {
            theme::set_theme(t);
            let mut app = fixture_app();
            write_ui_snapshot(&mut app, SnapshotConfig::HD, &dir.join(format!("{name}.png")))
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        }

        theme::set_theme(Theme::Dark);
        let mut app = fixture_app();
        write_ui_snapshot(
            &mut app,
            SnapshotConfig::HD_150,
            &dir.join("dark_default_150.png"),
        )
        .expect("dark_default_150");

        // Defects brief item 4: the timeline case (row alignment, the
        // footer, folder indentation) needs a document that actually has
        // twelve layers and a folder to show any of it.
        let mut many_layers = fixture_app_with_many_layers();
        write_ui_snapshot(
            &mut many_layers,
            SnapshotConfig::HD,
            &dir.join("dark_timeline_many_layers.png"),
        )
        .expect("dark_timeline_many_layers");

        // Stages 4–6's own close-out: the two named presets that actually
        // ship, by name rather than by "whatever `Default` happens to be" —
        // `dark_default.png` above is Essentials too now that `Default` is,
        // but a reviewer comparing Essentials against Animator needs both
        // named for what they are, not for which one used to be first.
        for (t, workspace, name) in [
            (Theme::Dark, buzz_ui::Workspace::essentials(), "dark_essentials"),
            (Theme::Dark, buzz_ui::Workspace::animate(), "dark_animator"),
            (Theme::Light, buzz_ui::Workspace::essentials(), "light_essentials"),
            (
                Theme::Contrast,
                buzz_ui::Workspace::essentials(),
                "contrast_essentials",
            ),
        ] {
            theme::set_theme(t);
            let mut app = fixture_app().with_workspace(workspace);
            write_ui_snapshot(&mut app, SnapshotConfig::HD, &dir.join(format!("{name}.png")))
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        }

        theme::set_theme(was);
    }

    /// Writes `target/ui-snapshots/icons_{dark,light}.png` — every tool
    /// glyph at 20 px and at 28 px, plus the timeline transport strip at its
    /// own native size, per theme. Replaces `tools/make_icon_outlines.py`'s
    /// old Pillow contact sheet now that the icons it composited (the
    /// user's own sheet, reduced to raster outline masks) are gone; this is
    /// the Rust harness `docs/WORKSPACE_DESIGN_PLAN.md`'s icon follow-up
    /// asks for instead. `#[ignore]`d for the same reason `ui_snapshots` is:
    /// run explicitly with
    /// `cargo test -p buzz-app ui_snapshot::tests::icon_contact_sheets -- --ignored`.
    #[test]
    #[ignore = "needs a GPU adapter; run explicitly to produce the review PNGs"]
    fn icon_contact_sheets() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-snapshots");

        let was = theme::theme();
        for (t, name) in [(Theme::Dark, "icons_dark"), (Theme::Light, "icons_light")] {
            theme::set_theme(t);
            write_icon_contact_sheet(&dir.join(format!("{name}.png")))
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        }
        theme::set_theme(was);
    }
}
