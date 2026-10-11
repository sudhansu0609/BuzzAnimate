//! **Encode the next frame of a playback on its own thread** (E4/E5 of the
//! revised S5 in `docs/BIMARI5_LAG_PLAN.md`).
//!
//! A warm stage encode is 10-13 ms and the GPU another 18-25 ms, which misses a
//! 24 fps frame when they run one after the other on the render thread. So
//! while a *plain* stage plays (see [`crate::stage::is_plain`]) the render
//! thread lends the stage's [`DrawCache`] to a dedicated `buzz-encode` thread,
//! which walks it in frame order and hands finished encodings back through a
//! bounded queue ([`Prepared`]). One cache, one thread, frames in order: the
//! decisions (first sightings, the ladder, the LRU) are exactly the ones
//! sequential [`stage::build_plain_scene`] calls would make, so the bytes are
//! the same.
//!
//! # Ownership
//!
//! The cache is **lent**, not shared: [`Pipeline::start`] takes it by value
//! and [`Pipeline::stop`] gives it back (after raising its abort flag, so a
//! frame being drawn ends within a layer — E5). Whoever lent it holds a
//! stand-in meanwhile; see `App::lights` in `app.rs`.
//!
//! # Staleness
//!
//! Everything a [`Prepared`] was drawn from is named by its [`Epoch`]. The
//! render thread compares the epoch of *this* frame with the one the pipeline
//! runs under and, on any difference, stops the pipeline and draws the frame
//! itself: an encoding from before an edit is never shown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicU64, AtomicU8};

use buzz_jobs::{JobSystem, Pool};
use buzz_render::document::{DrawCache, LightDetail, NewStamp, SymbolTable};
use crossbeam_channel::{Receiver, Select, Sender, TryRecvError, bounded, unbounded};

use crate::stage::{self, StageInputs};

/// Finished frames the thread may hold ready ahead of the render thread.
pub const QUEUE_DEPTH: usize = 2;
/// `vello::Scene`s the thread cycles through. Together with the one the render
/// thread draws from, the encodings in the queue and the one being built.
const SCENES: usize = 3;
/// How many steps ahead of the playhead a frame may be and still count as
/// "ahead" (depth-2 queue, one in flight, one of slack).
const LOOKAHEAD: usize = 4;

/// `BUZZ_NO_ENCODE_AHEAD=1` turns the pipeline off: the stage is encoded on the
/// render thread exactly as before. Read once.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("BUZZ_NO_ENCODE_AHEAD").map_or(true, |v| v.is_empty() || v == "0")
    })
}

/// `BUZZ_NO_PREBUILD=1` turns the stamp pre-build pool off (E6): the encode
/// thread then builds every stamp itself. Read once.
pub fn prebuild_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("BUZZ_NO_PREBUILD").map_or(true, |v| v.is_empty() || v == "0")
    })
}

/// **Playback pre-build is off by default** (user 2026-10-01). Whole Bimari5,
/// `frame_cost a --pipelined`, two runs each: with it 682 / 697 frames
/// dropped, peak 1.62 GiB; without it 350 / 529 dropped, 1.22 GiB. Its four
/// jobs compete with the encode thread for the CPU. `BUZZ_PREBUILD=1` turns it
/// back on for playback; `BUZZ_NO_PREBUILD=1` still turns off both it and the
/// seek/scrub pre-build ([`SeekAhead`]), which was not measured and stays on.
/// Read once.
pub fn playback_prebuild_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        prebuild_enabled() && std::env::var("BUZZ_PREBUILD").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

/// Pre-build jobs in flight at once, and the slots (persistent worker caches)
/// they run on.
pub const PREBUILD_SLOTS: usize = 4;
/// How many frames past the encode head a pre-build may reach.
pub const PREBUILD_HORIZON: usize = 24;
/// Memory budget of each slot's own stamp cache.
const PREBUILD_SLOT_BUDGET: usize = 128 << 20;

/// What the pre-build pool has done, for the log and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrebuildStats {
    /// Jobs started.
    pub dispatched: u64,
    /// Stamps installed into the encode thread's cache.
    pub installed: u64,
    /// Results dropped because their epoch had ended.
    pub stale: u64,
}

#[derive(Default)]
struct PrebuildCounters {
    dispatched: AtomicU64,
    installed: AtomicU64,
    stale: AtomicU64,
}

/// One pre-build worker: a cache that lives as long as the epoch, and the
/// scene its jobs encode into (thrown away each time).
struct Slot {
    cache: DrawCache,
    scratch: vello::Scene,
}

/// A finished pre-build job.
struct Done {
    /// E7: a seek fan-out job (waited for by [`Prebuild::wait_fan`]).
    fan: bool,
    epoch_no: u64,
    slot: Slot,
    stamps: Vec<NewStamp>,
}

/// Where the next frame to pre-build is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Next {
    /// Not chosen yet: the frame after the encode head.
    Unset,
    At(u32),
    /// A non-looping run has no frame left.
    End,
}

/// How many playback steps lead from `from` to `to` (`0` when they are the
/// same frame), looking at most `limit` steps ahead; `None` when `to` is not
/// that near in front of `from`.
fn steps_between(
    scene: &buzz_scene::Scene,
    play: PlayRange,
    from: u32,
    to: u32,
    limit: usize,
) -> Option<usize> {
    let mut f = from;
    for i in 0..=limit {
        if f == to {
            return Some(i);
        }
        f = step_frame(scene, play, f)?;
    }
    None
}

