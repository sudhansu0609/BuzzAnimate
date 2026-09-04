//! **The pictures in the user guide, rendered by the program they document.**
//!
//! # Why the figures are generated rather than captured
//!
//! A screenshot pasted into a manual is correct on the day it is taken and
//! quietly wrong for ever afterwards. These are rendered by the same exporter
//! that renders the film, from scenes built by the same code the features
//! themselves use — so a figure that stops matching the feature stops matching
//! because the *feature* changed, and re-running this is how it is put right.
//!
//! # Running it
//!
//! ```text
//! cargo test -p buzz-export --test doc_figures -- --ignored --nocapture
//! ```
//!
//! Ignored by default because it writes into the repository, which is not
//! something an ordinary `cargo test` should do. Skips with no GPU, like every
//! other headless test here.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    CameraKey, CameraMove, LayerKind, Modifier, Object, ObjectId, ObjectKind, Scene, ShapeData,
};
use peniko::Color;

const INK: Color = Color::from_rgb8(0x1A, 0x1A, 0x22);
const SKIN: Color = Color::from_rgb8(0xF2, 0xD2, 0xB6);
const IRIS: Color = Color::from_rgb8(0x2E, 0x4A, 0x7A);

/// An ellipse, from the unit circle `buzz_geom` does re-export. A face is all
/// ellipses and there is no point in drawing one out of Béziers by hand.
fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> buzz_geom::BezPath {
    let unit = buzz_geom::Circle::new(Point::new(0.0, 0.0), 1.0).to_path(0.001);
    Affine::translate((cx, cy)) * Affine::scale_non_uniform(rx, ry) * unit
}

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping doc figures: no usable GPU ({e})"),
    }
}

/// Where the guide's pictures live.
fn images_dir() -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/images")
        .canonicalize()
        .expect("docs/images should exist");
    dir
}

fn write(frame: &Frame, name: &str) {
    let path = images_dir().join(name);
    frame.write_png(&path).expect("writing the figure");
    eprintln!("  wrote {}", path.display());
}

fn render(exporter: &mut Exporter, scene: &Scene, frame: u32) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, frame, &settings).expect("render")
}

// ---------------------------------------------------------------------------
// Blink
// ---------------------------------------------------------------------------

/// A face: a head, two eyes, two pupils. The eyes are their own objects,
/// because that is where a blink goes — on the eye artwork, not the character.
fn face(scene: &mut Scene) -> Vec<ObjectId> {
    let head_layer = scene.add_layer("Head", LayerKind::Normal);
    let mut head = Object::shape(
        ObjectId(1),
        ShapeData::filled(
            ellipse(275.0, 195.0, 95.0, 112.0),
            SKIN,
        ),
    );
    head.transform = Affine::IDENTITY;
    scene.add_object(head_layer, head).expect("a head");

    // **Both eyes are one grouped object.**
    //
    // Not tidiness. The blink's phase is seeded from the object's id — which is
    // what stops a *cast* blinking in unison — so two eyes left as two objects
    // blink independently, and a character who winks at random is a worse
    // problem than one who never blinks at all. One modifier, on the thing that
    // closes together. Building this figure is how that was found out, and it
    // is why the guide says it in bold.
    let eye_layer = scene.add_layer("Eyes", LayerKind::Normal);
    let mut parts: Vec<std::sync::Arc<Object>> = Vec::new();
    for (i, x) in [238.0_f64, 312.0].into_iter().enumerate() {
        parts.push(std::sync::Arc::new(Object::shape(
            ObjectId(10 + i as u64),
            ShapeData::filled(ellipse(x, 180.0, 26.0, 20.0), Color::WHITE),
        )));
        parts.push(std::sync::Arc::new(Object::shape(
            ObjectId(20 + i as u64),
            ShapeData::filled(ellipse(x, 181.0, 10.0, 11.0), IRIS),
        )));
    }
    let eyes = vec![
        scene
            .add_object(eye_layer, Object::group(ObjectId(40), parts))
            .expect("the eyes"),
    ];

    let mouth_layer = scene.add_layer("Mouth", LayerKind::Normal);
    let mut mouth = Object::shape(
        ObjectId(30),
        ShapeData::stroked(
            ellipse(275.0, 246.0, 34.0, 16.0),
            INK,
            3.0,
        ),
    );
    mouth.transform = Affine::IDENTITY;
    scene.add_object(mouth_layer, mouth).expect("a mouth");

    for id in [head_layer, eye_layer, mouth_layer] {
        scene.update_layer(id, |l| { l.frames.insert_frame(120); });
    }
    eyes
}

