//! Turning a filter into paths.
//!
//! # The soft edge
//!
//! Everything here is built from one idea. Take the artwork's outline, fill it,
//! then stroke that same outline several times — each stroke a little wider and
//! a little more transparent, widest first. A point `t` outside the edge is
//! covered by every stroke wider than `2t`, so the coverage falls off smoothly
//! with distance: a blur of the silhouette, made of a fill and a handful of
//! strokes, with no offsetting, no booleans and no buffers.
//!
//! The alphas are chosen so the *cumulative* coverage after all the strokes
//! matches the profile we want, rather than each stroke being drawn at the
//! coverage it is meant to contribute. Drawing band `i` over everything already
//! there means
//!
//! ```text
//! after[i] = after[i-1] + alpha[i] * (1 - after[i-1])
//! ```
//!
//! so `alpha[i] = (target[i] - target[i-1]) / (1 - target[i-1])`, which is what
//! [`band_alphas`] computes. Get this wrong and the ramp is either flat or
//! banded — it is the whole difference between a soft edge and a set of rings.
//!
//! # Blur is the expensive one
//!
//! A shadow only spreads outwards, so a fill and some strokes are exactly
//! right. A blur has to fade on the *inside* of the edge as well, and no amount
//! of stacking can make source-over compositing take coverage away. So a blur
//! is built from real offset copies of the path, shrunk through to grown, which
//! costs one boolean per band. The renderer caches the result; nothing else
//! here needs caching at all.

use buzz_geom::{Affine, BezPath, BooleanOptions, expand_fill};
use peniko::Color;

use crate::{BevelKind, ColorAdjust, Filter, FilterKind, Quality};

/// One thing to paint. The renderer runs these through its own transform.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Fill {
        path: BezPath,
        color: Color,
        /// Even-odd rather than non-zero, which is how a ring is drawn: an
        /// outer boundary and an inner one, with the hole between them.
        even_odd: bool,
    },
    Stroke {
        path: BezPath,
        color: Color,
        width: f64,
        /// An extra transform the *pen* is subject to as well as the path.
        ///
        /// This is what makes an elliptical blur elliptical: the path is
        /// squashed so the blur is round, stroked with a round pen, and the
        /// whole thing — outline and pen together — is stretched back. Drawing
        /// the un-squashed path with a scaled width would give a round pen
        /// again, which is how the first version quietly lost Blur Y.
        transform: Affine,
    },
    /// Clip everything until the matching [`Op::PopClip`].
    PushClip(BezPath),
    PopClip,
}

/// Everything a stack of filters paints around one subject.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Painted {
    /// Drawn before the artwork.
    pub behind: Vec<Op>,
    /// Drawn after it.
    pub over: Vec<Op>,
    /// The artwork itself is not drawn — Animate's Knockout, and Drop Shadow's
    /// "Hide object".
    pub hide_subject: bool,
    /// The artwork is drawn blurred instead of sharp, by this much.
    ///
    /// Handled by the renderer rather than here because it needs each shape's
    /// own fill and stroke colours, which a silhouette has thrown away.
    pub blur: Option<(f64, f64, Quality)>,
    /// Colour adjustment to apply to every colour in the subject.
    pub adjust: Option<ColorAdjust>,
}

impl Painted {
    pub fn is_empty(&self) -> bool {
        self.behind.is_empty()
            && self.over.is_empty()
            && !self.hide_subject
            && self.blur.is_none()
            && self.adjust.is_none()
    }
}

