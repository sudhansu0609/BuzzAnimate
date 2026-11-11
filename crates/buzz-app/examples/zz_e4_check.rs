//! E4 read-only check: zz_e4_check <library-root> [ROLE,...] — per role, per pose:
//! layer tree (names, parent), planned body/face renames, instance symbol lengths.
use buzz_doc::AssetLibrary;
use buzz_scene::{ObjectKind, Scene, SymbolId};

fn tree(scene: &Scene, sym: SymbolId, depth: usize, max: usize, out: &mut String) {
    let Some(s) = scene.library().get(sym) else { return };
    for l in s.layers.iter() {
        let parent = l.follows.and_then(|f| s.layers.get(f)).map(|p| p.name.clone()).unwrap_or_default();
        let parent2 = l.parent.and_then(|f| s.layers.get(f)).map(|p| p.name.clone()).unwrap_or_default();
        let mut line = format!("{}- {:?} [{:?}] follows={:?} folder={:?}", "  ".repeat(depth), l.name, l.kind, parent, parent2);
        let mut kids = Vec::new();
        for o in l.objects_at(0) {
            if let ObjectKind::Instance(i) = &o.kind {
                if let Some(cs) = scene.library().get(i.symbol) {
                    line += &format!(" => {:?} ({}f, first={}, {:?})", cs.name, cs.length(), i.first_frame, i.loop_mode);
                    kids.push(i.symbol);
                }
            } else {
                line += " (drawing)";
            }
        }
        out.push_str(&line);
        out.push('\n');
        if depth < max { for k in kids { tree(scene, k, depth + 1, max, out); } }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let library = AssetLibrary::at(std::path::Path::new(&a[0]));
    let roles: Vec<String> = a.get(1).map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_else(|| ["DEEPAK", "BHUSHAN", "MISHRA JI", "RADHE", "PADOSI 1", "PADOSI 2"].map(String::from).to_vec());
    let profile = buzz_app::rigging::naming_profile(None).map_err(anyhow::Error::msg)?;
    for role in roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| x.name == role && x.folder.ends_with("Khayal3Baje")) else { println!("== {role}: missing"); continue };
        let scene = library.load(&asset)?;
        let Some(main) = buzz_app::cast_combine::main_symbol(&scene) else { println!("== {role}: not combined"); continue };
        println!("== {role}");
        for pose in scene.library().symbol_set(main) {
            let s = scene.library().get(pose).unwrap();
            let body = buzz_app::rigging::body_layer_renames(&s.layers, &profile);
            let mut probe = scene.clone();
            let (own, head) = buzz_app::rigging::rename_face_layers(&mut probe, buzz_app::rigging::RigHome::Symbol(pose));
            println!("-- pose {:?} ({} layers) body renames: {:?}; face renames: {:?}; head renames: {:?}", s.name, s.layers.iter().count(),
                body.iter().map(|r| format!("{}->{}", r.from, r.to)).collect::<Vec<_>>(),
                own.iter().map(|r| format!("{}->{}", r.from, r.to)).collect::<Vec<_>>(),
                head.map(|(_, r)| r.iter().map(|r| format!("{}->{}", r.from, r.to)).collect::<Vec<_>>()));
            if std::env::var("E4_DEPTH").is_ok() || scene.library().symbol_set(main).first() == Some(&pose) {
                let mut out = String::new();
                tree(&scene, pose, 1, 3, &mut out);
                print!("{out}");
            }
        }
    }
    Ok(())
}
