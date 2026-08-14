//! Bringing an Animate asset library across, all of it, in one go.
//!
//! # What Animate keeps, and where
//!
//! `Documents/Adobe/Animate/<year>/Assets/Custom/<guid>/` holds one folder per
//! saved asset:
//!
//! * `manifest.json` — the name the animator gave it, and how Animate files it
//!   (`role`: Characters, Props…; `subCategory`: Objects, Backgrounds…),
//! * `<name>.fla` — the asset itself, which is an ordinary XFL container and
//!   therefore something this program already knows how to read,
//! * `<name>__an__r1x.png` and friends — thumbnails Animate drew for its own
//!   panel.
//!
//! So an import is: walk the folders, read each manifest for a name and a
//! place to file it under, run the `.fla` through the importer that has existed
//! since Phase 5, and save the result into the asset library. Nothing about the
//! format needed inventing; what was missing was the walk.
//!
//! # Why it runs on a thread
//!
//! A library of a thousand assets is a thousand zip archives to open and parse.
//! That is a minute or two of work, and doing it on the UI thread would freeze
//! the window for all of it — so the job runs on its own thread and reports
//! progress back through a channel, exactly as an export sequence does.

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, Sender};

/// One asset found in Animate's library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimateAsset {
    /// What the animator called it.
    pub name: String,
    /// Where it should be filed here, `/`-separated.
    pub folder: String,
    /// The `.fla` to read.
    pub source: PathBuf,
}

/// How far along an import is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// `done` of `total` assets attempted.
    Working { done: usize, total: usize },
    /// Everything that could be read has been saved.
    Finished {
        imported: usize,
        /// Assets this program cannot read *yet* — bitmaps, mostly, which
        /// §7 item 22 records as unimported by every reader here.
        skipped: usize,
        /// Assets that should have worked and did not, named.
        failed: Vec<String>,
    },
}

/// Can this program read the file an asset points at?
///
/// Animate's assets are usually `.fla`, but a bitmap dropped into the panel is
/// saved as the image itself. Those are recognised and *skipped* rather than
/// attempted: a failure list of three hundred "cannot import .png" lines
/// buries the handful of real problems.
fn is_importable(source: &Path) -> bool {
    source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| crate::import::IMPORTABLE.contains(&e.as_str()))
}

/// Where Animate keeps its assets on this machine, newest year first.
///
/// Offered as the folder picker's starting point: the path is long, buried
/// under `Documents`, and nobody should have to remember it.
pub fn likely_roots() -> Vec<PathBuf> {
    let Some(documents) = documents_dir() else {
        return Vec::new();
    };
    let animate = documents.join("Adobe").join("Animate");
    let Ok(entries) = std::fs::read_dir(&animate) else {
        return Vec::new();
    };

    let mut years: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("Assets").is_dir())
        .map(|p| p.join("Assets"))
        .collect();
    // A year is a number, so the newest sorts last; the newest is the one
    // somebody is most likely to want.
    years.sort();
    years.reverse();
    years
}

fn documents_dir() -> Option<PathBuf> {
    // `USERPROFILE/Documents` on Windows; `HOME/Documents` elsewhere. Not a
    // shell API call: this is a hint for a file picker, not a place anything is
    // written.
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    Some(home.join("Documents"))
}

/// Every asset under an Animate assets folder.
///
/// Takes either the `Assets` folder or its `Custom` subfolder, because both are
/// things somebody might reasonably point at.
pub fn scan(root: &Path) -> Vec<AnimateAsset> {
    let custom = if root.file_name().is_some_and(|n| n == "Custom") {
        root.to_path_buf()
    } else {
        root.join("Custom")
    };

    let Ok(entries) = std::fs::read_dir(&custom) else {
        return Vec::new();
    };

    let mut found: Vec<AnimateAsset> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|dir| read_manifest(&dir))
        .collect();

    found.sort_by(|a, b| (&a.folder, &a.name).cmp(&(&b.folder, &b.name)));
    found
}

