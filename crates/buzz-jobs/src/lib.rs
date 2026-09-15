//! The job system that keeps all of BuzzAnimate's cores busy.
//!
//! Adobe Animate does its authoring work on essentially one thread. On the
//! reference machine for this project — an i7-14700K with 20 cores / 28
//! threads — that leaves roughly 96% of the CPU idle while the user waits.
//!
//! # Two pools, not one
//!
//! A single shared pool lets a long background job (thumbnail generation, an
//! import, an autosave) occupy every worker and stall an interactive operation
//! behind it. Rayon has no job priorities, so priority is expressed
//! structurally instead:
//!
//! * [`Pool::Interactive`] — sized to the bulk of the machine. Work the user is
//!   actively waiting on: boolean ops, hit-testing, onion-skin frames.
//! * [`Pool::Background`] — deliberately small. Work that must never delay a
//!   keystroke: thumbnails, autosave, speculative prefetch.
//!
//! Both leave headroom for the UI and render threads, which are *not* pool
//! members and must never block on one.
//!
//! # Metrics
//!
//! Utilisation comes from per-worker OS CPU time rather than from timing
//! submitted jobs, because one submitted job usually fans out across the whole
//! pool. See [`clock`] for why that distinction matters.

pub mod clock;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use rayon::ThreadPool;

use crate::clock::WorkerClocks;

/// A way to ask work to stop.
///
/// # Why this is the only thing the job system knows about cancellation
///
/// Long work does not live here. The pools are rayon pools, sized for
/// data-parallel bursts, and a job that runs for minutes would squat on one of
/// the six background workers and starve everything behind it — autosave
/// included. So tasks are owned elsewhere (`buzz_app::tasks`), and what this
/// crate contributes is the one primitive both sides need to agree on.
///
/// A flag rather than a channel because the reader is in a loop and wants to
/// glance, not wait. Cloning shares the flag: cancel one, cancel all.
#[derive(Clone, Default, Debug)]
pub struct CancelToken(Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the work to stop. It stops when it next looks.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Has anybody asked?
    ///
    /// `Relaxed` on purpose: this is a hint that becomes true and never false,
    /// and the work it guards is not ordered against anything. Paying for
    /// acquire/release on every iteration of a tight loop would be a real cost
    /// for no guarantee anybody uses.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Which pool a unit of work belongs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// The user is waiting on this. Gets most of the machine.
    Interactive,
    /// Must never delay interaction. Gets a small slice.
    Background,
}

/// Job counters for one pool.
#[derive(Debug, Default)]
struct Counters {
    /// Jobs currently executing.
    in_flight: AtomicUsize,
    /// Jobs finished since startup.
    completed: AtomicU64,
}

/// One pool plus its instrumentation.
struct Worker {
    pool: ThreadPool,
    clocks: Arc<WorkerClocks>,
    counters: Arc<Counters>,
}

impl Worker {
    fn build(threads: usize, name: &'static str) -> Self {
        let clocks = Arc::new(WorkerClocks::new(threads));

        let start = Arc::clone(&clocks);
        let exit = Arc::clone(&clocks);

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(move |i| format!("{name}-{i}"))
            .start_handler(move |i| start.register(i))
            .exit_handler(move |i| exit.unregister(i))
            .build()
            .unwrap_or_else(|e| panic!("failed to build {name} pool: {e}"));

        Self {
            pool,
            clocks,
            counters: Arc::new(Counters::default()),
        }
    }
}

/// A point-in-time reading of a pool's counters.
#[derive(Debug, Clone)]
pub struct Snapshot {
    taken_at: Instant,
    cpu_nanos: Vec<u64>,
    completed: u64,
}

/// Utilisation over the window between two [`Snapshot`]s.
#[derive(Debug, Clone)]
pub struct Utilisation {
    /// Busy fraction in `0.0..=1.0` for each worker.
    pub per_worker: Vec<f64>,
    /// Mean across workers.
    pub mean: f64,
    /// Total core-seconds of CPU consumed during the window. A value of 8.0
    /// over a 1-second window means eight cores were saturated.
    pub cores_busy: f64,
    /// Jobs completed during the window.
    pub completed: u64,
    /// Length of the window in seconds.
    pub window_secs: f64,
    /// False when the platform cannot report CPU time, in which case
    /// `per_worker` is all zeroes and means nothing.
    pub available: bool,
}

/// Owns the thread pools and their metrics.
pub struct JobSystem {
    interactive: Worker,
    background: Worker,
}

impl JobSystem {
    /// Build pools sized for the current machine.
    ///
    /// Reserves two threads for the UI and render threads, then splits the
    /// remainder, giving the background pool a quarter (at least one thread).
    pub fn new() -> Self {
        let logical = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        // Reserve for the UI thread and the render thread.
        let poolable = logical.saturating_sub(2).max(1);
        let background = (poolable / 4).max(1);
        let interactive = poolable.saturating_sub(background).max(1);

        Self::with_sizes(interactive, background)
    }

