//! **The New Episode window's Look, remembered per series** (ANIMATE_COPILOT_PLAN
//! B7): `assets/comfy_profiles/<Series>/episode_settings.json`, next to the
//! series' ComfyUI `profile.json` (which it never rewrites). A series with no
//! file starts from the series' own defaults (captions, objects, the world).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the Look step (and the Cast step's checkbox) keeps for a series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesSettings {
    pub world: String,
    /// An index into `buzz_ui::produce_dialog::STYLE_PRESETS`.
    pub style: usize,
    pub style_text: String,
    pub captions: bool,
    pub objects: bool,
    pub angles: bool,
    pub allow_generated: bool,
}

impl SeriesSettings {
    /// What a series starts from: its profile's captions / objects, its default world.
    pub fn defaults(series: &str) -> Self {
        Self {
            world: buzz_ui::produce_dialog::default_world(series).to_string(),
            style: 0,
            style_text: String::new(),
            captions: crate::comfy_profile::captions_enabled(None, series),
            objects: crate::bg_objects::enabled(None, series),
            angles: true,
            allow_generated: false,
        }
    }
}

fn file_in(root: &Path, series: &str) -> PathBuf {
    root.join(crate::comfy_profile::canonical_series(series)).join("episode_settings.json")
}

/// The saved settings of `series` under the profiles folder `root`, if any.
pub fn load_in(root: &Path, series: &str) -> Option<SeriesSettings> {
    if series.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(file_in(root, series)).ok()?).ok()
}

/// Save `settings` for `series` under `root` (the series folder is made if it is new).
pub fn save_in(root: &Path, series: &str, settings: &SeriesSettings) -> std::io::Result<()> {
    if series.trim().is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no series name"));
    }
    let path = file_in(root, series);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?)
}

/// [`load_in`] on the real profiles folder, else the series defaults.
pub fn load(series: &str) -> SeriesSettings {
    load_in(&crate::comfy_profile::profiles_root(), series).unwrap_or_else(|| SeriesSettings::defaults(series))
}

/// [`save_in`] on the real profiles folder.
pub fn save(series: &str, settings: &SeriesSettings) -> std::io::Result<()> {
    save_in(&crate::comfy_profile::profiles_root(), series, settings)
}

/// The series that have a folder under the profiles root, sorted, plus the
/// Originals series so a fresh machine has one to start from.
pub fn series_list() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(crate::comfy_profile::profiles_root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    if !names.iter().any(|n| n.eq_ignore_ascii_case("originals")) {
        names.push("Originals".into());
    }
    names.retain(|n| !n.eq_ignore_ascii_case("chirailiya"));
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_per_series() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_in(dir.path(), "Khayal3Baje"), None);
        let a = SeriesSettings { world: "a town".into(), style: 2, style_text: "dusty".into(), captions: true, objects: false, angles: false, allow_generated: true };
        save_in(dir.path(), "Khayal3Baje", &a).unwrap();
        assert_eq!(load_in(dir.path(), "Khayal3Baje"), Some(a.clone()));
        // Another series is untouched.
        assert_eq!(load_in(dir.path(), "Chirailiya"), None);
        assert!(save_in(dir.path(), " ", &a).is_err());
    }
}
