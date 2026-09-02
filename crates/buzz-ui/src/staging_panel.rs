//! The Set the Scene and Animate Selection dialogs.
//!
//! Two dialogs rather than one, because they answer two different questions —
//! "what am I looking at?" and "what is this person doing?" — and because the
//! second one is used over and over on the same scene while the first is used
//! once.
//!
//! As everywhere else here, the state is separate from the drawing so that what
//! the dialog decides can be tested without a window, and the shell owns
//! everything that touches the document.

use egui::{RichText, Ui};

/// Where the scene is. A plain mirror of `buzz_act::Setting`, kept here so
/// `buzz-ui` does not depend on the staging crate — the same separation every
/// other panel has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingChoice {
    Daylight,
    Sunset,
    Night,
    Interior,
}

impl SettingChoice {
    pub const ALL: [SettingChoice; 4] = [
        SettingChoice::Daylight,
        SettingChoice::Sunset,
        SettingChoice::Night,
        SettingChoice::Interior,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingChoice::Daylight => "Daylight",
            SettingChoice::Sunset => "Sunset",
            SettingChoice::Night => "Night",
            SettingChoice::Interior => "Interior",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            SettingChoice::Daylight => "A high sun, a blue sky, and short hard shadows",
            SettingChoice::Sunset => "A low sun, a warm sky, and shadows running long",
            SettingChoice::Night => "A dark sky and one warm lamp doing the work",
            SettingChoice::Interior => "A wall, a floor, and a practical lamp",
        }
    }
}

/// What the figure is doing. Mirror of `buzz_act::Action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionChoice {
    Walk,
    Run,
    Talk,
    Idle,
}

impl ActionChoice {
    pub const ALL: [ActionChoice; 4] = [
        ActionChoice::Walk,
        ActionChoice::Run,
        ActionChoice::Talk,
        ActionChoice::Idle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ActionChoice::Walk => "Walk",
            ActionChoice::Run => "Run",
            ActionChoice::Talk => "Talk",
            ActionChoice::Idle => "Idle",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ActionChoice::Walk => "Legs and arms in opposition, the body rising twice a stride",
            ActionChoice::Run => "A longer stride, a deeper drop, and arms bent and driving",
            ActionChoice::Talk => "A weight shift, head movement on the stresses, hands coming up",
            ActionChoice::Idle => "Standing and breathing, so a held drawing is not a dead one",
        }
    }

    /// Does this one cross the stage?
    pub fn travels(self) -> bool {
        matches!(self, ActionChoice::Walk | ActionChoice::Run)
    }
}

/// Which dialog is open, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingDialog {
    Scene,
    Perform,
    /// A story typed as prose, staged and animated in one go.
    Direct,
    /// A curve was drawn; choose how the selected object travels along it.
    MotionPath,
    /// Add spring follow-through to a chain of the selected rig.
    Physics,
    /// Add a procedural wiggle to the selected object.
    Wiggle,
}

/// Everything the two dialogs remember.
///
/// View state: none of it is part of the artwork, none of it is saved with it,
/// and none of it is undone.
#[derive(Debug, Clone, PartialEq)]
pub struct StagingState {
    pub open: Option<StagingDialog>,

    // -- the scene --
    pub setting: SettingChoice,
    pub cast: usize,
    /// Where the ground meets the backdrop, as a fraction of the stage height.
    pub horizon: f64,
    /// How tall the nearest figure is, as a fraction of the stage height.
    pub figure_scale: f64,
    pub lit: bool,
    /// How long the shot is, in frames.
    pub frames: u32,

    // -- the performance --
    pub action: ActionChoice,
    pub from_frame: u32,
    pub to_frame: u32,
    /// Scales the angles, not the tempo.
    pub amount: f64,
    /// Scales the tempo.
    pub tempo: f64,
    /// How far it travels over the whole range, in document units.
    pub distance: f64,
    /// Frames between keys. Two is animating on twos.
    pub step: u32,

    // -- the motion path --
    /// Turn the object to face along the path as it travels.
    pub orient_to_path: bool,
    /// Timing along the path, as Animate's ease slider: negative eases in,
    /// positive eases out, zero is a constant rate.
    pub ease: f64,

