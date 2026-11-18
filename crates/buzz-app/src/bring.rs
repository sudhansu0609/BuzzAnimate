//! **Bring Character Here** (user 2026-11-18: "if I had a character in
//! previous or later frames, I shall be simply able to put it there and frame
//! number aligned — one touch instead of going to those frame numbers"; "it
//! shall also work for scenes and maybe different files").
//!
//! Every character the timeline shows somewhere other than the playhead — and
//! every one in the other scenes of this file and in the other open files — is
//! offered by name. Picking one puts it at the playhead as it stood there: on
//! its own layer (a layer of the same name in another scene or file, made if
//! there is none), where it was, its pose and its symbols' frame numbers set
//! to the playhead's, as Paste Stage sets them. One undo step.

use std::sync::Arc;

use buzz_scene::{LayerId, LayerKind, ObjectId, ObjectKind, Scene, SymbolId};

/// One character to bring: where it is, and what it is called.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The layer it is on, in the timeline it was found in.
    pub layer: LayerId,
    pub layer_name: String,
    /// The keyframe it was found on.
    pub frame: u32,
    pub object: ObjectId,
    pub symbol: SymbolId,
    /// The character's symbol name.
    pub name: String,
}

/// The characters on `layers` of `scene`, one per character and layer.
///
/// With `playhead` (the timeline being edited): those not on stage there, each
/// from its nearest keyframe before the playhead, else after it. Without (a
/// timeline elsewhere): each from its last keyframe, where that story part
/// leaves it.
pub fn candidates(scene: &Scene, layers: &buzz_scene::LayerStack, playhead: Option<u32>) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for layer in layers.iter() {
        if layer.kind != LayerKind::Normal {
            continue;
        }
        let showing: Vec<SymbolId> = playhead
            .map(|at| layer.frames.objects_at(at).iter().filter_map(|o| o.instance().map(|i| i.symbol)).collect())
            .unwrap_or_default();
        for key in layer.frames.keyframes() {
            for object in key.objects.iter() {
                let Some(instance) = object.instance() else { continue };
                let symbol = instance.symbol;
                if showing.contains(&symbol) || !crate::scene_life::is_character_symbol(scene, symbol, 0) {
                    continue;
                }
                let found = Candidate {
                    layer: layer.id,
                    layer_name: layer.name.clone(),
                    frame: key.start,
                    object: object.id,
                    symbol,
                    name: scene.library().get(symbol).map(|s| s.name.clone()).unwrap_or_default(),
                };
                match out.iter_mut().find(|c| c.symbol == symbol && c.layer == layer.id) {
                    None => out.push(found),
                    Some(kept) => {
                        // Nearest before the playhead wins, else nearest
                        // after; elsewhere, the last.
                        let better = match playhead {
                            None => found.frame > kept.frame,
                            Some(at) => match (found.frame <= at, kept.frame <= at) {
                                (true, true) => found.frame > kept.frame,
                                (false, false) => found.frame < kept.frame,
                                (before, _) => before,
                            },
                        };
                        if better {
                            *kept = found;
                        }
                    }
                }
            }
        }
    }
    out
}

