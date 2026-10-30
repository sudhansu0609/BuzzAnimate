//! **Script breakdown** (Producer P1): the questions a casting file answers by
//! hand, answered from the script.
//!
//! A writer's script says who speaks and what happens; the episode builder
//! (`buzz_app::episode`) also wants to know who stands where, which place each
//! scene is in (so one background can serve every scene set there), what the
//! light is, which props dress the set and where the camera cuts. Today a
//! person writes that into `casting.json`. This reads the
//! [`Screenplay`] and writes it instead.
//!
//! # Two passes, and a floor under both
//!
//! * **Pass 1**, once, over the whole script: the characters — names as the
//!   script writes them, aliases, a one-sentence look for a character-sheet
//!   prompt, voice-only or not, how big the part is.
//! * **Pass 2**, four scenes at a time with their text verbatim: per scene, the
//!   place, the setting, a background prompt, who is there and where, props,
//!   lighting and shots. Four keeps a batch well inside a local model's context
//!   and still lets it see that scene 5 is the tea stall of scene 4; the keys
//!   already given are passed on so a place keeps its key across batches.
//!
//! The producer must never stop on a bad answer, so [`breakdown`] never fails:
//! [`breakdown_offline`] is worked out first from the script alone (slugline
//! rules like the episode builder's own, speakers and the names in action
//! lines), and whatever the model gets wrong — an error, a missing scene, a
//! word outside the allowed ones — falls back to it, per batch or per field,
//! with a note through `progress`. Facts the parser already knows (the
//! slugline, who speaks on screen) are never overruled by the model.
//!
//! [`casting_value`] turns a breakdown into the casting file's JSON.

use std::collections::BTreeMap;

use buzz_act::screenplay::{Item, Pause, Place, Screenplay, ScriptScene, Slugline, Voice};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::llm::Llm;

/// What the script asks of the production, scene by scene.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Breakdown {
    pub title: String,
    /// The hints' `world`, kept with the breakdown so [`casting_value`] can end
    /// each background prompt with it.
    pub world: String,
    pub characters: Vec<CharacterBrief>,
    pub scenes: Vec<SceneBrief>,
    /// The hints' style text, ends every location look.
    pub style: String,
    /// AN4: character key (upper-case) -> the pose names its folder has pictures for.
    pub poses_available: BTreeMap<String, Vec<String>>,
    /// AN9: location key -> its look and seed family.
    pub looks: BTreeMap<String, LocationLook>,
}

impl Breakdown {
    /// The parts a breakdown takes from the script and hints, not from analysis.
    fn from_hints(play: &Screenplay, hints: &BreakdownHints) -> Self {
        Breakdown {
            title: play.title.clone(),
            world: hints.world.clone(),
            style: hints.style.clone(),
            poses_available: hints
                .known_poses
                .iter()
                .map(|(k, v)| (norm(k), v.iter().map(|p| p.to_lowercase().replace(' ', "_")).collect()))
                .collect(),
            ..Default::default()
        }
    }
}

/// One character of the script.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CharacterBrief {
    /// As the script writes the cue: `GUDDU`, `SHARMA JI`.
    pub name: String,
    /// Other ways the action lines name them.
    pub aliases: Vec<String>,
    /// One sentence: age, build, clothes, mood — for a character-sheet prompt.
    pub description: String,
    /// Heard, never seen (the narrator).
    pub voice_only: bool,
    /// 1 lead, 2 supporting, 3 extra.
    pub importance: u8,
}

/// How a scene is lit, in the words `buzz_light` is driven by (P5).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Lighting {
    /// `sun`, `lamp`, `moon`, `window` or `none`.
    pub key: String,
    /// `left`, `right` or `front`.
    pub from: String,
    pub warm: bool,
}

/// A camera cut.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ShotBrief {
    /// Index among the scene's dialogue lines (every cue, voice-over too).
    pub at_line: usize,
    /// `wide`, `medium` or `close`.
    pub kind: String,
    /// The character the shot is on; empty for a wide.
    pub on: String,
    /// AN9: the angle, one of [`CAMERAS`]; `kind` is what the episode builder
    /// frames (`low_angle`, `reverse`... are `medium` there).
    pub camera: String,
    /// AN9: the plate prompt of this angle: the location's look + the camera's
    /// framing words + nobody in frame. What the backdrop step draws from.
    pub background_prompt: String,
    /// AN9: shared by every shot of the location.
    pub seed_family: u64,
}

/// AN9: one location's look, shared by all its shots.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocationLook {
    /// Location description, time, weather, lighting, era/world and the series
    /// style text; no people, no camera words.
    pub look: String,
    pub seed_family: u64,
    /// The `setting` the look was written for.
    pub setting: String,
}

/// AN4: a `[Name: pose, position]` tag of the script.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StageTag {
    pub name: String,
    pub pose: String,
    /// `left`, `centre`, `right`, `door`, `30%`; empty when not given.
    pub position: String,
}

/// One scene of the script, broken down.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SceneBrief {
    /// The writer's scene number.
    pub number: u32,
    /// The scene's slugline as written, or the one it carries on from.
    pub slugline: String,
    pub interior: bool,
    /// `daylight`, `sunset`, `night`, `interior` or `storm`.
    pub setting: String,
    /// snake_case; the same place in different scenes gets the same key, so one
    /// background serves them.
    pub location_key: String,
    /// 2–4 sentences of what the camera sees: no people, time of day, mood.
    /// The drawing style (comic, painterly) is the producer's to add.
    pub background_prompt: String,
    /// On stage, in order of first appearance.
    pub present: Vec<String>,
    /// Named in the scene but not there.
    pub absent: Vec<String>,
    /// Feet across the stage, 0.15..0.85.
    pub place: BTreeMap<String, f64>,
    /// `left` or `right`, for those who walk in during the scene.
    pub enter: BTreeMap<String, String>,
    /// Words for the asset index: "tea stall", "charpoy".
    pub props: Vec<String>,
    pub lighting: Lighting,
    pub shots: Vec<ShotBrief>,
    pub mood: String,
    /// AN4: the pose picture each present character plays (a pose name from the
    /// character's folder); a script tag wins over the choice made from the action.
    pub poses: BTreeMap<String, String>,
    /// AN13: the concrete set nouns of the slugline and the first action line
    /// ("an old peepal tree", "a broken lamp post"); they lead the background prompt.
    pub set_nouns: Vec<String>,
}

/// What the producer knows besides the script.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BreakdownHints {
    /// "comic, light humour", "painterly".
    pub style: String,
    /// The Cast folder's names, so the model can see who has drawings.
    pub known_characters: Vec<String>,
    pub series: String,
    /// Where and when the story lives: "rural Bihar, India, late 1990s". Ends
    /// every background prompt, so a generator does not draw a generic street.
    pub world: String,
    /// AN4: character name -> the pose names its folder has pictures for.
    pub known_poses: BTreeMap<String, Vec<String>>,
}

/// AN9: the camera angles.
pub const CAMERAS: &[&str] = &["wide", "medium", "close", "low_angle", "high_angle", "over_shoulder", "reverse"];

const SETTINGS: &[&str] = &["daylight", "sunset", "night", "interior", "storm"];
const LIGHT_KEYS: &[&str] = &["sun", "lamp", "moon", "window", "none"];
const SIDES: &[&str] = &["left", "right", "front"];
const SHOT_KINDS: &[&str] = &["wide", "medium", "close"];
const BATCH: usize = 4;
/// Pass 1's script part size.
const PART_BYTES: usize = 12 * 1024;

// -- names --------------------------------------------------------------------------

/// Names are matched upper-case, as the casting file does.
fn norm(name: &str) -> String {
    name.trim().to_uppercase()
}