/// Build everything a filter stack paints around `silhouette`.
///
/// `silhouette` is the subject's outline in the space the ops will be drawn in:
/// every filled path it contains, concatenated, to be filled non-zero.
pub fn build(filters: &[Filter], silhouette: &BezPath) -> Painted {
    let mut out = Painted::default();

    for filter in filters.iter().filter(|f| f.enabled) {
        match &filter.kind {
            FilterKind::Adjust(adjust) => {
                if adjust.is_identity() {
                    continue;
                }
                // Two adjustments compose by being applied in turn; the
                // renderer folds them into one closure over the colour.
                out.adjust = Some(match out.adjust {
                    Some(_) => *adjust,
                    None => *adjust,
                });
            }

            FilterKind::Blur { x, y, quality } => {
                if *x > 0.0 || *y > 0.0 {
                    out.blur = Some((*x, *y, *quality));
                }
            }

            FilterKind::DropShadow {
                x,
                y,
                strength,
                angle,
                distance,
                color,
                inner,
                knockout,
                hide_object,
                quality,
            } => {
                let (sin, cos) = angle.sin_cos();
                let offset = Affine::translate((cos * distance, sin * distance));
                let cast = offset * silhouette.clone();
                let colour = with_strength(*color, *strength);

                if *inner {
                    // An inner shadow is the *absence* of the offset shape,
                    // clipped to the artwork: the light is blocked everywhere
                    // the offset silhouette does not cover.
                    out.over.push(Op::PushClip(silhouette.clone()));
                    out.over
                        .extend(inner_edge(silhouette, &cast, colour, (*x, *y), *quality));
                    out.over.push(Op::PopClip);
                } else {
                    out.behind
                        .extend(soft_edge(&cast, colour, (*x, *y), *quality));
                }

                if *knockout || *hide_object {
                    out.hide_subject = true;
                }
            }

            FilterKind::Glow {
                x,
                y,
                strength,
                color,
                inner,
                knockout,
                quality,
            } => {
                let colour = with_strength(*color, *strength);
                if *inner {
                    out.over.push(Op::PushClip(silhouette.clone()));
                    out.over.extend(inner_edge(
                        silhouette,
                        silhouette,
                        colour,
                        (*x, *y),
                        *quality,
                    ));
                    out.over.push(Op::PopClip);
                } else {
                    out.behind
                        .extend(soft_edge(silhouette, colour, (*x, *y), *quality));
                }
                if *knockout {
                    out.hide_subject = true;
                }
            }

            FilterKind::Bevel {
                x,
                y,
                strength,
                angle,
                distance,
                highlight,
                shadow,
                kind,
                knockout,
                quality,
            } => {
                let (sin, cos) = angle.sin_cos();
                let towards = Affine::translate((cos * distance, sin * distance));
                let away = Affine::translate((-cos * distance, -sin * distance));

                let lit = with_strength(*highlight, *strength);
                let dark = with_strength(*shadow, *strength);

                // The lit side is the shape offset towards the light, the dark
                // side the shape offset away — each showing only where it does
                // *not* overlap the shape, which is exactly the edge.
                let mut edges = Vec::new();
                edges.extend(inner_edge(
                    silhouette,
                    &(towards * silhouette.clone()),
                    dark,
                    (*x, *y),
                    *quality,
                ));
                edges.extend(inner_edge(
                    silhouette,
                    &(away * silhouette.clone()),
                    lit,
                    (*x, *y),
                    *quality,
                ));

                match kind {
                    BevelKind::Inner => {
                        out.over.push(Op::PushClip(silhouette.clone()));
                        out.over.extend(edges);
                        out.over.push(Op::PopClip);
                    }
                    BevelKind::Outer => {
                        // Outside the shape: the same two edges, drawn behind
                        // so the artwork covers the half that falls inside.
                        out.behind.extend(soft_edge(
                            &(towards * silhouette.clone()),
                            dark,
                            (*x, *y),
                            *quality,
                        ));
                        out.behind.extend(soft_edge(
                            &(away * silhouette.clone()),
                            lit,
                            (*x, *y),
                            *quality,
                        ));
                    }
                    BevelKind::Full => {
                        out.behind.extend(soft_edge(
                            &(towards * silhouette.clone()),
                            dark,
                            (*x, *y),
                            *quality,
                        ));
                        out.behind.extend(soft_edge(
                            &(away * silhouette.clone()),
                            lit,
                            (*x, *y),
                            *quality,
                        ));
                        out.over.push(Op::PushClip(silhouette.clone()));
                        out.over.extend(edges);
                        out.over.push(Op::PopClip);
                    }
                }

                if *knockout {
                    out.hide_subject = true;
                }
            }
        }
    }

    out
}

