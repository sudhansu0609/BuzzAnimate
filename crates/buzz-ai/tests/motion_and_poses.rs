use std::f64::consts::PI;

use buzz_act::figure::Joint;
use buzz_act::perform::Action;
use buzz_ai::character::{
    assemble_character, complete_parts, part_masks, skeleton, trace_character,
};
use buzz_ai::motion::{
    classify_motion, pose_from_skeleton, KeypointFrame,
    LocomotionClassification, MotionSequence, PoseLibrary,
};
use buzz_ai::pose::Person;
use buzz_ai::segment::matte;
use buzz_rig::wrap_pi;
use buzz_rig::RigPattern;
use buzz_scene::{LayerKind, ObjectId, ObjectKind, RigBinding, Scene};

#[test]
fn standard_pose_library_contains_standard_poses_and_applies() {
    let lib = PoseLibrary::standard();
    assert_eq!(lib.poses.len(), 6);

    let pose_names = ["idle", "walk", "run", "wave", "point", "sit"];
    for name in pose_names {
        let pose = lib.get(name);
        assert!(pose.is_some(), "Pose '{}' must exist in library", name);
        let p = pose.unwrap();
        assert_eq!(p.angles.len(), 11, "Must have 11 bone angle deltas");
        for &a in &p.angles {
            assert!(a.is_finite());
            assert!(a >= -PI - 1e-4 && a <= PI + 1e-4);
        }
    }

    // Apply pose to an Armature
    let mut next_id_val = 100u64;
    let mut obj = buzz_act::figure::build(
        &buzz_act::figure::FigureSpec::default(),
        ObjectId(42),
        || {
            next_id_val += 1;
            ObjectId(next_id_val)
        },
    );
    let ObjectKind::Armature(mut arm_data) = obj.kind.clone() else {
        panic!("figure must be armature");
    };

    let rest_pose = arm_data.armature.at_rest().pose();
    assert!(lib.apply_to_armature("sit", &mut arm_data));
    let current_pose = arm_data.armature.pose();
    assert_eq!(current_pose.len(), 11);
    // `CharacterPose::angles` are deltas from rest, not absolute angles: the
    // posed armature must equal rest + delta per bone, wrapped.
    let sit = lib.get("sit").unwrap();
    for i in 0..11 {
        let expected = wrap_pi(rest_pose[i] + sit.angles[i]);
        assert!(
            (current_pose[i] - expected).abs() < 1e-9,
            "bone {i}: posed {} != rest+delta {}",
            current_pose[i],
            expected
        );
    }
    obj.kind = ObjectKind::Armature(arm_data);

    // Apply performance to Scene
    let mut scene = Scene::default();
    let layer = scene.add_layer("Character", LayerKind::Normal);
    scene.add_object(layer, obj);

    let report = PoseLibrary::apply_action(&mut scene, ObjectId(42), Action::Walk, 0..24, 1.0);
    assert!(report.is_ok(), "apply_action must succeed");
    let rep = report.unwrap();
    assert!(rep.keyframes > 0, "must have written keyframes");
}

