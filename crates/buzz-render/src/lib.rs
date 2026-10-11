//! GPU device setup and Vello-backed vector rendering.
//!
//! Two responsibilities:
//!
//! * [`adapter`] — picking the right GPU, which is genuinely non-trivial on a
//!   workstation with virtual display drivers installed.
//! * [`GpuContext`] — the device, queue and Vello renderer, plus the scene
//!   building that honours the rebasing contract from `buzz-geom`.
//! * [`document`] — the walk that turns a [`buzz_scene::Scene`] into Vello
//!   drawing commands. It sits here rather than in the application because the
//!   window, the exporter and the headless tests must all encode a document
//!   the same way; an export that does not match the screen is the worst bug
//!   an animation tool can have.

pub mod adapter;
pub mod compositor;
pub mod document;
pub mod filters;
pub mod lighting;
pub mod warm;

use anyhow::{Context, Result};
use buzz_geom::{Affine, BezPath, Camera, PathEl, Point, RenderClip, RenderSplit, Shape, Vec2};
use buzz_scene::{GradientKind, GradientSpread, Paint};
use peniko::{Color, Fill};
use std::sync::Arc;
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};
use wgpu::{Device, Instance, Queue, TextureFormat, TextureView};

pub use adapter::{GpuPreference, Selection, SelectionError};
pub use compositor::Compositor;
pub use warm::GpuWarmer;
// Re-export the wgpu that vello uses, so downstream crates cannot accidentally
// link a second, incompatible copy. Vello itself goes with it, for the same
// reason: the exporter builds scenes and must build *these* scenes.
pub use vello;
pub use vello::wgpu;
// The patched crate `GpuContext::new` sets the buffer scale on (P1 in
// `docs/IMPORT_PERF_PLAN.md`) — re-exported so a caller reporting the scale
// (`examples/stage_cost.rs`) does not need its own dependency on it.
pub use vello_encoding;

/// Everything needed to rasterise a scene on the GPU.
pub struct GpuContext {
    pub instance: Instance,
    pub device: Device,
    pub queue: Queue,
    pub renderer: Renderer,
    pub selection: Selection,
    /// This context's **live** `vello_encoding` buffer scale — what
    /// [`Self::render`] and [`Self::render_measured`] actually set on their
    /// own thread before asking Vello to draw (V2 in
    /// `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`). [`Self::new`] starts this at
    /// `1` — Vello's own upstream buffer sizes — so a freshly opened window
    /// does not reserve [`Self::max_buffer_scale`]'s worth of transient
    /// buffers for a blank stage; [`Self::grow_to`] raises it (towards
    /// [`Self::max_buffer_scale`]) the moment a frame actually needs more,
    /// and [`Self::shrink_to`] gives it back once idle. An offline context
    /// (the exporter, headless renders) calls [`Self::start_generous`] right
    /// after construction instead, since it has no frame budget to grow
    /// into and must never degrade quality. Captured by a caller's own
    /// `DrawCache` (`DrawCache::set_scale`) so the budget it judges frames
    /// against is this context's own, not any other context's.
    pub buffer_scale: u32,
    /// The largest scale in `{8, 4, 2, 1}` this adapter's limits and
    /// [`Self::TRANSIENT_BUDGET_BYTES`] allow — what [`Self::pick_buffer_scale`]
    /// computed in [`Self::new`]. [`Self::buffer_scale`] never grows past
    /// this. See [`DrawCache::set_max_scale`](crate::document::DrawCache::set_max_scale).
    pub max_buffer_scale: u32,
    /// S7: the thread that keeps the GPU's clocks up during playback; made on
    /// the first [`Self::keep_warm`].
    warmer: Option<GpuWarmer>,
}

impl GpuContext {
    /// Bring up the GPU, honouring the user's adapter preference.
    pub async fn new(preference: &GpuPreference) -> Result<Self> {
        let instance = Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let selection = adapter::select(&instance, preference)
            .await
            .context("selecting a GPU adapter")?;

        tracing::info!("\n{}", selection.report());

        // P1 (`docs/IMPORT_PERF_PLAN.md`): the biggest Vello transient buffer
        // scale this adapter and a fixed memory budget allow — the ceiling
        // [`Self::grow_to`] may raise the *live* `buffer_scale` towards, not
        // the starting value any more. V2 in
        // `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`: reserving this much up front
        // for a blank stage was the ~1.3 GB this fixed. See
        // [`Self::start_generous`] for a context (the exporter, a headless
        // render) that should start here instead of at `1`.
        let adapter_limits = selection.adapter.limits();
        let max_buffer_scale = Self::pick_buffer_scale(&adapter_limits);
        // Vello's own upstream buffer sizes. A frame that actually needs more
        // grows into `max_buffer_scale` on demand — see
        // `DrawCache::reconsider`.
        let buffer_scale = 1;
        tracing::info!(
            max_buffer_scale,
            starting_scale = buffer_scale,
            max_transient_mb = Self::transient_buffer_bytes(max_buffer_scale).1 / (1024 * 1024),
            starting_transient_mb = Self::transient_buffer_bytes(buffer_scale).1 / (1024 * 1024),
            "Vello transient buffer scale (V2)"
        );

        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
            max_buffer_size: adapter_limits.max_buffer_size,
            ..wgpu::Limits::default()
        };

        let (device, queue) = selection
            .adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("buzz-device"),
                // Vello's compute pipelines run within downlevel defaults; not
                // demanding more keeps older drivers usable. The two storage/
                // buffer size limits above are raised to the adapter's own
                // maximum so the scaled-up transient buffers above can bind.
                required_features: wgpu::Features::empty(),
                required_limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .context("requesting a GPU device")?;

        let renderer = Self::build_renderer(&device)?;

        Ok(Self {
            instance,
            device,
            queue,
            renderer,
            selection,
            buffer_scale,
            max_buffer_scale,
            warmer: None,
        })
    }

    /// **Call while frames are being played:** keeps the GPU out of its idle
    /// power states for the next [`warm::KEEP_WARM_FOR`] (see [`warm`]), so the
    /// first heavy frame after a run of light ones is not drawn at idle clocks.
    /// No effect under `BUZZ_NO_GPU_WARM=1`. Cheap to call every frame.
    pub fn keep_warm(&mut self) {
        if !warm::enabled() {
            return;
        }
        let (device, queue) = (&self.device, &self.queue);
        self.warmer
            .get_or_insert_with(|| GpuWarmer::new(device, queue))
            .touch();
    }

    /// The warmer, once [`Self::keep_warm`] has made it.
    pub fn warmer(&self) -> Option<&GpuWarmer> {
        self.warmer.as_ref()
    }

    /// Grow the live [`Self::buffer_scale`] to at least `at_least`, clamped
    /// to [`Self::max_buffer_scale`] — never a quality loss, only a bigger
    /// allocation, so there is no gesture or playback to avoid this during.
    /// A no-op if the live scale is already there or higher. See V3 in
    /// `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md` and
    /// `DrawCache::reconsider`, which decides when to call this.
    pub fn grow_to(&mut self, at_least: u32) {
        self.buffer_scale = self.buffer_scale.max(at_least.min(self.max_buffer_scale).max(1));
    }

    /// Start this context at [`Self::max_buffer_scale`] rather than `1` — for
    /// an offline render (the exporter, a headless render, a test standing in
    /// for either) that has no frame budget to grow into and must never
    /// degrade quality. See V2/V5 in `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`.
    pub fn start_generous(&mut self) {
        self.buffer_scale = self.max_buffer_scale;
    }

    /// Halve the live [`Self::buffer_scale`] to `scale` (clamped to
    /// `[1, max_buffer_scale]`) and **rebuild the renderer**, so Vello's own
    /// pooled buffers — kept around for reuse rather than freed — are
    /// actually dropped instead of merely going unused. See V4 in
    /// `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`; `DrawCache::pending_shrink`
    /// decides when to call this, gated on there being no gesture or
    /// playback under way, since rebuilding costs a hitch. Leaves
    /// `self.buffer_scale` and `self.renderer` untouched if the rebuild
    /// itself fails, so a failed shrink cannot leave the context without a
    /// renderer.
    pub fn shrink_to(&mut self, scale: u32) -> Result<()> {
        let clamped = scale.max(1).min(self.max_buffer_scale);
        let fresh = Self::build_renderer(&self.device)?;
        self.renderer = fresh;
        self.buffer_scale = clamped;
        Ok(())
    }

    /// The largest scale in `{8, 4, 2, 1}` whose transient buffers both fit
    /// `limits` (`max_storage_buffer_binding_size` and `max_buffer_size`, the
    /// two an individual buffer is bound against) and stay within
    /// [`Self::TRANSIENT_BUDGET_BYTES`] in total. `1` — Vello's own upstream
    /// sizes — always qualifies, so this never fails to return.
    fn pick_buffer_scale(limits: &wgpu::Limits) -> u32 {
        let max_binding = u64::from(limits.max_storage_buffer_binding_size);
        let max_buffer = limits.max_buffer_size;
        for &scale in &Self::BUFFER_SCALE_CANDIDATES {
            let (largest, total) = Self::transient_buffer_bytes(scale);
            if largest <= max_binding && largest <= max_buffer && total <= Self::TRANSIENT_BUDGET_BYTES {
                return scale;
            }
        }
        1
    }

    /// Scales tried by [`Self::pick_buffer_scale`], largest first.
    const BUFFER_SCALE_CANDIDATES: [u32; 4] = [8, 4, 2, 1];

    /// Transient GPU memory budget for the scaled Vello buffers — an RTX
    /// 5060 Ti with 16 GB can spare this much so most frames are a single
    /// pass; see P1 in `docs/IMPORT_PERF_PLAN.md`.
    const TRANSIENT_BUDGET_BYTES: u64 = 1_500 * 1024 * 1024;

    /// `(largest single buffer, total)` in bytes for `vello_encoding`'s six
    /// bump-allocated buffers (`lines`, `segments`, `seg_counts`, `tiles`,
    /// `ptcl`, `bin_data`) at `scale`, computed from `vello_encoding`'s own
    /// element types and [`vello_encoding::BufferSize`] so this never drifts
    /// from `BufferSizes::new` in `third_party/vello_encoding`.
    fn transient_buffer_bytes(scale: u32) -> (u64, u64) {
        use vello_encoding::{BufferSize, LineSoup, PathSegment, SegmentCount, Tile};

        let lines = u64::from(
            BufferSize::<LineSoup>::new((1u32 << 21).saturating_mul(scale)).size_in_bytes(),
        );
        let segments = u64::from(
            BufferSize::<PathSegment>::new((1u32 << 21).saturating_mul(scale)).size_in_bytes(),
        );
        let seg_counts = u64::from(
            BufferSize::<SegmentCount>::new((1u32 << 21).saturating_mul(scale)).size_in_bytes(),
        );
        let tiles =
            u64::from(BufferSize::<Tile>::new((1u32 << 21).saturating_mul(scale)).size_in_bytes());
        let bin_data =
            u64::from(BufferSize::<u32>::new((1u32 << 18).saturating_mul(scale)).size_in_bytes());
        let ptcl =
            u64::from(BufferSize::<u32>::new((1u32 << 23).saturating_mul(scale)).size_in_bytes());

        let largest = [lines, segments, seg_counts, tiles, bin_data, ptcl]
            .into_iter()
            .max()
            .expect("six literal buffers");
        let total = lines + segments + seg_counts + tiles + bin_data + ptcl;
        (largest, total)
    }

    /// Blocking convenience wrapper for non-async callers.
    pub fn new_blocking(preference: &GpuPreference) -> Result<Self> {
        pollster::block_on(Self::new(preference))
    }

    /// The one place `Renderer::new` is called, so [`Self::new`] and the
    /// panic-recovery path in [`Self::render_measured`] (`gpu-truth` only)
    /// build it identically.
    fn build_renderer(device: &Device) -> Result<Renderer> {
        Renderer::new(
            device,
            RendererOptions {
                use_cpu: false,
                // Area anti-aliasing only: it is the fastest mode and is exact
                // for the non-self-intersecting fills that dominate vector
                // artwork. Compiling the MSAA permutations too would slow
                // startup for pipelines we never dispatch.
                antialiasing_support: experiment_aa_support(), // S12b experiment
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .map_err(|e| anyhow::anyhow!("creating the Vello renderer: {e:?}"))
    }

    /// Name of the GPU actually in use, for the HUD.
    pub fn adapter_summary(&self) -> String {
        self.selection.summary()
    }

    /// Rasterise `scene` into `target`.
    pub fn render(
        &mut self,
        scene: &Scene,
        target: &TextureView,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<()> {
        // V2 in `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`: this context's own live
        // scale, set on the calling thread right before Vello lays out the
        // `RenderConfig` for this call — see `vello_encoding::BUFFER_SCALE`.
        vello_encoding::set_scale_for_this_thread(self.buffer_scale);
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                target,
                &RenderParams {
                    base_color,
                    width: width.max(1),
                    height: height.max(1),
                    antialiasing_method: experiment_aa(), // S12b experiment
                },
            )
            .map_err(|e| anyhow::anyhow!("vello render failed: {e:?}"))
    }

    /// Rasterise `scene` into `target`, and read back Vello's own GPU counters
    /// for what it actually used — the ground truth
    /// [`crate::document::segment_ceiling`] and [`SceneBuilder::estimated_lines`]
    /// are both proxies for. Opt-in (`gpu-truth`) because it pays for a full
    /// GPU sync every call: see Phase 4b in
    /// `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
    #[cfg(feature = "gpu-truth")]
    pub fn render_measured(
        &mut self,
        scene: &Scene,
        target: &TextureView,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<GpuTruth> {
        // **An empty scene is measured without asking Vello at all.** Its
        // debug-layers download slices the line buffer by the true count
        // (`lib.rs:803`, see the panic this function already catches below),
        // and slicing an *empty* range there is a second, different panic —
        // "buffer slices can not be empty" — that has nothing to do with
        // overflow. Reporting that as `overflowed` would be wrong in the
        // opposite direction the flag exists to catch, so a scene with
        // nothing encoded is answered directly.
        if scene.encoding().n_path_segments == 0 {
            return Ok(GpuTruth::default());
        }

        // V2 in `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`: see `Self::render`.
        // `render_to_texture_async` below runs synchronously on this thread
        // inside `pollster::block_on`; only the device-polling thread spawned
        // below is a second thread, and it never lays out a `RenderConfig`.
        vello_encoding::set_scale_for_this_thread(self.buffer_scale);

        let params = RenderParams {
            base_color,
            width: width.max(1),
            height: height.max(1),
            antialiasing_method: experiment_aa(), // S12b experiment
        };

        // Vello's async path only resolves its buffer maps when *something*
        // polls the device; nothing else on this call does, so without a
        // thread doing it here `receiver.receive().await` inside Vello hangs
        // forever. Stopped the moment the render below returns, whichever way
        // it returns.
        let poll_device = self.device.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let poller_stop = std::sync::Arc::clone(&stop);
        let poller = std::thread::spawn(move || {
            while !poller_stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = poll_device.poll(wgpu::PollType::Poll);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });

        let renderer = &mut self.renderer;
        let device = &self.device;
        let queue = &self.queue;
        // **Overflow panics inside Vello**, not errors: its own debug-layers
        // download slices the line buffer by the *true* count the GPU wrote
        // (`vello`'s `lib.rs:803`), and when that count is larger than the
        // fixed-size buffer `vello_encoding::BufferSizes` allocated, slicing
        // it panics with "slice offset 0 size N is out of range for buffer of
        // size ...". That overflow is exactly the answer this function exists
        // to report, so it is caught here rather than allowed to take the
        // caller down with it.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[allow(deprecated)]
            pollster::block_on(renderer.render_to_texture_async(
                device,
                queue,
                scene,
                target,
                &params,
                vello::low_level::DebugLayers::none(),
            ))
        }));

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = poller.join();

        match outcome {
            Ok(Ok(Some(bump))) => Ok(GpuTruth {
                lines: u64::from(bump.lines),
                segments: u64::from(bump.segments),
                ptcl: u64::from(bump.ptcl),
                tile: u64::from(bump.tile),
                failed: bump.failed != 0,
                overflowed: false,
            }),
            Ok(Ok(None)) => Ok(GpuTruth::default()),
            Ok(Err(e)) => Err(anyhow::anyhow!("vello measured render failed: {e:?}")),
            Err(panic) => {
                // The panic happened mid-recording, inside state this
                // `Renderer` does not promise is still valid — so it is
                // rebuilt rather than reused. The device and queue are
                // untouched; only the renderer's own pipelines and buffers
                // are replaced.
                if let Ok(fresh) = Self::build_renderer(&self.device) {
                    self.renderer = fresh;
                }
                Ok(GpuTruth {
                    lines: parse_overflow_line_count(&panic).unwrap_or(0),
                    overflowed: true,
                    ..GpuTruth::default()
                })
            }
        }
    }
}

