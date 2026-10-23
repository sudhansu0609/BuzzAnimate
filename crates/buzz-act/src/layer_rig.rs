//! **Which rig is this?** — and the rest geometry of a rig made by layer
//! parenting, so a layer rig can be performed like a bone rig.
//!
//! A character is rigged one of two ways. A **bone rig** is one object, an
//! armature, with its parts inside it. A **layer rig** (Animate's Layer
//! Parenting, `RigKind::Layers`) is one layer per part, each named after its
//! slot in a [`RigPattern`], each part turning about its transformation point,
//! each layer following the layer of the part it hangs off.
//!
//! [`rig_of`] is the one place that tells the two apart. The performer, the
//! episode builder, the director and the agent's tools all ask it, so nothing
//! branches on the kind of rig twice.
//!
//! # Performing a layer rig
//!
//! [`rest_armature`] builds an armature in memory from the layers' rest
//! geometry — heads on the parts' pivots, tips on the pivot of the part that
//! continues the chain (or, at the end of a chain, along the part's own
//! drawing, measured as the auto-rigger measures it). The pose functions
//! ([`crate::perform::pose_on`], [`crate::perform::solve_arm`], handedness)
//! then work on it unchanged, and each keyed frame's pose becomes, for every
//! part layer, the transform that puts the part where the same bone would put
//! it — given what the layer already inherits from the layers it follows.

use std::sync::Arc;

use buzz_geom::{Affine, Point, Vec2};
use buzz_rig::{Armature, RigPattern, Side};
use buzz_scene::{LayerId, Object, ObjectId, ObjectKind, Scene};

/// What kind of rig a character is.
#[derive(Debug, Clone, PartialEq)]
pub enum CharacterRig {
    /// One armature object, holding its parts.
    Armature(ObjectId),
    /// Parts on followed layers named after their slots.
    Layers(LayerRigView),
}

impl CharacterRig {
    /// Can a performance drive it? An armature needs a person's bones (see
    /// [`crate::is_figure`]); a layer rig has them by its slot names.
    pub fn performable(&self, scene: &Scene) -> bool {
        match self {
            CharacterRig::Armature(id) => scene
                .find_object(*id)
                .is_some_and(|(_, object)| crate::is_figure(object)),
            CharacterRig::Layers(_) => true,
        }
    }

    /// The object that stands for the whole character: the armature, or the
    /// root part of a layer rig.
    pub fn body(&self) -> ObjectId {
        match self {
            CharacterRig::Armature(id) => *id,
            CharacterRig::Layers(view) => view.body,
        }
    }
}

/// A layer rig, read back from the document.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRigView {
    /// The root part's layer: the one the rest follow, directly or not.
    pub root: LayerId,
    /// The root part.
    pub body: ObjectId,
    /// The built-in pattern whose slot names the layers carry.
    pub pattern: RigPattern,
    /// `(slot, layer, part)` for every part layer, each followed layer before
    /// its followers (the root first). A slot can have several layers — a
    /// converted bone rig puts every part of a bone on a layer of its own.
    pub parts: Vec<(usize, LayerId, ObjectId)>,
}

impl LayerRigView {
    /// Every part layer, root first.
    pub fn layers(&self) -> impl Iterator<Item = LayerId> + '_ {
        self.parts.iter().map(|(_, layer, _)| *layer)
    }

    /// Is any part in this slot?
    pub fn has_slot(&self, slot: usize) -> bool {
        self.parts.iter().any(|(s, _, _)| *s == slot)
    }
}

/// **The one "which rig is this" helper.** `object` is an armature, or any
/// part of a layer rig (the root part, an arm, an eye); `None` for anything
/// else, or an object that is not in the scene.
pub fn rig_of(scene: &Scene, object: ObjectId) -> Option<CharacterRig> {
    let (layer, found) = scene.find_object(object)?;
    if matches!(found.kind, ObjectKind::Armature(_)) {
        return Some(CharacterRig::Armature(object));
    }
    rig_of_layer(scene, layer).map(CharacterRig::Layers)
}

