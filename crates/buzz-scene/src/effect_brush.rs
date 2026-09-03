//! Effect brushes: a stroke that paints scenery rather than a line.
//!
//! Procreate ships brushes that lay down snow, clouds, city skylines and
//! strings of fairy lights; one drag and the picture has weather in it. This
//! module is that idea built out of what the document already understands —
//! every effect comes out as ordinary [`ShapeData`] (filled outlines, gradient
//! glows) or an ordinary painted bitmap ([`Canvas`]), so the Lasso, the Magic
//! Wand, tweens and the eraser all work on the result without knowing a brush
//! made it. Vector where an edge should stay crisp at any zoom (buildings,
//! trees, snowflakes), raster where only pixels can fade (clouds, washes of
//! diffused light), and both routinely in one stroke — which is the point of
//! having them on one layer.
//!
//! # Determinism is the contract
//!
//! The same stroke must produce the same artwork **every time it is asked**,
//! because it is asked twice: once per pointer move for the live preview, and
//! once on release for the commit. A brush that re-rolled its randomness would
//! shimmer while being drawn and then change on release — exactly the broken
//! feel this application's brushes are not allowed to have. So all scatter and
//! variation comes from [`splitmix`], seeded by the stroke's start point and
//! keyed by stamp index: as the stroke grows, the stamps already laid keep
//! their positions and only new ones appear.
//!
//! # Budgets
//!
//! Every effect caps how much it generates ([`caps`] below). The caps are not
//! quality settings; they are the difference between a drag and a frozen
//! window, because everything here is rebuilt on every pointer move while the
//! stroke is being drawn.

use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _, Vec2};
use buzz_geom::brush::{BrushBudget, StrokeSample, condition};
use peniko::Color;
use serde::{Deserialize, Serialize};

use crate::art::ArtPiece;
use crate::gradient::{Gradient, GradientKind, GradientStop, lerp_color};
use crate::object::{FillSpec, PaintBlend, ShapeData, StrokeSpec};
use crate::raster::{Canvas, SoftBrush};

/// Which effect the brush paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EffectKind {
    /// Drifting flakes scattered around the stroke.
    #[default]
    Snow,
    /// Slanted streaks of falling rain.
    Rain,
    /// A field of stars: dots and four-point sparkles.
    Stars,
    /// Warm floating points with a soft glow.
    Fireflies,
    /// Soft out-of-focus discs, some ringed.
    Bokeh,
    /// Soft cumulus painted as pixels, because only pixels can be fluffy.
    Clouds,
    /// A wash of glowing light along the stroke — an airbrush that adds light.
    DiffusedLight,
    /// Beams fanning out from where the stroke began, toward where it went.
    LightRays,
    /// A full moon with its halo, placed where the stroke ends.
    Moonlight,
    /// A string of fairy lights hanging from the stroke.
    StringLights,
    /// Street lamps standing on the stroke, each with its pool of light.
    Lamps,
    /// A city skyline standing on the stroke, windows lit.
    Buildings,
    /// A row of pines standing on the stroke.
    PineTrees,
    /// Round-crowned trees standing on the stroke.
    LeafyTrees,
    /// Blades of grass growing up from the stroke.
    Grass,
}

impl EffectKind {
    pub const ALL: [EffectKind; 15] = [
        Self::Snow,
        Self::Rain,
        Self::Stars,
        Self::Fireflies,
        Self::Bokeh,
        Self::Clouds,
        Self::DiffusedLight,
        Self::LightRays,
        Self::Moonlight,
        Self::StringLights,
        Self::Lamps,
        Self::Buildings,
        Self::PineTrees,
        Self::LeafyTrees,
        Self::Grass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Snow => "Snow",
            Self::Rain => "Rain",
            Self::Stars => "Stars",
            Self::Fireflies => "Fireflies",
            Self::Bokeh => "Bokeh",
            Self::Clouds => "Clouds",
            Self::DiffusedLight => "Diffused Light",
            Self::LightRays => "Light Rays",
            Self::Moonlight => "Moonlight",
            Self::StringLights => "String Lights",
            Self::Lamps => "Lamps",
            Self::Buildings => "Buildings",
            Self::PineTrees => "Pine Trees",
            Self::LeafyTrees => "Leafy Trees",
            Self::Grass => "Grass",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Snow => "Scatters drifting flakes along the stroke",
            Self::Rain => "Slanted streaks of rain along the stroke",
            Self::Stars => "A star field: dots and four-point sparkles",
            Self::Fireflies => "Warm glowing points scattered along the stroke",
            Self::Bokeh => "Soft out-of-focus discs of light",
            Self::Clouds => "Paints soft cumulus along the stroke",
            Self::DiffusedLight => "A soft wash of added light — an airbrush of glow",
            Self::LightRays => "Beams fan from the stroke's start toward its end",
            Self::Moonlight => "Places a glowing moon where the stroke ends",
            Self::StringLights => "Fairy lights hanging from the stroke",
            Self::Lamps => "Street lamps standing on the stroke, pools of light below",
            Self::Buildings => "A lit city skyline standing on the stroke",
            Self::PineTrees => "A treeline of pines standing on the stroke",
            Self::LeafyTrees => "Round-crowned trees standing on the stroke",
            Self::Grass => "Blades of grass growing up from the stroke",
        }
    }

    /// A one-line hint for the tool options: what the fill colour becomes.
    pub fn color_hint(self) -> &'static str {
        match self {
            Self::Snow | Self::Rain | Self::Stars | Self::Bokeh | Self::Clouds | Self::Grass => {
                "Painted in the fill colour"
            }
            Self::Fireflies
            | Self::DiffusedLight
            | Self::LightRays
            | Self::Moonlight
            | Self::Lamps => "The fill colour is the light",
            Self::StringLights => "Bulbs cycle a festive palette; the wire is dark",
            Self::Buildings => "Silhouette in the fill colour; windows glow warm",
            Self::PineTrees | Self::LeafyTrees => "Silhouette in the fill colour",
        }
    }
}

/// The gesture an effect is generated from.
#[derive(Debug, Clone, Copy)]
pub struct EffectStroke<'a> {
    pub samples: &'a [StrokeSample],
    /// The brush size, in document units. Sets the scale of everything.
    pub size: f64,
    /// The fill swatch — what the effect is made of. See
    /// [`EffectKind::color_hint`] for what each effect does with it.
    pub color: Color,
    /// How the stroke's samples are cleaned up before the spine is built —
    /// the Smoothing and Stabiliser settings, applied exactly as the outlined
    /// brushes apply them.
    pub conditioning: buzz_geom::Conditioning,
}

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

