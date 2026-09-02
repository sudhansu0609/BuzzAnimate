//! Saved commands: scripts you keep by name and run again.
//!
//! Animate's Commands menu is exactly this — an Actions script saved under a
//! name so a repeated job is one click, not a re-type. A saved command is just
//! the script text in a `.js` file in a folder of its own, beside the user's
//! other application data, so it survives between documents and between runs and
//! can be edited with any text editor.

use std::path::{Path, PathBuf};

use crate::DocError;

/// Where saved commands live, beside the user's other application data.
pub fn commands_root() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("BuzzAnimate").join("commands")
}

/// One saved command: a name and its script source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedCommand {
    pub name: String,
    pub source: String,
}

/// The saved commands on disk, rescanned rather than watched — listing a folder
/// is microseconds and the list is only wanted when a menu opens.
#[derive(Debug, Clone, Default)]
pub struct CommandLibrary {
    root: Option<PathBuf>,
    commands: Vec<SavedCommand>,
    pub last_error: Option<String>,
}

impl CommandLibrary {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let mut library = Self { root: Some(root.into()), ..Default::default() };
        library.rescan();
        library
    }

    /// The library in the user's own data directory.
    pub fn user() -> Self {
        Self::at(commands_root())
    }

    pub fn iter(&self) -> impl Iterator<Item = &SavedCommand> {
        self.commands.iter()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Read the folder again, loading each script's source.
    pub fn rescan(&mut self) {
        self.commands.clear();
        self.last_error = None;
        let Some(root) = self.root.clone() else {
            return;
        };
        if !root.exists() {
            return;
        }
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) => {
                self.last_error = Some(e.to_string());
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("js")) {
                if let (Some(name), Ok(source)) = (
                    path.file_stem().map(|s| s.to_string_lossy().to_string()),
                    std::fs::read_to_string(&path),
                ) {
                    self.commands.push(SavedCommand { name, source });
                }
            }
        }
        self.commands.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// A file-name-safe version of a wanted name, never empty.
    fn safe_name(wanted: &str) -> String {
        let cleaned: String = wanted
            .trim()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let cleaned = cleaned.trim().to_string();
        if cleaned.is_empty() { "Command".to_string() } else { cleaned }
    }

    /// Keep `source` as a command under `name`, replacing any of the same name
    /// (unlike a template — a command you re-save is the same command improved).
    pub fn save(&mut self, name: &str, source: &str) -> Result<SavedCommand, DocError> {
        let Some(root) = self.root.clone() else {
            return Err(DocError::Io(std::io::Error::other("no commands folder is configured")));
        };
        std::fs::create_dir_all(&root)?;
        let name = Self::safe_name(name);
        let path = root.join(format!("{name}.js"));
        std::fs::write(&path, source)?;
        self.rescan();
        Ok(SavedCommand { name, source: source.to_string() })
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        let path = root.join(format!("{}.js", Self::safe_name(name)));
        let removed = std::fs::remove_file(path).is_ok();
        if removed {
            self.rescan();
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("buzzanimate-commands-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_saved_command_comes_back_with_its_source() {
        let root = temp_root("save");
        let mut library = CommandLibrary::at(&root);
        let saved = library.save("Grow By Ten", "fl.trace('hi');").expect("save");
        assert_eq!(saved.name, "Grow By Ten");

        let reloaded = CommandLibrary::at(&root);
        let one = reloaded.iter().next().expect("one command");
        assert_eq!(one.name, "Grow By Ten");
        assert_eq!(one.source, "fl.trace('hi');");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn re_saving_a_name_replaces_it() {
        let root = temp_root("replace");
        let mut library = CommandLibrary::at(&root);
        library.save("Job", "one").expect("first");
        library.save("Job", "two").expect("second");
        assert_eq!(library.len(), 1);
        assert_eq!(library.iter().next().unwrap().source, "two");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unsafe_name_is_made_into_a_filename() {
        let root = temp_root("unsafe");
        let mut library = CommandLibrary::at(&root);
        let saved = library.save("a/b:c", "x").expect("save");
        assert_eq!(saved.name, "a_b_c");
        let _ = std::fs::remove_dir_all(&root);
    }
}
