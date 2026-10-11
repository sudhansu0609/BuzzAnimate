//! H0 item 6 in `docs/IMPORT_PERF_PLAN.md`: **measure the exact app band
//! planner, GPU renders included.**
//!
//! `App::render` pays two different costs on a document too heavy for a
//! single Vello pass: once, on the frame it enters band mode, the whole-frame
//! ladder (`stage::build_scene`); every frame after that, while it stays in
//! band mode, `render_stage_bands` — which now means
//! [`buzz_app::app::plan_stage_bands`]'s recursive split-until-it-fits
//! planning (H0 item 2) plus one real GPU render and one real copy per leaf.
//! This times the second cost, the one the ~600 ms report was about, and
//! reads every leaf's own pixels back to check H0 item 1's fix actually
//! landed: a band the planner hands back must not be blank where the frame
//! genuinely has artwork.
//!
//! cargo run --release -p buzz-app --example stage_cost -- input.fla
//!
//! Loads a document into a headless [`Editor`] — no window — and sweeps
//! every 25th frame of its whole timeline at a 1600×900 stage, fit to zoom.
//! For each sampled frame:
//! - the whole-frame ladder decides, exactly as `App::render` does, whether
//!   this frame draws in one pass or in bands;
//! - a frame that fits in one pass is timed as that one GPU render;
//! - a frame that does not is planned with `plan_stage_bands`, and each leaf
//!   it returns is rendered into a scratch texture and copied into its own
//!   rows of the target, exactly `render_stage_bands`'s own dance — timed
//!   together, because that is the cost that repeats every frame while band
//!   mode holds;
//! - every leaf's rows are read back and compared against an unbanded
//!   reference render of the same frame: a row range with real artwork in
//!   the reference but only background in the leaf is a blank band.
//!
//! Reports the number of frames swept, the max and p95 ms, the worst frames,
//! the number of blank bands found (must be 0) and the largest band count
//! used in any one frame.

use std::time::{Duration, Instant};

use buzz_app::app::plan_stage_bands;
use buzz_app::editor::Editor;
use buzz_app::stage;
use buzz_doc::Document;
use buzz_geom::{Rect, Size};
use buzz_render::document::{DrawCache, line_budget, segment_ceiling};
use buzz_render::{GpuContext, GpuPreference, wgpu};
use peniko::Color;

/// The output size H0's own numbers are measured at.
const STAGE_W: u32 = 1600;
const STAGE_H: u32 = 900;
/// Every 25th frame — H0 item 6's own sweep density.
const STRIDE: u32 = 25;
const BACKGROUND: Color = Color::from_rgb8(0x2A, 0x2A, 0x2E);
/// The worst frames printed at the end, alongside the summary — the same
/// shape P0's own sweep reports.
const WORST_KEPT: usize = 10;

/// One GPU, three textures: `target` is what a frame's picture ends up in —
/// the single pass writes it directly, band mode copies each leaf's rows into
/// it, exactly `App::render`'s own `target`/`scratch` pair — and `reference`
/// holds the unbanded render of the same frame to check leaves against, kept
/// separate so building it never disturbs the picture being checked.
struct Gpu {
    ctx: GpuContext,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    scratch: wgpu::Texture,
    scratch_view: wgpu::TextureView,
    reference: wgpu::Texture,
    reference_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    /// The clear colour's own raw bytes, read back once from an empty render
    /// — see [`Gpu::background_pixel`] — rather than assumed, so the blank
    /// check does not depend on guessing `Rgba8Unorm`'s byte order.
    background_pixel: [u8; 4],
}

