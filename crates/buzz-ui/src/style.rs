//! The current drawing style, and Animate's two drawing models.
//!
//! # Merge Shape versus Object Drawing
//!
//! This is the Animate behaviour newcomers find strangest and long-time users
//! rely on constantly, so it is modelled explicitly rather than left implicit.
//!
//! * **Merge Shape** (the default). Raw shapes on the same layer interact
//!   destructively. Draw a red circle over a red square and they fuse into one
//!   shape. Draw a *blue* circle over the red square and it cuts a
//!   circle-shaped hole out of the red. This is why `buzz-geom`'s boolean
//!   operations exist.
//! * **Object Drawing** (the `J` toggle). Each shape stays a separate object
//!   and simply overlaps.
//!
//! Getting this wrong would make the drawing tools feel like a different
//! program.

use buzz_geom::Rect;
use buzz_scene::{Gradient, GradientKind, GradientStop, Paint};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// How new shapes interact with what is already on the layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DrawingMode {
    /// Shapes merge and cut destructively. Animate's default.
    #[default]
    MergeShape,
    /// Every shape stays its own object.
    ObjectDrawing,
}

impl DrawingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::MergeShape => "Merge Shape",
            Self::ObjectDrawing => "Object Drawing",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::MergeShape => Self::ObjectDrawing,
            Self::ObjectDrawing => Self::MergeShape,
        }
    }
}

/// Stroke style options offered in the Properties panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StrokeKind {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

impl StrokeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Dashed => "Dashed",
            Self::Dotted => "Dotted",
        }
    }
}

/// Animate's Color panel "type": what a new fill is painted with.
///
/// Animate's list also has None and Bitmap fill. None is the `fill_enabled`
/// flag this already had, and bitmaps are not imported at all (PROGRESS.md §7
/// item 22), so they would be a menu entry that could not be honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillKind {
    #[default]
    Solid,
    Linear,
    Radial,
}

impl FillKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Linear => "Linear gradient",
            Self::Radial => "Radial gradient",
        }
    }

    pub fn gradient_kind(self) -> Option<GradientKind> {
        match self {
            Self::Solid => None,
            Self::Linear => Some(GradientKind::Linear),
            Self::Radial => Some(GradientKind::Radial),
        }
    }
}

/// The stroke and fill new shapes are drawn with.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawStyle {
    pub stroke_color: Color,
    pub fill_color: Color,
    /// What a new fill is painted with.
    pub fill_kind: FillKind,
    /// The ramp a new gradient fill is given.
    ///
    /// Held **without a placement**: a gradient in the panel has no shape to
    /// sit on yet, so only the colours, their offsets and the spread mean
    /// anything. The transform is set when the gradient reaches a shape, by
    /// fitting it to that shape's bounds — which is what makes "draw a
    /// rectangle" produce a ramp across the rectangle rather than a ramp
    /// somewhere near the origin.
    pub fill_gradient: Gradient,
    /// `None` means the "no stroke" swatch.
    pub stroke_enabled: bool,
    /// `None` means the "no fill" swatch.
    pub fill_enabled: bool,
    pub stroke_width: f64,
    /// **How wide the eraser rubs**, in document units.
    ///
    /// Its own setting, because it used to be four times the *stroke width*
    /// slider — a number about outlines, defaulting to one, so the eraser was
    /// four units across however large the brush was and nothing in the tool
    /// options said so.
    pub eraser_size: f64,
    /// Animate's hairline: always one pixel, whatever the zoom.
    pub hairline: bool,
    pub stroke_kind: StrokeKind,
    pub drawing_mode: DrawingMode,
    /// Brush tool settings. Kept here rather than on the tool so that
    /// switching tools and coming back does not reset them.
    pub brush: crate::brush::BrushSettings,
    /// Magic Wand settings — tolerance and whether it spreads.
    ///
    /// Beside the brush settings and for the same reason: they belong to the
    /// user's way of working, not to a moment, so switching tools and coming
    /// back does not reset them.
    pub wand: buzz_scene::WandOptions,
    /// Paint Bucket gap closing — how large a gap in the outline the bucket
    /// bridges before filling. Animate's Gap Size.
    pub gap_size: buzz_scene::GapSize,
    /// Symmetry drawing — every stroke mirrored across the stage centre.
    pub symmetry: SymmetrySettings,
    /// Recently used colours, most recent first.
    pub swatches: Vec<Color>,
}