/// **Blink, open and shut.**
///
/// The figure the guide needs is the pair: the point of the feature is that the
/// eye is open almost all of the time and shuts for four frames, and a single
/// picture of either state says nothing about the other.
#[test]
#[ignore = "writes into docs/images; run explicitly"]
fn figure_blink() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::from_rgb8(0xE8, 0xEC, 0xF2);
        let eyes = face(&mut scene);

        // A fast blink, so a shut frame is easy to find and to point at.
        for id in &eyes {
            scene.update_object_across(0, 120, *id, |o| {
                o.modifiers.push(Modifier::Blink {
                    rate: 60.0,
                    duration: 0.16,
                });
            });
        }

        // Find the frame where the eye is most nearly shut, and one where it is
        // fully open, by measuring the rendered picture rather than by trusting
        // the arithmetic — which is the point of rendering the figure at all.
        let white_pixels = |f: &Frame| {
            f.pixels
                .chunks_exact(4)
                .filter(|p| p[0] > 240 && p[1] > 240 && p[2] > 240)
                .count()
        };

        let mut open = (0u32, 0usize);
        let mut shut = (0u32, usize::MAX);
        for frame in 0..96u32 {
            let f = render(exporter, &scene, frame);
            let n = white_pixels(&f);
            if n > open.1 {
                open = (frame, n);
            }
            if n < shut.1 {
                shut = (frame, n);
            }
        }
        eprintln!(
            "  blink: open at frame {} ({} px), shut at frame {} ({} px)",
            open.0, open.1, shut.0, shut.1
        );
        assert!(
            shut.1 * 2 < open.1,
            "the figure would not show a blink: {} px shut against {} px open",
            shut.1,
            open.1
        );

        let f = render(exporter, &scene, open.0);
        write(&f, "modifier_blink_open.png");
        let f = render(exporter, &scene, shut.0);
        write(&f, "modifier_blink_shut.png");
    });
}

// ---------------------------------------------------------------------------
// Camera moves
// ---------------------------------------------------------------------------

/// A stage with something to frame: a horizon, a figure, and marks near the
/// edges so a push in is visible as a *crop* rather than only as a scale.
fn framing_scene() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x12, 0x18, 0x28);
    let stage = scene.stage().stage_rect();

    let ground = scene.add_layer("Ground", LayerKind::Normal);
    let mut floor = Object::shape(
        ObjectId(1),
        ShapeData::filled(
            Rect::new(0.0, stage.height() * 0.72, stage.width(), stage.height()).to_path(1e-9),
            Color::from_rgb8(0x24, 0x2E, 0x40),
        ),
    );
    floor.transform = Affine::IDENTITY;
    scene.add_object(ground, floor).expect("a floor");

    // Posts across the width: what makes a pan read as a pan.
    let posts = scene.add_layer("Posts", LayerKind::Normal);
    for i in 0..7 {
        let x = stage.width() * (0.08 + 0.14 * i as f64);
        let mut post = Object::shape(
            ObjectId(100 + i as u64),
            ShapeData::filled(
                Rect::new(x - 5.0, stage.height() * 0.52, x + 5.0, stage.height() * 0.74)
                    .to_path(1e-9),
                Color::from_rgb8(0x50, 0x62, 0x80),
            ),
        );
        post.transform = Affine::IDENTITY;
        scene.add_object(posts, post).expect("a post");
    }

    let cast = scene.add_layer("Figure", LayerKind::Normal);
    let mut body = Object::shape(
        ObjectId(2),
        ShapeData::filled(
            Rect::new(
                stage.width() * 0.46,
                stage.height() * 0.42,
                stage.width() * 0.54,
                stage.height() * 0.74,
            )
            .to_path(1e-9),
            Color::from_rgb8(0xE6, 0xC4, 0x8A),
        ),
    );
    body.transform = Affine::IDENTITY;
    scene.add_object(cast, body).expect("a figure");

    for id in [ground, posts, cast] {
        scene.update_layer(id, |l| { l.frames.insert_frame(48); });
    }
    scene
}

