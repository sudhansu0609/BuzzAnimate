//! **The asset index** (AS1) and **Arrange by type & style** (AS2): the pure
//! half — records, the VLM batch, name heuristics, style clustering, the
//! arrange plan and its reversible apply. Rendering thumbnails and running
//! SigLIP over them lives in `buzz-app` (`asset_index`), which owns a GPU.
//!
//! One [`IndexRecord`] per asset **and per distinct visual symbol inside it**
//! (see `docs/PRODUCTION_SPEED_PLAN.md` § AS). The index is a JSON file in the
//! library root ([`INDEX_FILE`]); thumbnails and the arrange move log live in
//! a hidden folder beside it ([`INDEX_DIR`]), which the Assets panel does not
//! list (`AssetLibrary` skips dot folders).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::llm::{Llm, LlmError};
use crate::naming::{AssetCategory, AssetView};

/// The index file, in the library root.
pub const INDEX_FILE: &str = ".buzz-index.json";
/// Thumbnails (`thumbs/`) and the arrange log, in the library root.
pub const INDEX_DIR: &str = ".buzz-index";
/// The arrange move log, inside [`INDEX_DIR`].
pub const MOVE_LOG_FILE: &str = "arrange-log.json";
/// Bumped when a record's meaning changes; an older index is rebuilt.
pub const INDEX_VERSION: u32 = 1;
/// VLM batch size (C8's).
pub const VLM_BATCH: usize = 8;
/// Average-linkage cosine floor for two assets to share a Style folder.
pub const STYLE_COSINE: f32 = 0.80;
/// A style cluster smaller than this is not given a folder of its own; its
/// members go straight into the Type folder.
pub const MIN_STYLE_SIZE: usize = 4;
/// Most Style folders one Type gets: the nearest clusters merge past
/// [`STYLE_COSINE`] until there are no more than this many.
pub const MAX_STYLES: usize = 6;
/// Bumped when [`AssetKind`] gains kinds: records the VLM identified under an
/// older list are marked guessed again (re-guessed by name, re-asked by the
/// next VLM pass) without re-rendering anything.
pub const TAXONOMY_VERSION: u32 = 2;
/// The Type folder for near-empty or solid-box thumbnails ([`thumb_is_blank`]).
pub const CHECK_FOLDER: &str = "Other/Check";

/// The finer kind of an asset or symbol (C8's category is too coarse to file by).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    House,
    Shop,
    Interior,
    Wall,
    Structure,
    Tree,
    Bush,
    Sky,
    Ground,
    Road,
    Furniture,
    Vehicle,
    Animal,
    Character,
    Group,
    Effect,
    Other,
}

impl AssetKind {
    pub const ALL: [AssetKind; 17] = [
        Self::House,
        Self::Shop,
        Self::Interior,
        Self::Wall,
        Self::Structure,
        Self::Tree,
        Self::Bush,
        Self::Sky,
        Self::Ground,
        Self::Road,
        Self::Furniture,
        Self::Vehicle,
        Self::Animal,
        Self::Character,
        Self::Group,
        Self::Effect,
        Self::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::House => "house",
            Self::Shop => "shop",
            Self::Interior => "interior",
            Self::Wall => "wall",
            Self::Structure => "structure",
            Self::Tree => "tree",
            Self::Bush => "bush",
            Self::Sky => "sky",
            Self::Ground => "ground",
            Self::Road => "road",
            Self::Furniture => "furniture",
            Self::Vehicle => "vehicle",
            Self::Animal => "animal",
            Self::Character => "character",
            Self::Group => "group",
            Self::Effect => "effect",
            Self::Other => "other",
        }
    }
}

/// The Type folder a record files under (AS2).
pub fn type_folder(kind: AssetKind, category: AssetCategory) -> &'static str {
    match kind {
        AssetKind::Character | AssetKind::Group => "Characters",
        AssetKind::House => "Buildings/Houses",
        AssetKind::Shop => "Buildings/Shops",
        AssetKind::Interior => "Interiors",
        AssetKind::Wall => "Walls & Fences",
        AssetKind::Structure => "Structures",
        AssetKind::Tree => "Nature/Trees",
        AssetKind::Bush => "Nature/Bushes",
        AssetKind::Sky => "Sky",
        AssetKind::Ground | AssetKind::Road => "Ground & Roads",
        AssetKind::Furniture => "Furniture",
        AssetKind::Vehicle => "Vehicles",
        AssetKind::Animal => "Animals",
        AssetKind::Effect => "Effects",
        AssetKind::Other => match category {
            AssetCategory::Prop => "Props",
            AssetCategory::Character => "Characters",
            AssetCategory::Vehicle => "Vehicles",
            AssetCategory::Animal => "Animals",
            AssetCategory::Effect => "Effects",
            _ => "Other",
        },
    }
}

/// Every Type folder, in the order the dialog lists them.
pub const TYPE_FOLDERS: [&str; 17] = [
    "Characters",
    "Buildings/Houses",
    "Buildings/Shops",
    "Interiors",
    "Walls & Fences",
    "Structures",
    "Nature/Trees",
    "Nature/Bushes",
    "Sky",
    "Ground & Roads",
    "Furniture",
    "Props",
    "Vehicles",
    "Animals",
    "Effects",
    "Other",
    CHECK_FOLDER,
];

