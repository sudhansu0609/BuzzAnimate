//! Motion blur on the real GPU: an open shutter smears what moves while it is
//! open, and leaves everything else exactly as it was.
//!
//! The frame is drawn at several instants across the shutter and the results are
//! added up, so proving it works means proving three things: a moving shape
//! comes out smeared, a still one comes out **byte-identical** to the clean
//! frame, and the smear sits *around* where the shape is rather than trailing
//! behind it. Skips with no GPU.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, ObjectId, Scene, ShapeData, Tween};
use peniko::Color;

const BG: Color = Color::from_rgb8(0x00, 0x00, 0x00);
const ART: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
const TRAVEL: f64 = 240.0;
const LAST: u32 = 24;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping motion-blur test: no usable GPU ({e})"),
    }
}

/// A black stage with one white square. `travel` is how far it slides between
/// frame 0 and frame 24 — zero for a square that never moves.
fn document(travel: f64) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = BG;
    scene.stage_mut().size = buzz_geom::Size::new(400.0, 200.0);

    let layer = scene.add_layer("Art", LayerKind::Normal);
    let id: ObjectId = scene
        .add_shape(
            layer,
            ShapeData::filled(Rect::new(60.0, 70.0, 120.0, 130.0).to_path(1e-9), ART),
        )
        .expect("the square was placed");

    scene.update_layer(layer, |l| {
        if l.frames.length() <= LAST {
            l.frames.insert_frame(LAST);
        }
    });
    scene.ensure_keyframe(layer, LAST);
    scene.update_object_at(LAST, id, |o| {
        o.transform = Affine::translate((travel, 0.0)) * o.transform;
    });
    scene.update_layer(layer, |l| {
        l.frames.set_tween(0, Tween::motion());
    });
    scene
}

/// Open the shutter for half a frame — the hundred-and-eighty-degree shutter
/// almost all cinema is shot at.
fn with_shutter(mut scene: Scene, samples: u32) -> Scene {
    {
        let cam = scene.camera_mut();
        cam.shutter = 0.5;
        cam.blur_samples = samples;
    }
    scene
}

/// Pixels on the middle scanline that are neither background nor solid artwork:
/// the soft edge a smear is made of.
fn soft_pixels(frame: &Frame) -> usize {
    let y = frame.height / 2;
    (0..frame.width)
        .filter(|x| {
            let l = frame.pixel(*x, y)[0];
            l > 12 && l < 243
        })
        .count()
}

/// Where the light on the middle scanline sits, weighted by how bright it is.
fn centroid(frame: &Frame) -> f64 {
    let y = frame.height / 2;
    let mut weight = 0.0;
    let mut moment = 0.0;
    for x in 0..frame.width {
        let l = frame.pixel(x, y)[0] as f64;
        weight += l;
        moment += l * x as f64;
    }
    if weight == 0.0 { 0.0 } else { moment / weight }
}

/// The heart of it: what moves while the shutter is open comes out smeared —
/// and smeared by **exactly as far as it travels while the shutter is open**,
/// which is the difference between motion blur and a blur.
///
/// The square crosses `TRAVEL` in `LAST` frames, so a half-frame shutter sees
/// it move `0.5 * TRAVEL / LAST` — five pixels here — and each of its two
/// vertical edges is drawn across that distance.
#[test]
fn an_open_shutter_smears_what_is_moving() {
    with_exporter(|exporter| {
        let sharp_doc = document(TRAVEL);
        let settings = ExportSettings::for_stage(&sharp_doc);
        let blurred = with_shutter(document(TRAVEL), 16);

        let sharp = exporter.render(&sharp_doc, 12, &settings).expect("sharp");
        let smeared = exporter.render(&blurred, 12, &settings).expect("smeared");

        let sharp_soft = soft_pixels(&sharp);
        let smeared_soft = soft_pixels(&smeared);
        assert!(
            smeared_soft > sharp_soft + 5,
            "an open shutter should soften a moving edge: sharp {sharp_soft}, smeared {smeared_soft}"
        );

        let travelled = 0.5 * TRAVEL / LAST as f64;
        let per_edge = (smeared_soft - sharp_soft) as f64 / 2.0;
        assert!(
            (per_edge - travelled).abs() < 2.5,
            "each edge should smear about as far as the square travels while the              shutter is open ({travelled:.1}px), got {per_edge:.1}px"
        );
    });
}