/// **How much of a frame Vello's rasteriser actually used**, read back
/// through its debug-layers async path. Every field is `0`/`false` when
/// [`GpuContext::render_measured`]'s own catch of Vello's overflow panic
/// fired before any of them could be read; `overflowed` is then `true`. See
/// Phase 4b in `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
#[cfg(feature = "gpu-truth")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuTruth {
    pub lines: u64,
    pub segments: u64,
    pub ptcl: u64,
    pub tile: u64,
    /// Vello's own `BumpAllocators::failed`: a GPU-detected overflow of one
    /// of its dynamic-size buffers, reported without incident. Distinct from
    /// `overflowed`, which is the *download itself* failing before this could
    /// even be read.
    pub failed: bool,
    /// The debug download panicked (see [`GpuContext::render_measured`])
    /// before any of the counters above could be read. `lines` is parsed from
    /// the panic message where possible; the rest stay `0`.
    pub overflowed: bool,
}

/// Recover a requested-but-refused download size from the panic
/// `wgpu::BufferSlice::slice` raises when the size Vello asks for exceeds the
/// buffer's real one — "slice offset 0 size N is out of range for buffer of
/// size ..." — and turn it back into a line count. `N` is a byte count of
/// `vello_encoding::LineSoup` records, 24 bytes each (a `u32` path index, a
/// `u32` of padding, and two `[f32; 2]` endpoints).
#[cfg(feature = "gpu-truth")]
fn parse_overflow_line_count(panic: &(dyn std::any::Any + Send)) -> Option<u64> {
    const LINE_SOUP_BYTES: u64 = 24;
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())?;
    let after_size = message.split_once("size ")?.1;
    let digits: String = after_size.chars().take_while(char::is_ascii_digit).collect();
    let bytes: u64 = digits.parse().ok()?;
    Some(bytes / LINE_SOUP_BYTES)
}

/// Vello wants an `AaSupport`; this names the single configuration we compile.
/// S12b experiment: `BUZZ_AA` = `area` (default) | `msaa16` | `msaa8`, read
/// once. Anything else (or unset) is Area AA, exactly as before.
fn experiment_aa() -> AaConfig {
    static AA: std::sync::OnceLock<AaConfig> = std::sync::OnceLock::new();
    *AA.get_or_init(|| match std::env::var("BUZZ_AA").as_deref() {
        Ok("msaa16") => AaConfig::Msaa16,
        Ok("msaa8") => AaConfig::Msaa8,
        _ => AaConfig::Area,
    })
}

/// S12b experiment: the support flags for [`experiment_aa`]; Area only by default.
fn experiment_aa_support() -> vello::AaSupport {
    match experiment_aa() {
        AaConfig::Msaa16 => vello::AaSupport { area: false, msaa8: false, msaa16: true },
        AaConfig::Msaa8 => vello::AaSupport { area: false, msaa8: true, msaa16: false },
        _ => AaSupportArea::SUPPORT,
    }
}

/// S12c experiment: the pen used for seam-seal strokes. `BUZZ_SEAL_JOIN` =
/// `round` (default: `Stroke::new`, exactly as before) | `miter` (miter limit
/// 4, butt caps) | `bevel` (butt caps), read once. Seal strokes only; every
/// other stroke keeps `Stroke::new`. Anything else (or unset) is `round`.
fn seal_stroke(width: f64) -> kurbo::Stroke {
    #[derive(Clone, Copy, PartialEq)]
    enum SealJoin {
        Round,
        Miter,
        Bevel,
    }
    static JOIN: std::sync::OnceLock<SealJoin> = std::sync::OnceLock::new();
    let join = *JOIN.get_or_init(|| match std::env::var("BUZZ_SEAL_JOIN").as_deref() {
        Ok("miter") => SealJoin::Miter,
        Ok("bevel") => SealJoin::Bevel,
        _ => SealJoin::Round,
    });
    match join {
        SealJoin::Round => kurbo::Stroke::new(width),
        SealJoin::Miter => kurbo::Stroke::new(width)
            .with_join(kurbo::Join::Miter)
            .with_miter_limit(4.0)
            .with_caps(kurbo::Cap::Butt),
        SealJoin::Bevel => kurbo::Stroke::new(width)
            .with_join(kurbo::Join::Bevel)
            .with_caps(kurbo::Cap::Butt),
    }
}

struct AaSupportArea;
impl AaSupportArea {
    const SUPPORT: vello::AaSupport = vello::AaSupport {
        area: true,
        msaa8: false,
        msaa16: false,
    };
}

/// Format Vello renders into before blitting to the surface.
///
/// Vello's fine rasteriser writes through a storage texture, which surface
/// textures generally do not permit, so rendering goes to an owned texture and
/// is then blitted. This is the pattern Vello's own documentation recommends.
pub const RENDER_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// Builds Vello scenes while respecting the rebasing contract.
///
/// # The contract
///
/// Geometry is rebased into anchor-relative space **in `f64` on the CPU**, and
/// only then handed to Vello along with the small-magnitude view transform.
/// Passing document-space geometry with a fused transform would reintroduce the
/// How Animate's blend list maps onto Vello's.
///
/// Nine of the ten are one of Vello's mixing modes; Add is a *compositing*
/// operator rather than a mixing one — `Plus` sums the two, which is what
/// Animate's Add does. Layer means "composite as a group first", which is the
/// group itself, so it needs no equation at all.
fn blend_mode(blend: buzz_fx::Blend) -> peniko::BlendMode {
    use buzz_fx::Blend;
    let (mix, compose) = match blend {
        Blend::Normal | Blend::Layer => (peniko::Mix::Normal, peniko::Compose::SrcOver),
        Blend::Darken => (peniko::Mix::Darken, peniko::Compose::SrcOver),
        Blend::Multiply => (peniko::Mix::Multiply, peniko::Compose::SrcOver),
        Blend::Lighten => (peniko::Mix::Lighten, peniko::Compose::SrcOver),
        Blend::Screen => (peniko::Mix::Screen, peniko::Compose::SrcOver),
        Blend::Overlay => (peniko::Mix::Overlay, peniko::Compose::SrcOver),
        Blend::HardLight => (peniko::Mix::HardLight, peniko::Compose::SrcOver),
        Blend::Difference => (peniko::Mix::Difference, peniko::Compose::SrcOver),
        Blend::Add => (peniko::Mix::Normal, peniko::Compose::Plus),
    };
    peniko::BlendMode::new(mix, compose)
}

/// How far a filled shape is grown to close the seam against its neighbour,
/// in **screen pixels**. See [`SceneBuilder::fill_shape_paint_sealed`].
///
/// A stroke is centred on the path, so this width pushes the edge out by half
/// of itself; two neighbours therefore overlap by the whole of it. Just under a
/// pixel of overlap is enough to bring a shared boundary to full coverage, and
/// going wider only disturbs more of the silhouette.
pub(crate) const SEAM_SEAL_PX: f64 = 0.9;

/// The largest share of a shape's own size a seam seal (or a mask's growth,
/// which is the same idea) may take. See [`seal_width_px`]. A quarter: any
/// shape at least 3.6 px thick or 14.4 px long — a 200 px shape by a wide
/// margin — is sealed exactly as before.
const SEAL_MAX_SHARE: f64 = 0.25;

/// How much a shape's *length* counts towards its size, against its
/// thickness. See [`seal_width_px`].
const SEAL_EXTENT_SHARE: f64 = 0.25;

/// Below this many device pixels a seal is not drawn at all: it closes
/// nothing a viewer could see, and a stroke that thin is not rasterised in
/// proportion to its width (Vello draws a near-zero-width stroke far heavier
/// than it is — measured: a one-pixel square sealed with a `1e-300`-wide
/// stroke came out covering 2.4 pixels).
const SEAL_MIN_PX: f64 = 0.05;

/// Below this mean thickness (`2·|area| / perimeter`, in device pixels) a subpath
/// of a fill is not sealed: it encloses nothing. See [`sliver_subpaths`].
const SEAL_SLIVER_PX: f64 = 1e-3;

/// **How wide to draw a fill's seam seal**, in device pixels, for a shape
/// `thickness_px` thick and `extent_px` long on screen (see
/// `PathLineStats::thickness` and `PathLineStats::extent`) — or `None` for no
/// seal at all.
///
/// # The defect this exists for
///
/// The seal grows a shape by half its width on every side, which is nothing
/// on a shape a hundred pixels across and *everything* on one three pixels
/// across. barish1's face on the stage puts an iris 3.75 px wide on screen:
/// the full 0.9 px seal grew its radius by a quarter and its area by half,
/// the black pupil and outline grew with it and swallowed the brown, the
/// eye's thin dark rim tripled in width over the sclera, and the teeth — a
/// white fill 2.6 px tall — were drowned by the lip fills sealed around
/// them. Animate scan-converts a shape's regions together and grows nothing,
/// so its eyes stay brown and white at that size.
///
/// Growing the edge by `g` adds about `g / thickness` of the shape's area, so
/// capping the seal at [`SEAL_MAX_SHARE`] of the thickness caps the darkening
/// (or lightening) of a small compact shape at about half that share.
///
/// **A long thin shape keeps its seal, though.** Since seals are drawn
/// beneath their run's artwork ([`SceneBuilder::begin_seal_run`]) a seal no
/// longer eats the shapes drawn before it; inside the run it only fills the
/// pixels its neighbours leave short, and there a line half a pixel wide
/// *needs* its whole seal, or the region drawn over it takes its share of
/// every pixel along it (measured in `small_artwork.rs`: a half-pixel line
/// came out at 12 % coverage with the thickness cap alone, 35 % — its true
/// coverage — with it lifted). So a shape's size is the larger of its
/// thickness and a quarter of its length: an iris 3.75 px across is capped,
/// a nose line 15 px long is not.
///
/// Measured on barish1's body at frame 374 at stage size against Animate's
/// own export, mean |Δluma| over the face: 11.8 before this change, 9.1 after.
/// **The seal for a shape measured lazily**: `extent_px` is known for free
/// (the caller's bounds); `thickness_px` is asked for only when the
/// extent alone does not already earn the full seal — a shape under
/// [`SEAL_FULL_EXTENT_PX`] long, which at an ordinary zoom is nearly none of
/// them. Shared by fills and mask growth so the two stay in step.
pub(crate) fn seal_width_px_lazy(extent_px: f64, thickness_px: impl FnOnce() -> f64) -> Option<f64> {
    // S12b experiment: MSAA has no conflation seams, so no seal and no seal-driven
    // mask growth (document.rs `grow_mask_path` also goes through here).
    if experiment_aa() != AaConfig::Area {
        return None;
    }
    if extent_px >= SEAL_FULL_EXTENT_PX {
        return Some(SEAM_SEAL_PX);
    }
    seal_width_px(thickness_px(), extent_px)
}

/// The length from which a shape takes the whole seal on its extent alone:
/// `SEAL_MAX_SHARE × SEAL_EXTENT_SHARE × SEAL_FULL_EXTENT_PX = SEAM_SEAL_PX`.
const SEAL_FULL_EXTENT_PX: f64 = SEAM_SEAL_PX / (SEAL_MAX_SHARE * SEAL_EXTENT_SHARE);

pub(crate) fn seal_width_px(thickness_px: f64, extent_px: f64) -> Option<f64> {
    let size = thickness_px
        .max(0.0)
        .max(SEAL_EXTENT_SHARE * extent_px.max(0.0));
    let width = SEAM_SEAL_PX.min(SEAL_MAX_SHARE * size);
    (width >= SEAL_MIN_PX).then_some(width)
}

