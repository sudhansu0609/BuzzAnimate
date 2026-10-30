//! scratch demo — delete after use. A character flattened into one drawing,
//! rigged by AI ▸ Rig Character as Layers (style cut) + Rig Head, saved.
//!
//! zz_full_rig_demo <in.buzz> <symbol> <out.buzz> [png dir]
use buzz_geom::Affine;
use buzz_geom::Rect;
use buzz_scene::{Keyframe, Layer, LayerId, LayerKind, LayerTimeline, Object, Scene, SymbolId, SymbolKind};
use image::RgbaImage;
use std::sync::Arc;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (input, name, out) = (&a[0], &a[1], &a[2]);
    let (scenes, active) = buzz_doc::format::load_scenes(input).expect("load");
    let mut scene = scenes.into_iter().nth(active).unwrap().1;
    let source = scene.library().iter().find(|s| s.name == *name).map(|s| s.id).expect("symbol");

    // The character at rest as one drawing (masks applied).
    let probe = Object::instance_of(scene.next_object_id(), source);
    let shapes = buzz_app::rigging::flatten_rest(&scene, &probe, Affine::IDENTITY);
    eprintln!("flattened {} shapes", shapes.len());
    let mut objects = Vec::new();
    for s in shapes {
        objects.push(Arc::new(Object::shape(scene.next_object_id(), s)));
    }
    let drawn = scene.add_symbol(format!("{name} rigged"), SymbolKind::Graphic, None);
    scene.library_mut().update(drawn, |s| {
        let first = s.layers.iter().next().map(|l| l.id);
        if let Some(layer) = first {
            s.layers.update(layer, |l| {
                let mut k = Keyframe::new(0);
                k.objects = Arc::new(objects);
                l.frames = LayerTimeline::from_parts(vec![k], 1);
            });
        }
    });

    // The stage: only the new character.
    let inst = Arc::new(Object::instance_of(scene.next_object_id(), drawn));
    let mut stage = Layer::new(LayerId(scene.next_object_id().0), "Character", LayerKind::Normal);
    let mut k = Keyframe::new(0);
    k.objects = Arc::new(vec![inst]);
    stage.frames = LayerTimeline::from_parts(vec![k], 1);
    let old: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
    let layers = scene.edit_layers();
    for id in old {
        layers.remove(id);
    }
    layers.push_front(stage);

    // AI ▸ Rig Character as Layers, with "Rig head like my characters".
    let profile = buzz_app::rigging::naming_profile(None).expect("profile");
    let mut plan = buzz_app::rigging::plan_layer_rig_auto(&scene, drawn, 0, &profile, Some(true)).expect("plan");
    plan.rig_head = true;
    for n in &plan.notes {
        eprintln!("note: {n}");
    }
    eprintln!("pieces {}  head symbol {:?}", plan.pieces.len(), plan.head_symbol);
    let applied = buzz_app::rigging::apply_layer_rig_plan(&mut scene, &plan).expect("apply");
    eprintln!("layers after {}  head symbol {:?} (made {})", applied.layers_after, applied.head_symbol, applied.head_made);
    // As the editor's AI Rig as Layers: Rig Head on the applied head symbol.
    let mut summary = serde_json::json!({"head_symbol_made": applied.head_made});
    match applied.head_symbol {
        Some(h) => {
            eprintln!("head symbol {:?}", scene.library().get(h).map(|s| s.name.clone()));
            let before = scene.clone();
            let style = buzz_rig::head_style::HeadStyle::animate_convention();
            match buzz_app::rigging::rig_head(&mut scene, h, &style) {
                Ok(r) => {
                    eprintln!("rig head: {r:?}");
                    summary["eyes"] = r.eyes.into();
                    summary["eye_holes"] = r.eye_holes.into();
                    summary["view"] = r.view.clone().into();
                }
                Err(e) => eprintln!("rig head refused: {e}"),
            }
            let layers: Vec<String> = scene.library().get(h).map(|s| s.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default();
            for l in scene.library().get(h).unwrap().layers.iter() {
                eprintln!("    head layer {:?} {:?} frames {}", l.name, l.kind, l.frames.length());
            }
            summary["brows"] = layers.iter().filter(|n| n.to_lowercase().contains("brow")).count().into();
            summary["head_layers"] = layers.clone().into();
            if let Some(dir) = a.get(3) {
                buzz_doc::format::save(&before, &format!("{dir}/{name}_before.buzz")).expect("save before");
                renders(&before, &scene, h, name, dir, &mut summary);
            }
        }
        None => eprintln!("no head symbol"),
    }
    println!("{}", serde_json::to_string(&summary).unwrap());
    let s = scene.library().get(drawn).unwrap();
    let names: std::collections::HashMap<_, _> = s.layers.iter().map(|l| (l.id, l.name.clone())).collect();
    for l in s.layers.iter() {
        let what = l.objects_at(0).iter().map(|o| match o.instance() { Some(i) => scene.library().get(i.symbol).map(|s| s.name.clone()).unwrap_or_default(), None => "shape".into() }).collect::<Vec<_>>().join(",");
        eprintln!("  layer {:?} follows {:?} holds [{}]", l.name, l.follows.and_then(|f| names.get(&f).cloned()), what.chars().take(80).collect::<String>());
    }
    buzz_doc::format::save(&scene, out).expect("save");
    eprintln!("saved {out}");
}

