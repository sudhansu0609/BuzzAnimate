//! **Story to screenplay** (ANIMATE_COPILOT_PLAN B1): the creator's first-person
//! Hinglish narration, people described and not named, into the producer's
//! screenplay format. A port of the Studio's verbatim converter
//! (`animate_screenplay.py`).
//!
//! The model only extracts a stable cast and LABELS numbered units of the story
//! (scene breaks, speakers, tags, action lines). This code splits the story into
//! units, COPIES his sentences into the screenplay, then verifies that the
//! spoken text is unchanged. The model never returns text to be spoken.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::llm::{Backend, Llm, LlmError};

/// Units labelled per model call.
const BATCH: usize = 120;
const QUOTE_MARKS: [char; 5] = ['"', '\u{201C}', '\u{201D}', '\u{00AB}', '\u{00BB}'];

/// Why a conversion stopped.
#[derive(Debug, thiserror::Error)]
pub enum StoryError {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

fn fail<T>(msg: impl Into<String>) -> Result<T, StoryError> {
    Err(StoryError::Failed(msg.into()))
}

/// Whitespace runs collapsed to one space, ends trimmed.
pub fn norm(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// --- 1. units ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Narr,
    Quote,
}

/// One numbered piece of the story, exact text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub n: usize,
    pub kind: UnitKind,
    pub text: String,
}

/// The first quoted span at or after `from`: (start, end, inner), for `"…"`,
/// `“…”` and `«…»`; the inner text is never empty.
fn next_quote(s: &str, from: usize) -> Option<(usize, usize, &str)> {
    let mut idx = from;
    while idx < s.len() {
        let c = s[idx..].chars().next()?;
        let close = match c {
            '"' => Some('"'),
            '\u{201C}' => Some('\u{201D}'),
            '\u{00AB}' => Some('\u{00BB}'),
            _ => None,
        };
        if let Some(close) = close {
            let inner_start = idx + c.len_utf8();
            if let Some(rel) = s[inner_start..].find(close) {
                if rel > 0 {
                    let inner_end = inner_start + rel;
                    return Some((idx, inner_end + close.len_utf8(), &s[inner_start..inner_end]));
                }
            }
        }
        idx += c.len_utf8();
    }
    None
}

/// Splits after a sentence end (`. ! ? । …`) wherever whitespace follows.
fn split_sentences(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<char> = None;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if c.is_whitespace() && prev.is_some_and(|p| matches!(p, '.' | '!' | '?' | '\u{0964}' | '\u{2026}')) {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = iter.peek() {
                if d.is_whitespace() {
                    end = j + d.len_utf8();
                    iter.next();
                } else {
                    break;
                }
            }
            start = end;
            prev = None;
            continue;
        }
        prev = Some(c);
    }
    out.push(&s[start..]);
    out
}

/// Numbered units, exact text: quoted speech (`Quote`) and narration sentences (`Narr`).
pub fn split_units(script: &str) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    let mut add = |kind: UnitKind, text: &str| {
        let text = norm(text);
        if !text.is_empty() {
            units.push(Unit { n: units.len() + 1, kind, text });
        }
    };
    let mut pos = 0;
    while let Some((a, b, inner)) = next_quote(script, pos) {
        for s in split_sentences(&script[pos..a]) {
            add(UnitKind::Narr, s);
        }
        add(UnitKind::Quote, inner);
        pos = b;
    }
    for s in split_sentences(&script[pos..]) {
        add(UnitKind::Narr, s);
    }
    units
}

/// The story as it is spoken: quote marks gone, whitespace normalised.
pub fn source_spoken(script: &str) -> String {
    let cleaned: String = script.chars().map(|c| if QUOTE_MARKS.contains(&c) { ' ' } else { c }).collect();
    norm(&cleaned)
}

/// True when the text already has a `## SCENE` heading (a screenplay, not a story).
pub fn is_screenplay(text: &str) -> bool {
    text.lines().any(|l| l.trim_start().to_uppercase().starts_with("## SCENE"))
}

// --- model access -----------------------------------------------------------------

/// The endpoints the converter tries in turn: the OpenAI-compatible Gemini
/// harness (`http://127.0.0.1:8001/v1`, model `flash`) first, then LM Studio
/// (its loaded Gemma, else the first chat model). A failing endpoint passes the
/// call on; cancellation stops at once.
pub struct Endpoints {
    list: Vec<(Backend, Option<String>)>,
    resolved: Mutex<BTreeMap<usize, String>>,
}

