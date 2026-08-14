//! **Do the panels fit in the window?**
//!
//! Not "do they draw" — they always drew. The complaint that produced this file
//! was that the Library looked wrong, the Assets panel could not be found, and
//! the Layers and Properties panels crowded each other. Every one of those is
//! the same fact: a dock column is as tall as the window, and the panels in it
//! were taller than that. Nothing says so at a glance, because the column
//! scrolls — the panels below simply are not there.
//!
//! So this measures. A panel column that overflows the window is a defect, and
//! the numbers below are what makes it one that can be caught.

use buzz_doc::AssetLibrary;
use buzz_scene::{EditAt, LayerKind, Scene};
use buzz_ui::*;

/// The height of a dock column on a 1080p screen, once the menu bar, the
/// status bar and the timeline have taken their share.
const COLUMN: f32 = 854.0;

fn screen() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1920.0, 1040.0),
        )),
        ..Default::default()
    }
}

/// Draw a column of panels and report how tall it came out.
fn column_height(width: f32, draw: impl FnOnce(&mut egui::Ui)) -> f32 {
    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    let mut used = 0.0;
    let mut draw = Some(draw);
    let _ = ctx.run_ui(screen(), |ui| {
        egui::Panel::right("dock")
            .resizable(false)
            .exact_size(width)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("column")
                    .show(ui, |ui| {
                        let top = ui.cursor().top();
                        if let Some(draw) = draw.take() {
                            draw(ui);
                        }
                        used = ui.cursor().top() - top;
                    });
            });
    });
    used
}

/// **The Library and the Assets panel both fit, together, in their column.**
///
/// This is the "no asset panel seen" complaint. It was never missing from the
/// layout — it was below the Library, and the Library had been told it could
/// have the whole column.
#[test]
fn the_library_and_the_assets_panel_fit_side_by_side_in_one_column() {
    let mut scene = Scene::default();
    // A working library, not an empty one: an empty panel proves nothing.
    for i in 0..40 {
        scene.add_symbol(format!("Symbol {i}"), buzz_scene::SymbolKind::Graphic, None);
    }
    let mut lib_state = LibraryState::default();
    let assets = AssetLibrary::default();
    let mut asset_state = AssetPanelState::default();

    let used = column_height(240.0, |ui| {
        let _ = library_panel(ui, &mut scene, &mut lib_state);
        ui.separator();
        let _ = assets_panel(ui, &assets, &mut asset_state, false);
    });

    assert!(
        used < COLUMN,
        "the Library and the Assets panel need {used:.0} points of a \
         {COLUMN}-point column. Whatever is past the bottom cannot be found: \
         a column that overflows looks exactly like a panel that is missing."
    );
}

/// **The Layers, Properties and Colour panels fit, on a real character.**
///
/// Fifteen layers is an ordinary rigged character — head, hair, eyes, brows,
/// mouth, two arms, two hands, body, two legs, shadow, and a couple spare.
/// Every layer used to draw two rows and seventy points, so the Layers panel
/// alone was taller than the window and the three panels below it were nowhere.
#[test]
fn a_fifteen_layer_character_leaves_room_for_the_panels_below_it() {
    let mut scene = Scene::default();
    for name in [
        "Shadow",
        "Left leg",
        "Right leg",
        "Body",
        "Left arm",
        "Right arm",
        "Left hand",
        "Right hand",
        "Head",
        "Hair",
        "Left eye",
        "Right eye",
        "Brows",
        "Mouth",
        "Props",
    ] {
        scene.add_layer(name, LayerKind::Normal);
    }
    let mut selection = Selection::new();
    let mut style = DrawStyle::default();
    let mut view = ViewSettings::default();

    let used = column_height(343.0, |ui| {
        let _ = buzz_ui::panels::layers_panel(ui, &mut scene, &mut selection, 0);
        ui.separator();
        let _ = buzz_ui::panels::properties_panel(
            ui,
            &mut scene,
            &selection,
            &mut style,
            &mut view,
            EditAt::exact(0),
        );
        ui.separator();
        buzz_ui::panels::color_panel(ui, &scene, &mut style);
    });

    assert!(
        used < COLUMN,
        "Layers, Properties and Colour need {used:.0} points of a {COLUMN}-point \
         column for a fifteen-layer character. The panels past the bottom are \
         the ones the user reports as missing."
    );
}

/// One layer, one row. The details belong to the layer being worked on.
#[test]
fn a_layer_that_is_not_selected_costs_one_row() {
    let mut scene = Scene::default();
    for i in 0..10 {
        scene.add_layer(format!("Layer {i}"), LayerKind::Normal);
    }
    let mut selection = Selection::new();

    let used = column_height(343.0, |ui| {
        let _ = buzz_ui::panels::layers_panel(ui, &mut scene, &mut selection, 0);
    });

    // Ten rows, a header and the selected layer's second row. Generous, and
    // still less than half what two rows apiece cost.
    assert!(
        used < 420.0,
        "ten layers took {used:.0} points \u{2014} about {:.0} a layer",
        used / 10.0
    );
}
