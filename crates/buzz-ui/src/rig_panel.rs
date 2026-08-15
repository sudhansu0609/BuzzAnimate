//! The Armature panel: the bones of the selected rig, and what each may do.
//!
//! Animate keeps joint settings in the Properties panel when a bone is
//! selected. They live in a panel of their own here for one practical reason:
//! a rig is a *list*, and the thing an animator does with joint limits is
//! compare them across a chain — set a knee to bend one way, check the elbow
//! bends the other. One bone at a time makes that a memory exercise.
//!
//! Angles are shown in **degrees** and stored in radians. Nobody thinks about
//! an elbow in radians.

use buzz_rig::Armature;
use egui::{RichText, Ui};

/// What the user changed.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RigResponse {
    /// A bone's joint limits were changed, in radians. `None` clears them.
    pub set_limits: Option<(usize, Option<(f64, f64)>)>,
    /// A bone was pinned or unpinned.
    pub set_pinned: Option<(usize, bool)>,
    /// A bone was renamed.
    pub rename: Option<(usize, String)>,
    /// A bone was clicked, so the stage can highlight it.
    pub select_bone: Option<usize>,
    /// Put every bone back to the pose it was drawn in.
    pub reset_pose: bool,
    /// Adopt the current pose as the rest pose and re-bind the artwork.
    pub set_rest_pose: bool,

    // -- the pose library ------------------------------------------------
    /// Keep the pose on screen under this name.
    pub save_pose: Option<String>,
    /// Put the rig into a saved pose. The index is into the rig's own list.
    pub apply_pose: Option<usize>,
    /// Put the rig into a saved pose **and key it** at the playhead, so two
    /// applied poses tween into each other.
    pub key_pose: Option<usize>,
    /// Forget a saved pose.
    pub delete_pose: Option<usize>,
    /// Flip the pose on screen left-to-right.
    pub mirror_pose: bool,

    // -- editing the skeleton --------------------------------------------
    /// Remove a bone. Its children are adopted by its parent.
    pub delete_bone: Option<usize>,
    /// Point a bone at a different parent. `None` makes it a root.
    pub reparent: Option<(usize, Option<usize>)>,
}

/// State the pose list keeps between frames: what is being typed into the
/// name box. View state, so it lives here rather than in the document.
#[derive(Debug, Default, Clone)]
pub struct RigPanelState {
    pub new_pose_name: String,
}

/// Draw the panel for the selected armature.
///
/// `armature` is `None` when the selection is not a rig, which is most of the
/// time — the panel then says what to do rather than showing an empty list.
pub fn rig_panel(
    ui: &mut Ui,
    armature: Option<&Armature>,
    poses: &[buzz_scene::NamedPose],
    state: &mut RigPanelState,
) -> RigResponse {
    let mut response = RigResponse::default();

    ui.horizontal(|ui| {
        ui.heading("Armature");
        if let Some(armature) = armature {
            ui.label(
                RichText::new(format!("{} bones", armature.len()))
                    .small()
                    .weak(),
            );
        }
    });

    let Some(armature) = armature else {
        ui.label(
            RichText::new(
                "Select a rigged object to edit its bones.\n\nWith the Bone tool (M), drag \
                 across artwork to create an armature, then drag from a bone's tip to add \
                 the next one.",
            )
            .small()
            .weak(),
        );
        return response;
    };

    ui.horizontal(|ui| {
        if ui
            .small_button("Reset pose")
            .on_hover_text("Put every bone back where it was drawn")
            .clicked()
        {
            response.reset_pose = true;
        }
        if ui
            .small_button("Set rest pose")
            .on_hover_text("Treat the current pose as the one the artwork was drawn in")
            .clicked()
        {
            response.set_rest_pose = true;
        }
    });

    ui.separator();
    pose_library(ui, poses, state, &mut response);
    ui.separator();

    // The dock column already scrolls; see the note on `tool_bar`.

    let bones = armature.bones.len();
    for (index, bone) in armature.bones.iter().enumerate() {
        bone_row(ui, index, bone, bones, &mut response);
    }

    response
}

