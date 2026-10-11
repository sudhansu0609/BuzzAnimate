//! Render one library symbol of an imported document, frame by frame.
//!
//! cargo run --release -p buzz-app --example render_symbol -- input.fla "symbolName" out_prefix [frame ...] [--joints] [--crop1024] [--parts]
//!
//! The symbol is placed alone, at the centre of the stage, on an otherwise
//! empty document, so what comes out is the symbol's own timeline — a rigged
//! walk cycle, a head turn — without the scene it normally sits in. Writes
//! `out_prefix_<frame>.png` for each frame asked for.
//!
//! `--crop1024`: instead of the whole stage at `BUZZ_EXPORT_SCALE`, crops to
//! the character's own bounds (plus 4% margin) and scales so the result is
//! 1024px tall, transparent background. `--joints`' pixel coordinates are
//! reported in this same cropped space.
//!
//! `--parts`: additionally renders each recognised rig slot (Head, Chest,
//! Hips, Shoulder/Elbow/Hand L/R, Thigh/Knee/Foot L/R) alone — every other
//! layer hidden — into `<prefix>_<frame>.parts/<slot>.png`, in the exact
//! same framing as the main frame, for per-part ground-truth masks.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use buzz_scene::LayerId;

fn map_layer_to_slot(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    if n.contains("head") || n.contains("skull") || n.contains("face") {
        Some("Head")
    } else if n == "neck" {
        Some("Neck")
    } else if n.contains("body2") || n.contains("hip") || n.contains("pelvis") || n.contains("waist") {
        Some("Hips")
    } else if n.contains("body") || n.contains("chest") || n.contains("torso") || n.contains("ribs") {
        Some("Chest")
    } else if (n.contains("left") || n.contains("_l") || n.ends_with(" l")) && (n.contains("palm") || n.contains("hand")) {
        Some("Hand L")
    } else if (n.contains("right") || n.contains("_r") || n.ends_with(" r")) && (n.contains("palm") || n.contains("hand")) {
        Some("Hand R")
    } else if (n.contains("left") || n.contains("_l") || n.ends_with(" l")) && (n.contains("wrist") || n.contains("forearm") || n.contains("elbow")) {
        Some("Elbow L")
    } else if (n.contains("right") || n.contains("_r") || n.ends_with(" r")) && (n.contains("wrist") || n.contains("forearm") || n.contains("elbow")) {
        Some("Elbow R")
    } else if (n.contains("left") || n.contains("_l") || n.ends_with(" l")) && (n.contains("arm") || n.contains("bicep") || n.contains("shoulder")) {
        Some("Shoulder L")
    } else if (n.contains("right") || n.contains("_r") || n.ends_with(" r")) && (n.contains("arm") || n.contains("bicep") || n.contains("shoulder")) {
        Some("Shoulder R")
    } else if (n.contains("left") || n.contains("_l") || n.ends_with(" l")) && (n.contains("foot") || n.contains("boot") || n.contains("shoe") || n.contains("toe")) {
        Some("Foot L")
    } else if (n.contains("right") || n.contains("righ") || n.contains("_r") || n.ends_with(" r")) && (n.contains("foot") || n.contains("boot") || n.contains("shoe") || n.contains("toe")) {
        // "righ" (not just "right") tolerates a real source typo seen in
        // bhatta2.fla's sadhuSide symbol: a layer literally named "righFoot".
        Some("Foot R")
    } else if (n.contains("left") || n.contains("_l") || n.ends_with(" l")) && (n.contains("shin") || n.contains("knee") || n.contains("calf") || n.contains("lower leg")) {
        Some("Knee L")
    } else if (n.contains("right") || n.contains("_r") || n.ends_with(" r")) && (n.contains("shin") || n.contains("knee") || n.contains("calf") || n.contains("lower leg")) {
        Some("Knee R")
    } else if (n.contains("left") || n.contains("_l") || n.ends_with(" l")) && (n.contains("thigh") || n.contains("upper leg") || n.contains("leg")) {
        Some("Thigh L")
    } else if (n.contains("right") || n.contains("_r") || n.ends_with(" r")) && (n.contains("thigh") || n.contains("upper leg") || n.contains("leg")) {
        Some("Thigh R")
    } else {
        None
    }
}

