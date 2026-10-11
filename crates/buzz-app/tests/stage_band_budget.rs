//! H0 items 2 and 5 in `docs/IMPORT_PERF_PLAN.md`: `render_stage_bands`'
//! planning — pulled out into the headless `buzz_app::app::plan_stage_bands`
//! so it can be simulated without a window or a GPU — must (1) never hand
//! back a band whose own estimate is still over either budget, capping the
//! leaf count at 16 rather than doubling blindly, and (2) judge a stamped
//! symbol's "mostly off-screen" rule against the band it is actually being
//! drawn into, not the whole frame, so a character spanning several bands is
//! drawn live in each rather than appended whole into every one of them (H0
//! item 1).

use buzz_app::editor::Editor;
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Size};
use buzz_render::document::{DrawCache, line_budget, segment_ceiling};
use buzz_scene::{LayerKind, Scene, ShapeData, SymbolKind};
use peniko::Color;

// ---------------------------------------------------------------------------
// H0 item 1: a symbol spanning several bands is drawn live in each.
// ---------------------------------------------------------------------------

/// A tall stage with one symbol instance whose four separate shapes are
/// spread far enough apart that each lands inside a different 200-tall band,
/// while the instance's own bounds — the four shapes together — are less
/// than half inside any single one of them.
fn straddler_scene() -> Editor {
    use buzz_geom::{BezPath, PathEl};

    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(200.0, 800.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);

    let straddler = scene.add_symbol("Straddler", SymbolKind::Graphic, None);
    scene.library_mut().update(straddler, |symbol| {
        let inner = symbol.layers.iter().next().map(|l| l.id);
        let Some(inner) = inner else {
            return;
        };
        // Four shapes, one per band once the instance below is centred on
        // the stage: local y -300/-100/100/300 land at world y
        // 100/300/500/700 — the middle of bands [0,200), [200,400),
        // [400,600) and [600,800).
        for (i, local_y) in [-300.0, -100.0, 100.0, 300.0].into_iter().enumerate() {
            let r = 25.0;
            let segments = 80;
            let mut path = BezPath::new();
            for s in 0..segments {
                let t = s as f64 / f64::from(segments) * std::f64::consts::TAU;
                let p = Point::new(r * t.cos(), local_y + r * t.sin());
                if s == 0 {
                    path.push(PathEl::MoveTo(p));
                } else {
                    path.push(PathEl::LineTo(p));
                }
            }
            path.push(PathEl::ClosePath);
            symbol.layers.update(inner, |l| {
                l.frames.push_object(
                    0,
                    std::sync::Arc::new(buzz_scene::Object::shape(
                        buzz_scene::ObjectId(9001 + i as u64),
                        ShapeData::filled(path, Color::from_rgb8(0x20, 0x40, 0x80)),
                    )),
                );
            });
        }
    });

    scene.add_instance_at(layer, 0, straddler, Affine::translate((100.0, 400.0)));

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(200.0, 800.0);
    editor.camera.center = Point::new(100.0, 400.0);
    editor.camera.zoom = 1.0;
    editor
}

/// **The report, reproduced in miniature.** Before H0 item 1, a band's stamp
/// rule was judged against the whole frame, so a symbol on screen anywhere
/// was appended whole — every one of its shapes, unculled — into every band
/// its instance touched: four bands each paying for all four shapes instead
/// of one apiece. That is why more bands never shrank the largest band's own
/// lines, and why a band still over the line budget rasterised as black.
#[test]
fn a_symbol_spanning_several_bands_is_drawn_live_in_each_not_stamped_whole() {
    let editor = straddler_scene();
    let area = Rect::new(0.0, 0.0, 200.0, 800.0);
    let ceiling = segment_ceiling(200.0 * 800.0);

    // The instance drawn once, unbanded: the genuine cost of all four shapes
    // together, to compare each band's own cost against.
    let mut whole_cache = DrawCache::default();
    whole_cache.begin();
    let mut whole = vello::Scene::new();
    let lines_whole =
        buzz_app::stage::build_band(&mut whole, &editor, area, 1.0, &mut whole_cache, area);
    whole_cache.end();
    assert!(lines_whole > 0, "the fixture drew nothing at all");

    let mut cache = DrawCache::default();
    let planned =
        buzz_app::app::plan_stage_bands(&editor, &mut cache, area, 1.0, ceiling, 200, 800, 4);

    assert_eq!(
        planned.len(),
        4,
        "nothing here is over budget, so none of the four bands should split"
    );
    for band in &planned {
        assert!(
            band.lines < lines_whole,
            "band [{}, {}) cost {} lines, not less than the whole instance's {lines_whole} — \
             it looks stamped whole into this band rather than drawn live and culled",
            band.y0,
            band.y1,
            band.lines,
        );
    }
    let total: u64 = planned.iter().map(|b| b.lines).sum();
    assert!(
        total < lines_whole * 2,
        "the four bands cost {total} lines between them against one whole instance's \
         {lines_whole} — its shapes are being duplicated across bands rather than split \
         between them",
    );
}

