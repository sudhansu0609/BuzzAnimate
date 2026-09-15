//! The never-hang gate.
//!
//! The whole point of the Never-Hang Wave is that the frame stays cheap no
//! matter how big the document is. This builds a deliberately enormous one —
//! thousands of symbols, hundreds of layers, ten thousand frames, tens of
//! thousands of objects — and asserts that the operations that used to scale
//! with the whole document now do not.
//!
//! The budgets are generous (a loaded CI box, and debug builds get a slower
//! allocator than release) but still one to two orders of magnitude below the
//! O(document) costs they guard: those miss a tight bound by 100×, not by the
//! 2× a noisy machine adds. If one of these fails, an O(document) cost has been
//! put back on the frame.

use std::time::{Duration, Instant};

use buzz_doc::Document;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerKind, Scene, ShapeData, SymbolKind};
use buzz_ui::{LibraryState, TimelineState};
use peniko::Color;

const LAYERS: usize = 400;
const SYMBOLS: usize = 4_000;
const FRAMES: u32 = 10_000;
/// Objects per layer, so the document holds tens of thousands in total.
const OBJECTS_PER_LAYER: usize = 40;

/// A document big enough that any O(document) per-frame cost is a hang.
fn monster() -> Scene {
    let mut scene = Scene::default();

    // A pile of symbols for the Library.
    for i in 0..SYMBOLS {
        scene.add_symbol(format!("symbol {i}"), SymbolKind::Graphic, None);
    }

    // Many layers, each carrying some shapes; the first stretched to the full
    // frame count so the timeline is at its cap.
    let first = scene.layers().iter().next().expect("a layer").id;
    scene.update_layer(first, |l| {
        l.frames.insert_frame(FRAMES - 1);
    });
    for li in 0..LAYERS {
        let layer = scene.add_layer(format!("Layer {li}"), LayerKind::Normal);
        for oi in 0..OBJECTS_PER_LAYER {
            let x = (oi * 12) as f64;
            scene.add_shape(
                layer,
                ShapeData::filled(Rect::new(x, 0.0, x + 10.0, 10.0).to_path(1e-9), Color::WHITE),
            );
        }
    }
    scene
}

fn screen() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1920.0, 1040.0),
        )),
        ..Default::default()
    }
}

/// A no-op `Document::edit` must be cheap: it snapshots the scene for undo, and
/// with the library maps behind `Arc` that is pointer copies, not a node per
/// symbol/sound/image. Pins plan 1.5.
#[test]
fn a_no_op_edit_is_cheap_on_a_huge_document() {
    let mut doc = Document::new(monster());
    let start = Instant::now();
    for _ in 0..200 {
        doc.edit("touch", |scene| {
            let _ = scene.stage().size;
        });
    }
    let per = start.elapsed() / 200;
    assert!(
        per < Duration::from_millis(2),
        "a no-op edit took {per:?} on a {SYMBOLS}-symbol document — Scene::clone is not cheap"
    );
}

/// `frame_kind` is called once per timeline cell — thousands of columns times
/// every layer. A million calls must stay well under a frame. Pins plan 1.1.
#[test]
fn frame_kind_is_cheap_in_bulk() {
    let mut scene = Scene::default();
    let layer = scene.layers().iter().next().expect("a layer").id;
    scene.update_layer(layer, |l| {
        // 2 000 keyframes across 10 000 frames.
        for f in (0..FRAMES).step_by(5) {
            l.frames.insert_keyframe(f);
        }
    });
    let track = &scene.layers().get(layer).unwrap().frames;

    let start = Instant::now();
    let mut sink = 0u32;
    for i in 0..1_000_000u32 {
        sink = sink.wrapping_add(track.frame_kind(i % FRAMES) as u32);
    }
    std::hint::black_box(sink);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(50),
        "a million frame_kind calls took {elapsed:?} — it is not a binary search"
    );
}

