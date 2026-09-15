//! Somewhere for long work to live.
//!
//! # The rule this exists to keep
//!
//! > **The window must never stop responding.** Not for a script, not for an
//! > export, not for a heavy first frame, not for a file dialog.
//!
//! Six things break that rule, and each is the same shape: work that can take
//! longer than a frame, started from the UI thread and finished there. The fix
//! is always the same too — hand the work an owned copy of what it needs, run
//! it somewhere else, and let it report back over a channel. This is the
//! place that does the running and the reporting.
//!
//! # The rules, stated once
//!
//! 1. **No closure on the UI thread may exceed ~4 ms.** A frame is 16.7 ms and
//!    egui has to lay out, tessellate and encode inside it. Anything that
//!    *can* take longer comes through here.
//! 2. **Owned snapshot in, message out.** A `Scene` is a copy-on-write tree of
//!    `Arc`s, so a snapshot is pointer copies. No task holds a reference into
//!    live document state.
//! 3. **Cancel is observed within 100 ms.** A cancel button nobody notices for
//!    eight seconds is not a cancel button.
//! 4. **No blocking OS dialog on the UI thread. Ever.**
//! 5. **Commit on complete.** A finished task's result is applied on the next
//!    frame through `Document::edit`, so it lands as one ordinary undo step.
//! 6. **No work on rayon's global pool.** Use [`buzz_jobs::Pool`].
//!
//! # Why the registry is owned by `App` and not by `Editor`
//!
//! This is a bug fix as much as a design. The export job was already a field
//! of `App` while its progress dialog was a field of `Editor` — and opening a
//! document builds a fresh `Editor`. So the dialog was destroyed while the job
//! kept running: progress vanished, the Cancel button became unreachable, and
//! the completion message landed in the *new* document's status bar.
//!
//! Work that outlives a document must be owned by something that outlives a
//! document.
//!
//! # Why not `JobSystem`
//!
//! Its two pools are rayon pools, sized for data-parallel bursts. A
//! minutes-long export would squat on one of the six background workers and
//! starve the only other thing that uses that pool — autosave. Tasks that own
//! a GPU device and block on it want a thread of their own; short fan-out
//! wants a pool. Both are available here, and [`TaskKind`] is how a caller
//! says which it is.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use buzz_jobs::{CancelToken, JobSystem, Pool};
use crossbeam_channel::{Receiver, Sender};

/// Identifies one running task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(pub u64);

/// What a task is, for the Tasks panel to name and for quit to reason about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Export,
    ConcatFilm,
    Import,
    Open,
    Script,
    AssetScan,
    Thumbnails,
    Resample,
}

impl TaskKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Export => "Export",
            Self::ConcatFilm => "Film",
            Self::Import => "Import",
            Self::Open => "Open",
            Self::Script => "Script",
            Self::AssetScan => "Assets",
            Self::Thumbnails => "Thumbnails",
            Self::Resample => "Resample",
        }
    }

    /// Can the user stop this from the Tasks panel?
    ///
    /// Everything holds a cancel token, but stopping some work makes no sense to
    /// offer: a thumbnail batch or an asset scan is redone for free, and the
    /// button would be a way to make the program do *less* than it already
    /// finished. Work with a real cost to lose — or a real wait to escape —
    /// gets the button.
    pub fn can_cancel(self) -> bool {
        matches!(
            self,
            Self::Export | Self::ConcatFilm | Self::Import | Self::Open | Self::Script
        )
    }

    /// **Would losing this to a quit destroy work?**
    ///
    /// An export half-written to disk is a broken file and minutes of GPU time
    /// thrown away. A thumbnail is redrawn in a frame and an asset scan is
    /// redone on the next launch, so neither is worth a prompt: asking about
    /// everything is how a prompt stops being read.
    pub fn blocks_quit(self) -> bool {
        matches!(self, Self::Export | Self::ConcatFilm)
    }
}

/// How far along, and doing what.
#[derive(Debug, Clone, Default)]
pub struct TaskProgress {
    pub done: u64,
    pub total: u64,
    pub detail: String,
}

impl TaskProgress {
    /// `0.0..=1.0`, or `None` when the total is not known — which a progress
    /// bar should draw as indeterminate rather than as zero.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f32 / self.total as f32).clamp(0.0, 1.0))
    }
}

/// How a task ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    Finished(String),
    Cancelled,
    Failed(String),
}

