//! **A held lit layer appends its shadow pass instead of building it.**
//! `docs/RIG_SCENE_LAG_PLAN.md`, step L8 prime.
//!
//! The shadow pass of a lit layer (the alpha group, every caster's silhouette
//! thrown by the light, the group closed) is kept as an encoded fragment while
//! what is on the layer, the light and the view hold still. The counters run
//! the real draw walk on the CPU; the pixel test renders on the GPU and skips,
//! as the other GPU tests do, when there is no adapter. The bounds tests cover
//! the rig-extent memo the shadow pass reads.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Camera, Point, Rect, Shape as _, Size};
use buzz_render::document::{self, DrawCache, FrameOptions};
use buzz_render::{GpuContext, GpuPreference, SceneBuilder, vello, wgpu};
use buzz_scene::{
    ArmatureData, EdgeMode, LayerKind, Light, LightId, LightKind, Object, ObjectId, ObjectKind,
    Scene, ShadeModel, ShapeData, StrokeSpec, SymbolId, SymbolKind,
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
#[allow(dead_code)]
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

/// Encode `frame` through `cache` the way the window does (CPU only), looking at
/// `centre`.
fn encode(scene: &Scene, frame: u32, centre: Point, cache: &mut DrawCache) -> vello::Scene {
    let camera = Camera::new(centre, 1.0, Size::new(W, H));
    let mut encoded = vello::Scene::new();
    let mut builder = SceneBuilder::new(&mut encoded, &camera);
    let options = FrameOptions {
        lit: true,
        ..FrameOptions::default()
    };
    cache.begin();
    document::draw_frame_within(&mut builder, scene, frame, Affine::IDENTITY, &options, cache);
    cache.end();
    encoded
}

const MIDDLE: Point = Point::new(W / 2.0, H / 2.0);

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
#[derive(Clone, Copy, Debug, PartialEq)]
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


/// The GPU renders one encoding with a one-level flicker at the odd pixel (about
/// 1 render in 20 of the same encoding differs at one pixel by one level), so
/// "the same picture" is: at most two pixels differ and none by more than one
/// level. The encodings themselves are compared byte for byte, on the CPU.
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
    assert!(
        differing <= 2 && worst <= 1,
        "{what}: {}",
        diff(got, reference)
    );
}

fn drawn_pixels(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|px| px[..3] != [0x20, 0x24, 0x2c])
        .count()
}


// ---- the rig-extent memo ---------------------------------------------------

fn rig_object(scene: &Scene) -> Arc<Object> {
    Arc::clone(scene.find_object(RIG).expect("the rig is on the stage").1)
}

/// L8 prime-1: a held rig is measured once however often it is asked, a moved
/// one (a new object) is measured again, an edited library measures everything
/// again, and every answer is the number `Scene::resolved_bounds` gives.
#[test]
fn a_held_rig_is_measured_once_and_a_moved_or_edited_one_again() {
    let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
    set_pose(&mut scene, FIRST);
    let mut cache = DrawCache::new();
    cache.begin();

    let rig = rig_object(&scene);
    let first = cache.bounds.resolved_for_test(&rig, &scene);
    let again = cache.bounds.resolved_for_test(&rig, &scene);
    assert_eq!(cache.bounds.computed(), 1, "the second ask is a lookup");
    assert_eq!(first, again);
    assert_eq!(first, scene.resolved_bounds(&rig), "and it is the uncached value");

    // The rig moves: the edit gives the stage a new object (the cache holds the
    // old one), which is measured on its own.
    set_pose(&mut scene, SECOND);
    let moved = rig_object(&scene);
    assert!(!Arc::ptr_eq(&rig, &moved), "a re-posed rig is a new object");
    let second = cache.bounds.resolved_for_test(&moved, &scene);
    assert_eq!(cache.bounds.computed(), 2, "a changed pose is measured again");
    assert_eq!(second, scene.resolved_bounds(&moved));
    assert_ne!(first, second, "the pose really moved the extent");
    cache.bounds.resolved_for_test(&moved, &scene);
    assert_eq!(cache.bounds.computed(), 2, "and held again it is a lookup");

    // An edit to the library (a symbol the rig's parts are instances of) can
    // change an instance's extent without touching the instance: everything is
    // measured again.
    let symbol = scene.library().iter().next().expect("symbols").id;
    scene.library_mut().update(symbol, |s| s.name = "renamed".to_string());
    let edited = cache.bounds.resolved_for_test(&moved, &scene);
    assert_eq!(cache.bounds.computed(), 3, "an edited library measures again");
    assert_eq!(edited, scene.resolved_bounds(&moved));
}

// ---- counters (CPU) --------------------------------------------------------

