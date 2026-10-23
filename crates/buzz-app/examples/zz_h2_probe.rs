//! scratch probe (HEAD_RIG_PLAN H2) — delete after use.
use buzz_geom::{Affine, Rect};
use buzz_scene::{Keyframe, Layer, LayerId, LayerKind, LayerTimeline, LoopMode, Object, ObjectKind, Scene, ShapeData, SymbolId, SymbolKind};
use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::sync::Arc;

fn load(path: &str) -> Scene {
    if path.ends_with(".buzz") {
        let (scenes, active) = buzz_doc::format::load_scenes(path).expect("load");
        scenes.into_iter().nth(active).unwrap().1
    } else if path.ends_with(".xfl") {
        buzz_import_xfl::import_xfl_folder(std::path::Path::new(path).parent().unwrap()).expect("xfl").0
    } else {
        buzz_app::import::read(std::path::Path::new(path)).expect("import").scene
    }
}

fn find(scene: &Scene, name: &str, folder_hint: &str) -> SymbolId {
    let c: Vec<_> = scene.library().iter().filter(|s| s.name == name).collect();
    c.iter()
        .find(|s| s.folder.as_deref().unwrap_or("").to_lowercase().contains(&folder_hint.to_lowercase()))
        .or(c.first())
        .map(|s| s.id)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

/// Each drawn layer's rest shapes (masks applied), in paint order.
fn flatten_layers(scene: &Scene, sym: SymbolId, keep: impl Fn(&Layer) -> bool) -> Vec<(String, ShapeData)> {
    let s = scene.library().get(sym).unwrap().clone();
    let mut out = Vec::new();
    for layer in s.layers.paint_order() {
        if matches!(layer.kind, LayerKind::Mask | LayerKind::InverseMask | LayerKind::Folder | LayerKind::Guide) || !keep(layer) {
            continue;
        }
        let mut sc = scene.clone();
        let only = layer.id;
        sc.library_mut().update(sym, |t| {
            let ids: Vec<LayerId> = t.layers.iter().map(|l| l.id).collect();
            for id in ids {
                let skip = t.layers.get(id).is_some_and(|l| matches!(l.kind, LayerKind::Mask | LayerKind::InverseMask | LayerKind::Folder));
                if id != only && !skip {
                    t.layers.update(id, |l| l.visible = false);
                }
            }
        });
        let o = Object::instance_of(sc.next_object_id(), sym);
        for sh in buzz_app::rigging::flatten_rest(&sc, &o, Affine::IDENTITY) {
            out.push((layer.name.clone(), sh));
        }
    }
    out
}

fn merged_symbol(scene: &mut Scene, name: &str, shapes: &[(String, ShapeData)]) -> SymbolId {
    let id = scene.add_symbol(name, SymbolKind::Graphic, None);
    let objs: Vec<Arc<Object>> = shapes.iter().map(|(_, s)| Arc::new(Object::shape(scene.next_object_id(), s.clone()))).collect();
    scene.library_mut().update(id, |s| {
        let first = s.layers.iter().next().map(|l| l.id).unwrap();
        s.layers.update(first, |l| {
            let mut k = Keyframe::new(0);
            k.objects = Arc::new(objs);
            l.frames = LayerTimeline::from_parts(vec![k], 1);
        });
    });
    id
}

/// The symbol alone on the stage at identity; returns its bounds.
fn stage_alone(scene: &mut Scene, sym: SymbolId) -> Rect {
    let o = Object::instance_of(scene.next_object_id(), sym);
    let b = scene.instance_bounds(&o).unwrap_or(Rect::new(-100.0, -100.0, 100.0, 100.0));
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
    b
}

struct R {
    ex: buzz_export::Exporter,
    region: Rect,
    scale: f64,
}

impl R {
    fn render(&mut self, scene: &Scene) -> RgbaImage {
        let s = buzz_export::ExportSettings {
            width: (self.region.width() * self.scale).round().max(1.0) as u32,
            height: (self.region.height() * self.scale).round().max(1.0) as u32,
            transparent: true,
            region: Some(self.region),
        };
        let f = self.ex.render(scene, 0, &s).unwrap();
        RgbaImage::from_raw(f.width, f.height, f.pixels).unwrap()
    }
}

fn white(p: &Rgba<u8>) -> [f64; 3] {
    let a = p[3] as f64 / 255.0;
    [0, 1, 2].map(|k| p[k] as f64 * a + 255.0 * (1.0 - a))
}

fn pix(img: &RgbaImage, i: usize) -> Rgba<u8> {
    let r = img.as_raw();
    Rgba([r[i * 4], r[i * 4 + 1], r[i * 4 + 2], r[i * 4 + 3]])
}

fn pdiff(a: &RgbaImage, b: &RgbaImage, i: usize) -> f64 {
    let (x, y) = (white(&pix(a, i)), white(&pix(b, i)));
    (0..3).map(|k| (x[k] - y[k]).abs()).fold(0.0, f64::max)
}

fn diff(a: &RgbaImage, b: &RgbaImage) -> Value {
    let (mut max, mut sum, mut over) = (0.0f64, 0.0, 0usize);
    let n = (a.width() * a.height()) as usize;
    for i in 0..n {
        let d = pdiff(a, b, i);
        max = max.max(d);
        sum += d;
        over += (d > 24.0) as usize;
    }
    json!({"max": max.round(), "mean": (sum / n as f64 * 1000.0).round() / 1000.0, "over24": over, "px": n})
}

fn mask_of(img: &RgbaImage, t: u8) -> Vec<bool> {
    img.pixels().map(|p| p[3] > t).collect()
}

fn dilate(m: &[bool], w: u32, h: u32, r: i64) -> Vec<bool> {
    let (w, h) = (w as i64, h as i64);
    let mut out = vec![false; m.len()];
    for y in 0..h {
        for x in 0..w {
            if !m[(y * w + x) as usize] {
                continue;
            }
            for dy in -r..=r {
                for dx in -r..=r {
                    let (u, v) = (x + dx, y + dy);
                    if u >= 0 && v >= 0 && u < w && v < h {
                        out[(v * w + u) as usize] = true;
                    }
                }
            }
        }
    }
    out
}

/// `m` with its enclosed holes filled (an outline-only mask as its region).
fn fill_holes(m: &[bool], w: u32, h: u32) -> Vec<bool> {
    let (w, h) = (w as usize, h as usize);
    let mut outside = vec![false; m.len()];
    let mut stack: Vec<usize> = (0..m.len()).filter(|i| { let (x, y) = (i % w, i / w); (x == 0 || y == 0 || x + 1 == w || y + 1 == h) && !m[*i] }).collect();
    while let Some(i) = stack.pop() {
        if outside[i] || m[i] { continue; }
        outside[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 { stack.push(i - 1) }
        if x + 1 < w { stack.push(i + 1) }
        if y > 0 { stack.push(i - w) }
        if y + 1 < h { stack.push(i + w) }
    }
    outside.iter().map(|o| !o).collect()
}

fn erode(m: &[bool], w: u32, h: u32, r: i64) -> Vec<bool> {
    let inv: Vec<bool> = m.iter().map(|x| !x).collect();
    dilate(&inv, w, h, r).iter().map(|x| !x).collect()
}

fn layer_names(scene: &Scene, sym: SymbolId) -> Vec<String> {
    scene
        .library()
        .get(sym)
        .unwrap()
        .layers
        .iter()
        .map(|l| match l.kind {
            LayerKind::Mask => format!("{} [mask]", l.name),
            LayerKind::Masked => format!("{} [masked]", l.name),
            _ => l.name.clone(),
        })
        .collect()
}

fn edit_layers(scene: &Scene, sym: SymbolId, pred: impl Fn(&Layer) -> bool, f: impl Fn(&mut Layer)) -> Scene {
    let mut sc = scene.clone();
    sc.library_mut().update(sym, |s| {
        let ids: Vec<LayerId> = s.layers.iter().filter(|l| pred(l)).map(|l| l.id).collect();
        for id in ids {
            s.layers.update(id, |l| f(l));
        }
    });
    sc
}

fn objects_edit(l: &mut Layer, f: impl Fn(&mut Object)) {
    if let Some(objs) = l.frames.objects_at_mut(0) {
        for o in objs.iter_mut() {
            let mut ob = (**o).clone();
            f(&mut ob);
            *o = Arc::new(ob);
        }
    }
}

fn alone(scene: &Scene, sym: SymbolId, pred: impl Fn(&Layer) -> bool) -> Scene {
    edit_layers(scene, sym, |_| true, |l| {
        if pred(l) {
            if matches!(l.kind, LayerKind::Mask | LayerKind::Masked) {
                l.kind = LayerKind::Normal;
            }
        } else if l.kind != LayerKind::Folder {
            l.visible = false;
        }
    })
}

fn has(n: &str, w: &[&str]) -> bool {
    let n = n.to_lowercase();
    w.iter().any(|x| n.contains(x))
}

fn crop(img: &RgbaImage, r: (i64, i64, i64, i64)) -> RgbaImage {
    let (x0, y0) = (r.0.clamp(0, img.width() as i64 - 1) as u32, r.1.clamp(0, img.height() as i64 - 1) as u32);
    let (x1, y1) = (r.2.clamp(1, img.width() as i64) as u32, r.3.clamp(1, img.height() as i64) as u32);
    image::imageops::crop_imm(img, x0, y0, x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1)).to_image()
}

fn on_white_img(img: &RgbaImage) -> RgbaImage {
    let mut o = img.clone();
    for p in o.pixels_mut() {
        let w = white(p);
        *p = Rgba([w[0] as u8, w[1] as u8, w[2] as u8, 255]);
    }
    o
}

fn sheet(rows: Vec<Vec<RgbaImage>>, path: &str, max_h: u32) {
    let rows: Vec<Vec<RgbaImage>> = rows
        .into_iter()
        .map(|r| {
            r.into_iter()
                .map(|i| {
                    let i = on_white_img(&i);
                    if i.height() > max_h {
                        let w = (i.width() as f64 * max_h as f64 / i.height() as f64).max(1.0) as u32;
                        image::imageops::resize(&i, w, max_h, image::imageops::FilterType::Triangle)
                    } else {
                        i
                    }
                })
                .collect()
        })
        .collect();
    let gap = 6;
    let w = rows.iter().map(|r| r.iter().map(|i| i.width() + gap).sum::<u32>()).max().unwrap_or(1);
    let h: u32 = rows.iter().map(|r| r.iter().map(|i| i.height()).max().unwrap_or(0) + gap).sum();
    let mut out = RgbaImage::from_pixel(w.max(1), h.max(1), Rgba([200, 200, 210, 255]));
    let mut y = 0;
    for r in &rows {
        let mut x = 0;
        for i in r {
            image::imageops::overlay(&mut out, i, x as i64, y as i64);
            x += i.width() + gap;
        }
        y += r.iter().map(|i| i.height()).max().unwrap_or(0) + gap;
    }
    out.save(path).unwrap();
}

fn is_lid(n: &str) -> bool {
    has(n, &["lid"])
}
fn is_mask(l: &Layer) -> bool {
    l.kind == LayerKind::Mask
}
fn is_ball(l: &Layer) -> bool {
    l.kind == LayerKind::Masked || has(&l.name, &["ball"])
}
fn is_white(n: &str) -> bool {
    has(n, &["white"])
}
fn is_brow(n: &str) -> bool {
    has(n, &["brow"])
}
fn is_mouth(n: &str) -> bool {
    has(n, &["mouth"])
}
fn skinish(c: [f64; 3]) -> bool {
    c[0] > c[2] + 15.0 && c[0] > 90.0 && c[0] >= c[1]
}

/// Rig `sym` (alone on the stage of `scene`) in a document, and check it.
fn check(r: &mut R, scene: Scene, sym: SymbolId, label: &str, out: &str) -> (Value, Scene) {
    let style = buzz_rig::head_style::HeadStyle::animate_convention();
    let mut doc = buzz_doc::Document::new(scene);
    let before_scene = doc.scene().clone();
    let before = r.render(&before_scene);
    let depth0 = doc.history().undo_depth();
    let t = std::time::Instant::now();
    let mut result = None;
    doc.edit("Rig Head", |sc| result = Some(buzz_app::rigging::rig_head(sc, sym, &style)));
    let ms = t.elapsed().as_millis();
    let report = match result.unwrap() {
        Ok(rep) => rep,
        Err(e) => {
            println!("{label}: ERR {e}");
            return (json!({"head": label, "error": e}), before_scene);
        }
    };
    let rigged = doc.scene().clone();
    let one_step = doc.history().undo_depth() == depth0 + 1;
    let after = r.render(&rigged);
    let rest = diff(&before, &after);
    let names = layer_names(&rigged, sym);
    println!("{label}: {ms} ms layers {names:?}\n  notes {:?}\n  rest {rest}", report.notes);
    let (w, h) = (after.width(), after.height());
    let n = (w * h) as usize;

    // Blink.
    let lid_frames: Vec<RgbaImage> = (0..7u32)
        .map(|f| {
            let sc = edit_layers(&rigged, sym, |l| is_lid(&l.name), |l| {
                objects_edit(l, |o| {
                    if let ObjectKind::Instance(i) = &mut o.kind {
                        i.first_frame = f;
                        i.loop_mode = LoopMode::SingleFrame;
                    }
                })
            });
            r.render(&sc)
        })
        .collect();
    let white_alone = r.render(&alone(&rigged, sym, |l| is_white(&l.name)));
    let mask_alone = r.render(&alone(&rigged, sym, is_mask));
    let wm = mask_of(&white_alone, 128);
    let mm = mask_of(&mask_alone, 128);
    let light = |p: Rgba<u8>| {
        let c = white(&p);
        let mx = c.iter().cloned().fold(0.0, f64::max);
        let mn = c.iter().cloned().fold(255.0, f64::min);
        mn > 180.0 && mx - mn < 30.0 && p[3] > 200
    };
    let whites: Vec<usize> = (0..n).filter(|i| wm[*i] && mm[*i] && light(pix(&after, *i))).collect();
    let cover: Vec<f64> = lid_frames
        .iter()
        .map(|img| {
            let left = whites.iter().filter(|i| light(pix(img, **i))).count();
            1.0 - left as f64 / whites.len().max(1) as f64
        })
        .collect();
    let blink_ok = !whites.is_empty() && cover[3] >= 0.99 && cover[0] <= 0.01;

    // Eyeball moves.
    let mut eye_moves = Vec::new();
    let mut eye_imgs = Vec::new();
    let mask_zone = dilate(&mm, w, h, 2);
    let mask_core = erode(&mm, w, h, 2);
    let mut eyeball_ok = mm.iter().any(|x| *x);
    for (dx, dy) in [(-10.0, 0.0), (10.0, 0.0), (0.0, -10.0)] {
        let sc = edit_layers(&rigged, sym, is_ball, |l| objects_edit(l, |o| o.transform = Affine::translate((dx, dy)) * o.transform));
        let img = r.render(&sc);
        let (mut outside, mut inside, mut holes) = (0, 0, 0);
        for i in 0..n {
            if pdiff(&after, &img, i) > 24.0 {
                if mask_zone[i] { inside += 1 } else { outside += 1 }
            }
            if mask_core[i] && pix(&img, i)[3] < 250 {
                holes += 1;
            }
        }
        eyeball_ok &= outside == 0 && holes == 0 && inside > 0;
        eye_moves.push(json!({"dx": dx, "dy": dy, "changed_inside": inside, "changed_outside_mask": outside, "holes": holes}));
        eye_imgs.push(img);
    }

    // Brows up 8.
    let brow_alone = r.render(&alone(&rigged, sym, |l| is_brow(&l.name)));
    let bm = mask_of(&brow_alone, 128);
    let brows_found = rigged.library().get(sym).unwrap().layers.iter().filter(|l| is_brow(&l.name)).count();
    let brow_scene = edit_layers(&rigged, sym, |l| is_brow(&l.name), |l| objects_edit(l, |o| o.transform = Affine::translate((0.0, -8.0)) * o.transform));
    let brow_img = r.render(&brow_scene);
    let bmm = mask_of(&r.render(&alone(&brow_scene, sym, |l| is_brow(&l.name))), 128);
    let (mut bholes, mut bshown, mut bskin) = (0, 0, 0);
    for i in 0..n {
        if !bm[i] || pix(&after, i)[3] < 250 {
            continue;
        }
        if pix(&brow_img, i)[3] < 250 {
            bholes += 1;
        }
        if !bmm[i] {
            bshown += 1;
            if skinish(white(&pix(&brow_img, i))) {
                bskin += 1;
            }
        }
    }
    let brow_move_ok = brows_found > 0 && bholes == 0;

    // Mouth blank.
    let mouth_found = rigged.library().get(sym).unwrap().layers.iter().any(|l| is_mouth(&l.name));
    let mouthm = mask_of(&r.render(&alone(&rigged, sym, |l| is_mouth(&l.name))), 128);
    let mouth_img = r.render(&edit_layers(&rigged, sym, |l| is_mouth(&l.name), |l| l.visible = false));
    let (mut mholes, mut mshown, mut mskin) = (0, 0, 0);
    for i in 0..n {
        if !mouthm[i] || pix(&after, i)[3] < 250 {
            continue;
        }
        mshown += 1;
        if pix(&mouth_img, i)[3] < 250 {
            mholes += 1;
        }
        if skinish(white(&pix(&mouth_img, i))) {
            mskin += 1;
        }
    }
    let mouth_blank_ok = mouth_found && mholes == 0 && mshown > 0;

    // Undo.
    let undone = doc.undo();
    let back = r.render(doc.scene());
    let back_diff = diff(&before, &back);
    let same_layers = layer_names(doc.scene(), sym) == layer_names(&before_scene, sym);
    let same_symbol = doc.scene().library().get(sym).map(|s| format!("{:?}", s.layers)) == before_scene.library().get(sym).map(|s| format!("{:?}", s.layers));
    let undo = undone && one_step && back_diff["max"].as_f64().is_some_and(|m| m <= 2.0) && same_layers && same_symbol;

    // Sheet.
    let eb = mm.iter().enumerate().filter(|(_, m)| **m).fold((i64::MAX, i64::MAX, 0i64, 0i64), |a, (i, _)| {
        let (x, y) = ((i as u32 % w) as i64, (i as u32 / w) as i64);
        (a.0.min(x), a.1.min(y), a.2.max(x), a.3.max(y))
    });
    let pad = ((eb.2 - eb.0).max(10) / 4).max(12);
    let eye_box = if eb.0 == i64::MAX { (0, 0, w as i64, h as i64) } else { (eb.0 - pad, eb.1 - pad * 2, eb.2 + pad, eb.3 + pad) };
    let mut diff_img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    for (i, p) in diff_img.pixels_mut().enumerate() {
        let d = pdiff(&before, &after, i);
        if d > 24.0 {
            *p = Rgba([255, 0, 0, 255]);
        } else if d > 4.0 {
            *p = Rgba([255, 180, 180, 255]);
        }
    }
    let face_box = {
        let (x0, y0, x1, y1) = eye_box;
        let cx = (x0 + x1) / 2;
        let hw = (x1 - x0).max(40);
        (cx - hw, y0 - (y1 - y0), cx + hw, y1 + (hw as f64 * 1.6) as i64)
    };
    let mut layers_row = Vec::new();
    for l in rigged.library().get(sym).unwrap().layers.iter() {
        let id = l.id;
        layers_row.push(r.render(&alone(&rigged, sym, |m| m.id == id)));
    }
    let png = format!("{out}/{}.png", label.replace(' ', "_"));
    std::fs::create_dir_all(format!("{out}/raw")).ok();
    before.save(format!("{out}/raw/{}_before.png", label.replace(' ', "_"))).unwrap();
    after.save(format!("{out}/raw/{}_after.png", label.replace(' ', "_"))).unwrap();
    mask_alone.save(format!("{out}/raw/{}_mask.png", label.replace(' ', "_"))).unwrap();
    sheet(
        vec![
            vec![before.clone(), after.clone(), diff_img],
            lid_frames.iter().map(|i| crop(i, eye_box)).collect(),
            eye_imgs.iter().map(|i| crop(i, eye_box)).chain([crop(&after, eye_box)]).collect(),
            vec![crop(&after, face_box), crop(&brow_img, face_box), crop(&mouth_img, face_box)],
            layers_row,
        ],
        &png,
        420,
    );
    let v = json!({
        "head": label, "ms": ms, "layers": names, "view": report.view, "eyes": report.eyes, "eye_holes": report.eye_holes,
        "cut": report.cut, "irises_completed": report.irises_completed, "notes": report.notes,
        "skin": report.skin, "ink": report.ink,
        "rest_diff": rest, "blink_cover": cover.iter().map(|c| (c * 1000.0).round() / 1000.0).collect::<Vec<_>>(), "blink_ok": blink_ok,
        "eyeball": eye_moves, "eyeball_ok": eyeball_ok,
        "brows": {"found": brows_found, "holes": bholes, "shown_px": bshown, "skin_px": bskin}, "brow_move_ok": brow_move_ok,
        "mouth": {"found": mouth_found, "holes": mholes, "shown_px": mshown, "skin_px": mskin}, "mouth_blank_ok": mouth_blank_ok,
        "undo": undo, "undo_detail": {"same_symbol": same_symbol, "one_step": one_step, "undone": undone, "render": back_diff, "same_layers": same_layers},
        "png": png,
    });
    println!(
        "  blink {:?} eyeball_ok {eyeball_ok} {eye_moves:?}\n  brows ok {brow_move_ok} holes {bholes} shown {bshown} skin {bskin}; mouth ok {mouth_blank_ok} holes {mholes} shown {mshown} skin {mskin}; undo {undo}",
        cover
    );
    (v, rigged)
}

fn role(n: &str, kind: LayerKind) -> Option<&'static str> {
    let l = n.to_lowercase();
    Some(if l.contains("sound") {
        return None;
    } else if l.contains("lid") {
        "eyeLid"
    } else if kind == LayerKind::Mask {
        "eyeMask"
    } else if kind == LayerKind::Masked || l.contains("ball") {
        "eyeBall"
    } else if l.contains("white") {
        "eyeWhite"
    } else if l.contains("brow") {
        "brows"
    } else if l.contains("moust") || l == "layer_17" {
        "moustache"
    } else if l.contains("mouth") {
        "mouth"
    } else if l.contains("nose") {
        "nose"
    } else if l.contains("hair") || l.contains("pagdi") {
        "hair"
    } else if l.contains("base") || l == "layer_1" {
        "base"
    } else {
        return None;
    })
}