/// **E6: the stamp pre-build pool.** Up to [`PREBUILD_SLOTS`] jobs on
/// `Pool::Interactive`, each encoding one *future* frame of the plain stage
/// into a scratch scene on its own persistent [`DrawCache`] (symbol table
/// cloned per epoch, build on first sighting, 128 MiB) and handing back the
/// stamps that cache built. The encode thread installs them before each frame
/// ([`DrawCache::install_stamps`]), so a stamp first met 20 frames from now is a
/// hit when it arrives instead of a live draw followed by a build.
///
/// A stamp's bytes depend only on its key (E3), so what a worker built is what
/// the encode thread would have built. Jobs are tied to an **epoch generation**:
/// [`Self::reset`] (a new epoch, or the pipeline stopping) raises the old
/// generation's cancel flag, so jobs end within a layer, and anything they
/// return is dropped, never installed.
pub struct Prebuild {
    jobs: Arc<JobSystem>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    /// Ready slots of the current generation.
    idle: Vec<Slot>,
    /// Jobs started and not yet reported (any generation).
    in_flight: usize,
    epoch_no: u64,
    cancel: Arc<AtomicBool>,
    template: SymbolTable,
    next: Next,
    /// Did the encode thread's last pass build or draw-live anything?
    missed_last: bool,
    /// Did the last finished job build anything?
    last_job_built: bool,
    counters: Arc<PrebuildCounters>,
    /// Jobs allowed in flight at once, and the memory budget of each slot's cache.
    slots: usize,
    slot_budget: usize,
}

impl Prebuild {
    pub fn new(jobs: Arc<JobSystem>) -> Self {
        Self::with_counters(jobs, Arc::new(PrebuildCounters::default()))
    }

    /// **E7:** a pool of `slots` jobs in flight at once, each slot's cache held to
    /// `slot_budget` bytes (the seek fan-out wants 8).
    pub fn with_capacity(jobs: Arc<JobSystem>, slots: usize, slot_budget: usize) -> Self {
        let mut pb = Self::new(jobs);
        pb.slots = slots.max(1);
        pb.slot_budget = slot_budget;
        pb
    }

    fn with_counters(jobs: Arc<JobSystem>, counters: Arc<PrebuildCounters>) -> Self {
        let (done_tx, done_rx) = unbounded();
        Self {
            slots: PREBUILD_SLOTS,
            slot_budget: PREBUILD_SLOT_BUDGET,
            jobs,
            done_tx,
            done_rx,
            idle: Vec::new(),
            in_flight: 0,
            epoch_no: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            template: SymbolTable::default(),
            next: Next::Unset,
            missed_last: true,
            last_job_built: true,
            counters,
        }
    }

    pub fn stats(&self) -> PrebuildStats {
        PrebuildStats {
            dispatched: self.counters.dispatched.load(Ordering::Relaxed),
            installed: self.counters.installed.load(Ordering::Relaxed),
            stale: self.counters.stale.load(Ordering::Relaxed),
        }
    }

    /// Jobs started and not yet collected.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// **A new epoch (or the end of the pipeline).** Cancels what is running,
    /// drops the idle slots and everything still to come from the old
    /// generation, and takes `template` (the lent cache's symbol table) for the
    /// slots made from now on.
    ///
    /// **Frees nothing itself** (S7, stop latency): the idle slots hold up to
    /// 128 MiB of stamps each, and dropping them inline cost 60-130 ms of a
    /// [`Pipeline::stop`]; they go to the background pool instead ([`Self::reap`]).
    pub fn reset(&mut self, template: Option<&SymbolTable>) {
        self.epoch_no += 1;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let idle = std::mem::take(&mut self.idle);
        let old = std::mem::replace(&mut self.template, template.cloned().unwrap_or_default());
        self.reap((idle, old));
        self.next = Next::Unset;
        self.missed_last = true;
        self.last_job_built = true;
    }

