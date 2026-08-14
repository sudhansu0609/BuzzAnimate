//! Lights: a sun, a sky and lamps, and what they do to flat artwork.
//!
//! # The problem, stated honestly
//!
//! Blender lights a scene made of surfaces that have normals. A drawing has no
//! normals: it is coloured regions on a plane. So "lighting" here cannot mean
//! evaluating a BRDF — it has to mean the three things an animator actually
//! draws when they light a shot, and it has to make those three follow the
//! light rather than be painted by hand:
//!
//! 1. **The colour of the light** on everything it reaches, and the colour of
//!    the fill where it does not.
//! 2. **A shading crescent** on the side away from the light, and a
//!    **highlight** on the side towards it. Their *shape* is the artwork's own
//!    outline, offset — which is what makes them read as form rather than as a
//!    filter.
//! 3. **A cast shadow** on the surface behind, in the direction the light
//!    points, lengthening as the light gets lower.
//!
//! Move the sun and all three swing round together. That is the behaviour
//! being reproduced, and it is reproduced with geometry the renderer already
//! knows how to draw — no per-pixel shading, no normal maps, nothing that
//! stops being editable vector artwork.
//!
//! # Where "height" comes from
//!
//! A shadow's length is set by how far the caster stands above the surface
//! receiving it. Flat artwork has no thickness, so each light carries a
//! **standing height**: how far off the background the drawing is assumed to
//! be. **Layer depth adds to it** — a layer pushed towards the camera really
//! is in front of the background, and its shadow lengthens accordingly, using
//! the depth model Phase 7 already established.
//!
//! # Nothing happens without a light
//!
//! An empty rig changes no colour and generates no geometry, so a document
//! that never touches lighting renders exactly as it did before this existed.

pub mod geometry;

use buzz_geom::{Point, Vec2};
use peniko::Color;
use serde::{Deserialize, Serialize};

pub use geometry::{ShadeGeometry, cast_shadow, highlight_crescent, shade_crescent};

/// Stable identity for a light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LightId(pub u64);

/// The three kinds Blender's lamp menu offers that mean anything in two
/// dimensions.
///
/// Spot and area lights are deliberately absent: both are defined by a cone or
/// a rectangle *in three dimensions*, and their whole character — the falloff
/// across the cone, the softness from the area — is invisible on flat artwork.
/// A spot would be a lamp with extra numbers that changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LightKind {
    /// **Sun.** Parallel rays: the same direction everywhere on the stage, so
    /// every shadow runs the same way and stays the same length. `azimuth` is
    /// the compass direction in the plane of the stage, measured clockwise
    /// from the right; `elevation` is how high the sun is above that plane.
    Sun { azimuth: f64, elevation: f64 },
    /// **Sky.** The ambient fill — what lights the side facing away from
    /// everything else. Two colours, because a real sky is not one: `color` is
    /// overhead and `horizon` is at the bottom, and a shape is lit by the mix
    /// its own height on the stage implies.
    Sky { horizon: Color },
    /// **Lamp.** A point at `position` on the stage, `height` above it.
    /// Direction varies across the stage, so shadows radiate outwards and
    /// lengthen with distance — which is exactly how you tell a lamp from a
    /// sun in a finished shot.
    Lamp {
        position: Point,
        height: f64,
        radius: f64,
    },
}

impl LightKind {
    pub fn label(&self) -> &'static str {
        match self {
            LightKind::Sun { .. } => "Sun",
            LightKind::Sky { .. } => "Sky",
            LightKind::Lamp { .. } => "Lamp",
        }
    }

    /// A sun pointing down and to the right, as a default that reads well.
    pub fn sun() -> Self {
        LightKind::Sun {
            azimuth: -0.6,
            elevation: 0.9,
        }
    }

    pub fn sky() -> Self {
        LightKind::Sky {
            horizon: Color::from_rgb8(0x9A, 0x8C, 0x78),
        }
    }

    pub fn lamp(position: Point) -> Self {
        LightKind::Lamp {
            position,
            height: 160.0,
            radius: 320.0,
        }
    }
}

