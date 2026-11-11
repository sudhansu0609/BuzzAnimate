//! Gate for CAST_POSES_PLAN C2: combine the real roles' poses, sheet every pose.
//!
//! `cargo run --release -p buzz-app --example cast_poses_gate -- combine <library-root> <out-dir> [ROLE,ROLE...]`
//!   backs the whole Cast folder up, combines the roles (default: the 6 FLA roles),
//!   writes `<out-dir>/poses_<n>.png` (one row per role, one cell per pose, in set order)
//!   and prints one JSON object.
//!
//! `... -- sheet <library-root> <out-dir> [ROLE,...]` only draws the sheets.

use std::path::PathBuf;

use buzz_doc::AssetLibrary;
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_render::GpuPreference;
use buzz_scene::Scene;

const ROLES: [&str; 6] = ["DEEPAK", "BHUSHAN", "MISHRA JI", "RADHE", "PADOSI 1", "PADOSI 2"];

fn paste(sheet: &mut Frame, f: &Frame, col: u32, row: u32, cell: u32) {
    let sx = f.width as f64 / cell as f64;
    let sy = f.height as f64 / cell as f64;
    for y in 0..cell {
        for x in 0..cell {
            let (fx, fy) = (((x as f64 + 0.5) * sx) as u32, ((y as f64 + 0.5) * sy) as u32);
            let i = ((fy.min(f.height - 1) * f.width + fx.min(f.width - 1)) * 4) as usize;
            let o = (((row * cell + y) * sheet.width + col * cell + x) * 4) as usize;
            sheet.pixels[o..o + 4].copy_from_slice(&f.pixels[i..i + 4]);
        }
    }
}

/// One pose of a combined asset, lifted onto a blank stage of the asset's size.
fn pose_stage(asset: &Scene, main: buzz_scene::SymbolId, pose: buzz_scene::SymbolId) -> Option<Scene> {
    let mut stage = asset.clone();
    stage.edit_document();
    for id in stage.stage_layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        stage.remove_layer(id);
    }
    for id in stage.library().iter().map(|s| s.id).collect::<Vec<_>>() {
        stage.library_mut().remove(id);
    }
    buzz_act::pose::lift_character(&mut stage, asset, main, pose, "Cast/gate").ok()?;
    Some(stage)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("combine | sheet");
    let root = PathBuf::from(args.next().expect("library root"));
    let out = PathBuf::from(args.next().expect("out dir"));
    let roles: Vec<String> = match args.next() {
        Some(list) => list.split(',').map(|s| s.trim().to_string()).collect(),
        None => ROLES.iter().map(|s| s.to_string()).collect(),
    };
    std::fs::create_dir_all(&out)?;
    let mut library = AssetLibrary::at(&root);
    let mut report = serde_json::Map::new();
    if mode == "measure" || mode == "normalise" {
        if mode == "normalise" {
            let backup = buzz_app::cast_fla::backup_cast(&root).map_err(anyhow::Error::msg)?;
            report.insert("backup".into(), backup.display().to_string().into());
        }
        let mut rows = Vec::new();
        for role in &roles {
            let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|a| a.name == *role && a.folder.ends_with("Khayal3Baje")) else {
                continue;
            };
            let mut scene = library.load(&asset)?;
            let Some(main) = buzz_app::cast_combine::main_symbol(&scene) else {
                continue;
            };
            if mode == "normalise" {
                let done = buzz_app::cast_combine::normalise_poses(&mut scene, main);
                buzz_doc::format::save(&scene, &asset.path).map_err(anyhow::Error::msg)?;
                report.insert(format!("{role}_scaled"), done.iter().map(|(n, s)| format!("{n} x{s:.3}")).collect::<Vec<_>>().into());
            }
            let mut poses = Vec::new();
            for pose in scene.library().symbol_set(main) {
                let name = scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default();
                let b = buzz_app::cast_combine::pose_box(&scene, pose, 0);
                let mut mouth = String::from("-");
                if let Some(stage) = pose_stage(&scene, main, pose) {
                    let view = stage.layers().iter().find_map(|l| buzz_act::layer_rig::rig_of_layer(&stage, l.id));
                    if let Some(view) = view {
                        mouth = match buzz_act::textsync::find_layer_rig_mouth(&stage, &view) {
                            Some(id) => stage
                                .find_object(id)
                                .and_then(|(_, o)| o.instance().and_then(|i| stage.library().get(i.symbol)))
                                .map_or("?".into(), |s| format!("{} ({}f)", s.name, s.length())),
                            None => "SILENT".into(),
                        };
                    } else {
                        mouth = "no rig".into();
                    }
                }
                poses.push(serde_json::json!({"pose": name, "h": b.map(|b| (b.height() * 10.0).round() / 10.0), "feet": b.map(|b| [(b.center().x * 10.0).round() / 10.0, (b.y1 * 10.0).round() / 10.0]), "mouth": mouth}));
            }
            rows.push(serde_json::json!({"role": role, "poses": poses}));
        }
        report.insert("measured".into(), rows.into());
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if mode == "combine" {
        let backup = buzz_app::cast_fla::backup_cast(&root).map_err(anyhow::Error::msg)?;
        report.insert("backup".into(), backup.display().to_string().into());
        let stamp = buzz_app::cast_fresh::backup_stamp();
        let (done, failed) = buzz_app::cast_combine::combine_many(&mut library, Some(&roles), &stamp);
        report.insert(
            "combined".into(),
            done.iter().map(|d| serde_json::json!({"role": d.role, "poses": d.poses, "moved_to": d.backup.display().to_string()})).collect::<Vec<_>>().into(),
        );
        report.insert("failed".into(), failed.into());
    }

    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    let cell = 256u32;
    let mut rows: Vec<(String, Vec<Frame>, Vec<String>)> = Vec::new();
    for role in &roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|a| a.name == *role && a.folder.ends_with("Khayal3Baje")) else {
            continue;
        };
        let scene = library.load(&asset)?;
        let Some(main) = buzz_app::cast_combine::main_symbol(&scene) else {
            report.insert(format!("{role}_note"), "not combined".into());
            continue;
        };
        let mut frames = Vec::new();
        let mut names = Vec::new();
        for pose in scene.library().symbol_set(main) {
            let name = scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default();
            let Some(stage) = pose_stage(&scene, main, pose) else {
                names.push(format!("{name} (could not be lifted)"));
                continue;
            };
            let size = stage.stage().size;
            let scale = cell as f64 * 2.0 / size.width.max(size.height);
            frames.push(exporter.render(&stage, 0, &ExportSettings::scaled(&stage, scale))?);
            names.push(name);
        }
        rows.push((role.clone(), frames, names));
    }
    // Three roles to a sheet.
    for (n, chunk) in rows.chunks(3).enumerate() {
        let cols = chunk.iter().map(|(_, f, _)| f.len()).max().unwrap_or(1).max(1) as u32;
        let mut sheet = Frame { width: cell * cols, height: cell * chunk.len() as u32, pixels: vec![255; (cell * cols * cell * chunk.len() as u32 * 4) as usize] };
        for (r, (_, frames, _)) in chunk.iter().enumerate() {
            for (c, f) in frames.iter().enumerate() {
                paste(&mut sheet, f, c as u32, r as u32, cell);
            }
        }
        let path = out.join(format!("poses_{}.png", n + 1));
        sheet.write_png(&path)?;
        report.insert(format!("sheet_{}", n + 1), path.display().to_string().into());
    }
    report.insert(
        "poses".into(),
        rows.iter().map(|(r, _, names)| serde_json::json!({"role": r, "poses": names})).collect::<Vec<_>>().into(),
    );
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
