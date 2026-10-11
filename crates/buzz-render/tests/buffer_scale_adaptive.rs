//! `GpuContext`'s own live buffer scale — V2/V4 in
//! `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`. `crates/buzz-render/src/document.rs`
//! (`DrawCache::reconsider`/`note_shrink_headroom`) has the pure, no-GPU unit
//! tests for *when* to grow or shrink; this proves the `GpuContext` side —
//! `grow_to`, `start_generous` and `shrink_to` — actually changes the live
//! scale, clamps to the adapter's own max, and keeps rendering working after
//! a shrink (which rebuilds the whole `Renderer`, see `shrink_to`'s doc).
//!
//! Skips rather than fails without a GPU, exactly as
//! `buzz-app/tests/stage_lighting.rs` does.

use buzz_render::{GpuContext, GpuPreference, RENDER_FORMAT, wgpu};

fn gpu() -> Option<GpuContext> {
    match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping buffer-scale test: no usable GPU ({e})");
            None
        }
    }
}

/// A one-pixel offscreen target, just big enough to render a blank scene
/// into — these tests are about the scale bookkeeping, not the picture.
fn target(gpu: &GpuContext) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("buffer-scale-adaptive-target"),
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: RENDER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// V2: a freshly opened `GpuContext` starts at Vello's own upstream buffer
/// sizes, not the adapter's largest — the whole point of this plan.
#[test]
fn a_fresh_context_starts_at_scale_one() {
    let Some(gpu) = gpu() else { return };
    assert_eq!(gpu.buffer_scale, 1, "must not reserve max_buffer_scale for a blank stage");
    assert!(gpu.max_buffer_scale >= 1, "the adapter's own ceiling is always at least 1");
}

/// V3: `grow_to` raises the live scale, clamped to `max_buffer_scale`, and
/// never lowers it.
#[test]
fn grow_to_rises_and_clamps_to_max_buffer_scale() {
    let Some(mut gpu) = gpu() else { return };
    let max = gpu.max_buffer_scale;

    gpu.grow_to(2);
    assert_eq!(gpu.buffer_scale, 2.min(max));

    // Asking for far more than the adapter allows clamps rather than panics
    // or overshoots the binding limits `pick_buffer_scale` measured.
    gpu.grow_to(1_000_000);
    assert_eq!(gpu.buffer_scale, max, "must clamp to max_buffer_scale");

    // Asking for less than the current live scale is a no-op — `grow_to`
    // only ever rises; `shrink_to` is the one call that lowers it.
    gpu.grow_to(1);
    assert_eq!(gpu.buffer_scale, max, "grow_to must never lower the live scale");
}

/// V2/V5: an offline context (the exporter, a headless render) calls this
/// once, right after construction, to start generous rather than adapting.
#[test]
fn start_generous_jumps_straight_to_max_buffer_scale() {
    let Some(mut gpu) = gpu() else { return };
    gpu.start_generous();
    assert_eq!(gpu.buffer_scale, gpu.max_buffer_scale);
}

/// V4: `shrink_to` lowers the live scale (clamped to `[1, max_buffer_scale]`)
/// and leaves the context able to render — the rebuilt `Renderer` works,
/// it is not merely swapped for a broken one.
#[test]
fn shrink_to_lowers_the_scale_and_rendering_still_works() {
    let Some(mut gpu) = gpu() else { return };
    gpu.start_generous();

    let (_texture, view) = target(&gpu);
    let scene = buzz_render::vello::Scene::new();
    gpu.render(&scene, &view, 4, 4, peniko::Color::TRANSPARENT)
        .expect("render before shrinking");

    // If this adapter's `max_buffer_scale` is `1`, `start_generous` above
    // already left `buffer_scale` at `1` and this is a no-op — still worth
    // asserting the render below still works either way.
    gpu.shrink_to(1).expect("rebuilding the renderer at a smaller scale");
    assert_eq!(gpu.buffer_scale, 1);

    // A fresh render target: the first one may already be in a state Vello
    // does not promise after a renderer rebuild mid-frame, but a new call
    // with a fresh target is exactly what the next real frame does.
    let (_texture2, view2) = target(&gpu);
    gpu.render(&scene, &view2, 4, 4, peniko::Color::TRANSPARENT)
        .expect("render after shrinking must still work");

    // Asking for more than `max_buffer_scale` clamps rather than overshoots.
    gpu.shrink_to(1_000_000).expect("clamping rather than failing");
    assert_eq!(gpu.buffer_scale, gpu.max_buffer_scale);
}

/// V4 in the plan's own words: shrinking must actually let the pooled
/// buffers go, not just stop asking for them — measured, where this backend
/// reports it, through `wgpu::Device::generate_allocator_report`. Skips its
/// own assertion (rather than the whole test) when the backend does not
/// implement allocator reporting, or when this adapter never had anywhere
/// to shrink from (`max_buffer_scale` of `1`).
#[test]
fn shrink_to_gives_transient_memory_back_where_the_backend_can_report_it() {
    let Some(mut gpu) = gpu() else { return };
    let max = gpu.max_buffer_scale;
    if max <= 1 {
        eprintln!("skipping: this adapter's max_buffer_scale is 1; nothing to shrink from");
        return;
    }

    gpu.start_generous();
    let (_texture, view) = target(&gpu);
    let scene = buzz_render::vello::Scene::new();
    // Render a few times so Vello's resource pool actually allocates and
    // retains the grown-scale buffers rather than answering from an empty
    // pool once.
    for _ in 0..3 {
        gpu.render(&scene, &view, 4, 4, peniko::Color::TRANSPARENT)
            .expect("render at max scale");
    }

    // `Device::poll` is what actually reclaims a dropped buffer's memory on
    // most backends — wgpu-core only returns a destroyed resource to the
    // allocator once it has confirmed (through the submission it last used)
    // that the GPU is done with it, and that confirmation happens during a
    // poll, not synchronously on drop. Without this, "before" would still
    // include buffers this same maintenance cycle was already going to free
    // regardless of whether anything shrank.
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll before measuring");
    let Some(before) = gpu.device.generate_allocator_report() else {
        eprintln!("skipping: this backend does not implement generate_allocator_report");
        return;
    };

    gpu.shrink_to(1).expect("rebuilding the renderer at scale 1");
    // The old, grown-scale `Renderer` (and its pool) was just dropped inside
    // `shrink_to` — give wgpu a maintenance cycle to actually reclaim it
    // before rendering again (which would otherwise let a fresh, smaller
    // allocation coexist with the not-yet-reclaimed old one and make this
    // assertion fail for a reason that has nothing to do with whether the
    // shrink itself worked).
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll after the shrink");
    let (_texture2, view2) = target(&gpu);
    gpu.render(&scene, &view2, 4, 4, peniko::Color::TRANSPARENT)
        .expect("render after shrinking");
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll before measuring again");

    let after = gpu
        .device
        .generate_allocator_report()
        .expect("the same backend must keep reporting after a renderer rebuild");

    eprintln!(
        "allocator report: before={} MB after={} MB",
        before.total_allocated_bytes / (1024 * 1024),
        after.total_allocated_bytes / (1024 * 1024),
    );
    assert!(
        after.total_allocated_bytes <= before.total_allocated_bytes,
        "shrinking and rebuilding the renderer must not leave more allocated than before \
         (before={}, after={})",
        before.total_allocated_bytes,
        after.total_allocated_bytes,
    );
}
