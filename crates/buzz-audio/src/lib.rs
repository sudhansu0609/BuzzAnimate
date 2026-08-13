//! Sound: decoding, waveforms, playback and lip-sync analysis.
//!
//! # What this crate is for
//!
//! An animator works to a soundtrack. The dialogue arrives first, and every
//! decision after it — where the accents land, when a mouth opens — is made
//! against what they can hear. So sound is not a publishing concern to be
//! bolted on at the end; it has to be audible while drawing, at the right
//! frame, from wherever in the document you happen to be working.
//!
//! Nothing here knows about documents or layers. A [`Clip`] is samples, a
//! [`Player`] makes them audible, and [`analysis`] turns them into mouth
//! shapes. The document model decides *which* clip plays and when.
//!
//! # Everything is decoded to memory, once
//!
//! A minute of 44.1 kHz stereo is about 10 MB as `f32`, and dialogue tracks
//! are minutes rather than hours. Decoding once means seeking is arithmetic —
//! which is what scrubbing a timeline is, dozens of times a second — instead
//! of a decoder re-syncing to a bitstream on every playhead move.

pub mod analysis;
pub mod player;

use std::path::Path;

use anyhow::{Context, Result, bail};

pub use analysis::{LipSyncOptions, Viseme, VisemeTrack, analyse_visemes};
pub use player::{Player, PlayerState};

/// Decoded audio, ready to play, draw and analyse.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Name for the library, taken from the file.
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples, `-1.0..=1.0`.
    pub samples: Vec<f32>,
}

