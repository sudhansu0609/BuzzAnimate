//! E4 and E5 of the revised S5 in `docs/BIMARI5_LAG_PLAN.md`: the encode
//! thread draws exactly the frames the render thread would, hands its cache
//! back promptly, and an aborted pass leaves the cache sound.
//!
//! Headless: no window, no GPU. The `App`-level behaviour (an edit during play,
//! the stand-in cache) is tested in `app.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use buzz_app::editor::Editor;
use buzz_app::encode_ahead::{Epoch, Pipeline, PlayRange, Stopped, Taken};
use buzz_app::stage::{self, StageInputs};
use buzz_doc::Document;
use buzz_geom::{Affine, Point, Rect, Size};
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

/// A stage of characters, each made of an animated `part` symbol, itself
/// animated: nested animated symbols, the shape a real film is made of. A
/// second layer gives an aborted pass something to stop between.
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

/// Encode `frame` of `editor` into a fresh scene on `cache`, the way a render
/// does for a plain stage.
fn build(editor: &mut Editor, frame: u32, cache: &mut DrawCache) -> vello::Scene {
    editor.current_frame = frame;
    let mut scene = vello::Scene::new();
    stage::build_scene(&mut scene, editor, area(), 1.0, cache);
    scene
}

fn play_range(editor: &Editor) -> PlayRange {
    PlayRange { count: editor.playing_length(), looping: editor.playback.looping }
}

#[test]
fn thirty_played_frames_are_byte_identical_to_sequential_build_scene() {
    let mut editor = nested_animated_editor();
    let play = play_range(&editor);
    assert!(play.count < 30, "the fixture must loop inside the 30 frames");

    // The reference: one cache walked through frame 0 and then the 29 frames
    // after it, in playback order, one `build_scene` each.
    let mut sequential = DrawCache::new();
    let mut want = Vec::new();
    let mut frame = 0u32;
    for _ in 0..30 {
        want.push((frame, encoding_bytes(&build(&mut editor, frame, &mut sequential))));
        frame = (frame + 1) % play.count;
    }
    let distinct: std::collections::HashSet<_> = want.iter().map(|(_, b)| b.clone()).collect();
    assert!(distinct.len() > 3, "the animation must change the encoding");

    // The pipeline: the same start (frame 0 drawn here), then the thread draws
    // frames 1..29 from the lent cache while this thread only takes them.
    let mut lent = DrawCache::new();
    let first = build(&mut editor, 0, &mut lent);
    assert!(encoding_bytes(&first) == want[0].1, "frame 0 is the shared start");
    let mut pipeline = Pipeline::new();
    let mut inputs = StageInputs::from_editor(&editor, area(), 1.0);
    inputs.frame = 1;
    pipeline.start(lent, epoch(), inputs, play, 1, 0);
    assert!(pipeline.is_running());

    for (frame, bytes) in want.iter().skip(1) {
        pipeline.playhead(*frame);
        match pipeline.take(*frame, Duration::from_secs(10)) {
            Taken::Ready(p) => {
                assert_eq!(p.frame, *frame);
                assert!(p.epoch == epoch(), "frame {frame}: wrong epoch");
                assert!(
                    encoding_bytes(&p.scene) == *bytes,
                    "frame {frame}: differs from the sequential build_scene"
                );
                pipeline.recycle(p.scene);
            }
            Taken::Shown => panic!("frame {frame} was never taken, yet reads as shown"),
            Taken::NotReady => panic!("frame {frame}: not ready in 10 s"),
        }
    }
    assert_eq!(pipeline.stats.not_ready, 0);
    assert!(matches!(pipeline.stop(), Stopped::Returned(_)));
}

#[test]
fn the_cache_is_returned_within_10_ms_of_stop() {
    let mut editor = nested_animated_editor();
    let play = play_range(&editor);
    let mut times = Vec::new();
    for trial in 0..15u32 {
        let mut lent = DrawCache::new();
        build(&mut editor, 0, &mut lent);
        let mut pipeline = Pipeline::new();
        let mut inputs = StageInputs::from_editor(&editor, area(), 1.0);
        inputs.frame = 1;
        pipeline.start(lent, epoch(), inputs, play, 1, 0);
        // Take a few frames, then stop at a different moment each trial: the
        // thread is encoding, delivering or idle depending on the trial.
        for f in 1..=(trial % 4) {
            pipeline.playhead(f);
            if let Taken::Ready(p) = pipeline.take(f, Duration::from_secs(10)) {
                pipeline.recycle(p.scene);
            }
        }
        std::thread::sleep(Duration::from_micros(u64::from(trial) * 300));
        let started = Instant::now();
        let stopped = pipeline.stop();
        let ms = started.elapsed().as_secs_f64() * 1e3;
        assert!(matches!(stopped, Stopped::Returned(_)), "the cache must come back");
        assert!(!pipeline.is_running());
        times.push(ms);
    }
    times.sort_by(f64::total_cmp);
    assert!(
        times[times.len() - 1] < 10.0,
        "stop took {times:?} ms; the cache must be back within 10 ms"
    );
    println!("stop ms (sorted): {times:?}");
}