    /// Free `junk` on the background pool, so the thread that called pays for
    /// none of it.
    fn reap<T: Send + 'static>(&self, junk: T) {
        self.jobs.spawn(Pool::Background, move || drop(junk));
    }

    fn new_slot(&self) -> Slot {
        let mut cache = DrawCache::new();
        cache.set_build_on_first_sighting(true);
        cache.set_collect_new_stamps(true);
        cache.symbol_scenes.budget_bytes = self.slot_budget;
        cache.set_abort(Arc::clone(&self.cancel));
        cache.adopt_symbols(&self.template);
        Slot { cache, scratch: vello::Scene::new() }
    }

    /// Start a job that encodes `inputs.frame` (with `inputs.buffer_scale`) at
    /// `detail`. `false` when [`PREBUILD_SLOTS`] jobs are already in flight.
    pub fn dispatch(&mut self, inputs: StageInputs, detail: LightDetail) -> bool {
        self.dispatch_slice(inputs, detail, None)
    }

    /// [`Self::dispatch`] for one share of the frame's layers: `Some((k, n))`
    /// draws only the layers at position `k` modulo `n` of the stack's paint order
    /// (E7 seek fan-out), `None` all of them.
    pub fn dispatch_slice(
        &mut self,
        inputs: StageInputs,
        detail: LightDetail,
        slice: Option<(usize, usize)>,
    ) -> bool {
        if self.in_flight >= self.slots {
            return false;
        }
        let mut slot = self.idle.pop().unwrap_or_else(|| self.new_slot());
        // The key includes whether the seal is drawn, so the slot must judge at
        // the level the encode thread's cache is at.
        slot.cache.pin_detail(Some(detail));
        slot.cache.set_work_slice(slice);
        let fan = slice.is_some();
        let (tx, epoch_no) = (self.done_tx.clone(), self.epoch_no);
        self.in_flight += 1;
        self.counters.dispatched.fetch_add(1, Ordering::Relaxed);
        self.jobs.spawn(Pool::Interactive, move || {
            let mut slot = slot;
            // The line budget a pass judges against is per thread; leave the
            // pool thread as it was found.
            let before = buzz_render::vello_encoding::buffer_scale();
            slot.scratch.reset();
            slot.cache.begin();
            let cull = stage::cull_for(&inputs.camera);
            let _ = stage::encode_plain(&mut slot.scratch, &inputs, &mut slot.cache, cull);
            slot.cache.end();
            buzz_render::vello_encoding::set_scale_for_this_thread(before);
            let stamps = slot.cache.drain_new_stamps();
            // S7 (RSS): the slot keeps no copy of what it built. The encode
            // thread installs it (the same `Arc`s); a slot that kept 128 MiB of
            // them on top of the main cache's was most of the pipelined run's
            // extra 900 MiB (peak 2078 -> 1491 MiB on Bimari5 frames 0-1500).
            slot.cache.drop_stamps();
            let _ = tx.send(Done { fan, epoch_no, slot, stamps });
        });
        true
    }

    fn take(&mut self, done: Done, into: Option<&mut DrawCache>) -> usize {
        self.in_flight = self.in_flight.saturating_sub(1);
        match into {
            Some(cache) if done.epoch_no == self.epoch_no => {
                let n = cache.install_stamps(done.stamps);
                // "Did the job find anything the encode thread lacked?" A slot
                // keeps no stamps of its own (S7, RSS), so what it builds is not
                // by itself news: only what the main cache did not have is.
                self.last_job_built = n > 0;
                self.counters.installed.fetch_add(n as u64, Ordering::Relaxed);
                self.idle.push(done.slot);
                n
            }
            _ => {
                // An old epoch's result: never installed, its slot dropped (on
                // the background pool: a slot can hold 128 MiB).
                self.counters.stale.fetch_add(1, Ordering::Relaxed);
                self.reap(done);
                0
            }
        }
    }

    /// Install what the finished jobs of the **current** generation built into
    /// `into`, without waiting. Answers how many stamps were inserted.
    pub fn collect(&mut self, into: &mut DrawCache) -> usize {
        let mut n = 0;
        while let Ok(done) = self.done_rx.try_recv() {
            n += self.take(done, Some(into));
        }
        n
    }

    /// [`Self::collect`], waiting up to `timeout` for every job in flight (old
    /// generations' included, whose results are dropped).
    pub fn wait_idle(&mut self, into: &mut DrawCache, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        let mut n = self.collect(into);
        while self.in_flight > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.done_rx.recv_timeout(left) {
                Ok(done) => n += self.take(done, Some(into)),
                Err(_) => break,
            }
        }
        n
    }

    /// **E7:** wait up to `timeout` for `expected` fan-out jobs, installing every
    /// current-generation result that arrives meanwhile (scrub jobs included).
    /// Answers how many stamps were inserted.
    pub fn wait_fan(&mut self, into: &mut DrawCache, expected: usize, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        let (mut n, mut got) = (0, 0);
        while got < expected {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.done_rx.recv_timeout(left) {
                Ok(done) => {
                    got += usize::from(done.fan);
                    n += self.take(done, Some(into));
                }
                Err(_) => break,
            }
        }
        n
    }

    /// Jobs that may still be started right now.
    pub fn free_slots(&self) -> usize {
        self.slots.saturating_sub(self.in_flight)
    }

    /// The pass the encode thread just ran: did it build or draw live anything
    /// it had not seen? Pre-building stops while this and the last job's
    /// result say everything ahead is already warm.
    pub fn note_pass(&mut self, missed: bool) {
        self.missed_last = missed;
    }

    /// Is there anything to gain from building ahead right now?
    pub fn is_hot(&self) -> bool {
        self.missed_last || self.last_job_built
    }

    /// **Before the encode thread draws `head`:** install what finished, then
    /// start jobs for the frames after it, up to [`PREBUILD_HORIZON`] ahead.
    /// Does nothing when the frames ahead are already warm, when the main
    /// cache is over 90% of its budget, or while lights are built (a lit
    /// object is never stamped).
    pub fn tick(&mut self, main: &mut DrawCache, inputs: &StageInputs, play: PlayRange, head: u32) {
        self.collect(main);
        if !self.is_hot() || !main.symbol_reuse() || !main.lights.is_empty() {
            return;
        }
        let (_, bytes, budget, _, _) = main.symbol_scenes.retention_stats();
        // S9: a cache a warm-up filled slides along the playback: the stamps of
        // frames the playhead has left go to make room, so building ahead
        // carries on past the warmed frames. Any other cache takes no more.
        if bytes.saturating_mul(10) > budget.saturating_mul(9) && !main.make_room_for_stamps() {
            return;
        }
        let scene = Arc::clone(&inputs.scene);
        while self.in_flight < self.slots {
            let Some(frame) = self.next_frame(&scene, play, head) else {
                break;
            };
            let mut job = inputs.clone();
            job.frame = frame;
            if !self.dispatch(job, main.detail()) {
                break;
            }
            self.next = match step_frame(&scene, play, frame) {
                Some(n) => Next::At(n),
                None => Next::End,
            };
        }
    }

    /// The next frame to pre-build, or `None` when the horizon is reached or
    /// the run has ended. A frame behind the head (the encode thread caught up)
    /// restarts at the frame after it.
    fn next_frame(&mut self, scene: &buzz_scene::Scene, play: PlayRange, head: u32) -> Option<u32> {
        let after_head = step_frame(scene, play, head);
        let cand = match self.next {
            Next::End => return None,
            Next::Unset => after_head?,
            Next::At(f) => f,
        };
        match steps_between(scene, play, head, cand, PREBUILD_HORIZON + 1) {
            Some(d) if (1..=PREBUILD_HORIZON).contains(&d) => Some(cand),
            Some(d) if d > PREBUILD_HORIZON => None,
            // Behind the head, or the head itself: start again after it.
            _ => {
                let f = after_head?;
                self.next = Next::At(f);
                Some(f)
            }
        }
    }
}

impl Drop for Prebuild {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Workers a jump's target frame is fanned across (E7).
pub const SEEK_WORKERS: usize = 8;
/// Frames pre-built ahead of a scrub, in its direction (E7).
pub const SCRUB_HORIZON: u32 = 8;
/// Scrub pre-build jobs in flight at once; the rest of the pool is kept for a
/// jump's fan-out.
const SCRUB_IN_FLIGHT: usize = 4;
/// Memory budget of each seek slot's own stamp cache.
const SEEK_SLOT_BUDGET: usize = 48 << 20;
/// Longest the render thread waits for a fan-out before encoding anyway.
const FAN_WAIT: Duration = Duration::from_secs(5);

/// What the seek pre-build has done, for the log and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeekStats {
    /// Jumps whose target frame was fanned across workers.
    pub fanouts: u64,
    /// Jobs those fan-outs started.
    pub fan_jobs: u64,
    /// Wall milliseconds of the last fan-out (dispatch until every share is
    /// installed).
    pub last_fanout_ms: f64,
    /// Stamps the last fan-out installed.
    pub last_installed: u64,
    /// Scrub pre-build jobs started.
    pub scrub_jobs: u64,
}

