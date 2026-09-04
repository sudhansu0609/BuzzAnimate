//! The director: a story in, a staged and animated scene out.
//!
//! # What this is
//!
//! [`direct`] reads a few lines of ordinary prose —
//!
//! ```text
//! Night. Ana walks in from the left.
//! Ana talks to Ben. Ben listens.
//! Ben walks off right.
//! ```
//!
//! — and does what an animator would do with that brief: set the scene
//! ([`crate::staging`]), stand the named cast in it, and write the walks,
//! talks and waits onto the timeline as ordinary pose keyframes
//! ([`crate::perform`]), timed one after another the way the story tells
//! them. The result is layers, shapes and keyframes like everything else in
//! the document: one Ctrl+Z takes all of it back, and every frame of it can
//! be edited afterwards.
//!
//! # What "understanding" means here, honestly
//!
//! There is no language model in this crate and nothing is learned. The
//! parser is a keyword grammar: it knows the setting words, a few dozen verbs
//! sorted into four actions, the direction words, and that a capitalised word
//! it has no other explanation for is somebody's name. That covers the way
//! people actually write a brief — subject, verb, colour — and it fails
//! loudly rather than cleverly: every sentence it could not read is listed in
//! the report, because a director who silently skips a line of the script is
//! worse than one who asks.
//!
//! # The schedule
//!
//! Sentences run in story order. Each actor has a clock; a sentence starts
//! when everyone in it is free, and "meanwhile" starts it alongside the
//! previous one instead. Someone spoken *to* listens — an idle over the same
//! frames — because a character frozen solid while being talked at is the
//! most obvious way an automatic scene gives itself away. For the same
//! reason, everyone left standing when their part ends idles quietly to the
//! end of the shot: breathing, not stopped.

use buzz_geom::{Affine, Point};
use buzz_scene::{CameraKey, Scene};

use crate::perform::{self, Action, Performance};
use crate::staging::{self, SceneRecipe, Setting, StagedScene};

/// One thing one actor does, planned onto real frames.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedBeat {
    /// Index into the cast.
    pub actor: usize,
    pub action: Action,
    pub frames: std::ops::Range<u32>,
    /// How far this beat moves the actor, in world x. Zero for standing
    /// actions.
    pub travel: f64,
}

/// What the director built.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirectedScene {
    pub staged: StagedScene,
    /// The cast's names, in the order they first appear in the story.
    pub names: Vec<String>,
    pub beats: Vec<PlannedBeat>,
    /// How long the shot came out, in frames.
    pub frames: u32,
    /// The sentences that meant nothing to the parser, verbatim, so the user
    /// can see what was skipped rather than wonder.
    pub ignored: Vec<String>,
    pub message: String,
}

/// Why nothing could be directed.
#[derive(Debug, Clone, PartialEq)]
pub enum DirectError {
    /// No sentence produced an actor or an action.
    NothingUnderstood,
}

impl std::fmt::Display for DirectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirectError::NothingUnderstood => write!(
                f,
                "no line of that story named someone doing something \u{2014} try \
                 \u{201c}Ana walks in from the left. Ana talks to Ben.\u{201d}"
            ),
        }
    }
}

impl std::error::Error for DirectError {}

// ---------------------------------------------------------------------------
// The story, parsed
// ---------------------------------------------------------------------------

/// Which way a move goes, before the stage's geometry is known.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Move {
    /// Walk on from off-stage. `-1.0` is the left wing, `1.0` the right.
    Enter(f64),
    /// Walk off. Same convention.
    Exit(f64),
    /// Toward another actor, stopping at conversational distance.
    Toward(usize),
    /// To the mirrored position on the other side of the stage.
    Across,
    /// Some distance the way they are facing — "Ana walks."
    Wander,
}

#[derive(Debug, Clone, PartialEq)]
enum EventKind {
    Travel { running: bool, movement: Move },
    Talk { listener: Option<usize> },
    Idle,
}