/// L8 prime-2: the pass is encoded and kept on the first frame, appended on a
/// held one, and encoded again when the rig moves or the camera does; two
/// fragments per layer at most; the kill switch keeps nothing.
#[test]
fn a_held_layer_appends_its_shadow_pass_and_only_a_change_rebuilds_it() {
    let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
    set_pose(&mut scene, FIRST);
    let mut cache = DrawCache::new();

    encode(&scene, 0, MIDDLE, &mut cache);
    assert_eq!(cache.shadow_memo_counts(), (0, 1), "the first frame keeps its pass");

    // Held (frame 1 is the same keyframe): appended.
    encode(&scene, 1, MIDDLE, &mut cache);
    encode(&scene, 0, MIDDLE, &mut cache);
    assert_eq!(cache.shadow_memo_counts(), (2, 1), "held frames append it");

    // The rig moved: encoded again; then held again at the new pose.
    set_pose(&mut scene, SECOND);
    encode(&scene, 0, MIDDLE, &mut cache);
    assert_eq!(cache.shadow_memo_counts(), (2, 2), "a moved rig rebuilds the pass");
    encode(&scene, 1, MIDDLE, &mut cache);
    assert_eq!(cache.shadow_memo_counts(), (3, 2));

    // The camera moved: the fragment holds render-space coordinates, so again.
    let shifted = Point::new(MIDDLE.x + 31.0, MIDDLE.y - 7.0);
    encode(&scene, 0, shifted, &mut cache);
    assert_eq!(cache.shadow_memo_counts(), (3, 3), "a moved camera rebuilds the pass");
    encode(&scene, 0, shifted, &mut cache);
    assert_eq!(cache.shadow_memo_counts(), (4, 3));

    // At most two fragments per layer, and ones unused for two frames go: the
    // first view's is long gone.
    encode(&scene, 0, MIDDLE, &mut cache);
    assert_eq!(cache.shadow_memo_counts().0, 4, "an old view's fragment was dropped");

    // Off: nothing is kept or read.
    let mut off = DrawCache::new();
    off.set_shadow_memo(false);
    for frame in [0, 1, 0] {
        encode(&scene, frame, MIDDLE, &mut off);
    }
    assert_eq!(off.shadow_memo_counts(), (0, 0));
}

// ---- pixels (GPU) ----------------------------------------------------------

/// The memo changes no byte of the encoding: miss (encoded into a fragment and
/// appended), hit (the kept fragment appended), rig moved, camera moved, back —
/// every stream, transform and style of the frame equals what the live pass
/// (memo off) encodes.
#[test]
fn the_shadow_memo_encodes_exactly_the_live_bytes() {
    let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
    let mut on = DrawCache::new();
    let mut off = DrawCache::new();
    off.set_shadow_memo(false);
    let shifted = Point::new(MIDDLE.x + 31.0, MIDDLE.y - 7.0);
    let plan: [(&[f64], Point); 8] = [
        (FIRST, MIDDLE),
        (FIRST, MIDDLE),
        (FIRST, MIDDLE),
        (SECOND, MIDDLE),
        (SECOND, MIDDLE),
        (SECOND, shifted),
        (SECOND, shifted),
        (FIRST, MIDDLE),
    ];
    let mut current: &[f64] = &[];
    for (step, (pose, centre)) in plan.iter().enumerate() {
        if current != *pose {
            set_pose(&mut scene, pose);
            current = pose;
        }
        let frame = (step % 2) as u32;
        let memoised = encode(&scene, frame, *centre, &mut on);
        let live = encode(&scene, frame, *centre, &mut off);
        assert!(
            same_encoding(&memoised, &live),
            "step {step}: the memoised frame encodes different bytes from the live one"
        );
    }
    let (hits, misses) = on.shadow_memo_counts();
    assert!(hits >= 3 && misses >= 4, "the plan hit and missed: {hits} / {misses}");
}

/// **Pixel identity.** Shadows on (a sun and a flat sky, `Figure` edges, a rig of
/// symbol parts and two plain instances), the memo on against the memo off (held
/// to the gate that allows the GPU's own one-level flicker at a pixel; the
/// encodings are asserted byte-identical in the CPU test above), through: held frames, the rig moved, the camera moved, back —
/// at 1x, 1x culled and 4x. The counters show a hit on a held frame and only on
/// a held frame.
#[test]
fn the_shadow_memo_does_not_change_a_pixel() {
    let Some(mut gpu) = gpu() else { return };

    for (zoom, culled, a, b) in [
        (1.0, false, Focus::Cast, Focus::Object(RIG)),
        (1.0, true, Focus::Cast, Focus::Object(RIG)),
        (4.0, false, Focus::Object(RIG), Focus::Object(ObjectId(2))),
    ] {
        let mut scene = stamp_scene(EdgeMode::Figure, Sky::Flat, false);
        let mut on = DrawCache::new();
        let mut off = DrawCache::new();
        off.set_shadow_memo(false);

        let plan = [
            (FIRST, a),
            (FIRST, a),
            (FIRST, a),
            (SECOND, a),
            (SECOND, a),
            (SECOND, b),
            (SECOND, b),
            (FIRST, a),
            (FIRST, a),
        ];
        let mut current: &[f64] = &[];
        for (step, (pose, focus)) in plan.iter().enumerate() {
            // Only when the pose changes: editing the rig in place, even to the
            // same pose, is a new object (and the memo rightly treats it so).
            if current != *pose {
                set_pose(&mut scene, pose);
                current = pose;
            }
            let frame = (step % 2) as u32;
            let before = on.shadow_memo_counts();
            let memoised = gpu.render(&scene, frame, *focus, zoom, culled, &mut on);
            let reference = gpu.render(&scene, frame, *focus, zoom, culled, &mut off);
            assert!(
                drawn_pixels(&reference) > 2000,
                "the cast is on screen at {zoom}x on {focus:?}"
            );
            assert_close(
                &memoised,
                &reference,
                &format!("{zoom}x (culled {culled}), step {step}"),
            );
            let after = on.shadow_memo_counts();
            let held = step > 0 && plan[step] == plan[step - 1];
            let expected = if held { (1, 0) } else { (0, 1) };
            assert_eq!(
                (after.0 - before.0, after.1 - before.1),
                expected,
                "step {step} at {zoom}x: a hit exactly on a held frame"
            );
        }
        assert_eq!(off.shadow_memo_counts(), (0, 0));
    }
}
