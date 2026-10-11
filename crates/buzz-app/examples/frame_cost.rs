//! S0 in `docs/BIMARI5_LAG_PLAN.md`: **measure the app, not a proxy.**
//!
//! `examples/stage_cost.rs` (left alone — see its own doc comment) predates
//! V2–V5 of `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`: it never grows the buffer
//! scale and judges every frame fresh, so it cannot see the one thing
//! Diagnosis §4 in the lag plan is actually about — that once `App::render`
//! enters band mode it can get *trapped* there, because `stage_bands > 0`
//! skips the whole-frame ladder (`stage::build_scene` → `DrawCache::reconsider`,
//! the only place the buffer scale grows) on every later frame, so a stage
//! that entered bands at scale 1 never learns that scale 4 or 8 would have
//! let it out.
//!
//! This example replays **the app's own decision, frame by frame, with one
//! `stage_bands` carried across frames exactly the way `App` carries
//! `self.stage_bands`** — not a fresh judgement each time. [`Runner::step`]
//! mirrors `App::render` line for line: the whole-frame ladder runs only
//! while not already banding (app.rs's `wants_whole_frame_ladder`); either
//! way `GpuContext::grow_to` is called once, unconditionally, before
//! dispatch (app.rs ~10972); bands are drawn with the same
//! `plan_stage_bands` recursive planner `App` uses, and band mode is left on
//! the same "comfortable" rule `render_stage_bands` uses (app.rs ~1886).
//! Texture setup and the render/copy dance for a band leaf are the same
//! pattern as `stage_cost.rs`'s own `Gpu`, minus the reference-render blank
//! check that example does and this one has no need of.
//!
//! ```text
//! cargo run --release -p buzz-app --example frame_cost -- <file.fla> [a|b|c|d|all]
//! cargo run --release -p buzz-app --example frame_cost -- <file.fla> --layers <frame>
//! cargo run --release -p buzz-app --example frame_cost -- <file.fla> [a|d] --pipelined
//! cargo run --release -p buzz-app --example frame_cost -- <file.fla> [a|d] --pipelined --warm-open
//! cargo run --release -p buzz-app --example frame_cost -- <file.fla> warm-measure
//! ```
//!
//! `--warm-open` (S9): before playback, run **the app's own warm-up**
//! (`buzz_app::warm_open::run_warm` + `apply`: whole film in playback order on
//! the worker pool, stamps retained, the buffers sized once) and report the open
//! time and the stamps it kept; `BUZZ_STAMP_CACHE_MB` sets its budget as in the
//! app. `warm-measure` runs the same code with one worker and no budget and
//! prints how many distinct stamps and bytes the whole film needs, and the
//! bytes the first N frames need (`FRAME_COST_WARM_WORKERS` for more workers).
//! Diagnosis knobs: `FRAME_COST_WARM_PLAYED=1` warms just the `FRAME_COST_FRAMES`
//! about to be played, `FRAME_COST_HEAVY=4608,4636,..` names the frames whose own
//! numbers are printed, `FRAME_COST_START_SCALE=8` starts at that buffer scale and
//! `FRAME_COST_BIG_CACHE=<entries>` lifts the stamp cache to 8 GiB / that many
//! entries, both without warming anything.
//!
//! `--pipelined` (E7 of the revised S5): scenarios `a` and `d` through the
//! encode thread (`encode_ahead::Pipeline` with the E6 pre-build jobs) on a
//! simulated clock of the film's own frame rate; `FRAME_COST_FRAMES=0-100,4500-4800`
//! limits the playback to those frame ranges (a repeated range is played again:
//! frames after their first appearance are reported separately). S7 knobs, all
//! diagnosis only: `FRAME_COST_DIAG=1` (device allocation per frame, the slow
//! GPU frames in order), `FRAME_COST_CSV=<path>` (one line per frame),
//! `FRAME_COST_STOP_EVERY=n` (an injected stop every n frames, with each stop's
//! steps printed), `FRAME_COST_B_SLEEP_MS` / `FRAME_COST_WARM=1` (scenario `b`
//! idles that long before each frame, optionally with the GPU warmer touched).
//! `BUZZ_NO_GPU_WARM=1`, `BUZZ_NO_PREBUILD=1` and `BUZZ_NO_ENCODE_AHEAD=1` give
//! the A/B runs.
//!
//! Scenarios (`docs/BIMARI5_LAG_PLAN.md` S0 item 2):
//! - `a`: every frame `0..frame_count`, in order — playback.
//! - `b`: scrub `0, 20, 715, 900, 4700, 20`.
//! - `c`: scenario `b` again at camera zoom 4× and 16×, centred on the stage
//!   (reported as `c4` and `c16`).
//! - `d`: idle shrink — play `0..=300`, then fake a 25 s idle wait via
//!   `DrawCache::note_shrink_headroom`'s own `now` parameter (no real wait)
//!   and follow `pending_shrink` down until the cache gives up scale or
//!   declines to offer any more, then jump to frames 20 and 900 to see what
//!   a shrink right before a heavy frame costs (Diagnosis §5: up to three
//!   wasted encodes as the ladder grows back).
//!
//! `--layers <frame>` is the diagnosis's own per-layer probe
//! (`docs/probes/zz_perf_probe.rs`, `bimari5_playback_and_layers`): a warm
//! encode of the full frame, then one per visible layer with just that layer
//! hidden (`Scene::update_layer`, a fresh `Editor`), reporting the delta —
//! pure CPU, no GPU, since `stage::build_scene` alone is what is measured.

use std::sync::Arc;
use std::time::{Duration, Instant};

use buzz_app::app::plan_stage_bands;
use buzz_app::editor::Editor;
use buzz_app::encode_ahead::{Epoch, Pipeline, PlayRange, SeekAhead, Stopped, Taken, step_frame};
use buzz_app::stage;
use buzz_doc::Document;
use buzz_geom::{Rect, Size};
use buzz_render::document::{DrawCache, line_budget_at, segment_ceiling_at};
use buzz_render::{GpuContext, GpuPreference, wgpu};
use buzz_scene::Scene;
use peniko::Color;

/// S0's own output size — matches `stage_cost.rs`.
const STAGE_W: u32 = 1600;
const STAGE_H: u32 = 900;
const BACKGROUND: Color = Color::from_rgb8(0x2A, 0x2A, 0x2E);
const WORST_KEPT: usize = 15;
/// `--layers` prints this many, top by hide-one-layer delta.
const LAYERS_KEPT: usize = 8;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Approximate heap size of an encoded scene — the same formula as the
/// private `scene_bytes` in `buzz-render/src/document.rs` (encoding streams'
/// lengths times their element sizes; late-bound resources not counted).
fn scene_bytes(scene: &vello::Scene) -> usize {
    let e = scene.encoding();
    std::mem::size_of_val(e.path_tags.as_slice())
        + std::mem::size_of_val(e.path_data.as_slice())
        + std::mem::size_of_val(e.draw_tags.as_slice())
        + std::mem::size_of_val(e.draw_data.as_slice())
        + std::mem::size_of_val(e.transforms.as_slice())
        + std::mem::size_of_val(e.styles.as_slice())
}

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

/// `try_stamp_symbol`'s refusal counters, snapshotted — S2 in
/// `docs/BIMARI5_LAG_PLAN.md`. `SymbolSceneCache::refusal_counts` never
/// resets on its own, so two snapshots around one warm encode, zipped, give
/// that one encode's own refusals.
fn print_refusal_delta(label: &str, before: &[(&'static str, u64)], after: &[(&'static str, u64)]) {
    print!("  refusals ({label}):");
    let mut any = false;
    for ((name, b), (_, a)) in before.iter().zip(after.iter()) {
        let d = a.saturating_sub(*b);
        if d > 0 {
            print!(" {name}={d}");
            any = true;
        }
    }
    if !any {
        print!(" none");
    }
    println!();
}

/// The stamp cache's retention state (S2-rest (c)): entries held, encoded
/// bytes against the budget (`BUZZ_STAMP_CACHE_MB`), evictions so far, and
/// first sightings drawn live instead of built (S2-rest (b)).
fn print_retention(label: &str, cache: &DrawCache) {
    let (entries, bytes, budget, evictions, first_sightings) =
        cache.symbol_scenes.retention_stats();
    println!(
        "  stamp cache ({label}): {entries} entries | {:.1} MiB of {:.0} MiB budget | \
         {evictions} evictions | {first_sightings} first sightings drawn live | \
         builds {} stamps {} (lit {})",
        bytes as f64 / (1024.0 * 1024.0),
        budget as f64 / (1024.0 * 1024.0),
        cache.symbol_scenes.builds,
        cache.symbol_scenes.stamps,
        cache.symbol_scenes.stamps_lit,
    );
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted
        .get(((sorted.len() as f64) * p) as usize)
        .or(sorted.last())
        .copied()
        .unwrap_or(0.0)
}

/// The input as a scene: a `.buzz` document gives its active scene (or the
/// one `FRAME_COST_SCENE=<index>` names); anything else goes through the
/// app's importer (.fla/.xfl/.swf/.pdf), exactly as before.
fn read_input(input: &str) -> anyhow::Result<buzz_app::import::Imported> {
    let path = std::path::Path::new(input);
    let is_buzz = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("buzz"));
    if !is_buzz {
        return buzz_app::import::read(path).map_err(anyhow::Error::msg);
    }
    let mut doc = Document::open(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let names = doc.scene_names();
    if let Some(i) = std::env::var("FRAME_COST_SCENE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        doc.switch_scene(i);
    }
    let mut scene = doc.scene().clone();
    // Diagnosis knobs for a lit document (RIG_SCENE_LAG_PLAN): the same scene
    // with its light rig switched off, or with the edge bands off, for A/B.
    if std::env::var("FRAME_COST_UNLIT").is_ok_and(|v| v == "1") {
        scene.lights_mut().enabled = false;
        println!("FRAME_COST_UNLIT: light rig disabled");
    }
    if std::env::var("FRAME_COST_EDGES_OFF").is_ok_and(|v| v == "1") {
        scene.lights_mut().edges = buzz_scene::EdgeMode::Off;
        println!("FRAME_COST_EDGES_OFF: edge bands off");
    }
    // `FRAME_COST_NO_RIM=1`: every light's rim set to 0 (what the staging
    // change of 2026-10-06 does for new set-up scenes), for the A/B on a
    // document built before it.
    if std::env::var("FRAME_COST_NO_RIM").is_ok_and(|v| v == "1") {
        for light in scene.lights_mut().lights.iter_mut() {
            light.rim = 0.0;
        }
        println!("FRAME_COST_NO_RIM: rim 0 on every light");
    }
    // `FRAME_COST_FLAT_SKY=1`: every sky's horizon set to its colour (what
    // `staging::light()` builds since L3'' of RIG_SCENE_LAG_PLAN), for the A/B
    // on a document built before it: a flat sky is position-independent light,
    // so the stamp cache works under it.
    if std::env::var("FRAME_COST_FLAT_SKY").is_ok_and(|v| v == "1") {
        for light in scene.lights_mut().lights.iter_mut() {
            let colour = light.color;
            if let buzz_scene::LightKind::Sky { horizon } = &mut light.kind {
                *horizon = colour;
            }
        }
        println!("FRAME_COST_FLAT_SKY: every sky's horizon = its colour");
    }
    // `FRAME_COST_ONE_LIGHT=1`: keep only the key (the first sun or lamp), so
    // the cost of the fill/sky light can be measured on its own.
    if std::env::var("FRAME_COST_ONE_LIGHT").is_ok_and(|v| v == "1") {
        let rig = scene.lights_mut();
        let key = rig.lights.iter().position(|l| {
            matches!(
                l.kind,
                buzz_scene::LightKind::Sun { .. } | buzz_scene::LightKind::Lamp { .. }
            )
        });
        if let Some(i) = key {
            let keep = rig.lights[i].clone();
            rig.lights = vec![keep];
        }
        println!("FRAME_COST_ONE_LIGHT: {} light(s) kept", rig.lights.len());
    }
    if let Ok(mode) = std::env::var("FRAME_COST_EDGES") {
        scene.lights_mut().edges = match mode.as_str() {
            "off" => buzz_scene::EdgeMode::Off,
            "shapes" => buzz_scene::EdgeMode::Shapes,
            _ => buzz_scene::EdgeMode::Figure,
        };
        println!("FRAME_COST_EDGES: {:?}", scene.lights_mut().edges);
    }
    Ok(buzz_app::import::Imported {
        scene,
        summary: format!(".buzz document, {} scene(s): {names:?}", names.len()),
        unsupported: Vec::new(),
        scene_name: None,
        more_scenes: Vec::new(),
    })
}

/// A headless `Editor` on `scene`, at the stage rect, 1600×900 — the same
/// setup `stage_cost.rs` and `docs/probes/zz_perf_probe.rs` both use.
fn framed_editor(scene: Scene) -> Editor {
    let mut e = Editor::new(Document::new(scene));
    e.camera.viewport = Size::new(f64::from(STAGE_W), f64::from(STAGE_H));
    let stage_rect = e.scene().stage().stage_rect();
    e.camera.fit_to_rect(stage_rect, 1.0);
    e
}

/// Pure CPU: one `stage::build_scene` encode, timed. Used only by `--layers`,
/// which never touches a GPU — see the module doc.
fn encode_ms(editor: &Editor, cache: &mut DrawCache) -> f64 {
    let area = Rect::new(0.0, 0.0, f64::from(STAGE_W), f64::from(STAGE_H));
    let mut scene = vello::Scene::new();
    let t = Instant::now();
    stage::build_scene(&mut scene, editor, area, 1.0, cache);
    ms(t)
}

/// One GPU, two textures — `target` and `scratch`, exactly `stage_cost.rs`'s
/// own pair — minus the reference-render blank check that example does:
/// this one measures cost, not correctness.
struct Gpu {
    ctx: GpuContext,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    scratch: wgpu::Texture,
    scratch_view: wgpu::TextureView,
}

impl Gpu {
    fn new() -> anyhow::Result<Self> {
        // **Starts at scale 1, exactly `App::resumed` — see `GpuContext::new`.**
        // Not `start_generous`: S0 is about the app's own ladder, which only
        // ever grows on demand.
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
        let target = make("frame-cost-target");
        let scratch = make("frame-cost-scratch");
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let scratch_view = scratch.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            ctx,
            target,
            target_view,
            scratch,
            scratch_view,
        })
    }

    /// The ordinary single pass, straight into `target` — `App::render`'s
    /// `active.gpu.render(&active.vello, ...)`.
    fn render_whole(&mut self, scene: &vello::Scene) {
        self.ctx
            .render(scene, &self.target_view, STAGE_W, STAGE_H, BACKGROUND)
            .expect("vello render");
    }

    /// One band leaf: rendered into `scratch`, then copied into its own rows
    /// of `target` — `render_stage_bands`'s own two-texture dance.
    fn render_band(&mut self, scene: &vello::Scene, y0: u32, y1: u32) {
        self.ctx
            .render(scene, &self.scratch_view, STAGE_W, STAGE_H, BACKGROUND)
            .expect("vello render");
        if y1 <= y0 {
            return;
        }
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-cost-band-copy"),
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

    /// Wait for everything submitted so far to actually finish — the other
    /// half of "GPU ms", alongside submission itself.
    fn poll(&self) {
        self.ctx
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .expect("poll device");
    }
}

/// One frame's measured cost and what the state machine did with it.
#[derive(Debug, Clone)]
struct FrameRecord {
    frame: u32,
    encode_ms: f64,
    /// `render_call_ms + gpu_wait_ms`, as before E0.
    gpu_ms: f64,
    /// E0: CPU time until the render call(s) returned (Vello's own
    /// resolve/pack, buffer uploads, submit) — before any waiting for the GPU.
    render_call_ms: f64,
    /// E0: time in `device.poll(Wait)` after the render call(s) returned.
    gpu_wait_ms: f64,
    /// E0: vello encoding size in bytes (sum of the frame's scene(s)).
    enc_bytes: usize,
    /// Bands actually drawn this frame — `0` for a single-pass frame.
    bands: u32,
    /// The live `GpuContext` buffer scale after this frame.
    scale: u32,
    lines: u64,
    /// Did `GpuContext::grow_to` actually raise the scale this frame?
    grew: bool,
}

