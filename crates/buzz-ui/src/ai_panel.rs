//! **The AI panel** (F7.0 of `docs/LOCAL_AI_PLAN.md`): every AI flow in one
//! docked panel, as collapsible sections in the order the plan gives —
//! Models, Character from Image, Auto-Rig selection, Scene from Image, Direct
//! Story, Identify & Rename, Transcribe, Activity.
//!
//! Like every other panel it only *reports*: each `section_*` draws from the
//! state it is given and returns an [`AiPanelAction`] for the shell (which
//! owns the document, the task registry and the model calls) to carry out.
//! The three old dialogs are thin wrappers over the same bodies
//! (`models_body`, `character_body`, `AiRenameState::body`), so the AI menu
//! still works exactly as before.

use egui::{Context, RichText, Ui};

use crate::ai_character_dialog::{AiCharacterDialogAction, AiCharacterState};
use crate::ai_models_dialog::AiModelsState;
use crate::ai_rename_dialog::{AiRenameAction, AiRenameState};
use crate::tasks_panel::TaskRow;
use crate::workspace::AiSettings;

/// One detected object of a Scene from Image run, as the panel shows it.
#[derive(Debug, Clone, Default)]
pub struct SceneObjectRow {
    pub label: String,
    pub kind: String,
    /// SigLIP shortlist: (candidate symbol name, cosine), best first.
    pub shortlist: Vec<(String, f64)>,
    /// Chosen shortlist position (the vision model's pick, editable), or
    /// `None` to leave the object out.
    pub chosen: Option<usize>,
}

/// A finished Scene from Image analysis waiting for **Build**.
#[derive(Debug, Clone, Default)]
pub struct SceneReview {
    pub image_name: String,
    pub rows: Vec<SceneObjectRow>,
    pub secs: f64,
    pub note: String,
}

/// A parsed story waiting for **Apply**.
#[derive(Debug, Clone, Default)]
pub struct StoryReview {
    pub beats: Vec<String>,
    pub cues: Vec<String>,
    pub cast: Vec<String>,
    pub secs: f64,
    pub model: String,
}

/// An Auto-Rig detection waiting for **Apply**: layer → slot.
#[derive(Debug, Clone, Default)]
pub struct RigReview {
    pub symbol: String,
    pub mapping: Vec<(String, String)>,
    pub secs: f64,
}

/// A transcription waiting for **Apply**; the lines are editable.
#[derive(Debug, Clone, Default)]
pub struct TranscriptReview {
    pub sound: String,
    pub lines: Vec<(f64, f64, String)>,
    pub secs: f64,
    pub provider: String,
}

/// What one line of the Assistant's step log is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// The model's short thought.
    Thought,
    /// A tool call the model made.
    Call,
    /// A call's result, ok.
    Ok,
    /// A call's result, an error (also a declined confirmation).
    Failed,
    /// The model's final reply.
    Reply,
    /// The app's own note (stopped, step cap, backend error).
    Note,
}

/// One line of the Assistant's step log.
#[derive(Debug, Clone)]
pub struct AssistantStep {
    pub kind: StepKind,
    pub text: String,
}

/// The Assistant section's state. The shell (`buzz-app/src/agent_bridge.rs`)
/// fills the log as a run streams in; the panel only draws it.
#[derive(Debug, Clone, Default)]
pub struct AssistantState {
    /// The request being typed.
    pub request: String,
    pub log: Vec<AssistantStep>,
    pub running: bool,
    /// "Claude (sonnet)" / "LM Studio: model" - what a run would use.
    pub backend_label: String,
    /// Shown while a long call runs ("running script...").
    pub busy_note: Option<String>,
    /// A call waiting for the user's Yes/No (what it would do).
    pub confirm: Option<String>,
    /// The model's final reply of the last run.
    pub reply: String,
    /// Studio-style guidance when no backend is available.
    pub guidance: Option<String>,
    /// Voice commands (Stage V): what the shell reports; the panel draws it.
    pub voice: VoiceUi,
    /// "Allow all" is on (persisted by the shell in `ai.json`).
    pub allow_all: bool,
}

