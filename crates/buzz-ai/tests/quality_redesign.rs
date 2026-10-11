//! Tests for the "Opus review of the quality run: redesign C2-C4" items:
//! item 2 (mask cleaning + Voronoi label fill), item 6 (leg silhouette-split
//! fallback), and a smoke test of items 1/3/4 (whole-character trace, split
//! by part, vector joint caps) against a real ground-truth fixture.

use buzz_ai::character::{self, Bone, FaceFeatures, Joint, Skeleton};
use buzz_ai::pose::Person;
use buzz_ai::segment;
use buzz_geom::{BezPath, PathEl, Point, Shape};
use image::{GrayImage, Luma, Rgba};

fn flatten_points(path: &BezPath) -> Vec<Point> {
    let mut out = Vec::new();
    kurbo::flatten(path.iter(), 0.5, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => out.push(p),
        PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => out.push(p),
        PathEl::ClosePath => {}
    });
    out
}

// --- item 2: mask cleaning ---

#[test]
fn clean_part_mask_denoises_speckle_and_fills_small_holes() {
    let (w, h) = (60u32, 60u32);
    let mut mask = vec![0u8; (w * h) as usize];
    // Main 20x20 blob.
    for y in 20..40u32 {
        for x in 20..40u32 {
            mask[(y * w + x) as usize] = 255;
        }
    }
    // A 2x2 interior hole (4px, well under 2% of the blob's 400px area).
    for y in 28..30u32 {
        for x in 28..30u32 {
            mask[(y * w + x) as usize] = 0;
        }
    }
    // An isolated 1px speckle far from the main blob.
    mask[(5 * w + 5) as usize] = 255;

    let cleaned = segment::clean_part_mask(&mask, w, h);
    assert_eq!(cleaned[(29 * w + 29) as usize], 255, "small interior hole must be filled");
    assert_eq!(cleaned[(5 * w + 5) as usize], 0, "isolated speckle must be removed (not the largest component)");
    assert_eq!(cleaned[(25 * w + 25) as usize], 255, "main blob must survive cleaning");
}

#[test]
fn voronoi_labels_assigns_nearest_seed_within_matte() {
    let (w, h) = (10u32, 1u32);
    let mut matte = GrayImage::new(w, h);
    for x in 0..w {
        matte.put_pixel(x, 0, Luma([255]));
    }
    let mut labels = vec![-1i32; (w * h) as usize];
    labels[0] = 0; // seed A at x=0
    labels[9] = 1; // seed B at x=9
    segment::voronoi_labels(&mut labels, &matte, w, h);
    assert_eq!(labels[2], 0, "x=2 is nearer to seed A");
    assert_eq!(labels[7], 1, "x=7 is nearer to seed B");
    assert_eq!(labels[0], 0, "seeded pixels keep their own label");
}

// --- item 6: leg silhouette-split fallback ---

fn synthetic_two_leg_skeleton(low_conf: bool) -> Skeleton {
    let c = if low_conf { 0.1 } else { 0.9 };
    let joints = vec![
        Joint { name: "Hips".into(), x: 80.0, y: 100.0, confidence: 0.9 },
        Joint { name: "Thigh L".into(), x: 60.0, y: 100.0, confidence: c },
        Joint { name: "Knee L".into(), x: 60.0, y: 170.0, confidence: c },
        Joint { name: "Ankle L".into(), x: 60.0, y: 240.0, confidence: c },
        Joint { name: "Foot L".into(), x: 60.0, y: 250.0, confidence: 0.9 },
        Joint { name: "Thigh R".into(), x: 100.0, y: 100.0, confidence: c },
        Joint { name: "Knee R".into(), x: 100.0, y: 170.0, confidence: c },
        Joint { name: "Ankle R".into(), x: 100.0, y: 240.0, confidence: c },
        Joint { name: "Foot R".into(), x: 100.0, y: 250.0, confidence: 0.9 },
    ];
    let bones = vec![
        Bone { name: "Thigh L".into(), head: (60.0, 100.0), tail: (60.0, 170.0), confidence: c, length: 70.0 },
        Bone { name: "Knee L".into(), head: (60.0, 170.0), tail: (60.0, 240.0), confidence: c, length: 70.0 },
        Bone { name: "Foot L".into(), head: (60.0, 240.0), tail: (60.0, 250.0), confidence: 0.9, length: 10.0 },
        Bone { name: "Thigh R".into(), head: (100.0, 100.0), tail: (100.0, 170.0), confidence: c, length: 70.0 },
        Bone { name: "Knee R".into(), head: (100.0, 170.0), tail: (100.0, 240.0), confidence: c, length: 70.0 },
        Bone { name: "Foot R".into(), head: (100.0, 240.0), tail: (100.0, 250.0), confidence: 0.9, length: 10.0 },
    ];
    Skeleton { joints, bones, face: FaceFeatures::default(), character_height: 250.0 }
}

