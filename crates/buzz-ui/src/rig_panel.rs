//! The Rigging panel: sorting drawings into a skeleton, and editing the bones.
//!
//! Two jobs, one panel, in the order they happen.
//!
//! # Assembling
//!
//! A character arrives as a stack of drawings — an arm, a forearm, a head —
//! and rigging it by hand is a bone dragged along each one, twenty times,
//! before the first frame of animation exists. So the top of this panel is the
//! list of *slots* a [`buzz_rig::RigPattern`] declares, and filling them is the
//! whole of rigging: a drawing in "Elbow L" becomes a bone along that drawing,
//! parented to "Shoulder L", named for the slot it went in.
//!
//! There are three ways to fill a slot and they are three different moods:
//!
//! * **Auto-assign** reads the layer names. On artwork exported tidily from
//!   Photoshop or Animate this fills the whole character at once, which is the
//!   point of it — the names are already there and nobody should have to
//!   repeat them by hand.
//! * **Click a slot, then click the drawing on the stage.** Overlapping parts
//!   are the normal case on a character standing in a rest pose, and a drag
//!   cannot pick between two things under the same pixel while a click on an
//!   armed slot can.
//! * **Drag a drawing into a slot**, from the tray of loose parts at the
//!   bottom of this panel or from a layer in the Layers panel.
//!
//! Deliberately *not* a fourth way: dragging off the stage. On the stage a
//! drag already means "move this object", and overloading it would make
//! rigging and posing the same gesture.
//!
//! # Editing
//!
//! Below that is what was here before: the bones of the selected rig, their
//! joint limits and pins, and the character's pose library. Animate keeps
//! those in the Properties panel one bone at a time; they are a list here
//! because the thing an animator does with joint limits is *compare* them
//! along a chain, and one bone at a time makes that a memory exercise.
//!
//! Angles are shown in **degrees** and stored in radians. Nobody thinks about
//! an elbow in radians.

use buzz_rig::{Armature, Direction, JointLimitDefaults, RigKind, RigPattern, RotationMode};
use buzz_scene::{LayerId, ObjectId};
use egui::{RichText, Ui};

use crate::Palette;

/// A drawing on the stage that has not been rigged yet.
///
/// The `layer` is carried so that a drag from the Layers panel can be resolved
/// here rather than sent back to the document and returned: what a layer means
/// in this panel is "the drawing on it".
#[derive(Debug, Clone, PartialEq)]
pub struct LoosePart {
    pub object: ObjectId,
    pub layer: LayerId,
    /// What to show it as: the object's name, its layer's, or a last resort.
    pub name: String,
}

/// Something being dragged towards a slot.
///
/// Two sources, because the two panels have two different things to give: the
/// tray below knows the drawing, and a layer row knows only its layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraggedPart {
    Object(ObjectId),
    Layer(LayerId),
}

/// What the user changed.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RigResponse {
    /// A bone's joint limits were changed, in radians. `None` clears them.
    pub set_limits: Option<(usize, Option<(f64, f64)>)>,
    /// A bone's joint widget asked for its limits on more than one bone:
    /// [`JointEdit::ApplyToChain`](crate::joint_widget::JointEdit::ApplyToChain)
    /// or [`JointEdit::ApplyToAll`](crate::joint_widget::JointEdit::ApplyToAll).
    pub apply_limits: Option<crate::joint_widget::JointEdit>,
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

    // -- assembling a rig from loose drawings ----------------------------
    /// Build a rig: the pattern to build it from, and what went in each slot.
    ///
    /// The whole assignment travels in one go because building it is one undo
    /// step: rigging a character is a single decision, and an animator who
    /// regrets it should press Ctrl+Z once, not eleven times.
    pub build_rig: Option<(String, Vec<Option<ObjectId>>)>,
    /// Put a different drawing into one slot of the rig already selected —
    /// a redrawn arm, dropped back where the old one was.
    pub replace_part: Option<(usize, ObjectId)>,
    /// Select a drawing on the stage, so the animator can see which one it is.
    pub select_part: Option<ObjectId>,

    // -- default joint limits ----------------------------------------------
    /// **Save as default**: write this table as the user's `rig_limits.json`.
    pub save_default_limits: Option<JointLimitDefaults>,
    /// **Apply to this rig**: give the selected rig's bones these limits,
    /// each about its own rest angle.
    pub apply_default_limits: Option<JointLimitDefaults>,

    // -- converting between bone and layer rigs -------------------------------
    /// **Convert to layer rig**: take the selected bone rig apart into layers.
    pub convert_to_layers: bool,
    /// **Convert to bone rig**: rebuild the selected layer rig as an armature.
    pub convert_to_bones: bool,

    // -- the head ------------------------------------------------------------
    /// **Name Layers**: give these layers of this head their picked names.
    pub name_head_layers: Option<(buzz_scene::SymbolId, Vec<(buzz_scene::LayerId, String)>)>,
    /// **Rig Head** on this head.
    pub rig_head: Option<buzz_scene::SymbolId>,
}

/// State the panel keeps between frames. All of it is about what is on screen
/// rather than what is in the document, which is why it lives here.
#[derive(Debug, Clone)]
pub struct RigPanelState {
    pub new_pose_name: String,

    /// Which built-in pattern the assembly section is working with.
    pub pattern: usize,
    /// What has been put in each slot of it, one entry per slot.
    pub slots: Vec<Option<ObjectId>>,
    /// A slot waiting for a drawing to be clicked. Read by the stage, which is
    /// where the click lands.
    pub armed: Option<usize>,
    /// What the last gesture did, said under the list.
    pub note: Option<String>,
    /// The **Default limits** table being edited — the user's saved
    /// `rig_limits.json`, filled in by the app; the built-in anatomical table
    /// until it is.
    pub default_limits: Option<JointLimitDefaults>,
    /// What **Rig Character** builds: an armature, or layer parenting.
    pub kind: RigKind,
    /// Whether the selection is part of a layer rig, so **Convert to bone
    /// rig** is offered. Set by the app before the panel is drawn.
    pub layer_rig: bool,
    /// The user's **naming profile** (the names their Ctrl+R / Shift+R
    /// commands give: `leftArm`, `leftWrist`, `leftPalm`, `body`…): what each
    /// slot is called in the list, and the names a layer is matched by first.
    /// Set by the app; `None` shows the slots' own names.
    pub profile: Option<buzz_rig::pattern::NamingProfile>,
    /// The head the **Rig Head** section works on (the selected instance's,
    /// or the open symbol's), its layers and the name each would get. Set by
    /// the app; `None` when there is no head here.
    pub head: Option<HeadSection>,
    /// The name picked for each of the head's layers, by layer: what **Name
    /// Layers** gives them. Filled from the suggestions when the head changes.
    pub head_tags: Vec<(buzz_scene::LayerId, String)>,
    /// The head `head_tags` were filled for.
    pub head_tags_for: Option<buzz_scene::SymbolId>,
    /// The names picked for heads worked on before, so going back to one
    /// finds them as they were left.
    pub head_tags_kept: Vec<(buzz_scene::SymbolId, Vec<(buzz_scene::LayerId, String)>)>,
    /// Slot drawings that still exist on this timeline though not on the
    /// frame shown — kept rather than forgotten. Set by the app.
    pub alive: Vec<ObjectId>,
    /// The names of slot drawings that are not loose on this timeline — on
    /// another keyframe, or on the body while the head inside it is open —
    /// so a kept slot still says what is in it. Set by the app.
    pub kept_names: Vec<(ObjectId, String)>,
    /// **The user's own parts**, added to a built-in skeleton: a tail, a
    /// dupatta, a bag — each connected to a part already in the list, so
    /// Rig Character parents it there. Kept per pattern.
    pub extra_parts: Vec<ExtraPart>,
    /// The **Add part** row being filled in: its name and what it hangs off.
    pub new_part_name: String,
    pub new_part_parent: usize,
}