// ---------------------------------------------------------------------------
// H0 items 2 and 3: an over-budget band is split, not doubled, until every
// leaf fits.
// ---------------------------------------------------------------------------

/// A square stage tiled with many instances of one heavy symbol, spread
/// across its whole area — heavy enough, in enough of them, that encoding it
/// as a single band overruns `LINE_BUDGET`.
fn dense_symbol_scene(instances: usize) -> Editor {
    use buzz_geom::{BezPath, PathEl};

    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(1200.0, 1200.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);

    let filler = scene.add_symbol("Filler", SymbolKind::Graphic, None);
    scene.library_mut().update(filler, |symbol| {
        let inner = symbol.layers.iter().next().map(|l| l.id);
        let Some(inner) = inner else {
            return;
        };
        // A single heavy shape, centred on the symbol's own local origin so
        // each instance's transform below is a pure translate to its place
        // on the stage. Calibrated against `dense_document` in
        // `stage_lighting.rs` (500 shapes of 600 segments each measure
        // 1,500,004 lines there — about 3,000 lines per 600-segment shape):
        // 3,000 segments here costs roughly 15,000 lines per instance.
        let r = 25.0;
        let segments = 3000;
        let mut path = BezPath::new();
        for s in 0..segments {
            let t = s as f64 / f64::from(segments) * std::f64::consts::TAU;
            let p = Point::new(r * t.cos(), r * t.sin());
            if s == 0 {
                path.push(PathEl::MoveTo(p));
            } else {
                path.push(PathEl::LineTo(p));
            }
        }
        path.push(PathEl::ClosePath);
        symbol.layers.update(inner, |l| {
            l.frames.push_object(
                0,
                std::sync::Arc::new(buzz_scene::Object::shape(
                    buzz_scene::ObjectId(9001),
                    ShapeData::filled(path, Color::from_rgb8(0x20, 0x40, 0x80)),
                )),
            );
        });
    });

    let across = (instances as f64).sqrt().ceil() as usize;
    let step = 1200.0 / across as f64;
    for i in 0..instances {
        let cx = (i % across) as f64 * step + step * 0.5;
        let cy = (i / across) as f64 * step + step * 0.5;
        scene.add_instance_at(layer, 0, filler, Affine::translate((cx, cy)));
    }

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(1200.0, 1200.0);
    editor.camera.center = Point::new(600.0, 600.0);
    editor.camera.zoom = 1.0;
    editor
}

/// **The planning`render_stage_bands` used to get wrong.** A band still over
/// budget after being encoded used to be handed straight to the GPU anyway —
/// rasterising nothing, so its rows came out black — while the *frame*
/// doubled its band count regardless of which band had actually overflowed.
/// `plan_stage_bands` instead splits just the band that is over, recursively,
/// until every leaf it hands back fits both budgets.
#[test]
fn an_over_budget_band_is_split_until_every_leaf_fits() {
    let editor = dense_symbol_scene(170);
    let area = Rect::new(0.0, 0.0, 1200.0, 1200.0);
    let ceiling = segment_ceiling(1200.0 * 1200.0);
    let mut cache = DrawCache::default();

    // `n: 1` — a single band covering the whole stage — so every leaf in
    // the answer came from splitting, not from the starting count.
    let planned =
        buzz_app::app::plan_stage_bands(&editor, &mut cache, area, 1.0, ceiling, 1200, 1200, 1);

    assert!(
        planned.len() > 1,
        "the fixture must actually be over budget as a single band, or splitting is never \
         exercised here"
    );
    assert!(
        planned.len() <= 16,
        "H0 item 3: the leaf count is what the next frame's band count is drawn from, capped \
         at 16 — got {}",
        planned.len()
    );
    for band in &planned {
        assert!(
            band.lines <= line_budget(),
            "band [{}, {}) is still over line_budget(): {} lines",
            band.y0,
            band.y1,
            band.lines,
        );
        assert!(
            band.segments <= ceiling,
            "band [{}, {}) is still over the segment ceiling: {} of {ceiling}",
            band.y0,
            band.y1,
            band.segments,
        );
    }
}
