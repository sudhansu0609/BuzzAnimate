//! The timeline grid is virtualized: however many layers and frames a document
//! has, only the cells the user can see are painted. Without this a document
//! with hundreds of layers and thousands of frames emitted millions of shapes a
//! frame and froze the window — the import hang, paid on the timeline.
//!
//! This measures it: a 200-layer, ~10 000-frame timeline drawn into a normal
//! window must emit a bounded number of shapes, not the ~6 million a full grid
//! would.

use buzz_scene::{LayerKind, Scene};
use buzz_ui::{TimelineState, Waveform};

fn screen() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1920.0, 1040.0),
        )),
        ..Default::default()
    }
}

/// A deliberately enormous timeline: 200 layers, ~10 000 frames long.
fn monster() -> Scene {
    let mut scene = Scene::default();
    let first = scene.layers().iter().next().expect("a layer").id;
    // Stretch the first layer to 9 999 frames so the grid is at its cap.
    scene.update_layer(first, |l| {
        l.frames.insert_frame(9_998);
    });
    for i in 0..200 {
        scene.add_layer(format!("Layer {i}"), LayerKind::Normal);
    }
    scene
}

fn state() -> TimelineState {
    TimelineState {
        current_frame: 0,
        active_layer: None,
        camera_selected: false,
        playing: false,
        onion_enabled: false,
        auto_keyframe: false,
        edit_multiple: false,
        onion_before: 2,
        onion_after: 2,
        frame_width: 8.0,
        row_scale: 1.0,
        waveforms: std::collections::BTreeMap::new(),
    }
}

#[test]
fn a_huge_timeline_paints_a_bounded_number_of_shapes() {
    let scene = monster();
    let st = state();

    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);

    // Warm-up pass, then a measured pass — egui settles scroll/layout state on
    // the first frame, and it is the steady-state cost we care about.
    for _ in 0..2 {
        let _ = ctx.run_ui(screen(), |ui| {
            let _ = buzz_ui::timeline_panel(ui, &scene, &st);
        });
    }
    let output = ctx.run_ui(screen(), |ui| {
        let _ = buzz_ui::timeline_panel(ui, &scene, &st);
    });

    let shapes = output.shapes.len();
    // A full grid would be 200 × 9 999 × ~3 ≈ 6 million. Virtualized, only a
    // screen's worth of rows and columns is painted — tens of thousands at most.
    assert!(
        shapes < 60_000,
        "the timeline painted {shapes} shapes — virtualization is not bounding the grid"
    );
    assert!(shapes > 0, "it should still draw the visible part");
}

#[test]
fn the_waveform_type_holds_its_levels_by_arc() {
    // A cheap guard that the waveform envelope is shared, not copied, into the
    // panel state (plan 1.2).
    let w = Waveform {
        start_frame: 0,
        levels: std::sync::Arc::new(vec![0.1, 0.2, 0.3]),
    };
    let clone = w.clone();
    assert!(std::sync::Arc::ptr_eq(&w.levels, &clone.levels));
}
