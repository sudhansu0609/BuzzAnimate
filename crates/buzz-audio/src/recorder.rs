//! **Recording a take** — a line of dialogue, at the microphone's own rate.
//!
//! [`crate::mic`] is for voice *commands*: 16 kHz, cut into utterances by a
//! voice-activity detector, never kept. A performance is the opposite on every
//! count. It is kept, it is heard, it is re-voiced into a character — and a
//! voice made from a 16 kHz take has nothing above 8 kHz to make an `s` or a
//! breath out of. So a take records at whatever the device runs at, in mono,
//! from Record until Stop, with nothing cut: the pause before a line is part of
//! how it is said. Only the dead air at either end is trimmed afterwards
//! ([`trim_silence`]).
//!
//! Audio never leaves this process; what happens to the take is the caller's.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::Clip;

/// The longest take: ten minutes. A whole scene in one go is fine; a
/// microphone left recording all afternoon is not.
const MAX_SECONDS: f64 = 600.0;

/// A take being recorded. Dropping it stops the microphone.
pub struct TakeRecorder {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    shared: Arc<Mutex<Shared>>,
    join: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    rate: u32,
    samples: Vec<f32>,
    device: Option<String>,
    error: Option<String>,
}

impl TakeRecorder {
    /// Start recording from the named input (the default when `None` or not
    /// found). The device opens on the recording thread: [`Self::device`]
    /// names it once it has, [`Self::error`] says why it did not.
    pub fn start(device: Option<String>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (s2, l2, sh2) = (stop.clone(), level.clone(), shared.clone());
        let join = std::thread::Builder::new()
            .name("buzz-take".into())
            .spawn(move || record(device, s2, l2, sh2))
            .ok();
        Self { stop, level, shared, join }
    }

    /// The input's name, once it is open.
    pub fn device(&self) -> Option<String> {
        self.shared.lock().ok()?.device.clone()
    }

    /// Why recording stopped or never started.
    pub fn error(&self) -> Option<String> {
        self.shared.lock().ok()?.error.clone()
    }

    /// Seconds recorded so far.
    pub fn seconds(&self) -> f64 {
        self.shared
            .lock()
            .map(|s| if s.rate > 0 { s.samples.len() as f64 / s.rate as f64 } else { 0.0 })
            .unwrap_or(0.0)
    }

    /// The input level just now, 0 … 1.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Stop, close the microphone, and hand back the take — trimmed of the
    /// silence at either end, never inside. `None` when nothing was heard.
    pub fn stop(mut self, name: &str) -> Option<Clip> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        let shared = std::mem::take(&mut *self.shared.lock().ok()?);
        if shared.rate == 0 || shared.samples.is_empty() {
            return None;
        }
        let trimmed = trim_silence(&shared.samples, shared.rate);
        if trimmed.is_empty() {
            return None;
        }
        Clip::new(name, shared.rate, 1, trimmed).ok()
    }
}

impl Drop for TakeRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// The take without the dead air at either end, keeping a fifth of a second
/// of room either side so the first consonant is not clipped and the line does
/// not end on a cut. Everything between the first and last sound is kept as
/// performed. Silence is judged against the loudest moment, so a quiet room
/// and a loud one trim the same.
pub fn trim_silence(samples: &[f32], rate: u32) -> Vec<f32> {
    let win = (rate as usize / 100).max(1);
    let rms: Vec<f32> = samples
        .chunks(win)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let loudest = rms.iter().copied().fold(0.0f32, f32::max);
    if loudest < 1e-4 {
        return Vec::new();
    }
    let floor = (loudest * 0.05).max(0.002);
    let (Some(first), Some(last)) = (rms.iter().position(|r| *r > floor), rms.iter().rposition(|r| *r > floor)) else {
        return Vec::new();
    };
    let pad = 20; // windows of 10 ms
    let a = first.saturating_sub(pad) * win;
    let b = ((last + 1 + pad) * win).min(samples.len());
    samples[a..b].to_vec()
}

