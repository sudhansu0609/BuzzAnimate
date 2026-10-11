//! **A lit layer may stamp its symbols — exactly.** `docs/RIG_SCENE_LAG_PLAN.md`,
//! step L3''.
//!
//! `try_stamp_symbol` used to refuse every instance on a lit layer
//! (`neutral_ctx`), so a rig of symbols was walked and encoded live, shape by
//! shape, every frame. With a rig of suns and a *flat* sky the light cannot tell
//! one placement from another (`LightRig::is_position_independent`), so a stamp
//! encoded with the light applied is the live encoding for every instance. The
//! counters below run the real draw walk on the CPU; the pixel tests render on
//! the GPU and skip, as the other GPU tests do, when there is no adapter.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Camera, Point, Rect, Shape as _, Size};
use buzz_render::document::{self, DrawCache, FrameOptions};
use buzz_render::{GpuContext, GpuPreference, SceneBuilder, vello, wgpu};
use buzz_scene::{
    ArmatureData, EdgeMode, Gradient, GradientKind, GradientStop, ImageAsset, ImageFill, ImageId,
    LayerKind, Light, LightId, LightKind, Object, ObjectId, ObjectKind, Paint, Scene, ShadeModel,
    ShapeData, StrokeSpec, SymbolId, SymbolKind,
};
use peniko::Color;

const W: f64 = 480.0;
const H: f64 = 320.0;
const RIG: ObjectId = ObjectId(1);

fn rect(x: f64, y: f64, w: f64, h: f64) -> BezPath {
    Rect::new(x, y, x + w, y + h).to_path(1e-9)
}

/// A one-frame graphic symbol of five filled shapes (one stroked), plus, when
/// `nested` is given, an instance of another symbol placed with a rotation and
/// a scale — so building it builds a stamp inside a stamp.
fn art_symbol(scene: &mut Scene, name: &str, seed: f64, nested: Option<SymbolId>) -> SymbolId {
    let id = scene.add_symbol(name, SymbolKind::Graphic, None);
    let layer = scene
        .library()
        .get(id)
        .expect("just added")
        .layers
        .iter()
        .next()
        .expect("a symbol always has a layer")
        .id;
    let base = 10_000 + id.0 * 100;
    scene.library_mut().update(id, |symbol| {
        symbol.layers.update(layer, |l| {
            for k in 0..5u64 {
                let mut data = ShapeData::filled(
                    rect(
                        seed + k as f64 * 13.0,
                        seed + (k % 3) as f64 * 9.0,
                        24.0,
                        28.0 + k as f64 * 3.0,
                    ),
                    Color::from_rgb8(190 - (k * 20) as u8, 90 + (k * 30) as u8, 80 + (k * 25) as u8),
                );
                if k == 2 {
                    data.stroke = Some(StrokeSpec::new(Color::from_rgb8(30, 30, 40), 2.5));
                }
                l.frames
                    .push_object(0, Arc::new(Object::shape(ObjectId(base + k), data)));
            }
            if let Some(inner) = nested {
                let child = Object::instance_of(ObjectId(base + 50), inner).with_transform(
                    Affine::translate((26.0, 8.0)) * Affine::rotate(0.45) * Affine::scale(0.8),
                );
                l.frames.push_object(0, Arc::new(child));
            }
        });
    });
    id
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sky {
    Flat,
    Mixed,
}

/// A lit stage: a sun and a sky (flat or mixed) and, optionally, a lamp;
/// shadows on (the sun default); one layer holding a rig of three symbol
/// instances (the last symbol nests another) and two plain instances, one
/// scaled and rotated, one mirrored.
fn stamp_scene(edges: EdgeMode, sky: Sky, lamp: bool) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(W, H);
    let layer = scene.add_layer("Cast", LayerKind::Normal);

    let rig = scene.lights_mut();
    rig.enabled = true;
    rig.model = ShadeModel::Form;
    rig.modelling = 1.0;
    rig.edges = edges;
    let mut sun = Light::new(
        LightId(1),
        "Key",
        LightKind::Sun {
            azimuth: std::f64::consts::PI,
            elevation: 0.6,
        },
    );
    sun.rim = 0.4;
    rig.lights.push(sun);
    let blue = Color::from_rgb8(0x50, 0x70, 0xE0);
    let mut sky_light = Light::new(LightId(2), "Fill", LightKind::sky());
    sky_light.color = blue;
    sky_light.intensity = 0.5;
    sky_light.kind = LightKind::Sky {
        horizon: match sky {
            Sky::Flat => blue,
            Sky::Mixed => Color::from_rgb8(0xE0, 0x90, 0x50),
        },
    };
    rig.lights.push(sky_light);
    if lamp {
        rig.lights.push(Light::new(
            LightId(3),
            "Lamp",
            LightKind::lamp(Point::new(240.0, 90.0)),
        ));
    }

    let leaf = art_symbol(&mut scene, "leaf", 6.0, None);
    let arm = art_symbol(&mut scene, "arm", 14.0, None);
    let torso = art_symbol(&mut scene, "torso", 22.0, None);
    let head = art_symbol(&mut scene, "head", 10.0, Some(leaf));

    let mut data = ArmatureData::chain(&[60.0, 60.0, 60.0]);
    for (i, symbol) in [arm, torso, head].iter().enumerate() {
        let part = Object::instance_of(ObjectId(200 + i as u64), *symbol)
            .with_transform(Affine::translate((90.0 + i as f64 * 70.0, 70.0)));
        data.bind_rigid(Arc::new(part), i);
    }
    let mut rig_object = Object::group(RIG, Vec::new());
    rig_object.kind = ObjectKind::Armature(data);
    scene.add_object(layer, rig_object);

    scene.add_object(
        layer,
        Object::instance_of(ObjectId(2), torso).with_transform(
            Affine::translate((330.0, 40.0)) * Affine::rotate(0.3) * Affine::scale(1.5),
        ),
    );
    scene.add_object(
        layer,
        Object::instance_of(ObjectId(3), head)
            .with_transform(Affine::new([-1.0, 0.0, 0.0, 1.0, 440.0, 190.0])),
    );
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(10);
    });
    scene
}

