//! **A voice**: every slider of the voice designer, as numbers that save.
//!
//! A profile is a recipe applied to the creator's own performance. It never
//! replaces the performance: his timing, his stresses, his emotion are all
//! still there, carried by pitch marks and loudness that the recipe moves
//! around but does not invent. That is why the same man can be a dozen
//! characters and every one of them still *acts*.

use serde::{Deserialize, Serialize};

/// Every knob of the voice designer.
///
/// All fields default to "leave the recording as it is", so a profile with
/// only `name` set is the creator's own voice, untouched — and a file written
/// by an older build that lacks a knob gets that knob switched off rather than
/// failing to load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceProfile {
    pub name: String,

    // -- pitch & size --------------------------------------------------------
    /// Pitch, in semitones (−12 … +12). Twelve is an octave.
    pub pitch: f32,
    /// Formant shift, as a ratio (0.7 … 1.45): the *size* of the throat and
    /// mouth. Above 1 is smaller (a woman, a child, a mouse); below 1 is
    /// bigger (a large man, a giant). This, not pitch, is what stops a raised
    /// voice sounding like a sped-up tape.
    pub formant: f32,
    /// Intonation range (0 … 2): how far the pitch swings around its centre.
    /// 1 is the performance; 0 is a monotone robot; 1.5 is sing-song.
    pub intonation: f32,
    /// Speaking rate (0.6 … 1.6), without changing pitch.
    pub tempo: f32,

    // -- texture -------------------------------------------------------------
    /// Air in the voice (0 … 1).
    pub breathiness: f32,
    /// Rasp and growl (0 … 1): sub-harmonics and jitter.
    pub roughness: f32,
    /// Vibrato / quaver: pitch wobble depth in semitones (0 … 1.5).
    pub vibrato: f32,
    /// Vibrato speed, Hz (3 … 9).
    pub vibrato_rate: f32,
    /// Tremolo: level wobble (0 … 0.8).
    pub tremolo: f32,

    // -- tone ----------------------------------------------------------------
    /// Body / chest, dB at the low end (−12 … +12).
    pub warmth: f32,
    /// Presence / brightness, dB at the top (−12 … +12).
    pub brightness: f32,
    /// Nasal colour, dB around 1 kHz (−12 … +12).
    pub nasal: f32,
    /// Telephone / radio (0 … 1).
    pub telephone: f32,

    // -- level & space -------------------------------------------------------
    /// Volume, dB (−24 … +12).
    pub gain: f32,
    /// Evens out loud and soft (0 … 1).
    pub compression: f32,
    /// Room reverb (0 … 1).
    pub room: f32,
    /// Bring every line to the same peak level.
    pub normalize: bool,

    /// How much a line's emotion is allowed to push this voice (0 … 1): 0
    /// leaves the performance exactly as acted; 1 adds the full
    /// [`Emotion::push`] on top. Off by default — he already acted it.
    pub emotion_push: f32,
}

impl Default for VoiceProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            pitch: 0.0,
            formant: 1.0,
            intonation: 1.0,
            tempo: 1.0,
            breathiness: 0.0,
            roughness: 0.0,
            vibrato: 0.0,
            vibrato_rate: 5.5,
            tremolo: 0.0,
            warmth: 0.0,
            brightness: 0.0,
            nasal: 0.0,
            telephone: 0.0,
            gain: 0.0,
            compression: 0.0,
            room: 0.0,
            normalize: true,
            emotion_push: 0.0,
        }
    }
}

/// The voices a new character starts from. Every one is just a profile —
/// pick one, then move the sliders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Preset {
    /// The recording as it is: only levelled.
    Natural,
    Man,
    DeepMan,
    YoungMan,
    OldMan,
    Woman,
    YoungWoman,
    OldWoman,
    Boy,
    Girl,
    Giant,
    LittleCreature,
    Ghost,
    Robot,
    Radio,
}