    // -- the follow-through physics --
    /// How hard the chain is pulled back to the pose — how fast it catches up.
    pub spring_stiffness: f64,
    /// How much of the swing survives — lower is bouncier.
    pub spring_damping: f64,
    /// Which bone tops the sprung chain, as an index into [`Self::physics_bones`].
    pub physics_root: usize,
    /// Also react to the whole body's movement — turning with it, trailing its
    /// acceleration — not only to the keyed pose.
    pub physics_couple: bool,
    /// Stay live (attach a modifier, re-evaluated every frame) rather than bake
    /// keyframes. The always-in-sync default.
    pub physics_live: bool,
    /// The selected rig's bone names, filled when the dialog opens so the chain
    /// can be chosen by name.
    pub physics_bones: Vec<String>,

    // -- the wiggle --
    /// Peak jitter, in document units.
    pub wiggle_amplitude: f64,
    /// Roughly how many times a second it wanders through its range.
    pub wiggle_frequency: f64,
    /// Stay live rather than bake keyframes.
    pub wiggle_live: bool,

    // -- the story --
    /// The prose the director reads. Kept between openings: a story is
    /// something the user iterates on, and a box that forgot it would cost
    /// them the text every time they tweaked a sentence.
    pub story: String,

    /// Why the last attempt could not run, if it could not. Shown in the
    /// dialog rather than only in the status bar, because the dialog is where
    /// the user is looking and the reason is usually "nothing is selected".
    pub problem: Option<String>,
}

impl Default for StagingState {
    fn default() -> Self {
        Self {
            open: None,
            setting: SettingChoice::Sunset,
            cast: 2,
            horizon: 0.66,
            figure_scale: 0.62,
            lit: true,
            frames: 48,
            action: ActionChoice::Talk,
            from_frame: 0,
            to_frame: 47,
            amount: 1.0,
            tempo: 1.0,
            distance: 400.0,
            step: 2,
            orient_to_path: true,
            ease: 0.0,
            spring_stiffness: 120.0,
            spring_damping: 12.0,
            physics_root: 0,
            physics_couple: true,
            physics_live: true,
            physics_bones: Vec::new(),
            wiggle_amplitude: 12.0,
            wiggle_frequency: 1.5,
            wiggle_live: true,
            story: String::new(),
            problem: None,
        }
    }
}

impl StagingState {
    /// Open the scene dialog, with the shot's length filled in from the
    /// document rather than from whatever was typed last: a length that does
    /// not match the film is a silent way to build a scene that runs out.
    pub fn open_scene(&mut self, frame_count: u32) {
        self.open = Some(StagingDialog::Scene);
        self.frames = frame_count.max(1);
        self.problem = None;
    }

    /// Open the performance dialog over `frames`, which the shell fills from
    /// the playhead and the length of the film.
    pub fn open_perform(&mut self, from: u32, to: u32) {
        self.open = Some(StagingDialog::Perform);
        self.from_frame = from;
        self.to_frame = to.max(from);
        self.problem = None;
    }

    /// Open the story dialog.
    pub fn open_direct(&mut self) {
        self.open = Some(StagingDialog::Direct);
        self.problem = None;
    }

    /// Open the motion-path dialog over `frames`, filled from the playhead and
    /// the length of the film — the object should travel the drawn curve across
    /// the shot the animator is standing in.
    pub fn open_motion_path(&mut self, from: u32, to: u32) {
        self.open = Some(StagingDialog::MotionPath);
        self.from_frame = from;
        self.to_frame = to.max(from);
        self.problem = None;
    }

    /// Open the follow-through dialog over `frames`, with the selected rig's bone
    /// names so a chain can be chosen. Clamps the remembered chain root into the
    /// new rig's range.
    pub fn open_physics(&mut self, from: u32, to: u32, bones: Vec<String>) {
        self.open = Some(StagingDialog::Physics);
        self.from_frame = from;
        self.to_frame = to.max(from);
        self.physics_root = self.physics_root.min(bones.len().saturating_sub(1));
        self.physics_bones = bones;
        self.problem = None;
    }

    /// Open the wiggle dialog over `frames`.
    pub fn open_wiggle(&mut self, from: u32, to: u32) {
        self.open = Some(StagingDialog::Wiggle);
        self.from_frame = from;
        self.to_frame = to.max(from);
        self.problem = None;
    }

    pub fn close(&mut self) {
        self.open = None;
        self.problem = None;
    }

