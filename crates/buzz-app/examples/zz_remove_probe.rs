//! scratch probe — delete after use (SPLIT_PICTURE_PLAN S7).
//! zz_remove_probe <doc.buzz> <out dir> [name:x,y[;x,y|-x,y...] ...]   (points in bitmap px; "-" before x = negative)
//! Renders the artwork (saves view.png), then for each target removes it and measures the gate.
use std::path::PathBuf;

use image::{GrayImage, Rgba, RgbaImage};

fn psnr_outside(a: &RgbaImage, b: &RgbaImage, skip: &GrayImage) -> f64 {
    let mut se = 0f64;
    let mut n = 0u64;
    for (x, y, p) in a.enumerate_pixels() {
        if skip.get_pixel(x, y)[0] > 0 {
            continue;
        }
        let q = b.get_pixel(x, y);
        for k in 0..3 {
            let d = p[k] as f64 - q[k] as f64;
            se += d * d;
        }
        n += 3;
    }
    let mse = se / n.max(1) as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

fn render(ex: &mut buzz_export::Exporter, scene: &buzz_scene::Scene, region: Option<buzz_geom::Rect>, size: Option<(u32, u32)>, transparent: bool) -> RgbaImage {
    let mut s = buzz_export::ExportSettings::for_stage(scene);
    s.transparent = transparent;
    if let Some((w, h)) = size {
        s.width = w;
        s.height = h;
    }
    s.region = region;
    let f = ex.render(scene, 0, &s).unwrap();
    RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
}

fn bad(img: &RgbaImage, hole: &GrayImage) -> usize {
    img.enumerate_pixels().filter(|(x, y, p)| hole.get_pixel(*x, *y)[0] > 0 && (p[3] < 250 || (p[0] >= 250 && p[1] >= 250 && p[2] >= 250))).count()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out).unwrap();
    let doc = buzz_doc::Document::open(&a[0]).unwrap();
    let e = buzz_app::editor::Editor::new(doc);
    let scene = e.scene().clone();
    let layer = scene.layers().iter().next().unwrap().id;
    let t = std::time::Instant::now();
    let prep = buzz_app::split_artwork::prepare_remove(&scene, layer, 0, true).unwrap();
    println!("prepared {}x{} in {:.1} s; to_world {:?}", prep.picture.width(), prep.picture.height(), t.elapsed().as_secs_f64(), prep.to_world.as_coeffs());
    prep.picture.save(out.join("view.png")).unwrap();
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let opts = buzz_app::bg_objects::ObjectOptions::default();
    let before = render(&mut ex, &scene, None, None, false);
    let mut sheet_rows: Vec<RgbaImage> = Vec::new();
    for spec in a.iter().skip(2) {
        let (name, pts) = spec.split_once(':').unwrap();
        let points: Vec<(f32, f32, bool)> = pts
            .split(';')
            .map(|p| {
                let (neg, p) = p.strip_prefix('-').map_or((false, p), |r| (true, r));
                let (x, y) = p.split_once(',').unwrap();
                (x.parse().unwrap(), y.parse().unwrap(), !neg)
            })
            .collect();
        let t = std::time::Instant::now();
        let mask = buzz_app::split_artwork::mask_at(&prep, &points).unwrap();
        let mt = t.elapsed().as_secs_f64();
        mask.save(out.join(format!("mask_{name}.png"))).unwrap();
        let t = std::time::Instant::now();
        let mut note = |s: &str| println!("  step: {s}");
        let plan = match buzz_app::split_artwork::plan_remove(&scene, &prep, buzz_app::split_artwork::RemoveTarget::Mask(&mask), &opts, &mut note) {
            Ok(p) => p,
            Err(e) => {
                println!("REMOVE {name}: FAILED {e}");
                continue;
            }
        };
        let pt = t.elapsed().as_secs_f64();
        let mut doc2 = buzz_doc::Document::open(&a[0]).unwrap();
        let mut n = 0;
        doc2.edit("Remove from Picture", |s| n = buzz_app::split_artwork::apply_remove(s, &plan));
        let after_scene = doc2.scene().clone();
        let after = render(&mut ex, &after_scene, None, None, false);
        let after_t = render(&mut ex, &after_scene, None, None, true);
        after.save(out.join(format!("after_{name}.png"))).unwrap();
        let skip = GrayImage::from_raw(plan.hole.width(), plan.hole.height(), buzz_ai::segment::dilate(plan.hole.as_raw(), plan.hole.width(), plan.hole.height(), 8)).unwrap();
        let p = psnr_outside(&before, &after, &skip);
        let bad100 = bad(&after_t, &plan.hole);
        // 400 %: the hole's box (padded) rendered at 4x.
        let (w, h) = plan.hole.dimensions();
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
        for (x, y, v) in plan.hole.enumerate_pixels() {
            if v[0] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
        let (x0, y0, x1, y1) = (x0.saturating_sub(10), y0.saturating_sub(10), (x1 + 10).min(w), (y1 + 10).min(h));
        let (cw, ch) = (x1 - x0, y1 - y0);
        let region = prep.to_world.transform_rect_bbox(buzz_geom::Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64));
        let big_t = render(&mut ex, &after_scene, Some(region), Some((cw * 4, ch * 4)), true);
        let mut big_hole = GrayImage::new(cw * 4, ch * 4);
        for (x, y, v) in big_hole.enumerate_pixels_mut() {
            *v = *plan.hole.get_pixel(x0 + x / 4, y0 + y / 4);
        }
        let bad400 = bad(&big_t, &big_hole);
        let big = render(&mut ex, &after_scene, Some(region), Some((cw * 4, ch * 4)), false);
        big.save(out.join(format!("after400_{name}.png"))).unwrap();
        println!(
            "REMOVE {name}: mask {:.2}s plan {:.1}s shapes removed/cut {n} (whole {} cuts {}) patch shapes {} lama {} hole box {cw}x{ch}  bad100 {bad100} bad400 {bad400} untouched PSNR {p:.2} dB",
            mt,
            pt,
            plan.remove_ids.len(),
            plan.cuts.len(),
            plan.patch.len(),
            plan.lama
        );
        // Row: before crop | mask overlay | after crop, padded by 30 px, at 3x.
        let (px0, py0, px1, py1) = (x0.saturating_sub(30), y0.saturating_sub(30), (x1 + 30).min(w), (y1 + 30).min(h));
        let (rw, rh) = (px1 - px0, py1 - py0);
        let mut ov = image::imageops::crop_imm(&before, px0, py0, rw, rh).to_image();
        for (x, y, v) in ov.enumerate_pixels_mut() {
            if mask.get_pixel(px0 + x, py0 + y)[0] > 127 {
                *v = Rgba([(v[0] as u16 / 2 + 127) as u8, v[1] / 2, v[2] / 2, 255]);
            }
        }
        let tiles = [
            image::imageops::crop_imm(&before, px0, py0, rw, rh).to_image(),
            ov,
            image::imageops::crop_imm(&after, px0, py0, rw, rh).to_image(),
        ];
        let mut row = RgbaImage::from_pixel(rw * 3 * 3 + 20, rh * 3, Rgba([40, 40, 40, 255]));
        for (i, t) in tiles.iter().enumerate() {
            let up = image::imageops::resize(t, rw * 3, rh * 3, image::imageops::FilterType::Nearest);
            image::imageops::overlay(&mut row, &up, (i as u32 * (rw * 3 + 10)) as i64, 0);
        }
        sheet_rows.push(row);
    }
    if !sheet_rows.is_empty() {
        let (sw, sh): (u32, u32) = (sheet_rows.iter().map(|r| r.width()).max().unwrap(), sheet_rows.iter().map(|r| r.height() + 10).sum());
        let mut sheet = RgbaImage::from_pixel(sw, sh, Rgba([20, 20, 20, 255]));
        let mut y = 0i64;
        for r in &sheet_rows {
            image::imageops::overlay(&mut sheet, r, 0, y);
            y += r.height() as i64 + 10;
        }
        sheet.save(out.join("sheet.png")).unwrap();
    }
}
