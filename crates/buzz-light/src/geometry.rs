//! Light as geometry: the shading crescent, the highlight, and the cast
//! shadow.
//!
//! # Why geometry rather than pixels
//!
//! The obvious way to light a picture is per-pixel: normals, a dot product, a
//! fragment shader. This renderer draws vector paths through Vello, which has
//! no hook for a shader of ours — but more importantly, pixels would be the
//! wrong answer anyway. Everything this produces is a **path**, so it survives
//! unbounded zoom like the rest of the artwork, exports at any resolution, and
//! can be inspected, cached and reasoned about.
//!
//! It is also what a hand-drawn shadow *is*: a shape, offset from the artwork
//! it belongs to.
//!
//! # The three shapes, and how each is built
//!
//! * **Shade** — the artwork minus a copy of itself shifted *towards* the
//!   light. What remains is a crescent on the far side: the terminator.
//! * **Highlight** — the artwork minus a copy shifted *away*, leaving the
//!   crescent nearest the light.
//! * **Cast shadow** — the artwork projected onto the surface behind it. For a
//!   sun that is a translation, because its rays are parallel. For a lamp it
//!   is a **scale about the lamp's position**, which is what similar triangles
//!   give you and is why a lamp's shadows splay outwards and grow.

use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _, Vec2};
use peniko::Color;

use crate::{Light, LightKind};

/// Everything one light generates for one shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShadeGeometry {
    /// The crescent away from the light.
    pub shade: Option<BezPath>,
    /// The crescent towards it.
    pub highlight: Option<BezPath>,
    /// The shadow thrown onto whatever is behind.
    pub cast: Option<BezPath>,
}

impl ShadeGeometry {
    pub fn is_empty(&self) -> bool {
        self.shade.is_none() && self.highlight.is_none() && self.cast.is_none()
    }
}

/// How far the crescents reach into the shape.
///
/// Proportional to the shape's *smaller* side, so a long thin limb gets a
/// crescent along its length rather than one that swallows it whole.
pub fn crescent_offset(bounds: Rect, direction: Vec2, width: f64) -> Vec2 {
    let extent = bounds.width().min(bounds.height()).max(1e-6);
    let reach = extent * width.clamp(0.0, 0.9);
    let length = direction.hypot();
    if length <= f64::EPSILON {
        return Vec2::ZERO;
    }
    direction * (reach / length)
}

/// **How much of the form the light leaves in shade**, from a light's softness.
///
/// # Why this is not just the softness
///
/// Softness used to *be* the width: the band was `softness` of the shape across,
/// so turning it down turned the shading down with it. At the bottom of the
/// slider a character had a two-per-cent rim of dark along one edge and nothing
/// else, which is not a hard light — it is no light modelling at all. The
/// complaint it produced was that the softness slider did not work.
///
/// A hard light does not shade *less*. It shades exactly as much and gets there
/// in one step: the terminator is a line rather than a gradient. So the width
/// has a floor, and below it softness stops changing how much of the form is
/// dark and changes only how sharply the dark begins — see [`shade_feather`].
///
/// Above the floor a softer light does also wrap further round the form, which
/// is true of real ones: a big source lights round a curve that a small one
/// leaves black.
pub fn shade_width(softness: f64) -> f64 {
    softness.clamp(0.0, 1.0).max(HARD_SHADE_WIDTH).min(0.9)
}

/// **The narrowest a shaded side gets**, as a fraction of the form.
///
/// What a hard light leaves dark. Not a sliver: the point of a hard light is a
/// crisp terminator across a properly shaded form, and a form with a hairline
/// of dark down one edge reads as unlit artwork with a defect.
const HARD_SHADE_WIDTH: f64 = 0.3;

/// **How gradually the shade arrives**, as the fraction of the band the
/// terminator ramps over.
///
/// Zero at a softness of zero, which is the whole point: no ramp, one step from
/// lit to shaded, an exact boundary. A small source — the sun through a gap, a
/// bare bulb, anything far away or tiny — throws a terminator you could cut
/// yourself on, and that is what the bottom of the slider now means.
///
/// At the top the ramp spans the band, which is a window on an overcast day.
pub fn shade_feather(softness: f64) -> f64 {
    softness.clamp(0.0, 1.0)
}

/// The shaded crescent on the side away from the light.
///
/// `towards` points from the artwork **towards** the light, in stage
/// coordinates. Returns `None` when the light is directly in front — there is
/// no terminator on a shape lit head-on, which is correct and is also why a
/// noon sun looks flat.
pub fn shade_crescent(path: &BezPath, towards: Vec2, softness: f64) -> Option<BezPath> {
    let bounds = path.bounding_box();
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return None;
    }
    let offset = crescent_offset(bounds, towards, shade_width(softness));
    if offset.hypot() < 1e-6 {
        return None;
    }

    // The artwork, minus itself shifted towards the light: what is left is the
    // part the light no longer reaches.
    let shifted = Affine::translate(offset) * path.clone();
    difference(path, &shifted)
}

/// How much narrower the highlight is than the shade.
///
/// A highlight is a glint, and one as wide as the terminator reads as a second
/// light rather than as sheen. Named because the renderer has to feather the
/// band across exactly the width it was built with, and guessing the number
/// twice is how the two drift apart.
///
/// **A third narrower than it was.** At 0.45 the highlight was a broad band
/// down one side of every shape: it lit the figure, but what it read as was the
/// artwork having been painted in two tones, not as light catching an edge. The
/// complaint it produced was that a lamp changes the overall colour of a
/// drawing and nothing else — which is exactly right, because a wash and a
/// broad band are both washes.
///
/// The width is half of it. The other half is what the band is *filled* with,
/// and that has since stopped being a mix towards the light's colour and become
/// a screen of it — see `Illumination::highlight` and `GLINT_LIGHT`. A narrow
/// band that adds the light to what the artwork already is, is what an edge
/// catching the light looks like; a broad one that replaces it is a second
/// drawing.
///
/// **Not narrower than this.** Below about a quarter the band stops carrying
/// enough of the light's colour for the frame as a whole to read as lit at all;
/// `stage_lighting::a_default_sun_lights_rather_than_dims` measures exactly
/// that and fails at a fifth. The renderer pays for the band in the same fill
/// either way, so a rim costs nothing the highlight did not already cost.
pub const HIGHLIGHT_SHARE: f64 = 0.30;

/// **How far a shade crescent reaches in from the far edge of the shape.**
///
/// The band's thickness along the light's own direction, which is the distance
/// a feathered terminator has to ramp over. Measured from the same offset the
/// geometry is built from, so the ramp and the shape it fills always agree.
pub fn shade_reach(bounds: Rect, towards: Vec2, softness: f64) -> f64 {
    crescent_offset(bounds, towards, shade_width(softness)).hypot()
}

/// [`shade_reach`], for the narrower highlight band.
pub fn highlight_reach(bounds: Rect, towards: Vec2, softness: f64) -> f64 {
    crescent_offset(bounds, -towards, shade_width(softness) * HIGHLIGHT_SHARE).hypot()
}

/// The lit crescent on the side towards the light.
pub fn highlight_crescent(path: &BezPath, towards: Vec2, softness: f64) -> Option<BezPath> {
    let bounds = path.bounding_box();
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return None;
    }
    let offset = crescent_offset(bounds, -towards, shade_width(softness) * HIGHLIGHT_SHARE);
    if offset.hypot() < 1e-6 {
        return None;
    }

    let shifted = Affine::translate(offset) * path.clone();
    difference(path, &shifted)
}

/// The shadow this shape throws onto the surface behind it.
///
/// `height` is how far the artwork stands above that surface, in document
/// units. Returns `None` when the light is at or below the surface — a light
/// on the horizon casts a shadow of infinite length, and an infinite shadow is
/// not a shape.
pub fn cast_shadow(path: &BezPath, light: &Light, at: Point, height: f64) -> Option<BezPath> {
    let _ = at;
    Some(shadow_transform(light, height)? * path.clone())
}