#[test]
fn a_stopped_pipeline_restarts_under_a_new_epoch_and_shows_no_old_frame() {
    let mut editor = nested_animated_editor();
    let play = play_range(&editor);

    let mut lent = DrawCache::new();
    for f in 0..=3 {
        build(&mut editor, f, &mut lent);
    }
    let mut pipeline = Pipeline::new();
    let mut inputs = StageInputs::from_editor(&editor, area(), 1.0);
    inputs.frame = 4;
    pipeline.start(lent, epoch(), inputs.clone(), play, 4, 3);
    pipeline.playhead(4);
    let Taken::Ready(p) = pipeline.take(4, Duration::from_secs(10)) else {
        panic!("frame 4 not ready");
    };
    assert!(p.epoch == epoch());
    pipeline.recycle(p.scene);

    // An edit: stop (the thread may be anywhere, with frames queued) and start
    // again under the next epoch with the cache that came back.
    let Stopped::Returned(back) = pipeline.stop() else {
        panic!("cache not returned");
    };
    let next = Epoch { revision: 2, ..epoch() };
    pipeline.start(*back, next, inputs, play, 5, 4);
    for f in 5..12 {
        pipeline.playhead(f);
        match pipeline.take(f, Duration::from_secs(10)) {
            Taken::Ready(p) => {
                assert_eq!(p.frame, f);
                assert!(p.epoch == next, "frame {f} came from the old epoch");
                pipeline.recycle(p.scene);
            }
            _ => panic!("frame {f} not ready"),
        }
    }
    assert!(matches!(pipeline.stop(), Stopped::Returned(_)));
}

#[test]
fn an_aborted_pass_is_not_judged_and_leaves_the_next_frame_exact() {
    let mut editor = nested_animated_editor();

    // Two caches walked through frames 0..=4.
    let mut aborted = DrawCache::new();
    let mut fresh = DrawCache::new();
    for f in 0..=4 {
        build(&mut editor, f, &mut aborted);
        build(&mut editor, f, &mut fresh);
    }

    // Frame 5 on `aborted` with the flag raised: the pass stops at the first
    // layer, is not judged (`note_lines`), and still ends its generation.
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    aborted.set_abort(Arc::clone(&flag));
    assert!(!aborted.is_aborted());
    let lines_before = aborted.last_lines();
    assert!(lines_before > 0, "the earlier frames were judged");
    flag.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(aborted.is_aborted());
    editor.current_frame = 5;
    let mut partial = vello::Scene::new();
    stage::build_scene(&mut partial, &editor, area(), 1.0, &mut aborted);
    let whole = build(&mut editor, 5, &mut DrawCache::new());
    assert!(
        partial.encoding().n_path_segments < whole.encoding().n_path_segments,
        "the aborted pass must stop before drawing the layers"
    );
    assert_eq!(
        aborted.last_lines(),
        lines_before,
        "an aborted pass is not judged: note_lines must not run"
    );
    flag.store(false, std::sync::atomic::Ordering::Relaxed);

    // The next frame, after the abort, is byte-identical to the same frame on
    // the cache that never saw frame 5.
    let after_abort = encoding_bytes(&build(&mut editor, 6, &mut aborted));
    let after_fresh = encoding_bytes(&build(&mut editor, 6, &mut fresh));
    assert!(after_abort == after_fresh, "frame 6 differs after an aborted frame 5");
}

/// A stage whose one layer holds `n` small rectangles: a single layer that takes
/// tens of milliseconds to encode, which a layer-boundary abort cannot interrupt.
fn one_big_layer_editor(n: u64) -> Editor {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(400.0, 300.0);
    let layer = scene.add_layer("Crowd", LayerKind::Normal);
    scene.update_layer(layer, |l| {
        for i in 0..n {
            let (x, y) = ((i % 200) as f64 * 2.0, (i / 200 % 150) as f64 * 2.0);
            l.frames.push_object(
                0,
                shape(1000 + i, KRect::new(x, y, x + 1.5, y + 1.5), ((i % 250) as u8, 80, 160)),
            );
        }
        l.frames.insert_frame(5);
    });
    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(400.0, 300.0);
    editor.camera.center = Point::new(200.0, 150.0);
    editor.camera.zoom = 1.0;
    editor
}

/// S7 (stop latency): the abort is looked at between objects, not only between
/// layers, so one heavy layer no longer holds a stop up for its whole encode.
#[test]
fn an_abort_inside_one_big_layer_stops_within_the_layer() {
    let mut editor = one_big_layer_editor(30_000);
    // The whole layer, on a cache that is not aborted.
    let mut whole_cache = DrawCache::new();
    let t = Instant::now();
    let whole = build(&mut editor, 0, &mut whole_cache);
    let whole_ms = t.elapsed().as_secs_f64() * 1e3;
    assert!(whole_ms > 20.0, "the fixture must take a while to encode ({whole_ms:.1} ms)");

    // The same frame, the flag raised 2 ms into the encode by another thread.
    let mut cache = DrawCache::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    cache.set_abort(Arc::clone(&flag));
    let raiser = {
        let flag = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2));
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        })
    };
    let t = Instant::now();
    let partial = build(&mut editor, 0, &mut cache);
    let aborted_ms = t.elapsed().as_secs_f64() * 1e3;
    raiser.join().unwrap();
    assert!(cache.is_aborted());
    assert!(
        partial.encoding().n_path_segments < whole.encoding().n_path_segments,
        "the aborted pass must have stopped inside the layer"
    );
    assert!(
        aborted_ms < whole_ms / 2.0,
        "aborted after {aborted_ms:.1} ms of a {whole_ms:.1} ms encode: the abort was not seen inside the layer"
    );

    // And the cache is as good as one that never saw it.
    flag.store(false, std::sync::atomic::Ordering::Relaxed);
    let after = encoding_bytes(&build(&mut editor, 1, &mut cache));
    let fresh = encoding_bytes(&build(&mut editor, 1, &mut DrawCache::new()));
    assert!(after == fresh, "the frame after an aborted one differs from a fresh cache's");
}
