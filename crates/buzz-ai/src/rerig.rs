//! Rig or re-rig existing vector characters (Chunk C9).
//!
//! Takes existing vector characters (such as imported FLA symbols or hand-drawn layers),
//! maps existing parts to humanoid rig slots via bone capsule overlap, sets joint pivots,
//! builds the armature using `autorig::assemble` without re-tracing, and classifies existing
//! timeline motion cycles (e.g. "walk 0–39").

use std::sync::Arc;

use image::GrayImage;
use serde::{Deserialize, Serialize};

use buzz_act::autorig;
use buzz_geom::{Point, Rect};
use buzz_rig::pattern::RigPattern;
use buzz_scene::{ArmatureData, Object, Symbol, SymbolId, SymbolKind};

use crate::character::Skeleton;
use crate::motion::{
    classify_motion, KeypointFrame, LocomotionClassification, MotionClassification,
    MotionSequence, PoseLibrary,
};

/// A candidate vector part from an existing character (layer or top-level object).
#[derive(Debug, Clone)]
pub struct VectorPartCandidate {
    pub name: String,
    pub bounds: Rect,
    pub mask: Option<GrayImage>,
    pub object: Arc<Object>,
}

/// A bone capsule used for geometric overlap scoring against parts.
#[derive(Debug, Clone)]
pub struct BoneCapsule {
    pub slot_index: usize,
    pub slot_name: String,
    pub head: Point,
    pub tail: Point,
    pub radius: f64,
}

impl BoneCapsule {
    /// Distance from point P to the line segment [head, tail].
    pub fn distance_to_point(&self, p: Point) -> f64 {
        let seg = self.tail - self.head;
        let len_sq = seg.x * seg.x + seg.y * seg.y;
        if len_sq <= 1e-6 {
            return (p - self.head).hypot();
        }
        let pt = p - self.head;
        let t = ((pt.x * seg.x + pt.y * seg.y) / len_sq).clamp(0.0, 1.0);
        let proj = self.head + seg * t;
        (p - proj).hypot()
    }

    /// Whether point P lies inside this bone capsule.
    pub fn contains_point(&self, p: Point) -> bool {
        self.distance_to_point(p) <= self.radius
    }
}

/// Outcome of mapping an existing vector part to a humanoid rig slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartSlotMapping {
    pub part_name: String,
    pub slot_index: usize,
    pub slot_name: String,
    pub pivot: (f64, f64),
    pub overlap_score: f64,
}

/// Labeled motion range detected in a character's timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineMotionRange {
    pub label: String,
    pub classification: MotionClassification,
    pub start_frame: u32,
    pub end_frame: u32,
}

/// The result of auto-rigging an existing vector character.
pub struct ReriggedCharacter {
    pub armature: ArmatureData,
    pub mappings: Vec<PartSlotMapping>,
    pub character_symbol: Symbol,
    pub motion_ranges: Vec<TimelineMotionRange>,
}