/// The layer rig `layer` belongs to: its whole follow tree, the slot of each
/// layer read from its name in the built-in pattern that names the most of
/// them. `None` when the layer neither follows nor is followed, or when no
/// layer in the tree is named after a slot.
pub fn rig_of_layer(scene: &Scene, layer: LayerId) -> Option<LayerRigView> {
    let layers = scene.layers();
    layers.get(layer)?;
    let mut top = layer;
    // Bounded by the layer count, against a follow cycle in a corrupt file.
    for _ in 0..layers.len() {
        match layers.get(top).and_then(|l| l.follows) {
            Some(up) if up != layer && layers.get(up).is_some() => top = up,
            _ => break,
        }
    }
    // Breadth first, so a followed layer always comes before its followers.
    let mut tree = vec![top];
    let mut index = 0;
    while index < tree.len() {
        for child in layers.followers_of(tree[index]) {
            if !tree.contains(&child) {
                tree.push(child);
            }
        }
        index += 1;
    }
    if tree.len() < 2 {
        return None;
    }

    let names: Vec<&str> = tree
        .iter()
        .filter_map(|id| layers.get(*id).map(|l| l.name.as_str()))
        .collect();
    let pattern = RigPattern::builtin()
        .into_iter()
        .map(|p| (names.iter().filter(|n| p.slot_for(n).is_some()).count(), p))
        .filter(|(hits, _)| *hits > 0)
        .min_by(|(a, pa), (b, pb)| b.cmp(a).then(pa.slots.len().cmp(&pb.slots.len())))
        .map(|(_, p)| p)?;

    let mut parts = Vec::new();
    for id in tree {
        let Some(l) = layers.get(id) else {
            continue;
        };
        let Some(slot) = pattern.slot_for(&l.name) else {
            continue;
        };
        let Some(object) = l.frames.keyframes().first().and_then(|k| k.objects.first()) else {
            continue;
        };
        parts.push((slot, id, object.id));
    }
    let (_, root, body) = *parts.first()?;
    Some(LayerRigView {
        root,
        body,
        pattern,
        parts,
    })
}

/// Invert an affine, or `None` if it is singular.
fn invert(t: Affine) -> Option<Affine> {
    let c = t.as_coeffs();
    let determinant = c[0] * c[3] - c[1] * c[2];
    (determinant.abs() > 1e-12).then(|| t.inverse())
}

/// A layer's rest transform: the pose recorded when it was linked, or — for a
/// layer nothing follows, which has none — its first keyframe's first object,
/// the same fallback [`buzz_scene::LayerStack::motion_of`] reads.
fn rest_of(scene: &Scene, layer: LayerId) -> Option<Affine> {
    let l = scene.layers().get(layer)?;
    l.rest_pose.or_else(|| {
        l.frames
            .keyframes()
            .first()
            .and_then(|k| k.objects.first())
            .map(|o| o.transform)
    })
}

/// **Record the rest of every part layer that has none**, before anything is
/// keyed over it. A followed layer's motion is measured from its first
/// keyframe when it has no recorded rest, and a performance rewrites that
/// keyframe; recording it first keeps every later performance measuring from
/// the same rest. Changes nothing that is drawn.
pub fn record_rests(scene: &mut Scene, view: &LayerRigView) {
    for (_, layer, _) in &view.parts {
        let has = scene.layers().get(*layer).is_some_and(|l| l.rest_pose.is_some());
        if has {
            continue;
        }
        if let Some(rest) = rest_of(scene, *layer) {
            scene.update_layer(*layer, |l| l.rest_pose = Some(rest));
        }
    }
}

/// One part of a layer rig at rest.
#[derive(Debug, Clone, PartialEq)]
pub struct PartRest {
    pub slot: usize,
    pub layer: LayerId,
    pub object: ObjectId,
    /// The part's transform at rest, in the rig's rest space.
    pub rest: Affine,
}

