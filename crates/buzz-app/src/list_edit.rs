//! Mutations that a panel's button and the AI agent's `edit_list` / dialog
//! commands both make, so there is one copy of each (docs/AI_AGENT_PLAN.md, D2).
//!
//! Each was inline in `App`'s panel code (Filters, Live Motion, Rigging, the
//! Trace Bitmap dialog) and is lifted here unchanged: same undo label, same
//! gesture handling, same status line. The panels call these; so does
//! `crate::agent`.

use buzz_scene::{Filter, LayerId, Modifier, ObjectId, ObjectKind};

use crate::editor::Editor;

impl Editor {
    // -- Filters panel ---------------------------------------------------------

    /// Apply `change` to the filter list of the selected object or the active
    /// layer, as one `doc.edit` carrying `label`. Both targets write the same
    /// `Vec`, so the whole panel is one code path with one place that decides
    /// where an edit lands.
    pub fn edit_filter_list(
        &mut self,
        target: buzz_ui::FilterTarget,
        object: Option<ObjectId>,
        layer: Option<LayerId>,
        label: &'static str,
        change: &dyn Fn(&mut Vec<Filter>),
    ) {
        match target {
            buzz_ui::FilterTarget::Object => {
                let Some(id) = object else { return };
                self.doc.edit(label, |scene| {
                    scene.update_object(id, |o| change(&mut o.filters));
                });
            }
            buzz_ui::FilterTarget::Layer => {
                let Some(id) = layer else { return };
                self.doc.edit(label, |scene| {
                    scene.update_layer(id, |l| change(&mut l.filters));
                });
            }
        }
    }

    // -- Live Motion (modifiers) -------------------------------------------------

    /// Attach a live modifier. It lives on every keyframe copy of the object
    /// (Edit-Multiple-Frames style) so it never flickers.
    pub fn add_modifier(&mut self, id: ObjectId, modifier: Modifier) {
        self.doc.edit("Add Modifier", |scene| {
            scene.update_object_across(0, u32::MAX, id, |o| o.modifiers.push(modifier));
        });
        self.doc.end_gesture();
        self.status = Some(format!("Added {}", modifier.label()));
    }

    pub fn remove_modifier(&mut self, id: ObjectId, index: usize) {
        self.doc.edit("Remove Modifier", |scene| {
            scene.update_object_across(0, u32::MAX, id, |o| {
                if index < o.modifiers.len() {
                    o.modifiers.remove(index);
                }
            });
        });
        self.doc.end_gesture();
    }

    /// Replace a modifier's parameters. No `end_gesture`: consecutive drags
    /// coalesce into one undo step; a caller that wants a step of its own ends
    /// the gesture itself.
    pub fn set_modifier(&mut self, id: ObjectId, index: usize, modifier: Modifier) {
        self.doc.edit("Edit Modifier", |scene| {
            scene.update_object_across(0, u32::MAX, id, |o| {
                if let Some(slot) = o.modifiers.get_mut(index) {
                    *slot = modifier;
                }
            });
        });
    }

    // -- Rigging panel -------------------------------------------------------------

    pub fn rig_reset_pose(&mut self, object: ObjectId) {
        self.doc.edit("Reset Pose", |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind {
                    let rest = rig.armature.at_rest().pose();
                    rig.armature.set_pose(&rest);
                }
            });
        });
    }

    /// Adopting a new rest pose changes what the weights were bound against, so
    /// the artwork has to be re-bound — otherwise it would keep bending about
    /// the old skeleton.
    pub fn rig_set_rest_pose(&mut self, object: ObjectId) {
        self.doc.edit("Set Rest Pose", |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind {
                    rig.armature.set_rest_here();
                    rig.rebind();
                }
            });
        });
    }

    pub fn rig_save_pose(&mut self, object: ObjectId, name: String) {
        self.doc.edit("Save Pose", |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind {
                    let angles = rig.armature.pose();
                    // Saving over a name replaces it, which is what somebody
                    // typing the same name again means. A second "reach" that
                    // does not replace the first is a list that fills with
                    // near-duplicates.
                    match rig.poses.iter_mut().find(|p| p.name == name) {
                        Some(existing) => existing.angles = angles,
                        None => rig.poses.push(buzz_scene::NamedPose { name, angles }),
                    }
                }
            });
        });
    }

    pub fn rig_mirror_pose(&mut self, object: ObjectId) {
        self.doc.edit("Mirror Pose", |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind {
                    let flipped = rig.armature.mirrored_pose();
                    rig.armature.set_pose(&flipped);
                }
            });
        });
    }

    /// Put a rig into one of its saved poses, optionally keying it first.
    pub fn apply_named_pose(&mut self, object: ObjectId, index: usize, key: bool) {
        if key {
            // A keyframe first, so the pose lands *on* one and the span before
            // it can tween into it. Without this, applying two poses on one
            // keyframe would simply overwrite the first.
            self.run(buzz_ui::Command::InsertKeyframe);
        }
        let label = if key { "Key Pose" } else { "Apply Pose" };
        self.doc.edit(label, |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind
                    && let Some(pose) = rig.poses.get(index)
                {
                    let angles = pose.angles.clone();
                    rig.armature.set_pose(&angles);
                }
            });
        });
    }

    pub fn rig_delete_pose(&mut self, object: ObjectId, index: usize) {
        self.doc.edit("Delete Pose", |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind
                    && index < rig.poses.len()
                {
                    rig.poses.remove(index);
                }
            });
        });
    }

    /// Weights were computed against a skeleton that no longer exists, so the
    /// artwork is re-bound — otherwise it would keep bending about a bone that
    /// has gone.
    pub fn rig_delete_bone(&mut self, object: ObjectId, bone: usize) {
        self.doc.edit("Delete Bone", |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind
                    && rig.armature.remove_bone(bone)
                {
                    rig.rebind();
                }
            });
        });
    }

    /// Point a bone at a different parent (`None` makes it a root). Returns
    /// `false` when it was refused — the only reason is a cycle — having said so
    /// on the status line.
    pub fn rig_reparent_bone(&mut self, object: ObjectId, bone: usize, parent: Option<usize>) -> bool {
        let mut refused = false;
        self.doc.edit("Reparent Bone", |scene| {
            scene.update_object(object, |target| {
                if let ObjectKind::Armature(rig) = &mut target.kind {
                    if rig.armature.reparent_bone(bone, parent) {
                        rig.rebind();
                    } else {
                        refused = true;
                    }
                }
            });
        });
        if refused {
            // Said rather than silently ignored: the only reason to refuse is a
            // cycle, and "nothing happened" is indistinguishable from a broken
            // control.
            self.status = Some("A bone cannot hang off itself or off one of its own children".into());
        }
        !refused
    }

    // -- Trace Bitmap dialog -------------------------------------------------------

    /// The dialog's Trace button: remember the options, then trace the selection.
    pub fn confirm_trace_options(&mut self, options: buzz_scene::TraceOptions) {
        self.trace_dialog = None;
        self.workspace.trace = options;
        self.workspace.save();
        self.trace_with(options);
    }
}