    /// The frame range, as a half-open range ready for the performance.
    ///
    /// Ordered, so a range typed backwards animates those frames rather than
    /// none at all — the same rule the export dialog follows.
    pub fn range(&self) -> std::ops::Range<u32> {
        let first = self.from_frame.min(self.to_frame);
        let last = self.from_frame.max(self.to_frame);
        first..last + 1
    }
}

/// What the user chose.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StagingResponse {
    /// Build the scene from the current settings.
    pub set_scene: bool,
    /// Write the performance onto the selection.
    pub perform: bool,
    /// Direct the typed story: stage it and animate it.
    pub direct: bool,
    /// Send the selected object along the drawn motion path.
    pub follow_path: bool,
    /// Bake follow-through onto the selected rig's chain.
    pub add_physics: bool,
    /// Bake a wiggle onto the selected object.
    pub add_wiggle: bool,
    pub cancelled: bool,
}

/// Draw whichever dialog is open.
///
/// `can_perform` is whether the current selection is something that could be
/// animated. The dialog still opens without one — an animator who opens it and
/// then remembers to click the character should not have to reopen it — but the
/// button is disabled and says why.
pub fn staging_dialog(
    ctx: &egui::Context,
    state: &mut StagingState,
    can_perform: bool,
) -> StagingResponse {
    let mut response = StagingResponse::default();
    let Some(which) = state.open else {
        return response;
    };

    let title = match which {
        StagingDialog::Scene => "Set the Scene",
        StagingDialog::Perform => "Animate Selection",
        StagingDialog::Direct => "Direct a Story",
        StagingDialog::MotionPath => "Send Along Path",
        StagingDialog::Physics => "Add Follow-Through",
        StagingDialog::Wiggle => "Add Wiggle",
    };

    let mut still_open = true;
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| match which {
            StagingDialog::Scene => scene_view(ui, state, &mut response),
            StagingDialog::Perform => perform_view(ui, state, can_perform, &mut response),
            StagingDialog::Direct => direct_view(ui, state, &mut response),
            StagingDialog::MotionPath => motion_view(ui, state, &mut response),
            StagingDialog::Physics => physics_view(ui, state, &mut response),
            StagingDialog::Wiggle => wiggle_view(ui, state, &mut response),
        });

    if !still_open {
        response.cancelled = true;
    }
    if response.cancelled {
        state.close();
    }
    response
}

fn scene_view(ui: &mut Ui, state: &mut StagingState, response: &mut StagingResponse) {
    ui.label(
        RichText::new(
            "A ground plane, a backdrop, a light rig and people standing on it. \
             Everything it makes is ordinary artwork on ordinary layers \u{2014} draw over it.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label("Setting");
        egui::ComboBox::from_id_salt("scene-setting")
            .selected_text(state.setting.label())
            .width(180.0)
            .show_ui(ui, |ui| {
                for choice in SettingChoice::ALL {
                    ui.selectable_value(&mut state.setting, choice, choice.label())
                        .on_hover_text(choice.description());
                }
            });
    });
    ui.label(RichText::new(state.setting.description()).small().weak());
    ui.add_space(4.0);

    egui::Grid::new("scene-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("People");
            ui.add(egui::Slider::new(&mut state.cast, 0..=6));
            ui.end_row();

            ui.label("Horizon");
            ui.add(
                egui::Slider::new(&mut state.horizon, 0.25..=0.9)
                    .fixed_decimals(2)
                    .custom_formatter(|v, _| format!("{:.0}% down", v * 100.0)),
            )
            .on_hover_text(
                "Where the ground meets the backdrop. Everything else follows it: where \
                 the cast stands, how big they are drawn, and how far the light hangs \
                 above the floor.",
            );
            ui.end_row();

            ui.label("Figure height");
            ui.add(
                egui::Slider::new(&mut state.figure_scale, 0.2..=1.0)
                    .fixed_decimals(2)
                    .custom_formatter(|v, _| format!("{:.0}% of the stage", v * 100.0)),
            );
            ui.end_row();

            ui.label("Frames");
            ui.add(egui::DragValue::new(&mut state.frames).range(1..=100_000))
                .on_hover_text("Every layer is made this long, so a performance has somewhere to go");
            ui.end_row();
        });

    ui.checkbox(&mut state.lit, "Light it").on_hover_text(
        "A key, a fill, and an edge glow around the cast. Without it the scene is flat \
         colour, and the cast sits on the background rather than in it.",
    );

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Set the Scene").clicked() {
            response.set_scene = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