impl Endpoints {
    pub fn standard() -> Self {
        Self::new(vec![
            (Backend::OpenAiCompatible { base_url: "http://127.0.0.1:8001/v1".into() }, Some("flash".into())),
            (Backend::default_openai(), None),
        ])
    }

    pub fn new(list: Vec<(Backend, Option<String>)>) -> Self {
        Self { list, resolved: Mutex::new(BTreeMap::new()) }
    }

    fn model_for(&self, i: usize) -> Result<String, LlmError> {
        if let Some(m) = &self.list[i].1 {
            return Ok(m.clone());
        }
        if let Some(m) = self.resolved.lock().unwrap().get(&i) {
            return Ok(m.clone());
        }
        let chat: Vec<String> = self.list[i].0.list_models()?.into_iter().filter(|id| !id.to_lowercase().contains("embed")).collect();
        let pick = chat.iter().find(|id| id.to_lowercase().contains("gemma")).or(chat.first()).cloned();
        match pick {
            Some(m) => {
                self.resolved.lock().unwrap().insert(i, m.clone());
                Ok(m)
            }
            None => Err(LlmError::Other(format!("{} has no chat model", self.list[i].0.display_name()))),
        }
    }

    fn try_each<T>(&self, cancel: &dyn Fn() -> bool, mut call: impl FnMut(&Backend, &str) -> Result<T, LlmError>) -> Result<T, LlmError> {
        let mut errors: Vec<String> = Vec::new();
        for i in 0..self.list.len() {
            if cancel() {
                return Err(LlmError::Other("cancelled".into()));
            }
            let name = self.list[i].0.display_name();
            let result = self.model_for(i).and_then(|m| call(&self.list[i].0, &m));
            match result {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if cancel() {
                        return Err(LlmError::Other("cancelled".into()));
                    }
                    errors.push(format!("{name}: {e}"));
                }
            }
        }
        Err(LlmError::Other(format!("no model answered ({})", errors.join("; "))))
    }
}

impl Llm for Endpoints {
    fn chat_json<T: DeserializeOwned>(&self, _model: &str, system: &str, user: &str, images: &[&[u8]], schema: Value) -> Result<T, LlmError> {
        self.try_each(&|| false, |b, m| b.chat_json::<T>(m, system, user, images, schema.clone()))
    }

    fn chat_json_cancellable<T: DeserializeOwned + Send + 'static>(
        &self,
        _model: &str,
        system: &str,
        user: &str,
        images: &[&[u8]],
        schema: Value,
        cancel: &dyn Fn() -> bool,
    ) -> Result<T, LlmError> {
        self.try_each(cancel, |b, m| b.chat_json_cancellable::<T>(m, system, user, images, schema.clone(), cancel))
    }

    fn embed(&self, _model: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, LlmError> {
        self.try_each(&|| false, |b, m| b.embed(m, texts))
    }
}

/// Finds the first object holding any of `keys`, however deep the model nested it.
fn find<'a>(data: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    match data {
        Value::Object(map) => {
            if keys.iter().any(|k| map.contains_key(*k)) {
                return Some(data);
            }
            map.values().find_map(|v| find(v, keys))
        }
        Value::Array(list) => list.iter().find_map(|v| find(v, keys)),
        _ => None,
    }
}

fn ask(
    llm: &impl Llm,
    model: &str,
    system: &str,
    task: &Value,
    schema: Value,
    keys: &[&str],
    cancel: &dyn Fn() -> bool,
) -> Result<Value, StoryError> {
    if cancel() {
        return Err(StoryError::Cancelled);
    }
    let user = serde_json::to_string_pretty(task).unwrap_or_default();
    let data: Value = match llm.chat_json_cancellable::<Value>(model, system, &user, &[], schema, cancel) {
        Ok(v) => v,
        Err(_) if cancel() => return Err(StoryError::Cancelled),
        Err(e) => return fail(format!("the model failed: {e}")),
    };
    match find(&data, keys) {
        Some(v) => Ok(v.clone()),
        None => fail(format!("the model returned no usable {}", keys.join("/"))),
    }
}

// --- 2. cast ----------------------------------------------------------------------

