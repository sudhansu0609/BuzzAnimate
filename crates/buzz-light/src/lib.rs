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
pub mod track;

use buzz_geom::{Point, Rect, Vec2};
use peniko::Color;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

pub use geometry::{
    HIGHLIGHT_SHARE, RIM_REACH, crescent_offset, highlight_reach, shade_reach,
    GloomBand, LightPool, RimGlow, ShadeGeometry, cast_shadow, crescent_direction, crescents,
    gloom_at, gloom_band, highlight_crescent, light_pool, rim_glow, shade_crescent,
    shadow_transform,
};
pub use track::{LightKey, LightTrack};

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
    /// **Gloom.** A source of *darkness*, which is not a thing Blender has and
    /// not a thing three-dimensional lighting needs: there, dark is what you
    /// get where light does not reach, and the way to make more of it is to
    /// put something in the way.
    ///
    /// Flat artwork has nothing to put in the way. A drawing is lit by the
    /// tint on its own colours, so the only dark a rig can produce is the fill
    /// light — one level, everywhere, however many lamps are switched on. That
    /// is why a lit drawing so often reads as *tinted* rather than as lit: the
    /// bright end moves and the dark end never does, and it is the distance
    /// between them that the eye reads as light.
    ///
    /// A gloom is the other end, made movable. It is deliberately **not** a
    /// point source — an inverse-square hole of dark centred on a spot looks
    /// like a smudge on the lens. It is a **wall**: wide across, thrown a long
    /// way forward, and fading as it goes. Stood off the far side of the stage
    /// from a lamp and thrown back across it, the two meet somewhere in the
    /// middle and the picture gains a range it cannot otherwise have.
    ///
    /// `edge` is where the near face of the wall stands and `facing` is the
    /// bearing it throws along — the same convention a sun's azimuth uses,
    /// clockwise from the right. The wall itself runs *across* that bearing,
    /// `width` wide, and the dark has faded to nothing `throw` along it.
    /// Outside that quad a gloom does nothing at all, which is what lets you
    /// aim one: stand it off-stage and only its long tail reaches the picture.
    Gloom {
        edge: Point,
        facing: f64,
        throw: f64,
        width: f64,
    },
}

