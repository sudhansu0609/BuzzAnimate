//! alaxmi_sheets: contact sheets of the production's plates, source picture over its traced layers.
//!
//!   alaxmi_sheets --assets <library root> --backgrounds <production>/backgrounds.json --out-dir <dir>
//!       [--series Khayal3Baje] [--first 1 --last 10]
//!
//! Writes `plates_sheet_<first>-<last>.png`: one row per story scene (key `village_sN`), three cells
//! (wide, `__low_angle`, `__high_angle`), each the source picture with the rendered trace under it.
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use image::{Rgba, RgbaImage, imageops};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut assets, mut bg, mut out_dir, mut series, mut first, mut last) = (None, None, None, "Khayal3Baje".to_string(), 1u32, 10u32);
    while let Some(flag) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{what} needs a value"));
        match flag.as_str() {
            "--assets" => assets = Some(PathBuf::from(value("--assets")?)),
            "--backgrounds" => bg = Some(PathBuf::from(value("--backgrounds")?)),
            "--out-dir" => out_dir = Some(PathBuf::from(value("--out-dir")?)),
            "--series" => series = value("--series")?,
            "--first" => first = value("--first")?.parse()?,
            "--last" => last = value("--last")?.parse()?,
            other => bail!("unknown flag {other}"),
        }
    }
    let assets = assets.context("--assets is required")?;
    let bg = bg.context("--backgrounds is required")?;
    let out_dir = out_dir.context("--out-dir is required")?;
    std::fs::create_dir_all(&out_dir)?;
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&bg)?)?;
    let map = value.get("backgrounds").unwrap_or(&value);
    let library = buzz_doc::AssetLibrary::at(&assets);
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    let (cw, ch) = (480u32, 270u32);
    let rows = last - first + 1;
    let mut sheet = RgbaImage::from_pixel(cw * 3 + 8, (ch * 2 + 22) * rows, Rgba([30, 30, 30, 255]));
    for (r, n) in (first..=last).enumerate() {
        for (c, suffix) in ["", "__low_angle", "__high_angle"].iter().enumerate() {
            let key = format!("village_s{n}{suffix}");
            let x = c as u32 * (cw + 4);
            let y = r as u32 * (ch * 2 + 22);
            let Some(entry) = map.get(&key) else {
                eprintln!("{key}: not in backgrounds.json");
                continue;
            };
            if let Some(png) = entry["png"].as_str() {
                if let Ok(img) = image::open(png) {
                    let small = imageops::resize(&img.to_rgba8(), cw, ch, imageops::FilterType::Triangle);
                    imageops::overlay(&mut sheet, &small, x as i64, (y + 11) as i64);
                }
            }
            let asset = library.assets().iter().find(|a| a.folder == format!("Backgrounds/{series}") && a.name == key);
            match asset {
                Some(a) => {
                    let scene = library.load(a).map_err(|e| anyhow!("{key}: {e}"))?;
                    let settings = buzz_export::ExportSettings::for_stage(&scene);
                    let f = exporter.render(&scene, 0, &settings)?;
                    let img = RgbaImage::from_raw(f.width, f.height, f.pixels).ok_or_else(|| anyhow!("bad frame"))?;
                    let small = imageops::resize(&img, cw, ch, imageops::FilterType::Triangle);
                    imageops::overlay(&mut sheet, &small, x as i64, (y + 11 + ch) as i64);
                }
                None => eprintln!("{key}: no traced asset in the library"),
            }
        }
    }
    let path = out_dir.join(format!("plates_sheet_{first}-{last}.png"));
    sheet.save(&path)?;
    println!("{}", path.display());
    Ok(())
}
