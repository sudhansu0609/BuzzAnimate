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
pub mod document;
pub mod filters;
pub mod lighting;

use anyhow::{Context, Result};
use buzz_geom::{Affine, BezPath, Camera, RenderClip, RenderSplit, Shape};
use buzz_scene::{GradientKind, GradientSpread, Paint};
use peniko::{Color, Fill};
use std::sync::Arc;
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};
use wgpu::{Device, Instance, Queue, TextureFormat, TextureView};

pub use adapter::{GpuPreference, Selection, SelectionError};
// Re-export the wgpu that vello uses, so downstream crates cannot accidentally
// link a second, incompatible copy. Vello itself goes with it, for the same
// reason: the exporter builds scenes and must build *these* scenes.
pub use vello;
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
const SEAM_SEAL_PX: f64 = 0.9;

/// Is this paint fully opaque everywhere?
///
/// A gradient has to be checked stop by stop: one transparent stop is enough to
/// make sealing it draw a dark rim, and the average would hide that.
fn is_opaque(paint: &Paint) -> bool {
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
            let brush = peniko::ImageBrush {
                image: data,
                sampler: peniko::ImageSampler {
                    x_extend: peniko::Extend::Pad,
                    y_extend: peniko::Extend::Pad,
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
        self.scene.fill(
            Fill::NonZero,
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
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
        self.fill_shape_paint(shape, paint, to_doc);
        if !is_opaque(paint) {
            return;
        }
        // In screen pixels: the seam is a property of the *rasteriser*, not of
        // the document, so it is the same width at every zoom. Slightly under a
        // pixel — a full pixel of overlap is enough to close the gap, and less
        // silhouette is disturbed than a wider stroke would disturb.
        let width = SEAM_SEAL_PX / self.view_scale().max(f64::MIN_POSITIVE);
        self.stroke_shape_paint(shape, paint, width, to_doc);
    }

    /// Stroke a document-space shape with a paint, which may be a gradient.
    pub fn stroke_shape_paint(
        &mut self,
        shape: &impl Shape,
        paint: &Paint,
        width: f64,
        to_doc: Affine,
    ) {
        let path = self.to_render_space(shape);
        let render_width = self.split.scale_length(width).max(f64::MIN_POSITIVE);
        let (brush, brush_transform) = self.brush_for(paint, to_doc);
        self.scene.stroke(
            &kurbo::Stroke::new(render_width),
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
    }

    /// Fill additively with a paint. See [`Self::fill_shape_additive`], which
    /// this is the gradient-capable form of.
    pub fn fill_shape_paint_additive(&mut self, shape: &impl Shape, paint: &Paint, to_doc: Affine) {
        let path = self.to_render_space(shape);
        let (brush, brush_transform) = self.brush_for(paint, to_doc);
        let blend = peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::Plus);
        self.scene
            .push_layer(Fill::NonZero, blend, 1.0, self.split.gpu_view, &path);
        self.scene.fill(
            Fill::NonZero,
            self.split.gpu_view,
            &brush,
            brush_transform,
            &path,
        );
        self.scene.pop_layer();
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
        self.scene
            .push_layer(Fill::NonZero, blend, 1.0, self.split.gpu_view, &path);
        self.scene
            .fill(Fill::NonZero, self.split.gpu_view, color, None, &path);
        self.scene.pop_layer();
    }

    /// Fill a shape with the **even-odd** rule.
    ///
    /// What a ring is drawn with: an outer boundary and an inner one in the
    /// same path, with the hole between them. Non-zero would fill the lot.
    pub fn fill_shape_even_odd(&mut self, shape: &impl Shape, color: Color) {
        let path = self.to_render_space(shape);
        self.scene
            .fill(Fill::EvenOdd, self.split.gpu_view, color, None, &path);
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
        let path = self.to_render_space(shape);
        let render_width = self.split.scale_length(width).max(f64::MIN_POSITIVE);

        // Render space is the document scaled about an anchor, so a transform
        // meant for document space has to be conjugated into it: shift to the
        // anchor, scale, apply, and undo. For the scale-only transforms this
        // is used with, that reduces to the transform itself.
        self.scene.stroke(
            &kurbo::Stroke::new(render_width),
            self.split.gpu_view * transform,
            color,
            None,
            &(transform.inverse() * path),
        );
    }

    /// Begin a group that composites with what is behind it using `blend`.
    ///
    /// Animate's blend modes need a backdrop to blend *with*, and without a
    /// group that backdrop is the entire stage — so every mode but Normal is
    /// drawn into one of these.
    pub fn push_blend(&mut self, bounds: buzz_geom::Rect, blend: buzz_fx::Blend) {
        let path = self.to_render_space(&bounds);
        self.scene.push_layer(
            Fill::NonZero,
            blend_mode(blend),
            1.0,
            self.split.gpu_view,
            &path,
        );
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
        self.scene.push_layer(
            Fill::NonZero,
            peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
            1.0,
            self.split.gpu_view,
            &path,
        );
    }

    /// Close the group opened by [`Self::push_isolation`].
    pub fn pop_isolation(&mut self) {
        self.scene.pop_layer();
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
        self.scene.push_layer(
            Fill::NonZero,
            peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
            1.0,
            self.split.gpu_view,
            &path,
        );
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
        self.scene
            .push_layer(Fill::NonZero, punch, 1.0, self.split.gpu_view, &path);
        self.scene.fill(
            Fill::NonZero,
            self.split.gpu_view,
            // Any opaque colour: `DestOut` reads the source's alpha only.
            Color::BLACK,
            None,
            &path,
        );
        self.scene.pop_layer();

        self.pop_isolation();
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
}
