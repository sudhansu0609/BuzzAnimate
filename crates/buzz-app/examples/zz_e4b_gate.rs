//! E4b gate: zz_e4b_gate <library-root> <out-dir> [ROLE,...]
//! Per role and per pose, from a placed copy (lift_character, the way the producer places
//! one): rest, half blink, shut, four visemes written with the head's own `Mouth` / `Eyelid`
//! layers (buzz_act::face), rendered, and the pixels of the eye band and the mouth band
//! compared with the rest frame. Writes one strip PNG per role (main pose) and prints JSON.
use buzz_audio::Viseme;
use buzz_doc::AssetLibrary;
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::Rect;
use buzz_render::GpuPreference;
use buzz_scene::{LayerId, Scene, SymbolId};
use serde_json::json;

fn stage_of(asset: &Scene, main: SymbolId, pose: SymbolId) -> Option<(Scene, Vec<LayerId>)> {
    let mut stage = asset.clone();
    stage.edit_document();
    for id in stage.stage_layers().iter().map(|l| l.id).collect::<Vec<_>>() {
        stage.remove_layer(id);
    }
    for id in stage.library().iter().map(|s| s.id).collect::<Vec<_>>() {
        stage.library_mut().remove(id);
    }
    stage.set_frame_count(30);
    let lifted = buzz_act::pose::lift_character(&mut stage, asset, main, pose, "Cast/e4").ok()?;
    for l in &lifted.layers {
        stage.update_layer(*l, |layer| {
            while layer.frames.length() < 30 {
                let n = layer.frames.length();
                layer.frames.insert_frame(n);
            }
        });
    }
    Some((stage, lifted.layers))
}

fn render(ex: &mut Exporter, scene: &Scene, frame: u32, region: Rect, width: u32) -> anyhow::Result<Frame> {
    let mut s = ExportSettings::for_stage(scene);
    s.width = width;
    s.height = ((width as f64 * region.height() / region.width().max(1.0)).round() as u32).clamp(16, 2000);
    s.transparent = false;
    s.region = Some(region);
    Ok(ex.render(scene, frame, &s)?)
}

