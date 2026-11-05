//! **The user's Animate commands as native commands** (JSFL_COMMANDS_PLAN J1, J4, J5).
//!
//! This is the part with no window in it, so a headless test can drive all of
//! it: finding the `.jsfl` scripts (Animate's Commands folder and the app's own
//! copy), importing them, the settings each script exposes and the values the
//! user changed, and Animate's keyboard shortcuts for them.
//!
//! The user's Animate files are only ever **read**. Imports copy; changed
//! settings are stored in BuzzAnimate's own `jsfl.json` and written into the
//! *text that runs*, never into the script on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use buzz_script::tunables::{self, Kind, Tunable};
use buzz_ui::workspace::KeyChord;
use serde::{Deserialize, Serialize};

/// The shared-settings include every command on the shelf loads.
pub const SHARED_STEM: &str = "commonVariables";

/// One script on the Commands menu.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptEntry {
    /// The file name without `.jsfl`: what `Commands.<name>` means in Animate.
    pub name: String,
    /// What the menu shows (`names.xml` can rename a stock command).
    pub label: String,
    pub path: PathBuf,
    /// The folder `fl.configURI` points at for this script: the parent of its
    /// `Commands` folder, so `configURI + "Commands/commonVariables.jsfl"` resolves.
    pub config_dir: PathBuf,
    /// An Animate-internal stub (it loads Animate's own `Javascript/` helpers).
    pub internal: bool,
}

/// The app's own Commands folder, where Import Animate Commands copies to.
pub fn app_commands_dir() -> PathBuf {
    buzz_doc::commands::commands_root().join("Commands")
}

fn jsfl_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsfl")))
        .collect();
    files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    files
}

/// Animate's Commands folder: the Animate version with the most scripts in it
/// (newest on a tie), under `%LOCALAPPDATA%/Adobe`.
pub fn animate_commands_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let adobe = PathBuf::from(local).join("Adobe");
    let mut best: Option<(usize, String, PathBuf)> = None;
    for e in std::fs::read_dir(adobe).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("Animate") {
            continue;
        }
        let dir = e.path().join("en_US").join("Configuration").join("Commands");
        let n = jsfl_files(&dir).len();
        if n == 0 {
            continue;
        }
        if best.as_ref().is_none_or(|(bn, bname, _)| (n, &name) > (*bn, bname)) {
            best = Some((n, name, dir));
        }
    }
    best.map(|(_, _, d)| d)
}

/// `names.xml`: `<name source="A" target="B">` pairs.
fn names_map(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(dir.join("names.xml")) else {
        return out;
    };
    for part in text.split("<name ").skip(1) {
        let attr = |key: &str| -> Option<String> {
            let at = part.find(&format!("{key}=\""))? + key.len() + 2;
            let end = part[at..].find('"')?;
            Some(part[at..at + end].to_string())
        };
        if let (Some(s), Some(t)) = (attr("source"), attr("target")) {
            out.insert(s, t);
        }
    }
    out
}

/// Every script in `dirs`, a name found in an earlier folder hiding the same
/// name in a later one. `commonVariables` is an include, not a command, so it
/// is not listed.
pub fn scan(dirs: &[PathBuf]) -> Vec<ScriptEntry> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for dir in dirs {
        let names = names_map(dir);
        for path in jsfl_files(dir) {
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let key = name.to_lowercase();
            if key == SHARED_STEM.to_lowercase() || seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let internal = std::fs::read_to_string(&path)
                .map(|t| t.contains("configURI + 'Javascript/") || t.contains("configURI + \"Javascript/"))
                .unwrap_or(false);
            out.push(ScriptEntry {
                label: names.get(&name).cloned().unwrap_or_else(|| name.clone()),
                config_dir: dir.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.clone()),
                name,
                path,
                internal,
            });
        }
    }
    out.sort_by_key(|e| e.label.to_lowercase());
    out
}

/// What Import Animate Commands did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub copied: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

