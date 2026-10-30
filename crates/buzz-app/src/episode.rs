//! **Episodes from a script**: the screenplay and a casting file in, staged,
//! timed, captioned and lip-synced scenes out. No audio anywhere.
//!
//! # What this is
//!
//! A writer's script says who is in a scene, what they say and what they do. A
//! casting file says which drawing plays each name. This joins the two into a
//! document an animator can open: one document scene per script scene, each
//! with its set, its cast standing on a ground line, a performance on every
//! character, a mouth that moves on the lines, and subtitles.
//!
//! Nothing here is new machinery. The set is [`buzz_act::stage_scene`] and
//! [`buzz_act::lay_scenery`] (or assets from the library, placed by the same
//! `AssetLibrary::place` the Assets panel uses), the performances are
//! [`buzz_act::perform_from`], the mouths go through
//! [`buzz_act::lipsync::write_track`] or, for a rig whose mouth is part of its
//! armature, [`buzz_act::textsync::write_rig_mouth`]. The words become frames
//! through [`buzz_act::script_timing`] and mouth shapes through
//! [`buzz_act::textsync`].
//!
//! # The casting file
//!
//! ```json
//! {
//!   "characters": {
//!     "GUDDU":    {"cast": "Guddu"},
//!     "CHHOTU":   {"asset": "chaiwalNew"},
//!     "SHARMA JI":{"puppet": true},
//!     "NARRATOR": {"voice_only": true}
//!   },
//!   "scenes": {
//!     "4": {
//!       "setting": "daylight",
//!       "background": ["backgroundRoadA"],
//!       "props": [{"asset": "chaiTapri", "x": 0.7, "y": 0.62, "h": 0.5}],
//!       "place": {"GUDDU": {"x": 0.3}, "MUNNA": {"x": 0.42}}
//!     }
//!   }
//! }
//! ```
//!
//! * **`characters`** decide by name how a cue is staged. `cast` is a rig in the
//!   Cast folder of the asset library; `asset` is any asset (not performed
//!   unless it contains a rig); `puppet` is the app's own built-in character,
//!   which needs no drawings (and renders several times slower than a rig); `voice_only` is heard and captioned but never
//!   seen. A name with none of these is **skipped and reported**, never a
//!   failure. Also: `h` (the figure's height as a fraction of the stage
//!   height), `faces` (`"left"`, `"right"` or `"front"`: which way the drawing
//!   looks, so it can be mirrored to face where it is going), `aliases` (other
//!   ways the action lines name them), `folder` (to pick between same-named
//!   assets), `gestures` (`false` to keep the gestures below off them).
//! * **`scenes`** are keyed by the script's own scene number; `"*"` applies to
//!   every scene with no entry of its own. `background` assets cover the
//!   stage; with none, the set is built (`setting`, `horizon`, `scenery`).
//!   `props` are assets scaled to `h` of the stage height with their base at `y`
//!   and their middle at `x` (fractions of the stage); `"front": true` puts one
//!   in front of the cast. `place` gives a character's `x` (and optionally `y`
//!   for the feet, `h`, `enter` from `"left"` or `"right"`, or its own `cast`,
//!   `asset` for this scene only). `"gestures": false` on a scene keeps every
//!   gesture out of it.
//!
//! # How the script becomes movement
//!
//! * **Who is there.** Everyone named in the scene's `place`, everyone who
//!   speaks on screen and everyone an action line names (by name or alias, in
//!   any case).
//! * **Entrances.** If a character's first appearance is an action line whose
//!   subject *arrives*, *walks in*, *enters*, *walks (briskly) toward*,
//!   *approaches* and so on, they start in the wings on the nearer side and walk
//!   to their mark over that line (at least the time a walk takes at their
//!   size). Otherwise they are on their mark from the first frame.
//! * **Exits.** *leaves*, *exits*, *walks off*, *runs off*, *runs toward*,
//!   *flies away* and the like send them to the nearer wing, walking or
//!   running, and they stay gone.
//! * **Talking.** The speaker talks over each of their lines (lines a few frames
//!   apart are one performance), with a mouth from the words. Everyone else is
//!   idle: a held drawing is a dead one.
//! * **Gestures.** A line can carry one gesture, chosen by [`gesture_for`] from
//!   an action line next to it that names the speaker (*bangs*, *waves*,
//!   *nods*, *sits*…) or from the words (*haan* nods, *nahi* shakes the head,
//!   *namaste*, *arre* waves, a question shrugs). It starts a moment into the
//!   line, over the talk, and the talk carries on after it. A shout (`!` or all
//!   capitals) talks bigger. The report lists every gesture and the word that
//!   chose it.
//! * **Captions.** A `Subtitles` layer, white with a dark outline at the bottom
//!   of the frame, one keyed caption per line: `NAME: line`. Voice-over lines are
//!   italic with no speaker. Each caption's keyframe is labelled with the speaker,
//!   so `Lip Sync from Captions` can find the lines when the recording arrives.
//!   The layer ignores the lights, so a lit scene neither shades the captions
//!   nor throws them onto the ground.
//! * **Title cards.** With a top-level `"title_cards": true` (default off),
//!   every scene opens with a `FadeUp` card of its title over its first two
//!   seconds, on its own top layer ([`buzz_act::title`]); a scene's `"title"`
//!   replaces the script's.
//! * **Blinks.** A puppet's eyes carry the `Blink` modifier. A rig's eyes are
//!   parts of its armature, where a modifier would squash the whole figure, so a
//!   rig blinks by swapping its eye parts for a half-shut and a shut copy on its
//!   own keys, every three to five seconds, differently for each character.
//! * **Mouths.** A rig's mouth part shows the frame of its mouth symbol for the
//!   shape at each key ([`buzz_act::textsync::write_rig_mouth`]); a puppet's
//!   mouth layer is written by [`buzz_act::lipsync::write_track`], the audio lip
//!   sync's own writer. Both are closed everywhere except over a line.
//!
//! # Rigs made from images are heavy, and this makes them lighter
//!
//! A rig keyframe holds the whole armature, parts and their drawings included,
//! and the image pipeline's parts carry megabytes of path data inline. A
//! performance on twos over a minute is thousands of keys, and the first run of
//! this wanted thirty-five gigabytes to save one scene. So each rigid part of a
//! placed rig is moved into a library symbol of its own, and the part becomes an
//! instance of it (`slim_rig`): it draws the same, and a key costs a few hundred
//! bytes instead of megabytes.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use buzz_act::perform::{self, Action, Performance};
use buzz_act::puppet::{self, PuppetSpec};
use buzz_act::scenery::{self, Scenery};
use buzz_act::screenplay::{Dialogue, Item, Place as SlugPlace, Screenplay, ScriptScene, Slugline, Voice};
use buzz_act::script_timing::{Schedule, Timing};
use buzz_act::staging::{self, SceneRecipe, Setting};
use buzz_act::textsync::{self, SpokenLine};
use buzz_act::FigureSpec;
use buzz_act::layer_rig::{CharacterRig, LayerRigView};
use buzz_doc::{Asset, AssetLibrary, Document};
use buzz_geom::{Affine, Point, Rect, Size};
use buzz_scene::{LayerId, Modifier, Object, ObjectId, ObjectKind, Scene, ShapeData, StrokeSpec, TextData};
use peniko::Color;
use serde::Deserialize;
use serde_json::Value;

// -- the casting file ---------------------------------------------------------------

/// How one named character is played.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CastEntry {
    /// A rig in the asset library's Cast folder.
    pub cast: Option<String>,
    /// Any asset in the library. Performed only if it contains a rig.
    pub asset: Option<String>,
    /// The built-in puppet: `true`, or an object with any of `eyes`, `skin`,
    /// `shirt`, `trousers` (hex colours) and `h`.
    pub puppet: Option<Value>,
    /// Heard and captioned, never seen.
    pub voice_only: bool,
    /// Standing height, as a fraction of the stage height.
    pub h: Option<f64>,
    /// Which way the drawing looks: `"left"`, `"right"` or `"front"`.
    pub faces: Option<String>,
    /// Other names the action lines call them.
    pub aliases: Vec<String>,
    /// The asset folder to look in, to tell same-named assets apart.
    pub folder: Option<String>,
    /// `false` keeps gestures off this character. Default on.
    pub gestures: Option<bool>,
}

/// Where and how one character stands in one scene.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PlaceSpec {
    /// Middle of the feet across the stage, `0..1`.
    pub x: Option<f64>,
    /// Feet down the stage, `0..1`. Default is the scene's ground line.
    pub y: Option<f64>,
    pub h: Option<f64>,
    /// `"left"` or `"right"`: come in from there at first appearance.
    pub enter: Option<String>,
    /// Play this scene with a different Cast rig (another view, say).
    pub cast: Option<String>,
    /// Or with a different asset.
    pub asset: Option<String>,
    /// AN11: placed by hand on the stage (stage_pins); angle cuts do not re-stage it.
    pub pinned: bool,
    /// AN13: the pose a `[Name: pose]` tag asked for (`sitting`).
    pub pose: Option<String>,
}

/// AN11: one shot's own camera and background, from the breakdown.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShotBackground {
    /// Index among the scene's dialogue lines; the angle starts as that line starts.
    pub at_line: usize,
    pub camera: String,
    /// The background asset for that angle (the wide one when it has no picture).
    pub background: String,
    /// Where that picture's horizon is, `0..1` down the frame.
    pub horizon_row: Option<f64>,
}

/// A prop on the set.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PropSpec {
    pub asset: String,
    pub folder: Option<String>,
    /// Middle across the stage, `0..1`.
    pub x: Option<f64>,
    /// Base down the stage, `0..1`. Default is the ground line.
    pub y: Option<f64>,
    /// Height as a fraction of the stage height.
    pub h: Option<f64>,
    /// In front of the cast rather than behind it.
    pub front: bool,
}

/// One scene's set and where the cast stands.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SceneSet {
    /// `daylight`, `sunset`, `night`, `interior` or `storm`. Only used when the
    /// set is built (no `background`).
    pub setting: Option<String>,
    /// `village` (default), `forest`, `city`, `meadow`, `waterside`, `bare`.
    pub scenery: Option<String>,
    pub horizon: Option<f64>,
    /// The ground line the cast stands on, `0..1` down the stage.
    pub ground: Option<f64>,
    pub lit: Option<bool>,
    /// Assets that cover the stage, back to front.
    pub background: Vec<String>,
    pub props: Vec<PropSpec>,
    /// P8-C1: the breakdown's prop words. On a set from a `background`
    /// asset, each is matched in the asset index and stood on the ground in
    /// front of it, clear of the cast's marks; ignored without an index.
    pub props_wanted: Vec<String>,
    /// The breakdown's location key: scenes that share it share one prop
    /// layout (P8-C1). Absent: the `background` names are the key.
    pub location: Option<String>,
    pub place: BTreeMap<String, PlaceSpec>,
    /// **Named but not here.** "Raju is gone." and "a woman is talking to
    /// Amma" both name a character the scene does not show; without this the
    /// builder put them on stage because the text mentioned them. Their lines
    /// are still captioned.
    pub absent: Vec<String>,
    /// `false` keeps gestures out of this scene. Default on.
    pub gestures: Option<bool>,
    /// The words of this scene's title card (with `title_cards` on), instead
    /// of the script's own scene title.
    pub title: Option<String>,
    /// P5: the camera. `"auto"`, or a list of [`ShotSpec`]s. Absent: no
    /// camera keys at all.
    pub shots: Option<Shots>,
    /// P5: the key light, mapped onto the built set's lights. Absent: the
    /// setting's own lights, unchanged.
    pub lighting: Option<Lighting>,
    /// AN11: each shot's camera and background, in line order. Absent: one
    /// background for the whole scene.
    pub shot_backgrounds: Vec<ShotBackground>,
    /// AN11: the wide background's horizon, `0..1` down the frame.
    pub wide_horizon: Option<f64>,
}

/// P5: a scene's shots, `"auto"` or a list.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Shots {
    /// `"auto"` (any other word is warned about and ignored).
    Auto(String),
    List(Vec<ShotSpec>),
}

/// P5: one cut, from the casting file (or P1's breakdown).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShotSpec {
    /// Index among the scene's dialogue lines (voice-over too); the shot
    /// starts as that line starts.
    pub at_line: usize,
    /// `wide`, `medium` or `close`.
    pub kind: String,
    /// The character a medium or close is on; the line's speaker when absent.
    pub on: Option<String>,
    /// Seconds before cutting back to wide, when shorter than the time to the
    /// next shot.
    pub hold: Option<f64>,
}

/// P5: how a scene is lit, in the words of P1's breakdown.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Lighting {
    /// `sun`, `lamp`, `moon`, `window` or `none`.
    pub key: String,
    /// `left`, `right` or `front`.
    pub from: String,
    pub warm: bool,
}

/// P5: one cut the camera makes, for the report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShotCut {
    pub frame: u32,
    pub kind: String,
    pub on: Option<String>,
    /// The camera's centre on the stage and its zoom.
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    /// A wide that pushes in 3 % ends its push on this frame.
    pub push_to: Option<u32>,
}

/// The casting file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Casting {
    pub characters: BTreeMap<String, CastEntry>,
    pub scenes: BTreeMap<String, SceneSet>,
    /// Open every scene with a `FadeUp` title card over its first 2 s.
    /// Default off.
    pub title_cards: bool,
    /// AN10: whether a character who is not speaking still blinks and breathes.
    /// Absent = true; `false` holds them dead still outside their own lines.
    pub still_extras: Option<bool>,
}

/// What a name resolves to.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Cast(String),
    Asset(String),
    Puppet(Value),
    VoiceOnly,
    Uncast,
}

impl CastEntry {
    fn kind(&self) -> Kind {
        if self.voice_only {
            Kind::VoiceOnly
        } else if let Some(c) = &self.cast {
            Kind::Cast(c.clone())
        } else if let Some(a) = &self.asset {
            Kind::Asset(a.clone())
        } else if let Some(p) = &self.puppet {
            if matches!(p, Value::Bool(false) | Value::Null) {
                Kind::Uncast
            } else {
                Kind::Puppet(p.clone())
            }
        } else {
            Kind::Uncast
        }
    }
}

fn norm(name: &str) -> String {
    name.trim().to_uppercase()
}

impl Casting {
    /// Read a casting file.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut casting: Casting =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("casting file: {e}"))?;
        // Names are matched upper-case.
        casting.characters = std::mem::take(&mut casting.characters)
            .into_iter()
            .map(|(k, v)| (norm(&k), v))
            .collect();
        for set in casting.scenes.values_mut() {
            set.place = std::mem::take(&mut set.place)
                .into_iter()
                .map(|(k, v)| (norm(&k), v))
                .collect();
        }
        Ok(casting)
    }

    /// The set for a script scene: its own entry, else `"*"`, else nothing.
    pub fn scene_set(&self, number: u32) -> SceneSet {
        self.scenes
            .get(&number.to_string())
            .or_else(|| self.scenes.get("*"))
            .cloned()
            .unwrap_or_default()
    }

    fn character(&self, cue: &str) -> Option<(String, &CastEntry)> {
        let key = norm(cue);
        if let Some(entry) = self.characters.get(&key) {
            return Some((key, entry));
        }
        self.characters
            .iter()
            .find(|(_, e)| e.aliases.iter().any(|a| norm(a) == key))
            .map(|(k, e)| (k.clone(), e))
    }

    /// Every name an action line might use for a character who can be on
    /// stage, longest first so `SHARMA JI` wins over `SHARMA`.
    fn stage_names(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (key, entry) in &self.characters {
            if matches!(entry.kind(), Kind::VoiceOnly | Kind::Uncast) {
                continue;
            }
            let mut names = vec![key.to_lowercase()];
            if let Some(stem) = key.strip_suffix(" JI") {
                names.push(stem.trim().to_lowercase());
            }
            names.extend(entry.aliases.iter().map(|a| a.trim().to_lowercase()));
            for name in names {
                if !name.is_empty() {
                    out.push((name, key.clone()));
                }
            }
        }
        out.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        out
    }
}

// -- options and the report ------------------------------------------------------------

/// How the episode is staged, when the casting file does not say.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildOptions {
    pub width: f64,
    pub height: f64,
    pub fps: f64,
    /// A character's height as a fraction of the stage height.
    pub figure_height: f64,
    /// The ground line, as a fraction of the stage height from the top.
    pub ground: f64,
    pub timing: Timing,
    /// The asset index sets are dressed from (AS3). `None`: [`build_episode`]
    /// reads the library's own, when it has one.
    pub index: Option<Arc<buzz_ai::asset_index::AssetIndex>>,
    /// P2: the dialogue recording, matched to the script
    /// ([`buzz_act::audio_align`]). A scene with lines in it is timed by the
    /// recording, carries its cut of it on a sound layer and is lip-synced
    /// from it; every other scene and line keeps its text timing.
    pub audio: Option<Arc<buzz_act::audio_align::AudioPlan>>,
    /// L3 of `docs/LAYER_MANAGEMENT_PLAN.md`: run Arrange Layers (with its
    /// empty-layer cleanup) on every built scene, inside the same
    /// "Build Scene N" undo step. On by default.
    pub tidy_layers: bool,
    /// AN13: write the caption layer. Off for story channels (no captions or cards).
    pub captions: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            width: 1920.0,
            height: 1080.0,
            fps: 24.0,
            figure_height: 0.45,
            ground: 0.9,
            timing: Timing::default(),
            index: None,
            audio: None,
            tidy_layers: true,
            captions: true,
        }
    }
}

/// How one cast member was staged.
#[derive(Debug, Clone, PartialEq)]
pub struct CastNote {
    pub name: String,
    /// `Cast "Guddu"`, `asset "chaiwalNew"`, `puppet`.
    pub played_by: String,
    pub lines: usize,
    /// Whether a performance was written (a static asset has none).
    pub performs: bool,
    /// Whether the mouth moves on the lines.
    pub lip_synced: bool,
    /// Whether the eyes blink.
    pub blinks: bool,
    pub enters: bool,
    pub exits: bool,
    /// The object that stands for the character (an armature, or a layer
    /// rig's root part): what a storyboard frames a shot on.
    pub body: Option<ObjectId>,
}

/// AN10: one stretch of a character's mouth moving, and whose it is.
#[derive(Debug, Clone, PartialEq)]
pub struct LipSpan {
    pub speaker: String,
    pub start: u32,
    pub end: u32,
    /// `"audio"` (from the recording's mouth shapes) or `"text"` (from the words).
    pub source: &'static str,
}

/// One gesture a line was given.
#[derive(Debug, Clone, PartialEq)]
pub struct GestureNote {
    pub speaker: String,
    /// The frame it starts on.
    pub frame: u32,
    pub action: Action,
    /// The word that chose it.
    pub why: String,
}

/// What one scene came out as.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneReport {
    pub number: u32,
    pub title: String,
    pub frames: u32,
    pub seconds: f64,
    /// Dialogue lines in the script scene.
    pub lines: usize,
    /// Captions written.
    pub captions: usize,
    /// What the set was made of.
    pub set: String,
    /// AS3: each piece a set from the user's assets was dressed with, and why.
    pub set_pieces: Vec<String>,
    pub cast: Vec<CastNote>,
    /// Every gesture written, in the order they were written.
    pub gestures: Vec<GestureNote>,
    /// Names skipped, with why.
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    /// How the scene was timed: which lines from the recording, which from
    /// their text (P2).
    pub notes: Vec<String>,
    /// P5: the camera's cuts, in order. Empty with no `shots`.
    pub shots: Vec<ShotCut>,
    /// AN10: every lip-sync span with its speaker. Nobody else's mouth moves.
    pub lipsync: Vec<LipSpan>,
    /// AN11: per angle, its camera and the background it used.
    pub angles: Vec<AngleNote>,
}

/// AN11: one angle of a scene, for the report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AngleNote {
    pub start: u32,
    pub end: u32,
    pub camera: String,
    /// The background asset shown in this span.
    pub background: String,
    /// How the cast was re-staged for it (`""` for none).
    pub restaged: String,
}

/// The report for a build, as markdown.
pub fn report_markdown(title: &str, reports: &[SceneReport], fps: f64) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Build report: {title}\n\n"));
    let frames: u32 = reports.iter().map(|r| r.frames).sum();
    out.push_str(&format!(
        "{} scene(s), {frames} frames ({:.1} s at {fps} fps).\n\n",
        reports.len(),
        frames as f64 / fps.max(1e-6)
    ));
    for r in reports {
        out.push_str(&format!("## Scene {} \u{2014} {}\n\n", r.number, r.title));
        out.push_str(&format!(
            "* **Frames:** {} ({:.1} s)\n* **Lines:** {} dialogue, {} captions\n* **Set:** {}\n",
            r.frames, r.seconds, r.lines, r.captions, r.set
        ));
        if !r.set_pieces.is_empty() {
            out.push_str("* **Set pieces:**\n");
            for p in &r.set_pieces {
                out.push_str(&format!("  * {p}\n"));
            }
        }
        if r.cast.is_empty() {
            out.push_str("* **Cast:** nobody on stage\n");
        } else {
            out.push_str("* **Cast:**\n");
            for c in &r.cast {
                let mut how = Vec::new();
                if c.enters {
                    how.push("enters");
                }
                if c.exits {
                    how.push("exits");
                }
                how.push(if c.performs { "performs" } else { "static" });
                if c.lines > 0 {
                    how.push(if c.lip_synced { "lip-synced" } else { "mouth not animated" });
                }
                how.push(if c.blinks { "blinks" } else { "no blink" });
                out.push_str(&format!(
                    "  * {} \u{2014} {} \u{2014} {} line(s), {}\n",
                    c.name,
                    c.played_by,
                    c.lines,
                    how.join(", ")
                ));
            }
        }
        if !r.gestures.is_empty() {
            out.push_str("* **Gestures:**\n");
            for g in &r.gestures {
                out.push_str(&format!(
                    "  * {} \u{2014} frame {} \u{2014} {} (\"{}\")\n",
                    g.speaker,
                    g.frame,
                    g.action.label(),
                    g.why
                ));
            }
        }
        if r.lipsync.is_empty() {
            out.push_str("* **Lip sync:** no mouth moves in this scene\n");
        } else {
            out.push_str("* **Lip sync** (only the speaker's mouth moves, narration none):\n");
            for l in &r.lipsync {
                out.push_str(&format!(
                    "  * {} \u{2014} frames {}\u{2013}{} ({:.1}\u{2013}{:.1} s), from {}\n",
                    l.speaker,
                    l.start,
                    l.end,
                    l.start as f64 / fps.max(1e-6),
                    l.end as f64 / fps.max(1e-6),
                    l.source
                ));
            }
        }
        if !r.angles.is_empty() {
            out.push_str("* **Angles** (per shot: camera, background used):\n");
            for a in &r.angles {
                out.push_str(&format!(
                    "  * frames {}\u{2013}{} \u{2014} {} \u{2014} background {}{}\n",
                    a.start,
                    a.end,
                    a.camera,
                    if a.background.is_empty() { "(scene's)" } else { a.background.as_str() },
                    if a.restaged.is_empty() { String::new() } else { format!(" \u{2014} {}", a.restaged) }
                ));
            }
        }
        if r.skipped.is_empty() {
            out.push_str("* **Skipped:** none\n");
        } else {
            out.push_str("* **Skipped:**\n");
            for s in &r.skipped {
                out.push_str(&format!("  * {s}\n"));
            }
        }
        if !r.notes.is_empty() {
            out.push_str("* **Notes:**\n");
            for n in &r.notes {
                out.push_str(&format!("  * {n}\n"));
            }
        }
        if !r.warnings.is_empty() {
            out.push_str("* **Warnings:**\n");
            for w in &r.warnings {
                out.push_str(&format!("  * {w}\n"));
            }
        }
        out.push('\n');
    }
    out
}

