//! The Filters panel — Animate's Properties ▸ Filters, and its Blend list.
//!
//! Laid out as Animate lays it out, because the muscle memory is worth more
//! than any improvement available here: a list of filters with an enable box
//! each, a `+` to add one, and the selected filter's parameters underneath.
//!
//! Two departures, both deliberate and both recorded in PROGRESS.md §7:
//!
//! * **Filters can go on a layer**, not only on a symbol instance, so there is
//!   a target switch at the top. Animate has no such thing; blurring a
//!   background there means selecting it all and making it a symbol first.
//! * **Four of Animate's blend modes are missing** — Subtract, Invert, Alpha
//!   and Erase — because they are Flash's own compositing operators and there
//!   is nothing to express them with. They are left out rather than mapped
//!   onto something that looks nearly right.

use buzz_scene::{BevelKind, Blend, ColorAdjust, Filter, FilterKind, Modifier, Quality};
use egui::{RichText, Ui};

use crate::panels::{from_egui, to_egui};

/// What the panel is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterTarget {
    /// The selected object.
    #[default]
    Object,
    /// The active layer, as a whole.
    Layer,
}

/// Panel state that is not part of the document.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FilterPanelState {
    pub target: FilterTarget,
    /// Which row's parameters are shown.
    pub selected: usize,
}

/// What the user did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FilterResponse {
    /// Add this filter to the current target.
    pub add: Option<FilterKind>,
    /// Remove the filter at this index.
    pub remove: Option<usize>,
    /// Replace the filter at this index.
    pub changed: Option<(usize, Filter)>,
    /// Move a filter up (`-1`) or down (`+1`) the stack.
    pub reorder: Option<(usize, i32)>,
    /// A new blend mode for the selected object.
    pub set_blend: Option<Blend>,
    /// Attach this live modifier to the selected object.
    pub add_modifier: Option<Modifier>,
    /// Remove the modifier at this index.
    pub remove_modifier: Option<usize>,
    /// Replace the modifier at this index (a parameter was dragged).
    pub set_modifier: Option<(usize, Modifier)>,
}

