//! **A lit scene of rigged characters stops rebuilding its lighting geometry
//! every frame.** `docs/RIG_SCENE_LAG_PLAN.md`, steps L1, L2 and L0.
//!
//! Everything here runs the real draw walk (`draw_frame_within`) into a Vello
//! scene on the CPU — no GPU is needed to *encode* a frame — and reads the
//! counters the caches expose to see what a frame built. The pixel-identity
//! test at the bottom renders on the GPU and skips, as the other GPU tests do,
//! when there is no adapter.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Camera, Point, Rect, Shape as _, Size};
use buzz_render::document::{self, DrawCache, FrameOptions};
use buzz_render::{GpuContext, GpuPreference, SceneBuilder, vello, wgpu};
use buzz_scene::{
    ArmatureData, EdgeMode, LayerId, LayerKind, Light, LightId, LightKind, Object, ObjectId,
    ObjectKind, Scene, ShadeModel, ShapeData, StrokeSpec, SymbolKind,
};
use peniko::Color;

const W: f64 = 480.0;
const H: f64 = 320.0;

/// A rectangle as a path.
fn rect(x: f64, y: f64, w: f64, h: f64) -> BezPath {
    Rect::new(x, y, x + w, y + h).to_path(1e-9)
}

/// A scene with one layer, a key light that rims and models, and nothing on
/// the layer yet.
fn lit_scene(edges: EdgeMode, rim: f32) -> (Scene, LayerId) {
    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(W, H);
    let layer = scene.add_layer("Rig", LayerKind::Normal);

    scene.lights_mut().enabled = true;
    scene.lights_mut().model = ShadeModel::Form;
    scene.lights_mut().modelling = 1.0;
    scene.lights_mut().edges = edges;
    let mut sun = Light::new(
        LightId(1),
        "Key",
        LightKind::Sun {
            azimuth: std::f64::consts::PI,
            elevation: 0.5,
        },
    );
    sun.intensity = 2.0;
    sun.rim = rim;
    scene.lights_mut().lights.push(sun);
    (scene, layer)
}

/// An armature object wrapping `data`.
fn rig_object(id: u64, data: ArmatureData) -> Object {
    let mut object = Object::group(ObjectId(id), Vec::new());
    object.kind = ObjectKind::Armature(data);
    object
}

/// A rig of `parts` bare filled shapes, one rigid part per bone-ish slot.
fn bare_rig(parts: usize) -> ArmatureData {
    let mut data = ArmatureData::chain(&[60.0, 60.0, 60.0]);
    for i in 0..parts {
        let shape = Object::shape(
            ObjectId(100 + i as u64),
            ShapeData::filled(
                rect(40.0 + i as f64 * 22.0, 60.0 + (i % 3) as f64 * 15.0, 30.0, 40.0),
                Color::from_rgb8(200, 120 + (i * 9 % 100) as u8, 90),
            ),
        );
        data.bind_rigid(Arc::new(shape), i % 3);
    }
    data
}

/// Draw `frame` through `cache` the way the window does.
fn draw(scene: &Scene, frame: u32, cache: &mut DrawCache) -> vello::Scene {
    let camera = Camera::new(Point::new(W / 2.0, H / 2.0), 1.0, Size::new(W, H));
    let mut encoded = vello::Scene::new();
    let mut builder = SceneBuilder::new(&mut encoded, &camera);
    let options = FrameOptions {
        lit: true,
        ..FrameOptions::default()
    };
    cache.begin();
    document::draw_frame_within(
        &mut builder,
        scene,
        frame,
        Affine::IDENTITY,
        &options,
        cache,
    );
    cache.end();
    encoded
}

