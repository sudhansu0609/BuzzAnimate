use buzz_ai::character::{
    assemble_character, complete_parts, part_masks, skeleton, trace_character_whole,
};
use buzz_ai::pose::Person;
use buzz_ai::segment::matte;
use buzz_audio::{Viseme, VisemeTrack};
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, LoopMode, ObjectKind};

#[test]
fn face_rig_symbols_and_controls() {
    let png_path = "tests/data/pujariFront_0.png";
    let Ok(img) = image::open(png_path) else {
        eprintln!("Skipping test: {} not found", png_path);
        return;
    };
    let img = img.to_rgba8();
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
    let traced = trace_character_whole(&completed, &skel, &matte, Some(2.0)).expect("trace character");

    let mut assembled = assemble_character(
        &skel,
        &traced,
        "PujariFace",
        Some((img.width() as f64, img.height() as f64)),
        None,
    )
    .expect("assemble character with face rig");

    // 1. Verify face rig presence
    let face = assembled.face.clone().expect("face rig must be populated");
    assert!(
        assembled.scene.library().get(face.mouth.id).is_some(),
        "mouth symbol must be in library"
    );
    assert!(
        assembled.scene.library().get(face.left_eye.id).is_some(),
        "left eye symbol must be in library"
    );
    assert!(
        assembled.scene.library().get(face.right_eye.id).is_some(),
        "right eye symbol must be in library"
    );

    // 2. Verify mouth symbol has 10 viseme keyframes
    let mouth_sym = assembled.scene.library().get(face.mouth.id).unwrap();
    let mouth_layer = mouth_sym.layers.iter().next().unwrap();
    assert_eq!(
        mouth_layer.frames.length(),
        10,
        "mouth symbol must have 10 frames (1 per viseme)"
    );
    for frame_idx in 0..10 {
        assert!(
            mouth_layer.frames.keyframe_at(frame_idx).is_some(),
            "frame {} must be a keyframe",
            frame_idx
        );
        let objs = mouth_layer.frames.objects_at(frame_idx);
        // I5: Rest (frame 0) is the traced mouth when there is one, and EMPTY
        // when there is none — the drawn mouth is on the head, and a synthetic
        // Rest lens over it painted a dark mouth on every rig.
        if frame_idx == 0 {
            continue;
        }
        assert!(!objs.is_empty(), "frame {} must contain mouth shape objects", frame_idx);
    }

    // 3. Drive mouth from fixture VisemeTrack with apply_lipsync
    let mouth_layer_id = assembled.scene.add_layer("LipSync", LayerKind::Normal);
    let track = VisemeTrack {
        frames: vec![
            Viseme::Rest, Viseme::Rest, Viseme::Rest,
            Viseme::Ai, Viseme::Ai, Viseme::Ai,
            Viseme::O, Viseme::O, Viseme::O,
            Viseme::MBP, Viseme::MBP, Viseme::MBP,
            Viseme::Rest, Viseme::Rest,
        ],
        fps: 24.0,
    };

    let report = assembled.apply_lipsync(&track, 0, mouth_layer_id);
    assert_eq!(report.keyframes, 5);

    let sync_layer = assembled.scene.layers().get(mouth_layer_id).unwrap();
    assert_eq!(sync_layer.frames.keyframes().len(), 5);

    // Check keyframe at frame 3 has Viseme::Ai (frame 1)
    let objs_at_3 = sync_layer.frames.objects_at(3);
    assert_eq!(objs_at_3.len(), 1);
    if let ObjectKind::Instance(inst) = &objs_at_3[0].kind {
        assert_eq!(inst.symbol, face.mouth.id);
        assert_eq!(inst.loop_mode, LoopMode::SingleFrame);
        assert_eq!(inst.first_frame, Viseme::Ai.frame());
    } else {
        panic!("LipSync keyframe must hold an instance of the mouth symbol");
    }

    // 4. Test look_at gaze controls
    assembled.look_at(1.0, -0.5);
    let eye_sym = assembled.scene.library().get(face.left_eye.id).unwrap();
    let pupil_layer = eye_sym.layers.iter().find(|l| l.name == "Pupil").unwrap();
    let pupil_objs = pupil_layer.frames.objects_at(0);
    assert!(!pupil_objs.is_empty());
    let pupil_x = pupil_objs[0].transform.as_coeffs()[4];
    let pupil_y = pupil_objs[0].transform.as_coeffs()[5];
    assert!(
        pupil_x > 0.0 && pupil_x <= face.max_pupil_travel.0,
        "pupil x {} must be positive and bounded by max travel {}",
        pupil_x,
        face.max_pupil_travel.0
    );
    assert!(
        pupil_y < 0.0 && pupil_y >= -face.max_pupil_travel.1,
        "pupil y {} must be negative and bounded by max travel {}",
        pupil_y,
        face.max_pupil_travel.1
    );

    // 5. Test set_blink eyelid frames
    assembled.set_blink(0.0); // Open
    let eye_sym = assembled.scene.library().get(face.left_eye.id).unwrap();
    let eyelid_layer = eye_sym.layers.iter().find(|l| l.name == "Eyelid").unwrap();
    if let ObjectKind::Instance(inst) = &eyelid_layer.frames.objects_at(0)[0].kind {
        assert_eq!(inst.first_frame, 0, "blink 0.0 should be frame 0 (open)");
    }

    assembled.set_blink(0.5); // Half
    let eye_sym = assembled.scene.library().get(face.left_eye.id).unwrap();
    let eyelid_layer = eye_sym.layers.iter().find(|l| l.name == "Eyelid").unwrap();
    if let ObjectKind::Instance(inst) = &eyelid_layer.frames.objects_at(0)[0].kind {
        assert_eq!(inst.first_frame, 1, "blink 0.5 should be frame 1 (half)");
    }

    assembled.set_blink(1.0); // Closed
    let eye_sym = assembled.scene.library().get(face.left_eye.id).unwrap();
    let eyelid_layer = eye_sym.layers.iter().find(|l| l.name == "Eyelid").unwrap();
    if let ObjectKind::Instance(inst) = &eyelid_layer.frames.objects_at(0)[0].kind {
        assert_eq!(inst.first_frame, 2, "blink 1.0 should be frame 2 (closed)");
    }

    // 6. Headless render: verify no crashes or panics on render
    if let Ok(mut exporter) = Exporter::new(&GpuPreference::Automatic) {
        let settings = ExportSettings::for_stage(&assembled.scene);
        let frame = exporter.render(&assembled.scene, 0, &settings);
        assert!(frame.is_ok(), "Rendering assembled character scene with face rig must succeed");
    } else {
        eprintln!("Notice: skipping GPU export render check (no GPU available)");
    }
}

