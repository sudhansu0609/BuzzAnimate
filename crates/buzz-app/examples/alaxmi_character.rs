//! alaxmi_character: build ALAKSHMI's Cast rigs from cut parts (ALAXMI_PRODUCTION_PLAN B1c).
//!
//!   alaxmi_character dump <asset>                       layers, parents, parts of a library rig
//!   alaxmi_character height <asset>                     drawn height (character_bounds) of a library rig
//!   alaxmi_character build --spec spec.json --out file.buzz [--height H] [--relax DEG] [--name N]
//!       the spec (cut parts placed in picture pixels, back to front, each with its joint, and the mouth chart's
//!       ten frames) -> one layer rig, traced with `TraceOptions::bold()`, arms hung DEG below horizontal, drawn H tall.
//!   alaxmi_character save <file.buzz> <name>            into Cast/Khayal3Baje (back the folder up first)
//!   alaxmi_character render <file.buzz|asset> <out.png> [--mouth N] [--rotate "Layer:deg" ...] [--px N]
use std::path::Path;
use std::sync::Arc;

use buzz_doc::AssetLibrary;
use buzz_geom::{Affine, Point, Rect};
use buzz_scene::{LoopMode, Object, ObjectKind, Scene, SymbolKind};
use serde_json::Value;

fn load_any(name: &str) -> Scene {
    if Path::new(name).is_file() {
        return buzz_doc::format::load(Path::new(name)).expect("load");
    }
    let lib = AssetLibrary::user();
    let a = lib.assets().iter().find(|a| a.name.eq_ignore_ascii_case(name)).expect("asset");
    lib.load(a).unwrap()
}

fn dump(name: &str) {
    let scene = load_any(name);
    println!("stage {:?}", scene.stage().size);
    for l in scene.layers().iter() {
        let objs = l.objects_at(0);
        let desc: Vec<String> = objs
            .iter()
            .map(|o| match &o.kind {
                ObjectKind::Instance(i) => {
                    let s = scene.library().get(i.symbol);
                    format!("inst {:?} len={}", s.map(|s| s.name.clone()), s.map(|s| s.length()).unwrap_or(0))
                }
                ObjectKind::Shape(_) => "shape".into(),
                ObjectKind::Group(g) => format!("group{}", g.len()),
                _ => "other".into(),
            })
            .collect();
        println!("{:?} {:?} follows={:?} {:?}", l.id, l.name, l.follows, desc);
    }
}

fn pt(v: &Value) -> Point {
    Point::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}

/// The traced part: a group of its shapes in picture pixels (blurs on the soft ones).
fn traced(scene: &mut Scene, png: &Path, x: f64, y: f64) -> Object {
    let img = image::open(png).unwrap_or_else(|e| panic!("{}: {e}", png.display())).to_rgba8();
    let report = buzz_scene::trace(img.width(), img.height(), img.as_raw(), &buzz_scene::TraceOptions::bold());
    let blurs: std::collections::HashMap<usize, f64> = report.blurs.iter().copied().collect();
    let shift = Affine::translate((x, y));
    let mut kids = Vec::new();
    for (i, mut shape) in report.shapes.into_iter().enumerate() {
        shape.path = shift * shape.path;
        let mut o = Object::shape(scene.next_object_id(), shape);
        if let Some(&r) = blurs.get(&i) {
            o.filters.push(buzz_scene::TraceReport::blur_filter(r));
        }
        kids.push(Arc::new(o));
    }
    Object::group(scene.next_object_id(), kids)
}

