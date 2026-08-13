//! The Lighting panel: add lights, aim them, and set what they do.
//!
//! Laid out the way Blender's light properties are, because that is what an
//! animator asking for "a sun" has in mind: the rig at the top, then the list
//! of lights, then the selected light's own settings.
//!
//! Two deliberate departures from a plain column of sliders:
//!
//! * **Angles are degrees.** Nobody aims a sun in radians.
//! * **The sun gets a dial.** Direction and height are one gesture in the
//!   world — you point at the sun — and two sliders make you guess which
//!   number means what. The dial shows the answer: the handle *is* where the
//!   sun is, and the shadow runs the other way.

use buzz_geom::Point;
use buzz_scene::{Light, LightId, LightKind, LightRig};
use egui::{Color32, RichText, Ui};
use peniko::Color;

use crate::panels::{from_egui, to_egui};
use crate::theme::Palette;

/// What the user changed.
///
/// One field per kind of change rather than a mutated rig, so the editor can
/// make each one its own undo step with its own name: an animator who nudges
/// the sun and then regrets the colour should not lose both.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LightResponse {
    /// Add a light of this kind.
    pub add: Option<LightKind>,
    pub remove: Option<LightId>,
    pub select: Option<LightId>,
    /// A light was edited; this is the whole light, as it now is.
    pub changed: Option<Light>,
    /// The rig itself was switched on or off.
    pub set_enabled: Option<bool>,
    /// The fill colour left over where no light reaches.
    pub set_base: Option<Color>,
    /// How strongly shading and highlights are drawn.
    pub set_modelling: Option<f32>,
}

/// Panel state that is not part of the document.
#[derive(Debug, Clone, PartialEq)]
pub struct LightPanelState {
    pub selected: Option<LightId>,
    /// Draw the light handles on the stage, and let them be dragged.
    pub gizmos: bool,
}

impl Default for LightPanelState {
    fn default() -> Self {
        Self {
            selected: None,
            // On: a light you cannot see is a light you cannot aim, and the
            // handles cost nothing when there are no lights to draw.
            gizmos: true,
        }
    }
}

/// Where a new lamp is asked for. The editor re-homes it to the middle of the
/// view — it knows where the user is looking and the panel does not — so this
/// is only the fallback for a caller that has no view at all.
const NEW_LAMP: Point = Point::new(275.0, 120.0);

