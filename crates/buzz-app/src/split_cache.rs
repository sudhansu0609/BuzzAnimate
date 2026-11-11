//! **Same picture, same layers** (SPLIT_PICTURE_PLAN S11): the object-detection
//! cache and the run record of Split Picture into Layers.
//!
//! The VLM's object list is the only part of a split that can differ between two
//! runs on the same picture. It is asked at temperature 0 with a fixed seed, and
//! the final list (after the whole-picture and half-picture passes are unioned,
//! before SAM 2) is saved in `<user data>/BuzzAnimate/split_cache/<n>.json` with a
//! 64x36 thumbnail of the picture the VLM saw. A later split of the same picture
//! (thumbnail within [`THUMB_MAX_DIFF`], same model, same prompt version) reuses
//! the list and skips the VLM; renders are not bit-stable, hence the tolerance.
//!
//! Every split also writes `<user data>/BuzzAnimate/split_runs/<yyyymmdd-hhmmss>/`
//! (UTC; newest [`KEEP_RUNS`] kept): `input.png` (what the VLM saw), `objects.json`,
//! `result.json`, so a run can be reproduced.

use std::path::{Path, PathBuf};

use buzz_ai::scene_builder::DetectedSceneObject;
use image::RgbaImage;
use serde::{Deserialize, Serialize};

/// The sampling temperature Split Picture asks the VLM with.
pub const TEMPERATURE: f64 = 0.0;
/// The fixed sampling seed Split Picture asks the VLM with.
pub const SEED: u64 = 20_261_104;
/// The thumbnail the saved list is matched on.
const THUMB_W: u32 = 64;
const THUMB_H: u32 = 36;
/// Largest mean absolute difference (of 255, per channel value) for two thumbnails to be the same picture.
pub const THUMB_MAX_DIFF: f64 = 3.0;
/// How many run records are kept.
pub const KEEP_RUNS: usize = 20;
/// The cache file format.
const CACHE_VERSION: u32 = 1;

/// `<user data>/BuzzAnimate` (`%APPDATA%` on Windows).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate")
}

/// Where the saved object lists live.
pub fn cache_dir() -> PathBuf {
    data_dir().join("split_cache")
}

/// Where the run records live.
pub fn runs_dir() -> PathBuf {
    data_dir().join("split_runs")
}

// ------------------------------------------------------------------ detection cache

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    model: String,
    prompt_version: u32,
    small_objects: bool,
    /// The picture the list was made for, in pixels.
    picture: [u32; 2],
    /// `THUMB_W` x `THUMB_H` RGB, hex.
    thumb: String,
    /// The final VLM list, normalised boxes, before SAM 2 and the filters.
    objects: Vec<DetectedSceneObject>,
    ground_y: f64,
    saved: String,
}

/// A saved object list that matches a picture.
pub struct Saved {
    /// The cache file it came from.
    pub path: PathBuf,
    pub objects: Vec<DetectedSceneObject>,
    pub ground_y: f64,
}

/// The picture as a 64x36 RGB thumbnail.
fn thumbnail(picture: &RgbaImage) -> Vec<u8> {
    let small = image::imageops::resize(picture, THUMB_W, THUMB_H, image::imageops::FilterType::Triangle);
    small.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect()
}

/// Mean absolute difference of two thumbnails (0..255); infinite when they differ in size.
fn thumb_diff(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return f64::INFINITY;
    }
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.len() as f64
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || text.len() % 2 != 0 {
        return None;
    }
    (0..text.len() / 2).map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()).collect()
}

/// The saved list for `picture` made by `model` with this prompt version, if any: the
/// closest thumbnail within [`THUMB_MAX_DIFF`] (the lowest file name on a tie).
pub fn find(picture: &RgbaImage, model: &str, small_objects: bool) -> Option<Saved> {
    let thumb = thumbnail(picture);
    let mut files: Vec<PathBuf> = std::fs::read_dir(cache_dir())
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut best: Option<(f64, Saved)> = None;
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(entry) = serde_json::from_str::<Entry>(&text) else { continue };
        if entry.version != CACHE_VERSION
            || entry.model != model
            || entry.prompt_version != buzz_ai::scene_builder::BACKGROUND_OBJECTS_PROMPT_VERSION
            || entry.small_objects != small_objects
        {
            continue;
        }
        let Some(saved_thumb) = from_hex(&entry.thumb) else { continue };
        let diff = thumb_diff(&thumb, &saved_thumb);
        if diff <= THUMB_MAX_DIFF && best.as_ref().is_none_or(|(d, _)| diff < *d) {
            best = Some((diff, Saved { path, objects: entry.objects, ground_y: entry.ground_y }));
        }
    }
    best.map(|(_, s)| s)
}

