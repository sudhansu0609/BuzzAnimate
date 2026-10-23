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
use buzz_rig::RigPattern;
use buzz_rig::{Armature, Bone, IkOptions};
use buzz_scene::{
    ArmatureData, LayerId, LayerKind, Object, ObjectId, ObjectKind, RigBinding, Scene, WarpData,
};

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
    target_at_visible(scene, frame, point, tolerance, true)
}

/// [`target_at`], with the bones out of the way when they are hidden.
///
/// **A rig you cannot see must not be a rig you can grab.** Hiding the bones is
/// how an animator gets at the artwork underneath them — the skeleton of a
/// figure covers most of it — and a hidden bone that still swallowed the click
/// would make the toggle worse than useless.
pub fn target_at_visible(
    scene: &Scene,
    frame: u32,
    point: Point,
    tolerance: f64,
    bones_shown: bool,
) -> RigTarget {
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
                ObjectKind::Armature(rig) if bones_shown => {
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
                // **A hidden rig offers nothing at all**, rather than
                // offering its artwork. Falling through to the arm below would
                // hand an already-rigged object to the Bone tool as bare
                // artwork, and the tool would rig it a second time -- throwing
                // away the skeleton the animator hid.
                ObjectKind::Armature(_) => continue,
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
            drawing_object: false,
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

        // **The rest pose is left alone.**
        //
        // This used to call `set_rest_here`, which adopts the pose the bones
        // are in *now* as the pose they were drawn in. On a rig being built
        // that is right and invisible: the first bones are laid on artwork that
        // is not going anywhere, and a bone from `push_dragged` already rests
        // at the angle it was dragged at.
        //
        // On a rig that is finished and **posed** it takes the character apart.
        // Every rigidly bound part is drawn through `pose_transform`, which
        // measures a bone against its rest — so re-resting a posed skeleton
        // makes all of those the identity and the artwork snaps back to where
        // it was drawn while the bones stay where the animator put them.
        //
        // Easy to hit, too: grabbing the end of a bone is the natural way to
        // move a limb, and the end of a bone is what extends the chain.
        //
        // Weights are still relative to the skeleton, so skinned parts are
        // re-bound against it — that part was never the problem.
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

/// The range each limited joint may turn through, on the stage, for drawing
/// its wedge: the joint's head, the edges of the range as stage angles
/// (radians, y down, sweeping clockwise from `from` to `to`), and whether the
/// joint is locked.
///
/// An edge is the parent's world angle plus the stored limit — which is the
/// bone's rest angle plus the relative range — so the wedge turns with the
/// parent as the rig is posed. A free joint has no wedge.
pub fn stage_joint_arcs(
    scene: &Scene,
    frame: u32,
) -> Vec<(ObjectId, Vec<(Point, f64, f64, bool)>)> {
    let mut out = Vec::new();
    for layer in scene.layers().drawable_at(frame) {
        for object in layer.objects_at(frame).iter() {
            let ObjectKind::Armature(rig) = &object.kind else {
                continue;
            };
            let armature = &rig.armature;
            let joints = armature.joints();
            let transform = object.transform;
            // A mirrored rig turns the other way on the stage.
            let flipped = transform.determinant() < 0.0;
            let mut arcs = Vec::new();
            for (index, bone) in armature.bones.iter().enumerate() {
                let Some(limits) = bone.limits else { continue };
                let Some(&(head, _)) = joints.get(index) else {
                    continue;
                };
                let parent = bone.parent.map_or(0.0, |p| armature.world_angle(p));
                let stage_angle = |angle: f64| {
                    let along = transform * (head + Vec2::new(angle.cos(), angle.sin()))
                        - transform * head;
                    along.y.atan2(along.x)
                };
                let span = (limits.max - limits.min).max(0.0);
                let from = if flipped {
                    stage_angle(parent + limits.max)
                } else {
                    stage_angle(parent + limits.min)
                };
                let locked = bone.rotation_mode() == buzz_rig::RotationMode::Locked;
                arcs.push((transform * head, from, from + span, locked));
            }
            if !arcs.is_empty() {
                out.push((object.id, arcs));
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

/// The bone the Bone tool last pressed on (`Editor::selected_bone`), if its
/// rig is still an armature that has that bone.
pub fn selected_bone(scene: &Scene, selected: Option<(ObjectId, usize)>) -> Option<Bone> {
    let (object, index) = selected?;
    let (_, found) = scene.find_object(object)?;
    match &found.kind {
        ObjectKind::Armature(rig) => rig.armature.bones.get(index).cloned(),
        _ => None,
    }
}

/// Apply what the joint widget asked for to a skeleton.
///
/// A range copied to another bone keeps its numbers **relative to that bone's
/// own rest angle**: "−150° … 0° from where it was drawn" means the same on
/// every bone, whatever angle each one was drawn at. The chain is the bone and
/// every bone hanging off it; nothing above it or beside it changes.
pub fn apply_joint_edit(armature: &mut Armature, edit: &buzz_ui::joint_widget::JointEdit) {
    use buzz_rig::JointLimits;
    use buzz_ui::joint_widget::JointEdit;

    let (targets, relative): (Vec<usize>, Option<JointLimits>) = match *edit {
        JointEdit::Set { index, limits } => {
            if let Some(bone) = armature.bones.get_mut(index) {
                bone.limits = limits;
            }
            return;
        }
        JointEdit::ApplyToChain { index, limits } => {
            let Some(rest) = armature.bones.get(index).map(|b| b.rest_angle) else {
                return;
            };
            let relative = limits.map(|l| {
                let (min, max) = l.relative_to(rest);
                JointLimits::new(min, max)
            });
            (armature.subtree(index), relative)
        }
        JointEdit::ApplyToAll { limits } => ((0..armature.bones.len()).collect(), limits),
    };

    for index in targets {
        if let Some(bone) = armature.bones.get_mut(index) {
            bone.limits =
                relative.map(|r| JointLimits::from_relative(bone.rest_angle, r.min, r.max));
        }
    }
}

/// The Rig panel's **Apply to this rig**: every bone the table names (by slot
/// name) gets that row's limits about its own rest angle; a bone the table
/// does not name is left as it is.
pub fn apply_default_limits(armature: &mut Armature, table: &buzz_rig::JointLimitDefaults) {
    for bone in &mut armature.bones {
        if !table.rows.contains_key(&bone.name) {
            continue;
        }
        bone.limits = table
            .limits_for(&bone.name)
            .map(|r| buzz_rig::JointLimits::from_relative(bone.rest_angle, r.min, r.max));
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
            drawing_object: false,
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

// ---------------------------------------------------------------------------
// Rigging a character by layer parenting instead of bones.
// ---------------------------------------------------------------------------

/// A character rigged by layer parenting (Animate's Layer Parenting) rather
/// than by bones: one layer per part, each part turning about its joint, each
/// layer following the layer of the part it hangs off.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRig {
    /// `(slot, layer)` for every part, back to front. The slot is an index
    /// into the pattern (or into the armature's bones, for a converted rig);
    /// the layer is named after it.
    pub layers: Vec<(usize, LayerId)>,
    /// The layer of the first filled slot with no filled ancestor. Any other
    /// part with no filled ancestor follows it, so the character moves as one.
    pub root: LayerId,
}

/// One part of a layer rig before it is placed.
struct LayerPart {
    slot: usize,
    name: String,
    /// The artwork as it should stand, in the layer's coordinates.
    artwork: Object,
    /// The joint it turns about, in the layer's coordinates.
    joint: Point,
    /// The layer this part is split off (AI Rig as Layers), already cut down
    /// to the part's own object on every keyframe. When set, the part's layer
    /// is a copy of it — its settings, its folder and every keyframe — placed
    /// at its row, and `artwork` (the object as it stands at the rig's frame)
    /// only measures the pivot. `None` is a fresh layer holding `artwork` alone.
    from: Option<buzz_scene::Layer>,
    /// This part leads its slot (AI Rig as Layers picks the largest piece).
    /// With no part of a slot marked, the slot's front-most part leads.
    lead: bool,
}

/// Which layer stack a layer rig is built in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigHome {
    /// The timeline being edited: the stage, or the symbol open for editing.
    Active,
    /// A library symbol's own layers, whatever is open.
    Symbol(buzz_scene::SymbolId),
}

/// [`Scene::set_follows`] on a stack that need not be the active one: refuse
/// a cycle, record the parent's rest pose at `frame` if it has none, link.
fn set_follows_in(
    stack: &mut buzz_scene::LayerStack,
    child: LayerId,
    parent: Option<LayerId>,
    frame: u32,
) -> bool {
    if let Some(parent) = parent {
        if !stack.can_follow(child, parent) {
            return false;
        }
        let rest = stack
            .get(parent)
            .and_then(|l| l.frames.resolved_at(frame).iter().next().map(|o| o.transform));
        stack.update(parent, |l| {
            if l.rest_pose.is_none() {
                l.rest_pose = rest;
            }
        });
    }
    stack.update(child, |l| l.follows = parent)
}

/// Put each part alone on a new layer named after its slot (in front of
/// `home`, back to front, in `home`'s folder), set its transformation point on
/// its joint, and make each layer follow the layer of its nearest filled
/// ancestor in `parents`. Built in `target`'s layer stack. A part split off
/// a layer ([`LayerPart::from`]) goes in front of that layer instead, in its
/// folder, the parts of one layer in front of each other in order. With
/// `join_roots`, a part with no filled ancestor follows the rig's root so the
/// character moves as one; without, it stands alone (a naming profile whose
/// thighs, hips and body are roots).
fn place_layer_rig(
    scene: &mut Scene,
    target: RigHome,
    frame: u32,
    home: LayerId,
    parts: Vec<LayerPart>,
    parents: &[Option<usize>],
    join_roots: bool,
) -> Option<LayerRig> {
    if parts.is_empty() {
        return None;
    }
    // Ids first, from the document's allocator, the way `Scene::add_layer`
    // takes them; the stack below is borrowed from the scene.
    let ids: Vec<LayerId> = parts.iter().map(|_| LayerId(scene.next_object_id().0)).collect();
    let stack = match target {
        RigHome::Active => scene.edit_layers(),
        RigHome::Symbol(id) => scene.library_mut().layers_mut(id)?,
    };
    let (home_parent, home_len) = stack
        .get(home)
        .map_or((None, 1), |l| (l.parent, l.length()));

    let mut layers: Vec<(usize, LayerId)> = Vec::new();
    let mut anchor = home;
    let mut last_from: Option<LayerId> = None;
    let mut highest_object = 0;
    let mut leads: Vec<(usize, LayerId)> = Vec::new();
    for (part, layer) in parts.into_iter().zip(ids) {
        let mut artwork = part.artwork;
        // In the object's own coordinates, as `Object::pivot` is stored.
        let pivot = invert(artwork.transform).map(|inverse| inverse * part.joint);
        match part.from {
            None => {
                // As `Scene::add_layer`: on top, coloured by position.
                let mut fresh = buzz_scene::Layer::new(layer, part.name.clone(), LayerKind::Normal);
                fresh.color = buzz_scene::layer::default_color(stack.len());
                stack.push_front(fresh);
                // Move it to just in front of the last one placed, so the
                // parts keep their paint order where the character was.
                if let Some(index) = stack.index_of(anchor) {
                    stack.reorder(layer, index.saturating_sub(1));
                }
                stack.update(layer, |l| {
                    l.parent = home_parent;
                    if l.length() < home_len {
                        l.frames.insert_frame(home_len - 1);
                    }
                });
                artwork.pivot = pivot;
                highest_object = highest_object.max(artwork.id.0);
                let mut placed = false;
                stack.update(layer, |l| placed = l.push_object_at(0, Arc::new(artwork)));
                if !placed {
                    return None;
                }
            }
            Some(mut copy) => {
                // The first part off a layer stands in front of that layer;
                // the next part off it, in front of the one before.
                if last_from != Some(copy.id) {
                    anchor = copy.id;
                    last_from = Some(copy.id);
                }
                let object = artwork.id;
                copy.id = layer;
                copy.name = part.name.clone();
                for key in copy.frames.keyframes_mut() {
                    for each in Arc::make_mut(&mut key.objects).iter_mut() {
                        if each.id == object {
                            Arc::make_mut(each).pivot = pivot;
                        }
                    }
                }
                highest_object = highest_object.max(object.0);
                let index = stack.index_of(anchor).unwrap_or(0);
                stack.insert(index, copy);
            }
        }
        if part.lead {
            leads.push((part.slot, layer));
        }
        layers.push((part.slot, layer));
        anchor = layer;
    }
    scene.reserve_ids_above(highest_object);
    let stack = match target {
        RigHome::Active => scene.edit_layers(),
        RigHome::Symbol(id) => scene.library_mut().layers_mut(id)?,
    };

    // A bone can carry several parts (an arm and its sleeve), each on its own
    // layer. The slot's *front-most* layer leads: it is the one a click on
    // the limb lands on, the other pieces of the limb follow it, and so does
    // every child — so moving the arm carries its sleeve and its elbow.
    // A part marked as its slot's lead (AI Rig as Layers) leads instead.
    let layer_of = |slot: usize| {
        leads
            .iter()
            .find(|(s, _)| *s == slot)
            .or_else(|| layers.iter().rev().find(|(s, _)| *s == slot))
            .map(|(_, l)| *l)
    };
    let ancestor_layer = |slot: usize| {
        let mut current = parents.get(slot).copied().flatten();
        // Bounded by the slot count, against a parent cycle.
        for _ in 0..parents.len() {
            let parent = current?;
            if let Some(layer) = layer_of(parent) {
                return Some(layer);
            }
            current = parents.get(parent).copied().flatten();
        }
        None
    };
    let root = layers
        .iter()
        .map(|(slot, _)| *slot)
        .filter(|slot| ancestor_layer(*slot).is_none())
        .min()
        .and_then(layer_of)?;

    for &(slot, layer) in &layers {
        let lead = layer_of(slot).filter(|lead| *lead != layer);
        let parent = lead
            .or_else(|| ancestor_layer(slot))
            .or((join_roots && layer != root).then_some(root));
        if parent.is_some() {
            set_follows_in(stack, layer, parent, frame);
        }
    }
    Some(LayerRig { layers, root })
}

/// [`rig_character`], but rigged by **layer parenting** instead of bones.
///
/// One layer per filled slot, named after the slot, with the part alone on
/// it; the part's transformation point on its joint — the head of the bone
/// [`buzz_act::autorig::fit_slots`] fits to it, the same fitting
/// [`rig_character`] uses — and its layer following the layer of the nearest
/// filled ancestor slot. Posing is Free Transform rotation about the pivot.
/// The parts leave their own layers; nothing moves. The caller wraps this in
/// one `Document::edit`.
pub fn rig_character_layers(
    scene: &mut Scene,
    frame: u32,
    pattern: &RigPattern,
    slots: &[Option<ObjectId>],
) -> Option<LayerRig> {
    // Paint order, as in `rig_character`.
    let mut ordered: Vec<(usize, ObjectId)> = Vec::new();
    let mut home = None;
    for layer in scene.layers().selectable() {
        for object in layer.objects_at(frame) {
            if let Some(slot) = slots.iter().position(|s| *s == Some(object.id))
                && slot < pattern.slots.len()
            {
                ordered.push((slot, object.id));
                home = Some(layer.id);
            }
        }
    }
    let home = home?;

    let taken: Vec<(usize, Arc<Object>)> = ordered
        .iter()
        .filter_map(|(slot, id)| scene.find_object(*id).map(|(_, art)| (*slot, art.clone())))
        .collect();
    let fitted = buzz_act::autorig::fit_slots(pattern, &taken);
    let parts: Vec<LayerPart> = taken
        .iter()
        .map(|(slot, art)| LayerPart {
            slot: *slot,
            name: pattern.slots[*slot].name.clone(),
            artwork: (**art).clone(),
            // A drawing too small to measure turns about its own middle.
            joint: fitted[*slot].map_or_else(|| art.bounds().center(), |(head, _)| head),
            from: None,
            lead: false,
        })
        .collect();
    if parts.is_empty() {
        return None;
    }

    for (_, id) in &ordered {
        scene.remove_object(*id);
    }
    let parents: Vec<Option<usize>> = pattern.slots.iter().map(|s| s.parent).collect();
    place_layer_rig(scene, RigHome::Active, frame, home, parts, &parents, true)
}

/// **Bones → Layers.** Take a bone rig apart into a layer rig: each rigidly
/// bound part on its own layer named after its bone, standing where the pose
/// shows it (the pose becomes the layers' rest), its pivot on the bone's head,
/// following the layer of its nearest ancestor bone that has a part.
///
/// Refuses (`None`, nothing changed) a rig with skinned artwork, which bends
/// at the joints and cannot be one layer per bone.
pub fn bones_to_layers(scene: &mut Scene, frame: u32, rig: ObjectId) -> Option<LayerRig> {
    let (home, holder) = scene.find_object(rig)?;
    let ObjectKind::Armature(data) = &holder.kind else {
        return None;
    };
    let armature = &data.armature;
    let mut parts = Vec::new();
    for part in &data.parts {
        let RigBinding::Rigid(bone) = part.binding else {
            return None;
        };
        if bone >= armature.len() {
            return None;
        }
        let mut artwork = (*part.artwork).clone();
        artwork.transform = holder.transform * armature.pose_transform(bone) * artwork.transform;
        parts.push(LayerPart {
            slot: bone,
            name: armature.bones[bone].name.clone(),
            artwork,
            joint: holder.transform * armature.head(bone),
            from: None,
            lead: false,
        });
    }
    if parts.is_empty() {
        return None;
    }
    let parents: Vec<Option<usize>> = armature.bones.iter().map(|b| b.parent).collect();

    scene.remove_object(rig);
    place_layer_rig(scene, RigHome::Active, frame, home, parts, &parents, true)
}

/// Every armature on the stage at `frame` taken apart by [`bones_to_layers`]:
/// `(converted, refused)`. A refused rig (skinned artwork) is left as it was.
pub fn convert_all_to_layers(scene: &mut Scene, frame: u32) -> (usize, usize) {
    let rigs: Vec<ObjectId> = scene
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(frame).iter())
        .filter(|o| matches!(o.kind, ObjectKind::Armature(_)))
        .map(|o| o.id)
        .collect();
    let mut converted = 0;
    for &rig in &rigs {
        if bones_to_layers(scene, frame, rig).is_some() {
            converted += 1;
        }
    }
    (converted, rigs.len() - converted)
}

/// The layer rig `layer` belongs to: every layer in its follow tree, from the
/// top of the chain down, root first. `None` when the layer neither follows
/// nor is followed — a lone layer is not a rig.
pub fn layer_rig_of(scene: &Scene, layer: LayerId) -> Option<Vec<LayerId>> {
    let layers = scene.layers();
    layers.get(layer)?;
    let mut root = layer;
    // Bounded by the layer count, against a follow cycle in a corrupt file.
    for _ in 0..layers.len() {
        match layers.get(root).and_then(|l| l.follows) {
            Some(up) if up != layer => root = up,
            _ => break,
        }
    }
    let mut tree = vec![root];
    let mut index = 0;
    while index < tree.len() {
        for child in layers.followers_of(tree[index]) {
            if !tree.contains(&child) {
                tree.push(child);
            }
        }
        index += 1;
    }
    (tree.len() > 1).then_some(tree)
}

/// What [`repair_layer_rigs`] found and did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Layer rigs (follow trees of two or more layers) looked at.
    pub rigs: usize,
    /// Layers whose `follows` link changed.
    pub links_changed: usize,
    /// Keyframes whose objects were rebaked so nothing moved.
    pub keys_rebaked: usize,
}

/// One rig's rewiring: the new link of every layer in it, and the order to
/// rebake them in (every layer after the layer it follows).
struct RigRepair {
    follows: Vec<(LayerId, Option<LayerId>)>,
    order: Vec<LayerId>,
}

/// **Repair Layer Rigs.** Rewire layer rigs built with the old
/// [`place_layer_rig`] wiring — a slot's *back-most* layer led, its other
/// pieces followed the parent slot, its children followed the back-most piece
/// — to the current one: the slot's front-most layer leads, the slot's other
/// pieces follow it, and so does every child slot's lead.
///
/// A slot is recovered from the layer names (every piece of one part is named
/// after its slot), and a slot's parent from the layer its pieces follow
/// outside the slot. Every keyframe of every layer in a rewired rig is rebaked
/// so its world transform is what it was, parents before children, so no
/// keyframe looks different. A rig already wired this way is left alone.
pub fn repair_layer_rigs(scene: &mut Scene, scope: buzz_scene::CleanupScope) -> RepairReport {
    let mut report = RepairReport::default();
    let targets: Vec<Option<buzz_scene::SymbolId>> = match scope {
        buzz_scene::CleanupScope::Timeline => {
            vec![scene.editing_symbol().filter(|id| scene.library().get(*id).is_some())]
        }
        buzz_scene::CleanupScope::WholeScene => std::iter::once(None)
            .chain(scene.library().iter().map(|s| Some(s.id)))
            .collect(),
    };
    for target in targets {
        let plans = {
            let stack = match target {
                None => scene.stage_layers(),
                Some(id) => match scene.library().get(id) {
                    Some(symbol) => &symbol.layers,
                    None => continue,
                },
            };
            let (rigs, plans) = rig_repair_plans(stack);
            report.rigs += rigs;
            plans
        };
        if plans.is_empty() {
            continue;
        }
        let stack = match target {
            None => scene.edit_stage_layers(),
            Some(id) => match scene.library_mut().layers_mut(id) {
                Some(stack) => stack,
                None => continue,
            },
        };
        for plan in &plans {
            let (links, keys) = apply_rig_repair(stack, plan);
            report.links_changed += links;
            report.keys_rebaked += keys;
        }
    }
    report
}

/// How many follow trees `stack` holds, and the rewiring of those that need it.
fn rig_repair_plans(stack: &buzz_scene::LayerStack) -> (usize, Vec<RigRepair>) {
    use std::collections::HashMap;
    let roots = stack.follow_roots();
    // Members of each tree, front to back.
    let mut trees: Vec<(LayerId, Vec<LayerId>)> = Vec::new();
    for layer in stack.iter() {
        let Some(&root) = roots.get(&layer.id) else {
            continue;
        };
        match trees.iter_mut().find(|(r, _)| *r == root) {
            Some((_, members)) => members.push(layer.id),
            None => trees.push((root, vec![layer.id])),
        }
    }
    let follows_of = |id: LayerId| stack.get(id).and_then(|l| l.follows);

    let mut plans = Vec::new();
    'tree: for (root, members) in &trees {
        if members
            .iter()
            .any(|id| stack.get(*id).is_some_and(|l| l.follows_bone.is_some()))
        {
            continue;
        }
        // Slots by layer name, each front to back; a name whose pieces hang
        // off different slots is not one part, so its pieces stand alone.
        let mut slots: Vec<Vec<LayerId>> = Vec::new();
        // A slot's own name and its display name ("Shoulder R", "Upper Arm R")
        // are one part; names that only read as one part in the user's
        // convention (`canonical_slot`) are not merged.
        let key = |name: &str| buzz_rig::pattern::exact_slot(name).map_or_else(|| name.to_string(), str::to_string);
        for id in members {
            let name = key(stack.get(*id).map_or("", |l| l.name.as_str()));
            match slots.iter_mut().find(|s| stack.get(s[0]).is_some_and(|l| key(&l.name) == name)) {
                Some(slot) => slot.push(*id),
                None => slots.push(vec![*id]),
            }
        }
        let parent_slot = loop {
            let slot_of: HashMap<LayerId, usize> = slots
                .iter()
                .enumerate()
                .flat_map(|(i, s)| s.iter().map(move |id| (*id, i)))
                .collect();
            let mut parents: Vec<Option<usize>> = vec![None; slots.len()];
            let mut split = None;
            for (i, pieces) in slots.iter().enumerate() {
                let outside: Vec<usize> = pieces
                    .iter()
                    .filter_map(|id| follows_of(*id))
                    .filter_map(|t| slot_of.get(&t).copied())
                    .filter(|s| *s != i)
                    .collect();
                if outside.iter().any(|s| *s != outside[0]) {
                    split = Some(i);
                    break;
                }
                parents[i] = outside.first().copied();
            }
            match split {
                Some(i) => {
                    let pieces = slots.remove(i);
                    slots.extend(pieces.into_iter().map(|id| vec![id]));
                }
                None => break parents,
            }
        };
        let Some(root_slot) = slots.iter().position(|s| s.contains(root)) else {
            continue;
        };
        // Every slot but the root's must hang off another, and reach the root.
        for (i, _) in slots.iter().enumerate() {
            let mut current = i;
            for _ in 0..=slots.len() {
                if current == root_slot {
                    break;
                }
                match parent_slot[current] {
                    Some(up) => current = up,
                    None => continue 'tree,
                }
            }
            if current != root_slot {
                continue 'tree;
            }
        }

        let lead = |slot: usize| slots[slot][0];
        let mut follows = Vec::new();
        let mut order = Vec::new();
        let mut queue = vec![root_slot];
        let mut index = 0;
        while index < queue.len() {
            let slot = queue[index];
            index += 1;
            for &piece in &slots[slot] {
                let target = if piece == lead(slot) {
                    (slot != root_slot).then(|| parent_slot[slot].map(lead)).flatten()
                } else {
                    Some(lead(slot))
                };
                follows.push((piece, target));
                order.push(piece);
            }
            queue.extend((0..slots.len()).filter(|s| *s != root_slot && parent_slot[*s] == Some(slot)));
        }
        if follows.iter().any(|(id, target)| follows_of(*id) != *target) {
            plans.push(RigRepair { follows, order });
        }
    }
    (trees.len(), plans)
}

/// Rewire one rig and rebake it so every keyframe keeps its world transform:
/// `(links changed, keyframes rebaked)`.
fn apply_rig_repair(stack: &mut buzz_scene::LayerStack, plan: &RigRepair) -> (usize, usize) {
    use std::collections::{HashMap, HashSet};
    let close = |a: Affine, b: Affine| {
        a.as_coeffs()
            .iter()
            .zip(b.as_coeffs().iter())
            .all(|(x, y)| (x - y).abs() < 1e-6)
    };

    // Where everything stands under the old links.
    let mut worlds: HashMap<LayerId, Vec<(u32, Vec<Affine>)>> = HashMap::new();
    for &id in &plan.order {
        let Some(layer) = stack.get(id) else { continue };
        let keys = layer
            .frames
            .keyframes()
            .iter()
            .map(|k| {
                let inherited = stack.inherited_transform(id, k.start);
                (k.start, k.objects.iter().map(|o| inherited * o.transform).collect())
            })
            .collect();
        worlds.insert(id, keys);
    }
    let was_followed: HashSet<LayerId> = plan
        .order
        .iter()
        .copied()
        .filter(|id| !stack.followers_of(*id).is_empty())
        .collect();
    let followed: HashSet<LayerId> = plan.follows.iter().filter_map(|(_, t)| *t).collect();

    let mut links = 0;
    for &(id, target) in &plan.follows {
        if stack.get(id).and_then(|l| l.follows) != target {
            stack.update(id, |l| {
                l.follows = target;
                l.follows_bone = None;
            });
            links += 1;
        }
    }

    // Parents first, so each layer's new inheritance is already final.
    let mut keys = 0;
    for &id in &plan.order {
        let mut rebaked = false;
        for (start, objects) in worlds.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(into_local) = invert(stack.inherited_transform(id, *start)) else {
                continue;
            };
            let local: Vec<Affine> = objects.iter().map(|w| into_local * *w).collect();
            let differs = stack
                .get(id)
                .and_then(|l| l.frames.keyframes().iter().find(|k| k.start == *start))
                .is_some_and(|k| k.objects.iter().zip(&local).any(|(o, n)| !close(o.transform, *n)));
            if differs {
                stack.update(id, |l| {
                    if let Some(k) = l.frames.keyframes_mut().iter_mut().find(|k| k.start == *start) {
                        let objects = Arc::make_mut(&mut k.objects);
                        for (object, transform) in objects.iter_mut().zip(&local) {
                            Arc::make_mut(object).transform = *transform;
                        }
                    }
                });
                keys += 1;
                rebaked = true;
            }
        }
        // The rest pose its followers measure motion from: where it stands at
        // frame 0, recorded as `Scene::set_follows` does, for a layer that has
        // just become a parent or whose keys moved; none for one nothing
        // follows any more.
        if followed.contains(&id) {
            if rebaked || !was_followed.contains(&id) {
                let rest = stack
                    .get(id)
                    .and_then(|l| l.frames.resolved_at(0u32).iter().next().map(|o| o.transform));
                stack.update(id, |l| l.rest_pose = rest);
            }
        } else if stack.get(id).is_some_and(|l| l.rest_pose.is_some()) {
            stack.update(id, |l| l.rest_pose = None);
        }
    }
    (links, keys)
}

/// The built-in pattern whose slot names match the most of these layers'
/// names (ties to the pattern with fewer slots), for **Convert to bone rig**.
/// `None` when no layer is named after any slot.
pub fn pattern_for_layers(scene: &Scene, layers: &[LayerId]) -> Option<RigPattern> {
    let names: Vec<&str> = layers
        .iter()
        .filter_map(|id| scene.layers().get(*id).map(|l| l.name.as_str()))
        .collect();
    RigPattern::builtin()
        .into_iter()
        .map(|p| (names.iter().filter(|n| p.slot_for(n).is_some()).count(), p))
        .filter(|(hits, _)| *hits > 0)
        .min_by(|(a, pa), (b, pb)| b.cmp(a).then(pa.slots.len().cmp(&pb.slots.len())))
        .map(|(_, p)| p)
}

/// **Layers → Bones.** Rebuild a bone rig from a layer rig: each layer's slot
/// is read back from its name in `pattern`, and every part it shows at
/// `frame` (tweened, the parenting applied) is bound to that slot's bone,
/// so a slot drawn on several layers keeps all of them. The links are cut
/// and the parts leave their layers on every keyframe; the armature goes on
/// the front-most part's layer, shown at `frame`. Layers left empty are
/// removed.
///
/// **The bones are the layer rig's own joints**, as
/// [`buzz_act::layer_rig::rest_armature`] reads them: each slot's bone
/// starts at its part's transformation point and ends at the next joint of
/// its chain, else where its drawing ends (an instance measured by its
/// resolved bounds). The auto-rigger's measure of each drawing alone
/// ([`buzz_act::assemble`]) cannot be used here: an instance part measures
/// as a 2×2 box without the library, which built ep1's symbol-part
/// characters a skeleton about 1 px long.
///
/// Nothing moves at `frame`: the pose drawn there becomes the rest pose.
/// The layer rig's animation on other frames is not carried over.
pub fn layers_to_bones(
    scene: &mut Scene,
    frame: u32,
    pattern: &RigPattern,
    layers: &[LayerId],
) -> Option<ObjectId> {
    use buzz_geom::Shape as _;

    // Every part, back to front (the order the armature paints them), where
    // it is drawn at `frame`.
    let mut taken: Vec<(usize, Arc<Object>)> = Vec::new();
    let mut lead: Vec<Option<(bool, Point)>> = vec![None; pattern.slots.len()];
    let mut stand_ins: Vec<(usize, Arc<Object>)> = Vec::new();
    let mut home = None;
    for layer in scene.layers().paint_order() {
        if !layers.contains(&layer.id) {
            continue;
        }
        let Some(slot) = pattern.slot_for(&layer.name).filter(|s| *s < pattern.slots.len()) else {
            continue;
        };
        let inherited = scene.layers().inherited_transform(layer.id, frame);
        // A layer following one of its own slot's layers is not the joint.
        let first_of_slot = layer
            .follows
            .and_then(|up| scene.layers().get(up))
            .is_none_or(|up| pattern.slot_for(&up.name) != Some(slot));
        for object in layer.frames.resolved_at(frame).iter() {
            let mut placed = object.clone();
            placed.transform = inherited * object.transform;
            let pivot = scene.pivot_of(&placed);
            match lead[slot] {
                Some((true, _)) => {}
                Some((false, _)) if !first_of_slot => {}
                _ => lead[slot] = Some((first_of_slot, pivot)),
            }
            let stand_in = match placed.kind {
                ObjectKind::Shape(_) | ObjectKind::Group(_) => Arc::new(placed.clone()),
                _ => Arc::new(Object::shape(
                    ObjectId(0),
                    buzz_scene::ShapeData::filled(scene.resolved_bounds(&placed).to_path(0.1), peniko::Color::BLACK),
                )),
            };
            stand_ins.push((slot, stand_in));
            taken.push((slot, Arc::new(placed)));
            home = Some(layer.id);
        }
    }
    let home = home?;

    // The joints, chained as `rest_armature` chains them.
    let count = pattern.slots.len();
    let pivots: Vec<Option<Point>> = lead.iter().map(|l| l.map(|(_, p)| p)).collect();
    let fitted = buzz_act::autorig::fit_slots(pattern, &stand_ins);
    let aim = pattern.world_angles();
    let same_side = |a: buzz_rig::Side, b: buzz_rig::Side| {
        matches!(
            (a, b),
            (buzz_rig::Side::Either, buzz_rig::Side::Either)
                | (buzz_rig::Side::Left, buzz_rig::Side::Left)
                | (buzz_rig::Side::Right, buzz_rig::Side::Right)
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
            .filter(|(c, s)| s.parent == Some(slot) && pivots[*c].is_some() && same_side(pattern.slots[slot].side, s.side))
            .map(|(c, _)| c)
            .min();
        let along = match (chain.and_then(|c| pivots[c]), fitted[slot]) {
            (Some(next), _) if (next - head).hypot() > 1e-6 => next - head,
            (_, Some((h, t))) if (t - h).hypot() > 1e-6 => t - h,
            _ => Vec2::new(aim[slot].cos(), aim[slot].sin()),
        };
        joints[slot] = Some((head, head + along));
    }
    let rig = buzz_act::assemble_from_joints(pattern, &joints, &taken)?;

    // The parts leave their layers on every keyframe, so nothing is drawn
    // twice, and the links are cut.
    for (_, part) in &taken {
        while scene.remove_object(part.id).is_some() {}
    }
    for &id in layers {
        scene.set_follows(id, None, frame);
        scene.update_layer(id, |l| l.rest_pose = None);
    }
    // A home layer the parts were all of becomes one keyframe, so the
    // armature shows on every frame rather than until its next empty key.
    let emptied = scene.layers().get(home).is_some_and(|l| l.all_objects().next().is_none());
    if emptied {
        scene.update_layer(home, |l| {
            let length = l.frames.length();
            l.frames = buzz_scene::LayerTimeline::from_parts(vec![buzz_scene::Keyframe::new(0)], length);
        });
    }
    let id = scene.next_object_id();
    scene.add_object_at(
        home,
        frame,
        Object {
            id,
            name: None,
            // Identity: the parts kept the transforms they are drawn with.
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
            drawing_object: false,
        },
    )?;
    for &id in layers {
        let empty = scene
            .layers()
            .get(id)
            .is_some_and(|l| l.all_objects().next().is_none());
        if empty && id != home {
            scene.remove_layer(id);
        }
    }
    Some(id)
}

// ---------------------------------------------------------------------------
// AI ▸ Rig Character as Layers (docs/AI_LAYER_RIG_AND_MOTION_COPY_PLAN.md
// R1 plan, R2 apply): every piece of a character symbol onto its own layer
// inside the symbol, named after its `biped_full` slot and rigged by layer
// parenting, with nothing moving on any keyframe.
// ---------------------------------------------------------------------------

/// Where a [`LayerRigPlan`]'s joints came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointSource {
    /// DWPose found the figure in a render of the symbol.
    DwPose,
    /// No detector: slots from the names and a standing figure guessed from
    /// the bounds, each joint fitted to its drawing by
    /// [`buzz_act::autorig::fit_slots`]. Lower confidence.
    FitSlots,
}

/// One piece of a character: a top-level object on one of its layers.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedPiece {
    /// The symbol layer it is on now.
    pub layer: LayerId,
    pub object: ObjectId,
    /// Its slot in `RigPattern::biped_full`.
    pub slot: usize,
    pub slot_name: String,
    /// The name of the layer it will get: `<Slot>` for the slot's lead,
    /// `<Slot> · <old name or role>` for its other pieces.
    pub new_layer_name: String,
    /// The slot's front-most piece, whose layer the slot's other pieces and
    /// its child slots follow.
    pub lead: bool,
    /// Capsule overlap plus name affinity (`buzz_ai::rerig::full_overlap_score`).
    pub score: f64,
}

/// A nested symbol with a generic name ("part 14", "Symbol 3", "… copy 2")
/// renamed after the character and the slot it rides on.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRename {
    pub symbol: buzz_scene::SymbolId,
    pub from: String,
    pub to: String,
}

