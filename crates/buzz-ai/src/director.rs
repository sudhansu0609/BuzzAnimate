//! Directing through the local LLM (Chunk C11).
//!
//! Natural language brief ("Ramu walks from the hut to the tree, says line 3, looks at the moon")
//! -> LLM produces structured JSON list of existing director verbs
//! -> Validates, previews, and applies as one undoable edit with lip sync and C6/C7 gaze & gesture controls.

use serde::{Deserialize, Serialize};

use buzz_act::direct::{self, DirectedScene, DirectError};
use buzz_act::perform::Action;
use buzz_scene::Scene;

use crate::llm::{Llm, LlmError};

/// Available high-level director verbs mapped to buzz_act staging and perform actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectorVerb {
    EnterLeft,
    EnterRight,
    WalkTo,
    WalkAcross,
    WalkAway,
    RunTo,
    RunAway,
    Talk,
    Listen,
    Idle,
    Sit,
    Stand,
    Point,
    Wave,
    Reach,
    React,
    Turn,
    LookAt,
}

impl DirectorVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnterLeft => "enter_left",
            Self::EnterRight => "enter_right",
            Self::WalkTo => "walk_to",
            Self::WalkAcross => "walk_across",
            Self::WalkAway => "walk_away",
            Self::RunTo => "run_to",
            Self::RunAway => "run_away",
            Self::Talk => "talk",
            Self::Listen => "listen",
            Self::Idle => "idle",
            Self::Sit => "sit",
            Self::Stand => "stand",
            Self::Point => "point",
            Self::Wave => "wave",
            Self::Reach => "reach",
            Self::React => "react",
            Self::Turn => "turn",
            Self::LookAt => "look_at",
        }
    }
}

/// A single directed action beat performed by an actor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectorBeat {
    pub actor: String,
    pub verb: DirectorVerb,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialogue: Option<String>,
    /// Normalized gaze offset (dx, dy) in -1.0..=1.0 for look_at controls (C6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gaze: Option<(f64, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}

/// Structured story script produced by local LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectorScript {
    pub setting: String,
    pub scenery: String,
    pub cast: Vec<String>,
    pub beats: Vec<DirectorBeat>,
}

/// Strict JSON schema for LLM director script generation.
pub fn director_verbs_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "setting": {
                "type": "string",
                "enum": ["day", "night", "morning", "evening", "interior", "exterior", "dusk", "dawn"]
            },
            "scenery": {
                "type": "string",
                "enum": ["forest", "street", "field", "room", "village", "city", "none"]
            },
            "cast": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1
            },
            "beats": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "actor": { "type": "string" },
                        "verb": {
                            "type": "string",
                            "enum": [
                                "enter_left", "enter_right", "walk_to", "walk_across", "walk_away",
                                "run_to", "run_away", "talk", "listen", "idle", "sit", "stand",
                                "point", "wave", "reach", "react", "turn", "look_at"
                            ]
                        },
                        "target": { "type": "string" },
                        "dialogue": { "type": "string" },
                        "gaze": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "seconds": { "type": "number" }
                    },
                    "required": ["actor", "verb"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["setting", "scenery", "cast", "beats"],
        "additionalProperties": false
    })
}

/// Queries local LLM with natural language brief to produce structured director script.
pub fn parse_story_to_script(
    llm: &impl Llm,
    model: &str,
    natural_language_brief: &str,
) -> Result<DirectorScript, LlmError> {
    let system_prompt = "You are a professional animation director. Given a natural language story brief, \
convert it into a structured sequence of director verbs. \
Identify the setting (day, night, morning, evening, interior, exterior), scenery (forest, street, field, room, village, city), \
the cast of character names, and the chronological list of beats using the allowed verbs: \
enter_left, enter_right, walk_to, walk_across, walk_away, run_to, run_away, talk, listen, idle, sit, stand, point, wave, reach, react, turn, look_at. \
Whenever the brief quotes what someone says (text in double quotes), add a `talk` beat for that speaker whose `dialogue` \
is the quoted words copied exactly, without the quote marks; never leave `dialogue` out of a talk beat the brief quotes. \
Use `target` for whom or where a verb is aimed (walk_to, run_to, point, look_at, talk). \
Output strictly valid JSON according to the schema.";

    let schema = director_verbs_schema();

    let mut script: DirectorScript = llm.chat_json(
        model,
        system_prompt,
        natural_language_brief,
        &[],
        schema,
    )?;
    fill_dialogue_from_brief(&mut script, natural_language_brief);

    Ok(script)
}

