//! E4b read-only dump: zz_e4b_dump <library-root> ROLE POSE — the head symbol's layers, every
//! key (start, instance symbol, first_frame, loop, transform) of the lid / mouth layers.
use buzz_doc::AssetLibrary;
use buzz_scene::{ObjectKind, Scene, SymbolId};

fn dump(scene: &Scene, sym: SymbolId, depth: usize, out: &mut Vec<String>) {
    let Some(s) = scene.library().get(sym) else { return };
    let pad = "  ".repeat(depth);
    for l in s.layers.iter() {
        out.push(format!("{pad}layer {:?} {:?} keys={}", l.name, l.kind, l.frames.keyframes().len()));
        for k in l.frames.keyframes().iter().take(12) {
            for o in k.objects.iter() {
                if let ObjectKind::Instance(i) = &o.kind {
                    let n = scene.library().get(i.symbol).map(|c| (c.name.clone(), c.length())).unwrap_or_default();
                    out.push(format!("{pad}  @{} -> {:?} ({}f) first={} {:?} tr={:?} vis={}", k.start, n.0, n.1, i.first_frame, i.loop_mode, o.transform.as_coeffs().map(|v| (v * 10.0).round() / 10.0), o.visible));
                } else {
                    out.push(format!("{pad}  @{} drawing", k.start));
                }
            }
        }
        for k in l.frames.keyframes().iter().take(1) {
            for o in k.objects.iter() {
                if let ObjectKind::Instance(i) = &o.kind {
                    let n = scene.library().get(i.symbol).map(|c| c.name.to_lowercase()).unwrap_or_default();
                    if depth < 4 && (n.contains("eye") || n.contains("lid") || n.contains("wrap") || n.contains("symbol") || n.contains("head") || n.contains("face") || depth == 0) && scene.library().get(i.symbol).is_some_and(|c| c.length() > 1) {
                        dump(scene, i.symbol, depth + 1, out);
                    }
                }
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let library = AssetLibrary::at(std::path::Path::new(&a[0]));
    let asset = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| x.name == a[1] && x.folder.ends_with("Khayal3Baje")).expect("role");
    let scene = library.load(&asset)?;
    let main = buzz_app::cast_combine::main_symbol(&scene).unwrap();
    for sym in scene.library().iter() {
        let n = sym.name.to_lowercase();
        if sym.length() == 7 && (n.contains("lash") || n.contains("eyelid")) {
            let mut line = format!("chart {:?}:", sym.name);
            for f in 0..7 {
                let mut u: Option<buzz_geom::Rect> = None;
                for l in sym.layers.iter() {
                    for o in l.objects_at(f).iter() {
                        let r = scene.resolved_bounds(o);
                        u = Some(u.map_or(r, |x| x.union(r)));
                    }
                }
                line += &u.map_or(" none".into(), |r| format!(" [{:.0}x{:.0}]", r.width(), r.height()));
            }
            println!("{line}");
        }
    }
    for pose in scene.library().symbol_set(main) {
        let s = scene.library().get(pose).unwrap();
        if s.name != a[2] { continue; }
        let mut out = Vec::new();
        dump(&scene, pose, 0, &mut out);
        for l in out { println!("{l}"); }
    }
    Ok(())
}
