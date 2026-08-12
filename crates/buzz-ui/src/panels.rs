//! Panel rendering.
//!
//! Panels take state by reference and return the [`Command`]s the user raised.
//! Nothing here owns editor state, so the same panel can be driven by tests or
//! by the running application.

use buzz_scene::{LayerId, LayerKind, Scene};
use egui::{Color32, RichText, Ui};
use peniko::Color;

use crate::command::{Command, shortcut_text};
use crate::selection::Selection;
use crate::style::{DrawStyle, DrawingMode, StrokeKind};
use crate::theme::{Metrics, Palette};
use crate::tools::{TOOL_GROUPS, ToolId, ToolStatus};
use crate::view::ViewSettings;

/// Convert a document colour into an egui colour.
///
/// # Precision
///
/// `Color32` stores **premultiplied** 8-bit channels, so a translucent colour
/// loses low-order bits on the way in and cannot be recovered exactly: at
/// `alpha = 200`, a red channel of `1` premultiplies to `0`. Opaque colours
/// round-trip exactly.
///
/// This is acceptable because the document's [`Color`] stays authoritative —
/// panels write back only when the widget reports a change — so the quantised
/// value is only adopted when the user actually edits that colour. It is
/// documented rather than worked around because a silent precision loss in
/// colour handling is exactly the sort of thing that shows up later as a
/// mysterious drift.
pub fn to_egui(color: Color) -> Color32 {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Convert an egui colour back into a document colour.
pub fn from_egui(color: Color32) -> Color {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    Color::from_rgba8(r, g, b, a)
}

/// Draw the menu bar. Returns whatever the user chose.
pub fn menu_bar(
    ui: &mut Ui,
    scene: &Scene,
    selection: &Selection,
    view: &ViewSettings,
    can_undo: bool,
    can_redo: bool,
) -> Vec<Command> {
    let mut raised = Vec::new();

    egui::MenuBar::new().ui(ui, |ui| {
        let has_selection = !selection.is_empty();

        let item = |ui: &mut Ui, command: Command, enabled: bool, out: &mut Vec<Command>| {
            let shortcut = shortcut_text(ui.ctx(), command);
            let button = egui::Button::new(command.label()).shortcut_text(shortcut);
            if ui.add_enabled(enabled, button).clicked() {
                out.push(command);
                ui.close();
            }
        };

        ui.menu_button("File", |ui| {
            for c in [Command::New, Command::Open] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            for c in [Command::ImportToStage, Command::ImportToLibrary] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            for c in [Command::Save, Command::SaveAs] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            for c in [Command::Close, Command::Quit] {
                item(ui, c, true, &mut raised);
            }
        });

        ui.menu_button("Edit", |ui| {
            item(ui, Command::Undo, can_undo, &mut raised);
            item(ui, Command::Redo, can_redo, &mut raised);
            ui.separator();
            for c in [Command::Cut, Command::Copy, Command::Paste, Command::Delete] {
                let enabled = c == Command::Paste || has_selection;
                item(ui, c, enabled, &mut raised);
            }
            ui.separator();
            item(ui, Command::DuplicateSelection, has_selection, &mut raised);
            item(ui, Command::SelectAll, true, &mut raised);
            item(ui, Command::Deselect, has_selection, &mut raised);
            ui.separator();
            // Animate's Edit menu is also where you step in and out of a
            // symbol. Edit Symbol works on a selected instance; Edit Document
            // only means anything when a symbol is actually open.
            item(ui, Command::EditSymbol, has_selection, &mut raised);
            item(
                ui,
                Command::EditDocument,
                !scene.edit_path().is_empty(),
                &mut raised,
            );
        });

        ui.menu_button("View", |ui| {
            for c in [
                Command::ZoomIn,
                Command::ZoomOut,
                Command::ZoomActual,
                Command::ZoomFitInWindow,
                Command::ZoomShowFrame,
                Command::ZoomShowAll,
            ] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();

            let toggle = |ui: &mut Ui, command: Command, on: bool, out: &mut Vec<Command>| {
                let mark = if on { "✔ " } else { "   " };
                let shortcut = shortcut_text(ui.ctx(), command);
                let button = egui::Button::new(format!("{mark}{}", command.label()))
                    .shortcut_text(shortcut);
                if ui.add(button).clicked() {
                    out.push(command);
                    ui.close();
                }
            };
            toggle(ui, Command::ToggleRulers, view.show_rulers, &mut raised);
            toggle(ui, Command::ToggleGrid, view.show_grid, &mut raised);
            toggle(ui, Command::ToggleGuides, view.show_guides, &mut raised);
            toggle(ui, Command::ToggleSnapping, view.snap.to_objects, &mut raised);
            toggle(ui, Command::TogglePasteboard, view.show_pasteboard, &mut raised);
        });

        ui.menu_button("Insert", |ui| {
            item(ui, Command::NewSymbol, true, &mut raised);
            ui.separator();
            // Tweens are left enabled whatever the playhead is on: the editor
            // is the one that knows whether the frame is a keyframe, and it
            // says so in the status bar. A greyed-out item with no explanation
            // teaches the user less than a message that names the reason.
            for c in [
                Command::CreateMotionTween,
                Command::CreateShapeTween,
                Command::CreateClassicTween,
                Command::RemoveTween,
            ] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            for c in [Command::NewLayer, Command::NewLayerFolder] {
                item(ui, c, true, &mut raised);
            }
            item(
                ui,
                Command::DeleteLayer,
                scene.layers().len() > 1,
                &mut raised,
            );
        });

        // Animate keeps playback under Control and the camera under View; the
        // camera gets its own menu here because it has more than one item.
        ui.menu_button("Control", |ui| {
            for c in [
                Command::PlayPause,
                Command::FirstFrame,
                Command::PreviousFrame,
                Command::NextFrame,
                Command::LastFrame,
            ] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            for c in [
                Command::InsertFrame,
                Command::RemoveFrame,
                Command::InsertKeyframe,
                Command::InsertBlankKeyframe,
                Command::ClearKeyframe,
            ] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            item(ui, Command::ToggleOnionSkin, true, &mut raised);
        });

        ui.menu_button("Camera", |ui| {
            let on = scene.camera().enabled;
            let mark = if on { "✔ " } else { "   " };
            if ui
                .button(format!("{mark}{}", Command::ToggleCamera.label()))
                .clicked()
            {
                raised.push(Command::ToggleCamera);
                ui.close();
            }
            ui.separator();
            for c in [
                Command::AddCameraKeyframe,
                Command::RemoveCameraKeyframe,
                Command::ResetCamera,
            ] {
                item(ui, c, on, &mut raised);
            }
            ui.separator();
            ui.label(
                RichText::new(format!("{} keyframes", scene.camera().keys().len()))
                    .small()
                    .weak(),
            );
        });

        ui.menu_button("Modify", |ui| {
            // Animate puts Convert to Symbol at the top of Modify, not under
            // Insert — Insert's entry is New Symbol, which is a different
            // thing: one wraps a selection, the other starts from nothing.
            item(ui, Command::ConvertToSymbol, has_selection, &mut raised);
            ui.separator();
            item(ui, Command::GroupSelection, has_selection, &mut raised);
            item(ui, Command::UngroupSelection, has_selection, &mut raised);
            ui.separator();
            for c in [
                Command::BringToFront,
                Command::BringForward,
                Command::SendBackward,
                Command::SendToBack,
            ] {
                item(ui, c, has_selection, &mut raised);
            }
            ui.separator();
            ui.label(RichText::new("Shape").small().weak());
            for c in [
                Command::ConvertLinesToFills,
                Command::ExpandFill,
                Command::SmoothSelection,
                Command::StraightenSelection,
            ] {
                item(ui, c, has_selection, &mut raised);
            }
        });
    });

    raised
}

/// The vertical tool strip.
pub fn tool_bar(ui: &mut Ui, active: ToolId, style: &mut DrawStyle) -> Option<ToolId> {
    let mut chosen = None;

    ui.vertical_centered(|ui| {
        for (index, group) in TOOL_GROUPS.iter().enumerate() {
            if index > 0 {
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);
            }
            for tool in group.iter().copied() {
                if tool_button(ui, tool, tool == active) {
                    chosen = Some(tool);
                }
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        color_wells(ui, style);
    });

    chosen
}

fn tool_button(ui: &mut Ui, tool: ToolId, active: bool) -> bool {
    let size = egui::vec2(Metrics::TOOL_BUTTON, Metrics::TOOL_BUTTON);
    let ready = tool.is_ready();

    let mut text = RichText::new(tool.glyph()).size(15.0);
    if !ready {
        text = text.color(Palette::TEXT_DIM);
    }

    let button = egui::Button::new(text)
        .min_size(size)
        .fill(if active {
            Palette::ACTIVE
        } else {
            Palette::RAISED
        });

    let response = ui.add_enabled(ready, button);

    let tip = match tool.status() {
        ToolStatus::Ready => match tool.shortcut() {
            Some(key) => format!("{} ({})", tool.name(), key.name()),
            None => tool.name().to_string(),
        },
        ToolStatus::Planned(note) => format!("{} — not yet available\n{note}", tool.name()),
    };
    response.on_hover_text(tip).clicked()
}

/// Stroke and fill wells, with Animate's swap and reset controls.
fn color_wells(ui: &mut Ui, style: &mut DrawStyle) {
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.vertical(|ui| {
            well(ui, "Stroke", &mut style.stroke_color, &mut style.stroke_enabled);
            ui.add_space(2.0);
            well(ui, "Fill", &mut style.fill_color, &mut style.fill_enabled);
        });
    });

    ui.horizontal(|ui| {
        // Letters, not symbols: `X` and `D` are Animate's own shortcuts for
        // these actions, and they always render.
        if ui
            .small_button("X")
            .on_hover_text("Swap stroke and fill (X)")
            .clicked()
        {
            style.swap_colors();
        }
        if ui
            .small_button("D")
            .on_hover_text("Black and white (D)")
            .clicked()
        {
            style.reset_colors();
        }
    });
}

