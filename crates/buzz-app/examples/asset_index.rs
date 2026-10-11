//! **AS1/AS2 headless**: build or refresh the asset index of a library, and
//! show what Arrange by type & style would do — without moving anything.
//!
//! ```text
//! cargo run --release -p buzz-app --example asset_index -- [--root <assets dir>]
//!     [--limit N] [--no-vlm] [--model <vlm>] [--no-build]
//!     [--plan <out-dir>] [--name-styles] [--symbols <out-dir> <asset name>...]
//! ```
//!
//! - default root: the user's library (`%APPDATA%/BuzzAnimate/assets`).
//! - `--limit N`: index at most N assets that need work this run.
//! - `--no-vlm`: name heuristics only (records marked `guessed`); otherwise
//!   LM Studio at 127.0.0.1:1234 with `--model` (default `google/gemma-4-12b`).
//! - `--plan <dir>`: a DRY RUN of the arrange — counts per destination folder
//!   and one contact sheet per Type (`<dir>/type-<Type>-p<N>.png`), plus
//!   `<dir>/plan.json`. Nothing is moved.
//! - `--name-styles`: before the plan, ask the VLM to name the style
//!   clusters (names are kept in the index).
//! - `--symbols <dir> <name>...`: a sheet per named asset of its per-symbol
//!   records (`<dir>/symbols-<name>.png`).
//! - `--apply` (with `--no-build`): MOVE every asset of the current plan,
//!   logged; `--undo` puts the last arrange back.
//!
//! Writes only the index (`<root>/.buzz-index.json`, `<root>/.buzz-index/`)
//! and the files under the output folders.

use std::path::{Path, PathBuf};

use buzz_ai::asset_index::{AssetIndex, IndexRecord, TYPE_FOLDERS};
use buzz_app::asset_index as idx;
use image::{Rgba, RgbaImage};

