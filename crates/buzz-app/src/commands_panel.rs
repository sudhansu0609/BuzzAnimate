//! **Window ▸ Commands** — the JSFL command shelf as a panel of its own.
//!
//! The shelf is the folder Animate keeps saved commands in, brought over as it
//! is: `<config>/BuzzAnimate/Configuration/Commands/*.jsfl` (see
//! [`buzz_script::default_config_dir`], which also finds Animate's own folder).
//! This panel is what makes it feel like home rather than like a folder:
//!
//! * **Every command, grouped** — eyes, eyebrows, hands and body, poses, layers
//!   and rig, frames and keys, talking — a button each, with the key it is on.
//! * **The shelf's settings** — the `CONFIG` numbers every command reads from
//!   `commonVariables.jsfl` (how many frames a move takes, how far, how much
//!   sway) — as fields to change and save, instead of a text file to edit.
//! * **Import Animate shortcuts** — the keymap made in Animate, keys for the
//!   commands included, mapped onto this program ([`buzz_ui::animate_keys`]).
//! * **Questions** — a command that asks something with `prompt()` (how many
//!   loops, which pose) gets a dialog; the last answers are remembered.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::RichText;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandsAction {
    /// Run this command (file stem), with these answers to its questions
    /// (`None`: ask first if it has any).
    Run { name: String, answers: Option<Vec<String>> },
    /// Read this Animate keymap file into the shortcuts.
    ImportKeymap(PathBuf),
    /// Choose an Animate keymap file to import.
    PickKeymap,
}

/// One command on the shelf.
#[derive(Debug, Clone, PartialEq)]
pub struct ShelfCommand {
    pub name: String,
    pub path: PathBuf,
}

/// The groups, in the order they are shown, with the words that put a command
/// in one. The first match wins.
const GROUPS: &[(&str, &[&str])] = &[
    ("Eyebrows", &["brow"]),
    ("Eyes", &["eye"]),
    ("Hands and body", &["hand", "wrist", "palm", "plam", "body", "limb"]),
    ("Poses", &["pose"]),
    ("Talking and lip sync", &["talk", "lipsync"]),
    ("Layers and rig", &["rename", "layer", "parent", "charactersetup", "symbol", "shadow"]),
    ("Frames and keys", &["frame", "keyframe", "tween", "extend"]),
];

fn group_of(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    GROUPS
        .iter()
        .find(|(_, words)| words.iter().any(|w| lower.contains(w)))
        .map(|(g, _)| *g)
        .unwrap_or("Other")
}

/// The settings file every command reads first.
pub const SETTINGS_FILE: &str = "commonVariables.jsfl";

/// Not commands: the shared settings, and old copies kept beside the real one.
fn is_helper(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "commonvariables" || lower.ends_with("_backup") || lower.ends_with(" backup")
}

/// The commands in `config/Commands`, by name.
pub fn shelf(config: &Path) -> Vec<ShelfCommand> {
    let mut out: Vec<ShelfCommand> = std::fs::read_dir(config.join("Commands"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("jsfl")))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().to_string();
            (!is_helper(&name)).then_some(ShelfCommand { name, path })
        })
        .collect();
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

// ------------------------------------------------------------------ settings

/// One setting from `commonVariables.jsfl`: `moveAmount: 5,` inside `CONFIG`
/// or `var moveFrames = 6;` above it.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub name: String,
    pub value: SettingValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Number(f64),
    Flag(bool),
    /// Another setting's value (`handMoveFrames: moveFrames`) — shown, not
    /// edited, since changing the one it follows is the point of writing it so.
    Follows(String),
}

/// One setting line, taken apart so it can be put back together with only its
/// value changed.
struct SettingLine<'a> {
    /// Everything before the value: indent, `var `, the name, `=` or `:`.
    head: String,
    name: &'a str,
    value: &'a str,
    /// What follows the value: `;` or `,`, spaces, a `// comment`, the line end.
    tail: String,
}