fn well(ui: &mut Ui, label: &str, color: &mut Color, enabled: &mut bool) {
    ui.horizontal(|ui| {
        let mut rgba = to_egui(*color);
        if ui.color_edit_button_srgba(&mut rgba).changed() {
            *color = from_egui(rgba);
            *enabled = true;
        }
        let mark = if *enabled { "*" } else { "x" };
        if ui
            .small_button(mark)
            .on_hover_text(format!("{label}: click to toggle on or off"))
            .clicked()
        {
            *enabled = !*enabled;
        }
    });
}

/// The selected object, if exactly one is selected and it is an instance.
fn single_selected_instance(scene: &Scene, selection: &Selection) -> Option<buzz_scene::ObjectId> {
    let mut ids = selection.iter();
    let id = ids.next()?;
    if ids.next().is_some() {
        return None;
    }
    let (_, object) = scene.find_object(id)?;
    object.instance().map(|_| id)
}

/// Animate's Properties panel for a symbol instance.
///
/// Everything here is a property of the *placement*, not of the symbol: two
/// instances of one symbol can start on different frames, loop differently and
/// carry different colour effects, which is what makes symbols worth using.
fn instance_properties(ui: &mut Ui, scene: &mut Scene, id: buzz_scene::ObjectId) -> bool {
    use buzz_scene::{ColorEffect, LoopMode};

    let Some((_, object)) = scene.find_object(id) else {
        return false;
    };
    let Some(instance) = object.instance() else {
        return false;
    };
    let instance = instance.clone();

    let Some(symbol) = scene.library().get(instance.symbol) else {
        ui.add_space(8.0);
        ui.label(RichText::new("Symbol Instance").strong());
        ui.label(
            RichText::new("The symbol this instance refers to is no longer in the library.")
                .weak()
                .italics(),
        );
        return false;
    };
    let (symbol_name, symbol_kind, symbol_length) =
        (symbol.name.clone(), symbol.kind, symbol.length());

    // Collected first, applied after the widgets, so the scene is not borrowed
    // while the panel is drawn.
    let mut new_first_frame = instance.first_frame;
    let mut new_loop = instance.loop_mode;
    let mut new_color = instance.color;
    let mut edited = false;

    ui.add_space(8.0);
    ui.label(RichText::new("Symbol Instance").strong());

    egui::Grid::new("instance-props")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Symbol");
            ui.label(RichText::new(&symbol_name).strong())
                .on_hover_text(symbol_kind.label());
            ui.end_row();

            ui.label("Frames");
            ui.label(RichText::new(format!("{symbol_length}")).weak());
            ui.end_row();

            ui.label("First frame");
            // Frames are shown one-based, as Animate numbers them, while the
            // model counts from zero.
            let mut shown = new_first_frame + 1;
            if ui
                .add(egui::DragValue::new(&mut shown).range(1..=symbol_length.max(1)))
                .changed()
            {
                new_first_frame = shown.saturating_sub(1);
                edited = true;
            }
            ui.end_row();

            // Only a graphic follows the parent playhead, so only a graphic has
            // anything to loop. Showing the control for a movie clip would
            // promise behaviour that cannot happen.
            if symbol_kind.follows_parent_timeline() {
                ui.label("Looping");
                egui::ComboBox::from_id_salt("instance-loop")
                    .selected_text(new_loop.label())
                    .show_ui(ui, |ui| {
                        for mode in [LoopMode::Loop, LoopMode::PlayOnce, LoopMode::SingleFrame] {
                            if ui
                                .selectable_value(&mut new_loop, mode, mode.label())
                                .clicked()
                            {
                                edited = true;
                            }
                        }
                    });
                ui.end_row();
            }
        });

    // -- colour effect ------------------------------------------------------
    let effect = ColorEffect::from_transform(&instance.color);

    ui.label(RichText::new("Color Effect").small().weak());
    egui::Grid::new("instance-color")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Style");
            egui::ComboBox::from_id_salt("instance-effect")
                .selected_text(effect.label())
                .show_ui(ui, |ui| {
                    // Switching style starts that effect at a visible amount,
                    // rather than at zero where it would look broken.
                    let options = [
                        ColorEffect::None,
                        ColorEffect::Brightness(0.0),
                        ColorEffect::Tint {
                            color: Color::from_rgba8(0xFF, 0x00, 0x00, 0xFF),
                            amount: 0.5,
                        },
                        ColorEffect::Alpha(1.0),
                    ];
                    for option in options {
                        let selected = std::mem::discriminant(&option)
                            == std::mem::discriminant(&effect);
                        if ui.selectable_label(selected, option.label()).clicked()
                            && !selected
                            && let Some(t) = option.to_transform()
                        {
                            new_color = t;
                            edited = true;
                        }
                    }
                });
            ui.end_row();

            match effect {
                ColorEffect::Brightness(amount) => {
                    ui.label("Brightness");
                    let mut percent = amount * 100.0;
                    if ui
                        .add(egui::Slider::new(&mut percent, -100.0..=100.0).suffix("%"))
                        .changed()
                    {
                        new_color = buzz_scene::ColorTransform::brightness(percent / 100.0);
                        edited = true;
                    }
                    ui.end_row();
                }
                ColorEffect::Alpha(amount) => {
                    ui.label("Alpha");
                    let mut percent = amount * 100.0;
                    if ui
                        .add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%"))
                        .changed()
                    {
                        new_color = buzz_scene::ColorTransform::alpha(percent / 100.0);
                        edited = true;
                    }
                    ui.end_row();
                }
                ColorEffect::Tint { color, amount } => {
                    let mut tint = to_egui(color);
                    let mut percent = amount * 100.0;

                    ui.label("Tint");
                    if ui.color_edit_button_srgba(&mut tint).changed() {
                        new_color =
                            buzz_scene::ColorTransform::tint(from_egui(tint), percent / 100.0);
                        edited = true;
                    }
                    ui.end_row();

                    ui.label("Amount");
                    if ui
                        .add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%"))
                        .changed()
                    {
                        new_color =
                            buzz_scene::ColorTransform::tint(from_egui(tint), percent / 100.0);
                        edited = true;
                    }
                    ui.end_row();
                }
                ColorEffect::Advanced => {
                    ui.label("Advanced");
                    ui.label(
                        RichText::new("Set by a tween or an import")
                            .small()
                            .weak(),
                    )
                    .on_hover_text(
                        "This colour effect is not one of Animate's four named ones. \
                         Choosing a style above replaces it.",
                    );
                    ui.end_row();
                }
                ColorEffect::None => {}
            }
        });

    if edited {
        scene.update_object(id, |o| {
            if let buzz_scene::ObjectKind::Instance(i) = &mut o.kind {
                i.first_frame = new_first_frame;
                i.loop_mode = new_loop;
                i.color = new_color;
            }
        });
    }
    edited
}

