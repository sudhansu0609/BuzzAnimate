//! **Editing a Cast character's poses** (CAST_POSES_PLAN C2 e1-e3): add a pose
//! from a Library symbol, a picture or an FLA symbol, remove or rename one, write
//! a character edited on the stage back over its asset, and keep a symbol set
//! with the character when it is added to the Cast.
//!
//! Every write backs the asset up first (a copy under
//! `<root>/_cast_backup/<stamp>_<what>/<rel path>`); nothing is erased.

use std::path::{Path, PathBuf};

use buzz_doc::{Asset, AssetLibrary};
use buzz_scene::{ImportTarget, LayerId, ObjectId, Scene, SymbolId};

use crate::cast_combine::{main_symbol, pose_scene};

/// Where a new pose's artwork comes from.
pub enum PoseInput<'a> {
    /// A symbol of this document's Library, by name.
    LibrarySymbol(&'a Scene, &'a str),
    /// A rig already built (a picture or an FLA symbol): its stage is the pose.
    Rig(Scene),
}

fn root_of(library: &AssetLibrary) -> Result<PathBuf, String> {
    library.root().map(Path::to_path_buf).ok_or_else(|| "there is no assets folder on this machine".to_string())
}

/// Copy `asset`'s file to `<root>/_cast_backup/<stamp>_<tag>/<rel path>`.
pub fn backup_asset(root: &Path, asset: &Asset, stamp: &str, tag: &str) -> Result<PathBuf, String> {
    let base = root.join(crate::cast_fresh::BACKUP_FOLDER).join(format!("{stamp}_{tag}"));
    let rel = asset
        .path
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| PathBuf::from(&asset.folder).join(asset.path.file_name().unwrap_or_default()));
    let to = base.join(rel);
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not make the backup folder: {e}"))?;
    }
    std::fs::copy(&asset.path, &to).map_err(|e| format!("could not back up {}: {e}", asset.name))?;
    Ok(base)
}

/// The asset's scene as a combined character: its own when it is one, else the
/// single pose wrapped in a main symbol named after the asset.
fn combined_of(library: &AssetLibrary, asset: &Asset) -> Result<(Scene, SymbolId), String> {
    let scene = library.load(asset).map_err(|e| format!("could not read {}: {e}", asset.name))?;
    match main_symbol(&scene) {
        Some(main) => Ok((scene, main)),
        None => pose_scene(&scene, &asset.name),
    }
}

fn name_of(scene: &Scene, id: SymbolId) -> String {
    scene.library().get(id).map(|s| s.name.clone()).unwrap_or_default()
}

fn write_back(library: &mut AssetLibrary, asset: &Asset, scene: &Scene, stamp: &str, tag: &str) -> Result<PathBuf, String> {
    let root = root_of(library)?;
    let backup = backup_asset(&root, asset, stamp, tag)?;
    buzz_doc::format::save(scene, &asset.path).map_err(|e| format!("could not save {}: {e}", asset.name))?;
    library.rescan();
    Ok(backup)
}

/// The poses of a scene's combined character, main first (empty when it is one pose).
pub fn pose_names(scene: &Scene) -> Vec<String> {
    match main_symbol(scene) {
        Some(main) => scene.library().symbol_set(main).into_iter().map(|id| name_of(scene, id)).collect(),
        None => Vec::new(),
    }
}

/// **Add a pose** `<Character> <label>` to `asset`. A character that is a single
/// pose becomes a combined one with the new pose in its set. Returns the pose's
/// full name.
pub fn add_pose(library: &mut AssetLibrary, asset: &Asset, label: &str, input: PoseInput<'_>, stamp: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("the pose needs a name".into());
    }
    let (mut combined, main) = combined_of(library, asset)?;
    let full = format!("{} {label}", name_of(&combined, main));
    if combined
        .library()
        .symbol_set(main)
        .iter()
        .any(|id| name_of(&combined, *id).eq_ignore_ascii_case(&full))
    {
        return Err(format!("{} already has a pose called {full}", asset.name));
    }
    let before: Vec<SymbolId> = combined.library().iter().map(|s| s.id).collect();
    let (wanted, held) = match input {
        PoseInput::LibrarySymbol(doc, name) => {
            let id = doc.library().find_by_name(name).map(|s| s.id).ok_or_else(|| format!("no symbol called {name} in the Library"))?;
            let held = doc.extract_symbol(id).ok_or_else(|| format!("could not take {name} from the Library"))?;
            (name.to_string(), held)
        }
        PoseInput::Rig(rig) => {
            let (held, _) = pose_scene(&rig, &full)?;
            (full.clone(), held)
        }
    };
    let report = combined.merge(&held, ImportTarget::Library);
    let given = report.renamed.iter().find(|(w, _)| *w == wanted).map(|(_, g)| g.clone()).unwrap_or(wanted);
    let new_id = combined
        .library()
        .iter()
        .filter(|s| !before.contains(&s.id))
        .find(|s| s.name == given)
        .map(|s| s.id)
        .ok_or_else(|| format!("{label}: not found after the merge"))?;
    // Named `<Character> <label>` so the Pose dropdown reads well.
    let unique = if combined.library().find_by_name(&full).is_none() { full.clone() } else { combined.library().unique_name(&full) };
    combined.library_mut().update(new_id, |s| s.name = unique.clone());
    if !combined.library_mut().include_in_set(main, new_id) {
        return Err(format!("{label}: could not join {}'s set", asset.name));
    }
    write_back(library, asset, &combined, stamp, "pose")?;
    Ok(unique)
}

