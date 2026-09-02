//! A brush made out of artwork you already drew.
//!
//! Animate's *Create Brush From Selection* takes the selected artwork and
//! repeats it along a stroke. It takes the **shape**; what it does not take is
//! what the shape was painted with, so a brush made from a red leaf with a
//! gradient down it comes out as a flat silhouette in whatever colour the fill
//! swatch happens to be. That is a different thing from the artwork you
//! pointed at, and it is the thing this module exists to fix: a captured brush
//! keeps its colours, its gradients and its bitmaps, and stamps the artwork
//! itself.
//!
//! # Stamp space
//!
//! Captured artwork is normalised once, at capture: recentred on its own
//! middle and scaled so its longest side is exactly `1.0`. Everything
//! downstream is then a plain multiplication — the brush's size slider *is*
//! the scale factor — and the size setting means the same thing for a captured
//! brush as it does for a built-in one.
//!
//! **Paint is carried through every one of those transforms.** A gradient and
//! a bitmap are both positioned in space; moving artwork without moving its
//! paint leaves the picture sliding about inside its own outline, which is the
//! whole reason [`Paint::transformed`] exists.
//!
//! # Why stamps are merged by piece
//!
//! A stroke can ask for hundreds of stamps, and captured artwork can be
//! several shapes. Emitting one shape per piece per stamp is tens of thousands
//! of objects for one drag — and it is rebuilt on every pointer move to
//! preview it, so it is not a slow drag, it is a stopped application.
//!
//! So pieces that are painted in a **flat colour** are merged: every stamp's
//! copy of piece *k* goes into one path, filled once. A hundred stamps of a
//! three-shape leaf is three shapes, not three hundred. Pieces carrying a
//! gradient or a bitmap cannot be merged — their paint is positioned, so each
//! copy needs its own — and those are capped instead.
//!
//! The visible consequence, stated plainly because it is a real one: merged
//! stamps are layered piece-by-piece rather than stamp-by-stamp. Where stamps
//! overlap — spacing tighter than the artwork is wide — every stamp's piece 2
//! draws over every stamp's piece 1, instead of each stamp drawing complete in
//! turn. At the spacings a pattern brush is actually used at the two are the
//! same picture, and the alternative is a frozen window.

use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _, Size};
use peniko::Color;

use crate::art::ArtPiece;
use crate::object::{FillSpec, Paint, ShapeData, StrokeSpec};

/// How many positioned copies — gradient- or bitmap-painted pieces, which
/// cannot be merged — one stroke may place.
///
/// Not a quality setting. Each of these is a separate shape with its own
/// paint, rebuilt on every pointer move; this is the number that keeps a long
/// stroke of textured stamps interactive.
pub const MAX_POSITIONED: usize = 240;

/// Artwork captured as a brush.
///
/// In stamp space: centred on the origin, longest side `1.0`. See the module
/// header.
#[derive(Debug, Clone, PartialEq)]
pub struct BrushStamp {
    pieces: Vec<ShapeData>,
    /// The artwork's extent in stamp space. The longest side is `1.0`; the
    /// other is whatever the artwork's proportions make it.
    extent: Size,
    /// Does any piece carry paint of its own?
    ///
    /// False for a stamp captured from bare geometry — the scripting API and
    /// the old shape-only path both make those — and it is what decides
    /// whether a brush can offer to keep the artwork's colours at all.
    painted: bool,
}

