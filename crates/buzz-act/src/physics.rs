//! Baking secondary motion onto the timeline.
//!
//! # What this does, and what it does not
//!
//! It reads a rig's *primary* animation frame by frame, hands it to the spring
//! solver in [`buzz_rig::follow_through`], and writes the lagging result back as
//! ordinary keyframes. The physics is not in here — this is the strip of wiring
//! between the solver and the document, exactly as [`crate::perform`] is between
//! a walk cycle and the document. There is no solver and no live state in this
//! crate; the arithmetic lives one layer down in `buzz-rig` and `buzz-physics`.
//!
//! # It writes keyframes, and then it is gone
//!
//! Like a performance, the follow-through it bakes is ordinary editable animation
//! that does not remember it was generated. If the animator changes the primary
//! motion afterwards, they run this again — the honest cost of a baked result,
//! and the reason the solver was kept reusable for a live version later.
//!
//! # It reads the *tweened* motion
//!
//! The spring is driven at every frame, not only at keyframes: `resolved_at`
//! returns the interpolated pose on the in-between frames, so a chain follows the
//! smooth motion the animator sees, not the sparse poses they keyed.

use std::ops::Range;

use buzz_geom::Affine;
use buzz_physics::{Spring, Wiggle};
use buzz_scene::{ObjectId, ObjectKind, Scene};

/// What [`bake`] did, for the status line and the tests.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsReport {
    pub keyframes: u32,
    pub frames: u32,
    /// How many bones the chain drove.
    pub bones: u32,
    pub message: String,
}

/// Why follow-through could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsError {
    /// The object is not on the timeline.
    NoObject,
    /// The object is not rigged, so there is no chain to spring.
    NotRigged,
    /// The chosen root bone is not in the rig.
    NoChain,
    /// The frame range was empty.
    NoFrames,
}

impl std::fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhysicsError::NoObject => write!(f, "select an object to add follow-through to"),
            PhysicsError::NotRigged => write!(
                f,
                "only a rigged character has a chain to spring \u{2014} rig it with the Bone \
                 tool, or use Scene > Add Person"
            ),
            PhysicsError::NoChain => write!(f, "that bone is not part of this rig"),
            PhysicsError::NoFrames => write!(f, "there are no frames in that range to fill"),
        }
    }
}

impl std::error::Error for PhysicsError {}

/// **Bake spring follow-through onto a rig's chain.**
///
/// `root` names the top of the sprung chain (the bone plus everything below it);
/// `spring` its feel; `frames` the span to fill; `step` the frames between keys
/// (two for on-twos, matching a hand-drawn overlap). `coupling` above zero also
/// makes the chain react to the whole object's motion — turning with the body
/// and trailing its acceleration; zero leaves it driven by the pose alone. The
/// spring is integrated at every frame for accuracy and sampled at the step for
/// the keys, with a tween between — a walk does exactly this.
///
/// The caller wraps this in one `Document::edit` so the whole chain is one
/// Ctrl+Z.
pub fn bake(
    scene: &mut Scene,
    object: ObjectId,
    root: usize,
    spring: Spring,
    frames: Range<u32>,
    step: u32,
    coupling: f64,
) -> Result<PhysicsReport, PhysicsError> {
    if frames.start >= frames.end {
        return Err(PhysicsError::NoFrames);
    }

    // Topology and layer come off the stored object; validate it is a rig with
    // the chosen chain before touching the timeline.
    let (layer, found) = scene.find_object(object).ok_or(PhysicsError::NoObject)?;
    let ObjectKind::Armature(rig) = &found.kind else {
        return Err(PhysicsError::NotRigged);
    };
    let topology = rig.armature.clone();
    if root >= topology.bones.len() {
        return Err(PhysicsError::NoChain);
    }
    let bones = topology.subtree(root).len() as u32;
    let fps = scene.stage().frame_rate.max(1.0);

    // Extend the layer to span the range *before* reading it, so frames past the
    // last real keyframe resolve to that held pose rather than to nothing — and
    // so `ensure_keyframe` will accept them on the write pass, as in `perform`.
    let last = frames.end - 1;
    scene.update_layer(layer, |l| {
        if l.frames.length() <= last {
            l.frames.insert_frame(last);
        }
    });

    // Read the primary motion across the whole range, on ones, so the spring is
    // driven by the smooth tweened motion — the pose, and the object's own
    // placement (for coupling). Collected first so the immutable borrow is done
    // before the write pass.
    let (primary, object_world): (Vec<Vec<f64>>, Vec<Affine>) = {
        let Some(layer_ref) = scene.layers().get(layer) else {
            return Err(PhysicsError::NoObject);
        };
        let mut poses = Vec::with_capacity((frames.end - frames.start) as usize);
        let mut transforms = Vec::with_capacity(poses.capacity());
        for frame in frames.start..frames.end {
            let resolved = layer_ref.frames.resolved_at(frame);
            let object_here = resolved.iter().find(|o| o.id == object);
            poses.push(
                object_here
                    .and_then(|o| match &o.kind {
                        ObjectKind::Armature(r) => Some(r.armature.pose()),
                        _ => None,
                    })
                    .unwrap_or_else(|| topology.pose()),
            );
            transforms.push(object_here.map_or(Affine::IDENTITY, |o| o.transform));
        }
        (poses, transforms)
    };

    let modified = if coupling > 0.0 {
        buzz_rig::follow_through_coupled(
            &topology,
            root,
            spring,
            &primary,
            &object_world,
            coupling,
            fps,
        )
    } else {
        buzz_rig::follow_through(&topology, root, spring, &primary, fps)
    };

    let start = frames.start;
    let step = step.max(1);
    let mut written = 0u32;
    let mut frame = start;
    while frame < frames.end {
        let pose = &modified[(frame - start) as usize];
        scene.ensure_keyframe(layer, frame);
        scene.update_object_at(frame, object, |target| {
            if let ObjectKind::Armature(rig) = &mut target.kind {
                rig.armature.set_pose(pose);
            }
        });

        // A tween across the gap so keying on twos stays smooth on ones; the last
        // key has nothing after it to tween to.
        if frame + step < frames.end {
            scene.update_layer(layer, |l| {
                l.frames.set_tween(frame, buzz_scene::Tween::motion());
            });
        }

        written += 1;
        frame += step;
    }

    let frame_count = frames.end - frames.start;
    Ok(PhysicsReport {
        keyframes: written,
        frames: frame_count,
        bones,
        message: format!(
            "Follow-Through: {written} keyframe(s) over {frame_count} frame(s), {bones} bone(s)"
        ),
    })
}