#[test]
fn pose_from_skeleton_reproduces_bone_angles_within_12_degrees() {
    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
    let true_joints: Vec<(String, (f64, f64))> =
        serde_json::from_str(&joints_str).expect("parse joints");

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (name, (x, y)) in &true_joints {
        let (xf, yf) = (*x as f32, *y as f32);
        match name.as_str() {
            "Head" => keypoints[0] = (xf, yf, 0.95),
            "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
            "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
            "Elbow L" => keypoints[7] = (xf, yf, 0.95),
            "Elbow R" => keypoints[8] = (xf, yf, 0.95),
            "Hand L" => keypoints[9] = (xf, yf, 0.95),
            "Hand R" => keypoints[10] = (xf, yf, 0.95),
            "Thigh L" => keypoints[11] = (xf, yf, 0.95),
            "Thigh R" => keypoints[12] = (xf, yf, 0.95),
            "Knee L" => keypoints[13] = (xf, yf, 0.95),
            "Knee R" => keypoints[14] = (xf, yf, 0.95),
            "Foot L" => keypoints[15] = (xf, yf, 0.95),
            "Foot R" => keypoints[16] = (xf, yf, 0.95),
            _ => {}
        }
    }

    let person = Person {
        bbox: [0.0, 0.0, 600.0, 800.0],
        keypoints,
    };
    let skel = skeleton(&person);
    let estimated = pose_from_skeleton(&skel, "PujariFrontPose");

    assert_eq!(estimated.angles.len(), 11);

    // Compute ground-truth bone directions directly from the ground-truth joints:
    let get_gt = |n: &str| -> (f64, f64) {
        true_joints
            .iter()
            .find(|(name, _)| name == n)
            .map(|(_, pt)| *pt)
            .unwrap()
    };

    let p_sh_l = get_gt("Shoulder L");
    let p_el_l = get_gt("Elbow L");
    let p_hd_l = get_gt("Hand L");
    let p_sh_r = get_gt("Shoulder R");
    let p_el_r = get_gt("Elbow R");
    let p_hd_r = get_gt("Hand R");
    let p_th_l = get_gt("Thigh L");
    let p_kn_l = get_gt("Knee L");
    let p_ft_l = get_gt("Foot L");
    let p_th_r = get_gt("Thigh R");
    let p_kn_r = get_gt("Knee R");
    let p_ft_r = get_gt("Foot R");

    let angle_diff = |a: f64, b: f64| -> f64 {
        wrap_pi(a - b).abs()
    };

    let tolerance = 12.0 * (PI / 180.0); // 12 degrees in radians (~0.209 rad)

    // Check Left Arm
    let gt_sh_l = (p_el_l.1 - p_sh_l.1).atan2(p_el_l.0 - p_sh_l.0);
    let gt_el_l = wrap_pi((p_hd_l.1 - p_el_l.1).atan2(p_hd_l.0 - p_el_l.0) - gt_sh_l);
    assert!(
        angle_diff(estimated.angles[Joint::ElbowL.index()], gt_el_l) <= tolerance,
        "Elbow L angle error within 12 deg (got {}, gt {})",
        estimated.angles[Joint::ElbowL.index()],
        gt_el_l
    );

    // Check Right Arm
    let gt_sh_r = (p_el_r.1 - p_sh_r.1).atan2(p_el_r.0 - p_sh_r.0);
    let gt_el_r = wrap_pi((p_hd_r.1 - p_el_r.1).atan2(p_hd_r.0 - p_el_r.0) - gt_sh_r);
    assert!(
        angle_diff(estimated.angles[Joint::ElbowR.index()], gt_el_r) <= tolerance,
        "Elbow R angle error within 12 deg (got {}, gt {})",
        estimated.angles[Joint::ElbowR.index()],
        gt_el_r
    );

    // Check Left Leg
    let gt_th_l = (p_kn_l.1 - p_th_l.1).atan2(p_kn_l.0 - p_th_l.0);
    let gt_kn_l = wrap_pi((p_ft_l.1 - p_kn_l.1).atan2(p_ft_l.0 - p_kn_l.0) - gt_th_l);
    assert!(
        angle_diff(estimated.angles[Joint::KneeL.index()], gt_kn_l) <= tolerance,
        "Knee L angle error within 12 deg (got {}, gt {})",
        estimated.angles[Joint::KneeL.index()],
        gt_kn_l
    );

    // Check Right Leg
    let gt_th_r = (p_kn_r.1 - p_th_r.1).atan2(p_kn_r.0 - p_th_r.0);
    let gt_kn_r = wrap_pi((p_ft_r.1 - p_kn_r.1).atan2(p_ft_r.0 - p_kn_r.0) - gt_th_r);
    assert!(
        angle_diff(estimated.angles[Joint::KneeR.index()], gt_kn_r) <= tolerance,
        "Knee R angle error within 12 deg (got {}, gt {})",
        estimated.angles[Joint::KneeR.index()],
        gt_kn_r
    );
}

