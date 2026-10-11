//! Microphone capture for voice commands (Stage V1).
//!
//! The pieces are deliberately split so everything except the device itself is
//! pure and testable without a microphone:
//!
//! * [`StreamResampler`] — any device rate, any channel count → 16 kHz mono,
//!   chunk by chunk (area averaging, so it low-passes as it decimates).
//! * [`UtteranceCutter`] — an energy voice-activity detector that cuts the
//!   16 kHz stream into utterances: speech starts after a few loud frames,
//!   ends after about 1.2 s of quiet, and is cut at 15 s regardless.
//! * [`MicInput`] — owns the cpal stream on its own thread, feeds the two
//!   above, and hands finished utterances to whoever polls it. The device is
//!   open exactly as long as the `MicInput` value lives; dropping it closes
//!   the microphone before `drop` returns.
//!
//! Audio is never written to disk and never leaves this process.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// What Whisper (and the VAD) work at.
pub const MIC_RATE: u32 = 16_000;

// ------------------------------------------------------------- resampler

/// Streaming resampler to mono [`MIC_RATE`] by area averaging: each output
/// sample is the mean of the input span it covers (fractional edges
/// weighted), which is a box low-pass and exact for integer ratios.
#[derive(Debug, Clone)]
pub struct StreamResampler {
    channels: usize,
    /// Input samples (frames) per output sample.
    step: f64,
    sum: f64,
    remaining: f64,
    /// A partial interleaved frame carried between calls.
    pending: Vec<f32>,
}

impl StreamResampler {
    pub fn new(from_rate: u32, channels: u16) -> Self {
        let step = from_rate.max(1) as f64 / MIC_RATE as f64;
        Self { channels: channels.max(1) as usize, step, sum: 0.0, remaining: step, pending: Vec::new() }
    }

    /// Feeds interleaved samples; returns the 16 kHz mono samples completed.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(interleaved.len() / self.channels / self.step.ceil() as usize + 1);
        let mut data: Vec<f32>;
        let slice: &[f32] = if self.pending.is_empty() {
            interleaved
        } else {
            data = std::mem::take(&mut self.pending);
            data.extend_from_slice(interleaved);
            &data
        };
        let frames = slice.len() / self.channels;
        for f in 0..frames {
            let frame = &slice[f * self.channels..(f + 1) * self.channels];
            let x = frame.iter().sum::<f32>() as f64 / self.channels as f64;
            let mut w = 1.0f64;
            while w > 1e-12 {
                let take = w.min(self.remaining);
                self.sum += x * take;
                self.remaining -= take;
                w -= take;
                if self.remaining <= 1e-9 {
                    out.push((self.sum / self.step) as f32);
                    self.sum = 0.0;
                    self.remaining = self.step;
                }
            }
        }
        let used = frames * self.channels;
        if used < slice.len() {
            self.pending = slice[used..].to_vec();
        }
        out
    }
}

// ------------------------------------------------------------------- VAD

/// One cut-out piece of speech.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    /// 16 kHz mono, with a little lead-in and tail around the speech.
    pub samples: Vec<f32>,
    /// Voiced span in seconds since the cutter started (first loud frame to
    /// the end of the last loud frame).
    pub start_s: f64,
    pub end_s: f64,
}

impl Utterance {
    pub fn voiced_seconds(&self) -> f64 {
        self.end_s - self.start_s
    }
}

/// Tunables for [`UtteranceCutter`].
#[derive(Debug, Clone, Copy)]
pub struct VadOptions {
    /// RMS floor below which a frame is never speech.
    pub min_rms: f32,
    /// Speech must exceed the tracked noise floor by this factor.
    pub floor_factor: f32,
    /// Silence that ends an utterance.
    pub hang_s: f64,
    /// Longest utterance; cut here even mid-speech.
    pub max_s: f64,
    /// Shortest voiced span worth transcribing.
    pub min_voiced_s: f64,
    /// Audio kept before the first loud frame and after the last.
    pub pad_s: f64,
}

impl Default for VadOptions {
    fn default() -> Self {
        Self { min_rms: 0.012, floor_factor: 3.0, hang_s: 1.2, max_s: 15.0, min_voiced_s: 0.4, pad_s: 0.25 }
    }
}