/// Quoted speech in a brief, in order, with the cast member who says it
/// (`None` when no cast name is near the quote).
pub fn quoted_lines(brief: &str, cast: &[String]) -> Vec<(Option<String>, String)> {
    let chars: Vec<(usize, char)> = brief.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start_byte, c) = chars[i];
        // Single quotes count only as a quotation (not an apostrophe) when
        // one opens after a space or a colon and the other closes before a
        // non-letter: `talks to him: 'Namaste, Baba.' The Sadhu…`.
        let opens_single = c == '\''
            && (i == 0 || matches!(chars[i - 1].1, ' ' | ':' | '\n' | '(' | '\u{2014}'));
        let close = match c {
            '"' => '"',
            '\u{201C}' => '\u{201D}',
            '\u{2018}' => '\u{2019}',
            '\'' if opens_single => '\'',
            _ => {
                i += 1;
                continue;
            }
        };
        let closes_here = |k: usize| {
            chars[k].1 == close
                && (close != '\'' || chars.get(k + 1).is_none_or(|n| !n.1.is_alphanumeric()))
        };
        let Some(j) = (i + 1..chars.len()).find(|&k| closes_here(k)) else {
            break;
        };
        let text_start = start_byte + c.len_utf8();
        let text = brief[text_start..chars[j].0].trim().to_string();
        let after_byte = chars[j].0 + close.len_utf8();
        if !text.is_empty() {
            // The speaker: the last cast name before the quote in the same
            // sentence (Ramu says, "..."), else the first one right after it
            // ("..." says Ramu).
            let before_from = brief[..start_byte]
                .rfind(['.', '!', '?', '\n'])
                .map(|p| p + 1)
                .unwrap_or(0);
            let before = brief[before_from..start_byte].to_lowercase();
            let after_to = brief[after_byte..]
                .find(['.', '!', '?', '\n'])
                .map(|p| after_byte + p)
                .unwrap_or(brief.len());
            let after = brief[after_byte..after_to].to_lowercase();
            let speaker = cast
                .iter()
                .filter_map(|n| before.rfind(&n.to_lowercase()).map(|p| (p, n)))
                .max_by_key(|(p, _)| *p)
                .map(|(_, n)| n.clone())
                .or_else(|| {
                    cast.iter()
                        .filter_map(|n| after.find(&n.to_lowercase()).map(|p| (p, n)))
                        .min_by_key(|(p, _)| *p)
                        .map(|(_, n)| n.clone())
                });
            out.push((speaker, text));
        }
        i = j + 1;
    }
    out
}