/// Builds bone capsules for the standard 11 biped rig slots from a detected Skeleton.
pub fn build_bone_capsules(skel: &Skeleton) -> Vec<BoneCapsule> {
    let pattern = RigPattern::biped();
    let height = (skel.character_height as f64).max(100.0);
    let default_radius = height * 0.08;

    let get_pt = |name: &str| -> Option<Point> {
        skel.joint(name).map(|j| Point::new(j.x as f64, j.y as f64))
    };

    let mut capsules = Vec::new();

    for (slot_idx, slot) in pattern.slots.iter().enumerate() {
        let (head, tail, rad) = match slot_idx {
            0 => {
                // Hips (root): Hips -> Chest
                let h = get_pt("Hips").unwrap_or(Point::new(0.0, height * 0.5));
                let t = get_pt("Chest").unwrap_or(h + buzz_geom::Vec2::new(0.0, -height * 0.15));
                (h, t, default_radius * 1.2)
            }
            1 => {
                // Chest: Chest -> Neck/Head
                let h = get_pt("Chest").unwrap_or(Point::new(0.0, height * 0.35));
                let t = get_pt("Head").unwrap_or(h + buzz_geom::Vec2::new(0.0, -height * 0.15));
                (h, t, default_radius * 1.1)
            }
            2 => {
                // Head: Neck/Head -> Skull top
                let h = get_pt("Head").unwrap_or(Point::new(0.0, height * 0.2));
                let t = h + buzz_geom::Vec2::new(0.0, -height * 0.18);
                (h, t, default_radius * 1.3)
            }
            3 => {
                // Shoulder L (Upper Arm L): Shoulder L -> Elbow L
                let h = get_pt("Shoulder L").unwrap_or(Point::new(-height * 0.12, height * 0.35));
                let t = get_pt("Elbow L").unwrap_or(h + buzz_geom::Vec2::new(-height * 0.1, height * 0.15));
                (h, t, default_radius * 0.9)
            }
            4 => {
                // Elbow L (Forearm L): Elbow L -> Wrist L
                let h = get_pt("Elbow L").unwrap_or(Point::new(-height * 0.22, height * 0.5));
                let t = skel.bone("Elbow L").map(|b| Point::new(b.tail.0 as f64, b.tail.1 as f64))
                    .unwrap_or(h + buzz_geom::Vec2::new(-height * 0.08, height * 0.15));
                (h, t, default_radius * 0.8)
            }
            5 => {
                // Shoulder R (Upper Arm R): Shoulder R -> Elbow R
                let h = get_pt("Shoulder R").unwrap_or(Point::new(height * 0.12, height * 0.35));
                let t = get_pt("Elbow R").unwrap_or(h + buzz_geom::Vec2::new(height * 0.1, height * 0.15));
                (h, t, default_radius * 0.9)
            }
            6 => {
                // Elbow R (Forearm R): Elbow R -> Wrist R
                let h = get_pt("Elbow R").unwrap_or(Point::new(height * 0.22, height * 0.5));
                let t = skel.bone("Elbow R").map(|b| Point::new(b.tail.0 as f64, b.tail.1 as f64))
                    .unwrap_or(h + buzz_geom::Vec2::new(height * 0.08, height * 0.15));
                (h, t, default_radius * 0.8)
            }
            7 => {
                // Thigh L: Hips/Thigh L -> Knee L
                let h = get_pt("Thigh L").or_else(|| get_pt("Hips")).unwrap_or(Point::new(-height * 0.08, height * 0.55));
                let t = get_pt("Knee L").unwrap_or(h + buzz_geom::Vec2::new(0.0, height * 0.22));
                (h, t, default_radius * 1.0)
            }
            8 => {
                // Knee L (Shin L): Knee L -> Ankle L
                let h = get_pt("Knee L").unwrap_or(Point::new(-height * 0.08, height * 0.77));
                let t = get_pt("Foot L")
                    .or_else(|| skel.bone("Knee L").map(|b| Point::new(b.tail.0 as f64, b.tail.1 as f64)))
                    .unwrap_or(h + buzz_geom::Vec2::new(0.0, height * 0.2));
                (h, t, default_radius * 0.9)
            }
            9 => {
                // Thigh R: Hips/Thigh R -> Knee R
                let h = get_pt("Thigh R").or_else(|| get_pt("Hips")).unwrap_or(Point::new(height * 0.08, height * 0.55));
                let t = get_pt("Knee R").unwrap_or(h + buzz_geom::Vec2::new(0.0, height * 0.22));
                (h, t, default_radius * 1.0)
            }
            10 => {
                // Knee R (Shin R): Knee R -> Ankle R
                let h = get_pt("Knee R").unwrap_or(Point::new(height * 0.08, height * 0.77));
                let t = get_pt("Foot R")
                    .or_else(|| skel.bone("Knee R").map(|b| Point::new(b.tail.0 as f64, b.tail.1 as f64)))
                    .unwrap_or(h + buzz_geom::Vec2::new(0.0, height * 0.2));
                (h, t, default_radius * 0.9)
            }
            _ => (Point::ORIGIN, Point::ORIGIN, default_radius),
        };

        capsules.push(BoneCapsule {
            slot_index: slot_idx,
            slot_name: slot.name.clone(),
            head,
            tail,
            radius: rad,
        });
    }

    capsules
}

/// Part names that belong on the forearm slot: forearm and elbow, and the
/// hand itself (wrist, palm, fist, fingers, thumb), since the biped's Elbow
/// bone carries the hand.
fn is_hand_or_forearm(norm_name: &str) -> bool {
    ["elbow", "forearm", "hand", "wrist", "palm", "fist", "finger", "thumb"]
        .iter()
        .any(|k| norm_name.contains(k))
}