/// What AI ▸ Rig Character as Layers will do to one character symbol (R1):
/// worked out, nothing changed. [`apply_layer_rig_plan`] carries it out.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRigPlan {
    pub symbol: buzz_scene::SymbolId,
    /// The symbol frame the pieces were collected at.
    pub frame: u32,
    /// Every piece, back to front (paint order).
    pub pieces: Vec<PlannedPiece>,
    pub symbol_renames: Vec<SymbolRename>,
    /// The joint each `biped_full` slot turns about, in the symbol's own
    /// coordinates; `None` for a slot no piece went to.
    pub joints: Vec<Option<Point>>,
    pub source: JointSource,
    /// 0–1: the share of pieces whose slot rests on evidence (a name, or
    /// half the piece inside the bone), scaled down when no detector ran.
    pub confidence: f64,
    pub low_confidence: bool,
    pub notes: Vec<String>,
    /// How each piece whose name says a side was placed, and why
    /// (`buzz_ai::rerig::SideCall`), for the review; with a screen-sides
    /// profile, also each limb whose side was called by the side-view rule
    /// ("..., put on arm right (front-most is right)").
    pub side_calls: Vec<String>,
    /// The naming profile the names, parents and folders come from (N1).
    pub profile: buzz_rig::pattern::NamingProfile,
    /// The head symbol the head piece is an instance of, and its face layers
    /// renamed by the profile's Shift+R rules (`face_names`); no parenting
    /// inside it.
    pub head_symbol: Option<buzz_scene::SymbolId>,
    pub head_renames: Vec<LayerRename>,
    /// R4: vector pieces found merged — one drawing covering more than one
    /// body part — whether or not they were split.
    pub merged_found: usize,
    /// R4: the merged drawings cut along the skeleton (the pieces above are
    /// their parts); `None` when nothing was split.
    pub split: Option<MergedSplit>,
    /// The detector's `(head, tip)` per slot the plan was made from, so the
    /// review can plan again (split on or off) without running it again.
    pub detected: Option<Vec<Option<(Point, Point)>>>,
    /// S1: the cut style the merged drawings were split by (`None`: R4's
    /// generic split); kept so the review can plan again the same way.
    pub cut_style: Option<buzz_rig::cut_style::CutStyle>,
    /// H1: also Rig Head on `head_symbol` (the review's "Rig head like my
    /// characters"), in the same undo step.
    pub rig_head: bool,
}

/// **R4.** Merged drawings cut along the skeleton, worked out on a copy: the
/// symbol layers that held them, with each replaced by its parts on every
/// keyframe that shows the same drawing. Applied by
/// [`apply_layer_rig_plan`], which gives the parts fresh ids.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedSplit {
    /// The layers as they were when planned.
    pub originals: Vec<Arc<buzz_scene::Layer>>,
    /// The same layers with the merged drawings cut.
    pub layers: Vec<Arc<buzz_scene::Layer>>,
    /// Every id the parts (and their children) were given in the plan.
    pub new_ids: Vec<ObjectId>,
    /// Merged drawings cut.
    pub merged: usize,
    /// Each part (a piece of the plan) and the slot it was cut for.
    pub parts: Vec<(ObjectId, usize)>,
    /// Each joint overlap: the child slot, the joint, the disc's radius.
    pub overlaps: Vec<(usize, Point, f64)>,
    /// Keyframes of a merged drawing showing a different drawing than at the
    /// plan's frame, left as they are.
    pub unsplit_keys: usize,
    /// R4d: underlay parts (hidden-at-rest art of a joint parent under a
    /// limb stacked behind it), among `parts`; each is an extra of its slot.
    pub underlays: Vec<ObjectId>,
    /// S1: the pivot each cut slot turns about, placed by the cut style
    /// (empty for R4's generic split, whose slots turn about the detected
    /// joints).
    pub pivots: Vec<(usize, Point)>,
    /// S1: extra parts the cut style made (a scarf over the body), among
    /// `parts`, each with its layer name; never a slot's lead.
    pub extras: Vec<(ObjectId, String)>,
}

/// One layer of a symbol renamed.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRename {
    pub layer: LayerId,
    pub from: String,
    pub to: String,
}

/// A name that says nothing — "Layer_2", "Layer 2", "part 14", "Symbol 3",
/// "piece", "Shape 1", or nothing — ignoring any " copy N" suffix. Only such
/// names are replaced; "Towel copy 3" and "Artwork" are kept.
fn is_generic_name(name: &str) -> bool {
    const GENERIC: &[&str] = &["layer", "part", "symbol", "piece", "shape"];
    let mut words = buzz_rig::pattern::tokens(name);
    // " copy", " copy 2", " copy 2 copy": Animate's duplicate suffixes.
    loop {
        match words.as_slice() {
            [.., c] if c == "copy" => {
                words.pop();
            }
            [.., c, n] if c == "copy" && n.chars().all(|ch| ch.is_ascii_digit()) => {
                words.truncate(words.len() - 2);
            }
            _ => break,
        }
    }
    match words.split_first() {
        None => true,
        Some((first, rest)) => {
            GENERIC.contains(&first.as_str()) && rest.iter().all(|w| w.chars().all(|c| c.is_ascii_digit()))
        }
    }
}

/// One piece of a character as AI Rig as Layers sees it: where it is drawn at
/// the plan's frame (any layer parenting it already has, applied).
struct FoundPiece {
    layer: LayerId,
    layer_name: String,
    world: Arc<Object>,
    bounds: buzz_geom::Rect,
    name: String,
}

fn found_piece(
    scene: &Scene,
    stack: &buzz_scene::LayerStack,
    layer: &buzz_scene::Layer,
    object: &Object,
    frame: u32,
) -> FoundPiece {
    let inherited = stack.inherited_transform(layer.id, frame);
    let mut world = object.clone();
    world.transform = inherited * world.transform;
    let bounds = scene.resolved_bounds(&world);
    let nested = match &object.kind {
        ObjectKind::Instance(instance) => scene.library().get(instance.symbol).map(|s| s.name.clone()),
        _ => None,
    };
    // The layer's name says what the piece is, unless it says nothing.
    let name = match nested {
        Some(nested) if is_generic_name(&layer.name) => nested,
        _ => layer.name.clone(),
    };
    FoundPiece {
        layer: layer.id,
        layer_name: layer.name.clone(),
        world: Arc::new(world),
        bounds,
        name,
    }
}

/// What the naming rules of [`plan_layer_rig`] decided for a set of pieces
/// on their slots.
struct PieceNaming {
    leads: Vec<bool>,
    names: Vec<String>,
    symbol_renames: Vec<SymbolRename>,
    /// Limbs whose side the side-view rule called, for the review.
    side_calls: Vec<String>,
}

