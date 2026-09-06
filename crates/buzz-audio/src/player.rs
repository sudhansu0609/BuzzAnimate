//! Making a clip audible, in step with the playhead.
//!
//! # The shape of the problem
//!
//! The audio device pulls samples on its own thread, on its own clock,
//! whenever it feels like it. The editor pushes the playhead forward on the
//! UI thread, on the display's clock. Neither can wait for the other: blocking
//! the audio callback produces a click, and blocking the UI on audio produces
//! a stutter.
//!
//! So they share exactly one thing — a mixer behind a lock — and the *audio
//! clock is authoritative* while playing. The editor asks where the sound has
//! got to and moves the playhead there, rather than telling the sound where
//! the playhead is. Doing it the other way round means every dropped frame
//! nudges the audio, and dialogue that drifts against the picture is the one
//! defect an audience always notices.
//!
//! # Why the device is opened lazily and kept
//!
//! Opening an output stream takes tens of milliseconds and makes noise in the
//! system mixer. A document with no sound in it should never touch the audio
//! device at all, and one that does should open it once.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::Clip;

/// What the player is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    /// No device, or nothing loaded.
    Idle,
    Playing,
    Paused,
}

/// How a cue relates to the playhead.
///
/// The audio side's own copy of Animate's sync modes, deliberately: the mixer
/// must not have to understand a document to fill a buffer, and `Stop` never
/// reaches here — a stopped sound is one the document does not send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CueSync {
    /// **Tied to the timeline.** The frame decides the position, so scrubbing
    /// moves the sound and playback cannot drift from the picture. Dialogue.
    #[default]
    Stream,
    /// **Triggered, then independent.** Crossing its frame starts it, and from
    /// then on it runs on its own clock to its own end — it does not stop when
    /// the playhead does, and it does not move when the playhead is scrubbed.
    /// A door slam is not a position in the film; it is an event.
    Event,
    /// Like [`Self::Event`], except that triggering it again while it is
    /// already sounding does nothing, rather than starting a second copy.
    Start,
}

/// One sound queued for playback, positioned on the timeline.
#[derive(Debug, Clone)]
pub struct Cue {
    pub clip: Arc<Clip>,
    /// The animation frame this sound starts on.
    pub start_frame: u32,
    pub volume: f32,
    pub sync: CueSync,
}

/// An event sound that has been triggered and is running on its own clock.
#[derive(Debug, Clone)]
struct Voice {
    clip: Arc<Clip>,
    volume: f32,
    /// The mixer clock reading at which this voice began.
    began: u64,
}

/// The state the audio callback and the editor share.
///
/// Deliberately small and plain: the callback holds the lock for as long as it
/// takes to copy samples, and anything expensive in here would be heard.
#[derive(Default)]
struct Mixer {
    cues: Vec<Cue>,
    /// Output sample rate, so timeline positions convert to samples.
    sample_rate: u32,
    channels: u16,
    fps: f64,
    /// Where playback has reached, in output sample frames since frame zero.
    position: u64,
    playing: bool,
    volume: f32,
    /// **Output gain, ramped rather than switched.**
    ///
    /// Every abrupt change to what is being mixed is a step in the waveform,
    /// and a step is a click. There are three of them — starting, stopping, and
    /// moving the playhead — and the third is the one that hurts: dragging the
    /// playhead repositions the audio many times a second, and a run of clicks
    /// at a steady rate is not a series of clicks, it is a **tone**. That is the
    /// hum heard under the audio while scrubbing.
    ///
    /// One envelope fixes all three. It travels towards [`Self::target`] at a
    /// fixed rate — see `RAMP_SECONDS` — so nothing the mixer does ever arrives
    /// as a step.
    gain: f32,
    /// What [`Self::gain`] is travelling towards: one while playing, zero while
    /// stopped or while a reposition is waiting to happen.
    target: f32,
    /// **A move the mixer has not made yet.**
    ///
    /// A seek does not take effect where it is asked for. It sets this, the
    /// gain falls to zero over a few milliseconds, and *then* the position
    /// jumps and fades back in — so the join is a short dip rather than a step.
    /// Silent for the ear, and about ten milliseconds, which is a third of a
    /// frame at 24fps.
    pending_seek: Option<u64>,
    /// **How much of a scrub window is left to play**, in output sample frames.
    ///
    /// `None` is ordinary playback, which runs until it is stopped. `Some` is a
    /// scrub, and it is bounded — which is the whole difference between the
    /// two.
    ///
    /// # Why a scrub is bounded and playback is not
    ///
    /// A scrub used to reposition the audio and then let it *run*. Drag the
    /// playhead slower than real time — which is what dragging is — and the
    /// audio races ahead of the pointer, gets yanked back to the frame on the
    /// next move, and races ahead again: the same fragment over and over.
    /// Dragging back and forth over a few frames made it worse, because every
    /// pass replayed the same overlapping pieces.
    ///
    /// A jog wheel does not do that. Each frame the playhead crosses is played
    /// **once**, for as long as that frame lasts, and then it stops until the
    /// playhead moves again. Drag at real speed and the windows meet and it
    /// sounds continuous; drag slowly and you hear each frame once with a gap
    /// after it, which is what "finding the beat by ear" actually is.
    scrub_left: Option<u64>,
    /// How long one scrub window is, in output sample frames.
    scrub_window: u64,
    /// A clock that only ever goes forwards, in output sample frames.
    ///
    /// **Distinct from `position`, and that is the whole of honest Event
    /// sync.** `position` is where the *playhead* is: it jumps when you scrub
    /// and stands still when you stop. A triggered sound effect does neither,
    /// so it is timed against this instead.
    clock: u64,
    /// Event and Start sounds currently running on their own clock.
    voices: Vec<Voice>,
}