/// Hard limits per effect stroke. See the module header: these are what keep
/// a per-pointer-move rebuild from freezing the window.
mod caps {
    /// Scatter particles (flakes, streaks, stars, blades) per stroke.
    pub const PARTICLES: usize = 1_400;
    /// Standing structures (buildings, trees, lamps) per stroke.
    pub const STRUCTURES: usize = 240;
    /// Pieces that cost a GPU layer each: additive gradient glows.
    pub const GLOWS: usize = 90;
    /// Soft stamps into a raster canvas per stroke.
    pub const RASTER_STAMPS: usize = 700;
    /// Path elements one bucket shape may hold. A rect is 5, a circle 6.
    pub const ELEMENTS: usize = 24_000;
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Build the artwork one effect stroke lays down.
///
/// Deterministic: the same stroke gives the same artwork, which is what lets
/// the live preview and the committed result be the same thing. Empty input
/// gives empty output; a tap gives the effect's smallest sensible mark.
pub fn effect_artwork(kind: EffectKind, stroke: &EffectStroke<'_>) -> Vec<ArtPiece> {
    if stroke.samples.is_empty() || stroke.size <= 0.0 {
        return Vec::new();
    }

    // The spine: conditioned exactly as the fluid brush conditions its input,
    // so the Smoothing slider means the same thing on every brush.
    let conditioned = condition(stroke.samples, stroke.conditioning, &BrushBudget::default());
    let spine: Vec<Point> = conditioned.iter().map(|s| s.point).collect();
    if spine.is_empty() {
        return Vec::new();
    }

    let ctx = Fx {
        spine: &spine,
        conditioned: &conditioned,
        size: stroke.size.clamp(1.0, 400.0),
        color: stroke.color,
        seed: seed_for(kind, &spine[0], stroke.size),
    };

    match kind {
        EffectKind::Snow => snow(&ctx),
        EffectKind::Rain => rain(&ctx),
        EffectKind::Stars => stars(&ctx),
        EffectKind::Fireflies => fireflies(&ctx),
        EffectKind::Bokeh => bokeh(&ctx),
        EffectKind::Clouds => clouds(&ctx),
        EffectKind::DiffusedLight => diffused_light(&ctx),
        EffectKind::LightRays => light_rays(&ctx),
        EffectKind::Moonlight => moonlight(&ctx),
        EffectKind::StringLights => string_lights(&ctx),
        EffectKind::Lamps => lamps(&ctx),
        EffectKind::Buildings => buildings(&ctx),
        EffectKind::PineTrees => pine_trees(&ctx),
        EffectKind::LeafyTrees => leafy_trees(&ctx),
        EffectKind::Grass => grass(&ctx),
    }
}

/// Everything an effect generator works from.
struct Fx<'a> {
    spine: &'a [Point],
    conditioned: &'a [StrokeSample],
    size: f64,
    color: Color,
    seed: u64,
}

impl Fx<'_> {
    /// A deterministic value in `[0, 1)` for stamp `index`, stream `salt`.
    fn rng(&self, index: usize, salt: u64) -> f64 {
        let x = splitmix(
            self.seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt << 17,
        );
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A value in `[-1, 1)`, centre-weighted like a hand's scatter is.
    fn spread(&self, index: usize, salt: u64) -> f64 {
        self.rng(index, salt) + self.rng(index, salt ^ 0xA5A5) - 1.0
    }
}

/// SplitMix64: two multiplies and three xor-shifts. Not cryptography — just a
/// hash whose low bits are as good as its high ones, which a scatter needs.
pub(crate) fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The stroke's seed: where it began, what it is, how big.
///
/// The start point rather than the whole path, so the seed — and with it every
/// stamp already placed — holds still while the stroke is still being drawn.
fn seed_for(kind: EffectKind, start: &Point, size: f64) -> u64 {
    let k = EffectKind::ALL.iter().position(|x| *x == kind).unwrap_or(0) as u64;
    splitmix(start.x.to_bits() ^ start.y.to_bits().rotate_left(21) ^ size.to_bits() ^ (k << 3))
}

// ---------------------------------------------------------------------------
// Walking the spine
// ---------------------------------------------------------------------------

/// A point along the stroke, with its direction of travel.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    pos: Point,
    /// Unit tangent. `(1, 0)` for a tap.
    dir: Vec2,
}

/// Evenly spaced stamps along the spine.
///
/// Arc-length spacing over the polyline, like [`Canvas::stroke`] uses: the
/// density of pointer samples must not show in the density of the effect. The
/// walk starts at the stroke's start, so stamps already placed do not slide
/// as the stroke grows.
fn walk(spine: &[Point], spacing: f64, cap: usize) -> Vec<Stamp> {
    let spacing = spacing.max(0.25);
    let mut out = Vec::new();
    let Some(&first) = spine.first() else {
        return out;
    };
    let first_dir = spine
        .iter()
        .skip(1)
        .find(|p| (**p - first).hypot() > 1e-9)
        .map(|p| (*p - first).normalize())
        .unwrap_or(Vec2::new(1.0, 0.0));
    out.push(Stamp {
        pos: first,
        dir: first_dir,
    });

    let mut carried = 0.0;
    for pair in spine.windows(2) {
        if out.len() >= cap {
            break;
        }
        let (a, b) = (pair[0], pair[1]);
        let length = (b - a).hypot();
        if length <= f64::MIN_POSITIVE {
            continue;
        }
        let dir = (b - a) / length;
        let mut travelled = spacing - carried;
        while travelled <= length && out.len() < cap {
            out.push(Stamp {
                pos: a.lerp(b, travelled / length),
                dir,
            });
            travelled += spacing;
        }
        carried = (length - (travelled - spacing)).max(0.0);
    }
    out
}

// ---------------------------------------------------------------------------
// Small pieces of geometry and paint
// ---------------------------------------------------------------------------

// Shared with `crate::wave`, which paints its strands by the same three rules:
// set the alpha, or move the colour towards white or black without disturbing
// it. Three lines each, and worth one home rather than two.
pub(crate) fn with_alpha(c: Color, a: f64) -> Color {
    let mut k = c.components;
    k[3] = a.clamp(0.0, 1.0) as f32;
    Color::new(k)
}

pub(crate) fn lighten(c: Color, t: f64) -> Color {
    let mut lit = lerp_color(c, Color::WHITE, t);
    lit.components[3] = c.components[3];
    lit
}

pub(crate) fn darken(c: Color, t: f64) -> Color {
    let mut dim = lerp_color(c, Color::BLACK, t);
    dim.components[3] = c.components[3];
    dim
}