/// One person of the story, as the registry keeps them. Fields the app adds later
/// (`extra`) are preserved when the registry is rewritten.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CastMember {
    pub id: String,
    pub name: String,
    pub role: String,
    pub gender: String,
    pub age: String,
    pub look: String,
    pub clothes: String,
    pub aliases: Vec<String>,
    pub first_person: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl CastMember {
    pub fn from_value(v: &Value) -> Option<Self> {
        let map = v.as_object()?;
        let get = |k: &str| map.get(k).map(text_of).unwrap_or_default();
        let known = ["id", "name", "role", "gender", "age", "look", "clothes", "aliases", "first_person"];
        Some(Self {
            id: get("id"),
            name: get("name"),
            role: get("role"),
            gender: get("gender"),
            age: get("age"),
            look: get("look"),
            clothes: get("clothes"),
            aliases: map.get("aliases").and_then(Value::as_array).map(|a| a.iter().map(text_of).collect()).unwrap_or_default(),
            first_person: map.get("first_person").and_then(Value::as_bool).unwrap_or(false),
            extra: map.iter().filter(|(k, _)| !known.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
        })
    }

    fn keys(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = [norm(&self.name).to_lowercase(), norm(&self.id).to_lowercase()].into_iter().collect();
        out.extend(self.aliases.iter().map(|a| norm(a).to_lowercase()));
        out.remove("");
        out
    }
}

/// `<asset library>/Cast/<Series>/cast_registry.json`.
pub fn cast_registry_path(assets_root: &Path, series: &str) -> PathBuf {
    let cleaned: String = series.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    let mut dir = if cleaned.is_empty() { "Episode".to_string() } else { cleaned };
    // "Chirailiya" and "Originals" are one series; an existing Cast/Chirailiya/ is still read.
    if dir.eq_ignore_ascii_case("chirailiya") || dir.eq_ignore_ascii_case("originals") {
        let old = assets_root.join("Cast").join("Chirailiya");
        dir = if old.is_dir() && !assets_root.join("Cast").join("Originals").is_dir() { "Chirailiya".to_string() } else { "Originals".to_string() };
    }
    assets_root.join("Cast").join(dir).join("cast_registry.json")
}

fn load_registry(path: &Path) -> Vec<CastMember> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(CastMember::from_value).collect()))
        .unwrap_or_default()
}

fn same(a: &CastMember, b: &CastMember, generic: &BTreeSet<String>) -> bool {
    if a.first_person && b.first_person {
        return true;
    }
    // A word the story uses for two people ("vanrakshak" for both guards) names
    // nobody: it merged two guards into one character.
    let (ka, kb) = (a.keys(), b.keys());
    if ka.iter().any(|k| !generic.contains(k) && kb.contains(k)) {
        return true;
    }
    // Same role and the same written look, when there is a look to compare (two
    // blank looks matched every guard to the first one).
    let (look_a, look_b) = (norm(&a.look).to_lowercase(), norm(&b.look).to_lowercase());
    !a.role.is_empty() && !look_a.is_empty() && norm(&a.role).to_lowercase() == norm(&b.role).to_lowercase() && look_a == look_b
}

/// Returns (the episode's cast with registry ids/names/looks reused, the updated registry).
pub fn merge_registry(cast: Vec<CastMember>, registry: Vec<CastMember>) -> (Vec<CastMember>, Vec<CastMember>) {
    let mut registry = registry;
    // A word used for more than one person -- in this episode or across the
    // channel's registry -- identifies nobody. Counted in each separately: the
    // same person in both is one person, not two.
    fn shared(people: &[CastMember]) -> BTreeSet<String> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for c in people {
            for k in c.keys() {
                *counts.entry(k).or_default() += 1;
            }
        }
        counts.into_iter().filter(|(_, n)| *n > 1).map(|(k, _)| k).collect()
    }
    let mut generic = shared(&cast);
    generic.extend(shared(&registry));
    let mut taken: BTreeSet<usize> = BTreeSet::new(); // two people of one episode never become one cast member
    let mut out = Vec::new();
    for c in cast {
        let hit = (0..registry.len()).find(|i| !taken.contains(i) && same(&c, &registry[*i], &generic));
        match hit {
            Some(i) => {
                taken.insert(i);
                let h = &mut registry[i];
                let mut merged = c.clone();
                if !h.id.is_empty() {
                    merged.id = h.id.clone();
                }
                if !h.name.is_empty() {
                    merged.name = h.name.clone();
                }
                if !h.look.is_empty() {
                    merged.look = h.look.clone();
                }
                if !h.clothes.is_empty() {
                    merged.clothes = h.clothes.clone();
                }
                if !h.gender.is_empty() {
                    merged.gender = h.gender.clone();
                }
                if !h.age.is_empty() {
                    merged.age = h.age.clone();
                }
                let aliases: BTreeSet<String> = h.aliases.iter().chain(c.aliases.iter()).cloned().collect();
                h.aliases = aliases.into_iter().collect();
                out.push(merged);
            }
            None => {
                registry.push(c.clone());
                taken.insert(registry.len() - 1);
                out.push(c);
            }
        }
    }
    (out, registry)
}

