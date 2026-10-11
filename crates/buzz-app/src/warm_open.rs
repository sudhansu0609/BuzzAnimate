//! **S9 of `docs/BIMARI5_LAG_PLAN.md`: all preparation happens while the
//! document opens.**
//!
//! Playback is smooth only when every symbol stamp a frame needs already
//! exists; a stamp met for the first time costs a build (and, before S9, the
//! frame that met it drew it live first). So at open the film is walked in
//! playback order on the worker pool, **encode only** (no raster), with the
//! stamp cache's build-on-first-sighting switch on, and every stamp that turns
//! up is kept. A stamp's bytes depend only on its key (E3), so what a worker
//! builds is exactly what the stage would have built on demand.
//!
//! # Shape
//!
//! [`run_warm`] is one coordinator (a thread of its own in the app, behind the
//! Tasks panel's progress bar; `frame_cost --warm-open` calls it directly) and
//! up to [`MAX_WORKERS`] jobs on `Pool::Interactive`. A job is a [`CHUNK`] of
//! consecutive frames on a slot's own [`DrawCache`], so the "static across
//! generations" rules inside the encode see consecutive generations just as
//! the stage does. Before each job a slot is **synced** with the coordinator's
//! log of every stamp kept so far (shared `Arc`s, never copies) and afterwards
//! it keeps none, so a symbol shared by the whole film is built about once per
//! job at worst, and memory holds one copy of each kept stamp (a stamp the
//! budget turned away is freed at once).
//!
//! # Budget
//!
//! Never evicts: the warm-up fills [`warm_share`] (85%) of the cache's stamp
//! limit; when that is reached dispatch stops, jobs in flight finish, and a
//! stamp that no longer fits is dropped (its chunk does not count as covered).
//! [`WarmReport::frames_covered`] is the **prefix** of the playback order that
//! is fully warm. The rest of the limit is headroom: a cache a warm-up filled
//! is in **window mode** (`DrawCache::set_stamp_window`), so past the warmed
//! frames the pre-build carries on and the stamps of frames the playhead has
//! left go first.
//!
//! # Sizing
//!
//! The heaviest encoded frame's lines and segments give
//! the buffer scale to grow to straight away ([`apply`], the S4 formula in
//! [`buzz_render::document::fitting_scale`]); the first render after it
//! allocates the buffers with an empty scene (`WarmState::prealloc`), so no
//! frame of a playback pays for them.
//!
//! Off by default; `BUZZ_WARM_OPEN=1` turns it on (see [`enabled`]); `BUZZ_STAMP_CACHE_MB` sets the
//! stamp limit (default a quarter of RAM); `BUZZ_NO_STAMP_WINDOW=1` turns window
//! mode off.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use buzz_jobs::{JobSystem, Pool};
use buzz_render::document::{DrawCache, LightDetail, NewStamp, SymKey, SymbolTable, fitting_scale};
use crossbeam_channel::{RecvTimeoutError, unbounded};

use crate::stage::{self, StageInputs};

/// Consecutive frames one job encodes.
pub const CHUNK: usize = 8;
/// Most jobs in flight at once (also bounded by the pool).
pub const MAX_WORKERS: usize = 12;
/// How long the camera must hold still before a warm-up starts (a zoom
/// gesture or the first layout must not warm a level nobody stays at).
pub const SETTLE: Duration = Duration::from_millis(350);

/// **Off by default** (user 2026-10-01: within the stamp budget it warms only
/// the first ~600 of Bimari5's 4965 frames and made the whole film worse; see
/// the S9 review in `docs/BIMARI5_LAG_PLAN.md`). `BUZZ_WARM_OPEN=1` turns it
/// on. Read once.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("BUZZ_WARM_OPEN").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Physical RAM in bytes (a conservative 8 GiB when it cannot be read).
pub fn physical_ram_bytes() -> u64 {
    const FALLBACK: u64 = 8 << 30;
    #[cfg(windows)]
    {
        #[repr(C)]
        struct MemoryStatusEx {
            length: u32,
            memory_load: u32,
            total_phys: u64,
            avail_phys: u64,
            total_page_file: u64,
            avail_page_file: u64,
            total_virtual: u64,
            avail_virtual: u64,
            avail_extended_virtual: u64,
        }
        unsafe extern "system" {
            fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        }
        // SAFETY: a zeroed plain-old-data struct with `length` set, as the API asks.
        let mut m: MemoryStatusEx = unsafe { std::mem::zeroed() };
        m.length = std::mem::size_of::<MemoryStatusEx>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut m) };
        if ok != 0 && m.total_phys > 0 { m.total_phys } else { FALLBACK }
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
                let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
                Some(kb * 1024)
            })
            .unwrap_or(FALLBACK)
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(FALLBACK)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        FALLBACK
    }
}

