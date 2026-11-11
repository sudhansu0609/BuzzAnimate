//! scratch probe — delete after use (SPLIT_PICTURE_PLAN S5).
//! zz_split_artwork_probe <doc.buzz> <out dir>
//! Trace Bitmap (quick) the first picture, then the app's artwork split, with the gate numbers.
use std::path::PathBuf;

use image::RgbaImage;

fn psnr(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let mut se = 0f64;
    for (p, q) in a.pixels().zip(b.pixels()) {
        for k in 0..3 {
            let d = p[k] as f64 - q[k] as f64;
            se += d * d;
        }
    }
    let mse = se / (a.width() * a.height() * 3) as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(&a[1]);
    std::fs::create_dir_all(out.join("layers")).unwrap();
    std::fs::create_dir_all(out.join("hidden")).unwrap();
    // SAFETY: single-threaded here.
    unsafe { std::env::set_var("BUZZ_SPLIT_DEBUG_DIR", out.join("vlm")) };
    let bold = std::env::var("BOLD").is_ok();
    let mut e = if a[0].ends_with(".png") && std::env::var("PLACE").is_ok() {
        // The app's own way in: a picture imported onto a stage of STAGE=w,h.
        let mut e = buzz_app::editor::Editor::default();
        let st: Vec<f64> = std::env::var("STAGE").unwrap_or("1920,1080".into()).split(',').filter_map(|x| x.parse().ok()).collect();
        e.doc.edit("size", |scene| scene.stage_mut().size = buzz_geom::Size::new(st[0], st[1]));
        e.place_background(std::path::Path::new(&a[0])).unwrap();
        e
    } else if a[0].ends_with(".png") {
        let bytes = std::fs::read(&a[0]).unwrap();
        let mut e = buzz_app::editor::Editor::default();
        let layer = e.doc.scene().layers().iter().next().unwrap().id;
        let (sw, sh) = (1920.0, 1080.0);
        e.doc.edit("size", |scene| scene.stage_mut().size = buzz_geom::Size::new(sw, sh));
        e.doc.edit("import", |scene| {
            use buzz_geom::Shape as _;
            let asset = scene.add_image("Picture", &bytes).expect("decodes");
            let r = buzz_geom::Rect::new(0.0, 0.0, sw, sh);
            let fill = buzz_scene::ImageFill::new(asset, r);
            let shape = buzz_scene::ShapeData { path: r.to_path(1e-9), fill: Some(buzz_scene::FillSpec::image(fill)), stroke: None, blend: Default::default() };
            scene.add_shape(layer, shape).unwrap();
        });
        e
    } else {
        buzz_app::editor::Editor::new(buzz_doc::Document::open(&a[0]).unwrap())
    };
    let first = e.scene().layers().iter().find_map(|l| l.frames.keyframes().first().and_then(|k| k.objects.first().cloned())).unwrap();
    if matches!(&first.kind, buzz_scene::ObjectKind::Shape(sd) if matches!(sd.fill.as_ref().map(|f| &f.paint), Some(buzz_scene::Paint::Image(_)))) {
        e.selection.set([first.id]);
        if bold {
            e.trace_with(buzz_scene::TraceOptions::bold());
        } else {
            e.run(buzz_ui::Command::TraceBitmap);
        }
        e.finish_pending_trace_now();
    }
    let layer = e.scene().layers().iter().find(|l| l.frames.objects_at(0).len() > 0).unwrap().id;
    let before_scene = e.scene().clone();
    let count = |s: &buzz_scene::Scene, l: buzz_scene::LayerId| s.layers().get(l).map_or(0, |l| l.frames.objects_at(0).len());
    let shapes_before = count(&before_scene, layer);
    println!("traced: {shapes_before} shapes on the source layer");

    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let render = |ex: &mut buzz_export::Exporter, scene: &buzz_scene::Scene, transparent: bool| {
        let mut settings = buzz_export::ExportSettings::for_stage(scene);
        settings.transparent = transparent;
        let f = ex.render(scene, 0, &settings).unwrap();
        RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
    };
    let before = render(&mut ex, &before_scene, false);
    before.save(out.join("before.png")).unwrap();
    let transparent_before = render(&mut ex, &before_scene, true).pixels().filter(|p| p[3] < 250).count();

    let opts = buzz_app::bg_objects::ObjectOptions { max_objects: 16, small_objects: true, ..Default::default() };
    let t = std::time::Instant::now();
    let mut note = |s: &str| println!("step: {s}");
    let plan = buzz_app::split_artwork::plan_split(&before_scene, layer, 0, None, &opts, &mut note).unwrap();
    println!("plan in {:.1} s; notes: {:?}", t.elapsed().as_secs_f64(), plan.notes);
    plan.picture.save(out.join("rendered_input.png")).unwrap();
    for (i, o) in plan.objects.iter().enumerate() {
        o.mask.save(out.join(format!("mask_{i}_{}{}.png", o.label.replace(' ', "_"), o.n))).unwrap();
        println!("OBJECT {} {} bbox {:?} shapes {} cuts {} filler {}", o.label, o.n, o.bbox, o.shape_ids.len(), o.cuts.len(), o.filler.len());
    }
    let taken: usize = plan.objects.iter().map(|o| o.shape_ids.len() + o.cuts.len()).sum();
    let cut_rests: usize = plan.objects.iter().map(|o| o.cuts.iter().filter(|c| c.rest.is_some()).count()).sum();
    let fillers: usize = plan.objects.iter().map(|o| o.filler.len()).sum();
    e.doc.edit("Split into Layers", |s| {
        let made = buzz_app::split_artwork::apply(s, &plan);
        println!("made {made:?}");
    });
    let after_scene = e.scene().clone();
    let left = count(&after_scene, layer);
    let layers: Vec<_> = after_scene.layers().iter().map(|l| (l.id, l.name.clone(), l.frames.objects_at(0).len())).collect();
    println!("layers: {layers:?}");
    println!("LAYERS_BACK_TO_FRONT {:?}", layers.iter().rev().map(|l| l.1.clone()).collect::<Vec<_>>());
    println!("source layer: {shapes_before} -> {left} shapes; taken {taken}; fillers {fillers}; conserved {}", left + taken == shapes_before + fillers + cut_rests);
    // Each object layer should hold exactly one instance of one symbol.
    for (id, name, n) in &layers {
        if *id != layer {
            let objs = after_scene.layers().get(*id).unwrap().frames.objects_at(0);
            let sym = objs.first().and_then(|o| o.instance()).map(|i| i.symbol);
            let inner = sym.and_then(|s| after_scene.library().get(s)).map(|s| s.layers.iter().map(|l| l.frames.objects_at(0).len()).collect::<Vec<_>>());
            println!("  layer {name}: {n} object(s), symbol shapes per layer {inner:?}");
        }
    }

    let after = render(&mut ex, &after_scene, false);
    after.save(out.join("composite_after.png")).unwrap();
    {
        let mut err = RgbaImage::new(before.width(), before.height());
        for (x, y, p) in before.enumerate_pixels() {
            let q = after.get_pixel(x, y);
            let d = (0..3).map(|k| (p[k] as i32 - q[k] as i32).abs()).max().unwrap_or(0) as u32;
            let v = (d * 4).min(255) as u8;
            err.put_pixel(x, y, image::Rgba([v, v, v, 255]));
        }
        err.save(out.join("error.png")).unwrap();
    }
    // Image-filled shapes per layer (inside the symbol), and the scene saved and reopened.
    for (id, name, _) in &layers {
        let l = after_scene.layers().get(*id).unwrap();
        let sym = l.frames.objects_at(0).first().and_then(|o| o.instance()).map(|i| i.symbol);
        if let Some(sym) = sym.and_then(|s| after_scene.library().get(s)) {
            let (mut n, mut img) = (0, 0);
            for il in sym.layers.iter() {
                for o in il.frames.objects_at(0).iter() {
                    if let buzz_scene::ObjectKind::Shape(sd) = &o.kind {
                        n += 1;
                        img += matches!(sd.fill.as_ref().map(|f| &f.paint), Some(buzz_scene::Paint::Image(_))) as usize;
                    }
                }
            }
            println!("IMAGESHAPES {name}: {img} of {n}");
        }
    }
    {
        let path = out.join("after.buzz");
        buzz_doc::format::save(&after_scene, &path).unwrap();
        let re = buzz_doc::Document::open(&path).unwrap();
        let again = render(&mut ex, re.scene(), false);
        again.save(out.join("reopened.png")).unwrap();
        println!("PSNR reopened vs before: {:.2} dB", psnr(&before, &again));
    }
    // Zoomed and shrunk views of before and after (a cached symbol drawn at another scale).
    for (name, region, size) in [("zoom2x", buzz_geom::Rect::new(300.0, 200.0, 1260.0, 740.0), (1920u32, 1080u32)), ("zoom4x", buzz_geom::Rect::new(700.0, 400.0, 1180.0, 670.0), (1920, 1080)), ("half", buzz_geom::Rect::new(0.0, 0.0, 1920.0, 1080.0), (960, 540))] {
        let at = |ex: &mut buzz_export::Exporter, scene: &buzz_scene::Scene| {
            let mut settings = buzz_export::ExportSettings::for_stage(scene);
            settings.width = size.0;
            settings.height = size.1;
            settings.region = Some(region);
            let f = ex.render(scene, 0, &settings).unwrap();
            RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
        };
        let (b, a2) = (at(&mut ex, &before_scene), at(&mut ex, &after_scene));
        a2.save(out.join(format!("view_{name}_after.png"))).unwrap();
        b.save(out.join(format!("view_{name}_before.png"))).unwrap();
        println!("PSNR {name} before/after: {:.2} dB", psnr(&b, &a2));
    }
    println!("PSNR composite before/after: {:.2} dB", psnr(&before, &after));
    println!("transparent px (alpha<250) baseline: {transparent_before}");
    let mut alone_tiles: Vec<RgbaImage> = Vec::new();
    let mut objmask: Option<RgbaImage> = None;
    let mut hidden_max_excl_sky = 0usize;
    for (id, name, _) in &layers {
        if *id == layer {
            continue;
        }
        let file = name.replace(' ', "_");
        // The object layer alone.
        let mut alone = after_scene.clone();
        for (other, _, _) in &layers {
            if other != id {
                alone.update_layer(*other, |l| l.visible = false);
            }
        }
        render(&mut ex, &alone, false).save(out.join("layers").join(format!("{file}.png"))).unwrap();
        {
            let ta = render(&mut ex, &alone, true);
            let mut flat = RgbaImage::from_pixel(ta.width(), ta.height(), image::Rgba([200, 0, 200, 255]));
            image::imageops::overlay(&mut flat, &ta, 0, 0);
            alone_tiles.push(flat);
            if !matches!(name.as_str(), "Sky" | "Far" | "Land") {
                let m = objmask.get_or_insert_with(|| RgbaImage::new(ta.width(), ta.height()));
                for (x, y, p) in ta.enumerate_pixels() {
                    if p[3] >= 250 {
                        m.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
                    }
                }
            }
        }
        // How much of its mask the object's own shapes paint.
        if let Some(o) = plan.objects.iter().find(|o| format!("{} {}", o.label, o.n) == *name) {
            let ta = render(&mut ex, &alone, true);
            let (mut inm, mut hit) = (0u64, 0u64);
            for (x, y, p) in o.mask.enumerate_pixels() {
                if p[0] > 127 {
                    inm += 1;
                    if ta.get_pixel(x, y)[3] > 8 {
                        hit += 1;
                    }
                }
            }
            println!("PAINTED {name}: {:.1}% of its mask ({hit}/{inm})", hit as f64 * 100.0 / inm.max(1) as f64);
        }
        // The composite with this object hidden.
        let mut hidden = after_scene.clone();
        hidden.update_layer(*id, |l| l.visible = false);
        render(&mut ex, &hidden, false).save(out.join("hidden").join(format!("{file}.png"))).unwrap();
        let t = render(&mut ex, &hidden, true);
        let tp = t.pixels().filter(|p| p[3] < 250).count();
        let t0 = t.pixels().filter(|p| p[3] == 0).count();
        let t128 = t.pixels().filter(|p| p[3] < 128).count();
        let mut map = RgbaImage::new(t.width(), t.height());
        for (x, y, p) in t.enumerate_pixels() {
            map.put_pixel(x, y, if p[3] < 250 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 255]) });
        }
        map.save(out.join("hidden").join(format!("{file}_transparent.png"))).unwrap();
        if name != "Sky" {
            hidden_max_excl_sky = hidden_max_excl_sky.max(t0);
        }
        println!("HIDDEN {name}: transparent px alpha<250 {tp}, alpha<128 {t128}, alpha==0 {t0} (baseline {transparent_before})");
    }
    println!("HIDDEN_ALPHA0_MAX_EXCL_SKY {hidden_max_excl_sky}");
    // Insert test: a rectangle layer right above Land shows behind the objects and in front of the street.
    if let Some((land, _, _)) = layers.iter().find(|l| l.1 == "Land") {
        let mut t = after_scene.clone();
        let nl = t.add_layer("TEST", buzz_scene::LayerKind::Normal);
        let li = t.layer_index(*land).unwrap();
        t.reorder_layer(nl, li.saturating_sub(1));
        let mut path = buzz_geom::BezPath::new();
        path.move_to((-5000.0, -5000.0));
        path.line_to((5000.0, -5000.0));
        path.line_to((5000.0, 5000.0));
        path.line_to((-5000.0, 5000.0));
        path.close_path();
        t.add_shape(nl, buzz_scene::ShapeData::filled(path, peniko::Color::from_rgb8(255, 0, 255)));
        let with = render(&mut ex, &t, false);
        with.save(out.join("insert_test.png")).unwrap();
        if let Some(m) = &objmask {
            let (mut in_obj, mut same, mut out_n, mut mag) = (0u64, 0u64, 0u64, 0u64);
            for (x, y, p) in with.enumerate_pixels() {
                let q = after.get_pixel(x, y);
                if m.get_pixel(x, y)[3] > 0 {
                    in_obj += 1;
                    same += ((0..3).all(|k| (p[k] as i32 - q[k] as i32).abs() < 6)) as u64;
                } else {
                    out_n += 1;
                    mag += (p[0] > 240 && p[1] < 15 && p[2] > 240) as u64;
                }
            }
            let (a, b) = (same as f64 / in_obj.max(1) as f64, mag as f64 / out_n.max(1) as f64);
            println!("INSERT_TEST objects unchanged {:.2}% street magenta {:.2}% ok {}", a * 100.0, b * 100.0, a > 0.99 && b > 0.97);
        } else {
            println!("INSERT_TEST no object layers");
        }
    }
    {
        let tw = 480u32;
        let th = tw * before.height() / before.width();
        let rows = alone_tiles.len().div_ceil(4).max(1) as u32;
        let mut sh = RgbaImage::from_pixel(tw * 4 + 30, (th + 10) * rows, image::Rgba([20, 20, 20, 255]));
        for (i, t) in alone_tiles.iter().enumerate() {
            let s = image::imageops::resize(t, tw, th, image::imageops::FilterType::Triangle);
            image::imageops::overlay(&mut sh, &s, ((i % 4) as u32 * (tw + 10)) as i64, ((i / 4) as u32 * (th + 10)) as i64);
        }
        sh.save(out.join("layers_sheet.png")).unwrap();
    }
    // Sheet: composite before | after, then each hidden, 3 per row at 640 px wide.
    let mut tiles: Vec<RgbaImage> = vec![before.clone(), after.clone()];
    let mut names: Vec<_> = std::fs::read_dir(out.join("hidden")).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png") && !p.to_string_lossy().ends_with("_transparent.png")).collect();
    names.sort();
    for p in names {
        tiles.push(image::open(p).unwrap().to_rgba8());
    }
    let tw = 640u32;
    let th = tw * before.height() / before.width();
    let rows = tiles.len().div_ceil(3) as u32;
    let mut sheet = RgbaImage::from_pixel(tw * 3 + 20, (th + 10) * rows, image::Rgba([20, 20, 20, 255]));
    for (i, t) in tiles.iter().enumerate() {
        let s = image::imageops::resize(t, tw, th, image::imageops::FilterType::Triangle);
        image::imageops::overlay(&mut sheet, &s, ((i % 3) as u32 * (tw + 10)) as i64, ((i / 3) as u32 * (th + 10)) as i64);
    }
    sheet.save(out.join("sheet.png")).unwrap();
}
