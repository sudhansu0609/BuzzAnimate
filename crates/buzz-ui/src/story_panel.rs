//! **The Story panel** — the brief, the set, and the scenery, all on screen.
//!
//! # Why a panel and not the dialogs it replaces
//!
//! *Direct a Story* and *Set the Scene* were modal dialogs, and a modal is the
//! wrong shape for both of them. A dialog is for a question asked once and
//! answered: pick a file, choose a size. Directing is not that. It is **written
//! and rewritten** — change a verb, look at the shot, change it back — and a
//! box that covers the stage while you type is a box you cannot see the result
//! through. You typed, you clicked OK, the dialog vanished, and if the blocking
//! was wrong you opened it again and started from the words you could remember.
//!
//! Worse: the prose itself was thrown away. The director laid out sixty layers
//! from a paragraph and kept no record of the paragraph, so a shot could not be
//! *edited*, only rebuilt from scratch. [`buzz_scene::Scene::brief`] fixes the
//! keeping; this panel is where it is read and changed.
//!
//! # Three things in one panel, because they are one job
//!
//! **The story** is the shot list and the words behind whichever shot you are
//! standing in. **The set** is where and when it is. **The scenery** is what is
//! in it. An animator setting up a shot moves between all three in a minute,
//! and they were three menu items in two menus.
//!
//! # Your own drawings
//!
//! The effect brushes fill a shot in one stroke and they are generic on
//! purpose. They are not *your* trees. Every part of a set can take a library
//! symbol instead, and the panel is where you say which — see
//! [`buzz_act::scenery::SceneryArt`], mirrored here as ids so this crate keeps
//! its independence from the staging one.

use egui::{RichText, Ui};

use crate::staging_panel::SettingChoice;

/// What is in the shot, beyond the ground and the sky.
///
/// A plain mirror of `buzz_act::scenery::Scenery`, kept here for the same
/// reason [`SettingChoice`] is: `buzz-ui` does not depend on the staging crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneryChoice {
    Bare,
    Forest,
    Village,
    City,
    Meadow,
    Waterside,
}

impl SceneryChoice {
    pub const ALL: [SceneryChoice; 6] = [
        SceneryChoice::Bare,
        SceneryChoice::Forest,
        SceneryChoice::Village,
        SceneryChoice::City,
        SceneryChoice::Meadow,
        SceneryChoice::Waterside,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bare => "Bare",
            Self::Forest => "Forest",
            Self::Village => "Village",
            Self::City => "City",
            Self::Meadow => "Meadow",
            Self::Waterside => "Waterside",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Bare => "Ground and sky, and nothing between them",
            Self::Forest => "A treeline along the horizon, grass at the front",
            Self::Village => "Low houses among trees, with the ground up to the door",
            Self::City => "A lit skyline, and street lamps down the path",
            Self::Meadow => "Open ground: grass near, a thin line of trees far off",
            Self::Waterside => "A shore, with the bank in front of it",
        }
    }
}

/// Which part of a set a drawing stands in for. Mirror of
/// `buzz_act::scenery::SceneryPart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneryPartChoice {
    Trees,
    Grass,
    Buildings,
    Lamps,
}

impl SceneryPartChoice {
    pub const ALL: [SceneryPartChoice; 4] = [
        SceneryPartChoice::Trees,
        SceneryPartChoice::Grass,
        SceneryPartChoice::Buildings,
        SceneryPartChoice::Lamps,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Trees => "Trees",
            Self::Grass => "Grass",
            Self::Buildings => "Buildings",
            Self::Lamps => "Lamps",
        }
    }

    /// What a drawing put in this part is used for, in one line.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Trees => "Stood along the horizon, jittered in size and spacing",
            Self::Grass => "Laid as a mat across the near ground, overlapping",
            Self::Buildings => "Stood in a row, shoulder to shoulder",
            Self::Lamps => "Spaced widely down the path",
        }
    }

    /// Where it sits in [`StoryState::art`].
    fn index(self) -> usize {
        match self {
            Self::Trees => 0,
            Self::Grass => 1,
            Self::Buildings => 2,
            Self::Lamps => 3,
        }
    }
}

