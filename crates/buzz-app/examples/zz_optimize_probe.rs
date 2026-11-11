//! scratch probe (TRACE_PICTURE_PLAN T13) — Optimize Shapes on a traced
//! picture through the headless Editor: counts, render time, PSNR, seams.
//! Usage: zz_optimize_probe <file.buzz> <out dir> [smoothing merge(0|1) tiny]...
use buzz_geom::Rect;
use image::RgbaImage;
use serde_json::json;
use std::time::Instant;

fn render(ex: &mut buzz_export::Exporter, scene: &buzz_scene::Scene, region: Rect, scale: f64, transparent: bool) -> RgbaImage {
    let s = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent,
        region: Some(region),
    };
    let f = ex.render(scene, 0, &s).unwrap();
    RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
}

fn timed(ex: &mut buzz_export::Exporter, scene: &buzz_scene::Scene, region: Rect) -> (f64, f64) {
    let t = Instant::now();
    let _ = render(ex, scene, region, 1.0, false);
    let cold = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    for _ in 0..5 {
        let _ = render(ex, scene, region, 1.0, false);
    }
    (cold, t.elapsed().as_secs_f64() * 200.0)
}

fn psnr(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let mut se = 0.0;
    let mut n: f64 = 0.0;
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

fn clear(a: &RgbaImage) -> usize {
    a.pixels().filter(|p| p[3] < 128).count()
}

fn side_by_side(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(a.width() + b.width() + 8, a.height().max(b.height()), image::Rgba([255, 0, 255, 255]));
    image::imageops::overlay(&mut out, a, 0, 0);
    image::imageops::overlay(&mut out, b, a.width() as i64 + 8, 0);
    out
}

fn counts(scene: &buzz_scene::Scene) -> (usize, usize) {
    let (mut shapes, mut segs) = (0, 0);
    for l in scene.layers().iter() {
        for k in l.frames.keyframes() {
            for o in k.objects.iter() {
                if let buzz_scene::ObjectKind::Shape(s) = &o.kind {
                    shapes += 1;
                    segs += buzz_app::rigging::segment_count(&s.path);
                }
            }
        }
    }
    (shapes, segs)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = &a[1];
    std::fs::create_dir_all(out).unwrap();
    let mut runs: Vec<(f64, bool, f64)> = Vec::new();
    for c in a[2..].chunks(3) {
        if c.len() == 3 {
            runs.push((c[0].parse().unwrap(), c[1] == "1", c[2].parse().unwrap()));
        }
    }
    if runs.is_empty() {
        runs.push((0.5, true, 1.0));
    }
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let mut results = Vec::new();
    for (i, (smoothing, merge, tiny)) in runs.into_iter().enumerate() {
        let doc = buzz_doc::Document::open(&a[0]).unwrap();
        let mut e = buzz_app::editor::Editor::new(doc);
        let before = e.scene().clone();
        let region = before.content_bounds().unwrap_or(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        let (shapes_b, segs_b) = counts(&before);
        let (cold_b, warm_b) = if i == 0 { timed(&mut ex, &before, region) } else { (0.0, 0.0) };
        let settings = buzz_app::rigging::OptimizeSettings { smoothing, merge_same_colour: merge, remove_tiny: tiny };
        let t = Instant::now();
        let report = e.optimize_shapes_now(None, settings).expect("optimized");
        let opt_ms = t.elapsed().as_secs_f64() * 1000.0;
        let after = e.scene().clone();
        let (shapes_a, segs_a) = counts(&after);
        let (cold_a, warm_a) = timed(&mut ex, &after, region);
        let wb = render(&mut ex, &before, region, 1.0, false);
        let wa = render(&mut ex, &after, region, 1.0, false);
        let p = psnr(&wb, &wa);
        // 400 % crops: seams (see-through pixels) before / after, and a sheet.
        let crops = [
            Rect::new(region.x0 + 100.0, region.y0 + 300.0, region.x0 + 420.0, region.y0 + 480.0),
            Rect::new(region.x0 + 900.0, region.y0 + 500.0, region.x0 + 1220.0, region.y0 + 680.0),
            Rect::new(region.x0 + 1500.0, region.y0 + 150.0, region.x0 + 1820.0, region.y0 + 330.0),
        ];
        let mut seams = Vec::new();
        let mut rows: Vec<RgbaImage> = Vec::new();
        for r in crops {
            let tb = render(&mut ex, &before, r, 4.0, true);
            let ta = render(&mut ex, &after, r, 4.0, true);
            seams.push((clear(&tb), clear(&ta)));
            let ob = render(&mut ex, &before, r, 4.0, false);
            let oa = render(&mut ex, &after, r, 4.0, false);
            rows.push(side_by_side(&ob, &oa));
        }
        let w = rows.iter().map(|r| r.width()).max().unwrap_or(1);
        let h: u32 = rows.iter().map(|r| r.height() + 8).sum();
        let mut sheet = RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
        let mut y = 0;
        for r in &rows {
            image::imageops::overlay(&mut sheet, r, 0, y);
            y += r.height() as i64 + 8;
        }
        sheet.save(format!("{out}/sheet_{i}.png")).unwrap();
        results.push(json!({
            "settings": {"smoothing": smoothing, "merge": merge, "tiny": tiny},
            "shapes": [shapes_b, shapes_a], "segments": [segs_b, segs_a], "report": [report.shapes_before, report.shapes_after, report.segments_before, report.segments_after],
            "optimize_ms": opt_ms, "psnr_db": p,
            "render_ms_cold_before_after": [cold_b, cold_a], "render_ms_warm_before_after": [warm_b, warm_a],
            "clear_px_400_before_after": seams,
        }));
    }
    println!("{}", json!(results));
}
