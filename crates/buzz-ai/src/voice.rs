//! Voice commands (Stage V): the "Hey Buzz" wake matcher, the pure mode/mute
//! state machine, and the service that runs the microphone and the multilingual
//! Whisper off the UI thread.
//!
//! Everything here is local: audio goes microphone → VAD → Whisper on this
//! PC and is dropped; nothing is written to disk or sent anywhere. A command
//! only ever *fills the Assistant's request box* — the user presses Run.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

pub use crate::ai_config::VoiceMode;
use buzz_audio::mic::{MicEvent, MicInput};

/// A wake phrase with nothing after it waits this long for the command.
pub const COMMAND_WINDOW_S: f64 = 6.0;

// ------------------------------------------------------------ wake matcher

/// What an utterance's transcript says about the wake word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeMatch {
    /// No wake phrase.
    None,
    /// The phrase and nothing else: the next utterance is the command.
    WakeOnly,
    /// The phrase followed by the command (the text after it).
    Command(String),
}

/// Wake spellings, normalised (lowercase, no diacritics/nukta, no
/// punctuation). Whisper writes the same sound many ways, in either script.
const WAKE_VARIANTS: &[&str] = &[
    // Latin
    "hey buzz", "hey buz", "hey bus", "hey buss", "hey bazz", "hey baz", "hay buzz", "hay bus",
    "hi buzz", "he buzz", "hai buzz", "a buzz", "ay buzz", "hey bhuz", "hey bass",
    // Devanagari (the nukta in बज़ is stripped by normalisation)
    "हे बज", "हे बस", "हे बज्ज", "हेय बज", "हेय बस", "है बज", "हे बझ", "ए बज", "हे बज्", "हे बॉज",
    "हे बज्जु", "हाय बज", "हे बझ्ज",
];

fn is_devanagari(c: char) -> bool {
    ('\u{0900}'..='\u{097F}').contains(&c)
}

/// Lowercase, strip diacritics (Latin accents, the Devanagari nukta and other
/// combining marks) and punctuation from one token.
pub fn normalise_token(tok: &str) -> String {
    let mut out = String::new();
    for c in tok.chars().flat_map(|c| c.to_lowercase()) {
        let c = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        };
        match c {
            // combining marks, nukta, danda
            '\u{0300}'..='\u{036F}' | '\u{093C}' | '\u{0964}' | '\u{0965}' => {}
            c if is_devanagari(c) => out.push(c),
            c if c.is_alphanumeric() => out.push(c),
            _ => {}
        }
    }
    out
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Best distance of `cand` to any wake variant, with the tolerance for it.
fn wake_distance(cand: &str) -> Option<usize> {
    let c: Vec<char> = cand.chars().collect();
    let squashed: Vec<char> = cand.chars().filter(|c| *c != ' ').collect();
    let mut best: Option<usize> = None;
    for v in WAKE_VARIANTS {
        let vc: Vec<char> = v.chars().collect();
        let vs: Vec<char> = v.chars().filter(|c| *c != ' ').collect();
        // Allow 2 edits, but never more than a third of a short phrase.
        let tol = 2.min(vs.len() / 3);
        let d = edit_distance(&c, &vc).min(edit_distance(&squashed, &vs));
        if d <= tol && best.map_or(true, |b| d < b) {
            best = Some(d);
        }
    }
    best
}