/// **An armature standing where the layer rig stands at rest**, one bone per
/// pattern slot, and the rest of every part.
///
/// Each filled slot's bone starts at its part's pivot. It ends at the pivot of
/// the child part that continues the chain — the child on the same side
/// (a spine's child on the spine, a left arm's on the left arm), lowest slot
/// first — or, with none, where the auto-rigger's measure of the part's own
/// drawing ends ([`crate::autorig::fit_slots`]). Empty slots get the pattern's
/// own bone, as [`crate::assemble_from_joints`] gives them. A layer rig
/// carries no joint limits, so the armature has none.
pub fn rest_armature(scene: &Scene, view: &LayerRigView) -> Option<(Armature, Vec<PartRest>)> {
    let pattern = &view.pattern;
    let count = pattern.slots.len();
    let mut parts: Vec<PartRest> = Vec::new();
    let mut drawn: Vec<(usize, Arc<Object>)> = Vec::new();
    let mut pivots: Vec<Option<Point>> = vec![None; count];
    for &(slot, layer, object) in &view.parts {
        if slot >= count {
            continue;
        }
        let Some(rest) = rest_of(scene, layer) else {
            continue;
        };
        let Some(art) = scene
            .layers()
            .get(layer)
            .and_then(|l| l.frames.keyframes().first())
            .and_then(|k| k.objects.iter().find(|o| o.id == object))
        else {
            continue;
        };
        let mut at_rest = (**art).clone();
        at_rest.transform = rest;
        let pivot = match at_rest.pivot {
            Some(local) => rest * local,
            // A part with no transformation point turns about its middle.
            None => at_rest.bounds().center(),
        };
        if pivots[slot].is_none() {
            pivots[slot] = Some(pivot);
        }
        drawn.push((slot, Arc::new(at_rest)));
        parts.push(PartRest {
            slot,
            layer,
            object,
            rest,
        });
    }
    if parts.is_empty() {
        return None;
    }

    let fitted = crate::autorig::fit_slots(pattern, &drawn);
    let aim = pattern.world_angles();
    let same_side = |a: Side, b: Side| {
        matches!(
            (a, b),
            (Side::Either, Side::Either) | (Side::Left, Side::Left) | (Side::Right, Side::Right)
        )
    };
    let mut joints: Vec<Option<(Point, Point)>> = vec![None; count];
    for slot in 0..count {
        let Some(head) = pivots[slot] else {
            continue;
        };
        let chain = pattern
            .slots
            .iter()
            .enumerate()
            .filter(|(c, s)| {
                s.parent == Some(slot)
                    && pivots[*c].is_some()
                    && same_side(pattern.slots[slot].side, s.side)
            })
            .map(|(c, _)| c)
            .min();
        let along = match (chain.and_then(|c| pivots[c]), fitted[slot]) {
            (Some(next), _) if (next - head).hypot() > 1e-6 => next - head,
            (_, Some((h, t))) if (t - h).hypot() > 1e-6 => t - h,
            // Nothing to measure: a short bone the way the pattern points.
            _ => Vec2::new(aim[slot].cos(), aim[slot].sin()),
        };
        joints[slot] = Some((head, head + along));
    }

    let data = crate::autorig::assemble_from_joints(pattern, &joints, &drawn)?;
    let mut armature = data.armature;
    for bone in &mut armature.bones {
        bone.limits = None;
    }
    Some((armature, parts))
}

