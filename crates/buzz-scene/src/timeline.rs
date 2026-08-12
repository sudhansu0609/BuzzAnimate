//! Frames and keyframes on a layer.
//!
//! # Animate's frame model, which this reproduces exactly
//!
//! A layer is a row of frames. A **keyframe** holds artwork and occupies every
//! frame up to the next keyframe, so the timeline is a sequence of spans rather
//! than a value per frame. A **blank keyframe** is a keyframe holding nothing —
//! distinct from "no frame at all", because it actively clears what the
//! previous keyframe was showing.
//!
//! Three states a frame can be in, and they are genuinely different:
//!
//! | State | Meaning |
//! |---|---|
//! | Beyond the span | The layer does not exist here; nothing is drawn |
//! | Inside a span | Shows the keyframe that started the span |
//! | A keyframe | Starts a new span, with its own artwork |
//!
//! Getting this wrong is not cosmetic: a `.fla` importer maps directly onto it,
//! and "empty" versus "absent" changes what a mask reveals.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::object::{Object, ObjectId};

/// A keyframe: artwork plus the frame it starts on.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe {
    /// Zero-based frame this keyframe begins at.
    pub start: u32,
    /// Artwork, in paint order. Empty means a blank keyframe.
    pub objects: Arc<Vec<Arc<Object>>>,
    /// Optional frame label, shown in the timeline and used by scripts.
    pub label: Option<String>,
    /// A tween running from here to the next keyframe.
    pub tween: crate::tween::Tween,
}

impl Keyframe {
    pub fn new(start: u32) -> Self {
        Self {
            start,
            objects: Arc::new(Vec::new()),
            label: None,
            tween: crate::tween::Tween::default(),
        }
    }

    /// A keyframe that deliberately shows nothing.
    pub fn is_blank(&self) -> bool {
        self.objects.is_empty()
    }
}

/// What occupies a particular frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameKind {
    /// Past the end of the layer's span.
    Empty,
    /// A keyframe with artwork. Animate draws a filled circle.
    Keyframe,
    /// A keyframe with no artwork. Animate draws a hollow circle.
    BlankKeyframe,
    /// Continues the keyframe before it.
    Span,
    /// The last frame of a span. Animate draws a hollow rectangle.
    SpanEnd,
}

impl FrameKind {
    /// Is there a layer here at all?
    pub fn exists(self) -> bool {
        !matches!(self, Self::Empty)
    }

    /// Does a new keyframe start here?
    pub fn starts_keyframe(self) -> bool {
        matches!(self, Self::Keyframe | Self::BlankKeyframe)
    }
}

/// A tween and the stretch of frames it covers.
///
/// Returned by [`LayerTimeline::tween_span_at`], and the basis of the tinted
/// span with an arrow that the timeline draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TweenSpan {
    pub tween: crate::tween::Tween,
    /// Frame the tween starts on — always a keyframe.
    pub start: u32,
    /// Keyframe the tween runs to. `None` means there is not one, so the tween
    /// is broken and nothing will move.
    pub end: Option<u32>,
}

impl TweenSpan {
    /// Does this tween have somewhere to go?
    pub fn is_complete(&self) -> bool {
        self.end.is_some()
    }

    /// The last frame the tween covers, which is where the arrowhead goes.
    pub fn last_frame(&self, layer_length: u32) -> u32 {
        match self.end {
            Some(end) => end.saturating_sub(1),
            None => layer_length.saturating_sub(1),
        }
    }
}

/// What a frame resolves to once tweening is taken into account.
///
/// Borrowed when untweened so the common path costs nothing; owned when a
/// tween had to synthesise the state.
#[derive(Debug)]
pub enum ResolvedFrame<'a> {
    Stored(&'a [Arc<Object>]),
    Tweened(Vec<Object>),
}

impl ResolvedFrame<'_> {
    pub fn len(&self) -> usize {
        match self {
            Self::Stored(objects) => objects.len(),
            Self::Tweened(objects) => objects.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterate whichever representation this is.
    pub fn iter(&self) -> Box<dyn Iterator<Item = &Object> + '_> {
        match self {
            Self::Stored(objects) => Box::new(objects.iter().map(|o| &**o)),
            Self::Tweened(objects) => Box::new(objects.iter()),
        }
    }
}