/// The `biped_full` slots of each limb pair: left slots, right slots, what.
const LIMB_CHAINS: [(&[usize], &[usize], &str); 3] =
    [(&[3, 4, 11], &[5, 6, 12], "arm"), (&[7, 8, 13], &[9, 10, 14], "leg"), (&[15], &[16], "brow")];

/// **The screen-side rule** (N1) for a screen-sides profile: the side each
/// `biped_full` slot's limb is named for. Per limb pair (arms, legs, brows):
/// 1. when the geometry is clear — the roots more than 10 % of the body's
///    width apart, or a lone limb's root that far off the centre line — the
///    screen side of the limb's root joint (x of shoulder or hip; where the
///    root pieces are drawn when the joint lies outside them) decides
///    (N3), and a name saying the other side gets a call for the review.
///    DWPose's anatomical L/R lands on the opposite screen side of a front
///    view, which is why the slot's own letter is not used;
/// 2. otherwise (a side view, overlapping limbs) a side word in the pieces'
///    names wins (the other limb takes the other side);
/// 3. with no side word either, the front-most limb (paint order) is the
///    right one, with a call for the review.
fn screen_sides(
    found: &[FoundPiece],
    slots: &[usize],
    joints: &[Option<Point>],
    calls: &mut Vec<String>,
) -> Vec<Option<buzz_rig::Side>> {
    use buzz_rig::Side;
    let mut out: Vec<Option<Side>> = vec![None; RigPattern::biped_full().slots.len()];
    let Some(extent) = found.iter().map(|f| f.bounds).reduce(|a, b| a.union(b)) else {
        return out;
    };
    let width = extent.width().max(1.0);
    let opposite = |s: Side| if s == Side::Left { Side::Right } else { Side::Left };
    for (left_chain, right_chain, what) in LIMB_CHAINS {
        let members = |chain: &[usize]| -> Vec<usize> { (0..found.len()).filter(|i| chain.contains(&slots[*i])).collect() };
        let (a, b) = (members(left_chain), members(right_chain));
        if a.is_empty() && b.is_empty() {
            continue;
        }
        let named = |m: &[usize]| -> Option<Side> {
            let (mut l, mut r) = (0, 0);
            for &i in m {
                match buzz_rig::pattern::name_side(&found[i].name) {
                    Some(Side::Left) => l += 1,
                    Some(Side::Right) => r += 1,
                    _ => {}
                }
            }
            match l.cmp(&r) {
                std::cmp::Ordering::Greater => Some(Side::Left),
                std::cmp::Ordering::Less => Some(Side::Right),
                std::cmp::Ordering::Equal => None,
            }
        };
        // The x of the limb's root joint (shoulder or hip) — unless it lies
        // outside the root pieces' own span, as a detector joint does for a
        // piece whose kept side word sent it to the other side's slot; then
        // where those pieces are drawn.
        let root_x = |chain: &[usize], m: &[usize]| -> f64 {
            let root = chain.iter().copied().find(|s| m.iter().any(|&i| slots[i] == *s));
            let span = root.and_then(|s| {
                m.iter().filter(|&&i| slots[i] == s).map(|&i| found[i].bounds).reduce(|a, b| a.union(b))
            });
            let joint = root.and_then(|s| joints.get(s).copied().flatten()).map(|p| p.x);
            match (joint, span) {
                (Some(x), Some(r)) if x >= r.x0 && x <= r.x1 => x,
                (_, Some(r)) => r.center().x,
                (Some(x), None) => x,
                (None, None) => m.first().map_or(extent.center().x, |i| found[*i].bounds.center().x),
            }
        };
        // The centre line: the chest's joint, else the hips', else the
        // middle; for the brows, the head's.
        let around: &[usize] = if what == "brow" { &[2] } else { &[1, 0] };
        let centre = around
            .iter()
            .find_map(|s| joints.get(*s).copied().flatten())
            .map_or(extent.center().x, |p| p.x);
        let mut side_view = |m: &[usize], put: &str| {
            for &i in m {
                calls.push(format!(
                    "{}: side view ({what}s within 10% of the body width), put on {put} (front-most is right)",
                    found[i].layer_name
                ));
            }
        };
        // N3: where the root joints stand decides when it is clear — the two
        // roots more than 10 % of the body's width apart, or a lone limb's
        // root that far off the centre line. Otherwise (a side view,
        // overlapping limbs) the geometry can't tell.
        let geometry: Option<(Side, Side)> = if !a.is_empty() && !b.is_empty() {
            let (xa, xb) = (root_x(left_chain, &a), root_x(right_chain, &b));
            ((xa - xb).abs() >= 0.1 * width)
                .then(|| if xa < xb { (Side::Left, Side::Right) } else { (Side::Right, Side::Left) })
        } else {
            let (chain, m) = if a.is_empty() { (right_chain, &b) } else { (left_chain, &a) };
            let dx = root_x(chain, m) - centre;
            (dx.abs() >= 0.1 * width).then(|| {
                let side = if dx < 0.0 { Side::Left } else { Side::Right };
                (side, side)
            })
        };
        let side_word = |s: Side| if s == Side::Left { "left" } else { "right" };
        let (sa, sb) = match (geometry, named(&a), named(&b)) {
            (Some((sa, sb)), _, _) => {
                // A name saying the other side (our own old rigs' anatomical
                // "Shoulder R") loses to the screen; the review flags it.
                for (m, side) in [(&a, sa), (&b, sb)] {
                    for &i in m.iter() {
                        if let Some(said) = buzz_rig::pattern::name_side(&found[i].name)
                            && said != Side::Either
                            && said != side
                        {
                            calls.push(format!(
                                "{}: named {}, put on {what} {} (screen side: the {what} joints are clearly apart, more than 10% of the body width)",
                                found[i].layer_name,
                                side_word(said),
                                side_word(side)
                            ));
                        }
                    }
                }
                (sa, sb)
            }
            (None, Some(x), Some(y)) => (x, y),
            (None, Some(x), None) => (x, opposite(x)),
            (None, None, Some(y)) => (opposite(y), y),
            (None, None, None) if !a.is_empty() && !b.is_empty() => {
                let a_front = a.iter().max() > b.iter().max();
                let (sa, sb) = if a_front { (Side::Right, Side::Left) } else { (Side::Left, Side::Right) };
                side_view(&a, &format!("{what} {}", if a_front { "right" } else { "left" }));
                side_view(&b, &format!("{what} {}", if a_front { "left" } else { "right" }));
                (sa, sb)
            }
            (None, None, None) => {
                let m = if a.is_empty() { &b } else { &a };
                side_view(m, &format!("{what} right"));
                (Side::Right, Side::Right)
            }
        };
        for &s in left_chain {
            out[s] = Some(sa);
        }
        for &s in right_chain {
            out[s] = Some(sb);
        }
    }
    out
}

