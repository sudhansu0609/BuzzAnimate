//! **Cast from FLA** (ALAXMI_PRODUCTION_PLAN B1a).
//!
//! `cast_from_fla --fla <cast.fla> [--series Khayal3Baje] [--root <library root>] [--roles DEEPAK,RADHE]
//!  [--ai-rig] [--sheet <out.png>] [--dry]`
//!
//! Imports each role wrapper of the FLA as locked Cast assets under
//! `Cast/<series>/` (backing the Cast folder up first), writes the series'
//! `casting.json`, prints the report as JSON. `--sheet` writes a contact sheet
//! of every role x view and one walk frame per cycle, loaded back from disk.
//! `--dry` only lists what the FLA holds. `--sheet-only` skips the import.
//! The FLA is only read.

use std::path::PathBuf;

use buzz_app::cast_fla::{self, View};
use buzz_doc::AssetLibrary;
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;
use image::{Rgba, RgbaImage};

const CELL_W: u32 = 200;
const CELL_H: u32 = 280;
const LABEL_H: u32 = 22;

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn text(img: &mut RgbaImage, s: &str, x0: u32, y0: u32) {
    // 3x5 digits/letters are not worth a font here: draw with a tiny block font for caps/digits only.
    const FONT: &[(char, [u8; 5])] = &[
        ('A', [0b010, 0b101, 0b111, 0b101, 0b101]), ('B', [0b110, 0b101, 0b110, 0b101, 0b110]),
        ('C', [0b011, 0b100, 0b100, 0b100, 0b011]), ('D', [0b110, 0b101, 0b101, 0b101, 0b110]),
        ('E', [0b111, 0b100, 0b110, 0b100, 0b111]), ('F', [0b111, 0b100, 0b110, 0b100, 0b100]),
        ('G', [0b011, 0b100, 0b101, 0b101, 0b011]), ('H', [0b101, 0b101, 0b111, 0b101, 0b101]),
        ('I', [0b111, 0b010, 0b010, 0b010, 0b111]), ('J', [0b001, 0b001, 0b001, 0b101, 0b010]),
        ('K', [0b101, 0b101, 0b110, 0b101, 0b101]), ('L', [0b100, 0b100, 0b100, 0b100, 0b111]),
        ('M', [0b101, 0b111, 0b111, 0b101, 0b101]), ('N', [0b110, 0b101, 0b101, 0b101, 0b101]),
        ('O', [0b010, 0b101, 0b101, 0b101, 0b010]), ('P', [0b110, 0b101, 0b110, 0b100, 0b100]),
        ('Q', [0b010, 0b101, 0b101, 0b111, 0b011]), ('R', [0b110, 0b101, 0b110, 0b101, 0b101]),
        ('S', [0b011, 0b100, 0b010, 0b001, 0b110]), ('T', [0b111, 0b010, 0b010, 0b010, 0b010]),
        ('U', [0b101, 0b101, 0b101, 0b101, 0b111]), ('V', [0b101, 0b101, 0b101, 0b101, 0b010]),
        ('W', [0b101, 0b101, 0b111, 0b111, 0b101]), ('X', [0b101, 0b101, 0b010, 0b101, 0b101]),
        ('Y', [0b101, 0b101, 0b010, 0b010, 0b010]), ('Z', [0b111, 0b001, 0b010, 0b100, 0b111]),
        ('0', [0b111, 0b101, 0b101, 0b101, 0b111]), ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
        ('2', [0b110, 0b001, 0b010, 0b100, 0b111]), ('3', [0b110, 0b001, 0b010, 0b001, 0b110]),
        ('4', [0b101, 0b101, 0b111, 0b001, 0b001]), ('5', [0b111, 0b100, 0b110, 0b001, 0b110]),
        ('6', [0b011, 0b100, 0b111, 0b101, 0b111]), ('7', [0b111, 0b001, 0b010, 0b010, 0b010]),
        ('8', [0b111, 0b101, 0b111, 0b101, 0b111]), ('9', [0b111, 0b101, 0b111, 0b001, 0b110]),
        ('-', [0b000, 0b000, 0b111, 0b000, 0b000]), ('_', [0b000, 0b000, 0b000, 0b000, 0b111]),
    ];
    let scale = 2;
    let mut x = x0;
    for c in s.to_uppercase().chars() {
        if let Some((_, rows)) = FONT.iter().find(|(f, _)| *f == c) {
            for (ry, row) in rows.iter().enumerate() {
                for rx in 0..3 {
                    if row & (0b100 >> rx) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let (px, py) = (x + rx * scale + dx, y0 + ry as u32 * scale + dy);
                                if px < img.width() && py < img.height() {
                                    img.put_pixel(px, py, Rgba([20, 20, 20, 255]));
                                }
                            }
                        }
                    }
                }
            }
        }
        x += 4 * scale;
    }
}