/// One light.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Light {
    pub id: LightId,
    pub name: String,
    pub kind: LightKind,
    pub color: Color,
    /// Multiplies the colour. One is "full".
    pub intensity: f32,
    pub enabled: bool,
    /// Does this light cast shadows, and how dark are they?
    pub shadows: bool,
    /// `0.0..=1.0`.
    pub shadow_strength: f32,
    /// How far off the background flat artwork is assumed to stand, in
    /// document units. Layer depth is added to it.
    pub standing_height: f64,
    /// How wide the shading crescent is, as a fraction of the shape.
    ///
    /// This is the closest thing to a light's *size*: a small, hard light
    /// gives a narrow terminator, a broad soft one gives a wide gradient.
    pub softness: f64,
}

impl Light {
    pub fn new(id: LightId, name: impl Into<String>, kind: LightKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            color: Color::from_rgb8(0xFF, 0xF2, 0xD8),
            intensity: 1.0,
            enabled: true,
            shadows: true,
            shadow_strength: 0.45,
            standing_height: 40.0,
            softness: 0.35,
        }
    }

    /// Is this the ambient fill rather than a directional source?
    pub fn is_ambient(&self) -> bool {
        matches!(self.kind, LightKind::Sky { .. })
    }

    /// Where the light is, as seen from `point` on a surface at `depth`.
    ///
    /// Returns the unit vector **towards the light** and how strongly it
    /// arrives. `None` for a sky, which has no direction.
    pub fn towards(&self, point: Point, depth: f64) -> Option<(Vec3, f32)> {
        match self.kind {
            LightKind::Sky { .. } => None,
            LightKind::Sun { azimuth, elevation } => {
                let (sin_e, cos_e) = elevation.sin_cos();
                let (sin_a, cos_a) = azimuth.sin_cos();
                // Towards the sun: along the compass bearing, tilted up out of
                // the stage by the elevation.
                Some((
                    Vec3::new(cos_a * cos_e, sin_a * cos_e, sin_e).unit(),
                    self.intensity,
                ))
            }
            LightKind::Lamp {
                position,
                height,
                radius,
            } => {
                // The lamp sits `height` in front of the focal plane; the
                // surface sits at `depth` behind it.
                let to_light =
                    Vec3::new(position.x - point.x, position.y - point.y, height + depth);
                let distance = to_light.length();
                if distance <= f64::EPSILON {
                    return Some((Vec3::new(0.0, 0.0, 1.0), self.intensity));
                }
                // Inverse-square falloff, softened so a lamp does not become a
                // pinprick: `radius` is the distance at which it is half as
                // strong, which is a number an animator can reason about.
                let falloff = 1.0 / (1.0 + (distance / radius.max(1.0)).powi(2));
                Some((to_light.unit(), self.intensity * falloff as f32))
            }
        }
    }

    /// The ambient colour this light contributes at `point`.
    ///
    /// Only a sky contributes ambient, and it mixes its two colours by how
    /// high on the stage the point is — which is what makes a sky read as a
    /// sky rather than as a flat wash.
    pub fn ambient_at(&self, point: Point, stage_height: f64) -> Option<Color> {
        let LightKind::Sky { horizon } = self.kind else {
            return None;
        };
        let t = if stage_height > 0.0 {
            (point.y / stage_height).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some(mix(self.color, horizon, t as f32).multiply_alpha(self.intensity.clamp(0.0, 4.0)))
    }
}

/// A three-dimensional direction. Small enough to keep here rather than pull a
/// linear-algebra crate in for three fields and two methods.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn unit(self) -> Self {
        let length = self.length();
        if length <= f64::EPSILON {
            return Self::new(0.0, 0.0, 1.0);
        }
        Self::new(self.x / length, self.y / length, self.z / length)
    }

    /// The part of this direction lying in the plane of the stage.
    pub fn planar(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

/// Every light in a document, plus the settings they share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightRig {
    pub lights: Vec<Light>,
    /// Off by default. An empty or disabled rig changes nothing at all.
    pub enabled: bool,
    /// Light left over when nothing reaches a surface, so unlit artwork is
    /// dim rather than black. Blender's world colour, in effect.
    pub base: Color,
    /// How strongly the shading and highlight crescents are drawn, `0..=1`.
    ///
    /// Separate from the light's own intensity because it is a *style*
    /// decision: the same lighting can be rendered as a soft gradient or as
    /// hard cel shading, and animators disagree about which they want.
    pub modelling: f32,
}