impl Clip {
    /// Decode a file, choosing the reader from its contents.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Sound".to_string());
        Self::decode(&bytes, &name)
    }

    /// Decode bytes already in memory.
    ///
    /// WAV goes through `hound` and everything else through Symphonia. Two
    /// readers rather than one because WAV is what dialogue is almost always
    /// delivered as, and `hound` is a hundred lines of well-understood code
    /// where Symphonia is a probe-and-demux pipeline — when a file will not
    /// open, it matters which of those said no.
    pub fn decode(bytes: &[u8], name: &str) -> Result<Self> {
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            return Self::decode_wav(bytes, name);
        }
        Self::decode_symphonia(bytes, name)
    }

    fn decode_wav(bytes: &[u8], name: &str) -> Result<Self> {
        let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))
            .context("reading the WAV header")?;
        let spec = reader.spec();

        // Integer PCM is scaled by its own full range rather than by 32768 for
        // everything: a 24-bit file scaled as 16-bit comes out 256 times too
        // loud, which is not subtle but is easy to write.
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<std::result::Result<_, _>>()
                .context("reading float samples")?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 * scale))
                    .collect::<std::result::Result<_, _>>()
                    .context("reading integer samples")?
            }
        };

        Self::new(name, spec.sample_rate, spec.channels, samples)
    }

    fn decode_symphonia(bytes: &[u8], name: &str) -> Result<Self> {
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::codecs::DecoderOptions;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let source = std::io::Cursor::new(bytes.to_vec());
        let stream = MediaSourceStream::new(Box::new(source), Default::default());

        let probed = symphonia::default::get_probe()
            .format(
                &Hint::new(),
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .context("this is not an audio format BuzzAnimate can read")?;

        let mut format = probed.format;
        let track = format
            .default_track()
            .context("the file has no audio track")?
            .clone();
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .context("no decoder for this audio codec")?;

        let mut samples: Vec<f32> = Vec::new();
        let mut sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
        let mut channels = track
            .codec_params
            .channels
            .map(|c| c.count() as u16)
            .unwrap_or(2);

        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                // The end of the stream arrives as an error; anything else is
                // a real failure and is reported rather than silently
                // truncating the dialogue.
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(symphonia::core::errors::Error::ResetRequired) => break,
                Err(e) => return Err(e).context("reading audio packets"),
            };
            if packet.track_id() != track.id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    sample_rate = spec.rate;
                    channels = spec.channels.count() as u16;

                    let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    buffer.copy_interleaved_ref(decoded);
                    samples.extend_from_slice(buffer.samples());
                }
                // A damaged packet in the middle of a file should cost that
                // packet, not the whole take.
                Err(symphonia::core::errors::Error::DecodeError(e)) => {
                    tracing::warn!("skipping a damaged audio packet: {e}");
                }
                Err(e) => return Err(e).context("decoding audio"),
            }
        }

        Self::new(name, sample_rate, channels, samples)
    }

    /// Build a clip from samples already decoded.
    pub fn new(name: &str, sample_rate: u32, channels: u16, samples: Vec<f32>) -> Result<Self> {
        if sample_rate == 0 {
            bail!("the file reports a sample rate of zero");
        }
        if channels == 0 {
            bail!("the file reports no channels");
        }
        if samples.is_empty() {
            bail!("the file contains no audio");
        }
        Ok(Self {
            name: name.to_string(),
            sample_rate,
            channels,
            samples,
        })
    }

    /// Sample frames — one per instant, however many channels there are.
    pub fn len(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn duration_seconds(&self) -> f64 {
        self.len() as f64 / self.sample_rate as f64
    }

    /// How many animation frames this clip covers at `fps`.
    pub fn duration_frames(&self, fps: f64) -> u32 {
        if fps <= 0.0 {
            return 0;
        }
        (self.duration_seconds() * fps).ceil() as u32
    }

    /// One channel, averaged — what analysis and the waveform both want.
    pub fn mono(&self) -> Vec<f32> {
        let channels = self.channels.max(1) as usize;
        if channels == 1 {
            return self.samples.clone();
        }
        self.samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    }

    /// Minimum and maximum per bucket, for drawing a waveform.
    ///
    /// Peaks rather than averages: an averaged waveform of speech is a
    /// featureless blur, because the positive and negative halves cancel. The
    /// shape an animator reads — where the words are — is the *envelope*.
    pub fn peaks(&self, buckets: usize) -> Vec<(f32, f32)> {
        let mono = self.mono();
        if buckets == 0 || mono.is_empty() {
            return Vec::new();
        }
        let per = (mono.len() as f64 / buckets as f64).max(1.0);

        (0..buckets)
            .map(|i| {
                let start = (i as f64 * per) as usize;
                let end = (((i + 1) as f64 * per) as usize).min(mono.len());
                if start >= end {
                    return (0.0, 0.0);
                }
                let slice = &mono[start..end];
                let min = slice.iter().copied().fold(f32::INFINITY, f32::min);
                let max = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                (min, max)
            })
            .collect()
    }

    /// Loudness per animation frame, `0.0..=1.0` — the envelope a waveform
    /// strip draws and lip sync starts from.
    pub fn frame_levels(&self, fps: f64) -> Vec<f32> {
        if fps <= 0.0 {
            return Vec::new();
        }
        let mono = self.mono();
        let per_frame = (self.sample_rate as f64 / fps).max(1.0);
        let frames = self.duration_frames(fps) as usize;

        (0..frames)
            .map(|i| {
                let start = (i as f64 * per_frame) as usize;
                let end = (((i + 1) as f64 * per_frame) as usize).min(mono.len());
                if start >= end {
                    return 0.0;
                }
                let sum: f32 = mono[start..end].iter().map(|s| s * s).sum();
                (sum / (end - start) as f32).sqrt().min(1.0)
            })
            .collect()
    }

    /// The samples covering one animation frame, for analysis.
    pub fn frame_window(&self, frame: u32, fps: f64) -> &[f32] {
        if fps <= 0.0 {
            return &[];
        }
        let channels = self.channels.max(1) as usize;
        let per_frame = (self.sample_rate as f64 / fps).max(1.0);
        let start = ((frame as f64 * per_frame) as usize * channels).min(self.samples.len());
        let end = (((frame + 1) as f64 * per_frame) as usize * channels).min(self.samples.len());
        &self.samples[start..end]
    }

    /// Rough memory cost, for the status bar and for deciding what to warn on.
    pub fn bytes(&self) -> usize {
        self.samples.len() * std::mem::size_of::<f32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second of a 440 Hz tone, as a real file would arrive.
    fn tone(seconds: f64, rate: u32, channels: u16) -> Clip {
        let frames = (seconds * rate as f64) as usize;
        let mut samples = Vec::with_capacity(frames * channels as usize);
        for i in 0..frames {
            let t = i as f64 / rate as f64;
            let v = (t * 440.0 * std::f64::consts::TAU).sin() as f32 * 0.5;
            for _ in 0..channels {
                samples.push(v);
            }
        }
        Clip::new("Tone", rate, channels, samples).expect("a clip")
    }

    fn wav_bytes(clip: &Clip, bits: u16) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: clip.channels,
            sample_rate: clip.sample_rate,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut out, spec).expect("writer");
            let scale = (1i64 << (bits - 1)) as f32 - 1.0;
            for s in &clip.samples {
                writer.write_sample((s * scale) as i32).expect("write");
            }
            writer.finalize().expect("finalize");
        }
        out.into_inner()
    }

    #[test]
    fn a_wav_file_decodes_to_the_samples_it_holds() {
        let source = tone(0.25, 44_100, 2);
        let bytes = wav_bytes(&source, 16);

        let decoded = Clip::decode(&bytes, "Dialogue").expect("decode");
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.len(), source.len());
        assert!((decoded.duration_seconds() - 0.25).abs() < 0.01);

        // Sample values survive, within 16-bit quantisation.
        for (a, b) in source.samples.iter().zip(&decoded.samples).take(500) {
            assert!((a - b).abs() < 0.001, "{a} became {b}");
        }
    }

    /// A 24-bit file scaled as though it were 16-bit comes out 256 times too
    /// loud. Not subtle, but very easy to write.
    #[test]
    fn bit_depth_is_scaled_by_its_own_range() {
        let source = tone(0.05, 22_050, 1);
        for bits in [16u16, 24, 32] {
            let decoded = Clip::decode(&wav_bytes(&source, bits), "x").expect("decode");
            let loudest = decoded.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(
                (0.4..=0.6).contains(&loudest),
                "{bits}-bit came back at {loudest}, not around 0.5"
            );
        }
    }

    #[test]
    fn a_file_that_is_not_audio_is_refused_with_a_reason() {
        let error = Clip::decode(b"this is not a sound file at all", "x")
            .expect_err("should be refused");
        let message = error.to_string();
        assert!(!message.is_empty());
    }

    #[test]
    fn stereo_mixes_down_to_mono_by_averaging() {
        let clip = Clip::new("x", 8_000, 2, vec![1.0, -1.0, 0.5, 0.5]).expect("clip");
        assert_eq!(clip.mono(), vec![0.0, 0.5]);
        assert_eq!(clip.len(), 2, "two sample frames, not four samples");
    }

    #[test]
    fn peaks_describe_the_envelope_rather_than_the_average() {
        let clip = tone(0.5, 44_100, 1);
        let peaks = clip.peaks(64);

        assert_eq!(peaks.len(), 64);
        // A tone's envelope is flat and full: every bucket reaches both ways.
        for (min, max) in &peaks {
            assert!(*max > 0.4, "a bucket should reach the tone's peak: {max}");
            assert!(*min < -0.4, "and its trough: {min}");
        }
    }

    #[test]
    fn peaks_cope_with_more_buckets_than_samples() {
        let clip = Clip::new("x", 8_000, 1, vec![0.5; 4]).expect("clip");
        let peaks = clip.peaks(100);
        assert_eq!(peaks.len(), 100);
        assert!(peaks.iter().all(|(min, max)| min.is_finite() && max.is_finite()));
    }

    #[test]
    fn frame_levels_give_one_reading_per_animation_frame() {
        let clip = tone(1.0, 44_100, 1);
        let levels = clip.frame_levels(24.0);

        assert_eq!(levels.len(), 24, "one second at 24 fps");
        // RMS of a 0.5-amplitude sine is 0.5/sqrt(2) ~ 0.354.
        for level in &levels {
            assert!((0.3..=0.4).contains(level), "level {level}");
        }
    }

    #[test]
    fn silence_reads_as_silence() {
        let clip = Clip::new("x", 44_100, 1, vec![0.0; 44_100]).expect("clip");
        assert!(clip.frame_levels(24.0).iter().all(|l| *l < 1e-6));
    }

    #[test]
    fn a_frame_window_covers_that_frames_samples() {
        let clip = tone(1.0, 48_000, 2);
        let window = clip.frame_window(0, 24.0);
        assert_eq!(window.len(), 2_000 * 2, "48000/24 frames, two channels");

        // Past the end is empty rather than a panic.
        assert!(clip.frame_window(9_999, 24.0).is_empty());
    }

    #[test]
    fn a_clip_reports_how_many_frames_it_covers() {
        let clip = tone(2.0, 44_100, 1);
        assert_eq!(clip.duration_frames(24.0), 48);
        assert_eq!(clip.duration_frames(0.0), 0, "an impossible rate is not a panic");
    }

    #[test]
    fn empty_or_malformed_audio_is_refused() {
        assert!(Clip::new("x", 44_100, 1, Vec::new()).is_err());
        assert!(Clip::new("x", 0, 1, vec![0.1]).is_err());
        assert!(Clip::new("x", 44_100, 0, vec![0.1]).is_err());
    }
}
