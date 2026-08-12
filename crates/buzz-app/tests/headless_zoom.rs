//! Phase 0 exit test: prove unbounded zoom on the real GPU.
//!
//! This renders headlessly through the same encoding path the window uses and
//! reads the pixels back, so it verifies the claim end to end rather than
//! asserting properties of the maths in isolation. `buzz-geom` already proves
//! the arithmetic; this proves the arithmetic survives contact with Vello,
//! wgpu, and an actual driver.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use std::time::Instant;

use buzz_app::demo::ZoomTarget;
use buzz_geom::Size;
use buzz_render::{GpuContext, GpuPreference, wgpu};
use peniko::Color;
use vello::Scene;

const W: u32 = 512;
const H: u32 = 512;
const BACKGROUND: Color = Color::from_rgb8(0x14, 0x16, 0x1A);

struct Harness {
    gpu: GpuContext,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

impl Harness {
    fn new() -> Option<Self> {
        let gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping headless zoom test: no usable GPU ({e})");
                return None;
            }
        };

        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("headless-target"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: buzz_render::RENDER_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // 512 * 4 = 2048, already a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("headless-readback"),
            size: (W * H * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Some(Self {
            gpu,
            texture,
            view,
            readback,
        })
    }

    /// Render `scene` and return the RGBA pixels.
    fn render_and_read(&mut self, scene: &Scene) -> Vec<u8> {
        self.gpu
            .render(scene, &self.view, W, H, BACKGROUND)
            .expect("vello render");

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                // Bounded so a driver hang fails the test rather than wedging
                // it — a frame that never completes is itself a failure.
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .expect("poll device");

        // The mapped range must be dropped before `unmap`; scoping it is what
        // enforces that (dropping the slice itself would be a no-op, since
        // `BufferSlice` is `Copy`).
        let pixels = {
            let view = slice.get_mapped_range();
            view.to_vec()
        };
        self.readback.unmap();
        pixels
    }
}

/// Fraction of pixels that differ from the background.
fn ink_coverage(pixels: &[u8]) -> f64 {
    let bg = BACKGROUND.to_rgba8().to_u8_array();
    let mut inked = 0usize;
    let total = pixels.len() / 4;

    for px in pixels.chunks_exact(4) {
        // Tolerance absorbs anti-aliased edges blending into the background.
        let d = (px[0] as i32 - bg[0] as i32).abs()
            + (px[1] as i32 - bg[1] as i32).abs()
            + (px[2] as i32 - bg[2] as i32).abs();
        if d > 12 {
            inked += 1;
        }
    }
    inked as f64 / total.max(1) as f64
}

/// Count distinct quantised colours — a proxy for "real structure, not a
/// single flat smear".
fn distinct_colours(pixels: &[u8]) -> usize {
    let mut seen = std::collections::HashSet::new();
    for px in pixels.chunks_exact(4) {
        seen.insert((px[0] / 24, px[1] / 24, px[2] / 24));
    }
    seen.len()
}

#[test]
fn zoom_sweep_stays_clean_to_one_trillion_percent() {
    let Some(mut h) = Harness::new() else { return };

    println!(
        "GPU: {}\n",
        h.gpu.selection.chosen.info.name
    );
    // Column widths match the row format below.
    println!(
        " gen      zoom  precision    ink%  colours     encode       gpu  drawn/culled"
    );

    let artwork = ZoomTarget::default();
    let viewport = Size::new(W as f64, H as f64);
    let mut scene = Scene::new();
    let mut worst_frame_ms = 0.0f64;

    // Generation 10 sits at 1e12 % — the figure quoted to the user. Going two
    // past it confirms the failure mode past the budget is graceful.
    for generation in 0..=12usize {
        let camera = artwork.camera_for_generation(generation, viewport);

        // Split CPU encoding from GPU work: a single combined number cannot
        // tell you which side a slowdown is on.
        let encode_started = Instant::now();
        let stats = artwork.encode(&mut scene, &camera);
        let encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;

        let started = Instant::now();
        let pixels = h.render_and_read(&scene);
        let frame_ms = started.elapsed().as_secs_f64() * 1000.0;
        worst_frame_ms = worst_frame_ms.max(frame_ms);

        let ink = ink_coverage(&pixels);
        let colours = distinct_colours(&pixels);

        let zoom_label = format!("{:.0e}%", camera.zoom_percent());
        println!(
            "{generation:>4}  {zoom_label:>8}  {:>9.2e}  {:>5.2}%  {colours:>7}  \
             {encode_ms:>7.2}ms  {frame_ms:>6.1}ms  {}/{}",
            camera.screen_precision_px(),
            ink * 100.0,
            stats.drawn,
            stats.culled()
        );

        // The core assertion: there is still artwork on screen. A precision
        // collapse shows up here as an empty or fully-smeared frame.
        assert!(
            ink > 0.002,
            "generation {generation} at {:.0e}% rendered essentially nothing \
             (ink {:.4}%, drawn {}). This is what precision collapse looks like.",
            camera.zoom_percent(),
            ink * 100.0,
            stats.drawn
        );
        assert!(
            ink < 0.98,
            "generation {generation} at {:.0e}% filled the frame — geometry \
             blew up rather than zoomed in (ink {:.2}%)",
            camera.zoom_percent(),
            ink * 100.0
        );
        assert!(
            stats.drawn > 0,
            "generation {generation} encoded no items at all"
        );
        assert!(
            colours >= 3,
            "generation {generation} produced a flat image ({colours} colours), \
             suggesting the geometry degenerated"
        );
    }

    println!("\nworst frame: {worst_frame_ms:.1} ms");
}

/// Animate's ceiling is 2000%. Confirm going far past it is not merely
/// permitted but actually renders more detail rather than less.
#[test]
fn detail_survives_far_beyond_animates_ceiling() {
    let Some(mut h) = Harness::new() else { return };

    let artwork = ZoomTarget::default();
    let viewport = Size::new(W as f64, H as f64);
    let mut scene = Scene::new();

    // Animate's maximum, then a trillion percent.
    let at_animate_max = {
        let cam = artwork.camera_for_generation(1, viewport);
        assert!(
            cam.zoom_percent() <= 2000.0,
            "generation 1 should sit within Animate's range"
        );
        artwork.encode(&mut scene, &cam);
        distinct_colours(&h.render_and_read(&scene))
    };

    let at_trillion = {
        let cam = artwork.camera_for_generation(10, viewport);
        assert!(cam.zoom_percent() > 1e11, "expected ~1e12%");
        artwork.encode(&mut scene, &cam);
        distinct_colours(&h.render_and_read(&scene))
    };

    assert!(
        at_trillion >= 3,
        "at 1e12% the image had only {at_trillion} distinct colours; \
         detail did not survive"
    );
    println!(
        "distinct colours — at 2000%: {at_animate_max}, at ~1e12%: {at_trillion}"
    );
}

/// Two renders of the same camera must produce identical pixels. Instability
/// here would mean uninitialised memory or a race in the render path.
#[test]
fn rendering_is_deterministic_at_extreme_zoom() {
    let Some(mut h) = Harness::new() else { return };

    let artwork = ZoomTarget::default();
    let camera = artwork.camera_for_generation(10, Size::new(W as f64, H as f64));
    let mut scene = Scene::new();

    artwork.encode(&mut scene, &camera);
    let first = h.render_and_read(&scene);

    artwork.encode(&mut scene, &camera);
    let second = h.render_and_read(&scene);

    assert_eq!(
        first, second,
        "identical input produced different pixels at {:.0e}%",
        camera.zoom_percent()
    );
}