/// Computes the overlap score (0.0 to 1.0+) between a vector part and a bone capsule.
pub fn compute_overlap_score(part: &VectorPartCandidate, capsule: &BoneCapsule) -> f64 {
    let norm_name = part.name.to_lowercase().replace(['_', '-'], " ");
    let slot_norm = capsule.slot_name.to_lowercase();

    // Check for side conflict (e.g. part says 'left' or '_l' while capsule is 'right')
    let is_part_left = norm_name.contains("left") || norm_name.ends_with(" l") || norm_name.contains(" l ") || norm_name.contains("_l");
    // "righ" too: real layer names carry typos (bhatta2's `righFoot`).
    let is_part_right = norm_name.contains("righ") || norm_name.ends_with(" r") || norm_name.contains(" r ") || norm_name.contains("_r");
    let is_capsule_left = slot_norm.contains(" l");
    let is_capsule_right = slot_norm.contains(" r");

    if (is_part_left && is_capsule_right) || (is_part_right && is_capsule_left) {
        return 0.0;
    }

    let mut name_affinity = 0.0;
    // Semantic keyword bonus
    if slot_norm.contains("head") && (norm_name.contains("head") || norm_name.contains("face") || norm_name.contains("hair")) {
        name_affinity += 1.5;
    } else if slot_norm.contains("shoulder") && (norm_name.contains("shoulder") || norm_name.contains("arm") || norm_name.contains("bicep")) && !is_hand_or_forearm(&norm_name) {
        name_affinity += 1.5;
    } else if slot_norm.contains("elbow") && is_hand_or_forearm(&norm_name) {
        // The biped has no hand bone: its Elbow slot (the forearm) carries
        // the hand, so wrists, palms, fists and fingers ride on it (F7.4).
        name_affinity += 1.5;
    } else if slot_norm.contains("thigh") && (norm_name.contains("thigh") || (norm_name.contains("leg") && !norm_name.contains("shin") && !norm_name.contains("knee") && !norm_name.contains("foot"))) {
        name_affinity += 1.5;
    } else if slot_norm.contains("knee") && (norm_name.contains("knee") || norm_name.contains("shin") || norm_name.contains("foot") || norm_name.contains("calf")) {
        name_affinity += 1.5;
    } else if slot_norm.contains("hips") && (norm_name.contains("hips") || norm_name.contains("pelvis") || norm_name.contains("waist") || norm_name.contains("dhoti") || norm_name.contains("skirt")) {
        name_affinity += 1.5;
    } else if slot_norm.contains("chest") && (norm_name.contains("chest") || norm_name.contains("torso") || norm_name.contains("body") || norm_name.contains("kurta")) {
        name_affinity += 1.5;
    }

    geometric_overlap(part, capsule) + name_affinity
}

/// The fraction of the part (mask pixels, or bounds samples) inside the
/// bone's capsule — the geometric half of [`compute_overlap_score`].
pub fn geometric_overlap(part: &VectorPartCandidate, capsule: &BoneCapsule) -> f64 {
    if let Some(ref mask) = part.mask {
        let (w, h) = mask.dimensions();
        if w == 0 || h == 0 {
            0.0
        } else {
            let mut inside_count = 0usize;
            let mut total_count = 0usize;

            // Sample every 4th pixel for speed
            for y in (0..h).step_by(4) {
                for x in (0..w).step_by(4) {
                    if mask.get_pixel(x, y)[0] > 64 {
                        total_count += 1;
                        let pt = Point::new(x as f64, y as f64);
                        if capsule.contains_point(pt) {
                            inside_count += 1;
                        }
                    }
                }
            }
            if total_count > 0 {
                inside_count as f64 / total_count as f64
            } else {
                0.0
            }
        }
    } else {
        // Fallback: evaluate bounds overlap with capsule
        let mut inside = 0usize;
        let b = part.bounds;
        let samples = [
            Point::new(b.x0, b.y0),
            Point::new(b.x1, b.y0),
            Point::new(b.x0, b.y1),
            Point::new(b.x1, b.y1),
            b.center(),
            Point::new(b.center().x, b.y0),
            Point::new(b.center().x, b.y1),
            Point::new(b.x0, b.center().y),
            Point::new(b.x1, b.center().y),
        ];
        for pt in samples {
            if capsule.contains_point(pt) {
                inside += 1;
            }
        }
        inside as f64 / samples.len() as f64
    }
}

/// True when the part's name says one side and the slot is the other.
fn side_conflict(part_name: &str, slot_name: &str) -> bool {
    let n = part_name.to_lowercase().replace(['_', '-'], " ");
    let s = slot_name.to_lowercase();
    let left = n.contains("left") || n.ends_with(" l") || n.contains(" l ");
    let right = n.contains("righ") || n.ends_with(" r") || n.contains(" r ");
    (left && s.contains(" r")) || (right && s.contains(" l"))
}

