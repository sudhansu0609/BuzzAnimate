//! Set the Scene, Add Person and Animate Selection, on the editor's document.
//!
//! The thinking is all in `buzz-act`, which knows nothing about documents,
//! undo or selection. This is the strip of wiring between it and the editor:
//! translate the dialog's choices, run it inside one `Document::edit` so a
//! whole scene or a whole walk is one Ctrl+Z, and say what happened.

use buzz_act::{Action, FigureSpec, Performance, SceneRecipe, Setting};
use buzz_scene::{LayerKind, ObjectId, Scene};

use crate::editor::Editor;

/// The dialog's setting, as the staging crate's.
pub fn setting_of(choice: buzz_ui::SettingChoice) -> Setting {
    match choice {
        buzz_ui::SettingChoice::Daylight => Setting::Daylight,
        buzz_ui::SettingChoice::Sunset => Setting::Sunset,
        buzz_ui::SettingChoice::Night => Setting::Night,
        buzz_ui::SettingChoice::Interior => Setting::Interior,
        buzz_ui::SettingChoice::Storm => Setting::Storm,
    }
}

/// The dialog's action, as the staging crate's.
pub fn action_of(choice: buzz_ui::ActionChoice) -> Action {
    match choice {
        buzz_ui::ActionChoice::Walk => Action::Walk,
        buzz_ui::ActionChoice::Run => Action::Run,
        buzz_ui::ActionChoice::Talk => Action::Talk,
        buzz_ui::ActionChoice::Idle => Action::Idle,
    }
}

impl Editor {
    /// **Set the scene**: ground, backdrop, lights and a cast.
    ///
    /// One edit, so the whole arrangement is one undo step — an animator who
    /// does not like it must be able to take all of it back with one press,
    /// not thirty.
    pub fn set_the_scene(&mut self, state: &buzz_ui::StagingState) {
        let recipe = SceneRecipe {
            setting: setting_of(state.setting),
            cast: state.cast,
            horizon: state.horizon,
            figure_scale: state.figure_scale,
            lit: state.lit,
            frames: state.frames,
            clouds: state.clouds,
            water: state.water,
        };

        let mut message = String::new();
        let mut first = None;
        self.doc.edit("Set the Scene", |scene| {
            let built = buzz_act::stage_scene(scene, &recipe);
            message = built.message.clone();
            first = built.actors().next();
        });
        self.doc.end_gesture();

        // The first of the cast is left selected, so Animate Selection has
        // something to act on straight away — which is the very next thing
        // anyone does after setting a scene.
        //
        // Not `after_context_change`: nothing about the *context* changed. That
        // one clears the selection and sends the playhead back to frame one,
        // which is right when you step into a symbol and wrong here, where the
        // whole point is to leave somebody selected to animate.
        match first {
            Some(id) => self.select_and_activate(id),
            None => self.selection.clear(),
        }
        self.status = Some(message);
    }

    /// **Direct the typed story**: stage it, cast it, animate it — one edit,
    /// one undo step.
    ///
    /// What could not be read stays visible twice over: the director lists
    /// the skipped sentences in its message, and a story with nothing
    /// readable at all is refused with the reason in the dialog, leaving the
    /// document exactly as it was.
    pub fn direct_story(&mut self, state: &mut buzz_ui::StagingState) {
        let story = state.story.clone();

        // **A brief with more than one shot in it becomes more than one scene.**
        //
        // The writing already says where the cuts go — a blank line, or a line
        // that is only a setting — so a page of prose comes out as an animatic
        // rather than as one impossibly busy scene. A short brief splits into
        // one shot and takes exactly the path it always did, which is why this
        // needed no second button.
        if buzz_act::split_shots(&story).len() > 1 {
            let directed = self.direct_sequence(&story);
            if directed > 0 {
                state.problem = None;
                state.close();
            } else {
                state.problem = self.status.clone();
            }
            return;
        }

        let mut outcome: Option<Result<buzz_act::DirectedScene, buzz_act::DirectError>> = None;
        self.doc.edit("Direct a Story", |scene| {
            outcome = Some(buzz_act::direct(scene, &story));
        });
        self.doc.end_gesture();

        match outcome {
            Some(Ok(directed)) => {
                state.problem = None;
                state.close();
                match directed.staged.actors().next() {
                    Some(id) => self.select_and_activate(id),
                    None => self.selection.clear(),
                }
                self.status = Some(directed.message);
            }
            Some(Err(e)) => {
                // The edit produced nothing; drop it rather than leaving an
                // empty step in the history for the user to undo past.
                self.doc.undo();
                state.problem = Some(e.to_string());
            }
            None => state.problem = Some("Nothing happened".into()),
        }
    }

