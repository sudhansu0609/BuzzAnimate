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
use buzz_scene::{EdgeMode, Light, LightId, LightKind, LightRig, SHADOW_LENGTH_RANGE, ShadowFall};
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
    /// Add a lamp and set it alight. Its own field rather than a `LightKind`,
    /// because a fire is a lamp plus a handful of settings rather than a kind
    /// of its own; see [`buzz_scene::Light::make_fire`].
    pub add_fire: bool,
    /// Add a sky and set it striking. A preset for the same reason a fire is —
    /// see [`buzz_scene::Light::make_storm`] — and a sky rather than a sun
    /// because a sheet of lightning has no direction: it lights the whole stage
    /// at once, which is the thing an animator is after.
    pub add_storm: bool,
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
    /// What the light draws its bands around: nothing, each shape, or the
    /// whole figure.
    pub set_edges: Option<EdgeMode>,
    /// The keyframe button was pressed: `true` to key the selected light at the
    /// playhead, `false` to remove the key there.
    pub key: Option<bool>,
}

/// Panel state that is not part of the document.
#[derive(Debug, Clone, PartialEq)]
pub struct LightPanelState {
    pub selected: Option<LightId>,
    /// The playhead, so the keyframe button can key here and show whether the
    /// selected light already has a key at this frame.
    pub current_frame: u32,
    /// Draw the light handles on the stage, and let them be dragged.
    pub gizmos: bool,
    /// **What the renderer had to leave out of the last frame**, if anything.
    ///
    /// A document dense enough that its lit frame will not fit the rasteriser
    /// has some of its lighting trimmed away rather than losing the frame — see
    /// `buzz_render::document::LightDetail`. That has to be *said*: an animator
    /// looking at a lamp with the modelling missing, and no explanation, is
    /// looking at the same silence this whole mechanism exists to end.
    pub trimmed: Option<&'static str>,
}

impl Default for LightPanelState {
    fn default() -> Self {
        Self {
            selected: None,
            current_frame: 0,
            // On: a light you cannot see is a light you cannot aim, and the
            // handles cost nothing when there are no lights to draw.
            gizmos: true,
            trimmed: None,
        }
    }
}

/// Where a new lamp is asked for. The editor re-homes it to the middle of the
/// view — it knows where the user is looking and the panel does not — so this
/// is only the fallback for a caller that has no view at all.
const NEW_LAMP: Point = Point::new(275.0, 120.0);

