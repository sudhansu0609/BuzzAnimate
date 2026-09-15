//! Brush settings, and the pattern shapes a pattern brush stamps.
//!
//! The geometry lives in [`buzz_geom::brush`]; what lives here is the part the
//! user chooses — which brush, how big, how smooth, and what shape a pattern
//! brush repeats.
//!
//! # The brushes, in Animate's terms
//!
//! * **Fluid** is Animate's Brush: a filled stroke whose width answers to how
//!   the stroke was made. It is the default because it is what a brush is.
//! * **Pattern** and **Art** are Animate's Paint Brush, which repeats a source
//!   shape along the stroke or stretches one copy over it.
//!
//! # Why the pattern shapes are built in code
//!
//! A pattern brush needs a source shape. Animate ships a library of them and
//! lets you make your own from a selection; both are offered here, but the
//! built-in set is defined as geometry in this file rather than loaded from an
//! asset. That keeps the application self-contained, keeps the shapes exact at
//! any zoom, and avoids shipping artwork whose provenance would need checking.

use buzz_geom::{BezPath, BrushProfile, PatternFit, Point, Shape as _, WidthResponse};
use serde::{Deserialize, Serialize};

/// Which brush the Brush tool is currently being.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BrushKind {
    /// A filled stroke whose width follows pressure or speed.
    #[default]
    Fluid,
    /// **A plain brush: one width, all the way along.**
    ///
    /// The same filled outline as [`Self::Fluid`], with the response taken
    /// out — what a marker or a technical pen does, and what you want when a
    /// line has to read as one weight rather than as a gesture. It still
    /// tapers if asked, because a taper is a shape you choose rather than
    /// something the stroke's speed decided for you.
    Normal,
    /// A source shape repeated along the stroke.
    Pattern,
    /// A single source shape stretched over the whole stroke.
    Art,
    /// Painted pixels with a soft edge — Photoshop's round brush.
    ///
    /// The one brush here that is not geometry, and the only one that can fade
    /// at its edge: a soft edge is a different opacity at every point of a
    /// region, which no outline describes. See [`buzz_scene::raster`].
    Raster,
    /// A stroke that paints scenery: snow, clouds, skylines, strings of
    /// lights. See [`buzz_scene::effect_brush`] — one drag lays down vector
    /// silhouettes, gradient glows and painted pixels together, which is what
    /// mixed raster-and-vector drawing on one layer is for.
    Effect,
    /// A stroke that **flows**: smoke, water, wavy hair, ribbons.
    ///
    /// See [`buzz_scene::wave`]. The one brush here whose stroke can commit an
    /// *animation* rather than a drawing — a wave has a phase, and advancing it
    /// across frames is what makes smoke rise rather than merely sit there.
    Wave,
}

impl BrushKind {
    /// Every kind, in the order the Type picker offers them.
    pub const ALL: [BrushKind; 7] = [
        Self::Fluid,
        Self::Normal,
        Self::Raster,
        Self::Pattern,
        Self::Art,
        Self::Effect,
        Self::Wave,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Fluid => "Fluid",
            Self::Normal => "Normal",
            Self::Pattern => "Pattern",
            Self::Art => "Art",
            Self::Raster => "Soft",
            Self::Effect => "Effect",
            Self::Wave => "Wave",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Fluid => "A filled stroke that thins as you draw faster",
            Self::Normal => "A filled stroke of one steady width, like a marker",
            Self::Pattern => "Repeats a shape along the stroke",
            Self::Art => "Stretches one shape over the whole stroke",
            Self::Raster => "Paints pixels with a soft edge, as an airbrush does",
            Self::Effect => "Paints scenery: snow, clouds, skylines, lights",
            Self::Wave => "Draws what flows: smoke, water, wavy hair — and animates it",
        }
    }

    /// Does this brush choose its own compositing, piece by piece, rather than
    /// answering to the Build up setting?
    pub fn composites_itself(self) -> bool {
        matches!(self, Self::Effect | Self::Wave)
    }

    /// Does this brush stamp a source shape?
    pub fn uses_pattern(self) -> bool {
        matches!(self, Self::Pattern | Self::Art)
    }

    /// Does this brush draw a filled outline along the stroke — the two that
    /// share the width, taper and viscosity settings?
    pub fn is_outlined(self) -> bool {
        matches!(self, Self::Fluid | Self::Normal)
    }
}