/// `sym` alone on the stage at identity.
fn stage_alone(scene: &Scene, sym: SymbolId) -> Scene {
    let mut scene = scene.clone();
    let o = Object::instance_of(scene.next_object_id(), sym);
    let mut layer = Layer::new(LayerId(scene.next_object_id().0), "probe", LayerKind::Normal);
    let mut k = Keyframe::new(0);
    k.objects = Arc::new(vec![Arc::new(o)]);
    layer.frames = LayerTimeline::from_parts(vec![k], 1);
    let stack = scene.edit_stage_layers();
    let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
    for id in ids {
        stack.remove(id);
    }
    stack.insert(0, layer);
    scene.camera_mut().enabled = false;
    scene
}

fn render(ex: &mut buzz_export::Exporter, scene: &Scene, region: Rect, scale: f64) -> RgbaImage {
    let s = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent: true,
        region: Some(region),
    };
    let f = ex.render(scene, 0, &s).unwrap();
    RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
}

fn on_white(img: &RgbaImage) -> RgbaImage {
    on(img, [255.0; 3])
}

/// Over a flat colour (a layer alone goes on grey, so a white mask shows).
fn on(img: &RgbaImage, bg: [f64; 3]) -> RgbaImage {
    let mut o = img.clone();
    for p in o.pixels_mut() {
        let a = p[3] as f64 / 255.0;
        for k in 0..3 {
            p[k] = (p[k] as f64 * a + bg[k] * (1.0 - a)).round() as u8;
        }
        p[3] = 255;
    }
    o
}

/// The head before/after rigging at 4x, each head layer alone, and the rest
/// diff (alpha and premultiplied colour, per channel, > 64).
fn renders(before: &Scene, after: &Scene, head: SymbolId, name: &str, dir: &str, summary: &mut serde_json::Value) {
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let probe = Object::instance_of(buzz_scene::ObjectId(u64::MAX - 7), head);
    let b = before.instance_bounds(&probe).unwrap_or(Rect::new(-100.0, -100.0, 100.0, 100.0));
    let b2 = after.instance_bounds(&probe).unwrap_or(b);
    let region = b.union(b2).inflate(4.0, 4.0);
    let scale = 4.0;
    let img0 = render(&mut ex, &stage_alone(before, head), region, scale);
    let img1 = render(&mut ex, &stage_alone(after, head), region, scale);
    on_white(&img0).save(format!("{dir}/{name}_head_before.png")).unwrap();
    on_white(&img1).save(format!("{dir}/{name}_head_after.png")).unwrap();
    // Also counted only where the head was painted before (a see-through
    // eye hole made into an eye is meant to change).
    let (mut over, mut max, mut over_painted) = (0usize, 0i32, 0usize);
    let mut diff = RgbaImage::new(img0.width(), img0.height());
    for ((p, q), d) in img0.pixels().zip(img1.pixels()).zip(diff.pixels_mut()) {
        let pm = |x: &image::Rgba<u8>, k: usize| (x[k] as i32 * x[3] as i32) / 255;
        let mut m = (p[3] as i32 - q[3] as i32).abs();
        for k in 0..3 {
            m = m.max((pm(p, k) - pm(q, k)).abs());
        }
        max = max.max(m);
        if m > 64 {
            over += 1;
            over_painted += (p[3] > 0) as usize;
            *d = image::Rgba([255, 0, 0, 255]);
        } else {
            let g = 255 - (m * 3).min(200) as u8;
            *d = image::Rgba([g, g, g, 255]);
        }
    }
    diff.save(format!("{dir}/{name}_head_diff.png")).unwrap();
    summary["rest_changed_px"] = over.into();
    summary["rest_changed_px_painted"] = over_painted.into();
    summary["rest_max_diff"] = max.into();
    summary["rest_px"] = (img0.width() * img0.height()).into();
    // Each layer alone (masks drawn as plain layers).
    let mut pngs = vec![format!("{dir}/{name}_head_before.png"), format!("{dir}/{name}_head_after.png"), format!("{dir}/{name}_head_diff.png")];
    let layers: Vec<(LayerId, String)> = after.library().get(head).unwrap().layers.iter().filter(|l| l.kind != LayerKind::Folder).map(|l| (l.id, l.name.clone())).collect();
    for (i, (id, lname)) in layers.iter().enumerate() {
        let mut sc = after.clone();
        sc.library_mut().update(head, |s| {
            let ids: Vec<LayerId> = s.layers.iter().map(|l| l.id).collect();
            for other in ids {
                s.layers.update(other, |l| {
                    if l.kind == LayerKind::Folder {
                        return;
                    }
                    if other == *id {
                        l.visible = true;
                        if matches!(l.kind, LayerKind::Mask | LayerKind::Masked | LayerKind::InverseMask) {
                            l.kind = LayerKind::Normal;
                        }
                    } else {
                        l.visible = false;
                    }
                });
            }
        });
        let img = render(&mut ex, &stage_alone(&sc, head), region, scale);
        let path = format!("{dir}/{name}_layer{i:02}_{}.png", lname.replace(' ', "_"));
        on(&img, [190.0, 190.0, 200.0]).save(&path).unwrap();
        pngs.push(path);
    }
    summary["pngs"] = pngs.into();
}
