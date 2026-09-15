//! The Story panel, on the editor's document.
//!
//! The thinking is all in `buzz-act`, which knows nothing about documents, undo
//! or panels; the panel is all in `buzz-ui`, which knows nothing about staging.
//! This is the strip of wiring between them — the same arrangement
//! [`crate::staging`] has, and for the same reason.

use buzz_act::scenery::{Scenery, SceneryArt, SceneryPart};
use buzz_act::SceneRecipe;
use buzz_scene::SymbolId;

use crate::editor::Editor;

/// The panel's scenery, as the staging crate's.
pub fn scenery_of(choice: buzz_ui::SceneryChoice) -> Scenery {
    match choice {
        buzz_ui::SceneryChoice::Bare => Scenery::Bare,
        buzz_ui::SceneryChoice::Forest => Scenery::Forest,
        buzz_ui::SceneryChoice::Village => Scenery::Village,
        buzz_ui::SceneryChoice::City => Scenery::City,
        buzz_ui::SceneryChoice::Meadow => Scenery::Meadow,
        buzz_ui::SceneryChoice::Waterside => Scenery::Waterside,
    }
}

/// The panel's part, as the staging crate's.
pub fn part_of(choice: buzz_ui::SceneryPartChoice) -> SceneryPart {
    match choice {
        buzz_ui::SceneryPartChoice::Trees => SceneryPart::Trees,
        buzz_ui::SceneryPartChoice::Grass => SceneryPart::Grass,
        buzz_ui::SceneryPartChoice::Buildings => SceneryPart::Buildings,
        buzz_ui::SceneryPartChoice::Lamps => SceneryPart::Lamps,
    }
}

impl Editor {
    /// Everything the Story panel needs to draw: the shot list, with the words
    /// each shot was made from.
    pub fn shot_summaries(&self) -> Vec<buzz_ui::ShotSummary> {
        let names = self.doc.scene_names();
        self.doc
            .film()
            .iter()
            .enumerate()
            .map(|(i, scene)| buzz_ui::ShotSummary {
                name: names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("Scene {}", i + 1)),
                brief: scene.brief().to_string(),
                frames: scene.frame_count(),
                layers: scene.stage_layers().len(),
            })
            .collect()
    }

    /// The library, as the panel offers it for the scenery parts.
    ///
    /// The document's own timeline, not whatever symbol happens to be open:
    /// choosing a tree is a choice about the film.
    pub fn library_choices(&self) -> Vec<(u64, String)> {
        self.doc
            .scene()
            .library()
            .iter()
            .map(|symbol| (symbol.id.0, symbol.name.clone()))
            .collect()
    }

    /// The drawings chosen for each part, as the staging crate wants them.
    ///
    /// A symbol that has since been deleted is dropped rather than carried as a
    /// dangling id: the part falls back to the brush, which is the honest
    /// answer and is what the panel will show on its next frame.
    pub fn scenery_art(&self, state: &buzz_ui::StoryState) -> SceneryArt {
        let library = self.doc.scene().library();
        let mut art = SceneryArt::default();
        for part in buzz_ui::SceneryPartChoice::ALL {
            let chosen = state
                .art_for(part)
                .map(SymbolId)
                .filter(|id| library.get(*id).is_some());
            art.set(part_of(part), chosen);
        }
        art
    }

    /// **Set the scene from the Story panel.**
    ///
    /// The same call the dialog made, taking the panel's own settings. One
    /// edit, so a set an animator does not like is one press to take back.
    pub fn story_set_scene(&mut self, state: &buzz_ui::StoryState) {
        let recipe = SceneRecipe {
            setting: crate::staging::setting_of(state.setting),
            cast: state.cast,
            horizon: state.horizon,
            figure_scale: state.figure_scale,
            lit: state.lit,
            frames: state.frames,
            clouds: state.clouds,
            water: state.water,
        };
        let mut message = String::new();
        self.doc.edit("Set the Scene", |scene| {
            message = buzz_act::stage_scene(scene, &recipe).message;
        });
        self.doc.end_gesture();
        self.status = Some(message);
    }

    /// **Lay the chosen scenery into the shot that is open.**
    ///
    /// The horizon comes from the panel rather than from the set that is
    /// already there, because the two are set together and a treeline laid at a
    /// different horizon from the ground it stands on is the one mistake worth
    /// designing out.
    ///
    /// The backdrop is found by name, which is what `Set the Scene` calls it.
    /// Without one the scenery still lands, at the front — a shot with no sky
    /// has nothing to go behind.
    pub fn story_lay_scenery(&mut self, state: &buzz_ui::StoryState) -> usize {
        let what = scenery_of(state.scenery);
        if what == Scenery::Bare {
            self.status = Some("Bare scenery has nothing to lay".into());
            return 0;
        }
        let art = self.scenery_art(state);
        let stage = self.doc.scene().stage().stage_rect();
        let horizon_y = stage.y0 + stage.height() * state.horizon.clamp(0.15, 0.95);
        let backdrop = self
            .doc
            .scene()
            .stage_layers()
            .iter()
            .find(|l| l.name == "Sky")
            .map(|l| l.id);

        let mut report = buzz_act::scenery::SceneryReport::default();
        self.doc.edit("Lay Scenery", |scene| {
            report = buzz_act::scenery::lay_with(scene, what, horizon_y, backdrop, &art);
        });
        self.doc.end_gesture();

        let own = art.is_empty();
        self.status = Some(format!(
            "{}: {} piece(s) on {} layer(s){}",
            what.label(),
            report.pieces,
            report.layers.len(),
            if own { "" } else { ", from your own drawings" },
        ));
        report.pieces
    }

    /// **Direct the panel's draft into the shot that is open.**
    ///
    /// Replaces it: directing is how that shot is built, and laying a second
    /// staged scene over the first is never what re-directing means.
    pub fn story_direct(&mut self, state: &mut buzz_ui::StoryState) {
        let story = state.draft.clone();
        let mut outcome: Option<Result<buzz_act::DirectedScene, buzz_act::DirectError>> = None;
        self.doc.edit("Direct", |scene| {
            *scene = buzz_scene::Scene::default();
            outcome = Some(buzz_act::direct(scene, &story));
        });

        match outcome {
            Some(Ok(directed)) => {
                self.doc.end_gesture();
                let at = self.doc.active_scene();
                let title = buzz_act::split_shots(&story)
                    .first()
                    .map(|s| s.title.clone())
                    .unwrap_or_else(|| format!("Scene {}", at + 1));
                self.doc.rename_scene(at, title);
                state.ignored = directed.ignored.clone();
                state.report = Some(directed.message.clone());
                state.draft_from = Some(at);
                self.status = Some(directed.message);
                self.after_context_change();
            }
            Some(Err(e)) => {
                // Nothing was built, so nothing is kept — the shot is left
                // exactly as it was rather than as an empty scene.
                self.doc.undo();
                self.doc.end_gesture();
                state.report = Some(format!("{e}"));
                self.status = Some(format!("{e}"));
            }
            None => {}
        }
    }
}