// ---------------------------------------------------------------------------
// T4: the rig keeps the DRAWN eyes (synthetic, no models, no fixture).
// ---------------------------------------------------------------------------

use std::sync::Arc;

use buzz_ai::character::{Skeleton, TracedCharacter, TracedPart};
use buzz_ai::face::{build_face_rig, set_eye_blink, set_eye_look_at, DrawnEyes};
use buzz_scene::{Object, ShapeData};
use kurbo::{BezPath, Circle, Ellipse, Point, Rect, Shape as _};
use peniko::Color;

/// A standing figure on a 600 x 1200 canvas with an eye contour (12 px half-width,
/// 5 px half-height) at (270, 80) and (330, 80), and the nose at (300, 100).
fn figure_skeleton() -> Skeleton {
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
    for (first, cx) in [(59usize, 270.0f32), (65, 330.0)] {
        for i in 0..6 {
            let a = i as f32 * std::f32::consts::TAU / 6.0;
            keypoints[first + i] = (cx + 12.0 * a.cos(), 80.0 + 5.0 * a.sin(), 0.9);
        }
    }
    skeleton(&Person { bbox: [0.0, 0.0, 600.0, 1200.0], keypoints })
}

const EYE_WHITE: (u8, u8, u8) = (250, 250, 250);
const PUPIL_INK: (u8, u8, u8) = (15, 10, 10);
const SKIN: (u8, u8, u8) = (232, 180, 150);
const BROW: (u8, u8, u8) = (70, 45, 30);

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::from_rgb8(c.0, c.1, c.2)
}