/// Looks for "hey buzz" at the start of `text` (first ~3 words, Latin or
/// Devanagari, fuzzy) and returns what follows.
pub fn match_wake(text: &str) -> WakeMatch {
    // Original tokens, each with its normalised form; pure punctuation drops out.
    let toks: Vec<(&str, String)> = text
        .split_whitespace()
        .map(|t| (t, normalise_token(t)))
        .filter(|(_, n)| !n.is_empty())
        .collect();
    // (distance, prefix words); smallest distance wins, then the shorter prefix.
    let mut best: Option<(usize, usize)> = None;
    for k in 1..=3.min(toks.len()) {
        let cand = toks[..k].iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(" ");
        if let Some(d) = wake_distance(&cand) {
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, k));
            }
        }
    }
    let Some((_, k)) = best else { return WakeMatch::None };
    let rest: Vec<&str> = toks[k..].iter().map(|(t, _)| *t).collect();
    let rest = rest.join(" ");
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || ",.:;!?-—\"'।".contains(c)).trim_end();
    if normalise_token(rest).is_empty() {
        WakeMatch::WakeOnly
    } else {
        WakeMatch::Command(rest.to_string())
    }
}

// --------------------------------------------------------- state machine

/// Everything the mode/mute logic knows.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceCtl {
    pub mode: VoiceMode,
    pub muted: bool,
    /// The mic is (or is being) open.
    pub mic_open: bool,
    /// A push recording is in progress.
    pub push_recording: bool,
    /// After a bare "Hey Buzz": the time until which the next utterance is
    /// the command.
    pub awaiting_until: Option<f64>,
}

impl VoiceCtl {
    pub fn new(mode: VoiceMode, muted: bool) -> Self {
        Self { mode, muted, mic_open: false, push_recording: false, awaiting_until: None }
    }
}

/// What can happen to the voice system.
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceInput {
    SetMode(VoiceMode),
    SetMuted(bool),
    /// The mic button or its hotkey.
    PushPress,
    /// A push recording ended (silence, or asked to), mic already closed.
    PushEnded,
    /// The mic could not be opened or died.
    MicFailed,
    /// An utterance was transcribed. `push`: it came from a push recording.
    Transcript { text: String, now_s: f64, push: bool },
}

/// What the caller must do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEffect {
    OpenWake,
    OpenPush,
    /// Tell a push recording to stop now (it will flush and end itself).
    FinishPush,
    /// Close the microphone at once.
    CloseMic,
    /// Put this text in the Assistant's request box (never runs it).
    Fill(String),
    /// Show a short status line.
    Status(String),
}

/// The pure transition function for mode, mute, push and wake.
pub fn step(ctl: &mut VoiceCtl, input: VoiceInput) -> Vec<VoiceEffect> {
    use VoiceEffect::*;
    let mut fx = Vec::new();
    let close = |ctl: &mut VoiceCtl, fx: &mut Vec<VoiceEffect>| {
        if ctl.mic_open {
            fx.push(CloseMic);
        }
        ctl.mic_open = false;
        ctl.push_recording = false;
        ctl.awaiting_until = None;
    };
    match input {
        VoiceInput::SetMuted(true) => {
            ctl.muted = true;
            close(ctl, &mut fx);
            fx.push(Status("Mic muted".into()));
        }
        VoiceInput::SetMuted(false) => {
            ctl.muted = false;
            if ctl.mode == VoiceMode::Wake && !ctl.mic_open {
                ctl.mic_open = true;
                fx.push(OpenWake);
            }
            fx.push(Status("Mic unmuted".into()));
        }
        VoiceInput::SetMode(m) => {
            close(ctl, &mut fx);
            ctl.mode = m;
            if m == VoiceMode::Wake && !ctl.muted {
                ctl.mic_open = true;
                fx.push(OpenWake);
            }
        }
        VoiceInput::PushPress => {
            if ctl.muted {
                fx.push(Status("Mic muted".into()));
            } else if ctl.mode != VoiceMode::Push {
                fx.push(Status("Set the mic mode to Push to talk first".into()));
            } else if ctl.push_recording {
                fx.push(FinishPush);
            } else {
                ctl.push_recording = true;
                ctl.mic_open = true;
                fx.push(OpenPush);
            }
        }
        VoiceInput::PushEnded => {
            ctl.push_recording = false;
            ctl.mic_open = false;
        }
        VoiceInput::MicFailed => {
            ctl.push_recording = false;
            ctl.mic_open = false;
        }
        VoiceInput::Transcript { text, now_s, push } => {
            let text = text.trim().to_string();
            if ctl.muted {
                fx.push(Status("Mic muted".into()));
            } else if push {
                if ctl.mode == VoiceMode::Push {
                    if text.is_empty() {
                        fx.push(Status("Didn't catch that".into()));
                    } else {
                        fx.push(Fill(text));
                    }
                }
            } else if ctl.mode == VoiceMode::Wake {
                match match_wake(&text) {
                    WakeMatch::Command(c) => {
                        ctl.awaiting_until = None;
                        fx.push(Fill(c));
                    }
                    WakeMatch::WakeOnly => {
                        ctl.awaiting_until = Some(now_s + COMMAND_WINDOW_S);
                        fx.push(Status("Listening for your command…".into()));
                    }
                    WakeMatch::None => {
                        if ctl.awaiting_until.is_some_and(|t| now_s <= t) && !text.is_empty() {
                            ctl.awaiting_until = None;
                            fx.push(Fill(text));
                        }
                    }
                }
            }
        }
    }
    fx
}

