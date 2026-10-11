//! T10: an image-built character is rigged to `RigPattern::biped_full()` — the eleven
//! standard biped bones, then Wrist L/R, Ankle L/R and Brow L/R — so hands, feet and
//! eyebrows move on their own while everything addressed by the old indices keeps working.
//!
//! Synthetic: a 600 x 1200 figure with rectangles for parts, so nothing here needs a model.

use buzz_ai::character::{assemble_character, skeleton, TracedCharacter, TracedPart};
use buzz_ai::motion::PoseLibrary;
use buzz_ai::pose::Person;
use buzz_rig::RigPattern;
use buzz_scene::{ArmatureData, ObjectId, ObjectKind, RigBinding, ShapeData};
use kurbo::{Rect, Shape as _};
use peniko::Color;

const SKIN: Color = Color::from_rgb8(232, 180, 150);
const SHIRT: Color = Color::from_rgb8(60, 90, 160);
const BROW: Color = Color::from_rgb8(70, 45, 30);

fn figure_person() -> Person {
    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (slot, (x, y)) in [
        (0, (300.0, 100.0)), // nose
        (5, (340.0, 300.0)),
        (6, (260.0, 300.0)),
        (7, (400.0, 450.0)),
        (8, (200.0, 450.0)),
        (9, (440.0, 600.0)), // wrist L
        (10, (160.0, 600.0)),
        (11, (320.0, 700.0)),
        (12, (280.0, 700.0)),
        (13, (320.0, 950.0)),
        (14, (280.0, 950.0)),
        (15, (320.0, 1150.0)), // ankle L
        (16, (280.0, 1150.0)),
    ] {
        keypoints[slot] = (x, y, 0.95);
    }
    // Eyebrows (COCO-WholeBody 40..45 left, 45..50 right), five points each, 24 px wide,
    // at y 63, either side of the nose (x 300): the left brow's inner end is x 282.
    for (first, centre) in [(40usize, 270.0f32), (45, 330.0)] {
        for i in 0..5 {
            keypoints[first + i] = (centre - 12.0 + i as f32 * 6.0, 63.0, 0.9);
        }
    }
    Person { bbox: [0.0, 0.0, 600.0, 1200.0], keypoints }
}

fn part(name: &str, depth: i32, rect: Rect, colour: Color) -> TracedPart {
    TracedPart {
        name: name.into(),
        depth,
        shapes: vec![ShapeData::filled(rect.to_path(0.1), colour)],
        ink: Vec::new(),
        caps: Vec::new(),
    }
}

fn figure_parts() -> TracedCharacter {
    TracedCharacter {
        parts: vec![
            part("Hips", 70, Rect::new(260.0, 500.0, 340.0, 700.0), SHIRT),
            part("Chest", 72, Rect::new(250.0, 200.0, 350.0, 500.0), SHIRT),
            part("Head", 90, Rect::new(230.0, 20.0, 370.0, 140.0), SKIN),
            part("Shoulder L", 80, Rect::new(335.0, 300.0, 405.0, 450.0), SHIRT),
            part("Elbow L", 80, Rect::new(395.0, 450.0, 445.0, 600.0), SKIN),
            part("Wrist L", 80, Rect::new(430.0, 600.0, 470.0, 690.0), SKIN),
            part("Knee L", 60, Rect::new(300.0, 700.0, 340.0, 1000.0), SHIRT),
            part("Ankle L", 60, Rect::new(300.0, 1150.0, 340.0, 1200.0), SKIN),
            part("Left Eyebrow", 100, Rect::new(258.0, 61.0, 282.0, 65.0), BROW),
            part("Right Eyebrow", 100, Rect::new(318.0, 61.0, 342.0, 65.0), BROW),
        ],
        palette: vec![BROW, SKIN, SHIRT],
        uniform_width: 2.0,
    }
}

fn assembled_rig() -> (ArmatureData, buzz_ai::character::AssembledCharacter) {
    let skel = skeleton(&figure_person());
    let assembled = assemble_character(&skel, &figure_parts(), "Full", Some((600.0, 1200.0)), None).expect("assemble");
    (assembled.armature.clone(), assembled)
}