/// **Poses this character owns.**
///
/// A pose was a fact about one keyframe: to reuse it you posed the rig again
/// by hand, every time. Named and kept on the rig, it travels with the
/// character — through the clipboard, into the Assets library, into another
/// film — and *Key* turns the list from a posing aid into a way of animating:
/// pose A on one frame, pose B twelve frames later, and the tween is the
/// whole shot.
fn pose_library(
    ui: &mut Ui,
    poses: &[buzz_scene::NamedPose],
    state: &mut RigPanelState,
    out: &mut RigResponse,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Poses").strong());
        ui.label(RichText::new(format!("{}", poses.len())).small().weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Mirror")
                .on_hover_text(
                    "Flip the pose on screen left to right \u{2014} the same pose, other side",
                )
                .clicked()
            {
                out.mirror_pose = true;
            }
        });
    });

    // Save. The name box is sized to what is left on purpose: it sits in a
    // dock column, and the button beside it must stay reachable.
    ui.horizontal(|ui| {
        let room = (ui.available_width() - 46.0).max(40.0);
        ui.add_sized(
            egui::vec2(room, ui.spacing().interact_size.y),
            egui::TextEdit::singleline(&mut state.new_pose_name).hint_text("name this pose"),
        );
        let named = !state.new_pose_name.trim().is_empty();
        if ui
            .add_enabled(named, egui::Button::new("Save").small())
            .on_hover_text("Keep the pose on screen, under this name")
            .on_disabled_hover_text("Type a name first")
            .clicked()
        {
            out.save_pose = Some(state.new_pose_name.trim().to_string());
            state.new_pose_name.clear();
        }
    });

    if poses.is_empty() {
        ui.label(
            RichText::new(
                "No poses yet. Pose the rig, name it and press Save \u{2014} then one \
                 click puts the character back into it, in any scene.",
            )
            .small()
            .weak()
            .italics(),
        );
        return;
    }

    for (index, pose) in poses.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("\u{1F5D1}")
                    .on_hover_text("Forget this pose")
                    .clicked()
                {
                    out.delete_pose = Some(index);
                }
                if ui
                    .small_button("Key")
                    .on_hover_text(
                        "Apply it and key it at the playhead, so it tweens from the \
                         pose before it",
                    )
                    .clicked()
                {
                    out.key_pose = Some(index);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let room = ui.available_width().max(1.0);
                    if ui
                        .add_sized(
                            egui::vec2(room, ui.spacing().interact_size.y),
                            egui::Button::new(RichText::new(&pose.name).small()).truncate(),
                        )
                        .on_hover_text("Put the rig into this pose")
                        .clicked()
                    {
                        out.apply_pose = Some(index);
                    }
                });
            });
        });
    }
}