    /// **Put one more person on the stage**, on a layer of their own.
    ///
    /// A layer each, because two characters in a shot are performed at
    /// different times and a performance writes keyframes onto a layer.
    pub fn add_person(&mut self) {
        let stage = self.doc.scene().stage().stage_rect();
        let spec = FigureSpec {
            height: stage.height() * 0.62,
            ..FigureSpec::default()
        };
        // Where the camera is looking, on the ground: a character added while
        // the animator is looking somewhere else should arrive where they are
        // looking, not at the origin of a stage scrolled off screen.
        let stands_on = buzz_geom::Point::new(
            self.camera.center.x,
            stage.y0 + stage.height() * 0.9,
        );

        let mut placed = None;
        let mut name = String::new();
        self.doc.edit("Add Person", |scene| {
            let n = 1 + scene
                .stage_layers()
                .iter()
                .filter(|l| l.name.starts_with("Person"))
                .count();
            name = format!("Person {n}");
            let layer = scene.add_stage_layer(&name, LayerKind::Normal);

            let id = scene.next_object_id();
            let mut person = buzz_act::build_figure(&spec, id, || scene.next_object_id());
            person.name = Some(name.clone());
            person.transform =
                buzz_geom::Affine::translate(stands_on.to_vec2()) * person.transform;
            placed = scene.add_object(layer, person);

            // As long as the film, or the new arrival vanishes after frame one
            // while everything else carries on.
            let last = scene.frame_count().saturating_sub(1);
            scene.update_stage_layer(layer, |l| {
                if l.frames.length() <= last {
                    l.frames.insert_frame(last);
                }
            });
        });
        self.doc.end_gesture();

        match placed {
            Some(id) => {
                self.select_and_activate(id);
                self.status = Some(format!("{name} added \u{2014} Scene > Animate Selection"));
            }
            None => self.status = Some("There was nowhere to put a person".into()),
        }
    }

    /// Is the selection something a performance could drive?
    ///
    /// Asked by the dialog so the button can be disabled with a reason rather
    /// than the command failing after it is pressed.
    pub fn selection_is_performable(&self) -> bool {
        self.performable_selection().is_some()
    }

    /// The selected rig, if exactly one rigged thing is selected.
    fn performable_selection(&self) -> Option<ObjectId> {
        let scene: &Scene = self.doc.scene();
        self.selection
            .iter()
            .find(|id| {
                scene
                    .find_object(*id)
                    .is_some_and(|(_, object)| buzz_act::is_figure(object))
            })
    }

    /// **Animate the selection.** Writes ordinary pose keyframes and returns.
    ///
    /// The problem, if there is one, goes back into the dialog's own state: it
    /// is where the user is looking, and "nothing suitable is selected" written
    /// only into the status bar is a message nobody reads.
    pub fn perform_selection(&mut self, state: &mut buzz_ui::StagingState) {
        let Some(object) = self.performable_selection() else {
            state.problem = Some(
                "Select a rigged character first. Scene > Add Person makes one that is \
                 already rigged."
                    .into(),
            );
            return;
        };

        let action = action_of(state.action);
        let performance = Performance {
            action,
            frames: state.range(),
            amount: state.amount,
            tempo: state.tempo,
            distance: if action.travels() { state.distance } else { 0.0 },
            step: state.step,
        };

        let mut outcome = None;
        self.doc.edit(action.undo_label(), |scene| {
            outcome = Some(buzz_act::perform(scene, object, &performance));
        });
        self.doc.end_gesture();

        match outcome {
            Some(Ok(report)) => {
                state.problem = None;
                state.close();
                self.status = Some(report.message);
            }
            Some(Err(e)) => {
                // The edit produced nothing usable; drop it rather than leaving
                // an empty step in the history for the user to undo past.
                self.doc.undo();
                state.problem = Some(e.to_string());
            }
            None => state.problem = Some("Nothing happened".into()),
        }
    }

