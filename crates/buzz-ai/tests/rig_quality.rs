//! Regression tests for the picture -> character rig quality defects found on the
//! series rigs (`docs/SERIES_EP1_PLAN.md`, Q1-Q4), against the real pujariFront
//! fixture:
//!
//! - **Q1** pale discs on chest/hips/knees: the part completion must only fill
//!   where a part is hidden under another that paints over it, and joint caps sit
//!   behind every part, inside the silhouette.
//! - **Q3** the face rig's mouth must hold Rest, not loop the viseme symbol.
//! - **Q4** an eye part owns the eye, not the temple around it (the white wedges
//!   beside each eye).
//!
//! (Q2, small-region merging, is tested beside `clean_composite_regions`.)

use buzz_ai::character::{self, CompletedParts, PartMasks, Skeleton, TracedCharacter};
use buzz_ai::pose::Person;
use buzz_ai::segment;
use buzz_geom::{BezPath, PathEl, Point};
use image::{GrayImage, RgbaImage};

const FACE_POINTS: usize = 6;

/// pujariFront with its true joints as the detection, plus (optionally) an eye
/// contour either side of the head, which the fixture's joints file lacks.
fn load(with_eyes: bool, with_mouth: bool) -> Option<(RgbaImage, GrayImage, Skeleton)> {
    let img = image::open("tests/data/pujariFront_0.png").ok()?.to_rgba8();
    let matte = segment::matte(&img, None).ok()?;
    let joints: Vec<(String, (f64, f64))> =
        serde_json::from_str(&std::fs::read_to_string("tests/data/pujariFront_0.joints.json").ok()?).ok()?;
    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    let mut head = (0.0f32, 0.0f32);
    for (name, (x, y)) in &joints {
        let (xf, yf) = (*x as f32, *y as f32);
        let slot = match name.as_str() {
            "Head" => {
                head = (xf, yf);
                0
            }
            "Shoulder L" => 5,
            "Shoulder R" => 6,
            "Elbow L" => 7,
            "Elbow R" => 8,
            "Hand L" => 9,
            "Hand R" => 10,
            "Thigh L" => 11,
            "Thigh R" => 12,
            "Knee L" => 13,
            "Knee R" => 14,
            "Foot L" => 15,
            "Foot R" => 16,
            _ => continue,
        };
        keypoints[slot] = (xf, yf, 0.95);
    }
    if with_eyes {
        // COCO-WholeBody face points: left eye 59..65, right eye 65..71. A flat
        // eye contour 24 px wide, 5 px tall, on either side of the head joint.
        for i in 0..FACE_POINTS {
            let t = i as f32 / (FACE_POINTS - 1) as f32;
            let (dx, dy) = (t * 24.0, ((t - 0.5).abs() * -8.0) + 4.0);
            keypoints[59 + i] = (head.0 - 40.0 + dx, head.1 + 10.0 + dy, 0.9);
            keypoints[65 + i] = (head.0 + 16.0 + dx, head.1 + 10.0 + dy, 0.9);
        }
    }
    if with_mouth {
        // COCO-WholeBody face points 71..91: a 20-point mouth contour, 30 px wide and
        // 6 px tall, 40 px below the head joint.
        for i in 0..20 {
            let t = i as f32 / 19.0;
            keypoints[71 + i] = (head.0 - 15.0 + t * 30.0, head.1 + 40.0 + ((t - 0.5).abs() * 6.0), 0.9);
        }
    }
    let person = Person { bbox: [0.0, 0.0, img.width() as f32, img.height() as f32], keypoints };
    let skel = character::skeleton(&person);
    Some((img, matte, skel))
}

fn stages(img: &RgbaImage, matte: &GrayImage, skel: &Skeleton) -> (PartMasks, CompletedParts, TracedCharacter) {
    let parts = character::part_masks(img, matte, skel).expect("part masks");
    let completed = character::complete_parts(img, matte, skel, &parts).expect("complete parts");
    let traced = character::trace_character_whole(&completed, skel, matte, Some(2.0)).expect("trace");
    (parts, completed, traced)
}

fn flatten_points(path: &BezPath) -> Vec<Point> {
    let mut out = Vec::new();
    kurbo::flatten(path.iter(), 0.5, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => out.push(p),
        PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => out.push(p),
        PathEl::ClosePath => {}
    });
    out
}