/// Maps each candidate vector part to the best-matching rig bone slot.
pub fn map_parts_to_slots(
    parts: &[VectorPartCandidate],
    capsules: &[BoneCapsule],
) -> Vec<PartSlotMapping> {
    // **Whose left?** An artist names layers either from the character's
    // side or from the viewer's; DWPose's L/R are the character's. Named the
    // other way, every "right" part lands on the other arm's bones and tears
    // off when the rig poses (F7 run: sadhuSide's forearm flew off in wave).
    // So the parts are mapped both ways — names as written, and with left and
    // right swapped — and the reading whose parts sit better *inside* their
    // bones (geometric overlap only) wins. A tie keeps the names as written.
    let as_written = map_parts_to_slots_once(parts, capsules);
    let swapped_parts: Vec<VectorPartCandidate> = parts
        .iter()
        .map(|p| VectorPartCandidate { name: swap_sides(&p.name), ..p.clone() })
        .collect();
    let mut swapped = map_parts_to_slots_once(&swapped_parts, capsules);
    let fit = |m: &[PartSlotMapping], ps: &[VectorPartCandidate]| -> f64 {
        m.iter()
            .zip(ps)
            .map(|(m, p)| geometric_overlap(p, &capsules[m.slot_index.min(capsules.len() - 1)]))
            .sum()
    };
    let (a, b) = (fit(&as_written, parts), fit(&swapped, &swapped_parts));
    if b > a + 1e-6 {
        for (m, p) in swapped.iter_mut().zip(parts) {
            m.part_name = p.name.clone();
        }
        swapped
    } else {
        as_written
    }
}