fn white_path(cx: f64) -> BezPath {
    Ellipse::new((cx, 80.0), (11.0, 4.0), 0.0).to_path(0.05)
}

fn pupil_path(cx: f64, offset: f64) -> BezPath {
    Circle::new((cx + offset, 80.0), 3.0).to_path(0.05)
}

fn brow_path(cx: f64) -> BezPath {
    Rect::new(cx - 12.0, 62.0, cx + 12.0, 66.0).to_path(0.1)
}

/// The head part: skin, two brows, and each eye drawn as a light white and a dark pupil.
fn drawn_face() -> TracedCharacter {
    let head = TracedPart {
        name: "Head".into(),
        depth: 10,
        shapes: vec![
            ShapeData::filled(Rect::new(230.0, 20.0, 370.0, 140.0).to_path(0.1), rgb(SKIN)),
            ShapeData::filled(brow_path(270.0), rgb(BROW)),
            ShapeData::filled(brow_path(330.0), rgb(BROW)),
            ShapeData::filled(white_path(270.0), rgb(EYE_WHITE)),
            ShapeData::filled(pupil_path(270.0, 2.0), rgb(PUPIL_INK)),
            ShapeData::filled(white_path(330.0), rgb(EYE_WHITE)),
            ShapeData::filled(pupil_path(330.0, -2.0), rgb(PUPIL_INK)),
        ],
        ink: Vec::new(),
        caps: Vec::new(),
    };
    TracedCharacter { parts: vec![head], palette: vec![rgb(PUPIL_INK), rgb(SKIN)], uniform_width: 2.0 }
}

fn shape_of(o: &Object) -> Option<&ShapeData> {
    match &o.kind {
        ObjectKind::Shape(s) => Some(s),
        _ => None,
    }
}

/// Unsigned areas of the shapes in `objects` filled with `colour`.
fn areas_filled_with(objects: &[Arc<Object>], colour: Color) -> Vec<f64> {
    objects
        .iter()
        .filter_map(|o| shape_of(o))
        .filter(|s| s.fill.as_ref().map(|f| f.color()) == Some(colour))
        .map(|s| s.path.area().abs())
        .collect()
}

