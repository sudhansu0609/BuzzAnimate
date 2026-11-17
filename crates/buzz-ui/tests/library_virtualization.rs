//! A library with thousands of symbols must draw without a per-frame cost that
//! scales with the whole library. The heavy work — the use-count walk and the
//! thumbnail bookkeeping — is off the panel now; this measures what the panel
//! itself emits.

use buzz_scene::{Scene, SymbolKind};
use buzz_ui::LibraryState;

fn screen() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1920.0, 1040.0),
        )),
        ..Default::default()
    }
}

fn library_of(n: usize) -> Scene {
    let mut scene = Scene::default();
    for i in 0..n {
        scene.add_symbol(format!("symbol {i}"), SymbolKind::Graphic, None);
    }
    scene
}

#[test]
fn a_large_library_panel_draws_within_budget() {
    let mut scene = library_of(10_000);
    let mut state = LibraryState::default();
    let usage = std::collections::BTreeMap::new();

    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);

    // Warm-up, then a timed steady-state pass.
    for _ in 0..2 {
        let _ = ctx.run_ui(screen(), |ui| {
            let _ = buzz_ui::library_panel(ui, &mut scene, &mut state, &usage, &mut |_, _| None);
        });
    }
    let start = std::time::Instant::now();
    let output = ctx.run_ui(screen(), |ui| {
        let _ = buzz_ui::library_panel(ui, &mut scene, &mut state, &usage, &mut |_, _| None);
    });
    let elapsed = start.elapsed();

    // Virtualized, this is a screenful of rows a frame, not ten thousand: it
    // measured ~0.8 ms where the un-virtualized panel was ~90 ms. The bound is
    // loose enough for a loaded CI box and still an order below the old cost.
    assert!(
        elapsed < std::time::Duration::from_millis(15),
        "a 10k-symbol library panel took {elapsed:?} — virtualization regressed"
    );
    assert!(!output.shapes.is_empty(), "it should draw the library");
}

/// Hovering a symbol's small picture asks for its big one, for the tooltip
/// preview; a Library nobody hovers only ever asks for the small ones.
#[test]
fn hovering_a_thumbnail_asks_for_the_big_picture() {
    let mut scene = library_of(1);
    let mut state = LibraryState::default();
    let usage = std::collections::BTreeMap::new();
    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);

    let mut asked = Vec::new();
    let mut frame = |at: Option<egui::Pos2>, t: f64, asked: &mut Vec<buzz_ui::ThumbnailSize>| {
        let mut input = screen();
        input.time = Some(t);
        if let Some(at) = at {
            input.events.push(egui::Event::PointerMoved(at));
        }
        let _ = ctx.run_ui(input, |ui| {
            let _ = buzz_ui::library_panel(ui, &mut scene, &mut state, &usage, &mut |_, size| {
                asked.push(size);
                None
            });
        });
    };
    for i in 0..3 {
        frame(None, i as f64, &mut asked);
    }
    assert!(!asked.contains(&buzz_ui::ThumbnailSize::Preview), "nothing hovered yet");

    // Sweep the pointer down the thumbnail column until the tooltip shows.
    let mut t = 3.0;
    'sweep: for x in (16..48).step_by(4) {
        for y in (0..240).step_by(4) {
            for _ in 0..3 {
                t += 1.0;
                frame(Some(egui::pos2(x as f32, y as f32)), t, &mut asked);
            }
            if asked.contains(&buzz_ui::ThumbnailSize::Preview) {
                break 'sweep;
            }
        }
    }
    assert!(asked.contains(&buzz_ui::ThumbnailSize::Preview), "hovering asks for the preview");
}