impl Preset {
    pub const ALL: [Preset; 15] = [
        Preset::Natural,
        Preset::Man,
        Preset::DeepMan,
        Preset::YoungMan,
        Preset::OldMan,
        Preset::Woman,
        Preset::YoungWoman,
        Preset::OldWoman,
        Preset::Boy,
        Preset::Girl,
        Preset::Giant,
        Preset::LittleCreature,
        Preset::Ghost,
        Preset::Robot,
        Preset::Radio,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Natural => "Natural (my voice)",
            Preset::Man => "Man",
            Preset::DeepMan => "Deep man",
            Preset::YoungMan => "Young man",
            Preset::OldMan => "Old man",
            Preset::Woman => "Woman",
            Preset::YoungWoman => "Young woman",
            Preset::OldWoman => "Old woman",
            Preset::Boy => "Boy",
            Preset::Girl => "Girl",
            Preset::Giant => "Giant",
            Preset::LittleCreature => "Little creature",
            Preset::Ghost => "Ghost / whisper",
            Preset::Robot => "Robot",
            Preset::Radio => "Radio / phone",
        }
    }

    /// The preset as a profile. These assume an adult male performer — the
    /// creator — so "Woman" raises pitch and shrinks the throat; a female
    /// performer would start from Natural and go the other way.
    pub fn profile(self) -> VoiceProfile {
        let base = VoiceProfile { name: self.label().into(), ..VoiceProfile::default() };
        match self {
            Preset::Natural => base,
            Preset::Man => VoiceProfile { pitch: -1.0, formant: 0.97, warmth: 2.0, ..base },
            Preset::DeepMan => VoiceProfile {
                pitch: -4.0,
                formant: 0.88,
                warmth: 5.0,
                brightness: -2.0,
                intonation: 0.85,
                ..base
            },
            Preset::YoungMan => VoiceProfile { pitch: 2.0, formant: 1.04, brightness: 2.0, ..base },
            Preset::OldMan => VoiceProfile {
                pitch: -1.5,
                formant: 0.95,
                roughness: 0.35,
                vibrato: 0.35,
                vibrato_rate: 5.0,
                breathiness: 0.2,
                tempo: 0.9,
                brightness: -3.0,
                ..base
            },
            Preset::Woman => VoiceProfile {
                pitch: 7.0,
                formant: 1.17,
                intonation: 1.15,
                breathiness: 0.12,
                brightness: 2.0,
                warmth: -2.0,
                ..base
            },
            Preset::YoungWoman => VoiceProfile {
                pitch: 9.0,
                formant: 1.2,
                intonation: 1.25,
                breathiness: 0.1,
                brightness: 3.0,
                warmth: -3.0,
                ..base
            },
            Preset::OldWoman => VoiceProfile {
                pitch: 5.0,
                formant: 1.12,
                roughness: 0.3,
                vibrato: 0.45,
                vibrato_rate: 5.5,
                breathiness: 0.25,
                tempo: 0.9,
                brightness: -2.0,
                ..base
            },
            Preset::Boy => VoiceProfile {
                pitch: 9.0,
                formant: 1.25,
                intonation: 1.2,
                tempo: 1.05,
                warmth: -4.0,
                ..base
            },
            Preset::Girl => VoiceProfile {
                pitch: 11.0,
                formant: 1.3,
                intonation: 1.3,
                tempo: 1.05,
                warmth: -4.0,
                brightness: 2.0,
                ..base
            },
            Preset::Giant => VoiceProfile {
                pitch: -9.0,
                formant: 0.76,
                warmth: 6.0,
                tempo: 0.85,
                room: 0.35,
                ..base
            },
            Preset::LittleCreature => VoiceProfile {
                pitch: 12.0,
                formant: 1.42,
                intonation: 1.4,
                tempo: 1.15,
                nasal: 4.0,
                ..base
            },
            Preset::Ghost => VoiceProfile {
                breathiness: 0.8,
                intonation: 0.7,
                room: 0.7,
                tremolo: 0.2,
                gain: -3.0,
                ..base
            },
            Preset::Robot => VoiceProfile {
                intonation: 0.0,
                roughness: 0.25,
                nasal: 3.0,
                telephone: 0.25,
                ..base
            },
            Preset::Radio => VoiceProfile { telephone: 1.0, compression: 0.5, ..base },
        }
    }
}

/// How a line is said. Read from the script's tag — `(angry)`, `[khush]` —
/// or filled in by the breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Emotion {
    #[default]
    Neutral,
    Happy,
    Sad,
    Angry,
    /// Horrified, terrified, scared.
    Scared,
    Serious,
    Surprised,
    Disgusted,
    Whisper,
    Excited,
    Tired,
}

