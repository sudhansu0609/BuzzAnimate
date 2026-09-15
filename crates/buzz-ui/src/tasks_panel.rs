//! The Tasks panel: one place to see what the program is doing.
//!
//! Exports, imports, background opens, asset scans, thumbnail batches — every
//! long job the [task registry](buzz_app) runs shows up here with a progress
//! bar and, where it can be stopped, a Cancel. It is **global**: it belongs to
//! the program rather than to the open document, so `File ▸ New` in the middle
//! of an export changes nothing about it.
//!
//! Like every other panel here it only *reports*. It raises a [`TaskAction`]
//! when the user asks to cancel a job or reveal a finished file, and the shell
//! — which owns the registry and the queue — carries it out.

use egui::{RichText, Ui};

/// One running job, as the panel needs to draw it.
#[derive(Debug, Clone)]
pub struct TaskRow {
    /// Opaque handle, echoed back in [`TaskAction::Cancel`].
    pub id: u64,
    /// What kind of work — "Export", "Import", "Script"…
    pub kind: String,
    /// What it is working on — a file name, usually.
    pub label: String,
    /// `0.0..=1.0`, or `None` for work whose length is not known, which draws
    /// as an indeterminate bar rather than an empty one.
    pub progress: Option<f32>,
    /// A line under the bar: "frame 30 of 120", "reading shot.fla"…
    pub detail: String,
    pub elapsed_secs: f64,
    /// Whether this job can be stopped. A thumbnail batch cannot; an export can.
    pub can_cancel: bool,
}

/// One finished export, kept so it can be revealed.
#[derive(Debug, Clone)]
pub struct FinishedRow {
    /// Opaque handle, echoed back in [`TaskAction::Reveal`].
    pub id: u64,
    pub label: String,
    pub ok: bool,
    pub message: String,
}

/// What the panel shows this frame.
pub struct TasksView<'a> {
    pub running: &'a [TaskRow],
    pub finished: &'a [FinishedRow],
    /// Exports queued behind the one running, if any.
    pub queued: usize,
}

/// What the user asked the panel to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAction {
    /// Stop this running job.
    Cancel(u64),
    /// Show this finished file in its folder.
    Reveal(u64),
}

/// Draw the panel and return whatever the user asked for.
pub fn tasks_panel(ui: &mut Ui, view: &TasksView) -> Option<TaskAction> {
    let mut action = None;

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.heading("Tasks");
        if view.queued > 0 {
            ui.label(
                RichText::new(format!("· {} queued", view.queued))
                    .weak()
                    .small(),
            );
        }
    });
    ui.separator();

    if view.running.is_empty() && view.finished.is_empty() {
        ui.add_space(8.0);
        ui.weak("Nothing running.");
        ui.weak("Exports, imports and other background work appear here.");
        return None;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in view.running {
                if let Some(a) = running_row(ui, row) {
                    action = Some(a);
                }
                ui.add_space(6.0);
            }

            if !view.finished.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Finished").weak().small());
                ui.separator();
                for row in view.finished {
                    if let Some(a) = finished_row(ui, row) {
                        action = Some(a);
                    }
                }
            }
        });

    action
}

fn running_row(ui: &mut Ui, row: &TaskRow) -> Option<TaskAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        ui.label(RichText::new(&row.kind).strong());
        ui.label(RichText::new(&row.label).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{:.0}s", row.elapsed_secs))
                    .weak()
                    .monospace()
                    .small(),
            );
            if row.can_cancel && ui.small_button("Stop").clicked() {
                action = Some(TaskAction::Cancel(row.id));
            }
        });
    });

    match row.progress {
        Some(fraction) => {
            ui.add(egui::ProgressBar::new(fraction).desired_height(6.0));
        }
        None => {
            // Length unknown: a moving bar says "working" without lying about
            // how far along it is.
            ui.add(egui::ProgressBar::new(0.0).desired_height(6.0).animate(true));
        }
    }
    if !row.detail.is_empty() {
        ui.label(RichText::new(&row.detail).weak().small());
    }

    action
}

fn finished_row(ui: &mut Ui, row: &FinishedRow) -> Option<TaskAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        let mark = if row.ok { "\u{2713}" } else { "\u{2717}" };
        let colour = if row.ok {
            egui::Color32::from_rgb(0x4C, 0xAF, 0x50)
        } else {
            egui::Color32::from_rgb(0xE5, 0x53, 0x53)
        };
        ui.label(RichText::new(mark).color(colour).strong());
        ui.label(&row.label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if row.ok && ui.small_button("Reveal").clicked() {
                action = Some(TaskAction::Reveal(row.id));
            }
        });
    })
    .response
    .on_hover_text(&row.message);
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_with_no_total_is_indeterminate() {
        // Not a rendering test — just the invariant the drawing relies on: a
        // task with no known total carries `None`, not a misleading zero.
        let row = TaskRow {
            id: 1,
            kind: "Import".into(),
            label: "big.fla".into(),
            progress: None,
            detail: "reading".into(),
            elapsed_secs: 2.0,
            can_cancel: false,
        };
        assert!(row.progress.is_none());
    }
}