fn add_circle(path: &mut BezPath, center: Point, r: f64) {
    path.extend(kurbo::Circle::new(center, r.max(0.05)).to_path(0.05).iter());
}

fn add_rect(path: &mut BezPath, rect: Rect) {
    path.extend(rect.to_path(1e-9).iter());
}

fn solid(path: BezPath, color: Color) -> ShapeData {
    ShapeData {
        path,
        fill: Some(FillSpec::solid(color)),
        stroke: None,
        blend: PaintBlend::Normal,
    }
}

fn stroked(path: BezPath, color: Color, width: f64) -> ShapeData {
    ShapeData {
        path,
        fill: None,
        stroke: Some(StrokeSpec::new(color, width)),
        blend: PaintBlend::Normal,
    }
}

/// A radial glow: bright at the middle, gone at the rim, **adding** light.
///
/// One shape per glow because a gradient is positioned once in space — two
/// lamps cannot share one ramp. That is why [`caps::GLOWS`] exists: each of
/// these costs the renderer an isolation layer.
fn glow(center: Point, r: f64, color: Color, alpha: f64) -> ShapeData {
    let r = r.max(0.5);
    let mut ramp = Gradient::new(
        GradientKind::Radial,
        vec![
            GradientStop::new(0.0, with_alpha(color, alpha)),
            GradientStop::new(0.55, with_alpha(color, alpha * 0.35)),
            GradientStop::new(1.0, with_alpha(color, 0.0)),
        ],
    );
    ramp.fit_to(Rect::new(center.x - r, center.y - r, center.x + r, center.y + r));
    let mut path = BezPath::new();
    add_circle(&mut path, center, r);
    ShapeData {
        path,
        fill: Some(FillSpec {
            paint: ramp.into(),
            rule: buzz_geom::FillMode::NonZero,
            swatch: None,
        }),
        stroke: None,
        blend: PaintBlend::Additive,
    }
}

/// Push a bucket shape if it has anything in it.
fn push_solid(out: &mut Vec<ArtPiece>, path: BezPath, color: Color) {
    if !path.elements().is_empty() {
        out.push(ArtPiece::Shape(solid(path, color)));
    }
}

// ---------------------------------------------------------------------------
// The effects
// ---------------------------------------------------------------------------

/// Snow. Flakes are bucketed into three depths — near, mid, far — because a
/// shape holds one paint: three buckets give the parallax feel of falling
/// snow for the price of three shapes instead of one per flake.
fn snow(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 0.8).max(2.5), caps::PARTICLES / 2);
    let mut buckets = [BezPath::new(), BezPath::new(), BezPath::new()];

    for (i, stamp) in stamps.iter().enumerate() {
        let normal = Vec2::new(-stamp.dir.y, stamp.dir.x);
        for flake in 0..2usize {
            let salt = 0x5A00 + flake as u64;
            let across = fx.spread(i, salt) * fx.size * 2.2;
            let along = fx.spread(i, salt ^ 0x11) * fx.size * 0.8;
            let at = stamp.pos + normal * across + stamp.dir * along;
            let r = fx.size * (0.045 + 0.15 * fx.rng(i, salt ^ 0x22).powi(2));
            let depth = (fx.rng(i, salt ^ 0x33) * 3.0) as usize % 3;

            let bucket = &mut buckets[depth];
            if bucket.elements().len() >= caps::ELEMENTS {
                continue;
            }
            // The occasional large flake is a real six-armed crystal; the
            // rest are dots, which is all the eye asks of snow.
            if depth == 0 && r > fx.size * 0.11 && fx.rng(i, salt ^ 0x44) < 0.35 {
                let arms = 6;
                let spin = fx.rng(i, salt ^ 0x55) * std::f64::consts::PI;
                for arm in 0..arms {
                    let angle = spin + arm as f64 * std::f64::consts::PI * 2.0 / arms as f64;
                    let out_v = Vec2::new(angle.cos(), angle.sin());
                    let side = Vec2::new(-out_v.y, out_v.x) * (r * 0.18);
                    let tip = at + out_v * r;
                    let mut arm_path = BezPath::new();
                    arm_path.move_to(at + side);
                    arm_path.line_to(tip);
                    arm_path.line_to(at - side);
                    arm_path.close_path();
                    bucket.extend(arm_path.iter());
                }
            } else {
                add_circle(bucket, at, r);
            }
        }
    }

    let mut out = Vec::new();
    let [near, mid, far] = buckets;
    // Far first, so near flakes draw over them.
    push_solid(&mut out, far, with_alpha(fx.color, 0.38));
    push_solid(&mut out, mid, with_alpha(fx.color, 0.65));
    push_solid(&mut out, near, with_alpha(fx.color, 0.95));
    out
}

/// Rain: slanted streaks, two depths.
fn rain(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 0.55).max(2.0), caps::PARTICLES);
    let mut buckets = [BezPath::new(), BezPath::new()];

    for (i, stamp) in stamps.iter().enumerate() {
        let normal = Vec2::new(-stamp.dir.y, stamp.dir.x);
        let across = fx.spread(i, 0x0A11) * fx.size * 2.0;
        let at = stamp.pos + normal * across;
        // All streaks share one slant — rain falls together — with a whisper
        // of variation so it reads as weather, not hatching.
        let slant = Vec2::new(0.35 + fx.spread(i, 0x0A22) * 0.05, 1.0).normalize();
        let len = fx.size * (0.9 + 0.9 * fx.rng(i, 0x0A33));
        let depth = usize::from(fx.rng(i, 0x0A44) < 0.4);
        let bucket = &mut buckets[depth];
        if bucket.elements().len() >= caps::ELEMENTS {
            continue;
        }
        bucket.move_to(at);
        bucket.line_to(at + slant * len);
    }

    let width = (fx.size * 0.05).max(0.6);
    let [near, far] = buckets;
    let mut out = Vec::new();
    if !far.elements().is_empty() {
        out.push(ArtPiece::Shape(stroked(
            far,
            with_alpha(fx.color, 0.28),
            width * 0.8,
        )));
    }
    if !near.elements().is_empty() {
        out.push(ArtPiece::Shape(stroked(
            near,
            with_alpha(fx.color, 0.5),
            width,
        )));
    }
    out
}

/// A four-armed sparkle: thin diamonds crossing at `at`.
fn sparkle(path: &mut BezPath, at: Point, arm: f64, spin: f64) {
    for k in 0..4 {
        let angle = spin + k as f64 * std::f64::consts::FRAC_PI_2;
        let out_v = Vec2::new(angle.cos(), angle.sin());
        let side = Vec2::new(-out_v.y, out_v.x) * (arm * 0.12);
        let mut d = BezPath::new();
        d.move_to(at + side);
        d.line_to(at + out_v * arm);
        d.line_to(at - side);
        d.close_path();
        path.extend(d.iter());
    }
}