/// `(bone, posed bounds)` of every rigidly bound part, in bind order.
fn posed_bounds(rig: &ArmatureData) -> Vec<(usize, Rect)> {
    rig.parts
        .iter()
        .zip(rig.posed())
        .filter_map(|(p, posed)| match p.binding {
            RigBinding::Rigid(bone) => Some((bone, posed.bounds())),
            _ => None,
        })
        .collect()
}

fn same(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < 1e-6 && (a.y0 - b.y0).abs() < 1e-6 && (a.x1 - b.x1).abs() < 1e-6 && (a.y1 - b.y1).abs() < 1e-6
}

/// Pose `rig` with `bone` turned by `radians` from where it is.
fn turned(rig: &ArmatureData, bone: usize, radians: f64) -> ArmatureData {
    let mut out = rig.clone();
    let mut angles = out.armature.pose();
    angles[bone] += radians;
    out.armature.set_pose(&angles);
    out
}

#[test]
fn a_character_assembles_to_seventeen_bones_with_the_standard_eleven_first() {
    let (rig, _) = assembled_rig();
    let biped = RigPattern::biped();
    let full = RigPattern::biped_full();
    assert_eq!(rig.pattern.as_deref(), Some("Biped Full"));
    assert_eq!(rig.armature.len(), 17);
    for (i, slot) in biped.slots.iter().enumerate() {
        let bone = &rig.armature.bones[i];
        assert_eq!(bone.name, slot.name, "bone {i} keeps its standard name");
        assert_eq!(bone.parent, slot.parent, "bone {i} keeps its standard parent");
    }
    let tail: Vec<(&str, Option<usize>)> =
        rig.armature.bones[11..].iter().map(|b| (b.name.as_str(), b.parent)).collect();
    assert_eq!(
        tail,
        [
            ("Wrist L", Some(4)),
            ("Wrist R", Some(6)),
            ("Ankle L", Some(8)),
            ("Ankle R", Some(10)),
            ("Brow L", Some(2)),
            ("Brow R", Some(2)),
        ]
    );
    for (bone, slot) in rig.armature.bones.iter().zip(&full.slots) {
        assert_eq!((bone.name.as_str(), bone.parent), (slot.name.as_str(), slot.parent));
    }
}

#[test]
fn the_hand_the_foot_and_the_brows_have_their_own_bones_at_their_own_joints() {
    let (rig, _) = assembled_rig();
    let skel = skeleton(&figure_person());
    let at = |i: usize| rig.armature.head(i);
    // The forearm ends at the wrist, and the hand's bone starts there and ends at the hand point.
    let wrist = skel.joint("Wrist L").unwrap();
    let hand = skel.joint("Hand L").unwrap();
    assert!((at(11).x - wrist.x as f64).abs() < 1e-6 && (at(11).y - wrist.y as f64).abs() < 1e-6, "Wrist L starts at the wrist");
    let wrist_tip = rig.armature.tip(4);
    assert!((wrist_tip.x - wrist.x as f64).abs() < 1e-6 && (wrist_tip.y - wrist.y as f64).abs() < 1e-6, "Elbow L ends at the wrist");
    let hand_tip = rig.armature.tip(11);
    assert!((hand_tip.x - hand.x as f64).abs() < 1e-6 && (hand_tip.y - hand.y as f64).abs() < 1e-6, "Wrist L ends at the hand point");
    // The shin ends at the ankle, the foot's bone runs ankle to foot point.
    let ankle = skel.joint("Ankle L").unwrap();
    let foot = skel.joint("Foot L").unwrap();
    assert!((rig.armature.tip(8).x - ankle.x as f64).abs() < 1e-6 && (rig.armature.tip(8).y - ankle.y as f64).abs() < 1e-6);
    assert!((at(13).x - ankle.x as f64).abs() < 1e-6 && (at(13).y - ankle.y as f64).abs() < 1e-6);
    assert!((rig.armature.tip(13).y - foot.y as f64).abs() < 1e-6);
    // A brow bone runs from the inner end (nearest the nose, x 300) to the outer end.
    assert!((at(15).x - 282.0).abs() < 1e-6 && (at(15).y - 63.0).abs() < 1e-6, "Brow L starts at its inner end: {:?}", at(15));
    assert!((rig.armature.tip(15).x - 258.0).abs() < 1e-6, "and ends at the outer end: {:?}", rig.armature.tip(15));
    assert!((at(16).x - 318.0).abs() < 1e-6, "Brow R starts at its inner end: {:?}", at(16));
    assert!((rig.armature.tip(16).x - 342.0).abs() < 1e-6);
}