/// **Q1.** A part's completion may only fill a pixel that another part owns if
/// that part paints over it (a higher depth, or the same depth and later in the
/// list): then the fill is hidden. Filling a pixel a lower part owns paints over
/// that part's real artwork in the completing part's flat colour: the pale discs.
#[test]
fn completion_never_fills_a_pixel_owned_by_a_part_it_would_paint_over() {
    let Some((img, matte, skel)) = load(false, false) else { return };
    let parts = character::part_masks(&img, &matte, &skel).expect("part masks");
    let completed = character::complete_parts(&img, &matte, &skel, &parts).expect("complete parts");

    let ordered: Vec<&character::PartMask> = parts.parts.iter().chain(parts.props.iter()).collect();
    for (position, done) in completed.parts.iter().enumerate() {
        assert_eq!(ordered[position].name, done.name, "completed parts are in the same order as the masks");
    }
    // B1b / B1c (docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md): the straight cuts at the
    // joints move pixels between a limb and its neighbour, and a borrowing chain is
    // moved behind its counterpart, so who owns a pixel and who paints over whom
    // are the completed parts' (`depth`, list position), not the raw masks'. What
    // must hold is what this test is for: no completion lies over visible artwork.
    // At rest, the topmost completed part at every pixel a part owned shows the
    // source pixel itself.
    let src = img.as_raw();
    let mut hidden_fills = 0usize;
    for i in 0..(completed.width * completed.height) as usize {
        if src[i * 4 + 3] <= 128 || !ordered.iter().any(|m| m.mask[i] > 128) {
            continue;
        }
        let painting: Vec<usize> =
            (0..completed.parts.len()).filter(|&p| completed.parts[p].rgba[i * 4 + 3] > 128).collect();
        let top = painting.iter().copied().max_by_key(|&p| (completed.parts[p].depth, p));
        let Some(top) = top else { panic!("pixel {i} is owned but no completed part paints it") };
        assert_eq!(
            &completed.parts[top].rgba[i * 4..i * 4 + 4],
            &src[i * 4..i * 4 + 4],
            "{} (depth {}) shows a completion at pixel {i}, over the visible artwork there",
            completed.parts[top].name,
            completed.parts[top].depth
        );
        hidden_fills += painting.len() - 1;
    }
    assert!(hidden_fills > 0, "the fixture must have some genuinely hidden completion for this to mean anything");
}

/// **Q1.** Joint caps are rig parts at the very back, bound to their own bone,
/// and every point of every cap lies inside the character's silhouette (so a cap
/// can never show as a bump or a pale patch outside the drawing); no cap is left
/// among a part's own shapes.
#[test]
fn joint_caps_sit_behind_every_part_and_inside_the_silhouette() {
    let Some((img, matte, skel)) = load(false, false) else { return };
    let (_, _, traced) = stages(&img, &matte, &skel);
    let assembled =
        character::assemble_character(&skel, &traced, "Pujari", Some((img.width() as f64, img.height() as f64)), None)
            .expect("assemble");

    let capped: Vec<&character::TracedPart> = traced.parts.iter().filter(|p| !p.caps.is_empty()).collect();
    assert!(capped.len() >= 8, "most bone parts have a cap, got {}", capped.len());

    // The first rig parts are the caps (ids 36000..37000), then nothing but other parts.
    let is_cap = |p: &buzz_scene::RigPart| (36000..37000).contains(&p.artwork.id.0);
    let rig = &assembled.armature.parts;
    let cap_count = rig.iter().filter(|p| is_cap(p)).count();
    assert_eq!(cap_count, capped.len(), "one rig part per capped traced part");
    assert!(rig[..cap_count].iter().all(is_cap), "every cap is behind every other part in the paint order");
    assert!(rig[cap_count..].iter().all(|p| !is_cap(p)));
    for cap in &rig[..cap_count] {
        assert!(matches!(cap.binding, buzz_scene::RigBinding::Rigid(_)), "a cap turns with its own bone");
    }

    // Inside the silhouette (within a pixel of it).
    let inside = |p: Point| {
        let (cx, cy) = (p.x.round() as i64, p.y.round() as i64);
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                let (x, y) = (cx + dx, cy + dy);
                x >= 0
                    && y >= 0
                    && (x as u32) < matte.width()
                    && (y as u32) < matte.height()
                    && matte.get_pixel(x as u32, y as u32)[0] > 128
            })
        })
    };
    for part in &capped {
        for shape in &part.caps {
            for p in flatten_points(&shape.path) {
                assert!(inside(p), "{}'s cap has a point ({:.1}, {:.1}) outside the silhouette", part.name, p.x, p.y);
            }
        }
    }

    // And a cap is never a pale or background colour of its own: its fill is one of
    // the character's palette colours, the colour of the part around its joint.
    for part in &capped {
        for shape in part.caps.iter().filter_map(|s| s.fill.as_ref()) {
            let c = shape.color().to_rgba8();
            assert!(
                traced.palette.iter().any(|p| p.to_rgba8() == c),
                "{}'s cap colour {c:?} is not a palette colour",
                part.name
            );
        }
    }
}