/// **Bake a wiggle onto an object's placement.**
///
/// Adds a deterministic wandering offset to the object's transform each frame —
/// an idle sway, a breeze, a handheld shake — on top of whatever motion it
/// already has. Works on any object, not only a rig. Keyed on the step with a
/// tween between; a fast shake wants keys on ones, a slow breath is happy on
/// twos.
///
/// The seed is the object's id, so two objects wiggling with the same settings
/// still move independently rather than in lockstep.
///
/// The caller wraps this in one `Document::edit`.
pub fn wiggle(
    scene: &mut Scene,
    object: ObjectId,
    wiggle: Wiggle,
    frames: Range<u32>,
    step: u32,
) -> Result<PhysicsReport, PhysicsError> {
    if frames.start >= frames.end {
        return Err(PhysicsError::NoFrames);
    }
    let layer = scene.find_object(object).ok_or(PhysicsError::NoObject)?.0;
    let fps = scene.stage().frame_rate.max(1.0);
    let seed = object.0;

    // Extend the layer to span the range before reading it, so a static object's
    // placement is held across every frame rather than falling off after the one
    // keyframe it was added on.
    let last = frames.end - 1;
    scene.update_layer(layer, |l| {
        if l.frames.length() <= last {
            l.frames.insert_frame(last);
        }
    });

    // The object's own placement across the range, so the wiggle rides on top of
    // any motion it already has rather than replacing it. Collected before the
    // write pass so the borrow is done.
    let base: Vec<Affine> = {
        let Some(layer_ref) = scene.layers().get(layer) else {
            return Err(PhysicsError::NoObject);
        };
        (frames.start..frames.end)
            .map(|frame| {
                layer_ref
                    .frames
                    .resolved_at(frame)
                    .iter()
                    .find(|o| o.id == object)
                    .map_or(Affine::IDENTITY, |o| o.transform)
            })
            .collect()
    };

    let start = frames.start;
    let step = step.max(1);
    let mut written = 0u32;
    let mut frame = start;
    while frame < frames.end {
        let primary = base[(frame - start) as usize];
        let offset = buzz_physics::wiggle_at(wiggle, seed, frame as f64 / fps);
        // Prepended, so the jitter is a shift on the stage regardless of the
        // object's own rotation and scale.
        let jittered = Affine::translate((offset.dx, offset.dy)) * primary;

        scene.ensure_keyframe(layer, frame);
        scene.update_object_at(frame, object, |target| {
            target.transform = jittered;
        });

        if frame + step < frames.end {
            scene.update_layer(layer, |l| {
                l.frames.set_tween(frame, buzz_scene::Tween::motion());
            });
        }

        written += 1;
        frame += step;
    }

    let frame_count = frames.end - frames.start;
    Ok(PhysicsReport {
        keyframes: written,
        frames: frame_count,
        bones: 0,
        message: format!("Wiggle: {written} keyframe(s) over {frame_count} frame(s)"),
    })
}