fn build(spec_path: &Path, out: &Path, height: Option<f64>, relax: Option<f64>, name: &str) {
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(spec_path).unwrap()).unwrap();
    let dir = spec_path.parent().unwrap();
    let mut scene = Scene::default();
    // Where the picture goes: scaled so the figure is `height` tall, centred on the stage.
    let (top, bottom) = (spec["height_ref"][0].as_f64().unwrap(), spec["height_ref"][1].as_f64().unwrap());
    let cx = spec["centre_x"].as_f64().unwrap();
    let s = height.map_or(1.0, |h| h / (bottom - top));
    scene.stage_mut().size = buzz_geom::Size::new(1920.0, 1080.0);
    let stage = scene.stage().size;
    let place = Affine::translate((stage.width / 2.0, stage.height / 2.0)) * Affine::scale(s) * Affine::translate((-cx, -(top + bottom) / 2.0));
    let hang = relax.map(f64::to_radians);
    let arm_turn = |arm: &str| -> Affine {
        let Some(h) = hang else { return Affine::IDENTITY };
        let sh = pt(&spec[format!("shoulder_{arm}")]);
        // From straight out sideways to `h` below horizontal.
        let angle = if arm == "L" { -h } else { h };
        Affine::rotate_about(angle, sh)
    };

    let pattern = buzz_rig::RigPattern::biped_full();
    let mut parts = Vec::new();
    for p in spec["parts"].as_array().unwrap() {
        let slot = p["slot"].as_u64().unwrap() as usize;
        let layer_name = p["layer"].as_str().unwrap().to_string();
        let arm = p["arm"].as_str().unwrap_or("");
        let turn = if arm.is_empty() { Affine::IDENTITY } else { arm_turn(arm) };
        let joint = pt(&p["joint"]);
        let mut art = if p["mouth"].as_bool().unwrap_or(false) {
            // The chart: a symbol with a frame per viseme (buzz_audio::Viseme::frame), the panel for each.
            let sym = scene.add_symbol(format!("{name} mouth"), SymbolKind::Graphic, Some("Cast parts"));
            let inner = scene.library().get(sym).and_then(|s| s.layers.iter().next()).map(|l| l.id).unwrap();
            let frames = p["frames"].as_array().unwrap();
            let mut groups: Vec<Option<Arc<Object>>> = vec![None; frames.len()];
            let mut per_frame = Vec::new();
            for (f, fr) in frames.iter().enumerate() {
                let src = fr["same_as"].as_u64().map(|v| v as usize).unwrap_or(f);
                if groups[src].is_none() {
                    let fr_src = &frames[src];
                    let g = traced(&mut scene, &dir.join(fr_src["png"].as_str().unwrap()), fr_src["x"].as_f64().unwrap(), fr_src["y"].as_f64().unwrap());
                    groups[src] = Some(Arc::new(g));
                }
                per_frame.push(groups[src].clone().unwrap());
            }
            scene.library_mut().update(sym, |sy| {
                sy.layers.update(inner, |l| {
                    for (f, g) in per_frame.iter().enumerate() {
                        l.frames.insert_keyframe(f as u32);
                        l.frames.set_objects(f as u32, vec![g.clone()]);
                    }
                });
            });
            let mut o = Object::instance_of(scene.next_object_id(), sym);
            if let ObjectKind::Instance(i) = &mut o.kind {
                i.first_frame = 0;
                i.loop_mode = LoopMode::SingleFrame;
            }
            o.name = Some("mouth".into());
            o
        } else {
            traced(&mut scene, &dir.join(p["png"].as_str().unwrap()), p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap())
        };
        art.transform = place * turn * art.transform;
        parts.push(buzz_app::rigging::PartSpec { slot, name: layer_name, artwork: art, joint: place * turn * joint });
    }
    let n = parts.len();
    scene.stage_mut().background = peniko::Color::WHITE;
    buzz_app::rigging::rig_parts_as_layers(&mut scene, 0, &pattern, parts).expect("rig");
    // The head layer leads its slot (not the front-most hair layer): the hair layers follow the head, the head
    // follows what the hair layer followed (the chest).
    let by_name = |scene: &Scene, n: &str| scene.layers().iter().find(|l| l.name == n).map(|l| l.id);
    if let (Some(head), Some(hf)) = (by_name(&scene, "Head"), by_name(&scene, "Hair Front")) {
        let up = scene.layers().get(hf).and_then(|l| l.follows);
        if up != Some(head) {
            scene.update_layer(head, |l| l.follows = up);
            for n in ["Hair Back", "Hair Front"] {
                if let Some(h) = by_name(&scene, n) {
                    scene.update_layer(h, |l| l.follows = Some(head));
                }
            }
        }
    }
    // The starter layer is empty: drop it.
    let empty: Vec<_> = scene.layers().iter().filter(|l| l.objects_at(0).is_empty()).map(|l| l.id).collect();
    for id in empty {
        scene.remove_layer(id);
    }
    let first = scene.layers().iter().next().unwrap().id;
    let rig = buzz_act::layer_rig::rig_of_layer(&scene, first);
    if let Some(view) = &rig {
        buzz_act::layer_rig::record_rests(&mut scene, view);
    }
    // Hair layers are not rig slots: record their rest by hand so a later performance measures from it.
    for n in ["Hair Back", "Hair Front"] {
        if let Some(id) = by_name(&scene, n) {
            let rest = scene.layers().get(id).and_then(|l| l.frames.keyframes().iter().find_map(|k| k.objects.first()).map(|o| o.transform));
            scene.update_layer(id, |l| l.rest_pose = l.rest_pose.or(rest));
        }
    }
    buzz_doc::format::save(&scene, out).unwrap();
    let b = buzz_app::cast_fla::character_bounds(&scene, 0).unwrap();
    println!("{n} parts; rig found: {}; layers {}; bounds {:?} (h {:.1}); scale {s:.4}", rig.is_some(), scene.layers().iter().count(), b, b.height());
}

