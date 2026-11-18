//! **Puppet mode**: a character's parts as one row and one thing to pose.
//!
//! A character drawn in Animate is a dozen part layers — body, arms, wrists,
//! palms, head — linked by layer parenting. Animating it means seeing all of
//! them, and one stray drag shifts a limb off its joint. A **puppet** files
//! those parts under one folder (`Layer::puppet`), which the timeline shows as
//! one row carrying every part's keys; on the stage a part only *turns* about
//! its joint, and every pose is keyed with an eased tween from the pose
//! before. Nothing is moved, copied or re-timed: the parts, their parenting,
//! their keys and every script that reads them are exactly as they were.

use std::sync::Arc;

use buzz_geom::Affine;
use buzz_scene::{
    Easing, FrameSegment, Layer, LayerId, LayerKind, LayerStack, LayerTimeline, LoopMode, ObjectId, ObjectKind, Scene,
    Tween,
};

/// The puppet folder `layer` sits in (or is), on the open timeline.
pub fn puppet_of(scene: &Scene, layer: LayerId) -> Option<LayerId> {
    let layers = scene.layers();
    let mut at = layers.get(layer)?;
    for _ in 0..layers.len() + 1 {
        if at.puppet {
            return Some(at.id);
        }
        at = layers.get(at.parent?)?;
    }
    None
}