/// Where a running task reports to.
#[derive(Clone)]
pub struct ProgressSink(Arc<Mutex<TaskProgress>>);

impl ProgressSink {
    /// **A sink attached to nothing**, for work running outside the registry.
    ///
    /// A headless render has no Tasks panel to draw a bar in and no window to
    /// draw it on, but `run_export` reports through one regardless — so it gets
    /// somewhere to report that nobody reads. Cheaper and far less invasive
    /// than threading an `Option` through every task that already works.
    pub fn detached() -> Self {
        Self(Arc::new(Mutex::new(TaskProgress::default())))
    }

    /// What has been reported so far, for a caller polling it themselves.
    pub fn read(&self) -> (u64, u64) {
        let p = lock(&self.0);
        (p.done, p.total)
    }

    pub fn set(&self, done: u64, total: u64) {
        let mut p = lock(&self.0);
        p.done = done;
        p.total = total;
    }

    pub fn detail(&self, detail: impl Into<String>) {
        lock(&self.0).detail = detail.into();
    }
}

/// A task that panics mid-update would otherwise poison the lock and take the
/// panel's progress readout down with it. The worst a recovered lock can hold
/// is a half-written progress line, which is exactly what we want to show.
fn lock(m: &Mutex<TaskProgress>) -> std::sync::MutexGuard<'_, TaskProgress> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a task closure is handed: how to report, and how to be stopped.
#[derive(Clone)]
pub struct TaskCtx {
    pub cancel: CancelToken,
    pub progress: ProgressSink,
}

impl TaskCtx {
    /// Shorthand for the check every task loop has to make.
    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// One piece of long work, as the registry sees it.
pub struct Task {
    pub id: TaskId,
    pub kind: TaskKind,
    pub label: String,
    pub cancel: CancelToken,
    pub started: Instant,
    progress: Arc<Mutex<TaskProgress>>,
    /// `Some` for work on a thread of its own; `None` for pool work, which
    /// rayon owns and which cannot be joined.
    join: Option<std::thread::JoinHandle<()>>,
    done: Receiver<TaskOutcome>,
    /// Set once the outcome has been taken, so `poll` reports it once.
    finished: bool,
}

impl Task {
    pub fn progress(&self) -> TaskProgress {
        lock(&self.progress).clone()
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}

/// Every long-running piece of work in the program.
#[derive(Default)]
pub struct TaskRegistry {
    tasks: Vec<Task>,
    next: u64,
}

impl TaskRegistry {
    /// Start work on a thread of its own.
    ///
    /// For work that owns something expensive for its whole life — an export
    /// owns a GPU device — or that blocks on it. A pool worker doing either
    /// would be a pool worker nobody else can have.
    pub fn spawn_thread<F>(&mut self, kind: TaskKind, label: impl Into<String>, f: F) -> TaskId
    where
        F: FnOnce(TaskCtx) -> TaskOutcome + Send + 'static,
    {
        let (ctx, cancel, progress, tx, rx) = self.prepare();
        let name = format!("buzz-{}", kind.label().to_lowercase());
        let join = std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                let outcome = f(ctx);
                // The receiver is gone if the registry dropped the task; that
                // is not an error, it is nobody listening any more.
                let _ = tx.send(outcome);
            })
            .ok();
        self.register(kind, label, cancel, progress, join, rx)
    }

    /// Start work on one of the job pools.
    ///
    /// For work that is short or fans out — an asset rescan, a batch of
    /// thumbnails. Cannot be joined, so it never blocks quit.
    pub fn spawn_pool<F>(
        &mut self,
        jobs: &JobSystem,
        pool: Pool,
        kind: TaskKind,
        label: impl Into<String>,
        f: F,
    ) -> TaskId
    where
        F: FnOnce(TaskCtx) -> TaskOutcome + Send + 'static,
    {
        let (ctx, cancel, progress, tx, rx) = self.prepare();
        jobs.spawn(pool, move || {
            let outcome = f(ctx);
            let _ = tx.send(outcome);
        });
        self.register(kind, label, cancel, progress, None, rx)
    }

