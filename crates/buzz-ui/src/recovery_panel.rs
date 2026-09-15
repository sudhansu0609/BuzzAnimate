//! The recovery prompt: what was found after a crash, and what to do with it.
//!
//! # Why it asks
//!
//! Autosave writes a recovery copy beside the document rather than over it, so
//! nothing the user chose to keep is ever overwritten. The consequence is that
//! *something has to offer it back* — a recovery file nobody is told about is
//! an autosave that did not happen.
//!
//! # Why it does not open one automatically
//!
//! The recovery may be newer than the document and still not be what the user
//! wants: they may have deliberately closed without saving. Silently replacing
//! a document with a copy of unsaved changes is its own kind of data loss, so
//! this lists what was found, says when each was written, and lets the user
//! choose. "Later" leaves everything exactly where it is, and the prompt comes
//! back next launch.

use std::path::PathBuf;

use egui::{RichText, Ui};

/// One recovery file, described for the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEntry {
    pub path: PathBuf,
    /// The document it belongs to, if that file is still there.
    pub document: Option<PathBuf>,
    /// How long ago it was written, in seconds.
    pub age_seconds: u64,
}

impl RecoveryEntry {
    /// The name to show: the document's, or the recovery's own stem.
    pub fn title(&self) -> String {
        if let Some(document) = &self.document
            && let Some(name) = document.file_name()
        {
            return name.to_string_lossy().to_string();
        }
        let stem = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().replace(".recovery.buzz", ""))
            .unwrap_or_default();
        // Unsaved work is filed under `untitled-<process id>` so that two
        // sessions cannot overwrite each other's recoveries. That number means
        // nothing to the person reading the prompt.
        if stem.starts_with("untitled") {
            return "Untitled work".to_string();
        }
        if stem.is_empty() {
            "Untitled work".to_string()
        } else {
            stem
        }
    }

    /// "4 minutes ago", in the terms somebody would judge it by.
    pub fn when(&self) -> String {
        let s = self.age_seconds;
        let plural = |n: u64, unit: &str| {
            if n == 1 {
                format!("1 {unit} ago")
            } else {
                format!("{n} {unit}s ago")
            }
        };
        match s {
            0..=59 => "just now".to_string(),
            60..=3_599 => plural(s / 60, "minute"),
            3_600..=86_399 => plural(s / 3_600, "hour"),
            _ => plural(s / 86_400, "day"),
        }
    }

    /// Was this work never saved to a document at all?
    pub fn is_unsaved_work(&self) -> bool {
        self.document.is_none()
    }
}

/// Panel state: what was found, and whether it is being shown.
#[derive(Debug, Clone, Default)]
pub struct RecoveryState {
    pub found: Vec<RecoveryEntry>,
}

impl RecoveryState {
    pub fn is_open(&self) -> bool {
        !self.found.is_empty()
    }
}

/// What the user chose.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum RecoveryChoice {
    #[default]
    None,
    /// Open this recovery file.
    Recover(RecoveryEntry),
    /// Delete it.
    Discard(RecoveryEntry),
    /// Leave everything alone; ask again next launch.
    Later,
}

/// Draw the prompt. Does nothing when there is nothing to offer.
pub fn recovery_dialog(ctx: &egui::Context, state: &RecoveryState) -> RecoveryChoice {
    if !state.is_open() {
        return RecoveryChoice::None;
    }

    let mut choice = RecoveryChoice::None;
    egui::Window::new("Recover unsaved work")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| body(ui, state, &mut choice));
    choice
}

fn body(ui: &mut Ui, state: &RecoveryState, choice: &mut RecoveryChoice) {
    ui.set_min_width(420.0);
    ui.label(
        RichText::new(
            "BuzzAnimate did not close normally. These autosaves were found \u{2014} \
             each one is a document, and opening it changes nothing on disk until \
             you save.",
        )
        .small(),
    );
    ui.add_space(6.0);

    for entry in &state.found {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(entry.title()).strong());
                let what = if entry.is_unsaved_work() {
                    "never saved".to_string()
                } else {
                    "unsaved changes".to_string()
                };
                ui.label(
                    RichText::new(format!("{what} \u{00B7} {}", entry.when()))
                        .small()
                        .weak(),
                )
                .on_hover_text(entry.path.display().to_string());
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("Discard")
                    .on_hover_text("Delete this autosave")
                    .clicked()
                {
                    *choice = RecoveryChoice::Discard(entry.clone());
                }
                if ui
                    .button("Recover")
                    .on_hover_text("Open it as a document")
                    .clicked()
                {
                    *choice = RecoveryChoice::Recover(entry.clone());
                }
            });
        });
        ui.separator();
    }

    ui.horizontal(|ui| {
        if ui.button("Later").clicked() {
            *choice = RecoveryChoice::Later;
        }
        ui.label(
            RichText::new("nothing is deleted, and you will be asked again next time")
                .small()
                .weak()
                .italics(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(age: u64, document: bool) -> RecoveryEntry {
        RecoveryEntry {
            path: PathBuf::from("C:/work/Scene 1.recovery.buzz"),
            document: document.then(|| PathBuf::from("C:/work/Scene 1.buzz")),
            age_seconds: age,
        }
    }

    #[test]
    fn ages_read_the_way_somebody_would_judge_them() {
        assert_eq!(entry(0, true).when(), "just now");
        assert_eq!(entry(59, true).when(), "just now");
        assert_eq!(entry(60, true).when(), "1 minute ago");
        assert_eq!(entry(600, true).when(), "10 minutes ago");
        assert_eq!(entry(3_600, true).when(), "1 hour ago");
        assert_eq!(entry(90_000, true).when(), "1 day ago");
    }

    /// Work that was never saved is named after the recovery file, because
    /// there is no document to name it after \u2014 and it is the case that matters
    /// most, since there is nothing else on disk.
    #[test]
    fn unsaved_work_is_still_named_and_flagged() {
        let mut orphan = entry(120, false);
        orphan.path = PathBuf::from("C:/appdata/untitled-4821.recovery.buzz");
        assert!(orphan.is_unsaved_work());
        assert_eq!(
            orphan.title(),
            "Untitled work",
            "the process id in the filename means nothing to the reader"
        );
    }

    #[test]
    fn a_recovery_with_a_document_is_named_after_it() {
        assert_eq!(entry(30, true).title(), "Scene 1.buzz");
        assert!(!entry(30, true).is_unsaved_work());
    }

    /// Nothing found, nothing shown: the prompt must never appear on a clean
    /// launch.
    #[test]
    fn nothing_found_shows_nothing() {
        let state = RecoveryState::default();
        assert!(!state.is_open());

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut choice = RecoveryChoice::Later;
        let _ = ctx.run_ui(Default::default(), |ui| {
            choice = recovery_dialog(ui.ctx(), &state);
        });
        assert_eq!(choice, RecoveryChoice::None);
    }

    /// And it draws with several entries, including one that has no document.
    #[test]
    fn the_prompt_draws_what_it_found() {
        let state = RecoveryState {
            found: vec![entry(45, true), entry(9_000, false)],
        };
        assert!(state.is_open());

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = recovery_dialog(ui.ctx(), &state);
        });
    }
}
