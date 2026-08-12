//! Prove build-up paint on the real GPU: alpha 0.2 crossing alpha 0.3 gives
//! **0.5** in the overlap, not the 0.44 ordinary compositing produces.
//!
//! The arithmetic is already asserted in `buzz_scene::PaintBlend`. This is the
//! part that arithmetic cannot settle: whether the claim survives Vello's layer
//! encoding, wgpu, and an actual driver. Compositing bugs live precisely in
//! that gap — a blend mode that is right on paper and applied to the wrong
//! surface produces exactly this kind of silent, plausible-looking error.
//!
//! # Reading alpha out of an opaque frame
//!
//! The rendered target has no usable alpha channel of its own — it is composited
//! onto an opaque background before it is read back. So alpha is measured the
//! way the eye would: paint **black** onto a **white** stage and read the grey.
//! A pixel at `v` out of 255 covered by black at alpha `a` satisfies
//! `v = 255 x (1 - a)`, so `a = 1 - v/255`. That measures what the user
//! actually sees, which is the claim being made.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_geom::{Camera, Point, Rect, Size};
use buzz_render::{GpuContext, GpuPreference, SceneBuilder, wgpu};
use buzz_scene::{LayerKind, PaintBlend, Scene as Document, ShapeData};
use peniko::Color;
use vello::Scene;

const W: u32 = 256;
const H: u32 = 256;

/// White, so black paint reads directly as coverage.
const BACKGROUND: Color = Color::WHITE;

struct Harness {
    gpu: GpuContext,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

impl Harness {
    fn new() -> Option<Self> {
        let gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping build-up test: no usable GPU ({e})");
                return None;
            }
        };

        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("build-up-target"),
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

        // 256 * 4 = 1024, a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("build-up-readback"),
            size: (W * H * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Some(Self {
            gpu,
            texture,
            view,
            readback,
        })
    }

    fn render_and_read(&mut self, scene: &Scene) -> Vec<u8> {
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

        let pixels = {
            let view = slice.get_mapped_range();
            view.to_vec()
        };
        self.readback.unmap();
        pixels
    }
}