/// Copy the scripts of `from` into `to` (the originals stay as they are).
/// Animate-internal stubs are skipped and listed; `commonVariables.jsfl` and
/// `names.xml` come along because commands load them.
pub fn import_animate_commands(from: &Path, to: &Path) -> Result<ImportReport, String> {
    std::fs::create_dir_all(to).map_err(|e| format!("could not create {}: {e}", to.display()))?;
    let mut report = ImportReport::default();
    for path in jsfl_files(from) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
        if text.contains("configURI + 'Javascript/") || text.contains("configURI + \"Javascript/") {
            report.skipped.push((name, "Animate's own command (needs Animate's built-in scripts)".into()));
            continue;
        }
        std::fs::copy(&path, to.join(&name)).map_err(|e| format!("{name}: {e}"))?;
        report.copied.push(name);
    }
    let names = from.join("names.xml");
    if names.exists() {
        let _ = std::fs::copy(&names, to.join("names.xml"));
    }
    Ok(report)
}

// ---- settings and keys on disk ----------------------------------------------

/// One key bound to a script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub script: String,
    pub chord: KeyChord,
}

/// The app's own record about the commands: where Animate's folder is, the
/// settings the user changed, and the keys bound to scripts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JsflStore {
    /// Overrides Animate's Commands folder (the default is found by itself).
    #[serde(default)]
    pub commands_dir: Option<PathBuf>,
    /// script stem (or [`SHARED_STEM`]) -> setting name -> literal.
    #[serde(default)]
    pub settings: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub keys: Vec<KeyBinding>,
}

pub fn store_path() -> PathBuf {
    buzz_doc::commands::commands_root()
        .parent()
        .map(|p| p.join("jsfl.json"))
        .unwrap_or_else(|| PathBuf::from("jsfl.json"))
}

impl JsflStore {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    /// The folders the menu lists: the app's own first, then Animate's.
    pub fn dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![app_commands_dir()];
        if let Some(d) = self.commands_dir.clone().or_else(animate_commands_dir) {
            dirs.push(d);
        }
        dirs
    }

    pub fn value(&self, scope: &str, name: &str) -> Option<&String> {
        self.settings.get(scope)?.get(name)
    }

    /// Set (or, with `None`, reset to the script's own value) one setting.
    pub fn set(&mut self, scope: &str, name: &str, literal: Option<String>) {
        match literal {
            Some(l) => {
                self.settings.entry(scope.to_string()).or_default().insert(name.to_string(), l);
            }
            None => {
                if let Some(m) = self.settings.get_mut(scope) {
                    m.remove(name);
                    if m.is_empty() {
                        self.settings.remove(scope);
                    }
                }
            }
        }
    }

    pub fn reset_script(&mut self, scope: &str) {
        self.settings.remove(scope);
    }

    /// Everything needed to run `entry` with the user's settings: the script
    /// text with its own overrides written in, and the overrides filed under
    /// the shared include (applied when the script loads it).
    pub fn prepare(&self, entry: &ScriptEntry) -> Result<(String, Vec<(String, String, String)>), String> {
        let bytes = std::fs::read(&entry.path).map_err(|e| format!("{}: {e}", entry.path.display()))?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        let own: Vec<(String, String)> = self
            .settings
            .get(&entry.name)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let shared: Vec<(String, String, String)> = self
            .settings
            .get(SHARED_STEM)
            .map(|m| m.iter().map(|(k, v)| (SHARED_STEM.to_string(), k.clone(), v.clone())).collect())
            .unwrap_or_default();
        Ok((tunables::apply(&text, &own), shared))
    }
}

/// The settings a script offers: its own, and the shared include's when it loads it.
pub fn tunables_of(entry: &ScriptEntry) -> (Vec<Tunable>, Vec<Tunable>) {
    let text = std::fs::read(&entry.path)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let own = tunables::parse(&text);
    let shared = if text.contains(SHARED_STEM) {
        shared_tunables(&entry.config_dir)
    } else {
        Vec::new()
    };
    (own, shared)
}

