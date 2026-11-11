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

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// When the running frame began, in ms since [`epoch`] plus one; 0 between
/// frames. Atomics only, so the UI thread never waits on the stall watchdog.
static FRAME_START: AtomicU64 = AtomicU64::new(0);
/// Index of the section the running frame is in; [`NO_SECTION`] before the first.
static CURRENT_SECTION: AtomicU8 = AtomicU8::new(NO_SECTION);
const NO_SECTION: u8 = u8::MAX;
/// A frame running longer than this is a stall worth a log line.
const STALL_AFTER: Duration = Duration::from_secs(5);
/// A stall that goes on is repeated this often, not every second.
const STALL_REPEAT: Duration = Duration::from_secs(30);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// **Say where the UI thread is stuck.** The per-section warning above only
/// fires when a section *ends*, so a frame that never ends (a driver wait, a
/// deadlock) logs nothing at all. This thread reads the published section and
/// frame start once a second and warns while a frame overruns.
fn start_stall_watchdog() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new().name("ui-stall-watchdog".into()).spawn(|| {
            let mut warned_frame = 0u64;
            let mut last_warned = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let start = FRAME_START.load(Ordering::Relaxed);
                if start == 0 {
                    continue;
                }
                let running = epoch().elapsed().saturating_sub(Duration::from_millis(start - 1));
                if running <= STALL_AFTER {
                    continue;
                }
                if start != warned_frame || last_warned.elapsed() >= STALL_REPEAT {
                    let label = Section::ALL
                        .get(CURRENT_SECTION.load(Ordering::Relaxed) as usize)
                        .map_or("before the first section", |s| s.label());
                    tracing::warn!("UI stalled {} s in section '{}'", running.as_secs(), label);
                    warned_frame = start;
                    last_warned = Instant::now();
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("could not start the UI stall watchdog: {e}");
        }
    });
}

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
        start_stall_watchdog();
        CURRENT_SECTION.store(NO_SECTION, Ordering::Relaxed);
        FRAME_START.store(epoch().elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
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
        CURRENT_SECTION.store(section.index() as u8, Ordering::Relaxed);
    }

    /// Close the frame: charge the last open section, store the frame, and warn
    /// if any section ran over budget.
    pub fn end_frame(&mut self) {
        FRAME_START.store(0, Ordering::Relaxed);
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
            parts.push(format!(
                "{} {:.1}",
                s.label(),
                f.section(s).as_secs_f64() * 1000.0
            ));
        }
        format!(
            "{} | total {:.1} ms",
            parts.join("  "),
            f.total().as_secs_f64() * 1000.0
        )
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