/// Swaps "left"/"right" (and the side tokens " l"/" r", `_l`/`_r`) in a
/// part name, keeping everything else.
fn swap_sides(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::with_capacity(name.len());
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < name.len() {
        let rest = &lower[i..];
        if rest.starts_with("left") {
            out.push_str("right");
            i += 4;
        } else if rest.starts_with("right") {
            out.push_str("left");
            i += 5;
        } else if rest.starts_with("righ") {
            out.push_str("left");
            i += 4;
        } else if (bytes[i] == b'l' || bytes[i] == b'r')
            && i > 0
            && matches!(bytes[i - 1], b' ' | b'_' | b'-')
            && (i + 1 == bytes.len() || matches!(bytes[i + 1], b' ' | b'_' | b'-'))
        {
            out.push(if bytes[i] == b'l' { 'r' } else { 'l' });
            i += 1;
        } else {
            let ch = name[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn map_parts_to_slots_once(
    parts: &[VectorPartCandidate],
    capsules: &[BoneCapsule],
) -> Vec<PartSlotMapping> {
    let mut mappings = Vec::with_capacity(parts.len());

    for part in parts {
        let mut best_slot = 0;
        let mut best_score = -1.0;

        for (idx, capsule) in capsules.iter().enumerate() {
            let score = compute_overlap_score(part, capsule);
            if score > best_score {
                best_score = score;
                best_slot = idx;
            }
        }

        // Nothing overlapped and no name matched: take the nearest bone to
        // the part's centre rather than slot 0 (Hips), which is where F6a's
        // palms ended up.
        if best_score <= 0.0 {
            let c = part.bounds.center();
            if let Some((idx, _)) = capsules
                .iter()
                .enumerate()
                .filter(|(_, cap)| compute_overlap_score(part, cap) >= 0.0 && !side_conflict(&part.name, &cap.slot_name))
                .map(|(i, cap)| (i, cap.distance_to_point(c)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
            {
                best_slot = idx;
            }
        }

        let capsule = &capsules[best_slot];
        let pivot = (capsule.head.x, capsule.head.y);

        mappings.push(PartSlotMapping {
            part_name: part.name.clone(),
            slot_index: capsule.slot_index,
            slot_name: capsule.slot_name.clone(),
            pivot,
            overlap_score: best_score,
        });
    }

    mappings
}

/// Rigs or re-rigs an existing vector character by mapping its parts to bone slots,
/// setting joint pivots, and assembling an ArmatureData via `autorig::assemble`.
///
/// `limits` overrides the pattern's joint limits per slot (the user's saved
/// `rig_limits.json`); `None` keeps the built-in anatomical ones.
pub fn rerig_vector_character(
    skel: &Skeleton,
    parts: &[VectorPartCandidate],
    character_name: &str,
    limits: Option<&buzz_rig::JointLimitDefaults>,
) -> anyhow::Result<ReriggedCharacter> {
    let capsules = build_bone_capsules(skel);
    let mappings = map_parts_to_slots(parts, &capsules);

    let mut pattern = RigPattern::biped();
    if let Some(limits) = limits {
        pattern.apply_limits(limits);
    }

    // Prepare inputs for autorig::assemble in back-to-front paint order
    let mut autorig_parts: Vec<(usize, Arc<Object>)> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let slot = mappings[i].slot_index;
        autorig_parts.push((slot, part.object.clone()));
    }

    // Pivot each bone at its own detected joint rather than forcing it to
    // start at its parent's tip — see `character::biped_joints_from_skeleton`.
    let joints = crate::character::biped_joints_from_skeleton(skel);
    let mut armature = autorig::assemble_from_joints(&pattern, &joints, &autorig_parts)
        .ok_or_else(|| anyhow::anyhow!("Failed to assemble armature for character '{character_name}'"))?;

    // Add standard pose library (idle, walk, run, wave, point, sit)
    armature.poses = PoseLibrary::standard().to_named_poses(&armature);

    // Build the character symbol container
    let mut char_symbol = Symbol::new(SymbolId(1), character_name, SymbolKind::Graphic);
    if let Some(hips) = skel.joint("Hips") {
        char_symbol.registration = Point::new(hips.x as f64, hips.y as f64);
    }

    Ok(ReriggedCharacter {
        armature,
        mappings,
        character_symbol: char_symbol,
        motion_ranges: Vec::new(),
    })
}

/// Classifies motions across a sequence of keypoint frames from a character's timeline,
/// segmenting and labeling frame ranges (e.g. "walk 0–39", "idle 40–60").
pub fn classify_timeline_motions(
    frames: &[KeypointFrame],
    fps: f64,
) -> Vec<TimelineMotionRange> {
    if frames.len() < 4 {
        return Vec::new();
    }

    let mut seq = MotionSequence::new(fps);
    for f in frames {
        seq.push(f.clone());
    }

    let total_frames = frames.len();
    let classification = classify_motion(&seq);

    let motion_name = match classification.locomotion {
        LocomotionClassification::Walk => "walk",
        LocomotionClassification::Run => "run",
        LocomotionClassification::Idle => {
            if classification.hand_gesture {
                "gesture"
            } else if classification.eye_movement {
                "eyes"
            } else {
                "idle"
            }
        }
    };

    let label = format!("{} 0–{}", motion_name, total_frames - 1);

    vec![TimelineMotionRange {
        label,
        classification,
        start_frame: 0,
        end_frame: (total_frames - 1) as u32,
    }]
}

// ---------------------------------------------------------------------------
// AI Rig as Layers (docs/AI_LAYER_RIG_AND_MOTION_COPY_PLAN.md R1): the same
// capsule overlap and name affinity, on `RigPattern::biped_full`'s seventeen
// slots, one piece per top-level object, any number of pieces per slot.
// ---------------------------------------------------------------------------

/// `(head, tip)` per `RigPattern::biped_full` slot from a detected skeleton,
/// in the skeleton's coordinates; `None` where the detector was not sure.
pub fn biped_full_joints(skel: &Skeleton) -> Vec<Option<(Point, Point)>> {
    crate::character::biped_joints_from_skeleton(skel)
}

/// A standing, front-facing figure filling `bounds`, as `(head, tip)` per
/// `RigPattern::biped_full` slot: the stand-in skeleton when no detector ran.
/// "L" slots on the viewer's left, as the pattern's own rest angles have them.
pub fn guessed_biped_full_joints(bounds: Rect) -> Vec<Option<(Point, Point)>> {
    let h = bounds.height().max(1.0);
    let cx = bounds.center().x;
    let p = |dx: f64, fy: f64| Point::new(cx + dx * h, bounds.y0 + fy * h);
    let shoulder = |s: f64| (p(s * 0.11, 0.20), p(s * 0.14, 0.37));
    let elbow = |s: f64| (p(s * 0.14, 0.37), p(s * 0.15, 0.50));
    let wrist = |s: f64| (p(s * 0.15, 0.50), p(s * 0.15, 0.57));
    let thigh = |s: f64| (p(s * 0.06, 0.53), p(s * 0.06, 0.75));
    let knee = |s: f64| (p(s * 0.06, 0.75), p(s * 0.06, 0.94));
    let ankle = |s: f64| (p(s * 0.06, 0.94), p(s * 0.10, 0.98));
    let brow = |s: f64| (p(s * 0.015, 0.08), p(s * 0.05, 0.08));
    vec![
        Some((p(0.0, 0.55), p(0.0, 0.42))), // Hips
        Some((p(0.0, 0.42), p(0.0, 0.17))), // Chest
        Some((p(0.0, 0.17), p(0.0, 0.0))),  // Head
        Some(shoulder(-1.0)),               // Shoulder L
        Some(elbow(-1.0)),                  // Elbow L
        Some(shoulder(1.0)),                // Shoulder R
        Some(elbow(1.0)),                   // Elbow R
        Some(thigh(-1.0)),                  // Thigh L
        Some(knee(-1.0)),                   // Knee L
        Some(thigh(1.0)),                   // Thigh R
        Some(knee(1.0)),                    // Knee R
        Some(wrist(-1.0)),                  // Wrist L
        Some(wrist(1.0)),                   // Wrist R
        Some(ankle(-1.0)),                  // Ankle L
        Some(ankle(1.0)),                   // Ankle R
        Some(brow(-1.0)),                   // Brow L
        Some(brow(1.0)),                    // Brow R
    ]
}

/// One capsule per `RigPattern::biped_full` slot from `(head, tip)` joints
/// (`guessed` fills any the detector left out), radii as a fraction of the
/// figure's `height`.
pub fn biped_full_capsules(
    joints: &[Option<(Point, Point)>],
    guessed: &[Option<(Point, Point)>],
    height: f64,
) -> Vec<BoneCapsule> {
    let pattern = RigPattern::biped_full();
    let radius = [
        0.10, 0.11, 0.12, 0.05, 0.045, 0.05, 0.045, 0.07, 0.06, 0.07, 0.06, 0.05, 0.05, 0.045, 0.045,
        0.025, 0.025,
    ];
    pattern
        .slots
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            let (head, tail) = joints
                .get(index)
                .copied()
                .flatten()
                .or_else(|| guessed.get(index).copied().flatten())
                .unwrap_or((Point::ORIGIN, Point::ORIGIN));
            BoneCapsule {
                slot_index: index,
                slot_name: slot.name.clone(),
                head,
                tail,
                radius: height.max(1.0) * radius.get(index).copied().unwrap_or(0.05),
            }
        })
        .collect()
}

/// What a piece's name says it is, as a `biped_full` slot stem ("Head",
/// "Wrist", "Brow", ...), or `None` when the name says nothing about the body.
/// Whole words (`buzz_rig::pattern::tokens`), so `forearm` is not an arm.
pub fn body_word(name: &str) -> Option<&'static str> {
    // A slot's own or display name ("Wrist R" is the hand), then the user's
    // Animate convention ("rightWrist" is the forearm, "leftPalm" the hand,
    // "midCloth" the hips), then the words below.
    if let Some(slot) = buzz_rig::pattern::canonical_slot(name) {
        return Some(match slot.trim_end_matches(" L").trim_end_matches(" R") {
            "Hips" => "Hips",
            "Chest" => "Chest",
            "Head" => "Head",
            "Shoulder" => "Shoulder",
            "Elbow" => "Elbow",
            "Wrist" => "Wrist",
            "Thigh" => "Thigh",
            "Knee" => "Knee",
            "Ankle" => "Ankle",
            _ => "Brow",
        });
    }
    if let Some(part) = buzz_rig::pattern::body_part(name) {
        return Some(part);
    }
    let words = buzz_rig::pattern::tokens(name);
    let has = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    let joined = words.join(" ");
    if has(&["brow", "brows", "eyebrow", "eyebrows"]) {
        return Some("Brow");
    }
    if has(&[
        "head", "face", "eye", "eyes", "eyelid", "eyelids", "pupil", "pupils", "mouth", "lip", "lips",
        "hair", "nose", "ear", "ears", "beard", "moustache", "mustache", "teeth", "tongue", "skull",
        "cap", "hat", "turban", "cheek", "chin", "jaw", "glasses", "neck",
    ]) {
        return Some("Head");
    }
    if has(&[
        "hand", "hands", "palm", "palms", "wrist", "wrists", "fist", "finger", "fingers", "thumb",
    ]) {
        return Some("Wrist");
    }
    if has(&[
        "foot", "feet", "shoe", "shoes", "boot", "boots", "ankle", "toe", "toes", "sandal", "chappal",
    ]) {
        return Some("Ankle");
    }
    if has(&["forearm", "elbow"]) || joined.contains("lower arm") {
        return Some("Elbow");
    }
    if has(&["knee", "shin", "calf"]) || joined.contains("lower leg") {
        return Some("Knee");
    }
    if has(&["arm", "arms", "upperarm", "shoulder", "bicep", "sleeve"]) {
        return Some("Shoulder");
    }
    if has(&["thigh", "leg", "legs"]) {
        return Some("Thigh");
    }
    if has(&[
        "hips", "hip", "pelvis", "waist", "dhoti", "skirt", "pants", "trouser", "trousers", "shorts",
        "lungi", "pyjama", "pajama",
    ]) {
        return Some("Hips");
    }
    if has(&["chest", "torso", "body", "kurta", "shirt", "vest", "jacket", "coat", "spine", "belly"]) {
        return Some("Chest");
    }
    None
}

/// `(left, right)`: which side a piece's name says it is on.
fn name_sides(name: &str) -> (bool, bool) {
    // `buzz_rig::pattern::name_side`: substrings as the user's own Animate
    // commands match, so typos read (bhatta2's `righFoot`, `righttThigh`,
    // `man2BBleftEyebrow`, `lefEyebrow1`).
    match buzz_rig::pattern::name_side(name) {
        Some(buzz_rig::Side::Left) => (true, false),
        Some(buzz_rig::Side::Right) => (false, true),
        _ => {
            let (squashed, words) = (name.to_lowercase(), buzz_rig::pattern::tokens(name));
            let both = |w: &[&str]| words.iter().any(|t| w.contains(&t.as_str()));
            // Both said: as before, both flags.
            let left = squashed.contains("lef") || both(&["l", "lt", "lft"]);
            let right = squashed.contains("righ") || both(&["r", "rt", "rgt"]);
            (left && right, left && right)
        }
    }
}

/// A piece's own name with its side written the rig's way — "Right Eye" →
/// "Eye R", "palmLeft" → "Palm L" — and a name that says no side (or both)
/// as it is ("Mouth").
pub fn side_normalised_name(name: &str) -> String {
    let (left, right) = name_sides(name);
    if left == right {
        return name.trim().to_string();
    }
    let words: Vec<String> = buzz_rig::pattern::tokens(name)
        .into_iter()
        .filter(|w| {
            !matches!(w.as_str(), "l" | "lt" | "lft" | "r" | "rt" | "rgt") && !w.starts_with("lef") && !w.starts_with("righ")
        })
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        })
        .collect();
    let side = if left { "L" } else { "R" };
    if words.is_empty() { side.to_string() } else { format!("{} {side}", words.join(" ")) }
}