/// Draw the panel.
///
/// `filters` is whatever the target currently has; `blend` is the selected
/// object's blend mode, or `None` when nothing is selected.
pub fn filter_panel(
    ui: &mut Ui,
    filters: &[Filter],
    blend: Option<Blend>,
    modifiers: &[Modifier],
    state: &mut FilterPanelState,
    has_selection: bool,
) -> FilterResponse {
    let mut out = FilterResponse::default();

    ui.horizontal(|ui| {
        ui.heading("Filters");
        if !filters.is_empty() {
            ui.label(RichText::new(format!("{}", filters.len())).small().weak());
        }
    });

    // -- what the filters are being put on ----------------------------------
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.target, FilterTarget::Object, "Object")
            .on_hover_text("Filters on the selected artwork");
        ui.selectable_value(&mut state.target, FilterTarget::Layer, "Layer")
            .on_hover_text("Filters on the whole layer — not something Animate can do");
    });

    if state.target == FilterTarget::Object && !has_selection {
        ui.label(
            RichText::new("Select artwork to filter it, or switch to Layer.")
                .small()
                .weak(),
        );
        return out;
    }

    // -- blend mode ---------------------------------------------------------
    if state.target == FilterTarget::Object
        && let Some(current) = blend
    {
        ui.horizontal(|ui| {
            ui.label("Blend");
            egui::ComboBox::from_id_salt("object-blend")
                .selected_text(current.label())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for mode in Blend::ALL {
                        if ui.selectable_label(mode == current, mode.label()).clicked() {
                            out.set_blend = Some(mode);
                        }
                    }
                });
        });
    }

    // -- live motion (procedural modifiers) ---------------------------------
    if state.target == FilterTarget::Object {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Live Motion").strong());
            if !modifiers.is_empty() {
                ui.label(RichText::new(format!("{}", modifiers.len())).small().weak());
            }
            ui.menu_button("+", |ui| {
                if ui.button("Look At").clicked() {
                    out.add_modifier = Some(Modifier::LookAt { x: 0.0, y: 0.0 });
                    ui.close();
                }
                if ui.button("Squash & Stretch").clicked() {
                    out.add_modifier = Some(Modifier::AutoSquashStretch { amount: 0.01 });
                    ui.close();
                }
                // A resting adult: fourteen breaths a minute, at the depth the
                // modifier calls one. See `Modifier::Breathe`.
                if ui
                    .button("Breathe")
                    .on_hover_text(
                        "The chest rises and falls on every held pose, so a character \
                         standing still reads as alive rather than as a picture.",
                    )
                    .clicked()
                {
                    out.add_modifier = Some(Modifier::Breathe {
                        rate: 14.0,
                        depth: 1.0,
                    });
                    ui.close();
                }
                // A resting rate: one blink about every five seconds, each one
                // four frames long at twenty-four. See `Modifier::Blink`.
                if ui
                    .button("Blink")
                    .on_hover_text(
                        "The eye shuts and opens every few seconds. Put it on the eye \
                         artwork, not the whole character \u{2014} the lid falls on \
                         whatever drawing you give it.",
                    )
                    .clicked()
                {
                    out.add_modifier = Some(Modifier::Blink {
                        rate: 12.0,
                        duration: 0.16,
                    });
                    ui.close();
                }
                // A head is very nearly a cylinder, so the default is the
                // whole of one. See `Modifier::Turn`.
                if ui
                    .button("Turn")
                    .on_hover_text(
                        "Turns a face by carrying its features round a cylinder \
                         instead of rotating the drawing. Group the head first: \
                         the backmost part is the form, the rest are features. \
                         The angle is the object's own 3D yaw, keyed like anything \
                         else.",
                    )
                    .clicked()
                {
                    out.add_modifier = Some(Modifier::Turn { round: 1.0 });
                    ui.close();
                }
                // A breeze through a mid-stiff tree: leans an eighth of its own
                // height at a full gust, a gust every five seconds or so.
                if ui
                    .button("Sway")
                    .on_hover_text(
                        "Wind: the drawing bends downwind from its base in gusts. Trees, \
                         grass, banners, hanging signs.",
                    )
                    .clicked()
                {
                    out.add_modifier = Some(Modifier::Sway {
                        amount: 0.12,
                        rate: 0.2,
                    });
                    ui.close();
                }
                if ui
                    .button("Drift")
                    .on_hover_text(
                        "A steady move that loops: clouds crossing the sky, the surface of \
                         a river, a street behind a window. Set the wrap to how far it must \
                         travel before it may start again.",
                    )
                    .clicked()
                {
                    out.add_modifier = Some(Modifier::Drift {
                        dx: 12.0,
                        dy: 0.0,
                        span: 0.0,
                        phase: 0.0,
                    });
                    ui.close();
                }
            });
        });
        if modifiers.is_empty() {
            ui.label(
                RichText::new(
                    "Springs and wiggles are added from the Scene menu; look-at, \
                     squash & stretch, breathing, blinking, turning and sway, here.",
                )
                .small()
                .weak(),
            );
        }
        for (i, modifier) in modifiers.iter().enumerate() {
            let mut edited = *modifier;
            let mut changed = false;
            ui.horizontal(|ui| {
                if ui.small_button("\u{2715}").on_hover_text("Remove").clicked() {
                    out.remove_modifier = Some(i);
                }
                ui.label(modifier.label());
                match &mut edited {
                    Modifier::Wiggle {
                        amplitude,
                        frequency,
                    } => {
                        changed |= ui.add(egui::DragValue::new(amplitude).prefix("amp ").speed(0.2)).changed();
                        changed |= ui.add(egui::DragValue::new(frequency).prefix("Hz ").speed(0.05)).changed();
                    }
                    Modifier::Spring {
                        stiffness, damping, ..
                    } => {
                        changed |= ui.add(egui::DragValue::new(stiffness).prefix("k ").speed(1.0)).changed();
                        changed |= ui.add(egui::DragValue::new(damping).prefix("d ").speed(0.2)).changed();
                    }
                    Modifier::LookAt { x, y } => {
                        changed |= ui.add(egui::DragValue::new(x).prefix("x ")).changed();
                        changed |= ui.add(egui::DragValue::new(y).prefix("y ")).changed();
                    }
                    Modifier::AutoSquashStretch { amount } => {
                        changed |= ui
                            .add(egui::DragValue::new(amount).prefix("amount ").speed(0.001))
                            .changed();
                    }
                    Modifier::Breathe { rate, depth } => {
                        changed |= ui
                            .add(
                                egui::DragValue::new(rate)
                                    .prefix("bpm ")
                                    .speed(0.5)
                                    .range(0.5..=120.0),
                            )
                            .on_hover_text("Breaths per minute: 14 at rest, 30 after running")
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(depth)
                                    .prefix("depth ")
                                    .speed(0.05)
                                    .range(0.0..=4.0),
                            )
                            .changed();
                    }
                    Modifier::Blink { rate, duration } => {
                        changed |= ui
                            .add(
                                egui::DragValue::new(rate)
                                    .prefix("bpm ")
                                    .speed(0.5)
                                    .range(0.5..=240.0),
                            )
                            .on_hover_text(
                                "Blinks per minute: 12 is at rest, and much past 20 \
                                 starts to read as nerves",
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(duration)
                                    .prefix("s ")
                                    .speed(0.01)
                                    .range(0.02..=4.0),
                            )
                            .on_hover_text(
                                "How long one blink takes. 0.16s is a real one \u{2014} \
                                 four frames at 24fps.",
                            )
                            .changed();
                    }
                    Modifier::Turn { round } => {
                        changed |= ui
                            .add(
                                egui::DragValue::new(round)
                                    .prefix("round ")
                                    .speed(0.02)
                                    .range(0.0..=1.0),
                            )
                            .on_hover_text(
                                "How much of a cylinder the drawing is: 1.0 for a \
                                 head, lower for something flatter, 0 for a board \
                                 that only slides",
                            )
                            .changed();
                    }
                    Modifier::Drift {
                        dx,
                        dy,
                        span,
                        phase,
                    } => {
                        changed |= ui
                            .add(egui::DragValue::new(dx).prefix("dx ").speed(0.5))
                            .on_hover_text("Document units per second, across")
                            .changed();
                        changed |= ui
                            .add(egui::DragValue::new(dy).prefix("dy ").speed(0.5))
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(span)
                                    .prefix("wrap ")
                                    .speed(2.0)
                                    .range(0.0..=1.0e6),
                            )
                            .on_hover_text("How far it travels before it loops. 0 never loops.")
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(phase)
                                    .prefix("start ")
                                    .speed(0.01)
                                    .range(0.0..=1.0),
                            )
                            .on_hover_text(
                                "How far into the loop it already is. Give several objects \
                                 different values and they scatter instead of crossing in \
                                 formation.",
                            )
                            .changed();
                    }
                    Modifier::Sway { amount, rate } => {
                        changed |= ui
                            .add(
                                egui::DragValue::new(amount)
                                    .prefix("lean ")
                                    .speed(0.01)
                                    .range(0.0..=2.0),
                            )
                            .on_hover_text("How far the top leans, as a share of its own height")
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(rate)
                                    .prefix("Hz ")
                                    .speed(0.01)
                                    .range(0.01..=20.0),
                            )
                            .on_hover_text("Gusts per second")
                            .changed();
                    }
                }
            });
            if changed {
                out.set_modifier = Some((i, edited));
            }
        }
    }

    // -- add ----------------------------------------------------------------
    ui.horizontal(|ui| {
        ui.menu_button("+ Add", |ui| {
            for kind in FilterKind::all() {
                if ui.button(kind.label()).clicked() {
                    out.add = Some(kind);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Add a filter to the top of the stack");

        if filters.is_empty() {
            ui.label(RichText::new("none").small().weak());
        }
    });

    if filters.is_empty() {
        return out;
    }

    ui.separator();

    // -- the stack ----------------------------------------------------------
    for (index, filter) in filters.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut enabled = filter.enabled;
            if ui
                .checkbox(&mut enabled, "")
                .on_hover_text("Switch this filter off without losing its settings")
                .changed()
            {
                out.changed = Some((
                    index,
                    Filter {
                        enabled,
                        ..filter.clone()
                    },
                ));
            }

            if ui
                .selectable_label(state.selected == index, filter.label())
                .clicked()
            {
                state.selected = index;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("x")
                    .on_hover_text("Delete this filter")
                    .clicked()
                {
                    out.remove = Some(index);
                }
                // Order matters: a glow under a blur is not a blurred glow.
                if ui
                    .add_enabled(index + 1 < filters.len(), egui::Button::new("v").small())
                    .on_hover_text("Move down the stack")
                    .clicked()
                {
                    out.reorder = Some((index, 1));
                }
                if ui
                    .add_enabled(index > 0, egui::Button::new("^").small())
                    .on_hover_text("Move up the stack")
                    .clicked()
                {
                    out.reorder = Some((index, -1));
                }
            });
        });
    }

    // -- the selected filter's parameters -----------------------------------
    let index = state.selected.min(filters.len() - 1);
    let filter = &filters[index];
    ui.separator();
    ui.label(RichText::new(filter.label()).strong());

    let mut edited = filter.clone();
    if parameters(ui, &mut edited.kind) {
        out.changed = Some((index, edited));
    }

    out
}

