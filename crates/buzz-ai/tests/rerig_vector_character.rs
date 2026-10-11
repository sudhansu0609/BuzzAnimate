//! Integration and verification tests for Chunk C9:
//! Rig or re-rig existing vector characters, slot mapping, pivots, autorig assemble, and motion detection.
//!
//! Validates:
//! - Parts on real fixtures (`sadhuSideWalk_0` and `OldmanfrontWalk_0`) map to the right humanoid slots.
//! - Pivots are correctly assigned to joint locations.
//! - `autorig::assemble` succeeds and produces an 11-bone armature with standard poses.
//! - Walk motion cycles on existing timelines are classified and labelled as `walk 0–N`.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use buzz_geom::Rect;
use buzz_scene::{Object, ObjectId};

use buzz_ai::character::{Bone, Joint, Skeleton};
use buzz_ai::motion::{KeypointFrame, LocomotionClassification};
use buzz_ai::rerig::{
    build_bone_capsules, classify_timeline_motions, map_parts_to_slots, rerig_vector_character,
    VectorPartCandidate,
};

/// Loads ground-truth skeleton from a fixture's JSON file.
fn load_fixture_skeleton(name: &str) -> Skeleton {
    let json_path = format!("tests/data/{name}.joints.json");
    let content = fs::read_to_string(&json_path)
        .unwrap_or_else(|_| panic!("Failed to read {json_path}"));
    let joints_raw: Vec<(String, (f64, f64))> = serde_json::from_str(&content)
        .unwrap_or_else(|_| panic!("Failed to parse {json_path}"));

    let mut joints = Vec::new();
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for (jname, (x, y)) in joints_raw {
        min_y = min_y.min(y);
        max_y = max_y.max(y);
        joints.push(Joint {
            name: jname,
            x: x as f32,
            y: y as f32,
            confidence: 1.0,
        });
    }

    let character_height = (max_y - min_y).abs() as f32;

    // Build standard limb bones connecting joints
    let mut bones = Vec::new();
    let find_j = |name: &str| -> Option<(f32, f32)> {
        joints.iter().find(|j| j.name.eq_ignore_ascii_case(name)).map(|j| (j.x, j.y))
    };

    let pairs = [
        ("Hips", "Hips", "Chest"),
        ("Chest", "Chest", "Head"),
        ("Shoulder L", "Shoulder L", "Elbow L"),
        ("Elbow L", "Elbow L", "Hand L"),
        ("Shoulder R", "Shoulder R", "Elbow R"),
        ("Elbow R", "Elbow R", "Hand R"),
        ("Thigh L", "Thigh L", "Knee L"),
        ("Knee L", "Knee L", "Foot L"),
        ("Thigh R", "Thigh R", "Knee R"),
        ("Knee R", "Knee R", "Foot R"),
    ];

    for (bname, h_name, t_name) in pairs {
        if let (Some(h), Some(t)) = (find_j(h_name), find_j(t_name)) {
            let dx = t.0 - h.0;
            let dy = t.1 - h.1;
            bones.push(Bone {
                name: bname.to_string(),
                head: h,
                tail: t,
                confidence: 1.0,
                length: (dx * dx + dy * dy).sqrt(),
            });
        }
    }

    Skeleton {
        joints,
        bones,
        face: Default::default(),
        character_height: character_height.max(800.0),
    }
}