/// Resolves each of `symbol`'s layers to a canonical rig slot name, at most
/// once per slot ("exactly one entry per joint name").
///
/// Two real-world wrinkles beyond plain name matching:
/// - **Split torso.** Several rigs name their torso in two layers that both
///   read as "Chest" by `map_layer_to_slot` (e.g. `sadhuSideTorso2` /
///   `sideTorso1Sadhu`, `ThakurTorsoBase` / `ThakurTorsoUpStraight`): a root
///   segment nothing else parents on top of `follows: None`, and a follower
///   that arms/head attach to. The one *other layers follow* is kept as
///   "Chest"; the followed root is relabelled "Hips" — a structural
///   distinction (who parents on whom), not a naming guess.
/// - **True duplicates.** If a slot still collides after that (no follow
///   relationship to disambiguate it), only the first layer encountered
///   keeps the slot; later ones are left unmapped, so no joint name is ever
///   emitted twice.
fn resolve_slots(symbol: &buzz_scene::Symbol) -> HashMap<LayerId, String> {
    let mut direct: Vec<(LayerId, &'static str)> = Vec::new();
    for l in symbol.layers.iter() {
        if let Some(slot) = map_layer_to_slot(&l.name) {
            direct.push((l.id, slot));
        }
    }

    let chest_ids: Vec<LayerId> = direct.iter().filter(|(_, s)| *s == "Chest").map(|(id, _)| *id).collect();
    let mut overrides: HashMap<LayerId, &'static str> = HashMap::new();
    if chest_ids.len() >= 2 {
        for &id in &chest_ids {
            if symbol.layers.iter().any(|l| l.follows == Some(id)) {
                overrides.insert(id, "Hips");
            }
        }
    }

    let mut seen: HashSet<&'static str> = HashSet::new();
    let mut out = HashMap::new();
    for (id, slot) in direct {
        let slot = overrides.get(&id).copied().unwrap_or(slot);
        if seen.insert(slot) {
            out.insert(id, slot.to_string());
        }
    }
    out
}

/// An object's bounds transformed into stage space, or `None` when it has
/// no extent. Each rig part here is a symbol instance, and
/// `Object::bounds()` cannot see through an instance to the library (it
/// returns a tiny 2x2 placeholder around the pivot, by design — see its doc
/// comment); `Scene::instance_bounds` resolves the real, posed extent
/// through the library.
fn object_stage_rect(scene: &buzz_scene::Scene, obj: &buzz_scene::Object, placed_trans: kurbo::Affine) -> Option<kurbo::Rect> {
    let b = scene.instance_bounds(obj).unwrap_or_else(|| obj.bounds());
    if b.width() <= 0.0 && b.height() <= 0.0 {
        return None;
    }
    let mut rect: Option<kurbo::Rect> = None;
    for (cx, cy) in [(b.x0, b.y0), (b.x1, b.y0), (b.x0, b.y1), (b.x1, b.y1)] {
        let p = placed_trans * kurbo::Point::new(cx, cy);
        rect = Some(match rect {
            None => kurbo::Rect::from_points(p, p),
            Some(r) => r.union_pt(p),
        });
    }
    rect
}

/// The character's own drawn bounds at `frame`, or `None` if nothing is
/// visible.
///
/// Two passes, so an unrelated prop or shadow on its own layer (observed in
/// `OldmanfrontWalk`: a thrown object and a ground-shadow ellipse, each its
/// own unnamed layer, ballooning the crop) doesn't count as "the character":
/// first, a core bbox from layers that resolved to a rig slot; then widen it
/// with any *other* layer whose bounds actually sit on the character (its
/// centre falls within the core, generously inflated) — clothing details
/// like `thailabag`/`midCloth` that carry no slot name but are drawn right
/// on the body, not a prop or shadow placed elsewhere on the same timeline.
fn character_bbox(
    scene: &buzz_scene::Scene,
    symbol: &buzz_scene::Symbol,
    frame: u32,
    placed_trans: kurbo::Affine,
    slot_map: &HashMap<LayerId, String>,
) -> Option<kurbo::Rect> {
    let mut core: Option<kurbo::Rect> = None;
    for l in symbol.layers.iter() {
        if !slot_map.contains_key(&l.id) {
            continue;
        }
        for obj in l.objects_at(frame) {
            if let Some(r) = object_stage_rect(scene, obj, placed_trans) {
                core = Some(match core {
                    None => r,
                    Some(c) => c.union(r),
                });
            }
        }
    }
    let core = core?;
    let tolerant = core.inflate(core.width() * 0.5, core.height() * 0.5);

    let mut bbox = core;
    for l in symbol.layers.iter() {
        if slot_map.contains_key(&l.id) {
            continue;
        }
        for obj in l.objects_at(frame) {
            if let Some(r) = object_stage_rect(scene, obj, placed_trans)
                && tolerant.contains(r.center())
            {
                bbox = bbox.union(r);
            }
        }
    }
    Some(bbox)
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let export_joints = if let Some(pos) = args.iter().position(|a| a == "--joints") {
        args.remove(pos);
        true
    } else {
        false
    };
    let crop1024 = if let Some(pos) = args.iter().position(|a| a == "--crop1024") {
        args.remove(pos);
        true
    } else {
        false
    };
    let export_parts = if let Some(pos) = args.iter().position(|a| a == "--parts") {
        args.remove(pos);
        true
    } else {
        false
    };

    let usage = "usage: render_symbol <file.fla> <symbol name> <out prefix> [frame ...] [--joints] [--crop1024] [--parts]";
    let mut args_iter = args.into_iter();
    let input = args_iter.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let name = args_iter.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let prefix = args_iter.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let mut frames: Vec<u32> = args_iter.map(|v| v.parse()).collect::<Result<_, _>>()?;

    let imported = buzz_app::import::read(std::path::Path::new(&input)).map_err(anyhow::Error::msg)?;
    let mut scene = imported.scene;
    // `BUZZ_SYMBOL_WITH_LAYER=name`: of several library symbols sharing
    // `name` (Animate's "Duplicate Items Folder" copies), the one that has a
    // layer called this.
    let with_layer = std::env::var("BUZZ_SYMBOL_WITH_LAYER").ok();
    let chosen = scene
        .library()
        .iter()
        .find(|s| s.name == name && with_layer.as_ref().is_none_or(|l| s.layers.iter().any(|x| &x.name == l)))
        .cloned();
    let Some(symbol) = chosen.or_else(|| scene.library().find_by_name(&name).cloned()) else {
        let mut names: Vec<_> = scene.library().iter().map(|s| s.name.clone()).collect();
        names.sort();
        anyhow::bail!("no symbol {name:?}; the library has: {}", names.join(", "));
    };
    // `BUZZ_ONLY_LAYERS=a,b`: draw only these layers of the symbol (plus any
    // mask they need), to see which layer is wrong.
    let symbol = if let Ok(only) = std::env::var("BUZZ_ONLY_LAYERS") {
        let keep: Vec<&str> = only.split(',').collect();
        let mut edited = (*symbol).clone();
        let ids: Vec<LayerId> = edited.layers.iter().map(|l| l.id).collect();
        for id in ids {
            edited.layers.update(id, |l| l.visible = keep.contains(&l.name.as_str()));
        }
        scene.library_mut().insert(edited);
        scene.library().get(symbol.id).cloned().unwrap()
    } else {
        symbol
    };
    // `BUZZ_ONLY_OBJECTS=0,2`: keep only these objects (by index) of every
    // keyframe of the symbol, to bisect one drawing shape by shape.
    let symbol = if let Ok(only) = std::env::var("BUZZ_ONLY_OBJECTS") {
        let keep: Vec<usize> = only.split(',').filter_map(|v| v.trim().parse().ok()).collect();
        let mut edited = (*symbol).clone();
        let ids: Vec<LayerId> = edited.layers.iter().map(|l| l.id).collect();
        for id in ids {
            edited.layers.update(id, |l| {
                for k in l.frames.keyframes_mut() {
                    let objects: Vec<_> = k.objects.iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, o)| o.clone()).collect();
                    k.objects = Arc::new(objects);
                }
            });
        }
        scene.library_mut().insert(edited);
        scene.library().get(symbol.id).cloned().unwrap()
    } else {
        symbol
    };
    let length = symbol.length().max(1);
    println!("{name}: {:?}, {length} frames, {} layers", symbol.kind, symbol.layers.len());
    for layer in symbol.layers.iter() {
        println!(
            "  layer {:<24} kind {:?} follows {:?} rest_pose {}",
            layer.name,
            layer.kind,
            layer.follows,
            layer.rest_pose.is_some()
        );
    }
    if frames.is_empty() {
        frames = (0..length).step_by((length as usize / 6).max(1)).collect();
    }

    let slot_map = resolve_slots(&symbol);
    if export_parts {
        let mut slots: Vec<_> = slot_map.values().cloned().collect();
        slots.sort();
        println!("  resolved {} part slot(s): {}", slots.len(), slots.join(", "));
    }

    // Everything else off the stage: the symbol alone, centred, on a timeline
    // as long as its own so every frame of it can be reached.
    let ids: Vec<_> = scene.layers().iter().map(|l| l.id).collect();
    for id in ids {
        scene.edit_layers().remove(id);
    }
    scene.camera_mut().enabled = false;
    let layer = scene.add_layer("probe", buzz_scene::LayerKind::Normal);
    let centre = scene.stage().stage_rect().center();
    // `BUZZ_PLACE=dx,dy,scale`: an explicit placement, for a symbol drawn far
    // from its own origin.
    let explicit = std::env::var("BUZZ_PLACE").ok().and_then(|v| {
        let p: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (p.len() == 3).then(|| kurbo::Affine::translate((p[0], p[1])) * kurbo::Affine::scale(p[2]))
    });
    let placed_trans = if let Some(t) = explicit {
        t
    } else if std::env::var_os("BUZZ_AT_ORIGIN").is_some() {
        kurbo::Affine::IDENTITY
    } else {
        kurbo::Affine::translate(centre.to_vec2())
    };
    let placed = scene.add_instance_at(layer, 0, symbol.id, placed_trans);
    println!("placed {placed:?} at {centre:?}, symbol kind {:?}", symbol.kind);
    // `BUZZ_FIRST_FRAME=n` and `BUZZ_LOOP=loop|once|single`: how the placed
    // graphic instance plays, to reproduce an Animate placement exactly.
    if let Some(id) = placed {
        let first: Option<u32> = std::env::var("BUZZ_FIRST_FRAME").ok().and_then(|v| v.parse().ok());
        let mode = std::env::var("BUZZ_LOOP").ok();
        if first.is_some() || mode.is_some() {
            scene.update_object_at(0, id, |o| {
                if let buzz_scene::ObjectKind::Instance(i) = &mut o.kind {
                    if let Some(f) = first {
                        i.first_frame = f;
                    }
                    i.loop_mode = match mode.as_deref() {
                        Some("once") => buzz_scene::LoopMode::PlayOnce,
                        Some("single") => buzz_scene::LoopMode::SingleFrame,
                        _ => buzz_scene::LoopMode::Loop,
                    };
                }
            });
            println!("instance first_frame={first:?} loop={mode:?}");
        }
    }
    scene.edit_layers().update(layer, |l| {
        let keys = l.frames.keyframes().to_vec();
        l.frames = buzz_scene::LayerTimeline::from_parts(keys, length);
    });
    let _ = Arc::clone(&symbol);
    for o in symbol.objects_at(0).iter().take(6) {
        let c = o.transform.as_coeffs();
        println!("  part at ({:.0},{:.0}) lin [{:.2} {:.2} {:.2} {:.2}] bounds {:?}", c[4], c[5], c[0], c[1], c[2], c[3], o.bounds());
    }

    let transparent = std::env::var_os("BUZZ_EXPORT_OPAQUE").is_none();
    let scale = std::env::var("BUZZ_EXPORT_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5);

    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    // `BUZZ_EXPORT_NO_CULL=1` renders without culling, to tell a culling
    // fault from a drawing one.
    if std::env::var_os("BUZZ_EXPORT_NO_CULL").is_some() {
        exporter.set_cull(false);
    }
    // `BUZZ_NO_SYMBOL_REUSE=1`: draw every instance live, to tell a cache
    // defect from a drawing defect.
    if std::env::var_os("BUZZ_NO_SYMBOL_REUSE").is_some() {
        exporter.set_symbol_reuse(false);
    }

    let all_layer_ids: Vec<LayerId> = symbol.layers.iter().map(|l| l.id).collect();

    for frame in frames {
        let l = scene.layers().iter().next().unwrap();
        println!("  doc frame {frame}: layer objects {}, symbol objects at frame {}", l.objects_at(frame).len(), symbol.objects_at(frame).len());

        let settings = if crop1024 {
            let bbox = character_bbox(&scene, &symbol, frame, placed_trans, &slot_map)
                .ok_or_else(|| anyhow::anyhow!("frame {frame}: nothing visible to crop to"))?;
            let pad_x = bbox.width() * 0.04;
            let pad_y = bbox.height() * 0.04;
            let region = kurbo::Rect::new(bbox.x0 - pad_x, bbox.y0 - pad_y, bbox.x1 + pad_x, bbox.y1 + pad_y);
            let out_h = 1024u32;
            let out_w = ((out_h as f64) * region.width() / region.height().max(1e-6)).round().max(1.0) as u32;
            buzz_export::ExportSettings {
                width: out_w,
                height: out_h,
                transparent,
                region: Some(region),
            }
        } else {
            let mut s = buzz_export::ExportSettings::scaled(&scene, scale);
            s.transparent = transparent;
            s
        };

        let picture = exporter.render(&scene, frame, &settings)?;
        let path = format!("{prefix}_{frame}.png");
        picture.write_png(std::path::Path::new(&path))?;
        println!("frame {frame} -> {path}");

        // Document-space -> pixel-space projection matching what the
        // renderer just used (the whole stage, or `settings.region` when
        // `--crop1024` cropped to the character).
        let (region_center, cam_zoom) = match settings.region {
            Some(r) => (r.center(), settings.width as f64 / r.width().max(1e-6)),
            None => (
                scene.stage().stage_rect().center(),
                settings.width as f64 / scene.stage().size.width.max(1.0),
            ),
        };
        let to_px = |p: kurbo::Point| -> (f64, f64) {
            (
                (p.x - region_center.x) * cam_zoom + settings.width as f64 / 2.0,
                (p.y - region_center.y) * cam_zoom + settings.height as f64 / 2.0,
            )
        };

        if export_joints {
            let mut joints: Vec<(String, (f64, f64))> = Vec::new();
            let mut found_armature = false;
            for obj in symbol.objects_at(frame) {
                if let buzz_scene::ObjectKind::Armature(arm_data) = &obj.kind {
                    found_armature = true;
                    for (i, bone) in arm_data.armature.bones.iter().enumerate() {
                        let head = arm_data.armature.head(i);
                        let world = placed_trans * obj.transform * kurbo::Point::new(head.x, head.y);
                        joints.push((bone.name.clone(), to_px(world)));
                    }
                }
            }
            if !found_armature {
                // A resolved pivot that doesn't land on the character's own
                // drawn silhouette (dilated a few percent of the frame, for
                // pivots that legitimately sit just outside the ink, like
                // "Head" just above the hairline) means the rigging
                // resolution itself failed for that layer at this frame —
                // observed on sadhuSide's "rightPalm": `resolved_at_rigged`
                // at frame 0 landed over 1000px from a 393px-wide crop,
                // while its structurally identical sibling "leftPalm"
                // resolved fine — not that the joint truly sits there.
                // Dropping it is more honest than writing a garbage truth
                // coordinate; this is the same bar
                // `ground_truth_joints_lie_inside_dilated_matte` checks.
                let dilate_px = (settings.width.max(settings.height) as f64 * 0.05).max(12.0);
                let inside_dilated_matte = |px: (f64, f64)| -> bool {
                    let (w, h) = (picture.width as i32, picture.height as i32);
                    let r = dilate_px.ceil() as i32;
                    let (cx, cy) = (px.0.round() as i32, px.1.round() as i32);
                    for dy in -r..=r {
                        for dx in -r..=r {
                            if (dx * dx + dy * dy) as f64 > dilate_px * dilate_px {
                                continue;
                            }
                            let (x, y) = (cx + dx, cy + dy);
                            if x >= 0 && y >= 0 && x < w && y < h && picture.pixel(x as u32, y as u32)[3] > 16 {
                                return true;
                            }
                        }
                    }
                    false
                };
                for (id, slot) in &slot_map {
                    let resolved = symbol.layers.resolved_at_rigged(*id, frame);
                    if let Some(obj) = resolved.iter().next() {
                        let pivot = obj.transform * kurbo::Point::ZERO;
                        let world = placed_trans * pivot;
                        let px = to_px(world);
                        if !inside_dilated_matte(px) {
                            eprintln!(
                                "  WARNING: {slot} (layer {id:?}) resolved to ({:.1},{:.1}), outside the drawn silhouette (dilated {dilate_px:.0}px); dropped",
                                px.0, px.1
                            );
                            continue;
                        }
                        joints.push((slot.clone(), px));
                    }
                }

                // "Chest"/"Hips" container layers don't reliably carry a
                // joint-like registration point the way limb layers do (a
                // rigger sets an arm's pivot at the shoulder so it rotates
                // correctly there; a torso panel's pivot can be anywhere —
                // verified against pujariFront, where the raw "body"/"body2"
                // pivots landed near shoulder/elbow height instead of the
                // sternum/pelvis). Re-derive them the way the detector
                // defines the same joints (`character::skeleton`: pelvis =
                // hip midpoint, chest = midpoint of neck-proxy and pelvis)
                // from limb joints that do carry a reliable pivot, so truth
                // and detection mean the same thing.
                let find = |joints: &[(String, (f64, f64))], name: &str| {
                    joints.iter().find(|(n, _)| n == name).map(|(_, p)| *p)
                };
                if let (Some(tl), Some(tr)) = (find(&joints, "Thigh L"), find(&joints, "Thigh R")) {
                    let hips = ((tl.0 + tr.0) / 2.0, (tl.1 + tr.1) / 2.0);
                    if let (Some(sl), Some(sr)) = (find(&joints, "Shoulder L"), find(&joints, "Shoulder R")) {
                        let neck = ((sl.0 + sr.0) / 2.0, (sl.1 + sr.1) / 2.0);
                        let chest = ((neck.0 + hips.0) / 2.0, (neck.1 + hips.1) / 2.0);
                        joints.retain(|(n, _)| n != "Chest");
                        joints.push(("Chest".to_string(), chest));
                    }
                    joints.retain(|(n, _)| n != "Hips");
                    joints.push(("Hips".to_string(), hips));
                }
            }
            let joints_path = format!("{prefix}_{frame}.joints.json");
            let json_data = serde_json::to_string_pretty(&joints)?;
            std::fs::write(&joints_path, json_data)?;
            println!("joints {frame} -> {joints_path} ({} joints)", joints.len());
        }

        if export_parts {
            let parts_dir = format!("{prefix}_{frame}.parts");
            std::fs::create_dir_all(&parts_dir)?;
            let mut written = 0usize;

            // Frontmost-first order (layer index 0 is the topmost row —
            // buzz-scene's `layer.rs`), so the topmost-part label map below
            // can claim each pixel the first time a part covers it.
            let mut ordered_slots: Vec<(LayerId, String)> =
                slot_map.iter().map(|(id, s)| (*id, s.clone())).collect();
            ordered_slots.sort_by_key(|(id, _)| all_layer_ids.iter().position(|x| x == id).unwrap_or(usize::MAX));

            let mut label_map = vec![0u8; (settings.width * settings.height) as usize];
            let mut label_names: Vec<String> = Vec::new();

            for (id, slot) in &ordered_slots {
                // Reset to the pristine symbol, then hide every layer except
                // this one, so the isolated render lines up pixel-for-pixel
                // with the main frame above (same `settings`).
                scene.library_mut().insert((*symbol).clone());
                scene.library_mut().update(symbol.id, |s| {
                    for &other in &all_layer_ids {
                        if other != *id {
                            s.layers.update(other, |l| l.visible = false);
                        }
                    }
                });
                let part_picture = exporter.render(&scene, frame, &settings)?;
                let slug = slot.to_lowercase().replace(' ', "_");
                let part_path = format!("{parts_dir}/{slug}.png");
                part_picture.write_png(std::path::Path::new(&part_path))?;
                written += 1;

                // Topmost-part label map (item 5 of the "Opus review of the
                // quality run" redesign): this part claims every pixel it
                // covers that no frontmost-of-it part has already claimed —
                // "the pixels where that part is the topmost in the
                // composite render", used to build fair, visible-only C2 IoU
                // truth.
                label_names.push(slot.clone());
                let label_value = label_names.len() as u8; // 1-based; 0 = none
                for py in 0..settings.height {
                    for px in 0..settings.width {
                        let idx = (py * settings.width + px) as usize;
                        if label_map[idx] == 0 && part_picture.pixel(px, py)[3] > 128 {
                            label_map[idx] = label_value;
                        }
                    }
                }
            }

            let mut label_png = image::GrayImage::new(settings.width, settings.height);
            for (i, &v) in label_map.iter().enumerate() {
                label_png.put_pixel(i as u32 % settings.width, i as u32 / settings.width, image::Luma([v]));
            }
            let label_path = format!("{parts_dir}/_topmost.png");
            label_png.save(&label_path)?;
            let names_path = format!("{parts_dir}/_topmost.json");
            std::fs::write(&names_path, serde_json::to_string_pretty(&label_names)?)?;

            // Restore the pristine symbol so the next frame's main render
            // (and its own --parts pass) starts clean.
            scene.library_mut().insert((*symbol).clone());
            println!("parts {frame} -> {parts_dir} ({written} part(s), topmost label map -> {label_path})");
        }
    }
    Ok(())
}