const CELL_W: u32 = 220;
const CELL_H: u32 = 250;
const PIC: u32 = 180;
const COLS: u32 = 8;
const ROWS: u32 = 6;
const BG: [u8; 3] = [0xF2, 0xF0, 0xEA];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = buzz_doc::assets::default_root();
    let mut limit = None;
    let mut vlm = true;
    let mut build = true;
    let mut model = "google/gemma-4-12b".to_string();
    let mut plan_dir: Option<PathBuf> = None;
    let mut name_styles = false;
    let mut symbols: Option<(PathBuf, Vec<String>)> = None;
    let mut apply = false;
    let mut undo = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                root = PathBuf::from(&args[i]);
            }
            "--limit" => {
                i += 1;
                limit = Some(args[i].parse()?);
            }
            "--no-vlm" => vlm = false,
            "--no-build" => build = false,
            "--model" => {
                i += 1;
                model = args[i].clone();
            }
            "--plan" => {
                i += 1;
                plan_dir = Some(PathBuf::from(&args[i]));
            }
            "--name-styles" => name_styles = true,
            "--apply" => apply = true,
            "--undo" => undo = true,
            "--symbols" => {
                i += 1;
                let dir = PathBuf::from(&args[i]);
                let names = args[i + 1..].to_vec();
                i = args.len();
                symbols = Some((dir, names));
                continue;
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
        i += 1;
    }
    let backend = buzz_ai::llm::Backend::default_openai();

    if build {
        let opts = idx::BuildOptions {
            limit,
            vlm: vlm.then(|| (backend.clone(), model.clone())),
        };
        let t = std::time::Instant::now();
        let mut last = String::new();
        let report = idx::build_index(
            &root,
            &opts,
            &mut |done, total, what| {
                let line = format!("[{done}/{total}] {what}");
                if what.starts_with("Identifying") || done % 50 == 0 {
                    if line != last {
                        eprintln!("{line}");
                        last = line;
                    }
                }
            },
            &|| false,
        )?;
        let secs = t.elapsed().as_secs_f64();
        println!("index: {}", AssetIndex::path_in(&root).display());
        println!(
            "assets {} (rendered {}, kept {}, failed {}), records {} ({} symbols inside assets), guessed {}",
            report.assets_total,
            report.assets_rendered,
            report.assets_kept,
            report.assets_failed.len(),
            report.records,
            report.symbol_records,
            report.guessed
        );
        for f in report.assets_failed.iter().take(20) {
            println!("  failed: {f}");
        }
        println!(
            "render {:.1} s, SigLIP {:.1} s, total {:.1} s",
            report.render_seconds, report.embed_seconds, secs
        );
        if let Some(e) = &report.embed_error {
            println!("SigLIP: {e}");
        }
        if !report.vlm_batches.is_empty() {
            let mean = report.vlm_batches.iter().sum::<f64>() / report.vlm_batches.len() as f64;
            println!(
                "VLM: {} batches, {} records identified, mean {:.1} s/batch; per batch: {}",
                report.vlm_batches.len(),
                report.vlm_records,
                mean,
                report.vlm_batches.iter().map(|s| format!("{s:.1}")).collect::<Vec<_>>().join(" ")
            );
        }
        if let Some(e) = &report.vlm_error {
            println!("VLM: {e}");
        }
    }

    let mut library = buzz_doc::AssetLibrary::at(&root);
    // **Apply / undo**, the same functions the Arrange dialog calls: every
    // move of the current plan, logged so `--undo` can put it all back.
    if undo {
        let back = buzz_ai::asset_index::undo_arrange(&mut library).map_err(anyhow::Error::msg)?;
        println!("UNDO: {back} file(s) put back");
        return Ok(());
    }
    if apply {
        let plan = idx::plan_for(&library);
        println!("APPLY: {} planned moves, {} Cast assets kept", plan.moves.len(), plan.cast_kept);
        let log = buzz_ai::asset_index::apply_arrange(&mut library, &plan.moves, |n, total| {
            if n % 100 == 0 {
                eprintln!("[{n}/{total}]");
            }
        })
        .map_err(anyhow::Error::msg)?;
        println!("APPLIED: {} file(s) moved, {} folder(s) made", log.moved.len(), log.created_folders.len());
        return Ok(());
    }
    if name_styles {
        let plan = idx::plan_for(&library);
        match idx::name_styles(&root, &plan, &backend, &model, &mut |_, _, _| {}, &|| false) {
            Ok(n) => println!("named {n} style clusters"),
            Err(e) => println!("style naming stopped: {e}"),
        }
    }

    if let Some(dir) = plan_dir {
        std::fs::create_dir_all(&dir)?;
        let plan = idx::plan_for(&library);
        let index = AssetIndex::load(&root);
        println!(
            "DRY RUN: {} moves, {} Cast assets kept, {} unindexed",
            plan.moves.len(),
            plan.cast_kept,
            plan.unindexed
        );
        for (folder, n) in plan.counts() {
            println!("  {folder}: {n}");
        }
        let json: Vec<serde_json::Value> = plan
            .moves
            .iter()
            .map(|m| {
                let r = index.asset_record(&m.path);
                serde_json::json!({
                    "path": m.path, "from": m.from, "to": m.to, "guessed": m.guessed,
                    "kind": r.map(|r| r.kind.as_str()), "label": r.and_then(|r| r.label.clone()),
                })
            })
            .collect();
        std::fs::write(
            dir.join("plan.json"),
            serde_json::to_string_pretty(&serde_json::json!({"counts": plan.counts(), "moves": json}))?,
        )?;
        for ty in TYPE_FOLDERS {
            let moves: Vec<&buzz_ai::asset_index::ArrangeMove> =
                plan.moves.iter().filter(|m| m.type_folder == ty).collect();
            if moves.is_empty() {
                continue;
            }
            let cells: Vec<(Option<RgbaImage>, String, String)> = moves
                .iter()
                .map(|m| {
                    let style = if m.style.is_empty() { String::new() } else { m.style.clone() };
                    let flag = if m.guessed { " ?" } else { "" };
                    (idx::load_thumb(&root, &m.thumb), m.name.clone(), format!("{style}{flag}"))
                })
                .collect();
            let safe = ty.replace(['/', ' ', '&'], "_");
            let pages = write_pages(&dir, &format!("type-{safe}"), ty, &cells)?;
            println!("  sheet {ty}: {pages} page(s)");
        }
    }

    if let Some((dir, names)) = symbols {
        std::fs::create_dir_all(&dir)?;
        let index = AssetIndex::load(&root);
        for name in names {
            let Some(asset) = index.records.iter().find(|r| r.symbol.is_none() && r.name == name) else {
                println!("{name}: not in the index");
                continue;
            };
            let mut cells = vec![cell_of(&root, asset, "WHOLE ASSET")];
            let syms: Vec<&IndexRecord> = index.symbol_records(&asset.path).collect();
            for r in &syms {
                cells.push(cell_of(&root, r, ""));
            }
            println!("{name}: {} symbol records", syms.len());
            for r in &syms {
                println!(
                    "  #{} {} kind {} view {} guessed {} bounds {:.0}x{:.0} tags {:?}",
                    r.symbol.unwrap_or(0),
                    r.name,
                    r.kind.as_str(),
                    r.view.as_str(),
                    r.guessed,
                    r.bounds[2] - r.bounds[0],
                    r.bounds[3] - r.bounds[1],
                    r.tags
                );
            }
            write_pages(&dir, &format!("symbols-{name}"), &name, &cells)?;
        }
    }
    Ok(())
}

