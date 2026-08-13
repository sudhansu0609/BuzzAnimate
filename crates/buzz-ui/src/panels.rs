//! Panel rendering.
//!
//! Panels take state by reference and return the [`Command`]s the user raised.
//! Nothing here owns editor state, so the same panel can be driven by tests or
//! by the running application.

use buzz_scene::{
    EditAt, Gradient, GradientKind, GradientSpread, GradientStop, LayerId, LayerKind, Scene,
};
use egui::{Color32, RichText, Ui};
use peniko::Color;

use crate::command::{Command, shortcut_text};
use crate::selection::Selection;
use crate::style::{DrawStyle, DrawingMode, FillKind, StrokeKind};
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

/// Everything the menus need to know to draw themselves.
///
/// A menu bar has to reflect the state of the whole editor — what can be
/// undone, which panels are open, whether the light handles are showing — and
/// that is a long argument list by nature. Gathering it here means adding a
/// tick to a menu does not change the shape of every call site.
pub struct MenuState<'a> {
    pub scene: &'a Scene,
    pub selection: &'a Selection,
    pub view: &'a ViewSettings,
    pub can_undo: bool,
    pub can_redo: bool,
    /// The layout, for the Window menu's ticks.
    pub workspace: &'a crate::workspace::Workspace,
    /// Whether the on-stage light handles are shown. Not read off
    /// `ViewSettings` because it belongs to the Lighting panel's own state.
    pub light_gizmos: bool,
}

/// Draw the menu bar. Returns whatever the user chose.
pub fn menu_bar(ui: &mut Ui, state: &MenuState<'_>) -> Vec<Command> {
    let MenuState {
        scene,
        selection,
        view,
        can_undo,
        can_redo,
        workspace,
        light_gizmos,
    } = *state;
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
            item(ui, Command::ImportSound, true, &mut raised);
            // Animate keeps Export in a submenu of File, one entry per output.
            ui.menu_button("Export", |ui| {
                for c in [Command::ExportImage, Command::ExportSequence] {
                    item(ui, c, true, &mut raised);
                }
            });
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
                let button =
                    egui::Button::new(format!("{mark}{}", command.label())).shortcut_text(shortcut);
                if ui.add(button).clicked() {
                    out.push(command);
                    ui.close();
                }
            };
            toggle(ui, Command::ToggleRulers, view.show_rulers, &mut raised);
            toggle(ui, Command::ToggleGrid, view.show_grid, &mut raised);
            toggle(ui, Command::ToggleGuides, view.show_guides, &mut raised);
            toggle(
                ui,
                Command::ToggleSnapping,
                view.snap.to_objects,
                &mut raised,
            );
            toggle(
                ui,
                Command::TogglePasteboard,
                view.show_pasteboard,
                &mut raised,
            );
            toggle(ui, Command::ToggleLightGizmos, light_gizmos, &mut raised);
        });

        // Animate's Window menu: what is on screen, and where.
        ui.menu_button("Window", |ui| {
            for id in crate::workspace::PanelId::ALL {
                let mark = if workspace.is_open(id) { "\u{2714} " } else { "   " };
                if ui.button(format!("{mark}{}", id.title())).clicked() {
                    raised.push(Command::TogglePanel(id));
                    ui.close();
                }
            }
            ui.separator();

            // The interface theme. Animate keeps this in Preferences; there
            // is no Preferences dialog here, and the Window menu is where the
            // rest of the chrome's own settings already live.
            let lit = crate::theme::theme() == crate::theme::Theme::Light;
            let mark = if lit { "\u{2714} " } else { "   " };
            if ui
                .button(format!("{mark}{}", Command::ToggleTheme.label()))
                .clicked()
            {
                raised.push(Command::ToggleTheme);
                ui.close();
            }
            ui.separator();

            let mark = if workspace.locked { "\u{2714} " } else { "   " };
            let shortcut = shortcut_text(ui.ctx(), Command::ToggleLayoutLock);
            if ui
                .add(
                    egui::Button::new(format!("{mark}Lock Layout")).shortcut_text(shortcut),
                )
                .on_hover_text("Stop panels being dragged, resized or moved")
                .clicked()
            {
                raised.push(Command::ToggleLayoutLock);
                ui.close();
            }
            if ui
                .button("Reset Layout")
                .on_hover_text("Put every panel back where it started")
                .clicked()
            {
                raised.push(Command::ResetWorkspace);
                ui.close();
            }
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

            // Blender puts lights under Add; Animate has no lighting at all,
            // so Insert is where a user of either would look first.
            ui.separator();
            ui.menu_button("Light", |ui| {
                for c in [Command::AddSun, Command::AddSky, Command::AddLamp] {
                    item(ui, c, true, &mut raised);
                }
            });
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
            // Animate keeps the frame clipboard with the frame commands.
            for c in [
                Command::CutFrames,
                Command::CopyFrames,
                Command::PasteFrames,
                Command::ClearFrames,
                Command::ReverseFrames,
            ] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            item(ui, Command::ToggleOnionSkin, true, &mut raised);
            item(ui, Command::ToggleAutoKeyframe, true, &mut raised);
            item(ui, Command::ToggleEditMultipleFrames, true, &mut raised);
            ui.separator();
            // Animate keeps sound on the frame it starts from, so its commands
            // sit with the other frame commands.
            item(ui, Command::AttachSound, true, &mut raised);
            item(ui, Command::RemoveSound, true, &mut raised);
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

        // Animate's Commands menu holds saved JSFL commands. Ours runs what is
        // in the Actions panel, which is the same idea before there is anywhere
        // to save one.
        ui.menu_button("Commands", |ui| {
            item(ui, Command::LipSync, true, &mut raised);
            item(ui, Command::NewMouthSymbol, true, &mut raised);
            ui.separator();
            item(ui, Command::ToggleActionsPanel, true, &mut raised);
            ui.separator();
            item(ui, Command::RunScript, true, &mut raised);
            item(ui, Command::ClearScriptOutput, true, &mut raised);
            ui.separator();
            ui.label(
                RichText::new("Write scripts in the Actions panel")
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
            item(ui, Command::BrushFromSelection, has_selection, &mut raised);
            ui.separator();
            ui.label(RichText::new("Shape").small().weak());
            for c in [
                Command::ConvertLinesToFills,
                Command::ExpandFill,
                Command::SmoothSelection,
                Command::StraightenSelection,
                Command::RecogniseShape,
            ] {
                item(ui, c, has_selection, &mut raised);
            }

            // Animate keeps these in a Transform submenu of Modify.
            ui.separator();
            ui.menu_button("Transform", |ui| {
                for c in [
                    Command::FlipHorizontal,
                    Command::FlipVertical,
                    Command::RotateClockwise,
                    Command::RotateAnticlockwise,
                ] {
                    item(ui, c, has_selection, &mut raised);
                }
            });
        });

        // Last, as it is everywhere.
        ui.menu_button("Help", |ui| {
            item(ui, Command::About, true, &mut raised);
        });
    });

    raised
}

