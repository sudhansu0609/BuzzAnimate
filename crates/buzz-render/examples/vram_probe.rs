//! V1 in `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`: how much transient GPU memory
//! a blank document actually reserves, at startup and after the first frame.
//!
//! cargo run -p buzz-render --example vram_probe
//!
//! Prints `wgpu::Device::generate_allocator_report()`'s totals (bytes
//! actually allocated by this device, across every buffer and texture it
//! owns — the swapchain and egui's own textures are not part of this
//! process, since neither exists here) right after `GpuContext::new` and
//! again after rendering one empty `vello::Scene`, so the before/after this
//! plan asks for can be read straight from the log rather than eyeballed in
//! a task manager. `None` (some backends do not implement allocator
//! reporting) is reported as such rather than a wrong number.
use buzz_render::{GpuContext, GpuPreference, RENDER_FORMAT, wgpu};

fn report(gpu: &GpuContext, label: &str) {
    match gpu.device.generate_allocator_report() {
        Some(r) => {
            println!(
                "{label}: buffer_scale={} max_buffer_scale={} allocated={:.1} MB reserved={:.1} MB ({} allocations, {} blocks)",
                gpu.buffer_scale,
                gpu.max_buffer_scale,
                r.total_allocated_bytes as f64 / (1024.0 * 1024.0),
                r.total_reserved_bytes as f64 / (1024.0 * 1024.0),
                r.allocations.len(),
                r.blocks.len(),
            );
        }
        None => println!(
            "{label}: buffer_scale={} max_buffer_scale={} (this backend does not implement generate_allocator_report)",
            gpu.buffer_scale, gpu.max_buffer_scale
        ),
    }
}

fn main() -> anyhow::Result<()> {
    let mut gpu = GpuContext::new_blocking(&GpuPreference::Automatic)?;
    println!("adapter: {}", gpu.adapter_summary());
    report(&gpu, "startup (before any frame)");

    // An ordinary full-HD blank stage — the case this plan's title is about.
    let (width, height) = (1920u32, 1080u32);
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vram-probe"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: RENDER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let scene = buzz_render::vello::Scene::new();
    gpu.render(&scene, &view, width, height, peniko::Color::TRANSPARENT)?;
    gpu.device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(std::time::Duration::from_secs(10)),
    })?;
    report(&gpu, "after first frame (blank scene)");

    // V5: the one-frame hitch a real grow costs — `GpuContext::grow_to`
    // itself is just an integer store, so the cost is entirely in the next
    // `render` call actually asking Vello for the bigger buffers the grown
    // scale needs. Measured against a steady-state render already at that
    // scale, so the number is the grow's own cost, not just this scale's
    // ordinary per-frame time on this adapter.
    if gpu.max_buffer_scale > 1 {
        let warm = std::time::Instant::now();
        gpu.render(&scene, &view, width, height, peniko::Color::TRANSPARENT)?;
        gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })?;
        let warm_ms = warm.elapsed().as_secs_f64() * 1e3;

        gpu.grow_to(gpu.max_buffer_scale);
        let grow = std::time::Instant::now();
        gpu.render(&scene, &view, width, height, peniko::Color::TRANSPARENT)?;
        gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })?;
        let grow_ms = grow.elapsed().as_secs_f64() * 1e3;
        report(&gpu, "after growing to max_buffer_scale");

        let steady = std::time::Instant::now();
        gpu.render(&scene, &view, width, height, peniko::Color::TRANSPARENT)?;
        gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })?;
        let steady_ms = steady.elapsed().as_secs_f64() * 1e3;

        println!(
            "grow hitch: at scale 1 {warm_ms:.2} ms, growing frame {grow_ms:.2} ms, steady at max scale {steady_ms:.2} ms (hitch \u{2248} {:.2} ms)",
            grow_ms - steady_ms
        );
    }

    Ok(())
}
