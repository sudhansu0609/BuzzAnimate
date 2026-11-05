//! **The small dialogs a running script opens** (JSFL_COMMANDS_PLAN J6):
//! `prompt`, `confirm`, XMLUI panels (`fl.xmlPanel`) and the file picker
//! (`fl.browseForFileURL`).
//!
//! The script runs on a thread of its own and **waits** for the answer: its
//! question comes over a channel to the UI thread, which draws it as a modal
//! over the "Running script" overlay; the answer goes back the same way and the
//! script carries on from the line that asked. Closing a dialog (Cancel, Esc) is
//! a cancel -- `prompt` gives `null`, `confirm` false, a panel `dismiss ==
//! "cancel"`; Stop on the overlay ends the wait too.
//!
//! [`ScriptDialogs::pending`] and [`ScriptDialogs::answer`] are the test hook: a
//! headless test reads what the script asked and answers it without a window.

use std::sync::Arc;
use std::time::Duration;

use buzz_script::xmlui::{Control, ControlKind, Form};
use buzz_script::{Answer, Question};
use crossbeam_channel::{Receiver, Sender};

/// A question on its way to the UI thread, with the way back.
pub type Asked = (Question, Sender<Answer>);

/// What the dialog on screen holds while a person edits it.
struct Open {
    question: Question,
    reply: Sender<Answer>,
    /// The prompt's text.
    text: String,
    /// One value per control of an XMLUI panel.
    values: Vec<String>,
    focused: bool,
}

/// The dialogs a script has asked for, and the one on screen.
pub struct ScriptDialogs {
    tx: Sender<Asked>,
    rx: Receiver<Asked>,
    open: Option<Open>,
}

impl Default for ScriptDialogs {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self { tx, rx, open: None }
    }
}

/// The hook a script run is given: asks over `tx` and blocks for the answer,
/// giving up (as a cancel) when `cancelled` says the run was stopped. A file
/// picker is the system's own dialog, opened right there on the script's thread.
pub fn ask_hook(tx: Sender<Asked>, cancelled: Arc<dyn Fn() -> bool + Send + Sync>) -> buzz_script::AskFn {
    Arc::new(move |question| {
        if let Question::PickFile { title, save } = &question {
            let dialog = rfd::FileDialog::new().set_title(title.as_str());
            let picked = if *save { dialog.save_file() } else { dialog.pick_file() };
            return picked.map(Answer::File).unwrap_or(Answer::Cancelled);
        }
        let (reply, answer) = crossbeam_channel::bounded(1);
        if tx.send((question, reply)).is_err() {
            return Answer::Cancelled;
        }
        loop {
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(a) => return a,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if cancelled() {
                        return Answer::Cancelled;
                    }
                }
                Err(_) => return Answer::Cancelled,
            }
        }
    })
}

impl ScriptDialogs {
    /// Where a script run sends its questions.
    pub fn sender(&self) -> Sender<Asked> {
        self.tx.clone()
    }

    /// Take the next question if none is showing.
    pub fn pump(&mut self) {
        if self.open.is_some() {
            return;
        }
        if let Ok((question, reply)) = self.rx.try_recv() {
            let (text, values) = match &question {
                Question::Prompt { default, .. } => (default.clone(), Vec::new()),
                Question::Form(f) => (String::new(), f.controls.iter().map(|c| c.value.clone()).collect()),
                _ => (String::new(), Vec::new()),
            };
            self.open = Some(Open { question, reply, text, values, focused: false });
        }
    }

    /// The question on screen, if there is one. (Test hook.)
    pub fn pending(&self) -> Option<&Question> {
        self.open.as_ref().map(|o| &o.question)
    }

    /// Answer the question on screen, as the buttons do. (Test hook, and the
    /// buttons' own path.)
    pub fn answer(&mut self, answer: Answer) {
        if let Some(open) = self.open.take() {
            let _ = open.reply.send(answer);
        }
    }

    /// Close whatever is open, and every question still waiting, as cancelled.
    pub fn close(&mut self) {
        self.answer(Answer::Cancelled);
        while let Ok((_, reply)) = self.rx.try_recv() {
            let _ = reply.send(Answer::Cancelled);
        }
    }

    /// What pressing OK / Accept would answer with the dialog as it stands.
    pub fn current_answer(&self) -> Option<Answer> {
        let open = self.open.as_ref()?;
        Some(match &open.question {
            Question::Prompt { .. } => Answer::Text(open.text.clone()),
            Question::Confirm { .. } => Answer::Yes(true),
            Question::Form(f) => Answer::Form(form_values(f, &open.values)),
            Question::PickFile { .. } => Answer::Cancelled,
        })
    }

