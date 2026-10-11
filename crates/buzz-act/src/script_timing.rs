//! **How long a scene takes, worked out from its words.**
//!
//! There is no recording yet, so there is no clock to read. What there is, is a
//! script, and a script has a speaking pace: a line takes about as long to say
//! as it has words, a pause is a pause, and a paragraph of stage direction
//! takes as long as it takes to read. [`Timing`] holds those paces in one
//! struct, because they are the numbers somebody will want to change after the
//! first screening, and a constant buried in the middle of a function is a
//! constant nobody finds.
//!
//! # The rules
//!
//! * **Dialogue:** `max(0.9 s, words × 0.36 s)`, plus 0.35 s for each `...` or
//!   `—` (a hesitation is time spent) and 0.15 s for each `?` or `!`. Then a
//!   0.3 s gap before whatever comes next.
//! * **Pauses:** a beat is 0.8 s, a long silence 2.0 s, a silence or a pause
//!   1.2 s.
//! * **Action:** `chars × 0.05 s`, never under 1.0 s and never over 3.0 s.
//! * **A beat break** (`---`) adds 0.5 s.
//!
//! Everything is sequential: a line starts when the thing before it has
//! finished. Overlapping speech is a thing a recording will say; text cannot.
//!
//! # Frames, not seconds
//!
//! A scene is laid out in frames at the document's rate, with each item's start
//! worked out from the running time in seconds rather than from adding up
//! rounded lengths, so a long scene does not drift by a frame per line.

use crate::screenplay::{Dialogue, Item, Pause, ScriptScene};

/// The paces. [`Timing::default`] is the series' starting point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// Frames per second the schedule is laid out at.
    pub fps: f64,
    /// A spoken line is never shorter than this, however few words.
    pub min_line: f64,
    /// Seconds per word.
    pub per_word: f64,
    /// Extra seconds for each `...`, `…` or `—` in a line.
    pub per_hesitation: f64,
    /// Extra seconds for each `?` or `!` in a line.
    pub per_emphasis: f64,
    /// Silence after a line, before whatever follows.
    pub line_gap: f64,
    pub beat: f64,
    pub long_silence: f64,
    pub silence: f64,
    /// Seconds per character of action.
    pub action_per_char: f64,
    pub action_min: f64,
    pub action_max: f64,
    /// What a `---` adds.
    pub break_extra: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            fps: 24.0,
            min_line: 0.9,
            per_word: 0.36,
            per_hesitation: 0.35,
            per_emphasis: 0.15,
            line_gap: 0.3,
            beat: 0.8,
            long_silence: 2.0,
            silence: 1.2,
            action_per_char: 0.05,
            action_min: 1.0,
            action_max: 3.0,
            break_extra: 0.5,
        }
    }
}

/// How many words a line has: runs of characters with a letter or digit in
/// them, so a lone dash is not a word.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
        .count()
}

impl Timing {
    /// Seconds to say `text`, not counting the gap that follows it.
    pub fn dialogue_seconds(&self, text: &str) -> f64 {
        let words = word_count(text) as f64;
        let hesitations = text.matches("...").count()
            + text.matches('\u{2026}').count()
            + text.matches('\u{2014}').count();
        let emphasis = text.matches(['?', '!']).count();
        (words * self.per_word).max(self.min_line)
            + hesitations as f64 * self.per_hesitation
            + emphasis as f64 * self.per_emphasis
    }

    pub fn pause_seconds(&self, pause: Pause) -> f64 {
        match pause {
            Pause::Beat => self.beat,
            Pause::LongSilence => self.long_silence,
            Pause::Silence => self.silence,
        }
    }

    pub fn action_seconds(&self, text: &str) -> f64 {
        (text.chars().count() as f64 * self.action_per_char).clamp(self.action_min, self.action_max)
    }

    /// Seconds an item holds the scene for, **including** the gap after a
    /// spoken line.
    pub fn item_seconds(&self, item: &Item) -> f64 {
        match item {
            Item::Action(text) => self.action_seconds(text),
            Item::Dialogue(Dialogue { text, .. }) => self.dialogue_seconds(text) + self.line_gap,
            Item::Pause(p) => self.pause_seconds(*p),
            Item::Break => self.break_extra,
            Item::Slugline(_) => 0.0,
        }
    }

    /// A length in seconds as whole frames. At least one, for anything that
    /// takes any time at all.
    pub fn frames(&self, seconds: f64) -> u32 {
        if seconds <= 0.0 {
            return 0;
        }
        ((seconds * self.fps).round() as u32).max(1)
    }

    /// Lay a scene's items out on the timeline.
    pub fn schedule(&self, scene: &ScriptScene) -> Schedule {
        let mut slots = Vec::with_capacity(scene.items.len());
        let mut clock = 0.0f64;
        for (index, item) in scene.items.iter().enumerate() {
            let start = (clock * self.fps).round() as u32;
            // What the item itself occupies: for a line, the saying of it and
            // not the gap, which is silence before the next thing.
            let seconds = match item {
                Item::Dialogue(Dialogue { text, .. }) => self.dialogue_seconds(text),
                other => self.item_seconds(other),
            };
            let frames = self.frames(seconds);
            slots.push(Slot {
                item: index,
                start,
                frames,
            });
            clock += self.item_seconds(item);
        }
        let total = (clock * self.fps).round() as u32;
        let last_end = slots.iter().map(|s| s.start + s.frames).max().unwrap_or(0);
        Schedule {
            slots,
            total_frames: total.max(last_end).max(1),
        }
    }
}