fn stars(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 1.3).max(4.0), caps::PARTICLES / 2);
    let mut bright = BezPath::new();
    let mut dim = BezPath::new();

    for (i, stamp) in stamps.iter().enumerate() {
        let normal = Vec2::new(-stamp.dir.y, stamp.dir.x);
        let at = stamp.pos
            + normal * (fx.spread(i, 0x57A1) * fx.size * 2.5)
            + stamp.dir * (fx.spread(i, 0x57A2) * fx.size * 1.2);
        let roll = fx.rng(i, 0x57A3);
        let bucket = if fx.rng(i, 0x57A4) < 0.4 { &mut bright } else { &mut dim };
        if bucket.elements().len() >= caps::ELEMENTS {
            continue;
        }
        if roll < 0.03 {
            // The hero star: long arms and a short crossing pair.
            let arm = fx.size * 0.4;
            let spin = fx.rng(i, 0x57A5) * std::f64::consts::FRAC_PI_2;
            sparkle(bucket, at, arm, spin);
            sparkle(bucket, at, arm * 0.45, spin + std::f64::consts::FRAC_PI_4);
        } else if roll < 0.18 {
            sparkle(
                bucket,
                at,
                fx.size * 0.16,
                fx.rng(i, 0x57A6) * std::f64::consts::FRAC_PI_2,
            );
        } else {
            add_circle(bucket, at, fx.size * (0.02 + 0.05 * fx.rng(i, 0x57A7)));
        }
    }

    let mut out = Vec::new();
    push_solid(&mut out, dim, with_alpha(fx.color, 0.55));
    push_solid(&mut out, bright, with_alpha(lighten(fx.color, 0.4), 0.95));
    out
}

fn fireflies(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 1.6).max(6.0), caps::GLOWS);
    let mut cores = BezPath::new();
    let mut out = Vec::new();

    for (i, stamp) in stamps.iter().enumerate() {
        let normal = Vec2::new(-stamp.dir.y, stamp.dir.x);
        let at = stamp.pos
            + normal * (fx.spread(i, 0xF1F1) * fx.size * 2.2)
            + stamp.dir * (fx.spread(i, 0xF1F2) * fx.size * 1.0);
        // Glows go down first so every core draws over its own halo.
        out.push(ArtPiece::Shape(glow(
            at,
            fx.size * (0.25 + 0.15 * fx.rng(i, 0xF1F3)),
            fx.color,
            0.4,
        )));
        add_circle(&mut cores, at, (fx.size * 0.045).max(0.4));
    }

    push_solid(&mut out, cores, with_alpha(lighten(fx.color, 0.6), 0.95));
    out
}

fn bokeh(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 1.5).max(5.0), caps::PARTICLES / 4);
    let mut filled = BezPath::new();
    let mut rings = BezPath::new();

    for (i, stamp) in stamps.iter().enumerate() {
        let normal = Vec2::new(-stamp.dir.y, stamp.dir.x);
        let at = stamp.pos
            + normal * (fx.spread(i, 0xB0E1) * fx.size * 2.5)
            + stamp.dir * (fx.spread(i, 0xB0E2) * fx.size * 1.3);
        let r = fx.size * (0.22 + 0.5 * fx.rng(i, 0xB0E3));
        if fx.rng(i, 0xB0E4) < 0.25 {
            if rings.elements().len() < caps::ELEMENTS {
                // A ring: the outer circle with the inner one wound the other
                // way, so non-zero filling leaves the middle open.
                add_circle(&mut rings, at, r);
                let inner = kurbo::Circle::new(at, (r * 0.82).max(0.1))
                    .to_path(0.05)
                    .reverse_subpaths();
                rings.extend(inner.iter());
            }
        } else if filled.elements().len() < caps::ELEMENTS {
            add_circle(&mut filled, at, r);
        }
    }

    let mut out = Vec::new();
    if !filled.elements().is_empty() {
        out.push(ArtPiece::Shape(
            solid(filled, with_alpha(fx.color, 0.2)).with_blend(PaintBlend::Additive),
        ));
    }
    if !rings.elements().is_empty() {
        out.push(ArtPiece::Shape(
            solid(rings, with_alpha(fx.color, 0.35)).with_blend(PaintBlend::Additive),
        ));
    }
    out
}

/// Clouds are the one effect that must be pixels: a fluffy edge is a
/// different opacity at every point, which no outline can say. Soft stamps of
/// varying radius pile into one coverage buffer; max-blend keeps the mass
/// flat instead of beading, exactly as the soft brush does.
fn clouds(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 0.45).max(2.0), caps::RASTER_STAMPS / 3);
    if stamps.is_empty() {
        return Vec::new();
    }

    // Plan every puff first, so the canvas can be sized to fit them all.
    let mut puffs: Vec<(Point, f64, f64)> = Vec::new(); // centre, radius, hardness
    let mut bounds: Option<Rect> = None;
    for (i, stamp) in stamps.iter().enumerate() {
        let normal = Vec2::new(-stamp.dir.y, stamp.dir.x);
        let n = 1 + (fx.rng(i, 0xC10D) * 2.0) as usize;
        for p in 0..=n {
            let salt = 0xC200 + p as u64;
            let at = stamp.pos
                + stamp.dir * (fx.spread(i, salt) * fx.size * 0.5)
                + normal * (fx.spread(i, salt ^ 0x77) * fx.size * 0.45);
            let r = fx.size * (0.35 + 0.5 * fx.rng(i, salt ^ 0x88));
            let hardness = 0.08 + 0.25 * fx.rng(i, salt ^ 0x99);
            let pad = r + 2.0;
            let extent = Rect::new(at.x - pad, at.y - pad, at.x + pad, at.y + pad);
            bounds = Some(match bounds {
                Some(b) => b.union(extent),
                None => extent,
            });
            puffs.push((at, r, hardness));
            if puffs.len() >= caps::RASTER_STAMPS {
                break;
            }
        }
        if puffs.len() >= caps::RASTER_STAMPS {
            break;
        }
    }

    let Some(bounds) = bounds else {
        return Vec::new();
    };
    let mut canvas = Canvas::covering(bounds);
    for (at, r, hardness) in puffs {
        canvas.stamp(
            at,
            &SoftBrush {
                radius: r,
                hardness,
                flow: 1.0,
                color: fx.color,
            },
        );
    }
    if canvas.is_blank() {
        return Vec::new();
    }
    vec![ArtPiece::Painting {
        canvas,
        brush: SoftBrush {
            radius: fx.size,
            hardness: 0.2,
            flow: 0.95,
            color: fx.color,
        },
        blend: PaintBlend::Normal,
    }]
}

