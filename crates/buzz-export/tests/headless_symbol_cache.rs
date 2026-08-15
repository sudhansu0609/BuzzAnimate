//! **The symbol-encoding cache draws the same picture, only faster.**
//!
//! An instance-heavy Animate import is laggy because a symbol placed many times
//! is re-encoded once per instance. The cache encodes each eligible symbol once
//! and stamps it per instance (see `buzz-render/src/document.rs`). That stamp is
//! precision-critical and shares the render split, so the only honest proof is
//! pixels: render the same document with the cache off and on, and compare.
//!
//! Parity is tolerance-based, not byte-exact: the stamped path takes one more
//! `f32` rounding than the live one, so a handful of edge pixels may differ by a
//! least significant bit or two. The gate is max channel difference ≤ 2 and at
//! least 99.9% of pixels identical. Each case also asserts the cache actually
//! engaged, so a mistake that quietly disabled it could not pass this vacuously.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    Gradient, Layer, LayerId, LayerKind, Object, ObjectId, ObjectKind, PaintBlend, Scene,
    ShapeData, SymbolId, SymbolKind,
};
use peniko::Color;

const RED: Color = Color::from_rgb8(0xE0, 0x20, 0x20);
const BLUE: Color = Color::from_rgb8(0x20, 0x40, 0xE0);
const GREEN: Color = Color::from_rgb8(0x20, 0xC0, 0x40);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping symbol-cache parity test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// Render `scene` with the cache off and on and assert the pixels match within
/// tolerance. Also assert the cache engaged unless `expect_engage` is false.
fn assert_parity(exporter: &mut Exporter, scene: &Scene, expect_engage: bool) {
    let settings = ExportSettings::for_stage(scene);

    // Each fixture is an unrelated document; render it from a clean cache.
    exporter.reset_caches();
    exporter.set_symbol_reuse(false);
    let off = exporter.render(scene, 0, &settings).expect("render, cache off");

    let before = exporter.symbol_cache_stats();
    exporter.set_symbol_reuse(true);
    let on = exporter.render(scene, 0, &settings).expect("render, cache on");
    let after = exporter.symbol_cache_stats();

    if expect_engage {
        assert!(
            after.1 > before.1,
            "the cache never stamped anything — this parity test is vacuous"
        );
    }

    assert_eq!(
        (off.width, off.height),
        (on.width, on.height),
        "the two renders differ in size"
    );
    let px = (off.width * off.height) as usize;
    let mut max_diff = 0i32;
    let mut exact = 0usize;
    for p in 0..px {
        let o = &off.pixels[p * 4..p * 4 + 4];
        let n = &on.pixels[p * 4..p * 4 + 4];
        let mut same = true;
        for c in 0..4 {
            let d = (o[c] as i32 - n[c] as i32).abs();
            max_diff = max_diff.max(d);
            same &= d == 0;
        }
        exact += same as usize;
    }
    let frac = exact as f64 / px as f64;
    assert!(max_diff <= 2, "cache changed a pixel by {max_diff} (> 2 LSB)");
    assert!(
        frac >= 0.999,
        "only {:.4}% of pixels identical with the cache on",
        frac * 100.0
    );
}

// --- fixture builders -------------------------------------------------------

fn white_stage() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene
}

fn first_layer(scene: &Scene, symbol: SymbolId) -> LayerId {
    scene
        .library()
        .get(symbol)
        .unwrap()
        .layers
        .iter()
        .next()
        .unwrap()
        .id
}

/// A one-layer graphic symbol holding `objects`.
fn symbol_of(scene: &mut Scene, name: &str, objects: Vec<Object>) -> SymbolId {
    let id = scene.add_symbol(name, SymbolKind::Graphic, None);
    let layer = first_layer(scene, id);
    scene.library_mut().update(id, |s| {
        s.layers.update(layer, |l| {
            l.frames
                .set_objects(0, objects.into_iter().map(Arc::new).collect());
        });
    });
    id
}

fn filled(id: u64, rect: Rect, color: Color) -> Object {
    Object::shape(ObjectId(id), ShapeData::filled(rect.to_path(1e-9), color))
}

fn place(scene: &mut Scene, layer: LayerId, symbol: SymbolId, transform: Affine) {
    scene.add_instance_at(layer, 0, symbol, transform);
}

// --- the cases --------------------------------------------------------------