fn bone_row(
    ui: &mut Ui,
    index: usize,
    bone: &buzz_rig::Bone,
    bones: usize,
    response: &mut RigResponse,
) {
    ui.push_id(index, |ui| {
        ui.horizontal(|ui| {
            let mut name = bone.name.clone();
            if ui
                .add(egui::TextEdit::singleline(&mut name).desired_width(90.0))
                .changed()
            {
                response.rename = Some((index, name));
            }

            // **The parent, as a menu rather than a label.**
            //
            // Building a rig used to be additive only: one bone in the wrong
            // place meant starting the skeleton again, which is why nobody
            // rigged the second character. Choosing a parent here is how a
            // mistake is fixed rather than rebuilt. A bone cannot be offered
            // itself or anything hanging off it, because a cycle in a skeleton
            // is an infinite loop in the solver — the model refuses those too.
            let parent = match bone.parent {
                Some(p) => format!("< {p}"),
                None => "root".to_string(),
            };
            egui::ComboBox::from_id_salt(("bone-parent", index))
                .selected_text(RichText::new(parent).small())
                .width(64.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(bone.parent.is_none(), "root").clicked() {
                        response.reparent = Some((index, None));
                    }
                    for other in 0..bones {
                        if other == index {
                            continue;
                        }
                        if ui
                            .selectable_label(bone.parent == Some(other), format!("bone {other}"))
                            .clicked()
                        {
                            response.reparent = Some((index, Some(other)));
                        }
                    }
                })
                .response
                .on_hover_text("Which bone this one hangs off");

            if ui
                .small_button("\u{1F5D1}")
                .on_hover_text(
                    "Delete this bone. Anything hanging off it moves up to its \
                     parent rather than going with it.",
                )
                .clicked()
            {
                response.delete_bone = Some(index);
            }

            let mut pinned = bone.pinned;
            if ui
                .checkbox(&mut pinned, "Pin")
                .on_hover_text("A pinned joint stays put: inverse kinematics stops here")
                .changed()
            {
                response.set_pinned = Some((index, pinned));
            }
        });

        ui.horizontal(|ui| {
            let mut limited = bone.limits.is_some();
            if ui
                .checkbox(&mut limited, "Limit")
                .on_hover_text("Restrict how far this joint may turn")
                .changed()
            {
                response.set_limits = Some((
                    index,
                    // A joint that has just been limited starts at a quarter
                    // turn either way — wide enough to be usable immediately,
                    // narrow enough to show that the limit is doing something.
                    limited.then_some((-FRAC_QUARTER, FRAC_QUARTER)),
                ));
            }

            if let Some(limits) = bone.limits {
                let mut min = limits.min.to_degrees();
                let mut max = limits.max.to_degrees();
                let changed = ui
                    .add(
                        egui::DragValue::new(&mut min)
                            .speed(1.0)
                            .range(-180.0..=180.0)
                            .suffix("°"),
                    )
                    .changed()
                    | ui.add(
                        egui::DragValue::new(&mut max)
                            .speed(1.0)
                            .range(-180.0..=180.0)
                            .suffix("°"),
                    )
                    .changed();

                if changed {
                    response.set_limits = Some((index, Some((min.to_radians(), max.to_radians()))));
                }
            }

            ui.label(
                RichText::new(format!("{:.0}°", bone.angle.to_degrees()))
                    .small()
                    .weak(),
            )
            .on_hover_text("Where this joint is now, relative to its parent");
        });
        ui.separator();
    });
}

/// A quarter turn, in radians: the range a newly limited joint starts with.
const FRAC_QUARTER: f64 = std::f64::consts::FRAC_PI_2;

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_rig::Bone;

    #[test]
    fn a_response_starts_empty() {
        let response = RigResponse::default();
        assert!(response.set_limits.is_none());
        assert!(response.set_pinned.is_none());
        assert!(!response.reset_pose);
    }

    /// Degrees on screen, radians in the model. The conversion is the only
    /// arithmetic in the panel and it is exactly the kind that gets inverted
    /// by accident.
    #[test]
    fn limits_convert_between_degrees_and_radians() {
        let bone = Bone::new("elbow", None, 10.0, 0.0).with_limits(-FRAC_QUARTER, FRAC_QUARTER);
        let limits = bone.limits.expect("limits");

        assert!((limits.min.to_degrees() - -90.0).abs() < 1e-9);
        assert!((limits.max.to_degrees() - 90.0).abs() < 1e-9);
        assert!(((-90.0f64).to_radians() - limits.min).abs() < 1e-9);
    }

    #[test]
    fn the_panel_draws_with_and_without_a_rig() {
        let ctx = egui::Context::default();
        let mut armature = Armature::new(buzz_geom::Point::ZERO);
        armature.push(Bone::new("upper", None, 50.0, 0.0));
        armature.push(
            Bone::new("fore", Some(0), 40.0, 0.3)
                .with_limits(-1.0, 1.0)
                .pinned(),
        );

        // egui 0.35 roots the UI in a `Ui` rather than a `Context`.
        let _ = ctx.run_ui(Default::default(), |ui| {
            let mut state = RigPanelState::default();
            let empty = rig_panel(ui, None, &[], &mut state);
            assert_eq!(empty, RigResponse::default());
            let _ = rig_panel(ui, Some(&armature), &[], &mut state);
        });
    }
}