/// One channel of a clip, sampled at a fractional position.
///
/// **Linear interpolation, not nearest-neighbour.** Nearest holds each source
/// sample for a whole run of output samples and then jumps, which is a
/// staircase — and a staircase is broadband noise, heard as the aliasing and
/// the not-quite-right pitch §7 item 40 recorded. Interpolating between the two
/// neighbouring samples costs one multiply and removes most of it.
///
/// The position is a `f64` computed from the absolute output position by the
/// caller, never accumulated, so this cannot drift however long it runs.
fn sample_at(clip: &Clip, position: f64, channel: usize, clip_channels: usize) -> f32 {
    let frames = clip.len();
    if frames == 0 || position < 0.0 {
        return 0.0;
    }
    let i = position.floor();
    let t = (position - i) as f32;
    let i = i as usize;
    if i + 1 >= frames {
        // The last sample has no neighbour to interpolate towards; holding it
        // is correct and is one sample long.
        return if i < frames {
            clip.samples[i * clip_channels + channel]
        } else {
            0.0
        };
    }
    let a = clip.samples[i * clip_channels + channel];
    let b = clip.samples[(i + 1) * clip_channels + channel];
    a + (b - a) * t
}

/// How long the output takes to travel between silence and full.
///
/// Long enough to be inaudible as a click, short enough to be inaudible as a
/// fade: five milliseconds is about a tenth of a frame at 24fps, and it is the
/// figure every editor's declicker lands on.
const RAMP_SECONDS: f32 = 0.005;

impl Mixer {
    /// How far the gain moves per output sample frame.
    fn ramp_step(&self) -> f32 {
        let samples = self.sample_rate.max(1) as f32 * RAMP_SECONDS;
        if samples <= 1.0 { 1.0 } else { 1.0 / samples }
    }

    /// Fill `output` with whatever the cues have at the current position.
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let channels = self.channels.max(1) as usize;
        let frames = output.len() / channels;
        if frames == 0 {
            return;
        }

        // Where the envelope is heading. A waiting reposition takes priority
        // over playing: the gain has to reach zero before the jump can happen.
        // A spent scrub window is silent for the same reason it is bounded.
        let spent = self.scrub_left == Some(0);
        self.target = if self.pending_seek.is_some() || spent {
            0.0
        } else if self.playing {
            1.0
        } else {
            0.0
        };

        // **Event voices are rendered even when the playhead is stopped.**
        // That is what makes them events rather than positions: pressing stop
        // ends the film, not the door slam that was already sounding.
        //
        // The gain has to have arrived, too: cutting the tail of a fade-out to
        // take the cheap path would put back the click the fade is there to
        // remove.
        if (!self.playing || spent) && self.voices.is_empty() && self.gain <= 0.0 {
            self.gain = 0.0;
            self.clock += frames as u64;
            if let Some(to) = self.pending_seek.take() {
                self.position = to;
                // A fresh window at the new frame: the move *is* the scrub.
                if self.scrub_left.is_some() {
                    self.scrub_left = Some(self.scrub_window);
                }
            }
            return;
        }

        if self.playing {
            self.trigger_events(frames);
        }

        for cue in &self.cues {
            if cue.sync != CueSync::Stream || !self.playing {
                continue;
            }
            let clip = &cue.clip;
            // Where this cue begins, in *output* sample frames.
            let start = if self.fps > 0.0 {
                (cue.start_frame as f64 / self.fps * self.sample_rate as f64) as u64
            } else {
                0
            };
            let ratio = clip.sample_rate as f64 / self.sample_rate as f64;
            let clip_channels = clip.channels.max(1) as usize;
            let gain = cue.volume * self.volume;

            for i in 0..frames {
                let at = self.position + i as u64;
                if at < start {
                    continue;
                }
                // Computed from the absolute position every time, so a stream
                // cannot drift from the picture however long it plays.
                let source = ((at - start) as f64) * ratio;
                if source >= clip.len() as f64 {
                    continue;
                }
                for c in 0..channels {
                    let sample =
                        sample_at(clip, source, c.min(clip_channels - 1), clip_channels) * gain;
                    output[i * channels + c] += sample;
                }
            }
        }