/// R1's lead and naming rules, shared by [`plan_layer_rig`] and
/// [`relabel_layer_rig_plan`], in `profile`'s names. Each slot's lead is its
/// largest piece that matched something (`matched[i]`), ties to the
/// front-most, among the pieces not marked `extra` (all of them, when every
/// piece of a slot is). The lead takes the profile's name for its slot (on
/// the screen side, [`screen_sides`], for a screen-sides profile); any other
/// piece keeps its own name — the Shift+R face name with `face_names`, else
/// its side written the rig's way — unless that name is generic (or would
/// read as a slot), when it is the profile's extra name. Nested symbols with
/// generic names that ride one slot only are renamed "<Character> <part>".
#[allow(clippy::too_many_arguments)]
fn name_layer_rig_pieces(
    scene: &Scene,
    character: &str,
    found: &[FoundPiece],
    slots: &[usize],
    matched: &[bool],
    extra: &[bool],
    joints: &[Option<Point>],
    profile: &buzz_rig::pattern::NamingProfile,
    notes: &mut Vec<String>,
) -> PieceNaming {
    let pattern = RigPattern::biped_full();
    let area = |i: usize| found[i].bounds.area();
    // A piece whose name says its slot's part ("midCloth1" on the hips) leads
    // before a bigger one that only lies there (a gamcha).
    let named = |i: usize| {
        let stem = pattern.slots[slots[i]].name.trim_end_matches(" L").trim_end_matches(" R");
        buzz_ai::rerig::body_word(&found[i].name) == Some(stem)
    };
    let mut leads: Vec<Option<usize>> = vec![None; pattern.slots.len()];
    for all in [false, true] {
        for (index, &slot) in slots.iter().enumerate() {
            if !all && extra.get(index).copied().unwrap_or(false) {
                continue;
            }
            if all && leads[slot].is_some_and(|l| !extra.get(l).copied().unwrap_or(false)) {
                continue;
            }
            let lead = &mut leads[slot];
            let better = match *lead {
                None => true,
                Some(best) => {
                    (matched[index], named(index), area(index)) >= (matched[best], named(best), area(best))
                }
            };
            if better {
                *lead = Some(index);
            }
        }
    }

    let mut side_calls = Vec::new();
    let sides = if profile.screen_sides {
        screen_sides(found, slots, joints, &mut side_calls)
    } else {
        vec![None; pattern.slots.len()]
    };
    let part_name = |slot: usize| profile.layer_name(&pattern.slots[slot].name, sides[slot]);

    let mut leads_out = Vec::with_capacity(found.len());
    let mut names: Vec<String> = Vec::with_capacity(found.len());
    let mut extras: Vec<usize> = vec![0; pattern.slots.len()];
    for (index, (f, &slot)) in found.iter().zip(slots).enumerate() {
        let display = part_name(slot);
        let lead = leads[slot] == Some(index);
        let own = [&f.layer_name, &f.name]
            .into_iter()
            .find(|n| !is_generic_name(n))
            .map(|n| {
                if profile.face_names {
                    buzz_rig::pattern::face_layer_name(n).unwrap_or_else(|| n.trim().to_string())
                } else {
                    buzz_ai::rerig::side_normalised_name(n)
                }
            })
            .filter(|n| buzz_rig::pattern::canonical_slot(n).is_none());
        let mut extra_name = || {
            extras[slot] += 1;
            profile.extra_name(&display, extras[slot])
        };
        let base = match (matched[index], lead, own) {
            (false, _, _) => {
                let name = extra_name();
                notes.push(format!(
                    "'{}' matched no body part: put on {display} as '{name}'",
                    f.layer_name
                ));
                name
            }
            (true, true, _) => display.clone(),
            (true, false, Some(own)) => own,
            (true, false, None) => extra_name(),
        };
        let mut name = base.clone();
        let mut n = 2;
        while names.contains(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        names.push(name);
        leads_out.push(lead);
    }

    // Nested symbols with generic names, after the character and their slot —
    // only a symbol every piece of which rides one slot.
    let slot_name = |i: usize| pattern.slots[slots[i]].name.as_str();
    let mut symbol_renames: Vec<SymbolRename> = Vec::new();
    let mut seen: Vec<buzz_scene::SymbolId> = Vec::new();
    for (index, f) in found.iter().enumerate() {
        let ObjectKind::Instance(instance) = &f.world.kind else {
            continue;
        };
        let id = instance.symbol;
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let Some(nested) = scene.library().get(id) else {
            continue;
        };
        if !is_generic_name(&nested.name) {
            continue;
        }
        let on: Vec<&str> = found
            .iter()
            .enumerate()
            .filter(|(_, g)| matches!(&g.world.kind, ObjectKind::Instance(i) if i.symbol == id))
            .map(|(j, _)| slot_name(j))
            .collect();
        if on.iter().any(|s| *s != slot_name(index)) {
            notes.push(format!("'{}' kept its name: it is on {}", nested.name, on.join(" and ")));
            continue;
        }
        let base = format!("{character} {}", part_name(slots[index]));
        let mut to = base.clone();
        let mut n = 2;
        while scene.library().find_by_name(&to).is_some() || symbol_renames.iter().any(|r| r.to == to) {
            to = format!("{base} {n}");
            n += 1;
        }
        symbol_renames.push(SymbolRename { symbol: id, from: nested.name.clone(), to });
    }
    PieceNaming { leads: leads_out, names, symbol_renames, side_calls }
}

/// **R4d.** An underlay part (`MergedSplit::underlays`) takes its slot lead's
/// name and "Under" (`bodyUnder`); it follows the lead as any extra does.
fn name_underlays(names: &mut [String], leads: &[bool], slots: &[usize], underlay: &[bool]) {
    for i in 0..names.len() {
        if !underlay.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some(lead) = (0..names.len()).find(|&j| leads.get(j) == Some(&true) && slots.get(j) == slots.get(i))
        else {
            continue;
        };
        let base = format!("{}Under", names[lead]);
        let mut name = base.clone();
        let mut n = 2;
        while names.iter().enumerate().any(|(j, x)| j != i && *x == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        names[i] = name;
    }
}

/// **S1.** A cut style's extra part (`MergedSplit::extras`, a scarf) takes the
/// name the split gave it, made unique.
fn name_style_extras(names: &mut [String], objects: &[ObjectId], split: Option<&MergedSplit>) {
    let Some(split) = split else { return };
    for i in 0..names.len() {
        let Some((_, base)) = split.extras.iter().find(|(id, _)| Some(id) == objects.get(i)) else { continue };
        let mut name = base.clone();
        let mut n = 2;
        while names.iter().enumerate().any(|(j, x)| j != i && *x == name) {
            name = format!("{base}{n}");
            n += 1;
        }
        names[i] = name;
    }
}

/// The head symbol (the instance leading the Head slot) and its face layers
/// renamed by the profile's Shift+R rules — `characterSetup.jsfl` step 4 —
/// when the profile has `face_names`.
fn head_face_renames(
    scene: &Scene,
    found: &[FoundPiece],
    slots: &[usize],
    leads: &[bool],
    profile: &buzz_rig::pattern::NamingProfile,
) -> (Option<buzz_scene::SymbolId>, Vec<LayerRename>) {
    if !profile.face_names {
        return (None, Vec::new());
    }
    let head = (0..found.len()).find(|&i| slots[i] == 2 && leads[i]);
    let Some(ObjectKind::Instance(instance)) = head.map(|i| &found[i].world.kind) else {
        return (None, Vec::new());
    };
    let Some(symbol) = scene.library().get(instance.symbol) else {
        return (None, Vec::new());
    };
    (Some(instance.symbol), face_layer_renames(&symbol.layers))
}

/// Every layer of `stack` the Shift+R rules rename (folders skipped).
fn face_layer_renames(stack: &buzz_scene::LayerStack) -> Vec<LayerRename> {
    stack
        .iter()
        .filter(|l| l.kind != LayerKind::Folder)
        .filter_map(|l| {
            let to = buzz_rig::pattern::face_layer_name(&l.name)?;
            (!to.eq_ignore_ascii_case(l.name.trim())).then(|| LayerRename { layer: l.id, from: l.name.clone(), to })
        })
        .collect()
}

/// **R3.** After the review moved pieces to other slots (or marked them
/// `extra`, never their slot's lead unless every piece of the slot is), work
/// the leads, layer names and symbol renames out again by R1's rules, as
/// [`plan_layer_rig`] would for these slots. `slot_name` follows `slot`;
/// a slot newly filled gets its joint from the piece's bounds when the plan
/// has none. Names the reviewer typed are the caller's to put back. `false`,
/// with the plan unchanged, when a piece is no longer in the symbol.
pub fn relabel_layer_rig_plan(scene: &Scene, plan: &mut LayerRigPlan, extra: &[bool]) -> bool {
    // R4: the parts of a split drawing are only in the plan's own view.
    let view = plan_scene(scene, plan);
    let scene: &Scene = &view;
    let cut: std::collections::HashSet<ObjectId> =
        plan.split.iter().flat_map(|s| s.parts.iter().map(|(id, _)| *id)).collect();
    let Some(character) = scene.library().get(plan.symbol) else {
        return false;
    };
    let pattern = RigPattern::biped_full();
    let stack = &character.layers;
    let mut found = Vec::with_capacity(plan.pieces.len());
    for piece in &plan.pieces {
        let Some(layer) = stack.get(piece.layer) else {
            return false;
        };
        let Some(object) = layer.objects_at(plan.frame).iter().find(|o| o.id == piece.object) else {
            return false;
        };
        if piece.slot >= pattern.slots.len() {
            return false;
        }
        let mut f = found_piece(scene, stack, layer, object, plan.frame);
        if cut.contains(&f.world.id) {
            f.layer_name.clear();
            f.name.clear();
        }
        found.push(f);
    }
    let slots: Vec<usize> = plan.pieces.iter().map(|p| p.slot).collect();
    let matched: Vec<bool> = plan.pieces.iter().map(|p| p.score > 0.0).collect();
    let mut scratch = Vec::new();
    plan.joints.resize(pattern.slots.len(), None);
    // R4d: an underlay part stays an extra, named after its slot's lead.
    let underlay: Vec<bool> = plan
        .pieces
        .iter()
        .map(|p| plan.split.as_ref().is_some_and(|s| s.underlays.contains(&p.object)))
        .collect();
    // S1: a cut style's extra (a scarf) never leads either.
    let style_extra: Vec<bool> = plan
        .pieces
        .iter()
        .map(|p| plan.split.as_ref().is_some_and(|s| s.extras.iter().any(|(id, _)| *id == p.object)))
        .collect();
    let extra: Vec<bool> = (0..plan.pieces.len())
        .map(|i| extra.get(i).copied().unwrap_or(false) || underlay[i] || style_extra[i])
        .collect();
    let mut naming = name_layer_rig_pieces(
        scene,
        &character.name,
        &found,
        &slots,
        &matched,
        &extra,
        &plan.joints,
        &plan.profile,
        &mut scratch,
    );
    name_underlays(&mut naming.names, &naming.leads, &slots, &underlay);
    let objects: Vec<ObjectId> = plan.pieces.iter().map(|p| p.object).collect();
    name_style_extras(&mut naming.names, &objects, plan.split.as_ref());
    for (index, piece) in plan.pieces.iter_mut().enumerate() {
        piece.slot_name = pattern.slots[piece.slot].name.clone();
        piece.lead = naming.leads[index];
        piece.new_layer_name = naming.names[index].clone();
        if plan.joints[piece.slot].is_none() && piece.lead {
            plan.joints[piece.slot] = Some(found[index].bounds.center());
        }
    }
    plan.symbol_renames = naming.symbol_renames;
    let (head_symbol, head_renames) = head_face_renames(scene, &found, &slots, &naming.leads, &plan.profile);
    plan.head_symbol = head_symbol;
    plan.head_renames = head_renames;
    true
}

/// **R1.** Plan AI Rig as Layers for `symbol` at its `frame`, changing
/// nothing.
///
/// Every top-level object on every normal layer is a piece. Each goes to its
/// best `RigPattern::biped_full` slot by `buzz_ai::rerig::map_pieces_to_full_slots`
/// — capsule overlap plus name affinity, head pieces always on Head — against
/// the `detected` joints (`(head, tip)` per slot, from
/// `crate::app::detect_symbol_joints`) or, with `None`, a standing figure
/// guessed from the pieces' bounds, which marks the plan lower confidence.
/// Each filled slot's joint is the detected head (not where a kept side word
/// sits outside the detector's bone for that side), else
/// [`buzz_act::autorig::fit_slots`] on the slot's largest piece, else the
/// guessed head. The slot's largest matched piece leads (ties to the
/// front-most) and takes `profile`'s name for the slot; the rest keep their
/// own names unless generic (`name_layer_rig_pieces`). With a profile that
/// names a shadow, shadow pieces are left out of the rig; with `face_names`,
/// the head symbol's face layers get the Shift+R names (`head_renames`).
/// `None` when the symbol has no pieces at `frame`.
pub fn plan_layer_rig(
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    detected: Option<&[Option<(Point, Point)>]>,
    profile: &buzz_rig::pattern::NamingProfile,
) -> Option<LayerRigPlan> {
    plan_layer_rig_with(scene, symbol, frame, detected, profile, Some(false))
}

/// The pieces of `stack` at `frame`, back to front, each standing where it is
/// drawn (any layer parenting it already has, applied); objects on mask,
/// guide and folder layers counted, shadows (with a profile that names one)
/// left out by layer name.
fn collect_layer_rig_pieces(
    scene: &Scene,
    stack: &buzz_scene::LayerStack,
    frame: u32,
    profile: &buzz_rig::pattern::NamingProfile,
) -> (Vec<FoundPiece>, usize, Vec<String>) {
    let mut found: Vec<FoundPiece> = Vec::new();
    let mut skipped = 0;
    let mut shadows: Vec<String> = Vec::new();
    for layer in stack.paint_order() {
        let objects = layer.objects_at(frame);
        if layer.kind != LayerKind::Normal {
            skipped += objects.len();
            continue;
        }
        for object in objects {
            let piece = found_piece(scene, stack, layer, object, frame);
            let is_shadow = [&piece.layer_name, &piece.name].iter().any(|n| n.to_lowercase().contains("shadow"));
            if profile.shadow.is_some() && is_shadow {
                shadows.push(piece.layer_name.clone());
                continue;
            }
            found.push(piece);
        }
    }
    (found, skipped, shadows)
}

/// [`plan_layer_rig`] with R4's choice: `split` `Some(true)` cuts every
/// merged drawing (a vector piece covering more than one body part) along
/// the skeleton, `Some(false)` never does, `None` does when one is found.
pub fn plan_layer_rig_with(
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    detected: Option<&[Option<(Point, Point)>]>,
    profile: &buzz_rig::pattern::NamingProfile,
    split: Option<bool>,
) -> Option<LayerRigPlan> {
    plan_layer_rig_styled(scene, symbol, frame, detected, profile, split, None)
}

/// [`plan_layer_rig_with`] with a cut style (S1/S2): a merged drawing that is
/// split is cut the way `style` says — garments with the part the style puts
/// them on, straight cuts square to the bone, each piece completed under the
/// pieces stacked over it, pivots where the style puts them. `None`: R4's
/// generic split.
pub fn plan_layer_rig_styled(
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    detected: Option<&[Option<(Point, Point)>]>,
    profile: &buzz_rig::pattern::NamingProfile,
    split: Option<bool>,
    style: Option<&buzz_rig::cut_style::CutStyle>,
) -> Option<LayerRigPlan> {
    use kurbo::Shape as _;
    let original = scene;
    let pattern = RigPattern::biped_full();
    let mut notes = Vec::new();

    // The pieces, back to front, each standing where it is drawn at `frame`
    // (any layer parenting it already has, applied).
    let (mut found, skipped, mut shadows) =
        collect_layer_rig_pieces(scene, &scene.library().get(symbol)?.layers, frame, profile);
    if found.is_empty() {
        return None;
    }

    // R4: merged drawings, cut along the skeleton when asked (or, with no
    // answer, when there are any). The capsules come from the drawing as it
    // stands; the parts fill the same extent.
    let extent = found
        .iter()
        .map(|f| f.bounds)
        .reduce(|a, b| a.union(b))
        .unwrap_or_default();
    let guessed = buzz_ai::rerig::guessed_biped_full_joints(extent);
    let capsules =
        buzz_ai::rerig::biped_full_capsules(detected.unwrap_or(&[]), &guessed, extent.height());
    let partition = BonePartition::new(&capsules, extent);
    let merged: Vec<(LayerId, ObjectId, PieceLabels)> = found
        .iter()
        .filter_map(|f| {
            let labels = piece_labels(scene, &f.world, &partition)?;
            labels.merged.then_some((f.layer, f.world.id, labels))
        })
        .collect();
    let merged_found = merged.len();
    let mut split_done: Option<(Scene, MergedSplit)> = None;
    if merged_found > 0 && split.unwrap_or(true) {
        split_done = match style {
            Some(style) => split_merged_styled(scene, symbol, frame, merged, &partition, &capsules, style),
            None => split_merged_drawings(scene, symbol, frame, merged, &partition, &capsules),
        };
        if split_done.is_none() {
            notes.push("the merged drawing could not be split; its pieces are as drawn".into());
        }
    } else if merged_found > 0 {
        notes.push(format!(
            "{merged_found} merged drawing{} found (one drawing over several body parts), not split",
            if merged_found == 1 { "" } else { "s" }
        ));
    }
    let split_slot: std::collections::HashMap<ObjectId, usize> = split_done
        .as_ref()
        .map(|(_, s)| s.parts.iter().copied().collect())
        .unwrap_or_default();
    let scene: &Scene = match &split_done {
        Some((split_scene, s)) => {
            let (again, _, again_shadows) =
                collect_layer_rig_pieces(split_scene, &split_scene.library().get(symbol)?.layers, frame, profile);
            found = again;
            shadows = again_shadows;
            // A part's name is its source's, which says nothing about it: it
            // takes the profile's names for its slot.
            for f in found.iter_mut().filter(|f| split_slot.contains_key(&f.world.id)) {
                f.layer_name.clear();
                f.name.clear();
            }
            notes.push(format!(
                "split {} merged drawing{} along the skeleton into {} pieces, {} joint overlap{}{}",
                s.merged,
                if s.merged == 1 { "" } else { "s" },
                s.parts.len(),
                s.overlaps.len(),
                if s.overlaps.len() == 1 { "" } else { "s" },
                if s.unsplit_keys > 0 {
                    format!(" ({} keyframes showing another drawing left as drawn)", s.unsplit_keys)
                } else {
                    String::new()
                }
            ));
            split_scene
        }
        None => original,
    };
    let character = scene.library().get(symbol)?;
    if skipped > 0 {
        notes.push(format!("{skipped} objects on mask, guide or folder layers left as they are"));
    }
    if !shadows.is_empty() {
        shadows.dedup();
        notes.push(format!("shadow left out of the rig: {}", shadows.join(", ")));
    }
    if found.is_empty() {
        return None;
    }

    let candidates: Vec<buzz_ai::rerig::VectorPartCandidate> = found
        .iter()
        .map(|f| buzz_ai::rerig::VectorPartCandidate {
            name: f.name.clone(),
            bounds: f.bounds,
            mask: None,
            object: f.world.clone(),
        })
        .collect();
    let (mut mappings, mut calls): (Vec<_>, Vec<_>) =
        buzz_ai::rerig::map_pieces_to_full_slots(&candidates, &capsules, detected.is_some())
            .into_iter()
            .unzip();
    // A part cut from a merged drawing stays on the slot it was cut for.
    let cut_for: Vec<Option<usize>> = found.iter().map(|f| split_slot.get(&f.world.id).copied()).collect();
    for ((slot, m), call) in cut_for.iter().zip(mappings.iter_mut()).zip(calls.iter_mut()) {
        if let Some(slot) = *slot {
            m.slot_index = slot;
            m.slot_name = pattern.slots[slot].name.clone();
            m.overlap_score = 1.0;
            *call = None;
        }
    }
    let side_calls: Vec<String> = found
        .iter()
        .zip(&calls)
        .zip(&mappings)
        .filter_map(|((f, call), m)| {
            let call = (*call)?;
            let other = if call.named == 'L' { 'R' } else { 'L' };
            let threshold = buzz_ai::rerig::SIDE_CONTRADICTION;
            Some(if call.flipped {
                format!(
                    "{}: named {}, put on {} (score {:.2} on {} < {threshold}, {:.2} on {other})",
                    f.name, call.named, m.slot_name, call.named_score, call.named, call.other_score
                )
            } else {
                format!(
                    "{}: kept {} on {} (name wins: score {:.2} ≥ {threshold}; bone overlap {:.2} {}, {:.2} {other})",
                    f.name,
                    call.named,
                    m.slot_name,
                    call.named_score,
                    call.named_overlap,
                    call.named,
                    call.other_overlap
                )
            })
        })
        .collect();
    // Slots a kept side word sits poorly in (the detector's sides read the
    // other way, as on a side view): their joints come from the drawing.
    let mut contradicted = vec![false; pattern.slots.len()];
    for (call, m) in calls.iter().zip(&mappings) {
        if let Some(call) = call
            && !call.flipped
            && call.named_overlap < buzz_ai::rerig::SIDE_CONTRADICTION
            && call.other_overlap > call.named_overlap
        {
            contradicted[m.slot_index] = true;
        }
    }
    // An upper-body extra (gamcha, scarf): a piece whose name says no body
    // part, put on the torso or an upper arm, standing above the hips joint,
    // goes to the profile's `upper_extras` part (the body) and follows it.
    let mut mappings = mappings;
    if let Some(target) = profile.upper_extras.as_deref().and_then(|p| pattern.slot_named(p)) {
        let hips = detected
            .and_then(|d| d.first().copied().flatten())
            .or(guessed.first().copied().flatten())
            .map(|(head, _)| head.y);
        for ((f, m), cut) in found.iter().zip(mappings.iter_mut()).zip(&cut_for) {
            if cut.is_none()
                && matches!(m.slot_index, 0 | 1 | 3 | 5)
                && m.slot_index != target
                && buzz_ai::rerig::body_word(&f.name).is_none()
                && hips.is_some_and(|y| f.bounds.center().y < y)
            {
                notes.push(format!(
                    "'{}' names no body part and sits above the hips: an upper-body extra on {}, not {}",
                    f.layer_name, pattern.slots[target].name, m.slot_name
                ));
                m.slot_index = target;
                m.slot_name = pattern.slots[target].name.clone();
            }
        }
    }
    // N3: two pieces that both read as the body and no hips piece — the lower
    // one is the lower cloth (the profile's hips part, `midCloth`, a root of
    // its own), not an extra following the body.
    if !mappings.iter().any(|m| m.slot_index == 0) {
        let body: Vec<usize> = (0..found.len())
            .filter(|&i| mappings[i].slot_index == 1 && buzz_ai::rerig::body_word(&found[i].name) == Some("Chest"))
            .collect();
        if body.len() >= 2
            && let Some(lowest) = body
                .iter()
                .copied()
                .max_by(|&x, &y| found[x].bounds.center().y.total_cmp(&found[y].bounds.center().y))
        {
            notes.push(format!(
                "'{}' is the lower of {} body pieces and there is no hips piece: put on {}",
                found[lowest].layer_name,
                body.len(),
                pattern.slots[0].name
            ));
            mappings[lowest].slot_index = 0;
            mappings[lowest].slot_name = pattern.slots[0].name.clone();
        }
    }
    let source = if detected.is_some() { JointSource::DwPose } else { JointSource::FitSlots };

    // Joints: detected, else fitted to the slot's largest piece, else guessed.
    let mut largest: Vec<Option<usize>> = vec![None; pattern.slots.len()];
    for (index, mapping) in mappings.iter().enumerate() {
        let area = |i: usize| found[i].bounds.area();
        let slot = &mut largest[mapping.slot_index];
        if slot.is_none_or(|best| area(index) > area(best)) {
            *slot = Some(index);
        }
    }
    let stand_ins: Vec<(usize, Arc<Object>)> = largest
        .iter()
        .enumerate()
        .filter_map(|(slot, piece)| {
            let f = &found[(*piece)?];
            // An instance measures as a 2×2 box without the library; its
            // resolved bounds stand in for its outline.
            let art = match f.world.kind {
                ObjectKind::Shape(_) | ObjectKind::Group(_) => f.world.clone(),
                _ => Arc::new(Object::shape(
                    ObjectId(0),
                    buzz_scene::ShapeData::filled(f.bounds.to_path(0.1), peniko::Color::BLACK),
                )),
            };
            Some((slot, art))
        })
        .collect();
    let fitted = buzz_act::autorig::fit_slots(&pattern, &stand_ins);
    let joints: Vec<Option<Point>> = (0..pattern.slots.len())
        .map(|slot| {
            largest[slot]?;
            // A slot with parts cut from a merged drawing turns about the
            // joint it was cut at, where its overlap disc is.
            if cut_for.iter().any(|c| *c == Some(slot)) {
                // S1: where the cut style put the slot's pivot.
                if let Some((_, p)) = split_done.as_ref().and_then(|(_, s)| s.pivots.iter().find(|(x, _)| *x == slot)) {
                    return Some(*p);
                }
                return capsules.get(slot).map(|c| c.head);
            }
            Some(
                detected
                    .filter(|_| !contradicted[slot])
                    .and_then(|d| d.get(slot).copied().flatten())
                    .or(fitted[slot])
                    .or(guessed[slot])
                    .map_or_else(|| extent.center(), |(head, _)| head),
            )
        })
        .collect();

    // Leads, names and symbol renames by R1's rules (`name_layer_rig_pieces`).
    let slots: Vec<usize> = mappings.iter().map(|m| m.slot_index).collect();
    let matched: Vec<bool> = mappings.iter().map(|m| m.overlap_score > 0.0).collect();
    // R4d: an underlay part never leads its slot and is named after the lead.
    let underlay: Vec<bool> = found
        .iter()
        .map(|f| split_done.as_ref().is_some_and(|(_, s)| s.underlays.contains(&f.world.id)))
        .collect();
    // S1: a cut style's extra (a scarf) never leads its slot either.
    let never_lead: Vec<bool> = found
        .iter()
        .zip(&underlay)
        .map(|(f, u)| *u || split_done.as_ref().is_some_and(|(_, s)| s.extras.iter().any(|(id, _)| *id == f.world.id)))
        .collect();
    let mut naming =
        name_layer_rig_pieces(scene, &character.name, &found, &slots, &matched, &never_lead, &joints, profile, &mut notes);
    name_underlays(&mut naming.names, &naming.leads, &slots, &underlay);
    let objects: Vec<ObjectId> = found.iter().map(|f| f.world.id).collect();
    name_style_extras(&mut naming.names, &objects, split_done.as_ref().map(|(_, s)| s));
    let (head_symbol, head_renames) = head_face_renames(scene, &found, &slots, &naming.leads, profile);
    if let Some(head) = head_symbol.and_then(|id| scene.library().get(id))
        && !head_renames.is_empty()
    {
        notes.push(format!(
            "face layers in '{}': {}",
            head.name,
            head_renames.iter().map(|r| format!("{} \u{2192} {}", r.from, r.to)).collect::<Vec<_>>().join(", ")
        ));
    }
    let mut side_calls = side_calls;
    side_calls.append(&mut naming.side_calls);
    let pieces: Vec<PlannedPiece> = found
        .iter()
        .zip(&mappings)
        .enumerate()
        .map(|(index, (f, mapping))| PlannedPiece {
            layer: f.layer,
            object: f.world.id,
            slot: mapping.slot_index,
            slot_name: pattern.slots[mapping.slot_index].name.clone(),
            new_layer_name: naming.names[index].clone(),
            lead: naming.leads[index],
            score: mapping.overlap_score,
        })
        .collect();
    let symbol_renames = naming.symbol_renames;

    let sure = pieces.iter().filter(|p| p.score >= 0.5).count() as f64 / pieces.len() as f64;
    let confidence = match source {
        JointSource::DwPose => sure,
        JointSource::FitSlots => sure * 0.6,
    };
    if source == JointSource::FitSlots {
        notes.push("joints fitted to the drawings (no pose detector): lower confidence".into());
    }
    Some(LayerRigPlan {
        symbol,
        frame,
        pieces,
        symbol_renames,
        joints,
        source,
        confidence,
        low_confidence: source == JointSource::FitSlots || confidence < 0.5,
        notes,
        side_calls,
        profile: profile.clone(),
        head_symbol,
        head_renames,
        merged_found,
        split: split_done.map(|(_, s)| s),
        detected: detected.map(<[_]>::to_vec),
        cut_style: style.cloned(),
        rig_head: false,
    })
}

/// [`plan_layer_rig`] with DWPose's joints when the pose models are installed
/// and find a figure (`crate::app::detect_symbol_joints`), otherwise the
/// `fit_slots` fallback, with a note saying why. `split` as
/// [`plan_layer_rig_with`]: `None` splits merged drawings when there are any.
pub fn plan_layer_rig_auto(
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    profile: &buzz_rig::pattern::NamingProfile,
    split: Option<bool>,
) -> Option<LayerRigPlan> {
    // S1: a merged drawing is split by "My Animate cut style" by default.
    let style = buzz_rig::cut_style::CutStyle::animate_convention();
    plan_layer_rig_auto_styled(scene, symbol, frame, profile, split, Some(&style))
}

/// [`plan_layer_rig_auto`] with the cut style to split merged drawings by
/// (`None`: R4's generic split).
pub fn plan_layer_rig_auto_styled(
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    profile: &buzz_rig::pattern::NamingProfile,
    split: Option<bool>,
    style: Option<&buzz_rig::cut_style::CutStyle>,
) -> Option<LayerRigPlan> {
    let (detected, why) = match crate::app::detect_symbol_joints(scene.clone(), symbol) {
        Ok(joints) => (Some(joints), None),
        Err(e) => (None, Some(format!("DWPose not used: {e}"))),
    };
    let mut plan = plan_layer_rig_styled(scene, symbol, frame, detected.as_deref(), profile, split, style)?;
    if let Some(why) = why {
        plan.notes.insert(0, why);
    }
    Some(plan)
}

/// `plan` made again from the same joints, with R4's split on or off (the
/// review's "Split merged drawing"), keeping its note on why DWPose was not
/// used. The review keeps its slot corrections and typed names (R4b).
pub fn replan_layer_rig(scene: &Scene, plan: &LayerRigPlan, split: bool) -> Option<LayerRigPlan> {
    let mut again = plan_layer_rig_styled(
        scene,
        plan.symbol,
        plan.frame,
        plan.detected.as_deref(),
        &plan.profile,
        Some(split),
        plan.cut_style.as_ref(),
    )?;
    if let Some(why) = plan.notes.first().filter(|n| n.starts_with("DWPose not used")) {
        again.notes.insert(0, why.clone());
    }
    Some(again)
}

/// The scene a plan's pieces are in: `scene` itself, or with R4's split
/// installed (plan-time ids) when the plan cut a merged drawing.
fn plan_scene<'a>(scene: &'a Scene, plan: &LayerRigPlan) -> std::borrow::Cow<'a, Scene> {
    let Some(split) = &plan.split else {
        return std::borrow::Cow::Borrowed(scene);
    };
    let mut view = scene.clone();
    if let Some(stack) = view.library_mut().layers_mut(plan.symbol) {
        for layer in &split.layers {
            stack.update(layer.id, |l| *l = (**layer).clone());
        }
    }
    std::borrow::Cow::Owned(view)
}

// ---------------------------------------------------------------------------
// R4: a merged drawing cut along the skeleton. Each path region goes to the
// bone capsule it lies nearest (a Voronoi partition around the bone segments
// weighted by the capsules' radii, the side of a joint towards the parent
// belonging to the parent); a region
// spanning a joint is cut against those regions by boolean intersection
// (`buzz_geom::boolean`, i_overlay). Around each joint, overlap discs of the
// artwork either side are painted under the layer's artwork, so a bend
// shows no gap.
// ---------------------------------------------------------------------------

/// S1/S2: the style-driven split (`CutStyle`).
mod style_cut;
use style_cut::split_merged_styled;

/// H1: Rig Head (`HeadStyle`), `docs/HEAD_RIG_PLAN.md`.
mod head_rig;
mod head_cut;
pub use head_rig::{HeadRigReport, flatten_rest, head_rig_flag, head_symbol_for, head_target, rig_head};

/// The `biped_full` slots a merged drawing is cut between: every body part
/// but the brows, which sit inside the head.
const SPLIT_SLOTS: usize = 15;

/// A body part's share of a vector piece's area from which it counts as
/// covered; two make the piece a merged drawing.
const MERGED_SHARE: f64 = 0.1;

/// A body part's share of one path region's area from which the region is
/// cut for it; a region with less than this on every other part goes whole.
const CUT_SHARE: f64 = 0.02;

/// The label grid of a [`BonePartition`]: every cell given to the bone its
/// centre belongs to.
struct BoneGrid {
    origin: Point,
    cell: f64,
    cols: usize,
    rows: usize,
    labels: Vec<u8>,
}

/// The plane split between the bones of a figure.
struct BonePartition {
    /// Per bone: its slot, head and tip, and the direction whose negative
    /// side, from the head, lies behind the joint, on the parent's side
    /// (`None` for the root).
    bones: Vec<(usize, Point, Point, Option<Vec2>)>,
    /// Each bone's capsule radius: a point belongs to the capsule whose
    /// surface it is nearest, so a wide torso keeps its flanks.
    radius: Vec<f64>,
    /// Added to the distance of a point behind a bone's joint.
    penalty: f64,
    /// The figure's extent.
    bounds: buzz_geom::Rect,
    grid: std::cell::OnceCell<BoneGrid>,
    /// Each bone's region, and the same grown by [`BonePartition::seam`].
    regions: std::cell::OnceCell<(Vec<buzz_geom::BezPath>, Vec<buzz_geom::BezPath>)>,
}

fn segment_distance(p: Point, head: Point, tip: Point) -> f64 {
    let seg = tip - head;
    let len_sq = seg.hypot2();
    if len_sq <= 1e-12 {
        return (p - head).hypot();
    }
    let t = ((p - head).dot(seg) / len_sq).clamp(0.0, 1.0);
    (p - (head + seg * t)).hypot()
}

impl BonePartition {
    fn new(capsules: &[buzz_ai::rerig::BoneCapsule], bounds: buzz_geom::Rect) -> Self {
        let pattern = RigPattern::biped_full();
        let bones = capsules
            .iter()
            .take(SPLIT_SLOTS)
            .map(|cap| {
                let slot = cap.slot_index;
                // Along a chain (elbow, wrist, knee, ankle, waist, neck) the
                // joint line crosses the parent bone; where a limb branches
                // off (shoulder, hip) it crosses the limb itself.
                let dir = pattern.slots.get(slot).and_then(|s| s.parent).and_then(|parent| {
                    let p = capsules.get(parent)?;
                    let up = p.tail - p.head;
                    let own = cap.tail - cap.head;
                    let chain = up.hypot() > 1e-9 && (cap.head - p.tail).hypot() < 0.3 * up.hypot();
                    let d = if chain { up } else { own };
                    (d.hypot() > 1e-9).then(|| d / d.hypot())
                });
                (slot, cap.head, cap.tail, dir)
            })
            .collect();
        let long = bounds.width().max(bounds.height()).max(1.0);
        Self {
            bones,
            radius: capsules.iter().take(SPLIT_SLOTS).map(|c| c.radius).collect(),
            penalty: long * 4.0,
            bounds,
            grid: std::cell::OnceCell::new(),
            regions: std::cell::OnceCell::new(),
        }
    }

    fn long(&self) -> f64 {
        self.bounds.width().max(self.bounds.height()).max(1.0)
    }

    /// How far a part lower in paint order reaches under the one above it,
    /// and how far an overlap disc keeps inside the part it copies: one grid
    /// cell, so anti-aliased edges never meet at rest.
    fn seam(&self) -> f64 {
        self.long() / 1024.0
    }

    fn opts(&self, fill: buzz_geom::FillMode) -> buzz_geom::BooleanOptions {
        buzz_geom::BooleanOptions { fill, ..buzz_geom::BooleanOptions::for_shape_size(self.long()).fast() }
    }

    /// The bone `p` belongs to: the capsule whose surface it is nearest,
    /// a point behind a joint (on the parent's side) never the child's.
    fn bone_at(&self, p: Point) -> usize {
        let mut best = (f64::INFINITY, 0);
        for (index, (_, head, tip, dir)) in self.bones.iter().enumerate() {
            let mut d = segment_distance(p, *head, *tip) - self.radius[index];
            if let Some(dir) = dir
                && (p - *head).dot(*dir) < 0.0
            {
                d += self.penalty;
            }
            if d < best.0 {
                best = (d, index);
            }
        }
        best.1
    }