// -- reading the action lines ------------------------------------------------------------------

/// Whole-word occurrences of `needle` in `hay`, both already lower-case.
fn find_word(hay: &str, needle: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(at) = hay[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        let before_ok = hay[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after_ok = hay[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return Some(start);
        }
        from = start + needle.len().max(1);
        if from >= hay.len() {
            break;
        }
    }
    None
}

/// What an action clause has its subject do about the stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Movement {
    None,
    Enter,
    Exit { run: bool },
}

const ENTER_PHRASES: &[&str] = &[
    "arrives",
    "arrive",
    "walks in",
    "walks briskly",
    "walks toward",
    "walks up",
    "walks over",
    "comes in",
    "comes toward",
    "enters",
    "approaches",
    "approach",
];

const EXIT_PHRASES: &[(&str, bool)] = &[
    ("runs toward", true),
    ("runs off", true),
    ("runs away", true),
    ("flies away", false),
    ("walks off", false),
    ("walks away", false),
    ("leaves", false),
    ("exits", false),
    ("goes out", false),
    ("heads out", false),
    ("storms out", false),
];

fn movement_of(clause_lower: &str) -> Movement {
    for (phrase, run) in EXIT_PHRASES {
        if find_word(clause_lower, phrase).is_some() {
            return Movement::Exit { run: *run };
        }
    }
    for phrase in ENTER_PHRASES {
        if find_word(clause_lower, phrase).is_some() {
            return Movement::Enter;
        }
    }
    Movement::None
}

/// Storyboard S1: whether an action line moves someone on or off, by the
/// builder's own phrases: `Some(true)` an entrance, `Some(false)` an exit.
pub fn action_moves(action_text: &str) -> Option<bool> {
    clauses(action_text).iter().find_map(|c| match movement_of(c) {
        Movement::Enter => Some(true),
        Movement::Exit { .. } => Some(false),
        Movement::None => None,
    })
}

/// The clauses of an action line, lower-case. Split at the words that start a
/// second thing happening, not at commas: `RAJU, 28, walks briskly` is one
/// clause with its subject first.
fn clauses(text: &str) -> Vec<String> {
    let mut lowered = text.to_lowercase();
    for sep in [" as ", " and ", " while ", " then ", "; ", ". ", "! ", "? "] {
        lowered = lowered.replace(sep, "\u{1}");
    }
    lowered
        .split('\u{1}')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

// -- gestures ---------------------------------------------------------------------------------

/// A gesture chosen for a line, and the word that chose it.
#[derive(Debug, Clone, PartialEq)]
struct Cue {
    action: Action,
    amount: f64,
    why: String,
    /// An action line asked for it, rather than the words suggesting it. Only
    /// these may land on two of a speaker's lines in a row.
    asked: bool,
}

/// The first word that starts with one of `stems` — the director's rule
/// (`buzz_act::direct`), a prefix of a whole word, so "walks" answers to
/// "walk" and a name with "walk" inside it does not.
fn stem_word(words: &[String], stems: &[&str]) -> Option<String> {
    words
        .iter()
        .find(|w| stems.iter().any(|s| w.starts_with(s)))
        .cloned()
}

/// **What an action line has its subject do with their body**, if it is one
/// of the gestures. The stems are the director's own (`buzz_act::direct`), so
/// the two read *sits*, *gets up* and *flinches* the same way; *wave* and
/// *beckon* are a wave here rather than the director's point, and a bang on
/// something is a small reaction.
fn action_cue(action_text: &str) -> Option<Cue> {
    let lower = action_text.to_lowercase();
    let words = buzz_act::direct::words_of(&lower);
    if words.is_empty() {
        return None;
    }
    let cue = |action: Action, amount: f64, why: String| Cue {
        action,
        amount,
        why,
        asked: true,
    };
    let up = lower.contains(" up");
    // Getting up before sitting down: "stands up" has nothing to do with a
    // chair, and "sits up" is not sitting down either.
    let getting_up = stem_word(&words, &["rise", "rose", "risen"]).or_else(|| {
        up.then(|| stem_word(&words, &["stand", "get", "got", "clamber", "haul"]))
            .flatten()
    });
    if let Some(w) = getting_up {
        return Some(cue(Action::Stand, 1.0, w));
    }
    if let Some(w) = stem_word(&words, &["sit", "sat", "seat", "perch", "slump"]) {
        return Some(cue(Action::Sit, 1.0, w));
    }
    if let Some(w) = stem_word(
        &words,
        &["react", "recoil", "flinch", "startl", "jump", "gasp", "wince", "jolt", "shrink"],
    ) {
        return Some(cue(Action::React, 1.0, w));
    }
    // A blow on something: the body jolts with it, by less than a fright.
    if let Some(w) = stem_word(&words, &["bang", "slam", "thump"]) {
        return Some(cue(Action::React, 0.6, w));
    }
    let reaching = stem_word(&words, &["reach", "grab", "grasp", "seiz", "snatch", "lift", "pluck"])
        .or_else(|| up.then(|| stem_word(&words, &["pick", "take", "took"])).flatten());
    if let Some(w) = reaching {
        return Some(cue(Action::Reach, 1.0, w));
    }
    if let Some(w) = stem_word(&words, &["wave", "waving", "beckon"]) {
        return Some(cue(Action::Wave, 1.0, w));
    }
    if let Some(w) = stem_word(&words, &["point", "gestur", "indicat"]) {
        return Some(cue(Action::Point, 1.0, w));
    }
    if let Some(w) = stem_word(&words, &["nod"]) {
        return Some(cue(Action::Nod, 1.0, w));
    }
    if let Some(w) = stem_word(&words, &["shrug"]) {
        return Some(cue(Action::Shrug, 1.0, w));
    }
    None
}

/// **What the words themselves suggest**, Romanised Hindi and English: a
/// leading *haan* nods, a leading *nahi* shakes the head, a greeting anywhere
/// is a namaste, a call for attention at the start is a small wave, and a
/// question with nothing else to go on shrugs.
fn dialogue_cue(line: &str) -> Option<Cue> {
    let lower = line.to_lowercase();
    let words = buzz_act::direct::words_of(&lower);
    let cue = |action: Action, amount: f64, why: &str| Cue {
        action,
        amount,
        why: why.to_string(),
        asked: false,
    };
    if let Some(w) = words
        .iter()
        .find(|w| matches!(w.as_str(), "namaste" | "namaskar" | "pranam"))
    {
        return Some(cue(Action::Namaste, 1.0, w));
    }
    if words.windows(2).any(|p| p[0] == "ram" && p[1] == "ram") {
        return Some(cue(Action::Namaste, 1.0, "ram ram"));
    }
    if let Some(first) = words.first().map(String::as_str) {
        match first {
            "haan" | "ha" | "ji" | "theek" | "accha" => return Some(cue(Action::Nod, 1.0, first)),
            "nahi" | "nahin" | "na" | "no" => return Some(cue(Action::HeadShake, 1.0, first)),
            "arre" | "oye" | "suno" | "hey" => return Some(cue(Action::Wave, 0.6, first)),
            _ => {}
        }
    }
    if line.trim_end().ends_with('?') {
        return Some(cue(Action::Shrug, 0.5, "?"));
    }
    None
}

/// Both, the action line first: a writer who says what the body does has
/// said more than the words imply.
fn cue_for(line: &str, action_text: &str) -> Option<Cue> {
    action_cue(action_text).or_else(|| dialogue_cue(line))
}

/// **The gesture for a line**, and how big: from `action_text` (the action
/// line next to it that names the speaker, or `""`) and then from the line's
/// own words. At most one; `None` for a line that is just talk.
pub fn gesture_for(line: &str, action_text: &str) -> Option<(Action, f64)> {
    cue_for(line, action_text).map(|c| (c.action, c.amount))
}

/// **How big the talking is.** A shout — a `!`, or a line in capitals — is
/// talked bigger, with no gesture of its own for it.
pub fn talk_amount(line: &str) -> f64 {
    let letters: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    let capitals = letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase());
    if line.contains('!') || capitals { 1.3 } else { 1.0 }
}

/// The clauses of an action line that are about `key`: the ones it is the
/// first-named in, and the unnamed ones after those, which carry on with the
/// same subject ("The conductor bangs the bus and shouts").
fn clauses_about(text: &str, key: &str, names: &[(String, String)]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut subject: Option<String> = None;
    for clause in clauses(text) {
        let first = names
            .iter()
            .filter_map(|(alias, k)| find_word(&clause, alias).map(|at| (at, k)))
            .min_by_key(|(at, _)| *at);
        if let Some((_, k)) = first {
            subject = Some(k.clone());
        }
        if subject.as_deref() == Some(key) {
            out.push(clause);
        }
    }
    out.join(". ")
}

/// How long a gesture takes in a line: the move's own time, kept between
/// `0.9 s` and `1.4 s`.
fn gesture_seconds(action: Action) -> f64 {
    action.cycle_seconds().clamp(0.9, 1.4)
}

/// Does this rig have a head bone? The head is bone 2 on the puppet and on
/// every Cast rig (`RigPattern::biped_full` keeps the puppet's order); a rig
/// whose third bone is something else, or that has no third bone, has none.
fn has_head_bone(scene: &Scene, body: ObjectId) -> bool {
    let head = buzz_act::Joint::Head.index();
    match buzz_act::layer_rig::rig_of(scene, body) {
        Some(CharacterRig::Armature(_)) => scene.find_object(body).is_some_and(|(_, o)| match &o.kind {
            ObjectKind::Armature(rig) => rig
                .armature
                .bones
                .get(head)
                .is_some_and(|b| b.name.to_lowercase().contains("head")),
            _ => false,
        }),
        // A layer rig has a head when a part is in the head's slot.
        Some(CharacterRig::Layers(view)) => {
            view.has_slot(head)
                && view
                    .pattern
                    .slots
                    .get(head)
                    .is_some_and(|s| s.name.to_lowercase().contains("head"))
        }
        None => false,
    }
}

// -- who is in the scene --------------------------------------------------------------

#[derive(Debug, Clone)]
struct Line {
    start: u32,
    frames: u32,
    text: String,
    /// How big the talk is over it ([`talk_amount`]).
    amount: f64,
    /// The gesture it carries, if any.
    cue: Option<Cue>,
}

#[derive(Debug, Clone)]
struct Member {
    key: String,
    entry: CastEntry,
    place: PlaceSpec,
    first_item: usize,
    enter_item: Option<usize>,
    exit: Option<(usize, bool)>,
    lines: Vec<Line>,
    /// AN10: moves the action lines ask of this character (stand, sit, point,
    /// turn) that no spoken line of theirs already carries.
    moves: Vec<Move>,
    /// AN10: blinks and breathing while not speaking ([`Casting::still_extras`]).
    extras: bool,
}

#[derive(Debug, Clone)]
struct Move {
    start: u32,
    cue: Cue,
}

/// AN10: the body moves an action line may ask of its subject at its own time.
fn move_cue(clause: &str) -> Option<Cue> {
    if let Some(c) = action_cue(clause)
        && matches!(c.action, Action::Stand | Action::Sit | Action::Point)
    {
        return Some(c);
    }
    let words = buzz_act::direct::words_of(clause);
    stem_word(&words, &["turn"]).map(|w| Cue { action: Action::Turn, amount: 1.0, why: w, asked: true })
}

fn snap_even(frame: u32) -> u32 {
    frame & !1
}

/// Work out who is on stage, when they arrive and leave and what they say.
fn roster(
    src: &ScriptScene,
    schedule: &Schedule,
    casting: &Casting,
    set: &SceneSet,
    report: &mut SceneReport,
) -> Vec<Member> {
    let names = casting.stage_names();
    let mut members: Vec<Member> = Vec::new();
    let mut unmatched: BTreeMap<String, usize> = BTreeMap::new();
    // Who the script has named so far. A character the casting file places is
    // *there* from the first frame unless the script brings them in: being in
    // `place` says where they stand, not that they were already standing.
    let mut appeared: HashSet<String> = HashSet::new();
    let absent: HashSet<String> = set.absent.iter().map(|n| n.trim().to_uppercase()).collect();

    fn index_of(members: &[Member], key: &str) -> Option<usize> {
        members.iter().position(|m| m.key == key)
    }

    fn ensure(
        members: &mut Vec<Member>,
        casting: &Casting,
        set: &SceneSet,
        key: &str,
        item: usize,
    ) -> usize {
        if let Some(i) = index_of(members, key) {
            return i;
        }
        members.push(Member {
            key: key.to_string(),
            entry: casting.characters.get(key).cloned().unwrap_or_default(),
            place: set.place.get(key).cloned().unwrap_or_default(),
            first_item: item,
            enter_item: None,
            exit: None,
            lines: Vec::new(),
            moves: Vec::new(),
            extras: casting.still_extras != Some(false),
        });
        members.len() - 1
    }

    // Everyone the set places is there from the first item.
    for key in set.place.keys() {
        match casting.characters.get(key).map(|e| e.kind()) {
            Some(Kind::Uncast) | Some(Kind::VoiceOnly) | None => {
                report.skipped.push(format!(
                    "{key}: placed in the casting file's scene entry but has no cast, asset or puppet"
                ));
            }
            Some(_) => {
                let i = ensure(&mut members, casting, set, key, 0);
                members[i].first_item = usize::MAX;
            }
        }
    }

    for (index, item) in src.items.iter().enumerate() {
        let slot = schedule.slots[index];
        match item {
            Item::Action(text) => {
                for clause in clauses(text) {
                    // Who this clause names, in the order the words come.
                    let mut found: Vec<(usize, String)> = Vec::new();
                    for (alias, key) in &names {
                        if absent.contains(&key.to_uppercase()) {
                            continue;
                        }
                        if let Some(at) = find_word(&clause, alias)
                            && !found.iter().any(|(_, k)| k == key)
                        {
                            found.push((at, key.clone()));
                        }
                    }
                    found.sort_by_key(|(at, _)| *at);
                    let subject = found.first().map(|(_, k)| k.clone());
                    let movement = movement_of(&clause);

                    for (_, key) in &found {
                        let first_time = appeared.insert(key.clone());
                        let i = ensure(&mut members, casting, set, key, index);
                        if first_time {
                            members[i].first_item = index;
                        }
                        let is_subject = subject.as_deref() == Some(key.as_str());
                        // AN10: an action line that says they stand / sit / point /
                        // turn moves them at its own time, unless a line of theirs
                        // right beside it already carries the move.
                        if is_subject && matches!(movement, Movement::None) {
                            let beside = |n: Option<usize>| {
                                n.and_then(|n| src.items.get(n)).is_some_and(|it| {
                                    matches!(it, Item::Dialogue(d) if d.voice == Voice::OnScreen
                                        && casting.character(&d.speaker).map(|(k, _)| k) == Some(key.clone()))
                                })
                            };
                            if !beside(index.checked_sub(1))
                                && !beside(Some(index + 1))
                                && let Some(cue) = move_cue(&clause)
                            {
                                members[i].moves.push(Move { start: snap_even(slot.start), cue });
                            }
                        }
                        match movement {
                            Movement::Enter if is_subject && first_time => {
                                members[i].enter_item = Some(index);
                            }
                            Movement::Exit { run } if is_subject && members[i].exit.is_none() => {
                                members[i].exit = Some((index, run));
                            }
                            _ => {}
                        }
                    }
                }
            }
            Item::Dialogue(Dialogue { speaker, voice, text }) => {
                if *voice != Voice::OnScreen {
                    continue;
                }
                match casting.character(speaker) {
                    Some((key, _)) if absent.contains(&key.to_uppercase()) => {}
                    Some((key, entry)) if !matches!(entry.kind(), Kind::VoiceOnly | Kind::Uncast) => {
                        let i = ensure(&mut members, casting, set, &key, index);
                        if appeared.insert(key.clone()) {
                            members[i].first_item = index;
                        }
                        let start = snap_even(slot.start);
                        // **Gestures**, unless the casting file turned them off
                        // for this character or this scene — which also keeps a
                        // shout at ordinary size, so off is exactly as before.
                        let gestures = entry.gestures != Some(false) && set.gestures != Some(false);
                        let cue = if gestures {
                            // The action line just before, then just after, if
                            // it is about the speaker. One between two of the
                            // same speaker's lines belongs to the later, which
                            // it leads into, so it is not used twice.
                            let next_is_theirs = matches!(
                                src.items.get(index + 2),
                                Some(Item::Dialogue(d)) if casting.character(&d.speaker).map(|(k, _)| k) == Some(key.clone())
                            );
                            let near = [index.checked_sub(1), (!next_is_theirs).then_some(index + 1)];
                            near.into_iter()
                                .flatten()
                                .find_map(|n| match src.items.get(n) {
                                    Some(Item::Action(action)) => action_cue(&clauses_about(action, &key, &names)),
                                    _ => None,
                                })
                                .or_else(|| dialogue_cue(text))
                        } else {
                            None
                        };
                        members[i].lines.push(Line {
                            start,
                            frames: (slot.end().saturating_sub(start)).max(1),
                            text: text.clone(),
                            amount: if gestures { talk_amount(text) } else { 1.0 },
                            cue,
                        });
                    }
                    Some(_) => {}
                    None => *unmatched.entry(norm(speaker)).or_insert(0) += 1,
                }
            }
            _ => {}
        }
    }

    // A place entry can force an entrance from a side at first appearance.
    for m in &mut members {
        if m.enter_item.is_none() && m.place.enter.is_some() && m.first_item != usize::MAX {
            m.enter_item = Some(m.first_item);
        }
        // Placed but never named: there from the start.
        if m.first_item == usize::MAX {
            m.first_item = 0;
        }
        // **Not on two lines in a row** unless an action line asked for the
        // second: a gesture on every line reads as fidgeting.
        let mut last_had = false;
        for line in &mut m.lines {
            if last_had && line.cue.as_ref().is_some_and(|c| !c.asked) {
                line.cue = None;
            }
            last_had = line.cue.is_some();
        }
    }

    for (name, count) in unmatched {
        report.skipped.push(format!(
            "{name}: no casting entry \u{2014} {count} line(s) captioned, not staged"
        ));
    }
    members
}

// -- the set --------------------------------------------------------------------------------

fn setting_named(name: &str) -> Option<Setting> {
    match name.trim().to_lowercase().as_str() {
        "daylight" | "day" | "morning" | "afternoon" => Some(Setting::Daylight),
        "sunset" | "evening" | "dusk" | "dawn" => Some(Setting::Sunset),
        "night" => Some(Setting::Night),
        "interior" | "inside" | "int" => Some(Setting::Interior),
        "storm" => Some(Setting::Storm),
        _ => None,
    }
}

fn scenery_named(name: &str) -> Option<Scenery> {
    match name.trim().to_lowercase().as_str() {
        "village" => Some(Scenery::Village),
        "forest" => Some(Scenery::Forest),
        "city" => Some(Scenery::City),
        "meadow" => Some(Scenery::Meadow),
        "waterside" => Some(Scenery::Waterside),
        "bare" | "none" => Some(Scenery::Bare),
        _ => None,
    }
}

/// The words a set from the user's assets is chosen by (AS3): the slugline's
/// place, the scene's title and its action lines.
fn set_text(src: &ScriptScene) -> String {
    let mut text = String::new();
    if let Some(s) = &src.slugline {
        text.push_str(&s.location);
        text.push(' ');
    }
    text.push_str(&src.title);
    for item in &src.items {
        match item {
            Item::Action(a) => {
                text.push(' ');
                text.push_str(a);
            }
            Item::Slugline(s) => {
                text.push(' ');
                text.push_str(&s.location);
            }
            _ => {}
        }
    }
    text
}

/// Where the cast will stand and the props go, `0..1` across the stage, as
/// the staging below works it out: the set keeps its ground pieces clear.
fn marks_of(members: &[Member], set: &SceneSet) -> Vec<f64> {
    let unspecified = members.iter().filter(|m| m.place.x.is_none()).count();
    let mut spread = 0usize;
    let mut out: Vec<f64> = members
        .iter()
        .map(|m| match m.place.x {
            Some(x) => x.clamp(-0.5, 1.5),
            None => {
                let x = 0.2 + 0.6 * (spread as f64 + 0.5) / unspecified.max(1) as f64;
                spread += 1;
                x
            }
        })
        .collect();
    out.extend(set.props.iter().map(|p| p.x.unwrap_or(0.5)));
    out
}

/// The setting a slugline implies.
fn setting_for(slug: &Slugline) -> Setting {
    if slug.place == SlugPlace::Interior {
        return Setting::Interior;
    }
    let time = slug.time.to_uppercase();
    if time.contains("NIGHT") {
        Setting::Night
    } else if ["EVENING", "DUSK", "SUNSET", "DAWN"].iter().any(|t| time.contains(t)) {
        Setting::Sunset
    } else {
        Setting::Daylight
    }
}

/// Which asset a name means. Case-insensitive; `folder` narrows it; the
/// Cast folder wins when `prefer_cast`.
fn find_asset<'a>(
    assets: &'a AssetLibrary,
    name: &str,
    folder: Option<&str>,
    prefer_cast: bool,
) -> Option<&'a Asset> {
    let wanted = name.trim().to_lowercase();
    let mut matches: Vec<&Asset> = assets
        .assets()
        .iter()
        .filter(|a| a.name.to_lowercase() == wanted)
        .filter(|a| {
            folder.is_none_or(|f| {
                let f = f.trim().trim_matches('/').to_lowercase();
                a.folder.to_lowercase() == f || a.folder.to_lowercase().starts_with(&format!("{f}/"))
            })
        })
        .collect();
    matches.sort_by_key(|a| {
        let in_cast = a.folder.eq_ignore_ascii_case("cast") || a.folder.to_lowercase().starts_with("cast/");
        (if prefer_cast { !in_cast } else { false }, a.folder.len(), a.folder.clone())
    });
    matches.into_iter().next()
}

/// What a placed asset brought with it.
pub(crate) struct Placed {
    /// Layers that nothing else follows: the ones to move and scale.
    pub(crate) top: Vec<LayerId>,
    /// The box round their artwork on frame zero.
    pub(crate) bounds: Option<Rect>,
    /// The first rig among them.
    pub(crate) body: Option<ObjectId>,
    /// Every layer that arrived, front first.
    pub(crate) all: Vec<LayerId>,
}

