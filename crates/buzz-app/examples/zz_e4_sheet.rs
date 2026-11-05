//! E4 read-only sheet: zz_e4_sheet <library-root> <out-dir> [ROLE,...]
//! Per role: row 1 rest / arms +30 / thighs +30 (main pose), row 2 head close-ups
//! with the lid chart on frames 0..n, row 3 mouth chart frames 0..n.
use buzz_doc::AssetLibrary;
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::Affine;
use buzz_render::GpuPreference;
use buzz_scene::{LayerId, LoopMode, ObjectKind, Scene, SymbolId};

fn stage_of(asset: &Scene, main: SymbolId, pose: SymbolId) -> Option<Scene> {
    let mut stage = asset.clone();
    stage.edit_document();
    for id in stage.stage_layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        stage.remove_layer(id);
    }
    for id in stage.library().iter().map(|s| s.id).collect::<Vec<_>>() {
        stage.library_mut().remove(id);
    }
    buzz_act::pose::lift_character(&mut stage, asset, main, pose, "Cast/e4").ok()?;
    Some(stage)
}

fn rotate_layers(stage: &mut Scene, pick: &dyn Fn(&str) -> bool, deg: f64) -> usize {
    let mut n = 0;
    let ids: Vec<(LayerId, String)> = stage.layers().iter().map(|l| (l.id, l.name.to_lowercase())).collect();
    for (id, name) in ids {
        if !pick(&name) {
            continue;
        }
        let Some(obj) = stage.layers().get(id).and_then(|l| l.objects_at(0).first().cloned()) else { continue };
        let p = stage.pivot_local_of(&obj);
        let m = obj.transform * Affine::translate((p.x, p.y)) * Affine::rotate(deg.to_radians()) * Affine::translate((-p.x, -p.y));
        stage.update_object_at(0, obj.id, |o| o.transform = m);
        n += 1;
    }
    n
}

/// (symbol holding the layer, layer, symbol the layer shows) of the first layer named like `want`.
fn find_layer(stage: &Scene, want: &str) -> Option<(SymbolId, LayerId, SymbolId)> {
    for s in stage.library().iter() {
        for l in s.layers.iter() {
            if l.name.to_lowercase().contains(want) {
                if let Some(o) = l.objects_at(0).first() {
                    if let ObjectKind::Instance(i) = &o.kind {
                        return Some((s.id, l.id, i.symbol));
                    }
                }
            }
        }
    }
    None
}

fn set_frame(stage: &mut Scene, sym: SymbolId, layer: LayerId, frame: u32) {
    if let Some(stack) = stage.library_mut().layers_mut(sym) {
        stack.update(layer, |l| {
            if let Some(first) = l.frames.objects_at(0).first() {
                let mut o = (**first).clone();
                if let ObjectKind::Instance(i) = &mut o.kind {
                    i.loop_mode = LoopMode::SingleFrame;
                    i.first_frame = frame;
                }
                l.frames.set_objects(0, vec![std::sync::Arc::new(o)]);
            }
        });
    }
}

fn only_head(stage: &mut Scene) {
    let head = stage
        .layers()
        .iter()
        .find(|l| {
            let n = l.name.to_lowercase();
            n.contains("head") || n.contains("face")
        })
        .map(|l| l.id);
    for id in stage.layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        if Some(id) != head {
            stage.update_layer(id, |l| l.visible = false);
        }
    }
}