fn main() {
    unsafe { std::env::set_var("BUZZ_NO_SYMBOL_CACHE", "1") };
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = a[1].clone();
    let ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).unwrap();
    let mut r = R { ex, region: Rect::ZERO, scale: 1.0 };
    let mut results = Vec::new();
    match a[0].as_str() {
        "ep1" => {
            let scene = load(&a[2]);
            let which: Vec<&str> = a[3].split(',').collect();
            for c in ["Guddu", "Lalten", "Munna"] {
                if which.contains(&"merged") {
                    let cast = find(&scene, c, "");
                    println!("{c} cast layers {:?}", layer_names(&scene, cast));
                    let parts = ["mouth", "right eyebrow", "left eyebrow", "right eye", "left eye", "head"];
                    let shapes = flatten_layers(&scene, cast, |l| parts.contains(&l.name.to_lowercase().as_str()));
                    let mut sc = scene.clone();
                    let sym = merged_symbol(&mut sc, &format!("{c} merged head"), &shapes);
                    let b = stage_alone(&mut sc, sym);
                    r.region = b.inflate(6.0, 6.0);
                    r.scale = (900.0 / b.width().max(b.height())).min(4.0);
                    results.push(check(&mut r, sc, sym, &format!("ep1 {c} merged"), &out).0);
                }
                if which.contains(&"cast") {
                    let mut sc = scene.clone();
                    let sym = find(&sc, &format!("{c}_head"), "");
                    let b = stage_alone(&mut sc, sym);
                    r.region = b.inflate(6.0, 6.0);
                    r.scale = (900.0 / b.width().max(b.height())).min(4.0);
                    results.push(check(&mut r, sc, sym, &format!("ep1 {c}_head as-is"), &out).0);
                }
            }
        }
        "gt" => {
            // gt <out> <sampleNaming.xfl> <womanSample.xfl>
            let heads = [("pradhan FrontHead", 0usize, "FrontHead", "pradhan"), ("man1 frontHEadman", 0, "frontHEadman", "man1"), ("woman head1", 1, "head1", "woman")];
            let docs = [load(&a[2]), load(&a[3])];
            for (label, d, name, hint) in heads {
                let scene = &docs[d];
                let user = find(scene, name, hint);
                let shapes = flatten_layers(scene, user, |l| !is_lid(&l.name));
                let mut sc = scene.clone();
                let sym = merged_symbol(&mut sc, &format!("{name} merged"), &shapes);
                let b = stage_alone(&mut sc, sym);
                r.region = b.inflate(6.0, 6.0);
                r.scale = (700.0 / b.width().max(b.height())).min(6.0);
                let (mut v, rigged) = check(&mut r, sc, sym, &format!("gt {label}"), &out);
                if v.get("error").is_some() {
                    results.push(v);
                    continue;
                }
                // The user's head, at the same place (identity).
                let mut usc = scene.clone();
                stage_alone(&mut usc, user);
                let user_layers = usc.library().get(user).unwrap().layers.clone();
                let ours_layers = rigged.library().get(sym).unwrap().layers.clone();
                let mut user_roles: Vec<&str> = Vec::new();
                for l in user_layers.iter() {
                    let id = l.id;
                    let img = r.render(&alone(&usc, user, |m| m.id == id));
                    let empty = mask_of(&img, 128).iter().all(|x| !x);
                    if let Some(x) = role(&l.name, l.kind) {
                        if (!empty || x == "eyeLid") && user_roles.last() != Some(&x) {
                            user_roles.push(x)
                        }
                    }
                }
                let mut our_roles: Vec<&str> = Vec::new();
                for l in ours_layers.iter() {
                    if let Some(x) = role(&l.name, l.kind) {
                        if our_roles.last() != Some(&x) {
                            our_roles.push(x)
                        }
                    }
                }
                let stack_ok = |v: &[&str]| {
                    let p = v.iter().position(|x| *x == "eyeLid");
                    p.is_some_and(|p| v.get(p..p + 4) == Some(&["eyeLid", "eyeMask", "eyeBall", "eyeWhite"][..])) && v.last() == Some(&"base")
                };
                let mut us: Vec<&str> = user_roles.clone();
                us.sort();
                let mut os: Vec<&str> = our_roles.clone();
                os.sort();
                let structure_equal = us == os && stack_ok(&user_roles) && stack_ok(&our_roles);
                let mut iou = serde_json::Map::new();
                let mut parts_rows = Vec::new();
                let lid_shut = edit_layers(&rigged, sym, |l| is_lid(&l.name), |l| {
                    objects_edit(l, |o| {
                        if let ObjectKind::Instance(i) = &mut o.kind {
                            i.first_frame = 3;
                            i.loop_mode = LoopMode::SingleFrame;
                        }
                    })
                });
                for want in ["eyeWhite", "eyeMask", "eyeBall", "eyeLid", "brows", "moustache", "mouth", "nose", "hair", "base"] {
                    let ui = r.render(&alone(&usc, user, |l| role(&l.name, l.kind) == Some(want)));
                    let oi = if want == "eyeLid" {
                        r.render(&alone(&lid_shut, sym, |l| role(&l.name, l.kind) == Some(want)))
                    } else {
                        r.render(&alone(&rigged, sym, |l| role(&l.name, l.kind) == Some(want)))
                    };
                    let (mut um, mut om) = (mask_of(&ui, 128), mask_of(&oi, 128));
                    if want == "eyeMask" {
                        um = fill_holes(&um, ui.width(), ui.height());
                        om = fill_holes(&om, oi.width(), oi.height());
                    }
                    let inter = um.iter().zip(&om).filter(|(a, b)| **a && **b).count();
                    let uni = um.iter().zip(&om).filter(|(a, b)| **a || **b).count();
                    if uni > 0 {
                        iou.insert(want.into(), json!((inter as f64 / uni as f64 * 1000.0).round() / 1000.0));
                    }
                    parts_rows.push(vec![ui, oi]);
                }
                sheet(parts_rows, &format!("{out}/{}_parts.png", label.replace(' ', "_")), 200);
                println!("  user roles {user_roles:?}\n  our roles  {our_roles:?}\n  structure_equal {structure_equal} iou {:?}", iou);
                v["user_roles"] = json!(user_roles);
                v["our_roles"] = json!(our_roles);
                v["structure_equal"] = json!(structure_equal);
                v["iou"] = Value::Object(iou);
                results.push(v);
            }
        }
        _ => panic!("mode"),
    }
    let path = format!("{out}/h2_results_{}.json", a[0]);
    std::fs::write(&path, serde_json::to_string_pretty(&results).unwrap()).unwrap();
    println!("wrote {path}");
}
