//! Align, distribute and match size — Animate's Align panel, as arithmetic.
//!
//! # Why this is a module of pure functions
//!
//! Laying out a stage is the part of the work that was reported as slow, and
//! it is also the part that is easiest to get subtly wrong: "distribute
//! evenly" means two different things depending on whether you are spacing
//! *centres* or *gaps*, and the difference only shows when the objects are
//! different sizes. So the arithmetic is here, taking rectangles and returning
//! offsets, where it can be checked against worked examples without a
//! document, a selection or a GPU.
//!
//! Everything returns **offsets**, one per input rectangle, in the order they
//! were given. The caller translates by them. Nothing here knows what an
//! object is.

use buzz_geom::Vec2;
use kurbo::Rect;

/// Which edges or centres to line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    LeftEdges,
    HorizontalCentres,
    RightEdges,
    TopEdges,
    VerticalCentres,
    BottomEdges,
}

/// How to spread things out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Distribute {
    /// Equal distance between centres, ignoring how wide each one is.
    HorizontalCentres,
    VerticalCentres,
    /// Equal *gaps*, which is what the eye reads as evenly spaced when the
    /// objects are different sizes.
    HorizontalSpacing,
    VerticalSpacing,
}

/// Make everything the same size as the largest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchSize {
    Width,
    Height,
    Both,
}

impl Align {
    pub fn label(self) -> &'static str {
        match self {
            Self::LeftEdges => "Left Edges",
            Self::HorizontalCentres => "Horizontal Centre",
            Self::RightEdges => "Right Edges",
            Self::TopEdges => "Top Edges",
            Self::VerticalCentres => "Vertical Centre",
            Self::BottomEdges => "Bottom Edges",
        }
    }

    pub const ALL: [Align; 6] = [
        Self::LeftEdges,
        Self::HorizontalCentres,
        Self::RightEdges,
        Self::TopEdges,
        Self::VerticalCentres,
        Self::BottomEdges,
    ];
}

impl Distribute {
    pub fn label(self) -> &'static str {
        match self {
            Self::HorizontalCentres => "Horizontal Centres",
            Self::VerticalCentres => "Vertical Centres",
            Self::HorizontalSpacing => "Horizontal Spacing",
            Self::VerticalSpacing => "Vertical Spacing",
        }
    }

    pub const ALL: [Distribute; 4] = [
        Self::HorizontalCentres,
        Self::VerticalCentres,
        Self::HorizontalSpacing,
        Self::VerticalSpacing,
    ];
}

impl MatchSize {
    pub fn label(self) -> &'static str {
        match self {
            Self::Width => "Width",
            Self::Height => "Height",
            Self::Both => "Width and Height",
        }
    }

    pub const ALL: [MatchSize; 3] = [Self::Width, Self::Height, Self::Both];
}

/// The rectangle everything is lined up against.
///
/// Animate's Align panel has an "Align to stage" switch, and it changes what
/// the operation *means*: without it, things move towards each other; with it,
/// they move towards the frame. Both are wanted often enough that the switch
/// is the feature.
fn frame(bounds: &[Rect], to_stage: Option<Rect>) -> Option<Rect> {
    match to_stage {
        Some(stage) => Some(stage),
        // The union of what is selected. One object aligned to itself is a
        // no-op, which is correct and is why this needs no special case.
        None => bounds
            .iter()
            .copied()
            .reduce(|a, b| a.union(b))
            .filter(|_| !bounds.is_empty()),
    }
}

/// Offsets that line every rectangle up.
pub fn align_offsets(bounds: &[Rect], op: Align, to_stage: Option<Rect>) -> Vec<Vec2> {
    let Some(frame) = frame(bounds, to_stage) else {
        return Vec::new();
    };
    bounds
        .iter()
        .map(|r| match op {
            Align::LeftEdges => Vec2::new(frame.x0 - r.x0, 0.0),
            Align::RightEdges => Vec2::new(frame.x1 - r.x1, 0.0),
            Align::HorizontalCentres => Vec2::new(frame.center().x - r.center().x, 0.0),
            Align::TopEdges => Vec2::new(0.0, frame.y0 - r.y0),
            Align::BottomEdges => Vec2::new(0.0, frame.y1 - r.y1),
            Align::VerticalCentres => Vec2::new(0.0, frame.center().y - r.center().y),
        })
        .collect()
}

