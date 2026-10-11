//! E1 and E2 of the revised S5 in `docs/BIMARI5_LAG_PLAN.md`: the plain stage
//! encode, extracted from `stage::encode` so it can run from `StageInputs`
//! alone (no `Editor`) on another thread, must draw exactly what the
//! editor-driven path always has, and must carry its buffer scale with it.

use std::sync::Arc;

use buzz_app::editor::Editor;
use buzz_app::stage::{self, StageInputs};
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Size};
use buzz_render::document::{DrawCache, line_budget, line_budget_at};
use buzz_scene::{LayerKind, Object, ObjectId, Scene, ShapeData, SymbolKind};
use kurbo::Rect as KRect;
use peniko::Color;

fn shape(id: u64, rect: KRect, rgb: (u8, u8, u8)) -> Arc<Object> {
    Arc::new(Object::shape(
        ObjectId(id),
        ShapeData::filled(
            buzz_geom::Shape::to_path(&rect, 1e-9),
            Color::from_rgb8(rgb.0, rgb.1, rgb.2),
        ),
    ))
}

/// A stage of characters, each made of an animated `part` symbol, itself
/// animated: nested animated symbols, the shape a real film is made of.
fn nested_animated_editor() -> Editor {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(400.0, 300.0);

    let part = scene.add_symbol("part", SymbolKind::Graphic, None);
    scene.library_mut().update(part, |s| {
        let inner = s.layers.iter().next().map(|l| l.id).expect("a layer");
        s.layers.update(inner, |l| {
            l.frames.push_object(0, shape(1, KRect::new(0.0, 0.0, 12.0, 12.0), (200, 60, 60)));
            l.frames.insert_keyframe(3);
            l.frames.push_object(3, shape(2, KRect::new(14.0, 0.0, 30.0, 12.0), (60, 60, 200)));
        });
    });

    let character = scene.add_symbol("character", SymbolKind::Graphic, None);
    scene.library_mut().update(character, |s| {
        let inner = s.layers.iter().next().map(|l| l.id).expect("a layer");
        s.layers.update(inner, |l| {
            for i in 0..3u64 {
                l.frames.push_object(
                    0,
                    Arc::new(
                        Object::instance_of(ObjectId(100 + i), part)
                            .with_transform(Affine::translate((0.0, i as f64 * 16.0))),
                    ),
                );
            }
            l.frames.insert_keyframe(5);
            l.frames.push_object(5, shape(3, KRect::new(40.0, 0.0, 52.0, 48.0), (40, 160, 40)));
        });
    });

    let layer = scene.add_layer("Cast", LayerKind::Normal);
    for i in 0..6 {
        scene.add_instance_at(
            layer,
            0,
            character,
            Affine::translate((20.0 + i as f64 * 60.0, 40.0 + (i % 2) as f64 * 100.0)),
        );
    }
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(25);
    });

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(400.0, 300.0);
    editor.camera.center = Point::new(200.0, 150.0);
    editor.camera.zoom = 1.0;
    editor
}

/// Every stream the GPU renders from, as one byte string, for exact comparison.
fn encoding_bytes(vs: &vello::Scene) -> Vec<u8> {
    fn pod<T>(v: &[T]) -> &[u8] {
        // SAFETY: only called on vello_encoding's `Pod` streams, read-only.
        unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
    }
    let e = vs.encoding();
    let mut out = Vec::new();
    out.extend_from_slice(pod(e.path_tags.as_slice()));
    out.extend_from_slice(pod(e.path_data.as_slice()));
    out.extend_from_slice(pod(e.draw_tags.as_slice()));
    out.extend_from_slice(pod(e.draw_data.as_slice()));
    out.extend_from_slice(pod(e.transforms.as_slice()));
    out.extend_from_slice(pod(e.styles.as_slice()));
    out.extend_from_slice(
        format!("{} {} {:?}", e.n_clips, e.n_open_clips, e.resources.color_stops).as_bytes(),
    );
    out
}

#[test]
fn plain_equals_build_scene() {
    let mut editor = nested_animated_editor();
    assert!(stage::is_plain(&editor), "the fixture is a plain stage");
    let area = Rect::new(0.0, 0.0, 400.0, 300.0);

    // Three caches walked in the same frame order: the editor-driven ladder
    // (the reference), `build_scene` (which routes a plain stage to the
    // extracted path) and the extracted path called directly.
    let (mut reference, mut routed, mut direct) =
        (DrawCache::new(), DrawCache::new(), DrawCache::new());
    let mut distinct = std::collections::HashSet::new();
    for frame in 0..20u32 {
        editor.current_frame = frame;

        let mut a = vello::Scene::new();
        stage::build_scene_generic(&mut a, &editor, area, 1.0, &mut reference);
        let mut b = vello::Scene::new();
        stage::build_scene(&mut b, &editor, area, 1.0, &mut routed);
        let mut c = vello::Scene::new();
        stage::build_plain_scene(
            &mut c,
            &StageInputs::from_editor(&editor, area, 1.0),
            &mut direct,
        );

        let want = encoding_bytes(&a);
        assert!(encoding_bytes(&b) == want, "frame {frame}: build_scene differs");
        assert!(encoding_bytes(&c) == want, "frame {frame}: build_plain_scene differs");
        distinct.insert(want);
    }
    assert!(
        distinct.len() > 1,
        "the animation must actually change the encoding across the 20 frames"
    );
}

#[test]
fn plain_encode_gives_the_same_bytes_on_a_spawned_thread() {
    let editor = nested_animated_editor();
    let area = Rect::new(0.0, 0.0, 400.0, 300.0);
    let mut inputs = StageInputs::from_editor(&editor, area, 1.0);
    inputs.frame = 7;
    inputs.buffer_scale = 4;

    let mut here = vello::Scene::new();
    stage::build_plain_scene(&mut here, &inputs, &mut DrawCache::new());
    assert_eq!(
        buzz_render::vello_encoding::buffer_scale(),
        4,
        "encoding sets the calling thread's buffer scale"
    );
    let want = encoding_bytes(&here);

    let (bytes, before, after) = std::thread::spawn(move || {
        // A fresh thread starts at scale 1 until the encode sets it.
        let before = line_budget();
        let mut vs = vello::Scene::new();
        stage::build_plain_scene(&mut vs, &inputs, &mut DrawCache::new());
        (encoding_bytes(&vs), before, line_budget())
    })
    .join()
    .expect("the encode thread");

    assert_eq!(before, line_budget_at(1));
    assert_eq!(after, line_budget_at(4), "the spawned thread encodes at the inputs' scale");
    assert!(bytes == want, "same bytes on a spawned thread");
}