#[derive(Debug, Clone, PartialEq)]
struct Event {
    actor: usize,
    kind: EventKind,
    /// An explicit "for three seconds", if the sentence gave one.
    seconds: Option<f64>,
    /// Starts alongside the previous sentence instead of after it.
    simultaneous: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Story {
    setting: Setting,
    /// The writer mentioned cloud, or a sky worth looking at.
    clouds: bool,
    /// The writer put water in the shot: a river, a lake, a shore.
    water: bool,
    names: Vec<String>,
    events: Vec<Event>,
    ignored: Vec<String>,
}

/// Words that are capitalised for reasons other than being a name.
///
/// Sentence-initial words are capitalised by grammar, so any of these at the
/// front of a sentence must not become a character. Lowercased for the
/// comparison.
const NOT_NAMES: &[&str] = &[
    "a", "an", "the", "and", "then", "meanwhile", "while", "after", "before", "later", "suddenly",
    "next", "now", "finally", "he", "she", "they", "it", "i", "we", "his", "her", "their", "at",
    "in", "on", "to", "from", "into", "out", "off", "night", "day", "morning", "evening",
    "sunset", "dusk", "dawn", "noon", "afternoon", "inside", "outside", "interior", "left",
    "right", "two", "three", "four", "five", "one", "scene", "stage", "there", "everyone",
    "somebody", "someone", "nobody", "moon", "moonlight", "stars", "rain", "snow", "wind", "sun",
    "sky", "street", "room", "kitchen", "office", "house", "city", "park", "forest", "both",
];

/// Nouns that stand for an unnamed character: "a man walks in" casts one, and
/// "the man" thereafter is the same man.
const ANONYMOUS: &[&str] = &[
    "man", "woman", "boy", "girl", "person", "figure", "stranger", "child", "kid", "guard",
    "friend", "character",
];

fn split_sentences(story: &str) -> Vec<String> {
    story
        .split(['.', '!', '?', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn words_of(sentence: &str) -> Vec<String> {
    sentence
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The setting, from the first sentence that names one.
fn setting_of(sentences: &[String]) -> Setting {
    for sentence in sentences {
        let s = sentence.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| s.contains(w));
        // **A storm before a night**, because a storm *is* a night and the
        // word that matters is the more specific one: "a stormy night" is a
        // storm, and matching "night" first would take the lightning out of it.
        if has(&["storm", "stormy", "thunder", "lightning", "tempest", "downpour"]) {
            return Setting::Storm;
        }
        // Night before day: "one night, in broad daylight, he dreamed" is a
        // story about a night.
        if has(&["night", "midnight", "moonlit", "moonlight", "dark"]) {
            return Setting::Night;
        }
        if has(&["sunset", "dusk", "evening", "golden hour", "sundown"]) {
            return Setting::Sunset;
        }
        if has(&["inside", "indoors", "interior", "room", "kitchen", "office", "house"]) {
            return Setting::Interior;
        }
        if has(&["day", "morning", "noon", "afternoon", "sunny", "daylight", "outside"]) {
            return Setting::Daylight;
        }
    }
    Setting::Sunset
}

/// An explicit duration, if the sentence spells one out: "for 3 seconds".
fn seconds_of(lower: &str) -> Option<f64> {
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '.')
        .filter(|w| !w.is_empty())
        .collect();
    for pair in words.windows(2) {
        if pair[1].starts_with("second") || pair[1] == "sec" || pair[1] == "secs" {
            if let Ok(n) = pair[0].parse::<f64>() {
                return Some(n.clamp(0.3, 30.0));
            }
            let spelled = match pair[0] {
                "a" | "one" => 1.0,
                "two" => 2.0,
                "three" => 3.0,
                "four" => 4.0,
                "five" => 5.0,
                "six" => 6.0,
                "ten" => 10.0,
                _ => continue,
            };
            return Some(spelled);
        }
    }
    None
}

/// Any of `stems` as a *word prefix* in the sentence.
///
/// Stems rather than words so "walks", "walked" and "walking" all answer to
/// "walk" — and a prefix match against whole words, not a substring search,
/// so "swalk" in some name cannot become a verb.
fn has_stem(words_lower: &[String], stems: &[&str]) -> bool {
    words_lower
        .iter()
        .any(|w| stems.iter().any(|s| w.starts_with(s)))
}

fn parse(story: &str) -> Story {
    let sentences = split_sentences(story);
    let setting = setting_of(&sentences);
    // **What is in the shot besides the people.** Both of these are one live
    // modifier per object rather than any keyframes, so a story that mentions
    // a river gets a moving river at no cost to the schedule below — see
    // `staging::water`.
    //
    // A storm arrives with cloud whether the writer said so or not, but that
    // rule lives in `staging::build` rather than here: it has to hold however
    // the scene is made, and a copy of it in each caller is a copy that gets
    // out of step.
    let scenery = sentences.join(" ").to_lowercase();
    let mentions = |words: &[&str]| words.iter().any(|w| scenery.contains(w));
    let clouds = mentions(&["cloud", "overcast", "clouded", "sky above", "clear sky"]);
    let water = mentions(&[
        "river", "stream", "creek", "canal", "lake", "sea", "ocean", "shore", "beach",
        "riverbank", "water",
    ]);

    let mut names: Vec<String> = Vec::new();
    let mut anonymous: Vec<(String, usize)> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();
    let mut last_actor: Option<usize> = None;

    for sentence in &sentences {
        let words = words_of(sentence);
        let words_lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let lower = sentence.to_lowercase();

        // -- who ---------------------------------------------------------
        // Mentions are collected **in word order**, because word order is
        // grammar: in "The man talks to Ana", the first body found is the
        // subject and the second is who he is talking to. Two kinds of
        // mention: capitalised words with no other explanation are names,
        // and "a man" casts an unnamed character — whom "the man" refers to
        // ever after. The cast is capped where the staging crate caps it; a
        // sixth character is a crowd, and a crowd is scenery.
        let mut mentioned: Vec<usize> = Vec::new();
        for word in &words {
            let key = word.to_lowercase();
            let found = if word.chars().next().is_some_and(char::is_uppercase)
                && !NOT_NAMES.contains(&key.as_str())
                && !ANONYMOUS.contains(&key.as_str())
            {
                match names.iter().position(|n| n.eq_ignore_ascii_case(word)) {
                    Some(i) => Some(i),
                    None if names.len() < 5 => {
                        names.push(word.clone());
                        Some(names.len() - 1)
                    }
                    None => None,
                }
            } else if ANONYMOUS.contains(&key.as_str()) {
                match anonymous.iter().find(|(n, _)| *n == key) {
                    Some((_, i)) => Some(*i),
                    None if names.len() < 5 => {
                        let mut label = key.clone();
                        if let Some(first) = label.get_mut(0..1) {
                            first.make_ascii_uppercase();
                        }
                        names.push(format!("The {label}"));
                        anonymous.push((key.clone(), names.len() - 1));
                        Some(names.len() - 1)
                    }
                    None => None,
                }
            } else {
                None
            };
            if let Some(i) = found
                && !mentioned.contains(&i)
            {
                mentioned.push(i);
            }
        }

        // -- what --------------------------------------------------------
        let running = has_stem(
            &words_lower,
            &["run", "dash", "sprint", "hurr", "rush", "race", "flee", "jog", "bolt"],
        );
        let walking = has_stem(
            &words_lower,
            &[
                "walk", "stroll", "enter", "arriv", "come", "leav", "exit", "cross", "approach",
                "head", "step", "wander", "move",
            ],
        ) || words_lower
            .iter()
            // "go" is too short to be a stem — "good" and "gold" would walk —
            // so its forms are matched whole.
            .any(|w| matches!(w.as_str(), "go" | "goes" | "going" | "went" | "gone"));
        let talking = has_stem(
            &words_lower,
            &[
                "talk", "say", "speak", "spoke", "tell", "chat", "argu", "explain", "whisper",
                "shout", "ask", "repl", "greet", "call", "answer", "discuss",
            ],
        );
        let idling = has_stem(
            &words_lower,
            &[
                "wait", "stand", "paus", "listen", "stay", "breath", "watch", "think", "idle",
                "look", "rest", "stop",
            ],
        );

        let actor = mentioned.first().copied().or(last_actor);
        let Some(actor) = actor else {
            // A sentence about nobody: scenery ("Night."), or noise. Setting
            // sentences are not worth reporting as failures.
            if !(running || walking || talking || idling) {
                continue;
            }
            ignored.push(sentence.clone());
            continue;
        };

        let kind = if running || walking {
            let toward_other = mentioned.iter().find(|i| **i != actor).copied();
            let side = if lower.contains("left") { -1.0 } else { 1.0 };
            let movement = if has_stem(&words_lower, &["enter", "arriv", "come"])
                || lower.contains(" in ")
                || lower.ends_with(" in")
            {
                Move::Enter(if lower.contains("right") { 1.0 } else { -1.0 })
            } else if has_stem(&words_lower, &["leav", "exit"])
                || lower.contains(" off ")
                || lower.ends_with(" off")
                || lower.contains(" away")
                || lower.contains(" out")
            {
                Move::Exit(side)
            } else if let Some(other) = toward_other {
                Move::Toward(other)
            } else if lower.contains("across") {
                Move::Across
            } else {
                Move::Wander
            };
            EventKind::Travel { running, movement }
        } else if talking {
            EventKind::Talk {
                listener: mentioned.iter().find(|i| **i != actor).copied(),
            }
        } else if idling {
            EventKind::Idle
        } else {
            // Somebody named, doing nothing the grammar knows. Reported, so
            // the writer can rephrase rather than wonder.
            ignored.push(sentence.clone());
            last_actor = Some(actor);
            continue;
        };

        events.push(Event {
            actor,
            kind,
            seconds: seconds_of(&lower),
            simultaneous: lower.starts_with("meanwhile")
                || lower.starts_with("while")
                || lower.contains("at the same time"),
        });
        last_actor = Some(actor);
    }

    Story {
        setting,
        clouds,
        water,
        names,
        events,
        ignored,
    }
}

// ---------------------------------------------------------------------------
// The stage direction
// ---------------------------------------------------------------------------

/// Where one actor is and which way they face, threaded beat to beat.
///
/// Threaded here rather than read back from the document because a
/// performance writes *keyframes*: the object found on the timeline between
/// beats is whichever keyframe governs, and asking it "where are you now?"
/// mid-schedule answers for the wrong frame. The director already knows,
/// because the director put them there.
struct ActorState {
    /// The object's placement for the next beat: position and mirror.
    placed: Affine,
    /// World x, kept alongside `placed` for arithmetic.
    x: f64,
    /// `1.0` facing right, `-1.0` facing left, as the transform mirrors it.
    facing: f64,
    /// The next free frame on this actor's clock.
    cursor: u32,
    /// Off after they exit: an actor who has left is not made to idle in the
    /// wings.
    on_stage: bool,
}

impl ActorState {
    /// Turn to face `direction`, by mirroring about their own feet.
    fn face(&mut self, direction: f64) {
        if direction != 0.0 && direction.signum() != self.facing {
            self.placed = self.placed * Affine::scale_non_uniform(-1.0, 1.0);
            self.facing = direction.signum();
        }
    }

    /// Move `dx` along the stage, in world units.
    fn advance(&mut self, dx: f64) {
        self.placed = Affine::translate((dx, 0.0)) * self.placed;
        self.x += dx;
    }
}

/// **Direct a story.**
///
/// Builds the set, casts everyone the story names, and writes their
/// performances onto the timeline in story order. One call; the caller wraps
/// it in one `Document::edit` so the whole scene is one undo step.
/// **Frame the shot** — where the camera looks, and when it cuts.
///
/// # Why the director should do this at all
///
/// A staged, performed scene with a locked-off camera is a stage play seen from
/// row H. It is not wrong, and it is not a film: what makes a shot read is that
/// the camera is *near the thing that matters*, and that it changes when the
/// thing that matters changes. That is a decision, but it is a decision with an
/// obvious default — look at whoever is doing something — and the obvious
/// default is exactly what an animator should not have to type out for every
/// beat of every shot.
///
/// # The rules, and they are few
///
/// * **Somebody talking is who the shot is about.** The camera comes in on
///   them: closer, centred on the figure. Between two speakers this **cuts** —
///   keys on adjacent frames, so the change happens between one frame and the
///   next, because a camera that drifts across the room during a conversation is
///   a camera nobody asked for.
/// * **Somebody walking is followed.** Keys at both ends of the beat, so the
///   camera moves with them — a pan, not a cut, because the movement is the
///   point of the beat.
/// * **Nothing else moves the camera.** An idle holds whatever framing it
///   inherited. A shot with no talking and no walking gets one wide key and
///   stays there, which is the locked-off camera it had before.
///
/// # It never frames tighter than the figure
///
/// The zoom is worked out from the actor's own height so a close shot is close
/// *on them* rather than by some number of pixels, and it is bounded so nobody's
/// head leaves the frame. Guessing a framing that cuts somebody's head off is
/// worse than not framing at all.
///
/// Returns how many camera keys were written. Nothing is written for a shot
/// with no cast, which has nothing to look at.
pub fn frame_the_shot(scene: &mut Scene, directed: &DirectedScene) -> usize {
    if directed.staged.cast.is_empty() {
        return 0;
    }
    let stage = scene.stage().stage_rect();
    let centre = stage.center();

    /// How much of the frame's height a figure fills in a close shot.
    const CLOSE_FILL: f64 = 0.72;
    /// And how much of it a walk is framed at — room to travel into.
    const WIDE_FILL: f64 = 0.45;
    /// Never past this, whatever the arithmetic says: a shot that is more
    /// magnified than this is somebody's chin.
    const MAX_ZOOM: f64 = 2.2;

    // Where an actor is, and how tall, at a frame.
    let look_at = |scene: &Scene, actor: usize, frame: u32| -> Option<(Point, f64)> {
        let (layer, _) = directed.staged.cast.get(actor)?;
        let bounds = scene.layers().get(*layer)?.bounds_at(frame)?;
        Some((bounds.center(), bounds.height().max(1.0)))
    };

    let zoom_for = |height: f64, fill: f64| {
        ((stage.height() * fill) / height).clamp(1.0, MAX_ZOOM)
    };

    let mut keys: Vec<CameraKey> = Vec::new();
    fn push(keys: &mut Vec<CameraKey>, frame: u32, at: Point, zoom: f64) {
        let mut key = CameraKey::new(frame, at);
        key.zoom = zoom;
        keys.push(key);
    }

    // The shot opens wide on the whole stage, so the first cut has something to
    // cut *from*.
    push(&mut keys, 0, centre, 1.0);

    for beat in &directed.beats {
        match beat.action {
            Action::Talk => {
                let Some((at, height)) = look_at(scene, beat.actor, beat.frames.start) else {
                    continue;
                };
                let zoom = zoom_for(height, CLOSE_FILL);
                // A cut: the frame before keeps the old framing, and this one
                // has the new. Adjacent keys give no time to interpolate.
                if beat.frames.start > 0
                    && let Some(previous) = keys.last().copied()
                {
                    push(&mut keys, beat.frames.start - 1, previous.center, previous.zoom);
                }
                push(&mut keys, beat.frames.start, at, zoom);
            }
            Action::Walk | Action::Run => {
                // Followed, at both ends, so the camera travels with them.
                if let Some((from, height)) = look_at(scene, beat.actor, beat.frames.start) {
                    push(&mut keys, beat.frames.start, from, zoom_for(height, WIDE_FILL));
                }
                if beat.frames.end > beat.frames.start + 1
                    && let Some((to, height)) = look_at(scene, beat.actor, beat.frames.end - 1)
                {
                    push(&mut keys, beat.frames.end - 1, to, zoom_for(height, WIDE_FILL));
                }
            }
            // Standing still is not a reason to move the camera.
            Action::Idle => {}
        }
    }

    // A beat that starts on frame 0 lands on the opening wide key; the later
    // one is the framing that was actually chosen, so it wins. Deduplicated
    // here rather than left to `set_key` so the count below is honest.
    let mut seen: Vec<CameraKey> = Vec::new();
    for key in keys {
        match seen.iter_mut().find(|k| k.frame == key.frame) {
            Some(existing) => *existing = key,
            None => seen.push(key),
        }
    }
    let keys = seen;

    // Every key still the opening wide one means nothing was worth cutting to,
    // and a locked-off camera says that with less machinery than a camera track
    // holding one key.
    let untouched = keys
        .iter()
        .all(|k| k.zoom == 1.0 && k.center == centre);
    if untouched {
        return 0;
    }

    let written = keys.len();
    let camera = scene.camera_mut();
    camera.enabled = true;
    for key in keys {
        camera.set_key(key.clamped());
    }
    written
}

/// **One shot of a longer brief**: what to call it, and the prose that makes it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedShot {
    /// A name for the scene, taken from the shot's own first words so the
    /// timeline reads like the brief rather than "Scene 1, Scene 2, Scene 3".
    pub title: String,
    /// The sentences of this shot, which [`direct`] reads as a whole brief.
    pub story: String,
}

/// **Cut a brief into shots.**
///
/// # Why a film is not one long shot
///
/// [`direct`] stages one scene and animates the cast in it, which is a shot. A
/// story is several: the cast changes, the place changes, and the camera cuts.
/// Given a longer brief this splits it the way the writing already does, so a
/// page of prose becomes an animatic rather than one impossibly busy scene.
///
/// # Where the cuts go
///
/// Two marks, both of which people already use without being asked:
///
/// * A **blank line**. Paragraphs are how prose separates beats, and a writer
///   who has put a gap between two of them has already said they are apart.
/// * A **line that is only a setting** — "Night.", "Interior." — which is the
///   screenplay's own slug line, doing exactly this job.
///
/// A brief with neither is one shot, which is what it was before this existed
/// and what a two-sentence description should stay.
///
/// The setting carries forward: a shot that does not name a time of day is in
/// the same one as the shot before it, because that is what a reader assumes
/// and re-establishing it every paragraph is not how anybody writes.
pub fn split_shots(story: &str) -> Vec<PlannedShot> {
    let mut shots: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    // The setting in force, so a later shot that does not restate it keeps it.
    let mut standing: Option<String> = None;

    let push = |current: &mut Vec<String>, shots: &mut Vec<Vec<String>>| {
        if current.iter().any(|l| !l.trim().is_empty()) {
            shots.push(std::mem::take(current));
        } else {
            current.clear();
        }
    };

    for line in story.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            push(&mut current, &mut shots);
            continue;
        }
        if is_setting_line(trimmed) {
            // A slug line starts a shot and stands as its setting.
            push(&mut current, &mut shots);
            standing = Some(trimmed.to_string());
            current.push(trimmed.to_string());
            continue;
        }
        // A shot that opens without a setting inherits the standing one.
        if current.is_empty()
            && let Some(setting) = &standing
            && !is_setting_line(trimmed)
        {
            current.push(setting.clone());
        }
        current.push(trimmed.to_string());
    }
    push(&mut current, &mut shots);