/// The parameters for one filter. Returns whether anything changed.
fn parameters(ui: &mut Ui, kind: &mut FilterKind) -> bool {
    let mut changed = false;

    match kind {
        FilterKind::Blur { x, y, quality } => {
            changed |= blur_fields(ui, x, y);
            changed |= quality_field(ui, quality);
        }

        FilterKind::DropShadow {
            x,
            y,
            strength,
            angle,
            distance,
            color,
            inner,
            knockout,
            hide_object,
            quality,
        } => {
            changed |= blur_fields(ui, x, y);
            changed |= strength_field(ui, strength);
            changed |= angle_field(ui, angle);
            changed |= distance_field(ui, distance);
            changed |= colour_field(ui, "Colour", color);
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(inner, "Inner")
                    .on_hover_text("Shade the inside of the shape instead of behind it")
                    .changed();
                changed |= ui
                    .checkbox(knockout, "Knockout")
                    .on_hover_text("Keep the shadow, drop the artwork")
                    .changed();
                changed |= ui
                    .checkbox(hide_object, "Hide object")
                    .on_hover_text("The shadow with nothing casting it")
                    .changed();
            });
            changed |= quality_field(ui, quality);
        }

        FilterKind::Glow {
            x,
            y,
            strength,
            color,
            inner,
            knockout,
            quality,
        } => {
            changed |= blur_fields(ui, x, y);
            changed |= strength_field(ui, strength);
            changed |= colour_field(ui, "Colour", color);
            ui.horizontal(|ui| {
                changed |= ui.checkbox(inner, "Inner").changed();
                changed |= ui.checkbox(knockout, "Knockout").changed();
            });
            changed |= quality_field(ui, quality);
        }

        FilterKind::Bevel {
            x,
            y,
            strength,
            angle,
            distance,
            highlight,
            shadow,
            kind,
            knockout,
            quality,
        } => {
            changed |= blur_fields(ui, x, y);
            changed |= strength_field(ui, strength);
            changed |= angle_field(ui, angle);
            changed |= distance_field(ui, distance);
            changed |= colour_field(ui, "Highlight", highlight);
            changed |= colour_field(ui, "Shadow", shadow);
            ui.horizontal(|ui| {
                ui.label("Type");
                for option in BevelKind::ALL {
                    if ui
                        .selectable_label(*kind == option, option.label())
                        .clicked()
                    {
                        *kind = option;
                        changed = true;
                    }
                }
            });
            changed |= ui.checkbox(knockout, "Knockout").changed();
            changed |= quality_field(ui, quality);
        }

        FilterKind::Adjust(adjust) => {
            changed |= adjust_fields(ui, adjust);
        }
        FilterKind::GradientMap(map) => {
            changed |= colour_field(ui, "Shadow", &mut map.shadow);
            changed |= colour_field(ui, "Highlight", &mut map.highlight);
        }
    }

    changed
}