/// How new strokes are mirrored as they are drawn — a mandala/character-symmetry
/// aid. The axes pass through the centre of the stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymmetryMode {
    /// Draw once, no mirroring.
    Off,
    /// Mirror left↔right across the vertical centre line.
    MirrorX,
    /// Mirror top↔bottom across the horizontal centre line.
    MirrorY,
    /// Mirror across both axes at once — four-fold.
    Both,
    /// Rotate the stroke into `n` copies around the centre.
    Radial,
}

impl SymmetryMode {
    pub const ALL: [SymmetryMode; 5] = [
        SymmetryMode::Off,
        SymmetryMode::MirrorX,
        SymmetryMode::MirrorY,
        SymmetryMode::Both,
        SymmetryMode::Radial,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SymmetryMode::Off => "Off",
            SymmetryMode::MirrorX => "Mirror ↔",
            SymmetryMode::MirrorY => "Mirror ↕",
            SymmetryMode::Both => "Mirror ✛",
            SymmetryMode::Radial => "Radial",
        }
    }
}

/// Symmetry mode plus how many copies a radial symmetry makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymmetrySettings {
    pub mode: SymmetryMode,
    /// Number of copies for [`SymmetryMode::Radial`], 2..=24.
    pub radial_count: u32,
}

impl Default for SymmetrySettings {
    fn default() -> Self {
        Self { mode: SymmetryMode::Off, radial_count: 6 }
    }
}

impl SymmetrySettings {
    /// Whether any mirroring is active.
    pub fn is_on(self) -> bool {
        self.mode != SymmetryMode::Off
    }
}

/// Animate's default swatch row.
fn default_swatches() -> Vec<Color> {
    vec![
        Color::BLACK,
        Color::WHITE,
        Color::from_rgb8(0xFF, 0x00, 0x00),
        Color::from_rgb8(0x00, 0xFF, 0x00),
        Color::from_rgb8(0x00, 0x00, 0xFF),
        Color::from_rgb8(0xFF, 0xFF, 0x00),
        Color::from_rgb8(0x00, 0xFF, 0xFF),
        Color::from_rgb8(0xFF, 0x00, 0xFF),
        Color::from_rgb8(0x99, 0x99, 0x99),
        Color::from_rgb8(0xFF, 0x99, 0x00),
    ]
}

impl Default for DrawStyle {
    fn default() -> Self {
        Self {
            stroke_color: Color::BLACK,
            fill_color: Color::from_rgb8(0x00, 0x66, 0xCC),
            fill_kind: FillKind::Solid,
            // The fill colour fading to nothing: switching to a gradient then
            // shows the colour that was already chosen, rather than replacing
            // it with two arbitrary ones.
            fill_gradient: Gradient::new(
                GradientKind::Linear,
                vec![
                    GradientStop::new(0.0, Color::from_rgb8(0x00, 0x66, 0xCC)),
                    GradientStop::new(1.0, Color::WHITE),
                ],
            ),
            stroke_enabled: true,
            fill_enabled: true,
            stroke_width: 1.0,
            // A little wider than the default brush, as a rubber is.
            eraser_size: 16.0,
            hairline: false,
            stroke_kind: StrokeKind::Solid,
            drawing_mode: DrawingMode::default(),
            brush: crate::brush::BrushSettings::default(),
            wand: buzz_scene::WandOptions::default(),
            gap_size: buzz_scene::GapSize::default(),
            symmetry: SymmetrySettings::default(),
            swatches: default_swatches(),
        }
    }
}

