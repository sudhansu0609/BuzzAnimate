//! scratch probe (TRACE_PICTURE_PLAN T12/T12b/T12c) — Even Out Lines through the
//! headless Editor the way the dialog drives it: the work runs on another
//! thread, the UI side is timed; then white-gap measures on 1x and 400 % crops.
//! Usage: zz_even_async_probe <file.buzz> <out dir>
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

fn luma(p: &image::Rgba<u8>) -> f64 {
    (0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64) / 255.0
}

/// Pixels that were dark and are now near white (`white`), and that were
/// opaque and are now see-through (`clear`), on opaque-white renders / alpha.
fn gaps(before_white: &RgbaImage, after_white: &RgbaImage, before_a: &RgbaImage, after_a: &RgbaImage) -> (usize, usize) {
    let (mut white, mut clear) = (0, 0);
    for (b, a) in before_white.pixels().zip(after_white.pixels()) {
        if luma(b) < 0.5 && luma(a) > 0.9 {
            white += 1;
        }
    }
    for (b, a) in before_a.pixels().zip(after_a.pixels()) {
        if b[3] > 127 && a[3] <= 127 {
            clear += 1;
        }
    }
    (white, clear)
}

fn side_by_side(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(a.width() + b.width() + 8, a.height().max(b.height()), image::Rgba([255, 0, 255, 255]));
    image::imageops::overlay(&mut out, a, 0, 0);
    image::imageops::overlay(&mut out, b, a.width() as i64 + 8, 0);
    out
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = &a[1];
    std::fs::create_dir_all(out).unwrap();
    let doc = buzz_doc::Document::open(&a[0]).unwrap();
    let mut e = buzz_app::editor::Editor::new(doc);
    let before = e.scene().clone();

    // Open: the UI thread only queues the work.
    let t = Instant::now();
    e.run(Command::EvenLines);
    let ui_return_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (g, work) = e.even_lines_work.take().expect("work queued");
    let working_flag = e.even_lines_dialog.working;
    let t_open = Instant::now();
    let handle = std::thread::spawn(move || buzz_app::editor::run_even_lines_work(work, &|_| {}));
    let mut idle_frames = 0;
    while !handle.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        idle_frames += 1;
    }
    let done = handle.join().unwrap();
    let result_ready_s = t_open.elapsed().as_secs_f64();
    let t = Instant::now();
    e.finish_even_lines(g, done);
    let first_preview_ms = t.elapsed().as_secs_f64() * 1000.0;
    let d = &e.even_lines_dialog;
    let (has_face, has_body, lines) = (d.has_face, d.has_body, d.body_lines + d.face_lines);

    // A slider drag: draft retraces, then the release.
    let mut frame_max = first_preview_ms;
    let mut drag_trace_ms = Vec::new();
    for (i, draft) in [(1, true), (2, true), (3, true), (3, false)] {
        e.even_lines_dialog.solid_above += 0.5 * i as f64;
        let t = Instant::now();
        e.even_lines_action(EvenLinesAction::Retrace { draft });
        frame_max = frame_max.max(t.elapsed().as_secs_f64() * 1000.0);
        let (g, work) = e.even_lines_work.take().expect("retrace queued");
        let t_w = Instant::now();
        let handle = std::thread::spawn(move || buzz_app::editor::run_even_lines_work(work, &|_| {}));
        let done = handle.join().unwrap();
        drag_trace_ms.push((t_w.elapsed().as_secs_f64() * 1000.0).round());
        let t = Instant::now();
        e.finish_even_lines(g, done);
        frame_max = frame_max.max(t.elapsed().as_secs_f64() * 1000.0);
    }
    let t = Instant::now();
    e.even_lines_action(EvenLinesAction::Apply);
    let apply_ms = t.elapsed().as_secs_f64() * 1000.0;
    let after = e.scene().clone();
    let shapes = |s: &buzz_scene::Scene| s.layers().iter().map(|l| l.frames.keyframes().iter().map(|k| k.objects.len()).sum::<usize>()).sum::<usize>();

    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let full = before.content_bounds().unwrap_or(Rect::new(0.0, 0.0, 1920.0, 1080.0));
    let mut results = serde_json::Map::new();
    let mut sheet_rows: Vec<RgbaImage> = Vec::new();
    let crops = [
        ("whole", full, 1.0),
        ("a", Rect::new(full.x0 + 100.0, full.y0 + 300.0, full.x0 + 420.0, full.y0 + 480.0), 4.0),
        ("b", Rect::new(full.x0 + 900.0, full.y0 + 500.0, full.x0 + 1220.0, full.y0 + 680.0), 4.0),
        ("c", Rect::new(full.x0 + 1500.0, full.y0 + 150.0, full.x0 + 1820.0, full.y0 + 330.0), 4.0),
    ];
    for (name, region, scale) in crops {
        let bw = render(&mut ex, &before, region, scale, false);
        let aw = render(&mut ex, &after, region, scale, false);
        let ba = render(&mut ex, &before, region, scale, true);
        let aa = render(&mut ex, &after, region, scale, true);
        let (white, clear) = gaps(&bw, &aw, &ba, &aa);
        results.insert(name.to_string(), json!({"scale": scale, "turned_white": white, "turned_clear": clear}));
        let pair = side_by_side(&bw, &aw);
        pair.save(format!("{out}/{name}_pair.png")).unwrap();
        if name != "whole" {
            sheet_rows.push(pair);
        }
    }
    let w = sheet_rows.iter().map(|r| r.width()).max().unwrap_or(1);
    let h: u32 = sheet_rows.iter().map(|r| r.height() + 8).sum();
    let mut sheet = RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 255, 255]));
    let mut y = 0;
    for r in &sheet_rows {
        image::imageops::overlay(&mut sheet, r, 0, y);
        y += r.height() as i64 + 8;
    }
    sheet.save(format!("{out}/sheet_400.png")).unwrap();

    println!(
        "{}",
        json!({
            "ui_return_ms": ui_return_ms,
            "working_flag_after_open": working_flag,
            "result_ready_s": result_ready_s,
            "idle_frames_while_working": idle_frames,
            "first_preview_ui_ms": first_preview_ms,
            "drag_frame_ms_max": frame_max,
            "drag_trace_ms": drag_trace_ms,
            "apply_ms": apply_ms,
            "has_face": has_face, "has_body": has_body, "lines": lines,
            "objects_before": shapes(&before), "objects_after": shapes(&after),
            "gaps": results,
        })
    );
}
