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
    /// The saved templates, by name, for the File menu to list. Names rather
    /// than paths: a menu shows names, and the editor holds the list this
    /// indexes into.
    pub templates: &'a [String],
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
        templates,
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

            // **Start from a stage you set up once.**
            //
            // Every film began empty: background, camera, lights and the cast
            // laid out again by hand. A template is a whole document kept
            // aside, so starting from one carries all of it.
            ui.menu_button("New from Template", |ui| {
                if templates.is_empty() {
                    ui.label(
                        RichText::new(
                            "No templates yet.\nSet a stage up, then File \u{25b8} Save as Template.",
                        )
                        .small()
                        .weak(),
                    );
                    return;
                }
                for (index, name) in templates.iter().enumerate() {
                    if ui.button(name.as_str()).clicked() {
                        raised.push(Command::NewFromTemplate(index));
                        ui.close();
                    }
                }
            });
            item(ui, Command::SaveAsTemplate, true, &mut raised);
            ui.separator();
            for c in [Command::ImportToStage, Command::ImportToLibrary] {
                item(ui, c, true, &mut raised);
            }
            item(ui, Command::ImportImage, true, &mut raised);
            item(ui, Command::ImportSound, true, &mut raised);
            // Animate keeps Export in a submenu of File, one entry per output.
            ui.menu_button("Export", |ui| {
                for c in [
                    // First, because it is the one that carries the *document*
                    // rather than a picture of it.
                    Command::ExportFla,
                    Command::ExportImage,
                    Command::ExportSequence,
                    Command::ExportVideo,
                    Command::ExportGif,
                    Command::ExportWebp,
                ] {
                    item(ui, c, true, &mut raised);
                }
            });
            ui.separator();
            for c in [Command::Save, Command::SaveAs] {
                item(ui, c, true, &mut raised);
            }
            ui.separator();
            for c in [Command::SaveSnapshot, Command::Snapshots] {
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
                let mark = if workspace.is_open(id) {
                    "\u{2714} "
                } else {
                    "   "
                };
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
                .add(egui::Button::new(format!("{mark}Lock Layout")).shortcut_text(shortcut))
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
                for c in [
                    Command::AddSun,
                    Command::AddSky,
                    Command::AddLamp,
                    Command::AddGloom,
                    Command::AddFire,
                ] {
                    item(ui, c, true, &mut raised);
                }
                ui.separator();
                // Animate the selected light: key its state at the playhead, or
                // clear the key there. The editor reports if no light is chosen.
                item(ui, Command::AddLightKeyframe, true, &mut raised);
                item(ui, Command::RemoveLightKeyframe, true, &mut raised);
            });

            // **Staging, beside the lights and for the same reason.** Neither
            // is in Animate at all, and both answer the same question — what is
            // in this shot before anything is drawn? Insert is where somebody
            // coming from either Animate or Blender looks first.
            ui.separator();
            ui.menu_button("Scene", |ui| {
                // The scenes of the film first: they are what the menu is
                // named after, and the breadcrumb above the stage is too
                // quiet a home for the only way to make one.
                item(ui, Command::AddScene, true, &mut raised);
                item(ui, Command::DuplicateScene, true, &mut raised);
                ui.separator();
                item(ui, Command::SetScene, true, &mut raised);
                item(ui, Command::DirectScene, true, &mut raised);
                item(ui, Command::AddPerson, true, &mut raised);
                ui.separator();
                // Left enabled whatever is selected: the dialog is where the
                // reason belongs, because "select a rigged character" is
                // something to read, not something to infer from a grey item.
                item(ui, Command::Perform, true, &mut raised);
                item(ui, Command::AddFollowThrough, true, &mut raised);
                item(ui, Command::AddWiggle, true, &mut raised);
                item(ui, Command::BakeModifiers, has_selection, &mut raised);
                item(ui, Command::ClearModifiers, has_selection, &mut raised);
                ui.separator();
                item(ui, Command::SetReverse, has_selection, &mut raised);
                item(ui, Command::ClearReverse, has_selection, &mut raised);
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

            // Animate keeps Align in a submenu of Modify, and a panel besides.
            // The submenu is where the hand goes; the thirteen operations do
            // not each want a keystroke.
            ui.menu_button("Align", |ui| {
                // **Two halves, because "align to stage" is a different
                // operation and not a modifier on this one.** Spelling both
                // out beats a checkbox whose state you cannot see from here.
                for op in crate::align::Align::ALL {
                    if ui
                        .add_enabled(has_selection, egui::Button::new(op.label()))
                        .clicked()
                    {
                        raised.push(Command::Align {
                            op,
                            to_stage: false,
                        });
                        ui.close();
                    }
                }
                ui.separator();
                ui.menu_button("To Stage", |ui| {
                    for op in crate::align::Align::ALL {
                        if ui
                            .add_enabled(has_selection, egui::Button::new(op.label()))
                            .clicked()
                        {
                            raised.push(Command::Align { op, to_stage: true });
                            ui.close();
                        }
                    }
                });

                ui.separator();
                ui.label(RichText::new("Distribute").small().weak());
                for op in crate::align::Distribute::ALL {
                    if ui
                        .add_enabled(has_selection, egui::Button::new(op.label()))
                        .on_hover_text(match op {
                            crate::align::Distribute::HorizontalSpacing
                            | crate::align::Distribute::VerticalSpacing => {
                                "Equal gaps \u{2014} what the eye reads as evenly \
                                 spaced when the objects are different sizes"
                            }
                            _ => "Equal distance between centres",
                        })
                        .clicked()
                    {
                        raised.push(Command::Distribute(op));
                        ui.close();
                    }
                }

                ui.separator();
                ui.label(RichText::new("Match Size").small().weak());
                for op in crate::align::MatchSize::ALL {
                    if ui
                        .add_enabled(has_selection, egui::Button::new(op.label()))
                        .on_hover_text("Scale everything up to the largest, about its own centre")
                        .clicked()
                    {
                        raised.push(Command::MatchSize(op));
                        ui.close();
                    }
                }
            });
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

    // **No scroll area of its own.** The dock column it sits in already
    // scrolls, and a vertical scroll area inside another one is told it has
    // the whole column to fill — so it claims the lot, its scrollbar lands on
    // top of the column's, and every panel below it is pushed off the bottom
    // of the window. That single mistake, repeated in five panels, is what hid
    // the Library and the Assets panel entirely.
    {
        // **As many columns as it has been given room for.** One column
        // is Animate's strip and what the default width holds; widen the
        // dock and the tools flow into two or three rather than leaving a
        // column of empty space beside them.
        let spacing = ui.spacing().item_spacing.x;
        let per_row = ((ui.available_width() + spacing) / (Metrics::TOOL_BUTTON + spacing))
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
    }

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

/// Magic Wand settings.
///
/// Two dials, both of which change the answer completely, and neither of which
/// can be guessed from a result — a wand that took too little and one that took
/// too much look the same until you look at what is left. So they sit beside
/// the brush settings, in the same place, rather than behind a menu.
fn wand_properties(ui: &mut Ui, style: &mut DrawStyle) {
    egui::Grid::new("wand-props").num_columns(2).show(ui, |ui| {
        ui.label("Tolerance");
        // Shown 0–255, which is the scale every other editor uses and the one
        // the numbers in people's heads are in.
        let mut tolerance = style.wand.tolerance * 255.0;
        if ui
            .add(egui::Slider::new(&mut tolerance, 0.0..=255.0).fixed_decimals(0))
            .on_hover_text("How far a colour may differ from the one clicked and still be taken")
            .changed()
        {
            style.wand.tolerance = tolerance / 255.0;
        }
        ui.end_row();

        ui.label("Contiguous");
        ui.checkbox(&mut style.wand.contiguous, "").on_hover_text(
            "Take only what joins on to the click. Off takes every matching \
             colour in the picture — all of the sky, even where a tree divides it.",
        );
        ui.end_row();
    });
}

/// Eraser settings: how wide it rubs.
///
/// Its own size, rather than four times the stroke-width slider — which is a
/// number about outlines, defaults to one, and left the eraser four units
/// across whatever the brush was set to, with nothing in the options saying so.
fn eraser_properties(ui: &mut Ui, style: &mut DrawStyle) {
    egui::Grid::new("eraser-props").num_columns(2).show(ui, |ui| {
        ui.label("Size");
        ui.add(
            egui::Slider::new(&mut style.eraser_size, 1.0..=200.0)
                .logarithmic(true)
                .suffix(" px"),
        )
        .on_hover_text(
            "How wide the eraser rubs, in document units \u{2014} the same units the \
             brush size is in.",
        );
        ui.end_row();
    });
    ui.label(
        RichText::new(
            "Rubbing through a shape cuts it in two: each piece becomes a shape of \
             its own, so they can be moved apart.",
        )
        .small()
        .weak(),
    );
}

/// Paint Bucket settings: the gap size the fill will bridge.
fn bucket_properties(ui: &mut Ui, style: &mut DrawStyle) {
    egui::Grid::new("bucket-props").num_columns(2).show(ui, |ui| {
        ui.label("Gap size");
        egui::ComboBox::from_id_salt("bucket-gap")
            .selected_text(style.gap_size.label())
            .show_ui(ui, |ui| {
                for gap in buzz_scene::GapSize::ALL {
                    ui.selectable_value(&mut style.gap_size, gap, gap.label());
                }
            })
            .response
            .on_hover_text(
                "How large a gap in the outline the bucket will close before it \
                 fills. Larger settings fill sketchier line art but can spill \
                 through wider openings.",
            );
        ui.end_row();
    });
}

/// Brush settings, with a live preview of the pattern being stamped.
///
/// Animate puts these in the tool options strip under the toolbar. They are
/// here in Properties because that is where this application's contextual
/// settings already live, and splitting them across two places would be worse
/// than the deviation.
fn brush_properties(ui: &mut Ui, style: &mut DrawStyle) {
    use crate::brush::{BrushKind, PatternShape};

    egui::Grid::new("brush-props").num_columns(2).show(ui, |ui| {
        ui.label("Type");
        egui::ComboBox::from_id_salt("brush-kind")
            .selected_text(style.brush.kind.label())
            .show_ui(ui, |ui| {
                for kind in BrushKind::ALL {
                    ui.selectable_value(&mut style.brush.kind, kind, kind.label())
                        .on_hover_text(kind.description());
                }
            });
        ui.end_row();

        if style.brush.kind == BrushKind::Effect {
            ui.label("Effect");
            egui::ComboBox::from_id_salt("brush-effect")
                .selected_text(style.brush.effect.label())
                .show_ui(ui, |ui| {
                    for effect in buzz_scene::EffectKind::ALL {
                        ui.selectable_value(&mut style.brush.effect, effect, effect.label())
                            .on_hover_text(effect.description());
                    }
                })
                .response
                .on_hover_text(style.brush.effect.description());
            ui.end_row();
        }

        if style.brush.kind == BrushKind::Wave {
            ui.label("Wave");
            // Not `selectable_value`: picking a kind has to load its preset,
            // so the choice goes through `set_wave` rather than writing the
            // field behind its back.
            egui::ComboBox::from_id_salt("brush-wave")
                .selected_text(style.brush.wave.label())
                .show_ui(ui, |ui| {
                    for wave in buzz_scene::WaveKind::ALL {
                        let chosen = style.brush.wave == wave;
                        if ui
                            .selectable_label(chosen, wave.label())
                            .on_hover_text(wave.description())
                            .clicked()
                        {
                            style.brush.set_wave(wave);
                        }
                    }
                })
                .response
                .on_hover_text(style.brush.wave.description());
            ui.end_row();
        }

        ui.label("Size");
        ui.add(
            egui::Slider::new(&mut style.brush.size, 1.0..=200.0)
                .logarithmic(true)
                .suffix(" px"),
        );
        ui.end_row();

        ui.label("Smoothing");
        ui.add(egui::Slider::new(&mut style.brush.smoothing, 0.0..=1.0))
            .on_hover_text(
                "Evens out a line after it is drawn, by pulling each point towards \
                 its neighbours. It sees the whole stroke, so it cannot lag \u{2014} and \
                 the ends never move.",
            );
        ui.end_row();

        ui.label("Stabiliser");
        ui.add(egui::Slider::new(&mut style.brush.stabiliser, 0.0..=1.0))
            .on_hover_text(
                "Drags the ink along behind the pointer, the way a heavy hand-rest \
                 does, so the shake never reaches the paper. The line catches up \
                 when you let go. Different from Smoothing: this one is a lag, and \
                 that is the point of it.",
            );
        ui.end_row();

        // A generated stroke chooses its own compositing per piece — a glow
        // adds, a silhouette covers — so Build up would be a lie there. What
        // those brushes show instead is what they do with the fill swatch,
        // which is the question their colour actually raises.
        if !style.brush.kind.composites_itself() {
            ui.label("Build up");
            ui.checkbox(&mut style.brush.build_up, "")
                .on_hover_text(
                    "Opacity adds where strokes overlap: 20% crossing 30% gives                  50%, not 44%. Paint deepens as you work over it, the way ink                  does.",
                );
            ui.end_row();
        } else {
            let hint = if style.brush.kind == BrushKind::Wave {
                style.brush.wave.color_hint()
            } else {
                style.brush.effect.color_hint()
            };
            ui.label("Colour");
            ui.label(
                egui::RichText::new(hint)
                    .size(10.5)
                    .color(Palette::text_dim()),
            );
            ui.end_row();
        }

        if style.brush.kind == BrushKind::Wave {
            wave_settings(ui, &mut style.brush.wave_settings);
        }

        if style.brush.kind == BrushKind::Raster {
            ui.label("Hardness");
            ui.add(egui::Slider::new(&mut style.brush.hardness, 0.0..=1.0))
                .on_hover_text(
                    "Where the edge starts to fade. 1 is a hard edge; 0 fades \
                     from the very middle, as an airbrush does.",
                );
            ui.end_row();

            ui.label("Flow");
            ui.add(egui::Slider::new(&mut style.brush.flow, 0.05..=1.0))
                .on_hover_text("How much paint the stroke lays down");
            ui.end_row();
        }

        // Width, taper and viscosity belong to both brushes that draw a filled
        // outline; only the *response* is what makes one of them fluid.
        if style.brush.kind.is_outlined() {
            ui.label("Viscosity");
            ui.add(egui::Slider::new(&mut style.brush.viscosity, 0.0..=1.0))
                .on_hover_text(
                    "How much the paint resists spreading. Thin paint lets the \
                     stroke's edge bulge outwards between the points it was drawn \
                     through, which reads as ink that ran after you drew it; thick \
                     paint holds the edge you made.",
                );
            ui.end_row();

            ui.label("Taper");
            ui.add(egui::Slider::new(&mut style.brush.taper, 0.0..=0.5))
                .on_hover_text("How much of the stroke narrows to a point");
            ui.end_row();

            if style.brush.taper > 0.0 {
                ui.label("Taper ends");
                egui::ComboBox::from_id_salt("brush-taper-ends")
                    .selected_text(style.brush.taper_ends.label())
                    .show_ui(ui, |ui| {
                        for ends in buzz_geom::TaperEnds::ALL {
                            ui.selectable_value(&mut style.brush.taper_ends, ends, ends.label())
                                .on_hover_text(ends.description());
                        }
                    })
                    .response
                    .on_hover_text(style.brush.taper_ends.description());
                ui.end_row();
            }
        }

        if style.brush.kind == BrushKind::Fluid {
            ui.label("Thinnest");
            ui.add(egui::Slider::new(&mut style.brush.min_ratio, 0.0..=1.0))
                .on_hover_text("How thin the stroke gets at full speed or lightest pressure");
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
                    if style.brush.custom_stamp.is_some() {
                        ui.selectable_value(
                            &mut style.brush.pattern,
                            PatternShape::Custom,
                            PatternShape::Custom.label(),
                        );
                    }
                });
            ui.end_row();

            // Only a captured brush has paint of its own to keep, so the
            // choice is only offered where it means something.
            if style.brush.pattern_stamp().is_some_and(|s| s.is_painted()) {
                ui.label("Artwork colours");
                ui.checkbox(&mut style.brush.keep_source_paint, "")
                    .on_hover_text(
                        "Stamp the artwork exactly as you drew it \u{2014} its colours, \
                         gradients and bitmaps. Off stamps its outline only, painted \
                         by the fill swatch.",
                    );
                ui.end_row();
            }

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

/// The Wave brush's own settings — how the flow is shaped, and how long it
/// runs for.
///
/// Emitted into the caller's grid, in the order you would reach for them:
/// the shape of the wave first, then the bundle it is drawn as, then the
/// animation it commits. Lengths are in brush sizes rather than pixels, which
/// is what makes a wave the same wave when the brush is resized — so the
/// sliders say so.
fn wave_settings(ui: &mut Ui, wave: &mut buzz_scene::WaveSettings) {
    ui.label("Amplitude");
    ui.add(egui::Slider::new(&mut wave.amplitude, 0.0..=4.0).suffix("\u{d7}"))
        .on_hover_text("How far a strand swings across the stroke, in brush sizes");
    ui.end_row();

    ui.label("Wavelength");
    ui.add(egui::Slider::new(&mut wave.wavelength, 0.5..=16.0).suffix("\u{d7}"))
        .on_hover_text("Distance from one crest to the next, in brush sizes");
    ui.end_row();

    ui.label("Turbulence");
    ui.add(egui::Slider::new(&mut wave.turbulence, 0.0..=1.0))
        .on_hover_text(
            "A second, shorter wave laid over the first. Without it a bundle \
             reads as a drawn sine; with it, as something moving in air or \
             water.",
        );
    ui.end_row();

    ui.label("Drift");
    ui.add(egui::Slider::new(&mut wave.drift, -3.0..=3.0).suffix("\u{d7}"))
        .on_hover_text("A lean that grows along the stroke \u{2014} the draught that bends a plume");
    ui.end_row();

    ui.label("Strands");
    ui.add(egui::Slider::new(&mut wave.strands, 1..=64))
        .on_hover_text("How many strands the bundle holds");
    ui.end_row();

    ui.label("Spread");
    ui.add(egui::Slider::new(&mut wave.spread, 0.0..=8.0).suffix("\u{d7}"))
        .on_hover_text("How far the bundle spreads across the stroke, in brush sizes");
    ui.end_row();

    ui.label("Thickness");
    ui.add(egui::Slider::new(&mut wave.thickness, 0.02..=2.0))
        .on_hover_text("Strand width, as a fraction of the brush size");
    ui.end_row();

    ui.label("Taper");
    ui.add(egui::Slider::new(&mut wave.taper, 0.0..=1.0))
        .on_hover_text("How much a strand narrows towards its far end. At 1 it finishes in a point.");
    ui.end_row();

    ui.label("Frames");
    ui.add(egui::Slider::new(&mut wave.frames, 1..=240))
        .on_hover_text(
            "Frames one stroke commits. At 1 the wave is a still drawing; above \
             that it is baked onto that many keyframes and loops seamlessly.",
        );
    ui.end_row();

    if wave.is_animated() {
        ui.label("Cycles");
        ui.add(egui::Slider::new(&mut wave.cycles, 1..=8))
            .on_hover_text(
                "Whole waves that pass in one loop \u{2014} the speed of the flow. Whole, \
                 because a fractional count is a loop that jolts when it repeats.",
            );
        ui.end_row();
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

    // A gentle S, so the rotation of the stamps is visible.
    let span = rect.width() as f64 - 16.0;
    let spine = buzz_geom::catmull_rom(&[
        buzz_geom::Point::new(0.0, 6.0),
        buzz_geom::Point::new(span * 0.33, -6.0),
        buzz_geom::Point::new(span * 0.66, 6.0),
        buzz_geom::Point::new(span, -6.0),
    ]);
    // The preview budget, because this is drawn every frame the panel is open.
    let budget = buzz_geom::BrushBudget::preview();

    // **What to draw, and in what colour.** A captured brush that keeps its
    // own paint contributes one entry per piece, each in that piece's own
    // colour, so the strip shows the brush the user actually made. Everything
    // else is one silhouette in the fill swatch, as before.
    let mut parts: Vec<(buzz_geom::BezPath, egui::Color32)> = Vec::new();
    if style.brush.stamps_its_own_paint()
        && let Some(stamp) = style.brush.pattern_stamp()
    {
        let size = style.brush.size.max(1.0);
        let plan = buzz_geom::stamp_transforms(
            &spine,
            stamp.source_rect(size),
            style.brush.fit(),
            &budget,
        );
        // The placements are in the scaled stamp's space; the artwork itself
        // is in unit stamp space, so the size goes on here.
        let placed: Vec<buzz_geom::Affine> = plan
            .transforms
            .iter()
            .map(|t| *t * buzz_geom::Affine::scale(size))
            .collect();
        for shape in stamp.place_many(&placed).shapes {
            // egui's painter has no gradient or bitmap brush, so a piece's one
            // standing colour is what the strip can show. It is enough to
            // judge spacing, size and *which brush this is*, which is what the
            // strip is for; the stage draws the real thing.
            let colour = shape
                .fill
                .as_ref()
                .map(|f| f.paint.color())
                .or_else(|| shape.stroke.as_ref().map(|s| s.paint.color()))
                .unwrap_or(Color::BLACK);
            parts.push((shape.path, to_egui(colour)));
        }
    } else if let Some(source) = style.brush.pattern_path() {
        let swatch = to_egui(if style.fill_enabled {
            style.fill_color_for_preview()
        } else {
            Color::BLACK
        });
        let stamped = buzz_geom::stamp_along(&spine, &source, style.brush.fit(), &budget);
        parts.push((stamped.path, swatch));
    }

    // Fit whatever came out into the strip.
    let bounds = parts
        .iter()
        .map(|(path, _)| buzz_geom::Shape::bounding_box(path))
        .reduce(|a, b| a.union(b));
    let Some(bounds) = bounds.filter(|b| b.width() > 0.0 && b.height() > 0.0) else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No shape yet",
            egui::FontId::proportional(11.0),
            Palette::text_dim(),
        );
        return;
    };
    let scale = ((rect.width() as f64 - 12.0) / bounds.width())
        .min((rect.height() as f64 - 12.0) / bounds.height())
        .min(1.0);

    let to_screen = |p: buzz_geom::Point| -> egui::Pos2 {
        egui::pos2(
            rect.center().x + ((p.x - bounds.center().x) * scale) as f32,
            rect.center().y + ((p.y - bounds.center().y) * scale) as f32,
        )
    };

    // Each subpath is one stamp, so they are drawn separately rather than
    // joined into a single polyline that would connect them all together.
    for (path, colour) in &parts {
        let mut current: Vec<egui::Pos2> = Vec::new();
        let flush = |points: &mut Vec<egui::Pos2>| {
            if points.len() >= 3 {
                painter.add(egui::Shape::convex_polygon(
                    std::mem::take(points),
                    *colour,
                    egui::Stroke::NONE,
                ));
            } else {
                points.clear();
            }
        };
        kurbo::flatten(path.iter(), 0.2 / scale.max(1e-6), |element| match element {
            kurbo::PathEl::MoveTo(p) => {
                flush(&mut current);
                current.push(to_screen(p));
            }
            kurbo::PathEl::LineTo(p) => current.push(to_screen(p)),
            kurbo::PathEl::ClosePath => flush(&mut current),
            _ => {}
        });
        flush(&mut current);
    }
}

/// Contextual properties for the current selection.
/// The Effects section of the document properties: the full-frame compositor.
///
/// Edits a `Copy` of the settings and writes the whole bundle back only when
/// something actually moved, so an untouched panel never bumps the document's
/// revision or marks it dirty — the same discipline the width and FPS rows use.
fn effects_properties(ui: &mut Ui, scene: &mut Scene) -> bool {
    let mut post = scene.stage().post;
    let mut changed = false;

    egui::CollapsingHeader::new(RichText::new("Effects").strong())
        .id_salt("effects-section")
        .default_open(false)
        .show(ui, |ui| {
            changed |= ui
                .checkbox(&mut post.enabled, "Enable effects")
                .changed();
            ui.add_enabled_ui(post.enabled, |ui| {
                changed |= bloom_controls(ui, &mut post.bloom);
                changed |= grade_controls(ui, &mut post.grade);
                changed |= vignette_controls(ui, &mut post.vignette);
                changed |= grain_controls(ui, &mut post.grain);
            });
        });

    if changed {
        scene.stage_mut().post = post;
    }
    changed
}

/// Depth-of-field controls: a camera aperture and the depth in focus. Writes
/// back only on change, so an untouched panel never dirties the document.
fn depth_of_field_properties(ui: &mut Ui, scene: &mut Scene) -> bool {
    let mut aperture = scene.camera().aperture;
    let mut focus = scene.camera().focus_depth;
    let mut changed = false;

    egui::CollapsingHeader::new(RichText::new("Depth of Field").strong())
        .id_salt("dof-section")
        .default_open(false)
        .show(ui, |ui| {
            egui::Grid::new("dof-grid").num_columns(2).show(ui, |ui| {
                ui.label("Aperture")
                    .on_hover_text("0 is a pinhole — everything sharp");
                if ui
                    .add(egui::Slider::new(&mut aperture, 0.0..=0.2).step_by(0.001))
                    .changed()
                {
                    scene.camera_mut().aperture = aperture;
                    changed = true;
                }
                ui.end_row();

                ui.label("Focus depth")
                    .on_hover_text("The layer depth that stays sharp");
                if ui
                    .add(egui::DragValue::new(&mut focus).speed(1.0))
                    .changed()
                {
                    scene.camera_mut().focus_depth = focus;
                    changed = true;
                }
                ui.end_row();
            });
        });
    changed
}

fn bloom_controls(ui: &mut Ui, b: &mut buzz_scene::BloomSettings) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new("Bloom")
        .id_salt("fx-bloom")
        .show(ui, |ui| {
            changed |= ui.checkbox(&mut b.enabled, "On").changed();
            egui::Grid::new("fx-bloom-grid").num_columns(2).show(ui, |ui| {
                ui.label("Threshold");
                changed |= ui
                    .add(egui::Slider::new(&mut b.threshold, 0.0..=1.0))
                    .changed();
                ui.end_row();
                ui.label("Intensity");
                changed |= ui
                    .add(egui::Slider::new(&mut b.intensity, 0.0..=3.0))
                    .changed();
                ui.end_row();
                ui.label("Radius");
                changed |= ui.add(egui::Slider::new(&mut b.radius, 0.0..=1.0)).changed();
                ui.end_row();
            });
        });
    changed
}

fn grade_controls(ui: &mut Ui, g: &mut buzz_scene::GradeSettings) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new("Colour Grade")
        .id_salt("fx-grade")
        .show(ui, |ui| {
            changed |= ui.checkbox(&mut g.enabled, "On").changed();
            egui::Grid::new("fx-grade-grid").num_columns(2).show(ui, |ui| {
                ui.label("Exposure");
                changed |= ui.add(egui::Slider::new(&mut g.exposure, -3.0..=3.0)).changed();
                ui.end_row();
                ui.label("Contrast");
                changed |= ui.add(egui::Slider::new(&mut g.contrast, 0.0..=2.0)).changed();
                ui.end_row();
                ui.label("Saturation");
                changed |= ui.add(egui::Slider::new(&mut g.saturation, 0.0..=2.0)).changed();
                ui.end_row();
                ui.label("Temperature");
                changed |= ui.add(egui::Slider::new(&mut g.temperature, -1.0..=1.0)).changed();
                ui.end_row();
                ui.label("Tint");
                changed |= ui.add(egui::Slider::new(&mut g.tint, -1.0..=1.0)).changed();
                ui.end_row();
                ui.label("Lift");
                changed |= ui.add(egui::Slider::new(&mut g.lift, -0.5..=0.5)).changed();
                ui.end_row();
                ui.label("Gamma");
                changed |= ui.add(egui::Slider::new(&mut g.gamma, 0.1..=3.0)).changed();
                ui.end_row();
                ui.label("Gain");
                changed |= ui.add(egui::Slider::new(&mut g.gain, 0.0..=2.0)).changed();
                ui.end_row();
            });
        });
    changed
}

fn vignette_controls(ui: &mut Ui, v: &mut buzz_scene::VignetteSettings) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new("Vignette")
        .id_salt("fx-vignette")
        .show(ui, |ui| {
            changed |= ui.checkbox(&mut v.enabled, "On").changed();
            egui::Grid::new("fx-vignette-grid").num_columns(2).show(ui, |ui| {
                ui.label("Amount");
                changed |= ui.add(egui::Slider::new(&mut v.amount, 0.0..=1.0)).changed();
                ui.end_row();
                ui.label("Softness");
                changed |= ui.add(egui::Slider::new(&mut v.softness, 0.0..=1.0)).changed();
                ui.end_row();
                ui.label("Colour");
                changed |= color_row(ui, "fx-vignette-colour", &mut v.color);
                ui.end_row();
            });
        });
    changed
}

