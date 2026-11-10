//! **Window ▸ Voices** — make a voice from your own, give it to a character,
//! and put that character's lines on the timeline, by hand, one at a time.
//! No Producer needed. See `docs/VOICES_AND_ACTING_PLAN.md`.
//!
//! Four sections, top to bottom, in the order the work goes:
//!
//! 1. **Take** — record a line from the microphone (or import a file) and
//!    play it back as it was, or in the voice below.
//! 2. **Voice** — start from a preset (Man, Woman, Old Man, Girl …) and move
//!    the sliders: pitch, throat size, intonation, speed, breath, rasp,
//!    quaver, tone, volume, room. Save it under a name; it is in the library,
//!    for every film.
//! 3. **Characters** — every character on the stage, and the voice each
//!    speaks with.
//! 4. **Put on timeline** — the take, in the chosen character's voice, at the
//!    playhead, on that character's own `Voice:` layer, and that character's
//!    mouth — only theirs — lip-synced over exactly that line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use buzz_audio::Clip;
use buzz_audio::player::{Cue, CueSync};
use buzz_voice::{Emotion, Preset, VoiceProfile};
use egui::RichText;

use crate::voices::{self, StageCharacter};

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum VoicesAction {
    /// Open a file picker for a take.
    ImportTake,
    /// Put the take on the timeline for `character`, in `voice` (`None`: as
    /// recorded), said with `emotion`.
    PlaceLine {
        character: String,
        voice: Option<VoiceProfile>,
        emotion: Emotion,
        intensity: f32,
    },
}

pub struct VoicesPanel {
    /// The last thing that happened, shown at the bottom.
    pub status: String,
    /// The voice being designed.
    pub editing: VoiceProfile,
    /// The saved voice `editing` was loaded from or saved as.
    saved_as: Option<String>,
    /// Saved voices, re-read when the library changes or after a save.
    voices: Vec<VoiceProfile>,
    voices_of: Option<PathBuf>,
    /// Character name → voice name, for the editor.
    assigned: BTreeMap<String, String>,
    assigned_of: Option<PathBuf>,

    recorder: Option<buzz_audio::TakeRecorder>,
    /// The take: a recording or an imported file, mono.
    pub take: Option<Arc<Clip>>,
    /// The take in the voice it was last rendered in.
    voiced: Option<(VoiceProfile, Arc<Clip>)>,
    audition: Option<buzz_audio::Player>,
    /// Re-render and play straight away whenever a slider is let go.
    pub live: bool,

    /// Whose line it is.
    pub target: Option<String>,
    pub emotion: Emotion,
    pub intensity: f32,
}

impl Default for VoicesPanel {
    fn default() -> Self {
        Self {
            status: String::new(),
            editing: VoiceProfile { name: "New voice".into(), ..Preset::Natural.profile() },
            saved_as: None,
            voices: Vec::new(),
            voices_of: None,
            assigned: BTreeMap::new(),
            assigned_of: None,
            recorder: None,
            take: None,
            voiced: None,
            audition: None,
            live: false,
            target: None,
            emotion: Emotion::Neutral,
            intensity: 0.65,
        }
    }
}

/// "My own voice" in a voice picker: the recording as it is.
const OWN: &str = "(my own voice)";

impl VoicesPanel {
    /// Use `path` as the take.
    pub fn load_take(&mut self, path: &Path) {
        match Clip::open(path) {
            Ok(clip) => {
                let mono = Clip::new(&clip.name, clip.sample_rate, 1, clip.mono().to_vec());
                match mono {
                    Ok(c) => {
                        self.status = format!("Take: {} ({:.1} s)", c.name, c.duration_seconds());
                        self.take = Some(Arc::new(c));
                        self.voiced = None;
                    }
                    Err(e) => self.status = format!("Could not use {}: {e}", path.display()),
                }
            }
            Err(e) => self.status = format!("Could not open {}: {e}", path.display()),
        }
    }

    /// The voice a character speaks with in the editor, if one is set.
    pub fn voice_for(&self, root: Option<&Path>, character: &str) -> Option<VoiceProfile> {
        let name = self.assigned.get(character)?;
        root.and_then(|r| voices::load(r, name))
    }