/// The app's own render loop, replayed: one `stage_bands` carried across
/// calls to [`Self::step`], exactly `App` carries `self.stage_bands` across
/// frames. See the module doc for the app.rs line ranges this mirrors.
struct Runner {
    gpu: Gpu,
    cache: DrawCache,
    stage_bands: u32,
    area: Rect,
}

impl Runner {
    fn new() -> anyhow::Result<Self> {
        let gpu = Gpu::new()?;
        let mut cache = DrawCache::default();
        // App-faithful: the ceiling this cache may grow towards comes from
        // the real adapter (`App::resumed`'s `self.lights.set_max_scale`),
        // but the *live* scale starts at `DrawCache::default`'s `1` and only
        // ever moves via `reconsider`/`commit_shrink` — deliberately never
        // `cache.set_scale` here, which would skip the very ladder S0 is
        // measuring.
        cache.set_max_scale(gpu.ctx.max_buffer_scale);
        // Same escape hatch `App::new` honours: a live-draw baseline.
        if std::env::var("BUZZ_NO_SYMBOL_CACHE").is_ok() {
            cache.set_symbol_reuse(false);
        }
        let area = Rect::new(0.0, 0.0, f64::from(STAGE_W), f64::from(STAGE_H));
        let mut gpu = gpu;
        // S9 diagnosis: `FRAME_COST_START_SCALE=8` starts at that buffer scale
        // (what sizing once does) without warming anything.
        if let Some(s) = std::env::var("FRAME_COST_START_SCALE").ok().and_then(|v| v.parse::<u32>().ok()) {
            gpu.ctx.grow_to(s);
            cache.set_scale(gpu.ctx.buffer_scale);
        }
        // S9 diagnosis: `FRAME_COST_BIG_CACHE=<entries>` lifts the stamp cache's
        // limits as a warm-up does (8 GiB, that many entries) without warming.
        if let Some(n) = std::env::var("FRAME_COST_BIG_CACHE").ok().and_then(|v| v.parse::<usize>().ok()) {
            cache.set_stamp_limits(8usize << 30, n);
        }
        Ok(Self {
            gpu,
            cache,
            stage_bands: 0,
            area,
        })
    }

    /// One frame, `App::render`'s own decision (app.rs ~10740-10800,
    /// ~10960-11000): the whole-frame ladder runs only while not already
    /// banding; either way the `GpuContext` is grown to the cache's scale
    /// once, unconditionally, before dispatch; bands are drawn with
    /// `plan_stage_bands` and band mode is left on `render_stage_bands`'s own
    /// "comfortable" rule (app.rs ~1886).
    fn step(&mut self, editor: &Editor) -> FrameRecord {
        let banding_already = self.stage_bands > 0;
        let scale_before = self.gpu.ctx.buffer_scale;

        let mut encode_ms = 0.0;
        let mut whole_scene = None;
        let mut enc_bytes = 0usize;
        if !banding_already {
            let mut scene = vello::Scene::new();
            let t = Instant::now();
            stage::build_scene(&mut scene, editor, self.area, 1.0, &mut self.cache);
            encode_ms += ms(t);
            if self.cache.over_budget() {
                self.stage_bands = self.cache.bands_needed();
            }
            enc_bytes = scene_bytes(&scene);
            whole_scene = Some(scene);
        }

        // Unconditional — app.rs's own `active.gpu.grow_to(self.lights.scale())`,
        // called once regardless of which branch above ran. A no-op while
        // banding, because bands never move `cache.scale()` — that is
        // Diagnosis §4's trap.
        self.gpu.ctx.grow_to(self.cache.scale());

        let mut render_call_ms = 0.0;
        let mut gpu_wait_ms = 0.0;
        let mut bands_used = 0u32;
        if self.stage_bands > 0 {
            let ceiling = self
                .cache
                .segment_ceiling(f64::from(STAGE_W) * f64::from(STAGE_H));
            let t = Instant::now();
            let planned = plan_stage_bands(
                editor,
                &mut self.cache,
                self.area,
                1.0,
                ceiling,
                STAGE_W,
                STAGE_H,
                self.stage_bands,
            );
            encode_ms += ms(t);
            bands_used = planned.len() as u32;

            let mut largest_segments = 0u32;
            let mut largest_lines = 0u64;
            let t = Instant::now();
            for band in &planned {
                enc_bytes += scene_bytes(&band.scene);
                largest_segments = largest_segments.max(band.segments);
                largest_lines = largest_lines.max(band.lines);
                let (y0, y1) = (band.y0.round() as u32, band.y1.round() as u32);
                self.gpu.render_band(&band.scene, y0, y1);
            }
            render_call_ms += ms(t);
            let t = Instant::now();
            self.gpu.poll();
            gpu_wait_ms += ms(t);

            let comfortable = largest_segments < ceiling * 2 / 5
                && largest_lines < self.cache.line_budget() * 2 / 5;
            self.stage_bands = if comfortable { 0 } else { bands_used.min(16) };
        } else if let Some(scene) = whole_scene {
            let t = Instant::now();
            self.gpu.render_whole(&scene);
            render_call_ms += ms(t);
            let t = Instant::now();
            self.gpu.poll();
            gpu_wait_ms += ms(t);
        }
        let gpu_ms = render_call_ms + gpu_wait_ms;

        FrameRecord {
            frame: editor.current_frame,
            encode_ms,
            gpu_ms,
            render_call_ms,
            gpu_wait_ms,
            enc_bytes,
            bands: bands_used,
            scale: self.gpu.ctx.buffer_scale,
            lines: self.cache.last_lines(),
            grew: self.gpu.ctx.buffer_scale != scale_before,
        }
    }
}

struct ScenarioReport {
    name: String,
    records: Vec<FrameRecord>,
}

impl ScenarioReport {
    fn print(&self) {
        let mut totals: Vec<f64> = self
            .records
            .iter()
            .map(|r| r.encode_ms + r.gpu_ms)
            .collect();
        totals.sort_by(|a, b| a.total_cmp(b));
        let p50 = median(&totals);
        let p95 = percentile(&totals, 0.95);
        let max = totals.last().copied().unwrap_or(0.0);
        let band_frames = self.records.iter().filter(|r| r.bands > 0).count();
        let max_bands = self.records.iter().map(|r| r.bands).max().unwrap_or(0);
        let grows = self.records.iter().filter(|r| r.grew).count();

        println!(
            "scenario {}: {} frames | p50 {:.2} ms  p95 {:.2} ms  max {:.2} ms | band-mode {}/{} | \
             max bands {max_bands} | grows {grows}",
            self.name,
            self.records.len(),
            p50,
            p95,
            max,
            band_frames,
            self.records.len(),
        );

        if self.records.len() <= 24 {
            println!("  in visit order:");
            for r in &self.records {
                println!(
                    "    frame {:>5}: encode {:>8.2} ms  gpu {:>8.2} ms (render-call cpu {:>7.2}  gpu wait {:>7.2})  bands {:>2}  lines {}  enc_bytes {}",
                    r.frame, r.encode_ms, r.gpu_ms, r.render_call_ms, r.gpu_wait_ms, r.bands, r.lines, r.enc_bytes
                );
            }
        }
        let mut worst = self.records.clone();
        worst.sort_by(|a, b| (b.encode_ms + b.gpu_ms).total_cmp(&(a.encode_ms + a.gpu_ms)));
        worst.truncate(WORST_KEPT);
        println!("  worst frames:");
        for r in &worst {
            println!(
                "    frame {:>5}: encode {:>8.2} ms  gpu {:>8.2} ms (render-call cpu {:>7.2}  gpu wait {:>7.2})  bands {:>2}  scale {:>2}  lines {}  enc_bytes {}",
                r.frame, r.encode_ms, r.gpu_ms, r.render_call_ms, r.gpu_wait_ms, r.bands, r.scale, r.lines, r.enc_bytes
            );
        }
    }
}

fn scenario_a(scene: &Scene) -> anyhow::Result<ScenarioReport> {
    let mut editor = framed_editor(scene.clone());
    let total_frames = editor.scene().frame_count();
    let mut runner = Runner::new()?;
    let mut records = Vec::with_capacity(total_frames as usize);
    for f in 0..total_frames {
        editor.current_frame = f;
        records.push(runner.step(&editor));
    }
    Ok(ScenarioReport { name: "a".into(), records })
}

/// The scrub sequence S0 item 2 asks for: `0, 20, 715, 900, 4700, 20`.
const SCRUB: [u32; 6] = [0, 20, 715, 900, 4700, 20];

/// `FRAME_COST_SCRUB=0,20,20` overrides the scrub sequence (diagnosis only).
fn scrub() -> Vec<u32> {
    match std::env::var("FRAME_COST_SCRUB") {
        Ok(v) => v.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
        Err(_) => SCRUB.to_vec(),
    }
}

fn scenario_b(scene: &Scene) -> anyhow::Result<ScenarioReport> {
    let mut editor = framed_editor(scene.clone());
    let mut runner = Runner::new()?;
    let mut records = Vec::new();
    // S7 diagnosis: `FRAME_COST_B_SLEEP_MS=3000` idles that long before every frame
    // but the first, so the GPU's clocks can drop before the next one.
    let idle = std::env::var("FRAME_COST_B_SLEEP_MS").ok().and_then(|v| v.parse::<u64>().ok());
    // `FRAME_COST_WARM=1`: touch the warmer every 200 ms of the idle, as the
    // frames of a playback would.
    let warm = std::env::var_os("FRAME_COST_WARM").is_some();
    for (i, f) in scrub().into_iter().enumerate() {
        if let (Some(ms), true) = (idle, i > 0) {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if warm {
                    runner.gpu.ctx.keep_warm();
                }
                std::thread::sleep(Duration::from_millis(200).min(until - Instant::now()));
            }
        }
        editor.current_frame = f;
        records.push(runner.step(&editor));
    }
    print_retention("after scenario b", &runner.cache);
    Ok(ScenarioReport { name: "b".into(), records })
}

/// Scenario `b` again, zoomed `factor`×, centred on the stage — `fit_to_rect`
/// in `framed_editor` already put `camera.center` on the stage's own centre,
/// so scaling `zoom` in place keeps it there.
fn scenario_c(scene: &Scene, factor: f64) -> anyhow::Result<ScenarioReport> {
    let mut editor = framed_editor(scene.clone());
    editor.camera.zoom *= factor;
    let mut runner = Runner::new()?;
    let mut records = Vec::new();
    for f in scrub() {
        editor.current_frame = f;
        records.push(runner.step(&editor));
    }
    print_retention(&format!("after scenario c{}", factor as u32), &runner.cache);
    Ok(ScenarioReport {
        name: format!("c{}", factor as u32),
        records,
    })
}

/// Idle shrink (S0 item 2d, Diagnosis §5): play `0..=300`, fake a 25 s idle
/// wait on `DrawCache::note_shrink_headroom` (its own `now` parameter exists
/// so a test — or this probe — need not actually wait), follow
/// `pending_shrink` down until the cache is back at scale 1 or declines to
/// offer, then jump to frames 20 and 900 to see what a shrink right before a
/// heavy frame costs.
fn scenario_d(scene: &Scene) -> anyhow::Result<ScenarioReport> {
    let mut editor = framed_editor(scene.clone());
    let mut runner = Runner::new()?;
    let mut records = Vec::new();

    for f in 0..=300u32 {
        editor.current_frame = f;
        records.push(runner.step(&editor));
    }

    let pixels = f64::from(STAGE_W) * f64::from(STAGE_H);
    let mut shrinks = 0u32;
    loop {
        if runner.cache.scale() <= 1 {
            break;
        }
        // First call establishes `shrink_eligible_since` (if the cache is
        // comfortable at half its current scale); the second, 25 simulated
        // seconds later, clears `SHRINK_IDLE` (20 s) in one step.
        runner.cache.note_shrink_headroom(pixels, Instant::now());
        runner
            .cache
            .note_shrink_headroom(pixels, Instant::now() + Duration::from_secs(25));
        let Some(half) = runner.cache.pending_shrink() else {
            break;
        };
        runner.gpu.ctx.shrink_to(half)?;
        runner.cache.commit_shrink(half);
        shrinks += 1;
        println!("  scenario d: shrank to scale {half}");
    }
    if shrinks == 0 {
        println!(
            "  scenario d: no shrink offered (cache scale after frame 300: {})",
            runner.cache.scale()
        );
    }

    for f in [20u32, 900] {
        editor.current_frame = f;
        records.push(runner.step(&editor));
    }

    Ok(ScenarioReport { name: "d".into(), records })
}

/// Process working set in MiB — raw `K32GetProcessMemoryInfo` (kernel32), so
/// no extra crate. `None` off Windows or on failure.
#[cfg(windows)]
fn rss_mib() -> Option<f64> {
    rss_counters(false)
}

/// The process's peak working set so far, MiB (S7: the RSS peak reported next
/// to the sampled `rss_max`).
#[cfg(windows)]
fn peak_rss_mib() -> Option<f64> {
    rss_counters(true)
}

#[cfg(not(windows))]
fn peak_rss_mib() -> Option<f64> {
    None
}

#[cfg(windows)]
fn rss_counters(peak: bool) -> Option<f64> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c: Counters = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<Counters>() as u32;
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    let bytes = if peak { c.peak_working_set_size } else { c.working_set_size };
    (ok != 0).then(|| bytes as f64 / (1024.0 * 1024.0))
}



/// The process's private commit (bytes it has allocated, resident or not), MiB.
#[cfg(windows)]
fn commit_mib() -> Option<f64> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c: Counters = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<Counters>() as u32;
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then(|| c.pagefile_usage as f64 / (1024.0 * 1024.0))
}

#[cfg(not(windows))]
fn commit_mib() -> Option<f64> {
    None
}

#[cfg(not(windows))]
fn rss_mib() -> Option<f64> {
    None
}

/// `warm`: what a whole-document warm-up at open would cost. Pass 1 encodes
/// every frame in order (no GPU) with build-on-first-sighting and an
/// effectively unlimited stamp budget; the GPU is then grown once to the scale
/// that fits the heaviest frame; pass 2 replays scenarios a and b.
fn scenario_warm(scene: &Scene) -> anyhow::Result<()> {
    let mut editor = framed_editor(scene.clone());
    let total_frames = editor.scene().frame_count();
    let mut runner = Runner::new()?;
    runner.cache.set_build_on_first_sighting(true);
    runner.cache.symbol_scenes.budget_bytes = 64usize << 30;

    // Pass 1: encode only.
    let pixels = f64::from(STAGE_W) * f64::from(STAGE_H);
    let mut max_lines = (0u64, 0u32);
    let mut max_segments = (0u32, 0u32);
    let t = Instant::now();
    for f in 0..total_frames {
        editor.current_frame = f;
        let mut scene = vello::Scene::new();
        stage::build_scene(&mut scene, &editor, runner.area, 1.0, &mut runner.cache);
        let lines = runner.cache.last_lines();
        let segs = runner.cache.last_encode();
        if lines > max_lines.0 {
            max_lines = (lines, f);
        }
        if segs > max_segments.0 {
            max_segments = (segs, f);
        }
    }
    let wall_s = t.elapsed().as_secs_f64();
    let (entries, bytes, budget, evictions, first_sightings) =
        runner.cache.symbol_scenes.retention_stats();
    let rss = rss_mib();
    println!(
        "warm pass 1: {total_frames} frames in {wall_s:.2} s ({:.2} ms/frame)",
        wall_s * 1e3 / f64::from(total_frames.max(1))
    );
    println!(
        "  max lines {} at frame {} | max segments {} at frame {}",
        max_lines.0, max_lines.1, max_segments.0, max_segments.1
    );
    println!(
        "  stamp entries {entries} | {:.1} MiB (budget {:.0} MiB) | evictions {evictions} | \
         first sightings drawn live {first_sightings} | builds {} stamps {}",
        bytes as f64 / (1024.0 * 1024.0),
        budget as f64 / (1024.0 * 1024.0),
        runner.cache.symbol_scenes.builds,
        runner.cache.symbol_scenes.stamps,
    );
    match rss {
        Some(r) => println!("  process RSS (working set) {r:.0} MiB"),
        None => println!("  process RSS: unavailable"),
    }

    // Fit scale: smallest power of two whose per-scale budgets
    // (`line_budget_at`/`segment_ceiling_at`) hold the heaviest frame.
    let max_scale = runner.gpu.ctx.max_buffer_scale;
    let mut fit = 1u32;
    loop {
        if (max_lines.0 <= line_budget_at(u64::from(fit))
            && max_segments.0 <= segment_ceiling_at(pixels, u64::from(fit)))
            || fit >= max_scale
        {
            break;
        }
        fit = (fit * 2).min(max_scale);
    }
    println!(
        "  fit scale {fit} (max_buffer_scale {max_scale}; cache scale after pass 1 {}) | \
         budgets at fit: lines {} segments {}",
        runner.cache.scale(),
        line_budget_at(u64::from(fit)),
        segment_ceiling_at(pixels, u64::from(fit)),
    );
    let target = fit.max(runner.cache.scale());
    runner.cache.set_scale(target);
    runner.gpu.ctx.grow_to(target);
    println!("  gpu grown once to scale {target}");

    // Pass 2a.
    let builds_before = runner.cache.symbol_scenes.builds;
    let mut records = Vec::with_capacity(total_frames as usize);
    for f in 0..total_frames {
        editor.current_frame = f;
        records.push(runner.step(&editor));
    }
    let a = ScenarioReport { name: "warm-a".into(), records };
    a.print();
    print_warm_extra(&a, runner.cache.symbol_scenes.builds - builds_before);
    print_retention("after warm-a", &runner.cache);

    // Pass 2b.
    let builds_before = runner.cache.symbol_scenes.builds;
    let mut records = Vec::new();
    for f in scrub() {
        editor.current_frame = f;
        records.push(runner.step(&editor));
    }
    let b = ScenarioReport { name: "warm-b".into(), records };
    b.print();
    print_warm_extra(&b, runner.cache.symbol_scenes.builds - builds_before);
    print_retention("after warm-b", &runner.cache);
    Ok(())
}

