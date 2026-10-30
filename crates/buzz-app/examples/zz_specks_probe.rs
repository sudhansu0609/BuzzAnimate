//! scratch probe (TRACE_SPILL_AND_OUTPAINT_PLAN A6b): Remove Background Specks
//! headlessly on a COPY of a .buzz, with the command's defaults; writes the
//! cleaned copy, before/after renders, and one JSON line.
//! Usage: zz_specks_probe <file.buzz> <out dir> [symbol name|-] [scale]
//! With a symbol (default: the file's stem, e.g. "Lalten", when the library has
//! it) the scope is that symbol (as if its instance were selected) and it is
//! rendered alone; otherwise (`-`) the whole stage of every scene (A6e: a
//! Cast/Layers rig is drawn from its stage layers, so use `-` for those). Renders (transparent, and over
//! magenta) at 1x and at `scale` (default 3): `<name>_{before,after}_{1x,Nx}.png`,
//! `<name>_{before,after}_{1x,Nx}_alpha.png`. The cleaned file is
//! `<out dir>/<file name>`; it must not be the input.
use buzz_geom::Rect;
use buzz_scene::{Keyframe, Layer, LayerId, LayerKind, LayerTimeline, Object, ObjectKind, Scene, SymbolId};
use image::RgbaImage;
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;