impl LightRig {
    /// A number that changes when — and only when — the lighting changes.
    ///
    /// # Why this is not the document's revision
    ///
    /// Generated lighting geometry is cached, and the cache is thrown away
    /// whenever the lights move, because every crescent and every shadow is
    /// then wrong. It used to be thrown away whenever the **document**'s
    /// revision changed, which is a different thing entirely: a revision bumps
    /// on every edit, and a drag bumps it on every mouse move.
    ///
    /// So dragging one hand rebuilt the shading and the cast shadow of every
    /// shape in the film, once per frame, for as long as the drag lasted. On a
    /// few rectangles that is invisible; on a real character it is seconds per
    /// frame, which is indistinguishable from the application having hung —
    /// and it made lit artwork impossible to pose, which is the one thing
    /// lighting is for.
    ///
    /// Keyed on the lights themselves, an edit to *artwork* keeps the whole
    /// cache; the objects that actually moved miss it on their own account,
    /// because the entries are keyed by the object's shared pointer and a
    /// changed object has a new one.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        // Floats are hashed by their bits: two rigs that differ only in a
        // colour or an angle must not collide, and `f64` is not `Hash`.
        fn f(hasher: &mut impl Hasher, v: f64) {
            v.to_bits().hash(hasher);
        }
        fn colour(hasher: &mut impl Hasher, c: Color) {
            for channel in c.components {
                channel.to_bits().hash(hasher);
            }
        }

        self.enabled.hash(&mut hasher);
        colour(&mut hasher, self.base);
        self.modelling.to_bits().hash(&mut hasher);
        self.lights.len().hash(&mut hasher);

        for light in &self.lights {
            light.id.0.hash(&mut hasher);
            colour(&mut hasher, light.color);
            light.intensity.to_bits().hash(&mut hasher);
            light.enabled.hash(&mut hasher);
            light.shadows.hash(&mut hasher);
            light.shadow_strength.to_bits().hash(&mut hasher);
            f(&mut hasher, light.standing_height);
            f(&mut hasher, light.softness);
            match &light.kind {
                LightKind::Sun { azimuth, elevation } => {
                    0u8.hash(&mut hasher);
                    f(&mut hasher, *azimuth);
                    f(&mut hasher, *elevation);
                }
                LightKind::Sky { horizon } => {
                    1u8.hash(&mut hasher);
                    colour(&mut hasher, *horizon);
                }
                LightKind::Lamp {
                    position,
                    height,
                    radius,
                } => {
                    2u8.hash(&mut hasher);
                    f(&mut hasher, position.x);
                    f(&mut hasher, position.y);
                    f(&mut hasher, *height);
                    f(&mut hasher, *radius);
                }
            }
        }
        hasher.finish()
    }
}

impl Default for LightRig {
    fn default() -> Self {
        Self {
            lights: Vec::new(),
            enabled: false,
            // Not black: unlit artwork should stay recognisable, and a
            // document that switches lighting on should not go dark.
            base: Color::from_rgb8(0x6E, 0x74, 0x82),
            modelling: 0.8,
        }
    }
}

impl LightRig {
    /// Is there anything that would change how the document looks?
    pub fn is_active(&self) -> bool {
        self.enabled && self.lights.iter().any(|l| l.enabled)
    }

    pub fn get(&self, id: LightId) -> Option<&Light> {
        self.lights.iter().find(|l| l.id == id)
    }

    pub fn get_mut(&mut self, id: LightId) -> Option<&mut Light> {
        self.lights.iter_mut().find(|l| l.id == id)
    }

    pub fn remove(&mut self, id: LightId) -> Option<Light> {
        let index = self.lights.iter().position(|l| l.id == id)?;
        Some(self.lights.remove(index))
    }

    /// The lights that cast: everything directional and enabled.
    pub fn casters(&self) -> impl Iterator<Item = &Light> {
        self.lights.iter().filter(|l| l.enabled && !l.is_ambient())
    }

