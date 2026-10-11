//! F7.5 regression: one `buzz_export::Exporter` reused across the whole
//! stage, 256² symbol thumbnails and back, on a real document.
//! `cargo run --release -p buzz-app --example exporter_reuse -- <fla>`
use std::path::Path;
fn main() -> anyhow::Result<()> {
    let fla = std::env::args().nth(1).expect("fla path");
    let scene = buzz_app::import::read(Path::new(&fla)).map_err(anyhow::Error::msg)?.scene;
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?;
    let syms: Vec<_> = scene.library().iter().take(12).cloned().collect();
    if std::env::var("FIRST").is_ok() {
        let png = buzz_ui::ai_rename_dialog::render_symbol_thumbnail(&mut ex, &scene, &syms[2]);
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        let lit = img.pixels().filter(|p| p[0] > 20 || p[1] > 20 || p[2] > 20).count();
        println!("first-thumbnail {} lit px {lit}", syms[2].name);
    }
    for round in 0..3 {
        let s = buzz_export::ExportSettings::scaled(&scene, 0.5);
        let f = ex.render(&scene, 0, &s)?;
        println!("round {round}: stage {}x{}", f.width, f.height);
        for sym in &syms {
            let png = buzz_ui::ai_rename_dialog::render_symbol_thumbnail(&mut ex, &scene, sym);
            assert!(png.len() > 60, "thumbnail of {} is the fallback", sym.name);
            if let Ok(dir) = std::env::var("THUMBS") {
                let img = image::load_from_memory(&png).unwrap().to_rgba8();
                let opaque = img.pixels().filter(|p| p[3] > 0).count();
                println!("  {} {}x{} opaque {opaque}", sym.name, img.width(), img.height());
                let _ = img.save(std::path::Path::new(&dir).join(format!("t{round}_{}.png", sym.id.0)));
            }
        }
        let s = buzz_export::ExportSettings::for_stage(&scene);
        let f = ex.render(&scene, scene.frame_count() / 2, &s)?;
        println!("round {round}: full {}x{}; {} thumbnails ok", f.width, f.height, syms.len());
    }
    println!("OK");
    Ok(())
}
