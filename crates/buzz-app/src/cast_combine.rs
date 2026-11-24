//! **One Cast character, all its poses** (CAST_POSES_PLAN C2 a/b): the
//! separate assets of a role (`DEEPAK`, `DEEPAK Side`, `DEEPAK Back`,
//! `DEEPAK 3-4`, `DEEPAK Pose walk_front`, ...) merged into one asset `DEEPAK`.
//!
//! The combined asset's library holds one symbol per pose, each holding that
//! pose's layer rig; the main symbol (named after the character, the Front
//! pose) owns a symbol *set* listing every other pose symbol (named
//! `<Role> <Pose>`), and the asset's stage is one layer holding one instance
//! of the main symbol. Properties > Symbol Instance > Pose is the existing Set
//! control, so a pose is picked per frame with `Scene::pick_from_set`.
//!
//! Every file is backed up before it is touched: the front asset is copied and
//! the others are *moved* to `<root>/_cast_backup/<stamp>_combine/`.

use std::path::{Path, PathBuf};

use std::sync::Arc;

use buzz_doc::{Asset, AssetLibrary};
use buzz_geom::{Affine, Point, Rect};
use buzz_scene::{ObjectKind, Scene, SymbolId, SymbolKind};

/// `(role, pose symbol name)` of a separate asset, or `None` for the role's own
/// (front) asset or an unrelated name.
pub fn pose_of(name: &str) -> Option<(String, String)> {
    for (suffix, pose) in [(" 3-4", "3-4"), (" Back", "Back"), (" Side", "Side")] {
        if let Some(role) = name.strip_suffix(suffix) {
            return Some((role.to_string(), format!("{role} {pose}")));
        }
    }
    let (role, pose) = crate::cast_build::pose_of_rig(name)?;
    let words: Vec<String> = pose
        .split(['_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect();
    Some((role.to_string(), format!("{role} {}", words.join(" "))))
}

/// Whether `scene` is already a combined character: its stage instance's
/// symbol owns a set.
pub fn is_combined(scene: &Scene) -> bool {
    main_symbol(scene).is_some()
}

/// The symbol (and so the stage instance) of a combined character.
pub fn main_symbol(scene: &Scene) -> Option<SymbolId> {
    scene.stage_layers().iter().find_map(|l| {
        l.all_objects().find_map(|o| match &o.kind {
            ObjectKind::Instance(i) if !scene.library().symbol_set(i.symbol).is_empty() => Some(i.symbol),
            _ => None,
        })
    })
}

/// Every role with separate pose assets beside it: `(folder, role)`.
pub fn combinable_roles(library: &AssetLibrary) -> Vec<(String, String)> {
    let cast = crate::cast_fresh::cast_characters(library);
    let mut out: Vec<(String, String)> = Vec::new();
    for a in &cast {
        if let Some((role, _)) = pose_of(&a.name)
            && cast.iter().any(|b| b.folder == a.folder && b.name == role)
            && !out.iter().any(|(f, r)| *f == a.folder && *r == role)
        {
            out.push((a.folder.clone(), role));
        }
    }
    out.sort();
    out
}

/// A scene holding one symbol `name` whose layers are `scene`'s stage layers,
/// placed by one instance.
pub(crate) fn pose_scene(scene: &Scene, name: &str) -> Result<(Scene, SymbolId), String> {
    let mut s = scene.clone();
    let id = s.add_symbol(name, SymbolKind::Graphic, None);
    let layers = s.stage_layers().clone();
    *s.library_mut().layers_mut(id).ok_or("no symbol layers")? = layers;
    let held = crate::asset_index::symbol_scene(&s, id, name).ok_or("could not wrap the pose")?;
    let symbol = held
        .library()
        .iter()
        .find(|x| x.name == name)
        .map(|x| x.id)
        .ok_or("the pose symbol vanished")?;
    let mut held = held;
    unwrap_pose(&mut held, symbol);
    Ok((held, symbol))
}

/// **A pose that only wraps another drawing, made into that drawing.**
///
/// A character kept from a selection on the stage is saved as one instance of
/// its body symbol on one layer, so its main pose opened onto a single layer
/// and a single frame, while every other pose opened onto its parts. When
/// `pose`'s only artwork is one instance of a symbol on one keyframe, its
/// layers become that symbol's — every part layer and frame — carried by the
/// instance's placement so it looks the same. Empty layers (Master Audio) stay
/// on top. Returns whether it was unwrapped.
pub fn unwrap_pose(scene: &mut Scene, pose: SymbolId) -> bool {
    let Some((object, layers)) = wrapped(scene, pose) else {
        return false;
    };
    scene.library_mut().update(pose, |s| s.layers = layers);
    transform_pose(scene, pose, object.transform);
    true
}

/// The instance `pose` only wraps, and the layers it would have unwrapped:
/// see [`unwrap_pose`]. `None` when it is not a wrapper.
fn wrapped(scene: &Scene, pose: SymbolId) -> Option<(buzz_scene::Object, buzz_scene::LayerStack)> {
    let sym = scene.library().get(pose)?;
    let mut held: Option<buzz_scene::Object> = None;
    let mut kept: Vec<buzz_scene::Layer> = Vec::new();
    for layer in sym.layers.iter() {
        let objects: Vec<&buzz_scene::Object> = layer.all_objects().map(|o| &**o).collect();
        if objects.is_empty() {
            kept.push((**layer).clone());
            continue;
        }
        if held.is_some() || objects.len() != 1 || layer.frames.keyframe_count() != 1 {
            return None;
        }
        held = Some(objects[0].clone());
    }
    let object = held?;
    let ObjectKind::Instance(instance) = &object.kind else {
        return None;
    };
    let inner = scene.library().get(instance.symbol)?;
    if inner.id == pose || inner.layers.is_empty() || !inner.set.is_empty() {
        return None;
    }
    let mut layers = inner.layers.clone();
    for layer in kept.into_iter().rev() {
        if !layers.iter().any(|l| l.name == layer.name) {
            layers.insert(0, layer);
        }
    }
    Some((object, layers))
}

/// [`unwrap_pose`] on every character's main pose in `scene` (each symbol
/// that owns a pose set). How many were unwrapped.
pub fn unwrap_characters(scene: &mut Scene) -> usize {
    let owners: Vec<SymbolId> = scene.library().iter().filter(|s| !s.set.is_empty()).map(|s| s.id).collect();
    owners.into_iter().filter(|id| unwrap_pose(scene, *id)).count()
}

/// Whether any character's main pose in `scene` is a wrapper
/// [`unwrap_characters`] would unwrap — checked without editing.
pub fn has_wrapped_characters(scene: &Scene) -> bool {
    scene.library().iter().any(|s| !s.set.is_empty() && wrapped(scene, s.id).is_some())
}

/// **Unwrap every Cast character's main pose**, in the Cast files: each file
/// changed is copied to `_cast_backup/<stamp>_unwrap/` first. The names of
/// the characters fixed.
pub fn unwrap_cast(library: &mut AssetLibrary, stamp: &str) -> Vec<String> {
    let root = library.root().map(Path::to_path_buf).unwrap_or_default();
    let base = root.join(crate::cast_fresh::BACKUP_FOLDER).join(format!("{stamp}_unwrap"));
    let mut fixed = Vec::new();
    for asset in crate::cast_fresh::cast_characters(library) {
        let Ok(mut scene) = library.load(&asset) else {
            continue;
        };
        if unwrap_characters(&mut scene) == 0 {
            continue;
        }
        let rel = asset.path.strip_prefix(&root).map(Path::to_path_buf).unwrap_or_else(|_| PathBuf::from(asset.path.file_name().unwrap_or_default()));
        let copy_to = base.join(rel);
        if copy_to.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
            && std::fs::copy(&asset.path, &copy_to).is_ok()
            && buzz_doc::format::save(&scene, &asset.path).is_ok()
        {
            fixed.push(asset.name.clone());
        }
    }
    if !fixed.is_empty() {
        library.rescan();
    }
    fixed
}

/// The box a pose symbol's artwork fills at `frame` (followers carrying what
/// they inherit), the one the pose switch measures too.
pub fn pose_box(scene: &Scene, symbol: SymbolId, frame: u32) -> Option<Rect> {
    let sym = scene.library().get(symbol)?;
    let mut out: Option<Rect> = None;
    for l in sym.layers.iter() {
        let inherited = sym.layers.inherited_transform(l.id, frame);
        for o in l.frames.objects_at(frame) {
            if !o.visible {
                continue;
            }
            let r = buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(o));
            if r.width() <= 0.0 && r.height() <= 0.0 {
                continue;
            }
            out = Some(out.map_or(r, |u| u.union(r)));
        }
    }
    out
}

/// Move and scale everything a pose symbol holds by `m` (every layer, every
/// key, the recorded rest poses): followers inherit motion, so all of them must
/// be carried, not only the roots.
fn transform_pose(scene: &mut Scene, symbol: SymbolId, m: Affine) {
    let Some(layers) = scene.library_mut().layers_mut(symbol) else {
        return;
    };
    let ids: Vec<buzz_scene::LayerId> = layers.iter().map(|l| l.id).collect();
    for id in ids {
        layers.update(id, |layer| {
            let keys: Vec<u32> = layer.frames.keyframes().iter().map(|k| k.start).collect();
            for start in keys {
                let moved: Vec<Arc<buzz_scene::Object>> = layer
                    .frames
                    .keyframes()
                    .iter()
                    .find(|k| k.start == start)
                    .map(|k| {
                        k.objects
                            .iter()
                            .map(|o| {
                                let mut c = (**o).clone();
                                c.transform = m * c.transform;
                                Arc::new(c)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                layer.frames.set_objects(start, moved);
            }
            if let Some(rest) = layer.rest_pose {
                layer.rest_pose = Some(m * rest);
            }
        });
    }
}

/// **Make every pose of a combined character as tall as its main pose, standing
/// on the same ground point** (the main pose's feet: bottom centre of its box).
/// Returns `(pose name, scale applied)` for each pose touched; a pose already
/// at that size and place is left alone, so running it again changes nothing.
pub fn normalise_poses(scene: &mut Scene, main: SymbolId) -> Vec<(String, f64)> {
    let Some(target) = pose_box(scene, main, 0) else {
        return Vec::new();
    };
    let feet = Point::new(target.center().x, target.y1);
    let mut done = Vec::new();
    for pose in scene.library().symbol_set(main) {
        if pose == main {
            continue;
        }
        let Some(b) = pose_box(scene, pose, 0) else {
            continue;
        };
        if b.height() < 1e-6 {
            continue;
        }
        let scale = target.height() / b.height();
        let there = Point::new(b.center().x, b.y1);
        if (scale - 1.0).abs() < 0.005 && (there - feet).hypot() < 0.5 {
            continue;
        }
        let m = Affine::translate(feet.to_vec2()) * Affine::scale(scale) * Affine::translate((-there.x, -there.y));
        transform_pose(scene, pose, m);
        done.push((scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default(), scale));
    }
    done
}

/// What a combine did to one role.
#[derive(Debug, Clone)]
pub struct CombineReport {
    pub role: String,
    pub poses: Vec<String>,
    pub backup: PathBuf,
}

/// **Combine the poses of `role`** in library `folder` into one asset. Nothing is
/// written if any pose cannot be read; the separate assets are moved, not erased.
pub fn combine_role(library: &mut AssetLibrary, folder: &str, role: &str, stamp: &str) -> Result<CombineReport, String> {
    let root = library.root().map(Path::to_path_buf).ok_or("there is no assets folder on this machine")?;
    let cast = crate::cast_fresh::cast_characters(library);
    let main = cast
        .iter()
        .find(|a| a.folder == folder && a.name == role)
        .cloned()
        .ok_or_else(|| format!("no asset called '{role}' in {folder}"))?;
    let mut parts: Vec<(Asset, String)> = cast
        .iter()
        .filter(|a| a.folder == folder)
        .filter_map(|a| pose_of(&a.name).filter(|(r, _)| r == role).map(|(_, p)| (a.clone(), p)))
        .collect();
    parts.sort_by(|a, b| a.1.cmp(&b.1));
    if parts.is_empty() {
        return Err(format!("{role} has no separate poses to combine"));
    }
    let front_scene = library.load(&main).map_err(|e| format!("could not read {role}: {e}"))?;
    if is_combined(&front_scene) {
        return Err(format!("{role} is already combined"));
    }
    let mut loaded = Vec::new();
    for (asset, pose) in &parts {
        let scene = library.load(asset).map_err(|e| format!("could not read {}: {e}", asset.name))?;
        loaded.push((scene, pose.clone()));
    }

    let (mut combined, main_id) = pose_scene(&front_scene, role)?;
    let mut names = vec![format!("{role} Front")];
    for (scene, pose) in &loaded {
        let (held, _) = pose_scene(scene, pose)?;
        let before: Vec<SymbolId> = combined.library().iter().map(|s| s.id).collect();
        combined.merge(&held, buzz_scene::ImportTarget::Library);
        let new_id = combined
            .library()
            .iter()
            .filter(|s| !before.contains(&s.id))
            .find(|s| s.name == *pose || s.name.starts_with(&format!("{pose}_")))
            .map(|s| s.id)
            .ok_or_else(|| format!("{pose}: not found after the merge"))?;
        if !combined.library_mut().include_in_set(main_id, new_id) {
            return Err(format!("{pose}: could not join {role}'s set"));
        }
        names.push(pose.clone());
    }

    normalise_poses(&mut combined, main_id);

    // Back up first: the front asset is copied, the others are moved.
    let base = root.join(crate::cast_fresh::BACKUP_FOLDER).join(format!("{stamp}_combine"));
    let rel = |a: &Asset| {
        a.path
            .strip_prefix(&root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| PathBuf::from(&a.folder).join(a.path.file_name().unwrap_or_default()))
    };
    let copy_to = base.join(rel(&main));
    if let Some(dir) = copy_to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not make the backup folder: {e}"))?;
    }
    std::fs::copy(&main.path, &copy_to).map_err(|e| format!("could not back up {role}: {e}"))?;
    buzz_doc::format::save(&combined, &main.path).map_err(|e| format!("could not save {role}: {e}"))?;
    for (asset, _) in &parts {
        crate::cast_fresh::move_file(&asset.path, &base.join(rel(asset)))?;
    }
    library.rescan();
    Ok(CombineReport {
        role: role.to_string(),
        poses: names,
        backup: base,
    })
}

/// Combine every role in `only` (all combinable roles when `None`; names are
/// matched without case). Returns what was combined and what failed.
pub fn combine_many(library: &mut AssetLibrary, only: Option<&[String]>, stamp: &str) -> (Vec<CombineReport>, Vec<String>) {
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for (folder, role) in combinable_roles(library) {
        if let Some(only) = only
            && !only.iter().any(|n| n.trim().eq_ignore_ascii_case(&role))
        {
            continue;
        }
        match combine_role(library, &folder, &role, stamp) {
            Ok(r) => done.push(r),
            Err(e) => failed.push(format!("{role}: {e}")),
        }
    }
    (done, failed)
}

/// Whether `rig` (a separate pose asset's name, `<X> Side`, `<X> Pose walk_front`...)
/// already lives as a pose of the combined character `<X>` in `library`.
pub fn has_pose(library: &AssetLibrary, rig: &str) -> bool {
    let Some((role, pose)) = pose_of(rig) else {
        return false;
    };
    let cast = crate::cast_fresh::cast_characters(library);
    let Some(asset) = cast.iter().find(|a| a.name == role) else {
        return false;
    };
    library.load(asset).ok().is_some_and(|scene| {
        main_symbol(&scene).is_some_and(|m| {
            scene
                .library()
                .symbol_set(m)
                .iter()
                .any(|id| scene.library().get(*id).is_some_and(|s| s.name == pose))
        })
    })
}

/// What a right-click Pose menu acts on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PoseTarget {
    /// An instance whose symbol sits in a set (a combined character placed as one instance).
    Instance(buzz_scene::ObjectId),
    /// A character unpacked onto the stage: its layer folder.
    Lifted(buzz_scene::LayerId),
}

/// The poses to offer for a right-clicked selection: `(target, pose shown at
/// `frame`, [(pose symbol, name)])`, or `None` when it is not a character with poses.
pub fn pose_choices(scene: &Scene, selected: Option<buzz_scene::ObjectId>, frame: u32) -> Option<(PoseTarget, String, Vec<(SymbolId, String)>)> {
    let (layer, object) = scene.find_object(selected?)?;
    if let Some(folder) = buzz_act::pose::character_folder_of(scene, layer) {
        let poses = buzz_act::pose::poses_of(scene, folder);
        let shown = buzz_act::pose::shown_pose(scene, folder, frame).unwrap_or_default();
        return (poses.len() > 1).then_some((PoseTarget::Lifted(folder), shown, poses));
    }
    let here = scene.layers().get(layer)?.frames.objects_at(frame).iter().find(|o| o.id == object.id).cloned();
    let shown = here.as_ref().unwrap_or(object).instance()?.symbol;
    let mut shown_name = scene.library().get(shown)?.name.clone();
    let mut set = scene.library().symbol_set(shown);
    // **A pose copy outside the set** (`DeepakFront DeepakBack 1`, made by a
    // Duplicate Symbol or a re-import) is still that character: offer the set
    // its original belongs to, shown as that original.
    if set.is_empty()
        && let Some((owner_set, original)) = stray_pose_set(scene, layer, shown)
    {
        set = owner_set;
        shown_name = original;
    }
    let poses: Vec<(SymbolId, String)> = set
        .into_iter()
        .filter_map(|id| scene.library().get(id).map(|s| (id, s.name.clone())))
        .collect();
    (poses.len() > 1).then_some((PoseTarget::Instance(object.id), shown_name, poses))
}

/// A name without the ` 2`, ` 3`… a duplicate symbol is given.
pub(crate) fn without_copy_number(name: &str) -> &str {
    let mut name = name.trim_end();
    while let Some((head, tail)) = name.rsplit_once(' ')
        && !tail.is_empty()
        && tail.bytes().all(|b| b.is_ascii_digit())
    {
        name = head.trim_end();
    }
    name
}

/// The pose set a symbol in no set belongs to, and the name of the pose it is
/// a copy of: the set holding a pose of the same name less its copy number,
/// else — on a layer brought in from the Cast — that character's own set.
fn stray_pose_set(scene: &Scene, layer: buzz_scene::LayerId, stray: SymbolId) -> Option<(Vec<SymbolId>, String)> {
    let library = scene.library();
    let name = library.get(stray)?.name.clone();
    let base = without_copy_number(&name);
    let owners = || library.iter().filter(|s| !s.set.is_empty());
    for owner in owners() {
        let set = library.symbol_set(owner.id);
        if let Some(original) = set
            .iter()
            .filter_map(|id| library.get(*id))
            .find(|s| s.id != stray && without_copy_number(&s.name) == base)
        {
            return Some((set, original.name.clone()));
        }
    }
    let source = scene.layers().get(layer)?.cast_source.clone()?;
    let character = source.rsplit('/').next()?.split('|').next()?.to_string();
    let owner = owners().find(|s| without_copy_number(&s.name) == character)?;
    Some((library.symbol_set(owner.id), name))
}

/// The symbol a selected instance shows at `frame`, for linking more poses to
/// it (right-click ▸ Pose ▸ Add Pose). `None` for anything that is not a
/// plain instance — a character unpacked onto the stage gets its poses from
/// the Cast instead.
pub fn pose_symbol(scene: &Scene, selected: Option<buzz_scene::ObjectId>, frame: u32) -> Option<SymbolId> {
    let (layer, object) = scene.find_object(selected?)?;
    if buzz_act::pose::character_folder_of(scene, layer).is_some() {
        return None;
    }
    let here = scene.layers().get(layer)?.frames.objects_at(frame).iter().find(|o| o.id == object.id).cloned();
    Some(here.as_ref().unwrap_or(object).instance()?.symbol)
}

/// What right-click ▸ Pose ▸ Add Pose offers for `shown`: every symbol
/// [`buzz_scene::Library::include_in_set`] would accept, the ones Swap… ranks
/// as similar first (`similar`), then the rest by name.
pub fn addable_poses(scene: &Scene, shown: SymbolId) -> (Vec<(SymbolId, String)>, Vec<(SymbolId, String)>) {
    let library = scene.library();
    let owner = library.symbol_set(shown).first().copied().unwrap_or(shown);
    let in_some_set: std::collections::HashSet<SymbolId> = library.iter().flat_map(|s| s.set.iter().copied()).collect();
    let addable = |id: SymbolId| {
        id != owner && library.get(id).is_some_and(|s| s.set.is_empty()) && !in_some_set.contains(&id)
    };
    let similar: Vec<(SymbolId, String)> = library
        .similar_symbols(shown, &scene.cached_bounds_table())
        .into_iter()
        .filter(|(id, _)| addable(*id))
        .filter_map(|(id, _)| library.get(id).map(|s| (id, s.name.clone())))
        .collect();
    let mut rest: Vec<(SymbolId, String)> = library
        .iter()
        .filter(|s| addable(s.id) && !similar.iter().any(|(id, _)| *id == s.id))
        .map(|s| (s.id, s.name.clone()))
        .collect();
    rest.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    (similar, rest)
}

/// Turn the target to `pose` from `frame` on (one scene edit).
pub fn pick_pose(scene: &mut Scene, target: PoseTarget, frame: u32, pose: SymbolId) -> bool {
    match target {
        PoseTarget::Instance(id) => {
            if scene.pick_from_set(frame, id, pose, buzz_scene::SetStart::FromStart) {
                return true;
            }
            // A pose copy outside the set (see `pose_choices`) is switched the
            // same way — keyed at `frame`, from the new pose's first frame —
            // into the set it was offered.
            let Some((layer, object)) = scene.find_object(id) else {
                return false;
            };
            let here = scene.layers().get(layer).and_then(|l| l.frames.objects_at(frame).iter().find(|o| o.id == id).cloned());
            let Some(shown) = here.as_ref().unwrap_or(object).instance().map(|i| i.symbol) else {
                return false;
            };
            let offered = scene.library().symbol_set(shown).is_empty()
                && stray_pose_set(scene, layer, shown).is_some_and(|(set, _)| set.contains(&pose));
            if !offered || shown == pose {
                return false;
            }
            scene.ensure_keyframe_for(frame, id);
            scene.update_object_at(frame, id, |object| {
                if let buzz_scene::ObjectKind::Instance(instance) = &mut object.kind {
                    instance.symbol = pose;
                    instance.first_frame = 0;
                }
            })
        }
        PoseTarget::Lifted(folder) => buzz_act::pose::switch_lifted_pose(scene, folder, frame, pose).is_ok_and(|r| r.changed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A main pose that only wraps its body symbol opens onto the body's
    /// layers and frames once unwrapped, in the same place.
    #[test]
    fn a_wrapped_main_pose_is_unwrapped_into_its_parts_in_place() {
        let mut scene = Scene::default();
        let body = scene.add_symbol("Body", SymbolKind::Graphic, None);
        if let Some(layers) = scene.library_mut().layers_mut(body) {
            let first = layers.iter().next().map(|l| l.id).unwrap();
            layers.update(first, |l| l.name = "Arm".into());
            layers.push_front(buzz_scene::Layer::normal(buzz_scene::LayerId(9001), "Head"));
        }
        let main = scene.add_symbol("Ana", SymbolKind::Graphic, None);
        let back = scene.add_symbol("Ana Back", SymbolKind::Graphic, None);
        assert!(scene.library_mut().include_in_set(main, back));
        let placed = buzz_scene::Object::instance_of(buzz_scene::ObjectId(9002), body).with_transform(Affine::scale(0.5));
        if let Some(layers) = scene.library_mut().layers_mut(main) {
            let first = layers.iter().next().map(|l| l.id).unwrap();
            layers.update(first, |l| {
                l.push_object_at(0, Arc::new(placed.clone()));
            });
        }
        assert!(has_wrapped_characters(&scene));
        assert_eq!(unwrap_characters(&mut scene), 1);
        let names: Vec<String> = scene.library().get(main).unwrap().layers.iter().map(|l| l.name.clone()).collect();
        assert!(names.contains(&"Arm".to_string()) && names.contains(&"Head".to_string()), "{names:?}");
        assert!(!has_wrapped_characters(&scene), "nothing left to unwrap");
    }

    /// A duplicated pose (`Ana Back 1`) sits in no set; right-click ▸ Pose
    /// still lists the character's poses and switches to them.
    #[test]
    fn a_pose_copy_outside_the_set_still_offers_and_switches_poses() {
        let mut scene = Scene::default();
        scene.set_frame_count(20);
        let front = scene.add_symbol("Ana", buzz_scene::SymbolKind::Graphic, None);
        let back = scene.add_symbol("Ana Back", buzz_scene::SymbolKind::Graphic, None);
        let side = scene.add_symbol("Ana Side", buzz_scene::SymbolKind::Graphic, None);
        assert!(scene.library_mut().include_in_set(front, back));
        assert!(scene.library_mut().include_in_set(front, side));
        let copy = scene.add_symbol("Ana Back 1", buzz_scene::SymbolKind::Graphic, None);
        let layer = scene.add_stage_layer("Ana", buzz_scene::LayerKind::Normal);
        let id = scene
            .add_instance_at(layer, 0, copy, buzz_geom::Affine::IDENTITY)
            .expect("placed");
        let (_, shown, poses) = pose_choices(&scene, Some(id), 0).expect("offers the set");
        assert_eq!(shown, "Ana Back");
        assert_eq!(poses.len(), 3);
        let (target, ..) = pose_choices(&scene, Some(id), 0).unwrap();
        assert!(pick_pose(&mut scene, target, 0, side));
        assert_eq!(pose_choices(&scene, Some(id), 0).unwrap().1, "Ana Side");
    }

    /// The gate: three separate assets of one role become one asset whose set
    /// lists every pose; the separate files are moved to the backup folder;
    /// placing it and picking Back at frame 10 and Side at frame 20 shows
    /// Front on 0-9, Back on 10-19 and Side from 20.
    #[test]
    fn combining_poses_makes_one_character_you_can_switch_per_frame() {
        let dir = tempfile::tempdir().expect("temp asset folder");
        let mut library = AssetLibrary::at(dir.path());
        for name in ["Raju", "Raju Side", "Raju Back", "Raju Pose walk_front", "Other"] {
            let mut scene = Scene::default();
            scene.add_stage_layer("Body", buzz_scene::LayerKind::Normal);
            library.save(name, "Cast/Series", &scene).expect("saved");
        }
        let (done, failed) = combine_many(&mut library, None, "2026-11-03_000000");
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].poses, vec!["Raju Front", "Raju 3-4", "Raju Back", "Raju Side", "Raju Walk Front"].into_iter().filter(|p| done[0].poses.contains(&p.to_string())).map(String::from).collect::<Vec<_>>());
        assert_eq!(done[0].poses.len(), 4, "{:?}", done[0].poses);

        let names: Vec<String> = library.assets().iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, vec!["Other".to_string(), "Raju".to_string()], "only the combined asset and the unrelated one are listed");
        let moved = dir.path().join("_cast_backup").join("2026-11-03_000000_combine").join("Cast").join("Series");
        for name in ["Raju Side", "Raju Back", "Raju Pose walk_front", "Raju"] {
            assert!(moved.join(format!("{name}.buzz")).exists(), "{name} is in the backup");
        }

        let asset = library.assets().iter().find(|a| a.name == "Raju").cloned().unwrap();
        let mut scene = Scene::default();
        scene.set_frame_count(30);
        library.place(&asset, &mut scene).expect("places");
        let main = main_symbol(&scene).expect("the placed character has a set");
        let set = scene.library().symbol_set(main);
        assert_eq!(set.len(), 4, "the main pose plus three more");
        let named = |n: &str| set.iter().copied().find(|id| scene.library().get(*id).is_some_and(|s| s.name == n)).unwrap_or_else(|| panic!("no pose {n}"));
        let (back, side) = (named("Raju Back"), named("Raju Side"));
        let (layer, id) = scene
            .stage_layers()
            .iter()
            .find_map(|l| l.all_objects().find(|o| matches!(&o.kind, ObjectKind::Instance(i) if i.symbol == main)).map(|o| (l.id, o.id)))
            .unwrap();
        scene.update_layer(layer, |l| {
            while l.frames.length() < 30 {
                l.frames.insert_frame(l.frames.length());
            }
        });
        assert!(scene.pick_from_set(10, id, back, buzz_scene::SetStart::FromStart));
        assert!(scene.pick_from_set(20, id, side, buzz_scene::SetStart::FromStart));
        let shown = |frame: u32| {
            let l = scene.layers().get(layer).unwrap();
            let o = l.frames.objects_at(frame).iter().find(|o| o.id == id).unwrap().clone();
            o.instance().unwrap().symbol
        };
        for f in 0..10 {
            assert_eq!(shown(f), main, "frame {f}");
        }
        for f in 10..20 {
            assert_eq!(shown(f), back, "frame {f}");
        }
        for f in [20, 29] {
            assert_eq!(shown(f), side, "frame {f}");
        }
    }

    #[test]
    fn pose_names_read_well() {
        assert_eq!(pose_of("DEEPAK Side"), Some(("DEEPAK".into(), "DEEPAK Side".into())));
        assert_eq!(pose_of("PADOSI 1 3-4"), Some(("PADOSI 1".into(), "PADOSI 1 3-4".into())));
        assert_eq!(pose_of("DEEPAK Pose walk_front"), Some(("DEEPAK".into(), "DEEPAK Walk Front".into())));
        assert_eq!(pose_of("DEEPAK"), None);
    }
}
