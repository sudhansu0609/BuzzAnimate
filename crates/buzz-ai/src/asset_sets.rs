//! **Sets from the user's own assets** (AS3): the pure half. From a scene's
//! words (slugline + action lines), its time of day and where the cast
//! stands, choose pieces from the asset index ([`crate::asset_index`]) and
//! say where each goes. Placing them is `buzz_app::asset_sets`.
//!
//! # The set it plans
//!
//! * **Exterior:** one sky that covers the stage above the horizon; trees
//!   along the horizon (far, depth 0.85); buildings in front of them (mid,
//!   depth 0.6) — shops first when the words name a stall or a market; one or
//!   two big trees at the edges (near, depth 0.35); a few bushes on the ground
//!   line, kept away from the cast's marks (depth 0.15). Every piece stands
//!   with its base on a line below the horizon, so nothing floats.
//! * **Interior** (`INT.`): one room that covers the stage, chosen by the
//!   slugline's words (`RAJU'S HOUSE` → a house interior), never a vehicle,
//!   tent, temple… unless the words say so.
//!
//! # How a piece is chosen
//!
//! Kind first (the index's `kind`), then words: the scene's words (and a few
//! Hindi/Hinglish synonyms) against the record's name, tags and VLM label.
//! Then **style**: the first building chosen is the anchor, and every later
//! piece gains for SigLIP similarity to it, measured against the other pieces
//! of its own kind (an embedding is mostly about *what* a picture is, so the
//! raw cosine of a tree to a house is low for every tree). Then **variety**: a
//! piece is never used twice, nor a near-copy of one already used (cosine
//! ≥ 0.97, or the same name but for a `copy` suffix), unless nothing else is
//! left. Ties are broken by a jitter seeded from the scene, so the same scene
//! always gets the same set and two scenes get different ones.
//!
//! Records never used: anything in `Cast/`, characters, groups, animals,
//! vehicles, effects, blank thumbnails, and — outdoors — interiors (by kind,
//! by an `interior` tag, or by an `In`/`room` name), and a symbol or part
//! whose kind was only guessed from its asset unless its own name names that
//! kind (`stove` inside `stall` is not a shop). Parts (AS4) stand in as trees
//! and bushes, never as skies, rooms or buildings (a part of a house is a
//! fragment). Skies are chosen by their mean colour as well as their name
//! ([`IndexRecord::tone`]). Backdrops (a thumbnail
//! whose drawing fills its box: [`IndexRecord::fill`]) are only ever covers,
//! never horizon pieces, and covers are only backdrops.
//!
//! # Colour and variety (AS5)
//!
//! An embedding says *what* a picture is far more than how it is painted, so
//! a lime-green bush came out nearest the style anchor of a set of dark,
//! muted greens. The colour gate measures that instead: each record's
//! [`Swatch`] (mean CIE Lab of its thumbnail's opaque pixels, and their mean
//! chroma), supplied by the caller ([`plan_set_coloured`]). Once the sky, the
//! buildings and the treeline are chosen, their median Lab is the set's
//! palette; a near tree or a bush whose ΔE (CIE76) to it is over
//! [`COLOUR_MAX_DE`], or whose chroma is more than [`COLOUR_MAX_CHROMA`] over
//! the palette's, is dropped — unless too few pass, when the nearest by that
//! measure are kept, so a role is never empty. And two scenes in one place
//! (same words, different scene numbers, so different seeds) get different
//! pieces: every pick but the sky and the shop the script names is drawn,
//! by the seed, from the best few within [`VARIETY_BAND`] of the best.

use std::collections::HashSet;

use crate::asset_index::{confident_name_guess, cosine, is_cast, name_tokens, AssetIndex, AssetKind, IndexRecord};

/// Time of day, for choosing a sky.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeOfDay {
    #[default]
    Day,
    Evening,
    Night,
}

/// What a set is asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct SetQuery {
    /// The scene's words: slugline location, title, action lines.
    pub text: String,
    /// An `INT.` scene: a room, not a street.
    pub interior: bool,
    pub time: TimeOfDay,
    /// Prefer the style of this asset (a name, matched case-insensitively,
    /// or words to match), when given.
    pub style: Option<String>,
    /// Same seed, same set.
    pub seed: u64,
    /// Where the cast (and the casting's props) stand, `0..1` across the
    /// stage: the ground pieces keep clear of these.
    pub marks: Vec<f64>,
    /// Where the ground meets the sky, `0..1` down the stage.
    pub horizon: f64,
    /// The ground line the cast stands on, `0..1` down the stage.
    pub ground: f64,
}

impl Default for SetQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            interior: false,
            time: TimeOfDay::Day,
            style: None,
            seed: 0,
            marks: Vec::new(),
            horizon: 0.6,
            ground: 0.9,
        }
    }
}

/// Where a piece goes in the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRole {
    /// Covers the stage above the horizon.
    Sky,
    /// Covers the whole stage.
    Room,
    /// Trees along the horizon.
    Far,
    /// Buildings in front of them.
    Mid,
    /// A big tree at the edge.
    Near,
    /// On the ground line, clear of the cast.
    Ground,
}