    /// Build pools with explicit sizes. Mainly for tests and benchmarks.
    pub fn with_sizes(interactive_threads: usize, background_threads: usize) -> Self {
        let interactive_threads = interactive_threads.max(1);
        let background_threads = background_threads.max(1);

        tracing::info!(
            logical = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(0),
            interactive = interactive_threads,
            background = background_threads,
            cpu_metrics = WorkerClocks::supported(),
            "job system online"
        );

        Self {
            interactive: Worker::build(interactive_threads, "buzz-work"),
            background: Worker::build(background_threads, "buzz-bg"),
        }
    }

    /// Number of workers in the interactive pool.
    pub fn interactive_threads(&self) -> usize {
        self.interactive.pool.current_num_threads()
    }

    /// Number of workers in the background pool.
    pub fn background_threads(&self) -> usize {
        self.background.pool.current_num_threads()
    }

    /// Jobs currently executing in the interactive pool.
    pub fn in_flight(&self) -> usize {
        self.interactive.counters.in_flight.load(Ordering::Relaxed)
    }

    fn worker(&self, pool: Pool) -> &Worker {
        match pool {
            Pool::Interactive => &self.interactive,
            Pool::Background => &self.background,
        }
    }

    /// Run `f` on the given pool and block until it finishes.
    ///
    /// Nested rayon constructs inside `f` run on the same pool, so this is the
    /// entry point for any parallel algorithm.
    pub fn run<T, F>(&self, pool: Pool, f: F) -> T
    where
        F: FnOnce() -> T + Send,
        T: Send,
    {
        let w = self.worker(pool);
        let counters = Arc::clone(&w.counters);
        w.pool.install(move || {
            let _guard = JobGuard::start(counters);
            f()
        })
    }

    /// Fire and forget. Returns immediately.
    pub fn spawn<F>(&self, pool: Pool, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let w = self.worker(pool);
        let counters = Arc::clone(&w.counters);
        w.pool.spawn(move || {
            let _guard = JobGuard::start(counters);
            f();
        });
    }

    /// Read a pool's counters.
    pub fn sample(&self, pool: Pool) -> Snapshot {
        let w = self.worker(pool);
        Snapshot {
            taken_at: Instant::now(),
            cpu_nanos: w.clocks.cpu_nanos(),
            completed: w.counters.completed.load(Ordering::Relaxed),
        }
    }

    /// Utilisation of `pool` between `since` and now.
    pub fn utilisation_since(&self, pool: Pool, since: &Snapshot) -> Utilisation {
        let now = self.sample(pool);
        let window = now.taken_at.duration_since(since.taken_at).as_secs_f64();
        let available = WorkerClocks::supported();

        if window <= 0.0 || !available {
            return Utilisation {
                per_worker: vec![0.0; now.cpu_nanos.len()],
                mean: 0.0,
                cores_busy: 0.0,
                completed: now.completed.saturating_sub(since.completed),
                window_secs: window.max(0.0),
                available,
            };
        }

        let window_nanos = window * 1e9;
        let per_worker: Vec<f64> = now
            .cpu_nanos
            .iter()
            .zip(&since.cpu_nanos)
            .map(|(a, b)| (a.saturating_sub(*b) as f64 / window_nanos).clamp(0.0, 1.0))
            .collect();

        let cores_busy: f64 = per_worker.iter().sum();
        let mean = if per_worker.is_empty() {
            0.0
        } else {
            cores_busy / per_worker.len() as f64
        };

        Utilisation {
            per_worker,
            mean,
            cores_busy,
            completed: now.completed.saturating_sub(since.completed),
            window_secs: window,
            available,
        }
    }
}

impl Default for JobSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Keeps job counters correct even if the job panics.
struct JobGuard {
    counters: Arc<Counters>,
}