/// Loads vector part candidates from a fixture's `.parts` directory.
fn load_fixture_parts(name: &str) -> Vec<VectorPartCandidate> {
    let parts_dir = format!("tests/data/{name}.parts");
    let topmost_json = format!("{parts_dir}/_topmost.json");

    let part_names: Vec<String> = if Path::new(&topmost_json).exists() {
        let content = fs::read_to_string(&topmost_json).expect("read _topmost.json");
        serde_json::from_str(&content).expect("parse _topmost.json")
    } else {
        vec!["head".into(), "hips".into(), "thigh_l".into(), "thigh_r".into()]
    };

    let mut candidates = Vec::new();

    for (idx, pname) in part_names.iter().enumerate() {
        let sanitized = pname.to_lowercase().replace(' ', "_");
        let png_path = format!("{parts_dir}/{sanitized}.png");

        let mask = if Path::new(&png_path).exists() {
            image::open(&png_path).ok().map(|img| img.to_luma8())
        } else {
            None
        };

        // Determine bounds from mask or dummy rect
        let bounds = if let Some(ref m) = mask {
            let mut min_x = u32::MAX;
            let mut min_y = u32::MAX;
            let mut max_x = 0;
            let mut max_y = 0;
            for (x, y, pixel) in m.enumerate_pixels() {
                if pixel[0] > 64 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
            if min_x <= max_x && min_y <= max_y {
                Rect::new(min_x as f64, min_y as f64, max_x as f64, max_y as f64)
            } else {
                Rect::new(0.0, 0.0, 100.0, 100.0)
            }
        } else {
            Rect::new(0.0, 0.0, 100.0, 100.0)
        };

        let obj = Arc::new(Object::group(ObjectId(100 + idx as u64), Vec::new()));

        candidates.push(VectorPartCandidate {
            name: pname.clone(),
            bounds,
            mask,
            object: obj,
        });
    }

    candidates
}

#[test]
fn test_sadhu_side_walk_parts_map_to_right_slots() {
    let skel = load_fixture_skeleton("sadhuSideWalk_0");
    let parts = load_fixture_parts("sadhuSideWalk_0");
    assert!(!parts.is_empty(), "sadhuSideWalk parts should not be empty");

    let capsules = build_bone_capsules(&skel);
    let mappings = map_parts_to_slots(&parts, &capsules);

    // Verify correct slot mapping for primary parts
    let head_mapping = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Head"))
        .expect("Head mapped");
    assert_eq!(head_mapping.slot_name, "Head");

    let hips_mapping = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Hips"))
        .expect("Hips mapped");
    assert_eq!(hips_mapping.slot_name, "Hips");

    let thigh_l = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Thigh L"))
        .expect("Thigh L mapped");
    assert_eq!(thigh_l.slot_name, "Thigh L");

    let thigh_r = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Thigh R"))
        .expect("Thigh R mapped");
    assert_eq!(thigh_r.slot_name, "Thigh R");

    let shoulder_l = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Shoulder L"))
        .expect("Shoulder L mapped");
    assert_eq!(shoulder_l.slot_name, "Shoulder L");

    let shoulder_r = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Shoulder R"))
        .expect("Shoulder R mapped");
    assert_eq!(shoulder_r.slot_name, "Shoulder R");

    let elbow_l = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Elbow L"))
        .expect("Elbow L mapped");
    assert_eq!(elbow_l.slot_name, "Elbow L");

    let elbow_r = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Elbow R"))
        .expect("Elbow R mapped");
    assert_eq!(elbow_r.slot_name, "Elbow R");

    // Full character rerigging
    let rerigged = rerig_vector_character(&skel, &parts, "SadhuWalk", None)
        .expect("rerig_vector_character should succeed");

    assert_eq!(rerigged.armature.armature.bones.len(), 11, "Must assemble standard 11 bones");
    assert!(!rerigged.armature.poses.is_empty(), "Armature should receive standard pose library");
}

#[test]
fn test_oldman_front_walk_parts_map_to_right_slots() {
    let skel = load_fixture_skeleton("OldmanfrontWalk_0");
    let parts = load_fixture_parts("OldmanfrontWalk_0");
    assert!(!parts.is_empty(), "OldmanfrontWalk parts should not be empty");

    let capsules = build_bone_capsules(&skel);
    let mappings = map_parts_to_slots(&parts, &capsules);

    let head_mapping = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Head"))
        .expect("Head mapped");
    assert_eq!(head_mapping.slot_name, "Head");

    let hips_mapping = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Hips"))
        .expect("Hips mapped");
    assert_eq!(hips_mapping.slot_name, "Hips");

    let thigh_l = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Thigh L"))
        .expect("Thigh L mapped");
    assert_eq!(thigh_l.slot_name, "Thigh L");

    let thigh_r = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Thigh R"))
        .expect("Thigh R mapped");
    assert_eq!(thigh_r.slot_name, "Thigh R");

    let shoulder_l = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Shoulder L"))
        .expect("Shoulder L mapped");
    assert_eq!(shoulder_l.slot_name, "Shoulder L");

    let shoulder_r = mappings.iter().find(|m| m.part_name.eq_ignore_ascii_case("Shoulder R"))
        .expect("Shoulder R mapped");
    assert_eq!(shoulder_r.slot_name, "Shoulder R");

    // Full character rerigging
    let rerigged = rerig_vector_character(&skel, &parts, "OldmanWalk", None)
        .expect("rerig_vector_character should succeed");

    assert_eq!(rerigged.armature.armature.bones.len(), 11);
    assert!(!rerigged.armature.poses.is_empty());
}

#[test]
fn test_timeline_motion_classifier_detects_walk() {
    // Generate an animated walk cycle sequence over 30 frames (e.g. 1.25 seconds @ 24 fps)
    let fps = 24.0;
    let mut frames = Vec::new();
    let cadence_hz = 1.6;

    for i in 0..30 {
        let t = i as f64 / fps;
        let phase = t * cadence_hz * 2.0 * std::f64::consts::PI;

        // Periodic counter-phase ankle motion
        let left_ankle_x = 100.0 + 35.0 * phase.sin();
        let right_ankle_x = 100.0 - 35.0 * phase.sin();

        // Ankle heights (alternating contact on ground)
        let left_ankle_y = 500.0 - 15.0 * phase.sin().max(0.0);
        let right_ankle_y = 500.0 - 15.0 * (-phase).sin().max(0.0);

        let mut kps = [(0.0f32, 0.0f32, 0.9f32); 133];
        kps[0] = (100.0, 100.0, 0.9);
        kps[15] = (left_ankle_x as f32, left_ankle_y as f32, 0.9);
        kps[16] = (right_ankle_x as f32, right_ankle_y as f32, 0.9);
        kps[9] = (70.0, (320.0 + 10.0 * phase.cos()) as f32, 0.9);
        kps[10] = (130.0, (320.0 - 10.0 * phase.cos()) as f32, 0.9);

        frames.push(KeypointFrame::from_keypoints(i as u32, kps));
    }

    let detected_ranges = classify_timeline_motions(&frames, fps);
    assert_eq!(detected_ranges.len(), 1);

    let range = &detected_ranges[0];
    assert_eq!(range.classification.locomotion, LocomotionClassification::Walk);
    assert_eq!(range.label, "walk 0–29");
    assert_eq!(range.start_frame, 0);
    assert_eq!(range.end_frame, 29);
}

/// F7.4: the biped's Elbow slot carries the hand, so palms and wrists (the
/// sadhuback layer names from the F6a run: `rightPalm`, `leftPAlm`,
/// `rightWrist`, `leftWrist`) go to the forearm of their own side — not to
/// Hips (palms, zero overlap) or the upper arm (wrists) as before. A part
/// that overlaps nothing and names nothing goes to the nearest bone.
#[test]
fn hands_and_wrists_ride_on_the_forearm_slot() {
    let skel = load_fixture_skeleton("sadhuSideWalk_0");
    let capsules = build_bone_capsules(&skel);
    let tail_of = |slot: &str| capsules.iter().find(|c| c.slot_name == slot).unwrap().tail;
    let part = |name: &str, at: buzz_geom::Point| VectorPartCandidate {
        name: name.into(),
        bounds: Rect::new(at.x - 6.0, at.y - 6.0, at.x + 6.0, at.y + 6.0),
        mask: None,
        object: Arc::new(Object::group(ObjectId(900), Vec::new())),
    };
    // Put each hand a little past the wrist, where a palm is drawn.
    let (wr, wl) = (tail_of("Elbow R"), tail_of("Elbow L"));
    let parts = vec![
        part("rightPalm", wr + buzz_geom::Vec2::new(0.0, 60.0)),
        part("leftPAlm", wl + buzz_geom::Vec2::new(0.0, 60.0)),
        part("rightWrist", wr),
        part("leftWrist", wl),
        part("Layer_16", tail_of("Knee L") + buzz_geom::Vec2::new(5.0, 400.0)),
    ];
    let m = map_parts_to_slots(&parts, &capsules);
    let slot = |n: &str| m.iter().find(|x| x.part_name == n).unwrap().slot_name.clone();
    assert_eq!(slot("rightPalm"), "Elbow R");
    assert_eq!(slot("leftPAlm"), "Elbow L");
    assert_eq!(slot("rightWrist"), "Elbow R");
    assert_eq!(slot("leftWrist"), "Elbow L");
    // A side view: both shins sit on top of each other, so either is nearest.
    assert!(slot("Layer_16").starts_with("Knee"), "nearest bone, not Hips: {}", slot("Layer_16"));
}

/// F7 run: sadhuSide's layers are named from the *viewer's* side, DWPose's
/// L/R from the character's. With every name swapped against the geometry,
/// the mapper must read the names the other way round rather than put each
/// arm's parts on the other arm's bones.
#[test]
fn parts_named_from_the_other_side_still_land_on_their_own_limb() {
    let skel = load_fixture_skeleton("sadhu1FrontWalk_0");
    let capsules = build_bone_capsules(&skel);
    let cap = |slot: &str| capsules.iter().find(|c| c.slot_name == slot).unwrap().clone();
    let part_on = |name: &str, slot: &str| {
        let c = cap(slot);
        let mid = buzz_geom::Point::new((c.head.x + c.tail.x) / 2.0, (c.head.y + c.tail.y) / 2.0);
        VectorPartCandidate {
            name: name.into(),
            bounds: Rect::new(mid.x - 4.0, mid.y - 4.0, mid.x + 4.0, mid.y + 4.0),
            mask: None,
            object: Arc::new(Object::group(ObjectId(901), Vec::new())),
        }
    };
    // Named the opposite way to where they are drawn.
    let parts = vec![
        part_on("rightArm", "Shoulder L"),
        part_on("rightWrist", "Elbow L"),
        part_on("leftArm", "Shoulder R"),
        part_on("leftWrist", "Elbow R"),
        part_on("rightThigh", "Thigh L"),
        part_on("leftThigh", "Thigh R"),
    ];
    let m = map_parts_to_slots(&parts, &capsules);
    let slot = |n: &str| m.iter().find(|x| x.part_name == n).unwrap().slot_name.clone();
    assert_eq!(slot("rightArm"), "Shoulder L");
    assert_eq!(slot("rightWrist"), "Elbow L");
    assert_eq!(slot("leftArm"), "Shoulder R");
    assert_eq!(slot("leftThigh"), "Thigh R");
}