#[test]
fn each_part_rides_the_bone_of_its_own_slot() {
    let (rig, _) = assembled_rig();
    let bones: Vec<usize> = rig
        .parts
        .iter()
        .filter_map(|p| match p.binding {
            RigBinding::Rigid(b) => Some(b),
            _ => None,
        })
        .collect();
    for want in [0, 1, 2, 3, 4, 8, 11, 13, 15, 16] {
        assert!(bones.contains(&want), "a part is bound to bone {want}: {bones:?}");
    }
    // Each of the four new bones that has a part holds exactly one (no caps in this fixture).
    for bone in [11, 13, 15, 16] {
        assert_eq!(bones.iter().filter(|&&b| b == bone).count(), 1, "bone {bone}");
    }
}

#[test]
fn turning_the_wrist_moves_the_hand_and_not_the_forearm() {
    let (rig, _) = assembled_rig();
    let before = posed_bounds(&rig);
    let after = posed_bounds(&turned(&rig, 11, 30f64.to_radians()));
    assert_eq!(before.len(), after.len());
    let mut moved_hand = false;
    for ((bone, b), (_, a)) in before.iter().zip(&after) {
        if *bone == 11 {
            assert!(!same(*b, *a), "the hand part's posed bounds change when Wrist L turns");
            moved_hand = true;
        } else {
            assert!(same(*b, *a), "a part on bone {bone} must not move when only Wrist L turns");
        }
    }
    assert!(moved_hand);

    // The hand is the forearm's child: turning the elbow carries it along.
    let elbow = posed_bounds(&turned(&rig, 4, 20f64.to_radians()));
    for ((bone, b), (_, a)) in before.iter().zip(&elbow) {
        if *bone == 4 || *bone == 11 {
            assert!(!same(*b, *a), "bone {bone} follows the elbow");
        }
    }
}

#[test]
fn turning_a_brow_moves_only_the_brow_shapes() {
    let (rig, _) = assembled_rig();
    let before = posed_bounds(&rig);
    let after = posed_bounds(&turned(&rig, 15, 30f64.to_radians()));
    let mut moved = 0;
    for ((bone, b), (_, a)) in before.iter().zip(&after) {
        if *bone == 15 {
            assert!(!same(*b, *a), "the left brow turns");
            moved += 1;
        } else {
            assert!(same(*b, *a), "a part on bone {bone} must not move when only Brow L turns");
        }
    }
    assert_eq!(moved, 1);
}

#[test]
fn the_standard_eleven_angle_poses_still_apply_to_the_seventeen_bone_rig() {
    let (rig, _) = assembled_rig();
    let library = PoseLibrary::standard();
    for pose in &library.poses {
        assert_eq!(pose.angles.len(), 11, "the library still speaks eleven bones ({})", pose.name);
    }
    assert_eq!(rig.poses.len(), library.poses.len());
    for named in &rig.poses {
        assert_eq!(named.angles.len(), 17, "{} is stored for every bone", named.name);
    }
    let rest = rig.armature.at_rest().pose();
    for pose in &library.poses {
        let mut posed = rig.clone();
        assert!(library.apply_to_armature(&pose.name, &mut posed), "{} applies", pose.name);
        let angles = posed.armature.pose();
        assert_eq!(angles.len(), 17);
        for i in 11..17 {
            assert!(
                (angles[i] - rest[i]).abs() < 1e-9,
                "{}: bone {i} (wrist, ankle, brow) keeps its rest angle",
                pose.name
            );
        }
    }
    // And the wave pose really moves the right arm.
    let mut waved = rig.clone();
    assert!(library.apply_to_armature("wave", &mut waved));
    let angles = waved.armature.pose();
    assert!((angles[5] - rest[5]).abs() > 0.5 && (angles[6] - rest[6]).abs() > 0.1, "the wave raises the right arm");
}