/// Whether `name` occurs in `text` as whole words (both already lower-case):
/// "raju" in "Raju's bike" but not in "rajubhai".
fn mentions(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(p) = text[from..].find(name) {
        let start = from + p;
        let end = start + name.len();
        let before = text[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = text[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        from = start + name.len().max(1);
        while !text.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

/// `CHHOTU'S TEA STALL` → `chhotus_tea_stall`.
fn snake(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if c == '\'' || c == '\u{2019}' {
            // An apostrophe joins: "chhotus", not "chhotu_s".
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// `CHHOTU'S TEA STALL` → `Chhotu's tea stall`.
fn sentence_case(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Every name a character goes by, lower-case, longest first so `sharma ji`
/// wins over `sharma`. Voice-only characters are never on stage, so are left
/// out.
fn stage_names(characters: &[CharacterBrief]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for c in characters.iter().filter(|c| !c.voice_only) {
        let key = norm(&c.name);
        for n in std::iter::once(&c.name).chain(&c.aliases) {
            let n = n.trim().to_lowercase();
            if !n.is_empty() {
                out.push((n, key.clone()));
            }
        }
    }
    out.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    out
}

/// The character a name the model wrote means: by name or alias, any case.
/// A name that matches nobody is kept, upper-case: the model may have seen
/// someone the cues never name.
fn resolve(name: &str, characters: &[CharacterBrief]) -> String {
    let key = norm(name);
    characters
        .iter()
        .find(|c| norm(&c.name) == key || c.aliases.iter().any(|a| norm(a) == key))
        .map(|c| norm(&c.name))
        .unwrap_or(key)
}

/// Never on stage as one figure: a narrator, or a cue several people say at
/// once (`ALL`, `ALL THREE`, `BOTH`), which is lines, not a character.
fn is_voice_only(key: &str, characters: &[CharacterBrief]) -> bool {
    is_group(key) || characters.iter().any(|c| norm(&c.name) == key && c.voice_only)
}

fn is_group(key: &str) -> bool {
    key == "ALL" || key.starts_with("ALL ") || matches!(key, "BOTH" | "EVERYONE" | "TOGETHER" | "CROWD")
}

// -- the scene's own facts ----------------------------------------------------------------

/// Each scene's slugline: its own, or the one the scene before it ended in
/// (scene 5 carries on at scene 4's tea stall, as the episode builder reads it).
fn carried_sluglines(play: &Screenplay) -> Vec<Option<Slugline>> {
    let mut last: Option<Slugline> = None;
    play.scenes
        .iter()
        .map(|s| {
            let own = s.slugline.clone().or_else(|| last.clone());
            last = s
                .items
                .iter()
                .rev()
                .find_map(|i| match i {
                    Item::Slugline(sl) => Some(sl.clone()),
                    _ => None,
                })
                .or_else(|| own.clone());
            own
        })
        .collect()
}

/// The setting a slugline implies: the episode builder's `setting_for` rules.
fn setting_for(slug: Option<&Slugline>) -> &'static str {
    let Some(slug) = slug else {
        return "daylight";
    };
    if slug.place == Place::Interior {
        return "interior";
    }
    let time = slug.time.to_uppercase();
    if time.contains("NIGHT") {
        "night"
    } else if ["EVENING", "DUSK", "SUNSET", "DAWN"].iter().any(|t| time.contains(t)) {
        "sunset"
    } else {
        "daylight"
    }
}

/// The light a setting suggests when nobody says otherwise: the sun high on the
/// right by day, low on the left at sunset, a cool moon at night, a window
/// indoors.
fn lighting_for(setting: &str) -> Lighting {
    let (key, from, warm) = match setting {
        "daylight" => ("sun", "right", true),
        "sunset" => ("sun", "left", true),
        "night" => ("moon", "left", false),
        "interior" => ("window", "left", false),
        _ => ("none", "front", false),
    };
    Lighting { key: key.into(), from: from.into(), warm }
}

fn dialogue_count(scene: &ScriptScene) -> usize {
    scene.items.iter().filter(|i| matches!(i, Item::Dialogue(_))).count()
}

/// Feet across the stage for the `i`-th of `n`, spread evenly 0.25..0.75.
fn spread(i: usize, n: usize) -> f64 {
    if n <= 1 {
        0.5
    } else {
        0.25 + 0.5 * i as f64 / (n - 1) as f64
    }
}

/// Who speaks on screen, in order: the parser knows this for certain.
fn on_screen_speakers(scene: &ScriptScene, characters: &[CharacterBrief]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in &scene.items {
        if let Item::Dialogue(d) = item
            && d.voice == Voice::OnScreen
        {
            let key = resolve(&d.speaker, characters);
            if !is_voice_only(&key, characters) && !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out
}

// -- offline ------------------------------------------------------------------------------

/// **The breakdown from the script alone**, with no model: what the producer
/// falls back to, and the floor every model answer is checked against.
pub fn breakdown_offline(play: &Screenplay, hints: &BreakdownHints) -> Breakdown {
    let characters = offline_characters(play, hints);
    let slugs = carried_sluglines(play);
    let scenes = play
        .scenes
        .iter()
        .zip(&slugs)
        .map(|(s, slug)| offline_scene(s, slug.as_ref(), &characters))
        .collect();
    let mut b = Breakdown { characters, scenes, ..Breakdown::from_hints(play, hints) };
    tidy(&mut b, play);
    b
}

// -- the last pass ---------------------------------------------------------------------------

/// **What every breakdown goes through last**, from the model or not: aliases
/// that are names, a character for everyone on stage, marks that do not
/// overlap, a key light with a side. Public so a breakdown read back from a
/// file (the producer's resume) gets the same treatment.
pub fn tidy(b: &mut Breakdown, play: &Screenplay) {
    tidy_aliases(&mut b.characters);
    add_unlisted(b, play);
    for s in &mut b.scenes {
        tidy_marks(&mut s.place);
        // A front key light has no direction to throw shadows by.
        if (s.lighting.key == "sun" || s.lighting.key == "moon") && s.lighting.from == "front" {
            s.lighting.from = "right".into();
        }
        s.background_prompt = without_people(&s.background_prompt, &s.slugline);
    }
    // AN4 / AN9: tags and poses, a camera per shot, one look per location.
    apply_stage(b, play);
    fill_cameras(b);
    apply_looks(b);
}

/// Words that put a person (or a group) into a picture. A background generator
/// draws whoever the prompt names, whatever the exclusions say after (P6
/// review: "three young men are seated together" drew four men on the bench).
const PEOPLE_WORDS: &[&str] = &[
    "men", "man", "woman", "women", "people", "person", "persons", "boy", "girl", "child", "children", "kid",
    "customer", "customers", "villager", "villagers", "passenger", "passengers", "crowd", "friends", "figure",
    "figures", "he", "she", "they", "his", "her", "their", "seated", "sitting", "standing", "chatting", "talking",
    "waiting",
];

/// `prompt` and "Details: a kettle, glasses, a wooden bench." from the scene's
/// prop words (any naming a person left out): the people filter drops whole
/// sentences, and the objects in them with the people (P6 review: a bare set).
fn with_details(prompt: String, props: &[String]) -> String {
    let items: Vec<String> = props
        .iter()
        .map(|p| p.trim().trim_end_matches('.').to_lowercase())
        .filter(|p| !p.is_empty())
        .filter(|p| !p.split(|c: char| !c.is_alphabetic()).any(|w| !w.is_empty() && PEOPLE_WORDS.contains(&w)))
        .map(|p| {
            let plural = p.ends_with('s') && !p.ends_with("ss");
            if plural { p } else { format!("a {p}") }
        })
        .collect();
    if items.is_empty() {
        return prompt;
    }
    format!("{} Details: {}.", prompt.trim_end(), items.join(", "))
}

/// `prompt` without every sentence that names a person or a group; when
/// nothing is left, the slugline's place words and "empty".
fn without_people(prompt: &str, slugline: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut sentence = String::new();
    let mut flush = |sentence: &mut String, kept: &mut Vec<String>| {
        let s = sentence.trim().to_string();
        sentence.clear();
        if s.is_empty() {
            return;
        }
        let lower = s.to_lowercase();
        let names_someone =
            lower.split(|c: char| !c.is_alphabetic()).any(|w| !w.is_empty() && PEOPLE_WORDS.contains(&w));
        if !names_someone {
            kept.push(s);
        }
    };
    for c in prompt.chars() {
        sentence.push(c);
        if matches!(c, '.' | '!' | '?') {
            flush(&mut sentence, &mut kept);
        }
    }
    flush(&mut sentence, &mut kept);
    if !kept.is_empty() {
        return kept.join(" ");
    }
    let raw = slugline.trim();
    let rest = ["INT./EXT.", "EXT./INT.", "INT/EXT.", "I/E.", "INT.", "EXT."]
        .iter()
        .find_map(|p| raw.get(..p.len()).filter(|h| h.eq_ignore_ascii_case(p)).map(|_| &raw[p.len()..]))
        .unwrap_or(raw);
    let place = rest.split(['\u{2014}', '\u{2013}']).next().unwrap_or(rest).split(" - ").next().unwrap_or(rest).trim();
    let place = place.trim_matches(|c: char| c == '*' || c.is_whitespace());
    if place.is_empty() { "Empty.".to_string() } else { format!("{}, empty.", sentence_case(place)) }
}

/// An alias is a name: at most two words, each capitalised, no apostrophe or
/// digit. The episode builder finds people in action lines by alias, so a
/// phrase like "Tea stall owner" or "Raju's mother" would cast the wrong
/// person; such a phrase is kept as the description when there is none.
fn tidy_aliases(characters: &mut [CharacterBrief]) {
    fn name_like(a: &str) -> bool {
        let words: Vec<&str> = a.split_whitespace().collect();
        !words.is_empty()
            && words.len() <= 2
            && !a.chars().any(|c| c == '\'' || c == '\u{2019}' || c.is_ascii_digit())
            && words.iter().all(|w| w.chars().next().is_some_and(char::is_uppercase))
    }
    for c in characters {
        let (keep, drop): (Vec<String>, Vec<String>) =
            std::mem::take(&mut c.aliases).into_iter().partition(|a| name_like(a));
        c.aliases = keep;
        if c.description.trim().is_empty() && !drop.is_empty() {
            c.description = format!("{}.", sentence_case(&drop.join(", ")).trim_end_matches('.'));
        }
    }
}

/// Everyone on stage gets a character, even with no cue (a family that moves
/// in silently): importance 3, described by the first action line of the
/// scene that names them.
fn add_unlisted(b: &mut Breakdown, play: &Screenplay) {
    for s in &b.scenes {
        for name in s.present.iter().chain(s.place.keys()) {
            let key = norm(name);
            if key.is_empty()
                || is_group(&key)
                || b.characters
                    .iter()
                    .any(|c| norm(&c.name) == key || c.aliases.iter().any(|a| norm(a) == key))
            {
                continue;
            }
            let lower = key.to_lowercase();
            let description = play
                .scene(s.number)
                .and_then(|src| {
                    src.items.iter().find_map(|i| match i {
                        Item::Action(a) if mentions(&a.to_lowercase(), &lower) => Some(a.trim().to_string()),
                        _ => None,
                    })
                })
                .unwrap_or_default();
            b.characters.push(CharacterBrief {
                name: key,
                aliases: Vec::new(),
                description,
                voice_only: false,
                importance: 3,
            });
        }
    }
}

/// Marks inside 0.15..0.85; when two stand within 0.08 of each other, the
/// whole scene is spread evenly over that range, left to right as they were.
fn tidy_marks(place: &mut BTreeMap<String, f64>) {
    for x in place.values_mut() {
        *x = if x.is_finite() { x.clamp(0.15, 0.85) } else { 0.5 };
    }
    let mut order: Vec<(f64, String)> = place.iter().map(|(k, x)| (*x, k.clone())).collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    if order.windows(2).any(|w| w[1].0 - w[0].0 < 0.08) {
        let n = order.len();
        for (i, (_, k)) in order.iter().enumerate() {
            let x = if n <= 1 { 0.5 } else { 0.15 + 0.7 * i as f64 / (n - 1) as f64 };
            place.insert(k.clone(), x);
        }
    }
}

/// Every cue's name in order of first line, plus any known Cast name an action
/// line mentions. Voice-only when every line is a voice-over; a description
/// from the script's introduction line (`GUDDU, 28. Hair carefully combed.`)
/// when it has one; importance by how many lines they have.
fn offline_characters(play: &Screenplay, hints: &BreakdownHints) -> Vec<CharacterBrief> {
    // (key, lines, any line on screen or off screen)
    let mut seen: Vec<(String, usize, bool)> = Vec::new();
    for scene in &play.scenes {
        for item in &scene.items {
            if let Item::Dialogue(d) = item {
                let key = norm(&d.speaker);
                if is_group(&key) {
                    continue;
                }
                let shown = d.voice != Voice::VoiceOver;
                match seen.iter_mut().find(|(k, _, _)| *k == key) {
                    Some(e) => {
                        e.1 += 1;
                        e.2 |= shown;
                    }
                    None => seen.push((key, 1, shown)),
                }
            }
        }
    }
    let actions: Vec<&str> = play
        .scenes
        .iter()
        .flat_map(|s| &s.items)
        .filter_map(|i| match i {
            Item::Action(a) => Some(a.as_str()),
            _ => None,
        })
        .collect();
    let all_action = actions.join("\n").to_lowercase();
    for known in &hints.known_characters {
        let key = norm(known);
        if !key.is_empty() && !seen.iter().any(|(k, _, _)| *k == key) && mentions(&all_action, &key.to_lowercase()) {
            seen.push((key, 0, true));
        }
    }
    let most = seen.iter().filter(|(_, _, shown)| *shown).map(|(_, n, _)| *n).max().unwrap_or(0);
    seen.into_iter()
        .map(|(key, lines, shown)| {
            let description = actions
                .iter()
                .find(|a| a.starts_with(&format!("{key},")))
                .map(|a| a.trim().to_string())
                .unwrap_or_default();
            let aliases = key
                .strip_suffix(" JI")
                .map(|stem| vec![stem.trim().to_string()])
                .unwrap_or_default();
            let importance = if shown && lines * 3 >= most && most > 0 {
                1
            } else if lines >= 3 {
                2
            } else {
                3
            };
            CharacterBrief { name: key, aliases, description, voice_only: lines > 0 && !shown, importance }
        })
        .collect()
}

/// One scene from its slugline and its lines: present = on-screen speakers and
/// names in action lines, spread evenly in order of appearance; one wide shot.
fn offline_scene(scene: &ScriptScene, slug: Option<&Slugline>, characters: &[CharacterBrief]) -> SceneBrief {
    let names = stage_names(characters);
    let mut present: Vec<String> = Vec::new();
    for item in &scene.items {
        match item {
            Item::Dialogue(d) if d.voice == Voice::OnScreen => {
                let key = resolve(&d.speaker, characters);
                if !is_voice_only(&key, characters) && !present.contains(&key) {
                    present.push(key);
                }
            }
            Item::Action(a) => {
                let text = a.to_lowercase();
                // In order of where each name first falls in the line.
                let mut found: Vec<(usize, String)> = names
                    .iter()
                    .filter(|(n, _)| mentions(&text, n))
                    .filter_map(|(n, key)| text.find(n.as_str()).map(|p| (p, key.clone())))
                    .collect();
                found.sort();
                for (_, key) in found {
                    if !present.contains(&key) {
                        present.push(key);
                    }
                }
            }
            _ => {}
        }
    }
    let place = present
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), spread(i, present.len())))
        .collect();

    let setting = setting_for(slug);
    let location = slug.map(|s| s.location.clone()).unwrap_or_else(|| scene.title.clone());
    let first_action = scene.items.iter().find_map(|i| match i {
        Item::Action(a) => Some(a.trim().to_string()),
        _ => None,
    });
    let nouns = set_nouns(slug.map(|s| s.raw.as_str()).unwrap_or(""), first_action.as_deref());
    let mut background_prompt = sentence_case(&location);
    if let Some(s) = slug
        && !s.time.trim().is_empty()
    {
        background_prompt.push_str(", ");
        background_prompt.push_str(&s.time.trim().to_lowercase());
    }
    background_prompt.push('.');
    if let Some(a) = first_action {
        background_prompt.push(' ');
        background_prompt.push_str(&a);
    }
    background_prompt.push_str(" No people.");

    SceneBrief {
        number: scene.number,
        slugline: slug.map(|s| s.raw.clone()).unwrap_or_default(),
        interior: slug.is_some_and(|s| s.place == Place::Interior),
        setting: setting.into(),
        location_key: snake(&location),
        background_prompt,
        present,
        absent: Vec::new(),
        place,
        enter: BTreeMap::new(),
        props: Vec::new(),
        lighting: lighting_for(setting),
        shots: offline_shots(scene, characters),
        mood: String::new(),
        set_nouns: nouns,
        ..Default::default()
    }
}

// -- the model --------------------------------------------------------------------------

/// A scene as the model reads it: the script's own words, each cue numbered so
/// a shot can name the line it cuts on.
fn scene_text(scene: &ScriptScene, carried: Option<&Slugline>) -> String {
    let mut out = format!("## SCENE {} — {}\n\n", scene.number, scene.title);
    match (&scene.slugline, carried) {
        (Some(s), _) => out.push_str(&format!("**{}**\n\n", s.raw)),
        (None, Some(c)) => out.push_str(&format!("(no slugline: carries on at {})\n\n", c.raw)),
        (None, None) => {}
    }
    let mut line = 0usize;
    for item in &scene.items {
        match item {
            Item::Action(a) => out.push_str(&format!("{a}\n\n")),
            Item::Dialogue(d) => {
                let tag = match d.voice {
                    Voice::OnScreen => "",
                    Voice::VoiceOver => " (V.O.)",
                    Voice::OffScreen => " (O.S.)",
                };
                out.push_str(&format!("[line {line}] **{}{tag}**\n{}\n\n", d.speaker, d.text));
                line += 1;
            }
            Item::Pause(p) => out.push_str(match p {
                Pause::Beat => "Beat.\n\n",
                Pause::LongSilence => "Long silence.\n\n",
                Pause::Silence => "Silence.\n\n",
            }),
            Item::Break => out.push_str("---\n\n"),
            Item::Slugline(s) => out.push_str(&format!("**{}**\n\n", s.raw)),
        }
    }
    out
}

fn hints_text(hints: &BreakdownHints) -> String {
    let mut out = String::new();
    if !hints.series.trim().is_empty() {
        out.push_str(&format!("Series: {}\n", hints.series.trim()));
    }
    if !hints.style.trim().is_empty() {
        out.push_str(&format!("Style: {}\n", hints.style.trim()));
    }
    if !hints.known_characters.is_empty() {
        out.push_str(&format!(
            "Character drawings the user already has (Cast folder): {}\n",
            hints.known_characters.join(", ")
        ));
    }
    out
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LlmCharacters {
    characters: Vec<LlmCharacter>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LlmCharacter {
    name: String,
    aliases: Vec<String>,
    description: String,
    voice_only: bool,
    importance: i64,
}

fn characters_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "characters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "aliases": { "type": "array", "items": { "type": "string" } },
                        "description": { "type": "string" },
                        "voice_only": { "type": "boolean" },
                        "importance": { "type": "integer", "enum": [1, 2, 3] }
                    },
                    "required": ["name", "aliases", "description", "voice_only", "importance"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["characters"],
        "additionalProperties": false
    })
}

/// Pass 1: the characters. The model's descriptions, aliases and importance
/// win; the cue list and who speaks on screen are the parser's, so a speaker
/// the model forgot is kept and nobody with a line on screen becomes
/// voice-only. `None` when the answer is unusable.
fn llm_characters(
    llm: &impl Llm,
    model: &str,
    play: &Screenplay,
    hints: &BreakdownHints,
    offline: &[CharacterBrief],
    progress: &mut dyn FnMut(&str),
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<CharacterBrief>> {
    let system = "You are a script supervisor breaking down an animated web series script for production. \
List every character of the script: everyone who has a cue, and anyone else the action lines show on screen. \
`name` is the name exactly as the script writes the cue, in capitals (GUDDU, SHARMA JI); a character with no cue gets their name in capitals. \
`aliases` are the other ways the action lines call them (a first name, a relation like AMMA, a nickname); leave the name itself out. \
`description` is one sentence for a character-sheet drawing: age, build, face, clothes, attitude — from the script, and filled in plausibly for the setting when the script is silent. No names of real people. \
`voice_only` is true only for someone never seen (a narrator). \
`importance` is 1 for a lead, 2 for a supporting part, 3 for an extra. \
Output strictly valid JSON according to the schema.";
    let speakers: Vec<String> = offline
        .iter()
        .map(|c| format!("{}{}", c.name, if c.voice_only { " (only voice-over)" } else { "" }))
        .collect();
    // The script in parts of at most PART_BYTES, whole scenes each: the whole
    // Ep1 script in one prompt killed a 27B model in LM Studio (2026-10-14).
    let slugs = carried_sluglines(play);
    let mut parts: Vec<String> = Vec::new();
    for (s, sl) in play.scenes.iter().zip(&slugs) {
        let text = scene_text(s, sl.as_ref());
        match parts.last_mut() {
            Some(last) if last.len() + text.len() <= PART_BYTES => last.push_str(&text),
            _ => parts.push(text),
        }
    }
    // Merged by name across parts; the first description wins.
    let mut answer: Vec<LlmCharacter> = Vec::new();
    let count = parts.len();
    for (i, part) in parts.iter().enumerate() {
        let user = format!(
            "{}Title: {}\nCues found in the whole script: {}\n\nTHE SCRIPT, part {} of {count}\n\n{part}",
            hints_text(hints),
            play.title,
            speakers.join(", "),
            i + 1
        );
        progress(&format!("characters: asking the model (part {} of {count}, {} bytes)", i + 1, part.len()));
        let got: LlmCharacters = match llm.chat_json_cancellable(model, system, &user, &[], characters_schema(), cancel) {
            Ok(a) => a,
            Err(_) if cancel() => return None,
            Err(e) => {
                progress(&format!("characters part {}: model failed ({e})", i + 1));
                continue;
            }
        };
        for c in got.characters.into_iter().filter(|c| !c.name.trim().is_empty()) {
            match answer.iter_mut().find(|a| norm(&a.name) == norm(&c.name)) {
                Some(a) => {
                    if a.description.trim().is_empty() {
                        a.description = c.description;
                    }
                    for al in c.aliases {
                        if !a.aliases.iter().any(|x| norm(x) == norm(&al)) {
                            a.aliases.push(al);
                        }
                    }
                    a.voice_only &= c.voice_only;
                    a.importance = a.importance.min(c.importance);
                }
                None => answer.push(c),
            }
        }
    }
    if answer.is_empty() {
        progress("characters: no usable answer from any part; using the script's cues");
        return None;
    }

    let mut out: Vec<CharacterBrief> = Vec::new();
    for off in offline {
        let key = norm(&off.name);
        let found = answer.iter().position(|c| {
            norm(&c.name) == key || c.aliases.iter().any(|a| norm(a) == key) || off.aliases.iter().any(|a| norm(a) == norm(&c.name))
        });
        let Some(i) = found else {
            out.push(off.clone());
            continue;
        };
        let c = answer.remove(i);
        let mut aliases = off.aliases.clone();
        for a in c.aliases.iter().chain(std::iter::once(&c.name)) {
            let a = a.trim();
            if !a.is_empty() && norm(a) != key && !aliases.iter().any(|x| norm(x) == norm(a)) {
                aliases.push(a.to_string());
            }
        }
        let description = if c.description.trim().is_empty() { off.description.clone() } else { c.description.trim().to_string() };
        // A cue the parser saw on screen is never voice-only; a voice the
        // parser only heard stays voice-only unless the model sees them.
        let voice_only = off.voice_only && c.voice_only;
        out.push(CharacterBrief {
            name: key,
            aliases,
            description,
            voice_only,
            importance: c.importance.clamp(1, 3) as u8,
        });
    }
    for c in answer {
        let key = norm(&c.name);
        if is_group(&key) || out.iter().any(|o| norm(&o.name) == key || o.aliases.iter().any(|a| norm(a) == key)) {
            continue;
        }
        out.push(CharacterBrief {
            name: key.clone(),
            aliases: c
                .aliases
                .iter()
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty() && norm(a) != key)
                .collect(),
            description: c.description.trim().to_string(),
            voice_only: c.voice_only,
            importance: c.importance.clamp(1, 3) as u8,
        });
    }
    progress(&format!("characters: {} from the model", out.len()));
    Some(out)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LlmScenes {
    scenes: Vec<LlmScene>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LlmScene {
    number: i64,
    interior: bool,
    setting: String,
    location_key: String,
    background_prompt: String,
    present: Vec<String>,
    absent: Vec<String>,
    place: Vec<LlmPlace>,
    enter: Vec<LlmEnter>,
    props: Vec<String>,
    lighting: Lighting,
    shots: Vec<LlmShot>,
    mood: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LlmPlace {
    name: String,
    x: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LlmEnter {
    name: String,
    from: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LlmShot {
    at_line: i64,
    kind: String,
    on: String,
    camera: String,
}

/// Pass 2's schema. Maps are lists of `{name, …}` here: strict JSON schema
/// (LM Studio's grammar) cannot describe an object with free keys.
fn scenes_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scenes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "number": { "type": "integer" },
                        "interior": { "type": "boolean" },
                        "setting": { "type": "string", "enum": SETTINGS },
                        "location_key": { "type": "string" },
                        "background_prompt": { "type": "string", "description": "describe the empty location only; never mention people" },
                        "present": { "type": "array", "items": { "type": "string" } },
                        "absent": { "type": "array", "items": { "type": "string" } },
                        "place": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": { "name": { "type": "string" }, "x": { "type": "number" } },
                                "required": ["name", "x"],
                                "additionalProperties": false
                            }
                        },
                        "enter": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "from": { "type": "string", "enum": ["left", "right"] }
                                },
                                "required": ["name", "from"],
                                "additionalProperties": false
                            }
                        },
                        "props": { "type": "array", "items": { "type": "string" } },
                        "lighting": {
                            "type": "object",
                            "properties": {
                                "key": { "type": "string", "enum": LIGHT_KEYS },
                                "from": { "type": "string", "enum": SIDES },
                                "warm": { "type": "boolean" }
                            },
                            "required": ["key", "from", "warm"],
                            "additionalProperties": false
                        },
                        "shots": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "at_line": { "type": "integer" },
                                    "kind": { "type": "string", "enum": SHOT_KINDS },
                                    "on": { "type": "string" },
                                    "camera": { "type": "string", "enum": CAMERAS }
                                },
                                "required": ["at_line", "kind", "on", "camera"],
                                "additionalProperties": false
                            }
                        },
                        "mood": { "type": "string" }
                    },
                    "required": [
                        "number", "interior", "setting", "location_key", "background_prompt", "present",
                        "absent", "place", "enter", "props", "lighting", "shots", "mood"
                    ],
                    "additionalProperties": false
                }
            }
        },
        "required": ["scenes"],
        "additionalProperties": false
    })
}