fn setting_line(line: &str) -> Option<SettingLine<'_>> {
    let body = line.trim_end_matches(['\r', '\n']);
    let ending = &line[body.len()..];
    let (code, comment) = match body.find("//") {
        Some(at) => (&body[..at], &body[at..]),
        None => (body, ""),
    };
    let trimmed = code.trim_start();
    let indent = &code[..code.len() - trimmed.len()];
    let (head_word, rest, sep) = match trimmed.strip_prefix("var ") {
        Some(r) => ("var ", r, '='),
        None => ("", trimmed, ':'),
    };
    let (name_part, value_part) = rest.split_once(sep)?;
    let name = name_part.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let value_trimmed = value_part.trim_end();
    let (value, punct) = match value_trimmed.strip_suffix([';', ',']) {
        Some(v) => (v.trim(), &value_trimmed[value_trimmed.len() - 1..]),
        None => (value_trimmed.trim(), ""),
    };
    let lead = &value_part[..value_part.len() - value_part.trim_start().len()];
    let after = &value_part[value_trimmed.len()..];
    Some(SettingLine {
        head: format!("{indent}{head_word}{name_part}{sep}{lead}"),
        name,
        value,
        tail: format!("{punct}{after}{comment}{ending}"),
    })
}

fn setting_value(value: &str) -> Option<SettingValue> {
    if let Ok(n) = value.parse::<f64>() {
        Some(SettingValue::Number(n))
    } else if value == "true" || value == "false" {
        Some(SettingValue::Flag(value == "true"))
    } else if !value.is_empty() && value.chars().all(|c| c.is_alphanumeric() || c == '_') {
        Some(SettingValue::Follows(value.to_string()))
    } else {
        None
    }
}

/// Read the settings out of the shared file's text, in the order they appear.
pub fn read_settings(text: &str) -> Vec<Setting> {
    text.split_inclusive('\n')
        .filter_map(setting_line)
        .filter_map(|l| Some(Setting { name: l.name.to_string(), value: setting_value(l.value)? }))
        .collect()
}

/// Write `settings` (as [`read_settings`] gave them, in the same order) back
/// into the text, changing only the values: comments, functions and layout are
/// left exactly as they were.
pub fn write_settings(text: &str, settings: &[Setting]) -> String {
    let format = |v: &SettingValue| match v {
        SettingValue::Number(n) if n.fract() == 0.0 => format!("{}", *n as i64),
        SettingValue::Number(n) => format!("{n}"),
        SettingValue::Flag(b) => b.to_string(),
        SettingValue::Follows(s) => s.clone(),
    };
    let mut next = settings.iter();
    text.split_inclusive('\n')
        .map(|line| match setting_line(line).filter(|l| setting_value(l.value).is_some()) {
            Some(l) => match next.next() {
                Some(s) if s.name == l.name => format!("{}{}{}", l.head, format(&s.value), l.tail),
                _ => line.to_string(),
            },
            None => line.to_string(),
        })
        .collect()
}

// ------------------------------------------------------------------ the panel

/// A command waiting for its questions to be answered.
#[derive(Debug, Clone)]
struct Asking {
    name: String,
    questions: Vec<String>,
    answers: Vec<String>,
}

#[derive(Default)]
pub struct CommandsPanel {
    pub status: String,
    filter: String,
    config_of: Option<PathBuf>,
    commands: Vec<ShelfCommand>,
    settings: Vec<Setting>,
    settings_dirty: bool,
    asking: Option<Asking>,
    /// The last answers given to each command's questions.
    remembered: BTreeMap<String, Vec<String>>,
}

impl CommandsPanel {
    fn answers_file(config: &Path) -> PathBuf {
        config.join("Commands").join(".buzz_answers.json")
    }

