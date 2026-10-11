//! **A render must not depend on render history.** `docs/BIMARI5_LAG_PLAN.md`,
//! S13b.
//!
//! The vendored `vello_encoding` resolver used to report a 0x0 image atlas for
//! a frame with no late-bound resources (no images, gradients or glyphs).
//! Vello then dropped its persistent atlas texture, while the image cache still
//! listed every image as resident and clean, so the next frame with an image
//! fill drew from a blank atlas. These tests render with ONE renderer:
//! image scene, solid-only scene, image scene again, and require the two image
//! renders to be identical. Same again with a gradient.
//!
//! Skips rather than fails without a GPU.

use buzz_render::{GpuContext, GpuPreference, vello, wgpu};
use peniko::{Color, Fill};
use std::sync::Arc;

const W: u32 = 256;
const H: u32 = 256;
const BACKGROUND: Color = Color::from_rgb8(0x20, 0x20, 0x20);

struct Harness {
    gpu: GpuContext,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

fn harness() -> Option<Harness> {
    let gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping atlas history test: no usable GPU ({e})");
            return None;
        }
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("atlas-history-target"),
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
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("atlas-history-readback"),
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Some(Harness {
        gpu,
        texture,
        view,
        readback,
    })
}

impl Harness {
    fn render(&mut self, scene: &vello::Scene) -> Vec<u8> {
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
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .expect("poll device");
        let pixels = slice.get_mapped_range().to_vec();
        self.readback.unmap();
        pixels
    }
}

fn rect() -> kurbo::Rect {
    kurbo::Rect::new(16.0, 16.0, 240.0, 240.0)
}

/// A scene whose only fill is an image: a 16x16 colourful test card.
fn image_scene(image: &peniko::ImageData) -> vello::Scene {
    let mut scene = vello::Scene::new();
    let brush = peniko::ImageBrush {
        image: image.clone(),
        sampler: peniko::ImageSampler {
            x_extend: peniko::Extend::Pad,
            y_extend: peniko::Extend::Pad,
            quality: peniko::ImageQuality::Low,
            alpha: 1.0,
        },
    };
    scene.fill(
        Fill::NonZero,
        kurbo::Affine::IDENTITY,
        &brush,
        Some(kurbo::Affine::scale(14.0)),
        &rect(),
    );
    scene
}

fn gradient_scene() -> vello::Scene {
    let mut scene = vello::Scene::new();
    let mut g = peniko::Gradient::new_linear((16.0, 0.0), (240.0, 0.0));
    g = g.with_stops([
        Color::from_rgb8(0xFF, 0x20, 0x20),
        Color::from_rgb8(0x20, 0x40, 0xFF),
    ]);
    scene.fill(Fill::NonZero, kurbo::Affine::IDENTITY, &g, None, &rect());
    scene
}

/// Only solid fills: no late-bound resources at all.
fn solid_scene() -> vello::Scene {
    let mut scene = vello::Scene::new();
    scene.fill(
        Fill::NonZero,
        kurbo::Affine::IDENTITY,
        Color::from_rgb8(0x80, 0xC0, 0x40),
        None,
        &rect(),
    );
    scene
}

fn test_card() -> peniko::ImageData {
    let (w, h) = (16u32, 16u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(x * 16) as u8, (y * 16) as u8, ((x ^ y) * 16) as u8, 255]);
        }
    }
    peniko::ImageData {
        data: peniko::Blob::new(Arc::new(px)),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::Alpha,
        width: w,
        height: h,
    }
}

fn count_differing(a: &[u8], b: &[u8]) -> usize {
    assert_eq!(a.len(), b.len());
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 1))
        .count()
}

#[test]
fn an_image_fill_renders_the_same_after_a_patchless_frame() {
    let Some(mut h) = harness() else { return };
    let image = test_card();
    let p1 = h.render(&image_scene(&image));
    // Sanity: the image actually drew (not the flat background).
    assert!(
        count_differing(&p1, &h.render(&solid_scene())) > 1000,
        "image scene should differ from a solid scene"
    );
    let _ = h.render(&solid_scene());
    let p2 = h.render(&image_scene(&image));
    assert_eq!(
        count_differing(&p1, &p2),
        0,
        "image render changed after a frame with no resources"
    );
}

#[test]
fn a_gradient_fill_renders_the_same_after_a_patchless_frame() {
    let Some(mut h) = harness() else { return };
    let p1 = h.render(&gradient_scene());
    let _ = h.render(&solid_scene());
    let p2 = h.render(&gradient_scene());
    assert_eq!(
        count_differing(&p1, &p2),
        0,
        "gradient render changed after a frame with no resources"
    );
}
