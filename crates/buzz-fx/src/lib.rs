//! Animate's filters — Blur, Drop Shadow, Glow, Bevel and Adjust Color.
//!
//! # Why these are geometry and not a raster pass
//!
//! In Animate a filter is a raster effect: the movie clip is rendered to a
//! surface and the surface is blurred. That is the obvious implementation, and
//! it is the wrong one here, for the same reason lighting is not shaded per
//! pixel (see `buzz-light`): Vello offers no shader hook, and a raster
//! post-pass would throw away the one property this project is built on —
//! artwork that survives 10¹⁴% zoom. A blur baked at 100% is a smear at 10 000%.
//!
//! So a filter is expressed as **paths**:
//!
//! * a **soft edge** is a fill plus a ramp of concentric strokes, each a little
//!   wider and a little more transparent, which is a blur of the silhouette
//!   with no booleans and no buffers;
//! * a **drop shadow** is that soft edge, in the shadow's colour, offset by the
//!   light's angle and distance, drawn behind;
//! * a **glow** is the same thing centred, inside or out;
//! * a **bevel** is a highlight and a shadow along the edge, clipped to the
//!   shape, which is what an edge lit from one side looks like;
//! * a **blur** is a stack of offset copies of the artwork from shrunk to
//!   grown, which is the one effect that does cost booleans;
//! * **Adjust Color** is exact arithmetic on the colours themselves.
//!
//! Everything stays vector: it zooms, it exports, and it is still there when
//! the document is reopened.
//!
//! # What this cannot do
//!
//! A real blur mixes a shape with what is *inside* it. These build from the
//! outline, so a two-colour drawing blurs each shape against its own edge
//! rather than into its neighbour. For flat vector artwork — which is what this
//! program makes — that is very close. It is recorded as a limitation rather
//! than sold as parity.

use peniko::Color;
use serde::{Deserialize, Serialize};

mod geometry;

pub use geometry::{Op, Painted, blur_ops, build, soft_edge};

/// How many bands a soft edge is built from.
///
/// Animate's own Low/Medium/High, and for the same reason: quality costs
/// geometry, and a shadow behind a moving character does not need the bands a
/// still title card does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Quality {
    Low,
    #[default]
    Medium,
    High,
}

impl Quality {
    /// Bands in the ramp.
    pub fn bands(self) -> usize {
        match self {
            Self::Low => 4,
            Self::Medium => 8,
            Self::High => 14,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    pub const ALL: [Quality; 3] = [Quality::Low, Quality::Medium, Quality::High];
}

/// Where a bevel's light falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BevelKind {
    /// Inside the shape's edge — Animate's default.
    #[default]
    Inner,
    /// Outside it.
    Outer,
    /// Both.
    Full,
}

impl BevelKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inner => "Inner",
            Self::Outer => "Outer",
            Self::Full => "Full",
        }
    }

    pub const ALL: [BevelKind; 3] = [BevelKind::Inner, BevelKind::Outer, BevelKind::Full];
}

/// One filter, and whether it is switched on.
///
/// The enable flag is Animate's: a filter you are experimenting with is turned
/// off, not deleted, so its settings survive being reconsidered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub kind: FilterKind,
    pub enabled: bool,
}

impl Filter {
    pub fn new(kind: FilterKind) -> Self {
        Self {
            kind,
            enabled: true,
        }
    }

    pub fn label(&self) -> &'static str {
        self.kind.label()
    }

    /// Interpolate two filters for a motion tween, as Animate does.
    ///
    /// Only filters of the **same kind** interpolate; anything else holds the
    /// starting value. Animate has the same rule, and for the same reason:
    /// there is no halfway point between a blur and a bevel, and inventing one
    /// would look like a bug rather than an effect.
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        Self {
            kind: self.kind.lerp(&other.kind, t),
            enabled: self.enabled,
        }
    }
}

