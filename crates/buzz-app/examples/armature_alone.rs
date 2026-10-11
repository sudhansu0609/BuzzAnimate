//! F7.6 check: a bare Armature object on a stage layer, rendered alone.
use std::path::Path;
use buzz_scene::{LayerKind, ObjectKind};
fn main() -> anyhow::Result<()> {
    let fla = std::env::args().nth(1).expect("fla");
    let out = std::env::args().nth(2).expect("out dir");
    let scene = buzz_app::import::read(Path::new(&fla)).map_err(anyhow::Error::msg)?.scene;
    // Find an armature object anywhere in the library.
    let mut found = None;
    'outer: for sym in scene.library().iter() {
        for layer in sym.layers.iter() {
            for obj in layer.all_objects() {
                if let ObjectKind::Armature(_) = &obj.kind { found = Some((sym.name.clone(), obj.clone())); break 'outer; }
            }
        }
    }
    if found.is_none() {
        let sym = scene.library().iter().find(|s| s.name == "sadhuback").expect("sadhuback").clone();
        let mut parts = Vec::new();
        for (i, layer) in sym.layers.iter().enumerate() {
            let objs = layer.objects_at(0);
            if objs.is_empty() { continue; }
            let o = if objs.len() == 1 { objs[0].clone() } else { std::sync::Arc::new(buzz_scene::Object::group(buzz_scene::ObjectId(900_000 + i as u64), objs.to_vec())) };
            parts.push((i % 11, o));
        }
        let rig = buzz_act::autorig::assemble(&buzz_rig::pattern::RigPattern::biped(), &parts).expect("assemble");
        let mut o = buzz_scene::Object::group(buzz_scene::ObjectId(1), vec![]);
        o.kind = ObjectKind::Armature(rig);
        found = Some(("sadhuback (assembled)".into(), std::sync::Arc::new(o)));
    }
    let Some((name, obj)) = found else { unreachable!() };
    println!("armature from symbol {name}");
    let mut s = scene.clone();
    s.edit_document();
    if std::env::var("NOCAM").is_ok() { s.camera_mut().enabled = false; }
    for id in s.layers().iter().map(|l| l.id).collect::<Vec<_>>() { s.remove_layer(id); }
    let layer = s.add_layer("Armature", LayerKind::Normal);
    let mut o = (*obj).clone();
    let b = s.resolved_bounds(&o);
    let b = if std::env::var("OLD").is_ok() { o.bounds() } else { b };
    println!("resolved bounds {b:?}; object.bounds {:?}", o.bounds());
    if let ObjectKind::Armature(rig) = &o.kind {
        for (i, part) in rig.posed().iter().enumerate() {
            println!("part {i}: bounds {:?} resolved {:?} kind_instance={}", part.bounds(), s.resolved_bounds(part), part.instance().is_some());
        }
        println!("bones {:?}", rig.armature.bounds());
    }
    let scale = 800.0 / b.height().max(1.0);
    s.stage_mut().size = buzz_geom::Size::new(b.width() * scale + 100.0, 900.0);
    o.transform = buzz_geom::Affine::translate((s.stage().size.width / 2.0, 450.0)) * buzz_geom::Affine::scale(scale) * buzz_geom::Affine::translate((-b.center().x, -b.center().y));
    if std::env::var("IDENT").is_ok() {
        s.stage_mut().size = buzz_geom::Size::new(2400.0, 1600.0);
        o.transform = buzz_geom::Affine::translate((400.0, 400.0));
        println!("expected bbox {:?}", s.resolved_bounds(&o));
    }
    o.id = s.next_object_id();
    s.add_object(layer, o);
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?;
    let st = buzz_export::ExportSettings { transparent: true, ..buzz_export::ExportSettings::for_stage(&s) };
    let f = ex.render(&s, 0, &st)?;
    let opaque = f.pixels.chunks(4).filter(|p| p[3] > 0).count();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..f.height { for x in 0..f.width { if f.pixel(x, y)[3] > 0 { x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y); } } }
    println!("{}x{} opaque px {opaque} bbox {x0},{y0}-{x1},{y1}", f.width, f.height);
    f.write_png(&Path::new(&out).join("armature_alone.png"))?;
    Ok(())
}