/// [`compute_overlap_score`] for a `biped_full` capsule: the geometric
/// overlap plus 1.5 when the name names the slot ([`body_word`]); `0.0` when
/// the name says the other side.
pub fn full_overlap_score(part: &VectorPartCandidate, capsule: &BoneCapsule) -> f64 {
    full_score(part, capsule, false)
}

/// The side a name says, read with left and right swapped when `swap`.
fn sides_read(name: &str, swap: bool) -> (bool, bool) {
    let (left, right) = name_sides(name);
    if swap { (right, left) } else { (left, right) }
}

/// True when a name that says one side meets a slot of the other.
fn sides_clash((left, right): (bool, bool), slot: &str) -> bool {
    let (slot_left, slot_right) = (slot.ends_with(" L"), slot.ends_with(" R"));
    (left && !right && slot_right) || (right && !left && slot_left)
}

fn full_score(part: &VectorPartCandidate, capsule: &BoneCapsule, swap: bool) -> f64 {
    let slot = capsule.slot_name.as_str();
    if sides_clash(sides_read(&part.name, swap), slot) {
        return 0.0;
    }
    let stem = slot.trim_end_matches(" L").trim_end_matches(" R");
    let affinity = match body_word(&part.name) {
        Some(word) if word == stem => 1.5,
        _ => 0.0,
    };
    geometric_overlap(part, capsule) + affinity
}

