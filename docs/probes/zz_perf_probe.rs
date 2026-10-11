//! Perf probe behind the diagnosis in docs/BIMARI5_LAG_PLAN.md. Not compiled.
//! To run: copy to crates/buzz-app/src/zz_perf_probe.rs, append to app.rs
//!   #[cfg(test)] #[path = "zz_perf_probe.rs"] mod zz_perf_probe;
//! then: BUZZ_PROBE_FILE=<file.fla> cargo test --release -p buzz-app --lib
//!   <bimari5_frame_costs|bimari5_playback_and_layers|bimari5_dump_layers> -- --ignored --nocapture
//! Remove both again afterwards.
use super::*;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

#[test]
#[ignore]
fn bimari5_frame_costs() {
    let path = std::env::var("BUZZ_PROBE_FILE").expect("BUZZ_PROBE_FILE");
    let t = Instant::now();
    let imported = crate::import::read(std::path::Path::new(&path)).unwrap();
    eprintln!("import {:.0} ms: {}", ms(t), imported.summary);
    let mut app = App::new(GpuPreference::Automatic).with_document(Document::new(imported.scene));
    eprintln!("total frames {}", app.editor.scene().frame_count());

    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
    let mut tick = 0.0f64;
    let mut ui_frame = |app: &mut App| {
        tick += 0.016;
        let t = Instant::now();
        let out = ctx.run_ui(
            egui::RawInput { screen_rect: Some(screen), time: Some(tick), ..Default::default() },
            |ui| {
                app.build_ui(ui);
            },
        );
        let build = ms(t);
        let t = Instant::now();
        let _jobs = ctx.tessellate(out.shapes, out.pixels_per_point);
        (build, ms(t))
    };
    for _ in 0..5 {
        ui_frame(&mut app);
    }

    let area = buzz_geom::Rect::new(0.0, 0.0, 1600.0, 900.0);
    let mut cache = buzz_render::document::DrawCache::default();
    let mut gpu = buzz_render::GpuContext::new_blocking(&GpuPreference::Automatic).unwrap();
    eprintln!("adapter: {} max_buffer_scale {}", gpu.selection.summary(), gpu.max_buffer_scale);
    cache.set_max_scale(gpu.max_buffer_scale);
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe"),
        size: wgpu::Extent3d { width: 1600, height: 900, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: buzz_render::RENDER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mut raster = |gpu: &mut buzz_render::GpuContext, scene: &vello::Scene| {
        let t = Instant::now();
        gpu.render(scene, &view, 1600, 900, peniko::Color::from_rgb8(0x2A, 0x2A, 0x2E)).unwrap();
        gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
        ms(t)
    };
    let frames: Vec<u32> = std::env::var("BUZZ_PROBE_FRAMES")
        .unwrap_or("0,20,100,300,500,700,715,730,900".into())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    for f in frames {
        app.editor.set_frame(f);
        app.editor.camera.viewport = buzz_geom::Size::new(1600.0, 900.0);
        let stage_rect = app.editor.scene().stage().stage_rect();
        app.editor.camera.fit_to_rect(stage_rect, 1.0);
        let (b1, t1) = ui_frame(&mut app);
        let (b2, t2) = ui_frame(&mut app);
        let mut scene = vello::Scene::new();
        let t = Instant::now();
        crate::stage::build_scene(&mut scene, &app.editor, area, 1.0, &mut cache);
        let enc1 = ms(t);
        let mut scene = vello::Scene::new();
        let t = Instant::now();
        crate::stage::build_scene(&mut scene, &app.editor, area, 1.0, &mut cache);
        let enc2 = ms(t);
        gpu.grow_to(cache.scale());
        let g1 = raster(&mut gpu, &scene);
        let g2 = raster(&mut gpu, &scene);
        eprintln!("          gpu raster first {g1:7.1} again {g2:7.1} (gpu scale {})", gpu.buffer_scale);
        eprintln!(
            "frame {f:5}: ui cold {b1:7.1}+tess {t1:5.1} | ui warm {b2:7.1}+tess {t2:5.1} | encode cold {enc1:7.1} warm {enc2:7.1} | lines {} bands {} over {} scale {} detail {:?}",
            cache.last_lines(),
            cache.bands_needed(),
            cache.over_budget(),
            cache.scale(),
            cache.detail()
        );
    }
}

fn encode_ms(editor: &crate::editor::Editor, cache: &mut buzz_render::document::DrawCache) -> f64 {
    let area = buzz_geom::Rect::new(0.0, 0.0, 1600.0, 900.0);
    let mut scene = vello::Scene::new();
    let t = Instant::now();
    crate::stage::build_scene(&mut scene, editor, area, 1.0, cache);
    ms(t)
}

fn framed(scene: buzz_scene::Scene) -> crate::editor::Editor {
    let mut e = crate::editor::Editor::new(Document::new(scene));
    e.camera.viewport = buzz_geom::Size::new(1600.0, 900.0);
    let r = e.scene().stage().stage_rect();
    e.camera.fit_to_rect(r, 1.0);
    e
}

#[test]
#[ignore]
fn bimari5_playback_and_layers() {
    let path = std::env::var("BUZZ_PROBE_FILE").expect("BUZZ_PROBE_FILE");
    let base = crate::import::read(std::path::Path::new(&path)).unwrap().scene;
    let mut cache = buzz_render::document::DrawCache::default();
    cache.set_max_scale(8);

    // Playback: every frame once, in order, as the playhead visits them.
    for (from, to) in [(0u32, 48u32), (690, 740), (880, 930)] {
        let mut e = framed(base.clone());
        let mut v = Vec::new();
        for f in from..to {
            e.current_frame = f;
            let sb = cache.symbol_scenes.builds;
            let t = encode_ms(&e, &mut cache);
            v.push((f, t, cache.symbol_scenes.builds - sb));
        }
        let mut s: Vec<f64> = v.iter().map(|x| x.1).collect();
        s.sort_by(|a, b| a.total_cmp(b));
        eprintln!(
            "playback {from}..{to}: median {:.1} p95 {:.1} max {:.1} ms; new symbol builds/frame avg {:.1}",
            s[s.len() / 2],
            s[s.len() * 95 / 100],
            s[s.len() - 1],
            v.iter().map(|x| x.2 as f64).sum::<f64>() / v.len() as f64
        );
    }

    // Per layer: warm encode with that one layer hidden.
    for f in [715u32, 900, 4700] {
        let mut e = framed(base.clone());
        e.current_frame = f;
        encode_ms(&e, &mut cache);
        let full = (0..3).map(|_| encode_ms(&e, &mut cache)).fold(f64::MAX, f64::min);
        let mut rows = Vec::new();
        let ids: Vec<_> = base.layers().iter().map(|l| (l.id, l.name.clone(), l.visible)).collect();
        for (id, name, vis) in ids {
            if !vis {
                continue;
            }
            let mut s = base.clone();
            s.update_layer(id, |l| l.visible = false);
            let mut e = framed(s);
            e.current_frame = f;
            encode_ms(&e, &mut cache);
            let t = (0..2).map(|_| encode_ms(&e, &mut cache)).fold(f64::MAX, f64::min);
            rows.push((full - t, name));
        }
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        eprintln!("frame {f}: full warm encode {full:.1} ms; top layers by cost:");
        for (d, n) in rows.iter().take(6) {
            eprintln!("    {d:6.1} ms  {n}");
        }
    }
}

#[derive(Default, Debug)]
struct Tally {
    shapes: usize,
    instances: usize,
    filters: usize,
    blends: usize,
    tints: usize,
    non_similar: usize,
    masks: usize,
    inv_masks: usize,
    groups: usize,
    rigs_or_other: usize,
    max_depth: usize,
    symbols: std::collections::BTreeSet<String>,
}

fn similar(a: &kurbo::Affine) -> bool {
    let [a0, a1, a2, a3, _, _] = a.as_coeffs();
    let s1 = a0.hypot(a1);
    let s2 = a2.hypot(a3);
    (s1 - s2).abs() < 0.03 * s1.max(s2) && (a0 * a2 + a1 * a3).abs() < 0.03 * s1 * s2
}

fn walk(lib: &buzz_scene::symbol::Library, objs: &[std::sync::Arc<buzz_scene::object::Object>], depth: usize, t: &mut Tally) {
    t.max_depth = t.max_depth.max(depth);
    for o in objs {
        if !o.filters.is_empty() { t.filters += 1; }
        if format!("{:?}", o.blend) != "Normal" { t.blends += 1; }
        if !similar(&o.transform) { t.non_similar += 1; }
        match &o.kind {
            buzz_scene::object::ObjectKind::Shape(_) => t.shapes += 1,
            buzz_scene::object::ObjectKind::Group(c) => { t.groups += 1; walk(lib, c, depth, t); }
            buzz_scene::object::ObjectKind::Instance(i) => {
                t.instances += 1;
                if !i.color.is_identity() { t.tints += 1; }
                if let Some(s) = lib.get(i.symbol) {
                    if t.symbols.len() < 12 { t.symbols.insert(s.name.clone()); }
                    for l in s.layers.iter() {
                        if l.kind.is_mask() { t.masks += 1; }
                        if l.kind.is_inverted_mask() { t.inv_masks += 1; }
                        // every frame of the child, as an animated graphic can show any
                        let mut seen = std::collections::HashSet::new();
                        for f in 0..l.length() {
                            let ob = l.objects_at(f);
                            if seen.insert(ob.as_ptr() as usize) { walk(lib, ob, depth + 1, t); }
                        }
                    }
                }
            }
            _ => t.rigs_or_other += 1,
        }
    }
}

#[test]
#[ignore]
fn bimari5_dump_layers() {
    let path = std::env::var("BUZZ_PROBE_FILE").expect("BUZZ_PROBE_FILE");
    let scene = crate::import::read(std::path::Path::new(&path)).unwrap().scene;
    let lib = scene.library();
    for (name, f) in [("Layer_29", 715u32), ("charriotRunning", 900), ("well", 4700), ("charriotRunning", 4700)] {
        for l in scene.layers().iter().filter(|l| l.name == name) {
            let objs = l.objects_at(f);
            eprintln!("== {name} @ {f}: kind {:?} {} top objects, layer effects {}", l.kind, objs.len(), l.has_layer_effects());
            for o in objs.iter().take(6) {
                let a = o.transform.as_coeffs();
                let what = match &o.kind {
                    buzz_scene::object::ObjectKind::Instance(i) => format!(
                        "Instance {:?} loop {:?} first {} tint {}",
                        lib.get(i.symbol).map(|s| s.name.clone()), i.loop_mode, i.first_frame, !i.color.is_identity()
                    ),
                    buzz_scene::object::ObjectKind::Shape(_) => "Shape".into(),
                    k => format!("{:?}", std::mem::discriminant(k)),
                };
                eprintln!("   {what} | xf [{:.3} {:.3} {:.3} {:.3}] similar {} filters {} blend {:?} spatial {:?} mods {}",
                    a[0], a[1], a[2], a[3], similar(&o.transform), o.filters.len(), o.blend, o.spatial, o.modifiers.len());
            }
            let mut t = Tally::default();
            walk(lib, objs, 0, &mut t);
            eprintln!("   subtree: {t:?}");
        }
    }
}

#[test]
#[ignore]
fn renderer_rebuild_cost() {
    let t = Instant::now();
    let mut gpu = buzz_render::GpuContext::new_blocking(&GpuPreference::Automatic).unwrap();
    eprintln!("context new {:.0} ms", ms(t));
    for s in [4u32, 2, 1] {
        let t = Instant::now();
        gpu.shrink_to(s).unwrap();
        eprintln!("shrink_to({s}) renderer rebuild {:.0} ms", ms(t));
        let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 1600, height: 900, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: buzz_render::RENDER_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut sc = vello::Scene::new();
        sc.fill(peniko::Fill::NonZero, kurbo::Affine::IDENTITY, peniko::Color::WHITE, None, &kurbo::Circle::new((100.0, 100.0), 50.0));
        for i in 0..2 {
            let t = Instant::now();
            gpu.render(&sc, &view, 1600, 900, peniko::Color::BLACK).unwrap();
            gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
            eprintln!("   render #{i} after rebuild {:.0} ms", ms(t));
        }
    }
}