    fn refresh(&mut self, root: Option<&Path>) {
        let root = root.map(Path::to_path_buf);
        if self.voices_of != root {
            self.voices = root.as_deref().map(voices::list).unwrap_or_default();
            self.voices_of = root.clone();
        }
        if self.assigned_of != root {
            self.assigned = root.as_deref().map(voices::load_editor_voices).unwrap_or_default();
            self.assigned_of = root;
        }
    }

    fn rescan(&mut self) {
        self.voices_of = None;
        self.assigned_of = None;
    }

    fn play(&mut self, clip: Arc<Clip>) {
        let player = self.audition.get_or_insert_with(|| buzz_audio::Player::new(100.0));
        player.set_cues(vec![Cue {
            clip,
            start_frame: 0,
            end_frame: None,
            trim_start: 0.0,
            trim_end: None,
            volume: 1.0,
            sync: CueSync::Stream,
        }]);
        if let Err(e) = player.play(0) {
            self.status = format!("Could not play: {e}");
        }
    }

    fn stop_playing(&mut self) {
        if let Some(p) = &mut self.audition {
            p.pause();
        }
    }

    /// The take in the voice being designed, rendered once per change.
    fn voiced_take(&mut self) -> Option<Arc<Clip>> {
        let take = self.take.clone()?;
        if let Some((profile, clip)) = &self.voiced
            && *profile == self.editing
        {
            return Some(clip.clone());
        }
        let started = std::time::Instant::now();
        let samples = buzz_voice::render(&take.mono(), take.sample_rate, &self.editing);
        let clip = Arc::new(Clip::new("voiced", take.sample_rate, 1, samples).ok()?);
        self.status = format!(
            "In \"{}\": {:.1} s, made in {} ms",
            self.editing.name,
            clip.duration_seconds(),
            started.elapsed().as_millis()
        );
        self.voiced = Some((self.editing.clone(), clip.clone()));
        Some(clip)
    }

