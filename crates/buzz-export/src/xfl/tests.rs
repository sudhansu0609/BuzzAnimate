//! **The writer against the reader.**
//!
//! A `.fla` this program cannot open again is a `.fla` nobody should trust
//! Animate with, so every test here writes a document and reads it back
//! through `buzz_import_xfl` — the same parser that reads Adobe's own files,
//! which has been tested against them. It is the strongest check available
//! without Animate itself, and it catches the failure that matters most: a
//! file that opens *empty*.

use super::*;
use buzz_geom::{Point, Rect, Shape as _};
use buzz_scene::{LayerKind, ShapeData};

fn square(x: f64, y: f64, size: f64) -> buzz_geom::BezPath {
    Rect::new(x, y, x + size, y + size).to_path(1e-9)
}

/// Write, then read back with the importer.
fn round_trip(scene: &Scene) -> (Scene, FlaReport) {
    let (bytes, report) = fla_bytes(scene).expect("the document should write");
    let (back, import) =
        buzz_import_xfl::import_fla_bytes(&bytes).expect("and read back as a .fla");
    assert!(
        import.is_complete(),
        "the importer had trouble with our own file: {}",
        import.summary()
    );
    (back, report)
}

#[test]
fn an_empty_document_writes_a_file_that_opens() {
    let scene = Scene::default();
    let (back, _) = round_trip(&scene);
    assert_eq!(
        back.stage().size.width,
        scene.stage().size.width,
        "the stage should come back the size it went out"
    );
}

/// The stage's own properties, which everything else is drawn against.
#[test]
fn the_stage_survives() {
    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(1280.0, 720.0);
    scene.stage_mut().frame_rate = 30.0;
    scene.stage_mut().background = Color::from_rgba8(0x20, 0x30, 0x40, 0xFF);

    let (back, _) = round_trip(&scene);
    assert_eq!(back.stage().size.width, 1280.0);
    assert_eq!(back.stage().size.height, 720.0);
    assert_eq!(back.stage().frame_rate, 30.0);
    assert_eq!(
        back.stage().background.to_rgba8().to_u8_array()[..3],
        [0x20, 0x30, 0x40]
    );
}

/// Artwork has to arrive **where it was** and **the colour it was**. A file
/// that opens with the drawing in the wrong place is the failure this whole
/// module exists to avoid.
#[test]
fn a_shape_keeps_its_place_and_its_colour() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(square(120.0, 80.0, 60.0), Color::from_rgba8(0xE0, 0x40, 0x60, 0xFF)),
    );

    let (back, _) = round_trip(&scene);
    let objects: Vec<_> = back
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(0).iter().cloned())
        .collect();
    assert_eq!(objects.len(), 1, "one shape out, one shape back");

    let bounds = objects[0].bounds();
    assert!(
        (bounds.x0 - 120.0).abs() < 0.1
            && (bounds.y0 - 80.0).abs() < 0.1
            && (bounds.width() - 60.0).abs() < 0.1,
        "the square came back at {bounds:?}"
    );
}