pub fn shared_tunables(config_dir: &Path) -> Vec<Tunable> {
    std::fs::read(config_dir.join("Commands").join(format!("{SHARED_STEM}.jsfl")))
        .map(|b| tunables::parse(&String::from_utf8_lossy(&b)))
        .unwrap_or_default()
}

/// The literal text for a typed value of `kind`.
pub fn literal_for(kind: Kind, text: &str) -> Option<String> {
    let t = text.trim();
    let lit = match kind {
        Kind::Text => format!("\"{}\"", t.trim_matches(['"', '\'']).replace('"', "'")),
        Kind::Numbers if !t.starts_with('[') => format!("[{t}]"),
        _ => t.to_string(),
    };
    tunables::literal_is(kind, &lit).then_some(lit)
}

// ---- Animate's keyboard shortcuts (J4) ----------------------------------------

/// One `<keycmd>` of Animate's shortcut file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimateKey {
    pub cmd: String,
    /// The character (`<uchar>`) or Animate's virtual-key code (`<key>`).
    pub uchar: Option<String>,
    pub vkey: Option<u32>,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// `<ext/>`: the key on the numeric keypad.
    pub numpad: bool,
    pub hold: bool,
}

/// Read Animate's shortcut file (`Shortcuts.xml` and friends).
pub fn parse_shortcuts_xml(xml: &str) -> Vec<AnimateKey> {
    let mut out = Vec::new();
    for block in xml.split("<keycmd>").skip(1) {
        let block = block.split("</keycmd>").next().unwrap_or("");
        let tag = |t: &str| -> Option<String> {
            let open = format!("<{t}>");
            let at = block.find(&open)? + open.len();
            let end = block[at..].find(&format!("</{t}>"))?;
            Some(block[at..at + end].to_string())
        };
        let Some(cmd) = tag("cmd") else { continue };
        out.push(AnimateKey {
            cmd,
            uchar: tag("uchar"),
            vkey: tag("key").and_then(|k| k.trim().parse().ok()),
            ctrl: block.contains("<cmdkey/>"),
            shift: block.contains("<shift/>"),
            alt: block.contains("<alt/>"),
            numpad: block.contains("<ext/>"),
            hold: block.contains("<hold/>"),
        });
    }
    out
}

/// Animate's virtual-key codes (the `<key>` element), as egui key names.
fn vkey_name(code: u32) -> Option<&'static str> {
    Some(match code {
        1 => "Space",
        2 => "Backspace",
        3 => "Tab",
        4 | 5 => "Enter",
        6 => "Escape",
        7 => "F1",
        8 => "F2",
        9 => "F3",
        10 => "F4",
        11 => "F5",
        12 => "F6",
        13 => "F7",
        14 => "F8",
        15 => "F9",
        16 => "F10",
        17 => "F11",
        18 => "F12",
        35 => "Delete",
        36 => "Home",
        37 => "End",
        38 => "PageUp",
        39 => "PageDown",
        42 => "ArrowLeft",
        43 => "ArrowRight",
        44 => "ArrowUp",
        45 => "ArrowDown",
        _ => return None,
    })
}

/// The egui name of the key for a `<uchar>`.
fn uchar_name(c: &str) -> Option<String> {
    let ch = c.chars().next()?;
    if c.chars().count() != 1 {
        return None;
    }
    Some(match ch {
        'a'..='z' | 'A'..='Z' => ch.to_ascii_uppercase().to_string(),
        '0'..='9' => ch.to_string(),
        '-' => "Minus".into(),
        '=' | '+' => "Equals".into(),
        ',' => "Comma".into(),
        '.' => "Period".into(),
        ';' => "Semicolon".into(),
        '/' => "Slash".into(),
        '\\' => "Backslash".into(),
        '[' => "OpenBracket".into(),
        ']' => "CloseBracket".into(),
        '`' => "Backtick".into(),
        '\'' => "Quote".into(),
        _ => return None,
    })
}

