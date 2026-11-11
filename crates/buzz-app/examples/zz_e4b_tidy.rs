//! E4b (1): zz_e4b_tidy <library-root> [--apply] [ROLE,...] — name the Mouth / Eyelid chart
//! layers inside every pose's head symbol and unwrap the lid chart (cast_fla::tidy_pose).
//! Dry run by default; --apply backs the Cast folder up first, then saves.
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
    if apply {
        let b = buzz_app::cast_fla::backup_cast(&root).map_err(anyhow::Error::msg)?;
        println!("backup {}", b.display());
    }
    for role in roles {
        let Some(asset) = buzz_app::cast_fresh::cast_characters(&library).into_iter().find(|x| x.name == role && x.folder.ends_with("Khayal3Baje")) else { continue };
        let mut scene = library.load(&asset)?;
        let Some(main) = buzz_app::cast_combine::main_symbol(&scene) else { continue };
        for pose in scene.library().symbol_set(main) {
            let name = scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default();
            let done = buzz_app::cast_fla::tidy_pose(&mut scene, pose);
            println!("{role} / {name}: {done:?}");
        }
        if apply {
            buzz_doc::format::save(&scene, &asset.path).map_err(anyhow::Error::msg)?;
        }
    }
    Ok(())
}
