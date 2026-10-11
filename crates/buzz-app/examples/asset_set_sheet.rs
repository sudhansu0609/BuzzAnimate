//! **AS3/AS4 proof sheets.**
//!
//! ```text
//! cargo run -p buzz-app --example asset_set_sheet -- kinds <index.json> <out dir> sky,interior
//! cargo run -p buzz-app --example asset_set_sheet -- pieces <index.json> <out.png> [bush asset]
//! cargo run -p buzz-app --example asset_set_sheet -- scan <index.json>
//! ```
//!
//! `kinds`: contact sheets of the index records of those kinds, each with its
//! fill and mean colour (what the set builder reads). `scan`: every asset
//! that has parts (AS4), read only. `pieces` (AS4): three bushes placed
//! separately from the parts of one asset of loose shapes (default
//! `BushNew1`) and two figures placed alone from the symbols of
//! `BabaGroup2Latest1`, through the set builder's own placement
//! (`asset_sets::place_piece`), side by side on one stage; prints what each
//! placement brought (symbols, layers) against the whole asset. Reads the
//! user's library; writes only the PNG.

use std::path::Path;

use buzz_ai::asset_index::{AssetIndex, IndexRecord};
use buzz_ai::asset_sets::{SetPiece, SetRole};
use buzz_app::asset_index as idx;
use buzz_app::asset_sets;
use buzz_doc::AssetLibrary;
use buzz_geom::Size;
use buzz_render::GpuPreference;
use image::{Rgba, RgbaImage};

const CELL_W: u32 = 220;
const CELL_H: u32 = 250;
const PIC: u32 = 180;
const COLS: u32 = 8;
const BG: [u8; 3] = [0xF2, 0xF0, 0xEA];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("kinds") if args.len() >= 4 => kinds(Path::new(&args[1]), Path::new(&args[2]), &args[3]),
        Some("pieces") if args.len() >= 3 => {
            pieces(Path::new(&args[1]), Path::new(&args[2]), args.get(3).map_or("BushNew1", String::as_str))
        }
        Some("scan") if args.len() >= 2 => scan(Path::new(&args[1])),
        _ => anyhow::bail!("usage: asset_set_sheet kinds <index.json> <out dir> <kind,kind> | pieces <index.json> <out.png>"),
    }
}

fn kinds(index_path: &Path, out: &Path, kinds: &str) -> anyhow::Result<()> {
    let index = asset_sets::load_index_file(index_path).ok_or_else(|| anyhow::anyhow!("no index"))?;
    let dir = index_path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(out)?;
    for kind in kinds.split(',').map(str::trim).filter(|k| !k.is_empty()) {
        let recs: Vec<&IndexRecord> = index.records.iter().filter(|r| r.kind.as_str() == kind).collect();
        let cells: Vec<(Option<RgbaImage>, String, String)> = recs
            .iter()
            .map(|r| {
                let fill = r.fill.map(|f| format!("{f:.2}")).unwrap_or("-".into());
                let tone = r.tone.map(|t| format!("{} {} {}", t[0], t[1], t[2])).unwrap_or("-".into());
                let blank = if r.blank == Some(true) { " BLANK" } else { "" };
                (idx::load_thumb(dir, &r.thumb), r.name.clone(), format!("F{fill} C{tone}{blank}"))
            })
            .collect();
        sheet(&out.join(format!("kind-{kind}.png")), kind, &cells)?;
        eprintln!("{kind}: {} records", recs.len());
    }
    Ok(())
}