fn bbox(f: &Frame) -> (u32, u32, u32, u32) {
    let bg = [f.pixels[0], f.pixels[1], f.pixels[2], f.pixels[3]];
    let (mut x0, mut y0, mut x1, mut y1) = (f.width, f.height, 0, 0);
    for y in 0..f.height {
        for x in 0..f.width {
            let i = ((y * f.width + x) * 4) as usize;
            if f.pixels[i..i + 4] != bg {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0, y0, x1, y1)
}

fn paste_crop(sheet: &mut Frame, f: &Frame, col: u32, row: u32, cell: u32, bb: (u32, u32, u32, u32)) {
    let (w, h) = ((bb.2.saturating_sub(bb.0) + 1) as f64, (bb.3.saturating_sub(bb.1) + 1) as f64);
    let s = w.max(h) / cell as f64;
    for y in 0..cell {
        for x in 0..cell {
            let (fx, fy) = ((bb.0 as f64 + (x as f64 + 0.5) * s) as u32, (bb.1 as f64 + (y as f64 + 0.5) * s) as u32);
            let o = (((row * cell + y) * sheet.width + col * cell + x) * 4) as usize;
            if fx < f.width && fy < f.height {
                let i = ((fy * f.width + fx) * 4) as usize;
                sheet.pixels[o..o + 4].copy_from_slice(&f.pixels[i..i + 4]);
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let library = AssetLibrary::at(std::path::Path::new(&a[0]));
    let out = std::path::PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out)?;
    let roles: Vec<String> = a
        .get(2)
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_else(|| ["DEEPAK", "BHUSHAN", "MISHRA JI", "RADHE", "PADOSI 1", "PADOSI 2"].map(String::from).to_vec());
    let mut ex = Exporter::new(&GpuPreference::Automatic)?;
    let cell = 220u32;
    for role in roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| x.name == role && x.folder.ends_with("Khayal3Baje")) else { continue };
        let scene = library.load(&asset)?;
        let Some(main) = buzz_app::cast_combine::main_symbol(&scene) else { continue };
        let pose = scene.library().symbol_set(main)[0];
        let Some(base) = stage_of(&scene, main, pose) else {
            println!("{role}: lift failed");
            continue;
        };
        let size = base.stage().size;
        let full = ExportSettings::scaled(&base, cell as f64 * 2.0 / size.width.max(size.height));
        let mut sheet = Frame { width: cell * 14, height: cell * 3, pixels: vec![255; (cell * 14 * cell * 3 * 4) as usize] };
        let mut cells = vec![base.clone()];
        let mut arms = base.clone();
        let na = rotate_layers(&mut arms, &|n| n.contains("arm") && !n.contains("wrist") && !n.contains("palm") && (n.contains("left") || n.contains("right")), 30.0);
        cells.push(arms);
        let mut legs = base.clone();
        let nl = rotate_layers(&mut legs, &|n| n.contains("thigh"), 30.0);
        cells.push(legs);
        for (c, s) in cells.iter().enumerate() {
            let f = ex.render(s, 0, &full)?;
            let bb = bbox(&f);
            paste_crop(&mut sheet, &f, c as u32, 0, cell, bb);
        }
        let mut head_base = base.clone();
        let _ = only_head;
        let hs = ExportSettings::scaled(&head_base, 2.0);
        let mut notes = vec![format!("arms rotated {na}, thighs rotated {nl}")];
        let mut lid_frames = Vec::new();
        if let Some((psym, lyr, chart)) = find_layer(&head_base, "eyelid") {
            let mut target = (psym, lyr, chart);
            for _ in 0..3 {
                let len = head_base.library().get(target.2).map(|s| s.length()).unwrap_or(0);
                if len > 1 && len <= 12 {
                    break;
                }
                let Some(s) = head_base.library().get(target.2).cloned() else { break };
                let next = s.layers.iter().find_map(|l| {
                    l.objects_at(0).first().and_then(|o| match &o.kind {
                        ObjectKind::Instance(i) => Some((s.id, l.id, i.symbol)),
                        _ => None,
                    })
                });
                if let Some(n) = next {
                    target = n;
                }
            }
            let n = head_base.library().get(target.2).map(|s| s.length()).unwrap_or(0);
            notes.push(format!("lid chart {n}f"));
            for k in 0..n.min(10) {
                let mut s = head_base.clone();
                set_frame(&mut s, target.0, target.1, k);
                lid_frames.push(ex.render(&s, 0, &hs)?);
            }
        } else {
            notes.push("NO eyeLid layer".into());
        }
        let mut mouth_frames = Vec::new();
        if let Some((psym, lyr, chart)) = find_layer(&head_base, "mouth") {
            let n = head_base.library().get(chart).map(|s| s.length()).unwrap_or(0);
            notes.push(format!("mouth chart {n}f"));
            for k in 0..n.min(14) {
                let mut s = head_base.clone();
                set_frame(&mut s, psym, lyr, k);
                mouth_frames.push(ex.render(&s, 0, &hs)?);
            }
        } else {
            notes.push("NO mouth layer".into());
        }
        for (row, frames) in [(1u32, &lid_frames), (2u32, &mouth_frames)] {
            let mut u = (u32::MAX, u32::MAX, 0, 0);
            for f in frames {
                let b = bbox(f);
                if b.2 >= b.0 {
                    u = (u.0.min(b.0), u.1.min(b.1), u.2.max(b.2), u.3.max(b.3));
                }
            }
            let side = ((u.3.saturating_sub(u.1) + 1) as f64 * 0.17) as u32;
            let cx = (u.0 + u.2) / 2;
            let u = (cx.saturating_sub(side / 2), u.1, cx + side / 2, u.1 + side);
            for (c, f) in frames.iter().enumerate() {
                paste_crop(&mut sheet, f, c as u32, row, cell, u);
            }
        }
        let path = out.join(format!("e4_{}.png", role.replace(' ', "_")));
        sheet.write_png(&path)?;
        println!("{role}: {} | {}", notes.join(", "), path.display());
    }
    Ok(())
}
