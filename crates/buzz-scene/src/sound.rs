//! Sound in the document: imported clips, and where they play.
//!
//! # The rule that shapes this module
//!
//! **A sound belongs to the document, not to the timeline you are looking
//! at.** Open a character symbol to animate its walk and the dialogue you are
//! animating *to* must keep playing; open the head inside that character and
//! it must still keep playing. Otherwise the one thing you need to hear
//! disappears exactly when you go in to do the work it is for.
//!
//! So sounds are collected by [`Scene::stage_cues`], which reads the
//! document's own timeline — `stage_layers` — regardless of which symbol is
//! open for editing, at any depth. That is the same distinction saving
//! already makes, and it is why it exists: the document is one thing, and your
//! view of it is another.
//!
//! # Why the original file is kept
//!
//! A [`SoundAsset`] holds the bytes as imported. Keeping the decoded samples
//! instead would inflate a 3 MB MP3 into 50 MB of `f32` in every saved
//! document and every undo snapshot, and would bake in one decoder's idea of
//! the file forever. Decoding happens once on load, into a cache the editor
//! owns.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Stable identity for an imported sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SoundId(pub u64);

/// An imported sound, as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundAsset {
    pub id: SoundId,
    /// Name in the Library.
    pub name: String,
    /// The file, byte for byte. Shared, because undo snapshots the scene and a
    /// dialogue track must not be copied for every brush stroke.
    pub data: Arc<Vec<u8>>,
    /// Lower-case extension — what the file is, for saving it back out.
    pub format: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Length in sample frames, so duration needs no decoder.
    pub length: u64,
}

impl SoundAsset {
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.length as f64 / self.sample_rate as f64
    }

    /// How many animation frames this sound covers at `fps`.
    pub fn duration_frames(&self, fps: f64) -> u32 {
        if fps <= 0.0 {
            return 0;
        }
        (self.duration_seconds() * fps).ceil() as u32
    }

    /// File name inside the `.buzz` container's `media/` directory.
    pub fn file_name(&self) -> String {
        format!("sound-{}.{}", self.id.0, self.format)
    }
}

/// Animate's four sound sync modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SoundSync {
    /// Plays independently once triggered, to its own end. Animate's default,
    /// and right for a one-off effect.
    #[default]
    Event,
    /// Like Event, but a second trigger while it is already playing is
    /// ignored rather than overlapping.
    Start,
    /// Silences this sound from here.
    Stop,
    /// **Tied to the timeline.** The frame decides the position, so scrubbing
    /// moves the sound and playback cannot drift from the picture. This is
    /// what dialogue is, and what lip sync is animated against.
    Stream,
}

impl SoundSync {
    pub fn label(self) -> &'static str {
        match self {
            SoundSync::Event => "Event",
            SoundSync::Start => "Start",
            SoundSync::Stop => "Stop",
            SoundSync::Stream => "Stream",
        }
    }

    /// Does this sound follow the playhead?
    pub fn follows_timeline(self) -> bool {
        self == SoundSync::Stream
    }
}

/// A sound placed on a keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SoundRef {
    pub sound: SoundId,
    pub sync: SoundSync,
    /// `0.0..=1.0`.
    pub volume: f32,
    /// How many times to play. Zero means loop for as long as the timeline
    /// runs, which is what Animate's "loop" checkbox does.
    pub loops: u32,
}

impl SoundRef {
    /// Dialogue: streamed, full volume, played once.
    pub fn stream(sound: SoundId) -> Self {
        Self {
            sound,
            sync: SoundSync::Stream,
            volume: 1.0,
            loops: 1,
        }
    }

    pub fn event(sound: SoundId) -> Self {
        Self {
            sound,
            sync: SoundSync::Event,
            volume: 1.0,
            loops: 1,
        }
    }
}

/// One sound, positioned on the document's timeline.
///
/// What the player is handed: which clip, which frame it starts on, and how
/// loud. Deliberately free of layers and keyframes — the audio thread should
/// not have to understand a document to fill a buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundCue {
    pub sound: SoundId,
    pub start_frame: u32,
    pub volume: f32,
    pub sync: SoundSync,
}

/// Every sound a document holds.
///
/// The map is behind an `Arc` so cloning the library — which every
/// `Document::edit` does — is a pointer copy rather than one tree node per
/// sound; see [`crate::Library`] for the full rationale. Mutation forks the map
/// once through [`Arc::make_mut`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundLibrary {
    sounds: Arc<BTreeMap<SoundId, Arc<SoundAsset>>>,
}

impl SoundLibrary {
    pub fn len(&self) -> usize {
        self.sounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sounds.is_empty()
    }

    pub fn get(&self, id: SoundId) -> Option<&Arc<SoundAsset>> {
        self.sounds.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<SoundAsset>> {
        self.sounds.values()
    }

    pub fn insert(&mut self, asset: SoundAsset) {
        Arc::make_mut(&mut self.sounds).insert(asset.id, Arc::new(asset));
    }

    pub fn remove(&mut self, id: SoundId) -> Option<Arc<SoundAsset>> {
        Arc::make_mut(&mut self.sounds).remove(&id)
    }

    /// A name no existing sound has, so the Library stays readable.
    pub fn unique_name(&self, wanted: &str) -> String {
        if !self.sounds.values().any(|s| s.name == wanted) {
            return wanted.to_string();
        }
        for n in 2..10_000 {
            let candidate = format!("{wanted} {n}");
            if !self.sounds.values().any(|s| s.name == candidate) {
                return candidate;
            }
        }
        wanted.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: u64, name: &str, length: u64) -> SoundAsset {
        SoundAsset {
            id: SoundId(id),
            name: name.to_string(),
            data: Arc::new(vec![0; 16]),
            format: "wav".into(),
            sample_rate: 44_100,
            channels: 1,
            length,
        }
    }

    #[test]
    fn duration_comes_from_the_sample_count_without_decoding() {
        let sound = asset(1, "Line", 44_100 * 2);
        assert!((sound.duration_seconds() - 2.0).abs() < 1e-9);
        assert_eq!(sound.duration_frames(24.0), 48);
        assert_eq!(sound.duration_frames(0.0), 0, "not a division by zero");
    }

    #[test]
    fn a_sound_names_its_file_in_the_container() {
        let sound = asset(7, "Line", 100);
        assert_eq!(sound.file_name(), "sound-7.wav");
    }

    #[test]
    fn the_library_keeps_names_distinct() {
        let mut library = SoundLibrary::default();
        library.insert(asset(1, "Dialogue", 10));
        assert_eq!(library.unique_name("Dialogue"), "Dialogue 2");
        assert_eq!(library.unique_name("Footsteps"), "Footsteps");

        library.insert(asset(2, "Dialogue 2", 10));
        assert_eq!(library.unique_name("Dialogue"), "Dialogue 3");
    }

    #[test]
    fn only_stream_follows_the_playhead() {
        assert!(SoundSync::Stream.follows_timeline());
        assert!(!SoundSync::Event.follows_timeline());
        assert!(!SoundSync::Start.follows_timeline());
        assert!(!SoundSync::Stop.follows_timeline());
    }

    #[test]
    fn dialogue_defaults_to_streaming_at_full_volume() {
        let reference = SoundRef::stream(SoundId(3));
        assert_eq!(reference.sync, SoundSync::Stream);
        assert_eq!(reference.volume, 1.0);
        assert_eq!(reference.loops, 1);
    }
}