/// Where a new wall of dark is asked for. As with a lamp, the editor throws
/// this away and aims one against whatever is already lighting the shot — see
/// [`buzz_scene::LightRig::opposing_gloom`] — so this is only what a caller
/// with no view at all would get.
const NEW_GLOOM: Point = Point::new(-200.0, 200.0);

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

    if let Some(trimmed) = state.trimmed {
        ui.label(RichText::new(trimmed).small().weak())
            .on_hover_text(
                "This document has more artwork in a frame than the renderer can                  rasterise with the lighting drawn in full, so the heaviest part                  of it is left out. Colour, falloff and the lamp's pool are                  unaffected. Zooming in, or simplifying the artwork, brings the                  rest back.",
            );
    }

    // **Wrapped, not in one row.** Five buttons side by side are wider than the
    // narrowest column the dock allows, so in a narrow dock the last of them —
    // Gloom and Fire — were simply off the edge of the panel with no way to
    // reach them. Wrapping costs a line of height when the column is narrow and
    // nothing at all when it is not.
    ui.horizontal_wrapped(|ui| {
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
        if ui
            .small_button("+ Gloom")
            .on_hover_text(
                "A wall of darkness with a long throw. Added facing back across the stage at \
                 whatever is lighting it, so the dark end of the picture moves as well as the \
                 bright one.",
            )
            .clicked()
        {
            out.add = Some(LightKind::gloom(NEW_GLOOM));
        }
        if ui
            .small_button("\u{1F525} Fire")
            .on_hover_text(
                "A lamp that gutters, in the colour of a hearth. It moves every frame with \
                 no keyframes at all \u{2014} scrub the timeline to see it.",
            )
            .clicked()
        {
            out.add_fire = true;
        }
        if ui
            .small_button("\u{26A1} Storm")
            .on_hover_text(
                "A dark sky that strikes: a leader, a beat of nothing, then the whole stage \
                 white for a few frames and gone. Every few seconds, never twice the same, \
                 with no keyframes at all \u{2014} scrub the timeline to see it.",
            )
            .clicked()
        {
            out.add_storm = true;
        }
    });

    if rig.lights.is_empty() {
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "No lights — artwork draws exactly as you painted it.\n\nAdd a sun for one \
                 direction, a sky to fill the shadows, or a lamp for light that falls off \
                 with distance. A gloom does the opposite: it takes light away, in a wide \
                 band thrown across the stage.",
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
            // **A gloom's strength stops at one.** It is the fraction of the
            // light it takes away, and taking away more than all of it means
            // nothing — a slider that ran to four would spend three quarters of
            // its travel doing nothing at all, which is how a control teaches
            // an animator that it is broken.
            let gloom = edited.is_gloom();
            let range = if gloom { 0.0..=1.0 } else { 0.0..=4.0 };
            ui.label(if gloom { "Depth" } else { "Strength" });
            changed |= ui
                .add(egui::Slider::new(&mut edited.intensity, range).fixed_decimals(2))
                .on_hover_text(if gloom {
                    "How much of the light it stops where the dark is deepest"
                } else {
                    "How brightly it burns"
                })
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
                            egui::Slider::new(&mut height, 0.0..=90.0)
                                .suffix("\u{b0}")
                                .fixed_decimals(0),
                        )
                        .on_hover_text(
                            "How high the sun stands. 0 is on the horizon, lighting \
                             from the side with a long shadow; 90 is straight \
                             overhead, lighting the tops of things with the shadow \
                             underneath them.",
                        )
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
                // **Front and back.** A lamp is a point in three dimensions:
                // `position` is where it stands across the stage and this is
                // how far in front of it. Dragging the stalk on the stage sets
                // the same number \u{2014} see `crate::lights::LightGesture::Raise`.
                ui.horizontal(|ui| {
                    ui.label("Forward");
                    changed |= ui
                        .add(egui::Slider::new(height, 4.0..=1200.0).suffix(" px"))
                        .on_hover_text(
                            "How far in front of the stage it hangs. Close in, its light \
                             falls off hard across the picture and its shadows splay; far \
                             back, it behaves more and more like a sun. Draggable on the \
                             stage by the stalk on the lamp.",
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Reach");
                    changed |= ui
                        .add(egui::Slider::new(radius, 40.0..=3000.0).suffix(" px"))
                        .on_hover_text("The distance at which it is half as bright")
                        .changed();
                });
                // **Fire**, as a preset rather than a fourth kind of light.
                // Everything a fire is, a lamp already has; the only things that
                // make it fire are the colour and the fact that it will not hold
                // still. See `buzz_scene::Light::make_fire`.
                ui.horizontal(|ui| {
                    if ui
                        .button("\u{1F525} Make it fire")
                        .on_hover_text(
                            "A hearth colour, a hard gutter and a tighter reach. Everything \
                             else about the lamp is left alone.",
                        )
                        .clicked()
                    {
                        edited.make_fire();
                        changed = true;
                    }
                    ui.label(RichText::new("scrub to see it move").small().weak());
                });

                ui.horizontal(|ui| {
                    ui.label("Flicker");
                    changed |= ui
                        .add(egui::Slider::new(&mut edited.flicker, 0.0..=1.0).fixed_decimals(2))
                        .on_hover_text(
                            "How much it gutters. The brightness and the colour move every \
                             frame \u{2014} never the position, which would turn every \
                             shaded edge in the film once a frame. Zero is a steady lamp.",
                        )
                        .changed();
                });

                // Only a lamp has this, because only a lamp falls off. A sun's
                // light *is* the tint on the artwork; there is no pool to draw
                // and nothing for a slider to do.
                ui.horizontal(|ui| {
                    ui.label("Glow");
                    changed |= ui
                        .add(egui::Slider::new(&mut edited.glow, 0.0..=1.0).fixed_decimals(2))
                        .on_hover_text(
                            "How much of this lamp's light you can see — the pool it lays on \
                             the stage and the halo around it. At zero it still shades and \
                             still casts, which is how you use a lamp only to model form.",
                        )
                        .changed();
                });
            }

            LightKind::Gloom {
                edge,
                facing,
                throw,
                width,
            } => {
                let mut degrees = facing.to_degrees();
                ui.horizontal(|ui| {
                    ui.label("Throws");
                    if ui
                        .add(
                            egui::Slider::new(&mut degrees, -180.0..=180.0)
                                .suffix("\u{b0}")
                                .fixed_decimals(0),
                        )
                        .on_hover_text("Which way the darkness rolls in")
                        .changed()
                    {
                        *facing = degrees.to_radians();
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Stands at");
                    changed |= ui
                        .add(egui::DragValue::new(&mut edge.x).speed(1.0).prefix("x "))
                        .changed();
                    changed |= ui
                        .add(egui::DragValue::new(&mut edge.y).speed(1.0).prefix("y "))
                        .changed();
                    ui.label(
                        RichText::new("keep it off the stage")
                            .small()
                            .weak(),
                    );
                });

                ui.horizontal(|ui| {
                    ui.label("Throw");
                    changed |= ui
                        .add(egui::Slider::new(throw, 100.0..=6000.0).suffix(" px"))
                        .on_hover_text(
                            "How far it reaches before it has faded to nothing. Long is the \
                             point: a short throw reads as a grey shape rather than as dark.",
                        )
                        .changed();
                });

                ui.horizontal(|ui| {
                    ui.label("Width");
                    changed |= ui
                        .add(egui::Slider::new(width, 100.0..=8000.0).suffix(" px"))
                        .on_hover_text(
                            "How wide the wall is. Wider than the picture unless you want a \
                             shaft of dark rather than a whole side of it.",
                        )
                        .changed();
                });
            }
        }

        // Shading, cast shadows and standing height are all questions about a
        // light with a direction. A sky has none and a gloom has none, and a
        // checkbox that cannot change the picture is worse than no checkbox.
        if edited.is_directional() {
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(&mut edited.shadows, "Shadows")
                    .on_hover_text("Cast a shadow of the artwork")
                    .changed();
                if edited.shadows {
                    // **Named, because an unlabelled slider beside a checkbox
                    // is not a setting anybody can find.** It was already the
                    // shadow's darkness and it was already adjustable; what it
                    // did not say anywhere was that it was. `Depth` rather than
                    // `Strength`: what the number moves is how dark the shadow
                    // lands, from the ground barely dimmed to black.
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut edited.shadow_strength, 0.0..=1.0)
                                .fixed_decimals(2)
                                .text("Depth"),
                        )
                        .on_hover_text(
                            "How dark the cast shadow lands. 0 leaves the ground                              untouched; 1 is a solid silhouette.",
                        )
                        .changed();
                }
            });

            // **Length, separately from the light's height.**
            //
            // How long a shadow runs is the honest consequence of how high the
            // light is — and the light's height is also what decides where the
            // terminator sits on every figure on the stage. So "shorter shadow"
            // and "keep this light where it is" are two wishes the geometry
            // will not grant at once, and an animator settles that the way they
            // always have: by drawing the shadow the shot needs.
            if edited.shadows {
                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut edited.shadow_length,
                            SHADOW_LENGTH_RANGE,
                        )
                        .fixed_decimals(2)
                        .text("Length"),
                    )
                    .on_hover_text(
                        "How far the shadow runs, against what the light's height                          says it should. 1 is that answer exactly; 0 puts the                          shadow under its caster. The direction still comes from                          the light.",
                    )
                    .changed();
            }

            // **What the shadow lands on.** Two different pictures, not two
            // settings of one: on the ground it is anchored at the figure's
            // feet and lies away from the light; on a wall it is the figure's
            // own silhouette offset behind it. See `buzz_scene::ShadowFall`.
            if edited.shadows {
                ui.horizontal(|ui| {
                    ui.label("Falls on");
                    for fall in ShadowFall::ALL {
                        changed |= ui
                            .selectable_value(&mut edited.fall, fall, fall.label())
                            .on_hover_text(match fall {
                                ShadowFall::Ground => {
                                    "The floor the artwork stands on. The shadow starts at the \
                                     figure's feet and lies away from the light, short when the \
                                     light is high and long when it is low."
                                }
                                ShadowFall::Wall => {
                                    "The surface behind it. The shadow is the figure's own \
                                     silhouette, upright and full size, offset away from the \
                                     light \u{2014} for a figure standing close in front of a \
                                     wall."
                                }
                            })
                            .changed();
                    }
                });
            }

            // Only a wall shadow asks how far the artwork stands in front of
            // it; a shadow on the floor starts at the feet, and the height that
            // matters there is the light's.
            if edited.fall == ShadowFall::Wall {
                ui.horizontal(|ui| {
                    ui.label("Stands off");
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut edited.standing_height, 0.0..=400.0)
                                .suffix(" px"),
                        )
                        .on_hover_text(
                            "How far the artwork is assumed to stand off the background. Flat \
                             drawings have no thickness, so this is what gives them a shadow on \
                             the wall at all \u{2014} layer depth adds to it.",
                        )
                        .changed();
                });
            }

            ui.horizontal(|ui| {
                ui.label("Softness");
                changed |= ui
                    .add(egui::Slider::new(&mut edited.softness, 0.0..=1.0).fixed_decimals(2))
                    .on_hover_text(
                        "How gradually the shaded side arrives. 0 is a hard light: \
                         one step from lit to shaded, an exact boundary. Turning it \
                         up feathers the terminator and wraps it further round the \
                         form, the way a bigger source does.",
                    )
                    .changed();
            });

            // **How hard the lit edge lands**, which is the other half of the
            // pair above: softness says how *wide* the modelling is, this says
            // how *strong* the bright side of it is. The band is feathered
            // either way, so turning this down softens rather than shrinks.
            ui.horizontal(|ui| {
                ui.label("Edge highlight");
                changed |= ui
                    .add(egui::Slider::new(&mut edited.glint, 0.0..=1.0).fixed_decimals(2))
                    .on_hover_text(
                        "How brightly the light catches the near side of a shape. Full is a \
                         wet, polished sheen; a drawing is usually matte, so the default sits \
                         near half. Zero leaves the lit side its own colour, with only the \
                         shaded side to model it.",
                    )
                    .changed();
            });

            // **Lightning.** The counterpart of the flicker above: one is a
            // light that never quite holds still, the other a light that holds
            // still and then does not. See `buzz_scene::Light::storm`.
            ui.horizontal(|ui| {
                ui.label("Lightning");
                changed |= ui
                    .add(egui::Slider::new(&mut edited.storm, 0.0..=1.0).fixed_decimals(2))
                    .on_hover_text(
                        "How hard and how often it strikes. A tenth is a storm on the \
                         horizon, flickering every few seconds; full is overhead and the \
                         frame goes white. Turn the light itself right down first \u{2014} a \
                         flash only reads against the dark. Zero is off.",
                    )
                    .changed();
            });
            if edited.storm > 0.0 {
                ui.horizontal(|ui| {
                    if ui
                        .button("\u{26A1} Make it a storm")
                        .on_hover_text(
                            "A cold spark colour, the light turned down to night, and an \
                             edge glow so figures are rimmed by the flash.",
                        )
                        .clicked()
                    {
                        edited.make_storm();
                        changed = true;
                    }
                    ui.label(RichText::new("scrub to see it strike").small().weak());
                });
            }

            // **The one thing lighting does that leaves the silhouette
            // bright.** Everything else \u{2014} the tint, the terminator, the
            // highlight \u{2014} happens inside the line, so a lit drawing could
            // never come up brighter than the picture around it. See
            // `buzz_light::rim_glow`.
            ui.horizontal(|ui| {
                ui.label("Edge glow");
                changed |= ui
                    .add(egui::Slider::new(&mut edited.rim, 0.0..=1.0).fixed_decimals(2))
                    .on_hover_text(
                        "A glow around the outside edge of everything this light reaches, in \
                         its colour \u{2014} Animate's Glow filter, laid by the light instead of \
                         by hand. It comes up as the light comes up and falls off with it, so \
                         a figure walking out of a lamp loses its rim on the way. Zero is off.",
                    )
                    .changed();
            });
        }

        if changed {
            out.changed = Some(edited);
        }

        // Animate this light: key its whole state at the playhead, or clear the
        // key there. The keys themselves show as a channel on the timeline.
        ui.separator();
        ui.horizontal(|ui| {
            let has_key = light
                .track
                .as_ref()
                .is_some_and(|t| t.has_key_at(state.current_frame));
            if ui
                .button(format!("\u{25C6} Key at {}", state.current_frame))
                .on_hover_text("Add a keyframe for this light at the playhead")
                .clicked()
            {
                out.key = Some(true);
            }
            if has_key && ui.button("Remove key").clicked() {
                out.key = Some(false);
            }
        });
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

        // **Edges before strength**, because it is the bigger decision. See
        // `buzz_scene::EdgeMode`: what the shaded side and the glint are
        // measured against decides whether a lit character reads as one body or
        // as a pile of separately outlined pieces.
        ui.label("Edges");
        let mut edges = rig.edges;
        for mode in EdgeMode::ALL {
            if ui
                .selectable_value(&mut edges, mode, mode.label())
                .on_hover_text(match mode {
                    EdgeMode::Off => {
                        "No shaded side and no highlight. The light still tints \
                         what it reaches, and a lamp keeps its glow, its falloff \
                         and its shadows \u{2014} for artwork already drawn with \
                         its own shading in it."
                    }
                    EdgeMode::Shapes => {
                        "A shaded side and a highlight on every shape, each \
                         measured against its own outline. Right for a layer of \
                         separate props; on a character it outlines every piece \
                         of the drawing separately."
                    }
                    EdgeMode::Figure => {
                        "One shaded side and one highlight per figure, measured \
                         around the whole character \u{2014} group, rig, symbol \
                         and all. What a light does to a body, and cheaper than \
                         Shapes on artwork made of many pieces."
                    }
                })
                .changed()
            {
                out.set_edges = Some(edges);
            }
        }

        // Greyed out rather than hidden when nothing is being modelled: the
        // strength is still set, and the edges come back at it.
        ui.label("Modelling");
        let mut modelling = rig.modelling;
        if ui
            .add_enabled(
                rig.edges != EdgeMode::Off,
                egui::Slider::new(&mut modelling, 0.0..=1.0).fixed_decimals(2),
            )
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
/// # Why it is this big, and why the middle behaves specially
///
/// It was thirty-four points across, with the whole range from horizon to
/// overhead squeezed into that radius and **overhead at the exact centre** — a
/// single point. Worse, a guard skipped any drag within a point of the middle,
/// so ninety degrees was not merely hard to hit, it was unreachable: the
/// closest the dial could get was about eighty-seven. And near the centre a
/// pointer movement of one pixel swings the bearing right round, so the sun
/// jitters through every azimuth on the way up.
///
/// That is the "the sun only goes in a small circle" report, and the "I cannot
/// get it high enough" one before it. The dial is now big enough to aim, the
/// middle of it is a **target rather than a point** — anywhere inside it means
/// straight overhead, and the bearing is left alone there because a sun
/// directly above has no bearing worth reading — and the rings are drawn where
/// the angles actually are.
///
/// Returns whether the drag changed anything.
fn sun_dial(ui: &mut Ui, azimuth: &mut f64, elevation: &mut f64) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SUN_DIAL_RADIUS * 2.0 + 10.0),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let centre = egui::pos2(rect.left() + SUN_DIAL_RADIUS + 6.0, rect.center().y);
    let quarter = std::f64::consts::FRAC_PI_2;

    painter.circle_filled(centre, SUN_DIAL_RADIUS, Palette::panel());
    painter.circle_stroke(
        centre,
        SUN_DIAL_RADIUS,
        egui::Stroke::new(1.0, Palette::border()),
    );
    // Where the angles are: the rim is the horizon and the middle is overhead,
    // the way a fisheye photograph of the sky maps it, so these sit at thirty
    // and sixty degrees up. They used to be one ring at half the radius,
    // described in a comment as the horizon — which is the rim — and an
    // animator who read it as the edge of the dial never took the sun above
    // forty-five.
    for up in [30.0f64, 60.0] {
        let t = 1.0 - up.to_radians() / quarter;
        painter.circle_stroke(
            centre,
            SUN_DIAL_OVERHEAD + (SUN_DIAL_RADIUS - SUN_DIAL_OVERHEAD) * t as f32,
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18)),
        );
    }

    let mut changed = false;
    if let Some(pos) = (response.dragged() || response.clicked())
        .then(|| ui.ctx().input(|i| i.pointer.interact_pos()))
        .flatten()
    {
        let offset = pos - centre;
        // The bearing is left exactly as it was at the top: a sun directly
        // above has no direction across the picture to read, and taking one
        // from a two-pixel offset is how the old dial span the azimuth wildly
        // on the way up.
        if offset.length() > SUN_DIAL_OVERHEAD {
            *azimuth = (offset.y as f64).atan2(offset.x as f64);
        }
        *elevation = sun_dial_elevation(offset.length());
        changed = true;
    }

    // Where the sun sits on the dial, and where its shadow runs.
    let t = 1.0 - (*elevation / quarter).clamp(0.0, 1.0);
    let (sin_a, cos_a) = azimuth.sin_cos();
    let reach = SUN_DIAL_OVERHEAD + (SUN_DIAL_RADIUS - SUN_DIAL_OVERHEAD) * t as f32;
    let arm = egui::vec2(cos_a as f32, sin_a as f32) * reach;

    painter.line_segment(
        [centre, centre - arm],
        egui::Stroke::new(2.0, Color32::from_rgba_unmultiplied(0, 0, 0, 120)),
    );
    painter.line_segment(
        [centre, centre + arm],
        egui::Stroke::new(1.5, Palette::border()),
    );
    painter.circle_filled(centre + arm, 6.0, Color32::from_rgb(0xFF, 0xD9, 0x6A));

    painter.text(
        egui::pos2(centre.x + SUN_DIAL_RADIUS + 12.0, rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        format!("{:.0}\u{b0}", azimuth.to_degrees()),
        egui::FontId::proportional(11.0),
        Palette::text(),
    );
    painter.text(
        egui::pos2(centre.x + SUN_DIAL_RADIUS + 12.0, rect.center().y + 8.0),
        egui::Align2::LEFT_CENTER,
        format!("{:.0}\u{b0} up", elevation.to_degrees()),
        egui::FontId::proportional(11.0),
        Palette::text_dim(),
    );

    changed
}