// ---------------------------------------------------------------- service

/// What the UI should do after [`VoiceService::tick`] or an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceOut {
    /// A command for the request box.
    Fill(String),
    /// Short message for the Assistant section ("Mic muted", errors, …).
    Status(String),
    /// Everything Whisper heard, wake word included.
    Heard(String),
}

struct Job {
    samples: Vec<f32>,
    push: bool,
    now_s: f64,
}

struct Done {
    result: Result<String, String>,
    push: bool,
    now_s: f64,
}

/// Owns the microphone and the Whisper worker. Not `Send`-sensitive: lives
/// on the UI thread, which only ever polls it.
pub struct VoiceService {
    ctl: VoiceCtl,
    device: Option<String>,
    mic: Option<MicInput>,
    /// The device confirmed it opened.
    opened: bool,
    started: Instant,
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    /// A transcription is running.
    pub busy: bool,
    jobs_in_flight: usize,
    pub last_heard: String,
}

impl VoiceService {
    pub fn new(mode: VoiceMode, muted: bool, device: Option<String>) -> Self {
        let (jtx, jrx) = channel::<Job>();
        let (dtx, drx) = channel::<Done>();
        let _ = std::thread::Builder::new().name("buzz-voice-whisper".into()).spawn(move || {
            while let Ok(job) = jrx.recv() {
                let result = crate::transcribe::transcribe_command(&job.samples, &|| false)
                    .map(|h| h.text)
                    .map_err(|e| e.to_string());
                if dtx.send(Done { result, push: job.push, now_s: job.now_s }).is_err() {
                    break;
                }
            }
        });
        Self {
            ctl: VoiceCtl::new(VoiceMode::Off, muted),
            device,
            mic: None,
            opened: false,
            started: Instant::now(),
            jobs: Some(jtx),
            done: drx,
            busy: false,
            jobs_in_flight: 0,
            last_heard: String::new(),
        }
        .with_initial_mode(mode)
    }

    fn with_initial_mode(mut self, mode: VoiceMode) -> Self {
        let mut out = Vec::new();
        self.apply(VoiceInput::SetMode(mode), &mut out);
        self
    }

    pub fn mode(&self) -> VoiceMode {
        self.ctl.mode
    }

    pub fn muted(&self) -> bool {
        self.ctl.muted
    }

    pub fn set_device(&mut self, device: Option<String>) {
        if self.device != device {
            self.device = device;
            // Re-open on the new device if the mic is open.
            if self.mic.is_some() && self.ctl.mode == VoiceMode::Wake {
                let mode = self.ctl.mode;
                let mut out = Vec::new();
                self.apply(VoiceInput::SetMode(VoiceMode::Off), &mut out);
                self.apply(VoiceInput::SetMode(mode), &mut out);
            }
        }
    }