/// The built-in shapes a pattern brush can repeat.
///
/// Each is defined in a box roughly 10 units wide and centred on the origin,
/// so that a stamp sits on the stroke rather than beside it, and so the
/// spacing default reads sensibly against the shape's own size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PatternShape {
    /// A round dot: the plainest pattern, and a dotted line at wide spacing.
    #[default]
    Dot,
    /// A short bar across the stroke, which reads as a railway line.
    Dash,
    /// A tapered leaf, the shape most obviously improved by following the
    /// tangent.
    Leaf,
    /// A five-pointed star.
    Star,
    /// A triangle pointing along the stroke, which reads as an arrow chain.
    Arrow,
    /// A square rotated onto its corner.
    Diamond,
    /// Whatever the user made from a selection.
    Custom,
}

impl PatternShape {
    pub const BUILT_IN: [PatternShape; 6] = [
        Self::Dot,
        Self::Dash,
        Self::Leaf,
        Self::Star,
        Self::Arrow,
        Self::Diamond,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dot => "Dot",
            Self::Dash => "Dash",
            Self::Leaf => "Leaf",
            Self::Star => "Star",
            Self::Arrow => "Arrow",
            Self::Diamond => "Diamond",
            Self::Custom => "From Selection",
        }
    }

    /// The shape's geometry, in its own coordinates.
    ///
    /// Returns `None` for [`PatternShape::Custom`], whose geometry is held on
    /// [`BrushSettings::custom_pattern`] because it comes from the document.
    pub fn path(self) -> Option<BezPath> {
        let path = match self {
            Self::Dot => kurbo::Circle::new(Point::ZERO, 5.0).to_path(1e-4),
            Self::Dash => kurbo::Rect::new(-1.5, -5.0, 1.5, 5.0).to_path(1e-9),
            Self::Leaf => leaf(),
            Self::Star => star(5, 5.0, 2.1),
            Self::Arrow => polygon(&[(-5.0, -4.0), (5.0, 0.0), (-5.0, 4.0), (-2.5, 0.0)]),
            Self::Diamond => polygon(&[(0.0, -5.0), (5.0, 0.0), (0.0, 5.0), (-5.0, 0.0)]),
            Self::Custom => return None,
        };
        Some(path)
    }
}

/// A leaf, as two mirrored quadratics meeting at points fore and aft.
fn leaf() -> BezPath {
    let mut path = BezPath::new();
    path.move_to(Point::new(-5.0, 0.0));
    path.quad_to(Point::new(0.0, -4.5), Point::new(5.0, 0.0));
    path.quad_to(Point::new(0.0, 4.5), Point::new(-5.0, 0.0));
    path.close_path();
    path
}

