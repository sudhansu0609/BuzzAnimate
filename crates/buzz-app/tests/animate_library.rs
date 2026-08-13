//! A check against the Animate library installed on this machine: how many
//! assets there are, how they file, and whether they actually import.
//!
//! `cargo test -p buzz-app --test animate_library -- --ignored --nocapture`

use std::path::PathBuf;

fn animate_root() -> Option<PathBuf> {
    buzz_app::animate_assets::likely_roots().into_iter().next()
}

#[test]
#[ignore = "reads the Animate library installed on this machine"]
fn the_real_animate_library_imports() {
    let Some(root) = animate_root() else {
        eprintln!("no Animate library on this machine");
        return;
    };
    println!("library: {}", root.display());

    let found = buzz_app::animate_assets::scan(&root);
    println!("found {} assets", found.len());

    let mut folders: std::collections::BTreeMap<String, usize> = Default::default();
    let mut bitmaps = 0;
    for asset in &found {
        *folders.entry(asset.folder.clone()).or_default() += 1;
        if asset
            .source
            .extension()
            .is_some_and(|e| !e.eq_ignore_ascii_case("fla") && !e.eq_ignore_ascii_case("xfl"))
        {
            bitmaps += 1;
        }
    }
    for (folder, count) in &folders {
        println!("  {folder}: {count}");
    }
    println!("not documents (bitmaps and the like): {bitmaps}");
    assert!(!found.is_empty(), "the library should hold something");

    // A sample imported for real, into a temporary library.
    let dir = tempfile::tempdir().expect("temp");
    let mut library = buzz_doc::AssetLibrary::at(dir.path());
    let mut ok = 0;
    let mut failed = Vec::new();
    for asset in found.iter().take(25) {
        match buzz_app::import::read(&asset.source) {
            Ok(read) => match library.save(&asset.name, &asset.folder, &read.scene) {
                Ok(_) => ok += 1,
                Err(e) => failed.push(format!("{}: {e}", asset.name)),
            },
            Err(e) => failed.push(format!("{}: {e}", asset.name)),
        }
    }
    println!("imported {ok} of 25 sampled");
    for line in &failed {
        println!("  {line}");
    }
    assert!(ok > 0, "not one asset could be read: {failed:?}");
}
