//! Prove layer and object opacity on the real GPU, through the stage's own
//! frame options: a layer dimmed to 50%, an instance with an Alpha effect, a
//! **bitmap** on a dimmed layer (which once stayed solid, because a picture's
//! pixels never reach `map_colors`), opacity changing under a warm cache, and
//! a symbol opened in place showing its own colours, not the effect on the
//! instance it came from.
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

use buzz_geom::{Camera, Rect, Size};
use buzz_render::{GpuContext, GpuPreference, SceneBuilder, wgpu};
use buzz_scene::{Scene as Document, ShapeData, SymbolKind, Object, ObjectId, ObjectKind, ColorTransform};
use buzz_render::document::{FrameOptions, DrawCache, MaskDisplay};
use std::sync::Arc;
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
                eprintln!("skipping opacity test: no usable GPU ({e})");
                return None;
            }
        };

        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("opacity-target"),
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
            label: Some("opacity-readback"),
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
            // A panicking test must not take the rest down with it: the GPU
            // context is still perfectly usable, so the poison is recovered
            // rather than propagated as six more failures that hide the one
            // real fault.
            let mut harness = mutex.lock().unwrap_or_else(|e| e.into_inner());
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

/// Render a document through the same path the window uses.
fn render_opts(harness: &mut Harness, document: &Document, on_stage: bool) -> Vec<u8> {
    let mut scene = Scene::new();
    // Frame the document's own stage, so a test can place artwork anywhere on
    // it without also having to know what this function decided to look at.
    let stage = document.stage().stage_rect();
    let mut camera = Camera::new(stage.center(), 1.0, Size::new(W as f64, H as f64));
    // `margin` is a divisor: 1.0 is a tight fit.
    camera.fit_to_rect(stage, 1.0);

    {
        let mut builder = SceneBuilder::new(&mut scene, &camera);
        // The stage rectangle, so the paint has something to sit on — exactly
        // as `stage::build_scene` does.
        builder.fill_shape(&document.stage().stage_rect(), document.stage().background);
        if on_stage {
            let live = FrameOptions { masks: MaskDisplay::WhenLocked, lit: true, layer_alpha: true, place: document.edit_place(), cull: Some(document.stage().stage_rect()), ..FrameOptions::default() };
            let mut cache = DrawCache::new();
            cache.begin();
            buzz_render::document::draw_frame_within(&mut builder, document, 0, document.camera_transform(0), &live, &mut cache);
        } else {
            buzz_render::document::draw_document(&mut builder, document, 0);
        }
    }

    harness.render_and_read(&scene)
}


#[test]
fn layer_alpha_on_stage() {
    with_harness(|h| {
        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        d.add_shape(layer, ShapeData::filled(buzz_geom::Shape::to_path(&Rect::new(20.0,20.0,180.0,180.0),1e-9), ink(1.0)));
        d.update_layer(layer, |l| l.alpha = 0.5);
        let levels = alpha_levels(&render_opts(h, &d, true));
        eprintln!("layer levels {levels:?}");
        assert!(has_level(&levels, 0.5), "{levels:?}");
    });
}

#[test]
fn instance_alpha() {
    with_harness(|h| {
        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        let sym = d.add_symbol("s", SymbolKind::Graphic, None);
        let sl = d.library().get(sym).unwrap().layers.iter().next().unwrap().id;
        d.library_mut().update(sym, |s| { s.layers.update(sl, |l| {
            l.frames.push_object(0, Arc::new(Object::shape(ObjectId(9999), ShapeData::filled(buzz_geom::Shape::to_path(&Rect::new(20.0,20.0,180.0,180.0),1e-9), ink(1.0)))));
        }); });
        let id = d.add_instance_at(layer, 0, sym, kurbo::Affine::IDENTITY).unwrap();
        d.update_object(id, |o| if let ObjectKind::Instance(i) = &mut o.kind { i.color = ColorTransform::alpha(0.5); });
        for stage in [false, true] {
            let levels = alpha_levels(&render_opts(h, &d, stage));
            eprintln!("instance stage={stage} levels {levels:?}");
            assert!(has_level(&levels, 0.5), "stage={stage} {levels:?}");
        }
    });
}

fn render_cached(h: &mut Harness, d: &Document, cache: &mut DrawCache) -> Vec<f64> {
    alpha_levels(&render_pixels_cached(h, d, cache))
}