/// One asset, or one distinct symbol inside an asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRecord {
    /// `path` for the asset itself, `path#symbol` for a symbol inside it.
    pub id: String,
    /// The asset file, relative to the library root, `/`-separated.
    pub path: String,
    /// The symbol inside the asset (`SymbolId.0`), `None` for the whole asset.
    #[serde(default)]
    pub symbol: Option<u64>,
    /// The asset's name, or the symbol's.
    pub name: String,
    /// The asset's folder (relative, `/`-separated, empty = root).
    pub folder: String,
    /// Thumbnail PNG, relative to the library root.
    pub thumb: String,
    /// SigLIP image embedding of the thumbnail (L2-normalised); empty when
    /// SigLIP could not run.
    #[serde(default, with = "b64_f32")]
    pub embedding: Vec<f32>,
    pub category: AssetCategory,
    pub kind: AssetKind,
    pub view: AssetView,
    #[serde(default)]
    pub tags: Vec<String>,
    /// The VLM's snake_case description (C8 `name`), when it ran.
    #[serde(default)]
    pub label: Option<String>,
    /// `[x0, y0, x1, y1]` in the asset's (or the symbol's parent's) space.
    pub bounds: [f64; 4],
    /// width / height.
    pub aspect: f64,
    /// The asset file's modification time, seconds since the epoch.
    pub mtime: u64,
    /// True when category/kind/view/tags come from name heuristics, not the VLM.
    pub guessed: bool,
    /// [`TAXONOMY_VERSION`] the kind was decided under (0 = before AS2b).
    #[serde(default)]
    pub taxonomy: u32,
    /// The thumbnail is near-empty or a solid box ([`thumb_is_blank`]);
    /// `None` until measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blank: Option<bool>,
    /// AS4: a **part** of an asset made of loose shapes (one connected group
    /// of its artwork, found by [`cluster_boxes`]); `bounds` is the part's box
    /// in the asset's main timeline. `None` for an asset or a symbol record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<u32>,
    /// The frame the thumbnail was taken at: the first with ink, for an
    /// animated asset whose frame 0 is empty. `None` = frame 0, taken before
    /// that rule (a near-empty one is re-rendered on the next build).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_frame: Option<u32>,
    /// How much of the thumbnail's drawn box is drawn (0..1): near 1 for a
    /// backdrop (a sky, a room, a whole scene), well under for a cut-out
    /// (a house, a tree). `None` until measured ([`thumb_fill`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<f32>,
    /// The mean colour of the thumbnail's opaque pixels ([`thumb_tone`]):
    /// tells a day sky from an evening or a night one when the name does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<[u8; 3]>,
}

impl IndexRecord {
    /// The whole asset (not a symbol or a part inside it).
    pub fn is_asset(&self) -> bool {
        self.symbol.is_none() && self.part.is_none()
    }
}

/// The whole index file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetIndex {
    pub version: u32,
    pub records: Vec<IndexRecord>,
    /// Style folder names given by the VLM, by [`style_key`].
    #[serde(default)]
    pub style_names: BTreeMap<String, String>,
}

impl AssetIndex {
    pub fn path_in(root: &Path) -> PathBuf {
        root.join(INDEX_FILE)
    }

    /// The index in `root`, or an empty one when there is none (or it is
    /// unreadable, or from an older [`INDEX_VERSION`]).
    pub fn load(root: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(Self::path_in(root)) else {
            return Self { version: INDEX_VERSION, ..Default::default() };
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(index) if index.version == INDEX_VERSION => index,
            _ => Self { version: INDEX_VERSION, ..Default::default() },
        }
    }

    /// Written to a temporary file and renamed over the old one, so a crash
    /// mid-write never leaves half an index.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = Self::path_in(root);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    }

    /// The whole-asset record for `path`.
    pub fn asset_record(&self, path: &str) -> Option<&IndexRecord> {
        self.records.iter().find(|r| r.path == path && r.is_asset())
    }

    /// Records of the symbols inside the asset at `path`.
    pub fn symbol_records<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a IndexRecord> + 'a {
        self.records.iter().filter(move |r| r.path == path && r.symbol.is_some())
    }

    /// After an arrange (or its undo) moved files: every record of a moved
    /// asset follows it. `moves` maps old relative path → new relative path.
    pub fn follow_moves(&mut self, moves: &HashMap<String, String>) {
        for r in &mut self.records {
            if let Some(to) = moves.get(&r.path) {
                r.path = to.clone();
                r.folder = folder_of(to);
                r.name = if r.is_asset() { stem_of(to) } else { r.name.clone() };
                r.id = match (r.symbol, r.part) {
                    (Some(s), _) => format!("{to}#{s}"),
                    (None, Some(p)) => format!("{to}#part{p}"),
                    (None, None) => to.clone(),
                };
            }
        }
    }
}

/// The folder part of a relative asset path.
pub fn folder_of(rel: &str) -> String {
    rel.rsplit_once('/').map(|(f, _)| f.to_string()).unwrap_or_default()
}

/// The file stem of a relative asset path.
pub fn stem_of(rel: &str) -> String {
    let file = rel.rsplit_once('/').map(|(_, f)| f).unwrap_or(rel);
    file.strip_suffix(&format!(".{}", buzz_doc::EXTENSION)).unwrap_or(file).to_string()
}

/// `path` relative to `root`, `/`-separated.
pub fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether an asset in `folder` is part of the Cast, which an arrange never moves.
pub fn is_cast(folder: &str) -> bool {
    folder == "Cast" || folder.starts_with("Cast/")
}

// ------------------------------------------------------------ embeddings

/// Embeddings as base64 of little-endian f32s: a quarter of the JSON text.
mod b64_f32 {
    use base64::prelude::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[f32], s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        s.serialize_str(&BASE64_STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = BASE64_STANDARD.decode(text).map_err(serde::de::Error::custom)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }
}

/// Cosine of two embeddings (they are L2-normalised, but not trusted to be).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na <= 0.0 || nb <= 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// A transparent thumbnail laid over a flat colour, for SigLIP (which drops
/// alpha, so transparent pixels would read as black) and the VLM.
pub fn flatten_on(img: &image::RgbaImage, bg: [u8; 3]) -> image::RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + bg[c] as u32 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
    out
}

/// A thumbnail with almost no ink (under 0.5 % of its pixels drawn), or a
/// solid box: one colour over 90 % of its opaque pixels and the drawn pixels
/// filling at least 95 % of their bounding box. Such assets are filed under
/// [`CHECK_FOLDER`] for a person to look at, not under a Type.
pub fn thumb_is_blank(img: &image::RgbaImage) -> bool {
    let total = (img.width() as usize * img.height() as usize).max(1);
    let (mut ink, mut x0, mut y0, mut x1, mut y1) = (0usize, u32::MAX, u32::MAX, 0u32, 0u32);
    let mut colours: HashMap<(u8, u8, u8), usize> = HashMap::new();
    let mut solid = 0usize;
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 8 {
            ink += 1;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
        if p[3] > 128 {
            solid += 1;
            *colours.entry((p[0] >> 4, p[1] >> 4, p[2] >> 4)).or_default() += 1;
        }
    }
    if ink * 200 < total {
        return true;
    }
    let dominant = colours.values().copied().max().unwrap_or(0);
    let box_area = (x1 - x0) as usize * (y1 - y0) as usize;
    solid > 0 && dominant * 10 >= solid * 9 && ink * 100 >= box_area * 95
}

