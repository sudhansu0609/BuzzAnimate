//! A point on the timeline — a whole frame, or somewhere between two.
//!
//! # Why this exists
//!
//! Nearly everything in this program happens *on* a frame: keyframes are
//! integers, a symbol's own timeline advances in whole frames, and a cache
//! keyed by anything else would never hit. So the model is `u32`-framed
//! throughout, and rightly.
//!
//! **Motion blur is the exception.** A shutter is open for part of a frame, and
//! what it records is the artwork at a succession of instants *between* frames —
//! that is the whole reason the smear exists. Threading a second, fractional
//! time argument through every lookup would have meant either duplicating each
//! one or churning every call site in the editor and the tests to say `as f64`.
//!
//! [`AtTime`] is the third way: the lookups take `impl AtTime`, `u32` and `f64`
//! both satisfy it, and every existing caller keeps saying exactly what it said
//! before. A frame *is* a time — the one at its own start — so there is no
//! second code path to drift, and passing a whole frame is bit-identical to
//! what it was.

/// When something is being asked for: a whole frame, or a fractional time
/// between two of them.
///
/// Implemented for `u32` (a frame) and `f64` (frames, fractionally). Both
/// answer the same two questions: where this is on the continuous timeline, and
/// which whole frame governs it.
pub trait AtTime: Copy {
    /// The position on the timeline, counted in frames. `2.5` is halfway
    /// between frame 2 and frame 3.
    fn as_time(self) -> f64;

    /// The whole frame this falls on — the one whose keyframe governs it, and
    /// the one a cache is keyed by.
    ///
    /// Floored rather than rounded: for the whole of frame 2, up to but not
    /// including frame 3, the governing keyframe is frame 2's.
    fn frame(self) -> u32 {
        let time = self.as_time();
        if time <= 0.0 {
            return 0;
        }
        // Saturating rather than wrapping: a nonsense time from a script must
        // not become a small frame number.
        if time >= u32::MAX as f64 {
            return u32::MAX;
        }
        time.floor() as u32
    }
}

impl AtTime for u32 {
    fn as_time(self) -> f64 {
        self as f64
    }

    fn frame(self) -> u32 {
        self
    }
}

impl AtTime for f64 {
    fn as_time(self) -> f64 {
        // A time that is not a number would silently poison every lookup it
        // reached; frame 0 is the one answer that is always safe.
        if self.is_finite() { self } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_frame_is_the_time_at_its_own_start() {
        assert_eq!(7u32.as_time(), 7.0);
        assert_eq!(7u32.frame(), 7);
    }

    #[test]
    fn a_fractional_time_is_governed_by_the_frame_it_falls_on() {
        assert_eq!(7.0f64.frame(), 7, "exactly on it");
        assert_eq!(7.5f64.frame(), 7, "halfway through it");
        assert_eq!(7.999f64.frame(), 7, "very nearly the next one");
        assert_eq!(8.0f64.frame(), 8, "and then the next one");
    }

    #[test]
    fn a_time_before_the_start_is_frame_zero() {
        assert_eq!((-3.5f64).frame(), 0);
        assert_eq!((-3.5f64).as_time(), -3.5, "the time itself is not clamped");
    }

    #[test]
    fn a_nonsense_time_reads_as_the_start() {
        assert_eq!(f64::NAN.as_time(), 0.0);
        assert_eq!(f64::INFINITY.as_time(), 0.0);
        assert_eq!(f64::NAN.frame(), 0);
    }
}
