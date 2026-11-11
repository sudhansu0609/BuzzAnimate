//! **A Cast character's poses on the stage** (CAST_POSES_PLAN C2).
//!
//! A combined Cast asset holds one symbol per pose, each holding that pose's
//! layer rig. A produced scene needs the rig *on the stage* (the performer, the
//! blinks and the lip sync only work on stage layers), so [`lift_character`]
//! unpacks one pose's layers into a layer **folder named after the character**.
//! The folder records its Cast asset in `Layer::cast_source`, and each lifted
//! layer records `<asset>|<pose symbol name>`, so a later switch knows which
//! layers are which pose. The whole combined library travels with the first
//! lift, so every other pose is one symbol away.
//!
//! [`switch_lifted_pose`] turns the character to another pose from a frame on:
//! the pose showing there gets a blank keyframe, the chosen pose's layers are
//! shown instead, stood where the old pose's feet were and as tall as it was,
//! and the old pose's mouth shapes and blinks are carried across to the new one
//! so it keeps talking and blinking. One call is one edit of the scene (one undo
//! step when the caller wraps it in a single document edit).

use std::sync::Arc;

use buzz_geom::{Affine, Point, Rect};
use buzz_scene::{
    ImportTarget, Keyframe, LayerId, LayerKind, LayerTimeline, LoopMode, Object, ObjectId, ObjectKind, Scene,
    SymbolId,
};

use crate::layer_rig::{LayerRigView, key_at, record_rests, rig_of_layer};

