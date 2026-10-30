//! **His own cast** (ANIMATE_COPILOT_PLAN B2): an episode is cast from the
//! creator's OWN characters, never generic ones (ComfyUI draws one only when
//! the run allows it and nothing of his fits).
//!
//! Three sources feed one catalogue:
//!
//! 1. the asset library's `Characters/` (and its style subfolders) and `Cast/`;
//! 2. his FLA files (`B:\youtubeProjects\Khayal3Baje\latestLibrary\charactersNew1.fla`
//!    and the stories folder's `morecharacters`, `womanSample`, `sampleNaming`),
//!    imported through the ordinary importer into `Characters/<fla stem>/`, one
//!    asset per character symbol, again only when the FLA's modification time
//!    changes. The FLA is only ever read;
//! 3. the story's own folder (`<story>/animate/*.fla`), imported the same way
//!    under `Characters/<story>/`.
//!
//! For each cast member (name, role, gender, age, look, aliases) the catalogue
//! gives a shortlist (name and alias match first, then the words of the written
//! description against each character's name, label and tags), and the model
//! picks one from the shortlist with a reason. A name that matches exactly needs
//! no model. The choice is kept per series in `<library>/Cast/<Series>/casting.json`
//! (member id -> asset path, `locked`) and reused next episode; a locked choice
//! is never changed.
//!
//! A SigLIP *text* tower is not part of the app (`buzz_ai::siglip` embeds
//! pictures only), so the description match here is word overlap against the
//! index's names, VLM labels and tags rather than a text embedding.
//!
//! Rig readiness ([`readiness`]): a chosen character can lip-sync only when it
//! has a mouth the mouth writers can drive, and gesture only with arms. The
//! report says which; the build uses what works.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use buzz_ai::asset_index::{self as ai_index, AssetIndex};
use buzz_ai::breakdown::CharacterBrief;
use buzz_ai::llm::Llm;
use buzz_ai::story::CastMember;
use buzz_doc::{Asset, AssetLibrary};
use crate::comfy_profile::CastSources;
use buzz_scene::{Layer, ObjectKind, Scene, SymbolId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// His FLA files with the characters, imported under `Characters/<stem>/`.
pub const FLA_SOURCES: [&str; 4] = [
    r"B:\youtubeProjects\Khayal3Baje\latestLibrary\charactersNew1.fla",
    r"B:\youtubeProjects\Khayal3Baje\stories\morecharacters.fla",
    r"B:\youtubeProjects\Khayal3Baje\stories\womanSample.fla",
    r"B:\youtubeProjects\Khayal3Baje\stories\sampleNaming.fla",
];

/// The most characters one FLA contributes.
const MAX_PER_FLA: usize = 400;
/// The shortlist the model chooses from, and the dialog's alternatives.
pub const SHORTLIST: usize = 5;
/// A score at which the name settles it and no model is asked.
const SETTLED: f64 = 100.0;

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

// ------------------------------------------------------------------ FLA import

/// What a sync of the FLA sources did, one line each.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub imported: Vec<String>,
    pub unchanged: Vec<String>,
    pub notes: Vec<String>,
}