fn set_pose(scene: &mut Scene, pose: &[f64]) {
    let found = scene.update_object(RIG, |object| {
        if let ObjectKind::Armature(data) = &mut object.kind {
            data.armature.set_pose(pose);
        }
    });
    assert!(found, "the rig is on the stage");
}

const FIRST: &[f64] = &[0.3, -0.2, 0.4];
const SECOND: &[f64] = &[0.55, 0.15, -0.35];

/// Encode `frame` through `cache` the way the window does (CPU only).
fn encode(scene: &Scene, frame: u32, lit: bool, cache: &mut DrawCache) -> vello::Scene {
    let camera = Camera::new(Point::new(W / 2.0, H / 2.0), 1.0, Size::new(W, H));
    let mut encoded = vello::Scene::new();
    let mut builder = SceneBuilder::new(&mut encoded, &camera);
    let options = FrameOptions {
        lit,
        ..FrameOptions::default()
    };
    cache.begin();
    document::draw_frame_within(&mut builder, scene, frame, Affine::IDENTITY, &options, cache);
    cache.end();
    encoded
}

/// Are two encodings the same bytes — every stream, transforms and styles
/// included? Stronger than comparing pixels, and free of the GPU's own run-to-run
/// flicker (the same encoding rendered twice differs by one level at one pixel in
/// about 1 render in 20 on this scene).
fn same_encoding(a: &vello::Scene, b: &vello::Scene) -> bool {
    let (a, b) = (a.encoding(), b.encoding());
    a.path_tags == b.path_tags
        && a.path_data == b.path_data
        && a.draw_tags == b.draw_tags
        && a.draw_data == b.draw_data
        && format!("{:?}", a.transforms) == format!("{:?}", b.transforms)
        && format!("{:?}", a.styles) == format!("{:?}", b.styles)
        && (a.n_paths, a.n_path_segments, a.n_clips, a.n_open_clips)
            == (b.n_paths, b.n_path_segments, b.n_clips, b.n_open_clips)
}

fn refused(cache: &DrawCache, name: &str) -> u64 {
    cache
        .symbol_scenes
        .refusal_counts()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
        .expect("a named refusal counter")
}

// ---- counters (CPU) --------------------------------------------------------