/// The stamp budget the warm-up works to: `BUZZ_STAMP_CACHE_MB` when set, else
/// a quarter of physical RAM (never less than the cache's own 512 MiB default).
pub fn default_budget_bytes() -> usize {
    if let Some(mb) = std::env::var("BUZZ_STAMP_CACHE_MB").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return mb.saturating_mul(1 << 20);
    }
    ((physical_ram_bytes() / 4) as usize).max(512 << 20)
}


/// **The part of the stamp limit a warm-up may fill**: 85%. The rest is the
/// headroom the stage needs to keep building ahead when playback passes the
/// warmed frames (the pre-build stops above 90% of the limit, and a cache that
/// ended a warm-up full would start playback by evicting what it had just built).
pub fn warm_share(limit_bytes: usize) -> usize {
    limit_bytes / 100 * 85
}

/// What one warm-up is to do.
pub struct WarmJob {
    /// The stage as it will be shown (camera, area, output scale); `frame` is
    /// overwritten per frame.
    pub inputs: StageInputs,
    /// The frames to encode, in playback order.
    pub frames: Vec<u32>,
    /// Bytes of stamps the warm-up may add. Never exceeded ([`warm_share`] of the
    /// limit, less what the cache already holds).
    pub budget_bytes: usize,
    /// The stamp cache's whole byte limit once the warm-up is installed
    /// (`BUZZ_STAMP_CACHE_MB` or a quarter of RAM; see [`default_budget_bytes`]).
    pub limit_bytes: usize,
    /// Jobs in flight at once.
    pub workers: usize,
    /// The detail level the stage's cache is at (it is part of a stamp's key).
    pub detail: LightDetail,
    /// The stage cache's symbol table, so a slot need not re-walk the library.
    pub symbols: SymbolTable,
}

/// What a warm-up did.
#[derive(Clone, Debug, Default)]
pub struct WarmReport {
    pub frames_total: u32,
    /// Length of the prefix of the frame list that is fully warm.
    pub frames_covered: u32,
    /// Frames encoded in all (a chunk past the budget stop may be encoded too).
    pub frames_encoded: u32,
    pub stamp_keys: usize,
    pub stamp_bytes: usize,
    pub budget_bytes: usize,
    /// The cache's whole limit (see [`WarmJob::limit_bytes`]).
    pub limit_bytes: usize,
    /// Dispatch stopped because the budget was reached.
    pub stopped_by_budget: bool,
    /// Stamps built but dropped because they no longer fit.
    pub dropped_stamps: usize,
    pub cancelled: bool,
    /// Heaviest encoded frame: estimated lines and the frame.
    pub peak_lines: u64,
    pub peak_lines_frame: u32,
    /// Most encoded segments of any encoded frame and the frame.
    pub peak_segments: u32,
    pub peak_segments_frame: u32,
    pub wall_s: f64,
    /// `(frames fully warm so far, stamp bytes so far)` each time the warm
    /// prefix grew: how much memory the first N frames need. Exact with one
    /// worker, an upper bound otherwise.
    pub curve: Vec<(u32, usize)>,
    /// The buffer scale [`apply`] grew the cache to (`0` before it ran).
    pub fit_scale: u32,
}

/// What a warm-up produced: the stamps (retained by whoever installs them) and
/// the report.
pub struct WarmOutput {
    pub stamps: Vec<NewStamp>,
    pub report: WarmReport,
}