/// `<root>/.my_cast_fla.json`: for each FLA the mtime it was imported at and
/// the assets it made (so a re-import only ever rewrites its own files).
fn stamps_path(root: &Path) -> PathBuf {
    root.join(".my_cast_fla.json")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stamp {
    mtime: u64,
    group: String,
    assets: Vec<String>,
}

fn load_stamps(root: &Path) -> BTreeMap<String, Stamp> {
    std::fs::read_to_string(stamps_path(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_stamps(root: &Path, stamps: &BTreeMap<String, Stamp>) {
    if let Ok(text) = serde_json::to_string_pretty(stamps) {
        let _ = std::fs::write(stamps_path(root), text);
    }
}

fn layers_of(stack: &buzz_scene::LayerStack) -> Vec<&Layer> {
    stack.iter().map(|l| l.as_ref()).collect()
}

/// A timeline laid out like a puppet: four or more painted layers, most holding
/// exactly one instance. Its instances are parts, not characters.
fn puppet_like(layers: &[&Layer]) -> bool {
    let painted: Vec<&&Layer> = layers.iter().filter(|l| !l.objects_at(0).is_empty()).collect();
    let single = painted.iter().filter(|l| l.objects_at(0).len() == 1 && l.objects_at(0)[0].instance().is_some()).count();
    painted.len() >= 4 && single * 10 >= painted.len() * 6
}

/// The character symbols of an imported FLA: the distinct symbols placed on the
/// stage (looking inside a single wrapper symbol, as every Animate asset has);
/// a puppet-shaped timeline is one character, the wrapper; with nothing placed,
/// the library symbols no other symbol uses.
pub fn character_symbols(scene: &Scene) -> Vec<(SymbolId, String)> {
    let named = |id: SymbolId| scene.library().get(id).map(|s| (id, s.name.clone()));
    let junk = |name: &str| name.trim().is_empty() || name.contains("&#060") || name.starts_with('<');
    let mut layers = layers_of(scene.stage_layers());
    let mut wrapper: Option<SymbolId> = None;
    let visible: Vec<_> = layers.iter().flat_map(|l| l.objects_at(0).iter()).filter(|o| o.visible).collect();
    if visible.len() == 1
        && let Some(i) = visible[0].instance()
        && let Some(sym) = scene.library().get(i.symbol)
    {
        wrapper = Some(sym.id);
        layers = layers_of(&sym.layers);
    }
    if puppet_like(&layers) {
        return wrapper.and_then(named).into_iter().collect();
    }
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    let mut out: Vec<(SymbolId, String)> = Vec::new();
    for layer in &layers {
        for o in layer.objects_at(0).iter().filter(|o| o.visible) {
            if let Some(i) = o.instance()
                && seen.insert(i.symbol.0)
                && let Some((id, name)) = named(i.symbol)
                && !junk(&name)
            {
                out.push((id, name));
            }
        }
    }
    if out.is_empty() {
        let used: BTreeSet<u64> = scene
            .library()
            .iter()
            .flat_map(|s| s.layers.iter().flat_map(|l| l.all_objects().filter_map(|o| o.instance().map(|i| i.symbol.0)).collect::<Vec<_>>()).collect::<Vec<_>>())
            .collect();
        out = scene.library().iter().filter(|s| !used.contains(&s.id.0) && !junk(&s.name)).map(|s| (s.id, s.name.clone())).collect();
    }
    out.truncate(MAX_PER_FLA);
    out
}

/// Import one FLA into `Characters/<group>/`, one asset per character symbol,
/// unless its mtime is what was imported last time. Never modifies the FLA, and
/// never overwrites an asset of his that this import did not make.
pub fn sync_fla(root: &Path, fla: &Path, group: &str, say: &mut dyn FnMut(&str)) -> SyncReport {
    let mut report = SyncReport::default();
    let key = fla.display().to_string();
    let label = fla.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| key.clone());
    if !fla.is_file() {
        report.notes.push(format!("{label}: {} not found (skipped)", fla.display()));
        return report;
    }
    let mtime = crate::asset_index::mtime_of(fla);
    let mut stamps = load_stamps(root);
    if stamps.get(&key).is_some_and(|s| s.mtime == mtime && s.group == group) {
        report.unchanged.push(label);
        return report;
    }
    say(&format!("importing the characters of {} into Characters/{group}\u{2026}", fla.display()));
    let imported = match crate::import::read(fla) {
        Ok(i) => i,
        Err(e) => {
            report.notes.push(format!("{label}: could not read it: {e}"));
            return report;
        }
    };
    let folder = format!("Characters/{group}");
    let mut library = AssetLibrary::at(root);
    let ours: BTreeSet<String> = stamps.get(&key).map(|s| s.assets.iter().cloned().collect()).unwrap_or_default();
    let mut symbols = character_symbols(&imported.scene);
    let whole = symbols.is_empty();
    let mut made: Vec<String> = Vec::new();
    let mut save = |library: &mut AssetLibrary, name: &str, scene: &Scene, report: &mut SyncReport| {
        let clean: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
        let clean = clean.trim().to_string();
        let clash = library.assets().iter().any(|a| a.folder == folder && a.name.eq_ignore_ascii_case(&clean));
        if clash && !ours.iter().any(|o| o.eq_ignore_ascii_case(&clean)) {
            report.notes.push(format!("{label}: Characters/{group}/{clean} is already there and not from this import (kept)"));
            return;
        }
        match library.save(&clean, &folder, scene) {
            Ok(a) => made.push(a.name),
            Err(e) => report.notes.push(format!("{label}: {clean}: {e}")),
        }
    };
    if whole {
        save(&mut library, &label, &imported.scene, &mut report);
    } else {
        for (id, name) in symbols.drain(..) {
            if let Some(one) = crate::asset_index::symbol_scene(&imported.scene, id, &name) {
                save(&mut library, &name, &one, &mut report);
            }
        }
    }
    report.imported.push(format!("{label}: {} character(s)", made.len()));
    stamps.insert(key, Stamp { mtime, group: group.to_string(), assets: made });
    save_stamps(root, &stamps);
    report
}

/// The series' `cast_sources` (`None`: no profile or no such key).
pub fn sources_of(series: &str) -> Option<CastSources> {
    crate::comfy_profile::profile_for(series).and_then(|p| p.cast_sources)
}

/// Whether `pattern` (with `*` wildcards, case-insensitive) matches `name`.
fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_lowercase(), name.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !rest.starts_with(part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(at) = rest.find(part) {
            rest = &rest[at + part.len()..];
        } else {
            return false;
        }
    }
    true
}

/// The test rigs (`Ana 3`, `Bo 2`) are never characters, for any series.
const TEST_RIGS: [&str; 2] = ["Ana *", "Bo *"];

/// The read-only line the Look step shows: where the series' characters come from.
pub fn sources_line(series: &str, names: &[String]) -> String {
    let Some(s) = sources_of(series) else { return "Characters from: the whole library".into() };
    let mut from = s.library.join(", ");
    if !s.fla.is_empty() {
        from.push_str(&format!(" + {} FLA file(s)", s.fla.len()));
    }
    let shown: Vec<&str> = names.iter().take(6).map(String::as_str).collect();
    let more = if names.len() > shown.len() { "..." } else { "" };
    format!("Characters from: {from} ({}{more})", shown.join(", "))
}

/// The FLA files of a story's own folder: `<dir>/animate/*.fla`.
pub fn story_flas(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir.join("animate"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fla"))).collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// Sync his FLA files, and the story's own when `story_dir` is given.
pub fn sync_sources(root: &Path, series: &str, story_dir: Option<&Path>, say: &mut dyn FnMut(&str)) -> SyncReport {
    let sources = sources_of(series);
    let mut all = SyncReport::default();
    let mut merge = |r: SyncReport| {
        all.imported.extend(r.imported);
        all.unchanged.extend(r.unchanged);
        all.notes.extend(r.notes);
    };
    let flas: Vec<String> = match &sources {
        Some(s) => s.fla.clone(),
        None => FLA_SOURCES.iter().map(|p| p.to_string()).collect(),
    };
    for path in flas {
        let path = PathBuf::from(path);
        let group = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        merge(sync_fla(root, &path, &group, say));
    }
    if let Some(dir) = story_dir.filter(|_| sources.as_ref().is_none_or(|s| s.story_animate_folder)) {
        let group = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if !group.is_empty() {
            for fla in story_flas(dir) {
                merge(sync_fla(root, &fla, &group, say));
            }
        }
    }
    all
}

// ------------------------------------------------------------------ catalogue

/// One character of his, as the shortlist sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The asset's name (what the casting file and the build call it).
    pub name: String,
    /// Library folder, `/`-separated.
    pub folder: String,
    /// Path relative to the library root, `/`-separated.
    pub path: String,
    /// The index's thumbnail, relative to the library root (empty: none yet).
    pub thumb: String,
    /// The index's label and tags, space-separated.
    pub text: String,
}

fn split_camel(word: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    for c in word.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev = None;
            continue;
        }
        if let Some(p) = prev
            && ((p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic()))
            && !cur.is_empty()
        {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
        prev = Some(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

const STOP: [&str; 12] = ["the", "and", "with", "for", "symbol", "copy", "new", "buzz", "layer", "layers", "rig", "a"];

fn tokens(s: &str) -> Vec<String> {
    split_camel(s).into_iter().filter(|t| t.len() >= 2 && !STOP.contains(&t.as_str())).collect()
}

/// The words a written description may also be said with.
fn synonyms(word: &str) -> Vec<&'static str> {
    match word {
        "female" | "woman" | "lady" | "wife" | "aurat" | "mother" | "bahu" | "sasu" => vec!["woman", "girl", "lady", "wife", "aurat", "female", "mother"],
        "male" | "man" | "husband" | "guy" | "aadmi" | "father" => vec!["man", "boy", "male", "guy", "husband", "father"],
        "boy" | "child" | "kid" | "son" | "young" => vec!["boy", "child", "kid", "son", "baby"],
        "girl" | "daughter" => vec!["girl", "daughter", "child", "kid"],
        "baby" | "infant" => vec!["baby", "infant", "child"],
        "old" | "elderly" | "aged" | "budha" | "budhi" | "grandmother" | "grandfather" => vec!["old", "elder", "elderly", "budha", "budhi", "grandma", "grandpa"],
        "ghost" | "chudail" | "witch" | "spirit" | "bhoot" => vec!["ghost", "chudail", "witch", "spirit", "demon", "bhoot"],
        "demon" | "monster" | "rakshas" => vec!["demon", "monster", "rakshas"],
        "sadhu" | "baba" | "monk" | "saint" | "tantrik" => vec!["sadhu", "baba", "monk", "saint", "tantrik"],
        "skeleton" | "skull" | "kankaal" => vec!["skeleton", "skull", "kankaal"],
        "scooter" | "bike" | "motorbike" => vec!["scooter", "bike", "motorbike"],
        _ => Vec::new(),
    }
}

fn female_word(t: &str) -> bool {
    ["woman", "girl", "lady", "wife", "aurat", "female", "mother", "chudail", "cooking"].contains(&t)
}
fn male_word(t: &str) -> bool {
    ["man", "boy", "male", "guy", "husband", "father", "sadhu", "baba"].contains(&t)
}

/// Strip a Cast rig's view suffix: the character, not its Back/3-4 picture.
fn base_of(name: &str) -> &str {
    for suffix in [" 3-4", " Back", " Side"] {
        if let Some(b) = name.strip_suffix(suffix) {
            return b;
        }
    }
    name
}

/// A name that is a part of a character or a numbered copy, not a character.
fn not_a_character(name: &str) -> bool {
    let lower = name.to_lowercase();
    let toks = tokens(name);
    let part = toks.iter().any(|t| ["mouth", "eye", "eyes", "eyeopening", "palm", "hand", "body", "shape"].contains(&t.as_str()));
    let numbered = lower.rsplit_once(' ').is_some_and(|(b, n)| !b.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    part || numbered || crate::cast_build::pose_of_rig(name).is_some()
}

/// Whether `a` is the library asset `find_asset` would pick for its name.
fn pick_order(a: &Asset) -> (bool, usize, String) {
    let in_cast = a.folder.eq_ignore_ascii_case("cast") || a.folder.to_lowercase().starts_with("cast/");
    (!in_cast, a.folder.len(), a.folder.clone())
}

/// **The catalogue**: his characters in the library (`Cast/`, `Characters/` and
/// its style and imported-FLA subfolders), named as the build finds them. A name
/// that two folders share is listed once, as the build would pick it; views
/// (`Raju Back`) and pose variants belong to their character and are not
/// listed.
pub fn catalogue(root: &Path) -> Vec<Entry> {
    catalogue_for(root, "")
}

/// [`catalogue`] of one series: only the folders its `cast_sources` name (no
/// sources: `Cast/` and `Characters/`), minus the excluded names and the test rigs.
pub fn catalogue_for(root: &Path, series: &str) -> Vec<Entry> {
    catalogue_with(root, sources_of(series).as_ref())
}

/// Whether an asset in library folder `folder` called `name` is one of the series' characters.
fn in_sources(sources: Option<&CastSources>, folder: &str, name: &str) -> bool {
    let f = folder.to_lowercase();
    let folders: Vec<String> = match sources {
        Some(s) if !s.library.is_empty() => s.library.iter().map(|d| d.trim_matches('/').to_lowercase()).collect(),
        _ => vec!["cast".into(), "characters".into()],
    };
    let mut exclude: Vec<&str> = TEST_RIGS.to_vec();
    if let Some(s) = sources {
        exclude.extend(s.exclude.iter().map(String::as_str));
    }
    folders.iter().any(|d| f == *d || f.starts_with(&format!("{d}/"))) && !exclude.iter().any(|p| glob_match(p, name))
}

fn catalogue_with(root: &Path, sources: Option<&CastSources>) -> Vec<Entry> {
    let library = AssetLibrary::at(root);
    let index = AssetIndex::load(root);
    let wanted = |a: &&Asset| in_sources(sources, &a.folder, &a.name) && !not_a_character(&a.name) && base_of(&a.name) == a.name;
    let mut assets: Vec<&Asset> = library.assets().iter().filter(wanted).collect();
    assets.sort_by_key(|a| pick_order(a));
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<Entry> = Vec::new();
    for a in assets {
        if !seen.insert(a.name.to_lowercase()) {
            continue;
        }
        let rel = ai_index::relative(root, &a.path);
        let record = index.asset_record(&rel);
        let mut text: Vec<String> = Vec::new();
        if let Some(r) = record {
            text.extend(r.label.clone());
            text.extend(r.tags.iter().cloned());
            text.push(r.kind.as_str().to_string());
        }
        text.extend(a.tags.iter().cloned());
        out.push(Entry {
            name: a.name.clone(),
            folder: a.folder.clone(),
            path: rel,
            thumb: record.map(|r| r.thumb.clone()).unwrap_or_default(),
            text: text.join(" "),
        });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

// ------------------------------------------------------------------ members and shortlist

/// A person of the story to cast.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub role: String,
    pub gender: String,
    pub age: String,
    pub look: String,
    pub aliases: Vec<String>,
}

impl Member {
    /// From the story converter's `cast.json` entry.
    pub fn from_cast(c: &CastMember) -> Self {
        let mut look = c.look.clone();
        if !c.clothes.trim().is_empty() {
            look = format!("{look} {}", c.clothes);
        }
        Self { id: c.id.clone(), name: c.name.clone(), role: c.role.clone(), gender: c.gender.clone(), age: c.age.clone(), look, aliases: c.aliases.clone() }
    }

    /// From the breakdown's brief, for a screenplay with no `cast.json`.
    pub fn from_brief(b: &CharacterBrief) -> Self {
        Self { id: String::new(), name: b.name.clone(), look: b.description.clone(), aliases: b.aliases.clone(), ..Default::default() }
    }

    /// The key a choice is kept under.
    pub fn key(&self) -> String {
        let k = if self.id.trim().is_empty() { norm(&self.name) } else { norm(&self.id) };
        k.to_uppercase()
    }

    fn name_keys(&self) -> Vec<String> {
        std::iter::once(&self.name).chain(std::iter::once(&self.id)).chain(self.aliases.iter()).map(|n| norm(n)).filter(|k| !k.is_empty()).collect()
    }
}

/// The story's `cast.json` entries, if the run came with one.
pub fn read_cast_json(path: &Path) -> Vec<CastMember> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(CastMember::from_value).collect()))
        .unwrap_or_default()
}

/// The member for a breakdown brief: the story's own cast entry when one names
/// the same person, else the brief.
pub fn member_for(brief: &CharacterBrief, story_cast: &[CastMember]) -> Member {
    let keys: BTreeSet<String> = std::iter::once(&brief.name).chain(brief.aliases.iter()).map(|n| norm(n)).filter(|k| !k.is_empty()).collect();
    let hit = story_cast.iter().find(|c| std::iter::once(&c.name).chain(std::iter::once(&c.id)).chain(c.aliases.iter()).map(|n| norm(n)).any(|k| !k.is_empty() && keys.contains(&k)));
    match hit {
        Some(c) => {
            let mut m = Member::from_cast(c);
            for a in &brief.aliases {
                if !m.aliases.contains(a) {
                    m.aliases.push(a.clone());
                }
            }
            if m.look.trim().is_empty() {
                m.look = brief.description.clone();
            }
            m
        }
        None => Member::from_brief(brief),
    }
}

/// A character with the score the shortlist gave it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub entry: Entry,
    pub score: f64,
}

fn score(entry: &Entry, m: &Member) -> f64 {
    let ek = norm(base_of(&entry.name));
    let etoks = tokens(&entry.name);
    let mut s = 0.0f64;
    let keys = m.name_keys();
    for key in &keys {
        if *key == ek {
            s = s.max(SETTLED);
        } else if key.len() >= 4 && ek.len() >= 4 && (key.contains(&ek) || ek.contains(key)) {
            s = s.max(70.0);
        }
    }
    // Words of the member's names that are words of the character's.
    let mut name_words: BTreeSet<String> = BTreeSet::new();
    for n in std::iter::once(&m.name).chain(m.aliases.iter()) {
        name_words.extend(tokens(n).into_iter().filter(|t| t.len() >= 3));
    }
    s += 25.0 * etoks.iter().filter(|t| name_words.contains(*t)).count().min(2) as f64;
    // The written description against the character's name, label and tags.
    let mut have: BTreeSet<String> = etoks.iter().cloned().collect();
    have.extend(tokens(&entry.text));
    let mut said: BTreeSet<String> = BTreeSet::new();
    for t in tokens(&format!("{} {} {} {}", m.role, m.gender, m.age, m.look)) {
        said.insert(t.clone());
        said.extend(synonyms(&t).into_iter().map(String::from));
    }
    s += 4.0 * said.iter().filter(|t| have.contains(*t)).count().min(6) as f64;
    // A man for a woman (or the reverse) is a worse fit than no word at all.
    let g = format!("{} {}", m.gender, m.role).to_lowercase();
    let wants_female = g.contains("female") || g.contains("woman") || g.contains("girl");
    let wants_male = !wants_female && (g.contains("male") || g.contains("man") || g.contains("boy"));
    let has_f = have.iter().any(|t| female_word(t));
    let has_m = have.iter().any(|t| male_word(t));
    if (wants_female && has_m && !has_f) || (wants_male && has_f && !has_m) {
        s -= 8.0;
    }
    s
}

/// **The shortlist**: the best `n` characters for a member, best first (name and
/// alias matches, then the description), only those with a positive score.
pub fn shortlist(catalogue: &[Entry], m: &Member, n: usize) -> Vec<Candidate> {
    let mut all: Vec<Candidate> = catalogue.iter().map(|e| Candidate { entry: e.clone(), score: score(e, m) }).filter(|c| c.score > 0.0).collect();
    all.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.entry.name.to_lowercase().cmp(&b.entry.name.to_lowercase())));
    all.truncate(n);
    all
}