#[test]
fn a_seventeen_bone_rig_survives_save_and_load() {
    let (rig, assembled) = assembled_rig();
    let bytes = buzz_doc::format::to_bytes(&assembled.scene).expect("save");
    let loaded = buzz_doc::format::from_bytes(&bytes).expect("load");
    let (_, object) = loaded.find_object(ObjectId(1)).expect("the armature object");
    let ObjectKind::Armature(back) = &object.kind else { panic!("the object is still an armature") };

    assert_eq!(back.pattern.as_deref(), Some("Biped Full"));
    assert_eq!(back.armature.len(), 17);
    for (a, b) in rig.armature.bones.iter().zip(&back.armature.bones) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.parent, b.parent);
        assert!((a.length - b.length).abs() < 1e-6, "{} length", a.name);
        assert!((a.rest_angle - b.rest_angle).abs() < 1e-9, "{} rest angle", a.name);
        assert!((a.offset.x - b.offset.x).abs() < 1e-6 && (a.offset.y - b.offset.y).abs() < 1e-6, "{} offset", a.name);
    }
    assert_eq!(back.parts.len(), rig.parts.len());
    for (a, b) in rig.parts.iter().zip(&back.parts) {
        assert_eq!(a.binding, b.binding, "bindings survive");
    }
    assert_eq!(back.poses.len(), rig.poses.len());
    for (a, b) in rig.poses.iter().zip(&back.poses) {
        assert_eq!(a.angles.len(), 17);
        assert_eq!(b.angles.len(), 17, "{} keeps all seventeen angles", a.name);
    }
    // The Rigging panel finds the pattern again by the name the rig carries.
    let pattern = RigPattern::named("Biped Full").expect("registered");
    assert_eq!(pattern.slots.len(), back.armature.len());
}

/// The matte of the figure: one connected body, an arm out to the left (image right)
/// that runs straight on past the wrist into a hand, and two straight legs.
fn figure_matte(w: u32, h: u32) -> image::GrayImage {
    let mut matte = image::GrayImage::new(w, h);
    for (x0, y0, x1, y1) in [
        (230u32, 20u32, 370u32, 700u32), // head, chest, hips
        (335, 300, 405, 450),            // upper arm
        (395, 450, 445, 600),            // forearm
        (430, 600, 470, 690),            // hand
        (260, 700, 340, 1200),           // both legs, ankles and feet
    ] {
        for y in y0..y1 {
            for x in x0..x1 {
                matte.put_pixel(x, y, image::Luma([255]));
            }
        }
    }
    matte
}

/// Which part's mask holds the pixel.
fn owner_of(parts: &buzz_ai::character::PartMasks, x: u32, y: u32) -> Option<&str> {
    let i = (y * parts.width + x) as usize;
    parts.parts.iter().find(|p| p.mask[i] > 128).map(|p| p.name.as_str())
}

