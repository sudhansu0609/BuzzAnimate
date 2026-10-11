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