// ------------------------------------------------------------------ casting.json

/// One member's kept choice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// The asset, relative to the library root.
    pub asset: String,
    /// Its name (what the build calls it).
    pub name: String,
    /// Never changed by a later episode.
    pub locked: bool,
    #[serde(default)]
    pub reason: String,
    /// The member's name when it was chosen, for reading the file.
    #[serde(default)]
    pub member: String,
}

/// `<library>/Cast/<Series>/casting.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CastingFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub series: String,
    /// Member key ([`Member::key`]) -> choice.
    #[serde(default)]
    pub choices: BTreeMap<String, Choice>,
}

pub fn casting_path(root: &Path, series: &str) -> PathBuf {
    buzz_ai::story::cast_registry_path(root, series).with_file_name("casting.json")
}

pub fn load_casting(root: &Path, series: &str) -> CastingFile {
    let mut f: CastingFile = std::fs::read_to_string(casting_path(root, series)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    f.version = 1;
    f.series = series.to_string();
    f
}

pub fn save_casting(root: &Path, file: &CastingFile) -> std::io::Result<()> {
    let path = casting_path(root, &file.series);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(file).unwrap_or_default())
}

// ------------------------------------------------------------------ casting

/// How a member's character was settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// A locked choice from an earlier episode or the Casting dialog.
    Locked,
    /// An earlier choice, reused.
    Kept,
    /// The name matches exactly.
    Name,
    /// The model picked it from the shortlist.
    Model,
    /// No model answered: the shortlist's best.
    Best,
}