fn place_asset(scene: &mut Scene, assets: &AssetLibrary, asset: &Asset) -> Result<Placed, String> {
    let before: HashSet<LayerId> = scene.layers().iter().map(|l| l.id).collect();
    assets
        .place(asset, scene)
        .map_err(|e| format!("could not place {:?}: {e}", asset.name))?;
    Ok(arrived_since(scene, &before))
}

/// **Place a scene that is already loaded** (one symbol or one part of an
/// asset, AS4) exactly as [`place_asset`] places a whole asset.
pub(crate) fn place_scene(scene: &mut Scene, incoming: &Scene) -> Placed {
    let before: HashSet<LayerId> = scene.layers().iter().map(|l| l.id).collect();
    scene.merge(incoming, buzz_scene::ImportTarget::Stage);
    arrived_since(scene, &before)
}

/// The layers that arrived since `before`, their box and their first rig.
fn arrived_since(scene: &Scene, before: &HashSet<LayerId>) -> Placed {
    let arrived: Vec<LayerId> = scene
        .layers()
        .iter()
        .map(|l| l.id)
        .filter(|id| !before.contains(id))
        .collect();
    let arrived_set: HashSet<LayerId> = arrived.iter().copied().collect();

    let mut top = Vec::new();
    let mut bounds: Option<Rect> = None;
    let mut body = None;
    for id in &arrived {
        let Some(layer) = scene.layers().get(*id) else {
            continue;
        };
        if layer.follows.is_some_and(|f| arrived_set.contains(&f)) {
            continue;
        }
        top.push(*id);
        for object in layer.objects_at(0) {
            if !object.visible {
                continue;
            }
            if body.is_none() && matches!(object.kind, ObjectKind::Armature(_)) {
                body = Some(object.id);
            }
            let b = drawn_bounds(scene, object);
            if b.width() <= 0.0 && b.height() <= 0.0 {
                continue;
            }
            bounds = Some(bounds.map_or(b, |u| u.union(b)));
        }
        // A layer rig's root part stands for the character, and the box is
        // round every part, not only the root's.
        if body.is_none()
            && let Some(rig) = buzz_act::layer_rig::rig_of_layer(scene, *id)
        {
            body = Some(rig.body);
            for part in rig.layers().filter(|l| l != id) {
                let Some(layer) = scene.layers().get(part) else {
                    continue;
                };
                for object in layer.objects_at(0) {
                    let b = scene.resolved_bounds(object);
                    if !object.visible || (b.width() <= 0.0 && b.height() <= 0.0) {
                        continue;
                    }
                    bounds = Some(bounds.map_or(b, |u| u.union(b)));
                }
            }
        }
    }
    Placed { top, bounds, body, all: arrived }
}

/// **What an object draws**, measured for standing it on its mark (LS): an
/// armature by its visible posed parts that draw something, and nothing else.
///
/// `Scene::resolved_bounds` also takes in the bones and every empty part, and
/// an empty part (a group with nothing in it) measures as a point at the
/// armature's origin — well above and to the left of a Cast figure — so the
/// box was up to 40 % wider and a few percent taller than the drawing, and the
/// character stood off its mark. The same character as a layer rig is measured
/// by its parts' drawings (below), and the two now agree.
fn drawn_bounds(scene: &Scene, object: &Object) -> Rect {
    match &object.kind {
        ObjectKind::Armature(rig) => rig
            .posed()
            .iter()
            .filter(|part| part.visible)
            .map(|part| buzz_scene::object::transform_rect(object.transform, scene.resolved_bounds(part)))
            .filter(|b| !(b.width() <= 0.0 && b.height() <= 0.0))
            .reduce(|a, b| a.union(b))
            .unwrap_or_else(|| scene.resolved_bounds(object)),
        _ => scene.resolved_bounds(object),
    }
}

/// Apply `t` to everything on the layers, on every keyframe.
pub(crate) fn transform_layers(scene: &mut Scene, layers: &[LayerId], t: Affine) {
    for id in layers {
        scene.update_layer(*id, |layer| {
            for keyframe in layer.frames.keyframes_mut() {
                let objects = Arc::make_mut(&mut keyframe.objects);
                for object in objects.iter_mut() {
                    let o = Arc::make_mut(object);
                    o.transform = t * o.transform;
                }
            }
        });
    }
}

/// Scale so the box is `height` tall, with the middle of its feet at `(x, y)`.
pub(crate) fn stand_on(bounds: Rect, height: f64, x: f64, y: f64) -> (Affine, f64) {
    let scale = height / bounds.height().max(1e-6);
    let t = Affine::translate((x, y))
        * Affine::scale(scale)
        * Affine::translate((-bounds.center().x, -bounds.y1));
    (t, scale)
}

/// Scale so the box covers the stage, centred.
pub(crate) fn cover(bounds: Rect, stage: Rect) -> Affine {
    let scale = (stage.width() / bounds.width().max(1e-6)).max(stage.height() / bounds.height().max(1e-6));
    Affine::translate((stage.center().x, stage.center().y))
        * Affine::scale(scale)
        * Affine::translate((-bounds.center().x, -bounds.center().y))
}

fn mirror_about(x: f64) -> Affine {
    Affine::translate((x, 0.0)) * Affine::scale_non_uniform(-1.0, 1.0) * Affine::translate((-x, 0.0))
}