fn sheet(path: &Path, title: &str, cells: &[(Option<RgbaImage>, String, String)]) -> anyhow::Result<()> {
    let rows = (cells.len() as u32).div_ceil(COLS).max(1);
    let head = 30;
    let mut img = RgbaImage::from_pixel(COLS * CELL_W, head + rows * CELL_H, Rgba([BG[0], BG[1], BG[2], 255]));
    label(&mut img, &format!("{title}  {} ITEMS", cells.len()), 8, 8, 2);
    for (k, (pic, name, second)) in cells.iter().enumerate() {
        let (cx, cy) = ((k as u32 % COLS) * CELL_W, head + (k as u32 / COLS) * CELL_H);
        if let Some(pic) = pic {
            let small = image::imageops::resize(pic, PIC, PIC, image::imageops::FilterType::Triangle);
            let (ox, oy) = (cx + (CELL_W - PIC) / 2, cy + 4);
            for (x, y, px) in small.enumerate_pixels() {
                let a = px[3] as u32;
                let mix = |c: u8, b: u8| ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8;
                img.put_pixel(ox + x, oy + y, Rgba([mix(px[0], 255), mix(px[1], 255), mix(px[2], 255), 255]));
            }
        }
        let short: String = name.chars().take(17).collect();
        label(&mut img, &short, cx + 4, cy + PIC + 10, 2);
        let short2: String = second.chars().take(34).collect();
        label(&mut img, &short2, cx + 4, cy + PIC + 34, 1);
    }
    img.save(path)?;
    Ok(())
}

/// Every indexed asset (not the Cast) that has parts, with how many: what
/// the next index refresh will add (read only).
fn scan(index_path: &Path) -> anyhow::Result<()> {
    let index = asset_sets::load_index_file(index_path).ok_or_else(|| anyhow::anyhow!("no index"))?;
    let library = AssetLibrary::user();
    let root = library.root().ok_or_else(|| anyhow::anyhow!("no library"))?.to_path_buf();
    let (mut assets, mut with_parts, mut parts) = (0, 0, 0);
    for a in library.assets() {
        if buzz_ai::asset_index::is_cast(&a.folder) {
            continue;
        }
        let rel = buzz_ai::asset_index::relative(&root, &a.path);
        let Some(r) = index.asset_record(&rel) else { continue };
        let Ok(scene) = library.load(a) else { continue };
        assets += 1;
        let p = idx::distinct_parts(&scene);
        if !p.is_empty() {
            with_parts += 1;
            parts += p.len();
            println!("{}\t{}\t{}", a.name, r.kind.as_str(), p.len());
        }
    }
    println!("{assets} assets looked at, {with_parts} with parts, {parts} parts");
    Ok(())
}

/// The record of `asset` (by name), whole or one symbol of it.
fn find<'a>(index: &'a AssetIndex, asset: &str, symbol: Option<u64>) -> Option<&'a IndexRecord> {
    index
        .records
        .iter()
        .find(|r| buzz_ai::asset_index::stem_of(&r.path) == asset && r.symbol == symbol && r.part.is_none())
}