/// **S9: the app's own warm-up at open**, on `runner`'s cache and GPU: the same
/// calls `App::tick_warm` makes (`warm_open::run_warm`, `warm_open::apply`, then
/// the GPU grown to the cache's scale). `budget` is `None` for the app's own
/// default (`BUZZ_STAMP_CACHE_MB` or a quarter of RAM); `workers` the pool jobs.
fn warm_open_into(
    runner: &mut Runner,
    editor: &Editor,
    jobs: &Arc<buzz_jobs::JobSystem>,
    budget: Option<usize>,
    workers: usize,
) -> buzz_app::warm_open::WarmReport {
    use buzz_app::warm_open as warm;
    let count = editor.playing_length();
    // `budget` is the cache's whole stamp limit; the warm-up fills 85% of it.
    let limit = budget.unwrap_or_else(warm::default_budget_bytes);
    println!(
        "warm-open: playing_length {count} (frame_count {}), stamp limit {} MiB (warm-up share {} MiB), workers {workers}, physical RAM {:.1} GiB",
        editor.scene().frame_count(),
        limit >> 20,
        warm::warm_share(limit) >> 20,
        warm::physical_ram_bytes() as f64 / (1u64 << 30) as f64,
    );
    // `FRAME_COST_FRAMES=a-b` (diagnosis) warms just the frames about to be played.
    let frames = if std::env::var_os("FRAME_COST_WARM_PLAYED").is_some() {
        pipelined_frames(count)
    } else {
        warm::frames_from(0, count)
    };
    let mut inputs = stage::StageInputs::from_editor(editor, runner.area, 1.0);
    inputs.frame = frames[0];
    inputs.buffer_scale = runner.cache.max_scale().max(runner.cache.scale());
    let job = warm::WarmJob {
        inputs,
        frames,
        budget_bytes: warm::warm_share(limit).saturating_sub(runner.cache.stamp_bytes()),
        limit_bytes: limit,
        workers,
        detail: runner.cache.detail(),
        symbols: runner.cache.symbols.clone(),
    };
    let t = Instant::now();
    let last_tenth = std::cell::Cell::new(0u64);
    let out = warm::run_warm(jobs, job, &|| false, &|done, total| {
        let tenth = done * 10 / total.max(1);
        if tenth > last_tenth.get() {
            last_tenth.set(tenth);
            println!("  warm-open: {done}/{total} frames ({:.1} s)", t.elapsed().as_secs_f64());
        }
    });
    let pixels = f64::from(STAGE_W) * f64::from(STAGE_H);
    let applied_at = Instant::now();
    let report = warm::apply(&mut runner.cache, out, pixels);
    runner.gpu.ctx.grow_to(runner.cache.scale());
    // The buffers are only really allocated by the first render at the new
    // scale: make that an empty one here, at open, not frame 0's.
    {
        let t = Instant::now();
        runner.gpu.render_whole(&vello::Scene::new());
        runner.gpu.poll();
        println!("  warm-open: first render at scale {} (empty scene, allocates the buffers): {:.1} ms", runner.gpu.ctx.buffer_scale, ms(t));
        let t = Instant::now();
        runner.gpu.render_whole(&vello::Scene::new());
        runner.gpu.poll();
        println!("  warm-open: second empty render: {:.1} ms", ms(t));
    }
    println!(
        "warm-open: {} frames covered of {} (encoded {}) in {:.2} s (+ install/size {:.0} ms) | stamps {} = {:.1} MiB (budget {} MiB, by budget {}, dropped {}) | peak lines {} (frame {}) segments {} (frame {}) | fit scale {} (gpu {})",
        report.frames_covered,
        report.frames_total,
        report.frames_encoded,
        report.wall_s,
        ms(applied_at),
        report.stamp_keys,
        report.stamp_bytes as f64 / (1024.0 * 1024.0),
        report.budget_bytes >> 20,
        report.stopped_by_budget,
        report.dropped_stamps,
        report.peak_lines,
        report.peak_lines_frame,
        report.peak_segments,
        report.peak_segments_frame,
        report.fit_scale,
        runner.gpu.ctx.buffer_scale,
    );
    println!(
        "  process RSS after warm-open {:?} MiB (private commit {:?} MiB), peak {:?} MiB | cache holds {} stamps, {:.1} MiB accounted",
        rss_mib().map(f64::round),
        commit_mib().map(f64::round),
        peak_rss_mib().map(f64::round),
        runner.cache.stamp_count(),
        runner.cache.stamp_bytes() as f64 / (1024.0 * 1024.0),
    );
    report
}