/// Push-to-talk stops after this much continuous silence.
pub const PUSH_SILENCE_S: f64 = 3.0;

const FRAME: usize = (MIC_RATE as usize) / 50; // 20 ms

/// Energy VAD that cuts a 16 kHz mono stream into [`Utterance`]s.
#[derive(Debug, Clone)]
pub struct UtteranceCutter {
    opt: VadOptions,
    /// Samples not yet forming a whole frame.
    carry: Vec<f32>,
    /// Frames consumed so far (absolute index of the next frame).
    frame_no: u64,
    noise: f32,
    /// Consecutive loud frames while idle (speech needs 3).
    run: u32,
    /// Recent frames kept as lead-in while idle.
    lead: std::collections::VecDeque<Vec<f32>>,
    active: bool,
    buf: Vec<f32>,
    first_loud: u64,
    last_loud: u64,
    quiet_run: u64,
    /// Number of loud frames in the current utterance.
    loud_frames: u64,
}

impl UtteranceCutter {
    pub fn new(opt: VadOptions) -> Self {
        Self {
            opt,
            carry: Vec::new(),
            frame_no: 0,
            noise: 0.003,
            run: 0,
            lead: Default::default(),
            active: false,
            buf: Vec::new(),
            first_loud: 0,
            last_loud: 0,
            quiet_run: 0,
            loud_frames: 0,
        }
    }

    /// Whether an utterance is being collected right now.
    pub fn in_speech(&self) -> bool {
        self.active
    }

    fn pad_frames(&self) -> usize {
        (self.opt.pad_s * 50.0).ceil() as usize
    }

    /// Feeds 16 kHz mono samples; returns every utterance that completed.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Utterance> {
        let mut done = Vec::new();
        self.carry.extend_from_slice(samples);
        let n = self.carry.len() / FRAME;
        let data = std::mem::take(&mut self.carry);
        for f in 0..n {
            let frame = &data[f * FRAME..(f + 1) * FRAME];
            if let Some(u) = self.frame(frame) {
                done.push(u);
            }
        }
        self.carry = data[n * FRAME..].to_vec();
        done
    }

    /// Ends any utterance in progress (for the end of a file or on stop).
    pub fn flush(&mut self) -> Option<Utterance> {
        if self.active {
            self.finish()
        } else {
            None
        }
    }

    fn finish(&mut self) -> Option<Utterance> {
        self.active = false;
        self.run = 0;
        let voiced = (self.last_loud + 1 - self.first_loud) as f64 / 50.0;
        let loud = self.loud_frames;
        let mut samples = std::mem::take(&mut self.buf);
        self.quiet_run = 0;
        self.loud_frames = 0;
        if voiced < self.opt.min_voiced_s || loud < 3 {
            return None;
        }
        // Trim the silence tail down to `pad_s` past the last loud frame.
        let first_frame_in_buf = self.first_loud.saturating_sub(self.pad_frames() as u64);
        let keep_to = (self.last_loud + 1 + self.pad_frames() as u64 - first_frame_in_buf) as usize * FRAME;
        samples.truncate(keep_to);
        Some(Utterance {
            samples,
            start_s: self.first_loud as f64 / 50.0,
            end_s: (self.last_loud + 1) as f64 / 50.0,
        })
    }

    fn frame(&mut self, frame: &[f32]) -> Option<Utterance> {
        let idx = self.frame_no;
        self.frame_no += 1;
        let rms = (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt();
        let thresh = self.opt.min_rms.max(self.noise * self.opt.floor_factor);
        let loud = rms > thresh;
        if !self.active {
            if loud {
                self.run += 1;
            } else {
                self.run = 0;
                // Track the noise floor only while quiet.
                self.noise = (self.noise * 0.98 + rms * 0.02).clamp(0.0005, 0.05);
            }
            self.lead.push_back(frame.to_vec());
            let keep = self.pad_frames() + 3;
            while self.lead.len() > keep {
                self.lead.pop_front();
            }
            if self.run >= 3 {
                // Speech began 3 frames ago.
                self.active = true;
                self.first_loud = idx + 1 - 3;
                self.last_loud = idx;
                self.loud_frames = 3;
                self.quiet_run = 0;
                self.buf.clear();
                for fr in self.lead.drain(..) {
                    self.buf.extend_from_slice(&fr);
                }
                // Drop lead-in older than pad_s before first_loud.
                let lead_frames = (self.buf.len() / FRAME) as u64;
                let want = (self.first_loud - (idx + 1 - lead_frames)).min(self.pad_frames() as u64);
                let _ = want;
                let start_frame = idx + 1 - lead_frames;
                let desired_start = self.first_loud.saturating_sub(self.pad_frames() as u64);
                if desired_start > start_frame {
                    let cut = (desired_start - start_frame) as usize * FRAME;
                    self.buf.drain(..cut);
                }
            }
            return None;
        }
        // Active.
        self.buf.extend_from_slice(frame);
        if loud {
            self.last_loud = idx;
            self.loud_frames += 1;
            self.quiet_run = 0;
        } else {
            self.quiet_run += 1;
        }
        let hang = (self.opt.hang_s * 50.0).round() as u64;
        let max = (self.opt.max_s * 50.0).round() as u64;
        if self.quiet_run >= hang || idx + 1 - self.first_loud >= max {
            return self.finish();
        }
        None
    }
}