fn grain_controls(ui: &mut Ui, g: &mut buzz_scene::GrainSettings) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new("Grain")
        .id_salt("fx-grain")
        .show(ui, |ui| {
            changed |= ui.checkbox(&mut g.enabled, "On").changed();
            egui::Grid::new("fx-grain-grid").num_columns(2).show(ui, |ui| {
                ui.label("Amount");
                changed |= ui.add(egui::Slider::new(&mut g.amount, 0.0..=1.0)).changed();
                ui.end_row();
                ui.label("Size");
                changed |= ui.add(egui::Slider::new(&mut g.size, 1.0..=8.0)).changed();
                ui.end_row();
            });
        });
    changed
}

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
            let mut bg = scene.stage().background;
            if color_row(ui, "stage-bg", &mut bg) {
                scene.stage_mut().background = bg;
                changed = true;
            }
            ui.end_row();

            ui.label("Sort by depth")
                .on_hover_text("Draw layers ordered by depth rather than the timeline");
            let mut sort = scene.stage().sort_by_depth;
            if ui.checkbox(&mut sort, "").changed() {
                scene.stage_mut().sort_by_depth = sort;
                changed = true;
            }
            ui.end_row();
        });

        // Depth of field: a document-level camera setting, so it sits with the
        // document rather than in the timeline.
        changed |= depth_of_field_properties(ui, scene);

        // The full-frame look. Its own section because it is a different kind of
        // thing from the stage's size — the colour and mood of the finished
        // film, not its dimensions.
        changed |= effects_properties(ui, scene);
    } else if let Some(bounds) = selection.bounds_at(scene, at.frame) {
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

    // **Rolled up, not gone.**
    //
    // These two are settings for one tool each, and open they took six hundred
    // points of a panel that also has to show the document and the selection —
    // enough to push the Colour panel below it clean off the window. Closed by
    // default and remembered once opened, which is what every other long
    // section of settings does.
    egui::CollapsingHeader::new(RichText::new("Brush").strong())
        .id_salt("brush-section")
        .default_open(false)
        .show(ui, |ui| brush_properties(ui, style));
    egui::CollapsingHeader::new(RichText::new("Eraser").strong())
        .id_salt("eraser-section")
        .default_open(false)
        .show(ui, |ui| eraser_properties(ui, style));
    egui::CollapsingHeader::new(RichText::new("Magic Wand").strong())
        .id_salt("wand-section")
        .default_open(false)
        .show(ui, |ui| wand_properties(ui, style));
    egui::CollapsingHeader::new(RichText::new("Paint Bucket").strong())
        .id_salt("bucket-section")
        .default_open(false)
        .show(ui, |ui| bucket_properties(ui, style));

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
    // **Rolled up.** Ten checkboxes and a number, every one of which is also on
    // the View menu with its keyboard shortcut beside it. Open, they were a
    // quarter of the panel's height for settings chosen once a project.
    egui::CollapsingHeader::new(RichText::new("View").strong())
        .id_salt("view-section")
        .default_open(false)
        .show(ui, |ui| {
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
        });

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
        let mut c = style.stroke_color;
        if color_row(ui, "stroke", &mut c) {
            style.stroke_color = c;
            style.stroke_enabled = true;
            style.remember(c);
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
            let mut c = style.fill_color;
            if color_row(ui, "fill", &mut c) {
                style.fill_color = c;
                style.fill_enabled = true;
                style.remember(c);
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
            if count > 2
                && ui
                    .small_button("\u{1F5D1}")
                    .on_hover_text("Remove stop")
                    .clicked()
            {
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
/// The Layers panel.
///
/// `frame` is the playhead's, because clicking a layer selects what is on it
/// *now* — a layer's artwork is different on different frames, and selecting
/// frame zero's while looking at frame forty would select things not on screen.
/// A colour with **two ways in**: the picker, and a hex field.
///
/// # Why both
///
/// The picker is a button that opens a popup, and a popup is the one kind of
/// control that can be present, correct and still unreachable — behind a panel,
/// off the edge of a window, or closed by whatever else the frame is doing. A
/// hex field cannot fail that way: it is six characters, in the notation every
/// palette, every style guide and every other program already uses.
///
/// Returns whether the colour changed.
pub fn color_row(ui: &mut Ui, id: &str, color: &mut Color) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut egui_color = to_egui(*color);
        if ui
            .color_edit_button_srgba(&mut egui_color)
            .on_hover_text("Pick a colour")
            .changed()
        {
            *color = from_egui(egui_color);
            changed = true;
        }

        // The field is authoritative only while it is being typed in: while
        // the pointer is elsewhere it shows whatever the colour actually is,
        // so picking a colour updates the text and typing updates the picker.
        let key = egui::Id::new(("hex", id));
        let live = ui.memory(|m| m.data.get_temp::<String>(key));
        let mut text = live.unwrap_or_else(|| hex_of(*color));

        let response = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(74.0)
                .font(egui::TextStyle::Monospace)
                .hint_text("#RRGGBB"),
        );
        if response.has_focus() || response.changed() {
            ui.memory_mut(|m| m.data.insert_temp(key, text.clone()));
            if let Some(parsed) = parse_hex(&text)
                && parsed != *color
            {
                *color = parsed;
                changed = true;
            }
        } else {
            // Focus lost: forget the half-typed text so the field goes back to
            // showing the truth rather than whatever was abandoned in it.
            ui.memory_mut(|m| m.data.remove::<String>(key));
        }
    });
    changed
}