/// The fraction of a thumbnail's pixels with any ink (alpha over 8).
pub fn thumb_ink(img: &image::RgbaImage) -> f64 {
    let total = (img.width() as usize * img.height() as usize).max(1);
    img.pixels().filter(|p| p[3] > 8).count() as f64 / total as f64
}

/// A thumbnail too empty to show anything (the ink half of [`thumb_is_blank`]).
pub fn thumb_is_empty(img: &image::RgbaImage) -> bool {
    thumb_ink(img) * 200.0 < 1.0
}

/// How much of the box round a thumbnail's ink is opaque (alpha over 128):
/// a backdrop fills its box, a cut-out does not. 0 for an empty thumbnail.
pub fn thumb_fill(img: &image::RgbaImage) -> f32 {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut solid = 0usize;
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
        if p[3] > 128 {
            solid += 1;
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return 0.0;
    }
    let area = (x1 - x0) as usize * (y1 - y0) as usize;
    (solid as f64 / area.max(1) as f64).min(1.0) as f32
}

/// The mean colour of a thumbnail's opaque pixels (alpha over 128); black
/// for an empty one.
pub fn thumb_tone(img: &image::RgbaImage) -> [u8; 3] {
    let (mut n, mut sum) = (0u64, [0u64; 3]);
    for p in img.pixels().filter(|p| p[3] > 128) {
        n += 1;
        for c in 0..3 {
            sum[c] += p[c] as u64;
        }
    }
    if n == 0 {
        return [0; 3];
    }
    [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
}

/// AS4: **connected groups of boxes** — two boxes are in one group when they
/// overlap once each is grown by `gap / 2` (so boxes closer than `gap` join),
/// transitively. Returns a group label per box; labels are 0..n in the order
/// of each group's leftmost box (left to right).
pub fn cluster_boxes(boxes: &[[f64; 4]], gap: f64) -> Vec<usize> {
    let n = boxes.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let h = gap / 2.0;
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (boxes[i], boxes[j]);
            let touch = a[0] - h <= b[2] + h && b[0] - h <= a[2] + h && a[1] - h <= b[3] + h && b[1] - h <= a[3] + h;
            if touch {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                if ri != rj {
                    parent[rj] = ri;
                }
            }
        }
    }
    let roots: Vec<usize> = (0..n).map(|i| root(&mut parent, i)).collect();
    let mut left: HashMap<usize, f64> = HashMap::new();
    for i in 0..n {
        let e = left.entry(roots[i]).or_insert(f64::INFINITY);
        *e = e.min(boxes[i][0]);
    }
    let mut order: Vec<(usize, f64)> = left.into_iter().collect();
    order.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let label: HashMap<usize, usize> = order.iter().enumerate().map(|(l, (r, _))| (*r, l)).collect();
    roots.iter().map(|r| label[r]).collect()
}

/// What a name says **confidently**: a kind keyword hit (`palm1Anim` → tree,
/// `bushesLatest` → bush, `riverWave` → ground). `None` when the name names
/// no kind. A blank thumbnail (an animation whose first frame is empty)
/// yields to this rather than going to [`CHECK_FOLDER`].
pub fn confident_name_guess(name: &str) -> Option<Guess> {
    let g = guess_from_name(name);
    (g.kind != AssetKind::Other).then_some(g)
}

// ------------------------------------------------------------ the VLM

/// C8's schema with `kind` added.
pub fn index_schema() -> serde_json::Value {
    let kinds: Vec<&str> = AssetKind::ALL.iter().map(|k| k.as_str()).collect();
    serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "category": {
                    "type": "string",
                    "enum": ["character", "prop", "background", "vehicle", "animal", "effect", "other"]
                },
                "kind": { "type": "string", "enum": kinds },
                "view": { "type": "string", "enum": ["front", "side", "back", "3q", "na"] },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["id", "name", "category", "kind", "view", "tags"],
            "additionalProperties": false
        }
    })
}

/// One answer of the VLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifiedKind {
    pub id: String,
    pub name: String,
    pub category: AssetCategory,
    pub kind: AssetKind,
    pub view: AssetView,
    pub tags: Vec<String>,
}

/// One VLM item: an id, the current name, and the thumbnail PNG.
pub struct KindItem {
    pub id: String,
    pub old_name: String,
    pub png_bytes: Vec<u8>,
}

/// Identify up to [`VLM_BATCH`] thumbnails: C8's call with `kind`.
pub fn identify_kinds_batch(
    llm: &impl Llm,
    model: &str,
    batch: &[KindItem],
) -> Result<Vec<IdentifiedKind>, LlmError> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }
    let system = "You are a professional 2D animation asset cataloguer for an Indian cartoon series. \
Identify what each thumbnail shows. Return a JSON array with one entry per thumbnail: \
- id: the exact id given for the image \
- name: a concise, descriptive snake_case name (e.g. 'old_man_with_stick_side', 'thatched_hut', 'banyan_tree') \
- category: one of character, prop, background, vehicle, animal, effect, other \
- kind: one of house (the outside of any home, hut, palace, temple, school, hospital or building), shop (stall, dhaba, market), \
interior (an indoor room/background: any room, corridor, classroom, bathroom, kitchen, hall, the inside of a house, tent, hospital or vehicle, a ceiling), \
wall (a wall, boundary wall, fence, gate, railing), \
structure (a pole, electric pole, lamp post, pillar, tower, well, water tank, bridge, a bare stall frame), \
tree, bush (bush, grass, plant, flowers), sky (sky, clouds, sun, moon), ground (ground, field, river, water), road (road, path, lane), \
furniture, vehicle, animal, character (one person, or one person's head/body part), group (several people together), \
effect (fire, smoke, rain, light, speed lines), other (any other object) \
- view: one of front, side, back, 3q, na \
- tags: 2-5 keyword tags (include day/night/interior when they apply). \
The current names are hints written by animators (Hindi/Hinglish words are common: ghar=house, ped=tree, almari=cupboard, \
kamra=room, andar=inside, deewar=wall, khamba=pole, kuan=well, pul=bridge; a name ending in 'In' or 'Inside' means the interior).";
    let mut user = String::from("Classify these images in order:\n");
    for (i, item) in batch.iter().enumerate() {
        user.push_str(&format!("Image {}: id='{}', current_name='{}'\n", i + 1, item.id, item.old_name));
    }
    user.push_str("\nOutput a valid JSON array according to the schema.");
    let images: Vec<&[u8]> = batch.iter().map(|i| i.png_bytes.as_slice()).collect();
    llm.chat_json(model, system, &user, &images, index_schema())
}