    fn prepare(
        &self,
    ) -> (
        TaskCtx,
        CancelToken,
        Arc<Mutex<TaskProgress>>,
        Sender<TaskOutcome>,
        Receiver<TaskOutcome>,
    ) {
        let cancel = CancelToken::new();
        let progress = Arc::new(Mutex::new(TaskProgress::default()));
        // Bounded at one: a task sends exactly one outcome, and an unbounded
        // channel here would only hide a task that sent twice.
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = TaskCtx {
            cancel: cancel.clone(),
            progress: ProgressSink(Arc::clone(&progress)),
        };
        (ctx, cancel, progress, tx, rx)
    }

    fn register(
        &mut self,
        kind: TaskKind,
        label: impl Into<String>,
        cancel: CancelToken,
        progress: Arc<Mutex<TaskProgress>>,
        join: Option<std::thread::JoinHandle<()>>,
        done: Receiver<TaskOutcome>,
    ) -> TaskId {
        self.next += 1;
        let id = TaskId(self.next);
        self.tasks.push(Task {
            id,
            kind,
            label: label.into(),
            cancel,
            started: Instant::now(),
            progress,
            join,
            done,
            finished: false,
        });
        id
    }

    /// Ask a task to stop. It stops when it next looks.
    pub fn cancel(&mut self, id: TaskId) {
        if let Some(task) = self.tasks.iter().find(|t| t.id == id) {
            task.cancel.cancel();
        }
    }

    pub fn cancel_all(&mut self) {
        for task in &self.tasks {
            task.cancel.cancel();
        }
    }

