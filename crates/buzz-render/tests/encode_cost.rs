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

    // The baseline is the encode *without* the symbol cache — every instance
    // walked and encoded live. The cache is on by default now, so turn it off
    // here to measure what it saves against.
    let mut cache = DrawCache::new();
    cache.set_symbol_reuse(false);
    let options = FrameOptions::default();

    // Warm, then measure a few frames.
    for _ in 0..2 {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        cache.begin();
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut cache);
        cache.end();
    }

    let start = Instant::now();
    let frames = 10;
    let mut n_paths = 0;
    for _ in 0..frames {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        cache.begin();
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
        cache.begin();
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &near_options, &mut cache);
        cache.end();
    }
    let start = Instant::now();
    let mut near_paths = 0;
    for _ in 0..frames {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &near);
        cache.begin();
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

    // Zoomed to fit, with the symbol cache on: every character is visible, so
    // culling cannot help — but the symbol is encoded once and stamped, so the
    // walk is over one character's worth of artwork, not three hundred.
    let mut reuse = DrawCache::new();
    reuse.set_symbol_reuse(true);

    // The first, cold frame encodes exactly two symbols — the character and its
    // part — whatever the number of instances.
    {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        reuse.begin();
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut reuse);
        reuse.end();
    }
    assert_eq!(
        reuse.symbol_scenes.builds, 2,
        "a cold frame should encode only the character and the part, not per instance"
    );

    // Warm frames re-encode nothing: they are all stamps.
    let warm_before = reuse.symbol_scenes.builds;
    let start = Instant::now();
    for _ in 0..frames {
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        reuse.begin();
        document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut reuse);
        reuse.end();
    }
    let reuse_per = start.elapsed() / frames;
    eprintln!(
        "zoomed-to-fit encode (symbol cache on): {reuse_per:?}/frame, {} builds over {frames} warm frames",
        reuse.symbol_scenes.builds - warm_before
    );

    assert_eq!(
        reuse.symbol_scenes.builds - warm_before,
        0,
        "warm frames should stamp, not re-encode"
    );
    assert!(
        reuse_per * 3 < per,
        "symbol reuse ({reuse_per:?}) is not much cheaper than encoding every instance ({per:?})"
    );
}

