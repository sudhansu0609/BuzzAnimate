//! Voice commands in the app (Stage V4 of `docs/AI_AGENT_PLAN.md`).
//!
//! `buzz_ai::voice::VoiceService` owns the microphone and the Whisper worker;
//! this is the shell around it: it applies the user's actions (mode picker, mic
//! button, mute, device), persists them in `ai.json`, mirrors the service's
//! state into the Assistant section every frame, and turns a recognised command
//! into text in the Assistant's request box. A command is **never run** from
//! here: the user reviews the text and presses Run.
//!
//! The microphone is opened only while the mode needs it (never when Off or
//! muted, never in tests) and is released the moment that stops being true, and
//! when this struct is dropped on quit.

use std::time::{Duration, Instant};

use buzz_ai::ai_config::{AiConfig, VoiceMode};
use buzz_ai::voice::{VoiceInput, VoiceOut, VoiceService};

use crate::app::App;

#[derive(Default)]
pub struct VoiceBridge {
    service: Option<VoiceService>,
    devices_at: Option<Instant>,
    model_at: Option<Instant>,
}

impl App {
    /// Creates the service on first use from `ai.json` (unit tests get the
    /// defaults: nothing read from disk, nothing opened).
    fn voice_ensure(&mut self) {
        if self.voice_bridge.service.is_some() {
            return;
        }
        let cfg = if cfg!(test) { AiConfig::default() } else { AiConfig::load() };
        let v = &mut self.ai_panel.assistant.voice;
        v.mode = cfg.effective_voice_mode();
        v.muted = cfg.voice_muted;
        v.device = cfg.voice_device.clone();
        v.model_ready = buzz_ai::transcribe::multi_ready();
        if !cfg!(test) {
            self.ai_panel.assistant.allow_all = cfg.allow_all;
            self.agent_bridge.allow_all.store(cfg.allow_all, std::sync::atomic::Ordering::Relaxed);
        }
        let mut svc = VoiceService::new(VoiceMode::Off, cfg.voice_muted, cfg.voice_device.clone());
        let mut out = Vec::new();
        svc.apply(VoiceInput::SetMode(cfg.effective_voice_mode()), &mut out);
        self.voice_bridge.service = Some(svc);
        self.voice_handle(out);
    }

