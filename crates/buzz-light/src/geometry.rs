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
fn crescent_offset(bounds: Rect, direction: Vec2, softness: f64) -> Vec2 {
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

/// The lit crescent on the side towards the light.
pub fn highlight_crescent(path: &BezPath, towards: Vec2, softness: f64) -> Option<BezPath> {
    let bounds = path.bounding_box();
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return None;
    }
    // Narrower than the shade: a highlight is a glint, and one as wide as the
    // terminator reads as a second light rather than as sheen.
    let offset = crescent_offset(bounds, -towards, softness * 0.45);
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
    if !light.shadows || height <= 0.0 {
        return None;
    }

    match light.kind {
        LightKind::Sky { .. } => None,

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
            Some(Affine::translate(away) * path.clone())
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
            let scale = (lamp_height / gap).min(MAX_SHADOW_RATIO);
            let about = Affine::translate(position.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-position.to_vec2());
            let _ = at;
            Some(about * path.clone())
        }
    }
}

/// The longest a shadow may be, as a multiple of the caster's height.
///
/// Twelve is a very low sun — about five degrees. Past that the shadow is
/// longer than any stage and its far end is off-screen anyway, so the only
/// thing the extra length costs is rasterisation.
const MAX_SHADOW_RATIO: f64 = 12.0;

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
    let Some((towards, _)) = light.towards(at, depth) else {
        return ShadeGeometry::default();
    };

    let planar = towards.planar();
    // A light directly in front has no direction *in the plane*, so it
    // produces no crescents — only fill. Trying to build them from a
    // zero-length vector is where a stray NaN would come from.
    let modelled = modelling > 0.01 && planar.hypot() > 1e-6;

    ShadeGeometry {
        shade: modelled
            .then(|| shade_crescent(path, planar, light.softness))
            .flatten(),
        highlight: modelled
            .then(|| highlight_crescent(path, planar, light.softness))
            .flatten(),
        cast: cast_shadow(path, light, at, height),
    }
}

/// Boolean difference, with the tolerance derived from the shapes themselves.
fn difference(a: &BezPath, b: &BezPath) -> Option<BezPath> {
    let bounds = a.bounding_box();
    let options = buzz_geom::BooleanOptions::for_shape_size(bounds.width().hypot(bounds.height()));
    let result = buzz_geom::boolean(a, b, buzz_geom::BoolOp::Difference, options);
    (!result.elements().is_empty()).then_some(result)
}

#[cfg(test)]
mod tests {
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
}