    /// The figure's extent (and a margin) on a grid of 1/1024 of its size.
    fn grid(&self) -> &BoneGrid {
        self.grid.get_or_init(|| {
            let long = self.long();
            let cell = long / 1024.0;
            let area = self.bounds.inflate(long * 0.05, long * 0.05);
            let cols = (area.width() / cell).ceil().max(1.0) as usize;
            let rows = (area.height() / cell).ceil().max(1.0) as usize;
            let mut labels = Vec::with_capacity(cols * rows);
            for r in 0..rows {
                for c in 0..cols {
                    let p = Point::new(area.x0 + (c as f64 + 0.5) * cell, area.y0 + (r as f64 + 0.5) * cell);
                    labels.push(self.bone_at(p) as u8);
                }
            }
            BoneGrid { origin: Point::new(area.x0, area.y0), cell, cols, rows, labels }
        })
    }

    /// The bones whose regions `rect` touches.
    fn bones_in(&self, rect: buzz_geom::Rect) -> Vec<usize> {
        let g = self.grid();
        let col = |x: f64| ((x - g.origin.x) / g.cell).floor();
        let row = |y: f64| ((y - g.origin.y) / g.cell).floor();
        let (c0, c1) = (col(rect.x0).max(0.0), col(rect.x1).min(g.cols as f64 - 1.0));
        let (r0, r1) = (row(rect.y0).max(0.0), row(rect.y1).min(g.rows as f64 - 1.0));
        let mut seen = vec![false; self.bones.len()];
        if c0 <= c1 && r0 <= r1 {
            for r in r0 as usize..=r1 as usize {
                for c in c0 as usize..=c1 as usize {
                    seen[g.labels[r * g.cols + c] as usize] = true;
                }
            }
        }
        // Anything past the grid's edge goes with the bone it is nearest.
        if !(c0 <= c1 && r0 <= r1) || rect.x0 < g.origin.x || rect.y0 < g.origin.y {
            seen[self.bone_at(rect.center())] = true;
        }
        (0..seen.len()).filter(|&b| seen[b]).collect()
    }

    /// Each bone's region as a path (the cells of the bone merged; the
    /// regions tile the grid exactly), and each grown by [`Self::seam`].
    fn regions(&self) -> &(Vec<buzz_geom::BezPath>, Vec<buzz_geom::BezPath>) {
        use kurbo::Shape as _;
        self.regions.get_or_init(|| {
            let g = self.grid();
            let mut runs: Vec<buzz_geom::BezPath> = vec![buzz_geom::BezPath::new(); self.bones.len()];
            for r in 0..g.rows {
                let y0 = g.origin.y + r as f64 * g.cell;
                let mut start = 0;
                let mut current = g.labels[r * g.cols];
                for c in 1..=g.cols {
                    let next = if c < g.cols { g.labels[r * g.cols + c] } else { u8::MAX };
                    if next != current {
                        let rect = buzz_geom::Rect::new(
                            g.origin.x + start as f64 * g.cell,
                            y0,
                            g.origin.x + c as f64 * g.cell,
                            y0 + g.cell,
                        );
                        runs[current as usize].extend(rect.to_path(0.1));
                        start = c;
                        current = next;
                    }
                }
            }
            // One overlay per bone: every run rectangle in one path, merged
            // under the non-zero rule (they all wind the same way) by
            // intersecting with the whole grid, which takes nothing away.
            let opts = self.opts(buzz_geom::FillMode::NonZero);
            let whole = buzz_geom::Rect::new(
                g.origin.x,
                g.origin.y,
                g.origin.x + g.cols as f64 * g.cell,
                g.origin.y + g.rows as f64 * g.cell,
            )
            .to_path(0.1);
            let regions: Vec<buzz_geom::BezPath> = runs
                .iter()
                .map(|path| {
                    if path.elements().is_empty() {
                        return buzz_geom::BezPath::new();
                    }
                    buzz_geom::boolean(path, &whole, buzz_geom::BoolOp::Intersect, opts)
                })
                .collect();
            let grown = regions.iter().map(|r| buzz_geom::expand_fill(r, self.seam(), opts)).collect();
            (regions, grown)
        })
    }

    /// How many bones sit between `bone` and the root.
    fn depth(&self, bone: usize) -> usize {
        let pattern = RigPattern::biped_full();
        let mut slot = self.bones[bone].0;
        let mut depth = 0;
        while let Some(parent) = pattern.slots.get(slot).and_then(|s| s.parent) {
            slot = parent;
            depth += 1;
        }
        depth
    }

    /// The parent bone of `bone` in the pattern, if it is in the partition.
    fn parent(&self, bone: usize) -> Option<usize> {
        let pattern = RigPattern::biped_full();
        let parent = pattern.slots.get(self.bones[bone].0)?.parent?;
        self.bones.iter().position(|b| b.0 == parent)
    }

    /// R4c: the bone `bone` turns from at its joint — its pattern parent, or
    /// the hips for a thigh (a root in the pattern).
    fn joint_parent(&self, bone: usize) -> Option<usize> {
        self.parent(bone).or_else(|| {
            matches!(self.bones[bone].0, 7 | 9).then(|| self.bones.iter().position(|b| b.0 == 0)).flatten()
        })
    }

    /// How many [`Self::joint_parent`] steps lie between `bone` and the root.
    fn joint_depth(&self, bone: usize) -> usize {
        let mut depth = 0;
        let mut at = bone;
        while let Some(up) = self.joint_parent(at) {
            depth += 1;
            at = up;
            if depth > self.bones.len() {
                break;
            }
        }
        depth
    }

    /// Per bone: whether it is `bone` or lies below it.
    fn subtree(&self, bone: usize) -> Vec<bool> {
        (0..self.bones.len())
            .map(|b| {
                let mut at = Some(b);
                for _ in 0..=self.bones.len() {
                    match at {
                        Some(x) if x == bone => return true,
                        Some(x) => at = self.joint_parent(x),
                        None => return false,
                    }
                }
                false
            })
            .collect()
    }

    /// The direction whose negative side, from `bone`'s joint, lies behind
    /// the joint line (the parent's side); a root's own direction.
    fn joint_dir(&self, bone: usize) -> Vec2 {
        let (_, head, tip, dir) = self.bones[bone];
        dir.unwrap_or_else(|| {
            let d = tip - head;
            if d.hypot() > 1e-9 { d / d.hypot() } else { Vec2::new(0.0, 1.0) }
        })
    }

    /// **R4c.** The plane split again by the drawing itself. The capsules
    /// alone hand a garment's edge to the limb it lies over (a shirt flap to
    /// the thigh, a collar to the upper arm). So the paints (in the figure's
    /// space, back to front) are laid on the grid, the top one per cell, and
    /// the fills joined cell to cell without a line between (thin paints:
    /// ink, folds, marks) make *panels*. At each joint, root first, a panel
    /// reaching across is cut only when it is a limb crossing the joint: on
    /// the child's side it runs at least half the bone's length along it and
    /// lies mostly within twice the capsule's radius of it. Otherwise it goes
    /// whole: its part past the joint to the parent's side when its centre
    /// lies behind the joint line (above the hips, inside the shoulder),
    /// else all of it to the bone holding most of it. A line cell then takes
    /// the label of the fills beside it, the one nearer the root where two
    /// panels meet (a hem stays with its garment), and a bare cell next to
    /// paint that of the paint, so no edge is cut off its part.
    fn refined(&self, paints: &[(buzz_geom::BezPath, buzz_geom::FillMode, bool)]) -> (BonePartition, Vec<bool>) {
        use kurbo::Shape as _;
        let g = self.grid();
        let (cols, rows, cell, origin) = (g.cols, g.rows, g.cell, g.origin);
        let n = cols * rows;
        let centre = |i: usize| {
            Point::new(origin.x + ((i % cols) as f64 + 0.5) * cell, origin.y + ((i / cols) as f64 + 0.5) * cell)
        };
        const NONE: u32 = u32::MAX;

        // 1. The top paint per cell (a cell is a paint's when its centre is
        // inside), and which paints are lines.
        let mut cover = vec![NONE; n];
        let mut line = vec![false; paints.len()];
        let mut xs: Vec<(f64, i32)> = Vec::new();
        for (index, (path, rule, _)) in paints.iter().enumerate() {
            let edges = path_edges(path, cell * 0.25);
            let perimeter: f64 = edges.iter().map(|(a, b)| (*b - *a).hypot()).sum();
            line[index] = perimeter <= 1e-9 || 2.0 * path.area().abs() / perimeter < LINE_CELLS * cell;
            let bbox = path.bounding_box();
            let r0 = ((bbox.y0 - origin.y) / cell - 0.5).ceil().max(0.0);
            let r1 = ((bbox.y1 - origin.y) / cell - 0.5).floor().min(rows as f64 - 1.0);
            if r1 < r0 {
                continue;
            }
            for r in r0 as usize..=r1 as usize {
                let y = origin.y + (r as f64 + 0.5) * cell;
                xs.clear();
                for (a, b) in &edges {
                    if (a.y <= y) != (b.y <= y) {
                        let t = (y - a.y) / (b.y - a.y);
                        xs.push((a.x + t * (b.x - a.x), if b.y > a.y { 1 } else { -1 }));
                    }
                }
                xs.sort_by(|p, q| p.0.total_cmp(&q.0));
                let mut wind = 0;
                for k in 0..xs.len().saturating_sub(1) {
                    wind += xs[k].1;
                    let inside = match rule {
                        buzz_geom::FillMode::EvenOdd => k % 2 == 0,
                        buzz_geom::FillMode::NonZero => wind != 0,
                    };
                    if !inside {
                        continue;
                    }
                    let c0 = ((xs[k].0 - origin.x) / cell - 0.5).ceil().max(0.0);
                    let c1 = ((xs[k + 1].0 - origin.x) / cell - 0.5).floor().min(cols as f64 - 1.0);
                    if c1 < c0 {
                        continue;
                    }
                    for c in c0 as usize..=c1 as usize {
                        cover[r * cols + c] = index as u32;
                    }
                }
            }
        }
        let fill = |i: usize| cover[i] != NONE && !line[cover[i] as usize];
        let neighbours = |i: usize| {
            let (c, r) = (i % cols, i / cols);
            [
                (c > 0).then(|| i - 1),
                (c + 1 < cols).then(|| i + 1),
                (r > 0).then(|| i - cols),
                (r + 1 < rows).then(|| i + cols),
            ]
        };

        // 2. Panels: fills joined cell to cell.
        let mut panel = vec![NONE; n];
        let mut panels: Vec<Vec<usize>> = Vec::new();
        let mut queue: Vec<usize> = Vec::new();
        for start in 0..n {
            if !fill(start) || panel[start] != NONE {
                continue;
            }
            let id = panels.len() as u32;
            let mut cells = Vec::new();
            panel[start] = id;
            queue.push(start);
            while let Some(i) = queue.pop() {
                cells.push(i);
                for j in neighbours(i).into_iter().flatten() {
                    if fill(j) && panel[j] == NONE {
                        panel[j] = id;
                        queue.push(j);
                    }
                }
            }
            panels.push(cells);
        }

        // 3. At each joint, root first: a panel across it is cut only when
        // it is a limb crossing it.
        let bones = self.bones.len();
        let mut labels = g.labels.clone();
        let mut order: Vec<usize> = (0..bones).filter(|b| self.joint_parent(*b).is_some()).collect();
        order.sort_by_key(|b| (self.joint_depth(*b), *b));
        for c in order {
            let Some(parent) = self.joint_parent(c) else { continue };
            let sub = self.subtree(c);
            let (_, head, tip, _) = self.bones[c];
            let len = (tip - head).hypot();
            if len <= 1e-9 {
                continue;
            }
            let u = (tip - head) / len;
            let dir = self.joint_dir(c);
            let reach = 2.0 * self.radius[c];
            for cells in &panels {
                let mut count = vec![0usize; bones];
                let (mut on_sub, mut extent, mut within) = (0usize, f64::NEG_INFINITY, 0usize);
                let mut sum = Vec2::ZERO;
                for &i in cells {
                    let b = labels[i] as usize;
                    count[b] += 1;
                    let p = centre(i);
                    sum += p.to_vec2();
                    if sub[b] {
                        on_sub += 1;
                    }
                    if b == c {
                        extent = extent.max((p - head).dot(u));
                        if segment_distance(p, head, tip) <= reach {
                            within += 1;
                        }
                    }
                }
                if on_sub == 0 || on_sub == cells.len() {
                    continue;
                }
                let own = count[c];
                let limb = own > 0 && extent >= 0.5 * len && within * 2 >= own;
                let centroid = (sum / cells.len() as f64).to_point();
                let behind = (centroid - head).dot(dir) < 0.0;
                let to = if limb {
                    None
                } else if behind {
                    Some(if count[parent] > 0 {
                        parent
                    } else {
                        (0..bones).filter(|b| !sub[*b]).max_by_key(|b| (count[*b], usize::MAX - b)).unwrap_or(parent)
                    })
                } else {
                    (0..bones).max_by_key(|b| (count[*b], usize::MAX - b))
                };
                let Some(to) = to else { continue };
                for &i in cells {
                    if !behind || sub[labels[i] as usize] {
                        labels[i] = to as u8;
                    }
                }
            }
        }

        // 3b. A loose paper mark: a small panel all in the stage's colour,
        // off the figure (it meets bare ground and lines only, no other
        // fill). Invisible where drawn, it would show as a white sliver on
        // whatever a limb carried it over, so its paints stay with the root
        // (the caller puts them there whole; the cells keep their label, so
        // the ink beside them is not cut).
        let mut exterior = vec![false; n];
        let mut front: Vec<usize> = (0..n)
            .filter(|&i| {
                let (c, r) = (i % cols, i / cols);
                (c == 0 || r == 0 || c + 1 == cols || r + 1 == rows) && cover[i] == NONE
            })
            .collect();
        for &i in &front {
            exterior[i] = true;
        }
        while let Some(i) = front.pop() {
            for j in neighbours(i).into_iter().flatten() {
                if cover[j] == NONE && !exterior[j] {
                    exterior[j] = true;
                    front.push(j);
                }
            }
        }
        let painted = cover.iter().filter(|c| **c != NONE).count();
        let mut loose = vec![false; n];
        let mut loose_paints = vec![false; paints.len()];
        for (id, cells) in panels.iter().enumerate() {
            if cells.len() as f64 > LOOSE_SHARE * painted as f64 || !cells.iter().all(|&i| paints[cover[i] as usize].2) {
                continue;
            }
            let (mut ground, mut other) = (false, false);
            for &i in cells {
                for j in neighbours(i).into_iter().flatten() {
                    if exterior[j] {
                        ground = true;
                    } else if fill(j) && panel[j] != id as u32 {
                        other = true;
                    }
                }
            }
            if !ground || other {
                continue;
            }
            for &i in cells {
                loose[i] = true;
                loose_paints[cover[i] as usize] = true;
            }
        }

        // 4. Line cells: the label of the fills beside them (not loose
        // marks), the one nearest the root among those within twice the
        // nearest's distance (and a little), through the line.
        let fill = |i: usize| fill(i) && !loose[i];
        let mut dist = vec![u32::MAX; n];
        let mut near = vec![NONE; n];
        let mut front: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for i in 0..n {
            if fill(i) {
                dist[i] = 0;
                near[i] = i as u32;
                front.push_back(i);
            }
        }
        while let Some(i) = front.pop_front() {
            for j in neighbours(i).into_iter().flatten() {
                if cover[j] != NONE && !loose[j] && dist[j] == u32::MAX {
                    dist[j] = dist[i] + 1;
                    near[j] = near[i];
                    front.push_back(j);
                }
            }
        }
        let depth: Vec<usize> = (0..bones).map(|b| self.joint_depth(b)).collect();
        let fill_labels = labels.clone();
        for i in 0..n {
            if cover[i] == NONE || fill(i) || loose[i] || near[i] == NONE {
                continue;
            }
            let d = dist[i] as i64;
            let window = (2 * d + 3).min(LINE_WINDOW);
            let (c, r) = ((i % cols) as i64, (i / cols) as i64);
            let mut best: Option<(usize, i64, usize)> = None;
            for dy in -window..=window {
                for dx in -window..=window {
                    let (x, y) = (c + dx, r + dy);
                    if x < 0 || y < 0 || x >= cols as i64 || y >= rows as i64 || dx * dx + dy * dy > window * window {
                        continue;
                    }
                    let j = y as usize * cols + x as usize;
                    if !fill(j) {
                        continue;
                    }
                    let b = fill_labels[j] as usize;
                    let key = (depth[b], dx * dx + dy * dy, b);
                    if best.is_none_or(|k| key < k) {
                        best = Some(key);
                    }
                }
            }
            labels[i] = best.map_or(fill_labels[near[i] as usize], |(_, _, b)| b as u8);
        }

        // 5. Bare cells next to paint: the paint's label.
        let mut steps = vec![u32::MAX; n];
        let mut front: std::collections::VecDeque<usize> = (0..n).filter(|i| cover[*i] != NONE).collect();
        for &i in &front {
            steps[i] = 0;
        }
        while let Some(i) = front.pop_front() {
            if steps[i] >= BARE_CELLS {
                continue;
            }
            for j in neighbours(i).into_iter().flatten() {
                if cover[j] == NONE && steps[j] == u32::MAX {
                    steps[j] = steps[i] + 1;
                    labels[j] = labels[i];
                    front.push_back(j);
                }
            }
        }

        let refined = BonePartition {
            bones: self.bones.clone(),
            radius: self.radius.clone(),
            penalty: self.penalty,
            bounds: self.bounds,
            grid: std::cell::OnceCell::from(BoneGrid { origin, cell, cols, rows, labels }),
            regions: std::cell::OnceCell::new(),
        };
        (refined, loose_paints)
    }
}

/// R4c: a paint narrower than this many grid cells on average (twice its
/// area over its outline) is a line — ink, a fold, a mark — and parts panels.
const LINE_CELLS: f64 = 3.0;

/// R4c: how far (in cells) a line cell looks for the fills beside it.
const LINE_WINDOW: i64 = 16;

/// R4c: a loose paper mark is a panel of at most this share of the painted
/// cells.
const LOOSE_SHARE: f64 = 0.01;

/// R4c: a solid paint whose channels are all within this of the stage's
/// colour's is paper-coloured.
const PAPER_DIFF: u8 = 40;

/// R4d: a joint cap's or underlay's fill is a paint at least this much lighter
/// (luminance, 0–255) than the outline's colour.
const INK_MARGIN: f64 = 20.0;

/// R4d: a joint cap's clip closes the front piece's silhouette over gaps and
/// holes up to twice this many grid cells wide (a merged drawing's ragged
/// seams, where only the back piece paints at rest).
const CLOSE_CELLS: f64 = 6.0;

/// R4c: how far (in cells) a bare cell next to paint takes the paint's label.
const BARE_CELLS: u32 = 3;

/// R4d: an underlay reaches this many overlap radii from its joint.
const UNDER_REACH: f64 = 1.5;

/// R4d: and no further than this many overlap radii from its parent's artwork.
const UNDER_NEAR: f64 = 0.5;

/// R4d: an underlay smaller than this many grid cells is not made.
const UNDER_MIN_CELLS: f64 = 16.0;

/// R4d: `path` with its holes filled: of each disjoint piece, only its
/// outermost contour.
fn outer_contours(path: &buzz_geom::BezPath) -> buzz_geom::BezPath {
    use kurbo::Shape as _;
    let mut out = buzz_geom::BezPath::new();
    for piece in buzz_geom::split_disjoint(path) {
        let mut contours: Vec<buzz_geom::BezPath> = Vec::new();
        for el in piece.elements() {
            if matches!(el, kurbo::PathEl::MoveTo(_)) || contours.is_empty() {
                contours.push(buzz_geom::BezPath::new());
            }
            if let Some(c) = contours.last_mut() {
                c.push(*el);
            }
        }
        if let Some(outer) = contours.into_iter().max_by(|a, b| a.area().abs().total_cmp(&b.area().abs())) {
            for el in outer.elements() {
                out.push(*el);
            }
        }
    }
    out
}

/// A path flattened to its edges, every subpath closed.
fn path_edges(path: &buzz_geom::BezPath, tolerance: f64) -> Vec<(Point, Point)> {
    let mut edges = Vec::new();
    let mut start: Option<Point> = None;
    let mut last: Option<Point> = None;
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if let (Some(s), Some(l)) = (start, last) {
                edges.push((l, s));
            }
            start = Some(p);
            last = Some(p);
        }
        kurbo::PathEl::LineTo(p) => {
            if let Some(l) = last {
                edges.push((l, p));
            }
            last = Some(p);
        }
        kurbo::PathEl::ClosePath => {
            if let (Some(s), Some(l)) = (start, last) {
                edges.push((l, s));
            }
            last = start;
        }
        _ => {}
    });
    if let (Some(s), Some(l)) = (start, last) {
        edges.push((l, s));
    }
    edges
}

/// One paint of a leaf, in the figure's space: its fill, or its stroke as
/// the outline it paints (Convert Lines to Fills).
struct LeafPaint {
    spec: buzz_scene::FillSpec,
    region: buzz_geom::BezPath,
    /// The region cut by each bone it lies on; only for a region on more
    /// than one bone.
    parts: Vec<(usize, buzz_geom::BezPath)>,
}

/// One leaf of a vector piece: a shape (or anything else a group holds).
struct LeafLabels {
    /// From the piece's own space to the leaf's.
    inner: Affine,
    object: Arc<Object>,
    /// Its paints, when it is a plain shape that can be cut (not a bitmap
    /// fill, no filters).
    paints: Vec<LeafPaint>,
    /// Its area per bone of the partition.
    area: Vec<f64>,
}