impl Gpu {
    fn new() -> anyhow::Result<Self> {
        let ctx = GpuContext::new_blocking(&GpuPreference::Automatic)
            .map_err(|e| anyhow::anyhow!("no usable GPU: {e}"))?;
        let make = |label: &str| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: STAGE_W,
                    height: STAGE_H,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: buzz_render::RENDER_FORMAT,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let target = make("stage-cost-target");
        let scratch = make("stage-cost-scratch");
        let reference = make("stage-cost-reference");
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let scratch_view = scratch.create_view(&wgpu::TextureViewDescriptor::default());
        let reference_view = reference.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stage-cost-readback"),
            size: u64::from(STAGE_W * STAGE_H * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut gpu = Self {
            ctx,
            target,
            target_view,
            scratch,
            scratch_view,
            reference,
            reference_view,
            readback,
            background_pixel: [0; 4],
        };
        gpu.ctx
            .render(
                &vello::Scene::new(),
                &gpu.target_view,
                STAGE_W,
                STAGE_H,
                BACKGROUND,
            )
            .expect("clear render");
        let empty = gpu.read(&gpu.target);
        gpu.background_pixel = [empty[0], empty[1], empty[2], empty[3]];
        Ok(gpu)
    }

    /// The ordinary single pass: `vello` rendered straight into `target`.
    fn render_whole(&mut self, vello: &vello::Scene) -> Vec<u8> {
        self.ctx
            .render(vello, &self.target_view, STAGE_W, STAGE_H, BACKGROUND)
            .expect("vello render");
        self.read(&self.target)
    }

    /// The reference render for the blank check: `vello` into `reference`,
    /// left untouched by anything band mode does to `target`.
    fn render_reference(&mut self, vello: &vello::Scene) -> Vec<u8> {
        self.ctx
            .render(vello, &self.reference_view, STAGE_W, STAGE_H, BACKGROUND)
            .expect("vello render");
        self.read(&self.reference)
    }

    /// One band leaf: rendered into `scratch`, then copied into its own rows
    /// of `target` — `render_stage_bands`'s own two-texture dance.
    fn render_band(&mut self, vello: &vello::Scene, y0: u32, y1: u32) {
        self.ctx
            .render(vello, &self.scratch_view, STAGE_W, STAGE_H, BACKGROUND)
            .expect("vello render");
        if y1 <= y0 {
            return;
        }
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("stage-cost-band-copy"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.scratch,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: STAGE_W,
                height: y1 - y0,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit([encoder.finish()]);
    }

    fn read(&self, texture: &wgpu::Texture) -> Vec<u8> {
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(STAGE_W * 4),
                    rows_per_image: Some(STAGE_H),
                },
            },
            wgpu::Extent3d {
                width: STAGE_W,
                height: STAGE_H,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit([encoder.finish()]);
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        self.ctx
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .expect("poll device");
        let pixels = {
            let view = slice.get_mapped_range();
            view.to_vec()
        };
        self.readback.unmap();
        pixels
    }

    /// Does the row range `[y0, y1)` hold any pixel other than the clear
    /// colour? `false` for a leaf that rasterised nothing there.
    fn row_range_has_content(&self, pixels: &[u8], y0: u32, y1: u32) -> bool {
        let stride = (STAGE_W * 4) as usize;
        let (y0, y1) = (y0.min(STAGE_H), y1.min(STAGE_H));
        pixels[y0 as usize * stride..y1 as usize * stride]
            .chunks_exact(4)
            .any(|p| p != self.background_pixel)
    }
}

/// Render one frame the way `App::render` does — a single pass, or bands —
/// and report `(ms, bands, blank_bands_here)`. Shared by the main sweep and
/// P2's own consecutive-stretch pass (`docs/IMPORT_PERF_PLAN.md`) so the two
/// time exactly the same work.
fn render_one(
    editor: &mut Editor,
    gpu: &mut Gpu,
    cache: &mut DrawCache,
    area: Rect,
    ceiling: u32,
    frame: u32,
) -> (f64, u32, u32) {
    editor.current_frame = frame;
    let mut whole = vello::Scene::new();
    stage::build_scene(&mut whole, editor, area, 1.0, cache);
    let needs_bands = cache.over_budget();
    if !needs_bands {
        let render_started = Instant::now();
        gpu.render_whole(&whole);
        return (render_started.elapsed().as_secs_f64() * 1e3, 1, 0);
    }

    let n = cache.bands_needed();
    let plan_started = Instant::now();
    let planned = plan_stage_bands(editor, cache, area, 1.0, ceiling, STAGE_W, STAGE_H, n);
    for band in &planned {
        let (y0, y1) = (band.y0.round() as u32, band.y1.round() as u32);
        gpu.render_band(&band.scene, y0, y1);
    }
    let ms = plan_started.elapsed().as_secs_f64() * 1e3;
    let bands = planned.len() as u32;

    let banded_pixels = gpu.read(&gpu.target);
    let reference_pixels = gpu.render_reference(&whole);
    let mut blank_here = 0u32;
    for band in &planned {
        let (y0, y1) = (band.y0.round() as u32, band.y1.round() as u32);
        if gpu.row_range_has_content(&reference_pixels, y0, y1)
            && !gpu.row_range_has_content(&banded_pixels, y0, y1)
        {
            blank_here += 1;
            eprintln!(
                "frame {frame:>5}: band [{y0}, {y1}) is blank where the reference has pixels"
            );
        }
    }
    (ms, bands, blank_here)
}

/// The middle of a sorted, non-empty `[f64]`. Even length averages the two
/// middle values.
fn median(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: stage_cost <file.fla>"))?;

    let started = Instant::now();
    let imported =
        buzz_app::import::read(std::path::Path::new(&input)).map_err(anyhow::Error::msg)?;
    println!(
        "import: {:.0} ms — {}",
        started.elapsed().as_secs_f64() * 1e3,
        imported.summary
    );

    let mut editor = Editor::new(Document::new(imported.scene));
    editor.camera.viewport = Size::new(f64::from(STAGE_W), f64::from(STAGE_H));
    let stage_rect = editor.scene().stage().stage_rect();
    editor.camera.fit_to_rect(stage_rect, 1.0);
    let area = Rect::new(0.0, 0.0, f64::from(STAGE_W), f64::from(STAGE_H));
    let total_frames = editor.scene().frame_count();

    let mut gpu = Gpu::new()?;
    // One retained cache for the whole sweep, exactly as `App` keeps one for
    // the life of the window — so a symbol stamped on one sampled frame is
    // still in the cache, warm, the next.
    let mut cache = DrawCache::default();
    // P2 (`docs/IMPORT_PERF_PLAN.md`): the budget belongs to this cache, not
    // the process — adopt the scale `Gpu::new` above just picked from the
    // real adapter, exactly as `App::init` and `Exporter::new` do, so this
    // sweep measures the same budget the window actually renders against
    // rather than `DrawCache::default()`'s deterministic scale of `1`.
    cache.set_scale(gpu.ctx.buffer_scale);
    // Computed only now, after `Gpu::new` has raised the process-wide scale
    // (P1) — `segment_ceiling` reads it at call time, so calling this any
    // earlier would report Vello's unscaled ceiling instead of the one this
    // sweep's own GPU actually got.
    let ceiling = segment_ceiling(f64::from(STAGE_W) * f64::from(STAGE_H));

    let mut sampled = 0u32;
    // `(frame, ms, bands, single_pass_line_estimate)` — P1 in
    // `docs/IMPORT_PERF_PLAN.md`: the line estimate is reported for every
    // sampled frame, banded or not, so which frames actually need bands is
    // visible rather than inferred from timing alone.
    let mut ms_by_frame: Vec<(u32, f64, u32, u64)> = Vec::new();
    let mut blank_bands = 0u32;
    let mut max_bands = 0u32;
    let mut frames_needing_bands = 0u32;
    // P2 (`docs/IMPORT_PERF_PLAN.md`): a "first-appearance" frame is one
    // that paid at least one cold encode — a layer or a symbol this cache
    // had not built before — rather than stamping fragments already warm.
    // The target is that none of these costs more than 1.3x the swept
    // median.
    let mut first_appearance_ms: Vec<f64> = Vec::new();

    let mut frame = 0u32;
    while frame < total_frames {
        sampled += 1;
        let (layer_builds_before, _) = cache.layer_cache_stats();
        let symbol_builds_before = cache.symbol_scenes.builds;

        let (ms, bands, blank_here) = render_one(&mut editor, &mut gpu, &mut cache, area, ceiling, frame);
        let lines_estimate = cache.last_lines();
        if cache.over_budget() {
            frames_needing_bands += 1;
        }
        max_bands = max_bands.max(bands);
        blank_bands += blank_here;
        ms_by_frame.push((frame, ms, bands, lines_estimate));

        let (layer_builds_after, _) = cache.layer_cache_stats();
        let symbol_builds_after = cache.symbol_scenes.builds;
        if layer_builds_after > layer_builds_before || symbol_builds_after > symbol_builds_before {
            first_appearance_ms.push(ms);
        }

        frame += STRIDE;
    }

    let mut sorted_ms: Vec<f64> = ms_by_frame.iter().map(|(_, ms, _, _)| *ms).collect();
    sorted_ms.sort_by(|a, b| a.total_cmp(b));
    let max_ms = sorted_ms.last().copied().unwrap_or(0.0);
    let median_ms = median(&sorted_ms);
    let p95_ms = sorted_ms
        .get(((sorted_ms.len() as f64) * 0.95) as usize)
        .or(sorted_ms.last())
        .copied()
        .unwrap_or(0.0);
    let first_appearance_max_ms = first_appearance_ms.iter().cloned().fold(0.0, f64::max);
    let first_appearance_max_ratio = if median_ms > 0.0 {
        first_appearance_max_ms / median_ms
    } else {
        0.0
    };

    let mut worst = ms_by_frame.clone();
    worst.sort_by(|a, b| b.1.total_cmp(&a.1));
    worst.truncate(WORST_KEPT);

    println!("frames swept: {sampled} (every {STRIDE}th of {total_frames})");
    println!("max ms: {max_ms:.2}");
    println!("median ms: {median_ms:.2}");
    println!("p95 ms: {p95_ms:.2}");
    println!("blank bands: {blank_bands} (must be 0)");
    println!("max bands: {max_bands}");
    // P1 in `docs/IMPORT_PERF_PLAN.md`: the Vello buffer scale actually in
    // use, and how many sampled frames still needed bands at it — the
    // number H0's bigger buffers are meant to drive toward 0.
    println!(
        "vello buffer scale: {}x (line budget {})",
        buzz_render::vello_encoding::buffer_scale(),
        line_budget()
    );
    println!("frames needing bands: {frames_needing_bands} of {sampled}");
    println!(
        "first-appearance frames: {} of {sampled}, worst {first_appearance_max_ms:.2} ms \
         ({first_appearance_max_ratio:.2}x median, target <= 1.30x)",
        first_appearance_ms.len()
    );
    println!(
        "layer cache: {} builds, {} stamps",
        cache.layer_cache_stats().0,
        cache.layer_cache_stats().1
    );
    println!("worst frames:");
    for (frame, ms, bands, lines_estimate) in &worst {
        println!(
            "  frame {frame:>5}: {ms:>8.2} ms, {bands} bands, single-pass estimate {lines_estimate} lines"
        );
    }

    // P2 (`docs/IMPORT_PERF_PLAN.md`): a consecutive stretch — every frame,
    // not every 25th — which is what real playback does, with steady-state
    // and first-frame numbers reported separately. 1880..=1960, clamped to
    // this document's own length.
    let stretch_start = 1880u32.min(total_frames.saturating_sub(1));
    let stretch_end = 1960u32.min(total_frames.saturating_sub(1));
    if stretch_end > stretch_start {
        let mut stretch_ms: Vec<(u32, f64)> = Vec::new();
        for f in stretch_start..=stretch_end {
            let (ms, _bands, blank_here) =
                render_one(&mut editor, &mut gpu, &mut cache, area, ceiling, f);
            blank_bands += blank_here;
            stretch_ms.push((f, ms));
        }
        let first_ms = stretch_ms.first().map(|(_, ms)| *ms).unwrap_or(0.0);
        let mut rest: Vec<f64> = stretch_ms[1.min(stretch_ms.len())..]
            .iter()
            .map(|(_, ms)| *ms)
            .collect();
        rest.sort_by(|a, b| a.total_cmp(b));
        let steady_ms = median(&rest);
        println!(
            "consecutive {stretch_start}-{stretch_end}: first {first_ms:.2} ms, steady (median) \
             {steady_ms:.2} ms"
        );
        println!("blank bands total (sweep + consecutive stretch): {blank_bands} (must be 0)");
    } else {
        println!(
            "consecutive 1880-1960: skipped, document has only {total_frames} frames"
        );
    }

    Ok(())
}