fn cell_of(root: &Path, r: &IndexRecord, extra: &str) -> (Option<RgbaImage>, String, String) {
    let flag = if r.guessed { " ?" } else { "" };
    let second = if extra.is_empty() {
        format!("{} {}{flag}", r.kind.as_str(), r.view.as_str())
    } else {
        format!("{extra} {}{flag}", r.kind.as_str())
    };
    (idx::load_thumb(root, &r.thumb), r.name.clone(), second)
}

/// Pages of `COLS`×`ROWS` cells; returns how many.
fn write_pages(
    dir: &Path,
    stem: &str,
    title: &str,
    cells: &[(Option<RgbaImage>, String, String)],
) -> anyhow::Result<usize> {
    let per = (COLS * ROWS) as usize;
    let pages = cells.len().div_ceil(per).max(1);
    for (p, chunk) in cells.chunks(per).enumerate() {
        let rows = (chunk.len() as u32).div_ceil(COLS);
        let head = 30;
        let mut sheet = RgbaImage::from_pixel(COLS * CELL_W, head + rows * CELL_H, Rgba([BG[0], BG[1], BG[2], 255]));
        label(&mut sheet, &format!("{title}  {} ITEMS  PAGE {}/{pages}", cells.len(), p + 1), 8, 8, 2);
        for (k, (pic, name, second)) in chunk.iter().enumerate() {
            let (cx, cy) = ((k as u32 % COLS) * CELL_W, head + (k as u32 / COLS) * CELL_H);
            for x in cx..cx + CELL_W {
                sheet.put_pixel(x, cy, Rgba([0xC8, 0xC4, 0xBC, 255]));
            }
            for y in cy..cy + CELL_H {
                sheet.put_pixel(cx, y, Rgba([0xC8, 0xC4, 0xBC, 255]));
            }
            if let Some(pic) = pic {
                let small = image::imageops::resize(pic, PIC, PIC, image::imageops::FilterType::Triangle);
                let (ox, oy) = (cx + (CELL_W - PIC) / 2, cy + 4);
                for (x, y, px) in small.enumerate_pixels() {
                    let a = px[3] as u32;
                    let mix = |c: u8, b: u8| ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8;
                    sheet.put_pixel(
                        ox + x,
                        oy + y,
                        Rgba([mix(px[0], 255), mix(px[1], 255), mix(px[2], 255), 255]),
                    );
                }
            }
            let short: String = name.chars().take(17).collect();
            label(&mut sheet, &short, cx + 4, cy + PIC + 10, 2);
            let short2: String = second.chars().take(34).collect();
            label(&mut sheet, &short2, cx + 4, cy + PIC + 34, 1);
        }
        let file = if pages == 1 { format!("{stem}.png") } else { format!("{stem}-p{}.png", p + 1) };
        sheet.save(dir.join(file))?;
    }
    Ok(pages)
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