/// L1: a held pose gives the cache the same parts, so the second frame's
/// crescents are all hits.
#[test]
fn a_held_lit_rig_builds_its_crescents_once() {
    let (mut scene, layer) = lit_scene(EdgeMode::Shapes, 0.0);
    scene.add_object(layer, rig_object(1, bare_rig(12)));

    let mut cache = DrawCache::new();
    draw(&scene, 0, &mut cache);
    let built = cache.lights.builds();
    let places = cache.lights.len();
    assert!(built >= 12, "the first frame builds a crescent per shape: {built}");
    assert!(places >= 12, "and keeps them: {places}");

    draw(&scene, 0, &mut cache);
    assert_eq!(
        cache.lights.builds(),
        built,
        "the same pose on the next frame builds nothing"
    );
    assert_eq!(cache.lights.len(), places, "and adds no placements");
    assert_eq!(
        cache.lights.inline_spent(),
        std::time::Duration::ZERO,
        "so nothing was spent building on the second frame"
    );
}

/// L1: a moved bone is a different pose, so the parts it moves are rebuilt —
/// the cache misses exactly as it does for a moved object.
#[test]
fn a_moved_bone_rebuilds_the_crescents() {
    let (mut scene, layer) = lit_scene(EdgeMode::Shapes, 0.0);
    scene.add_object(layer, rig_object(1, bare_rig(12)));

    let mut cache = DrawCache::new();
    draw(&scene, 0, &mut cache);
    let built = cache.lights.builds();

    // A new pose is a new object in the document (copy-on-write), exactly as
    // a keyframe edit makes one.
    let (mut moved, layer) = lit_scene(EdgeMode::Shapes, 0.0);
    let mut data = bare_rig(12);
    data.armature.set_pose(&[0.4, -0.3, 0.2]);
    moved.add_object(layer, rig_object(1, data));
    draw(&moved, 0, &mut cache);
    assert!(
        cache.lights.builds() > built,
        "a different pose has to build its own crescents"
    );
}

/// A tweened or modifier-posed rig has no stored object behind it, so its
/// parts stay owner-less and are built inline: nothing is queued for later,
/// and the frame is not stale. (This is what keeps a walking, tweened rig
/// exactly as lit as it was.)
#[test]
fn a_rig_with_no_stored_object_is_built_inline_not_deferred() {
    let (mut scene, layer) = lit_scene(EdgeMode::Shapes, 0.0);
    // Two keyframes with different poses: the frames between are tweened.
    let a = rig_object(1, bare_rig(6));
    let mut b_data = bare_rig(6);
    b_data.armature.set_pose(&[0.5, 0.2, -0.2]);
    let b = rig_object(1, b_data);
    scene.add_object_at(layer, 0, a);
    scene.update_layer(layer, |l| {
        l.frames.insert_keyframe(8);
        l.frames.push_object(8, Arc::new(b));
        l.frames.set_tween(0, buzz_scene::Tween::classic());
    });

    let mut cache = DrawCache::new();
    cache.lights.set_inline_budget(std::time::Duration::ZERO);
    draw(&scene, 4, &mut cache);
    assert!(
        !cache.lights.is_stale(),
        "a tweened rig is built on the spot even with no inline budget"
    );
    assert!(cache.lights.take_misses().is_empty(), "nothing to defer to");
}

/// A symbol of `frames` frames; each frame holds a few shapes that differ from
/// frame to frame, so an animated symbol has a different silhouette per frame.
fn part_symbol(scene: &mut Scene, name: &str, frames: u32, seed: f64) -> buzz_scene::SymbolId {
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
    scene.library_mut().update(id, |symbol| {
        symbol.layers.update(layer, |l| {
            for frame in 0..frames {
                if frame > 0 {
                    l.frames.insert_keyframe(frame);
                }
                for k in 0..4u64 {
                    let shape = Object::shape(
                        ObjectId(1000 + frame as u64 * 10 + k),
                        ShapeData::filled(
                            rect(
                                seed + k as f64 * 14.0,
                                seed + frame as f64 * 9.0,
                                24.0,
                                30.0 + k as f64 * 4.0,
                            ),
                            Color::from_rgb8(180, 100 + (k * 30) as u8, 110),
                        ),
                    );
                    l.frames.push_object(frame, Arc::new(shape));
                }
            }
        });
    });
    id
}