/// **The whole of a cast shadow, as one affine.**
///
/// A shadow is the caster's own outline, moved: translated for a sun, because
/// parallel rays move every point of it the same way, and scaled about the lamp
/// for a lamp, because that is what similar triangles give. Neither needs a
/// boolean, and neither depends on the shape — only on the light and on how far
/// the artwork stands above the surface catching it.
///
/// Separating this from the crescents is the difference between a light you can
/// drag and one you cannot. Shadows used to be built and cached beside the
/// crescents, so aiming a sun threw away the cheap geometry and the expensive
/// geometry together and *neither* could be redrawn until several hundred
/// boolean differences had finished. Now every shadow in the document is one
/// matrix multiply per shape per frame and follows the light exactly, live,
/// however heavy the artwork.
///
/// `None` when this light throws nothing: a sky, a light with shadows switched
/// off, artwork lying on the surface itself, or a light so low that the shadow
/// would run away to infinity.
pub fn shadow_transform(light: &Light, height: f64) -> Option<Affine> {
    if !light.shadows || height <= 0.0 {
        return None;
    }

    match light.kind {
        // A sky has no direction to cast along; a gloom has no light to cast
        // with. Neither throws anything.
        LightKind::Sky { .. } | LightKind::Gloom { .. } => None,

        LightKind::Sun { azimuth, elevation } => {
            // Below the horizon: nothing is lit, so nothing casts.
            if elevation <= 0.02 {
                return None;
            }
            // A caster of `height` under a sun at `elevation` throws a shadow
            // `height / tan(elevation)` long, pointing away from the sun.
            let length = height / elevation.tan();
            // Bounded: a sun a whisker above the horizon would otherwise
            // produce a shadow kilometres long, which is arithmetically right
            // and useless — and very slow to rasterise.
            let length = length.min(height * MAX_SHADOW_RATIO);
            // And then what the animator asked for. Applied after the bound, so
            // the setting means the same thing at every elevation instead of
            // doing nothing wherever the geometry was already clamped.
            let length = length * light.shadow_length.max(0.0) as f64;
            let (sin_a, cos_a) = azimuth.sin_cos();
            let away = Vec2::new(-cos_a, -sin_a) * length;
            Some(Affine::translate(away))
        }

        LightKind::Lamp {
            position,
            height: lamp_height,
            ..
        } => {
            // Similar triangles: a point `height` above the floor, lit from
            // `lamp_height` above it, throws its shadow at
            // `lamp_height / (lamp_height - height)` times its distance from
            // directly under the lamp. That is a scale about the lamp's
            // position — the whole projection, in one affine.
            let gap = lamp_height - height;
            if gap <= 1.0 {
                // The lamp is level with the artwork or below it: the shadow
                // runs off to infinity, so there is nothing sensible to draw.
                return None;
            }
            let scale = (lamp_height / gap).clamp(1.0, MAX_LAMP_SCALE);
            // The multiplier scales how far the shadow is pushed out from under
            // the lamp, not the lamp's arithmetic: at 1 this is the similar
            // triangles above, at 0 the shadow sits under its caster.
            let scale = 1.0 + (scale - 1.0) * light.shadow_length.max(0.0) as f64;
            Some(
                Affine::translate(position.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-position.to_vec2()),
            )
        }
    }
}

/// **How a light throws this layer's shadows**, ready to be asked for one
/// caster at a time.
///
/// A shadow on a **wall** is the same affine for everything on the layer: the
/// surface is parallel to the picture plane, so a translation (a sun) or a
/// scale about the lamp (a lamp) puts every caster's silhouette where it
/// belongs, and the layer can work it out once.
///
/// A shadow on the **ground** cannot be. It is anchored at the caster's own
/// feet — that is what makes it read as a shadow rather than as a copy — so it
/// depends on where the caster stands and how tall it is. Hence a value that
/// carries what the *light* contributes and is asked, per caster, for the rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadowThrow {
    /// One affine for the whole layer.
    Wall(Affine),
    /// Worked out per caster; see [`ShadowThrow::at`].
    Ground {
        /// A sun's fixed bearing, or `None` for a lamp, whose bearing is
        /// wherever the caster happens to stand relative to it.
        away: Option<Vec2>,
        /// Shadow length per unit of caster height — `1 / tan(elevation)` —
        /// for a sun. `None` for a lamp, whose elevation is different from
        /// every point of the stage.
        stretch: Option<f64>,
        /// The lamp, as its position in the plane and its height above the
        /// floor.
        lamp: Option<(Point, f64)>,
    },
}

/// **The least rake a floor is drawn at**, as a fraction of the shadow's own
/// length.
///
/// A shadow lying on the ground is the caster laid flat along it, so its depth
/// on the screen is however much of the floor the picture shows. Taken
/// literally from the light's bearing, a light exactly to the side lays the
/// shadow along a line across the frame with no depth at all — arithmetically
/// right for a floor seen exactly edge-on, and useless, because a line is not a
/// shadow.
///
/// Every 2D stage is drawn from a little above its floor, and this is that
/// little: a shadow always keeps at least this much of its length as depth on
/// the screen, leaning towards the viewer when the light says nothing either
/// way. It scales *with* the length, so a light straight overhead still puts a
/// puddle underfoot rather than a wedge.
const MIN_GROUND_RAKE: f64 = 0.34;

/// **How a light throws shadows onto whatever catches them.**
///
/// `height` is what a wall shadow needs — how far the artwork stands in front
/// of the surface behind it — and is ignored by a ground shadow, which is
/// anchored at the feet and cares only about how high the light is.
///
/// `None` when this light throws nothing: a sky, a gloom, shadows switched off,
/// a light below the horizon, or (on a wall) artwork lying flat on the surface
/// itself.
pub fn shadow_throw(light: &Light, height: f64) -> Option<ShadowThrow> {
    if !light.shadows {
        return None;
    }
    if light.fall == crate::ShadowFall::Wall {
        return shadow_transform(light, height).map(ShadowThrow::Wall);
    }

    match light.kind {
        LightKind::Sky { .. } | LightKind::Gloom { .. } => None,
        LightKind::Sun { azimuth, elevation } => {
            if elevation <= 0.02 {
                return None;
            }
            let (sin_a, cos_a) = azimuth.sin_cos();
            Some(ShadowThrow::Ground {
                away: Some(Vec2::new(-cos_a, -sin_a)),
                // A caster of height `h` under a light at `elevation` throws a
                // shadow `h / tan(elevation)` long. Bounded for the same reason
                // a wall shadow is: a light on the horizon means a shadow
                // kilometres long, which is right and useless.
                // Bounded first, then scaled by what the animator asked for,
                // so the setting means the same thing at every elevation.
                stretch: Some(
                    (1.0 / elevation.tan()).min(MAX_SHADOW_RATIO)
                        * light.shadow_length.max(0.0) as f64,
                ),
                lamp: None,
            })
        }
        LightKind::Lamp {
            position, height, ..
        } => Some(ShadowThrow::Ground {
            away: None,
            stretch: Some(light.shadow_length.max(0.0) as f64),
            lamp: Some((position, height.max(1.0))),
        }),
    }
}

impl ShadowThrow {
    /// The affine that throws the shadow of a caster standing in `caster`.
    ///
    /// # What the ground projection is
    ///
    /// The caster is flat artwork standing upright on the floor, so the bottom
    /// of its box is where it meets the ground and the rest of it is height
    /// above that. The shadow lays that height down along the floor, away from
    /// the light:
    ///
    /// * the feet stay exactly where they are — that is the whole point, and it
    ///   is what stops a shadow drifting off its owner;
    /// * a point `h` above them lands `h · stretch` away along the light's
    ///   bearing;
    /// * `stretch` is `1 / tan(elevation)`, so a light overhead gives a puddle
    ///   and a low one a long throw.
    ///
    /// For a lamp the elevation is not one number for the stage — it is how
    /// high the lamp is *seen from this caster* — so it is worked out here,
    /// from the lamp's height and how far away it stands. That is what makes a
    /// figure walking away from a lamp grow a longer shadow as it goes.
    pub fn at(&self, caster: Rect) -> Affine {
        match *self {
            Self::Wall(affine) => affine,
            Self::Ground {
                away,
                stretch,
                lamp,
            } => {
                // Where the caster meets the floor: the bottom of its box, and
                // its middle across.
                let base = Point::new((caster.x0 + caster.x1) * 0.5, caster.y1);

                let (away, stretch) = match (away, stretch, lamp) {
                    (Some(away), Some(stretch), _) => (away, stretch),
                    (_, wanted, Some((position, lamp_height))) => {
                        // A lamp has no one elevation for the stage, so its
                        // stretch is worked out per caster here — and the
                        // light's own length setting arrives as `stretch` with
                        // no bearing beside it, to be applied to the result.
                        let asked = wanted.unwrap_or(1.0);
                        let out = base - position;
                        let distance = out.hypot();
                        if distance < 1e-6 {
                            // Directly underneath: no bearing, and no shadow to
                            // speak of either.
                            (Vec2::new(0.0, 1.0), 0.0)
                        } else {
                            // The lamp's elevation from here is
                            // `atan(lamp_height / distance)`, so the stretch —
                            // its cotangent — is `distance / lamp_height`.
                            (
                                out / distance,
                                (distance / lamp_height).min(MAX_SHADOW_RATIO) * asked,
                            )
                        }
                    }
                    // Neither a bearing nor a lamp: nothing to throw along.
                    _ => (Vec2::new(0.0, 1.0), 0.0),
                };

                // The floor is seen from a little above, never exactly
                // edge-on — see `MIN_GROUND_RAKE`. Towards the viewer when the
                // light gives no preference, which is where a floor is.
                //
                // **Towards the viewer on a tie**, and the tie is generous: a
                // light exactly across the stage has a bearing whose vertical
                // part is a rounding error, and taking its sign would decide
                // between a shadow in front of the figure and one hidden behind
                // it on the strength of `sin(pi)`.
                let rake = if away.y > 1e-6 {
                    away.y.max(MIN_GROUND_RAKE)
                } else if away.y < -1e-6 {
                    away.y.min(-MIN_GROUND_RAKE)
                } else {
                    MIN_GROUND_RAKE
                };

                // `h = base.y − y` is height above the floor, so
                //   x' = x + away.x · stretch · h
                //   y' = base.y + rake · stretch · h
                // which is linear in (x, y): one affine, and the same one for
                // every shape of this caster.
                let sx = away.x * stretch;
                let sy = rake * stretch;
                Affine::new([
                    1.0,
                    0.0,
                    -sx,
                    -sy,
                    sx * base.y,
                    base.y * (1.0 + sy),
                ])
            }
        }
    }