/// Offsets that spread every rectangle out evenly.
///
/// **The two outermost do not move.** Distributing is about what is between
/// them, and an operation that also slid the ends would be a different one —
/// it would make the row narrower or wider, which is not what was asked for.
/// With fewer than three rectangles there is nothing between anything, so
/// every offset is zero.
pub fn distribute_offsets(bounds: &[Rect], op: Distribute) -> Vec<Vec2> {
    let mut out = vec![Vec2::ZERO; bounds.len()];
    if bounds.len() < 3 {
        return out;
    }

    let horizontal = matches!(
        op,
        Distribute::HorizontalCentres | Distribute::HorizontalSpacing
    );
    // Work in one axis at a time; the other is untouched.
    let low = |r: &Rect| if horizontal { r.x0 } else { r.y0 };
    let high = |r: &Rect| if horizontal { r.x1 } else { r.y1 };
    let mid = |r: &Rect| {
        if horizontal {
            r.center().x
        } else {
            r.center().y
        }
    };

    // Sorted by position, so the answer does not depend on the order things
    // happened to be selected in.
    let mut order: Vec<usize> = (0..bounds.len()).collect();
    order.sort_by(|&a, &b| mid(&bounds[a]).total_cmp(&mid(&bounds[b])));

    let first = order[0];
    let last = order[order.len() - 1];

    let targets: Vec<f64> = match op {
        Distribute::HorizontalCentres | Distribute::VerticalCentres => {
            let start = mid(&bounds[first]);
            let end = mid(&bounds[last]);
            let step = (end - start) / (order.len() - 1) as f64;
            (0..order.len()).map(|i| start + step * i as f64).collect()
        }
        Distribute::HorizontalSpacing | Distribute::VerticalSpacing => {
            // Equal gaps: the room left over once every object has taken its
            // own size, shared between the spaces between them.
            let span = high(&bounds[last]) - low(&bounds[first]);
            let filled: f64 = order.iter().map(|&i| high(&bounds[i]) - low(&bounds[i])).sum();
            let gap = (span - filled) / (order.len() - 1) as f64;
            let mut at = low(&bounds[first]);
            let mut targets = Vec::with_capacity(order.len());
            for &i in &order {
                let size = high(&bounds[i]) - low(&bounds[i]);
                targets.push(at + size / 2.0);
                at += size + gap;
            }
            targets
        }
    };

    for (slot, &index) in order.iter().enumerate() {
        let delta = targets[slot] - mid(&bounds[index]);
        out[index] = if horizontal {
            Vec2::new(delta, 0.0)
        } else {
            Vec2::new(0.0, delta)
        };
    }
    out
}

