//! **Start Fresh** for the saved Cast characters (CAST_FRESH_PLAN F1): clear
//! every keyframe of a character kept in the asset library, keeping one pose,
//! so it starts clean in a new file.
//!
//! The clearing itself is [`buzz_scene::Scene::start_fresh`]; this is the
//! part that touches disk: a backup of the file first, outside the Cast folder
//! so it is not listed, then the cleared character saved in place.

use std::path::{Path, PathBuf};

use buzz_doc::{Asset, AssetLibrary};
use buzz_scene::{FreshReport, KeepPose};

/// The folder, under the library root, that holds the backups.
pub const BACKUP_FOLDER: &str = "_cast_backup";

/// Every character in the Cast folder and its subfolders.
pub fn cast_characters(library: &AssetLibrary) -> Vec<Asset> {
    library
        .assets()
        .iter()
        .filter(|a| a.folder == buzz_ui::CAST_FOLDER || a.folder.starts_with(&format!("{}/", buzz_ui::CAST_FOLDER)))
        .cloned()
        .collect()
}

/// `YYYY-MM-DD_HHMMSS` (UTC) for the backup folder, from now.
pub fn backup_stamp() -> String {
    stamp_of(std::time::SystemTime::now())
}

fn stamp_of(time: std::time::SystemTime) -> String {
    // RFC 3339 "2026-11-02T14:03:05Z" -> "2026-11-02_140305".
    let text = humantime::format_rfc3339_seconds(time).to_string();
    let (date, clock) = text.trim_end_matches('Z').split_once('T').unwrap_or((&text, ""));
    format!("{date}_{}", clock.replace(':', ""))
}

/// Where `asset` is copied before it is rewritten.
pub fn backup_path(root: &Path, asset: &Asset, stamp: &str) -> PathBuf {
    let relative = asset.path.strip_prefix(root).map(Path::to_path_buf).unwrap_or_else(|_| {
        let file = asset.path.file_name().map(PathBuf::from).unwrap_or_default();
        Path::new(&asset.folder).join(file)
    });
    root.join(BACKUP_FOLDER).join(stamp).join(relative)
}

/// What Start Fresh did to one character.
#[derive(Debug, Clone)]
pub struct FreshOutcome {
    pub name: String,
    pub report: FreshReport,
    pub backup: PathBuf,
}

/// Back `asset` up, clear every keyframe but one pose, and save it in place.
/// The library is rescanned by the save. Nothing is written if the backup or
/// the load fails.
pub fn start_fresh_asset(
    library: &mut AssetLibrary,
    asset: &Asset,
    keep: KeepPose,
    stamp: &str,
) -> Result<FreshOutcome, String> {
    let root = library
        .root()
        .map(Path::to_path_buf)
        .ok_or_else(|| "there is no assets folder on this machine".to_string())?;
    let mut scene = library.load(asset).map_err(|e| format!("could not read {}: {e}", asset.name))?;

    // Never overwrite an earlier backup of the same file (two runs in one second).
    let mut backup = backup_path(&root, asset, stamp);
    let mut again = 2;
    while backup.exists() {
        backup = backup_path(&root, asset, &format!("{stamp}_{again}"));
        again += 1;
    }
    if let Some(dir) = backup.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not make the backup folder: {e}"))?;
    }
    std::fs::copy(&asset.path, &backup).map_err(|e| format!("could not back up {}: {e}", asset.name))?;

    let report = scene.start_fresh(keep);
    buzz_doc::format::save(&scene, &asset.path).map_err(|e| format!("could not save {}: {e}", asset.name))?;
    library.rescan();
    Ok(FreshOutcome {
        name: asset.name.clone(),
        report,
        backup,
    })
}