impl SetRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sky => "sky",
            Self::Room => "room",
            Self::Far => "far",
            Self::Mid => "mid",
            Self::Near => "near",
            Self::Ground => "ground",
        }
    }

    /// The layer depth the piece goes at (parallax): further is larger.
    pub fn depth(self) -> f64 {
        match self {
            Self::Sky => 0.95,
            Self::Room => 0.9,
            Self::Far => 0.85,
            Self::Mid => 0.6,
            Self::Near => 0.35,
            Self::Ground => 0.15,
        }
    }

    /// Covers (sky, room) are scaled to cover; the rest stand on a line.
    pub fn covers(self) -> bool {
        matches!(self, Self::Sky | Self::Room)
    }
}

/// One chosen piece and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPiece {
    /// The index record's id.
    pub id: String,
    pub path: String,
    pub symbol: Option<u64>,
    pub part: Option<u32>,
    pub name: String,
    pub role: SetRole,
    /// Middle across the stage, `0..1` (covers: 0.5).
    pub x: f64,
    /// Base down the stage, `0..1` (sky: the horizon; room: 1).
    pub base: f64,
    /// Height as a fraction of the stage height (covers: 0).
    pub height: f64,
    pub depth: f64,
    /// Why it was chosen.
    pub why: String,
}

/// The whole set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetPlan {
    /// Back to front: the cover first, then far, mid, near, ground.
    pub pieces: Vec<SetPiece>,
    /// What could not be matched, and other things worth saying.
    pub notes: Vec<String>,
    /// Nothing in the index fits: the caller lays procedural scenery.
    pub procedural: bool,
}

impl SetPlan {
    /// One line per piece, for a report.
    pub fn lines(&self) -> Vec<String> {
        self.pieces
            .iter()
            .map(|p| format!("{} \u{2014} {} ({}) \u{2014} {}", p.role.label(), p.name, p.id, p.why))
            .collect()
    }
}

// ------------------------------------------------------------------ words

/// Words that say nothing about a place.
const STOP: &[&str] = &[
    "the", "and", "with", "from", "into", "onto", "that", "this", "his", "her", "their", "them", "they", "for", "are",
    "was", "were", "has", "have", "had", "but", "not", "who", "what", "all", "one", "two", "out", "off", "over", "under",
    "back", "front", "earlier", "later", "morning", "evening", "night", "day", "afternoon", "dawn", "dusk", "noon",
    "ext", "int", "scene", "cut", "continuous", "same", "time", "quick", "insert", "beat", "looks", "look", "toward",
    "towards", "sits", "sitting", "stands", "standing", "walks", "walking", "says", "said", "then", "there", "here",
];

/// A few Hindi/Hinglish names for the places a script names in English.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("tea", &["chai", "tapri", "stall", "dhaba"]),
    ("stall", &["tapri", "stall", "dhaba", "shop", "dukan"]),
    ("shop", &["shop", "dukan", "dukaan", "stall", "store"]),
    ("store", &["shop", "dukan", "store"]),
    ("market", &["market", "bazaar", "bazar", "haat", "shop"]),
    ("house", &["house", "ghar", "home", "haveli"]),
    ("home", &["house", "ghar", "home"]),
    ("village", &["village", "vill", "gaon", "hut", "jhopdi"]),
    ("colony", &["house", "quarter", "building", "colony"]),
    ("lane", &["lane", "gali", "road", "path"]),
    ("road", &["road", "sadak", "way"]),
    ("street", &["street", "road", "gali", "lane"]),
    ("field", &["field", "khet", "crop", "wheat"]),
    ("fields", &["field", "khet", "crop", "wheat"]),
    ("temple", &["mandir", "temple"]),
    ("river", &["river", "nadi", "ghat"]),
    ("office", &["office", "building", "court", "bank"]),
    ("school", &["school", "class"]),
    ("hospital", &["hospital"]),
    ("forest", &["forest", "jungle", "tree"]),
    ("tree", &["tree", "ped"]),
    ("room", &["room", "kamra"]),
    ("kitchen", &["kitchen", "rasoi"]),
];

