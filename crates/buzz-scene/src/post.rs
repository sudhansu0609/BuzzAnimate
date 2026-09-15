//! The film's look: the full-frame compositor settings.
//!
//! # Why these live on the scene, not on the export
//!
//! Bloom, grain, a vignette and a colour grade are the *look of the film*. An
//! animator sets them while watching the stage, the way a colourist works on a
//! monitor rather than by exporting and looking. So they belong to the document
//! and are visible on the stage; the exporter reads the very same settings, and
//! [`buzz_render`]'s compositor is the one implementation both go through — the
//! stage and the finished frame cannot drift apart because there is nothing to
//! drift from.
//!
//! # Why every field is a plain number
//!
//! [`PostSettings`] is `Copy`, which keeps [`crate::StageProperties`] `Copy` —
//! stage setup is passed by value all over the shell, and a heap-allocating
//! field there would be a quiet cost on a hot path. Every setting here is a
//! `bool`, an `f32`, or a [`Color`] (itself `Copy`), so the whole bundle stays
//! a handful of words on the stack.
//!
//! # The disabled default
//!
//! [`PostSettings::default`] is `enabled: false`, and a disabled compositor is
//! a bit-exact passthrough (asserted by a render test). So a document that never
//! touches the Effects panel looks exactly as it did before this existed, and an
//! older file — which carries no post settings at all — loads to this default.

use peniko::Color;
use serde::{Deserialize, Serialize};

/// The full-frame post-processing applied after the artwork is rendered.
///
/// A single master `enabled` gates the whole chain; each pass then has its own
/// switch, so an animator can dial in a grade with bloom off, or preview bloom
/// alone, without losing the settings of the passes they are not looking at.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PostSettings {
    /// Master switch. When `false` the compositor is a straight passthrough and
    /// nothing below is read. `false` is the derived default.
    pub enabled: bool,
    pub bloom: BloomSettings,
    pub grade: GradeSettings,
    pub vignette: VignetteSettings,
    pub grain: GrainSettings,
    #[serde(default)]
    pub posterise: PosteriseSettings,
    #[serde(default)]
    pub halftone: HalftoneSettings,
    #[serde(default)]
    pub hatching: HatchingSettings,
}

impl PostSettings {
    /// Does this actually change any pixel? False when the master switch is off,
    /// or on but with every pass either disabled or at its neutral value. The
    /// compositor uses this to skip its whole chain and blit straight through.
    pub fn is_identity(&self) -> bool {
        !self.enabled
            || (!self.bloom.enabled
                && self.grade.is_neutral()
                && !self.vignette.enabled
                && !self.grain.enabled
                && !self.posterise.enabled
                && !self.halftone.enabled
                && !self.hatching.enabled)
    }
}

/// Posterise: flatten each colour channel to a few levels, for a graphic,
/// screen-printed look.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PosteriseSettings {
    pub enabled: bool,
    /// Levels per channel, 2..=16. Fewer is flatter.
    pub levels: u32,
}

impl Default for PosteriseSettings {
    fn default() -> Self {
        Self { enabled: false, levels: 6 }
    }
}

/// Halftone: render the frame as dots whose size follows the brightness, on
/// white — the comic-book / newsprint look.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HalftoneSettings {
    pub enabled: bool,
    /// Dot cell size in output pixels. Larger is coarser.
    pub scale: f32,
}

impl Default for HalftoneSettings {
    fn default() -> Self {
        Self { enabled: false, scale: 6.0 }
    }
}

/// Hatching: parallel ink lines whose density follows the darkness, on white —
/// a pen-and-ink cross-hatch look.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HatchingSettings {
    pub enabled: bool,
    /// Line spacing in output pixels. Larger is coarser.
    pub scale: f32,
}

impl Default for HatchingSettings {
    fn default() -> Self {
        Self { enabled: false, scale: 6.0 }
    }
}

