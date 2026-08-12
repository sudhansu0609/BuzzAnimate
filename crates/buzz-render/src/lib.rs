//! GPU device setup and Vello-backed vector rendering.
//!
//! Two responsibilities:
//!
//! * [`adapter`] — picking the right GPU, which is genuinely non-trivial on a
//!   workstation with virtual display drivers installed.
//! * [`GpuContext`] — the device, queue and Vello renderer, plus the scene
//!   building that honours the rebasing contract from `buzz-geom`.

pub mod adapter;

use anyhow::{Context, Result};
use buzz_geom::{Affine, BezPath, Camera, RenderClip, RenderSplit, Shape};
use peniko::{Color, Fill};
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};
use wgpu::{Device, Instance, Queue, TextureFormat, TextureView};

pub use adapter::{GpuPreference, Selection, SelectionError};
// Re-export the wgpu that vello uses, so downstream crates cannot accidentally
// link a second, incompatible copy.
pub use vello::wgpu;

/// Everything needed to rasterise a scene on the GPU.
pub struct GpuContext {
    pub instance: Instance,
    pub device: Device,
    pub queue: Queue,
    pub renderer: Renderer,
    pub selection: Selection,
}

impl GpuContext {
    /// Bring up the GPU, honouring the user's adapter preference.
    pub async fn new(preference: &GpuPreference) -> Result<Self> {
        let instance = Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let selection = adapter::select(&instance, preference)
            .await
            .context("selecting a GPU adapter")?;

        tracing::info!("\n{}", selection.report());

        let (device, queue) = selection
            .adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("buzz-device"),
                // Vello's compute pipelines run within downlevel defaults; not
                // demanding more keeps older drivers usable.
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .context("requesting a GPU device")?;

        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                // Area anti-aliasing only: it is the fastest mode and is exact
                // for the non-self-intersecting fills that dominate vector
                // artwork. Compiling the MSAA permutations too would slow
                // startup for pipelines we never dispatch.
                antialiasing_support: AaSupportArea::SUPPORT,
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .map_err(|e| anyhow::anyhow!("creating the Vello renderer: {e:?}"))?;

        Ok(Self {
            instance,
            device,
            queue,
            renderer,
            selection,
        })
    }

    /// Blocking convenience wrapper for non-async callers.
    pub fn new_blocking(preference: &GpuPreference) -> Result<Self> {
        pollster::block_on(Self::new(preference))
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
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| anyhow::anyhow!("vello render failed: {e:?}"))
    }
}

/// Vello wants an `AaSupport`; this names the single configuration we compile.
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
/// precision collapse that caps Animate at 2000%. See `buzz_geom::camera`.
pub struct SceneBuilder<'a> {
    scene: &'a mut Scene,
    split: RenderSplit,
    clip: RenderClip,
}

impl<'a> SceneBuilder<'a> {
    /// Begin building against `camera`. Resets the scene.
    pub fn new(scene: &'a mut Scene, camera: &Camera) -> Self {
        scene.reset();
        Self {
            scene,
            split: camera.render_split(),
            clip: RenderClip::new(camera.visible_doc_rect()),
        }
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
    fn to_render_space(&self, shape: &impl Shape) -> BezPath {
        // Cheap even at extreme tolerance: kurbo grows a circle's segment
        // count as the sixth root of radius/tolerance, so a 300-unit circle at
        // a 5e-12 tolerance is ~200 cubics, not millions.
        let path = shape.to_path(self.tolerance());
        let clipped = self.clip.apply(&path);
        let centred = Affine::translate(-self.split.anchor.to_vec2()) * clipped;
        Affine::scale(self.split.scale) * centred
    }

    /// The region within which geometry is preserved exactly.
    pub fn clip_bounds(&self) -> buzz_geom::Rect {
        self.clip.bounds()
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
    pub fn with_viewport_offset(mut self, offset: buzz_geom::Vec2) -> Self {
        self.split.gpu_view = Affine::translate(offset) * self.split.gpu_view;
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
        self.scene
            .fill(Fill::NonZero, self.split.gpu_view, color, None, &path);
    }

    /// Stroke a document-space shape with a width in document units.
    pub fn stroke_shape(&mut self, shape: &impl Shape, color: Color, width: f64) {
        let path = self.to_render_space(shape);
        // Geometry is already magnified, and `gpu_view` has unit scale, so the
        // stroke width must be magnified to match.
        let render_width = self.split.scale_length(width).max(f64::MIN_POSITIVE);
        self.scene.stroke(
            &kurbo::Stroke::new(render_width),
            self.split.gpu_view,
            color,
            None,
            &path,
        );
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Point, Rect, Size};

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
            assert!(tol > 0.0 && tol.is_finite(), "bad tolerance {tol} at {zoom}");
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
}