/// One shot, as the panel shows it in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotSummary {
    pub name: String,
    /// The prose it was directed from, or empty for a shot built by hand.
    pub brief: String,
    pub frames: u32,
    pub layers: usize,
}

impl ShotSummary {
    /// The first line of the brief, for the list. Empty when there is none.
    fn first_line(&self) -> &str {
        self.brief.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
    }
}

/// Everything the panel remembers between frames.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryState {
    /// The prose being written. Loaded from the current shot when you move to
    /// one, and kept while you edit it.
    pub draft: String,
    /// Which shot `draft` was loaded from, so moving to another one can offer
    /// its words instead of silently replacing what you are typing.
    pub draft_from: Option<usize>,

    // -- the set --
    pub setting: SettingChoice,
    pub cast: usize,
    pub horizon: f64,
    pub figure_scale: f64,
    pub lit: bool,
    pub clouds: bool,
    pub water: bool,
    pub frames: u32,

    // -- the scenery --
    pub scenery: SceneryChoice,
    /// A library symbol per part, by id. `0` means "use the brush".
    pub art: [u64; 4],

    /// What the last run reported, shown where the buttons are.
    pub report: Option<String>,
    /// Sentences the director could not read, verbatim.
    pub ignored: Vec<String>,

    pub show_set: bool,
    pub show_scenery: bool,
}

impl Default for StoryState {
    fn default() -> Self {
        Self {
            draft: String::new(),
            draft_from: None,
            setting: SettingChoice::Daylight,
            cast: 0,
            horizon: 0.62,
            figure_scale: 0.62,
            lit: true,
            clouds: false,
            water: false,
            frames: 48,
            scenery: SceneryChoice::Bare,
            art: [0; 4],
            report: None,
            ignored: Vec::new(),
            show_set: true,
            show_scenery: true,
        }
    }
}

impl StoryState {
    /// Put a shot's own words in the editor.
    pub fn load(&mut self, shot: usize, brief: &str) {
        self.draft = brief.to_string();
        self.draft_from = Some(shot);
        self.report = None;
        self.ignored.clear();
    }

    /// The symbol chosen for a part, or `None` for the brush.
    pub fn art_for(&self, part: SceneryPartChoice) -> Option<u64> {
        let id = self.art[part.index()];
        (id != 0).then_some(id)
    }
}

/// What the user asked for.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StoryResponse {
    /// Direct the draft into the shot that is open, replacing it.
    pub direct: bool,
    /// Direct the draft as a whole film: a scene per paragraph.
    pub direct_sequence: bool,
    /// Build the set from the settings, without directing anything.
    pub set_scene: bool,
    /// Lay the chosen scenery into the shot that is open.
    pub lay_scenery: bool,
    /// Go to this shot, and bring its words with it.
    pub go_to: Option<usize>,
}

/// Draw the Story panel.
///
/// `shots` is every scene in the film, in order; `current` is the one open.
/// `symbols` is the library, offered for the scenery parts.
pub fn story_panel(
    ui: &mut Ui,
    state: &mut StoryState,
    shots: &[ShotSummary],
    current: usize,
    symbols: &[(u64, String)],
) -> StoryResponse {
    let mut response = StoryResponse::default();

    ui.horizontal(|ui| {
        ui.heading("Story");
        ui.label(
            RichText::new(match shots.len() {
                1 => "1 shot".to_string(),
                n => format!("{n} shots"),
            })
            .small()
            .weak(),
        );
    });

    shot_list(ui, shots, current, &mut response);
    ui.add_space(6.0);
    the_words(ui, state, shots, current, &mut response);
    ui.add_space(6.0);
    the_set(ui, state, &mut response);
    ui.add_space(4.0);
    the_scenery(ui, state, symbols, &mut response);

    response
}

/// **The shot list, with the words that made each one.**
///
/// This is the half of the panel that did not exist anywhere before: the brief
/// was consumed and forgotten, so a film of six shots was six scene names and
/// no way to see what any of them was asked to be.
fn shot_list(ui: &mut Ui, shots: &[ShotSummary], current: usize, response: &mut StoryResponse) {
    egui::ScrollArea::vertical()
        .max_height(120.0)
        .id_salt("story-shots")
        .show(ui, |ui| {
            for (i, shot) in shots.iter().enumerate() {
                let open = i == current;
                let title = format!("{}. {}", i + 1, shot.name);
                let row = ui.selectable_label(open, RichText::new(title).strong());
                if row.clicked() && !open {
                    response.go_to = Some(i);
                }
                let line = shot.first_line();
                let detail = if line.is_empty() {
                    format!("{} frames, {} layers — built by hand", shot.frames, shot.layers)
                } else {
                    format!("\u{201c}{line}\u{201d}")
                };
                ui.label(RichText::new(detail).small().weak());
                ui.add_space(2.0);
            }
        });
}

