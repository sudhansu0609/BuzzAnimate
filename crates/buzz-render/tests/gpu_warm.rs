//! S7 in `docs/BIMARI5_LAG_PLAN.md`: `GpuContext::keep_warm` starts the
//! warmer's submissions, they stop by themselves once the touches stop, and
//! the picture a render produces while it runs is unchanged.
//!
//! Skips rather than fails without a GPU, like the other tests here.

use std::time::{Duration, Instant};

use buzz_render::{GpuContext, GpuPreference, RENDER_FORMAT, warm, wgpu};

fn gpu() -> Option<GpuContext> {
    match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping gpu warm test: no usable GPU ({e})");
            None
        }
    }
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn touching_starts_the_warmer_and_it_stops_by_itself() {
    if !warm::enabled() {
        return;
    }
    let Some(mut gpu) = gpu() else { return };
    assert!(gpu.warmer().is_none(), "no thread until the first touch");
    gpu.keep_warm();
    let warmer = gpu.warmer().expect("made by the first touch");
    assert!(warmer.is_active());
    wait_for("the first submission", || warmer.submits() > 0);
    // Touches stopped: after KEEP_WARM_FOR it is inactive and submits no more.
    wait_for("the warmer to stop", || !warmer.is_active());
    std::thread::sleep(Duration::from_millis(100));
    let settled = warmer.submits();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(warmer.submits(), settled, "it kept submitting after its time was up");
}

#[test]
fn a_render_while_warming_is_unchanged() {
    let Some(mut gpu) = gpu() else { return };
    let (w, h) = (64u32, 64u32);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu-warm-target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: RENDER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut scene = buzz_render::vello::Scene::new();
    scene.fill(
        peniko::Fill::NonZero,
        kurbo::Affine::IDENTITY,
        peniko::Color::from_rgba8(200, 30, 30, 255),
        None,
        &kurbo::Rect::new(8.0, 8.0, 40.0, 50.0),
    );
    let mut pictures = Vec::new();
    for warm_on in [false, true] {
        if warm_on {
            gpu.keep_warm();
        }
        gpu.render(&scene, &view, w, h, peniko::Color::TRANSPARENT).expect("render");
        pictures.push(read_back(&gpu, &target, w, h));
    }
    assert_eq!(pictures[0], pictures[1], "warming changed the picture");
    assert!(pictures[0].iter().any(|b| *b != 0), "the render drew nothing");
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
    let bytes_per_row = (w * 4).next_multiple_of(256);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gpu-warm-readback"),
        size: u64::from(bytes_per_row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(10)) })
        .expect("poll");
    let data = slice.get_mapped_range().to_vec();
    data
}
