//! The Commands menu's state and the **Command Settings** window
//! (JSFL_COMMANDS_PLAN J1, J5).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use buzz_script::tunables::{Kind, Tunable};
use buzz_ui::jsfl_menu::{JsflMenu, JsflMenuEntry};
use buzz_ui::workspace::KeyChord;

use crate::jsfl_commands::{
    self as core, ImportReport, JsflStore, SHARED_STEM, ScriptEntry, ShortcutImport,
};

/// Everything the app keeps about the user's Animate commands.
pub struct JsflState {
    pub store: JsflStore,
    pub store_path: PathBuf,
    pub entries: Vec<ScriptEntry>,
    scanned: Option<Instant>,
    pub window_open: bool,
    /// The script whose settings are showing (`SHARED_STEM` for the shared file).
    pub selected: Option<String>,
    /// The last thing an import said, for the window and the status bar.
    pub note: Option<String>,
    pub last_import: Option<ImportReport>,
    pub last_shortcuts: Option<ShortcutImport>,
    /// The displaced BuzzAnimate shortcuts have been unbound this session.
    pub precedence_applied: bool,
    /// Text being typed into a text/list field, by `scope/name`.
    typing: std::collections::BTreeMap<String, String>,
    /// The tunables of `selected`, read once when it is picked.
    shown: Option<(String, Vec<Tunable>, Vec<Tunable>)>,
}

impl Default for JsflState {
    fn default() -> Self {
        Self::at(core::store_path())
    }
}

impl JsflState {
    pub fn at(store_path: PathBuf) -> Self {
        let store = JsflStore::load(&store_path);
        let mut s = Self {
            store,
            store_path,
            entries: Vec::new(),
            scanned: None,
            window_open: false,
            selected: None,
            note: None,
            last_import: None,
            last_shortcuts: None,
            precedence_applied: false,
            typing: Default::default(),
            shown: None,
        };
        s.rescan();
        s
    }

    pub fn rescan(&mut self) {
        self.entries = core::scan(&self.store.dirs());
        self.scanned = Some(Instant::now());
    }

