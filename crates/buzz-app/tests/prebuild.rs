//! E6 of the revised S5 in `docs/BIMARI5_LAG_PLAN.md`: the stamp pre-build
//! pool. Workers build the stamps of frames ahead of the encode thread on their
//! own caches; the encode thread installs them, and a key built ahead is a hit
//! when its frame arrives, with exactly the bytes a local build gives.

use std::sync::Arc;
use std::time::Duration;

use buzz_app::editor::Editor;
use buzz_app::encode_ahead::{
    Epoch, PREBUILD_HORIZON, PREBUILD_SLOTS, Pipeline, PlayRange, Prebuild, Stopped, Taken,
};
use buzz_app::stage::{self, StageInputs};
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Size};
use buzz_jobs::JobSystem;
use buzz_render::document::{DrawCache, LightDetail};
use buzz_render::{GpuContext, GpuPreference, wgpu};
use buzz_scene::{LayerKind, Object, ObjectId, Scene, ShapeData, SymbolKind};
use kurbo::Rect as KRect;
use peniko::Color;

const WAIT: Duration = Duration::from_secs(60);

fn shape(id: u64, rect: KRect, rgb: (u8, u8, u8)) -> Arc<Object> {
    Arc::new(Object::shape(
        ObjectId(id),
        ShapeData::filled(
            buzz_geom::Shape::to_path(&rect, 1e-9),
            Color::from_rgb8(rgb.0, rgb.1, rgb.2),
        ),
    ))
}

/// Characters made of an animated `part` symbol, itself animated: nested
/// animated symbols, the shape a real film is made of.
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

fn area() -> Rect {
    Rect::new(0.0, 0.0, 400.0, 300.0)
}

fn epoch() -> Epoch {
    Epoch {
        revision: 1,
        camera: [0; 4],
        area: [0; 4],
        scale_bits: 1.0f64.to_bits(),
        detail: LightDetail::Full,
        lights_generation: 0,
        buffer_scale: 1,
    }
}

fn play_range(editor: &Editor) -> PlayRange {
    PlayRange { count: editor.playing_length(), looping: editor.playback.looping }
}

fn inputs_at(editor: &mut Editor, frame: u32) -> StageInputs {
    editor.current_frame = frame;
    StageInputs::from_editor(editor, area(), 1.0)
}

fn jobs() -> Arc<JobSystem> {
    Arc::new(JobSystem::with_sizes(PREBUILD_SLOTS, 1))
}

/// A cache primed like a pre-build slot: builds on first sighting and records
/// what it built.
fn collecting() -> DrawCache {
    let mut cache = DrawCache::new();
    cache.set_build_on_first_sighting(true);
    cache.set_collect_new_stamps(true);
    cache
}

/// Pre-build `frame` on the pool and install the result into `main`.
fn prebuild_into(pb: &mut Prebuild, main: &mut DrawCache, editor: &mut Editor, frame: u32) -> usize {
    let inputs = inputs_at(editor, frame);
    assert!(pb.dispatch(inputs, main.detail()), "a slot is free");
    pb.wait_idle(main, WAIT)
}

#[test]
fn worker_installed_keys_are_hits_on_the_next_frame() {
    let mut editor = nested_animated_editor();
    let mut main = DrawCache::new();
    let mut pb = Prebuild::new(jobs());
    pb.reset(Some(&main.symbols));

    let installed = prebuild_into(&mut pb, &mut main, &mut editor, 1);
    assert!(installed >= 2, "the character and its nested part were built ahead");
    assert_eq!(main.symbol_scenes.builds, 0, "the main cache built nothing itself");
    assert_eq!(pb.stats().installed, installed as u64);

    // The frame the stamps were built for: every one is a hit.
    let inputs = inputs_at(&mut editor, 1);
    let mut scene = vello::Scene::new();
    stage::build_plain_scene(&mut scene, &inputs, &mut main);
    assert_eq!(main.symbol_scenes.builds, 0, "no build is counted on the frame after installs");
    assert_eq!(main.symbol_scenes.first_sightings, 0, "and nothing is drawn live as a first sighting");
    assert!(main.symbol_scenes.stamps > 0, "the installed stamps were used");
}

#[test]
fn installed_stamp_bytes_equal_the_stamp_built_locally() {
    let mut editor = nested_animated_editor();
    let mut main = DrawCache::new();
    let mut pb = Prebuild::new(jobs());
    pb.reset(Some(&main.symbols));
    let installed = prebuild_into(&mut pb, &mut main, &mut editor, 1);
    assert!(installed >= 2);

    let mut local = collecting();
    let inputs = inputs_at(&mut editor, 1);
    let mut scene = vello::Scene::new();
    stage::build_plain_scene(&mut scene, &inputs, &mut local);
    let built = local.drain_new_stamps();
    assert_eq!(built.len(), installed, "the same keys are built either way");
    for stamp in &built {
        let theirs = main.stamp_scene(&stamp.key).expect("the key was installed");
        assert!(
            encoding_bytes(&theirs) == encoding_bytes(&stamp.scene),
            "{:?}: an installed stamp differs from the local build",
            stamp.key
        );
    }
}