const CAST_SYSTEM: &str = "You are the character planner of an animated web series.";

fn cast_schema() -> Value {
    let s = json!({"type": "string"});
    json!({"type": "object", "additionalProperties": false, "required": ["cast"], "properties": {"cast": {"type": "array", "items": {
        "type": "object", "additionalProperties": false,
        "required": ["id", "name", "role", "gender", "age", "look", "clothes", "aliases", "first_person"],
        "properties": {"id": s, "name": s, "role": s, "gender": s, "age": s, "look": s, "clothes": s,
            "aliases": {"type": "array", "items": s}, "first_person": {"type": "boolean"}}}}}})
}

fn upper_id(raw: &str) -> String {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join("_").to_uppercase();
    let id: String = joined.chars().filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect();
    if id.is_empty() { "X".into() } else { id }
}

pub fn extract_cast(llm: &impl Llm, model: &str, script: &str, registry: &[CastMember], cancel: &dyn Fn() -> bool) -> Result<Vec<CastMember>, StoryError> {
    let task = json!({
        "task": "cast_extraction",
        "instruction": "Read this first-person Hinglish story and list every person in it as a stable cast. \
People are described, not named ('ek budhi aurat', 'truck wala'): give each a short descriptive stable name \
('Budhi Amma', 'Truck Driver'). The first-person narrator (main/maine/mujhe) is a character too (first_person true). \
Return ONLY JSON {\"cast\":[{\"id\":\"UPPER_SNAKE\",\"name\":\"\",\"role\":\"\",\"gender\":\"\",\"age\":\"\",\"look\":\"\",\
\"clothes\":\"\",\"aliases\":[\"phrases the text uses for them\"],\"first_person\":false}]}. \
Reuse an id/name from known_cast when it is the same person. Every distinct person gets their own id, even when two \
share a role (two guards are two people). `look` is permanent appearance only (age, build, face, hair, skin) from the \
text, never a passing state (scared, pale, tired); never write any spoken line.",
        "known_cast": registry.iter().map(|r| json!({"id": r.id, "name": r.name, "role": r.role, "aliases": r.aliases, "look": r.look, "first_person": r.first_person})).collect::<Vec<_>>(),
        "story": script,
    });
    let found = ask(llm, model, CAST_SYSTEM, &task, cast_schema(), &["cast"], cancel)?;
    let mut cast = Vec::new();
    for v in found.get("cast").and_then(Value::as_array).into_iter().flatten() {
        let Some(mut c) = CastMember::from_value(v) else { continue };
        if norm(&c.name).is_empty() {
            continue;
        }
        c.name = norm(&c.name);
        c.id = upper_id(if c.id.trim().is_empty() { &c.name } else { &c.id });
        cast.push(c);
    }
    if cast.is_empty() {
        return fail("the cast came back empty");
    }
    Ok(cast)
}

// --- 3. labels --------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Tag {
    pub character: String,
    pub pose: Option<String>,
    pub position: Option<String>,
}

/// What the model said about the units: scenes {n: slugline}, speakers {n: id},
/// tags {n: [..]}, actions {n: [..]}.
#[derive(Debug, Clone, Default)]
pub struct Labels {
    pub scenes: BTreeMap<usize, String>,
    pub speakers: BTreeMap<usize, String>,
    pub tags: BTreeMap<usize, Vec<Tag>>,
    pub actions: BTreeMap<usize, Vec<String>>,
}