    /// Rescan if the folder may have changed since the last look (the menu was
    /// just opened).
    pub fn rescan_if_stale(&mut self) {
        if self.scanned.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
            self.rescan();
        }
    }

    pub fn entry(&self, name: &str) -> Option<&ScriptEntry> {
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    pub fn save(&mut self) {
        if let Err(e) = self.store.save(&self.store_path) {
            self.note = Some(format!("Could not save command settings: {e}"));
        }
    }

    /// The list the Commands menu draws.
    pub fn menu(&self) -> JsflMenu {
        JsflMenu {
            entries: self
                .entries
                .iter()
                .map(|e| {
                    let keys: Vec<String> = self
                        .store
                        .keys
                        .iter()
                        .filter(|k| k.script.eq_ignore_ascii_case(&e.name))
                        .map(|k| k.chord.label())
                        .collect();
                    JsflMenuEntry {
                        name: e.name.clone(),
                        label: e.label.clone(),
                        shortcut: keys.join(", "),
                        enabled: !e.internal,
                        hint: if e.internal {
                            "An Animate built-in command: it needs Animate's own scripts".into()
                        } else {
                            e.path.display().to_string()
                        },
                    }
                })
                .collect(),
        }
    }

    /// Which scripts' keys were pressed this frame. Longer chords are tried
    /// first so Ctrl+Alt+8 is not taken for 8.
    pub fn pressed(&self, ctx: &egui::Context) -> Vec<String> {
        if ctx.memory(|m| m.focused().is_some()) {
            return Vec::new();
        }
        let mut bindings: Vec<_> = self
            .store
            .keys
            .iter()
            .filter_map(|b| b.chord.to_shortcut().map(|s| (b.script.clone(), s)))
            .collect();
        bindings.sort_by_key(|(_, s)| {
            std::cmp::Reverse([s.modifiers.command || s.modifiers.ctrl, s.modifiers.alt, s.modifiers.shift]
                .iter()
                .filter(|m| **m)
                .count())
        });
        let mut out = Vec::new();
        for (script, shortcut) in bindings {
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                out.push(script);
            }
        }
        out
    }

    fn pick(&mut self, name: Option<String>) {
        self.selected = name;
        self.shown = None;
        self.typing.clear();
    }

    /// Open the window, on a script if one is named.
    pub fn open_settings(&mut self, name: Option<String>) {
        self.window_open = true;
        if name.is_some() {
            self.pick(name);
        }
    }

    fn load_shown(&mut self) {
        let Some(sel) = self.selected.clone() else { return };
        if self.shown.as_ref().is_some_and(|(n, _, _)| *n == sel) {
            return;
        }
        let (own, shared) = if sel == SHARED_STEM {
            let dir = self
                .entries
                .first()
                .map(|e| e.config_dir.clone())
                .or_else(|| self.store.dirs().first().and_then(|d| d.parent().map(|p| p.to_path_buf())));
            (dir.map(|d| core::shared_tunables(&d)).unwrap_or_default(), Vec::new())
        } else if let Some(e) = self.entry(&sel) {
            core::tunables_of(e)
        } else {
            (Vec::new(), Vec::new())
        };
        self.shown = Some((sel, own, shared));
    }

    /// How many scripts have at least one setting, and how many settings there
    /// are in all (each script's own, plus the shared file's once).
    pub fn count_settings(&self) -> (usize, usize) {
        let mut scripts = 0;
        let mut values = 0;
        for e in &self.entries {
            let (own, _) = core::tunables_of(e);
            if !own.is_empty() {
                scripts += 1;
                values += own.len();
            }
        }
        if let Some(e) = self.entries.first() {
            let shared = core::shared_tunables(&e.config_dir);
            values += shared.len();
        }
        (scripts, values)
    }

    /// Draw the Command Settings window.
    pub fn show_window(&mut self, ctx: &egui::Context) {
        if !self.window_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Command Settings")
            .open(&mut open)
            .default_size([760.0, 480.0])
            .show(ctx, |ui| self.window_body(ui));
        self.window_open = open;
    }

    fn window_body(&mut self, ui: &mut egui::Ui) {
        if let Some(note) = &self.note {
            ui.label(egui::RichText::new(note).small());
        }
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(
                    "Changed values are kept by BuzzAnimate and used the next time the command runs; \
                     your Animate files are never edited.",
                )
                .small()
                .weak(),
            );
        });
        ui.separator();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(230.0);
                egui::ScrollArea::vertical().id_salt("jsfl-list").max_height(400.0).show(ui, |ui| {
                    let shared_selected = self.selected.as_deref() == Some(SHARED_STEM);
                    let changed = self.store.settings.get(SHARED_STEM).is_some_and(|m| !m.is_empty());
                    let label = format!("Shared settings{}", if changed { " *" } else { "" });
                    if ui.selectable_label(shared_selected, label).clicked() {
                        self.pick(Some(SHARED_STEM.to_string()));
                    }
                    ui.separator();
                    let names: Vec<(String, String, bool)> = self
                        .entries
                        .iter()
                        .map(|e| {
                            (
                                e.name.clone(),
                                e.label.clone(),
                                self.store.settings.get(&e.name).is_some_and(|m| !m.is_empty()),
                            )
                        })
                        .collect();
                    for (name, label, changed) in names {
                        let on = self.selected.as_deref() == Some(name.as_str());
                        let text = format!("{label}{}", if changed { " *" } else { "" });
                        if ui.selectable_label(on, text).clicked() {
                            self.pick(Some(name));
                        }
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| {
            self.load_shown();
            let Some((name, own, shared)) = self.shown.clone() else {
                ui.label("Pick a command on the left.");
                return;
            };
            ui.horizontal(|ui| {
                ui.heading(if name == SHARED_STEM { "Shared settings (commonVariables)" } else { name.as_str() });
                if self.store.settings.get(&name).is_some_and(|m| !m.is_empty())
                    && ui.button("Reset to script default").clicked()
                {
                    self.store.reset_script(&name);
                    self.typing.clear();
                    self.save();
                }
            });
            egui::ScrollArea::vertical().show(ui, |ui| {
                if own.is_empty() {
                    ui.label(egui::RichText::new("This command has no numbers to set.").weak());
                }
                for t in &own {
                    self.tunable_row(ui, &name, t);
                }
                if !shared.is_empty() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("From commonVariables.jsfl \u{2014} shared by every command that includes it")
                            .strong(),
                    );
                    for t in &shared {
                        self.tunable_row(ui, SHARED_STEM, t);
                    }
                }
            });
            });
        });
    }

    fn tunable_row(&mut self, ui: &mut egui::Ui, scope: &str, t: &Tunable) {
        let stored = self.store.value(scope, &t.name).cloned();
        let current = stored.clone().unwrap_or_else(|| t.default.clone());
        let mut new_value: Option<String> = None;
        let mut reset = false;
        ui.horizontal(|ui| {
            let label = ui.label(&t.name);
            if !t.label.is_empty() {
                label.on_hover_text(&t.label);
            }
            match t.kind {
                Kind::Number => {
                    let default = t.default_number().unwrap_or(0.0);
                    let mut v: f64 = current.trim().parse().unwrap_or(default);
                    let whole = !t.default.contains('.');
                    let span = (default.abs() * 4.0).max(10.0);
                    let lo = if default < 0.0 { -span } else { 0.0 };
                    let slider = egui::Slider::new(&mut v, lo..=span)
                        .clamping(egui::SliderClamping::Never)
                        .step_by(if whole { 1.0 } else { 0.1 });
                    if ui.add(slider).changed() {
                        new_value = Some(if whole { format!("{}", v.round() as i64) } else { format!("{v:.3}").trim_end_matches('0').trim_end_matches('.').to_string() });
                    }
                }
                Kind::Bool => {
                    let mut on = current.trim() == "true";
                    if ui.checkbox(&mut on, "").changed() {
                        new_value = Some(on.to_string());
                    }
                }
                Kind::Text | Kind::Numbers => {
                    let key = format!("{scope}/{}", t.name);
                    let shown = current.trim_matches(['"', '\'', '[', ']']).to_string();
                    let buf = self.typing.entry(key).or_insert(shown);
                    if ui.text_edit_singleline(buf).lost_focus() {
                        if let Some(l) = core::literal_for(t.kind, buf) {
                            new_value = Some(l);
                        }
                    }
                }
            }
            if stored.is_some() && ui.small_button("Reset").on_hover_text(format!("Back to {}", t.default)).clicked() {
                reset = true;
            }
            if !t.label.is_empty() {
                ui.label(egui::RichText::new(&t.label).small().weak());
            }
        });
        if reset {
            self.store.set(scope, &t.name, None);
            self.typing.remove(&format!("{scope}/{}", t.name));
            self.save();
        } else if let Some(v) = new_value {
            // A value equal to the script's own is no override.
            if v == t.default {
                self.store.set(scope, &t.name, None);
            } else {
                self.store.set(scope, &t.name, Some(v));
            }
            self.save();
        }
    }
}

/// The chord of a BuzzAnimate command's current shortcut.
pub fn chord_of(shortcut: egui::KeyboardShortcut) -> KeyChord {
    KeyChord::from_shortcut(shortcut)
}