/// Contextual properties for the current selection.
pub fn properties_panel(
    ui: &mut Ui,
    scene: &mut Scene,
    selection: &Selection,
    style: &mut DrawStyle,
    view: &mut ViewSettings,
) -> bool {
    let mut changed = false;

    ui.heading(selection.describe(scene));
    ui.separator();

    if selection.is_empty() {
        ui.label(RichText::new("Document").strong());
        egui::Grid::new("doc-props").num_columns(2).show(ui, |ui| {
            ui.label("Width");
            let mut w = scene.stage().size.width;
            if ui.add(egui::DragValue::new(&mut w).range(1.0..=16384.0)).changed() {
                scene.stage_mut().size.width = w;
                changed = true;
            }
            ui.end_row();

            ui.label("Height");
            let mut h = scene.stage().size.height;
            if ui.add(egui::DragValue::new(&mut h).range(1.0..=16384.0)).changed() {
                scene.stage_mut().size.height = h;
                changed = true;
            }
            ui.end_row();

            ui.label("FPS");
            let mut fps = scene.stage().frame_rate;
            if ui
                .add(egui::DragValue::new(&mut fps).range(0.01..=240.0).speed(0.1))
                .changed()
            {
                scene.stage_mut().frame_rate = fps;
                changed = true;
            }
            ui.end_row();

            ui.label("Background");
            let mut bg = to_egui(scene.stage().background);
            if ui.color_edit_button_srgba(&mut bg).changed() {
                scene.stage_mut().background = from_egui(bg);
                changed = true;
            }
            ui.end_row();
        });
    } else if let Some(bounds) = selection.bounds(scene) {
        egui::Grid::new("sel-props").num_columns(2).show(ui, |ui| {
            ui.label("X");
            ui.label(format!("{:.2}", bounds.x0));
            ui.end_row();
            ui.label("Y");
            ui.label(format!("{:.2}", bounds.y0));
            ui.end_row();
            ui.label("W");
            ui.label(format!("{:.2}", bounds.width()));
            ui.end_row();
            ui.label("H");
            ui.label(format!("{:.2}", bounds.height()));
            ui.end_row();
        });
    }

    // A single selected instance gets Animate's instance properties. More than
    // one would need a multi-edit model that Animate itself does not have here,
    // so the section only appears for one.
    if let Some(id) = single_selected_instance(scene, selection) {
        changed |= instance_properties(ui, scene, id);
    }

    ui.add_space(8.0);
    ui.label(RichText::new("Stroke and Fill").strong());
    egui::Grid::new("style-props").num_columns(2).show(ui, |ui| {
        ui.label("Width");
        let mut width = style.stroke_width;
        if ui
            .add(
                egui::DragValue::new(&mut width)
                    .range(0.0..=200.0)
                    .speed(0.1),
            )
            .changed()
        {
            style.stroke_width = width;
        }
        ui.end_row();

        ui.label("Hairline");
        ui.checkbox(&mut style.hairline, "");
        ui.end_row();

        ui.label("Style");
        egui::ComboBox::from_id_salt("stroke-kind")
            .selected_text(style.stroke_kind.label())
            .show_ui(ui, |ui| {
                for kind in [StrokeKind::Solid, StrokeKind::Dashed, StrokeKind::Dotted] {
                    ui.selectable_value(&mut style.stroke_kind, kind, kind.label());
                }
            });
        ui.end_row();
    });

    ui.add_space(8.0);
    // The Merge Shape / Object Drawing toggle, which changes how the drawing
    // tools behave rather than how anything looks.
    let mut object_drawing = style.drawing_mode == DrawingMode::ObjectDrawing;
    if ui
        .checkbox(&mut object_drawing, "Object Drawing")
        .on_hover_text(
            "Off: shapes merge and cut destructively, like Animate's default.\n\
             On: each shape stays a separate object.",
        )
        .changed()
    {
        style.drawing_mode = if object_drawing {
            DrawingMode::ObjectDrawing
        } else {
            DrawingMode::MergeShape
        };
    }

    ui.add_space(8.0);
    ui.label(RichText::new("View").strong());
    ui.checkbox(&mut view.show_rulers, "Rulers");
    ui.checkbox(&mut view.show_grid, "Grid");
    ui.checkbox(&mut view.show_guides, "Guides");
    ui.checkbox(&mut view.lock_guides, "Lock guides");
    ui.horizontal(|ui| {
        ui.label("Grid size");
        ui.add(
            egui::DragValue::new(&mut view.grid_spacing)
                .range(0.1..=1000.0)
                .speed(0.5),
        );
    });
    ui.label(RichText::new("Snap to").small().weak());
    ui.checkbox(&mut view.snap.to_guides, "Guides");
    ui.checkbox(&mut view.snap.to_grid, "Grid");
    ui.checkbox(&mut view.snap.to_objects, "Objects");
    ui.checkbox(&mut view.snap.to_pixels, "Pixels");

    changed
}