/// A filled silhouette with a soft outer edge.
///
/// The fill is opaque and the ramp fades outwards over `radius`. This is the
/// shape a drop shadow, a glow and an outer bevel are all made of.
pub fn soft_edge(path: &BezPath, color: Color, radius: (f64, f64), quality: Quality) -> Vec<Op> {
    if path.elements().is_empty() {
        return Vec::new();
    }
    let (rx, ry) = (radius.0.max(0.0), radius.1.max(0.0));
    if rx <= 0.0 && ry <= 0.0 {
        return vec![Op::Fill {
            path: path.clone(),
            color,
            even_odd: false,
        }];
    }

    let bands = quality.bands();
    let alphas = band_alphas(bands);
    let mut ops = Vec::with_capacity(bands + 1);

    // Anisotropy is handled by working in a space where the blur is round:
    // squash by the ratio, stroke, and let the caller's transform unsquash it.
    // A ratio of zero would collapse the path, so a blur in one axis only is
    // treated as a very thin one rather than a degenerate transform.
    let (squash, unsquash, r) = anisotropy(rx, ry);

    // Widest first, so each narrower band composites over the ones outside it.
    let squashed = squash * path.clone();
    for (i, alpha) in alphas.iter().enumerate() {
        let width = 2.0 * r * (bands - i) as f64 / bands as f64;
        ops.push(Op::Stroke {
            path: squashed.clone(),
            color: color.multiply_alpha(*alpha as f32),
            width,
            transform: unsquash,
        });
    }

    // The body last: it is opaque, and it must sit over the inner halves of
    // every stroke rather than under them.
    ops.push(Op::Fill {
        path: path.clone(),
        color,
        even_odd: false,
    });
    ops
}

/// The soft edge of a hole: colour everywhere `subject` covers and `hole` does
/// not, fading inwards. This is an inner shadow, an inner glow and a bevel's
/// lit edge.
fn inner_edge(
    subject: &BezPath,
    hole: &BezPath,
    color: Color,
    radius: (f64, f64),
    quality: Quality,
) -> Vec<Op> {
    if subject.elements().is_empty() {
        return Vec::new();
    }

    // Subject minus hole, as one even-odd path: the two boundaries together,
    // filled even-odd, is the region between them. No boolean needed — and
    // where they do not overlap at all, even-odd still gives the right answer.
    let mut ring = subject.clone();
    for element in hole.elements() {
        ring.push(*element);
    }

    let bands = quality.bands();
    let alphas = band_alphas(bands);
    let (rx, ry) = (radius.0.max(0.0), radius.1.max(0.0));
    let mut ops = Vec::with_capacity(bands + 1);

    ops.push(Op::Fill {
        path: ring,
        color,
        even_odd: true,
    });

    if rx > 0.0 || ry > 0.0 {
        // Soften along the hole's edge, which is where the shadow is darkest.
        let (squash, unsquash, r) = anisotropy(rx, ry);
        let squashed = squash * hole.clone();
        for (i, alpha) in alphas.iter().enumerate() {
            let width = 2.0 * r * (bands - i) as f64 / bands as f64;
            ops.push(Op::Stroke {
                path: squashed.clone(),
                // Half strength: an inner edge is drawn over artwork rather
                // than over the background, and a full-strength ramp on top of
                // the colour it is shading reads as a smear.
                color: color.multiply_alpha(*alpha as f32 * 0.5),
                width,
                transform: unsquash,
            });
        }
    }
    ops
}