    /// **The furthest a shadow can reach from its caster**, for a caster no
    /// taller than `height`. What the renderer grows its culling rectangle by,
    /// so a figure just off the frame whose shadow falls into it is still
    /// drawn.
    pub fn reach(&self, height: f64) -> f64 {
        match *self {
            Self::Wall(affine) => {
                let c = affine.as_coeffs();
                c[4].hypot(c[5])
            }
            Self::Ground { stretch, lamp, .. } => {
                let stretch = stretch.unwrap_or(MAX_SHADOW_RATIO);
                // A lamp's stretch is per caster and is not known here, so the
                // worst case stands in for it — times the light's own length
                // setting, which for a lamp is all `stretch` carries. A reach
                // that under-reports is a shadow culled away at the edge of the
                // frame.
                let stretch = match lamp {
                    Some(_) => MAX_SHADOW_RATIO * stretch,
                    None => stretch,
                };
                height * stretch
            }
        }
    }
}

/// The longest a shadow may be, as a multiple of the caster's height.
///
/// Twelve is a very low sun — about five degrees. Past that the shadow is
/// longer than any stage and its far end is off-screen anyway, so the only
/// thing the extra length costs is rasterisation.
const MAX_SHADOW_RATIO: f64 = 12.0;

/// **The largest a lamp may scale a shadow.**
///
/// A lamp's shadow is a scale *about the lamp's position*, so the factor does
/// two things at once: it makes the shadow bigger, and it throws it further
/// from the caster in proportion to how far the caster already is. At three
/// times, a figure four hundred units from the lamp has its shadow eight
/// hundred units away and three times its size — off the stage, enormous, and
/// attached to nothing. That is the report, and it is not a rounding error: the
/// factor is `lamp_height / (lamp_height − standing_height)`, which is 1.8 at
/// the defaults and **diverges** as the lamp is lowered towards the height the
/// artwork is assumed to stand at. The old bound let it reach twelve.
///
/// Similar triangles say twelve is *correct* for a lamp a whisker above the
/// artwork. It is also useless: what an animator wants from moving a lamp
/// around is what Blender gives them, a shadow that stays attached to the thing
/// casting it and swings around it. Past about twice the caster, a shadow on
/// flat artwork stops reading as that thing's shadow at all.
const MAX_LAMP_SCALE: f64 = 2.0;

/// Everything one light makes for one shape, in one call.
///
/// `at` is where the shape sits, `height` how far it stands above the surface
/// receiving its shadow, and `depth` its layer's depth.
pub fn shade_for(
    path: &BezPath,
    light: &Light,
    at: Point,
    depth: f64,
    height: f64,
    modelling: f32,
) -> ShadeGeometry {
    let mut geometry = match crescent_direction(light, at, depth, modelling) {
        Some(towards) => crescents(path, towards, light.softness),
        None => ShadeGeometry::default(),
    };
    geometry.cast = cast_shadow(path, light, at, height);
    geometry
}

/// Which way the crescents on a shape at `at` face, or `None` if this light
/// draws none there.
///
/// **This is the whole of what a crescent knows about a light.** Not its
/// colour, not its strength, not how high it stands, not whether it casts —
/// every one of those changes the picture and not one of them turns the
/// terminator round.
///
/// Saying so in one function is what lets the shading cache key on a
/// *direction* rather than on a light. A sun climbing the sky, a lamp
/// brightening, a key light warming: all of them keep every crescent in the
/// document, because the cache can see that none of them moved one.
pub fn crescent_direction(light: &Light, at: Point, depth: f64, modelling: f32) -> Option<Vec2> {
    // Modelling switched off means no crescents at all, so there is nothing to
    // aim and nothing to build.
    if modelling <= 0.01 {
        return None;
    }
    // **Across the picture, not across the floor.** See
    // [`Light::screen_towards`]: a sun's bearing is measured on the ground and
    // its height is the part that has to become "up" on the screen, or raising
    // the sun moves nothing but the shadow.
    let planar = light.screen_towards(at, depth)?;
    // A light directly in front has no direction *in the plane*, so it
    // produces no crescents — only fill. Trying to build them from a
    // zero-length vector is where a stray NaN would come from.
    (planar.hypot() > 1e-6).then_some(planar)
}

/// The two crescents for one shape lit from `towards` — the expensive half of
/// lighting, a boolean difference each, and the reason any of this is cached.
///
/// `towards` need not be a unit vector; only its direction is read.
pub fn crescents(path: &BezPath, towards: Vec2, softness: f64) -> ShadeGeometry {
    ShadeGeometry {
        shade: shade_crescent(path, towards, softness),
        highlight: highlight_crescent(path, towards, softness),
        cast: None,
    }
}

/// **The pool of light a lamp lays on the stage**: the light you can actually
/// see, as opposed to what it does to a silhouette.
///
/// # Why a lamp needs one and a sun does not
///
/// A sun's rays are parallel, so the same light arrives everywhere and tinting
/// each shape by one colour is not an approximation — it is the answer. A lamp
/// is defined by the opposite: it falls off, and the falloff *is* the lamp.
///
/// The illumination model evaluates a light once per shape, at the middle of
/// that shape. For a sun that is exact. For a lamp it means a wall under a lamp
/// is filled with one flat colour, the same at the bright end as at the dark
/// end — no pool, no hot spot, nothing that reads as a light being on. Measured
/// on a lamp a hundred units from the left edge of a 550-unit wall: identical
/// pixels at x = 100 and x = 520.
///
/// So the lamp also lays a pool: a radial ramp of its own colour, centred where
/// it stands, following the same inverse-square falloff the shading uses, and
/// screened over the frame. It is a gradient rather than pixels, so it survives
/// unbounded zoom like everything else here, and it costs one filled circle per
/// lamp per frame however much artwork it falls on.
///
/// `None` when there is no pool to draw: any light that is not a lamp, one
/// switched off, one with its glow turned down, or one so weak or so far behind
/// the stage that nothing of it would land.
pub fn light_pool(light: &Light, depth: f64) -> Option<LightPool> {
    let LightKind::Lamp {
        position,
        height,
        radius,
    } = light.kind
    else {
        return None;
    };
    if !light.enabled {
        return None;
    }
    let strength = light.intensity.max(0.0) * light.glow.clamp(0.0, 1.0);
    if strength <= 0.001 {
        return None;
    }
    // How far the lamp stands in front of the surface it is lighting.
    let above = height + depth;
    if above <= 0.0 {
        return None;
    }
    let radius = radius.max(1.0);
    let reach = radius * POOL_REACH;

    // What arrives at a point `along` units from directly under the lamp. The
    // same two terms `Light::towards` uses — the inverse-square falloff, and
    // how square-on the light strikes — so the pool and the shading agree about
    // what this lamp is doing.
    let arriving = |along: f64| {
        let distance = (along * along + above * above).sqrt();
        let falloff = 1.0 / (1.0 + (distance / radius).powi(2));
        let facing = above / distance;
        (f64::from(strength) * facing * falloff) as f32
    };

    let mut ramp: Vec<(f64, f32)> = (0..POOL_STOPS)
        .map(|i| {
            let t = i as f64 / (POOL_STOPS - 1) as f64;
            (t, arriving(t * reach).clamp(0.0, 1.0))
        })
        .collect();
    // The outermost stop is forced to nothing so the pool has an edge rather
    // than a step: past the last stop a gradient pads with it for ever, and a
    // pool that never ended would be a flat wash over the whole document.
    if let Some(last) = ramp.last_mut() {
        last.1 = 0.0;
    }
    // Nothing worth drawing: a lamp behind everything, or turned right down.
    if ramp[0].1 <= 0.004 {
        return None;
    }

    Some(LightPool {
        centre: position,
        reach,
        ramp,
    })
}

/// A lamp's light, as something to draw. See [`light_pool`].
#[derive(Debug, Clone, PartialEq)]
pub struct LightPool {
    /// Where the lamp stands, in document space.
    pub centre: Point,
    /// The radius at which the pool has faded to nothing.
    pub reach: f64,
    /// From the middle outwards: `(fraction of reach, how much of the lamp's
    /// colour arrives there)`. Always ends at zero.
    pub ramp: Vec<(f64, f32)>,
}

/// How far past its half-strength radius a lamp is still worth drawing.
///
/// Three: the falloff goes as the cube of the distance out here, so at three
/// radii a lamp is delivering about two per cent of what it delivers under
/// itself. Further out is a wider circle to rasterise for a difference nobody
/// can see.
const POOL_REACH: f64 = 3.0;

/// How many steps the falloff is sampled at. Inverse-square is a curve and a
/// gradient ramp is straight between its stops, so this is how faithfully the
/// curve is followed — ten is smooth to the eye and well inside the fifteen a
/// gradient may carry.
const POOL_STOPS: usize = 10;

