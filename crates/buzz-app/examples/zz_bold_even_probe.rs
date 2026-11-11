//! scratch probe — delete after use (TRACE_PICTURE_PLAN T15).
//! zz_bold_even_probe <doc.buzz | picture.png> <out dir> <tag> [guddu <Guddu.buzz>]
//! Even Out Lines plain and with Bold outlines through the headless Editor (the way the dialog drives
//! it), then the gate numbers: weight, colour, breaks, see-through at 400 %. A .png is first traced
//! plainly (colour regions, ink as dark fills) into a scene.
use buzz_geom::Rect;
use buzz_ui::{Command, EvenLinesAction};
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

fn luma(p: &image::Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

/// ink = darker than 0.55 x its 21 px mean and luma < 90 (as zz_bold_measure).
fn ink_mask(img: &RgbaImage) -> Vec<bool> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let l: Vec<f32> = img.pixels().map(luma).collect();
    let mut integral = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += l[y * w + x] as f64;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let r = 10usize;
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (x0, x1, y0, y1) = (x.saturating_sub(r), (x + r + 1).min(w), y.saturating_sub(r), (y + r + 1).min(h));
            let sum = integral[y1 * (w + 1) + x1] - integral[y0 * (w + 1) + x1] - integral[y1 * (w + 1) + x0] + integral[y0 * (w + 1) + x0];
            let mean = sum / ((x1 - x0) * (y1 - y0)) as f64;
            l[i] < 90.0 && (l[i] as f64) < 0.55 * mean
        })
        .collect()
}

/// (p10, median, p90, median colour) of the ink weight, length-weighted.
fn weight(img: &RgbaImage) -> (f64, f64, f64, [u8; 3]) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let ink = ink_mask(img);
    let mut hrun = vec![0u16; w * h];
    let mut vrun = vec![0u16; w * h];
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if !ink[y * w + x] {
                x += 1;
                continue;
            }
            let s = x;
            while x < w && ink[y * w + x] {
                x += 1;
            }
            for k in s..x {
                hrun[y * w + k] = (x - s) as u16;
            }
        }
    }
    for x in 0..w {
        let mut y = 0;
        while y < h {
            if !ink[y * w + x] {
                y += 1;
                continue;
            }
            let s = y;
            while y < h && ink[y * w + x] {
                y += 1;
            }
            for k in s..y {
                vrun[k * w + x] = (y - s) as u16;
            }
        }
    }
    let mut ws: Vec<(f64, f64)> = Vec::new();
    let mut cols: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for i in 0..w * h {
        if !ink[i] {
            continue;
        }
        let m = hrun[i].min(vrun[i]) as f64;
        if !(1.0..=14.0).contains(&m) {
            continue;
        }
        ws.push((m, 1.0 / m));
        let p = img.get_pixel((i % w) as u32, (i / w) as u32);
        for c in 0..3 {
            cols[c].push(p[c]);
        }
    }
    ws.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let total: f64 = ws.iter().map(|x| x.1).sum();
    let pct = |q: f64| {
        let mut acc = 0.0;
        for (m, wt) in &ws {
            acc += wt;
            if acc >= total * q {
                return *m;
            }
        }
        0.0
    };
    let med = |v: &mut Vec<u8>| {
        v.sort_unstable();
        v.get(v.len() / 2).copied().unwrap_or(0)
    };
    (pct(0.1), pct(0.5), pct(0.9), [med(&mut cols[0]), med(&mut cols[1]), med(&mut cols[2])])
}

/// Breaks: strong colour edges of `src` (CIELAB dE > 25 across 3 px, chains of 40+ px) with no
/// dark pixel of `out` within 3 px, in runs whose extent is over `min_extent`. (all, near buildings).
fn breaks(src: &RgbaImage, out: &RgbaImage, min_extent: f64) -> (usize, usize) {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let lab: Vec<[f64; 3]> = src.pixels().map(|p| buzz_scene::trace::srgb_to_lab([p[0], p[1], p[2]])).collect();
    let mut edge = vec![false; w * h];
    for y in 0..h.saturating_sub(3) {
        for x in 0..w.saturating_sub(3) {
            let i = y * w + x;
            if buzz_scene::trace::delta_e(lab[i], lab[i + 3]) > 25.0 || buzz_scene::trace::delta_e(lab[i], lab[i + 3 * w]) > 25.0 {
                edge[i] = true;
            }
        }
    }
    // Dark in `out`, dilated by 3.
    let dark: Vec<bool> = out.pixels().map(|p| luma(p) < 60.0).collect();
    let mut near = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if dark[y * w + x] {
                for ny in y.saturating_sub(3)..=(y + 3).min(h - 1) {
                    for nx in x.saturating_sub(3)..=(x + 3).min(w - 1) {
                        near[ny * w + nx] = true;
                    }
                }
            }
        }
    }
    // Chains of edge pixels (8-connected), then uncovered runs inside long chains.
    let mut seen = vec![false; w * h];
    let (mut all, mut near_count) = (0usize, 0usize);
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !edge[start] || seen[start] {
            continue;
        }
        let mut comp = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(at) = stack.pop() {
            comp.push(at);
            let (x, y) = ((at % w) as i64, (at / w) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if edge[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        if comp.len() < 40 {
            continue;
        }
        // Uncovered pixels of the chain, grouped.
        let mut gap_seen = std::collections::HashSet::new();
        let set: std::collections::HashSet<usize> = comp.iter().copied().filter(|&i| !near[i]).collect();
        for &p in &set {
            if gap_seen.contains(&p) {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
            let mut st = vec![p];
            gap_seen.insert(p);
            while let Some(at) = st.pop() {
                let (x, y) = (at % w, at / w);
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx < 0 || ny < 0 {
                            continue;
                        }
                        let n = ny as usize * w + nx as usize;
                        if nx < w as i64 && set.contains(&n) && gap_seen.insert(n) {
                            st.push(n);
                        }
                    }
                }
            }
            let extent = ((x1 - x0) as f64).hypot((y1 - y0) as f64);
            if extent > min_extent {
                all += 1;
                if x0 < 640 || x1 > 1480 {
                    near_count += 1;
                }
            }
        }
    }
    (all, near_count)
}