/// Swatches and recently used colours.
pub fn color_panel(ui: &mut Ui, style: &mut DrawStyle) {
    ui.heading("Color");
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Stroke");
        let mut c = to_egui(style.stroke_color);
        if ui.color_edit_button_srgba(&mut c).changed() {
            style.stroke_color = from_egui(c);
            style.stroke_enabled = true;
            let remembered = style.stroke_color;
            style.remember(remembered);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Fill");
        let mut c = to_egui(style.fill_color);
        if ui.color_edit_button_srgba(&mut c).changed() {
            style.fill_color = from_egui(c);
            style.fill_enabled = true;
            let remembered = style.fill_color;
            style.remember(remembered);
        }
    });

    ui.add_space(6.0);
    ui.label(RichText::new("Swatches").small().weak());

    let swatches = style.swatches.clone();
    let mut picked: Option<(Color, bool)> = None;
    ui.horizontal_wrapped(|ui| {
        for color in &swatches {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 2.0, to_egui(*color));
            ui.painter()
                .rect_stroke(rect, 2.0, egui::Stroke::new(1.0, Palette::BORDER), egui::StrokeKind::Inside);

            let response = response.on_hover_text("Click sets fill · Shift-click sets stroke");
            if response.clicked() {
                picked = Some((*color, ui.input(|i| i.modifiers.shift)));
            }
        }
    });

    if let Some((color, to_stroke)) = picked {
        if to_stroke {
            style.stroke_color = color;
            style.stroke_enabled = true;
        } else {
            style.fill_color = color;
            style.fill_enabled = true;
        }
        style.remember(color);
    }
}