/// **The darkness one gloom lays over the frame.** See [`LightKind::Gloom`].
///
/// The exact counterpart of [`light_pool`], built the same way and for the same
/// reasons: one quad and one linear ramp, rebuilt every frame for the cost of
/// neither, so it follows a wall of dark being dragged and survives unbounded
/// zoom like everything else here.
///
/// # Why the dark is drawn and not tinted
///
/// A lamp does both — it tints the artwork it reaches *and* lays a pool, and
/// [`Light::glow`] is what keeps the two from being the same statement twice,
/// because light on a surface and light in the air are genuinely different
/// things. Darkness has no such pair. Taking light away from a shape's colours
/// and multiplying the finished picture down are the *same* removal, and doing
/// both would take it away twice.
///
/// So a gloom is drawn, and only drawn. That is not the lesser half: a tint is
/// one colour for a whole shape, and this lands per pixel, across a character's
/// face as readily as across the stage. It reaches a photograph and a gradient
/// and a hundred imported layers for the price of one quad, and it needs no
/// entry in any cache, because there is nothing to build.
///
/// `None` when there is nothing to draw: any light that is not a gloom, one
/// switched off, or one turned down until it stops nothing.
pub fn gloom_band(light: &Light) -> Option<GloomBand> {
    let LightKind::Gloom {
        edge,
        facing,
        throw,
        width,
    } = light.kind
    else {
        return None;
    };
    if !light.enabled {
        return None;
    }
    // Stopping more than all of the light means nothing, so this is the one
    // strength in the rig that is a fraction rather than a multiplier.
    let deepest = f64::from(light.intensity).clamp(0.0, 1.0);
    if deepest <= 0.004 {
        return None;
    }

    let throw = throw.max(1.0);
    let width = width.max(1.0);
    let (sin_f, cos_f) = facing.sin_cos();
    let facing = Vec2::new(cos_f, sin_f);

    // What survives, rather than what is taken away, because that is what the
    // renderer multiplies by — and because the interpolation has to happen in
    // linear light. A ramp between two encoded colours passes through a middle
    // that is nothing like half as dark, which is exactly the muddy grey band
    // an eye picks out of a picture immediately.
    let stopped = crate::to_linear(light.color);
    let ramp: Vec<(f64, Color)> = (0..GLOOM_STOPS)
        .map(|i| {
            let t = i as f64 / (GLOOM_STOPS - 1) as f64;
            let deep = (deepest * stopping(t)) as f32;
            let survives = [
                1.0 + (stopped[0] - 1.0) * deep,
                1.0 + (stopped[1] - 1.0) * deep,
                1.0 + (stopped[2] - 1.0) * deep,
            ];
            (t, crate::from_linear(survives, 255))
        })
        .collect();

    Some(GloomBand {
        edge,
        facing,
        throw,
        width,
        ramp,
    })
}

/// A wall of dark, as something to draw. See [`gloom_band`].
#[derive(Debug, Clone, PartialEq)]
pub struct GloomBand {
    /// Where the near face stands, in document space.
    pub edge: Point,
    /// The unit direction it throws along.
    pub facing: Vec2,
    /// How far along that direction the dark has faded to nothing.
    pub throw: f64,
    /// How wide the wall is, across the throw.
    pub width: f64,
    /// From the near face outwards: `(fraction of the throw, the colour the
    /// picture is multiplied by there)`. Always ends at white, which is a
    /// multiply that changes nothing — so the band has an edge rather than a
    /// step, for the same reason a pool's last stop is forced to zero.
    pub ramp: Vec<(f64, Color)>,
}

impl GloomBand {
    /// The quad the band covers, in document space.
    ///
    /// Nothing outside it is touched. A gloom is a shape like everything else
    /// here, which is what makes it aimable: stand it off the stage and only
    /// its long faded tail reaches the picture.
    pub fn quad(&self) -> BezPath {
        let across = self.across();
        let far = self.far();
        let mut path = BezPath::new();
        path.move_to(self.edge - across);
        path.line_to(self.edge + across);
        path.line_to(far + across);
        path.line_to(far - across);
        path.close_path();
        path
    }

    /// Where the throw ends: the point at which the dark has faded to nothing.
    pub fn far(&self) -> Point {
        self.edge + self.facing * self.throw
    }

    /// Half the wall, across the throw.
    fn across(&self) -> Vec2 {
        Vec2::new(-self.facing.y, self.facing.x) * (self.width * 0.5)
    }

    /// **Where the ramp goes**, as the affine a linear gradient wants.
    ///
    /// A gradient's unit space runs `-1..1` along its x axis, so the matrix has
    /// to put the first stop on the near face and the last one at the far end.
    /// The second column is merely non-singular — a linear ramp never reads it,
    /// but a zero column is a matrix that renders as nothing at all.
    pub fn ramp_transform(&self) -> Affine {
        let half = self.facing * (self.throw * 0.5);
        let across = self.across();
        let centre = self.edge + half;
        Affine::new([half.x, half.y, across.x, across.y, centre.x, centre.y])
    }
}

/// How much of the light a gloom stops, `t` of the way along its throw.
///
/// `1 - t²` rather than a straight `1 - t`. A straight fade spends its first
/// half in tones an eye cannot separate and its second half arriving at nothing
/// too fast, and the result reads as a grey wedge with a top edge on it — which
/// is the one thing a wall of dark must not look like. Squared, it holds near
/// full for the first third and then falls away, which is what a long throw
/// actually looks like: darkness that is simply *there*, thinning out.
fn stopping(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - t * t
}

/// How many places the fade is sampled at.
///
/// Fewer than a pool's ten, because this curve is a parabola rather than an
/// inverse square and a gradient's straight segments follow it far more closely
/// — and because it is stretched over a whole stage, where a stop buys less
/// than it does inside a lamp's disc.
const GLOOM_STOPS: usize = 8;

/// **How a gloom falls on one point**: which way its darkness travels, and how
/// much of the light it is taking away there.
///
/// The per-shape half of [`gloom_band`]. The band answers for the *frame* and
/// lands per pixel, which is what makes a wall of dark a wall; this answers for
/// one shape, so the shape can be given a dark edge on the side the darkness is
/// coming from. Without it a gloom is a wash over the picture and the figures
/// standing in it have no form — which is the same complaint a flat tint gets
/// from a light.
///
/// `None` for anything that is not a gloom, one switched off, and any point
/// outside the quad — a gloom does nothing outside its own band, and that is
/// what makes one aimable.
pub fn gloom_at(light: &Light, at: Point) -> Option<(Vec2, f32)> {
    let LightKind::Gloom {
        edge,
        facing,
        throw,
        width,
    } = light.kind
    else {
        return None;
    };
    if !light.enabled {
        return None;
    }
    let deepest = f64::from(light.intensity).clamp(0.0, 1.0);
    if deepest <= 0.004 {
        return None;
    }
    let throw = throw.max(1.0);
    let (sin_f, cos_f) = facing.sin_cos();
    let facing = Vec2::new(cos_f, sin_f);

    let out = at - edge;
    let along = out.dot(facing);
    if along < 0.0 || along > throw {
        return None;
    }
    let across = out.dot(Vec2::new(-facing.y, facing.x)).abs();
    if across > width.max(1.0) * 0.5 {
        return None;
    }
    let deep = deepest * stopping(along / throw);
    (deep > 0.004).then_some((facing, deep as f32))
}

/// Boolean difference, with the tolerance derived from the shapes themselves.
fn difference(a: &BezPath, b: &BezPath) -> Option<BezPath> {
    let bounds = a.bounding_box();
    let options = buzz_geom::BooleanOptions::for_shape_size(bounds.width().hypot(bounds.height()));
    let result = buzz_geom::boolean(a, b, buzz_geom::BoolOp::Difference, options);
    (!result.elements().is_empty()).then_some(result)
}

// ---------------------------------------------------------------------------
// The rim
// ---------------------------------------------------------------------------

/// **The widest a rim spreads**, in document units, at `rim == 1.0`.
///
/// A rim reads as light catching an edge only while it is *narrower than the
/// thing it is on*. Past that it stops being an edge and becomes a halo, and a
/// halo on every character in a shot is fog. Thirty units on a stage a few
/// hundred across is a strong rim on a limb and a visible one on a head.
pub const RIM_REACH: f64 = 30.0;

/// A glow to lay around the outside of a silhouette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RimGlow {
    /// The colour to glow, already carrying how much of it arrives: a rim that
    /// has fallen off across the stage arrives more transparent, not smaller,
    /// because a narrower rim reads as a *nearer* light rather than a dimmer
    /// one.
    pub color: Color,
    /// How far it spreads, in document units.
    pub reach: f64,
}

/// **The edge glow one light lays around artwork at `at` on a layer at
/// `depth`.**
///
/// The answer to "when the light comes up, the edges come up": the strength is
/// the light's own, so anything that moves the light's brightness \u2014 a
/// keyframed intensity, a fire's gutter, walking out of a lamp's reach \u2014 moves
/// the rim with it, and none of it needs a second track to animate.
///
/// # Why this is not a crescent
///
/// A highlight crescent is the artwork minus a copy of itself, so it lives
/// *inside* the silhouette and can never be brighter than the picture around
/// it. What an animator draws as a rim is outside the line, spilling onto the
/// background. It is the same shape a Glow filter makes, and it is built by the
/// same code (`buzz_fx::soft_edge`) from this colour and this reach.
///
/// `None` when there is no rim to draw: the light is off, its rim is turned
/// down, it is not a light at all (a gloom emits nothing), or it is too far
/// away for anything of it to arrive.
pub fn rim_glow(light: &Light, at: Point, depth: f64) -> Option<RimGlow> {
    if !light.enabled {
        return None;
    }
    let rim = light.rim.clamp(0.0, 1.0);
    if rim <= 0.001 {
        return None;
    }

    // How much of this light arrives here. A sun arrives the same everywhere,
    // so it is simply its strength; a lamp's falls off, which is what makes a
    // figure lose its rim as it walks out of the pool. A sky has no direction
    // and a gloom has no light: neither rims anything.
    let arriving = match light.kind {
        LightKind::Sky { .. } | LightKind::Gloom { .. } => return None,
        _ => {
            let (towards, strength) = light.towards(at, depth)?;
            // Square-on to the stage, as everything else here reads it: a light
            // grazing along the plane is the *most* interesting one for a rim,
            // so unlike fill this does not fall to nothing at the horizon. Held
            // off zero so a low sun still rims.
            let facing = (towards.z.max(0.0) as f32).max(0.35);
            strength * facing
        }
    };

    let alpha = (rim * arriving).clamp(0.0, 1.0);
    if alpha <= 0.004 {
        return None;
    }

    Some(RimGlow {
        color: light.color.multiply_alpha(alpha),
        // From the same number, so one slider both switches the rim on and
        // makes it wide enough to see. A rim at a tenth is a hairline catching
        // the edge; at full it is a figure standing in front of the sun.
        reach: RIM_REACH * f64::from(rim),
    })
}

