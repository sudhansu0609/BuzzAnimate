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
pub mod mic;
pub mod player;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};

pub use analysis::{
    LipSyncOptions, Phrase, PhraseOptions, Viseme, VisemeTrack, analyse_visemes, beats_from_levels,
    detect_beats, detect_phrases, phrases_from_levels,
};
pub use player::{Player, PlayerState};

/// Decoded audio, ready to play, draw and analyse.
///
/// `PartialEq` is hand-written to ignore [`Self::mono_cache`], which is a
/// memoised derivation of `samples` and so never changes what a clip *is*.
#[derive(Debug, Clone)]
pub struct Clip {
    /// Name for the library, taken from the file.
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples, `-1.0..=1.0`.
    pub samples: Vec<f32>,
    /// The mono mix-down, built once and shared.
    ///
    /// Every waveform level, every peak bucket and every lip-sync pass wants the
    /// samples as one channel; without this each call cloned the whole buffer —
    /// tens of megabytes for a few minutes of stereo, per call, per frame. Built
    /// lazily by [`Self::mono`] and handed out as an `Arc`, so it is computed at
    /// most once ever and every later reader gets a pointer copy.
    mono_cache: OnceLock<Arc<Vec<f32>>>,
}

impl PartialEq for Clip {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.sample_rate == other.sample_rate
            && self.channels == other.channels
            && self.samples == other.samples
    }
}

impl Clip {
    /// Decode a file, choosing the reader from its contents.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
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
        let mut reader =
            hound::WavReader::new(std::io::Cursor::new(bytes)).context("reading the WAV header")?;
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
        use symphonia::core::codecs::audio::AudioDecoderOptions;
        use symphonia::core::formats::probe::Hint;
        use symphonia::core::formats::{FormatOptions, TrackType};
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;

        // **0.6, not 0.5:** symphonia 0.5.5's MP3 decoder rejected every
        // granule whose data starts exactly at the end of the frame's main
        // data (a silent granule, legal) as "invalid main_data offset" and
        // the packet was skipped. Bimari5's 320 kbps narration lost 641 of
        // 7916 frames that way (8%, 16.8 s), so its sound ran early and sounded
        // sped up. 0.6 fixed the check (`<` became `<=`).
        let source = std::io::Cursor::new(bytes.to_vec());
        let stream = MediaSourceStream::new(Box::new(source), Default::default());