/// A VLM name (1–3 words) for one style cluster, from a contact sheet of up
/// to nine of its members: a plain art-style description ("Flat Colour",
/// "Ink Outline"), never subject matter. Empty when the answer names a
/// subject anyway (the caller then keeps "Style N").
pub fn name_style(
    llm: &impl Llm,
    model: &str,
    type_folder: &str,
    sheet_png: &[u8],
    count: usize,
) -> Result<String, LlmError> {
    #[derive(Deserialize)]
    struct Named {
        name: String,
    }
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"],
        "additionalProperties": false
    });
    let system = "You describe the drawing style shared by a group of 2D cartoon assets, shown together \
on one contact sheet. Reply with a plain 1-3 word description of the art style only: how the pictures \
are drawn and coloured (e.g. 'flat colour', 'ink outline', 'painted', 'sketchy line', 'soft shading', \
'silhouette', 'bold outline'). Never describe the subject matter: no objects, people, animals, places, \
times of day, themes or genres. No punctuation.";
    let user = format!(
        "This contact sheet shows {count} assets filed under '{type_folder}'. In 1-3 plain words, what is their shared art style?"
    );
    let named: Named = llm.chat_json(model, system, &user, &[sheet_png], schema)?;
    let name = clean_style_name(&named.name);
    // A subject word (a kind, or a word of the Type folder) is not a style.
    let subject = |w: &str| {
        let w = w.to_lowercase();
        let w = w.strip_suffix('s').unwrap_or(&w);
        AssetKind::ALL.iter().any(|k| k.as_str() == w)
            || type_folder
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .any(|t| !t.is_empty() && t.strip_suffix('s').unwrap_or(t) == w)
    };
    if name.split(' ').any(subject) {
        return Ok(String::new());
    }
    Ok(name)
}

/// At most three words of letters, digits and spaces, each capitalised.
pub fn clean_style_name(raw: &str) -> String {
    let words: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(3)
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect(),
                None => String::new(),
            }
        })
        .collect();
    words.join(" ")
}

// ------------------------------------------------------------ name heuristics

/// What a name alone suggests.
#[derive(Debug, Clone, PartialEq)]
pub struct Guess {
    pub category: AssetCategory,
    pub kind: AssetKind,
    pub view: AssetView,
    pub tags: Vec<String>,
}

