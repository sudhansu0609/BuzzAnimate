//! Builds a document that exercises Phase 4's UI, for looking at by hand.
//!
//! Run with `--ignored` and it writes a `.buzz` next to the target directory
//! that can be opened with `buzzanimate <path>`. It is ignored by default
//! because it writes a file and asserts nothing — it exists so the Library
//! panel, the breadcrumb and the tween spans can be inspected on screen, which
//! is how the font problems in Phase 2 were caught.

use buzz_geom::{Affine, Shape as _};
use buzz_scene::{
    LayerKind, Scene, ShapeData, SymbolKind, Tween,
};
use kurbo::Rect;
use peniko::Color;

#[test]
#[ignore = "writes a file for manual inspection"]
fn write_phase4_fixture() {
    let mut scene = Scene::default();
    let layer = scene.layers().iter().next().unwrap().id;

    // A library with folders at two depths, so the tree has something to show.
    let body = scene.add_symbol("Hero Body", SymbolKind::Graphic, Some("Characters"));
    let arm = scene.add_symbol("Hero Arm", SymbolKind::Graphic, Some("Characters/Hero"));
    let _loop_clip = scene.add_symbol("Background Loop", SymbolKind::MovieClip, None);
    let button = scene.add_symbol("Play Button", SymbolKind::Button, Some("UI"));
    scene.library_mut().add_folder("UI/Icons");

    // Artwork inside two of them, so their instances draw something.
    for (id, colour, rect) in [
        (body, Color::from_rgba8(0xE0, 0x70, 0x50, 0xFF), Rect::new(0.0, 0.0, 80.0, 120.0)),
        (arm, Color::from_rgba8(0x50, 0xA0, 0xE0, 0xFF), Rect::new(0.0, 0.0, 25.0, 90.0)),
        (button, Color::from_rgba8(0x80, 0xC0, 0x80, 0xFF), Rect::new(0.0, 0.0, 60.0, 30.0)),
    ] {
        let inner = scene.library().get(id).unwrap().layers.iter().next().unwrap().id;
        let object = buzz_scene::Object::shape(
            scene.next_object_id(),
            ShapeData::filled(rect.to_path(1e-9), colour),
        );
        scene.library_mut().update(id, |s| {
            s.layers.update(inner, |l| {
                l.frames.set_objects(0, vec![std::sync::Arc::new(object)]);
            });
        });
    }

    // Instances on the stage, one of them faded so the colour effect shows.
    let placed = scene
        .add_instance_at(layer, 0, body, Affine::translate((120.0, 90.0)))
        .unwrap();
    scene.add_instance_at(layer, 0, button, Affine::translate((320.0, 300.0)));
    scene.update_object(placed, |o| {
        if let buzz_scene::ObjectKind::Instance(i) = &mut o.kind {
            i.color = buzz_scene::ColorTransform::tint(Color::from_rgba8(255, 0, 0, 255), 0.4);
        }
    });

    // One layer per tween kind, plus a broken one, so the timeline shows all
    // four ways a span can be drawn at once.
    for (name, tween, complete) in [
        ("Motion", Tween::motion(), true),
        ("Classic", Tween::classic(), true),
        ("Shape", Tween::shape(), true),
        ("Broken", Tween::motion(), false),
    ] {
        let id = scene.add_layer(name, LayerKind::Normal);
        let shape = ShapeData::filled(
            Rect::new(0.0, 0.0, 40.0, 40.0).to_path(1e-9),
            Color::from_rgba8(0xC0, 0xC0, 0xC0, 0xFF),
        );
        scene.add_shape(id, shape);
        scene.update_layer(id, |l| {
            l.frames.insert_frame(23);
            if complete {
                l.frames.insert_keyframe(12);
            }
            l.frames.set_tween(0, tween);
        });
    }

    // The temp directory, not the crate directory: this is a scratch file for
    // looking at, and it must not end up in the repository.
    let mut path = std::env::temp_dir();
    path.push("phase4-fixture.buzz");
    buzz_doc::format::save(&scene, &path).expect("the fixture saves");
    println!("wrote {}", path.display());
}