/// **What each named move does to the framing.**
///
/// One picture per move at the frame it ends on, against the wide it started
/// from — which is the comparison the guide is making and the only one that
/// says anything.
#[test]
#[ignore = "writes into docs/images; run explicitly"]
fn figure_camera_moves() {
    with_exporter(|exporter| {
        let base = framing_scene();
        let stage = base.stage().stage_rect();

        // The framing every move starts from.
        let mut wide = base.clone();
        wide.camera_mut().enabled = true;
        wide.camera_mut().set_key(CameraKey::new(0, stage.center()));
        let f = render(exporter, &wide, 0);
        write(&f, "camera_move_start.png");

        for (movement, name) in [
            (CameraMove::PushIn, "camera_move_push_in.png"),
            (CameraMove::PullOut, "camera_move_pull_out.png"),
            (CameraMove::PanLeft, "camera_move_pan_left.png"),
            (CameraMove::PanRight, "camera_move_pan_right.png"),
            (CameraMove::Reveal, "camera_move_reveal_open.png"),
            (CameraMove::Drift, "camera_move_drift.png"),
        ] {
            let mut scene = base.clone();
            scene.camera_mut().enabled = true;
            scene.camera_mut().set_key(CameraKey::new(0, stage.center()));
            assert!(
                scene.camera_mut().add_move(movement, 0, 48, stage),
                "{} wrote no keys",
                movement.label()
            );
            // A reveal is shown by its *opening*, which is the derived key and
            // the thing that is surprising about it; everything else by where
            // it arrives.
            let frame = if movement == CameraMove::Reveal { 0 } else { 48 };
            let f = render(exporter, &scene, frame);
            write(&f, name);
        }
    });
}

/// **Eased against linear, at the same moment of the same move.**
///
/// The whole reason easing is on the camera: a linear pan is already at full
/// speed on the frame it starts. Two pictures a fifth of the way into an
/// identical move show it at a glance, where a description does not.
#[test]
#[ignore = "writes into docs/images; run explicitly"]
fn figure_camera_easing() {
    with_exporter(|exporter| {
        let base = framing_scene();
        let stage = base.stage().stage_rect();
        let start = stage.center();
        let end = Point::new(start.x + stage.width() * 0.25, start.y);

        for (name, ease) in [
            ("camera_ease_linear.png", buzz_scene::Easing::Linear),
            ("camera_ease_smooth.png", buzz_scene::camera_track::SMOOTH),
        ] {
            let mut scene = base.clone();
            scene.camera_mut().enabled = true;
            let mut a = CameraKey::new(0, start);
            a.ease = ease;
            scene.camera_mut().set_key(a);
            scene.camera_mut().set_key(CameraKey::new(48, end));
            // A fifth of the way in: far enough that a linear move has clearly
            // gone somewhere, early enough that an eased one has barely left.
            let f = render(exporter, &scene, 10);
            write(&f, name);
        }
    });
}

// ---------------------------------------------------------------------------
// Line weight
// ---------------------------------------------------------------------------

/// **Thicken and thin, on a drawing with two weights in it.**
///
/// Drawn with a heavy outline and a fine one, because the property worth
/// showing is that the *ratio* between them survives — which is the reason the
/// step multiplies rather than adds, and is invisible on a drawing that has
/// only one weight in it.
#[test]
#[ignore = "writes into docs/images; run explicitly"]
fn figure_line_weight() {
    with_exporter(|exporter| {
        let build = |factor: f64| {
            let mut scene = Scene::default();
            scene.stage_mut().background = Color::from_rgb8(0xF4, 0xF1, 0xEA);
            let layer = scene.add_layer("Line art", LayerKind::Normal);

            // A heavy contour and a fine interior line, the way line art is
            // actually weighted.
            let mut outer = Object::shape(
                ObjectId(1),
                ShapeData::stroked(
                    ellipse(275.0, 200.0, 110.0, 130.0),
                    INK,
                    8.0 * factor,
                ),
            );
            outer.transform = Affine::IDENTITY;
            scene.add_object(layer, outer).expect("a contour");

            for (i, y) in [160.0_f64, 200.0, 240.0].into_iter().enumerate() {
                let mut detail = Object::shape(
                    ObjectId(10 + i as u64),
                    ShapeData::stroked(
                        buzz_geom::Line::new(Point::new(215.0, y), Point::new(335.0, y))
                            .to_path(1e-9),
                        INK,
                        1.5 * factor,
                    ),
                );
                detail.transform = Affine::IDENTITY;
                scene.add_object(layer, detail).expect("a detail line");
            }
            scene.update_layer(layer, |l| { l.frames.insert_frame(2); });
            scene
        };

        // One press thinner, as drawn, and one press thicker: 0.8, 1.0, 1.25.
        for (factor, name) in [
            (0.8, "line_weight_thin.png"),
            (1.0, "line_weight_as_drawn.png"),
            (1.25, "line_weight_thick.png"),
        ] {
            let scene = build(factor);
            let f = render(exporter, &scene, 0);
            write(&f, name);
        }
    });
}