/// The vertical tool strip.
///
/// **Scrolls.** The strip is as tall as the window leaves it, and opening the
/// Actions panel or a tall timeline takes that height away — which silently
/// cut off everything below the Brush, including the Bone and Asset Warp
/// tools. A tool you cannot reach is worse than one that is greyed out with a
/// reason, and a screenshot was the only way that was ever going to be seen.
pub fn tool_bar(ui: &mut Ui, active: ToolId, style: &mut DrawStyle) -> Option<ToolId> {
    let mut chosen = None;

    egui::ScrollArea::vertical()
        .id_salt("tool-strip")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // **As many columns as it has been given room for.** One column
            // is Animate's strip and what the default width holds; widen the
            // dock and the tools flow into two or three rather than leaving a
            // column of empty space beside them.
            let spacing = ui.spacing().item_spacing.x;
            let per_row = ((ui.available_width() + spacing)
                / (Metrics::TOOL_BUTTON + spacing))
                .floor()
                .clamp(1.0, 8.0) as usize;

            ui.vertical_centered(|ui| {
                for (index, group) in TOOL_GROUPS.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(2.0);
                        ui.separator();
                        ui.add_space(2.0);
                    }
                    for row in group.chunks(per_row) {
                        ui.horizontal(|ui| {
                            for tool in row.iter().copied() {
                                if tool_button(ui, tool, tool == active) {
                                    chosen = Some(tool);
                                }
                            }
                        });
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                color_wells(ui, style);
            });
        });

    chosen
}