/// Is this paint fully opaque everywhere?
///
/// A gradient has to be checked stop by stop: one transparent stop is enough to
/// make sealing it draw a dark rim, and the average would hide that.
pub(crate) fn is_opaque(paint: &Paint) -> bool {
    match paint {
        Paint::Solid(c) => c.components[3] >= 1.0,
        Paint::Gradient(g) => g.stops().iter().all(|s| s.color.components[3] >= 1.0),
        // **Never sealed.** A photograph is opaque in the middle and its
        // interesting edges are where it is not; a cut-out is transparent over
        // most of its own rectangle. Growing either by half a pixel would
        // smear the border pixels outwards, which is exactly the fringe that
        // makes a composite look pasted on.
        Paint::Image(_) => false,
    }
}

/// probe only (S12): per-call line-charge records for `frame_cost --line-sources`
/// (`docs/BIMARI5_LAG_PLAN.md` S12). Gated by the `BUZZ_PROBE_LINE_SOURCES`
/// env var, read once; unset (the default) every hook below is one atomic
/// load and nothing else, and no production behaviour changes.
pub mod probe {
    use std::cell::{Cell, RefCell};
    use std::sync::OnceLock;

    /// What a charged path was for.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Kind {
        /// An ordinary fill (a lit / additive / atop fill charges its path twice).
        Fill,
        /// A clip or group layer (`push_clip`, `push_blend`, `push_isolation`, ...).
        Clip,
        /// An ordinary stroke.
        Stroke,
        /// A seam-seal stroke (`stroke_shape_paint_into` with `as_seal`).
        Seal,
    }

    /// One `add_fill_lines` / `add_stroke_lines` charge. `lines` is the raw
    /// charge, before `LINE_ESTIMATE_MARGIN`.
    #[derive(Clone, Copy, Debug)]
    pub struct Rec {
        pub kind: Kind,
        pub lines: u64,
        /// Device-pixel bounding box `[x0, y0, x1, y1]` of the flattened path.
        pub bbox: [f64; 4],
        /// Stroke width in device px (0 for fills / clips).
        pub stroke_px: f64,
    }

    /// Is the probe on? (`BUZZ_PROBE_LINE_SOURCES` set.)
    pub fn enabled() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("BUZZ_PROBE_LINE_SOURCES").is_some())
    }

    thread_local! {
        static RECS: RefCell<Vec<Rec>> = const { RefCell::new(Vec::new()) };
        static TAG: Cell<Option<Kind>> = const { Cell::new(None) };
        static BBOX: Cell<[f64; 4]> = const { Cell::new([0.0; 4]) };
        static STAGED: Cell<[u64; 6]> = const { Cell::new([0; 6]) };
        static MAKEUP: Cell<[u64; 7]> = const { Cell::new([0; 7]) };
    }

    fn count_segments(path: &kurbo::BezPath) -> [u64; 3] {
        let mut c = [0u64; 3];
        for el in path.elements() {
            match el {
                kurbo::PathEl::LineTo(_) | kurbo::PathEl::ClosePath => c[0] += 1,
                kurbo::PathEl::QuadTo(..) => c[1] += 1,
                kurbo::PathEl::CurveTo(..) => c[2] += 1,
                kurbo::PathEl::MoveTo(_) => {}
            }
        }
        c
    }

    /// probe only (S12c): the line/quad/cubic counts of a path before and
    /// after `clip.apply`, held until the caller knows it was a seal.
    pub(crate) fn stage_makeup(pre: &kurbo::BezPath, post: &kurbo::BezPath) {
        let (a, b) = (count_segments(pre), count_segments(post));
        STAGED.with(|s| s.set([a[0], a[1], a[2], b[0], b[1], b[2]]));
    }

    /// probe only (S12c): the staged path was a seal; add it to the tally
    /// `[paths, pre l/q/c, post l/q/c]`.
    pub(crate) fn note_seal_makeup() {
        let s = STAGED.with(|s| s.get());
        MAKEUP.with(|m| {
            let mut v = m.get();
            v[0] += 1;
            for i in 0..6 {
                v[1 + i] += s[i];
            }
            m.set(v);
        });
    }

    /// probe only (S12c): the seal makeup tally `[paths, pre l/q/c, post
    /// l/q/c]` since the last `SceneBuilder::new` on this thread.
    pub fn take_seal_makeup() -> [u64; 7] {
        MAKEUP.with(|m| m.replace([0; 7]))
    }

    /// Records since the last `SceneBuilder::new` on this thread.
    pub fn take() -> Vec<Rec> {
        RECS.with(|r| std::mem::take(&mut *r.borrow_mut()))
    }

    pub(crate) fn reset() {
        RECS.with(|r| r.borrow_mut().clear());
        TAG.with(|t| t.set(None));
        MAKEUP.with(|m| m.set([0; 7]));
    }

    pub(crate) fn set_bbox(b: [f64; 4]) {
        TAG.with(|t| t.set(None));
        BBOX.with(|c| c.set(b));
    }

    pub(crate) fn tag(k: Kind) {
        TAG.with(|t| t.set(Some(k)));
    }

    pub(crate) fn push(default: Kind, lines: u64, stroke_px: f64) {
        let kind = TAG.with(|t| t.get()).unwrap_or(default);
        let bbox = BBOX.with(|c| c.get());
        RECS.with(|r| {
            r.borrow_mut().push(Rec {
                kind,
                lines,
                bbox,
                stroke_px,
            })
        });
    }
}

/// Vello's own flattening tolerance, in device pixels — mirrored here so
/// [`SceneBuilder::estimated_lines`] lands on the same line counts
/// `vello_shaders`' `flatten.wgsl` produces. Drifting from this constant
/// would silently invalidate the calibration `import_budget --features
/// gpu-truth` checks against Vello's own counters. See Phase 4a in
/// `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
const FLATTEN_TOLERANCE_PX: f64 = 0.25;

/// A fitted safety margin over the raw line estimate, applied once in
/// [`SceneBuilder::estimated_lines`] rather than into every join or fill
/// term, so one constant covers whatever the model still does not capture
/// exactly.
///
/// Fitted against `import_budget --features gpu-truth` on three real `.fla`
/// imports, after `docs/RENDER_SEAMS_PLAN.md` Step 3 taught joins the angle
/// they actually turn (each still charged at least 2 lines — one per offset
/// side — never the worst-case `π` every join used to cost). The raw
/// (unmargined) ratios measured, `est / actual`:
///
/// | file | frame | cull | est | actual | raw ratio |
/// |---|---|---|---|---|---|
/// | bhatta2.fla | 0 | stage | 2 031 958 | 1 822 659 | 1.11 |
/// | sustipur1.fla | 0 | no cull | 50 646 | 46 304 | 1.09 |
/// | sustipur1.fla | 0 | stage | 39 359 | 35 749 | 1.10 |
/// | sustipur1.fla | 300 | no cull | 601 498 | 575 601 | **1.04** |
/// | sustipur1.fla | 300 | stage | 471 388 | 451 042 | 1.05 |
/// | sustipur1.fla | 1000 | no cull | 595 066 | 569 354 | 1.05 |
/// | road_traffic.fla | 0 | stage | 42 789 | 39 442 | 1.08 |
///
/// (Every frame not listed either overflowed Vello's `gpu-truth` debug
/// buffer before it could report an actual count — a frame already far over
/// budget on its own, orthogonal to this margin — or had no content at that
/// frame at all.) The worst raw ratio was `1.04`; `1.02` lifts every sample
/// safely past the `1.05` floor this project wants between the estimate and
/// Vello's own count, with headroom to spare — re-measured with the margin
/// applied, the worst sample (`sustipur1.fla`, frames 300 and 1000) came
/// back at `1.07`.
const LINE_ESTIMATE_MARGIN: f64 = 1.02;

/// Elevate a quadratic Bézier's control point to the cubic with the same
/// curve, so [`wang_cubic_lines`] has one formula to apply to either.
fn quad_to_cubic(p0: Point, p1: Point, p2: Point) -> (Point, Point, Point, Point) {
    let c1 = p0 + (p1 - p0) * (2.0 / 3.0);
    let c2 = p2 + (p1 - p2) * (2.0 / 3.0);
    (p0, c1, c2, p2)
}

/// Wang's bound for a cubic Bézier: the number of lines it takes to flatten
/// the curve to within [`FLATTEN_TOLERANCE_PX`], **always at least as many**
/// as Vello's own Euler-spiral flattener needs for the same tolerance.
/// `device_scale` carries the control points — still in render space — into
/// the device pixels Vello actually flattens in; multiplying the
/// *measurement* rather than transforming the whole path again is what keeps
/// this cheap, since `M` is a vector norm of point differences and so scales
/// linearly with a uniform scale of the points themselves. Capped at 100 per
/// segment, as Vello's own flattener is.
fn wang_cubic_lines(p0: Point, p1: Point, p2: Point, p3: Point, device_scale: f64) -> u64 {
    let m1 = (p0.to_vec2() - p1.to_vec2() * 2.0 + p2.to_vec2()).hypot();
    let m2 = (p1.to_vec2() - p2.to_vec2() * 2.0 + p3.to_vec2()).hypot();
    let m = m1.max(m2) * device_scale;
    let n = (0.75 * m / FLATTEN_TOLERANCE_PX).sqrt().ceil();
    (n.max(1.0) as u64).min(100)
}

/// The angle a single flattened line can span on a round join or cap's arc,
/// for a stroke `width_px` device pixels wide, without the arc bulging past
/// [`FLATTEN_TOLERANCE_PX`] of the true curve. `≤ 0.0` only when the geometry
/// is degenerate enough that no angle clears the tolerance at all.
fn round_join_step(width_px: f64) -> f64 {
    let radius = (width_px / 2.0).max(FLATTEN_TOLERANCE_PX);
    let inner = (1.0 - FLATTEN_TOLERANCE_PX / radius).clamp(-1.0, 1.0);
    2.0 * inner.acos()
}

/// Vello's round join or cap, turned through `turn` radians, as a
/// device-pixel arc turned into line segments — at least **two**, however
/// small the turn, because a join is stroked on both offset sides of the
/// centreline (the same reason [`stroke_line_estimate`] doubles `stats.fill`)
/// and each side emits its own line even for a turn too small to need a
/// second one for the arc alone. `width_px` is the stroke's width, already
/// in device pixels.
fn round_arc_lines_for_turn(width_px: f64, turn: f64) -> u64 {
    let step = round_join_step(width_px);
    if step <= 0.0 {
        return 2;
    }
    (turn / step).ceil().max(2.0) as u64
}

/// Vello's round join or cap at its worst turn — a full reversal, `π`
/// radians. What every join used to be charged, before
/// `docs/RENDER_SEAMS_PLAN.md` Step 3 taught a join the angle it actually
/// turns, and what an open-subpath cap is still charged: a cap's own
/// direction is whatever the pen was doing when the subpath ended, not a
/// turn between two drawn segments, so there is nothing to measure it
/// against. `width_px` is the stroke's width, already in device pixels.
fn round_arc_lines(width_px: f64) -> u64 {
    round_arc_lines_for_turn(width_px, std::f64::consts::PI)
}

/// The unsigned angle a round join turns through, from an incoming tangent
/// direction to an outgoing one — `0` continuing straight on, `π` doubling
/// straight back. A degenerate (zero-length) direction either side turns
/// nothing rather than something undefined: a coincident point's own angle
/// is not meaningful, and Vello does not flatten an arc for one either.
fn turn_angle(incoming: Vec2, outgoing: Vec2) -> f64 {
    if incoming == Vec2::ZERO || outgoing == Vec2::ZERO {
        return 0.0;
    }
    let mut delta = outgoing.angle() - incoming.angle();
    if delta > std::f64::consts::PI {
        delta -= std::f64::consts::TAU;
    } else if delta < -std::f64::consts::PI {
        delta += std::f64::consts::TAU;
    }
    delta.abs()
}

/// A cubic's own tangent direction at `p0`: towards the first control point
/// that differs from it, falling back as far as `p3` for a curve with
/// coincident controls. `Vec2::ZERO` only when every point coincides.
fn cubic_start_tangent(p0: Point, p1: Point, p2: Point, p3: Point) -> Vec2 {
    for p in [p1, p2, p3] {
        let d = p - p0;
        if d != Vec2::ZERO {
            return d;
        }
    }
    Vec2::ZERO
}

/// The mirror of [`cubic_start_tangent`], at the curve's end.
fn cubic_end_tangent(p0: Point, p1: Point, p2: Point, p3: Point) -> Vec2 {
    for p in [p2, p1, p0] {
        let d = p3 - p;
        if d != Vec2::ZERO {
            return d;
        }
    }
    Vec2::ZERO
}

/// Charge one join's own round-arc cost, at the angle it actually turns,
/// against `join_lines` — or nothing, when gathered without a known stroke
/// width (a fill or clip call, which never reads `join_lines`).
fn charge_join(stroke_width_px: Option<f64>, incoming: Vec2, outgoing: Vec2, join_lines: &mut u64) {
    if let Some(width) = stroke_width_px {
        *join_lines += round_arc_lines_for_turn(width, turn_angle(incoming, outgoing));
    }
}

/// What one walk over a render-space path gathers for
/// [`SceneBuilder::estimated_lines`]: the path's own fill cost, and — for a
/// stroke built from the same path — the cost of its joins, each at the
/// angle it actually turns (`docs/RENDER_SEAMS_PLAN.md` Step 3), and its
/// open-subpath caps, still worst-case. Gathered in one walk because a shape
/// has to stay cheaper to estimate than to draw, or the estimate defeats the
/// reason it exists.
#[derive(Default, Clone, Copy)]
struct PathLineStats {
    /// Lines needed to flatten and fill the path once: Wang's bound per
    /// curve, one line per straight edge, and one per closing edge. Vello
    /// always closes a subpath to fill it; `PathEl::ClosePath` is where that
    /// line shows up in the walk below, so only an explicitly closed subpath
    /// — which is every shape this renderer draws — counts one.
    fill: u64,
    /// The summed round-arc cost of every join — every vertex of a closed
    /// subpath (it wraps around), the interior corners only of an open one —
    /// each charged at the angle it actually turns rather than the worst
    /// case. `0` when gathered for a fill or clip, which never reads this.
    join_lines: u64,
    /// Open-subpath ends — two per open subpath, zero for a closed one,
    /// whose ends are joins instead.
    caps: u64,
}