#[test]
fn motion_classifier_identifies_walk_run_idle_and_gestures() {
    let fps = 24.0;
    let n_frames = 48; // 2 seconds

    // 1. Idle Sequence (stationary ankles)
    let mut idle_seq = MotionSequence::new(fps);
    for frame in 0..n_frames {
        let mut kps = [(0.0f32, 0.0f32, 0.9f32); 133];
        kps[0] = (250.0, 100.0, 0.9); // Head
        kps[15] = (230.0, 500.0, 0.9); // Ankle L
        kps[16] = (270.0, 500.0, 0.9); // Ankle R
        kps[9] = (200.0, 300.0, 0.9); // Wrist L
        kps[10] = (300.0, 300.0, 0.9); // Wrist R
        idle_seq.push(KeypointFrame::from_keypoints(frame, kps));
    }
    let idle_res = classify_motion(&idle_seq);
    assert_eq!(idle_res.locomotion, LocomotionClassification::Idle);
    assert!(!idle_res.has_flight_phase);
    assert!(!idle_res.hand_gesture);
    assert!(!idle_res.eye_movement);

    // 2. Walk Sequence (periodic ankles, no flight phase)
    let mut walk_seq = MotionSequence::new(fps);
    for frame in 0..n_frames {
        let phase = frame as f64 / 24.0 * (2.0 * PI); // 1 stride / sec
        let mut kps = [(0.0f32, 0.0f32, 0.9f32); 133];
        kps[0] = (250.0, 100.0, 0.9);
        // Feet alternate: when one is high, other is on ground (y = 500)
        let y_l = 500.0 - (phase.sin().max(0.0) * 30.0) as f32;
        let y_r = 500.0 - ((-phase).sin().max(0.0) * 30.0) as f32;
        let x_l = 250.0 + (phase.cos() * 50.0) as f32;
        let x_r = 250.0 - (phase.cos() * 50.0) as f32;

        kps[15] = (x_l, y_l, 0.9);
        kps[16] = (x_r, y_r, 0.9);
        kps[9] = (200.0, 300.0, 0.9);
        kps[10] = (300.0, 300.0, 0.9);
        walk_seq.push(KeypointFrame::from_keypoints(frame, kps));
    }
    let walk_res = classify_motion(&walk_seq);
    assert_eq!(walk_res.locomotion, LocomotionClassification::Walk);
    assert!(!walk_res.has_flight_phase);

    // 3. Run Sequence (faster tempo + flight phase where both feet leave ground)
    let mut run_seq = MotionSequence::new(fps);
    for frame in 0..n_frames {
        let phase = frame as f64 / 12.0 * (2.0 * PI); // 2 strides / sec
        let mut kps = [(0.0f32, 0.0f32, 0.9f32); 133];
        kps[0] = (250.0, 100.0, 0.9);
        // Both feet elevated during transition
        let y_l = 500.0 - (35.0 + phase.sin() * 25.0) as f32;
        let y_r = 500.0 - (35.0 - phase.sin() * 25.0) as f32;
        let x_l = 250.0 + (phase.cos() * 80.0) as f32;
        let x_r = 250.0 - (phase.cos() * 80.0) as f32;

        kps[15] = (x_l, y_l, 0.9);
        kps[16] = (x_r, y_r, 0.9);
        kps[9] = (200.0, 260.0, 0.9);
        kps[10] = (300.0, 260.0, 0.9);
        run_seq.push(KeypointFrame::from_keypoints(frame, kps));
    }
    let run_res = classify_motion(&run_seq);
    assert_eq!(run_res.locomotion, LocomotionClassification::Run);
    assert!(run_res.has_flight_phase);

    // 4. Hand Gesture Sequence (rapid arm wave)
    let mut wave_seq = MotionSequence::new(fps);
    for frame in 0..n_frames {
        let mut kps = [(0.0f32, 0.0f32, 0.9f32); 133];
        kps[0] = (250.0, 100.0, 0.9);
        kps[15] = (230.0, 500.0, 0.9);
        kps[16] = (270.0, 500.0, 0.9);
        // Right hand rapidly waving high above chest
        let wave_y = 120.0 + ((frame as f64 * 0.8).sin() * 40.0) as f32;
        let wave_x = 320.0 + ((frame as f64 * 0.8).cos() * 50.0) as f32;
        kps[9] = (200.0, 300.0, 0.9);
        kps[10] = (wave_x, wave_y, 0.9);
        wave_seq.push(KeypointFrame::from_keypoints(frame, kps));
    }
    let wave_res = classify_motion(&wave_seq);
    assert!(wave_res.hand_gesture, "Hand gesture must be detected");

    // 5. Eye Movement Sequence (pupil gaze shifting)
    let mut gaze_seq = MotionSequence::new(fps);
    for frame in 0..n_frames {
        let mut kps = [(0.0f32, 0.0f32, 0.9f32); 133];
        kps[0] = (250.0, 100.0, 0.9);
        kps[15] = (230.0, 500.0, 0.9);
        kps[16] = (270.0, 500.0, 0.9);
        let pupil = ((frame as f64 * 0.2).sin() * 0.8, 0.0);
        gaze_seq.push(KeypointFrame::from_keypoints(frame, kps).with_pupil(pupil));
    }
    let gaze_res = classify_motion(&gaze_seq);
    assert!(gaze_res.eye_movement, "Eye movement must be detected");
}