impl LeafLabels {
    /// The bones it is cut between, most area first: `None` when it goes
    /// whole (no other bone holds [`CUT_SHARE`] of it), with its bone.
    fn cut(&self) -> Result<Vec<usize>, usize> {
        let sum: f64 = self.area.iter().sum();
        let top = (0..self.area.len()).max_by(|&a, &b| self.area[a].total_cmp(&self.area[b])).unwrap_or(0);
        let shared = (0..self.area.len()).filter(|&b| sum > 0.0 && self.area[b] / sum >= CUT_SHARE).count();
        if shared < 2 || self.paints.is_empty() {
            return Err(top);
        }
        let mut bones: Vec<usize> = (0..self.area.len()).filter(|&b| self.area[b] > 0.0).collect();
        bones.sort_by(|&a, &b| self.area[b].total_cmp(&self.area[a]));
        Ok(bones)
    }
}

/// A vector piece measured against the partition.
struct PieceLabels {
    leaves: Vec<LeafLabels>,
    /// Two or more bones each hold [`MERGED_SHARE`] of its area.
    merged: bool,
}

/// An object with nothing on it but its artwork: no filters, blend, effect.
fn plain_object(o: &Object) -> bool {
    let plain = Object::group(ObjectId(0), Vec::new());
    o.visible && o.filters.is_empty() && o.blend == plain.blend && o.modifiers.is_empty() && o.text.is_none()
}

/// How `world` (a piece standing where it is drawn) covers the bones of
/// `partition`, leaf by leaf: a paint on more than one bone's region is cut
/// against each by boolean intersection, so the areas are exact. `None` for
/// anything but a plain shape or group.
fn piece_labels(scene: &Scene, world: &Object, partition: &BonePartition) -> Option<PieceLabels> {
    use kurbo::Shape as _;
    if !matches!(world.kind, ObjectKind::Shape(_) | ObjectKind::Group(_)) || !plain_object(world) {
        return None;
    }
    fn walk(object: &Arc<Object>, inner: Affine, out: &mut Vec<(Affine, Arc<Object>)>) {
        match &object.kind {
            ObjectKind::Group(children) => {
                for child in children {
                    walk(child, inner * child.transform, out);
                }
            }
            _ => out.push((inner, object.clone())),
        }
    }
    let mut leaves = Vec::new();
    match &world.kind {
        ObjectKind::Group(children) => {
            for child in children {
                walk(child, child.transform, &mut leaves);
            }
        }
        _ => leaves.push((Affine::IDENTITY, Arc::new(world.clone()))),
    }
    let bones = partition.bones.len();
    let mut out = Vec::with_capacity(leaves.len());
    let mut total = vec![0.0; bones];
    for (inner, object) in leaves {
        let at = world.transform * inner;
        let mut area = vec![0.0; bones];
        let mut paints: Vec<LeafPaint> = Vec::new();
        if let ObjectKind::Shape(shape) = &object.kind
            && plain_object(&object)
            && shape.fill.as_ref().is_none_or(|f| !f.paint.is_image())
        {
            if let Some(fill) = &shape.fill {
                paints.push(LeafPaint { spec: fill.clone(), region: at * shape.path.clone(), parts: Vec::new() });
            }
            if let Some(stroke) = shape.stroke.as_ref().filter(|s| s.width > 0.0 && !s.hairline) {
                let outline = buzz_geom::outline_stroke(&shape.path, buzz_geom::StrokeStyle::new(stroke.width), 0.05);
                paints.push(LeafPaint {
                    spec: buzz_scene::FillSpec {
                        paint: stroke.paint.clone(),
                        rule: buzz_geom::FillMode::NonZero,
                        swatch: stroke.swatch.clone(),
                    },
                    region: at * outline,
                    parts: Vec::new(),
                });
            }
            paints.retain(|p| !p.region.elements().is_empty());
        }
        let mut measured = false;
        for paint in &mut paints {
            let bbox = paint.region.bounding_box();
            let on = partition.bones_in(bbox);
            if on.len() == 1 {
                area[on[0]] += paint.region.area().abs().max(1e-9);
                measured = true;
                continue;
            }
            let (regions, _) = partition.regions();
            let opts = partition.opts(paint.spec.rule);
            for b in on {
                let part = buzz_geom::boolean(&paint.region, &regions[b], buzz_geom::BoolOp::Intersect, opts);
                let a = part.area().abs();
                if a > 1e-9 {
                    area[b] += a;
                    paint.parts.push((b, part));
                    measured = true;
                }
            }
        }
        if !measured {
            // Not a shape, or nothing measurable: where its bounds sit.
            let mut placed = (*object).clone();
            placed.transform = at;
            let bounds = scene.resolved_bounds(&placed);
            area[partition.bone_at(bounds.center())] += bounds.area().max(1e-9);
            paints.clear();
        }
        for (t, a) in total.iter_mut().zip(&area) {
            *t += a;
        }
        out.push(LeafLabels { inner, object, paints, area });
    }
    let sum: f64 = total.iter().sum();
    let covered = total.iter().filter(|a| sum > 0.0 && **a / sum >= MERGED_SHARE).count();
    Some(PieceLabels { leaves: out, merged: covered >= 2 })
}

/// One thing painted on a split layer, back to front: a leaf (whole or a
/// cut part or an overlap disc) on a bone, in the piece's space with the
/// piece's placement folded in, or an object left as it is.
enum SplitItem {
    Part(usize, Object),
    Kept(Arc<Object>),
}