/// **How thick a region is**, in the path's own units: `2 × area /
/// perimeter`, holes subtracted.
///
/// For a strip — a brush outline, a ring round an eye — that is its width;
/// for a disc it is the radius. It is the one number that says how much of a
/// shape a fixed-width rim is: growing the edge by `g` adds about
/// `g / thickness` of the area, whatever the outline's shape. `0.0` for a path
/// with no length.
///
/// A walk of its own, and only ever asked of a shape a few device pixels
/// long — see [`seal_width_px`] — so it costs nothing on artwork drawn at an
/// ordinary size. The area is Green's theorem per segment, exact for lines and
/// Béziers, every subpath closed as a fill closes it; the length is each
/// segment's chord and control polygon averaged, within a few percent for any
/// curve that is not a cusp.
fn path_thickness(path: &BezPath) -> f64 {
    use kurbo::ParamCurveArea as _;
    let mut area = 0.0f64;
    let mut length = 0.0f64;
    let mut start = Point::ORIGIN;
    let mut current = Point::ORIGIN;
    let close = |current: Point, start: Point, area: &mut f64, length: &mut f64| {
        if current != start {
            *area += kurbo::Line::new(current, start).signed_area();
            *length += (start - current).hypot();
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                close(current, start, &mut area, &mut length);
                start = p;
                current = p;
            }
            PathEl::LineTo(p) => {
                area += kurbo::Line::new(current, p).signed_area();
                length += (p - current).hypot();
                current = p;
            }
            PathEl::QuadTo(p1, p2) => {
                let (c0, c1, c2, c3) = quad_to_cubic(current, p1, p2);
                area += kurbo::CubicBez::new(c0, c1, c2, c3).signed_area();
                length += cubic_length_estimate(c0, c1, c2, c3);
                current = p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                area += kurbo::CubicBez::new(current, p1, p2, p3).signed_area();
                length += cubic_length_estimate(current, p1, p2, p3);
                current = p3;
            }
            PathEl::ClosePath => {
                close(current, start, &mut area, &mut length);
                current = start;
            }
        }
    }
    close(current, start, &mut area, &mut length);
    if length > 0.0 { 2.0 * area.abs() / length } else { 0.0 }
}

/// **The subpaths of a fill that enclose nothing**, by index (`0` is the one the
/// first `MoveTo` starts): those whose mean thickness `2·|area| / perimeter` (as
/// [`path_thickness`] measures it, each subpath closed as a fill closes it) is under
/// `min_thickness`, in the path's own units, or not a number at all. Only a path of
/// two subpaths or more is measured; for anything else, and for ordinary artwork,
/// the answer is empty and nothing is allocated.
///
/// # The defect this exists for
///
/// A boolean result can carry such a subpath beside its real outline: three points a
/// few millionths of a pixel apart, left where two outlines nearly coincided (A3 of
/// docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md). Filled, it paints nothing. But the seal
/// strokes every subpath, and in f32 those three points are one point: the stroke
/// has no direction anywhere on it, and its winding leaked along the tile row, a band
/// of tiles in the fill's colour out to the right edge of the *whole path's* bounds —
/// the 96 × 14 px skin-coloured rectangle under Raju's right hand. A seal closes the
/// seam along an edge, and a subpath that encloses nothing has no edge to close. (A
/// path of one such subpath has bounds a point wide, so the leak has nowhere to go.)
fn sliver_subpaths(elements: &[PathEl], min_thickness: f64) -> Vec<usize> {
    use kurbo::ParamCurveArea as _;
    let mut slivers = Vec::new();
    if elements.iter().filter(|el| matches!(el, PathEl::MoveTo(_))).nth(1).is_none() {
        return slivers;
    }
    // The subpath being walked: its index, area, length, start and current point.
    let mut walk: Option<(usize, f64, f64, Point, Point)> = None;
    let close = |area: &mut f64, length: &mut f64, start: Point, current: &mut Point| {
        if *current != start {
            *area += kurbo::Line::new(*current, start).signed_area();
            *length += (start - *current).hypot();
            *current = start;
        }
    };
    let mut finish = |walk: Option<(usize, f64, f64, Point, Point)>| {
        if let Some((k, mut area, mut length, start, mut current)) = walk {
            close(&mut area, &mut length, start, &mut current);
            if !(length > 0.0 && 2.0 * area.abs() / length >= min_thickness) {
                slivers.push(k);
            }
        }
    };
    let mut count = 0usize;
    for el in elements {
        match *el {
            PathEl::MoveTo(p) => {
                finish(walk.take());
                walk = Some((count, 0.0, 0.0, p, p));
                count += 1;
            }
            PathEl::ClosePath => {
                if let Some((_, area, length, start, current)) = walk.as_mut() {
                    close(area, length, *start, current);
                }
            }
            PathEl::LineTo(p) => {
                if let Some((_, area, length, _, current)) = walk.as_mut() {
                    *area += kurbo::Line::new(*current, p).signed_area();
                    *length += (p - *current).hypot();
                    *current = p;
                }
            }
            PathEl::QuadTo(p1, p2) => {
                if let Some((_, area, length, _, current)) = walk.as_mut() {
                    let (c0, c1, c2, c3) = quad_to_cubic(*current, p1, p2);
                    *area += kurbo::CubicBez::new(c0, c1, c2, c3).signed_area();
                    *length += cubic_length_estimate(c0, c1, c2, c3);
                    *current = p2;
                }
            }
            PathEl::CurveTo(p1, p2, p3) => {
                if let Some((_, area, length, _, current)) = walk.as_mut() {
                    *area += kurbo::CubicBez::new(*current, p1, p2, p3).signed_area();
                    *length += cubic_length_estimate(*current, p1, p2, p3);
                    *current = p3;
                }
            }
        }
    }
    finish(walk);
    slivers
}

/// `elements` without the subpaths numbered in `drop` (ascending, numbered as
/// [`sliver_subpaths`] numbers them).
fn without_subpaths(elements: &[PathEl], drop: &[usize]) -> BezPath {
    let mut out = BezPath::new();
    let mut subpath: Option<usize> = None;
    let mut count = 0usize;
    for el in elements {
        if let PathEl::MoveTo(_) = el {
            subpath = Some(count);
            count += 1;
        }
        if subpath.is_some_and(|k| drop.binary_search(&k).is_ok()) {
            continue;
        }
        out.push(*el);
    }
    out
}

/// Fold a finished subpath into `join_lines`/`caps`: a closed subpath's last
/// join — from its last segment's own direction back round to its first,
/// which the walk below cannot charge until the subpath is known to be
/// closed — or an open one's two caps. Called both when [`PathEl::MoveTo`]
/// starts the next subpath and once more when the path ends.
#[allow(clippy::too_many_arguments)]
fn flush_subpath(
    segments: u64,
    closed: bool,
    first_start_dir: Option<Vec2>,
    last_end_dir: Option<Vec2>,
    stroke_width_px: Option<f64>,
    join_lines: &mut u64,
    caps: &mut u64,
) {
    if segments == 0 {
        return;
    }
    if closed {
        if let (Some(first), Some(last)) = (first_start_dir, last_end_dir) {
            charge_join(stroke_width_px, last, first, join_lines);
        }
    } else {
        *caps += 2;
    }
}

/// One pass over `path`'s elements, in render space, producing the stats
/// [`SceneBuilder::to_render_space`] stashes for whichever fill, stroke or
/// clip call asks for them next. `stroke_width_px` is `Some` only when the
/// caller already knows this path is about to be stroked, at that device
/// width; a fill or clip call passes `None`, and `join_lines` stays `0`
/// since nothing reads it. See [`PathLineStats`].
fn path_line_stats(path: &BezPath, device_scale: f64, stroke_width_px: Option<f64>) -> PathLineStats {
    let mut fill = 0u64;
    let mut join_lines = 0u64;
    let mut caps = 0u64;

    let mut start = Point::ORIGIN;
    let mut current = Point::ORIGIN;
    let mut segments = 0u64;
    let mut closed = false;
    // The current subpath's first tangent direction, kept in case it turns
    // out to be closed and needs a wraparound join at the end; the running
    // last one, charged against the next segment's own start as soon as
    // that segment is seen.
    let mut first_start_dir: Option<Vec2> = None;
    let mut prev_end_dir: Option<Vec2> = None;

    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush_subpath(
                    segments,
                    closed,
                    first_start_dir,
                    prev_end_dir,
                    stroke_width_px,
                    &mut join_lines,
                    &mut caps,
                );
                start = p;
                current = p;
                segments = 0;
                closed = false;
                first_start_dir = None;
                prev_end_dir = None;
            }
            PathEl::LineTo(p) => {
                let dir = p - current;
                match prev_end_dir {
                    Some(prev) => charge_join(stroke_width_px, prev, dir, &mut join_lines),
                    None => first_start_dir = Some(dir),
                }
                prev_end_dir = Some(dir);
                fill += 1;
                segments += 1;
                current = p;
            }
            PathEl::QuadTo(p1, p2) => {
                let (c0, c1, c2, c3) = quad_to_cubic(current, p1, p2);
                let start_dir = cubic_start_tangent(c0, c1, c2, c3);
                let end_dir = cubic_end_tangent(c0, c1, c2, c3);
                match prev_end_dir {
                    Some(prev) => charge_join(stroke_width_px, prev, start_dir, &mut join_lines),
                    None => first_start_dir = Some(start_dir),
                }
                prev_end_dir = Some(end_dir);
                fill += wang_cubic_lines(c0, c1, c2, c3, device_scale);
                segments += 1;
                current = p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                let start_dir = cubic_start_tangent(current, p1, p2, p3);
                let end_dir = cubic_end_tangent(current, p1, p2, p3);
                match prev_end_dir {
                    Some(prev) => charge_join(stroke_width_px, prev, start_dir, &mut join_lines),
                    None => first_start_dir = Some(start_dir),
                }
                prev_end_dir = Some(end_dir);
                fill += wang_cubic_lines(current, p1, p2, p3, device_scale);
                segments += 1;
                current = p3;
            }
            PathEl::ClosePath => {
                let dir = start - current;
                match prev_end_dir {
                    Some(prev) => charge_join(stroke_width_px, prev, dir, &mut join_lines),
                    None => first_start_dir = Some(dir),
                }
                prev_end_dir = Some(dir);
                fill += 1;
                segments += 1;
                closed = true;
                current = start;
            }
        }
    }
    flush_subpath(
        segments,
        closed,
        first_start_dir,
        prev_end_dir,
        stroke_width_px,
        &mut join_lines,
        &mut caps,
    );

    PathLineStats {
        fill,
        join_lines,
        caps,
    }
}

/// A cubic's arc length, as the mean of its chord and its control polygon.
/// The true length lies between the two, and their mean is within a few
/// percent for any curve that is not a cusp — plenty for
/// [`PathLineStats::thickness`], which only has to tell a hairline from a
/// field.
fn cubic_length_estimate(p0: Point, p1: Point, p2: Point, p3: Point) -> f64 {
    let chord = (p3 - p0).hypot();
    let polygon = (p1 - p0).hypot() + (p2 - p1).hypot() + (p3 - p2).hypot();
    0.5 * (chord + polygon)
}

/// The flattened-line estimate for a stroke of `width_px` device pixels,
/// from the centreline's own stats: both offset sides flatten like the fill
/// would (`2 ×`), plus each join's own round-arc cost — already summed at
/// this width by [`path_line_stats`] — and a worst-case round arc at every
/// open-subpath cap.
fn stroke_line_estimate(stats: &PathLineStats, width_px: f64) -> u64 {
    stats.fill * 2 + stats.join_lines + stats.caps * round_arc_lines(width_px)
}

/// The largest factor an affine's linear part can stretch a unit vector by —
/// its greatest singular value. Used to size a stroke's pen conservatively
/// when it is transformed non-uniformly (see [`SceneBuilder::stroke_transformed`]):
/// the largest stretch rather than an average keeps the line estimate from
/// landing under Vello's real count, which is the one direction it must
/// never err in.
fn max_stretch(m: Affine) -> f64 {
    let [a, b, c, d, _, _] = m.as_coeffs();
    let sum_sq = a * a + b * b + c * c + d * d;
    let det = a * d - b * c;
    let disc = (sum_sq * sum_sq - 4.0 * det * det).max(0.0).sqrt();
    ((sum_sq + disc) / 2.0).max(0.0).sqrt()
}

/// precision collapse that caps Animate at 2000%. See `buzz_geom::camera`.
pub struct SceneBuilder<'a> {
    scene: &'a mut Scene,
    split: RenderSplit,
    clip: RenderClip,
    /// The camera's own visible rectangle, **tight** — unlike `clip`, which is
    /// grown by `RenderClip`'s margin so off-screen geometry can still bleed a
    /// stroke or a shadow into view. A cull wants the tight rectangle: it only
    /// needs to know what can be *seen*, and a caller that wants headroom for a
    /// stroke adds its own margin around this rather than inheriting the clip's,
    /// which was sized for a different purpose (encoding stability, not cull
    /// tightness) and is nine times the area at fit zoom.
    visible: buzz_geom::Rect,
    /// Running total of [`Self::estimated_lines`], charged by every fill,
    /// stroke and clip as it is built. See Phase 4a in
    /// `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`: `encoded_segments` alone let a
    /// frame through that Vello's line buffer then dropped, because a segment
    /// costs a different number of lines depending on how curvy and how
    /// stroked the artwork is.
    lines: u64,
    /// `gpu_view`'s own scalar magnification — the piece of the transform
    /// chain [`Self::to_render_space`] does not already apply, because it is
    /// carried on the GPU side of the split (see `RenderSplit`). `gpu_view` is
    /// always a similarity (a rotation, an optional uniform output scale from
    /// [`Self::with_output_scale`], and a translation), so this one number is
    /// exact and is kept up to date there rather than recomputed — a
    /// determinant and a square root — for every shape drawn.
    device_scale: f64,
    /// The stats [`Self::to_render_space`] most recently gathered, read by
    /// whichever fill, stroke or clip call asked for the path immediately
    /// after. Never stale in practice: nothing calls `to_render_space` again
    /// before the caller that asked for it has charged `lines` from it.
    last_stats: PathLineStats,
    /// Open **seal runs**, innermost last. See [`Self::begin_seal_run`].
    /// Everything drawn while one is open goes into its scene rather than
    /// `scene`; only the seam seals of its fills go one level out, so they end
    /// up beneath the run's artwork when the run is closed and appended.
    pub(crate) runs: Vec<SealRun>,
    /// Scenes of closed runs, reset and kept for the next run, so a frame of
    /// a thousand symbols does not allocate a thousand encodings.
    pub(crate) spare_runs: Vec<Scene>,
}

