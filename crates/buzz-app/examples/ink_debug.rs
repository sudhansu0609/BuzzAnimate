//! Per-shape statistics of an Ink trace, for debugging holes and fit quality.
//!
//! cargo run --release -p buzz-app --example ink_debug -- in.png [bold] [--tuning <file>] [--bold <px>]
//!
//! The tracer tuning is the saved one (`trace_tuning.json`, or
//! `BUZZ_TRACE_TUNING`), or `--tuning <file>`; `[bold]` / `--bold` override
//! `ink_bold` (default: the options' own, 0).

use buzz_geom::{PathEl, Point};

fn main() -> anyhow::Result<()> {
    // `--tuning <file>` and `--bold <px>` anywhere; the rest is positional.
    let mut tuning_file: Option<String> = None;
    let mut bold_flag: Option<f64> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(a) = raw.next() {
        match a.as_str() {
            "--tuning" => tuning_file = Some(raw.next().ok_or_else(|| anyhow::anyhow!("--tuning needs a file"))?),
            "--bold" => bold_flag = Some(raw.next().ok_or_else(|| anyhow::anyhow!("--bold needs px"))?.parse()?),
            _ => positional.push(a),
        }
    }
    let mut args = positional.into_iter();
    let input = args.next().ok_or_else(|| anyhow::anyhow!("usage: ink_debug <in.png> [bold] [--tuning <file>] [--bold <px>]"))?;
    let bold: Option<f64> = args.next().map(|v| v.parse()).transpose()?;
    // Optional: `inkcolour` to trace with `TraceOptions::ink_colour()`, and a
    // `x0,y0,x1,y1` box to list only the shapes whose bounds lie inside it.
    let mut mode = String::from("ink");
    let mut only: Option<buzz_geom::Rect> = None;
    for a in args {
        if a == "inkcolour" || a == "ink" {
            mode = a;
        } else {
            let n: Vec<f64> = a.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if n.len() == 4 {
                only = Some(buzz_geom::Rect::new(n[0], n[1], n[2], n[3]));
            }
        }
    }
    let bytes = std::fs::read(&input)?;
    let img = buzz_scene::ImageAsset::decode(buzz_scene::ImageId(1), "input", &bytes)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut options =
        if mode == "inkcolour" { buzz_scene::TraceOptions::ink_colour() } else { buzz_scene::TraceOptions::ink() }
            .with_saved_tuning();
    if let Some(file) = &tuning_file {
        options.tuning = buzz_scene::TraceTuning::load(std::path::Path::new(file)).map_err(|e| anyhow::anyhow!(e))?;
    }
    if let Some(bold) = bold_flag.or(bold) {
        options.ink_bold = bold;
    }
    let started = std::time::Instant::now();
    let report = buzz_scene::trace(img.width, img.height, &img.pixels, &options);
    println!("{} in {:.0} ms", report.message, started.elapsed().as_secs_f64() * 1e3);
    for (i, shape) in report.shapes.iter().enumerate() {
        if let Some(b) = only {
            let bb = buzz_geom::Shape::bounding_box(&shape.path);
            if !(bb.x0 >= b.x0 && bb.y0 >= b.y0 && bb.x1 <= b.x1 && bb.y1 <= b.y1) {
                continue;
            }
            let class = if i < report.fills {
                "fill"
            } else if i < report.fills + report.strokes {
                "colour line"
            } else {
                "ink"
            };
            println!(
                "{class} {i}: {:?} area {:.0} bbox ({:.0},{:.0})-({:.0},{:.0})",
                shape.fill.as_ref().map(|f| f.color().to_rgba8().to_u8_array()),
                buzz_geom::Shape::area(&shape.path).abs(),
                bb.x0,
                bb.y0,
                bb.x1,
                bb.y1
            );
            continue;
        }
        let colour = shape.fill.as_ref().map(|f| format!("{:?}", f.paint)).unwrap_or_default();
        // Split into subpaths and measure each one's signed area and element count.
        let mut subpaths: Vec<(f64, usize, usize)> = Vec::new(); // (signed area, elements, lines)
        let mut cur: Vec<Point> = Vec::new();
        let mut els = 0usize;
        let mut lines = 0usize;
        let flush = |cur: &mut Vec<Point>, els: &mut usize, lines: &mut usize, out: &mut Vec<(f64, usize, usize)>| {
            if cur.len() >= 3 {
                let mut a = 0.0;
                for j in 0..cur.len() {
                    let p = cur[j];
                    let q = cur[(j + 1) % cur.len()];
                    a += p.x * q.y - q.x * p.y;
                }
                out.push((a / 2.0, *els, *lines));
            }
            cur.clear();
            *els = 0;
            *lines = 0;
        };
        for el in shape.path.elements() {
            match el {
                PathEl::MoveTo(p) => {
                    flush(&mut cur, &mut els, &mut lines, &mut subpaths);
                    cur.push(*p);
                    els += 1;
                }
                PathEl::LineTo(p) => {
                    cur.push(*p);
                    els += 1;
                    lines += 1;
                }
                PathEl::QuadTo(_, p) => {
                    cur.push(*p);
                    els += 1;
                }
                PathEl::CurveTo(_, _, p) => {
                    cur.push(*p);
                    els += 1;
                }
                PathEl::ClosePath => {}
            }
        }
        flush(&mut cur, &mut els, &mut lines, &mut subpaths);
        let positive = subpaths.iter().filter(|s| s.0 > 0.0).count();
        let negative = subpaths.len() - positive;
        let total_els: usize = subpaths.iter().map(|s| s.1).sum();
        let total_lines: usize = subpaths.iter().map(|s| s.2).sum();
        println!(
            "shape {i}: {colour} subpaths {} (+{positive} / -{negative}) elements {total_els} (lines {total_lines}) bbox {:?}",
            subpaths.len(),
            buzz_geom::Shape::bounding_box(&shape.path)
        );
        if i < 3 {
            let mut sorted = subpaths.clone();
            sorted.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
            for s in sorted.iter().take(24) {
                println!("    area {:>12.1}  elements {:>5}  lines {:>5}", s.0, s.1, s.2);
            }
        }
    }
    Ok(())
}
