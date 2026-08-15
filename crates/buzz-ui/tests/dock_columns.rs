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
//!
//! The same complaint came back a second time, sideways: panels "obscured by
//! the scroll bar", the Library hidden, the Layers panel's switches nowhere to
//! be found. Also one fact — a column is only as wide as it is, and a scroll
//! bar that floats over the content takes the right-hand end of every row in
//! it. The tests at the foot of this file measure *width* for the same reason
//! the ones above measure height.

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

/// Draw a column of panels and report how far past its right edge they ran.
///
/// Zero or less means everything fitted. A positive number is the width of
/// whatever is hanging off the end of the panel, where it is drawn under the
/// scroll bar, under the next column, or not at all.
fn column_overflow(width: f32, draw: impl FnOnce(&mut egui::Ui)) -> f32 {
    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);
    let mut over = 0.0;
    let mut draw = Some(draw);
    let _ = ctx.run_ui(screen(), |ui| {
        egui::Panel::right("dock")
            .resizable(false)
            .exact_size(width)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("column")
                    .show(ui, |ui| {
                        // What the scroll area is willing to give the contents,
                        // which is the column less the bar's own width.
                        let usable = ui.max_rect().right();
                        if let Some(draw) = draw.take() {
                            draw(ui);
                        }
                        // `min_rect` is what the contents actually took, and it
                        // grows past `max_rect` when a row does not fit.
                        over = ui.min_rect().right() - usable;
                    });
            });
    });
    over
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
        let _ = library_panel(ui, &mut scene, &mut lib_state, &Default::default(), &mut |_| None);
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

/// **A scroll bar reserves its width; it does not sit on the panel.**
///
/// This is the "obscured by the scroll bar" report, as one number. egui's
/// default bar floats over the content, and every dock column is a scroll
/// area — so the bar was drawn across the right-hand end of whatever panel
/// was in it, which is where the panels keep their menus and their buttons.
#[test]
fn a_dock_column_leaves_room_for_its_scroll_bar() {
    let ctx = egui::Context::default();
    buzz_ui::theme::apply(&ctx);

    // Both slots, because the theme installs the same style in each and a
    // dark-only application still has to survive a light system setting.
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let scroll = ctx.style_of(theme).spacing.scroll;
        assert!(
            !scroll.floating,
            "the dock's scroll bars float over the panels again \u{2014} the right-hand \
             end of every row in a column is drawn underneath one"
        );

        // And the width it takes is the width the layouts are budgeted against.
        assert!(
            scroll.bar_width + scroll.bar_inner_margin + scroll.bar_outer_margin
                <= Metrics::SCROLL_BAR,
            "the bar now takes more than the {} points the panels leave for it",
            Metrics::SCROLL_BAR
        );
    }
}

/// **A layer's row fits in the narrowest column the workspace allows.**
///
/// The eye, the padlock and the outline box are the controls that were
/// reported missing. They were never missing: they were at the *left* of a row
/// that ended in a variable-length name, in a column the user had dragged down
/// to 144 points, with a floating scroll bar over the last few. Every one of
/// those three things is fixed by this measurement staying at zero.
///
/// **With a layer selected.** The first version of this test did not select
/// one, so the two rows that only the selected layer draws — its parent and
/// its kind — were never measured, and they were the pair that overflowed by
/// more than fifty points. A Layers panel with nothing selected is not the one
/// anybody uses.
#[test]
fn a_layer_row_fits_the_narrowest_column() {
    let narrowest = *buzz_ui::workspace::COLUMN_WIDTH_RANGE.start();

    let mut scene = Scene::default();
    // A long name, because a short one would fit anything and prove nothing.
    let long = scene.add_layer("Right forearm, overlap pass", LayerKind::Normal);
    scene.add_layer("Head", LayerKind::Normal);

    for (what, active) in [("no layer selected", None), ("a layer selected", Some(long))] {
        let mut selection = Selection::new();
        selection.set_active_layer(active);

        let over = column_overflow(narrowest, |ui| {
            let _ = buzz_ui::panels::layers_panel(ui, &mut scene, &mut selection, 0);
        });

        assert!(
            over <= 0.0,
            "with {what}, a layer row runs {over:.0} points past the end of a \
             {narrowest:.0}-point column. That is not only a clipped control: a \
             widget wider than its column expands the column's own rect, and the \
             stage is then laid out underneath the panel."
        );
    }
}