/// One part the user added to a pattern (see [`RigPanelState::extra_parts`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraPart {
    /// Which built-in pattern it was added to.
    pub pattern: usize,
    pub name: String,
    /// The slot it is connected to, in the pattern's own list (always earlier).
    pub parent: usize,
}

/// A head symbol, as the Rig Head section shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadSection {
    pub symbol: buzz_scene::SymbolId,
    pub name: String,
    /// Its layers, top first: the layer, its name now, and the name the
    /// user's Shift+R command (else Rig Head's reading of it) would give it.
    pub layers: Vec<(buzz_scene::LayerId, String, Option<String>)>,
}

/// The names a head layer can be given: the user's Shift+R command's
/// (`renameHeadLayers.jsfl`), then Rig Head's other parts.
pub const HEAD_NAMES: &[&str] = &[
    "eyeWhite", "leftEyeWhite", "rightEyeWhite", "eyeBall", "eyebrow", "leftEyebrow", "rightEyebrow",
    "eyeLid", "mouth", "nose", "hair", "base", "moustacheFront",
];

impl Default for RigPanelState {
    /// **Rig Character** builds a layer rig unless the user picks Bones
    /// (LR2): layer parenting is what new characters get.
    fn default() -> Self {
        Self {
            new_pose_name: String::new(),
            pattern: 0,
            slots: Vec::new(),
            armed: None,
            note: None,
            default_limits: None,
            kind: RigKind::Layers,
            layer_rig: false,
            profile: None,
            head: None,
            head_tags: Vec::new(),
            head_tags_for: None,
            head_tags_kept: Vec::new(),
            alive: Vec::new(),
            kept_names: Vec::new(),
            extra_parts: Vec::new(),
            new_part_name: String::new(),
            new_part_parent: 0,
        }
    }
}

impl RigPanelState {
    /// The pattern the assembly section is showing.
    pub fn pattern(&self) -> RigPattern {
        let patterns = RigPattern::builtin();
        let index = self.pattern.min(patterns.len().saturating_sub(1));
        self.with_extras(patterns.into_iter().nth(index).unwrap_or_else(RigPattern::biped))
    }

    /// `pattern` (the one on show) with the user's own parts added after its
    /// slots, each a free joint hanging off the part it was connected to.
    pub fn with_extras(&self, mut pattern: RigPattern) -> RigPattern {
        for extra in self.extra_parts.iter().filter(|e| e.pattern == self.pattern) {
            let parent = extra.parent.min(pattern.slots.len().saturating_sub(1));
            let lower = extra.name.to_lowercase();
            let side = if lower.contains("left") {
                buzz_rig::pattern::Side::Left
            } else if lower.contains("right") {
                buzz_rig::pattern::Side::Right
            } else {
                buzz_rig::pattern::Side::Either
            };
            pattern.slots.push(buzz_rig::pattern::Slot {
                name: extra.name.clone(),
                parent: (!pattern.slots.is_empty()).then_some(parent),
                rest_angle: 0.0,
                rest_len: 0.08,
                limits: None,
                side,
                required: false,
                aliases: vec![extra.name.to_lowercase()],
            });
        }
        pattern
    }

    /// Add a part called `name`, connected to slot `parent` of the pattern
    /// on show. Its slot starts empty.
    pub fn add_part(&mut self, name: &str, parent: usize) -> bool {
        let name = name.trim();
        let pattern = self.pattern();
        if name.is_empty() || parent >= pattern.slots.len() || pattern.slots.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
            return false;
        }
        if self.slots.len() != pattern.slots.len() {
            self.slots = vec![None; pattern.slots.len()];
        }
        self.extra_parts.push(ExtraPart { pattern: self.pattern, name: name.to_string(), parent });
        self.slots.push(None);
        true
    }

    /// Take out the user's part in slot `slot`, and every part of theirs
    /// connected to it.
    pub fn remove_part(&mut self, slot: usize) {
        let base = RigPattern::builtin()
            .into_iter()
            .nth(self.pattern)
            .map(|p| p.slots.len())
            .unwrap_or(0);
        let mut doomed = vec![slot];
        let mut index = base;
        let mut kept = Vec::new();
        let mut remap: Vec<Option<usize>> = (0..base).map(Some).collect();
        for extra in self.extra_parts.iter() {
            if extra.pattern != self.pattern {
                kept.push(extra.clone());
                continue;
            }
            if doomed.contains(&index) || doomed.contains(&extra.parent) {
                doomed.push(index);
                remap.push(None);
            } else {
                let mut e = extra.clone();
                e.parent = remap.get(extra.parent).copied().flatten().unwrap_or(0);
                remap.push(Some(remap.iter().flatten().count()));
                kept.push(e);
            }
            index += 1;
        }
        self.extra_parts = kept;
        let old = std::mem::take(&mut self.slots);
        self.slots = old
            .into_iter()
            .enumerate()
            .filter(|(i, _)| remap.get(*i).is_none_or(|r| r.is_some()))
            .map(|(_, held)| held)
            .collect();
        self.armed = None;
    }

    /// Put a drawing in a slot, taking it out of whatever slot it was in.
    ///
    /// One drawing cannot be two limbs, and silently leaving it in both would
    /// bind the same artwork to two bones — which draws it twice and moves it
    /// in two directions at once.
    pub fn assign(&mut self, slot: usize, object: ObjectId) {
        for held in self.slots.iter_mut() {
            if *held == Some(object) {
                *held = None;
            }
        }
        if let Some(place) = self.slots.get_mut(slot) {
            *place = Some(object);
        }
        if self.armed == Some(slot) {
            self.armed = None;
        }
    }

    /// Fit the slot list to `pattern`, forgetting drawings that are no longer
    /// on offer — one that has been deleted, or that is already inside a rig.
    fn reconcile(&mut self, pattern: &RigPattern, parts: &[LoosePart]) {
        if self.slots.len() != pattern.slots.len() {
            self.slots = vec![None; pattern.slots.len()];
            self.armed = None;
        }
        let alive = std::mem::take(&mut self.alive);
        for held in self.slots.iter_mut() {
            if held.is_some_and(|id| !parts.iter().any(|p| p.object == id) && !alive.contains(&id)) {
                *held = None;
            }
        }
        if self.armed.is_some_and(|slot| slot >= self.slots.len()) {
            self.armed = None;
        }
    }
}