/// Pixels that differ (any channel by more than 24) between two frames: how many, and
/// the mean row of them as a fraction of the height.
fn changed(a: &Frame, b: &Frame) -> (usize, f64) {
    let (mut n, mut sum) = (0usize, 0f64);
    for y in 0..a.height.min(b.height) {
        for x in 0..a.width {
            let i = ((y * a.width + x) * 4) as usize;
            if (0..3).any(|c| (a.pixels[i + c] as i32 - b.pixels[i + c] as i32).abs() > 24) {
                n += 1;
                sum += y as f64;
            }
        }
    }
    (n, if n > 0 { sum / n as f64 / a.height as f64 } else { 0.0 })
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
    let mut report = Vec::new();
    for role in roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| x.name == role && x.folder.ends_with("Khayal3Baje")) else { continue };
        let scene = library.load(&asset)?;
        let Some(main) = buzz_app::cast_combine::main_symbol(&scene) else { continue };
        let mut poses = Vec::new();
        for pose in scene.library().symbol_set(main) {
            let name = scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default();
            let Some((mut stage, layers)) = stage_of(&scene, main, pose) else {
                poses.push(json!({"pose": name, "error": "lift failed"}));
                continue;
            };
            let Some(view) = layers.iter().find_map(|l| buzz_act::layer_rig::rig_of_layer(&stage, *l)) else {
                poses.push(json!({"pose": name, "error": "no rig"}));
                continue;
            };
            let Some(face) = buzz_act::face::face_of(&stage, &view) else {
                poses.push(json!({"pose": name, "mouth": false, "eyelid": false, "note": "no Mouth/Eyelid layer in the head"}));
                continue;
            };
            if std::env::var("E4_DEBUG").is_ok() {
                let mut todo = vec![face.head_symbol];
                let mut seen = Vec::new();
                while let Some(id) = todo.pop() {
                    if seen.contains(&id) { continue; }
                    seen.push(id);
                    let Some(sy) = stage.library().get(id) else { continue };
                    for l in sy.layers.iter() {
                        let multi = l.frames.keyframe_count();
                        println!("  [{}] {} ({}f) layer {:?} keys={} len={}", id.0, sy.name, sy.length(), l.name, multi, l.frames.length());
                        for k in l.frames.keyframes().iter().take(1) { for o in k.objects.iter() { if let Some(i) = o.instance() { todo.push(i.symbol); } } }
                    }
                }
            }
            let synced = stage.find_object(face.head).and_then(|(_, o)| o.instance().map(|i| i.sync_to_root)).unwrap_or(false);
            // The head's box on the stage, split into an eye band and a mouth band.
            let Some((_, head_obj)) = stage.find_object(face.head) else { continue };
            let head_obj = head_obj.clone();
            let inherited = stage.layers().inherited_transform(face.head_layer, 0u32);
            let head = buzz_scene::object::transform_rect(inherited, stage.resolved_bounds(&head_obj));
            let hb = Rect::new(head.x0, head.y0, head.x1, head.y1);
            let mut keys_lid: Vec<(u32, u32)> = Vec::new();
            let mut keys_mouth: Vec<(u32, u32)> = Vec::new();
            let shapes = [Viseme::Ai, Viseme::E, Viseme::O, Viseme::MBP];
            if let Some((_, f)) = face.lid {
                keys_lid = vec![(1, f[1]), (2, f[2]), (3, f[0])];
            }
            if face.mouth.is_some() {
                keys_mouth = std::iter::once((0, Viseme::Rest.frame())).chain(shapes.iter().enumerate().map(|(i, v)| (4 + i as u32, v.frame()))).collect();
            }
            if std::env::var("E4_NOKEYS").is_ok() {
                keys_lid.clear();
                keys_mouth.clear();
            }
            if let Some((spot, _)) = face.lid {
                buzz_act::face::put_chart_keys(&mut stage, &face, spot, &keys_lid);
            }
            if let Some(spot) = face.mouth {
                buzz_act::face::put_chart_keys(&mut stage, &face, spot, &keys_mouth);
            }
            let width = 360;
            let frames: Vec<Frame> = (0..8).map(|f| render(&mut ex, &stage, f, hb, width)).collect::<anyhow::Result<_>>()?;
            let h = frames[0].height;
            // What a blink changes (half shut, shut) and what a viseme changes, against the rest frame.
            let eye: Vec<(usize, f64)> = (1..=2).map(|f| changed(&frames[0], &frames[f])).collect();
            let mouth: Vec<(usize, f64)> = (4..8).map(|f| changed(&frames[0], &frames[f])).collect();
            let min_px = (width as usize * h as usize) / 200;
            let eye_y = eye[1].1;
            let mouth_y = mouth.iter().filter(|m| m.0 > min_px).map(|m| m.1).sum::<f64>() / mouth.iter().filter(|m| m.0 > min_px).count().max(1) as f64;
            // The eyes are above the mouth: the changes of a blink sit higher than the changes of a viseme.
            let apart = mouth_y - eye_y > 0.08;
            let blinks = face.lid.is_some() && eye.iter().all(|p| p.0 > min_px) && apart;
            let speaks = face.mouth.is_some() && mouth.iter().filter(|p| p.0 > min_px).count() >= 3 && apart;
            if std::env::var("E4_ALL").is_ok() || name == scene.library().get(main).map(|s| s.name.clone()).unwrap_or_default() {
                // The strip: rest, half, shut, then the four visemes (frames 0, 1, 2, 4..7).
                let cell_w = width;
                let cell_h = h;
                let order = [0usize, 1, 2, 4, 5, 6, 7];
                let mut sheet = Frame { width: cell_w * order.len() as u32, height: cell_h, pixels: vec![255; (cell_w * order.len() as u32 * cell_h * 4) as usize] };
                for (c, f) in order.iter().enumerate() {
                    for y in 0..cell_h.min(frames[*f].height) {
                        for x in 0..cell_w {
                            let src = ((y * frames[*f].width + x) * 4) as usize;
                            let dst = ((y * sheet.width + c as u32 * cell_w + x) * 4) as usize;
                            sheet.pixels[dst..dst + 4].copy_from_slice(&frames[*f].pixels[src..src + 4]);
                        }
                    }
                }
                let path = out.join(format!("e4b_{}.png", name.replace(' ', "_")));
                sheet.write_png(&path)?;
                println!("strip {}", path.display());
            }
            poses.push(json!({"pose": name, "head_synced": synced, "mouth": face.mouth.is_some(), "eyelid": face.lid.is_some(),
                "lid_frames": face.lid.map(|(_, f)| f), "blink_px_half_shut_shut": eye.iter().map(|p| p.0).collect::<Vec<_>>(), "blink_rows": (eye_y * 100.0).round() / 100.0,
                "viseme_px": mouth.iter().map(|p| p.0).collect::<Vec<_>>(), "viseme_rows": (mouth_y * 100.0).round() / 100.0,
                "min_px": min_px, "blinks": blinks, "speaks": speaks}));
        }
        // Switching the placed Front pose to Side carries the keys into the Side pose's own head.
        let mut switched = json!(null);
        let side = scene.library().symbol_set(main).into_iter().find(|p| scene.library().get(*p).is_some_and(|s| s.name == format!("{role} Side")));
        if let Some(side) = side
            && let Some((mut stage, layers)) = stage_of(&scene, main, main)
        {
            let view = layers.iter().find_map(|l| buzz_act::layer_rig::rig_of_layer(&stage, *l));
            if let Some(view) = view
                && let Some(face) = buzz_act::face::face_of(&stage, &view)
            {
                let folder = stage.layers().get(layers[0]).and_then(|l| l.parent).unwrap_or(layers[0]);
                if let Some(spot) = face.mouth {
                    buzz_act::face::put_chart_keys(&mut stage, &face, spot, &[(4, 1), (8, 3), (12, 0)]);
                }
                if let Some((spot, f)) = face.lid {
                    buzz_act::face::put_chart_keys(&mut stage, &face, spot, &[(2, f[1]), (4, f[2]), (6, f[0])]);
                }
                let side = buzz_act::pose::pose_for_view(&stage, folder, "side").unwrap_or(side);
                match buzz_act::pose::switch_lifted_pose(&mut stage, folder, 0, side) {
                    Ok(r) => {
                        let src = format!("|{role} Side");
                        let new_face = stage
                            .layers()
                            .iter()
                            .filter(|l| l.cast_source.as_deref().is_some_and(|c| c.ends_with(&src)))
                            .find_map(|l| buzz_act::layer_rig::rig_of_layer(&stage, l.id))
                            .and_then(|v| buzz_act::face::face_of(&stage, &v));
                        let m = new_face.and_then(|f| f.mouth).map(|sp| buzz_act::face::chart_track(&stage, sp, 0, 20));
                        let l = new_face.and_then(|f| f.lid).map(|(sp, _)| buzz_act::face::chart_track(&stage, sp, 0, 20));
                        switched = json!({"mouth_keys": r.mouth_keys, "blink_keys": r.blink_keys, "side_mouth_track": m, "side_lid_track": l});
                    }
                    Err(e) => switched = json!({"error": e}),
                }
            }
        }
        report.push(json!({"role": role, "poses": poses, "switch_front_to_side": switched}));
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