    /// The red "Listening" indicator: the mic really is open.
    pub fn is_listening(&self) -> bool {
        self.mic.is_some() && self.opened
    }

    /// Mic requested but still opening.
    pub fn is_opening(&self) -> bool {
        self.mic.is_some() && !self.opened
    }

    pub fn is_recording(&self) -> bool {
        self.ctl.push_recording
    }

    pub fn level(&self) -> f32 {
        self.mic.as_ref().map(|m| m.level()).unwrap_or(0.0)
    }

    /// Applies a user action; messages go to `out`.
    pub fn apply(&mut self, input: VoiceInput, out: &mut Vec<VoiceOut>) {
        let effects = step(&mut self.ctl, input);
        self.run_effects(effects, out);
    }

    fn run_effects(&mut self, effects: Vec<VoiceEffect>, out: &mut Vec<VoiceOut>) {
        for e in effects {
            match e {
                VoiceEffect::OpenWake | VoiceEffect::OpenPush => {
                    if !crate::transcribe::multi_ready() {
                        out.push(VoiceOut::Status(
                            "Voice needs the 'Whisper small multilingual' model: download it in AI ▸ Models".into(),
                        ));
                        let _ = step(&mut self.ctl, VoiceInput::MicFailed);
                        continue;
                    }
                    let push = e == VoiceEffect::OpenPush;
                    self.opened = false;
                    self.mic = Some(if push {
                        MicInput::start_push(self.device.clone())
                    } else {
                        MicInput::start(self.device.clone())
                    });
                }
                VoiceEffect::FinishPush => {
                    if let Some(m) = &self.mic {
                        m.finish();
                    }
                }
                VoiceEffect::CloseMic => {
                    // Dropping joins the capture thread: the device is released.
                    self.mic = None;
                    self.opened = false;
                }
                VoiceEffect::Fill(t) => out.push(VoiceOut::Fill(t)),
                VoiceEffect::Status(s) => out.push(VoiceOut::Status(s)),
            }
        }
    }