/// A rig whose parts are instances of `symbols` (the "slimmed rig" shape: each
/// rigid part is a symbol instance), each riding a bone of a three-bone chain.
fn instance_rig(symbols: &[buzz_scene::SymbolId]) -> ArmatureData {
    let mut data = ArmatureData::chain(&[60.0, 60.0, 60.0]);
    for (i, symbol) in symbols.iter().enumerate() {
        let part = Object::instance_of(ObjectId(200 + i as u64), *symbol)
            .with_transform(Affine::translate((i as f64 * 70.0, 0.0)));
        data.bind_rigid(Arc::new(part), i % 3);
    }
    data
}

/// A lit scene, default edges (`Figure`), holding one rig on a layer long
/// enough that frames 0 and 1 are the same held keyframe — only `elapsed`
/// differs between them.
fn held_instance_rig_scene(frames_per_part: u32) -> Scene {
    let (mut scene, layer) = lit_scene(EdgeMode::Figure, 0.0);
    let symbols: Vec<_> = (0..4)
        .map(|i| part_symbol(&mut scene, &format!("part{i}"), frames_per_part, 20.0 + i as f64 * 8.0))
        .collect();
    scene.add_object(layer, rig_object(1, instance_rig(&symbols)));
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(10);
    });
    scene
}

/// L2: a held lit rig of static symbol instances keeps its figure geometry
/// from one frame to the next — `elapsed` moved, nothing it draws did.
#[test]
fn a_held_rig_of_static_symbols_reuses_its_figure_on_the_next_frame() {
    let scene = held_instance_rig_scene(1);
    let mut cache = DrawCache::new();
    draw(&scene, 0, &mut cache);
    let built = cache.lights.builds();
    let places = cache.lights.len();
    assert!(built >= 1, "the first frame builds the figure: {built}");

    draw(&scene, 1, &mut cache);
    assert_eq!(
        cache.lights.builds(),
        built,
        "frame 1 (elapsed 1, same pose) reuses the figure geometry"
    );
    assert_eq!(cache.lights.len(), places, "and adds no entry");

    draw(&scene, 7, &mut cache);
    assert_eq!(cache.lights.builds(), built, "nor does frame 7");
}

/// L2: an animated symbol inside the rig really does change the silhouette
/// with the playhead, so the figure is rebuilt when `elapsed` moves.
#[test]
fn a_rig_with_an_animated_symbol_rebuilds_its_figure_as_it_plays() {
    let scene = held_instance_rig_scene(2);
    let mut cache = DrawCache::new();
    draw(&scene, 0, &mut cache);
    let built = cache.lights.builds();
    assert!(built >= 1);

    draw(&scene, 1, &mut cache);
    assert!(
        cache.lights.builds() > built,
        "the symbols are on another frame: the silhouette is new"
    );
}

/// L2: a different pose is a different figure.
#[test]
fn a_changed_pose_misses_the_figure_cache() {
    let scene = held_instance_rig_scene(1);
    let mut cache = DrawCache::new();
    draw(&scene, 0, &mut cache);
    let built = cache.lights.builds();

    let (mut moved, layer) = lit_scene(EdgeMode::Figure, 0.0);
    let symbols: Vec<_> = (0..4)
        .map(|i| part_symbol(&mut moved, &format!("part{i}"), 1, 20.0 + i as f64 * 8.0))
        .collect();
    let mut data = instance_rig(&symbols);
    data.armature.set_pose(&[0.5, 0.3, -0.4]);
    moved.add_object(layer, rig_object(1, data));
    draw(&moved, 0, &mut cache);
    assert!(
        cache.lights.builds() > built,
        "a re-posed rig has to build its own figure"
    );
}

// ---------------------------------------------------------------------------
// L0: the layer rim and its silhouette
// ---------------------------------------------------------------------------