    /// Whatever finished since last time. Called once a frame.
    ///
    /// A task whose thread died without sending — a panic — is reported as
    /// `Failed` rather than left running for ever in the panel. Silence and
    /// still-working look identical from here, and only one of them is true.
    pub fn poll(&mut self) -> Vec<(TaskId, TaskKind, TaskOutcome)> {
        let mut out = Vec::new();
        for task in &mut self.tasks {
            if task.finished {
                continue;
            }
            match task.done.try_recv() {
                Ok(outcome) => {
                    task.finished = true;
                    out.push((task.id, task.kind, outcome));
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    task.finished = true;
                    out.push((
                        task.id,
                        task.kind,
                        TaskOutcome::Failed("the task stopped without saying why".into()),
                    ));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }
        // Finished tasks are joined and dropped, so a thread is never
        // abandoned while it is still writing.
        self.tasks.retain_mut(|task| {
            if !task.finished {
                return true;
            }
            if let Some(join) = task.join.take() {
                let _ = join.join();
            }
            false
        });
        out
    }

    pub fn running(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(|t| !t.finished)
    }

    pub fn is_running(&self, id: TaskId) -> bool {
        self.tasks.iter().any(|t| t.id == id && !t.finished)
    }

    pub fn len(&self) -> usize {
        self.running().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tasks that would lose real work if the program exited now.
    pub fn quit_blockers(&self) -> Vec<&Task> {
        self.running().filter(|t| t.kind.blocks_quit()).collect()
    }

    /// Wait for everything to stop, having asked it to.
    ///
    /// Used on the way out once the user has said to abandon whatever is
    /// running: an export that is cancelled still has a `.part` file to remove,
    /// and the process exiting first would leave it there.
    pub fn cancel_and_join(&mut self) {
        self.cancel_all();
        for task in &mut self.tasks {
            if let Some(join) = task.join.take() {
                let _ = join.join();
            }
        }
        self.tasks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> TaskRegistry {
        TaskRegistry::default()
    }

    /// Wait for `poll` to report something, or give up. Tests must not hang
    /// when the thing they are testing is broken.
    fn wait_for_outcome(registry: &mut TaskRegistry) -> Vec<(TaskId, TaskKind, TaskOutcome)> {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let finished = registry.poll();
            if !finished.is_empty() {
                return finished;
            }
            if Instant::now() > deadline {
                return Vec::new();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_task_runs_and_reports_back() {
        let mut registry = registry();
        let id = registry.spawn_thread(TaskKind::Open, "a file", |_| {
            TaskOutcome::Finished("opened".into())
        });
        assert!(registry.is_running(id) || !registry.is_empty());

        let finished = wait_for_outcome(&mut registry);
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].0, id);
        assert_eq!(finished[0].2, TaskOutcome::Finished("opened".into()));
        assert!(registry.is_empty(), "it should have been cleared away");
    }

    /// An outcome is reported **once**: a panel that saw the same completion
    /// on every frame would announce it for ever.
    #[test]
    fn an_outcome_is_reported_once() {
        let mut registry = registry();
        registry.spawn_thread(TaskKind::Open, "a file", |_| {
            TaskOutcome::Finished("done".into())
        });
        assert_eq!(wait_for_outcome(&mut registry).len(), 1);
        assert!(registry.poll().is_empty());
    }

    /// Progress reported from the task is visible from the UI side while it
    /// runs — the whole point of having a registry rather than a join handle.
    #[test]
    fn progress_is_visible_while_the_task_runs() {
        let (gate_tx, gate_rx) = crossbeam_channel::bounded::<()>(0);
        let mut registry = registry();
        registry.spawn_thread(TaskKind::Export, "a film", move |ctx| {
            ctx.progress.set(3, 10);
            ctx.progress.detail("frame 3 of 10");
            // Hold here until the test has looked.
            let _ = gate_rx.recv();
            TaskOutcome::Finished("done".into())
        });

        // Spin until the task has reported something.
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let seen = registry
                .running()
                .next()
                .map(|t| t.progress())
                .unwrap_or_default();
            if seen.done == 3 {
                assert_eq!(seen.total, 10);
                assert_eq!(seen.fraction(), Some(0.3));
                assert_eq!(seen.detail, "frame 3 of 10");
                break;
            }
            assert!(Instant::now() < deadline, "progress never arrived");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let _ = gate_tx.send(());
        wait_for_outcome(&mut registry);
    }

    /// **Cancel is observed by the task itself**, which is the only way it can
    /// stop cleanly enough to tidy up after itself.
    #[test]
    fn cancelling_stops_the_task() {
        let mut registry = registry();
        let id = registry.spawn_thread(TaskKind::Export, "a film", |ctx| {
            for _ in 0..100_000 {
                if ctx.cancelled() {
                    return TaskOutcome::Cancelled;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            TaskOutcome::Finished("ran to the end".into())
        });

        registry.cancel(id);
        let finished = wait_for_outcome(&mut registry);
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].2, TaskOutcome::Cancelled);
    }

    /// A task that panics is reported as failed rather than left running for
    /// ever in the panel.
    #[test]
    fn a_panicking_task_is_reported_as_failed() {
        let mut registry = registry();
        registry.spawn_thread(TaskKind::Import, "a broken file", |_| {
            panic!("something went wrong inside the task");
        });

        let finished = wait_for_outcome(&mut registry);
        assert_eq!(finished.len(), 1);
        assert!(matches!(finished[0].2, TaskOutcome::Failed(_)));
        assert!(registry.is_empty());
    }

    /// **Only work worth losing sleep over blocks a quit.** Asking about
    /// thumbnails is how a prompt stops being read.
    #[test]
    fn only_real_work_blocks_a_quit() {
        let (gate_tx, gate_rx) = crossbeam_channel::bounded::<()>(0);
        let held = gate_rx.clone();
        let mut registry = registry();
        registry.spawn_thread(TaskKind::Thumbnails, "pictures", move |_| {
            let _ = held.recv();
            TaskOutcome::Finished(String::new())
        });
        assert!(registry.quit_blockers().is_empty(), "thumbnails blocked quit");

        let held = gate_rx.clone();
        registry.spawn_thread(TaskKind::Export, "shot3.mp4", move |_| {
            let _ = held.recv();
            TaskOutcome::Finished(String::new())
        });
        let blockers = registry.quit_blockers();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].label, "shot3.mp4");

        let _ = gate_tx.send(());
        let _ = gate_tx.send(());
        registry.cancel_and_join();
    }

    /// Cancelling on the way out waits for the work to actually stop, so a
    /// half-written file gets cleaned up rather than left behind.
    #[test]
    fn cancel_and_join_waits_for_the_work_to_stop() {
        let tidied = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&tidied);

        let mut registry = registry();
        registry.spawn_thread(TaskKind::Export, "a film", move |ctx| {
            while !ctx.cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // What an export does with its `.part` file.
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            TaskOutcome::Cancelled
        });

        registry.cancel_and_join();
        assert!(
            tidied.load(std::sync::atomic::Ordering::SeqCst),
            "the process would have exited before the task tidied up"
        );
        assert!(registry.is_empty());
    }

    /// An unknown id is not a panic — a panel can hold an id for a task that
    /// finished a frame ago.
    #[test]
    fn cancelling_something_that_has_gone_is_harmless() {
        let mut registry = registry();
        registry.cancel(TaskId(999));
        assert!(registry.is_empty());
    }
}