fn hex_colour(text: &str) -> Option<Color> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some(Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

// -- an actor on the set ------------------------------------------------------------------

/// Where a character's mouth is written.
#[derive(Debug, Clone, PartialEq)]
enum Mouth {
    None,
    /// A part of the armature.
    Rig(ObjectId),
    /// A part of a layer rig, on a layer of its own.
    RigLayer(ObjectId),
    /// A layer of its own, with the symbol to show and where.
    Layer {
        layer: LayerId,
        symbol: buzz_scene::SymbolId,
        placement: Affine,
    },
}

struct Actor {
    member: Member,
    played_by: String,
    body: Option<ObjectId>,
    mouth: Mouth,
    /// The body's transform for the next beat.
    placed: Affine,
    /// World x of the middle of the feet right now.
    x: f64,
    mark_x: f64,
    /// Which way the drawing looks (+1 right, -1 left, 0 front) and which way it
    /// faces now.
    drawn: f64,
    facing: f64,
    height: f64,
    half_width: f64,
    offstage: bool,
    /// The eye parts of a rig, each with its open, half-shut and shut symbols.
    eyes: Vec<(usize, [buzz_scene::SymbolId; 3])>,
    /// The eye parts of a layer rig, by object: the same, on their own layers.
    layer_eyes: Vec<(ObjectId, [buzz_scene::SymbolId; 3])>,
    /// Blinking comes with the character (a puppet's eyes carry the modifier).
    blinks: bool,
    /// AN11: the layers to move and scale (empty for a puppet), and every layer
    /// the asset brought.
    top: Vec<LayerId>,
    layers: Vec<LayerId>,
    /// AN11: a pose-variant instance; shown only in `pose_spans`, while the
    /// base instance of the same character is hidden.
    variant: bool,
    pose_spans: Vec<(u32, u32)>,
}

impl Actor {
    fn face(&mut self, direction: f64) {
        if self.drawn != 0.0 && direction != 0.0 && direction.signum() != self.facing {
            self.placed = mirror_about(self.x) * self.placed;
            self.facing = direction.signum();
        }
    }
}

#[derive(Debug, Clone)]
enum SegKind {
    Enter { run: bool, tempo: f64 },
    Exit { run: bool, tempo: f64 },
    Talk { amount: f64 },
    /// A one-shot inside a line. Written after the talk it sits in, over it:
    /// its keys replace the talk's inside its range and the talk's keys either
    /// side stay, so the line goes talk, gesture, talk.
    Gesture { action: Action, amount: f64 },
}

#[derive(Debug, Clone)]
struct Seg {
    range: Range<u32>,
    kind: SegKind,
}

fn walk_frames(distance: f64, speed: f64, fps: f64) -> u32 {
    ((distance.abs() / speed.max(1.0) * fps).round() as u32).max(8)
}

/// How a walk is paced.
struct Pace {
    frames: u32,
    run: bool,
    tempo: f64,
}

/// **How long a crossing takes, and whether it is a walk.**
///
/// As long as the action line, or as long as the distance takes at a walking
/// pace if that is longer, but never past the time there is: a character has to
/// be on their mark before they speak and gone before the scene ends. Short on
/// time, the walk gets brisker, and past a point it is a run.
fn pace(
    distance: f64,
    slot_frames: u32,
    available: u32,
    wants_run: bool,
    walk_speed: f64,
    run_speed: f64,
    fps: f64,
) -> Pace {
    let natural_walk = walk_frames(distance, walk_speed, fps);
    let natural_run = walk_frames(distance, run_speed, fps);
    let natural = if wants_run { natural_run } else { natural_walk };
    let frames = slot_frames.max(natural).min(available.max(8));
    let run = wants_run || (frames as f64) < natural_walk as f64 * 0.67;
    let base = if run { natural_run } else { natural_walk };
    Pace {
        frames,
        run,
        tempo: (base as f64 / frames as f64).clamp(1.0, 1.6),
    }
}

fn overlaps(a: &Range<u32>, b: &Range<u32>) -> bool {
    a.start < b.end && b.start < a.end
}

// -- captions ---------------------------------------------------------------------------------

struct Caption {
    start: u32,
    end: u32,
    text: String,
    italic: bool,
    speaker: Option<String>,
}

/// Break `text` into lines no wider than `max_width`, and as even as that
/// allows: a caption that has to take two lines is two halves, not a long line
/// and one stranded word.
fn wrap(text: &str, size: f64, style: buzz_text::FontStyle, max_width: f64) -> String {
    let greedy = |limit: f64| -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        for word in text.split_whitespace() {
            let trial = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if !current.is_empty() && buzz_text::measure_styled(&trial, size, None, style).0 > limit {
                lines.push(std::mem::take(&mut current));
                current = word.to_string();
            } else {
                current = trial;
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        lines
    };

    let lines = greedy(max_width);
    if lines.len() <= 1 {
        return lines.join("
");
    }
    // The narrowest limit that still takes the same number of lines.
    let wanted = lines.len();
    let (mut low, mut high) = (0.0f64, max_width);
    for _ in 0..14 {
        let mid = (low + high) / 2.0;
        if greedy(mid).len() <= wanted {
            high = mid;
        } else {
            low = mid;
        }
    }
    greedy(high).join("
")
}

/// Write the captions onto a `Subtitles` layer. Returns how many were written.
fn write_captions(scene: &mut Scene, captions: &[Caption], total: u32, warnings: &mut Vec<String>) -> usize {
    if captions.is_empty() {
        return 0;
    }
    let stage = scene.stage().stage_rect();
    let size = (stage.height() / 20.0).max(8.0);
    let baseline = stage.y1 - stage.height() * 0.09;
    let max_width = stage.width() * 0.84;
    let ink = Color::WHITE;
    let edge = Color::from_rgb8(0x10, 0x10, 0x16);

    struct Drawn {
        range: Range<u32>,
        label: Option<String>,
        path: buzz_geom::BezPath,
        data: TextData,
        width: f64,
        rows: usize,
        row_height: f64,
        ids: [ObjectId; 2],
    }

    let mut drawn: Vec<Drawn> = Vec::new();
    for c in captions {
        let style = buzz_text::FontStyle::new(false, c.italic);
        let body = match &c.speaker {
            Some(who) => format!("{who}: {}", c.text),
            None => c.text.clone(),
        };
        let text = wrap(&body, size, style, max_width);
        let Some(path) = buzz_text::outline_styled(&text, size, None, style, buzz_text::TextAlign::Centre) else {
            warnings.push("no font available: captions were not drawn".to_string());
            return 0;
        };
        let (width, height) = buzz_text::measure_styled(&text, size, None, style);
        let rows = text.lines().count().max(1);
        drawn.push(Drawn {
            range: c.start..c.end,
            label: c.speaker.clone(),
            path,
            data: TextData {
                content: body,
                size,
                font: None,
                style,
                align: buzz_text::TextAlign::Centre,
                runs: Vec::new(),
            },
            width,
            rows,
            row_height: height / rows as f64,
            ids: [ObjectId(0), ObjectId(0)],
        });
    }
    for d in &mut drawn {
        d.ids = [scene.next_object_id(), scene.next_object_id()];
    }

    let layer = scene.add_stage_layer("Subtitles", buzz_scene::LayerKind::Normal);
    let count = drawn.len();
    scene.update_layer(layer, |l| {
        // Captions sit on the screen, not in the world: the lights must not
        // shade them or throw a copy of them onto the grass (F1).
        l.unlit = true;
        while l.frames.length() < total.max(1) {
            l.frames.insert_frame(l.frames.length());
        }
        for d in &drawn {
            let start = d.range.start.min(l.frames.length().saturating_sub(1));
            let end = d.range.end.min(l.frames.length());
            // Up from the baseline by the rows above the last, so the block's
            // bottom edge is where a single line's would be.
            let top = baseline - d.row_height * (d.rows.saturating_sub(1)) as f64;
            let at = Affine::translate((stage.center().x - d.width / 2.0, top));

            let mut outline = ShapeData::filled(d.path.clone(), edge);
            outline.stroke = Some(StrokeSpec::new(edge, size * 0.2));
            let mut shadow = Object::shape(d.ids[0], outline);
            shadow.name = Some("Caption outline".into());
            shadow.transform = at;
            let mut words = Object::shape(d.ids[1], ShapeData::filled(d.path.clone(), ink));
            words.name = Some("Caption".into());
            words.transform = at;
            words.text = Some(d.data.clone());

            l.frames.insert_blank_keyframe(start);
            l.frames
                .set_objects(start, vec![Arc::new(shadow), Arc::new(words)]);
            l.frames.set_label(start, d.label.clone());
            if end < l.frames.length() && !l.frames.is_keyframe(end) {
                l.frames.insert_blank_keyframe(end);
            }
        }
    });
    count
}

// -- building a scene ----------------------------------------------------------------------

/// What carries from one scene to the next: a scene with no slugline of its own
/// is the same place as the one before it.
#[derive(Debug, Clone, Default)]
pub struct Carry {
    setting: Option<Setting>,
}

/// **Build one script scene into `scene`.**
///
/// The caller wraps it in one `Document::edit`, so a whole scene is one undo
/// step.
pub fn build_scene(
    scene: &mut Scene,
    src: &ScriptScene,
    casting: &Casting,
    assets: &AssetLibrary,
    opts: &BuildOptions,
    carry: &mut Carry,
) -> SceneReport {
    let timing = Timing {
        fps: opts.fps,
        ..opts.timing
    };
    // P2: a recording with lines for this scene times them; text times the rest.
    let mut audio_notes: Vec<String> = Vec::new();
    let audio = opts
        .audio
        .as_deref()
        .and_then(|plan| scene_audio(plan, &timing, src, &mut audio_notes));
    let schedule = match &audio {
        Some(a) => a.timed.schedule.clone(),
        None => timing.schedule(src),
    };
    let total = schedule.total_frames;
    let fps = opts.fps;
    let stage = scene.stage().stage_rect();
    let set = casting.scene_set(src.number);
    // AN11: where each dialogue line starts, and each shot's angle over them.
    let line_starts: Vec<u32> = src
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i, Item::Dialogue(_)))
        .filter_map(|(ix, _)| schedule.slots.get(ix).map(|s| s.start))
        .collect();
    let mut segments = angle_segments(&set, &line_starts, total);
    if set.background.is_empty() {
        for s in &mut segments {
            s.background.clear();
        }
    }
    let mut angle_gates: Vec<(Vec<LayerId>, Vec<(u32, u32)>)> = Vec::new();

    let mut report = SceneReport {
        number: src.number,
        title: src.title.clone(),
        frames: total,
        seconds: total as f64 / fps.max(1e-6),
        lines: src
            .items
            .iter()
            .filter(|i| matches!(i, Item::Dialogue(_)))
            .count(),
        notes: audio_notes,
        ..Default::default()
    };

    let mut members = roster(src, &schedule, casting, &set, &mut report);
    // The order they first appear is the order they are spread across the stage.
    members.sort_by_key(|m| (m.first_item, m.key.clone()));

    let ground = set.ground.unwrap_or(opts.ground).clamp(0.3, 1.0);

    // -- the set ---------------------------------------------------------------------
    let mut front_props: Vec<&PropSpec> = Vec::new();
    if !set.background.is_empty() {
        let mut used = Vec::new();
        let mut base_layers: Vec<LayerId> = Vec::new();
        for name in &set.background {
            // `Backgrounds/<series>/<key>` (the producer's) names its folder too.
            let found = match name.trim().trim_matches('/').rsplit_once('/') {
                Some((folder, base)) => find_asset(assets, base, Some(folder), false),
                None => find_asset(assets, name, None, false),
            };
            match found {
                Some(asset) => match place_asset(scene, assets, asset) {
                    Ok(placed) => {
                        if let Some(b) = placed.bounds {
                            transform_layers(scene, &placed.top, cover(b, stage));
                        }
                        used.push(name.clone());
                        base_layers.extend(placed.all.iter().copied());
                    }
                    Err(e) => report.warnings.push(e),
                },
                None => report
                    .skipped
                    .push(format!("background {name:?}: no such asset in the library")),
            }
        }
        // AN11: each other angle's picture on layers of its own, shown only in
        // its shots; the wide one is hidden in those shots.
        let base_name = set.background[0].clone();
        let mut base_hidden: Vec<(u32, u32)> = Vec::new();
        let mut others: Vec<String> = Vec::new();
        for s in &segments {
            if !s.background.eq_ignore_ascii_case(&base_name) && !others.iter().any(|o| o.eq_ignore_ascii_case(&s.background)) {
                others.push(s.background.clone());
            }
        }
        for other in others {
            let spans: Vec<(u32, u32)> = segments
                .iter()
                .filter(|s| s.background.eq_ignore_ascii_case(&other))
                .map(|s| (s.start, s.end))
                .collect();
            match place_cover(scene, assets, &other, stage) {
                Ok(layers) => {
                    base_hidden.extend(spans.iter().copied());
                    angle_gates.push((layers, complement(&spans, total)));
                }
                Err(e) => {
                    report.warnings.push(format!("{e}; the wide background is kept for those shots"));
                    for s in segments.iter_mut().filter(|s| s.background.eq_ignore_ascii_case(&other)) {
                        s.background = base_name.clone();
                    }
                }
            }
        }
        if !base_hidden.is_empty() {
            angle_gates.push((base_layers, base_hidden));
        }
        report.set = format!("background: {}", used.join(", "));
        // P8-C1: the breakdown's props from the index, on the ground in front
        // of the picture, clear of the cast's marks. No index: none.
        if !used.is_empty()
            && !set.props_wanted.is_empty()
            && let Some(index) = opts.index.as_deref()
        {
            // One layout per location: the words and the cast's marks of
            // every scene at it, seeded by its key.
            let key_of = |s: &SceneSet| {
                s.location
                    .clone()
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or_else(|| s.background.join("|"))
            };
            let key = key_of(&set);
            let mut numbered: Vec<(u32, &SceneSet)> = casting
                .scenes
                .iter()
                .filter(|(_, s)| key_of(s) == key)
                .map(|(n, s)| (n.parse().unwrap_or(u32::MAX), s))
                .collect();
            numbered.sort_by_key(|(n, _)| *n);
            let mut words: Vec<String> = Vec::new();
            let mut marks: Vec<f64> = Vec::new();
            for (_, s) in &numbered {
                for w in &s.props_wanted {
                    if !words.iter().any(|x| x.eq_ignore_ascii_case(w)) {
                        words.push(w.clone());
                    }
                }
                marks.extend(s.place.values().filter_map(|p| p.x).map(|x| x.clamp(-0.5, 1.5)));
            }
            if numbered.is_empty() {
                words = set.props_wanted.clone();
                marks = marks_of(&members, &set);
            }
            let dressed = crate::asset_sets::dress_props(
                scene,
                assets,
                index,
                &key,
                &words,
                &marks,
                ground,
                opts.figure_height,
            );
            report.set_pieces.extend(dressed.placed.iter().cloned());
            report.notes.extend(dressed.plan.notes.iter().cloned());
            for f in &dressed.failed {
                report.warnings.push(format!("prop not placed: {f}"));
            }
        }
        // P6: a background asset brings no lights, so the casting's lighting
        // gets the key and the sky fill a built set would have made first.
        // The asset's cover layers (its Sky) stay unlit as they were saved.
        if let Some(lighting) = &set.lighting {
            if set.lit.unwrap_or(true) {
                let setting = set
                    .setting
                    .as_deref()
                    .and_then(setting_named)
                    .or_else(|| src.slugline.as_ref().map(setting_for))
                    .or(carry.setting)
                    .unwrap_or(Setting::Daylight);
                let lights_before = scene.lights().lights.len();
                add_set_lights(scene, setting);
                apply_lighting(scene, lighting, setting, stage, lights_before, &mut report);
                carry.setting = Some(setting);
            } else {
                report.warnings.push("lighting ignored: the scene is not lit".into());
            }
        }
    } else {
        let setting = set
            .setting
            .as_deref()
            .and_then(|s| {
                let parsed = setting_named(s);
                if parsed.is_none() {
                    report.warnings.push(format!("unknown setting {s:?}; using the slugline's"));
                }
                parsed
            })
            .or_else(|| src.slugline.as_ref().map(setting_for))
            .or(carry.setting)
            .unwrap_or(Setting::Daylight);
        let horizon = set.horizon.unwrap_or(0.6).clamp(0.15, 0.95);
        let recipe = SceneRecipe {
            setting,
            cast: 0,
            horizon,
            figure_scale: opts.figure_height,
            lit: set.lit.unwrap_or(true),
            frames: total,
            ..SceneRecipe::default()
        };
        let lights_before = scene.lights().lights.len();
        let staged = staging::build(scene, &recipe);
        // P5: the casting's key light, over the lights the setting just made.
        if let Some(lighting) = &set.lighting {
            if recipe.lit {
                apply_lighting(scene, lighting, setting, stage, lights_before, &mut report);
            } else {
                report.warnings.push("lighting ignored: the scene is not lit".into());
            }
        }
        let horizon_y = stage.y0 + stage.height() * horizon;

        // **AS3: the set from the user's own assets**, when the casting names
        // no scenery (or says "assets") and the library has an index.
        let scenery_word = set.scenery.as_deref().map(|s| s.trim().to_lowercase());
        let wants_assets = scenery_word.as_deref() == Some("assets");
        let mut dressed_set = false;
        let mut backdrop = staged.backdrop;
        if scenery_word.is_none() || wants_assets {
            match opts.index.as_deref() {
                Some(index) => {
                    let query = buzz_ai::asset_sets::SetQuery {
                        text: set_text(src),
                        interior: setting == Setting::Interior,
                        time: match setting {
                            Setting::Night => buzz_ai::asset_sets::TimeOfDay::Night,
                            Setting::Sunset => buzz_ai::asset_sets::TimeOfDay::Evening,
                            _ => buzz_ai::asset_sets::TimeOfDay::Day,
                        },
                        style: None,
                        seed: buzz_ai::asset_sets::seed_of(&format!(
                            "{} {}",
                            src.number,
                            src.slugline.as_ref().map(|s| s.raw.as_str()).unwrap_or(&src.title)
                        )),
                        marks: marks_of(&members, &set),
                        horizon,
                        ground,
                    };
                    let frame = crate::asset_sets::SetFrame { backdrop: staged.backdrop, ground: staged.ground };
                    let dressed = crate::asset_sets::dress_set(scene, assets, index, &query, &frame);
                    report.set_pieces = dressed.placed.clone();
                    for f in &dressed.failed {
                        report.warnings.push(format!("set piece not placed: {f}"));
                    }
                    if dressed.needs_procedural() {
                        report.warnings.push(format!(
                            "set from your assets: nothing matched ({}); procedural scenery laid",
                            dressed.plan.notes.join("; ")
                        ));
                        // Laid in front of a sky that was placed, not behind it.
                        if let Some(sky) = dressed.sky_front {
                            backdrop = Some(sky);
                        }
                    } else {
                        dressed_set = true;
                        report.set = format!(
                            "{} set from your assets, horizon {:.2} \u{2014} {}",
                            setting.label(),
                            horizon,
                            dressed.summary()
                        );
                    }
                }
                None if wants_assets => report
                    .warnings
                    .push("\"scenery\": \"assets\" but the library has no asset index; procedural scenery".into()),
                None => {}
            }
        }
        if !dressed_set {
            let kind = if wants_assets {
                Scenery::Village
            } else {
                set.scenery
                    .as_deref()
                    .and_then(|s| {
                        let parsed = scenery_named(s);
                        if parsed.is_none() {
                            report.warnings.push(format!("unknown scenery {s:?}; using village"));
                        }
                        parsed
                    })
                    .unwrap_or(Scenery::Village)
            };
            let laid = scenery::lay(scene, kind, horizon_y, backdrop);
            report.set = format!(
                "{} set, {} scenery ({} pieces), horizon {:.2}",
                setting.label(),
                format!("{kind:?}").to_lowercase(),
                laid.pieces,
                horizon
            );
        }
        carry.setting = Some(setting);
    }

    for prop in &set.props {
        if prop.front {
            front_props.push(prop);
            continue;
        }
        place_prop(scene, assets, prop, stage, ground, opts, &mut report);
    }

    // -- the cast -----------------------------------------------------------------------
    let unspecified = members.iter().filter(|m| m.place.x.is_none()).count();
    let mut spread_index = 0usize;
    let mut actors: Vec<Actor> = Vec::new();
    let mut variants: Vec<Actor> = Vec::new();
    // P5: each staged actor's feet, for framing a shot on them.
    let mut feet: Vec<(String, f64)> = Vec::new();
    for mut member in members {
        // AN13: a tagged pose with no pose picture falls back to the rig's own Sit
        // (held until a Stand), else stays standing and says so under Skipped.
        let tagged_pose = member.place.pose.clone().filter(|p| {
            !p.trim().is_empty()
                && !member.place.cast.as_deref().is_some_and(|c| crate::cast_build::pose_of_rig(c).is_some())
        });
        let mut tagged_note: Option<String> = None;
        if let Some(pose) = &tagged_pose {
            let name = member.entry.cast.clone().unwrap_or_else(|| member.key.clone());
            let word = pose.replace('_', " ");
            if pose_stem_of(pose) == "sit" {
                member.moves.insert(
                    0,
                    Move { start: 0, cue: Cue { action: Action::Sit, amount: 1.0, why: format!("tagged {word}"), asked: true } },
                );
                tagged_note = Some(format!("{}: no {word} picture for {name}; the rig's own Sit is held", member.key));
            } else {
                tagged_note = Some(format!("{}: no {word} picture for {name}; standing", member.key));
                report.skipped.push(format!("{}: no {word} picture for {name}", member.key));
            }
        }
        let x_fraction = match member.place.x {
            Some(x) => x.clamp(-0.5, 1.5),
            None => {
                let x = 0.2 + 0.6 * (spread_index as f64 + 0.5) / unspecified.max(1) as f64;
                spread_index += 1;
                x
            }
        };
        let mark_x = stage.x0 + stage.width() * x_fraction;
        let feet_y = stage.y0 + stage.height() * member.place.y.unwrap_or(ground);
        let height_fraction = member
            .place
            .h
            .or(member.entry.h)
            .unwrap_or(opts.figure_height)
            .clamp(0.05, 1.5);
        let height = stage.height() * height_fraction;
        let key = member.key.clone();
        let member_copy = member.clone();
        if let Some(actor) = stage_actor(
            scene, assets, member, mark_x, feet_y, height, stage, total, &mut report,
        ) {
            let performs = actor.body.is_some();
            if let Some(n) = tagged_note.take() {
                if performs {
                    if n.contains("Sit is held") {
                        report.notes.push(n);
                    }
                } else if n.contains("Sit is held") {
                    report.skipped.push(format!("{}: no sitting picture and the figure has no rig to sit", key));
                }
            }
            actors.push(actor);
            feet.push((key.clone(), feet_y));
            // AN11: pose variants, shown in their beats only.
            if performs {
                for (rig, spans) in pose_swaps(src, &schedule, casting, assets, &member_copy, total, timing.frames(timing.line_gap)) {
                    let mut variant_member = member_copy.clone();
                    variant_member.place.cast = Some(rig.clone());
                    let mut scratch = SceneReport::default();
                    match stage_actor(scene, assets, variant_member, mark_x, feet_y, height, stage, total, &mut scratch) {
                        Some(mut v) if v.body.is_some() => {
                            v.variant = true;
                            v.pose_spans = spans;
                            variants.push(v);
                        }
                        _ => report.skipped.push(format!("{key}: pose rig {rig:?} could not be staged")),
                    }
                }
            }
        }
    }

    // -- the performances -----------------------------------------------------------------
    for actor in &mut actors {
        perform_actor(scene, actor, src, &schedule, stage, &timing, total, &mut report);
    }
    // AN11: a pose variant plays the same performance; its notes are the base's.
    let mut variant_report = SceneReport::default();
    for actor in &mut variants {
        perform_actor(scene, actor, src, &schedule, stage, &timing, total, &mut variant_report);
    }

    // -- the blinks, then the mouths ------------------------------------------------------
    //
    // In that order: each pass keys every frame the rig has, so the one that
    // runs second carries the first's work on the keys it adds.
    for actor in actors.iter_mut().chain(variants.iter_mut()) {
        // AN10: `still_extras` off: only speakers blink.
        if !actor.member.extras && actor.member.lines.is_empty() {
            continue;
        }
        if actor.body.is_some() && !actor.eyes.is_empty() {
            blink_rig(scene, actor, total, fps);
        }
        if actor.body.is_some() && !actor.layer_eyes.is_empty() {
            blink_layer_rig(scene, actor, total, fps);
        }
    }
    for actor in &mut actors {
        sync_mouth(scene, actor, total, fps, audio.as_ref(), &mut report);
    }
    for actor in &mut variants {
        sync_mouth(scene, actor, total, fps, audio.as_ref(), &mut variant_report);
    }
    if let Some(a) = &audio {
        attach_scene_sound(scene, a, src, fps, &mut report);
    }

    for prop in front_props {
        place_prop(scene, assets, prop, stage, ground, opts, &mut report);
    }

    // -- captions -----------------------------------------------------------------------------
    let gap = timing.frames(timing.line_gap);
    let mut captions: Vec<Caption> = Vec::new();
    for (index, item) in src.items.iter().enumerate() {
        if !opts.captions {
            break;
        }
        let Item::Dialogue(Dialogue { speaker, voice, text }) = item else {
            continue;
        };
        let slot = schedule.slots[index];
        let voice_only = casting
            .character(speaker)
            .is_some_and(|(_, e)| e.voice_only);
        let over = *voice == Voice::VoiceOver || voice_only;
        captions.push(Caption {
            start: slot.start,
            end: (slot.end() + gap).min(total),
            text: text.clone(),
            italic: over,
            speaker: if over { None } else { Some(norm(speaker)) },
        });
    }
    report.captions = write_captions(scene, &captions, total, &mut report.warnings);

    // -- title card ---------------------------------------------------------------------------
    if casting.title_cards {
        let words = set.title.clone().unwrap_or_else(|| src.title.clone());
        if !words.trim().is_empty() {
            let mut spec = buzz_act::title::TitleSpec::new(words.trim());
            spec.preset = buzz_act::title::TitlePreset::FadeUp;
            spec.size = (stage.height() / 12.0).max(8.0);
            spec.start = 0;
            // In the sky rather than over the faces, on a dark plate so white
            // words read against a pale sky.
            spec.position = buzz_act::title::TitlePosition::Top;
            spec.plate = Some(buzz_act::title::TitlePlate {
                color: Color::from_rgb8(0x14, 0x14, 0x1C),
                opacity: 0.6,
            });
            spec.total_frames = Some(((2.0 * fps).round() as u32).min(total.max(1)));
            if let Err(e) = buzz_act::title::add_title_card(scene, &spec) {
                report.warnings.push(format!("title card not added: {e}"));
            }
        }
    }

    // -- shots (P5) ---------------------------------------------------------------------------
    if let Some(shots) = &set.shots {
        let cast = ShotCast { actors: &actors, feet: &feet };
        let mut plan = plan_shots(shots, src, &schedule, casting, &cast, stage, &timing, total, &mut report);
        // AN13: a shot inside an angle picture keeps that picture's full frame.
        apply_angle_framing(&mut plan, &segments, stage, total);
        write_shots(scene, &plan, total, &mut report);
    }

    // -- AN11: each shot's angle: pictures cut at the shot, the cast re-staged, poses swapped --
    for (layers, hidden) in &angle_gates {
        hide_during(scene, layers, hidden, total);
    }
    restage_angles(scene, &segments, &actors, &variants, &feet, src, casting, stage, set.wide_horizon, total, &mut report);
    for v in &variants {
        if let Some(base) = actors.iter().find(|a| a.member.key == v.member.key) {
            hide_during(scene, &base.layers, &v.pose_spans, total);
        }
        hide_during(scene, &v.layers, &complement(&v.pose_spans, total), total);
    }

    // Every layer reaches the end, so nothing vanishes mid-shot.
    scene.set_frame_count(total);
    let last = total.saturating_sub(1);
    let layers: Vec<LayerId> = scene.layers().iter().map(|l| l.id).collect();
    for layer in layers {
        scene.update_layer(layer, |l| {
            if l.frames.length() <= last {
                l.frames.insert_frame(last);
            }
        });
    }

    for a in &actors {
        report.cast.push(CastNote {
            name: a.member.key.clone(),
            played_by: a.played_by.clone(),
            lines: a.member.lines.len(),
            performs: a.body.is_some(),
            lip_synced: a.mouth != Mouth::None && !a.member.lines.is_empty(),
            blinks: a.blinks,
            enters: a.member.enter_item.is_some(),
            exits: a.member.exit.is_some(),
            body: a.body,
        });
    }
    report
}

// -- angle cuts, re-staging and pose swaps (AN11) -----------------------------------------

/// One stretch of a scene shot from one camera angle.
#[derive(Debug, Clone, PartialEq)]
struct AngleSeg {
    start: u32,
    end: u32,
    at_line: usize,
    camera: String,
    /// The background asset shown: the angle's own picture, else the scene's wide one.
    background: String,
    horizon: Option<f64>,
}

/// The scene's angles in frames. wide / medium / close keep the wide
/// background; the others use their own picture when the casting gave one.
fn angle_segments(set: &SceneSet, line_starts: &[u32], total: u32) -> Vec<AngleSeg> {
    if set.shot_backgrounds.is_empty() || total == 0 {
        return Vec::new();
    }
    let base = set.background.first().cloned().unwrap_or_default();
    let mut list: Vec<&ShotBackground> = set.shot_backgrounds.iter().collect();
    list.sort_by_key(|s| s.at_line);
    let mut segs: Vec<AngleSeg> = Vec::new();
    for s in list {
        let start = if s.at_line == 0 {
            0
        } else {
            match line_starts.get(s.at_line) {
                Some(&f) => f.min(total - 1),
                None => continue,
            }
        };
        let camera = s.camera.trim().to_lowercase();
        let own = !matches!(camera.as_str(), "" | "wide" | "medium" | "close");
        let background = if own && !s.background.trim().is_empty() { s.background.clone() } else { base.clone() };
        if segs.last().is_some_and(|l| l.start == start) {
            segs.pop();
        }
        segs.push(AngleSeg {
            start,
            end: total,
            at_line: s.at_line,
            camera: if camera.is_empty() { "wide".into() } else { camera },
            background,
            horizon: if own { s.horizon_row } else { None },
        });
    }
    if segs.first().is_none_or(|s| s.start > 0) {
        segs.insert(0, AngleSeg { start: 0, end: total, at_line: 0, camera: "wide".into(), background: base, horizon: None });
    }
    for i in 0..segs.len() {
        segs[i].end = segs.get(i + 1).map_or(total, |n| n.start);
    }
    segs.retain(|s| s.end > s.start);
    segs
}

/// **AN13: an angle picture is framed whole.** Inside a reverse / over-the-shoulder /
/// low / high segment the camera sits at x1.0 on the stage centre (no extra
/// zoom on a soft picture, the listener's shoulder stays in frame); the shot
/// that was in force resumes where the segment ends. One shot list: the
/// report's Camera and Angles sections both read this plan.
fn apply_angle_framing(plan: &mut Vec<Shot>, segments: &[AngleSeg], stage: Rect, total: u32) {
    for seg in segments {
        if !matches!(seg.camera.as_str(), "reverse" | "over_shoulder" | "low_angle" | "high_angle") {
            continue;
        }
        let resume = plan.iter().rev().find(|s| s.start < seg.end).cloned();
        plan.retain(|s| !(s.start >= seg.start && s.start < seg.end));
        if seg.end < total && !plan.iter().any(|s| s.start == seg.end) {
            if let Some(mut r) = resume {
                r.start = seg.end;
                plan.push(r);
            }
        }
        plan.push(Shot { start: seg.start, kind: seg.camera.clone(), on: None, center: stage.center(), zoom: 1.0, push: false });
        plan.sort_by_key(|s| s.start);
    }
}

/// How far down the frame (pixels) the cast's ground line moves for a low or
/// high angle: the angle picture's horizon against the wide one's, else a default.
fn angle_dy(seg: &AngleSeg, wide_horizon: Option<f64>, stage: Rect) -> f64 {
    let h = stage.height();
    let valid = |x: f64| x > 0.0 && x < 1.0;
    match seg.camera.as_str() {
        "low_angle" | "high_angle" => match (seg.horizon, wide_horizon) {
            (Some(a), Some(w)) if valid(a) && valid(w) => ((a - w) * h).clamp(-0.2 * h, 0.2 * h),
            _ if seg.camera == "low_angle" => 0.05 * h,
            _ => -0.08 * h,
        },
        _ => 0.0,
    }
}

/// The frames of `0..total` that `spans` do not cover.
fn complement(spans: &[(u32, u32)], total: u32) -> Vec<(u32, u32)> {
    let mut sorted: Vec<(u32, u32)> = spans.iter().copied().filter(|(a, b)| a < b).collect();
    sorted.sort();
    let mut out = Vec::new();
    let mut at = 0u32;
    for (a, b) in sorted {
        if a > at {
            out.push((at, a.min(total)));
        }
        at = at.max(b);
    }
    if at < total {
        out.push((at, total));
    }
    out.retain(|(a, b)| a < b);
    out
}

/// A background asset on layers of its own, covering the stage.
fn place_cover(scene: &mut Scene, assets: &AssetLibrary, name: &str, stage: Rect) -> Result<Vec<LayerId>, String> {
    let found = match name.trim().trim_matches('/').rsplit_once('/') {
        Some((folder, base)) => find_asset(assets, base, Some(folder), false),
        None => find_asset(assets, name, None, false),
    };
    let asset = found.ok_or_else(|| format!("background {name:?}: no such asset in the library"))?;
    let placed = place_asset(scene, assets, asset)?;
    if let Some(b) = placed.bounds {
        transform_layers(scene, &placed.top, cover(b, stage));
    }
    Ok(placed.all)
}

/// Draw nothing on `layers` over `ranges`: keys at each edge, blank inside.
fn hide_during(scene: &mut Scene, layers: &[LayerId], ranges: &[(u32, u32)], total: u32) {
    let ranges: Vec<(u32, u32)> = ranges.iter().map(|&(a, b)| (a, b.min(total))).filter(|(a, b)| a < b).collect();
    if ranges.is_empty() || total == 0 {
        return;
    }
    for id in layers {
        scene.update_layer(*id, |l| {
            if l.frames.length() < total {
                l.frames.insert_frame(total - 1);
            }
            for &(a, b) in &ranges {
                l.frames.insert_keyframe(a);
                if b < total {
                    l.frames.insert_keyframe(b);
                }
            }
            let starts: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).collect();
            for s in starts {
                if ranges.iter().any(|&(a, b)| s >= a && s < b) {
                    l.frames.set_objects(s, Vec::new());
                }
            }
        });
    }
}

/// Apply `t` to everything on `layers` over `a..b` (keys at both edges).
fn transform_during(scene: &mut Scene, layers: &[LayerId], a: u32, b: u32, t: Affine, total: u32) {
    let b = b.min(total);
    if a >= b {
        return;
    }
    for id in layers {
        scene.update_layer(*id, |l| {
            if l.frames.length() < total {
                l.frames.insert_frame(total - 1);
            }
            l.frames.insert_keyframe(a);
            if b < total {
                l.frames.insert_keyframe(b);
            }
            for k in l.frames.keyframes_mut() {
                if k.start >= a && k.start < b {
                    for object in Arc::make_mut(&mut k.objects).iter_mut() {
                        let o = Arc::make_mut(object);
                        o.transform = t * o.transform;
                    }
                }
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    Speaker,
    Listener,
    Other,
}

/// **Re-stage one character for an angle.** `reverse` mirrors them about the
/// stage centre (position and facing); `over_shoulder` puts the listener large at
/// the frame edge (`side` -1 left / +1 right) and the speaker at mid-depth on the
/// other side; `low_angle` / `high_angle` move the ground line by `dy`.
fn angle_restage(camera: &str, role: Role, side: f64, stage: Rect, mark_x: f64, feet_y: f64, dy: f64) -> Option<Affine> {
    let (w, h) = (stage.width(), stage.height());
    match camera {
        "reverse" => Some(mirror_about(stage.center().x)),
        "over_shoulder" => {
            let (target_x, target_y, scale) = match role {
                Role::Listener => {
                    let edge = if side < 0.0 { stage.x0 + 0.12 * w } else { stage.x1 - 0.12 * w };
                    (edge, feet_y + 0.06 * h, 1.35)
                }
                Role::Speaker => (stage.center().x - side * 0.12 * w, feet_y - 0.03 * h, 0.92),
                Role::Other => return None,
            };
            Some(Affine::translate((target_x, target_y)) * Affine::scale(scale) * Affine::translate((-mark_x, -feet_y)))
        }
        "low_angle" | "high_angle" if dy != 0.0 => Some(Affine::translate((0.0, dy))),
        _ => None,
    }
}

/// Re-stage the cast in each angle's span and write the angles to the report.
#[allow(clippy::too_many_arguments)]
fn restage_angles(
    scene: &mut Scene,
    segments: &[AngleSeg],
    actors: &[Actor],
    variants: &[Actor],
    feet: &[(String, f64)],
    src: &ScriptScene,
    casting: &Casting,
    stage: Rect,
    wide_horizon: Option<f64>,
    total: u32,
    report: &mut SceneReport,
) {
    let speakers: Vec<Option<String>> = src
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Dialogue(Dialogue { speaker, voice, .. }) => {
                Some(if *voice == Voice::VoiceOver { None } else { casting.character(speaker).map(|(k, _)| k) })
            }
            _ => None,
        })
        .collect();
    let staged = |k: &str| actors.iter().any(|a| a.member.key == k && !a.top.is_empty());
    for seg in segments {
        let mut how: Vec<String> = Vec::new();
        let speaker = speakers.get(seg.at_line).cloned().flatten();
        let differs = |k: &&String| Some(k.as_str()) != speaker.as_deref() && staged(k);
        let listener = speakers
            .iter()
            .take(seg.at_line + 1)
            .rev()
            .flatten()
            .find(differs)
            .or_else(|| speakers.iter().skip(seg.at_line + 1).flatten().find(differs))
            .cloned()
            .or_else(|| {
                actors
                    .iter()
                    .find(|a| Some(a.member.key.as_str()) != speaker.as_deref() && !a.top.is_empty())
                    .map(|a| a.member.key.clone())
            });
        let side = listener
            .as_ref()
            .and_then(|k| actors.iter().find(|a| &a.member.key == k))
            .map_or(-1.0, |a| if a.mark_x < stage.center().x { -1.0 } else { 1.0 });
        let dy = angle_dy(seg, wide_horizon, stage);
        for a in actors.iter().chain(variants.iter()) {
            if a.top.is_empty() {
                continue;
            }
            let role = if Some(a.member.key.as_str()) == speaker.as_deref() {
                Role::Speaker
            } else if Some(&a.member.key) == listener.as_ref() {
                Role::Listener
            } else {
                Role::Other
            };
            let Some(&(_, feet)) = feet.iter().find(|(k, _)| *k == a.member.key) else { continue };
            let Some(t) = angle_restage(&seg.camera, role, side, stage, a.mark_x, feet, dy) else {
                continue;
            };
            if a.member.place.pinned {
                if !a.variant {
                    how.push(format!("{} pinned, kept", a.member.key));
                }
                continue;
            }
            transform_during(scene, &a.top, seg.start, seg.end, t, total);
            if !a.variant {
                how.push(a.member.key.clone());
            }
        }
        let verb = match seg.camera.as_str() {
            "reverse" => "mirrored",
            "over_shoulder" => "over the shoulder",
            "low_angle" | "high_angle" => "ground line moved",
            _ => "",
        };
        report.angles.push(AngleNote {
            start: seg.start,
            end: seg.end,
            camera: seg.camera.clone(),
            background: seg.background.clone(),
            restaged: if how.is_empty() || verb.is_empty() { String::new() } else { format!("{verb}: {}", how.join(", ")) },
        });
    }
}

/// The part of a pose word that action lines inflect (`sitting` -> `sit`).
fn pose_stem_of(pose: &str) -> String {
    let lower = pose.to_lowercase();
    let word = lower.split('_').next().unwrap_or_default();
    let mut stem = word;
    for suffix in ["ing", "ed", "e"] {
        if let Some(x) = stem.strip_suffix(suffix) {
            if x.len() >= 3 {
                stem = x;
            }
            break;
        }
    }
    let mut chars: Vec<char> = stem.chars().collect();
    if chars.len() > 3 && chars[chars.len() - 1] == chars[chars.len() - 2] {
        chars.pop();
    }
    chars.into_iter().collect()
}

/// **Pose swaps for one character**: each action line that gives them a pose
/// among their Cast folder's variants ("Guddu sits ..." -> `Guddu Pose sitting`)
/// shows that rig from the line's start to the end of the next spoken line, and
/// the base pose again after, unless the next beat keeps the same rig.
fn pose_swaps(
    src: &ScriptScene,
    schedule: &Schedule,
    casting: &Casting,
    assets: &AssetLibrary,
    member: &Member,
    total: u32,
    gap: u32,
) -> Vec<(String, Vec<(u32, u32)>)> {
    let Some(base_rig) = member.place.cast.clone().or_else(|| member.entry.cast.clone()) else {
        return Vec::new();
    };
    let base_name = crate::cast_build::pose_of_rig(&base_rig).map_or(base_rig.as_str(), |(b, _)| b).to_string();
    let variants: Vec<(String, String)> = assets
        .assets()
        .iter()
        .filter_map(|a| {
            let (b, p) = crate::cast_build::pose_of_rig(&a.name)?;
            (b.eq_ignore_ascii_case(&base_name) && !a.name.eq_ignore_ascii_case(&base_rig)).then(|| (p.to_string(), a.name.clone()))
        })
        .collect();
    pose_beats(src, schedule, &casting.stage_names(), &member.key, &variants, total, gap)
}

/// The pure part of [`pose_swaps`]: `variants` is `(pose, rig name)`.
fn pose_beats(
    src: &ScriptScene,
    schedule: &Schedule,
    names: &[(String, String)],
    key: &str,
    variants: &[(String, String)],
    total: u32,
    gap: u32,
) -> Vec<(String, Vec<(u32, u32)>)> {
    let mut beats: Vec<(String, u32, u32)> = Vec::new();
    for (i, item) in src.items.iter().enumerate() {
        let Item::Action(text) = item else { continue };
        let about = clauses_about(text, key, names);
        if about.is_empty() {
            continue;
        }
        let Some(slot) = schedule.slots.get(i) else { continue };
        for (pose, rig) in variants {
            let stem = pose_stem_of(pose);
            if stem.len() < 3 {
                continue;
            }
            let hit = about
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with(&stem) && w.len() <= stem.len() + 4);
            if hit {
                let end = src
                    .items
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find(|(_, it)| matches!(it, Item::Dialogue(_)))
                    .and_then(|(j, _)| schedule.slots.get(j).map(|s| s.end()))
                    .unwrap_or_else(|| slot.end());
                beats.push((rig.clone(), slot.start, end.max(slot.start + 1).min(total)));
                break;
            }
        }
    }
    beats.sort_by_key(|b| b.1);
    let mut merged: Vec<(String, u32, u32)> = Vec::new();
    for b in beats {
        if let Some(last) = merged.last_mut() {
            if last.0 == b.0 && b.1 <= last.2 + gap {
                last.2 = last.2.max(b.2);
                continue;
            }
            if b.1 < last.2 {
                last.2 = b.1;
            }
        }
        merged.push(b);
    }
    let mut out: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
    for (rig, a, b) in merged.into_iter().filter(|(_, a, b)| a < b) {
        match out.iter_mut().find(|(r, _)| *r == rig) {
            Some((_, spans)) => spans.push((a, b)),
            None => out.push((rig, vec![(a, b)])),
        }
    }
    out
}

// -- shots and lights (P5) ----------------------------------------------------------------

/// The cast as the camera sees it.
struct ShotCast<'a> {
    actors: &'a [Actor],
    /// Each staged actor's feet, by casting key.
    feet: &'a [(String, f64)],
}

impl ShotCast<'_> {
    fn actor(&self, key: &str) -> Option<&Actor> {
        self.actors.iter().find(|a| a.member.key == key)
    }

    fn feet_y(&self, key: &str) -> Option<f64> {
        self.feet.iter().find(|(k, _)| k == key).map(|(_, y)| *y)
    }
}