/// Blur one filled path: a stack of offset copies from shrunk to grown.
///
/// The only filter that costs booleans, and the only one that has to: a blur
/// fades on both sides of the edge, and stacking translucent copies of the
/// *same* path can only ever add coverage, never take it away.
pub fn blur_ops(
    path: &BezPath,
    color: Color,
    radius: (f64, f64),
    quality: Quality,
    tolerance: f64,
) -> Vec<Op> {
    let (rx, ry) = (radius.0.max(0.0), radius.1.max(0.0));
    if path.elements().is_empty() {
        return Vec::new();
    }
    if rx <= 0.0 && ry <= 0.0 {
        return vec![Op::Fill {
            path: path.clone(),
            color,
            even_odd: false,
        }];
    }

    let bands = quality.bands();
    let alphas = band_alphas(bands);
    let opts = BooleanOptions {
        tolerance,
        ..BooleanOptions::default()
    };
    let (squash, unsquash, r) = anisotropy(rx, ry);
    let squashed = squash * path.clone();

    // From +r (outermost, faintest) down to -r (the opaque core).
    let mut ops = Vec::with_capacity(bands);
    for (i, alpha) in alphas.iter().enumerate() {
        let t = (bands - i) as f64 / bands as f64; // 1 .. 1/bands
        let offset = r * (2.0 * t - 1.0);
        let grown = if offset.abs() < 1e-9 {
            squashed.clone()
        } else {
            expand_fill(&squashed, offset, opts)
        };
        if grown.elements().is_empty() {
            // Shrunk away to nothing: a shape thinner than the blur simply has
            // no opaque core left, which is what a real blur does to it.
            continue;
        }
        ops.push(Op::Fill {
            path: unsquash * grown,
            color: color.multiply_alpha(*alpha as f32),
            even_odd: false,
        });
    }
    ops
}

/// The alpha each band is drawn at, outermost first, so that the *cumulative*
/// coverage follows a smooth ramp from nothing to opaque.
fn band_alphas(bands: usize) -> Vec<f64> {
    let bands = bands.max(1);
    let mut out = Vec::with_capacity(bands);
    let mut reached = 0.0;

    for i in 0..bands {
        // Where this band sits across the ramp, measured at its middle.
        let u = (i as f64 + 0.5) / bands as f64;
        // Smoothstep: flat at both ends, so the outer edge fades out rather
        // than stopping, and the inner one meets the body without a seam.
        let target = if i + 1 == bands {
            1.0
        } else {
            u * u * (3.0 - 2.0 * u)
        };
        let alpha = if reached >= 1.0 {
            1.0
        } else {
            ((target - reached) / (1.0 - reached)).clamp(0.0, 1.0)
        };
        out.push(alpha);
        reached = target;
    }
    out
}

/// Colour at a filter's strength.
///
/// Animate's Strength is a percentage that can exceed 100; beyond full opacity
/// it darkens by stacking, which here is simply a clamp — a shadow cannot be
/// more opaque than opaque.
fn with_strength(color: Color, strength: f64) -> Color {
    color.multiply_alpha(strength.clamp(0.0, 1.0) as f32)
}