fn pieces(index_path: &Path, out: &Path, bush_name: &str) -> anyhow::Result<()> {
    let index = asset_sets::load_index_file(index_path).ok_or_else(|| anyhow::anyhow!("no index"))?;
    let library = AssetLibrary::user();
    let root = library.root().ok_or_else(|| anyhow::anyhow!("no library"))?.to_path_buf();

    // The bushes' parts, found now (the index gains them at its next
    // refresh); `bush_name` may list several assets, comma-separated.
    let mut records: Vec<IndexRecord> = Vec::new();
    for bush_name in bush_name.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let bush = find(&index, bush_name, None).ok_or_else(|| anyhow::anyhow!("{bush_name} not indexed"))?;
        let bush_asset = library
            .assets()
            .iter()
            .find(|a| buzz_ai::asset_index::relative(&root, &a.path) == bush.path)
            .ok_or_else(|| anyhow::anyhow!("{bush_name} not in the library"))?;
        let bush_scene = library.load(bush_asset)?;
        let parts = idx::distinct_parts(&bush_scene);
        eprintln!(
            "{bush_name}: {} symbols in its library, {} parts: {:?}",
            bush_scene.library().len(),
            parts.len(),
            parts
                .iter()
                .map(|(n, b)| format!("#{n} {:.0}x{:.0} at x {:.0}", b.width(), b.height(), b.x0))
                .collect::<Vec<_>>()
        );
        for (n, b) in parts.iter() {
            if records.len() >= 5 {
                break;
            }
            let mut r = bush.clone();
            r.id = format!("{}#part{n}", bush.path);
            r.part = Some(*n);
            r.name = format!("{bush_name} part {}", n + 1);
            r.bounds = [b.x0, b.y0, b.x1, b.y1];
            records.push(r);
        }
    }
    // BabaGroup2Latest1: two figures, each alone.
    for sym in [100u64, 102] {
        let r = find(&index, "BabaGroup2Latest1", Some(sym))
            .ok_or_else(|| anyhow::anyhow!("BabaGroup2Latest1#{sym} not indexed"))?;
        records.push(r.clone());
    }
    let whole_baba = {
        let r = find(&index, "BabaGroup2Latest1", None).ok_or_else(|| anyhow::anyhow!("no BabaGroup2Latest1"))?;
        asset_sets::record_scene(&library, r).map_err(anyhow::Error::msg)?
    };
    eprintln!("BabaGroup2Latest1 whole: {} symbols", whole_baba.library().len());

    let mut scene = buzz_scene::Scene::default();
    scene.stage_mut().size = Size::new(1920.0, 1080.0);
    let n = records.len() as f64;
    for (k, r) in records.iter().enumerate() {
        let before_syms = scene.library().len();
        let before_layers = scene.stage_layers().len();
        let piece = SetPiece {
            id: r.id.clone(),
            path: r.path.clone(),
            symbol: r.symbol,
            part: r.part,
            name: r.name.clone(),
            role: SetRole::Ground,
            x: (k as f64 + 0.5) / n,
            base: 0.82,
            // Each in its own fifth of the stage: no wider than 90 % of it.
            height: if r.symbol.is_some() {
                0.42
            } else {
                let aspect = (r.bounds[2] - r.bounds[0]) / (r.bounds[3] - r.bounds[1]).max(1e-6);
                (0.9 * 1920.0 / n / 1080.0 / aspect).min(0.3)
            },
            depth: 0.0,
            why: String::new(),
        };
        let layers = asset_sets::place_piece(&mut scene, &library, r, &piece, 540.0).map_err(anyhow::Error::msg)?;
        eprintln!(
            "placed {} ({}): +{} symbols, +{} layer(s) [{}]",
            r.name,
            r.id,
            scene.library().len() - before_syms,
            scene.stage_layers().len() - before_layers,
            layers.len()
        );
    }
    // A plain ground behind them, so white highlights read.
    let back = scene.add_stage_layer("Backdrop", buzz_scene::LayerKind::Normal);
    scene.add_shape(
        back,
        buzz_scene::ShapeData::filled(
            kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, 1920.0, 1080.0), 1e-9),
            peniko::Color::from_rgb8(0xB8, 0xC8, 0xD0),
        ),
    );
    let last = scene.stage_layers().len() - 1;
    scene.reorder_stage_layer(back, last);
    let mut settings = buzz_export::ExportSettings::for_stage(&scene);
    settings.width = 1920;
    settings.height = 1080;
    let mut exporter = buzz_export::Exporter::new(&GpuPreference::Automatic)?;
    let frame = exporter.render(&scene, 0, &settings)?;
    let mut img =
        RgbaImage::from_raw(frame.width, frame.height, frame.pixels).ok_or_else(|| anyhow::anyhow!("bad frame"))?;
    for (k, r) in records.iter().enumerate() {
        let x = ((k as f64 + 0.5) / n * 1920.0) as u32;
        label(&mut img, &r.name, x.saturating_sub(r.name.len() as u32 * 6), 1000, 2);
    }
    img.save(out)?;
    eprintln!("Wrote {}", out.display());
    Ok(())
}

/// A 5x7 bitmap font: each glyph is seven rows of five bits, high bit left.
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '&' => [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        _ => [0; 7],
    }
}

/// `text` in dark grey at `(x, y)`, each font pixel `scale` image pixels.
fn label(img: &mut RgbaImage, text: &str, x: u32, y: u32, scale: u32) {
    let ink = Rgba([0x30, 0x30, 0x38, 255]);
    for (k, c) in text.to_uppercase().chars().enumerate() {
        let rows = glyph(c);
        let gx = x + k as u32 * 6 * scale;
        for (ry, row) in rows.iter().enumerate() {
            for rx in 0..5u32 {
                if row & (0x10 >> rx) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (gx + rx * scale + dx, y + ry as u32 * scale + dy);
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, ink);
                        }
                    }
                }
            }
        }
    }
}