fn two_leg_matte() -> GrayImage {
    let mut m = GrayImage::new(160, 260);
    for y in 100..250u32 {
        for x in 50..70u32 {
            m.put_pixel(x, y, Luma([255]));
        }
        for x in 90..110u32 {
            m.put_pixel(x, y, Luma([255]));
        }
    }
    m
}

#[test]
fn legs_need_fallback_false_when_confident_and_separated() {
    let skel = synthetic_two_leg_skeleton(false);
    assert!(!character::legs_need_fallback(&skel));
}

#[test]
fn leg_fallback_silhouette_split_recovers_two_separated_legs() {
    let mut skel = synthetic_two_leg_skeleton(true);
    assert!(character::legs_need_fallback(&skel), "low-confidence legs must need the fallback");

    let matte = two_leg_matte();
    let applied = character::leg_fallback_silhouette_split(&mut skel, &matte);
    assert!(applied, "fallback should fire on low-confidence legs");

    let knee_l = skel.joint("Knee L").expect("knee l");
    let knee_r = skel.joint("Knee R").expect("knee r");
    assert!(knee_l.x < knee_r.x, "left knee ({}) should stay left of right knee ({})", knee_l.x, knee_r.x);
    assert!((knee_l.x - 60.0).abs() < 15.0, "left knee x should track the left leg column: {}", knee_l.x);
    assert!((knee_r.x - 100.0).abs() < 15.0, "right knee x should track the right leg column: {}", knee_r.x);
    assert!(knee_l.y > 100.0 && knee_l.y < 250.0, "knee should sit between hip and foot: {}", knee_l.y);

    let foot_l = skel.joint("Foot L").expect("foot l");
    assert!(foot_l.y > knee_l.y, "foot should sit below the knee");
}

// --- items 1/3/4: whole-character trace smoke test on a real fixture ---

fn load_pujari_skeleton() -> Option<(image::RgbaImage, GrayImage, Skeleton)> {
    let png_path = "tests/data/pujariFront_0.png";
    let img = image::open(png_path).ok()?.to_rgba8();
    let matte = segment::matte(&img, None).ok()?;

    let joints_path = "tests/data/pujariFront_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).ok()?;
    let true_joints: Vec<(String, (f64, f64))> = serde_json::from_str(&joints_str).ok()?;
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
    let person = Person { bbox: [0.0, 0.0, img.width() as f32, img.height() as f32], keypoints };
    let skel = character::skeleton(&person);
    Some((img, matte, skel))
}