/// The voice-command controls' display state. The shell
/// (`buzz-app/src/voice_bridge.rs`) refreshes it every frame from the
/// microphone service; the panel only draws it and reports clicks as actions.
#[derive(Debug, Clone, Default)]
pub struct VoiceUi {
    pub mode: buzz_ai::ai_config::VoiceMode,
    pub muted: bool,
    /// Input devices for the picker, and the chosen one (`None` = default).
    pub devices: Vec<String>,
    pub device: Option<String>,
    /// The microphone is open right now.
    pub listening: bool,
    /// Asked to open, not open yet.
    pub opening: bool,
    /// A push-to-talk recording is running.
    pub recording: bool,
    /// Input level `0.0..=1.0`.
    pub level: f32,
    /// The last thing Whisper heard.
    pub last_heard: String,
    /// A short message ("Mic muted", errors, "Heard: ...").
    pub status: Option<String>,
    /// The multilingual Whisper model is downloaded.
    pub model_ready: bool,
}

impl AssistantState {
    pub fn push(&mut self, kind: StepKind, text: impl Into<String>) {
        self.log.push(AssistantStep { kind, text: text.into() });
    }
}

/// The AI panel's own state (what is not in the three dialog states).
#[derive(Debug, Clone)]
pub struct AiPanelState {
    pub scene: Option<SceneReview>,
    pub story_brief: String,
    pub story: Option<StoryReview>,
    pub rig: Option<RigReview>,
    /// The library sound picked for Transcribe, by name.
    pub transcribe_sound: Option<String>,
    /// Who says the transcribed lines — the speaker whose mouth lip-syncs.
    pub transcribe_speaker: String,
    pub transcript: Option<TranscriptReview>,
    /// A one-line result per section, shown under its buttons.
    pub status: Option<String>,
    /// The Assistant section (Stage F of `docs/AI_AGENT_PLAN.md`).
    pub assistant: AssistantState,
    /// Draw every section unrolled (screenshots and headless tests).
    pub open_all: bool,
    /// Show only this section unrolled (index into [`SECTIONS`]) — for a
    /// screenshot of one flow.
    pub focus: Option<usize>,
}

impl Default for AiPanelState {
    fn default() -> Self {
        Self {
            scene: None,
            story_brief: String::new(),
            story: None,
            rig: None,
            transcribe_sound: None,
            transcribe_speaker: "Narrator".into(),
            transcript: None,
            status: None,
            assistant: AssistantState::default(),
            open_all: false,
            focus: None,
        }
    }
}

/// What the user asked the panel to do.
#[derive(Debug, Clone, PartialEq)]
pub enum AiPanelAction {
    /// The character section's **Create** produced a character.
    CharacterCreated,
    /// Detect the skeleton of the selected symbol and propose a mapping.
    AutoRigDetect,
    /// Commit the proposed rig.
    AutoRigApply,
    /// Pick a reference image and analyse it.
    ScenePickImage,
    /// Place the chosen symbols.
    SceneBuild,
    /// Parse the brief with the chat model.
    DirectStory(String),
    /// Execute the parsed story, with dialogue captions and lip sync.
    StoryApply,
    /// Start Identify & Rename on the library.
    IdentifyStart,
    /// Apply the checked renames.
    IdentifyApply(Vec<buzz_ai::naming::RenameProposal>),
    /// Transcribe this library sound.
    Transcribe(String),
    /// Put the transcript on a Captions layer and lip-sync the speaker.
    TranscriptApply,
    /// Stop a running AI task.
    Cancel(u64),
    /// Start the Assistant on this request.
    AssistantRun(String),
    /// Stop the Assistant's run between calls.
    AssistantStop,
    /// The user's answer to the Assistant's confirmation.
    AssistantConfirm(bool),
    /// Voice: the mic mode picker changed.
    VoiceMode(buzz_ai::ai_config::VoiceMode),
    /// Voice: the mute button (toggles).
    VoiceMute,
    /// Voice: the mic button (push to talk: start, or stop early).
    VoicePush,
    /// Voice: the input device picker (`None` = system default).
    VoiceDevice(Option<String>),
    /// The "Allow all" switch changed.
    AllowAll(bool),
}

/// Everything the panel draws from this frame.
pub struct AiPanelView<'a> {
    pub models: &'a mut AiModelsState,
    pub settings: &'a mut AiSettings,
    pub character: &'a mut AiCharacterState,
    pub rename: &'a mut AiRenameState,
    pub panel: &'a mut AiPanelState,
    /// Names of the sounds in the document's Sound library.
    pub sounds: &'a [String],
    /// Running AI tasks (the AI rows of the Tasks panel).
    pub activity: &'a [TaskRow],
    /// The symbol Auto-Rig would work on, if any is selected.
    pub rig_target: Option<String>,
}