fn editor_for(doc: &buzz_scene::Scene) -> buzz_app::editor::Editor {
    buzz_app::editor::Editor::new(buzz_doc::Document::new(doc.clone()))
}

/// Even Out Lines through the dialog's own calls; returns the scene after Apply and the timings.
fn even(scene: &buzz_scene::Scene, bold: bool) -> (buzz_scene::Scene, serde_json::Value) {
    let mut e = editor_for(scene);
    let t = Instant::now();
    e.run(Command::EvenLines);
    let ui_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t_open = Instant::now();
    e.finish_even_lines_now();
    let open_s = t_open.elapsed().as_secs_f64();
    let d = &e.even_lines_dialog;
    let counts = (d.face_lines, d.body_lines, d.solids);
    let mut bold_ms = 0.0;
    if bold {
        e.even_lines_dialog.bold = true;
        let bw = e.even_lines_dialog.bold_width;
        e.even_lines_dialog.face_width = bw;
        e.even_lines_dialog.body_width = bw;
        let t = Instant::now();
        e.even_lines_action(EvenLinesAction::Widths);
        bold_ms = t.elapsed().as_secs_f64() * 1000.0;
    }
    let t = Instant::now();
    e.even_lines_action(EvenLinesAction::Apply);
    let apply_ms = t.elapsed().as_secs_f64() * 1000.0;
    let d = &e.even_lines_dialog;
    let info = json!({
        "ui_return_ms": ui_ms, "open_to_result_s": open_s, "bold_preview_ms": bold_ms, "apply_ms": apply_ms,
        "lines_before_bold": [counts.0, counts.1, counts.2], "width": d.body_width,
        "after_shapes": e.scene().layers().iter().map(|l| l.frames.keyframes().iter().map(|k| k.objects.len()).sum::<usize>()).sum::<usize>(),
    });
    (e.scene().clone(), info)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (input, out, tag) = (&a[0], &a[1], &a[2]);
    std::fs::create_dir_all(out).unwrap();
    let mut source_png: Option<RgbaImage> = None;
    let before: buzz_scene::Scene = if input.ends_with(".png") {
        let img = image::open(input).unwrap().to_rgba8();
        let asset = buzz_scene::ImageAsset::decode(buzz_scene::ImageId(1), "in", &std::fs::read(input).unwrap()).unwrap();
        let options = buzz_scene::TraceOptions {
            mode: buzz_scene::TraceMode::Outline,
            colours: 64,
            keep_alpha: true,
            soft_tones: true,
            ..buzz_scene::TraceOptions::default()
        };
        let report = buzz_scene::trace(asset.width, asset.height, &asset.pixels, &options);
        println!("plain trace: {} shapes", report.shapes.len());
        let mut scene = buzz_scene::Scene::default();
        scene.stage_mut().size = buzz_geom::Size::new(asset.width as f64, asset.height as f64);
        let layer = scene.add_layer("trace", buzz_scene::LayerKind::Normal);
        let blurs: std::collections::HashMap<usize, f64> = report.blurs.iter().copied().collect();
        for (i, shape) in report.shapes.into_iter().enumerate() {
            let mut object = buzz_scene::Object::shape(buzz_scene::ObjectId(1_000_000 + i as u64), shape);
            if let Some(&radius) = blurs.get(&i) {
                object.filters.push(buzz_scene::TraceReport::blur_filter(radius));
            }
            scene.add_object(layer, object);
        }
        source_png = Some(img);
        scene
    } else {
        buzz_doc::Document::open(input).unwrap().scene().clone()
    };
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let full = before.content_bounds().unwrap_or(Rect::new(0.0, 0.0, 1920.0, 1080.0));
    let stage = Rect::new(0.0, 0.0, before.stage().size.width, before.stage().size.height);
    let full_stage = if input.ends_with(".png") { stage } else { full };

    let mut results = serde_json::Map::new();
    let mut renders: Vec<(&str, buzz_scene::Scene)> = vec![("before", before.clone())];
    for (name, bold) in [("even", false), ("bold", true)] {
        let (scene, info) = even(&before, bold);
        println!("{name}: {info}");
        results.insert(format!("{name}_timing"), info);
        renders.push((name, scene));
    }
    let crops = [
        ("a", Rect::new(full_stage.x0 + 100.0, full_stage.y0 + 300.0, full_stage.x0 + 420.0, full_stage.y0 + 480.0)),
        ("b", Rect::new(full_stage.x0 + 900.0, full_stage.y0 + 500.0, full_stage.x0 + 1220.0, full_stage.y0 + 680.0)),
        ("c", Rect::new(full_stage.x0 + 1500.0, full_stage.y0 + 150.0, full_stage.x0 + 1820.0, full_stage.y0 + 330.0)),
    ];
    let whole: Vec<RgbaImage> = renders.iter().map(|(_, s)| render(&mut ex, s, full_stage, 1.0, false)).collect();
    for ((name, _), img) in renders.iter().zip(&whole) {
        img.save(format!("{out}/{tag}_{name}_1x.png")).unwrap();
        let (p10, med, p90, col) = weight(img);
        let de = buzz_scene::trace::delta_e(buzz_scene::trace::srgb_to_lab(col), buzz_scene::trace::srgb_to_lab([0, 0, 0]));
        let reference = source_png.as_ref().unwrap_or(&whole[0]);
        let (b_all, b_near) = if reference.dimensions() == img.dimensions() { breaks(reference, img, 6.0) } else { (0, 0) };
        println!("{name}: weight p10/med/p90 {p10}/{med}/{p90}, colour #{:02x}{:02x}{:02x} dE-to-black {de:.1}, breaks>6px all {b_all} near {b_near}", col[0], col[1], col[2]);
        results.insert(format!("{name}_1x"), json!({"p10": p10, "median": med, "p90": p90, "colour": format!("#{:02x}{:02x}{:02x}", col[0], col[1], col[2]), "delta_e_black": de, "breaks_over_2w": b_all, "breaks_near": b_near}));
    }
    // 400 % crops: what turned white / see-through.
    let mut sheet_rows: Vec<RgbaImage> = Vec::new();
    let mut slivers = [0usize; 2];
    for (name, region) in crops {
        let bw = render(&mut ex, &renders[0].1, region, 4.0, false);
        let ba = render(&mut ex, &renders[0].1, region, 4.0, true);
        let mut row = vec![bw.clone()];
        for (k, (rname, scene)) in renders.iter().enumerate().skip(1) {
            let aw = render(&mut ex, scene, region, 4.0, false);
            let aa = render(&mut ex, scene, region, 4.0, true);
            let white = bw.pixels().zip(aw.pixels()).filter(|(b, a)| luma(b) < 127.0 && luma(a) > 230.0).count();
            let clear = ba.pixels().zip(aa.pixels()).filter(|(b, a)| b[3] > 127 && a[3] <= 127).count();
            println!("crop {name} {rname}: turned white {white}, turned clear {clear}");
            results.insert(format!("crop_{name}_{rname}"), json!({"white": white, "clear": clear}));
            slivers[k - 1] += white + clear;
            row.push(aw);
        }
        let w: u32 = row.iter().map(|r| r.width()).sum::<u32>() + 8 * (row.len() as u32 - 1);
        let mut strip = RgbaImage::from_pixel(w, row[0].height(), image::Rgba([255, 0, 255, 255]));
        let mut x = 0i64;
        for r in &row {
            image::imageops::overlay(&mut strip, r, x, 0);
            x += r.width() as i64 + 8;
        }
        sheet_rows.push(strip);
    }
    println!("slivers_400 even {} bold {}", slivers[0], slivers[1]);
    results.insert("slivers_400".into(), json!({"even": slivers[0], "bold": slivers[1]}));
    let w = sheet_rows.iter().map(|r| r.width()).max().unwrap();
    let h: u32 = sheet_rows.iter().map(|r| r.height() + 8).sum();
    let mut sheet = RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
    let mut y = 0i64;
    for r in &sheet_rows {
        image::imageops::overlay(&mut sheet, r, 0, y);
        y += r.height() as i64 + 8;
    }
    sheet.save(format!("{out}/{tag}_sheet_400.png")).unwrap();
    std::fs::write(format!("{out}/{tag}_results.json"), serde_json::to_string_pretty(&results).unwrap()).unwrap();

    // Characters unchanged: plain Even Out Lines on a rig.
    if a.get(3).map(String::as_str) == Some("guddu") {
        let doc = buzz_doc::Document::open(&a[4]).unwrap();
        let mut e = buzz_app::editor::Editor::new(doc);
        e.run(Command::EvenLines);
        e.finish_even_lines_now();
        let d = &e.even_lines_dialog;
        println!("guddu: {} face / {} body / {} solids", d.face_lines, d.body_lines, d.solids);
    }
}