impl DrawStyle {
    /// Swap stroke and fill, as Animate's swap arrow does.
    pub fn swap_colors(&mut self) {
        std::mem::swap(&mut self.stroke_color, &mut self.fill_color);
        std::mem::swap(&mut self.stroke_enabled, &mut self.fill_enabled);
    }

    /// Reset to black stroke and white fill, Animate's default button.
    pub fn reset_colors(&mut self) {
        self.stroke_color = Color::BLACK;
        self.fill_color = Color::WHITE;
        self.stroke_enabled = true;
        self.fill_enabled = true;
    }

    /// Remember a colour the user picked.
    pub fn remember(&mut self, color: Color) {
        let key = color.to_rgba8().to_u8_array();
        self.swatches.retain(|c| c.to_rgba8().to_u8_array() != key);
        self.swatches.insert(0, color);
        self.swatches.truncate(24);
    }

    /// Effective stroke for a new shape, if it has one.
    pub fn stroke_for_new_shape(&self) -> Option<(Color, f64, bool)> {
        self.stroke_enabled.then_some((
            self.stroke_color,
            self.stroke_width.max(0.0),
            self.hairline,
        ))
    }

    /// The paint a new shape covering `bounds` should be filled with.
    ///
    /// The bounds are what a gradient needs and a colour does not: a ramp has
    /// to be laid across *something*, and the shape being drawn is the only
    /// sensible thing. Animate does the same — draw a rectangle with a gradient
    /// selected and the ramp spans the rectangle.
    pub fn fill_for_new_shape(&self, bounds: Rect) -> Option<Paint> {
        if !self.fill_enabled {
            return None;
        }
        Some(match self.fill_kind.gradient_kind() {
            None => Paint::Solid(self.fill_color),
            Some(kind) => {
                let mut g = self.fill_gradient.clone();
                g.kind = kind;
                g.fit_to(bounds);
                Paint::Gradient(std::sync::Arc::new(g))
            }
        })
    }

    /// The colour a new fill would use, ignoring gradients.
    ///
    /// For the places that genuinely want one colour — the tool previews drawn
    /// as chrome, and the "can this fuse with what it overlaps" question.
    pub fn fill_color_for_preview(&self) -> Color {
        match self.fill_kind {
            FillKind::Solid => self.fill_color,
            _ => self.fill_gradient.average_color(),
        }
    }