/// `warm-measure` (S9 step 1): how many distinct stamps and bytes the whole film
/// needs at the default fit view, and the bytes the first N frames need.
fn scenario_warm_measure(scene: &Scene) -> anyhow::Result<()> {
    let editor = framed_editor(scene.clone());
    let mut runner = Runner::new()?;
    let jobs = Arc::new(buzz_jobs::JobSystem::new());
    let workers: usize = std::env::var("FRAME_COST_WARM_WORKERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let t = Instant::now();
    let report = warm_open_into(&mut runner, &editor, &jobs, Some(64usize << 30), workers);
    println!("warm-measure: whole film {:.1} s", t.elapsed().as_secs_f64());
    println!(
        "warm-measure: distinct stamp keys {} | {:.1} MiB | frames covered {} of {}",
        report.stamp_keys,
        report.stamp_bytes as f64 / (1024.0 * 1024.0),
        report.frames_covered,
        report.frames_total
    );
    println!("warm-measure: bytes needed by the first N frames (workers {workers}; exact with 1):");
    let mut next = 100u32;
    for (frames, bytes) in &report.curve {
        if *frames >= next || *frames == report.frames_covered {
            println!("  first {frames:>5} frames: {:>9.1} MiB", *bytes as f64 / (1024.0 * 1024.0));
            while next <= *frames {
                next = if next < 1000 { next + 200 } else { next + 500 };
            }
        }
    }
    Ok(())
}

/// Frames over 25 ms and new stamp builds — the two numbers `warm` adds.
fn print_warm_extra(r: &ScenarioReport, new_builds: u64) {
    let over = r.records.iter().filter(|x| x.encode_ms + x.gpu_ms > 25.0).count();
    let grows = r.records.iter().filter(|x| x.grew).count();
    println!(
        "  {}: frames over 25 ms {over} | grows {grows} | new stamp builds {new_builds}",
        r.name
    );
}

/// `--layers <frame>`: the diagnosis's own per-layer probe
/// (`docs/probes/zz_perf_probe.rs`, `bimari5_playback_and_layers`) — a warm
/// full-frame encode, then one per visible layer with just that layer
/// hidden, reporting the delta. Pure CPU: no `Gpu` here at all.
fn run_layers(input: &str, frame: u32) -> anyhow::Result<()> {
    let imported = read_input(input)?;
    let base = imported.scene;

    let mut cache = DrawCache::default();
    // The probe's own ceiling: generous enough that the ladder need not
    // fight this measurement for scale while it is judging layers.
    cache.set_max_scale(8);
    // The same escape hatch the GPU scenarios honour (and `App::new`): a
    // live-draw baseline. It was missing here, which once made an "unlit,
    // stamps off" warm encode read 3 ms when the stamps were in fact on.
    if std::env::var("BUZZ_NO_SYMBOL_CACHE").is_ok() {
        cache.set_symbol_reuse(false);
        println!("BUZZ_NO_SYMBOL_CACHE: symbol stamps off");
    }
    // `FRAME_COST_DETAIL=full|nomodelling|flat|unsealed` pins the lighting
    // detail, so the lit ladder can be measured one rung at a time.
    if let Ok(level) = std::env::var("FRAME_COST_DETAIL") {
        use buzz_render::document::LightDetail;
        let pinned = match level.as_str() {
            "nomodelling" => LightDetail::NoModelling,
            "flat" => LightDetail::Flat,
            "unsealed" => LightDetail::Unsealed,
            _ => LightDetail::Full,
        };
        cache.pin_detail(Some(pinned));
        println!("FRAME_COST_DETAIL: {pinned:?}");
    }

    let mut e = framed_editor(base.clone());
    e.current_frame = frame;
    encode_ms(&e, &mut cache);
    encode_ms(&e, &mut cache);
    let refusals_before_full = cache.symbol_scenes.refusal_counts();
    let full = (0..2)
        .map(|_| encode_ms(&e, &mut cache))
        .fold(f64::MAX, f64::min);
    let refusals_after_full = cache.symbol_scenes.refusal_counts();

    let ids: Vec<(buzz_scene::LayerId, String, bool)> = base
        .layers()
        .iter()
        .map(|l| (l.id, l.name.clone(), l.visible))
        .collect();

    let mut rows = Vec::new();
    for (id, name, visible) in &ids {
        if !visible {
            continue;
        }
        let mut hidden = base.clone();
        hidden.update_layer(*id, |l| l.visible = false);
        let mut eh = framed_editor(hidden);
        eh.current_frame = frame;
        encode_ms(&eh, &mut cache);
        let t = (0..2)
            .map(|_| encode_ms(&eh, &mut cache))
            .fold(f64::MAX, f64::min);
        rows.push((full - t, name.clone()));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));

    println!("frame {frame}: full warm encode {full:.2} ms; top layers by hide-one delta:");
    for (delta, name) in rows.iter().take(LAYERS_KEPT) {
        println!("  {delta:>8.2} ms  {name}");
    }
    print_refusal_delta(
        &format!("frame {frame}, full warm encode"),
        &refusals_before_full,
        &refusals_after_full,
    );
    print_retention(&format!("frame {frame}, after the hide-one probes"), &cache);

    // S2's own diagnosis: isolate the single heaviest layer (the diagnosis's
    // `Layer_29` / `charriotRunning` / `well`) and warm-encode it alone, so
    // its refusals are not mixed in with the other 37 layers'.
    if let Some((_, top_name)) = rows.first() {
        let mut isolated = base.clone();
        for (id, name, visible) in &ids {
            if *visible && name != top_name {
                isolated.update_layer(*id, |l| l.visible = false);
            }
        }
        let mut ei = framed_editor(isolated);
        ei.current_frame = frame;
        encode_ms(&ei, &mut cache);
        encode_ms(&ei, &mut cache);
        let before = cache.symbol_scenes.refusal_counts();
        let t = (0..2)
            .map(|_| encode_ms(&ei, &mut cache))
            .fold(f64::MAX, f64::min);
        let after = cache.symbol_scenes.refusal_counts();
        println!("  {top_name} alone: {t:.2} ms warm");
        print_refusal_delta(&format!("frame {frame}, {top_name} alone"), &before, &after);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `--picture-probe`: S10 step P0 in `docs/BIMARI5_LAG_PLAN.md`. Measurement
// only — nothing here changes production behaviour.
//
// **What the per-layer fingerprint covers.** Each *unit* (a layer, or a mask
// plus the layers it clips, which are atomic) is encoded ALONE — every other
// non-folder layer hidden, same stage / camera / fit view / `DrawCache` — and
// the fingerprint is a 64-bit hash of the resulting Vello `Encoding`: path
// tags, path data, draw tags, draw data, transforms, styles, gradient colour
// stops, the late-bound patches (ramp extend, image blob id + size, glyph-run
// index), and glyph positions. That is the exact input the GPU renders from,
// so equal fingerprint => the layer's Vello input is byte-identical. It folds
// in every input at once: resolved objects, tweens, modifiers, symbol inner
// frames (recursively), layer flags (alpha, blend, outline, tint, filters),
// layer-parenting transform, depth/camera projection, lights, stamps. It is
// FINER than nothing and can be COARSER than P2's structural
// `LayerFrameFp` (two different structural states that draw identical bytes
// share this fingerprint), so hit rates here are an optimistic bound. Glyph
// runs are hashed by index only (their per-run data has no accessor), so text
// layers are the one thing not fully covered. Lines per unit come from
// `DrawCache::last_lines()` of that unit's own encode, so no sampling of line
// counts is needed.
// ---------------------------------------------------------------------------

/// Bytes of a slice of plain-old-data (all the encoding streams are `Pod`).
fn pod_bytes<T>(v: &[T]) -> &[u8] {
    // SAFETY: only ever called on vello_encoding's `Pod` streams (no padding,
    // no interior pointers), read-only, lifetime tied to `v`.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

struct Fp(u64);

impl Fp {
    fn new() -> Self {
        Fp(0xcbf2_9ce4_8422_2325)
    }
    #[inline]
    fn word(&mut self, w: u64) {
        self.0 = (self.0 ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 ^= self.0 >> 29;
    }
    fn bytes(&mut self, b: &[u8]) {
        self.word(b.len() as u64);
        let mut chunks = b.chunks_exact(8);
        for c in &mut chunks {
            self.word(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let mut tail = [0u8; 8];
        let rem = chunks.remainder();
        tail[..rem.len()].copy_from_slice(rem);
        self.word(u64::from_le_bytes(tail));
    }
    fn text(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
}

/// The fingerprint of one encoded Vello scene — see the section comment.
fn fp_of_scene(scene: &vello::Scene) -> u64 {
    use buzz_render::vello_encoding::Patch;
    let e = scene.encoding();
    let mut h = Fp::new();
    h.bytes(pod_bytes(&e.path_tags));
    h.bytes(pod_bytes(&e.path_data));
    h.bytes(pod_bytes(&e.draw_tags));
    h.bytes(pod_bytes(&e.draw_data));
    h.bytes(pod_bytes(&e.transforms));
    h.bytes(pod_bytes(&e.styles));
    h.word(u64::from(e.n_clips));
    h.word(u64::from(e.n_open_clips));
    h.word(e.resources.color_stops.len() as u64);
    for s in &e.resources.color_stops {
        h.text(&format!("{s:?}"));
    }
    for p in &e.resources.patches {
        match p {
            Patch::Ramp {
                draw_data_offset,
                stops,
                extend,
            } => {
                h.word(1);
                h.word(*draw_data_offset as u64);
                h.word(stops.start as u64);
                h.word(stops.end as u64);
                h.text(&format!("{extend:?}"));
            }
            Patch::GlyphRun { index } => {
                h.word(2);
                h.word(*index as u64);
            }
            Patch::Image {
                draw_data_offset,
                image,
            } => {
                h.word(3);
                h.word(*draw_data_offset as u64);
                h.word(image.data.id());
                h.word(u64::from(image.width));
                h.word(u64::from(image.height));
                h.text(&format!("{:?}{:?}", image.format, image.alpha_type));
            }
        }
    }
    h.word(e.resources.glyphs.len() as u64);
    for g in &e.resources.glyphs {
        h.word(u64::from(g.id));
        h.word(u64::from(g.x.to_bits()));
        h.word(u64::from(g.y.to_bits()));
    }
    h.word(e.resources.glyph_runs.len() as u64);
    h.word(e.resources.normalized_coords.len() as u64);
    h.0
}

/// Ordinary least squares `y = a + b x`; returns `(a, b, r2)`.
fn ols(points: &[(f64, f64)]) -> (f64, f64, f64) {
    let n = points.len() as f64;
    if points.len() < 2 {
        return (points.first().map_or(0.0, |p| p.1), 0.0, 0.0);
    }
    let sx: f64 = points.iter().map(|p| p.0).sum();
    let sy: f64 = points.iter().map(|p| p.1).sum();
    let sxx: f64 = points.iter().map(|p| p.0 * p.0).sum();
    let sxy: f64 = points.iter().map(|p| p.0 * p.1).sum();
    let denom = n * sxx - sx * sx;
    let b = if denom.abs() < 1e-12 { 0.0 } else { (n * sxy - sx * sy) / denom };
    let a = (sy - b * sx) / n;
    let my = sy / n;
    let ss_tot: f64 = points.iter().map(|p| (p.1 - my).powi(2)).sum();
    let ss_res: f64 = points.iter().map(|p| (p.1 - (a + b * p.0)).powi(2)).sum();
    let r2 = if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 0.0 };
    (a, b, r2)
}

/// One cacheable-or-live thing in the stack: a layer, or a mask and the
/// layers it clips (atomic — S10 "mask runs are atomic").
struct Unit {
    name: String,
    ids: Vec<buzz_scene::LayerId>,
    /// Any member has `blend != Normal` — may only sit in the bottom run.
    blend_non_normal: bool,
}

/// Every non-folder layer not in `keep` hidden; folders stay so a layer's own
/// visibility is what decides (a hidden folder would hide its children).
fn only_scene(base: &Scene, keep: &[buzz_scene::LayerId]) -> Scene {
    let mut s = base.clone();
    for l in base.layers().iter() {
        if l.kind != buzz_scene::LayerKind::Folder && !keep.contains(&l.id) {
            s.update_layer(l.id, |x| x.visible = false);
        }
    }
    s
}

/// Units in paint order, bottom first.
fn build_units(base: &Scene) -> (Vec<Unit>, Vec<Editor>) {
    let groups = base.layers().mask_groups();
    let mut done: Vec<buzz_scene::LayerId> = Vec::new();
    let mut units = Vec::new();
    let mut editors = Vec::new();
    for l in base.layers().paint_order() {
        if l.kind == buzz_scene::LayerKind::Folder || done.contains(&l.id) {
            continue;
        }
        let group = groups
            .iter()
            .find(|g| g.mask == l.id || g.masked.contains(&l.id));
        let ids: Vec<buzz_scene::LayerId> = match group {
            Some(g) => {
                let mut v = g.masked.clone();
                v.push(g.mask);
                v
            }
            None => {
                if !l.visible {
                    continue;
                }
                vec![l.id]
            }
        };
        done.extend(ids.iter().copied());
        let members: Vec<_> = ids.iter().filter_map(|id| base.layers().get(*id)).collect();
        let name = members
            .iter()
            .map(|m| {
                if m.kind.is_mask() {
                    format!("{}[mask]", m.name)
                } else {
                    m.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("+");
        let blend_non_normal = members.iter().any(|m| format!("{:?}", m.blend) != "Normal");
        editors.push(framed_editor(only_scene(base, &ids)));
        units.push(Unit {
            name,
            ids,
            blend_non_normal,
        });
    }
    (units, editors)
}

/// One frame's plan under the S10 rules; `cacheable(u)` says whether unit `u`
/// may be served from a picture this frame.
struct Plan {
    lines_left: u64,
    /// Units drawn live (or in an upper run beyond the two allowed).
    live: Vec<usize>,
    bottom_key: Option<u64>,
    bottom_units: usize,
    upper_pictures: usize,
}

fn plan_frame(
    order: &[usize],
    units: &[Unit],
    active: &[bool],
    lines: &[u64],
    fp: &[u64],
    cacheable: &dyn Fn(usize) -> bool,
) -> Plan {
    let seq: Vec<usize> = order.iter().copied().filter(|u| active[*u]).collect();
    let mut i = 0;
    let mut key = Fp::new();
    while i < seq.len() && cacheable(seq[i]) {
        key.word(seq[i] as u64);
        key.word(fp[seq[i]]);
        i += 1;
    }
    let bottom_units = i;
    let mut live = Vec::new();
    let mut runs: Vec<Vec<usize>> = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for &u in &seq[i..] {
        if cacheable(u) && !units[u].blend_non_normal {
            run.push(u);
        } else {
            if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
            live.push(u);
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    let run_lines = |r: &Vec<usize>| r.iter().map(|u| lines[*u]).sum::<u64>();
    runs.sort_by_key(|r| std::cmp::Reverse(run_lines(r)));
    let mut upper_pictures = 0;
    for (k, r) in runs.iter().enumerate() {
        if k < 2 && run_lines(r) > 0 {
            upper_pictures += 1;
        } else {
            live.extend(r.iter().copied());
        }
    }
    let lines_left = live.iter().map(|u| lines[*u]).sum();
    Plan {
        lines_left,
        live,
        bottom_key: (bottom_units > 0).then_some(key.0),
        bottom_units,
        upper_pictures,
    }
}

fn read_target(gpu: &Gpu) -> Vec<u8> {
    let size = u64::from(STAGE_W) * u64::from(STAGE_H) * 4;
    let buf = gpu.ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("picture-probe-readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &gpu.target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
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
    gpu.ctx.queue.submit([encoder.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    gpu.poll();
    let out = slice.get_mapped_range().to_vec();
    buf.unmap();
    out
}

fn hist5(values: &[f64]) -> serde_json::Value {
    let mut h = [0u64; 5];
    for &v in values {
        let k = if v <= 10.0 {
            0
        } else if v <= 15.0 {
            1
        } else if v <= 22.0 {
            2
        } else if v <= 25.0 {
            3
        } else {
            4
        };
        h[k] += 1;
    }
    serde_json::json!({"le10": h[0], "10_15": h[1], "15_22": h[2], "22_25": h[3], "gt25": h[4]})
}

fn run_picture_probe(input: &str) -> anyhow::Result<()> {
    use std::collections::HashMap;
    let t_all = Instant::now();
    let imported = read_input(input)?;
    let base = imported.scene;
    let main_editor = framed_editor(base.clone());
    let n = main_editor.scene().frame_count() as usize;
    println!("picture-probe: {n} frames, import summary: {}", imported.summary);

    let mut runner = Runner::new()?;
    runner.cache.set_build_on_first_sighting(true);
    runner.cache.symbol_scenes.budget_bytes = 64usize << 30;

    // ---- pass 0: warm every stamp (same as `warm`), grow GPU once to fit.
    let pixels = f64::from(STAGE_W) * f64::from(STAGE_H);
    let mut editor = framed_editor(base.clone());
    let (mut max_lines, mut max_segments) = (0u64, 0u32);
    let t = Instant::now();
    for f in 0..n as u32 {
        editor.current_frame = f;
        let mut s = vello::Scene::new();
        stage::build_scene(&mut s, &editor, runner.area, 1.0, &mut runner.cache);
        max_lines = max_lines.max(runner.cache.last_lines());
        max_segments = max_segments.max(runner.cache.last_encode());
    }
    println!("pass 0 (warm encode all frames): {:.1} s; max lines {max_lines}", t.elapsed().as_secs_f64());
    let max_scale = runner.gpu.ctx.max_buffer_scale;
    let mut fit = 1u32;
    loop {
        if (max_lines <= line_budget_at(u64::from(fit))
            && max_segments <= segment_ceiling_at(pixels, u64::from(fit)))
            || fit >= max_scale
        {
            break;
        }
        fit = (fit * 2).min(max_scale);
    }
    let target = fit.max(runner.cache.scale());
    runner.cache.set_scale(target);
    runner.gpu.ctx.grow_to(target);
    println!("gpu grown once to scale {target}");

    // ---- calibration: real full-frame renders on ~40 sampled frames.
    let mut sample_frames: Vec<u32> = (0..40).map(|i| (i * n / 40) as u32).collect();
    for extra in [715u32, 900, 4700] {
        if (extra as usize) < n {
            sample_frames.push(extra);
        }
    }
    sample_frames.sort_unstable();
    sample_frames.dedup();
    let mut samples: Vec<FrameRecord> = Vec::new();
    for &f in &sample_frames {
        // Playback-like: the 8 frames before `f` in order (stamps for the
        // neighbourhood get built/touched as in sequential playback), then `f`.
        let mut rec = None;
        for g in f.saturating_sub(8)..=f {
            editor.current_frame = g;
            rec = Some(runner.step(&editor));
        }
        editor.current_frame = f;
        samples.push(rec.unwrap());
    }
    let single: Vec<(f64, f64)> = samples
        .iter()
        .filter(|r| r.bands == 0)
        .map(|r| (r.lines as f64 / 1e6, r.gpu_ms))
        .collect();
    let all_pts: Vec<(f64, f64)> = samples
        .iter()
        .map(|r| (r.lines as f64 / 1e6, r.gpu_ms))
        .collect();
    let (fit_pts, fit_label) = if single.len() >= 10 {
        (single.clone(), "single-pass samples only")
    } else {
        (all_pts.clone(), "all samples (bands included)")
    };
    let (icpt, slope, r2) = ols(&fit_pts);
    println!(
        "calibration ({fit_label}, {} pts, {} banded): gpu_ms = {icpt:.2} + {slope:.3} * Mlines (r2 {r2:.3})",
        fit_pts.len(),
        samples.iter().filter(|r| r.bands > 0).count()
    );
    let gpu_of = |lines: u64| (icpt + slope * lines as f64 / 1e6).max(0.0);

    // ---- units.
    let (units, mut units_ed) = build_units(&base);
    let nu = units.len();
    let mut order: Vec<usize> = (0..nu).collect(); // already paint order
    order.shrink_to_fit();
    println!("{nu} units (layers / mask groups) in paint order");
    if base.stage().sort_by_depth {
        println!("NOTE: stage.sort_by_depth is ON; unit order here is timeline order");
    }
    let mut blend_kinds: Vec<String> = Vec::new();
    for l in base.layers().iter() {
        let b = format!("{:?}", l.blend);
        if b != "Normal" && !blend_kinds.contains(&b) {
            blend_kinds.push(b);
        }
    }

    let mut depths: Vec<f64> = base.layers().iter().map(|l| l.depth).collect();
    depths.sort_by(|a, b| a.total_cmp(b));
    depths.dedup();
    let tilt = base.camera_has_tilt();
    let camera_key = |f: u32| -> u64 {
        let mut h = Fp::new();
        for c in base.camera_transform(f).as_coeffs() {
            h.word(c.to_bits());
        }
        h.word(u64::from(tilt));
        for &d in &depths {
            match base.camera_transform_at_depth(f, d) {
                Some(a) => {
                    for c in a.as_coeffs() {
                        h.word(c.to_bits());
                    }
                }
                None => h.word(0xDEAD),
            }
            if tilt {
                h.text(&format!("{:?}", base.camera_projection_at_depth(f, d)));
            }
        }
        h.0
    };

    // ---- pass 1: per-unit alone-encodes for every frame.
    let mut fps = vec![vec![0u64; nu]; n];
    let mut lines = vec![vec![0u64; nu]; n];
    let mut active = vec![vec![false; nu]; n];
    let mut cam_key = vec![0u64; n];
    let mut lights_active = vec![false; n];
    let mut lights_enabled_anywhere = base.lights().is_active();
    let rate = base.stage().frame_rate.max(1.0);
    let area = runner.area;
    // The fingerprint cache: symbol reuse (stamps) OFF, so the encoding is a
    // pure function of (layer, frame) — with stamps on, the stamp cache
    // (4096-entry LRU, thrashing on this document) decides live-vs-stamp per
    // visit and the same layer/frame encodes differently on repeat.
    let mut fp_cache = DrawCache::default();
    fp_cache.set_max_scale(runner.gpu.ctx.max_buffer_scale);
    fp_cache.set_symbol_reuse(false);
    fp_cache.set_scale(target);
    let mut live_lines = vec![vec![0u64; nu]; n];
    let t = Instant::now();
    for f in 0..n {
        let fu = f as u32;
        cam_key[f] = camera_key(fu);
        let rig = base.lights().resolved_at_rate(fu, rate).into_owned();
        lights_active[f] = rig.is_active();
        lights_enabled_anywhere |= lights_active[f];
        let drawable: Vec<buzz_scene::LayerId> =
            base.layers().drawable_at(fu).map(|l| l.id).collect();
        for (u, unit) in units.iter().enumerate() {
            if !unit.ids.iter().any(|id| drawable.contains(id)) {
                continue;
            }
            active[f][u] = true;
            units_ed[u].current_frame = fu;
            let mut s = vello::Scene::new();
            stage::build_scene(&mut s, &units_ed[u], area, 1.0, &mut runner.cache);
            lines[f][u] = runner.cache.last_lines();
            let mut sf = vello::Scene::new();
            stage::build_scene(&mut sf, &units_ed[u], area, 1.0, &mut fp_cache);
            live_lines[f][u] = fp_cache.last_lines();
            fps[f][u] = fp_of_scene(&sf);
        }
        if f % 500 == 0 {
            eprintln!("  probe frame {f}/{n} ({:.0} s)", t.elapsed().as_secs_f64());
        }
    }
    println!("pass 1 (per-unit encode + fingerprint, all frames): {:.1} s", t.elapsed().as_secs_f64());

    // ---- validation: sum of per-unit lines vs the real full-frame lines.
    let mut ratios: Vec<f64> = Vec::new();
    for r in &samples {
        let sum: u64 = lines[r.frame as usize].iter().sum();
        if r.lines > 0 {
            ratios.push(sum as f64 / r.lines as f64);
        }
    }
    if std::env::var("PICTURE_PROBE_DEBUG").is_ok() {
        for r in &samples {
            let sum: u64 = lines[r.frame as usize].iter().sum();
            let mut top: Vec<(u64, &str)> = (0..nu)
                .map(|u| (lines[r.frame as usize][u], units[u].name.as_str()))
                .collect();
            top.sort_by(|a, b| b.0.cmp(&a.0));
            println!(
                "  dbg frame {} full {} sum {} gpu {:.2} top {:?}",
                r.frame,
                r.lines,
                sum,
                r.gpu_ms,
                &top[..top.len().min(4)]
            );
        }
    }
    if let Ok(list) = std::env::var("PICTURE_PROBE_HIDE_ONE") {
        // Debug: which unit's removal drops the full frame's lines, vs its own alone-lines.
        for fr in list.split(',').filter_map(|x| x.trim().parse::<u32>().ok()) {
            let mut full_ed = framed_editor(base.clone());
            full_ed.current_frame = fr;
            let mut s = vello::Scene::new();
            stage::build_scene(&mut s, &full_ed, runner.area, 1.0, &mut runner.cache);
            let full = runner.cache.last_lines();
            let mut sum = 0u64;
            println!("  hide-one frame {fr}: full lines {full}");
            for u in 0..nu {
                let mut sc = base.clone();
                for id in &units[u].ids {
                    sc.update_layer(*id, |x| x.visible = false);
                }
                let mut ed = framed_editor(sc);
                ed.current_frame = fr;
                let mut s2 = vello::Scene::new();
                stage::build_scene(&mut s2, &ed, runner.area, 1.0, &mut runner.cache);
                let without = runner.cache.last_lines();
                let delta = full.saturating_sub(without);
                sum += delta;
                if delta > 20000 || lines[fr as usize][u] > 20000 {
                    println!(
                        "    {:<28} alone {:>9}  hide-one delta {:>9}",
                        units[u].name, lines[fr as usize][u], delta
                    );
                }
            }
            println!("    sum of hide-one deltas {sum}");
        }
    }
    ratios.sort_by(|a, b| a.total_cmp(b));
    let ratio_note = format!(
        "sum(unit lines)/full-frame lines on {} sampled frames: min {:.3} median {:.3} max {:.3}",
        ratios.len(),
        ratios.first().copied().unwrap_or(0.0),
        median(&ratios),
        ratios.last().copied().unwrap_or(0.0)
    );
    println!("{ratio_note}");

    // ---- per-unit runs of equal fingerprint.
    let mut runlen = vec![vec![0u32; nu]; n];
    for u in 0..nu {
        let mut start = 0;
        for f in 1..=n {
            let same = f < n && fps[f][u] == fps[start][u] && active[f][u] == active[start][u];
            if !same {
                let len = (f - start) as u32;
                for g in start..f {
                    runlen[g][u] = len;
                }
                start = f;
            }
        }
    }

    // ---- reasons a unit's fingerprint changed at f vs f-1.
    let mut changed_count = vec![0u64; nu];
    let mut active_count = vec![0u64; nu];
    let mut static12_frames = vec![0u64; nu];
    let mut reasons: Vec<HashMap<&'static str, u64>> = vec![HashMap::new(); nu];
    for f in 0..n {
        for u in 0..nu {
            if active[f][u] {
                active_count[u] += 1;
                if runlen[f][u] >= 12 {
                    static12_frames[u] += 1;
                }
            }
            if !active[f][u] {
                continue;
            }
            let ch = f == 0 || fps[f][u] != fps[f - 1][u];
            if !ch {
                continue;
            }
            changed_count[u] += 1;
            let reason = if f == 0 {
                "first frame"
            } else if !active[f - 1][u] {
                "appears"
            } else {
                let (a, b) = (f as u32, f as u32 - 1);
                let members: Vec<_> = units[u]
                    .ids
                    .iter()
                    .filter_map(|id| base.layers().get(*id))
                    .collect();
                if members
                    .iter()
                    .any(|l| l.frames.keyframe_start(a) != l.frames.keyframe_start(b))
                {
                    "keyframe change"
                } else if members
                    .iter()
                    .any(|l| l.frames.tween_span_at(a).is_some())
                {
                    "tween"
                } else if units[u].ids.iter().any(|id| {
                    base.layers().inherited_transform(*id, a)
                        != base.layers().inherited_transform(*id, b)
                }) {
                    "follows-parent motion"
                } else {
                    "inner animation (symbol timeline/loop/modifier/rig)"
                }
            };
            *reasons[u].entry(reason).or_default() += 1;
        }
    }

    // ---- planning.
    struct FrameOut {
        moved: bool,
        total_lines: u64,
        lines_left: u64,
        gpu_left: f64,
        live: Vec<usize>,
        bottom_key: Option<u64>,
        bottom_units: usize,
        uppers: usize,
    }
    let plan_all = |variant12: bool| -> Vec<FrameOut> {
        (0..n)
            .map(|f| {
                let moved = f > 0 && cam_key[f] != cam_key[f - 1];
                let total: u64 = lines[f].iter().sum();
                let cold = moved || lights_active[f];
                let cacheable = |u: usize| -> bool {
                    if cold || f == 0 {
                        return false;
                    }
                    if variant12 {
                        runlen[f][u] >= 12
                    } else {
                        fps[f][u] == fps[f - 1][u]
                    }
                };
                let p = plan_frame(&order, &units, &active[f], &lines[f], &fps[f], &cacheable);
                let left = if cold { total } else { p.lines_left };
                FrameOut {
                    moved,
                    total_lines: total,
                    lines_left: left,
                    gpu_left: gpu_of(left),
                    live: p.live,
                    bottom_key: if cold { None } else { p.bottom_key },
                    bottom_units: if cold { 0 } else { p.bottom_units },
                    uppers: if cold { 0 } else { p.upper_pictures },
                }
            })
            .collect()
    };
    let out1 = plan_all(false);
    let out12 = plan_all(true);

    let summarize = |out: &[FrameOut]| -> serde_json::Value {
        let moving: Vec<&FrameOut> = out.iter().filter(|o| o.moved).collect();
        let stat: Vec<(usize, &FrameOut)> = out.iter().enumerate().filter(|(_, o)| !o.moved).collect();
        let gpu_vals: Vec<f64> = stat.iter().map(|(_, o)| o.gpu_left).collect();
        let mut worst: Vec<(usize, &FrameOut)> = stat
            .iter()
            .copied()
            .filter(|(_, o)| o.lines_left > 5_000_000 && o.gpu_left >= 22.0)
            .collect();
        let worst_total = worst.len();
        worst.sort_by(|a, b| b.1.gpu_left.total_cmp(&a.1.gpu_left));
        let worst_json: Vec<serde_json::Value> = worst
            .iter()
            .take(20)
            .map(|(f, o)| {
                serde_json::json!([
                    f,
                    o.lines_left,
                    (o.gpu_left * 100.0).round() / 100.0,
                    o.live.iter().map(|u| units[*u].name.clone()).collect::<Vec<_>>()
                ])
            })
            .collect();
        // Bottom-picture keys over static-camera frames with a non-empty run.
        let mut keys: HashMap<u64, u64> = HashMap::new();
        for (_, o) in &stat {
            if let Some(k) = o.bottom_key {
                *keys.entry(k).or_default() += 1;
            }
        }
        // Run lengths of a constant bottom key (consecutive static-camera frames).
        let mut key_runs: Vec<u64> = Vec::new();
        let mut cur: Option<(u64, u64)> = None;
        for o in out {
            match (o.bottom_key, cur) {
                (Some(k), Some((ck, len))) if k == ck && !o.moved => cur = Some((ck, len + 1)),
                (Some(k), c) => {
                    if let Some((_, len)) = c {
                        key_runs.push(len);
                    }
                    cur = Some((k, 1));
                }
                (None, c) => {
                    if let Some((_, len)) = c {
                        key_runs.push(len);
                    }
                    cur = None;
                }
            }
        }
        if let Some((_, len)) = cur {
            key_runs.push(len);
        }
        key_runs.sort_unstable();
        let kr: Vec<f64> = key_runs.iter().map(|x| *x as f64).collect();
        let baseline_over22 = stat
            .iter()
            .filter(|(_, o)| gpu_of(o.total_lines) >= 22.0)
            .count();
        serde_json::json!({
            "camera_moving": {
                "frames": moving.len(),
                "over_5M_lines": moving.iter().filter(|o| o.total_lines > 5_000_000).count(),
                "max_lines": moving.iter().map(|o| o.total_lines).max().unwrap_or(0),
            },
            "static_camera": {
                "frames": stat.len(),
                "gpu_left_hist": hist5(&gpu_vals),
                "frames_lines_left_gt5M_and_gpu_ge22": worst_total,
                "worst": worst_json,
                "uncached_baseline_frames_with_est_gpu_ge22": baseline_over22,
                "frames_with_any_picture": stat.iter().filter(|(_, o)| o.bottom_units > 0 || o.uppers > 0).count(),
                "frames_with_upper_picture": stat.iter().filter(|(_, o)| o.uppers > 0).count(),
                "max_lines_left": stat.iter().map(|(_, o)| o.lines_left).max().unwrap_or(0),
            },
            "distinct_bottom_keys": keys.len(),
            "bottom_key_run_lengths": {
                "runs": key_runs.len(),
                "min": kr.first().copied().unwrap_or(0.0),
                "median": median(&kr),
                "p90": percentile(&kr, 0.9),
                "max": kr.last().copied().unwrap_or(0.0),
                "runs_ge_12": key_runs.iter().filter(|x| **x >= 12).count(),
            },
        })
    };
    let sum1 = summarize(&out1);
    let sum12 = summarize(&out12);

    // ---- per-unit table.
    let mut rows: Vec<(String, u64, u64, u64, String, u64)> = (0..nu)
        .map(|u| {
            let mut rs: Vec<(&&str, &u64)> = reasons[u].iter().collect();
            rs.sort_by(|a, b| b.1.cmp(a.1));
            let dom = rs
                .first()
                .map(|(r, c)| format!("{r} ({c})"))
                .unwrap_or_else(|| "never changes".into());
            let max_lines_u = (0..n).map(|f| lines[f][u]).max().unwrap_or(0);
            (
                units[u].name.clone(),
                active_count[u],
                changed_count[u],
                static12_frames[u],
                dom,
                max_lines_u,
            )
        })
        .collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2));
    println!("\nunits by frames whose fingerprint changed vs previous frame:");
    println!("  {:<38} {:>7} {:>8} {:>9} {:>10}  dominant reason", "unit", "active", "changed", "static>=12", "max lines");
    for r in &rows {
        println!("  {:<38} {:>7} {:>8} {:>9} {:>10}  {}", r.0, r.1, r.2, r.3, r.5, r.4);
    }

    // ---- static-run length stats (all units, runs of equal fp while active).
    let mut all_runs: Vec<f64> = Vec::new();
    let mut frames_in_runs = [0u64; 4]; // 1, 2-11, 12-99, 100+
    for u in 0..nu {
        let mut f = 0;
        while f < n {
            if !active[f][u] {
                f += 1;
                continue;
            }
            let len = runlen[f][u] as usize;
            all_runs.push(len as f64);
            let b = match len {
                1 => 0,
                2..=11 => 1,
                12..=99 => 2,
                _ => 3,
            };
            frames_in_runs[b] += len as u64;
            f += len.max(1);
        }
    }
    all_runs.sort_by(|a, b| a.total_cmp(b));

    // ---- soundness: 300 random equal-fp pairs rendered alone at 1x.
    let mut rng: u64 = 0x1234_5678_9ABC_DEF1;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut groups: Vec<HashMap<u64, Vec<u32>>> = vec![HashMap::new(); nu];
    for f in 0..n {
        for u in 0..nu {
            // Only layers the GPU can draw alone in one pass (live lines within
            // the line budget at the grown scale) — beyond it the render is
            // not a valid reference.
            if active[f][u] && live_lines[f][u] > 0 && live_lines[f][u] <= line_budget_at(u64::from(target)) {
                groups[u].entry(fps[f][u]).or_default().push(f as u32);
            }
        }
    }
    let mut candidates: Vec<(usize, u32)> = Vec::new();
    for u in 0..nu {
        for v in groups[u].values() {
            if v.len() >= 2 {
                for &f in v {
                    candidates.push((u, f));
                }
            }
        }
    }
    let mut mismatches: Vec<serde_json::Value> = Vec::new();
    let mut pairs_done = 0usize;
    let mut fp_differs_at_render_time = 0usize;
    let mut encode_not_repeatable = 0usize;
    let mut gpu_not_repeatable = 0usize;
    let mut mismatches_equal_at_render = 0usize;
    let t = Instant::now();
    if candidates.is_empty() {
        println!("soundness: no equal-fp candidates");
    } else {
        for _ in 0..300 {
            let (u, f1) = candidates[(next() % candidates.len() as u64) as usize];
            let group = &groups[u][&fps[f1 as usize][u]];
            let far: Vec<u32> = group.iter().copied().filter(|g| g.abs_diff(f1) > 1).collect();
            let pool = if far.is_empty() {
                group.iter().copied().filter(|g| *g != f1).collect::<Vec<_>>()
            } else {
                far
            };
            let f2 = pool[(next() % pool.len() as u64) as usize];
            // Re-fingerprint the scene actually rendered: the pass-1 fingerprint
            // was taken earlier, and the cache moves on in between.
            let mut render = |f: u32| -> (Vec<u8>, u64, u64) {
                units_ed[u].current_frame = f;
                let mut s = vello::Scene::new();
                stage::build_scene(&mut s, &units_ed[u], area, 1.0, &mut fp_cache);
                let now = fp_of_scene(&s);
                let lines_now = fp_cache.last_lines();
                runner.gpu.render_whole(&s);
                runner.gpu.poll();
                (read_target(&runner.gpu), now, lines_now)
            };
            let (a, fa, _) = render(f1);
            let (b, fb, _) = render(f2);
            pairs_done += 1;
            let (a2, fa2, _) = render(f1);
            let pass1_equal_f1 = fa == fps[f1 as usize][u];
            let pass1_equal_f2 = fb == fps[f2 as usize][u];
            let render_time_fp_equal = fa == fb;
            if !render_time_fp_equal {
                fp_differs_at_render_time += 1;
            }
            if fa != fa2 {
                encode_not_repeatable += 1;
            }
            if fa == fa2 && a != a2 {
                gpu_not_repeatable += 1;
            }
            if a != b {
                let diff_px = a
                    .chunks_exact(4)
                    .zip(b.chunks_exact(4))
                    .filter(|(x, y)| x != y)
                    .count();
                let max_d = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
                let category = if !render_time_fp_equal {
                    "encoding differs at render time (pass-1 fp was not reproducible)"
                } else if a != a2 {
                    "same encoding, GPU output not repeatable"
                } else {
                    "equal fp at render time, repeatable, yet pixels differ (fp misses an input)"
                };
                if render_time_fp_equal {
                    mismatches_equal_at_render += 1;
                }
                mismatches.push(serde_json::json!({
                    "layer": units[u].name, "f1": f1, "f2": f2,
                    "differing_px": diff_px, "max_channel_diff": max_d,
                    "category": category,
                    "pass1_fp_reproduced": [pass1_equal_f1, pass1_equal_f2],
                    "same_input_rerender_pixels_identical": a == a2,
                }));
            }
        }
    }
    println!(
        "soundness: {pairs_done} pairs, {} mismatches ({:.1} s)",
        mismatches.len(),
        t.elapsed().as_secs_f64()
    );

    // ---- final report.
    let mut live_rows: Vec<serde_json::Value> = Vec::new();
    for r in rows.iter().take(10) {
        if r.2 > 0 {
            live_rows.push(serde_json::json!([r.0, r.2, r.4]));
        }
    }
    let bottom_mib = f64::from(STAGE_W) * f64::from(STAGE_H) * 4.0 / (1024.0 * 1024.0);
    let bottom_mib_1080 = 1920.0 * 1080.0 * 4.0 / (1024.0 * 1024.0);
    let report = serde_json::json!({
        "status": "done",
        "runtime_min": (t_all.elapsed().as_secs_f64() / 60.0 * 10.0).round() / 10.0,
        "variant_static_vs_prev_frame": sum1,
        "variant_static_ge_12_frames": sum12,
        "ms_per_mline_fit": slope,
        "fit_intercept_ms": icpt,
        "fit_r2": r2,
        "fit_points": fit_pts.iter().map(|p| serde_json::json!([p.0, p.1])).collect::<Vec<_>>(),
        "fit_label": fit_label,
        "bottom_mib_each_1600x900": bottom_mib,
        "bottom_mib_each_1920x1080": bottom_mib_1080,
        "most_live_layers": live_rows,
        "static_run_stats": {
            "runs": all_runs.len(),
            "median": median(&all_runs),
            "p90": percentile(&all_runs, 0.9),
            "frames_in_runs_len_1": frames_in_runs[0],
            "frames_in_runs_len_2_11": frames_in_runs[1],
            "frames_in_runs_len_12_99": frames_in_runs[2],
            "frames_in_runs_len_100_plus": frames_in_runs[3],
        },
        "lights_active_frames": lights_active.iter().filter(|x| **x).count(),
        "lights_active": lights_enabled_anywhere,
        "non_normal_blends_in_document": blend_kinds,
        "soundness": {
            "pairs": pairs_done,
            "pixel_mismatches_any_cause": mismatches.len(),
            "pairs_whose_render_time_fp_differs_from_each_other": fp_differs_at_render_time,
            "pairs_where_same_input_encodes_differently_on_repeat": encode_not_repeatable,
            "pairs_where_identical_encoding_renders_differently_on_repeat": gpu_not_repeatable,
            "mismatches_with_equal_fp_at_render_time": mismatches_equal_at_render,
            "details": mismatches
        },
        "line_sum_check": ratio_note,
        "units": nu,
        "frames": n,
        "max_total_lines": out1.iter().map(|o| o.total_lines).max().unwrap_or(0),
    });
    println!("\n{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// `--picture-repeat <unit name> <frame> <reps>`: render one unit's live
/// encoding `reps` times and count distinct outputs — is the GPU repeatable
/// on identical input? (Diagnosis for the soundness check's 1-px flakes.)
fn run_picture_repeat(input: &str, name: &str, frame: u32, reps: usize) -> anyhow::Result<()> {
    let imported = read_input(input)?;
    let base = imported.scene;
    let (units, mut eds) = build_units(&base);
    let mut runner = Runner::new()?;
    runner.gpu.ctx.grow_to(8);
    let mut cache = DrawCache::default();
    cache.set_max_scale(runner.gpu.ctx.max_buffer_scale);
    cache.set_symbol_reuse(false);
    cache.set_scale(8);
    let u = units
        .iter()
        .position(|x| x.name == name)
        .ok_or_else(|| anyhow::anyhow!("no unit named {name}"))?;
    eds[u].current_frame = frame;
    let mut outputs: Vec<Vec<u8>> = Vec::new();
    let mut fps: Vec<u64> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for _ in 0..reps {
        let mut s = vello::Scene::new();
        stage::build_scene(&mut s, &eds[u], runner.area, 1.0, &mut cache);
        fps.push(fp_of_scene(&s));
        runner.gpu.render_whole(&s);
        runner.gpu.poll();
        let px = read_target(&runner.gpu);
        match outputs.iter().position(|o| *o == px) {
            Some(i) => counts[i] += 1,
            None => {
                outputs.push(px);
                counts.push(1);
            }
        }
    }
    fps.dedup();
    println!(
        "{name} frame {frame}: {reps} renders -> {} distinct outputs {:?}; distinct encodings {}; live lines {}",
        outputs.len(),
        counts,
        fps.len(),
        cache.last_lines()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// `--line-sources F1,F2,...`: S12 in `docs/BIMARI5_LAG_PLAN.md`. Measurement
// only. Needs `BUZZ_PROBE_LINE_SOURCES=1` (a probe-only hook in
// `buzz_render::probe` that records every line charge with its path's device
// bbox). Symbol cache OFF (live), culled, exactly as the app draws.
// ---------------------------------------------------------------------------

/// Mirror of `buzz_render`'s private `LINE_ESTIMATE_MARGIN`.
const LS_MARGIN: f64 = 1.02;

#[derive(Default, Clone)]
struct LsAgg {
    /// Raw lines by kind: fill, clip, stroke, seal.
    by_kind: [u64; 4],
    /// Records by kind.
    count: [u64; 4],
    /// Lines of paths whose bbox overlaps the viewport by < 20%, by kind.
    off20: [u64; 4],
    /// Lines of paths wholly outside the viewport (bbox inflated by half the
    /// stroke width + 1 px), by kind.
    out_full: [u64; 4],
    /// sum(lines * (1 - overlap fraction)): a rough upper bound on the part of
    /// every path that lies outside the viewport.
    partial_off_est: f64,
    /// Sub-pixel bboxes (both dims < 1 px / < 2 px), lines by kind.
    sub1: [u64; 4],
    sub2: [u64; 4],
    sub1_paths: u64,
    sub2_paths: u64,
}

fn ls_kind_idx(k: buzz_render::probe::Kind) -> usize {
    use buzz_render::probe::Kind;
    match k {
        Kind::Fill => 0,
        Kind::Clip => 1,
        Kind::Stroke => 2,
        Kind::Seal => 3,
    }
}

fn ls_axis(lo: f64, hi: f64, vlo: f64, vhi: f64) -> f64 {
    let len = hi - lo;
    if len <= 1e-9 {
        return if lo >= vlo && lo <= vhi { 1.0 } else { 0.0 };
    }
    ((hi.min(vhi) - lo.max(vlo)).max(0.0) / len).min(1.0)
}

fn ls_analyse(recs: &[buzz_render::probe::Rec]) -> LsAgg {
    let (w, h) = (f64::from(STAGE_W), f64::from(STAGE_H));
    let mut a = LsAgg::default();
    for r in recs {
        let k = ls_kind_idx(r.kind);
        a.by_kind[k] += r.lines;
        a.count[k] += 1;
        let b = r.bbox;
        let frac = ls_axis(b[0], b[2], 0.0, w) * ls_axis(b[1], b[3], 0.0, h);
        a.partial_off_est += r.lines as f64 * (1.0 - frac);
        if frac < 0.2 {
            a.off20[k] += r.lines;
        }
        let g = r.stroke_px / 2.0 + 1.0;
        if b[2] + g < 0.0 || b[0] - g > w || b[3] + g < 0.0 || b[1] - g > h {
            a.out_full[k] += r.lines;
        }
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        if bw < 2.0 && bh < 2.0 {
            a.sub2[k] += r.lines;
            a.sub2_paths += 1;
            if bw < 1.0 && bh < 1.0 {
                a.sub1[k] += r.lines;
                a.sub1_paths += 1;
            }
        }
    }
    a
}

fn ls_scaled(raw: u64) -> u64 {
    (raw as f64 * LS_MARGIN).ceil() as u64
}

fn ls_sum(a: &[u64; 4]) -> u64 {
    a.iter().sum()
}

/// One alpha readback of the stage target.
fn ls_render_alpha(runner: &mut Runner, editor: &Editor) -> (Vec<u8>, Vec<buzz_render::probe::Rec>, u64) {
    let mut scene = vello::Scene::new();
    stage::build_scene(&mut scene, editor, runner.area, 1.0, &mut runner.cache);
    let recs = buzz_render::probe::take();
    let lines = runner.cache.last_lines();
    runner.gpu.ctx.grow_to(runner.cache.scale());
    runner
        .gpu
        .ctx
        .render(&scene, &runner.gpu.target_view, STAGE_W, STAGE_H, Color::TRANSPARENT)
        .expect("vello render");
    runner.gpu.poll();
    (read_target(&runner.gpu), recs, lines)
}

fn run_line_sources(input: &str, frames: &[u32]) -> anyhow::Result<()> {
    use serde_json::json;
    if !buzz_render::probe::enabled() {
        anyhow::bail!("--line-sources needs BUZZ_PROBE_LINE_SOURCES=1 in the environment");
    }
    let imported = read_input(input)?;
    let base = imported.scene;
    let mut base_t = base.clone();
    base_t.stage_mut().background = Color::TRANSPARENT;
    let (units, mut editors_t) = build_units(&base_t);
    println!("line-sources: {} units", units.len());

    let mut runner = Runner::new()?;
    runner.cache.set_symbol_reuse(false); // live, as BUZZ_NO_SYMBOL_CACHE=1
    let (w, h) = (STAGE_W as usize, STAGE_H as usize);

    for &f in frames {
        let mut editor = framed_editor(base.clone());
        editor.current_frame = f;

        // --- timing (GPU ms) on the app's own ladder, warmed.
        runner.stage_bands = 0;
        let mut gpu_ms = f64::MAX;
        let mut last = None;
        for _ in 0..4 {
            let rec = runner.step(&editor);
            gpu_ms = if last.is_none() { rec.gpu_ms } else { gpu_ms.min(rec.gpu_ms) };
            last = Some(rec);
        }
        let last = last.unwrap();

        // --- records of the final pass of one full-frame encode.
        let mut scene = vello::Scene::new();
        stage::build_scene(&mut scene, &editor, runner.area, 1.0, &mut runner.cache);
        let recs = buzz_render::probe::take();
        // S12c: what the seal paths are made of, [paths, pre l/q/c, post l/q/c].
        let seal_makeup = buzz_render::probe::take_seal_makeup();
        let full_lines = runner.cache.last_lines();
        // S12c: Vello's own line count for this scene (`--features gpu-truth`).
        #[cfg(feature = "gpu-truth")]
        let gpu_truth_lines = {
            runner.gpu.ctx.grow_to(runner.cache.scale());
            let t = runner
                .gpu
                .ctx
                .render_measured(&scene, &runner.gpu.target_view, STAGE_W, STAGE_H, Color::BLACK)?;
            println!("GPU_TRUTH frame {f}: lines {} overflowed {}", t.lines, t.overflowed);
            t.lines
        };
        #[cfg(not(feature = "gpu-truth"))]
        let gpu_truth_lines = 0u64;
        println!("SEAL_MAKEUP frame {f}: paths {} pre l/q/c {:?} post l/q/c {:?}", seal_makeup[0], &seal_makeup[1..4], &seal_makeup[4..7]);
        if std::env::var_os("LS_SKIP_UNITS").is_some() {
            println!(
                "LS_JSON {}",
                json!({"frame": f, "lines": full_lines, "gpu_truth_lines": gpu_truth_lines,
                       "seal_lines": ls_scaled(ls_analyse(&recs).by_kind[3]),
                       "seal_makeup": seal_makeup.to_vec()})
            );
            continue;
        }
        let agg = ls_analyse(&recs);
        let raw_total: u64 = ls_sum(&agg.by_kind);
        println!(
            "frame {f}: lines {full_lines} (records*1.02 = {}) gpu {gpu_ms:.1} ms (step lines {}, scale {}, bands {}); \
             {} records",
            ls_scaled(raw_total),
            last.lines,
            last.scale,
            last.bands,
            recs.len()
        );

        // --- per unit.
        struct Row {
            name: String,
            delta: u64,
            alone: u64,
            mask_px: u64,
            occ_mask: f64,
            occ_bbox: f64,
            blend_above: bool,
            kinds: [u64; 4],
            top_recs: Vec<buzz_render::probe::Rec>,
        }
        let mut rows: Vec<Row> = Vec::new();
        for (i, unit) in units.iter().enumerate() {
            // hide-one delta (lines the unit adds to the full frame)
            let mut hidden = base.clone();
            for id in &unit.ids {
                hidden.update_layer(*id, |l| l.visible = false);
            }
            let mut eh = framed_editor(hidden);
            eh.current_frame = f;
            let mut s = vello::Scene::new();
            stage::build_scene(&mut s, &eh, runner.area, 1.0, &mut runner.cache);
            let _ = buzz_render::probe::take();
            let delta = full_lines.saturating_sub(runner.cache.last_lines());

            // alone (transparent stage): mask + bbox
            let ea = &mut editors_t[i];
            ea.current_frame = f;
            let (mask_px_buf, arecs, alone) = ls_render_alpha(&mut runner, &ea);
            let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for r in arecs.iter().skip(1) {
                bb[0] = bb[0].min(r.bbox[0] - r.stroke_px / 2.0);
                bb[1] = bb[1].min(r.bbox[1] - r.stroke_px / 2.0);
                bb[2] = bb[2].max(r.bbox[2] + r.stroke_px / 2.0);
                bb[3] = bb[3].max(r.bbox[3] + r.stroke_px / 2.0);
            }
            let (bx0, by0, bx1, by1) = (
                bb[0].floor().max(0.0) as usize,
                bb[1].floor().max(0.0) as usize,
                (bb[2].ceil().max(0.0) as usize).min(w),
                (bb[3].ceil().max(0.0) as usize).min(h),
            );

            // above (transparent stage): every unit above i, nothing else
            let above_ids: Vec<buzz_scene::LayerId> = units[i + 1..]
                .iter()
                .flat_map(|u| u.ids.iter().copied())
                .collect();
            let blend_above = units[i + 1..].iter().any(|u| u.blend_non_normal);
            let mut eab = framed_editor(only_scene(&base_t, &above_ids));
            eab.current_frame = f;
            let (above_buf, _, _) = ls_render_alpha(&mut runner, &eab);

            let (mut mask_px, mut covered) = (0u64, 0u64);
            for p in 0..w * h {
                if mask_px_buf[p * 4 + 3] > 0 {
                    mask_px += 1;
                    if above_buf[p * 4 + 3] == 255 {
                        covered += 1;
                    }
                }
            }
            let (mut bbox_px, mut bbox_cov) = (0u64, 0u64);
            for y in by0..by1 {
                for x in bx0..bx1 {
                    bbox_px += 1;
                    if above_buf[(y * w + x) * 4 + 3] == 255 {
                        bbox_cov += 1;
                    }
                }
            }
            rows.push(Row {
                name: unit.name.clone(),
                delta,
                alone,
                mask_px,
                occ_mask: if mask_px > 0 { covered as f64 / mask_px as f64 } else { -1.0 },
                occ_bbox: if bbox_px > 0 { bbox_cov as f64 / bbox_px as f64 } else { -1.0 },
                blend_above,
                kinds: {
                    let mut k = [0u64; 4];
                    for r in arecs.iter() {
                        k[ls_kind_idx(r.kind)] += r.lines;
                    }
                    k
                },
                top_recs: if alone > 300_000 {
                    let mut v: Vec<_> = arecs.clone();
                    v.sort_by_key(|r| std::cmp::Reverse(r.lines));
                    v.truncate(2000);
                    v
                } else {
                    Vec::new()
                },
            });
        }

        let mut by_delta: Vec<&Row> = rows.iter().collect();
        by_delta.sort_by_key(|r| std::cmp::Reverse(r.delta));
        println!("  top layers by hide-one delta (alone lines, mask px, occluded share of mask px / of bbox, blend above):");
        for r in by_delta.iter().take(10) {
            println!(
                "    {:>9} d  {:>9} alone  {:>8} px  occ {:>6.3} / {:>6.3}  blend_above={}  {}",
                r.delta, r.alone, r.mask_px, r.occ_mask, r.occ_bbox, r.blend_above, r.name
            );
            println!(
                "        raw lines fill/clip/stroke/seal = {:?}",
                r.kinds
            );
        }
        for r in by_delta.iter().take(10).filter(|r| !r.top_recs.is_empty()) {
            println!("  heavy layer {}: top records (kind, lines, device bbox w x h, stroke px):", r.name);
            for t in r.top_recs.iter().take(8) {
                println!(
                    "    {:?} {:>8}  {:>9.1} x {:>9.1}  bbox at ({:.0},{:.0})  stroke {:.2}",
                    t.kind,
                    t.lines,
                    t.bbox[2] - t.bbox[0],
                    t.bbox[3] - t.bbox[1],
                    t.bbox[0],
                    t.bbox[1],
                    t.stroke_px
                );
            }
            // lines-per-record spread by kind over the kept top 2000
            let mut big = [0u64; 4];
            for t in &r.top_recs {
                big[ls_kind_idx(t.kind)] += t.lines;
            }
            println!("    top-2000 records' raw lines fill/clip/stroke/seal = {big:?}");
        }
        // Occlusion sums, by hide-one delta. "sound" = no non-Normal blend unit above.
        let occ = |thr: f64, sound_only: bool| -> u64 {
            rows.iter()
                .filter(|r| r.occ_mask >= thr && (!sound_only || !r.blend_above))
                .map(|r| r.delta)
                .sum()
        };
        let occ_bbox_full: u64 = rows
            .iter()
            .filter(|r| r.occ_bbox >= 1.0 && !r.blend_above)
            .map(|r| r.delta)
            .sum();
        let no_px: u64 = rows.iter().filter(|r| r.mask_px == 0).map(|r| r.delta).sum();
        let sum_delta: u64 = rows.iter().map(|r| r.delta).sum();
        let sum_alone: u64 = rows.iter().map(|r| r.alone).sum();
        let paths = agg.count[0] + agg.count[2] + agg.count[3];
        let drawn = agg.by_kind[0] + agg.by_kind[2] + agg.by_kind[3];
        let j = json!({
            "frame": f,
            "lines": full_lines,
            "gpu_ms": (gpu_ms * 10.0).round() / 10.0,
            "top_layers": by_delta.iter().take(10).map(|r| json!([r.name, r.delta])).collect::<Vec<_>>(),
            "seal_lines": ls_scaled(agg.by_kind[3]),
            "stroke_lines": ls_scaled(agg.by_kind[2]),
            "fill_lines": ls_scaled(agg.by_kind[0]),
            "clip_group_lines": ls_scaled(agg.by_kind[1]),
            "occluded_full_lines": occ(1.0, true),
            "occluded_95_lines": occ(0.95, true),
            "occluded_full_lines_incl_blend_above": occ(1.0, false),
            "occluded_95_lines_incl_blend_above": occ(0.95, false),
            "occluded_bbox_full_lines": occ_bbox_full,
            "layers_with_no_visible_px_lines": no_px,
            "mostly_offscreen_path_lines": ls_scaled(ls_sum(&agg.off20)),
            "mostly_offscreen_by_kind_fill_clip_stroke_seal": agg.off20.iter().map(|x| ls_scaled(*x)).collect::<Vec<_>>(),
            "fully_outside_path_lines": ls_scaled(ls_sum(&agg.out_full)),
            "fully_outside_by_kind_fill_clip_stroke_seal": agg.out_full.iter().map(|x| ls_scaled(*x)).collect::<Vec<_>>(),
            "outside_viewport_upper_bound_lines": ls_scaled(agg.partial_off_est.round() as u64),
            "subpixel_lt1_lines": ls_scaled(ls_sum(&agg.sub1)),
            "subpixel_lt2_lines": ls_scaled(ls_sum(&agg.sub2)),
            "subpixel_lt1_by_kind_fill_clip_stroke_seal": agg.sub1.iter().map(|x| ls_scaled(*x)).collect::<Vec<_>>(),
            "subpixel_lt2_by_kind_fill_clip_stroke_seal": agg.sub2.iter().map(|x| ls_scaled(*x)).collect::<Vec<_>>(),
            "subpixel_lt1_paths": agg.sub1_paths,
            "subpixel_lt2_paths": agg.sub2_paths,
            "paths": paths,
            "clip_records": agg.count[1],
            "records_by_kind_fill_clip_stroke_seal": agg.count,
            "avg_lines_per_path": if paths > 0 { (ls_scaled(drawn) as f64 / paths as f64 * 100.0).round() / 100.0 } else { 0.0 },
            "sum_hide_one_delta": sum_delta,
            "sum_alone_lines": sum_alone,
        });
        println!("LS_JSON {j}");
    }
    Ok(())
}


// -- --pipelined (E7): the encode thread and the seek fan-out, measured -------

/// One frame of a pipelined playback, as the render thread lived it.
#[derive(Debug, Clone, Copy)]
struct PipeRecord {
    frame: u32,
    /// Render-thread time: waiting for the encode thread + `gpu.render` call +
    /// device poll (for a frame the render thread encoded itself: its encode too).
    render_thread_ms: f64,
    wait_ms: f64,
    render_ms: f64,
    /// The encode thread's own time for this frame (`None` for a frame encoded
    /// on the render thread).
    encode_thread_ms: Option<f64>,
    queue: usize,
    not_ready: bool,
    installs: u64,
    pipelined: bool,
    /// S7: the `gpu.render` call and the `device.poll(Wait)` after it, split.
    call_ms: f64,
    poll_ms: f64,
    /// S7 (`FRAME_COST_DIAG=1`): bytes the device had allocated after this
    /// frame (`generate_allocator_report`), in MiB; `0` when not sampled.
    alloc_mib: f64,
    /// S7: the encoding's own size in bytes (as `scene_bytes`).
    enc_mb: f64,
    /// S7: is this the first time this frame number was played in this run?
    first: bool,
    /// S7: unix time (s) when this frame's render finished, for lining up with
    /// an external GPU-clock log.
    at_unix_s: f64,
}

#[derive(Default)]
struct PipeReport {
    name: String,
    records: Vec<PipeRecord>,
    /// Latency of every `Pipeline::stop` (ms) until the cache was home.
    stop_ms: Vec<f64>,
    /// S7: each stop's steps (reaction, pre-build reset, hand-back, receive), ms.
    stop_parts: Vec<[f64; 4]>,
    /// S7: the encode thread's phase at each stop, and the aborted pass's tail (ms).
    stop_phase: Vec<(&'static str, f64)>,
    fallbacks: u32,
    rss_start: Option<f64>,
    rss_max: Option<f64>,
    rss_end: Option<f64>,
    wall_s: f64,
    late_frames: u32,
}

impl PipeReport {
    fn print(&self, interval_ms: f64) {
        let mut totals: Vec<f64> = self.records.iter().map(|r| r.render_thread_ms).collect();
        totals.sort_by(f64::total_cmp);
        let over = totals.iter().filter(|t| **t > 25.0).count();
        let piped = self.records.iter().filter(|r| r.pipelined).count();
        let not_ready = self.records.iter().filter(|r| r.not_ready).count();
        let mut enc: Vec<f64> = self.records.iter().filter_map(|r| r.encode_thread_ms).collect();
        enc.sort_by(f64::total_cmp);
        let q_avg = self.records.iter().map(|r| r.queue as f64).sum::<f64>()
            / self.records.len().max(1) as f64;
        let q_min = self.records.iter().filter(|r| r.pipelined).map(|r| r.queue).min().unwrap_or(0);
        let installs: u64 = self.records.iter().map(|r| r.installs).sum();
        let max_stop = self.stop_ms.iter().copied().fold(0.0, f64::max);
        let again: Vec<&PipeRecord> = self.records.iter().filter(|r| !r.first).collect();
        println!(
            "  after first appearance: {} frames, render thread max {:.2} ms, over 30 ms: {} | GPU part (render call + poll) max {:.2} ms, over 30 ms: {}",
            again.len(),
            again.iter().map(|r| r.render_thread_ms).fold(0.0, f64::max),
            again.iter().filter(|r| r.render_thread_ms > 30.0).count(),
            again.iter().map(|r| r.render_ms).fold(0.0, f64::max),
            again.iter().filter(|r| r.render_ms > 30.0).count(),
        );
        println!(
            "scenario {} (pipelined, sim clock {:.2} ms/frame): {} frames ({} pipelined, {} on the render thread, {} fallbacks) | wall {:.1} s",
            self.name,
            interval_ms,
            self.records.len(),
            piped,
            self.records.len() - piped,
            self.fallbacks,
            self.wall_s,
        );
        println!(
            "  render thread: p50 {:.2} ms  p95 {:.2} ms  max {:.2} ms | frames > 25 ms: {over} | not-ready: {not_ready} | dropped (playhead skipped): {}",
            median(&totals),
            percentile(&totals, 0.95),
            totals.last().copied().unwrap_or(0.0),
            self.late_frames,
        );
        println!(
            "  encode thread: p50 {:.2} ms  p95 {:.2} ms  max {:.2} ms | queue depth avg {:.2} min {} | stamps installed {installs}",
            median(&enc),
            percentile(&enc, 0.95),
            enc.last().copied().unwrap_or(0.0),
            q_avg,
            q_min,
        );
        // S9: the heavy frames' own numbers (`FRAME_COST_HEAVY=4608,4636,...`).
        let heavy: Vec<u32> = std::env::var("FRAME_COST_HEAVY")
            .ok()
            .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_else(|| vec![4608, 4636, 4700, 0, 20]);
        for f in heavy {
            for r in self.records.iter().filter(|r| r.frame == f) {
                println!(
                    "  heavy frame {f:>5}: encode thread {:?} ms | render thread {:.2} ms (wait {:.2}, call {:.2}, poll {:.2}) | pipelined {} not-ready {} first {} | enc {:.1} MB",
                    r.encode_thread_ms.map(|e| (e * 100.0).round() / 100.0),
                    r.render_thread_ms,
                    r.wait_ms,
                    r.call_ms,
                    r.poll_ms,
                    r.pipelined,
                    r.not_ready,
                    r.first,
                    r.enc_mb,
                );
            }
        }
        println!(
            "  stops {} | max stop latency {:.2} ms | RSS start {:?} max {:?} end {:?} MiB (growth {:?})",
            self.stop_ms.len(),
            max_stop,
            self.rss_start.map(|r| r.round()),
            self.rss_max.map(|r| r.round()),
            self.rss_end.map(|r| r.round()),
            self.rss_start.zip(self.rss_end).map(|(a, b)| (b - a).round()),
        );
        if let Some(path) = std::env::var_os("FRAME_COST_CSV") {
            // S7: one line per frame, for offline analysis.
            let mut csv = String::from(
                "idx,frame,total_ms,wait_ms,call_ms,poll_ms,alloc_mib,enc_mb,first,not_ready,pipelined,queue,installs,encode_ms,at_unix_s
",
            );
            for (k, r) in self.records.iter().enumerate() {
                csv.push_str(&format!(
                    "{k},{},{:.3},{:.3},{:.3},{:.3},{:.1},{:.2},{},{},{},{},{},{:.3},{:.3}
",
                    r.frame,
                    r.render_thread_ms,
                    r.wait_ms,
                    r.call_ms,
                    r.poll_ms,
                    r.alloc_mib,
                    r.enc_mb,
                    u8::from(r.first),
                    u8::from(r.not_ready),
                    u8::from(r.pipelined),
                    r.queue,
                    r.installs,
                    r.encode_thread_ms.unwrap_or(0.0),
                    r.at_unix_s,
                ));
            }
            let _ = std::fs::write(path, csv);
        }
        if std::env::var_os("FRAME_COST_DIAG").is_some() {
            // S7: every frame whose GPU part (render call + poll) took > 40 ms,
            // in playback order, with the device's allocated bytes after it.
            println!("  diag: frames with render call + poll > 40 ms (playback order):");
            let mut prev_alloc = 0.0;
            for (k, r) in self.records.iter().enumerate() {
                if r.render_ms > 40.0 {
                    println!(
                        "    #{k:>5} frame {:>5}: call {:>7.2} poll {:>7.2}  alloc {:>7.0} MiB (delta {:+.0})  enc {:.1} MB  first {}  not-ready {}  pipelined {}",
                        r.frame, r.call_ms, r.poll_ms, r.alloc_mib, r.alloc_mib - prev_alloc, r.enc_mb, r.first, r.not_ready, r.pipelined
                    );
                }
                if r.alloc_mib > 0.0 {
                    prev_alloc = r.alloc_mib;
                }
            }
            let mut allocs = 0;
            let mut last = 0.0;
            for r in &self.records {
                if r.alloc_mib > 0.0 {
                    if (r.alloc_mib - last).abs() > 1.0 {
                        allocs += 1;
                    }
                    last = r.alloc_mib;
                }
            }
            println!("  diag: frames where the device's allocated bytes changed by > 1 MiB: {allocs}");
        }
        println!("  process peak working set: {:?} MiB", peak_rss_mib().map(f64::round));
        for (k, p) in self.stop_parts.iter().enumerate() {
            println!(
                "  stop #{k}: total {:.2} ms = reaction {:.2} + prebuild reset {:.2} + hand-back {:.2} + receive {:.2} | thread was in {} (pass ended {:.2} ms after the abort)",
                self.stop_ms.get(k).copied().unwrap_or(0.0),
                p[0], p[1], p[2], p[3],
                self.stop_phase.get(k).map_or("?", |x| x.0),
                self.stop_phase.get(k).map_or(0.0, |x| x.1)
            );
        }
        let mut worst = self.records.clone();
        worst.sort_by(|a, b| b.render_thread_ms.total_cmp(&a.render_thread_ms));
        worst.truncate(WORST_KEPT);
        println!("  worst render-thread frames:");
        for r in &worst {
            println!(
                "    frame {:>5}: {:>8.2} ms (wait {:>7.2}  render+poll {:>7.2} = call {:>7.2} + poll {:>7.2})  alloc {:.0} MiB  enc {:.1} MB  first {}  encode-thread {:?}  queue {}  pipelined {}  not-ready {}",
                r.frame,
                r.render_thread_ms,
                r.wait_ms,
                r.render_ms,
                r.call_ms,
                r.poll_ms,
                r.alloc_mib,
                r.enc_mb,
                r.first,
                r.encode_thread_ms.map(|e| (e * 100.0).round() / 100.0),
                r.queue,
                r.pipelined,
                r.not_ready
            );
        }
    }
}

/// `FRAME_COST_FRAMES=0-100,4500-4800` plays just those ranges (a jump between
/// them); default is the whole film.
fn pipelined_frames(total: u32) -> Vec<u32> {
    match std::env::var("FRAME_COST_FRAMES") {
        Ok(v) => v
            .split(',')
            .filter_map(|r| {
                let (a, b) = r.split_once('-')?;
                Some(a.trim().parse::<u32>().ok()?..=b.trim().parse::<u32>().ok()?)
            })
            .flatten()
            .filter(|f| *f < total)
            .collect(),
        Err(_) => (0..total).collect(),
    }
}

/// A stand-in for the cache while the encode thread has the real one.
fn stand_in_for(cache: &DrawCache) -> DrawCache {
    let mut s = DrawCache::default();
    s.set_scale(cache.scale());
    s.set_max_scale(cache.max_scale());
    s
}

fn epoch_for(revision: u64, cache: &DrawCache, inputs: &stage::StageInputs) -> Epoch {
    Epoch {
        revision,
        camera: [0; 4],
        area: [0; 4],
        scale_bits: 1.0f64.to_bits(),
        detail: cache.detail(),
        lights_generation: 0,
        buffer_scale: inputs.buffer_scale,
    }
}

/// Play `frames` (in order) through the encode thread the way `App::render`
/// does, on a simulated clock of one frame per `interval`, recording what the
/// render thread paid. A jump in `frames`, a band-mode frame or an injected
/// stop (every `STOP_EVERY` pipelined frames, standing in for an edit) is drawn
/// by the render thread; the pipeline restarts after it.
fn play_pipelined(
    name: &str,
    runner: &mut Runner,
    editor: &mut Editor,
    pipeline: &mut Pipeline,
    frames: &[u32],
    interval: Duration,
) -> PipeReport {
    // `FRAME_COST_STOP_EVERY=40` stops far more often, for the stop-latency numbers.
    let stop_every: usize = std::env::var("FRAME_COST_STOP_EVERY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    let mut report = PipeReport { name: name.into(), ..PipeReport::default() };
    report.rss_start = rss_mib();
    report.rss_max = report.rss_start;
    let play = PlayRange { count: editor.playing_length(), looping: false };
    let mut shown = vello::Scene::new();
    let mut revision = 0u64;
    let mut since_stop = 0usize;
    let wall = Instant::now();
    let start_clock = Instant::now();

    let stop = |pipeline: &mut Pipeline, runner: &mut Runner, report: &mut PipeReport| {
        if pipeline.is_running() {
            match pipeline.stop() {
                Stopped::Returned(c) => runner.cache = *c,
                _ => panic!("the encode thread lost the cache"),
            }
            report.stop_ms.push(pipeline.stats.last_stop_ms);
            report.stop_parts.push(pipeline.stats.last_stop_parts_ms);
            report
                .stop_phase
                .push((pipeline.stats.last_stop_phase, pipeline.stats.last_stop_build_tail_ms));
        }
    };

    // **The playhead follows the wall clock, as `Editor::advance_playback` does:**
    // frame `i` of `frames` is due at `i * interval`; a render that runs late
    // finds the playhead further on and the frames between are dropped.
    let per = interval.as_secs_f64();
    let mut last_i: Option<usize> = None;
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let diag = std::env::var_os("FRAME_COST_DIAG").is_some();
    // Which `FRAME_COST_FRAMES` range each frame belongs to (a change of range
    // is a seek, not playback).
    let mut range_id = vec![0u32; frames.len()];
    for k in 1..frames.len() {
        range_id[k] = range_id[k - 1] + u32::from(frames[k] != frames[k - 1] + 1);
    }
    loop {
        let due = start_clock.elapsed().as_secs_f64() / per;
        let i = (due.floor() as usize).min(frames.len().saturating_sub(1));
        if Some(i) == last_i {
            if due >= frames.len() as f64 {
                break;
            }
            let wait = ((i + 1) as f64 - due) * per;
            std::thread::sleep(Duration::from_secs_f64(wait.max(0.0)));
            continue;
        }
        let f = frames[i];
        if let Some(l) = last_i {
            report.late_frames += (i - l - 1) as u32;
        }
        let same_run = last_i.is_some_and(|l| range_id[l] == range_id[i]);
        last_i = Some(i);
        editor.current_frame = f;
        // S7: as `App::render` does while playing (`BUZZ_NO_GPU_WARM=1` turns it off).
        runner.gpu.ctx.keep_warm();
        let installs_before = pipeline.prebuild_stats().installed;
        let contiguous = same_run;
        let t0 = Instant::now();
        let mut record = PipeRecord {
            frame: f,
            render_thread_ms: 0.0,
            wait_ms: 0.0,
            render_ms: 0.0,
            encode_thread_ms: None,
            queue: 0,
            not_ready: false,
            installs: 0,
            pipelined: false,
            call_ms: 0.0,
            poll_ms: 0.0,
            alloc_mib: 0.0,
            enc_mb: 0.0,
            first: seen.insert(f),
            at_unix_s: 0.0,
        };

        let mut pipelined = pipeline.is_running() && contiguous;
        if pipeline.is_running() && !contiguous {
            stop(pipeline, runner, &mut report);
        }
        if pipelined {
            pipeline.playhead(f);
            record.queue = pipeline.queued();
            let t = Instant::now();
            match pipeline.take(f, interval) {
                Taken::Ready(mut p) if p.over_budget() => {
                    pipeline.recycle(std::mem::replace(&mut p.scene, vello::Scene::new()));
                    report.fallbacks += 1;
                    stop(pipeline, runner, &mut report);
                    pipelined = false;
                }
                Taken::Ready(mut p) => {
                    record.wait_ms = ms(t);
                    record.encode_thread_ms = Some(p.encode_ms);
                    std::mem::swap(&mut shown, &mut p.scene);
                    runner.gpu.ctx.grow_to(p.scale());
                    pipeline.recycle(p.scene);
                }
                Taken::Shown => {}
                Taken::NotReady => {
                    record.wait_ms = ms(t);
                    record.not_ready = true;
                }
            }
        }
        if pipelined {
            let t = Instant::now();
            runner.gpu.render_whole(&shown);
            let call = ms(t);
            let t = Instant::now();
            runner.gpu.poll();
            record.call_ms = call;
            record.poll_ms = ms(t);
            record.render_ms = call + record.poll_ms;
            record.enc_mb = scene_bytes(&shown) as f64 * 1e-6;
            record.pipelined = true;
            since_stop += 1;
            if since_stop >= stop_every {
                // An edit arrives: the cache comes home, the next frame restarts.
                stop(pipeline, runner, &mut report);
                since_stop = 0;
            }
        } else {
            // The render thread's own path: encode + render (`Runner::step`).
            let rec = runner.step(editor);
            record.render_ms = rec.encode_ms + rec.gpu_ms;
            // The next frame is drawn ahead from here, when the stage allows.
            let next = frames.get(i + 1).copied();
            if runner.stage_bands == 0
                && buzz_app::encode_ahead::enabled()
                && next == Some(f + 1)
                && let Some(first) = step_frame(editor.scene(), play, f)
            {
                revision += 1;
                let inputs = stage::StageInputs::from_editor(editor, runner.area, 1.0);
                let epoch = epoch_for(revision, &runner.cache, &inputs);
                let stand_in = stand_in_for(&runner.cache);
                let cache = std::mem::replace(&mut runner.cache, stand_in);
                pipeline.start(cache, epoch, inputs, play, first, f);
            }
        }
        record.render_thread_ms = ms(t0);
        record.at_unix_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        record.installs = pipeline.prebuild_stats().installed - installs_before;
        if diag
            && let Some(r) = runner.gpu.ctx.device.generate_allocator_report()
        {
            record.alloc_mib = r.total_allocated_bytes as f64 / (1024.0 * 1024.0);
        }
        report.records.push(record);
        if i % 200 == 0
            && let Some(r) = rss_mib()
        {
            report.rss_max = Some(report.rss_max.map_or(r, |m| m.max(r)));
        }
        if i + 1 >= frames.len() {
            break;
        }
    }
    stop(pipeline, runner, &mut report);
    report.wall_s = wall.elapsed().as_secs_f64();
    report.rss_end = rss_mib();
    if let Some(r) = report.rss_end {
        report.rss_max = Some(report.rss_max.map_or(r, |m| m.max(r)));
    }
    report
}

fn frame_interval(scene: &Scene) -> Duration {
    // The film's own rate (the brief's simulated clock is 24 fps).
    let fps = f64::from(scene.stage().frame_rate);
    Duration::from_secs_f64(1.0 / if fps.is_finite() && fps > 0.0 { fps } else { 24.0 })
}

fn scenario_a_pipelined(scene: &Scene, warm_open: bool) -> anyhow::Result<()> {
    let mut editor = framed_editor(scene.clone());
    let total = editor.scene().frame_count();
    let frames = pipelined_frames(total);
    let mut runner = Runner::new()?;
    let jobs = Arc::new(buzz_jobs::JobSystem::new());
    if warm_open {
        warm_open_into(&mut runner, &editor, &jobs, None, jobs.interactive_threads());
    }
    let mut pipeline = Pipeline::for_app(jobs);
    let interval = frame_interval(scene);
    let report = play_pipelined("a", &mut runner, &mut editor, &mut pipeline, &frames, interval);
    report.print(interval.as_secs_f64() * 1e3);
    println!("  final buffer scale {} (cache {})", runner.gpu.ctx.buffer_scale, runner.cache.scale());
    let s = pipeline.prebuild_stats();
    println!(
        "  prebuild: dispatched {} installed {} stale {} | pipeline: taken {} not_ready {} discarded {} stops {}",
        s.dispatched,
        s.installed,
        s.stale,
        pipeline.stats.taken,
        pipeline.stats.not_ready,
        pipeline.stats.discarded,
        pipeline.stats.stops
    );
    Ok(())
}

/// Scenario d, pipelined: play `0..=300` through the encode thread, then the
/// idle-shrink steps, then jump to frames 20 and 900 with the seek fan-out.
/// Then the fan-out against a single thread on a *cold* cache for each target.
fn scenario_d_pipelined(scene: &Scene, warm_open: bool) -> anyhow::Result<()> {
    let mut editor = framed_editor(scene.clone());
    let total = editor.scene().frame_count();
    let mut runner = Runner::new()?;
    let jobs = Arc::new(buzz_jobs::JobSystem::new());
    if warm_open {
        warm_open_into(&mut runner, &editor, &jobs, None, jobs.interactive_threads());
    }
    let mut pipeline = Pipeline::for_app(Arc::clone(&jobs));
    let interval = frame_interval(scene);
    let frames: Vec<u32> = (0..=300u32).filter(|f| *f < total).collect();
    let report = play_pipelined(
        "d (play 0..=300)",
        &mut runner,
        &mut editor,
        &mut pipeline,
        &frames,
        interval,
    );
    report.print(interval.as_secs_f64() * 1e3);

    // Idle shrink, as scenario d.
    let pixels = f64::from(STAGE_W) * f64::from(STAGE_H);
    while runner.cache.scale() > 1 {
        runner.cache.note_shrink_headroom(pixels, Instant::now());
        runner
            .cache
            .note_shrink_headroom(pixels, Instant::now() + Duration::from_secs(25));
        let Some(half) = runner.cache.pending_shrink() else {
            break;
        };
        runner.gpu.ctx.shrink_to(half)?;
        runner.cache.commit_shrink(half);
        println!("  scenario d (pipelined): shrank to scale {half}");
    }

    // The jumps, with the fan-out in front of the encode.
    let mut seek = SeekAhead::new(Arc::clone(&jobs));
    println!("  jumps with the seek fan-out (after the playback above):");
    for f in [20u32, 900] {
        if f >= total {
            continue;
        }
        editor.current_frame = f;
        let inputs = stage::StageInputs::from_editor(&editor, runner.area, 1.0);
        let epoch = epoch_for(1, &runner.cache, &inputs);
        let jobs_before = seek.stats.fan_jobs;
        println!(
            "    (before frame {f}: lights empty {}, symbol reuse {}, retention {:?})",
            runner.cache.lights.is_empty(),
            runner.cache.symbol_reuse(),
            runner.cache.symbol_scenes.retention_stats()
        );
        let t = Instant::now();
        let installed = seek.before_encode(epoch, &mut runner.cache, &inputs, true);
        let fan_ms = ms(t);
        let builds_before = runner.cache.symbol_scenes.builds;
        let rec = runner.step(&editor);
        let built_here = runner.cache.symbol_scenes.builds - builds_before;
        seek.after_encode(&mut runner.cache, &inputs, editor.playing_length(), built_here > 0);
        println!(
            "    frame {f:>5}: fan-out {fan_ms:.2} ms ({installed} stamps installed, {} jobs) + warm encode {:.2} ms + gpu {:.2} ms = {:.2} ms | render thread built {built_here} stamps itself",
            seek.stats.fan_jobs - jobs_before,
            rec.encode_ms,
            rec.gpu_ms,
            fan_ms + rec.encode_ms + rec.gpu_ms,
        );
    }

    // Fan-out against one thread, cold cache, CPU only (no GPU): the same
    // `build_scene` the render thread would run, with and without the fan-out.
    println!(
        "  fan-out vs single thread, cold cache each (CPU: dispatch..install + warm encode vs one cold encode):"
    );
    for f in [20u32, 715, 900, 4700] {
        if f >= total {
            continue;
        }
        editor.current_frame = f;
        let area = runner.area;
        let max_scale = runner.gpu.ctx.max_buffer_scale;
        // The symbol memo is built once when a document opens, so every cache
        // here starts with it (it is not what a seek pays for).
        let mut proto = DrawCache::default();
        proto.symbols.refresh(editor.scene());
        let mut single_cache = DrawCache::default();
        single_cache.set_max_scale(max_scale);
        single_cache.adopt_symbols(&proto.symbols);
        let mut single_scene = vello::Scene::new();
        let t = Instant::now();
        stage::build_scene(&mut single_scene, &editor, area, 1.0, &mut single_cache);
        let single_ms = ms(t);

        // The same cold encode with every stamp built on first sight, one
        // thread: what a warm encode costs without workers.
        let mut build_cache = DrawCache::default();
        build_cache.set_max_scale(max_scale);
        build_cache.adopt_symbols(&proto.symbols);
        build_cache.set_build_on_first_sighting(true);
        let mut build_scene = vello::Scene::new();
        let t = Instant::now();
        stage::build_scene(&mut build_scene, &editor, area, 1.0, &mut build_cache);
        let single_build_ms = ms(t);

        let mut fan_cache = DrawCache::default();
        fan_cache.set_max_scale(max_scale);
        fan_cache.adopt_symbols(&proto.symbols);
        let mut fan = SeekAhead::new(Arc::clone(&jobs));
        let inputs = stage::StageInputs::from_editor(&editor, area, 1.0);
        let epoch = epoch_for(1, &fan_cache, &inputs);
        let t = Instant::now();
        let installed = fan.before_encode(epoch, &mut fan_cache, &inputs, true);
        let fan_ms = ms(t);
        let mut warm_scene = vello::Scene::new();
        let builds_before = fan_cache.symbol_scenes.builds;
        let t = Instant::now();
        stage::build_scene(&mut warm_scene, &editor, area, 1.0, &mut fan_cache);
        let warm_ms = ms(t);
        // How the work divides: each layer share built alone on a fresh slot
        // cache (build on first sight), one after another.
        let layers_here = editor.scene().layers().drawable_at(f).count();
        let n = 8usize;
        let mut shares = Vec::new();
        for k in 0..n {
            let mut slot = DrawCache::default();
            slot.set_max_scale(max_scale);
            slot.adopt_symbols(&proto.symbols);
            if k == 0 {
                let inputs0 = stage::StageInputs::from_editor(&editor, area, 1.0);
                let t = Instant::now();
                slot.symbols.refresh(&inputs0.scene);
                println!("      (symbols.refresh on an adopted slot: {:.2} ms)", ms(t));
            }
            slot.set_build_on_first_sighting(true);
            slot.set_work_slice(Some((k, n)));
            let mut scratch = vello::Scene::new();
            let inputs = stage::StageInputs::from_editor(&editor, area, 1.0);
            slot.begin();
            let t = Instant::now();
            let cull = stage::cull_for(&inputs.camera);
            let _ = stage::encode_plain(&mut scratch, &inputs, &mut slot, cull);
            slot.end();
            let first_ms = ms(t);
            // The same share again on the same slot: memo and stamps warm.
            scratch.reset();
            slot.begin();
            let t = Instant::now();
            let _ = stage::encode_plain(&mut scratch, &inputs, &mut slot, cull);
            slot.end();
            shares.push((
                (first_ms * 10.0).round() / 10.0,
                slot.symbol_scenes.builds,
                (ms(t) * 10.0).round() / 10.0,
            ));
        }
        println!("    frame {f:>5}: {layers_here} layers; per-share (first pass ms, builds, second pass ms) when run alone: {shares:?}");
        println!(
            "    frame {f:>5}: single {single_ms:.2} ms (stamps drawn live on first sight) | single, build-all {single_build_ms:.2} ms | fan-out {fan_ms:.2} ms ({installed} stamps, {} jobs) + warm encode {warm_ms:.2} ms = {:.2} ms | warm encode built {} stamps",
            fan.stats.fan_jobs,
            fan_ms + warm_ms,
            fan_cache.symbol_scenes.builds - builds_before,
        );
    }
    Ok(())
}
fn main() -> anyhow::Result<()> {
    // Diagnosis: allocation churn (20 threads, mid-size blocks held then freed),
    // to see whether a churned heap alone slows the pool jobs.
    if let Some(g) = std::env::var("FRAME_COST_CHURN_GIB").ok().and_then(|v| v.parse::<usize>().ok()) {
        let t = Instant::now();
        let hs: Vec<_> = (0..20)
            .map(|k| {
                std::thread::spawn(move || {
                    let mut held: Vec<Vec<u8>> = Vec::new();
                    let mut x = 0x9E3779B97F4A7C15u64 ^ k;
                    let mut total = 0usize;
                    while total < (g << 30) / 20 {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        let size = 4096 + (x % (900 * 1024)) as usize;
                        held.push(vec![(x & 0xff) as u8; size]);
                        total += size;
                        if x % 3 == 0 && held.len() > 8 {
                            let i = (x as usize >> 8) % held.len();
                            total -= held.swap_remove(i).len();
                        }
                    }
                    drop(held);
                })
            })
            .collect();
        for h in hs {
            let _ = h.join();
        }
        println!("churned {g} GiB of mid-size blocks in {:.1} s", t.elapsed().as_secs_f64());
    }
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or_else(|| {
        anyhow::anyhow!(
            "usage: frame_cost <file.fla> [a|b|c|d|all] | frame_cost <file.fla> --layers <frame>"
        )
    })?;
    let rest: Vec<String> = args.collect();

    if let Some(pos) = rest.iter().position(|a| a == "--line-sources") {
        let frames: Vec<u32> = rest
            .get(pos + 1)
            .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_default();
        anyhow::ensure!(!frames.is_empty(), "--line-sources needs F1,F2,...");
        return run_line_sources(&input, &frames);
    }
    if let Some(pos) = rest.iter().position(|a| a == "--picture-repeat") {
        let name = rest.get(pos + 1).cloned().unwrap_or_default();
        let frame: u32 = rest.get(pos + 2).and_then(|s| s.parse().ok()).unwrap_or(0);
        let reps: usize = rest.get(pos + 3).and_then(|s| s.parse().ok()).unwrap_or(20);
        return run_picture_repeat(&input, &name, frame, reps);
    }
    if rest.iter().any(|a| a == "--picture-probe") {
        return run_picture_probe(&input);
    }

    if let Some(pos) = rest.iter().position(|a| a == "--layers") {
        let frame: u32 = rest
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("--layers needs a frame number"))?;
        return run_layers(&input, frame);
    }
    let pipelined = rest.iter().any(|a| a == "--pipelined");
    let warm_open = rest.iter().any(|a| a == "--warm-open");
    let scenario = rest
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str)
        .unwrap_or("all");

    let started = Instant::now();
    let imported = read_input(&input)?;
    println!("import: {:.0} ms — {}", ms(started), imported.summary);
    let scene = imported.scene;

    let total_frames = framed_editor(scene.clone()).scene().frame_count();
    println!("total frames: {total_frames}");
    println!("stage frame_rate: {}", scene.stage().frame_rate);

    if pipelined {
        // `--pipelined`: scenarios a and d only, through the encode thread.
        if scenario == "a" || scenario == "all" {
            scenario_a_pipelined(&scene, warm_open)?;
        }
        if scenario == "d" || scenario == "all" {
            scenario_d(&scene)?.print();
            scenario_d_pipelined(&scene, warm_open)?;
        }
        return Ok(());
    }
    if scenario == "a" || scenario == "all" {
        scenario_a(&scene)?.print();
    }
    if scenario == "b" || scenario == "all" {
        scenario_b(&scene)?.print();
    }
    if scenario == "c" || scenario == "all" {
        scenario_c(&scene, 4.0)?.print();
        scenario_c(&scene, 16.0)?.print();
    }
    if scenario == "d" || scenario == "all" {
        scenario_d(&scene)?.print();
    }
    if scenario == "warm" {
        scenario_warm(&scene)?;
    }
    if scenario == "warm-measure" {
        scenario_warm_measure(&scene)?;
    }

    Ok(())
}