/// Draw the panel.
///
/// `parts` is every drawing on the stage that is not already rigged; `bound`
/// says, for a rig that was assembled from a pattern, which drawing is in
/// which slot. Both come from the document and neither is stored here.
pub fn rig_panel(
    ui: &mut Ui,
    armature: Option<&Armature>,
    poses: &[buzz_scene::NamedPose],
    parts: &[LoosePart],
    bound: Option<(&str, &[(usize, String)])>,
    state: &mut RigPanelState,
    rig: &mut crate::workspace::RigOptions,
) -> RigResponse {
    let mut response = RigResponse::default();

    // **How the rig behaves, at the top of the rig's own panel.**
    //
    // These live in the Bone tool's options too, which is where somebody
    // holding that tool looks. It is the wrong place for the one person who
    // most wants them: an animator drawing over a character is holding a
    // *drawing* tool, and the skeleton is in the way of both the pen and the
    // pointer.
    ui.horizontal_wrapped(|ui| {
        use crate::workspace::BoneMode;
        ui.checkbox(&mut rig.show_bones, "Show bones while the Bone tool is active")
            .on_hover_text(
                "Hidden, the bones are out of the pointer's way as well as out \
                 of sight, so the artwork under them can be worked on. With any \
                 other tool they are never drawn",
            );
        ui.separator();
        if ui
            .selectable_label(rig.mode == BoneMode::Create, BoneMode::Create.label())
            .on_hover_text("The Bone tool builds: start, extend or branch a chain (Shift+M toggles)")
            .clicked()
        {
            rig.mode = BoneMode::Create;
        }
        if ui
            .selectable_label(rig.mode == BoneMode::Move, BoneMode::Move.label())
            .on_hover_text("The Bone tool poses: drag any part of a bone to move the limb (Shift+M toggles)")
            .clicked()
        {
            rig.mode = BoneMode::Move;
        }
    });
    ui.separator();

    match bound {
        // A rig that was assembled from a pattern: show what is in it, so a
        // redrawn part can go back where the old one was.
        Some((name, filled)) => rigged_slots(ui, name, filled, parts, &mut response),
        None => assembly(ui, parts, state, &mut response),
    }

    // Converting a rig that already exists, either way, one undo step each.
    if armature.is_some() || state.layer_rig {
        ui.horizontal_wrapped(|ui| {
            if armature.is_some()
                && ui
                    .small_button("Convert to layer rig")
                    .on_hover_text(
                        "One layer per part, each turning about its joint and following the \
                         part it hangs off. One undo step",
                    )
                    .clicked()
            {
                response.convert_to_layers = true;
            }
            if state.layer_rig
                && ui
                    .small_button("Convert to bone rig")
                    .on_hover_text(
                        "Build a skeleton from these layers, read by their slot names. One undo step",
                    )
                    .clicked()
            {
                response.convert_to_bones = true;
            }
        });
    }

    head_section(ui, state, &mut response);

    // The table is for the pattern on screen: the selected rig's own, or the
    // one being assembled.
    let pattern = bound
        .and_then(|(name, _)| RigPattern::named(name))
        .unwrap_or_else(|| state.pattern());
    default_limits(ui, &pattern, armature.is_some(), state, &mut response);

    ui.separator();
    armature_section(ui, armature, poses, state, &mut response);
    response
}

/// **Rig Head**, beside Rig Character: the head's layers, each with the name
/// it is tagged as (the user's Shift+R names), **Name Layers** to give them
/// those names, and **Rig Head** to build the head's structure from them.
fn head_section(ui: &mut Ui, state: &mut RigPanelState, out: &mut RigResponse) {
    ui.separator();
    ui.heading("Rig Head");
    let Some(head) = state.head.clone() else {
        ui.label(
            RichText::new("Select a head instance on the stage, or open the head symbol.")
                .small()
                .weak()
                .italics(),
        );
        return;
    };
    // A new head: start from the names its layers would get.
    if state.head_tags_for != Some(head.symbol) {
        // The names picked for the head being left are kept for coming back.
        if let Some(old) = state.head_tags_for {
            let tags = std::mem::take(&mut state.head_tags);
            state.head_tags_kept.retain(|(s, _)| *s != old);
            state.head_tags_kept.push((old, tags));
        }
        let kept = state.head_tags_kept.iter().find(|(s, _)| *s == head.symbol).map(|(_, t)| t.clone());
        state.head_tags = head
            .layers
            .iter()
            .map(|(id, now, suggested)| {
                let picked = kept.as_ref().and_then(|k| k.iter().find(|(l, _)| l == id).map(|(_, t)| t.clone()));
                (*id, picked.unwrap_or_else(|| suggested.clone().unwrap_or_else(|| now.clone())))
            })
            .collect();
        state.head_tags_for = Some(head.symbol);
    }
    ui.label(RichText::new(format!("Head: {}", head.name)).small());
    egui::Grid::new("rig-head-layers").num_columns(3).spacing([6.0, 2.0]).show(ui, |ui| {
        for (id, now, _) in &head.layers {
            let Some(tag) = state.head_tags.iter_mut().find(|(l, _)| l == id).map(|(_, t)| t) else {
                continue;
            };
            ui.label(RichText::new(now).small());
            ui.label(RichText::new("\u{2192}").small().weak());
            egui::ComboBox::from_id_salt(("rig-head-tag", id.0))
                .selected_text(RichText::new(tag.as_str()).small())
                .width(130.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(tag, now.clone(), format!("{now} (keep)"));
                    for name in HEAD_NAMES {
                        ui.selectable_value(tag, (*name).to_string(), *name);
                    }
                });
            ui.end_row();
        }
    });
    ui.horizontal_wrapped(|ui| {
        let renames: Vec<(buzz_scene::LayerId, String)> = state
            .head_tags
            .iter()
            .filter(|(id, tag)| head.layers.iter().any(|(l, now, _)| l == id && now != tag))
            .cloned()
            .collect();
        if ui
            .add_enabled(!renames.is_empty(), egui::Button::new("Name Layers"))
            .on_hover_text("Give the head's layers these names (your Shift+R names). One undo step")
            .on_disabled_hover_text("Every layer already has the name picked for it")
            .clicked()
        {
            out.name_head_layers = Some((head.symbol, renames));
        }
        if ui
            .button(RichText::new("Rig Head").strong())
            .on_hover_text(
                "Build the head's structure from its layers: eye lid (blink), eye mask, eye ball, eye white, \
                 brows, mouth symbol, nose, hair, base. One undo step",
            )
            .clicked()
        {
            out.rig_head = Some(head.symbol);
        }
    });
}