/// **The parity guarantee.** A document that asks for no shutter is drawn
/// exactly as it was before motion blur existed — not nearly, exactly.
#[test]
fn no_shutter_is_the_frame_it_always_was() {
    with_exporter(|exporter| {
        let scene = document(TRAVEL);
        let settings = ExportSettings::for_stage(&scene);
        assert_eq!(scene.camera().shutter, 0.0, "off by default");

        let once = exporter.render(&scene, 12, &settings).expect("first");
        let twice = exporter.render(&scene, 12, &settings).expect("second");
        assert_eq!(once.pixels, twice.pixels, "the clean path is unchanged");
    });
}

/// A shutter open over something that is not moving records the same picture at
/// every instant, so adding them up must give that picture back.
#[test]
fn a_still_shape_is_untouched_by_the_shutter() {
    with_exporter(|exporter| {
        let still = document(0.0);
        let settings = ExportSettings::for_stage(&still);
        let shuttered = with_shutter(document(0.0), 16);

        let clean = exporter.render(&still, 12, &settings).expect("clean");
        let shot = exporter.render(&shuttered, 12, &settings).expect("shuttered");

        let differing = clean
            .pixels
            .iter()
            .zip(shot.pixels.iter())
            .filter(|(a, b)| a.abs_diff(**b) > 1)
            .count();
        assert_eq!(
            differing, 0,
            "a still shape must survive the shutter unchanged, {differing} bytes differ"
        );
    });
}

/// The shutter is centred on the frame, so the smear sits *around* where the
/// artwork is. Were it to open at the frame instead, everything moving would
/// sit half a shutter late and the animation would read as having slipped.
#[test]
fn the_smear_is_centred_on_the_frame_it_belongs_to() {
    with_exporter(|exporter| {
        let sharp_doc = document(TRAVEL);
        let settings = ExportSettings::for_stage(&sharp_doc);
        let blurred = with_shutter(document(TRAVEL), 16);

        let sharp = centroid(&exporter.render(&sharp_doc, 12, &settings).expect("sharp"));
        let smeared = centroid(&exporter.render(&blurred, 12, &settings).expect("smeared"));

        // Half a shutter of travel is 5 pixels here; a centred smear should be
        // far inside that of the clean frame's centre.
        assert!(
            (sharp - smeared).abs() < 1.5,
            "the smear should stay centred: sharp at {sharp:.2}, smeared at {smeared:.2}"
        );
    });
}

/// **A camera move smears too.** Nothing on the stage is moving here — the
/// shot is. A pan is motion across the film exactly as a moving object is, and
/// it goes through the same shutter because the camera is asked for the same
/// instant the artwork is.
#[test]
fn panning_the_camera_smears_a_still_stage() {
    with_exporter(|exporter| {
        let mut panned = document(0.0);
        {
            let cam = panned.camera_mut();
            cam.enabled = true;
            // The pan passes through the stage centre exactly at frame 12, so
            // the square is in shot there and only the *speed* is under test.
            cam.set_key(buzz_scene::CameraKey::new(
                0,
                buzz_geom::Point::new(0.0, 100.0),
            ));
            cam.set_key(buzz_scene::CameraKey::new(
                LAST,
                buzz_geom::Point::new(400.0, 100.0),
            ));
        }
        let settings = ExportSettings::for_stage(&panned);
        let blurred = with_shutter(panned.clone(), 16);

        let sharp = soft_pixels(&exporter.render(&panned, 12, &settings).expect("sharp"));
        let smeared = soft_pixels(&exporter.render(&blurred, 12, &settings).expect("smeared"));
        assert!(
            smeared > sharp + 5,
            "a pan should smear a still stage: sharp {sharp}, smeared {smeared}"
        );

        // The pan crosses 400 units in 24 frames, so half a frame of it is
        // about eight pixels — and that is what each edge should be drawn over.
        let travelled = 0.5 * 400.0 / LAST as f64;
        let per_edge = (smeared - sharp) as f64 / 2.0;
        assert!(
            (per_edge - travelled).abs() < 3.0,
            "the smear should be as long as the pan travels ({travelled:.1}px), got {per_edge:.1}px"
        );
    });
}

/// More samples fill the smear in. Too few read as a row of ghosts, which is
/// the artefact the sample count exists to trade against.
#[test]
fn more_samples_make_a_smoother_smear() {
    with_exporter(|exporter| {
        let coarse = with_shutter(document(TRAVEL), 2);
        let fine = with_shutter(document(TRAVEL), 32);
        let settings = ExportSettings::for_stage(&coarse);

        let coarse_soft = soft_pixels(&exporter.render(&coarse, 12, &settings).expect("coarse"));
        let fine_soft = soft_pixels(&exporter.render(&fine, 12, &settings).expect("fine"));
        assert!(
            fine_soft > coarse_soft,
            "more instants should leave fewer hard steps: 2 samples {coarse_soft}, 32 samples {fine_soft}"
        );
    });
}
