//! Headless GPU tests for the full-frame compositor.
//!
//! Each test skips cleanly when no adapter is present (headless CI), so the
//! suite is green on a machine with no GPU and meaningful on one with a GPU.
//! Creating the pipelines is itself a test: naga compiles and validates
//! `compositor.wgsl` when the pipeline is built, so a shader typo fails here.

use buzz_render::wgpu;
use buzz_render::{Compositor, GpuContext, GpuPreference};
use buzz_scene::PostSettings;

const W: u32 = 64;
const H: u32 = 64;

/// A device and queue, or `None` when there is no usable GPU.
fn gpu() -> Option<GpuContext> {
    match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping: no GPU ({e})");
            None
        }
    }
}

/// Run the compositor over `input` (RGBA8, W×H) and read the result back.
fn run(gpu: &mut GpuContext, input: &[u8], post: &PostSettings, frame_index: u32) -> Vec<u8> {
    assert_eq!(input.len(), (W * H * 4) as usize);
    let device = &gpu.device;
    let queue = &gpu.queue;

    let in_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-input"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &in_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        input,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(W * 4),
            rows_per_image: Some(H),
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    let in_view = in_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let out_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-output"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let out_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let mut compositor = Compositor::new(device, wgpu::TextureFormat::Rgba8Unorm);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test-encoder"),
    });
    compositor.run(
        device,
        queue,
        &mut encoder,
        &in_view,
        &out_view,
        W,
        H,
        post,
        frame_index,
    );

    // Read the output back through a buffer. W*4 == 256, already aligned.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-readback"),
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &out_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
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
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().to_vec();
    readback.unmap();
    data
}

/// Pixel index into an RGBA8 buffer.
fn at(x: u32, y: u32) -> usize {
    ((y * W + x) * 4) as usize
}

fn solid(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut v = vec![0u8; (W * H * 4) as usize];
    for px in v.chunks_mut(4) {
        px.copy_from_slice(&[r, g, b, 255]);
    }
    v
}

#[test]
fn disabled_is_bit_identical_to_input() {
    let Some(mut gpu) = gpu() else { return };

    // A varied input so a passthrough that dropped or shifted bits would show.
    let mut input = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = at(x, y);
            input[i] = (x * 4) as u8;
            input[i + 1] = (y * 4) as u8;
            input[i + 2] = ((x + y) * 2) as u8;
            input[i + 3] = 255;
        }
    }

    let out = run(&mut gpu, &input, &PostSettings::default(), 0);
    assert_eq!(out, input, "a disabled compositor must be an exact copy");
}

#[test]
fn vignette_darkens_the_corners() {
    let Some(mut gpu) = gpu() else { return };

    let input = solid(180, 180, 180);
    let mut post = PostSettings::default();
    post.enabled = true;
    post.vignette.enabled = true;
    post.vignette.amount = 0.9;
    post.vignette.softness = 0.8;

    let out = run(&mut gpu, &input, &post, 0);
    let centre = out[at(W / 2, H / 2)];
    let corner = out[at(0, 0)];
    assert!(
        corner < centre,
        "corner ({corner}) should be darker than centre ({centre})"
    );
    assert_eq!(
        centre, 180,
        "the centre of a vignette should be untouched, was {centre}"
    );
}

#[test]
fn bloom_bleeds_past_a_bright_dot() {
    let Some(mut gpu) = gpu() else { return };

    // Black frame with a bright block in the middle.
    let mut input = vec![0u8; (W * H * 4) as usize];
    for px in input.chunks_mut(4) {
        px[3] = 255;
    }
    for y in (H / 2 - 3)..(H / 2 + 3) {
        for x in (W / 2 - 3)..(W / 2 + 3) {
            let i = at(x, y);
            input[i..i + 3].copy_from_slice(&[255, 255, 255]);
        }
    }

    let mut post = PostSettings::default();
    post.enabled = true;
    post.bloom.enabled = true;
    post.bloom.threshold = 0.3;
    post.bloom.intensity = 1.0;
    post.bloom.radius = 1.0;

    let out = run(&mut gpu, &input, &post, 0);
    // A pixel well outside the block was black; bloom should have lifted it.
    let away = out[at(W / 2 + 10, H / 2)];
    assert!(away > 0, "bloom should bleed past the block's edge, was {away}");
}

#[test]
fn grain_is_deterministic_across_identical_renders() {
    let Some(mut gpu) = gpu() else { return };

    let input = solid(128, 128, 128);
    let mut post = PostSettings::default();
    post.enabled = true;
    post.grain.enabled = true;
    post.grain.amount = 0.3;

    let a = run(&mut gpu, &input, &post, 7);
    let b = run(&mut gpu, &input, &post, 7);
    assert_eq!(a, b, "the same frame index must grain identically");

    let c = run(&mut gpu, &input, &post, 8);
    assert_ne!(a, c, "a different frame index should grain differently");
}