/// **Q3.** The face rig's mouth instance holds frame 0 (Rest); it must not loop
/// the ten-frame viseme symbol, which flickers on any timeline.
#[test]
fn the_mouth_part_holds_rest_instead_of_looping_the_visemes() {
    let Some((img, matte, skel)) = load(false, false) else { return };
    let (_, _, traced) = stages(&img, &matte, &skel);
    let assembled =
        character::assemble_character(&skel, &traced, "Pujari", Some((img.width() as f64, img.height() as f64)), None)
            .expect("assemble");
    let mouth = assembled.face.as_ref().expect("a face rig").mouth.clone();
    assert!(mouth.length() > 1, "the mouth symbol has the viseme frames");

    let instances: Vec<&buzz_scene::SymbolInstance> = assembled
        .armature
        .parts
        .iter()
        .filter_map(|p| match &p.artwork.kind {
            buzz_scene::ObjectKind::Instance(inst) if inst.symbol == mouth.id => Some(inst),
            _ => None,
        })
        .collect();
    assert_eq!(instances.len(), 1, "the rig holds exactly one mouth instance");
    assert_eq!(instances[0].loop_mode, buzz_scene::LoopMode::SingleFrame, "the mouth must not loop");
    assert_eq!(instances[0].first_frame, 0, "the mouth holds Rest");
}

/// **Q4.** An eye part owns the eye, not the hair and skin beside it: the drawn eye
/// is dropped for the synthetic one, so any other pixel the eye part owned was left
/// with no paint (a white wedge by each eye). No pixel of an eye part may lie
/// farther from its eye's centre than a little over one eye's width.
///
/// A synthetic standing figure on a 600 x 1200 matte (so an eye's search discs are
/// 30 px, far wider than the 12 px eye) with eye contours placed clear of every bone,
/// where the part can win its whole disc.
#[test]
fn an_eye_part_owns_the_eye_and_not_the_temple_around_it() {
    let (w, h) = (600u32, 1200u32);
    let matte = GrayImage::from_pixel(w, h, image::Luma([255]));
    let img = RgbaImage::from_pixel(w, h, image::Rgba([120, 150, 200, 255]));
    let mut keypoints = [(0.0f32, 0.0f32, 0.0f32); 133];
    for (slot, (x, y)) in [
        (0, (300.0, 100.0)),
        (5, (340.0, 300.0)),
        (6, (260.0, 300.0)),
        (7, (400.0, 450.0)),
        (8, (200.0, 450.0)),
        (9, (440.0, 600.0)),
        (10, (160.0, 600.0)),
        (11, (320.0, 700.0)),
        (12, (280.0, 700.0)),
        (13, (320.0, 950.0)),
        (14, (280.0, 950.0)),
        (15, (320.0, 1150.0)),
        (16, (280.0, 1150.0)),
    ] {
        keypoints[slot] = (x, y, 0.95);
    }
    // Eye contours (59..65 left, 65..71 right) at the far right and left, 12 px wide, 4 px tall.
    for i in 0..6 {
        let t = i as f32 / 5.0;
        keypoints[59 + i] = (500.0 + t * 12.0, 800.0 + (t - 0.5).abs() * 4.0, 0.9);
        keypoints[65 + i] = (80.0 + t * 12.0, 800.0 + (t - 0.5).abs() * 4.0, 0.9);
    }
    let skel = character::skeleton(&Person { bbox: [0.0, 0.0, w as f32, h as f32], keypoints });
    assert!(skel.face.left_eye.len() >= 2 && skel.face.right_eye.len() >= 2, "the eye contours were set");
    let parts = character::part_masks(&img, &matte, &skel).expect("part masks");

    for (name, points) in [("Left Eye", &skel.face.left_eye), ("Right Eye", &skel.face.right_eye)] {
        let part = parts.get(name).unwrap_or_else(|| panic!("{name} part"));
        let x0 = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        let x1 = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        let y0 = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let y1 = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let reach = 1.1 * (x1 - x0).max(y1 - y0) + 4.0;
        let mut owned = 0usize;
        let mut farthest = 0.0f32;
        for (i, &v) in part.mask.iter().enumerate() {
            if v > 128 {
                owned += 1;
                let (x, y) = ((i as u32 % parts.width) as f32, (i as u32 / parts.width) as f32);
                farthest = farthest.max(((x - cx).powi(2) + (y - cy).powi(2)).sqrt());
            }
        }
        assert!(owned > 100, "{name} must own its eye ({owned} px) for this to mean anything");
        // The morphological clean-up of the mask may grow it by a pixel or two.
        assert!(
            farthest <= reach + 3.0,
            "{name} owns a pixel {farthest:.0} px from its centre; its reach is {reach:.0} px ({owned} px owned)"
        );
    }
}