/// And so does the Library, which is the other panel reported as hidden.
#[test]
fn the_library_fits_the_narrowest_column() {
    let narrowest = *buzz_ui::workspace::COLUMN_WIDTH_RANGE.start();

    let mut scene = Scene::default();
    for i in 0..12 {
        scene.add_symbol(
            format!("Background element {i}"),
            buzz_scene::SymbolKind::Graphic,
            None,
        );
    }
    let mut state = LibraryState::default();

    let over = column_overflow(narrowest, |ui| {
        let _ = library_panel(ui, &mut scene, &mut state, &Default::default(), &mut |_| None);
    });

    assert!(
        over <= 0.0,
        "the Library runs {over:.0} points past the end of a {narrowest:.0}-point column"
    );
}

/// **And every other panel that goes in a column, too.**
///
/// The report was not about one panel. It was "the right side and the other
/// tabs" — so this walks the lot at the narrowest width the workspace will
/// give them, and names the one that does not fit.
#[test]
fn every_docked_panel_fits_the_narrowest_column() {
    let narrowest = *buzz_ui::workspace::COLUMN_WIDTH_RANGE.start();

    let mut scene = Scene::default();
    scene.add_layer("Head", LayerKind::Normal);
    scene.add_symbol("Hero Body", buzz_scene::SymbolKind::Graphic, None);

    let selection = Selection::new();
    let mut style = DrawStyle::default();
    let mut view = ViewSettings::default();
    let mut swatches = SwatchState::default();
    let mut filters = FilterPanelState::default();
    let mut lights = LightPanelState::default();
    let mut actions = ActionsState::default();
    let mut assets_state = AssetPanelState::default();
    let assets = AssetLibrary::default();
    let rig = buzz_scene::LightRig::default();

    // Each is drawn on its own, because a panel that overflows has to be
    // named — a single figure for the whole column says only that something
    // somewhere is too wide.
    let mut failures: Vec<String> = Vec::new();
    let check = |name: &str, over: f32, failures: &mut Vec<String>| {
        if over > 0.0 {
            failures.push(format!("{name} by {over:.0} points"));
        }
    };

    check(
        "Properties",
        column_overflow(narrowest, |ui| {
            let _ = buzz_ui::panels::properties_panel(
                ui,
                &mut scene,
                &selection,
                &mut style,
                &mut view,
                EditAt::exact(0),
            );
        }),
        &mut failures,
    );
    check(
        "Color",
        column_overflow(narrowest, |ui| {
            buzz_ui::panels::color_panel(ui, &scene, &mut style);
        }),
        &mut failures,
    );
    check(
        "Swatches",
        column_overflow(narrowest, |ui| {
            swatch_panel(ui, &mut scene, &mut swatches, &mut style);
        }),
        &mut failures,
    );
    check(
        "Layer Depth",
        column_overflow(narrowest, |ui| {
            let _ = depth_panel(ui, &scene, selection.active_layer());
        }),
        &mut failures,
    );
    check(
        "Armature",
        column_overflow(narrowest, |ui| {
            let _ = rig_panel(ui, None, &[], &mut RigPanelState::default());
        }),
        &mut failures,
    );
    check(
        "Filters",
        column_overflow(narrowest, |ui| {
            let _ = filter_panel(ui, &[], None, &mut filters, false);
        }),
        &mut failures,
    );
    check(
        "Lighting",
        column_overflow(narrowest, |ui| {
            let _ = light_panel(ui, &rig, &mut lights);
        }),
        &mut failures,
    );
    check(
        "Sound",
        column_overflow(narrowest, |ui| {
            let _ = sound_panel(ui, &[], None, true, 0);
        }),
        &mut failures,
    );
    check(
        "Assets",
        column_overflow(narrowest, |ui| {
            let _ = assets_panel(ui, &assets, &mut assets_state, false);
        }),
        &mut failures,
    );
    check(
        "Actions",
        column_overflow(narrowest, |ui| {
            let _ = actions_panel(ui, &mut actions, &[]);
        }),
        &mut failures,
    );
    check(
        "Camera",
        column_overflow(narrowest, |ui| {
            let _ = camera_panel(ui, scene.camera(), 0);
        }),
        &mut failures,
    );

    assert!(
        failures.is_empty(),
        "these panels run past the end of a {narrowest:.0}-point column, which is \
         where their controls go missing: {}",
        failures.join(", ")
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