const SCENES_SYSTEM: &str = "You are a script supervisor and layout artist breaking down an animated web series for production. \
For every scene given, answer one entry with the scene's own number. \
`interior`: indoors or not. `setting`: daylight, sunset (dawn, dusk, evening), night, interior or storm. \
`location_key`: snake_case words for the place (chhotu_tea_stall); the same place in different scenes MUST get the same key, \
so reuse a key from the list of keys already used when the scene is set there. \
`background_prompt`: 2 to 4 sentences describing only what the camera sees of the empty place: the setting, buildings, objects, ground, sky, time of day, light and mood. \
No people, no characters, no animals that act, no text or signs with words, no art-style words: describe the empty location only; never mention people. \
`present`: the characters seen on screen in this scene (not voice-overs), names as in the character list. \
`absent`: characters the scene mentions who are not there. \
`place`: for every present character, the feet position across the stage, x from 0.15 (left) to 0.85 (right); \
people who talk to each other stand near each other, nobody stands on top of another. \
`enter`: only characters who walk into the scene after it starts, and the side they come from. \
`props`: short everyday nouns for set dressing the scene needs (\"tea stall\", \"wooden bench\", \"kettle\", \"charpoy\"), at most 8. \
`lighting`: key light sun (outdoors by day or at sunset), lamp (a lamp or bulb at night), moon (outdoors at night), window (indoors by day) or none; \
from left, right or front; warm for sunlight, lamps and sunsets. \
`shots`: camera cuts. `at_line` is the [line n] number a cut starts on. The first shot is wide at line 0. \
Add a medium or close shot `on` the speaking character for a long or important line or a punchline, and go back to wide afterwards; \
leave `on` empty for a wide shot. Keep it to a few cuts per scene. \
`camera` is the angle of each shot, chosen from what happens there: wide (the establishing shot, kind wide), \
medium or reverse (dialogue, alternating between two speakers; kind medium), close (a reveal, a reaction, a punchline; kind close), \
low_angle (something big, scary or powerful looms; kind medium), high_angle (looking down from a roof, a fall, someone small; kind medium), \
over_shoulder (a private or tense exchange; kind medium). \
`mood`: two or three words. \
Output strictly valid JSON according to the schema.";