#[cfg(test)]
mod tests {

    fn gloom(throw: f64) -> crate::Light {
        crate::Light::new(
            crate::LightId(1),
            "Gloom",
            crate::LightKind::Gloom {
                edge: buzz_geom::Point::new(-200.0, 0.0),
                facing: 0.0,
                throw,
                width: 600.0,
            },
        )
    }

    /// **Darkest at the wall, gone by the end.**
    ///
    /// The last stop has to be white — a multiply that changes nothing — for
    /// the same reason a pool's last stop is forced to zero: past its last stop
    /// a gradient pads for ever, and a band that never ended would be a flat
    /// wash over the whole document.
    #[test]
    fn a_gloom_is_deepest_at_its_wall_and_gone_at_the_far_end() {
        let band = super::gloom_band(&gloom(400.0)).expect("a band");

        let luma = |c: peniko::Color| {
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            u32::from(r) + u32::from(g) + u32::from(b)
        };

        let first = band.ramp.first().expect("a first stop").1;
        let last = band.ramp.last().expect("a last stop").1;
        assert!(luma(first) < 60, "the wall is nearly black: {first:?}");
        assert_eq!(
            last.to_rgba8().to_u8_array(),
            [255, 255, 255, 255],
            "the far end must multiply by white, or the band never ends"
        );

        // Monotone all the way out: a fade that brightened anywhere in the
        // middle would read as a band of its own.
        for pair in band.ramp.windows(2) {
            assert!(
                luma(pair[0].1) <= luma(pair[1].1),
                "the dark must only ever thin out: {pair:?}"
            );
        }
    }

    /// The ramp has to start on the wall and finish where the dark runs out,
    /// or the gradient and the quad describe two different bands.
    #[test]
    fn the_ramp_runs_from_the_wall_to_the_end_of_the_throw() {
        let band = super::gloom_band(&gloom(400.0)).expect("a band");
        let placed = band.ramp_transform();

        let start = placed * buzz_geom::Point::new(-1.0, 0.0);
        let end = placed * buzz_geom::Point::new(1.0, 0.0);
        assert!((start - band.edge).hypot() < 1e-9, "{start:?}");
        assert!((end - band.far()).hypot() < 1e-9, "{end:?}");

        // And the quad it is painted through covers exactly that span.
        let bounds = {
            use buzz_geom::Shape as _;
            band.quad().bounding_box()
        };
        assert!((bounds.x0 - band.edge.x).abs() < 1e-9);
        assert!((bounds.x1 - band.far().x).abs() < 1e-9);
        assert!((bounds.height() - band.width).abs() < 1e-9);
    }

    /// A gloom turned right down stops nothing, and a quad that darkens by
    /// nothing is a full-frame layer bought for no picture at all.
    #[test]
    fn a_gloom_turned_down_draws_nothing() {
        let mut dark = gloom(400.0);
        dark.intensity = 0.0;
        assert!(super::gloom_band(&dark).is_none());

        let mut off = gloom(400.0);
        off.enabled = false;
        assert!(super::gloom_band(&off).is_none());

        // And nothing else in the rig lays one.
        let sun = crate::Light::new(crate::LightId(2), "Sun", crate::LightKind::sun());
        assert!(super::gloom_band(&sun).is_none());
    }

    /// **A gloom falls on a shape only where the gloom is**, which is the whole
    /// of what makes one aimable: outside its quad it does nothing, so a wall
    /// stood off the stage darkens the near figures and leaves the far ones.
    #[test]
    fn a_gloom_reaches_a_shape_only_inside_its_own_band() {
        use buzz_geom::Point;

        let dark = gloom(400.0);
        // Deepest at the wall, thinner further along, nothing past the end.
        let near = super::gloom_at(&dark, Point::new(-190.0, 0.0)).expect("at the wall");
        let far = super::gloom_at(&dark, Point::new(60.0, 0.0)).expect("down the throw");
        assert!(near.1 > far.1, "{near:?} against {far:?}");
        assert!(
            super::gloom_at(&dark, Point::new(400.0, 0.0)).is_none(),
            "past the end of the throw a gloom must reach nothing"
        );
        assert!(
            super::gloom_at(&dark, Point::new(-300.0, 0.0)).is_none(),
            "behind the wall is outside it too"
        );
        assert!(
            super::gloom_at(&dark, Point::new(0.0, 900.0)).is_none(),
            "and so is off the side"
        );

        // The direction is the way the darkness travels, so the dark edge lands
        // on the side the wall is on.
        assert!(near.0.x > 0.9, "it throws to the right: {:?}", near.0);

        let mut off = gloom(400.0);
        off.enabled = false;
        assert!(super::gloom_at(&off, Point::new(-190.0, 0.0)).is_none());

        let sun = crate::Light::new(crate::LightId(2), "Sun", crate::LightKind::sun());
        assert!(super::gloom_at(&sun, Point::ZERO).is_none());
    }

    /// **A lamp's shadow stays attached to the thing casting it.**
    ///
    /// The report: the shadows become huge and go a long way from the artwork.
    /// A lamp's shadow is a scale *about the lamp*, so the factor does two
    /// things at once — it enlarges the shadow and it throws it away from the
    /// caster in proportion to how far the caster already is. The factor is
    /// `lamp_height / (lamp_height − standing_height)`, which **diverges** as
    /// the lamp is lowered towards the height the artwork stands at, and the
    /// old bound let it reach twelve: a figure four hundred units from the lamp
    /// got a shadow twelve times its size, four thousand units away.
    #[test]
    fn a_lamps_shadow_never_runs_away_from_its_caster() {
        use buzz_geom::{Point, Rect, Shape as _};

        let caster = Rect::new(400.0, 200.0, 460.0, 320.0);
        let path = caster.to_path(1e-9);

        // The worst case an animator can reach with the sliders: a lamp barely
        // above the height the artwork is assumed to stand at.
        for (lamp_height, standing) in [(160.0, 70.0), (100.0, 90.0), (400.0, 390.0)] {
            let mut light = crate::Light::new(
                crate::LightId(1),
                "Lamp",
                crate::LightKind::Lamp {
                    position: Point::new(60.0, 200.0),
                    height: lamp_height,
                    radius: 300.0,
                },
            );
            light.standing_height = standing;

            let Some(shadow) = super::cast_shadow(&path, &light, caster.center(), standing) else {
                continue;
            };
            let thrown = shadow.bounding_box();

            assert!(
                thrown.width() <= caster.width() * 2.5,
                "lamp at {lamp_height} over artwork standing at {standing}: the shadow \
                 came out {:.0} wide against a caster {:.0} wide",
                thrown.width(),
                caster.width()
            );
            let travelled = (thrown.center() - caster.center()).hypot();
            assert!(
                travelled <= caster.width() * 8.0,
                "lamp at {lamp_height} over artwork standing at {standing}: the shadow \
                 landed {travelled:.0} units from its caster, which is {:.1} times the \
                 caster's own width",
                travelled / caster.width()
            );
        }
    }

    /// **The light is seen, the shadow is cast, and both follow the light.**
    ///
    /// A sun from one side must shade the far side of a shape and throw its
    /// shadow away from itself; move the sun to the opposite side and both
    /// must swap. That is what "directional" means, and it is the whole
    /// difference between lighting and a tint over everything.
    #[test]
    fn shading_and_shadow_follow_the_light() {
        use buzz_geom::{Rect, Shape as _};

        let path = Rect::new(100.0, 100.0, 180.0, 180.0).to_path(1e-9);
        let at = buzz_geom::Point::new(140.0, 140.0);

        let sun = |azimuth: f64| {
            let mut light = Light::new(
                LightId(1),
                "Sun",
                LightKind::Sun {
                    azimuth,
                    elevation: 0.5,
                },
            );
            light.shadows = true;
            light
        };

        // Azimuth zero: the light lies along +x, so the shading falls on the
        // side away from it and the shadow is thrown the other way.
        let east = shade_for(&path, &sun(0.0), at, 0.0, 60.0, 1.0);
        assert!(east.shade.is_some(), "a lit shape should be shaded");
        assert!(east.highlight.is_some(), "and catch a highlight");
        let east_cast = east.cast.clone().expect("and throw a shadow");

        let west = shade_for(&path, &sun(std::f64::consts::PI), at, 0.0, 60.0, 1.0);
        let west_cast = west.cast.clone().expect("a shadow from the other side too");

        // The two shadows must lie on opposite sides of the artwork. Compared
        // by where their weight is, which is what the eye reads.
        let middle = |p: &buzz_geom::BezPath| p.bounding_box().center().x;
        let art = path.bounding_box().center().x;
        assert!(
            (middle(&east_cast) - art).signum() != (middle(&west_cast) - art).signum(),
            "the shadow should swap sides with the light: {} then {}",
            middle(&east_cast),
            middle(&west_cast)
        );

        // The shading crescents swap with it.
        let shade_side = |g: &ShadeGeometry| {
            g.shade
                .as_ref()
                .map(|p| p.bounding_box().center().x - art)
                .unwrap_or(0.0)
        };
        assert!(
            shade_side(&east).signum() != shade_side(&west).signum(),
            "the shaded side should swap with the light too"
        );
    }