/// One worker's persistent cache (emptied of stamps after every job: the
/// coordinator's log is the one place a stamp lives).
struct Slot {
    cache: DrawCache,
    scratch: vello::Scene,
}

struct Done {
    slot: Slot,
    chunk: usize,
    stamps: Vec<NewStamp>,
    aborted: bool,
    encoded: u32,
    /// `(lines, frame)` and `(segments, frame)` peaks of the chunk.
    peak_lines: (u64, u32),
    peak_segments: (u32, u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChunkState {
    Pending,
    Running,
    /// Encoded, and every stamp it built was kept.
    Done,
    /// Encoded, but a stamp did not fit the budget.
    Partial,
}

/// Walk `job.frames` on the worker pool and collect every stamp they need.
/// Blocks until finished, out of budget or `cancelled` answers `true`;
/// `progress(chunks_done_frames, total_frames)` is called as results arrive.
/// A cancelled run returns no stamps.
pub fn run_warm(
    jobs: &Arc<JobSystem>,
    job: WarmJob,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u64, u64),
) -> WarmOutput {
    let started = Instant::now();
    let WarmJob { inputs, frames, budget_bytes, limit_bytes, workers, detail, symbols } = job;
    let total = frames.len();
    let chunks: Vec<Vec<u32>> = frames.chunks(CHUNK).map(<[u32]>::to_vec).collect();
    let workers = workers.clamp(1, MAX_WORKERS).min(jobs.interactive_threads().max(1));
    let abort = Arc::new(AtomicBool::new(false));
    let (tx, rx) = unbounded::<Done>();

    let mut state = vec![ChunkState::Pending; chunks.len()];
    let mut peaks: Vec<((u64, u32), (u32, u32))> = vec![((0, 0), (0, 0)); chunks.len()];
    let mut idle: Vec<Slot> = Vec::new();
    let mut log: Vec<NewStamp> = Vec::new();
    let mut keys: HashSet<SymKey> = HashSet::new();
    let mut bytes = 0usize;
    let (mut next, mut in_flight) = (0usize, 0usize);
    let (mut encoded, mut dropped) = (0u32, 0usize);
    let (mut stop_dispatch, mut by_budget, mut was_cancelled) = (false, false, false);
    let mut curve: Vec<(u32, usize)> = Vec::new();
    let mut prefix_chunks = 0usize;

    loop {
        while !stop_dispatch && in_flight < workers && next < chunks.len() {
            let mut slot = idle.pop().unwrap_or_else(|| {
                let mut cache = DrawCache::new();
                cache.set_build_on_first_sighting(true);
                cache.set_collect_new_stamps(true);
                // A slot never evicts: the warm-up's own budget is the limit.
                cache.set_stamp_limits(usize::MAX / 2, usize::MAX);
                cache.set_abort(Arc::clone(&abort));
                cache.adopt_symbols(&symbols);
                cache.pin_detail(Some(detail));
                Slot { cache, scratch: vello::Scene::new() }
            });
            // Everything installed so far, as shared `Arc`s (the slot keeps none
            // of it afterwards, so a stamp the budget turned away is freed).
            let sync: Vec<NewStamp> = log.clone();
            let (chunk, frames, inputs, tx) = (next, chunks[next].clone(), inputs.clone(), tx.clone());
            state[chunk] = ChunkState::Running;
            next += 1;
            in_flight += 1;
            jobs.spawn(Pool::Interactive, move || {
                let mut slot = slot;
                let before = buzz_render::vello_encoding::buffer_scale();
                slot.cache.install_stamps_retained(sync);
                let cull = stage::cull_for(&inputs.camera);
                let mut inputs = inputs;
                let (mut peak_lines, mut peak_segments) = ((0u64, 0u32), (0u32, 0u32));
                let (mut aborted, mut done) = (false, 0u32);
                for f in frames {
                    inputs.frame = f;
                    slot.scratch.reset();
                    slot.cache.begin();
                    let (segments, lines) = stage::encode_plain(&mut slot.scratch, &inputs, &mut slot.cache, cull);
                    let was_aborted = slot.cache.is_aborted();
                    slot.cache.end();
                    if was_aborted {
                        aborted = true;
                        break;
                    }
                    done += 1;
                    if lines > peak_lines.0 {
                        peak_lines = (lines, f);
                    }
                    if segments > peak_segments.0 {
                        peak_segments = (segments, f);
                    }
                }
                buzz_render::vello_encoding::set_scale_for_this_thread(before);
                let stamps = slot.cache.drain_new_stamps();
                slot.cache.drop_stamps();
                let _ = tx.send(Done {
                    slot,
                    chunk,
                    stamps,
                    aborted,
                    encoded: done,
                    peak_lines,
                    peak_segments,
                });
            });
        }
        if in_flight == 0 {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(done) => {
                in_flight -= 1;
                let Done { slot, chunk, stamps, aborted, encoded: n, peak_lines, peak_segments } = done;
                idle.push(slot);
                if !aborted && !was_cancelled {
                    encoded += n;
                    peaks[chunk] = (peak_lines, peak_segments);
                    let mut partial = false;
                    for s in stamps {
                        if keys.contains(&s.key) {
                            continue;
                        }
                        if bytes + s.bytes > budget_bytes {
                            dropped += 1;
                            partial = true;
                            continue;
                        }
                        bytes += s.bytes;
                        keys.insert(s.key);
                        log.push(s);
                    }
                    state[chunk] = if partial { ChunkState::Partial } else { ChunkState::Done };
                    if partial || bytes.saturating_mul(100) >= budget_bytes.saturating_mul(97) {
                        stop_dispatch = true;
                        by_budget = true;
                    }
                    while prefix_chunks < chunks.len() && state[prefix_chunks] == ChunkState::Done {
                        prefix_chunks += 1;
                        let frames_done: usize = chunks[..prefix_chunks].iter().map(Vec::len).sum();
                        curve.push((frames_done as u32, bytes));
                    }
                    let finished: usize = state
                        .iter()
                        .zip(&chunks)
                        .filter(|(s, _)| matches!(s, ChunkState::Done | ChunkState::Partial))
                        .map(|(_, c)| c.len())
                        .sum();
                    progress(finished as u64, total as u64);
                } else {
                    state[chunk] = ChunkState::Pending;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !was_cancelled && cancelled() {
            was_cancelled = true;
            stop_dispatch = true;
            abort.store(true, Ordering::Relaxed);
        }
    }

    let covered_chunks = state.iter().take_while(|s| **s == ChunkState::Done).count();
    let frames_covered: usize = chunks[..covered_chunks].iter().map(Vec::len).sum();
    let mut report = WarmReport {
        frames_total: total as u32,
        frames_covered: frames_covered as u32,
        frames_encoded: encoded,
        stamp_keys: log.len(),
        stamp_bytes: bytes,
        budget_bytes,
        limit_bytes,
        stopped_by_budget: by_budget,
        dropped_stamps: dropped,
        cancelled: was_cancelled,
        curve,
        ..WarmReport::default()
    };
    // Every frame that was encoded counts for sizing, a chunk whose stamps the
    // budget turned away included: its lines are as real as any.
    for (p, s) in peaks.iter().zip(&state) {
        if !matches!(s, ChunkState::Done | ChunkState::Partial) {
            continue;
        }
        if p.0.0 > report.peak_lines {
            (report.peak_lines, report.peak_lines_frame) = p.0;
        }
        if p.1.0 > report.peak_segments {
            (report.peak_segments, report.peak_segments_frame) = p.1;
        }
    }
    report.wall_s = started.elapsed().as_secs_f64();
    // The slots (and their copies of `Arc`s) go on the background pool: a
    // cancelled or finished warm-up must not free gigabytes on its own thread.
    jobs.spawn(Pool::Background, move || drop((idle, symbols)));
    if was_cancelled {
        jobs.spawn(Pool::Background, move || drop(log));
        return WarmOutput { stamps: Vec::new(), report };
    }
    WarmOutput { stamps: log, report }
}

/// **Install a finished warm-up into the stage's cache and size it once.**
/// The stamps are retained, the cache's byte budget and entry ceiling are
/// raised to what the warm-up worked to (so nothing it built is evicted), and
/// the buffer scale goes straight to the one that fits the heaviest warmed
/// frame; the caller then grows its GPU to [`DrawCache::scale`].
pub fn apply(cache: &mut DrawCache, out: WarmOutput, output_pixels: f64) -> WarmReport {
    let WarmOutput { stamps, mut report } = out;
    let limit = report
        .limit_bytes
        .max(cache.stamp_bytes().saturating_add(report.budget_bytes))
        .max(cache.stamp_budget());
    cache.set_stamp_limits(limit, usize::MAX);
    // The film may be larger than the budget: slide along it, not thrash.
    // (`BUZZ_NO_STAMP_WINDOW=1` turns it off, for A/B runs.)
    cache.set_stamp_window(std::env::var("BUZZ_NO_STAMP_WINDOW").map_or(true, |v| v.is_empty() || v == "0"));
    cache.install_stamps_retained(stamps);
    let fit = fitting_scale(report.peak_lines, report.peak_segments, output_pixels, cache.max_scale());
    if fit > cache.scale() {
        cache.set_scale(fit);
    }
    report.fit_scale = cache.scale();
    report
}

/// A frame list from `first` through the end of the film and then round to the
/// frames before it (the order a looping playback meets them in).
pub fn frames_from(first: u32, count: u32) -> Vec<u32> {
    let first = first.min(count.saturating_sub(1));
    (first..count).chain(0..first).collect()
}

/// Is the stage's camera level (`zoom` x output scale) the one `warmed` was
/// done for?
pub fn same_level(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1e-12)
}

/// A warm-up the app has started and not yet collected.
pub struct Running {
    pub task: crate::tasks::TaskId,
    pub rx: crossbeam_channel::Receiver<WarmOutput>,
    /// Which document activation it belongs to (a tab switch ends it).
    pub activation: u64,
    /// The camera level (`zoom` x output scale) it warms.
    pub level: f64,
    /// A re-warm for a zoom change, not the one at open.
    pub zoom: bool,
}

/// What `App` keeps about the warm-up (S9).
#[derive(Default)]
pub struct WarmState {
    /// The warm-up is switched on for this app ([`enabled`]; tests set it).
    pub on: bool,
    /// The document was just opened or switched to and has not been warmed.
    pub needs: bool,
    /// The user stopped a warm-up: no more until the next document.
    pub declined: bool,
    /// The app itself cancelled the running warm-up (a tab switch, a zoom).
    pub superseded: bool,
    pub running: Option<Running>,
    /// The camera level whose stamps the cache holds.
    pub warmed_level: Option<f64>,
    /// The level the camera has held since when (the settle timer).
    pub settle: Option<(f64, Instant)>,
    /// Bumped on every document activation.
    pub activation: u64,
    /// The last warm-up's report.
    pub last: Option<WarmReport>,
    /// The buffers were just grown: the next single-pass render starts with an
    /// empty-scene render that allocates them (see `App::render`).
    pub prealloc: bool,
    /// Zoom re-warms completed, and what the last one cost.
    pub zoom_warms: u32,
    pub last_zoom_warm_s: Option<f64>,
}

impl WarmState {
    /// Something here wants the next frame (a warm-up to collect, a timer to
    /// watch).
    pub fn busy(&self) -> bool {
        self.running.is_some() || self.settle.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_from_is_the_playback_order() {
        assert_eq!(frames_from(0, 4), vec![0, 1, 2, 3]);
        assert_eq!(frames_from(2, 5), vec![2, 3, 4, 0, 1]);
        assert_eq!(frames_from(9, 3), vec![2, 0, 1]);
    }

    #[test]
    fn levels_compare_by_relative_difference() {
        assert!(same_level(1.5, 1.5 + 1e-9));
        assert!(!same_level(1.5, 1.51));
    }

    #[test]
    fn the_default_budget_is_a_quarter_of_ram_or_the_override() {
        // Not asserting on the environment: only the shape.
        let b = default_budget_bytes();
        assert!(b >= 1 << 20);
    }
}