fn the_words(
    ui: &mut Ui,
    state: &mut StoryState,
    shots: &[ShotSummary],
    current: usize,
    response: &mut StoryResponse,
) {
    // **Offer the shot's own words rather than taking them.** Replacing what
    // somebody is typing because they clicked another shot is the kind of thing
    // that loses a paragraph.
    let here = shots.get(current);
    let stale = state.draft_from != Some(current);
    if stale && here.is_some_and(|s| !s.brief.is_empty()) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("This shot was directed from its own brief.").small());
            if ui.small_button("Load it").clicked() {
                let brief = here.map(|s| s.brief.clone()).unwrap_or_default();
                state.load(current, &brief);
            }
        });
        ui.add_space(2.0);
    }

    ui.label(RichText::new("The brief").strong());
    ui.label(
        RichText::new(
            "A blank line starts a new shot. Say where it is, who is in it, \
             and what they do.",
        )
        .small()
        .weak(),
    );
    egui::ScrollArea::vertical()
        .max_height(150.0)
        .id_salt("story-draft")
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut state.draft)
                    .desired_width(f32::INFINITY)
                    .desired_rows(6)
                    .hint_text(
                        "Night. A city street.\n\
                         Ana walks in from the left.\n\
                         Ana talks to Ben for 3 seconds. Ben listens.",
                    ),
            );
        });

    let has_words = !state.draft.trim().is_empty();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(has_words, egui::Button::new("Direct this shot"))
            .on_hover_text("Stage, cast, block and frame the shot that is open, from these words")
            .clicked()
        {
            response.direct = true;
        }
        if ui
            .add_enabled(has_words, egui::Button::new("Direct the whole brief"))
            .on_hover_text("A scene per paragraph, played one after another")
            .clicked()
        {
            response.direct_sequence = true;
        }
    });

    if let Some(report) = &state.report {
        ui.add_space(2.0);
        ui.label(RichText::new(report).small());
    }
    for line in &state.ignored {
        ui.label(
            RichText::new(format!("could not read: \u{201c}{line}\u{201d}"))
                .small()
                .color(ui.visuals().warn_fg_color),
        );
    }
}

fn the_set(ui: &mut Ui, state: &mut StoryState, response: &mut StoryResponse) {
    let header = egui::CollapsingHeader::new(RichText::new("The set").strong())
        .default_open(state.show_set)
        .id_salt("story-set");
    header.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Where");
            egui::ComboBox::from_id_salt("story-setting")
                .selected_text(state.setting.label())
                .show_ui(ui, |ui| {
                    for choice in SettingChoice::ALL {
                        ui.selectable_value(&mut state.setting, choice, choice.label())
                            .on_hover_text(choice.description());
                    }
                });
        });

        slider(ui, "Horizon", &mut state.horizon, 0.2..=0.9, "story-horizon");
        slider(
            ui,
            "Figure size",
            &mut state.figure_scale,
            0.2..=0.95,
            "story-figure",
        );

        ui.horizontal(|ui| {
            ui.label("Cast");
            ui.add(egui::DragValue::new(&mut state.cast).range(0..=6));
            ui.label("Frames");
            ui.add(egui::DragValue::new(&mut state.frames).range(1..=16_000));
        });

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut state.lit, "Light it");
            ui.checkbox(&mut state.clouds, "Cloud");
            ui.checkbox(&mut state.water, "Water");
        });

        if ui
            .button("Set the scene")
            .on_hover_text("Ground, backdrop, a light rig, and the cast standing on it")
            .clicked()
        {
            response.set_scene = true;
        }
    });
}

