//! The Actions panel — where a script is written and run.
//!
//! Animate's Actions panel is an editor with an Output panel underneath it, and
//! that pairing is the whole ergonomics of scripting: you write, you run, and
//! what the script said appears immediately below where you typed it. Splitting
//! them across two docks would mean the error message is somewhere the user is
//! not looking at the moment it appears.
//!
//! # This panel does not run anything
//!
//! It holds text and reports that the user pressed Run. The engine lives in
//! `buzz-script` and is driven by the editor, which is what owns the document
//! and can therefore commit a run as one undo step. Keeping the panel ignorant
//! of the engine is what lets the whole thing be tested without a JavaScript
//! runtime — and keeps `buzz-ui` free of one.

use egui::{Color32, RichText, Ui};

/// Everything the panel remembers between frames.
///
/// View state, not document state: a script in the box is not part of the
/// artwork, is not saved with it and is not undone by Ctrl+Z.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActionsState {
    // Whether the panel is on screen is the *workspace's* business — see
    // `buzz_ui::Workspace`. It used to be a flag here as well, and two answers
    // to "is this panel open" is how a panel ends up shown in one place and
    // hidden in the other.
    /// What the user has typed.
    pub source: String,
    /// Lines the last run passed to `fl.trace`.
    pub output: Vec<String>,
    /// The last run's failure, already worded for a person.
    pub error: Option<String>,
    /// One line about the last run, shown next to the Run button.
    pub summary: Option<String>,
}

impl ActionsState {
    /// Record what a run produced.
    pub fn report(&mut self, output: Vec<String>, error: Option<String>, summary: String) {
        self.output = output;
        self.error = error;
        self.summary = Some(summary);
    }

    /// Empty the Output area, as Animate's `fl.outputPanel.clear()` does.
    pub fn clear_output(&mut self) {
        self.output.clear();
        self.error = None;
        self.summary = None;
    }

    /// Is there anything worth running?
    pub fn has_source(&self) -> bool {
        !self.source.trim().is_empty()
    }
}

/// One script the panel can offer to load, named for a menu.
///
/// Mirrors `buzz_script::Sample` without depending on it — the panel takes
/// whatever the caller hands it, so `buzz-ui` needs no script engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleEntry {
    pub name: &'static str,
    pub summary: &'static str,
    pub source: &'static str,
}

/// What the user did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ActionsResponse {
    /// Run what is in the box.
    pub run: bool,
}

/// Draw the Actions panel.
pub fn actions_panel(
    ui: &mut Ui,
    state: &mut ActionsState,
    samples: &[SampleEntry],
) -> ActionsResponse {
    let mut response = ActionsResponse::default();

    ui.horizontal(|ui| {
        ui.heading("Actions");
        ui.label(RichText::new("JavaScript").small().weak());
    });

    // -- the toolbar --------------------------------------------------------
    ui.horizontal(|ui| {
        let run = ui
            .add_enabled(state.has_source(), egui::Button::new("▶ Run"))
            .on_hover_text("Run the script against this document (Ctrl+Enter)");
        if run.clicked() {
            response.run = true;
        }

        // `⏷` rather than the obvious `▼`: egui's bundled fonts have no glyph
        // for U+25BC and draw an empty box, which a screenshot of this very
        // panel caught. U+23F7 and `▶` above were both confirmed the same way.
        ui.menu_button("Examples ⏷", |ui| {
            for sample in samples {
                if ui
                    .button(sample.name)
                    .on_hover_text(sample.summary)
                    .clicked()
                {
                    // Replacing rather than appending: appending would run the
                    // previous script again as well, which is never what
                    // picking an example means.
                    state.source = sample.source.to_string();
                    state.clear_output();
                    ui.close();
                }
            }
        });

        if ui
            .button("Clear")
            .on_hover_text("Empty the Output area")
            .clicked()
        {
            state.clear_output();
        }

        if let Some(summary) = &state.summary {
            ui.label(RichText::new(summary).small().weak());
        }
    });

    ui.add_space(2.0);

    // Script on the left, Output on the right. The panel is docked along the
    // bottom, so it is wide and short: stacking the two would leave three
    // visible lines of each. Side by side, the error appears level with the
    // code that caused it.
    ui.columns(2, |columns| {
        source_editor(&mut columns[0], state);
        output_area(&mut columns[1], state);
    });

    response
}

/// The code box.
///
/// `code_editor` gives a monospace font and turns off the word wrapping that
/// makes prose editing pleasant and code editing unpleasant. Tab is left to
/// egui's focus handling, so the panel can still be left by keyboard.
fn source_editor(ui: &mut Ui, state: &mut ActionsState) {
    ui.label(RichText::new("Script").small().strong());
    egui::ScrollArea::vertical()
        .id_salt("actions_source")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut state.source)
                    .code_editor()
                    .desired_rows(8)
                    .desired_width(f32::INFINITY)
                    .hint_text("var d = fl.getDocumentDOM();\nfl.trace(d.width);"),
            );
        });
}

/// Everything the last run said.
fn output_area(ui: &mut Ui, state: &ActionsState) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Output").small().strong());
        if state.output.is_empty() && state.error.is_none() {
            ui.label(RichText::new("nothing yet").small().weak());
        }
    });

    egui::ScrollArea::vertical()
        .id_salt("actions_output")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &state.output {
                ui.label(RichText::new(line).monospace().small());
            }
            if let Some(error) = &state.error {
                // The failure goes last, after whatever the script managed to
                // trace before it — which is usually how you find where it got
                // to. Red rather than a leading "Error:", because the eye finds
                // the colour first in a wall of monospace.
                ui.label(
                    RichText::new(error)
                        .monospace()
                        .small()
                        .color(Color32::from_rgb(0xE0, 0x5A, 0x4A)),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_panel_has_nothing_to_run() {
        let mut state = ActionsState::default();
        assert!(!state.has_source());

        state.source = "   \n\t ".to_string();
        assert!(!state.has_source(), "whitespace is not a script");

        state.source = "fl.trace(1);".to_string();
        assert!(state.has_source());
    }

    #[test]
    fn reporting_a_run_replaces_the_previous_output() {
        let mut state = ActionsState::default();
        state.report(vec!["first".into()], None, "ok".into());
        assert_eq!(state.output, vec!["first".to_string()]);

        state.report(vec!["second".into()], Some("boom".into()), "failed".into());
        assert_eq!(state.output, vec!["second".to_string()]);
        assert_eq!(state.error.as_deref(), Some("boom"));
    }

    #[test]
    fn clearing_leaves_the_script_alone() {
        let mut state = ActionsState {
            source: "fl.trace(1);".into(),
            ..ActionsState::default()
        };
        state.report(vec!["1".into()], Some("boom".into()), "failed".into());
        state.clear_output();

        assert!(state.output.is_empty());
        assert!(state.error.is_none());
        assert!(state.summary.is_none());
        assert_eq!(
            state.source, "fl.trace(1);",
            "clearing output is not clearing the script"
        );
    }
}
