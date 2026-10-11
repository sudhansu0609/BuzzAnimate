//! S9 of `docs/BIMARI5_LAG_PLAN.md`: the warm-up at document open. Every frame
//! is encoded in playback order on the worker pool and every stamp it needs is
//! kept, so playing afterwards builds nothing; the budget stops it (never an
//! eviction), a cancel returns nothing, and the buffers are sized once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use buzz_app::editor::Editor;
use buzz_app::stage::{self, StageInputs};
use buzz_app::warm_open::{self, WarmJob, WarmOutput, WarmReport};
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Size};
use buzz_jobs::JobSystem;
use buzz_render::document::{DrawCache, LightDetail};
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

/// Characters made of an animated `part` symbol, itself animated: nested
/// animated symbols, the shape a real film is made of. `frames` long.
fn nested_animated_editor(frames: u32) -> Editor {
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
        l.frames.insert_frame(frames - 1);
    });
    // A parade: a different symbol every 8 frames, so later frames keep needing
    // stamps nobody has built yet (the budget test's stamp growth).
    let parade = scene.add_layer("Parade", LayerKind::Normal);
    for k in 0..(frames / 8) {
        let sym = scene.add_symbol(&format!("p{k}"), SymbolKind::Graphic, None);
        scene.library_mut().update(sym, |s| {
            let inner = s.layers.iter().next().map(|l| l.id).expect("a layer");
            s.layers.update(inner, |l| {
                l.frames.push_object(
                    0,
                    shape(
                        1000 + u64::from(k),
                        KRect::new(0.0, 0.0, 10.0 + f64::from(k), 14.0),
                        (30 + (k % 200) as u8, 90, 160),
                    ),
                );
            });
        });
        let at = k * 8;
        scene.update_layer(parade, |l| {
            if at > 0 {
                l.frames.insert_keyframe(at);
            }
        });
        scene.add_instance_at(parade, at, sym, Affine::translate((300.0, 200.0)));
    }
    scene.update_layer(parade, |l| {
        l.frames.insert_frame(frames - 1);
    });
    let sky = scene.add_layer("Sky", LayerKind::Normal);
    scene.update_layer(sky, |l| {
        l.frames.push_object(0, shape(900, KRect::new(0.0, 0.0, 400.0, 20.0), (120, 180, 240)));
        l.frames.insert_frame(frames - 1);
    });

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(400.0, 300.0);
    editor.camera.center = Point::new(200.0, 150.0);
    editor.camera.zoom = 1.0;
    editor
}

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

fn jobs() -> Arc<JobSystem> {
    Arc::new(JobSystem::with_sizes(4, 1))
}

fn job_for(editor: &mut Editor, budget: usize, workers: usize) -> WarmJob {
    let count = editor.playing_length();
    let frames = warm_open::frames_from(0, count);
    editor.current_frame = frames[0];
    let cache = DrawCache::new();
    WarmJob {
        inputs: StageInputs::from_editor(editor, area(), 1.0),
        frames,
        budget_bytes: budget,
        limit_bytes: budget.saturating_mul(2),
        workers,
        detail: LightDetail::Full,
        symbols: cache.symbols.clone(),
    }
}

fn run(editor: &mut Editor, budget: usize, workers: usize) -> WarmOutput {
    warm_open::run_warm(&jobs(), job_for(editor, budget, workers), &|| false, &|_, _| {})
}

/// Encode every frame in order on `cache`, answering the bytes of each.
fn play_all(editor: &mut Editor, cache: &mut DrawCache) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for f in 0..editor.playing_length() {
        editor.current_frame = f;
        let inputs = StageInputs::from_editor(editor, area(), 1.0);
        let mut scene = vello::Scene::new();
        stage::build_plain_scene(&mut scene, &inputs, cache);
        out.push(encoding_bytes(&scene));
    }
    out
}