#[test]
fn the_rig_keeps_the_drawn_eyes_and_builds_the_pupil_and_lid_from_them() {
    let skel = figure_skeleton();
    let traced = drawn_face();
    let mut assembled = assemble_character(&skel, &traced, "Drawn", Some((600.0, 1200.0)), None).expect("assemble");
    let face = assembled.face.clone().expect("face rig");
    let brow_area = brow_path(270.0).area().abs();
    let white_area = white_path(270.0).area().abs();

    // The head part keeps the skin and BOTH brows exactly; the eye artwork has left it.
    let head = assembled.part_symbols[0].layers.iter().next().expect("artwork layer").frames.objects_at(0);
    let brows = areas_filled_with(head, rgb(BROW));
    assert_eq!(brows.len(), 2, "both brows stay in the head part");
    assert!(brows.iter().all(|a| (a - brow_area).abs() < 1e-6), "brows untouched: {brows:?} vs {brow_area}");
    assert_eq!(areas_filled_with(head, rgb(SKIN)).len(), 1, "the skin stays");
    assert!(areas_filled_with(head, rgb(EYE_WHITE)).is_empty(), "the eye whites moved into the eye");
    assert!(areas_filled_with(head, rgb(PUPIL_INK)).is_empty(), "the pupils moved into the eye");

    for (eye, lid, cx, pupil_offset) in [
        (&face.left_eye, &face.left_eyelid, 270.0, 2.0),
        (&face.right_eye, &face.right_eyelid, 330.0, -2.0),
    ] {
        // White layer: the drawn white, to 1 % of its area.
        let white = eye.layers.iter().find(|l| l.name == "White").expect("White layer").frames.objects_at(0);
        let whites = areas_filled_with(white, rgb(EYE_WHITE));
        assert_eq!(whites.len(), 1, "the drawn white, once");
        assert!((whites[0] - white_area).abs() / white_area < 0.01, "white {} vs drawn {}", whites[0], white_area);

        // Pupil layer: ONE object, centred on the drawn pupil to 2 px.
        let pupil = eye.layers.iter().find(|l| l.name == "Pupil").expect("Pupil layer").frames.objects_at(0);
        assert_eq!(pupil.len(), 1, "look-at moves the pupil layer's one object");
        let c = pupil[0].bounds().center();
        let centre = Point::new(c.x + eye.registration.x, c.y + eye.registration.y);
        assert!(
            (centre.x - (cx + pupil_offset)).abs() < 2.0 && (centre.y - 80.0).abs() < 2.0,
            "pupil centred at {centre:?}, drawn at ({}, 80)",
            cx + pupil_offset
        );

        // The shut lid (frame 2) is the white's shape in skin colour, outlined.
        let lid_layer = lid.layers.iter().next().expect("lid layer");
        assert!(lid_layer.frames.objects_at(0).is_empty(), "open lid is empty");
        let shut = lid_layer.frames.objects_at(2);
        let fill = shape_of(&shut[0]).expect("lid fill");
        assert_eq!(fill.fill.as_ref().map(|f| f.color()), Some(rgb(SKIN)), "skin-coloured lid");
        let lid_area = fill.path.area().abs();
        assert!((lid_area - white_area).abs() / white_area < 0.02, "shut lid {lid_area} vs white {white_area}");
        let outline = shape_of(&shut[1]).expect("lid outline");
        assert_eq!(outline.stroke.as_ref().map(|s| s.width), Some(2.0), "outlined at the uniform width");
        // Half: the upper half only, about half the area.
        let half = shape_of(&lid_layer.frames.objects_at(1)[0]).expect("half lid").path.area().abs();
        assert!((half - white_area * 0.5).abs() / white_area < 0.06, "half lid {half} vs half white {}", white_area * 0.5);
    }

    // Blink sets the Eyelid instance to frame 2.
    let mut eye = (**assembled.scene.library().get(face.left_eye.id).unwrap()).clone();
    set_eye_blink(&mut eye, 1.0);
    let lid_layer = eye.layers.iter().find(|l| l.name == "Eyelid").unwrap();
    match &lid_layer.frames.objects_at(0)[0].kind {
        ObjectKind::Instance(inst) => assert_eq!(inst.first_frame, 2),
        _ => panic!("the Eyelid layer holds the lid instance"),
    }

    // Look-at moves the pupil object, by at most 0.4 x the white's half size.
    assert!((face.max_pupil_travel.0 - 0.4 * 11.0).abs() < 0.2, "travel {:?}", face.max_pupil_travel);
    let mut eye = (**assembled.scene.library().get(face.left_eye.id).unwrap()).clone();
    set_eye_look_at(&mut eye, 1.0, 0.0, face.max_pupil_travel);
    let moved = eye.layers.iter().find(|l| l.name == "Pupil").unwrap().frames.objects_at(0)[0].transform.as_coeffs();
    assert!((moved[4] - face.max_pupil_travel.0).abs() < 1e-9 && moved[5].abs() < 1e-9, "pupil moved: {moved:?}");
    assembled.look_at(-1.0, 0.0);
    let eye = assembled.scene.library().get(face.left_eye.id).unwrap();
    let moved = eye.layers.iter().find(|l| l.name == "Pupil").unwrap().frames.objects_at(0)[0].transform.as_coeffs();
    assert!(moved[4] < 0.0, "assembled.look_at moves the pupil object: {moved:?}");
}