#[test]
fn results_from_an_old_epoch_are_never_installed() {
    let mut editor = nested_animated_editor();
    let mut main = DrawCache::new();
    let mut pb = Prebuild::new(jobs());
    pb.reset(Some(&main.symbols));
    for frame in 1..=3 {
        let inputs = inputs_at(&mut editor, frame);
        assert!(pb.dispatch(inputs, main.detail()));
    }
    // The epoch ends (an edit, a seek, a stop) with the jobs in flight.
    pb.reset(Some(&main.symbols));
    let installed = pb.wait_idle(&mut main, WAIT);
    assert_eq!(installed, 0, "nothing from the old epoch is installed");
    assert_eq!(pb.in_flight(), 0);
    assert_eq!(main.symbol_scenes.retention_stats().0, 0, "the cache is untouched");
    assert_eq!(pb.stats().installed, 0);
    assert_eq!(pb.stats().stale, 3, "all three results were dropped as stale");

    // The new epoch works as usual.
    let installed = prebuild_into(&mut pb, &mut main, &mut editor, 1);
    assert!(installed >= 2);
}

#[test]
fn ticks_keep_at_most_four_in_flight_and_stay_inside_the_horizon() {
    let mut editor = nested_animated_editor();
    let play = play_range(&editor);
    let mut main = DrawCache::new();
    let mut pb = Prebuild::new(jobs());
    pb.reset(Some(&main.symbols));
    let inputs = inputs_at(&mut editor, 0);

    pb.tick(&mut main, &inputs, play, 0);
    assert!(pb.in_flight() >= 1 && pb.in_flight() <= PREBUILD_SLOTS);
    assert!(pb.stats().dispatched as usize <= PREBUILD_SLOTS, "four at once, no more");

    // The head stays at frame 0: the pool may run up to the horizon, never past.
    for _ in 0..100 {
        pb.wait_idle(&mut main, WAIT);
        pb.note_pass(true);
        pb.tick(&mut main, &inputs, play, 0);
        assert!(pb.in_flight() <= PREBUILD_SLOTS);
    }
    pb.wait_idle(&mut main, WAIT);
    let dispatched = pb.stats().dispatched as usize;
    assert!(dispatched >= 1);
    assert!(
        dispatched <= PREBUILD_HORIZON,
        "{dispatched} jobs for a head that never moved: at most the horizon"
    );
}

#[test]
fn warm_frames_and_a_full_cache_start_nothing() {
    let mut editor = nested_animated_editor();
    let play = play_range(&editor);
    let mut main = DrawCache::new();
    let mut pb = Prebuild::new(jobs());
    pb.reset(Some(&main.symbols));

    // A slot builds frame 1, then builds it again: the second job has nothing
    // new, so what is ahead is warm.
    assert!(prebuild_into(&mut pb, &mut main, &mut editor, 1) >= 2);
    assert_eq!(prebuild_into(&mut pb, &mut main, &mut editor, 1), 0);
    pb.note_pass(false);
    assert!(!pb.is_hot());
    let before = pb.stats().dispatched;
    let inputs = inputs_at(&mut editor, 0);
    pb.tick(&mut main, &inputs, play, 0);
    assert_eq!(pb.stats().dispatched, before, "warm: nothing started");

    // A miss on the encode thread makes it hot again ...
    pb.note_pass(true);
    assert!(pb.is_hot());
    // ... but not when the main cache is over 90% of its budget.
    main.symbol_scenes.budget_bytes = main.symbol_scenes.bytes.max(1);
    pb.tick(&mut main, &inputs, play, 0);
    assert_eq!(pb.stats().dispatched, before, "over 90% of the budget: nothing started");
    main.symbol_scenes.budget_bytes = 512 << 20;
    pb.tick(&mut main, &inputs, play, 0);
    assert!(pb.stats().dispatched > before, "under the budget it starts again");
    pb.wait_idle(&mut main, WAIT);
}

