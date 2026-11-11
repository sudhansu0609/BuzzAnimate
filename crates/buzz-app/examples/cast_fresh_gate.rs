//! Gate for CAST_FRESH_PLAN F1: Start Fresh on a COPY of the Cast folder.
//!
//! `cargo run --release -p buzz-app --example cast_fresh_gate -- <library-root> <out-dir> [keep_frame]`
//!
//! `<library-root>` must be a copy (it is rewritten). Prints one JSON object of
//! measurements and writes `<out-dir>/sheet.png` (6 characters, before | after).

use std::collections::BTreeSet;
use std::path::PathBuf;

use buzz_doc::AssetLibrary;
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_render::GpuPreference;
use buzz_scene::{KeepPose, Object, ObjectKind, Scene, SymbolId};

fn symbols_of(object: &Object, out: &mut Vec<SymbolId>) {
    match &object.kind {
        ObjectKind::Instance(i) => out.push(i.symbol),
        ObjectKind::Group(children) => children.iter().for_each(|c| symbols_of(c, out)),
        ObjectKind::Armature(rig) => rig.parts.iter().for_each(|p| symbols_of(&p.artwork, out)),
        _ => {}
    }
}

/// Max keyframes on any layer of any non-picker symbol reachable from the stage.
fn max_keys(scene: &Scene, skip: &BTreeSet<SymbolId>) -> usize {
    let mut queue = Vec::new();
    for layer in scene.stage_layers().iter() {
        for o in layer.all_objects() {
            symbols_of(o, &mut queue);
        }
    }
    let mut seen = BTreeSet::new();
    let mut max = 0;
    while let Some(id) = queue.pop() {
        if skip.contains(&id) || !seen.insert(id) {
            continue;
        }
        let Some(symbol) = scene.library().get(id) else { continue };
        for layer in symbol.layers.iter() {
            max = max.max(layer.keyframe_count());
            for o in layer.all_objects() {
                symbols_of(o, &mut queue);
            }
        }
    }
    max
}

fn stage_ok(scene: &Scene) -> bool {
    scene
        .stage_layers()
        .iter()
        .all(|l| l.keyframe_count() == 1 && l.frames.length() == 1 && !l.audio_guide)
}

/// (symbol id, layer id, keyframes, symbol length) of every picker symbol.
fn picker_shape(scene: &Scene, skip: &BTreeSet<SymbolId>) -> Vec<(u64, usize, u32)> {
    let mut v = Vec::new();
    for id in skip {
        if let Some(s) = scene.library().get(*id) {
            v.push((id.0, s.layers.iter().map(|l| l.keyframe_count()).sum(), s.length()));
        }
    }
    v
}

fn settings(scene: &Scene, max_dim: u32) -> ExportSettings {
    let size = scene.stage().size;
    let scale = max_dim as f64 / size.width.max(size.height).max(1.0);
    ExportSettings::scaled(scene, scale)
}

