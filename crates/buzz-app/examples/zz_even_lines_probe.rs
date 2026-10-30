//! scratch probe (EVEN_LINES_PLAN E2) — Even Out Lines headlessly on a .buzz
//! with the dialog's defaults; prints JSON and writes before/after renders.
//! Usage: zz_even_lines_probe <file.buzz> <out dir> [symbol name] [hand x0,y0,x1,y1] [name=x0,y0,x1,y1 ...]
//! E1d: `silhouette_lost` / `paint_outside` from 1x renders, and on a 400 %
//! crop of the hand (stage units; "-" for the symbol name keeps the default).
//! With a symbol (default: the file's stem, e.g. "Guddu", when the library has
//! it), the scope is that symbol (as if its instance were selected) and it is
//! rendered alone on the stage; otherwise the whole stage.
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{Keyframe, Layer, LayerId, LayerKind, LayerTimeline, Object, ObjectKind, Scene};
use std::sync::Arc;
use image::RgbaImage;
use serde_json::json;
use std::time::Instant;

fn load(path: &str) -> Scene {
    let (scenes, active) = buzz_doc::format::load_scenes(path).expect("load");
    scenes.into_iter().nth(active).unwrap().1
}

fn stage_bounds(scene: &Scene) -> Rect {
    let mut out: Option<Rect> = None;
    for layer in scene.stage_layers().iter() {
        for o in layer.objects_at(0) {
            let b = match &o.kind {
                ObjectKind::Instance(_) => scene.instance_bounds(o),
                ObjectKind::Shape(s) => Some((o.transform * s.path.clone()).bounding_box()),
                _ => None,
            };
            if let Some(b) = b {
                out = Some(out.map_or(b, |r| r.union(b)));
            }
        }
    }
    out.unwrap_or(Rect::new(0.0, 0.0, 100.0, 100.0))
}

fn render(ex: &mut buzz_export::Exporter, scene: &Scene, region: Rect, scale: f64) -> RgbaImage {
    let s = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent: std::env::var_os("BUZZ_PROBE_TRANSPARENT").is_some(),
        region: Some(region),
    };
    let f = ex.render(scene, 0, &s).unwrap();
    RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
}

/// The filled, unstroked shape area on "Ink" layers of `symbols` and the
/// symbols inside them (every keyframe).
fn ink_fill_area(scene: &Scene, symbols: &[buzz_scene::SymbolId]) -> f64 {
    let mut seen: Vec<buzz_scene::SymbolId> = Vec::new();
    let mut todo: Vec<buzz_scene::SymbolId> = symbols.to_vec();
    let mut area = 0.0;
    while let Some(id) = todo.pop() {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let Some(s) = scene.library().get(id) else { continue };
        for layer in s.layers.iter() {
            let ink = layer.name.trim().eq_ignore_ascii_case("ink");
            for kf in layer.frames.keyframes() {
                for o in kf.objects.iter() {
                    todo.extend(buzz_app::rigging::nested_symbols(o));
                    if let ObjectKind::Shape(sd) = &o.kind
                        && ink
                        && sd.fill.is_some()
                        && sd.stroke.is_none()
                    {
                        area += (o.transform * sd.path.clone()).area().abs();
                    }
                }
            }
        }
    }
    area
}

fn pair(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(a.width() + b.width(), a.height().max(b.height()), image::Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut out, a, 0, 0);
    image::imageops::overlay(&mut out, b, a.width() as i64, 0);
    out
}

/// E1d: pixels that stop being background-free (`lost`: not background
/// before, background after) or start being painted (`outside`), with a
/// min-channel change over 64. Background = min channel >= 245. Also a marked
/// picture (lost red, outside blue over the after render) and up to 12 stage
/// positions of each.
fn silhouette_change(b: &RgbaImage, a: &RgbaImage, region: Rect, scale: f64) -> (usize, usize, RgbaImage, Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let minc = |p: &image::Rgba<u8>| p[0].min(p[1]).min(p[2]) as i32;
    let mut marked = a.clone();
    let (mut lost, mut outside) = (0, 0);
    let (mut lost_at, mut outside_at) = (Vec::new(), Vec::new());
    for y in 0..b.height().min(a.height()) {
        for x in 0..b.width().min(a.width()) {
            let (mb, ma) = (minc(b.get_pixel(x, y)), minc(a.get_pixel(x, y)));
            if (ma - mb).abs() <= 64 {
                continue;
            }
            let at = [region.x0 + (x as f64 + 0.5) / scale, region.y0 + (y as f64 + 0.5) / scale];
            if mb < 245 && ma >= 245 {
                lost += 1;
                if lost_at.len() < 12 {
                    lost_at.push(at);
                }
                marked.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            } else if mb >= 245 && ma < 245 {
                outside += 1;
                if outside_at.len() < 12 {
                    outside_at.push(at);
                }
                marked.put_pixel(x, y, image::Rgba([0, 0, 255, 255]));
            }
        }
    }
    (lost, outside, marked, lost_at, outside_at)
}

