use buzz_ai::character::skeleton;
use buzz_ai::pose::Person;

#[test]
fn skeleton_maps_keypoints_to_humanoid_slots() {
    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];

    // Nose
    keypoints[0] = (100.0, 50.0, 0.95);
    // Left & Right Eye
    keypoints[1] = (95.0, 45.0, 0.9);
    keypoints[2] = (105.0, 45.0, 0.9);
    // Left & Right Ear
    keypoints[3] = (90.0, 50.0, 0.85);
    keypoints[4] = (110.0, 50.0, 0.85);
    // Shoulders
    keypoints[5] = (80.0, 100.0, 0.9);
    keypoints[6] = (120.0, 100.0, 0.9);
    // Elbows
    keypoints[7] = (70.0, 150.0, 0.85);
    keypoints[8] = (130.0, 150.0, 0.85);
    // Wrists
    keypoints[9] = (65.0, 200.0, 0.85);
    keypoints[10] = (135.0, 200.0, 0.85);
    // Hips
    keypoints[11] = (85.0, 220.0, 0.9);
    keypoints[12] = (115.0, 220.0, 0.9);
    // Knees
    keypoints[13] = (85.0, 300.0, 0.88);
    keypoints[14] = (115.0, 300.0, 0.88);
    // Ankles
    keypoints[15] = (85.0, 380.0, 0.85);
    keypoints[16] = (115.0, 380.0, 0.85);

    let person = Person {
        bbox: [50.0, 30.0, 150.0, 400.0],
        keypoints,
    };

    let skel = skeleton(&person);

    // Height from bbox is 370
    assert!((skel.character_height - 370.0).abs() < 1e-3);

    // Check Neck: midpoint of shoulders (80, 100) and (120, 100) -> (100, 100)
    let neck = skel.joint("Neck").expect("Neck joint exists");
    assert!((neck.x - 100.0).abs() < 1e-3);
    assert!((neck.y - 100.0).abs() < 1e-3);

    // Check Hips: midpoint of hips (85, 220) and (115, 220) -> (100, 220)
    let hips = skel.joint("Hips").expect("Hips joint exists");
    assert!((hips.x - 100.0).abs() < 1e-3);
    assert!((hips.y - 220.0).abs() < 1e-3);

    // Check Chest: midpoint of neck and hips -> (100, 160)
    let chest = skel.joint("Chest").expect("Chest joint exists");
    assert!((chest.x - 100.0).abs() < 1e-3);
    assert!((chest.y - 160.0).abs() < 1e-3);

    // Check bones existence and lengths
    let bone_thigh_l = skel.bone("Thigh L").expect("Thigh L bone exists");
    assert!((bone_thigh_l.head.0 - 85.0).abs() < 1e-3);
    assert!((bone_thigh_l.head.1 - 220.0).abs() < 1e-3);
    assert!((bone_thigh_l.tail.0 - 85.0).abs() < 1e-3);
    assert!((bone_thigh_l.tail.1 - 300.0).abs() < 1e-3);
    assert!((bone_thigh_l.length - 80.0).abs() < 1e-3);

    let bone_knee_l = skel.bone("Knee L").expect("Knee L bone exists");
    assert!((bone_knee_l.length - 80.0).abs() < 1e-3);

    // Verify round-trip JSON serialization
    let json = serde_json::to_string(&skel).expect("skeleton serializes");
    let deserialized: buzz_ai::character::Skeleton =
        serde_json::from_str(&json).expect("skeleton deserializes");
    assert_eq!(skel, deserialized);
}
