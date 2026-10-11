//! E7 of the revised S5 in `docs/BIMARI5_LAG_PLAN.md` (and item 0 of it): a stamp
//! built while the abort flag is raised is never kept; a jump's target frame has
//! its stamps built on up to 8 workers and installed before the render thread
//! encodes it warm; a scrub pre-builds the frames ahead of it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use buzz_app::editor::Editor;
use buzz_app::encode_ahead::{Epoch, SEEK_WORKERS, SeekAhead};
use buzz_app::stage::{self, StageInputs};
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Size};
use buzz_jobs::JobSystem;
use buzz_render::document::{DrawCache, LightDetail};
use buzz_render::{GpuContext, GpuPreference, wgpu};
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

/// Six cast layers of characters (two kinds, each made of animated `part`
/// instances) and a sky: enough layers to fan out, nested animated symbols as in
/// a real film.
fn cast_editor() -> Editor {
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

    let mut kinds = Vec::new();
    for (name, parts, rgb) in [("character", 3u64, (40, 160, 40)), ("second", 2, (160, 40, 160))] {
        let sym = scene.add_symbol(name, SymbolKind::Graphic, None);
        scene.library_mut().update(sym, |s| {
            let inner = s.layers.iter().next().map(|l| l.id).expect("a layer");
            s.layers.update(inner, |l| {
                for i in 0..parts {
                    l.frames.push_object(
                        0,
                        Arc::new(
                            Object::instance_of(ObjectId(100 + i), part)
                                .with_transform(Affine::translate((0.0, i as f64 * 16.0))),
                        ),
                    );
                }
                l.frames.insert_keyframe(5);
                l.frames.push_object(5, shape(3, KRect::new(40.0, 0.0, 52.0, 48.0), rgb));
            });
        });
        kinds.push(sym);
    }

    for row in 0..6usize {
        let layer = scene.add_layer(&format!("Cast{row}"), LayerKind::Normal);
        for i in 0..3 {
            scene.add_instance_at(
                layer,
                0,
                kinds[(row + i) % 2],
                Affine::translate((
                    10.0 + i as f64 * 120.0 + row as f64 * 7.0,
                    10.0 + row as f64 * 45.0,
                )),
            );
        }
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(25);
        });
    }
    let sky = scene.add_layer("Sky", LayerKind::Normal);
    scene.update_layer(sky, |l| {
        l.frames.push_object(0, shape(900, KRect::new(0.0, 0.0, 400.0, 20.0), (120, 180, 240)));
        l.frames.insert_frame(25);
    });

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(400.0, 300.0);
    editor.camera.center = Point::new(200.0, 150.0);
    editor.camera.zoom = 1.0;
    editor
}

fn area() -> Rect {
    Rect::new(0.0, 0.0, 400.0, 300.0)
}

fn epoch(revision: u64) -> Epoch {
    Epoch {
        revision,
        camera: [0; 4],
        area: [0; 4],
        scale_bits: 1.0f64.to_bits(),
        detail: LightDetail::Full,
        lights_generation: 0,
        buffer_scale: 1,
    }
}

fn inputs_at(editor: &mut Editor, frame: u32) -> StageInputs {
    editor.current_frame = frame;
    StageInputs::from_editor(editor, area(), 1.0)
}

fn jobs() -> Arc<JobSystem> {
    Arc::new(JobSystem::with_sizes(SEEK_WORKERS, 1))
}