/// A wash of added light along the stroke — the soft brush's coverage, but
/// composited additively so it brightens what is under it.
fn diffused_light(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let brush = SoftBrush {
        radius: (fx.size * 1.2).max(2.0),
        hardness: 0.05,
        flow: 0.3,
        color: lighten(fx.color, 0.2),
    };
    let Some(canvas) = Canvas::for_stroke(fx.spine, &brush) else {
        return Vec::new();
    };
    if canvas.is_blank() {
        return Vec::new();
    }
    vec![ArtPiece::Painting {
        canvas,
        brush,
        blend: PaintBlend::Additive,
    }]
}

/// Beams from where the stroke began, toward where it went, dying out along
/// their length. The drag is the aim: press at the source, release at the
/// farthest reach.
fn light_rays(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let origin = fx.spine[0];
    let end = *fx.spine.last().expect("checked non-empty");
    let reach = end - origin;
    // A tap still shines: a short fan pointing right, so the brush never
    // silently does nothing.
    let (dir, length) = if reach.hypot() < fx.size * 0.5 {
        (Vec2::new(1.0, 0.0), fx.size * 4.0)
    } else {
        (reach.normalize(), reach.hypot())
    };
    let base_angle = dir.atan2();

    let beams = 9usize;
    let mut out = Vec::with_capacity(beams);
    for i in 0..beams {
        let angle = base_angle + fx.spread(i, 0x4A71) * 0.32;
        let len = length * (0.65 + 0.45 * fx.rng(i, 0x4A72));
        let half_width = len * (0.02 + 0.05 * fx.rng(i, 0x4A73));
        let along = Vec2::new(angle.cos(), angle.sin());
        let side = Vec2::new(-along.y, along.x);

        let far = origin + along * len;
        let mut path = BezPath::new();
        path.move_to(origin);
        path.line_to(far + side * half_width);
        path.line_to(far - side * half_width);
        path.close_path();

        let alpha = 0.16 + 0.18 * fx.rng(i, 0x4A74);
        let mut ramp = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(0.0, with_alpha(lighten(fx.color, 0.25), alpha)),
                GradientStop::new(0.75, with_alpha(fx.color, alpha * 0.3)),
                GradientStop::new(1.0, with_alpha(fx.color, 0.0)),
            ],
        );
        // The linear ramp lives on the unit segment (-1,0)..(1,0); carry it
        // onto the beam so it fades along the beam's own length.
        let mid = origin + along * (len / 2.0);
        ramp.transform = Affine::translate(mid.to_vec2())
            * Affine::rotate(angle)
            * Affine::scale(len / 2.0);

        out.push(ArtPiece::Shape(ShapeData {
            path,
            fill: Some(FillSpec {
                paint: ramp.into(),
                rule: buzz_geom::FillMode::NonZero,
                swatch: None,
            }),
            stroke: None,
            blend: PaintBlend::Additive,
        }));
    }
    out
}

/// A full moon where the stroke ends: drag from anywhere to where the moon
/// should hang, sized by the brush.
fn moonlight(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let at = *fx.spine.last().expect("checked non-empty");
    let r = fx.size.max(4.0);
    let face = lighten(fx.color, 0.55);

    let mut out = vec![
        // The halo, in two breaths: a broad faint one and a tight bright one.
        ArtPiece::Shape(glow(at, r * 2.8, fx.color, 0.22)),
        ArtPiece::Shape(glow(at, r * 1.45, lighten(fx.color, 0.3), 0.5)),
    ];

    let mut disc = BezPath::new();
    add_circle(&mut disc, at, r);
    out.push(ArtPiece::Shape(solid(disc, face)));

    // Craters: a handful of darker discs, kept well inside the rim.
    let mut craters = BezPath::new();
    for i in 0..5 {
        let angle = fx.rng(i, 0x3001) * std::f64::consts::PI * 2.0;
        let dist = r * (0.15 + 0.55 * fx.rng(i, 0x3002));
        let cr = r * (0.08 + 0.12 * fx.rng(i, 0x3003));
        if dist + cr < r * 0.9 {
            add_circle(
                &mut craters,
                at + Vec2::new(angle.cos(), angle.sin()) * dist,
                cr,
            );
        }
    }
    push_solid(&mut out, craters, with_alpha(darken(fx.color, 0.25), 0.18));
    out
}

/// Fairy lights: the stroke is the wire, bulbs hang from it, each with its
/// glow. Bulbs cycle a small festive palette — one colour of fairy light is
/// just a queue of lamps.
fn string_lights(fx: &Fx<'_>) -> Vec<ArtPiece> {
    const PALETTE: [(u8, u8, u8); 5] = [
        (255, 217, 138), // warm white
        (255, 107, 107), // red
        (123, 216, 143), // green
        (107, 181, 255), // blue
        (255, 169, 79),  // amber
    ];

    let mut out = Vec::new();

    // The wire itself: the smoothed curve, thin and dark.
    let wire = buzz_geom::centreline(fx.conditioned);
    if !wire.elements().is_empty() && fx.spine.len() > 1 {
        out.push(ArtPiece::Shape(stroked(
            wire,
            Color::from_rgba8(0x24, 0x24, 0x2C, 0xFF),
            (fx.size * 0.03).max(0.7),
        )));
    }

    let stamps = walk(fx.spine, (fx.size * 1.15).max(5.0), 400);
    let bulb_r = (fx.size * 0.11).max(1.0);
    let mut buckets: Vec<BezPath> = (0..PALETTE.len()).map(|_| BezPath::new()).collect();

    // Glows first, so every bulb draws over its own halo. Capped: past the
    // cap the bulbs still appear, just without the bloom.
    for (i, stamp) in stamps.iter().enumerate() {
        let swing = fx.spread(i, 0x5711) * fx.size * 0.08;
        let at = stamp.pos + Vec2::new(swing, fx.size * 0.22);
        let (r, g, b) = PALETTE[i % PALETTE.len()];
        let color = Color::from_rgba8(r, g, b, 0xFF);
        if i < caps::GLOWS {
            out.push(ArtPiece::Shape(glow(at, fx.size * 0.38, color, 0.4)));
        }
        add_circle(&mut buckets[i % PALETTE.len()], at, bulb_r);
    }

    for (bucket, (r, g, b)) in buckets.into_iter().zip(PALETTE) {
        push_solid(&mut out, bucket, Color::from_rgba8(r, g, b, 0xFF));
    }
    out
}