/// Keeps the unused-import warning honest when the file is compiled without
/// running the ignored tests, and documents that a figure scene is an ordinary
/// scene like any other.
#[test]
fn a_figure_scene_is_an_ordinary_scene() {
    let scene = framing_scene();
    assert!(scene.layers().len() >= 3);
    let drawn = scene
        .layers()
        .iter()
        .next()
        .map(|l| {
            l.frames
                .resolved_at(0)
                .iter()
                .filter(|o| matches!(o.kind, ObjectKind::Shape(_)))
                .count()
        })
        .unwrap_or(0);
    assert!(drawn > 0, "the figure scene has nothing drawn in it");
}

// ---------------------------------------------------------------------------
// The head turn
// ---------------------------------------------------------------------------

/// A whole head as **one group**: the form first, then the features painted on
/// it. That order is what the turn reads — backmost is the head, the rest go
/// round the cylinder.
fn turnable_head(scene: &mut Scene) -> ObjectId {
    let layer = scene.add_layer("Head", LayerKind::Normal);
    let mut parts: Vec<std::sync::Arc<Object>> = Vec::new();

    // 0: the form.
    parts.push(std::sync::Arc::new(Object::shape(
        ObjectId(1),
        ShapeData::filled(ellipse(275.0, 200.0, 95.0, 112.0), SKIN),
    )));
    // Hair, sitting on the form and turning with the rest.
    parts.push(std::sync::Arc::new(Object::shape(
        ObjectId(2),
        ShapeData::filled(ellipse(275.0, 118.0, 92.0, 46.0), Color::from_rgb8(0x4A, 0x33, 0x28)),
    )));
    // Eyes.
    for (i, x) in [238.0_f64, 312.0].into_iter().enumerate() {
        parts.push(std::sync::Arc::new(Object::shape(
            ObjectId(10 + i as u64),
            ShapeData::filled(ellipse(x, 180.0, 24.0, 18.0), Color::WHITE),
        )));
        parts.push(std::sync::Arc::new(Object::shape(
            ObjectId(20 + i as u64),
            ShapeData::filled(ellipse(x, 181.0, 9.0, 10.0), IRIS),
        )));
    }
    // A nose on the centre line: the feature that shows a turn most.
    parts.push(std::sync::Arc::new(Object::shape(
        ObjectId(30),
        ShapeData::filled(ellipse(275.0, 214.0, 11.0, 22.0), Color::from_rgb8(0xE0, 0xB8, 0x98)),
    )));
    // A mouth.
    parts.push(std::sync::Arc::new(Object::shape(
        ObjectId(31),
        ShapeData::stroked(ellipse(275.0, 252.0, 30.0, 13.0), INK, 3.0),
    )));

    let id = scene
        .add_object(layer, Object::group(ObjectId(50), parts))
        .expect("a head");
    scene.update_layer(layer, |l| { l.frames.insert_frame(4); });
    id
}

/// **A face turning, from one drawing.**
///
/// Five angles off the same artwork, with nothing drawn twice. The figure is
/// the argument: if the features do not sweep and foreshorten, the feature does
/// not work, and no amount of prose in the guide will cover for it.
#[test]
#[ignore = "writes into docs/images; run explicitly"]
fn figure_head_turn() {
    with_exporter(|exporter| {
        for (yaw, name) in [
            (-0.55_f64, "head_turn_left.png"),
            (-0.28, "head_turn_left_quarter.png"),
            (0.0, "head_turn_front.png"),
            (0.28, "head_turn_right_quarter.png"),
            (0.55, "head_turn_right.png"),
        ] {
            let mut scene = Scene::default();
            scene.stage_mut().background = Color::from_rgb8(0xE8, 0xEC, 0xF2);
            let id = turnable_head(&mut scene);
            scene.update_object_across(0, 4, id, |o| {
                o.spatial.rotation_y = yaw;
                o.modifiers.push(Modifier::Turn { round: 1.0 });
            });
            let f = render(exporter, &scene, 0);
            write(&f, name);
        }
    });
}