/// **R4.** Cut the merged pieces in `targets` along `partition`'s bones, on
/// a copy of `scene`. Every plain vector piece on a layer holding one is
/// read leaf by leaf (shape by shape): a leaf goes whole to the bone holding
/// it, or, spanning a joint, is cut against the bones' regions (a stroke as
/// its outline), the parts lower in paint order reaching a grid cell under
/// the ones above so no seam shows. Around each joint the parent gets a
/// copy of the child's artwork within half a limb's width (so the child
/// turning leaves no hole), and a cut child part a copy of its parent's (so
/// the outside of a bend is filled), each kept a cell inside the drawing's
/// outline and painted under everything on the layer, so the picture at
/// rest is unchanged.
/// Everything of one bone on a layer makes one part (R4b; a group when more
/// than one, in paint order, overlap copies first), the parts stacked in the
/// rig's order, on the keyframe showing `frame` and on any other keyframe
/// showing the same drawing. `None` when nothing was cut.
fn split_merged_drawings(
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    targets: Vec<(LayerId, ObjectId, PieceLabels)>,
    base: &BonePartition,
    capsules: &[buzz_ai::rerig::BoneCapsule],
) -> Option<(Scene, MergedSplit)> {
    use kurbo::Shape as _;
    use std::collections::{HashMap, HashSet};
    let stack = &scene.library().get(symbol)?.layers;
    let target_ids: HashSet<(LayerId, ObjectId)> = targets.iter().map(|(l, o, _)| (*l, *o)).collect();
    let mut layer_ids: Vec<LayerId> = Vec::new();
    for (layer, _, _) in &targets {
        if !layer_ids.contains(layer) {
            layer_ids.push(*layer);
        }
    }
    let mut labelled: HashMap<(LayerId, ObjectId), PieceLabels> =
        targets.into_iter().map(|(l, o, p)| ((l, o), p)).collect();

    // R4c: the plane split again by the drawing itself (see
    // `BonePartition::refined`), from every paint on the layers, back to front.
    // A solid paint within a few percent of the stage's colour is "paper":
    // drawn there, it does not show.
    let paper = scene.stage().background.to_rgba8();
    let is_paper = |spec: &buzz_scene::FillSpec| match &spec.paint {
        buzz_scene::Paint::Solid(c) => {
            let c = c.to_rgba8();
            c.a == 255 && [(c.r, paper.r), (c.g, paper.g), (c.b, paper.b)].iter().all(|(x, y)| x.abs_diff(*y) <= PAPER_DIFF)
        }
        _ => false,
    };
    let mut paints: Vec<(buzz_geom::BezPath, buzz_geom::FillMode, bool)> = Vec::new();
    // Where each object's paints start in `paints`.
    let mut first_paint: HashMap<(LayerId, ObjectId), usize> = HashMap::new();
    for layer_id in &layer_ids {
        let layer = stack.get(*layer_id)?;
        let inherited = stack.inherited_transform(*layer_id, frame);
        for object in layer.objects_at(frame) {
            let labels = labelled.get(&(*layer_id, object.id)).map(|p| {
                p.leaves
                    .iter()
                    .flat_map(|l| l.paints.iter().map(|p| (p.region.clone(), p.spec.rule, is_paper(&p.spec))))
                    .collect::<Vec<_>>()
            });
            let labels = labels.or_else(|| {
                let mut world = (**object).clone();
                world.transform = inherited * world.transform;
                piece_labels(scene, &world, base).map(|p| {
                    p.leaves
                        .iter()
                        .flat_map(|l| l.paints.iter().map(|p| (p.region.clone(), p.spec.rule, is_paper(&p.spec))))
                        .collect()
                })
            });
            first_paint.insert((*layer_id, object.id), paints.len());
            paints.extend(labels.into_iter().flatten());
        }
    }
    let (refined, loose_paints) = base.refined(&paints);
    let partition = &refined;
    let root_bone = partition.bones.iter().position(|b| b.0 == 0);
    // Read again against the refined plane.
    labelled.clear();
    let seam = partition.seam();
    let long = partition.long();

    let mut work = scene.clone();
    let mut originals: Vec<Arc<buzz_scene::Layer>> = Vec::new();
    let mut layers_out: Vec<buzz_scene::Layer> = Vec::new();
    let mut new_ids: Vec<ObjectId> = Vec::new();
    let mut parts_out: Vec<(ObjectId, usize)> = Vec::new();
    let mut underlays: Vec<ObjectId> = Vec::new();
    let mut overlaps: Vec<(usize, Point, f64)> = Vec::new();
    let mut unsplit_keys = 0;
    let merged = target_ids.len();

    let joint_of = |bone: usize| partition.bones[bone].1;
    let dir_of = |bone: usize| {
        partition.bones[bone].3.unwrap_or_else(|| {
            let d = partition.bones[bone].2 - partition.bones[bone].1;
            if d.hypot() > 1e-9 { d / d.hypot() } else { Vec2::new(0.0, 1.0) }
        })
    };
    let cap_of = |bone: usize| capsules.get(partition.bones[bone].0).map_or(long * 0.05, |c| c.radius);

    for layer_id in layer_ids {
        let layer = stack.get(layer_id)?;
        let shown: Vec<Arc<Object>> = layer.objects_at(frame).to_vec();
        let inherited = stack.inherited_transform(layer_id, frame);

        // 1. Every object on the layer: a vector piece read leaf by leaf (a
        // leaf whole on one bone, or cut), anything else kept as it is.
        let mut read: Vec<(Arc<Object>, Option<Vec<(LeafLabels, Result<Vec<usize>, usize>)>>)> = Vec::new();
        // R4c: per object in `read`, which of its leaves are loose paper marks.
        let mut read_loose: Vec<Vec<bool>> = Vec::new();
        for object in &shown {
            let labels = labelled.remove(&(layer_id, object.id)).or_else(|| {
                let mut world = (**object).clone();
                world.transform = inherited * world.transform;
                piece_labels(scene, &world, partition)
            });
            // R4c: a leaf all of loose paper marks goes whole to the root.
            let mut next = first_paint.get(&(layer_id, object.id)).copied();
            let mut loose_flags = Vec::new();
            let leaves = labels.map(|p| {
                p.leaves
                    .into_iter()
                    .map(|l| {
                        let start = next.unwrap_or(usize::MAX);
                        next = next.map(|n| n + l.paints.len());
                        let loose = !l.paints.is_empty()
                            && (start..start.saturating_add(l.paints.len())).all(|i| loose_paints.get(i) == Some(&true));
                        let cut = match root_bone {
                            Some(root) if loose => Err(root),
                            _ => l.cut(),
                        };
                        loose_flags.push(loose && root_bone.is_some());
                        (l, cut)
                    })
                    .collect()
            });
            read.push((object.clone(), leaves));
            read_loose.push(loose_flags);
        }

        // 2. Half the limb's width at each joint, from the artwork of the
        // child bone near the joint line. R4d: every joint a bone turns at
        // (`BonePartition::joint_parent`), so the hips (a thigh turning from
        // the lower cloth) get one too.
        let mut half = vec![0.0f64; partition.bones.len()];
        {
            let mut reach = |bone: usize, path: &buzz_geom::BezPath| {
                if partition.joint_parent(bone).is_none() {
                    return;
                }
                let (joint, dir, cap) = (joint_of(bone), dir_of(bone), cap_of(bone));
                for (a, _) in path_edges(path, 0.5) {
                    let v = a - joint;
                    if (0.0..=cap).contains(&v.dot(dir)) {
                        half[bone] = half[bone].max(v.cross(dir).abs());
                    }
                }
            };
            for (_, leaves) in &read {
                for (l, cut) in leaves.iter().flatten() {
                    match cut {
                        Err(b) => l.paints.iter().for_each(|paint| reach(*b, &paint.region)),
                        Ok(_) => l.paints.iter().for_each(|paint| paint.parts.iter().for_each(|(b, part)| reach(*b, part))),
                    }
                }
            }
        }
        let radius: Vec<f64> = (0..partition.bones.len())
            .map(|b| {
                let cap = cap_of(b);
                if half[b] > 0.0 { half[b].clamp(cap * 0.3, cap * 1.5) } else { cap }
            })
            .collect();
        let discs: Vec<buzz_geom::BezPath> =
            (0..partition.bones.len()).map(|b| kurbo::Circle::new(joint_of(b), radius[b]).to_path(0.05)).collect();
        // The drawing near the joints, a cell inside its outline: an overlap
        // copy is cut to it, so at rest it never reaches the drawing's
        // anti-aliased edge, while copies of neighbouring colours still meet
        // (no hairline between them when a part turns away).
        let jointed: Vec<usize> = (0..partition.bones.len()).filter(|b| partition.joint_parent(*b).is_some()).collect();
        let mut near_all: Vec<buzz_geom::BezPath> = Vec::new();
        for (_, leaves) in &read {
            for (l, _) in leaves.iter().flatten() {
                for paint in &l.paints {
                    let bbox = paint.region.bounding_box();
                    for &b in &jointed {
                        if bbox.intersect(discs[b].bounding_box()).is_zero_area() {
                            continue;
                        }
                        let near = buzz_geom::boolean(
                            &paint.region,
                            &discs[b],
                            buzz_geom::BoolOp::Intersect,
                            partition.opts(paint.spec.rule),
                        );
                        if !near.elements().is_empty() {
                            near_all.push(near);
                        }
                    }
                }
            }
        }
        let nonzero = partition.opts(buzz_geom::FillMode::NonZero);
        let inside_outline =
            buzz_geom::expand_fill(&buzz_geom::union_all(&near_all, nonzero), -seam, nonzero);
        // A copy of `path` (filled by `rule`) near `bone`'s joint.
        let near_joint = |bone: usize, path: &buzz_geom::BezPath, rule: buzz_geom::FillMode| -> Option<buzz_geom::BezPath> {
            if path.bounding_box().intersect(discs[bone].bounding_box()).is_zero_area() {
                return None;
            }
            let near = buzz_geom::boolean(path, &discs[bone], buzz_geom::BoolOp::Intersect, partition.opts(rule));
            if near.elements().is_empty() {
                return None;
            }
            let inside = buzz_geom::boolean(&near, &inside_outline, buzz_geom::BoolOp::Intersect, nonzero);
            (!inside.elements().is_empty() && inside.area().abs() > 1e-9).then_some(inside)
        };

        // 3. The things painted, back to front.
        let mut items: Vec<SplitItem> = Vec::new();
        // The overlap discs, under everything of their slot (R4b: step 4
        // keeps each a cell away from colours on lower slot layers): each is
        // a copy of artwork at its own place, so at rest it does not show.
        let mut under: Vec<SplitItem> = Vec::new();
        // R4b: each item's paint, in the figure's space, by its index in
        // `items`, to read which part was drawn in front where two meet.
        // The key is the paint's leaf and its number in it.
        let mut meet: Vec<(usize, usize, buzz_geom::BezPath, buzz_geom::FillMode, (usize, usize))> = Vec::new();
        // R4d: the paint of each entry in `meet` (its colour, for joint caps
        // and underlays).
        let mut meet_spec: Vec<buzz_scene::FillSpec> = Vec::new();
        // Each overlap copy in the figure's space, the way back to its leaf's
        // space, and the key of the paint it copies.
        let mut under_meta: Vec<(buzz_geom::BezPath, Affine, (usize, usize))> = Vec::new();
        // R4c: per copy, the joint's child bone and whether the copy is the
        // parent's (of the child's artwork) or the child's (of the parent's).
        let mut under_joint: Vec<(usize, bool)> = Vec::new();
        let mut leaf_no = 0usize;
        // R4c: loose paper marks among `items`: the item, its paint in the
        // figure's space and the way back to its leaf's space.
        let mut loose_items: Vec<(usize, buzz_geom::BezPath, Affine)> = Vec::new();
        for ((object, leaves), loose_flags) in read.iter().zip(&read_loose) {
            let Some(leaves) = leaves else {
                items.push(SplitItem::Kept(object.clone()));
                continue;
            };
            let world = inherited * object.transform;
            for (index, (l, cut)) in leaves.iter().enumerate() {
                leaf_no += 1;
                let back = invert(world * l.inner);
                let blend = match &l.object.kind {
                    ObjectKind::Shape(s) => s.blend,
                    _ => buzz_scene::PaintBlend::Normal,
                };
                let as_part = |bone: usize, path: buzz_geom::BezPath, spec: &buzz_scene::FillSpec| {
                    let mut o = (*l.object).clone();
                    o.transform = object.transform * l.inner;
                    o.kind = ObjectKind::Shape(buzz_scene::ShapeData {
                        path: back.unwrap_or(Affine::IDENTITY) * path,
                        fill: Some(spec.clone()),
                        stroke: None,
                        blend,
                    });
                    SplitItem::Part(bone, o)
                };
                let whole_bone = match cut {
                    Err(b) => *b,
                    Ok(bones) => bones[0],
                };
                let is_cut = cut.is_ok() && back.is_some();
                // The leaf's paint per bone, as it will be painted.
                let pieces: Vec<(usize, usize, &buzz_geom::BezPath)> = if is_cut {
                    l.paints
                        .iter()
                        .enumerate()
                        .flat_map(|(k, paint)| paint.parts.iter().map(move |(b, part)| (*b, k, part)))
                        .collect()
                } else {
                    l.paints.iter().enumerate().map(|(k, paint)| (whole_bone, k, &paint.region)).collect()
                };
                if back.is_some() {
                    // Overlap discs, under everything on the layer: the
                    // parent keeps the child's artwork near the joint (no
                    // hole when the child turns); a cut child part takes its
                    // parent's (the outside of a bend filled). R4d: the
                    // joint parent, so a thigh and the lower cloth (separate
                    // roots in the convention) overlap at the hip as well.
                    for (bone, k, path) in &pieces {
                        let rule = if is_cut { buzz_geom::FillMode::NonZero } else { l.paints[*k].spec.rule };
                        if let Some(parent) = partition.joint_parent(*bone)
                            && let Some(copy) = near_joint(*bone, path, rule)
                        {
                            under_meta.push((copy.clone(), back.unwrap_or(Affine::IDENTITY), (leaf_no, *k)));
                            under.push(as_part(parent, copy, &l.paints[*k].spec));
                            under_joint.push((*bone, true));
                        }
                        // R4c: every child of this bone takes a copy of its
                        // artwork near the child's joint, not only a cut
                        // leaf's own part: a child stacked behind keeps it
                        // under the parent, so the parent's edge never opens
                        // onto background as the child turns (step 4 keeps
                        // the copies of children stacked in front out).
                        for child in (0..partition.bones.len()).filter(|c| partition.joint_parent(*c) == Some(*bone)) {
                            if let Some(copy) = near_joint(child, path, rule) {
                                under_meta.push((copy.clone(), back.unwrap_or(Affine::IDENTITY), (leaf_no, *k)));
                                under.push(as_part(child, copy, &l.paints[*k].spec));
                                under_joint.push((child, false));
                            }
                        }
                    }
                }
                if !is_cut {
                    let mut whole = (*l.object).clone();
                    whole.transform = object.transform * l.inner;
                    if loose_flags.get(index) == Some(&true)
                        && let (Some(back), [paint]) = (back, l.paints.as_slice())
                        && matches!(&l.object.kind, ObjectKind::Shape(s) if s.stroke.is_none())
                    {
                        loose_items.push((items.len(), paint.region.clone(), back));
                    }
                    for (k, paint) in l.paints.iter().enumerate() {
                        meet.push((items.len(), whole_bone, paint.region.clone(), paint.spec.rule, (leaf_no, k)));
                        meet_spec.push(paint.spec.clone());
                    }
                    items.push(SplitItem::Part(whole_bone, whole));
                    continue;
                }
                // The cut parts, deepest bone first; each but the top one
                // reaching a cell under the next, so no seam shows.
                let (_, grown) = partition.regions();
                for (k, paint) in l.paints.iter().enumerate() {
                    let mut order: Vec<usize> = paint.parts.iter().map(|(b, _)| *b).collect();
                    order.sort_by_key(|b| (std::cmp::Reverse(partition.depth(*b)), *b));
                    let opts = partition.opts(paint.spec.rule);
                    for (n, b) in order.iter().enumerate() {
                        let Some((_, part)) = paint.parts.iter().find(|(x, _)| x == b) else { continue };
                        let path = if n + 1 < order.len() {
                            let reached = buzz_geom::boolean(&paint.region, &grown[*b], buzz_geom::BoolOp::Intersect, opts);
                            if reached.elements().is_empty() { part.clone() } else { reached }
                        } else {
                            part.clone()
                        };
                        meet.push((items.len(), *b, part.clone(), paint.spec.rule, (leaf_no, k)));
                        meet_spec.push(paint.spec.clone());
                        items.push(as_part(*b, path, &paint.spec));
                    }
                }
            }
        }

        // 4. R4b: one part per slot — everything of one bone on the layer
        // (its overlap copies first, then its artwork in paint order) in one
        // object. A merged drawing's colours never overlap (each cuts the
        // ones under it), so the parts can stack in the rig's order without
        // changing the picture: the head on top; an arm behind or in front
        // of the body, a leg behind or in front of the lower cloth, as drawn
        // where they meet (else arms over the body, legs under the cloth);
        // each limb's lower parts over its upper. Objects kept as they are
        // keep their places; the parts stand where the first one was.
        let bones_n = partition.bones.len();
        let bone_of_slot = |slot: usize| partition.bones.iter().position(|b| b.0 == slot);
        let mut by_bone: Vec<Vec<Object>> = vec![Vec::new(); bones_n];
        let mut mean = vec![(0.0f64, 0usize); bones_n];
        let mut out: Vec<Arc<Object>> = Vec::new();
        let mut parts_at: Option<usize> = None;
        // R4c: where each loose mark went: its bone, its place in the bone's
        // run, its paint and the way back.
        let mut loose_at: Vec<(usize, usize, buzz_geom::BezPath, Affine, usize)> = Vec::new();
        for (index, item) in items.into_iter().enumerate() {
            match item {
                SplitItem::Part(bone, object) => {
                    mean[bone].0 += index as f64;
                    mean[bone].1 += 1;
                    parts_at.get_or_insert(out.len());
                    if let Some((_, region, back)) = loose_items.iter().find(|(i, _, _)| *i == index) {
                        loose_at.push((bone, by_bone[bone].len(), region.clone(), *back, index));
                    }
                    by_bone[bone].push(object);
                }
                SplitItem::Kept(object) => out.push(object),
            }
        }
        // Was `limb` drawn in front of `torso` where they meet (within the
        // limb's joint disc)? `None` when it can't be told.
        let in_front = |limb: usize, torso: usize| -> Option<bool> {
            let (lb, tb) = (bone_of_slot(limb)?, bone_of_slot(torso)?);
            // R4d: a thigh (a pattern root) now has an overlap radius of its
            // own; the evidence is read in the capsule-sized disc it was read
            // in before, so the stacking does not change.
            let root_disc;
            let disc = if partition.parent(lb).is_none() {
                root_disc = kurbo::Circle::new(joint_of(lb), cap_of(lb)).to_path(0.05);
                &root_disc
            } else {
                &discs[lb]
            };
            let dbox = disc.bounding_box();
            let mut sum = [(0.0f64, 0usize); 2];
            for (index, bone, region, rule, _) in &meet {
                let k = if *bone == lb {
                    0
                } else if *bone == tb {
                    1
                } else {
                    continue;
                };
                if region.bounding_box().intersect(dbox).is_zero_area() {
                    continue;
                }
                let near = buzz_geom::boolean(region, disc, buzz_geom::BoolOp::Intersect, partition.opts(*rule));
                if near.elements().is_empty() || near.area().abs() <= 1e-9 {
                    continue;
                }
                sum[k].0 += *index as f64;
                sum[k].1 += 1;
            }
            if sum[0].1 == 0 || sum[1].1 == 0 {
                return None;
            }
            let (ml, mt) = (sum[0].0 / sum[0].1 as f64, sum[1].0 / sum[1].1 as f64);
            (ml != mt).then_some(ml > mt)
        };
        let pattern = RigPattern::biped_full();
        // The stacking tree: the pattern's parents, the thighs (roots in the
        // pattern) under the hips; the head placed last, on top.
        let stack_parent = |slot: usize| -> Option<usize> {
            match slot {
                7 | 9 => Some(0),
                _ => pattern.slots.get(slot).and_then(|s| s.parent),
            }
        };
        let front: Vec<bool> = (0..SPLIT_SLOTS)
            .map(|slot| match slot {
                3 | 5 => in_front(slot, 1).unwrap_or(true),
                7 | 9 => in_front(slot, 0).unwrap_or(false),
                _ => true,
            })
            .collect();
        let drawn_at = |slot: usize| {
            bone_of_slot(slot)
                .map(|b| mean[b])
                .filter(|m| m.1 > 0)
                .map_or(f64::INFINITY, |m| m.0 / m.1 as f64)
        };
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); SPLIT_SLOTS];
        let mut roots: Vec<usize> = Vec::new();
        for slot in (0..SPLIT_SLOTS).filter(|s| *s != 2) {
            match stack_parent(slot).filter(|p| *p < SPLIT_SLOTS) {
                Some(p) => kids[p].push(slot),
                None => roots.push(slot),
            }
        }
        for list in kids.iter_mut().chain(std::iter::once(&mut roots)) {
            list.sort_by(|a, b| drawn_at(*a).total_cmp(&drawn_at(*b)).then(a.cmp(b)));
        }
        fn stack_slots(slot: usize, kids: &[Vec<usize>], front: &[bool], order: &mut Vec<usize>) {
            for &k in kids[slot].iter().filter(|k| !front[**k]) {
                stack_slots(k, kids, front, order);
            }
            order.push(slot);
            for &k in kids[slot].iter().filter(|k| front[**k]) {
                stack_slots(k, kids, front, order);
            }
        }
        let mut order: Vec<usize> = Vec::new();
        for &root in &roots {
            stack_slots(root, &kids, &front, &mut order);
        }
        order.push(2);
        let mut bone_order: Vec<usize> = order.iter().filter_map(|s| bone_of_slot(*s)).collect();
        let rest: Vec<usize> = (0..bones_n).filter(|b| !bone_order.contains(b)).collect();
        bone_order.extend(rest);
        // An overlap copy on a slot layer above a neighbouring colour's layer
        // would fade that colour's anti-aliased edge at rest: it keeps a cell
        // away from everything on lower slot layers, except its own paint and
        // paints copied onto the same layer (copies of neighbouring colours
        // still meet, so a bend shows no hairline).
        let mut rank = vec![usize::MAX; bones_n];
        for (r, b) in bone_order.iter().enumerate() {
            rank[*b] = r;
        }
        // R4c: a copy must stay under the part that turns away from it, never
        // on top of other artwork once the joint turns. The parent's copy of
        // the child's artwork is kept only when the child stacks in front of
        // the parent (the child covers it); the child's copy of the parent's
        // only when the child stacks behind (the parent covers it). So the
        // body keeps no copy of a back arm's shoulder over the turned sleeve.
        let keep: Vec<bool> = under_joint
            .iter()
            .map(|&(child, parent_copy)| {
                let Some(parent) = partition.joint_parent(child) else { return false };
                if parent_copy { rank[child] > rank[parent] } else { rank[child] < rank[parent] }
            })
            .collect();
        for (&(child, _), kept) in under_joint.iter().zip(&keep) {
            if *kept && overlaps.iter().all(|(s, _, _)| *s != partition.bones[child].0) {
                overlaps.push((partition.bones[child].0, joint_of(child), radius[child]));
            }
        }
        let mut copied: Vec<std::collections::HashSet<(usize, usize)>> = vec![Default::default(); bones_n];
        for ((item, (_, _, key)), kept) in under.iter().zip(&under_meta).zip(&keep) {
            if let SplitItem::Part(bone, _) = item
                && *kept
            {
                copied[*bone].insert(*key);
            }
        }
        let mut copies: Vec<Vec<Object>> = vec![Vec::new(); bones_n];
        for ((item, (path, back, key)), kept) in under.into_iter().zip(under_meta).zip(keep) {
            let SplitItem::Part(bone, mut object) = item else { continue };
            if !kept {
                continue;
            }
            let reach = path.bounding_box().inflate(seam * 2.0, seam * 2.0);
            let lower: Vec<buzz_geom::BezPath> = meet
                .iter()
                .filter(|(_, b, region, _, k)| {
                    rank[*b] < rank[bone]
                        && *k != key
                        && !copied[bone].contains(k)
                        && !region.bounding_box().intersect(reach).is_zero_area()
                })
                .map(|(_, _, region, _, _)| region.clone())
                .collect();
            if !lower.is_empty() {
                let near = buzz_geom::expand_fill(&buzz_geom::union_all(&lower, nonzero), seam, nonzero);
                let kept = buzz_geom::boolean(&path, &near, buzz_geom::BoolOp::Difference, nonzero);
                if kept.elements().is_empty() || kept.area().abs() <= 1e-9 {
                    continue;
                }
                if let ObjectKind::Shape(shape) = &mut object.kind {
                    shape.path = back * kept;
                }
            }
            copies[bone].push(object);
        }
        // R4c: a loose paper mark now on a slot layer above its neighbours
        // keeps a cell away from paints on lower slot layers, as a copy does:
        // its anti-aliased edge would otherwise lighten theirs, and a strip of
        // paper colour less does not show.
        for (bone, pos, region, back, index) in loose_at {
            let reach = region.bounding_box().inflate(seam * 2.0, seam * 2.0);
            let lower: Vec<buzz_geom::BezPath> = meet
                .iter()
                .filter(|(i, b, r, _, _)| {
                    *i != index && rank[*b] < rank[bone] && !r.bounding_box().intersect(reach).is_zero_area()
                })
                .map(|(_, _, r, _, _)| r.clone())
                .collect();
            if lower.is_empty() {
                continue;
            }
            let near = buzz_geom::expand_fill(&buzz_geom::union_all(&lower, nonzero), seam, nonzero);
            let kept = buzz_geom::boolean(&region, &near, buzz_geom::BoolOp::Difference, nonzero);
            if let Some(ObjectKind::Shape(shape)) = by_bone[bone].get_mut(pos).map(|o| &mut o.kind)
                && !kept.elements().is_empty()
            {
                shape.path = back * kept;
                shape.fill.as_mut().map(|f| f.rule = buzz_geom::FillMode::NonZero);
            }
        }

        // R4d: what the drawing paints near a joint. Thin paints (as
        // `BonePartition::refined` reads them) are its lines; the colour with
        // the most line area near a point is the outline's there, and its
        // width twice a line's area over its outline.
        let perimeter = |region: &buzz_geom::BezPath| -> f64 {
            path_edges(region, seam * 0.25).iter().map(|(a, b)| (*b - *a).hypot()).sum()
        };
        let thin: Vec<bool> = meet
            .iter()
            .map(|(_, _, region, _, _)| {
                let p = perimeter(region);
                p <= 1e-9 || 2.0 * region.area().abs() / p < LINE_CELLS * seam
            })
            .collect();
        // Per colour, the area of `meet`'s paints accepted by `take` inside
        // `area`, and that area times the paint's width.
        let paints_in = |area: &buzz_geom::BezPath, take: &dyn Fn(usize, usize) -> bool| {
            let abox = area.bounding_box();
            let mut colours: Vec<(buzz_scene::FillSpec, f64, f64)> = Vec::new();
            for (i, (_, b, region, rule, _)) in meet.iter().enumerate() {
                if !take(i, *b) || is_paper(&meet_spec[i]) || region.bounding_box().intersect(abox).is_zero_area() {
                    continue;
                }
                let a = buzz_geom::boolean(region, area, buzz_geom::BoolOp::Intersect, partition.opts(*rule))
                    .area()
                    .abs();
                if a <= 1e-9 {
                    continue;
                }
                let p = perimeter(region);
                let width = if p > 1e-9 { 2.0 * region.area().abs() / p } else { 0.0 };
                match colours.iter_mut().find(|(s, _, _)| s.paint == meet_spec[i].paint) {
                    Some(c) => {
                        c.1 += a;
                        c.2 += a * width;
                    }
                    None => colours.push((meet_spec[i].clone(), a, a * width)),
                }
            }
            colours
        };
        // A paint's luminance (0–255; anything but a solid colour counts as
        // light).
        let luma = |spec: &buzz_scene::FillSpec| match &spec.paint {
            buzz_scene::Paint::Solid(c) => {
                let c = c.to_rgba8();
                0.299 * c.r as f64 + 0.587 * c.g as f64 + 0.114 * c.b as f64
            }
            _ => 255.0,
        };
        // The outline's paint and width in `area`: of the line colours there
        // holding a fifth of the line area or more, the darkest (folds and
        // texture marks are lighter than the ink).
        let ink_in = |area: &buzz_geom::BezPath| {
            let colours = paints_in(area, &|i, _| thin[i]);
            let total: f64 = colours.iter().map(|c| c.1).sum();
            colours
                .into_iter()
                .filter(|c| c.1 >= 0.2 * total)
                .min_by(|x, y| luma(&x.0).total_cmp(&luma(&y.0)))
                .map(|(spec, a, aw)| (spec, aw / a))
        };
        // The same over the whole drawing on the layer, for a joint with no
        // line paint near it (merged ink runs thick there).
        let ink_all = {
            let mut colours: Vec<(buzz_scene::FillSpec, f64, f64)> = Vec::new();
            for (i, (_, _, region, _, _)) in meet.iter().enumerate() {
                if !thin[i] || is_paper(&meet_spec[i]) {
                    continue;
                }
                let (a, p) = (region.area().abs(), perimeter(region));
                if a <= 1e-9 || p <= 1e-9 {
                    continue;
                }
                match colours.iter_mut().find(|(s, _, _)| s.paint == meet_spec[i].paint) {
                    Some(c) => {
                        c.1 += a;
                        c.2 += 2.0 * a * a / p;
                    }
                    None => colours.push((meet_spec[i].clone(), a, 2.0 * a * a / p)),
                }
            }
            let total: f64 = colours.iter().map(|c| c.1).sum();
            colours
                .into_iter()
                .filter(|c| c.1 >= 0.2 * total)
                .min_by(|x, y| luma(&x.0).total_cmp(&luma(&y.0)))
                .map(|(spec, a, aw)| (spec, aw / a))
        };
        // `bone`'s main fill in `area`: not a line, and clearly lighter than
        // `ink` (merged ink runs thick in places and is no fill).
        let fill_in = |bone: usize, area: &buzz_geom::BezPath, ink: Option<&buzz_scene::FillSpec>| {
            let floor = ink.map_or(f64::NEG_INFINITY, |s| luma(s) + INK_MARGIN);
            paints_in(area, &|i, b| b == bone && !thin[i] && luma(&meet_spec[i]) > floor)
                .into_iter()
                .max_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(spec, _, _)| spec)
        };
        // `bone`'s own artwork (not copies) reaching into `area`.
        let art_of = |want: &dyn Fn(usize) -> bool, area: buzz_geom::Rect| -> Vec<buzz_geom::BezPath> {
            meet.iter()
                .filter(|(_, b, region, _, _)| want(*b) && !region.bounding_box().intersect(area).is_zero_area())
                .map(|(_, _, region, _, _)| region.clone())
                .collect()
        };
        // A new shape on the layer from a path in the figure's space.
        let to_layer = invert(inherited).unwrap_or(Affine::IDENTITY);
        let new_shape = |path: buzz_geom::BezPath, spec: &buzz_scene::FillSpec| {
            Object::shape(
                ObjectId(0),
                buzz_scene::ShapeData {
                    path: to_layer * path,
                    fill: Some(buzz_scene::FillSpec { rule: buzz_geom::FillMode::NonZero, ..spec.clone() }),
                    stroke: None,
                    blend: buzz_scene::PaintBlend::Normal,
                },
            )
        };

        // R4d: `path` with gaps and holes narrower than twice `cells` grid
        // cells closed.
        let close = |path: &buzz_geom::BezPath, cells: f64| {
            let k = cells * seam;
            buzz_geom::expand_fill(&buzz_geom::expand_fill(path, k, nonzero), -k, nonzero)
        };

        // R4d (c): a rounded joint cap on the piece stacked behind at each
        // joint — a disc of its own fill, the joint's overlap radius (half the
        // limb's width), cut to the front piece's silhouette a cell inside so
        // the front piece hides it at rest, inked in the outline's colour and
        // width along its outer edge (the arc, and the cut where the front's
        // silhouette ends it; not where it meets the back piece's artwork).
        // Painted over the back piece's overlap copies and under its
        // artwork: when the joint bends, the back piece's edge reads as one
        // inked line round the joint.
        let mut caps: Vec<Vec<Object>> = vec![Vec::new(); bones_n];
        for c in 0..bones_n {
            let Some(p) = partition.joint_parent(c) else { continue };
            if by_bone[c].is_empty() || by_bone[p].is_empty() {
                continue;
            }
            let (back, front) = if rank[c] < rank[p] { (c, p) } else { (p, c) };
            let (centre, r) = (joint_of(c), radius[c]);
            let front_art = art_of(&|b| b == front, discs[c].bounding_box());
            if front_art.is_empty() {
                continue;
            }
            let back_art = buzz_geom::union_all(&art_of(&|b| b == back, discs[c].bounding_box()), nonzero);
            // The front's silhouette with its small holes and notches closed
            // (where only the back piece paints at rest, a merged drawing's
            // ragged seam), kept a cell inside what the two pieces paint, so
            // nothing of the cap shows at rest.
            let front_u = buzz_geom::union_all(&front_art, nonzero);
            let both = buzz_geom::boolean(&front_u, &back_art, buzz_geom::BoolOp::Union, nonzero);
            let clip = buzz_geom::boolean(
                &close(&front_u, CLOSE_CELLS),
                &buzz_geom::expand_fill(&both, -seam, nonzero),
                buzz_geom::BoolOp::Intersect,
                nonzero,
            );
            let cap = buzz_geom::boolean(&discs[c], &clip, buzz_geom::BoolOp::Intersect, nonzero);
            if cap.elements().is_empty() || cap.area().abs() <= seam * seam {
                continue;
            }
            let ink = ink_in(&kurbo::Circle::new(centre, 2.0 * r).to_path(0.05)).or_else(|| ink_all.clone());
            let Some(fill) = fill_in(back, &discs[c], ink.as_ref().map(|i| &i.0)) else { continue };
            caps[back].push(new_shape(cap.clone(), &fill));
            if let Some((ink, width)) = ink {
                let w = width.clamp(seam, 0.5 * r);
                // The outline runs along the cap's outer edge: the disc's arc
                // and, where the front piece's silhouette cut it, that cut;
                // not where it meets the back piece's own artwork, nor round
                // a hole.
                let inside = outer_contours(&buzz_geom::boolean(
                    &cap,
                    &buzz_geom::expand_fill(&back_art, 2.0 * seam, nonzero),
                    buzz_geom::BoolOp::Union,
                    nonzero,
                ));
                let ring = buzz_geom::boolean(
                    &cap,
                    &buzz_geom::expand_fill(&inside, -w, nonzero),
                    buzz_geom::BoolOp::Difference,
                    nonzero,
                );
                if !ring.elements().is_empty() && ring.area().abs() > seam * seam {
                    caps[back].push(new_shape(ring, &ink));
                }
            }
        }

        // R4d (a): a limb stacked behind its joint parent leaves the parent's
        // side open when it turns (the corner a sleeve covered under the
        // armpit). The parent gets hidden-at-rest art there: the limb's area
        // near the joint (within `UNDER_REACH` × the overlap radius) that
        // nothing else paints and that lies beside the parent (within
        // `UNDER_NEAR` × the radius of its artwork), a cell inside the limb's
        // outline, in the parent's fill, inked along its edges that meet no
        // other artwork. It goes on an underlay part of the parent's slot
        // right under the lowest such limb (`bodyUnder`), following the
        // parent; the limb covers it at rest. Only where there is such area.
        let mut underlay: Vec<Vec<Object>> = vec![Vec::new(); bones_n];
        let mut under_child: Vec<Option<usize>> = vec![None; bones_n];
        for c in 0..bones_n {
            let Some(p) = partition.joint_parent(c) else { continue };
            if rank[c] >= rank[p] || by_bone[c].is_empty() || by_bone[p].is_empty() {
                continue;
            }
            let sub = partition.subtree(c);
            let (centre, r) = (joint_of(c), radius[c]);
            let reach = kurbo::Circle::new(centre, UNDER_REACH * r).to_path(0.05);
            let rbox = reach.bounding_box();
            let limb = art_of(&|b| sub[b], rbox);
            let still = art_of(&|b| !sub[b], rbox);
            let own = art_of(&|b| b == p, rbox);
            if limb.is_empty() || own.is_empty() {
                continue;
            }
            // Closed by two cells first: the limb's colours meet edge to edge,
            // and a hairline between them must not open into a hole.
            let limb = buzz_geom::expand_fill(
                &buzz_geom::expand_fill(&buzz_geom::union_all(&limb, nonzero), 2.0 * seam, nonzero),
                -3.0 * seam,
                nonzero,
            );
            let still = buzz_geom::union_all(&still, nonzero);
            let beside = buzz_geom::expand_fill(&buzz_geom::union_all(&own, nonzero), UNDER_NEAR * r, nonzero);
            let mut open = buzz_geom::boolean(&limb, &reach, buzz_geom::BoolOp::Intersect, nonzero);
            open = buzz_geom::boolean(
                &open,
                &buzz_geom::expand_fill(&still, seam, nonzero),
                buzz_geom::BoolOp::Difference,
                nonzero,
            );
            open = buzz_geom::boolean(&open, &beside, buzz_geom::BoolOp::Intersect, nonzero);
            if open.elements().is_empty() || open.area().abs() < UNDER_MIN_CELLS * seam * seam {
                continue;
            }
            // The parent's fill where it meets this area (the trousers at a
            // hip, not the shirt above), else its main fill near the joint.
            let ink = ink_in(&reach).or_else(|| ink_all.clone());
            let floor = ink.as_ref().map(|i| &i.0);
            let Some(fill) = fill_in(p, &buzz_geom::expand_fill(&open, 3.0 * seam, nonzero), floor)
                .or_else(|| fill_in(p, &reach, floor))
            else {
                continue;
            };
            underlay[p].push(new_shape(open.clone(), &fill));
            if let Some((ink, width)) = ink {
                let w = width.clamp(seam, 0.5 * r);
                // Its edges along other artwork, and round holes, are inside
                // this; the rest are the outline it continues.
                let closed = outer_contours(&buzz_geom::boolean(
                    &open,
                    &buzz_geom::expand_fill(&still, 2.0 * seam, nonzero),
                    buzz_geom::BoolOp::Union,
                    nonzero,
                ));
                let edge = buzz_geom::boolean(
                    &open,
                    &buzz_geom::expand_fill(&closed, -w, nonzero),
                    buzz_geom::BoolOp::Difference,
                    nonzero,
                );
                if !edge.elements().is_empty() && edge.area().abs() > seam * seam {
                    underlay[p].push(new_shape(edge, &ink));
                }
            }
            if under_child[p].is_none_or(|x| rank[c] < rank[x]) {
                under_child[p] = Some(c);
            }
        }

        let mut slot_parts: Vec<Arc<Object>> = Vec::new();
        for bone in bone_order {
            // R4d: an underlay goes right under the lowest limb it is for.
            for p in (0..bones_n).filter(|p| under_child[*p] == Some(bone)) {
                let mut run = std::mem::take(&mut underlay[p]);
                if run.is_empty() {
                    continue;
                }
                let id = work.next_object_id();
                new_ids.push(id);
                let object = if run.len() == 1 {
                    let mut o = run.remove(0);
                    o.id = id;
                    o
                } else {
                    let kids: Vec<Arc<Object>> = run
                        .drain(..)
                        .map(|mut o| {
                            o.id = work.next_object_id();
                            new_ids.push(o.id);
                            Arc::new(o)
                        })
                        .collect();
                    Object::group(id, kids)
                };
                parts_out.push((id, partition.bones[p].0));
                underlays.push(id);
                slot_parts.push(Arc::new(object));
            }
            let mut run = std::mem::take(&mut copies[bone]);
            run.append(&mut caps[bone]);
            run.append(&mut by_bone[bone]);
            if run.is_empty() {
                continue;
            }
            let id = work.next_object_id();
            new_ids.push(id);
            let object = if run.len() == 1 {
                let mut o = run.remove(0);
                o.id = id;
                o
            } else {
                let kids: Vec<Arc<Object>> = run
                    .drain(..)
                    .map(|mut o| {
                        o.id = work.next_object_id();
                        new_ids.push(o.id);
                        Arc::new(o)
                    })
                    .collect();
                Object::group(id, kids)
            };
            parts_out.push((id, partition.bones[bone].0));
            slot_parts.push(Arc::new(object));
        }
        let at = parts_at.unwrap_or(out.len()).min(out.len());
        out.splice(at..at, slot_parts);

        // 5. On the keyframe showing `frame`, and any showing the same.
        let mut edited = (**layer).clone();
        for key in edited.frames.keyframes_mut() {
            if *key.objects == shown {
                key.objects = Arc::new(out.clone());
            } else if key.objects.iter().any(|o| target_ids.contains(&(layer_id, o.id))) {
                unsplit_keys += 1;
            }
        }
        originals.push(layer.clone());
        layers_out.push(edited);
    }
    if parts_out.is_empty() {
        return None;
    }
    let stack = work.library_mut().layers_mut(symbol)?;
    let mut layers = Vec::new();
    for layer in layers_out {
        let id = layer.id;
        let copy = layer.clone();
        stack.update(id, move |l| *l = copy);
        layers.push(Arc::new(layer));
    }
    Some((
        work,
        MergedSplit {
            originals,
            layers,
            new_ids,
            merged,
            parts: parts_out,
            overlaps,
            unsplit_keys,
            underlays,
            pivots: Vec::new(),
            extras: Vec::new(),
        },
    ))
}

