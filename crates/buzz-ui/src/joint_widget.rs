//! **How far a joint may turn**, as one widget shared by the Rig panel and the
//! Bone tool's Tool Options.
//!
//! Limits are shown **relative to the bone's rest angle** — the angle it was
//! drawn at — in degrees: an elbow that bends 150° one way and not at all the
//! other reads as "−150° … 0°" whatever angle the arm happened to be drawn at.
//! They are stored the way every limit is stored, absolute and relative to the
//! parent ([`JointLimits::from_relative`]), so saved documents and the IK
//! solver never see the difference.
//!
//! Screen space, y down, as everywhere in the rig: a positive angle turns
//! **clockwise** on screen.

use buzz_rig::{Bone, Direction, JointLimits, RotationMode, wrap_pi};
use egui::{RichText, Ui};

/// One joint's limits the way the widget edits them: how it turns, which way,
/// and the range **relative to the rest angle, in degrees** (`min ≤ 0 ≤ max`).
///
/// The same shape as a row of [`buzz_rig::JointLimitDefaults`], so the
/// defaults table is drawn with the same pieces as a bone.
pub type LimitSpec = (RotationMode, Direction, f64, f64);

/// What the joint widget asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JointEdit {
    /// Give bone `index` these limits — absolute, about its own rest angle, as
    /// a bone stores them. `None` frees the joint.
    Set {
        index: usize,
        limits: Option<JointLimits>,
    },
    /// Give bone `index` these limits (absolute, about its rest angle), and
    /// every bone hanging off it the same range about *its own* rest angle.
    ApplyToChain {
        index: usize,
        limits: Option<JointLimits>,
    },
    /// Give every bone of the rig this range. **Relative to the rest angle**
    /// (radians, `min ≤ 0 ≤ max`), because there is no one bone for it to be
    /// absolute about: each bone gets it re-expressed about its own rest angle.
    ApplyToAll { limits: Option<JointLimits> },
}

/// Plain words for which way is which, shown wherever an angle is typed.
const SIGN_HOVER: &str = "Degrees away from the angle the bone was drawn at. Positive turns \
                          clockwise on screen (y points down), negative turns counter-clockwise";

/// A bone's limits as the widget shows them.
pub fn spec_of(bone: &Bone) -> LimitSpec {
    let rest = bone.rest_angle;
    match (bone.rotation_mode(), bone.limits) {
        (RotationMode::Limited, Some(limits)) => {
            let (min, max) = limits.relative_to(rest);
            (
                RotationMode::Limited,
                limits.direction(rest),
                min.to_degrees(),
                max.to_degrees(),
            )
        }
        (RotationMode::Locked, _) => (RotationMode::Locked, Direction::Both, 0.0, 0.0),
        _ => (RotationMode::Free, Direction::Both, 0.0, 0.0),
    }
}

/// The range a spec stands for, **relative to the rest angle, in radians**,
/// with its direction applied: `None` for a free joint, `0..0` for a locked
/// one. A one-way direction keeps only its side of the range.
pub fn relative_limits(spec: LimitSpec) -> Option<JointLimits> {
    let (mode, direction, min, max) = spec;
    match mode {
        RotationMode::Free => None,
        RotationMode::Locked => Some(JointLimits::new(0.0, 0.0)),
        RotationMode::Limited => {
            let (mut min, mut max) = (min.min(max), min.max(max));
            match direction {
                Direction::Both => {}
                Direction::Clockwise => {
                    min = min.max(0.0);
                    max = max.max(0.0);
                }
                Direction::CounterClockwise => {
                    min = min.min(0.0);
                    max = max.min(0.0);
                }
            }
            Some(JointLimits::new(min.to_radians(), max.to_radians()))
        }
    }
}

/// The limits a spec stands for on a bone drawn at `rest`, stored the way the
/// bone stores them.
pub fn limits_about(rest: f64, spec: LimitSpec) -> Option<JointLimits> {
    relative_limits(spec).map(|r| JointLimits::from_relative(rest, r.min, r.max))
}

/// Switch how the joint turns. A joint that starts being limited starts at a
/// quarter turn either way of its rest angle — wide enough to be usable at
/// once, narrow enough to show the limit is doing something — unless it
/// already had a range to go back to.
fn set_mode(spec: &mut LimitSpec, mode: RotationMode) {
    if mode == RotationMode::Limited && (spec.0 != RotationMode::Limited)
        && (spec.3 - spec.2).abs() <= 1e-9
    {
        let quarter = crate::rig_panel::FRAC_QUARTER.to_degrees();
        *spec = (RotationMode::Limited, Direction::Both, -quarter, quarter);
        return;
    }
    spec.0 = mode;
}

/// Switch which way a limited joint may turn, keeping as much of the range as
/// still makes sense: a one-way joint keeps its own side, and a side that has
/// to be made up borrows the other side's reach.
fn set_direction(spec: &mut LimitSpec, direction: Direction) {
    let reach = spec.3.max(-spec.2);
    let reach = if reach > 1e-9 {
        reach
    } else {
        crate::rig_panel::FRAC_QUARTER.to_degrees()
    };
    match direction {
        Direction::Both => {
            if spec.2 > -1e-9 {
                spec.2 = -reach;
            }
            if spec.3 < 1e-9 {
                spec.3 = reach;
            }
        }
        Direction::Clockwise => {
            spec.2 = 0.0;
            if spec.3 < 1e-9 {
                spec.3 = reach;
            }
        }
        Direction::CounterClockwise => {
            spec.3 = 0.0;
            if spec.2 > -1e-9 {
                spec.2 = -reach;
            }
        }
    }
    spec.1 = direction;
}