/// Pass 2 for one batch. `None` when the call fails or the answer leaves a
/// scene of the batch out; the caller then keeps the offline scenes.
fn llm_scenes(
    llm: &impl Llm,
    model: &str,
    batch: &[(&ScriptScene, Option<&Slugline>)],
    hints: &BreakdownHints,
    characters: &[CharacterBrief],
    keys_used: &BTreeMap<String, String>,
    progress: &mut dyn FnMut(&str),
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<LlmScene>> {
    let numbers: Vec<u32> = batch.iter().map(|(s, _)| s.number).collect();
    let cast: Vec<String> = characters
        .iter()
        .map(|c| {
            let mut line = c.name.clone();
            if !c.aliases.is_empty() {
                line.push_str(&format!(" (also {})", c.aliases.join(", ")));
            }
            if c.voice_only {
                line.push_str(" [voice only]");
            }
            line
        })
        .collect();
    let keys: Vec<String> = keys_used.iter().map(|(k, slug)| format!("{k} ({slug})")).collect();
    let scenes: String = batch.iter().map(|(s, sl)| scene_text(s, *sl)).collect();
    let user = format!(
        "{}Characters: {}\nLocation keys already used: {}\n\nScenes to break down: {}\n\n{scenes}",
        hints_text(hints),
        cast.join("; "),
        if keys.is_empty() { "none yet".to_string() } else { keys.join("; ") },
        numbers.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
    );
    let answer: LlmScenes = match llm.chat_json_cancellable(model, SCENES_SYSTEM, &user, &[], scenes_schema(), cancel) {
        Ok(a) => a,
        Err(e) => {
            progress(&format!("scenes {numbers:?}: model failed ({e}); using the offline breakdown"));
            return None;
        }
    };
    let missing: Vec<u32> = numbers
        .iter()
        .copied()
        .filter(|n| !answer.scenes.iter().any(|s| s.number == *n as i64))
        .collect();
    if !missing.is_empty() {
        progress(&format!("scenes {numbers:?}: the model left out {missing:?}; using the offline breakdown"));
        return None;
    }
    Some(answer.scenes)
}

/// The model's scene over the offline one, field by field: whatever is
/// missing or outside the allowed words keeps the offline value.
fn merge_scene(
    off: &SceneBrief,
    got: LlmScene,
    src: &ScriptScene,
    slug: Option<&Slugline>,
    characters: &[CharacterBrief],
) -> SceneBrief {
    let mut out = off.clone();
    // The slugline's INT./EXT. is a fact; the model decides only when it says neither.
    out.interior = match slug.map(|s| s.place) {
        Some(Place::Interior) => true,
        Some(Place::Exterior) => false,
        _ => got.interior,
    };
    if SETTINGS.contains(&got.setting.as_str()) {
        out.setting = got.setting;
    }
    let key = snake(&got.location_key);
    if !key.is_empty() {
        out.location_key = key;
    }
    if !got.background_prompt.trim().is_empty() {
        out.background_prompt = got.background_prompt.trim().to_string();
    }

    // Who is there: the model's list, never without those who speak on screen.
    let mut present: Vec<String> = Vec::new();
    for n in got.present.iter().chain(&on_screen_speakers(src, characters)) {
        let k = resolve(n, characters);
        if !k.is_empty() && !is_voice_only(&k, characters) && !present.contains(&k) {
            present.push(k);
        }
    }
    out.absent = Vec::new();
    for n in &got.absent {
        let k = resolve(n, characters);
        if !k.is_empty() && !present.contains(&k) && !out.absent.contains(&k) {
            out.absent.push(k);
        }
    }
    out.place = present
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let x = got
                .place
                .iter()
                .find(|p| resolve(&p.name, characters) == *k && p.x.is_finite())
                .map(|p| p.x.clamp(0.15, 0.85))
                .unwrap_or_else(|| spread(i, present.len()));
            (k.clone(), x)
        })
        .collect();
    out.enter = got
        .enter
        .iter()
        .filter(|e| e.from == "left" || e.from == "right")
        .map(|e| (resolve(&e.name, characters), e.from.clone()))
        .filter(|(k, _)| present.contains(k))
        .collect();
    out.present = present;

    let mut props: Vec<String> = Vec::new();
    for p in &got.props {
        let p = p.trim().to_lowercase();
        if !p.is_empty() && !props.contains(&p) {
            props.push(p);
        }
    }
    out.props = props;

    if LIGHT_KEYS.contains(&got.lighting.key.as_str()) {
        out.lighting.key = got.lighting.key;
        out.lighting.warm = got.lighting.warm;
    }
    if SIDES.contains(&got.lighting.from.as_str()) {
        out.lighting.from = got.lighting.from;
    }

    // Cuts on lines that exist, one per line, wide at the start.
    let lines = dialogue_count(src);
    let mut shots: Vec<ShotBrief> = Vec::new();
    for s in got.shots {
        if s.at_line < 0 || (s.at_line as usize >= lines && s.at_line != 0) || !SHOT_KINDS.contains(&s.kind.as_str()) {
            continue;
        }
        let on = if s.on.trim().is_empty() { String::new() } else { resolve(&s.on, characters) };
        if s.kind != "wide" && on.is_empty() {
            continue;
        }
        if shots.iter().any(|x| x.at_line == s.at_line as usize) {
            continue;
        }
        // AN9: the model's camera when it is one of ours and fits the kind; else filled in by `tidy`.
        let camera = if CAMERAS.contains(&s.camera.as_str()) && kind_of_camera(&s.camera) == s.kind { s.camera } else { String::new() };
        shots.push(ShotBrief { at_line: s.at_line as usize, kind: s.kind, on, camera, ..Default::default() });
    }
    shots.sort_by_key(|s| s.at_line);
    if shots.first().is_none_or(|s| s.at_line != 0) {
        shots.insert(0, ShotBrief { at_line: 0, kind: "wide".into(), camera: "wide".into(), ..Default::default() });
    }
    out.shots = shots;
    out.mood = got.mood.trim().to_string();
    out
}