/// **Where the character stands**: the transform from its rig's own space to
/// the stage. An armature's is its object's transform; a layer rig's is how
/// far its root part has been moved from rest (with whatever the root
/// inherits). `None` for an object that is not there; an object that is not a
/// rig answers with its own transform.
pub fn placement(scene: &Scene, object: ObjectId) -> Option<Affine> {
    let (_, found) = scene.find_object(object)?;
    match rig_of(scene, object) {
        Some(CharacterRig::Layers(view)) => {
            let (_, root) = scene.find_object(view.body)?;
            let rest = rest_of(scene, view.root)?;
            let inherited = scene.layers().inherited_transform(view.root, 0u32);
            Some(inherited * root.transform * invert(rest).unwrap_or(Affine::IDENTITY))
        }
        _ => Some(found.transform),
    }
}

/// Stand the character at `placed` (see [`placement`]) on the keyframe that
/// owns `frame`. A layer rig moves its root part; the rest follow it.
pub fn set_placement(scene: &mut Scene, object: ObjectId, frame: u32, placed: Affine) {
    match rig_of(scene, object) {
        Some(CharacterRig::Layers(view)) => {
            let Some(rest) = rest_of(scene, view.root) else {
                return;
            };
            let inherited = scene.layers().inherited_transform(view.root, frame);
            let own = invert(inherited).unwrap_or(Affine::IDENTITY) * placed * rest;
            scene.update_object_at(frame, view.body, |o| o.transform = own);
        }
        _ => {
            scene.update_object_at(frame, object, |o| o.transform = placed);
        }
    }
}

/// **A keyframe on `layer` at `frame` holding what is drawn there now** —
/// the tweened in-between, not a copy of the key before — and carrying on
/// with the same tween. For a part of a layer rig, where a key on one layer
/// alone (an eye shutting, a mouth changing) must not hold that part still
/// while the parts round it keep moving.
///
/// Returns whether a keyframe was made.
pub fn key_at(scene: &mut Scene, layer: LayerId, frame: u32) -> bool {
    let Some(l) = scene.layers().get(layer) else {
        return false;
    };
    if l.frames.is_keyframe(frame) || frame >= l.frames.length() {
        return false;
    }
    let now: Vec<Arc<Object>> = l
        .frames
        .resolved_at(frame)
        .iter()
        .map(|o| Arc::new(o.clone()))
        .collect();
    let tween = l.frames.tween_at(frame);
    let mut made = false;
    scene.update_layer(layer, |l| {
        made = l.frames.insert_keyframe(frame);
        if made {
            l.frames.set_objects(frame, now);
            l.frames.set_tween(frame, tween);
        }
    });
    made
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::{LayerKind, ShapeData};
    use buzz_geom::{Rect, Shape as _};

    fn part(scene: &mut Scene, name: &str, bar: Rect, pivot: Point) -> (LayerId, ObjectId) {
        let layer = scene.add_layer(name, LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(bar.to_path(1e-9), peniko::Color::WHITE),
            )
            .expect("a part");
        scene.update_object(id, |o| o.pivot = Some(pivot));
        (layer, id)
    }

    #[test]
    fn a_layer_rig_is_recognised_from_any_of_its_parts_and_an_armature_as_itself() {
        let mut scene = Scene::default();
        let (chest, chest_id) = part(&mut scene, "Chest", Rect::new(0.0, 0.0, 20.0, 100.0), Point::new(10.0, 100.0));
        let (arm, arm_id) = part(&mut scene, "Shoulder L", Rect::new(20.0, 0.0, 80.0, 10.0), Point::new(20.0, 5.0));
        let (_, loose) = part(&mut scene, "Prop", Rect::new(0.0, 0.0, 5.0, 5.0), Point::ZERO);
        scene.set_follows(arm, Some(chest), 0);

        let Some(CharacterRig::Layers(view)) = rig_of(&scene, arm_id) else {
            panic!("the arm's layer rig was not found");
        };
        assert_eq!(view.root, chest);
        assert_eq!(view.body, chest_id);
        assert_eq!(view.parts.len(), 2);
        assert_eq!(rig_of(&scene, chest_id), Some(CharacterRig::Layers(view)));
        assert_eq!(rig_of(&scene, loose), None, "a lone layer is not a rig");
    }
}