/// A part artwork: `n` small shapes in a group, a couple of them stroked, so
/// the silhouette has fills, strokes and a lot of subpaths.
fn part_artwork(id: u64, n: usize, x: f64, stroked: usize) -> Object {
    let children: Vec<Arc<Object>> = (0..n)
        .map(|i| {
            let mut data = ShapeData::filled(
                rect(
                    x + (i % 6) as f64 * 9.0,
                    40.0 + (i / 6) as f64 * 11.0,
                    14.0,
                    10.0,
                ),
                Color::from_rgb8(190, 90 + (i * 7 % 120) as u8, 80),
            );
            if i < stroked {
                data.stroke = Some(StrokeSpec::new(Color::from_rgb8(30, 30, 40), 2.5));
            }
            Arc::new(Object::shape(ObjectId(id * 1000 + i as u64), data))
        })
        .collect();
    Object::group(ObjectId(id), children)
}

/// The synthetic rig of the plan pixel test: three rigid parts of thirty
/// shapes each (two of them stroked), one part **mirrored**.
fn group_rig() -> ArmatureData {
    let mut data = ArmatureData::chain(&[70.0, 70.0, 70.0]);
    let a = part_artwork(1, 30, 60.0, 2);
    let b = part_artwork(2, 30, 130.0, 0);
    // Mirrored about x = 200: the winding of every outline it contributes
    // flips, which the silhouette has to decide after placing it.
    let c = part_artwork(3, 30, 130.0, 0)
        .with_transform(Affine::new([-1.0, 0.0, 0.0, 1.0, 400.0, 0.0]));
    data.bind_rigid(Arc::new(a), 0);
    data.bind_rigid(Arc::new(b), 1);
    data.bind_rigid(Arc::new(c), 2);
    data
}

fn rim_scene(data: ArmatureData, frames: u32) -> (Scene, LayerId) {
    let (mut scene, layer) = lit_scene(EdgeMode::Figure, 0.6);
    scene.add_object(layer, rig_object(1, data));
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(frames);
    });
    (scene, layer)
}

fn posed(pose: &[f64]) -> ArmatureData {
    let mut data = group_rig();
    data.armature.set_pose(pose);
    data
}

/// L0: a lit layer with one rig, the same pose on two frames, gathers its
/// silhouette once and shares the rim ops between the two.
#[test]
fn a_held_rig_layer_gathers_and_builds_its_rim_once() {
    let (scene, layer) = rim_scene(posed(&[0.3, -0.2, 0.4]), 10);
    let mut cache = DrawCache::new();

    draw(&scene, 0, &mut cache);
    let first = cache.layer_fx_probe(layer).expect("a rim was built");
    assert!(
        !first.behind.is_empty() || !first.over.is_empty(),
        "the test must actually lay a rim"
    );
    let work = cache.silhouette_work();
    assert!(work.1 > 0, "a rigid part is gathered from its memo: {work:?}");

    draw(&scene, 1, &mut cache);
    let second = cache.layer_fx_probe(layer).expect("still there");
    assert!(Arc::ptr_eq(&first, &second), "the same pose shares its ops");
    assert_eq!(
        cache.silhouette_work(),
        work,
        "and the second frame gathered nothing at all"
    );
    let (hits, _) = cache.layer_fx_counts();
    assert_eq!(hits, 1, "one hit, on the second frame");
}