/// Save the final VLM list for `picture`: over `replace` (the entry a re-detect overrides)
/// when given, else as the next `<n>.json`. Returns where it went; a failure to write is not an error.
pub fn save(
    picture: &RgbaImage,
    model: &str,
    small_objects: bool,
    objects: &[DetectedSceneObject],
    ground_y: f64,
    replace: Option<&Path>,
) -> Option<PathBuf> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = match replace {
        Some(p) => p.to_path_buf(),
        None => {
            let next = std::fs::read_dir(&dir)
                .ok()?
                .filter_map(|e| e.ok())
                .filter_map(|e| e.path().file_stem()?.to_str()?.parse::<u64>().ok())
                .max()
                .map_or(1, |n| n + 1);
            dir.join(format!("{next}.json"))
        }
    };
    let entry = Entry {
        version: CACHE_VERSION,
        model: model.to_string(),
        prompt_version: buzz_ai::scene_builder::BACKGROUND_OBJECTS_PROMPT_VERSION,
        small_objects,
        picture: [picture.width(), picture.height()],
        thumb: to_hex(&thumbnail(picture)),
        objects: objects.to_vec(),
        ground_y,
        saved: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
    };
    std::fs::write(&path, serde_json::to_string(&entry).ok()?).ok()?;
    Some(path)
}

// ------------------------------------------------------------------ run record

thread_local! {
    /// The record folder the last detection on this thread made (see [`take_last_run`]).
    static LAST_RUN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Remember (or forget) the run record of the detection that just ran on this thread.
pub fn set_last_run(dir: Option<PathBuf>) {
    LAST_RUN.with(|l| *l.borrow_mut() = dir);
}

/// The run record folder of the detection that ran on this thread, once: the split
/// that called it adds its `result.json` there.
pub fn take_last_run() -> Option<PathBuf> {
    LAST_RUN.with(|l| l.borrow_mut().take())
}

/// `yyyymmdd-hhmmss` (UTC) for now.
fn run_stamp() -> String {
    // RFC 3339 "2026-11-04T10:15:00Z" -> "20261104-101500".
    let text = humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string();
    let (date, clock) = text.trim_end_matches('Z').split_once('T').unwrap_or((&text, ""));
    format!("{}-{}", date.replace('-', ""), clock.replace(':', ""))
}

/// Does `name` look like a run folder (`yyyymmdd-hhmmss`, optionally `-2`...)?
fn is_run_name(name: &str) -> bool {
    let mut parts = name.split('-');
    let digits = |s: Option<&str>, n: usize| s.is_some_and(|s| s.len() == n && s.bytes().all(|b| b.is_ascii_digit()));
    if !(digits(parts.next(), 8) && digits(parts.next(), 6)) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(extra) => parts.next().is_none() && !extra.is_empty() && extra.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// Make a new run record folder holding `input.png` (the picture the VLM saw) and keep the
/// newest [`KEEP_RUNS`]. `None` when it cannot be made; a split never fails because of its record.
pub fn start_run(input_png: &[u8]) -> Option<PathBuf> {
    let root = runs_dir();
    std::fs::create_dir_all(&root).ok()?;
    let stamp = run_stamp();
    let mut dir = root.join(&stamp);
    let mut n = 1;
    while std::fs::create_dir(&dir).is_err() {
        n += 1;
        if n > 99 {
            return None;
        }
        dir = root.join(format!("{stamp}-{n}"));
    }
    std::fs::write(dir.join("input.png"), input_png).ok()?;
    prune_runs(&root);
    Some(dir)
}

/// Delete the oldest run folders beyond [`KEEP_RUNS`] (only folders named like a run).
fn prune_runs(root: &Path) {
    let Ok(read) = std::fs::read_dir(root) else { return };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_run_name(n))
        .collect();
    names.sort();
    if names.len() > KEEP_RUNS {
        for old in &names[..names.len() - KEEP_RUNS] {
            let _ = std::fs::remove_dir_all(root.join(old));
        }
    }
}

/// Write `value` as `dir/name`, pretty. Failures are ignored.
pub fn write_json(dir: &Path, name: &str, value: &serde_json::Value) {
    if let Ok(text) = serde_json::to_string_pretty(value) {
        let _ = std::fs::write(dir.join(name), text);
    }
}