#[test]
fn after_a_warm_up_playing_the_film_builds_nothing_and_draws_nothing_live_first() {
    let mut editor = nested_animated_editor(40);
    let out = run(&mut editor, 1 << 30, 4);
    let total = out.report.frames_total;
    assert_eq!(total, 40);
    assert_eq!(out.report.frames_covered, total, "the whole film fit the budget");
    assert!(!out.report.stopped_by_budget && !out.report.cancelled);
    assert!(out.report.stamp_keys >= 4, "stamps were kept: {}", out.report.stamp_keys);
    assert!(out.report.peak_lines > 0 && out.report.peak_segments > 0);

    let mut cache = DrawCache::new();
    let report = warm_open::apply(&mut cache, out, 400.0 * 300.0);
    assert_eq!(cache.stamp_count(), report.stamp_keys);
    assert_eq!(cache.stamp_bytes(), report.stamp_bytes, "every kept stamp is counted");

    let (builds, first) = (cache.symbol_scenes.builds, cache.symbol_scenes.first_sightings);
    play_all(&mut editor, &mut cache);
    assert_eq!(cache.symbol_scenes.builds, builds, "no stamp is built while playing");
    assert_eq!(
        cache.symbol_scenes.first_sightings, first,
        "nothing is drawn live as a first sighting"
    );
    assert_eq!(cache.stamp_count(), report.stamp_keys, "nothing was evicted");
}

#[test]
fn warmed_frames_are_byte_identical_to_building_on_demand() {
    let mut editor = nested_animated_editor(40);
    let out = run(&mut editor, 1 << 30, 4);
    let mut warmed = DrawCache::new();
    warm_open::apply(&mut warmed, out, 400.0 * 300.0);
    let from_warm = play_all(&mut editor, &mut warmed);

    // On demand, one sighting is enough to build (the warm-up's own setting),
    // frames in the same order, one thread.
    let mut on_demand = DrawCache::new();
    on_demand.set_build_on_first_sighting(true);
    let sequential = play_all(&mut editor, &mut on_demand);

    assert_eq!(from_warm.len(), sequential.len());
    for (f, (a, b)) in from_warm.iter().zip(&sequential).enumerate() {
        assert!(a == b, "frame {f}: the warmed encoding differs from the one built on demand");
    }
}

#[test]
fn the_budget_stops_the_warm_up_without_evicting_and_the_covered_frames_are_a_prefix() {
    let mut editor = nested_animated_editor(120);
    let whole = run(&mut editor, 1 << 30, 1);
    assert!(whole.report.stamp_bytes > 0);
    let curve = whole.report.curve.clone();
    assert!(!curve.is_empty() && curve.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 <= w[1].1));
    // What the first two thirds of the film need: with this much room the rest
    // cannot be warmed.
    let budget = curve[curve.len() * 2 / 3].1;
    assert!(budget < whole.report.stamp_bytes);

    let out = run(&mut editor, budget, 4);
    let r: &WarmReport = &out.report;
    assert!(r.stopped_by_budget, "the budget was reached");
    assert!(r.stamp_bytes <= budget, "never over budget: {} > {budget}", r.stamp_bytes);
    assert!(r.frames_covered < r.frames_total, "the rest is left to build as it plays");
    assert!(r.frames_covered > 0, "the first frames are warm");
    let covered = r.frames_covered;
    let kept = r.stamp_keys;

    // Installed with room to spare: nothing is evicted by the frames it covers.
    let mut cache = DrawCache::new();
    let report = warm_open::apply(&mut cache, out, 400.0 * 300.0);
    assert_eq!(cache.stamp_count(), kept);
    let before = cache.symbol_scenes.builds;
    for f in 0..covered {
        editor.current_frame = f;
        let inputs = StageInputs::from_editor(&editor, area(), 1.0);
        let mut scene = vello::Scene::new();
        stage::build_plain_scene(&mut scene, &inputs, &mut cache);
    }
    assert_eq!(cache.symbol_scenes.builds, before, "the covered prefix is all hits");
    assert!(report.stamp_bytes <= budget);
    assert_eq!(cache.stamp_count(), kept, "and nothing was evicted");
}