    /// **Stash a drawn motion path and open the dialog** to send the selected
    /// object along it.
    ///
    /// The object is taken now, not when the dialog confirms: the animator drew
    /// the curve for the thing that was selected, and a stray click before
    /// pressing Send should not redirect the path onto something else.
    pub fn begin_motion_path(&mut self, path: buzz_geom::BezPath) {
        let object = {
            let scene = self.doc.scene();
            self.selection
                .iter()
                .find(|id| scene.find_object(*id).is_some())
        };
        let Some(object) = object else {
            self.status =
                Some("Select an object first, then draw a motion path for it to follow".into());
            return;
        };

        self.pending_motion_path = Some((path, object));
        // Over the whole film from the playhead, matching the performance
        // dialog: an animator on frame 12 means "travel from here".
        let last = self.doc.scene().frame_count().saturating_sub(1);
        let from = self.current_frame.min(last);
        self.staging.open_motion_path(from, last);
    }

    /// **Send the object along the drawn path.** Bakes ordinary transform
    /// keyframes and returns, as one undo step.
    ///
    /// Like [`Self::perform_selection`], a problem goes back into the dialog's
    /// own state rather than only the status bar, because the dialog is where
    /// the user is looking.
    pub fn follow_motion_path(&mut self, state: &mut buzz_ui::StagingState) {
        let Some((path, object)) = self.pending_motion_path.clone() else {
            state.problem = Some("Draw a path first".into());
            return;
        };
        if self.doc.scene().find_object(object).is_none() {
            self.pending_motion_path = None;
            state.problem = Some("The object to move is no longer on the stage".into());
            return;
        }

        let opts = buzz_act::MotionPathOptions {
            frames: state.range(),
            easing: buzz_scene::Easing::Strength(state.ease),
            orient_to_path: state.orient_to_path,
            step: state.step,
        };

        let mut outcome = None;
        self.doc.edit("Motion Path", |scene| {
            outcome = Some(buzz_act::follow_path(scene, object, &path, &opts));
        });
        self.doc.end_gesture();

        match outcome {
            Some(Ok(report)) => {
                self.pending_motion_path = None;
                state.problem = None;
                state.close();
                self.select_and_activate(object);
                self.status = Some(report.message);
            }
            Some(Err(e)) => {
                // The edit produced nothing usable; drop it rather than leaving
                // an empty step in the history.
                self.doc.undo();
                state.problem = Some(e.to_string());
            }
            None => state.problem = Some("Nothing happened".into()),
        }
    }

    /// The selected rig, if exactly one rigged thing is selected. Any armature,
    /// not only a biped figure — a tail on a fish is a chain too.
    fn rigged_selection(&self) -> Option<ObjectId> {
        let scene: &Scene = self.doc.scene();
        self.selection.iter().find(|id| {
            matches!(
                scene.find_object(*id),
                Some((_, o)) if matches!(o.kind, buzz_scene::ObjectKind::Armature(_))
            )
        })
    }

    /// **Add follow-through** to the selected rig's chain. Bakes ordinary pose
    /// keyframes and returns, as one undo step.
    pub fn add_follow_through(&mut self, state: &mut buzz_ui::StagingState) {
        let Some(object) = self.rigged_selection() else {
            state.problem = Some(
                "Select a rigged character first. Scene > Add Person makes one that is \
                 already rigged."
                    .into(),
            );
            return;
        };

        let root = state.physics_root;
        // A modest default strength: enough to read as weight, small enough that a
        // normal walk does not fling the hair. The clamp in the solver guards the
        // extremes.
        let coupling = if state.physics_couple { 0.012 } else { 0.0 };

        // Live: attach a modifier evaluated every frame, so re-timing the body
        // re-follows the hair with nothing to re-bake. Re-adding replaces an
        // existing spring rather than stacking a second one.
        if state.physics_live {
            self.doc.edit("Add Spring", |scene| {
                scene.update_object_across(0, u32::MAX, object, |o| {
                    o.modifiers
                        .retain(|m| !matches!(m, buzz_scene::Modifier::Spring { .. }));
                    o.modifiers.push(buzz_scene::Modifier::Spring {
                        root,
                        stiffness: state.spring_stiffness,
                        damping: state.spring_damping,
                        coupling,
                    });
                });
            });
            self.doc.end_gesture();
            state.problem = None;
            state.close();
            self.select_and_activate(object);
            self.status = Some("Follow-through added \u{2014} live".into());
            return;
        }

        let spring = buzz_act::Spring::new(state.spring_stiffness, state.spring_damping);
        let frames = state.range();
        let step = state.step;

        let mut outcome = None;
        self.doc.edit("Follow-Through", |scene| {
            outcome = Some(buzz_act::follow_through_bake(
                scene, object, root, spring, frames, step, coupling,
            ));
        });
        self.doc.end_gesture();

        match outcome {
            Some(Ok(report)) => {
                state.problem = None;
                state.close();
                self.select_and_activate(object);
                self.status = Some(report.message);
            }
            Some(Err(e)) => {
                self.doc.undo();
                state.problem = Some(e.to_string());
            }
            None => state.problem = Some("Nothing happened".into()),
        }
    }