    shots
        .into_iter()
        .map(|lines| {
            let story = lines.join("\n");
            PlannedShot {
                title: shot_title(&lines),
                story,
            }
        })
        .collect()
}

/// Is this line nothing but a setting — the screenplay's slug line?
///
/// Deliberately strict: three words at most, and every one of them either a
/// setting word or punctuation. "Night." is a slug; "Night falls and Ana walks
/// in" is a sentence that happens to open with one, and cutting there would
/// throw away the action.
fn is_setting_line(line: &str) -> bool {
    let words = words_of(line);
    if words.is_empty() || words.len() > 3 {
        return false;
    }
    const SETTING_WORDS: &[&str] = &[
        "night", "evening", "dark", "midnight", "dusk", "sunset", "sundown",
        "interior", "inside", "indoors", "kitchen", "room", "day", "daylight",
        "morning", "noon", "afternoon", "exterior", "outside", "later", "then",
    ];
    words
        .iter()
        .all(|w| SETTING_WORDS.contains(&w.to_lowercase().as_str()))
}

/// A short name for a shot, from its own words.
///
/// The first line that is not just a setting, cut to a few words — so a scene
/// list reads "Ana walks in", not "Scene 2".
fn shot_title(lines: &[String]) -> String {
    let line = lines
        .iter()
        .find(|l| !is_setting_line(l))
        .or_else(|| lines.first())
        .map(|l| l.as_str())
        .unwrap_or("Shot");
    let words: Vec<&str> = line.split_whitespace().take(5).collect();
    let title = words.join(" ");
    let title = title.trim_end_matches(['.', ',', ';', ':']).to_string();
    if title.is_empty() {
        "Shot".to_string()
    } else {
        title
    }
}