fn sheet(root: &std::path::Path, series: &str, roles: &[String], out: &std::path::Path) -> anyhow::Result<()> {
    let library = AssetLibrary::at(root);
    let folder = format!("Cast/{series}");
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    let columns: Vec<(String, View, bool)> = [View::Front, View::Side, View::Back, View::ThreeQuarter]
        .into_iter()
        .map(|v| (format!("{}", v.word()), v, false))
        .chain([View::Front, View::Side, View::Back, View::ThreeQuarter].into_iter().map(|v| (format!("walk {}", v.word()), v, true)))
        .collect();
    let mut img = RgbaImage::from_pixel(columns.len() as u32 * CELL_W, roles.len() as u32 * CELL_H, Rgba([236, 232, 224, 255]));
    for (row, role) in roles.iter().enumerate() {
        for (col, (title, view, walk)) in columns.iter().enumerate() {
            let name = cast_fla::asset_name(role, *view, *walk);
            let (x0, y0) = (col as u32 * CELL_W, row as u32 * CELL_H);
            for x in x0..x0 + CELL_W {
                img.put_pixel(x, y0, Rgba([180, 176, 168, 255]));
            }
            for y in y0..y0 + CELL_H {
                img.put_pixel(x0, y, Rgba([180, 176, 168, 255]));
            }
            let Some(asset) = library.assets().iter().find(|a| a.folder == folder && a.name == name) else {
                text(&mut img, &format!("{role} {title} NONE"), x0 + 6, y0 + 6);
                continue;
            };
            let scene = library.load(asset)?;
            // A walk: the frame in the middle of the cycle.
            let frame = if *walk { scene.layers().iter().map(|l| l.frames.length()).max().unwrap_or(1) / 4 } else { 0 };
            let Some(b) = cast_fla::character_bounds(&scene, frame) else {
                text(&mut img, &format!("{role} {title} EMPTY"), x0 + 6, y0 + 6);
                continue;
            };
            let aspect = CELL_W as f64 / (CELL_H - LABEL_H) as f64;
            let mut h = b.height() * 1.1;
            let mut w = h * aspect;
            if w < b.width() * 1.1 {
                w = b.width() * 1.1;
                h = w / aspect;
            }
            let region = kurbo::Rect::new(b.center().x - w / 2.0, b.center().y - h / 2.0, b.center().x + w / 2.0, b.center().y + h / 2.0);
            let mut settings = ExportSettings::for_stage(&scene);
            settings.width = CELL_W;
            settings.height = CELL_H - LABEL_H;
            settings.region = Some(region);
            let pic = exporter.render(&scene, frame, &settings)?;
            let cell = RgbaImage::from_raw(pic.width, pic.height, pic.pixels).ok_or_else(|| anyhow::anyhow!("bad buffer"))?;
            for (x, y, p) in cell.enumerate_pixels() {
                let a = p[3] as u32;
                let mix = |c: u8, bg: u8| ((c as u32 * a + bg as u32 * (255 - a)) / 255) as u8;
                img.put_pixel(x0 + x, y0 + LABEL_H + y, Rgba([mix(p[0], 236), mix(p[1], 232), mix(p[2], 224), 255]));
            }
            text(&mut img, &format!("{role} {title}{}", if *walk { format!(" F{frame}") } else { String::new() }), x0 + 6, y0 + 6);
        }
    }
    img.save(out)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fla = PathBuf::from(arg(&args, "--fla").unwrap_or_else(|| r"B:\youtubeProjects\Khayal3Baje\stories\alaxmi\BuzzAnimate\cast.fla".into()));
    let series = arg(&args, "--series").unwrap_or_else(|| "Khayal3Baje".into());
    let root = match arg(&args, "--root") {
        Some(r) => PathBuf::from(r),
        None => AssetLibrary::user().root().map(std::path::Path::to_path_buf).ok_or_else(|| anyhow::anyhow!("no asset library"))?,
    };
    let only: Vec<String> = arg(&args, "--roles").map(|r| r.split(',').map(|s| s.trim().to_string()).collect()).unwrap_or_default();
    if args.iter().any(|a| a == "--check") {
        // Does the series' casting resolve against the catalogue the producer reads?
        let catalogue = buzz_app::my_cast::catalogue_for(&root, &series);
        let file = buzz_app::my_cast::load_casting(&root, &series);
        for (key, c) in &file.choices {
            let hit = catalogue.iter().any(|e| e.path == c.asset);
            println!("{key}: {} locked={} in catalogue={hit}", c.asset, c.locked);
        }
        println!("catalogue entries in Cast/{series}: {}", catalogue.iter().filter(|e| e.folder == format!("Cast/{series}")).count());
        return Ok(());
    }
    if args.iter().any(|a| a == "--dry") {
        let imported = buzz_app::import::read(&fla).map_err(|e| anyhow::anyhow!(e))?;
        for r in cast_fla::find_roles(&imported.scene) {
            println!("{} <- {}", r.role, r.wrapper);
            for ((view, walk), p) in &r.chosen {
                println!("   {} {} <- {} ({} layers)", view.word(), if *walk { "walk" } else { "pose" }, p.name, p.layers);
            }
            println!("   extra: {:?}", r.extra);
        }
        return Ok(());
    }
    let mut role_names: Vec<String> = cast_fla::ROLES.iter().map(|(_, r)| r.to_string()).collect();
    if !only.is_empty() {
        role_names.retain(|r| only.iter().any(|o| o.eq_ignore_ascii_case(r)));
    }
    if !args.iter().any(|a| a == "--sheet-only") {
        let req = cast_fla::CastFlaRequest { fla, root: root.clone(), series: series.clone(), ai_rig: args.iter().any(|a| a == "--ai-rig"), only };
        let mut say = |l: &str| eprintln!("{l}");
        let report = cast_fla::import_cast_fla(&req, &mut say).map_err(|e| anyhow::anyhow!(e))?;
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    }
    if let Some(out) = arg(&args, "--sheet") {
        sheet(&root, &series, &role_names, std::path::Path::new(&out))?;
    }
    Ok(())
}
