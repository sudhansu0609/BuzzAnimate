//! **A band's cull actually drops the artwork outside its own strip**
//! (Phase 4d).
//!
//! `stage::build_band` is what the window falls back to, one horizontal strip
//! at a time, when a frame is still over budget at `LightDetail::Unsealed`.
//! For that to cost less than drawing the whole area, culling to a band's own
//! rows has to genuinely drop the objects outside them — this renders nothing
//! and reads no pixels back, so it needs no GPU and runs everywhere.

use buzz_app::editor::Editor;
use buzz_doc::Document;
use buzz_geom::{Point, Rect, Shape as _, Size};
use buzz_render::document::DrawCache;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

/// A tall stage with artwork spread down it: 20 rows, each a small filled
/// rectangle spanning the stage's width, evenly stacked from top to bottom.
fn tall_document() -> Editor {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(200.0, 800.0);
    let layer = scene.add_layer("Rows", LayerKind::Normal);

    const ROWS: usize = 20;
    for i in 0..ROWS {
        let y0 = i as f64 * 40.0;
        let y1 = y0 + 30.0;
        scene.add_shape(
            layer,
            ShapeData::filled(
                buzz_geom::Rect::new(0.0, y0, 200.0, y1).to_path(1e-9),
                Color::from_rgb8(0x20, 0x40, 0x80),
            ),
        );
    }

    let mut editor = Editor::new(Document::new(scene));
    // The viewport matches the stage exactly: one document unit to one
    // physical pixel, so `area` below can be read straight off it.
    editor.camera.viewport = Size::new(200.0, 800.0);
    editor.camera.center = Point::new(100.0, 400.0);
    editor.camera.zoom = 1.0;
    editor
}

#[test]
fn a_band_costs_less_than_the_whole_area() {
    let editor = tall_document();
    let area = Rect::new(0.0, 0.0, 200.0, 800.0);
    let mut cache = DrawCache::default();

    cache.begin();
    let mut whole = vello::Scene::new();
    let lines_whole =
        buzz_app::stage::build_band(&mut whole, &editor, area, 1.0, &mut cache, area);

    let top_half = Rect::new(0.0, 0.0, 200.0, 400.0);
    let mut top = vello::Scene::new();
    let lines_top =
        buzz_app::stage::build_band(&mut top, &editor, area, 1.0, &mut cache, top_half);

    let bottom_half = Rect::new(0.0, 400.0, 200.0, 800.0);
    let mut bottom = vello::Scene::new();
    let lines_bottom =
        buzz_app::stage::build_band(&mut bottom, &editor, area, 1.0, &mut cache, bottom_half);
    cache.end();

    assert!(
        lines_top < lines_whole,
        "top half ({lines_top}) should cost less than the whole area ({lines_whole})"
    );
    assert!(
        lines_bottom < lines_whole,
        "bottom half ({lines_bottom}) should cost less than the whole area ({lines_whole})"
    );
}
