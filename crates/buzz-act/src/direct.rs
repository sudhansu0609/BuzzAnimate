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

use buzz_geom::Affine;
use buzz_scene::Scene;

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

    Ok(DirectedScene {
        staged,
        names: parsed.names,
        beats,
        frames: total,
        ignored: parsed.ignored,
        message,
    })
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