// ---------------------------------------------------------------------------
// Tracing a bitmap
// ---------------------------------------------------------------------------

/// **A picture, and the artwork it becomes.**
///
/// Drawn as pixels on purpose — a soft-edged, anti-aliased sketch with a bit of
/// noise in it, because a clean synthetic shape would prove nothing about a
/// real scan.
#[test]
#[ignore = "writes into docs/images; run explicitly"]
fn figure_trace_bitmap() {
    with_exporter(|exporter| {
        // A face-ish doodle in soft grey ink on off-white paper.
        let (w, h) = (220usize, 260usize);
        let ink = |x: f64, y: f64| -> f64 {
            let ring = |cx: f64, cy: f64, rx: f64, ry: f64, t: f64| {
                let d = (((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2)).sqrt();
                (1.0 - ((d - 1.0).abs() / t)).clamp(0.0, 1.0)
            };
            let blob = |cx: f64, cy: f64, r: f64| {
                let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                (1.0 - d / r).clamp(0.0, 1.0)
            };
            ring(110.0, 120.0, 78.0, 96.0, 0.05_f64)
                .max(blob(84.0, 104.0, 11.0))
                .max(blob(136.0, 104.0, 11.0))
                .max(ring(110.0, 168.0, 34.0, 18.0, 0.35_f64))
        };
        let mut pixels = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                // A little deterministic grain, so the trace has to cope with
                // something other than perfectly flat colour.
                let grain = (((x * 7919 + y * 104_729) % 17) as f64 - 8.0) * 1.5;
                let a = ink(x as f64, y as f64);
                let v = ((1.0 - a) * 244.0 + a * 26.0 + grain).clamp(0.0, 255.0) as u8;
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }

        // The picture itself, drawn as a bitmap on the stage.
        let mut before = Scene::default();
        before.stage_mut().background = Color::from_rgb8(0xE8, 0xEC, 0xF2);
        {
            let layer = before.add_layer("Scan", LayerKind::Normal);
            let png = encode_png(w as u32, h as u32, &pixels);
            let asset = before.add_image("Scan", &png).expect("decodes");
            let area = Rect::new(165.0, 55.0, 385.0, 315.0);
            let fill = buzz_scene::ImageFill::new(asset, area);
            let shape = ShapeData {
                path: area.to_path(1e-9),
                fill: Some(buzz_scene::FillSpec::image(fill)),
                stroke: None,
                blend: Default::default(),
            };
            before.add_shape(layer, shape).expect("placed");
            before.update_layer(layer, |l| { l.frames.insert_frame(2); });
        }
        let f = render(exporter, &before, 0);
        write(&f, "trace_before.png");

        // And the same picture traced, drawn as shapes.
        for (options, name, tint) in [
            (buzz_scene::TraceOptions::line_art(), "trace_line_art.png", true),
            (buzz_scene::TraceOptions::default(), "trace_colour.png", false),
        ] {
            let report = buzz_scene::trace(w as u32, h as u32, &pixels, &options);
            eprintln!("  {name}: {}", report.message);
            let mut after = Scene::default();
            after.stage_mut().background = Color::from_rgb8(0xE8, 0xEC, 0xF2);
            let layer = after.add_layer("Traced", LayerKind::Normal);
            let place = Affine::translate((165.0, 55.0))
                * Affine::scale_non_uniform(220.0 / w as f64, 260.0 / h as f64);
            for (i, shape) in report.shapes.iter().enumerate() {
                let mut shape = shape.clone();
                shape.path = place * shape.path.clone();
                // The line-art figure is tinted so the traced *shapes* are
                // visibly shapes rather than a picture of the original.
                if tint && let Some(fill) = shape.fill.as_mut() {
                    fill.paint = buzz_scene::Paint::Solid(Color::from_rgb8(0x1E, 0x3A, 0x6E));
                }
                let _ = i;
                after.add_shape(layer, shape).expect("placed");
            }
            after.update_layer(layer, |l| { l.frames.insert_frame(2); });
            let f = render(exporter, &after, 0);
            write(&f, name);
        }
    });
}

/// Encode RGBA8 as a PNG, so the figure can go through the real import path
/// rather than a back door the users do not have.
fn encode_png(w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(pixels).expect("png body");
    }
    out
}