/// The Assistant section's title; it is drawn first, above [`SECTIONS`].
pub const ASSISTANT: &str = "Assistant";

/// The section titles, in order. The headless test checks each is drawn.
pub const SECTIONS: [&str; 8] = [
    "Models",
    "Character from Image",
    "Auto-Rig selection",
    "Scene from Image",
    "Direct Story",
    "Identify & Rename",
    "Transcribe",
    "Activity",
];

/// A collapsible section like Properties': a strong title, open by default
/// for the first two.
fn section(ui: &mut Ui, title: &str, open: bool, force: Option<bool>, add: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt(("ai_panel", title))
        .default_open(open)
        .open(force)
        .show(ui, add);
}

/// The Assistant: a request box, Run / Stop, the live step log, inline
/// confirmation and the final reply.
pub fn section_assistant(ui: &mut Ui, state: &mut AssistantState) -> Option<AiPanelAction> {
    let mut action = None;
    ui.label(
        RichText::new("Describe what you want; the Assistant operates the app with the same commands, tools and panels you use. One Ctrl+Z undoes the whole run.")
            .small()
            .weak(),
    );
    ui.horizontal(|ui| {
        let mut on = state.allow_all;
        if ui
            .checkbox(&mut on, "Allow all")
            .on_hover_text("Voice commands run at once, confirmations are auto-approved and excluded commands are unlocked. Quit/close stay blocked.")
            .changed()
        {
            action = Some(AiPanelAction::AllowAll(on));
        }
        if state.allow_all {
            ui.label(allow_all_badge());
        }
    });
    if let Some(a) = voice_controls(ui, &mut state.voice, state.allow_all) {
        action = Some(a);
    }
    let running = state.running;
    let response = ui.add_enabled(
        !running,
        egui::TextEdit::multiline(&mut state.request)
            .hint_text("Put a red ball on a new layer and bounce it across 2 seconds")
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    let ctrl_enter = response.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        let can_run = !running && !state.request.trim().is_empty();
        if ui.add_enabled(can_run, egui::Button::new("Run")).on_hover_text("Ctrl+Enter").clicked()
            || (ctrl_enter && can_run)
        {
            action = Some(AiPanelAction::AssistantRun(state.request.trim().to_string()));
        }
        if ui.add_enabled(running, egui::Button::new("Stop")).clicked() {
            action = Some(AiPanelAction::AssistantStop);
        }
        if ui.add_enabled(!running, egui::Button::new("Clear")).clicked() {
            state.log.clear();
            state.reply.clear();
            state.guidance = None;
        }
        if running {
            ui.spinner();
        }
        if !state.backend_label.is_empty() {
            ui.label(RichText::new(&state.backend_label).small().weak());
        }
    });
    if let Some(g) = &state.guidance {
        ui.label(RichText::new(g).small().color(ui.visuals().warn_fg_color));
    }
    if let Some(note) = &state.busy_note {
        ui.label(RichText::new(note).small().italics());
    }
    if let Some(what) = &state.confirm {
        ui.group(|ui| {
            ui.label(RichText::new(format!("Allow this? {what}")).small().strong());
            ui.horizontal(|ui| {
                if ui.button("Yes").clicked() {
                    action = Some(AiPanelAction::AssistantConfirm(true));
                }
                if ui.button("No").clicked() {
                    action = Some(AiPanelAction::AssistantConfirm(false));
                }
            });
        });
    }
    egui::ScrollArea::vertical()
        .id_salt("assistant_log")
        .max_height(220.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for step in &state.log {
                let (tag, text) = match step.kind {
                    StepKind::Thought => ("thought", RichText::new(&step.text).small().italics()),
                    StepKind::Call => ("call", RichText::new(&step.text).small().monospace()),
                    StepKind::Ok => ("ok", RichText::new(&step.text).small()),
                    StepKind::Failed => ("error", RichText::new(&step.text).small().color(ui.visuals().error_fg_color)),
                    StepKind::Reply => ("reply", RichText::new(&step.text).small().strong()),
                    StepKind::Note => ("note", RichText::new(&step.text).small().weak()),
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tag).small().weak());
                    ui.label(text);
                });
            }
        });
    if !state.reply.trim().is_empty() {
        ui.separator();
        ui.label(RichText::new(&state.reply));
    }
    action
}