/// Read one asset folder, if it holds an asset.
fn read_manifest(dir: &Path) -> Option<AnimateAsset> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&text).ok()?;

    let file = manifest.get("assetFile")?.as_str()?;
    let source = dir.join(file);
    if !source.exists() {
        return None;
    }

    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|n| !n.trim().is_empty())
        .map(|n| n.to_string())
        .or_else(|| source.file_stem().map(|s| s.to_string_lossy().to_string()))?;

    Some(AnimateAsset {
        name,
        folder: folder_for(&manifest),
        source,
    })
}

/// Where an asset is filed here, from how Animate filed it.
///
/// `Animate/<role>/<subCategory>` — under one folder of its own, because
/// dropping a thousand assets in among somebody's own would be rude, and with
/// Animate's own arrangement kept, because that is the arrangement they know.
fn folder_for(manifest: &serde_json::Value) -> String {
    let part = |key: &str| {
        manifest
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "None")
            .map(|s| s.replace(['/', '\\'], "-"))
    };

    let mut folder = String::from("Animate");
    if let Some(role) = part("role") {
        folder.push('/');
        folder.push_str(&role);
    }
    if let Some(sub) = part("subCategory") {
        folder.push('/');
        folder.push_str(&sub);
    }
    folder
}

/// Import every asset, on a thread, reporting progress as it goes.
///
/// Returns immediately. Each asset that fails is *named* in the summary rather
/// than stopping the run: one unreadable file out of a thousand should cost one
/// asset, not the import.
pub fn import_all(
    assets: Vec<AnimateAsset>,
    mut library: buzz_doc::AssetLibrary,
) -> Receiver<Progress> {
    let (tx, rx): (Sender<Progress>, Receiver<Progress>) = crossbeam_channel::unbounded();

    std::thread::spawn(move || {
        let total = assets.len();
        let mut imported = 0usize;
        let mut skipped = 0usize;
        let mut failed: Vec<String> = Vec::new();

        for (index, asset) in assets.into_iter().enumerate() {
            if !is_importable(&asset.source) {
                skipped += 1;
                if index % 20 == 0 || index + 1 == total {
                    let _ = tx.send(Progress::Working {
                        done: index + 1,
                        total,
                    });
                }
                continue;
            }

            match crate::import::read(&asset.source) {
                Ok(read) => match library.save(&asset.name, &asset.folder, &read.scene) {
                    Ok(_) => imported += 1,
                    Err(e) => failed.push(format!("{}: {e}", asset.name)),
                },
                Err(e) => failed.push(format!("{}: {e}", asset.name)),
            }

            // Every twentieth, and at the end: a channel message per asset is
            // more traffic than a progress bar can use.
            if index % 20 == 0 || index + 1 == total {
                let _ = tx.send(Progress::Working {
                    done: index + 1,
                    total,
                });
            }
        }

        let _ = tx.send(Progress::Finished {
            imported,
            skipped,
            failed,
        });
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a folder shaped like Animate's, with `count` assets in it.
    fn animate_library(dir: &Path, count: usize) {
        let custom = dir.join("Custom");
        std::fs::create_dir_all(&custom).expect("custom");

        for i in 0..count {
            let asset = custom.join(format!("guid-{i}"));
            std::fs::create_dir_all(&asset).expect("asset dir");
            std::fs::write(asset.join(format!("Prop {i}.fla")), b"PK\x03\x04not really")
                .expect("fla");
            std::fs::write(
                asset.join("manifest.json"),
                format!(
                    r#"{{"assetFile":"Prop {i}.fla","name":"Prop {i}",
                        "role":"Props","subCategory":"Objects"}}"#
                ),
            )
            .expect("manifest");
        }
    }

    #[test]
    fn every_asset_in_the_folder_is_found() {
        let dir = tempfile::tempdir().expect("temp");
        animate_library(dir.path(), 3);

        let found = scan(dir.path());
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].name, "Prop 0");
        assert_eq!(found[0].folder, "Animate/Props/Objects");
        assert!(found[0].source.exists());
    }

    /// Pointing at `Custom` directly works too: it is what somebody browsing
    /// the folder is most likely to land on.
    #[test]
    fn the_custom_folder_can_be_given_directly() {
        let dir = tempfile::tempdir().expect("temp");
        animate_library(dir.path(), 2);
        assert_eq!(scan(&dir.path().join("Custom")).len(), 2);
    }

    /// A folder that is not an Animate library gives nothing, rather than an
    /// error the user has to dismiss.
    #[test]
    fn an_unrelated_folder_yields_nothing() {
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("notes.txt"), b"hello").expect("write");
        assert!(scan(dir.path()).is_empty());
    }

    /// An asset folder missing its `.fla`, or its manifest, is skipped rather
    /// than importing something that is not there.
    #[test]
    fn an_incomplete_asset_is_skipped() {
        let dir = tempfile::tempdir().expect("temp");
        let custom = dir.path().join("Custom");
        std::fs::create_dir_all(custom.join("broken")).expect("dir");
        std::fs::write(
            custom.join("broken").join("manifest.json"),
            br#"{"assetFile":"gone.fla","name":"Gone"}"#,
        )
        .expect("manifest");
        std::fs::create_dir_all(custom.join("no-manifest")).expect("dir");

        assert!(scan(dir.path()).is_empty());
    }

    /// Animate's own filing is kept, under one folder of its own.
    #[test]
    fn assets_are_filed_the_way_animate_filed_them() {
        let folder = |json: &str| folder_for(&serde_json::from_str(json).unwrap());

        assert_eq!(
            folder(r#"{"role":"Characters","subCategory":"Objects"}"#),
            "Animate/Characters/Objects"
        );
        assert_eq!(folder(r#"{"role":"Props"}"#), "Animate/Props");
        assert_eq!(folder("{}"), "Animate");
        assert_eq!(
            folder(r#"{"role":"a/b"}"#),
            "Animate/a-b",
            "a slash in a name must not become a folder"
        );
    }

    /// **One bad file costs one asset.** An import of a thousand that stopped
    /// at the first unreadable one would be useless.
    #[test]
    fn a_failure_does_not_stop_the_run() {
        let dir = tempfile::tempdir().expect("temp");
        animate_library(dir.path(), 3);
        let library_dir = tempfile::tempdir().expect("library");
        let library = buzz_doc::AssetLibrary::at(library_dir.path());

        // The fixtures are not real `.fla` files, so all three fail — which is
        // exactly the case worth checking: the run still finishes and says so.
        let progress = import_all(scan(dir.path()), library);
        let mut last = None;
        while let Ok(message) = progress.recv() {
            last = Some(message);
        }

        match last {
            Some(Progress::Finished {
                imported, failed, ..
            }) => {
                assert_eq!(imported, 0);
                assert_eq!(failed.len(), 3, "each failure is named: {failed:?}");
            }
            other => panic!("the run should have finished: {other:?}"),
        }
    }

    /// **A bitmap asset is skipped, not failed.** Animate's panel takes images
    /// as well as symbols, and this program does not read bitmaps yet (§7 item
    /// 22) — three hundred "cannot import .png" lines would bury the failures
    /// that matter.
    #[test]
    fn assets_this_program_cannot_read_are_skipped_quietly() {
        let dir = tempfile::tempdir().expect("temp");
        let custom = dir.path().join("Custom");
        std::fs::create_dir_all(custom.join("image")).expect("dir");
        std::fs::write(custom.join("image").join("Sky.png"), b"not really a png").expect("png");
        std::fs::write(
            custom.join("image").join("manifest.json"),
            br#"{"assetFile":"Sky.png","name":"Sky","role":"Backgrounds"}"#,
        )
        .expect("manifest");

        let found = scan(dir.path());
        assert_eq!(found.len(), 1, "it is still an asset in the library");

        let library_dir = tempfile::tempdir().expect("library");
        let progress = import_all(found, buzz_doc::AssetLibrary::at(library_dir.path()));
        let mut last = None;
        while let Ok(message) = progress.recv() {
            last = Some(message);
        }

        match last {
            Some(Progress::Finished {
                imported,
                skipped,
                failed,
            }) => {
                assert_eq!(imported, 0);
                assert_eq!(skipped, 1);
                assert!(failed.is_empty(), "a bitmap is not a failure: {failed:?}");
            }
            other => panic!("{other:?}"),
        }
    }
}