/// Builds a real traced-character armature the same model-free way
/// `assemble_character.rs`'s non-ignored test does (matte + part masks + trace,
/// no ML models), so its rest angles come from the drawing rather than from
/// `RigPattern::biped()`.
fn build_traced_armature() -> buzz_scene::ArmatureData {
    let png_path = "tests/data/pujariFront_0.png";
    let img = image::open(png_path)
        .unwrap_or_else(|e| panic!("{png_path} must be present for this test: {e}"))
        .to_rgba8();
    let matte = matte(&img, None).expect("matte");

    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
    let true_joints: Vec<(String, (f64, f64))> =
        serde_json::from_str(&joints_str).expect("parse joints");

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (name, (x, y)) in &true_joints {
        let (xf, yf) = (*x as f32, *y as f32);
        match name.as_str() {
            "Head" => keypoints[0] = (xf, yf, 0.95),
            "Shoulder L" => keypoints[5] = (xf, yf, 0.95),
            "Shoulder R" => keypoints[6] = (xf, yf, 0.95),
            "Elbow L" => keypoints[7] = (xf, yf, 0.95),
            "Elbow R" => keypoints[8] = (xf, yf, 0.95),
            "Hand L" => keypoints[9] = (xf, yf, 0.95),
            "Hand R" => keypoints[10] = (xf, yf, 0.95),
            "Thigh L" => keypoints[11] = (xf, yf, 0.95),
            "Thigh R" => keypoints[12] = (xf, yf, 0.95),
            "Knee L" => keypoints[13] = (xf, yf, 0.95),
            "Knee R" => keypoints[14] = (xf, yf, 0.95),
            "Foot L" => keypoints[15] = (xf, yf, 0.95),
            "Foot R" => keypoints[16] = (xf, yf, 0.95),
            _ => {}
        }
    }

    let person = Person {
        bbox: [0.0, 0.0, img.width() as f32, img.height() as f32],
        keypoints,
    };
    let skel = skeleton(&person);
    let parts = part_masks(&img, &matte, &skel).expect("part masks");
    let completed = complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced = trace_character(&completed, Some(2.0)).expect("trace character");

    let assembled = assemble_character(
        &skel,
        &traced,
        "Pujari",
        Some((img.width() as f64, img.height() as f64)),
        None,
    )
    .expect("assemble character");

    assembled.armature
}

