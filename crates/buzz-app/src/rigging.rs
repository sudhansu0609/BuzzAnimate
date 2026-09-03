//! The Bone and Asset Warp tools: building a rig, posing it, warping artwork.
//!
//! # Why these gestures live outside the tool machine
//!
//! Every other tool works on the pointer alone — a rectangle needs two corners
//! and nothing else. Rigging needs to know what is *under* the pointer before
//! the drag begins: whether this is a new bone, a child of an existing one, or
//! a bone being posed. That is a document query, and the tool machine
//! deliberately cannot reach the document. So the editor answers the question
//! first and starts the gesture itself.
//!
//! # What each drag does, following Animate
//!
//! * **Bone tool on unrigged artwork** — creates an armature and the first
//!   bone, and binds the artwork to it.
//! * **Bone tool from a bone's tip** — adds a child bone, so a chain is built
//!   by dragging one bone after another, as in Animate.
//! * **Bone tool on a bone** — poses it with inverse kinematics, so the chain
//!   above it follows the drag.
//! * **Asset Warp on artwork** — turns it into warped artwork with a grid of
//!   handles; dragging a handle moves it and the artwork follows.
//!
//! Animate poses with the Selection tool rather than the Bone tool. Posing is
//! on the Bone tool here as well as building, because the Selection tool
//! already means "move the whole object" and a rig you cannot move as a whole
//! would be worse than one you pose with the tool you built it with. Recorded
//! as a deviation rather than left to be discovered.
//!
//! # The other way to build a rig
//!
//! Everything above is a bone at a time. The foot of this file is the other
//! way — sorting a character's drawings into the named slots of a
//! [`RigPattern`] and building the whole skeleton at once. That is what the
//! Rigging panel drives, and it is here rather than in the panel for the
//! reason everything else here is: it is a document edit, and the panel cannot
//! reach the document.

use std::sync::Arc;

use buzz_geom::{Affine, Point, Vec2};
use buzz_rig::{Armature, Bone, IkOptions};
use buzz_rig::RigPattern;
use buzz_scene::{ArmatureData, Object, ObjectId, ObjectKind, Scene, WarpData};

/// Invert an affine, or `None` if it is singular.
///
/// A singular transform — an object scaled to nothing — has no inverse, and
/// every rigging gesture needs one to work in the object's own coordinates.
/// Returning `None` makes that a gesture that does nothing rather than a
/// division by zero that scatters the rig across the stage.
fn invert(t: buzz_geom::Affine) -> Option<buzz_geom::Affine> {
    let c = t.as_coeffs();
    let determinant = c[0] * c[3] - c[1] * c[2];
    (determinant.abs() > 1e-12).then(|| t.inverse())
}

/// How close a click must come to a bone or handle, in screen pixels.
pub const GRAB_PX: f64 = 8.0;

/// A rigging drag in progress.
#[derive(Debug, Clone, PartialEq)]
pub enum RigGesture {
    /// Dragging out a new bone from `head`.
    Building {
        object: Option<ObjectId>,
        parent: Option<usize>,
        head: Point,
        current: Point,
    },
    /// Posing an existing bone with IK.
    Posing {
        object: ObjectId,
        bone: usize,
        current: Point,
    },
    /// Dragging a warp handle.
    Warping {
        object: ObjectId,
        handle: usize,
        current: Point,
    },
}

/// What is under the pointer, as far as rigging is concerned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RigTarget {
    /// A bone's tip: dragging from here adds a child bone.
    BoneTip(ObjectId, usize),
    /// A bone's body: dragging poses it.
    Bone(ObjectId, usize),
    /// A warp handle.
    Handle(ObjectId, usize),
    /// Artwork with no rig yet.
    Artwork(ObjectId),
    /// Empty space.
    Nothing,
}