fn as_n(v: Option<&Value>) -> Option<usize> {
    match v? {
        Value::Number(n) => n.as_u64().map(|n| n as usize).or_else(|| n.as_f64().map(|f| f as usize)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn str_of(v: Option<&Value>) -> Option<String> {
    v.map(text_of).filter(|s| !s.is_empty())
}

const LABEL_SYSTEM: &str = "You are the storyboard planner of an animated web series. You label story units; you never write text to be spoken.";

fn labels_schema() -> Value {
    let s = json!({"type": "string"});
    let i = json!({"type": "integer"});
    let list = |props: Value, req: Vec<&str>| json!({"type": "array", "items": {"type": "object", "additionalProperties": false, "required": req, "properties": props}});
    json!({"type": "object", "additionalProperties": false, "required": ["scenes", "speakers", "tags", "actions"], "properties": {
        "scenes": list(json!({"start": i, "slugline": s}), vec!["start", "slugline"]),
        "speakers": list(json!({"n": i, "id": s}), vec!["n", "id"]),
        "tags": list(json!({"before": i, "character": s, "pose": s, "position": s}), vec!["before", "character", "pose", "position"]),
        "actions": list(json!({"before": i, "text": s}), vec!["before", "text"]),
    }})
}

pub fn get_labels(llm: &impl Llm, model: &str, units: &[Unit], cast: &[CastMember], progress: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<Labels, StoryError> {
    let mut labels = Labels::default();
    let ids: Vec<Value> = cast.iter().map(|c| json!({"id": c.id, "name": c.name, "aliases": c.aliases, "first_person": c.first_person})).collect();
    let mut prev = String::new();
    let total = units.len().div_ceil(BATCH);
    for (b, chunk) in units.chunks(BATCH).enumerate() {
        progress(&format!("labelling the story, part {} of {total}", b + 1));
        let task = json!({
            "task": "label_units",
            "instruction": "Label numbered story units. Return ONLY numbers and labels, NEVER text to be spoken. JSON: \
{\"scenes\":[{\"start\":N,\"slugline\":\"INT. PLACE - NIGHT\"}],\"speakers\":[{\"n\":N,\"id\":\"CAST_ID\"}],\
\"tags\":[{\"before\":N,\"character\":\"Name\",\"pose\":\"sitting\",\"position\":\"left\"}],\
\"actions\":[{\"before\":N,\"text\":\"Budhi Amma turns towards the door.\"}]}. \
scenes: every place/time change (INT./EXT.), the first unit starts a scene. speakers: for every unit marked quote, the \
cast id of who says it. tags/actions are optional (empty lists are fine), short, and describe only what the narration already says.",
            "cast": ids,
            "previous_slugline": prev,
            "units": chunk.iter().map(|u| json!({"n": u.n, "kind": if u.kind == UnitKind::Quote { "quote" } else { "narr" }, "text": u.text.chars().take(400).collect::<String>()})).collect::<Vec<_>>(),
        });
        let found = ask(llm, model, LABEL_SYSTEM, &task, labels_schema(), &["scenes", "speakers", "tags", "actions"], cancel)?;
        for s in found.get("scenes").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(n), Some(slug)) = (as_n(s.get("start")), str_of(s.get("slugline"))) {
                labels.scenes.insert(n, norm(&slug).to_uppercase());
            }
        }
        match found.get("speakers") {
            Some(Value::Object(map)) => {
                for (k, v) in map {
                    if let Ok(n) = k.trim().parse::<usize>() {
                        labels.speakers.insert(n, text_of(v));
                    }
                }
            }
            Some(Value::Array(list)) => {
                for s in list {
                    let n = as_n(s.get("n").or_else(|| s.get("unit")).or_else(|| s.get("number")));
                    let id = str_of(s.get("id").or_else(|| s.get("speaker")).or_else(|| s.get("cast_id")));
                    if let (Some(n), Some(id)) = (n, id) {
                        labels.speakers.insert(n, id);
                    }
                }
            }
            _ => {}
        }
        for t in found.get("tags").and_then(Value::as_array).into_iter().flatten() {
            if let Some(n) = as_n(t.get("before")) {
                labels.tags.entry(n).or_default().push(Tag {
                    character: str_of(t.get("character")).unwrap_or_default(),
                    pose: str_of(t.get("pose")),
                    position: str_of(t.get("position")),
                });
            }
        }
        for a in found.get("actions").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(n), Some(text)) = (as_n(a.get("before")), str_of(a.get("text"))) {
                labels.actions.entry(n).or_default().push(norm(&text));
            }
        }
        if let Some((_, last)) = labels.scenes.iter().next_back() {
            prev = last.clone();
        }
    }
    Ok(labels)
}

// --- 4. assemble + verify ---------------------------------------------------------

fn slug(text: &str) -> String {
    // " - " / " – " between words becomes " — ".
    let t = norm(text).to_uppercase();
    let words: Vec<&str> = t.split(' ').collect();
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if i > 0 && i + 1 < words.len() && !w.is_empty() && w.chars().all(|c| c == '-' || c == '\u{2013}') {
            out.push('\u{2014}');
        } else {
            out.push_str(w);
        }
    }
    out
}