impl JobGuard {
    fn start(counters: Arc<Counters>) -> Self {
        counters.in_flight.fetch_add(1, Ordering::Relaxed);
        Self { counters }
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.counters.completed.fetch_add(1, Ordering::Relaxed);
        self.counters.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    /// A chunk of arithmetic the optimiser cannot fold away.
    fn grind(seed: u64) -> u64 {
        let mut acc = seed | 1;
        for k in 0..400_000u64 {
            acc = acc.wrapping_mul(6364136223846793005).wrapping_add(k | 1);
            acc ^= acc >> 17;
        }
        acc
    }

    #[test]
    fn pools_reserve_headroom_for_ui_and_render_threads() {
        let js = JobSystem::new();
        let logical = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let total = js.interactive_threads() + js.background_threads();
        assert!(
            total <= logical.saturating_sub(2).max(1),
            "pools ({total}) must not consume the UI and render headroom of \
             {logical} logical CPUs"
        );
        assert!(js.interactive_threads() >= js.background_threads());
    }

    /// The multicore claim, as an executable proof.
    ///
    /// Measures real CPU consumption, so it fails if work silently collapses
    /// onto one worker — the exact failure mode this project exists to avoid.
    #[test]
    fn nested_parallel_work_really_spreads_across_workers() {
        if !clock::WorkerClocks::supported() {
            eprintln!("skipping: per-thread CPU time unavailable on this platform");
            return;
        }

        const THREADS: usize = 8;
        let js = JobSystem::with_sizes(THREADS, 2);
        let before = js.sample(Pool::Interactive);

        let total = js.run(Pool::Interactive, || {
            (0..256u64)
                .into_par_iter()
                .map(grind)
                // Wrapping, not `sum()`: these are deliberately huge values and
                // `sum()` would trip debug overflow checks.
                .reduce(|| 0u64, |a, b| a.wrapping_add(b))
        });
        std::hint::black_box(total);

        let u = js.utilisation_since(Pool::Interactive, &before);
        let busy = u.per_worker.iter().filter(|f| **f > 0.10).count();

        assert!(
            busy >= THREADS / 2,
            "expected at least {} of {THREADS} workers busy, saw {busy}: {:?}",
            THREADS / 2,
            u.per_worker
        );
        assert!(
            u.cores_busy > 2.0,
            "expected multiple cores saturated, got {:.2} core-seconds per second",
            u.cores_busy
        );
    }

    #[test]
    fn run_returns_the_closure_value() {
        let js = JobSystem::with_sizes(2, 1);
        assert_eq!(js.run(Pool::Interactive, || 6 * 7), 42);
    }

    #[test]
    fn background_pool_is_separate_from_interactive() {
        let js = JobSystem::with_sizes(4, 2);
        let bg_before = js.sample(Pool::Background);
        let inter_before = js.sample(Pool::Interactive);

        js.run(Pool::Background, || {
            (0..16u64)
                .into_par_iter()
                .map(grind)
                .reduce(|| 0, |a, b| a ^ b)
        });

        let inter = js.utilisation_since(Pool::Interactive, &inter_before);
        let bg = js.utilisation_since(Pool::Background, &bg_before);

        assert_eq!(bg.per_worker.len(), 2);
        assert_eq!(inter.per_worker.len(), 4);
        if clock::WorkerClocks::supported() {
            assert!(
                inter.cores_busy < 0.5,
                "background work leaked into the interactive pool: {:?}",
                inter.per_worker
            );
        }
    }

    #[test]
    fn spawned_background_work_completes() {
        let js = JobSystem::with_sizes(2, 2);
        let counter = Arc::new(AtomicU32::new(0));

        for _ in 0..16 {
            let c = Arc::clone(&counter);
            js.spawn(Pool::Background, move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while counter.load(Ordering::SeqCst) < 16 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 16);
    }

    #[test]
    fn a_panicking_job_still_releases_its_slot() {
        let js = JobSystem::with_sizes(2, 1);
        let before = js.sample(Pool::Interactive);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            js.run(Pool::Interactive, || panic!("boom"));
        }));
        assert!(result.is_err());

        assert_eq!(js.in_flight(), 0, "panicking job leaked an in-flight slot");
        let u = js.utilisation_since(Pool::Interactive, &before);
        assert_eq!(u.completed, 1, "panicking job should still be counted");
    }

    #[test]
    fn utilisation_is_bounded_and_near_zero_when_idle() {
        let js = JobSystem::with_sizes(4, 1);

        // Let the pool settle before the first snapshot. Spawning threads costs
        // CPU, and rayon workers spin briefly before parking; measuring from
        // the instant of construction folds that startup cost into the window
        // and makes an idle pool look busy on a loaded machine.
        std::thread::sleep(Duration::from_millis(50));

        let before = js.sample(Pool::Interactive);
        std::thread::sleep(Duration::from_millis(100));
        let u = js.utilisation_since(Pool::Interactive, &before);

        assert_eq!(u.per_worker.len(), 4);
        assert!(u.per_worker.iter().all(|f| (0.0..=1.0).contains(f)));
        assert!(
            u.mean < 0.05,
            "idle pool reported {} mean utilisation",
            u.mean
        );
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;

    #[test]
    fn a_fresh_token_is_not_cancelled() {
        assert!(!CancelToken::new().is_cancelled());
    }

    /// Cancelling one clone cancels them all — that is what makes it usable
    /// as a handle held on one side and read on the other.
    #[test]
    fn a_clone_shares_the_flag() {
        let held = CancelToken::new();
        let given_away = held.clone();
        assert!(!given_away.is_cancelled());

        held.cancel();
        assert!(given_away.is_cancelled());
    }

    /// It only ever goes one way, which is what lets the reader glance at it
    /// without synchronising.
    #[test]
    fn cancelling_twice_is_still_cancelled() {
        let token = CancelToken::new();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
    }

    /// The shape the work actually uses: a loop that checks and stops.
    #[test]
    fn a_worker_thread_stops_when_asked() {
        let token = CancelToken::new();
        let worker = token.clone();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&done);

        let handle = std::thread::spawn(move || {
            for i in 0..1_000_000 {
                if worker.is_cancelled() {
                    return i;
                }
                counted.fetch_add(1, Ordering::Relaxed);
            }
            -1
        });

        // Let it get going, then stop it.
        while done.load(Ordering::Relaxed) < 10 {
            std::hint::spin_loop();
        }
        token.cancel();

        let stopped_at = handle.join().expect("the worker panicked");
        assert!(stopped_at >= 0, "it ran to the end instead of stopping");
    }
}