    /// A shape with neither stroke nor fill would be invisible and
    /// unselectable, so the tools refuse to create one.
    pub fn can_draw(&self) -> bool {
        self.stroke_enabled || self.fill_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_animates_starting_state() {
        let s = DrawStyle::default();
        assert_eq!(s.drawing_mode, DrawingMode::MergeShape);
        assert!(s.stroke_enabled && s.fill_enabled);
        assert_eq!(s.stroke_width, 1.0);
        assert!(!s.swatches.is_empty());
    }

    #[test]
    fn swapping_exchanges_both_colour_and_enabled_state() {
        let mut s = DrawStyle {
            stroke_color: Color::from_rgb8(1, 2, 3),
            fill_color: Color::from_rgb8(9, 8, 7),
            fill_enabled: false,
            ..Default::default()
        };

        s.swap_colors();

        assert_eq!(s.stroke_color.to_rgba8().to_u8_array()[0], 9);
        assert_eq!(s.fill_color.to_rgba8().to_u8_array()[0], 1);
        assert!(!s.stroke_enabled, "the disabled flag must travel too");
        assert!(s.fill_enabled);
    }

    #[test]
    fn recent_colours_move_to_the_front_without_duplicating() {
        let mut s = DrawStyle::default();
        let red = Color::from_rgb8(0xFF, 0x00, 0x00);
        let before = s.swatches.len();

        s.remember(red);
        assert_eq!(
            s.swatches[0].to_rgba8().to_u8_array(),
            red.to_rgba8().to_u8_array()
        );
        assert_eq!(
            s.swatches.len(),
            before,
            "an existing colour should move, not be added again"
        );

        let novel = Color::from_rgb8(0x12, 0x34, 0x56);
        s.remember(novel);
        assert_eq!(s.swatches.len(), before + 1);
    }

    #[test]
    fn the_swatch_list_is_bounded() {
        let mut s = DrawStyle::default();
        for i in 0..200u8 {
            s.remember(Color::from_rgb8(i, i, i));
        }
        assert!(
            s.swatches.len() <= 24,
            "swatches grew to {}",
            s.swatches.len()
        );
    }

    #[test]
    fn a_shape_with_no_stroke_and_no_fill_is_refused() {
        let mut s = DrawStyle::default();
        assert!(s.can_draw());

        s.stroke_enabled = false;
        assert!(s.can_draw(), "a fill alone is fine");

        s.fill_enabled = false;
        assert!(!s.can_draw(), "neither stroke nor fill would be invisible");
    }

    #[test]
    fn new_shape_style_respects_the_disabled_swatches() {
        let mut s = DrawStyle::default();
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert!(s.stroke_for_new_shape().is_some());
        assert!(s.fill_for_new_shape(area).is_some());

        s.stroke_enabled = false;
        assert!(s.stroke_for_new_shape().is_none());

        s.fill_enabled = false;
        assert!(s.fill_for_new_shape(area).is_none());
    }

    /// A gradient fill is laid across the shape being drawn, not left at the
    /// origin — which is what makes drawing a rectangle with a gradient
    /// selected produce a ramp across that rectangle.
    #[test]
    fn a_new_gradient_fill_is_fitted_to_the_shape() {
        let s = DrawStyle {
            fill_kind: FillKind::Linear,
            ..Default::default()
        };

        let area = Rect::new(100.0, 200.0, 300.0, 400.0);
        let paint = s.fill_for_new_shape(area).expect("filled");
        let g = paint.gradient().expect("it should be a gradient");
        let h = g.handles();

        assert!((h.center.x - 200.0).abs() < 1e-9, "centre {:?}", h.center);
        assert!((h.center.y - 300.0).abs() < 1e-9, "centre {:?}", h.center);
        assert!(
            (h.end.x - 300.0).abs() < 1e-9,
            "the ramp should reach the right edge"
        );
    }

    /// Switching the type switches the paint, and switching back gets the
    /// solid colour that was there before rather than a colour from the ramp.
    #[test]
    fn the_fill_type_selects_between_a_colour_and_a_ramp() {
        let mut s = DrawStyle::default();
        let area = Rect::new(0.0, 0.0, 10.0, 10.0);
        let solid = s.fill_for_new_shape(area).expect("filled");
        assert!(!solid.is_gradient());

        s.fill_kind = FillKind::Radial;
        let radial = s.fill_for_new_shape(area).expect("filled");
        assert_eq!(
            radial.gradient().map(|g| g.kind),
            Some(GradientKind::Radial),
            "the panel's kind must reach the shape"
        );

        s.fill_kind = FillKind::Solid;
        assert_eq!(s.fill_for_new_shape(area), Some(solid));
    }

    #[test]
    fn drawing_mode_toggles_both_ways() {
        assert_eq!(
            DrawingMode::MergeShape.toggled(),
            DrawingMode::ObjectDrawing
        );
        assert_eq!(
            DrawingMode::ObjectDrawing.toggled(),
            DrawingMode::MergeShape
        );
        assert_eq!(DrawingMode::MergeShape.label(), "Merge Shape");
    }

    #[test]
    fn resetting_restores_black_on_white() {
        let mut s = DrawStyle {
            stroke_enabled: false,
            fill_enabled: false,
            ..Default::default()
        };
        s.reset_colors();

        assert!(s.stroke_enabled && s.fill_enabled);
        assert_eq!(s.stroke_color.to_rgba8().to_u8_array()[0], 0);
        assert_eq!(s.fill_color.to_rgba8().to_u8_array()[0], 255);
    }
}
