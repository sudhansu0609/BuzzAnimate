use image::{GrayImage, Luma, Rgba, RgbaImage};
use buzz_ai::character::{complete_parts, part_masks, skeleton};
use buzz_ai::pose::Person;

#[test]
fn complete_part_covers_capsule_and_joint_cap_without_holes() {
    let w = 200;
    let h = 300;
    let mut matte = GrayImage::new(w, h);
    // Fill entire canvas for matte
    for y in 0..h {
        for x in 0..w {
            matte.put_pixel(x, y, Luma([255]));
        }
    }

    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.put_pixel(x, y, Rgba([100, 150, 200, 255]));
        }
    }

    // Forearm from (100, 100) to (100, 180)
    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    keypoints[0] = (100.0, 50.0, 0.95);
    keypoints[5] = (100.0, 60.0, 0.95);  // Shoulder L
    keypoints[6] = (100.0, 60.0, 0.95);  // Shoulder R
    keypoints[7] = (100.0, 100.0, 0.95); // Elbow L (parent joint)
    keypoints[9] = (100.0, 180.0, 0.95); // Wrist L
    keypoints[11] = (95.0, 200.0, 0.95);
    keypoints[12] = (105.0, 200.0, 0.95);

    let person = Person {
        bbox: [80.0, 40.0, 120.0, 220.0],
        keypoints,
    };
    let skel = skeleton(&person);
    let parts = part_masks(&img, &matte, &skel).expect("parts");
    let completed = complete_parts(&img, &matte, &skel, &parts).expect("completed");

    let forearm = completed.get("Elbow L").expect("Elbow L forearm completed");
    let forearm_img = forearm.to_image();

    // Check that along the core of the forearm bone (100, 110..170), alpha is solid 255 (no holes)
    for y in 110..170 {
        let p = forearm_img.get_pixel(100, y);
        assert_eq!(p[3], 255, "hole found in forearm capsule at (100, {y})");
    }

    // Check that the joint cap at parent joint (100, 100) is filled with alpha 255
    let parent_p = forearm_img.get_pixel(100, 100);
    assert_eq!(parent_p[3], 255, "parent joint cap is solid at (100, 100)");

    // Test rotation ±60° about the elbow:
    // A disc centered at (100, 100) of radius 5px must be solid
    for dy in -5..=5 {
        for dx in -5..=5 {
            if dx * dx + dy * dy <= 25 {
                let px = 100 + dx;
                let py = 100 + dy;
                let p = forearm_img.get_pixel(px as u32, py as u32);
                assert_eq!(
                    p[3], 255,
                    "gap in rotating joint cap disk at ({px}, {py})"
                );
            }
        }
    }
}

#[test]
fn complete_parts_on_ground_truth_fixture() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        return;
    };
    let img = img.to_rgba8();
    let matte = buzz_ai::segment::matte(&img, None).expect("matte");

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
    let parts = part_masks(&img, &matte, &skel).expect("parts");
    let completed = complete_parts(&img, &matte, &skel, &parts).expect("completed");

    assert_eq!(completed.parts.len(), parts.parts.len());
    let mut with_pixels = 0;
    // B1c (docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md): the straight cuts at the joints
    // move pixels between a limb and its neighbour, so a part may give some of the
    // segmenter's pixels away; no visible pixel is lost from the picture: every
    // pixel a bone part owned stays opaque in the bone part that owns it now.
    let mut covered = vec![false; (img.width() * img.height()) as usize];
    for comp in &completed.parts {
        if skel.bone(&comp.name).is_some() {
            let part_img = comp.to_image();
            let opaque_pixels = part_img.pixels().filter(|p| p[3] > 128).count();
            for (i, p) in part_img.pixels().enumerate() {
                covered[i] |= p[3] > 128;
            }
            if opaque_pixels > 0 {
                with_pixels += 1;
            }
        }
    }
    for part in parts.parts.iter().filter(|p| skel.bone(&p.name).is_some()) {
        let lost = part.mask.iter().enumerate().filter(|&(i, v)| *v > 128 && !covered[i]).count();
        assert_eq!(lost, 0, "{lost} of part {}'s own pixels are lost from the picture", part.name);
    }
    assert!(with_pixels >= 8, "most bone parts have pixels, got {with_pixels}");
}