/// L0: a moved bone rebuilds the rim, and rebuilds it from the part memos,
/// without walking a single shape live.
#[test]
fn a_changed_pose_rebuilds_the_rim_from_the_parts_memo() {
    // One scene, two keyframes: the second pose is the first rig re-posed, so
    // the two share every part artwork (as two poses of one character do).
    let first_pose = posed(&[0.3, -0.2, 0.4]);
    let mut moved = first_pose.clone();
    moved.armature.set_pose(&[0.5, 0.1, -0.3]);
    let (mut scene, layer) = lit_scene(EdgeMode::Figure, 0.6);
    scene.add_object_at(layer, 0, rig_object(1, first_pose));
    scene.update_layer(layer, |l| {
        l.frames.insert_keyframe(5);
        l.frames.push_object(5, Arc::new(rig_object(1, moved)));
        l.frames.insert_frame(10);
    });
    let mut cache = DrawCache::new();

    draw(&scene, 0, &mut cache);
    let first = cache.layer_fx_probe(layer).expect("a rim");
    let (live_before, replayed_before, recorded_before) = cache.silhouette_work();
    assert_eq!(recorded_before, 3, "three parts recorded once");

    draw(&scene, 5, &mut cache);
    let second = cache.layer_fx_probe(layer).expect("a rim");
    assert!(!Arc::ptr_eq(&first, &second), "a different pose, different ops");
    let (live, replayed, recorded) = cache.silhouette_work();
    assert_eq!(live, live_before, "no shape was walked live: {live}");
    assert_eq!(recorded, recorded_before, "no part was re-recorded");
    assert!(replayed > replayed_before, "the part outlines were replayed");
}

/// L0: a rig with no stored object (a tween) is keyed on its pose: the same
/// tweened frame drawn twice hits, the next frame misses.
#[test]
fn a_tweened_rig_layer_is_keyed_on_its_pose() {
    let (mut scene, layer) = lit_scene(EdgeMode::Figure, 0.6);
    let a = rig_object(1, posed(&[0.0, 0.0, 0.0]));
    let b = rig_object(1, posed(&[0.6, 0.3, -0.4]));
    scene.add_object_at(layer, 0, a);
    scene.update_layer(layer, |l| {
        l.frames.insert_keyframe(8);
        l.frames.push_object(8, Arc::new(b));
        l.frames.set_tween(0, buzz_scene::Tween::classic());
    });

    let mut cache = DrawCache::new();
    draw(&scene, 3, &mut cache);
    let at3 = cache.layer_fx_probe(layer).expect("a rim");
    let work = cache.silhouette_work();

    draw(&scene, 3, &mut cache);
    let again = cache.layer_fx_probe(layer).expect("a rim");
    assert!(Arc::ptr_eq(&at3, &again), "the same tweened pose hits");
    assert_eq!(cache.silhouette_work(), work, "and gathers nothing");

    draw(&scene, 4, &mut cache);
    let at4 = cache.layer_fx_probe(layer).expect("a rim");
    assert!(!Arc::ptr_eq(&at3, &at4), "the next frame is another pose");
}

/// L0: an animated symbol in a part moves the silhouette with the playhead, so
/// the rim of the layer is rebuilt as it plays, and a static one is not.
#[test]
fn only_an_animated_symbol_moves_the_rim_with_the_playhead() {
    for (frames_per_part, expect_shared) in [(1u32, true), (2u32, false)] {
        let (mut scene, layer) = lit_scene(EdgeMode::Figure, 0.6);
        let symbols: Vec<_> = (0..4)
            .map(|i| {
                part_symbol(
                    &mut scene,
                    &format!("part{i}"),
                    frames_per_part,
                    20.0 + i as f64 * 8.0,
                )
            })
            .collect();
        scene.add_object(layer, rig_object(1, instance_rig(&symbols)));
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(10);
        });

        let mut cache = DrawCache::new();
        draw(&scene, 0, &mut cache);
        let first = cache.layer_fx_probe(layer).expect("a rim");
        draw(&scene, 1, &mut cache);
        let second = cache.layer_fx_probe(layer).expect("a rim");
        assert_eq!(
            Arc::ptr_eq(&first, &second),
            expect_shared,
            "{frames_per_part}-frame symbols: ops shared across frames = {expect_shared}"
        );
    }
}