    fn rescan(&mut self, config: Option<&Path>) {
        self.config_of = config.map(Path::to_path_buf);
        self.commands = config.map(shelf).unwrap_or_default();
        self.settings = config
            .and_then(|c| std::fs::read_to_string(c.join("Commands").join(SETTINGS_FILE)).ok())
            .map(|t| read_settings(&t))
            .unwrap_or_default();
        self.settings_dirty = false;
        self.remembered = config
            .and_then(|c| std::fs::read_to_string(Self::answers_file(c)).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    }

    /// A command asked `questions` (with their defaults): ask the person.
    pub fn ask(&mut self, name: &str, questions: Vec<(String, String)>) {
        let remembered = self.remembered.get(name).cloned().unwrap_or_default();
        let answers = questions
            .iter()
            .enumerate()
            .map(|(i, (_, default))| remembered.get(i).cloned().unwrap_or_else(|| default.clone()))
            .collect();
        self.asking = Some(Asking {
            name: name.to_string(),
            questions: questions.into_iter().map(|(q, _)| q).collect(),
            answers,
        });
    }

    fn remember(&mut self, name: &str, answers: &[String]) {
        self.remembered.insert(name.to_string(), answers.to_vec());
        if let Some(config) = &self.config_of
            && let Ok(text) = serde_json::to_string_pretty(&self.remembered)
        {
            let _ = std::fs::write(Self::answers_file(config), text);
        }
    }

    /// The questions dialog, drawn over everything while a command waits.
    pub fn dialog(&mut self, ctx: &egui::Context) -> Option<CommandsAction> {
        let asking = self.asking.as_mut()?;
        let mut action = None;
        let mut close = false;
        egui::Window::new(format!("{} asks", asking.name))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                for (i, question) in asking.questions.iter().enumerate() {
                    ui.label(question);
                    let edit = ui.add(egui::TextEdit::singleline(&mut asking.answers[i]).desired_width(260.0));
                    if i == 0 {
                        edit.request_focus();
                    }
                    ui.add_space(4.0);
                }
                ui.horizontal(|ui| {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Run").clicked() || enter {
                        action = Some(CommandsAction::Run {
                            name: asking.name.clone(),
                            answers: Some(asking.answers.clone()),
                        });
                        close = true;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                });
            });
        if close {
            let done = self.asking.take();
            if let (Some(done), Some(_)) = (done, &action) {
                self.remember(&done.name, &done.answers);
            }
        }
        action
    }

    /// Draw the panel. `keys` gives the label of the key each command is on.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        config: Option<&Path>,
        keys: &BTreeMap<String, Vec<String>>,
    ) -> Option<CommandsAction> {
        if self.config_of.as_deref() != config {
            self.rescan(config);
        }
        let mut action = None;
        let Some(config) = config else {
            ui.label("No command shelf found.");
            ui.label(
                RichText::new(
                    "Copy Animate's Commands folder into BuzzAnimate/Configuration/Commands \
                     in your settings folder, then press Rescan.",
                )
                .small()
                .weak(),
            );
            if ui.button("Rescan").clicked() {
                self.config_of = Some(PathBuf::new());
            }
            return None;
        };

        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Find a command").desired_width(140.0));
            if ui.small_button("Rescan").clicked() {
                self.rescan(Some(config));
            }
        });
        ui.label(RichText::new(format!("{} commands in {}", self.commands.len(), config.join("Commands").display())).small().weak());

        egui::CollapsingHeader::new(RichText::new("Shortcuts").strong())
            .default_open(keys.is_empty())
            .show(ui, |ui| {
                let files = buzz_ui::animate_keys::keymap_files(config);
                if files.is_empty() {
                    ui.label(RichText::new("No Animate keymap in Configuration/Keyboard Shortcuts.").small().weak());
                }
                for file in files.iter().take(4) {
                    let name = file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                    if ui
                        .button(format!("Import {name}"))
                        .on_hover_text("Use this Animate keymap here: its keys for menus, tools and these commands. Ctrl becomes Cmd on a Mac.")
                        .clicked()
                    {
                        action = Some(CommandsAction::ImportKeymap(file.clone()));
                    }
                }
                if ui.button("Import another Animate keymap\u{2026}").clicked() {
                    action = Some(CommandsAction::PickKeymap);
                }
            });

        if !self.settings.is_empty() {
            egui::CollapsingHeader::new(RichText::new("Settings").strong())
                .id_salt("commands-settings")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(RichText::new(format!("Read by every command, from {SETTINGS_FILE}")).small().weak());
                    egui::Grid::new("commands-settings-grid").num_columns(2).striped(true).show(ui, |ui| {
                        for setting in &mut self.settings {
                            ui.label(&setting.name);
                            match &mut setting.value {
                                SettingValue::Number(n) => {
                                    if ui.add(egui::DragValue::new(n).speed(0.5)).changed() {
                                        self.settings_dirty = true;
                                    }
                                }
                                SettingValue::Flag(b) => {
                                    if ui.checkbox(b, "").changed() {
                                        self.settings_dirty = true;
                                    }
                                }
                                SettingValue::Follows(other) => {
                                    ui.label(RichText::new(format!("= {other}")).weak());
                                }
                            }
                            ui.end_row();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.settings_dirty, egui::Button::new("Save settings")).clicked() {
                            let path = config.join("Commands").join(SETTINGS_FILE);
                            match std::fs::read_to_string(&path) {
                                Ok(text) => match std::fs::write(&path, write_settings(&text, &self.settings)) {
                                    Ok(()) => {
                                        self.settings_dirty = false;
                                        self.status = "Settings saved".into();
                                    }
                                    Err(e) => self.status = format!("Could not save: {e}"),
                                },
                                Err(e) => self.status = format!("Could not read {}: {e}", path.display()),
                            }
                        }
                        if self.settings_dirty && ui.button("Revert").clicked() {
                            self.rescan(Some(config));
                        }
                    });
                });
        }

        let filter = self.filter.to_lowercase();
        let mut groups: Vec<(&str, Vec<&ShelfCommand>)> = Vec::new();
        for command in &self.commands {
            if !filter.is_empty() && !command.name.to_lowercase().contains(&filter) {
                continue;
            }
            let g = group_of(&command.name);
            match groups.iter_mut().find(|(name, _)| *name == g) {
                Some((_, list)) => list.push(command),
                None => groups.push((g, vec![command])),
            }
        }
        let order = |g: &str| GROUPS.iter().position(|(n, _)| *n == g).unwrap_or(GROUPS.len());
        groups.sort_by_key(|(g, _)| order(g));
        for (group, list) in groups {
            egui::CollapsingHeader::new(RichText::new(format!("{group} ({})", list.len())).strong())
                .id_salt(("commands-group", group))
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new(("commands-grid", group)).num_columns(2).show(ui, |ui| {
                        for command in list {
                            if ui
                                .button(&command.name)
                                .on_hover_text(command.path.display().to_string())
                                .clicked()
                            {
                                action = Some(CommandsAction::Run { name: command.name.clone(), answers: None });
                            }
                            let key = keys.get(&command.name).map(|k| k.join(", ")).unwrap_or_default();
                            ui.label(RichText::new(key).small().weak());
                            ui.end_row();
                        }
                    });
                });
        }

        if !self.status.is_empty() {
            ui.separator();
            ui.label(RichText::new(&self.status).small().weak());
        }
        action
    }

    /// The command named `name` on the shelf, if there is one.
    pub fn find(&self, name: &str) -> Option<&ShelfCommand> {
        self.commands.iter().find(|c| c.name == name)
    }

    /// Make sure the shelf has been read (a key can run a command before the
    /// panel was ever drawn).
    pub fn ensure(&mut self, config: Option<&Path>) {
        if self.config_of.as_deref() != config {
            self.rescan(config);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMON: &str = "// common.jsfl\r\n\r\n//✅ Global variables\r\nvar moveFrames = 6;\r\nvar CONFIG = {\r\n    moveFrames: moveFrames,\r\n    moveAmount: 5,\r\n    alertOnMissingLayer: true,\r\n    blinkStartOffset: 20,// relative to start frame\r\n    blinkFrames: 5\r\n\r\n};\r\n\r\nfunction getDoc() {\r\n    var doc = fl.getDocumentDOM();\r\n}\r\n";

    #[test]
    fn the_shelf_settings_are_read() {
        let s = read_settings(COMMON);
        let names: Vec<&str> = s.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["moveFrames", "moveFrames", "moveAmount", "alertOnMissingLayer", "blinkStartOffset", "blinkFrames"]);
        assert_eq!(s[0].value, SettingValue::Number(6.0));
        assert_eq!(s[1].value, SettingValue::Follows("moveFrames".into()));
        assert_eq!(s[3].value, SettingValue::Flag(true));
    }

    #[test]
    fn saving_changes_only_the_values() {
        let mut s = read_settings(COMMON);
        s[0].value = SettingValue::Number(8.0);
        s[2].value = SettingValue::Number(7.5);
        s[4].value = SettingValue::Number(12.0);
        let out = write_settings(COMMON, &s);
        assert!(out.contains("var moveFrames = 8;\r\n"), "{out}");
        assert!(out.contains("    moveAmount: 7.5,\r\n"), "{out}");
        assert!(out.contains("    blinkStartOffset: 12,// relative to start frame\r\n"), "{out}");
        assert!(out.contains("    blinkFrames: 5\r\n"), "{out}");
        assert!(out.contains("function getDoc() {\r\n    var doc = fl.getDocumentDOM();\r\n}"), "the functions are untouched: {out}");
        assert_eq!(read_settings(&out)[0].value, SettingValue::Number(8.0));
    }

    #[test]
    fn commands_are_grouped_by_what_they_move() {
        assert_eq!(group_of("eyeLeft"), "Eyes");
        assert_eq!(group_of("RevEyebrowsUp"), "Eyebrows");
        assert_eq!(group_of("2LeftHandWristPalm"), "Hands and body");
        assert_eq!(group_of("switchPoseWalking"), "Poses");
        assert_eq!(group_of("renameHeadLayers"), "Layers and rig");
        assert_eq!(group_of("copyExtendFrames"), "Frames and keys");
        assert_eq!(group_of("autoTalkingAnimation"), "Talking and lip sync");
        assert!(is_helper("commonVariables") && is_helper("copyExtendFrames_backup"));
    }
}