#[test]
fn a_white_with_a_hole_where_the_pupil_sits_gets_a_backing_and_a_lid_that_covers_the_hole() {
    let skel = figure_skeleton();
    let mut traced = drawn_face();
    // Both whites are traced as a ring: the white with the pupil's disc cut out.
    let hole = pupil_path(270.0, 0.0);
    let ring = buzz_geom::boolean(
        &white_path(270.0),
        &hole,
        buzz_geom::BoolOp::Difference,
        buzz_geom::BooleanOptions::default(),
    );
    let ring_area = ring.area().abs();
    let white_area = white_path(270.0).area().abs();
    assert!(ring_area < white_area * 0.9, "the ring has a hole ({ring_area} vs {white_area})");
    traced.parts[0].shapes[3] = ShapeData::filled(ring, rgb(EYE_WHITE));
    let assembled = assemble_character(&skel, &traced, "Ring", Some((600.0, 1200.0)), None).expect("assemble");
    let face = assembled.face.expect("face rig");

    let white = face.left_eye.layers.iter().find(|l| l.name == "White").unwrap().frames.objects_at(0);
    let whites = areas_filled_with(white, rgb(EYE_WHITE));
    assert_eq!(whites.len(), 2, "a backing and the drawn ring");
    assert!((whites[0] - white_area).abs() / white_area < 0.02, "the backing is the whole white: {whites:?}");
    assert!((whites[1] - ring_area).abs() / ring_area < 0.02, "the ring as drawn: {whites:?}");

    let shut = face.left_eyelid.layers.iter().next().unwrap().frames.objects_at(2);
    let lid_area = shape_of(&shut[0]).unwrap().path.area().abs();
    assert!((lid_area - white_area).abs() / white_area < 0.02, "the shut lid covers the pupil's hole: {lid_area} vs {white_area}");
}

#[test]
fn an_eye_with_nothing_drawn_gets_a_synthetic_pupil_and_an_ellipse_lid() {
    let skel = figure_skeleton();
    let traced = drawn_face();
    let face = build_face_rig(&skel, &traced, "Bare", &traced.palette, 2.0, 500, &DrawnEyes::default());
    for eye in [&face.left_eye, &face.right_eye] {
        let white = eye.layers.iter().find(|l| l.name == "White").unwrap().frames.objects_at(0);
        assert!(white.is_empty(), "no synthetic white any more");
        let pupil = eye.layers.iter().find(|l| l.name == "Pupil").unwrap().frames.objects_at(0);
        assert_eq!(pupil.len(), 1, "a synthetic pupil");
        assert!(pupil[0].bounds().center().distance(Point::ORIGIN) < 0.5, "centred on the eye");
    }
    let shut = face.left_eyelid.layers.iter().next().unwrap().frames.objects_at(2);
    assert!(shape_of(&shut[0]).unwrap().path.area().abs() > 10.0, "an ellipse lid");
    assert!((face.left_eye_center.x - 270.0).abs() < 1.0 && (face.left_eye_center.y - 80.0).abs() < 1.0);
}

// ---------------------------------------------------------------------------
// T9: the ink inside an eye's box is the eye's outline (White layer), never its pupil.
// ---------------------------------------------------------------------------

const RING_INK: (u8, u8, u8) = (30, 20, 15);

/// A dark outline ring round the white of the eye at `cx`: an ink shape, as the
/// brush tracer draws an eye's outline, just inside the eye's landmark box.
fn eye_ring_path(cx: f64) -> BezPath {
    let outer = Ellipse::new((cx, 80.0), (11.5, 4.2), 0.0).to_path(0.05);
    let inner = Ellipse::new((cx, 80.0), (10.0, 3.0), 0.0).to_path(0.05);
    buzz_geom::boolean(&outer, &inner, buzz_geom::BoolOp::Difference, buzz_geom::BooleanOptions::default())
}