/// The microphone controls: mode picker, mic and mute buttons, device picker,
/// the red listening indicator with its level meter, and the last-heard line.
/// The amber "Allow all" badge shown in the Assistant and by the indicator.
fn allow_all_badge() -> RichText {
    RichText::new(" ALLOW ALL ").small().strong().background_color(egui::Color32::from_rgb(230, 160, 20)).color(egui::Color32::BLACK)
}

fn voice_controls(ui: &mut Ui, v: &mut VoiceUi, allow_all: bool) -> Option<AiPanelAction> {
    use buzz_ai::ai_config::VoiceMode;
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Voice").small().strong());
        let before = v.mode;
        let mut mode = v.mode;
        egui::ComboBox::from_id_salt("voice_mode")
            .selected_text(mode.label())
            .show_ui(ui, |ui| {
                for m in VoiceMode::ALL {
                    ui.selectable_value(&mut mode, m, m.label());
                }
            });
        if mode != before {
            action = Some(AiPanelAction::VoiceMode(mode));
        }
        // The mic button: only does something in Push to talk.
        let push_label = if v.recording { "Stop mic" } else { "Mic" };
        let push = ui
            .add_enabled(v.mode == VoiceMode::Push, egui::Button::new(push_label))
            .on_hover_text("Push to talk: press to record, again (or 3 s of silence) to stop. F12");
        if push.clicked() {
            action = Some(AiPanelAction::VoicePush);
        }
        // Mute: struck-through mic while muted; closes the mic in every mode.
        let mute_text = if v.muted {
            RichText::new("Mic").strikethrough().color(ui.visuals().error_fg_color)
        } else {
            RichText::new("Mute")
        };
        if ui
            .button(mute_text)
            .on_hover_text(if v.muted { "Muted: click to unmute (Shift+F12)" } else { "Mute the microphone (Shift+F12)" })
            .clicked()
        {
            action = Some(AiPanelAction::VoiceMute);
        }
    });
    if v.mode != VoiceMode::Off && !v.devices.is_empty() {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Microphone").small());
            let current = v.device.clone();
            let mut pick = current.clone();
            egui::ComboBox::from_id_salt("voice_device")
                .selected_text(current.clone().unwrap_or_else(|| "System default".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut pick, None, "System default");
                    for d in &v.devices {
                        ui.selectable_value(&mut pick, Some(d.clone()), d);
                    }
                });
            if pick != current {
                action = Some(AiPanelAction::VoiceDevice(pick));
            }
        });
    }
    // The indicator is shown whenever the mic is open, and says so when muted.
    if v.muted {
        ui.label(RichText::new("Mic muted").small().strong().color(ui.visuals().error_fg_color));
    } else if v.listening {
        ui.horizontal(|ui| {
            let word = if v.recording { "\u{25CF} Recording" } else { "\u{25CF} Listening" };
            ui.label(RichText::new(word).strong().color(egui::Color32::from_rgb(220, 40, 40)));
            ui.add(egui::ProgressBar::new(v.level.clamp(0.0, 1.0)).desired_width(80.0).desired_height(8.0));
            if allow_all {
                ui.label(allow_all_badge());
            }
        });
    } else if v.opening {
        ui.label(RichText::new("Opening the microphone\u{2026}").small().weak());
    }
    if v.mode != VoiceMode::Off && !v.model_ready {
        ui.label(
            RichText::new("Voice needs the \"Whisper small multilingual\" model: download it under Models below.")
                .small()
                .color(ui.visuals().warn_fg_color),
        );
    }
    if !v.last_heard.is_empty() {
        ui.label(RichText::new(format!("Heard: {}", v.last_heard)).small().italics());
    }
    if let Some(s) = &v.status {
        ui.label(RichText::new(s).small().weak());
    }
    action
}

fn elapsed_note(ui: &mut Ui, activity: &[TaskRow]) -> Option<u64> {
    let mut cancel = None;
    for row in activity {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(format!("{} \u{00b7} {:.0}s", row.label, row.elapsed_secs)).small());
            if row.can_cancel && ui.small_button("Cancel").clicked() {
                cancel = Some(row.id);
            }
        });
    }
    cancel
}