fn tool_button(ui: &mut Ui, tool: ToolId, active: bool) -> bool {
    let size = egui::vec2(Metrics::TOOL_BUTTON, Metrics::TOOL_BUTTON);
    let ready = tool.is_ready();

    // An empty button, with the symbol painted into it afterwards. The symbol
    // is drawn geometry rather than a character — see [`crate::icons`] — so it
    // cannot come out as an empty box the way a missing glyph does.
    let button = egui::Button::new("").min_size(size).fill(if active {
        Palette::active()
    } else {
        Palette::raised()
    });

    let response = ui.add_enabled(ready, button);

    // Bright when it is the tool in hand or under the pointer, dim otherwise —
    // so the strip reads as one active tool rather than twenty-two equals.
    let ink = if ready && (active || response.hovered()) {
        Palette::text()
    } else {
        Palette::text_dim()
    };
    // Inset, so the symbol does not touch the button's edge.
    crate::icons::tool_icon(ui.painter(), response.rect.shrink(6.0), tool, ink);

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
            well(
                ui,
                "Stroke",
                &mut style.stroke_color,
                &mut style.stroke_enabled,
            );
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
fn instance_properties(
    ui: &mut Ui,
    scene: &mut Scene,
    id: buzz_scene::ObjectId,
    at: EditAt,
) -> bool {
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
    let mut new_symbol = instance.symbol;
    let mut edited = false;

    // Every other symbol this instance could point at instead.
    let others: Vec<(buzz_scene::SymbolId, String)> = scene
        .library()
        .iter()
        .filter(|s| s.id != instance.symbol)
        .map(|s| (s.id, s.name.clone()))
        .collect();

    ui.add_space(8.0);
    ui.label(RichText::new("Symbol Instance").strong());

    egui::Grid::new("instance-props")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Symbol");
            ui.horizontal(|ui| {
                ui.label(RichText::new(&symbol_name).strong())
                    .on_hover_text(symbol_kind.label());

                // Animate's **Swap**: point this instance at a different
                // symbol and keep everything else — where it is, how it is
                // transformed, its colour effect, its looping. Replacing the
                // instance instead would lose all of that, which is exactly
                // what the button exists to avoid.
                ui.menu_button("Swap\u{2026}", |ui| {
                    if others.is_empty() {
                        ui.label(RichText::new("no other symbols").small().weak());
                    }
                    for (id, name) in &others {
                        if ui.button(name).clicked() {
                            new_symbol = *id;
                            edited = true;
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Point this instance at a different symbol");
            });
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
                        let selected =
                            std::mem::discriminant(&option) == std::mem::discriminant(&effect);
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
                    ui.label(RichText::new("Set by a tween or an import").small().weak())
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
        scene.update_object_where(at, id, |o| {
            if let buzz_scene::ObjectKind::Instance(i) = &mut o.kind {
                i.symbol = new_symbol;
                i.first_frame = new_first_frame;
                i.loop_mode = new_loop;
                i.color = new_color;
            }
        });

        // A swap can leave the instance pointing past the end of its new
        // symbol, which would show nothing at all. Pull it back rather than
        // letting the instance go blank.
        let length = scene
            .library()
            .get(new_symbol)
            .map(|s| s.length())
            .unwrap_or(1);
        if new_first_frame >= length {
            scene.update_object_where(at, id, |o| {
                if let buzz_scene::ObjectKind::Instance(i) = &mut o.kind {
                    i.first_frame = length.saturating_sub(1);
                }
            });
        }
    }
    edited
}

/// Brush settings, with a live preview of the pattern being stamped.
///
/// Animate puts these in the tool options strip under the toolbar. They are
/// here in Properties because that is where this application's contextual
/// settings already live, and splitting them across two places would be worse
/// than the deviation.
fn brush_properties(ui: &mut Ui, style: &mut DrawStyle) {
    use crate::brush::{BrushKind, PatternShape};

    ui.add_space(8.0);
    ui.label(RichText::new("Brush").strong());

    egui::Grid::new("brush-props").num_columns(2).show(ui, |ui| {
        ui.label("Type");
        egui::ComboBox::from_id_salt("brush-kind")
            .selected_text(style.brush.kind.label())
            .show_ui(ui, |ui| {
                for kind in [BrushKind::Fluid, BrushKind::Pattern, BrushKind::Art] {
                    ui.selectable_value(&mut style.brush.kind, kind, kind.label())
                        .on_hover_text(kind.description());
                }
            });
        ui.end_row();

        ui.label("Size");
        ui.add(
            egui::Slider::new(&mut style.brush.size, 1.0..=200.0)
                .logarithmic(true)
                .suffix(" px"),
        );
        ui.end_row();

        ui.label("Smoothing");
        ui.add(egui::Slider::new(&mut style.brush.smoothing, 0.0..=1.0))
            .on_hover_text("Steadies a shaky hand. The ends never move.");
        ui.end_row();

        ui.label("Build up");
        ui.checkbox(&mut style.brush.build_up, "")
            .on_hover_text(
                "Opacity adds where strokes overlap: 20% crossing 30% gives                  50%, not 44%. Paint deepens as you work over it, the way ink                  does.",
            );
        ui.end_row();

        if style.brush.kind == BrushKind::Fluid {
            ui.label("Thinnest");
            ui.add(egui::Slider::new(&mut style.brush.min_ratio, 0.0..=1.0))
                .on_hover_text("How thin the stroke gets at full speed or lightest pressure");
            ui.end_row();

            ui.label("Taper");
            ui.add(egui::Slider::new(&mut style.brush.taper, 0.0..=0.5))
                .on_hover_text("How much of each end narrows to a point");
            ui.end_row();

            ui.label("Pressure");
            ui.checkbox(&mut style.brush.use_pressure, "")
                .on_hover_text(
                    "Follow pen pressure instead of speed. A mouse reports no \
                     pressure, so with this on a mouse paints a constant width.",
                );
            ui.end_row();

            if !style.brush.use_pressure {
                ui.label("Fastest at");
                ui.add(
                    egui::Slider::new(&mut style.brush.reference_speed, 100.0..=4000.0)
                        .suffix(" px/s"),
                )
                .on_hover_text("The speed at which the stroke reaches its thinnest");
                ui.end_row();
            }
        }

        if style.brush.kind.uses_pattern() {
            ui.label("Shape");
            egui::ComboBox::from_id_salt("brush-pattern")
                .selected_text(style.brush.pattern.label())
                .show_ui(ui, |ui| {
                    for shape in PatternShape::BUILT_IN {
                        ui.selectable_value(&mut style.brush.pattern, shape, shape.label());
                    }
                    // Only offered once there is something to offer.
                    if style.brush.custom_pattern.is_some() {
                        ui.selectable_value(
                            &mut style.brush.pattern,
                            PatternShape::Custom,
                            PatternShape::Custom.label(),
                        );
                    }
                });
            ui.end_row();

            if style.brush.kind == BrushKind::Pattern {
                ui.label("Spacing");
                ui.add(
                    egui::Slider::new(&mut style.brush.spacing, 0.5..=200.0)
                        .logarithmic(true)
                        .suffix(" px"),
                )
                .on_hover_text(
                    "Distance between stamps. Very close spacing on a long \
                     stroke is widened automatically so the drawing stays \
                     responsive.",
                );
                ui.end_row();
            }
        }
    });

    if style.brush.kind.uses_pattern() {
        brush_pattern_preview(ui, style);
    }
}

/// Draw the pattern as it will be stamped, along a short sample stroke.
///
/// Worth the few lines: the difference between spacing 4 and spacing 40 is
/// obvious in a picture and almost meaningless as a number.
fn brush_pattern_preview(ui: &mut Ui, style: &DrawStyle) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().min(220.0), 46.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Palette::panel());

    let Some(source) = style.brush.pattern_path() else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No shape yet",
            egui::FontId::proportional(11.0),
            Palette::text_dim(),
        );
        return;
    };

    // A gentle S, so the rotation of the stamps is visible.
    let span = rect.width() as f64 - 16.0;
    let spine = buzz_geom::catmull_rom(&[
        buzz_geom::Point::new(0.0, 6.0),
        buzz_geom::Point::new(span * 0.33, -6.0),
        buzz_geom::Point::new(span * 0.66, 6.0),
        buzz_geom::Point::new(span, -6.0),
    ]);

    // The preview budget, because this is drawn every frame the panel is open.
    let stamped = buzz_geom::stamp_along(
        &spine,
        &source,
        style.brush.fit(),
        &buzz_geom::BrushBudget::preview(),
    );

    // Fit whatever came out into the strip.
    let bounds = buzz_geom::Shape::bounding_box(&stamped.path);
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return;
    }
    let scale = ((rect.width() as f64 - 12.0) / bounds.width())
        .min((rect.height() as f64 - 12.0) / bounds.height())
        .min(1.0);

    // The brush preview is chrome drawn with egui's painter, which has no
    // gradient brush — one colour standing in for the ramp is enough to judge
    // spacing and stamp size, which is what this strip is for.
    let colour = to_egui(if style.fill_enabled {
        style.fill_color_for_preview()
    } else {
        Color::BLACK
    });
    let to_screen = |p: buzz_geom::Point| -> egui::Pos2 {
        egui::pos2(
            rect.center().x + ((p.x - bounds.center().x) * scale) as f32,
            rect.center().y + ((p.y - bounds.center().y) * scale) as f32,
        )
    };

    // Each subpath is one stamp, so they are drawn separately rather than
    // joined into a single polyline that would connect them all together.
    let mut current: Vec<egui::Pos2> = Vec::new();
    let flush = |points: &mut Vec<egui::Pos2>| {
        if points.len() >= 3 {
            painter.add(egui::Shape::convex_polygon(
                std::mem::take(points),
                colour,
                egui::Stroke::NONE,
            ));
        } else {
            points.clear();
        }
    };
    kurbo::flatten(
        stamped.path.iter(),
        0.2 / scale.max(1e-6),
        |element| match element {
            kurbo::PathEl::MoveTo(p) => {
                flush(&mut current);
                current.push(to_screen(p));
            }
            kurbo::PathEl::LineTo(p) => current.push(to_screen(p)),
            kurbo::PathEl::ClosePath => flush(&mut current),
            _ => {}
        },
    );
    flush(&mut current);
}