/// The scene's words, lower-case, with synonyms added.
fn query_words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '\'') {
        let w = raw.trim_matches('\'').to_lowercase();
        let w = w.strip_suffix("'s").unwrap_or(&w).to_string();
        if w.chars().count() < 3 || STOP.contains(&w.as_str()) || w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Some((_, more)) = SYNONYMS.iter().find(|(k, _)| *k == w) {
            for m in *more {
                if !out.iter().any(|o| o == m) {
                    out.push(m.to_string());
                }
            }
        }
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// A record's own words: name tokens, tags, the VLM's label words.
fn record_words(r: &IndexRecord) -> Vec<String> {
    let mut out = name_tokens(&r.name);
    for t in &r.tags {
        out.extend(t.to_lowercase().split(['_', ' ', '-']).filter(|w| !w.is_empty()).map(String::from));
    }
    if let Some(l) = &r.label {
        out.extend(l.to_lowercase().split(['_', ' ', '-']).filter(|w| !w.is_empty()).map(String::from));
    }
    out
}

/// How many of the scene's words the record's words carry (a word counts when
/// either contains the other and the shorter is 4 letters or more, or they
/// are equal).
fn word_hits(query: &[String], words: &[String]) -> usize {
    query
        .iter()
        .filter(|q| {
            words.iter().any(|w| {
                w == *q || (w.len().min(q.len()) >= 4 && (w.contains(q.as_str()) || q.contains(w.as_str())))
            })
        })
        .count()
}

fn has_any(words: &[String], what: &[&str]) -> bool {
    words.iter().any(|w| what.iter().any(|x| w == x || (x.len() >= 4 && w.contains(x))))
}

/// The name without a trailing `copy`, `copy 2`…: copies of one drawing.
fn family(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.find(" copy") {
        Some(i) => lower[..i].trim().to_string(),
        None => lower.trim().to_string(),
    }
}

// ------------------------------------------------------------------ seeded jitter

fn mix(mut x: u64) -> u64 {
    // splitmix64
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `0..1`, fixed for a seed and a key.
fn unit(seed: u64, key: &str) -> f64 {
    (mix(seed ^ hash_str(key)) >> 11) as f64 / (1u64 << 53) as f64
}

/// A seed from any text (a scene number and its slugline, say).
pub fn seed_of(text: &str) -> u64 {
    mix(hash_str(text))
}

// ------------------------------------------------------------------ colour (AS5)

/// A record's colour: the mean CIE Lab (D65) of its thumbnail's opaque
/// pixels, and their mean chroma (`hypot(a, b)`, so a picture of saturated
/// greens and reds is told from a grey one with the same mean).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swatch {
    pub lab: [f32; 3],
    pub chroma: f32,
}

/// Over this ΔE (CIE76) from the set's palette, a near tree or a bush is off
/// the set's colours. Picked by looking at every bush and tree of the user's
/// library against the Ep1 village sets (palette about L 21, a −7, b 13):
/// under it, the dark, muted greens the sets are painted in; over it, black
/// silhouettes, grey placeholder slabs, bright cartoon greens, flowers, the
/// chimneys and factory "plants" filed as bushes, the lime bush (42). 24 kept
/// the chimneys; under 20 starts losing ordinary muted bushes.
pub const COLOUR_MAX_DE: f32 = 20.0;

/// Over the palette's chroma by more than this, a piece is too saturated for
/// the set (the lime bush: +25; the brightest bushes that still sit well:
/// +18).
pub const COLOUR_MAX_CHROMA: f32 = 20.0;

/// How far under the best score a pick may still be drawn for variety.
pub const VARIETY_BAND: f64 = 0.8;

/// How many of the best a pick is drawn from, at most.
const VARIETY_TOP: usize = 4;

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn lab_f(t: f32) -> f32 {
    if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 }
}

/// CIE Lab (D65) of an sRGB colour.
pub fn srgb_to_lab(rgb: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let (fx, fy, fz) = (lab_f(x), lab_f(y), lab_f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The [`Swatch`] of a thumbnail (pixels with alpha over 128); `None` when
/// nothing in it is opaque.
pub fn thumb_swatch(img: &image::RgbaImage) -> Option<Swatch> {
    let mut lut: std::collections::HashMap<[u8; 3], [f32; 3]> = std::collections::HashMap::new();
    let (mut n, mut sum, mut chroma) = (0u64, [0f64; 3], 0f64);
    for p in img.pixels().filter(|p| p[3] > 128) {
        let rgb = [p[0], p[1], p[2]];
        let lab = *lut.entry(rgb).or_insert_with(|| srgb_to_lab(rgb));
        n += 1;
        for c in 0..3 {
            sum[c] += lab[c] as f64;
        }
        chroma += (lab[1] as f64).hypot(lab[2] as f64);
    }
    (n > 0).then(|| Swatch {
        lab: sum.map(|v| (v / n as f64) as f32),
        chroma: (chroma / n as f64) as f32,
    })
}

/// The set's palette: the per-channel median of its pieces' swatches.
fn palette_of(swatches: &[Swatch]) -> Option<Swatch> {
    if swatches.is_empty() {
        return None;
    }
    let median = |mut v: Vec<f32>| {
        v.sort_by(f32::total_cmp);
        let m = v.len() / 2;
        if v.len() % 2 == 0 { (v[m - 1] + v[m]) / 2.0 } else { v[m] }
    };
    Some(Swatch {
        lab: [0, 1, 2].map(|c| median(swatches.iter().map(|s| s.lab[c]).collect())),
        chroma: median(swatches.iter().map(|s| s.chroma).collect()),
    })
}

/// How far off the palette a swatch is: 1 at a threshold, under 1 inside
/// both, so one number orders the fallback.
fn off_palette(s: &Swatch, palette: &Swatch) -> f32 {
    let de = (0..3).map(|c| (s.lab[c] - palette.lab[c]).powi(2)).sum::<f32>().sqrt();
    (de / COLOUR_MAX_DE).max((s.chroma - palette.chroma) / COLOUR_MAX_CHROMA)
}

/// The candidates that sit in the palette; at least `need` of them (the
/// nearest) when fewer do. Unmeasured candidates pass. Returns the kept
/// pool and how many were dropped.
fn colour_gate<'a, 'b>(
    pool: &[&'b Cand<'a>],
    palette: Option<&Swatch>,
    colour: &dyn Fn(&IndexRecord) -> Option<Swatch>,
    need: usize,
) -> (Vec<&'b Cand<'a>>, usize) {
    let Some(palette) = palette else { return (pool.to_vec(), 0) };
    let mut scored: Vec<(f32, &'b Cand<'a>)> =
        pool.iter().map(|c| (colour(c.r).map_or(0.0, |s| off_palette(&s, palette)), *c)).collect();
    let kept: Vec<&'b Cand<'a>> = scored.iter().filter(|(d, _)| *d <= 1.0).map(|(_, c)| *c).collect();
    if kept.len() >= need.min(pool.len()) {
        let dropped = pool.len() - kept.len();
        return (kept, dropped);
    }
    scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.r.id.cmp(&b.1.r.id)));
    let kept: Vec<&'b Cand<'a>> = scored.iter().take(need).map(|(_, c)| *c).collect();
    let dropped = pool.len() - kept.len();
    (kept, dropped)
}