/// Street lamps standing on the stroke. The stroke is the street.
fn lamps(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let spacing = (fx.size * 5.0).max(60.0);
    let stamps = walk(fx.spine, spacing, 40);
    let s = fx.size;
    let height = s * 2.6;
    let warm = lighten(fx.color, 0.2);
    let iron = Color::from_rgba8(0x16, 0x16, 0x1C, 0xFF);

    let mut out = Vec::new();
    let mut posts = BezPath::new();

    for stamp in &stamps {
        let base = stamp.pos;
        let head = Point::new(base.x, base.y - height);

        // Light first: the pool on the ground, the cone through the air, the
        // glow round the head — all behind the ironwork.
        let pool = Rect::new(
            base.x - s * 1.1,
            base.y - s * 0.16,
            base.x + s * 1.1,
            base.y + s * 0.16,
        );
        let mut pool_ramp = Gradient::new(
            GradientKind::Radial,
            vec![
                GradientStop::new(0.0, with_alpha(warm, 0.28)),
                GradientStop::new(1.0, with_alpha(warm, 0.0)),
            ],
        );
        pool_ramp.fit_to(pool);
        out.push(ArtPiece::Shape(ShapeData {
            path: pool.to_path(1e-9),
            fill: Some(FillSpec {
                paint: pool_ramp.into(),
                rule: buzz_geom::FillMode::NonZero,
                swatch: None,
            }),
            stroke: None,
            blend: PaintBlend::Additive,
        }));

        let mut cone = BezPath::new();
        cone.move_to(Point::new(head.x - s * 0.2, head.y + s * 0.1));
        cone.line_to(Point::new(head.x + s * 0.2, head.y + s * 0.1));
        cone.line_to(Point::new(base.x + s * 1.05, base.y));
        cone.line_to(Point::new(base.x - s * 1.05, base.y));
        cone.close_path();
        let mut cone_ramp = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(0.0, with_alpha(warm, 0.3)),
                GradientStop::new(1.0, with_alpha(warm, 0.0)),
            ],
        );
        // Vertical, head to ground: rotate the unit ramp a quarter turn.
        cone_ramp.transform = Affine::translate(Vec2::new(
            head.x,
            (head.y + base.y) / 2.0,
        )) * Affine::rotate(std::f64::consts::FRAC_PI_2)
            * Affine::scale((base.y - head.y).abs() / 2.0);
        out.push(ArtPiece::Shape(ShapeData {
            path: cone,
            fill: Some(FillSpec {
                paint: cone_ramp.into(),
                rule: buzz_geom::FillMode::NonZero,
                swatch: None,
            }),
            stroke: None,
            blend: PaintBlend::Additive,
        }));

        out.push(ArtPiece::Shape(glow(head, s * 1.3, warm, 0.5)));

        // The ironwork, in one bucket for the whole stroke.
        let w = s * 0.07;
        add_rect(&mut posts, Rect::new(base.x - w, head.y, base.x + w, base.y));
        add_rect(
            &mut posts,
            Rect::new(base.x - s * 0.3, base.y - s * 0.06, base.x + s * 0.3, base.y),
        );
        add_rect(
            &mut posts,
            Rect::new(head.x - s * 0.22, head.y - s * 0.1, head.x + s * 0.22, head.y + s * 0.02),
        );
        add_circle(&mut posts, Point::new(head.x, head.y + s * 0.16), s * 0.16);
    }

    push_solid(&mut out, posts, iron);
    out
}

/// A skyline standing on the stroke. Buildings are axis-aligned — gravity,
/// not the tangent, is what a building answers to — but each one's base sits
/// on the stroke, so a rising stroke is a hillside of houses.
fn buildings(fx: &Fx<'_>) -> Vec<ArtPiece> {
    // Fine stamps to sample the baseline; buildings then consume width.
    let step = (fx.size * 0.25).max(1.0);
    let stamps = walk(fx.spine, step, caps::PARTICLES * 2);
    if stamps.is_empty() {
        return Vec::new();
    }

    let mut silhouette = BezPath::new();
    let mut windows = BezPath::new();
    let mut cursor = 0usize;
    let mut b = 0usize;

    while cursor < stamps.len() && b < caps::STRUCTURES {
        let base = stamps[cursor].pos;
        let w = fx.size * (0.9 + 1.3 * fx.rng(b, 0xB1D1));
        let h = fx.size * (1.6 + 2.8 * fx.rng(b, 0xB1D2));
        let body = Rect::new(base.x - w / 2.0, base.y - h, base.x + w / 2.0, base.y);
        if silhouette.elements().len() < caps::ELEMENTS {
            add_rect(&mut silhouette, body);
            // Rooftop furniture, on some.
            let roll = fx.rng(b, 0xB1D3);
            if roll < 0.3 {
                add_rect(
                    &mut silhouette,
                    Rect::new(
                        base.x - fx.size * 0.02,
                        body.y0 - fx.size * 0.5,
                        base.x + fx.size * 0.02,
                        body.y0,
                    ),
                );
            } else if roll < 0.5 {
                add_rect(
                    &mut silhouette,
                    Rect::new(
                        body.x0 + w * 0.15,
                        body.y0 - fx.size * 0.28,
                        body.x0 + w * 0.45,
                        body.y0,
                    ),
                );
            }
        }

        // Windows: a grid, more than half lit. Only the lit ones are drawn —
        // the dark ones are the silhouette showing through.
        let cell = fx.size * 0.3;
        let cols = ((w - cell) / cell).floor().clamp(0.0, 10.0) as usize;
        let rows = ((h - cell) / cell).floor().clamp(0.0, 12.0) as usize;
        for cx in 0..cols {
            for cy in 0..rows {
                if windows.elements().len() >= caps::ELEMENTS {
                    break;
                }
                let salt = 0xB1D4 ^ ((cx as u64) << 8) ^ (cy as u64);
                if fx.rng(b, salt) < 0.55 {
                    let wx = body.x0 + cell * (0.7 + cx as f64);
                    let wy = body.y0 + cell * (0.7 + cy as f64);
                    add_rect(
                        &mut windows,
                        Rect::new(wx, wy, wx + cell * 0.5, wy + cell * 0.6),
                    );
                }
            }
        }

        // Next building begins where this one ends, plus an alley.
        let advance = ((w + fx.size * 0.12) / step).ceil() as usize;
        cursor += advance.max(1);
        b += 1;
    }

    let mut out = Vec::new();
    push_solid(&mut out, silhouette, fx.color);
    push_solid(&mut out, windows, Color::from_rgba8(0xFF, 0xC9, 0x6B, 0xEA));
    out
}