/// One open seal run: the artwork drawn since it began, and how many groups
/// (clips, blends, alpha groups) that artwork has opened and not yet closed.
/// A seal drawn inside such a group must stay inside it, so it is drawn in
/// line — the old way — rather than lifted beneath the run.
pub(crate) struct SealRun {
    scene: Scene,
    open_groups: u32,
    /// The seals lifted out of the run, in draw order, drawn beneath its
    /// artwork — in *reverse* — when it closes. See [`SceneBuilder::end_seal_run`].
    seals: Vec<SealOp>,
}

/// One seam seal, held until its run closes.
struct SealOp {
    path: BezPath,
    brush: peniko::Brush,
    brush_transform: Option<Affine>,
    width: f64,
    view: Affine,
}

/// The scene drawing goes into: the innermost open run's, or the root.
/// A free function over the two fields, so a caller can still read
/// `self.split` in the same expression.
fn target<'s>(root: &'s mut Scene, runs: &'s mut [SealRun]) -> &'s mut Scene {
    match runs.last_mut() {
        Some(run) => &mut run.scene,
        None => root,
    }
}

impl<'a> SceneBuilder<'a> {
    /// Begin building against `camera`. Resets the scene.
    pub fn new(scene: &'a mut Scene, camera: &Camera) -> Self {
        scene.reset();
        if probe::enabled() {
            probe::reset(); // probe only (S12)
        }
        Self {
            scene,
            split: camera.render_split(),
            clip: RenderClip::new(camera.visible_doc_rect()),
            visible: camera.visible_doc_rect(),
            lines: 0,
            // `render_split` hands the GPU only a rotation and a translation
            // (see `RenderSplit::gpu_view`'s doc): unit scale, until
            // `with_output_scale` multiplies it in.
            device_scale: 1.0,
            last_stats: PathLineStats::default(),
            runs: Vec::new(),
            spare_runs: Vec::new(),
        }
    }

    /// **Begin a seal run**: until [`Self::end_seal_run`], every fill's seam
    /// seal is drawn *beneath* all the artwork of the run instead of over the
    /// artwork drawn before it.
    ///
    /// # The defect this exists for
    ///
    /// The seal ([`Self::fill_shape_paint_sealed`]) grows a fill half a
    /// device pixel on every side, over whatever lies next to it. Drawn in
    /// line, that half pixel lands on the shape drawn *before* — and imported
    /// artwork draws its line work first and its colour regions after, so
    /// every region swallowed half a pixel of the lines under it from each
    /// side. At stage size, where barish1's nose and cheek lines are well
    /// under a pixel wide, they vanished altogether; Animate, which
    /// scan-converts a shape's regions together, keeps them as dark
    /// one-pixel lines.
    ///
    /// Beneath the run, a seal can only show where the run's own artwork
    /// leaves a pixel short of full coverage — which is exactly the pale
    /// conflation seam it exists to close — while every fill keeps its own
    /// exact edge over its neighbours. A thin line between two regions comes
    /// out at its true coverage, and a seam between two regions is still
    /// closed.
    ///
    /// Runs nest (a symbol inside a layer inside a symbol): an inner run's
    /// seals go into the outer run's artwork, beneath the inner symbol but
    /// above what the outer run drew before it, which is also what a stamped
    /// copy of the inner symbol draws, so stamping stays pixel-exact.
    ///
    /// A seal drawn while the run has a group of its own open (a mask's clip,
    /// a blend, an alpha group) stays in line, inside that group, as before:
    /// lifted out, it would escape the clip or the group's opacity.
    pub fn begin_seal_run(&mut self) {
        let mut scene = self.spare_runs.pop().unwrap_or_default();
        scene.reset();
        self.runs.push(SealRun {
            scene,
            open_groups: 0,
            seals: Vec::new(),
        });
    }

    /// Close the innermost seal run: the seals it lifted out are drawn, last
    /// first, and its artwork appended over them. See [`Self::begin_seal_run`].
    pub fn end_seal_run(&mut self) {
        let Some(run) = self.runs.pop() else {
            return;
        };
        debug_assert_eq!(run.open_groups, 0, "a seal run must close the groups it opened");
        let parent = target(&mut *self.scene, &mut self.runs);
        // **Last drawn, first sealed**: in reverse, so where two seals
        // overlap under a seam the *lower* shape's is on top. A pixel two
        // regions share is short exactly the lower region's part — the upper
        // one is drawn over it at its own coverage — so that is the colour
        // the gap must be filled with. Drawn forward, a line under a colour
        // region lost two thirds of its coverage to the region's seal.
        for op in run.seals.iter().rev() {
            parent.stroke(
                &seal_stroke(op.width), // S12c experiment
                op.view,
                &op.brush,
                op.brush_transform,
                &op.path,
            );
        }
        parent.append(&run.scene, None);
        let mut scene = run.scene;
        scene.reset();
        self.spare_runs.push(scene);
    }

    /// A group was opened in the current run; see [`SealRun`].
    fn group_opened(&mut self) {
        if let Some(run) = self.runs.last_mut() {
            run.open_groups += 1;
        }
    }

    /// A group was closed in the current run; see [`SealRun`].
    fn group_closed(&mut self) {
        if let Some(run) = self.runs.last_mut() {
            run.open_groups = run.open_groups.saturating_sub(1);
        }
    }

    /// The scene a caller appends a pre-encoded fragment into (a stamped
    /// symbol, a cached layer): wherever drawing is going right now.
    pub(crate) fn target_scene(&mut self) -> &mut Scene {
        target(&mut *self.scene, &mut self.runs)
    }

    /// Move a document-space shape into render space, in `f64`.
    ///
    /// Three stages, in an order that is load-bearing:
    ///
    /// 1. **Clip in document space.** Bounds both segment count and coordinate
    ///    magnitude for shapes far larger than the viewport. Done *before* the
    ///    magnification so the clip rectangle is expressed in the same units as
    ///    the geometry.
    /// 2. **Subtract the anchor**, in `f64`. Operands are document-scale, so
    ///    this is well conditioned and the result is small.
    /// 3. **Magnify**, in `f64`. A pure scale has no translation term, so there
    ///    is nothing to cancel against.
    ///
    /// Steps 2 and 3 must stay separate: fusing them evaluates
    /// `zoom·p − zoom·anchor` and reintroduces catastrophic cancellation. See
    /// [`buzz_geom::RenderSplit`].
    fn to_render_space(&mut self, shape: &impl Shape) -> BezPath {
        self.to_render_space_for_stroke(shape, None)
    }

    /// [`Self::to_render_space`], for a caller that already knows this path
    /// is about to be stroked at `stroke_width_px` device pixels — so
    /// [`path_line_stats`] can cost each join at the angle it actually
    /// turns, which needs the width to know how many lines one turned degree
    /// is worth (`docs/RENDER_SEAMS_PLAN.md` Step 3). `None` for a fill or a
    /// clip, which never stroke what they just measured.
    fn to_render_space_for_stroke(
        &mut self,
        shape: &impl Shape,
        stroke_width_px: Option<f64>,
    ) -> BezPath {
        // Cheap even at extreme tolerance: kurbo grows a circle's segment
        // count as the sixth root of radius/tolerance, so a 300-unit circle at
        // a 5e-12 tolerance is ~200 cubics, not millions.
        let path = shape.to_path(self.tolerance());
        let clipped = self.clip.apply(&path);
        if probe::enabled() {
            probe::stage_makeup(&path, &clipped); // probe only (S12c)
        }
        let centred = Affine::translate(-self.split.anchor.to_vec2()) * clipped;
        let render_path = Affine::scale(self.split.scale) * centred;
        // One walk over the finished render-space path, done here rather than
        // by every fill/stroke/clip call that goes on to use it, so charging
        // `estimated_lines` never costs a second pass over the geometry. See
        // `PathLineStats`.
        self.last_stats = path_line_stats(&render_path, self.device_scale, stroke_width_px);
        if probe::enabled() {
            // probe only (S12): device-pixel bbox of this path.
            let mut dev = render_path.clone();
            dev.apply_affine(self.split.gpu_view);
            let b = dev.bounding_box();
            probe::set_bbox([b.x0, b.y0, b.x1, b.y1]);
        }
        render_path
    }

    /// The region within which geometry is preserved exactly.
    pub fn clip_bounds(&self) -> buzz_geom::Rect {
        self.clip.bounds()
    }

    /// **What can actually be seen** — the camera's visible rectangle, with no
    /// margin. A cull built from this (plus whatever small safety margin the
    /// caller wants) skips geometry the viewport truly cannot show, unlike
    /// [`Self::clip_bounds`], which is nine times the area at fit zoom and was
    /// never meant to gate what gets encoded at all.
    pub fn visible_bounds(&self) -> buzz_geom::Rect {
        self.visible
    }

    /// **How much geometry this scene has taken so far**, as Vello's own count
    /// of encoded path segments.
    ///
    /// Read rather than tallied: the encoding is the thing that will be
    /// rasterised, so its own number cannot drift from what was drawn. It is
    /// what [`crate::document::DrawCache::reconsider`] judges a frame by; see
    /// there for why a frame has to be judged at all.
    pub fn encoded_segments(&self) -> u32 {
        self.scene.encoding().n_path_segments
            + self.runs.iter().map(|r| r.scene.encoding().n_path_segments).sum::<u32>()
    }

    /// **How many lines Vello's flattener will produce for this scene so
    /// far**, mirroring `vello_shaders`' own `flatten.wgsl` closely enough,
    /// and padded by [`LINE_ESTIMATE_MARGIN`], that the estimate never lands
    /// under the GPU's real count (see the calibration `import_budget
    /// --features gpu-truth` checks, and that constant's own doc for the
    /// fitted data). This is what [`crate::document::DrawCache::note_lines`]
    /// judges a frame by, alongside [`Self::encoded_segments`]: segments
    /// alone missed the line buffer overflowing on curvy, heavily stroked
    /// artwork well under the segment ceiling — see "Status after Phases
    /// 1-3" in `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
    pub fn estimated_lines(&self) -> u64 {
        (self.lines as f64 * LINE_ESTIMATE_MARGIN).ceil() as u64
    }

    /// Charge this call's fill (or clip) against [`Self::estimated_lines`],
    /// from the stats the `to_render_space` call that built its path just
    /// gathered. Called twice, back to back, wherever a method fills the same
    /// path it just clipped to (`push_layer` then `fill`) — Vello flattens
    /// the shared path once for each.
    fn add_fill_lines(&mut self) {
        self.lines += self.last_stats.fill;
        if probe::enabled() {
            probe::push(probe::Kind::Fill, self.last_stats.fill, 0.0); // probe only (S12)
        }
    }

    /// Charge a stroke of `render_width` — already in render-space units,
    /// i.e. a document length times `split.scale` — against
    /// [`Self::estimated_lines`].
    fn add_stroke_lines(&mut self, render_width: f64) {
        let device_width = render_width * self.device_scale;
        self.lines += stroke_line_estimate(&self.last_stats, device_width);
        if probe::enabled() {
            // probe only (S12)
            probe::push(
                probe::Kind::Stroke,
                stroke_line_estimate(&self.last_stats, device_width),
                device_width,
            );
        }
    }

    /// Shift the rendered output by a screen-space offset.
    ///
    /// The editor draws the stage into the central area between the docked
    /// panels, but Vello renders across the whole window texture. The camera's
    /// viewport is set to the central rectangle's *size*, and this supplies its
    /// *origin*, so document coordinates land in the right place on screen.
    ///
    /// Applied to the GPU transform only, which keeps it away from the
    /// precision-critical CPU stages.
    ///
    /// **A non-finite offset is ignored rather than applied.** The window
    /// derives this from the rectangle egui gave the stage, and that rectangle
    /// is `Rect::NOTHING` — infinities — until the layout has been measured
    /// once, which is the first frame after the app opens and can be a frame
    /// again after the window is maximised. An infinity here goes into the GPU
    /// transform, every coordinate through it comes out NaN, and the whole
    /// frame rasterises to nothing: a black stage, with the lighting apparently
    /// off because *everything* is off. Drawing the frame at the origin for one
    /// frame is wrong by a few pixels; drawing it black is wrong by the whole
    /// picture.
    pub fn with_viewport_offset(mut self, offset: buzz_geom::Vec2) -> Self {
        if !offset.x.is_finite() || !offset.y.is_finite() {
            return self;
        }
        self.split.gpu_view = Affine::translate(offset) * self.split.gpu_view;
        self
    }

    /// Scale the rendered output by the display's device-pixel ratio.
    ///
    /// The camera works in **logical** points, the same space egui lays the
    /// panels and draws the selection chrome in, so a click maps to the same
    /// place the chrome is drawn. Vello's target is **physical** pixels, so the
    /// finished output is scaled up by `pixels_per_point` here — on the GPU
    /// transform only, after the precision-critical CPU split. On a 1:1 display
    /// this is the identity. Apply **before** [`Self::with_viewport_offset`] so
    /// the offset, already in physical pixels, is not scaled again.
    pub fn with_output_scale(mut self, scale: f64) -> Self {
        self.split.gpu_view = Affine::scale(scale) * self.split.gpu_view;
        // Kept in step with `gpu_view`'s own magnification — see the field's
        // doc comment.
        self.device_scale *= scale.abs();
        self
    }