/// Name words, split at separators, digits and lower→upper case changes.
pub fn name_tokens(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        let boundary = match prev {
            None => false,
            Some(p) => {
                !c.is_alphanumeric()
                    || (p.is_ascii_digit() != c.is_ascii_digit())
                    || (p.is_lowercase() && c.is_uppercase())
            }
        };
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if c.is_alphanumeric() {
            cur.push(c);
        }
        prev = Some(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    // Caps lock left on mid-word ("DOGnEW", "HENcAGE"): the stray lower-case
    // letter starts the next word, not ends this one.
    for k in 0..out.len().saturating_sub(1) {
        let chars: Vec<char> = out[k].chars().collect();
        let n = chars.len();
        if n >= 3
            && chars[..n - 1].iter().all(|c| c.is_uppercase())
            && chars[n - 1].is_lowercase()
            && out[k + 1].chars().all(|c| c.is_uppercase() || c.is_ascii_digit())
        {
            let moved = out[k].pop().expect("non-empty");
            out[k + 1].insert(0, moved);
        }
    }
    out.retain(|t| !t.chars().all(|c| c.is_ascii_digit()));
    out.into_iter().map(|t| t.to_lowercase()).collect()
}

/// Keyword lists in priority order (the first kind with a hit wins). A
/// leading `=` means "a whole name word only" (short words that would match
/// inside others: `man` in `mandir`); the rest also match anywhere in the name.
const KIND_KEYWORDS: &[(AssetKind, &[&str])] = &[
    (
        AssetKind::Group,
        &["group", "crowd", "bheed", "bhid", "=men", "=log", "characters", "=people", "family", "parivar", "भीड़", "समूह"],
    ),
    // Insides before houses and vehicles ("HospitalIn", "CarInterior"); a
    // bare "In"/"Ins" word turns a house or shop into its interior below.
    (
        AssetKind::Interior,
        &[
            "interior", "inside", "=andar", "bhitar", "bheetar", "room", "romm", "kamra", "corridor",
            "coridoor", "corridoor", "classroom", "classromm", "bathroom", "toilet", "kitchen", "rasoi", "ceiling",
            "=hall", "gallary", "gallery", "कमरा", "अंदर",
        ],
    ),
    (
        AssetKind::Wall,
        &[
            "=wall", "walls", "deewar", "deewaar", "diwar", "boundary", "boundry", "fence", "=baad", "=baadh", "gate",
            "railing", "grill", "jangla", "दीवार",
        ],
    ),
    (
        AssetKind::Structure,
        &[
            "=pole", "poles", "electricpole", "lamppost", "lamp post", "khamba", "khambha", "pillar", "tower", "=well",
            "kuan", "kuaan", "=tank", "tanki", "watertank", "bridge", "=pul", "puliya", "stallframe", "stall frame",
            "खंभा", "कुआं", "पुल",
        ],
    ),
    (
        AssetKind::Effect,
        &[
            "fire", "=aag", "smoke", "dhuan", "dhuaan", "=rain", "raindrop", "barish", "baarish", "drops", "splash", "spark", "flash",
            "speedline", "speed line", "blood", "=fog", "mist", "=wind", "windblow", "=hawa", "lightning", "bijli", "firefly",
            "jugnoo", "explosion", "dust", "glow", "magic", "flame", "आग", "धुआं", "बारिश",
        ],
    ),
    (
        AssetKind::Vehicle,
        &[
            "=car", "=cars", "=bus", "truck", "train", "engine", "=van", "=auto", "rikshaw", "rickshaw", "cycle",
            "=bike", "scooter", "ambulance", "bulldozer", "tempo", "=cart", "cartfix", "thela", "chariot", "charriot",
            "=tractor", "=jeep", "vehicle", "vehhice", "=boat", "=nav", "plane", "jahaj", "gaadi", "gadi", "गाड़ी",
        ],
    ),
    (
        AssetKind::Animal,
        &[
            "=dog", "kutta", "=cat", "billi", "=cow", "=gai", "=gaay", "=ox", "bail", "buffalo", "bhains", "goat", "bakri",
            "horse", "ghoda", "camel", "oont", "=hen", "murgi", "bird", "chidiya", "=owl", "ullu", "fish", "machli",
            "=bat", "=fly", "=flyb", "monkey", "bandar", "snake", "saanp", "elephant", "haathi", "tiger", "sher",
            "lion", "deer", "parrot", "tota", "कुत्ता", "गाय", "घोड़ा",
        ],
    ),
    (
        AssetKind::House,
        &[
            "house", "ghar", "home", "=hut", "huts", "jhopdi", "jhopri", "haveli", "mahal", "mehal", "palace", "fort",
            "mandir", "temple", "building", "hospital", "school", "class", "=bank", "bank1", "postoffice", "post office",
            "khandahar", "ruins", "=dalaan", "court", "garage", "=cabin", "woodcabin", "tent", "quarter", "roof",
            "entrance", "घर", "मकान", "मंदिर",
        ],
    ),
    (
        AssetKind::Shop,
        &["shop", "dukan", "dukaan", "dhaba", "stall", "tapri", "haat", "market", "bazaar", "bazar", "booth", "दुकान"],
    ),
    (
        AssetKind::Furniture,
        &[
            "almari", "almira", "amlari", "cupboard", "cabinet", "table", "tabel", "chair", "kursi", "=bed", "bedside",
            "bedsheet", "=khat", "khatiya", "charpai", "=cot", "sofa", "bench", "throne", "cradle", "palna", "sandook",
            "shelf", "stool", "desk", "machan", "parda", "curtain", "windo", "window", "अलमारी", "कुर्सी",
        ],
    ),
    (
        AssetKind::Character,
        &[
            "=man", "woman", "aadmi", "aurat", "girl", "ladki", "=boy", "ladka", "baby", "=kid", "child", "bachcha",
            "baccha", "budha", "budhiya", "=old", "oldman", "dada", "dadi", "=nana", "=nani", "baba", "sadhu", "pujari",
            "preist", "priest", "police", "soldier", "sipahi", "raja", "rani", "rajkumar", "king", "queen", "witch",
            "chudail", "ghost", "bhoot", "demon", "shaitaan", "shaitan", "pisach", "daku", "thakur", "doctor", "nurse",
            "conductor", "postman", "wife", "husband", "father", "mother", "body", "head", "face", "mouth", "=eye",
            "eyes", "=hand", "handset", "handsset", "lips", "hair", "moustache", "mooch", "skeleton", "kankaal", "mantri", "pradhan",
            "majdoor", "mazdoor", "chaiwal", "angrez", "british", "lathait", "farmer", "kisan", "officer", "guard",
            "worker", "=walk", "walking", "torso", "teacher", "student", "contractor", "thekedar", "murtikaar", "wardboy",
            "bhagat", "maharaj", "person", "आदमी", "औरत", "लड़की", "लड़का",
        ],
    ),
    (
        AssetKind::Tree,
        &[
            "tree", "=ped", "=pedh", "palm", "plam", "bamboo", "banana", "kela", "coconut", "cocnut", "nariyal",
            "peepal", "peeple", "banyan", "bargad", "neem", "trunk", "jungle", "forest", "=van", "पेड़",
        ],
    ),
    (
        AssetKind::Bush,
        &[
            "bush", "shrub", "jhadi", "jhaadi", "grass", "gras", "ghas", "ghaas", "plant", "=leaf", "leaves", "flower",
            "phool", "=lata", "tulsi", "crop", "fasal", "झाड़ी", "घास",
        ],
    ),
    (
        AssetKind::Sky,
        &[
            "sky", "aasmaan", "asmaan", "aakash", "akash", "cloud", "badal", "=moon", "chand", "=sun", "sunset",
            "sunred", "suraj", "=star", "tara", "आसमान", "बादल",
        ],
    ),
    (
        AssetKind::Road,
        &[
            "road", "sadak", "=path", "path2", "=way", "wayfront", "straightway", "lane", "=gali", "street", "highway",
            "diversion", "cliffroad", "सड़क",
        ],
    ),
    (
        AssetKind::Ground,
        &[
            "ground", "zameen", "field", "khet", "=land", "landground", "river", "nadi", "naadi", "nahar", "lake",
            "talab", "pond", "water", "cliff", "mountain", "pahad", "hill", "platform", "floor", "farsh", "drought",
            "pitgaddha", "ghat", "dumpyard", "baseplatform", "ज़मीन", "नदी",
        ],
    ),
];

/// Objects with no finer kind: category `prop`, kind `other`.
const PROP_KEYWORDS: &[&str] = &[
    "=bag", "jhola", "bottle", "=cup", "glass", "=lota", "=pot", "pots", "matka", "bowl", "thali", "kettle", "lamp",
    "lantern", "lalten", "bulb", "torch", "mashaal", "mashal", "candle", "diya", "=deep", "phone", "mobile", "iphone",
    "cell", "book", "diary", "clock", "watch", "umbrella", "stick", "lathi", "rifle", "=gun", "sword", "talwar",
    "money", "coin", "sikka", "basket", "=box", "tiffin", "chulha", "stove", "kadhai", "=tv", "=fan", "speaker",
    "speraker", "=mala", "agarbatti", "=jar", "vase", "sack", "bori", "hukka", "chasma", "glasses", "pagdi",
    "turban", "roti", "chawal", "daal", "aalu", "jalebi", "chips", "=key", "rope", "tool", "kudal", "shovel",
    "spatula", "splatula", "transformer", "pole", "post", "brick", "block", "stone", "pathar", "potli", "feather",
    "handle", "bell", "arthi", "pyre", "telephone", "walkie", "gutkha", "kanche", "tube", "soop", "dates", "nariyal",
    "chabutra", "emergency", "decor", "light", "machine", "thaila", "skull", "cage", "shoe",
];

fn keyword_hit(keyword: &str, tokens: &[String], whole: &str) -> bool {
    if let Some(word) = keyword.strip_prefix('=') {
        tokens.iter().any(|t| t == word || t.strip_suffix('s') == Some(word))
    } else {
        whole.contains(keyword)
    }
}

/// What a name (and its folder) suggests, for when the VLM is down.
pub fn guess_from_name(name: &str) -> Guess {
    let tokens = name_tokens(name);
    let whole_raw: String = name.to_lowercase();
    // "background" is a category, not a kind: take it out so its "ground"
    // does not file every background under Ground & Roads.
    let mut whole = whole_raw.replace("background", " ").replace("nackground", " ");
    whole.retain(|c| c != '_' && c != '-');
    let tokens: Vec<String> = tokens
        .into_iter()
        .filter(|t| t != "background" && t != "nackground")
        .collect();
    let is_background = whole_raw.contains("background") || whole_raw.contains("nackground");

    let mut kind = AssetKind::Other;
    let mut tags: Vec<String> = Vec::new();
    for (k, words) in KIND_KEYWORDS {
        if let Some(word) = words.iter().find(|w| keyword_hit(w, &tokens, &whole)) {
            kind = *k;
            tags.push(word.trim_start_matches('=').to_string());
            break;
        }
    }
    // A palm with a hand, a set or a mala in the name is a hand, not a tree.
    if kind == AssetKind::Tree
        && whole.contains("palm")
        && !whole.contains("tree")
        && (whole.contains("hand") || whole.contains("mala") || whole.contains("set") || whole == "palm")
    {
        kind = AssetKind::Character;
        tags.push("hand".into());
    }
    // "HaveliIn1", "inHouse", "housesIns": the inside of the building.
    if matches!(kind, AssetKind::House | AssetKind::Shop) && tokens.iter().any(|t| t == "in" || t == "ins") {
        kind = AssetKind::Interior;
    }
    let prop = kind == AssetKind::Other && PROP_KEYWORDS.iter().any(|w| keyword_hit(w, &tokens, &whole));

    let category = match kind {
        AssetKind::Character | AssetKind::Group => AssetCategory::Character,
        AssetKind::Animal => AssetCategory::Animal,
        AssetKind::Vehicle => AssetCategory::Vehicle,
        AssetKind::Effect => AssetCategory::Effect,
        AssetKind::House
        | AssetKind::Shop
        | AssetKind::Interior
        | AssetKind::Wall
        | AssetKind::Structure
        | AssetKind::Sky
        | AssetKind::Ground
        | AssetKind::Road => {
            AssetCategory::Background
        }
        AssetKind::Tree | AssetKind::Bush | AssetKind::Furniture => AssetCategory::Prop,
        AssetKind::Other if prop => AssetCategory::Prop,
        AssetKind::Other if is_background || whole.contains("scene") => AssetCategory::Background,
        AssetKind::Other => AssetCategory::Other,
    };

    let has = |w: &str| tokens.iter().any(|t| t == w) || whole.contains(w);
    let view = if !matches!(kind, AssetKind::Character | AssetKind::Animal | AssetKind::Vehicle | AssetKind::Group) {
        AssetView::Na
    } else if whole.contains("34th") || whole.contains("3-4") || whole.contains("3q") || has("threequarter") {
        AssetView::ThreeQuarter
    } else if has("side") {
        AssetView::Side
    } else if has("back") || has("behind") {
        AssetView::Back
    } else if has("front") || has("straight") {
        AssetView::Front
    } else {
        AssetView::Na
    };

    for (word, tag) in [
        ("night", "night"),
        ("evening", "evening"),
        ("eveming", "evening"),
        ("day", "day"),
        ("village", "village"),
        ("city", "city"),
        ("dark", "dark"),
        ("anim", "animated"),
        ("swing", "animated"),
        ("moving", "animated"),
        ("scene", "scene"),
    ] {
        if whole.contains(word) && !tags.iter().any(|t| t == tag) {
            tags.push(tag.into());
        }
    }
    if tokens.iter().any(|t| t == "in" || t == "inside" || t == "interior" || t == "ins")
        || whole.contains("room")
        || whole.contains("corrid")
    {
        tags.push("interior".into());
    }
    if is_background {
        tags.push("background".into());
    }
    Guess { category, kind, view, tags }
}

// ------------------------------------------------------------ style clusters

/// Average-linkage agglomerative clustering on cosine similarity: pairs of
/// clusters merge while their mean pairwise cosine is at least `threshold`.
/// Returns one cluster label per item (labels are 0..n_clusters, the largest
/// cluster first).
pub fn cluster_styles(embeddings: &[&[f32]], threshold: f32) -> Vec<usize> {
    cluster_styles_capped(embeddings, threshold, usize::MAX, 1)
}

/// [`cluster_styles`], then the nearest clusters keep merging (past
/// `threshold`) while more than `max_big` clusters have `min_size` members or
/// more. Outliers, the last to merge, are left in small clusters.
pub fn cluster_styles_capped(embeddings: &[&[f32]], threshold: f32, max_big: usize, min_size: usize) -> Vec<usize> {
    let n = embeddings.len();
    if n == 0 {
        return Vec::new();
    }
    let mut sim = vec![0.0f32; n * n];
    for i in 0..n {
        for j in i + 1..n {
            let s = cosine(embeddings[i], embeddings[j]);
            sim[i * n + j] = s;
            sim[j * n + i] = s;
        }
    }
    let mut size = vec![1usize; n];
    let mut active = vec![true; n];
    let mut parent: Vec<usize> = (0..n).collect();
    let mut big = if min_size <= 1 { n } else { 0 };
    loop {
        let mut best = (f32::NEG_INFINITY, 0, 0);
        for i in 0..n {
            if !active[i] {
                continue;
            }
            let row = &sim[i * n..i * n + n];
            for j in i + 1..n {
                if active[j] && row[j] > best.0 {
                    best = (row[j], i, j);
                }
            }
        }
        let (s, i, j) = best;
        if !s.is_finite() || (s < threshold && big <= max_big) {
            break;
        }
        big = big + usize::from(size[i] + size[j] >= min_size)
            - usize::from(size[i] >= min_size)
            - usize::from(size[j] >= min_size);
        // Merge j into i.
        let (si, sj) = (size[i] as f32, size[j] as f32);
        for k in 0..n {
            if !active[k] || k == i || k == j {
                continue;
            }
            let v = (si * sim[i * n + k] + sj * sim[j * n + k]) / (si + sj);
            sim[i * n + k] = v;
            sim[k * n + i] = v;
        }
        size[i] += size[j];
        active[j] = false;
        parent[j] = i;
    }
    let root = |mut x: usize| {
        while parent[x] != x {
            x = parent[x];
        }
        x
    };
    let roots: Vec<usize> = (0..n).map(root).collect();
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for r in &roots {
        *counts.entry(*r).or_default() += 1;
    }
    let mut order: Vec<(usize, usize)> = counts.into_iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let label: HashMap<usize, usize> = order.iter().enumerate().map(|(l, (r, _))| (*r, l)).collect();
    roots.iter().map(|r| label[r]).collect()
}

/// A stable key for one style cluster: its Type and its members.
pub fn style_key(type_folder: &str, members: &[&str]) -> String {
    let mut sorted: Vec<&str> = members.to_vec();
    sorted.sort_unstable();
    // FNV-1a, so the key is the same on every run and every Rust version.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for m in sorted {
        for b in m.bytes().chain(std::iter::once(0)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    // `v2`: names asked with the AS2b art-style prompt; the older,
    // subject-matter names ("Lantern Props") no longer match any cluster.
    format!("{type_folder}|v2|{h:016x}")
}

// ------------------------------------------------------------ the arrange plan

/// One proposed move.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangeMove {
    /// The asset, relative to the library root.
    pub path: String,
    pub name: String,
    /// Where it is now.
    pub from: String,
    /// Where it would go: `Type` or `Type/Style`.
    pub to: String,
    pub type_folder: String,
    /// Empty when it goes straight into the Type folder.
    pub style: String,
    /// Thumbnail, relative to the library root (empty when not indexed).
    pub thumb: String,
    /// True when the kind came from name heuristics (or the asset is not
    /// indexed yet).
    pub guessed: bool,
    pub selected: bool,
}

/// One style cluster of the plan (for naming it).
#[derive(Debug, Clone, PartialEq)]
pub struct StyleCluster {
    pub key: String,
    pub type_folder: String,
    /// Relative paths of the members.
    pub members: Vec<String>,
    /// The name the plan used ("Style N" until the VLM names it).
    pub name: String,
}

/// The whole plan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArrangePlan {
    /// Moves, ordered by destination then name. Assets already where the
    /// plan would put them are left out.
    pub moves: Vec<ArrangeMove>,
    pub clusters: Vec<StyleCluster>,
    /// Assets the plan leaves alone because they are in `Cast/`.
    pub cast_kept: usize,
    /// Assets with no index record (filed by name only, no style).
    pub unindexed: usize,
}

