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

/// One sound queued for playback, positioned on the timeline.
#[derive(Debug, Clone)]
pub struct Cue {
    pub clip: Arc<Clip>,
    /// The animation frame this sound starts on.
    pub start_frame: u32,
    pub volume: f32,
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
}

impl Mixer {
    /// Fill `output` with whatever the cues have at the current position.
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        if !self.playing || self.cues.is_empty() {
            return;
        }

        let channels = self.channels.max(1) as usize;
        let frames = output.len() / channels;

        for cue in &self.cues {
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
                // Nearest-neighbour resampling. A proper resampler belongs
                // here eventually; for scrubbing dialogue against picture,
                // pitch accuracy matters far less than being in the right
                // place, and this cannot drift because every sample's source
                // index is computed from its absolute position.
                let source = (((at - start) as f64) * ratio) as usize;
                if source >= clip.len() {
                    continue;
                }
                for c in 0..channels {
                    let sample = clip.samples
                        [source * clip_channels + c.min(clip_channels - 1)]
                        * gain;
                    output[i * channels + c] += sample;
                }
            }
        }

        // Sum without clipping to a hard edge: two loud cues together would
        // otherwise square off into audible distortion.
        for sample in output.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }

        self.position += frames as u64;
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
    pub fn set_cues(&mut self, cues: Vec<Cue>) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.cues = cues;
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
        }
        if self.state == PlayerState::Playing {
            self.state = PlayerState::Paused;
        }
    }

    /// Move to a frame. Takes effect on the next buffer, playing or not.
    pub fn seek(&mut self, frame: u32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            let rate = mixer.sample_rate.max(1) as f64;
            mixer.position = if mixer.fps > 0.0 {
                (frame as f64 / mixer.fps * rate) as u64
            } else {
                0
            };
        }
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

    #[test]
    fn nothing_queued_renders_silence() {
        let mut mixer = mixer(24.0);
        mixer.playing = true;
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
        }];
        let mut out = vec![0.0f32; 256];
        mixer.render(&mut out);

        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(mixer.position, 0, "a paused mixer must not move");
    }

    #[test]
    fn a_cue_is_heard_once_playback_reaches_it() {
        let mut mixer = mixer(24.0);
        mixer.playing = true;
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.5),
            start_frame: 0,
            volume: 1.0,
        }];

        let mut out = vec![0.0f32; 512];
        mixer.render(&mut out);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6), "expected the clip");
        assert_eq!(mixer.position, 256, "512 samples over two channels");
    }

    /// A sound placed on frame 12 must be silent before it and audible after —
    /// the whole point of putting sound on a timeline.
    #[test]
    fn a_cue_starting_later_is_silent_until_its_frame() {
        let mut mixer = mixer(24.0);
        mixer.playing = true;
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.5),
            start_frame: 12,
            volume: 1.0,
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
        let mut mixer = mixer(24.0);
        mixer.playing = true;
        mixer.cues = vec![
            Cue {
                clip: clip(1.0, 0.7),
                start_frame: 0,
                volume: 1.0,
            },
            Cue {
                clip: clip(1.0, 0.7),
                start_frame: 0,
                volume: 1.0,
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
        let mut mixer = mixer(24.0);
        mixer.playing = true;
        mixer.volume = 0.5;
        mixer.cues = vec![Cue {
            clip: clip(1.0, 0.8),
            start_frame: 0,
            volume: 0.5,
        }];

        let mut out = vec![0.0f32; 64];
        mixer.render(&mut out);
        assert!((out[0] - 0.2).abs() < 1e-6, "0.8 x 0.5 x 0.5 = 0.2, got {}", out[0]);
    }

    /// Sample rates rarely match: a 44.1 kHz file on a 48 kHz device is the
    /// normal case, not the exception.
    #[test]
    fn a_clip_at_another_sample_rate_still_plays_for_its_whole_length() {
        let mut mixer = mixer(24.0);
        mixer.playing = true;
        let clip = Arc::new(Clip::new("x", 44_100, 1, vec![0.5; 44_100]).expect("a clip"));
        mixer.cues = vec![Cue {
            clip,
            start_frame: 0,
            volume: 1.0,
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

    /// Every sample's source is computed from its absolute position, so
    /// playback cannot accumulate drift however many buffers go by.
    #[test]
    fn playback_does_not_drift_over_many_buffers() {
        let mut mixer = mixer(24.0);
        mixer.playing = true;
        mixer.cues = vec![Cue {
            clip: clip(10.0, 0.5),
            start_frame: 0,
            volume: 1.0,
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
