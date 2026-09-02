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
pub fn crescent_offset(bounds: Rect, direction: Vec2, softness: f64) -> Vec2 {
    let extent = bounds.width().min(bounds.height()).max(1e-6);
    let reach = extent * softness.clamp(0.02, 0.9);
    let length = direction.hypot();
    if length <= f64::EPSILON {
        return Vec2::ZERO;
    }
    direction * (reach / length)
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
    let offset = crescent_offset(bounds, towards, softness);
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
/// **A third narrower than it was, and much more saturated with it.** At 0.45
/// the highlight was a broad band down one side of every shape: it lit the
/// figure, but what it read as was the artwork having been painted in two
/// tones, not as light catching an edge. The complaint it produced was that a
/// lamp changes the overall colour of a drawing and nothing else — which is
/// exactly right, because a wash and a broad band are both washes.
///
/// The pair matters. Narrowing alone only makes the wash smaller, so
/// `Illumination::highlight`'s mix went up with it: a *narrow* band at a lot of
/// the light's colour is what an edge catching the light looks like.
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
    crescent_offset(bounds, towards, softness).hypot()
}

/// [`shade_reach`], for the narrower highlight band.
pub fn highlight_reach(bounds: Rect, towards: Vec2, softness: f64) -> f64 {
    crescent_offset(bounds, -towards, softness * HIGHLIGHT_SHARE).hypot()
}

/// The lit crescent on the side towards the light.
pub fn highlight_crescent(path: &BezPath, towards: Vec2, softness: f64) -> Option<BezPath> {
    let bounds = path.bounding_box();
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return None;
    }
    let offset = crescent_offset(bounds, -towards, softness * HIGHLIGHT_SHARE);
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
            Some(
                Affine::translate(position.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-position.to_vec2()),
            )
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
    let (towards, _) = light.towards(at, depth)?;
    let planar = towards.planar();
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
        let hard = shade_crescent(&square(), Vec2::new(1.0, 0.0), 0.1).expect("hard");
        let soft = shade_crescent(&square(), Vec2::new(1.0, 0.0), 0.6).expect("soft");

        assert!(
            soft.bounding_box().width() > hard.bounding_box().width() * 3.0,
            "soft {:?} should be much wider than hard {:?}",
            soft.bounding_box(),
            hard.bounding_box()
        );
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

    /// A light straight in front produces fill and nothing else — a shape lit
    /// head-on has no terminator, which is why noon looks flat.
    #[test]
    fn a_light_directly_in_front_makes_no_crescents() {
        let overhead = sun(0.0, std::f64::consts::FRAC_PI_2);
        let geometry = shade_for(&square(), &overhead, Point::ZERO, 0.0, 50.0, 1.0);

        assert!(geometry.shade.is_none(), "no terminator when lit head-on");
        assert!(geometry.highlight.is_none());
        assert!(geometry.cast.is_some(), "but it still casts, straight down");
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