impl ArrangePlan {
    /// Count of planned assets per destination folder (moves only).
    pub fn counts(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for m in &self.moves {
            *out.entry(m.to.clone()).or_insert(0) += 1;
        }
        out
    }
}

/// The plan for `assets` (relative path, name, folder) from `index`.
pub fn plan_arrange(index: &AssetIndex, assets: &[(String, String, String)], threshold: f32) -> ArrangePlan {
    let mut plan = ArrangePlan::default();
    // (path, name, folder, type, embedding, thumb, guessed)
    struct Item<'a> {
        path: &'a str,
        name: &'a str,
        folder: &'a str,
        type_folder: &'static str,
        embedding: &'a [f32],
        thumb: &'a str,
        guessed: bool,
    }
    let mut by_type: BTreeMap<&'static str, Vec<Item>> = BTreeMap::new();
    for (path, name, folder) in assets {
        if is_cast(folder) {
            plan.cast_kept += 1;
            continue;
        }
        let item = match index.asset_record(path) {
            Some(r) => Item {
                path,
                name,
                folder,
                // A blank thumbnail goes to Check, unless the name says
                // confidently what it is (an animation with an empty frame 0).
                type_folder: match (r.blank == Some(true), confident_name_guess(name)) {
                    (true, Some(g)) => type_folder(g.kind, g.category),
                    (true, None) => CHECK_FOLDER,
                    (false, _) => type_folder(r.kind, r.category),
                },
                embedding: &r.embedding,
                thumb: &r.thumb,
                guessed: r.guessed,
            },
            None => {
                plan.unindexed += 1;
                let g = guess_from_name(name);
                Item {
                    path,
                    name,
                    folder,
                    type_folder: type_folder(g.kind, g.category),
                    embedding: &[],
                    thumb: "",
                    guessed: true,
                }
            }
        };
        by_type.entry(item.type_folder).or_default().push(item);
    }

    for (ty, items) in by_type {
        // What needs a look gets no Style folders.
        let with_emb: Vec<usize> = (0..items.len())
            .filter(|&i| ty != CHECK_FOLDER && !items[i].embedding.is_empty())
            .collect();
        let embs: Vec<&[f32]> = with_emb.iter().map(|&i| items[i].embedding).collect();
        let labels = cluster_styles_capped(&embs, threshold, MAX_STYLES, MIN_STYLE_SIZE);
        let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (k, &l) in labels.iter().enumerate() {
            members.entry(l).or_default().push(with_emb[k]);
        }
        let big: Vec<(usize, Vec<usize>)> = members
            .into_iter()
            .filter(|(_, m)| m.len() >= MIN_STYLE_SIZE)
            .collect();
        let mut style_of: HashMap<usize, String> = HashMap::new();
        // Only worth sub-folders when the Type splits into two styles or more.
        if big.len() >= 2 {
            let mut used: Vec<String> = Vec::new();
            for (n, (_, m)) in big.iter().enumerate() {
                let paths: Vec<&str> = m.iter().map(|&i| items[i].path).collect();
                let key = style_key(ty, &paths);
                let mut name = index
                    .style_names
                    .get(&key)
                    .cloned()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| format!("Style {}", n + 1));
                if used.contains(&name) {
                    let base = name.clone();
                    let mut k = 2;
                    while used.contains(&name) {
                        name = format!("{base} {k}");
                        k += 1;
                    }
                }
                used.push(name.clone());
                for &i in m {
                    style_of.insert(i, name.clone());
                }
                plan.clusters.push(StyleCluster {
                    key,
                    type_folder: ty.to_string(),
                    members: paths.iter().map(|p| p.to_string()).collect(),
                    name,
                });
            }
        }
        for (i, item) in items.iter().enumerate() {
            let style = style_of.get(&i).cloned().unwrap_or_default();
            let to = if style.is_empty() { ty.to_string() } else { format!("{ty}/{style}") };
            if to == item.folder {
                continue;
            }
            plan.moves.push(ArrangeMove {
                path: item.path.to_string(),
                name: item.name.to_string(),
                from: item.folder.to_string(),
                to,
                type_folder: ty.to_string(),
                style,
                thumb: item.thumb.to_string(),
                guessed: item.guessed,
                selected: true,
            });
        }
    }
    plan.moves.sort_by(|a, b| a.to.cmp(&b.to).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    plan
}