fn record(device: Option<String>, stop: Arc<AtomicBool>, level: Arc<AtomicU32>, shared: Arc<Mutex<Shared>>) {
    let fail = |msg: String| {
        if let Ok(mut s) = shared.lock() {
            s.error = Some(msg);
        }
    };
    let host = cpal::default_host();
    let dev = device
        .as_deref()
        .and_then(|name| host.input_devices().ok()?.find(|d| d.name().map(|n| n == name).unwrap_or(false)))
        .or_else(|| host.default_input_device());
    let Some(dev) = dev else {
        return fail("No microphone found".into());
    };
    let name = dev.name().unwrap_or_else(|_| "microphone".into());
    let cfg = match dev.default_input_config() {
        Ok(c) => c,
        Err(e) => return fail(format!("Microphone '{name}': {e}")),
    };
    let (rate, channels, format) = (cfg.sample_rate().0, cfg.channels().max(1) as usize, cfg.sample_format());
    let stream_cfg: cpal::StreamConfig = cfg.into();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let err_shared = shared.clone();
    let err_fn = move |e: cpal::StreamError| {
        if let Ok(mut s) = err_shared.lock() {
            s.error = Some(format!("Microphone error: {e}"));
        }
    };
    let built = match format {
        cpal::SampleFormat::F32 => {
            let t = tx.clone();
            dev.build_input_stream(&stream_cfg, move |d: &[f32], _| { let _ = t.send(d.to_vec()); }, err_fn, None)
        }
        cpal::SampleFormat::I16 => {
            let t = tx.clone();
            dev.build_input_stream(
                &stream_cfg,
                move |d: &[i16], _| { let _ = t.send(d.iter().map(|&s| s as f32 / 32768.0).collect()); },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let t = tx.clone();
            dev.build_input_stream(
                &stream_cfg,
                move |d: &[u16], _| { let _ = t.send(d.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect()); },
                err_fn,
                None,
            )
        }
        other => return fail(format!("Microphone '{name}': unsupported sample format {other:?}")),
    };
    let stream = match built.and_then(|s| s.play().map(|()| s).map_err(|e| cpal::BuildStreamError::BackendSpecific {
        err: cpal::BackendSpecificError { description: e.to_string() },
    })) {
        Ok(s) => s,
        Err(e) => return fail(format!("Microphone '{name}': {e}")),
    };
    drop(tx);
    if let Ok(mut s) = shared.lock() {
        s.rate = rate;
        s.device = Some(name);
    }
    let max = (MAX_SECONDS * rate as f64) as usize;
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                let mono: Vec<f32> = chunk
                    .chunks(channels)
                    .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
                    .collect();
                let rms = (mono.iter().map(|v| v * v).sum::<f32>() / mono.len().max(1) as f32).sqrt();
                level.store((rms * 4.0).min(1.0).to_bits(), Ordering::Relaxed);
                if let Ok(mut s) = shared.lock() {
                    if s.samples.len() < max {
                        s.samples.extend_from_slice(&mono);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(stream);
    level.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_air_is_trimmed_and_the_pause_inside_is_kept() {
        let rate = 16_000;
        let tone = |n: usize| (0..n).map(|i| (i as f32 * 0.2).sin() * 0.3).collect::<Vec<f32>>();
        let mut take = vec![0.0; rate as usize]; // 1 s of nothing
        take.extend(tone(rate as usize / 2));
        take.extend(vec![0.0; rate as usize / 2]); // a pause inside the line
        take.extend(tone(rate as usize / 2));
        take.extend(vec![0.0; rate as usize * 2]);
        let trimmed = trim_silence(&take, rate);
        let secs = trimmed.len() as f32 / rate as f32;
        assert!((secs - (1.5 + 0.4)).abs() < 0.03, "1.5 s of line + 0.2 s room either side, got {secs}");
    }

    #[test]
    fn silence_is_no_take() {
        assert!(trim_silence(&vec![0.0; 8000], 16_000).is_empty());
    }
}