    /// Call every frame: collects mic events and finished transcriptions.
    pub fn tick(&mut self) -> Vec<VoiceOut> {
        let mut out = Vec::new();
        let push = self.ctl.push_recording;
        let events = self.mic.as_ref().map(|m| m.poll()).unwrap_or_default();
        let mut ended = false;
        for ev in events {
            match ev {
                MicEvent::Opened(_) => self.opened = true,
                MicEvent::Utterance(u) => {
                    let now_s = self.started.elapsed().as_secs_f64();
                    if let Some(tx) = &self.jobs {
                        if tx.send(Job { samples: u.samples, push, now_s }).is_ok() {
                            self.jobs_in_flight += 1;
                            self.busy = true;
                        }
                    }
                }
                MicEvent::Ended => ended = true,
                MicEvent::Failed(msg) => {
                    out.push(VoiceOut::Status(msg));
                    let _ = step(&mut self.ctl, VoiceInput::MicFailed);
                    self.mic = None;
                    self.opened = false;
                    // Wake mode that died stays off until the user toggles it.
                    self.ctl.mode = if self.ctl.mode == VoiceMode::Wake { VoiceMode::Off } else { self.ctl.mode };
                }
            }
        }
        if ended {
            let _ = step(&mut self.ctl, VoiceInput::PushEnded);
            self.mic = None;
            self.opened = false;
        }
        while let Ok(d) = self.done.try_recv() {
            self.jobs_in_flight = self.jobs_in_flight.saturating_sub(1);
            self.busy = self.jobs_in_flight > 0;
            match d.result {
                Ok(text) => {
                    if !text.trim().is_empty() {
                        self.last_heard = text.trim().to_string();
                        out.push(VoiceOut::Heard(self.last_heard.clone()));
                    }
                    let fx = step(&mut self.ctl, VoiceInput::Transcript { text, now_s: d.now_s, push: d.push });
                    self.run_effects(fx, &mut out);
                }
                Err(e) => out.push(VoiceOut::Status(format!("Voice: {e}"))),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use VoiceEffect::*;

    fn cmd(s: &str) -> WakeMatch {
        WakeMatch::Command(s.to_string())
    }

    #[test]
    fn wake_phrases_in_latin_and_devanagari_split_off_the_command() {
        let table: &[(&str, WakeMatch)] = &[
            ("Hey Buzz, add a red ball", cmd("add a red ball")),
            ("hey buzz add a red ball.", cmd("add a red ball.")),
            ("HEY BUZZ! Make the cat wave", cmd("Make the cat wave")),
            ("Hey bus, play the animation", cmd("play the animation")),
            ("Hay buzz play", cmd("play")),
            ("A buzz, add a tree", cmd("add a tree")),
            ("Hey buz add a tree", cmd("add a tree")),
            ("Heybuzz add a tree", cmd("add a tree")),
            ("Hey, Buzz: new layer", cmd("new layer")),
            ("हे बज़, एक लाल गेंद जोड़ो", cmd("एक लाल गेंद जोड़ो")),
            ("हे बज एक पेड़ बनाओ", cmd("एक पेड़ बनाओ")),
            ("हे बस, play karo", cmd("play karo")),
            ("हेय बज़। कैट को हिलाओ", cmd("कैट को हिलाओ")),
            ("Hey Buzz", WakeMatch::WakeOnly),
            ("hey buzz.", WakeMatch::WakeOnly),
            ("हे बज़", WakeMatch::WakeOnly),
            ("Héy Buzz, café", cmd("café")),
        ];
        for (text, want) in table {
            assert_eq!(&match_wake(text), want, "{text}");
        }
    }

    #[test]
    fn ordinary_speech_is_not_a_wake_phrase() {
        for text in [
            "",
            "add a red ball to the scene",
            "hello there",
            "make the bus bigger",
            "how is the buzz in the hive",
            "the buses are late",
            "play the animation",
            "एक लाल गेंद जोड़ो",
            "बस करो",
            "हेलो दोस्तों",
        ] {
            assert_eq!(match_wake(text), WakeMatch::None, "{text}");
        }
    }

    #[test]
    fn normalisation_strips_case_accents_nukta_and_punctuation() {
        assert_eq!(normalise_token("Buzz,"), "buzz");
        assert_eq!(normalise_token("Héy!"), "hey");
        assert_eq!(normalise_token("बज़"), "बज");
        assert_eq!(normalise_token("..."), "");
    }

    fn ctl(mode: VoiceMode) -> VoiceCtl {
        let mut c = VoiceCtl::new(VoiceMode::Off, false);
        let _ = step(&mut c, VoiceInput::SetMode(mode));
        c
    }

    fn said(t: &str, now: f64, push: bool) -> VoiceInput {
        VoiceInput::Transcript { text: t.into(), now_s: now, push }
    }

    #[test]
    fn push_start_then_silence_ends_it_and_the_transcript_fills_the_box() {
        let mut c = ctl(VoiceMode::Push);
        assert!(!c.mic_open, "push keeps the mic closed between presses");
        assert_eq!(step(&mut c, VoiceInput::PushPress), vec![OpenPush]);
        assert!(c.mic_open && c.push_recording);
        // 3 s of silence: the recorder ends itself.
        assert!(step(&mut c, VoiceInput::PushEnded).is_empty());
        assert!(!c.mic_open && !c.push_recording);
        // No wake word needed in push mode.
        assert_eq!(step(&mut c, said("add a red ball", 5.0, true)), vec![Fill("add a red ball".into())]);
        assert_eq!(step(&mut c, said("  ", 6.0, true)), vec![Status("Didn't catch that".into())]);
    }

    #[test]
    fn pressing_again_stops_early() {
        let mut c = ctl(VoiceMode::Push);
        step(&mut c, VoiceInput::PushPress);
        assert_eq!(step(&mut c, VoiceInput::PushPress), vec![FinishPush]);
        step(&mut c, VoiceInput::PushEnded);
        assert!(!c.mic_open);
        assert_eq!(step(&mut c, VoiceInput::PushPress), vec![OpenPush]);
    }

    #[test]
    fn mute_closes_the_mic_from_any_state_and_wins_over_every_mode() {
        for mode in VoiceMode::ALL {
            let mut c = ctl(mode);
            if mode == VoiceMode::Push {
                step(&mut c, VoiceInput::PushPress);
            }
            let was_open = c.mic_open;
            let fx = step(&mut c, VoiceInput::SetMuted(true));
            assert_eq!(fx.contains(&CloseMic), was_open, "{mode:?}");
            assert!(!c.mic_open && !c.push_recording && c.muted);
            // Switching mode while muted never opens the mic.
            let fx = step(&mut c, VoiceInput::SetMode(VoiceMode::Wake));
            assert!(!fx.contains(&OpenWake) && !c.mic_open);
        }
    }

    #[test]
    fn muted_push_press_and_wake_hits_do_nothing_but_say_so() {
        let mut c = ctl(VoiceMode::Push);
        step(&mut c, VoiceInput::SetMuted(true));
        assert_eq!(step(&mut c, VoiceInput::PushPress), vec![Status("Mic muted".into())]);
        assert!(!c.mic_open);
        let mut w = ctl(VoiceMode::Wake);
        step(&mut w, VoiceInput::SetMuted(true));
        assert_eq!(step(&mut w, said("hey buzz add a tree", 1.0, false)), vec![Status("Mic muted".into())]);
        // Unmuting in wake mode re-opens the mic.
        assert!(step(&mut w, VoiceInput::SetMuted(false)).contains(&OpenWake));
        assert!(w.mic_open);
    }

    #[test]
    fn wake_mode_opens_only_while_on_and_a_bare_wake_waits_six_seconds() {
        let mut c = VoiceCtl::new(VoiceMode::Off, false);
        assert!(!c.mic_open);
        assert_eq!(step(&mut c, VoiceInput::SetMode(VoiceMode::Wake)), vec![OpenWake]);
        assert_eq!(step(&mut c, VoiceInput::SetMode(VoiceMode::Off)), vec![CloseMic]);
        step(&mut c, VoiceInput::SetMode(VoiceMode::Wake));
        // Speech without the wake word is ignored.
        assert!(step(&mut c, said("add a tree", 1.0, false)).is_empty());
        // Bare wake, then the command in time.
        assert_eq!(step(&mut c, said("Hey Buzz", 10.0, false)), vec![Status("Listening for your command…".into())]);
        assert_eq!(step(&mut c, said("add a tree", 14.0, false)), vec![Fill("add a tree".into())]);
        // Bare wake, command too late.
        step(&mut c, said("Hey Buzz", 20.0, false));
        assert!(step(&mut c, said("add a tree", 27.0, false)).is_empty());
        // Wake plus command in one go.
        assert_eq!(step(&mut c, said("hey buzz new layer", 30.0, false)), vec![Fill("new layer".into())]);
        // A push transcript is not a wake transcript.
        assert!(step(&mut c, said("hello", 31.0, true)).is_empty());
    }

    #[test]
    fn switching_mode_closes_a_push_recording() {
        let mut c = ctl(VoiceMode::Push);
        step(&mut c, VoiceInput::PushPress);
        assert_eq!(step(&mut c, VoiceInput::SetMode(VoiceMode::Off)), vec![CloseMic]);
        assert!(!c.push_recording);
        // Pressing in a non-push mode explains itself.
        assert_eq!(
            step(&mut c, VoiceInput::PushPress),
            vec![Status("Set the mic mode to Push to talk first".into())]
        );
    }
}