fn the_scenery(
    ui: &mut Ui,
    state: &mut StoryState,
    symbols: &[(u64, String)],
    response: &mut StoryResponse,
) {
    let header = egui::CollapsingHeader::new(RichText::new("The scenery").strong())
        .default_open(state.show_scenery)
        .id_salt("story-scenery");
    header.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("What is in it");
            egui::ComboBox::from_id_salt("story-scenery-kind")
                .selected_text(state.scenery.label())
                .show_ui(ui, |ui| {
                    for choice in SceneryChoice::ALL {
                        ui.selectable_value(&mut state.scenery, choice, choice.label())
                            .on_hover_text(choice.description());
                    }
                });
        });
        ui.label(RichText::new(state.scenery.description()).small().weak());

        ui.add_space(4.0);
        ui.label(RichText::new("Drawn by").strong());
        ui.label(
            RichText::new(
                "A symbol from the library stands in for the brush. \
                 Whatever is left on Brush is generated.",
            )
            .small()
            .weak(),
        );

        for part in SceneryPartChoice::ALL {
            ui.horizontal(|ui| {
                ui.label(part.label());
                let index = part.index();
                let chosen = state.art[index];
                let name = symbols
                    .iter()
                    .find(|(id, _)| *id == chosen)
                    .map(|(_, n)| n.as_str())
                    .unwrap_or("Brush");
                egui::ComboBox::from_id_salt(("story-art", index))
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.art[index], 0, "Brush");
                        for (id, name) in symbols {
                            ui.selectable_value(&mut state.art[index], *id, name);
                        }
                    });
            })
            .response
            .on_hover_text(part.hint());
        }

        ui.add_space(4.0);
        let usable = state.scenery != SceneryChoice::Bare;
        if ui
            .add_enabled(usable, egui::Button::new("Lay the scenery"))
            .on_hover_text("Behind the cast where it belongs behind, in front where it belongs in front")
            .clicked()
        {
            response.lay_scenery = true;
        }
        if !usable {
            ui.label(
                RichText::new("Bare has nothing to lay.")
                    .small()
                    .weak(),
            );
        }
    });
}

/// A labelled slider that takes what is left of the row.
///
/// The same arrangement the Layer Depth panel uses, and for the same reason: a
/// label, egui's fixed 100-point slider and its number box come to more than a
/// dock column at its narrowest, and the number box is the half you can type
/// into.
fn slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    salt: &str,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.spacing_mut().slider_width = (ui.available_width() - 66.0).max(40.0);
        ui.push_id(salt, |ui| {
            ui.add(egui::Slider::new(value, range).fixed_decimals(2));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots() -> Vec<ShotSummary> {
        vec![
            ShotSummary {
                name: "1 - Forest".into(),
                brief: "A forest at dawn.\nAna walks in from the left.\n".into(),
                frames: 120,
                layers: 20,
            },
            ShotSummary {
                name: "2 - Village".into(),
                brief: String::new(),
                frames: 96,
                layers: 8,
            },
        ]
    }

    /// The list shows the words behind a shot, which is the whole reason the
    /// brief is kept at all.
    #[test]
    fn a_shot_shows_the_first_line_of_its_brief() {
        let shots = shots();
        assert_eq!(shots[0].first_line(), "A forest at dawn.");
        assert_eq!(shots[1].first_line(), "", "a hand-built shot has no brief");
    }

    /// Loading a shot's brief takes it, and remembers where it came from — so
    /// moving to another shot offers its words rather than taking over what is
    /// being typed.
    #[test]
    fn loading_a_brief_records_which_shot_it_came_from() {
        let mut state = StoryState::default();
        assert_eq!(state.draft_from, None);
        state.load(1, "Night. Ben waits.");
        assert_eq!(state.draft, "Night. Ben waits.");
        assert_eq!(state.draft_from, Some(1));
    }

    /// Zero is "use the brush", so a part nobody has chosen a drawing for reads
    /// as `None` rather than as symbol zero.
    #[test]
    fn an_unset_part_uses_the_brush() {
        let mut state = StoryState::default();
        assert_eq!(state.art_for(SceneryPartChoice::Trees), None);
        state.art[SceneryPartChoice::Trees.index()] = 42;
        assert_eq!(state.art_for(SceneryPartChoice::Trees), Some(42));
        assert_eq!(state.art_for(SceneryPartChoice::Grass), None);
    }
}