fn normalise_line(t: &str) -> String {
    t.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// **The fallback for a model that leaves `dialogue` empty** (F6a: qwen
/// directed 6/6 verbs right but dropped every line). Every quoted line of
/// the brief that no beat already carries goes onto the speaker's next
/// `talk` beat without dialogue, or onto a new `talk` beat at the end.
/// Returns how many lines were lifted.
pub fn fill_dialogue_from_brief(script: &mut DirectorScript, brief: &str) -> usize {
    let mut lifted = 0;
    for (speaker, text) in quoted_lines(brief, &script.cast) {
        let key = normalise_line(&text);
        let carried = script.beats.iter().any(|b| {
            b.dialogue.as_deref().is_some_and(|d| {
                let d = normalise_line(d);
                !d.is_empty() && (d.contains(&key) || key.contains(&d))
            })
        });
        if carried {
            continue;
        }
        let speaker = speaker
            .or_else(|| script.cast.first().cloned())
            .unwrap_or_else(|| "Narrator".to_string());
        if let Some(beat) = script.beats.iter_mut().find(|b| {
            b.verb == DirectorVerb::Talk
                && b.dialogue.as_deref().is_none_or(|d| d.trim().is_empty())
                && b.actor.eq_ignore_ascii_case(&speaker)
        }) {
            beat.dialogue = Some(text);
        } else {
            script.beats.push(DirectorBeat {
                actor: speaker,
                verb: DirectorVerb::Talk,
                target: None,
                dialogue: Some(text),
                gaze: None,
                seconds: None,
            });
        }
        lifted += 1;
    }
    lifted
}

/// Converts structured DirectorScript into canonical prose readable by `buzz_act::direct::direct`.
/// **The name an actor goes by in the prose `direct` reads.** `direct` casts
/// each capitalised word as a person, so "Old Man" became two people ("Old"
/// and "The Man") and "old man" nobody — and that actor's lines never became
/// cues. Each word is capitalised and the words joined: "Old Man" and
/// "old man" both read as `OldMan`. `dialogue_cues` matches by the same key.
pub fn stage_name(actor: &str) -> String {
    actor
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect()
}

pub fn script_to_story_text(script: &DirectorScript) -> String {
    let mut prose = String::new();

    // Setting line
    let setting_cap = match script.setting.as_str() {
        "night" => "Night.",
        "morning" => "Morning.",
        "evening" => "Evening.",
        "interior" => "Interior.",
        _ => "Day.",
    };
    prose.push_str(setting_cap);

    if !script.scenery.is_empty() && script.scenery != "none" {
        prose.push(' ');
        let mut sc = script.scenery.clone();
        if let Some(first) = sc.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        prose.push_str(&format!("{sc}."));
    }
    prose.push('\n');

    // Beats
    for beat in &script.beats {
        let actor = &stage_name(&beat.actor);
        let beat = &DirectorBeat {
            target: beat.target.as_deref().map(|t| {
                // A target that is one of the cast goes by its stage name
                // too; anything else ("the well") stays as written.
                if script.cast.iter().any(|c| c.eq_ignore_ascii_case(t))
                    || script.beats.iter().any(|b| b.actor.eq_ignore_ascii_case(t))
                {
                    stage_name(t)
                } else {
                    t.to_string()
                }
            }),
            ..beat.clone()
        };
        let sentence = match beat.verb {
            DirectorVerb::EnterLeft => format!("{actor} walks in from the left."),
            DirectorVerb::EnterRight => format!("{actor} walks in from the right."),
            DirectorVerb::WalkTo => {
                if let Some(ref target) = beat.target {
                    format!("{actor} walks to {target}.")
                } else {
                    format!("{actor} walks.")
                }
            }
            DirectorVerb::WalkAcross => format!("{actor} walks across."),
            DirectorVerb::WalkAway => format!("{actor} walks off."),
            DirectorVerb::RunTo => {
                if let Some(ref target) = beat.target {
                    format!("{actor} runs to {target}.")
                } else {
                    format!("{actor} runs.")
                }
            }
            DirectorVerb::RunAway => format!("{actor} runs off."),
            DirectorVerb::Talk => {
                if let Some(ref target) = beat.target {
                    format!("{actor} talks to {target}.")
                } else {
                    format!("{actor} talks.")
                }
            }
            DirectorVerb::Listen => format!("{actor} listens."),
            DirectorVerb::Idle => format!("{actor} waits."),
            DirectorVerb::Sit => format!("{actor} sits."),
            DirectorVerb::Stand => format!("{actor} stands up."),
            DirectorVerb::Point => format!("{actor} points."),
            DirectorVerb::Wave => format!("{actor} waves."),
            DirectorVerb::Reach => format!("{actor} reaches."),
            DirectorVerb::React => format!("{actor} reacts."),
            DirectorVerb::Turn => format!("{actor} turns."),
            DirectorVerb::LookAt => {
                if let Some(ref target) = beat.target {
                    format!("{actor} looks at {target}.")
                } else {
                    format!("{actor} looks.")
                }
            }
        };
        prose.push_str(&sentence);
        prose.push('\n');
    }

    prose
}

/// Executes a DirectorScript against a scene, building set, actors, performances, and camera tracking.
pub fn execute_director_script(
    scene: &mut Scene,
    script: &DirectorScript,
) -> Result<DirectedScene, DirectError> {
    let story_prose = script_to_story_text(script);
    let directed = direct::direct(scene, &story_prose)?;

    // Frame the shot automatically with camera cuts and pans
    direct::frame_the_shot(scene, &directed);

    // `beat.gaze` (a normalized eye-offset) is not applied here: `direct`
    // has no gaze/eye-target control to drive, whether or not the beat also
    // named a `target`, so pretending to apply it would be dead code. A
    // `look_at` beat's `target`, when present, still reaches `direct` as
    // "{actor} looks at {target}." through `script_to_story_text`, but the
    // grammar reads that only as an idling beat and does not extract a gaze
    // direction from it either. Gaze control is not wired up yet.

    Ok(directed)
}

/// One line of spoken dialogue, timed against the frames `direct` scheduled
/// for that actor's talk beat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialogueCue {
    pub speaker: String,
    pub text: String,
    pub start_frame: u32,
    pub end_frame: u32,
}