/// L0: with the memo off, nothing is kept and nothing is shared.
#[test]
fn the_kill_switch_keeps_nothing() {
    let (scene, layer) = rim_scene(posed(&[0.3, -0.2, 0.4]), 10);
    let mut cache = DrawCache::new();
    cache.set_layer_fx_memo(false);
    draw(&scene, 0, &mut cache);
    draw(&scene, 1, &mut cache);
    assert!(cache.layer_fx_probe(layer).is_none());
    assert_eq!(cache.layer_fx_counts(), (0, 0));
    let (_, replayed, recorded) = cache.silhouette_work();
    assert_eq!((replayed, recorded), (0, 0), "every gather is the live walk");
}

// ---- pixels ---------------------------------------------------------------

const PW: u32 = 512;
const PH: u32 = 256;
const BACKDROP: Color = Color::from_rgb8(0x20, 0x24, 0x2c);

struct Gpu {
    gpu: GpuContext,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

fn gpu() -> Option<Gpu> {
    let gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping rig lag pixel test: no usable GPU ({e})");
            return None;
        }
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rig-lag-target"),
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
        label: Some("rig-lag-readback"),
        size: (PW * PH * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Some(Gpu {
        gpu,
        texture,
        view,
        readback,
    })
}

impl Gpu {
    /// Draw `frame` of `scene` through `cache` at `zoom`, render it, read it back.
    fn render(&mut self, scene: &Scene, frame: u32, zoom: f64, cache: &mut DrawCache) -> Vec<u8> {
        // Centred on the artwork, so a 4x view is still looking at the rig.
        let centre = scene
            .layers()
            .iter()
            .filter_map(|layer| layer.bounds_at(frame))
            .reduce(|a, b| a.union(b))
            .map(|bounds| bounds.center())
            .unwrap_or(Point::new(250.0, 100.0));
        let camera = Camera::new(centre, zoom, Size::new(PW as f64, PH as f64));
        let mut encoded = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut encoded, &camera);
        let options = FrameOptions {
            lit: true,
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

/// **Pixel identity.** A lit scene (a sun with a 0.6 rim, `EdgeMode::Figure`,
/// one rig of three rigid parts of thirty shapes, two stroked, one part
/// mirrored) rendered through a warm memo (cold, then a hit, then another pose
/// through the part memos, then back) is byte-identical to the live walk with
/// the memo off, at 1x and 4x.
#[test]
fn the_memo_does_not_change_a_pixel() {
    let Some(mut gpu) = gpu() else { return };

    let first: &[f64] = &[0.3, -0.2, 0.4];
    let second: &[f64] = &[0.55, 0.15, -0.35];
    for zoom in [1.0, 4.0] {
        // The persistent, memo-on cache.
        let mut warm = DrawCache::new();
        warm.set_layer_fx_memo(true);
        let mut checked = 0;
        for pose in [first, second, first] {
            let (scene, layer) = rim_scene(posed(pose), 10);
            let (live_scene, _) = rim_scene(posed(pose), 10);

            let memo_cold_or_warm = gpu.render(&scene, 0, zoom, &mut warm);
            let memo_again = gpu.render(&scene, 1, zoom, &mut warm);

            let mut live = DrawCache::new();
            live.set_layer_fx_memo(false);
            let reference = gpu.render(&live_scene, 0, zoom, &mut live);

            let drawn = reference
                .chunks_exact(4)
                .filter(|px| px[..3] != [0x20, 0x24, 0x2c])
                .count();
            assert!(
                drawn > 2000,
                "the rig and its rim are on screen at {zoom}x: {drawn}"
            );
            let ops = warm.layer_fx_probe(layer).expect("a rim was laid");
            assert!(!ops.behind.is_empty() || !ops.over.is_empty());

            assert!(
                memo_cold_or_warm == reference,
                "memo on differs from the live walk at {zoom}x, pose {pose:?}"
            );
            assert!(
                memo_again == reference,
                "memo hit differs from the live walk at {zoom}x, pose {pose:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 3);
        let (hits, _) = warm.layer_fx_counts();
        assert!(hits >= 3, "each pose second frame was a memo hit: {hits}");
    }
}