impl Emotion {
    pub const ALL: [Emotion; 11] = [
        Emotion::Neutral,
        Emotion::Happy,
        Emotion::Sad,
        Emotion::Angry,
        Emotion::Scared,
        Emotion::Serious,
        Emotion::Surprised,
        Emotion::Disgusted,
        Emotion::Whisper,
        Emotion::Excited,
        Emotion::Tired,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Emotion::Neutral => "neutral",
            Emotion::Happy => "happy",
            Emotion::Sad => "sad",
            Emotion::Angry => "angry",
            Emotion::Scared => "scared",
            Emotion::Serious => "serious",
            Emotion::Surprised => "surprised",
            Emotion::Disgusted => "disgusted",
            Emotion::Whisper => "whisper",
            Emotion::Excited => "excited",
            Emotion::Tired => "tired",
        }
    }

    /// Read a script tag: `angry`, `very angry`, `slightly scared`,
    /// `horrified`, and the Hinglish words a script written in Roman Hindi
    /// actually uses — `gussa`, `khush`, `dukhi`, `darr`, `dheere se`.
    /// Returns the emotion and its intensity (0 … 1).
    pub fn parse(tag: &str) -> Option<(Emotion, f32)> {
        let lower = tag.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let has = |w: &str| words.iter().any(|x| x.starts_with(w));
        let intensity = if has("very") || has("bahut") || has("extremely") || has("furious") || has("terrified") || has("horrif") {
            1.0
        } else if has("slight") || has("little") || has("thoda") || has("bit") || has("mild") {
            0.35
        } else {
            0.65
        };
        let table: [(&[&str], Emotion); 10] = [
            (&["angr", "furious", "rage", "gussa", "gusse", "irritat", "annoy", "shout", "yell"], Emotion::Angry),
            (&["horrif", "terrif", "scare", "afraid", "fear", "panic", "darr", "dar", "nervous", "anxious"], Emotion::Scared),
            (&["happy", "joy", "laugh", "cheer", "delight", "khush", "smil", "glad"], Emotion::Happy),
            (&["sad", "cry", "tear", "dukh", "upset", "sorrow", "grief", "udaas", "udas"], Emotion::Sad),
            (&["serious", "stern", "grave", "calm", "firm", "gambhir"], Emotion::Serious),
            (&["surpris", "shock", "amaz", "astonish", "hairan", "hairaan"], Emotion::Surprised),
            (&["disgust", "gross", "sneer", "contempt", "chhi"], Emotion::Disgusted),
            (&["whisper", "softly", "quiet", "dheere", "hush"], Emotion::Whisper),
            (&["excit", "eager", "thrill", "josh"], Emotion::Excited),
            (&["tired", "sleepy", "exhaust", "weary", "bored", "thak"], Emotion::Tired),
        ];
        table
            .iter()
            .find(|(keys, _)| keys.iter().any(|k| has(k)))
            .map(|(_, e)| (*e, intensity))
            .or_else(|| has("neutral").then_some((Emotion::Neutral, 0.0)))
    }

    /// What this emotion does to a voice at full intensity.
    pub fn push(self) -> Push {
        let none = Push::default();
        match self {
            Emotion::Neutral => none,
            Emotion::Happy => Push { pitch: 2.0, range: 1.25, tempo: 1.06, brightness: 2.0, ..none },
            Emotion::Sad => Push { pitch: -1.5, range: 0.7, tempo: 0.88, breath: 0.15, gain: -2.0, ..none },
            Emotion::Angry => Push {
                pitch: 1.0,
                range: 1.3,
                tempo: 1.08,
                rough: 0.25,
                compression: 0.35,
                gain: 2.0,
                brightness: 2.5,
                ..none
            },
            Emotion::Scared => Push {
                pitch: 3.0,
                range: 1.4,
                tempo: 1.1,
                vibrato: 0.35,
                breath: 0.2,
                ..none
            },
            Emotion::Serious => Push { pitch: -1.0, range: 0.8, tempo: 0.94, warmth: 1.5, ..none },
            Emotion::Surprised => Push { pitch: 3.0, range: 1.5, ..none },
            Emotion::Disgusted => Push { pitch: -0.5, range: 0.9, nasal: 3.0, rough: 0.1, ..none },
            Emotion::Whisper => Push { breath: 0.6, gain: -6.0, ..none },
            Emotion::Excited => Push { pitch: 2.5, range: 1.4, tempo: 1.12, ..none },
            Emotion::Tired => Push { pitch: -1.5, range: 0.6, tempo: 0.85, breath: 0.2, ..none },
        }
    }
}