/// Sun + flat sky + `Figure` edges: every instance stamps, nothing is refused
/// for its light, and a held frame is all hits.
#[test]
fn a_sun_and_a_flat_sky_stamp_lit() {
    let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
    set_pose(&mut scene, FIRST);
    let mut cache = DrawCache::new();
    for frame in [0, 1, 0] {
        encode(&scene, frame, true, &mut cache);
    }
    assert_eq!(refused(&cache, "neutral_ctx"), 0, "no instance is refused for its light");
    assert!(cache.symbol_scenes.builds > 0, "stamps were built");
    assert!(cache.symbol_scenes.stamps_lit > 0, "and they are lit stamps");
    assert_eq!(cache.symbol_scenes.stamps, cache.symbol_scenes.stamps_lit);

    // Held: the next frames build nothing new and every instance is a hit.
    let (builds, stamps) = (cache.symbol_scenes.builds, cache.symbol_scenes.stamps);
    encode(&scene, 1, true, &mut cache);
    assert_eq!(cache.symbol_scenes.builds, builds, "a held lit frame builds no stamp");
    assert!(cache.symbol_scenes.stamps > stamps, "it stamps");

    // The rig moved: the placements rotate, the scales do not, so the same
    // stamps are hit again.
    set_pose(&mut scene, SECOND);
    let builds = cache.symbol_scenes.builds;
    encode(&scene, 0, true, &mut cache);
    assert_eq!(cache.symbol_scenes.builds, builds, "a moved rig stamps from the same entries");
    assert_eq!(refused(&cache, "neutral_ctx"), 0);
}

/// A sky that mixes by height, a lamp, and per-shape edges all draw live: the
/// refusal is counted and nothing lit is stamped.
#[test]
fn a_mixed_sky_a_lamp_and_shape_edges_are_refused() {
    for (what, scene) in [
        ("mixed sky", stamp_scene(EdgeMode::Figure, Sky::Mixed, false)),
        ("lamp", stamp_scene(EdgeMode::Figure, Sky::Flat, true)),
        ("shape edges", stamp_scene(EdgeMode::Shapes, Sky::Flat, false)),
    ] {
        let mut cache = DrawCache::new();
        let mut live = DrawCache::new();
        live.set_symbol_reuse(false);
        for frame in [0, 1, 0] {
            let encoded = encode(&scene, frame, true, &mut cache);
            let reference = encode(&scene, frame, true, &mut live);
            assert!(
                same_encoding(&encoded, &reference),
                "{what}: a refused light encodes exactly what the live walk does"
            );
        }
        assert!(refused(&cache, "neutral_ctx") > 0, "{what}: refused and counted");
        assert_eq!(cache.symbol_scenes.stamps_lit, 0, "{what}: nothing lit is stamped");
        assert_eq!(cache.symbol_scenes.builds, 0, "{what}: nothing built");
    }
}

/// `SymKey` carries the light: the same symbols under two rigs are two
/// entries each, and a lit stamp never serves an unlit draw (or the reverse).
#[test]
fn the_same_symbol_under_two_rigs_is_two_entries_and_lit_never_aliases_unlit() {
    let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
    set_pose(&mut scene, FIRST);
    let mut cache = DrawCache::new();
    for frame in [0, 1, 0] {
        encode(&scene, frame, true, &mut cache);
    }
    let one_rig = cache.stamp_count();
    assert!(one_rig > 0);

    // A different sun colour: another rig fingerprint, another set of stamps.
    scene.lights_mut().lights[0].color = Color::from_rgb8(0xFF, 0x90, 0x40);
    for frame in [0, 1, 0] {
        encode(&scene, frame, true, &mut cache);
    }
    assert_eq!(
        cache.stamp_count(),
        2 * one_rig,
        "the same symbols under a different rig are new entries, and the old ones are kept"
    );

    // The same scene drawn unlit (the layer's lights are off for this pass):
    // its stamps are separate again.
    for frame in [0, 1, 0] {
        encode(&scene, frame, false, &mut cache);
    }
    assert!(
        cache.stamp_count() > 2 * one_rig,
        "unlit stamps are their own entries, not the lit ones"
    );
}

// ---- pixels (GPU) ----------------------------------------------------------

const PW: u32 = 512;
const PH: u32 = 256;
const BACKDROP: Color = Color::from_rgb8(0x20, 0x24, 0x2c);

