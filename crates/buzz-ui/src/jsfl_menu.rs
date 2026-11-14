//! The user's Animate scripts on the Commands menu (JSFL_COMMANDS_PLAN J1).
//!
//! The list lives in the application (it scans folders and knows the keys); the
//! menu only draws what it is handed. The application leaves the list in the
//! egui context each frame and picks the click up from there, so the menu bar's
//! signature -- and every test that builds one -- stays as it is.

use egui::{Id, Ui};

/// One script, as the menu shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JsflMenuEntry {
    /// The script's name (what a click asks to run).
    pub name: String,
    pub label: String,
    /// The keys bound to it, already spelled for the screen.
    pub shortcut: String,
    pub enabled: bool,
    /// Hover text: why it is disabled, or what the script is.
    pub hint: String,
}

/// The whole list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JsflMenu {
    pub entries: Vec<JsflMenuEntry>,
}

/// What the user chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsflAction {
    Run(String),
    /// The settings window, on this script if one is named.
    Settings(Option<String>),
    ImportCommands,
    ImportShortcuts,
}

fn menu_id() -> Id {
    Id::new("buzz-jsfl-menu")
}
fn action_id() -> Id {
    Id::new("buzz-jsfl-action")
}
fn shown_id() -> Id {
    Id::new("buzz-jsfl-menu-shown")
}

/// Leave the list for the menu to draw this frame.
/// Ask for the scripts folder to be looked at again, as opening the menu does.
pub fn mark_shown(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(shown_id(), true));
}

/// A Commands-menu choice made somewhere other than this menu (the macOS
/// menu bar), handed to the window as a click here would be.
pub fn raise(ctx: &egui::Context, action: JsflAction) {
    ctx.data_mut(|d| d.insert_temp(action_id(), Some(action)));
}

/// The scripts as last published, for the macOS menu bar to list.
pub fn published(ctx: &egui::Context) -> JsflMenu {
    ctx.data(|d| d.get_temp(menu_id())).unwrap_or_default()
}

pub fn publish(ctx: &egui::Context, menu: JsflMenu) {
    ctx.data_mut(|d| d.insert_temp(menu_id(), menu));
}

/// Did the Commands menu draw its scripts since the last call? The application
/// rescans the folder when it did -- the list is current whenever it is looked at.
pub fn take_shown(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(shown_id())).unwrap_or(false)
}

/// The click, if there was one.
pub fn take_action(ctx: &egui::Context) -> Option<JsflAction> {
    ctx.data_mut(|d| d.remove_temp::<Option<JsflAction>>(action_id())).flatten()
}

/// Draw the scripts into the Commands menu being built in `ui`.
pub fn show(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    ctx.data_mut(|d| d.insert_temp(shown_id(), true));
    let menu: JsflMenu = ctx.data(|d| d.get_temp(menu_id())).unwrap_or_default();
    let mut raised: Option<JsflAction> = None;

    if menu.entries.is_empty() {
        ui.label(egui::RichText::new("No Animate commands found").small().weak());
    } else {
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for e in &menu.entries {
                ui.horizontal(|ui| {
                    let mut button = egui::Button::new(&e.label);
                    if !e.shortcut.is_empty() {
                        button = button.shortcut_text(e.shortcut.clone());
                    }
                    let response = ui.add_enabled(e.enabled, button);
                    let response = if e.hint.is_empty() {
                        response
                    } else {
                        response.on_hover_text(&e.hint).on_disabled_hover_text(&e.hint)
                    };
                    if response.clicked() {
                        raised = Some(JsflAction::Run(e.name.clone()));
                        ui.close();
                    }
                    if e.enabled
                        && ui
                            .small_button("\u{2699}")
                            .on_hover_text("Settings for this command")
                            .clicked()
                    {
                        raised = Some(JsflAction::Settings(Some(e.name.clone())));
                        ui.close();
                    }
                });
            }
        });
    }
    ui.separator();
    if ui.button("Import Animate Commands\u{2026}").clicked() {
        raised = Some(JsflAction::ImportCommands);
        ui.close();
    }
    if ui.button("Import Animate Shortcuts\u{2026}").clicked() {
        raised = Some(JsflAction::ImportShortcuts);
        ui.close();
    }
    if ui.button("Command Settings\u{2026}").clicked() {
        raised = Some(JsflAction::Settings(None));
        ui.close();
    }
    if let Some(a) = raised {
        ctx.data_mut(|d| d.insert_temp(action_id(), Some(a)));
    }
}