/// What casting a member came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Chosen { entry: Entry, reason: String, via: Via },
    /// "no character of yours fits": the member stays off stage.
    NoFit(String),
    /// Nothing of his fits and the run allows a drawing: the old path draws one.
    Generate,
}

/// The casting of one series over one catalogue.
pub struct MyCast {
    pub root: PathBuf,
    pub catalogue: Vec<Entry>,
    pub file: CastingFile,
    pub allow_generated: bool,
    /// The local vision model that describes a character with no description
    /// yet (`None`: nobody is described).
    pub vision: Option<buzz_ai::story::Endpoints>,
    looks_done: bool,
}

impl MyCast {
    /// The library's catalogue and the series' casting file.
    pub fn open(root: &Path, series: &str, allow_generated: bool) -> Self {
        Self { root: root.to_path_buf(), catalogue: catalogue_for(root, series), file: load_casting(root, series), allow_generated, vision: None, looks_done: false }
    }

    pub fn from_parts(root: &Path, catalogue: Vec<Entry>, file: CastingFile, allow_generated: bool) -> Self {
        Self { root: root.to_path_buf(), catalogue, file, allow_generated, vision: None, looks_done: false }
    }

    pub fn save(&self) -> std::io::Result<()> {
        save_casting(&self.root, &self.file)
    }