fn render(scene_in: &str, out: &Path, mouth: Option<u32>, rotations: &[(String, f64)], px: f64, transparent: bool) {
    let mut scene = load_any(scene_in);
    if let Some(m) = mouth {
        let ids: Vec<_> = scene.layers().iter().flat_map(|l| l.objects_at(0).iter().map(|o| o.id).collect::<Vec<_>>()).collect();
        for id in ids {
            scene.update_object_at(0, id, |o| {
                if let ObjectKind::Instance(i) = &mut o.kind {
                    i.first_frame = m;
                    i.loop_mode = LoopMode::SingleFrame;
                }
            });
        }
    }
    for (layer_name, deg) in rotations {
        let layer = scene.layers().iter().find(|l| l.name.eq_ignore_ascii_case(layer_name)).map(|l| l.id).expect("layer");
        let obj = scene.layers().get(layer).unwrap().objects_at(0)[0].clone();
        let local = obj.pivot.unwrap_or_else(|| obj.local_bounds().center());
        let world = obj.transform * local;
        let turn = Affine::rotate_about(deg.to_radians(), world);
        scene.update_object_at(0, obj.id, |o| o.transform = turn * o.transform);
    }
    let region = buzz_app::cast_fla::character_bounds(&scene, 0).unwrap();
    let pad = region.width().max(region.height()) * 0.06;
    let region = Rect::new(region.x0 - pad, region.y0 - pad, region.x1 + pad, region.y1 + pad);
    let k = px / region.width().max(region.height());
    let settings = buzz_export::ExportSettings {
        width: (region.width() * k).round().max(1.0) as u32,
        height: (region.height() * k).round().max(1.0) as u32,
        transparent,
        region: Some(region),
    };
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    scene.stage_mut().background = peniko::Color::WHITE;
    let frame = ex.render(&scene, 0, &settings).unwrap();
    frame.write_png(out).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let flag = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    match a.first().map(String::as_str) {
        Some("dump") => dump(&a[1]),
        Some("height") => {
            let s = load_any(&a[1]);
            let b = buzz_app::cast_fla::character_bounds(&s, 0).unwrap();
            println!("{} height {:.2} width {:.2} bounds {:?}", a[1], b.height(), b.width(), b);
        }
        Some("build") => build(
            Path::new(&flag("--spec").unwrap()),
            Path::new(&flag("--out").unwrap()),
            flag("--height").map(|v| v.parse().unwrap()),
            flag("--relax").map(|v| v.parse().unwrap()),
            &flag("--name").unwrap_or_else(|| "ALAKSHMI".into()),
        ),
        Some("save") => {
            let scene = buzz_doc::format::load(Path::new(&a[1])).unwrap();
            let mut lib = AssetLibrary::user();
            let asset = lib.save(&a[2], "Cast/Khayal3Baje", &scene).unwrap();
            println!("saved {}", asset.path.display());
        }
        Some("ready") => {
            let lib = AssetLibrary::user();
            for n in &a[1..] {
                match buzz_app::my_cast::readiness_of(&lib, n) {
                    Some(r) => println!("{}", r.line()),
                    None => println!("{n}: not in the library"),
                }
            }
        }
        Some("render") => {
            let rots: Vec<(String, f64)> = a
                .windows(2)
                .filter(|w| w[0] == "--rotate")
                .map(|w| {
                    let (n, d) = w[1].rsplit_once(':').unwrap();
                    (n.to_string(), d.parse().unwrap())
                })
                .collect();
            render(&a[1], Path::new(&a[2]), flag("--mouth").map(|v| v.parse().unwrap()), &rots, flag("--px").map_or(900.0, |v| v.parse().unwrap()), a.iter().any(|x| x == "--transparent"));
        }
        _ => eprintln!("usage: alaxmi_character dump|height|build|save|render ..."),
    }
}