#[test]
fn trace_character_whole_splits_by_part_and_adds_joint_caps() {
    let Some((img, matte, skel)) = load_pujari_skeleton() else {
        return;
    };
    let parts = character::part_masks(&img, &matte, &skel).expect("part masks");
    let completed = character::complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced =
        character::trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace whole");

    assert_eq!(traced.uniform_width, 2.0);
    assert!(traced.palette.len() >= 6, "palette should have at least 6 colours, got {}", traced.palette.len());

    let mut total_shapes = 0;
    let mut parts_with_shapes = 0;
    for part in &traced.parts {
        if !part.shapes.is_empty() {
            parts_with_shapes += 1;
        }
        for shape in &part.shapes {
            total_shapes += 1;
            if let Some(stroke) = &shape.stroke {
                assert!(
                    (stroke.width - 2.0).abs() < 1e-6,
                    "stroke width must equal uniform_width in part {}",
                    part.name
                );
            }
        }
    }
    assert!(total_shapes > 0, "whole-character trace must produce shapes");
    assert!(parts_with_shapes >= 4, "several parts should carry their own vector shapes, got {parts_with_shapes}");

    // A bone-backed limb part should carry at least its base fill (item 4), and
    // its joint cap (a filled disc and the outline of its exposed arc) kept
    // apart from its shapes: the cap sits behind every part, not at the part's
    // own depth.
    let arm = traced.parts.iter().find(|p| p.name == "Elbow L").expect("Elbow L part");
    assert!(!arm.shapes.is_empty(), "Elbow L should have at least a base fill");
    assert!(
        arm.caps.iter().any(|s| s.fill.is_some() && s.stroke.is_none()),
        "Elbow L should have a joint-cap disc"
    );
    assert!(arm.caps.iter().any(|s| s.stroke.is_some()), "Elbow L should have the cap's exposed-arc outline");
    for part in &traced.parts {
        if skel.bone(&part.name).is_none() {
            assert!(part.caps.is_empty(), "{} has no bone, so no joint cap", part.name);
        }
    }
}

// --- "Opus review of the redesign" next-run items 4/5: unit tests for both
// artefacts ---

/// Same shape as `load_pujari_skeleton`, for `OldmanfrontWalk_0` (the
/// fixture the "pale disc" artefact was found on).
fn load_oldman_skeleton() -> Option<(image::RgbaImage, GrayImage, Skeleton)> {
    let png_path = "tests/data/OldmanfrontWalk_0.png";
    let img = image::open(png_path).ok()?.to_rgba8();
    let matte = segment::matte(&img, None).ok()?;

    let joints_path = "tests/data/OldmanfrontWalk_0.joints.json";
    let joints_str = std::fs::read_to_string(joints_path).ok()?;
    let true_joints: Vec<(String, (f64, f64))> = serde_json::from_str(&joints_str).ok()?;
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
    let person = Person { bbox: [0.0, 0.0, img.width() as f32, img.height() as f32], keypoints };
    let skel = character::skeleton(&person);
    Some((img, matte, skel))
}

/// Regression test for the "pale disc at OldmanfrontWalk's foot" artefact:
/// a foot part with no segmented pixels at all in this pose (fully hidden,
/// or its joint landing in the empty gap between two separated legs of a
/// walk cycle) must still get a `dom_color` grounded in the character's own
/// colours, not `dominant_color`'s generic pale-grey `[200, 200, 200]`
/// "no pixels" default — and `trace_character_whole`'s joint-cap disc and
/// close-the-cut fill for that part must use that same colour, not a
/// separately re-derived (and possibly still-empty) one.
#[test]
fn foot_dom_color_is_not_the_pale_grey_fallback_on_oldman() {
    let Some((img, matte, skel)) = load_oldman_skeleton() else {
        return;
    };
    let parts = character::part_masks(&img, &matte, &skel).expect("part masks");
    let completed = character::complete_parts(&img, &matte, &skel, &parts).expect("complete parts");

    let is_pale_grey = |c: [u8; 3]| -> bool {
        let dr = c[0] as f32 - 200.0;
        let dg = c[1] as f32 - 200.0;
        let db = c[2] as f32 - 200.0;
        (dr * dr + dg * dg + db * db).sqrt() < 25.0
    };

    // T10: the foot part is named for its bone, "Ankle L/R" (ankle to foot point).
    let mut checked_any = false;
    for name in ["Ankle L", "Ankle R"] {
        let Some(part) = completed.get(name) else { continue };
        checked_any = true;
        assert!(
            !is_pale_grey(part.dom_color),
            "{name}'s dom_color {:?} must not be the pale-grey no-pixels fallback",
            part.dom_color
        );
    }
    assert!(checked_any, "OldmanfrontWalk_0 must have at least one Foot part to check");

    let traced = character::trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace whole");
    for name in ["Ankle L", "Ankle R"] {
        let Some(part) = traced.parts.iter().find(|p| p.name == name) else { continue };
        for shape in part.shapes.iter().chain(&part.caps) {
            if let Some(fill) = &shape.fill {
                let c = fill.color().to_rgba8();
                assert!(
                    !is_pale_grey([c.r, c.g, c.b]),
                    "{name} fill {:?} must not be the pale-grey no-pixels fallback",
                    (c.r, c.g, c.b)
                );
            }
        }
    }
}