pub fn direct(scene: &mut Scene, story: &str) -> Result<DirectedScene, DirectError> {
    let parsed = parse(story);
    if parsed.names.is_empty() || parsed.events.is_empty() {
        return Err(DirectError::NothingUnderstood);
    }

    let fps = scene.stage().frame_rate.max(1.0);
    let stage = scene.stage().stage_rect();

    // The set, with the cast standing on it. The staged marks are each
    // actor's home position; entrances and crossings move them from there.
    let recipe = SceneRecipe {
        setting: parsed.setting,
        cast: parsed.names.len(),
        // Provisional: every layer is stretched to the real total at the end,
        // once the schedule has decided what the total is.
        frames: 48,
        // What the writer put in the shot besides the people. Both are live
        // motion, so neither of them is on the schedule below.
        clouds: parsed.clouds,
        water: parsed.water,
        ..SceneRecipe::default()
    };
    let staged = staging::build(scene, &recipe);

    // Name the cast what the story called them, on the layer and the object
    // both: a timeline that says "Ana" is a timeline the writer can read.
    for (i, (layer, id)) in staged.cast.iter().enumerate() {
        if let Some(name) = parsed.names.get(i) {
            scene.update_stage_layer(*layer, |l| l.name = name.clone());
            scene.update_object_at(0, *id, |o| o.name = Some(name.clone()));
        }
    }

    let mut actors: Vec<ActorState> = staged
        .cast
        .iter()
        .map(|(_, id)| {
            let placed = scene
                .find_object(*id)
                .map(|(_, o)| o.transform)
                .unwrap_or(Affine::IDENTITY);
            let coeffs = placed.as_coeffs();
            ActorState {
                placed,
                x: coeffs[4],
                facing: if coeffs[0] < 0.0 { -1.0 } else { 1.0 },
                cursor: 0,
                on_stage: true,
            }
        })
        .collect();

    // A body is roughly this tall on stage; walking pace follows from it.
    // Real people walk about half their height a second and run at well over
    // twice that, and those two numbers are what make an automatic walk
    // cover ground in the time a walk should.
    let height = stage.height() * recipe.figure_scale;
    let walk_speed = (height * 0.55).max(1.0);
    let run_speed = (height * 1.6).max(1.0);
    let offstage = stage.width() * 0.16;
    // Conversational distance: close enough to be talking, far enough not to
    // overlap silhouettes.
    let gap = height * 0.42;

    let mut beats: Vec<PlannedBeat> = Vec::new();
    let mut prev_start = 0u32;

    for event in &parsed.events {
        if event.actor >= actors.len() {
            continue;
        }

        // Who is in this sentence decides when it can start.
        let listener = match event.kind {
            EventKind::Talk { listener } => listener.filter(|l| *l < actors.len()),
            _ => None,
        };
        // A free actor starts alongside whatever else is happening — two
        // entrances in two sentences arrive together, which is how a scene
        // opens — and a busy one waits for their own last beat to finish.
        // "Meanwhile" pins the start to the previous sentence's start, but
        // never earlier than the actor is free: one body, one schedule.
        let mut start = actors[event.actor].cursor;
        if let Some(l) = listener {
            start = start.max(actors[l].cursor);
        }
        if event.simultaneous {
            start = start.max(prev_start);
        }

        match &event.kind {
            EventKind::Travel { running, movement } => {
                let actor = event.actor;

                // The destination is decided from where they stand *now* —
                // for an entrance that is their mark, read before they are
                // moved to the wings below.
                let target = match movement {
                    // Back to their mark: the position staging chose, which
                    // is where the rest of the scene expects them.
                    Move::Enter(_) => actors[actor].x,
                    Move::Exit(side) => {
                        if *side < 0.0 {
                            stage.x0 - offstage
                        } else {
                            stage.x1 + offstage
                        }
                    }
                    Move::Toward(other) => {
                        let other_x = actors[*other].x;
                        other_x + gap * (actors[actor].x - other_x).signum()
                    }
                    Move::Across => stage.center().x * 2.0 - actors[actor].x,
                    Move::Wander => {
                        // A stretch of stage the way they face, turned back
                        // at the edges so "Ana walks" never walks her out of
                        // the shot.
                        let stride = stage.width() * 0.3;
                        let mut to = actors[actor].x + stride * actors[actor].facing;
                        let margin = stage.width() * 0.08;
                        if to > stage.x1 - margin || to < stage.x0 + margin {
                            to = actors[actor].x - stride * actors[actor].facing;
                        }
                        to
                    }
                };

                // An entrance begins in the wings. Honoured only as the
                // actor's first beat: mid-story, "comes in" from wherever
                // they are would be a teleport, and a walk from where they
                // stand is the honest reading.
                if let Move::Enter(side) = movement
                    && actors[actor].cursor == 0
                {
                    let wing = if *side < 0.0 {
                        stage.x0 - offstage
                    } else {
                        stage.x1 + offstage
                    };
                    let from_mark = wing - actors[actor].x;
                    actors[actor].advance(from_mark);
                    // From frame zero: they must not be discovered standing
                    // at their mark before their entrance.
                    let placed = actors[actor].placed;
                    if let Some((_, id)) = staged.cast.get(actor) {
                        scene.update_object_at(0, *id, |o| o.transform = placed);
                    }
                }

                let dx = target - actors[actor].x;
                if dx.abs() < height * 0.05 {
                    // Already there. Nothing to write, nothing to report.
                    continue;
                }

                let speed = if *running { run_speed } else { walk_speed };
                let seconds = event
                    .seconds
                    .unwrap_or((dx.abs() / speed).clamp(0.6, 8.0));
                let frames = ((seconds * fps).round() as u32).max(4);
                let range = start..start + frames;

                actors[actor].face(dx.signum());
                let performance = Performance {
                    action: if *running { Action::Run } else { Action::Walk },
                    frames: range.clone(),
                    amount: 1.0,
                    tempo: 1.0,
                    // Positive is the way the figure faces, and the actor
                    // has just turned to face the way this goes.
                    distance: dx.abs(),
                    step: 2,
                };
                if let Some((_, id)) = staged.cast.get(actor) {
                    let _ = perform::apply_from(scene, *id, &performance, actors[actor].placed);
                }
                actors[actor].advance(dx);
                actors[actor].cursor = range.end;
                actors[actor].on_stage = !matches!(movement, Move::Exit(_));
                beats.push(PlannedBeat {
                    actor,
                    action: performance.action,
                    frames: range,
                    travel: dx,
                });
            }

            EventKind::Talk { .. } => {
                let actor = event.actor;
                let seconds = event.seconds.unwrap_or(3.2);
                let frames = ((seconds * fps).round() as u32).max(8);
                let range = start..start + frames;

                // Speaker and listener face each other. A conversation held
                // back to back is a very particular story, and not the one
                // anybody typed.
                if let Some(l) = listener {
                    let between = actors[l].x - actors[actor].x;
                    actors[actor].face(between.signum());
                    actors[l].face(-between.signum());
                }

                let talk = Performance {
                    action: Action::Talk,
                    frames: range.clone(),
                    amount: 1.0,
                    tempo: 1.0,
                    distance: 0.0,
                    step: 2,
                };
                if let Some((_, id)) = staged.cast.get(actor) {
                    let _ = perform::apply_from(scene, *id, &talk, actors[actor].placed);
                }
                actors[actor].cursor = range.end;
                beats.push(PlannedBeat {
                    actor,
                    action: Action::Talk,
                    frames: range.clone(),
                    travel: 0.0,
                });

                // **The listener listens.** Quieter than the speaker — this
                // is a weight shift and breathing, not a second speech.
                if let Some(l) = listener {
                    let listen = Performance {
                        action: Action::Idle,
                        frames: range.clone(),
                        amount: 0.8,
                        tempo: 1.0,
                        distance: 0.0,
                        step: 2,
                    };
                    if let Some((_, id)) = staged.cast.get(l) {
                        let _ = perform::apply_from(scene, *id, &listen, actors[l].placed);
                    }
                    actors[l].cursor = range.end;
                    beats.push(PlannedBeat {
                        actor: l,
                        action: Action::Idle,
                        frames: range,
                        travel: 0.0,
                    });
                }
            }

            EventKind::Idle => {
                let actor = event.actor;
                let seconds = event.seconds.unwrap_or(1.8);
                let frames = ((seconds * fps).round() as u32).max(6);
                let range = start..start + frames;
                let idle = Performance {
                    action: Action::Idle,
                    frames: range.clone(),
                    amount: 1.0,
                    tempo: 1.0,
                    distance: 0.0,
                    step: 2,
                };
                if let Some((_, id)) = staged.cast.get(actor) {
                    let _ = perform::apply_from(scene, *id, &idle, actors[actor].placed);
                }
                actors[actor].cursor = range.end;
                beats.push(PlannedBeat {
                    actor,
                    action: Action::Idle,
                    frames: range,
                    travel: 0.0,
                });
            }
        }

        prev_start = start;
    }

    if beats.is_empty() {
        return Err(DirectError::NothingUnderstood);
    }

    // The shot runs a beat past the last action, and never shorter than two
    // seconds: a scene that cuts on the very frame the last thing happens
    // reads as a mistake.
    let total = beats
        .iter()
        .map(|b| b.frames.end)
        .max()
        .unwrap_or(0)
        .max((fps * 2.0) as u32)
        + (fps * 0.5) as u32;

    // **Nobody freezes.** An actor still on stage whose part ended early
    // idles to the end of the shot — breathing, not stopped — which is most
    // of the difference between a directed scene and a diorama.
    for (i, state) in actors.iter_mut().enumerate() {
        if !state.on_stage || state.cursor + 6 >= total {
            continue;
        }
        let range = state.cursor..total;
        let idle = Performance {
            action: Action::Idle,
            frames: range.clone(),
            amount: 0.9,
            tempo: 1.0,
            distance: 0.0,
            step: 2,
        };
        if let Some((_, id)) = staged.cast.get(i) {
            let _ = perform::apply_from(scene, *id, &idle, state.placed);
        }
        state.cursor = total;
        beats.push(PlannedBeat {
            actor: i,
            action: Action::Idle,
            frames: range,
            travel: 0.0,
        });
    }

    // Every stage layer as long as the shot — the sky has to outlast the
    // story told under it.
    let last = total.saturating_sub(1);
    let layers: Vec<buzz_scene::LayerId> = scene.stage_layers().iter().map(|l| l.id).collect();
    for layer in layers {
        scene.update_stage_layer(layer, |l| {
            if l.frames.length() <= last {
                l.frames.insert_frame(last);
            }
        });
    }

    let seconds = total as f64 / fps;
    let mut message = format!(
        "{}: {} in the cast, {} beat(s) over {seconds:.1}s",
        parsed.setting.label(),
        parsed.names.join(", "),
        beats.len(),
    );
    if !parsed.ignored.is_empty() {
        message.push_str(&format!(
            " \u{2014} couldn't read: \u{201c}{}\u{201d}",
            parsed.ignored.join("\u{201d}, \u{201c}")
        ));
    }

    let directed = DirectedScene {
        staged,
        names: parsed.names,
        beats,
        frames: total,
        ignored: parsed.ignored,
        message,
    };

    // **And then it is shot.** Staging and performing put the scene on the
    // stage; framing is what makes it a shot rather than a stage play seen from
    // row H. A scene with nothing worth cutting to keeps its locked-off camera.
    frame_the_shot(scene, &directed);

    Ok(directed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::ObjectKind;

    fn thigh_angle(scene: &Scene, frame: u32, object: buzz_scene::ObjectId) -> Option<f64> {
        let found = scene.layers().iter().find_map(|l| {
            l.frames
                .objects_at(frame)
                .iter()
                .find(|o| o.id == object)
                .cloned()
        })?;
        match &found.kind {
            ObjectKind::Armature(rig) => {
                Some(rig.armature.bones[crate::figure::Joint::ThighL.index()].angle)
            }
            _ => None,
        }
    }

    /// The whole promise, in one test: a story in, a staged scene with named
    /// actors and keyframed performances out.
    #[test]
    fn a_story_becomes_a_staged_animated_scene() {
        let mut scene = Scene::default();
        let directed = direct(
            &mut scene,
            "Night. Ana walks in from the left. Ana talks to Ben. Ben walks off right.",
        )
        .expect("the story is readable");

        assert_eq!(directed.names, vec!["Ana", "Ben"]);
        assert!(directed.ignored.is_empty(), "ignored: {:?}", directed.ignored);
        assert!(
            directed.beats.len() >= 3,
            "an entrance, a talk, a listen and an exit: {:?}",
            directed.beats
        );
        assert!(directed.frames >= 48);

        // The set is a night set.
        assert!(directed.staged.backdrop.is_some());

        // The timeline really holds keys.
        let keys: usize = scene
            .stage_layers()
            .iter()
            .map(|l| l.frames.keyframe_count())
            .sum();
        assert!(keys > 20, "the beats should be keyframed, got {keys}");
    }

    /// Ana enters from the left wing: at frame 0 she is off stage, and by the
    /// end of her entrance she is standing on her mark.
    #[test]
    fn an_entrance_starts_in_the_wings_and_ends_on_the_mark() {
        let mut scene = Scene::default();
        let directed = direct(&mut scene, "Ana walks in from the left. Ana waits.").unwrap();
        let (_, ana) = directed.staged.cast[0];
        let stage = scene.stage().stage_rect();

        let x_at = |scene: &Scene, frame: u32| -> f64 {
            scene
                .layers()
                .iter()
                .find_map(|l| {
                    l.frames
                        .objects_at(frame)
                        .iter()
                        .find(|o| o.id == ana)
                        .map(|o| o.transform.as_coeffs()[4])
                })
                .expect("Ana is somewhere")
        };

        let entrance = directed
            .beats
            .iter()
            .find(|b| b.travel.abs() > 0.0)
            .expect("an entrance beat");
        assert!(
            x_at(&scene, 0) < stage.x0 + 1.0,
            "at frame 0 she is in the wings: {}",
            x_at(&scene, 0)
        );
        let arrived = x_at(&scene, entrance.frames.end.saturating_sub(2));
        assert!(
            arrived > stage.x0 + stage.width() * 0.1,
            "by the end of the walk she is on stage: {arrived}"
        );
    }

    /// Someone spoken to is not a statue: the listener gets keys over the
    /// same frames the speaker talks.
    #[test]
    fn the_listener_listens_while_the_speaker_talks() {
        let mut scene = Scene::default();
        let directed = direct(&mut scene, "Ana talks to Ben.").unwrap();

        let talk = directed
            .beats
            .iter()
            .find(|b| b.action == Action::Talk)
            .expect("a talk beat");
        let listen = directed
            .beats
            .iter()
            .find(|b| b.action == Action::Idle && b.actor != talk.actor)
            .expect("the listener idles");
        assert_eq!(talk.frames, listen.frames, "over the same frames");
    }

    /// Beats for one actor come one after another, never overlapping: a
    /// person cannot walk and talk two schedules at once.
    #[test]
    fn one_actors_beats_never_overlap() {
        let mut scene = Scene::default();
        let directed = direct(
            &mut scene,
            "Ana walks in from the left. Ana talks to Ben. Ana walks off right. Ben waits.",
        )
        .unwrap();

        for a in 0..directed.names.len() {
            let mut ranges: Vec<_> = directed
                .beats
                .iter()
                .filter(|b| b.actor == a)
                .map(|b| b.frames.clone())
                .collect();
            ranges.sort_by_key(|r| r.start);
            for pair in ranges.windows(2) {
                assert!(
                    pair[0].end <= pair[1].start,
                    "actor {a} overlaps: {pair:?}"
                );
            }
        }
    }

    /// Nobody freezes: an actor whose part ends early idles to the end of
    /// the shot.
    #[test]
    fn everyone_still_on_stage_breathes_to_the_end() {
        let mut scene = Scene::default();
        let directed = direct(&mut scene, "Ana talks to Ben for 2 seconds. Ana waits.").unwrap();

        for (i, _) in directed.names.iter().enumerate() {
            let last_end = directed
                .beats
                .iter()
                .filter(|b| b.actor == i)
                .map(|b| b.frames.end)
                .max()
                .unwrap_or(0);
            assert!(
                last_end >= directed.frames.saturating_sub(1),
                "actor {i} stops at {last_end} of {}",
                directed.frames
            );
        }
    }

    /// The settings really steer the set.
    #[test]
    fn the_setting_words_choose_the_setting() {
        for (text, expected) in [
            ("One night, Ana waits.", Setting::Night),
            ("At sunset, Ana waits.", Setting::Sunset),
            ("In the kitchen, Ana waits.", Setting::Interior),
            ("One sunny morning, Ana waits.", Setting::Daylight),
        ] {
            let parsed = parse(text);
            assert_eq!(parsed.setting, expected, "for {text:?}");
        }
    }

    /// "A man" is cast, and "the man" is the same man.
    #[test]
    fn an_unnamed_character_is_cast_once() {
        let parsed = parse("A man walks in from the right. The man talks to Ana.");
        assert_eq!(parsed.names.len(), 2, "the man and Ana: {:?}", parsed.names);
        assert_eq!(parsed.events.len(), 2);
        assert_eq!(
            parsed.events[0].actor, parsed.events[1].actor,
            "both sentences are about the same man"
        );
    }

    /// A sentence the grammar cannot read is reported, not swallowed.
    #[test]
    fn what_could_not_be_read_is_said() {
        let mut scene = Scene::default();
        let directed = direct(
            &mut scene,
            "Ana walks in from the left. Ana contemplates the ineffable.",
        )
        .unwrap();
        assert_eq!(directed.ignored.len(), 1);
        assert!(directed.message.contains("couldn't read"));
    }

    /// A story with nothing in it says so instead of building an empty set.
    #[test]
    fn an_unreadable_story_is_an_error() {
        let mut scene = Scene::default();
        let layers_before = scene.stage_layers().len();
        assert!(direct(&mut scene, "").is_err());
        assert!(direct(&mut scene, "The rain fell.").is_err());
        assert_eq!(
            scene.stage_layers().len(),
            layers_before,
            "and nothing was built on the way to saying so"
        );
    }

    /// Explicit durations are honoured: "for 4 seconds" is four seconds of
    /// frames at the stage's rate.
    #[test]
    fn an_explicit_duration_is_honoured() {
        let mut scene = Scene::default();
        let fps = scene.stage().frame_rate.max(1.0);
        let directed = direct(&mut scene, "Ana talks to Ben for 4 seconds.").unwrap();
        let talk = directed
            .beats
            .iter()
            .find(|b| b.action == Action::Talk)
            .unwrap();
        assert_eq!(talk.frames.len() as u32, (4.0 * fps) as u32);
    }

    /// The walk really lands on the rig: the thigh swings somewhere in the
    /// stride. Sampled at several frames, not one — a walk's sine passes
    /// through its rest angle twice a cycle, and a single unlucky sample
    /// (the exact midpoint of an odd cycle count is one) lands on a zero.
    #[test]
    fn a_directed_walk_actually_poses_the_figure() {
        let mut scene = Scene::default();
        let directed = direct(&mut scene, "Ana walks in from the left.").unwrap();
        let (_, ana) = directed.staged.cast[0];
        let walk = &directed.beats[0];

        let rest = thigh_angle(&scene, walk.frames.start, ana).expect("Ana has a skeleton");
        let len = walk.frames.len() as u32;
        let swung = (1..8)
            .map(|i| walk.frames.start + len * i / 8)
            .filter_map(|f| thigh_angle(&scene, f, ana))
            .any(|angle| (angle - rest).abs() > 0.02);
        assert!(swung, "the thigh never left {rest} across the whole walk");
    }
}


#[cfg(test)]
mod framing_tests {
    use super::*;

    fn shot(story: &str) -> (Scene, DirectedScene) {
        let mut scene = Scene::default();
        let directed = direct(&mut scene, story).expect("it directs");
        (scene, directed)
    }

    /// A scene where somebody talks gets a camera: the shot is about them.
    ///
    /// A brief whose talking starts on the very first frame has nothing to cut
    /// *from*, so it opens on the speaker rather than cutting to them — one
    /// key, and a closer one than the stage.
    #[test]
    fn a_conversation_is_shot_rather_than_watched_from_row_h() {
        let (scene, _) = shot("Ana talks to Ben.");
        assert!(scene.camera().enabled, "the shot is framed");
        assert!(
            scene.camera().keys().iter().any(|k| k.zoom > 1.0),
            "and framed on the speaker rather than on the whole stage"
        );
    }

    /// **The cut is a cut.** Two keys on adjacent frames leave no room to
    /// interpolate, which is what stops the camera drifting across the room in
    /// the middle of a conversation.
    #[test]
    fn coming_in_on_a_speaker_cuts_rather_than_drifts() {
        let (scene, _) = shot("Ana waits for 2 seconds. Ana talks to Ben.");
        let frames: Vec<u32> = scene.camera().keys().iter().map(|k| k.frame).collect();
        assert!(
            frames.windows(2).any(|w| w[1] == w[0] + 1),
            "there should be a pair of adjacent keys — a cut. Got {frames:?}"
        );
    }

    /// A close shot is closer than the wide one it cut from. Given something
    /// to open on first, so there is a wide framing for the cut to leave.
    #[test]
    fn the_shot_comes_in_on_whoever_is_talking() {
        let (scene, _) = shot("Ana walks in from the left.
Ana talks to Ben.");
        let zooms: Vec<f64> = scene.camera().keys().iter().map(|k| k.zoom).collect();
        let widest = zooms.iter().copied().fold(f64::MAX, f64::min);
        let closest = zooms.iter().copied().fold(0.0f64, f64::max);
        assert!(
            closest > widest,
            "it should come in on the speaker: {zooms:?}"
        );
    }

    /// **And never so close that somebody loses their head.** A framing that
    /// guessed a crop is worse than no framing at all.
    #[test]
    fn it_never_frames_tighter_than_a_person() {
        let (scene, _) = shot("Ana talks to Ben. Ben talks to Ana.");
        for key in scene.camera().keys() {
            assert!(
                key.zoom <= 2.2,
                "a zoom of {} is somebody's chin",
                key.zoom
            );
            assert!(key.zoom >= 1.0, "and never further out than the stage");
        }
    }

    /// A walk is followed rather than cut to: keys at both ends of the beat, so
    /// the camera travels with the actor.
    #[test]
    fn a_walk_is_followed() {
        let (scene, directed) = shot("Ana walks in from the left.");
        let walk = directed
            .beats
            .iter()
            .find(|b| b.action == Action::Walk)
            .expect("a walk was planned");
        let frames: Vec<u32> = scene.camera().keys().iter().map(|k| k.frame).collect();
        assert!(
            frames.contains(&walk.frames.start),
            "keyed where the walk starts: {frames:?}"
        );
        assert!(
            frames.iter().any(|f| *f >= walk.frames.end - 1),
            "and where it ends: {frames:?}"
        );
    }

    /// Standing still is not a reason to move the camera. A shot with nothing
    /// but an idle keeps the locked-off camera it always had.
    #[test]
    fn a_still_scene_keeps_its_locked_off_camera() {
        let (scene, _) = shot("Ana waits.");
        assert!(
            !scene.camera().enabled,
            "nothing happened that was worth cutting to"
        );
    }
}

#[cfg(test)]
mod sequence_tests {
    use super::*;

    #[test]
    fn a_short_brief_is_one_shot() {
        let shots = split_shots("Night. Ana walks in from the left.");
        assert_eq!(shots.len(), 1, "nothing said to cut");
    }

    #[test]
    fn a_blank_line_starts_a_new_shot() {
        let shots = split_shots(
            "Night. Ana walks in from the left.\n\nBen walks off right.",
        );
        assert_eq!(shots.len(), 2);
        assert!(shots[1].story.contains("Ben walks off"));
    }

    #[test]
    fn a_setting_on_its_own_line_starts_a_new_shot() {
        let shots = split_shots(
            "Night.\nAna walks in from the left.\nDay.\nBen walks off right.",
        );
        assert_eq!(shots.len(), 2, "the slug line cuts");
        assert!(shots[0].story.starts_with("Night."));
        assert!(shots[1].story.starts_with("Day."));
    }

    /// A sentence that merely *opens* with a setting word is action, not a slug
    /// line, and cutting there would throw the action away.
    #[test]
    fn a_sentence_beginning_with_a_setting_is_not_a_cut() {
        let shots = split_shots("Night falls and Ana walks in from the left.");
        assert_eq!(shots.len(), 1);
        assert!(shots[0].story.contains("Ana walks in"));
    }

    /// The setting carries forward, because that is what a reader assumes and
    /// nobody restates the time of day every paragraph.
    #[test]
    fn a_later_shot_keeps_the_setting_in_force() {
        let shots = split_shots("Night.\nAna waits.\n\nBen walks off right.");
        assert_eq!(shots.len(), 2);
        assert!(
            shots[1].story.to_lowercase().contains("night"),
            "the second shot is still at night: {:?}",
            shots[1].story
        );
    }

    /// A scene list should read like the brief, not like "Scene 1, Scene 2".
    #[test]
    fn a_shot_is_named_after_its_own_words() {
        let shots = split_shots("Night.\nAna walks in from the left.\n\nBen waits.");
        assert_eq!(shots[0].title, "Ana walks in from the");
        assert_eq!(shots[1].title, "Ben waits");
    }

    #[test]
    fn blank_lines_alone_are_no_shots_at_all() {
        assert!(split_shots("").is_empty());
        assert!(split_shots("\n\n   \n").is_empty());
    }
}