// ---------------------------------------------------------------------------
// The default joint limits, per slot.
// ---------------------------------------------------------------------------

/// One row per slot of `pattern`, in slot order: the slot's name and its
/// limits in `table` — a slot the table does not name turns freely.
pub fn default_limit_rows(
    pattern: &RigPattern,
    table: &JointLimitDefaults,
) -> Vec<(String, crate::joint_widget::LimitSpec)> {
    pattern
        .slots
        .iter()
        .map(|slot| {
            let spec = table
                .rows
                .get(&slot.name)
                .copied()
                .unwrap_or((RotationMode::Free, Direction::Both, 0.0, 0.0));
            (slot.name.clone(), spec)
        })
        .collect()
}

/// **Default limits**: how far each joint of a newly built rig may turn,
/// slot by slot, with the joint widget's own row pieces. Closed until asked
/// for — it is a setting, not something done to the rig on screen.
fn default_limits(
    ui: &mut Ui,
    pattern: &RigPattern,
    has_rig: bool,
    state: &mut RigPanelState,
    out: &mut RigResponse,
) {
    use crate::joint_widget::{direction_buttons, mode_buttons, range_values};

    egui::CollapsingHeader::new(RichText::new("Default limits").strong())
        .id_salt("rig-default-limits")
        .default_open(false)
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "How far each joint of a {} may turn, from the angle it is drawn at. \
                     Every rig built after Save as default uses this table.",
                    pattern.name
                ))
                .small()
                .weak()
                .italics(),
            );
            let table = state
                .default_limits
                .get_or_insert_with(JointLimitDefaults::anatomical);

            for (name, mut spec) in default_limit_rows(pattern, table) {
                ui.push_id(("default-limit", name.as_str()), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_sized(
                            egui::vec2(64.0, ui.spacing().interact_size.y),
                            egui::Label::new(RichText::new(&name).small()).truncate(),
                        );
                        let mut changed = mode_buttons(ui, &mut spec);
                        if spec.0 == RotationMode::Limited {
                            changed |= direction_buttons(ui, &mut spec);
                            changed |= range_values(ui, &mut spec);
                        }
                        if changed {
                            table.rows.insert(name.clone(), spec);
                        }
                    });
                });
            }

            ui.horizontal_wrapped(|ui| {
                if ui
                    .small_button("Save as default")
                    .on_hover_text(
                        "Keep this table: every rig built from now on \u{2014} Rig Character, \
                         Character from Image, the series tools \u{2014} uses it",
                    )
                    .clicked()
                {
                    out.save_default_limits = Some(table.clone());
                }
                if ui
                    .small_button("Reset to anatomical")
                    .on_hover_text(
                        "Put back the built-in table. Nothing is kept until Save as default",
                    )
                    .clicked()
                {
                    *table = JointLimitDefaults::anatomical();
                }
                if ui
                    .add_enabled(has_rig, egui::Button::new("Apply to this rig").small())
                    .on_hover_text(
                        "Give the selected rig's bones these limits, each measured from the \
                         angle that bone was drawn at. Bones the table does not name are \
                         left alone",
                    )
                    .on_disabled_hover_text("Select a rig first")
                    .clicked()
                {
                    out.apply_default_limits = Some(table.clone());
                }
            });
        });
}

// ---------------------------------------------------------------------------
// Assembling.
// ---------------------------------------------------------------------------

fn assembly(ui: &mut Ui, parts: &[LoosePart], state: &mut RigPanelState, out: &mut RigResponse) {
    let pattern = state.pattern();
    state.reconcile(&pattern, parts);

    ui.horizontal(|ui| {
        ui.heading("Rig");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let filled = state.slots.iter().filter(|s| s.is_some()).count();
            ui.label(
                RichText::new(format!("{filled}/{}", state.slots.len()))
                    .small()
                    .weak(),
            )
            .on_hover_text("Slots filled");
        });
    });

    // **The pattern, first.** Everything below it is a different list
    // depending on the answer, so asking anywhere else would be asking after
    // the fact.
    ui.horizontal(|ui| {
        let names: Vec<String> = RigPattern::builtin().into_iter().map(|p| p.name).collect();
        egui::ComboBox::from_id_salt("rig-pattern")
            .selected_text(RichText::new(&pattern.name).small())
            .width(96.0)
            .show_ui(ui, |ui| {
                for (index, name) in names.iter().enumerate() {
                    if ui.selectable_label(index == state.pattern, name).clicked() {
                        state.pattern = index;
                        // A different skeleton has different slots, so nothing
                        // sorted into the old one survives.
                        state.slots.clear();
                        state.armed = None;
                        state.note = None;
                    }
                }
            })
            .response
            .on_hover_text("Which skeleton these drawings are being sorted into");

        if ui
            .small_button("Auto")
            .on_hover_text(
                "Fill the slots from the drawings' own names \u{2014} leftArm, L_arm, \
                 arm_left and arm.L all mean the same thing",
            )
            .clicked()
        {
            auto_assign(&pattern, parts, state);
        }
        if ui
            .small_button("Clear")
            .on_hover_text("Empty every slot. Nothing in the document changes")
            .clicked()
        {
            state.slots = vec![None; pattern.slots.len()];
            state.armed = None;
            state.note = None;
        }
    });

    // Only when nothing is in the list either: with the head open inside the
    // body there are no loose drawings here, but the body's parts are still
    // in their slots and the list is what says so.
    if parts.is_empty() && state.slots.iter().all(Option::is_none) {
        ui.label(
            RichText::new(
                "Nothing on the stage to rig yet. Draw the character's parts, or import \
                 them \u{2014} one drawing per limb.",
            )
            .small()
            .weak()
            .italics(),
        );
        return;
    }

    ui.add_space(2.0);
    let builtin_len = RigPattern::builtin().into_iter().nth(state.pattern).map(|p| p.slots.len()).unwrap_or(0);
    let mut remove = None;
    for index in 0..pattern.slots.len() {
        if index >= builtin_len {
            ui.horizontal(|ui| {
                let parent = pattern.slots[index].parent.and_then(|p| pattern.slots.get(p)).map(|p| slot_label(state, &p.name)).unwrap_or_default();
                ui.label(RichText::new(format!("your part, on {parent}")).small().weak());
                if ui.small_button("\u{2715}").on_hover_text("Take this part out of the list").clicked() {
                    remove = Some(index);
                }
            });
        }
        slot_row(ui, &pattern, index, parts, state, out);
    }
    if let Some(index) = remove {
        state.remove_part(index);
    }
    // **Add a part of your own**, connected to one already in the list.
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.new_part_name).hint_text("New part (tail, bag\u{2026})").desired_width(110.0));
        let names: Vec<String> = pattern.slots.iter().map(|s| slot_label(state, &s.name)).collect();
        let at = state.new_part_parent.min(names.len().saturating_sub(1));
        egui::ComboBox::from_id_salt("rig-new-part-parent")
            .selected_text(RichText::new(format!("on {}", names.get(at).cloned().unwrap_or_default())).small())
            .width(90.0)
            .show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    ui.selectable_value(&mut state.new_part_parent, i, name);
                }
            })
            .response
            .on_hover_text("The part it is connected to: it moves with that part");
        if ui.small_button("Add part").clicked() {
            let name = state.new_part_name.clone();
            if state.add_part(&name, at) {
                state.new_part_name.clear();
            } else {
                state.note = Some("Give the part a name no other part has".into());
            }
        }
    });

    // -- what is still missing, and the button ------------------------------

    let full: Vec<bool> = state.slots.iter().map(Option::is_some).collect();
    let missing = pattern.missing_required(&full);
    let any = full.iter().any(|f| *f);

    if !missing.is_empty() {
        ui.label(
            RichText::new(format!("Still empty: {}", missing.join(", ")))
                .small()
                .weak(),
        )
        .on_hover_text(
            "A rig can be built without these. Every slot gets a bone either way, so a \
             walk cycle still runs \u{2014} it just moves nothing where a drawing is missing.",
        );
    }
    if let Some(note) = &state.note {
        ui.label(RichText::new(note).small().weak().italics());
    }

    rig_kind_row(ui, &mut state.kind);

    ui.horizontal(|ui| {
        let room = ui.available_width().max(1.0);
        if ui
            .add_enabled(
                any,
                egui::Button::new(RichText::new("Rig Character").strong())
                    .min_size(egui::vec2(room, 0.0)),
            )
            .on_hover_text("Build the skeleton and move these drawings into it. One undo step")
            .on_disabled_hover_text("Put a drawing in at least one slot first")
            .clicked()
        {
            out.build_rig = Some((pattern.name.clone(), state.slots.clone()));
        }
    });

    tray(ui, parts, state);
}