/// Each piece onto its best `biped_full` slot (several pieces may share one):
/// the best [`full_overlap_score`]; a head piece (eyes, mouth, hair, ...)
/// always on Head and a brow on its side's Brow (Head when it names no side);
/// nothing at all → the capsule nearest the piece's middle on its side.
/// **A side word in a name wins**: a piece whose name says left or right
/// keeps that side unless, with `try_swap` (real detected joints), geometry
/// clearly contradicts it — a [`full_overlap_score`] (name affinity included)
/// under [`SIDE_CONTRADICTION`] on the side it names, and higher on the other. Each sided
/// piece's call comes back beside its mapping.
pub fn map_pieces_to_full_slots(
    parts: &[VectorPartCandidate],
    capsules: &[BoneCapsule],
    try_swap: bool,
) -> Vec<(PartSlotMapping, Option<SideCall>)> {
    let as_written = map_pieces_once(parts, capsules, false);
    // The names' sides swapped, not the names: `swap_sides` lowercases, and
    // `body_word` needs the camel case to find the words in `palmLeft`.
    let swapped = map_pieces_once(parts, capsules, true);
    let inside = |m: &PartSlotMapping, p: &VectorPartCandidate| {
        capsules.get(m.slot_index).map_or(0.0, |c| geometric_overlap(p, c))
    };
    as_written
        .into_iter()
        .zip(swapped)
        .zip(parts)
        .map(|((written, other), part)| {
            let (left, right) = name_sides(&part.name);
            if left == right {
                return (written, None);
            }
            // The score includes the name's own affinity, so a name that
            // says the part as well as the side ("OldmanFrontArmLeft") is
            // never overruled; only a bare side word on a piece that sits
            // in neither its bone nor its name's can be.
            let flip = try_swap
                && written.overlap_score < SIDE_CONTRADICTION
                && other.overlap_score > written.overlap_score;
            let call = SideCall {
                named: if left { 'L' } else { 'R' },
                flipped: flip,
                named_score: written.overlap_score,
                other_score: other.overlap_score,
                named_overlap: inside(&written, part),
                other_overlap: inside(&other, part),
            };
            (if flip { other } else { written }, Some(call))
        })
        .collect()
}