impl AnimateKey {
    /// The same key as a BuzzAnimate chord, if egui has a key of that name.
    /// (egui does not tell the numeric keypad from the number row, so a keypad
    /// 4 binds the same key as the row's 4.)
    pub fn chord(&self) -> Option<KeyChord> {
        let key = match (&self.uchar, self.vkey) {
            (Some(c), _) => uchar_name(c)?,
            (None, Some(v)) => vkey_name(v)?.to_string(),
            _ => return None,
        };
        // The name egui itself uses for that key, if it has one.
        let key = egui::Key::from_name(&key)?.name().to_string();
        Some(KeyChord {
            command: self.ctrl,
            ctrl: false,
            alt: self.alt,
            shift: self.shift,
            key,
        })
    }
}

/// Two chords that press the same keys (Ctrl and Command are one on Windows).
pub fn same_chord(a: &KeyChord, b: &KeyChord) -> bool {
    (a.command || a.ctrl) == (b.command || b.ctrl)
        && a.alt == b.alt
        && a.shift == b.shift
        && a.key == b.key
}

/// A BuzzAnimate command that lost a key to a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub script: String,
    pub chord: KeyChord,
    pub displaced: String,
}

/// What importing Animate's shortcuts did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShortcutImport {
    pub bound: Vec<KeyBinding>,
    pub conflicts: Vec<Conflict>,
    /// `Commands.<name>` entries with no matching script.
    pub unmatched: Vec<String>,
}

/// Bind every `Commands.<name>` entry to the script of that name. A script may
/// get several keys. `builtin` answers which BuzzAnimate commands currently
/// use a chord; those are the conflicts -- the Animate key wins.
pub fn bind_commands(
    keys: &[AnimateKey],
    scripts: &[ScriptEntry],
    builtin: &dyn Fn(&KeyChord) -> Vec<String>,
) -> ShortcutImport {
    let mut out = ShortcutImport::default();
    for k in keys {
        let Some(name) = k.cmd.strip_prefix("Commands.") else { continue };
        let Some(script) = scripts.iter().find(|s| s.name.eq_ignore_ascii_case(name)) else {
            let shown = if name.is_empty() { "(empty command name)".to_string() } else { name.to_string() };
            if !out.unmatched.contains(&shown) {
                out.unmatched.push(shown);
            }
            continue;
        };
        let Some(chord) = k.chord() else {
            out.unmatched.push(format!("{name} (key not available in BuzzAnimate)"));
            continue;
        };
        let binding = KeyBinding { script: script.name.clone(), chord: chord.clone() };
        if out.bound.iter().any(|b| b.script == binding.script && same_chord(&b.chord, &chord)) {
            continue;
        }
        for displaced in builtin(&chord) {
            out.conflicts.push(Conflict {
                script: script.name.clone(),
                chord: chord.clone(),
                displaced,
            });
        }
        out.bound.push(binding);
    }
    out
}

/// Animate's active shortcut set: the newest Animate version under
/// `%APPDATA%/Adobe/Animate`, the set named by `currentShortcutSet` in its
/// `Settings.xml` (else `Shortcuts.xml`).
pub fn animate_shortcuts_file() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("APPDATA")?).join("Adobe").join("Animate");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("Shortcuts").is_dir())
        .collect();
    versions.sort();
    // The version the Commands folder belongs to ("Animate 2023" -> 2023), so
    // the keys come from the same Animate the scripts were written in.
    let year = animate_commands_dir().and_then(|c| {
        c.to_string_lossy()
            .split(|ch: char| !ch.is_ascii_digit())
            .find(|d| d.len() == 4 && d.starts_with("20"))
            .map(str::to_string)
    });
    let matching = year.and_then(|y| {
        versions
            .iter()
            .find(|v| v.file_name().is_some_and(|n| n.to_string_lossy() == y.as_str()))
            .cloned()
    });
    let dir = matching.or_else(|| versions.pop())?;
    let set = std::fs::read_to_string(dir.join("Settings.xml"))
        .ok()
        .and_then(|t| {
            let at = t.find("<key>currentShortcutSet</key>")?;
            let rest = &t[at..];
            let a = rest.find("<ustring>")? + "<ustring>".len();
            let b = rest[a..].find("</ustring>")?;
            Some(rest[a..a + b].to_string())
        })
        .unwrap_or_else(|| "Shortcuts".to_string());
    let file = dir.join("Shortcuts").join(format!("{set}.xml"));
    if file.exists() { Some(file) } else { Some(dir.join("Shortcuts").join("Shortcuts.xml")) }
}