fn polygon(points: &[(f64, f64)]) -> BezPath {
    let mut path = BezPath::new();
    for (i, (x, y)) in points.iter().enumerate() {
        let point = Point::new(*x, *y);
        if i == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    path.close_path();
    path
}

fn star(points: usize, outer: f64, inner: f64) -> BezPath {
    let mut path = BezPath::new();
    let step = std::f64::consts::PI / points as f64;
    for i in 0..points * 2 {
        let radius = if i % 2 == 0 { outer } else { inner };
        let angle = i as f64 * step - std::f64::consts::FRAC_PI_2;
        let point = Point::new(angle.cos() * radius, angle.sin() * radius);
        if i == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    path.close_path();
    path
}

/// Everything the Brush tool needs, as the user set it.
#[derive(Debug, Clone, PartialEq)]
pub struct BrushSettings {
    pub kind: BrushKind,
    /// Width at full pressure, in document units.
    pub size: f64,
    /// `0.0`–`1.0`, matching Animate's Smoothing slider.
    pub smoothing: f64,
    /// **How far the ink is dragged behind the pointer**, `0.0`–`1.0`.
    ///
    /// A separate dial from [`Self::smoothing`] because it is a separate
    /// thing: smoothing evens out a line it can see all of, and a stabiliser
    /// keeps the shake from reaching the paper in the first place by lagging.
    /// See [`buzz_geom::Conditioning`].
    pub stabiliser: f64,
    /// How much the paint resists spreading. See
    /// [`buzz_geom::BrushProfile::viscosity`].
    pub viscosity: f64,
    /// Which ends [`Self::taper`] applies to.
    pub taper_ends: buzz_geom::TaperEnds,
    /// Narrowest width as a fraction of [`Self::size`].
    pub min_ratio: f64,
    /// How much of each end tapers to a point.
    pub taper: f64,
    /// Follow the device's pressure rather than the stroke's speed.
    ///
    /// Off by default, and the reason is worth stating: a mouse reports a
    /// constant pressure of 1.0, so a pressure-driven brush on a mouse paints
    /// a dead constant width. Speed is the response that makes a *mouse*
    /// stroke look drawn, so it is the default until a tablet says otherwise.
    pub use_pressure: bool,
    /// Speed, in document units per second, at which the stroke is narrowest.
    pub reference_speed: f64,
    pub pattern: PatternShape,
    /// Distance between stamps, in document units.
    pub spacing: f64,
    /// The artwork for [`PatternShape::Custom`] — **with its paint**.
    ///
    /// Behind an `Arc` because a brush is copied wherever the style is, and a
    /// brush made from a detailed drawing can carry a bitmap: sharing it makes
    /// a style clone a pointer rather than a photograph.
    pub custom_stamp: Option<std::sync::Arc<buzz_scene::BrushStamp>>,
    /// Stamp the captured artwork's **own colours and textures** rather than
    /// a silhouette in the fill swatch.
    ///
    /// On by default, because a brush made from a drawing that came out flat
    /// grey is not the brush anyone was pointing at. Turned off it is
    /// Animate's behaviour: the outline, painted by the current swatch — which
    /// is what you want when the artwork is a shape you drew to *be* a nib.
    pub keep_source_paint: bool,
    /// Paint that **builds up** where strokes overlap.
    ///
    /// With this on, a stroke at alpha 0.2 crossing one at 0.3 gives exactly
    /// 0.5 in the overlap rather than the 0.44 that ordinary compositing
    /// produces, so working over an area deepens it the way ink does.
    /// See [`buzz_scene::PaintBlend::Additive`].
    pub build_up: bool,
    /// Where a soft brush's fade begins, as a fraction of its radius.
    ///
    /// `1.0` is a hard edge, `0.0` fades from the very middle. Photoshop's
    /// Hardness, and the same numbers. Only [`BrushKind::Raster`] reads it.
    pub hardness: f64,
    /// How much paint a soft brush lays down, `0.0`–`1.0`.
    pub flow: f64,
    /// Which scenery the Effect brush paints. Only [`BrushKind::Effect`]
    /// reads it.
    pub effect: buzz_scene::EffectKind,
    /// Which flowing thing the Wave brush draws. Only [`BrushKind::Wave`]
    /// reads it, and it reads [`Self::wave_settings`] with it.
    pub wave: buzz_scene::WaveKind,
    /// How that wave is shaped — amplitude, wavelength, how many strands, and
    /// how many frames a commit bakes.
    ///
    /// Kept beside the kind rather than inside it because the kind is a
    /// *preset*: picking Smoke loads smoke-ish numbers here, and every one of
    /// them is then a slider. See [`buzz_scene::WaveKind::preset`].
    pub wave_settings: buzz_scene::WaveSettings,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            kind: BrushKind::default(),
            size: 12.0,
            smoothing: 0.5,
            // Off unless asked for: a brush that trailed behind the pointer
            // without being switched on reads as the application being slow.
            stabiliser: 0.0,
            viscosity: 0.65,
            taper_ends: buzz_geom::TaperEnds::default(),
            min_ratio: 0.35,
            taper: 0.12,
            use_pressure: false,
            reference_speed: 900.0,
            pattern: PatternShape::default(),
            spacing: 12.0,
            custom_stamp: None,
            keep_source_paint: true,
            // Half soft: plainly a soft brush at a glance, and still definite
            // enough to draw an edge with.
            hardness: 0.5,
            flow: 1.0,
            // Off by default: Animate composites normally, and a document
            // whose overlaps silently deepen would surprise anyone who did
            // not ask for it.
            build_up: false,
            effect: buzz_scene::EffectKind::default(),
            wave: buzz_scene::WaveKind::default(),
            wave_settings: buzz_scene::WaveSettings::default(),
        }
    }
}

impl BrushSettings {
    /// The geometry profile for the brushes that draw a filled outline.
    pub fn profile(&self) -> BrushProfile {
        BrushProfile {
            width: self.size.max(0.0),
            min_ratio: self.min_ratio.clamp(0.0, 1.0),
            // A plain brush is one width all the way along — the response is
            // exactly what it does not have.
            response: match (self.kind, self.use_pressure) {
                (BrushKind::Normal, _) => WidthResponse::Uniform,
                (_, true) => WidthResponse::Pressure,
                (_, false) => WidthResponse::Speed {
                    reference_speed: self.reference_speed.max(1.0),
                },
            },
            smoothing: self.smoothing.clamp(0.0, 1.0),
            stabiliser: self.stabiliser.clamp(0.0, 1.0),
            taper: self.taper.clamp(0.0, 0.5),
            taper_ends: self.taper_ends,
            viscosity: self.viscosity.clamp(0.0, 1.0),
        }
    }

