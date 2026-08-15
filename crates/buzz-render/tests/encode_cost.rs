//! The stage encode on an instance-heavy document — what an imported Animate
//! file is. Zoomed to fit, every symbol is visible and the encode is inherently
//! heavy; but **zoomed in, the normal working state, culling must skip the
//! off-screen symbols' whole subtrees** so the document is workable. This gates
//! that: the zoomed-in encode must be a fraction of the zoomed-to-fit one.

use std::time::Instant;

use buzz_geom::{Affine, Camera, Point, Rect, Shape as _, Size};
use buzz_render::document::{self, DrawCache, FrameOptions};
use buzz_render::{SceneBuilder, vello};
use buzz_scene::{LayerKind, Scene, ShapeData, SymbolKind};
use peniko::Color;

fn instance_heavy() -> Scene {
    let mut scene = Scene::default();

    // A "part": a symbol of 20 shapes.
    let part = scene.add_symbol("part", SymbolKind::Graphic, None);
    let part_layer = scene
        .library()
        .get(part)
        .unwrap()
        .layers
        .iter()
        .next()
        .unwrap()
        .id;
    scene.library_mut().update(part, |s| {
        for i in 0..20 {
            let x = i as f64 * 5.0;
            s.layers.update(part_layer, |l| {
                l.frames.push_object(
                    0,
                    std::sync::Arc::new(buzz_scene::Object::shape(
                        buzz_scene::ObjectId(10_000 + i),
                        ShapeData::filled(Rect::new(x, 0.0, x + 4.0, 4.0).to_path(1e-9), Color::WHITE),
                    )),
                );
            });
        }
    });

    // A "character": 10 instances of the part.
    let character = scene.add_symbol("character", SymbolKind::Graphic, None);
    let char_layer = scene
        .library()
        .get(character)
        .unwrap()
        .layers
        .iter()
        .next()
        .unwrap()
        .id;
    scene.library_mut().update(character, |s| {
        for i in 0..10 {
            let y = i as f64 * 8.0;
            s.layers.update(char_layer, |l| {
                l.frames.push_object(
                    0,
                    std::sync::Arc::new(
                        buzz_scene::Object::instance_of(
                            buzz_scene::ObjectId(20_000 + i),
                            part,
                        )
                        .with_transform(Affine::translate((0.0, y))),
                    ),
                );
            });
        }
    });

    // The stage: 300 instances of the character, spread across a grid.
    let layer = scene.add_layer("Cast", LayerKind::Normal);
    for i in 0..300 {
        let x = (i % 20) as f64 * 120.0;
        let y = (i / 20) as f64 * 100.0;
        scene.add_instance_at(layer, 0, character, Affine::translate((x, y)));
    }
    scene
}

#[test]
fn measure_encode_zoomed_to_fit() {
    let scene = instance_heavy();

    // A camera that fits the whole document — everything on-screen, nothing
    // culled, exactly like the view right after an import.
    let camera = Camera::new(Point::new(1200.0, 750.0), 0.4, Size::new(1600.0, 1000.0));

    let mut cache = DrawCache::new();
    let options = FrameOptions::default();

    // Warm, then measure a few frames.
    for _ in 0..2 {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        cache.begin(0);
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut cache);
        cache.end();
    }

    let start = Instant::now();
    let frames = 10;
    let mut n_paths = 0;
    for _ in 0..frames {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        cache.begin(0);
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut cache);
        cache.end();
        n_paths = vs.encoding().n_paths;
    }
    let per = start.elapsed() / frames;
    eprintln!(
        "zoomed-to-fit encode: {per:?}/frame, {n_paths} paths (300 chars x 10 parts x 20 shapes = 60k shapes)"
    );

    // Zoomed in on a few characters, **with culling on** as the stage runs it:
    // off-screen characters should skip their whole subtree.
    let near = Camera::new(Point::new(120.0, 100.0), 3.0, Size::new(800.0, 600.0));
    let cull_rect = SceneBuilder::new(&mut vello::Scene::new(), &near).clip_bounds();
    let near_options = FrameOptions {
        cull: Some(cull_rect),
        ..FrameOptions::default()
    };
    for _ in 0..2 {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &near);
        cache.begin(0);
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &near_options, &mut cache);
        cache.end();
    }
    let start = Instant::now();
    let mut near_paths = 0;
    for _ in 0..frames {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &near);
        cache.begin(0);
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &near_options, &mut cache);
        cache.end();
        near_paths = vs.encoding().n_paths;
    }
    let near_per = start.elapsed() / frames;
    eprintln!(
        "zoomed-in encode (culling on): {near_per:?}/frame, {near_paths} paths (most characters culled)"
    );

    // Culling an off-screen character skips its whole subtree, so far fewer
    // paths are encoded and the frame is a fraction of the zoomed-to-fit cost.
    assert!(
        near_paths * 4 < n_paths,
        "zoomed-in culling barely helped: {near_paths} vs {n_paths} paths — instance culling is not skipping subtrees"
    );
    assert!(
        near_per * 3 < per,
        "zoomed-in encode ({near_per:?}) is not much cheaper than zoomed-to-fit ({per:?})"
    );
}
