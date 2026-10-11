//! zz_buzz_sheet doc.buzz out_dir stride [scale] — render every `stride`th frame
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (scenes, active) = buzz_doc::format::load_scenes(&a[0]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let stride: u32 = a[2].parse()?;
    let scale: f64 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(0.15);
    let only: Option<u32> = a.get(4).and_then(|v| v.parse().ok());
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    for (i, (name, scene)) in scenes.iter().enumerate() {
        println!("scene {i} {name:?}{} frames={} layers={}", if i == active { " (active)" } else { "" }, scene.frame_count(), scene.layers().len());
        if i != active { continue; }
        let settings = buzz_export::ExportSettings::scaled(scene, scale);
        let mut f = 0;
        while f < scene.frame_count() {
            if only.map_or(true, |o| o == f) {
                let p = exporter.render(scene, f, &settings)?;
                p.write_png(std::path::Path::new(&format!("{}/f{:05}.png", a[1], f)))?;
            }
            f += stride;
        }
    }
    Ok(())
}