fn render_alpha(ex: &mut buzz_export::Exporter, scene: &Scene, region: Rect, scale: f64) -> RgbaImage {
    let s = buzz_export::ExportSettings {
        width: (region.width() * scale).round().max(1.0) as u32,
        height: (region.height() * scale).round().max(1.0) as u32,
        transparent: true,
        region: Some(region),
    };
    let f = ex.render(scene, 0, &s).unwrap();
    RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
}

/// E1e: out of the character by alpha (transparent renders): `lost` = alpha
/// > 0.5 before and < 0.5 after, `outside` = the reverse. Marked over the
/// after render on white (lost red, outside blue), and up to 12 stage
/// positions of each.
fn alpha_change(b: &RgbaImage, a: &RgbaImage, region: Rect, scale: f64) -> (usize, usize, RgbaImage, Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let mut marked = RgbaImage::new(a.width(), a.height());
    let (mut lost, mut outside) = (0, 0);
    let (mut lost_at, mut outside_at) = (Vec::new(), Vec::new());
    for y in 0..a.height() {
        for x in 0..a.width() {
            let pa = a.get_pixel(x, y);
            let al = pa[3] as f64 / 255.0;
            let over = |c: u8| (c as f64 + 255.0 * (1.0 - al)).round().min(255.0) as u8;
            marked.put_pixel(x, y, image::Rgba([over(pa[0]), over(pa[1]), over(pa[2]), 255]));
            if x >= b.width() || y >= b.height() {
                continue;
            }
            let (ab, aa) = (b.get_pixel(x, y)[3], pa[3]);
            let at = [region.x0 + (x as f64 + 0.5) / scale, region.y0 + (y as f64 + 0.5) / scale];
            if ab > 127 && aa <= 127 {
                lost += 1;
                if lost_at.len() < 12 {
                    lost_at.push(at);
                }
                marked.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            } else if ab <= 127 && aa > 127 {
                outside += 1;
                if outside_at.len() < 12 {
                    outside_at.push(at);
                }
                marked.put_pixel(x, y, image::Rgba([0, 0, 255, 255]));
            }
        }
    }
    (lost, outside, marked, lost_at, outside_at)
}