/// The layer list. Returns a command if the user asked for one.
pub fn layers_panel(
    ui: &mut Ui,
    scene: &mut Scene,
    selection: &mut Selection,
) -> Option<Command> {
    let mut command = None;

    ui.horizontal(|ui| {
        ui.heading("Layers");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("🗑").on_hover_text("Delete layer").clicked() {
                command = Some(Command::DeleteLayer);
            }
            if ui.small_button("Fld").on_hover_text("New folder").clicked() {
                command = Some(Command::NewLayerFolder);
            }
            if ui.small_button("➕").on_hover_text("New layer").clicked() {
                command = Some(Command::NewLayer);
            }
        });
    });
    ui.separator();

    let active = selection.active_layer();
    let ids: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();

    egui::ScrollArea::vertical().show(ui, |ui| {
        for id in ids {
            let Some(layer) = scene.layers().get(id) else {
                continue;
            };
            let (name, kind, visible, locked, outline, color, depth) = (
                layer.name.clone(),
                layer.kind,
                layer.visible,
                layer.locked,
                layer.outline,
                layer.color,
                layer.parent.is_some() as usize,
            );

            let mut set_visible = visible;
            let mut set_locked = locked;
            let mut set_outline = outline;

            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);

                if ui
                    .selectable_label(set_visible, if set_visible { "O" } else { "-" })
                    .on_hover_text("Show or hide")
                    .clicked()
                {
                    set_visible = !set_visible;
                }
                if ui
                    .selectable_label(set_locked, if set_locked { "🔒" } else { "🔓" })
                    .on_hover_text("Lock")
                    .clicked()
                {
                    set_locked = !set_locked;
                }
                if ui
                    .selectable_label(set_outline, "▢")
                    .on_hover_text("Show as outlines")
                    .clicked()
                {
                    set_outline = !set_outline;
                }

                // The layer's colour chip, used for outline view.
                let (chip, _) = ui.allocate_exact_size(egui::vec2(8.0, 12.0), egui::Sense::hover());
                ui.painter().rect_filled(chip, 1.0, to_egui(color));

                let mark = match kind {
                    LayerKind::Folder => "F ",
                    LayerKind::Mask => "M ",
                    LayerKind::Masked => ". ",
                    LayerKind::Guide => "G ",
                    LayerKind::Guided => ". ",
                    LayerKind::Normal => "",
                };
                if ui
                    .selectable_label(active == Some(id), format!("{mark}{name}"))
                    .clicked()
                {
                    selection.set_active_layer(Some(id));
                }
            });

            if set_visible != visible || set_locked != locked || set_outline != outline {
                scene.update_layer(id, |l| {
                    l.visible = set_visible;
                    l.locked = set_locked;
                    l.outline = set_outline;
                });
            }
        }
    });

    command
}