    /// Draw the dialog, if a question is open, and send what was chosen.
    pub fn show(&mut self, ctx: &egui::Context) {
        self.pump();
        let Some(open) = self.open.as_mut() else { return };
        let mut chosen: Option<bool> = None; // Some(true) = OK / yes, Some(false) = cancel / no
        let mut yes_no = false;
        egui::Modal::new(egui::Id::new("script-question")).show(ctx, |ui| {
            ui.set_width(380.0);
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            match &open.question {
                Question::Prompt { message, .. } => {
                    ui.heading("Script asks");
                    ui.add_space(6.0);
                    ui.label(message);
                    ui.add_space(6.0);
                    let edit = ui.add(egui::TextEdit::singleline(&mut open.text).desired_width(f32::INFINITY));
                    if !open.focused {
                        edit.request_focus();
                        open.focused = true;
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || enter {
                            chosen = Some(true);
                        }
                        if ui.button("Cancel").clicked() || escape {
                            chosen = Some(false);
                        }
                    });
                }
                Question::Confirm { message } => {
                    yes_no = true;
                    ui.heading("Script asks");
                    ui.add_space(6.0);
                    ui.label(message);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Yes").clicked() {
                            chosen = Some(true);
                        }
                        if ui.button("No").clicked() || escape {
                            chosen = Some(false);
                        }
                    });
                }
                Question::Form(form) => {
                    if !form.title.is_empty() {
                        ui.heading(&form.title);
                        ui.add_space(6.0);
                    }
                    let mut enter = false;
                    for (control, value) in form.controls.iter().zip(open.values.iter_mut()) {
                        enter |= control_ui(ui, control, value);
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if form.buttons.iter().any(|b| b == "accept") && (ui.button("OK").clicked() || enter) {
                            chosen = Some(true);
                        }
                        if form.buttons.iter().any(|b| b == "cancel") && (ui.button("Cancel").clicked() || escape) {
                            chosen = Some(false);
                        }
                    });
                }
                Question::PickFile { .. } => chosen = Some(false),
            }
        });
        match chosen {
            None => {}
            Some(true) => {
                if let Some(a) = self.current_answer() {
                    self.answer(a);
                }
            }
            Some(false) => self.answer(if yes_no { Answer::Yes(false) } else { Answer::Cancelled }),
        }
    }
}

/// `(id, value)` of every control that has an id.
fn form_values(form: &Form, values: &[String]) -> Vec<(String, String)> {
    form.controls
        .iter()
        .zip(values)
        .filter(|(c, _)| !c.id.is_empty() && c.kind != ControlKind::Label)
        .map(|(c, v)| (c.id.clone(), v.clone()))
        .collect()
}

/// One control of an XMLUI panel. True when Enter was pressed in a text box.
fn control_ui(ui: &mut egui::Ui, control: &Control, value: &mut String) -> bool {
    let mut enter = false;
    match control.kind {
        ControlKind::Label => {
            ui.label(&control.label);
        }
        ControlKind::Text => {
            ui.horizontal(|ui| {
                if !control.label.is_empty() {
                    ui.label(&control.label);
                }
                let edit = ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
                enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            });
        }
        ControlKind::Number => {
            ui.horizontal(|ui| {
                if !control.label.is_empty() {
                    ui.label(&control.label);
                }
                let mut n: f64 = value.parse().unwrap_or(control.min);
                if ui.add(egui::Slider::new(&mut n, control.min..=control.max)).changed() {
                    *value = format!("{n}");
                }
            });
        }
        ControlKind::Checkbox => {
            let mut on = value == "true";
            if ui.checkbox(&mut on, &control.label).changed() {
                *value = on.to_string();
            }
        }
        ControlKind::Dropdown => {
            ui.horizontal(|ui| {
                if !control.label.is_empty() {
                    ui.label(&control.label);
                }
                let shown = control
                    .options
                    .iter()
                    .find(|(_, v)| v == value)
                    .map(|(l, _)| l.clone())
                    .unwrap_or_else(|| value.clone());
                egui::ComboBox::from_id_salt(("xmlui", &control.id))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (label, v) in &control.options {
                            ui.selectable_value(value, v.clone(), label);
                        }
                    });
            });
        }
        ControlKind::Radio => {
            if !control.label.is_empty() {
                ui.label(&control.label);
            }
            for (label, v) in &control.options {
                ui.radio_value(value, v.clone(), label);
            }
        }
    }
    enter
}