impl BrushStamp {
    /// Capture flattened artwork as a brush.
    ///
    /// `parts` is what [`crate::Object::flatten`] produces: each shape with
    /// the accumulated transform that places it on the stage. Both are used —
    /// the transform is applied to the geometry *and* to the paint, so a brush
    /// made from a group comes out looking like the group did.
    ///
    /// `None` when there is nothing with any area in it, which is what lets
    /// the caller say so rather than handing back an invisible brush.
    pub fn capture(parts: &[(Affine, ShapeData)]) -> Option<Self> {
        let placed: Vec<ShapeData> = parts
            .iter()
            .map(|(transform, shape)| transformed_shape(shape, *transform))
            .filter(|shape| !shape.path.elements().is_empty())
            .collect();
        if placed.is_empty() {
            return None;
        }

        let bounds = union_bounds(&placed)?;
        let longest = bounds.width().max(bounds.height());
        if !(longest > 0.0) || !longest.is_finite() {
            return None;
        }

        // Centre on the origin, then scale the longest side to one. Stamps sit
        // *on* the stroke, so artwork drawn at (400, 300) must not stamp 500
        // units away from the pointer.
        let normalise =
            Affine::scale(1.0 / longest) * Affine::translate(-bounds.center().to_vec2());
        let pieces: Vec<ShapeData> = placed
            .iter()
            .map(|shape| transformed_shape(shape, normalise))
            .collect();
        let painted = pieces
            .iter()
            .any(|p| p.fill.is_some() || p.stroke.is_some());

        Some(Self {
            pieces,
            extent: Size::new(bounds.width() / longest, bounds.height() / longest),
            painted,
        })
    }

    /// Capture bare geometry, with no paint of its own.
    ///
    /// What the shape-only path and the scripting API make: the stroke's own
    /// fill swatch paints it, exactly as a built-in pattern is painted.
    pub fn from_path(path: BezPath) -> Option<Self> {
        Self::capture(&[(
            Affine::IDENTITY,
            ShapeData {
                path,
                fill: None,
                stroke: None,
                blend: crate::object::PaintBlend::Normal,
            },
        )])
    }

    pub fn pieces(&self) -> &[ShapeData] {
        &self.pieces
    }

    /// The artwork's proportions in stamp space; the longest side is `1.0`.
    pub fn extent(&self) -> Size {
        self.extent
    }

    /// Does this stamp carry paint of its own — colours, gradients, bitmaps?
    pub fn is_painted(&self) -> bool {
        self.painted
    }

    /// Every piece's outline in one path, in stamp space.
    ///
    /// The silhouette: what a brush that is *not* keeping the artwork's
    /// colours stamps, and what the tool options draw as a thumbnail.
    pub fn outline(&self) -> BezPath {
        let mut out = BezPath::new();
        for piece in &self.pieces {
            out.extend(piece.path.iter());
        }
        out
    }

    /// One colour standing in for the whole stamp — for a swatch, and for the
    /// places that can only draw in one colour.
    pub fn average_color(&self) -> Color {
        let mut sum = [0.0f32; 4];
        let mut n = 0.0f32;
        for piece in &self.pieces {
            for paint in piece
                .fill
                .as_ref()
                .map(|f| &f.paint)
                .into_iter()
                .chain(piece.stroke.as_ref().map(|s| &s.paint))
            {
                let c = paint.color().components;
                for i in 0..4 {
                    sum[i] += c[i];
                }
                n += 1.0;
            }
        }
        if n <= 0.0 {
            return Color::BLACK;
        }
        Color::new([sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n])
    }