    fn entry_at(&self, path: &str) -> Option<&Entry> {
        self.catalogue.iter().find(|e| e.path == path)
    }

    /// **Cast one member.** A locked choice is reused as it is; an earlier choice
    /// whose character is still there is reused; otherwise the shortlist is made
    /// and the model picks from it (an exact name needs no model). The choice is
    /// recorded; call [`MyCast::save`] after the run.
    pub fn cast_member(&mut self, llm: &impl Llm, m: &Member, cancel: &dyn Fn() -> bool) -> Outcome {
        let key = m.key();
        if let Some(c) = self.file.choices.get(&key).cloned() {
            match self.entry_at(&c.asset).cloned() {
                Some(entry) => {
                    let via = if c.locked { Via::Locked } else { Via::Kept };
                    let reason = if c.reason.is_empty() { "chosen before".to_string() } else { c.reason };
                    return Outcome::Chosen { entry, reason, via };
                }
                None if c.locked => {
                    return Outcome::NoFit(format!("the locked character {} is no longer in your library", c.asset));
                }
                None => {}
            }
        }
        let list = shortlist(&self.catalogue, m, SHORTLIST);
        let gone = |allow: bool, why: String| if allow { Outcome::Generate } else { Outcome::NoFit(why) };
        let (entry, reason, via) = match list.first() {
            Some(top) if top.score >= SETTLED => (top.entry.clone(), format!("the name {} matches", top.entry.name), Via::Name),
            top => {
                if self.catalogue.is_empty() {
                    return gone(self.allow_generated, "no character of yours fits".into());
                }
                self.ensure_looks(cancel);
                match pick_from_catalogue(llm, m, &self.catalogue, &self.looks_text(), cancel) {
                    Pick::Chose(i, why) => (self.catalogue[i].clone(), why, Via::Model),
                    Pick::None(why) => return gone(self.allow_generated, format!("no character of yours fits ({why})")),
                    Pick::Failed => match top {
                        Some(t) => (t.entry.clone(), "best match by name and description (no model answered)".into(), Via::Best),
                        None => return gone(self.allow_generated, "no character of yours fits".into()),
                    },
                }
            }
        };
        self.file.choices.insert(
            key,
            Choice { asset: entry.path.clone(), name: entry.name.clone(), locked: false, reason: reason.clone(), member: m.name.clone() },
        );
        Outcome::Chosen { entry, reason, via }
    }
}

enum Pick {
    Chose(usize, String),
    None(String),
    Failed,
}

/// Characters per model call; a bigger catalogue is asked in chunks and the
/// chunks' winners are asked again.
const CHUNK: usize = 120;

/// The task for one member over a slice of the catalogue: every candidate's id
/// (its path), name, folder and description, and the member.
pub fn catalogue_prompt(m: &Member, entries: &[(&Entry, String)]) -> Value {
    json!({
        "task": "casting",
        "instruction": "Pick the ONE character from `candidates` (the creator's own drawings; names are file names and may \
say little: judge by `description`, folder and the name's words) that best plays this person of the story. Choose by who they \
are (gender, age, role, look), not by the order. Answer {\"pick\": \"<exact candidate id>\", \"reason\": \"<one short sentence>\"}. \
Answer {\"pick\": \"none\", \"reason\": ...} only when no candidate could play them at all.",
        "person": {"name": m.name, "role": m.role, "gender": m.gender, "age": m.age, "look": m.look, "aliases": m.aliases},
        "candidates": entries.iter().map(|(e, d)| json!({"id": e.path, "name": e.name, "folder": e.folder, "description": d})).collect::<Vec<_>>(),
    })
}