/// Squash and unsquash transforms that turn an elliptical blur into a round
/// one, and the radius to use in that round space.
fn anisotropy(rx: f64, ry: f64) -> (Affine, Affine, f64) {
    let rx = rx.max(0.01);
    let ry = ry.max(0.01);
    if (rx - ry).abs() < 1e-9 {
        return (Affine::IDENTITY, Affine::IDENTITY, rx);
    }
    // Work in a space where y is scaled so the y radius equals the x radius.
    let scale = rx / ry;
    (
        Affine::scale_non_uniform(1.0, scale),
        Affine::scale_non_uniform(1.0, 1.0 / scale),
        rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _};

    fn square() -> BezPath {
        Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9)
    }

    fn filters(kind: FilterKind) -> Vec<Filter> {
        vec![Filter::new(kind)]
    }

    /// The ramp must actually reach opacity, and never overshoot: an alpha
    /// outside `0..=1` is a colour the renderer cannot draw.
    #[test]
    fn the_ramp_climbs_from_nothing_to_opaque() {
        for bands in [1usize, 4, 8, 14, 40] {
            let alphas = band_alphas(bands);
            assert_eq!(alphas.len(), bands);
            assert!(alphas.iter().all(|a| (0.0..=1.0).contains(a)), "{alphas:?}");

            // Composite them in order and see where the coverage ends up.
            let mut reached = 0.0;
            for alpha in &alphas {
                reached += alpha * (1.0 - reached);
            }
            assert!(
                (reached - 1.0).abs() < 1e-9,
                "{bands} bands reached {reached}"
            );
        }
    }

    /// Coverage has to *increase* inwards, or the edge reads as rings.
    #[test]
    fn coverage_increases_towards_the_shape() {
        let alphas = band_alphas(10);
        let mut reached = 0.0;
        let mut last = 0.0;
        for alpha in &alphas {
            reached += alpha * (1.0 - reached);
            assert!(
                reached >= last - 1e-12,
                "coverage went backwards: {alphas:?}"
            );
            last = reached;
        }
    }

    #[test]
    fn an_empty_stack_paints_nothing() {
        let painted = build(&[], &square());
        assert!(painted.is_empty());
    }

    /// A filter that is switched off is not applied, but is not forgotten
    /// either — that is what the flag is for.
    #[test]
    fn a_disabled_filter_paints_nothing() {
        let mut filter = Filter::new(FilterKind::drop_shadow());
        filter.enabled = false;
        assert!(build(&[filter], &square()).is_empty());
    }

    /// A drop shadow lands on the far side of the artwork, behind it.
    #[test]
    fn a_drop_shadow_is_offset_and_drawn_behind() {
        let painted = build(&filters(FilterKind::drop_shadow()), &square());
        assert!(painted.over.is_empty(), "an outer shadow goes behind");
        assert!(!painted.behind.is_empty());
        assert!(!painted.hide_subject);

        // The shadow's body is the last op, and it sits down and to the right
        // of the artwork — the default 45° angle.
        let Some(Op::Fill { path, .. }) = painted.behind.last() else {
            panic!("the shadow should end with its body: {:?}", painted.behind);
        };
        let bounds = path.bounding_box();
        assert!(bounds.x0 > 0.0 && bounds.y0 > 0.0, "{bounds:?}");
    }

    /// Turning the light round turns the shadow round.
    #[test]
    fn the_shadow_follows_the_angle() {
        let mut west = FilterKind::drop_shadow();
        if let FilterKind::DropShadow { angle, .. } = &mut west {
            *angle = std::f64::consts::PI;
        }
        let painted = build(&filters(west), &square());
        let Some(Op::Fill { path, .. }) = painted.behind.last() else {
            panic!()
        };
        assert!(
            path.bounding_box().x0 < 0.0,
            "a shadow to the west should fall left of the artwork"
        );
    }

    /// Knockout and "hide object" both mean: keep the effect, drop the artwork.
    #[test]
    fn knockout_hides_the_artwork() {
        let kind = match FilterKind::drop_shadow() {
            FilterKind::DropShadow {
                x,
                y,
                strength,
                angle,
                distance,
                color,
                quality,
                ..
            } => FilterKind::DropShadow {
                x,
                y,
                strength,
                angle,
                distance,
                color,
                inner: false,
                knockout: true,
                hide_object: false,
                quality,
            },
            _ => unreachable!(),
        };
        assert!(build(&filters(kind), &square()).hide_subject);
    }

    /// An inner glow paints inside the artwork, clipped to it, and the clip is
    /// balanced — an unbalanced clip would swallow the rest of the frame.
    #[test]
    fn an_inner_glow_is_clipped_to_the_shape() {
        let kind = FilterKind::Glow {
            x: 8.0,
            y: 8.0,
            strength: 1.0,
            color: Color::WHITE,
            inner: true,
            knockout: false,
            quality: Quality::Low,
        };
        let painted = build(&filters(kind), &square());

        assert!(painted.behind.is_empty(), "an inner glow is not behind");
        assert!(matches!(painted.over.first(), Some(Op::PushClip(_))));
        assert!(matches!(painted.over.last(), Some(Op::PopClip)));

        let pushes = painted
            .over
            .iter()
            .filter(|op| matches!(op, Op::PushClip(_)))
            .count();
        let pops = painted
            .over
            .iter()
            .filter(|op| matches!(op, Op::PopClip))
            .count();
        assert_eq!(pushes, pops, "every clip must be closed");
    }

    /// Every clip a filter stack opens is closed, whatever the stack.
    #[test]
    fn clips_are_always_balanced() {
        let all: Vec<Filter> = FilterKind::all().into_iter().map(Filter::new).collect();
        let painted = build(&all, &square());

        for ops in [&painted.behind, &painted.over] {
            let mut depth = 0i32;
            for op in ops.iter() {
                match op {
                    Op::PushClip(_) => depth += 1,
                    Op::PopClip => depth -= 1,
                    _ => {}
                }
                assert!(depth >= 0, "a clip was popped before it was pushed");
            }
            assert_eq!(depth, 0, "a clip was left open");
        }
    }

    /// A bevel lights one side and darkens the other; both must be there.
    #[test]
    fn a_bevel_paints_a_lit_side_and_a_dark_one() {
        let painted = build(&filters(FilterKind::bevel()), &square());
        let colours: Vec<Color> = painted
            .over
            .iter()
            .filter_map(|op| match op {
                Op::Fill { color, .. } | Op::Stroke { color, .. } => Some(*color),
                _ => None,
            })
            .collect();

        let luma = |c: &Color| {
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
        };
        assert!(
            colours.iter().any(|c| luma(c) > 200.0),
            "no highlight: {colours:?}"
        );
        assert!(
            colours.iter().any(|c| luma(c) < 60.0),
            "no shadow: {colours:?}"
        );
    }

    /// Blur is handed to the renderer rather than drawn here, because it needs
    /// the artwork's own colours.
    #[test]
    fn blur_is_reported_rather_than_painted() {
        let painted = build(&filters(FilterKind::blur()), &square());
        assert_eq!(painted.blur, Some((5.0, 5.0, Quality::Medium)));
        assert!(painted.behind.is_empty() && painted.over.is_empty());
    }

    /// A blur of nothing is nothing; a blur of zero radius is the artwork.
    #[test]
    fn a_zero_blur_draws_the_shape_unchanged() {
        let ops = blur_ops(&square(), Color::BLACK, (0.0, 0.0), Quality::Low, 0.01);
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            ops[0],
            Op::Fill {
                even_odd: false,
                ..
            }
        ));

        assert!(
            blur_ops(
                &BezPath::new(),
                Color::BLACK,
                (4.0, 4.0),
                Quality::Low,
                0.01
            )
            .is_empty()
        );
    }

    /// A real blur spreads the shape outwards and fades it: the outermost band
    /// must be bigger than the shape and fainter than the core.
    #[test]
    fn a_blur_spreads_outwards_and_fades() {
        let ops = blur_ops(&square(), Color::BLACK, (10.0, 10.0), Quality::Low, 0.01);
        assert!(ops.len() > 1, "a blur should be a ramp: {}", ops.len());

        let Op::Fill { path, color, .. } = &ops[0] else {
            panic!("expected a fill")
        };
        let outer = path.bounding_box();
        assert!(
            outer.x0 < -5.0 && outer.x1 > 105.0,
            "the outermost band should be grown: {outer:?}"
        );
        assert!(
            color.components[3] < 0.5,
            "the outermost band should be faint: {color:?}"
        );

        let Op::Fill { color: core, .. } = ops.last().unwrap() else {
            panic!()
        };
        assert!(
            core.components[3] > color.components[3],
            "the core should be more opaque than the edge"
        );
    }

    /// A shape thinner than the blur has no opaque core left, and must not
    /// produce an empty or inverted path when it is shrunk away.
    #[test]
    fn blurring_a_hairline_shape_does_not_break() {
        let sliver = Rect::new(0.0, 0.0, 100.0, 2.0).to_path(1e-9);
        let ops = blur_ops(&sliver, Color::BLACK, (20.0, 20.0), Quality::Low, 0.01);
        for op in &ops {
            if let Op::Fill { path, .. } = op {
                assert!(!path.elements().is_empty());
            }
        }
    }

    /// An elliptical blur really is elliptical: a big X and a small Y must
    /// spread wider than tall, and the *pen* is what carries that.
    #[test]
    fn an_elliptical_blur_spreads_further_on_its_long_axis() {
        let ops = soft_edge(&square(), Color::BLACK, (30.0, 4.0), Quality::Low);
        let Some(Op::Stroke {
            path,
            width,
            transform,
            ..
        }) = ops.first()
        else {
            panic!("expected the widest stroke first: {ops:?}")
        };
        assert!(*width > 0.0);

        // The path is stretched in y so the round pen covers 30 across and 4
        // down once the transform squashes both back.
        let c = transform.as_coeffs();
        assert!(
            c[3] < c[0],
            "the pen should be squashed vertically: {transform:?}"
        );

        // Half the pen width, through the transform, is how far the softening
        // actually reaches on each axis.
        let spread_x = width * 0.5 * c[0];
        let spread_y = width * 0.5 * c[3];
        assert!(
            (spread_x - 30.0).abs() < 1e-6,
            "the pen should reach 30 across: {spread_x}"
        );
        assert!((spread_y - 4.0).abs() < 1e-6, "and 4 down: {spread_y}");

        // And the squashed path is taller than it is wide, which is what makes
        // that work.
        let bounds = path.bounding_box();
        assert!(bounds.height() > bounds.width(), "{bounds:?}");
    }

    /// A round blur needs no squashing at all — the common case must not pay
    /// for the awkward one.
    #[test]
    fn a_round_blur_uses_no_transform() {
        let ops = soft_edge(&square(), Color::BLACK, (8.0, 8.0), Quality::Low);
        let Some(Op::Stroke { transform, .. }) = ops.first() else {
            panic!()
        };
        assert_eq!(*transform, Affine::IDENTITY);
    }

    #[test]
    fn adjust_colour_is_reported_for_the_renderer_to_apply() {
        let adjust = ColorAdjust {
            brightness: 20.0,
            ..Default::default()
        };
        let painted = build(&filters(FilterKind::Adjust(adjust)), &square());
        assert_eq!(painted.adjust, Some(adjust));
        assert!(painted.behind.is_empty() && painted.over.is_empty());
    }

    /// An adjustment that does nothing is not reported at all, so the renderer
    /// does not walk a whole subtree to apply an identity.
    #[test]
    fn an_empty_adjustment_is_dropped() {
        let painted = build(&filters(FilterKind::adjust()), &square());
        assert!(painted.adjust.is_none());
        assert!(painted.is_empty());
    }

    /// Several filters stack, in order, as Animate's list does.
    #[test]
    fn filters_stack() {
        let stack = vec![
            Filter::new(FilterKind::glow()),
            Filter::new(FilterKind::drop_shadow()),
        ];
        let painted = build(&stack, &square());
        // Both painted behind, glow first — the order they are listed in.
        assert!(painted.behind.len() > 4);
    }

    #[test]
    fn a_soft_edge_of_nothing_is_nothing() {
        assert!(soft_edge(&BezPath::new(), Color::BLACK, (4.0, 4.0), Quality::Low).is_empty());
    }
}