#[test]
fn the_hand_stops_at_the_wrist_line_and_the_foot_at_the_ankle_line() {
    let skel = skeleton(&figure_person());
    let (w, h) = (600u32, 1200u32);
    let parts =
        buzz_ai::character::part_masks(&image::RgbaImage::new(w, h), &figure_matte(w, h), &skel).expect("part masks");

    // Beyond the wrist line the pixel is the hand, short of it the forearm.
    assert_eq!(owner_of(&parts, 450, 660), Some("Wrist L"));
    assert_eq!(owner_of(&parts, 420, 570), Some("Elbow L"));
    // The same at the ankle: below the line is the foot, above it the shin.
    assert_eq!(owner_of(&parts, 320, 1180), Some("Ankle L"));
    assert_eq!(owner_of(&parts, 320, 1140), Some("Knee L"));

    // And it is exact: no pixel of the hand is short of the wrist line and no pixel of the
    // forearm is past it (the line is perpendicular to the forearm bone at the wrist).
    for (near, far, from, to) in [("Elbow L", "Wrist L", "Elbow L", "Wrist L"), ("Knee L", "Ankle L", "Knee L", "Ankle L")] {
        let (a, b) = (skel.joint(from).unwrap(), skel.joint(to).unwrap());
        let (ux, uy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
        let len = ux.hypot(uy);
        let (ux, uy) = (ux / len, uy / len);
        let along = |x: u32, y: u32| (x as f64 - b.x as f64) * ux + (y as f64 - b.y as f64) * uy;
        let (mut hand, mut forearm) = (0, 0);
        for y in 0..h {
            for x in 0..w {
                match owner_of(&parts, x, y) {
                    Some(n) if n == far => {
                        hand += 1;
                        assert!(along(x, y) >= 0.0, "{far} pixel ({x}, {y}) is short of the line ({})", along(x, y));
                    }
                    Some(n) if n == near => {
                        forearm += 1;
                        assert!(along(x, y) < 0.0, "{near} pixel ({x}, {y}) is past the line ({})", along(x, y));
                    }
                    _ => {}
                }
            }
        }
        assert!(hand > 100 && forearm > 100, "{near}: {forearm} px, {far}: {hand} px");
    }
}

/// Needs the models and the Raju picture (`cargo test --release -p buzz-ai --test biped_full
/// -- --ignored --nocapture`): builds Raju's rig the way the wizard does, prints the bone names,
/// which part rides which bone, and where the brow parts sit against the brow landmarks.
#[test]
#[ignore]
fn raju_rig_has_seventeen_bones_with_models() {
    let path = "B:/youtubeProjects/Originals/series/ep1/raju/Friendly Indian Office-Goer Character.png";
    let Ok(img) = image::open(path) else {
        eprintln!("no Raju picture, skipping");
        return;
    };
    let img = img.to_rgba8();
    let persons = match buzz_ai::pose::detect(&img) {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprintln!("models not available, skipping");
            return;
        }
    };
    let matte = buzz_ai::segment::matte(&img, None).expect("matte");
    let skel = skeleton(&persons[0]);
    let parts = buzz_ai::character::part_masks(&img, &matte, &skel).expect("part masks");
    let completed = buzz_ai::character::complete_parts(&img, &matte, &skel, &parts).expect("complete");
    let traced = buzz_ai::character::trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace");
    let assembled = assemble_character(&skel, &traced, "Raju", Some((img.width() as f64, img.height() as f64)), None).expect("assemble");
    let names: Vec<&str> = assembled.armature.armature.bones.iter().map(|b| b.name.as_str()).collect();
    eprintln!("BONES {names:?}");
    assert_eq!(names.len(), 17);
    for (i, part) in traced.parts.iter().enumerate() {
        let mut bounds: Option<Rect> = None;
        for shape in part.shapes.iter().chain(&part.ink) {
            let b = shape.path.bounding_box();
            bounds = Some(bounds.map_or(b, |u| u.union(b)));
        }
        eprintln!("PART {i} {:?}: {} shapes, {} ink, bounds {:?}", part.name, part.shapes.len(), part.ink.len(), bounds);
    }
    for (n, pts) in [("left brow", &skel.face.left_brow), ("right brow", &skel.face.right_brow)] {
        eprintln!("LANDMARKS {n}: {pts:?}");
    }
    for slot in 11..17 {
        eprintln!("BONE {slot} {}: head {:?} tip {:?}", names[slot], assembled.armature.armature.head(slot), assembled.armature.armature.tip(slot));
    }
}