/// Poll the pool until every job it has in flight has reported.
fn settle(seek: &mut SeekAhead, main: &mut DrawCache, inputs: &StageInputs, rev: u64) {
    for _ in 0..600 {
        seek.before_encode(epoch(rev), main, inputs, false);
        if seek.in_flight() == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the pool did not settle");
}

// -- item 0: a stamp built under abort is never kept -------------------------

#[test]
fn a_stamp_built_under_abort_is_never_kept_and_the_next_frames_are_whole() {
    let mut editor = cast_editor();
    let mut tripped_somewhere = false;
    for n in 0..=24usize {
        let flag = Arc::new(AtomicBool::new(false));
        let mut cache = DrawCache::new();
        cache.set_abort(Arc::clone(&flag));
        cache.trip_abort_after_symbol_layers(n);

        // Frame 0 on a cold cache: the abort is raised in the middle of it.
        let inputs = inputs_at(&mut editor, 0);
        let mut partial = vello::Scene::new();
        stage::build_plain_scene(&mut partial, &inputs, &mut cache);
        if !cache.is_aborted() {
            continue;
        }
        tripped_somewhere = true;
        flag.store(false, Ordering::Relaxed);

        // The frames after it carry every symbol complete: as many path
        // segments as the cache-off live frame.
        for frame in 1..=8u32 {
            let inputs = inputs_at(&mut editor, frame);
            let mut after = vello::Scene::new();
            stage::build_plain_scene(&mut after, &inputs, &mut cache);
            let mut live_cache = DrawCache::new();
            live_cache.set_symbol_reuse(false);
            let mut live = vello::Scene::new();
            stage::build_plain_scene(&mut live, &inputs, &mut live_cache);
            assert_eq!(
                after.encoding().n_path_segments,
                live.encoding().n_path_segments,
                "trip after {n} symbol layers: frame {frame} draws a truncated symbol"
            );
        }
    }
    assert!(tripped_somewhere, "the abort must have been raised mid-build in some trial");
}

#[test]
fn an_abort_in_the_first_stamp_build_leaves_no_stamp() {
    let mut editor = cast_editor();
    let mut proved = 0;
    for n in 0..=24usize {
        let flag = Arc::new(AtomicBool::new(false));
        let mut cache = DrawCache::new();
        cache.set_abort(Arc::clone(&flag));
        cache.trip_abort_after_symbol_layers(n);
        let inputs = inputs_at(&mut editor, 0);
        let mut partial = vello::Scene::new();
        stage::build_plain_scene(&mut partial, &inputs, &mut cache);
        if !cache.is_aborted() {
            continue;
        }
        let (entries, bytes, _, _, _) = cache.symbol_scenes.retention_stats();
        // Whatever was built before the trip is whole; a build cut by it is
        // neither held nor counted.
        assert_eq!(entries as u64, cache.symbol_scenes.builds, "trip {n}: held == built");
        if entries == 0 {
            assert_eq!(bytes, 0);
            proved += 1;
        }
    }
    assert!(proved >= 1, "at least one trip landed inside the first build");
}

// -- E7: fan-out ---------------------------------------------------------------

#[test]
fn a_jump_fans_the_target_frame_across_workers_and_the_encode_is_warm() {
    let mut editor = cast_editor();
    let mut main = DrawCache::new();
    let mut seek = SeekAhead::new(jobs());
    let inputs = inputs_at(&mut editor, 7);

    let installed = seek.before_encode(epoch(1), &mut main, &inputs, true);
    assert!(installed >= 2, "the characters and their parts were built on workers");
    assert_eq!(seek.stats.fanouts, 1);
    assert!(seek.stats.fan_jobs >= 2 && seek.stats.fan_jobs <= SEEK_WORKERS as u64);
    assert_eq!(main.symbol_scenes.builds, 0, "the render thread built nothing yet");

    let mut scene = vello::Scene::new();
    stage::build_plain_scene(&mut scene, &inputs, &mut main);
    assert_eq!(main.symbol_scenes.builds, 0, "the warm encode builds no stamp");
    assert_eq!(main.symbol_scenes.first_sightings, 0, "and draws nothing live as a first sighting");
    assert!(main.symbol_scenes.stamps > 0);
}

#[test]
fn no_jump_means_no_fan_out() {
    let mut editor = cast_editor();
    let mut main = DrawCache::new();
    let mut seek = SeekAhead::new(jobs());
    let inputs = inputs_at(&mut editor, 7);
    assert_eq!(seek.before_encode(epoch(1), &mut main, &inputs, false), 0);
    assert_eq!(seek.stats.fanouts, 0);
}

// -- E7: scrubbing ---------------------------------------------------------------

fn encode_home(
    seek: &mut SeekAhead,
    main: &mut DrawCache,
    editor: &mut Editor,
    frame: u32,
    rev: u64,
) {
    let inputs = inputs_at(editor, frame);
    seek.before_encode(epoch(rev), main, &inputs, false);
    let before = main.symbol_scenes.builds + main.symbol_scenes.first_sightings;
    let mut scene = vello::Scene::new();
    stage::build_plain_scene(&mut scene, &inputs, main);
    let missed = main.symbol_scenes.builds + main.symbol_scenes.first_sightings > before;
    seek.after_encode(main, &inputs, editor.playing_length(), missed);
}

#[test]
fn scrubbing_forward_prebuilds_the_frames_ahead() {
    let mut editor = cast_editor();
    let mut main = DrawCache::new();
    let mut seek = SeekAhead::new(jobs());
    encode_home(&mut seek, &mut main, &mut editor, 1, 1);
    assert_eq!(seek.stats.scrub_jobs, 0, "one frame is not a direction yet");
    encode_home(&mut seek, &mut main, &mut editor, 2, 1);
    assert!(seek.stats.scrub_jobs >= 1, "the frames after a forward scrub are started");

    let inputs = inputs_at(&mut editor, 3);
    settle(&mut seek, &mut main, &inputs, 1);
    assert!(seek.prebuild_stats().installed > 0, "their stamps were installed");
    // Frame 3 lies inside the horizon and was built ahead: nothing new.
    let builds = main.symbol_scenes.builds;
    let mut scene = vello::Scene::new();
    stage::build_plain_scene(&mut scene, &inputs, &mut main);
    assert_eq!(main.symbol_scenes.builds, builds, "frame 3 builds no stamp of its own");
}

#[test]
fn scrubbing_backward_prebuilds_the_frames_behind() {
    let mut editor = cast_editor();
    let mut main = DrawCache::new();
    let mut seek = SeekAhead::new(jobs());
    encode_home(&mut seek, &mut main, &mut editor, 12, 1);
    encode_home(&mut seek, &mut main, &mut editor, 11, 1);
    assert!(seek.stats.scrub_jobs >= 1);
    let inputs = inputs_at(&mut editor, 10);
    settle(&mut seek, &mut main, &inputs, 1);
    assert!(seek.prebuild_stats().installed > 0);
}

#[test]
fn an_epoch_change_never_installs_what_the_old_epoch_built() {
    let mut editor = cast_editor();
    let mut main = DrawCache::new();
    let mut seek = SeekAhead::new(jobs());
    encode_home(&mut seek, &mut main, &mut editor, 1, 1);
    encode_home(&mut seek, &mut main, &mut editor, 2, 1);
    assert!(seek.stats.scrub_jobs >= 1);
    let installed_before = seek.prebuild_stats().installed;
    // An edit (a new revision) arrives before the jobs are collected.
    let inputs = inputs_at(&mut editor, 2);
    settle(&mut seek, &mut main, &inputs, 2);
    assert_eq!(
        seek.prebuild_stats().installed,
        installed_before,
        "results of epoch 1 are dropped, not installed"
    );
}

// -- pixels --------------------------------------------------------------------

/// A multiple of 64 so a row of RGBA8 meets wgpu's copy alignment; the stage is 400 wide.
const W: u32 = 512;
const H: u32 = 300;

fn render_rgba(gpu: &mut GpuContext, scene: &vello::Scene) -> Vec<u8> {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("prebuild-target"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
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
        label: Some("prebuild-readback"),
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    gpu.render(scene, &view, W, H, Color::from_rgb8(0x20, 0x20, 0x20)).expect("vello render");
    let mut encoder =
        gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(W * 4),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .expect("poll device");
    let pixels = slice.get_mapped_range().to_vec();
    readback.unmap();
    pixels
}

/// Every pixel within 2/255, and at most 0.1% of pixels needing even that:
/// the tolerance `buzz-render`'s stamp-vs-live comparisons use, unchanged.
fn assert_pixels_match(live: &[u8], other: &[u8], label: &str) {
    assert_eq!(live.len(), other.len(), "{label}: readback size mismatch");
    let count = (W * H) as usize;
    let (mut off, mut max_diff) = (0usize, 0i32);
    for px in 0..count {
        let base = px * 4;
        let diff = (0..4)
            .map(|c| (live[base + c] as i32 - other[base + c] as i32).abs())
            .max()
            .unwrap_or(0);
        max_diff = max_diff.max(diff);
        if diff > 2 {
            off += 1;
        }
    }
    let allowed = ((count as f64) * 0.001).ceil() as usize;
    assert!(
        off <= allowed,
        "{label}: {off}/{count} pixels differ by more than 2/255 (max {max_diff}/255), allowed {allowed}"
    );
}


#[test]
fn a_jump_encoded_after_fan_out_installs_matches_a_cache_off_live_render_at_1x() {
    let mut gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping seek pixel test: no usable GPU ({e})");
            return;
        }
    };
    let mut editor = cast_editor();
    for frame in [1u32, 7, 13] {
        let mut main = DrawCache::new();
        let mut seek = SeekAhead::new(jobs());
        let inputs = inputs_at(&mut editor, frame);
        assert!(seek.before_encode(epoch(1), &mut main, &inputs, true) >= 2);
        let mut after_installs = vello::Scene::new();
        stage::build_plain_scene(&mut after_installs, &inputs, &mut main);
        assert_eq!(main.symbol_scenes.builds, 0, "frame {frame}: every stamp was installed");

        let mut live_cache = DrawCache::new();
        live_cache.set_symbol_reuse(false);
        let mut live = vello::Scene::new();
        stage::build_plain_scene(&mut live, &inputs, &mut live_cache);

        let live_px = render_rgba(&mut gpu, &live);
        let installed_px = render_rgba(&mut gpu, &after_installs);
        assert_pixels_match(&live_px, &installed_px, &format!("frame {frame} after fan-out"));
    }
}

