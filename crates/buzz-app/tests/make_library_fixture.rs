//! Write a document with a library of visibly different symbols, for looking
//! at by hand.
//!
//! ```sh
//! cargo test -p buzz-app --test make_library_fixture -- --ignored --nocapture
//! ```
//!
//! Ignored by default because it writes a file and exists to be *looked at*:
//! whether the Library's thumbnails are drawn, framed and recognisable is the
//! kind of thing only a picture settles. Every symbol here is a different
//! shape and colour on purpose — a library of identical squares would prove
//! that something rendered, not that the right something rendered.

use std::sync::Arc;

use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_scene::{LayerKind, Object, ObjectId, Scene, ShapeData, SymbolKind};
use kurbo::{BezPath, Circle, Ellipse};
use peniko::Color;

fn shape_object(scene: &mut Scene, data: ShapeData) -> Object {
    Object::shape(scene.next_object_id(), data)
}

/// Put one drawing inside a new symbol, and return its id.
fn symbol_from(scene: &mut Scene, name: &str, data: ShapeData) -> buzz_scene::SymbolId {
    let id = scene.add_symbol(name, SymbolKind::Graphic, None);
    let layer = scene
        .library()
        .get(id)
        .expect("just added")
        .layers
        .iter()
        .next()
        .expect("a symbol starts with a layer")
        .id;
    let art = shape_object(scene, data);
    scene.library_mut().update(id, |s| {
        s.layers.update(layer, |l| {
            l.frames.set_objects(0, vec![Arc::new(art)]);
        });
    });
    id
}

fn star(centre: Point, outer: f64, inner: f64, points: usize) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..points * 2 {
        let r = if i % 2 == 0 { outer } else { inner };
        let a = std::f64::consts::PI * i as f64 / points as f64 - std::f64::consts::FRAC_PI_2;
        let p = Point::new(centre.x + r * a.cos(), centre.y + r * a.sin());
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path.close_path();
    path
}

#[test]
#[ignore = "writes a file to look at"]
fn write_library_fixture() {
    let mut scene = Scene::default();

    // Six symbols, each a different shape *and* colour, so a row of
    // thumbnails can be told apart at a glance — which is the whole point.
    let kinds: Vec<(&str, ShapeData)> = vec![
        (
            "Red Square",
            ShapeData::filled(
                Rect::new(0.0, 0.0, 120.0, 120.0).to_path(1e-9),
                Color::from_rgb8(0xD9, 0x3A, 0x3A),
            ),
        ),
        (
            "Blue Circle",
            ShapeData::filled(
                Circle::new(Point::new(60.0, 60.0), 60.0).to_path(1e-9),
                Color::from_rgb8(0x2F, 0x6F, 0xD0),
            ),
        ),
        (
            "Green Star",
            ShapeData::filled(
                star(Point::new(60.0, 60.0), 62.0, 26.0, 5),
                Color::from_rgb8(0x3F, 0xA8, 0x55),
            ),
        ),
        (
            "Wide Ellipse",
            ShapeData::filled(
                Ellipse::new(Point::new(90.0, 40.0), (90.0, 30.0), 0.0).to_path(1e-9),
                Color::from_rgb8(0xE0, 0x8A, 0x1E),
            ),
        ),
        (
            "Tall Bar",
            ShapeData::filled(
                Rect::new(0.0, 0.0, 30.0, 200.0).to_path(1e-9),
                Color::from_rgb8(0x8E, 0x44, 0xC4),
            ),
        ),
        (
            "Tiny Dot",
            ShapeData::filled(
                Circle::new(Point::new(4.0, 4.0), 4.0).to_path(1e-9),
                Color::from_rgb8(0x1B, 0xB8, 0xB0),
            ),
        ),
    ];

    let mut ids = Vec::new();
    for (name, data) in kinds {
        ids.push(symbol_from(&mut scene, name, data));
    }

    // A nested symbol, because a thumbnail has to draw instances too — this is
    // the case that would silently come out blank if `draw_symbol` walked only
    // the shapes it found directly.
    let group = scene.add_symbol("Two Together", SymbolKind::Graphic, None);
    let group_layer = scene
        .library()
        .get(group)
        .expect("just added")
        .layers
        .iter()
        .next()
        .expect("a symbol starts with a layer")
        .id;
    let mut inside = Vec::new();
    for (i, id) in ids.iter().take(2).enumerate() {
        let mut instance = Object::instance_of(scene.next_object_id(), *id);
        instance.transform = Affine::translate((i as f64 * 90.0, i as f64 * 40.0));
        inside.push(Arc::new(instance));
    }
    scene.library_mut().update(group, |s| {
        s.layers.update(group_layer, |l| {
            l.frames.set_objects(0, inside);
        });
    });

    // One instance on the stage, so the document opens on something.
    let layer = scene.layers().iter().next().expect("a layer").id;
    let mut placed = Object::instance_of(scene.next_object_id(), ids[2]);
    placed.transform = Affine::translate((820.0, 460.0));
    scene.add_object_at(layer, 0, placed);

    let path = std::env::temp_dir().join("buzzanimate-library-fixture.buzz");
    let mut doc = buzz_doc::Document::new(scene);
    doc.save_as(&path).expect("write the fixture");
    println!("wrote {}", path.display());
}

/// Keeps `ObjectId` in use for the helper above without a warning.
#[allow(dead_code)]
fn _types(_: ObjectId, _: LayerKind) {}