/// What lies under `point`, preferring bone tips to bone bodies.
///
/// Tips win because a tip is also part of the bone: without the preference,
/// building a chain would be impossible — every attempt to extend a bone would
/// pose it instead.
pub fn target_at(scene: &Scene, frame: u32, point: Point, tolerance: f64) -> RigTarget {
    let mut artwork = None;

    for layer in scene.layers().selectable() {
        for object in layer.objects_at(frame).iter().rev() {
            if !object.visible || object.locked {
                continue;
            }
            let Some(inverse) = invert(object.transform) else {
                continue;
            };
            let local = inverse * point;

            match &object.kind {
                ObjectKind::Armature(rig) => {
                    for (index, (_, tip)) in rig.segments().iter().enumerate() {
                        if (local - *tip).hypot() <= tolerance {
                            return RigTarget::BoneTip(object.id, index);
                        }
                    }
                    if let Some((index, distance)) = rig.armature.nearest_bone(local)
                        && distance <= tolerance
                    {
                        return RigTarget::Bone(object.id, index);
                    }
                }
                ObjectKind::Warp(warp) => {
                    if let Some((index, distance)) = warp.nearest_handle(local)
                        && distance <= tolerance
                    {
                        return RigTarget::Handle(object.id, index);
                    }
                }
                _ => {
                    if artwork.is_none() && object.bounds().contains(point) {
                        artwork = Some(object.id);
                    }
                }
            }
        }
    }

    match artwork {
        Some(id) => RigTarget::Artwork(id),
        None => RigTarget::Nothing,
    }
}

/// Wrap a shape in an armature, with a first bone from `head` to `tip`.
///
/// The artwork moves *into* the armature rather than being copied, the same
/// way F8 moves a selection into a symbol: two copies of the same drawing, one
/// rigged and one not, is not what anybody means by rigging it.
pub fn rig_object(
    scene: &mut Scene,
    frame: u32,
    object: ObjectId,
    head: Point,
    tip: Point,
) -> bool {
    // The artwork inside needs an id of its own: ids are unique across the
    // whole document, and reusing the wrapper's would put the same number in
    // two places — which is exactly what the importer's round-trip test
    // guards against.
    let inner_id = scene.next_object_id();
    let mut rigged = false;

    scene.update_object_at(frame, object, |target| {
        let Some(inverse) = invert(target.transform) else {
            return;
        };
        // Bones are built in the object's own coordinates, so the rig follows
        // its artwork when the object is later moved or scaled.
        let mut armature = Armature::new(inverse * head);
        armature.push_dragged("Bone 1", None, inverse * head, inverse * tip);

        // The artwork moves *into* the armature rather than being copied.
        let artwork = std::mem::replace(&mut target.kind, ObjectKind::Group(Vec::new()));
        let inner = buzz_scene::Object {
            id: inner_id,
            name: None,
            transform: buzz_geom::Affine::IDENTITY,
            kind: artwork,
            locked: false,
            visible: true,
            filters: Vec::new(),
            blend: buzz_scene::Blend::Normal,
            spatial: Default::default(),
            pivot: None,
            modifiers: Vec::new(),
            text: None,
            turnaround: Default::default(),
        };

        let mut rig = ArmatureData::new(armature);
        rig.bind_shape(std::sync::Arc::new(inner));
        target.kind = ObjectKind::Armature(rig);
        rigged = true;
    });
    rigged
}

/// Add a child bone to an existing armature.
pub fn add_bone(
    scene: &mut Scene,
    frame: u32,
    object: ObjectId,
    parent: Option<usize>,
    head: Point,
    tip: Point,
) {
    scene.update_object_at(frame, object, |target| {
        let ObjectKind::Armature(rig) = &mut target.kind else {
            return;
        };
        let Some(inverse) = invert(target.transform) else {
            return;
        };
        let name = format!("Bone {}", rig.armature.len() + 1);
        rig.armature
            .push_dragged(name, parent, inverse * head, inverse * tip);
        // Weights are relative to the skeleton, so a new bone means new
        // weights: without this the artwork would ignore the bone just added.
        rig.armature.set_rest_here();
        rig.rebind();
    });
}

/// Pose a bone so its tip follows `target`.
pub fn pose_bone(scene: &mut Scene, frame: u32, object: ObjectId, bone: usize, target: Point) {
    scene.update_object_at(frame, object, |object_ref| {
        let Some(inverse) = invert(object_ref.transform) else {
            return;
        };
        let local = inverse * target;
        let ObjectKind::Armature(rig) = &mut object_ref.kind else {
            return;
        };
        buzz_rig::solve_to(&mut rig.armature, bone, local, &IkOptions::default());
    });
}