/// **Remove a pose** (its full name). The main pose cannot be removed.
pub fn remove_pose(library: &mut AssetLibrary, asset: &Asset, pose: &str, stamp: &str) -> Result<(), String> {
    let scene = library.load(asset).map_err(|e| format!("could not read {}: {e}", asset.name))?;
    let main = main_symbol(&scene).ok_or_else(|| format!("{} has only one pose", asset.name))?;
    let id = scene
        .library()
        .symbol_set(main)
        .into_iter()
        .find(|id| name_of(&scene, *id) == pose)
        .ok_or_else(|| format!("{} has no pose called {pose}", asset.name))?;
    if id == main {
        return Err("the main pose cannot be removed".into());
    }
    let mut scene = scene;
    scene.library_mut().remove_from_set(main, id);
    scene.library_mut().remove(id);
    // Whatever only that pose used goes with it.
    let layer = scene.stage_layers().iter().next().map(|l| l.name.clone()).unwrap_or_else(|| asset.name.clone());
    let kept = crate::asset_index::symbol_scene(&scene, main, &layer).ok_or("could not rebuild the character")?;
    write_back(library, asset, &kept, stamp, "remove_pose")?;
    Ok(())
}

/// **Rename a pose** (full name) to `<Character> <label>`.
pub fn rename_pose(library: &mut AssetLibrary, asset: &Asset, pose: &str, label: &str, stamp: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("the pose needs a name".into());
    }
    let mut scene = library.load(asset).map_err(|e| format!("could not read {}: {e}", asset.name))?;
    let main = main_symbol(&scene).ok_or_else(|| format!("{} has only one pose", asset.name))?;
    let full = format!("{} {label}", name_of(&scene, main));
    let set = scene.library().symbol_set(main);
    let id = set
        .iter()
        .copied()
        .find(|id| name_of(&scene, *id) == pose)
        .ok_or_else(|| format!("{} has no pose called {pose}", asset.name))?;
    if id == main {
        return Err("the main pose is named after the character; rename the character instead".into());
    }
    if set.iter().any(|other| *other != id && name_of(&scene, *other).eq_ignore_ascii_case(&full)) {
        return Err(format!("{} already has a pose called {full}", asset.name));
    }
    scene.library_mut().update(id, |s| s.name = full.clone());
    write_back(library, asset, &scene, stamp, "rename_pose")?;
    Ok(full)
}

/// **A rig from a picture** through the cast build's picture route (judge the
/// pose, trace the parts, rig by layers), in a scratch folder.
pub fn rig_from_picture(picture: &Path, scratch: &Path, say: &mut dyn FnMut(&str)) -> Result<Scene, String> {
    let input = scratch.join("in");
    std::fs::create_dir_all(&input).map_err(|e| e.to_string())?;
    std::fs::copy(picture, input.join("Pose picture.png")).map_err(|e| format!("could not read {}: {e}", picture.display()))?;
    let lib_dir = scratch.join("assets");
    std::fs::create_dir_all(&lib_dir).map_err(|e| e.to_string())?;
    let library = AssetLibrary::at(&lib_dir);
    let request = crate::cast_build::CastRequest {
        characters_dir: Some(input),
        wanted: Vec::new(),
        style: crate::backdrop::Style::default(),
        replace: true,
        comfy: None,
        out: scratch.join("out"),
    };
    let report = crate::cast_build::build_cast(&library, &request, say);
    let Some((_, rig, _)) = report.built.first() else {
        let why = report.skipped.first().map(|(_, l)| l.clone()).unwrap_or_else(|| "nothing was built".into());
        return Err(format!("the picture could not be rigged: {why}"));
    };
    let library = AssetLibrary::at(&lib_dir);
    let asset = library.assets().iter().find(|a| a.name == *rig).cloned().ok_or("the built rig is not in the scratch library")?;
    library.load(&asset).map_err(|e| format!("could not read the built rig: {e}"))
}