/// A layer's frames.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerTimeline {
    /// Sorted by `start`, always non-empty, always beginning at frame 0.
    keyframes: Vec<Keyframe>,
    /// Number of frames the layer occupies. At least 1.
    length: u32,
}

impl Default for LayerTimeline {
    fn default() -> Self {
        // Animate gives a new layer one blank keyframe at frame 1.
        Self {
            keyframes: vec![Keyframe::new(0)],
            length: 1,
        }
    }
}

impl LayerTimeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames occupied, at least 1.
    pub fn length(&self) -> u32 {
        self.length
    }

    pub fn keyframes(&self) -> &[Keyframe] {
        &self.keyframes
    }

    pub fn keyframe_count(&self) -> usize {
        self.keyframes.len()
    }

    /// Index of the keyframe governing `frame`, if the layer reaches it.
    fn index_at(&self, frame: u32) -> Option<usize> {
        if frame >= self.length {
            return None;
        }
        // `partition_point` finds the first keyframe starting after `frame`.
        let after = self.keyframes.partition_point(|k| k.start <= frame);
        after.checked_sub(1)
    }

    /// The keyframe shown at `frame`.
    pub fn keyframe_at(&self, frame: u32) -> Option<&Keyframe> {
        self.index_at(frame).and_then(|i| self.keyframes.get(i))
    }

    /// Artwork shown at `frame`. Empty beyond the span or on a blank keyframe.
    ///
    /// **Untweened**: this is the keyframe's stored artwork. Use
    /// [`Self::resolved_at`] for what should actually be drawn.
    pub fn objects_at(&self, frame: u32) -> &[Arc<Object>] {
        match self.keyframe_at(frame) {
            Some(k) => &k.objects,
            None => &[],
        }
    }

    /// Artwork to draw at `frame`, with any tween applied.
    ///
    /// Returns borrowed objects when there is no tween, so the common case
    /// allocates nothing; a tween has to build new objects because the
    /// interpolated state does not exist anywhere in the document.
    pub fn resolved_at(&self, frame: u32) -> ResolvedFrame<'_> {
        let Some(index) = self.index_at(frame) else {
            return ResolvedFrame::Stored(&[]);
        };
        let keyframe = &self.keyframes[index];

        if !keyframe.tween.is_active() {
            return ResolvedFrame::Stored(&keyframe.objects);
        }

        // The tween runs to the next keyframe, or to the end of the span if
        // this is the last one.
        let Some(next) = self.keyframes.get(index + 1) else {
            return ResolvedFrame::Stored(&keyframe.objects);
        };

        let span = next.start.saturating_sub(keyframe.start);
        if span == 0 {
            return ResolvedFrame::Stored(&keyframe.objects);
        }
        let progress = (frame - keyframe.start) as f64 / span as f64;
        if progress <= 0.0 {
            return ResolvedFrame::Stored(&keyframe.objects);
        }

        ResolvedFrame::Tweened(crate::tween::interpolate_objects(
            &keyframe.objects,
            &next.objects,
            &keyframe.tween,
            progress,
        ))
    }

    /// Set the tween on the keyframe governing `frame`.
    pub fn set_tween(&mut self, frame: u32, tween: crate::tween::Tween) -> bool {
        match self.keyframe_at_mut(frame) {
            Some(k) => {
                k.tween = tween;
                true
            }
            None => false,
        }
    }

    /// The tween on the keyframe governing `frame`.
    pub fn tween_at(&self, frame: u32) -> crate::tween::Tween {
        self.keyframe_at(frame)
            .map(|k| k.tween)
            .unwrap_or_default()
    }

    /// The tween governing `frame`, together with the frames it runs between.
    ///
    /// Returns `None` where no tween is set. The `end` is `None` when the
    /// tween has no following keyframe — it is *broken*, interpolating towards
    /// nothing, and [`Self::resolved_at`] holds the keyframe instead of
    /// animating. That case is worth distinguishing rather than hiding,
    /// because it is the usual reason a tween that was just created does not
    /// appear to do anything; Animate draws it as a dashed line.
    pub fn tween_span_at(&self, frame: u32) -> Option<TweenSpan> {
        let index = self.index_at(frame)?;
        let keyframe = &self.keyframes[index];
        if !keyframe.tween.is_active() {
            return None;
        }
        Some(TweenSpan {
            tween: keyframe.tween,
            start: keyframe.start,
            end: self.keyframes.get(index + 1).map(|k| k.start),
        })
    }

    /// What the timeline draws at `frame`.
    pub fn frame_kind(&self, frame: u32) -> FrameKind {
        if frame >= self.length {
            return FrameKind::Empty;
        }
        if let Some(k) = self.keyframes.iter().find(|k| k.start == frame) {
            return if k.is_blank() {
                FrameKind::BlankKeyframe
            } else {
                FrameKind::Keyframe
            };
        }
        if frame + 1 == self.length {
            FrameKind::SpanEnd
        } else {
            FrameKind::Span
        }
    }

    /// Is `frame` the start of a keyframe?
    pub fn is_keyframe(&self, frame: u32) -> bool {
        self.keyframes.iter().any(|k| k.start == frame)
    }

    /// Frame the keyframe governing `frame` starts on.
    pub fn keyframe_start(&self, frame: u32) -> Option<u32> {
        self.keyframe_at(frame).map(|k| k.start)
    }

    /// Mutable access to the keyframe governing `frame`.
    ///
    /// Edits land on the keyframe that owns the frame, so drawing on frame 7
    /// of a span that began at frame 5 modifies frame 5's artwork — which is
    /// what Animate does.
    pub fn keyframe_at_mut(&mut self, frame: u32) -> Option<&mut Keyframe> {
        let index = self.index_at(frame)?;
        self.keyframes.get_mut(index)
    }

    // -- editing operations, matching Animate's shortcuts -------------------

    /// **F5** — extend the layer so `frame` exists.
    ///
    /// Returns false if it already did.
    pub fn insert_frame(&mut self, frame: u32) -> bool {
        let needed = frame.saturating_add(1);
        if needed <= self.length {
            // Inserting inside the span pushes everything after it along.
            self.shift_keyframes_from(frame, 1);
            self.length += 1;
            return true;
        }
        self.length = needed;
        true
    }

    /// **Shift+F5** — remove `frame`, pulling later frames back.
    pub fn remove_frame(&mut self, frame: u32) -> bool {
        if frame >= self.length || self.length <= 1 {
            return false;
        }
        // A keyframe sitting exactly here goes with the frame.
        self.keyframes.retain(|k| k.start != frame || k.start == 0);
        self.shift_keyframes_from(frame + 1, -1);
        self.length -= 1;
        true
    }

    /// **F6** — insert a keyframe at `frame`, carrying the previous artwork.
    ///
    /// Duplicating the content is the point: F6 is how you make a copy to
    /// modify, whereas F7 starts from nothing.
    pub fn insert_keyframe(&mut self, frame: u32) -> bool {
        if self.is_keyframe(frame) {
            return false;
        }
        let objects = self
            .keyframe_at(frame)
            .map(|k| Arc::clone(&k.objects))
            .unwrap_or_default();

        self.length = self.length.max(frame + 1);
        self.push_keyframe(Keyframe {
            start: frame,
            objects,
            label: None,
            tween: crate::tween::Tween::default(),
        });
        true
    }

    /// **F7** — insert an empty keyframe at `frame`.
    pub fn insert_blank_keyframe(&mut self, frame: u32) -> bool {
        if self.is_keyframe(frame) {
            // Turning an existing keyframe blank is still a useful action.
            if let Some(k) = self.keyframes.iter_mut().find(|k| k.start == frame) {
                k.objects = Arc::new(Vec::new());
                return true;
            }
            return false;
        }
        self.length = self.length.max(frame + 1);
        self.push_keyframe(Keyframe::new(frame));
        true
    }

    /// **Shift+F6** — remove the keyframe at `frame`.
    ///
    /// Its frames merge into the preceding keyframe's span. Frame 0's keyframe
    /// cannot be removed; a layer must always start with one.
    pub fn clear_keyframe(&mut self, frame: u32) -> bool {
        if frame == 0 || !self.is_keyframe(frame) {
            return false;
        }
        self.keyframes.retain(|k| k.start != frame);
        true
    }

    /// Set the artwork of the keyframe governing `frame`.
    pub fn set_objects(&mut self, frame: u32, objects: Vec<Arc<Object>>) -> bool {
        match self.keyframe_at_mut(frame) {
            Some(k) => {
                k.objects = Arc::new(objects);
                true
            }
            None => false,
        }
    }

    /// Add an object to the keyframe governing `frame`.
    pub fn push_object(&mut self, frame: u32, object: Arc<Object>) -> bool {
        match self.keyframe_at_mut(frame) {
            Some(k) => {
                Arc::make_mut(&mut k.objects).push(object);
                true
            }
            None => false,
        }
    }

    /// Remove an object from wherever it appears.
    pub fn remove_object(&mut self, id: ObjectId) -> Option<Arc<Object>> {
        for keyframe in &mut self.keyframes {
            if let Some(index) = keyframe.objects.iter().position(|o| o.id == id) {
                return Some(Arc::make_mut(&mut keyframe.objects).remove(index));
            }
        }
        None
    }

    /// Every object on the layer, across all keyframes.
    pub fn all_objects(&self) -> impl Iterator<Item = &Arc<Object>> {
        self.keyframes.iter().flat_map(|k| k.objects.iter())
    }

    /// Mutable access to every keyframe.
    ///
    /// Used to edit an object wherever it lives without first knowing its
    /// frame. Changing a keyframe's `start` through this would break the sorted
    /// invariant, so don't — use the insert and clear operations instead.
    pub fn keyframes_mut(&mut self) -> &mut [Keyframe] {
        &mut self.keyframes
    }

    /// Mutable artwork of the keyframe governing `frame`.
    pub fn objects_at_mut(&mut self, frame: u32) -> Option<&mut Vec<Arc<Object>>> {
        self.keyframe_at_mut(frame)
            .map(|k| Arc::make_mut(&mut k.objects))
    }

    /// Set the frame label for the keyframe governing `frame`.
    pub fn set_label(&mut self, frame: u32, label: Option<String>) -> bool {
        match self.keyframe_at_mut(frame) {
            Some(k) => {
                k.label = label;
                true
            }
            None => false,
        }
    }

    /// Rebuild from parts, for the importer and the loader.
    pub fn from_parts(mut keyframes: Vec<Keyframe>, length: u32) -> Self {
        keyframes.sort_by_key(|k| k.start);
        keyframes.dedup_by_key(|k| k.start);
        // A layer must begin with a keyframe at frame 0, or `objects_at` would
        // have nothing to show for the opening frames.
        if keyframes.first().map(|k| k.start) != Some(0) {
            keyframes.insert(0, Keyframe::new(0));
        }
        let highest = keyframes.last().map(|k| k.start).unwrap_or(0);
        Self {
            keyframes,
            length: length.max(highest + 1).max(1),
        }
    }

    fn push_keyframe(&mut self, keyframe: Keyframe) {
        let at = self.keyframes.partition_point(|k| k.start < keyframe.start);
        self.keyframes.insert(at, keyframe);
    }

    /// Move keyframes at or after `from` by `delta` frames.
    fn shift_keyframes_from(&mut self, from: u32, delta: i64) {
        for keyframe in &mut self.keyframes {
            if keyframe.start >= from && keyframe.start > 0 {
                let shifted = keyframe.start as i64 + delta;
                keyframe.start = shifted.max(1) as u32;
            }
        }
        self.keyframes.sort_by_key(|k| k.start);
        self.keyframes.dedup_by_key(|k| k.start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::ShapeData;
    use buzz_geom::Shape as _;
    use kurbo::Rect;
    use peniko::Color;

    fn object(id: u64) -> Arc<Object> {
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
        ))
    }

    #[test]
    fn a_new_layer_has_one_blank_keyframe_like_animate() {
        let t = LayerTimeline::new();
        assert_eq!(t.length(), 1);
        assert_eq!(t.keyframe_count(), 1);
        assert_eq!(t.frame_kind(0), FrameKind::BlankKeyframe);
        assert_eq!(t.frame_kind(1), FrameKind::Empty);
    }

    #[test]
    fn a_keyframe_governs_every_frame_until_the_next() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(9);
        // A *blank* keyframe, so the two spans have unmistakably different
        // content. F6 would have copied frame 0's artwork here.
        t.insert_blank_keyframe(5);
        t.push_object(5, object(2));

        // Frames 0..4 show the first keyframe, 5..9 the second.
        assert_eq!(t.objects_at(0).len(), 1);
        assert_eq!(t.objects_at(4)[0].id, ObjectId(1));
        assert_eq!(t.objects_at(5).len(), 1);
        assert_eq!(t.objects_at(5)[0].id, ObjectId(2));
        assert_eq!(t.objects_at(9)[0].id, ObjectId(2));
        assert!(t.objects_at(10).is_empty(), "past the span");
    }

    /// The distinction that matters: nothing there versus deliberately empty.
    #[test]
    fn a_blank_keyframe_is_not_the_same_as_no_frame() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(9);
        t.insert_blank_keyframe(5);

        assert_eq!(t.frame_kind(5), FrameKind::BlankKeyframe);
        assert!(t.frame_kind(5).exists(), "the frame exists");
        assert!(t.objects_at(5).is_empty(), "but shows nothing");

        assert_eq!(t.frame_kind(20), FrameKind::Empty);
        assert!(!t.frame_kind(20).exists(), "this one does not exist at all");
    }

    #[test]
    fn frame_kinds_match_animates_drawing_conventions() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(4);

        assert_eq!(t.frame_kind(0), FrameKind::Keyframe, "filled circle");
        assert_eq!(t.frame_kind(1), FrameKind::Span);
        assert_eq!(t.frame_kind(4), FrameKind::SpanEnd, "hollow rectangle");
        assert_eq!(t.frame_kind(5), FrameKind::Empty);
    }

    /// F6 carries the artwork forward; F7 does not.
    #[test]
    fn f6_duplicates_content_and_f7_starts_empty() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(9);

        assert!(t.insert_keyframe(3));
        assert_eq!(t.objects_at(3).len(), 1, "F6 should carry the artwork");

        assert!(t.insert_blank_keyframe(6));
        assert!(t.objects_at(6).is_empty(), "F7 should start blank");
    }

    #[test]
    fn f6_on_an_existing_keyframe_does_nothing() {
        let mut t = LayerTimeline::new();
        assert!(!t.insert_keyframe(0), "frame 0 is already a keyframe");
        assert_eq!(t.keyframe_count(), 1);
    }

    #[test]
    fn f5_extends_the_span() {
        let mut t = LayerTimeline::new();
        assert_eq!(t.length(), 1);
        t.insert_frame(9);
        assert_eq!(t.length(), 10);
        assert_eq!(t.frame_kind(9), FrameKind::SpanEnd);
    }

    #[test]
    fn inserting_a_frame_inside_a_span_pushes_later_keyframes_along() {
        let mut t = LayerTimeline::new();
        t.insert_frame(9);
        t.insert_keyframe(5);
        assert!(t.is_keyframe(5));

        t.insert_frame(2);
        assert!(!t.is_keyframe(5), "the keyframe should have moved");
        assert!(t.is_keyframe(6));
        assert_eq!(t.length(), 11);
    }

    #[test]
    fn removing_a_frame_pulls_later_frames_back() {
        let mut t = LayerTimeline::new();
        t.insert_frame(9);
        t.insert_keyframe(5);

        assert!(t.remove_frame(2));
        assert_eq!(t.length(), 9);
        assert!(t.is_keyframe(4), "the keyframe should have shifted back");
    }

    #[test]
    fn shift_f6_merges_a_keyframe_into_the_one_before() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(9);
        t.insert_blank_keyframe(5);
        assert!(t.objects_at(5).is_empty());

        assert!(t.clear_keyframe(5));
        assert_eq!(
            t.objects_at(5).len(),
            1,
            "frame 5 should now show the earlier keyframe"
        );
    }

    /// A layer must always start with a keyframe, or early frames would have
    /// nothing to display.
    #[test]
    fn the_first_keyframe_cannot_be_removed() {
        let mut t = LayerTimeline::new();
        t.insert_frame(9);
        assert!(!t.clear_keyframe(0));
        assert!(t.is_keyframe(0));

        assert!(!t.remove_frame(0) || t.is_keyframe(0));
    }

    #[test]
    fn a_layer_never_shrinks_below_one_frame() {
        let mut t = LayerTimeline::new();
        assert!(!t.remove_frame(0), "the only frame cannot be removed");
        assert_eq!(t.length(), 1);
    }

    #[test]
    fn drawing_inside_a_span_edits_the_keyframe_that_owns_it() {
        let mut t = LayerTimeline::new();
        t.insert_frame(9);
        // Frame 7 is inside the span starting at 0.
        assert!(t.push_object(7, object(42)));
        assert_eq!(t.objects_at(0).len(), 1, "the edit landed on frame 0");
        assert_eq!(t.objects_at(7)[0].id, ObjectId(42));
    }

    #[test]
    fn objects_can_be_removed_from_any_keyframe() {
        let mut t = LayerTimeline::new();
        t.insert_frame(9);
        t.insert_keyframe(5);
        t.push_object(0, object(1));
        t.push_object(5, object(2));

        assert!(t.remove_object(ObjectId(2)).is_some());
        assert!(t.objects_at(5).is_empty());
        assert_eq!(t.all_objects().count(), 1);
        assert!(t.remove_object(ObjectId(999)).is_none());
    }

    #[test]
    fn frame_labels_round_trip() {
        let mut t = LayerTimeline::new();
        assert!(t.set_label(0, Some("intro".into())));
        assert_eq!(t.keyframe_at(0).unwrap().label.as_deref(), Some("intro"));
    }

    /// A malformed file must not produce a timeline with no opening keyframe.
    #[test]
    fn rebuilding_repairs_a_missing_first_keyframe() {
        let t = LayerTimeline::from_parts(vec![Keyframe::new(5), Keyframe::new(9)], 12);
        assert!(t.is_keyframe(0), "a frame-0 keyframe should be inserted");
        assert_eq!(t.length(), 12);
        assert!(t.objects_at(0).is_empty());
    }

    #[test]
    fn rebuilding_sorts_and_deduplicates() {
        let t = LayerTimeline::from_parts(
            vec![Keyframe::new(7), Keyframe::new(0), Keyframe::new(7)],
            3,
        );
        let starts: Vec<u32> = t.keyframes().iter().map(|k| k.start).collect();
        assert_eq!(starts, vec![0, 7]);
        assert_eq!(t.length(), 8, "length must cover the last keyframe");
    }

    #[test]
    fn lookups_are_fast_on_a_long_timeline() {
        let keys: Vec<Keyframe> = (0..2000).map(|i| Keyframe::new(i * 5)).collect();
        let t = LayerTimeline::from_parts(keys, 10_000);

        let started = std::time::Instant::now();
        for frame in 0..10_000u32 {
            let _ = t.objects_at(frame);
        }
        assert!(
            started.elapsed().as_millis() < 200,
            "10k lookups took {:?}; the search should be binary",
            started.elapsed()
        );
    }

    /// A tween covers every frame its keyframe governs, and stops at the next
    /// keyframe — which is exactly the stretch the timeline tints.
    #[test]
    fn a_tween_span_runs_from_its_keyframe_to_the_next() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(19);
        t.insert_keyframe(10);
        assert!(t.set_tween(0, crate::tween::Tween::classic()));

        for frame in 0..10 {
            let span = t.tween_span_at(frame).unwrap_or_else(|| {
                panic!("frame {frame} is inside the tweened span");
            });
            assert_eq!(span.start, 0);
            assert_eq!(span.end, Some(10));
            assert!(span.is_complete());
            assert_eq!(span.last_frame(t.length()), 9, "the arrow goes on frame 9");
        }

        assert!(
            t.tween_span_at(10).is_none(),
            "the second keyframe carries no tween of its own"
        );
    }

    /// A tween with no following keyframe interpolates towards nothing. The
    /// model has to say so, because that is the difference between a dashed
    /// line and a working animation.
    #[test]
    fn a_tween_with_no_next_keyframe_is_reported_as_broken() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(9);
        assert!(t.set_tween(0, crate::tween::Tween::motion()));

        let span = t.tween_span_at(4).expect("the tween is set");
        assert_eq!(span.end, None);
        assert!(!span.is_complete());
        assert_eq!(span.last_frame(t.length()), t.length() - 1);

        // And it draws the keyframe unchanged rather than inventing motion.
        assert!(matches!(t.resolved_at(4), ResolvedFrame::Stored(_)));
    }

    #[test]
    fn frames_beyond_the_layer_carry_no_tween() {
        let mut t = LayerTimeline::new();
        t.push_object(0, object(1));
        t.insert_frame(4);
        t.set_tween(0, crate::tween::Tween::shape());

        assert!(t.tween_span_at(4).is_some());
        assert!(
            t.tween_span_at(500).is_none(),
            "a frame the layer does not reach cannot be tweened"
        );
    }
}
