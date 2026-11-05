//! hair_flow_gate: render and measure the Hair Flow modifier.
//!
//!   hair_flow_gate --out DIR                                  a made-up head with hair locks, the head swaying
//!   hair_flow_gate --out DIR --doc a.buzz --layers "Head"     the named layers of a .buzz (an in-memory copy)
//!
//! Writes DIR/strip.png (48 frames, 12 x 4), DIR/flow.mp4 (needs ffmpeg), DIR/frames/f_000.png.. and prints a JSON
//! line with: root drift (px, over the root band of the flowed outline, all frames), ms per part-frame for
//! `modified_object_at`, and the whole-frame render time with and without the modifier.
use anyhow::{Context, Result, bail};
use buzz_doc::Document;
use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{BezPath, Point, Rect};
use buzz_render::GpuPreference;
use buzz_scene::{LayerId, LayerKind, Modifier, Object, ObjectId, ObjectKind, Scene, ShapeData, StrokeSpec, Tween};
use kurbo::{Affine, Shape as _};
use peniko::Color;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

const FRAMES: u32 = 48;

/// One tapered lock: root `width` wide at (`x`, `top`), `len` long, bowed by `bow`, ending in a point.
fn lock(x: f64, top: f64, len: f64, width: f64, bow: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((x - width * 0.5, top));
    p.curve_to((x - width * 0.6 + bow * 0.3, top + len * 0.35), (x - width * 0.3 + bow, top + len * 0.7), (x + bow * 1.2, top + len));
    p.curve_to((x + width * 0.3 + bow, top + len * 0.7), (x + width * 0.6 + bow * 0.3, top + len * 0.35), (x + width * 0.5, top));
    p.close_path();
    p
}

fn synthetic() -> (Scene, Vec<LayerId>) {
    let mut scene = Scene::default();
    scene.stage_mut().size = kurbo::Size::new(540.0, 540.0);
    scene.stage_mut().background = Color::from_rgb8(0xE6, 0xDF, 0xD2);
    let hair_layer = scene.add_layer("Hair Back", LayerKind::Normal);
    let face_layer = scene.add_layer("Head", LayerKind::Normal);
    let mut locks: Vec<Arc<Object>> = Vec::new();
    let tones = [(0x2B, 0x17, 0x0E), (0x3A, 0x1F, 0x12), (0x24, 0x13, 0x0B)];
    for i in 0..11 {
        let t = i as f64 / 10.0;
        let x = 175.0 + 190.0 * t;
        let len = 300.0 + 90.0 * ((t * 7.0).sin() * 0.5 + 0.5) + 40.0 * (1.0 - (t - 0.5).abs() * 2.0);
        let bow = (t - 0.5) * 50.0;
        let tone = tones[i % 3];
        let mut shape = ShapeData::filled(lock(x, 80.0, len, 46.0, bow), Color::from_rgb8(tone.0, tone.1, tone.2));
        shape.stroke = Some(StrokeSpec::new(Color::from_rgb8(0x12, 0x08, 0x05), 2.0));
        let id = scene.next_object_id();
        locks.push(Arc::new(Object::shape(id, shape)));
    }
    let id = scene.next_object_id();
    let hair = scene.add_object(hair_layer, Object::group(id, locks)).unwrap();
    // The face and a scalp cap over the roots.
    let face = kurbo::Ellipse::new((270.0, 175.0), (88.0, 105.0), 0.0).to_path(0.1);
    let id = scene.next_object_id();
    let face_id = scene
        .add_object(face_layer, Object::shape(id, ShapeData::filled(face, Color::from_rgb8(0xE9, 0xC4, 0xA0))))
        .unwrap();
    let cap = kurbo::Ellipse::new((270.0, 100.0), (118.0, 46.0), 0.0).to_path(0.1);
    let id = scene.next_object_id();
    scene.add_object(face_layer, Object::shape(id, ShapeData::filled(cap, Color::from_rgb8(0x2B, 0x17, 0x0E))));
    for layer in [hair_layer, face_layer] {
        scene.update_layer(layer, |l| {
            if l.frames.length() < FRAMES {
                l.frames.insert_frame(FRAMES - 1);
            }
        });
    }
    // The head sways side to side, the hair layer with it: the tips should trail.
    for (layer, id) in [(hair_layer, hair), (face_layer, face_id)] {
        for (frame, dx) in [(0u32, 0.0), (12, 40.0), (24, 0.0), (36, -40.0), (47, 0.0)] {
            scene.ensure_keyframe(layer, frame);
            scene.update_object_at(frame, id, |o| o.transform = Affine::translate((dx, 0.0)));
            if frame != 47 {
                scene.update_layer(layer, |l| {
                    l.frames.set_tween(frame, Tween::motion());
                });
            }
        }
    }
    (scene, vec![hair_layer])
}