/// An emotion's nudge to a voice. Additive except `range` and `tempo`, which
/// multiply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Push {
    pub pitch: f32,
    pub range: f32,
    pub tempo: f32,
    pub breath: f32,
    pub rough: f32,
    pub vibrato: f32,
    pub compression: f32,
    pub gain: f32,
    pub brightness: f32,
    pub warmth: f32,
    pub nasal: f32,
}

impl Default for Push {
    fn default() -> Self {
        Self {
            pitch: 0.0,
            range: 1.0,
            tempo: 1.0,
            breath: 0.0,
            rough: 0.0,
            vibrato: 0.0,
            compression: 0.0,
            gain: 0.0,
            brightness: 0.0,
            warmth: 0.0,
            nasal: 0.0,
        }
    }
}

impl VoiceProfile {
    /// This voice for one line said with `emotion` at `intensity`, scaled by
    /// [`Self::emotion_push`].
    pub fn for_line(&self, emotion: Emotion, intensity: f32) -> VoiceProfile {
        let k = (self.emotion_push * intensity).clamp(0.0, 1.0);
        if k <= 0.0 {
            return self.clone();
        }
        let p = emotion.push();
        VoiceProfile {
            pitch: self.pitch + p.pitch * k,
            intonation: self.intonation * p.range.powf(k),
            tempo: self.tempo * p.tempo.powf(k),
            breathiness: (self.breathiness + p.breath * k).min(1.0),
            roughness: (self.roughness + p.rough * k).min(1.0),
            vibrato: self.vibrato + p.vibrato * k,
            vibrato_rate: if p.vibrato > 0.0 { self.vibrato_rate.max(6.5) } else { self.vibrato_rate },
            compression: (self.compression + p.compression * k).min(1.0),
            gain: self.gain + p.gain * k,
            brightness: self.brightness + p.brightness * k,
            warmth: self.warmth + p.warmth * k,
            nasal: self.nasal + p.nasal * k,
            ..self.clone()
        }
    }

    /// Every number pulled into the range its slider offers, so a hand-edited
    /// file cannot ask for something the engine was never meant to do.
    pub fn clamped(&self) -> VoiceProfile {
        VoiceProfile {
            pitch: self.pitch.clamp(-12.0, 12.0),
            formant: self.formant.clamp(0.7, 1.45),
            intonation: self.intonation.clamp(0.0, 2.0),
            tempo: self.tempo.clamp(0.6, 1.6),
            breathiness: self.breathiness.clamp(0.0, 1.0),
            roughness: self.roughness.clamp(0.0, 1.0),
            vibrato: self.vibrato.clamp(0.0, 1.5),
            vibrato_rate: self.vibrato_rate.clamp(3.0, 9.0),
            tremolo: self.tremolo.clamp(0.0, 0.8),
            warmth: self.warmth.clamp(-12.0, 12.0),
            brightness: self.brightness.clamp(-12.0, 12.0),
            nasal: self.nasal.clamp(-12.0, 12.0),
            telephone: self.telephone.clamp(0.0, 1.0),
            gain: self.gain.clamp(-24.0, 12.0),
            compression: self.compression.clamp(0.0, 1.0),
            room: self.room.clamp(0.0, 1.0),
            emotion_push: self.emotion_push.clamp(0.0, 1.0),
            ..self.clone()
        }
    }

    /// Does the voice need re-voicing (PSOLA), or only effects?
    pub(crate) fn moves_pitch_or_time(&self) -> bool {
        self.pitch.abs() > 0.01
            || (self.formant - 1.0).abs() > 0.005
            || (self.intonation - 1.0).abs() > 0.01
            || (self.tempo - 1.0).abs() > 0.005
            || self.vibrato > 0.01
            || self.roughness > 0.01
    }
}