/// **E7: stamps for a seek.** The playhead jumped while not playing, so the
/// target frame meets stamps nobody has built; the render thread would build each
/// one itself, one after another. This fans the frame's layers across up to
/// [`SEEK_WORKERS`] workers ([`Prebuild::dispatch_slice`]), installs what they
/// built into the render thread's own cache, and only then does the render
/// thread encode the frame, warm. While the playhead is scrubbed, the frames
/// ahead of it in its direction ([`SCRUB_HORIZON`]) are pre-built in the
/// background and installed before the next frame.
///
/// Exact in the same way E6 is: a stamp's bytes depend only on its key (E3), so
/// installed stamps are the ones the render thread would have built.
/// `BUZZ_NO_PREBUILD=1` turns it off.
pub struct SeekAhead {
    pre: Prebuild,
    epoch: Option<Epoch>,
    last_frame: Option<u32>,
    /// `1`, `-1` or `0` (not scrubbing yet).
    dir: i32,
    /// Frames a scrub job has been started for under this epoch.
    started: std::collections::HashSet<u32>,
    /// Frames the home cache has encoded (or had fanned out) since its last
    /// eviction: a jump to one of them needs nothing built.
    warm: std::collections::HashSet<u32>,
    evictions: u64,
    pub stats: SeekStats,
}

impl SeekAhead {
    pub fn new(jobs: Arc<JobSystem>) -> Self {
        Self::with_capacity(jobs, SEEK_WORKERS, SEEK_SLOT_BUDGET)
    }

    pub fn with_capacity(jobs: Arc<JobSystem>, slots: usize, slot_budget: usize) -> Self {
        Self {
            pre: Prebuild::with_capacity(jobs, slots, slot_budget),
            epoch: None,
            last_frame: None,
            dir: 0,
            started: std::collections::HashSet::new(),
            warm: std::collections::HashSet::new(),
            evictions: 0,
            stats: SeekStats::default(),
        }
    }

    /// Jobs started and not yet collected.
    pub fn in_flight(&self) -> usize {
        self.pre.in_flight()
    }

    /// The pool's own counters (installs, dispatches, stale results).
    pub fn prebuild_stats(&self) -> PrebuildStats {
        self.pre.stats()
    }

    fn usable(main: &DrawCache) -> bool {
        prebuild_enabled() && main.symbol_reuse() && main.lights.is_empty()
    }

    /// **Before the render thread encodes `inputs.frame`.** Resets the pool on a
    /// new `epoch`, installs what finished scrub jobs built, and - when `jump`
    /// (the playhead moved by more than one frame while not playing) to a frame
    /// the cache is not known to hold - fans the frame's objects across workers
    /// and installs their stamps. Answers how many stamps were installed.
    pub fn before_encode(
        &mut self,
        epoch: Epoch,
        main: &mut DrawCache,
        inputs: &StageInputs,
        jump: bool,
    ) -> usize {
        if !Self::usable(main) {
            return 0;
        }
        if self.epoch != Some(epoch) {
            self.pre.reset(Some(&main.symbols));
            self.epoch = Some(epoch);
            self.started.clear();
            self.warm.clear();
            self.dir = 0;
            self.last_frame = None;
        }
        let mut installed = self.pre.collect(main);
        // An eviction may have taken what a warm frame needed.
        let evictions = main.symbol_scenes.retention_stats().3;
        if evictions != self.evictions {
            self.evictions = evictions;
            self.warm.clear();
        }
        let frame = inputs.frame;
        // No budget gate for a jump (unlike a scrub): the stamps installed are what
        // the encode is about to build anyway, and `end` evicts unused ones first.
        if !jump || self.warm.contains(&frame) || self.started.contains(&frame) {
            return installed;
        }

        let workers = SEEK_WORKERS.min(self.pre.free_slots());
        if workers < 2 {
            return installed;
        }
        let started = Instant::now();
        let detail = main.detail();
        let mut sent = 0;
        for k in 0..workers {
            if self.pre.dispatch_slice(inputs.clone(), detail, Some((k, workers))) {
                sent += 1;
            }
        }
        let n = self.pre.wait_fan(main, sent, FAN_WAIT);
        installed += n;
        self.warm.insert(frame);
        self.stats.fanouts += 1;
        self.stats.fan_jobs += sent as u64;
        self.stats.last_fanout_ms = started.elapsed().as_secs_f64() * 1e3;
        self.stats.last_installed = n as u64;
        installed
    }

    fn near_budget(main: &DrawCache) -> bool {
        let (_, bytes, budget, _, _) = main.symbol_scenes.retention_stats();
        bytes.saturating_mul(10) > budget.saturating_mul(9)
    }

    /// **After the render thread encoded `inputs.frame`:** record whether it had
    /// to build anything (`missed`), and while the playhead is scrubbed start
    /// background pre-builds of the next [`SCRUB_HORIZON`] frames in its
    /// direction. `count` is the number of frames the timeline plays.
    pub fn after_encode(
        &mut self,
        main: &mut DrawCache,
        inputs: &StageInputs,
        count: u32,
        missed: bool,
    ) {
        self.pre.note_pass(missed);
        let frame = inputs.frame;
        if let Some(last) = self.last_frame
            && last != frame
        {
            self.dir = if frame > last { 1 } else { -1 };
        }
        self.last_frame = Some(frame);
        self.warm.insert(frame);
        if self.dir == 0
            || self.epoch.is_none()
            || !Self::usable(main)
            || !self.pre.is_hot()
            || Self::near_budget(main)
        {
            return;
        }
        let detail = main.detail();
        for step in 1..=SCRUB_HORIZON {
            if self.pre.in_flight() >= SCRUB_IN_FLIGHT {
                break;
            }
            let target = i64::from(frame) + i64::from(self.dir) * i64::from(step);
            if target < 0 || target >= i64::from(count) {
                break;
            }
            let target = target as u32;
            if !self.started.insert(target) {
                continue;
            }
            let mut job = inputs.clone();
            job.frame = target;
            if !self.pre.dispatch(job, detail) {
                self.started.remove(&target);
                break;
            }
            self.stats.scrub_jobs += 1;
        }
    }
}

