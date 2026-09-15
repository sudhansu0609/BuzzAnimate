//! One piece of artwork a brush lays down.
//!
//! Two brushes here produce more than a single filled outline — the effect
//! brushes ([`crate::effect_brush`]) and a brush captured from artwork
//! ([`crate::stamp`]) — and both produce the same two kinds of thing: a
//! vector shape, or a patch of painted pixels that has to become a bitmap
//! before it can be drawn. This is that pair, in one place, so the preview,
//! the commit and the tests all develop a piece the same way.
//!
//! # Why the bitmap is not resolved here
//!
//! A painted piece cannot turn itself into a [`ShapeData`]: the pixels have
//! to be registered somewhere that can issue an identity for them, and the
//! two callers want different somewheres. The editor puts them in the
//! document's image library, where they are saved with the file; a live
//! preview issues a throwaway identity sixty times a second and would fill
//! that library with rubbish. So [`ArtPiece::to_shape`] takes the registrar
//! as an argument, and neither caller has to know how the other works.

use std::sync::Arc;

use buzz_geom::{Rect, Shape as _};

use crate::image::{ImageAsset, ImageFill};
use crate::object::{FillSpec, PaintBlend, ShapeData};
use crate::raster::{Canvas, SoftBrush};

/// A shape, or the pixels of one.
///
/// Order matters wherever these are produced in a list: they are drawn and
/// committed first-to-last, so a glow that belongs behind a lamp post comes
/// earlier.
#[derive(Debug, Clone, PartialEq)]
pub enum ArtPiece {
    /// Vector artwork: an ordinary shape.
    Shape(ShapeData),
    /// Painted pixels, to become a bitmap-filled shape when developed.
    ///
    /// The brush carries the colour and flow the coverage is developed with,
    /// exactly as a soft-brush stroke's own commit does.
    Painting {
        canvas: Canvas,
        brush: SoftBrush,
        blend: PaintBlend,
    },
}

impl ArtPiece {
    /// The drawable shape this piece becomes.
    ///
    /// `register` turns a freshly painted canvas into a shared asset. See the
    /// module header for why that is the caller's job.
    pub fn to_shape(
        &self,
        register: &mut dyn FnMut(&Canvas, &SoftBrush) -> Arc<ImageAsset>,
    ) -> ShapeData {
        match self {
            Self::Shape(shape) => shape.clone(),
            Self::Painting {
                canvas,
                brush,
                blend,
            } => {
                let area = canvas.area();
                let mut fill = ImageFill::new(register(canvas, brush), area);
                // The canvas is already at the document's own pixel scale, so
                // it is drawn one painted pixel to one document unit.
                // Smoothing it would blur paint against the grid it was
                // painted on.
                fill.smooth = false;
                ShapeData {
                    path: buzz_geom::Shape::to_path(&area, 1e-9),
                    fill: Some(FillSpec::image(fill)),
                    stroke: None,
                    blend: *blend,
                }
            }
        }
    }

    /// Where this piece sits, without developing it.
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Shape(shape) => shape.path.bounding_box(),
            Self::Painting { canvas, .. } => canvas.area(),
        }
    }

    /// Does this piece add light rather than paint over what is under it?
    pub fn is_additive(&self) -> bool {
        match self {
            Self::Shape(shape) => shape.blend.is_additive(),
            Self::Painting { blend, .. } => blend.is_additive(),
        }
    }
}

/// Develop a run of pieces with one registrar. See [`ArtPiece::to_shape`].
pub fn to_shapes(
    pieces: &[ArtPiece],
    register: &mut dyn FnMut(&Canvas, &SoftBrush) -> Arc<ImageAsset>,
) -> Vec<ShapeData> {
    pieces.iter().map(|p| p.to_shape(register)).collect()
}