// ------------------------------------------------------------ the device

/// What a [`MicInput`] reports.
#[derive(Debug, Clone)]
pub enum MicEvent {
    /// The device opened; carries its name.
    Opened(String),
    /// A finished utterance (16 kHz mono).
    Utterance(Utterance),
    /// Push-to-talk recording is over (silence timeout or finish request);
    /// the microphone is already closed. Any speech came first as `Utterance`.
    Ended,
    /// The device could not be opened or died; the input has stopped.
    Failed(String),
}

/// Names of the input devices, for the picker.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// An open microphone. Dropping it stops the stream and joins the thread, so
/// the device is released before `drop` returns.
pub struct MicInput {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    finish: Arc<AtomicBool>,
    rx: Receiver<MicEvent>,
    join: Option<JoinHandle<()>>,
}

impl MicInput {
    /// Starts capturing from the named device (default input when `None` or
    /// not found). Opening happens on the capture thread: poll [`Self::poll`]
    /// for `Opened` or `Failed`.
    pub fn start(device: Option<String>) -> Self {
        Self::spawn(device, false)
    }

    /// Push-to-talk: records one utterance (ends after 3 s of silence, 28 s
    /// at most, or on [`Self::finish`]), then emits `Ended` and closes itself.
    pub fn start_push(device: Option<String>) -> Self {
        Self::spawn(device, true)
    }

    /// Asks a push recording to stop now: what was said is flushed as an
    /// utterance, then `Ended`.
    pub fn finish(&self) {
        self.finish.store(true, Ordering::SeqCst);
    }

    fn spawn(device: Option<String>, push: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let (tx, rx) = channel();
        let finish = Arc::new(AtomicBool::new(false));
        let (s2, l2, f2) = (stop.clone(), level.clone(), finish.clone());
        let join = std::thread::Builder::new()
            .name("buzz-mic".into())
            .spawn(move || capture_thread(device, push, s2, l2, f2, tx))
            .ok();
        Self { stop, level, finish, rx, join }
    }

    /// Everything that has happened since the last call.
    pub fn poll(&self) -> Vec<MicEvent> {
        self.rx.try_iter().collect()
    }