fn title_of(slug: &str) -> String {
    let t = slug.trim_start();
    let t = if t.starts_with("INT") || t.starts_with("EXT") {
        t[3..].trim_start_matches(['.', '/']).trim_start()
    } else {
        t
    };
    let first = t.split(" \u{2014} ").next().unwrap_or("").trim();
    if first.is_empty() { "SCENE".into() } else { first.to_string() }
}

/// BuzzAnimate's format: `## SCENE n — TITLE`, bold slugline, action lines with inline
/// `[Name: pose, position]` tags, bold cue lines (two trailing spaces) then the text.
pub fn assemble(units: &[Unit], cast: &[CastMember], labels: &mut Labels) -> String {
    let by_id = |id: &str| cast.iter().find(|c| c.id.to_uppercase() == id.to_uppercase());
    let by_name = |name: &str| cast.iter().find(|c| c.name.to_lowercase() == name.to_lowercase());
    let first_person = cast.iter().find(|c| c.first_person);
    let mut introduced: BTreeSet<String> = BTreeSet::new();
    let mut blocks: Vec<String> = Vec::new();
    let mut last_speaker: Option<String> = None;
    let mut scene_no = 0;

    // The producer reads no description file: a character without pictures is
    // drawn from the first action line that mentions them.
    let intro = |c: &CastMember, introduced: &mut BTreeSet<String>, blocks: &mut Vec<String>| {
        if !introduced.insert(c.id.clone()) {
            return;
        }
        let bits: Vec<String> = [&c.age, &c.look].iter().map(|b| norm(b)).filter(|b| !b.is_empty()).collect();
        let mut text = if bits.is_empty() { String::new() } else { format!("{} is {}", c.name, bits.join(", ")) };
        if !c.clothes.is_empty() {
            let lead = if text.is_empty() { format!("{} is ", c.name) } else { format!("{text}, ") };
            text = format!("{lead}wearing {}", norm(&c.clothes));
        }
        if !text.is_empty() {
            blocks.push(format!("{text}."));
        }
    };

    for u in units {
        let n = u.n;
        if n == 1 && !labels.scenes.contains_key(&1) {
            labels.scenes.insert(1, "INT. UNKNOWN PLACE \u{2014} NIGHT".into());
        }
        if let Some(raw) = labels.scenes.get(&n) {
            scene_no += 1;
            let s = slug(raw);
            blocks.push(format!("## SCENE {scene_no} \u{2014} {}", title_of(&s)));
            blocks.push(format!("**{s}**"));
        }
        for t in labels.tags.get(&n).cloned().unwrap_or_default() {
            let who = by_name(&norm(&t.character)).or_else(|| by_id(&t.character));
            if let Some(who) = who {
                intro(who, &mut introduced, &mut blocks);
                let tag = format!("[{}: {}, {}]", who.name, norm(t.pose.as_deref().unwrap_or("standing")), norm(t.position.as_deref().unwrap_or("centre")));
                match labels.actions.get_mut(&n).and_then(|a| a.first_mut()) {
                    Some(first) => *first = format!("{first} {tag}"),
                    None => blocks.push(format!("{} {}. {tag}", who.name, norm(t.pose.as_deref().unwrap_or("stands")))),
                }
            }
        }
        for a in labels.actions.get(&n).into_iter().flatten() {
            blocks.push(a.clone());
        }
        if u.kind == UnitKind::Quote {
            let label = labels.speakers.get(&n).cloned().unwrap_or_default();
            let sp = by_id(&label)
                .or_else(|| by_name(&label))
                .or_else(|| last_speaker.as_deref().and_then(by_id))
                .or(first_person)
                .unwrap_or(&cast[0]);
            last_speaker = Some(sp.id.clone());
            intro(sp, &mut introduced, &mut blocks);
            blocks.push(format!("**{}**  \n{}", sp.name.to_uppercase(), u.text));
        } else {
            blocks.push(format!("**NARRATOR (V.O.)**  \n{}", u.text));
        }
    }
    blocks.join("\n\n") + "\n"
}

fn is_cue(line: &str) -> bool {
    let Some(inner) = line.strip_prefix("**").and_then(|l| l.strip_suffix("**")) else { return false };
    let name = inner.strip_suffix(" (V.O.)").unwrap_or(inner);
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, ' ' | '.' | '\'' | '_' | '-'))
}