/// `Rig kind: ◉ Bones ○ Layers`, and the one line about what a layer rig
/// cannot do. Shared by the Rig panel and the AI character dialog.
pub fn rig_kind_row(ui: &mut Ui, kind: &mut RigKind) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Rig kind:").small());
        ui.radio_value(kind, RigKind::Bones, "Bones")
            .on_hover_text("An armature: IK, the director's verbs and lip-sync");
        ui.radio_value(kind, RigKind::Layers, "Layers")
            .on_hover_text("Layer parenting: one layer per part, posed with Free Transform");
    });
    if *kind == RigKind::Layers {
        ui.label(RichText::new(RigKind::LAYERS_HINT).small().weak().italics());
    }
}

/// One slot: its name, what is in it, and every way of putting something there.
fn slot_row(
    ui: &mut Ui,
    pattern: &RigPattern,
    index: usize,
    parts: &[LoosePart],
    state: &mut RigPanelState,
    out: &mut RigResponse,
) {
    let slot = &pattern.slots[index];
    let held = state.slots.get(index).copied().flatten();
    let armed = state.armed == Some(index);

    let mut clicked_slot = false;
    let mut cleared = false;

    ui.push_id(("rig-slot", index), |ui| {
        let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(1, 0));
        let (_, dropped) = ui.dnd_drop_zone::<DraggedPart, _>(frame, |ui| {
            ui.horizontal(|ui| {
                // Filled, wanted, or optional — in that order of weight, so the
                // shape of the character reads down the left edge of the list.
                let (mark, colour) = match (held.is_some(), slot.required) {
                    (true, _) => ("\u{25CF}", Palette::active()),
                    (false, true) => ("\u{25CB}", Palette::text_dim()),
                    (false, false) => ("\u{00B7}", Palette::text_dim()),
                };
                ui.label(RichText::new(mark).small().color(colour));

                // The user's name for the part (`leftArm`), the slot's own
                // ("Shoulder L") in the tooltip.
                let label = profile_name(state.profile.as_ref(), &slot.name).unwrap_or_else(|| slot.name.clone());
                ui.add_sized(
                    egui::vec2(64.0, ui.spacing().interact_size.y),
                    egui::Label::new(RichText::new(&label).small()).truncate(),
                )
                .on_hover_text(format!("{label} \u{2014} the {} slot", slot.name));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if held.is_some()
                        && ui
                            .small_button("\u{2715}")
                            .on_hover_text("Take this drawing back out of the slot")
                            .clicked()
                    {
                        cleared = true;
                    }

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let room = ui.available_width().max(24.0);
                        let label = match held {
                            Some(id) => parts
                                .iter()
                                .find(|p| p.object == id)
                                .map(|p| p.name.clone())
                                .or_else(|| state.kept_names.iter().find(|(k, _)| *k == id).map(|(_, n)| n.clone()))
                                .unwrap_or_else(|| "\u{2014}".to_string()),
                            None if armed => "click it on the stage".to_string(),
                            None => "empty".to_string(),
                        };
                        let text = if held.is_some() {
                            RichText::new(label).small()
                        } else {
                            RichText::new(label).small().weak().italics()
                        };
                        let button = egui::Button::selectable(armed, text).truncate();
                        let response =
                            ui.add_sized(egui::vec2(room, ui.spacing().interact_size.y), button);
                        if response.clicked() {
                            clicked_slot = true;
                        }
                        response.on_hover_text(match held {
                            Some(_) => "Click to find this drawing on the stage",
                            None => "Click, then click the drawing on the stage. Or drag one here",
                        });
                    });
                });
            });
        });

        if let Some(payload) = dropped {
            match resolve(*payload, parts) {
                Some(object) => {
                    state.assign(index, object);
                    state.note = None;
                }
                // A layer with nothing on it, or with several drawings on it:
                // there is no single answer, and picking one at random would
                // be worse than saying so.
                None => {
                    state.note = Some(
                        "That layer does not hold exactly one drawing \u{2014} group its \
                         parts first, or drag from the tray below."
                            .into(),
                    );
                }
            }
        }
    });

    if cleared && let Some(place) = state.slots.get_mut(index) {
        *place = None;
    }
    if clicked_slot {
        match held {
            // Filled: show the animator which drawing this is, rather than
            // making them guess from a name.
            Some(id) => out.select_part = Some(id),
            // Empty: arm it, and let the next click on the stage fill it.
            // Clicking an armed slot again disarms it, so the gesture is
            // escapable without a second control.
            None => state.armed = if armed { None } else { Some(index) },
        }
    }
}