/// `object` with its id, and its children's, mapped by `ids`.
fn remap_object_ids(object: &Arc<Object>, ids: &std::collections::HashMap<ObjectId, ObjectId>) -> Arc<Object> {
    let mut o = (**object).clone();
    if let Some(id) = ids.get(&o.id) {
        o.id = *id;
    }
    if let ObjectKind::Group(children) = &o.kind {
        o.kind = ObjectKind::Group(children.iter().map(|c| remap_object_ids(c, ids)).collect());
    }
    Arc::new(o)
}

/// What [`apply_layer_rig_plan`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedLayerRig {
    pub rig: LayerRig,
    /// Layers in the symbol before and after.
    pub layers_before: usize,
    pub layers_after: usize,
    /// Keyframes rebaked so nothing moved.
    pub keys_rebaked: usize,
    /// Nested symbols renamed.
    pub renamed: usize,
    /// The profile's legs folder was made (the leg layers were together
    /// below the body, not in a folder yet).
    pub legs_folder: bool,
    /// Face layers renamed inside the head symbol.
    pub face_renamed: usize,
}

/// **R2.** Carry out `plan` inside its character symbol: each piece on a
/// layer of its own named as planned — a copy of the layer it was on (its
/// settings, folder and every keyframe, cut down to the piece by object id)
/// standing in front of it, so paint order is exactly as drawn — the layers
/// left empty removed, the rig linked by [`place_layer_rig`] (front-most
/// piece of a slot leads, the slot's other pieces follow it, each lead
/// follows the nearest filled ancestor slot's lead, pivots on the joints),
/// every keyframe rebaked by Repair Layer Rigs' rebake so no keyframe moves,
/// and the nested symbols renamed. The caller wraps it in one
/// `Document::edit`. `None`, with nothing changed, when the plan no longer
/// matches the symbol.
pub fn apply_layer_rig_plan(scene: &mut Scene, plan: &LayerRigPlan) -> Option<AppliedLayerRig> {
    let Some(split) = &plan.split else {
        return apply_layer_rig_pieces(scene, plan);
    };
    // R4: the merged drawings' layers must be as planned; their parts go in
    // first, with fresh ids, then the pieces are rigged as any others.
    let stack = &scene.library().get(plan.symbol)?.layers;
    if split.originals.iter().any(|o| stack.get(o.id).is_none_or(|l| **l != **o)) {
        return None;
    }
    let mut work = scene.clone();
    let ids: std::collections::HashMap<ObjectId, ObjectId> =
        split.new_ids.iter().map(|id| (*id, work.next_object_id())).collect();
    let stack = work.library_mut().layers_mut(plan.symbol)?;
    for layer in &split.layers {
        let mut layer = (**layer).clone();
        for key in layer.frames.keyframes_mut() {
            key.objects = Arc::new(key.objects.iter().map(|o| remap_object_ids(o, &ids)).collect());
        }
        let id = layer.id;
        stack.update(id, move |l| *l = layer);
    }
    let mut plan = plan.clone();
    plan.split = None;
    for piece in &mut plan.pieces {
        if let Some(id) = ids.get(&piece.object) {
            piece.object = *id;
        }
    }
    let applied = apply_layer_rig_pieces(&mut work, &plan)?;
    *scene = work;
    Some(applied)
}

/// [`apply_layer_rig_plan`] on pieces that are all in the symbol.
fn apply_layer_rig_pieces(scene: &mut Scene, plan: &LayerRigPlan) -> Option<AppliedLayerRig> {
    use std::collections::HashSet;
    let pattern = RigPattern::biped_full();
    let before = scene.library().get(plan.symbol)?.layers.clone();
    if plan.pieces.is_empty() {
        return None;
    }
    let stands_at = |piece: &PlannedPiece| -> Option<Object> {
        let layer = before.get(piece.layer)?;
        let object = layer.objects_at(plan.frame).iter().find(|o| o.id == piece.object)?;
        let mut artwork = (**object).clone();
        artwork.transform = before.inherited_transform(piece.layer, plan.frame) * artwork.transform;
        Some(artwork)
    };
    let piece_ids: HashSet<ObjectId> = plan.pieces.iter().map(|p| p.object).collect();
    let mut parts = Vec::with_capacity(plan.pieces.len());
    for piece in &plan.pieces {
        if piece.slot >= pattern.slots.len() {
            return None;
        }
        let artwork = stands_at(piece)?;
        let mut copy = (**before.get(piece.layer)?).clone();
        copy.follows = None;
        copy.follows_bone = None;
        copy.rest_pose = None;
        // Every keyframe standing where it is drawn, the old parenting baked
        // in, so the unlinked copy shows exactly what the layer showed.
        for key in copy.frames.keyframes_mut() {
            let inherited = before.inherited_transform(piece.layer, key.start);
            let kept: Vec<Arc<Object>> = key
                .objects
                .iter()
                .filter(|o| o.id == piece.object)
                .map(|o| {
                    let mut o = (**o).clone();
                    o.transform = inherited * o.transform;
                    Arc::new(o)
                })
                .collect();
            key.objects = Arc::new(kept);
        }
        let joint = plan
            .joints
            .get(piece.slot)
            .copied()
            .flatten()
            .unwrap_or_else(|| scene.resolved_bounds(&artwork).center());
        parts.push(LayerPart {
            slot: piece.slot,
            name: piece.new_layer_name.clone(),
            artwork,
            joint,
            from: Some(copy),
            lead: piece.lead,
        });
    }

    // On a copy, so a plan that fails half way leaves nothing behind.
    let mut work = scene.clone();
    // The profile's parent table, not the biped pattern's: with "My Animate
    // convention" the body, the hips and the thighs are roots.
    let parents = plan.profile.parents(&pattern);
    let home = plan.pieces[0].layer;
    let rig = place_layer_rig(
        &mut work,
        RigHome::Symbol(plan.symbol),
        plan.frame,
        home,
        parts,
        &parents,
        plan.profile.join_roots,
    )?;

    let stack = work.library_mut().layers_mut(plan.symbol)?;
    // The pieces leave the layers they were on; a layer left with nothing is
    // removed, one still holding other artwork keeps it where it stood.
    let mut originals: Vec<LayerId> = Vec::new();
    for piece in &plan.pieces {
        if !originals.contains(&piece.layer) {
            originals.push(piece.layer);
        }
    }
    let mut removed: Vec<LayerId> = Vec::new();
    for &id in &originals {
        let mut left = false;
        stack.update(id, |l| {
            for key in l.frames.keyframes_mut() {
                let inherited = before.inherited_transform(id, key.start);
                let kept: Vec<Arc<Object>> = key
                    .objects
                    .iter()
                    .filter(|o| !piece_ids.contains(&o.id))
                    .map(|o| {
                        let mut o = (**o).clone();
                        o.transform = inherited * o.transform;
                        Arc::new(o)
                    })
                    .collect();
                key.objects = Arc::new(kept);
            }
            left = l.all_objects().next().is_some();
            l.follows = None;
            l.follows_bone = None;
            l.rest_pose = None;
        });
        if !left {
            stack.remove(id);
            removed.push(id);
        }
    }
    // Anything else that followed a removed layer is released, as
    // `Scene::remove_layer` does.
    let orphans: Vec<LayerId> = stack
        .iter()
        .filter(|l| l.follows.is_some_and(|f| removed.contains(&f) || originals.contains(&f)))
        .map(|l| l.id)
        .collect();
    for id in orphans {
        stack.update(id, |l| l.follows = None);
    }

    // The links `place_layer_rig` made, parents first; cut, then made again
    // by Repair's rebake, which keeps every keyframe where it stands.
    let follows: Vec<(LayerId, Option<LayerId>)> = rig
        .layers
        .iter()
        .map(|(_, id)| (*id, stack.get(*id).and_then(|l| l.follows)))
        .collect();
    let mut order: Vec<LayerId> = Vec::new();
    while order.len() < follows.len() {
        let placed = order.len();
        for (id, target) in &follows {
            if !order.contains(id) && target.is_none_or(|t| order.contains(&t)) {
                order.push(*id);
            }
        }
        if order.len() == placed {
            return None;
        }
    }
    for (id, _) in &follows {
        stack.update(*id, |l| {
            l.follows = None;
            l.rest_pose = None;
        });
    }
    let (_, keys_rebaked) = apply_rig_repair(stack, &RigRepair { follows, order });

    let legs_folder = match &plan.profile.legs_folder {
        Some(name) => {
            let id = LayerId(work.next_object_id().0);
            let stack = work.library_mut().layers_mut(plan.symbol)?;
            make_legs_folder(stack, &rig, id, name)
        }
        None => false,
    };
    let layers_after = work.library().get(plan.symbol).map_or(0, |s| s.layers.len());

    // The head symbol's face layers (no parenting inside it).
    let mut face_renamed = 0;
    if let Some(head) = plan.head_symbol
        && let Some(stack) = work.library_mut().layers_mut(head)
    {
        for rename in &plan.head_renames {
            if stack.get(rename.layer).is_some_and(|l| l.name == rename.from) {
                stack.update(rename.layer, |l| l.name = rename.to.clone());
                face_renamed += 1;
            }
        }
    }

    let mut renamed = 0;
    for rename in &plan.symbol_renames {
        if work.library().get(rename.symbol).is_some_and(|s| s.name == rename.from)
            && work.library().find_by_name(&rename.to).is_none()
        {
            work.library_mut().update(rename.symbol, |s| s.name = rename.to.clone());
            renamed += 1;
        }
    }

    *scene = work;
    Some(AppliedLayerRig {
        rig,
        layers_before: before.len(),
        layers_after,
        keys_rebaked,
        renamed,
        legs_folder,
        face_renamed,
    })
}

/// The `biped_full` slots that are legs: hips (lower cloth), thighs, shins,
/// feet.
const LEG_SLOTS: [usize; 7] = [0, 7, 8, 9, 10, 13, 14];

/// Put a just-placed rig's leg layers (and a shadow right below them) in a
/// new folder `name` — only when they are all top-level, together, and below
/// the body's layers, so paint order is unchanged. `false`, with nothing
/// changed, otherwise (already in a folder, or apart).
fn make_legs_folder(stack: &mut buzz_scene::LayerStack, rig: &LayerRig, id: LayerId, name: &str) -> bool {
    let mut legs: Vec<LayerId> = rig.layers.iter().filter(|(s, _)| LEG_SLOTS.contains(s)).map(|(_, l)| *l).collect();
    if legs.is_empty() {
        return false;
    }
    let top_level = |l: LayerId| stack.get(l).is_some_and(|l| l.parent.is_none());
    if !legs.iter().all(|l| top_level(*l)) {
        return false;
    }
    let mut rows: Vec<usize> = legs.iter().filter_map(|l| stack.index_of(*l)).collect();
    if rows.len() != legs.len() {
        return false;
    }
    rows.sort_unstable();
    // A shadow right below the legs goes in with them.
    let mut below = rows[rows.len() - 1] + 1;
    while let Some(layer) = stack.iter().nth(below) {
        if layer.parent.is_none() && layer.kind == LayerKind::Normal && layer.name.to_lowercase().contains("shadow") {
            rows.push(below);
            legs.push(layer.id);
            below += 1;
        } else {
            break;
        }
    }
    let together = rows.windows(2).all(|w| w[1] == w[0] + 1);
    let body_above = rig
        .layers
        .iter()
        .filter(|(s, _)| *s == 1)
        .filter_map(|(_, l)| stack.index_of(*l))
        .all(|b| b < rows[0]);
    if !together || !body_above {
        return false;
    }
    let span = legs.iter().filter_map(|l| stack.get(*l)).map(|l| l.length()).max().unwrap_or(1);
    let mut folder = buzz_scene::Layer::new(id, name, LayerKind::Folder);
    if span > 1 {
        folder.frames.insert_frame(span - 1);
    }
    stack.insert(rows[0], folder);
    for l in legs {
        stack.update(l, |layer| layer.parent = Some(id));
    }
    true
}

// ---------------------------------------------------------------------------
// Naming profiles and Rename Body / Face Layers
// (docs/AI_LAYER_RIG_AND_MOTION_COPY_PLAN.md N1, N2).
// ---------------------------------------------------------------------------

/// The naming profiles: the built-in ones ("My Animate convention" first,
/// then "Descriptive") with `rig_naming.json` beside the workspace file laid
/// over them — one profile object or a list; a profile named like a built-in
/// replaces it, and the file's first profile becomes the default.
pub fn naming_profiles() -> Vec<buzz_rig::pattern::NamingProfile> {
    use buzz_rig::pattern::NamingProfile;
    let path = buzz_ui::workspace::workspace_path().with_file_name("rig_naming.json");
    let overrides = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| {
            serde_json::from_str::<Vec<NamingProfile>>(&text)
                .ok()
                .or_else(|| serde_json::from_str::<NamingProfile>(&text).ok().map(|p| vec![p]))
        })
        .unwrap_or_default();
    NamingProfile::with_overrides(overrides)
}

/// The naming profile called `name` (case-insensitive), or the default with
/// `None`.
pub fn naming_profile(name: Option<&str>) -> Result<buzz_rig::pattern::NamingProfile, String> {
    let all = naming_profiles();
    match name {
        None => Ok(all.into_iter().next().unwrap_or_else(buzz_rig::pattern::NamingProfile::animate_convention)),
        Some(wanted) => all.iter().find(|p| p.name.eq_ignore_ascii_case(wanted.trim())).cloned().ok_or_else(|| {
            let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
            format!("no naming profile '{}'; profiles: {}", wanted.trim(), names.join(", "))
        }),
    }
}

fn stack_in(scene: &Scene, target: RigHome) -> Option<&buzz_scene::LayerStack> {
    match target {
        RigHome::Active => Some(scene.layers()),
        RigHome::Symbol(id) => scene.library().get(id).map(|s| &s.layers),
    }
}

/// Make `renames` on `target`'s layers (each only while the layer still has
/// its old name); how many were made. Nothing to do touches nothing.
fn apply_layer_renames(scene: &mut Scene, target: RigHome, renames: &[LayerRename]) -> usize {
    if renames.is_empty() {
        return 0;
    }
    let stack = match target {
        RigHome::Active => scene.edit_layers(),
        RigHome::Symbol(id) => match scene.library_mut().layers_mut(id) {
            Some(stack) => stack,
            None => return 0,
        },
    };
    let mut made = 0;
    for rename in renames {
        if stack.get(rename.layer).is_some_and(|l| l.name == rename.from) {
            stack.update(rename.layer, |l| l.name = rename.to.clone());
            made += 1;
        }
    }
    made
}

/// What **Rename Body Layers** would do on `stack`: every layer (folders
/// aside) whose name says a body part ([`buzz_rig::pattern::canonical_slot`])
/// gets `profile`'s name for it, its own side kept; a shadow gets the
/// profile's shadow name. "Layer…" and "sound" names, names that say no part
/// (gamcha, base1) and names that already are the answer, ignoring case, are
/// left alone — so a second run changes nothing.
pub fn body_layer_renames(
    stack: &buzz_scene::LayerStack,
    profile: &buzz_rig::pattern::NamingProfile,
) -> Vec<LayerRename> {
    stack
        .iter()
        .filter(|l| l.kind != LayerKind::Folder)
        .filter_map(|l| {
            let to = profile.body_layer_name(&l.name)?;
            Some(LayerRename { layer: l.id, from: l.name.clone(), to })
        })
        .collect()
}

/// **Rename Body Layers** (N2; Ctrl+R in the user's Animate) on `target`:
/// the renames made.
pub fn rename_body_layers(
    scene: &mut Scene,
    target: RigHome,
    profile: &buzz_rig::pattern::NamingProfile,
) -> Vec<LayerRename> {
    let renames = stack_in(scene, target).map(|s| body_layer_renames(s, profile)).unwrap_or_default();
    apply_layer_renames(scene, target, &renames);
    renames
}

/// The head symbol on `stack`, as `characterSetup.jsfl` finds it: the first
/// symbol instance (at frame 0) on a layer whose name says head, or whose
/// symbol's name says head or face.
fn head_symbol_on(scene: &Scene, stack: &buzz_scene::LayerStack) -> Option<buzz_scene::SymbolId> {
    stack.iter().filter(|l| l.kind != LayerKind::Folder).find_map(|layer| {
        layer.objects_at(0).iter().find_map(|o| {
            let ObjectKind::Instance(instance) = &o.kind else {
                return None;
            };
            let symbol = scene.library().get(instance.symbol)?;
            let lib = symbol.name.to_lowercase();
            let on_head = buzz_rig::pattern::body_part(&layer.name) == Some("Head");
            (on_head || lib.contains("head") || lib.contains("face")).then_some(instance.symbol)
        })
    })
}

/// **Rename Face Layers** (N2; Shift+R in the user's Animate) on `target`,
/// recursing into the head symbol as `characterSetup.jsfl` does: the Shift+R
/// rules ([`buzz_rig::pattern::face_layer_name`]) on `target`'s layers, then
/// on the head symbol's. Returns the renames made on `target`, and the head
/// symbol with its renames.
pub fn rename_face_layers(
    scene: &mut Scene,
    target: RigHome,
) -> (Vec<LayerRename>, Option<(buzz_scene::SymbolId, Vec<LayerRename>)>) {
    let Some(stack) = stack_in(scene, target) else {
        return (Vec::new(), None);
    };
    let own = face_layer_renames(stack);
    let this = match target {
        RigHome::Symbol(id) => Some(id),
        RigHome::Active => scene.editing_symbol(),
    };
    let head = head_symbol_on(scene, stack)
        .filter(|id| Some(*id) != this)
        .and_then(|id| scene.library().get(id).map(|s| (id, face_layer_renames(&s.layers))));
    apply_layer_renames(scene, target, &own);
    if let Some((id, renames)) = &head {
        apply_layer_renames(scene, RigHome::Symbol(*id), renames);
    }
    (own, head)
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