    /// How this brush wants raw pointer samples cleaned up — for the brushes
    /// that stamp rather than outline, which have no profile to carry it.
    pub fn conditioning(&self) -> buzz_geom::Conditioning {
        buzz_geom::Conditioning {
            smoothing: self.smoothing.clamp(0.0, 1.0),
            stabiliser: self.stabiliser.clamp(0.0, 1.0),
        }
    }

    /// Choose which flowing thing the Wave brush draws, **and load its
    /// preset**.
    ///
    /// Picking "Smoke" has to give you smoke, not the last wave's numbers
    /// under a new name — a preset that does not load is a preset that is not
    /// there. Re-picking the kind already chosen leaves the sliders alone, so
    /// a tweaked wave is not thrown away by a stray click on its own name.
    pub fn set_wave(&mut self, kind: buzz_scene::WaveKind) {
        if self.wave != kind {
            self.wave = kind;
            self.wave_settings = kind.preset();
        }
    }

    /// How a pattern brush lays its source down.
    pub fn fit(&self) -> PatternFit {
        match self.kind {
            BrushKind::Art => PatternFit::Stretch,
            _ => PatternFit::Repeat {
                spacing: self.spacing.max(0.1),
            },
        }
    }

    /// The source shape, scaled to the brush size.
    ///
    /// Returns `None` when a custom pattern is selected but none has been
    /// made yet, so the caller can say so rather than painting nothing and
    /// leaving the user wondering.
    pub fn pattern_path(&self) -> Option<BezPath> {
        let base = match self.pattern {
            // The captured artwork's silhouette. Already centred and
            // normalised in stamp space, so the scaling below is the identity
            // apart from the size setting.
            PatternShape::Custom => self.custom_stamp.as_ref()?.outline(),
            other => other.path()?,
        };
        if base.elements().is_empty() {
            return None;
        }

        // The built-ins are drawn 10 units across, so the size setting scales
        // them; a custom shape is scaled to match, which keeps the size slider
        // meaning the same thing whatever is selected.
        let bounds = kurbo::Shape::bounding_box(&base);
        let extent = bounds.height().max(bounds.width());
        if extent <= 0.0 {
            return None;
        }
        let scale = self.size.max(0.01) / extent;
        Some(kurbo::Affine::scale(scale) * base)
    }

    /// How strokes from this brush combine with the paint under them.
    pub fn blend(&self) -> buzz_scene::PaintBlend {
        if self.build_up {
            buzz_scene::PaintBlend::Additive
        } else {
            buzz_scene::PaintBlend::Normal
        }
    }

    /// The captured artwork, when a custom brush is the one selected.
    pub fn pattern_stamp(&self) -> Option<&std::sync::Arc<buzz_scene::BrushStamp>> {
        (self.pattern == PatternShape::Custom)
            .then_some(self.custom_stamp.as_ref())
            .flatten()
    }

    /// **Will this brush stamp the artwork's own colours and textures?**
    ///
    /// Only a captured brush can: there is nothing else with paint of its own
    /// to keep. The three conditions are exactly the three things that have to
    /// be true, and the caller uses this to choose between stamping artwork
    /// and stamping an outline.
    pub fn stamps_its_own_paint(&self) -> bool {
        self.kind.uses_pattern()
            && self.keep_source_paint
            && self.pattern_stamp().is_some_and(|s| s.is_painted())
    }

    /// Adopt artwork from the document as the custom brush, paint and all.
    pub fn set_custom_stamp(&mut self, stamp: buzz_scene::BrushStamp) {
        self.custom_stamp = Some(std::sync::Arc::new(stamp));
        self.pattern = PatternShape::Custom;
    }