/// Every keyframe start on the layers filed under `folder`, sorted: what the
/// puppet's one row shows.
pub fn puppet_keys(stack: &LayerStack, folder: LayerId) -> Vec<u32> {
    let mut keys: Vec<u32> = stack
        .iter()
        .filter(|l| inside(stack, l.id, folder))
        .flat_map(|l| l.frames.keyframes().iter().map(|k| k.start).collect::<Vec<_>>())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

fn inside(stack: &LayerStack, layer: LayerId, folder: LayerId) -> bool {
    let mut at = stack.get(layer).and_then(|l| l.parent);
    for _ in 0..stack.len() + 1 {
        match at {
            Some(p) if p == folder => return true,
            Some(p) => at = stack.get(p).and_then(|l| l.parent),
            None => return false,
        }
    }
    false
}

/// A layer that stays outside a puppet: a sound (the master audio guide, a
/// dialogue track), which is not a part to pose.
fn stays_out(layer: &buzz_scene::Layer) -> bool {
    layer.audio_guide || layer.main_sound || layer.frames.keyframes().iter().any(|k| k.sound.is_some())
}

/// **File every part of `stack` under a new puppet folder** named `name`,
/// placed where the topmost part was, collapsed. Top-level layers only (a
/// part already in a folder goes with its folder); sounds stay outside.
/// `None` when there is nothing to file. `id` is the new folder's.
pub fn puppetize(stack: &mut LayerStack, id: LayerId, name: &str) -> Option<LayerId> {
    let parts: Vec<LayerId> = stack.iter().filter(|l| l.parent.is_none() && !stays_out(l)).map(|l| l.id).collect();
    let first = *parts.first()?;
    let at = stack.iter().position(|l| l.id == first)?;
    let mut folder = buzz_scene::Layer::new(id, name, LayerKind::Folder);
    folder.puppet = true;
    folder.collapsed = true;
    stack.insert(at, folder);
    for part in parts {
        stack.update(part, |l| l.parent = Some(id));
    }
    Some(id)
}

/// **Undo a puppet**: a folder this module made (no Cast link) is dissolved,
/// its parts back at the top level where they stood; a Cast character's own
/// folder only stops being a puppet. Whether anything changed.
pub fn unpuppet(stack: &mut LayerStack, folder: LayerId) -> bool {
    let Some(layer) = stack.get(folder) else { return false };
    if layer.cast_source.is_some() {
        return stack.update(folder, |l| {
            l.puppet = false;
            l.collapsed = false;
        });
    }
    let children: Vec<LayerId> = stack.iter().filter(|l| l.parent == Some(folder)).map(|l| l.id).collect();
    for c in children {
        stack.update(c, |l| l.parent = None);
    }
    stack.remove(folder).is_some()
}

/// The span of `layer` a keyframe holding `object` covers, at or before
/// `frame` when several do: `(first, last)` frames.
fn instance_span(layer: &Layer, object: ObjectId, frame: u32) -> Option<(u32, u32)> {
    let keys = layer.frames.keyframes();
    let holding = |k: &&buzz_scene::Keyframe| k.objects.iter().any(|o| o.id == object);
    let key = keys
        .iter()
        .filter(holding)
        .filter(|k| k.start <= frame)
        .last()
        .or_else(|| keys.iter().find(holding))?;
    let next = keys.iter().map(|k| k.start).find(|s| *s > key.start);
    let last = next.map_or(layer.frames.length().saturating_sub(1), |n| n - 1);
    Some((key.start, last.max(key.start)))
}

/// `segment` with every object, and the rest pose, carried by `by`.
fn carried(mut segment: FrameSegment, by: Affine) -> FrameSegment {
    for key in &mut segment.keyframes {
        key.objects = Arc::new(
            key.objects
                .iter()
                .map(|o| {
                    let mut o = (**o).clone();
                    o.transform = by * o.transform;
                    Arc::new(o)
                })
                .collect(),
        );
    }
    segment
}

/// Show or hide `object` on the keyframe of `layer` starting at `start`.
fn set_shown(stack: &mut LayerStack, layer: LayerId, start: u32, object: ObjectId, shown: bool) {
    stack.update(layer, |l| {
        for key in l.frames.keyframes_mut().iter_mut().filter(|k| k.start == start) {
            for each in Arc::make_mut(&mut key.objects).iter_mut() {
                if each.id == object {
                    Arc::make_mut(each).visible = shown;
                }
            }
        }
    });
}

/// **Puppet Mode on a character placed on the open timeline** (user
/// 2026-11-17: "it shall open in main timeline, the puppet shall work in main
/// timeline, but when I remove puppet mode I shall be able to go inside the
/// symbol and there I will have all layers").
///
/// The character's part layers are lifted out of its symbol onto this
/// timeline, filed under a puppet folder where the instance's layer is: each
/// one as the instance shows it — its frames from the instance's first frame,
/// laid over the span of the keyframe holding the instance, and carried by the
/// instance's transform (the parenting's rest poses with them, so the links
/// still hold). The instance stays, hidden, and the folder names it
/// (`Layer::puppet_instance`); [`lower`] writes the poses back into the
/// symbol. Sound layers stay in the symbol. `None` (nothing changed) when
/// `instance` is not a symbol instance on the open timeline or its symbol has
/// no parts. `folder` is the new folder's id.
pub fn lift(scene: &mut Scene, instance: ObjectId, frame: u32, folder: LayerId) -> Option<LayerId> {
    let (home, object) = scene.find_object(instance)?;
    let placed = object.instance()?.clone();
    let transform = object.transform;
    let (start, last) = instance_span(scene.layers().get(home)?, instance, frame)?;
    let span = last - start + 1;
    let symbol = scene.library().get(placed.symbol)?.clone();
    let parts: Vec<Arc<Layer>> = symbol.layers.iter().filter(|l| !stays_out(l)).cloned().collect();
    if parts.is_empty() {
        return None;
    }
    // Fresh ids, the symbol's layers in the same order.
    let ids: Vec<LayerId> = parts.iter().map(|_| LayerId(scene.next_object_id().0)).collect();
    let map = |id: LayerId| parts.iter().position(|p| p.id == id).map(|i| ids[i]);
    let first = placed.first_frame;
    let lifted: Vec<Layer> = parts
        .iter()
        .zip(&ids)
        .map(|(part, id)| {
            let mut layer = (**part).clone();
            layer.id = *id;
            layer.parent = Some(part.parent.and_then(map).unwrap_or(folder));
            // The lifted folder is the puppet; one made inside the symbol
            // before is only a folder here.
            layer.puppet = false;
            layer.follows = part.follows.and_then(map);
            layer.rest_pose = part.rest_pose.map(|r| transform * r);
            // Held on its first frame, a frozen instance lifts as that one pose.
            let to = if placed.loop_mode == LoopMode::SingleFrame { first } else { first + span - 1 };
            let mut frames = LayerTimeline::from_parts(Vec::new(), start + span);
            if let Some(segment) = part.frames.segment(first.min(part.frames.length().saturating_sub(1)), to) {
                frames.overwrite(start, &carried(segment, transform));
            }
            layer.frames = frames;
            layer
        })
        .collect();

    let stack = scene.edit_layers();
    let at = stack.index_of(home)?;
    let mut puppet = Layer::new(folder, format!("{} (puppet)", symbol.name), LayerKind::Folder);
    puppet.puppet = true;
    puppet.puppet_instance = Some(instance);
    puppet.parent = stack.get(home).and_then(|l| l.parent);
    for (offset, layer) in std::iter::once(puppet).chain(lifted).enumerate() {
        stack.insert(at + offset, layer);
    }
    set_shown(stack, home, start, instance, false);
    Some(folder)
}

/// **Puppet Mode off for a lifted puppet** ([`lift`]): every part's frames
/// over the instance's span go back into the symbol from the instance's first
/// frame, its transform taken back off; a part matched to the symbol's layer
/// of the same name replaces those frames there, a new one becomes a new
/// layer. Its parenting goes with it. The folder and its parts leave this
/// timeline and the instance shows again — a frozen instance with poses keyed
/// across the span now plays them once. With the instance gone the parts stay
/// here as layers. Whether anything changed.
pub fn lower(scene: &mut Scene, folder: LayerId) -> bool {
    let Some(instance) = scene.layers().get(folder).and_then(|l| l.puppet_instance) else {
        return false;
    };
    let stack = scene.layers();
    let members: Vec<Arc<Layer>> = stack.iter().filter(|l| inside(stack, l.id, folder)).cloned().collect();
    // The keyframe it was lifted from: the lifted layers end where its span
    // does, and only there is the instance hidden.
    let end = members.iter().map(|m| m.frames.length()).max().unwrap_or(1);
    let found = scene.find_object(instance).and_then(|(home, object)| {
        let placed = object.instance()?.clone();
        let layer = scene.layers().get(home)?;
        let held: Vec<u32> = layer
            .frames
            .keyframes()
            .iter()
            .filter(|k| k.objects.iter().any(|o| o.id == instance))
            .map(|k| k.start)
            .collect();
        let hidden = held.iter().copied().find(|s| {
            layer.frames.keyframe_at(*s).is_some_and(|k| k.objects.iter().any(|o| o.id == instance && !o.visible))
        });
        let start = hidden
            .or_else(|| held.iter().copied().find(|s| instance_span(layer, instance, *s).is_some_and(|(_, l)| l + 1 == end)))
            .or_else(|| held.first().copied())?;
        let span = instance_span(layer, instance, start)?;
        let transform = layer.frames.keyframe_at(start)?.objects.iter().find(|o| o.id == instance)?.transform;
        Some((home, placed, transform, span))
    });
    let mut written = false;
    if let Some((home, placed, transform, (start, last))) = found
        && transform.determinant().abs() > 1e-12
    {
        let back = transform.inverse();
        let first = placed.first_frame;
        let mut keyed_across = false;
        let mut new_ids: Vec<LayerId> = Vec::new();
        for _ in &members {
            new_ids.push(LayerId(scene.next_object_id().0));
        }
        if let Some(symbol) = scene.library_mut().layers_mut(placed.symbol) {
            // Each member's layer in the symbol: the one of its name, else new.
            let targets: Vec<LayerId> = members
                .iter()
                .zip(&new_ids)
                .map(|(m, fresh)| {
                    symbol
                        .iter()
                        .find(|l| l.name == m.name && l.kind == m.kind && !stays_out(l))
                        .map_or(*fresh, |l| l.id)
                })
                .collect();
            let target_of = |id: LayerId| members.iter().position(|m| m.id == id).map(|i| targets[i]);
            let to = if placed.loop_mode == LoopMode::SingleFrame { first } else { first + (last - start) };
            for (member, target) in members.iter().zip(&targets) {
                let parent = member.parent.filter(|p| *p != folder).and_then(target_of);
                let follows = member.follows.and_then(target_of);
                // **Only what was changed goes back.** A part left as it was
                // lifted is not rewritten: rewriting it would re-time the
                // symbol (its length, its keys) for every other instance.
                let posed = member.frames.segment(start, last);
                if let Some(existing) = symbol.get(*target) {
                    let as_lifted: Option<FrameSegment> = existing
                        .frames
                        .segment(first.min(existing.frames.length().saturating_sub(1)), to)
                        .map(|s| carried(s, transform));
                    let same = as_lifted.as_ref().map(|s| &s.keyframes) == posed.as_ref().map(|s| &s.keyframes);
                    if same && existing.parent == parent && existing.follows == follows {
                        continue;
                    }
                }
                let segment = posed.map(|s| carried(s, back));
                keyed_across |= segment.as_ref().is_some_and(|s| s.keyframes.len() > 1);
                let rest = member.rest_pose.map(|r| back * r);
                if symbol.get(*target).is_none() {
                    let mut layer = (**member).clone();
                    layer.id = *target;
                    layer.frames = LayerTimeline::from_parts(Vec::new(), 1);
                    symbol.insert(0, layer);
                }
                symbol.update(*target, |l| {
                    if let Some(segment) = &segment {
                        l.frames.overwrite(first, segment);
                    }
                    l.parent = parent;
                    l.follows = follows;
                    l.follows_bone = member.follows_bone;
                    l.rest_pose = rest;
                    l.visible = member.visible;
                    l.locked = member.locked;
                });
            }
        }
        let stack = scene.edit_layers();
        set_shown(stack, home, start, instance, true);
        if placed.loop_mode == LoopMode::SingleFrame && keyed_across {
            stack.update(home, |l| {
                for key in l.frames.keyframes_mut() {
                    for each in Arc::make_mut(&mut key.objects).iter_mut() {
                        if each.id == instance
                            && let ObjectKind::Instance(i) = &mut Arc::make_mut(each).kind
                        {
                            i.loop_mode = LoopMode::PlayOnce;
                        }
                    }
                }
            });
        }
        written = true;
    }
    let stack = scene.edit_layers();
    if !written {
        // The instance is gone (or cannot be undone): the poses stay here as
        // layers rather than being lost with the folder.
        stack.update(folder, |l| l.puppet_instance = None);
        return unpuppet(stack, folder);
    }
    for member in &members {
        stack.remove(member.id);
    }
    stack.remove(folder);
    true
}

/// Does `stack` look like a character's own timeline: part layers named for
/// a head, a body or the arms.
pub fn is_character_stack(stack: &LayerStack) -> bool {
    stack.iter().any(|l| {
        let n: String = l.name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        matches!(n.as_str(), "head" | "body" | "rightarm" | "leftarm")
    })
}

/// The ease a puppet's poses are tweened with: Animate's ease in and out,
/// so a limb starts and settles rather than snapping.
pub fn pose_ease() -> Tween {
    Tween { easing: Easing::CubicBezier { x1: 0.42, y1: 0.0, x2: 0.58, y2: 1.0 }, ..Tween::classic() }
}

/// **Tween the poses either side of `frame`** on `layer`: the keyframe before
/// eases into the pose keyed on `frame`, and that pose eases into the next
/// one when there is one. A tween already set (a hand-made one) is kept.
pub fn ease_around(scene: &mut Scene, layer: LayerId, frame: u32) {
    scene.update_layer(layer, |l| {
        let starts: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).collect();
        let before = starts.iter().copied().filter(|s| *s < frame).max();
        let after = starts.iter().copied().any(|s| s > frame);
        let ease = pose_ease();
        if let Some(b) = before
            && !l.frames.tween_at(b).is_active()
        {
            l.frames.set_tween(b, ease);
        }
        if after && starts.contains(&frame) && !l.frames.tween_at(frame).is_active() {
            l.frames.set_tween(frame, ease);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character() -> Scene {
        let mut scene = Scene::default();
        let first = scene.layers().iter().next().unwrap().id;
        scene.update_layer(first, |l| l.name = "body".into());
        let arm = scene.add_layer("rightArm", LayerKind::Normal);
        scene.update_layer(arm, |l| l.follows = Some(first));
        let audio = scene.add_layer("Master Audio", LayerKind::Normal);
        scene.update_layer(audio, |l| l.audio_guide = true);
        scene
    }

    /// **Made and unmade without touching a part**: the parts go under one
    /// collapsed puppet folder, parenting and order kept, the master audio
    /// outside; undone, they are back exactly as they were.
    #[test]
    fn a_puppet_files_the_parts_and_undoing_it_puts_them_back() {
        let mut scene = character();
        let before: Vec<(String, Option<LayerId>)> = scene.layers().iter().map(|l| (l.name.clone(), l.follows)).collect();
        assert!(is_character_stack(scene.layers()));
        let id = LayerId(scene.next_object_id().0);
        let folder = scene.edit_open_layers(|stack| puppetize(stack, id, "Puppet")).expect("made");
        let layers = scene.layers();
        let puppet = layers.get(folder).unwrap();
        assert!(puppet.puppet && puppet.collapsed && puppet.kind == LayerKind::Folder);
        for l in layers.iter() {
            match l.name.as_str() {
                "body" | "rightArm" => assert_eq!(puppet_of(&scene, l.id), Some(folder), "{} is a part", l.name),
                "Master Audio" => assert_eq!(puppet_of(&scene, l.id), None, "the audio stays out"),
                _ => {}
            }
        }
        assert!(scene.edit_open_layers(|stack| unpuppet(stack, folder)));
        let after: Vec<(String, Option<LayerId>)> = scene.layers().iter().map(|l| (l.name.clone(), l.follows)).collect();
        assert_eq!(after, before, "back as it was");
        assert!(scene.layers().iter().all(|l| l.parent.is_none()));
    }

    /// **A pose keyed between two others eases in and out of both.**
    #[test]
    fn a_new_pose_is_eased_from_the_one_before_and_into_the_next() {
        let mut scene = character();
        let arm = scene.layers().iter().find(|l| l.name == "rightArm").unwrap().id;
        scene.update_layer(arm, |l| {
            l.frames.insert_frame(40);
            l.frames.insert_keyframe(20);
            l.frames.insert_keyframe(40);
        });
        ease_around(&mut scene, arm, 20);
        let frames = &scene.layers().get(arm).unwrap().frames;
        assert_eq!(frames.tween_at(0), pose_ease(), "into the new pose");
        assert_eq!(frames.tween_at(20), pose_ease(), "and on to the next");
        assert!(!frames.tween_at(40).is_active(), "the last pose holds");
    }
}