/// A colour as `#RRGGBB`, or `#RRGGBBAA` when it is not opaque.
fn hex_of(color: Color) -> String {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

/// Read `#RGB`, `#RRGGBB` or `#RRGGBBAA`, with or without the hash.
///
/// Lenient on the way in and strict on the way out: somebody pasting a colour
/// from a style guide should not have to think about which of the three
/// notations it is written in.
fn parse_hex(text: &str) -> Option<Color> {
    let text = text.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(&text[i..i + 2], 16).ok();
    match text.len() {
        // Short form: each digit stands for a pair, so `#f0c` is `#ff00cc`.
        3 => {
            let digit = |i: usize| u8::from_str_radix(&text[i..i + 1], 16).ok();
            let (r, g, b) = (digit(0)?, digit(1)?, digit(2)?);
            Some(Color::from_rgba8(r * 17, g * 17, b * 17, 255))
        }
        6 => Some(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, 255)),
        8 => Some(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

/// Which switch a layer row is drawing.
///
/// Public because the timeline draws the same three columns beside its own
/// layer names, as Animate does. One definition of what an eye looks like, so
/// the two places cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerIcon {
    /// The eye column: shown, or struck through when hidden.
    Eye,
    /// The padlock column: closed when locked, open when not.
    Lock,
    /// The outline column: a hollow square in the layer's own colour.
    Outline,
}

impl LayerIcon {
    /// The three columns, in the order Animate rules them.
    pub const ALL: [LayerIcon; 3] = [Self::Eye, Self::Lock, Self::Outline];

    /// The short word over the column, and what the switch does when clicked.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Eye => "show",
            Self::Lock => "lock",
            Self::Outline => "out",
        }
    }

    /// What this switch means, spelled out — the same words wherever it is
    /// drawn, because it is the same switch.
    pub fn hint(self, on: bool) -> &'static str {
        match (self, on) {
            (Self::Eye, true) => {
                "Visible \u{2014} click to hide. A hidden layer is left out of the export too."
            }
            (Self::Eye, false) => "Hidden \u{2014} click to show",
            (Self::Lock, true) => "Locked \u{2014} click to unlock",
            (Self::Lock, false) => {
                "Unlocked \u{2014} click to lock. A locked layer cannot be selected or edited."
            }
            (Self::Outline, _) => {
                "Show this layer as outlines, in its own colour. \
                 A working view: the export draws it filled."
            }
        }
    }
}