struct Gpu {
    /// Draw with the lights on (the default) or as an unlit document.
    lit: bool,
    gpu: GpuContext,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

fn gpu() -> Option<Gpu> {
    let gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping lit stamp pixel test: no usable GPU ({e})");
            return None;
        }
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lit-stamps-target"),
        size: wgpu::Extent3d {
            width: PW,
            height: PH,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: buzz_render::RENDER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lit-stamps-readback"),
        size: (PW * PH * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Some(Gpu {
        lit: true,
        gpu,
        texture,
        view,
        readback,
    })
}

/// Where the camera looks: the whole cast, or the middle of one object.
#[derive(Clone, Copy, Debug)]
enum Focus {
    Cast,
    Object(ObjectId),
}

impl Focus {
    fn point(self, scene: &Scene, frame: u32) -> Point {
        match self {
            Focus::Cast => scene
                .layers()
                .iter()
                .filter_map(|layer| layer.bounds_at(frame))
                .reduce(|a, b| a.union(b))
                .map(|bounds| bounds.center())
                .unwrap_or(Point::new(250.0, 100.0)),
            Focus::Object(id) => {
                let (_, object) = scene.find_object(id).expect("the focus is on the stage");
                scene.resolved_bounds(object).center()
            }
        }
    }
}

impl Gpu {
    /// Draw `frame` of `scene` through `cache` at `zoom` and render it. `culled`
    /// passes the visible document rectangle (a little inflated) as the cull, as
    /// the window does.
    fn render(
        &mut self,
        scene: &Scene,
        frame: u32,
        focus: Focus,
        zoom: f64,
        culled: bool,
        cache: &mut DrawCache,
    ) -> Vec<u8> {
        let centre = focus.point(scene, frame);
        let camera = Camera::new(centre, zoom, Size::new(PW as f64, PH as f64));
        let mut encoded = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut encoded, &camera);
        let (hw, hh) = (PW as f64 / 2.0 / zoom, PH as f64 / 2.0 / zoom);
        let options = FrameOptions {
            lit: self.lit,
            cull: culled.then(|| {
                Rect::new(centre.x - hw - 8.0, centre.y - hh - 8.0, centre.x + hw + 8.0, centre.y + hh + 8.0)
            }),
            ..FrameOptions::default()
        };
        cache.begin();
        document::draw_frame_within(&mut builder, scene, frame, Affine::IDENTITY, &options, cache);
        cache.end();

        self.gpu
            .render(&encoded, &self.view, PW, PH, BACKDROP)
            .expect("vello render");
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(PW * 4),
                    rows_per_image: Some(PH),
                },
            },
            wgpu::Extent3d {
                width: PW,
                height: PH,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .expect("poll device");
        let pixels = slice.get_mapped_range().to_vec();
        self.readback.unmap();
        pixels
    }
}

/// How the two renders differ, for a failure message: pixels that differ, the
/// biggest channel step, and the first one.
fn diff(a: &[u8], b: &[u8]) -> String {
    let mut count = 0usize;
    let mut worst = 0u8;
    let mut first = None;
    for (i, (pa, pb)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
        if pa != pb {
            count += 1;
            worst = worst.max(pa.iter().zip(pb).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0));
            first.get_or_insert((i as u32 % PW, i as u32 / PW, [pa[0], pa[1], pa[2]], [pb[0], pb[1], pb[2]]));
        }
    }
    format!("{count} pixels differ, worst channel step {worst}, first {first:?}")
}

/// **The stamp parity gate** the other stamp tests use (`headless_symbol_cache`):
/// a stamped path takes one more `f32` rounding than the live one, so a handful
/// of anti-aliased edge pixels may differ by a least significant bit or two.
/// The gate is max channel difference <= 2 and at least 99.9% of pixels
/// identical. (The *unlit* stamp path differs from live by as many pixels on this
/// very scene; a missing band, a wrong colour or an unlit stamp reused under a
/// light differ by tens of levels over hundreds of pixels.)
fn assert_close(got: &[u8], reference: &[u8], what: &str) {
    assert_eq!(got.len(), reference.len());
    let mut differing = 0usize;
    let mut worst = 0u8;
    for (a, b) in got.chunks_exact(4).zip(reference.chunks_exact(4)) {
        if a != b {
            differing += 1;
            worst = worst.max(a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0));
        }
    }
    let total = got.len() / 4;
    assert!(
        worst <= 2 && differing * 1000 <= total,
        "{what}: stamps differ from live: {} (gate: <= 2 levels, <= 0.1% of {total} pixels)",
        diff(got, reference)
    );
}