/// The drawings nobody has sorted yet, as things to drag.
fn tray(ui: &mut Ui, parts: &[LoosePart], state: &mut RigPanelState) {
    let loose: Vec<&LoosePart> = parts
        .iter()
        .filter(|part| !state.slots.contains(&Some(part.object)))
        .collect();

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Loose parts").strong().small());
        ui.label(RichText::new(format!("{}", loose.len())).small().weak());
    });

    if loose.is_empty() {
        ui.label(
            RichText::new("Every drawing on the stage is in a slot.")
                .small()
                .weak()
                .italics(),
        );
        return;
    }

    // **Rows of fixed-width chips, counted out rather than wrapped.**
    //
    // These are short names and there are often twenty of them, and a dock
    // column is not tall enough to scroll through a list that long on top of
    // the slots above it. Two things forced the arithmetic:
    //
    // * `horizontal_wrapped` does not wrap these. Every chip is a drag source,
    //   a drag source is a `scope`, and a scope is laid out as one unit that
    //   the wrapping never looks inside — so thirteen chips came out on one
    //   row, a thousand points past the end of the column.
    // * A *truncating* button takes all the width on offer, so a chip sized to
    //   whatever is left fills the row by itself.
    //
    // So the width is a constant and the number per row is worked out from the
    // column. `dock_columns` is what catches this if it drifts.
    let per_row = ((ui.available_width() / (CHIP + 8.0)).floor() as usize).max(1);
    for row in loose.chunks(per_row) {
        ui.horizontal(|ui| {
            for part in row {
                let drag_id = ui.id().with(("loose-part", part.object.0));
                let response = ui
                    .dnd_drag_source(drag_id, DraggedPart::Object(part.object), |ui| {
                        ui.add_sized(
                            egui::vec2(CHIP, ui.spacing().interact_size.y),
                            egui::Button::new(RichText::new(&part.name).small()).truncate(),
                        )
                    })
                    .inner;

                // An armed slot turns the tray into a second way of filling it,
                // for a drawing that is buried under three others on the stage
                // and cannot be clicked at all.
                if response.clicked()
                    && let Some(slot) = state.armed
                {
                    state.assign(slot, part.object);
                }
                response.on_hover_text(match state.armed {
                    Some(_) => format!(
                        "{}\n\nClick to put it in the armed slot, or drag it to another",
                        part.name
                    ),
                    None => format!("{}\n\nDrag this onto a slot", part.name),
                });
            }
        });
    }
}

/// Which drawing a drag was carrying.
///
/// A layer stands for the one drawing on it; a layer holding none or several
/// has no single answer, and the caller says so rather than choosing.
fn resolve(dragged: DraggedPart, parts: &[LoosePart]) -> Option<ObjectId> {
    match dragged {
        DraggedPart::Object(id) => parts.iter().any(|p| p.object == id).then_some(id),
        DraggedPart::Layer(layer) => {
            let mut on_it = parts.iter().filter(|p| p.layer == layer);
            let first = on_it.next()?;
            on_it.next().is_none().then_some(first.object)
        }
    }
}

/// Fill every slot the drawings' own names account for.
/// The naming profile's name for a slot ("Elbow L" -> `leftWrist` in the user's
/// convention, where the wrist layer is the forearm); `None` when the profile
/// has no such part.
/// What a slot is called on screen and what its layer gets named when it is
/// tagged: the user's own name for it (`leftArm`), else the slot's.
pub fn slot_label(state: &RigPanelState, slot: &str) -> String {
    profile_name(state.profile.as_ref(), slot).unwrap_or_else(|| slot.to_string())
}

pub fn profile_name(profile: Option<&buzz_rig::pattern::NamingProfile>, slot: &str) -> Option<String> {
    let profile = profile?;
    let (stem, right) = match (slot.strip_suffix(" L"), slot.strip_suffix(" R")) {
        (Some(stem), _) => (stem, false),
        (_, Some(stem)) => (stem, true),
        _ => (slot, false),
    };
    let part = profile.parts.iter().find(|p| p.part == stem)?;
    Some(if right { part.right.clone() } else { part.left.clone() })
}

/// Fill the slots from the layer names: first a layer named exactly as the
/// naming profile names a slot (`leftWrist` is the forearm, not the wrist),
/// then by what the remaining names say (`L_upperArm`, `torso`).
fn auto_assign(pattern: &RigPattern, parts: &[LoosePart], state: &mut RigPanelState) {
    let names: Vec<String> = parts.iter().map(|p| p.name.clone()).collect();
    let mut taken = vec![false; parts.len()];
    let mut chosen: Vec<Option<usize>> = pattern
        .slots
        .iter()
        .map(|slot| {
            let wanted = profile_name(state.profile.as_ref(), &slot.name)?;
            let index = (0..parts.len()).find(|&i| !taken[i] && names[i].trim().eq_ignore_ascii_case(&wanted))?;
            taken[index] = true;
            Some(index)
        })
        .collect();
    let matched = buzz_rig::match_parts(pattern, &names);
    for (slot, found) in matched.iter().enumerate() {
        if let Some(index) = *found
            && chosen[slot].is_none()
            && !taken[index]
        {
            taken[index] = true;
            chosen[slot] = Some(index);
        }
    }

    let mut filled = 0;
    state.slots = chosen
        .iter()
        .map(|found| {
            found.and_then(|index| {
                filled += 1;
                parts.get(index).map(|p| p.object)
            })
        })
        .collect();
    state.armed = None;
    state.note = Some(match filled {
        0 => "No layer name looked like a body part. Sort them by hand, or rename the \
              layers and try again."
            .to_string(),
        _ => format!(
            "Named {filled} of {} from the layer names.",
            state.slots.len()
        ),
    });
}

// ---------------------------------------------------------------------------
// A rig that already exists.
// ---------------------------------------------------------------------------