/// Edge of one of the three layer switches, and of the column heading over it.
/// Shared so the labels line up with what they name.
pub const LAYER_SWITCH: f32 = 15.0;
/// The layer's colour chip, which sits beside the switches.
pub const CHIP_WIDTH: f32 = 8.0;

/// Paint one of Animate's three layer columns into an arbitrary rectangle.
///
/// # Why these are painted and not characters
///
/// The same reason the tool icons are (see `icons.rs`): egui's bundled fonts
/// cover a scattering of the symbols an interface wants, and the ones missing
/// render as an empty box. This row previously used `O` for *visible* and `O`
/// again for *outlines* — two different switches with one glyph between them,
/// which is worse than a box, because it looks deliberate.
///
/// Painted shapes always render, at any size, in any theme.
///
/// # Why it takes a `Painter` rather than a `Ui`
///
/// The Layers panel lays these out as widgets; the timeline paints its whole
/// grid by hand and hit-tests it afterwards, because a row of egui widgets per
/// layer per frame is not a thing a timeline can afford. Both need the same
/// eye, so the drawing lives here and each caller brings its own rectangle.
pub fn paint_layer_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: LayerIcon,
    on: bool,
    layer_color: Color,
    hovered: bool,
) {
    // Off is dim, on is plain — except the outline switch, which shows the
    // layer's own colour when it is on, because that is the colour the artwork
    // is about to be drawn in.
    let ink = match (icon, on) {
        (LayerIcon::Outline, true) => to_egui(layer_color),
        (_, true) => Palette::text(),
        (_, false) => Palette::text_dim(),
    };
    if hovered {
        painter.rect_filled(rect, 2.0, Palette::raised());
    }

    let side = rect.width().min(rect.height());
    let c = rect.center();
    let s = side * 0.5;
    let at = |x: f32, y: f32| egui::pos2(c.x + x * s, c.y + y * s);
    let stroke = egui::Stroke::new(1.3, ink);

    match icon {
        LayerIcon::Eye => {
            // A lens: two arcs meeting at the corners, with a pupil. Drawn as
            // a polyline because egui has no arc primitive.
            let lens = |up: f32| -> Vec<egui::Pos2> {
                (0..=12)
                    .map(|i| {
                        let t = i as f32 / 12.0;
                        let x = -0.85 + t * 1.7;
                        // A parabola through (-0.85, 0), (0, up), (0.85, 0).
                        let y = up * (1.0 - (x / 0.85).powi(2));
                        at(x, y)
                    })
                    .collect()
            };
            painter.add(egui::Shape::line(lens(-0.62), stroke));
            painter.add(egui::Shape::line(lens(0.62), stroke));
            if on {
                painter.circle_filled(c, side * 0.13, ink);
            } else {
                // Struck through: the universal "not this".
                painter.line_segment([at(-0.95, 0.95), at(0.95, -0.95)], stroke);
            }
        }
        LayerIcon::Lock => {
            let body = egui::Rect::from_min_max(at(-0.6, 0.0), at(0.6, 0.8));
            if on {
                painter.rect_filled(body, 1.5, ink);
                // A closed shackle, sitting on the body.
                painter.add(egui::Shape::line(
                    (0..=10)
                        .map(|i| {
                            let a = std::f32::consts::PI * (1.0 + i as f32 / 10.0);
                            at(0.38 * a.cos(), -0.05 + 0.42 * a.sin())
                        })
                        .collect(),
                    stroke,
                ));
            } else {
                painter.rect_stroke(body, 1.5, stroke, egui::StrokeKind::Inside);
                // Open: the shackle swung up and to one side.
                painter.add(egui::Shape::line(
                    (0..=10)
                        .map(|i| {
                            let a = std::f32::consts::PI * (1.0 + i as f32 / 10.0);
                            at(-0.42 + 0.38 * a.cos(), -0.25 + 0.42 * a.sin())
                        })
                        .collect(),
                    stroke,
                ));
            }
        }
        LayerIcon::Outline => {
            // Animate's hollow square, and filled when the layer is drawn
            // normally — so the switch shows what the artwork looks like
            // rather than what clicking it would do.
            let box_rect = egui::Rect::from_min_max(at(-0.7, -0.7), at(0.7, 0.7));
            if on {
                painter.rect_stroke(box_rect, 1.0, stroke, egui::StrokeKind::Inside);
            } else {
                painter.rect_filled(box_rect, 1.0, ink);
            }
        }
    }
}