/// Layer names, order and switches.
#[test]
fn layers_keep_their_names_and_switches() {
    let mut scene = Scene::default();
    let top = scene.add_layer("Top", LayerKind::Normal);
    let bottom = scene.add_layer("Bottom", LayerKind::Normal);
    scene.update_layer(bottom, |l| l.locked = true);
    scene.update_layer(top, |l| l.visible = false);
    for id in [top, bottom] {
        scene.add_shape(id, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
    }

    let (back, _) = round_trip(&scene);
    let names: Vec<String> = back.layers().iter().map(|l| l.name.clone()).collect();
    assert!(
        names.contains(&"Top".to_string()) && names.contains(&"Bottom".to_string()),
        "got {names:?}"
    );
    let by_name = |want: &str| {
        back.layers()
            .iter()
            .find(|l| l.name == want)
            .expect("the layer")
            .clone()
    };
    assert!(!by_name("Top").visible, "a hidden layer stays hidden");
    assert!(by_name("Bottom").locked, "a locked layer stays locked");
}

/// **A rigged character stays rigged.** This is the round trip that matters
/// for the work: parenting comes in from Animate, and it has to go back.
#[test]
fn layer_parenting_goes_back_out() {
    let mut scene = Scene::default();
    let body = scene.add_layer("body", LayerKind::Normal);
    let arm = scene.add_layer("arm", LayerKind::Normal);
    scene.add_shape(body, ShapeData::filled(square(0.0, 0.0, 40.0), Color::WHITE));
    scene.add_shape(arm, ShapeData::filled(square(50.0, 0.0, 20.0), Color::WHITE));
    scene.update_layer(arm, |l| l.follows = Some(body));

    let (back, _) = round_trip(&scene);
    let find = |want: &str| {
        back.layers()
            .iter()
            .find(|l| l.name == want)
            .expect("the layer")
            .clone()
    };
    let (body, arm) = (find("body"), find("arm"));
    assert_eq!(
        arm.follows,
        Some(body.id),
        "the arm should still follow the body"
    );
}

/// Symbols and the instances that place them.
#[test]
fn a_symbol_and_its_instance_survive() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Cast", LayerKind::Normal);
    let symbol = scene.add_symbol("Hero", SymbolKind::Graphic, None);
    let inner = scene
        .library()
        .get(symbol)
        .and_then(|s| s.layers.iter().next())
        .map(|l| l.id)
        .expect("a layer inside the symbol");
    let art = scene.next_object_id();
    let art = Object::shape(
        art,
        ShapeData::filled(square(0.0, 0.0, 30.0), Color::from_rgba8(0x30, 0xC0, 0x60, 0xFF)),
    );
    scene.library_mut().update(symbol, |s| {
        s.layers.update(inner, |l| {
            l.frames.set_objects(0, vec![std::sync::Arc::new(art)]);
        });
    });
    scene.add_instance_at(layer, 0, symbol, Affine::translate((200.0, 140.0)));

    let (back, report) = round_trip(&scene);
    assert_eq!(report.symbols, 1, "one symbol written");
    assert!(
        back.library().find_by_name("Hero").is_some(),
        "the symbol should be in the library that comes back"
    );

    // And the instance is placed where it was put.
    let placed: Vec<_> = back
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(0).iter().cloned())
        .filter(|o| o.instance().is_some())
        .collect();
    assert_eq!(placed.len(), 1, "one instance out, one back");
    let c = placed[0].transform.as_coeffs();
    assert!(
        (c[4] - 200.0).abs() < 0.1 && (c[5] - 140.0).abs() < 0.1,
        "the instance came back at {:?}",
        (c[4], c[5])
    );
}

/// Keyframes and how long they last.
#[test]
fn keyframes_and_their_spans_survive() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(layer, ShapeData::filled(square(0.0, 0.0, 20.0), Color::WHITE));
    scene.edit_layers().update(layer, |l| {
        l.frames.insert_keyframe(10);
        l.frames.insert_frame(20);
    });

    let (back, _) = round_trip(&scene);
    let art = back
        .layers()
        .iter()
        .find(|l| l.name == "Art")
        .expect("the layer");
    let starts: Vec<u32> = art.frames.keyframes().iter().map(|k| k.start).collect();
    assert!(
        starts.contains(&0) && starts.contains(&10),
        "both keyframes should come back, got {starts:?}"
    );
    assert!(
        art.frames.length() >= 20,
        "the span should reach frame 20, got {}",
        art.frames.length()
    );
}

/// **What cannot travel is said out loud.** An exporter that quietly drops a
/// document's gradients is a trap.
#[test]
fn what_cannot_travel_is_reported() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let gradient =
        buzz_scene::Gradient::linear(Color::WHITE, Color::BLACK, Rect::new(0.0, 0.0, 50.0, 50.0));
    scene.add_shape(
        layer,
        ShapeData {
            path: square(0.0, 0.0, 50.0),
            fill: Some(buzz_scene::FillSpec {
                paint: buzz_scene::Paint::Gradient(Box::new(gradient).into()),
                rule: buzz_geom::FillMode::NonZero,
            }),
            stroke: None,
            blend: buzz_scene::PaintBlend::Normal,
        },
    );

    let (_, report) = round_trip(&scene);
    assert!(
        report.skipped.iter().any(|s| s.contains("gradient")),
        "the report should name gradients, got {:?}",
        report.skipped
    );
    assert!(
        report.summary().contains("not carried"),
        "and say so in its summary: {}",
        report.summary()
    );
}

/// A symbol whose name would be an illegal file name must not write outside
/// `LIBRARY/`, nor produce a package that will not open.
#[test]
fn an_awkward_symbol_name_is_still_written_safely() {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Cast", LayerKind::Normal);
    let symbol = scene.add_symbol("../../etc/passwd", SymbolKind::Graphic, None);
    scene.add_instance_at(layer, 0, symbol, Affine::IDENTITY);

    let (bytes, _) = fla_bytes(&scene).expect("it should still write");
    let reader =
        zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("a readable package");
    for name in reader.file_names() {
        assert!(
            !name.contains(".."),
            "no entry may escape the package: {name}"
        );
    }
}