/// **The breakdown, through the model**, with the offline breakdown under it.
///
/// Never fails: pass 1 falls back to the script's cues, and each batch of four
/// scenes that the model gets wrong keeps its offline scenes. Every fallback
/// is said through `progress`.
pub fn breakdown(
    llm: &impl Llm,
    model: &str,
    play: &Screenplay,
    hints: &BreakdownHints,
    progress: &mut dyn FnMut(&str),
) -> Breakdown {
    breakdown_cancellable(llm, model, play, hints, progress, &|| false).expect("never cancelled")
}

/// [`breakdown`] that stops when `cancel` says so (P8-C2): a model call in
/// flight is given up within about a quarter second
/// ([`Llm::chat_json_cancellable`]) and `None` comes back.
pub fn breakdown_cancellable(
    llm: &impl Llm,
    model: &str,
    play: &Screenplay,
    hints: &BreakdownHints,
    progress: &mut dyn FnMut(&str),
    cancel: &dyn Fn() -> bool,
) -> Option<Breakdown> {
    let offline_characters = offline_characters(play, hints);
    let characters = llm_characters(llm, model, play, hints, &offline_characters, progress, cancel);
    if cancel() {
        return None;
    }
    let characters = characters.unwrap_or(offline_characters);
    let slugs = carried_sluglines(play);
    let mut scenes: Vec<SceneBrief> = Vec::new();
    let mut keys_used: BTreeMap<String, String> = BTreeMap::new();
    let all: Vec<(&ScriptScene, Option<&Slugline>)> = play.scenes.iter().zip(slugs.iter().map(Option::as_ref)).collect();
    for batch in all.chunks(BATCH) {
        let numbers: Vec<u32> = batch.iter().map(|(s, _)| s.number).collect();
        progress(&format!("scenes {numbers:?}: asking the model"));
        // The floor for this batch, worked out with the model's characters:
        // their aliases find more people in the action lines.
        let offline_batch: Vec<SceneBrief> = batch
            .iter()
            .map(|(s, sl)| offline_scene(s, *sl, &characters))
            .collect();
        let got = llm_scenes(llm, model, batch, hints, &characters, &keys_used, progress, cancel);
        if cancel() {
            return None;
        }
        match got {
            Some(mut got) => {
                for ((src, slug), off) in batch.iter().zip(&offline_batch) {
                    let i = got.iter().position(|g| g.number == src.number as i64).expect("checked in llm_scenes");
                    scenes.push(merge_scene(off, got.remove(i), src, *slug, &characters));
                }
            }
            None => scenes.extend(offline_batch),
        }
        for s in &scenes[scenes.len() - batch.len()..] {
            keys_used.entry(s.location_key.clone()).or_insert_with(|| s.slugline.clone());
        }
    }
    let mut b = Breakdown { characters, scenes, ..Breakdown::from_hints(play, hints) };
    tidy(&mut b, play);
    Some(b)
}

