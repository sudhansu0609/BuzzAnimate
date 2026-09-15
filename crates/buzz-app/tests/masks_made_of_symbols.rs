//! **A mask made of a symbol instance clips.**
//!
//! Every Animate character in the asset library masks its eyeballs the same
//! way: a mask layer holding an *instance* of the eye-white symbol, over a
//! masked layer holding the eyeballs. The eye white is a symbol because it is
//! drawn once and used in every eye, and the animator dragged that symbol onto
//! the mask layer rather than redrawing it there.
//!
//! The mask's clip region used to be built by flattening the mask layer's
//! objects, and flattening skips instances — it has no library to open them
//! with. So the region was empty, no clip was opened, and the eyeballs were
//! drawn whole over the face. "The masked layers do not remain masked" was
//! exactly right, and it was every character.
//!
//! Both places a mask can live are covered: on the stage, and inside a symbol,
//! which is where a character keeps its eyes.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{Layer, LayerId, LayerKind, Object, Scene, ShapeData, SymbolId, SymbolKind};
use kurbo::Rect;
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping mask test: no usable GPU ({e})"),
    }
}

fn square(scene: &mut Scene, size: f64, color: Color) -> Object {
    Object::shape(
        scene.next_object_id(),
        ShapeData::filled(Rect::new(0.0, 0.0, size, size).to_path(1e-9), color),
    )
}

/// A library symbol holding one filled square at its origin — the eye white.
fn square_symbol(scene: &mut Scene, name: &str, size: f64) -> SymbolId {
    let id = scene.add_symbol(name, SymbolKind::Graphic, None);
    let layer = scene
        .library()
        .get(id)
        .unwrap()
        .layers
        .iter()
        .next()
        .unwrap()
        .id;
    let art = square(scene, size, Color::WHITE);
    scene.library_mut().update(id, |s| {
        s.layers.update(layer, |l| {
            l.frames.set_objects(0, vec![Arc::new(art)]);
        });
    });
    id
}

/// Where the eye white sits, and how big it is. The ink under it covers the
/// whole stage, so anything outside this square that is still inked is a mask
/// that did not clip.
const EYE: (f64, f64, f64) = (200.0, 150.0, 100.0);

fn inside_eye() -> (u32, u32) {
    ((EYE.0 + EYE.2 / 2.0) as u32, (EYE.1 + EYE.2 / 2.0) as u32)
}

fn outside_eye() -> (u32, u32) {
    (30, 30)
}

/// The mask on the stage: an eye-white instance on a locked mask layer, over a
/// masked layer of ink covering the whole stage.
fn masked_on_the_stage() -> Scene {
    let mut scene = Scene::default();
    let stage = scene.stage().size;
    let white = square_symbol(&mut scene, "Eye White", EYE.2);

    let ink = scene.add_layer("Ink", LayerKind::Masked);
    let black = square(&mut scene, stage.width.max(stage.height), Color::BLACK);
    scene.add_object_at(ink, 0, black);

    let mask = scene.add_layer("Eye", LayerKind::Mask);
    scene.add_instance_at(mask, 0, white, Affine::translate((EYE.0, EYE.1)));
    // Locked, which is Animate's rule for a stage mask being in force.
    scene.edit_layers().update(mask, |l| l.locked = true);
    scene
}

/// The same eye, but inside a Head symbol placed on the stage — which is how
/// every character carries its own.
fn masked_inside_a_symbol() -> Scene {
    let mut scene = Scene::default();
    let stage = scene.stage().size;
    let white = square_symbol(&mut scene, "Eye White", EYE.2);

    let head = scene.add_symbol("Head", SymbolKind::Graphic, None);
    let ink_layer = scene
        .library()
        .get(head)
        .unwrap()
        .layers
        .iter()
        .next()
        .unwrap()
        .id;
    let black = square(&mut scene, stage.width.max(stage.height), Color::BLACK);
    let eye = Object::instance_of(scene.next_object_id(), white)
        .with_transform(Affine::translate((EYE.0, EYE.1)));
    let mask_id = LayerId(scene.next_object_id().0);
    scene.library_mut().update(head, |s| {
        s.layers.update(ink_layer, |l| {
            l.kind = LayerKind::Masked;
            l.frames.set_objects(0, vec![Arc::new(black)]);
        });
        let mut mask = Layer::new(mask_id, "Eye", LayerKind::Mask);
        mask.frames.set_objects(0, vec![Arc::new(eye)]);
        s.layers.insert(0, mask);
    });

    let layer = scene.layers().iter().next().unwrap().id;
    scene.add_instance_at(layer, 0, head, Affine::IDENTITY);
    scene
}

fn is_dark(p: [u8; 4]) -> bool {
    p[0] < 60 && p[1] < 60 && p[2] < 60
}

fn check(exporter: &mut Exporter, scene: &Scene, what: &str) {
    let settings = ExportSettings::for_stage(scene);
    let frame: Frame = exporter.render(scene, 0, &settings).expect(what);
    let (ix, iy) = inside_eye();
    let (ox, oy) = outside_eye();
    assert!(
        is_dark(frame.pixel(ix, iy)),
        "{what}: the ink inside the eye should show through the mask, got {:?}",
        frame.pixel(ix, iy)
    );
    assert!(
        !is_dark(frame.pixel(ox, oy)),
        "{what}: the ink outside the eye was drawn — the mask clipped nothing, got {:?}",
        frame.pixel(ox, oy)
    );
}

#[test]
fn a_symbol_instance_on_a_stage_mask_layer_clips() {
    with_exporter(|exporter| check(exporter, &masked_on_the_stage(), "stage mask"));
}

#[test]
fn a_symbol_instance_on_a_mask_layer_inside_a_symbol_clips() {
    with_exporter(|exporter| check(exporter, &masked_inside_a_symbol(), "symbol mask"));
}