// ------------------------------------------------------------------ candidates

/// What a record is good for.
struct Cand<'a> {
    r: &'a IndexRecord,
    words: Vec<String>,
}

/// Interior-ness from anything but the kind (a house record that is a room).
fn looks_inside(words: &[String], r: &IndexRecord) -> bool {
    r.kind == AssetKind::Interior
        || r.tags.iter().any(|t| t.eq_ignore_ascii_case("interior") || t.eq_ignore_ascii_case("indoor"))
        || words.iter().any(|w| matches!(w.as_str(), "in" | "ins" | "inside" | "interior" | "indoor" | "room" | "kamra" | "andar"))
        || r.label.as_deref().is_some_and(|l| l.contains("interior") || l.contains("room") || l.contains("inside"))
}

/// A backdrop: its drawing fills its box (or the VLM called it a scene).
fn is_backdrop(c: &Cand) -> bool {
    c.r.fill.is_some_and(|f| f >= 0.9)
        || has_any(&c.words, &["background", "scene", "scenery", "landscape", "backdrop"])
}

/// A cut-out: measured and clearly not filling its box, and not a scene.
fn is_cutout(c: &Cand) -> bool {
    c.r.fill.is_none_or(|f| f < 0.86) && !has_any(&c.words, &["background", "scene", "scenery", "landscape", "backdrop"])
}

/// A symbol or part whose kind was only guessed takes its asset's guess
/// when its own name says nothing (`stove` inside `stall` came out a shop):
/// such a record is used only when its own name names its kind.
fn name_backs_kind(r: &IndexRecord) -> bool {
    r.is_asset() || !r.guessed || confident_name_guess(&r.name).is_some_and(|g| g.kind == r.kind)
}

/// Never part of a set.
fn excluded(r: &IndexRecord) -> bool {
    is_cast(&r.folder)
        || r.blank == Some(true)
        || !name_backs_kind(r)
        || matches!(
            r.kind,
            AssetKind::Character | AssetKind::Group | AssetKind::Animal | AssetKind::Vehicle | AssetKind::Effect
        )
}

fn time_words(t: TimeOfDay) -> (&'static [&'static str], &'static [&'static str]) {
    // (wanted, refused)
    match t {
        TimeOfDay::Day => (&["day", "blue", "skyblue"], &["night", "dark", "evening", "eveming", "sunset", "red", "reddish", "moon", "star", "stars"]),
        TimeOfDay::Evening => (&["evening", "eveming", "sunset", "red", "reddish", "dusk"], &["night", "moon", "stars"]),
        TimeOfDay::Night => (&["night", "dark", "moon", "stars", "star"], &["day", "sunset", "evening", "eveming"]),
    }
}

/// Whether a sky's mean colour can be this time of day: a day sky is bright
/// and at least as blue as red, an evening one redder than blue, a night one
/// dark. Unmeasured skies are allowed (the words decide).
fn tone_allows(tone: Option<[u8; 3]>, t: TimeOfDay) -> bool {
    let Some([r, g, b]) = tone else { return true };
    let (r, g, b) = (r as f64, g as f64, b as f64);
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
    match t {
        TimeOfDay::Day => b >= r && lum >= 110.0,
        TimeOfDay::Evening => r > b + 25.0 && lum >= 35.0,
        TimeOfDay::Night => lum < 70.0,
    }
}