/// **Row piece: Free | Limited | Locked.** Returns whether it changed.
pub fn mode_buttons(ui: &mut Ui, spec: &mut LimitSpec) -> bool {
    let mut changed = false;
    for (mode, label, hover) in [
        (
            RotationMode::Free,
            "Free",
            "The joint turns any amount, either way",
        ),
        (
            RotationMode::Limited,
            "Limited",
            "The joint turns only between the two angles you set",
        ),
        (
            RotationMode::Locked,
            "Locked",
            "The joint does not turn at all: the bone holds the angle it was drawn at",
        ),
    ] {
        if ui
            .selectable_label(spec.0 == mode, RichText::new(label).small())
            .on_hover_text(hover)
            .clicked()
            && spec.0 != mode
        {
            set_mode(spec, mode);
            changed = true;
        }
    }
    changed
}

/// **Row piece: ⟲ Both | ↻ Clockwise | ↺ Counter.** Returns whether it
/// changed.
pub fn direction_buttons(ui: &mut Ui, spec: &mut LimitSpec) -> bool {
    let mut changed = false;
    for (direction, label, hover) in [
        (
            Direction::Both,
            "\u{27F2} Both",
            "The joint may turn either way from where it was drawn",
        ),
        (
            Direction::Clockwise,
            "\u{21BB} Clockwise",
            "The joint may only turn clockwise on screen from where it was drawn",
        ),
        (
            Direction::CounterClockwise,
            "\u{21BA} Counter",
            "The joint may only turn counter-clockwise on screen from where it was drawn",
        ),
    ] {
        if ui
            .selectable_label(spec.1 == direction, RichText::new(label).small())
            .on_hover_text(hover)
            .clicked()
            && spec.1 != direction
        {
            set_direction(spec, direction);
            changed = true;
        }
    }
    changed
}

/// **Row piece: Min / Max**, in degrees from the rest angle, each range held
/// to its own side of zero and pinned at zero by a one-way direction. Returns
/// whether either changed.
///
/// Existing values are not clamped on display: a limit saved by an older
/// version can sit outside these ranges, and clamping it silently would edit
/// the document every frame.
pub fn range_values(ui: &mut Ui, spec: &mut LimitSpec) -> bool {
    let mut changed = false;
    let direction = spec.1;

    ui.label(RichText::new("Min").small()).on_hover_text(SIGN_HOVER);
    if direction == Direction::Clockwise {
        ui.label(RichText::new("0°").small().weak()).on_hover_text(
            "Clockwise only: the joint cannot turn counter-clockwise past where it was drawn",
        );
    } else {
        changed |= ui
            .add(
                egui::DragValue::new(&mut spec.2)
                    .speed(1.0)
                    .range(-180.0..=0.0)
                    .clamp_existing_to_range(false)
                    .suffix("°"),
            )
            .on_hover_text(SIGN_HOVER)
            .changed();
    }

    ui.label(RichText::new("Max").small()).on_hover_text(SIGN_HOVER);
    if direction == Direction::CounterClockwise {
        ui.label(RichText::new("0°").small().weak()).on_hover_text(
            "Counter-clockwise only: the joint cannot turn clockwise past where it was drawn",
        );
    } else {
        changed |= ui
            .add(
                egui::DragValue::new(&mut spec.3)
                    .speed(1.0)
                    .range(0.0..=180.0)
                    .clamp_existing_to_range(false)
                    .suffix("°"),
            )
            .on_hover_text(SIGN_HOVER)
            .changed();
    }
    changed
}

/// **The joint widget** for bone `index`: how it turns, which way, how far,
/// where it is now, and the two buttons that copy all of that along the rig.
pub fn joint_limits(ui: &mut Ui, bone: &Bone, index: usize) -> Option<JointEdit> {
    let rest = bone.rest_angle;
    let mut spec = spec_of(bone);
    let mut out = None;

    ui.push_id(("joint-limits", index), |ui| {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Rotation").small())
                .on_hover_text("How this joint may turn");
            changed |= mode_buttons(ui, &mut spec);
        });
        if spec.0 == RotationMode::Limited {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Direction").small())
                    .on_hover_text("Which way this joint may turn from where it was drawn");
                changed |= direction_buttons(ui, &mut spec);
            });
            ui.horizontal(|ui| {
                changed |= range_values(ui, &mut spec);
            });
        }

        ui.label(
            RichText::new(format!(
                "now {:+.0}° of rest",
                wrap_pi(bone.angle - rest).to_degrees()
            ))
            .small()
            .weak(),
        )
        .on_hover_text("Where this joint is now, measured from the angle it was drawn at");

        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button("Apply to chain")
                .on_hover_text(
                    "Give this bone and every bone hanging off it the same rotation, direction \
                     and range, each measured from the angle that bone was drawn at",
                )
                .clicked()
            {
                out = Some(JointEdit::ApplyToChain {
                    index,
                    limits: limits_about(rest, spec),
                });
            }
            if ui
                .small_button("Apply to all bones")
                .on_hover_text(
                    "Give every bone of this rig the same rotation, direction and range, each \
                     measured from the angle that bone was drawn at",
                )
                .clicked()
            {
                out = Some(JointEdit::ApplyToAll {
                    limits: relative_limits(spec),
                });
            }
        });

        if changed && out.is_none() {
            out = Some(JointEdit::Set {
                index,
                limits: limits_about(rest, spec),
            });
        }
    });
    out
}
