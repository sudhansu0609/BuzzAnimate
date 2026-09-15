//! The symbol memo must be **linear** in the document, not exponential in its
//! nesting. A rig is a symbol of symbols of symbols; measuring each symbol's
//! extent by re-walking the library from scratch (as an earlier version did)
//! is exponential in the nesting depth and froze the first frame after
//! importing a rig-heavy file. This gates that `SymbolTable::refresh` — which
//! runs on that first frame — stays fast however deep the nesting.

use std::sync::Arc;
use std::time::Instant;

use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::document::DrawCache;
use buzz_scene::{Object, ObjectId, Scene, ShapeData, SymbolId, SymbolKind};
use peniko::Color;

/// A rig-like library: `levels` deep, each non-leaf symbol placing `fanout`
/// instances of the level below it — the shape an Animate character has.
fn nested_library(levels: usize, fanout: usize) -> Scene {
    let mut scene = Scene::default();

    let leaf = scene.add_symbol("leaf", SymbolKind::Graphic, None);
    let ll = scene
        .library()
        .get(leaf)
        .unwrap()
        .layers
        .iter()
        .next()
        .unwrap()
        .id;
    scene.library_mut().update(leaf, |s| {
        s.layers.update(ll, |l| {
            l.frames.push_object(
                0,
                Arc::new(Object::shape(
                    ObjectId(1),
                    ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
                )),
            );
        });
    });

    let mut prev: Vec<SymbolId> = vec![leaf];
    let mut oid = 100u64;
    for lvl in 1..levels {
        let mut here = Vec::new();
        for i in 0..fanout {
            let sym = scene.add_symbol(format!("L{lvl}_{i}"), SymbolKind::Graphic, None);
            let sl = scene
                .library()
                .get(sym)
                .unwrap()
                .layers
                .iter()
                .next()
                .unwrap()
                .id;
            scene.library_mut().update(sym, |s| {
                for j in 0..fanout {
                    let child = prev[j % prev.len()];
                    s.layers.update(sl, |l| {
                        oid += 1;
                        l.frames.push_object(
                            0,
                            Arc::new(
                                Object::instance_of(ObjectId(oid), child).with_transform(
                                    Affine::translate((j as f64 * 12.0, lvl as f64 * 12.0)),
                                ),
                            ),
                        );
                    });
                }
            });
            here.push(sym);
        }
        prev = here;
    }
    scene
}

#[test]
fn refresh_is_linear_not_exponential_in_nesting() {
    // Deep (up to the symbol-nesting limit) and branching — the case that blew
    // up. Naively this is `fanout^levels` measurements; memoised it is one pass.
    let scene = nested_library(12, 6);
    let n = scene.library().len();

    let mut cache = DrawCache::new();
    let start = Instant::now();
    cache.symbols.refresh(&scene);
    let elapsed = start.elapsed();
    eprintln!("refresh over {n} nested symbols took {elapsed:?}");

    assert!(
        elapsed.as_millis() < 100,
        "refresh over {n} nested symbols took {elapsed:?} — the memo is re-walking subtrees again"
    );
}
