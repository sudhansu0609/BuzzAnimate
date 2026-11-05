//! scratch probe (TRACE_PICTURE_PLAN T14) — lamp + sun on a big traced picture,
//! through the headless Editor: UI encode ms idle / while dragging the lamp,
//! exporter render ms cold / warm, at 100 % and 400 %, optionally after
//! Optimize Shapes. Usage: zz_light_probe <file.buzz> <out dir> [optimize] [nolight]
use buzz_geom::{Rect, Size};
use buzz_render::document::DrawCache;
use buzz_ui::command::Command;
use buzz_render::{GpuContext, GpuPreference, wgpu};
use image::RgbaImage;
use std::time::Instant;

fn render(ex: &mut buzz_export::Exporter, scene: &buzz_scene::Scene, region: Rect, scale: f64) -> RgbaImage {
    let s = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent: false,
        region: Some(region),
    };
    let f = ex.render(scene, 0, &s).unwrap();
    RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
}

fn psnr(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let (mut se, mut n) = (0.0f64, 0.0f64);
    for (p, q) in a.pixels().zip(b.pixels()) {
        for c in 0..3 {
            let d = p[c] as f64 - q[c] as f64;
            se += d * d;
            n += 1.0;
        }
    }
    let mse = se / n.max(1.0);
    if mse <= 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn gpu_ms(ctx: &mut GpuContext, view: &wgpu::TextureView, scene: &buzz_render::vello::Scene) -> f64 {
    let t = Instant::now();
    ctx.render(scene, view, 1600, 900, peniko::Color::from_rgb8(0x2A, 0x2A, 0x2E)).unwrap();
    ctx.device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) })
        .unwrap();
    ms(t)
}

fn read_back(ctx: &GpuContext, tex: &wgpu::Texture) -> RgbaImage {
    let (w, h) = (1600u32, 900u32);
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (w * h * 4) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
    let px = slice.get_mapped_range().to_vec();
    RgbaImage::from_raw(w, h, px).unwrap()
}

fn scene_bytes(scene: &buzz_render::vello::Scene) -> usize {
    let e = scene.encoding();
    std::mem::size_of_val(e.path_tags.as_slice())
        + std::mem::size_of_val(e.path_data.as_slice())
        + std::mem::size_of_val(e.draw_tags.as_slice())
        + std::mem::size_of_val(e.draw_data.as_slice())
}