/// The bug this guards against: `PoseLibrary::standard()` used to turn a
/// `Beat`'s rest-relative deltas into *absolute* angles using the generic
/// `RigPattern::biped()` rest angles, then apply those absolute angles
/// straight onto a traced character's armature — whose own rest angles come
/// from its drawing and differ from the generic ones. Every standard pose
/// must instead bend the character's own rest pose.
#[test]
fn standard_poses_bend_a_traced_characters_own_rest_pose() {
    let real_armature = build_traced_armature();
    let real_rest: Vec<f64> = real_armature.armature.at_rest().pose();

    // (c) Proves this test exercises the bug: the traced rig's own rest pose
    // must differ from the generic biped pattern's on at least one bone.
    let generic_rest: Vec<f64> = RigPattern::biped()
        .slots
        .iter()
        .map(|s| s.rest_angle)
        .collect();
    let max_diff = real_rest
        .iter()
        .zip(generic_rest.iter())
        .map(|(a, b)| wrap_pi(a - b).abs())
        .fold(0.0_f64, f64::max);
    assert!(
        max_diff > 0.05,
        "traced rest {real_rest:?} must differ from generic biped rest {generic_rest:?} \
         by > 0.05 rad on some bone, else this test would not exercise the bug"
    );

    let lib = PoseLibrary::standard();

    // (a) Idle must leave every bone within 0.15 rad of the character's own rest.
    let mut idle_armature = real_armature.clone();
    assert!(lib.apply_to_armature("idle", &mut idle_armature));
    let idle_pose = idle_armature.armature.pose();
    for (i, (&posed, &rest)) in idle_pose.iter().zip(real_rest.iter()).enumerate() {
        let diff = wrap_pi(posed - rest).abs();
        assert!(
            diff < 0.15,
            "bone {i}: idle pose ({posed}) strayed {diff} rad from rest ({rest})"
        );
    }

    // (b) Joint continuity is structural here: a bone's world head/tail is
    // always its parent's tip plus its own *relative* angle (buzz_rig::Armature
    // stores angles relative to the parent), so nothing can tear a limb off by
    // construction. What can go wrong is the angle itself, so assert directly
    // that every posed bone equals rest + delta, for every standard pose.
    for pose in &lib.poses {
        let mut posed_armature = real_armature.clone();
        assert!(lib.apply_to_armature(&pose.name, &mut posed_armature));
        let posed_angles = posed_armature.armature.pose();
        for (i, bone) in posed_armature.armature.bones.iter().enumerate() {
            if bone.parent.is_none() {
                continue;
            }
            // "wave"'s ElbowR is the one deliberate exception: its delta is
            // resolved against this armature's own rest pose (so the hand
            // ends up above the elbow whatever this character's arm rests
            // like) rather than being the library's fixed delta — see
            // `wave_raises_the_hand_above_the_elbow_on_a_bent_rest_arm` and
            // `motion::resolve_deltas`.
            if pose.name == "wave" && i == Joint::ElbowR.index() {
                continue;
            }
            let delta = pose.angles.get(i).copied().unwrap_or(0.0);
            let expected = wrap_pi(real_rest[i] + delta);
            let actual = posed_angles[i];
            assert!(
                (actual - expected).abs() < 1e-6,
                "pose '{}' bone {i}: posed angle {actual} != rest+delta {expected}",
                pose.name
            );
        }
    }
}