/// Where one item sits on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// Index into [`ScriptScene::items`].
    pub item: usize,
    pub start: u32,
    /// How long the item itself lasts. For a spoken line that is the speaking,
    /// without the gap that follows.
    pub frames: u32,
}

impl Slot {
    pub fn end(&self) -> u32 {
        self.start + self.frames
    }
}

/// A whole scene on the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub slots: Vec<Slot>,
    /// How many frames the scene runs.
    pub total_frames: u32,
}

impl Schedule {
    pub fn seconds(&self, fps: f64) -> f64 {
        self.total_frames as f64 / fps.max(1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screenplay::Screenplay;

    fn t() -> Timing {
        Timing::default()
    }

    #[test]
    fn a_short_line_still_takes_the_minimum() {
        // One word at 0.36 s is under the 0.9 s floor.
        assert!((t().dialogue_seconds("Arre!") - (0.9 + 0.15)).abs() < 1e-9);
        assert!((t().dialogue_seconds("Haan.") - 0.9).abs() < 1e-9);
    }

    #[test]
    fn a_long_line_takes_a_word_each() {
        // Nine words: 9 x 0.36 = 3.24.
        let line = "Main soch raha hoon kuch bada karun aaj bhi";
        assert_eq!(word_count(line), 9);
        assert!((t().dialogue_seconds(line) - 3.24).abs() < 1e-9);
    }

    #[test]
    fn hesitations_and_emphasis_add_time() {
        // "Haan... roz hi jaati hogi." five words (1.8) + one ellipsis.
        let line = "Haan... roz hi jaati hogi.";
        assert_eq!(word_count(line), 5);
        assert!((t().dialogue_seconds(line) - (1.8 + 0.35)).abs() < 1e-9);
        // A question mark: 0.15.
        let q = "Kitne baje jaayegi?";
        assert!((t().dialogue_seconds(q) - (1.08 + 0.15)).abs() < 1e-9);
        // A dash counts as a hesitation and is not a word.
        let d = "Nahi \u{2014} rukho";
        assert_eq!(word_count(d), 2);
        assert!((t().dialogue_seconds(d) - (0.9 + 0.35)).abs() < 1e-9);
    }

    #[test]
    fn pauses_have_their_own_lengths() {
        assert_eq!(t().pause_seconds(Pause::Beat), 0.8);
        assert_eq!(t().pause_seconds(Pause::LongSilence), 2.0);
        assert_eq!(t().pause_seconds(Pause::Silence), 1.2);
    }

    #[test]
    fn action_is_by_its_length_and_clamped() {
        // 4 characters = 0.2 s, floored at 1.0.
        assert_eq!(t().action_seconds("Tin."), 1.0);
        // 40 characters = 2.0 s.
        assert!((t().action_seconds(&"x".repeat(40)) - 2.0).abs() < 1e-9);
        // 200 characters = 10 s, capped at 3.0.
        assert_eq!(t().action_seconds(&"x".repeat(200)), 3.0);
    }

    #[test]
    fn a_break_adds_half_a_second() {
        assert_eq!(t().item_seconds(&Item::Break), 0.5);
        assert_eq!(t().frames(0.5), 12);
    }

    #[test]
    fn the_schedule_runs_items_end_to_end_with_a_gap_after_each_line() {
        let play = Screenplay::parse(
            "## SCENE 1 \u{2014} X\n\nTin roof.\n\n**GUDDU**\nHaan.\n\nBeat.\n\n**MUNNA**\nHaan.\n\n---\n",
        );
        let scene = &play.scenes[0];
        let schedule = t().schedule(scene);
        let starts: Vec<u32> = schedule.slots.iter().map(|s| s.start).collect();
        // Action 1.0 s -> 24; line 0.9 s (+0.3 gap) -> starts 24, next at 24+29=53
        // (clock 2.2 s -> 52.8 -> 53); beat 0.8 s; line; break.
        assert_eq!(starts[0], 0);
        assert_eq!(starts[1], 24);
        assert_eq!(starts[2], 53);
        assert_eq!(schedule.slots[1].frames, 22, "0.9 s of speaking, no gap");
        // 1.0 + 1.2 + 0.8 + 1.2 + 0.5 = 4.7 s.
        assert_eq!(schedule.total_frames, (4.7f64 * 24.0).round() as u32);
        // Slots never overlap.
        for pair in schedule.slots.windows(2) {
            assert!(pair[0].end() <= pair[1].start + 1, "{pair:?}");
        }
    }

    #[test]
    fn a_trailing_gap_is_part_of_the_scene() {
        let play = Screenplay::parse("## SCENE 1 \u{2014} X\n\n**GUDDU**\nHaan.\n");
        let schedule = t().schedule(&play.scenes[0]);
        // 0.9 + 0.3 s.
        assert_eq!(schedule.total_frames, 29);
        assert_eq!(schedule.slots[0].frames, 22);
    }

    #[test]
    fn the_constants_can_be_tuned_in_one_place() {
        let slow = Timing {
            per_word: 0.5,
            ..Timing::default()
        };
        assert!(slow.dialogue_seconds("a b c d e f g h i j") > t().dialogue_seconds("a b c d e f g h i j"));
    }
}