/// One shot of a plan: from `start` until the next shot starts.
#[derive(Debug, Clone)]
struct Shot {
    start: u32,
    kind: String,
    on: Option<String>,
    center: Point,
    zoom: f64,
    /// A slow 3 % push-in over the shot.
    push: bool,
}

/// A dialogue line, as the camera plans around it.
struct ShotLine {
    slot: buzz_act::script_timing::Slot,
    /// The casting key of a speaker who has a figure on stage; `None` for
    /// voice-over, voice-only and uncast names.
    speaker: Option<String>,
}

fn wide_shot(start: u32, stage: Rect) -> Shot {
    Shot {
        start,
        kind: "wide".into(),
        on: None,
        center: stage.center(),
        zoom: 1.0,
        push: false,
    }
}

/// Is `a` on stage over `from..to`: on their mark inside the stage, arrived
/// by the start, not gone before the end.
fn on_stage(a: &Actor, schedule: &Schedule, stage: Rect, from: u32, to: u32) -> bool {
    let inside = a.mark_x >= stage.x0 && a.mark_x <= stage.x1;
    let arrived = a
        .member
        .enter_item
        .map_or(true, |i| schedule.slots.get(i).is_some_and(|s| s.start <= from));
    let gone = a
        .member
        .exit
        .is_some_and(|(i, _)| schedule.slots.get(i).is_some_and(|s| s.start < to));
    inside && arrived && !gone
}

/// A frame of `zoom` centred near `(cx, cy)`, moved so it never leaves the
/// stage.
fn clamp_frame(stage: Rect, cx: f64, cy: f64, zoom: f64) -> (Point, f64) {
    let zoom = zoom.max(1.0);
    let (w, h) = (stage.width() / zoom, stage.height() / zoom);
    (
        Point::new(
            cx.clamp(stage.x0 + w / 2.0, stage.x1 - w / 2.0),
            cy.clamp(stage.y0 + h / 2.0, stage.y1 - h / 2.0),
        ),
        zoom,
    )
}

/// **A medium (knees up) or a close (chest up) on `who`**: that much of the
/// figure fills 70 % of the frame height, with a little headroom. A partner
/// within 0.35 of the stage width is kept in frame, by centring between the
/// two and widening the shot as far as it takes (never past the stage).
fn frame_on(stage: Rect, who: &Actor, feet_y: f64, partner: Option<&Actor>, close: bool) -> (Point, f64) {
    // How much of the figure shows, from the top of the head down: the knees
    // are about 0.28 of a standing height up from the feet.
    let shows = if close { 0.42 } else { 0.72 };
    let mut zoom = (stage.height() * 0.70 / (who.height * shows).max(1.0)).max(1.0);
    let mut cx = who.mark_x;
    if let Some(p) = partner {
        let dx = (p.mark_x - who.mark_x).abs();
        if dx <= 0.35 * stage.width() {
            let need = dx + who.half_width + p.half_width + 0.04 * stage.width();
            zoom = zoom.min(stage.width() / need.max(1.0)).max(1.0);
            let slack = (stage.width() / zoom / 2.0 - who.half_width).max(0.0);
            cx = ((who.mark_x + p.mark_x) / 2.0).clamp(who.mark_x - slack, who.mark_x + slack);
        }
    }
    let frame_h = stage.height() / zoom;
    let top = feet_y - who.height - 0.12 * frame_h;
    clamp_frame(stage, cx, top + frame_h / 2.0, zoom)
}

/// The shot on `key` over line `at`, keeping the other speaker (the line
/// before's, else the line after's) in frame when they are near.
#[allow(clippy::too_many_arguments)]
fn shot_on(
    cast: &ShotCast,
    lines: &[ShotLine],
    at: usize,
    key: &str,
    close: bool,
    schedule: &Schedule,
    stage: Rect,
    start: u32,
) -> Option<Shot> {
    let who = cast.actor(key)?;
    let feet_y = cast.feet_y(key)?;
    let line = lines.get(at)?;
    let (from, to) = (line.slot.start, line.slot.end());
    let other = |l: &ShotLine| {
        l.speaker
            .as_deref()
            .filter(|k| *k != key)
            .and_then(|k| cast.actor(k))
            .filter(|a| on_stage(a, schedule, stage, from, to))
    };
    let partner = lines[..at]
        .iter()
        .rev()
        .find_map(|l| other(l))
        .or_else(|| lines[at + 1..].iter().find_map(|l| other(l)));
    let (center, zoom) = frame_on(stage, who, feet_y, partner, close);
    Some(Shot {
        start,
        kind: if close { "close" } else { "medium" }.into(),
        on: Some(key.to_string()),
        center,
        zoom,
        push: false,
    })
}

/// **Plan a scene's shots**, `"auto"` or from a list.
///
/// Auto: wide for the first 2 s; a medium on the speaker for a line of 3 s or
/// more when two or more characters are on stage, cut in as the line starts
/// and back to wide after it (with its caption gap); the same for a change
/// of speaker whose line is 1.5 s or more once the shot before has run 6 s
/// (P8-C3); never two cuts within
/// 3 s; a wide that holds more than 10 s pushes in 3 %.
#[allow(clippy::too_many_arguments)]
fn plan_shots(
    shots: &Shots,
    src: &ScriptScene,
    schedule: &Schedule,
    casting: &Casting,
    cast: &ShotCast,
    stage: Rect,
    timing: &Timing,
    total: u32,
    report: &mut SceneReport,
) -> Vec<Shot> {
    let fps = timing.fps;
    let secs = |s: f64| (s * fps).round().max(0.0) as u32;
    let gap = timing.frames(timing.line_gap);
    let lines: Vec<ShotLine> = src
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let Item::Dialogue(Dialogue { speaker, voice, .. }) = item else {
                return None;
            };
            let slot = *schedule.slots.get(index)?;
            let speaker = if *voice == Voice::VoiceOver {
                None
            } else {
                casting
                    .character(speaker)
                    .filter(|(_, e)| !e.voice_only)
                    .map(|(k, _)| k)
                    .filter(|k| cast.actor(k).is_some())
            };
            Some(ShotLine { slot, speaker })
        })
        .collect();

    let mut plan: Vec<Shot> = Vec::new();
    match shots {
        Shots::Auto(word) => {
            if !word.trim().eq_ignore_ascii_case("auto") {
                report
                    .warnings
                    .push(format!("shots {word:?}: not \"auto\" or a list; no camera"));
                return plan;
            }
            let (open, spacing, long) = (secs(2.0), secs(3.0).max(1), secs(3.0));
            plan.push(wide_shot(0, stage));
            let mut last_cut: Option<u32> = None;
            for (at, line) in lines.iter().enumerate() {
                let Some(key) = line.speaker.as_deref() else {
                    continue;
                };
                let (start, end) = (line.slot.start, line.slot.end());
                // P8-C3: a change of speaker also cuts, when the shot before
                // has run 6 s and the new speaker's line is 1.5 s or more.
                let previous = lines[..at].iter().rev().find_map(|l| l.speaker.as_deref());
                let ran = start.saturating_sub(plan.last().map_or(0, |s| s.start));
                let change = previous != Some(key) && ran >= secs(6.0) && line.slot.frames >= secs(1.5);
                if (line.slot.frames < long && !change) || start < open || start >= total {
                    continue;
                }
                if last_cut.is_some_and(|c| start < c + spacing) {
                    continue;
                }
                let here = cast
                    .actors
                    .iter()
                    .filter(|a| on_stage(a, schedule, stage, start, end))
                    .count();
                let speaker_here = cast
                    .actor(key)
                    .is_some_and(|a| on_stage(a, schedule, stage, start, end));
                if here < 2 || !speaker_here {
                    continue;
                }
                let Some(shot) = shot_on(cast, &lines, at, key, false, schedule, stage, start) else {
                    continue;
                };
                plan.push(shot);
                last_cut = Some(start);
                let back = (end + gap).min(total);
                if back + 1 < total {
                    plan.push(wide_shot(back, stage));
                    last_cut = Some(back);
                }
            }
            let hold = secs(10.0);
            for i in 0..plan.len() {
                let end = plan.get(i + 1).map_or(total, |s| s.start);
                if plan[i].kind == "wide" && end.saturating_sub(plan[i].start) > hold {
                    plan[i].push = true;
                }
            }
        }
        Shots::List(list) => {
            let mut specs = list.clone();
            specs.sort_by_key(|s| s.at_line);
            let mut holds: Vec<(u32, u32)> = Vec::new();
            for spec in &specs {
                let Some(line) = lines.get(spec.at_line) else {
                    report.warnings.push(format!(
                        "shot at line {}: the scene has {} line(s); skipped",
                        spec.at_line,
                        lines.len()
                    ));
                    continue;
                };
                let start = if spec.at_line == 0 { 0 } else { line.slot.start.min(total.saturating_sub(1)) };
                let kind = spec.kind.trim().to_lowercase();
                let shot = match kind.as_str() {
                    "medium" | "close" => {
                        let who = match spec.on.as_deref().filter(|n| !n.trim().is_empty()) {
                            Some(name) => casting.character(name).map(|(k, _)| k),
                            None => line.speaker.clone(),
                        };
                        match who.and_then(|k| {
                            shot_on(cast, &lines, spec.at_line, &k, kind == "close", schedule, stage, start)
                        }) {
                            Some(s) => s,
                            None => {
                                report.warnings.push(format!(
                                    "{kind} shot at line {}: {} is not on stage; wide instead",
                                    spec.at_line,
                                    spec.on.as_deref().unwrap_or("the speaker")
                                ));
                                wide_shot(start, stage)
                            }
                        }
                    }
                    "wide" | "" => wide_shot(start, stage),
                    other => {
                        report
                            .warnings
                            .push(format!("shot at line {}: unknown kind {other:?}; wide", spec.at_line));
                        wide_shot(start, stage)
                    }
                };
                plan.retain(|s| s.start != start);
                plan.push(shot);
                if let Some(h) = spec.hold.filter(|h| *h > 0.0) {
                    holds.push((start, start + secs(h)));
                }
            }
            plan.sort_by_key(|s| s.start);
            for (start, end) in holds {
                let next = plan.iter().find(|s| s.start > start).map_or(total, |s| s.start);
                let held_is_wide = plan.iter().find(|s| s.start == start).is_none_or(|s| s.kind == "wide");
                if end < next && end + 1 < total && !held_is_wide {
                    plan.push(wide_shot(end, stage));
                }
            }
            plan.sort_by_key(|s| s.start);
            if plan.first().is_none_or(|s| s.start > 0) {
                plan.insert(0, wide_shot(0, stage));
            }
        }
    }
    plan
}

/// **Key the camera for a plan**: each shot is a key on its first frame and a
/// held (or 3 %-pushed) key on its last, so a cut is two keys a frame apart
/// and never a move. Then the captions are pinned to the frame.
fn write_shots(scene: &mut Scene, plan: &[Shot], total: u32, report: &mut SceneReport) {
    if plan.is_empty() || total == 0 {
        return;
    }
    let last = total - 1;
    {
        let camera = scene.camera_mut();
        camera.enabled = true;
        for (i, shot) in plan.iter().enumerate() {
            if shot.start > last {
                break;
            }
            let end = plan.get(i + 1).map_or(total, |s| s.start).min(total);
            let mut key = buzz_scene::CameraKey::new(shot.start, shot.center);
            key.zoom = shot.zoom;
            camera.set_key(key);
            let hold_to = end.saturating_sub(1);
            let mut push_to = None;
            if hold_to > shot.start {
                let mut held = key;
                held.frame = hold_to;
                if shot.push {
                    held.zoom = shot.zoom * 1.03;
                    push_to = Some(hold_to);
                }
                camera.set_key(held);
            }
            report.shots.push(ShotCut {
                frame: shot.start,
                kind: shot.kind.clone(),
                on: shot.on.clone(),
                x: shot.center.x,
                y: shot.center.y,
                scale: shot.zoom,
                push_to,
            });
            report.notes.push(format!(
                "shot: frame {} {}{} (camera {:.0}, {:.0} x{:.2}){}",
                shot.start,
                shot.kind,
                shot.on.as_deref().map(|o| format!(" on {o}")).unwrap_or_default(),
                shot.center.x,
                shot.center.y,
                shot.zoom,
                push_to.map(|f| format!(", pushing in 3 % to frame {f}")).unwrap_or_default()
            ));
        }
    }
    pin_captions(scene);
}

/// **Captions stay on the screen.** A layer cannot ignore the camera, so each
/// caption keyframe is counter-transformed by the inverse of the camera on its
/// frame: split at every camera key a caption spans, and where the camera
/// moves under one (a push-in), split at its last frame and classic-tweened
/// between, so it holds still on the screen while the world moves. A split
/// copy keeps the speaker label, so `Lip Sync from Captions` still covers the
/// whole line.
fn pin_captions(scene: &mut Scene) {
    let Some(layer) = scene.layers().iter().find(|l| l.name == "Subtitles").map(|l| l.id) else {
        return;
    };
    let camera = scene.camera().clone();
    let size = scene.stage().size;
    let at = |frame: u32| camera.transform_at(frame, size);
    let keys: Vec<u32> = camera.keys().iter().map(|k| k.frame).collect();
    fn split(frames: &mut buzz_scene::LayerTimeline, f: u32) {
        if f < frames.length() && !frames.is_keyframe(f) && !frames.objects_at(f).is_empty() {
            let label = frames.keyframe_at(f).and_then(|k| k.label.clone());
            frames.insert_keyframe(f);
            frames.set_label(f, label);
        }
    }
    scene.update_layer(layer, |l| {
        let length = l.frames.length();
        for &f in &keys {
            split(&mut l.frames, f);
        }
        let starts: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).collect();
        for (i, &s) in starts.iter().enumerate() {
            let next = starts.get(i + 1).copied().unwrap_or(length);
            if next > s + 1 && at(s) != at(next - 1) {
                split(&mut l.frames, next - 1);
            }
        }
        let starts: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).collect();
        for (i, &s) in starts.iter().enumerate() {
            let objects = l.frames.objects_at(s).to_vec();
            if objects.is_empty() {
                continue;
            }
            let inverse = at(s).inverse();
            let pinned: Vec<Arc<Object>> = objects
                .iter()
                .map(|o| {
                    let mut o = (**o).clone();
                    o.transform = inverse * o.transform;
                    Arc::new(o)
                })
                .collect();
            l.frames.set_objects(s, pinned);
            let next = starts.get(i + 1).copied().unwrap_or(length);
            if next > s + 1 && next < length && at(s) != at(next) {
                l.frames.set_tween(s, buzz_scene::Tween::classic());
            }
        }
    });
}

/// The key and the sky fill a built set gets from `staging` (its private
/// `light`), for a set made of a background asset: same names, colours and
/// strengths, no rim, no edge bands. [`apply_lighting`] then sets the key.
fn add_set_lights(scene: &mut Scene, setting: Setting) {
    use buzz_scene::LightKind;
    let stage = scene.stage().stage_rect();
    let key_kind = match setting {
        Setting::Daylight => LightKind::Sun { azimuth: -0.9, elevation: 1.05 },
        Setting::Sunset => LightKind::Sun { azimuth: -0.35, elevation: 0.55 },
        Setting::Storm => LightKind::sky(),
        Setting::Night | Setting::Interior => LightKind::Lamp {
            position: Point::new(stage.x0 + stage.width() * 0.22, stage.y0 + stage.height() * 0.05),
            height: 220.0,
            radius: stage.width() * 0.75,
        },
    };
    let (key_colour, fill_colour, fill_strength) = match setting {
        Setting::Daylight => (Color::from_rgb8(0xFF, 0xF6, 0xE2), Color::from_rgb8(0xA8, 0xC6, 0xE8), 0.85),
        Setting::Sunset => (Color::from_rgb8(0xFF, 0xB2, 0x6B), Color::from_rgb8(0x6C, 0x7E, 0xC2), 0.6),
        Setting::Night => (Color::from_rgb8(0xFF, 0xC9, 0x7A), Color::from_rgb8(0x2C, 0x3C, 0x6A), 0.35),
        Setting::Interior => (Color::from_rgb8(0xFF, 0xE3, 0xB8), Color::from_rgb8(0x8E, 0x88, 0x80), 0.55),
        Setting::Storm => (Color::from_rgb8(0xD6, 0xE4, 0xFF), Color::from_rgb8(0x22, 0x2E, 0x52), 0.22),
    };
    let key_id = scene.add_light(key_kind);
    let fill_id = scene.add_light(LightKind::sky());
    let rig = scene.lights_mut();
    rig.enabled = true;
    rig.edges = buzz_scene::EdgeMode::Off;
    if let Some(key) = rig.get_mut(key_id) {
        key.name = "Key".to_string();
        key.color = key_colour;
        key.rim = 0.0;
        key.shadows = true;
        key.shadow_strength = if setting == Setting::Night { 0.5 } else { 0.38 };
        key.standing_height = 26.0;
    }
    if let Some(fill) = rig.get_mut(fill_id) {
        fill.name = "Sky".to_string();
        fill.color = fill_colour;
        fill.kind = LightKind::Sky { horizon: fill_colour };
        fill.intensity = fill_strength;
    }
}