fn ask_pick(llm: &impl Llm, m: &Member, entries: &[(&Entry, String)], cancel: &dyn Fn() -> bool) -> Pick {
    let task = catalogue_prompt(m, entries);
    let schema = json!({"type": "object", "additionalProperties": false, "required": ["pick", "reason"],
        "properties": {"pick": {"type": "string"}, "reason": {"type": "string"}}});
    let user = serde_json::to_string_pretty(&task).unwrap_or_default();
    let system = "You are the casting director of an animated web series. You only ever cast from the creator's own characters.";
    let answer: Value = match llm.chat_json_cancellable::<Value>("", system, &user, &[], schema, cancel) {
        Ok(v) => v,
        Err(_) => return Pick::Failed,
    };
    let name = answer["pick"].as_str().unwrap_or("").trim().to_string();
    let reason = answer["reason"].as_str().unwrap_or("").trim().to_string();
    if name.eq_ignore_ascii_case("none") {
        return Pick::None(reason);
    }
    match entries.iter().position(|(e, _)| e.path == name || e.name.eq_ignore_ascii_case(&name)) {
        Some(i) => Pick::Chose(i, if reason.is_empty() { "picked by the model".into() } else { reason }),
        None => Pick::Failed,
    }
}

/// **The model casts from the whole catalogue**: an index into `catalogue`
/// with a reason, or none / failed.
fn pick_from_catalogue(llm: &impl Llm, m: &Member, catalogue: &[Entry], looks: &BTreeMap<String, String>, cancel: &dyn Fn() -> bool) -> Pick {
    let desc = |e: &Entry| looks.get(&e.path).cloned().unwrap_or_else(|| e.text.clone());
    let all: Vec<(usize, &Entry, String)> = catalogue.iter().enumerate().map(|(i, e)| (i, e, desc(e))).collect();
    let mut winners: Vec<(usize, String)> = Vec::new();
    let mut none_why = String::new();
    let mut answered = false;
    for chunk in all.chunks(CHUNK) {
        let entries: Vec<(&Entry, String)> = chunk.iter().map(|(_, e, d)| (*e, d.clone())).collect();
        match ask_pick(llm, m, &entries, cancel) {
            Pick::Chose(i, why) => {
                answered = true;
                winners.push((chunk[i].0, why));
            }
            Pick::None(why) => {
                answered = true;
                none_why = why;
            }
            Pick::Failed => {}
        }
    }
    match winners.len() {
        0 if answered => Pick::None(none_why),
        0 => Pick::Failed,
        1 => Pick::Chose(winners[0].0, winners[0].1.clone()),
        _ => {
            let entries: Vec<(&Entry, String)> = winners.iter().map(|(i, _)| (&catalogue[*i], desc(&catalogue[*i]))).collect();
            match ask_pick(llm, m, &entries, cancel) {
                Pick::Chose(k, why) => Pick::Chose(winners[k].0, why),
                _ => Pick::Chose(winners[0].0, winners[0].1.clone()),
            }
        }
    }
}

impl MyCast {
    /// The index's description of each character that has one (label, then
    /// the catalogue text), keyed by asset path.
    fn looks_text(&self) -> BTreeMap<String, String> {
        let index = AssetIndex::load(&self.root);
        self.catalogue
            .iter()
            .filter_map(|e| {
                let label = index.asset_record(&e.path).and_then(|r| r.label.clone()).filter(|l| !l.trim().is_empty())?;
                let label = label.replace('_', " ");
                let d = if e.text.contains(&label) { e.text.clone() } else { format!("{label} {}", e.text) };
                Some((e.path.clone(), d))
            })
            .collect()
    }

    /// Once per run: every character whose index record has no description
    /// gets its thumbnail described by the local vision model (LM Studio's
    /// Gemma; the backend sends `reasoning_effort: "none"`) and the sentence
    /// is kept as the record's label in the asset index.
    fn ensure_looks(&mut self, cancel: &dyn Fn() -> bool) {
        if self.looks_done {
            return;
        }
        self.looks_done = true;
        let Some(vision) = &self.vision else { return };
        let mut index = AssetIndex::load(&self.root);
        let mut changed = false;
        for e in &self.catalogue {
            if cancel() {
                break;
            }
            if e.thumb.is_empty() {
                continue;
            }
            let Some(pos) = index.records.iter().position(|r| r.path == e.path && r.is_asset()) else { continue };
            if index.records[pos].label.as_deref().is_some_and(|l| !l.trim().is_empty()) {
                continue;
            }
            let Ok(png) = std::fs::read(self.root.join(&e.thumb)) else { continue };
            let schema = json!({"type": "object", "additionalProperties": false, "required": ["description"],
                "properties": {"description": {"type": "string"}}});
            let user = "Describe this cartoon character in one short sentence for a casting director: gender, apparent age, build, \
clothes and colours, hair or headgear, anything that marks who they are. Answer {\"description\": \"...\"}.";
            let got: Result<Value, _> =
                vision.chat_json_cancellable::<Value>("", "You describe characters of an animated series.", user, &[png.as_slice()], schema, cancel);
            if let Ok(v) = got
                && let Some(d) = v["description"].as_str().map(str::trim).filter(|d| !d.is_empty())
            {
                index.records[pos].label = Some(d.chars().take(300).collect());
                changed = true;
            }
        }
        if changed {
            let _ = index.save(&self.root);
        }
    }
}

// ------------------------------------------------------------------ rig readiness

