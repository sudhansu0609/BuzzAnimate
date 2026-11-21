//! **The Rig panel's slots, kept** (user 2026-11-21: "when I rig a character,
//! the rig stays there, be it body or head; it shall not reset"):
//! `rig_slots.json` beside the workspace file (as `rig_parts.json` is), one
//! entry per film and per timeline — the body's, each head's — holding the
//! template and the drawing put in each slot. A restart, the film opened
//! again, or a trip out to the stage and back finds the slots as they were
//! left. A missing or unreadable file is no slots.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One timeline's slots: the template and, per slot, the drawing's object id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub pattern: usize,
    pub slots: Vec<Option<u64>>,
}

/// Film path → timeline (`""` the stage, else the open symbols' names joined
/// by `/`) → its slots.
type Store = BTreeMap<String, BTreeMap<String, Kept>>;

/// Where they live: `rig_slots.json` beside `workspace.json`; a test's
/// scratch workspace gets a file of its own named for it.
pub fn path() -> PathBuf {
    let workspace = buzz_ui::workspace::workspace_path();
    match workspace.file_stem().and_then(|s| s.to_str()) {
        Some("workspace") | None => workspace.with_file_name("rig_slots.json"),
        Some(stem) => workspace.with_file_name(format!("{stem}-rig_slots.json")),
    }
}

fn load_from(file: &Path) -> Store {
    std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The slots kept for `timeline` of the film at `film`.
pub fn get(film: &Path, timeline: &str) -> Option<Kept> {
    get_in(&path(), film, timeline)
}

/// Keep `kept` for `timeline` of `film`; `None`, or no slot filled, forgets it.
pub fn put(film: &Path, timeline: &str, kept: Option<&Kept>) -> std::io::Result<()> {
    put_in(&path(), film, timeline, kept)
}

fn get_in(file: &Path, film: &Path, timeline: &str) -> Option<Kept> {
    load_from(file).get(&film.to_string_lossy().to_string())?.get(timeline).cloned()
}

fn put_in(file: &Path, film: &Path, timeline: &str, kept: Option<&Kept>) -> std::io::Result<()> {
    let mut store = load_from(file);
    let key = film.to_string_lossy().to_string();
    match kept.filter(|k| k.slots.iter().any(Option::is_some)) {
        Some(k) => {
            store.entry(key).or_default().insert(timeline.to_string(), k.clone());
        }
        None => {
            if let Some(film) = store.get_mut(&key) {
                film.remove(timeline);
                if film.is_empty() {
                    store.remove(&key);
                }
            }
        }
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, serde_json::to_string_pretty(&store).map_err(std::io::Error::other)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_kept_per_film_and_timeline_and_an_empty_list_is_forgotten() {
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("rig_slots.json");
        let film = Path::new("/films/a.buzz");
        let body = Kept { pattern: 1, slots: vec![Some(7), None, Some(9)] };
        let head = Kept { pattern: 4, slots: vec![Some(11)] };
        put_in(&file, film, "Raju", Some(&body)).unwrap();
        put_in(&file, film, "Raju/Head", Some(&head)).unwrap();
        assert_eq!(get_in(&file, film, "Raju"), Some(body));
        assert_eq!(get_in(&file, film, "Raju/Head"), Some(head));
        assert_eq!(get_in(&file, Path::new("/films/b.buzz"), "Raju"), None, "another film's are its own");
        put_in(&file, film, "Raju", Some(&Kept { pattern: 1, slots: vec![None] })).unwrap();
        assert_eq!(get_in(&file, film, "Raju"), None);
        assert!(get_in(&file, film, "Raju/Head").is_some());
    }
}