/// A document showing build-up paint against ordinary compositing.
///
/// Two rows of crossing translucent bars: the top row composites normally, the
/// bottom row builds up. The overlaps should differ visibly — 0.44 against
/// 0.50 — and the file itself proves the blend survives a save and reload,
/// which is what format version 4 added.
#[test]
#[ignore = "writes a file for manual inspection"]
fn write_build_up_fixture() {
    use buzz_scene::PaintBlend;

    let mut scene = Scene::default();
    let normal_layer = scene.layers().iter().next().unwrap().id;
    scene.update_layer(normal_layer, |l| l.name = "Normal".into());
    let build_up_layer = scene.add_layer("Build Up", LayerKind::Normal);

    let ink = |alpha: f64| Color::from_rgba8(0x10, 0x30, 0x80, (alpha * 255.0).round() as u8);

    for (layer, top, blend) in [
        (normal_layer, 40.0, PaintBlend::Normal),
        (build_up_layer, 220.0, PaintBlend::Additive),
    ] {
        // A horizontal bar at 0.2 and a vertical one at 0.3, crossing.
        for (rect, alpha) in [
            (Rect::new(60.0, top + 50.0, 480.0, top + 100.0), 0.2),
            (Rect::new(220.0, top, 320.0, top + 150.0), 0.3),
        ] {
            scene.add_shape(
                layer,
                ShapeData::filled(rect.to_path(1e-9), ink(alpha)).with_blend(blend),
            );
        }
    }

    let mut path = std::env::temp_dir();
    path.push("build-up-fixture.buzz");
    buzz_doc::format::save(&scene, &path).expect("the fixture saves");
    println!("wrote {}", path.display());
}

/// A document arranged in depth, for looking at the parallax by hand.
///
/// Five layers from far behind the stage to just in front of it, each a band
/// of squares. Scrubbing the camera keyframes sweeps the near layers past the
/// far ones, which is the effect layer depth exists to produce.
#[test]
#[ignore = "writes a file for manual inspection"]
fn write_depth_fixture() {
    let mut scene = Scene::default();

    // Back to front, so the nearest layer ends up at the top of the stack and
    // therefore paints last.
    let bands = [
        ("Sky", 2400.0, Color::from_rgb8(0x9F, 0xC5, 0xE8)),
        ("Hills", 1200.0, Color::from_rgb8(0x6F, 0xA8, 0xDC)),
        ("Trees", 400.0, Color::from_rgb8(0x38, 0x76, 0x1D)),
        ("Stage", 0.0, Color::from_rgb8(0xE0, 0x6C, 0x3B)),
        ("Foreground", -400.0, Color::from_rgb8(0x20, 0x20, 0x28)),
    ];

    let first = scene.layers().iter().next().unwrap().id;
    scene.update_layer(first, |l| l.name = "Sky".into());

    for (index, (name, depth, colour)) in bands.iter().enumerate() {
        let layer = if index == 0 {
            first
        } else {
            scene.add_layer(*name, LayerKind::Normal)
        };
        scene.update_layer(layer, |l| l.depth = *depth);

        // The layer has to last as long as the camera move, or scrubbing to
        // the end shows an empty stage.
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(48);
        });

        // A row of squares, so the horizontal sweep is easy to follow.
        let y = 120.0 + index as f64 * 30.0;
        for column in 0..7 {
            let x = -200.0 + column as f64 * 160.0;
            scene.add_shape(
                layer,
                ShapeData::filled(Rect::new(x, y, x + 90.0, y + 60.0).to_path(1e-9), *colour),
            );
        }
    }

    // A camera that pans right across two seconds, so the parallax is visible
    // by scrubbing rather than only by moving the depth sliders.
    scene.camera_mut().enabled = true;
    scene
        .camera_mut()
        .set_key(buzz_scene::CameraKey::new(0, buzz_geom::Point::new(275.0, 200.0)));
    scene
        .camera_mut()
        .set_key(buzz_scene::CameraKey::new(48, buzz_geom::Point::new(675.0, 200.0)));

    let mut path = std::env::temp_dir();
    path.push("depth-fixture.buzz");
    buzz_doc::format::save(&scene, &path).expect("the fixture saves");
    println!("wrote {}", path.display());
}