/// **Everything outside the frame number that an encoding depends on.** Two
/// frames drawn under equal epochs are frames of the same picture; a
/// [`Prepared`] whose epoch differs from the current one is never shown.
///
/// `detail` and `buffer_scale` are what the cache was lent with: the cache's
/// own ladder moves them while it runs (reported in [`Prepared`]), and only a
/// change from *outside* ends the pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Epoch {
    pub revision: u64,
    pub camera: [u64; 4],
    pub area: [u64; 4],
    /// The display's `pixels_per_point`, as raw bits.
    pub scale_bits: u64,
    pub detail: LightDetail,
    pub lights_generation: u64,
    pub buffer_scale: u32,
}

/// How the playhead moves, so the thread can work out which frame plays next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayRange {
    /// Frames that play from the active scene (`Editor::playing_length`).
    pub count: u32,
    pub looping: bool,
}

/// The frame after `frame` in playback order, or `None` when playback stops
/// there. The same rules as `Editor::advance_playback` for one frame: a looping
/// section first, then the end of the timeline.
pub fn step_frame(scene: &buzz_scene::Scene, play: PlayRange, frame: u32) -> Option<u32> {
    let next = frame + 1;
    if let Some(back) = scene.looping().wrap(next) {
        return Some(back);
    }
    if next >= play.count {
        play.looping.then(|| next % play.count.max(1))
    } else {
        Some(next)
    }
}

/// Is `to` the playhead `from`, or at most [`LOOKAHEAD`] steps in front of it?
fn is_ahead(scene: &buzz_scene::Scene, play: PlayRange, from: u32, to: u32) -> bool {
    let mut f = from;
    for _ in 0..=LOOKAHEAD {
        if f == to {
            return true;
        }
        match step_frame(scene, play, f) {
            Some(n) => f = n,
            None => return false,
        }
    }
    false
}

/// What the lent cache looked like after the last frame it drew — what
/// `App::lights` would answer if the cache were at home.
#[derive(Clone, Copy, Debug)]
pub struct LightState {
    pub detail: LightDetail,
    pub scale: u32,
    pub empty: bool,
    pub stale: bool,
    pub last_lines: u64,
    pub over_budget: bool,
}

impl LightState {
    pub fn of(cache: &DrawCache) -> Self {
        Self {
            detail: cache.detail(),
            scale: cache.scale(),
            empty: cache.lights.is_empty(),
            stale: cache.lights.is_stale(),
            last_lines: cache.last_lines(),
            over_budget: cache.over_budget(),
        }
    }
}

/// One finished stage encoding.
pub struct Prepared {
    pub epoch: Epoch,
    pub frame: u32,
    pub scene: vello::Scene,
    pub light: LightState,
    pub encode_ms: f64,
}

impl Prepared {
    /// The buffer scale the cache judged this frame at; the render thread calls
    /// `GpuContext::grow_to` with it before rendering.
    pub fn scale(&self) -> u32 {
        self.light.scale
    }

    /// Did the frame overrun the rasteriser even after the ladder? The pipeline
    /// ends and the frame is drawn in bands.
    pub fn over_budget(&self) -> bool {
        self.light.over_budget
    }

    /// The flattened lines of the frame (as `DrawCache::last_lines`).
    pub fn lines(&self) -> u64 {
        self.light.last_lines
    }
}

enum Cmd {
    Start(Box<Start>),
    Playhead(u32),
    Stop,
}

struct Start {
    epoch: Epoch,
    inputs: StageInputs,
    play: PlayRange,
    first: u32,
    cache: DrawCache,
}

struct Job {
    epoch: Epoch,
    inputs: StageInputs,
    play: PlayRange,
    cache: DrawCache,
    /// The frame the render thread last said it wants.
    head: u32,
    /// The next frame to encode; `None` once a non-looping run has ended.
    next: Option<u32>,
}

/// The `buzz-encode` thread's state.
struct Worker {
    cmd_rx: Receiver<Cmd>,
    prepared_tx: Sender<Prepared>,
    back_tx: Sender<DrawCache>,
    recycle_rx: Receiver<vello::Scene>,
    abort: Arc<AtomicBool>,
    pool: Vec<vello::Scene>,
    made: usize,
    job: Option<Job>,
    closed: bool,
    /// E6: the stamp pre-build pool; `None` without a job system or under
    /// `BUZZ_NO_PREBUILD=1`.
    prebuild: Option<Prebuild>,
    trace: Arc<StopTrace>,
}

