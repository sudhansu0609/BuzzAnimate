//! **One menu description, two menu bars.**
//!
//! [`crate::panels::menu_bar`] is written once, against [`MenuUi`] — a stand-in
//! for `egui::Ui` with just the calls a menu makes (`menu_button`, `button`,
//! `label`, `separator`, `close`, `add_enabled_ui`). Drawn into egui it is the
//! menu row in the window; walked by a [`Recorder`] it is a [`MenuNode`] tree
//! the macOS menu bar at the top of the screen is built from. One description,
//! so the two can never list different things.
//!
//! # How the recorder learns what a button does
//!
//! A menu says what a button does only inside `if button.clicked() { … }`.
//! So the recorder walks the menus once with nothing clicked, numbering the
//! buttons as it meets them, and then once per button with that one reporting
//! a click, and keeps whatever commands that walk raised. A couple of hundred
//! walks of plain code, done only when the menus change.

use egui::{RichText, Ui};

use crate::command::Command;

/// One entry of a recorded menu.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuNode {
    /// A submenu, with its entries.
    Menu { title: String, enabled: bool, items: Vec<MenuNode> },
    /// A button. `action` is what it does (filled in by [`record_menus`]);
    /// `checked` is a leading tick (`✔ `) taken off its label.
    Item { label: String, enabled: bool, checked: bool, action: Option<MenuAction> },
    /// A line of text that does nothing (a note, a count).
    Note(String),
    Separator,
}

/// What a recorded menu item does.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Command(Command),
    /// One of the user's Animate scripts, or the Commands menu's own items
    /// about them (see `crate::jsfl_menu`).
    Jsfl(crate::jsfl_menu::JsflAction),
}

/// What [`MenuUi`] records into: the tree so far and the buttons' numbering.
#[derive(Default)]
pub struct Recorder {
    /// The scripts the Commands menu lists, as the window last published them.
    jsfl: crate::jsfl_menu::JsflMenu,
    /// The menus being filled, innermost last; the bottom one is the bar.
    stack: Vec<Vec<MenuNode>>,
    /// Buttons met so far in this walk.
    count: usize,
    /// The button that reports a click in this walk, if any.
    click: Option<usize>,
    /// Disabled by an enclosing `add_enabled_ui(false, …)`.
    disabled: usize,
}