/// Contextual properties for the current selection.
pub fn properties_panel(
    ui: &mut Ui,
    scene: &mut Scene,
    selection: &Selection,
    style: &mut DrawStyle,
    view: &mut ViewSettings,
    at: EditAt,
) -> bool {
    let mut changed = false;

    ui.heading(selection.describe(scene));
    ui.separator();

    if selection.is_empty() {
        ui.label(RichText::new("Document").strong());
        egui::Grid::new("doc-props").num_columns(2).show(ui, |ui| {
            ui.label("Width");
            let mut w = scene.stage().size.width;
            if ui
                .add(egui::DragValue::new(&mut w).range(1.0..=16384.0))
                .changed()
            {
                scene.stage_mut().size.width = w;
                changed = true;
            }
            ui.end_row();

            ui.label("Height");
            let mut h = scene.stage().size.height;
            if ui
                .add(egui::DragValue::new(&mut h).range(1.0..=16384.0))
                .changed()
            {
                scene.stage_mut().size.height = h;
                changed = true;
            }
            ui.end_row();

            ui.label("FPS");
            let mut fps = scene.stage().frame_rate;
            if ui
                .add(
                    egui::DragValue::new(&mut fps)
                        .range(0.01..=240.0)
                        .speed(0.1),
                )
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
        changed |= instance_properties(ui, scene, id, at);
    }

    // Which way the selection faces in space. Animate keeps this for movie
    // clips; here any object can have it.
    if let Some(id) = selection.iter().next() {
        changed |= spatial_properties(ui, scene, id, at);
    }

    brush_properties(ui, style);

    ui.add_space(8.0);
    ui.label(RichText::new("Stroke and Fill").strong());
    egui::Grid::new("style-props")
        .num_columns(2)
        .show(ui, |ui| {
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

/// The document's palette, and the colours most recently used.
///
/// **Two rows, because they answer different questions.** The palette is what
/// this production's colours *are*: named, saved with the file, edited in the
/// Swatches panel. Recent colours are what this session has been using, which
/// is worth a row while sketching and worthless tomorrow. Animate has only the
/// first; the second was here before the palette existed and earns its keep.
pub fn color_panel(ui: &mut Ui, scene: &Scene, style: &mut DrawStyle) {
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
        if let Some(swatch) = scene.swatches().find_color(style.stroke_color) {
            ui.label(RichText::new(&swatch.name).small().weak());
        }
    });
    ui.horizontal(|ui| {
        ui.label("Fill");
        egui::ComboBox::from_id_salt("fill kind")
            .selected_text(style.fill_kind.label())
            .width(140.0)
            .show_ui(ui, |ui| {
                for kind in [FillKind::Solid, FillKind::Linear, FillKind::Radial] {
                    ui.selectable_value(&mut style.fill_kind, kind, kind.label());
                }
            });
    });

    if style.fill_kind == FillKind::Solid {
        ui.horizontal(|ui| {
            ui.add_space(38.0);
            let mut c = to_egui(style.fill_color);
            if ui.color_edit_button_srgba(&mut c).changed() {
                style.fill_color = from_egui(c);
                style.fill_enabled = true;
                let remembered = style.fill_color;
                style.remember(remembered);
            }
            if let Some(swatch) = scene.swatches().find_color(style.fill_color) {
                ui.label(RichText::new(&swatch.name).small().weak());
            }
        });
    } else {
        gradient_editor(ui, &mut style.fill_gradient);
        style.fill_enabled = true;
    }

    let mut picked: Option<(Color, bool)> = None;

    ui.add_space(6.0);
    ui.label(RichText::new("Swatches").small().weak());
    let palette: Vec<(String, Color)> = scene
        .swatches()
        .iter()
        .map(|s| (s.path(), s.color))
        .collect();
    ui.horizontal_wrapped(|ui| {
        if palette.is_empty() {
            ui.label(
                RichText::new("none \u{2014} add colours in the Swatches panel")
                    .small()
                    .weak()
                    .italics(),
            );
        }
        for (name, color) in &palette {
            if swatch_chip(ui, *color, name) {
                picked = Some((*color, ui.input(|i| i.modifiers.shift)));
            }
        }
    });

    ui.add_space(4.0);
    ui.label(RichText::new("Recent").small().weak());
    let recent = style.swatches.clone();
    ui.horizontal_wrapped(|ui| {
        for color in &recent {
            if swatch_chip(ui, *color, "Recently used") {
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

/// Edit a gradient: the ramp, its stops, and how it behaves past the ends.
///
/// The ramp is drawn as the ramp, not described in numbers, for the reason the
/// brush preview strip exists: the difference between two stop layouts is
/// obvious as a picture and nearly meaningless as a list of offsets.
///
/// Returns whether anything changed.
pub fn gradient_editor(ui: &mut Ui, gradient: &mut Gradient) -> bool {
    let mut changed = false;

    // The ramp. egui has no gradient brush, so it is drawn as a column of thin
    // filled rectangles — one per pixel of width, sampled from the model. That
    // is the same `sample` the renderer's stops come from, so what is shown
    // here and what lands on the stage cannot disagree about the colours.
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
    let steps = (rect.width().round() as usize).clamp(1, 512);
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left() + rect.width() * t0, rect.top()),
            egui::pos2(rect.left() + rect.width() * t1, rect.bottom()),
        );
        ui.painter()
            .rect_filled(band, 0.0, to_egui(gradient.sample(f64::from(t0))));
    }
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, Palette::border()),
        egui::StrokeKind::Inside,
    );

    // Clicking the ramp adds a stop there, in the colour the ramp already has
    // at that point — so a new stop never changes the picture until it is
    // dragged or recoloured. Animate's gradient bar behaves the same way.
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let t = f64::from((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        let mut stops = gradient.stops().to_vec();
        if stops.len() < buzz_scene::MAX_STOPS {
            stops.push(GradientStop::new(t, gradient.sample(t)));
            gradient.set_stops(stops);
            changed = true;
        }
    }

    ui.add_space(2.0);

    // The stops, as rows. A row per stop rather than draggable pips on the bar
    // because a colour needs a picker beside it, and a picker cannot live on a
    // 24-pixel strip.
    let mut stops = gradient.stops().to_vec();
    let mut remove: Option<usize> = None;
    let count = stops.len();
    for (i, stop) in stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let mut c = to_egui(stop.color);
            if ui.color_edit_button_srgba(&mut c).changed() {
                stop.color = from_egui(c);
                changed = true;
            }
            if ui
                .add(
                    egui::DragValue::new(&mut stop.offset)
                        .speed(0.005)
                        .range(0.0..=1.0)
                        .fixed_decimals(3),
                )
                .changed()
            {
                changed = true;
            }
            // Two stops are the fewest a ramp can have; removing below that
            // would leave a gradient that is a colour, and the model would
            // silently pad it back.
            if count > 2 && ui.small_button("\u{1F5D1}").on_hover_text("Remove stop").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        stops.remove(i);
        changed = true;
    }
    if changed {
        gradient.set_stops(stops);
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new("Overflow").small().weak());
        egui::ComboBox::from_id_salt("gradient spread")
            .selected_text(gradient.spread.label())
            .width(100.0)
            .show_ui(ui, |ui| {
                for s in [
                    GradientSpread::Pad,
                    GradientSpread::Reflect,
                    GradientSpread::Repeat,
                ] {
                    if ui
                        .selectable_value(&mut gradient.spread, s, s.label())
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
    });

    if gradient.kind == GradientKind::Radial {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Focal").small().weak());
            if ui
                .add(
                    egui::Slider::new(&mut gradient.focal, -1.0..=1.0)
                        .fixed_decimals(2)
                        .show_value(true),
                )
                .on_hover_text("Where the hot spot sits along the ramp's own axis")
                .changed()
            {
                changed = true;
            }
        });
    }

    changed
}

/// One colour square. Returns whether it was clicked.
fn swatch_chip(ui: &mut Ui, color: Color, name: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
    ui.painter().rect_filled(rect, 2.0, to_egui(color));
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, Palette::border()),
        egui::StrokeKind::Inside,
    );
    response
        .on_hover_text(format!(
            "{name}\nClick sets fill \u{b7} Shift-click sets stroke"
        ))
        .clicked()
}