/// Animate's name for a menu command, and BuzzAnimate's `Command` debug name,
/// for the shortcuts the two programs share.
const SHARED_COMMANDS: &[(&str, &str)] = &[
    ("File.New", "New"),
    ("File.Open", "Open"),
    ("File.Close", "Close"),
    ("File.Save", "Save"),
    ("File.SaveAs", "SaveAs"),
    ("Edit.Undo", "Undo"),
    ("Edit.Redo", "Redo"),
    ("Edit.Cut", "Cut"),
    ("Edit.Copy", "Copy"),
    ("Edit.Paste", "Paste"),
    ("Edit.PasteInPlace", "PasteInPlace"),
    ("Edit.SelectAll", "SelectAll"),
    ("Edit.DeselectAll", "DeselectAll"),
    ("Edit.Clear", "Delete"),
    ("Modify.Group", "GroupSelection"),
    ("Modify.Ungroup", "UngroupSelection"),
    ("Modify.BreakApart", "BreakApart"),
    ("Modify.ConvertToSymbol", "ConvertToSymbol"),
    ("Timeline.InsertFrame", "InsertFrame"),
    ("Timeline.RemoveFrames", "RemoveFrames"),
    ("Timeline.ConvertToKeyframes", "InsertKeyframe"),
    ("Timeline.ConvertToBlankKeyframes", "InsertBlankKeyframe"),
    ("Timeline.ClearFrames", "ClearFrames"),
    ("Timeline.ClearKeyframe", "ClearKeyframe"),
    ("Timeline.CreateClassicTween", "CreateClassicTween"),
    ("Control.Play", "PlayPause"),
    ("Goto.First", "FirstFrame"),
    ("Goto.Last", "LastFrame"),
    ("Goto.Next", "NextFrame"),
    ("Goto.Previous", "PreviousFrame"),
];

