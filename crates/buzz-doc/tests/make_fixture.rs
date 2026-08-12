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