        // The triggered sounds, each on its own clock.
        let (clock, sample_rate, master) = (self.clock, self.sample_rate, self.volume);
        self.voices.retain(|voice| {
            let clip = &voice.clip;
            let ratio = clip.sample_rate as f64 / sample_rate.max(1) as f64;
            let clip_channels = clip.channels.max(1) as usize;
            let gain = voice.volume * master;
            let mut alive = false;

            for i in 0..frames {
                let at = clock + i as u64;
                let source = (at.saturating_sub(voice.began) as f64) * ratio;
                if source >= clip.len() as f64 {
                    break;
                }
                alive = true;
                for c in 0..channels {
                    let sample =
                        sample_at(clip, source, c.min(clip_channels - 1), clip_channels) * gain;
                    output[i * channels + c] += sample;
                }
            }
            alive
        });

        // **The envelope, and the clamp, in one pass over the buffer.**
        //
        // The gain is per sample *frame* rather than per sample, so the
        // channels of a stereo pair are scaled together and the image does not
        // wander during a ramp.
        let step = self.ramp_step();
        for i in 0..frames {
            if self.gain < self.target {
                self.gain = (self.gain + step).min(self.target);
            } else if self.gain > self.target {
                self.gain = (self.gain - step).max(self.target);
            }
            for c in 0..channels {
                let at = i * channels + c;
                // Summed without clipping to a hard edge: two loud cues
                // together would otherwise square off into audible distortion.
                output[at] = (output[at] * self.gain).clamp(-1.0, 1.0);
            }
        }

        if self.playing {
            self.position += frames as u64;
        }
        self.clock += frames as u64;
        // A scrub window is spent as it plays, and stops when it runs out.
        if let Some(left) = &mut self.scrub_left {
            *left = left.saturating_sub(frames as u64);
        }

        // The join, once the gain is out of the way.
        if self.gain <= 0.0
            && let Some(to) = self.pending_seek.take()
        {
            self.position = to;
            if self.scrub_left.is_some() {
                self.scrub_left = Some(self.scrub_window);
            }
        }
    }

    /// Start any Event or Start cue the playhead crosses in this buffer.
    fn trigger_events(&mut self, frames: usize) {
        if self.fps <= 0.0 {
            return;
        }
        let (from, to) = (self.position, self.position + frames as u64);
        for cue in &self.cues {
            if cue.sync == CueSync::Stream {
                continue;
            }
            let start = (cue.start_frame as f64 / self.fps * self.sample_rate as f64) as u64;
            if start < from || start >= to {
                continue;
            }
            // Start's one difference from Event: it declines to stack a
            // second copy on one already sounding.
            //
            // **The test is the same *sound*, not the same cue.** Animate's
            // rule is about the clip: two Start cues of one footstep a frame
            // apart are exactly the case the mode exists to suppress, and
            // comparing cue indices — which differ — suppresses nothing. The
            // clips are shared out of the document's sound cache, so one
            // asset is one `Arc` and pointer equality is the identity test.
            if cue.sync == CueSync::Start
                && self.voices.iter().any(|v| Arc::ptr_eq(&v.clip, &cue.clip))
            {
                continue;
            }
            self.voices.push(Voice {
                clip: Arc::clone(&cue.clip),
                volume: cue.volume,
                // Timed from where in *this buffer* the trigger fell, so a
                // sound effect is not quantised to the buffer size.
                began: self.clock + (start - from),
            });
        }
    }
}

/// Plays a document's sound.
pub struct Player {
    mixer: Arc<Mutex<Mixer>>,
    /// Held so the stream stays alive; dropping it stops the audio.
    stream: Option<cpal::Stream>,
    state: PlayerState,
    /// Why there is no sound, if there is no sound.
    unavailable: Option<String>,
}

impl Player {
    /// Create a player without touching the audio device.
    ///
    /// The device is opened by the first [`Self::play`], so a document with no
    /// sound never opens one.
    pub fn new(fps: f64) -> Self {
        Self {
            mixer: Arc::new(Mutex::new(Mixer {
                fps,
                volume: 1.0,
                ..Mixer::default()
            })),
            stream: None,
            state: PlayerState::Idle,
            unavailable: None,
        }
    }

    pub fn state(&self) -> PlayerState {
        self.state
    }

    /// Why audio is unavailable, if it is — for the status bar.
    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    pub fn is_playing(&self) -> bool {
        self.state == PlayerState::Playing
    }