/// How well a sky's mean colour fits the time of day, and why.
fn tone_fit(tone: Option<[u8; 3]>, t: TimeOfDay) -> (f64, String) {
    let Some([r, g, b]) = tone else { return (0.0, String::new()) };
    let (r, g, b) = (r as f64, g as f64, b as f64);
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
    let fit = match t {
        TimeOfDay::Day => 4.0 * (b - r) / 255.0,
        TimeOfDay::Evening => 3.0 * (r - b) / 255.0,
        TimeOfDay::Night => (70.0 - lum) / 50.0,
    };
    (fit.max(0.0), format!("colour fits the {}", match t {
        TimeOfDay::Day => "day",
        TimeOfDay::Evening => "evening",
        TimeOfDay::Night => "night",
    }))
}

/// The scene's sense of place.
#[derive(Debug, Clone, Copy, Default)]
struct Wants {
    shops: bool,
    houses: bool,
    forest: bool,
}

fn wants(query: &[String]) -> Wants {
    let q = |w: &[&str]| query.iter().any(|x| w.contains(&x.as_str()));
    let shops = q(&["shop", "stall", "tapri", "dhaba", "market", "bazaar", "store", "dukan"]);
    let forest = q(&["forest", "jungle"]) && !q(&["village", "house", "colony", "road", "street", "lane"]);
    Wants { shops, houses: !forest, forest }
}

/// Style bonus: how much closer to the anchor than the average piece of its
/// kind, scaled to about -1..1.
struct Style<'a> {
    anchor: Option<&'a [f32]>,
}

impl Style<'_> {
    fn bonus(&self, c: &Cand, pool: &[&Cand]) -> f64 {
        let Some(a) = self.anchor else { return 0.0 };
        if c.r.embedding.is_empty() {
            return 0.0;
        }
        let sims: Vec<f64> = pool
            .iter()
            .filter(|p| !p.r.embedding.is_empty())
            .map(|p| cosine(&p.r.embedding, a) as f64)
            .collect();
        if sims.is_empty() {
            return 0.0;
        }
        let mean = sims.iter().sum::<f64>() / sims.len() as f64;
        let max = sims.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let s = cosine(&c.r.embedding, a) as f64;
        ((s - mean) / (max - mean + 1e-3)).clamp(-1.0, 1.0)
    }
}

/// Pieces already used, to keep the set varied.
#[derive(Default)]
struct Used<'a> {
    ids: HashSet<String>,
    families: HashSet<String>,
    embeddings: Vec<&'a [f32]>,
}

impl<'a> Used<'a> {
    fn near_copy(&self, r: &IndexRecord) -> bool {
        self.ids.contains(&r.id)
            || self.families.contains(&family(&r.name))
            || (!r.embedding.is_empty() && self.embeddings.iter().any(|e| cosine(e, &r.embedding) >= 0.97))
    }

    fn take(&mut self, r: &'a IndexRecord) {
        self.ids.insert(r.id.clone());
        self.families.insert(family(&r.name));
        if !r.embedding.is_empty() {
            self.embeddings.push(&r.embedding);
        }
    }
}

/// Score each candidate, best first: `(score, why)`.
fn rank<'a, 'b>(
    pool: &[&'b Cand<'a>],
    query: &[String],
    style: &Style,
    seed: u64,
    word_weight: f64,
    extra: impl Fn(&Cand) -> (f64, String),
) -> Vec<(f64, &'b Cand<'a>, String)> {
    let mut out: Vec<(f64, &Cand, String)> = pool
        .iter()
        .map(|c| {
            let hits = word_hits(query, &c.words);
            let st = style.bonus(c, pool);
            let (bonus, note) = extra(c);
            let jitter = unit(seed, &c.r.id) * 0.9;
            let score = word_weight * hits as f64 + 1.0 * st + bonus + jitter + if c.r.guessed { 0.0 } else { 0.15 };
            let mut why = format!("{}", c.r.kind.as_str());
            if hits > 0 {
                why.push_str(&format!(", {hits} word(s) match the scene"));
            }
            if style.anchor.is_some() && !c.r.embedding.is_empty() {
                why.push_str(&format!(", style {st:+.2}"));
            }
            if !note.is_empty() {
                why.push_str(", ");
                why.push_str(&note);
            }
            if c.r.guessed {
                why.push_str(", kind from name");
            }
            (score, *c, why)
        })
        .collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.r.id.cmp(&b.1.r.id)));
    out
}

/// The best not-yet-used candidate (a used one only when nothing else is left).
fn pick<'a, 'b>(ranked: &[(f64, &'b Cand<'a>, String)], used: &Used) -> Option<(&'b Cand<'a>, String)> {
    ranked
        .iter()
        .find(|(_, c, _)| !used.near_copy(c.r))
        .or_else(|| ranked.first())
        .map(|(_, c, why)| (*c, why.clone()))
}