/// **A rig from one FLA symbol**, the cast FLA importer's way (cut to one frame, layer-rigged).
pub fn rig_from_fla(fla: &Path, symbol: &str) -> Result<Scene, String> {
    let imported = crate::import::read(fla)?;
    let id = imported
        .scene
        .library()
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(symbol.trim()))
        .map(|s| (s.id, s.name.clone(), s.layers.iter().count()))
        .ok_or_else(|| format!("no symbol called {symbol} in {}", fla.display()))?;
    let piece = crate::cast_fla::Piece {
        symbol: id.0,
        name: id.1,
        view: crate::cast_fla::View::Front,
        walk: false,
        layers: id.2,
    };
    let (scene, _) = crate::cast_fla::build_scene(&imported.scene, &piece, symbol.trim(), true, false)?;
    Ok(scene)
}

/// **Add to Cast carries the set** (e3): `Scene::extract` of the selection, plus
/// every member of the set of each symbol it holds (with what they use), so a
/// character with poses keeps them.
pub fn extract_with_sets(scene: &Scene, frame: u32, ids: &[ObjectId]) -> Scene {
    let mut out = scene.extract(frame, ids);
    for _ in 0..8 {
        let missing: Vec<SymbolId> = out
            .library()
            .iter()
            .flat_map(|s| s.set.iter().copied())
            .filter(|m| out.library().get(*m).is_none())
            .collect();
        if missing.is_empty() {
            break;
        }
        for m in missing {
            if let Some(held) = scene.extract_symbol(m) {
                for s in held.library().iter() {
                    if out.library().get(s.id).is_none() {
                        out.library_mut().insert((**s).clone());
                    }
                }
            }
        }
    }
    out
}

/// **Save changes to <name>** (e2): the layers of `doc` that came from the Cast
/// asset `key` (`Cast/<Series>/<Name>`) are written back over that asset, with
/// their symbols (a set travels with its owner). Returns how many layers were saved.
pub fn save_changes(library: &mut AssetLibrary, doc: &Scene, asset: &Asset, stamp: &str) -> Result<usize, String> {
    let key = format!("{}/{}", asset.folder, asset.name);
    let layers: Vec<LayerId> = doc
        .stage_layers()
        .iter()
        .filter(|l| l.cast_source.as_deref() == Some(key.as_str()))
        .map(|l| l.id)
        .collect();
    if layers.is_empty() {
        return Err(format!("nothing on the stage came from {}", asset.name));
    }
    let mut scene = crate::asset_index::layers_scene(doc, &layers);
    for id in &layers {
        scene.update_layer(*id, |l| l.cast_source = None);
    }
    write_back(library, asset, &scene, stamp, "save")?;
    Ok(layers.len())
}