/// **A joint-assembled character bends at its real joints, and does not come
/// apart.** `assemble_character` now pivots each bone at its own detected
/// joint (`Bone::offset`) rather than forcing it to start at its parent's
/// tip. For every standard pose this checks two things a detaching limb
/// would break: FK continuity (a child's head is always exactly where its
/// parent carries it, offset and all) and that the artwork on either side of
/// a joint stays the same distance apart it was at rest, not tearing or
/// telescoping as the character moves.
#[test]
fn standard_poses_keep_every_joint_connected() {
    let real_armature = build_traced_armature();
    let lib = PoseLibrary::standard();

    // Which bones carry at least one rigidly-bound part — the "connected
    // parts" half of the check only means anything between two bones that
    // both have real artwork on them.
    let has_rigid_part = |armature: &buzz_scene::ArmatureData, bone: usize| {
        armature
            .parts
            .iter()
            .any(|p| matches!(p.binding, RigBinding::Rigid(b) if b == bone))
    };

    let rest_gaps: Vec<Option<f64>> = real_armature
        .armature
        .bones
        .iter()
        .enumerate()
        .map(|(i, bone)| {
            let parent = bone.parent?;
            (has_rigid_part(&real_armature, i) && has_rigid_part(&real_armature, parent))
                .then(|| (real_armature.armature.head(i) - real_armature.armature.tip(parent)).hypot())
        })
        .collect();

    for pose in &lib.poses {
        let mut posed = real_armature.clone();
        assert!(lib.apply_to_armature(&pose.name, &mut posed));
        let arm = &posed.armature;

        for (i, bone) in arm.bones.iter().enumerate() {
            let Some(parent) = bone.parent else { continue };

            // FK continuity, recomputed independently of `Armature::head`:
            // a child's head is its parent's tip, plus its own fixed offset
            // turned by however far the parent has turned.
            let parent_angle = arm.world_angle(parent);
            let (s, c) = parent_angle.sin_cos();
            let turned_offset = buzz_geom::Vec2::new(
                bone.offset.x * c - bone.offset.y * s,
                bone.offset.x * s + bone.offset.y * c,
            );
            let expected_head = arm.tip(parent) + turned_offset;
            assert!(
                (arm.head(i) - expected_head).hypot() < 1.0,
                "pose '{}' bone {i} ({}): head {:?} strayed from where its \
                 parent carries it, {:?}",
                pose.name,
                bone.name,
                arm.head(i),
                expected_head
            );

            // Connected artwork stays connected: the gap between the two
            // parts' joint-end points does not grow or shrink as the
            // character moves, whatever it was at rest.
            if let Some(rest_gap) = rest_gaps[i] {
                let posed_gap = (arm.head(i) - arm.tip(parent)).hypot();
                assert!(
                    (posed_gap - rest_gap).abs() < 2.0,
                    "pose '{}' bone {i} ({}): joint gap moved from {rest_gap} to \
                     {posed_gap} — the parts came apart",
                    pose.name,
                    bone.name
                );
            }
        }
    }

    // `wave` must actually move the right arm — raising and bending it is
    // the whole pose.
    let mut waved = real_armature.clone();
    assert!(lib.apply_to_armature("wave", &mut waved));
    let rest_shoulder_r = real_armature.armature.world_angle(Joint::ShoulderR.index());
    let waved_shoulder_r = waved.armature.world_angle(Joint::ShoulderR.index());
    let moved = wrap_pi(waved_shoulder_r - rest_shoulder_r).abs();
    assert!(
        moved > 0.5,
        "wave should swing the right shoulder by more than 0.5 rad, moved {moved} rad \
         (rest {rest_shoulder_r}, waved {waved_shoulder_r})"
    );
}

/// A fixed `ElbowR` delta, chosen for a rig whose arm hangs straight down at
/// rest, left the forearm pointing sideways/down on a character whose arm is
/// already bent at rest (pujariFront's namaste-like drawn pose) — the "wave"
/// hand ended up hanging instead of raised. `PoseLibrary::to_named_poses` /
/// `apply_to_armature` must instead resolve the delta against *this*
/// armature's own rest pose so the hand ends up above the elbow. Screen y
/// points down, so "above" is the smaller y.
#[test]
fn wave_raises_the_hand_above_the_elbow_on_a_bent_rest_arm() {
    let real_armature = build_traced_armature();
    let lib = PoseLibrary::standard();

    let mut waved = real_armature.clone();
    assert!(lib.apply_to_armature("wave", &mut waved));

    let elbow_r = Joint::ElbowR.index();
    let head = waved.armature.head(elbow_r);
    let tip = waved.armature.tip(elbow_r);
    assert!(
        tip.y < head.y,
        "wave: Elbow R tip {tip:?} (the hand) should be above (smaller y than) \
         its head {head:?} (the elbow)"
    );

    // The same must hold for the poses stored on the rig itself
    // (`to_named_poses`), which is what a saved character actually plays
    // back from.
    let named = lib.to_named_poses(&real_armature);
    let wave_pose = named.iter().find(|p| p.name == "wave").expect("wave pose stored");
    let mut from_named = real_armature.clone();
    from_named.armature.set_pose(&wave_pose.angles);
    let head2 = from_named.armature.head(elbow_r);
    let tip2 = from_named.armature.tip(elbow_r);
    assert!(
        tip2.y < head2.y,
        "stored wave pose: Elbow R tip {tip2:?} should be above its head {head2:?}"
    );
}