/// How the non-`Commands.` shortcuts of an Animate file differ from
/// BuzzAnimate's defaults (reported, never applied): `(animate command, animate
/// chord, buzz default)` for each one that maps to a BuzzAnimate command with a
/// different key, and the count of entries with no BuzzAnimate counterpart.
pub fn other_differences(
    keys: &[AnimateKey],
    default_for: &dyn Fn(&str) -> Option<KeyChord>,
) -> (Vec<String>, usize) {
    let mut differences = Vec::new();
    let mut no_counterpart = 0;
    let mut seen_cmds: Vec<&str> = Vec::new();
    for k in keys.iter().filter(|k| !k.cmd.starts_with("Commands.") && !k.hold) {
        let Some((_, buzz)) = SHARED_COMMANDS.iter().find(|(a, _)| *a == k.cmd) else {
            if !seen_cmds.contains(&k.cmd.as_str()) {
                no_counterpart += 1;
            }
            seen_cmds.push(&k.cmd);
            continue;
        };
        let Some(chord) = k.chord() else { continue };
        let ours = default_for(buzz);
        // Animate lists several keys for one command; it differs only if none match.
        let matches = keys
            .iter()
            .filter(|o| o.cmd == k.cmd)
            .filter_map(AnimateKey::chord)
            .any(|c| ours.as_ref().is_some_and(|o| same_chord(o, &c)));
        if !matches {
            let line = format!(
                "{} is {} in Animate, {} here",
                k.cmd,
                chord.label(),
                ours.map(|c| c.label()).unwrap_or_else(|| "unbound".into())
            );
            if !differences.contains(&line) {
                differences.push(line);
            }
        }
    }
    (differences, no_counterpart)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<keymap><keycmd>\n<cmd>Commands.eyeLeft</cmd>\n<uchar>4</uchar>\n<ext/>\n</keycmd>\n\
        <keycmd><cmd>Commands.RevEyeUp</cmd><uchar>8</uchar><ext/><mod><cmdkey/><alt/></mod></keycmd>\n\
        <keycmd><cmd>Commands.switchPoseInteractive</cmd><uchar>I</uchar></keycmd>\n\
        <keycmd><cmd>Commands.switchPoseInteractive</cmd><uchar>5</uchar></keycmd>\n\
        <keycmd><cmd>Commands.extendFrames</cmd><key>18</key><mod><shift/><cmdkey/></mod></keycmd>\n\
        <keycmd><cmd>Commands.nothingLikeIt</cmd><uchar>Q</uchar></keycmd>\n\
        <keycmd><cmd>File.Save</cmd><uchar>S</uchar><mod><cmdkey/></mod></keycmd></keymap>";

    fn script(name: &str) -> ScriptEntry {
        ScriptEntry {
            name: name.into(),
            label: name.into(),
            path: PathBuf::from(name),
            config_dir: PathBuf::new(),
            internal: false,
        }
    }

    #[test]
    fn animate_keys_bind_to_scripts_and_report_conflicts() {
        let keys = parse_shortcuts_xml(XML);
        assert_eq!(keys.len(), 7);
        let scripts = ["eyeLeft", "RevEyeUp", "switchPoseInteractive", "extendFrames"].map(script);
        let builtin = |c: &KeyChord| -> Vec<String> {
            if c.key == "I" && !c.command && !c.alt && !c.shift {
                vec!["SomeToolCommand".to_string()]
            } else {
                vec![]
            }
        };
        let r = bind_commands(&keys, &scripts, &builtin);
        assert_eq!(r.bound.len(), 5, "switchPoseInteractive keeps both its keys");
        assert_eq!(r.unmatched, ["nothingLikeIt"]);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].displaced, "SomeToolCommand");
        let find = |key: &str, command: bool, alt: bool, shift: bool| {
            r.bound.iter().find(|b| {
                b.chord.key == key && b.chord.command == command && b.chord.alt == alt && b.chord.shift == shift
            })
        };
        assert_eq!(find(egui::Key::Num4.name(), false, false, false).unwrap().script, "eyeLeft");
        assert_eq!(find(egui::Key::Num8.name(), true, true, false).unwrap().script, "RevEyeUp");
        assert_eq!(find("I", false, false, false).unwrap().script, "switchPoseInteractive");
        assert_eq!(find(egui::Key::Num5.name(), false, false, false).unwrap().script, "switchPoseInteractive");
        assert_eq!(find(egui::Key::F12.name(), true, false, true).unwrap().script, "extendFrames");
    }

    #[test]
    fn a_changed_setting_is_written_into_the_text_that_runs() {
        let dir = std::env::temp_dir().join(format!("jsfl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("move.jsfl");
        std::fs::write(&path, "var moveAmount = 5; // pixels\nfl.trace(moveAmount);\n").unwrap();
        let mut entry = script("move");
        entry.path = path.clone();
        let mut store = JsflStore::default();
        store.set("move", "moveAmount", Some("12".into()));
        let (text, _) = store.prepare(&entry).unwrap();
        assert!(text.starts_with("var moveAmount = 12; // pixels"));
        // The file on disk is untouched.
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("var moveAmount = 5;"));
        store.set("move", "moveAmount", None);
        assert!(store.settings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