fn psnr(a: &Frame, b: &Frame) -> f64 {
    if a.width != b.width || a.height != b.height {
        return 0.0;
    }
    let mut sum = 0f64;
    for (x, y) in a.pixels.iter().zip(&b.pixels) {
        let d = *x as f64 - *y as f64;
        sum += d * d;
    }
    let mse = sum / a.pixels.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// Downsample 2x and paste into a cell of `cell` x `cell` on a sheet.
fn paste(sheet: &mut Frame, f: &Frame, col: u32, row: u32, cell: u32) {
    for y in 0..cell.min(f.height / 2) {
        for x in 0..cell.min(f.width / 2) {
            let mut acc = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (((2 * y + dy) * f.width + 2 * x + dx) * 4) as usize;
                for c in 0..4 {
                    acc[c] += f.pixels[i + c] as u32;
                }
            }
            let o = (((row * cell + y) * sheet.width + col * cell + x) * 4) as usize;
            for c in 0..4 {
                sheet.pixels[o + c] = (acc[c] / 4) as u8;
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("library root (a copy)"));
    let out = PathBuf::from(args.next().expect("out dir"));
    let keep = args.next().and_then(|a| a.parse::<u32>().ok());
    std::fs::create_dir_all(&out)?;
    let mut library = AssetLibrary::at(&root);
    let characters = buzz_app::cast_fresh::cast_characters(&library);
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    let stamp = buzz_app::cast_fresh::backup_stamp();

    let mut keys_after_max = 0usize;
    let mut keys_before_max = 0usize;
    let mut swaps_unchanged = true;
    let mut psnr_min = f64::MAX;
    let mut psnr_worst = String::new();
    let mut new_doc_ok = true;
    let mut backup_ok = true;
    let mut stage_ok_all = true;
    let mut errors: Vec<String> = Vec::new();
    let mut with_pickers = 0;
    let mut cleared_total = 0usize;
    let mut picked: Vec<(String, Frame, Frame)> = Vec::new();

    let inject = std::env::var_os("INJECT").is_some();
    let mut injected = 0usize;
    for (index, asset) in characters.iter().enumerate() {
        if inject {
            // Give the copy real keyframes to clear: 12 frames, keys at 0, 4, 8 on
            // every stage layer and every non-picker symbol layer.
            let mut scene = library.load(asset)?;
            let skip = scene.picker_symbols();
            let widen = |stack: &mut buzz_scene::LayerStack| {
                let ids: Vec<_> = stack.iter().filter(|l| !l.audio_guide).map(|l| l.id).collect();
                for id in ids {
                    stack.update(id, |l| {
                        while l.frames.length() < 12 {
                            l.frames.insert_frame(l.frames.length());
                        }
                        l.frames.insert_keyframe(4);
                        l.frames.insert_keyframe(8);
                    });
                }
            };
            widen(scene.edit_stage_layers());
            let symbols: Vec<_> = scene.library().iter().map(|s| s.id).collect();
            for id in symbols {
                if !skip.contains(&id) {
                    if let Some(stack) = scene.library_mut().layers_mut(id) {
                        widen(stack);
                    }
                }
            }
            buzz_doc::format::save(&scene, &asset.path)?;
            library.rescan();
            injected += 1;
        }
        let before = library.load(asset)?;
        let skip = before.picker_symbols();
        let shape_before = picker_shape(&before, &skip);
        keys_before_max = keys_before_max.max(max_keys(&before, &skip));
        let frame_before = exporter.render(&before, 0, &settings(&before, 512))?;
        let original = std::fs::read(&asset.path)?;

        let keep_pose = keep.map_or(KeepPose::First, KeepPose::Frame);
        let outcome = match buzz_app::cast_fresh::start_fresh_asset(&mut library, asset, keep_pose, &stamp) {
            Ok(o) => o,
            Err(e) => {
                errors.push(format!("{}: {e}", asset.name));
                continue;
            }
        };
        let after = library.load(asset)?;
        let skip_after = after.picker_symbols();
        if skip_after != skip {
            errors.push(format!("{}: picker set changed", asset.name));
        }
        if !skip.is_empty() {
            with_pickers += 1;
        }
        if picker_shape(&after, &skip) != shape_before {
            swaps_unchanged = false;
            errors.push(format!("{}: a picker symbol changed", asset.name));
        }
        let k = max_keys(&after, &skip);
        keys_after_max = keys_after_max.max(k);
        if !stage_ok(&after) {
            stage_ok_all = false;
            errors.push(format!("{}: stage not one frame/key", asset.name));
        }
        let frame_after = exporter.render(&after, 0, &settings(&after, 512))?;
        let p = psnr(&frame_before, &frame_after);
        if p < psnr_min {
            psnr_min = p;
            psnr_worst = asset.name.clone();
        }

        // Backup exists, is the original byte for byte, and loads.
        match std::fs::read(&outcome.backup) {
            Ok(b) if b == original && buzz_doc::format::load(&outcome.backup).is_ok() => {}
            _ => {
                backup_ok = false;
                errors.push(format!("{}: backup mismatch", asset.name));
            }
        }

        // Into a new empty document.
        let mut doc = Scene::default();
        match library.place(asset, &mut doc) {
            Ok(_) => {
                let skip_doc = doc.picker_symbols();
                let ok = stage_ok(&doc) && max_keys(&doc, &skip_doc) <= 1 && !doc.stage_layers().is_empty();
                if !ok {
                    new_doc_ok = false;
                    errors.push(format!("{}: new doc not clean (stage_ok {} keys {})", asset.name, stage_ok(&doc), max_keys(&doc, &skip_doc)));
                }
            }
            Err(e) => {
                new_doc_ok = false;
                errors.push(format!("{}: place failed {e}", asset.name));
            }
        }
        cleared_total += outcome.report.keyframes_cleared + outcome.report.stage_keyframes_cleared;
        if picked.len() < 6 && index % 11 == 0 && outcome.report.keyframes_cleared + outcome.report.stage_keyframes_cleared > 0 {
            picked.push((asset.name.clone(), frame_before, frame_after));
        }
    }

    // The sheet: before | after, six rows.
    let cell = 256u32;
    let mut sheet = Frame {
        width: cell * 2,
        height: cell * picked.len().max(1) as u32,
        pixels: vec![255; (cell * 2 * cell * picked.len().max(1) as u32 * 4) as usize],
    };
    for (row, (_, b, a)) in picked.iter().enumerate() {
        paste(&mut sheet, b, 0, row as u32, cell);
        paste(&mut sheet, a, 1, row as u32, cell);
    }
    let sheet_path = out.join("sheet.png");
    sheet.write_png(&sheet_path)?;

    println!(
        "{}",
        serde_json::json!({
            "characters": characters.len(),
            "with_pickers": with_pickers,
            "injected": injected,
            "keys_cleared_total": cleared_total,
            "keys_before_max": keys_before_max,
            "keys_after_max": keys_after_max,
            "stage_one_frame_all": stage_ok_all,
            "swap_symbols_unchanged": swaps_unchanged,
            "psnr_min_db": psnr_min,
            "psnr_worst": psnr_worst,
            "new_doc_instance_ok": new_doc_ok,
            "backup_ok": backup_ok,
            "sheet": sheet_path.display().to_string(),
            "sheet_names": picked.iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
            "errors": errors,
        })
    );
    Ok(())
}
