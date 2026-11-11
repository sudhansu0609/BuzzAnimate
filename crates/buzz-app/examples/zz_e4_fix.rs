//! E4 fix (layer names): zz_e4_fix <library-root> [--apply] [ROLE,...]
//! Runs Rename Body Layers + Rename Face Layers (naming profile) on every pose
//! symbol of each combined Cast role (and the head symbols inside). Dry run by
//! default: prints what would change and which names would end up duplicated.
//! --apply backs the Cast folder up first (cast_fla::backup_cast), then saves.
//! Only run --apply once CAST_POSES_PLAN C2f is built/accepted.
use buzz_app::rigging::{rename_body_layers, rename_face_layers, RigHome};
use buzz_doc::AssetLibrary;

fn main() -> anyhow::Result<()> {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::PathBuf::from(a.remove(0));
    let apply = a.iter().any(|x| x == "--apply");
    a.retain(|x| x != "--apply");
    let roles: Vec<String> = a
        .first()
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_else(|| ["DEEPAK", "BHUSHAN", "MISHRA JI", "RADHE", "PADOSI 1", "PADOSI 2"].map(String::from).to_vec());
    let library = AssetLibrary::at(&root);
    let profile = buzz_app::rigging::naming_profile(None).map_err(anyhow::Error::msg)?;
    if apply {
        let b = buzz_app::cast_fla::backup_cast(&root).map_err(anyhow::Error::msg)?;
        println!("backup {}", b.display());
    }
    for role in roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| x.name == role && x.folder.ends_with("Khayal3Baje")) else { continue };
        let mut scene = library.load(&asset)?;
        let Some(main) = buzz_app::cast_combine::main_symbol(&scene) else { continue };
        let (mut body, mut face, mut dup) = (0, 0, Vec::new());
        for pose in scene.library().symbol_set(main) {
            body += rename_body_layers(&mut scene, RigHome::Symbol(pose), &profile).len();
            let (own, head) = rename_face_layers(&mut scene, RigHome::Symbol(pose));
            face += own.len() + head.map_or(0, |(_, r)| r.len());
            let pname = scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default();
            let (view, _) = buzz_app::cast_fla::classify(&pname);
            let fixed = buzz_app::cast_fla::fix_side_duplicates(&mut scene, pose, view);
            if !fixed.is_empty() {
                println!("  {pname}: left/right duplicates by x: {fixed:?}");
            }
            let mut seen = std::collections::HashMap::<String, u32>::new();
            if let Some(s) = scene.library().get(pose) {
                for l in s.layers.iter() {
                    *seen.entry(l.name.to_lowercase()).or_default() += 1;
                }
                for (n, c) in seen {
                    if c > 1 && !n.starts_with("layer") && n != "sound" {
                        dup.push(format!("{}: {n} x{c}", s.name));
                    }
                }
            }
        }
        println!("{role}: body renames {body}, face renames {face}, duplicate names after: {dup:?}");
        if apply {
            buzz_doc::format::save(&scene, &asset.path).map_err(anyhow::Error::msg)?;
        }
    }
    Ok(())
}