fn blur_fields(ui: &mut Ui, x: &mut f64, y: &mut f64) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Blur X");
        changed |= ui
            .add(egui::Slider::new(x, 0.0..=100.0).fixed_decimals(1))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Blur Y");
        changed |= ui
            .add(egui::Slider::new(y, 0.0..=100.0).fixed_decimals(1))
            .changed();
    });
    changed
}

fn strength_field(ui: &mut Ui, strength: &mut f64) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Strength");
        changed |= ui
            .add(egui::Slider::new(strength, 0.0..=1.0).fixed_decimals(2))
            .changed();
    });
    changed
}

/// Angle in **degrees**, like Animate's — nobody aims a shadow in radians.
fn angle_field(ui: &mut Ui, angle: &mut f64) -> bool {
    let mut degrees = angle.to_degrees();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Angle");
        if ui
            .add(
                egui::Slider::new(&mut degrees, -180.0..=180.0)
                    .suffix("\u{b0}")
                    .fixed_decimals(0),
            )
            .changed()
        {
            *angle = degrees.to_radians();
            changed = true;
        }
    });
    changed
}

fn distance_field(ui: &mut Ui, distance: &mut f64) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Distance");
        changed |= ui
            .add(egui::Slider::new(distance, -60.0..=60.0).fixed_decimals(1))
            .changed();
    });
    changed
}