/// **Q3/Q4 (mouth size).** The mouth symbol's Rest frame is the mouth, not the whole
/// patch of face the traced mouth part covers: every shape on it lies within the
/// detected mouth landmarks' box (a little padded), so everything scaled to it (the
/// placeholder visemes, a mouth chart) is a mouth's width and not a face's. What the
/// part drew beyond the mouth is kept, bound to the head under the mouth, so the
/// patch round the mouth stays painted.
///
/// Built by hand (a mouth part holding the base fill and a skin fill across a 110 x 70
/// px patch, and lips, an inside and a line within the 30 px mouth) because no
/// fixture's mouth part owns pixels where a real mouth would.
#[test]
fn the_mouth_symbol_is_the_width_of_the_mouth_and_not_of_the_patch_of_face_round_it() {
    use buzz_scene::ShapeData;
    use kurbo::Shape as _;

    let Some((_, _, skel)) = load(false, true) else { return };
    assert!(skel.face.mouth.len() >= 3, "the mouth contour was set");
    let (x0, x1) = (
        skel.face.mouth.iter().map(|p| p.0).fold(f32::MAX, f32::min) as f64,
        skel.face.mouth.iter().map(|p| p.0).fold(f32::MIN, f32::max) as f64,
    );
    let (y0, y1) = (
        skel.face.mouth.iter().map(|p| p.1).fold(f32::MAX, f32::min) as f64,
        skel.face.mouth.iter().map(|p| p.1).fold(f32::MIN, f32::max) as f64,
    );
    let centre = Point::new((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let rect = |w: f64, h: f64| kurbo::Rect::from_center_size(centre, (w, h)).to_path(0.1);
    let skin = peniko::Color::from_rgb8(200, 140, 100);
    let ink = peniko::Color::from_rgb8(30, 20, 15);
    let mouth = character::TracedPart {
        name: "Mouth".into(),
        depth: 100,
        shapes: vec![
            ShapeData::filled(rect(110.0, 70.0), skin),          // the base fill
            ShapeData::filled(rect(104.0, 60.0), skin),          // the skin, clipped to the mask
            ShapeData::filled(rect(24.0, 6.0), ink),             // the inside of the mouth
            ShapeData::stroked(rect(28.0, 8.0), ink, 2.0),       // the line round it
        ],
        ink: Vec::new(),
        caps: Vec::new(),
    };
    let traced = TracedCharacter { parts: vec![mouth], palette: vec![ink, skin], uniform_width: 2.0 };
    let face = buzz_ai::face::build_face_rig(&skel, &traced, "Test", &traced.palette, 2.0, 500, &buzz_ai::face::DrawnEyes::default());

    let padded_width = (x1 - x0) * 1.4 + 6.0;
    let sym = &face.mouth;
    let rest = sym.layers.iter().next().expect("the mouth layer").frames.objects_at(0);
    assert_eq!(rest.len(), 2, "the Rest frame holds the mouth's two shapes, not the patch's");
    let mut bounds: Option<buzz_geom::Rect> = None;
    for o in rest.iter() {
        let b = o.bounds();
        bounds = Some(bounds.map_or(b, |u| u.union(b)));
    }
    let width = bounds.unwrap().width();
    assert!(width <= padded_width + 1.0, "the Rest frame is {width:.0} px wide, over the mouth's padded {padded_width:.0} px");
    assert!((face.mouth_center - centre).hypot() < 2.0, "the mouth is centred on the mouth: {:?} vs {centre:?}", face.mouth_center);
    assert_eq!(face.mouth_underlay.len(), 2, "the base fill and the skin fill are kept under the mouth");

    // A mouth part with nothing traced inside the mouth (its mask took patches of the
    // face edge instead): no mouth to measure, so the mouth goes where the landmarks
    // say it is, at their width, and everything the part drew stays as the underlay.
    let none_inside = character::TracedPart {
        name: "Mouth".into(),
        depth: 100,
        shapes: vec![ShapeData::filled(rect(110.0, 70.0), skin), ShapeData::filled(rect(104.0, 60.0), skin)],
        ink: Vec::new(),
        caps: Vec::new(),
    };
    let traced = TracedCharacter { parts: vec![none_inside], palette: vec![ink, skin], uniform_width: 2.0 };
    let face = buzz_ai::face::build_face_rig(&skel, &traced, "Test", &traced.palette, 2.0, 500, &buzz_ai::face::DrawnEyes::default());
    assert!(
        (face.mouth_center - centre).hypot() < 2.0,
        "with no traced mouth the mouth is where the landmarks put it: {:?} vs {centre:?}",
        face.mouth_center
    );
    assert_eq!(face.mouth_underlay.len(), 2, "everything the part drew stays under the mouth");
}