pub fn section_models(ui: &mut Ui, models: &mut AiModelsState, settings: &mut AiSettings) {
    crate::ai_models_dialog::models_body(ui, models, settings);
}

pub fn section_character(ui: &mut Ui, ctx: &Context, character: &mut AiCharacterState) -> Option<AiPanelAction> {
    character.poll(ctx);
    match crate::ai_character_dialog::character_body(ui, character, ctx) {
        // The kind stays on `character.rig_kind`, where the shell reads it.
        AiCharacterDialogAction::Created(assembled, _) => {
            character.assembled = Some(assembled);
            Some(AiPanelAction::CharacterCreated)
        }
        _ => None,
    }
}

pub fn section_auto_rig(ui: &mut Ui, panel: &mut AiPanelState, target: Option<&str>) -> Option<AiPanelAction> {
    let mut action = None;
    ui.label(
        RichText::new("Rigs a drawn character: renders it, finds its skeleton with DWPose and maps each layer to a bone.")
            .small()
            .weak(),
    );
    ui.horizontal(|ui| {
        let label = match target {
            Some(t) => format!("Auto-Rig '{t}'"),
            None => "Auto-Rig selection".to_string(),
        };
        let has_review = panel.rig.is_some();
        if ui
            .add_enabled(target.is_some(), egui::Button::new(if has_review { "Re-detect" } else { &label }))
            .on_disabled_hover_text("Select a character symbol in the Library or on stage")
            .clicked()
        {
            action = Some(AiPanelAction::AutoRigDetect);
        }
        if has_review && ui.button("Apply").clicked() {
            action = Some(AiPanelAction::AutoRigApply);
        }
    });
    if let Some(rig) = &panel.rig {
        ui.label(RichText::new(format!("{} \u{00b7} {:.1}s", rig.symbol, rig.secs)).small());
        egui::Grid::new("ai_rig_mapping").num_columns(2).striped(true).show(ui, |ui| {
            ui.label(RichText::new("Layer").small().strong());
            ui.label(RichText::new("Bone").small().strong());
            ui.end_row();
            for (layer, slot) in &rig.mapping {
                ui.label(RichText::new(layer).small());
                ui.label(RichText::new(slot).small());
                ui.end_row();
            }
        });
    }
    action
}

pub fn section_scene(ui: &mut Ui, panel: &mut AiPanelState) -> Option<AiPanelAction> {
    let mut action = None;
    ui.label(
        RichText::new("The vision model lists the objects; SigLIP shortlists the library's placeable symbols for each; the model confirms one.")
            .small()
            .weak(),
    );
    ui.horizontal(|ui| {
        if ui.button("Pick image\u{2026}").clicked() {
            action = Some(AiPanelAction::ScenePickImage);
        }
        let ready = panel.scene.as_ref().is_some_and(|s| s.rows.iter().any(|r| r.chosen.is_some()));
        if ui.add_enabled(ready, egui::Button::new("Build")).clicked() {
            action = Some(AiPanelAction::SceneBuild);
        }
    });
    if let Some(scene) = &mut panel.scene {
        ui.label(RichText::new(format!("{} \u{00b7} {:.1}s{}", scene.image_name, scene.secs, scene.note)).small());
        egui::Grid::new("ai_scene_rows").num_columns(2).striped(true).show(ui, |ui| {
            for (i, row) in scene.rows.iter_mut().enumerate() {
                ui.label(RichText::new(format!("{} ({})", row.label, row.kind)).small());
                let text = match row.chosen {
                    Some(c) => row
                        .shortlist
                        .get(c)
                        .map(|(n, v)| format!("{n}  {v:.2}"))
                        .unwrap_or_default(),
                    None => "\u{2014} leave out".into(),
                };
                egui::ComboBox::from_id_salt(("ai_scene_choice", i))
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(row.chosen.is_none(), "\u{2014} leave out").clicked() {
                            row.chosen = None;
                        }
                        for (k, (name, cos)) in row.shortlist.iter().enumerate() {
                            if ui
                                .selectable_label(row.chosen == Some(k), format!("{name}  {cos:.2}"))
                                .clicked()
                            {
                                row.chosen = Some(k);
                            }
                        }
                    });
                ui.end_row();
            }
        });
    }
    action
}