// ------------------------------------------------------------ apply / undo

/// One file moved by an arrange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedFile {
    /// Relative path before.
    pub from: String,
    /// Relative path after.
    pub to: String,
}

/// One applied arrange.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveLog {
    /// Seconds since the epoch.
    pub when: u64,
    pub moved: Vec<MovedFile>,
    /// Folders the arrange made (relative), so Undo can remove them when empty.
    pub created_folders: Vec<String>,
}

/// Every applied arrange, oldest first; Undo takes the last.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveLogs {
    pub logs: Vec<MoveLog>,
}

impl MoveLogs {
    pub fn path_in(root: &Path) -> PathBuf {
        root.join(INDEX_DIR).join(MOVE_LOG_FILE)
    }

    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(Self::path_in(root))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = Self::path_in(root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }
}

/// Whether there is an arrange to undo in `root`.
pub fn can_undo_arrange(root: &Path) -> bool {
    !MoveLogs::load(root).logs.is_empty()
}

/// **Apply** the selected moves with `AssetLibrary::move_to_folder`, writing
/// the move log as it goes (so a cut-off still leaves an undoable log), then
/// moving the index records along. `Cast/` is refused whatever the plan says.
/// Returns the log of what moved.
pub fn apply_arrange(
    library: &mut buzz_doc::AssetLibrary,
    moves: &[ArrangeMove],
    mut progress: impl FnMut(usize, usize),
) -> Result<MoveLog, String> {
    let root = library.root().map(Path::to_path_buf).ok_or("no assets folder")?;
    let existing: std::collections::HashSet<String> = library.folders().iter().cloned().collect();
    let mut logs = MoveLogs::load(&root);
    logs.logs.push(MoveLog {
        when: now_secs(),
        ..Default::default()
    });
    let selected: Vec<&ArrangeMove> = moves.iter().filter(|m| m.selected).collect();
    let total = selected.len();
    let mut errors = Vec::new();
    for (n, m) in selected.iter().enumerate() {
        progress(n, total);
        if is_cast(&m.from) || is_cast(&m.to) {
            continue;
        }
        let Some(asset) = library
            .assets()
            .iter()
            .find(|a| relative(&root, &a.path) == m.path)
            .cloned()
        else {
            errors.push(format!("{}: no longer there", m.path));
            continue;
        };
        let name = library.unique_name(&asset.name, &m.to);
        let target = format!("{}/{}.{}", m.to, name, buzz_doc::EXTENSION);
        if let Err(e) = library.move_to_folder(&asset, &m.to) {
            errors.push(format!("{}: {e}", m.path));
            continue;
        }
        let log = logs.logs.last_mut().expect("pushed above");
        log.moved.push(MovedFile { from: m.path.clone(), to: target });
        // Every folder on the way that was not there before.
        let mut acc = String::new();
        for part in m.to.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            if !existing.contains(&acc) && !log.created_folders.contains(&acc) {
                log.created_folders.push(acc.clone());
            }
        }
        logs.save(&root).map_err(|e| e.to_string())?;
    }
    progress(total, total);
    let log = logs.logs.last().cloned().unwrap_or_default();
    if log.moved.is_empty() {
        logs.logs.pop();
    }
    logs.save(&root).map_err(|e| e.to_string())?;
    let mut index = AssetIndex::load(&root);
    if !index.records.is_empty() {
        let map: HashMap<String, String> = log.moved.iter().map(|m| (m.from.clone(), m.to.clone())).collect();
        index.follow_moves(&map);
        index.save(&root).map_err(|e| e.to_string())?;
    }
    if !errors.is_empty() && log.moved.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(log)
}