fn direct_view(ui: &mut Ui, state: &mut StagingState, response: &mut StagingResponse) {
    ui.label(
        RichText::new(
            "Type what happens; the director stages it and animates it. It reads names, \
             the setting, and verbs like walk, run, talk and wait \u{2014} \u{201c}in from the \
             left\u{201d}, \u{201c}off right\u{201d}, \u{201c}to Ben\u{201d}, \u{201c}for 3 \
             seconds\u{201d}. Everything it makes is ordinary layers and keyframes: one \
             Ctrl+Z takes the whole scene back.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    let hint = "Night. Ana walks in from the left.\nAna talks to Ben for 4 seconds.\nBen walks off right.";
    ui.add(
        egui::TextEdit::multiline(&mut state.story)
            .hint_text(hint)
            .desired_width(360.0)
            .desired_rows(6),
    );

    if let Some(problem) = &state.problem {
        ui.add_space(4.0);
        ui.label(
            RichText::new(problem)
                .small()
                .color(egui::Color32::from_rgb(0xE0, 0x8C, 0x3C)),
        );
    }

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        let go = ui.add_enabled(
            !state.story.trim().is_empty(),
            egui::Button::new("Direct It"),
        );
        if go.clicked() {
            response.direct = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

fn perform_view(
    ui: &mut Ui,
    state: &mut StagingState,
    can_perform: bool,
    response: &mut StagingResponse,
) {
    ui.label(
        RichText::new(
            "Writes ordinary pose keyframes onto the selected rig. Nothing stays \
             generated \u{2014} edit, retime or delete any of them afterwards.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label("Doing");
        egui::ComboBox::from_id_salt("perform-action")
            .selected_text(state.action.label())
            .width(180.0)
            .show_ui(ui, |ui| {
                for choice in ActionChoice::ALL {
                    ui.selectable_value(&mut state.action, choice, choice.label())
                        .on_hover_text(choice.description());
                }
            });
    });
    ui.label(RichText::new(state.action.description()).small().weak());
    ui.add_space(4.0);

    egui::Grid::new("perform-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Frames");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut state.from_frame).prefix("from "));
                ui.add(egui::DragValue::new(&mut state.to_frame).prefix("to "));
            });
            ui.end_row();

            ui.label("Amount");
            ui.add(
                egui::Slider::new(&mut state.amount, 0.2..=2.0)
                    .fixed_decimals(2),
            )
            .on_hover_text("Scales the movement, not the timing. Half is listless, two is broad.");
            ui.end_row();

            ui.label("Tempo");
            ui.add(egui::Slider::new(&mut state.tempo, 0.25..=3.0).fixed_decimals(2))
                .on_hover_text("How many cycles fit in the range. Rounded to a whole number, so a walk never stops mid-stride.");
            ui.end_row();

            if state.action.travels() {
                ui.label("Travels");
                ui.add(
                    egui::Slider::new(&mut state.distance, -2000.0..=2000.0)
                        .suffix(" px")
                        .fixed_decimals(0),
                )
                .on_hover_text(
                    "How far it goes over the whole range, in the direction it faces. \
                     Negative walks backwards.",
                );
                ui.end_row();
            }

            ui.label("Keys every");
            ui.add(egui::Slider::new(&mut state.step, 1..=6).suffix(" frame(s)"))
                .on_hover_text(
                    "Two is animating on twos, which is what hand-drawn animation does. \
                     A tween fills the gaps, so the motion is smooth either way \u{2014} this \
                     decides how much of it you can grab.",
                );
            ui.end_row();
        });

    if let Some(problem) = &state.problem {
        ui.add_space(4.0);
        // Amber in both themes, like the accent: a warning that changed colour
        // with the theme would stop reading as one.
        ui.label(
            RichText::new(problem)
                .small()
                .color(egui::Color32::from_rgb(0xE0, 0x8C, 0x3C)),
        );
    }

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        let go = ui.add_enabled(can_perform, egui::Button::new("Animate"));
        if !can_perform {
            go.on_hover_text(
                "Select a rigged character on the stage first. Scene > Add Person makes \
                 one that is already rigged.",
            );
        } else if go.clicked() {
            response.perform = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

fn motion_view(ui: &mut Ui, state: &mut StagingState, response: &mut StagingResponse) {
    ui.label(
        RichText::new(
            "Bakes ordinary keyframes carrying the selected object along the curve \
             you drew. Nothing stays generated \u{2014} edit, retime or delete any of \
             them afterwards.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    egui::Grid::new("motion-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Frames");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut state.from_frame).prefix("from "));
                ui.add(egui::DragValue::new(&mut state.to_frame).prefix("to "));
            });
            ui.end_row();

            ui.label("Timing");
            ui.add(
                egui::Slider::new(&mut state.ease, -100.0..=100.0)
                    .fixed_decimals(0)
                    .custom_formatter(|v, _| {
                        if v < -0.5 {
                            format!("ease in {:.0}", -v)
                        } else if v > 0.5 {
                            format!("ease out {v:.0}")
                        } else {
                            "steady".to_string()
                        }
                    }),
            )
            .on_hover_text(
                "How the object accelerates along the path. Ease in starts slow, ease \
                 out arrives slow; steady is a constant rate.",
            );
            ui.end_row();

            ui.label("Facing");
            ui.checkbox(&mut state.orient_to_path, "Turn to follow the path")
                .on_hover_text(
                    "On, the object rotates to face the way it is heading \u{2014} a car or \
                     a fish. Off, it keeps its own orientation and only moves.",
                );
            ui.end_row();

            ui.label("Keys every");
            ui.add(egui::Slider::new(&mut state.step, 1..=6).suffix(" frame(s)"))
                .on_hover_text(
                    "Two is animating on twos. A tween fills the gaps, so the motion is \
                     smooth either way \u{2014} this decides how much of it you can grab.",
                );
            ui.end_row();
        });

    if let Some(problem) = &state.problem {
        ui.add_space(4.0);
        ui.label(
            RichText::new(problem)
                .small()
                .color(egui::Color32::from_rgb(0xE0, 0x8C, 0x3C)),
        );
    }

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Send").clicked() {
            response.follow_path = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