/// Turn a shape into warped artwork with a starting grid of handles.
pub fn warp_object(
    scene: &mut Scene,
    frame: u32,
    object: ObjectId,
    columns: usize,
    rows: usize,
) -> bool {
    let mut warped = false;
    scene.update_object_at(frame, object, |target| {
        // Only a single path can be warped point by point. A group would have
        // to be flattened first, which is the user's decision rather than
        // something to do behind their back.
        let ObjectKind::Shape(shape) = &target.kind else {
            return;
        };
        target.kind = ObjectKind::Warp(WarpData::new(shape.clone()).with_grid(columns, rows));
        warped = true;
    });
    warped
}

/// Move a warp handle.
pub fn move_handle(scene: &mut Scene, frame: u32, object: ObjectId, handle: usize, to: Point) {
    scene.update_object_at(frame, object, |target| {
        let Some(inverse) = invert(target.transform) else {
            return;
        };
        let local = inverse * to;
        let ObjectKind::Warp(warp) = &mut target.kind else {
            return;
        };
        if let Some(h) = warp.handles.get_mut(handle) {
            h.current = local;
        }
    });
}

/// Add a handle to warped artwork.
pub fn add_handle(scene: &mut Scene, frame: u32, object: ObjectId, at: Point) {
    scene.update_object_at(frame, object, |target| {
        let Some(inverse) = invert(target.transform) else {
            return;
        };
        let local = inverse * at;
        if let ObjectKind::Warp(warp) = &mut target.kind {
            warp.add_handle(local);
        }
    });
}

/// Where a bone's head and tip sit on the stage, for drawing the rig.
pub fn stage_segments(scene: &Scene, frame: u32) -> Vec<(ObjectId, Vec<(Point, Point)>)> {
    let mut out = Vec::new();
    for layer in scene.layers().drawable_at(frame) {
        for object in layer.objects_at(frame).iter() {
            if let ObjectKind::Armature(rig) = &object.kind {
                let segments = rig
                    .segments()
                    .into_iter()
                    .map(|(head, tip)| (object.transform * head, object.transform * tip))
                    .collect();
                out.push((object.id, segments));
            }
        }
    }
    out
}

/// Where warp handles sit on the stage.
pub fn stage_handles(scene: &Scene, frame: u32) -> Vec<(ObjectId, Vec<(Point, bool)>)> {
    let mut out = Vec::new();
    for layer in scene.layers().drawable_at(frame) {
        for object in layer.objects_at(frame).iter() {
            if let ObjectKind::Warp(warp) = &object.kind {
                let handles = warp
                    .handles
                    .iter()
                    .map(|h| (object.transform * h.current, h.is_moved()))
                    .collect();
                out.push((object.id, handles));
            }
        }
    }
    out
}

/// A bone's direction, for drawing it as Animate's tapered shape.
pub fn bone_outline(head: Point, tip: Point, width: f64) -> [Point; 4] {
    let along = tip - head;
    let length = along.hypot();
    if length <= f64::EPSILON {
        return [head; 4];
    }
    let unit = along / length;
    let across = Vec2::new(-unit.y, unit.x) * width;
    // A quarter of the way along is where Animate's bone is widest, which is
    // what makes the direction of a bone readable at a glance.
    let shoulder = head + along * 0.25;
    [head, shoulder + across, tip, shoulder - across]
}

/// A default bone name for a rig built by dragging.
pub fn next_bone_name(armature: &Armature) -> String {
    format!("Bone {}", armature.len() + 1)
}