/// The filters Animate offers, with the parameters it offers for them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilterKind {
    /// Soften the artwork itself.
    Blur {
        /// Radius across and down, separately — Animate's Blur X and Blur Y.
        x: f64,
        y: f64,
        quality: Quality,
    },

    /// A shadow of the artwork's silhouette, thrown behind it.
    DropShadow {
        x: f64,
        y: f64,
        /// How dark, `0..=1`. Animate's Strength is a percentage.
        strength: f64,
        /// Direction, in radians.
        angle: f64,
        /// How far the shadow is thrown.
        distance: f64,
        color: Color,
        /// Draw the shadow *inside* the shape rather than behind it.
        inner: bool,
        /// Hide the artwork and keep only the shadow.
        knockout: bool,
        /// Animate's "Hide object": the shadow alone, with nothing casting it.
        hide_object: bool,
        quality: Quality,
    },

    /// A halo, outside the artwork or inside its edge.
    Glow {
        x: f64,
        y: f64,
        strength: f64,
        color: Color,
        inner: bool,
        knockout: bool,
        quality: Quality,
    },

    /// A lit edge: highlight towards the light, shadow away from it.
    Bevel {
        x: f64,
        y: f64,
        strength: f64,
        angle: f64,
        distance: f64,
        highlight: Color,
        shadow: Color,
        kind: BevelKind,
        knockout: bool,
        quality: Quality,
    },

    /// Brightness, contrast, saturation and hue — exact, and free.
    Adjust(ColorAdjust),

    /// Recolour by brightness: dark tones take the `shadow` colour, light tones
    /// the `highlight`, everything between a blend of the two. A duotone
    /// gradient map — sepia, cyanotype, and every two-colour graded look.
    GradientMap(GradientMap),
}

/// A duotone gradient map: luminance 0 maps to `shadow`, luminance 1 to
/// `highlight`, linearly between. Alpha is left untouched, so a cut-out stays
/// cut out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientMap {
    pub shadow: Color,
    pub highlight: Color,
}

impl Default for GradientMap {
    fn default() -> Self {
        // A warm sepia, the most recognisable gradient map.
        Self {
            shadow: Color::from_rgb8(0x2B, 0x1D, 0x10),
            highlight: Color::from_rgb8(0xF2, 0xE6, 0xCF),
        }
    }
}

impl GradientMap {
    /// Remap one colour by its luminance, keeping its alpha.
    pub fn apply(&self, c: Color) -> Color {
        let [r, g, b, a] = c.to_rgba8().to_u8_array();
        // Rec. 601 luma, which is what an eye reads as brightness.
        let l = (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64) / 255.0;
        let mix = |lo: u8, hi: u8| (lo as f64 + (hi as f64 - lo as f64) * l).round() as u8;
        let [sr, sg, sb, _] = self.shadow.to_rgba8().to_u8_array();
        let [hr, hg, hb, _] = self.highlight.to_rgba8().to_u8_array();
        Color::from_rgba8(mix(sr, hr), mix(sg, hg), mix(sb, hb), a)
    }

    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        Self {
            shadow: lerp_color(self.shadow, other.shadow, t),
            highlight: lerp_color(self.highlight, other.highlight, t),
        }
    }
}