impl Worker {
    fn run(mut self) {
        while !self.closed {
            let busy = self.job.as_ref().is_some_and(|j| j.next.is_some());
            let cmd = if busy {
                match self.cmd_rx.try_recv() {
                    Ok(c) => Some(c),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else {
                match self.cmd_rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            };
            match cmd {
                Some(c) => self.apply(c),
                None => self.encode_one(),
            }
        }
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Start(start) => {
                self.trace.phase.store(PHASE_START, Ordering::Relaxed);
                let Start { epoch, mut inputs, play, first, mut cache } = *start;
                self.abort.store(false, Ordering::Relaxed);
                cache.set_abort(Arc::clone(&self.abort));
                // What a pass evicts is freed on the background pool, not here
                // (S7, stop latency): see `DrawCache::defer_frees`.
                cache.defer_frees(self.prebuild.is_some());
                // A new epoch: whatever the pool was building is for another one.
                if let Some(pb) = self.prebuild.as_mut() {
                    pb.reset(Some(&cache.symbols));
                }
                inputs.frame = first;
                self.job = Some(Job {
                    epoch,
                    inputs,
                    play,
                    cache,
                    head: first,
                    next: Some(first),
                });
            }
            Cmd::Playhead(frame) => {
                if let Some(job) = &mut self.job {
                    job.head = frame;
                }
            }
            Cmd::Stop => {
                self.trace.mark(&self.trace.taken);
                self.trace.phase.store(PHASE_IDLE, Ordering::Relaxed);
                // Cleared here, after the render thread raised it before
                // sending this, so the next job starts un-aborted.
                self.abort.store(false, Ordering::Relaxed);
                if let Some(pb) = self.prebuild.as_mut() {
                    pb.reset(None);
                }
                self.trace.mark(&self.trace.reset_done);
                if let Some(mut job) = self.job.take() {
                    // The cache goes home freeing for itself again, and what it
                    // had set aside is freed elsewhere.
                    job.cache.defer_frees(false);
                    let junk = job.cache.take_graveyard();
                    if let (Some(pb), false) = (&self.prebuild, junk.is_empty()) {
                        pb.reap(junk);
                    }
                    // Marked before the send: the render thread may read it as
                    // soon as the cache arrives.
                    self.trace.mark(&self.trace.returned);
                    let _ = self.back_tx.send(job.cache);
                }
            }
        }
    }

    /// A scene to draw into: one from the pool, one the render thread gave
    /// back, a new one while fewer than [`SCENES`] exist, else wait for one.
    /// `None` when a command arrived while waiting (already applied).
    fn scene(&mut self) -> Option<vello::Scene> {
        if let Some(s) = self.pool.pop() {
            return Some(s);
        }
        while let Ok(s) = self.recycle_rx.try_recv() {
            self.pool.push(s);
        }
        if let Some(s) = self.pool.pop() {
            return Some(s);
        }
        if self.made < SCENES {
            self.made += 1;
            return Some(vello::Scene::new());
        }
        let recycle_rx = self.recycle_rx.clone();
        let cmd_rx = self.cmd_rx.clone();
        let mut sel = Select::new();
        let r = sel.recv(&recycle_rx);
        let _c = sel.recv(&cmd_rx);
        let oper = sel.select();
        if oper.index() == r {
            return oper.recv(&recycle_rx).ok();
        }
        match oper.recv(&cmd_rx) {
            Ok(cmd) => self.apply(cmd),
            Err(_) => self.closed = true,
        }
        None
    }

    fn encode_one(&mut self) {
        let Some(mut vello) = self.scene() else {
            return;
        };
        let Some(job) = self.job.as_mut() else {
            self.pool.push(vello);
            return;
        };
        let Some(mut frame) = job.next else {
            self.pool.push(vello);
            return;
        };
        // Behind the playhead: nothing older than the frame being shown is of
        // any use, so skip ahead to it.
        if !is_ahead(&job.inputs.scene, job.play, job.head, frame) {
            frame = job.head;
        }
        job.inputs.frame = frame;
        // The cache grows its own scale as the ladder demands, and the line
        // budget the encode judges against is per thread.
        job.inputs.buffer_scale = job.inputs.buffer_scale.max(job.cache.scale());

        // E6: stamps the pool finished are installed before this frame, and
        // jobs for the frames after it are started.
        self.trace.phase.store(PHASE_TICK, Ordering::Relaxed);
        if let Some(pb) = self.prebuild.as_mut() {
            pb.tick(&mut job.cache, &job.inputs, job.play, frame);
        }
        let misses = |c: &DrawCache| c.symbol_scenes.builds + c.symbol_scenes.first_sightings;
        let before = misses(&job.cache);

        let started = Instant::now();
        self.trace.phase.store(PHASE_BUILD, Ordering::Relaxed);
        stage::build_plain_scene(&mut vello, &job.inputs, &mut job.cache);
        self.trace.phase.store(PHASE_DELIVER, Ordering::Relaxed);
        let encode_ms = started.elapsed().as_secs_f64() * 1e3;
        let junk = job.cache.take_graveyard();
        if let (Some(pb), false) = (&self.prebuild, junk.is_empty()) {
            pb.reap(junk);
        }
        if job.cache.is_aborted() {
            self.trace.mark(&self.trace.build_done);
            // Partial; thrown away. The command that raised the flag follows.
            self.pool.push(vello);
            return;
        }
        if let Some(pb) = self.prebuild.as_mut() {
            pb.note_pass(misses(&job.cache) > before);
        }
        job.next = step_frame(&job.inputs.scene, job.play, frame);
        let prepared = Prepared {
            epoch: job.epoch,
            frame,
            scene: vello,
            light: LightState::of(&job.cache),
            encode_ms,
        };
        self.deliver(prepared);
    }

    /// Put `prepared` on the queue, staying responsive to commands while it is
    /// full. A command that replaces the job drops it.
    fn deliver(&mut self, prepared: Prepared) {
        let tx = self.prepared_tx.clone();
        let cmd_rx = self.cmd_rx.clone();
        loop {
            let mut sel = Select::new();
            let s = sel.send(&tx);
            let _c = sel.recv(&cmd_rx);
            let oper = sel.select();
            if oper.index() == s {
                let _ = oper.send(&tx, prepared);
                return;
            }
            match oper.recv(&cmd_rx) {
                Ok(cmd) => {
                    let replaces = !matches!(cmd, Cmd::Playhead(_));
                    self.apply(cmd);
                    if replaces {
                        self.pool.push(prepared.scene);
                        return;
                    }
                }
                Err(_) => {
                    self.closed = true;
                    return;
                }
            }
        }
    }
}

/// What [`Pipeline::take`] found for the frame wanted.
pub enum Taken {
    /// An encoding of exactly that frame, under the running epoch.
    Ready(Prepared),
    /// That frame was already taken: its encoding is still in the render
    /// thread's scene.
    Shown,
    /// Not finished within the wait.
    NotReady,
}

/// What [`Pipeline::stop`] gave back.
pub enum Stopped {
    NotRunning,
    Returned(Box<DrawCache>),
    /// The encode thread is gone with the cache.
    Lost,
}

struct Running {
    epoch: Epoch,
    scene: Arc<buzz_scene::Scene>,
    play: PlayRange,
    shown: Option<u32>,
    stash: Vec<Prepared>,
    light: LightState,
}

/// Counters, for the log and for tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Frames handed to the render thread.
    pub taken: u64,
    /// Waits that ended without the frame.
    pub not_ready: u64,
    /// Encodings dropped as the wrong frame or the wrong epoch.
    pub discarded: u64,
    /// Pipeline stops.
    pub stops: u64,
    /// Milliseconds the last [`Pipeline::stop`] took until the cache was back.
    pub last_stop_ms: f64,
    /// The last stop's steps, milliseconds: the encode thread reaching the stop
    /// command after the abort was raised, its pre-build reset, handing the
    /// cache back, and the render thread receiving it.
    pub last_stop_parts_ms: [f64; 4],
    /// What the encode thread was busy with when the last stop was raised, and
    /// how long after that its aborted pass finished drawing (ms).
    pub last_stop_phase: &'static str,
    pub last_stop_build_tail_ms: f64,
}