fn main() {
    unsafe { std::env::set_var("BUZZ_NO_SYMBOL_CACHE", "1") };
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (path, out) = (&a[0], &a[1]);
    std::fs::create_dir_all(out).unwrap();
    let name = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().to_lowercase();
    let mut scene = load(path);
    scene.camera_mut().enabled = false;
    let wanted = a.get(2).cloned().filter(|w| w != "-").unwrap_or_else(|| {
        std::path::Path::new(path).file_stem().unwrap().to_string_lossy().to_string()
    });
    let symbol = scene.library().iter().find(|s| s.name == wanted).map(|s| s.id);
    let mut alone_bounds = None;
    if let Some(sym) = symbol {
        // The symbol alone on the stage, at identity.
        let o = Object::instance_of(scene.next_object_id(), sym);
        alone_bounds = scene.instance_bounds(&o);
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
    }

    let t0 = Instant::now();
    let scope = match symbol {
        Some(sym) => buzz_app::rigging::EvenLinesScope { symbols: vec![sym], ..Default::default() },
        None => buzz_app::rigging::EvenLinesScope { whole_timeline: true, ..Default::default() },
    };
    let plan = buzz_app::rigging::plan_even_lines(&scene, &scope);
    let t_plan = t0.elapsed().as_secs_f64() * 1000.0;
    let quarter = |v: f64, lo: f64, hi: f64| ((v * 4.0).round() / 4.0).clamp(lo, hi);
    let settings = buzz_app::rigging::EvenLinesSettings {
        face_width: quarter(plan.face_measured.or(plan.ink_measured).unwrap_or(1.5), 0.25, 12.0),
        body_width: quarter(plan.body_measured.or(plan.ink_measured).unwrap_or(2.0), 0.25, 12.0),
        keep_weight: true,
        straighten: 0.0,
        solid_above: quarter(buzz_app::rigging::default_solid_above(&plan), 0.5, 40.0),
    };
    let t1 = Instant::now();
    let mut trace = buzz_app::rigging::trace_even_lines(&plan, settings.straighten, settings.solid_above, false);
    let t_trace = t1.elapsed().as_secs_f64() * 1000.0;
    let t2 = Instant::now();
    let draft = buzz_app::rigging::trace_even_lines(&plan, settings.straighten, settings.solid_above, true);
    let t_draft = t2.elapsed().as_secs_f64() * 1000.0;
    // A Solid-above slider step while dragging: only sets it can change.
    let t2b = Instant::now();
    let step = buzz_app::rigging::retrace_even_lines(&plan, draft, settings.straighten, settings.solid_above + 0.25, true);
    let t_draft_step = t2b.elapsed().as_secs_f64() * 1000.0;
    drop(step);
    let mut after = scene.clone();
    let t3 = Instant::now();
    let report = buzz_app::rigging::apply_even_lines(&mut after, &plan, &mut trace, &settings);
    let t_apply = t3.elapsed().as_secs_f64() * 1000.0;

    let region = alone_bounds.or_else(|| scene.content_bounds()).unwrap_or_else(|| stage_bounds(&scene)).inflate(10.0, 10.0);
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let fit = (1600.0 / region.width().max(region.height())).min(4.0);
    let mut pngs = Vec::new();
    let mut whole = Vec::new();
    for (label, sc) in [("before", &scene), ("after", &after)] {
        let img = render(&mut ex, sc, region, fit);
        let p = format!("{out}/{name}_{label}.png");
        img.save(&p).unwrap();
        pngs.push(p);
        whole.push(img);
    }
    let p = format!("{out}/{name}_pair.png");
    pair(&whole[0], &whole[1]).save(&p).unwrap();
    pngs.push(p);
    // The face: a square at the top middle of the character.
    let side = region.height() * 0.2;
    let cx = region.center().x;
    let face = Rect::new(cx - side / 2.0, region.y0 + 10.0, cx + side / 2.0, region.y0 + 10.0 + side);
    let mut faces = Vec::new();
    for (label, sc) in [("before", &scene), ("after", &after)] {
        let img = render(&mut ex, sc, face, 4.0);
        let p = format!("{out}/{name}_face400_{label}.png");
        img.save(&p).unwrap();
        pngs.push(p);
        faces.push(img);
    }
    let p = format!("{out}/face_pair.png");
    pair(&faces[0], &faces[1]).save(&p).unwrap();
    pngs.push(p);
    // The mouth: the lower middle of the face square, at 400 %.
    let mouth = Rect::new(
        face.x0 + side * 0.25,
        face.y0 + side * 0.62,
        face.x0 + side * 0.72,
        face.y0 + side * 0.92,
    );
    let mut mouths = Vec::new();
    for (label, sc) in [("before", &scene), ("after", &after)] {
        let img = render(&mut ex, sc, mouth, 4.0);
        let p = format!("{out}/{name}_mouth400_{label}.png");
        img.save(&p).unwrap();
        pngs.push(p);
        mouths.push(img);
    }
    let p = format!("{out}/{name}_mouth_pair.png");
    pair(&mouths[0], &mouths[1]).save(&p).unwrap();
    pngs.push(p);
    // E1d: the silhouette, from 1x renders and a 400 % crop of the hand.
    let mut ones = Vec::new();
    for (label, sc) in [("before", &scene), ("after", &after)] {
        let img = render(&mut ex, sc, region, 1.0);
        img.save(format!("{out}/{name}_1x_{label}.png")).unwrap();
        ones.push(img);
    }
    let (lost, outside, marked, lost_at, outside_at) = silhouette_change(&ones[0], &ones[1], region, 1.0);
    let p = format!("{out}/{name}_silhouette_1x.png");
    marked.save(&p).unwrap();
    pngs.push(p);
    let hand = a.get(3).and_then(|h| {
        let v: Vec<f64> = h.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        (v.len() == 4).then(|| Rect::new(v[0], v[1], v[2], v[3]))
    });
    let mut hand_json = serde_json::Value::Null;
    if let Some(hand) = hand {
        let mut hands = Vec::new();
        for (label, sc) in [("before", &scene), ("after", &after)] {
            let img = render(&mut ex, sc, hand, 4.0);
            let p = format!("{out}/{name}_hand400_{label}.png");
            img.save(&p).unwrap();
            pngs.push(p);
            hands.push(img);
        }
        let (hl, ho, hm, hl_at, ho_at) = silhouette_change(&hands[0], &hands[1], hand, 4.0);
        let p = format!("{out}/{name}_hand400_marked.png");
        hm.save(&p).unwrap();
        pngs.push(p);
        hand_json = json!({"rect": [hand.x0, hand.y0, hand.x1, hand.y1], "hand400_lost": hl, "hand400_outside": ho, "lost_at": hl_at, "outside_at": ho_at});
    }
    // E1e: by alpha at 1x and on 400 % crops: the face square and any
    // `name=x0,y0,x1,y1` given after the symbol name (stage units).
    let (al, ao, am, al_at, ao_at) = {
        let b = render_alpha(&mut ex, &scene, region, 1.0);
        let a = render_alpha(&mut ex, &after, region, 1.0);
        alpha_change(&b, &a, region, 1.0)
    };
    let p = format!("{out}/{name}_alpha_1x_marked.png");
    am.save(&p).unwrap();
    pngs.push(p);
    let mut crops: Vec<(String, Rect)> = vec![("face".to_string(), face)];
    for arg in a.iter().skip(3) {
        let Some((label, r)) = arg.split_once('=') else { continue };
        let v: Vec<f64> = r.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        if v.len() == 4 {
            crops.push((label.to_string(), Rect::new(v[0], v[1], v[2], v[3])));
        }
    }
    let mut crops_json = serde_json::Map::new();
    for (label, r) in &crops {
        let b = render_alpha(&mut ex, &scene, *r, 4.0);
        let a = render_alpha(&mut ex, &after, *r, 4.0);
        b.save(format!("{out}/{name}_{label}400_alpha_before.png")).unwrap();
        a.save(format!("{out}/{name}_{label}400_alpha_after.png")).unwrap();
        let (l, o, m, l_at, o_at) = alpha_change(&b, &a, *r, 4.0);
        let p = format!("{out}/{name}_{label}400_alpha_marked.png");
        m.save(&p).unwrap();
        pngs.push(p);
        crops_json.insert(label.clone(), json!({"rect": [r.x0, r.y0, r.x1, r.y1], "lost": l, "outside": o, "lost_at": l_at, "outside_at": o_at}));
    }
    let alpha_json = json!({"lost_1x": al, "outside_1x": ao, "lost_at": al_at, "outside_at": ao_at, "crops": crops_json});
    let roots: Vec<buzz_scene::SymbolId> = symbol.into_iter().collect();
    let ink_before = ink_fill_area(&scene, &roots);
    let ink_after = ink_fill_area(&after, &roots);
    let stroke_fraction = if ink_before > 0.0 { (ink_before - ink_after) / ink_before } else { 0.0 };
    println!(
        "{}",
        json!({
            "file": path,
            "scope_symbol": symbol.map(|_| wanted.clone()),
            "ink_shapes": plan.ink_shapes,
            "face_lines": report.face_lines,
            "body_lines": report.body_lines,
            "solids": report.solids,
            "kept_fill_shapes": plan.kept_fills,
            "ink_area_replaced_by_strokes": stroke_fraction,
            "face_median_px": plan.face_measured,
            "body_median_px": plan.body_measured,
            "settings": {"face_width": settings.face_width, "body_width": settings.body_width, "solid_above": settings.solid_above},
            "ms": {"plan": t_plan.round(), "trace": t_trace.round(), "apply": t_apply.round(), "total": (t_plan + t_trace + t_apply).round(), "draft_trace": t_draft.round(), "draft_solid_step": t_draft_step.round()},
            "silhouette_lost": lost,
            "paint_outside": outside,
            "silhouette_lost_at": lost_at,
            "paint_outside_at": outside_at,
            "hand": hand_json,
            "alpha": alpha_json,
            "kept_as_drawn": report.kept_as_drawn,
            "fit_scale": fit,
            "region": [region.x0, region.y0, region.x1, region.y1],
            "pngs": pngs,
        })
    );
}