#[test]
fn a_grid_of_translated_instances() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let dot = symbol_of(
            &mut scene,
            "dot",
            vec![filled(1, Rect::new(0.0, 0.0, 24.0, 24.0), BLUE)],
        );
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for row in 0..5 {
            for col in 0..5 {
                let x = 40.0 + col as f64 * 90.0;
                let y = 40.0 + row as f64 * 70.0;
                place(&mut scene, cast, dot, Affine::translate((x, y)));
            }
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn rotated_instances() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let arrow = symbol_of(
            &mut scene,
            "arrow",
            vec![filled(1, Rect::new(-30.0, -6.0, 30.0, 6.0), RED)],
        );
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for (i, turns) in [0.0, 0.25, 0.5, 0.75].iter().enumerate() {
            let x = 90.0 + i as f64 * 120.0;
            place(
                &mut scene,
                cast,
                arrow,
                Affine::translate((x, 200.0)) * Affine::rotate(turns * std::f64::consts::TAU),
            );
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn a_reflected_instance() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        // An L-shape, so a reflection is visible.
        let ell = symbol_of(
            &mut scene,
            "ell",
            vec![
                filled(1, Rect::new(0.0, 0.0, 20.0, 80.0), GREEN),
                filled(2, Rect::new(0.0, 60.0, 80.0, 80.0), GREEN),
            ],
        );
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        place(&mut scene, cast, ell, Affine::translate((120.0, 150.0)));
        place(
            &mut scene,
            cast,
            ell,
            Affine::translate((360.0, 150.0)) * Affine::FLIP_Y,
        );
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn nested_two_deep() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let part = symbol_of(
            &mut scene,
            "part",
            vec![filled(1, Rect::new(0.0, 0.0, 20.0, 20.0), BLUE)],
        );
        // A character places the part three times, rotated.
        let character = scene.add_symbol("character", SymbolKind::Graphic, None);
        let cl = first_layer(&scene, character);
        scene.library_mut().update(character, |s| {
            s.layers.update(cl, |l| {
                let objs: Vec<Arc<Object>> = (0..3)
                    .map(|i| {
                        Arc::new(
                            Object::instance_of(ObjectId(100 + i), part)
                                .with_transform(Affine::translate((i as f64 * 26.0, 0.0))),
                        )
                    })
                    .collect();
                l.frames.set_objects(0, objs);
            });
        });
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..4 {
            place(
                &mut scene,
                cast,
                character,
                Affine::translate((60.0 + i as f64 * 110.0, 180.0)),
            );
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn a_mask_inside_a_symbol() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let symbol = scene.add_symbol("Porthole", SymbolKind::Graphic, None);
        let art = first_layer(&scene, symbol);
        scene.library_mut().update(symbol, |s| {
            s.layers.update(art, |l| {
                l.kind = LayerKind::Masked;
                l.frames.set_objects(
                    0,
                    vec![Arc::new(filled(9001, Rect::new(0.0, 0.0, 120.0, 120.0), BLUE))],
                );
            });
            let mask = Layer {
                kind: LayerKind::Mask,
                ..Layer::normal(LayerId(9002), "Mask")
            };
            s.layers.push_front(mask);
            s.layers.update(LayerId(9002), |l| {
                l.frames.set_objects(
                    0,
                    vec![Arc::new(filled(9003, Rect::new(0.0, 0.0, 60.0, 120.0), RED))],
                );
            });
        });
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..3 {
            place(
                &mut scene,
                cast,
                symbol,
                Affine::translate((60.0 + i as f64 * 150.0, 140.0)),
            );
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn a_gradient_fill_inside_a_symbol() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mut shape = ShapeData::filled(rect.to_path(1e-9), BLUE);
        shape.fill = Some(buzz_scene::FillSpec::gradient(Gradient::linear(
            RED, BLUE, rect,
        )));
        let blob = symbol_of(
            &mut scene,
            "blob",
            vec![Object::shape(ObjectId(1), shape)],
        );
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..3 {
            place(
                &mut scene,
                cast,
                blob,
                Affine::translate((50.0 + i as f64 * 160.0, 150.0)),
            );
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn strokes_and_seam_sealed_fills_inside_a_symbol() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let stroked = Object::shape(
            ObjectId(1),
            ShapeData::stroked(Rect::new(0.0, 0.0, 80.0, 80.0).to_path(1e-9), RED, 6.0),
        );
        // Two abutting fills exercise the seam seal.
        let left = filled(2, Rect::new(0.0, 0.0, 40.0, 80.0), GREEN);
        let right = filled(3, Rect::new(40.0, 0.0, 80.0, 80.0), GREEN);
        let sym = symbol_of(&mut scene, "stroked", vec![left, right, stroked]);
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..3 {
            place(
                &mut scene,
                cast,
                sym,
                Affine::translate((60.0 + i as f64 * 150.0, 160.0)),
            );
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn additive_paint_inside_a_symbol() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let glow = symbol_of(
            &mut scene,
            "glow",
            vec![
                Object::shape(
                    ObjectId(1),
                    ShapeData::filled(Rect::new(0.0, 0.0, 60.0, 60.0).to_path(1e-9), RED)
                        .with_blend(PaintBlend::Additive),
                ),
                Object::shape(
                    ObjectId(2),
                    ShapeData::filled(Rect::new(30.0, 30.0, 90.0, 90.0).to_path(1e-9), GREEN)
                        .with_blend(PaintBlend::Additive),
                ),
            ],
        );
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..3 {
            place(
                &mut scene,
                cast,
                glow,
                Affine::translate((60.0 + i as f64 * 150.0, 150.0)),
            );
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn guide_and_outline_layers_inside_a_symbol() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        let symbol = scene.add_symbol("rigged", SymbolKind::Graphic, None);
        let base = first_layer(&scene, symbol);
        scene.library_mut().update(symbol, |s| {
            // Base artwork.
            s.layers.update(base, |l| {
                l.frames.set_objects(
                    0,
                    vec![Arc::new(filled(1, Rect::new(0.0, 0.0, 80.0, 80.0), BLUE))],
                );
            });
            // A guide layer, which draws faded.
            let mut guide = Layer::normal(LayerId(9101), "Guide");
            guide.kind = LayerKind::Guide;
            s.layers.push_front(guide);
            s.layers.update(LayerId(9101), |l| {
                l.frames.set_objects(
                    0,
                    vec![Arc::new(filled(2, Rect::new(40.0, 40.0, 120.0, 120.0), GREEN))],
                );
            });
            // An outline layer, which draws as silhouettes in the layer colour.
            let mut outline = Layer::normal(LayerId(9102), "Ink");
            outline.outline = true;
            s.layers.push_front(outline);
            s.layers.update(LayerId(9102), |l| {
                l.frames.set_objects(
                    0,
                    vec![Arc::new(filled(3, Rect::new(20.0, 20.0, 60.0, 60.0), RED))],
                );
            });
        });
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..3 {
            place(
                &mut scene,
                cast,
                symbol,
                Affine::translate((50.0 + i as f64 * 150.0, 120.0)),
            );
        }
        assert_parity(exporter, &scene, true);
    });
}

#[test]
fn a_looping_graphic_at_several_phases() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        // A two-frame graphic: red, then green.
        let flip = scene.add_symbol("flip", SymbolKind::Graphic, None);
        let layer = first_layer(&scene, flip);
        scene.library_mut().update(flip, |s| {
            s.layers.update(layer, |l| {
                l.frames.set_objects(
                    0,
                    vec![Arc::new(filled(1, Rect::new(0.0, 0.0, 80.0, 80.0), RED))],
                );
                l.frames.insert_frame(1);
                l.frames.insert_blank_keyframe(1);
                l.frames.set_objects(
                    1,
                    vec![Arc::new(filled(2, Rect::new(0.0, 0.0, 80.0, 80.0), GREEN))],
                );
            });
        });
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        // Two instances, held on different first frames — different inner frames,
        // so the cache must key them apart.
        for (i, phase) in [0u32, 1u32].iter().enumerate() {
            let mut obj = Object::instance_of(ObjectId(200 + i as u64), flip)
                .with_transform(Affine::translate((100.0 + i as f64 * 200.0, 150.0)));
            if let ObjectKind::Instance(ref mut inst) = obj.kind {
                inst.first_frame = *phase;
                inst.loop_mode = buzz_scene::LoopMode::SingleFrame;
            }
            scene.update_layer(cast, |l| {
                l.frames.push_object(0, Arc::new(obj.clone()));
            });
        }
        assert_parity(exporter, &scene, true);
    });
}
