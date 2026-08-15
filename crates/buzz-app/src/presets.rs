//! Where export presets are kept between runs.
//!
//! The built-ins ship with the program; the user's own are read from and
//! written to `export_presets.json` beside the dock layout and the asset
//! library — app-level, because a preset is a fact about where the animator
//! sends their films, not about any one film. See [`buzz_export::preset`].

use std::path::PathBuf;

use buzz_export::ExportPreset;

/// The presets available to the Export dialog: the built-ins, then the user's.
pub struct PresetLibrary {
    /// Only the user's own — the built-ins are prepended on demand so a new
    /// built-in in a later version appears without rewriting anyone's file.
    user: Vec<ExportPreset>,
}

impl Default for PresetLibrary {
    fn default() -> Self {
        Self::load()
    }
}

impl PresetLibrary {
    /// Read the user's presets from disk. A missing or unreadable file is not
    /// an error — it just means there are none yet.
    pub fn load() -> Self {
        let user = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<ExportPreset>>(&text).ok())
            .unwrap_or_default()
            .into_iter()
            // A file that somehow carries a built-in flag is not trusted to
            // define a built-in; it is the program's list that decides those.
            .map(|mut p| {
                p.builtin = false;
                p
            })
            .collect();
        Self { user }
    }

    /// Everything to show, built-ins first.
    pub fn all(&self) -> Vec<ExportPreset> {
        let mut all = ExportPreset::built_ins();
        all.extend(self.user.iter().cloned());
        all
    }

    /// Just the names, for the dropdown.
    pub fn names(&self) -> Vec<String> {
        self.all().into_iter().map(|p| p.name).collect()
    }

    /// Save the current settings under a name, replacing a user preset of the
    /// same name. A name that collides with a built-in is refused — the
    /// built-ins are the program's, not the user's to shadow — and reported.
    pub fn add(&mut self, mut preset: ExportPreset) -> Result<(), String> {
        preset.builtin = false;
        let name = preset.name.trim().to_string();
        if name.is_empty() {
            return Err("Give the preset a name first".into());
        }
        if ExportPreset::built_ins().iter().any(|b| b.name == name) {
            return Err(format!("\u{201C}{name}\u{201D} is a built-in preset name"));
        }
        preset.name = name.clone();
        // Replace an existing user preset of the same name rather than making a
        // second with the same label.
        if let Some(slot) = self.user.iter_mut().find(|p| p.name == name) {
            *slot = preset;
        } else {
            self.user.push(preset);
        }
        self.save();
        Ok(())
    }

    /// Remove a user preset by name. Built-ins cannot be removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.user.len();
        self.user.retain(|p| p.name != name);
        let removed = self.user.len() != before;
        if removed {
            self.save();
        }
        removed
    }

    /// Is this one the user's, and so removable?
    pub fn is_user(&self, name: &str) -> bool {
        self.user.iter().any(|p| p.name == name)
    }

    fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.user) {
            // Best-effort: a preset the program could not write is a lost
            // convenience, not lost work, and nothing else waits on it.
            let _ = std::fs::write(&path, text);
        }
    }

    /// `%APPDATA%/BuzzAnimate/export_presets.json`, beside the dock layout.
    fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("BUZZANIMATE_EXPORT_PRESETS") {
            return PathBuf::from(path);
        }
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(std::env::temp_dir);
        base.join("BuzzAnimate").join("export_presets.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_export::PresetFormat;
    use std::sync::{Mutex, MutexGuard};

    // The path is chosen by a process-global env var, so the tests that set it
    // must not run at the same time. This guard serialises them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_env() -> (tempfile::TempDir, MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("temp dir");
        // SAFETY: the guard above makes this the only thread touching the env
        // var for the duration of the test.
        unsafe {
            std::env::set_var("BUZZANIMATE_EXPORT_PRESETS", dir.path().join("presets.json"));
        }
        (dir, guard)
    }

    fn preset(name: &str) -> ExportPreset {
        ExportPreset {
            name: name.into(),
            format: PresetFormat::Gif,
            height: Some(480),
            quality: 0,
            transparent: false,
            audio: false,
            hardware: false,
            lossless: false,
            builtin: false,
        }
    }

    #[test]
    fn built_ins_are_always_there() {
        let _env = temp_env();
        let lib = PresetLibrary::load();
        assert!(lib.names().iter().any(|n| n == "YouTube 1080p"));
    }

    #[test]
    fn a_saved_preset_survives_a_reload() {
        let _env = temp_env();
        let mut lib = PresetLibrary::load();
        lib.add(preset("Discord")).expect("saved");

        let reloaded = PresetLibrary::load();
        assert!(reloaded.names().iter().any(|n| n == "Discord"));
        assert!(reloaded.is_user("Discord"));
        assert!(!reloaded.is_user("YouTube 1080p"), "a built-in is not the user's");
    }

    #[test]
    fn a_builtin_name_cannot_be_shadowed() {
        let _env = temp_env();
        let mut lib = PresetLibrary::load();
        let err = lib.add(preset("YouTube 1080p")).unwrap_err();
        assert!(err.contains("built-in"), "{err}");
    }

    #[test]
    fn saving_the_same_name_replaces_rather_than_duplicates() {
        let _env = temp_env();
        let mut lib = PresetLibrary::load();
        lib.add(preset("Mine")).unwrap();
        let mut second = preset("Mine");
        second.height = Some(720);
        lib.add(second).unwrap();

        let count = lib.names().iter().filter(|n| *n == "Mine").count();
        assert_eq!(count, 1, "one entry, updated");
    }

    #[test]
    fn a_user_preset_can_be_removed() {
        let _env = temp_env();
        let mut lib = PresetLibrary::load();
        lib.add(preset("Temp")).unwrap();
        assert!(lib.remove("Temp"));
        assert!(!lib.remove("YouTube 1080p"), "a built-in cannot be removed");
    }
}