/// What a chosen character can do for the build.
#[derive(Debug, Clone, PartialEq)]
pub struct Readiness {
    pub name: String,
    pub mouth: bool,
    pub arms: bool,
    /// `bones`, `layers` or `none`.
    pub rig: &'static str,
    /// Mouth-named symbols exist but nothing drives them.
    pub loose_mouth: bool,
}

impl Readiness {
    /// The report line: `Woman3Symbol: mouth ok, arms ok`, or what to run.
    pub fn line(&self) -> String {
        if self.rig == "none" {
            let loose = if self.loose_mouth { " (mouth symbols found but not rigged)" } else { "" };
            return format!("{}: no mouth: run AI Layer Rig{loose}", self.name);
        }
        let mouth = if self.mouth { "mouth ok".to_string() } else { "no mouth: run AI Layer Rig".to_string() };
        let arms = if self.arms { "arms ok" } else { "no arms (no gestures)" };
        format!("{}: {mouth}, {arms}", self.name)
    }
}

fn arm_word(name: &str) -> bool {
    let n = name.to_lowercase();
    ["arm", "shoulder", "hand", "elbow"].iter().any(|w| n.contains(w))
}

/// **Is this character ready to perform?** It is placed in a scratch scene the way
/// the build places it and asked the questions the mouth writers ask
/// (`find_rig_mouth`, `find_layer_rig_mouth`); arms come from the rig's bones or
/// slots. Never fails: an asset that will not load is reported as having nothing.
pub fn readiness(library: &AssetLibrary, asset: &Asset) -> Readiness {
    let none = |loose_mouth| Readiness { name: asset.name.clone(), mouth: false, arms: false, rig: "none", loose_mouth };
    let Ok(loaded) = library.load(asset) else { return none(false) };
    let loose = loaded.library().iter().any(|s| s.name.to_lowercase().contains("mouth"));
    let mut scene = Scene::default();
    if library.place(asset, &mut scene).is_err() {
        return none(loose);
    }
    for layer in scene.layers().iter() {
        for object in layer.objects_at(0) {
            if let ObjectKind::Armature(rig) = &object.kind {
                let arms = rig.armature.bones.iter().any(|b| arm_word(&b.name));
                let mouth = buzz_act::textsync::find_rig_mouth(&scene, object.id).is_some();
                return Readiness { name: asset.name.clone(), mouth, arms, rig: "bones", loose_mouth: loose };
            }
        }
        if let Some(view) = buzz_act::layer_rig::rig_of_layer(&scene, layer.id) {
            let arms = view.parts.iter().any(|(slot, _, _)| view.pattern.slots.get(*slot).is_some_and(|s| arm_word(&s.name)));
            let mouth = buzz_act::textsync::find_layer_rig_mouth(&scene, &view).is_some();
            return Readiness { name: asset.name.clone(), mouth, arms, rig: "layers", loose_mouth: loose };
        }
    }
    none(loose)
}

/// [`readiness`] for the asset named `name` (the build's lookup: Cast first).
pub fn readiness_of(library: &AssetLibrary, name: &str) -> Option<Readiness> {
    let wanted = name.to_lowercase();
    let mut matches: Vec<&Asset> = library.assets().iter().filter(|a| a.name.to_lowercase() == wanted).collect();
    matches.sort_by_key(|a| pick_order(a));
    matches.first().map(|a| readiness(library, a))
}

// ------------------------------------------------------------------ the Casting dialog's data

/// The people to cast for the dialog: the episode folder's `cast.json`, else the
/// series' remembered cast (`cast_registry.json`).
pub fn members_for_dialog(out: &Path, root: &Path, series: &str) -> Vec<Member> {
    let mut cast = read_cast_json(&out.join("cast.json"));
    if cast.is_empty() {
        cast = read_cast_json(&buzz_ai::story::cast_registry_path(root, series));
    }
    cast.iter().map(Member::from_cast).collect()
}

/// One person with the character kept (or suggested) for them and the others
/// worth a look.
#[derive(Debug, Clone)]
pub struct ViewRow {
    pub member: Member,
    pub chosen: Option<Candidate>,
    pub reason: String,
    pub locked: bool,
    /// Nothing was kept: `chosen` is the shortlist's best.
    pub suggested: bool,
    pub alternatives: Vec<Candidate>,
}

/// Rows for the Casting dialog (no model: the kept choice, else the shortlist's
/// best as a suggestion).
pub fn view(catalogue: &[Entry], file: &CastingFile, members: &[Member]) -> Vec<ViewRow> {
    members
        .iter()
        .map(|m| {
            let list = shortlist(catalogue, m, SHORTLIST + 1);
            let kept = file.choices.get(&m.key());
            let kept_entry = kept.and_then(|c| catalogue.iter().find(|e| e.path == c.asset));
            let (chosen, reason, locked, suggested) = match (kept, kept_entry) {
                (Some(c), Some(e)) => {
                    let score = list.iter().find(|x| x.entry.path == e.path).map(|x| x.score).unwrap_or(0.0);
                    (Some(Candidate { entry: e.clone(), score }), c.reason.clone(), c.locked, false)
                }
                _ => match list.first() {
                    Some(top) => (Some(top.clone()), "suggested by name and description".to_string(), false, true),
                    None => (None, "no character of yours fits".to_string(), false, false),
                },
            };
            let alternatives = list
                .into_iter()
                .filter(|c| chosen.as_ref().is_none_or(|x| x.entry.path != c.entry.path))
                .take(SHORTLIST)
                .collect();
            ViewRow { member: m.clone(), chosen, reason, locked, suggested, alternatives }
        })
        .collect()
}

/// Write the dialog's picks into the series' `casting.json`: every member with a
/// character is kept (a suggestion becomes a choice only because he saved it),
/// one without is forgotten.
pub fn save_view(root: &Path, series: &str, picks: &[(Member, Option<(Entry, String, bool)>)]) -> std::io::Result<()> {
    let mut file = load_casting(root, series);
    for (m, pick) in picks {
        match pick {
            Some((entry, reason, locked)) => {
                file.choices.insert(
                    m.key(),
                    Choice { asset: entry.path.clone(), name: entry.name.clone(), locked: *locked, reason: reason.clone(), member: m.name.clone() },
                );
            }
            None => {
                file.choices.remove(&m.key());
            }
        }
    }
    save_casting(root, &file)
}

