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
        // Tests run side by side in one process: each thread its own file,
        // so one test's write never overwrites another's.
        #[cfg(test)]
        Some(stem) => workspace.with_file_name(format!("{stem}-{:?}-rig_slots.json", std::thread::current().id()).replace(['(', ')'], "")),
        #[cfg(not(test))]
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

/// **A duplicated symbol keeps its Rig panel tags.** Every timeline of
/// `film` kept under a path through `from` (`Raju`, `Raju/Head`) is kept again
/// under the same path through `to`. The copy's layers hold the same drawings,
/// so the same slots point at them. How many were copied.
pub fn copy_renamed(film: &Path, from: &str, to: &str) -> std::io::Result<usize> {
    copy_renamed_in(&path(), film, from, to)
}

fn copy_renamed_in(file: &Path, film: &Path, from: &str, to: &str) -> std::io::Result<usize> {
    let store = load_from(file);
    let Some(kept) = store.get(&film.to_string_lossy().to_string()) else {
        return Ok(0);
    };
    let mut copies = Vec::new();
    for (timeline, slots) in kept {
        let parts: Vec<&str> = timeline.split('/').collect();
        if !parts.contains(&from) {
            continue;
        }
        let renamed = parts.iter().map(|p| if *p == from { to } else { p }).collect::<Vec<_>>().join("/");
        if !kept.contains_key(&renamed) {
            copies.push((renamed, slots.clone()));
        }
    }
    for (timeline, slots) in &copies {
        put_in(file, film, timeline, Some(slots))?;
    }
    Ok(copies.len())
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

    #[test]
    fn a_duplicate_keeps_the_rig_tags_of_every_timeline_through_it() {
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("rig_slots.json");
        let film = Path::new("/films/a.buzz");
        let body = Kept { pattern: 1, slots: vec![Some(7)] };
        let head = Kept { pattern: 4, slots: vec![Some(11)] };
        put_in(&file, film, "Raju", Some(&body)).unwrap();
        put_in(&file, film, "Raju/Head", Some(&head)).unwrap();
        put_in(&file, film, "Mishra", Some(&body)).unwrap();
        assert_eq!(copy_renamed_in(&file, film, "Raju", "Raju copy").unwrap(), 2);
        assert_eq!(get_in(&file, film, "Raju copy"), Some(body.clone()));
        assert_eq!(get_in(&file, film, "Raju copy/Head"), Some(head));
        assert_eq!(get_in(&file, film, "Raju"), Some(body), "the original keeps its own");
        assert_eq!(get_in(&file, film, "Mishra copy"), None);
        assert_eq!(copy_renamed_in(&file, Path::new("/films/b.buzz"), "Raju", "X").unwrap(), 0);
    }
}