/// **The casting's key light, over the lights the setting made** (the ones
/// from index `first` on): `sun` a sun from `from` at 55° (15° and warm at
/// sunset), `moon` a cool sun at 30° with the sky fill turned down, `lamp` a
/// lamp at the top of the stage on the `from` side, `window` a lamp just
/// outside the frame on that side, `none` the setting's sky fill alone.
fn apply_lighting(
    scene: &mut Scene,
    lighting: &Lighting,
    setting: Setting,
    stage: Rect,
    first: usize,
    report: &mut SceneReport,
) {
    use buzz_scene::LightKind;
    use std::f64::consts::{FRAC_PI_2, PI};

    let from = lighting.from.trim().to_lowercase();
    let side = match from.as_str() {
        "left" => -1.0,
        "right" => 1.0,
        "front" | "" => 0.0,
        other => {
            report.warnings.push(format!("lighting from {other:?}: not left, right or front; front"));
            0.0
        }
    };
    // Clockwise from the right, towards the light; the sun's depth component
    // kept the sign the setting's own key uses (-0.9 is from the right).
    let azimuth = if side < 0.0 {
        -(PI - 0.9)
    } else if side > 0.0 {
        -0.9
    } else {
        -FRAC_PI_2
    };
    let word = lighting.key.trim().to_lowercase();
    let rig = scene.lights_mut();
    let new: Vec<(buzz_scene::LightId, String)> =
        rig.lights.iter().skip(first).map(|l| (l.id, l.name.clone())).collect();
    let Some(key_id) = new.iter().find(|(_, n)| n != "Sky").map(|(id, _)| *id) else {
        report.warnings.push("lighting ignored: the set made no key light".into());
        return;
    };
    let fill_id = new.iter().find(|(_, n)| n == "Sky").map(|(id, _)| *id);
    let warm = lighting.warm;
    match word.as_str() {
        "sun" => {
            let sunset = setting == Setting::Sunset;
            let elevation = if sunset { 15f64 } else { 55f64 }.to_radians();
            if let Some(key) = rig.get_mut(key_id) {
                key.kind = LightKind::Sun { azimuth, elevation };
                key.color = if sunset {
                    Color::from_rgb8(0xFF, 0xB2, 0x6B)
                } else if warm {
                    Color::from_rgb8(0xFF, 0xE6, 0xBF)
                } else {
                    Color::from_rgb8(0xF4, 0xF6, 0xFF)
                };
            }
        }
        "moon" => {
            if let Some(key) = rig.get_mut(key_id) {
                key.kind = LightKind::Sun {
                    azimuth,
                    elevation: 30f64.to_radians(),
                };
                key.color = Color::from_rgb8(0xB8, 0xC8, 0xF0);
                key.intensity *= 0.7;
            }
            if let Some(fill) = fill_id.and_then(|id| rig.get_mut(id)) {
                fill.intensity *= 0.6;
            }
        }
        "lamp" | "window" => {
            let position = if word == "lamp" {
                Point::new(stage.center().x + side * stage.width() * 0.35, stage.y0)
            } else if side == 0.0 {
                Point::new(stage.center().x, stage.y0 - stage.height() * 0.05)
            } else {
                let x = if side < 0.0 {
                    stage.x0 - stage.width() * 0.05
                } else {
                    stage.x1 + stage.width() * 0.05
                };
                Point::new(x, stage.y0 + stage.height() * 0.3)
            };
            if let Some(key) = rig.get_mut(key_id) {
                let mut kind = LightKind::lamp(position);
                if let LightKind::Lamp { height, radius, .. } = &mut kind {
                    *height = 220.0;
                    *radius = stage.width() * if word == "lamp" { 0.75 } else { 0.9 };
                }
                key.kind = kind;
                key.color = match (word.as_str(), warm) {
                    ("lamp", true) => Color::from_rgb8(0xFF, 0xBE, 0x78),
                    ("lamp", false) => Color::from_rgb8(0xFF, 0xDD, 0xAA),
                    (_, true) => Color::from_rgb8(0xFF, 0xE8, 0xC8),
                    _ => Color::from_rgb8(0xEE, 0xF2, 0xFF),
                };
            }
        }
        "none" => {
            rig.remove(key_id);
        }
        other => {
            report
                .warnings
                .push(format!("lighting key {other:?}: not sun, lamp, moon, window or none; the setting's"));
            return;
        }
    }
    report.notes.push(format!(
        "lighting: {word} from {}{}",
        if from.is_empty() { "front" } else { from.as_str() },
        if warm { ", warm" } else { "" }
    ));
}

fn place_prop(
    scene: &mut Scene,
    assets: &AssetLibrary,
    prop: &PropSpec,
    stage: Rect,
    ground: f64,
    opts: &BuildOptions,
    report: &mut SceneReport,
) {
    let Some(asset) = find_asset(assets, &prop.asset, prop.folder.as_deref(), false) else {
        report
            .skipped
            .push(format!("prop {:?}: no such asset in the library", prop.asset));
        return;
    };
    match place_asset(scene, assets, asset) {
        Ok(placed) => {
            let Some(bounds) = placed.bounds else {
                report.warnings.push(format!("prop {:?} has nothing to draw", prop.asset));
                return;
            };
            let h = prop.h.unwrap_or(opts.figure_height).clamp(0.02, 3.0) * stage.height();
            let x = stage.x0 + stage.width() * prop.x.unwrap_or(0.5);
            let y = stage.y0 + stage.height() * prop.y.unwrap_or(ground);
            let (t, _) = stand_on(bounds, h, x, y);
            transform_layers(scene, &placed.top, t);
        }
        Err(e) => report.warnings.push(e),
    }
}

/// Put one character on the set at their mark, at the right size.
#[allow(clippy::too_many_arguments)]
fn stage_actor(
    scene: &mut Scene,
    assets: &AssetLibrary,
    member: Member,
    mark_x: f64,
    feet_y: f64,
    height: f64,
    stage: Rect,
    total: u32,
    report: &mut SceneReport,
) -> Option<Actor> {
    let drawn = match member.entry.faces.as_deref().map(str::to_lowercase).as_deref() {
        Some("right") => 1.0,
        Some("left") => -1.0,
        _ => 0.0,
    };
    // A scene's own `cast`/`asset` replaces the character's for that scene.
    let kind = if let Some(c) = &member.place.cast {
        Kind::Cast(c.clone())
    } else if let Some(a) = &member.place.asset {
        Kind::Asset(a.clone())
    } else {
        member.entry.kind()
    };
    let toward_centre = if mark_x < stage.center().x { 1.0 } else { -1.0 };

    match kind {
        Kind::Puppet(style) => {
            let colour = |key: &str| style.get(key).and_then(Value::as_str).and_then(hex_colour);
            // A puppet is drawn facing right; `facing` mirrors it to face the
            // middle of the stage.
            let mut figure = FigureSpec {
                height,
                facing: toward_centre,
                ..FigureSpec::default()
            };
            let nth = report.cast.len() + 1;
            figure.palette = buzz_act::Palette::nth(nth);
            if let Some(c) = colour("skin") {
                figure.palette.skin = c;
            }
            if let Some(c) = colour("shirt") {
                figure.palette.shirt = c;
            }
            if let Some(c) = colour("trousers") {
                figure.palette.trousers = c;
            }
            let spec = PuppetSpec {
                name: member.key.clone(),
                figure,
                at: Point::new(mark_x, feet_y),
                eyes: colour("eyes").unwrap_or(PuppetSpec::default().eyes),
                frames: total.max(2),
                // **No Breathe modifier.** The Idle and Talk performances already
                // breathe, and the modifier on a rig makes every frame several
                // times slower to render (measured: a scene with one puppet went
                // from fifteen seconds for ten frames to a minute).
                breathe_rate: 0.0,
                ..PuppetSpec::default()
            };
            report.warnings.push(format!(
                "{}: the built-in puppet is for blocking; a scene with one renders several times slower than the same scene made of Cast rigs",
                member.key
            ));
            let built = puppet::build(scene, &spec);
            let placed = scene.find_object(built.body).map(|(_, o)| o.transform)?;
            Some(Actor {
                played_by: "puppet".into(),
                body: Some(built.body),
                mouth: Mouth::Layer {
                    layer: built.mouth_layer,
                    symbol: built.mouth_symbol,
                    placement: Affine::translate(built.mouth_at.to_vec2()) * Affine::scale(built.mouth_scale),
                },
                placed,
                x: mark_x,
                mark_x,
                // `figure.facing` already faces the middle.
                drawn: 1.0,
                facing: toward_centre,
                height,
                half_width: height * 0.22,
                offstage: false,
                eyes: Vec::new(),
                layer_eyes: Vec::new(),
                blinks: spec.blink_rate > 0.0,
                top: Vec::new(),
                layers: Vec::new(),
                variant: false,
                pose_spans: Vec::new(),
                member,
            })
        }
        Kind::Cast(name) | Kind::Asset(name) => {
            let is_cast = member.place.cast.is_some() || (member.place.asset.is_none() && member.entry.cast.is_some());
            let Some(asset) = find_asset(assets, &name, member.entry.folder.as_deref(), is_cast) else {
                report.skipped.push(format!(
                    "{}: no asset called {name:?} in the library",
                    member.key
                ));
                return None;
            };
            let placed = match place_asset(scene, assets, asset) {
                Ok(p) => p,
                Err(e) => {
                    report.skipped.push(format!("{}: {e}", member.key));
                    return None;
                }
            };
            let Some(bounds) = placed.bounds else {
                report
                    .skipped
                    .push(format!("{}: asset {name:?} has nothing to draw", member.key));
                return None;
            };
            let (t, scale) = stand_on(bounds, height, mark_x, feet_y);
            transform_layers(scene, &placed.top, t);
            // Which rig it is, asked once.
            let rig = placed.body.and_then(|body| buzz_act::layer_rig::rig_of(scene, body));
            match &rig {
                Some(CharacterRig::Armature(body)) => {
                    slim_rig(scene, *body, &member.key);
                }
                Some(CharacterRig::Layers(view)) => {
                    slim_layer_rig(scene, view, &member.key);
                }
                None => {}
            }
            // Separate eye objects take the Blink modifier. A rig's eyes are parts
            // of its armature, where a modifier would squash the whole figure, so
            // those are blinked by swapping the eye part's drawing (below) — and a
            // layer rig's eyes the same way, on their own layers.
            let separate_eyes = add_blinks(scene, &placed.top);
            let (eyes, layer_eyes) = match &rig {
                Some(CharacterRig::Armature(body)) => (eye_variants(scene, *body, &member.key), Vec::new()),
                Some(CharacterRig::Layers(view)) => (Vec::new(), layer_eye_variants(scene, view, &member.key)),
                None => (Vec::new(), Vec::new()),
            };
            let blinks = separate_eyes > 0 || !eyes.is_empty() || !layer_eyes.is_empty();
            if !blinks {
                report.warnings.push(format!(
                    "{}: no separate eyes and no eye parts with eyelids found, so no blink",
                    member.key
                ));
            }
            let mut actor = Actor {
                played_by: format!("{} {name:?}", if is_cast { "Cast" } else { "asset" }),
                body: placed.body,
                mouth: Mouth::None,
                placed: placed
                    .body
                    .and_then(|b| buzz_act::layer_rig::placement(scene, b))
                    .unwrap_or(Affine::IDENTITY),
                x: mark_x,
                mark_x,
                drawn,
                facing: if drawn != 0.0 { drawn } else { 0.0 },
                height,
                half_width: bounds.width() * scale / 2.0,
                offstage: false,
                eyes,
                layer_eyes,
                blinks,
                top: placed.top.clone(),
                layers: placed.all.clone(),
                variant: false,
                pose_spans: Vec::new(),
                member,
            };
            if let Some(body) = placed.body {
                match &rig {
                    Some(CharacterRig::Layers(view)) => {
                        if let Some(mouth) = textsync::find_layer_rig_mouth(scene, view) {
                            actor.mouth = Mouth::RigLayer(mouth);
                        }
                    }
                    _ => {
                        if textsync::find_rig_mouth(scene, body).is_some() {
                            actor.mouth = Mouth::Rig(body);
                        }
                    }
                }
            } else {
                // A drawing with no rig stands where it is put.
                report.warnings.push(format!(
                    "{}: {name:?} has no rig, so it does not perform",
                    actor.member.key
                ));
            }
            // Face the middle, if the drawing looks one way.
            if drawn != 0.0 && drawn != toward_centre {
                transform_layers(scene, &placed.top, mirror_about(mark_x));
                actor.placed = mirror_about(mark_x) * actor.placed;
                actor.facing = toward_centre;
            }
            Some(actor)
        }
        Kind::VoiceOnly | Kind::Uncast => {
            report.skipped.push(format!(
                "{}: no cast, asset or puppet in the casting file",
                member.key
            ));
            None
        }
    }
}

/// **Keep a rig's keyframes small** by moving each bulky part's drawing into the
/// library.
///
/// A keyframe of a rig holds the whole armature, parts included, and a rig the
/// image pipeline makes carries its drawing *inside* the parts: several
/// megabytes of path data per part-set. One key per two frames over a
/// seventy-second scene is thousands of copies, and the document cannot be
/// written (the first run of this wanted thirty-five gigabytes). A part that is
/// an instance of a library symbol costs a few hundred bytes a key, and draws
/// the same, so each rigid part whose artwork is a group or a shape becomes an
/// instance of a symbol made for it, carrying the artwork's own transform.
/// Skinned parts need their paths where they are and are left alone.
///
/// Returns how many parts were moved.
fn slim_rig(scene: &mut Scene, body: ObjectId, label: &str) -> usize {
    use buzz_scene::RigBinding;

    let Some(parts) = scene.find_object(body).and_then(|(_, o)| match &o.kind {
        ObjectKind::Armature(rig) => Some(rig.parts.clone()),
        _ => None,
    }) else {
        return 0;
    };

    let mut moved = 0usize;
    let mut slimmed: Vec<buzz_scene::RigPart> = Vec::with_capacity(parts.len());
    for (index, part) in parts.into_iter().enumerate() {
        let bulky = matches!(part.binding, RigBinding::Rigid(_))
            && matches!(part.artwork.kind, ObjectKind::Group(_) | ObjectKind::Shape(_));
        if !bulky {
            slimmed.push(part);
            continue;
        }
        let symbol = scene.add_symbol(
            format!("{label} part {index}"),
            buzz_scene::SymbolKind::Graphic,
            Some("Cast parts"),
        );
        let mut inner = (*part.artwork).clone();
        let place = inner.transform;
        inner.transform = Affine::IDENTITY;
        let Some(layer) = scene
            .library()
            .get(symbol)
            .and_then(|s| s.layers.iter().next())
            .map(|l| l.id)
        else {
            slimmed.push(part);
            continue;
        };
        scene.library_mut().update(symbol, |s| {
            s.layers.update(layer, |l| {
                l.frames.set_objects(0, vec![Arc::new(inner)]);
            });
        });
        let mut instance = Object::instance_of(part.artwork.id, symbol);
        instance.transform = place;
        instance.name = part.artwork.name.clone();
        slimmed.push(buzz_scene::RigPart {
            artwork: Arc::new(instance),
            binding: part.binding,
        });
        moved += 1;
    }

    if moved > 0 {
        scene.update_object_across(0, u32::MAX, body, |o| {
            if let ObjectKind::Armature(rig) = &mut o.kind {
                rig.parts = slimmed.clone();
            }
        });
    }
    moved
}

/// **The eyes of a rig, and a shut version of each.**
///
/// A rig's eye is a part: an instance of a composite symbol (white, pupil, and
/// an eyelid that shows one frame of a three-frame lid symbol). Blinking is that
/// part showing the half-shut and the shut eye in turn, which are the same
/// composite with the lid on another frame: this makes those two copies for each
/// eye and returns `(part index, [open, half, shut])`.
fn eye_variants(scene: &mut Scene, body: ObjectId, label: &str) -> Vec<(usize, [buzz_scene::SymbolId; 3])> {
    let Some(parts) = scene.find_object(body).and_then(|(_, o)| match &o.kind {
        ObjectKind::Armature(rig) => Some(rig.parts.clone()),
        _ => None,
    }) else {
        return Vec::new();
    };

    let mut found: Vec<(usize, buzz_scene::SymbolId)> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let ObjectKind::Instance(instance) = &part.artwork.kind else {
            continue;
        };
        let Some(symbol) = scene.library().get(instance.symbol) else {
            continue;
        };
        let name = symbol.name.to_lowercase();
        let has_lid = symbol.layers.iter().any(|l| l.name == "Eyelid");
        if name.contains("eye") && has_lid && !name.contains("brow") {
            found.push((index, instance.symbol));
        }
    }

    let mut out = Vec::new();
    for (index, open) in found {
        let Some(original) = scene.library().get(open).cloned() else {
            continue;
        };
        let mut shut_ids = [open; 3];
        for (slot, frame) in [(1usize, 1u32), (2, 2)] {
            let id = scene.add_symbol(
                format!("{label} eye {}", if frame == 1 { "half" } else { "shut" }),
                buzz_scene::SymbolKind::Graphic,
                Some("Cast parts"),
            );
            let mut copy = (*original).clone();
            copy.id = id;
            copy.name = scene
                .library()
                .get(id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| copy.name.clone());
            copy.folder = Some("Cast parts".to_string());
            // The lid on its half-shut or shut frame.
            let lid = copy.layers.iter().find(|l| l.name == "Eyelid").map(|l| l.id);
            if let Some(lid) = lid {
                copy.layers.update(lid, |l| {
                    if let Some(first) = l.frames.objects_at(0).first() {
                        let mut updated = (**first).clone();
                        if let ObjectKind::Instance(i) = &mut updated.kind {
                            i.loop_mode = buzz_scene::LoopMode::SingleFrame;
                            i.first_frame = frame;
                        }
                        l.frames.set_objects(0, vec![Arc::new(updated)]);
                    }
                });
            }
            scene.library_mut().insert(copy);
            shut_ids[slot] = id;
        }
        out.push((index, shut_ids));
    }
    out
}

/// [`slim_rig`] for a **layer rig**: each part whose drawing is a group or a
/// shape becomes an instance of a symbol made for it, keeping its id, its
/// transform and its transformation point, so a key per two frames on every
/// part layer costs a few hundred bytes rather than a copy of the drawing.
/// Returns how many parts were moved.
fn slim_layer_rig(scene: &mut Scene, rig: &LayerRigView, label: &str) -> usize {
    let mut moved = 0usize;
    for (index, &(_, _, id)) in rig.parts.iter().enumerate() {
        let Some(art) = scene.find_object(id).map(|(_, o)| o.clone()) else {
            continue;
        };
        if !matches!(art.kind, ObjectKind::Group(_) | ObjectKind::Shape(_)) {
            continue;
        }
        let symbol = scene.add_symbol(
            format!("{label} part {index}"),
            buzz_scene::SymbolKind::Graphic,
            Some("Cast parts"),
        );
        let Some(inner_layer) = scene
            .library()
            .get(symbol)
            .and_then(|s| s.layers.iter().next())
            .map(|l| l.id)
        else {
            continue;
        };
        // The drawing in the symbol stands where it does in its own space;
        // the instance carries the transform, exactly as the part did.
        let mut inner = (*art).clone();
        inner.transform = Affine::IDENTITY;
        inner.pivot = None;
        scene.library_mut().update(symbol, |s| {
            s.layers.update(inner_layer, |l| {
                l.frames.set_objects(0, vec![Arc::new(inner)]);
            });
        });
        let kind = Object::instance_of(id, symbol).kind;
        scene.update_object_across(0, u32::MAX, id, |o| o.kind = kind.clone());
        moved += 1;
    }
    moved
}

/// [`eye_variants`] for a **layer rig**: its eye parts (each an instance of a
/// composite eye symbol with an `Eyelid` layer, on a layer of its own) with a
/// half-shut and a shut copy of each, by object.
fn layer_eye_variants(scene: &mut Scene, rig: &LayerRigView, label: &str) -> Vec<(ObjectId, [buzz_scene::SymbolId; 3])> {
    let mut found: Vec<(ObjectId, buzz_scene::SymbolId)> = Vec::new();
    for &(_, _, id) in &rig.parts {
        let Some((_, object)) = scene.find_object(id) else {
            continue;
        };
        let ObjectKind::Instance(instance) = &object.kind else {
            continue;
        };
        let Some(symbol) = scene.library().get(instance.symbol) else {
            continue;
        };
        let name = symbol.name.to_lowercase();
        let has_lid = symbol.layers.iter().any(|l| l.name == "Eyelid");
        if name.contains("eye") && has_lid && !name.contains("brow") {
            found.push((id, instance.symbol));
        }
    }

    let mut out = Vec::new();
    for (id, open) in found {
        let Some(original) = scene.library().get(open).cloned() else {
            continue;
        };
        let mut shut_ids = [open; 3];
        for (slot, frame) in [(1usize, 1u32), (2, 2)] {
            let copy_id = scene.add_symbol(
                format!("{label} eye {}", if frame == 1 { "half" } else { "shut" }),
                buzz_scene::SymbolKind::Graphic,
                Some("Cast parts"),
            );
            let mut copy = (*original).clone();
            copy.id = copy_id;
            copy.name = scene
                .library()
                .get(copy_id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| copy.name.clone());
            copy.folder = Some("Cast parts".to_string());
            let lid = copy.layers.iter().find(|l| l.name == "Eyelid").map(|l| l.id);
            if let Some(lid) = lid {
                copy.layers.update(lid, |l| {
                    if let Some(first) = l.frames.objects_at(0).first() {
                        let mut updated = (**first).clone();
                        if let ObjectKind::Instance(i) = &mut updated.kind {
                            i.loop_mode = buzz_scene::LoopMode::SingleFrame;
                            i.first_frame = frame;
                        }
                        l.frames.set_objects(0, vec![Arc::new(updated)]);
                    }
                });
            }
            scene.library_mut().insert(copy);
            shut_ids[slot] = copy_id;
        }
        out.push((id, shut_ids));
    }
    out
}

/// Frames at which a character blinks: every three to five seconds, the first
/// after one to three, each on an even frame so it lands on a key of a
/// performance written on twos. Seeded by the character, so a cast does not
/// blink together.
fn blink_starts(seed: &str, total: u32, fps: f64) -> Vec<u32> {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.bytes() {
        state = (state ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
    }
    let mut next = move || {
        // splitmix64
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) as f64 / u64::MAX as f64
    };
    let mut out = Vec::new();
    let mut at = fps * (1.0 + 2.0 * next());
    while (at as u32) + 8 < total {
        out.push((at as u32) & !1);
        at += fps * (3.0 + 2.0 * next());
    }
    out
}

/// How shut the eye is at `frame`: 0 open, 1 half, 2 shut. A blink is half,
/// shut, half over six frames.
fn eye_state(starts: &[u32], frame: u32) -> usize {
    for &b in starts {
        if frame < b {
            break;
        }
        match frame - b {
            0..=1 => return 1,
            2..=3 => return 2,
            4..=5 => return 1,
            _ => {}
        }
    }
    0
}

/// Write a rig's blinks: keys through each blink, and on every key the rig has,
/// each eye showing the state it should at that frame.
fn blink_rig(scene: &mut Scene, actor: &Actor, total: u32, fps: f64) {
    let Some(body) = actor.body else {
        return;
    };
    let Some((layer, _)) = scene.find_object(body) else {
        return;
    };
    let starts = blink_starts(&actor.member.key, total, fps);
    for b in &starts {
        for step in [0u32, 2, 4, 6] {
            if b + step < total {
                scene.ensure_keyframe(layer, b + step);
            }
        }
    }
    let keys: Vec<u32> = scene
        .layers()
        .get(layer)
        .map(|l| {
            l.frames
                .keyframes()
                .iter()
                .filter(|k| k.objects.iter().any(|o| o.id == body))
                .map(|k| k.start)
                .collect()
        })
        .unwrap_or_default();
    for frame in keys {
        let state = eye_state(&starts, frame);
        let eyes = actor.eyes.clone();
        scene.update_object_at(frame, body, |target| {
            let ObjectKind::Armature(rig) = &mut target.kind else {
                return;
            };
            for (index, symbols) in &eyes {
                let Some(part) = rig.parts.get_mut(*index) else {
                    continue;
                };
                let mut artwork = (*part.artwork).clone();
                if let ObjectKind::Instance(i) = &mut artwork.kind {
                    i.symbol = symbols[state];
                }
                part.artwork = Arc::new(artwork);
            }
        });
    }
}