fn is_slugline_cue(line: &str) -> bool {
    ["**INT", "**EXT"].iter().any(|p| line.strip_prefix(p).is_some_and(|r| r.starts_with(['.', '/', ' '])))
}

/// The spoken lines of a screenplay: the text under each bold cue (`**NARRATOR (V.O.)**` / `**NAME**`).
pub fn spoken_lines(screenplay: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let flush = |block: &mut Vec<&str>, out: &mut Vec<String>| {
        if block.len() >= 2 && is_cue(block[0].trim()) && !is_slugline_cue(block[0].trim()) {
            out.push(norm(&block[1..].join(" ")));
        }
        block.clear();
    };
    for line in screenplay.lines() {
        if line.trim().is_empty() {
            flush(&mut block, &mut out);
        } else {
            block.push(line);
        }
    }
    flush(&mut block, &mut out);
    out
}

pub fn spoken_text(screenplay: &str) -> String {
    norm(&spoken_lines(screenplay).join(" "))
}

/// Errors, naming the first spoken line that differs, unless the spoken text is the story's.
pub fn verify(screenplay: &str, script: &str) -> Result<(), StoryError> {
    if spoken_text(screenplay) == source_spoken(script) {
        return Ok(());
    }
    let want: Vec<String> = split_units(script).into_iter().map(|u| u.text).collect();
    let got = spoken_lines(screenplay);
    for i in 0..want.len().max(got.len()) {
        let (w, g) = (want.get(i).map(String::as_str).unwrap_or("(nothing)"), got.get(i).map(String::as_str).unwrap_or("(nothing)"));
        if w != g {
            return fail(format!("the screenplay's spoken text differs from the story at spoken line {}: story {w:?} vs screenplay {g:?}", i + 1));
        }
    }
    fail("the screenplay's spoken text differs from the story")
}

// --- the whole conversion ---------------------------------------------------------