impl FilterKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Blur { .. } => "Blur",
            Self::DropShadow { .. } => "Drop Shadow",
            Self::Glow { .. } => "Glow",
            Self::Bevel { .. } => "Bevel",
            Self::Adjust(_) => "Adjust Color",
            Self::GradientMap(_) => "Gradient Map",
        }
    }

    pub fn gradient_map() -> Self {
        Self::GradientMap(GradientMap::default())
    }

    /// Animate's defaults, which are the ones an animator's hands expect.
    pub fn blur() -> Self {
        Self::Blur {
            x: 5.0,
            y: 5.0,
            quality: Quality::default(),
        }
    }

    pub fn drop_shadow() -> Self {
        Self::DropShadow {
            x: 5.0,
            y: 5.0,
            strength: 1.0,
            // 45°, which is where Animate puts it.
            angle: std::f64::consts::FRAC_PI_4,
            distance: 5.0,
            color: Color::BLACK,
            inner: false,
            knockout: false,
            hide_object: false,
            quality: Quality::default(),
        }
    }

    pub fn glow() -> Self {
        Self::Glow {
            x: 5.0,
            y: 5.0,
            strength: 1.0,
            color: Color::from_rgb8(0xFF, 0x00, 0x00),
            inner: false,
            knockout: false,
            quality: Quality::default(),
        }
    }

    pub fn bevel() -> Self {
        Self::Bevel {
            x: 5.0,
            y: 5.0,
            strength: 1.0,
            angle: std::f64::consts::FRAC_PI_4,
            distance: 5.0,
            highlight: Color::WHITE,
            shadow: Color::BLACK,
            kind: BevelKind::default(),
            knockout: false,
            quality: Quality::default(),
        }
    }

    pub fn adjust() -> Self {
        Self::Adjust(ColorAdjust::default())
    }

    /// Interpolate with another filter of the same kind.
    ///
    /// Colours and numbers mix; flags, quality and bevel type take the
    /// starting value, because there is no half of "knockout".
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * t;

        match (self, other) {
            (
                Self::Blur { x, y, quality },
                Self::Blur {
                    x: bx,
                    y: by,
                    quality: _,
                },
            ) => Self::Blur {
                x: mix(*x, *bx),
                y: mix(*y, *by),
                quality: *quality,
            },

            (
                Self::DropShadow {
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
                },
                Self::DropShadow {
                    x: bx,
                    y: by,
                    strength: bs,
                    angle: ba,
                    distance: bd,
                    color: bc,
                    ..
                },
            ) => Self::DropShadow {
                x: mix(*x, *bx),
                y: mix(*y, *by),
                strength: mix(*strength, *bs),
                // The shortest way round, so a shadow swinging past due east
                // does not spin all the way back through west.
                angle: angle + shortest_turn(*angle, *ba) * t,
                distance: mix(*distance, *bd),
                color: lerp_color(*color, *bc, t),
                inner: *inner,
                knockout: *knockout,
                hide_object: *hide_object,
                quality: *quality,
            },

            (
                Self::Glow {
                    x,
                    y,
                    strength,
                    color,
                    inner,
                    knockout,
                    quality,
                },
                Self::Glow {
                    x: bx,
                    y: by,
                    strength: bs,
                    color: bc,
                    ..
                },
            ) => Self::Glow {
                x: mix(*x, *bx),
                y: mix(*y, *by),
                strength: mix(*strength, *bs),
                color: lerp_color(*color, *bc, t),
                inner: *inner,
                knockout: *knockout,
                quality: *quality,
            },

            (
                Self::Bevel {
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
                },
                Self::Bevel {
                    x: bx,
                    y: by,
                    strength: bs,
                    angle: ba,
                    distance: bd,
                    highlight: bh,
                    shadow: bsh,
                    ..
                },
            ) => Self::Bevel {
                x: mix(*x, *bx),
                y: mix(*y, *by),
                strength: mix(*strength, *bs),
                angle: angle + shortest_turn(*angle, *ba) * t,
                distance: mix(*distance, *bd),
                highlight: lerp_color(*highlight, *bh, t),
                shadow: lerp_color(*shadow, *bsh, t),
                kind: *kind,
                knockout: *knockout,
                quality: *quality,
            },

            (Self::Adjust(a), Self::Adjust(b)) => Self::Adjust(a.lerp(b, t)),

            (Self::GradientMap(a), Self::GradientMap(b)) => Self::GradientMap(a.lerp(b, t)),

            // Different kinds: hold the start, as Animate does.
            _ => self.clone(),
        }
    }

    /// One of each, for a menu.
    pub fn all() -> Vec<Self> {
        vec![
            Self::blur(),
            Self::drop_shadow(),
            Self::glow(),
            Self::bevel(),
            Self::adjust(),
            Self::gradient_map(),
        ]
    }

    /// How far outside the artwork this filter paints, in document units.
    ///
    /// The renderer needs it to size the region it isolates and to decide
    /// whether a filtered object is on screen at all.
    pub fn reach(&self) -> f64 {
        match self {
            Self::Blur { x, y, .. } => x.max(*y),
            Self::DropShadow {
                x,
                y,
                distance,
                inner,
                ..
            } => {
                if *inner {
                    0.0
                } else {
                    x.max(*y) + distance.abs()
                }
            }
            Self::Glow { x, y, inner, .. } => {
                if *inner {
                    0.0
                } else {
                    x.max(*y)
                }
            }
            Self::Bevel {
                x,
                y,
                distance,
                kind,
                ..
            } => match kind {
                BevelKind::Inner => 0.0,
                _ => x.max(*y) + distance.abs(),
            },
            Self::Adjust(_) => 0.0,
            Self::GradientMap(_) => 0.0,
        }
    }
}