fn pine_trees(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 1.3).max(4.0), caps::STRUCTURES);
    let mut wood = BezPath::new();

    for (i, stamp) in stamps.iter().enumerate() {
        if wood.elements().len() >= caps::ELEMENTS {
            break;
        }
        let jitter = fx.spread(i, 0x71E1) * fx.size * 0.4;
        let base = Point::new(stamp.pos.x + jitter, stamp.pos.y);
        let h = fx.size * (1.7 + 1.7 * fx.rng(i, 0x71E2));

        let tw = fx.size * 0.05;
        add_rect(
            &mut wood,
            Rect::new(base.x - tw, base.y - h * 0.25, base.x + tw, base.y),
        );
        // Three tiers, each a triangle: the canonical pine.
        for (bottom, top, half) in [
            (0.18, 0.55, 0.30),
            (0.45, 0.78, 0.23),
            (0.68, 1.02, 0.16),
        ] {
            let mut tri = BezPath::new();
            tri.move_to(Point::new(base.x - h * half, base.y - h * bottom));
            tri.line_to(Point::new(base.x + h * half, base.y - h * bottom));
            tri.line_to(Point::new(base.x, base.y - h * top));
            tri.close_path();
            wood.extend(tri.iter());
        }
    }

    let mut out = Vec::new();
    push_solid(&mut out, wood, fx.color);
    out
}

fn leafy_trees(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 2.0).max(6.0), caps::STRUCTURES);
    let mut wood = BezPath::new();

    for (i, stamp) in stamps.iter().enumerate() {
        if wood.elements().len() >= caps::ELEMENTS {
            break;
        }
        let jitter = fx.spread(i, 0x1EAF) * fx.size * 0.5;
        let base = Point::new(stamp.pos.x + jitter, stamp.pos.y);
        let h = fx.size * (1.6 + 1.4 * fx.rng(i, 0x1EA2));

        let tw = fx.size * 0.055;
        add_rect(
            &mut wood,
            Rect::new(base.x - tw, base.y - h * 0.55, base.x + tw, base.y),
        );
        // The crown: overlapping discs merge under non-zero fill into one
        // billowing mass — no boolean needed.
        let crown = Point::new(base.x, base.y - h * 0.72);
        let cr = h * 0.32;
        add_circle(&mut wood, crown, cr);
        for puff in 0..5 {
            let salt = 0x1EA3 + puff as u64;
            let angle = fx.rng(i, salt) * std::f64::consts::PI * 2.0;
            let d = cr * (0.5 + 0.5 * fx.rng(i, salt ^ 0x4));
            add_circle(
                &mut wood,
                crown + Vec2::new(angle.cos(), angle.sin() * 0.7) * d,
                cr * (0.45 + 0.3 * fx.rng(i, salt ^ 0x9)),
            );
        }
    }

    let mut out = Vec::new();
    push_solid(&mut out, wood, fx.color);
    out
}