    /// The **key light**: the strongest directional one, which is the light
    /// whose shadow an animator actually looks at.
    ///
    /// Shading and cast shadows follow this one. Summing crescents from every
    /// light would be physically nicer and visually a mess — two overlapping
    /// terminators on flat artwork read as dirt, which is why hand-drawn
    /// animation lights from one key and fills with the rest.
    pub fn key(&self) -> Option<&Light> {
        self.casters().max_by(|a, b| {
            let strength = |l: &Light| {
                l.intensity
                    * match l.kind {
                        LightKind::Sun { elevation, .. } => elevation.sin().max(0.1) as f32,
                        _ => 1.0,
                    }
            };
            strength(a).total_cmp(&strength(b))
        })
    }

    /// How a surface at `point`, on a layer at `depth`, is lit.
    ///
    /// `stage_height` is the stage's height in document units, used by the sky
    /// to mix its two colours.
    pub fn illuminate(&self, point: Point, depth: f64, stage_height: f64) -> Illumination {
        if !self.is_active() {
            return Illumination::unlit();
        }

        let mut ambient = to_linear(self.base);
        for light in self.lights.iter().filter(|l| l.enabled) {
            if let Some(colour) = light.ambient_at(point, stage_height) {
                let c = to_linear(colour);
                ambient = [ambient[0] + c[0], ambient[1] + c[1], ambient[2] + c[2]];
            }
        }

        let mut direct = [0.0f32; 3];
        for light in self.lights.iter().filter(|l| l.enabled && !l.is_ambient()) {
            let Some((towards, strength)) = light.towards(point, depth) else {
                continue;
            };
            // Flat artwork faces the camera, so the surface normal is `+z` and
            // `N·L` is simply how far the light is *in front of* the stage. A
            // light at the horizon therefore adds almost no fill and casts a
            // very long shadow, which is exactly right.
            let facing = towards.z.max(0.0) as f32;
            let c = to_linear(light.color);
            direct[0] += c[0] * facing * strength;
            direct[1] += c[1] * facing * strength;
            direct[2] += c[2] * facing * strength;
        }

        Illumination {
            ambient,
            direct,
            key: self.key().map(|l| l.id),
        }
    }
}

/// What reaches one point: the fill light and the direct light, kept apart so
/// the shaded side can be drawn with the ambient alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Illumination {
    ambient: [f32; 3],
    direct: [f32; 3],
    pub key: Option<LightId>,
}

impl Illumination {
    /// Full daylight: what an unlit document uses, so nothing changes colour.
    pub fn unlit() -> Self {
        Self {
            ambient: [1.0, 1.0, 1.0],
            direct: [0.0, 0.0, 0.0],
            key: None,
        }
    }

    /// Is this doing anything at all?
    pub fn is_neutral(&self) -> bool {
        self.ambient == [1.0, 1.0, 1.0] && self.direct == [0.0, 0.0, 0.0]
    }

    /// A colour as it appears fully lit.
    pub fn apply(&self, base: Color) -> Color {
        self.tint(base, true)
    }

    /// A colour as it appears in shade — ambient only, which is what makes the
    /// shaded side take the *sky's* colour rather than a grey.
    pub fn apply_shaded(&self, base: Color) -> Color {
        self.tint(base, false)
    }

    fn tint(&self, base: Color, direct: bool) -> Color {
        let source = to_linear(base);
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            let light = self.ambient[i] + if direct { self.direct[i] } else { 0.0 };
            // Soft shoulder rather than a hard clamp: a bright light should
            // bleach towards white smoothly instead of flattening into a
            // posterised block the moment it exceeds one.
            let lit = source[i] * light;
            out[i] = lit / (1.0 + (lit - 1.0).max(0.0));
        }
        from_linear(out, base.to_rgba8().to_u8_array()[3])
    }

    /// The colour a highlight is drawn in: the artwork, pushed towards the
    /// light's own colour.
    pub fn highlight(&self, base: Color, light: Color, strength: f32) -> Color {
        mix(self.apply(base), light, strength.clamp(0.0, 1.0) * 0.55)
    }
}

/// Blend two colours in linear light.
///
/// In sRGB, mixing halfway between black and white gives 0.5, which *looks*
/// far too dark; the same mix in linear light lands where the eye expects.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (x, y) = (to_linear(a), to_linear(b));
    let alpha_a = a.to_rgba8().to_u8_array()[3] as f32;
    let alpha_b = b.to_rgba8().to_u8_array()[3] as f32;
    from_linear(
        [
            x[0] + (y[0] - x[0]) * t,
            x[1] + (y[1] - x[1]) * t,
            x[2] + (y[2] - x[2]) * t,
        ],
        (alpha_a + (alpha_b - alpha_a) * t) as u8,
    )
}