/// The Cast asset the stage selection was placed from (its layer's `cast_source`).
pub fn source_of(doc: &Scene, library: &AssetLibrary, selected: Option<ObjectId>) -> Option<Asset> {
    let (layer, _) = doc.find_object(selected?)?;
    let l = doc.layers().get(layer)?;
    let key = l.cast_source.as_deref().filter(|k| !k.contains('|'))?;
    // A lifted (produced) character is edited through its pose symbols, not saved back.
    if l.kind == buzz_scene::LayerKind::Folder {
        return None;
    }
    library.assets().iter().find(|a| format!("{}/{}", a.folder, a.name) == key).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape as _;
    use buzz_scene::{LayerKind, SymbolKind};

    /// A Cast asset with one layer "Body" holding one shape.
    fn saved(library: &mut AssetLibrary, name: &str) -> Asset {
        let mut scene = Scene::default();
        let layer = scene.add_stage_layer("Body", LayerKind::Normal);
        scene.add_shape(layer, buzz_scene::ShapeData::filled(kurbo::Rect::new(0.0, 0.0, 10.0, 30.0).to_path(1e-9), peniko::Color::WHITE));
        library.save(name, "Cast/Series", &scene).expect("saved")
    }

    /// e1 (library-symbol source): a single-pose character gets a pose from a
    /// symbol of the open document; the asset becomes a combined character whose
    /// set names it `<Character> <label>`; the old file is backed up; rename and
    /// remove work and the main pose cannot be removed.
    #[test]
    fn add_rename_and_remove_a_pose_from_a_library_symbol() {
        let dir = tempfile::tempdir().expect("temp asset folder");
        let mut library = AssetLibrary::at(dir.path());
        let asset = saved(&mut library, "Raju");

        let mut doc = Scene::default();
        let sitting = doc.add_symbol("Sitting rig", SymbolKind::Graphic, None);
        let first = doc.library().get(sitting).unwrap().layers.iter().next().unwrap().id;
        doc.library_mut().update(sitting, |s| {
            s.layers.update(first, |l| {
                let id = buzz_scene::ObjectId(900);
                l.frames.set_objects(0, vec![std::sync::Arc::new(buzz_scene::Object::shape(id, buzz_scene::ShapeData::filled(kurbo::Rect::new(0.0, 0.0, 40.0, 20.0).to_path(1e-9), peniko::Color::BLACK)))]);
            });
        });

        let full = add_pose(&mut library, &asset, "Sitting", PoseInput::LibrarySymbol(&doc, "Sitting rig"), "2026-11-03_010101").expect("added");
        assert_eq!(full, "Raju Sitting");
        let scene = library.load(&library.assets().iter().find(|a| a.name == "Raju").cloned().unwrap()).unwrap();
        assert_eq!(pose_names(&scene), vec!["Raju".to_string(), "Raju Sitting".to_string()]);
        let backup = dir.path().join("_cast_backup").join("2026-11-03_010101_pose").join("Cast").join("Series").join("Raju.buzz");
        assert!(backup.exists(), "the old asset is backed up");

        let asset = library.assets().iter().find(|a| a.name == "Raju").cloned().unwrap();
        assert!(add_pose(&mut library, &asset, "Sitting", PoseInput::LibrarySymbol(&doc, "Sitting rig"), "2026-11-03_010102").is_err(), "no duplicate pose");
        assert_eq!(rename_pose(&mut library, &asset, "Raju Sitting", "Seated", "2026-11-03_010103").unwrap(), "Raju Seated");
        assert!(remove_pose(&mut library, &asset, "Raju", "2026-11-03_010104").is_err(), "the main pose stays");
        remove_pose(&mut library, &asset, "Raju Seated", "2026-11-03_010105").expect("removed");
        let scene = library.load(&library.assets().iter().find(|a| a.name == "Raju").cloned().unwrap()).unwrap();
        assert!(pose_names(&scene).is_empty(), "one pose left: no set");
        assert!(scene.library().iter().all(|s| s.name != "Raju Seated"), "its symbol went with it");
    }

    /// e2 + e3: a Cast character placed on the stage remembers its asset; after
    /// the user adds a pose to its set (Include...) "Save changes" writes it back
    /// over the asset, backup first, and the set comes with it; Add to Cast of a
    /// selection carries the set.
    #[test]
    fn a_placed_character_saves_back_with_its_set_and_add_to_cast_carries_the_set() {
        let dir = tempfile::tempdir().expect("temp asset folder");
        let mut library = AssetLibrary::at(dir.path());
        let asset = saved(&mut library, "Raju");
        // Make it combined (one extra pose) so it is a set owner.
        let mut doc = Scene::default();
        let alt = doc.add_symbol("Alt", SymbolKind::Graphic, None);
        add_pose(&mut library, &asset, "Alt", PoseInput::LibrarySymbol(&doc, "Alt"), "2026-11-03_020201").expect("added");
        let asset = library.assets().iter().find(|a| a.name == "Raju").cloned().unwrap();
        let _ = alt;

        // Place it the way the Cast panel does, remembering the source.
        let mut stage = Scene::default();
        library.place(&asset, &mut stage).expect("places");
        let key = format!("{}/{}", asset.folder, asset.name);
        let placed: Vec<LayerId> = stage.stage_layers().iter().map(|l| l.id).collect();
        for id in &placed {
            stage.update_layer(*id, |l| l.cast_source = Some(key.clone()));
        }
        let main = main_symbol(&stage).expect("a set owner on the stage");
        let selected = stage.stage_layers().iter().find_map(|l| l.all_objects().next().map(|o| o.id)).unwrap();
        assert_eq!(source_of(&stage, &library, Some(selected)).map(|a| a.name), Some("Raju".to_string()));

        // The user adds a third pose to the set on the stage and saves.
        let third = stage.add_symbol("Raju Third", SymbolKind::Graphic, None);
        assert!(stage.library_mut().include_in_set(main, third));
        let carried = extract_with_sets(&stage, 0, &[selected]);
        assert!(main_symbol(&carried).is_some() && carried.library().symbol_set(main).len() == 3, "Add to Cast keeps the whole set");

        let n = save_changes(&mut library, &stage, &asset, "2026-11-03_020202").expect("saved back");
        assert_eq!(n, placed.len());
        let after = library.load(&library.assets().iter().find(|a| a.name == "Raju").cloned().unwrap()).unwrap();
        assert_eq!(pose_names(&after).len(), 3, "{:?}", pose_names(&after));
        let backup = dir.path().join("_cast_backup").join("2026-11-03_020202_save").join("Cast").join("Series").join("Raju.buzz");
        assert!(backup.exists(), "the asset as it was is backed up");
    }
}