impl LightKind {
    pub fn label(&self) -> &'static str {
        match self {
            LightKind::Sun { .. } => "Sun",
            LightKind::Sky { .. } => "Sky",
            LightKind::Lamp { .. } => "Lamp",
            LightKind::Gloom { .. } => "Gloom",
        }
    }

    /// A sun up and to the right, as a default that reads well.
    ///
    /// **Not overhead.** A shadow's length is the caster's standing height over
    /// the tangent of the elevation, so a sun at 52° threw one about eight tenths
    /// of the standing height — which on flat artwork lands almost entirely
    /// *underneath* the drawing that cast it and cannot be seen. The first thing
    /// an animator does after adding a sun is look for its shadow, and the honest
    /// report on finding none is that the sun does not work.
    ///
    /// 40° throws a shadow about a fifth longer than the caster stands tall, out
    /// from under it and onto the floor, and still delivers most of the light a
    /// higher sun would: `sin 40°` is 0.64 against 0.79.
    pub fn sun() -> Self {
        LightKind::Sun {
            azimuth: -0.6,
            elevation: 0.7,
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

    /// A wall of dark stood at `edge`, thrown to the right.
    ///
    /// **Long and wide by default**, because the failure a short narrow one
    /// produces is not "a subtle gloom" but "a grey rectangle on the picture":
    /// the moment either end of the quad is inside the frame it stops reading
    /// as darkness and starts reading as a shape. Nine hundred units of throw
    /// on a stage a few hundred across puts the far end well outside it, and
    /// the sides further out still.
    ///
    /// [`LightRig::opposing_gloom`] is the one an animator actually wants —
    /// this is what it starts from when there is no light to oppose.
    pub fn gloom(edge: Point) -> Self {
        LightKind::Gloom {
            edge,
            facing: 0.0,
            throw: 900.0,
            width: 2400.0,
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
    /// **How much of this lamp's light you can see.** `0.0` draws none of it;
    /// `1.0` is the full pool.
    ///
    /// A sun and a sky ignore it. They arrive the same way everywhere on the
    /// stage, so what they do *is* the flat tint on the artwork and there is
    /// nothing else to draw. A lamp is the opposite: its whole character is
    /// that it falls off, and a lamp that only tinted each shape by the light
    /// at that shape's middle showed no falloff at all — a wall under a lamp
    /// came out one flat colour, which reads as a filter rather than as a lamp.
    /// So a lamp lays a **pool** ([`crate::light_pool`]), and this is how
    /// strongly it is laid.
    ///
    /// Separate from `intensity` because they are different questions: how
    /// bright the lamp is, and how much of its light is in the air to be seen.
    /// A lamp turned down to nothing here still shades and still casts, which
    /// is how you use one purely to model form.
    #[serde(default = "full")]
    pub glow: f32,
    /// **How much this light gutters**, `0.0..=1.0`. Zero is a steady light,
    /// which is every light until this existed.
    ///
    /// A fire is not a lamp with an orange bulb. What makes a torch, a candle or
    /// a hearth read as fire is that it is *never still*: the brightness moves
    /// every frame, the colour goes redder as it drops, and the pool breathes
    /// with it. Keyframing that by hand is forty keys for two seconds of film
    /// and it still comes out looking mechanical, because a hand cannot help
    /// making a pattern.
    ///
    /// So it is a number rather than a track. The value is smoothed noise on the
    /// frame — two rates, a slow breath and a fast gutter, because a single rate
    /// reads as a pulse — seeded from the light's own id, so two torches in one
    /// shot flicker differently and neither ever repeats the other.
    ///
    /// **It moves the brightness and the colour, never the position.** A lamp
    /// that jittered across the stage would turn every crescent in the document
    /// on every frame, which is the one thing the shading cache cannot absorb;
    /// see [`LightRig::aim`]. Brightness and colour turn nothing.
    #[serde(default)]
    pub flicker: f32,
    /// **How brightly this light rims the artwork it reaches**, `0.0..=1.0`.
    /// Zero is a light that does not, which is every light until this existed.
    ///
    /// # What it is
    ///
    /// A glow around the *outside* edge of a drawing, in the light's own
    /// colour, spilling onto whatever is behind it. It is Animate's Glow filter
    /// \u2014 the same geometry, through the same code \u2014 with two differences that
    /// are the whole point: it is laid by the **light** rather than set on the
    /// artwork, so it appears when the light comes up and goes when it goes;
    /// and it takes the light's colour and the light's falloff, so a figure
    /// walking out of a lamp's reach loses its rim as it goes.
    ///
    /// # Why a light needs one at all
    ///
    /// Everything else lighting does here happens *inside* the silhouette: the
    /// tint, the terminator, the highlight. A cast shadow is the one thing that
    /// leaves it, and it is dark. So a lit drawing had no way to be brighter
    /// than the picture around it \u2014 which is exactly what a strong light on a
    /// figure looks like, and what an animator draws by hand as a rim.
    ///
    /// Its *width* comes from this number too, so one slider makes it appear
    /// rather than two making it appear and then be visible. See
    /// [`Light::rim_glow`].
    #[serde(default)]
    pub rim: f32,
    /// The light's animation, if it has one. `None` is a static light — every
    /// document until Wave 9a, and most since. See [`LightTrack`], and
    /// [`LightRig::resolved_at`] for how the renderer reads it.
    ///
    /// Deliberately left out of the fingerprints below: what is measured is the
    /// *resolved* light's values, and the track is how you get those values,
    /// not one of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<LightTrack>,
}

/// The serde default for [`Light::glow`]: a lamp in a file written before the
/// pool existed lit nothing visibly, and the honest reading of what it meant is
/// a lamp at full strength.
fn full() -> f32 {
    1.0
}

impl Light {
    pub fn new(id: LightId, name: impl Into<String>, kind: LightKind) -> Self {
        let mut light = Self {
            id,
            name: name.into(),
            kind,
            color: Color::from_rgb8(0xFF, 0xF2, 0xD8),
            // **A light at full strength must not merely dim the artwork.**
            //
            // At 1.0 it did. The lit side of a shape gets `ambient + direct`,
            // and for the default sun that summed to about 0.80 of the artwork's
            // own brightness — the fill light is a dim blue-grey and a sun at
            // 40° delivers `sin 40°`, which is 0.64. Nearly equal across the
            // three channels, so what reached the screen was the whole picture
            // multiplied by 0.8: a tenth darker, no hue shift, nothing that
            // reads as a light being on.
            //
            // Measured on a real film — a 28-layer Animate import — switching
            // the sun on moved 77% of the stage and shifted it by ten levels,
            // equally on red, green and blue. Every pixel changed and the
            // picture looked identical. The honest report on that is that
            // lighting does nothing, and that is the report it got.
            //
            // 1.3 puts the lit side back at the brightness the artwork was
            // drawn at, so the sun's whole effect goes into the *difference*
            // between the lit side and the shaded one rather than into a global
            // dim. The shaded side is unchanged — it is the ambient alone — so
            // the contrast that reads as form goes up rather than the picture
            // going down. See `LightRig::base` for the other half of the pair.
            intensity: 1.3,
            enabled: true,
            shadows: true,
            shadow_strength: 0.45,
            // **How a flat drawing gets a shadow at all**, so the default has to
            // be one that produces a visible one. Forty put the whole shadow
            // under the artwork at any reasonable sun; seventy puts it on the
            // floor beside it. It costs nothing in brightness — standing height
            // scales the shadow and touches nothing else.
            standing_height: 70.0,
            softness: 0.35,
            // **A pool you can see, not a wash over the stage.**
            //
            // At full the pool is the lamp's whole colour laid over the frame
            // out to three times its reach — which is most of a stage, and it
            // is screened, so the picture comes up evenly bright underneath it.
            // The report it produced was that a lamp "does not highlight edges,
            // it makes the whole area bright", and that is exactly what it was
            // doing: the pool was drowning the very thing that reads as light,
            // which is the difference between the near side of a figure and its
            // far side.
            //
            // Blender has no pool at all — light in the air needs a volume, and
            // there is no volume here. A third keeps what the pool is genuinely
            // for, a lamp that is visible in an empty shot, without it being the
            // loudest thing in a full one. The slider still goes to one.
            glow: 0.35,
            flicker: 0.0,
            // **Off by default.** A rim is a deliberate look, not a property of
            // light, and switching one on for every document that has ever
            // added a lamp would change finished films. It also costs a
            // silhouette per lit layer, which nothing should pay for unasked.
            rim: 0.0,
            track: None,
        };

        // **A gloom reads every one of these backwards**, so it cannot take the
        // defaults a light takes.
        //
        // `color` is not what it emits — it emits nothing — but what it leaves
        // behind: the colour the picture is multiplied towards where the dark
        // is deepest. Near-black with a blue bias, because that is what an
        // unlit surface under a sky actually is, and a neutral grey reads as a
        // dirty lens instead.
        //
        // `intensity` is how much light it stops, and stopping more than all of
        // it means nothing, so the range is `0..=1` and full is 1.0 — not the
        // 1.3 a light wants for the reasons above.
        //
        // `shadows` is off and stays off: a gloom has no direction to cast
        // along and nothing to cast, and leaving the flag set would put its ID
        // in front of a checkbox that could never do anything.
        if matches!(kind, LightKind::Gloom { .. }) {
            light.color = Color::from_rgb8(0x0B, 0x0E, 0x18);
            light.intensity = 1.0;
            light.shadows = false;
        }
        light
    }

    /// Is this the ambient fill rather than a directional source?
    pub fn is_ambient(&self) -> bool {
        matches!(self.kind, LightKind::Sky { .. })
    }

    /// **This lamp as a fire**: a hearth colour, a strong gutter, and a shorter
    /// reach than a bulb of the same brightness would have.
    ///
    /// A preset rather than a fourth kind of light. Everything a fire is, a lamp
    /// already has — a place on the stage, a falloff, a pool in the air — and
    /// the only things that make it fire are the colour and the fact that it
    /// will not hold still. A `LightKind::Fire` would have been a lamp with the
    /// same fields and a different name.
    pub fn make_fire(&mut self) {
        self.color = Color::from_rgb8(0xFF, 0x9A, 0x3C);
        self.flicker = 0.6;
        // **The brightness is left alone.** Turning it up as well was the
        // obvious move and it is wrong: a lamp's pool is laid at the strength
        // the lamp arrives with, and past full it clamps — so a fire that was
        // both brighter *and* guttering upwards put an opaque disc of its own
        // colour over the picture on its bright frames. The figure standing in
        // front of the fire disappeared for a frame and came back on the next,
        // which is the worst kind of flicker.
        //
        // A fire reads as fire from its colour, its tighter circle and the fact
        // that it will not hold still. How bright it is stays the animator's.
        // **A fire is one of the few lights that really does light the air.**
        // An ordinary lamp lays a third of its pool, because a full one washes
        // the stage; a fire is a visible thing in the shot rather than a bulb,
        // and the glow around it is half of what says so.
        self.glow = self.glow.max(0.6);
        // **A fire rims what stands in front of it.** It is the brightest thing
        // in its own shot and close to whatever it lights, which is the one
        // arrangement where an edge really does come up brighter than the
        // picture \u2014 and, because the rim follows the light's intensity, it
        // gutters with the flame rather than sitting there steadily.
        self.rim = self.rim.max(0.5);
        if let LightKind::Lamp { radius, .. } = &mut self.kind {
            *radius *= 0.8;
        }
    }

    /// This light as it stands at `frame`, once its gutter is applied.
    ///
    /// Borrowed-shaped rather than in place because the rig resolves a *copy*
    /// per frame; see [`LightRig::resolved_at`]. A light with no flicker is
    /// returned unchanged, so nothing pays for this that has not asked for it.
    pub fn flickered(&self, frame: u32) -> Light {
        let amount = self.flicker.clamp(0.0, 1.0);
        if amount <= 0.0 {
            return self.clone();
        }
        let n = flicker_noise(self.id.0, frame);
        // **Never to nothing.** A flame gutters; it does not switch off, and a
        // light that reached zero would take every crescent in the shot with it
        // for one frame and put them back on the next, which reads as a fault
        // rather than as fire.
        // **A gutter, not a strobe.** Half again and back is a fault light; a
        // real flame moves by something like a third and never stops moving.
        // The floor is well under anything the noise reaches, and is there so
        // no arithmetic can ever hand the renderer a light of zero.
        let factor = (1.0 + amount as f64 * n * 0.35).max(0.2);
        let mut out = self.clone();
        out.intensity = self.intensity * factor as f32;
        // The pool breathes with the light, but less: a flame's *reach* is
        // steadier than its brightness, and a pool that pumped in and out at
        // full depth reads as a lamp on a dimmer.
        out.glow = (self.glow * (0.75 + 0.25 * factor as f32)).clamp(0.0, 1.0);
        // Redder as it drops, which is what a flame actually does: the dim part
        // of a fire is the ember colour, not a dimmer version of the flame.
        let towards_ember = ((1.0 - factor).max(0.0) * amount as f64).min(1.0);
        out.color = mix(self.color, EMBER, towards_ember as f32);
        out
    }

    /// Is this a wall of dark rather than a light?
    pub fn is_gloom(&self) -> bool {
        matches!(self.kind, LightKind::Gloom { .. })
    }

    /// **Does this light have a direction that shading can follow?**
    ///
    /// The question every crescent, every cast shadow and the choice of key
    /// light actually asks. It used to be spelled `!is_ambient()`, which was
    /// the same set only for as long as a sky was the one kind with no
    /// direction — a gloom has none either, and it must no more throw a
    /// terminator than the dark under a table does.
    pub fn is_directional(&self) -> bool {
        matches!(self.kind, LightKind::Sun { .. } | LightKind::Lamp { .. })
    }

    /// The part of a fingerprint this one light contributes, so
    /// [`LightRig::fingerprint`] and anything measuring one light agree on what
    /// counts as a change.
    ///
    /// **This is no longer a cache key**, and a per-light version of it used to
    /// be. Keying generated geometry here meant that brightening a lamp, or
    /// warming it, or switching its shadows off, threw away every boolean the
    /// lamp had ever lit — because all of those are "a change to this light",
    /// and only a hash could not tell them from the one change that moves a
    /// crescent. What the cache keys on now is the crescent's own direction;
    /// see `buzz_render::lighting` and [`crate::crescent_direction`].
    fn hash_into(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        fn f(hasher: &mut impl std::hash::Hasher, v: f64) {
            v.to_bits().hash(hasher);
        }
        fn colour(hasher: &mut impl std::hash::Hasher, c: Color) {
            for channel in c.components {
                channel.to_bits().hash(hasher);
            }
        }

        self.id.0.hash(hasher);
        colour(hasher, self.color);
        self.intensity.to_bits().hash(hasher);
        self.enabled.hash(hasher);
        self.shadows.hash(hasher);
        self.shadow_strength.to_bits().hash(hasher);
        f(hasher, self.standing_height);
        f(hasher, self.softness);
        self.glow.to_bits().hash(hasher);
        self.flicker.to_bits().hash(hasher);
        self.rim.to_bits().hash(hasher);
        match &self.kind {
            LightKind::Sun { azimuth, elevation } => {
                0u8.hash(hasher);
                f(hasher, *azimuth);
                f(hasher, *elevation);
            }
            LightKind::Sky { horizon } => {
                1u8.hash(hasher);
                colour(hasher, *horizon);
            }
            LightKind::Lamp {
                position,
                height,
                radius,
            } => {
                2u8.hash(hasher);
                f(hasher, position.x);
                f(hasher, position.y);
                f(hasher, *height);
                f(hasher, *radius);
            }
            LightKind::Gloom {
                edge,
                facing,
                throw,
                width,
            } => {
                3u8.hash(hasher);
                f(hasher, edge.x);
                f(hasher, edge.y);
                f(hasher, *facing);
                f(hasher, *throw);
                f(hasher, *width);
            }
        }
    }

    /// Where the light is, as seen from `point` on a surface at `depth`.
    ///
    /// Returns the unit vector **towards the light** and how strongly it
    /// arrives. `None` for a sky, which has no direction.
    pub fn towards(&self, point: Point, depth: f64) -> Option<(Vec3, f32)> {
        match self.kind {
            // Neither has a direction that light arrives from: a sky arrives
            // from all of them, and a gloom does not arrive at all. Answering
            // `None` here is what keeps a gloom out of every downstream sum
            // without a special case in any of them — no direct light, no
            // crescent to turn, no shadow to cast.
            LightKind::Sky { .. } | LightKind::Gloom { .. } => None,
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

    /// **This one light's direct contribution at `point`**, in linear light.
    ///
    /// The per-light half of [`LightRig::illuminate`], split out so that the
    /// point answer and the *field* answer ([`LightRig::field`]) are the same
    /// arithmetic rather than two copies of it that can drift apart. A sky
    /// contributes nothing here: its light is the ambient, and it arrives with
    /// no direction.
    pub fn direct_at(&self, point: Point, depth: f64) -> [f32; 3] {
        let Some((towards, strength)) = self.towards(point, depth) else {
            return [0.0; 3];
        };
        // Flat artwork faces the camera, so the surface normal is `+z` and
        // `N·L` is simply how far the light is *in front of* the stage. A
        // light at the horizon therefore adds almost no fill and casts a
        // very long shadow, which is exactly right.
        let facing = towards.z.max(0.0) as f32;
        let c = to_linear(self.color);
        [
            c[0] * facing * strength,
            c[1] * facing * strength,
            c[2] * facing * strength,
        ]
    }

    /// A lamp's direct contribution at planar distance `r` from where it
    /// stands.
    ///
    /// **This is the whole reason a lamp can be drawn as a gradient.** Both
    /// terms in [`direct_at`](Self::direct_at) — the inverse-square falloff and
    /// how square-on the light strikes — depend on the surface point only
    /// through `|p − position|`, so a lamp's light is *radially symmetric in
    /// document space* about the point it stands over. A radial gradient centred
    /// there is not an approximation of it; it is that function, exactly, up to
    /// the resolution of the ramp.
    ///
    /// Answered by asking [`direct_at`](Self::direct_at) about a point at that
    /// distance, so the two can never disagree. `None` for anything but a lamp.
    pub fn direct_at_radius(&self, r: f64, depth: f64) -> Option<[f32; 3]> {
        let LightKind::Lamp { position, .. } = self.kind else {
            return None;
        };
        Some(self.direct_at(Point::new(position.x + r.max(0.0), position.y), depth))
    }

    /// The ambient colour this light contributes at `point`.
    ///
    /// Only a sky contributes ambient, and it mixes its two colours by how
    /// high on the stage the point is — which is what makes a sky read as a
    /// sky rather than as a flat wash.
    /// **The colour only.** Strength is applied by the caller, in linear light.
    ///
    /// It used to be folded in here with `multiply_alpha`, which multiplies a
    /// colour's *alpha* — and the only reader, [`LightRig::illuminate`], takes
    /// the three colour channels and drops the alpha on the floor. So a sky's
    /// Strength slider moved a number that nothing ever read: the one control
    /// that could have made a sky brighter did nothing at all, at any setting,
    /// which is most of what "the sky does not work" meant.
    pub fn ambient_at(&self, point: Point, stage_height: f64) -> Option<Color> {
        let LightKind::Sky { horizon } = self.kind else {
            return None;
        };
        let t = if stage_height > 0.0 {
            (point.y / stage_height).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some(mix(self.color, horizon, t as f32))
    }
}

/// **How far a highlight is pushed towards the light's own colour.**
///
/// Raised with the same change that narrowed [`crate::HIGHLIGHT_SHARE`], and
/// for the same reason: the two are one decision. A broad band at a little of
/// the light's colour reads as the artwork having been painted in two tones; a
/// narrow band at a lot of it reads as an edge catching the light, which is
/// what a highlight is for. Narrowing without brightening would only have made
/// the wash smaller.
const RIM_MIX: f32 = 0.78;

/// What a guttering flame drops towards: the colour of the ember rather than a
/// dimmer copy of the flame.
const EMBER: Color = Color::from_rgb8(0xC2, 0x3A, 0x10);

/// A hash with no state and no crate behind it, for turning a light's id and a
/// frame number into the same number every time.
///
/// Determinism is the whole requirement: the frame an exporter renders on one
/// machine and the frame the window shows on another have to be the same
/// picture, so the gutter cannot come from a random number generator or from
/// the clock.
fn hash01(a: u64) -> f64 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Smoothed value noise at `t`, in `0..=1`: a random number per whole step,
/// eased between them so the result is a wander rather than a staircase.
fn wobble(seed: u64, t: f64) -> f64 {
    let base = t.floor();
    let f = t - base;
    // Smoothstep, so the curve leaves each sample flat and there is no corner
    // on the frame a step is crossed.
    let s = f * f * (3.0 - 2.0 * f);
    let step = base as i64 as u64;
    let a = hash01(seed ^ step.wrapping_mul(0x2545_F491_4F6C_DD1D));
    let b = hash01(seed ^ step.wrapping_add(1).wrapping_mul(0x2545_F491_4F6C_DD1D));
    a + (b - a) * s
}

/// The gutter for one light at one frame, in `-1..=1`.
///
/// **Two rates.** One reads as a pulse however it is tuned, because the eye
/// finds the period immediately. A slow breath under a fast gutter has no
/// period to find, which is what a flame looks like.
fn flicker_noise(seed: u64, frame: u32) -> f64 {
    let t = f64::from(frame);
    let slow = wobble(seed ^ 0xA1A1_A1A1, t / 7.0);
    let fast = wobble(seed ^ 0xB2B2_B2B2, t / 2.3);
    (slow * 0.6 + fast * 0.4) * 2.0 - 1.0
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

        // The base colour is hashed by its bits: two rigs that differ only in a
        // fill colour must not collide, and `f32` is not `Hash`.
        fn colour(hasher: &mut impl Hasher, c: Color) {
            for channel in c.components {
                channel.to_bits().hash(hasher);
            }
        }

        self.enabled.hash(&mut hasher);
        colour(&mut hasher, self.base);
        self.modelling.to_bits().hash(&mut hasher);
        self.lights.len().hash(&mut hasher);

        // Each light hashes itself, through the same routine `Light::fingerprint`
        // uses, so the rig-wide number and any single light's number always
        // agree on what a change is.
        for light in &self.lights {
            light.hash_into(&mut hasher);
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
    /// A number that changes when — and only when — a crescent could move.
    ///
    /// Not the same thing as [`fingerprint`](Self::fingerprint), which changes
    /// on anything that alters the picture. This changes on the far smaller set
    /// that alters generated *geometry*: how strongly the rig models at all, and
    /// then, per light, where it lies and how soft it is. Colour, strength and
    /// shadow settings are all absent, because none of them turns a terminator.
    ///
    /// The window uses it to tell an off-thread build that is still worth
    /// finishing from one being built for a light that has since moved on.
    pub fn aim(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.modelling.to_bits().hash(&mut hasher);
        for light in self.lights.iter().filter(|l| l.enabled && l.is_directional()) {
            light.id.0.hash(&mut hasher);
            light.softness.to_bits().hash(&mut hasher);
            match light.kind {
                LightKind::Sky { .. } | LightKind::Gloom { .. } => {}
                LightKind::Sun { azimuth, elevation } => {
                    azimuth.to_bits().hash(&mut hasher);
                    elevation.to_bits().hash(&mut hasher);
                }
                LightKind::Lamp {
                    position, height, ..
                } => {
                    position.x.to_bits().hash(&mut hasher);
                    position.y.to_bits().hash(&mut hasher);
                    height.to_bits().hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }

    /// Is there anything that would change how the document looks?
    pub fn is_active(&self) -> bool {
        self.enabled && self.lights.iter().any(|l| l.enabled)
    }

    /// Does any light in the rig animate?
    ///
    /// A gutter counts. It has no keys and no length — it is a function of the
    /// frame — but it is the same question the renderer is asking: does this rig
    /// have to be resolved again for the frame being drawn, or is what is on the
    /// `LightRig` already the answer?
    pub fn animates(&self) -> bool {
        self.lights
            .iter()
            .any(|l| l.flicker > 0.0 || l.track.as_ref().is_some_and(|t| t.animates()))
    }

    /// The rig with every animated light resolved to its state at `frame`.
    ///
    /// Borrowed when nothing animates — which is every document that never
    /// keyframes a light — so the common case allocates nothing. When something
    /// does animate, one small clone of the rig resolves it, and the renderer
    /// then lights and caches from concrete values exactly as it always has. A
    /// static light in an animated rig keeps its identical fingerprint frame to
    /// frame, so it stays cached; see [`crate::track`].
    pub fn resolved_at(&self, frame: u32) -> Cow<'_, LightRig> {
        if !self.animates() {
            return Cow::Borrowed(self);
        }
        let mut rig = self.clone();
        for light in &mut rig.lights {
            if let Some(track) = light.track.clone()
                && track.animates()
            {
                *light = track.state_at(frame, light);
            }
            // After the track, not before: the keys say how bright the fire is
            // meant to be at this point in the shot, and the gutter is what
            // happens around that.
            if light.flicker > 0.0 {
                *light = light.flickered(frame);
            }
        }
        Cow::Owned(rig)
    }

    /// The highest frame any light in the rig is keyed at, for working out the
    /// document's animated length.
    pub fn last_animated_frame(&self) -> u32 {
        self.lights
            .iter()
            .filter_map(|l| l.track.as_ref())
            .filter(|t| t.animates())
            .map(|t| t.last_frame())
            .max()
            .unwrap_or(0)
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
        self.lights.iter().filter(|l| l.enabled && l.is_directional())
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

    /// **A wall of dark aimed against the key light.**
    ///
    /// The gloom an animator wants nine times out of ten, and the one gesture
    /// that is tedious to build by hand: stand it off the side of `stage` the
    /// key light is *not* on, turn it to face back across the stage, and throw
    /// it far enough to die somewhere near the light. What the picture gains is
    /// the thing a rig of lights alone cannot give it — the dark end moving as
    /// well as the bright one, so the same lamp reads twice as strong without
    /// being turned up at all.
    ///
    /// Sized from the stage rather than from the lamp: the failure to avoid is
    /// an edge of the quad landing inside the frame, and the stage is what
    /// says where the frame is. Both ends and both sides finish outside it.
    ///
    /// With no directional light to oppose — an empty rig, or one of nothing
    /// but sky — the dark comes in from the left, which is a direction the
    /// animator can then turn rather than a refusal to make one.
    pub fn opposing_gloom(&self, stage: Rect) -> LightKind {
        let centre = stage.center();
        let span = stage.width().hypot(stage.height()).max(1.0);

        // Which way the key light lies, seen from the middle of the stage. A
        // light straight in front of the stage has no bearing in the plane at
        // all, and normalising a zero vector is where a NaN would come from.
        let towards = self
            .key()
            .and_then(|light| light.towards(centre, 0.0))
            .map(|(direction, _)| direction.planar())
            .filter(|planar| planar.hypot() > 1e-6)
            .map(|planar| planar / planar.hypot())
            .unwrap_or(Vec2::new(1.0, 0.0));

        LightKind::Gloom {
            // Half a stage-diagonal back from the middle, along the line to the
            // light: far enough that the near face is off the frame whichever
            // way round the stage the light happens to be.
            edge: centre - towards * (span * 0.5),
            facing: towards.y.atan2(towards.x),
            // A full diagonal of throw puts the far end level with the light
            // rather than short of it, so the fade runs the whole width of the
            // picture instead of stopping in the middle of it.
            throw: span,
            width: span * 1.6,
        }
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
                // In linear light, and on the colour rather than the alpha —
                // see `ambient_at`. This is what makes a sky's Strength do
                // anything at all.
                let k = light.intensity.clamp(0.0, 4.0);
                let c = to_linear(colour);
                ambient = [
                    ambient[0] + c[0] * k,
                    ambient[1] + c[1] * k,
                    ambient[2] + c[2] * k,
                ];
            }
        }

        let mut direct = [0.0f32; 3];
        for light in self.lights.iter().filter(|l| l.enabled && l.is_directional()) {
            // **A lamp lights the artwork, and it used to be left out of here.**
            //
            // The argument for leaving it out was that this term is one colour
            // for the whole shape, taken at the shape's middle — right for a
            // sun, whose parallel rays deliver the same light everywhere, and
            // wrong for a lamp, whose whole character is that it falls off. A
            // wall filled with the single colour found at its centre shows no
            // hot spot and no falloff, so the lamp laid a pool instead
            // (`light_pool`) and touched the artwork not at all.
            //
            // **But "not at all" is the worse error.** A lamp then had no
            // colour you could put on a face, and carrying it across the stage
            // changed nothing about the figure it was carried towards. Moving a
            // lamp closer to a character has to make the near side of that
            // character brighter — that is what a lamp *is* — and the version
            // that only laid a pool could not do it, however bright the lamp.
            //
            // So it is summed here like any other light, with the inverse-square
            // falloff `Light::towards` already works out, and the flat-wall case
            // is met where it actually lives: a shape is lit by the light
            // arriving at *it*, so a face near the lamp is lit more than a wall
            // behind it, and the terminator across each shape comes from the
            // crescents, which take their direction from this same lamp. The
            // pool stays, because a pool is light in the air rather than light
            // on a surface, and `glow` is what turns it down.
            let d = light.direct_at(point, depth);
            direct[0] += d[0];
            direct[1] += d[1];
            direct[2] += d[2];
        }

        Illumination {
            ambient,
            direct,
            key: self.key().map(|l| l.id),
        }
    }

    /// How a **region** at `depth` is lit, rather than a point.
    ///
    /// # Why a region, and what was wrong with a point
    ///
    /// [`illuminate`](Self::illuminate) answers for one point, and the renderer
    /// asked it once per shape, at that shape's middle. For a sun that is not an
    /// approximation — parallel rays deliver the same light everywhere, so one
    /// colour for the shape *is* the answer. For a lamp it is the whole defect:
    /// a lamp is defined by falling off, and a shape filled with the single
    /// colour found at its centre shows no falloff at all. A wall under a lamp
    /// came out flat; the near side of a face was the same brightness as the far
    /// side; carrying a lamp across the stage changed one number per shape and
    /// nothing within any of them.
    ///
    /// A lamp's light is radially symmetric about the point it stands over — see
    /// [`Light::direct_at_radius`] — so it is *exactly* a radial ramp in document
    /// space. This returns that ramp, and the renderer lays it over the artwork
    /// as a gradient. The falloff then lands per pixel: bright where the lamp is
    /// near, dark where it is far, across a single shape as readily as across the
    /// stage.
    ///
    /// # One lamp ramps; the rest stay flat
    ///
    /// Light adds, and two radial ramps do not sum to a third, so only one lamp
    /// can be drawn as a gradient without a group and a pass per light. The one
    /// chosen is the lamp whose brightness varies *most* across these bounds —
    /// which is precisely the one for which a flat answer would be most wrong.
    /// Every other lamp, and every sun, is taken at the middle exactly as before.
    /// With a single lamp — the ordinary case — the result is exact.
    ///
    /// A lamp so far away, or so weak, that it does not vary measurably across
    /// the region is left flat too: a gradient whose ends match is a solid that
    /// costs more to draw.
    pub fn field(&self, bounds: Rect, depth: f64, stage_height: f64) -> LightField {
        if !self.is_active() {
            return LightField::unlit();
        }
        let here = bounds.center();

        let mut ambient = to_linear(self.base);
        for light in self.lights.iter().filter(|l| l.enabled) {
            if let Some(colour) = light.ambient_at(here, stage_height) {
                let k = light.intensity.clamp(0.0, 4.0);
                let c = to_linear(colour);
                ambient = [
                    ambient[0] + c[0] * k,
                    ambient[1] + c[1] * k,
                    ambient[2] + c[2] * k,
                ];
            }
        }

        // The lamp with the most to say across these bounds.
        let mut ramping: Option<(f32, LightId, f64, f64)> = None;
        for light in self.lights.iter().filter(|l| l.enabled && l.is_directional()) {
            let LightKind::Lamp { position, .. } = light.kind else {
                continue;
            };
            let (near, far) = radii(bounds, position);
            let (Some(a), Some(b)) = (
                light.direct_at_radius(near, depth),
                light.direct_at_radius(far, depth),
            ) else {
                continue;
            };
            let varies = (luma(a) - luma(b)).abs();
            if varies > VARIES && ramping.is_none_or(|(most, ..)| varies > most) {
                ramping = Some((varies, light.id, near, far));
            }
        }

        // Everything not being ramped, taken at the middle.
        let mut uniform = [0.0f32; 3];
        for light in self.lights.iter().filter(|l| l.enabled && l.is_directional()) {
            if ramping.is_some_and(|(_, id, ..)| id == light.id) {
                continue;
            }
            let d = light.direct_at(here, depth);
            uniform = [uniform[0] + d[0], uniform[1] + d[1], uniform[2] + d[2]];
        }

        let lamp = ramping.and_then(|(_, id, ..)| {
            let light = self.get(id)?;
            let LightKind::Lamp {
                position, radius, ..
            } = light.kind
            else {
                return None;
            };
            // **The ramp spans the lamp, not the shape.**
            //
            // Fitting it to each shape's own bounds would put more stops where
            // they are read, and would cost a fresh gradient — three, with the
            // screen and the shading tone — for every shape on the stage, every
            // frame. Measured on four hundred shapes that was half the cost of
            // drawing them.
            //
            // Spanning the lamp instead makes the ramp a property of the *light*
            // rather than of what it falls on, so every shape it reaches asks for
            // the same one and it is built once a frame. Past the last stop a
            // gradient pads, which is the honest answer out there anyway: at
            // three radii a lamp is delivering about two per cent of what it
            // delivers underneath itself, and a shape that far out has no falloff
            // worth ramping — it takes the flat answer and never reaches here.
            let reach = (radius.max(1.0) * RAMP_REACH).max(1e-6);
            let mut stops: Vec<(f64, [f32; 3])> = Vec::with_capacity(RAMP_STOPS);
            for i in 0..RAMP_STOPS {
                let t = i as f64 / (RAMP_STOPS - 1) as f64;
                stops.push((t, light.direct_at_radius(t * reach, depth)?));
            }
            Some(LampRamp {
                centre: position,
                reach,
                stops,
            })
        });

        LightField {
            ambient,
            uniform,
            lamp,
            key: self.key().map(|l| l.id),
        }
    }
}

/// How much a lamp's brightness has to move across a region before it is worth
/// drawing as a ramp rather than as one flat tint.
///
/// In linear light, so it is a fraction of full: two per cent is below what a
/// gradient can show in eight bits over the width of a shape, and above the
/// noise of a lamp that is simply a long way off.
const VARIES: f32 = 0.02;

/// How many places the lamp's falloff is sampled.
///
/// Inverse-square is a curve and a gradient ramp is straight between its stops.
/// Fifteen is what a gradient may carry, and the curve is flattest exactly where
/// the stops are sparsest — out at the edge of the reach, where the lamp has
/// almost nothing left to deliver.
const RAMP_STOPS: usize = 15;

/// How far past its half-strength radius a lamp's ramp is drawn.
///
/// The same three the pool uses, and for the same reason: the falloff goes as
/// the cube of the distance out there, so at three radii a lamp delivers about
/// two per cent of what it delivers underneath itself.
const RAMP_REACH: f64 = 3.0;

/// How near and how far `bounds` gets to `at`, in the plane.
///
/// Zero for the near distance when the point is inside, which is right: a lamp
/// standing over a shape lights the ground directly under itself.
fn radii(bounds: Rect, at: Point) -> (f64, f64) {
    let dx = (bounds.x0 - at.x).max(0.0).max(at.x - bounds.x1);
    let dy = (bounds.y0 - at.y).max(0.0).max(at.y - bounds.y1);
    let near = dx.hypot(dy);
    // The farthest corner.
    let fx = (at.x - bounds.x0).abs().max((at.x - bounds.x1).abs());
    let fy = (at.y - bounds.y0).abs().max((at.y - bounds.y1).abs());
    (near, fx.hypot(fy))
}

/// Perceived brightness of a linear-light triple, for comparing two lights.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
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
        mix(self.apply(base), light, strength.clamp(0.0, 1.0) * RIM_MIX)
    }

    /// How far a highlight over **artwork that cannot be recoloured** is
    /// pushed towards the light. The `t` of [`highlight`](Self::highlight), for
    /// a caller that must lay the light's colour over the picture at an alpha
    /// rather than mix it into one.
    pub fn highlight_strength(strength: f32) -> f32 {
        strength.clamp(0.0, 1.0) * RIM_MIX
    }

    /// The light itself, **as colours to composite with**.
    ///
    /// # Why this exists
    ///
    /// [`apply`](Self::apply) takes a colour and returns it lit, which is the
    /// whole model for artwork made of coloured regions. A **bitmap** has no
    /// such colour to take: it is thirty million pixels, and rewriting them for
    /// a light would cost more than the frame it is drawn in. So a photograph
    /// went through `Paint::map_colors`, which cannot touch an image, and came
    /// out exactly as painted — no tint, no shading, no highlight, on every
    /// imported drawing and everything Break Apart produces. That is the
    /// difference between "the lights work" and "the lights do nothing" for a
    /// document made of pictures.
    ///
    /// The light is laid *over* the picture instead of folded into it, which
    /// the GPU does per pixel for the cost of one layer.
    ///
    /// # What the two colours are
    ///
    /// `multiply` is the light where it is no brighter than full. Multiplying
    /// is exactly what [`tint`](Self::tint) does — `source × light` — so over a
    /// bitmap the composited result is the same arithmetic the vector path
    /// takes, per pixel.
    ///
    /// `screen` carries whatever is left above full, because a multiply can
    /// only darken. It brightens towards white without reaching it, which is
    /// the same choice a lamp's pool makes and for the same reason: `Plus`
    /// blows past white the moment a bright light meets a pale surface.
    /// **It is an approximation** — the exact shoulder in `tint` is a function
    /// of the pixel, which is the one thing a blend mode does not know — and it
    /// is only reached by lights brighter than full, which the defaults are not.
    ///
    /// # Encoded, not linear
    ///
    /// The compositor multiplies encoded values, so the factor is encoded too:
    /// under the sRGB transfer curve `srgb(s·L) ≈ srgb(s)·srgb(L)`, which is
    /// what lands the composited result where `tint` puts it.
    pub fn as_filter(&self, direct: bool) -> LightFilter {
        let mut multiply = [0.0f32; 3];
        let mut screen = [0.0f32; 3];
        let mut brightens = false;

        for i in 0..3 {
            let light = self.ambient[i] + if direct { self.direct[i] } else { 0.0 };
            multiply[i] = light.clamp(0.0, 1.0);
            // `1 − 1/L` is what a screen pass has to carry to take a surface
            // already at `source` up towards `source · L`. Zero at or below
            // full, so an ordinary light asks for no second pass at all.
            let over = 1.0 - 1.0 / light.max(1.0);
            screen[i] = over.clamp(0.0, 1.0);
            brightens |= screen[i] > 0.0;
        }

        LightFilter {
            multiply: from_linear(multiply, 255),
            screen: brightens.then(|| from_linear(screen, 255)),
        }
    }

    /// The step from **fully lit to shaded**, as a colour to multiply by.
    ///
    /// The shaded side of a bitmap cannot be painted with `apply_shaded`, for
    /// the same reason the lit side cannot be painted with `apply`. But the
    /// picture is already lit by then — [`as_filter`](Self::as_filter) has run
    /// over the whole shape — so the crescent only has to carry the *ratio*
    /// between the two, `ambient / (ambient + direct)`, which is never above
    /// one and is therefore a plain multiply.
    ///
    /// Multiplying composes, so `source × lit × (ambient / lit)` is
    /// `source × ambient` exactly: the shaded side of a photograph lands on the
    /// same colour the shaded side of a drawing does.
    pub fn shade_filter(&self) -> Color {
        let mut ratio = [1.0f32; 3];
        for i in 0..3 {
            let lit = self.ambient[i] + self.direct[i];
            // A channel with no light at all is already black; there is nothing
            // for the crescent to take away, and dividing would be a NaN.
            ratio[i] = if lit > 0.0 {
                (self.ambient[i] / lit).clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        from_linear(ratio, 255)
    }
}

/// **How a region is lit**, when that is not one answer.
///
/// The spatial form of [`Illumination`]: the part of the light that is the same
/// everywhere over the region, plus at most one lamp's radial falloff across it.
/// Built by [`LightRig::field`], which is where the reasoning lives.
///
/// A field with no lamp is a plain [`Illumination`] and says so through
/// [`uniform`](Self::uniform) — the renderer takes its fast path and encodes
/// exactly what it always did.
#[derive(Debug, Clone, PartialEq)]
pub struct LightField {
    ambient: [f32; 3],
    /// Direct light that does not vary usefully here.
    uniform: [f32; 3],
    lamp: Option<LampRamp>,
    pub key: Option<LightId>,
}

/// One lamp's falloff across a region: where it stands, how far the ramp runs,
/// and what it delivers along the way.
#[derive(Debug, Clone, PartialEq)]
struct LampRamp {
    centre: Point,
    reach: f64,
    /// `(offset along the ramp, this lamp's direct light there)`, in linear
    /// light, ordered outwards from `centre`.
    stops: Vec<(f64, [f32; 3])>,
}

impl LampRamp {
    /// The lamp's own light at `t` along the ramp, straight between stops —
    /// which is how a gradient reads it, so this and the drawn pixel agree.
    fn sample(&self, t: f64) -> [f32; 3] {
        let Some(first) = self.stops.first() else {
            return [0.0; 3];
        };
        if t <= first.0 {
            return first.1;
        }
        for pair in self.stops.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.0 {
                let span = b.0 - a.0;
                let k = if span > 0.0 {
                    ((t - a.0) / span) as f32
                } else {
                    0.0
                };
                return [
                    a.1[0] + (b.1[0] - a.1[0]) * k,
                    a.1[1] + (b.1[1] - a.1[1]) * k,
                    a.1[2] + (b.1[2] - a.1[2]) * k,
                ];
            }
        }
        self.stops.last().map_or([0.0; 3], |s| s.1)
    }
}

impl LightField {
    /// Full daylight: what an unlit document uses, so nothing changes colour.
    pub fn unlit() -> Self {
        Self {
            ambient: [1.0, 1.0, 1.0],
            uniform: [0.0, 0.0, 0.0],
            lamp: None,
            key: None,
        }
    }

    /// Is this doing anything at all?
    pub fn is_neutral(&self) -> bool {
        self.lamp.is_none() && self.uniform().is_neutral()
    }

    /// **The one answer for the whole region**, taken at its middle.
    ///
    /// Exact when nothing varies — which is every rig made of suns and skies,
    /// and every lamp far enough off to be flat here. When a lamp *is* ramping
    /// this is still the honest average to fall back on, and is what artwork
    /// that cannot carry a gradient is lit by.
    pub fn uniform(&self) -> Illumination {
        let along = self.lamp.as_ref().map_or(0.0, |lamp| {
            // The middle of the ramp, so falling back never lands on the
            // brightest or the dimmest end of it.
            (lamp.stops.first().map_or(0.0, |s| s.0) + 1.0) * 0.5
        });
        self.along(along)
    }

    /// The light arriving **at one point** in the region.
    ///
    /// Interpolated from the same ramp the renderer lays as a gradient, so a
    /// value worked out here and the pixel drawn there agree. Used where the
    /// light has to be known at a particular place rather than laid across one —
    /// feathering a terminator, which needs the tone at each step along it.
    pub fn at(&self, point: Point) -> Illumination {
        let along = self.lamp.as_ref().map_or(0.0, |lamp| {
            ((point - lamp.centre).hypot() / lamp.reach).clamp(0.0, 1.0)
        });
        self.along(along)
    }

    /// The light at `t` along the lamp's ramp.
    fn along(&self, t: f64) -> Illumination {
        let mut direct = self.uniform;
        if let Some(lamp) = &self.lamp {
            let d = lamp.sample(t);
            direct = [direct[0] + d[0], direct[1] + d[1], direct[2] + d[2]];
        }
        Illumination {
            ambient: self.ambient,
            direct,
            key: self.key,
        }
    }

    /// The lamp disc this field ramps over — where the lamp stands, in document
    /// space, and how far out the ramp reaches. `None` when nothing varies.
    pub fn disc(&self) -> Option<(Point, f64)> {
        self.lamp.as_ref().map(|l| (l.centre, l.reach))
    }

    /// **A number that changes when — and only when — the ramp would.**
    ///
    /// Turning a ramp into gradients costs three allocations, and every shape one
    /// lamp reaches asks for the same three. This is what lets the renderer build
    /// them once and hand the rest a shared handle: two fields with the same
    /// fingerprint produce identical paints. It covers the fill and the flat
    /// lights as well as the lamp, because those are what the stops are added to
    /// — a sky that mixes by height gives shapes at different heights different
    /// numbers here, and they correctly get their own.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for channel in self.ambient.iter().chain(self.uniform.iter()) {
            channel.to_bits().hash(&mut hasher);
        }
        match &self.lamp {
            None => 0u8.hash(&mut hasher),
            Some(lamp) => {
                1u8.hash(&mut hasher);
                lamp.centre.x.to_bits().hash(&mut hasher);
                lamp.centre.y.to_bits().hash(&mut hasher);
                lamp.reach.to_bits().hash(&mut hasher);
                for (at, light) in &lamp.stops {
                    at.to_bits().hash(&mut hasher);
                    for channel in light {
                        channel.to_bits().hash(&mut hasher);
                    }
                }
            }
        }
        hasher.finish()
    }

    /// The ramp, as `(offset, the light arriving there)`.
    ///
    /// Each illumination is the whole of what reaches that radius — the fill,
    /// every flat light, and the ramping lamp — so a caller can turn it into a
    /// filter with [`Illumination::as_filter`] exactly as it would for a point.
    /// Empty when the field is uniform.
    pub fn ramp(&self) -> Vec<(f64, Illumination)> {
        let Some(lamp) = &self.lamp else {
            return Vec::new();
        };
        lamp.stops
            .iter()
            .map(|(at, d)| {
                (
                    *at,
                    Illumination {
                        ambient: self.ambient,
                        direct: [
                            self.uniform[0] + d[0],
                            self.uniform[1] + d[1],
                            self.uniform[2] + d[2],
                        ],
                        key: self.key,
                    },
                )
            })
            .collect()
    }
}

/// The light as something to lay **over** a picture. See
/// [`Illumination::as_filter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightFilter {
    /// Multiplied into what is already drawn: the light up to full.
    pub multiply: Color,
    /// Screened over it afterwards: whatever the light has above full. `None`
    /// for any light that does not exceed it, which is most of them.
    pub screen: Option<Color>,
}

impl LightFilter {
    /// Does this change the picture at all?
    ///
    /// A rig that is on but delivering full white light must draw no passes:
    /// an unlit document has to encode exactly what it always did.
    pub fn is_neutral(&self) -> bool {
        self.screen.is_none() && self.multiply == Color::WHITE
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


    fn gloom(edge: Point, facing: f64) -> Light {
        Light::new(
            LightId(9),
            "Gloom",
            LightKind::Gloom {
                edge,
                facing,
                throw: 400.0,
                width: 2000.0,
            },
        )
    }

    /// **A gloom takes light away and does nothing else.**
    ///
    /// It has no direction light arrives from, so it must add nothing to the
    /// direct sum, throw no shadow, turn no terminator, and never be picked as
    /// the key. The whole of what it does is drawn — see
    /// [`crate::gloom_band`] — and every one of these is a way the old
    /// `!is_ambient()` reading of "directional" would have let it leak into
    /// arithmetic that is not about it.
    #[test]
    fn a_gloom_neither_lights_nor_shades() {
        let dark = gloom(Point::new(-300.0, 0.0), 0.0);
        assert!(dark.towards(Point::new(50.0, 50.0), 0.0).is_none());
        assert_eq!(dark.direct_at(Point::new(50.0, 50.0), 0.0), [0.0; 3]);
        assert!(!dark.is_directional());
        assert!(crate::shadow_transform(&dark, 70.0).is_none());
        assert!(crate::crescent_direction(&dark, Point::ZERO, 0.0, 1.0).is_none());

        let rig = rig_with(dark);
        assert!(rig.key().is_none(), "darkness is never the key light");
        assert_eq!(rig.casters().count(), 0);
    }

    /// A gloom beside a sun must leave the sun's own answer untouched: adding
    /// darkness is a statement about the picture, not about how the sun lights
    /// the shape it falls on.
    #[test]
    fn adding_a_gloom_does_not_change_what_the_lights_deliver() {
        let mut rig = rig_with(sun(0.6, 0.7));
        let at = Point::new(120.0, 90.0);
        let before = rig.illuminate(at, 0.0, 400.0);

        rig.lights.push(gloom(Point::new(-300.0, 0.0), 0.0));
        assert_eq!(rig.illuminate(at, 0.0, 400.0), before);
        assert_eq!(rig.key().map(|l| l.id), Some(LightId(1)));
    }

    /// **The gesture the feature is for**: one gloom, aimed at the light it is
    /// fighting, without the animator working out a bearing.
    ///
    /// It has to face *towards* the light — the darkness rolls in from the far
    /// side and dies as it nears the lamp — and its wall has to stand outside
    /// the stage, because a wall with an edge inside the frame reads as a grey
    /// rectangle rather than as dark.
    #[test]
    fn a_gloom_is_aimed_against_the_key_light() {
        let stage = Rect::new(0.0, 0.0, 550.0, 400.0);
        // A lamp off to the right of the stage.
        let lamp = Light::new(
            LightId(2),
            "Lamp",
            LightKind::Lamp {
                position: Point::new(900.0, 200.0),
                height: 160.0,
                radius: 320.0,
            },
        );
        let rig = rig_with(lamp);

        let LightKind::Gloom {
            edge,
            facing,
            throw,
            width,
        } = rig.opposing_gloom(stage)
        else {
            panic!("opposing_gloom must make a gloom");
        };

        assert!(
            edge.x < stage.x0,
            "the wall stands off the side away from the lamp, not on the stage: {edge:?}"
        );
        assert!(
            facing.abs() < 0.35,
            "it throws back towards the lamp, which is off to the right: {facing}"
        );
        assert!(
            edge.x + throw * facing.cos() > stage.x1,
            "the throw has to cross the whole picture, not stop inside it"
        );
        assert!(width > stage.height(), "the wall is wider than the stage");
    }

    /// With nothing to oppose there is still a gloom, facing a direction the
    /// animator can then turn. Refusing to make one — or making one with a NaN
    /// bearing out of a zero-length vector — is the failure to avoid.
    #[test]
    fn a_gloom_with_no_light_to_oppose_still_points_somewhere() {
        let rig = LightRig {
            lights: vec![Light::new(LightId(1), "Sky", LightKind::sky())],
            enabled: true,
            ..LightRig::default()
        };
        let LightKind::Gloom { facing, throw, .. } =
            rig.opposing_gloom(Rect::new(0.0, 0.0, 550.0, 400.0))
        else {
            panic!("opposing_gloom must make a gloom");
        };
        assert!(facing.is_finite() && throw.is_finite() && throw > 0.0);
    }

    /// Moving the wall has to be a change the cache can see, or a gloom being
    /// dragged would leave the picture exactly as it was.
    #[test]
    fn moving_a_gloom_changes_the_rigs_fingerprint() {
        let rig = rig_with(gloom(Point::new(-300.0, 0.0), 0.0));
        let mut moved = rig.clone();
        moved.lights[0].kind = LightKind::Gloom {
            edge: Point::new(-280.0, 0.0),
            facing: 0.0,
            throw: 400.0,
            width: 2000.0,
        };
        assert_ne!(rig.fingerprint(), moved.fingerprint());
    }

    fn fire() -> Light {
        let mut lamp = Light::new(
            LightId(3),
            "Fire",
            LightKind::lamp(Point::new(100.0, 100.0)),
        );
        lamp.make_fire();
        lamp
    }

    /// **A fire is never still, and never goes out.**
    ///
    /// Both halves matter. A gutter that repeated, or that only moved every
    /// second frame, reads as a fault in the playback; one that reached zero
    /// would take every shaded edge in the shot with it for a frame and put
    /// them back on the next, which reads as a fault in the drawing.
    #[test]
    fn a_fire_gutters_every_frame_and_never_goes_out() {
        let lamp = fire();
        assert!(lamp.flicker > 0.0, "make_fire must set a gutter");

        let brightness: Vec<f32> = (0..48).map(|f| lamp.flickered(f).intensity).collect();
        for pair in brightness.windows(2) {
            assert_ne!(pair[0], pair[1], "two frames running at the same brightness");
        }
        for (frame, level) in brightness.iter().enumerate() {
            assert!(
                *level > 0.1 && level.is_finite(),
                "frame {frame} guttered out entirely: {level}"
            );
        }

        // It has to *move*, not merely differ in the last decimal.
        let low = brightness.iter().copied().fold(f32::MAX, f32::min);
        let high = brightness.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            high > low * 1.3,
            "the gutter is too small to see: {low} to {high}"
        );
    }

    /// The same film has to render the same on two machines and in two
    /// processes, so the gutter comes from a hash of the frame rather than from
    /// a clock or a random number generator.
    #[test]
    fn a_fires_gutter_is_the_same_every_time_it_is_asked() {
        let lamp = fire();
        for frame in [0, 1, 7, 113, 4096] {
            assert_eq!(
                lamp.flickered(frame).intensity,
                lamp.flickered(frame).intensity
            );
            assert_eq!(lamp.flickered(frame).color, lamp.flickered(frame).color);
        }
    }

    /// Two fires in one shot must not flicker in step, or they read as one
    /// light with two pools.
    #[test]
    fn two_fires_gutter_differently() {
        let mut a = fire();
        let mut b = fire();
        b.id = LightId(4);
        a.id = LightId(3);
        let same = (0..40)
            .filter(|f| a.flickered(*f).intensity == b.flickered(*f).intensity)
            .count();
        assert!(same < 3, "{same} of forty frames matched exactly");
    }

    /// **The gutter must not turn a single crescent.**
    ///
    /// The shading cache is keyed on the direction a light lies in, and a fire
    /// that jittered across the stage would move that direction for every shape
    /// in the film on every frame — a full rebuild per frame, which is the one
    /// cost this whole design exists to avoid. So a gutter moves the brightness
    /// and the colour and nothing else, and [`LightRig::aim`] must be able to
    /// see that.
    #[test]
    fn a_fires_gutter_never_moves_a_shaded_edge() {
        let rig = rig_with(fire());
        let aim = rig.resolved_at(0).aim();
        for frame in 1..30 {
            assert_eq!(
                rig.resolved_at(frame).aim(),
                aim,
                "frame {frame} moved the aim, so every crescent would rebuild"
            );
        }
        // And the position really is untouched.
        assert_eq!(rig.resolved_at(11).lights[0].kind, rig.lights[0].kind);
    }

    /// A rig with a fire in it animates, even with no keyframes anywhere: that
    /// is the question the renderer asks before deciding whether the rig on the
    /// document is already the answer for this frame.
    #[test]
    fn a_fire_makes_a_rig_animate_without_any_keys() {
        let steady = rig_with(Light::new(LightId(1), "Lamp", LightKind::lamp(Point::ZERO)));
        assert!(!steady.animates());
        assert!(rig_with(fire()).animates());
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