    /// A lamp is not a sun: its shadows **radiate**, so two shapes either side
    /// of it are thrown in opposite directions. That is how a lamp reads as a
    /// lamp in a finished shot.
    #[test]
    fn a_lamp_throws_its_shadows_outwards() {
        use buzz_geom::{Point, Rect, Shape as _};

        let mut lamp = Light::new(
            LightId(1),
            "Lamp",
            LightKind::Lamp {
                position: Point::new(400.0, 300.0),
                height: 200.0,
                radius: 1200.0,
            },
        );
        lamp.shadows = true;

        let thrown = |x: f64| {
            let path = Rect::new(x, 280.0, x + 40.0, 320.0).to_path(1e-9);
            let at = Point::new(x + 20.0, 300.0);
            let cast = shade_for(&path, &lamp, at, 0.0, 60.0, 1.0)
                .cast
                .expect("a shadow");
            cast.bounding_box().center().x - (x + 20.0)
        };

        // One shape to the left of the lamp, one to the right.
        let left = thrown(200.0);
        let right = thrown(600.0);
        assert!(
            left.signum() != right.signum(),
            "a lamp's shadows should point away from it on both sides,              got {left} and {right}"
        );
    }

    use super::*;
    use crate::{LightId, LightKind};
    use peniko::Color;