fn flatten(obj: &Object, acc: Affine, out: &mut Vec<Vec<Point>>) {
    match &obj.kind {
        ObjectKind::Shape(s) => {
            let mut poly = Vec::new();
            kurbo::flatten((acc * obj.transform * s.path.clone()).iter(), 0.05, |el| match el {
                kurbo::PathEl::MoveTo(p) => {
                    if poly.len() > 1 {
                        out.push(std::mem::take(&mut poly));
                    }
                    poly = vec![p];
                }
                kurbo::PathEl::LineTo(p) => poly.push(p),
                kurbo::PathEl::ClosePath => {
                    if let Some(first) = poly.first().copied() {
                        poly.push(first);
                    }
                }
                _ => {}
            });
            if poly.len() > 1 {
                out.push(poly);
            }
        }
        ObjectKind::Group(children) => {
            for c in children {
                flatten(c, acc * obj.transform, out);
            }
        }
        _ => {}
    }
}

fn dist_to_polylines(p: Point, polys: &[Vec<Point>]) -> f64 {
    let mut best = f64::INFINITY;
    for poly in polys {
        for w in poly.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let l2 = ab.hypot2();
            let t = if l2 < 1e-12 { 0.0 } else { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) };
            best = best.min((p - (a + ab * t)).hypot());
        }
    }
    best
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut out, mut doc_path, mut layer_names, mut scale) = (None, None, None::<String>, 1.0f64);
    let mut region_arg: Option<Rect> = None;
    let mut keep = false;
    while let Some(flag) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{what} needs a value"));
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--doc" => doc_path = Some(PathBuf::from(value("--doc")?)),
            "--layers" => layer_names = Some(value("--layers")?),
            "--keep" => keep = true, // use the Hair Flow already saved on the layers
            "--region" => {
                // x0,y0,x1,y1 in stage units (layer parenting means the layers' own bounds are not stage space).
                let v: Vec<f64> = value("--region")?.split(',').map(|n| n.trim().parse()).collect::<Result<_, _>>()?;
                if v.len() != 4 {
                    bail!("--region x0,y0,x1,y1");
                }
                region_arg = Some(Rect::new(v[0], v[1], v[2], v[3]));
            }
            "--scale" => scale = value("--scale")?.parse()?,
            other => bail!("unknown flag {other}"),
        }
    }
    let out = out.context("--out DIR")?;
    std::fs::create_dir_all(out.join("frames"))?;

    // The demonstration setting: stronger and faster than the eerie preset, so two seconds show it.
    let modifier = Modifier::HairFlow { strength: 0.10, rate: 0.5, wavelength: 0.5, wind: -30.0, lag: 0.25, root: 0.0 };

    let (mut scene, hair_layers, region) = match &doc_path {
        None => {
            let (scene, layers) = synthetic();
            (scene, layers, None)
        }
        Some(path) => {
            let doc = Document::open(path)?;
            let scene = doc.scene().clone();
            let wanted: Vec<String> = layer_names
                .clone()
                .context("--layers with --doc")?
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .collect();
            let layers: Vec<LayerId> = scene
                .layers()
                .iter()
                .filter(|l| wanted.contains(&l.name.to_lowercase()))
                .map(|l| l.id)
                .collect();
            if layers.is_empty() {
                bail!("no such layers");
            }
            // A Cast asset is one frame long: hold every layer for the length of the strip.
            let mut scene = scene;
            let all: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
            for layer in all {
                scene.update_layer(layer, |l| {
                    if l.frames.length() < FRAMES {
                        l.frames.insert_frame(FRAMES - 1);
                    }
                });
            }
            (scene, layers, Some(()))
        }
    };

    // Which objects, and (for a document) the region to frame.
    let mut ids: Vec<ObjectId> = Vec::new();
    let mut union: Option<Rect> = None;
    for layer in scene.layers().iter().filter(|l| hair_layers.contains(&l.id)) {
        for object in layer.frames.resolved_at(0u32).iter() {
            ids.push(object.id);
            let b = buzz_scene::object::transform_rect(object.transform, scene.resolved_bounds(object));
            union = Some(union.map_or(b, |u| u.union(b)));
        }
    }
    let rest_objects: Vec<Object> = scene
        .layers()
        .iter()
        .filter(|l| hair_layers.contains(&l.id))
        .flat_map(|l| l.frames.resolved_at(0u32).iter().cloned().collect::<Vec<_>>())
        .collect();

    // Whole-frame render time WITHOUT the modifier (for the delta).
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    let mut settings = ExportSettings::for_stage(&scene);
    let mut stage_region = None;
    if region.is_some() {
        let b = match region_arg {
            Some(r) => r,
            None => union.context("the layers are empty")?.inflate(40.0, 40.0),
        };
        // Square-ish frame around the layers.
        let side = b.width().max(b.height());
        let c = b.center();
        let r = Rect::new(c.x - side * 0.5, c.y - side * 0.5, c.x + side * 0.5, c.y + side * 0.5);
        stage_region = Some(r);
        settings.width = (720.0 * scale) as u32;
        settings.height = (720.0 * scale) as u32;
        settings.region = Some(r);
    } else {
        settings.width = (540.0 * scale) as u32;
        settings.height = (540.0 * scale) as u32;
    }
    exporter.render(&scene, 0, &settings)?; // warm up
    let t0 = Instant::now();
    for f in 0..FRAMES {
        exporter.render(&scene, f, &settings)?;
    }
    let without_ms = t0.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;

    // Now add the modifier.
    for id in ids.iter().filter(|_| !keep) {
        scene.update_object_across(0, u32::MAX, *id, |o| {
            o.modifiers.retain(|m| !matches!(m, Modifier::HairFlow { .. }));
            o.modifiers.push(modifier);
        });
    }

    // ms per part-frame: only the modifier evaluation, objects pre-resolved.
    let mut resolved: Vec<(LayerId, Vec<Object>)> = Vec::new();
    for f in 0..FRAMES {
        for layer in scene.layers().iter().filter(|l| hair_layers.contains(&l.id)) {
            resolved.push((layer.id, layer.frames.resolved_at(f).iter().cloned().collect()));
        }
    }
    let reps = 20;
    let mut samples = Vec::new();
    for r in 0..reps {
        for (i, (layer, objs)) in resolved.iter().enumerate() {
            let frame = (i / hair_layers.len()) as u32;
            for o in objs {
                let t = Instant::now();
                let ev = scene.modified_object_at(*layer, o, frame as f64 + r as f64 * 0.01);
                let dt = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(ev);
                if r > 0 {
                    samples.push(dt);
                }
            }
        }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    let p95 = samples[(samples.len() as f64 * 0.95) as usize];
    let max = *samples.last().unwrap();

    // Root drift: flowed outline points in the root band, distance to the rest outline.
    let mut rest_polys = Vec::new();
    for o in &rest_objects {
        flatten(o, Affine::IDENTITY, &mut rest_polys);
    }
    let top = rest_polys.iter().flatten().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let bottom = rest_polys.iter().flatten().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    let band = (bottom - top) * 0.01;
    let mut drift = 0.0f64;
    let mut tip_move = 0.0f64;
    for f in 0..FRAMES {
        // Compare with the rest outline of the same frame's placement (the head moves rigidly), so look at the
        // object's own local space: evaluate the modified object and flatten without its placement.
        for layer in scene.layers().iter().filter(|l| hair_layers.contains(&l.id)) {
            let objs: Vec<Object> = layer.frames.resolved_at(f).iter().cloned().collect();
            for o in &objs {
                let ev = scene.modified_object_at(layer.id, o, f).unwrap();
                let flowed = ev.object.unwrap_or_else(|| o.clone());
                let mut rest_local = Vec::new();
                let mut stripped = o.clone();
                stripped.modifiers.clear();
                stripped.transform = Affine::IDENTITY;
                flatten(&stripped, Affine::IDENTITY, &mut rest_local);
                let mut now_local = Vec::new();
                let mut flowed_stripped = flowed.clone();
                flowed_stripped.transform = Affine::IDENTITY;
                flatten(&flowed_stripped, Affine::IDENTITY, &mut now_local);
                let top_l = rest_local.iter().flatten().map(|p| p.y).fold(f64::INFINITY, f64::min);
                let bot_l = rest_local.iter().flatten().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
                let band_l = (bot_l - top_l) * 0.01;
                for p in now_local.iter().flatten().filter(|p| p.y <= top_l + band_l) {
                    let dd = dist_to_polylines(*p, &rest_local);
                    if dd > drift && std::env::var_os("HF_DEBUG").is_some() {
                        eprintln!("frame {f} point {p:?} dist {dd} top {top_l} band {band_l}");
                    }
                    drift = drift.max(dd);
                }
                for p in now_local.iter().flatten().filter(|p| p.y >= bot_l - band_l * 5.0) {
                    tip_move = tip_move.max(dist_to_polylines(*p, &rest_local));
                }
            }
        }
    }
    let _ = (top, band);

    // Whole-frame render WITH the modifier, plus the frames themselves.
    exporter.render(&scene, 0, &settings)?;
    let t0 = Instant::now();
    let mut frames = Vec::new();
    for f in 0..FRAMES {
        frames.push(exporter.render(&scene, f, &settings)?);
    }
    let with_ms = t0.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
    for (i, fr) in frames.iter().enumerate() {
        fr.write_png(&out.join("frames").join(format!("f_{i:03}.png")))?;
    }

    // The strip: 12 columns x 4 rows, each tile half size.
    let (tw, th) = (frames[0].width / 2, frames[0].height / 2);
    let mut sheet = image::RgbaImage::new(tw * 12, th * 4);
    for (i, fr) in frames.iter().enumerate() {
        let img = image::RgbaImage::from_raw(fr.width, fr.height, fr.pixels.clone()).unwrap();
        let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
        image::imageops::replace(&mut sheet, &small, (tw * (i as u32 % 12)) as i64, (th * (i as u32 / 12)) as i64);
    }
    sheet.save(out.join("strip.png"))?;

    // The film.
    let mp4 = out.join("flow.mp4");
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-stream_loop", "2", "-framerate", "24", "-i"])
        .arg(out.join("frames").join("f_%03d.png"))
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "16"])
        .arg(&mp4)
        .status()
        .context("ffmpeg")?;
    println!(
        "{}",
        serde_json::json!({
            "objects": ids.len(),
            "root_drift_px": drift,
            "root_band_pct_of_length": 1.0,
            "tip_move_px": tip_move,
            "modified_object_at_ms_mean": mean,
            "modified_object_at_ms_p95": p95,
            "modified_object_at_ms_max": max,
            "frame_render_ms_without": without_ms,
            "frame_render_ms_with": with_ms,
            "strip": out.join("strip.png"),
            "mp4": mp4,
            "ffmpeg_ok": status.success(),
            "region": stage_region.map(|r| format!("{r:?}")),
        })
    );
    Ok(())
}