/// Like [`pick`], but drawn by `seed` from the best few unused candidates
/// within [`VARIETY_BAND`] of the best and of the best's own kind (AS5), so
/// two scenes in one place get different pieces where there are
/// alternatives — a house for another house, never a house swapped for a
/// second shop.
fn pick_varied<'a, 'b>(
    ranked: &[(f64, &'b Cand<'a>, String)],
    used: &Used,
    seed: u64,
    key: &str,
) -> Option<(&'b Cand<'a>, String)> {
    let free: Vec<&(f64, &'b Cand<'a>, String)> = ranked.iter().filter(|(_, c, _)| !used.near_copy(c.r)).collect();
    let Some((best, kind)) = free.first().map(|f| (f.0, f.1.r.kind)) else { return pick(ranked, used) };
    let band: Vec<&(f64, &'b Cand<'a>, String)> =
        free.iter().take_while(|(s, _, _)| *s >= best - VARIETY_BAND).filter(|(_, c, _)| c.r.kind == kind).take(VARIETY_TOP).copied().collect();
    let i = ((unit(seed, key) * band.len() as f64) as usize).min(band.len() - 1);
    band.get(i).map(|(_, c, why)| (*c, why.clone()))
}

fn piece(c: &Cand, role: SetRole, x: f64, base: f64, height: f64, why: String) -> SetPiece {
    SetPiece {
        id: c.r.id.clone(),
        path: c.r.path.clone(),
        symbol: c.r.symbol,
        part: c.r.part,
        name: c.r.name.clone(),
        role,
        x,
        base,
        height,
        depth: role.depth(),
        why,
    }
}

/// Height so the piece is no wider than `max_w` of the stage width.
fn fit_height(want_h: f64, aspect: f64, stage_aspect: f64, max_w: f64) -> f64 {
    let w = want_h * aspect / stage_aspect;
    if w > max_w { want_h * max_w / w } else { want_h }
}

// ------------------------------------------------------------------ the plan

/// **Plan a set** for `query` from `index`. `stage_aspect` is width / height.
/// No colour gate: [`plan_set_coloured`] with no colours.
pub fn plan_set(index: &AssetIndex, query: &SetQuery, stage_aspect: f64) -> SetPlan {
    plan_set_coloured(index, query, stage_aspect, &|_| None)
}