#[test]
fn ink_in_the_eye_box_goes_to_the_white_layer_and_the_pupil_stays_the_drawn_fill() {
    let skel = figure_skeleton();
    let mut traced = drawn_face();
    let ring_area = eye_ring_path(270.0).area().abs();
    assert!(ring_area > 20.0, "the ring has an area ({ring_area})");
    // The ink: a dark ring round each eye (L* < 45, so by lightness alone it would be
    // "pupil") and one stroke over each brow (outside the eye boxes).
    traced.parts[0].ink = vec![
        ShapeData::filled(eye_ring_path(270.0), rgb(RING_INK)),
        ShapeData::filled(eye_ring_path(330.0), rgb(RING_INK)),
        ShapeData::filled(brow_path(270.0), rgb(RING_INK)),
        ShapeData::filled(brow_path(330.0), rgb(RING_INK)),
    ];
    let assembled = assemble_character(&skel, &traced, "Inked", Some((600.0, 1200.0)), None).expect("assemble");
    let face = assembled.face.clone().expect("face rig");

    // The head part: its artwork keeps the fills, an "Ink" layer ABOVE it (listed first:
    // the front of the list is the top) keeps the two brow strokes and not the eye rings.
    let head = &assembled.part_symbols[0];
    let names: Vec<&str> = head.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Ink", "Artwork"], "one symbol, ink above the artwork");
    let ink_layer = head.layers.iter().find(|l| l.name == "Ink").unwrap().frames.objects_at(0);
    assert_eq!(ink_layer.len(), 2, "only the brow strokes stay in the head's ink layer");
    assert!(areas_filled_with(ink_layer, rgb(RING_INK)).iter().all(|a| (a - brow_path(270.0).area().abs()).abs() < 1e-6));

    for (eye, cx, pupil_offset) in [(&face.left_eye, 270.0, 2.0), (&face.right_eye, 330.0, -2.0)] {
        // White layer: the drawn white, then the ring on top of it (last).
        let white = eye.layers.iter().find(|l| l.name == "White").expect("White layer").frames.objects_at(0);
        assert_eq!(areas_filled_with(white, rgb(EYE_WHITE)).len(), 1, "the drawn white");
        let rings = areas_filled_with(white, rgb(RING_INK));
        assert_eq!(rings.len(), 1, "the eye's ink ring is in the White layer");
        assert!((rings[0] - ring_area).abs() / ring_area < 0.02, "ring {} vs drawn {ring_area}", rings[0]);
        let last = shape_of(white.last().expect("white objects")).expect("shape");
        assert_eq!(last.fill.as_ref().map(|f| f.color()), Some(rgb(RING_INK)), "the ink is on top of the whites");

        // Pupil layer: ONE object, the drawn pupil fill; the dark ring is not in it.
        let pupil = eye.layers.iter().find(|l| l.name == "Pupil").expect("Pupil layer").frames.objects_at(0);
        assert_eq!(pupil.len(), 1, "one pupil object");
        assert!(areas_filled_with(pupil, rgb(RING_INK)).is_empty(), "the ring is never the pupil");
        let pupil_shape = shape_of(&pupil[0]).expect("the pupil is one shape");
        assert_eq!(pupil_shape.fill.as_ref().map(|f| f.color()), Some(rgb(PUPIL_INK)), "the drawn pupil fill");
        let c = pupil[0].bounds().center();
        let centre = Point::new(c.x + eye.registration.x, c.y + eye.registration.y);
        assert!(
            (centre.x - (cx + pupil_offset)).abs() < 2.0 && (centre.y - 80.0).abs() < 2.0,
            "pupil centred at {centre:?}, drawn at ({}, 80)",
            cx + pupil_offset
        );
    }
}