/// How an object combines with what is already painted — Animate's Blend list.
///
/// # What is missing, and why
///
/// Animate also offers Subtract, Invert, Alpha and Erase. Those are Flash's
/// own compositing operators, not Porter–Duff or the PDF/CSS mixing modes, and
/// there is nothing in Vello to express them: Alpha and Erase use the *parent*
/// clip's alpha as a mask, which is a whole compositing model rather than one
/// blend equation. They are recorded as missing (§7) rather than silently
/// mapped onto something that looks nearly right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Blend {
    #[default]
    Normal,
    /// Composite the object as its own group first. On its own it looks like
    /// Normal; what it changes is that the *children* blend with each other
    /// and not with the stage behind them.
    Layer,
    Darken,
    Multiply,
    Lighten,
    Screen,
    Overlay,
    HardLight,
    /// Sums with what is behind, which is how light and fire are drawn.
    Add,
    Difference,
}

impl Blend {
    /// **The colour that changes nothing** under this blend.
    ///
    /// White for a multiply, black for anything that lightens: painting it
    /// leaves the destination exactly as it was. That is what lets a pass be
    /// *faded out* from the inside — draw the effect, then paint towards the
    /// identity where it should not reach — without needing to erase, which
    /// costs a compositing mode of its own and therefore a layer per step.
    ///
    /// `None` for a blend with no such colour: under Normal every colour is
    /// drawn as itself, and the only way to change nothing is to draw nothing.
    pub fn identity(self) -> Option<peniko::Color> {
        match self {
            // `dst × 1` and `min(dst, 1)`.
            Self::Multiply | Self::Darken => Some(peniko::Color::WHITE),
            // `1 − (1 − dst)(1 − 0)`, `max(dst, 0)`, and `dst + 0`.
            Self::Screen | Self::Lighten | Self::Add => Some(peniko::Color::BLACK),
            // `|dst − 0|`.
            Self::Difference => Some(peniko::Color::BLACK),
            // Overlay and hard light take mid grey to themselves only where the
            // destination is mid grey; there is no one colour that is inert.
            Self::Normal | Self::Layer | Self::Overlay | Self::HardLight => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Layer => "Layer",
            Self::Darken => "Darken",
            Self::Multiply => "Multiply",
            Self::Lighten => "Lighten",
            Self::Screen => "Screen",
            Self::Overlay => "Overlay",
            Self::HardLight => "Hard Light",
            Self::Add => "Add",
            Self::Difference => "Difference",
        }
    }

    /// Does this need the object drawn into its own group?
    ///
    /// Everything except Normal does: a blend equation needs a backdrop to
    /// blend *with*, and without a group that backdrop is the whole stage.
    pub fn needs_group(self) -> bool {
        self != Self::Normal
    }

    pub const ALL: [Blend; 10] = [
        Blend::Normal,
        Blend::Layer,
        Blend::Darken,
        Blend::Multiply,
        Blend::Lighten,
        Blend::Screen,
        Blend::Overlay,
        Blend::HardLight,
        Blend::Add,
        Blend::Difference,
    ];
}

/// Brightness, contrast, saturation and hue, as Animate's Adjust Color panel
/// offers them.
///
/// Every one is a percentage in `-100..=100` except hue, which is degrees in
/// `-180..=180`, because those are the numbers on the sliders an animator has
/// used before.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ColorAdjust {
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub hue: f64,
}

impl ColorAdjust {
    /// Does this change anything at all?
    pub fn is_identity(&self) -> bool {
        self.brightness == 0.0 && self.contrast == 0.0 && self.saturation == 0.0 && self.hue == 0.0
    }

