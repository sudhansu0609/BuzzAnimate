//! Trace a PNG and render the vector result beside the source, for judging
//! trace quality by eye and by numbers.
//!
//! cargo run --release -p buzz-app --example trace_png -- in.png out.png [lineart|outline|both|ink|inkcolour] [uniform_width_px] [--tuning <file>] [--bold <px>]
//!
//! The tracer tuning is the saved one (`trace_tuning.json`, or
//! `BUZZ_TRACE_TUNING`), or `--tuning <file>`; `--bold` overrides `ink_bold`.
//!
//! `out.png` is the traced vectors alone, on white, at the source's size
//! times env `TRACE_PNG_SCALE` (default 1; e.g. 4 for judging 4× crops).
//! Prints the stroke and fill counts, and the time taken.

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
    let usage = "usage: trace_png <in.png> <out.png> [lineart|outline|both|ink|inkcolour] [uniform_width_px] [--tuning <file>] [--bold <px>]";
    let input = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let output = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let mode = args.next().unwrap_or_else(|| "lineart".into());
    let uniform: Option<f64> = args.next().map(|v| v.parse()).transpose()?;
    let scale: f64 = match std::env::var("TRACE_PNG_SCALE") {
        Ok(v) => v.parse()?,
        Err(_) => 1.0,
    };

    let bytes = std::fs::read(&input)?;
    let img = buzz_scene::ImageAsset::decode(buzz_scene::ImageId(1), "input", &bytes)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let (w, h) = (img.width, img.height);
    // `colour[:N]` is the default colour trace with N colours (default 16).
    let mut options = if mode == "ink" {
        buzz_scene::TraceOptions::ink()
    } else if mode == "inkcolour" {
        buzz_scene::TraceOptions::ink_colour()
    } else if mode.starts_with("colour") {
        let mut o = buzz_scene::TraceOptions::default();
        o.colours = mode.split(':').nth(1).and_then(|n| n.parse().ok()).unwrap_or(16);
        o
    } else {
        buzz_scene::TraceOptions::line_art()
    };
    options.mode = match mode.as_str() {
        "ink" => buzz_scene::TraceMode::Ink,
        "inkcolour" => buzz_scene::TraceMode::InkAndColour,
        "auto" => buzz_scene::TraceMode::Auto,
        "outline" => buzz_scene::TraceMode::Outline,
        m if m.starts_with("colourlines") => buzz_scene::TraceMode::ColourLines,
        m if m.starts_with("colour") => buzz_scene::TraceMode::Outline,
        "both" => buzz_scene::TraceMode::Both,
        _ => buzz_scene::TraceMode::LineArt,
    };
    if let Some(width) = uniform {
        options.line_style = buzz_scene::LineStyle::Stroke;
        options.uniform_width = Some(width);
    }
    options = options.with_saved_tuning();
    if let Some(file) = &tuning_file {
        options.tuning = buzz_scene::TraceTuning::load(std::path::Path::new(file)).map_err(|e| anyhow::anyhow!(e))?;
    }
    if let Some(bold) = bold_flag {
        options.ink_bold = bold;
    }

    let started = std::time::Instant::now();
    let report = buzz_scene::trace(w, h, &img.pixels, &options);
    println!(
        "traced {w}x{h} in {:.0} ms: {} shapes ({} strokes, {} fills) {}",
        started.elapsed().as_secs_f64() * 1e3,
        report.shapes.len(),
        report.strokes,
        report.fills,
        report.message
    );

    // The vectors on a white stage the size of the picture, one pixel to one
    // document unit, so the two halves line up. The vectors stay in source
    // pixels; the exporter renders them `scale`× larger.
    // `TRACE_PNG_CROP=x0,y0,x1,y1` (source pixels) renders only that window,
    // so a 4× crop fits the exporter's canvas limit.
    let crop: Option<(f64, f64, f64, f64)> = std::env::var("TRACE_PNG_CROP").ok().and_then(|v| {
        let n: Vec<f64> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        (n.len() == 4).then(|| (n[0], n[1], n[2], n[3]))
    });
    let (stage_w, stage_h, shift) = match crop {
        Some((x0, y0, x1, y1)) => (x1 - x0, y1 - y0, buzz_geom::Affine::translate((-x0, -y0))),
        None => (w as f64, h as f64, buzz_geom::Affine::IDENTITY),
    };
    let mut scene = buzz_scene::Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(stage_w, stage_h);
    // `BUZZ_EXAMPLE_BG=#rrggbb` paints the stage that colour instead of white
    // (to see light fills that spill outside the outlines).
    scene.stage_mut().background = std::env::var("BUZZ_EXAMPLE_BG")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok())
        .map(|v| peniko::Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
        .unwrap_or(peniko::Color::WHITE);
    let layer = scene.add_layer("trace", buzz_scene::LayerKind::Normal);
    for (i, mut shape) in report.shapes.into_iter().enumerate() {
        shape.path = shift * shape.path;
        scene.add_object(layer, buzz_scene::Object::shape(buzz_scene::ObjectId(1_000_000 + i as u64), shape));
    }
    let settings = buzz_export::ExportSettings::scaled(&scene, scale);
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    let traced = exporter.render(&scene, 0, &settings)?;

    traced.write_png(std::path::Path::new(&output))?;
    println!("-> {output}");

    // The metrics compare pixel for pixel with the source, so only at 1×.
    if scale != 1.0 || crop.is_some() {
        return Ok(());
    }

    // Compute metrics as specified in TRACE_QUALITY_PLAN
    // The metric functions live in `buzz_scene::trace_metrics`, shared with `rig_vs_source`.
    use buzz_scene::trace_metrics::{compute_edge_distance, compute_mean_delta_e, compute_small_features};
    let mean_de = compute_mean_delta_e(&img.pixels, &traced.pixels, w as usize, h as usize);
    let (edge_mean, edge_p95) = compute_edge_distance(&img.pixels, &traced.pixels, w as usize, h as usize);
    let (sm_matched, sm_total, sm_pct) = compute_small_features(&img.pixels, &traced.pixels, w as usize, h as usize);

    println!(
        "metrics: mean ΔE = {:.2}, edge distance mean = {:.2} px, p95 = {:.2} px, small features = {}/{} ({:.1}%), time = {:.0} ms",
        mean_de,
        edge_mean,
        edge_p95,
        sm_matched,
        sm_total,
        sm_pct,
        started.elapsed().as_secs_f64() * 1e3,
    );

    Ok(())
}