fn drawn_pixels(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|px| px[..3] != [0x20, 0x24, 0x2c])
        .count()
}

/// **Pixel parity.** Stamps on against `set_symbol_reuse(false)`, for a sun +
/// flat sky with `Figure` edges and shadows: first sighting (live), build, hit,
/// the rig moved, and back — at 1x and 4x, with and without a cull to the view
/// (at 4x part of the cast is off screen) — within the stamp parity gate
/// ([`assert_close`]). No instance is refused for its light and lit stamps were
/// taken.
#[test]
fn lit_stamps_do_not_change_a_pixel() {
    let Some(mut gpu) = gpu() else { return };

    // 1x on the whole cast; 4x on the rig's middle and on the scaled, rotated
    // plain instance — each with and without a cull to the view (at 4x much of
    // the cast is off screen, so the cull and the "mostly off screen" rule act).
    let cases = [
        (Focus::Cast, 1.0, false),
        (Focus::Cast, 1.0, true),
        (Focus::Object(RIG), 4.0, false),
        (Focus::Object(RIG), 4.0, true),
        (Focus::Object(ObjectId(2)), 4.0, false),
        (Focus::Object(ObjectId(2)), 4.0, true),
    ];
    for (focus, zoom, culled) in cases {
        let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
        let mut warm = DrawCache::new();
        let mut live = DrawCache::new();
        live.set_symbol_reuse(false);

        for pose in [FIRST, SECOND, FIRST] {
            set_pose(&mut scene, pose);
            let reference = gpu.render(&scene, 0, focus, zoom, culled, &mut live);
            assert!(
                drawn_pixels(&reference) > 2000,
                "the cast is on screen at {zoom}x on {focus:?}: {}",
                drawn_pixels(&reference)
            );
            for (pass, frame) in [0u32, 1, 0, 1].into_iter().enumerate() {
                let got = gpu.render(&scene, frame, focus, zoom, culled, &mut warm);
                assert_close(
                    &got,
                    &reference,
                    &format!("{zoom}x on {focus:?} (culled {culled}), pose {pose:?}, pass {pass}"),
                );
            }
        }
        assert_eq!(refused(&warm, "neutral_ctx"), 0, "no instance refused at {zoom}x");
        assert!(warm.symbol_scenes.stamps_lit > 0, "lit stamps were taken at {zoom}x");
        assert!(warm.symbol_scenes.builds > 0);
    }
}

/// A mixed sky and a lamp are refused (counted) and draw as live; so do
/// per-shape edges. (Their encodings are asserted byte-identical to the live
/// walk's in the counters test above; here the rendered pixels are held to the
/// stamp parity gate, because the GPU itself flickers by one level at a pixel.)
#[test]
fn refused_lights_still_draw_exactly_as_live() {
    let Some(mut gpu) = gpu() else { return };

    for (what, edges, sky, lamp) in [
        ("mixed sky", EdgeMode::Figure, Sky::Mixed, false),
        ("lamp", EdgeMode::Figure, Sky::Flat, true),
        ("shape edges", EdgeMode::Shapes, Sky::Flat, false),
    ] {
        let mut scene = stamp_scene(edges, sky, lamp);
        set_pose(&mut scene, FIRST);
        let mut warm = DrawCache::new();
        let mut live = DrawCache::new();
        live.set_symbol_reuse(false);
        for zoom in [1.0, 4.0] {
            let reference = gpu.render(&scene, 0, Focus::Cast, zoom, false, &mut live);
            assert!(drawn_pixels(&reference) > 2000, "{what}: the cast is on screen");
            for frame in [0u32, 1, 0] {
                let got = gpu.render(&scene, frame, Focus::Cast, zoom, false, &mut warm);
                assert_close(&got, &reference, &format!("{what} at {zoom}x"));
            }
        }
        assert!(refused(&warm, "neutral_ctx") > 0, "{what}: refused and counted");
        assert_eq!(warm.symbol_scenes.stamps_lit, 0, "{what}: no lit stamp");
    }
}

