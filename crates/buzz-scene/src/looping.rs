//! A section of the timeline that repeats — in playback **and** in the render.
//!
//! # Why this is not just a preview setting
//!
//! Animate's loop is a preview: the playhead cycles between two markers while
//! you work, and the published file knows nothing about it. That is the right
//! default for checking a walk cycle, and useless for the thing animators
//! actually want next — a background that loops eight times behind a scene, a
//! flickering light, a two-frame flag that flutters for the whole shot —
//! without duplicating the frames by hand.
//!
//! So this loop is part of the **document**. Playback cycles it, and an export
//! *repeats* it: the finished film contains the section as many times as asked
//! for. The timeline stays as short as the drawing actually is, and the length
//! of the film is the length of the film.

use serde::{Deserialize, Serialize};

/// The looping section of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopRegion {
    /// Off by default, so a document that does not use it behaves exactly as
    /// it always did.
    pub enabled: bool,
    /// First frame of the section, inclusive.
    pub start: u32,
    /// Last frame, inclusive — as an animator counts a range, and as the
    /// timeline shows it.
    pub end: u32,
    /// How many times the section plays **in total**. One is no repeat at all.
    pub repeats: u32,
}

impl Default for LoopRegion {
    fn default() -> Self {
        Self {
            enabled: false,
            start: 0,
            end: 0,
            repeats: 2,
        }
    }
}

/// The most a section may repeat.
///
/// Bounded because the playlist is materialised: a hundred thousand repeats of
/// a long section is a vector nobody meant to ask for, and the number is a
/// typed field.
pub const MAX_REPEATS: u32 = 999;

impl LoopRegion {
    /// Is this a region that would actually do something?
    pub fn is_active(&self) -> bool {
        self.enabled && self.end >= self.start && self.repeats > 1
    }

    /// How many frames the section covers.
    pub fn length(&self) -> u32 {
        if self.end >= self.start {
            self.end - self.start + 1
        } else {
            0
        }
    }

    /// Does this frame fall inside the section?
    pub fn contains(&self, frame: u32) -> bool {
        self.enabled && frame >= self.start && frame <= self.end
    }

    /// The next frame during playback, cycling within the section.
    ///
    /// Returns `None` when the region has nothing to say about this frame, so
    /// the caller carries on with its ordinary rule.
    pub fn wrap(&self, frame: u32) -> Option<u32> {
        if !self.enabled || self.end < self.start {
            return None;
        }
        (frame > self.end && frame > 0).then_some(self.start)
    }

    /// Brought into range for a document `frames` long.
    pub fn clamped(mut self, frames: u32) -> Self {
        let last = frames.saturating_sub(1);
        self.start = self.start.min(last);
        self.end = self.end.clamp(self.start, last);
        self.repeats = self.repeats.clamp(1, MAX_REPEATS);
        self
    }

    /// The document frame to draw for each frame of the finished film.
    ///
    /// Without a region this is simply every frame once. With one, the section
    /// appears as many times as asked for, and everything before and after it
    /// appears exactly once.
    pub fn playlist(&self, frames: u32) -> Vec<u32> {
        let frames = frames.max(1);
        if !self.is_active() {
            return (0..frames).collect();
        }

        let region = self.clamped(frames);
        let mut out = Vec::with_capacity((frames + region.length() * region.repeats) as usize);

        out.extend(0..region.start);
        for _ in 0..region.repeats {
            out.extend(region.start..=region.end);
        }
        out.extend((region.end + 1)..frames);
        out
    }