/// Bright-pass bloom: light bleeds out of the brightest parts of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BloomSettings {
    pub enabled: bool,
    /// Luminance above which a pixel contributes to the bloom, in the 0..1 range
    /// of the rendered frame. 0.8 keeps highlights blooming and midtones clean.
    pub threshold: f32,
    /// How strongly the blurred highlights are added back. 0 is invisible, 1 is
    /// strong.
    pub intensity: f32,
    /// The spread of the blur, 0..1, mapped to the number of dual-Kawase steps.
    pub radius: f32,
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 0.8,
            intensity: 0.5,
            radius: 0.5,
        }
    }
}

/// The colour grade: exposure, contrast, saturation, white balance and a
/// simplified lift/gamma/gain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradeSettings {
    pub enabled: bool,
    /// Exposure in stops. 0 is neutral; +1 doubles brightness.
    pub exposure: f32,
    /// Contrast about mid-grey. 1 is neutral.
    pub contrast: f32,
    /// Saturation. 1 is neutral, 0 is greyscale.
    pub saturation: f32,
    /// White balance, warm(+) to cool(−), −1..1.
    pub temperature: f32,
    /// Green(−) to magenta(+) tint, −1..1.
    pub tint: f32,
    /// Lift: raises the shadows. 0 is neutral.
    pub lift: f32,
    /// Gamma: bends the midtones. 1 is neutral.
    pub gamma: f32,
    /// Gain: scales the highlights. 1 is neutral.
    pub gain: f32,
}

impl Default for GradeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            lift: 0.0,
            gamma: 1.0,
            gain: 1.0,
        }
    }
}

impl GradeSettings {
    /// A grade that leaves every pixel untouched — either switched off, or on
    /// but with every control at its neutral value.
    pub fn is_neutral(&self) -> bool {
        !self.enabled
            || (self.exposure == 0.0
                && self.contrast == 1.0
                && self.saturation == 1.0
                && self.temperature == 0.0
                && self.tint == 0.0
                && self.lift == 0.0
                && self.gamma == 1.0
                && self.gain == 1.0)
    }
}

/// A vignette: the corners darken towards a colour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VignetteSettings {
    pub enabled: bool,
    /// How dark the corners go, 0..1.
    pub amount: f32,
    /// How gradually the darkening comes in, 0..1. Higher is softer.
    pub softness: f32,
    /// The colour the corners fall towards. Black is the ordinary cinematic
    /// vignette; a warm dark is sometimes wanted.
    pub color: Color,
}

impl Default for VignetteSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            amount: 0.3,
            softness: 0.5,
            color: Color::BLACK,
        }
    }
}

/// Film grain: a deterministic per-pixel, per-frame dither.
///
/// The grain is a hash of the pixel and the frame index, so two renders of the
/// same frame are identical — an export is reproducible, and the render test can
/// assert bit-equality.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GrainSettings {
    pub enabled: bool,
    /// Strength of the grain, 0..1.
    pub amount: f32,
    /// Grain cell size in output pixels. 1 is per-pixel; larger is coarser.
    pub size: f32,
}

impl Default for GrainSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            amount: 0.05,
            size: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        assert!(PostSettings::default().is_identity());
    }

    #[test]
    fn enabled_but_all_passes_neutral_is_still_identity() {
        let mut post = PostSettings::default();
        post.enabled = true;
        assert!(
            post.is_identity(),
            "master on with every pass off should still touch no pixel"
        );
    }

    #[test]
    fn one_active_pass_is_not_identity() {
        let mut post = PostSettings::default();
        post.enabled = true;
        post.vignette.enabled = true;
        assert!(!post.is_identity());
    }

    #[test]
    fn a_neutral_but_enabled_grade_is_neutral() {
        let mut grade = GradeSettings::default();
        grade.enabled = true;
        assert!(grade.is_neutral());
        grade.contrast = 1.2;
        assert!(!grade.is_neutral());
    }
}