    /// The rectangle one stamp covers at `size`, centred on the origin.
    ///
    /// What the stamping arithmetic measures spacing and rotation against.
    pub fn source_rect(&self, size: f64) -> Rect {
        let size = size.max(f64::MIN_POSITIVE);
        let (w, h) = (self.extent.width * size, self.extent.height * size);
        Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)
    }

    /// One copy of the artwork, placed by `transform`, paint and all.
    pub fn place(&self, transform: Affine) -> Vec<ShapeData> {
        self.pieces
            .iter()
            .map(|piece| transformed_shape(piece, transform))
            .collect()
    }

    /// **Many copies, merged where they can be.** See the module header for
    /// what is merged and what that costs.
    ///
    /// The transforms are stamp space to document space — including the
    /// brush's size — as [`Self::source_rect`] measures them.
    pub fn place_many(&self, transforms: &[Affine]) -> StampedArt {
        let mut shapes: Vec<ShapeData> = Vec::new();
        let mut truncated = false;
        let mut positioned = 0usize;

        for piece in &self.pieces {
            if mergeable(piece) {
                // Every copy of this piece into one path, filled once. The
                // paint is flat, so it is the same paint wherever the copy
                // landed and nothing is lost by sharing it.
                let mut merged = BezPath::new();
                for transform in transforms {
                    merged.extend((*transform * piece.path.clone()).iter());
                }
                if merged.elements().is_empty() {
                    continue;
                }
                // The stroke width still has to follow the transform, and one
                // merged path can only carry one width — so it takes the
                // first, which is right because a pattern brush places every
                // stamp at the same scale.
                let scale = transforms.first().copied().unwrap_or(Affine::IDENTITY);
                let mut shape = transformed_shape(piece, scale);
                shape.path = merged;
                shapes.push(shape);
            } else {
                for transform in transforms {
                    if positioned >= MAX_POSITIONED {
                        truncated = true;
                        break;
                    }
                    shapes.push(transformed_shape(piece, *transform));
                    positioned += 1;
                }
            }
        }

        StampedArt { shapes, truncated }
    }

    /// [`Self::place_many`], as art pieces ready to commit.
    pub fn stamp_pieces(&self, transforms: &[Affine]) -> Vec<ArtPiece> {
        self.place_many(transforms)
            .shapes
            .into_iter()
            .map(ArtPiece::Shape)
            .collect()
    }
}

/// What a run of stamps came to.
#[derive(Debug, Clone, PartialEq)]
pub struct StampedArt {
    pub shapes: Vec<ShapeData>,
    /// Set when positioned copies — gradient or bitmap pieces — hit
    /// [`MAX_POSITIONED`] and the rest of the stroke went unpainted. Unlike a
    /// widened spacing this *does* lose part of the stroke, so a caller that
    /// reports anything should report this.
    pub truncated: bool,
}

/// Can every copy of this piece share one paint?
///
/// A flat colour is the same colour wherever the copy lands. A gradient or a
/// bitmap is positioned in space, so two copies in two places are two
/// different paints and merging them would put one stamp's picture across all
/// of them.
fn mergeable(piece: &ShapeData) -> bool {
    let flat = |paint: &Paint| matches!(paint, Paint::Solid(_));
    piece.fill.as_ref().is_none_or(|f| flat(&f.paint))
        && piece.stroke.as_ref().is_none_or(|s| flat(&s.paint))
}

/// A shape carried through a transform: geometry, paint and stroke width.
///
/// The width matters as much as the rest. Scaling artwork to a tenth without
/// scaling its outline leaves a hairline drawing wrapped in a fat black band,
/// which is what a brush made from a stroked shape used to look like.
fn transformed_shape(shape: &ShapeData, transform: Affine) -> ShapeData {
    ShapeData {
        path: transform * shape.path.clone(),
        fill: shape.fill.as_ref().map(|f| FillSpec {
            paint: f.paint.transformed(transform),
            rule: f.rule,
        }),
        stroke: shape.stroke.as_ref().map(|s| StrokeSpec {
            paint: s.paint.transformed(transform),
            width: s.width * scale_factor(transform),
            hairline: s.hairline,
        }),
        blend: shape.blend,
    }
}

/// How much a transform scales a length.
///
/// The square root of the determinant: the one number that is right for a
/// uniform scale and the honest average for a squashed one, which is all a
/// single stroke width can express.
fn scale_factor(transform: Affine) -> f64 {
    let c = transform.as_coeffs();
    (c[0] * c[3] - c[1] * c[2]).abs().sqrt()
}