/// **Undo arrange**: put every file of the last arrange back where it was
/// (same name), remove the folders it made if they are empty, and drop the
/// log. Returns how many files went back.
pub fn undo_arrange(library: &mut buzz_doc::AssetLibrary) -> Result<usize, String> {
    let root = library.root().map(Path::to_path_buf).ok_or("no assets folder")?;
    let mut logs = MoveLogs::load(&root);
    let Some(log) = logs.logs.pop() else {
        return Err("there is no arrange to undo".into());
    };
    let mut back = 0;
    let mut restored: HashMap<String, String> = HashMap::new();
    let mut errors = Vec::new();
    for m in log.moved.iter().rev() {
        let Some(asset) = library
            .assets()
            .iter()
            .find(|a| relative(&root, &a.path) == m.to)
            .cloned()
        else {
            errors.push(format!("{}: no longer there", m.to));
            continue;
        };
        let from_folder = folder_of(&m.from);
        let from_name = stem_of(&m.from);
        if let Err(e) = library.move_to_folder(&asset, &from_folder) {
            errors.push(format!("{}: {e}", m.to));
            continue;
        }
        // A name stepped past a clash on the way out comes back as it was.
        let landed = library
            .assets()
            .iter()
            .find(|a| a.folder == from_folder && a.name == asset.name)
            .cloned();
        if let Some(landed) = landed
            && landed.name != from_name
            && !library.assets().iter().any(|a| a.folder == from_folder && a.name == from_name)
        {
            let _ = library.rename(&landed, &from_name);
        }
        restored.insert(m.to.clone(), m.from.clone());
        back += 1;
    }
    // Deepest first, and only when empty (`remove_dir` refuses otherwise).
    let mut folders = log.created_folders.clone();
    folders.sort_by_key(|f| std::cmp::Reverse(f.matches('/').count()));
    for f in folders {
        let _ = std::fs::remove_dir(root.join(f.replace('/', std::path::MAIN_SEPARATOR_STR)));
    }
    library.rescan();
    logs.save(&root).map_err(|e| e.to_string())?;
    let mut index = AssetIndex::load(&root);
    if !index.records.is_empty() {
        index.follow_moves(&restored);
        index.save(&root).map_err(|e| e.to_string())?;
    }
    if back == 0 && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(back)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