/// **Plan a set**, with each record's colour from `colour` (its thumbnail's
/// [`Swatch`], `None` when unknown) for the colour gate.
pub fn plan_set_coloured(
    index: &AssetIndex,
    query: &SetQuery,
    stage_aspect: f64,
    colour: &dyn Fn(&IndexRecord) -> Option<Swatch>,
) -> SetPlan {
    let mut plan = SetPlan::default();
    let words = query_words(&query.text);
    let want = wants(&words);
    let cands: Vec<Cand> = index
        .records
        .iter()
        .filter(|r| !excluded(r))
        .map(|r| Cand { r, words: record_words(r) })
        .collect();
    let seed = query.seed;
    let stage_aspect = stage_aspect.max(0.1);

    // A named style: the record it names, or the best word match.
    let mut style_anchor: Option<&[f32]> = None;
    if let Some(s) = query.style.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let lower = s.to_lowercase();
        let named = index
            .records
            .iter()
            .filter(|r| !r.embedding.is_empty())
            .find(|r| r.name.to_lowercase() == lower || r.id.to_lowercase() == lower)
            .or_else(|| {
                let sw = query_words(s);
                index
                    .records
                    .iter()
                    .filter(|r| !r.embedding.is_empty() && !is_cast(&r.folder))
                    .max_by_key(|r| (word_hits(&sw, &record_words(r)), std::cmp::Reverse(r.id.clone())))
                    .filter(|r| word_hits(&sw, &record_words(r)) > 0)
            });
        match named {
            Some(r) => {
                style_anchor = Some(&r.embedding);
                plan.notes.push(format!("style anchored on {:?}", r.name));
            }
            None => plan.notes.push(format!("no asset matches the style {s:?}; style not constrained")),
        }
    }
    let mut used = Used::default();

    // -------------------------------------------------------------- interior
    if query.interior {
        let refused_unless = [
            (&["car", "bus", "train", "vehicle", "dashboard", "jeep"][..], "a vehicle"),
            (&["tent"][..], "a tent"),
            (&["hospital"][..], "a hospital"),
            (&["mandir", "temple"][..], "a temple"),
            (&["mehal", "mahal", "palace", "rani", "raja", "pricess", "princess"][..], "a palace"),
            (&["bathroom", "toilet"][..], "a bathroom"),
            (&["class", "classroom", "classromm", "school"][..], "a classroom"),
            (&["corridoor", "corridor", "coridoor", "gallary", "gallery"][..], "a corridor"),
            (&["garage", "motor"][..], "a garage"),
            (&["ceiling"][..], "a ceiling"),
            (&["top", "topview"][..], "a top view"),
        ];
        let pool: Vec<&Cand> = cands
            .iter()
            .filter(|c| c.r.part.is_none())
            .filter(|c| c.r.kind == AssetKind::Interior || (looks_inside(&c.words, c.r) && is_backdrop(c)))
            .filter(|c| c.r.fill.is_none_or(|f| f >= 0.85) && c.r.aspect >= 1.1 && c.r.aspect <= 2.4)
            .filter(|c| {
                refused_unless
                    .iter()
                    .all(|(ws, _)| !has_any(&c.words, ws) || words.iter().any(|q| ws.contains(&q.as_str())))
            })
            .collect();
        let style = Style { anchor: style_anchor };
        let ranked = rank(&pool, &words, &style, seed, 1.2, |c| {
            let generic = has_any(&c.words, &["room", "house", "home", "ghar"]);
            (if generic { 0.4 } else { 0.0 }, String::new())
        });
        match pick(&ranked, &used) {
            Some((c, why)) => {
                used.take(c.r);
                plan.pieces.push(piece(c, SetRole::Room, 0.5, 1.0, 0.0, why));
            }
            None => {
                plan.notes.push("no interior in the index fits; procedural interior".into());
                plan.procedural = true;
            }
        }
        return plan;
    }

    // -------------------------------------------------------------- exterior
    let not_inside = |c: &&Cand| !looks_inside(&c.words, c.r);
    let style0 = Style { anchor: style_anchor };

    // The sky: a backdrop of kind sky, wide, of the right time of day.
    let (wanted, refused) = time_words(query.time);
    let skies: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.r.part.is_none() && c.r.kind == AssetKind::Sky && is_backdrop(c) && c.r.aspect >= 1.4)
        .filter(|c| c.r.fill.is_some())
        .filter(|c| !has_any(&c.words, refused))
        .filter(|c| tone_allows(c.r.tone, query.time))
        .collect();
    let ranked = rank(&skies, &words, &style0, seed, 0.3, |c| {
        let (b, mut why) = tone_fit(c.r.tone, query.time);
        let named = has_any(&c.words, wanted);
        if named {
            why = if why.is_empty() { "named for the time of day".into() } else { format!("{why}, named for it") };
        }
        (b + if named { 0.5 } else { 0.0 }, why)
    });
    match pick(&ranked, &used) {
        Some((c, why)) => {
            used.take(c.r);
            plan.pieces.push(piece(c, SetRole::Sky, 0.5, query.horizon, 0.0, why));
        }
        None => plan.notes.push("no sky in the index fits the time of day; procedural sky".into()),
    }

    let horizon = query.horizon;
    let ground = query.ground.max(horizon + 0.05);
    let between = ground - horizon;

    // Buildings: houses (and shops, when the words name one).
    let buildings: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.r.part.is_none() && matches!(c.r.kind, AssetKind::House | AssetKind::Shop))
        .filter(not_inside)
        .filter(|c| is_cutout(c) && c.r.aspect >= 0.6 && c.r.aspect <= 3.4)
        .filter(|c| !has_any(&c.words, &["top", "topview", "back", "dark", "silhouette", "roof", "ruin", "khandahar", "tent"]))
        .collect();
    let trees: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.r.kind == AssetKind::Tree)
        .filter(not_inside)
        .filter(|c| is_cutout(c) && c.r.aspect >= 0.35 && c.r.aspect <= 1.8)
        .filter(|c| !has_any(&c.words, &["top", "topview", "trunk", "forest", "jungle", "dark"]))
        .collect();
    let bushes: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.r.kind == AssetKind::Bush)
        .filter(not_inside)
        .filter(|c| is_cutout(c) && c.r.aspect >= 0.5 && c.r.aspect <= 3.5)
        .filter(|c| !has_any(&c.words, &["crop", "field", "lata", "tulsi", "animated"]))
        .collect();

    // The style anchor: the best building (or the named style).
    let mut anchor = style_anchor;
    let mut mid: Vec<(&Cand, String)> = Vec::new();
    if want.houses && !buildings.is_empty() {
        let n_mid = 3usize;
        for k in 0..n_mid {
            let style = Style { anchor };
            let shop_first = want.shops && k == 0;
            let ranked = rank(&buildings, &words, &style, seed.wrapping_add(k as u64), 1.2, |c| {
                let shop = c.r.kind == AssetKind::Shop;
                let b = if shop_first && shop {
                    2.5
                } else if shop && !want.shops {
                    -1.0
                } else {
                    0.0
                };
                (b, if b > 0.0 { "the scene names a shop".into() } else { String::new() })
            });
            // The shop the script names is the landmark: the same in every
            // scene there. The rest are drawn for variety.
            let chosen = if shop_first {
                pick(&ranked, &used)
            } else {
                pick_varied(&ranked, &used, seed, &format!("vary-mid{k}"))
            };
            let Some((c, why)) = chosen else { break };
            if used.near_copy(c.r) && k > 0 {
                break;
            }
            used.take(c.r);
            if anchor.is_none() && !c.r.embedding.is_empty() {
                anchor = Some(&c.r.embedding);
            }
            mid.push((c, why));
        }
    }
    if want.houses && mid.is_empty() {
        plan.notes.push("no house or shop in the index is a cut-out; no buildings".into());
    }
    let style = Style { anchor };

    // Far: a treeline along the horizon, left to right past both edges.
    let mut far: Vec<SetPiece> = Vec::new();
    if !trees.is_empty() {
        let mut cursor = -0.06;
        let mut k = 0u64;
        while cursor < 1.06 && far.len() < 8 {
            let ranked = rank(&trees, &words, &style, seed.wrapping_add(100 + k), 0.4, |_| (0.0, String::new()));
            let Some((c, why)) = pick_varied(&ranked, &used, seed, &format!("vary-far{k}")) else { break };
            used.take(c.r);
            let want_h = between * (0.95 + 0.25 * unit(seed, &format!("far-h{k}"))) * if want.forest { 1.3 } else { 1.0 };
            let h = fit_height(want_h, c.r.aspect, stage_aspect, 0.3);
            let w = h * c.r.aspect / stage_aspect;
            let x = cursor + w / 2.0;
            far.push(piece(c, SetRole::Far, x, horizon + 0.004, h, why));
            cursor += w * (0.7 + 0.5 * unit(seed, &format!("far-gap{k}")));
            k += 1;
        }
    } else {
        plan.notes.push("no tree in the index is a cut-out; no treeline".into());
    }

    // Mid: the buildings, spread across with the trees showing between.
    let mut mid_pieces: Vec<SetPiece> = Vec::new();
    let slots: Vec<f64> = match mid.len() {
        0 => Vec::new(),
        1 => vec![0.5],
        2 => vec![0.3, 0.72],
        _ => vec![0.14, 0.5, 0.86],
    };
    for (k, ((c, why), slot)) in mid.iter().zip(slots).enumerate() {
        let want_h = between * (1.05 + 0.2 * unit(seed, &format!("mid-h{k}")));
        let h = fit_height(want_h, c.r.aspect, stage_aspect, 0.36);
        let jig = (unit(seed, &format!("mid-x{k}")) - 0.5) * 0.06;
        mid_pieces.push(piece(c, SetRole::Mid, slot + jig, horizon + between * 0.12, h, why.clone()));
    }

    // The set's palette, from what is chosen so far (sky, buildings,
    // treeline); the near trees and the bushes must sit in it (AS5).
    let chosen: Vec<Swatch> = index
        .records
        .iter()
        .filter(|r| plan.pieces.iter().chain(&far).chain(&mid_pieces).any(|p| p.id == r.id))
        .filter_map(colour)
        .collect();
    let palette = palette_of(&chosen);
    let (near_trees, near_dropped) = colour_gate(&trees, palette.as_ref(), colour, 2);
    let (bushes, bush_dropped) = colour_gate(&bushes, palette.as_ref(), colour, 3);
    if near_dropped + bush_dropped > 0 {
        plan.notes.push(format!(
            "off the set's colours: {near_dropped} tree(s), {bush_dropped} bush(es) left out"
        ));
    }

    // Near: one or two big trees at the edges.
    let mut near: Vec<SetPiece> = Vec::new();
    for (k, edge) in [0.03f64, 0.97].iter().enumerate() {
        if near_trees.is_empty() {
            break;
        }
        // Only where no mark stands close to the edge.
        if query.marks.iter().any(|m| (m - edge).abs() < 0.08) {
            continue;
        }
        if k == 1 && unit(seed, "near-second") < 0.35 {
            continue;
        }
        let ranked =
            rank(&near_trees, &words, &style, seed.wrapping_add(200 + k as u64), 0.4, |_| (0.0, String::new()));
        let Some((c, why)) = pick_varied(&ranked, &used, seed, &format!("vary-near{k}")) else { break };
        used.take(c.r);
        let h = fit_height(between * 2.0, c.r.aspect, stage_aspect, 0.26);
        near.push(piece(c, SetRole::Near, *edge, horizon + between * 0.45, h, why));
    }

    // Ground: bushes on the ground line, clear of every mark.
    let mut low: Vec<SetPiece> = Vec::new();
    if !bushes.is_empty() {
        let mut spots: Vec<f64> = Vec::new();
        for k in 0..24 {
            if spots.len() >= 3 {
                break;
            }
            let x = 0.06 + 0.88 * unit(seed, &format!("bush-x{k}"));
            let clear = query.marks.iter().all(|m| (m - x).abs() >= 0.1) && spots.iter().all(|s| (s - x).abs() >= 0.15);
            if clear {
                spots.push(x);
            }
        }
        spots.sort_by(f64::total_cmp);
        for (k, x) in spots.iter().enumerate() {
            let ranked = rank(&bushes, &words, &style, seed.wrapping_add(300 + k as u64), 0.4, |_| (0.0, String::new()));
            let Some((c, why)) = pick_varied(&ranked, &used, seed, &format!("vary-bush{k}")) else { break };
            used.take(c.r);
            let h = fit_height(between * (0.28 + 0.12 * unit(seed, &format!("bush-h{k}"))), c.r.aspect, stage_aspect, 0.14);
            low.push(piece(c, SetRole::Ground, *x, ground + 0.012, h, why));
        }
    } else {
        plan.notes.push("no bush in the index is a cut-out; bare ground".into());
    }

    plan.pieces.extend(far);
    plan.pieces.extend(mid_pieces);
    plan.pieces.extend(near);
    plan.pieces.extend(low);
    if !plan.pieces.iter().any(|p| !p.role.covers()) {
        plan.procedural = true;
        plan.notes.push("nothing in the index matched the scene; procedural scenery".into());
    }
    plan
}