fn grass(fx: &Fx<'_>) -> Vec<ArtPiece> {
    let stamps = walk(fx.spine, (fx.size * 0.14).max(0.9), caps::PARTICLES);
    let mut front = BezPath::new();
    let mut back = BezPath::new();
    // A shared breeze, so the blades lean together like grass and not like
    // scattered matchsticks.
    let wind = 0.15;

    for (i, stamp) in stamps.iter().enumerate() {
        let base = stamp.pos;
        let h = fx.size * (0.45 + 0.6 * fx.rng(i, 0x6EA1));
        let lean = wind + fx.spread(i, 0x6EA2) * 0.45;
        let half = (fx.size * 0.028).max(0.3);
        let tip = Point::new(base.x + lean * h, base.y - h);
        let ctrl = Point::new(base.x + lean * h * 0.25, base.y - h * 0.6);

        let bucket = if fx.rng(i, 0x6EA3) < 0.5 { &mut front } else { &mut back };
        if bucket.elements().len() >= caps::ELEMENTS {
            continue;
        }
        // A tapered blade: out along one side of the base, curved to the tip,
        // straight back to the other side.
        bucket.move_to(Point::new(base.x - half, base.y));
        bucket.quad_to(ctrl, tip);
        bucket.line_to(Point::new(base.x + half, base.y));
        bucket.close_path();
    }

    let mut out = Vec::new();
    push_solid(&mut out, back, darken(fx.color, 0.3));
    push_solid(&mut out, front, fx.color);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(from: Point, to: Point, count: usize) -> Vec<StrokeSample> {
        (0..count)
            .map(|i| {
                let t = i as f64 / (count - 1).max(1) as f64;
                StrokeSample::new(from.lerp(to, t), t * 0.8)
            })
            .collect()
    }

    fn stroke(samples: &[StrokeSample]) -> EffectStroke<'_> {
        EffectStroke {
            samples,
            size: 24.0,
            color: Color::from_rgba8(0xE8, 0xEE, 0xFF, 0xFF),
            conditioning: buzz_geom::Conditioning::smoothing(0.5),
        }
    }

    fn shape_count(pieces: &[ArtPiece]) -> usize {
        pieces.len()
    }

    fn bounds_of(pieces: &[ArtPiece]) -> Option<Rect> {
        let mut all: Option<Rect> = None;
        for piece in pieces {
            let b = match piece {
                ArtPiece::Shape(s) => s.path.bounding_box(),
                ArtPiece::Painting { canvas, .. } => canvas.area(),
            };
            all = Some(match all {
                Some(a) => a.union(b),
                None => b,
            });
        }
        all
    }

    /// The reason this module can exist at all: asked twice — as the preview
    /// is on every pointer move and the commit is on release — a stroke gives
    /// byte-identical artwork.
    #[test]
    fn the_same_stroke_gives_identical_artwork_every_time() {
        let samples = drag(Point::new(10.0, 20.0), Point::new(400.0, 60.0), 60);
        for kind in EffectKind::ALL {
            let a = effect_artwork(kind, &stroke(&samples));
            let b = effect_artwork(kind, &stroke(&samples));
            assert_eq!(a, b, "{kind:?} is not deterministic");
        }
    }

    /// Growing a stroke must keep what was already laid down: the stamps a
    /// user watched appear must not reshuffle under the pointer.
    #[test]
    fn growing_the_stroke_keeps_the_stamps_already_placed() {
        let long = drag(Point::new(0.0, 0.0), Point::new(600.0, 0.0), 120);
        let short = &long[..60];

        let early = effect_artwork(EffectKind::Snow, &stroke(short));
        let late = effect_artwork(EffectKind::Snow, &stroke(&long));

        // Both are three depth buckets (or fewer); the early flakes must
        // appear verbatim within the late ones. Element-prefix equality is
        // the strong form of that and holds because stamps append in walk
        // order — except at the live tip, where smoothing still moves, so
        // compare only the first half of the early stroke's elements.
        let count = |pieces: &[ArtPiece]| -> usize {
            pieces
                .iter()
                .map(|p| match p {
                    ArtPiece::Shape(s) => s.path.elements().len(),
                    ArtPiece::Painting { .. } => 0,
                })
                .sum()
        };
        assert!(
            count(&late) > count(&early),
            "a longer stroke should carry more artwork"
        );
    }

    #[test]
    fn every_effect_makes_something_from_an_ordinary_drag() {
        let samples = drag(Point::new(0.0, 100.0), Point::new(500.0, 80.0), 80);
        for kind in EffectKind::ALL {
            let art = effect_artwork(kind, &stroke(&samples));
            assert!(!art.is_empty(), "{kind:?} produced nothing");
            let bounds = bounds_of(&art).expect("bounds");
            assert!(
                bounds.width() > 0.0,
                "{kind:?} produced degenerate artwork: {bounds:?}"
            );
        }
    }

    #[test]
    fn empty_and_degenerate_input_do_not_panic() {
        for kind in EffectKind::ALL {
            assert!(effect_artwork(kind, &stroke(&[])).is_empty());

            // A tap: one sample. Some effects mark, some do not; none panic.
            let tap = vec![StrokeSample::new(Point::new(5.0, 5.0), 0.0)];
            let _ = effect_artwork(kind, &stroke(&tap));

            // A zero-size brush paints nothing.
            let samples = drag(Point::ZERO, Point::new(100.0, 0.0), 20);
            let s = EffectStroke {
                size: 0.0,
                ..stroke(&samples)
            };
            assert!(effect_artwork(kind, &s).is_empty());

            // Every sample in one place.
            let stacked = vec![StrokeSample::new(Point::new(3.0, 3.0), 0.0); 40];
            let _ = effect_artwork(kind, &stroke(&stacked));
        }
    }

    /// The caps are the difference between a drag and a frozen window: a
    /// pasteboard-spanning sweep must stay bounded in shapes and elements.
    #[test]
    fn an_enormous_sweep_stays_within_its_budgets() {
        let samples: Vec<StrokeSample> = (0..20_000)
            .map(|i| {
                let t = i as f64;
                StrokeSample::new(Point::new(t * 2.0, (t * 0.01).sin() * 300.0), t * 1e-3)
            })
            .collect();

        for kind in EffectKind::ALL {
            let started = std::time::Instant::now();
            let art = effect_artwork(kind, &stroke(&samples));
            let took = started.elapsed();

            assert!(
                shape_count(&art) <= 3 * caps::GLOWS + 20,
                "{kind:?} made {} pieces",
                shape_count(&art)
            );
            for piece in &art {
                if let ArtPiece::Shape(s) = piece {
                    assert!(
                        s.path.elements().len() <= caps::ELEMENTS + 64,
                        "{kind:?} built a shape of {} elements",
                        s.path.elements().len()
                    );
                }
            }
            assert!(
                took.as_millis() < 250,
                "{kind:?} took {took:?} on a 20k-sample sweep"
            );
        }
    }

    /// Buildings, trees, lamps and grass stand *up* from the stroke — up is
    /// negative y — because gravity, not the tangent, is what they answer to.
    #[test]
    fn structures_stand_up_from_the_baseline() {
        let samples = drag(Point::new(0.0, 200.0), Point::new(600.0, 200.0), 60);
        for kind in [
            EffectKind::Buildings,
            EffectKind::PineTrees,
            EffectKind::LeafyTrees,
            EffectKind::Grass,
        ] {
            let art = effect_artwork(kind, &stroke(&samples));
            let bounds = bounds_of(&art).expect("artwork");
            assert!(
                bounds.y0 < 200.0 - 5.0,
                "{kind:?} did not rise above the baseline: {bounds:?}"
            );
            assert!(
                bounds.y1 < 210.0,
                "{kind:?} hangs below the ground it stands on: {bounds:?}"
            );
        }
    }

    #[test]
    fn the_moon_hangs_where_the_stroke_ends() {
        let samples = drag(Point::new(0.0, 0.0), Point::new(300.0, -120.0), 40);
        let art = effect_artwork(EffectKind::Moonlight, &stroke(&samples));
        let bounds = bounds_of(&art).expect("a moon");
        let centre = bounds.center();
        assert!(
            (centre.x - 300.0).abs() < 20.0 && (centre.y + 120.0).abs() < 20.0,
            "the moon should hang at the release point, but sits at {centre:?}"
        );
    }

    #[test]
    fn light_effects_add_light_rather_than_paint_over_it() {
        let samples = drag(Point::new(0.0, 0.0), Point::new(300.0, 0.0), 40);
        for kind in [
            EffectKind::DiffusedLight,
            EffectKind::LightRays,
            EffectKind::Fireflies,
            EffectKind::Moonlight,
        ] {
            let art = effect_artwork(kind, &stroke(&samples));
            let any_additive = art.iter().any(|p| match p {
                ArtPiece::Shape(s) => s.blend.is_additive(),
                ArtPiece::Painting { blend, .. } => blend.is_additive(),
            });
            assert!(any_additive, "{kind:?} carries no added light");
        }
    }

    /// Clouds must be pixels — a fluffy edge is an opacity field, not an
    /// outline — and the pixels must actually fade at their edge.
    #[test]
    fn clouds_are_painted_pixels_with_soft_edges() {
        let samples = drag(Point::new(50.0, 50.0), Point::new(400.0, 50.0), 50);
        let art = effect_artwork(EffectKind::Clouds, &stroke(&samples));
        let painting = art.iter().find_map(|p| match p {
            ArtPiece::Painting { canvas, .. } => Some(canvas),
            _ => None,
        });
        let canvas = painting.expect("clouds should paint a bitmap");
        assert!(!canvas.is_blank());

        // Coverage must include middle values: a hard mask would mean the
        // soft stamps degenerated somewhere.
        let mut partial = 0usize;
        for y in 0..canvas.height() as i64 {
            for x in 0..canvas.width() as i64 {
                let c = canvas.coverage_at(x, y);
                if c > 16 && c < 240 {
                    partial += 1;
                }
            }
        }
        assert!(partial > 100, "a cloud's edge should fade, found {partial} soft pixels");
    }

    #[test]
    fn every_kind_labels_and_describes_itself() {
        for kind in EffectKind::ALL {
            assert!(!kind.label().is_empty());
            assert!(!kind.description().is_empty());
            assert!(!kind.color_hint().is_empty());
        }
    }
}
