//! Render one frame from an imported document for visual diagnostics.
//! cargo run -p buzz-app --example render_import -- input.fla output.png [frame]

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or_else(|| anyhow::anyhow!("missing input document"))?;
    let output = args.next().ok_or_else(|| anyhow::anyhow!("missing output PNG"))?;
    let frame = args.next().map(|v| v.parse::<u32>()).transpose()?.unwrap_or(0);
    let started = std::time::Instant::now();
    let mut imported = buzz_app::import::read(std::path::Path::new(&input))
        .map_err(anyhow::Error::msg)?;
    // `BUZZ_NO_CAMERA=1`: the stage as Animate's `exportPNG` draws it.
    if std::env::var("BUZZ_NO_CAMERA").is_ok() {
        imported.scene.camera_mut().enabled = false;
    }
    println!("Import: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    let started = std::time::Instant::now();
    let _bounds = imported.scene.cached_bounds_table();
    println!("Bounds: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    println!("{}", imported.summary);
    for warning in &imported.unsupported {
        println!("Not imported: {warning}");
    }
    // `BUZZ_RENDER_SCALE` overrides the output scale (default 0.5).
    let scale = std::env::var("BUZZ_RENDER_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
    let mut settings = buzz_export::ExportSettings::scaled(&imported.scene, scale);
    // S12c: `BUZZ_RENDER_REGION=x0,y0,x1,y1,zoom` re-renders that stage
    // rectangle at `zoom` output pixels per unit (a crop at extra zoom).
    if let Ok(v) = std::env::var("BUZZ_RENDER_REGION") {
        let n: Vec<f64> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        anyhow::ensure!(n.len() == 5, "BUZZ_RENDER_REGION needs x0,y0,x1,y1,zoom");
        settings.width = ((n[2] - n[0]) * n[4]).round() as u32;
        settings.height = ((n[3] - n[1]) * n[4]).round() as u32;
        settings.region = Some(buzz_geom::Rect::new(n[0], n[1], n[2], n[3]));
    }
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    let started = std::time::Instant::now();
    let picture = exporter.render(&imported.scene, frame, &settings)?;
    println!("First render: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    let started = std::time::Instant::now();
    let _ = exporter.render(&imported.scene, frame, &settings)?;
    println!("Warm render: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    picture.write_png(std::path::Path::new(&output))?;
    println!("Rendered frame {frame}: {} x {} -> {output}", picture.width, picture.height);
    Ok(())
}