    /// Curve-flattening tolerance, in **document units**, for the current zoom.
    ///
    /// A fixed document-space tolerance is wrong in both directions, and both
    /// were measured:
    ///
    /// * **Too coarse when zoomed in.** At 1e12× zoom the artwork's features
    ///   are ~3e-10 document units across. A constant `1e-9` tolerance is
    ///   *larger than the shape*, so curves collapse and detail is destroyed
    ///   exactly where this project claims to excel.
    /// * **Too fine when zoomed out.** At 100% the same constant subdivides a
    ///   300-unit circle to nanometre precision, producing enormous paths for
    ///   sub-pixel gains.
    ///
    /// Anchoring to ~0.1 px on screen makes the cost and the quality both
    /// scale-invariant.
    #[inline]
    pub fn tolerance(&self) -> f64 {
        const TARGET_PX: f64 = 0.1;
        let scale = self.view_scale();
        if scale.is_finite() && scale > 0.0 {
            (TARGET_PX / scale).max(f64::MIN_POSITIVE)
        } else {
            TARGET_PX
        }
    }

    /// Fill a document-space shape.
    pub fn fill_shape(&mut self, shape: &impl Shape, color: Color) {
        let path = self.to_render_space(shape);
        target(&mut *self.scene, &mut self.runs)
            .fill(Fill::NonZero, self.split.gpu_view, color, None, &path);
        self.add_fill_lines();
    }

    /// The affine that carries document space into render space.
    ///
    /// The same anchor-then-scale the geometry takes in [`Self::to_render_space`],
    /// without the clip — a brush has no segments to bound.
    fn doc_to_render(&self) -> Affine {
        Affine::scale(self.split.scale) * Affine::translate(-self.split.anchor.to_vec2())
    }

    /// Turn a document paint into a Vello brush, plus the transform that puts
    /// it where the artwork is.
    ///
    /// # Why the brush transform, and not gradient coordinates in render space
    ///
    /// The gradient is defined in unit space, so everything that positions it —
    /// the object's placement, the camera, and the render split — is one
    /// matrix. Handing Vello that matrix as the brush transform means the
    /// gradient goes through *exactly* what the path went through, composed in
    /// `f64` here rather than accumulated separately. Vello's own encoding
    /// multiplies it by the same `gpu_view` the path is drawn with (see
    /// `Scene::fill`), so the ramp cannot drift away from the shape it fills at
    /// any zoom.
    fn brush_for(&self, paint: &Paint, to_doc: Affine) -> (peniko::Brush, Option<Affine>) {
        // A bitmap is a brush too, placed by the same chain — the object's
        // own space, then the camera, then the render split — so a photograph
        // stays stuck to the shape it fills at any zoom, exactly as a gradient
        // does. Vello samples the image over the unit square, which is the
        // space `ImageFill::transform` is defined in, so the two agree with no
        // conversion at all.
        if let Some(fill) = paint.image() {
            let asset = &fill.asset;
            let data = peniko::ImageData {
                // The same allocation the document holds, not a copy: an
                // `Arc<Vec<u8>>` coerces to the trait object Blob wants, so a
                // 4K photograph is shared with the scene rather than cloned
                // into every frame.
                // **A stable identity, not a fresh one.** The GPU caches by
                // this number, and `Blob::new` takes a new one from a global
                // counter on every call — so building the brush per frame made
                // every frame a cache miss and re-uploaded the whole
                // photograph. `blob_id` is the asset's own id mixed with a
                // counter that moves only when the pixels do.
                data: peniko::Blob::from_raw_parts(
                    Arc::clone(&asset.pixels) as Arc<dyn AsRef<[u8]> + Send + Sync>,
                    asset.blob_id(),
                ),
                format: peniko::ImageFormat::Rgba8,
                // Straight alpha, as the model stores it and as PNG does.
                alpha_type: peniko::ImageAlphaType::Alpha,
                width: asset.width,
                height: asset.height,
            };
            let quality = if fill.smooth {
                peniko::ImageQuality::Medium
            } else {
                peniko::ImageQuality::Low
            };
            // A tiling fill (a seamless texture) repeats across the shape; an
            // ordinary photograph pads its last row of pixels out to the edge.
            let extend = if fill.tile {
                peniko::Extend::Repeat
            } else {
                peniko::Extend::Pad
            };
            let brush = peniko::ImageBrush {
                image: data,
                sampler: peniko::ImageSampler {
                    x_extend: extend,
                    y_extend: extend,
                    quality,
                    alpha: 1.0,
                },
            };
            // Vello samples in *pixel* space, so the unit square has to be
            // scaled up to the image's own size before the placement is
            // applied. Getting this the wrong way round draws one pixel of the
            // photograph across the whole shape.
            let to_pixels = Affine::scale_non_uniform(
                1.0 / f64::from(asset.width).max(1.0),
                1.0 / f64::from(asset.height).max(1.0),
            );
            let placed = self.doc_to_render() * to_doc * fill.transform * to_pixels;
            return (peniko::Brush::Image(brush), Some(placed));
        }

        let Some(g) = paint.gradient() else {
            return (peniko::Brush::Solid(paint.color()), None);
        };

        let stops: Vec<peniko::ColorStop> = g
            .stops()
            .iter()
            .map(|s| peniko::ColorStop::from((s.offset as f32, s.color)))
            .collect();

        let kind = match g.kind {
            GradientKind::Linear => peniko::GradientKind::Linear(
                peniko::LinearGradientPosition::new((-1.0, 0.0), (1.0, 0.0)),
            ),
            // The focal point is Animate's: the hot spot slides along the unit
            // x axis while the outer circle stays put. A two-point radial with
            // a zero inner radius is exactly that, and is what SWF's focal
            // gradients mean as well.
            GradientKind::Radial => {
                peniko::GradientKind::Radial(peniko::RadialGradientPosition::new_two_point(
                    (g.focal.clamp(-1.0, 1.0), 0.0),
                    0.0,
                    (0.0, 0.0),
                    1.0,
                ))
            }
        };

        let mut brush = peniko::Gradient {
            kind,
            ..Default::default()
        };
        brush.extend = match g.spread {
            GradientSpread::Pad => peniko::Extend::Pad,
            GradientSpread::Reflect => peniko::Extend::Reflect,
            GradientSpread::Repeat => peniko::Extend::Repeat,
        };
        brush.stops = stops.as_slice().into();

        let placed = self.doc_to_render() * to_doc * g.transform;
        (peniko::Brush::Gradient(brush), Some(placed))
    }

