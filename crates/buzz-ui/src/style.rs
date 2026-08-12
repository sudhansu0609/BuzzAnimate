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

/// The stroke and fill new shapes are drawn with.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawStyle {
    pub stroke_color: Color,
    pub fill_color: Color,
    /// `None` means the "no stroke" swatch.
    pub stroke_enabled: bool,
    /// `None` means the "no fill" swatch.
    pub fill_enabled: bool,
    pub stroke_width: f64,
    /// Animate's hairline: always one pixel, whatever the zoom.
    pub hairline: bool,
    pub stroke_kind: StrokeKind,
    pub drawing_mode: DrawingMode,
    /// Brush tool settings. Kept here rather than on the tool so that
    /// switching tools and coming back does not reset them.
    pub brush: crate::brush::BrushSettings,
    /// Recently used colours, most recent first.
    pub swatches: Vec<Color>,
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
            stroke_enabled: true,
            fill_enabled: true,
            stroke_width: 1.0,
            hairline: false,
            stroke_kind: StrokeKind::Solid,
            drawing_mode: DrawingMode::default(),
            brush: crate::brush::BrushSettings::default(),
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
        self.stroke_enabled
            .then_some((self.stroke_color, self.stroke_width.max(0.0), self.hairline))
    }

    pub fn fill_for_new_shape(&self) -> Option<Color> {
        self.fill_enabled.then_some(self.fill_color)
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
        assert_eq!(s.swatches[0].to_rgba8().to_u8_array(), red.to_rgba8().to_u8_array());
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
        assert!(s.swatches.len() <= 24, "swatches grew to {}", s.swatches.len());
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
        assert!(s.stroke_for_new_shape().is_some());
        assert!(s.fill_for_new_shape().is_some());

        s.stroke_enabled = false;
        assert!(s.stroke_for_new_shape().is_none());
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