pub fn section_story(ui: &mut Ui, panel: &mut AiPanelState, settings: &AiSettings) -> Option<AiPanelAction> {
    let mut action = None;
    ui.add(
        egui::TextEdit::multiline(&mut panel.story_brief)
            .hint_text("Morning. Ramu enters from the left and says, \"The rain is coming.\"")
            .desired_rows(4)
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        let model = settings.model.clone().unwrap_or_else(|| "(no model)".into());
        if ui
            .add_enabled(!panel.story_brief.trim().is_empty(), egui::Button::new("Direct"))
            .clicked()
        {
            action = Some(AiPanelAction::DirectStory(panel.story_brief.clone()));
        }
        if panel.story.is_some() && ui.button("Apply").clicked() {
            action = Some(AiPanelAction::StoryApply);
        }
        ui.label(RichText::new(model).small().weak());
    });
    if let Some(story) = &panel.story {
        ui.label(
            RichText::new(format!(
                "{} beats, {} lines, cast {} \u{00b7} {} \u{00b7} {:.1}s",
                story.beats.len(),
                story.cues.len(),
                story.cast.join(", "),
                story.model,
                story.secs
            ))
            .small(),
        );
        for b in &story.beats {
            ui.label(RichText::new(format!("\u{2022} {b}")).small());
        }
        for c in &story.cues {
            ui.label(RichText::new(format!("\u{201c} {c}")).small().italics());
        }
    }
    action
}

pub fn section_identify(ui: &mut Ui, ctx: &Context, rename: &mut AiRenameState) -> Option<AiPanelAction> {
    rename.poll(ctx);
    let mut action = None;
    if !rename.is_running && ui.button("Identify library assets").clicked() {
        action = Some(AiPanelAction::IdentifyStart);
    }
    if let AiRenameAction::Apply(p) = rename.body(ui) {
        action = Some(AiPanelAction::IdentifyApply(p));
    }
    action
}

pub fn section_transcribe(ui: &mut Ui, panel: &mut AiPanelState, sounds: &[String]) -> Option<AiPanelAction> {
    let mut action = None;
    ui.label(RichText::new("Whisper base.en on this machine: speech to timed lines, then captions and lip sync.").small().weak());
    ui.horizontal(|ui| {
        ui.label("Sound");
        egui::ComboBox::from_id_salt("ai_transcribe_sound")
            .selected_text(panel.transcribe_sound.clone().unwrap_or_else(|| "(pick a library sound)".into()))
            .show_ui(ui, |ui| {
                for s in sounds {
                    if ui.selectable_label(panel.transcribe_sound.as_deref() == Some(s.as_str()), s).clicked() {
                        panel.transcribe_sound = Some(s.clone());
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Speaker");
        ui.add(egui::TextEdit::singleline(&mut panel.transcribe_speaker).desired_width(120.0));
        ui.label(RichText::new("English").small().weak());
    });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(panel.transcribe_sound.is_some(), egui::Button::new("Run"))
            .clicked()
        {
            action = panel.transcribe_sound.clone().map(AiPanelAction::Transcribe);
        }
        if panel.transcript.as_ref().is_some_and(|t| !t.lines.is_empty())
            && ui.button("Apply to captions & lip sync").clicked()
        {
            action = Some(AiPanelAction::TranscriptApply);
        }
    });
    if let Some(t) = &mut panel.transcript {
        ui.label(RichText::new(format!("{} \u{00b7} {:.1}s on {}", t.sound, t.secs, t.provider)).small());
        for (i, (a, b, text)) in t.lines.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{a:.2}\u{2013}{b:.2}")).small().monospace());
                ui.add(egui::TextEdit::singleline(text).id_salt(("ai_line", i)).desired_width(f32::INFINITY));
            });
        }
    }
    action
}