    /// Recent input level, `0.0..=1.0` (peak-held RMS of the latest chunk).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for MicInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn capture_thread(
    device: Option<String>,
    push: bool,
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    finish: Arc<AtomicBool>,
    tx: Sender<MicEvent>,
) {
    let host = cpal::default_host();
    let dev = device
        .as_deref()
        .and_then(|name| host.input_devices().ok()?.find(|d| d.name().map(|n| n == name).unwrap_or(false)))
        .or_else(|| host.default_input_device());
    let Some(dev) = dev else {
        let _ = tx.send(MicEvent::Failed("No microphone found".into()));
        return;
    };
    let name = dev.name().unwrap_or_else(|_| "microphone".into());
    let cfg = match dev.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(MicEvent::Failed(format!("Microphone '{name}': {e}")));
            return;
        }
    };
    let (rate, channels, format) = (cfg.sample_rate().0, cfg.channels(), cfg.sample_format());
    let (chunk_tx, chunk_rx) = channel::<Vec<f32>>();
    let err_tx = tx.clone();
    let err_fn = move |e: cpal::StreamError| {
        let _ = err_tx.send(MicEvent::Failed(format!("Microphone error: {e}")));
    };
    let stream_cfg: cpal::StreamConfig = cfg.into();
    let built = match format {
        cpal::SampleFormat::F32 => {
            let t = chunk_tx.clone();
            dev.build_input_stream(&stream_cfg, move |d: &[f32], _| { let _ = t.send(d.to_vec()); }, err_fn, None)
        }
        cpal::SampleFormat::I16 => {
            let t = chunk_tx.clone();
            dev.build_input_stream(
                &stream_cfg,
                move |d: &[i16], _| { let _ = t.send(d.iter().map(|&s| s as f32 / 32768.0).collect()); },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let t = chunk_tx.clone();
            dev.build_input_stream(
                &stream_cfg,
                move |d: &[u16], _| { let _ = t.send(d.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect()); },
                err_fn,
                None,
            )
        }
        other => {
            let _ = tx.send(MicEvent::Failed(format!("Microphone '{name}': unsupported sample format {other:?}")));
            return;
        }
    };
    let stream = match built {
        Ok(s) => match s.play() {
            Ok(()) => s,
            Err(e) => {
                let _ = tx.send(MicEvent::Failed(format!("Microphone '{name}': {e}")));
                return;
            }
        },
        Err(e) => {
            let _ = tx.send(MicEvent::Failed(format!("Microphone '{name}': {e}")));
            return;
        }
    };
    drop(chunk_tx);
    let _ = tx.send(MicEvent::Opened(name));
    let mut resampler = StreamResampler::new(rate, channels);
    let opt = if push {
        VadOptions { hang_s: PUSH_SILENCE_S, max_s: 28.0, ..VadOptions::default() }
    } else {
        VadOptions::default()
    };
    let mut cutter = UtteranceCutter::new(opt);
    let mut heard = 0usize;
    let mut ended = false;
    while !stop.load(Ordering::SeqCst) {
        if push && finish.load(Ordering::SeqCst) {
            if let Some(u) = cutter.flush() {
                let _ = tx.send(MicEvent::Utterance(u));
            }
            ended = true;
            break;
        }
        match chunk_rx.recv_timeout(Duration::from_millis(40)) {
            Ok(chunk) => {
                let mono = resampler.push(&chunk);
                if !mono.is_empty() {
                    let rms = (mono.iter().map(|x| x * x).sum::<f32>() / mono.len() as f32).sqrt();
                    // Scale so ordinary speech reads around half-scale.
                    let v = (rms * 6.0).min(1.0);
                    level.store(v.to_bits(), Ordering::Relaxed);
                }
                heard += mono.len();
                for u in cutter.push(&mono) {
                    let _ = tx.send(MicEvent::Utterance(u));
                    if push {
                        ended = true;
                    }
                }
                // Nothing said for the whole window: give up quietly.
                if push && !cutter.in_speech() && heard as f64 >= PUSH_SILENCE_S * MIC_RATE as f64 && !ended {
                    ended = true;
                }
                if ended {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    // Release the microphone first, then report the end.
    drop(stream);
    level.store(0, Ordering::Relaxed);
    if ended {
        let _ = tx.send(MicEvent::Ended);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f64, amp: f32, hz: f32) -> Vec<f32> {
        (0..(seconds * MIC_RATE as f64) as usize)
            .map(|i| (i as f32 / MIC_RATE as f32 * hz * std::f32::consts::TAU).sin() * amp)
            .collect()
    }

    fn silence(seconds: f64) -> Vec<f32> {
        vec![0.0; (seconds * MIC_RATE as f64) as usize]
    }

    /// Deterministic pseudo-noise.
    fn noise(seconds: f64, amp: f32) -> Vec<f32> {
        let mut s = 12345u32;
        (0..(seconds * MIC_RATE as f64) as usize)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                ((s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn run(signal: &[f32]) -> Vec<Utterance> {
        let mut c = UtteranceCutter::new(VadOptions::default());
        let mut out = Vec::new();
        // Feed in odd-sized chunks like a device would.
        for ch in signal.chunks(1000) {
            out.extend(c.push(ch));
        }
        out.extend(c.flush());
        out
    }

    #[test]
    fn silence_and_quiet_noise_give_no_utterances() {
        assert!(run(&silence(5.0)).is_empty());
        assert!(run(&noise(5.0, 0.004)).is_empty());
    }

    #[test]
    fn one_tone_burst_is_cut_with_correct_bounds() {
        let mut s = noise(1.0, 0.003);
        s.extend(tone(1.5, 0.3, 300.0));
        s.extend(noise(2.5, 0.003));
        let u = run(&s);
        assert_eq!(u.len(), 1, "{u:?}");
        assert!((u[0].start_s - 1.0).abs() < 0.1, "{}", u[0].start_s);
        assert!((u[0].end_s - 2.5).abs() < 0.1, "{}", u[0].end_s);
        // Lead-in and tail are small: not the whole 5 s.
        let secs = u[0].samples.len() as f64 / MIC_RATE as f64;
        assert!((1.5..2.2).contains(&secs), "{secs}");
    }

    #[test]
    fn two_bursts_split_by_a_long_pause_and_joined_by_a_short_one() {
        let mut s = silence(0.5);
        s.extend(tone(0.8, 0.3, 250.0));
        s.extend(silence(0.6)); // shorter than 1.2 s: same utterance
        s.extend(tone(0.8, 0.3, 250.0));
        s.extend(silence(2.0)); // ends it
        s.extend(tone(1.0, 0.3, 400.0));
        s.extend(silence(2.0));
        let u = run(&s);
        assert_eq!(u.len(), 2, "{u:?}");
        assert!((u[0].start_s - 0.5).abs() < 0.1 && (u[0].end_s - 2.7).abs() < 0.1, "{:?}", (u[0].start_s, u[0].end_s));
        assert!((u[1].start_s - 4.7).abs() < 0.1, "{}", u[1].start_s);
    }

    #[test]
    fn blips_shorter_than_the_minimum_are_dropped_and_long_speech_is_capped() {
        let mut s = silence(0.5);
        s.extend(tone(0.2, 0.3, 300.0)); // 0.2 s blip
        s.extend(silence(2.0));
        assert!(run(&s).is_empty());
        let mut long = silence(0.2);
        long.extend(tone(20.0, 0.3, 300.0));
        long.extend(silence(2.0));
        let u = run(&long);
        assert!(u.len() >= 2, "{}", u.len());
        assert!(u[0].voiced_seconds() <= 15.1, "{}", u[0].voiced_seconds());
    }

    #[test]
    fn resampler_48k_stereo_to_16k_mono_keeps_length_and_tone() {
        let rate = 48_000u32;
        let n = rate as usize; // 1 s
        let mut inter = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            inter.push(v);
            inter.push(v);
        }
        let mut r = StreamResampler::new(rate, 2);
        // Odd chunk sizes, including a split frame.
        let mut out = Vec::new();
        for ch in inter.chunks(1001) {
            out.extend(r.push(ch));
        }
        assert!((out.len() as i64 - 16_000).abs() <= 2, "{}", out.len());
        let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.02, "{rms}");
    }

    #[test]
    fn resampler_44k_mono_rate_is_exact_on_average() {
        let mut r = StreamResampler::new(44_100, 1);
        let out = r.push(&vec![0.25; 44_100]);
        assert!((out.len() as i64 - 16_000).abs() <= 1, "{}", out.len());
        assert!(out.iter().all(|&x| (x - 0.25).abs() < 1e-4));
        // Passing 16 kHz through unchanged.
        let mut r = StreamResampler::new(16_000, 1);
        assert_eq!(r.push(&[0.1, 0.2, 0.3]), vec![0.1, 0.2, 0.3]);
    }
}