/// The stage's own path, through a cache the caller keeps across frames.
fn render_pixels_cached(h: &mut Harness, d: &Document, cache: &mut DrawCache) -> Vec<u8> {
    let mut scene = Scene::new();
    let stage = d.stage().stage_rect();
    let mut camera = Camera::new(stage.center(), 1.0, Size::new(W as f64, H as f64));
    camera.fit_to_rect(stage, 1.0);
    {
        let mut builder = SceneBuilder::new(&mut scene, &camera);
        builder.fill_shape(&stage, d.stage().background);
        let live = FrameOptions { masks: MaskDisplay::WhenLocked, lit: true, layer_alpha: true, place: d.edit_place(), cull: Some(stage), ..FrameOptions::default() };
        cache.begin();
        buzz_render::document::draw_frame_within(&mut builder, d, 0, d.camera_transform(0), &live, cache);
        cache.end();
    }
    h.render_and_read(&scene)
}

#[test]
fn alpha_changes_with_persistent_cache() {
    with_harness(|h| {
        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        let sym = d.add_symbol("s", SymbolKind::Graphic, None);
        let sl = d.library().get(sym).unwrap().layers.iter().next().unwrap().id;
        d.library_mut().update(sym, |s| { s.layers.update(sl, |l| {
            l.frames.push_object(0, Arc::new(Object::shape(ObjectId(9999), ShapeData::filled(buzz_geom::Shape::to_path(&Rect::new(20.0,20.0,180.0,180.0),1e-9), ink(1.0)))));
        }); });
        let id = d.add_instance_at(layer, 0, sym, kurbo::Affine::IDENTITY).unwrap();
        let mut cache = DrawCache::new();
        for _ in 0..4 { eprintln!("opaque {:?}", render_cached(h, &d, &mut cache)); }
        d.update_layer(layer, |l| l.alpha = 0.5);
        for _ in 0..3 { eprintln!("layer 0.5 {:?}", render_cached(h, &d, &mut cache)); }
        d.update_layer(layer, |l| l.alpha = 1.0);
        for _ in 0..3 { eprintln!("layer back 1 {:?}", render_cached(h, &d, &mut cache)); }
        d.update_object(id, |o| if let ObjectKind::Instance(i) = &mut o.kind { i.color = ColorTransform::alpha(0.5); });
        for _ in 0..3 { eprintln!("instance 0.5 {:?}", render_cached(h, &d, &mut cache)); }
    });
}

#[test]
fn image_on_dimmed_layer() {
    with_harness(|h| {
        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        let pixels: Vec<u8> = std::iter::repeat([0u8, 0, 0, 255]).take(16 * 16).flatten().collect();
        let asset = Arc::new(buzz_scene::ImageAsset::from_pixels(buzz_scene::ImageId(77), "black", 16, 16, Arc::new(pixels)));
        let rect = Rect::new(20.0, 20.0, 180.0, 180.0);
        let mut shape = ShapeData::filled(buzz_geom::Shape::to_path(&rect, 1e-9), ink(1.0));
        shape.fill.as_mut().unwrap().paint = buzz_scene::Paint::Image(Box::new(buzz_scene::ImageFill::new(asset, rect)));
        d.add_shape(layer, shape);
        eprintln!("image opaque {:?}", alpha_levels(&render_opts(h, &d, true)));
        d.update_layer(layer, |l| l.alpha = 0.5);
        let levels = alpha_levels(&render_opts(h, &d, true));
        eprintln!("image layer 0.5 {levels:?}");
        assert!(has_level(&levels, 0.5), "{levels:?}");
    });
}