/// Regression test for the "stray dark filled band across pujariFront's
/// kurta hem" artefact's root cause: item 3's split-by-part logic used to
/// push a shape that touched only one part's *rasterised pixels* fully
/// unclipped, even when nothing else claimed the rest of that shape's much
/// larger bounding area (`owner[i] == -1`, e.g. a shape thinly connecting
/// ink lines across a wide span) — so a single part could end up with a
/// shape spanning far more of the body than that part's own silhouette.
/// Every shape assigned to a bone-backed part must stay geometrically
/// localised to that part: at least 75% of a fill's area (fills of 400 px^2 or
/// more; a region goes to its part unclipped from 92% inside, and a clipped
/// piece follows a part boundary simplified to up to ~12 px on the torso) must
/// lie on the part's own completed pixels — this used to be a bounding-box test,
/// from when every fill was clipped to the part's pixel boundary; strokes, no longer cut, are
/// bounded by diagonal instead. This used to be a fixed share of the character's height,
/// a stand-in that a part with a big honest silhouette (the kurta's Hips,
/// 42% of the height) tripped once completion stopped painting over the parts
/// beside it.
#[test]
fn traced_shapes_stay_localised_to_their_own_part_on_pujari() {
    let Some((img, matte, skel)) = load_pujari_skeleton() else {
        return;
    };
    let parts = character::part_masks(&img, &matte, &skel).expect("part masks");
    let completed = character::complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced = character::trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace whole");

    let mut checked_any = false;
    for part in &traced.parts {
        if skel.bone(&part.name).is_none() {
            continue;
        }
        let done = completed.get(&part.name).expect("completed part");
        let (mut bx0, mut by0, mut bx1, mut by1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (i, px) in done.rgba.chunks(4).enumerate() {
            if px[3] > 128 {
                let (x, y) = ((i as u32 % done.width) as f64, (i as u32 / done.width) as f64);
                bx0 = bx0.min(x);
                by0 = by0.min(y);
                bx1 = bx1.max(x + 1.0);
                by1 = by1.max(y + 1.0);
            }
        }
        for shape in &part.shapes {
            let pts = flatten_points(&shape.path);
            if pts.len() < 2 {
                continue;
            }
            let (x0, x1) = (
                pts.iter().map(|p| p.x).fold(f64::MAX, f64::min),
                pts.iter().map(|p| p.x).fold(f64::MIN, f64::max),
            );
            let (y0, y1) = (
                pts.iter().map(|p| p.y).fold(f64::MAX, f64::min),
                pts.iter().map(|p| p.y).fold(f64::MIN, f64::max),
            );
            checked_any = true;
            if shape.fill.is_some() {
                // A filled region goes to its part whole when >= 92% of it lies
                // inside that part (or it is under 400 px^2), and is otherwise
                // clipped to the part's smoothed boundary — so its bounding box
                // may overhang the part's box by the unclipped <= 8%. What must
                // hold is that (nearly) all of the fill's area lies on pixels
                // this part's completed picture covers: a region spanning other
                // parts or the empty background (the stray dark band) does not.
                let (mut inside, mut total) = (0u32, 0u32);
                for py in (y0.floor().max(0.0) as u32)..(y1.ceil() as u32).min(done.height) {
                    for px in (x0.floor().max(0.0) as u32)..(x1.ceil() as u32).min(done.width) {
                        if !shape.path.contains(Point::new(px as f64 + 0.5, py as f64 + 0.5)) {
                            continue;
                        }
                        total += 1;
                        if done.rgba[((py * done.width + px) as usize) * 4 + 3] > 128 {
                            inside += 1;
                        }
                    }
                }
                assert!(
                    total < 400 || inside as f64 >= 0.75 * total as f64,
                    "part {} has a {total}px^2 fill spanning ({x0:.0},{y0:.0})..({x1:.0},{y1:.0}) with only {inside} px on \
                     its own completed pixels (box ({bx0:.0},{by0:.0})..({bx1:.0},{by1:.0})) — likely a multi-part shape",
                    part.name
                );
            } else {
                // A stroke is cut at the part boundary but carried a little past
                // it across any gap, so only its overall reach is bounded.
                let diagonal = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
                assert!(
                    diagonal <= skel.character_height as f64 * 0.4,
                    "part {} has a stroke with bbox diagonal {diagonal:.0}px, over 40% of the character's height",
                    part.name
                );
            }
        }
    }
    assert!(checked_any, "must have checked at least one bone-backed part's shapes");
}

// --- F4c: "floating hat above the head" / "oversized detached disc near
// the foot" regressions on OldmanfrontWalk, and the reviewer's follow-up
// (F4c review): a disconnected blob must not be *deleted* — that silently
// destroys real, separately-drawn artwork (a detached hand, a held prop) —
// only kept from being glued to the wrong bone, and still shown to the user
// as its own unbound part. ---

/// `segment::matte` must drop a genuine speck (noise far under the size of
/// anything meaningful) but leave a substantial disconnected blob alone —
/// telling the two apart is `matte`'s whole job now; which bone (if any) a
/// *surviving* blob is allowed to belong to is `part_masks`' job, checked
/// separately below.
#[test]
fn matte_drops_a_tiny_speck_but_keeps_a_substantial_disconnected_blob() {
    let (w, h) = (60u32, 80u32);
    let mut img = image::RgbaImage::new(w, h);
    // The "body": a 40x40 solid block (1600px) — specks are measured
    // against this, its own largest component.
    for y in 30..70u32 {
        for x in 10..50u32 {
            img.put_pixel(x, y, Rgba([120, 90, 60, 255]));
        }
    }
    // A single-pixel speck: 1px is under 0.1% of 1600px (ceil(1.6) = 2), so
    // it must be dropped.
    img.put_pixel(5, 5, Rgba([200, 50, 50, 255]));
    // A substantial disconnected blob (36px, well over the 2px speck
    // threshold) with a clear gap to the body — real content, must survive.
    for y in 5..11u32 {
        for x in 25..31u32 {
            img.put_pixel(x, y, Rgba([90, 60, 30, 255]));
        }
    }

    let matte = segment::matte(&img, None).expect("matte");

    assert_eq!(matte.get_pixel(5, 5)[0], 0, "a 1px speck must be dropped from the matte");

    let mut body_kept = 0;
    for y in 30..70u32 {
        for x in 10..50u32 {
            if matte.get_pixel(x, y)[0] > 128 {
                body_kept += 1;
            }
        }
    }
    assert!(body_kept > 1500, "the main body silhouette (1600px) must survive intact, kept {body_kept} px");

    let mut blob_kept = 0;
    for y in 5..11u32 {
        for x in 25..31u32 {
            if matte.get_pixel(x, y)[0] > 128 {
                blob_kept += 1;
            }
        }
    }
    assert_eq!(
        blob_kept, 36,
        "a substantial disconnected blob (36px) must survive in the matte untouched, kept {blob_kept} px"
    );
}

/// Regression test for the "floating hat above the head" artefact's root
/// cause: a disconnected foreground blob in the source photo (a small
/// object that came loose mid-stride, a clear gap of background with no
/// connecting matte pixels) got absorbed into whichever bone ends up
/// nearest to it by chamfer distance — empirically "Hand R" on
/// OldmanfrontWalk, its raised-fist joint being closer to the loose object
/// than any other bone's own claimed pixels — by `part_masks`' nearest-bone
/// Voronoi gap-fill (item 5), which has no way to tell a real gap in the
/// character's own silhouette from an unrelated object that merely happens
/// to be closest.
///
/// The fix keeps the object (an earlier version deleted every disconnected
/// component outright, which would also have deleted a detached hand or a
/// held prop): `part_masks` pulls every matte component other than the
/// character's own main silhouette out of every bone/face part and returns
/// it as its own `Prop` mask (item 6), and `assemble_character` places each
/// as its own unbound layer at its own position rather than rigging it — so
/// (a) no bone/face part ever claims one of its pixels, and (b) it still
/// shows up in the assembled scene, visible and editable, as a "Prop" layer.
///
/// A real fixture (pujariFront_0) with a synthetic prop spliced into a
/// corner of the image, not a fully synthetic skeleton: `assemble_character`
/// needs a complete, self-consistent 11-bone biped skeleton to succeed, and
/// pujariFront_0's own detected one already is one.
#[test]
fn disconnected_prop_stays_out_of_every_bone_part_and_becomes_an_unbound_layer() {
    let Some((mut img, _matte, skel)) = load_pujari_skeleton() else {
        return;
    };

    // Splice a disconnected "prop" blob into the top-left corner — well
    // away from the character's own silhouette and every bone. Large enough
    // (40x40 = 1600px) to clear the 0.1% speck threshold against this
    // photo's own much larger silhouette (`matte`'s `SPECK_AREA_FRACTION`).
    let (pw, ph) = (img.width(), img.height());
    let (px0, py0, size) = (4u32, 4u32, 40u32);
    for y in py0..(py0 + size).min(ph) {
        for x in px0..(px0 + size).min(pw) {
            img.put_pixel(x, y, image::Rgba([200, 60, 60, 255]));
        }
    }
    let matte = segment::matte(&img, None).expect("matte");

    let parts = character::part_masks(&img, &matte, &skel).expect("part masks");
    assert!(!parts.props.is_empty(), "the spliced prop must show up as its own PartMask in `props`");

    for part in &parts.parts {
        for y in py0..(py0 + size).min(ph) {
            for x in px0..(px0 + size).min(pw) {
                assert_eq!(
                    part.mask[(y * parts.width + x) as usize],
                    0,
                    "part {} must not claim any pixel of the disconnected prop",
                    part.name
                );
            }
        }
    }

    let completed = character::complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced =
        character::trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace whole");
    let assembled =
        character::assemble_character(&skel, &traced, "PujariWithProp", Some((pw as f64, ph as f64)), None)
            .expect("assemble character");

    assert!(
        assembled.scene.layers().iter().any(|l| l.name.starts_with("Prop")),
        "the assembled scene must contain an unbound 'Prop' layer for the spliced prop"
    );
}

/// Regression test for the "oversized detached disc near OldmanfrontWalk's
/// left foot" artefact: `limb_radius_at_head`'s old `.min(bone.length)` cap
/// bounded *radius* at the full bone length — a no-op for an ordinary limb
/// bone (always much longer than it is wide) but not for a very short
/// terminal one: "Foot L" here is only ~21px ankle-to-toe, shorter than the
/// foot is wide, so the cap still let a disc roughly *twice* the bone's own
/// length balloon off past the (much smaller) drawn shoe, reading as
/// detached from it. A joint-cap disc bridges one joint, so its own
/// diameter must never exceed the bone it belongs to.
#[test]
fn joint_cap_discs_stay_within_half_their_bones_length_on_oldman() {
    let Some((img, matte, skel)) = load_oldman_skeleton() else {
        return;
    };
    let parts = character::part_masks(&img, &matte, &skel).expect("part masks");
    let completed = character::complete_parts(&img, &matte, &skel, &parts).expect("complete parts");
    let traced =
        character::trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace whole");

    let mut checked_any = false;
    for part in &traced.parts {
        let Some(bone) = skel.bone(&part.name) else { continue };
        let head = Point::new(bone.head.0 as f64, bone.head.1 as f64);

        // The joint-cap disc is the part's filled, unstroked cap shape: a disc
        // about the bone's own head joint, pulled in wherever the character's
        // silhouette ends sooner. Its radius is how far any point of it lies
        // from that joint.
        for shape in &part.caps {
            if shape.fill.is_none() || shape.stroke.is_some() {
                continue;
            }
            checked_any = true;
            let radius = flatten_points(&shape.path).iter().map(|p| (*p - head).hypot()).fold(0.0, f64::max);
            let max_radius = (bone.length as f64) * 0.5 + 1.0; // +1: float/flatten slack
            assert!(
                radius <= max_radius,
                "part {}'s joint-cap disc has radius {radius:.1}px, more than half its \
                 own bone's length ({:.1}px, so max {max_radius:.1}px) — an oversized cap \
                 detached from the part's own drawn artwork",
                part.name,
                bone.length
            );
        }
    }
    assert!(checked_any, "must have found at least one joint-cap disc to check");
}