/// Pairs each `Talk` beat that carries dialogue with the frame range `direct`
/// already scheduled for that actor's talk event, so the words can be fed to
/// the same caption/lip-sync join the subtitle importer uses — without
/// `direct` itself ever having to know what was said.
///
/// Matching is positional, per actor: a beat's dialogue does not appear on
/// [`PlannedBeat`], so the Nth `Talk` beat with dialogue for an actor in
/// `script.beats` is paired with the Nth [`Action::Talk`] beat `direct`
/// planned for that same actor in `directed.beats`, both read in order.
/// `fps` is accepted for parity with the rest of this module's frame/second
/// conversions but is unused here: `direct` already schedules in frames.
/// A beat whose actor or Talk event cannot be matched (an unrecognised name,
/// or a sentence `direct` could not read) is silently skipped, exactly as
/// `direct` itself silently skips what it cannot read. Pure: reads `script`
/// and `directed`, changes nothing.
pub fn dialogue_cues(
    script: &DirectorScript,
    directed: &DirectedScene,
    _fps: f64,
) -> Vec<DialogueCue> {
    use std::collections::{HashMap, VecDeque};

    // Each actor's planned Talk beats, in the order `direct` scheduled them.
    let mut talk_frames: HashMap<usize, VecDeque<std::ops::Range<u32>>> = HashMap::new();
    for beat in &directed.beats {
        if beat.action == Action::Talk {
            talk_frames
                .entry(beat.actor)
                .or_default()
                .push_back(beat.frames.clone());
        }
    }

    let mut cues = Vec::new();
    for beat in &script.beats {
        if beat.verb != DirectorVerb::Talk {
            continue;
        }
        let Some(text) = beat.dialogue.as_ref() else {
            continue;
        };
        let key = stage_name(&beat.actor);
        let Some(actor_idx) = directed
            .names
            .iter()
            .position(|n| n == &beat.actor || n.eq_ignore_ascii_case(&key))
        else {
            continue;
        };
        let Some(range) = talk_frames.get_mut(&actor_idx).and_then(VecDeque::pop_front) else {
            continue;
        };
        cues.push(DialogueCue {
            speaker: beat.actor.clone(),
            text: text.clone(),
            start_frame: range.start,
            end_frame: range.end,
        });
    }
    // One caption at a time: `direct` may start a reply while the other
    // actor's talk is still winding down (16 frames here), and a caption
    // layer holds one line per keyframe — so a line ends where the next
    // begins.
    cues.sort_by_key(|c| c.start_frame);
    for i in 1..cues.len() {
        let next = cues[i].start_frame;
        let prev = &mut cues[i - 1];
        if prev.end_frame > next {
            prev.end_frame = next.max(prev.start_frame + 1);
        }
    }
    cues
}