/// [`blink_rig`] for a **layer rig**: keys through each blink on each eye
/// part's own layer — holding the tweened pose there, so the eye keeps moving
/// with the head ([`buzz_act::layer_rig::key_at`]) — and on every key that
/// layer has, the eye showing the state it should at that frame.
fn blink_layer_rig(scene: &mut Scene, actor: &Actor, total: u32, fps: f64) {
    let starts = blink_starts(&actor.member.key, total, fps);
    for (object, symbols) in &actor.layer_eyes {
        let Some((layer, _)) = scene.find_object(*object) else {
            continue;
        };
        for b in &starts {
            for step in [0u32, 2, 4, 6] {
                if b + step < total {
                    buzz_act::layer_rig::key_at(scene, layer, b + step);
                }
            }
        }
        let keys: Vec<u32> = scene
            .layers()
            .get(layer)
            .map(|l| {
                l.frames
                    .keyframes()
                    .iter()
                    .filter(|k| k.objects.iter().any(|o| o.id == *object))
                    .map(|k| k.start)
                    .collect()
            })
            .unwrap_or_default();
        for frame in keys {
            let state = eye_state(&starts, frame);
            let symbol = symbols[state];
            scene.update_object_at(frame, *object, |target| {
                if let ObjectKind::Instance(i) = &mut target.kind {
                    i.symbol = symbol;
                }
            });
        }
    }
}

/// Add a Blink to the separate eye objects among `layers`, if there are any.
fn add_blinks(scene: &mut Scene, layers: &[LayerId]) -> usize {
    let mut targets: Vec<ObjectId> = Vec::new();
    for id in layers {
        let Some(layer) = scene.layers().get(*id) else {
            continue;
        };
        for object in layer.all_objects() {
            let name = object.name.clone().unwrap_or_default().to_lowercase();
            if name.contains("eye")
                && !name.contains("brow")
                && !matches!(object.kind, ObjectKind::Armature(_))
                && !object.modifiers.iter().any(|m| matches!(m, Modifier::Blink { .. }))
                && !targets.contains(&object.id)
            {
                targets.push(object.id);
            }
        }
    }
    for id in &targets {
        scene.update_object_across(0, u32::MAX, *id, |o| {
            o.modifiers.push(Modifier::Blink {
                rate: 12.0,
                duration: 0.16,
            });
        });
    }
    targets.len()
}

/// Write one character's performances: their entrance and exit, their lines and
/// idling in between.
#[allow(clippy::too_many_arguments)]
fn perform_actor(
    scene: &mut Scene,
    actor: &mut Actor,
    src: &ScriptScene,
    schedule: &Schedule,
    stage: Rect,
    timing: &Timing,
    total: u32,
    report: &mut SceneReport,
) {
    let Some(body) = actor.body else {
        return;
    };
    let fps = timing.fps;
    let walk_speed = (actor.height * 0.55).max(1.0);
    let run_speed = (actor.height * 1.6).max(1.0);
    let half_width = actor.half_width;
    let wing = |side: f64| -> f64 {
        let margin = half_width * 1.1 + stage.width() * 0.06;
        if side < 0.0 {
            stage.x0 - margin
        } else {
            stage.x1 + margin
        }
    };
    let nearer_side = |x: f64| -> f64 {
        if x < stage.center().x { -1.0 } else { 1.0 }
    };

    let mut walks: Vec<Seg> = Vec::new();
    let mut enter_from: Option<f64> = None;
    let mut exit_from: Option<u32> = None;

    // The entrance.
    if let Some(item) = actor.member.enter_item {
        let slot = schedule.slots[item];
        let side = match actor.member.place.enter.as_deref().map(str::to_lowercase).as_deref() {
            Some("left") => -1.0,
            Some("right") => 1.0,
            _ => nearer_side(actor.mark_x),
        };
        let from = wing(side);
        let start = snap_even(slot.start);
        // They must have arrived by the time they first speak, and by the end
        // of the scene.
        let by = actor
            .member
            .lines
            .first()
            .map(|l| l.start)
            .filter(|s| *s > start + 8)
            .unwrap_or(total);
        let p = pace(
            actor.mark_x - from,
            slot.frames,
            by.saturating_sub(start),
            false,
            walk_speed,
            run_speed,
            fps,
        );
        enter_from = Some(from);
        walks.push(Seg {
            range: start..start + p.frames,
            kind: SegKind::Enter {
                run: p.run,
                tempo: p.tempo,
            },
        });
    }
    // The exit.
    if let Some((item, run)) = actor.member.exit {
        let slot = schedule.slots[item];
        let start = snap_even(slot.start);
        let side = nearer_side(actor.mark_x);
        let p = pace(
            wing(side) - actor.mark_x,
            slot.frames,
            total.saturating_sub(start),
            run,
            walk_speed,
            run_speed,
            fps,
        );
        exit_from = Some(start);
        walks.push(Seg {
            range: start..start + p.frames,
            kind: SegKind::Exit {
                run: p.run,
                tempo: p.tempo,
            },
        });
    }
    walks.sort_by_key(|s| s.range.start);

    // The lines: each keeps clear of the walks.
    let mut kept: Vec<Line> = Vec::new();
    for line in &actor.member.lines {
        let range = line.start..line.start + line.frames;
        if exit_from.is_some_and(|gone| line.start >= gone) {
            report.warnings.push(format!(
                "{}: a line at frame {} comes after they have left, so it is captioned but not performed",
                actor.member.key, line.start
            ));
            continue;
        }
        if walks.iter().any(|w| overlaps(&w.range, &range)) {
            report.warnings.push(format!(
                "{}: a line at frame {} overlaps a walk, so it is captioned but not performed",
                actor.member.key, line.start
            ));
            continue;
        }
        kept.push(line.clone());
    }
    let _ = src;

    let mut segs = walks.clone();
    let mut i = 0usize;
    while i < kept.len() {
        let first = kept[i].start;
        let mut end = kept[i].start + kept[i].frames;
        let mut j = i + 1;
        while j < kept.len() {
            let gap_range = end..kept[j].start;
            if kept[j].start <= end + 8 && !walks.iter().any(|w| overlaps(&w.range, &gap_range)) {
                end = kept[j].start + kept[j].frames;
                j += 1;
            } else {
                break;
            }
        }
        // One talk over the run of lines, split only where a shout changes how
        // big it is.
        let mut talks: Vec<Seg> = Vec::new();
        for k in i..j {
            let from = if k == i { first } else { kept[k].start };
            let to = if k + 1 < j { kept[k + 1].start.max(from) } else { end };
            let amount = kept[k].amount;
            match talks.last_mut() {
                Some(Seg {
                    range,
                    kind: SegKind::Talk { amount: was },
                }) if *was == amount && range.end == from => range.end = to,
                _ => talks.push(Seg {
                    range: from..to,
                    kind: SegKind::Talk { amount },
                }),
            }
        }
        segs.extend(talks);
        // The gestures, each a moment into its line and never past its end.
        for line in &kept[i..j] {
            let Some(cue) = &line.cue else {
                continue;
            };
            if cue.action.needs_head() && !has_head_bone(scene, body) {
                report.warnings.push(format!(
                    "{}: the rig has no head bone, so the {} at frame {} (\"{}\") is left out",
                    actor.member.key,
                    cue.action.label().to_lowercase(),
                    line.start,
                    cue.why
                ));
                continue;
            }
            let start = snap_even(line.start + (0.15 * fps).round() as u32).max(line.start);
            let frames = (gesture_seconds(cue.action) * fps).round() as u32;
            let stop = (start + frames).min(line.start + line.frames);
            // Too little of the line left to read a move in: talk instead.
            if stop < start + 6 {
                continue;
            }
            segs.push(Seg {
                range: start..stop,
                kind: SegKind::Gesture {
                    action: cue.action,
                    amount: cue.amount,
                },
            });
            report.gestures.push(GestureNote {
                speaker: actor.member.key.clone(),
                frame: start,
                action: cue.action,
                why: cue.why.clone(),
            });
        }
        i = j;
    }
    // AN10: the moves action lines ask for, where nothing else is happening.
    // (TODO pose variants: a sit/stand could swap to the matching AN4 pose rig
    // at this frame; today the one-shot move is performed on the same rig.)
    let moves = std::mem::take(&mut actor.member.moves);
    for mv in &moves {
        if exit_from.is_some_and(|gone| mv.start >= gone) {
            continue;
        }
        if mv.cue.action.needs_head() && !has_head_bone(scene, body) {
            report.warnings.push(format!(
                "{}: the rig has no head bone, so the {} at frame {} (\"{}\") is left out",
                actor.member.key,
                mv.cue.action.label().to_lowercase(),
                mv.start,
                mv.cue.why
            ));
            continue;
        }
        let frames = (gesture_seconds(mv.cue.action) * fps).round() as u32;
        let range = mv.start..(mv.start + frames).min(total);
        // AN13: a tagged pose wins over the talk it overlaps (not over a walk).
        if mv.cue.why.starts_with("tagged ") {
            segs.retain(|s| !(overlaps(&s.range, &range) && matches!(s.kind, SegKind::Talk { .. } | SegKind::Gesture { .. })));
        }
        if range.end < range.start + 6 || segs.iter().any(|s| overlaps(&s.range, &range)) {
            continue;
        }
        report.gestures.push(GestureNote {
            speaker: actor.member.key.clone(),
            frame: range.start,
            action: mv.cue.action,
            why: mv.cue.why.clone(),
        });
        segs.push(Seg {
            range,
            kind: SegKind::Gesture { action: mv.cue.action, amount: mv.cue.amount },
        });
    }
    // Stable, so a talk is written before a gesture over it.
    segs.sort_by_key(|s| s.range.start);
    actor.member.lines = kept;

    // Frame zero: in the wings if the entrance is first.
    if let Some(from) = enter_from {
        let dx = from - actor.mark_x;
        actor.placed = Affine::translate((dx, 0.0)) * actor.placed;
        actor.x = from;
        let placed = actor.placed;
        buzz_act::layer_rig::set_placement(scene, body, 0, placed);
    }

    let mut cursor = 0u32;
    let mut problem: Option<String> = None;
    let run = |scene: &mut Scene, performance: Performance, placed: Affine, problem: &mut Option<String>| {
        if let Err(e) = perform::apply_from(scene, body, &performance, placed) {
            problem.get_or_insert(format!("{e}"));
        }
    };

    // AN13: once seated (Sit) the body is held: no talk, idle or other gesture
    // writes the standing rest pose over it until a Stand or an exit.
    let mut seated = false;
    for seg in &segs {
        let stands = matches!(seg.kind, SegKind::Gesture { action: Action::Stand, .. } | SegKind::Exit { .. });
        if seated && !stands {
            cursor = cursor.max(seg.range.end);
            continue;
        }
        // AN10: with `still_extras` off a gap holds the pose (no breathing).
        if actor.member.extras && !seated && seg.range.start > cursor + 3 {
            run(
                scene,
                Performance {
                    action: Action::Idle,
                    frames: cursor..seg.range.start,
                    amount: 0.9,
                    tempo: 1.0,
                    distance: 0.0,
                    step: 4,
                },
                actor.placed,
                &mut problem,
            );
        }
        match &seg.kind {
            SegKind::Talk { amount } => run(
                scene,
                Performance {
                    action: Action::Talk,
                    frames: seg.range.clone(),
                    amount: *amount,
                    tempo: 1.0,
                    distance: 0.0,
                    step: 2,
                },
                actor.placed,
                &mut problem,
            ),
            // Sit and Stand are written as the director writes them: the
            // one-shot, and whatever comes after starts from the standing rest
            // pose (see `Action::Sit`).
            SegKind::Gesture { action, amount } => run(
                scene,
                Performance {
                    action: *action,
                    frames: seg.range.clone(),
                    amount: *amount,
                    tempo: 1.0,
                    distance: 0.0,
                    step: 2,
                },
                actor.placed,
                &mut problem,
            ),
            SegKind::Enter { run: fast, tempo: stride } | SegKind::Exit { run: fast, tempo: stride } => {
                let target = match seg.kind {
                    SegKind::Enter { .. } => actor.mark_x,
                    _ => wing(nearer_side(actor.mark_x)),
                };
                let (running, tempo) = (*fast, *stride);
                let dx = target - actor.x;
                actor.face(dx.signum());
                // A figure's own units are not the stage's: its transform scales
                // them, and mirrors them when it faces left.
                let scale_x = actor.placed.as_coeffs()[0];
                let local = if scale_x.abs() > 1e-9 { dx / scale_x } else { dx };
                run(
                    scene,
                    Performance {
                        action: if running { Action::Run } else { Action::Walk },
                        frames: seg.range.clone(),
                        amount: 1.0,
                        tempo,
                        distance: local,
                        step: 2,
                    },
                    actor.placed,
                    &mut problem,
                );
                actor.placed = Affine::translate((dx, 0.0)) * actor.placed;
                actor.x = target;
                if matches!(seg.kind, SegKind::Exit { .. }) {
                    actor.offstage = true;
                }
            }
        }
        if let SegKind::Gesture { action, .. } = &seg.kind {
            match action {
                Action::Sit => seated = true,
                Action::Stand => seated = false,
                _ => {}
            }
        }
        if matches!(seg.kind, SegKind::Exit { .. }) {
            seated = false;
        }
        cursor = cursor.max(seg.range.end);
    }
    if actor.member.extras && !actor.offstage && !seated && cursor + 3 < total {
        run(
            scene,
            Performance {
                action: Action::Idle,
                frames: cursor..total,
                amount: 0.9,
                tempo: 1.0,
                distance: 0.0,
                step: 4,
            },
            actor.placed,
            &mut problem,
        );
    }
    if let Some(p) = problem {
        report
            .warnings
            .push(format!("{}: performance problem: {p}", actor.member.key));
        actor.body = None;
    }
}

/// P2: one scene's cut of the recording, and what it timed.
struct SceneAudio {
    timed: buzz_act::audio_align::AudioSchedule,
    /// The span, padded with the pre- and post-roll: scene frame 0 is its start.
    clip: buzz_audio::Clip,
    /// The mouth shapes heard in [`Self::clip`], on the scene's frames.
    visemes: buzz_audio::VisemeTrack,
    /// The frames of every line timed by the recording, `start..end`.
    ranges: Vec<Range<u32>>,
}

/// The recording's cut for `src`, when the plan has lines for it. Notes say
/// which lines were timed from the audio and which from their text.
fn scene_audio(
    plan: &buzz_act::audio_align::AudioPlan,
    timing: &Timing,
    src: &ScriptScene,
    notes: &mut Vec<String>,
) -> Option<SceneAudio> {
    use buzz_act::audio_align;
    let line_name = |item: usize| match src.items.get(item) {
        Some(Item::Dialogue(d)) => {
            let words: Vec<&str> = d.text.split_whitespace().take(4).collect();
            format!("{} \"{}\u{2026}\" (item {item})", d.speaker, words.join(" "))
        }
        _ => format!("item {item}"),
    };
    let Some(timed) = audio_align::schedule_scene(timing, src, plan) else {
        if plan.lines.iter().any(|l| l.scene == src.number) || plan.scene_spans.contains_key(&src.number) {
            notes.push("audio: the plan's lines for this scene could not be laid out; text timing".to_string());
        }
        return None;
    };
    let path = plan.path_for(src.number);
    let whole = match buzz_audio::Clip::open(path) {
        Ok(c) => c,
        Err(e) => {
            notes.push(format!("audio: {} did not open ({e}); text timing", path.display()));
            return None;
        }
    };
    let clip = audio_align::cut(&whole, timed.span, timed.pre_roll, timed.post_roll)?;
    let visemes = buzz_audio::analyse_visemes(&clip, timing.fps, &buzz_audio::LipSyncOptions::default());
    let ranges = timed
        .schedule
        .slots
        .iter()
        .filter(|s| timed.from_audio.contains(&s.item))
        .map(|s| s.start..s.end())
        .collect();
    notes.push(format!(
        "audio: {} {:.2}\u{2013}{:.2} s (+{:.2} s before, +{:.2} s after); {} line(s) timed from the recording, {} from their text",
        path.display(),
        timed.span.0,
        timed.span.1,
        timed.pre_roll,
        timed.post_roll,
        timed.from_audio.len(),
        timed.from_text.len()
    ));
    for &item in &timed.from_audio {
        notes.push(format!("from audio: {}", line_name(item)));
    }
    for &item in &timed.from_text {
        notes.push(format!("from text: {}", line_name(item)));
    }
    Some(SceneAudio {
        timed,
        clip,
        visemes,
        ranges,
    })
}

/// Put the scene's cut of the recording on a layer of its own, streaming from
/// frame 0 — the same as File ▸ Import Sound places one.
fn attach_scene_sound(scene: &mut Scene, audio: &SceneAudio, src: &ScriptScene, fps: f64, report: &mut SceneReport) {
    let clip = &audio.clip;
    let bytes = Arc::new(buzz_act::audio_align::wav_bytes(clip));
    let name = format!("Dialogue scene {}", src.number);
    let id = scene.add_sound(&name, bytes, "wav", clip.sample_rate, clip.channels, clip.len() as u64);
    let frames = clip.duration_frames(fps.max(1.0));
    if frames == 0 {
        report.warnings.push("audio: the scene's cut of the recording is empty".to_string());
        return;
    }
    let layer = scene.add_stage_layer(name, buzz_scene::LayerKind::Normal);
    scene.update_stage_layer(layer, |l| {
        l.frames.insert_frame(frames.saturating_sub(1));
        if let Some(keyframe) = l.frames.keyframe_at_mut(0) {
            keyframe.sound = Some(buzz_scene::SoundRef::stream(id));
        }
    });
}

/// Write one character's mouth over their lines, closed everywhere else.
///
/// With the scene's recording, a line it timed gets the shapes heard over its
/// frames; a line timed by its text gets the shapes read from its words.
fn sync_mouth(
    scene: &mut Scene,
    actor: &Actor,
    total: u32,
    fps: f64,
    audio: Option<&SceneAudio>,
    report: &mut SceneReport,
) {
    if actor.member.lines.is_empty() && actor.mouth == Mouth::None {
        return;
    }
    // The recording's range a line sits in: the one holding its middle.
    let heard = |l: &Line| -> Option<Range<u32>> {
        let mid = l.start + l.frames / 2;
        audio?.ranges.iter().find(|r| r.contains(&mid)).cloned()
    };
    let spoken: Vec<SpokenLine<'_>> = actor
        .member
        .lines
        .iter()
        .filter(|l| heard(l).is_none())
        .map(|l| SpokenLine {
            start: l.start,
            frames: l.frames,
            text: l.text.as_str(),
        })
        .collect();
    if spoken.is_empty() && actor.member.lines.iter().all(|l| heard(l).is_none()) {
        return;
    }
    // AN10: the report lists every span this character's mouth moves.
    for l in &actor.member.lines {
        match heard(l) {
            Some(r) => report.lipsync.push(LipSpan { speaker: actor.member.key.clone(), start: r.start, end: r.end, source: "audio" }),
            None => report.lipsync.push(LipSpan {
                speaker: actor.member.key.clone(),
                start: l.start,
                end: l.start + l.frames,
                source: "text",
            }),
        }
    }
    let mut track = textsync::line_track(total, fps, &spoken);
    if let Some(a) = audio {
        for range in actor.member.lines.iter().filter_map(|l| heard(l)) {
            for f in range {
                if let Some(slot) = track.frames.get_mut(f as usize) {
                    *slot = a.visemes.at(f);
                }
            }
        }
    }
    match &actor.mouth {
        Mouth::None => report.warnings.push(format!(
            "{}: no mouth found on the rig, so the lines are not lip-synced",
            actor.member.key
        )),
        Mouth::Rig(object) => {
            if actor.body.is_none() {
                return;
            }
            if let Err(e) = textsync::write_rig_mouth(scene, *object, &track, 0) {
                report
                    .warnings
                    .push(format!("{}: lip sync failed: {e}", actor.member.key));
            }
        }
        Mouth::RigLayer(object) => {
            if actor.body.is_none() {
                return;
            }
            if let Err(e) = textsync::write_layer_rig_mouth(scene, *object, &track, 0) {
                report
                    .warnings
                    .push(format!("{}: lip sync failed: {e}", actor.member.key));
            }
        }
        Mouth::Layer {
            layer,
            symbol,
            placement,
        } => {
            buzz_act::lipsync::write_track(scene, &track, 0, *layer, *symbol, *placement);
        }
    }
}

// -- the episode ----------------------------------------------------------------------------

/// **Build the chosen scenes into one document**, one document scene per script
/// scene, each a single undo step.
pub fn build_episode(
    play: &Screenplay,
    casting: &Casting,
    assets: &AssetLibrary,
    numbers: &[u32],
    opts: &BuildOptions,
) -> (Document, Vec<SceneReport>) {
    let mut doc = Document::default();
    doc.edit("Stage", |scene| {
        let stage = scene.stage_mut();
        stage.size = Size::new(opts.width, opts.height);
        stage.frame_rate = opts.fps;
    });
    doc.end_gesture();

    // Sets are dressed from the library's own asset index unless one was given.
    let mut with_index;
    let opts = if opts.index.is_none()
        && let Some(index) = crate::asset_sets::index_for(assets)
    {
        with_index = opts.clone();
        with_index.index = Some(Arc::new(index));
        &with_index
    } else {
        opts
    };

    let mut reports: Vec<SceneReport> = Vec::new();
    let mut carry = Carry::default();
    let mut first = true;
    for &number in numbers {
        let Some(src) = play.scene(number) else {
            reports.push(SceneReport {
                number,
                warnings: vec![format!("the script has no scene {number}")],
                ..Default::default()
            });
            continue;
        };
        if !first {
            doc.add_scene();
        }
        first = false;
        let index = doc.active_scene();
        let mut report = SceneReport::default();
        doc.edit(format!("Build Scene {number}"), |scene| {
            report = build_scene(scene, src, casting, assets, opts, &mut carry);
            // L3 auto-tidy: the built scene's layers into folders, its empty
            // layers gone, drawing order untouched. Part of the same step.
            if opts.tidy_layers {
                scene.arrange_layers(buzz_scene::ArrangeOptions::default());
            }
        });
        doc.end_gesture();
        doc.rename_scene(index, format!("Scene {number} \u{2014} {}", src.title));
        reports.push(report);
    }
    (doc, reports)
}