#[test]
fn a_frame_after_an_aborted_stamp_build_matches_a_cache_off_live_render_at_1x() {
    let mut gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping abort pixel test: no usable GPU ({e})");
            return;
        }
    };
    let mut editor = cast_editor();
    let mut tripped = 0;
    for n in 0..=5usize {
        let flag = Arc::new(AtomicBool::new(false));
        let mut cache = DrawCache::new();
        cache.set_abort(Arc::clone(&flag));
        cache.trip_abort_after_symbol_layers(n);
        let inputs = inputs_at(&mut editor, 0);
        let mut partial = vello::Scene::new();
        stage::build_plain_scene(&mut partial, &inputs, &mut cache);
        assert!(cache.is_aborted(), "trip {n} lands in frame 0");
        tripped += 1;
        flag.store(false, Ordering::Relaxed);
        for frame in 1..=4u32 {
            let inputs = inputs_at(&mut editor, frame);
            let mut after = vello::Scene::new();
            stage::build_plain_scene(&mut after, &inputs, &mut cache);
            let mut live_cache = DrawCache::new();
            live_cache.set_symbol_reuse(false);
            let mut live = vello::Scene::new();
            stage::build_plain_scene(&mut live, &inputs, &mut live_cache);
            assert_pixels_match(
                &render_rgba(&mut gpu, &live),
                &render_rgba(&mut gpu, &after),
                &format!("trip {n}, frame {frame}"),
            );
        }
    }
    assert_eq!(tripped, 6);
}