/// The slots of a rig that was assembled from a pattern, and what is in them.
///
/// The one gesture offered here is **replacement**: drop a redrawn arm on
/// "Elbow L" and it takes over that bone. Re-assembling the whole character
/// would mean unpacking it back onto the stage first, which is a decision the
/// animator should make with the Bone tool rather than have made for them by a
/// drop.
fn rigged_slots(
    ui: &mut Ui,
    pattern_name: &str,
    filled: &[(usize, String)],
    parts: &[LoosePart],
    out: &mut RigResponse,
) {
    let Some(pattern) = RigPattern::named(pattern_name) else {
        // A pattern this build does not have — a file from a later version, or
        // one that was renamed. The bone list below still works.
        ui.label(
            RichText::new(format!(
                "Rigged to an unknown pattern, \u{201C}{pattern_name}\u{201D}."
            ))
            .small()
            .weak(),
        );
        return;
    };

    ui.horizontal(|ui| {
        ui.heading("Rig");
        ui.label(RichText::new(&pattern.name).small().weak());
    });
    ui.label(
        RichText::new("Drag a drawing onto a slot to replace what is in it.")
            .small()
            .weak()
            .italics(),
    );

    for (index, slot) in pattern.slots.iter().enumerate() {
        let holding = filled
            .iter()
            .find(|(bone, _)| *bone == index)
            .map(|(_, name)| name.as_str());

        ui.push_id(("bound-slot", index), |ui| {
            let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(1, 0));
            let (_, dropped) = ui.dnd_drop_zone::<DraggedPart, _>(frame, |ui| {
                ui.horizontal(|ui| {
                    let mark = if holding.is_some() {
                        "\u{25CF}"
                    } else {
                        "\u{25CB}"
                    };
                    ui.label(RichText::new(mark).small().color(if holding.is_some() {
                        Palette::active()
                    } else {
                        Palette::text_dim()
                    }));
                    ui.add_sized(
                        egui::vec2(64.0, ui.spacing().interact_size.y),
                        egui::Label::new(RichText::new(&slot.name).small()).truncate(),
                    );
                    match holding {
                        Some(name) => {
                            ui.add(egui::Label::new(RichText::new(name).small()).truncate());
                        }
                        None => {
                            ui.label(RichText::new("bone only").small().weak().italics());
                        }
                    }
                });
            });

            if let Some(payload) = dropped
                && let Some(object) = resolve(*payload, parts)
            {
                out.replace_part = Some((index, object));
            }
        });
    }
}

// ---------------------------------------------------------------------------
// The bones themselves, and the pose library.
// ---------------------------------------------------------------------------

fn armature_section(
    ui: &mut Ui,
    armature: Option<&Armature>,
    poses: &[buzz_scene::NamedPose],
    state: &mut RigPanelState,
    response: &mut RigResponse,
) {
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
                "Select a rigged object to edit its bones.\n\nTo move a limb: pick up the \
                 Bone tool (M) and drag a bone \u{2014} anywhere along it, including its \
                 end. The chain above bends to follow; everything below is carried along.\
                 \n\nTo build a skeleton: set Drag to \u{201c}builds\u{201d}, drag across \
                 artwork to lay the first bone, then drag from its end to add the next.",
            )
            .small()
            .weak(),
        );
        return;
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
    pose_library(ui, poses, state, response);
    ui.separator();

    // The dock column already scrolls; see the note on `tool_bar`.

    let bones = armature.bones.len();
    for (index, bone) in armature.bones.iter().enumerate() {
        bone_row(ui, index, bone, bones, response);
    }
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

        // How far the joint may turn, relative to the angle it was drawn at —
        // the same widget as the Bone tool's Tool Options.
        match crate::joint_widget::joint_limits(ui, bone, index) {
            Some(crate::joint_widget::JointEdit::Set { index, limits }) => {
                response.set_limits = Some((index, limits.map(|l| (l.min, l.max))));
            }
            Some(edit) => response.apply_limits = Some(edit),
            None => {}
        }
        ui.separator();
    });
}

/// A quarter turn, in radians: the range a newly limited joint starts with.
pub(crate) const FRAC_QUARTER: f64 = std::f64::consts::FRAC_PI_2;