fn alone(scene: &Scene, sym: SymbolId) -> (Scene, Option<Rect>) {
    let mut scene = scene.clone();
    let o = Object::instance_of(scene.next_object_id(), sym);
    let bounds = scene.instance_bounds(&o);
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
    (scene, bounds)
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

fn on_magenta(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let a = p[3] as f64 / 255.0;
        let bg = [255.0, 0.0, 255.0];
        for c in 0..3 {
            p[c] = (p[c] as f64 * a + bg[c] * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
        }
        p[3] = 255;
    }
    out
}

fn count_shapes(scene: &Scene) -> usize {
    let mut n = 0;
    let mut visit = |stack: &buzz_scene::LayerStack| {
        for layer in stack.iter() {
            for kf in layer.frames.keyframes() {
                n += kf.objects.iter().filter(|o| matches!(o.kind, ObjectKind::Shape(_))).count();
            }
        }
    };
    visit(scene.stage_layers());
    for s in scene.library().iter() {
        visit(&s.layers);
    }
    n
}

fn main() {
    unsafe { std::env::set_var("BUZZ_NO_SYMBOL_CACHE", "1") };
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (path, out) = (&a[0], &a[1]);
    std::fs::create_dir_all(out).unwrap();
    let file_name = std::path::Path::new(path).file_name().unwrap().to_string_lossy().to_string();
    let saved = std::path::Path::new(out).join(&file_name);
    assert!(
        std::fs::canonicalize(path).ok() != std::fs::canonicalize(&saved).ok(),
        "the output would overwrite the input"
    );
    let stem = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
    let name = stem.to_lowercase().replace(' ', "_");
    let scale: f64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let loaded = buzz_doc::format::load_scenes_full(path).expect("load");
    let active = loaded.active;
    let mut scene = loaded.scenes[active].1.clone();
    scene.camera_mut().enabled = false;
    // `-`: no symbol (the whole stage); no argument: the file's stem.
    let wanted = match a.get(2) {
        Some(w) if w == "-" => String::new(),
        Some(w) => w.clone(),
        None => stem.clone(),
    };
    let symbol = scene.library().iter().find(|s| s.name == wanted).map(|s| s.id);
    if std::env::var_os("BUZZ_PROBE_DUMP").is_some() {
        for s in scene.library().iter() {
            eprintln!("symbol {} {:?} kind {:?} length {}", s.id.0, s.name, s.kind, s.length());
            for layer in s.layers.iter() {
                let keys: Vec<String> = layer
                    .frames
                    .keyframes()
                    .iter()
                    .map(|k| {
                        let inner: Vec<String> = k
                            .objects
                            .iter()
                            .map(|o| match &o.kind {
                                ObjectKind::Shape(_) => "S".to_string(),
                                ObjectKind::Instance(i) => format!(
                                    "I{}({:?},ff{:?})",
                                    i.symbol.0,
                                    o.name,
                                    format!("{:?}", i).chars().take(200).collect::<String>()
                                ),
                                other => format!("{:?}", std::mem::discriminant(other)),
                            })
                            .collect();
                        format!("@{}[{}]", k.start, inner.join(" "))
                    })
                    .collect();
                eprintln!("  layer {:?} {:?} visible {} : {}", layer.name, layer.kind, layer.visible, keys.join(" "));
            }
        }
    }
    let scope = match symbol {
        Some(sym) => buzz_app::rigging::EvenLinesScope { symbols: vec![sym], ..Default::default() },
        None => buzz_app::rigging::EvenLinesScope { whole_timeline: true, ..Default::default() },
    };
    let mut after = scene.clone();
    let t0 = Instant::now();
    // `BUZZ_PROBE_COLOUR=#rrggbb`: the paper colour, as the command's `colour` arg.
    let mut options = buzz_scene::background_specks::SpeckOptions::default();
    if let Ok(hex) = std::env::var("BUZZ_PROBE_COLOUR") {
        let h = hex.trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
        options.colour = Some([byte(0), byte(2), byte(4)]);
    }
    let report = buzz_app::rigging::remove_background_specks(&mut after, &scope, &options);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    // The cleaned copy, every scene and the document metadata as loaded.
    let mut scenes = loaded.scenes.clone();
    // A6e: with no symbol, every scene's stage is cleaned, not only the active one.
    if symbol.is_none() {
        for (i, (_, other)) in scenes.iter_mut().enumerate() {
            if i != active {
                let _ = buzz_app::rigging::remove_background_specks(other, &scope, &options);
            }
        }
    }
    scenes[active].1 = {
        let mut s = after.clone();
        s.camera_mut().enabled = loaded.scenes[active].1.camera().enabled;
        s
    };
    let records: Vec<buzz_doc::format::SceneRecord> = scenes
        .iter()
        .enumerate()
        .map(|(i, (n, s))| buzz_doc::format::SceneRecord { name: n, scene: s, key: loaded.keys[i], role: loaded.roles[i].clone() })
        .collect();
    buzz_doc::format::save_scenes_full(&records, active, &loaded.sequence, loaded.master_audio, loaded.master_audio_mode, &saved)
        .expect("save");

    let (before_r, after_r, region) = match symbol {
        Some(sym) => {
            let (b, bounds) = alone(&scene, sym);
            let (af, _) = alone(&after, sym);
            (b, af, bounds)
        }
        None => (scene.clone(), after.clone(), scene.content_bounds()),
    };
    let region = region.unwrap_or(Rect::new(0.0, 0.0, 100.0, 100.0)).inflate(16.0, 16.0);
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let mut pngs = Vec::new();
    for (label, sc) in [("before", &before_r), ("after", &after_r)] {
        for (tag, s) in [("1x".to_string(), 1.0), (format!("{scale}x"), scale)] {
            let img = render(&mut ex, sc, region, s);
            let p = format!("{out}/{name}_{label}_{tag}_alpha.png");
            img.save(&p).unwrap();
            pngs.push(p);
            let p = format!("{out}/{name}_{label}_{tag}.png");
            on_magenta(&img).save(&p).unwrap();
            pngs.push(p);
        }
    }
    println!(
        "{}",
        json!({
            "file": path,
            "saved": saved.to_string_lossy(),
            "scope_symbol": symbol.map(|_| wanted.clone()),
            "shapes_before": count_shapes(&scene),
            "shapes_after": count_shapes(&after),
            "shapes_removed": report.shapes_removed,
            "shapes_clipped": report.shapes_clipped,
            "area": report.area,
            "keyframes": report.keyframes,
            "keyframes_not_judged": report.keyframes_not_judged,
            "colour": report.colour.map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])),
            "ms": ms.round(),
            "region": [region.x0, region.y0, region.x1, region.y1],
            "scale": scale,
            "pngs": pngs,
        })
    );
}