/// Draw the panel.
pub fn light_panel(ui: &mut Ui, rig: &LightRig, state: &mut LightPanelState) -> LightResponse {
    let mut out = LightResponse::default();

    ui.horizontal(|ui| {
        ui.heading("Lighting");
        let mut enabled = rig.enabled;
        if ui
            .checkbox(&mut enabled, "")
            .on_hover_text("Light the artwork with this rig")
            .changed()
        {
            out.set_enabled = Some(enabled);
        }
        if !rig.lights.is_empty() {
            ui.label(
                RichText::new(format!("{} lights", rig.lights.len()))
                    .small()
                    .weak(),
            );
        }
    });

    ui.horizontal(|ui| {
        if ui
            .small_button("+ Sun")
            .on_hover_text("Parallel light: one direction everywhere, one shadow direction")
            .clicked()
        {
            out.add = Some(LightKind::sun());
        }
        if ui
            .small_button("+ Sky")
            .on_hover_text("Ambient fill, overhead and horizon. Casts nothing.")
            .clicked()
        {
            out.add = Some(LightKind::sky());
        }
        if ui
            .small_button("+ Lamp")
            .on_hover_text(
                "A point on the stage: shadows radiate from it and lengthen with distance",
            )
            .clicked()
        {
            out.add = Some(LightKind::lamp(NEW_LAMP));
        }
    });

    if rig.lights.is_empty() {
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "No lights — artwork draws exactly as you painted it.\n\nAdd a sun for one \
                 direction, a sky to fill the shadows, or a lamp for light that falls off \
                 with distance.",
            )
            .small()
            .weak(),
        );
        return out;
    }

    ui.checkbox(&mut state.gizmos, "Show on stage")
        .on_hover_text("Draw the lights on the stage, and drag them to aim");

    ui.separator();

    // -- the lights ---------------------------------------------------------
    for light in &rig.lights {
        ui.horizontal(|ui| {
            let mut enabled = light.enabled;
            if ui
                .checkbox(&mut enabled, "")
                .on_hover_text("Switch this light off without losing its settings")
                .changed()
            {
                out.changed = Some(Light {
                    enabled,
                    ..light.clone()
                });
            }

            let selected = state.selected == Some(light.id);
            let label = format!("{}  ({})", light.name, light.kind.label());
            if ui.selectable_label(selected, label).clicked() {
                state.selected = Some(light.id);
                out.select = Some(light.id);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("x")
                    .on_hover_text("Delete this light")
                    .clicked()
                {
                    out.remove = Some(light.id);
                }
                // The light's own colour, as a swatch that is also the editor.
                let mut colour = to_egui(light.color);
                if ui.color_edit_button_srgba(&mut colour).changed() {
                    out.changed = Some(Light {
                        color: from_egui(colour),
                        ..light.clone()
                    });
                }
            });
        });
    }

    // -- the selected light -------------------------------------------------
    //
    // With nothing selected the first light is shown rather than an empty
    // panel: adding a light and being told to select one is a step backwards
    // from having added it.
    let selected = state
        .selected
        .and_then(|id| rig.get(id))
        .or_else(|| rig.lights.first());

    if let Some(light) = selected {
        ui.separator();
        ui.label(RichText::new(&light.name).strong());

        let mut edited = light.clone();
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label("Strength");
            changed |= ui
                .add(egui::Slider::new(&mut edited.intensity, 0.0..=4.0).fixed_decimals(2))
                .changed();
        });

        match &mut edited.kind {
            LightKind::Sun { azimuth, elevation } => {
                changed |= sun_dial(ui, azimuth, elevation);

                let mut degrees = azimuth.to_degrees();
                ui.horizontal(|ui| {
                    ui.label("Direction");
                    if ui
                        .add(
                            egui::Slider::new(&mut degrees, -180.0..=180.0)
                                .suffix("\u{b0}")
                                .fixed_decimals(0),
                        )
                        .on_hover_text("Which way round the stage the sun lies")
                        .changed()
                    {
                        *azimuth = degrees.to_radians();
                        changed = true;
                    }
                });

                let mut height = elevation.to_degrees();
                ui.horizontal(|ui| {
                    ui.label("Height");
                    if ui
                        .add(
                            egui::Slider::new(&mut height, 2.0..=90.0)
                                .suffix("\u{b0}")
                                .fixed_decimals(0),
                        )
                        .on_hover_text("How high it stands. Low is long shadows.")
                        .changed()
                    {
                        *elevation = height.to_radians();
                        changed = true;
                    }
                });
            }

            LightKind::Sky { horizon } => {
                ui.horizontal(|ui| {
                    ui.label("Horizon");
                    let mut colour = to_egui(*horizon);
                    if ui
                        .color_edit_button_srgba(&mut colour)
                        .on_hover_text(
                            "The colour low on the stage; the light's own colour is overhead",
                        )
                        .changed()
                    {
                        *horizon = from_egui(colour);
                        changed = true;
                    }
                    ui.label(RichText::new("low on the stage").small().weak());
                });
            }

            LightKind::Lamp {
                position,
                height,
                radius,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Position");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut position.x)
                                .speed(1.0)
                                .prefix("x "),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut position.y)
                                .speed(1.0)
                                .prefix("y "),
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Height");
                    changed |= ui
                        .add(egui::Slider::new(height, 20.0..=1200.0).suffix(" px"))
                        .on_hover_text("How far in front of the stage it hangs")
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Reach");
                    changed |= ui
                        .add(egui::Slider::new(radius, 40.0..=3000.0).suffix(" px"))
                        .on_hover_text("The distance at which it is half as bright")
                        .changed();
                });
            }
        }

        if !edited.is_ambient() {
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(&mut edited.shadows, "Shadows")
                    .on_hover_text("Cast a shadow of the artwork onto what is behind it")
                    .changed();
                if edited.shadows {
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut edited.shadow_strength, 0.0..=1.0)
                                .fixed_decimals(2),
                        )
                        .changed();
                }
            });

            ui.horizontal(|ui| {
                ui.label("Stands off");
                changed |= ui
                    .add(egui::Slider::new(&mut edited.standing_height, 0.0..=400.0).suffix(" px"))
                    .on_hover_text(
                        "How far the artwork is assumed to stand off the background. Flat \
                         drawings have no thickness, so this is what gives them a shadow at \
                         all \u{2014} layer depth adds to it.",
                    )
                    .changed();
            });

            ui.horizontal(|ui| {
                ui.label("Softness");
                changed |= ui
                    .add(egui::Slider::new(&mut edited.softness, 0.02..=0.9).fixed_decimals(2))
                    .on_hover_text("How wide the shaded edge is. Narrow reads as a hard light.")
                    .changed();
            });
        }

        if changed {
            out.changed = Some(edited);
        }
    }

    // -- the rig ------------------------------------------------------------
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Fill");
        let mut base = to_egui(rig.base);
        if ui
            .color_edit_button_srgba(&mut base)
            .on_hover_text("What is left where no light reaches \u{2014} rarely quite black")
            .changed()
        {
            out.set_base = Some(from_egui(base));
        }

        ui.label("Modelling");
        let mut modelling = rig.modelling;
        if ui
            .add(egui::Slider::new(&mut modelling, 0.0..=1.0).fixed_decimals(2))
            .on_hover_text("How strongly the shaded side and the highlight are drawn")
            .changed()
        {
            out.set_modelling = Some(modelling);
        }
    });

    out
}