    /// Replace what is queued. Safe to call while playing.
    ///
    /// Sounds already triggered keep going — they carry their own clip, and
    /// cutting a sound effect off because the document was edited would be
    /// exactly the behaviour Event sync exists to avoid.
    pub fn set_cues(&mut self, cues: Vec<Cue>) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.cues = cues;
        }
    }

    /// Silence everything, including sounds already triggered.
    ///
    /// Distinct from [`Self::pause`], which only stops the playhead: a paused
    /// film still lets a door slam finish. This is for closing a document.
    pub fn silence(&mut self) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.playing = false;
            mixer.voices.clear();
        }
        if self.state == PlayerState::Playing {
            self.state = PlayerState::Paused;
        }
    }

    pub fn set_fps(&mut self, fps: f64) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.fps = fps;
        }
    }

    /// Master volume, `0.0..=1.0`.
    pub fn set_volume(&mut self, volume: f32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.volume = volume.clamp(0.0, 1.0);
        }
    }

    /// Is there anything to play at all?
    pub fn has_sound(&self) -> bool {
        self.mixer
            .lock()
            .map(|m| !m.cues.is_empty())
            .unwrap_or(false)
    }

    /// Start playing from `frame`.
    ///
    /// Opens the device if it is not open yet. A machine with no working audio
    /// output is not an error worth stopping the editor for — it is recorded
    /// and reported once, and everything else carries on silently.
    pub fn play(&mut self, frame: u32) -> Result<()> {
        if self.stream.is_none() {
            match self.open() {
                Ok(()) => {}
                Err(e) => {
                    let message = format!("{e:#}");
                    tracing::warn!("no audio output: {message}");
                    self.unavailable = Some(message);
                    return Ok(());
                }
            }
        }

        self.seek(frame);
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.playing = true;
            // Playback is unbounded; only a scrub is a window.
            mixer.scrub_left = None;
        }
        if let Some(stream) = &self.stream {
            stream.play().context("starting the audio stream")?;
        }
        self.state = PlayerState::Playing;
        Ok(())
    }

    /// Stop, leaving the position where it is.
    pub fn pause(&mut self) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.playing = false;
            mixer.scrub_left = None;
        }
        if self.state == PlayerState::Playing {
            self.state = PlayerState::Paused;
        }
    }

    /// **Move to a frame**, without a click.
    ///
    /// The jump does not happen here. It is handed to the mixer, which fades
    /// out, moves, and fades back in over about ten milliseconds — see
    /// [`Mixer::pending_seek`]. A seek that lands where the audio already is
    /// does nothing at all, so holding the playhead still while dragging
    /// something else does not stutter the sound.
    pub fn seek(&mut self, frame: u32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            let rate = mixer.sample_rate.max(1) as f64;
            let to = if mixer.fps > 0.0 {
                (frame as f64 / mixer.fps * rate) as u64
            } else {
                0
            };
            // Already there, or already on the way there.
            if mixer.pending_seek == Some(to)
                || (mixer.pending_seek.is_none() && mixer.position == to)
            {
                return;
            }
            // Nothing is sounding yet, so there is nothing to fade out of.
            if mixer.gain <= 0.0 && !mixer.playing {
                mixer.position = to;
                mixer.pending_seek = None;
                return;
            }
            mixer.pending_seek = Some(to);
        }
    }

    /// **Play one frame's worth of sound, from `frame`, and stop.**
    ///
    /// A jog wheel rather than playback: see [`Mixer::scrub_left`]. Repeated
    /// calls on the *same* frame do nothing at all, so a pointer jittering
    /// inside one frame does not machine-gun it.
    ///
    /// `fps` is the film's, so one window is one frame however the document is
    /// timed. A window shorter than the ramp would be all ramp, so it is held
    /// off a floor.
    pub fn scrub(&mut self, frame: u32) -> Result<()> {
        if self.stream.is_none() {
            match self.open() {
                Ok(()) => {}
                Err(e) => {
                    let message = format!("{e:#}");
                    tracing::warn!("no audio output: {message}");
                    self.unavailable = Some(message);
                    return Ok(());
                }
            }
        }

        let fresh = if let Ok(mut mixer) = self.mixer.lock() {
            let rate = mixer.sample_rate.max(1) as f64;
            let seconds = if mixer.fps > 0.0 { 1.0 / mixer.fps } else { 0.04 };
            // Two ramps and something to hear between them.
            mixer.scrub_window = (seconds.max(RAMP_SECONDS as f64 * 4.0) * rate) as u64;
            let was_scrubbing = mixer.scrub_left.is_some();
            mixer.scrub_left = Some(mixer.scrub_window);
            mixer.playing = true;
            !was_scrubbing
        } else {
            false
        };

        // `seek` is what moves it, click-free. On the first scrub of a drag
        // there is nothing sounding to fade out of, so it lands immediately.
        self.seek(frame);
        if fresh && let Some(stream) = &self.stream {
            stream.play().context("starting the audio stream")?;
        }
        self.state = PlayerState::Playing;
        Ok(())
    }

    /// Where the sound has actually reached, as an animation frame.
    ///
    /// This is what the playhead should follow while playing: the audio clock
    /// is the one the audience hears.
    pub fn position_frame(&self) -> Option<u32> {
        let mixer = self.mixer.lock().ok()?;
        if mixer.sample_rate == 0 || mixer.fps <= 0.0 {
            return None;
        }
        let seconds = mixer.position as f64 / mixer.sample_rate as f64;
        Some((seconds * mixer.fps) as u32)
    }

    /// Open the output device and start the callback.
    fn open(&mut self) -> Result<()> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("no audio output device")?;
        let config = device
            .default_output_config()
            .context("no usable output configuration")?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels();
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.sample_rate = sample_rate;
            mixer.channels = channels;
        }

        let mixer = Arc::clone(&self.mixer);
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.into(),
                move |output: &mut [f32], _| {
                    // A poisoned lock means a panic somewhere else; silence is
                    // the only safe thing an audio callback can do about it.
                    match mixer.lock() {
                        Ok(mut mixer) => mixer.render(output),
                        Err(_) => output.fill(0.0),
                    }
                },
                |e| tracing::error!("audio output error: {e}"),
                None,
            ),
            other => {
                anyhow::bail!("this device wants {other:?} samples, which is not supported yet")
            }
        }
        .context("opening the audio output stream")?;

        self.stream = Some(stream);
        tracing::info!("audio output open at {sample_rate} Hz, {channels} channels");
        Ok(())
    }
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player")
            .field("state", &self.state)
            .field("open", &self.stream.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(seconds: f64, value: f32) -> Arc<Clip> {
        let rate = 48_000;
        Arc::new(
            Clip::new(
                "Test",
                rate,
                1,
                vec![value; (seconds * rate as f64) as usize],
            )
            .expect("a clip"),
        )
    }

    /// The mixer is the part that has to be right; the device is the part that
    /// cannot be tested on a build machine. So the mixer is exercised
    /// directly, with no device involved.
    fn mixer(fps: f64) -> Mixer {
        Mixer {
            sample_rate: 48_000,
            channels: 2,
            fps,
            volume: 1.0,
            ..Mixer::default()
        }
    }

    /// A mixer already playing, with its output envelope **settled**.
    ///
    /// Everything below that is about the *mix* wants the steady state. The
    /// first few milliseconds of any playback are a ramp up from silence -- see
    /// `Mixer::gain` -- and a test that rendered one short buffer from a cold
    /// start would be measuring the declicker rather than the mixer. The
    /// envelope has tests of its own.
    fn rolling(fps: f64) -> Mixer {
        Mixer {
            playing: true,
            gain: 1.0,
            target: 1.0,
            ..mixer(fps)
        }
    }

    fn cue(clip: Arc<Clip>, start_frame: u32, sync: CueSync) -> Cue {
        Cue {
            clip,
            start_frame,
            volume: 1.0,
            sync,
        }
    }

    /// How loud a buffer is, so a test can say "it is sounding" without naming
    /// a sample.
    fn loudness(out: &[f32]) -> f32 {
        out.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// **§7 item 39, the whole of it.** An Event sound is triggered by the
    /// playhead crossing its frame, and from then on runs on its own clock —
    /// so stopping the film does not stop it. Before this every cue was
    /// timeline-positioned, and a sound effect died the instant playback did.
    #[test]
    fn an_event_sound_carries_on_after_the_playhead_stops() {
        let mut mixer = mixer(24.0);
        mixer.cues = vec![cue(clip(1.0, 0.5), 0, CueSync::Event)];
        mixer.playing = true;

        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert!(loudness(&out) > 0.4, "the event should have triggered");

        // Stop the film. The effect is barely started and must finish, which
        // is what Animate does.
        mixer.playing = false;
        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert!(
            loudness(&out) > 0.4,
            "an event sound must not stop with the playhead"
        );
    }

    /// A Stream sound does the opposite, and must keep doing it: it *is* a
    /// position in the film, so stopping the film stops it.
    #[test]
    fn a_stream_sound_stops_with_the_playhead() {
        let mut mixer = mixer(24.0);
        mixer.cues = vec![cue(clip(1.0, 0.5), 0, CueSync::Stream)];
        mixer.playing = true;

        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert!(loudness(&out) > 0.4);

        mixer.playing = false;
        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert_eq!(loudness(&out), 0.0, "a stream is a position, not an event");
    }

    /// Scrubbing moves a stream and leaves an event where it is.
    #[test]
    fn scrubbing_does_not_move_a_sounding_event() {
        let mut mixer = mixer(24.0);
        mixer.cues = vec![cue(clip(1.0, 0.5), 0, CueSync::Event)];
        mixer.playing = true;

        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert_eq!(mixer.voices.len(), 1, "one voice should be sounding");
        let began = mixer.voices[0].began;

        // Scrub somewhere else entirely.
        mixer.position = 40_000;
        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert_eq!(
            mixer.voices[0].began, began,
            "the event's own clock must be untouched by the playhead"
        );
    }

    /// An event ends when its clip does, rather than sounding for ever.
    #[test]
    fn an_event_voice_retires_at_the_end_of_its_clip() {
        let mut mixer = mixer(24.0);
        // A twentieth of a second: 2 400 sample frames.
        mixer.cues = vec![cue(clip(0.05, 0.5), 0, CueSync::Event)];
        mixer.playing = true;

        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert_eq!(mixer.voices.len(), 1);

        for _ in 0..40 {
            let mut out = vec![0.0f32; 512];
            mixer.render(&mut out);
        }
        assert!(mixer.voices.is_empty(), "the voice should have retired");
    }

    /// Start's one difference from Event: it will not stack a second copy on
    /// one already sounding.
    #[test]
    fn start_does_not_overlap_itself_but_event_does() {
        let source = clip(1.0, 0.5);

        let mut m = mixer(24.0);
        m.cues = vec![
            cue(Arc::clone(&source), 0, CueSync::Start),
            cue(Arc::clone(&source), 1, CueSync::Start),
        ];
        m.playing = true;
        // 24 fps at 48 kHz is 2 000 samples a frame, so this buffer crosses
        // both cues in one go.
        let mut out = vec![0.0f32; 8192 * 2];
        m.render(&mut out);
        assert_eq!(
            m.voices.len(),
            1,
            "Start must not stack a second copy on one already sounding"
        );

        let mut m = mixer(24.0);
        m.cues = vec![
            cue(Arc::clone(&source), 0, CueSync::Event),
            cue(source, 1, CueSync::Event),
        ];
        m.playing = true;
        let mut out = vec![0.0f32; 8192 * 2];
        m.render(&mut out);
        assert_eq!(m.voices.len(), 2, "two Events overlap, as they should");
    }

    /// **§7 item 40.** A clip at another rate is resampled by interpolating
    /// between neighbours rather than by holding the nearest.
    ///
    /// A ramp tells the two apart: nearest-neighbour comes out as a staircase,
    /// whose steps show up as *repeated* consecutive values. An interpolating
    /// resampler climbs smoothly and repeats nothing. So the assertion is that
    /// no two consecutive output samples are equal — which no staircase can
    /// satisfy.
    #[test]
    fn resampling_interpolates_rather_than_repeating_the_nearest_sample() {
        // A ramp at 24 kHz played out at 48 kHz, so every other output sample
        // falls exactly between two source samples.
        let ramp: Vec<f32> = (0..2400).map(|i| i as f32 / 2400.0).collect();
        let clip = Arc::new(Clip::new("Ramp", 24_000, 1, ramp).expect("a clip"));

        let mut m = Mixer {
            sample_rate: 48_000,
            channels: 1,
            fps: 24.0,
            volume: 1.0,
            ..Mixer::default()
        };
        m.cues = vec![cue(clip, 0, CueSync::Stream)];
        m.playing = true;

        let mut out = vec![0.0f32; 256];
        m.render(&mut out);

        let repeats = out.windows(2).filter(|p| p[0] == p[1]).count();
        assert_eq!(
            repeats, 0,
            "the output repeats samples — this is still nearest-neighbour"
        );

        // And it really is the ramp: rising, and halfway between the source
        // samples at the odd positions.
        assert!(out[2] > out[0], "the ramp should rise");
        let midpoint = (out[0] + out[2]) * 0.5;
        assert!(
            (out[1] - midpoint).abs() < 1e-6,
            "expected the midpoint {midpoint}, got {}",
            out[1]
        );
    }

    #[test]
    fn nothing_queued_renders_silence() {
        let mut mixer = rolling(24.0);
        let mut out = vec![0.5f32; 256];
        mixer.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_paused_mixer_renders_silence_and_does_not_advance() {
        let mut mixer = mixer(24.0);
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.5),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];
        let mut out = vec![0.0f32; 256];
        mixer.render(&mut out);

        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(mixer.position, 0, "a paused mixer must not move");
    }

    #[test]
    fn a_cue_is_heard_once_playback_reaches_it() {
        let mut mixer = rolling(24.0);
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.5),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];

        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 0.5).abs() < 1e-6),
            "expected the clip"
        );
        assert_eq!(mixer.position, 256, "512 samples over two channels");
    }

    /// A sound placed on frame 12 must be silent before it and audible after —
    /// the whole point of putting sound on a timeline.
    #[test]
    fn a_cue_starting_later_is_silent_until_its_frame() {
        let mut mixer = rolling(24.0);
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.5),
            start_frame: 12,
            volume: 1.0,
            sync: CueSync::Stream,
        }];

        // Frame 0: nothing yet.
        let mut out = vec![0.0f32; 256];
        mixer.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0), "the cue has not started");

        // Jump to frame 12: 12/24 s = 24 000 output frames.
        mixer.position = 24_000;
        mixer.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 0.5).abs() < 1e-6),
            "the cue should be audible from its own frame"
        );
    }

    #[test]
    fn two_cues_sum_and_stay_within_range() {
        let mut mixer = rolling(24.0);
        mixer.cues = vec![
            Cue {
                clip: clip(1.0, 0.7),
                start_frame: 0,
                volume: 1.0,
                sync: CueSync::Stream,
            },
            Cue {
                clip: clip(1.0, 0.7),
                start_frame: 0,
                volume: 1.0,
                sync: CueSync::Stream,
            },
        ];

        let mut out = vec![0.0f32; 128];
        mixer.render(&mut out);
        assert!(
            out.iter().all(|s| *s <= 1.0 && *s >= 0.9),
            "two loud cues should sum and clamp, got {:?}",
            &out[..4]
        );
    }

    #[test]
    fn volume_scales_what_is_heard() {
        let mut mixer = rolling(24.0);
        mixer.volume = 0.5;
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.8),
            start_frame: 0,
            volume: 0.5,
            sync: CueSync::Stream,
        }];

        let mut out = vec![0.0f32; 64];
        mixer.render(&mut out);
        assert!(
            (out[0] - 0.2).abs() < 1e-6,
            "0.8 x 0.5 x 0.5 = 0.2, got {}",
            out[0]
        );
    }

    /// Sample rates rarely match: a 44.1 kHz file on a 48 kHz device is the
    /// normal case, not the exception.
    #[test]
    fn a_clip_at_another_sample_rate_still_plays_for_its_whole_length() {
        let mut mixer = rolling(24.0);
        let clip = Arc::new(Clip::new("x", 44_100, 1, vec![0.5; 44_100]).expect("a clip"));
        mixer.cues = vec![Cue {
            clip,
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];

        // Half a second in, the clip (one second long) is still sounding.
        mixer.position = 24_000;
        let mut out = vec![0.0f32; 64];
        mixer.render(&mut out);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6));

        // Past its end, it stops rather than looping or reading past the end.
        mixer.position = 60_000;
        mixer.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0), "the clip should have ended");
    }

    /// **Nothing the mixer does arrives as a step.**
    ///
    /// A step in the waveform is a click, and the three places one could come
    /// from are starting, stopping, and moving the playhead. The third is the
    /// one that hurts: dragging the playhead repositions the audio many times a
    /// second, and a run of clicks at a steady rate is not a series of clicks,
    /// it is a tone -- the hum that used to sit under the sound while scrubbing.
    #[test]
    fn playback_fades_in_rather_than_starting_on_a_step() {
        let mut mixer = mixer(24.0);
        mixer.playing = true;
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.5),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];

        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert!(
            out[0].abs() < 0.02,
            "playback started on a step of {}",
            out[0]
        );
        assert!(
            (out[out.len() - 1] - 0.5).abs() < 1e-6,
            "the ramp should be over well inside one buffer, ended at {}",
            out[out.len() - 1]
        );
    }

    /// **A seek fades out, moves, and fades back in.**
    ///
    /// The move deliberately does not happen where it is asked for: the gain
    /// has to reach zero first, or the join is exactly the step this is here to
    /// avoid.
    #[test]
    fn a_seek_does_not_land_on_a_step() {
        let mut mixer = rolling(24.0);
        mixer.cues = vec![Cue {
            clip: clip(4.0, 0.5),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];

        // Settled, and loud.
        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert!((out[0] - 0.5).abs() < 1e-6);

        // Ask to move a long way. The position must not have moved yet.
        mixer.pending_seek = Some(96_000);
        let before = mixer.position;
        mixer.render(&mut out);
        assert!(
            out[out.len() - 1].abs() < 1e-6,
            "the buffer carrying a seek should end in silence, ended at {}",
            out[out.len() - 1]
        );
        assert_ne!(before, mixer.position, "the mixer stopped dead");
        assert_eq!(
            mixer.position, 96_000,
            "the move should have happened once the gain reached zero"
        );
        assert_eq!(mixer.pending_seek, None, "the move is done with");

        // And it comes back up rather than snapping on.
        mixer.render(&mut out);
        assert!(out[0].abs() < 0.02, "the new position came in on a step");
    }

    /// A seek that lands where the audio already is does nothing, so holding
    /// the playhead still while dragging something else cannot stutter it.
    #[test]
    fn a_seek_to_where_it_already_is_is_ignored() {
        let mut player = Player::new(24.0);
        {
            let mut mixer = player.mixer.lock().expect("the mixer");
            mixer.sample_rate = 48_000;
            mixer.channels = 2;
            mixer.playing = true;
            mixer.gain = 1.0;
            // Somewhere else entirely, so the first seek has work to do.
            mixer.position = 500;
        }
        player.seek(1);
        assert_eq!(
            player.mixer.lock().expect("the mixer").pending_seek,
            Some(2_000)
        );

        // Asked for the same frame again: still the one move, not a second.
        {
            let mut mixer = player.mixer.lock().expect("the mixer");
            mixer.pending_seek = None;
            mixer.position = 2_000;
        }
        player.seek(1);
        assert_eq!(
            player.mixer.lock().expect("the mixer").pending_seek,
            None,
            "a seek to where it already is should do nothing at all"
        );
    }

    /// **A scrub plays one frame and stops.**
    ///
    /// The bounded window is the whole difference between a jog wheel and
    /// playback. Without it a scrub repositions the audio and lets it *run*:
    /// drag slower than real time — which is what dragging is — and the sound
    /// races ahead of the pointer, is yanked back on the next move, and races
    /// ahead again. The same fragment over and over, and worse still scrubbing
    /// back and forth, because every pass replays the same overlapping pieces.
    #[test]
    fn a_scrub_window_runs_out_and_goes_quiet() {
        let mut mixer = rolling(24.0);
        mixer.cues = vec![Cue {
            clip: clip(4.0, 0.5),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];
        // One frame at 24fps: 2000 output frames at 48kHz.
        mixer.scrub_window = 2_000;
        mixer.scrub_left = Some(2_000);

        // Well inside the window: loud.
        let mut out = vec![0.0f32; 1_024];
        mixer.render(&mut out);
        assert!(
            (out[out.len() - 1] - 0.5).abs() < 1e-6,
            "the window should still be sounding, got {}",
            out[out.len() - 1]
        );

        // Past it: spent, and silent. Six buffers is three thousand output
        // frames against a two-thousand-frame window, with room for the ramp
        // down at the end of it.
        for _ in 0..6 {
            mixer.render(&mut out);
        }
        assert_eq!(mixer.scrub_left, Some(0), "the window did not run out");
        assert!(
            out.iter().all(|s| s.abs() < 1e-6),
            "a spent window is still sounding: {:?}",
            &out[..4]
        );
    }

    /// **A new frame opens a new window**, so dragging on keeps making sound.
    #[test]
    fn moving_to_another_frame_opens_a_fresh_window() {
        let mut mixer = rolling(24.0);
        mixer.cues = vec![Cue {
            clip: clip(4.0, 0.5),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];
        mixer.scrub_window = 2_000;
        mixer.scrub_left = Some(0);

        // Spent: render once so the gain is down where a seek expects it.
        let mut out = vec![0.0f32; 512];
        for _ in 0..3 {
            mixer.render(&mut out);
        }
        assert!(out.iter().all(|s| s.abs() < 1e-6));

        // The playhead moves.
        mixer.pending_seek = Some(4_000);
        mixer.render(&mut out);
        assert_eq!(mixer.pending_seek, None, "the move did not happen");
        assert_eq!(
            mixer.position, 4_000,
            "it should have landed on the new frame"
        );
        assert_eq!(
            mixer.scrub_left,
            Some(2_000),
            "a new frame should have opened a fresh window"
        );

        // And it makes sound again, which is the point.
        for _ in 0..2 {
            mixer.render(&mut out);
        }
        assert!(
            out.iter().any(|s| s.abs() > 0.1),
            "the new window is silent"
        );
    }

    /// **Playback is not a window.** Pressing play after a scrub must run to
    /// the end of the film rather than for a fortieth of a second.
    #[test]
    fn playing_clears_the_scrub_window() {
        let mut player = Player::new(24.0);
        {
            let mut mixer = player.mixer.lock().expect("the mixer");
            mixer.sample_rate = 48_000;
            mixer.channels = 2;
            mixer.scrub_left = Some(10);
            mixer.scrub_window = 2_000;
        }
        // No device in a test: `play` reports the failure and carries on, and
        // the state it sets before reaching for one is what matters here.
        let _ = player.play(0);
        let mixer = player.mixer.lock().expect("the mixer");
        assert_eq!(
            mixer.scrub_left, None,
            "playback inherited the scrub's window"
        );
    }

    /// Every sample's source is computed from its absolute position, so
    /// playback cannot accumulate drift however many buffers go by.
    #[test]
    fn playback_does_not_drift_over_many_buffers() {
        let mut mixer = rolling(24.0);
        mixer.cues = vec![Cue {
            clip: clip(10.0, 0.5),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }];

        let mut out = vec![0.0f32; 480];
        for _ in 0..200 {
            mixer.render(&mut out);
        }
        assert_eq!(
            mixer.position,
            200 * 240,
            "the position must be exactly the samples rendered"
        );
    }

    #[test]
    fn a_player_without_a_device_reports_it_and_carries_on() {
        let mut player = Player::new(24.0);
        assert_eq!(player.state(), PlayerState::Idle);
        assert!(!player.has_sound());

        player.set_cues(vec![Cue {
            clip: clip(0.1, 0.2),
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }]);
        assert!(player.has_sound());

        // On a machine with audio this starts playing; on one without, it
        // records why and returns success either way. Both are acceptable —
        // what must not happen is an error that stops the editor.
        player.play(0).expect("play must not fail the editor");
        assert!(matches!(
            player.state(),
            PlayerState::Playing | PlayerState::Idle
        ));
    }

    #[test]
    fn seeking_moves_the_position_to_that_frame() {
        let mut player = Player::new(24.0);
        {
            let mut mixer = player.mixer.lock().expect("lock");
            mixer.sample_rate = 48_000;
            mixer.channels = 2;
        }
        player.seek(24);
        assert_eq!(player.position_frame(), Some(24));

        player.seek(0);
        assert_eq!(player.position_frame(), Some(0));
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// Open the real audio device and play a tone.
    ///
    /// Ignored by default: a build machine may have no sound card, and a test
    /// that fails for that reason teaches nothing. Run it by hand to check
    /// that this machine's output actually works through our path:
    ///
    /// ```sh
    /// cargo test -p buzz-audio --lib device -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "opens the real audio device and makes a noise"]
    fn the_real_device_opens_and_plays() {
        let rate = 48_000;
        let samples: Vec<f32> = (0..rate)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (t * 440.0 * std::f32::consts::TAU).sin() * 0.2
            })
            .collect();
        let clip = Arc::new(Clip::new("Tone", rate as u32, 1, samples).expect("a clip"));

        let mut player = Player::new(24.0);
        player.set_cues(vec![Cue {
            clip,
            start_frame: 0,
            volume: 1.0,
            sync: CueSync::Stream,
        }]);

        player.play(0).expect("play");
        println!("state after play: {:?}", player.state());
        if let Some(reason) = player.unavailable() {
            println!("no audio device: {reason}");
            return;
        }
        assert_eq!(player.state(), PlayerState::Playing);

        std::thread::sleep(std::time::Duration::from_millis(700));
        let reached = player.position_frame().expect("a position");
        println!("position after 700 ms: frame {reached}");
        assert!(
            reached >= 12,
            "at 24 fps, 700 ms should be about 17 frames; the callback does not \
             appear to be running (got {reached})"
        );

        player.pause();
        assert_eq!(player.state(), PlayerState::Paused);
    }
}