/// **Switching a light on must not draw the artwork again, once per pass.**
///
/// The guard that was missing. A GPU will only bind a buffer up to a limit —
/// 128 MB on the hardware this was written against — and Vello's path data is
/// one buffer. Nothing here had ever asked how large the encoding *was*, so a
/// lighting pass that drew each shape a few more times looked right in every
/// picture, stayed inside every timing budget, and then had a real document's
/// frame refused by the driver, taking the process with it.
///
/// # There is no instancing
///
/// That is the fact the arithmetic turns on. `Scene::fill` re-encodes the path
/// it is handed, every time: a shape drawn under six transforms costs six copies
/// of its outline, not one copy and six matrices. So a lighting model that lays
/// passes *over* the artwork pays for that artwork again per pass, and one that
/// steps a ramp across a band pays for it once per step. Measured on a 28-layer
/// Animate import: 615 thousand path segments unlit became **11.5 million**, and
/// 9 MB of path data became 171 MB.
///
/// The fix is that a lamp goes into the **paint**. A lamp's light is radially
/// symmetric about the point it stands over, so a solid colour under one is
/// exactly a radial gradient of that colour lit at each radius — the shape is
/// drawn once, as it always was, and a gradient costs stops, which are not
/// geometry.
///
/// # Why this counts paths rather than bytes
///
/// Bytes are the thing that actually overflows, but they are a poor guard: a
/// shading band is a boolean, and a boolean comes back flattened, so a band
/// round a circle carries far more segments than the two cubics the circle was.
/// That ratio is a property of the artwork and swamps the signal. **Path count**
/// is exactly the quantity the failure moves — it is how many times something
/// was encoded — and it is blind to how complex each one is.
#[test]
fn switching_a_light_on_does_not_draw_the_artwork_again_per_pass() {
    use buzz_scene::{Light, LightId, LightKind};

    // **Not `instance_heavy`.** Its parts are four units across, and a lamp
    // barely varies over four units — so nothing would take the ramping path
    // and this would measure the wrong thing entirely. These are shapes big
    // enough for a lamp to fall off across, which is the case that has to stay
    // affordable.
    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(1600.0, 1000.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    for i in 0..1200 {
        let x = 20.0 + ((i * 53) % 1540) as f64;
        let y = 20.0 + ((i * 37) % 940) as f64;
        scene.add_shape(
            layer,
            ShapeData::filled(
                buzz_geom::Circle::new(Point::new(x, y), 26.0).to_path(0.05),
                Color::from_rgb8(0xC0, 0xB8, 0xA8),
            ),
        );
    }
    let camera = Camera::new(Point::new(800.0, 500.0), 1.0, Size::new(1600.0, 1000.0));
    // `lit` is off by default, so a test that leaves it there measures the unlit
    // encode three times over and passes whatever happens.
    let options = FrameOptions {
        lit: true,
        ..FrameOptions::default()
    };

    let encode = |scene: &Scene| {
        let mut cache = DrawCache::new();
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        cache.begin();
        document::draw_frame_within(&mut builder, scene, 0, Affine::IDENTITY, &options, &mut cache);
        cache.end();
        let enc = vs.encoding();
        (
            enc.n_paths as usize,
            enc.path_data.len() as f64 * 4.0 / 1048576.0,
        )
    };

    assert!(!scene.lights().is_active(), "the baseline must be unlit");
    let (unlit_paths, unlit_mb) = encode(&scene);

    // A lamp, which is the expensive case: its light varies across the shapes it
    // reaches, so those take the lit path rather than one flat tint.
    scene.lights_mut().enabled = true;
    scene.lights_mut().lights.push(Light::new(
        LightId(1),
        "Lamp",
        LightKind::Lamp {
            position: Point::new(400.0, 300.0),
            height: 220.0,
            radius: 400.0,
        },
    ));
    assert!(scene.lights().is_active(), "the lamp must actually be on");
    // And it has to be *ramping* somewhere, or this measures a flat tint and
    // says nothing at all about what the ramping path costs.
    assert!(
        scene
            .lights()
            .field(
                buzz_geom::Rect::new(360.0, 260.0, 440.0, 340.0),
                0.0,
                1000.0
            )
            .disc()
            .is_some(),
        "the lamp must vary across a shape beside it, or the path this guards is never taken"
    );
    let (lamp_paths, lamp_mb) = encode(&scene);

    // A sun lights every shape in the document rather than only what is near it.
    scene.lights_mut().lights.clear();
    scene
        .lights_mut()
        .lights
        .push(Light::new(LightId(2), "Sun", LightKind::sun()));
    let (sun_paths, sun_mb) = encode(&scene);

    eprintln!(
        "encoded paths: unlit {unlit_paths} ({unlit_mb:.1} MB), lamp {lamp_paths} ({lamp_mb:.1} MB),          sun {sun_paths} ({sun_mb:.1} MB)"
    );

    // Lighting draws three things the unlit frame does not — the cast shadow and
    // the two bands — and each is about one more outline. Four times the unlit
    // count leaves room for that and nothing like enough for a pass that redraws
    // the artwork, which lands at ten times and up.
    for (what, paths) in [("a lamp", lamp_paths), ("a sun", sun_paths)] {
        assert!(
            paths <= unlit_paths * 4,
            "{what} took the encoded path count from {unlit_paths} to {paths}. Something is \
             drawing the artwork again per pass, and on a real document the GPU will refuse \
             to bind the result."
        );
    }

    // **A lamp must not cost materially more than a sun.** They light the same
    // shapes with the same bands; the only difference is that a lamp's light is
    // a ramp rather than one colour, and a ramp is stops rather than geometry.
    // If a lamp ever starts compositing where a sun does not, it shows here.
    assert!(
        lamp_paths <= sun_paths + unlit_paths / 4,
        "a lamp encoded {lamp_paths} paths against a sun's {sun_paths}; a lamp's falloff \
         belongs in the paint, not in another pass over the artwork"
    );
}