/// Placeholder for the timeline, which arrives in Phase 3.
pub fn timeline_placeholder(ui: &mut Ui, scene: &Scene) {
    ui.horizontal(|ui| {
        ui.heading("Timeline");
        ui.label(
            RichText::new(format!("{:.0} fps", scene.stage().frame_rate))
                .small()
                .weak(),
        );
    });
    ui.separator();
    ui.label(
        RichText::new(
            "Frames, keyframes and tweening arrive in Phase 3. \
             Layers are already fully modelled and shown in the Layers panel.",
        )
        .weak()
        .italics(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_colours_round_trip_exactly() {
        for c in [
            Color::BLACK,
            Color::WHITE,
            Color::from_rgb8(0x12, 0x34, 0x56),
            Color::from_rgb8(0xFF, 0x00, 0x66),
        ] {
            let back = from_egui(to_egui(c));
            assert_eq!(
                c.to_rgba8().to_u8_array(),
                back.to_rgba8().to_u8_array(),
                "an opaque colour must survive the egui round trip exactly"
            );
        }
    }

    /// Pins the documented limit: `Color32` is premultiplied 8-bit, so a
    /// translucent colour comes back close but not identical. Asserting the
    /// bound stops the error growing unnoticed.
    #[test]
    fn translucent_colours_round_trip_within_a_known_bound() {
        for c in [
            Color::from_rgba8(1, 2, 3, 200),
            Color::from_rgba8(0x40, 0x80, 0xC0, 0x20),
            Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x01),
        ] {
            let before = c.to_rgba8().to_u8_array();
            let after = from_egui(to_egui(c)).to_rgba8().to_u8_array();

            assert_eq!(before[3], after[3], "alpha itself must be preserved");
            for channel in 0..3 {
                let drift = before[channel].abs_diff(after[channel]);
                assert!(
                    drift <= 4,
                    "channel {channel} drifted by {drift} ({before:?} -> {after:?})"
                );
            }
        }
    }

    /// Panels must not panic on an empty document, which is the state the app
    /// starts in and the one least often exercised by hand.
    #[test]
    fn panels_render_against_an_empty_document() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        let mut selection = Selection::new();
        let mut style = DrawStyle::default();
        let mut view = ViewSettings::default();

        // egui 0.35 roots the UI in a `Ui` rather than a `Context`, so panels
        // are built inside `run_ui`.
        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = menu_bar(ui, &scene, &selection, &view, false, false);
            let _ = tool_bar(ui, ToolId::Selection, &mut style);
            let _ = properties_panel(ui, &mut scene, &selection, &mut style, &mut view);
            color_panel(ui, &mut style);
            let _ = layers_panel(ui, &mut scene, &mut selection);
            timeline_placeholder(ui, &scene);
        });
    }

    /// The instance section only appears for exactly one selected instance —
    /// a shape, a mixed selection, or two instances all get the plain
    /// selection properties instead.
    #[test]
    fn the_instance_section_appears_for_one_selected_instance() {
        use buzz_geom::{Affine, Shape as _};
        use buzz_scene::{ShapeData, SymbolKind};
        use kurbo::Rect as KRect;

        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        let symbol = scene.add_symbol("Hero", SymbolKind::Graphic, None);
        let instance = scene
            .add_instance_at(layer, 0, symbol, Affine::IDENTITY)
            .unwrap();
        let shape = scene
            .add_shape(
                layer,
                ShapeData::filled(KRect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
            )
            .unwrap();

        let mut selection = Selection::new();
        selection.select_one(instance);
        assert_eq!(
            single_selected_instance(&scene, &selection),
            Some(instance),
            "one instance selected"
        );

        selection.select_one(shape);
        assert_eq!(
            single_selected_instance(&scene, &selection),
            None,
            "a shape is not an instance"
        );

        selection.set([instance, shape]);
        assert_eq!(
            single_selected_instance(&scene, &selection),
            None,
            "a mixed selection has no single instance to describe"
        );
    }

    /// A dangling instance must render an explanation rather than panic —
    /// deleting a symbol that is still placed is a thing users do.
    #[test]
    fn the_instance_section_survives_a_deleted_symbol() {
        use buzz_geom::Affine;
        use buzz_scene::SymbolKind;

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        let symbol = scene.add_symbol("Gone", SymbolKind::Graphic, None);
        let instance = scene
            .add_instance_at(layer, 0, symbol, Affine::IDENTITY)
            .unwrap();
        scene.library_mut().remove(symbol);

        let mut selection = Selection::new();
        selection.select_one(instance);
        let mut style = DrawStyle::default();
        let mut view = ViewSettings::default();

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = properties_panel(ui, &mut scene, &selection, &mut style, &mut view);
        });
    }

    #[test]
    fn panels_render_with_a_selection_and_several_layers() {
        use buzz_geom::Shape as _;
        use buzz_scene::ShapeData;
        use kurbo::Rect as KRect;

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        let folder = scene.add_layer("Folder", LayerKind::Folder);
        let mask = scene.add_layer("Mask", LayerKind::Mask);
        scene.update_layer(mask, |l| l.parent = Some(folder));
        let layer = scene.layers().iter().next().unwrap().id;
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(KRect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
            )
            .unwrap();

        let mut selection = Selection::new();
        selection.select_one(id);
        selection.set_active_layer(Some(layer));

        let mut style = DrawStyle::default();
        let mut view = ViewSettings::default();

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = menu_bar(ui, &scene, &selection, &view, true, true);
            let _ = properties_panel(ui, &mut scene, &selection, &mut style, &mut view);
            let _ = layers_panel(ui, &mut scene, &mut selection);
        });
    }
}