/// A shared handle, so the dialog and the producer can hold one catalogue.
pub type SharedCatalogue = Arc<Vec<Entry>>;

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_ai::llm::LlmError;
    use serde::de::DeserializeOwned;

    /// Fails every call, so a test that reaches the model is seen.
    struct Refuses;

    impl Llm for Refuses {
        fn chat_json<T: DeserializeOwned>(&self, _m: &str, _s: &str, _u: &str, _i: &[&[u8]], _sc: Value) -> Result<T, LlmError> {
            panic!("the model must not be asked");
        }

        fn embed(&self, _m: &str, _t: &[&str]) -> Result<Vec<Vec<f32>>, LlmError> {
            Err(LlmError::Other("no".into()))
        }
    }

    fn entry(name: &str, folder: &str, text: &str) -> Entry {
        Entry { name: name.into(), folder: folder.into(), path: format!("{folder}/{name}.buzz"), thumb: String::new(), text: text.into() }
    }

    fn library() -> Vec<Entry> {
        vec![
            entry("Woman3Symbol", "Characters", "woman saree"),
            entry("sadhuSideWalk", "Characters", "sadhu old man walking"),
            entry("boy", "Characters", "boy child"),
            entry("Raju", "Cast", ""),
        ]
    }

    fn member(name: &str, aliases: &[&str]) -> Member {
        Member { id: name.to_uppercase(), name: name.into(), aliases: aliases.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn each_series_casts_only_from_its_own_folders() {
        let src = |lib: &str| CastSources { library: vec![lib.into()], ..Default::default() };
        let (k, o) = (src("Characters"), src("Cast"));
        assert!(in_sources(Some(&k), "Characters/charactersNew1", "chudailSwinging"));
        assert!(!in_sources(Some(&k), "Cast", "Raju"));
        assert!(!in_sources(Some(&k), "Characters", "Ana 3"));
        assert!(in_sources(Some(&o), "Cast", "Raju"));
        assert!(!in_sources(Some(&o), "Characters", "chudailSwinging"));
        assert!(!in_sources(Some(&o), "Cast", "Bo 2"));
        assert!(in_sources(None, "Cast", "Raju") && !in_sources(None, "Cast", "Ana 1"));
    }

    #[test]
    fn a_locked_choice_is_reused_without_asking() {
        let m = member("Sharma Ji", &[]);
        let mut file = CastingFile::default();
        file.choices.insert(m.key(), Choice { asset: "Characters/boy.buzz".into(), name: "boy".into(), locked: true, ..Default::default() });
        let mut cast = MyCast::from_parts(Path::new("."), library(), file, false);
        match cast.cast_member(&Refuses, &m, &|| false) {
            Outcome::Chosen { entry, via, .. } => {
                assert_eq!(entry.name, "boy");
                assert_eq!(via, Via::Locked);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_shortlist_puts_a_name_or_alias_match_first() {
        let mut m = member("Budhiya", &["Raju"]);
        m.gender = "female".into();
        m.look = "woman in a saree".into();
        let list = shortlist(&library(), &m, 5);
        assert_eq!(list[0].entry.name, "Raju");
        assert!(list.iter().any(|c| c.entry.name == "Woman3Symbol"));
        // The exact alias settles it: the model is not asked.
        let mut cast = MyCast::from_parts(Path::new("."), library(), CastingFile::default(), false);
        assert!(matches!(cast.cast_member(&Refuses, &m, &|| false), Outcome::Chosen { via: Via::Name, .. }));
    }

    #[test]
    fn no_candidate_and_no_generated_cast_is_reported_and_not_drawn() {
        let m = Member { id: "X".into(), name: "Zzyzx".into(), role: "banker".into(), ..Default::default() };
        // The whole catalogue goes to the model; with no model answering and no shortlist, nobody fits.
        let mut cast = MyCast::from_parts(Path::new("."), library(), CastingFile::default(), false);
        assert!(matches!(cast.cast_member(&Silent, &m, &|| false), Outcome::NoFit(w) if w.contains("no character of yours fits")));
        let mut cast = MyCast::from_parts(Path::new("."), library(), CastingFile::default(), true);
        assert_eq!(cast.cast_member(&Silent, &m, &|| false), Outcome::Generate);
    }

    /// No model answers (every call errs).
    struct Silent;

    impl Llm for Silent {
        fn chat_json<T: DeserializeOwned>(&self, _m: &str, _s: &str, _u: &str, _i: &[&[u8]], _sc: Value) -> Result<T, LlmError> {
            Err(LlmError::Other("down".into()))
        }

        fn embed(&self, _m: &str, _t: &[&str]) -> Result<Vec<Vec<f32>>, LlmError> {
            Err(LlmError::Other("no".into()))
        }
    }

    #[test]
    fn the_casting_prompt_lists_every_candidate_with_its_description() {
        let lib = library();
        let m = Member { name: "Forest Officer".into(), role: "officer".into(), gender: "male".into(), ..Default::default() };
        let entries: Vec<(&Entry, String)> = lib.iter().map(|e| (e, format!("look of {}", e.name))).collect();
        let task = catalogue_prompt(&m, &entries);
        let cands = task["candidates"].as_array().unwrap();
        assert_eq!(cands.len(), lib.len());
        for (c, e) in cands.iter().zip(&lib) {
            assert_eq!((c["id"].as_str(), c["name"].as_str(), c["folder"].as_str()), (Some(e.path.as_str()), Some(e.name.as_str()), Some(e.folder.as_str())));
            assert_eq!(c["description"], format!("look of {}", e.name));
        }
        assert_eq!(task["person"]["name"], "Forest Officer");
    }
}
