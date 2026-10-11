//! **Keeps the GPU out of its idle power states while a playback runs** (S7 in
//! `docs/BIMARI5_LAG_PLAN.md`).
//!
//! A desktop GPU drops its core and memory clocks (NVIDIA P5/P8: memory clock
//! 810 or 405 MHz instead of 13.8 GHz) after a stretch of light frames. A film
//! whose frames alternate between a 2 ms background shot and a 25 ms heavy one
//! then pays the clock ramp on the first heavy frame: the same frame, with a
//! byte-identical encoding, took 10 ms right after a heavy one and 47-122 ms
//! after 4 s of idling (Bimari5 frame 1317, RTX 5060 Ti,
//! `frame_cost b` with `FRAME_COST_B_SLEEP_MS=4000`). Nothing in the picture
//! differs; only the clocks do.
//!
//! [`GpuWarmer`] is a thread that, **only for a moment after [`GpuWarmer::touch`]**,
//! repeatedly submits a short buffer-to-buffer copy on the same queue (one batch
//! at a time, never blocking on the device), so the driver sees steady load and
//! keeps its clocks up (16 MiB copies with longer gaps were measured not to be
//! enough). It draws nothing and touches no document buffer. Its two scratch buffers are allocated on first use and
//! freed when the touches stop. `BUZZ_NO_GPU_WARM=1` turns it off.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use wgpu::{Device, Queue};

/// Bytes each scratch buffer holds.
const WARM_BYTES: u64 = 64 << 20;
/// Copies per submission (each moves [`WARM_BYTES`] one way).
const COPIES_PER_SUBMIT: usize = 2;
/// Idle gap after a submission, and between looks for its completion.
const GAP: Duration = Duration::from_millis(2);
const POLL_GAP: Duration = Duration::from_micros(500);
/// How long a [`GpuWarmer::touch`] keeps the warmer going.
pub const KEEP_WARM_FOR: Duration = Duration::from_secs(1);

/// Is the warmer allowed? (`BUZZ_NO_GPU_WARM=1` says no.)
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("BUZZ_NO_GPU_WARM").map_or(true, |v| v.is_empty() || v == "0"))
}

struct Shared {
    base: Instant,
    /// Nanoseconds after `base` until which the warmer keeps going (`0` = not
    /// started).
    until_ns: AtomicU64,
    quit: AtomicBool,
    wake: (Mutex<()>, Condvar),
    submits: AtomicU64,
}

impl Shared {
    fn now_ns(&self) -> u64 {
        self.base.elapsed().as_nanos() as u64
    }

    fn active(&self) -> bool {
        self.until_ns.load(Ordering::Relaxed) > self.now_ns()
    }
}

/// See the module doc.
pub struct GpuWarmer {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl GpuWarmer {
    /// A parked warmer on `device`/`queue`: a thread that does nothing until
    /// [`Self::touch`].
    pub fn new(device: &Device, queue: &Queue) -> Self {
        let shared = Arc::new(Shared {
            base: Instant::now(),
            until_ns: AtomicU64::new(0),
            quit: AtomicBool::new(false),
            wake: (Mutex::new(()), Condvar::new()),
            submits: AtomicU64::new(0),
        });
        let (device, queue, s) = (device.clone(), queue.clone(), Arc::clone(&shared));
        let thread = std::thread::Builder::new()
            .name("buzz-gpu-warm".into())
            .spawn(move || run(&device, &queue, &s))
            .ok();
        Self { shared, thread }
    }

    /// Keep warming for [`KEEP_WARM_FOR`] from now. Cheap: call it every frame.
    pub fn touch(&self) {
        let s = &self.shared;
        let was_active = s.active();
        s.until_ns
            .store(s.now_ns() + KEEP_WARM_FOR.as_nanos() as u64, Ordering::Relaxed);
        if !was_active {
            let _guard = s.wake.0.lock();
            s.wake.1.notify_all();
        }
    }

    /// Is the warmer submitting work right now?
    pub fn is_active(&self) -> bool {
        self.shared.active()
    }

    /// Submissions made so far (for tests and the log).
    pub fn submits(&self) -> u64 {
        self.shared.submits.load(Ordering::Relaxed)
    }
}

impl Drop for GpuWarmer {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        {
            let _guard = self.shared.wake.0.lock();
            self.shared.wake.1.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(device: &Device, queue: &Queue, s: &Shared) {
    let mut scratch: Option<(wgpu::Buffer, wgpu::Buffer)> = None;
    let in_flight = Arc::new(AtomicBool::new(false));
    while !s.quit.load(Ordering::Relaxed) {
        if !s.active() {
            // Nothing is playing: give the scratch memory back and sleep.
            scratch = None;
            if let Ok(guard) = s.wake.0.lock() {
                let _ = s.wake.1.wait_timeout(guard, Duration::from_millis(100));
            }
            continue;
        }
        // **Never wait on the device from here** (`poll(Wait)` holds wgpu's fence
        // lock while it waits, and the render thread's own submit needs it: the
        // render call grew by up to a whole GPU frame). One batch at a time: the
        // next is submitted once the last has finished, noticed by a non-blocking
        // poll.
        if in_flight.load(Ordering::Acquire) {
            let _ = device.poll(wgpu::PollType::Poll);
            std::thread::sleep(POLL_GAP);
            continue;
        }
        let (a, b) = scratch.get_or_insert_with(|| {
            let make = |label| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: WARM_BYTES,
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            (make("buzz-gpu-warm-a"), make("buzz-gpu-warm-b"))
        });
        let mut encoder = device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("buzz-gpu-warm") });
        for i in 0..COPIES_PER_SUBMIT {
            if i % 2 == 0 {
                encoder.copy_buffer_to_buffer(a, 0, b, 0, WARM_BYTES);
            } else {
                encoder.copy_buffer_to_buffer(b, 0, a, 0, WARM_BYTES);
            }
        }
        in_flight.store(true, Ordering::Release);
        queue.submit([encoder.finish()]);
        s.submits.fetch_add(1, Ordering::Relaxed);
        let done = Arc::clone(&in_flight);
        queue.on_submitted_work_done(move || done.store(false, Ordering::Release));
        let _ = device.poll(wgpu::PollType::Poll);
        std::thread::sleep(GAP);
    }
}
