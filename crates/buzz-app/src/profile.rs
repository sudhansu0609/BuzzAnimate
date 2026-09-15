//! The frame watchdog.
//!
//! The Never-Hang Wave is only true if it *stays* true, and the way an
//! O(document) cost creeps back is quietly: one section of the frame grows with
//! the file until a big document stutters, long before anyone writes a test for
//! it. This times the frame's sections, keeps a short history for the debug HUD,
//! and says so — once, not every frame — when one blows the budget.
//!
//! It is always on. A handful of [`std::time::Instant`] reads a frame is
//! nothing next to what they measure, and a warning that only appears when it is
//! deserved is worth far more than the nanoseconds it costs.

use std::time::{Duration, Instant};

/// The ~4 ms a UI-thread frame section is allowed, from `ARCHITECTURE.md` §0.
pub const BUDGET: Duration = Duration::from_millis(4);

/// How many frames of history the HUD shows.
const HISTORY: usize = 120;

/// The named sections of a frame, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// egui layout, tessellation and the panels.
    Ui,
    /// Encoding the stage into the Vello scene.
    Encode,
    /// Installing off-thread results and lighting bookkeeping.
    Lights,
    /// GPU submission and present.
    Present,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Ui,
        Section::Encode,
        Section::Lights,
        Section::Present,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Ui => "ui",
            Section::Encode => "encode",
            Section::Lights => "lights",
            Section::Present => "present",
        }
    }

    fn index(self) -> usize {
        match self {
            Section::Ui => 0,
            Section::Encode => 1,
            Section::Lights => 2,
            Section::Present => 3,
        }
    }
}

/// One frame's timings.
#[derive(Debug, Clone, Copy, Default)]
pub struct Frame {
    sections: [Duration; 4],
}

impl Frame {
    pub fn section(&self, section: Section) -> Duration {
        self.sections[section.index()]
    }

    pub fn total(&self) -> Duration {
        self.sections.iter().copied().sum()
    }
}

/// Times a frame's sections, keeps a short history, and warns on a budget miss.
pub struct FrameProfiler {
    current: Frame,
    /// A `[Instant]` per section, set when it opens.
    open: Option<(Section, Instant)>,
    history: std::collections::VecDeque<Frame>,
    /// The last time a budget warning was emitted, so they are rate-limited to
    /// one a second rather than one a frame.
    last_warned: Option<Instant>,
}

impl Default for FrameProfiler {
    fn default() -> Self {
        Self {
            current: Frame::default(),
            open: None,
            history: std::collections::VecDeque::with_capacity(HISTORY),
            last_warned: None,
        }
    }
}

impl FrameProfiler {
    /// Begin a frame. Closes any section left open by a panic-free early return.
    pub fn begin_frame(&mut self) {
        self.open = None;
        self.current = Frame::default();
    }

    /// Open a section; the previously open one is closed and charged.
    pub fn enter(&mut self, section: Section) {
        let now = Instant::now();
        if let Some((prev, started)) = self.open.take() {
            self.current.sections[prev.index()] += now.duration_since(started);
        }
        self.open = Some((section, now));
    }

    /// Close the frame: charge the last open section, store the frame, and warn
    /// if any section ran over budget.
    pub fn end_frame(&mut self) {
        let now = Instant::now();
        if let Some((prev, started)) = self.open.take() {
            self.current.sections[prev.index()] += now.duration_since(started);
        }

        // The worst section this frame, for the warning.
        if let Some(worst) = Section::ALL
            .into_iter()
            .max_by_key(|s| self.current.section(*s))
            && self.current.section(worst) > BUDGET
        {
            let warn_now = self
                .last_warned
                .is_none_or(|t| now.duration_since(t) > Duration::from_secs(1));
            if warn_now {
                tracing::warn!(
                    "frame section '{}' ran {:.1} ms, over the {:.0} ms budget",
                    worst.label(),
                    self.current.section(worst).as_secs_f64() * 1000.0,
                    BUDGET.as_secs_f64() * 1000.0,
                );
                self.last_warned = Some(now);
            }
        }

        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(self.current);
    }

    /// The most recent completed frame, for the HUD.
    pub fn last(&self) -> Frame {
        self.history.back().copied().unwrap_or_default()
    }

    /// A compact one-line summary for the debug HUD.
    pub fn summary(&self) -> String {
        let f = self.last();
        let mut parts = Vec::new();
        for s in Section::ALL {
            parts.push(format!("{} {:.1}", s.label(), f.section(s).as_secs_f64() * 1000.0));
        }
        format!("{} | total {:.1} ms", parts.join("  "), f.total().as_secs_f64() * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_charged_to_the_one_that_was_open() {
        let mut p = FrameProfiler::default();
        p.begin_frame();
        p.enter(Section::Ui);
        std::thread::sleep(Duration::from_millis(2));
        p.enter(Section::Encode);
        std::thread::sleep(Duration::from_millis(1));
        p.end_frame();

        let f = p.last();
        assert!(f.section(Section::Ui) >= Duration::from_millis(2));
        assert!(f.section(Section::Encode) >= Duration::from_millis(1));
        // Sections not entered stay zero.
        assert_eq!(f.section(Section::Present), Duration::ZERO);
        assert!(f.total() >= Duration::from_millis(3));
    }

    #[test]
    fn history_is_bounded() {
        let mut p = FrameProfiler::default();
        for _ in 0..(HISTORY + 50) {
            p.begin_frame();
            p.enter(Section::Ui);
            p.end_frame();
        }
        assert_eq!(p.history.len(), HISTORY);
    }
}