    /// Draw the panel. `cast` is every character on stage, `selected` the one
    /// the selection is part of, `playhead` the 0-based frame a line goes on.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        root: Option<&Path>,
        cast: &[StageCharacter],
        selected: Option<&StageCharacter>,
        playhead: u32,
    ) -> Option<VoicesAction> {
        self.refresh(root);
        let mut action = None;

        egui::CollapsingHeader::new(RichText::new("1. Take").strong())
            .default_open(true)
            .show(ui, |ui| self.take_section(ui, &mut action));
        egui::CollapsingHeader::new(RichText::new("2. Voice").strong())
            .default_open(true)
            .show(ui, |ui| self.voice_section(ui, root));
        egui::CollapsingHeader::new(RichText::new("3. Characters").strong())
            .default_open(true)
            .show(ui, |ui| self.characters_section(ui, root, cast));
        egui::CollapsingHeader::new(RichText::new("4. Put on timeline").strong())
            .default_open(true)
            .show(ui, |ui| self.place_section(ui, root, cast, selected, playhead, &mut action));

        if !self.status.is_empty() {
            ui.separator();
            ui.label(RichText::new(&self.status).small().weak());
        }
        action
    }

    fn take_section(&mut self, ui: &mut egui::Ui, action: &mut Option<VoicesAction>) {
        if let Some(rec) = &self.recorder {
            let (secs, level, device, error) = (rec.seconds(), rec.level(), rec.device(), rec.error());
            ui.horizontal(|ui| {
                if ui.button("\u{25a0} Stop").clicked() {
                    let rec = self.recorder.take().expect("recording");
                    match rec.stop("Take") {
                        Some(clip) => {
                            self.status = format!("Recorded {:.1} s", clip.duration_seconds());
                            self.take = Some(Arc::new(clip));
                            self.voiced = None;
                        }
                        None => self.status = "Nothing was heard \u{2014} check the microphone".into(),
                    }
                }
                ui.label(format!("Recording {secs:.1} s"));
            });
            ui.add(egui::ProgressBar::new(level).desired_height(6.0));
            if let Some(d) = device {
                ui.label(RichText::new(d).small().weak());
            }
            if let Some(e) = error {
                ui.colored_label(egui::Color32::from_rgb(0xe0, 0x60, 0x50), e);
            }
            ui.ctx().request_repaint();
            return;
        }
        ui.horizontal(|ui| {
            if ui.button("\u{25cf} Record").on_hover_text("Record a line from the microphone").clicked() {
                self.stop_playing();
                self.recorder = Some(buzz_audio::TakeRecorder::start(None));
                self.status = "Recording \u{2014} say the line, then Stop".into();
            }
            if ui.button("Import\u{2026}").on_hover_text("Use a WAV, MP3 or other sound file as the take").clicked() {
                *action = Some(VoicesAction::ImportTake);
            }
        });
        let Some(take) = self.take.clone() else {
            ui.label(RichText::new("Record or import a line to hear it in a voice.").weak());
            return;
        };
        ui.label(format!("Take: {:.1} s", take.duration_seconds()));
        ui.horizontal(|ui| {
            if ui.button("\u{25b6} Original").clicked() {
                self.play(take.clone());
            }
            if ui.button("\u{25b6} In this voice").clicked()
                && let Some(clip) = self.voiced_take()
            {
                self.play(clip);
            }
            if ui.button("\u{25a0}").on_hover_text("Stop playing").clicked() {
                self.stop_playing();
            }
        });
        ui.checkbox(&mut self.live, "Play again when a slider is let go");
    }

    fn voice_section(&mut self, ui: &mut egui::Ui, root: Option<&Path>) {
        ui.horizontal(|ui| {
            let current = self.saved_as.clone().unwrap_or_else(|| "(unsaved)".into());
            egui::ComboBox::from_id_salt("voices-saved")
                .selected_text(current)
                .width(130.0)
                .show_ui(ui, |ui| {
                    for v in self.voices.clone() {
                        if ui.selectable_label(self.saved_as.as_deref() == Some(v.name.as_str()), &v.name).clicked() {
                            self.saved_as = Some(v.name.clone());
                            self.editing = v;
                        }
                    }
                    if self.voices.is_empty() {
                        ui.label(RichText::new("No saved voices yet").weak());
                    }
                });
            egui::ComboBox::from_id_salt("voices-preset")
                .selected_text("New from\u{2026}")
                .width(110.0)
                .show_ui(ui, |ui| {
                    for preset in Preset::ALL {
                        if ui.selectable_label(false, preset.label()).clicked() {
                            self.editing = VoiceProfile { name: format!("My {}", preset.label().to_lowercase()), ..preset.profile() };
                            self.saved_as = None;
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut self.editing.name).desired_width(140.0));
        });
        ui.horizontal(|ui| {
            let can = root.is_some() && !self.editing.name.trim().is_empty();
            if ui
                .add_enabled(can, egui::Button::new("Save"))
                .on_disabled_hover_text("Give it a name, and open an asset library")
                .clicked()
                && let Some(root) = root
            {
                match voices::save(root, &self.editing) {
                    Ok(_) => {
                        self.status = format!("Saved voice \"{}\"", self.editing.name);
                        self.saved_as = Some(self.editing.name.clone());
                        self.rescan();
                    }
                    Err(e) => self.status = format!("Could not save: {e}"),
                }
            }
            if ui.add_enabled(root.is_some() && self.saved_as.is_some(), egui::Button::new("Delete")).clicked()
                && let (Some(root), Some(name)) = (root, self.saved_as.clone())
            {
                match voices::delete(root, &name) {
                    Ok(()) => {
                        self.status = format!("Deleted voice \"{name}\"");
                        self.saved_as = None;
                        self.rescan();
                    }
                    Err(e) => self.status = format!("{e}"),
                }
            }
            if ui.button("Reset").on_hover_text("Back to your own voice, untouched").clicked() {
                let name = self.editing.name.clone();
                self.editing = VoiceProfile { name, ..Preset::Natural.profile() };
            }
        });

        let before = self.editing.clone();
        let mut released = false;
        let p = &mut self.editing;
        let mut slider = |ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str, tip: &str| {
            let r = ui.add(egui::Slider::new(value, range).text(label)).on_hover_text(tip);
            released |= r.drag_stopped() || (r.changed() && !r.dragged());
        };
        ui.label(RichText::new("Pitch and size").small().strong());
        slider(ui, &mut p.pitch, -12.0..=12.0, "Pitch (semitones)", "How high the voice is. +7 to +12 for a woman or child from a man's voice; −4 and below for a big man");
        slider(ui, &mut p.formant, 0.7..=1.45, "Throat size", "Above 1: a smaller throat (woman, child, little creature). Below 1: bigger (large man, giant). This is what keeps a raised voice from sounding like a sped-up tape");
        slider(ui, &mut p.intonation, 0.0..=2.0, "Intonation", "How far the pitch swings: 0 is a monotone robot, 1 is as you said it, more is sing-song");
        slider(ui, &mut p.tempo, 0.6..=1.6, "Speed", "Speaking rate, without changing pitch");
        ui.label(RichText::new("Texture").small().strong());
        slider(ui, &mut p.breathiness, 0.0..=1.0, "Breath", "Air in the voice: soft, tired, a whisper");
        slider(ui, &mut p.roughness, 0.0..=1.0, "Rasp / growl", "Gravel and growl: old, angry, a monster");
        slider(ui, &mut p.vibrato, 0.0..=1.5, "Quaver", "Pitch wobble, in semitones: age, fear");
        slider(ui, &mut p.vibrato_rate, 3.0..=9.0, "Quaver speed (Hz)", "How fast the quaver is");
        slider(ui, &mut p.tremolo, 0.0..=0.8, "Tremble", "The loudness wobbling");
        ui.label(RichText::new("Tone").small().strong());
        slider(ui, &mut p.warmth, -12.0..=12.0, "Body (dB)", "Chest and warmth at the low end");
        slider(ui, &mut p.brightness, -12.0..=12.0, "Brightness (dB)", "Presence and edge at the top");
        slider(ui, &mut p.nasal, -12.0..=12.0, "Nasal (dB)", "Through-the-nose colour");
        slider(ui, &mut p.telephone, 0.0..=1.0, "Phone / radio", "The voice through a small speaker");
        ui.label(RichText::new("Level and space").small().strong());
        slider(ui, &mut p.gain, -24.0..=12.0, "Volume (dB)", "Louder or quieter");
        slider(ui, &mut p.compression, 0.0..=1.0, "Even out", "Brings the loud and the quiet closer together");
        slider(ui, &mut p.room, 0.0..=1.0, "Room", "Reverb: a small room up to a hall");
        ui.checkbox(&mut p.normalize, "Same loudness for every line");
        slider(ui, &mut p.emotion_push, 0.0..=1.0, "Emotion push", "How much a line's emotion (angry, scared, happy\u{2026}) pushes this voice further. 0 keeps your performance exactly as acted");

        if self.live && released && self.editing != before && self.take.is_some() {
            if let Some(clip) = self.voiced_take() {
                self.play(clip);
            }
        }
    }

    fn characters_section(&mut self, ui: &mut egui::Ui, root: Option<&Path>, cast: &[StageCharacter]) {
        if cast.is_empty() {
            ui.label(RichText::new("No characters on stage at this frame.").weak());
            return;
        }
        let names: Vec<String> = self.voices.iter().map(|v| v.name.clone()).collect();
        let mut changed = false;
        egui::Grid::new("voices-cast").num_columns(3).striped(true).show(ui, |ui| {
            for who in cast {
                let label = if who.mouth.is_some() { who.name.clone() } else { format!("{} (no mouth)", who.name) };
                if ui
                    .selectable_label(self.target.as_deref() == Some(who.name.as_str()), label)
                    .on_hover_text("Lines go to this character")
                    .clicked()
                {
                    self.target = Some(who.name.clone());
                }
                let current = self.assigned.get(&who.name).cloned();
                egui::ComboBox::from_id_salt(("voices-of", &who.name))
                    .selected_text(current.clone().unwrap_or_else(|| OWN.into()))
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), OWN).clicked() {
                            self.assigned.remove(&who.name);
                            changed = true;
                        }
                        for n in &names {
                            if ui.selectable_label(current.as_deref() == Some(n.as_str()), n).clicked() {
                                self.assigned.insert(who.name.clone(), n.clone());
                                changed = true;
                            }
                        }
                    });
                if ui
                    .add_enabled(current.is_some(), egui::Button::new("Edit").small())
                    .on_hover_text("Load this character's voice into the sliders")
                    .clicked()
                    && let (Some(root), Some(name)) = (root, current)
                    && let Some(v) = voices::load(root, &name)
                {
                    self.editing = v;
                    self.saved_as = Some(name);
                }
                ui.end_row();
            }
        });
        if changed && let Some(root) = root {
            if let Err(e) = voices::save_editor_voices(root, &self.assigned) {
                self.status = format!("Could not save the voice list: {e}");
            }
        }
        if root.is_none() {
            ui.label(RichText::new("Open an asset library to keep voices.").weak());
        }
    }

    fn place_section(
        &mut self,
        ui: &mut egui::Ui,
        root: Option<&Path>,
        cast: &[StageCharacter],
        selected: Option<&StageCharacter>,
        playhead: u32,
        action: &mut Option<VoicesAction>,
    ) {
        // The selection on stage decides whose line it is, as soon as it
        // changes; a click in the list above decides otherwise.
        if let Some(sel) = selected
            && self.target.as_deref() != Some(sel.name.as_str())
            && !cast.iter().any(|c| Some(c.name.as_str()) == self.target.as_deref())
        {
            self.target = Some(sel.name.clone());
        }
        ui.horizontal(|ui| {
            ui.label("Who");
            egui::ComboBox::from_id_salt("voices-target")
                .selected_text(self.target.clone().unwrap_or_else(|| "choose\u{2026}".into()))
                .width(130.0)
                .show_ui(ui, |ui| {
                    for who in cast {
                        if ui.selectable_label(self.target.as_deref() == Some(who.name.as_str()), &who.name).clicked() {
                            self.target = Some(who.name.clone());
                        }
                    }
                });
            if let Some(sel) = selected
                && ui.small_button("Selected").on_hover_text(format!("Use {}, selected on stage", sel.name)).clicked()
            {
                self.target = Some(sel.name.clone());
            }
        });
        ui.horizontal(|ui| {
            ui.label("Said");
            egui::ComboBox::from_id_salt("voices-emotion")
                .selected_text(self.emotion.label())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for e in Emotion::ALL {
                        ui.selectable_value(&mut self.emotion, e, e.label());
                    }
                });
            ui.add(egui::Slider::new(&mut self.intensity, 0.0..=1.0).text("how much"));
        });

        let who = self.target.as_ref().and_then(|t| cast.iter().find(|c| &c.name == t));
        let voice = who.and_then(|w| self.voice_for(root, &w.name));
        if let Some(w) = who {
            let voice_name = voice.as_ref().map(|v| v.name.clone()).unwrap_or_else(|| OWN.into());
            ui.label(
                RichText::new(format!(
                    "At frame {} in {voice_name}. Only {}'s mouth moves, over this line.",
                    playhead + 1,
                    w.name
                ))
                .small(),
            );
            if w.mouth.is_none() {
                ui.label(RichText::new(format!("{} has no lip-sync mouth: the sound goes on, the mouth stays still.", w.name)).small().weak());
            }
        }
        let ready = self.take.is_some() && who.is_some() && self.recorder.is_none();
        if ui
            .add_enabled(ready, egui::Button::new("Put line on timeline"))
            .on_disabled_hover_text("Record or import a take, and choose who says it")
            .clicked()
            && let Some(w) = who
        {
            *action = Some(VoicesAction::PlaceLine {
                character: w.name.clone(),
                voice,
                emotion: self.emotion,
                intensity: self.intensity,
            });
        }
    }
}
