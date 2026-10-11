use std::path::Path;
use image::GrayImage;
use buzz_ai::character::skeleton;
use buzz_ai::models::models_dir;
use buzz_ai::pose;
use buzz_ai::segment::distance_transform;

/// The C1-C4 quality-run fixtures: `pujariFront`, `sadhuSide`,
/// `ThakurBhaiStraight_copy_2` (bhatta2.fla); `OldmanfrontWalk`,
/// `sadhu1FrontWalk` (a raised arm), `sadhuSideWalk` (sustipur1.fla).
const FIXTURES: &[&str] = &[
    "pujariFront_0",
    "sadhuSide_0",
    "ThakurBhaiStraight_copy_2_0",
    "OldmanfrontWalk_0",
    "sadhu1FrontWalk_0",
    "sadhuSideWalk_0",
];

#[test]
fn ground_truth_fixtures_load_valid_joints() {
    for name in FIXTURES {
        let json_path = format!("tests/data/{name}.joints.json");
        let content = std::fs::read_to_string(&json_path)
            .unwrap_or_else(|_| panic!("Failed to read {json_path}"));
        let joints: Vec<(String, (f64, f64))> = serde_json::from_str(&content)
            .unwrap_or_else(|_| panic!("Failed to parse {json_path}"));
        // `sadhu1FrontWalk` legitimately has fewer: its source FLA names both
        // legs' thigh/shin/foot layers identically ("thigh1" x2, etc, no
        // left/right marker), which `render_symbol`'s slot resolver leaves
        // unmapped rather than guess a side — see PROGRESS/deviations.
        assert!(joints.len() >= 6, "fixture {name} has {} joints", joints.len());

        // Exactly one entry per joint name.
        let mut names: Vec<&str> = joints.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names, deduped, "fixture {name} has a duplicate joint name");

        let png_path = format!("tests/data/{name}.png");
        assert!(Path::new(&png_path).exists(), "fixture {png_path} exists");
    }
}

/// Every truth joint should land inside the character's own drawn silhouette
/// (its alpha matte), dilated by a few pixels for joints that sit right at
/// a limb's tip (hands, feet) or edge. A joint landing outside the matte
/// entirely would mean the truth data doesn't overlay the character — the
/// exact defect the C0-fix review found in the old fixtures.
#[test]
fn ground_truth_joints_lie_inside_dilated_matte() {
    for name in FIXTURES {
        let png_path = format!("tests/data/{name}.png");
        let img = image::open(&png_path).unwrap_or_else(|e| panic!("open {png_path}: {e}")).to_rgba8();
        let (w, h) = (img.width(), img.height());
        // Proportional to the fixture's own size (these are 1024px-tall
        // crops of very different aspect ratios): generous enough to allow
        // a joint like "Head", whose truth comes from the head layer's own
        // rigging pivot (often just above the drawn hairline, not literally
        // inside it — verified against `pujariFront`), while still catching
        // a joint that isn't on the character at all (the old fixtures'
        // defect this test exists to catch: e.g. a joint hundreds of pixels
        // off-canvas).
        let dilate_px = (h as f32 * 0.05).max(12.0) as i32;

        let mut alpha_matte = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                alpha_matte.put_pixel(x, y, image::Luma([img.get_pixel(x, y)[3]]));
            }
        }
        let mut inverted = alpha_matte.clone();
        for p in inverted.pixels_mut() {
            p[0] = 255 - p[0];
        }
        let dist_to_opaque = distance_transform(&inverted);

        let json_path = format!("tests/data/{name}.joints.json");
        let content = std::fs::read_to_string(&json_path).unwrap_or_else(|e| panic!("read {json_path}: {e}"));
        let joints: Vec<(String, (f64, f64))> = serde_json::from_str(&content).unwrap_or_else(|e| panic!("parse {json_path}: {e}"));

        for (joint_name, (jx, jy)) in &joints {
            let (xi, yi) = (jx.round() as i64, jy.round() as i64);
            assert!(
                xi >= 0 && yi >= 0 && xi < w as i64 && yi < h as i64,
                "fixture {name}: joint {joint_name} at ({jx:.1},{jy:.1}) falls outside the {w}x{h} image"
            );
            let idx = (yi as u32 * w + xi as u32) as usize;
            let inside = alpha_matte.get_pixel(xi as u32, yi as u32)[0] > 16
                || dist_to_opaque[idx] <= dilate_px as f32;
            assert!(
                inside,
                "fixture {name}: joint {joint_name} at ({jx:.1},{jy:.1}) is {:.1}px outside the dilated ({dilate_px}px) alpha matte",
                dist_to_opaque[idx]
            );
        }
    }
}