/// Draw the whole panel. Returns every action the user asked for this frame.
pub fn ai_panel(ui: &mut Ui, ctx: &Context, view: AiPanelView<'_>) -> Vec<AiPanelAction> {
    let mut actions = Vec::new();
    let AiPanelView { models, settings, character, rename, panel, sounds, activity, rig_target } = view;
    // Background jobs report through these polls whether or not their
    // section is unrolled — a rolled-up section's body is never run, and a
    // job only polled there would never finish.
    models.poll(settings);
    character.poll(ctx);
    rename.poll(ctx);
    let (open_all, focus) = (panel.open_all, panel.focus);
    let force = move |i: usize| match focus {
        Some(f) => Some(i == f),
        None => open_all.then_some(true),
    };
    egui::ScrollArea::vertical().id_salt("ai_panel_scroll").show(ui, |ui| {
        if let Some(s) = &panel.status {
            ui.label(RichText::new(s).small());
        }
        // Above the others; unrolled unless a screenshot focuses another section.
        let assistant_force = match focus {
            Some(_) => Some(false),
            None => open_all.then_some(true),
        };
        section(ui, ASSISTANT, true, assistant_force, |ui| {
            actions.extend(section_assistant(ui, &mut panel.assistant))
        });
        section(ui, SECTIONS[0], true, force(0), |ui| section_models(ui, models, settings));
        section(ui, SECTIONS[1], true, force(1), |ui| actions.extend(section_character(ui, ctx, character)));
        section(ui, SECTIONS[2], false, force(2), |ui| {
            actions.extend(section_auto_rig(ui, panel, rig_target.as_deref()))
        });
        section(ui, SECTIONS[3], false, force(3), |ui| actions.extend(section_scene(ui, panel)));
        section(ui, SECTIONS[4], false, force(4), |ui| actions.extend(section_story(ui, panel, settings)));
        section(ui, SECTIONS[5], false, force(5), |ui| actions.extend(section_identify(ui, ctx, rename)));
        section(ui, SECTIONS[6], false, force(6), |ui| actions.extend(section_transcribe(ui, panel, sounds)));
        section(ui, SECTIONS[7], true, force(7), |ui| {
            if activity.is_empty() {
                ui.label(RichText::new("No AI task running.").small().weak());
            }
            if let Some(id) = elapsed_note(ui, activity) {
                actions.push(AiPanelAction::Cancel(id));
            }
        });
    });
    if !activity.is_empty() || panel.assistant.running {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(panel: &mut AiPanelState, open_all: bool) -> (Vec<String>, Vec<AiPanelAction>) {
        let ctx = Context::default();
        let mut models = AiModelsState::default();
        // No network probing from a unit test.
        models.detected_backends = Some(Vec::new());
        let mut settings = AiSettings::default();
        let mut character = AiCharacterState::default();
        let mut rename = AiRenameState::default();
        let activity = vec![TaskRow {
            id: 7,
            kind: "AI".into(),
            label: "Scene from Image".into(),
            progress: None,
            detail: String::new(),
            elapsed_secs: 3.0,
            can_cancel: true,
        }];
        let sounds = vec!["dialogue.wav".to_string()];
        panel.open_all = open_all;
        let mut actions = Vec::new();
        let mut texts = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let ctx = ui.ctx().clone();
                actions = ai_panel(
                    ui,
                    &ctx,
                    AiPanelView {
                        models: &mut models,
                        settings: &mut settings,
                        character: &mut character,
                        rename: &mut rename,
                        panel,
                        sounds: &sounds,
                        activity: &activity,
                        rig_target: Some("Ramu copy 2".into()),
                    },
                );
            });
            texts = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        }
        (texts, actions)
    }

    #[test]
    fn ai_panel_renders_every_section_headlessly() {
        let mut panel = AiPanelState::default();
        panel.assistant.running = true;
        panel.assistant.backend_label = "Claude (sonnet)".into();
        panel.assistant.confirm = Some("save_as /tmp/x".into());
        panel.assistant.push(StepKind::Call, "command new_layer");
        panel.assistant.push(StepKind::Ok, "new_layer ok");
        panel.assistant.reply = "All done.".into();
        panel.scene = Some(SceneReview {
            image_name: "frame.png".into(),
            rows: vec![SceneObjectRow {
                label: "old man".into(),
                kind: "character".into(),
                shortlist: vec![("Symbol 4".into(), 0.74), ("Symbol 34".into(), 0.73)],
                chosen: Some(0),
            }],
            secs: 12.0,
            note: String::new(),
        });
        panel.transcript = Some(TranscriptReview {
            sound: "dialogue.wav".into(),
            lines: vec![(0.0, 3.28, "Ramu, bring the water.".into())],
            secs: 2.2,
            provider: "DirectML".into(),
        });
        let (texts, _) = run(&mut panel, true);
        let all = texts.join("\n");
        for t in SECTIONS {
            assert!(all.contains(t), "section {t} not drawn: {all}");
        }
        assert!(all.contains(ASSISTANT), "Assistant section not drawn");
        for want in ["Run", "Stop", "Clear", "Allow this? save_as /tmp/x", "Yes", "No", "new_layer ok", "All done.", "Claude (sonnet)"] {
            assert!(all.contains(want), "{want} missing from the Assistant section");
        }
        for want in ["Claude API server", "Test connection", "Symbol 4", "Ramu, bring the water.", "Scene from Image \u{00b7} 3s", "Build"] {
            assert!(all.contains(want), "{want} missing");
        }
        // The stored key never appears in the panel, only masked.
        assert!(!all.contains("sk-local-d"), "a key leaked into the UI");
    }

    #[test]
    fn assistant_voice_controls_render_picker_mic_mute_and_indicator() {
        use buzz_ai::ai_config::VoiceMode;
        // Hey Buzz, mic open: the red Listening indicator and the meter.
        let mut panel = AiPanelState::default();
        panel.assistant.voice = VoiceUi {
            mode: VoiceMode::Wake,
            devices: vec!["USB Mic".into()],
            listening: true,
            level: 0.4,
            last_heard: "add a red ball".into(),
            model_ready: true,
            ..Default::default()
        };
        let all = run(&mut panel, true).0.join("\n");
        for want in ["Voice", "Hey Buzz (always listening)", "Mic", "Mute", "Microphone", "\u{25CF} Listening", "Heard: add a red ball"] {
            assert!(all.contains(want), "{want} missing: {all}");
        }
        assert!(!all.contains("Whisper small multilingual"), "model is ready, no guidance expected");
        // Push to talk, recording.
        panel.assistant.voice.mode = VoiceMode::Push;
        panel.assistant.voice.recording = true;
        let all = run(&mut panel, true).0.join("\n");
        assert!(all.contains("Push to talk") && all.contains("Stop mic") && all.contains("\u{25CF} Recording"), "{all}");
        // Muted wins: the struck-through mic and the notice, no Listening indicator.
        panel.assistant.voice.muted = true;
        panel.assistant.voice.listening = false;
        panel.assistant.voice.recording = false;
        let all = run(&mut panel, true).0.join("\n");
        assert!(all.contains("Mic muted") && !all.contains("\u{25CF} Listening"), "{all}");
        // Off: the mic stays closed and nothing is shown about listening; a missing
        // model is only mentioned when voice is on.
        panel.assistant.voice = VoiceUi::default();
        let all = run(&mut panel, true).0.join("\n");
        assert!(all.contains("Off") && !all.contains("Whisper small multilingual"), "{all}");
        panel.assistant.voice.mode = VoiceMode::Wake;
        let all = run(&mut panel, true).0.join("\n");
        assert!(all.contains("Whisper small multilingual"), "{all}");
        // Allow all: the amber badge shows (header and beside the indicator).
        panel.assistant.allow_all = true;
        panel.assistant.voice = VoiceUi { mode: VoiceMode::Wake, listening: true, model_ready: true, ..Default::default() };
        let all = run(&mut panel, true).0.join("\n");
        assert_eq!(all.matches("ALLOW ALL").count(), 2, "{all}");
        assert!(all.contains("Allow all"));
    }
}

#[cfg(test)]
mod poll_tests {
    use super::*;

    /// A job in a rolled-up section still finishes: Identify & Rename is
    /// rolled up by default, and its result used to wait forever (F7.10 run).
    #[test]
    fn a_rolled_up_sections_job_is_still_polled() {
        let ctx = Context::default();
        let mut models = AiModelsState::default();
        models.detected_backends = Some(Vec::new());
        let mut settings = AiSettings::default();
        let mut character = AiCharacterState::default();
        let mut rename = AiRenameState::default();
        // An empty library ends the job at once with a status line.
        rename.start_for_scene(
            &buzz_scene::Scene::default(),
            buzz_ai::llm::Backend::default_openai(),
            "m".into(),
        );
        let mut panel = AiPanelState::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx().clone();
            ai_panel(
                ui,
                &ctx,
                AiPanelView {
                    models: &mut models,
                    settings: &mut settings,
                    character: &mut character,
                    rename: &mut rename,
                    panel: &mut panel,
                    sounds: &[],
                    activity: &[],
                    rig_target: None,
                },
            );
        });
        assert!(!rename.is_running);
    }
}