    /// How long the finished film is, in frames.
    pub fn rendered_length(&self, frames: u32) -> u32 {
        let frames = frames.max(1);
        if !self.is_active() {
            return frames;
        }
        let region = self.clamped(frames);
        frames + region.length() * (region.repeats - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_without_a_region_plays_straight_through() {
        let region = LoopRegion::default();
        assert!(!region.is_active());
        assert_eq!(region.playlist(5), vec![0, 1, 2, 3, 4]);
        assert_eq!(region.rendered_length(5), 5);
    }

    /// The point of the whole thing: the section appears in the film as many
    /// times as it was asked for.
    #[test]
    fn a_region_repeats_in_the_playlist() {
        let region = LoopRegion {
            enabled: true,
            start: 2,
            end: 4,
            repeats: 3,
        };
        assert_eq!(
            region.playlist(7),
            vec![0, 1, 2, 3, 4, 2, 3, 4, 2, 3, 4, 5, 6]
        );
        assert_eq!(region.rendered_length(7), 13);
        assert_eq!(region.playlist(7).len() as u32, region.rendered_length(7));
    }

    /// One repeat is no repeat: the film is unchanged, and the region is not
    /// "active" even though it is switched on.
    #[test]
    fn a_single_repeat_changes_nothing() {
        let region = LoopRegion {
            enabled: true,
            start: 1,
            end: 3,
            repeats: 1,
        };
        assert!(!region.is_active());
        assert_eq!(region.playlist(5), vec![0, 1, 2, 3, 4]);
    }

    /// A region covering the whole timeline just repeats the whole timeline.
    #[test]
    fn a_region_covering_everything_repeats_everything() {
        let region = LoopRegion {
            enabled: true,
            start: 0,
            end: 2,
            repeats: 2,
        };
        assert_eq!(region.playlist(3), vec![0, 1, 2, 0, 1, 2]);
    }

    /// A region left over from a longer document must not read past the end.
    #[test]
    fn a_region_past_the_end_is_brought_back() {
        let region = LoopRegion {
            enabled: true,
            start: 40,
            end: 90,
            repeats: 3,
        };
        let playlist = region.playlist(5);
        assert!(
            playlist.iter().all(|f| *f < 5),
            "read past the end: {playlist:?}"
        );
        assert!(!playlist.is_empty());
    }

    /// An inverted range is not a range, and must not produce an empty film.
    #[test]
    fn an_inverted_region_is_ignored() {
        let region = LoopRegion {
            enabled: true,
            start: 6,
            end: 2,
            repeats: 4,
        };
        assert!(!region.is_active());
        assert_eq!(region.playlist(8).len(), 8);
    }

    /// Playback cycles within the section rather than running to the end.
    #[test]
    fn playback_wraps_at_the_end_of_the_section() {
        let region = LoopRegion {
            enabled: true,
            start: 3,
            end: 6,
            repeats: 2,
        };
        assert_eq!(
            region.wrap(7),
            Some(3),
            "past the end goes back to the start"
        );
        assert_eq!(region.wrap(5), None, "inside the section, carry on");
        assert_eq!(region.wrap(1), None, "before it, carry on");
    }

    /// A switched-off region says nothing about playback at all.
    #[test]
    fn a_disabled_region_does_not_wrap() {
        let region = LoopRegion {
            enabled: false,
            start: 3,
            end: 6,
            repeats: 4,
        };
        assert_eq!(region.wrap(9), None);
    }

    #[test]
    fn repeats_are_bounded() {
        let region = LoopRegion {
            enabled: true,
            start: 0,
            end: 1,
            repeats: u32::MAX,
        }
        .clamped(10);
        assert!(region.repeats <= MAX_REPEATS);
    }

    /// The length reported and the playlist produced must agree, whatever the
    /// numbers — the exporter sizes its progress bar on one and iterates the
    /// other.
    #[test]
    fn the_reported_length_matches_the_playlist() {
        for (start, end, repeats, frames) in [
            (0, 0, 2, 1),
            (0, 3, 5, 10),
            (7, 9, 2, 10),
            (2, 2, 9, 4),
            (0, 9, 3, 10),
        ] {
            let region = LoopRegion {
                enabled: true,
                start,
                end,
                repeats,
            };
            assert_eq!(
                region.playlist(frames).len() as u32,
                region.rendered_length(frames),
                "{start}..={end} x{repeats} over {frames}"
            );
        }
    }
}