/// A menu's `Ui`: the real one, or a [`Recorder`].
pub enum MenuUi<'a> {
    Egui(&'a mut Ui),
    Record(&'a mut Recorder),
}

/// A button's (or submenu's) response, as far as a menu asks of one.
pub enum MenuResponse {
    Egui(egui::Response),
    Record { clicked: bool },
}

impl MenuResponse {
    pub fn clicked(&self) -> bool {
        match self {
            Self::Egui(r) => r.clicked(),
            Self::Record { clicked } => *clicked,
        }
    }

    pub fn on_hover_text(self, text: impl Into<egui::WidgetText>) -> Self {
        match self {
            Self::Egui(r) => Self::Egui(r.on_hover_text(text)),
            record => record,
        }
    }
}

/// [`MenuUi::menu_button`]'s answer: the button's response, as egui's
/// `InnerResponse` carries it.
pub struct MenuInner {
    pub response: MenuResponse,
}

/// A leading tick on a menu label, as the menus write them, and the label
/// without it (or its blank stand-in).
fn split_tick(label: &str) -> (bool, String) {
    if let Some(rest) = label.strip_prefix("\u{2714} ").or_else(|| label.strip_prefix("\u{2714}")) {
        (true, rest.trim_start().to_string())
    } else {
        (false, label.trim_start().to_string())
    }
}

impl<'a> MenuUi<'a> {
    /// A submenu: drawn as egui's, or recorded (and always walked).
    pub fn menu_button(&mut self, title: impl Into<String>, add: impl FnOnce(&mut MenuUi<'_>)) -> MenuInner {
        let title = title.into();
        match self {
            Self::Egui(ui) => {
                let inner = ui.menu_button(title, |ui| add(&mut MenuUi::Egui(ui)));
                MenuInner { response: MenuResponse::Egui(inner.response) }
            }
            Self::Record(rec) => {
                rec.stack.push(Vec::new());
                let enabled = rec.disabled == 0;
                add(&mut MenuUi::Record(rec));
                let items = rec.stack.pop().unwrap_or_default();
                if let Some(parent) = rec.stack.last_mut() {
                    parent.push(MenuNode::Menu { title, enabled, items });
                }
                MenuInner { response: MenuResponse::Record { clicked: false } }
            }
        }
    }

    /// A plain button.
    pub fn button(&mut self, label: impl Into<String>) -> MenuResponse {
        self.button_with(label.into(), String::new(), true)
    }

    /// A button with its shortcut shown, enabled or not.
    pub fn button_with(&mut self, label: impl Into<String>, shortcut: impl Into<String>, enabled: bool) -> MenuResponse {
        let label = label.into();
        match self {
            Self::Egui(ui) => {
                let button = egui::Button::new(label).shortcut_text(shortcut.into());
                MenuResponse::Egui(ui.add_enabled(enabled, button))
            }
            Self::Record(rec) => {
                let n = rec.count;
                rec.count += 1;
                let (checked, label) = split_tick(&label);
                let enabled = enabled && rec.disabled == 0;
                if let Some(menu) = rec.stack.last_mut() {
                    menu.push(MenuNode::Item { label, enabled, checked, action: None });
                }
                MenuResponse::Record { clicked: rec.click == Some(n) }
            }
        }
    }

    /// A line of text.
    pub fn label(&mut self, text: impl Into<RichText>) {
        let text = text.into();
        match self {
            Self::Egui(ui) => {
                ui.label(text);
            }
            Self::Record(rec) => {
                if let Some(menu) = rec.stack.last_mut() {
                    menu.push(MenuNode::Note(text.text().to_string()));
                }
            }
        }
    }

    pub fn separator(&mut self) {
        match self {
            Self::Egui(ui) => {
                ui.separator();
            }
            Self::Record(rec) => {
                if let Some(menu) = rec.stack.last_mut()
                    && !matches!(menu.last(), None | Some(MenuNode::Separator))
                {
                    menu.push(MenuNode::Separator);
                }
            }
        }
    }

    /// Close the open menu, as a chosen item does.
    pub fn close(&mut self) {
        if let Self::Egui(ui) = self {
            ui.close();
        }
    }

    /// Everything `add` draws, enabled or not.
    pub fn add_enabled_ui(&mut self, enabled: bool, add: impl FnOnce(&mut MenuUi<'_>)) {
        match self {
            Self::Egui(ui) => {
                ui.add_enabled_ui(enabled, |ui| add(&mut MenuUi::Egui(ui)));
            }
            Self::Record(rec) => {
                if !enabled {
                    rec.disabled += 1;
                }
                add(&mut MenuUi::Record(rec));
                if !enabled {
                    rec.disabled -= 1;
                }
            }
        }
    }

    /// The user's Animate scripts, one item each, and the items about them —
    /// the Commands menu's part of [`crate::jsfl_menu`].
    pub fn jsfl_commands(&mut self) {
        match self {
            Self::Egui(ui) => crate::jsfl_menu::show(ui),
            Self::Record(rec) => {
                use crate::jsfl_menu::JsflAction as J;
                let mut items: Vec<MenuNode> = Vec::new();
                for e in &rec.jsfl.entries {
                    items.push(MenuNode::Item {
                        label: e.label.clone(),
                        enabled: e.enabled,
                        checked: false,
                        action: Some(MenuAction::Jsfl(J::Run(e.name.clone()))),
                    });
                }
                if items.is_empty() {
                    items.push(MenuNode::Note("No Animate commands found".into()));
                }
                items.push(MenuNode::Separator);
                for (label, action) in [
                    ("Import Animate Commands\u{2026}", J::ImportCommands),
                    ("Import Animate Shortcuts\u{2026}", J::ImportShortcuts),
                    ("Command Settings\u{2026}", J::Settings(None)),
                ] {
                    items.push(MenuNode::Item { label: label.into(), enabled: true, checked: false, action: Some(MenuAction::Jsfl(action)) });
                }
                if let Some(menu) = rec.stack.last_mut() {
                    menu.extend(items);
                }
            }
        }
    }

    /// A shortcut as the menu shows it ("⌘S"); none for the recorder, whose
    /// menus leave the keys to the window (see the module docs of
    /// `buzz-app`'s `native_menu`).
    pub fn shortcut(&self, shortcut: Option<egui::KeyboardShortcut>) -> String {
        match self {
            Self::Egui(ui) => crate::command::format_shortcut(ui.ctx(), shortcut),
            Self::Record(_) => String::new(),
        }
    }

    /// [`Self::shortcut`] for a command's own key.
    pub fn command_shortcut(&self, command: Command) -> String {
        match self {
            Self::Egui(ui) => crate::command::shortcut_text(ui.ctx(), command),
            Self::Record(_) => String::new(),
        }
    }
}

/// Walk `build` (a whole menu bar, written against [`MenuUi`], returning what
/// was raised) into its menus, each button knowing its command. See the
/// module docs.
pub fn record_menus(jsfl: &crate::jsfl_menu::JsflMenu, mut build: impl FnMut(&mut MenuUi<'_>) -> Vec<Command>) -> Vec<MenuNode> {
    let walk = |build: &mut dyn FnMut(&mut MenuUi<'_>) -> Vec<Command>, click: Option<usize>| {
        let mut rec = Recorder { stack: vec![Vec::new()], click, jsfl: jsfl.clone(), ..Default::default() };
        let raised = build(&mut MenuUi::Record(&mut rec));
        (rec.stack.pop().unwrap_or_default(), rec.count, raised)
    };
    let (mut menus, count, _) = walk(&mut build, None);
    let commands: Vec<Option<Command>> = (0..count).map(|n| walk(&mut build, Some(n)).2.first().copied()).collect();
    let mut next = 0;
    fill(&mut menus, &commands, &mut next);
    menus
}

/// Give each recorded button, in the order they were met, its command.
fn fill(nodes: &mut [MenuNode], commands: &[Option<Command>], next: &mut usize) {
    for node in nodes {
        match node {
            MenuNode::Menu { items, .. } => fill(items, commands, next),
            // Script items were recorded knowing what they do; the rest were
            // numbered as they were met.
            MenuNode::Item { action: Some(_), .. } => {}
            MenuNode::Item { action, .. } => {
                *action = commands.get(*next).copied().flatten().map(MenuAction::Command);
                *next += 1;
            }
            _ => {}
        }
    }
}

/// The recorded menus with every command blanked: what changes when an item
/// is enabled, ticked or added — cheap to compare each frame, so the costly
/// [`record_menus`] runs only when the menus actually change.
pub fn record_shape(jsfl: &crate::jsfl_menu::JsflMenu, mut build: impl FnMut(&mut MenuUi<'_>) -> Vec<Command>) -> Vec<MenuNode> {
    let mut rec = Recorder { stack: vec![Vec::new()], jsfl: jsfl.clone(), ..Default::default() };
    let _ = build(&mut MenuUi::Record(&mut rec));
    rec.stack.pop().unwrap_or_default()
}