/// The same symbols under two rigs (two sun colours) each draw with their own
/// light — stamps are keyed on the rig — and each matches live.
#[test]
fn two_rigs_sharing_symbols_each_get_their_own_colours() {
    let Some(mut gpu) = gpu() else { return };

    let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
    set_pose(&mut scene, FIRST);
    let mut warm = DrawCache::new();
    let mut live = DrawCache::new();
    live.set_symbol_reuse(false);

    let mut seen = Vec::new();
    for sun in [Color::from_rgb8(0xFF, 0xF2, 0xD8), Color::from_rgb8(0xFF, 0x70, 0x30), Color::from_rgb8(0xFF, 0xF2, 0xD8)] {
        scene.lights_mut().lights[0].color = sun;
        let reference = gpu.render(&scene, 0, Focus::Cast, 1.0, false, &mut live);
        let mut last = Vec::new();
        for frame in [0u32, 1, 0, 1] {
            last = gpu.render(&scene, frame, Focus::Cast, 1.0, false, &mut warm);
            assert_close(&last, &reference, &format!("sun {sun:?}"));
        }
        seen.push(last);
    }
    assert!(seen[0] != seen[1], "the two suns really are different pictures");
    assert!(seen[0] == seen[2], "and going back gives the first one again");
}

// ---- pictures and gradients inside a lit stamp -------------------------------

/// A lit stage (sun, flat sky, `Figure` edges, shadows) with two instances of one
/// symbol that holds a **bitmap** (translucent pixels, so it is composited and
/// isolated under the light), a **gradient**-filled shape and a plain one: a
/// scaled and rotated instance and a mirrored one.
fn picture_scene() -> Scene {
    picture_scene_with(true, true)
}

fn picture_scene_with(with_image: bool, with_gradient: bool) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(W, H);
    let layer = scene.add_layer("Cast", LayerKind::Normal);
    let rig = scene.lights_mut();
    rig.enabled = true;
    rig.model = ShadeModel::Form;
    rig.modelling = 1.0;
    rig.edges = EdgeMode::Figure;
    rig.lights.push(Light::new(
        LightId(1),
        "Key",
        LightKind::Sun {
            azimuth: std::f64::consts::PI,
            elevation: 0.6,
        },
    ));
    let blue = Color::from_rgb8(0x50, 0x70, 0xE0);
    let mut sky = Light::new(LightId(2), "Fill", LightKind::sky());
    sky.color = blue;
    sky.intensity = 0.5;
    sky.kind = LightKind::Sky { horizon: blue };
    rig.lights.push(sky);

    let symbol = scene.add_symbol("picture", SymbolKind::Graphic, None);
    let inner = scene
        .library()
        .get(symbol)
        .expect("just added")
        .layers
        .iter()
        .next()
        .expect("a symbol always has a layer")
        .id;
    // An 8x8 picture: a ramp of colour with alpha from opaque to faint.
    let mut pixels = Vec::new();
    for y in 0..8u32 {
        for x in 0..8u32 {
            pixels.extend_from_slice(&[(x * 30) as u8, (y * 30) as u8, 160, (255 - (x + y) * 14) as u8]);
        }
    }
    let asset = Arc::new(ImageAsset::from_pixels(
        ImageId(1),
        "tile",
        8,
        8,
        Arc::new(pixels),
    ));
    let mut picture = ShapeData::filled(rect(10.0, 10.0, 70.0, 56.0), Color::WHITE);
    if let Some(fill) = picture.fill.as_mut() {
        fill.paint = Paint::Image(Box::new(ImageFill::new(
            Arc::clone(&asset),
            Rect::new(10.0, 10.0, 80.0, 66.0),
        )));
    }
    let mut ramp = ShapeData::filled(rect(90.0, 14.0, 50.0, 60.0), Color::WHITE);
    if let Some(fill) = ramp.fill.as_mut() {
        let mut gradient = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(0.0, Color::from_rgb8(220, 80, 60)),
                GradientStop::new(1.0, Color::from_rgb8(60, 90, 220)),
            ],
        );
        gradient.transform = Affine::translate((90.0, 14.0)) * Affine::scale_non_uniform(50.0, 60.0);
        fill.paint = Paint::Gradient(Arc::new(gradient));
    }
    let plain = ShapeData::filled(rect(40.0, 70.0, 60.0, 24.0), Color::from_rgb8(120, 170, 90));
    scene.library_mut().update(symbol, |s| {
        s.layers.update(inner, |l| {
            let mut shapes = vec![plain];
            if with_image {
                shapes.push(picture);
            }
            if with_gradient {
                shapes.push(ramp);
            }
            for (i, data) in shapes.into_iter().enumerate() {
                l.frames
                    .push_object(0, Arc::new(Object::shape(ObjectId(60_000 + i as u64), data)));
            }
        });
    });

    scene.add_object(
        layer,
        Object::instance_of(ObjectId(2), symbol).with_transform(
            Affine::translate((40.0, 30.0)) * Affine::rotate(0.25) * Affine::scale(1.3),
        ),
    );
    scene.add_object(
        layer,
        Object::instance_of(ObjectId(3), symbol)
            .with_transform(Affine::new([-1.0, 0.0, 0.0, 1.0, 460.0, 170.0])),
    );
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(10);
    });
    scene
}