/// The box every piece sits in, including the width of any outline.
fn union_bounds(shapes: &[ShapeData]) -> Option<Rect> {
    let mut all: Option<Rect> = None;
    for shape in shapes {
        let mut bounds = shape.path.bounding_box();
        // A stroked shape is as wide as its outline, not as its centreline.
        if let Some(stroke) = &shape.stroke
            && !stroke.hairline
        {
            let half = stroke.width / 2.0;
            bounds = bounds.inflate(half, half);
        }
        all = Some(match all {
            Some(a) => a.union(bounds),
            None => bounds,
        });
    }
    all.filter(|b| b.width().is_finite() && b.height().is_finite())
}

/// A stamp placed at a single point, for a tap.
pub fn tap_transform(at: Point, size: f64) -> Affine {
    Affine::translate(at.to_vec2()) * Affine::scale(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gradient::Gradient;
    use crate::object::PaintBlend;

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        Rect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    fn red_square() -> Vec<(Affine, ShapeData)> {
        vec![(
            Affine::IDENTITY,
            ShapeData::filled(square(400.0, 300.0, 50.0), Color::from_rgb8(0xFF, 0, 0)),
        )]
    }

    /// **The whole point.** A brush captured from red artwork stamps red,
    /// rather than a silhouette in whatever the fill swatch happens to be.
    #[test]
    fn a_captured_brush_keeps_the_colour_it_was_made_from() {
        let stamp = BrushStamp::capture(&red_square()).expect("a stamp");
        assert!(stamp.is_painted(), "it carries paint of its own");

        let placed = stamp.place(Affine::scale(20.0));
        assert_eq!(placed.len(), 1);
        let fill = placed[0].fill.as_ref().expect("a fill");
        assert_eq!(fill.paint.color(), Color::from_rgb8(0xFF, 0, 0));
    }

    /// A gradient is positioned in space, so it has to travel with the
    /// artwork — otherwise the ramp stays where the original was drawn and
    /// every stamp shows one flat slice of it.
    #[test]
    fn a_gradient_travels_with_the_artwork_it_paints() {
        let area = Rect::new(0.0, 0.0, 100.0, 100.0);
        let shape = ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::gradient(Gradient::linear(
                Color::BLACK,
                Color::WHITE,
                area,
            ))),
            stroke: None,
            blend: PaintBlend::Normal,
        };
        let stamp = BrushStamp::capture(&[(Affine::IDENTITY, shape)]).expect("a stamp");

        let far = Affine::translate((900.0, 400.0)) * Affine::scale(60.0);
        let placed = stamp.place(far);
        let paint = &placed[0].fill.as_ref().unwrap().paint;
        let gradient = paint.gradient().expect("still a gradient");

        // The ramp's own transform must have moved with the artwork: its
        // centre should now be out where the stamp was placed.
        let centre = gradient.transform * Point::ZERO;
        assert!(
            (centre.x - 900.0).abs() < 1.0 && (centre.y - 400.0).abs() < 1.0,
            "the ramp stayed behind at {centre:?}"
        );
    }

    /// A bitmap fill is the texture the user is asking to keep, and it must
    /// survive capture and placement with its pixels intact.
    #[test]
    fn a_bitmap_texture_survives_capture_and_placement() {
        use crate::image::{ImageAsset, ImageFill, ImageId};
        use std::sync::Arc;

        let asset = Arc::new(ImageAsset::from_pixels(
            ImageId(7),
            "Texture",
            2,
            2,
            Arc::new(vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ]),
        ));
        let area = Rect::new(0.0, 0.0, 40.0, 40.0);
        let shape = ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::image(ImageFill::new(Arc::clone(&asset), area))),
            stroke: None,
            blend: PaintBlend::Normal,
        };

        let stamp = BrushStamp::capture(&[(Affine::IDENTITY, shape)]).expect("a stamp");
        let placed = stamp.place(Affine::translate((200.0, 50.0)) * Affine::scale(30.0));
        let fill = placed[0].fill.as_ref().expect("a fill");
        let image = fill.paint.image().expect("still a bitmap");

        assert_eq!(image.asset.id, ImageId(7), "the same pixels, shared");
        assert!(
            Arc::ptr_eq(&image.asset, &asset),
            "and shared rather than copied"
        );
        // And the picture moved with its shape.
        let centre = image.transform * Point::new(0.5, 0.5);
        assert!(
            (centre.x - 200.0).abs() < 1.0 && (centre.y - 50.0).abs() < 1.0,
            "the texture did not travel: {centre:?}"
        );
    }

    /// Capture normalises: wherever the artwork was drawn and however big, one
    /// stamp at size 40 is 40 units across and centred on the stroke.
    #[test]
    fn capture_recentres_and_rescales_whatever_it_is_given() {
        for (x, y, size) in [(0.0, 0.0, 10.0), (400.0, 300.0, 50.0), (-900.0, 20.0, 3.0)] {
            let parts = vec![(
                Affine::IDENTITY,
                ShapeData::filled(square(x, y, size), Color::WHITE),
            )];
            let stamp = BrushStamp::capture(&parts).expect("a stamp");
            let placed = stamp.place(Affine::scale(40.0));
            let bounds = placed[0].path.bounding_box();

            assert!(
                (bounds.width() - 40.0).abs() < 1e-6,
                "{size} at ({x}, {y}) stamped {} across",
                bounds.width()
            );
            assert!(
                bounds.center().to_vec2().hypot() < 1e-6,
                "and is off centre at {:?}",
                bounds.center()
            );
        }
    }

    /// A group keeps its arrangement: the transform each part was flattened
    /// with is applied, so a brush made from a group looks like the group.
    #[test]
    fn a_group_keeps_its_arrangement_and_its_colours() {
        let parts = vec![
            (
                Affine::IDENTITY,
                ShapeData::filled(square(0.0, 0.0, 10.0), Color::from_rgb8(0xFF, 0, 0)),
            ),
            (
                Affine::translate((30.0, 0.0)),
                ShapeData::filled(square(0.0, 0.0, 10.0), Color::from_rgb8(0, 0, 0xFF)),
            ),
        ];
        let stamp = BrushStamp::capture(&parts).expect("a stamp");
        assert_eq!(stamp.pieces().len(), 2, "both parts are kept separately");

        let placed = stamp.place(Affine::scale(40.0));
        assert_eq!(
            placed[0].fill.as_ref().unwrap().paint.color(),
            Color::from_rgb8(0xFF, 0, 0)
        );
        assert_eq!(
            placed[1].fill.as_ref().unwrap().paint.color(),
            Color::from_rgb8(0, 0, 0xFF),
            "the second part keeps its own colour"
        );
        // Wider than tall, because the two squares sit side by side.
        assert!(stamp.extent().width > stamp.extent().height);
    }

    /// An outline is scaled with the artwork it surrounds. Without this a
    /// brush made from a stroked shape draws a hairline drawing inside a fat
    /// band.
    #[test]
    fn a_stroke_width_is_scaled_with_the_artwork() {
        let shape = ShapeData::stroked(square(0.0, 0.0, 100.0), Color::BLACK, 10.0);
        let stamp = BrushStamp::capture(&[(Affine::IDENTITY, shape)]).expect("a stamp");
        let placed = stamp.place(Affine::scale(20.0));
        let width = placed[0].stroke.as_ref().expect("a stroke").width;

        // The artwork was 100 across (plus its outline) and is stamped at 20,
        // so a 10-wide outline comes out near 10 x 20/110.
        assert!(
            width > 1.0 && width < 3.0,
            "a 10-unit outline on 100-unit artwork stamped at 20 came out {width}"
        );
    }

    /// **Flat-coloured stamps merge.** A hundred stamps of a two-piece brush
    /// is two shapes, not two hundred — the difference between a drag and a
    /// stopped application.
    #[test]
    fn flat_stamps_merge_into_one_shape_per_piece() {
        let parts = vec![
            (
                Affine::IDENTITY,
                ShapeData::filled(square(0.0, 0.0, 10.0), Color::from_rgb8(0xFF, 0, 0)),
            ),
            (
                Affine::translate((0.0, 12.0)),
                ShapeData::filled(square(0.0, 0.0, 10.0), Color::from_rgb8(0, 0, 0xFF)),
            ),
        ];
        let stamp = BrushStamp::capture(&parts).expect("a stamp");
        let transforms: Vec<Affine> = (0..100)
            .map(|i| Affine::translate((i as f64 * 20.0, 0.0)) * Affine::scale(15.0))
            .collect();

        let art = stamp.place_many(&transforms);
        assert_eq!(art.shapes.len(), 2, "one shape per piece, not per stamp");
        assert!(!art.truncated);

        // And all hundred copies are really in there.
        let bounds = art.shapes[0].path.bounding_box();
        assert!(
            bounds.width() > 1900.0,
            "the merged path should span every stamp: {bounds:?}"
        );
    }

    /// A positioned paint cannot merge — each copy needs its own ramp — so
    /// those are capped instead, and the cap is admitted to.
    #[test]
    fn positioned_paints_are_capped_rather_than_merged() {
        let area = Rect::new(0.0, 0.0, 10.0, 10.0);
        let shape = ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::gradient(Gradient::linear(
                Color::BLACK,
                Color::WHITE,
                area,
            ))),
            stroke: None,
            blend: PaintBlend::Normal,
        };
        let stamp = BrushStamp::capture(&[(Affine::IDENTITY, shape)]).expect("a stamp");
        let transforms: Vec<Affine> = (0..MAX_POSITIONED * 3)
            .map(|i| Affine::translate((i as f64 * 5.0, 0.0)) * Affine::scale(10.0))
            .collect();

        let art = stamp.place_many(&transforms);
        assert_eq!(art.shapes.len(), MAX_POSITIONED);
        assert!(art.truncated, "and it says the stroke was cut short");
    }

    /// Bare geometry captured with no paint leaves the stroke's own swatch to
    /// paint it, which is the old behaviour and still the right one there.
    #[test]
    fn geometry_captured_without_paint_says_it_is_unpainted() {
        let stamp = BrushStamp::from_path(square(0.0, 0.0, 10.0)).expect("a stamp");
        assert!(!stamp.is_painted());
        assert!(stamp.place(Affine::scale(10.0))[0].fill.is_none());
    }

    #[test]
    fn degenerate_selections_produce_no_brush() {
        assert!(BrushStamp::capture(&[]).is_none());
        assert!(BrushStamp::from_path(BezPath::new()).is_none());

        // A single point has no area to scale by.
        let mut dot = BezPath::new();
        dot.move_to(Point::new(5.0, 5.0));
        assert!(BrushStamp::from_path(dot).is_none());
    }

    /// A big captured brush stamped hundreds of times has to stay inside a
    /// frame: this runs on every pointer move while the stroke is drawn.
    #[test]
    fn stamping_a_complex_brush_many_times_stays_interactive() {
        // Twelve pieces, as a detailed drawing would be.
        let parts: Vec<(Affine, ShapeData)> = (0..12)
            .map(|i| {
                (
                    Affine::translate((i as f64 * 8.0, 0.0)),
                    ShapeData::filled(square(0.0, 0.0, 10.0), Color::from_rgb8(i * 20, 0x40, 0x80)),
                )
            })
            .collect();
        let stamp = BrushStamp::capture(&parts).expect("a stamp");
        let transforms: Vec<Affine> = (0..600)
            .map(|i| Affine::translate((i as f64 * 12.0, 0.0)) * Affine::scale(24.0))
            .collect();

        let started = std::time::Instant::now();
        let art = stamp.place_many(&transforms);
        let took = started.elapsed();

        assert_eq!(art.shapes.len(), 12);
        assert!(
            took.as_millis() < 16,
            "600 stamps of a 12-piece brush took {took:?}; that is a stutter"
        );
    }
}