#[cfg(test)]
mod multi_word_actor_tests {
    use super::*;

    /// F7 run (Claude API server, 2026-09-26): the model cast "Old Man" and
    /// gave him the first line, but only the Sadhu's line became a cue —
    /// `direct` reads a two-word name as something else. Both lines must be
    /// cued, each under the name the script used.
    #[test]
    fn a_two_word_actor_keeps_his_line() {
        let beat = |actor: &str, verb, dialogue: Option<&str>| DirectorBeat {
            actor: actor.into(),
            verb,
            target: None,
            dialogue: dialogue.map(Into::into),
            gaze: None,
            seconds: None,
        };
        for old in ["Old Man", "old man"] {
            let script = DirectorScript {
                setting: "morning".into(),
                scenery: "village".into(),
                cast: vec!["Sadhu".into(), old.into()],
                beats: vec![
                    beat("Sadhu", DirectorVerb::EnterLeft, None),
                    beat(old, DirectorVerb::Turn, None),
                    beat(old, DirectorVerb::Talk, Some("Namaste, Baba.")),
                    beat("Sadhu", DirectorVerb::Listen, None),
                    beat("Sadhu", DirectorVerb::Talk, Some("Only if it rains.")),
                    beat("Sadhu", DirectorVerb::WalkAway, None),
                ],
            };
            let mut scene = buzz_scene::Scene::default();
            let directed = execute_director_script(&mut scene, &script).expect("directs");
            let cues = dialogue_cues(&script, &directed, 24.0);
            assert_eq!(cues.len(), 2, "{old}: names {:?}, cues {cues:?}", directed.names);
            assert_eq!(cues[0].speaker, old);
            assert_eq!(cues[1].speaker, "Sadhu");
            assert!(cues[0].end_frame <= cues[1].start_frame, "{cues:?}");
        }
    }
}

#[cfg(test)]
mod dialogue_fallback_tests {
    use super::*;

    fn script(beats: Vec<DirectorBeat>) -> DirectorScript {
        DirectorScript {
            setting: "day".into(),
            scenery: "village".into(),
            cast: vec!["Ramu".into(), "Thakur".into()],
            beats,
        }
    }

    fn beat(actor: &str, verb: DirectorVerb, dialogue: Option<&str>) -> DirectorBeat {
        DirectorBeat {
            actor: actor.into(),
            verb,
            target: None,
            dialogue: dialogue.map(Into::into),
            gaze: None,
            seconds: None,
        }
    }

    const BRIEF: &str = "Ramu enters from the left and says, \"The rain is coming.\" \
        Thakur turns to him. \u{201C}Then bring the cows in,\u{201D} replies Thakur.";

    #[test]
    fn quoted_lines_find_the_speaker_before_or_after_the_quote() {
        let cast = vec!["Ramu".to_string(), "Thakur".to_string()];
        let lines = quoted_lines(BRIEF, &cast);
        assert_eq!(
            lines,
            vec![
                (Some("Ramu".into()), "The rain is coming.".into()),
                (Some("Thakur".into()), "Then bring the cows in,".into()),
            ]
        );
    }

    #[test]
    fn single_quoted_speech_counts_but_apostrophes_do_not() {
        let cast = vec!["Sadhu".to_string(), "old man".to_string()];
        let brief = "The Sadhu's bowl is empty. The old man turns and talks to him: 'Namaste, Baba.' The Sadhu listens.";
        assert_eq!(
            quoted_lines(brief, &cast),
            vec![(Some("old man".into()), "Namaste, Baba.".into())]
        );
    }