        let mut format = symphonia::default::get_probe()
            .probe(
                &Hint::new(),
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .context("this is not an audio format BuzzAnimate can read")?;

        let track = format
            .default_track(TrackType::Audio)
            .context("the file has no audio track")?
            .clone();
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .context("the file has no audio track")?
            .clone();
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .context("no decoder for this audio codec")?;

        let mut samples: Vec<f32> = Vec::new();
        let mut sample_rate = params.sample_rate.unwrap_or(44_100);
        let mut channels = params.channels.as_ref().map(|c| c.count() as u16).unwrap_or(2);
        let mut buffer: Vec<f32> = Vec::new();

        loop {
            let packet = match format.next_packet() {
                Ok(Some(p)) => p,
                // The end of the stream; anything else is a real failure and
                // is reported rather than silently truncating the dialogue.
                Ok(None) => break,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(symphonia::core::errors::Error::ResetRequired) => break,
                Err(e) => return Err(e).context("reading audio packets"),
            };
            if packet.track_id != track.id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = decoded.spec();
                    sample_rate = spec.rate();
                    channels = spec.channels().count() as u16;

                    decoded.copy_to_vec_interleaved(&mut buffer);
                    samples.extend_from_slice(&buffer);
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
            mono_cache: OnceLock::new(),
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

    /// **The section between two in-clip times**, as its own [`Clip`] — what
    /// analysis wants for a sound that is only heard `trim_start..trim_end`
    /// of its own file (see `buzz_scene::SoundRef::trim_start`/`trim_end`).
    ///
    /// Lip sync and phrase detection both search *from the front of the
    /// clip*: without this, a shot whose Dialogue layer plays only a slice of
    /// a longer master track would have its mouth shapes (or its proposed
    /// cuts) searched for across audio that never plays in that shot at all.
    /// `end_seconds` of `None` means to the clip's own end. Out-of-range or
    /// empty requests come back as a clip of zero frames rather than
    /// panicking; the caller (analysis) already treats "nothing to hear" as
    /// its own, reportable outcome.
    pub fn trimmed(&self, start_seconds: f32, end_seconds: Option<f32>) -> Clip {
        let channels = self.channels.max(1) as usize;
        let total_frames = self.len();
        let to_frame = |seconds: f32| ((seconds.max(0.0) as f64) * self.sample_rate as f64).round() as usize;
        let start_frame = to_frame(start_seconds).min(total_frames);
        let end_frame = end_seconds
            .map(|s| to_frame(s).min(total_frames))
            .unwrap_or(total_frames)
            .max(start_frame);

        let samples = self.samples[start_frame * channels..end_frame * channels].to_vec();
        Clip {
            name: self.name.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples,
            mono_cache: OnceLock::new(),
        }
    }

    /// One channel, averaged — what analysis and the waveform both want.
    ///
    /// Built once and shared: the first caller pays the mix-down, every later
    /// one gets a pointer copy of the same `Arc`. See [`Self::mono_cache`].
    pub fn mono(&self) -> Arc<Vec<f32>> {
        self.mono_cache
            .get_or_init(|| {
                let channels = self.channels.max(1) as usize;
                let mono = if channels == 1 {
                    self.samples.clone()
                } else {
                    self.samples
                        .chunks(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                        .collect()
                };
                Arc::new(mono)
            })
            .clone()
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

    /// The mono mix-down at `target_rate` — what speech recognition wants
    /// (Whisper takes 16 kHz mono).
    ///
    /// A Hann-windowed sinc resampler, low-passed at the lower of the two
    /// Nyquist frequencies, so downsampling 44.1/48 kHz dialogue does not fold
    /// sibilants back into the speech band the way picking or linearly
    /// interpolating samples would.
    pub fn resampled_mono(&self, target_rate: u32) -> Vec<f32> {
        resample(&self.mono(), self.sample_rate, target_rate)
    }
}

/// Resamples one channel from `from` Hz to `to` Hz with a windowed sinc.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    // Cut-off relative to the input rate; a little under Nyquist.
    let cutoff = ratio.min(1.0) * 0.95;
    const ZERO_CROSSINGS: f64 = 16.0;
    let half_width = ZERO_CROSSINGS / cutoff; // in input samples
    let out_len = ((input.len() as f64) * ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let centre = n as f64 / ratio;
        let lo = ((centre - half_width).ceil().max(0.0)) as usize;
        let hi = ((centre + half_width).floor() as usize).min(input.len() - 1);
        let mut acc = 0.0f64;
        let mut norm = 0.0f64;
        for (k, &x) in input.iter().enumerate().take(hi + 1).skip(lo) {
            let t = k as f64 - centre;
            let arg = std::f64::consts::PI * t * cutoff;
            let sinc = if arg.abs() < 1e-9 { 1.0 } else { arg.sin() / arg };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * t / half_width).cos();
            let h = sinc * w;
            acc += x as f64 * h;
            norm += h;
        }
        out.push(if norm.abs() > 1e-12 { (acc / norm) as f32 } else { 0.0 });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 44.1 kHz stereo → 16 kHz mono keeps the length, a 440 Hz tone's
    /// amplitude, and removes a 10 kHz tone that 16 kHz cannot carry.
    #[test]
    fn resampling_to_16k_keeps_speech_band_and_drops_what_would_alias() {
        let rate = 44_100u32;
        let n = rate as usize; // one second
        let low: Vec<f32> = (0..n)
            .map(|i| (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        let high: Vec<f32> = (0..n)
            .map(|i| (i as f32 / rate as f32 * 10_000.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let lo16 = resample(&low, rate, 16_000);
        let hi16 = resample(&high, rate, 16_000);
        assert!((lo16.len() as i64 - 16_000).abs() <= 1, "{}", lo16.len());
        let mid = &lo16[1000..15_000];
        assert!((rms(mid) - 0.3536).abs() < 0.02, "440 Hz rms {}", rms(mid));
        assert!(rms(&hi16[1000..15_000]) < 0.02, "10 kHz leaked: {}", rms(&hi16[1000..15_000]));
        let stereo = Clip::new("s", rate, 2, low.iter().flat_map(|&x| [x, x]).collect()).unwrap();
        assert_eq!(stereo.resampled_mono(16_000).len(), lo16.len());
    }

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
        let error =
            Clip::decode(b"this is not a sound file at all", "x").expect_err("should be refused");
        let message = error.to_string();
        assert!(!message.is_empty());
    }

    /// **`trimmed` keeps only the section between the two in-points**, at the
    /// same sample rate — what a shot's own slice of a longer master track
    /// wants handed to analysis.
    #[test]
    fn trimmed_keeps_only_the_section_asked_for() {
        let clip = tone(4.0, 44_100, 1);

        let middle = clip.trimmed(1.0, Some(3.0));
        assert!(
            (middle.duration_seconds() - 2.0).abs() < 0.01,
            "expected ~2s, got {:.3}s",
            middle.duration_seconds()
        );

        let to_end = clip.trimmed(3.0, None);
        assert!(
            (to_end.duration_seconds() - 1.0).abs() < 0.01,
            "expected ~1s, got {:.3}s",
            to_end.duration_seconds()
        );

        // The samples themselves come from the right place, not just the
        // right length: frame 0 of the trimmed clip is frame 1s of the
        // source.
        let offset = (1.0 * 44_100.0) as usize;
        assert_eq!(middle.samples[0], clip.samples[offset]);
    }

    #[test]
    fn stereo_mixes_down_to_mono_by_averaging() {
        let clip = Clip::new("x", 8_000, 2, vec![1.0, -1.0, 0.5, 0.5]).expect("clip");
        assert_eq!(*clip.mono(), vec![0.0, 0.5]);
        // The mix-down is memoised, so a second call is the same allocation.
        assert!(Arc::ptr_eq(&clip.mono(), &clip.mono()));
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
        assert!(
            peaks
                .iter()
                .all(|(min, max)| min.is_finite() && max.is_finite())
        );
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
        assert_eq!(
            clip.duration_frames(0.0),
            0,
            "an impossible rate is not a panic"
        );
    }

    #[test]
    fn empty_or_malformed_audio_is_refused() {
        assert!(Clip::new("x", 44_100, 1, Vec::new()).is_err());
        assert!(Clip::new("x", 0, 1, vec![0.1]).is_err());
        assert!(Clip::new("x", 44_100, 0, vec![0.1]).is_err());
    }
}