#[test]
fn edit_in_place_shows_original_colours_and_all_frames() {
    with_harness(|h| {
        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        let sym = d.add_symbol("s", SymbolKind::Graphic, None);
        let sl = d.library().get(sym).unwrap().layers.iter().next().unwrap().id;
        d.library_mut().update(sym, |s| { s.layers.update(sl, |l| {
            for f in [0u32, 10, 20] {
                if f > 0 { l.frames.insert_blank_keyframe(f); }
                l.frames.push_object(f, Arc::new(Object::shape(ObjectId(9000 + f as u64), ShapeData::filled(buzz_geom::Shape::to_path(&Rect::new(20.0,20.0,180.0,180.0),1e-9), ink(1.0)))));
            }
        }); });
        let symbol_len = d.library().get(sym).unwrap().length();
        let id = d.add_instance_at(layer, 0, sym, kurbo::Affine::IDENTITY).unwrap();
        d.update_object(id, |o| if let ObjectKind::Instance(i) = &mut o.kind { i.color = ColorTransform::alpha(0.5); });
        let origin = buzz_scene::EditOrigin { layer, keyframe_start: 0, object: id };
        assert!(d.enter_symbol_in_place_from(sym, kurbo::Affine::IDENTITY, Some(origin)));
        eprintln!("symbol length {symbol_len}, frame_count inside {}", d.frame_count());
        assert_eq!(d.frame_count(), symbol_len);
        let mut scene = Scene::new();
        let stage = d.stage().stage_rect();
        let mut camera = Camera::new(stage.center(), 1.0, Size::new(W as f64, H as f64));
        camera.fit_to_rect(stage, 1.0);
        {
            let mut builder = SceneBuilder::new(&mut scene, &camera);
            builder.fill_shape(&stage, d.stage().background);
            let live = FrameOptions { layer_alpha: true, place: d.edit_place(), cull: Some(stage), ..FrameOptions::default() };
            let mut cache = DrawCache::new();
            cache.begin();
            buzz_render::document::draw_frame_within(&mut builder, &d, 15, d.camera_transform(15), &live, &mut cache);
            cache.end();
        }
        let levels = alpha_levels(&h.render_and_read(&scene));
        eprintln!("inside levels {levels:?}");
        // The instance is at 50% outside; opened, the symbol shows its own
        // solid colours, as Animate does.
        assert!(has_level(&levels, 1.0), "{levels:?}");
        assert!(!has_level(&levels, 0.5), "{levels:?}");
    });
}

/// How much of the frame is solid black: a filled square covers several
/// per cent of it, its outline a sliver.
fn solid_share(pixels: &[u8]) -> f64 {
    let solid = pixels.chunks_exact(4).filter(|p| alpha_of(p) > 0.9).count();
    solid as f64 / (pixels.len() / 4) as f64
}

/// **Outline view** (the third timeline column) draws every kind of artwork
/// on the layer as an outline: a shape, a symbol instance, and a bitmap fill.
#[test]
fn outline_layers_draw_outlines_for_every_kind_of_artwork() {
    with_harness(|h| {
        let rect = Rect::new(20.0, 20.0, 180.0, 180.0);
        let square = || ShapeData::filled(buzz_geom::Shape::to_path(&rect, 1e-9), ink(1.0));
        let mut cases: Vec<(&str, Document)> = Vec::new();

        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        d.add_shape(layer, square());
        cases.push(("shape", d));

        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        let sym = d.add_symbol("s", SymbolKind::Graphic, None);
        let sl = d.library().get(sym).unwrap().layers.iter().next().unwrap().id;
        d.library_mut().update(sym, |s| { s.layers.update(sl, |l| {
            l.frames.push_object(0, Arc::new(Object::shape(ObjectId(9100), square())));
        }); });
        d.add_instance_at(layer, 0, sym, kurbo::Affine::IDENTITY).unwrap();
        cases.push(("instance", d));

        let mut d = Document::default();
        let layer = d.layers().iter().next().unwrap().id;
        let pixels: Vec<u8> = std::iter::repeat([0u8, 0, 0, 255]).take(16 * 16).flatten().collect();
        let asset = Arc::new(buzz_scene::ImageAsset::from_pixels(buzz_scene::ImageId(78), "black", 16, 16, Arc::new(pixels)));
        let mut shape = square();
        shape.fill.as_mut().unwrap().paint = buzz_scene::Paint::Image(Box::new(buzz_scene::ImageFill::new(asset, rect)));
        d.add_shape(layer, shape);
        cases.push(("bitmap", d));

        for (name, mut d) in cases {
            // Through one cache kept across frames, as the stage keeps it, so
            // a cached encoding of the filled artwork cannot hide the switch.
            let mut cache = DrawCache::new();
            let mut before = 0.0;
            for _ in 0..4 {
                before = solid_share(&render_pixels_cached(h, &d, &mut cache));
            }
            let layer = d.layers().iter().next().unwrap().id;
            d.update_layer(layer, |l| l.outline = true);
            let mut after = 0.0;
            for _ in 0..3 {
                after = solid_share(&render_pixels_cached(h, &d, &mut cache));
            }
            eprintln!("{name}: solid {before:.3} filled, {after:.3} outlined");
            assert!(before > 0.05, "{name}: the filled artwork shows ({before:.3})");
            assert!(after < before / 4.0, "{name}: outlined, the artwork is no longer filled ({after:.3})");
        }
    });
}