/// **Put `candidate` at `at` on the timeline being edited.**
///
/// `from` is the scene it was found in when that is not this one (another
/// scene, another file); it was found on that scene's own timeline. Its
/// symbol is the one here of the same name, else copied in with every symbol
/// it uses. The new keyframe at `at` keeps whatever else the layer showed
/// there, the same character excepted (it is replaced). Every symbol it carries is set to show `at`. The object placed.
pub fn bring(scene: &mut Scene, from: Option<&Scene>, candidate: &Candidate, at: u32) -> Option<ObjectId> {
    let source = from.unwrap_or(scene);
    let stack = if from.is_some() { source.stage_layers() } else { source.layers() };
    let mut object = (**stack
        .get(candidate.layer)?
        .frames
        .objects_at(candidate.frame)
        .iter()
        .find(|o| o.id == candidate.object)?)
    .clone();

    let layer = match from {
        None => candidate.layer,
        Some(other) => {
            let here = scene.library().iter().find(|s| s.name == candidate.name).map(|s| s.id);
            let symbol = match here {
                Some(id) => id,
                None => scene.import_symbol(other, candidate.symbol)?.0,
            };
            object.id = scene.next_object_id();
            if let ObjectKind::Instance(i) = &mut object.kind {
                i.symbol = symbol;
            }
            let named = scene
                .layers()
                .iter()
                .find(|l| l.name == candidate.layer_name && l.kind == LayerKind::Normal)
                .map(|l| l.id);
            named.unwrap_or_else(|| scene.add_layer(candidate.layer_name.clone(), LayerKind::Normal))
        }
    };

    // Frame numbers: the playhead's, as Paste Stage sets them.
    let mut inside = None;
    if let ObjectKind::Instance(instance) = &mut object.kind
        && let Some(def) = scene.library().get(instance.symbol)
        && def.kind.follows_parent_timeline()
        && !instance.sync_to_root
        && at < def.length()
    {
        instance.first_frame = instance.first_frame_showing(at, 0, def.length());
        inside = Some(instance.symbol);
    }

    let id = object.id;
    let symbol = object.instance().map(|i| i.symbol);
    let placed = Arc::new(object);
    scene.update_layer(layer, |l| {
        if l.frames.length() <= at {
            l.frames.insert_frame(at);
        }
        if !l.frames.is_keyframe(at) {
            if l.frames.objects_at(at).is_empty() {
                l.frames.insert_blank_keyframe(at);
            } else {
                l.frames.insert_keyframe(at);
            }
        }
        if let Some(objects) = l.frames.objects_at_mut(at) {
            // The same character already standing there is replaced, not
            // doubled (the keyframe made here carries what the layer showed).
            objects.retain(|o| o.id != id && (symbol.is_none() || o.instance().map(|i| i.symbol) != symbol));
            objects.push(placed);
        }
    });
    if let Some(symbol) = inside {
        crate::editor::match_frames_inside(scene, symbol, at, &mut std::collections::HashSet::new());
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Affine;
    use buzz_scene::{Object, SymbolKind};

    /// A scene with a 400-frame character `guy` (body and head layers) on a
    /// layer `Raju`, moved to (100, 50) and shown on frames 0–9 only.
    fn scene_with_guy() -> (Scene, SymbolId) {
        let mut scene = Scene::default();
        let guy = scene.add_symbol("guy", SymbolKind::Graphic, None);
        scene.library_mut().update(guy, |s| {
            let first = s.layers.iter().next().unwrap().id;
            s.layers.update(first, |l| {
                l.name = "body".into();
                l.frames.insert_frame(399);
            });
        });
        let raju = scene.add_layer("Raju", LayerKind::Normal);
        let id = scene.next_object_id();
        scene.update_layer(raju, |l| {
            l.frames.push_object(0, Arc::new(Object::instance_of(id, guy).with_transform(Affine::translate((100.0, 50.0)))));
            l.frames.insert_blank_keyframe(10);
            l.frames.insert_frame(99);
        });
        (scene, guy)
    }

    fn placed_at(scene: &Scene, layer: &str, at: u32) -> Vec<(String, u32, Affine)> {
        let layer = scene.layers().iter().find(|l| l.name == layer).unwrap();
        layer
            .frames
            .objects_at(at)
            .iter()
            .filter_map(|o| {
                let i = o.instance()?;
                Some((scene.library().get(i.symbol)?.name.clone(), i.first_frame, o.transform))
            })
            .collect()
    }

    /// From this timeline's earlier frames: offered, then put at the playhead
    /// where it stood, showing the playhead's frame number; frames before
    /// are untouched.
    #[test]
    fn a_character_from_earlier_frames_comes_to_the_playhead_aligned() {
        let (mut scene, _) = scene_with_guy();
        let found = candidates(&scene, scene.layers(), Some(50));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].name.as_str(), found[0].frame), ("guy", 0));
        assert!(candidates(&scene, scene.layers(), Some(5)).is_empty(), "on stage at frame 5: not offered");

        bring(&mut scene, None, &found[0], 50).expect("placed");
        assert_eq!(placed_at(&scene, "Raju", 50), vec![("guy".into(), 50, Affine::translate((100.0, 50.0)))]);
        assert_eq!(placed_at(&scene, "Raju", 49), Vec::new(), "the blank before it stays blank");
        assert_eq!(placed_at(&scene, "Raju", 5)[0].1, 0, "the earlier frames keep their own numbers");
    }

    /// From another scene: its symbol copied in (with what it uses), a layer
    /// of the same name made, and aligned; brought twice, the symbol is not
    /// copied twice.
    #[test]
    fn a_character_from_another_scene_lands_on_a_layer_of_its_name() {
        let (other, _) = scene_with_guy();
        let mut scene = Scene::default();
        let found = candidates(&other, other.stage_layers(), None);
        assert_eq!(found.len(), 1);

        bring(&mut scene, Some(&other), &found[0], 7).expect("placed");
        assert_eq!(placed_at(&scene, "Raju", 7), vec![("guy".into(), 7, Affine::translate((100.0, 50.0)))]);
        bring(&mut scene, Some(&other), &found[0], 20).expect("placed again");
        assert_eq!(scene.library().iter().filter(|s| s.name.starts_with("guy")).count(), 1, "one guy in the library");
        assert_eq!(scene.layers().iter().filter(|l| l.name == "Raju").count(), 1, "one Raju layer");
        assert_eq!(placed_at(&scene, "Raju", 20)[0].1, 20);
    }
}