/// A headless pass of the timeline over the monster must be a screenful of
/// cells, not the whole grid. Pins plan 1.6.
#[test]
fn the_timeline_panel_is_bounded_on_the_monster() {
    let scene = monster();
    let state = TimelineState {
        current_frame: 0,
        active_layer: None,
        camera_selected: false,
        selected_light: None,
        playing: false,
        onion_enabled: false,
        auto_keyframe: false,
        edit_multiple: false,
        onion_before: 2,
        onion_after: 2,
        frame_width: 8.0,
        row_scale: 1.0,
        parenting_view: false,
        depth_view: false,
        focal_distance: buzz_scene::DEFAULT_FOCAL_DISTANCE,
        nearest_depth: -buzz_scene::DEFAULT_FOCAL_DISTANCE * 0.9,
        waveforms: std::collections::BTreeMap::new(),
        beats: Vec::new(),
    };

    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    for _ in 0..2 {
        let _ = ctx.run_ui(screen(), |ui| {
            let _ = buzz_ui::timeline_panel(ui, &scene, &state);
        });
    }
    let start = Instant::now();
    let out = ctx.run_ui(screen(), |ui| {
        let _ = buzz_ui::timeline_panel(ui, &scene, &state);
    });
    let elapsed = start.elapsed();
    assert!(
        out.shapes.len() < 80_000,
        "the timeline emitted {} shapes — the grid is not virtualized",
        out.shapes.len()
    );
    assert!(
        elapsed < Duration::from_millis(20),
        "the timeline pass took {elapsed:?}"
    );
}

/// A headless pass of the Library over the monster must build a screenful of
/// rows, not all four thousand. Pins plan 1.7. The use counts come from the
/// shell's background cache in the app, so an empty map is passed here.
#[test]
fn the_library_panel_is_bounded_on_the_monster() {
    let mut scene = monster();
    let mut state = LibraryState::default();
    let usage = std::collections::BTreeMap::new();

    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    for _ in 0..2 {
        let _ = ctx.run_ui(screen(), |ui| {
            let _ = buzz_ui::library_panel(ui, &mut scene, &mut state, &usage, &mut |_| None);
        });
    }
    let start = Instant::now();
    let _ = ctx.run_ui(screen(), |ui| {
        let _ = buzz_ui::library_panel(ui, &mut scene, &mut state, &usage, &mut |_| None);
    });
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(20),
        "a {SYMBOLS}-symbol Library panel took {elapsed:?} — it is not virtualized"
    );
}

/// **The reported bug, directly.** Committing a freshly-merged monster scene
/// (exactly what finishing an import does) and then drawing the panels must not
/// hang: the commit is a cheap Arc swap, and the panels are all bounded.
#[test]
fn committing_a_merged_monster_and_drawing_is_bounded() {
    let mut doc = Document::new(Scene::default());
    let merged = monster();

    // The commit finish_merge does: replace the scene wholesale, one undo step.
    let start = Instant::now();
    let mut merged = Some(merged);
    doc.edit("Import", |scene| {
        if let Some(m) = merged.take() {
            *scene = m;
        }
    });
    let commit = start.elapsed();
    assert!(
        commit < Duration::from_millis(50),
        "committing the merged scene took {commit:?}"
    );

    // And drawing the timeline over it is bounded (the frame after commit).
    let scene = doc.scene();
    let state = TimelineState {
        current_frame: 0,
        active_layer: None,
        camera_selected: false,
        selected_light: None,
        playing: false,
        onion_enabled: false,
        auto_keyframe: false,
        edit_multiple: false,
        onion_before: 2,
        onion_after: 2,
        frame_width: 8.0,
        row_scale: 1.0,
        parenting_view: false,
        depth_view: false,
        focal_distance: buzz_scene::DEFAULT_FOCAL_DISTANCE,
        nearest_depth: -buzz_scene::DEFAULT_FOCAL_DISTANCE * 0.9,
        waveforms: std::collections::BTreeMap::new(),
        beats: Vec::new(),
    };
    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    let _ = ctx.run_ui(screen(), |ui| {
        let _ = buzz_ui::timeline_panel(ui, scene, &state);
    });
}