    fn square() -> BezPath {
        Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9)
    }

    fn sun(azimuth: f64, elevation: f64) -> Light {
        let mut light = Light::new(LightId(1), "Sun", LightKind::Sun { azimuth, elevation });
        light.color = Color::WHITE;
        light
    }

    /// The shade lands on the side away from the light. Get this backwards and
    /// every shot is lit from the wrong side — obvious in a picture, invisible
    /// in a number.
    #[test]
    fn the_shade_is_on_the_far_side_from_the_light() {
        // Light towards +x, so the lit side is the right and the shade is left.
        let shade = shade_crescent(&square(), Vec2::new(1.0, 0.0), 0.3).expect("a crescent");
        let bounds = shade.bounding_box();

        assert!(
            bounds.x0 < 1.0,
            "the crescent should start at the left edge"
        );
        assert!(
            bounds.x1 < 50.0,
            "and stay on the left half, got {bounds:?}"
        );
    }

    #[test]
    fn the_highlight_is_on_the_near_side() {
        let highlight =
            highlight_crescent(&square(), Vec2::new(1.0, 0.0), 0.3).expect("a crescent");
        let bounds = highlight.bounding_box();

        assert!(
            bounds.x1 > 99.0,
            "the highlight should reach the right edge"
        );
        assert!(
            bounds.x0 > 50.0,
            "and stay on the right half, got {bounds:?}"
        );
    }

    /// Swing the light and the crescents swing with it — the property the
    /// whole feature exists for.
    #[test]
    fn the_crescents_follow_the_light_round() {
        let cases = [
            (Vec2::new(1.0, 0.0), "shade left"),
            (Vec2::new(-1.0, 0.0), "shade right"),
            (Vec2::new(0.0, 1.0), "shade top"),
            (Vec2::new(0.0, -1.0), "shade bottom"),
        ];

        for (towards, what) in cases {
            let shade = shade_crescent(&square(), towards, 0.3).expect(what);
            let centre = shade.bounding_box().center();
            // The crescent's centre should sit opposite the light.
            assert!(
                (centre.x - 50.0) * towards.x + (centre.y - 50.0) * towards.y < 0.0,
                "{what}: crescent at {centre:?} is not away from {towards:?}"
            );
        }
    }

    #[test]
    fn a_softer_light_makes_a_wider_terminator() {
        // Above the floor, where softness still widens the band. Below it a
        // softer light sharpens the terminator instead — see
        // `a_hard_light_shades_the_form_and_does_not_merely_stop_shading_it`.
        let hard = shade_crescent(&square(), Vec2::new(1.0, 0.0), 0.3).expect("hard");
        let soft = shade_crescent(&square(), Vec2::new(1.0, 0.0), 0.9).expect("soft");

        assert!(
            soft.bounding_box().width() > hard.bounding_box().width() * 2.5,
            "soft {:?} should be much wider than hard {:?}",
            soft.bounding_box(),
            hard.bounding_box()
        );
    }

    /// **The report: the softness slider did not work.**
    ///
    /// Softness used to *be* the band's width, so turning it down turned the
    /// shading down with it — at the bottom of the slider a figure had a
    /// two-per-cent rim of dark along one edge and nothing else. That is not a
    /// hard light, it is no modelling at all.
    ///
    /// A hard light shades exactly as much and gets there in one step. So the
    /// width has a floor, and below it softness stops changing how much of the
    /// form is dark and changes only how sharply the dark begins.
    #[test]
    fn a_hard_light_shades_the_form_and_does_not_merely_stop_shading_it() {
        let form = square().bounding_box();
        let extent = form.width().min(form.height());

        for softness in [0.0, 0.05, 0.2] {
            let shade = shade_crescent(&square(), Vec2::new(1.0, 0.0), softness)
                .unwrap_or_else(|| panic!("no shade at all at a softness of {softness}"));
            let width = shade.bounding_box().width();
            assert!(
                width >= extent * HARD_SHADE_WIDTH - 1.0,
                "a softness of {softness} left {width:.0} of a {extent:.0} form in                  shade, which is a rim rather than a shaded side"
            );
        }
    }

    /// And what softness *does* change down there: how sharply the shade
    /// arrives. Zero is one step — an exact boundary, which is what a hard
    /// light has.
    #[test]
    fn a_hard_light_has_no_ramp_in_its_terminator() {
        assert_eq!(shade_feather(0.0), 0.0, "a hard light must not feather");
        assert!(shade_feather(0.5) > 0.0);
        assert!(shade_feather(1.0) > shade_feather(0.5));
        // And the width stops changing below the floor, so the two settings
        // are genuinely independent down there.
        assert_eq!(shade_width(0.0), shade_width(0.2));
        assert!(shade_width(0.9) > shade_width(0.3));
    }

    /// The crescent is the artwork's own outline, not a rectangle: that is
    /// what makes it read as form.
    #[test]
    fn the_crescent_follows_the_artworks_outline() {
        let circle = kurbo::Circle::new(Point::new(50.0, 50.0), 50.0).to_path(0.01);
        let shade = shade_crescent(&circle, Vec2::new(1.0, 0.0), 0.3).expect("a crescent");

        // A rectangle would fill its bounding box; a crescent covers far less.
        let bounds = shade.bounding_box();
        let box_area = bounds.width() * bounds.height();
        let area = shade.area().abs();
        assert!(
            area < box_area * 0.75,
            "the crescent fills {area} of a {box_area} box, which is suspiciously rectangular"
        );
    }

    /// A sun's shadow runs away from it, and lengthens as the sun drops.
    #[test]
    fn a_sun_casts_away_from_itself_and_lengthens_as_it_sets() {
        let high = cast_shadow(&square(), &sun(0.0, 1.2), Point::ZERO, 50.0).expect("high");
        let low = cast_shadow(&square(), &sun(0.0, 0.3), Point::ZERO, 50.0).expect("low");

        // Sun towards +x, so shadows fall towards -x.
        assert!(high.bounding_box().x0 < 0.0, "{:?}", high.bounding_box());
        assert!(
            low.bounding_box().x0 < high.bounding_box().x0,
            "a lower sun should throw the shadow further: {:?} vs {:?}",
            low.bounding_box(),
            high.bounding_box()
        );
    }

    /// **A shadow on the ground starts at the feet.**
    ///
    /// The whole difference between a shadow and a second copy of the drawing:
    /// wherever the light is, the contact line does not move.
    #[test]
    fn a_ground_shadow_is_anchored_at_the_casters_feet() {
        let caster = Rect::new(100.0, 100.0, 200.0, 300.0);
        for elevation in [0.3, 0.7, 1.2] {
            let throw = shadow_throw(&sun(0.6, elevation), 50.0).expect("a throw");
            let feet = Point::new(150.0, caster.y1);
            let landed = throw.at(caster) * feet;
            assert!(
                (landed - feet).hypot() < 1e-9,
                "the feet moved to {landed:?} at elevation {elevation}"
            );
        }
    }

    /// A light overhead puts a puddle underfoot; a low one throws a long
    /// shadow. The complaint this comes from is a shadow that did neither —
    /// it was the caster's own silhouette, full size, hung in the air behind
    /// it, because it was being projected onto the *wall* rather than the
    /// floor.
    #[test]
    fn a_higher_light_gives_a_shorter_ground_shadow() {
        let caster = Rect::new(100.0, 100.0, 200.0, 300.0);
        let reach = |elevation: f64| {
            let throw = shadow_throw(&sun(0.0, elevation), 50.0).expect("a throw");
            let head = Point::new(150.0, caster.y0);
            (throw.at(caster) * head - Point::new(150.0, caster.y1)).hypot()
        };

        let overhead = reach(1.5);
        let middling = reach(0.8);
        let low = reach(0.3);
        assert!(
            overhead < middling && middling < low,
            "a lower light must throw further: {overhead:.0}, {middling:.0}, {low:.0}"
        );
        assert!(
            overhead < caster.height() * 0.5,
            "a light nearly overhead should leave a puddle, not a shadow \
             {overhead:.0} long"
        );
    }

    /// It lies away from the light, as a shadow does.
    #[test]
    fn a_ground_shadow_lies_away_from_the_light() {
        let caster = Rect::new(100.0, 100.0, 200.0, 300.0);
        let head = Point::new(150.0, caster.y0);

        // A sun towards +x throws along -x.
        let east = shadow_throw(&sun(0.0, 0.5), 50.0)
            .expect("a throw")
            .at(caster)
            * head;
        assert!(east.x < 100.0, "a sun at 0 casts along -x: {east:?}");

        // And a lamp throws outwards from wherever it stands.
        let mut lamp = Light::new(
            LightId(3),
            "Lamp",
            LightKind::Lamp {
                position: Point::new(0.0, 0.0),
                height: 200.0,
                radius: 600.0,
            },
        );
        lamp.shadows = true;
        let thrown = shadow_throw(&lamp, 50.0).expect("a throw").at(caster) * head;
        assert!(
            thrown.x > 150.0 && thrown.y > caster.y1,
            "a lamp up and to the left should throw down and right: {thrown:?}"
        );
    }

    /// **Walking away from a lamp lengthens the shadow**, because the lamp is
    /// lower in the sky the further off you stand. A sun cannot do this and a
    /// wall shadow does the opposite — it grows because the *projection*
    /// enlarges, not because the light is low.
    #[test]
    fn a_caster_further_from_a_lamp_throws_a_longer_ground_shadow() {
        let mut lamp = Light::new(
            LightId(4),
            "Lamp",
            LightKind::Lamp {
                position: Point::new(0.0, 0.0),
                height: 200.0,
                radius: 900.0,
            },
        );
        lamp.shadows = true;
        let throw = shadow_throw(&lamp, 50.0).expect("a throw");

        let length = |x: f64| {
            let caster = Rect::new(x, 100.0, x + 100.0, 300.0);
            let head = Point::new(x + 50.0, caster.y0);
            (throw.at(caster) * head - Point::new(x + 50.0, caster.y1)).hypot()
        };

        assert!(
            length(600.0) > length(200.0),
            "further from the lamp should mean a longer shadow: {:.0} against {:.0}",
            length(600.0),
            length(200.0)
        );
    }

    /// The wall projection is still there, and still what it was: the caster's
    /// own silhouette, moved.
    #[test]
    fn a_wall_shadow_still_translates_the_whole_caster() {
        let mut light = sun(0.0, 0.5);
        light.fall = crate::ShadowFall::Wall;
        let throw = shadow_throw(&light, 60.0).expect("a throw");
        assert!(matches!(throw, ShadowThrow::Wall(_)));

        let caster = Rect::new(100.0, 100.0, 200.0, 300.0);
        let moved = throw.at(caster).transform_rect_bbox(caster);
        assert!(
            (moved.width() - caster.width()).abs() < 1e-6
                && (moved.height() - caster.height()).abs() < 1e-6,
            "a sun's wall shadow is a translation: {moved:?}"
        );
        assert!(moved.x0 < caster.x0, "and it moves away from the light");
    }

    #[test]
    fn a_taller_caster_throws_a_longer_shadow() {
        let light = sun(0.0, 0.8);
        let short = cast_shadow(&square(), &light, Point::ZERO, 20.0).expect("short");
        let tall = cast_shadow(&square(), &light, Point::ZERO, 200.0).expect("tall");

        assert!(
            tall.bounding_box().x0 < short.bounding_box().x0,
            "height should lengthen the shadow"
        );
    }

    /// Turn the sun and the shadow swings round the compass.
    #[test]
    fn the_shadow_swings_with_the_sun() {
        let height = 60.0;
        let east = cast_shadow(&square(), &sun(0.0, 0.6), Point::ZERO, height).expect("east");
        let south = cast_shadow(
            &square(),
            &sun(std::f64::consts::FRAC_PI_2, 0.6),
            Point::ZERO,
            height,
        )
        .expect("south");

        assert!(east.bounding_box().x0 < -10.0, "a sun at 0 casts along -x");
        assert!(
            south.bounding_box().y0 < -10.0,
            "a sun at a quarter turn casts along -y"
        );
    }

    /// A light on the horizon would cast an infinite shadow. Bounded, not
    /// infinite — and never absent, because artwork lit from the side still
    /// needs its shadow.
    #[test]
    fn a_shadow_is_bounded_however_low_the_sun_gets() {
        let shadow = cast_shadow(&square(), &sun(0.0, 0.03), Point::ZERO, 40.0);
        // Casting nothing is allowed — a sun this low may be below the horizon
        // as far as the projection is concerned. Casting something *unbounded*
        // is not.
        if let Some(path) = shadow {
            let reach = path.bounding_box().x0.abs();
            assert!(
                reach <= 40.0 * MAX_SHADOW_RATIO + 100.0,
                "the shadow ran to {reach}, which is unbounded in practice"
            );
        }
    }

    #[test]
    fn a_sun_below_the_horizon_casts_nothing() {
        assert!(cast_shadow(&square(), &sun(0.0, 0.0), Point::ZERO, 50.0).is_none());
    }

    #[test]
    fn artwork_lying_on_the_surface_casts_nothing() {
        assert!(cast_shadow(&square(), &sun(0.0, 0.9), Point::ZERO, 0.0).is_none());
    }

    #[test]
    fn a_light_with_shadows_switched_off_casts_nothing() {
        let mut light = sun(0.0, 0.9);
        light.shadows = false;
        assert!(cast_shadow(&square(), &light, Point::ZERO, 50.0).is_none());
    }

    /// A lamp projects rather than translates: its shadow is bigger than the
    /// caster, and grows the closer the lamp gets.
    #[test]
    fn a_lamp_projects_a_shadow_larger_than_its_caster() {
        let lamp = Light::new(
            LightId(2),
            "Lamp",
            LightKind::Lamp {
                position: Point::new(50.0, 50.0),
                height: 200.0,
                radius: 400.0,
            },
        );

        let shadow = cast_shadow(&square(), &lamp, Point::new(50.0, 50.0), 50.0).expect("a shadow");
        let bounds = shadow.bounding_box();

        assert!(
            bounds.width() > 100.0,
            "a point light should enlarge the shadow, got {bounds:?}"
        );
        // 200 / (200 - 50) = 1.33x about the lamp.
        assert!((bounds.width() - 133.3).abs() < 2.0, "{bounds:?}");
    }

    #[test]
    fn a_closer_lamp_throws_a_bigger_shadow() {
        let make = |height: f64| {
            Light::new(
                LightId(2),
                "Lamp",
                LightKind::Lamp {
                    position: Point::new(50.0, 50.0),
                    height,
                    radius: 400.0,
                },
            )
        };
        let far = cast_shadow(&square(), &make(600.0), Point::new(50.0, 50.0), 50.0).expect("far");
        let near =
            cast_shadow(&square(), &make(120.0), Point::new(50.0, 50.0), 50.0).expect("near");

        assert!(
            near.bounding_box().width() > far.bounding_box().width() * 1.5,
            "near {:?}, far {:?}",
            near.bounding_box(),
            far.bounding_box()
        );
    }

    /// A lamp level with the artwork it is lighting would throw its shadow to
    /// infinity. Nothing is better than nonsense.
    #[test]
    fn a_lamp_level_with_the_artwork_casts_nothing() {
        let lamp = Light::new(
            LightId(2),
            "Lamp",
            LightKind::Lamp {
                position: Point::new(50.0, 50.0),
                height: 50.0,
                radius: 400.0,
            },
        );
        assert!(cast_shadow(&square(), &lamp, Point::new(50.0, 50.0), 50.0).is_none());
    }

    #[test]
    fn a_sky_casts_no_shadow_and_shades_nothing() {
        let sky = Light::new(LightId(3), "Sky", LightKind::sky());
        let geometry = shade_for(&square(), &sky, Point::ZERO, 0.0, 50.0, 1.0);
        assert!(
            geometry.is_empty(),
            "ambient light has no direction to shade from"
        );
    }

    /// **A sun overhead lights from overhead**, and the shade goes underneath.
    ///
    /// This used to assert the opposite — that a sun at ninety degrees produced
    /// no crescents at all, "which is why noon looks flat". That was true of
    /// the arithmetic and false of the picture: artwork here stands *upright*,
    /// so a light above it is not a light behind it. What actually happened was
    /// that the shading direction was read off the light's compass bearing,
    /// which shrinks to nothing as the sun rises and hits exactly zero at the
    /// top — so the modelling switched itself off at the one setting an
    /// animator reaches for when they want strong top light.
    ///
    /// See [`Light::screen_towards`].
    #[test]
    fn a_sun_overhead_shades_from_underneath() {
        let overhead = sun(0.0, std::f64::consts::FRAC_PI_2);
        let geometry = shade_for(&square(), &overhead, Point::ZERO, 0.0, 50.0, 1.0);

        let shade = geometry.shade.expect("a terminator under an overhead sun");
        let highlight = geometry.highlight.expect("and a lit top");
        assert!(geometry.cast.is_some(), "and it still casts, straight down");

        // `square()` is the artwork; the shade must sit below its middle and
        // the glint above it, because the light is above.
        let form = square().bounding_box();
        assert!(
            shade.bounding_box().center().y > form.center().y,
            "the shade is not underneath: {:?} in a form of {form:?}",
            shade.bounding_box()
        );
        assert!(
            highlight.bounding_box().center().y < form.center().y,
            "the glint is not on top: {:?}",
            highlight.bounding_box()
        );
    }

    /// **Raising the sun turns the light on the figure**, rather than only
    /// changing the length of its shadow.
    #[test]
    fn raising_the_sun_lifts_the_light_up_the_picture() {
        let low = crescent_direction(&sun(0.0, 0.15), Point::ZERO, 0.0, 1.0).expect("low");
        let high = crescent_direction(&sun(0.0, 1.4), Point::ZERO, 0.0, 1.0).expect("high");

        // Screen y grows downwards, so "up the picture" is more negative.
        let tilt = |v: buzz_geom::Vec2| -v.y / v.hypot();
        assert!(
            tilt(low) < 0.2,
            "a sun on the horizon should light from the side, not from above: {low:?}"
        );
        assert!(
            tilt(high) > 0.9,
            "a sun overhead should light from above: {high:?}"
        );
    }

    #[test]
    fn modelling_turned_off_leaves_only_the_cast_shadow() {
        let geometry = shade_for(&square(), &sun(0.0, 0.7), Point::ZERO, 0.0, 50.0, 0.0);
        assert!(geometry.shade.is_none());
        assert!(geometry.highlight.is_none());
        assert!(geometry.cast.is_some());
    }

    #[test]
    fn degenerate_artwork_produces_nothing_rather_than_panicking() {
        let empty = BezPath::new();
        assert!(shade_crescent(&empty, Vec2::new(1.0, 0.0), 0.3).is_none());
        assert!(highlight_crescent(&empty, Vec2::new(1.0, 0.0), 0.3).is_none());

        let hairline = Rect::new(10.0, 10.0, 10.0, 200.0).to_path(1e-9);
        let geometry = shade_for(&hairline, &sun(0.0, 0.7), Point::ZERO, 0.0, 20.0, 1.0);
        for path in [geometry.shade, geometry.highlight, geometry.cast]
            .into_iter()
            .flatten()
        {
            assert!(
                path.bounding_box().width().is_finite(),
                "degenerate input produced non-finite geometry"
            );
        }
    }

    // -- the rim ------------------------------------------------------------

    /// A light that has not been asked for a rim does not lay one, so every
    /// document that existed before this is untouched.
    #[test]
    fn no_rim_unless_it_is_asked_for() {
        let light = sun(0.0, 0.7);
        assert_eq!(light.rim, 0.0, "off by default");
        assert!(rim_glow(&light, Point::ZERO, 0.0).is_none());
    }

    /// **The edges come up as the light comes up.** The whole point of tying
    /// the rim to the light rather than to the artwork: turn the light down and
    /// the glow follows it without a second thing to animate.
    #[test]
    fn a_brighter_light_rims_more_brightly() {
        let mut dim = sun(0.0, 0.7);
        dim.rim = 0.6;
        dim.intensity = 0.3;
        let mut bright = dim.clone();
        bright.intensity = 1.3;

        let dim = rim_glow(&dim, Point::ZERO, 0.0).expect("a rim");
        let bright = rim_glow(&bright, Point::ZERO, 0.0).expect("a rim");

        assert!(
            bright.color.components[3] > dim.color.components[3],
            "the brighter light glows harder: {} against {}",
            bright.color.components[3],
            dim.color.components[3]
        );
        assert_eq!(
            bright.reach, dim.reach,
            "and it is the same width: a dimmer light is fainter, not narrower"
        );
    }

    /// A lamp falls off, so a figure across the stage from it loses its rim on
    /// the way. This is what makes a rim read as light rather than as an
    /// outline switched on.
    #[test]
    fn a_lamps_rim_falls_off_with_distance() {
        let mut lamp = Light::new(LightId(1), "Lamp", LightKind::lamp(Point::ZERO));
        lamp.rim = 0.8;

        let near = rim_glow(&lamp, Point::new(20.0, 0.0), 0.0).expect("a rim close in");
        let far = rim_glow(&lamp, Point::new(900.0, 0.0), 0.0);

        match far {
            Some(far) => assert!(
                far.color.components[3] < near.color.components[3],
                "further away is fainter: {} against {}",
                far.color.components[3],
                near.color.components[3]
            ),
            // Faded away entirely, which is the same statement more strongly.
            None => {}
        }
    }

    /// Neither a sky nor a gloom rims anything: one arrives from every
    /// direction at once and the other emits nothing at all.
    #[test]
    fn only_a_light_with_a_direction_rims() {
        for kind in [
            LightKind::sky(),
            LightKind::gloom(Point::new(-400.0, 0.0)),
        ] {
            let mut light = Light::new(LightId(9), "L", kind);
            light.rim = 1.0;
            light.enabled = true;
            assert!(
                rim_glow(&light, Point::ZERO, 0.0).is_none(),
                "{} must not rim",
                light.kind.label()
            );
        }
    }

    /// A rim is a look, not a property of light, so switching the light off
    /// takes it with it.
    #[test]
    fn a_light_that_is_off_rims_nothing() {
        let mut light = sun(0.0, 0.7);
        light.rim = 1.0;
        light.enabled = false;
        assert!(rim_glow(&light, Point::ZERO, 0.0).is_none());
    }

}