/// One GPU for the whole file.
///
/// Six tests each bringing up their own device in parallel is both wasteful
/// and unreliable: a context that fails to acquire makes its test *skip*, and
/// a test that silently skips is worse than one that fails.
fn with_harness(test: impl FnOnce(&mut Harness)) {
    static HARNESS: std::sync::OnceLock<Option<std::sync::Mutex<Harness>>> =
        std::sync::OnceLock::new();

    let shared = HARNESS.get_or_init(|| Harness::new().map(std::sync::Mutex::new));
    match shared {
        Some(mutex) => {
            let mut harness = mutex.lock().expect("the GPU harness is not poisoned");
            test(&mut harness);
        }
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// Effective alpha of black paint at a pixel, from its brightness.
fn alpha_of(pixel: &[u8]) -> f64 {
    1.0 - pixel[0] as f64 / 255.0
}

/// The distinct alpha levels covering a meaningful area of the frame.
///
/// Sampling named pixel coordinates would mean reproducing the camera's
/// document-to-screen mapping in the test, and getting that subtly wrong looks
/// exactly like a compositing bug. Reading the *histogram* instead asks the
/// question that is actually being claimed — "which opacities does this frame
/// contain?" — and cannot be thrown off by where the artwork landed.
///
/// Levels are rounded to 1/500 and any covering fewer than 200 pixels are
/// dropped, which discards anti-aliased edges without touching the regions.
fn alpha_levels(pixels: &[u8]) -> Vec<f64> {
    let mut counts: std::collections::BTreeMap<u64, usize> = std::collections::BTreeMap::new();
    for pixel in pixels.chunks_exact(4) {
        let alpha = alpha_of(pixel);
        let bucket = (alpha * 500.0).round() as u64;
        *counts.entry(bucket).or_default() += 1;
    }
    counts
        .into_iter()
        .filter(|(bucket, count)| *count >= 200 && *bucket > 0)
        .map(|(bucket, _)| bucket as f64 / 500.0)
        .collect()
}

/// Is `wanted` among the levels present, within one 8-bit step?
fn has_level(levels: &[f64], wanted: f64) -> bool {
    levels.iter().any(|l| (l - wanted).abs() <= 0.011)
}

/// Black ink at a given opacity.
fn ink(alpha: f64) -> Color {
    Color::from_rgba8(0, 0, 0, (alpha * 255.0).round() as u8)
}

/// A document with two overlapping black bars at the given alphas.
///
/// They cross in the middle, so one region has only the first, one only the
/// second, and one both — every case the mode has to get right, in one frame.
fn crossing_bars(alpha_a: f64, alpha_b: f64, blend: PaintBlend) -> Document {
    let mut document = Document::default();
    let layer = document.layers().iter().next().unwrap().id;

    for (rect, alpha) in [
        (Rect::new(20.0, 80.0, 180.0, 120.0), alpha_a),
        (Rect::new(80.0, 20.0, 120.0, 180.0), alpha_b),
    ] {
        document.add_shape(
            layer,
            ShapeData::filled(buzz_geom::Shape::to_path(&rect, 1e-9), ink(alpha))
                .with_blend(blend),
        );
    }
    document
}

/// Render a document through the same path the window uses.
fn render(harness: &mut Harness, document: &Document) -> Vec<u8> {
    let mut scene = Scene::new();
    let mut camera = Camera::new(
        Point::new(100.0, 100.0),
        1.0,
        Size::new(W as f64, H as f64),
    );
    // `margin` is a divisor: 1.0 is a tight fit.
    camera.fit_to_rect(Rect::new(0.0, 0.0, 200.0, 200.0), 1.0);

    {
        let mut builder = SceneBuilder::new(&mut scene, &camera);
        // The stage rectangle, so the paint has something to sit on — exactly
        // as `stage::build_scene` does.
        builder.fill_shape(&document.stage().stage_rect(), document.stage().background);
        buzz_app::stage::draw_document(&mut builder, document, 0);
    }

    harness.render_and_read(&scene)
}

/// The headline claim, measured in pixels the user would actually see.
#[test]
fn two_build_up_strokes_at_alpha_02_and_03_overlap_at_05() {
    with_harness(|harness| {

        let document = crossing_bars(0.2, 0.3, PaintBlend::Additive);
        let pixels = render(harness, &document);
        let levels = alpha_levels(&pixels);

        assert!(
            has_level(&levels, 0.2),
            "the first stroke alone should read 0.2; levels present: {levels:?}"
        );
        assert!(
            has_level(&levels, 0.3),
            "the second stroke alone should read 0.3; levels present: {levels:?}"
        );
        assert!(
            has_level(&levels, 0.5),
            "the overlap should build up to 0.5; levels present: {levels:?}"
        );
        assert!(
            !has_level(&levels, 0.44),
            "0.44 is the source-over answer and must not appear: {levels:?}"
        );
    });
}

/// The same document without build-up, to show the modes genuinely differ and
/// that the default is unchanged.
#[test]
fn without_build_up_the_same_strokes_overlap_at_044() {
    with_harness(|harness| {

        let document = crossing_bars(0.2, 0.3, PaintBlend::Normal);
        let pixels = render(harness, &document);
        let levels = alpha_levels(&pixels);

        assert!(
            has_level(&levels, 0.44),
            "ordinary compositing gives 0.2 + 0.3x0.8 = 0.44; levels: {levels:?}"
        );
        assert!(
            !has_level(&levels, 0.5),
            "and must not reach the additive 0.5: {levels:?}"
        );
    });
}

/// The defect the isolation group exists to prevent.
///
/// Additive compositing sums with the destination. Applied straight to the
/// canvas, black paint on a white stage would sum to white and the stroke would
/// vanish — the mode would look like it did nothing. This asserts the paint is
/// actually there.
#[test]
fn build_up_paint_does_not_dissolve_into_a_light_background() {
    with_harness(|harness| {

        let document = crossing_bars(0.25, 0.25, PaintBlend::Additive);
        let pixels = render(harness, &document);
        let levels = alpha_levels(&pixels);

        assert!(
            has_level(&levels, 0.25),
            "a single build-up stroke must still be visible on a white stage: {levels:?}"
        );
        assert!(
            has_level(&levels, 0.5),
            "and two of them must be visibly darker still: {levels:?}"
        );
    });
}

/// Build-up must not leak past the shapes that use it: a normal shape on the
/// same layer keeps composing normally.
#[test]
fn a_normal_shape_on_a_build_up_layer_still_composites_normally() {
    with_harness(|harness| {

        let mut document = Document::default();
        let layer = document.layers().iter().next().unwrap().id;

        // Two normal shapes overlapping, on a layer that also holds build-up paint
        // elsewhere — so the layer is isolated, but these two must be unaffected.
        for rect in [
            Rect::new(20.0, 80.0, 180.0, 120.0),
            Rect::new(80.0, 20.0, 120.0, 180.0),
        ] {
            document.add_shape(
                layer,
                ShapeData::filled(buzz_geom::Shape::to_path(&rect, 1e-9), ink(0.2))
                    .with_blend(PaintBlend::Normal),
            );
        }
        // Somewhere else entirely, so it cannot affect the sampled pixels.
        document.add_shape(
            layer,
            ShapeData::filled(
                buzz_geom::Shape::to_path(&Rect::new(0.0, 190.0, 10.0, 200.0), 1e-9),
                ink(0.3),
            )
            .with_blend(PaintBlend::Additive),
        );

        let pixels = render(harness, &document);


        let levels = alpha_levels(&pixels);
        assert!(
            has_level(&levels, 0.36),
            "two normal shapes at 0.2 give 0.2 + 0.2x0.8 = 0.36 even inside an \
             isolated layer; levels: {levels:?}"
        );
        assert!(
            !has_level(&levels, 0.4),
            "they must not have been swept into the additive path: {levels:?}"
        );
    });
}

/// Build-up on one layer must not accumulate with a different layer: layers
/// composite onto each other normally, which is what keeps them independent.
#[test]
fn build_up_does_not_cross_between_layers() {
    with_harness(|harness| {

        let mut document = Document::default();
        let lower = document.layers().iter().next().unwrap().id;
        let upper = document.add_layer("Upper", LayerKind::Normal);

        for (layer, rect) in [
            (lower, Rect::new(20.0, 80.0, 180.0, 120.0)),
            (upper, Rect::new(80.0, 20.0, 120.0, 180.0)),
        ] {
            document.add_shape(
                layer,
                ShapeData::filled(buzz_geom::Shape::to_path(&rect, 1e-9), ink(0.2))
                    .with_blend(PaintBlend::Additive),
            );
        }

        let pixels = render(harness, &document);


        let levels = alpha_levels(&pixels);
        assert!(
            has_level(&levels, 0.36),
            "across layers the two 0.2 strokes composite normally to 0.36; \
             levels: {levels:?}"
        );
        assert!(
            !has_level(&levels, 0.4),
            "build-up must not reach across a layer boundary: {levels:?}"
        );
    });
}

/// A whole drawing's worth of build-up paint must still render in a frame.
/// Every additive shape costs a Vello layer, so this is the cost that had to be
/// checked rather than assumed.
#[test]
fn many_build_up_strokes_still_render_quickly() {
    with_harness(|harness| {

        let mut document = Document::default();
        let layer = document.layers().iter().next().unwrap().id;
        let ink = Color::from_rgba8(0, 0, 0, 26); // ~0.1

        for i in 0..300 {
            let y = (i % 60) as f64 * 3.0;
            let x = (i / 60) as f64 * 8.0;
            document.add_shape(
                layer,
                ShapeData::filled(
                    buzz_geom::Shape::to_path(&Rect::new(x, y, x + 120.0, y + 2.5), 1e-9),
                    ink,
                )
                .with_blend(PaintBlend::Additive),
            );
        }

        let started = std::time::Instant::now();
        let pixels = render(harness, &document);
        let elapsed = started.elapsed();

        assert!(
            !alpha_levels(&pixels).is_empty(),
            "the test should have drawn something to time"
        );

        assert!(
            elapsed.as_millis() < 2_000,
            "300 build-up shapes took {elapsed:?} to render and read back"
        );
        eprintln!("300 build-up shapes rendered and read back in {elapsed:?}");
    });
}