fn build_parallel(misses: Vec<buzz_render::lighting::Miss>) -> Vec<buzz_render::lighting::Built> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let queue = std::sync::Mutex::new(misses);
    let out = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| loop {
                let m = queue.lock().unwrap().pop();
                let Some(m) = m else { break };
                let b = m.build();
                out.lock().unwrap().push(b);
            });
        }
    });
    out.into_inner().unwrap()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out).unwrap();
    let optimize = a.iter().any(|s| s == "optimize");
    let nolight = a.iter().any(|s| s == "nolight");
    let doc = buzz_doc::Document::open(&a[0]).unwrap();
    let mut e = buzz_app::editor::Editor::new(doc);
    e.camera.viewport = Size::new(1600.0, 900.0);
    let stage = e.scene().stage().stage_rect();
    e.camera.fit_to_rect(stage, 1.0);
    if optimize {
        let s = buzz_app::rigging::OptimizeSettings { smoothing: 0.5, merge_same_colour: true, remove_tiny: 1.0 };
        e.optimize_shapes_now(None, s).expect("optimized");
    }
    let region = e.scene().content_bounds().unwrap_or(stage);
    println!("region {:?} zoom {:.0}%", region, e.camera.zoom_percent());
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();

    if !nolight {
        let which = std::env::var("LIGHTS").unwrap_or_else(|_| "both".into());
        if which != "sun" {
            e.run(Command::AddLamp);
        }
        if which != "lamp" {
            e.run(Command::AddSun);
        }
        if std::env::var("SHADOWS").is_ok_and(|v| v == "0") {
            e.doc.edit("Shadows", |scene| {
                for l in scene.lights_mut().lights.iter_mut() {
                    l.shadows = false;
                }
            });
        }
    }
    for l in &e.scene().lights().lights {
        println!("light {:?} shadows={} {:?}", l.id, l.shadows, l.kind);
    }
    let lamp = e.scene().lights().lights.iter().find(|l| matches!(l.kind, buzz_scene::LightKind::Lamp { .. })).map(|l| l.id);

    // Exporter render (includes lighting), cold then warm.
    let t = Instant::now();
    let img = render(&mut ex, e.scene(), region, 1.0);
    let cold = ms(t);
    let t = Instant::now();
    for _ in 0..5 {
        let _ = render(&mut ex, e.scene(), region, 1.0);
    }
    let warm = ms(t) / 5.0;
    let img2 = render(&mut ex, e.scene(), region, 1.0);
    println!("RENDER cold {cold:.1} ms warm {warm:.1} ms, serial-vs-serial PSNR {:.2}", psnr(&img, &img2));
    if let Ok(p) = std::env::var("CMP") {
        let r = image::open(&p).unwrap().to_rgba8();
        println!("PSNR vs {p}: {:.2} dB", psnr(&img, &r));
    }
    img.save(out.join(format!("render_{}.png", if nolight { "unlit" } else { "lit" }))).unwrap();

    let mut gctx = GpuContext::new_blocking(&GpuPreference::Automatic).unwrap();
    let target = gctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("t"),
        size: wgpu::Extent3d { width: 1600, height: 900, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: buzz_render::RENDER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    for zoom_factor in [1.0f64, 4.0] {
        e.camera.fit_to_rect(stage, 1.0);
        if zoom_factor > 1.0 {
            let c = e.camera.center;
            e.camera.zoom_by_at(zoom_factor, c);
        }
        let area = Rect::new(0.0, 0.0, 1600.0, 900.0);
        let scale = 1.0;
        let mut vello = buzz_render::vello::Scene::new();
        let mut cache = DrawCache::default();
        cache.set_scale(gctx.buffer_scale);
        cache.set_max_scale(gctx.max_buffer_scale);
        match std::env::var("PIN").as_deref() {
            Ok("nomodel") => cache.pin_detail(Some(buzz_render::document::LightDetail::NoModelling)),
            Ok("flat") => cache.pin_detail(Some(buzz_render::document::LightDetail::Flat)),
            _ => {}
        }
        // The app's own cold start: nothing built on the frame thread, the
        // misses queued and built by a worker batch, installed, re-encoded.
        cache.lights.set_inline_budget(std::time::Duration::ZERO);
        cache.lights.set_queue(true);
        let report = |cache: &DrawCache| format!("detail {:?} encode {} lines {} ceiling {} scale {}", cache.detail(), cache.last_encode(), cache.last_lines(), cache.last_ceiling(), cache.scale());
        for round in 0..4 {
            let t = Instant::now();
            buzz_app::stage::build_scene(&mut vello, &e, area, scale, &mut cache);
            let enc = ms(t);
            gctx.grow_to(cache.scale());
            let misses = cache.lights.take_misses();
            let n = misses.len();
            let t = Instant::now();
            let built = build_parallel(misses);
            let bms = ms(t);
            cache.lights.install(built);
            println!("zoom {:.0}% round {round}: encode {enc:.1} ms, {n} misses built in {bms:.0} ms; {}", e.camera.zoom_percent(), report(&cache));
            if n == 0 {
                break;
            }
        }
        // idle: warm encodes (worst case: re-encode each frame), then settled.
        let mut v = Vec::new();
        for _ in 0..8 {
            let t = Instant::now();
            buzz_app::stage::build_scene(&mut vello, &e, area, scale, &mut cache);
            v.push(ms(t));
            let _ = cache.lights.take_misses();
        }
        println!("  idle encode ms: {:?}", v.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>());
        let g: Vec<f64> = (0..6).map(|_| gpu_ms(&mut gctx, &view, &vello)).collect();
        println!("  scene {:.1} MiB, lights cache {} entries; GPU ms: {:?}", scene_bytes(&vello) as f64 / 1048576.0, cache.lights.len(), g.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>());
        // Retained-texture parity: the stage as one render (what a skipped
        // redraw keeps showing) against a fresh render of the same encoding.
        gpu_ms(&mut gctx, &view, &vello);
        let kept = read_back(&gctx, &target);
        gpu_ms(&mut gctx, &view, &vello);
        let fresh = read_back(&gctx, &target);
        println!("  retained-vs-fresh PSNR {:.2} dB at {:.0}%", psnr(&kept, &fresh), e.camera.zoom_percent());
        kept.save(out.join(format!("stage_{:.0}.png", e.camera.zoom_percent()))).unwrap();
        // An idle redraw with the retained texture: only the blit to the surface.
        {
            let blitter = wgpu::util::TextureBlitter::new(&gctx.device, wgpu::TextureFormat::Rgba8Unorm);
            let dst = gctx.device.create_texture(&wgpu::TextureDescriptor { label: Some("dst"), size: wgpu::Extent3d { width: 1600, height: 900, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
            let dview = dst.create_view(&wgpu::TextureViewDescriptor::default());
            let mut v = Vec::new();
            for _ in 0..8 {
                let t = Instant::now();
                let mut enc = gctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                blitter.copy(&gctx.device, &mut enc, &view, &dview);
                gctx.queue.submit([enc.finish()]);
                gctx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
                v.push(ms(t));
            }
            println!("  retained idle redraw (blit only) ms: {:?}", v.iter().map(|x| (x * 100.0).round() / 100.0).collect::<Vec<_>>());
        }
        // drag the lamp, as the window does mid-gesture: nothing built inline.
        cache.lights.set_inline_budget(std::time::Duration::ZERO);
        cache.lights.set_queue(false);
        let draft = std::env::var("DRAFT").is_ok_and(|v| v == "1");
        cache.set_draft(draft);
        let mut v = Vec::new();
        if let Some(id) = lamp {
            for step in 1..=12 {
                e.doc.edit("Move Light", |scene| {
                    if let Some(l) = scene.lights_mut().get_mut(id)
                        && let buzz_scene::LightKind::Lamp { position, .. } = &mut l.kind
                    {
                        position.x += 20.0;
                        let _ = step;
                    }
                });
                let t = Instant::now();
                buzz_app::stage::build_scene(&mut vello, &e, area, scale, &mut cache);
                v.push(ms(t));
                let _ = cache.lights.take_misses();
            }
        }
        println!("  drag encode ms: {:?}", v.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>());
        let g: Vec<f64> = (0..4).map(|_| gpu_ms(&mut gctx, &view, &vello)).collect();
        println!("  drag GPU ms: {:?} scene {:.1} MiB", g.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>(), scene_bytes(&vello) as f64 / 1048576.0);
        cache.set_draft(false);
        let t = Instant::now();
        buzz_app::stage::build_scene(&mut vello, &e, area, scale, &mut cache);
        println!("  release encode {:.1} ms ({} misses) detail {:?} drafting {} encode {} lines {} ceiling {}", ms(t), cache.lights.take_misses().len(), cache.detail(), cache.drafting(), cache.last_encode(), cache.last_lines(), cache.last_ceiling());
        let g: Vec<f64> = (0..4).map(|_| gpu_ms(&mut gctx, &view, &vello)).collect();
        println!("  after drag: scene {:.1} MiB; GPU ms: {:?}", scene_bytes(&vello) as f64 / 1048576.0, g.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>());
    }
}