    #[test]
    fn empty_dialogue_is_filled_from_the_brief_onto_talk_beats() {
        let mut s = script(vec![
            beat("Ramu", DirectorVerb::EnterLeft, None),
            beat("Ramu", DirectorVerb::Talk, None),
            beat("Thakur", DirectorVerb::Turn, None),
        ]);
        assert_eq!(fill_dialogue_from_brief(&mut s, BRIEF), 2);
        assert_eq!(s.beats[1].dialogue.as_deref(), Some("The rain is coming."));
        // Thakur had no talk beat: one is added for his line.
        let last = s.beats.last().unwrap();
        assert_eq!((last.actor.as_str(), last.verb), ("Thakur", DirectorVerb::Talk));
        assert_eq!(last.dialogue.as_deref(), Some("Then bring the cows in,"));
    }

    #[test]
    fn dialogue_the_model_already_copied_is_not_duplicated() {
        let mut s = script(vec![
            beat("Ramu", DirectorVerb::Talk, Some("The rain is coming")),
            beat("Thakur", DirectorVerb::Talk, Some("Then bring the cows in")),
        ]);
        assert_eq!(fill_dialogue_from_brief(&mut s, BRIEF), 0);
        assert_eq!(s.beats.len(), 2);
    }
}

#[cfg(test)]
mod dialogue_cue_tests {
    use super::*;
    use buzz_act::direct::PlannedBeat;

    /// Two actors, alternating lines: cues come back in order, each one's
    /// frame range matching the planned Talk beat it was paired with, and
    /// nothing overlaps within an actor's own lines.
    #[test]
    fn alternating_lines_pair_in_order_with_non_overlapping_frames() {
        let script = DirectorScript {
            setting: "day".into(),
            scenery: "none".into(),
            cast: vec!["Ana".into(), "Ben".into()],
            beats: vec![
                DirectorBeat {
                    actor: "Ana".into(),
                    verb: DirectorVerb::Talk,
                    target: Some("Ben".into()),
                    dialogue: Some("Hello, Ben.".into()),
                    gaze: None,
                    seconds: None,
                },
                DirectorBeat {
                    actor: "Ben".into(),
                    verb: DirectorVerb::Talk,
                    target: Some("Ana".into()),
                    dialogue: Some("Hello, Ana.".into()),
                    gaze: None,
                    seconds: None,
                },
                DirectorBeat {
                    actor: "Ana".into(),
                    verb: DirectorVerb::Talk,
                    target: None,
                    dialogue: Some("Good to see you.".into()),
                    gaze: None,
                    seconds: None,
                },
            ],
        };

        let directed = DirectedScene {
            names: vec!["Ana".into(), "Ben".into()],
            beats: vec![
                PlannedBeat {
                    actor: 0,
                    action: Action::Talk,
                    frames: 0..24,
                    travel: 0.0,
                },
                PlannedBeat {
                    actor: 1,
                    action: Action::Talk,
                    frames: 24..48,
                    travel: 0.0,
                },
                PlannedBeat {
                    actor: 0,
                    action: Action::Talk,
                    frames: 48..72,
                    travel: 0.0,
                },
            ],
            ..Default::default()
        };

        let cues = dialogue_cues(&script, &directed, 24.0);
        assert_eq!(cues.len(), 3, "every dialogue beat should produce a cue: {cues:?}");

        assert_eq!(cues[0].speaker, "Ana");
        assert_eq!(cues[0].text, "Hello, Ben.");
        assert_eq!((cues[0].start_frame, cues[0].end_frame), (0, 24));

        assert_eq!(cues[1].speaker, "Ben");
        assert_eq!(cues[1].text, "Hello, Ana.");
        assert_eq!((cues[1].start_frame, cues[1].end_frame), (24, 48));

        assert_eq!(cues[2].speaker, "Ana");
        assert_eq!(cues[2].text, "Good to see you.");
        assert_eq!((cues[2].start_frame, cues[2].end_frame), (48, 72));

        // In order, and non-overlapping inside each actor's own lines.
        let ana_frames: Vec<_> = cues.iter().filter(|c| c.speaker == "Ana").collect();
        assert!(ana_frames[0].end_frame <= ana_frames[1].start_frame);
    }
}