// -- AN4: tags and poses ------------------------------------------------------------------------

/// `left`, `centre`, `right`, `door` or `30%`, normalised; `None` for anything else.
fn normalise_position(word: &str) -> Option<String> {
    let w = word.trim().to_lowercase();
    match w.as_str() {
        "left" | "l" => Some("left".into()),
        "right" | "r" => Some("right".into()),
        "centre" | "center" | "middle" | "c" => Some("centre".into()),
        "door" | "doorway" => Some("door".into()),
        _ => {
            let n: f64 = w.strip_suffix('%')?.trim().parse().ok()?;
            n.is_finite().then(|| format!("{}%", n.clamp(0.0, 100.0).round()))
        }
    }
}

/// Across the stage (0..1) for a normalised position.
pub fn position_x(position: &str) -> Option<f64> {
    match position {
        "left" => Some(0.2),
        "centre" => Some(0.5),
        "right" => Some(0.8),
        "door" => Some(0.88),
        p => p.strip_suffix('%')?.parse::<f64>().ok().map(|n| (n / 100.0).clamp(0.05, 0.95)),
    }
}

/// **`[Name: pose, position]` tags** in a paragraph of the script. Either part
/// may be left out (`[Guddu: sitting]`, `[Raju: right]`); a pose is any word
/// that is not a position. A bracket without a colon (`[VISUAL]`) is no tag.
pub fn parse_tags(text: &str) -> Vec<StageTag> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else { break };
        let inner = &after[..close];
        rest = &after[close + 1..];
        let Some((name, args)) = inner.split_once(':') else { continue };
        let name = name.trim();
        if name.is_empty() || name.split_whitespace().count() > 3 {
            continue;
        }
        let (mut pose, mut position) = (String::new(), String::new());
        for part in args.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match normalise_position(part) {
                Some(p) => {
                    if position.is_empty() {
                        position = p;
                    }
                }
                None => {
                    if pose.is_empty() {
                        pose = part.to_lowercase().replace(' ', "_");
                    }
                }
            }
        }
        if pose.is_empty() && position.is_empty() {
            continue;
        }
        out.push(StageTag { name: name.to_string(), pose, position });
    }
    out
}

/// The part of a pose word that action lines inflect: `sitting` -> `sit`,
/// `scared` -> `scar`, `pointing` -> `point`.
fn pose_stem(pose: &str) -> String {
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

/// The pose, among `variants`, an action line gives `names` ("Guddu sits on
/// the bench" -> `sitting`); `None` when no line does.
fn pose_from_actions(actions: &[String], names: &[String], variants: &[String]) -> Option<String> {
    for action in actions {
        let lower = action.to_lowercase();
        for clause in lower.split(['.', ',', ';', '!', '?']) {
            if !names.iter().any(|n| mentions(clause, n)) {
                continue;
            }
            for v in variants {
                let stem = pose_stem(v);
                if stem.len() < 3 {
                    continue;
                }
                let hit = clause
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| w.starts_with(&stem) && w.len() <= stem.len() + 4);
                if hit {
                    return Some(v.clone());
                }
            }
        }
    }
    None
}

/// Poses and stage tags, applied to every scene (idempotent; tidy runs it
/// again on a breakdown read from a file). The pose of a character comes from
/// the action lines among the pictures their folder has; a script tag
/// `[Name: pose, position]` then wins: its pose, its place across the stage
/// (`door` also walks them in) and being on stage at all.
fn apply_stage(b: &mut Breakdown, play: &Screenplay) {
    let characters = b.characters.clone();
    let available = b.poses_available.clone();
    for s in &mut b.scenes {
        let Some(src) = play.scene(s.number) else { continue };
        s.poses.clear();
        let actions: Vec<String> = src
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Action(a) => Some(a.clone()),
                _ => None,
            })
            .collect();
        for key in s.present.clone() {
            let Some(c) = characters.iter().find(|c| norm(&c.name) == key) else { continue };
            let variants: Vec<String> = std::iter::once(&c.name)
                .chain(&c.aliases)
                .find_map(|n| available.get(&norm(n)).cloned())
                .unwrap_or_default();
            if variants.is_empty() {
                continue;
            }
            let names: Vec<String> = std::iter::once(&c.name)
                .chain(&c.aliases)
                .map(|n| n.trim().to_lowercase())
                .filter(|n| !n.is_empty())
                .collect();
            if let Some(p) = pose_from_actions(&actions, &names, &variants) {
                s.poses.insert(key, p);
            }
        }
        for item in &src.items {
            let Item::Action(a) = item else { continue };
            for tag in parse_tags(a) {
                let key = resolve(&tag.name, &characters);
                if !characters.iter().any(|c| norm(&c.name) == key) || is_voice_only(&key, &characters) {
                    continue;
                }
                if !s.present.contains(&key) {
                    s.present.push(key.clone());
                    s.absent.retain(|x| *x != key);
                }
                if !tag.pose.is_empty() {
                    s.poses.insert(key.clone(), tag.pose.clone());
                }
                match position_x(&tag.position) {
                    Some(x) => {
                        s.place.insert(key.clone(), x);
                        if tag.position == "door" {
                            s.enter.insert(key.clone(), if x > 0.5 { "right" } else { "left" }.into());
                        }
                    }
                    None => {
                        s.place.entry(key.clone()).or_insert(0.5);
                    }
                }
            }
        }
    }
}

// -- AN9: cameras, looks, prompts ---------------------------------------------------------------

/// What the episode builder frames for a camera: it knows wide / medium / close.
pub fn kind_of_camera(camera: &str) -> &'static str {
    match camera {
        "wide" => "wide",
        "close" => "close",
        _ => "medium",
    }
}

fn has_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// The angle an action line asks for, if it asks for one.
fn camera_from_action(action_lower: &str) -> Option<&'static str> {
    if has_any(action_lower, &["from above", "rooftop", "terrace", "balcony", "looks down", "climbs", "falls from"]) {
        Some("high_angle")
    } else if has_any(action_lower, &["looms", "towers", "enormous", "giant", "stands over", "looks up"]) {
        Some("low_angle")
    } else if has_any(
        action_lower,
        &["suddenly", "reveal", "discover", "gasps", "realis", "realiz", "shocked", "notices", "finds ", "opens the"],
    ) {
        Some("close")
    } else {
        None
    }
}

/// **The camera of each shot with no model**: the establishing shot is wide;
/// a change of speaker cuts to a medium, alternating medium and reverse; a
/// reveal in the action before a line cuts to a close (a low or high angle when
/// the action looms or looks down). At most 8 cuts.
fn offline_shots(scene: &ScriptScene, characters: &[CharacterBrief]) -> Vec<ShotBrief> {
    let mut shots =
        vec![ShotBrief { at_line: 0, kind: "wide".into(), camera: "wide".into(), ..Default::default() }];
    let (mut line, mut flip) = (0usize, false);
    let mut last: Option<String> = None;
    let mut action = String::new();
    for item in &scene.items {
        match item {
            Item::Action(a) => action = a.to_lowercase(),
            Item::Dialogue(d) => {
                let key = resolve(&d.speaker, characters);
                let seen = d.voice == Voice::OnScreen && !is_voice_only(&key, characters);
                if seen && line > 0 && shots.len() < 8 {
                    let camera = match camera_from_action(&action) {
                        Some(c) => Some(c),
                        None if last.as_deref() != Some(key.as_str()) => {
                            flip = !flip;
                            Some(if flip { "medium" } else { "reverse" })
                        }
                        None => None,
                    };
                    if let Some(c) = camera {
                        shots.push(ShotBrief {
                            at_line: line,
                            kind: kind_of_camera(c).into(),
                            on: key.clone(),
                            camera: c.into(),
                            ..Default::default()
                        });
                    }
                }
                if seen {
                    last = Some(key);
                }
                action.clear();
                line += 1;
            }
            _ => {}
        }
    }
    shots
}

/// Gives every shot a camera that fits its kind: the model's was checked in
/// `merge_scene`; a missing one is `wide` / `close`, or medium alternating
/// with reverse. The shot's `kind` follows its camera.
fn fill_cameras(b: &mut Breakdown) {
    for s in &mut b.scenes {
        let mut mediums = 0usize;
        for shot in s.shots.iter_mut() {
            if !CAMERAS.contains(&shot.camera.as_str()) || kind_of_camera(&shot.camera) != shot.kind {
                shot.camera = match shot.kind.as_str() {
                    "wide" => "wide",
                    "close" => "close",
                    _ => {
                        let c = if mediums % 2 == 0 { "medium" } else { "reverse" };
                        mediums += 1;
                        c
                    }
                }
                .to_string();
            } else if kind_of_camera(&shot.camera) == "medium" {
                mediums += 1;
            }
        }
    }
}