    /// **Add a wiggle** to the selected object: an idle sway, a breeze, a shake.
    /// Works on any object, rig or not.
    pub fn add_wiggle(&mut self, state: &mut buzz_ui::StagingState) {
        let object = {
            let scene = self.doc.scene();
            self.selection
                .iter()
                .find(|id| scene.find_object(*id).is_some())
        };
        let Some(object) = object else {
            state.problem = Some("Select an object first.".into());
            return;
        };

        // Live: attach a wiggle modifier rather than baking keyframes.
        if state.wiggle_live {
            self.doc.edit("Add Wiggle", |scene| {
                scene.update_object_across(0, u32::MAX, object, |o| {
                    o.modifiers
                        .retain(|m| !matches!(m, buzz_scene::Modifier::Wiggle { .. }));
                    o.modifiers.push(buzz_scene::Modifier::Wiggle {
                        amplitude: state.wiggle_amplitude,
                        frequency: state.wiggle_frequency,
                    });
                });
            });
            self.doc.end_gesture();
            state.problem = None;
            state.close();
            self.select_and_activate(object);
            self.status = Some("Wiggle added \u{2014} live".into());
            return;
        }

        let wiggle = buzz_act::Wiggle::new(state.wiggle_amplitude, state.wiggle_frequency);
        let frames = state.range();
        let step = state.step;

        let mut outcome = None;
        self.doc.edit("Wiggle", |scene| {
            outcome = Some(buzz_act::wiggle_bake(scene, object, wiggle, frames, step));
        });
        self.doc.end_gesture();

        match outcome {
            Some(Ok(report)) => {
                state.problem = None;
                state.close();
                self.select_and_activate(object);
                self.status = Some(report.message);
            }
            Some(Err(e)) => {
                self.doc.undo();
                state.problem = Some(e.to_string());
            }
            None => state.problem = Some("Nothing happened".into()),
        }
    }