#[cfg(test)]
mod shadow_length_tests {
    use super::*;
    use crate::{Light, LightId, LightKind, ShadowFall};

    fn sun(elevation: f64) -> Light {
        let mut light = Light::new(LightId(1), "Sun", LightKind::Sun { azimuth: 0.0, elevation });
        light.shadows = true;
        light
    }

    /// A caster 100 tall, and how far its shadow reaches along the ground.
    fn ground_reach(light: &Light) -> f64 {
        let caster = Rect::new(0.0, 0.0, 20.0, 100.0);
        let throw = shadow_throw(light, 70.0).expect("a throw");
        let top = throw.at(caster) * Point::new(10.0, 0.0);
        (top - Point::new(10.0, 100.0)).hypot()
    }

    /// **The report: the shadow's length could not be controlled.**
    ///
    /// It could, but only by moving the light — and where the light is is also
    /// what decides the shading on every figure on the stage, so an animator who
    /// wants a shorter shadow and this light has nowhere to go. The multiplier
    /// is that place.
    #[test]
    fn the_length_setting_shortens_and_lengthens_the_throw() {
        let mut light = sun(0.6);
        let honest = ground_reach(&light);
        assert!(honest > 1.0, "the fixture throws nothing to measure");

        light.shadow_length = 0.5;
        let half = ground_reach(&light);
        assert!(
            (half / honest - 0.5).abs() < 0.02,
            "half length threw {half:.1} against {honest:.1}"
        );

        light.shadow_length = 2.0;
        let double = ground_reach(&light);
        assert!(
            (double / honest - 2.0).abs() < 0.05,
            "double length threw {double:.1} against {honest:.1}"
        );
    }

    /// Zero puts the shadow under its caster rather than throwing none: a
    /// silhouette underfoot is a real staging choice, and "no shadow" is what
    /// the Shadows switch is for.
    #[test]
    fn zero_length_lands_the_shadow_underfoot() {
        let mut light = sun(0.6);
        light.shadow_length = 0.0;
        assert!(ground_reach(&light) < 0.5);
    }

    /// **The bound is on the geometry, not on the animator.** The clamp exists
    /// so a sun on the horizon does not ask for a shadow kilometres long; it
    /// must not also stop a deliberate setting from doing anything, which is
    /// what applying it after the multiplier would do.
    #[test]
    fn the_setting_still_works_where_the_geometry_is_clamped() {
        // Low enough that `1 / tan` is past `MAX_SHADOW_RATIO` and clamped.
        let mut light = sun(0.05);
        let clamped = ground_reach(&light);
        light.shadow_length = 0.25;
        let asked = ground_reach(&light);
        assert!(
            (asked / clamped - 0.25).abs() < 0.02,
            "the clamp swallowed the setting: {asked:.1} against {clamped:.1}"
        );
    }

    /// And a wall shadow, which is a different projection entirely.
    #[test]
    fn a_wall_shadow_takes_the_same_setting() {
        let mut light = sun(0.6);
        light.fall = ShadowFall::Wall;
        let offset = |light: &Light| {
            let affine = shadow_transform(light, 70.0).expect("a throw");
            let c = affine.as_coeffs();
            c[4].hypot(c[5])
        };
        let honest = offset(&light);
        light.shadow_length = 0.5;
        assert!((offset(&light) / honest - 0.5).abs() < 0.02);
    }

    /// A file written before the setting existed means "as long as the geometry
    /// says", and the default has to be exactly that.
    #[test]
    fn the_default_changes_nothing() {
        assert_eq!(sun(0.6).shadow_length, 1.0);
    }
}