/// Scale factors that make everything the size of the largest.
///
/// Scaling is **about each rectangle's own centre**, so nothing wanders across
/// the stage while being resized. Returned as `(x, y)` multipliers, one per
/// input, in order.
pub fn match_size_scales(bounds: &[Rect], op: MatchSize) -> Vec<(f64, f64)> {
    let widest = bounds.iter().map(|r| r.width()).fold(0.0, f64::max);
    let tallest = bounds.iter().map(|r| r.height()).fold(0.0, f64::max);

    bounds
        .iter()
        .map(|r| {
            // A rectangle with no extent in an axis cannot be scaled into one
            // that has: zero times anything is still zero, and dividing by it
            // is worse.
            let sx = if r.width() > f64::EPSILON {
                widest / r.width()
            } else {
                1.0
            };
            let sy = if r.height() > f64::EPSILON {
                tallest / r.height()
            } else {
                1.0
            };
            match op {
                MatchSize::Width => (sx, 1.0),
                MatchSize::Height => (1.0, sy),
                MatchSize::Both => (sx, sy),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn aligning_left_moves_everything_to_the_leftmost_edge() {
        let boxes = [r(10.0, 0.0, 20.0, 20.0), r(50.0, 0.0, 20.0, 20.0)];
        let offsets = align_offsets(&boxes, Align::LeftEdges, None);

        assert!(near(offsets[0].x, 0.0), "the leftmost should not move");
        assert!(near(offsets[1].x, -40.0));
        assert!(near(offsets[1].y, 0.0), "the other axis is untouched");
    }

    #[test]
    fn aligning_centres_uses_the_union_of_the_selection() {
        // Union is 10..70, centre 40.
        let boxes = [r(10.0, 0.0, 20.0, 20.0), r(50.0, 0.0, 20.0, 20.0)];
        let offsets = align_offsets(&boxes, Align::HorizontalCentres, None);

        assert!(near(boxes[0].center().x + offsets[0].x, 40.0));
        assert!(near(boxes[1].center().x + offsets[1].x, 40.0));
    }

    /// **Align to stage is a different operation, not a variation on one.**
    #[test]
    fn aligning_to_the_stage_uses_the_stage_and_not_the_selection() {
        let stage = r(0.0, 0.0, 1920.0, 1080.0);
        let boxes = [r(10.0, 10.0, 20.0, 20.0)];

        let offsets = align_offsets(&boxes, Align::HorizontalCentres, Some(stage));
        assert!(near(boxes[0].center().x + offsets[0].x, 960.0));

        let offsets = align_offsets(&boxes, Align::BottomEdges, Some(stage));
        assert!(near(boxes[0].y1 + offsets[0].y, 1080.0));
    }

    #[test]
    fn one_object_aligned_to_itself_does_not_move() {
        let boxes = [r(10.0, 10.0, 20.0, 20.0)];
        for op in Align::ALL {
            let offsets = align_offsets(&boxes, op, None);
            assert!(near(offsets[0].x, 0.0) && near(offsets[0].y, 0.0), "{op:?}");
        }
    }

    #[test]
    fn aligning_nothing_returns_nothing() {
        assert!(align_offsets(&[], Align::LeftEdges, None).is_empty());
    }

    /// The ends stay put and the middle is spaced evenly between them.
    #[test]
    fn distributing_centres_leaves_the_outermost_alone() {
        let boxes = [
            r(0.0, 0.0, 10.0, 10.0),   // centre 5
            r(30.0, 0.0, 10.0, 10.0),  // centre 35
            r(100.0, 0.0, 10.0, 10.0), // centre 105
        ];
        let offsets = distribute_offsets(&boxes, Distribute::HorizontalCentres);

        assert!(near(offsets[0].x, 0.0), "first must not move");
        assert!(near(offsets[2].x, 0.0), "last must not move");
        // Halfway between 5 and 105 is 55.
        assert!(near(boxes[1].center().x + offsets[1].x, 55.0));
    }

    /// **Equal gaps, not equal centres** — the two differ as soon as the
    /// objects are different sizes, and this is the one the eye reads as
    /// evenly spaced.
    #[test]
    fn distributing_spacing_makes_the_gaps_equal() {
        let boxes = [
            r(0.0, 0.0, 10.0, 10.0),
            r(20.0, 0.0, 40.0, 10.0), // much wider
            r(100.0, 0.0, 10.0, 10.0),
        ];
        let offsets = distribute_offsets(&boxes, Distribute::HorizontalSpacing);
        let moved: Vec<Rect> = boxes
            .iter()
            .zip(&offsets)
            .map(|(r, o)| Rect::new(r.x0 + o.x, r.y0, r.x1 + o.x, r.y1))
            .collect();

        let gap_a = moved[1].x0 - moved[0].x1;
        let gap_b = moved[2].x0 - moved[1].x1;
        assert!(near(gap_a, gap_b), "gaps were {gap_a} and {gap_b}");
        assert!(near(moved[0].x0, 0.0), "first must not move");
        assert!(near(moved[2].x1, 110.0), "last must not move");
    }

    /// Distributing is about what is *between* things, so two of them have
    /// nothing to distribute.
    #[test]
    fn distributing_fewer_than_three_does_nothing() {
        let boxes = [r(0.0, 0.0, 10.0, 10.0), r(100.0, 0.0, 10.0, 10.0)];
        for op in Distribute::ALL {
            let offsets = distribute_offsets(&boxes, op);
            assert!(offsets.iter().all(|o| near(o.x, 0.0) && near(o.y, 0.0)));
        }
    }

    /// The answer must not depend on the order things were clicked in.
    #[test]
    fn distributing_does_not_depend_on_selection_order() {
        let forwards = [
            r(0.0, 0.0, 10.0, 10.0),
            r(30.0, 0.0, 10.0, 10.0),
            r(100.0, 0.0, 10.0, 10.0),
        ];
        let backwards = [forwards[2], forwards[1], forwards[0]];

        let a = distribute_offsets(&forwards, Distribute::HorizontalCentres);
        let b = distribute_offsets(&backwards, Distribute::HorizontalCentres);

        assert!(near(a[1].x, b[1].x), "the middle one moved differently");
    }

    #[test]
    fn matching_size_scales_up_to_the_largest() {
        let boxes = [r(0.0, 0.0, 10.0, 20.0), r(0.0, 0.0, 40.0, 5.0)];

        let scales = match_size_scales(&boxes, MatchSize::Width);
        assert!(near(scales[0].0, 4.0));
        assert!(near(scales[0].1, 1.0), "height untouched");
        assert!(near(scales[1].0, 1.0), "the widest does not change");

        let scales = match_size_scales(&boxes, MatchSize::Both);
        assert!(near(scales[0].1, 1.0), "already the tallest");
        assert!(near(scales[1].1, 4.0));
    }

    /// A degenerate rectangle — a straight horizontal line has no height —
    /// must not produce an infinity.
    #[test]
    fn matching_size_survives_a_flat_shape() {
        let boxes = [r(0.0, 0.0, 10.0, 0.0), r(0.0, 0.0, 20.0, 20.0)];
        let scales = match_size_scales(&boxes, MatchSize::Both);
        assert!(scales.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
        assert!(near(scales[0].1, 1.0), "a flat shape keeps its height");
    }
}
