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

use buzz_scene::{Easing, LayerId, LayerKind, LayerStack, Scene, Tween};

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