    /// Apply it to one colour.
    ///
    /// Worked in sRGB components rather than linear light, deliberately: these
    /// are Flash's controls, Flash applied them to 8-bit sRGB, and an animator
    /// who types 50 into Brightness expects what Animate shows, not what a
    /// physically-correct pipeline would give.
    pub fn apply(&self, color: Color) -> Color {
        if self.is_identity() {
            return color;
        }
        let [r, g, b, a] = color.to_rgba8().to_u8_array();
        let (mut r, mut g, mut b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);

        // Brightness: a straight offset, as Flash's is.
        if self.brightness != 0.0 {
            let offset = self.brightness.clamp(-100.0, 100.0) / 100.0;
            r += offset;
            g += offset;
            b += offset;
        }

        // Contrast: pivot about mid grey. The factor is the usual photographic
        // one, so -100 collapses to flat grey and +100 is hard.
        if self.contrast != 0.0 {
            let c = self.contrast.clamp(-100.0, 100.0) * 2.55;
            let factor = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
            r = (r - 0.5) * factor + 0.5;
            g = (g - 0.5) * factor + 0.5;
            b = (b - 0.5) * factor + 0.5;
        }

        // Saturation: mix towards or away from the luminance of the colour.
        if self.saturation != 0.0 {
            let amount = 1.0 + self.saturation.clamp(-100.0, 100.0) / 100.0;
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            r = luma + (r - luma) * amount;
            g = luma + (g - luma) * amount;
            b = luma + (b - luma) * amount;
        }

        // Hue: the standard rotation about the grey axis.
        if self.hue != 0.0 {
            let angle = self.hue.clamp(-180.0, 180.0).to_radians();
            let (sin, cos) = angle.sin_cos();
            let m = [
                0.213 + cos * 0.787 - sin * 0.213,
                0.715 - cos * 0.715 - sin * 0.715,
                0.072 - cos * 0.072 + sin * 0.928,
                0.213 - cos * 0.213 + sin * 0.143,
                0.715 + cos * 0.285 + sin * 0.140,
                0.072 - cos * 0.072 - sin * 0.283,
                0.213 - cos * 0.213 - sin * 0.787,
                0.715 - cos * 0.715 + sin * 0.715,
                0.072 + cos * 0.928 + sin * 0.072,
            ];
            let (nr, ng, nb) = (
                m[0] * r + m[1] * g + m[2] * b,
                m[3] * r + m[4] * g + m[5] * b,
                m[6] * r + m[7] * g + m[8] * b,
            );
            r = nr;
            g = ng;
            b = nb;
        }

        Color::from_rgba8(
            (r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (b.clamp(0.0, 1.0) * 255.0).round() as u8,
            a,
        )
    }

    /// Interpolate, so a motion tween can animate a filter as Animate does.
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * t;
        Self {
            brightness: mix(self.brightness, other.brightness),
            contrast: mix(self.contrast, other.contrast),
            saturation: mix(self.saturation, other.saturation),
            hue: mix(self.hue, other.hue),
        }
    }
}

/// The shortest signed turn from `from` to `to`, in radians.
fn shortest_turn(from: f64, to: f64) -> f64 {
    let full = std::f64::consts::TAU;
    let mut delta = (to - from) % full;
    if delta > full / 2.0 {
        delta -= full;
    } else if delta < -full / 2.0 {
        delta += full;
    }
    delta
}