fn physics_view(ui: &mut Ui, state: &mut StagingState, response: &mut StagingResponse) {
    ui.label(
        RichText::new(
            "Bakes a springy lag onto a chain \u{2014} a ponytail, a tail \u{2014} so it follows \
             through the character's motion. Ordinary keyframes; re-run it if you change \
             the animation.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    egui::Grid::new("physics-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Chain from");
            if state.physics_bones.is_empty() {
                ui.label(RichText::new("no rig selected").weak());
            } else {
                let root = state.physics_root.min(state.physics_bones.len() - 1);
                egui::ComboBox::from_id_salt("physics-root")
                    .selected_text(&state.physics_bones[root])
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in state.physics_bones.iter().enumerate() {
                            ui.selectable_value(&mut state.physics_root, i, name);
                        }
                    });
            }
            ui.end_row();

            ui.label("Stiffness");
            ui.add(egui::Slider::new(&mut state.spring_stiffness, 20.0..=400.0).fixed_decimals(0))
                .on_hover_text("How hard the chain is pulled back to the pose. Higher catches up sooner.");
            ui.end_row();

            ui.label("Damping");
            ui.add(egui::Slider::new(&mut state.spring_damping, 2.0..=40.0).fixed_decimals(0))
                .on_hover_text("How quickly the swing dies. Lower is bouncier; higher barely overshoots.");
            ui.end_row();

            ui.label("Body");
            ui.checkbox(&mut state.physics_couple, "Follow the body's movement")
                .on_hover_text(
                    "Also swing as the whole character turns, and trail behind it when it \
                     speeds up \u{2014} hair streaming behind a runner.",
                );
            ui.end_row();

            ui.label("Delivery");
            ui.checkbox(&mut state.physics_live, "Stay live")
                .on_hover_text(
                    "Live: re-evaluated every frame, so re-timing the body updates the hair \
                     with nothing to re-bake. Off: bake keyframes now (frames and step below).",
                );
            ui.end_row();

            if !state.physics_live {
                ui.label("Frames");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut state.from_frame).prefix("from "));
                    ui.add(egui::DragValue::new(&mut state.to_frame).prefix("to "));
                });
                ui.end_row();

                ui.label("Keys every");
                ui.add(egui::Slider::new(&mut state.step, 1..=6).suffix(" frame(s)"))
                    .on_hover_text(
                        "Two is on twos. A tween fills the gaps, so the swing is smooth either way.",
                    );
                ui.end_row();
            }
        });

    if let Some(problem) = &state.problem {
        ui.add_space(4.0);
        ui.label(
            RichText::new(problem)
                .small()
                .color(egui::Color32::from_rgb(0xE0, 0x8C, 0x3C)),
        );
    }

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        let enabled = !state.physics_bones.is_empty();
        let go = ui.add_enabled(enabled, egui::Button::new("Add"));
        if !enabled {
            go.on_hover_text("Select a rigged character first.");
        } else if go.clicked() {
            response.add_physics = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

fn wiggle_view(ui: &mut Ui, state: &mut StagingState, response: &mut StagingResponse) {
    ui.label(
        RichText::new(
            "Bakes a small wandering motion onto the selected object \u{2014} an idle sway, \
             a breeze, a handheld shake. Deterministic, and on top of whatever motion it \
             already has.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    egui::Grid::new("wiggle-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Amount");
            ui.add(
                egui::Slider::new(&mut state.wiggle_amplitude, 1.0..=120.0)
                    .suffix(" px")
                    .fixed_decimals(0),
            )
            .on_hover_text("How far it strays from where the object would otherwise be.");
            ui.end_row();

            ui.label("Speed");
            ui.add(
                egui::Slider::new(&mut state.wiggle_frequency, 0.2..=8.0)
                    .suffix(" Hz")
                    .fixed_decimals(1),
            )
            .on_hover_text("How fast it wanders. A breath is slow; a shake is fast.");
            ui.end_row();

            ui.label("Delivery");
            ui.checkbox(&mut state.wiggle_live, "Stay live")
                .on_hover_text(
                    "Live: re-evaluated every frame, nothing baked. Off: bake keyframes now \
                     (frames and step below).",
                );
            ui.end_row();

            if !state.wiggle_live {
                ui.label("Frames");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut state.from_frame).prefix("from "));
                    ui.add(egui::DragValue::new(&mut state.to_frame).prefix("to "));
                });
                ui.end_row();

                ui.label("Keys every");
                ui.add(egui::Slider::new(&mut state.step, 1..=6).suffix(" frame(s)"))
                    .on_hover_text("On ones catches a fast shake; a slow sway is fine on twos.");
                ui.end_row();
            }
        });

    if let Some(problem) = &state.problem {
        ui.add_space(4.0);
        ui.label(
            RichText::new(problem)
                .small()
                .color(egui::Color32::from_rgb(0xE0, 0x8C, 0x3C)),
        );
    }

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Add").clicked() {
            response.add_wiggle = true;
        }
        if ui.button("Cancel").clicked() {
            response.cancelled = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A range typed backwards animates those frames rather than none, which is
    /// the same rule the export dialog follows and the same surprise avoided.
    #[test]
    fn a_backwards_range_is_still_a_range() {
        let state = StagingState {
            from_frame: 40,
            to_frame: 10,
            ..StagingState::default()
        };
        assert_eq!(state.range(), 10..41);
    }

    /// Opening the scene dialog takes the shot's length from the document, so a
    /// scene is never built shorter than the film it is for.
    #[test]
    fn opening_the_scene_dialog_reads_the_films_length() {
        let mut state = StagingState::default();
        state.open_scene(120);
        assert_eq!(state.open, Some(StagingDialog::Scene));
        assert_eq!(state.frames, 120);
    }

    /// A film of no length still gets a scene one frame long rather than none.
    #[test]
    fn an_empty_film_still_gets_a_frame() {
        let mut state = StagingState::default();
        state.open_scene(0);
        assert_eq!(state.frames, 1);
    }

    /// Only the actions that travel have somewhere to travel to.
    #[test]
    fn only_walking_and_running_travel() {
        assert!(ActionChoice::Walk.travels());
        assert!(ActionChoice::Run.travels());
        assert!(!ActionChoice::Talk.travels());
        assert!(!ActionChoice::Idle.travels());
    }

    /// Closing clears the problem, so a reason from last time does not greet
    /// the next attempt.
    #[test]
    fn closing_forgets_the_last_problem() {
        let mut state = StagingState {
            problem: Some("nothing selected".into()),
            ..StagingState::default()
        };
        state.close();
        assert!(state.open.is_none());
        assert!(state.problem.is_none());
    }
}
