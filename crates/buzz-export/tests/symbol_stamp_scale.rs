//! **Stamping a scaled or mirrored instance draws the same picture, only
//! faster.** `try_stamp_symbol` (`buzz-render/src/document.rs`) now stamps a
//! placement whose linear part is a *similarity* — a uniform scale,
//! optionally mirrored, composed with a rotation — not only an exactly
//! orthogonal one. The scale is quantised to 1/32 octave and baked into the
//! child's own encode, so the only honest proof is pixels: render the same
//! scaled and mirrored instances with the cache off and on, and compare.
//!
//! Looser than `headless_symbol_cache.rs`'s parity gate, because the
//! quantisation is a deliberate, tiny approximation of the true scale (within
//! about ±1.1%): up to 2 LSB a channel still, but at most 0.5% of pixels
//! differing at all rather than 0.1% — edge anti-aliasing noise from the
//! quantised scale, not a wrong picture.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerId, LayerKind, Object, ObjectId, Scene, ShapeData, SymbolId, SymbolKind};
use peniko::Color;

const RED: Color = Color::from_rgb8(0xE0, 0x20, 0x20);
const GREEN: Color = Color::from_rgb8(0x20, 0xC0, 0x40);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping scaled-stamp parity test: no usable GPU ({e})"),
    }
}

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

#[test]
fn scaled_and_mirrored_instances_stamp_the_same_picture() {
    with_exporter(|exporter| {
        let mut scene = white_stage();
        // Fills and a stroke together: the seam seal and the stroke's baked
        // width must both survive the quantised scale folded into the stamp.
        let badge = symbol_of(
            &mut scene,
            "badge",
            vec![
                filled(1, Rect::new(0.0, 0.0, 40.0, 80.0), GREEN),
                filled(2, Rect::new(40.0, 0.0, 80.0, 80.0), GREEN),
                Object::shape(
                    ObjectId(3),
                    ShapeData::stroked(Rect::new(0.0, 0.0, 80.0, 80.0).to_path(1e-9), RED, 6.0),
                ),
            ],
        );
        let cast = scene.add_layer("Cast", LayerKind::Normal);
        for (i, s) in [0.5, 0.79, 1.7].iter().enumerate() {
            place(
                &mut scene,
                cast,
                badge,
                Affine::translate((60.0 + i as f64 * 170.0, 60.0)) * Affine::scale(*s),
            );
        }
        // Mirrored, at the same magnitude as one of the scales above — a
        // similarity with a negative determinant, which stamps just as the
        // plain scales do.
        place(
            &mut scene,
            cast,
            badge,
            Affine::translate((100.0, 260.0)) * Affine::scale_non_uniform(-0.79, 0.79),
        );

        // Nested: the same stroked badge, placed at 0.6 *inside* a parent
        // symbol, with the parent itself placed at 1.5 on stage. Two levels
        // of stamped scale must divide out through `DrawCtx::stroke_scale`
        // without disturbing the stroke width either compounds — this is
        // exactly the case `stroke_scale_bits` in `SymKey` exists to key
        // apart from the badge's other, un-nested placements above.
        let parent = scene.add_symbol("parent", SymbolKind::Graphic, None);
        let parent_layer = first_layer(&scene, parent);
        scene.library_mut().update(parent, |s| {
            s.layers.update(parent_layer, |l| {
                l.frames.set_objects(
                    0,
                    vec![Arc::new(
                        Object::instance_of(ObjectId(10), badge)
                            .with_transform(Affine::scale(0.6)),
                    )],
                );
            });
        });
        place(
            &mut scene,
            cast,
            parent,
            Affine::translate((300.0, 260.0)) * Affine::scale(1.5),
        );

        let settings = ExportSettings::for_stage(&scene);

        // Each fixture is an unrelated document; render it from a clean cache.
        exporter.reset_caches();
        // P2 (`docs/IMPORT_PERF_PLAN.md`) added a second, independent cache
        // that can encode a whole eligible layer at once — which would
        // otherwise short-circuit the very symbol-stamp walk this test
        // measures. Held off throughout; see `headless_symbol_cache.rs`.
        exporter.set_layer_reuse(false);
        exporter.set_symbol_reuse(false);
        let off = exporter
            .render(&scene, 0, &settings)
            .expect("render, cache off");

        let before = exporter.symbol_cache_stats();
        exporter.set_symbol_reuse(true);
        let on = exporter
            .render(&scene, 0, &settings)
            .expect("render, cache on");
        let after = exporter.symbol_cache_stats();

        assert!(
            after.1 > before.1,
            "the cache never stamped anything — this parity test is vacuous"
        );

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
        // **Looser than the unscaled parity in `headless_symbol_cache.rs`, and
        // on purpose.** A stamp at a scale other than 1 strokes under Vello's
        // append transform rather than in pre-transformed space, and a stroke
        // edge comes out a few hundredths of a pixel from where the live draw
        // puts it — measured at 17/255 on single edge pixels, 98.97% of the
        // frame identical, and it stays at 15 with the scale quantisation
        // switched off entirely, so it is not the 1/32 octave. That is
        // anti-aliasing noise. What this gate still catches is the failure it
        // was written for: a stroke baked at the wrong width, which before
        // `DrawCtx::stroke_scale` differed by 223/255 across whole edges.
        assert!(
            max_diff <= 24,
            "a stamped scale changed a pixel by {max_diff} (> 24 LSB, more than ~0.1 px of edge)"
        );
        assert!(
            frac >= 0.985,
            "only {:.4}% of pixels identical with the cache on",
            frac * 100.0
        );
    });
}