    /// Select one object and make its layer the active one.
    ///
    /// The pair belongs together: a selection whose layer is not active leaves
    /// the timeline highlighting a different row from the one the artwork is
    /// on, which is the arrangement `ToolAction::PickAt` already goes out of
    /// its way to avoid.
    fn select_and_activate(&mut self, id: ObjectId) {
        self.selection.select_one(id);
        if let Some((layer, _)) = self.doc.scene().find_object(id) {
            self.selection.set_active_layer(Some(layer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_ui::{ActionChoice, SettingChoice, StagingState};

    /// Setting a scene is **one** undo step. Thirty layers and two characters
    /// that have to be undone one at a time is not something anyone would use
    /// twice.
    #[test]
    fn a_whole_scene_is_one_undo_step() {
        let mut e = Editor::default();
        let before = e.scene().stage_layers().len();

        e.set_the_scene(&StagingState::default());
        assert!(
            e.scene().stage_layers().len() > before + 2,
            "a scene arrived"
        );

        e.doc.undo();
        assert_eq!(
            e.scene().stage_layers().len(),
            before,
            "and one press took all of it back"
        );
    }

    /// The first of the cast is left selected, because animating them is the
    /// very next thing anybody does.
    #[test]
    fn setting_a_scene_leaves_someone_selected() {
        let mut e = Editor::default();
        e.set_the_scene(&StagingState::default());
        assert!(e.selection_is_performable(), "and they can be animated");
    }

    /// A scene with nobody in it is a background, and is allowed.
    #[test]
    fn a_scene_can_have_no_cast() {
        let mut e = Editor::default();
        e.set_the_scene(&StagingState {
            cast: 0,
            ..StagingState::default()
        });
        assert!(!e.selection_is_performable());
        assert!(e.scene().stage_layers().len() >= 3, "a sky and a ground");
    }

    /// Adding a person puts a rigged character on a layer of their own and
    /// selects them.
    #[test]
    fn add_person_leaves_a_rigged_character_selected() {
        let mut e = Editor::default();
        let before = e.scene().stage_layers().len();
        e.add_person();
        assert_eq!(e.scene().stage_layers().len(), before + 1);
        assert!(e.selection_is_performable());
    }

    /// **A performance really lands, and it is one undo step.**
    #[test]
    fn a_performance_is_written_and_is_one_undo_step() {
        let mut e = Editor::default();
        e.add_person();
        let keys_before = keyframes(&e);

        let mut state = StagingState {
            action: ActionChoice::Walk,
            from_frame: 0,
            to_frame: 23,
            ..StagingState::default()
        };
        e.perform_selection(&mut state);
        assert!(state.problem.is_none(), "it ran: {:?}", state.problem);
        assert!(keyframes(&e) > keys_before + 5, "keys were written");

        e.doc.undo();
        assert_eq!(keyframes(&e), keys_before, "and one press took them back");
    }

    /// Asking to animate a drawing with no bones says why, in the dialog, and
    /// leaves no empty step in the history.
    #[test]
    fn animating_something_unrigged_says_why() {
        let mut e = Editor::default();
        let mut state = StagingState {
            action: ActionChoice::Idle,
            ..StagingState::default()
        };
        // Nothing selected at all.
        e.perform_selection(&mut state);
        let said = state.problem.clone().unwrap_or_default();
        assert!(
            said.contains("rigged"),
            "the reason should name what is missing, got {said:?}"
        );
        assert_eq!(state.open, None, "and the dialog stays as it was");
    }

    /// Every setting the dialog offers really maps to one the staging crate
    /// knows, so a new one cannot be added to the menu and quietly do nothing.
    #[test]
    fn every_choice_maps_through() {
        for choice in SettingChoice::ALL {
            let mapped = setting_of(choice);
            assert_eq!(mapped.label(), choice.label());
        }
        for choice in ActionChoice::ALL {
            let mapped = action_of(choice);
            assert_eq!(mapped.label(), choice.label());
        }
    }

    fn keyframes(e: &Editor) -> usize {
        e.scene()
            .stage_layers()
            .iter()
            .map(|l| l.frames.keyframe_count())
            .sum()
    }

    /// **A story becomes a scene, and it is one undo step.** The whole
    /// promise of Direct a Story, through the editor: set, cast, keyframes,
    /// somebody selected — and one press takes all of it back.
    #[test]
    fn a_directed_story_is_one_undo_step() {
        let mut e = Editor::default();
        let layers_before = e.scene().stage_layers().len();
        let mut state = StagingState {
            story: "Night. Ana walks in from the left. Ana talks to Ben for 3 seconds. \
                    Ben walks off right."
                .into(),
            ..StagingState::default()
        };

        e.direct_story(&mut state);
        assert!(state.problem.is_none(), "it ran: {:?}", state.problem);
        assert!(
            e.scene().stage_layers().len() >= layers_before + 4,
            "a sky, a ground, Ana and Ben"
        );
        assert!(keyframes(&e) > 10, "the performances are keyframed");
        assert!(e.selection_is_performable(), "somebody is selected");
        let said = e.status.clone().unwrap_or_default();
        assert!(
            said.contains("Ana") && said.contains("Ben"),
            "the report names the cast: {said:?}"
        );

        e.doc.undo();
        assert_eq!(
            e.scene().stage_layers().len(),
            layers_before,
            "one press took the whole scene back"
        );
    }

    /// A story the director cannot read is refused with the reason in the
    /// dialog, and the document is left untouched.
    #[test]
    fn an_unreadable_story_says_why_and_changes_nothing() {
        let mut e = Editor::default();
        let layers_before = e.scene().stage_layers().len();
        let mut state = StagingState {
            story: "The rain fell and fell.".into(),
            ..StagingState::default()
        };
        e.direct_story(&mut state);
        assert!(
            state.problem.as_deref().unwrap_or_default().contains("try"),
            "the reason offers an example: {:?}",
            state.problem
        );
        assert_eq!(e.scene().stage_layers().len(), layers_before);
        assert_eq!(state.open, None, "the dialog state is left to the caller");
    }
}