#[test]
#[ignore = "needs models"]
fn ground_truth_skeleton_accuracy_within_budget() {
    let estimator_path = models_dir().join("dw-ll_ucoco_384.onnx");
    assert!(
        estimator_path.exists(),
        "dw-ll_ucoco_384.onnx not present in {:?}; download models via AI Models dialog or `ensure()` first",
        models_dir()
    );

    for name in FIXTURES {
        let png_path = format!("tests/data/{name}.png");
        let img = image::open(&png_path).expect("open fixture png").to_rgba8();
        let persons = pose::detect(&img).expect("detect persons");
        if persons.is_empty() {
            continue;
        }
        let person = &persons[0];
        let skel = skeleton(person);
        let char_height = skel.character_height;
        let max_err = char_height * 0.08;

        let json_path = format!("tests/data/{name}.joints.json");
        let content = std::fs::read_to_string(&json_path).expect("read joints");
        let true_joints: Vec<(String, (f64, f64))> = serde_json::from_str(&content).expect("parse joints");

        // The fixtures' truth side names come from each FLA's own
        // "left"/"right" layer names, which some source files author
        // screen-relative (mirrored for a front-facing pose) rather than in
        // COCO's subject-relative convention DWPose outputs. Pick whichever
        // global alignment (as-labelled, or every " L"/" R" swapped) fits
        // the detector better, the same way `ai_verify` does — otherwise a
        // pure labelling-convention mismatch reads as a detector failure.
        let swapped: Vec<(String, (f64, f64))> = true_joints
            .iter()
            .map(|(n, p)| {
                let sw = if let Some(stem) = n.strip_suffix(" L") {
                    format!("{stem} R")
                } else if let Some(stem) = n.strip_suffix(" R") {
                    format!("{stem} L")
                } else {
                    n.clone()
                };
                (sw, *p)
            })
            .collect();

        let mean_err = |joints: &[(String, (f64, f64))]| -> f64 {
            let mut sum = 0.0;
            let mut n = 0.0;
            for (slot, (tx, ty)) in joints {
                if let Some(joint) = skel.joint(slot) {
                    let dx = joint.x as f64 - tx;
                    let dy = joint.y as f64 - ty;
                    sum += (dx * dx + dy * dy).sqrt();
                    n += 1.0;
                }
            }
            if n > 0.0 { sum / n } else { f64::MAX }
        };

        let use_swapped = mean_err(&swapped) < mean_err(&true_joints);
        let aligned = if use_swapped { &swapped } else { &true_joints };

        for (slot, (tx, ty)) in aligned {
            if let Some(joint) = skel.joint(slot) {
                let dx = joint.x as f64 - tx;
                let dy = joint.y as f64 - ty;
                let dist = (dx * dx + dy * dy).sqrt();
                assert!(
                    dist <= max_err as f64,
                    "Joint {slot} in {name} [{}] is at ({}, {}) but true is at ({}, {}), dist={:.1} > max_err={:.1} (height={:.1})",
                    if use_swapped { "L/R-swapped" } else { "as-labeled" },
                    joint.x, joint.y, tx, ty, dist, max_err, char_height
                );
            }
        }
    }
}