/// Story text -> verified screenplay text and its cast, and the updated registry.
/// Nothing is written.
pub fn convert_story(
    llm: &impl Llm,
    model: &str,
    script: &str,
    registry: Vec<CastMember>,
    progress: &mut dyn FnMut(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<(String, Vec<CastMember>, Vec<CastMember>), StoryError> {
    let units = split_units(script);
    if units.is_empty() {
        return fail("the story has no text");
    }
    progress(&format!("{} units; finding the cast", units.len()));
    let extracted = extract_cast(llm, model, script, &registry, cancel)?;
    let (cast, registry) = merge_registry(extracted, registry);
    progress(&format!("cast: {}", cast.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
    let mut labels = get_labels(llm, model, &units, &cast, progress, cancel)?;
    let screenplay = assemble(&units, &cast, &mut labels);
    verify(&screenplay, script)?;
    Ok((screenplay, cast, registry))
}

/// What a file conversion made.
#[derive(Debug, Clone)]
pub struct StoryOutput {
    pub screenplay: PathBuf,
    pub cast_path: PathBuf,
    pub registry: PathBuf,
    pub screenplay_text: String,
    pub cast: Vec<CastMember>,
}

/// **Story to screenplay**: reads `story`, writes `<out>/screenplay.md` and
/// `<out>/cast.json`, and keeps the series' registry at
/// `<assets_root>/Cast/<series>/cast_registry.json`.
pub fn story_to_screenplay(
    llm: &impl Llm,
    model: &str,
    story: &Path,
    series: &str,
    out: &Path,
    assets_root: &Path,
    progress: &mut dyn FnMut(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<StoryOutput, StoryError> {
    let bytes = std::fs::read(story).map_err(|e| StoryError::Failed(format!("reading {}: {e}", story.display())))?;
    let script = String::from_utf8_lossy(&bytes).into_owned();
    let registry_path = cast_registry_path(assets_root, series);
    let (screenplay, cast, registry) = convert_story(llm, model, &script, load_registry(&registry_path), progress, cancel)?;
    let write = |path: &Path, text: &str| -> Result<(), StoryError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoryError::Failed(format!("creating {}: {e}", dir.display())))?;
        }
        std::fs::write(path, text).map_err(|e| StoryError::Failed(format!("writing {}: {e}", path.display())))
    };
    let json_of = |v: &Vec<CastMember>| serde_json::to_string_pretty(v).unwrap_or_default();
    write(&registry_path, &json_of(&registry))?;
    let cast_path = out.join("cast.json");
    write(&cast_path, &json_of(&cast))?;
    let screenplay_path = out.join("screenplay.md");
    write(&screenplay_path, &screenplay)?;
    progress(&format!("wrote {}", screenplay_path.display()));
    Ok(StoryOutput { screenplay: screenplay_path, cast_path, registry: registry_path, screenplay_text: screenplay, cast })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers the two calls from canned JSON; no network.
    struct Stub {
        cast: Value,
        labels: Value,
    }

    impl Llm for Stub {
        fn chat_json<T: DeserializeOwned>(&self, _m: &str, _s: &str, user: &str, _i: &[&[u8]], _sc: Value) -> Result<T, LlmError> {
            let v = if user.contains("cast_extraction") { &self.cast } else { &self.labels };
            serde_json::from_value(v.clone()).map_err(|e| LlmError::Other(e.to_string()))
        }

        fn embed(&self, _m: &str, _t: &[&str]) -> Result<Vec<Vec<f32>>, LlmError> {
            Err(LlmError::Other("no".into()))
        }
    }

    fn member(id: &str, name: &str, aliases: &[&str]) -> CastMember {
        CastMember { id: id.into(), name: name.into(), role: "guard".into(), aliases: aliases.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn units_keep_exact_text_with_curly_quotes() {
        let story = "Main ghar gaya. Wahan \u{201C}Kaun hai?\u{201D} kisi ne poocha! Phir sab chup.";
        let units = split_units(story);
        let texts: Vec<&str> = units.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(texts, ["Main ghar gaya.", "Wahan", "Kaun hai?", "kisi ne poocha!", "Phir sab chup."]);
        assert_eq!(units[2].kind, UnitKind::Quote);
        assert_eq!(units[1].kind, UnitKind::Narr);
    }

    #[test]
    fn assemble_and_verify_a_first_person_story() {
        let story = "Raat ko main ghar laut raha tha. Ek budhi aurat ne kaha \"Beta, ruko.\" Main ruk gaya.";
        let stub = Stub {
            cast: json!({"data": {"cast": [
                {"id": "narrator", "name": "Main", "role": "narrator", "gender": "male", "age": "30", "look": "thin", "clothes": "", "aliases": [], "first_person": true},
                {"id": "BUDHI", "name": "Budhi Amma", "role": "old woman", "gender": "female", "age": "70", "look": "frail", "clothes": "white sari", "aliases": ["budhi aurat"], "first_person": false}]}}),
            labels: json!({"scenes": [{"start": 1, "slugline": "EXT. ROAD - NIGHT"}],
                "speakers": [{"n": 3, "id": "BUDHI"}], "tags": [{"before": 2, "character": "Budhi Amma", "pose": "standing", "position": "right"}], "actions": []}),
        };
        let dir = std::env::temp_dir().join(format!("buzz-story-test-{}", std::process::id()));
        let reg = dir.join("reg.json");
        let (play, cast, registry) = convert_story(&stub, "m", story, load_registry(&reg), &mut |_| {}, &|| false).unwrap();
        assert_eq!(cast.len(), 2);
        assert_eq!(registry.len(), 2);
        assert!(play.starts_with("## SCENE 1 \u{2014} ROAD\n\n**EXT. ROAD \u{2014} NIGHT**\n\n"));
        assert!(play.contains("**BUDHI AMMA**  \nBeta, ruko."));
        assert!(play.contains("[Budhi Amma: standing, right]"));
        assert!(verify(&play, story).is_ok());
        let tampered = play.replace("Main ruk gaya.", "Main bhaag gaya.");
        let err = verify(&tampered, story).unwrap_err().to_string();
        assert!(err.contains("spoken line 4") && err.contains("Main ruk gaya."), "{err}");
    }

    #[test]
    fn two_guards_sharing_a_word_stay_two_people() {
        let cast = vec![member("PAWAN", "Pawan", &["vanrakshak"]), member("RISHI", "Rishi", &["vanrakshak"])];
        let (out, registry) = merge_registry(cast.clone(), Vec::new());
        assert_eq!((out.len(), registry.len()), (2, 2));
        // The next episode meets the same two guards: still two, each its own.
        let (again, registry) = merge_registry(cast, registry);
        assert_eq!(registry.len(), 2);
        assert_eq!((again[0].id.as_str(), again[1].id.as_str()), ("PAWAN", "RISHI"));
    }
}
