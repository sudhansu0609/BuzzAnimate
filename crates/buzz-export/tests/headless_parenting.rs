//! Prove layer parenting on the real GPU: move the body, and the head goes
//! with it — in the picture, not just in the arithmetic.
//!
//! A rig that only works in a unit test is worthless; what matters is whether
//! the pixels of the child layer land somewhere else when the parent moves.
//! These render frames and read them back to say so.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerId, LayerKind, Scene, ShapeData};
use peniko::Color;

const BODY: Color = Color::from_rgb8(0x20, 0x40, 0xC0);
const HEAD: Color = Color::from_rgb8(0xC0, 0x30, 0x30);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping parenting test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A body square that slides 200 to the right between frames 0 and 10, and a
/// head square above it that is never keyed at all.
fn character() -> (Scene, LayerId, LayerId) {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;

    let body = scene.add_layer("Body", LayerKind::Normal);
    let head = scene.add_layer("Head", LayerKind::Normal);

    let square = |x0: f64, y0: f64, color: Color| {
        ShapeData::filled(Rect::new(x0, y0, x0 + 80.0, y0 + 80.0).to_path(1e-9), color)
    };
    scene.add_shape(body, square(100.0, 240.0, BODY));
    scene.add_shape(head, square(110.0, 150.0, HEAD));

    // The body moves; nothing at all is keyed on the head.
    let body_object = scene
        .layers()
        .get(body)
        .and_then(|l| l.objects_at(0).first().map(|o| o.id))
        .expect("the body");

    // Both layers run the full ten frames. A layer's span is one frame long
    // until something extends it, and a layer that does not reach frame 10 is
    // simply not drawn there — which would make the head's absence look like a
    // parenting failure.
    scene.update_layer(head, |l| {
        l.frames.insert_frame(10);
    });
    scene.update_layer(body, |l| {
        // Extend the span first, as F5 does. A keyframe made past the end of a
        // span comes up blank — PROGRESS.md §7 item 37 — and a body with no
        // artwork on frame 10 has no motion for anything to follow.
        l.frames.insert_frame(10);
        l.frames.insert_keyframe(10);
    });
    scene.update_object_at(10, body_object, |o| {
        o.transform = Affine::translate((200.0, 0.0));
    });

    (scene, body, head)
}

fn render(exporter: &mut Exporter, scene: &Scene, frame: u32) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, frame, &settings).expect("render")
}

/// Is this pixel that colour, near enough?
fn is(pixel: [u8; 4], colour: Color) -> bool {
    let [r, g, b, _] = colour.to_rgba8().to_u8_array();
    (pixel[0] as i32 - r as i32).abs() < 24
        && (pixel[1] as i32 - g as i32).abs() < 24
        && (pixel[2] as i32 - b as i32).abs() < 24
}

/// The promise every existing document depends on: no parenting, no change.
#[test]
fn a_document_without_parenting_renders_exactly_as_before() {
    with_exporter(|exporter| {
        let (scene, body, head) = character();
        let before = render(exporter, &scene, 10);

        // A link set and cleared again must leave the picture untouched.
        let mut linked = scene.clone();
        linked.update_layer(head, |l| l.follows = Some(body));
        linked.update_layer(head, |l| l.follows = None);

        assert_eq!(
            before.pixels,
            render(exporter, &linked, 10).pixels,
            "a document that follows nothing must be pixel-identical"
        );
    });
}

/// The whole point: the head is never keyed, and it moves anyway.
#[test]
fn a_head_follows_the_body_it_is_parented_to() {
    with_exporter(|exporter| {
        let (mut scene, body, head) = character();
        scene.update_layer(head, |l| l.follows = Some(body));
        // At the first keyframe nothing has moved, so the head is where it was
        // drawn — parenting must never shift artwork the moment it is linked.
        let start = render(exporter, &scene, 0);
        assert!(
            is(start.pixel(150, 190), HEAD),
            "the head moved as soon as it was parented: {:?}",
            start.pixel(150, 190)
        );

        // Ten frames later the body has travelled 200 to the right, and the
        // head has gone with it.
        let moved = render(exporter, &scene, 10);
        assert!(
            is(moved.pixel(350, 190), HEAD),
            "the head did not follow the body: {:?}",
            moved.pixel(350, 190)
        );
        assert!(
            !is(moved.pixel(150, 190), HEAD),
            "the head was left behind as well as moved"
        );
        assert!(
            is(moved.pixel(340, 280), BODY),
            "the body itself should be at the far end: {:?}",
            moved.pixel(340, 280)
        );
    });
}

/// Halfway through a tween the head is halfway too: parenting follows the
/// interpolated motion, not just the keyframes.
#[test]
fn a_following_layer_moves_with_a_tween() {
    with_exporter(|exporter| {
        let (mut scene, body, head) = character();
        scene.update_layer(head, |l| l.follows = Some(body));
        scene.update_layer(body, |l| {
            l.frames.set_tween(0, buzz_scene::Tween::classic());
        });

        let middle = render(exporter, &scene, 5);
        assert!(
            is(middle.pixel(250, 190), HEAD),
            "the head should be half way across: {:?}",
            middle.pixel(250, 190)
        );
    });
}

/// A chain: hat follows head follows body, and every link adds its motion.
#[test]
fn motion_accumulates_down_a_chain_on_the_gpu() {
    with_exporter(|exporter| {
        let (mut scene, body, head) = character();
        let hat = scene.add_layer("Hat", LayerKind::Normal);
        scene.add_shape(
            hat,
            ShapeData::filled(
                Rect::new(120.0, 100.0, 180.0, 140.0).to_path(1e-9),
                Color::from_rgb8(0x20, 0x90, 0x40),
            ),
        );
        scene.update_layer(hat, |l| {
            l.frames.insert_frame(10);
        });

        // The head moves up by 40 of its own accord, and follows the body.
        let head_object = scene
            .layers()
            .get(head)
            .and_then(|l| l.objects_at(0).first().map(|o| o.id))
            .expect("the head");
        scene.update_layer(head, |l| {
            l.frames.insert_frame(10);
            l.frames.insert_keyframe(10);
        });
        scene.update_object_at(10, head_object, |o| {
            o.transform = Affine::translate((0.0, -40.0));
        });

        scene.update_layer(head, |l| l.follows = Some(body));
        scene.update_layer(hat, |l| l.follows = Some(head));

        // The hat inherits the body's 200 across *and* the head's 40 up.
        let moved = render(exporter, &scene, 10);
        let hat_colour = Color::from_rgb8(0x20, 0x90, 0x40);
        assert!(
            is(moved.pixel(350, 80), hat_colour),
            "the hat should have inherited both motions: {:?}",
            moved.pixel(350, 80)
        );
    });
}