/// The sun's direction, as a dial you point.
///
/// The handle sits where the sun is: dragging it round the dial swings the
/// azimuth, and dragging it towards the middle raises the sun overhead. The
/// dark spoke opposite is where the shadow will fall, drawn because that —
/// not the angle — is what the animator is actually choosing.
///
/// Returns whether the drag changed anything.
fn sun_dial(ui: &mut Ui, azimuth: &mut f64, elevation: &mut f64) -> bool {
    const RADIUS: f32 = 34.0;

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), RADIUS * 2.0 + 10.0),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let centre = egui::pos2(rect.left() + RADIUS + 6.0, rect.center().y);

    painter.circle_filled(centre, RADIUS, Palette::PANEL);
    painter.circle_stroke(centre, RADIUS, egui::Stroke::new(1.0, Palette::BORDER));
    // The horizon ring: the sun on it is level with the stage, at the middle
    // it is straight overhead.
    painter.circle_stroke(
        centre,
        RADIUS * 0.5,
        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 20)),
    );

    let mut changed = false;
    // Dead centre has no direction to read, so a click there leaves the sun
    // where it is rather than snapping it to an arbitrary bearing.
    if let Some(pos) = (response.dragged() || response.clicked())
        .then(|| ui.ctx().input(|i| i.pointer.interact_pos()))
        .flatten()
        && (pos - centre).length() > 1.0
    {
        let offset = pos - centre;
        *azimuth = (offset.y as f64).atan2(offset.x as f64);
        // Distance from the middle is the sun's height, the way a fisheye
        // photograph of the sky maps it: the rim is the horizon.
        let t = (offset.length() / RADIUS).clamp(0.0, 1.0) as f64;
        *elevation = ((1.0 - t) * std::f64::consts::FRAC_PI_2).clamp(0.03, 1.55);
        changed = true;
    }

    // Where the sun sits on the dial, and where its shadow runs.
    let t = 1.0 - (*elevation / std::f64::consts::FRAC_PI_2).clamp(0.0, 1.0);
    let (sin_a, cos_a) = azimuth.sin_cos();
    let arm = egui::vec2(cos_a as f32, sin_a as f32) * (RADIUS * t as f32);

    painter.line_segment(
        [centre, centre - arm],
        egui::Stroke::new(2.0, Color32::from_rgba_unmultiplied(0, 0, 0, 120)),
    );
    painter.line_segment(
        [centre, centre + arm],
        egui::Stroke::new(1.5, Palette::BORDER),
    );
    painter.circle_filled(centre + arm, 5.0, Color32::from_rgb(0xFF, 0xD9, 0x6A));

    painter.text(
        egui::pos2(centre.x + RADIUS + 12.0, rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        format!("{:.0}\u{b0}", azimuth.to_degrees()),
        egui::FontId::proportional(11.0),
        Palette::TEXT,
    );
    painter.text(
        egui::pos2(centre.x + RADIUS + 12.0, rect.center().y + 8.0),
        egui::Align2::LEFT_CENTER,
        format!("{:.0}\u{b0} up", elevation.to_degrees()),
        egui::FontId::proportional(11.0),
        Palette::TEXT_DIM,
    );

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig(lights: Vec<Light>) -> LightRig {
        LightRig {
            lights,
            enabled: true,
            ..LightRig::default()
        }
    }

    #[test]
    fn an_empty_panel_offers_the_three_kinds_and_changes_nothing() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut state = LightPanelState::default();

        let _ = ctx.run_ui(Default::default(), |ui| {
            let response = light_panel(ui, &LightRig::default(), &mut state);
            assert_eq!(
                response,
                LightResponse::default(),
                "drawing changes nothing"
            );
        });
    }

    #[test]
    fn the_panel_draws_every_kind_of_light() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let rig = rig(vec![
            Light::new(LightId(1), "Sun", LightKind::sun()),
            Light::new(LightId(2), "Sky", LightKind::sky()),
            Light::new(LightId(3), "Lamp", LightKind::lamp(Point::new(10.0, 10.0))),
        ]);

        for selected in [None, Some(LightId(1)), Some(LightId(2)), Some(LightId(3))] {
            let mut state = LightPanelState {
                selected,
                gizmos: true,
            };
            let _ = ctx.run_ui(Default::default(), |ui| {
                let _ = light_panel(ui, &rig, &mut state);
            });
        }
    }

    /// Drawing must not select a light on the user's behalf, even though the
    /// first light's settings are what it shows.
    #[test]
    fn drawing_does_not_select() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut state = LightPanelState::default();
        let rig = rig(vec![Light::new(LightId(1), "Sun", LightKind::sun())]);

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = light_panel(ui, &rig, &mut state);
        });
        assert!(state.selected.is_none());
    }

    /// A light that was deleted while selected must not blank the panel.
    #[test]
    fn a_stale_selection_falls_back_to_the_first_light() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut state = LightPanelState {
            selected: Some(LightId(99)),
            gizmos: false,
        };
        let rig = rig(vec![Light::new(LightId(1), "Sun", LightKind::sun())]);

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = light_panel(ui, &rig, &mut state);
        });
    }

    #[test]
    fn gizmos_are_on_by_default() {
        assert!(LightPanelState::default().gizmos);
    }
}