/// How wide one loose part sits in the tray. Two to a row in the narrowest
/// dock column, which is the arrangement `dock_columns` measures.
const CHIP: f32 = 88.0;

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_rig::Bone;
    use buzz_scene::LayerId;

    fn part(id: u64, name: &str) -> LoosePart {
        LoosePart {
            object: ObjectId(id),
            layer: LayerId(1),
            name: name.to_string(),
        }
    }

    #[test]
    fn a_response_starts_empty() {
        let response = RigResponse::default();
        assert!(response.set_limits.is_none());
        assert!(response.set_pinned.is_none());
        assert!(response.build_rig.is_none());
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

    /// LR2: a fresh Rig panel builds a layer rig.
    #[test]
    fn a_fresh_rig_panel_defaults_to_a_layer_rig() {
        assert_eq!(RigPanelState::default().kind, RigKind::Layers);
    }

    /// The button that does most of the work, checked without a screen: the
    /// names go in, the slots come out filled.
    /// With the user's naming profile, the slots are called by their names
    /// and their names fill them: `leftWrist` is the forearm (Elbow L) and
    /// `leftPalm` the hand (Wrist L), not what the words alone would say.
    #[test]
    fn the_users_names_tag_the_parts() {
        let pattern = RigPattern::biped_full();
        let profile = buzz_rig::pattern::NamingProfile::animate_convention();
        assert_eq!(profile_name(Some(&profile), "Elbow L").as_deref(), Some("leftWrist"));
        assert_eq!(profile_name(Some(&profile), "Shoulder R").as_deref(), Some("rightArm"));
        let parts = vec![part(1, "body"), part(2, "head"), part(3, "leftArm"), part(4, "leftWrist"), part(5, "leftPalm")];
        let mut state = RigPanelState { profile: Some(profile), ..RigPanelState::default() };
        state.slots = vec![None; pattern.slots.len()];
        auto_assign(&pattern, &parts, &mut state);
        let at = |name: &str| state.slots[pattern.slot_named(name).unwrap()];
        assert_eq!(at("Chest"), Some(ObjectId(1)));
        assert_eq!(at("Head"), Some(ObjectId(2)));
        assert_eq!(at("Shoulder L"), Some(ObjectId(3)));
        assert_eq!(at("Elbow L"), Some(ObjectId(4)), "leftWrist is the forearm");
        assert_eq!(at("Wrist L"), Some(ObjectId(5)), "leftPalm is the hand");
    }

    #[test]
    fn auto_assign_fills_the_slots_from_the_layer_names() {
        let pattern = RigPattern::biped();
        let parts = vec![
            part(1, "hips"),
            part(2, "torso"),
            part(3, "head"),
            part(4, "L_upperArm"),
            part(5, "L_forearm"),
        ];
        let mut state = RigPanelState::default();
        state.slots = vec![None; pattern.slots.len()];

        auto_assign(&pattern, &parts, &mut state);

        assert_eq!(state.slots[0], Some(ObjectId(1)));
        assert_eq!(state.slots[1], Some(ObjectId(2)));
        assert_eq!(state.slots[2], Some(ObjectId(3)));
        assert_eq!(
            state.slots[pattern.slot_named("Shoulder L").unwrap()],
            Some(ObjectId(4))
        );
        assert_eq!(
            state.slots[pattern.slot_named("Elbow L").unwrap()],
            Some(ObjectId(5))
        );
        assert!(state.note.is_some(), "the panel said nothing about it");
    }

    /// One drawing is one limb. Leaving it in both slots would bind the same
    /// artwork to two bones and move it in two directions at once.
    #[test]
    fn assigning_a_drawing_takes_it_out_of_the_slot_it_was_in() {
        let mut state = RigPanelState {
            slots: vec![None; 4],
            ..Default::default()
        };
        state.assign(1, ObjectId(7));
        state.assign(3, ObjectId(7));

        assert_eq!(state.slots[1], None);
        assert_eq!(state.slots[3], Some(ObjectId(7)));
    }

    #[test]
    fn filling_an_armed_slot_disarms_it() {
        let mut state = RigPanelState {
            slots: vec![None; 3],
            armed: Some(2),
            ..Default::default()
        };
        state.assign(2, ObjectId(4));
        assert_eq!(state.armed, None);
    }

    /// A drawing that has been deleted, or that has just been rigged into a
    /// character, must not go on sitting in a slot: the next Rig Character
    /// would look for an object that is not there.
    #[test]
    fn a_drawing_that_has_gone_leaves_the_slot_it_was_in() {
        let pattern = RigPattern::prop();
        let mut state = RigPanelState {
            slots: vec![Some(ObjectId(1)), Some(ObjectId(2)), None],
            ..Default::default()
        };

        state.reconcile(&pattern, &[part(1, "base")]);
        assert_eq!(state.slots[0], Some(ObjectId(1)));
        assert_eq!(state.slots[1], None, "a deleted drawing stayed in its slot");
    }

    /// Changing the pattern changes the number of slots, and the old
    /// assignment means nothing against the new skeleton.
    #[test]
    fn a_different_pattern_gets_a_fresh_slot_list() {
        let mut state = RigPanelState {
            slots: vec![Some(ObjectId(1)); 11],
            armed: Some(9),
            ..Default::default()
        };
        state.reconcile(&RigPattern::prop(), &[part(1, "base")]);

        assert_eq!(state.slots.len(), 3);
        assert!(state.slots.iter().all(Option::is_none));
        assert_eq!(
            state.armed, None,
            "a slot that no longer exists stayed armed"
        );
    }

    /// A layer stands for the one drawing on it. Two drawings on it is a
    /// question, not an answer.
    #[test]
    fn a_layer_resolves_to_its_drawing_only_when_there_is_exactly_one() {
        let one = vec![part(1, "arm")];
        assert_eq!(
            resolve(DraggedPart::Layer(LayerId(1)), &one),
            Some(ObjectId(1))
        );

        let two = vec![part(1, "arm"), part(2, "arm shadow")];
        assert_eq!(resolve(DraggedPart::Layer(LayerId(1)), &two), None);
        assert_eq!(resolve(DraggedPart::Layer(LayerId(9)), &one), None);
    }

    #[test]
    fn a_dragged_drawing_that_is_no_longer_loose_is_refused() {
        let parts = vec![part(1, "arm")];
        assert_eq!(
            resolve(DraggedPart::Object(ObjectId(1)), &parts),
            Some(ObjectId(1))
        );
        assert_eq!(resolve(DraggedPart::Object(ObjectId(2)), &parts), None);
    }

    #[test]
    /// **Rig Head**: the head's layers start on their suggested names, and
    /// Name Layers asks for exactly the ones that change.
    #[test]
    fn the_head_section_names_the_layers() {
        let ctx = egui::Context::default();
        let head = buzz_scene::SymbolId(9);
        let (white, ball, nose) = (buzz_scene::LayerId(1), buzz_scene::LayerId(2), buzz_scene::LayerId(3));
        let mut state = RigPanelState {
            head: Some(HeadSection {
                symbol: head,
                name: "frontHead".into(),
                layers: vec![
                    (white, "EyeWhite".into(), Some("eyeWhite".into())),
                    (ball, "eyeBall".into(), Some("eyeBall".into())),
                    (nose, "Layer_7".into(), None),
                ],
            }),
            ..RigPanelState::default()
        };
        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = rig_panel(ui, None, &[], &[], None, &mut state, &mut Default::default());
        });
        assert_eq!(state.head_tags_for, Some(head));
        assert_eq!(
            state.head_tags,
            vec![(white, "eyeWhite".to_string()), (ball, "eyeBall".to_string()), (nose, "Layer_7".to_string())],
            "suggested names, else the name it has"
        );
        // What Name Layers would send: only the layer whose name changes.
        let renames: Vec<_> = state
            .head_tags
            .iter()
            .filter(|(id, tag)| state.head.as_ref().unwrap().layers.iter().any(|(l, now, _)| l == id && now != tag))
            .cloned()
            .collect();
        assert_eq!(renames, vec![(white, "eyeWhite".to_string())]);
    }

    #[test]
    fn the_panel_draws_in_every_state_it_has() {
        let ctx = egui::Context::default();
        let mut armature = Armature::new(buzz_geom::Point::ZERO);
        armature.push(Bone::new("upper", None, 50.0, 0.0));
        armature.push(
            Bone::new("fore", Some(0), 40.0, 0.3)
                .with_limits(-1.0, 1.0)
                .pinned(),
        );
        let parts = vec![part(1, "L_arm"), part(2, "head")];

        // egui 0.35 roots the UI in a `Ui` rather than a `Context`.
        let _ = ctx.run_ui(Default::default(), |ui| {
            let mut state = RigPanelState::default();
            // Nothing selected, nothing on the stage.
            let empty = rig_panel(
                ui,
                None,
                &[],
                &[],
                None,
                &mut state,
                &mut Default::default(),
            );
            assert_eq!(empty, RigResponse::default());
            // Parts on the stage, waiting to be sorted.
            let _ = rig_panel(
                ui,
                None,
                &[],
                &parts,
                None,
                &mut state,
                &mut Default::default(),
            );
            // A rig selected, built the old way.
            let _ = rig_panel(
                ui,
                Some(&armature),
                &[],
                &parts,
                None,
                &mut state,
                &mut Default::default(),
            );
            // A rig selected that was assembled from a pattern.
            let bound = [(0usize, "L_arm".to_string())];
            let _ = rig_panel(
                ui,
                Some(&armature),
                &[],
                &parts,
                Some(("Prop", &bound)),
                &mut state,
                &mut Default::default(),
            );
            // And one whose pattern this build has never heard of.
            let _ = rig_panel(
                ui,
                Some(&armature),
                &[],
                &parts,
                Some(("Centaur", &bound)),
                &mut state,
                &mut Default::default(),
            );
        });
    }
}