/// Words that end a noun phrase (a verb or a preposition starts the next one).
const NOUN_STOPS: &[&str] = &[
    "in", "on", "at", "near", "under", "by", "beside", "behind", "from", "that", "which", "who", "where", "while",
    "is", "are", "was", "were", "stands", "sits", "leans", "hangs", "lies", "flickers", "rises", "stood", "sat",
    "over", "above", "across", "against", "towards", "toward", "into", "through", "as",
];
const PERSON_WORDS: &[&str] = &[
    "man", "woman", "boy", "girl", "child", "children", "people", "person", "villager", "villagers", "figure", "he",
    "she", "they", "lady", "men", "women", "kid", "kids", "mother", "father",
];

fn is_article(w: &str) -> bool {
    matches!(w.to_lowercase().as_str(), "a" | "an" | "the" | "one" | "two" | "three" | "some")
}

/// One noun phrase from `piece` if it starts with an article: up to the first
/// stop word (a following "with ..." stays), at most nine words; `None` for a person.
fn noun_phrase(piece: &str) -> Option<String> {
    let words: Vec<&str> = piece.split_whitespace().collect();
    if words.len() < 2 || !is_article(words[0].trim_matches(|c: char| !c.is_alphanumeric())) {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for w in &words {
        let bare = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if !out.is_empty() && NOUN_STOPS.contains(&bare.as_str()) {
            break;
        }
        out.push(w);
        if out.len() >= 9 {
            break;
        }
    }
    let phrase = out.join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    let lower = phrase.to_lowercase();
    let people = lower.split_whitespace().any(|w| PERSON_WORDS.contains(&w));
    (phrase.split_whitespace().count() >= 2 && !people).then_some(phrase)
}

/// **AN13: the set's concrete nouns**: the list items of the scene's first action
/// line ("A narrow mud lane, an old peepal tree, a broken lamp post.") and the
/// noun phrase after a preposition in the slugline ("... NEAR AN OLD PEEPAL TREE").
/// Only phrases that start with an article are taken, people are left out.
pub fn set_nouns(slugline: &str, first_action: Option<&str>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |p: String| {
        let low = p.to_lowercase();
        if !found.iter().any(|f| f.to_lowercase().contains(&low) || low.contains(&f.to_lowercase())) {
            found.push(p);
        }
    };
    if let Some(action) = first_action {
        let first_sentence = action.split(['.', '!', '?']).next().unwrap_or("");
        for piece in first_sentence.replace(';', ",").split(',') {
            let piece = piece.trim();
            let piece = piece.strip_prefix("and ").unwrap_or(piece);
            if let Some(p) = noun_phrase(piece) {
                add(p);
            }
        }
    }
    let low = slugline.to_lowercase();
    let place = low.split(['\u{2014}', '\u{2013}']).next().unwrap_or("").split(" - ").next().unwrap_or("");
    for prep in [" near ", " by ", " beside ", " outside ", " behind ", " under ", " at "] {
        if let Some((_, rest)) = place.split_once(prep) {
            if let Some(p) = noun_phrase(rest.trim()) {
                add(p);
            }
            break;
        }
    }
    found.truncate(5);
    found
}

/// Nouns first: "A narrow mud lane, an old peepal tree." then the rest.
fn lead_with_nouns(nouns: &[String], rest: String) -> String {
    if nouns.is_empty() {
        return rest;
    }
    let lead = nouns.join(", ");
    format!("{}. {rest}", sentence_case_first(&lead))
}

/// At most `max` words, cut at the last full word and closed with a full stop.
fn cap_words(text: &str, max: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= max {
        return text.to_string();
    }
    let cut = words[..max].join(" ");
    format!("{}.", cut.trim_end_matches(|c: char| !c.is_alphanumeric()))
}

/// Words that say nothing to a picture generator.
const VAGUE_WORDS: &[&str] =
    &["beautiful", "nice", "lovely", "amazing", "stunning", "pretty", "gorgeous", "wonderful", "good", "great"];

fn strip_vague(text: &str) -> String {
    text.split(' ')
        .filter(|w| {
            let bare = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            !VAGUE_WORDS.contains(&bare.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The framing words of one camera angle.
fn camera_words(camera: &str) -> &'static str {
    match camera {
        "wide" => "Wide establishing view of the whole place, far back so all of it is in frame",
        "close" => "Close view on one detail of the place, shallow depth of field",
        "low_angle" => "Low camera near the ground looking up, walls and sky towering above",
        "high_angle" => "High camera looking down on the place from above, ground and rooftops seen from over their tops",
        "over_shoulder" => "Camera at shoulder height looking along the place, a soft dark foreground edge framing the view",
        "reverse" => "The reverse view: the camera turned about to look at the opposite side of the place",
        _ => "Medium view at eye level, the middle of the place in focus",
    }
}

/// FNV-1a of the location key, in 48 bits.
fn seed_family_of(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h & 0xFFFF_FFFF_FFFF
}

fn setting_phrase(setting: &str) -> &'static str {
    match setting {
        "sunset" => "at sunset",
        "night" => "at night",
        "interior" => "indoors",
        "storm" => "during a storm",
        _ => "by day",
    }
}

/// **A location's look**: the place and time from the slugline, the scene's own
/// description (people taken out), the weather and the light, the props as
/// nouns, the world (era, region) and the series style text.
fn location_look(s: &SceneBrief, world: &str, style: &str) -> String {
    let mut out = full_background_prompt(s, world);
    out.push(' ');
    out.push_str(match s.setting.as_str() {
        "storm" => "Heavy storm clouds, rain-wet ground, wind in the trees.",
        "night" => "Clear night sky, dark ground with pools of lamplight.",
        "sunset" => "Evening sky turning orange, long shadows.",
        "interior" => "Indoors, no sky.",
        _ => "Clear daytime sky, dry ground.",
    });
    let source = match s.lighting.key.as_str() {
        "sun" => "sunlight",
        "lamp" => "lamp light",
        "moon" => "moonlight",
        "window" => "daylight through a window",
        _ => "",
    };
    if !source.is_empty() {
        let side = match s.lighting.from.as_str() {
            "left" => " from the left",
            "right" => " from the right",
            _ => "",
        };
        out.push_str(&format!(" {} {source}{side}.", if s.lighting.warm { "Warm" } else { "Cool" }));
    }
    if !s.mood.trim().is_empty() {
        out.push_str(&format!(" Mood: {}.", s.mood.trim().to_lowercase()));
    }
    let mut out = with_details(out, &s.props);
    if !style.trim().is_empty() {
        out.push_str(&format!(" Style: {}.", style.trim().trim_end_matches('.')));
    }
    // AN13: the concrete set nouns lead; the text encoder handles ~70 words well.
    cap_words(&lead_with_nouns(&s.set_nouns, strip_vague(&out)), 70)
}

/// One shot's plate prompt: the look, the camera's framing words, nobody in frame.
fn shot_prompt(look: &str, other_setting: Option<&str>, camera: &str) -> String {
    let when = other_setting.map(|s| format!(" This shot is {}.", setting_phrase(s))).unwrap_or_default();
    format!("{}{when} {}. Nobody in frame, empty of people.", look.trim_end(), camera_words(camera))
}

/// One look and one seed family per location (the first scene set there
/// writes it), and every shot's plate prompt from its location's look and its
/// own camera. A scene of the same place at another time of day says so.
fn apply_looks(b: &mut Breakdown) {
    let (world, style) = (b.world.clone(), b.style.clone());
    let key_of = |s: &SceneBrief| {
        if s.location_key.trim().is_empty() { format!("scene_{}", s.number) } else { s.location_key.trim().to_string() }
    };
    let mut looks: BTreeMap<String, LocationLook> = BTreeMap::new();
    for s in &b.scenes {
        let key = key_of(s);
        if !looks.contains_key(&key) {
            let look = location_look(s, &world, &style);
            looks.insert(key.clone(), LocationLook { look, seed_family: seed_family_of(&key), setting: s.setting.clone() });
        }
    }
    for s in &mut b.scenes {
        let key = key_of(s);
        let Some(look) = looks.get(&key) else { continue };
        let other = (look.setting != s.setting).then(|| s.setting.clone());
        for shot in &mut s.shots {
            shot.seed_family = look.seed_family;
            shot.background_prompt = shot_prompt(&look.look, other.as_deref(), &shot.camera);
        }
    }
    b.looks = looks;
}

// -- the casting file -----------------------------------------------------------------------

/// The background prompt the generator gets: "<place>, <time>. <prompt>
/// <world>." A model's prompt often leaves out the place's name, the time of
/// day or the country, and a generator then draws a generic street at noon.
///
/// The place is the slugline's, in plain words, unless the scene is somewhere
/// else than the slugline it carries (scene 8's store under scene 7's
/// `INT. RAJU'S HOUSE`): then the location key's words. The time is the
/// slugline's in the same case, else the setting's.
fn full_background_prompt(s: &SceneBrief, world: &str) -> String {
    let raw = s.slugline.trim();
    let rest = ["INT./EXT.", "EXT./INT.", "INT/EXT.", "I/E.", "INT.", "EXT."]
        .iter()
        .find_map(|p| raw.get(..p.len()).filter(|h| h.eq_ignore_ascii_case(p)).map(|_| &raw[p.len()..]))
        .unwrap_or(raw);
    let (slug_place, slug_time) = match rest.split_once(['\u{2014}', '\u{2013}']).or_else(|| rest.split_once(" - ")) {
        Some((p, t)) => (p.trim(), t.trim()),
        None => (rest.trim(), ""),
    };
    let slug_key = snake(slug_place);
    // Same place when any long word of the key is in the slugline's place.
    let same_place = !slug_key.is_empty()
        && (s.location_key.is_empty()
            || s.location_key.split('_').filter(|w| w.len() > 3).any(|w| slug_key.contains(w)));
    let (place, time) = if same_place {
        (sentence_case(slug_place), slug_time.to_lowercase())
    } else {
        (sentence_case(&s.location_key.replace('_', " ")), String::new())
    };
    let time = if !time.is_empty() {
        time
    } else {
        match s.setting.as_str() {
            "sunset" => "evening",
            "night" => "night",
            "storm" => "stormy day",
            _ => "day",
        }
        .to_string()
    };
    let head = format!("{place}, {time}.");
    let mut prompt = s.background_prompt.trim().to_string();
    if prompt.to_lowercase().starts_with(&head.to_lowercase()) {
        prompt = prompt.get(head.len()..).unwrap_or("").trim().to_string();
    }
    let mut out = head;
    if !prompt.is_empty() {
        out.push(' ');
        out.push_str(&prompt);
        if !out.ends_with(['.', '!', '?']) {
            out.push('.');
        }
    }
    let world = world.trim().trim_end_matches('.');
    if !world.is_empty() {
        out.push(' ');
        out.push_str(&sentence_case_first(world));
        out.push('.');
    }
    out
}

/// Upper-cases the first letter only, keeping the rest as written ("rural
/// Bihar" → "Rural Bihar").
fn sentence_case_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// **The casting file's JSON** (`buzz_app::episode::Casting`) for a breakdown.
///
/// `cast_names` maps a script name (or alias) to a rig in the Cast folder; a
/// name in it the breakdown does not know is cast all the same, since the
/// user said so. Voice-only characters are `voice_only`; a character with no
/// rig is listed with its aliases only, which the builder skips and reports.
/// Each scene carries what the builder reads today (`setting`, `place` with
/// `x` and `enter`, `absent`) and what the producer and P5 read: `location`
/// (the key), `background_prompt`, `lighting`, `shots` and the props as index
/// words under `props_wanted` (the builder's own `props` are placed assets).
pub fn casting_value(b: &Breakdown, cast_names: &BTreeMap<String, String>) -> Value {
    let rig_for = |c: &CharacterBrief| {
        cast_names
            .iter()
            .find(|(k, _)| norm(k) == norm(&c.name) || c.aliases.iter().any(|a| norm(a) == norm(k)))
            .map(|(_, v)| v.clone())
    };
    let mut characters = serde_json::Map::new();
    for c in &b.characters {
        let mut entry = serde_json::Map::new();
        if c.voice_only {
            entry.insert("voice_only".into(), json!(true));
        } else if let Some(rig) = rig_for(c) {
            entry.insert("cast".into(), json!(rig));
        }
        if !c.aliases.is_empty() {
            entry.insert("aliases".into(), json!(c.aliases));
        }
        characters.insert(norm(&c.name), Value::Object(entry));
    }
    for (name, rig) in cast_names {
        let known = b
            .characters
            .iter()
            .any(|c| norm(&c.name) == norm(name) || c.aliases.iter().any(|a| norm(a) == norm(name)));
        if !known {
            characters.insert(norm(name), json!({ "cast": rig }));
        }
    }

    let mut scenes = serde_json::Map::new();
    for s in &b.scenes {
        let mut place = serde_json::Map::new();
        for (name, x) in &s.place {
            let mut spec = serde_json::Map::new();
            spec.insert("x".into(), json!((x * 1000.0).round() / 1000.0));
            if let Some(side) = s.enter.get(name) {
                spec.insert("enter".into(), json!(side));
            }
            // AN4: the pose (`apply_pose_rigs` turns it into a Cast rig).
            if let Some(pose) = s.poses.get(name) {
                spec.insert("pose".into(), json!(pose));
            }
            place.insert(name.clone(), Value::Object(spec));
        }
        let shots: Vec<Value> = s
            .shots
            .iter()
            .map(|sh| {
                let mut v = json!({
                    "at_line": sh.at_line,
                    "kind": sh.kind,
                    // AN9: the camera angle and the prompt this shot's plate is drawn from.
                    "camera": sh.camera,
                    "background_prompt": sh.background_prompt,
                    "seed_family": sh.seed_family,
                });
                if !sh.on.is_empty() {
                    v["on"] = json!(sh.on);
                }
                v
            })
            .collect();
        scenes.insert(
            s.number.to_string(),
            json!({
                "setting": s.setting,
                "place": place,
                "absent": s.absent,
                "props_wanted": s.props,
                "location": s.location_key,
                "background_prompt": cap_words(&lead_with_nouns(&s.set_nouns, with_details(full_background_prompt(s, &b.world), &s.props)), 70),
                "lighting": s.lighting,
                "shots": shots,
            }),
        );
    }
    let looks: serde_json::Map<String, Value> = b
        .looks
        .iter()
        .map(|(k, l)| (k.clone(), json!({ "look": l.look, "seed_family": l.seed_family })))
        .collect();
    json!({ "characters": characters, "scenes": scenes, "looks": looks })
}

/// **AN4**: turns each placed pose into the pose picture's Cast rig. `rigs`
/// maps a character key (upper-case) to `pose -> Cast rig`; a scene's place
/// entry whose pose has a rig gets it as `cast` (the builder's per-scene rig
/// swap). A pose without a rig is left as the base rig.
pub fn apply_pose_rigs(casting: &mut Value, rigs: &BTreeMap<String, BTreeMap<String, String>>) {
    let Some(scenes) = casting["scenes"].as_object_mut() else { return };
    for set in scenes.values_mut() {
        let Some(place) = set["place"].as_object_mut() else { continue };
        for (name, spec) in place.iter_mut() {
            let Some(pose) = spec["pose"].as_str().map(str::to_string) else { continue };
            let Some(by_pose) = rigs.get(&norm(name)) else { continue };
            let wanted = pose.to_lowercase().replace(' ', "_");
            if let Some(rig) = by_pose.get(&wanted) {
                spec["cast"] = json!(rig);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_tags_give_pose_and_position() {
        let t = parse_tags("The bench. [Guddu: sitting, left] and [Raju: scared, 30%] [VISUAL] [SOUND: wind]");
        assert_eq!(t[0], StageTag { name: "Guddu".into(), pose: "sitting".into(), position: "left".into() });
        assert_eq!(t[1], StageTag { name: "Raju".into(), pose: "scared".into(), position: "30%".into() });
        assert_eq!(position_x("left"), Some(0.2));
        assert_eq!(position_x("30%"), Some(0.3));
        // A tag wins over the offline placement.
        let play = Screenplay::parse("## SCENE 1 \u{2014} BENCH\n\n**EXT. TEA STALL \u{2014} DAY**\n\nGuddu waits. [Guddu: sitting, left]\n\n**GUDDU**\nHello.\n\n**RAJU**\nHi.\n");
        let b = breakdown_offline(&play, &BreakdownHints::default());
        assert_eq!(b.scenes[0].place["GUDDU"], 0.2);
        assert_eq!(b.scenes[0].poses["GUDDU"], "sitting");
    }

    #[test]
    fn offline_cameras_wide_then_medium_reverse() {
        let play = Screenplay::parse(
            "## SCENE 1 \u{2014} TALK\n\n**EXT. TEA STALL \u{2014} DAY**\n\nDusty lane, a tin roof.\n\n**GUDDU**\nOne.\n\n**RAJU**\nTwo.\n\n**GUDDU**\nThree.\n\n**RAJU**\nFour.\n",
        );
        let b = breakdown_offline(&play, &BreakdownHints { world: "rural Bihar, 2010s".into(), ..Default::default() });
        let cams: Vec<&str> = b.scenes[0].shots.iter().map(|s| s.camera.as_str()).collect();
        assert_eq!(cams, ["wide", "medium", "reverse", "medium"]);
        let wide = &b.scenes[0].shots[0];
        let low = shot_prompt("Tea stall.", None, "low_angle");
        assert!(low.contains("Low camera near the ground looking up") && low.contains("Nobody in frame"));
        assert!(wide.background_prompt.contains("Rural Bihar") && wide.seed_family == b.scenes[0].shots[1].seed_family);
    }

    /// AN13: the slugline's and first action line's set nouns lead the prompt, capped at ~70 words.
    #[test]
    fn set_nouns_lead_the_background_prompt() {
        let nouns = set_nouns(
            "EXT. VILLAGE LANE NEAR AN OLD PEEPAL TREE \u{2014} NIGHT",
            Some("A narrow mud lane, an old peepal tree, a broken lamp post. Ramesh walks in."),
        );
        assert!(nouns.iter().any(|n| n == "an old peepal tree") && nouns.iter().any(|n| n == "a broken lamp post"), "{nouns:?}");
        let s = SceneBrief { set_nouns: nouns, setting: "night".into(), ..Default::default() };
        let look = location_look(&s, "rural Bihar, India", "painterly");
        assert!(look.starts_with("A narrow mud lane, an old peepal tree"), "{look}");
        assert!(look.split_whitespace().count() <= 70);
    }
}
