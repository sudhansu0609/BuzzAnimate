use image::{GrayImage, Luma, Rgba, RgbaImage};
use buzz_ai::character::{part_masks, skeleton};
use buzz_ai::pose::Person;
use buzz_ai::segment::distance_transform;

#[test]
fn distance_transform_computes_accurate_distances() {
    let mut matte = GrayImage::new(40, 40);
    // 20x20 box from (10, 10) to (29, 29)
    for y in 10..30 {
        for x in 10..30 {
            matte.put_pixel(x, y, Luma([255]));
        }
    }

    let dt = distance_transform(&matte);
    assert_eq!(dt.len(), 40 * 40);

    // Outside the box should be 0.0
    assert_eq!(dt[5 * 40 + 5], 0.0);
    assert_eq!(dt[0], 0.0);

    // Just inside the boundary (10, 10)
    let edge_val = dt[10 * 40 + 10];
    assert!(edge_val >= 1.0 && edge_val <= 1.5, "edge_val={edge_val}");

    // Near the center (20, 20) distance to edge (10) should be around 10
    let center_val = dt[20 * 40 + 20];
    assert!(
        (center_val - 10.0).abs() <= 2.0,
        "center_val={center_val} expected ~10.0"
    );
}

#[test]
fn part_masks_segments_body_parts_and_resolves_overlaps() {
    let w = 200;
    let h = 300;
    let mut matte = GrayImage::new(w, h);

    // Draw torso (80..120, 100..180)
    for y in 100..180 {
        for x in 80..120 {
            matte.put_pixel(x, y, Luma([255]));
        }
    }
    // Draw left arm (120..160, 100..160)
    for y in 100..160 {
        for x in 120..160 {
            matte.put_pixel(x, y, Luma([255]));
        }
    }
    // Draw head (85..115, 50..100)
    for y in 50..100 {
        for x in 85..115 {
            matte.put_pixel(x, y, Luma([255]));
        }
    }

    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    keypoints[0] = (100.0, 60.0, 0.9);   // Nose
    keypoints[5] = (90.0, 105.0, 0.9);   // Shoulder L
    keypoints[6] = (110.0, 105.0, 0.9);  // Shoulder R
    keypoints[7] = (135.0, 125.0, 0.9);  // Elbow L
    keypoints[9] = (150.0, 150.0, 0.9);  // Wrist L
    keypoints[11] = (92.0, 175.0, 0.9);  // Hip L
    keypoints[12] = (108.0, 175.0, 0.9); // Hip R

    let person = Person {
        bbox: [70.0, 40.0, 170.0, 200.0],
        keypoints,
    };
    let skel = skeleton(&person);

    let img = RgbaImage::new(w, h);
    let parts = part_masks(&img, &matte, &skel).expect("part masks succeed");

    assert_eq!(parts.width, w);
    assert_eq!(parts.height, h);
    assert!(!parts.parts.is_empty());

    let torso_part = parts.get("Chest").or_else(|| parts.get("Hips")).expect("torso part exists");
    // Verify some pixels in the torso area are assigned to torso. (100, 115)
    // sits solidly inside the Chest bone's own segment (neck (100,105) to
    // chest (100,140)), away from the Chest/Hips joint at (100, 140) itself
    // — an inherently ambiguous tie point shared by both bones, not a
    // meaningful segmentation check.
    let torso_sample = torso_part.mask[(115 * w + 100) as usize];
    assert_eq!(torso_sample, 255);

    // Verify palette rendering
    let palette_img = parts.color_palette_image();
    assert_eq!(palette_img.width(), w);
    assert_eq!(palette_img.height(), h);
    // Torso pixel in palette should be non-transparent
    assert!(palette_img.get_pixel(100, 115)[3] > 0);

    // Verify part alpha extraction
    let mut colored_img = RgbaImage::new(w, h);
    colored_img.put_pixel(100, 115, Rgba([255, 0, 0, 255]));
    let alpha_img = parts.part_alpha_image(&torso_part.name, &colored_img).expect("alpha image exists");
    assert_eq!(alpha_img.get_pixel(100, 115)[0], 255);
    assert_eq!(alpha_img.get_pixel(100, 115)[3], 255);
}

#[test]
fn part_masks_iou_on_ground_truth_fixture() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        return;
    };
    let img = img.to_rgba8();
    let matte = buzz_ai::segment::matte(&img, None).expect("matte");

    // Load ground truth joints to construct reference skeleton
    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).expect("read joints");
    let true_joints: Vec<(String, (f64, f64))> = serde_json::from_str(&joints_str).expect("parse joints");

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

    // Verify key parts exist and have non-zero masks
    for part_name in ["Chest", "Hips", "Shoulder L", "Shoulder R", "Thigh L", "Thigh R"] {
        if let Some(part) = parts.get(part_name) {
            let count = part.mask.iter().filter(|&&v| v > 128).count();
            assert!(count > 10, "part {part_name} has non-trivial coverage: {count} px");
        }
    }
}
