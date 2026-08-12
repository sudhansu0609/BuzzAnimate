//! Per-worker CPU time accounting.
//!
//! # Why not time the jobs themselves
//!
//! The obvious approach is to wrap each submitted job in a timer. It does not
//! work here. A single submitted job typically expands into a `par_iter` that
//! fans out across every worker in the pool, so timing the submission measures
//! one worker and misses the actual parallelism entirely — precisely the thing
//! the HUD exists to display.
//!
//! # What this does instead
//!
//! Each pool worker registers its OS thread handle on startup, and utilisation
//! is derived from the kernel + user CPU time the operating system attributes
//! to that thread. That counts real work no matter how it was scheduled,
//! including nested rayon constructs, and it excludes time the thread spent
//! parked waiting for work.
//!
//! It also excludes *other processes*, which a system-wide per-core reading
//! would not. The HUD should show BuzzAnimate's own utilisation.

use std::sync::Mutex;

/// Registry of worker thread handles, indexed by rayon worker index.
#[derive(Debug, Default)]
pub struct WorkerClocks {
    /// `HANDLE` stored as `usize` so the registry stays `Send + Sync`.
    handles: Mutex<Vec<Option<usize>>>,
}

impl WorkerClocks {
    pub fn new(workers: usize) -> Self {
        Self {
            handles: Mutex::new(vec![None; workers]),
        }
    }

    /// Called from each worker as it starts.
    pub fn register(&self, index: usize) {
        if let Some(handle) = platform::current_thread_handle()
            && let Ok(mut slots) = self.handles.lock()
        {
            if index >= slots.len() {
                slots.resize(index + 1, None);
            }
            if let Some(old) = slots[index].replace(handle) {
                platform::close_handle(old);
            }
        }
    }

    /// Called from each worker as it exits.
    pub fn unregister(&self, index: usize) {
        if let Ok(mut slots) = self.handles.lock()
            && let Some(slot) = slots.get_mut(index)
            && let Some(handle) = slot.take()
        {
            platform::close_handle(handle);
        }
    }

    /// Cumulative CPU nanoseconds consumed by each worker since it started.
    ///
    /// Workers that have not registered, or platforms without support, report
    /// zero — callers should treat a flat all-zero reading as "unavailable"
    /// rather than "idle".
    pub fn cpu_nanos(&self) -> Vec<u64> {
        let slots = match self.handles.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        slots
            .iter()
            .map(|h| h.and_then(platform::thread_cpu_nanos).unwrap_or(0))
            .collect()
    }

    /// Whether this platform can report CPU time at all.
    pub const fn supported() -> bool {
        platform::SUPPORTED
    }
}

impl Drop for WorkerClocks {
    fn drop(&mut self) {
        if let Ok(mut slots) = self.handles.lock() {
            for slot in slots.iter_mut() {
                if let Some(h) = slot.take() {
                    platform::close_handle(h);
                }
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    pub const SUPPORTED: bool = true;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        /// FILETIME counts 100-nanosecond intervals.
        fn nanos(self) -> u64 {
            (((self.high as u64) << 32) | self.low as u64) * 100
        }
    }

    const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;

    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn GetCurrentProcess() -> isize;
        fn DuplicateHandle(
            source_process: isize,
            source_handle: isize,
            target_process: isize,
            target_handle: *mut isize,
            desired_access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        fn GetThreadTimes(
            thread: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    /// `GetCurrentThread` returns a pseudo-handle that is only valid on the
    /// calling thread, so it must be duplicated into a real handle before
    /// another thread can read this thread's times.
    pub fn current_thread_handle() -> Option<usize> {
        let mut real: isize = 0;
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut real,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        (ok != 0 && real != 0).then_some(real as usize)
    }

    pub fn thread_cpu_nanos(handle: usize) -> Option<u64> {
        let (mut c, mut e, mut k, mut u) = (
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
        );
        let ok = unsafe { GetThreadTimes(handle as isize, &mut c, &mut e, &mut k, &mut u) };
        (ok != 0).then(|| k.nanos() + u.nanos())
    }

    pub fn close_handle(handle: usize) {
        unsafe {
            CloseHandle(handle as isize);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    // BuzzAnimate targets Windows first. Utilisation reporting degrades to
    // "unavailable" elsewhere; nothing else in the job system depends on it.
    pub const SUPPORTED: bool = false;

    pub fn current_thread_handle() -> Option<usize> {
        None
    }
    pub fn thread_cpu_nanos(_handle: usize) -> Option<u64> {
        None
    }
    pub fn close_handle(_handle: usize) {}
}