/// `(differing pixels, worst channel step)` between two renders.
fn deviation(got: &[u8], reference: &[u8]) -> (usize, u8) {
    let mut count = 0usize;
    let mut worst = 0u8;
    for (a, b) in got.chunks_exact(4).zip(reference.chunks_exact(4)) {
        if a != b {
            count += 1;
            worst = worst.max(a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0));
        }
    }
    (count, worst)
}

/// **A picture is lit by compositing, inside an isolation group sized to its box
/// in the space it is encoded in** - for a rotated stamp not the group the live
/// walk lays (measured: 362 pixels / 29 levels off live, edge pixels at coverage
/// squared). So a symbol holding a bitmap draws live under a light: refused and
/// counted, the encoding the live walk's own, byte for byte.
#[test]
fn a_symbol_holding_a_picture_is_not_stamped_under_a_light() {
    let scene = picture_scene_with(true, false);
    let mut cache = DrawCache::new();
    let mut live = DrawCache::new();
    live.set_symbol_reuse(false);
    for frame in [0, 1, 0] {
        let encoded = encode(&scene, frame, true, &mut cache);
        let reference = encode(&scene, frame, true, &mut live);
        assert!(same_encoding(&encoded, &reference), "a refused picture is the live encoding");
    }
    assert!(refused(&cache, "neutral_ctx") > 0, "refused and counted");
    assert_eq!(cache.symbol_scenes.stamps_lit, 0);

    // Unlit, the same symbol still stamps (it is exact without the light).
    let mut unlit = DrawCache::new();
    for frame in [0, 1, 0] {
        encode(&scene, frame, false, &mut unlit);
    }
    assert!(unlit.symbol_scenes.stamps > 0, "unlit pictures still stamp");
}

/// A gradient is lit stop by stop and rides in the lit stamp. The lit stamp
/// deviates from live no more than the *unlit* stamp does on the same scene
/// (gradients in a rotated stamp carry a small noise of their own, 88 pixels /
/// 4 levels, with or without a light): the light adds none.
#[test]
fn a_lit_gradient_inside_a_stamp_deviates_no_more_than_an_unlit_one() {
    let scene = picture_scene_with(false, true);
    let mut cache = DrawCache::new();
    for frame in [0, 1, 0] {
        encode(&scene, frame, true, &mut cache);
    }
    assert_eq!(refused(&cache, "neutral_ctx"), 0);
    assert!(cache.symbol_scenes.stamps_lit > 0, "lit stamps were taken");

    let Some(mut gpu) = gpu() else { return };
    let mut measured = Vec::new();
    for lit in [false, true] {
        gpu.lit = lit;
        let mut warm = DrawCache::new();
        let mut live = DrawCache::new();
        live.set_symbol_reuse(false);
        let reference = gpu.render(&scene, 0, Focus::Cast, 1.0, false, &mut live);
        assert!(drawn_pixels(&reference) > 2000);
        let mut got = Vec::new();
        for frame in [0u32, 1, 0, 1] {
            got = gpu.render(&scene, frame, Focus::Cast, 1.0, false, &mut warm);
        }
        measured.push(deviation(&got, &reference));
    }
    let (unlit, lit) = (measured[0], measured[1]);
    assert!(
        lit.0 <= unlit.0 + 8 && lit.1 <= unlit.1.max(2),
        "lit {lit:?} vs unlit {unlit:?} (pixels, worst level) against live"
    );
}