#[test]
fn a_pipeline_with_a_job_system_prebuilds_and_still_delivers_every_frame() {
    let mut editor = nested_animated_editor();
    let play = play_range(&editor);
    assert!(play.count < 30, "the fixture must loop inside the 30 frames");

    let mut lent = DrawCache::new();
    editor.current_frame = 0;
    let mut first = vello::Scene::new();
    stage::build_scene(&mut first, &editor, area(), 1.0, &mut lent);
    let mut pipeline = Pipeline::with_jobs(jobs());
    let mut inputs = StageInputs::from_editor(&editor, area(), 1.0);
    inputs.frame = 1;
    pipeline.start(lent, epoch(), inputs, play, 1, 0);

    let mut frame = 1u32;
    for _ in 0..29 {
        pipeline.playhead(frame);
        match pipeline.take(frame, Duration::from_secs(10)) {
            Taken::Ready(p) => {
                assert_eq!(p.frame, frame);
                pipeline.recycle(p.scene);
            }
            Taken::Shown => panic!("frame {frame} was never taken, yet reads as shown"),
            Taken::NotReady => panic!("frame {frame}: not ready in 10 s"),
        }
        frame = (frame + 1) % play.count;
    }
    assert_eq!(pipeline.stats.not_ready, 0);
    assert!(pipeline.prebuild_stats().dispatched > 0, "the pool was used");
    let Stopped::Returned(home) = pipeline.stop() else {
        panic!("the cache must come home");
    };
    // Stopping ended the epoch: whatever was still running is dropped, never
    // installed into the cache that came home (or anywhere else).
    let at_stop = pipeline.prebuild_stats();
    let held = home.symbol_scenes.retention_stats();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(pipeline.prebuild_stats().installed, at_stop.installed);
    assert_eq!(home.symbol_scenes.retention_stats(), held);
}

// -- pixels --------------------------------------------------------------

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
fn a_frame_encoded_after_installs_matches_a_cache_off_live_render_at_1x() {
    let mut gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping prebuild pixel test: no usable GPU ({e})");
            return;
        }
    };
    let mut editor = nested_animated_editor();
    for frame in [1u32, 6, 12] {
        let mut main = DrawCache::new();
        let mut pb = Prebuild::new(jobs());
        pb.reset(Some(&main.symbols));
        assert!(prebuild_into(&mut pb, &mut main, &mut editor, frame) >= 2);

        let inputs = inputs_at(&mut editor, frame);
        let mut after_installs = vello::Scene::new();
        stage::build_plain_scene(&mut after_installs, &inputs, &mut main);
        assert_eq!(main.symbol_scenes.builds, 0, "frame {frame}: every stamp was installed");

        let mut live_cache = DrawCache::new();
        live_cache.set_symbol_reuse(false);
        let mut live = vello::Scene::new();
        stage::build_plain_scene(&mut live, &inputs, &mut live_cache);

        let live_px = render_rgba(&mut gpu, &live);
        let installed_px = render_rgba(&mut gpu, &after_installs);
        assert_pixels_match(&live_px, &installed_px, &format!("frame {frame} after installs"));
    }
}

/// S7 (stop latency): a stop with the pre-build pool populated and running
/// returns the cache within 10 ms (the idle slots are freed on the background
/// pool, not by the thread the render thread waits for), and the frame drawn on
/// the cache that came home is whole: as many path segments as the same frame
/// drawn live with the symbol cache off (stamped and live encodings differ in
/// bytes, never in what they draw).
#[test]
fn a_stop_with_the_prebuild_running_is_prompt_and_the_next_frame_whole() {
    let mut editor = nested_animated_editor();
    let play = play_range(&editor);

    let jobs = jobs();
    let mut worst = 0.0f64;
    let mut installed = 0u64;
    for trial in 0..10u32 {
        let mut lent = DrawCache::new();
        editor.current_frame = 0;
        let mut first = vello::Scene::new();
        stage::build_scene(&mut first, &editor, area(), 1.0, &mut lent);
        let mut pipeline = Pipeline::with_jobs(Arc::clone(&jobs));
        let mut inputs = StageInputs::from_editor(&editor, area(), 1.0);
        inputs.frame = 1;
        pipeline.start(lent, epoch(), inputs, play, 1, 0);
        let taken = 3 + trial % 5;
        let mut f = 1u32;
        for _ in 0..taken {
            pipeline.playhead(f);
            match pipeline.take(f, Duration::from_secs(10)) {
                Taken::Ready(p) => pipeline.recycle(p.scene),
                _ => panic!("frame {f} not ready"),
            }
            f = (f + 1) % play.count;
        }
        std::thread::sleep(Duration::from_micros(u64::from(trial) * 400));
        let Stopped::Returned(mut home) = pipeline.stop() else {
            panic!("the cache must come home");
        };
        worst = worst.max(pipeline.stats.last_stop_ms);
        installed += pipeline.prebuild_stats().installed;

        // The frame after the last one taken, drawn on the cache that came home,
        // against the same frame drawn live.
        editor.current_frame = f;
        let mut after = vello::Scene::new();
        stage::build_scene(&mut after, &editor, area(), 1.0, &mut home);
        let mut live_cache = DrawCache::new();
        live_cache.set_symbol_reuse(false);
        let mut live = vello::Scene::new();
        stage::build_scene(&mut live, &editor, area(), 1.0, &mut live_cache);
        assert_eq!(
            after.encoding().n_path_segments,
            live.encoding().n_path_segments,
            "trial {trial}: frame {f} after a stop is not whole"
        );
    }
    assert!(installed > 0, "the pre-build pool must have been in play");
    assert!(worst < 10.0, "a stop took {worst:.2} ms; the cache must be back within 10 ms");
}