    /// Fill a document-space shape with a paint, which may be a gradient.
    ///
    /// `to_doc` maps the paint's own space — the object's — into document
    /// space. It is the accumulated placement the caller already has, and it is
    /// what keeps a gradient stuck to its artwork when the artwork is moved,
    /// scaled or turned.
    pub fn fill_shape_paint(&mut self, shape: &impl Shape, paint: &Paint, to_doc: Affine) {
        let path = self.to_render_space(shape);
        let (brush, brush_transform) = self.brush_for(paint, to_doc);
        target(&mut *self.scene, &mut self.runs).fill(
            Fill::NonZero,
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
        self.add_fill_lines();
    }

    /// Fill a shape and **seal its edge**, so it does not leave a pale seam
    /// against the shape beside it.
    ///
    /// # The defect this exists for
    ///
    /// Two filled shapes that share an edge should meet with nothing between
    /// them. They do not. Every path is antialiased independently, so along a
    /// shared boundary each shape covers about *half* of every pixel — and
    /// compositing one over the other gives `0.5 + 0.5 × (1 − 0.5) = 0.75`, not
    /// `1.0`. The remaining quarter is the stage showing through, and it reads
    /// as a thin pale line tracing every border in the drawing. It is worst on
    /// imported artwork, because Flash drew a shape as a *soup of edges* that
    /// were scan-converted together in one pass, so every region in a `.fla`
    /// shares its boundary exactly with its neighbour. This is the well-known
    /// conflation artifact, and it is a property of compositing paths
    /// separately rather than a bug in any one of them.
    ///
    /// # The fix, and why it is a stroke
    ///
    /// The shape is stroked with **its own paint**, a hair under a pixel wide.
    /// That pushes its coverage half a pixel outwards, so two neighbours now
    /// overlap across the boundary instead of meeting exactly on it, and the
    /// pixel sums to full. Growing the geometry with a boolean would be exact
    /// and would cost an offset per shape per frame; a stroke is one extra path
    /// and the difference is half a pixel of silhouette.
    ///
    /// # When it must not happen
    ///
    /// **Only for opaque paint.** A translucent fill stroked with itself
    /// composites twice around its rim, which draws a visible darker outline —
    /// turning a subtle seam into an obvious border. Translucent artwork keeps
    /// the seam, which is the lesser of the two.
    pub fn fill_shape_paint_sealed(&mut self, shape: &impl Shape, paint: &Paint, to_doc: Affine) {
        let bounds = shape.bounding_box();
        let extent = bounds.width().max(bounds.height());
        self.fill_shape_paint_sealed_sized(shape, paint, to_doc, extent);
    }

    /// [`Self::fill_shape_paint_sealed`] for a caller that already knows the
    /// shape's longest side, in the same units as `shape` — the document
    /// draw has its bounds in hand — so deciding whether the shape is large
    /// enough for the whole seal costs no further walk of the path. See
    /// [`seal_width_px_lazy`].
    pub(crate) fn fill_shape_paint_sealed_sized(
        &mut self,
        shape: &impl Shape,
        paint: &Paint,
        to_doc: Affine,
        extent: f64,
    ) {
        self.fill_shape_paint(shape, paint, to_doc);
        if !is_opaque(paint) {
            return;
        }
        // In screen pixels: the seam is a property of the *rasteriser*, not of
        // the document, so it is the same width at every zoom. Slightly under a
        // pixel — a full pixel of overlap is enough to close the gap, and less
        // silhouette is disturbed than a wider stroke would disturb.
        //
        // **Never more than a small share of the shape itself.** See
        // [`seal_width_px`]: the caller's bounds give the extent for nothing;
        // only a shape too small for the full seal on that alone is measured
        // any further.
        let extent_px = extent * self.pixels_per_unit();
        let device_scale = self.device_scale;
        let split_scale = self.split.scale;
        let Some(seal_px) = seal_width_px_lazy(extent_px, || {
            // A small shape only: its document path, measured once and
            // carried to device pixels by the same scale the render path took.
            path_thickness(&shape.to_path(0.1 / (split_scale * device_scale).max(f64::MIN_POSITIVE)))
                * split_scale
                * device_scale
        }) else {
            return;
        };
        let width = seal_px / self.view_scale().max(f64::MIN_POSITIVE);
        // A3: a subpath that encloses nothing is never sealed (see `sliver_subpaths`).
        if let Some(elements) = shape.as_path_slice() {
            let min_thickness = SEAL_SLIVER_PX / self.pixels_per_unit().max(f64::MIN_POSITIVE);
            let slivers = sliver_subpaths(elements, min_thickness);
            if !slivers.is_empty() {
                let kept = without_subpaths(elements, &slivers);
                if !kept.elements().is_empty() {
                    self.stroke_shape_paint_into(&kept, paint, width, to_doc, true);
                }
                return;
            }
        }
        self.stroke_shape_paint_into(shape, paint, width, to_doc, true);
    }

    /// Stroke a document-space shape with a paint, which may be a gradient.
    pub fn stroke_shape_paint(
        &mut self,
        shape: &impl Shape,
        paint: &Paint,
        width: f64,
        to_doc: Affine,
    ) {
        self.stroke_shape_paint_into(shape, paint, width, to_doc, false);
    }

    /// [`Self::stroke_shape_paint`], drawn beneath the open seal run when
    /// `as_seal` is set. See [`Self::begin_seal_run`].
    fn stroke_shape_paint_into(
        &mut self,
        shape: &impl Shape,
        paint: &Paint,
        width: f64,
        to_doc: Affine,
        as_seal: bool,
    ) {
        let render_width = self.split.scale_length(width).max(f64::MIN_POSITIVE);
        let path =
            self.to_render_space_for_stroke(shape, Some(render_width * self.device_scale));
        let (brush, brush_transform) = self.brush_for(paint, to_doc);
        if as_seal
            && let Some(run) = self.runs.last_mut()
            && run.open_groups == 0
        {
            run.seals.push(SealOp {
                path,
                brush,
                brush_transform,
                width: render_width,
                view: self.split.gpu_view,
            });
            if probe::enabled() && as_seal {
                probe::tag(probe::Kind::Seal); // probe only (S12)
                probe::note_seal_makeup(); // probe only (S12c)
            }
            self.add_stroke_lines(render_width);
            return;
        }
        let pen = if as_seal {
            seal_stroke(render_width) // S12c experiment
        } else {
            kurbo::Stroke::new(render_width)
        };
        target(&mut *self.scene, &mut self.runs).stroke(
            &pen,
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
        if probe::enabled() && as_seal {
            probe::tag(probe::Kind::Seal); // probe only (S12)
            probe::note_seal_makeup(); // probe only (S12c)
        }
        self.add_stroke_lines(render_width);
    }

    /// Fill additively with a paint. See [`Self::fill_shape_additive`], which
    /// this is the gradient-capable form of.
    pub fn fill_shape_paint_additive(&mut self, shape: &impl Shape, paint: &Paint, to_doc: Affine) {
        let path = self.to_render_space(shape);
        let (brush, brush_transform) = self.brush_for(paint, to_doc);
        let blend = peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::Plus);
        target(&mut *self.scene, &mut self.runs)
            .push_layer(Fill::NonZero, blend, 1.0, self.split.gpu_view, &path);
        target(&mut *self.scene, &mut self.runs).fill(
            Fill::NonZero,
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
        target(&mut *self.scene, &mut self.runs).pop_layer();
        // The clip and the fill each flatten the same path once.
        self.add_fill_lines();
        self.add_fill_lines();
    }

    /// Fill with a paint the way **light** falls on what is already there:
    /// everything gets brighter, nothing gets darker, and nothing goes past
    /// white.
    ///
    /// Screen rather than Plus, which is the other obvious choice. Plus is what
    /// two beams of light do to each other in a vacuum and it blows past white
    /// the moment a lamp is anywhere near a pale surface — a cream wall under a
    /// warm lamp turns into a white disc with an edge. Screen is
    /// `1 − (1 − a)(1 − b)`: it approaches white and never reaches it, which is
    /// how a photograph of a lit wall behaves and what a painter reaches for.
    /// Over black it does nothing, which is also right — a lamp cannot light
    /// what absorbs everything.
    pub fn fill_shape_paint_lit(&mut self, shape: &impl Shape, paint: &Paint, to_doc: Affine) {
        let path = self.to_render_space(shape);
        let (brush, brush_transform) = self.brush_for(paint, to_doc);
        let blend = peniko::BlendMode::new(peniko::Mix::Screen, peniko::Compose::SrcOver);
        target(&mut *self.scene, &mut self.runs)
            .push_layer(Fill::NonZero, blend, 1.0, self.split.gpu_view, &path);
        target(&mut *self.scene, &mut self.runs).fill(
            Fill::NonZero,
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
        target(&mut *self.scene, &mut self.runs).pop_layer();
        self.add_fill_lines();
        self.add_fill_lines();
    }

    /// Lay `color` over what is already drawn inside `shape`, **and only where
    /// something is already drawn**.
    ///
    /// This is how light reaches artwork whose colours cannot be rewritten one
    /// by one — a bitmap. The light becomes a blend over the picture instead of
    /// a change to the paint; see [`buzz_light::Illumination::as_filter`].
    ///
    /// # Why `SrcAtop`
    ///
    /// A cut-out photograph is transparent over most of its own rectangle. Laid
    /// on with ordinary source-over, a multiply against a transparent backdrop
    /// paints the colour straight in — so the light would fill the hole in the
    /// cut-out with itself, and a character on a white stage would gain a
    /// coloured rectangle around it. `SrcAtop` keeps the backdrop's alpha, so
    /// the light lands on the picture and nowhere else.
    ///
    /// The caller is responsible for what "already drawn" means: called outside
    /// an isolation group the backdrop includes the stage, and the light would
    /// tint that too. See [`Self::push_isolation`].
    pub fn fill_shape_atop(&mut self, shape: &impl Shape, color: Color, blend: buzz_fx::Blend) {
        let path = self.to_render_space(shape);
        let mix = blend_mode(blend).mix;
        target(&mut *self.scene, &mut self.runs).push_layer(
            Fill::NonZero,
            peniko::BlendMode::new(mix, peniko::Compose::SrcAtop),
            1.0,
            self.split.gpu_view,
            &path,
        );
        target(&mut *self.scene, &mut self.runs)
            .fill(Fill::NonZero, self.split.gpu_view, color, None, &path);
        target(&mut *self.scene, &mut self.runs).pop_layer();
        self.add_fill_lines();
        self.add_fill_lines();
    }

    /// [`Self::fill_shape_atop`], with a paint instead of one colour.
    ///
    /// **This is what makes a lamp fall off across a shape.** A light laid over
    /// artwork as a solid can only tint it evenly; the same pass with a radial
    /// gradient — the lamp's own falloff, centred where it stands — lands the
    /// light per pixel, so the near side of a face is brighter than the far side
    /// of the same face. See [`buzz_light::LightRig::field`].
    ///
    /// `to_doc` maps the paint's own space into document space, exactly as
    /// [`Self::fill_shape_paint`] takes it. A light's ramp is already in document
    /// space, so what it wants is the projection alone.
    pub fn fill_shape_atop_paint(
        &mut self,
        shape: &impl Shape,
        paint: &Paint,
        to_doc: Affine,
        blend: buzz_fx::Blend,
    ) {
        let path = self.to_render_space(shape);
        let (brush, brush_transform) = self.brush_for(paint, to_doc);
        let mix = blend_mode(blend).mix;
        target(&mut *self.scene, &mut self.runs).push_layer(
            Fill::NonZero,
            peniko::BlendMode::new(mix, peniko::Compose::SrcAtop),
            1.0,
            self.split.gpu_view,
            &path,
        );
        target(&mut *self.scene, &mut self.runs).fill(
            Fill::NonZero,
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
        target(&mut *self.scene, &mut self.runs).pop_layer();
        self.add_fill_lines();
        self.add_fill_lines();
    }

    /// Fill a shape so that its opacity **adds** to whatever is under it.
    ///
    /// Two strokes at alpha 0.2 and 0.3 overlap at exactly 0.5, rather than the
    /// 0.44 that ordinary source-over gives. Where the colours differ the
    /// result is their mix weighted by how much of each is present, because
    /// the sum happens in premultiplied space and is divided back out by the
    /// summed alpha — so this deepens paint without washing it towards white.
    ///
    /// # This must be inside an isolation group
    ///
    /// Additive compositing sums the source with the destination, and the
    /// destination includes the stage. Called outside a group, a dark stroke
    /// on a white background would sum to white and vanish. See
    /// [`Self::push_isolation`], which the caller is responsible for opening.
    ///
    /// # Cost
    ///
    /// Vello sets blending per *layer*, not per fill, so each additive shape
    /// costs a layer push and pop. That is real GPU work, which is why it is a
    /// separate method the caller opts into rather than the default path.
    pub fn fill_shape_additive(&mut self, shape: &impl Shape, color: Color) {
        let path = self.to_render_space(shape);
        let blend = peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::Plus);

        // The shape is its own clip, so the fill inside the layer covers
        // exactly the shape and nothing else.
        target(&mut *self.scene, &mut self.runs)
            .push_layer(Fill::NonZero, blend, 1.0, self.split.gpu_view, &path);
        target(&mut *self.scene, &mut self.runs)
            .fill(Fill::NonZero, self.split.gpu_view, color, None, &path);
        target(&mut *self.scene, &mut self.runs).pop_layer();
        self.add_fill_lines();
        self.add_fill_lines();
    }

    /// Fill a shape with the **even-odd** rule.
    ///
    /// What a ring is drawn with: an outer boundary and an inner one in the
    /// same path, with the hole between them. Non-zero would fill the lot.
    pub fn fill_shape_even_odd(&mut self, shape: &impl Shape, color: Color) {
        let path = self.to_render_space(shape);
        target(&mut *self.scene, &mut self.runs)
            .fill(Fill::EvenOdd, self.split.gpu_view, color, None, &path);
        self.add_fill_lines();
    }

    /// Stroke a shape with an extra transform applied to the **pen** as well as
    /// the path.
    ///
    /// This is what draws an elliptical soft edge: `buzz-fx` squashes the path
    /// so the blur is round, and the transform stretches path and pen back
    /// together. A round pen scaled by a width would stay round.
    ///
    /// The transform is applied in document space, before the render split, so
    /// it is subject to the same magnification as everything else.
    pub fn stroke_transformed(
        &mut self,
        shape: &impl Shape,
        color: Color,
        width: f64,
        transform: Affine,
    ) {
        // The path is carried into the transformed space by hand; the pen is
        // carried by handing Vello the transform, which strokes under it.
        let render_width = self.split.scale_length(width).max(f64::MIN_POSITIVE);
        // The join/cap arc count is sized by the transform's largest
        // stretch rather than its scale alone — see the comment on
        // `add_stroke_lines` below — so `path_line_stats` must be gathered
        // at that same effective width, not the untransformed one.
        let path = self.to_render_space_for_stroke(
            shape,
            Some(render_width * max_stretch(transform) * self.device_scale),
        );

        // Render space is the document scaled about an anchor, so a transform
        // meant for document space has to be conjugated into it: shift to the
        // anchor, scale, apply, and undo. For the scale-only transforms this
        // is used with, that reduces to the transform itself.
        target(&mut *self.scene, &mut self.runs).stroke(
            &kurbo::Stroke::new(render_width),
            self.split.gpu_view * transform,
            color,
            None,
            &(transform.inverse() * path),
        );
        // The centreline flattens exactly where `add_stroke_lines` already
        // assumes: `transform` cancels between the path it is divided out of
        // here and the pen transform it is folded into above, leaving
        // `gpu_view * path` either way. Only the *pen* stays affected —
        // `transform` stretches a round pen into an ellipse along with the
        // path — so the join/cap arc count is sized by the transform's
        // largest stretch rather than its scale alone: an overestimate on the
        // minor axis, never the underestimate this exists to avoid.
        self.add_stroke_lines(render_width * max_stretch(transform));
    }

    /// Begin a group that composites with what is behind it using `blend`.
    ///
    /// Animate's blend modes need a backdrop to blend *with*, and without a
    /// group that backdrop is the entire stage — so every mode but Normal is
    /// drawn into one of these.
    pub fn push_blend(&mut self, bounds: buzz_geom::Rect, blend: buzz_fx::Blend) {
        let path = self.to_render_space(&bounds);
        target(&mut *self.scene, &mut self.runs).push_layer(
            Fill::NonZero,
            blend_mode(blend),
            1.0,
            self.split.gpu_view,
            &path,
        );
        if probe::enabled() {
            probe::tag(probe::Kind::Clip); // probe only (S12)
        }
        self.add_fill_lines();
        self.group_opened();
    }

    /// Begin a transparent group that later drawing composites into.
    ///
    /// Ordinary source-over drawing is unaffected by being inside one — an
    /// isolation group at full alpha is invisible to it — so a layer can be
    /// wrapped unconditionally when it holds any additive paint, without
    /// changing how its normal shapes look.
    ///
    /// `bounds` limits the group to the region that needs it. Every group is a
    /// render target, so an unbounded one would cost a full-viewport buffer.
    pub fn push_isolation(&mut self, bounds: buzz_geom::Rect) {
        let path = self.to_render_space(&bounds);
        target(&mut *self.scene, &mut self.runs).push_layer(
            Fill::NonZero,
            peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
            1.0,
            self.split.gpu_view,
            &path,
        );
        if probe::enabled() {
            probe::tag(probe::Kind::Clip); // probe only (S12)
        }
        self.add_fill_lines();
        self.group_opened();
    }

    /// Begin a group that is composited at `alpha` when it closes.
    ///
    /// **What this buys over drawing at that alpha directly**: inside the
    /// group, overlapping shapes do not compound. Two half-transparent shapes
    /// laid one on another make three quarters; the same two drawn opaque
    /// inside a group closed at a half make a half, everywhere, which is what
    /// a *silhouette* means. That is exactly the difference between a shadow
    /// and a stack of shadows — see the shadow pass in
    /// [`crate::document`].
    ///
    /// `bounds` limits the group as [`Self::push_isolation`] does, and for the
    /// same reason: a group is a render target.
    pub fn push_alpha_group(&mut self, bounds: buzz_geom::Rect, alpha: f32) {
        let path = self.to_render_space(&bounds);
        target(&mut *self.scene, &mut self.runs).push_layer(
            Fill::NonZero,
            peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
            alpha.clamp(0.0, 1.0),
            self.split.gpu_view,
            &path,
        );
        if probe::enabled() {
            probe::tag(probe::Kind::Clip); // probe only (S12)
        }
        self.add_fill_lines();
        self.group_opened();
    }

    /// Close the group opened by [`Self::push_isolation`] or
    /// [`Self::push_alpha_group`].
    pub fn pop_isolation(&mut self) {
        target(&mut *self.scene, &mut self.runs).pop_layer();
        self.group_closed();
    }

    /// Clip everything drawn until the next [`Self::pop_isolation`] to `shape`.
    ///
    /// This is what a mask layer does: the shape is the mask's own artwork, and
    /// the layers it claims show only where that artwork covers them.
    ///
    /// **Non-zero fill, deliberately.** A mask drawn as several separate blobs
    /// shows through all of them; under even-odd, two overlapping blobs would
    /// punch a hole where they cross, which is not what anybody drawing a mask
    /// means. The shape goes through the same document-space clipping and
    /// rebasing as artwork, so a mask survives extreme zoom like everything
    /// else.
    pub fn push_clip(&mut self, shape: &impl Shape) {
        let path = self.to_render_space(shape);
        target(&mut *self.scene, &mut self.runs).push_layer(
            Fill::NonZero,
            peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
            1.0,
            self.split.gpu_view,
            &path,
        );
        if probe::enabled() {
            probe::tag(probe::Kind::Clip); // probe only (S12)
        }
        self.add_fill_lines();
        self.group_opened();
    }

    /// Hide, rather than reveal: everything drawn until the matching
    /// [`Self::pop_inverse_clip`] is kept *except* where the shape covers it.
    ///
    /// **Why this is not a clip.** The obvious inverse — a huge rectangle with
    /// the mask's subpaths reversed inside it — is wrong for any mask made of
    /// overlapping blobs: under the non-zero rule two reversed overlapping
    /// shapes wind back to a filled region, and the overlap would show through
    /// the hole it is supposed to be part of. So the run is drawn into a group
    /// of its own and the mask is then *punched out* of it with `DestOut`,
    /// which is exact whatever the geometry does.
    ///
    /// `bounds` must cover everything the group draws: it is the group's
    /// render target, and artwork outside it is cut off.
    pub fn push_inverse_clip(&mut self, bounds: buzz_geom::Rect) {
        self.push_isolation(bounds);
    }

    /// Close the group opened by [`Self::push_inverse_clip`], punching `shape`
    /// out of it.
    pub fn pop_inverse_clip(&mut self, shape: &impl Shape) {
        let path = self.to_render_space(shape);
        let punch = peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::DestOut);

        // The shape is its own clip, so the layer that composites with
        // `DestOut` covers exactly the region to remove.
        target(&mut *self.scene, &mut self.runs)
            .push_layer(Fill::NonZero, punch, 1.0, self.split.gpu_view, &path);
        target(&mut *self.scene, &mut self.runs).fill(
            Fill::NonZero,
            self.split.gpu_view,
            // Any opaque colour: `DestOut` reads the source's alpha only.
            Color::BLACK,
            None,
            &path,
        );
        target(&mut *self.scene, &mut self.runs).pop_layer();
        self.add_fill_lines();
        self.add_fill_lines();

        self.pop_isolation();
    }

    /// Stroke a document-space shape with a width in document units.
    pub fn stroke_shape(&mut self, shape: &impl Shape, color: Color, width: f64) {
        // Geometry is already magnified, and `gpu_view` has unit scale, so the
        // stroke width must be magnified to match.
        let render_width = self.split.scale_length(width).max(f64::MIN_POSITIVE);
        let path =
            self.to_render_space_for_stroke(shape, Some(render_width * self.device_scale));
        target(&mut *self.scene, &mut self.runs).stroke(
            &kurbo::Stroke::new(render_width),
            self.split.gpu_view,
            color,
            None,
            &path,
        );
        self.add_stroke_lines(render_width);
    }

    /// Stroke with a constant on-screen width regardless of zoom.
    ///
    /// Guides, selection outlines and the stage border want this: the stroke
    /// is specified in pixels, so it is divided by the view scale to survive
    /// being multiplied by it again.
    pub fn stroke_hairline(&mut self, shape: &impl Shape, color: Color, px: f64) {
        let scale = self.view_scale().max(f64::MIN_POSITIVE);
        self.stroke_shape(shape, color, px / scale);
    }

    /// Document units to screen pixels. Used for culling and hairlines.
    #[inline]
    pub fn view_scale(&self) -> f64 {
        self.split.scale
    }

    /// Device pixels per document unit, output scale included. See
    /// [`crate::document`]'s `wants_seal_run`.
    pub(crate) fn pixels_per_unit(&self) -> f64 {
        self.split.scale * self.device_scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Point, Rect, Size};

    /// A shape at normal size takes the whole seal, exactly as before; an
    /// iris a few pixels across takes a share of its own radius; a thin line
    /// long enough to be line work keeps the whole seal; a speck gets none.
    #[test]
    fn seal_width_follows_the_shape_size() {
        assert_eq!(seal_width_px(100.0, 200.0), Some(SEAM_SEAL_PX));
        assert_eq!(seal_width_px(3.6, 3.6), Some(SEAM_SEAL_PX));
        let iris = seal_width_px(1.875, 3.75).unwrap();
        assert!((iris - 0.46875).abs() < 1e-9, "{iris}");
        assert_eq!(seal_width_px(0.5, 15.0), Some(SEAM_SEAL_PX));
        assert_eq!(seal_width_px(0.05, 0.1), None);
    }

    /// A3: the seal skips the subpaths that enclose nothing — three points 3e-6 px
    /// apart (verbatim from Raju's "Hand R" skin piece) and a line walked out and
    /// back — and keeps the outline and its hole; a one-subpath path is not measured.
    #[test]
    fn a_seal_skips_the_subpaths_that_enclose_nothing() {
        let mut path = Rect::new(43.0, 393.0, 191.0, 444.0).to_path(1e-9);
        path.extend(
            BezPath::from_svg(
                "M92.41563286635125,432.1693449456416 L92.4156357273742,432.16934470722305 \
                 L92.4156356081649,432.16934470722305 Z M78.0,433.0 L84.0,433.0 L78.0,433.0 Z",
            )
            .unwrap(),
        );
        path.extend(Rect::new(150.0, 400.0, 160.0, 410.0).to_path(1e-9).reverse_subpaths());
        assert_eq!(sliver_subpaths(path.elements(), SEAL_SLIVER_PX), vec![1, 2]);
        let kept = without_subpaths(path.elements(), &[1, 2]);
        assert_eq!(kept.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count(), 2);
        assert!((kept.area().abs() - (148.0 * 51.0 - 100.0)).abs() < 1e-6, "area {}", kept.area());
        let speck = Rect::new(0.0, 0.0, 1e-9, 1e-9).to_path(1e-9);
        assert!(sliver_subpaths(speck.elements(), SEAL_SLIVER_PX).is_empty());
    }

    /// `path_thickness` is a strip's width and a disc's radius,
    /// holes subtracted.
    #[test]
    fn path_thickness_measures_strips_discs_and_rings() {
        let strip = Rect::new(0.0, 0.0, 100.0, 2.0).to_path(1e-9);
        let t = path_thickness(&strip);
        assert!((t - 2.0 * 200.0 / 204.0).abs() < 1e-6, "strip {t}");
        let disc = kurbo::Circle::new(Point::new(0.0, 0.0), 10.0).to_path(1e-6);
        let t = path_thickness(&disc);
        assert!((t - 10.0).abs() < 0.1, "disc {t}");
        let mut ring = kurbo::Circle::new(Point::new(0.0, 0.0), 10.0).to_path(1e-6);
        let hole = kurbo::Circle::new(Point::new(0.0, 0.0), 9.0).to_path(1e-6).reverse_subpaths();
        ring.extend(hole);
        let t = path_thickness(&ring);
        assert!((t - 1.0).abs() < 0.05, "ring {t}");
    }

    /// Nothing large may reach the GPU: neither coordinates nor transform.
    #[test]
    fn gpu_receives_only_small_well_conditioned_numbers() {
        let mut scene = Scene::new();
        let cam = Camera::new(Point::new(1e6, 1e6), 1e9, Size::new(1920.0, 1080.0));
        let b = SceneBuilder::new(&mut scene, &cam);

        // A point 5e-7 document units off centre — 500 px away at this zoom.
        let far = Point::new(1e6 + 5e-7, 1e6 - 3e-7);
        let render_space = b.split.to_render_space(far);

        // After both CPU steps the coordinate is viewport-sized, not tiny and
        // not astronomical, so an f32 downcast is lossless in practice.
        //
        // Accuracy is bounded by f64 storage of the absolute document
        // coordinate, not by this transform, so check against the documented
        // precision budget rather than an arbitrary epsilon.
        let budget = cam.screen_precision_px();
        assert!(
            (render_space.x - 500.0).abs() < budget,
            "expected ~500 px within the {budget} px budget, got {render_space:?}"
        );
        assert!(
            render_space.x.abs() < 1e4 && render_space.y.abs() < 1e4,
            "render-space coords must stay viewport-sized, got {render_space:?}"
        );

        // The GPU transform must be unit scale — the magnification already
        // happened on the CPU in f64.
        let c = b.split.gpu_view.as_coeffs();
        let gpu_scale = (c[0] * c[0] + c[1] * c[1]).sqrt();
        assert!(
            (gpu_scale - 1.0).abs() < 1e-12,
            "GPU transform should be unit scale, was {gpu_scale}"
        );
        assert!(
            c[4].abs() <= 1920.0 && c[5].abs() <= 1080.0,
            "GPU translation must stay viewport-sized, got ({}, {})",
            c[4],
            c[5]
        );
    }

    /// The two CPU steps must not be fused; fusing restores the cancellation.
    #[test]
    fn fusing_the_cpu_steps_would_lose_precision() {
        let cam = Camera::new(Point::new(1e6, 1e6), 1e9, Size::new(1920.0, 1080.0));
        let split = cam.render_split();
        let p = Point::new(1e6 + 5e-7, 1e6 - 3e-7);

        let correct = split.to_render_space(p);

        // The tempting single matrix: scale ∘ translate(-anchor).
        let fused = Affine::scale(split.scale) * Affine::translate(-split.anchor.to_vec2());
        let wrong = fused * p;

        // In f64 the fused form is already visibly worse; in f32 it is fatal.
        let err = (wrong - correct).hypot();
        assert!(
            err > 1e-4,
            "expected the fused form to be measurably worse, error was {err}"
        );
    }

    /// The offset must move the picture without disturbing the scale, and
    /// without letting anything large reach the GPU transform.
    #[test]
    fn a_viewport_offset_shifts_output_without_changing_scale() {
        let mut scene = Scene::new();
        let cam = Camera::new(Point::new(100.0, 100.0), 4.0, Size::new(800.0, 600.0));

        let plain_scale = SceneBuilder::new(&mut scene, &cam).view_scale();
        let offset = buzz_geom::Vec2::new(64.0, 30.0);
        let shifted = SceneBuilder::new(&mut scene, &cam).with_viewport_offset(offset);

        assert_eq!(shifted.view_scale(), plain_scale, "scale must not change");

        let c = shifted.split.gpu_view.as_coeffs();
        let gpu_scale = (c[0] * c[0] + c[1] * c[1]).sqrt();
        assert!(
            (gpu_scale - 1.0).abs() < 1e-12,
            "the GPU transform must stay unit scale, was {gpu_scale}"
        );
        // Viewport centre (400, 300) plus the offset.
        assert!((c[4] - 464.0).abs() < 1e-9, "x translation was {}", c[4]);
        assert!((c[5] - 330.0).abs() < 1e-9, "y translation was {}", c[5]);
    }

    #[test]
    fn view_scale_tracks_zoom() {
        let mut scene = Scene::new();
        for zoom in [0.5, 1.0, 20.0, 1e6] {
            let cam = Camera::new(Point::ORIGIN, zoom, Size::new(800.0, 600.0));
            let b = SceneBuilder::new(&mut scene, &cam);
            assert!(
                (b.view_scale() - zoom).abs() < zoom * 1e-9,
                "view_scale {} should match zoom {zoom}",
                b.view_scale()
            );
        }
    }

    #[test]
    fn hairline_width_is_constant_on_screen() {
        let mut scene = Scene::new();
        let shape = Rect::new(0.0, 0.0, 10.0, 10.0);

        for zoom in [1.0, 100.0, 1e8] {
            let cam = Camera::new(Point::ORIGIN, zoom, Size::new(800.0, 600.0));
            let mut b = SceneBuilder::new(&mut scene, &cam);
            // 2 px on screen means 2/zoom in document units.
            let expected = 2.0 / zoom;
            let scale = b.view_scale();
            assert!((2.0 / scale - expected).abs() < expected * 1e-9);
            b.stroke_hairline(&shape, Color::WHITE, 2.0);
        }
    }

    /// Regression guard for the measured Phase 0 bug: a fixed document-space
    /// tolerance destroyed detail at deep zoom and wasted work at shallow zoom.
    #[test]
    fn flattening_tolerance_is_screen_relative_not_fixed() {
        let mut scene = Scene::new();

        for zoom in [1.0, 1e3, 1e6, 1e12] {
            let cam = Camera::new(Point::new(1024.0, 768.0), zoom, Size::new(512.0, 512.0));
            let b = SceneBuilder::new(&mut scene, &cam);
            let tol = b.tolerance();

            // Always the same error on screen, whatever the zoom.
            let on_screen = tol * b.view_scale();
            assert!(
                (on_screen - 0.1).abs() < 1e-9,
                "tolerance should be ~0.1 px on screen at zoom {zoom}, was {on_screen}"
            );
            assert!(
                tol > 0.0 && tol.is_finite(),
                "bad tolerance {tol} at {zoom}"
            );
        }

        // The specific failure: at 1e12x the artwork's smallest features are
        // ~3e-10 doc units. Tolerance must be far below that, not above it.
        let deep = Camera::new(Point::new(1024.0, 768.0), 1e12, Size::new(512.0, 512.0));
        let b = SceneBuilder::new(&mut scene, &deep);
        assert!(
            b.tolerance() < 3e-10 / 100.0,
            "tolerance {} would swallow the finest detail",
            b.tolerance()
        );
    }

    #[test]
    fn building_a_scene_at_extreme_zoom_produces_encoding() {
        let mut scene = Scene::new();
        let cam = Camera::new(Point::new(500.0, 500.0), 1e10, Size::new(1920.0, 1080.0));
        {
            let mut b = SceneBuilder::new(&mut scene, &cam);
            b.fill_shape(
                &Rect::new(500.0 - 1e-7, 500.0 - 1e-7, 500.0 + 1e-7, 500.0 + 1e-7),
                Color::WHITE,
            );
        }
        assert!(
            scene.encoding().n_paths > 0,
            "expected the fill to be encoded at 1e12% zoom"
        );
    }

    /// Phase 4a: a circle's own fill estimate should land in the ballpark of
    /// the textbook "how many chords to approximate a circle to a chord
    /// tolerance" formula — not exact, since the real walk goes through
    /// kurbo's cubic approximation of the circle and Wang's bound on each
    /// piece, but never wildly off it either.
    #[test]
    fn circle_fill_line_estimate_is_within_2x_of_the_textbook_arc_count() {
        let mut scene = Scene::new();
        let cam = Camera::new(Point::ORIGIN, 1.0, Size::new(800.0, 600.0));
        let mut b = SceneBuilder::new(&mut scene, &cam);
        let circle = buzz_geom::Circle::new(Point::new(400.0, 300.0), 100.0);
        b.fill_shape(&circle, Color::WHITE);

        let reference =
            (2.0 * std::f64::consts::PI / (2.0 * (1.0_f64 - 0.25 / 100.0).acos())).ceil();
        let lines = b.estimated_lines() as f64;
        assert!(
            lines >= reference && lines <= reference * 2.0,
            "expected within [1x, 2x] of the textbook {reference}, got {lines}"
        );
    }

    /// A straight-edged rectangle costs one line per drawn side, plus the
    /// closing line back to the start `PathEl::ClosePath` stands for — no
    /// curve subdivision to inflate it — before [`LINE_ESTIMATE_MARGIN`],
    /// the fitted safety margin `estimated_lines` multiplies every raw count
    /// by on the way out.
    #[test]
    fn straight_edged_rectangle_fill_estimate_is_four_times_the_margin() {
        let mut scene = Scene::new();
        let cam = Camera::new(Point::ORIGIN, 1.0, Size::new(800.0, 600.0));
        let mut b = SceneBuilder::new(&mut scene, &cam);
        b.fill_shape(&Rect::new(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        let raw_fill_lines = 4u64;
        let expected = (raw_fill_lines as f64 * LINE_ESTIMATE_MARGIN).ceil() as u64;
        assert_eq!(
            b.estimated_lines(),
            expected,
            "3 drawn sides plus the 1 the rectangle closes with, times the margin"
        );
    }

    /// A stroke flattens both offset sides of the same centreline, so it must
    /// cost at least twice what filling the identical shape would.
    #[test]
    fn stroke_line_estimate_is_at_least_twice_its_fill() {
        let cam = Camera::new(Point::ORIGIN, 1.0, Size::new(800.0, 600.0));
        let shape = Rect::new(0.0, 0.0, 40.0, 25.0);

        let mut fill_scene = Scene::new();
        let mut fb = SceneBuilder::new(&mut fill_scene, &cam);
        fb.fill_shape(&shape, Color::WHITE);
        let fill_lines = fb.estimated_lines();

        let mut stroke_scene = Scene::new();
        let mut sb = SceneBuilder::new(&mut stroke_scene, &cam);
        sb.stroke_shape(&shape, Color::WHITE, 2.0);
        let stroke_lines = sb.estimated_lines();

        assert!(
            stroke_lines >= fill_lines * 2,
            "stroke ({stroke_lines}) should be at least 2x the fill ({fill_lines})"
        );
    }
}