/// The bone the Properties panel should show for a selection.
pub fn selected_bone(scene: &Scene, object: ObjectId) -> Option<Bone> {
    let (_, found) = scene.find_object(object)?;
    match &found.kind {
        ObjectKind::Armature(rig) => rig.armature.bones.first().cloned(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rigging a character by sorting its drawings into a pattern.
// ---------------------------------------------------------------------------

/// Every drawing on the stage that could still become a limb.
///
/// **In paint order, back to front**, because that is the order
/// [`buzz_act::assemble`] wants: the layer stack the animator arranged is what
/// decides whether the head draws in front of the shoulders, and binding in
/// slot order instead throws it away.
///
/// Anything already rigged or warped is left out — it is not a loose part, it
/// is a rig — and so is anything on a locked or hidden layer, which is the same
/// rule [`target_at`] uses for what a click can reach.
pub fn loose_parts(scene: &Scene, frame: u32) -> Vec<buzz_ui::LoosePart> {
    let mut out = Vec::new();
    for layer in scene.layers().selectable() {
        let objects = layer.objects_at(frame);
        // A layer holding one drawing lends it its name, which is what makes
        // auto-assignment work on an import at all: Photoshop and Animate both
        // put the name of the part on the *layer*, not on the artwork.
        let alone = objects.len() == 1;
        for object in objects {
            if !object.visible
                || object.locked
                || matches!(object.kind, ObjectKind::Armature(_) | ObjectKind::Warp(_))
            {
                continue;
            }
            let name = object
                .name
                .clone()
                .or_else(|| alone.then(|| layer.name.clone()))
                .unwrap_or_else(|| format!("Drawing {}", object.id.0));
            out.push(buzz_ui::LoosePart {
                object: object.id,
                layer: layer.id,
                name,
            });
        }
    }
    out
}

/// The topmost unrigged drawing under `at`, for filling an armed slot.
///
/// Front to back, so what is clicked is what is on top — the same rule a
/// selection follows, and the reason clicking a slot then clicking the stage
/// exists at all: on a character standing in a rest pose the arms are over the
/// body, and a drag cannot choose between two things under one pixel.
pub fn part_at(scene: &Scene, frame: u32, at: Point) -> Option<buzz_ui::LoosePart> {
    loose_parts(scene, frame).into_iter().rev().find(|part| {
        scene
            .find_object(part.object)
            .is_some_and(|(_, object)| object.bounds().contains(at))
    })
}

/// Build a skeleton from `pattern` and move the sorted drawings into it.
///
/// `slots` says, for each slot of the pattern, which drawing was put in it.
/// Returns the new armature, or `None` if nothing was sorted — which is a rig
/// with nothing in it rather than an error worth a type of its own.
///
/// Nothing is taken out of its layer until the armature has actually been
/// assembled, so a refusal leaves the artwork exactly where it was rather than
/// half consumed. The caller wraps this in one `Document::edit`: rigging a
/// character is a single decision, and an animator who regrets it should press
/// Ctrl+Z once rather than eleven times.
pub fn rig_character(
    scene: &mut Scene,
    frame: u32,
    pattern: &RigPattern,
    slots: &[Option<ObjectId>],
) -> Option<ObjectId> {
    // Walked in paint order rather than in slot order — see `loose_parts`.
    let mut ordered: Vec<(usize, ObjectId)> = Vec::new();
    let mut home = None;
    for layer in scene.layers().selectable() {
        for object in layer.objects_at(frame) {
            if let Some(slot) = slots.iter().position(|s| *s == Some(object.id)) {
                ordered.push((slot, object.id));
                // The rig lands on the layer of the frontmost part, so the
                // character stays where it was in the stack.
                home = Some(layer.id);
            }
        }
    }
    let home = home?;

    let taken: Vec<(usize, Arc<Object>)> = ordered
        .iter()
        .filter_map(|(slot, id)| scene.find_object(*id).map(|(_, art)| (*slot, art.clone())))
        .collect();
    let rig = buzz_act::assemble(pattern, &taken)?;

    // The artwork moves *into* the armature rather than being copied, the same
    // way F8 moves a selection into a symbol: two copies of one drawing, one
    // rigged and one not, is not what anybody means by rigging it.
    for (_, id) in &ordered {
        scene.remove_object(*id);
    }

    let id = scene.next_object_id();
    // Identity, because the parts inside kept the transforms they were drawn
    // with: the character does not move when it is rigged.
    scene.add_object(
        home,
        Object {
            id,
            name: None,
            transform: Affine::IDENTITY,
            kind: ObjectKind::Armature(rig),
            locked: false,
            visible: true,
            filters: Vec::new(),
            blend: Default::default(),
            spatial: Default::default(),
            pivot: None,
            modifiers: Vec::new(),
            text: None,
            turnaround: Default::default(),
        },
    )
}

/// Put a different drawing into one slot of a rig that already exists.
///
/// The bone stays exactly as it is; only the artwork on it changes. That is
/// what makes redrawing a limb cheap — the pose library, the joint limits and
/// every keyframe of animation are facts about the skeleton, and none of them
/// care which picture is riding on it.
pub fn replace_part(scene: &mut Scene, rig: ObjectId, slot: usize, drawing: ObjectId) -> bool {
    let Some((_, art)) = scene.find_object(drawing) else {
        return false;
    };
    let replacement = art.clone();

    let Some((_, holder)) = scene.find_object(rig) else {
        return false;
    };
    // Parts live in the coordinates of the armature and the drawing was
    // dropped in the coordinates of the layer. Without this a rig that had been
    // moved or scaled would fling the new part across the stage.
    let into_rig = invert(holder.transform).unwrap_or(Affine::IDENTITY);
    let ObjectKind::Armature(data) = &holder.kind else {
        return false;
    };
    // Undoing the *current* pose as well as the placement, so the drawing stays
    // where it was dropped rather than jumping by however far the bone happens
    // to be posed.
    let posed = invert(data.armature.pose_transform(slot)).unwrap_or(Affine::IDENTITY);

    scene.remove_object(drawing);

    let mut done = false;
    scene.update_object(rig, |target| {
        let ObjectKind::Armature(data) = &mut target.kind else {
            return;
        };
        let mut artwork = (*replacement).clone();
        artwork.transform = posed * into_rig * artwork.transform;
        let artwork = Arc::new(artwork);

        match data
            .parts
            .iter_mut()
            .find(|part| matches!(part.binding, buzz_scene::RigBinding::Rigid(b) if b == slot))
        {
            // In place, so the new drawing paints where the old one did.
            Some(part) => part.artwork = artwork,
            None => data.bind_rigid(artwork, slot),
        }
        done = true;
    });
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::ShapeData;
    use peniko::Color;

    fn scene_with_limb() -> (Scene, ObjectId) {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().expect("a layer").id;
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    Rect::new(100.0, 92.0, 200.0, 108.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");
        (scene, id)
    }

    #[test]
    fn rigging_a_shape_replaces_it_with_an_armature_holding_it() {
        let (mut scene, id) = scene_with_limb();
        let before = scene.shape_count();

        rig_object(
            &mut scene,
            0,
            id,
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
        );

        let (_, object) = scene.find_object(id).expect("still there");
        let ObjectKind::Armature(rig) = &object.kind else {
            panic!("the shape was not rigged");
        };
        assert_eq!(rig.armature.len(), 1);
        assert_eq!(rig.parts.len(), 1, "the artwork moved into the rig");
        assert_eq!(
            scene.shape_count(),
            before,
            "the artwork should have moved, not been copied"
        );
    }

    #[test]
    fn a_second_drag_adds_a_child_bone() {
        let (mut scene, id) = scene_with_limb();
        rig_object(
            &mut scene,
            0,
            id,
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
        );
        add_bone(
            &mut scene,
            0,
            id,
            Some(0),
            Point::new(150.0, 100.0),
            Point::new(200.0, 100.0),
        );

        let (_, object) = scene.find_object(id).expect("still there");
        let ObjectKind::Armature(rig) = &object.kind else {
            panic!("expected an armature");
        };
        assert_eq!(rig.armature.len(), 2);
        assert_eq!(rig.armature.bones[1].parent, Some(0));
        assert!((rig.armature.tip(1) - Point::new(200.0, 100.0)).hypot() < 1e-9);
    }

    #[test]
    fn posing_a_bone_moves_the_artwork_with_it() {
        let (mut scene, id) = scene_with_limb();
        rig_object(
            &mut scene,
            0,
            id,
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
        );
        add_bone(
            &mut scene,
            0,
            id,
            Some(0),
            Point::new(150.0, 100.0),
            Point::new(200.0, 100.0),
        );

        let before = scene.find_object(id).expect("there").1.bounds();
        pose_bone(&mut scene, 0, id, 1, Point::new(150.0, 190.0));
        let after = scene.find_object(id).expect("there").1.bounds();

        assert!(
            after.y1 > before.y1 + 20.0,
            "the artwork did not follow the bone: {before:?} then {after:?}"
        );
    }

    /// A tip is also part of its bone, so without the preference a chain could
    /// never be extended — every drag from the end would pose instead.
    #[test]
    fn a_bone_tip_is_found_before_the_bone_itself() {
        let (mut scene, id) = scene_with_limb();
        rig_object(
            &mut scene,
            0,
            id,
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
        );

        assert_eq!(
            target_at(&scene, 0, Point::new(150.0, 100.0), 6.0),
            RigTarget::BoneTip(id, 0)
        );
        assert_eq!(
            target_at(&scene, 0, Point::new(120.0, 100.0), 6.0),
            RigTarget::Bone(id, 0)
        );
    }

    #[test]
    fn unrigged_artwork_is_offered_for_rigging() {
        let (scene, id) = scene_with_limb();
        assert_eq!(
            target_at(&scene, 0, Point::new(150.0, 100.0), 6.0),
            RigTarget::Artwork(id)
        );
        assert_eq!(
            target_at(&scene, 0, Point::new(400.0, 400.0), 6.0),
            RigTarget::Nothing
        );
    }

    #[test]
    fn warping_a_shape_gives_it_handles_that_move_the_artwork() {
        let (mut scene, id) = scene_with_limb();
        assert!(warp_object(&mut scene, 0, id, 3, 3));

        let (_, object) = scene.find_object(id).expect("there");
        let ObjectKind::Warp(warp) = &object.kind else {
            panic!("expected a warp");
        };
        assert_eq!(warp.handles.len(), 9);

        let before = scene.find_object(id).expect("there").1.bounds();
        move_handle(&mut scene, 0, id, 0, Point::new(40.0, 40.0));
        let after = scene.find_object(id).expect("there").1.bounds();
        assert!(after.x0 < before.x0 - 10.0, "{before:?} then {after:?}");
    }

    #[test]
    fn a_warp_handle_is_found_under_the_pointer() {
        let (mut scene, id) = scene_with_limb();
        warp_object(&mut scene, 0, id, 2, 2);

        // The grid's first handle sits on the artwork's top-left corner.
        assert_eq!(
            target_at(&scene, 0, Point::new(100.0, 92.0), 6.0),
            RigTarget::Handle(id, 0)
        );
    }

    #[test]
    fn a_group_is_not_warped_rather_than_warped_wrongly() {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().expect("a layer").id;
        let child = std::sync::Arc::new(buzz_scene::Object::shape(
            ObjectId(90),
            ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
        ));
        let id = scene
            .add_object(layer, buzz_scene::Object::group(ObjectId(91), vec![child]))
            .expect("a group");

        assert!(!warp_object(&mut scene, 0, id, 3, 3));
    }

    #[test]
    fn stage_segments_report_bones_where_they_are_drawn() {
        let (mut scene, id) = scene_with_limb();
        rig_object(
            &mut scene,
            0,
            id,
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
        );

        let segments = stage_segments(&scene, 0);
        assert_eq!(segments.len(), 1);
        let (object, bones) = &segments[0];
        assert_eq!(*object, id);
        assert!((bones[0].0 - Point::new(100.0, 100.0)).hypot() < 1e-9);
        assert!((bones[0].1 - Point::new(150.0, 100.0)).hypot() < 1e-9);
    }

    #[test]
    fn a_bone_outline_is_widest_near_its_head() {
        let outline = bone_outline(Point::ZERO, Point::new(100.0, 0.0), 6.0);
        assert_eq!(outline[0], Point::ZERO);
        assert_eq!(outline[2], Point::new(100.0, 0.0));
        // The two shoulders sit a quarter along, either side.
        assert!((outline[1].x - 25.0).abs() < 1e-9);
        assert!((outline[1].y - outline[3].y).abs() > 1e-9);
    }

    #[test]
    fn a_zero_length_bone_does_not_produce_nan() {
        let outline = bone_outline(Point::new(5.0, 5.0), Point::new(5.0, 5.0), 6.0);
        assert!(outline.iter().all(|p| p.is_finite()));
    }
}