/// Move `from` to `to`, making the folder; a rename, or copy-then-remove across
/// drives. A target that exists is never overwritten (a `_2` is added).
pub(crate) fn move_file(from: &Path, to: &Path) -> Result<PathBuf, String> {
    let mut to = to.to_path_buf();
    let mut again = 2;
    while to.exists() {
        let stem = to.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = to.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        to = to.with_file_name(format!("{stem}_{again}{ext}"));
        again += 1;
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not make the backup folder: {e}"))?;
    }
    if std::fs::rename(from, &to).is_err() {
        std::fs::copy(from, &to).map_err(|e| format!("could not move {}: {e}", from.display()))?;
        std::fs::remove_file(from).map_err(|e| format!("could not remove {}: {e}", from.display()))?;
    }
    Ok(to)
}

/// What a delete moved.
#[derive(Debug, Clone)]
pub struct DeletedOutcome {
    pub name: String,
    pub backup: PathBuf,
}

/// The backup folder a delete done now fills: `<root>/_cast_backup/<stamp>_deleted`.
pub fn deleted_folder(root: &Path, stamp: &str) -> PathBuf {
    root.join(BACKUP_FOLDER).join(format!("{stamp}_deleted"))
}

/// **Delete = move**: each asset file (and its index thumbnail, if there is one)
/// goes to `<root>/_cast_backup/<stamp>_deleted/<its path under the root>`.
/// Nothing is erased. The library is rescanned. Returns what moved and what failed.
pub fn delete_to_backup(
    library: &mut AssetLibrary,
    assets: &[Asset],
    stamp: &str,
) -> Result<(Vec<DeletedOutcome>, Vec<String>), String> {
    let root = library
        .root()
        .map(Path::to_path_buf)
        .ok_or_else(|| "there is no assets folder on this machine".to_string())?;
    let index = buzz_ai::asset_index::AssetIndex::load(&root);
    let base = deleted_folder(&root, stamp);
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for asset in assets {
        let rel = asset.path.strip_prefix(&root).map(Path::to_path_buf).unwrap_or_else(|_| {
            Path::new(&asset.folder).join(asset.path.file_name().map(PathBuf::from).unwrap_or_default())
        });
        let rel_text = buzz_ai::asset_index::relative(&root, &asset.path);
        let thumb = index
            .asset_record(&rel_text)
            .map(|r| r.thumb.clone())
            .filter(|t| !t.is_empty())
            .map(|t| root.join(t));
        match move_file(&asset.path, &base.join(&rel)) {
            Ok(backup) => {
                if let Some(thumb) = thumb.filter(|t| t.exists())
                    && let Ok(thumb_rel) = thumb.strip_prefix(&root)
                {
                    let _ = move_file(&thumb, &base.join(thumb_rel));
                }
                done.push(DeletedOutcome {
                    name: asset.name.clone(),
                    backup,
                });
            }
            Err(e) => failed.push(format!("{}: {e}", asset.name)),
        }
    }
    library.rescan();
    Ok((done, failed))
}

/// Names among `assets` that a series' `Cast/<Series>/casting.json` points at.
pub fn cast_in_series(root: &Path, assets: &[Asset]) -> Vec<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    let cast = root.join(buzz_ui::CAST_FOLDER);
    if let Ok(dir) = std::fs::read_dir(&cast) {
        for entry in dir.flatten() {
            let file = entry.path().join("casting.json");
            if file.is_file() {
                files.push(file);
            }
        }
    }
    let mut pointed: Vec<String> = Vec::new();
    for file in files {
        let Some(parsed) = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<crate::my_cast::CastingFile>(&t).ok())
        else {
            continue;
        };
        for choice in parsed.choices.values() {
            pointed.push(choice.asset.replace('\\', "/").to_lowercase());
        }
    }
    assets
        .iter()
        .filter(|a| pointed.contains(&buzz_ai::asset_index::relative(root, &a.path).replace('\\', "/").to_lowercase()))
        .map(|a| a.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_a_folder_name() {
        let stamp = stamp_of(std::time::UNIX_EPOCH + std::time::Duration::from_secs(86_400 + 3_661));
        assert_eq!(stamp, "1970-01-02_010101");
    }
}