fn to_linear(c: Color) -> [f32; 3] {
    let [r, g, b, _] = c.to_rgba8().to_u8_array();
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]
}

fn from_linear(c: [f32; 3], alpha: u8) -> Color {
    Color::from_rgba8(
        linear_to_srgb(c[0]),
        linear_to_srgb(c[1]),
        linear_to_srgb(c[2]),
        alpha,
    )
}

fn srgb_to_linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sun(azimuth: f64, elevation: f64) -> Light {
        Light::new(LightId(1), "Sun", LightKind::Sun { azimuth, elevation })
    }

    fn rig_with(light: Light) -> LightRig {
        LightRig {
            lights: vec![light],
            enabled: true,
            ..LightRig::default()
        }
    }

    #[test]
    fn an_empty_rig_changes_nothing() {
        let rig = LightRig::default();
        assert!(!rig.is_active());

        let illumination = rig.illuminate(Point::new(10.0, 10.0), 0.0, 400.0);
        assert!(illumination.is_neutral());

        for colour in [
            Color::WHITE,
            Color::BLACK,
            Color::from_rgb8(0x33, 0x66, 0x99),
        ] {
            assert_eq!(
                illumination.apply(colour),
                colour,
                "an unlit document must render exactly as it did before"
            );
        }
    }

    /// A rig with lights but switched off is still nothing: the switch is what
    /// the user reaches for, and it has to be complete.
    #[test]
    fn a_disabled_rig_changes_nothing() {
        let mut rig = rig_with(sun(0.0, 1.0));
        rig.enabled = false;
        assert!(!rig.is_active());
        assert!(rig.illuminate(Point::ZERO, 0.0, 400.0).is_neutral());

        rig.enabled = true;
        rig.lights[0].enabled = false;
        assert!(!rig.is_active(), "every light off is the same as no lights");
    }

    /// The sun's direction: azimuth is the compass bearing in the stage plane,
    /// elevation lifts it out towards the viewer.
    #[test]
    fn the_sun_points_where_it_is_aimed() {
        // Straight along +x, level with the stage.
        let (towards, _) = sun(0.0, 0.0)
            .towards(Point::ZERO, 0.0)
            .expect("a sun has a direction");
        assert!((towards.x - 1.0).abs() < 1e-9, "{towards:?}");
        assert!(towards.y.abs() < 1e-9);
        assert!(towards.z.abs() < 1e-9);

        // Overhead: straight out of the stage.
        let (overhead, _) = sun(0.0, std::f64::consts::FRAC_PI_2)
            .towards(Point::ZERO, 0.0)
            .expect("a direction");
        assert!((overhead.z - 1.0).abs() < 1e-9, "{overhead:?}");

        // A quarter turn: along +y.
        let (side, _) = sun(std::f64::consts::FRAC_PI_2, 0.0)
            .towards(Point::ZERO, 0.0)
            .expect("a direction");
        assert!((side.y - 1.0).abs() < 1e-9, "{side:?}");
    }

    /// Parallel rays: the same direction wherever you stand. This is the whole
    /// difference between a sun and a lamp.
    #[test]
    fn a_sun_points_the_same_way_everywhere() {
        let light = sun(0.7, 0.5);
        let a = light.towards(Point::new(-500.0, -500.0), 0.0).expect("a");
        let b = light.towards(Point::new(900.0, 700.0), 300.0).expect("b");
        assert!((a.0.x - b.0.x).abs() < 1e-12);
        assert!((a.0.y - b.0.y).abs() < 1e-12);
        assert!((a.0.z - b.0.z).abs() < 1e-12);
    }

    /// A lamp's direction turns as you move around it, which is what makes its
    /// shadows radiate.
    #[test]
    fn a_lamp_points_towards_itself_from_everywhere() {
        let lamp = Light::new(
            LightId(2),
            "Lamp",
            LightKind::lamp(Point::new(100.0, 100.0)),
        );

        let (left, _) = lamp.towards(Point::new(0.0, 100.0), 0.0).expect("left");
        assert!(
            left.x > 0.0,
            "from the left, the lamp is to the right: {left:?}"
        );

        let (right, _) = lamp.towards(Point::new(200.0, 100.0), 0.0).expect("right");
        assert!(
            right.x < 0.0,
            "from the right, it is to the left: {right:?}"
        );

        // And it is above both of them, which is what makes its shadows
        // radiate outwards rather than run parallel.
        assert!(left.z > 0.0 && right.z > 0.0);
        assert!(
            (left.x + right.x).abs() < 1e-9,
            "two points either side should mirror each other"
        );
    }

    #[test]
    fn a_lamp_dims_with_distance() {
        let lamp = Light::new(LightId(2), "Lamp", LightKind::lamp(Point::ZERO));
        let (_, near) = lamp.towards(Point::new(10.0, 0.0), 0.0).expect("near");
        let (_, far) = lamp.towards(Point::new(2_000.0, 0.0), 0.0).expect("far");

        assert!(near > far * 4.0, "near {near}, far {far}");
        assert!(far > 0.0, "falloff should approach zero, not reach it");
    }

    #[test]
    fn a_sky_has_no_direction_but_does_fill() {
        let sky = Light::new(LightId(3), "Sky", LightKind::sky());
        assert!(sky.is_ambient());
        assert!(sky.towards(Point::ZERO, 0.0).is_none());
        assert!(sky.ambient_at(Point::ZERO, 400.0).is_some());
    }

    /// The sky is two colours, mixed by height — a wash of one colour would
    /// not read as a sky.
    #[test]
    fn the_sky_mixes_its_two_colours_by_height() {
        let mut sky = Light::new(LightId(3), "Sky", LightKind::sky());
        sky.color = Color::from_rgb8(0x00, 0x00, 0xFF);
        sky.kind = LightKind::Sky {
            horizon: Color::from_rgb8(0xFF, 0x00, 0x00),
        };

        let top = sky.ambient_at(Point::new(0.0, 0.0), 400.0).expect("top");
        let bottom = sky
            .ambient_at(Point::new(0.0, 400.0), 400.0)
            .expect("bottom");

        assert!(
            top.to_rgba8().to_u8_array()[2] > 200,
            "the top is the zenith"
        );
        assert!(
            bottom.to_rgba8().to_u8_array()[0] > 200,
            "the bottom is the horizon"
        );
    }

    /// A light low on the horizon fills very little — its energy goes sideways,
    /// past the artwork. That is what makes a low sun dramatic.
    #[test]
    fn a_low_sun_fills_less_than_a_high_one() {
        let point = Point::new(100.0, 100.0);
        let high = rig_with(sun(0.0, 1.4)).illuminate(point, 0.0, 400.0);
        let low = rig_with(sun(0.0, 0.1)).illuminate(point, 0.0, 400.0);

        let grey = Color::from_rgb8(0x80, 0x80, 0x80);
        let brightness = |c: Color| {
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            r as u32 + g as u32 + b as u32
        };
        assert!(
            brightness(high.apply(grey)) > brightness(low.apply(grey)),
            "a high sun should light the artwork more"
        );
    }

    /// The light's colour reaches the artwork: a warm light on white artwork
    /// makes it warm. This is the first thing anybody checks.
    #[test]
    fn a_coloured_light_tints_what_it_lights() {
        let mut light = sun(0.0, 1.4);
        light.color = Color::from_rgb8(0xFF, 0x40, 0x00);
        light.intensity = 1.6;

        let mut rig = rig_with(light);
        rig.base = Color::from_rgb8(0x10, 0x10, 0x10);

        let lit = rig
            .illuminate(Point::ZERO, 0.0, 400.0)
            .apply(Color::from_rgb8(0x80, 0x80, 0x80));
        let [r, g, b, _] = lit.to_rgba8().to_u8_array();
        assert!(
            r > g && g > b,
            "an orange light should read orange: {lit:?}"
        );
    }

    /// The shaded side takes the *sky's* colour rather than a grey, which is
    /// what makes blue-shadowed sunlight look like sunlight.
    #[test]
    fn the_shaded_side_is_lit_by_the_sky_alone() {
        let mut sky = Light::new(LightId(3), "Sky", LightKind::sky());
        sky.color = Color::from_rgb8(0x40, 0x60, 0xFF);
        sky.kind = LightKind::Sky {
            horizon: Color::from_rgb8(0x40, 0x60, 0xFF),
        };

        let mut warm = sun(0.0, 1.2);
        warm.color = Color::from_rgb8(0xFF, 0xC0, 0x60);

        let rig = LightRig {
            lights: vec![sky, warm],
            enabled: true,
            base: Color::BLACK,
            ..LightRig::default()
        };

        let grey = Color::from_rgb8(0x9A, 0x9A, 0x9A);
        let illumination = rig.illuminate(Point::new(0.0, 200.0), 0.0, 400.0);

        let lit = illumination.apply(grey).to_rgba8().to_u8_array();
        let shaded = illumination.apply_shaded(grey).to_rgba8().to_u8_array();

        assert!(lit[0] > shaded[0], "the lit side should be brighter");
        assert!(
            shaded[2] > shaded[0],
            "the shaded side should take the sky's blue: {shaded:?}"
        );

        // The claim is *relative*: adding a warm key makes the lit side warmer
        // than the shade, whatever the sky is doing to both. Asserting that
        // red beats blue outright would only be testing which of the two
        // colours was written into the test.
        let warmth = |c: [u8; 4]| c[0] as f32 / c[2].max(1) as f32;
        assert!(
            warmth(lit) > warmth(shaded) * 1.2,
            "the lit side should be warmer than the shade: lit {lit:?}, shaded {shaded:?}"
        );
    }

    /// The key light is the one shadows follow, and it should be the one an
    /// animator would call the key: the strongest.
    #[test]
    fn the_key_is_the_strongest_directional_light() {
        let mut weak = sun(0.0, 1.0);
        weak.intensity = 0.2;
        weak.id = LightId(10);

        let mut strong = sun(2.0, 1.0);
        strong.intensity = 2.0;
        strong.id = LightId(11);

        let sky = Light::new(LightId(12), "Sky", LightKind::sky());

        let rig = LightRig {
            lights: vec![weak, sky, strong],
            enabled: true,
            ..LightRig::default()
        };
        assert_eq!(rig.key().map(|l| l.id), Some(LightId(11)));
    }

    #[test]
    fn a_rig_of_only_sky_has_no_key_and_casts_nothing() {
        let rig = LightRig {
            lights: vec![Light::new(LightId(3), "Sky", LightKind::sky())],
            enabled: true,
            ..LightRig::default()
        };
        assert!(rig.is_active(), "a sky still lights");
        assert!(rig.key().is_none(), "but nothing casts a shadow");
        assert_eq!(rig.casters().count(), 0);
    }

    #[test]
    fn colours_mix_in_linear_light() {
        // Halfway between black and white looks like mid grey, not 0x80.
        let middle = mix(Color::BLACK, Color::WHITE, 0.5)
            .to_rgba8()
            .to_u8_array();
        assert!(
            (185..=195).contains(&middle[0]),
            "linear-light midpoint should be about 0xBC, got {middle:?}"
        );
    }

    #[test]
    fn bright_light_bleaches_towards_white_rather_than_posterising() {
        let mut light = sun(0.0, 1.5);
        light.intensity = 8.0;
        light.color = Color::WHITE;
        let rig = rig_with(light);
        let illumination = rig.illuminate(Point::ZERO, 0.0, 400.0);

        let dark = illumination.apply(Color::from_rgb8(0x20, 0x20, 0x20));
        let mid = illumination.apply(Color::from_rgb8(0x60, 0x60, 0x60));
        assert!(
            mid.to_rgba8().to_u8_array()[0] >= dark.to_rgba8().to_u8_array()[0],
            "an overexposed image must still order its tones"
        );
    }

    #[test]
    fn removing_a_light_takes_it_out_of_the_rig() {
        let mut rig = rig_with(sun(0.0, 1.0));
        assert!(rig.get(LightId(1)).is_some());
        assert!(rig.remove(LightId(1)).is_some());
        assert!(rig.get(LightId(1)).is_none());
        assert!(!rig.is_active());
    }
}