/// **Where a [`Pipeline::stop`] spends its time** (S7, stop latency): each step's
/// timestamp in nanoseconds after `base`, written by the thread that reaches it.
struct StopTrace {
    base: Instant,
    raised: AtomicU64,
    taken: AtomicU64,
    reset_done: AtomicU64,
    returned: AtomicU64,
    /// What the encode thread was doing (`PHASE_*`) when the abort was raised.
    phase: AtomicU8,
    /// When the last aborted pass finished drawing.
    build_done: AtomicU64,
}

/// What the encode thread is busy with, for [`StopTrace::phase`].
const PHASE_IDLE: u8 = 0;
const PHASE_TICK: u8 = 1;
const PHASE_BUILD: u8 = 2;
const PHASE_DELIVER: u8 = 3;
const PHASE_START: u8 = 4;
const PHASE_NAMES: [&str; 5] = ["idle", "tick", "build", "deliver", "start"];

impl StopTrace {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            raised: AtomicU64::new(0),
            taken: AtomicU64::new(0),
            reset_done: AtomicU64::new(0),
            returned: AtomicU64::new(0),
            phase: AtomicU8::new(PHASE_IDLE),
            build_done: AtomicU64::new(0),
        }
    }

    fn mark(&self, at: &AtomicU64) {
        at.store(self.base.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

struct Channels {
    cmd_tx: Sender<Cmd>,
    prepared_rx: Receiver<Prepared>,
    back_rx: Receiver<DrawCache>,
    recycle_tx: Sender<vello::Scene>,
}

/// The render thread's end of the encode thread. Spawned on first use.
pub struct Pipeline {
    chan: Option<Channels>,
    abort: Arc<AtomicBool>,
    running: Option<Running>,
    pub stats: Stats,
    /// E6: where the stamp pre-build runs; `None` pre-builds nothing.
    jobs: Option<Arc<JobSystem>>,
    prebuild_counters: Arc<PrebuildCounters>,
    trace: Arc<StopTrace>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            chan: None,
            abort: Arc::new(AtomicBool::new(false)),
            running: None,
            stats: Stats::default(),
            jobs: None,
            prebuild_counters: Arc::new(PrebuildCounters::default()),
            trace: Arc::new(StopTrace::new()),
        }
    }

    /// A pipeline whose encode thread pre-builds the stamps of the frames
    /// ahead on `jobs`' interactive pool (E6). Unchanged when
    /// `BUZZ_NO_PREBUILD=1`.
    pub fn with_jobs(jobs: Arc<JobSystem>) -> Self {
        let mut pipeline = Self::new();
        pipeline.jobs = Some(jobs);
        pipeline
    }

    /// The app's pipeline: pre-builds only when
    /// [`playback_prebuild_enabled`] (off by default).
    pub fn for_app(jobs: Arc<JobSystem>) -> Self {
        if playback_prebuild_enabled() { Self::with_jobs(jobs) } else { Self::new() }
    }

    /// What the pre-build pool has done so far.
    pub fn prebuild_stats(&self) -> PrebuildStats {
        PrebuildStats {
            dispatched: self.prebuild_counters.dispatched.load(Ordering::Relaxed),
            installed: self.prebuild_counters.installed.load(Ordering::Relaxed),
            stale: self.prebuild_counters.stale.load(Ordering::Relaxed),
        }
    }

    /// Is a cache lent out right now?
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Encodings finished and waiting for the render thread (queued or stashed).
    pub fn queued(&self) -> usize {
        match (self.running.as_ref(), self.chan.as_ref()) {
            (Some(r), Some(c)) => c.prepared_rx.len() + r.stash.len(),
            _ => 0,
        }
    }

    /// The epoch the running pipeline draws under.
    pub fn epoch(&self) -> Option<&Epoch> {
        self.running.as_ref().map(|r| &r.epoch)
    }

    /// The lent cache's state after the last frame taken (or as lent).
    pub fn light(&self) -> Option<LightState> {
        self.running.as_ref().map(|r| r.light)
    }

    fn spawn(&mut self) -> &Channels {
        self.chan.get_or_insert_with(|| {
            let (cmd_tx, cmd_rx) = unbounded();
            let (prepared_tx, prepared_rx) = bounded(QUEUE_DEPTH);
            let (back_tx, back_rx) = bounded(1);
            let (recycle_tx, recycle_rx) = unbounded();
            let prebuild = self
                .jobs
                .clone()
                .filter(|_| prebuild_enabled())
                .map(|jobs| Prebuild::with_counters(jobs, Arc::clone(&self.prebuild_counters)));
            let worker = Worker {
                cmd_rx,
                prepared_tx,
                back_tx,
                recycle_rx,
                abort: Arc::clone(&self.abort),
                pool: Vec::new(),
                made: 0,
                job: None,
                closed: false,
                prebuild,
                trace: Arc::clone(&self.trace),
            };
            std::thread::Builder::new()
                .name("buzz-encode".into())
                .spawn(move || worker.run())
                .expect("spawning the encode thread");
            Channels { cmd_tx, prepared_rx, back_rx, recycle_tx }
        })
    }

    /// Lend `cache` to the encode thread. Frame `shown` is already on screen;
    /// the thread starts encoding at `first` (the frame after it).
    ///
    /// `inputs` carries the scene, camera, area, scale and buffer scale every
    /// frame is drawn with; its `frame` is ignored.
    pub fn start(
        &mut self,
        cache: DrawCache,
        epoch: Epoch,
        inputs: StageInputs,
        play: PlayRange,
        first: u32,
        shown: u32,
    ) {
        assert!(self.running.is_none(), "a pipeline is already running");
        let light = LightState::of(&cache);
        let scene = Arc::clone(&inputs.scene);
        self.abort.store(false, Ordering::Relaxed);
        let chan = self.spawn();
        let start = Start { epoch, inputs, play, first, cache };
        let _ = chan.cmd_tx.send(Cmd::Start(Box::new(start)));
        self.running = Some(Running {
            epoch,
            scene,
            play,
            shown: Some(shown),
            stash: Vec::new(),
            light,
        });
    }

    /// Stop drawing ahead and take the cache back. Raises the abort flag first,
    /// so a frame in the middle of being drawn ends at its next layer (E5);
    /// everything queued is discarded.
    pub fn stop(&mut self) -> Stopped {
        let Some(running) = self.running.take() else {
            return Stopped::NotRunning;
        };
        let started = Instant::now();
        self.stats.stops += 1;
        let Some(chan) = self.chan.as_ref() else {
            return Stopped::Lost;
        };
        self.trace.mark(&self.trace.raised);
        let phase = self.trace.phase.load(Ordering::Relaxed);
        self.abort.store(true, Ordering::Relaxed);
        if chan.cmd_tx.send(Cmd::Stop).is_err() {
            return Stopped::Lost;
        }
        let cache = chan.back_rx.recv();
        let received = self.trace.base.elapsed().as_nanos() as u64;
        // Nothing is produced after the cache is handed back.
        for p in chan.prepared_rx.try_iter() {
            let _ = chan.recycle_tx.send(p.scene);
        }
        for p in running.stash {
            let _ = chan.recycle_tx.send(p.scene);
        }
        self.stats.last_stop_ms = started.elapsed().as_secs_f64() * 1e3;
        let t = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let (raised, taken) = (t(&self.trace.raised), t(&self.trace.taken));
        let (reset, returned) = (t(&self.trace.reset_done), t(&self.trace.returned));
        let d = |a: u64, b: u64| b.saturating_sub(a) as f64 * 1e-6;
        self.stats.last_stop_parts_ms =
            [d(raised, taken), d(taken, reset), d(reset, returned), d(returned, received)];
        self.stats.last_stop_phase = PHASE_NAMES.get(usize::from(phase)).copied().unwrap_or("?");
        let built = t(&self.trace.build_done);
        self.stats.last_stop_build_tail_ms = if built > raised { d(raised, built) } else { 0.0 };
        match cache {
            Ok(cache) => Stopped::Returned(Box::new(cache)),
            Err(_) => Stopped::Lost,
        }
    }

    /// Tell the thread where the playhead is now.
    pub fn playhead(&self, frame: u32) {
        if self.running.is_some()
            && let Some(chan) = &self.chan
        {
            let _ = chan.cmd_tx.send(Cmd::Playhead(frame));
        }
    }

    /// Give a scene the render thread has finished with back for reuse.
    pub fn recycle(&self, scene: vello::Scene) {
        if let Some(chan) = &self.chan {
            let _ = chan.recycle_tx.send(scene);
        }
    }

    /// The encoding of `frame`, waiting up to `wait` for it. Encodings of other
    /// frames that are still to come are kept; the rest go back for reuse.
    pub fn take(&mut self, frame: u32, wait: Duration) -> Taken {
        let (Some(running), Some(chan)) = (self.running.as_mut(), self.chan.as_ref()) else {
            return Taken::NotReady;
        };
        if running.shown == Some(frame) {
            return Taken::Shown;
        }
        let epoch = running.epoch;
        let (scene, play) = (Arc::clone(&running.scene), running.play);
        // Anything kept that is no longer in front of the playhead is dead.
        let mut i = 0;
        while i < running.stash.len() {
            if is_ahead(&scene, play, frame, running.stash[i].frame) {
                i += 1;
            } else {
                let dead = running.stash.swap_remove(i);
                let _ = chan.recycle_tx.send(dead.scene);
                self.stats.discarded += 1;
            }
        }
        let mut found = running
            .stash
            .iter()
            .position(|p| p.frame == frame)
            .map(|i| running.stash.swap_remove(i));
        let deadline = Instant::now() + wait;
        while found.is_none() {
            let got = match chan.prepared_rx.try_recv() {
                Ok(p) => Some(p),
                Err(TryRecvError::Empty) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        None
                    } else {
                        chan.prepared_rx.recv_timeout(left).ok()
                    }
                }
                Err(TryRecvError::Disconnected) => None,
            };
            let Some(p) = got else {
                break;
            };
            if p.epoch != epoch {
                let _ = chan.recycle_tx.send(p.scene);
                self.stats.discarded += 1;
            } else if p.frame == frame {
                found = Some(p);
            } else if is_ahead(&scene, play, frame, p.frame) {
                running.stash.push(p);
            } else {
                let _ = chan.recycle_tx.send(p.scene);
                self.stats.discarded += 1;
            }
        }
        match found {
            Some(p) => {
                running.shown = Some(frame);
                running.light = p.light;
                self.stats.taken += 1;
                Taken::Ready(p)
            }
            None => {
                self.stats.not_ready += 1;
                Taken::NotReady
            }
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        // The thread ends when its command channel closes; a frame it is still
        // drawing is cut short.
        self.abort.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod stop_latency_tests {
    use super::*;

    /// Reports the thread it is dropped on.
    struct Probe(std::sync::mpsc::Sender<std::thread::ThreadId>);

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.0.send(std::thread::current().id());
        }
    }

    /// S7 (stop latency): what `reset` and a stale result throw away is freed on
    /// the background pool, never on the calling thread.
    #[test]
    fn reap_frees_on_another_thread() {
        let pb = Prebuild::new(Arc::new(JobSystem::with_sizes(1, 1)));
        let (tx, rx) = std::sync::mpsc::channel();
        pb.reap(Probe(tx));
        let on = rx.recv_timeout(Duration::from_secs(5)).expect("dropped");
        assert_ne!(on, std::thread::current().id());
    }

    #[test]
    fn reset_hands_the_idle_slots_off_instead_of_holding_or_freeing_them() {
        let mut pb = Prebuild::new(Arc::new(JobSystem::with_sizes(1, 1)));
        pb.idle.push(pb.new_slot());
        pb.idle.push(pb.new_slot());
        pb.reset(None);
        assert!(pb.idle.is_empty());
    }
}