/// Just the name of a scene's file-safe label, for a preview's file name.
pub fn scene_file_stem(number: u32, title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let slug = slug.trim_matches('_');
    format!("scene{number:02}_{slug}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_found_as_a_whole_word_only() {
        assert_eq!(find_word("raju checks his watch", "raju"), Some(0));
        assert_eq!(find_word("guddu looks at raju", "raju"), Some(15));
        // `man` is not inside `manager` or `woman`.
        assert_eq!(find_word("the manager waits", "man"), None);
        assert_eq!(find_word("a woman waits", "man"), None);
        assert_eq!(find_word("a man waits", "man"), Some(2));
        // A later whole word is found past an earlier part-word.
        assert_eq!(find_word("manager and man", "man"), Some(12));
    }

    #[test]
    fn movement_words_from_the_script_are_read() {
        // Entrances.
        assert_eq!(movement_of("raju, 28, walks briskly toward them."), Movement::Enter);
        assert_eq!(movement_of("raju arrives"), Movement::Enter);
        assert_eq!(movement_of("a customer approaches."), Movement::Enter);
        // Exits, including the plan's `runs toward`.
        assert_eq!(movement_of("a man runs toward it."), Movement::Exit { run: true });
        assert_eq!(movement_of("it flies away."), Movement::Exit { run: false });
        assert_eq!(movement_of("the bus leaves."), Movement::Exit { run: false });
        // Nothing.
        assert_eq!(movement_of("guddu straightens"), Movement::None);
        assert_eq!(movement_of("lalten sips tea."), Movement::None);
        assert_eq!(movement_of("raju checks his wristwatch."), Movement::None);
    }

    #[test]
    fn a_sentence_with_two_subjects_is_two_clauses() {
        let parts = clauses("Guddu straightens as Raju approaches.");
        assert_eq!(parts, vec!["guddu straightens".to_string(), "raju approaches.".to_string()]);
        // A comma does not split: the introduction keeps its subject first.
        assert_eq!(clauses("RAJU, 28, walks briskly toward them.").len(), 1);
    }

    #[test]
    fn a_slugline_chooses_the_setting() {
        let slug = |raw: &str| buzz_act::screenplay::Screenplay::parse(&format!("## SCENE 1 \u{2014} X\n\n**{raw}**\n"))
            .scenes[0]
            .slugline
            .clone()
            .unwrap();
        assert_eq!(setting_for(&slug("EXT. TEA STALL \u{2014} MORNING")), Setting::Daylight);
        assert_eq!(setting_for(&slug("EXT. FIELDS \u{2014} DAWN")), Setting::Sunset);
        assert_eq!(setting_for(&slug("EXT. WIRE \u{2014} EVENING")), Setting::Sunset);
        assert_eq!(setting_for(&slug("EXT. ROAD \u{2014} NIGHT")), Setting::Night);
        assert_eq!(setting_for(&slug("INT. BLOCK OFFICE \u{2014} DAY")), Setting::Interior);
    }

    #[test]
    fn a_walk_that_does_not_fit_is_brisker_and_then_a_run() {
        // Plenty of time: a walk at its natural pace.
        let easy = pace(300.0, 24, 400, false, 100.0, 300.0, 24.0);
        assert!(!easy.run);
        assert_eq!(easy.tempo, 1.0);
        // Not enough: it still finishes inside the time there is.
        let tight = pace(700.0, 24, 40, false, 100.0, 300.0, 24.0);
        assert!(tight.frames <= 40);
        assert!(tight.run, "a walk that cannot fit becomes a run");
        // Never past the time there is, however long the distance.
        assert!(pace(5000.0, 24, 30, false, 100.0, 300.0, 24.0).frames <= 30);
    }

    #[test]
    fn a_long_caption_wraps_into_even_lines_not_a_stranded_word() {
        let style = buzz_text::FontStyle::REGULAR;
        let text = "Hum toh wahi keh rahe hain. Itna seriously kyun le raha hai?";
        let (single, _) = buzz_text::measure_styled(text, 54.0, None, style);
        if single == 0.0 {
            return; // no font on this machine: nothing to measure
        }
        // Room for about three quarters of it: two lines.
        let wrapped = wrap(text, 54.0, style, single * 0.75);
        let rows: Vec<&str> = wrapped.lines().collect();
        assert_eq!(rows.len(), 2, "{wrapped:?}");
        // Neither is a stranded word: each carries at least a third of the text.
        assert!(rows.iter().all(|r| r.len() * 3 >= text.len()), "{rows:?}");
        // And nothing is lost.
        assert_eq!(rows.join(" "), text);
        // A short caption is untouched.
        assert_eq!(wrap("Haan.", 54.0, style, single), "Haan.");
    }

    #[test]
    fn file_stems_are_safe_for_a_preview() {
        assert_eq!(scene_file_stem(4, "THE THREE BEROZGAARS"), "scene04_the_three_berozgaars");
        assert_eq!(scene_file_stem(12, "AFTERNOON: THE THREE INVESTIGATORS"), "scene12_afternoon__the_three_investigators");
    }

    #[test]
    fn a_gesture_is_chosen_from_the_action_line_then_the_words() {
        let bangs = "The conductor bangs the side of the bus.";
        let cases: &[(&str, &str, Option<(Action, f64)>)] = &[
            // The Ep1 lines.
            ("Haan.", "", Some((Action::Nod, 1.0))),
            ("CHALO!", "", None),
            ("Kitne baje jaayegi?", "", Some((Action::Shrug, 0.5))),
            ("CHALO!", bangs, Some((Action::React, 0.6))),
            ("Jab aap sawaal khatam karenge.", "", None),
            ("Jab aap sawaal khatam karenge.", "The man thinks.", None),
            // The words.
            ("Nahi, main nahi jaunga.", "", Some((Action::HeadShake, 1.0))),
            ("No, no.", "", Some((Action::HeadShake, 1.0))),
            ("Namaste, Sharma ji.", "", Some((Action::Namaste, 1.0))),
            ("Aap ko pranam.", "", Some((Action::Namaste, 1.0))),
            ("Ram ram bhaiya.", "", Some((Action::Namaste, 1.0))),
            ("Arre, suno!", "", Some((Action::Wave, 0.6))),
            ("Hey!", "", Some((Action::Wave, 0.6))),
            ("Accha.", "", Some((Action::Nod, 1.0))),
            // A leading haan is a nod, not a question's shrug.
            ("Haan?", "", Some((Action::Nod, 1.0))),
            // The action line wins over the words.
            ("Kya?", "Guddu points at the bus.", Some((Action::Point, 1.0))),
            ("Chalo.", "Raju waves at them.", Some((Action::Wave, 1.0))),
            ("Theek hai.", "Raju sits on the bench.", Some((Action::Sit, 1.0))),
            ("Chalo.", "Raju gets up.", Some((Action::Stand, 1.0))),
            ("Ji.", "Amma nods slowly.", Some((Action::Nod, 1.0))),
            ("Pata nahi.", "Guddu shrugs.", Some((Action::Shrug, 1.0))),
            ("Arre!", "Raju flinches.", Some((Action::React, 1.0))),
            ("Yeh lo.", "Guddu picks up the glass.", Some((Action::Reach, 1.0))),
            ("Bas.", "Raju slams the door.", Some((Action::React, 0.6))),
        ];
        for (line, action, want) in cases {
            assert_eq!(gesture_for(line, action), *want, "{line:?} with {action:?}");
        }
        // The word that chose it is kept for the report.
        assert_eq!(cue_for("CHALO!", bangs).map(|c| c.why), Some("bangs".to_string()));
        assert_eq!(cue_for("Haan.", "").map(|c| c.why), Some("haan".to_string()));
    }

    #[test]
    fn a_shout_is_talked_bigger() {
        assert_eq!(talk_amount("CHALO!"), 1.3);
        assert_eq!(talk_amount("RUKO"), 1.3);
        assert_eq!(talk_amount("Ruko!"), 1.3);
        assert_eq!(talk_amount("Haan."), 1.0);
        assert_eq!(talk_amount("Kitne baje jaayegi?"), 1.0);
        assert_eq!(talk_amount("I."), 1.0);
    }

    fn scene_of(text: &str) -> ScriptScene {
        Screenplay::parse(&format!("## SCENE 1 \u{2014} BUS\n\n{text}"))
            .scenes
            .remove(0)
    }

    fn members_of(text: &str, casting: &str) -> Vec<Member> {
        let src = scene_of(text);
        let casting = Casting::from_json(casting).unwrap();
        let set = casting.scene_set(1);
        let timing = Timing { fps: 24.0, ..Timing::default() };
        let schedule = timing.schedule(&src);
        let mut report = SceneReport::default();
        roster(&src, &schedule, &casting, &set, &mut report)
    }

    fn cues(members: &[Member], key: &str) -> Vec<Option<Action>> {
        let m = members.iter().find(|m| m.key == key).expect("the member");
        m.lines.iter().map(|l| l.cue.as_ref().map(|c| c.action)).collect()
    }

    #[test]
    fn no_gesture_on_two_lines_in_a_row_unless_the_script_asks() {
        let casting = r#"{"characters": {"RAJU": {"asset": "r"}, "GUDDU": {"asset": "g"}}}"#;
        let words = "**RAJU**\nHaan.\n\n**RAJU**\nTheek hai.\n\n**RAJU**\nNahi.\n";
        assert_eq!(
            cues(&members_of(words, casting), "RAJU"),
            vec![Some(Action::Nod), None, Some(Action::HeadShake)]
        );
        // An action line asking for the second keeps it.
        let asked = "**RAJU**\nHaan.\n\nRaju nods again.\n\n**RAJU**\nTheek hai.\n";
        assert_eq!(
            cues(&members_of(asked, casting), "RAJU"),
            vec![Some(Action::Nod), Some(Action::Nod)]
        );
        // An action line about somebody else is not the speaker's.
        let other = "Guddu points at the bus.\n\n**RAJU**\nChalo.\n";
        assert_eq!(cues(&members_of(other, casting), "RAJU"), vec![None]);
        // The bang, from the line before, at 0.6.
        let bang = "The conductor bangs the side of the bus.\n\n**CONDUCTOR**\nCHALO!\n";
        let members = members_of(bang, r#"{"characters": {"CONDUCTOR": {"asset": "c"}}}"#);
        let line = &members[0].lines[0];
        let cue = line.cue.as_ref().expect("a cue");
        assert_eq!((cue.action, cue.amount, cue.why.as_str()), (Action::React, 0.6, "bangs"));
        assert_eq!(line.amount, 1.3, "a shout talks bigger");
    }

    #[test]
    fn gestures_can_be_turned_off_per_character_and_per_scene() {
        let words = "**RAJU**\nHaan.\n\n**GUDDU**\nNamaste!\n";
        let off_raju = r#"{"characters": {"RAJU": {"asset": "r", "gestures": false}, "GUDDU": {"asset": "g"}}}"#;
        let members = members_of(words, off_raju);
        assert_eq!(cues(&members, "RAJU"), vec![None]);
        assert_eq!(cues(&members, "GUDDU"), vec![Some(Action::Namaste)]);
        let off_scene = r#"{"characters": {"RAJU": {"asset": "r"}, "GUDDU": {"asset": "g"}},
                            "scenes": {"1": {"gestures": false}}}"#;
        let members = members_of(words, off_scene);
        assert_eq!(cues(&members, "RAJU"), vec![None]);
        assert_eq!(cues(&members, "GUDDU"), vec![None]);
        // Off is as before: the shout is ordinary size too.
        assert_eq!(members.iter().find(|m| m.key == "GUDDU").unwrap().lines[0].amount, 1.0);
    }

    /// AN11: a reverse shot mirrors the cast's positions about the stage centre.
    #[test]
    fn a_reverse_shot_mirrors_positions() {
        let stage = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let t = angle_restage("reverse", Role::Other, -1.0, stage, 300.0, 900.0, 0.0).expect("reverse re-stages");
        let p = t * Point::new(300.0, 500.0);
        assert!((p.x - 1620.0).abs() < 1e-9 && (p.y - 500.0).abs() < 1e-9, "{p:?}");
        assert!(angle_restage("wide", Role::Other, -1.0, stage, 300.0, 900.0, 0.0).is_none());
    }

    /// AN13: a shot inside an angle picture keeps the picture's full frame.
    #[test]
    fn an_angle_picture_shot_keeps_the_camera_at_full_frame() {
        let stage = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut plan = vec![
            wide_shot(0, stage),
            Shot { start: 30, kind: "medium".into(), on: Some("AMMA".into()), center: Point::new(400.0, 500.0), zoom: 2.16, push: true },
        ];
        let segs = vec![
            AngleSeg { start: 0, end: 30, at_line: 0, camera: "wide".into(), background: "W".into(), horizon: None },
            AngleSeg { start: 30, end: 90, at_line: 1, camera: "over_shoulder".into(), background: "O".into(), horizon: None },
        ];
        apply_angle_framing(&mut plan, &segs, stage, 90);
        let inside: Vec<&Shot> = plan.iter().filter(|s| s.start >= 30).collect();
        assert!(!inside.is_empty() && inside.iter().all(|s| s.zoom <= 1.03 && s.kind == "over_shoulder"), "{plan:?}");
    }

    /// AN11: an angle's background is drawn only in its own span.
    #[test]
    fn an_angle_picture_is_visible_only_in_its_span() {
        let mut set = SceneSet::default();
        set.background = vec!["Wide".into()];
        set.shot_backgrounds = vec![
            ShotBackground { at_line: 0, camera: "wide".into(), background: "Wide".into(), horizon_row: None },
            ShotBackground { at_line: 1, camera: "low_angle".into(), background: "Low".into(), horizon_row: None },
        ];
        let segs = angle_segments(&set, &[0, 12], 24);
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[1].start, segs[1].end, segs[1].background.as_str()), (12, 24, "Low"));
        let mut scene = Scene::default();
        let layer = scene.add_layer("Low", buzz_scene::LayerKind::Normal);
        let id = scene.next_object_id();
        scene.add_object(layer, Object::group(id, Vec::new())).expect("object");
        hide_during(&mut scene, &[layer], &complement(&[(12, 24)], 48), 48);
        let l = scene.layers().get(layer).expect("layer").clone();
        for f in [0, 11, 24, 47] {
            assert!(l.objects_at(f).is_empty(), "hidden at {f}");
        }
        for f in [12, 23] {
            assert!(!l.objects_at(f).is_empty(), "shown at {f}");
        }
    }

    /// A Cast-like rig: the image pipeline's seventeen bones, by name and in
    /// order — or, with no head, just a hips and a chest.
    fn rig(scene: &mut Scene, head: bool) -> ObjectId {
        use buzz_rig::{Armature, Bone, RigPattern};
        scene.stage_mut().frame_rate = 24.0;
        let layer = scene.add_layer("Cast", buzz_scene::LayerKind::Normal);
        let mut armature = Armature::new(Point::new(0.0, 0.0));
        if head {
            // Standing as the pattern draws a person, so the hand-targeted
            // gestures have a body to aim on.
            for slot in RigPattern::biped_full().slots {
                armature.push(Bone::new(slot.name.clone(), slot.parent, 40.0, slot.rest_angle));
            }
        } else {
            armature.push(Bone::new("Hips", None, 40.0, 0.0));
            armature.push(Bone::new("Chest", Some(0), 40.0, 0.0));
        }
        armature.set_rest_here();
        let id = scene.next_object_id();
        let mut object = Object::group(id, Vec::new());
        object.kind = ObjectKind::Armature(buzz_scene::ArmatureData::new(armature));
        scene.add_object(layer, object).expect("the rig goes on the layer")
    }

    /// Build `text` for RAJU on a rig, the way `build_scene` performs a cast
    /// member, and hand back what came out.
    fn perform_raju(text: &str, head: bool) -> (Scene, ObjectId, Line, SceneReport) {
        let src = scene_of(text);
        let casting = Casting::from_json(r#"{"characters": {"RAJU": {"asset": "r"}, "CONDUCTOR": {"asset": "c"}}}"#).unwrap();
        let set = casting.scene_set(1);
        let timing = Timing { fps: 24.0, ..Timing::default() };
        let schedule = timing.schedule(&src);
        let mut report = SceneReport::default();
        let member = roster(&src, &schedule, &casting, &set, &mut report)
            .into_iter()
            .find(|m| m.key == "RAJU")
            .expect("Raju is in the scene");
        let mut scene = Scene::default();
        let body = rig(&mut scene, head);
        let mut actor = Actor {
            member,
            played_by: "Cast \"Raju\"".into(),
            body: Some(body),
            mouth: Mouth::None,
            placed: Affine::IDENTITY,
            x: 960.0,
            mark_x: 960.0,
            drawn: 0.0,
            facing: 0.0,
            height: 400.0,
            half_width: 80.0,
            offstage: false,
            eyes: Vec::new(),
            layer_eyes: Vec::new(),
            blinks: false,
            top: Vec::new(),
            layers: Vec::new(),
            variant: false,
            pose_spans: Vec::new(),
        };
        let stage = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let total = schedule.total_frames;
        perform_actor(&mut scene, &mut actor, &src, &schedule, stage, &timing, total, &mut report);
        let line = actor.member.lines[0].clone();
        (scene, body, line, report)
    }

    /// How far the right hand is above the right shoulder, along the rig's
    /// own up (hips toward head), on whatever key governs `frame`.
    fn right_hand_above_shoulder(scene: &Scene, body: ObjectId, frame: u32) -> f64 {
        use buzz_act::Joint;
        let layer = scene.find_object(body).expect("the rig").0;
        let objects = scene.layers().get(layer).expect("the layer").frames.objects_at(frame);
        let ObjectKind::Armature(rig) = &objects.first().expect("the rig on that frame").kind else {
            panic!("not a rig");
        };
        let a = &rig.armature;
        let up = a.tip(Joint::Head.index()) - a.head(Joint::Hips.index());
        let up = up / up.hypot();
        (a.tip(Joint::ElbowR.index()) - a.head(Joint::ShoulderR.index())).dot(up)
    }

    fn is_key(scene: &Scene, body: ObjectId, frame: u32) -> bool {
        let layer = scene.find_object(body).expect("the rig").0;
        scene.layers().get(layer).expect("the layer").frames.is_keyframe(frame)
    }

    #[test]
    fn a_gesture_lands_inside_its_line_with_talk_either_side() {
        let text = "The conductor bangs the side of the bus.\n\n**CONDUCTOR**\nCHALO!\n\n\
                    **RAJU**\nArre suno, yeh bus kitne baje jaayegi, humko aaj hi bazaar tak pahunchna hai \
                    aur Amma ke liye dawai bhi leni hai, bahut zaroori kaam hai.\n";
        let (scene, body, line, report) = perform_raju(text, true);
        assert_eq!(report.gestures.len(), 1, "{:?}", report.gestures);
        let g = &report.gestures[0];
        assert_eq!((g.speaker.as_str(), g.action, g.why.as_str()), ("RAJU", Action::Wave, "arre"));
        let line_end = line.start + line.frames;
        let stop = g.frame + (1.4f64 * 24.0).round() as u32;
        assert!(line.start < g.frame && stop < line_end, "{line:?} against {g:?}");
        assert_eq!(g.frame, line.start + 4, "0.15 s into the line, on an even frame");

        // Measured by where the hand is, not by a shoulder angle: the
        // gestures are hand targets solved on the rig, so the angle that puts
        // a hand somewhere depends on the rig, and where it is does not.
        // Talk keys before it, inside the line: the hand below the shoulder.
        assert!(is_key(&scene, body, line.start));
        let low = right_hand_above_shoulder(&scene, body, line.start);
        assert!(low < 0.0, "a talking hand stays below the shoulder: {low}");
        // The wave's keys inside it: the hand above the shoulder. "Arre" is
        // a small wave (amount 0.6, which scales the solved angles), so the
        // hand clears the shoulder rather than reaching full height.
        let mid = g.frame + 16;
        assert!(is_key(&scene, body, mid));
        let up = right_hand_above_shoulder(&scene, body, mid);
        assert!(up > 0.0, "the arm is up for the wave: {up}");
        // And talk again after it.
        assert!(is_key(&scene, body, stop));
        let low = right_hand_above_shoulder(&scene, body, stop);
        assert!(low < 0.0, "a talking hand stays below the shoulder: {low}");
        assert!(is_key(&scene, body, snap_even(line_end - 4)));

        // The report says what was done and why.
        let md = report_markdown("Ep", &[report.clone()], 24.0);
        assert!(md.contains("RAJU \u{2014} frame") && md.contains("Wave (\"arre\")"), "{md}");
    }

    #[test]
    fn a_rig_with_no_head_bone_gets_no_nod_and_a_note() {
        let text = "**RAJU**\nHaan, main abhi aata hoon, thoda ruko bhai, bus mat chhodna, bas do minute.\n";
        let (scene, body, line, report) = perform_raju(text, false);
        assert!(report.gestures.is_empty(), "{:?}", report.gestures);
        assert!(
            report.warnings.iter().any(|w| w.contains("no head bone") && w.contains("nod")),
            "{:?}",
            report.warnings
        );
        // The line is still talked.
        assert!(is_key(&scene, body, line.start));
        // A rig with a head nods on the same line.
        let (_, _, _, headed) = perform_raju(text, true);
        assert_eq!(headed.gestures.len(), 1);
        assert_eq!(headed.gestures[0].action, Action::Nod);
    }

    #[test]
    fn standing_on_a_spot_puts_the_feet_there() {
        let bounds = Rect::new(100.0, 200.0, 300.0, 1000.0);
        let (t, scale) = stand_on(bounds, 400.0, 960.0, 972.0);
        assert!((scale - 0.5).abs() < 1e-9);
        let feet = t * Point::new(200.0, 1000.0);
        assert!((feet.x - 960.0).abs() < 1e-9 && (feet.y - 972.0).abs() < 1e-9, "{feet:?}");
        let head = t * Point::new(200.0, 200.0);
        assert!((head.y - 572.0).abs() < 1e-9, "{head:?}");
    }

    /// AN10: only the speaker has lines (so only their mouth is written), the
    /// narrator has none, and an action line's move lands on its subject.
    #[test]
    fn only_the_speaker_has_mouth_lines_and_narration_none() {
        let casting = r#"{"characters": {"RAJU": {"asset": "r"}, "GUDDU": {"asset": "g"},
                          "NARRATOR": {"voice_only": true}}}"#;
        let text = "**NARRATOR (V.O.)**\nRaju aur Guddu bus ka intezaar karte hain.\n\n\
                    **RAJU**\nBus late hai.\n\nGuddu sits on the bench.\n\n\
                    **NARRATOR (V.O.)**\nAur phir bus aayi.\n";
        let members = members_of(text, casting);
        assert!(members.iter().all(|m| m.key != "NARRATOR"), "the narrator is never on stage");
        let raju = members.iter().find(|m| m.key == "RAJU").expect("raju");
        let guddu = members.iter().find(|m| m.key == "GUDDU").expect("guddu");
        assert_eq!(raju.lines.len(), 1);
        assert!(guddu.lines.is_empty(), "the non-speaker has no mouth lines");
        // The action line moves Guddu (Sit); Raju is not asked to move.
        assert_eq!(guddu.moves.len(), 1);
        assert_eq!(guddu.moves[0].cue.action, Action::Sit);
        assert!(raju.moves.is_empty());
        assert!(raju.extras && guddu.extras);
        // `still_extras: false` is carried to every member.
        let off = casting.replace("{\"characters\"", "{\"still_extras\": false, \"characters\"");
        assert!(members_of(text, &off).iter().all(|m| !m.extras));
    }
}