    /// Every frame: collect what the microphone and Whisper produced, refresh
    /// the Assistant's voice display, keep repainting while the meter moves.
    pub(crate) fn voice_tick(&mut self, ctx: &egui::Context) {
        if cfg!(test) && self.voice_bridge.service.is_none() {
            return;
        }
        self.voice_ensure();
        let outs = self.voice_bridge.service.as_mut().map(|s| s.tick()).unwrap_or_default();
        self.voice_handle(outs);
        let now = Instant::now();
        if self.voice_bridge.model_at.is_none_or(|t| now.duration_since(t) > Duration::from_secs(2)) {
            self.voice_bridge.model_at = Some(now);
            self.ai_panel.assistant.voice.model_ready = buzz_ai::transcribe::multi_ready();
        }
        if self.agent_editor().workspace.is_open(buzz_ui::PanelId::Ai)
            && self.voice_bridge.devices_at.is_none_or(|t| now.duration_since(t) > Duration::from_secs(10))
        {
            self.voice_bridge.devices_at = Some(now);
            self.ai_panel.assistant.voice.devices = buzz_audio::mic::input_devices();
        }
        if let Some(s) = &self.voice_bridge.service {
            let v = &mut self.ai_panel.assistant.voice;
            v.mode = s.mode();
            v.muted = s.muted();
            v.listening = s.is_listening();
            v.opening = s.is_opening();
            v.recording = s.is_recording();
            v.level = s.level();
            if s.is_listening() || s.is_opening() || s.busy {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    /// Applies a voice input and handles what comes out.
    pub(crate) fn voice_apply(&mut self, input: VoiceInput) {
        self.voice_ensure();
        let mut out = Vec::new();
        if let Some(s) = self.voice_bridge.service.as_mut() {
            s.apply(input, &mut out);
        }
        self.voice_handle(out);
    }

    pub(crate) fn voice_handle(&mut self, outs: Vec<VoiceOut>) {
        for o in outs {
            match o {
                VoiceOut::Fill(text) => {
                    // For review: the box is filled, the panel comes forward,
                    // and the status line says what was heard. Nothing runs.
                    self.ai_panel.assistant.request = text.clone();
                    self.agent_editor_mut().workspace.reveal(buzz_ui::PanelId::Ai);
                    self.ai_panel.assistant.voice.status = Some(format!("Heard: {text}"));
                    // "Allow all": no review step, the same path as pressing
                    // Run (when a run is already going the box just waits).
                    if self.agent_allow_all() && !self.agent_active() {
                        self.agent_start(text.clone());
                        if self.agent_active() {
                            self.ai_panel.assistant.push(buzz_ui::ai_panel::StepKind::Note, format!("heard: {text}"));
                        }
                    }
                }
                VoiceOut::Status(s) => self.ai_panel.assistant.voice.status = Some(s),
                VoiceOut::Heard(t) => self.ai_panel.assistant.voice.last_heard = t,
            }
        }
    }

    /// Writes the voice settings into `ai.json` (re-read first, so the key and
    /// the other settings in it are left as they are). Not in unit tests.
    fn voice_save(&self) {
        if cfg!(test) {
            return;
        }
        let v = &self.ai_panel.assistant.voice;
        let mut cfg = AiConfig::load();
        cfg.set_voice_mode(v.mode);
        cfg.voice_muted = v.muted;
        cfg.voice_device = v.device.clone();
        cfg.allow_all = self.agent_allow_all();
        if let Err(e) = cfg.save() {
            tracing::warn!("could not save the voice settings: {e}");
        }
    }

    pub(crate) fn voice_set_mode(&mut self, mode: VoiceMode) {
        self.voice_apply(VoiceInput::SetMode(mode));
        self.ai_panel.assistant.voice.mode = mode;
        self.voice_save();
    }

    pub(crate) fn voice_toggle_mute(&mut self) {
        let muted = !self.ai_panel.assistant.voice.muted;
        self.voice_apply(VoiceInput::SetMuted(muted));
        self.ai_panel.assistant.voice.muted = muted;
        self.voice_save();
    }

    pub(crate) fn set_allow_all(&mut self, on: bool) {
        self.agent_bridge.allow_all.store(on, std::sync::atomic::Ordering::Relaxed);
        self.ai_panel.assistant.allow_all = on;
        self.voice_save();
    }

    pub(crate) fn voice_set_device(&mut self, device: Option<String>) {
        self.voice_ensure();
        if let Some(s) = self.voice_bridge.service.as_mut() {
            s.set_device(device.clone());
        }
        self.ai_panel.assistant.voice.device = device;
        self.voice_save();
    }
}

/// Test helpers for sibling modules.
#[cfg(test)]
pub(crate) mod testing {
    use super::VoiceOut;

    pub(crate) fn fill(text: &str) -> VoiceOut {
        VoiceOut::Fill(text.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_render::GpuPreference;

    /// No microphone is ever opened here: the mode stays Off or the mic is
    /// muted before anything could open it.
    #[test]
    fn a_heard_command_fills_the_request_box_and_never_runs() {
        let mut app = App::new(GpuPreference::default());
        app.voice_handle(vec![VoiceOut::Heard("hey buzz add a red ball".into()), VoiceOut::Fill("add a red ball".into())]);
        assert_eq!(app.ai_panel.assistant.request, "add a red ball");
        assert!(app.agent_editor().workspace.is_open(buzz_ui::PanelId::Ai), "the panel is revealed");
        assert!(!app.agent_active() && !app.ai_panel.assistant.running, "nothing runs until Run is pressed");
        let v = &app.ai_panel.assistant.voice;
        assert_eq!(v.status.as_deref(), Some("Heard: add a red ball"));
        assert_eq!(v.last_heard, "hey buzz add a red ball");
    }

    #[test]
    fn mute_wins_push_press_in_off_mode_explains_and_the_mic_stays_closed() {
        let mut app = App::new(GpuPreference::default());
        // Off: the press explains itself and nothing opens.
        app.dispatch(buzz_ui::Command::VoicePushToTalk);
        assert!(app.ai_panel.assistant.voice.status.as_deref().is_some_and(|s| s.contains("Push to talk")));
        assert!(!app.voice_bridge.service.as_ref().unwrap().is_listening());
        // Mute through the command (the hotkey's path), then pick Hey Buzz and press.
        // (Never unmute here: with the model installed that would open the real mic.)
        app.dispatch(buzz_ui::Command::VoiceMute);
        assert!(app.ai_panel.assistant.voice.muted);
        app.voice_set_mode(VoiceMode::Wake);
        app.dispatch(buzz_ui::Command::VoicePushToTalk);
        assert_eq!(app.ai_panel.assistant.voice.status.as_deref(), Some("Mic muted"));
        let svc = app.voice_bridge.service.as_ref().unwrap();
        assert!(!svc.is_listening() && !svc.is_opening(), "muted: the mic is never opened");
    }
}