/// **Hold a rig's chart parts still** (the face symbol, a palm set): a part whose
/// symbol has many frames and loops would cycle through them on the stage, and a
/// face chart caught mid-cycle can be a blank head. Every such part but the
/// mouth (which the lip sync drives) is set to show its first frame, held.
pub fn hold_chart_parts(scene: &mut Scene, view: &LayerRigView) {
    let mouth = crate::textsync::find_layer_rig_mouth(scene, view);
    for &(_, layer, id) in &view.parts {
        if Some(id) == mouth {
            continue;
        }
        let long = scene
            .find_object(id)
            .and_then(|(_, o)| o.instance().map(|i| (i.symbol, i.loop_mode)))
            .is_some_and(|(sym, mode)| mode == LoopMode::Loop && scene.library().get(sym).is_some_and(|s| s.length() > 1));
        if !long {
            continue;
        }
        scene.update_layer(layer, |l| {
            let keys: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).collect();
            for start in keys {
                let held: Vec<Arc<Object>> = l
                    .frames
                    .keyframes()
                    .iter()
                    .find(|k| k.start == start)
                    .map(|k| {
                        k.objects
                            .iter()
                            .map(|o| {
                                let mut c = (**o).clone();
                                if c.id == id
                                    && let ObjectKind::Instance(i) = &mut c.kind
                                {
                                    i.first_frame = 0;
                                    i.loop_mode = LoopMode::SingleFrame;
                                }
                                Arc::new(c)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                l.frames.set_objects(start, held);
            }
        });
    }
}

/// What [`lift_character`] put on the stage.
#[derive(Debug, Clone)]
pub struct Lifted {
    /// The character's layer folder.
    pub folder: LayerId,
    /// The main (Front) symbol, as named in this scene's library.
    pub main: SymbolId,
    /// The pose lifted, as named in this scene's library.
    pub pose: String,
    /// Every layer of that pose, front first.
    pub layers: Vec<LayerId>,
}

/// What [`switch_lifted_pose`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SwitchReport {
    pub from: String,
    pub to: String,
    /// The first frame the old pose shows again, or the end of the layers.
    pub until: u32,
    pub mouth_keys: usize,
    pub blink_keys: usize,
    /// False when the pose was already showing and nothing changed.
    pub changed: bool,
}

fn group_key(source: &str) -> Option<&str> {
    source.split_once('|').map(|(_, pose)| pose)
}

/// The character folder `layer` is, or sits in.
pub fn character_folder_of(scene: &Scene, layer: LayerId) -> Option<LayerId> {
    let layers = scene.layers();
    let mut at = layers.get(layer)?;
    for _ in 0..layers.len() + 1 {
        if at.kind == LayerKind::Folder && at.cast_source.as_deref().is_some_and(|s| !s.contains('|')) {
            return Some(at.id);
        }
        at = layers.get(at.parent?)?;
    }
    None
}

/// The main (Front) symbol of a character folder: the library symbol named like it.
pub fn main_symbol_of(scene: &Scene, folder: LayerId) -> Option<SymbolId> {
    let name = &scene.layers().get(folder)?.name;
    scene.library().find_by_name(name).map(|s| s.id)
}

/// Every pose of a character folder, main first: `(symbol, name)`.
pub fn poses_of(scene: &Scene, folder: LayerId) -> Vec<(SymbolId, String)> {
    let Some(main) = main_symbol_of(scene, folder) else {
        return Vec::new();
    };
    scene
        .library()
        .symbol_set(main)
        .into_iter()
        .filter_map(|id| scene.library().get(id).map(|s| (id, s.name.clone())))
        .collect()
}

/// The pose symbol for a view word (`Front`, `Side`, `Back`, `3-4`, `Walk Side`...).
pub fn pose_for_view(scene: &Scene, folder: LayerId, view: &str) -> Option<SymbolId> {
    let poses = poses_of(scene, folder);
    let main = poses.first()?.0;
    if view.eq_ignore_ascii_case("front") {
        return Some(main);
    }
    let suffix = format!(" {}", view.to_lowercase());
    poses.iter().skip(1).find(|(_, n)| n.to_lowercase().ends_with(&suffix)).map(|(id, _)| *id)
}

struct Group {
    pose: String,
    layers: Vec<LayerId>,
    root: LayerId,
}

fn groups_of(scene: &Scene, folder: LayerId) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for l in scene.layers().iter() {
        let Some(pose) = l.cast_source.as_deref().and_then(group_key) else {
            continue;
        };
        // Filed in the folder directly, or in a folder inside the pose's own layers.
        if character_folder_of(scene, l.id) != Some(folder) {
            continue;
        }
        match out.iter_mut().find(|g| g.pose == pose) {
            Some(g) => g.layers.push(l.id),
            None => out.push(Group { pose: pose.to_string(), layers: vec![l.id], root: l.id }),
        }
    }
    for g in &mut out {
        let inside = g.layers.clone();
        // The rig's own root part when the group is a rig (a spare layer nothing
        // follows must not be taken for it), else the first layer nothing in the group follows.
        g.root = g
            .layers
            .iter()
            .find_map(|id| rig_of_layer(scene, *id).map(|v| v.root))
            .or_else(|| {
                g.layers
                    .iter()
                    .copied()
                    .find(|id| scene.layers().get(*id).is_some_and(|l| l.follows.is_none_or(|f| !inside.contains(&f))))
            })
            .unwrap_or(g.root);
    }
    out
}

fn shows(scene: &Scene, g: &Group, frame: u32) -> bool {
    g.layers.iter().any(|id| scene.layers().get(*id).is_some_and(|l| !l.frames.objects_at(frame).is_empty()))
}

/// The layer rig of the pose showing at `frame` (a character folder's current rig).
pub fn rig_shown(scene: &Scene, folder: LayerId, frame: u32) -> Option<LayerRigView> {
    let groups = groups_of(scene, folder);
    let g = groups.iter().find(|g| shows(scene, g, frame))?;
    rig_of_layer(scene, g.root)
}

/// The pose showing at `frame`, by its symbol name.
pub fn shown_pose(scene: &Scene, folder: LayerId, frame: u32) -> Option<String> {
    groups_of(scene, folder).into_iter().find(|g| shows(scene, g, frame)).map(|g| g.pose)
}

/// Every layer of the character (the folder first), for the producer's later passes.
pub fn character_layers(scene: &Scene, folder: LayerId) -> Vec<LayerId> {
    let mut out = vec![folder];
    out.extend(
        scene
            .layers()
            .iter()
            .filter(|l| l.id != folder && character_folder_of(scene, l.id) == Some(folder))
            .map(|l| l.id),
    );
    out
}

/// Every layer of the character's poses that follows no other layer of its pose:
/// where to move and scale it.
pub fn character_tops(scene: &Scene, folder: LayerId) -> Vec<LayerId> {
    let mut out = Vec::new();
    for g in groups_of(scene, folder) {
        for id in &g.layers {
            if scene.layers().get(*id).is_some_and(|l| l.follows.is_none_or(|f| !g.layers.contains(&f))) {
                out.push(*id);
            }
        }
    }
    out
}

/// Put the copy of a pose's layers just under `folder`, filed in it.
fn adopt_pose(scene: &mut Scene, folder: LayerId, asset_key: &str, pose: SymbolId) -> (Vec<LayerId>, String) {
    let name = scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default();
    let at = scene.layers().index_of(folder).map_or(0, |i| i + 1);
    let ids = scene.unpack_symbol_layers(pose, at);
    for id in &ids {
        scene.update_layer(*id, |l| {
            if l.parent.is_none() {
                l.parent = Some(folder);
            }
            l.cast_source = Some(format!("{asset_key}|{name}"));
        });
    }
    (ids, name)
}

/// **Unpack a combined Cast asset's character onto the stage.** `asset` is the
/// loaded combined asset, `main` its main symbol, `pose` the pose symbol to show
/// first (in `asset`); `asset_key` is its Cast path (`Cast/<Series>/<Name>`).
/// The whole library of the asset joins the scene's, so every other pose can be
/// switched to later.
pub fn lift_character(scene: &mut Scene, asset: &Scene, main: SymbolId, pose: SymbolId, asset_key: &str) -> Result<Lifted, String> {
    let main_name = asset.library().get(main).ok_or("the asset has no main symbol")?.name.clone();
    let pose_name = asset.library().get(pose).ok_or("the asset has no such pose")?.name.clone();
    let report = scene.merge(asset, ImportTarget::Library);
    let given = |wanted: &str| {
        report.renamed.iter().find(|(w, _)| w == wanted).map(|(_, g)| g.clone()).unwrap_or_else(|| wanted.to_string())
    };
    let (main_given, pose_given) = (given(&main_name), given(&pose_name));
    let new_main = scene.library().find_by_name(&main_given).map(|s| s.id).ok_or("the character did not arrive")?;
    let new_pose = scene.library().find_by_name(&pose_given).map(|s| s.id).ok_or("the pose did not arrive")?;
    let folder = scene.add_stage_layer(main_given, LayerKind::Folder);
    scene.update_layer(folder, |l| {
        l.cast_source = Some(asset_key.to_string());
        l.collapsed = true;
    });
    let (layers, pose) = adopt_pose(scene, folder, asset_key, new_pose);
    if layers.is_empty() {
        return Err("the pose has no layers".into());
    }
    if let Some(view) = layers.iter().find_map(|l| rig_of_layer(scene, *l)) {
        hold_chart_parts(scene, &view);
        record_rests(scene, &view);
        // The character's own head, playing on the scene's frames: its mouth and lids are keyed inside it.
        crate::face::fork_face(scene, &view);
    }
    Ok(Lifted { folder, main: new_main, pose, layers })
}

// -------------------------------------------------------------------- the switch

fn union(a: Option<Rect>, b: Rect) -> Option<Rect> {
    if b.width() <= 0.0 && b.height() <= 0.0 {
        return a;
    }
    Some(a.map_or(b, |u| u.union(b)))
}

fn invert(a: Affine) -> Affine {
    if a.determinant().abs() < 1e-12 { Affine::IDENTITY } else { a.inverse() }
}

/// Where the rig stands at `frame`: its root part's motion from rest, with what
/// the root inherits ([`crate::layer_rig::placement`] at any frame).
fn placement_at(scene: &Scene, view: &LayerRigView, frame: u32) -> Option<Affine> {
    let layer = scene.layers().get(view.root)?;
    let resolved = layer.frames.resolved_at(frame);
    let root = resolved.iter().find(|o| o.id == view.body)?.transform;
    let rest = layer.rest_pose.or_else(|| layer.frames.keyframes().iter().find_map(|k| k.objects.first()).map(|o| o.transform))?;
    Some(scene.layers().inherited_transform(view.root, frame) * root * invert(rest))
}

/// Is `name` the lid layer of an eye symbol ("Eyelid", any case)?
pub fn is_lid_layer_name(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("eyelid")
}

/// **Which frames of a lid chart are open, half shut and shut**, read from the
/// chart itself: the lid drawn on each frame is measured, the tallest one is the
/// shut eye (it covers the most white), the next tallest the half-shut one, and
/// the open one is frame 0 unless that frame draws a lid as tall as the rest.
/// `[open, half, shut]`, or `None` for a chart of fewer than three frames or one
/// whose frames all look alike.
pub fn lid_chart_frames(scene: &Scene, chart: SymbolId) -> Option<[u32; 3]> {
    let symbol = scene.library().get(chart)?;
    let n = symbol.length().min(32);
    if n < 3 {
        return None;
    }
    let heights: Vec<f64> = (0..n)
        .map(|f| {
            let mut top = f64::MAX;
            let mut bottom = f64::MIN;
            for l in symbol.layers.iter() {
                for o in l.objects_at(f).iter() {
                    let r = scene.resolved_bounds(o);
                    if r.width() > 0.0 || r.height() > 0.0 {
                        top = top.min(r.y0);
                        bottom = bottom.max(r.y1);
                    }
                }
            }
            if bottom > top { bottom - top } else { 0.0 }
        })
        .collect();
    let by_height = |skip: &[u32]| (0..n).filter(|f| !skip.contains(f)).max_by(|a, b| heights[*a as usize].total_cmp(&heights[*b as usize]).then(b.cmp(a)));
    let shut = by_height(&[])?;
    let half = by_height(&[shut])?;
    let tallest = heights[shut as usize];
    if tallest <= 0.0 {
        return None;
    }
    let open = if heights[0] <= 0.5 * tallest {
        0
    } else {
        (0..n).min_by(|a, b| heights[*a as usize].total_cmp(&heights[*b as usize]).then(a.cmp(b)))?
    };
    (open != shut && open != half).then_some([open, half, shut])
}

/// Eye state of an eye symbol: 0 open, 1 half shut, 2 shut (the eyelid frame,
/// by the chart's own half and shut frames).
fn eye_state(scene: &Scene, symbol: SymbolId) -> usize {
    let Some(sym) = scene.library().get(symbol) else {
        return 0;
    };
    let lid = sym.layers.iter().find(|l| is_lid_layer_name(&l.name)).and_then(|l| l.frames.objects_at(0).first().and_then(|o| o.instance().map(|i| (i.symbol, i.first_frame))));
    let Some((chart, frame)) = lid else {
        return 0;
    };
    match lid_chart_frames(scene, chart) {
        Some([_, half, shut]) => match frame {
            f if f == shut => 2,
            f if f == half => 1,
            _ => 0,
        },
        None => match frame {
            1 => 1,
            2 => 2,
            _ => 0,
        },
    }
}

/// The eye parts of a layer rig: instances of a composite eye symbol with an
/// `Eyelid` layer, on a layer of their own.
pub fn layer_eye_parts(scene: &Scene, rig: &LayerRigView) -> Vec<(ObjectId, SymbolId)> {
    let mut found = Vec::new();
    for &(_, _, id) in &rig.parts {
        let Some((_, object)) = scene.find_object(id) else {
            continue;
        };
        let ObjectKind::Instance(instance) = &object.kind else {
            continue;
        };
        let Some(symbol) = scene.library().get(instance.symbol) else {
            continue;
        };
        let name = symbol.name.to_lowercase();
        let has_lid = symbol.layers.iter().any(|l| is_lid_layer_name(&l.name));
        if name.contains("eye") && has_lid && !name.contains("brow") {
            found.push((id, instance.symbol));
        }
    }
    found
}

/// A half-shut and a shut copy of each eye part of a layer rig, by object.
pub fn layer_eye_variants(scene: &mut Scene, rig: &LayerRigView, label: &str) -> Vec<(ObjectId, [SymbolId; 3])> {
    let mut out = Vec::new();
    for (id, open) in layer_eye_parts(scene, rig) {
        let Some(original) = scene.library().get(open).cloned() else {
            continue;
        };
        let mut shut_ids = [open; 3];
        // The chart's own half-shut and shut frames (1 and 2 for a chart that does not tell).
        let chart = original.layers.iter().find(|l| is_lid_layer_name(&l.name)).and_then(|l| l.frames.objects_at(0).first().and_then(|o| o.instance().map(|i| i.symbol)));
        let [_, half, shut] = chart.and_then(|c| lid_chart_frames(scene, c)).unwrap_or([0, 1, 2]);
        for (slot, frame) in [(1usize, half), (2, shut)] {
            let copy_id = scene.add_symbol(
                format!("{label} eye {}", if slot == 1 { "half" } else { "shut" }),
                buzz_scene::SymbolKind::Graphic,
                Some("Cast parts"),
            );
            let mut copy = (*original).clone();
            copy.id = copy_id;
            copy.name = scene.library().get(copy_id).map(|s| s.name.clone()).unwrap_or_else(|| copy.name.clone());
            copy.folder = Some("Cast parts".to_string());
            let lid = copy.layers.iter().find(|l| is_lid_layer_name(&l.name)).map(|l| l.id);
            if let Some(lid) = lid {
                copy.layers.update(lid, |l| {
                    if let Some(first) = l.frames.objects_at(0).first() {
                        let mut updated = (**first).clone();
                        if let ObjectKind::Instance(i) = &mut updated.kind {
                            i.loop_mode = LoopMode::SingleFrame;
                            i.first_frame = frame;
                        }
                        l.frames.set_objects(0, vec![Arc::new(updated)]);
                    }
                });
            }
            scene.library_mut().insert(copy);
            shut_ids[slot] = copy_id;
        }
        out.push((id, shut_ids));
    }
    out
}

/// The object of `id` on `layer` at `frame`.
fn object_on(scene: &Scene, layer: LayerId, id: ObjectId, frame: u32) -> Option<Arc<Object>> {
    scene.layers().get(layer)?.frames.objects_at(frame).iter().find(|o| o.id == id).cloned()
}

/// Mouth shape per change over `from..until` of the rig's mouth part: `(frame, first_frame)`.
fn mouth_track(scene: &Scene, view: &LayerRigView, from: u32, until: u32) -> Vec<(u32, u32)> {
    let Some(mouth) = crate::textsync::find_layer_rig_mouth(scene, view) else {
        // A mouth chart inside the head symbol (the Animate way).
        return crate::face::face_of(scene, view).and_then(|f| f.mouth).map(|spot| crate::face::chart_track(scene, spot, from, until)).unwrap_or_default();
    };
    let Some((layer, _)) = scene.find_object(mouth) else {
        return Vec::new();
    };
    let mut out: Vec<(u32, u32)> = Vec::new();
    for f in from..until {
        let Some(shape) = object_on(scene, layer, mouth, f).and_then(|o| o.instance().map(|i| i.first_frame)) else {
            continue;
        };
        if out.last().is_none_or(|(_, s)| *s != shape) {
            out.push((f, shape));
        }
    }
    out
}

/// Eye state per change over `from..until`, per eye part in order: `(frame, state)`.
fn eye_tracks(scene: &Scene, view: &LayerRigView, from: u32, until: u32) -> Vec<Vec<(u32, usize)>> {
    layer_eye_parts(scene, view)
        .into_iter()
        .map(|(id, _)| {
            let Some((layer, _)) = scene.find_object(id) else {
                return Vec::new();
            };
            let mut out: Vec<(u32, usize)> = Vec::new();
            let mut last = 0usize;
            for f in from..until {
                let Some(symbol) = object_on(scene, layer, id, f).and_then(|o| o.instance().map(|i| i.symbol)) else {
                    continue;
                };
                let state = eye_state(scene, symbol);
                if state != last {
                    out.push((f, state));
                    last = state;
                }
            }
            out
        })
        .collect()
}

/// The keyframe before `frame` must not tween into the key that now starts at `frame`.
fn cut_tween_before(layer: &mut buzz_scene::Layer, frame: u32) {
    let before = layer.frames.keyframes().iter().map(|k| k.start).filter(|s| *s < frame).max();
    if let Some(p) = before {
        layer.frames.set_tween(p, buzz_scene::Tween::default());
    }
}

/// The mouth chart frame of the pose showing at `frame`, when it has a mouth (for gates and tests).
pub fn mouth_shape_at(scene: &Scene, folder: LayerId, frame: u32) -> Option<u32> {
    let groups = groups_of(scene, folder);
    let g = groups.iter().find(|g| shows(scene, g, frame))?;
    let view = rig_of_layer(scene, g.root)?;
    let mouth = crate::textsync::find_layer_rig_mouth(scene, &view)?;
    let (layer, _) = scene.find_object(mouth)?;
    object_on(scene, layer, mouth, frame).and_then(|o| o.instance().map(|i| i.first_frame))
}

/// Does the pose showing at `frame` move (its root part is keyed somewhere else) before it
/// stops showing? A switch does not carry walking, so a caller can leave such a
/// character alone.
pub fn moves_after(scene: &Scene, folder: LayerId, frame: u32) -> bool {
    let groups = groups_of(scene, folder);
    let Some(g) = groups.iter().find(|g| shows(scene, g, frame)) else {
        return false;
    };
    let Some(view) = rig_of_layer(scene, g.root) else {
        return false;
    };
    let n = scene.layers().get(g.root).map_or(frame + 1, |l| l.frames.length());
    let until = (frame + 1..n).find(|f| !shows(scene, g, *f)).unwrap_or(n);
    let Some(here) = placement_at(scene, &view, frame) else {
        return false;
    };
    let keys: Vec<u32> = scene
        .layers()
        .get(g.root)
        .map(|l| l.frames.keyframes().iter().map(|k| k.start).filter(|s| *s > frame && *s < until).collect())
        .unwrap_or_default();
    keys.into_iter().any(|k| {
        placement_at(scene, &view, k).is_some_and(|p| {
            // Idle sway moves the root a little; a walk moves it across the stage.
            let (a, b) = (p.as_coeffs(), here.as_coeffs());
            (a[4] - b[4]).abs() > 25.0 || (a[5] - b[5]).abs() > 25.0
        })
    })
}

/// **Turn a lifted character to `pose` from `frame` on.** See the module note.
/// `pose` must be a member of the character's set (see [`poses_of`]). The pose
/// showing at `frame` is blanked from `frame` until it would have stopped
/// showing; the chosen pose is stood on the old pose's ground point at the
/// old pose's height, mirrored if the old pose was; mouth shapes and blinks of
/// the old pose in that span are written onto the new one.
pub fn switch_lifted_pose(scene: &mut Scene, folder: LayerId, frame: u32, pose: SymbolId) -> Result<SwitchReport, String> {
    let asset_key = scene
        .layers()
        .get(folder)
        .and_then(|l| l.cast_source.clone())
        .filter(|_| scene.layers().get(folder).is_some_and(|l| l.kind == LayerKind::Folder))
        .ok_or("this is not a lifted Cast character")?;
    let main = main_symbol_of(scene, folder).ok_or("the character's main symbol is not in the library")?;
    if !scene.library().symbol_set(main).contains(&pose) {
        return Err("that pose is not in the character's set".into());
    }
    let pose_name = scene.library().get(pose).map(|s| s.name.clone()).unwrap_or_default();
    let groups = groups_of(scene, folder);
    let old_index = groups.iter().position(|g| shows(scene, g, frame)).ok_or("no pose of this character shows on that frame")?;
    if groups[old_index].pose == pose_name {
        return Ok(SwitchReport { from: pose_name.clone(), to: pose_name, until: frame, ..SwitchReport::default() });
    }
    let n = groups
        .iter()
        .flat_map(|g| g.layers.iter())
        .filter_map(|l| scene.layers().get(*l).map(|l| l.frames.length()))
        .max()
        .unwrap_or(1)
        .max(frame + 1);
    let old = &groups[old_index];
    let until = (frame + 1..n).find(|f| !shows(scene, old, *f)).unwrap_or(n);
    let old_pose = old.pose.clone();
    let old_layers = old.layers.clone();
    let old_root = old.root;
    let old_view = rig_of_layer(scene, old_root);

    // -- the new pose's layers at rest: a fresh copy, kept or read from
    let mut target = groups.iter().find(|g| g.pose == pose_name).map(|g| (g.layers.clone(), g.root));
    let at = scene.layers().index_of(folder).map_or(0, |i| i + 1);
    let temp = scene.unpack_symbol_layers(pose, at);
    if temp.is_empty() {
        return Err("the pose has no layers".into());
    }
    if let Some(view) = temp.iter().find_map(|l| rig_of_layer(scene, *l)) {
        hold_chart_parts(scene, &view);
        // A pose shown for the first time gets its own head, before its rest is read.
        if target.is_none() {
            crate::face::fork_face(scene, &view);
        }
    }
    let mut rest: Vec<Vec<Arc<Object>>> = temp
        .iter()
        .map(|l| scene.layers().get(*l).map(|l| l.frames.objects_at(0).to_vec()).unwrap_or_default())
        .collect();
    // The new pose's box at rest, followers carrying what they inherit (as the old
    // pose's box below), measured on the fresh layers before they are blanked.
    let mut new_box: Option<Rect> = None;
    for l in &temp {
        let inherited = scene.layers().inherited_transform(*l, 0u32);
        for o in scene.layers().get(*l).map(|l| l.frames.objects_at(0).to_vec()).unwrap_or_default() {
            if o.visible {
                new_box = union(new_box, buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(&o)));
            }
        }
    }
    let new_layers: Vec<LayerId> = match &mut target {
        Some((layers, _)) => {
            for l in &temp {
                scene.remove_layer(*l);
            }
            layers.clone()
        }
        None => {
            if let Some(view) = temp.iter().find_map(|l| rig_of_layer(scene, *l)) {
                record_rests(scene, &view);
            }
            for id in &temp {
                scene.update_layer(*id, |l| {
                    if l.parent.is_none() {
                        l.parent = Some(folder);
                    }
                    l.cast_source = Some(format!("{asset_key}|{pose_name}"));
                    // Blank everywhere until it is switched to.
                    l.frames = LayerTimeline::from_parts(vec![Keyframe::new(0)], n);
                });
            }
            temp.clone()
        }
    };
    if new_layers.len() != rest.len() {
        return Err("the pose's layers changed since it was lifted".into());
    }
    // A pose switched back to keeps the object ids it already had on its layers,
    // so its rig reads the same on every frame.
    if target.is_some() {
        for (l, objects) in new_layers.iter().zip(rest.iter_mut()) {
            let keep: Vec<ObjectId> = scene
                .layers()
                .get(*l)
                .and_then(|layer| layer.frames.keyframes().iter().find(|k| !k.objects.is_empty()))
                .map(|k| k.objects.iter().map(|o| o.id).collect())
                .unwrap_or_default();
            if keep.len() == objects.len() {
                for (o, id) in objects.iter_mut().zip(keep) {
                    let mut copy = (**o).clone();
                    copy.id = id;
                    *o = Arc::new(copy);
                }
            }
        }
    }
    let inside = new_layers.clone();
    let is_root = |scene: &Scene, id: LayerId| scene.layers().get(id).is_some_and(|l| l.follows.is_none_or(|f| !inside.contains(&f)));

    // -- where the old pose stands at `frame`
    // Measured at `frame` itself, followers carrying what they inherit.
    let first_shown = frame;
    let mut old_box: Option<Rect> = None;
    for l in &old_layers {
        if let Some(layer) = scene.layers().get(*l) {
            // A follower's own transform is relative to what it inherits.
            let inherited = scene.layers().inherited_transform(*l, first_shown);
            for o in layer.frames.objects_at(first_shown) {
                if o.visible {
                    old_box = union(old_box, buzz_scene::object::transform_rect(inherited, scene.resolved_bounds(o)));
                }
            }
        }
    }
    let motion = old_view
        .as_ref()
        .and_then(|v| placement_at(scene, v, frame))
        .map(|now| (Affine::IDENTITY, now.determinant() < 0.0));

    let stand = match (old_box, new_box) {
        (Some(ob), Some(nb)) if nb.height() > 1e-6 => {
            let (m, mirrored) = motion.unwrap_or((Affine::IDENTITY, false));
            let feet = m * Point::new(ob.center().x, ob.y1);
            let scale = ob.height() * m.determinant().abs().sqrt() / nb.height();
            let sx = if mirrored { -scale } else { scale };
            Affine::translate(feet.to_vec2()) * Affine::new([sx, 0.0, 0.0, scale, 0.0, 0.0]) * Affine::translate((-nb.center().x, -nb.y1))
        }
        _ => Affine::IDENTITY,
    };

    // -- what the old pose says: mouth shapes and blinks over the span
    let mouth = old_view.as_ref().map(|v| mouth_track(scene, v, frame, until)).unwrap_or_default();
    let eyes = old_view.as_ref().map(|v| eye_tracks(scene, v, frame, until)).unwrap_or_default();
    // The old head's chart has its own viseme map; the new head's chart has another.
    let old_map = old_view
        .as_ref()
        .and_then(|v| crate::face::face_of(scene, v))
        .and_then(|f| crate::face::mouth_chart(scene, &f))
        .map(|c| crate::face::viseme_map_of(scene, c));
    // The lid chart inside the head: `(frame, state)` per change.
    let lids: Vec<(u32, usize)> = old_view
        .as_ref()
        .and_then(|v| crate::face::face_of(scene, v))
        .and_then(|f| f.lid)
        .map(|(spot, frames)| crate::face::chart_track(scene, spot, frame, until).into_iter().map(|(f, c)| (f, crate::face::lid_state_of(frames, c))).collect())
        .unwrap_or_default();

    // -- the old pose goes blank over the span
    for l in &old_layers {
        scene.update_layer(*l, |layer| {
            if layer.frames.length() < n {
                layer.frames.insert_frame(n - 1);
            }
            layer.frames.insert_keyframe(frame);
            cut_tween_before(layer, frame);
            let starts: Vec<u32> = layer.frames.keyframes().iter().map(|k| k.start).filter(|s| *s >= frame && *s < until).collect();
            for s in starts {
                layer.frames.set_objects(s, Vec::new());
                layer.frames.set_tween(s, buzz_scene::Tween::default());
            }
        });
    }
    // -- the new pose shows over the span
    for (i, l) in new_layers.iter().enumerate() {
        let root = is_root(scene, *l);
        let objects: Vec<Arc<Object>> = rest[i]
            .iter()
            .map(|o| {
                let mut copy = (**o).clone();
                if root {
                    copy.transform = stand * copy.transform;
                }
                Arc::new(copy)
            })
            .collect();
        scene.update_layer(*l, |layer| {
            if layer.frames.length() < n {
                layer.frames.insert_frame(n - 1);
            }
            if until < n {
                layer.frames.insert_keyframe(until);
            }
            layer.frames.insert_keyframe(frame);
            cut_tween_before(layer, frame);
            let starts: Vec<u32> = layer.frames.keyframes().iter().map(|k| k.start).filter(|s| *s >= frame && *s < until).collect();
            for s in starts {
                layer.frames.set_objects(s, objects.clone());
                layer.frames.set_tween(s, buzz_scene::Tween::default());
            }
        });
    }
    // The key at `until` was copied from a blank one: it stays blank.
    let mut report = SwitchReport { from: old_pose, to: pose_name, until, changed: true, ..SwitchReport::default() };

    // -- carry the mouth and the blinks across
    let Some(view) = new_layers.iter().find_map(|l| rig_of_layer(scene, *l)) else {
        return Ok(report);
    };
    if let Some(m) = crate::textsync::find_layer_rig_mouth(scene, &view)
        && let Some((layer, _)) = scene.find_object(m)
    {
        for (f, shape) in &mouth {
            key_at(scene, layer, *f);
            scene.update_object_at(*f, m, |o| {
                if let ObjectKind::Instance(i) = &mut o.kind {
                    i.first_frame = *shape;
                    i.loop_mode = LoopMode::SingleFrame;
                }
            });
            report.mouth_keys += 1;
        }
    }
    if let Some(face) = crate::face::face_of(scene, &view) {
        if !mouth.is_empty()
            && crate::textsync::find_layer_rig_mouth(scene, &view).is_none()
            && let Some(spot) = face.mouth
        {
            let new_map = crate::face::mouth_chart(scene, &face).map(|c| crate::face::viseme_map_of(scene, c));
            let keys: Vec<(u32, u32)> = match (old_map, new_map) {
                (Some(from), Some(to)) => mouth.iter().map(|(f, c)| (*f, to[crate::face::viseme_of_frame(&from, *c).frame() as usize])).collect(),
                _ => mouth.clone(),
            };
            crate::face::put_chart_keys(scene, &face, spot, &keys);
            report.mouth_keys += mouth.len();
        }
        if !lids.is_empty()
            && let Some((spot, frames)) = face.lid
        {
            let keys: Vec<(u32, u32)> = lids.iter().map(|(f, state)| (*f, frames[*state])).collect();
            crate::face::put_chart_keys(scene, &face, spot, &keys);
            report.blink_keys += keys.len();
        }
    }
    if eyes.iter().any(|e| !e.is_empty()) {
        let label = scene.layers().get(folder).map(|l| l.name.clone()).unwrap_or_default();
        let variants = layer_eye_variants(scene, &view, &label);
        for (track, (eye, symbols)) in eyes.iter().zip(variants.iter()) {
            let Some((layer, _)) = scene.find_object(*eye) else {
                continue;
            };
            for (f, state) in track {
                key_at(scene, layer, *f);
                let symbol = symbols[*state];
                scene.update_object_at(*f, *eye, |o| {
                    if let ObjectKind::Instance(i) = &mut o.kind {
                        i.symbol = symbol;
                    }
                });
                report.blink_keys += 1;
            }
            // Open again where the old pose stopped blinking is covered by the
            // track itself (a change back to state 0 is a change).
            let _ = layer;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{LayerKind, ShapeData, SymbolKind};

    /// One pose's rig on the stage of `scene`: a body, a head and a mouth part
    /// (an instance of `mouth`), `scale` times bigger.
    fn rig(scene: &mut Scene, mouth: SymbolId, scale: f64) {
        let mut parts = Vec::new();
        for (name, bar) in [
            ("Hips", Rect::new(0.0, 100.0, 20.0 * scale, 140.0 * scale)),
            ("Head", Rect::new(0.0, 0.0, 20.0 * scale, 60.0 * scale)),
        ] {
            let layer = scene.add_layer(name, LayerKind::Normal);
            let id = scene.add_shape(layer, ShapeData::filled(bar.to_path(1e-9), peniko::Color::WHITE)).expect("part");
            parts.push((layer, id));
        }
        let mouth_layer = scene.add_layer("Head 2", LayerKind::Normal);
        scene.add_instance_at(mouth_layer, 0, mouth, Affine::translate((5.0, 40.0)));
        scene.set_follows(parts[1].0, Some(parts[0].0), 0);
        scene.set_follows(mouth_layer, Some(parts[1].0), 0);
    }

    fn pose_symbol(scene: &mut Scene, name: &str, mouth: SymbolId, scale: f64) -> SymbolId {
        let mut staged = scene.clone();
        for id in staged.stage_layers().iter().map(|l| l.id).collect::<Vec<_>>() {
            staged.remove_layer(id);
        }
        rig(&mut staged, mouth, scale);
        let layers = staged.stage_layers().clone();
        let id = scene.add_symbol(name, SymbolKind::Graphic, None);
        *scene.library_mut().layers_mut(id).unwrap() = layers;
        id
    }

    #[test]
    fn switching_a_lifted_pose_turns_the_character_and_keeps_the_mouth_moving() {
        let mut asset = Scene::default();
        let mouth = asset.add_symbol("Raju mouth", SymbolKind::Graphic, None);
        let first = asset.library().get(mouth).unwrap().layers.iter().next().unwrap().id;
        asset.library_mut().update(mouth, |s| {
            s.layers.update(first, |l| {
                for _ in 0..buzz_audio::Viseme::COUNT {
                    let n = l.frames.length();
                    l.frames.insert_frame(n);
                }
            });
        });
        let front = pose_symbol(&mut asset, "Raju", mouth, 1.0);
        let side = pose_symbol(&mut asset, "Raju Side", mouth, 1.5);
        assert!(asset.library_mut().include_in_set(front, side));

        let mut scene = Scene::default();
        scene.set_frame_count(30);
        let lifted = lift_character(&mut scene, &asset, front, front, "Cast/Series/Raju").expect("lifts");
        assert_eq!(scene.layers().get(lifted.folder).unwrap().cast_source.as_deref(), Some("Cast/Series/Raju"));
        assert_eq!(character_folder_of(&scene, lifted.layers[0]), Some(lifted.folder));
        for l in &lifted.layers {
            scene.update_layer(*l, |layer| {
                while layer.frames.length() < 30 {
                    let n = layer.frames.length();
                    layer.frames.insert_frame(n);
                }
            });
        }
        // The front pose speaks: the mouth shape changes at frame 12.
        let view = rig_of_layer(&scene, lifted.layers[0]).expect("a rig");
        let m = crate::textsync::find_layer_rig_mouth(&scene, &view).expect("a mouth");
        let (mouth_layer, _) = scene.find_object(m).unwrap();
        key_at(&mut scene, mouth_layer, 12);
        scene.update_object_at(12, m, |o| {
            if let ObjectKind::Instance(i) = &mut o.kind {
                i.first_frame = 4;
            }
        });

        let poses = poses_of(&scene, lifted.folder);
        assert_eq!(poses.len(), 2, "{poses:?}");
        let side_id = pose_for_view(&scene, lifted.folder, "Side").expect("a side pose");
        assert_eq!(shown_pose(&scene, lifted.folder, 5).as_deref(), Some("Raju"));

        let report = switch_lifted_pose(&mut scene, lifted.folder, 10, side_id).expect("switches");
        assert!(report.changed && report.until == 30, "{report:?}");
        assert!(report.mouth_keys >= 1, "{report:?}");
        assert_eq!(shown_pose(&scene, lifted.folder, 9).as_deref(), Some("Raju"));
        assert_eq!(shown_pose(&scene, lifted.folder, 10).as_deref(), Some("Raju Side"));
        assert_eq!(shown_pose(&scene, lifted.folder, 29).as_deref(), Some("Raju Side"));

        // The side pose's root part (the hips) stands where the front's did: the
        // whole figure was measured 140 tall with its feet at 140, so the bigger
        // pose is scaled by 140/210 about its feet.
        let bounds = |pose: &str, frame: u32| {
            let root = groups_of(&scene, lifted.folder).into_iter().find(|g| g.pose == pose).unwrap().root;
            let mut b: Option<Rect> = None;
            for o in scene.layers().get(root).unwrap().frames.objects_at(frame) {
                b = union(b, scene.resolved_bounds(o));
            }
            b.unwrap()
        };
        let (a, c) = (bounds("Raju", 9), bounds("Raju Side", 10));
        assert!((c.height() - 110.0 * 140.0 / 210.0).abs() < 1.0, "scaled by 140/210: {c:?}");
        assert!((a.center().x - c.center().x).abs() < 1.0 && (a.y1 - c.y1).abs() < 1.0, "{a:?} {c:?}");

        // The mouth keeps its shapes on the side pose: 0 at frame 10, 4 from 12.
        let side_view = rig_of_layer(&scene, groups_of(&scene, lifted.folder).iter().find(|g| g.pose == "Raju Side").unwrap().root).expect("a rig");
        let sm = crate::textsync::find_layer_rig_mouth(&scene, &side_view).expect("a mouth");
        let (sl, _) = scene.find_object(sm).unwrap();
        let shape = |f: u32| object_on(&scene, sl, sm, f).unwrap().instance().unwrap().first_frame;
        assert_eq!((shape(10), shape(11), shape(12), shape(20)), (0, 0, 4, 4));

        // And back to the front pose at 20: the side is blank from there.
        let report = switch_lifted_pose(&mut scene, lifted.folder, 20, lifted.main).expect("switches back");
        assert!(report.changed);
        assert_eq!(shown_pose(&scene, lifted.folder, 19).as_deref(), Some("Raju Side"));
        assert_eq!(shown_pose(&scene, lifted.folder, 20).as_deref(), Some("Raju"));
    }
}