/// Mix two colours, in sRGB components, so a tween between two filter colours
/// passes through the shades an animator picked between them.
fn lerp_color(a: Color, b: Color, t: f64) -> Color {
    let [ar, ag, ab, aa] = a.to_rgba8().to_u8_array();
    let [br, bg, bb, ba] = b.to_rgba8().to_u8_array();
    let mix = |x: u8, y: u8| {
        (x as f64 + (y as f64 - x as f64) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::from_rgba8(mix(ar, br), mix(ag, bg), mix(ab, bb), mix(aa, ba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_map_sends_black_to_shadow_and_white_to_highlight() {
        let map = GradientMap {
            shadow: Color::from_rgb8(0x10, 0x20, 0x30),
            highlight: Color::from_rgb8(0xF0, 0xE0, 0xD0),
        };
        assert_eq!(map.apply(Color::BLACK).to_rgba8().to_u8_array()[..3], [0x10, 0x20, 0x30]);
        assert_eq!(map.apply(Color::WHITE).to_rgba8().to_u8_array()[..3], [0xF0, 0xE0, 0xD0]);
        // Alpha rides through untouched, so a cut-out stays cut out.
        let translucent = Color::from_rgba8(0, 0, 0, 0x80);
        assert_eq!(map.apply(translucent).to_rgba8().to_u8_array()[3], 0x80);
    }

    #[test]
    fn nothing_set_changes_nothing() {
        let adjust = ColorAdjust::default();
        assert!(adjust.is_identity());
        for colour in [Color::BLACK, Color::WHITE, Color::from_rgb8(3, 200, 90)] {
            assert_eq!(adjust.apply(colour), colour);
        }
    }

    #[test]
    fn brightness_lifts_and_lowers() {
        let mid = Color::from_rgb8(128, 128, 128);
        let up = ColorAdjust {
            brightness: 30.0,
            ..Default::default()
        }
        .apply(mid);
        let down = ColorAdjust {
            brightness: -30.0,
            ..Default::default()
        }
        .apply(mid);

        assert!(up.to_rgba8().r > 128, "{up:?}");
        assert!(down.to_rgba8().r < 128, "{down:?}");
    }

    /// Brightness cannot push a channel past the ends of the range, however
    /// hard it is driven.
    #[test]
    fn brightness_is_bounded() {
        for value in [-1000.0, -100.0, 100.0, 1000.0] {
            let out = ColorAdjust {
                brightness: value,
                ..Default::default()
            }
            .apply(Color::from_rgb8(200, 10, 90));
            let _ = out.to_rgba8();
        }
    }

    #[test]
    fn saturation_drains_colour_to_grey_and_back() {
        let vivid = Color::from_rgb8(220, 40, 40);
        let grey = ColorAdjust {
            saturation: -100.0,
            ..Default::default()
        }
        .apply(vivid);
        let [r, g, b, _] = grey.to_rgba8().to_u8_array();
        assert!(
            (r as i32 - g as i32).abs() < 3 && (g as i32 - b as i32).abs() < 3,
            "fully desaturated should be grey: {grey:?}"
        );

        let more = ColorAdjust {
            saturation: 60.0,
            ..Default::default()
        }
        .apply(vivid);
        assert!(
            more.to_rgba8().r >= vivid.to_rgba8().r,
            "raising saturation should not dull it"
        );
    }

    #[test]
    fn contrast_pushes_away_from_mid_grey() {
        let dark = Color::from_rgb8(100, 100, 100);
        let harder = ColorAdjust {
            contrast: 60.0,
            ..Default::default()
        }
        .apply(dark);
        assert!(harder.to_rgba8().r < 100, "{harder:?}");

        let softer = ColorAdjust {
            contrast: -60.0,
            ..Default::default()
        }
        .apply(dark);
        assert!(softer.to_rgba8().r > 100, "{softer:?}");
    }

    /// A full turn of hue comes back to where it started, near enough.
    #[test]
    fn hue_rotation_is_a_rotation() {
        let start = Color::from_rgb8(200, 60, 30);
        let half = ColorAdjust {
            hue: 180.0,
            ..Default::default()
        }
        .apply(start);
        assert_ne!(half, start, "half a turn should change the colour");

        let back = ColorAdjust {
            hue: -180.0,
            ..Default::default()
        }
        .apply(half);
        let [r, g, b, _] = back.to_rgba8().to_u8_array();
        let [sr, sg, sb, _] = start.to_rgba8().to_u8_array();
        assert!(
            (r as i32 - sr as i32).abs() < 40
                && (g as i32 - sg as i32).abs() < 40
                && (b as i32 - sb as i32).abs() < 40,
            "a turn and back should nearly return: {back:?} vs {start:?}"
        );
    }

    /// Alpha is never touched: transparency belongs to the colour effect and
    /// to the instance, not to Adjust Color.
    #[test]
    fn adjusting_colour_leaves_alpha_alone() {
        let translucent = Color::from_rgba8(200, 100, 50, 128);
        let out = ColorAdjust {
            brightness: 40.0,
            saturation: -80.0,
            hue: 90.0,
            contrast: 20.0,
        }
        .apply(translucent);
        assert_eq!(out.to_rgba8().a, 128);
    }

    #[test]
    fn adjustments_interpolate_for_a_tween() {
        let from = ColorAdjust::default();
        let to = ColorAdjust {
            brightness: 100.0,
            contrast: -40.0,
            saturation: 20.0,
            hue: 180.0,
        };
        let half = from.lerp(&to, 0.5);
        assert_eq!(half.brightness, 50.0);
        assert_eq!(half.contrast, -20.0);
        assert_eq!(half.hue, 90.0);

        assert_eq!(from.lerp(&to, 0.0), from);
        assert_eq!(from.lerp(&to, 1.0), to);
    }

    #[test]
    fn quality_buys_bands() {
        assert!(Quality::Low.bands() < Quality::Medium.bands());
        assert!(Quality::Medium.bands() < Quality::High.bands());
        assert!(Quality::Low.bands() >= 3, "too few to read as a ramp");
    }

    /// Reach is what the renderer isolates, so it must cover everything the
    /// filter paints — and be zero for the ones that paint nothing outside.
    #[test]
    fn reach_covers_what_a_filter_paints() {
        assert_eq!(FilterKind::adjust().reach(), 0.0);

        let FilterKind::DropShadow { .. } = FilterKind::drop_shadow() else {
            panic!()
        };
        assert!(FilterKind::drop_shadow().reach() >= 10.0);

        let inner = FilterKind::Glow {
            x: 20.0,
            y: 20.0,
            strength: 1.0,
            color: Color::WHITE,
            inner: true,
            knockout: false,
            quality: Quality::Low,
        };
        assert_eq!(inner.reach(), 0.0, "an inner glow stays inside");
    }

    /// A motion tween animates a filter — a glow that grows, a shadow that
    /// swings — which is most of what filters are used for in Animate.
    #[test]
    fn filters_of_the_same_kind_interpolate() {
        let from = Filter::new(FilterKind::Glow {
            x: 0.0,
            y: 0.0,
            strength: 0.0,
            color: Color::BLACK,
            inner: false,
            knockout: false,
            quality: Quality::Low,
        });
        let to = Filter::new(FilterKind::Glow {
            x: 20.0,
            y: 20.0,
            strength: 1.0,
            color: Color::WHITE,
            inner: false,
            knockout: false,
            quality: Quality::Low,
        });

        let half = from.lerp(&to, 0.5);
        let FilterKind::Glow {
            x, strength, color, ..
        } = half.kind
        else {
            panic!("kind changed")
        };
        assert_eq!(x, 10.0);
        assert_eq!(strength, 0.5);
        assert_eq!(color.to_rgba8().r, 128, "{color:?}");
    }

    /// Two different filters have no halfway point; holding the start is what
    /// Animate does, and inventing one would look like a bug.
    #[test]
    fn different_filters_do_not_interpolate() {
        let blur = Filter::new(FilterKind::blur());
        let bevel = Filter::new(FilterKind::bevel());
        assert_eq!(blur.lerp(&bevel, 0.5).kind, blur.kind);
    }

    /// A shadow swinging from just west of north to just east of north goes
    /// the short way, not three quarters of the way round the compass.
    #[test]
    fn a_shadow_swings_the_short_way() {
        let at = |angle: f64| {
            Filter::new(FilterKind::DropShadow {
                x: 4.0,
                y: 4.0,
                strength: 1.0,
                angle,
                distance: 5.0,
                color: Color::BLACK,
                inner: false,
                knockout: false,
                hide_object: false,
                quality: Quality::Low,
            })
        };
        let from = at(3.0);
        let to = at(-3.0);
        let FilterKind::DropShadow { angle, .. } = from.lerp(&to, 0.5).kind else {
            panic!()
        };
        // Halfway the short way is just past pi, not near zero.
        assert!(
            angle.abs() > 3.0,
            "the shadow went the long way round: {angle}"
        );
    }

    #[test]
    fn blend_modes_know_when_they_need_a_group() {
        assert!(!Blend::Normal.needs_group());
        for blend in Blend::ALL.iter().filter(|b| **b != Blend::Normal) {
            assert!(blend.needs_group(), "{blend:?}");
            assert!(!blend.label().is_empty());
        }
    }

    #[test]
    fn every_filter_has_a_name() {
        for kind in FilterKind::all() {
            assert!(!kind.label().is_empty());
        }
        assert_eq!(FilterKind::all().len(), 6);
    }
}