/// The layer list. Returns a command if the user asked for one.
pub fn layers_panel(ui: &mut Ui, scene: &mut Scene, selection: &mut Selection) -> Option<Command> {
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
    // Name and follow link for every layer, read before the rows below borrow
    // the scene mutably.
    let names: Vec<(LayerId, String, Option<LayerId>)> = scene
        .layers()
        .iter()
        .map(|l| (l.id, l.name.clone(), l.follows))
        .collect();
    // Which links would close a loop, worked out while the stack is still
    // borrowed: a layer must not be offered a parent that follows it.
    let allowed: Vec<(LayerId, Vec<LayerId>)> = ids
        .iter()
        .map(|id| {
            (
                *id,
                names
                    .iter()
                    .map(|(other, _, _)| *other)
                    .filter(|other| scene.layers().can_follow(*id, *other))
                    .collect(),
            )
        })
        .collect();
    let mut set_follows: Option<(LayerId, Option<LayerId>)> = None;
    let mut set_kind: Option<(LayerId, LayerKind)> = None;

    egui::ScrollArea::vertical()
        .id_salt("layer-list")
        .show(ui, |ui| {
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
                        .selectable_label(set_outline, "O")
                        .on_hover_text("Show as outlines")
                        .clicked()
                    {
                        set_outline = !set_outline;
                    }

                    // The layer's colour chip, used for outline view.
                    let (chip, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_filled(chip, 1.0, to_egui(color));

                    let mark = match kind {
                        LayerKind::Folder => "F ",
                        LayerKind::Mask => "M ",
                        // The mask that hides rather than reveals. Marked with
                        // a letter rather than a struck-through M: a combining
                        // stroke is one of the glyphs the interface font does
                        // not have, and would draw as a box.
                        LayerKind::InverseMask => "iM ",
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

                // Animate's Parent column: which layer this one follows, so
                // moving that layer's artwork moves this layer's with it.
                let follows = names
                    .iter()
                    .find(|(other, _, _)| *other == id)
                    .and_then(|(_, _, f)| *f);
                let choices = allowed
                    .iter()
                    .find(|(other, _)| *other == id)
                    .map(|(_, list)| list.clone())
                    .unwrap_or_default();

                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 12.0 + 18.0);
                    ui.label(RichText::new("follows").small().weak());

                    let label = match follows.and_then(|f| {
                        names.iter().find(|(other, _, _)| *other == f)
                    }) {
                        Some((_, name, _)) => name.clone(),
                        // An em dash reads as "nothing" without needing a word
                        // for it, and this line is repeated on every layer.
                        None => "\u{2014}".to_string(),
                    };

                    egui::ComboBox::from_id_salt(("follows", id.0))
                        .selected_text(RichText::new(label).small())
                        .width(96.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(follows.is_none(), "\u{2014}  none")
                                .clicked()
                            {
                                set_follows = Some((id, None));
                            }
                            for other in &choices {
                                let Some((_, name, _)) =
                                    names.iter().find(|(o, _, _)| o == other)
                                else {
                                    continue;
                                };
                                if ui
                                    .selectable_label(follows == Some(*other), name)
                                    .clicked()
                                {
                                    set_follows = Some((id, Some(*other)));
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "Layer Parenting: this layer's artwork moves with the \
                             layer it follows",
                        );

                    // **What kind of layer this is.** Masking is positional —
                    // a mask claims the run of masked layers directly below it
                    // — so this is the only control it needs: set one layer to
                    // Mask, the ones under it to Masked, and the stack does the
                    // rest. Folders are not offered: a folder holds layers, and
                    // turning one into a drawing layer would orphan them.
                    if kind != LayerKind::Folder {
                        egui::ComboBox::from_id_salt(("layer-kind", id.0))
                            .selected_text(RichText::new(kind.display_name()).small())
                            .width(92.0)
                            .show_ui(ui, |ui| {
                                for choice in [
                                    LayerKind::Normal,
                                    LayerKind::Mask,
                                    LayerKind::InverseMask,
                                    LayerKind::Masked,
                                    LayerKind::Guide,
                                    LayerKind::Guided,
                                ] {
                                    if ui
                                        .selectable_label(kind == choice, choice.display_name())
                                        .on_hover_text(layer_kind_hint(choice))
                                        .clicked()
                                    {
                                        set_kind = Some((id, choice));
                                    }
                                }
                            })
                            .response
                            .on_hover_text(layer_kind_hint(kind));
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

    if let Some((layer, follows)) = set_follows {
        scene.update_layer(layer, |l| l.follows = follows);
    }
    if let Some((layer, kind)) = set_kind {
        scene.update_layer(layer, |l| l.kind = kind);
    }

    command
}

/// What each layer type does, in one line.
///
/// Written out because five of the six are only meaningful in relation to the
/// layers around them, and a list of bare names says nothing about that.
fn layer_kind_hint(kind: LayerKind) -> &'static str {
    match kind {
        LayerKind::Normal => "An ordinary drawing layer",
        LayerKind::Mask => {
            "Its artwork is a stencil: the masked layers below show only where it covers them"
        }
        LayerKind::InverseMask => {
            "The stencil, inverted: the masked layers below are hidden where it covers them, \
             and show everywhere else"
        }
        LayerKind::Masked => "Clipped by the mask layer above",
        LayerKind::Guide => "Reference artwork \u{2014} visible here, never in the film",
        LayerKind::Guided => "Follows the motion guide on the guide layer above",
        LayerKind::Folder => "Holds other layers, and draws nothing itself",
    }
}

/// Animate's 3D Rotation and 3D Translation, for the selected object.
///
/// # What it is for
///
/// A flat drawing turned in space is still flat — but a *few* flat drawings at
/// different angles read as one solid thing when the camera moves past them.
/// Three cards for a tree, four walls for a house, a figure built of a body
/// card and two arm cards: turn each a little and a camera move discovers the
/// shape instead of sliding it.
fn spatial_properties(
    ui: &mut Ui,
    scene: &mut Scene,
    id: buzz_scene::ObjectId,
    at: EditAt,
) -> bool {
    let Some((_, object)) = scene.find_object(id) else {
        return false;
    };
    let mut spatial = object.spatial;
    let before = spatial;

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("3D").strong());
        if !spatial.is_flat() {
            ui.label(RichText::new("turned").small().weak());
        }
    });

    // Bounded a little short of edge-on. A card exactly side-on has no image
    // at all, and one past it shows its back — legal, and rarely what somebody
    // reaching for a slider meant.
    const LIMIT: f64 = 85.0;
    let angle = |ui: &mut Ui, label: &str, value: &mut f64, hint: &str| {
        let mut degrees = value.to_degrees();
        let mut touched = false;
        ui.horizontal(|ui| {
            ui.label(label);
            if ui
                .add(
                    egui::Slider::new(&mut degrees, -LIMIT..=LIMIT)
                        .suffix("\u{b0}")
                        .fixed_decimals(0),
                )
                .on_hover_text(hint)
                .changed()
            {
                *value = degrees.to_radians();
                touched = true;
            }
        });
        touched
    };

    let mut changed = false;
    changed |= angle(
        ui,
        "Rotate X",
        &mut spatial.rotation_x,
        "Tip the object away from the viewer, about the horizontal",
    );
    changed |= angle(
        ui,
        "Rotate Y",
        &mut spatial.rotation_y,
        "Turn the object about the vertical — the one that makes a card read \
         as a wall when the camera passes it",
    );
    changed |= angle(
        ui,
        "Rotate Z",
        &mut spatial.rotation_z,
        "Spin it in its own plane",
    );

    ui.horizontal(|ui| {
        ui.label("Z");
        if ui
            .add(
                egui::Slider::new(&mut spatial.z, -800.0..=800.0)
                    .suffix(" px")
                    .fixed_decimals(0),
            )
            .on_hover_text(
                "How far in front of or behind its layer the object sits. \
                 Negative is towards the camera.",
            )
            .changed()
        {
            changed = true;
        }
    });

    if !spatial.is_flat() && ui.small_button("Flatten").clicked() {
        spatial = buzz_scene::Spatial::default();
        changed = true;
    }

    if changed && spatial != before {
        scene.update_object_where(at, id, |o| o.spatial = spatial);
        return true;
    }
    false
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
            let workspace = crate::workspace::Workspace::animate();
            let _ = menu_bar(
                ui,
                &MenuState {
                    scene: &scene,
                    selection: &selection,
                    view: &view,
                    can_undo: false,
                    can_redo: false,
                    workspace: &workspace,
                    light_gizmos: true,
                },
            );
            let _ = tool_bar(ui, ToolId::Selection, &mut style);
            let _ = properties_panel(
                ui,
                &mut scene,
                &selection,
                &mut style,
                &mut view,
                EditAt::exact(0),
            );
            color_panel(ui, &scene, &mut style);
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
            let _ = properties_panel(
                ui,
                &mut scene,
                &selection,
                &mut style,
                &mut view,
                EditAt::exact(0),
            );
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
            let workspace = crate::workspace::Workspace::animate();
            let _ = menu_bar(
                ui,
                &MenuState {
                    scene: &scene,
                    selection: &selection,
                    view: &view,
                    can_undo: true,
                    can_redo: true,
                    workspace: &workspace,
                    light_gizmos: false,
                },
            );
            let _ = properties_panel(
                ui,
                &mut scene,
                &selection,
                &mut style,
                &mut view,
                EditAt::exact(0),
            );
            let _ = layers_panel(ui, &mut scene, &mut selection);
        });
    }
}