/// The dial's rim, which is the horizon.
pub const SUN_DIAL_RADIUS: f32 = 56.0;
/// Anywhere within this of the middle of the dial is straight overhead.
///
/// A target rather than a point. Ninety degrees used to live at the exact
/// centre, which no pointer lands on, behind a guard that ignored the middle
/// entirely — so the top of the sun's range could not be reached by dragging at
/// all.
pub const SUN_DIAL_OVERHEAD: f32 = 5.0;

/// **Where the sun dial puts a pointer**, as the elevation it means.
///
/// `from_centre` is how far the pointer is from the middle of the dial, in
/// points; the answer is in radians, from zero on the rim to a quarter turn
/// inside the overhead target.
pub fn sun_dial_elevation(from_centre: f32) -> f64 {
    let quarter = std::f64::consts::FRAC_PI_2;
    if from_centre <= SUN_DIAL_OVERHEAD {
        return quarter;
    }
    let span = (SUN_DIAL_RADIUS - SUN_DIAL_OVERHEAD).max(1.0);
    let t = ((from_centre - SUN_DIAL_OVERHEAD) / span).clamp(0.0, 1.0) as f64;
    ((1.0 - t) * quarter).clamp(0.0, quarter)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The report: the sun only went round in a small circle, and would not
    /// go high.**
    ///
    /// The dial mapped horizon-to-overhead across its radius with overhead at
    /// the exact centre — a single point — and then refused any drag within a
    /// point of that centre. So the top of the range was not merely fiddly, it
    /// was unreachable: the closest the dial could be dragged was about
    /// eighty-seven degrees, on a disc thirty-four points across.
    #[test]
    fn the_sun_dial_reaches_straight_overhead() {
        let quarter = std::f64::consts::FRAC_PI_2;

        // Dead centre, and anywhere in the target around it, is overhead.
        for from_centre in [0.0, 1.0, SUN_DIAL_OVERHEAD] {
            assert_eq!(
                sun_dial_elevation(from_centre),
                quarter,
                "{from_centre} points from the middle should be straight overhead"
            );
        }
        // The rim is the horizon, and past it stays there.
        assert_eq!(sun_dial_elevation(SUN_DIAL_RADIUS), 0.0);
        assert_eq!(sun_dial_elevation(SUN_DIAL_RADIUS * 3.0), 0.0);
    }

    /// The range in between is spread over the dial, and runs the right way
    /// round: further out is lower.
    #[test]
    fn the_sun_dial_falls_from_overhead_at_the_middle_to_the_horizon_at_the_rim() {
        let mut last = f64::INFINITY;
        for step in 0..=20 {
            let at = SUN_DIAL_RADIUS * step as f32 / 20.0;
            let elevation = sun_dial_elevation(at);
            assert!(
                elevation <= last + 1e-9,
                "the sun rose on the way out at {at}: {elevation} after {last}"
            );
            last = elevation;
        }

        // And the middle of the dial is somewhere near the middle of the range,
        // so the useful angles are not all crowded into a few pixels.
        let middle = sun_dial_elevation(SUN_DIAL_RADIUS / 2.0).to_degrees();
        assert!(
            (35.0..=55.0).contains(&middle),
            "half way out is {middle} degrees up, which is not half the range"
        );
    }

    /// It also has to be big enough to aim at. Thirty-four points across put
    /// ninety degrees of elevation into seventeen pixels.
    #[test]
    fn the_sun_dial_is_big_enough_to_aim() {
        assert!(
            SUN_DIAL_RADIUS >= 48.0,
            "the dial is {SUN_DIAL_RADIUS} points across the radius"
        );
        assert!(
            SUN_DIAL_OVERHEAD >= 3.0,
            "the overhead target is {SUN_DIAL_OVERHEAD} points and a pointer will miss it"
        );
    }

    fn rig(lights: Vec<Light>) -> LightRig {
        LightRig {
            lights,
            enabled: true,
            ..LightRig::default()
        }
    }

    #[test]
    fn an_empty_panel_offers_every_kind_and_changes_nothing() {
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
            Light::new(LightId(4), "Gloom", LightKind::gloom(Point::new(-20.0, 10.0))),
        ]);

        for selected in [
            None,
            Some(LightId(1)),
            Some(LightId(2)),
            Some(LightId(3)),
            Some(LightId(4)),
        ] {
            let mut state = LightPanelState {
                selected,
                gizmos: true,
                ..LightPanelState::default()
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
            ..LightPanelState::default()
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