    /// Adopt a bare shape as the custom pattern, to be painted by the stroke's
    /// own swatch. For the scripting API and for geometry with no paint.
    pub fn set_custom_pattern(&mut self, path: BezPath) {
        if let Some(stamp) = buzz_scene::BrushStamp::from_path(path) {
            self.set_custom_stamp(stamp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_pattern_has_real_geometry() {
        for shape in PatternShape::BUILT_IN {
            let path = shape
                .path()
                .unwrap_or_else(|| panic!("{shape:?} has no path"));
            let bounds = path.bounding_box();
            assert!(
                bounds.width() > 0.0 && bounds.height() > 0.0,
                "{shape:?} is degenerate: {bounds:?}"
            );
            assert!(!shape.label().is_empty());
        }
    }

    /// Stamps are centred on the stroke, so a source that is not centred on
    /// its own origin would hang off to one side.
    #[test]
    fn built_in_patterns_are_centred_on_the_origin() {
        for shape in PatternShape::BUILT_IN {
            let bounds = shape.path().unwrap().bounding_box();
            let centre = bounds.center();
            assert!(
                centre.x.abs() < 1.5 && centre.y.abs() < 1.5,
                "{shape:?} is off-centre at {centre:?}"
            );
        }
    }

    /// The size slider must mean the same thing whichever pattern is chosen,
    /// or changing shape silently changes scale.
    #[test]
    fn the_size_setting_scales_every_pattern_the_same_way() {
        for shape in PatternShape::BUILT_IN {
            let settings = BrushSettings {
                kind: BrushKind::Pattern,
                pattern: shape,
                size: 40.0,
                ..Default::default()
            };
            let path = settings.pattern_path().expect("built-ins have paths");
            let bounds = path.bounding_box();
            let extent = bounds.width().max(bounds.height());
            assert!(
                (extent - 40.0).abs() < 1e-6,
                "{shape:?} scaled to {extent} rather than 40"
            );
        }
    }

    #[test]
    fn a_custom_pattern_is_none_until_one_is_made() {
        let mut settings = BrushSettings {
            pattern: PatternShape::Custom,
            ..Default::default()
        };
        assert!(
            settings.pattern_path().is_none(),
            "the caller has to be able to tell the user there is no shape yet"
        );

        settings.set_custom_pattern(kurbo::Rect::new(0.0, 0.0, 4.0, 2.0).to_path(1e-9));
        assert!(settings.pattern_path().is_some());
        assert_eq!(settings.pattern, PatternShape::Custom);
    }

    /// A mouse reports a constant pressure, so a pressure-driven brush on a
    /// mouse paints a dead constant width. Speed has to be the default.
    #[test]
    fn the_default_brush_responds_to_speed_not_pressure() {
        let settings = BrushSettings::default();
        assert!(!settings.use_pressure);
        assert!(matches!(
            settings.profile().response,
            WidthResponse::Speed { .. }
        ));

        let with_tablet = BrushSettings {
            use_pressure: true,
            ..Default::default()
        };
        assert_eq!(with_tablet.profile().response, WidthResponse::Pressure);
    }

    #[test]
    fn the_art_brush_stretches_and_the_pattern_brush_repeats() {
        let art = BrushSettings {
            kind: BrushKind::Art,
            ..Default::default()
        };
        assert_eq!(art.fit(), PatternFit::Stretch);

        let pattern = BrushSettings {
            kind: BrushKind::Pattern,
            spacing: 7.0,
            ..Default::default()
        };
        assert_eq!(pattern.fit(), PatternFit::Repeat { spacing: 7.0 });
    }

    /// **A normal brush is one steady width.** The whole difference between
    /// it and the fluid brush, and it must hold whatever the speed and
    /// pressure settings happen to say.
    #[test]
    fn a_normal_brush_ignores_speed_and_pressure() {
        for use_pressure in [false, true] {
            let settings = BrushSettings {
                kind: BrushKind::Normal,
                use_pressure,
                ..Default::default()
            };
            assert_eq!(
                settings.profile().response,
                WidthResponse::Uniform,
                "a normal brush must not answer to anything"
            );
        }

        // And the fluid brush still does.
        let fluid = BrushSettings {
            kind: BrushKind::Fluid,
            ..Default::default()
        };
        assert!(matches!(
            fluid.profile().response,
            WidthResponse::Speed { .. }
        ));
    }

    /// Both outlined brushes share the width, taper and viscosity settings —
    /// which is what the tool options key on to decide what to show.
    #[test]
    fn both_outlined_brushes_share_the_outline_settings() {
        assert!(BrushKind::Fluid.is_outlined());
        assert!(BrushKind::Normal.is_outlined());
        for other in [
            BrushKind::Pattern,
            BrushKind::Art,
            BrushKind::Raster,
            BrushKind::Effect,
            BrushKind::Wave,
        ] {
            assert!(!other.is_outlined(), "{other:?} does not draw an outline");
        }
    }

    /// The taper mode, the viscosity and the stabiliser all have to reach the
    /// geometry, or they are three sliders that do nothing.
    #[test]
    fn the_new_settings_reach_the_profile() {
        let settings = BrushSettings {
            kind: BrushKind::Normal,
            taper: 0.3,
            taper_ends: buzz_geom::TaperEnds::End,
            viscosity: 0.8,
            stabiliser: 0.45,
            ..Default::default()
        };
        let profile = settings.profile();
        assert_eq!(profile.taper_ends, buzz_geom::TaperEnds::End);
        assert!((profile.viscosity - 0.8).abs() < 1e-9);
        assert!((profile.stabiliser - 0.45).abs() < 1e-9);

        // And the stamping brushes, which have no profile, still carry the
        // same two conditioning dials.
        let conditioning = settings.conditioning();
        assert!((conditioning.stabiliser - 0.45).abs() < 1e-9);
        assert!((conditioning.smoothing - settings.smoothing).abs() < 1e-9);
    }

    /// The stabiliser is off until it is asked for: a brush that lagged
    /// without being switched on reads as the application being slow.
    #[test]
    fn the_stabiliser_is_off_by_default() {
        assert_eq!(BrushSettings::default().stabiliser, 0.0);
        assert_eq!(BrushSettings::default().profile().stabiliser, 0.0);
    }

    /// Settings a user can drag to zero must not produce a nonsensical
    /// profile.
    #[test]
    fn extreme_settings_are_clamped_into_something_drawable() {
        let settings = BrushSettings {
            size: -5.0,
            smoothing: 9.0,
            min_ratio: -1.0,
            taper: 5.0,
            spacing: 0.0,
            reference_speed: 0.0,
            ..Default::default()
        };
        let profile = settings.profile();

        assert!(profile.width >= 0.0);
        assert!((0.0..=1.0).contains(&profile.smoothing));
        assert!((0.0..=1.0).contains(&profile.min_ratio));
        assert!((0.0..=0.5).contains(&profile.taper));
        assert!(matches!(
            profile.response,
            WidthResponse::Speed { reference_speed } if reference_speed >= 1.0
        ));
        assert!(matches!(
            settings.fit(),
            PatternFit::Repeat { spacing } if spacing > 0.0
        ));
    }

    #[test]
    fn brush_kinds_describe_themselves_for_the_tool_options() {
        for kind in BrushKind::ALL {
            assert!(!kind.label().is_empty());
            assert!(!kind.description().is_empty());
        }
        assert!(!BrushKind::Fluid.uses_pattern());
        assert!(BrushKind::Pattern.uses_pattern());
        assert!(BrushKind::Art.uses_pattern());
    }

    /// Every brush the settings can describe must be offered by the picker —
    /// the soft brush existed for a whole phase without being reachable, and
    /// this is the test that would have said so.
    #[test]
    fn the_type_picker_offers_every_brush_kind() {
        for kind in [
            BrushKind::Fluid,
            BrushKind::Pattern,
            BrushKind::Art,
            BrushKind::Raster,
            BrushKind::Effect,
            BrushKind::Wave,
        ] {
            assert!(
                BrushKind::ALL.contains(&kind),
                "{kind:?} is not offered in the Type picker"
            );
        }
    }

    /// Picking a wave kind has to bring its preset with it, and re-picking the
    /// one already chosen has to leave a tweaked wave alone.
    #[test]
    fn choosing_a_wave_kind_loads_its_preset_without_discarding_tweaks() {
        let mut settings = BrushSettings::default();
        settings.set_wave(buzz_scene::WaveKind::River);
        assert_eq!(settings.wave, buzz_scene::WaveKind::River);
        assert_eq!(
            settings.wave_settings,
            buzz_scene::WaveKind::River.preset(),
            "picking a kind must load its preset"
        );

        settings.wave_settings.amplitude = 4.25;
        settings.set_wave(buzz_scene::WaveKind::River);
        assert_eq!(
            settings.wave_settings.amplitude, 4.25,
            "re-picking the same kind threw away a tweaked wave"
        );
    }

    /// The two brushes that composite themselves piece by piece are the two
    /// that must not be offered the Build up switch — it would be a lie there.
    #[test]
    fn the_generated_brushes_composite_themselves() {
        assert!(BrushKind::Effect.composites_itself());
        assert!(BrushKind::Wave.composites_itself());
        for other in [
            BrushKind::Fluid,
            BrushKind::Normal,
            BrushKind::Pattern,
            BrushKind::Art,
            BrushKind::Raster,
        ] {
            assert!(!other.composites_itself(), "{other:?}");
        }
    }
}