fn colour_field(ui: &mut Ui, label: &str, color: &mut peniko::Color) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        let mut rgba = to_egui(*color);
        if ui.color_edit_button_srgba(&mut rgba).changed() {
            *color = from_egui(rgba);
            changed = true;
        }
    });
    changed
}

fn quality_field(ui: &mut Ui, quality: &mut Quality) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Quality");
        for option in Quality::ALL {
            if ui
                .selectable_label(*quality == option, option.label())
                .on_hover_text("How many bands the soft edge is built from")
                .clicked()
            {
                *quality = option;
                changed = true;
            }
        }
    });
    changed
}

fn adjust_fields(ui: &mut Ui, adjust: &mut ColorAdjust) -> bool {
    let mut changed = false;
    for (label, value, range) in [
        ("Brightness", &mut adjust.brightness, -100.0..=100.0),
        ("Contrast", &mut adjust.contrast, -100.0..=100.0),
        ("Saturation", &mut adjust.saturation, -100.0..=100.0),
        ("Hue", &mut adjust.hue, -180.0..=180.0),
    ] {
        ui.horizontal(|ui| {
            ui.label(label);
            changed |= ui
                .add(egui::Slider::new(value, range).fixed_decimals(0))
                .changed();
        });
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(
        filters: &[Filter],
        state: &mut FilterPanelState,
        has_selection: bool,
    ) -> FilterResponse {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut response = FilterResponse::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            response = filter_panel(ui, filters, Some(Blend::Normal), &[], state, has_selection);
        });
        response
    }

    #[test]
    fn an_empty_panel_changes_nothing() {
        let mut state = FilterPanelState::default();
        assert_eq!(draw(&[], &mut state, true), FilterResponse::default());
    }

    /// With nothing selected the panel says so rather than showing controls
    /// that would go nowhere.
    #[test]
    fn with_no_selection_the_object_tab_explains_itself() {
        let mut state = FilterPanelState::default();
        assert_eq!(draw(&[], &mut state, false), FilterResponse::default());
    }

    #[test]
    fn every_filter_draws_its_parameters() {
        for kind in FilterKind::all() {
            let filters = vec![Filter::new(kind)];
            let mut state = FilterPanelState::default();
            assert_eq!(draw(&filters, &mut state, true), FilterResponse::default());
        }
    }

    /// A stack of several, with the selection on each in turn.
    #[test]
    fn a_stack_draws_with_any_row_selected() {
        let filters: Vec<Filter> = FilterKind::all().into_iter().map(Filter::new).collect();
        for selected in 0..filters.len() {
            let mut state = FilterPanelState {
                target: FilterTarget::Object,
                selected,
            };
            let _ = draw(&filters, &mut state, true);
        }
    }

    /// A selection index left over from a longer stack must not panic the
    /// panel — deleting the last filter is the ordinary way to get one.
    #[test]
    fn a_stale_selection_falls_back_to_the_last_row() {
        let filters = vec![Filter::new(FilterKind::blur())];
        let mut state = FilterPanelState {
            target: FilterTarget::Object,
            selected: 7,
        };
        let _ = draw(&filters, &mut state, true);
    }

    /// The Layer tab works with nothing selected, which is the point of it.
    #[test]
    fn the_layer_tab_needs_no_selection() {
        let filters = vec![Filter::new(FilterKind::glow())];
        let mut state = FilterPanelState {
            target: FilterTarget::Layer,
            selected: 0,
        };
        let _ = draw(&filters, &mut state, false);
    }
}