/// Below this score on the side its side word names, geometry
/// is taken to contradict the name ([`map_pieces_to_full_slots`]).
pub const SIDE_CONTRADICTION: f64 = 0.2;

/// How a piece whose name says a side was placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideCall {
    /// The side the name says: `'L'` or `'R'`.
    pub named: char,
    /// Put on the other side, because geometry contradicted the name.
    pub flipped: bool,
    /// [`full_overlap_score`] on the named side's best slot, and on the
    /// other side's (what the flip is decided on).
    pub named_score: f64,
    pub other_score: f64,
    /// Share of the piece inside the named side's bone, and the other's.
    pub named_overlap: f64,
    pub other_overlap: f64,
}

fn map_pieces_once(
    parts: &[VectorPartCandidate],
    capsules: &[BoneCapsule],
    swap: bool,
) -> Vec<PartSlotMapping> {
    let named = |name: &str| capsules.iter().position(|c| c.slot_name == name);
    parts
        .iter()
        .map(|part| {
            let (left, right) = sides_read(&part.name, swap);
            let forced = match body_word(&part.name) {
                Some("Head") => named("Head"),
                Some("Brow") if left && !right => named("Brow L"),
                Some("Brow") if right && !left => named("Brow R"),
                Some("Brow") => named("Head"),
                _ => None,
            };
            let (best, score) = match forced {
                Some(index) => (index, full_score(part, &capsules[index], swap).max(1.5)),
                None => {
                    let mut best = 0;
                    let mut best_score = -1.0;
                    for (index, capsule) in capsules.iter().enumerate() {
                        let score = full_score(part, capsule, swap);
                        if score > best_score {
                            best_score = score;
                            best = index;
                        }
                    }
                    if best_score <= 0.0 {
                        let c = part.bounds.center();
                        if let Some((index, _)) = capsules
                            .iter()
                            .enumerate()
                            .filter(|(_, cap)| !sides_clash((left, right), &cap.slot_name))
                            .map(|(i, cap)| (i, cap.distance_to_point(c)))
                            .min_by(|a, b| a.1.total_cmp(&b.1))
                        {
                            best = index;
                        }
                    }
                    (best, best_score.max(0.0))
                }
            };
            let capsule = &capsules[best];
            PartSlotMapping {
                part_name: part.name.clone(),
                slot_index: capsule.slot_index,
                slot_name: capsule.slot_name.clone(),
                pivot: (capsule.head.x, capsule.head.y),
                overlap_score: score,
            }
        })
        .collect()
}