#[test]
fn a_cancelled_warm_up_returns_nothing() {
    let mut editor = nested_animated_editor(200);
    let stop = AtomicBool::new(false);
    let seen = std::sync::atomic::AtomicU64::new(0);
    let out = warm_open::run_warm(
        &jobs(),
        job_for(&mut editor, 1 << 30, 2),
        &|| stop.load(Ordering::Relaxed),
        &|done, _| {
            seen.store(done, Ordering::Relaxed);
            stop.store(true, Ordering::Relaxed);
        },
    );
    assert!(seen.load(Ordering::Relaxed) > 0, "progress was reported");
    assert!(out.report.cancelled);
    assert!(out.stamps.is_empty(), "a cancelled run installs nothing");
    assert!(out.report.frames_covered < 200);
}

#[test]
fn applying_sizes_the_buffers_once_to_the_heaviest_frame() {
    let pixels = 1600.0 * 900.0;
    let mut cache = DrawCache::new();
    cache.set_max_scale(8);
    let report = WarmReport {
        peak_lines: 7_700_000,
        peak_segments: 1_000_000,
        ..WarmReport::default()
    };
    let out = WarmOutput { stamps: Vec::new(), report };
    let report = warm_open::apply(&mut cache, out, pixels);
    let expected = buzz_render::document::fitting_scale(7_700_000, 1_000_000, pixels, 8);
    assert!(expected >= 4, "7.7 M lines do not fit a small buffer: {expected}");
    assert_eq!(cache.scale(), expected, "grown straight to the fitting scale");
    assert_eq!(report.fit_scale, expected);

    // A light film leaves the scale alone; a heavier one later is not shrunk.
    let mut small = DrawCache::new();
    small.set_max_scale(8);
    let out = WarmOutput {
        stamps: Vec::new(),
        report: WarmReport { peak_lines: 10, peak_segments: 10, ..WarmReport::default() },
    };
    warm_open::apply(&mut small, out, pixels);
    assert_eq!(small.scale(), 1);
}

#[test]
fn the_budget_default_is_the_override_or_a_quarter_of_ram() {
    // The override is read from the environment each call.
    let ram = warm_open::physical_ram_bytes();
    assert!(ram >= 1 << 30, "physical RAM read: {ram}");
    if std::env::var_os("BUZZ_STAMP_CACHE_MB").is_none() {
        let b = warm_open::default_budget_bytes() as u64;
        assert!(b >= 512 << 20);
        assert!(b <= (ram / 4).max(512 << 20));
    }
}

/// S9: past the warmed frames the pre-build keeps going on a warm-up's cache
/// (it makes room by letting the left-behind stamps go), while a plain cache
/// that is full still takes no more (the E6 rule).
#[test]
fn the_prebuild_keeps_going_on_a_full_window_cache_and_stops_on_a_full_plain_one() {
    use buzz_app::encode_ahead::{Prebuild, PlayRange};
    let mut editor = nested_animated_editor(40);
    let play = PlayRange { count: editor.playing_length(), looping: true };
    let inputs = {
        editor.current_frame = 0;
        StageInputs::from_editor(&editor, area(), 1.0)
    };
    for window in [false, true] {
        let out = run(&mut editor, 1 << 30, 2);
        let mut main = DrawCache::new();
        warm_open::apply(&mut main, out, 400.0 * 300.0);
        main.set_stamp_window(window);
        // Full: the budget is what it holds.
        let held = main.stamp_bytes();
        main.set_stamp_limits(held, usize::MAX);
        let mut pb = Prebuild::new(jobs());
        pb.reset(Some(&main.symbols));
        pb.note_pass(true);
        pb.tick(&mut main, &inputs, play, 0);
        let started = pb.stats().dispatched;
        pb.wait_idle(&mut main, std::time::Duration::from_secs(30));
        if window {
            assert!(started > 0, "a window cache makes room and builds ahead");
            assert!(main.stamp_bytes() <= held * 9 / 10 + 1, "room was made: {}", main.stamp_bytes());
        } else {
            assert_eq!(started, 0, "a full plain cache takes no more");
            assert_eq!(main.stamp_bytes(), held);
        }
    }
}