/// One layer switch, as a widget, for the Layers panel's own rows.
fn layer_toggle(ui: &mut Ui, icon: LayerIcon, on: bool, layer_color: Color) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(LAYER_SWITCH, LAYER_SWITCH),
        egui::Sense::click(),
    );
    paint_layer_icon(
        ui.painter(),
        rect,
        icon,
        on,
        layer_color,
        response.hovered(),
    );
    response
}

pub fn layers_panel(
    ui: &mut Ui,
    scene: &mut Scene,
    selection: &mut Selection,
    frame: u32,
) -> Option<Command> {
    let mut command = None;
    let can_delete = scene.layers().len() > 1;

    ui.horizontal(|ui| {
        ui.heading("Layers");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(can_delete, egui::Button::new("\u{1F5D1}").small())
                .on_hover_text("Delete the selected layer")
                .on_disabled_hover_text("A document must keep at least one layer")
                .clicked()
            {
                command = Some(Command::DeleteLayer);
            }
            if ui.small_button("Fld").on_hover_text("New folder").clicked() {
                command = Some(Command::NewLayerFolder);
            }
            if ui
                .small_button("\u{2795}")
                .on_hover_text("New layer")
                .clicked()
            {
                command = Some(Command::NewLayer);
            }
        });
    });

    // **A heading over the three switch columns.**
    //
    // The eye, the padlock and the outline box are painted line art fifteen
    // points square, in a dim grey, at the end of a row — and they were
    // reported as *missing options* by somebody looking straight at them. A
    // switch nobody recognises as a switch is not on the screen. Animate rules
    // these three columns and heads them; naming them here does the same job
    // in the space available, and the row also says where to right-click for
    // the same commands spelled out in words.
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right to left, so this reads back to front — the outline column
            // is the rightmost, as it is on the rows below and in the timeline.
            for icon in LayerIcon::ALL.into_iter().rev() {
                ui.add_sized(
                    egui::vec2(LAYER_SWITCH, 12.0),
                    egui::Label::new(RichText::new(icon.heading()).size(8.0).weak()),
                )
                .on_hover_text(icon.hint(true));
            }
            ui.add_space(CHIP_WIDTH);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new("right-click a layer for more")
                        .size(8.0)
                        .weak()
                        .italics(),
                );
            });
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

    // The dock column already scrolls; see the note on `tool_bar`.
    for id in ids {
        let Some(layer) = scene.layers().get(id) else {
            continue;
        };
        let (name, kind, visible, locked, outline, alpha, color, depth) = (
            layer.name.clone(),
            layer.kind,
            layer.visible,
            layer.locked,
            layer.outline,
            layer.alpha,
            layer.color,
            layer.parent.is_some() as usize,
        );

        let mut set_visible = visible;
        let mut set_locked = locked;
        let mut set_outline = outline;
        let mut select_this = false;

        // **The switches sit in a column of their own, hard against the right
        // edge; the name takes what is left.**
        //
        // They used to lead the row, with the name last — so a dock column
        // dragged narrow pushed the *name* off the end, and a column narrow
        // enough pushed the switches under the scroll bar. Animate's layout is
        // the other way round for this reason: the eye, the padlock and the
        // outline box are always in the same place, and the one thing that can
        // be any length is the one that gets truncated.
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // **Drawn, not lettered.** These three columns were an
                // `O`, a padlock and a second `O` — the same glyph for
                // "visible" and for "outlines", which is no way to tell
                // two switches apart. Animate draws an eye, a padlock and
                // a hollow square, and so does this: see `layer_toggle`.
                //
                // Laid out right to left, so this reads back to front: the
                // outline box is the rightmost of the three.
                if layer_toggle(ui, LayerIcon::Outline, set_outline, color)
                    .on_hover_text(LayerIcon::Outline.hint(set_outline))
                    .clicked()
                {
                    set_outline = !set_outline;
                }
                if layer_toggle(ui, LayerIcon::Lock, set_locked, color)
                    .on_hover_text(LayerIcon::Lock.hint(set_locked))
                    .clicked()
                {
                    set_locked = !set_locked;
                }
                if layer_toggle(ui, LayerIcon::Eye, set_visible, color)
                    .on_hover_text(LayerIcon::Eye.hint(set_visible))
                    .clicked()
                {
                    set_visible = !set_visible;
                }

                // The layer's colour chip, used for outline view.
                let (chip, _) =
                    ui.allocate_exact_size(egui::vec2(CHIP_WIDTH, 12.0), egui::Sense::hover());
                ui.painter().rect_filled(chip, 1.0, to_egui(color));

                // Whatever the switches left over belongs to the name.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
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
                    let width = ui.available_width().max(1.0);
                    // **A layer row is a drag source, for the Rigging panel.**
                    //
                    // Sorting a character's parts into a skeleton is done by
                    // name, by clicking, or by dragging — and the drawings are
                    // already listed here, one to a layer, which is where an
                    // animator who has just imported a character is looking.
                    // Dropping anywhere else does nothing at all: only a rig
                    // slot accepts this payload. See `rig_panel::DraggedPart`.
                    let drag_id = ui.id().with(("layer-drag", id.0));
                    let response = ui
                        .dnd_drag_source(
                            drag_id,
                            crate::rig_panel::DraggedPart::Layer(id),
                            |ui| {
                                ui.add_sized(
                                    egui::vec2(width, ui.spacing().interact_size.y),
                                    egui::Button::selectable(
                                        active == Some(id),
                                        format!("{mark}{name}"),
                                    )
                                    .truncate(),
                                )
                            },
                        )
                        .inner;
                    if response.clicked() {
                        // Animate selects the layer's artwork with it, so the
                        // obvious next move needs no second gesture.
                        select_this = true;
                    }

                    // **The same switches again, by name, on the right button.**
                    //
                    // The painted icons are the fast way once you know them, and
                    // nothing on a 15-point square says which one hides a layer.
                    // A menu that spells it out is how somebody finds these the
                    // first time, and it is where Delete belongs anyway: a
                    // destructive action does not want to be a 15-point square
                    // beside three toggles.
                    response.context_menu(|ui| {
                        ui.label(RichText::new(&name).small().weak());
                        ui.separator();
                        if ui
                            .button(if visible { "Hide Layer" } else { "Show Layer" })
                            .clicked()
                        {
                            set_visible = !visible;
                            ui.close();
                        }
                        if ui
                            .button(if locked { "Unlock Layer" } else { "Lock Layer" })
                            .clicked()
                        {
                            set_locked = !locked;
                            ui.close();
                        }
                        if ui
                            .button(if outline {
                                "Show Layer Filled"
                            } else {
                                "Show Layer as Outlines"
                            })
                            .clicked()
                        {
                            set_outline = !outline;
                            ui.close();
                        }
                        ui.separator();
                        if ui.button(Command::NewLayer.label()).clicked() {
                            command = Some(Command::NewLayer);
                            ui.close();
                        }
                        if ui.button(Command::NewLayerFolder.label()).clicked() {
                            command = Some(Command::NewLayerFolder);
                            ui.close();
                        }
                        ui.separator();
                        // Delete acts on the *active* layer, so the row being
                        // right-clicked has to become the active one first.
                        // Selecting happens below, before the command is
                        // dispatched, which is what makes that safe.
                        if ui
                            .add_enabled(can_delete, egui::Button::new(Command::DeleteLayer.label()))
                            .on_disabled_hover_text("A document must keep at least one layer")
                            .clicked()
                        {
                            select_this = true;
                            command = Some(Command::DeleteLayer);
                            ui.close();
                        }
                    });
                });
            });
        });

        if select_this {
            selection.select_layer(scene, id, frame);
        }

        if set_visible != visible
            || set_locked != locked
            || set_outline != outline
        {
            scene.update_layer(id, |l| {
                l.visible = set_visible;
                l.locked = set_locked;
                l.outline = set_outline;
            });
        }

        // **The second row belongs to the layer being worked on, and only to
        // it.**
        //
        // Parenting and layer kind were drawn for every layer, which put two
        // rows and seventy points on each. Twenty layers — an ordinary
        // character — filled the whole column with them, and everything below
        // the Layers panel went off the bottom of the window. They are settings
        // changed once and then left alone, so they belong to the selection,
        // which is exactly what Animate's Layer Properties does with them.
        if active != Some(id) {
            continue;
        }

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

        // **Transparency**, as a number rather than a switch, because it is
        // one. Drag it, or click and type.
        //
        // On the selected layer's row rather than on every row: it is a fifty-
        // point number field, and three of the panel's width went on it for
        // every layer in the document — width the layer's own *name* then did
        // not have. Animate keeps it in Layer Properties for the same reason.
        let mut set_alpha = alpha;
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0 + 18.0);
            ui.label(RichText::new("opacity").small().weak());
            let mut percent = (set_alpha * 100.0).round();
            if ui
                .add(
                    egui::DragValue::new(&mut percent)
                        .range(0.0..=100.0)
                        .speed(1.0)
                        .suffix("%")
                        .fixed_decimals(0),
                )
                .on_hover_text(
                    "How solid this layer is drawn while you work \u{2014} dim one \
                     to draw over it, or to see what is behind it. A working \
                     view only: the export draws every layer at full strength.",
                )
                .changed()
            {
                set_alpha = (percent / 100.0).clamp(0.0, 1.0);
            }
        });
        if set_alpha != alpha {
            scene.update_layer(id, |l| l.alpha = set_alpha);
        }

        // **One control to a row, each sized to the column.**
        //
        // These two combo boxes were 96 and 92 points wide, side by side, after
        // a label and an indent — a little over 250 points on a row a dock
        // column gives 191. That overflow did far more than clip a control: a
        // widget wider than its `Ui` expands that `Ui`'s *max* rect as well as
        // its min rect, which grew the panel's frame, which moved the whole
        // right-hand column 56 points right of where egui had placed it — and
        // the stage, laid out from what was left, then ran underneath it. The
        // ruler and the stage's own scrollbar were drawn on top of the
        // Properties panel, which is what "the scroll bar overlaps the panels"
        // was. A row that fits is not a tidiness question here.
        let indent = depth as f32 * 12.0 + 18.0;
        let field = |ui: &egui::Ui| (ui.available_width() - indent - 52.0).max(60.0);

        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.label(RichText::new("follows").small().weak());

            let label = match follows.and_then(|f| names.iter().find(|(other, _, _)| *other == f)) {
                Some((_, name, _)) => name.clone(),
                // An em dash reads as "nothing" without needing a word
                // for it, and this line is repeated on every layer.
                None => "\u{2014}".to_string(),
            };

            let width = field(ui);
            egui::ComboBox::from_id_salt(("follows", id.0))
                .selected_text(RichText::new(label).small())
                .width(width)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(follows.is_none(), "\u{2014}  none")
                        .clicked()
                    {
                        set_follows = Some((id, None));
                    }
                    for other in &choices {
                        let Some((_, name, _)) = names.iter().find(|(o, _, _)| o == other) else {
                            continue;
                        };
                        if ui.selectable_label(follows == Some(*other), name).clicked() {
                            set_follows = Some((id, Some(*other)));
                        }
                    }
                })
                .response
                .on_hover_text(
                    "Layer Parenting: this layer's artwork moves with the \
                             layer it follows",
                );
        });

        // **What kind of layer this is.** Masking is positional —
        // a mask claims the run of masked layers directly below it
        // — so this is the only control it needs: set one layer to
        // Mask, the ones under it to Masked, and the stack does the
        // rest. Folders are not offered: a folder holds layers, and
        // turning one into a drawing layer would orphan them.
        if kind != LayerKind::Folder {
            ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.label(RichText::new("kind").small().weak());
                let width = field(ui);
                egui::ComboBox::from_id_salt(("layer-kind", id.0))
                    .selected_text(RichText::new(kind.display_name()).small())
                    .width(width)
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
            });
        }
    }

    if let Some((layer, follows)) = set_follows {
        // The same call the timeline's parenting view makes, so both record
        // the pose the link was made at. See `Scene::set_follows`.
        scene.set_follows(layer, follows, frame);
    }
    if let Some((layer, kind)) = set_kind {
        scene.update_layer(layer, |l| l.kind = kind);
    }

    // **The camera, at the foot of the layer list.**
    //
    // The camera is not a layer — it is one shot the whole stack is seen
    // through, which is why it has never been in this list — but the layer
    // list is where somebody goes looking for it, because that is where
    // Animate puts its button and because "add a camera" is the same kind of
    // act as "add a layer". So it is offered here, in the place it is looked
    // for, and turning it on adds a camera keyframe rather than a row.
    ui.separator();
    ui.horizontal(|ui| {
        let on = scene.camera().enabled;
        let button = egui::Button::new(
            RichText::new(if on { "\u{1F3A5} Camera on" } else { "\u{1F3A5} Add Camera" }).small(),
        )
        .selected(on);
        if ui
            .add(button)
            .on_hover_text(if on {
                "The shot the whole stack is seen through. Click to turn it off \u{2014} \
                 the keyframes it has are kept."
            } else {
                "Frame the film through a camera: pan, zoom and turn the whole stack \
                 over time. Adds a first keyframe so there is something to aim."
            })
            .clicked()
        {
            command = Some(Command::ToggleCamera);
        }
    });

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
                    templates: &[],
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
            let _ = layers_panel(ui, &mut scene, &mut selection, 0);
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
                    templates: &[],
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
            let _ = layers_panel(ui, &mut scene, &mut selection, 0);
        });
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// **A panel in a dock column must take only the room it needs.**
    ///
    /// This is the regression that hid half the interface. Every list panel
    /// opened a vertical `ScrollArea` of its own, inside the one the dock
    /// column already had, with `auto_shrink([false, false])` — which says
    /// "fill the space available". Inside another scroll area the space
    /// available is the whole column, so the Layers panel took all of it, its
    /// scrollbar landed on top of the column's, and Properties, Library and the
    /// Assets panel were pushed off the bottom of the window where nobody could
    /// find them.
    ///
    /// Measured, not eyeballed: a panel showing one layer must not consume the
    /// height of the window.
    #[test]
    fn a_layers_panel_takes_only_the_room_its_rows_need() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = Scene::default();
        scene.add_layer("Only", buzz_scene::LayerKind::Normal);
        let mut selection = Selection::new();
        let mut used = 0.0;

        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("test-column")
                .show(ui, |ui| {
                    let top = ui.cursor().top();
                    let _ = layers_panel(ui, &mut scene, &mut selection, 0);
                    used = ui.cursor().top() - top;
                });
        });

        assert!(
            used > 0.0,
            "the panel drew nothing at all, so this proves nothing"
        );
        assert!(
            used < 400.0,
            "one layer took {used:.0} points of column. A panel that fills the \
             column pushes every panel below it off the screen \u{2014} which is \
             exactly what hid the Library and the Assets panel."
        );
    }

    #[test]
    fn a_tool_strip_takes_only_the_room_its_buttons_need() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut style = DrawStyle::default();
        let mut used = 0.0;

        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("test-column")
                .show(ui, |ui| {
                    let top = ui.cursor().top();
                    let _ = tool_bar(ui, ToolId::Selection, &mut style);
                    used = ui.cursor().top() - top;
                });
        });

        assert!(used > 0.0, "the tool strip drew nothing");
        // Twenty-three tools in eight groups, one column wide, plus the colour
        // wells. Tall, but a definite height rather than "all of it".
        assert!(
            used < 1200.0,
            "the tool strip took {used:.0} points of column"
        );
    }

    /// Hex in, hex out, in every notation somebody might paste.
    #[test]
    fn a_colour_can_be_typed_as_well_as_picked() {
        assert_eq!(
            parse_hex("#FF0066").map(|c| c.to_rgba8().to_u8_array()),
            Some([0xFF, 0x00, 0x66, 0xFF])
        );
        // Without the hash, and in lower case.
        assert_eq!(
            parse_hex("ff0066").map(|c| c.to_rgba8().to_u8_array()),
            Some([0xFF, 0x00, 0x66, 0xFF])
        );
        // The short form: each digit stands for a pair.
        assert_eq!(
            parse_hex("#f06").map(|c| c.to_rgba8().to_u8_array()),
            Some([0xFF, 0x00, 0x66, 0xFF])
        );
        // With alpha.
        assert_eq!(
            parse_hex("#10203040").map(|c| c.to_rgba8().to_u8_array()),
            Some([0x10, 0x20, 0x30, 0x40])
        );
        // And nonsense is refused rather than guessed at, so a half-typed
        // field does not repaint the stage on every keystroke.
        for bad in ["", "#", "12345", "#GGGGGG", "not a colour"] {
            assert!(parse_hex(bad).is_none(), "{bad:?} was accepted");
        }

        // Round trip: what is shown can be typed back.
        for c in [
            Color::BLACK,
            Color::WHITE,
            Color::from_rgb8(0x12, 0x34, 0x56),
            Color::from_rgba8(0x12, 0x34, 0x56, 0x78),
        ] {
            assert_eq!(
                parse_hex(&hex_of(c)).map(|c| c.to_rgba8().to_u8_array()),
                Some(c.to_rgba8().to_u8_array()),
                "{} did not survive being written out and read back",
                hex_of(c)
            );
        }
    }
}
