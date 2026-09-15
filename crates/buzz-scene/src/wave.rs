//! Waves: a stroke that flows — smoke, water, hair, ribbons.
//!
//! An effect brush ([`crate::effect_brush`]) scatters things *around* a
//! stroke. This one bends things *along* it. One drag lays down a bundle of
//! strands that follow the gesture and undulate across it, and the same
//! arithmetic that makes a rising plume of smoke makes a river's currents and
//! a head of wavy hair — the difference between them is the shape of the
//! envelope, not the shape of the maths. So they are one generator with
//! presets rather than three that drift apart.
//!
//! Everything comes out as ordinary [`ShapeData`], exactly as the effect
//! brushes do, so the Lasso, the eraser, tweens and the exporter all work on
//! the result without knowing a wave made it.
//!
//! # The phase is the whole feature
//!
//! A still wave is a decoration. A wave that *moves* is smoke, and the only
//! difference between the two is one number: [`wave_artwork`] takes a `phase`
//! in turns, and every strand's displacement is a sine of it. Advance the
//! phase across frames and the smoke rises, the river runs and the hair sways.
//! [`wave_loop`] does exactly that, and lands each frame on its own keyframe.
//!
//! **The loop closes exactly.** Phase is reduced into `0..1` on entry, and
//! every harmonic's phase is reduced the same way, so the artwork at phase 1
//! is not merely similar to the artwork at phase 0 — it is the same artwork,
//! piece for piece. That is what makes a baked loop seamless rather than
//! *nearly* seamless, and a cycle that visibly jolts once per second is worse
//! than no animation at all.
//!
//! # Determinism is the contract
//!
//! As with the effect brushes: the same stroke at the same phase must give the
//! same artwork every time it is asked, because it is asked twice — once per
//! pointer move for the live preview, once on release for the commit. All
//! per-strand variation comes from [`splitmix`](crate::effect_brush), seeded
//! by where the stroke began.
//!
//! # Budgets
//!
//! A wave is rebuilt on every pointer move while it is being drawn, and baked
//! frame count multiplies that on release. [`caps`] is what keeps a long drag
//! with sixty strands and a hundred frames from freezing the window.

use buzz_geom::brush::{BrushBudget, StrokeSample, condition};
use buzz_geom::{Affine, BezPath, Point, Rect, Vec2};
use peniko::Color;
use serde::{Deserialize, Serialize};

use crate::art::ArtPiece;
use crate::effect_brush::{darken, lighten, splitmix, with_alpha};
use crate::gradient::{Gradient, GradientKind, GradientStop};
use crate::object::{FillSpec, PaintBlend, ShapeData};

/// Which flowing thing the stroke lays down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WaveKind {
    /// A plume that widens, wanders and fades as it rises.
    #[default]
    Smoke,
    /// Long currents running along the stroke, with a sheen on the crests.
    River,
    /// Strands held at the root and loose at the tip, tapering to points.
    Hair,
    /// One plain undulating band — the generic wave, and the one to reach for
    /// when the preset you want is not on this list.
    Ribbon,
}

impl WaveKind {
    pub const ALL: [WaveKind; 4] = [Self::Smoke, Self::River, Self::Hair, Self::Ribbon];

    pub fn label(self) -> &'static str {
        match self {
            Self::Smoke => "Smoke",
            Self::River => "River",
            Self::Hair => "Wavy Hair",
            Self::Ribbon => "Ribbon",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Smoke => "A plume that widens and fades as it rises — drag upward",
            Self::River => "Currents running along the stroke, crests catching the light",
            Self::Hair => "Wavy strands, held at the root and tapering to their tips",
            Self::Ribbon => "One undulating band — the plain wave, yours to shape",
        }
    }

    /// What the fill swatch becomes, for the tool options.
    pub fn color_hint(self) -> &'static str {
        match self {
            Self::Smoke => "The plume is the fill colour, fading out as it rises",
            Self::River => "The water is the fill colour; crests are lit, troughs dark",
            Self::Hair => "Strands sit either side of the fill colour, for depth",
            Self::Ribbon => "Painted in the fill colour",
        }
    }

    /// Settings that make this kind look like the thing it is named after.
    ///
    /// Picking a kind loads these; every one of them is then a slider. The
    /// preset is a starting point, not a lock — which is the difference
    /// between a wave *feature* and four hard-coded effects.
    pub fn preset(self) -> WaveSettings {
        match self {
            // Few, fat, wandering strands that lean with the draught.
            Self::Smoke => WaveSettings {
                amplitude: 0.9,
                wavelength: 3.5,
                strands: 7,
                spread: 0.5,
                thickness: 0.6,
                taper: 0.15,
                turbulence: 0.55,
                drift: 0.35,
                frames: 24,
                cycles: 1,
            },
            // Many thin currents spread wide across the flow, running fast.
            Self::River => WaveSettings {
                amplitude: 0.35,
                wavelength: 4.5,
                strands: 16,
                spread: 2.6,
                thickness: 0.16,
                taper: 0.1,
                turbulence: 0.35,
                drift: 0.0,
                frames: 24,
                cycles: 2,
            },
            // A bundle of tapering strands, still at the root.
            Self::Hair => WaveSettings {
                amplitude: 0.7,
                wavelength: 2.6,
                strands: 18,
                // Wide enough that the strands read separately at the root
                // rather than merging into one slab of colour.
                spread: 1.3,
                thickness: 0.16,
                taper: 0.9,
                turbulence: 0.2,
                drift: 0.05,
                frames: 24,
                cycles: 1,
            },
            Self::Ribbon => WaveSettings {
                amplitude: 0.8,
                wavelength: 3.0,
                strands: 3,
                spread: 0.5,
                thickness: 0.5,
                taper: 0.25,
                turbulence: 0.15,
                drift: 0.0,
                frames: 24,
                cycles: 1,
            },
        }
    }

    /// Which way a *tap* flows, when there is no drag to take a direction
    /// from. Smoke rises; everything else runs to the right.
    fn tap_direction(self) -> Vec2 {
        match self {
            Self::Smoke => Vec2::new(0.0, -1.0),
            _ => Vec2::new(1.0, 0.0),
        }
    }
}

/// Everything about a wave that is a slider rather than a gesture.
///
/// Lengths are in multiples of the brush size, not document units, so a wave
/// drawn at size 8 and the same wave at size 80 are the same wave — which is
/// what makes these settings portable between drawings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaveSettings {
    /// How far a strand swings across the stroke, in brush sizes.
    pub amplitude: f64,
    /// Distance between one crest and the next, in brush sizes.
    pub wavelength: f64,
    /// How many strands the bundle holds.
    pub strands: usize,
    /// How far the bundle spreads across the stroke, in brush sizes.
    pub spread: f64,
    /// Strand width as a fraction of the brush size.
    pub thickness: f64,
    /// How much a strand narrows towards its far end, `0.0`–`1.0`. At `1.0` it
    /// finishes in a point, which is what a hair does.
    pub taper: f64,
    /// A second, shorter wave laid over the first, `0.0`–`1.0`. Without it a
    /// bundle reads as a machine-drawn sine; with it, as something moving in
    /// air or water.
    pub turbulence: f64,
    /// A steady lean across the stroke that grows along it — the draught that
    /// bends a plume, in brush sizes.
    pub drift: f64,
    /// Frames to bake when the wave is committed. `1` commits a still.
    pub frames: u32,
    /// Whole waves that pass in one baked loop. Whole, because a fractional
    /// count is a loop that jolts when it repeats.
    pub cycles: u32,
}

impl Default for WaveSettings {
    fn default() -> Self {
        WaveKind::default().preset()
    }
}

impl WaveSettings {
    /// The settings actually used: every field clamped into the range the
    /// generator is budgeted for. Called once at the top of
    /// [`wave_artwork`] so nothing downstream has to defend itself.
    fn sane(self) -> Self {
        Self {
            amplitude: self.amplitude.clamp(0.0, 8.0),
            wavelength: self.wavelength.clamp(0.2, 40.0),
            strands: self.strands.clamp(1, caps::STRANDS),
            spread: self.spread.clamp(0.0, 20.0),
            thickness: self.thickness.clamp(0.01, 4.0),
            taper: self.taper.clamp(0.0, 1.0),
            turbulence: self.turbulence.clamp(0.0, 1.0),
            drift: self.drift.clamp(-8.0, 8.0),
            frames: self.frames.clamp(1, caps::FRAMES),
            cycles: self.cycles.clamp(1, 32),
        }
    }

    /// Whether committing this wave lays down an animation rather than a
    /// still.
    pub fn is_animated(&self) -> bool {
        self.frames > 1
    }

    /// Phase at `frame`, for a wave left free-running rather than baked into
    /// a loop of its own — the same wave sampled on a document's timeline.
    pub fn phase_at(&self, frame: u32) -> f64 {
        let frames = self.frames.clamp(1, caps::FRAMES) as f64;
        (self.cycles.max(1) as f64 * frame as f64 / frames).rem_euclid(1.0)
    }
}

/// The gesture a wave is generated from.
#[derive(Debug, Clone, Copy)]
pub struct WaveStroke<'a> {
    pub samples: &'a [StrokeSample],
    /// The brush size, in document units. Sets the scale of everything.
    pub size: f64,
    /// The fill swatch. See [`WaveKind::color_hint`] for what each kind does
    /// with it.
    pub color: Color,
    /// How the pointer samples are cleaned up before the spine is built — the
    /// Smoothing and Stabiliser settings, applied exactly as every other brush
    /// here applies them.
    pub conditioning: buzz_geom::Conditioning,
    pub settings: WaveSettings,
}

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

/// Hard limits per wave. See the module header: a wave is rebuilt on every
/// pointer move, and baking multiplies that by the frame count.
mod caps {
    /// Strands in one bundle.
    pub const STRANDS: usize = 64;
    /// Points along one strand's centreline. A strand is a ribbon, so it costs
    /// twice this many path elements.
    pub const POINTS: usize = 320;
    /// Path elements one depth bucket may hold.
    pub const ELEMENTS: usize = 24_000;
    /// Frames one commit may bake. Ten seconds at 24fps.
    pub const FRAMES: u32 = 240;
}

/// Depth buckets. A shape holds one paint, so strands are grouped rather than
/// painted one at a time: three shapes for the whole bundle instead of sixty,
/// and the three tints are what give the bundle its depth.
const DEPTHS: usize = 3;

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Build the artwork one wave lays down at `phase`.
///
/// `phase` is in turns and is reduced into `0..1`, so phase 1 *is* phase 0 —
/// see the module header on why the loop has to close exactly. Empty input
/// gives empty output; a tap gives a short wave in the kind's own direction,
/// because a brush that silently does nothing reads as a broken brush.
pub fn wave_artwork(kind: WaveKind, stroke: &WaveStroke<'_>, phase: f64) -> Vec<ArtPiece> {
    if stroke.samples.is_empty() || stroke.size <= 0.0 {
        return Vec::new();
    }

    let size = stroke.size.clamp(1.0, 400.0);
    let settings = stroke.settings.sane();

    // Conditioned exactly as the fluid brush conditions its input, so the
    // Smoothing slider means the same thing on every brush.
    let conditioned = condition(stroke.samples, stroke.conditioning, &BrushBudget::default());
    let raw: Vec<Point> = conditioned.iter().map(|s| s.point).collect();
    let Some(spine) = Spine::build(&raw, size, kind) else {
        return Vec::new();
    };

    let wave = Wave {
        spine: &spine,
        size,
        color: stroke.color,
        settings,
        // Seeded by where the stroke began, not by the whole path, so the
        // strands already on screen hold still as the stroke grows.
        seed: seed_for(kind, spine.points[0], size),
        // Reduced here, once. Everything below reads this and never `phase`.
        phase: phase.rem_euclid(1.0),
    };

    let mut buckets: Vec<BezPath> = vec![BezPath::new(); DEPTHS];
    for k in 0..settings.strands {
        let bucket = &mut buckets[k % DEPTHS];
        if bucket.elements().len() >= caps::ELEMENTS {
            continue;
        }
        bucket.extend(wave.strand(kind, k).iter());
    }

    paint(kind, &wave, buckets)
}

/// Bake one seamless cycle of a wave, one entry per frame.
///
/// The frame count and the number of whole cycles in it are
/// [`WaveSettings::frames`] and [`WaveSettings::cycles`]. Frame 0 is the
/// artwork the live preview showed, which is what lets the thing drawn and the
/// thing committed be the same thing.
pub fn wave_loop(kind: WaveKind, stroke: &WaveStroke<'_>) -> Vec<Vec<ArtPiece>> {
    let settings = stroke.settings.sane();
    let frames = settings.frames;
    let cycles = settings.cycles as f64;
    (0..frames)
        .map(|i| {
            let phase = cycles * i as f64 / frames as f64;
            wave_artwork(kind, stroke, phase)
        })
        .collect()
}

/// The stroke's seed: where it began, what it is, how big.
fn seed_for(kind: WaveKind, start: Point, size: f64) -> u64 {
    let k = WaveKind::ALL.iter().position(|x| *x == kind).unwrap_or(0) as u64;
    splitmix(start.x.to_bits() ^ start.y.to_bits().rotate_left(21) ^ size.to_bits() ^ (k << 5))
}

// ---------------------------------------------------------------------------
// The spine
// ---------------------------------------------------------------------------

/// The stroke, resampled evenly by arc length.
///
/// Even spacing rather than the raw pointer samples, for the reason every
/// brush here resamples: the density of pointer events must not show up as the
/// density of the artwork. Arc length is carried alongside because a wave is a
/// function of *distance along the stroke*, not of point index — otherwise the
/// crests bunch up wherever the hand moved slowly.
struct Spine {
    points: Vec<Point>,
    /// Distance from the start to each point.
    arc: Vec<f64>,
    length: f64,
}

impl Spine {
    fn build(raw: &[Point], size: f64, kind: WaveKind) -> Option<Spine> {
        let first = *raw.first()?;

        let mut total = 0.0;
        for pair in raw.windows(2) {
            total += (pair[1] - pair[0]).hypot();
        }

        // A tap, or a stroke too short to bend: give it a direction of its own
        // so the brush still marks. Smoke rises from where you tapped.
        if total < size * 0.5 {
            return Some(Spine::straight(first, kind.tap_direction(), size * 6.0));
        }

        // Fine enough that a curve stays a curve, coarse enough that a
        // pasteboard-long sweep stays inside the point cap.
        let step = (size * 0.2).max(total / caps::POINTS as f64).max(0.25);
        let count = ((total / step).floor() as usize + 1).min(caps::POINTS);

        let mut points = Vec::with_capacity(count);
        let mut arc = Vec::with_capacity(count);
        let mut cursor = 0usize;
        let mut walked = 0.0;
        for i in 0..count {
            let target = total * (i as f64 / (count - 1).max(1) as f64);
            while cursor + 2 < raw.len() {
                let seg = (raw[cursor + 1] - raw[cursor]).hypot();
                if walked + seg >= target {
                    break;
                }
                walked += seg;
                cursor += 1;
            }
            let a = raw[cursor];
            let b = raw[(cursor + 1).min(raw.len() - 1)];
            let seg = (b - a).hypot();
            let local = if seg > 0.0 {
                ((target - walked) / seg).clamp(0.0, 1.0)
            } else {
                0.0
            };
            points.push(a.lerp(b, local));
            arc.push(target);
        }

        Some(Spine {
            points,
            arc,
            length: total,
        })
    }

    /// A straight spine of `length` from `at`, for a tap.
    fn straight(at: Point, dir: Vec2, length: f64) -> Spine {
        let count = 48usize;
        let mut points = Vec::with_capacity(count);
        let mut arc = Vec::with_capacity(count);
        for i in 0..count {
            let d = length * i as f64 / (count - 1) as f64;
            points.push(at + dir * d);
            arc.push(d);
        }
        Spine {
            points,
            arc,
            length,
        }
    }

    fn len(&self) -> usize {
        self.points.len()
    }

    /// Unit normal at point `i` — the direction a wave swings in.
    fn normal(&self, i: usize) -> Vec2 {
        let n = self.points.len();
        let a = self.points[i.saturating_sub(1)];
        let b = self.points[(i + 1).min(n - 1)];
        perpendicular(b - a)
    }

    /// Where the stroke went, overall: start, direction and length. Used to
    /// lay a fade along the plume rather than along the page.
    fn axis(&self) -> (Point, Vec2, f64) {
        let start = self.points[0];
        let end = self.points[self.points.len() - 1];
        let reach = end - start;
        if reach.hypot() < 1e-9 {
            (start, Vec2::new(1.0, 0.0), self.length.max(1.0))
        } else {
            (start, reach.normalize(), reach.hypot())
        }
    }
}

/// The unit normal of a direction. `(0, -1)` for a degenerate one, so a
/// stalled stroke swings somewhere rather than collapsing.
fn perpendicular(d: Vec2) -> Vec2 {
    let len = d.hypot();
    if len <= 1e-12 {
        Vec2::new(0.0, -1.0)
    } else {
        Vec2::new(-d.y / len, d.x / len)
    }
}

// ---------------------------------------------------------------------------
// Building one strand
// ---------------------------------------------------------------------------

/// Everything a strand is generated from.
struct Wave<'a> {
    spine: &'a Spine,
    size: f64,
    color: Color,
    settings: WaveSettings,
    seed: u64,
    /// Already reduced into `0..1`.
    phase: f64,
}

impl Wave<'_> {
    /// A deterministic value in `[0, 1)` for strand `index`, stream `salt`.
    fn rng(&self, index: usize, salt: u64) -> f64 {
        let x = splitmix(
            self.seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt << 17,
        );
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// One strand, as a closed ribbon around its own wavy centreline.
    fn strand(&self, kind: WaveKind, k: usize) -> BezPath {
        let s = &self.settings;
        let n = self.spine.len();
        if n < 2 {
            return BezPath::new();
        }

        // Where in the bundle this strand sits, `-1`..`1` across the stroke,
        // plus a little jitter so a bundle is not a comb.
        let across = if s.strands <= 1 {
            0.0
        } else {
            2.0 * k as f64 / (s.strands - 1) as f64 - 1.0
        };
        let lane = (across + (self.rng(k, 0x1A11) - 0.5) * 0.35) * s.spread * self.size;

        // Per-strand variation. Without it every strand crests in the same
        // place and the bundle reads as one thick painted sine.
        let amp = s.amplitude * self.size * (0.7 + 0.6 * self.rng(k, 0x1A22));
        let lambda = (s.wavelength * self.size * (0.85 + 0.3 * self.rng(k, 0x1A33))).max(0.5);
        let psi = self.rng(k, 0x1A44);
        let psi2 = self.rng(k, 0x1A55);
        let girth = 0.5 * s.thickness * self.size * (0.75 + 0.5 * self.rng(k, 0x1A66));

        // The harmonic runs three times as fast. Reduced into `0..1` the same
        // way the fundamental is: mathematically a no-op, since sine has
        // period one in this argument, and it is what keeps phase 1 *bitwise*
        // equal to phase 0 rather than merely close to it.
        let phase2 = (self.phase * 3.0).rem_euclid(1.0);

        let mut centre = Vec::with_capacity(n);
        let mut half = Vec::with_capacity(n);
        for i in 0..n {
            let u = if self.spine.length > 0.0 {
                (self.spine.arc[i] / self.spine.length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let arc = self.spine.arc[i];

            let swing = swing_envelope(kind, u);
            let turns = arc / lambda + psi - self.phase;
            let mut offset = amp * swing * (std::f64::consts::TAU * turns).sin();

            if s.turbulence > 0.0 {
                let fast = arc / (lambda * 0.41) + psi2 - phase2;
                offset += s.turbulence
                    * amp
                    * 0.35
                    * swing
                    * (std::f64::consts::TAU * fast).sin();
            }

            // The draught: a lean that grows along the stroke.
            offset += s.drift * self.size * u;

            let spread = lane * fan_envelope(kind, u);
            centre.push(self.spine.points[i] + self.spine.normal(i) * (spread + offset));
            half.push((girth * girth_envelope(kind, u) * (1.0 - s.taper * u.powf(1.2))).max(0.0));
        }

        ribbon(&centre, &half)
    }
}

/// How far a strand swings, along the stroke.
///
/// This is the only thing that separates a plume from a river: smoke is still
/// at its source and wanders more the further it gets, hair is held at the
/// root and loose at the tip, and water swings the same all the way along
/// because a current has no root.
fn swing_envelope(kind: WaveKind, u: f64) -> f64 {
    match kind {
        WaveKind::Smoke => u.powf(0.75),
        WaveKind::Hair => u.powf(1.4),
        WaveKind::River | WaveKind::Ribbon => 1.0,
    }
}

/// How far *apart* the strands sit, along the stroke.
///
/// Hair leaves a parting gathered and fans out towards the tips, and smoke
/// comes from a source rather than from a line. Without this the bundle is a
/// set of parallel lanes, which at the root of a head of hair reads as a comb
/// of flat bars — the one place the generator plainly looked drawn rather than
/// grown. A current has no source, so water and a plain ribbon keep their
/// spacing all the way along.
fn fan_envelope(kind: WaveKind, u: f64) -> f64 {
    match kind {
        WaveKind::Hair => 0.5 + 0.5 * u,
        WaveKind::Smoke => 0.45 + 0.55 * u,
        WaveKind::River | WaveKind::Ribbon => 1.0,
    }
}

/// How wide a strand is, along the stroke — before [`WaveSettings::taper`].
fn girth_envelope(kind: WaveKind, u: f64) -> f64 {
    match kind {
        // A plume widens as it rises: that spread *is* what reads as smoke.
        WaveKind::Smoke => 0.3 + 1.9 * u,
        WaveKind::River | WaveKind::Hair | WaveKind::Ribbon => 1.0,
    }
}

/// A closed band around a centreline, `half` units wide either side.
///
/// The normals come from the *centreline*, not from the spine, so the band
/// stays the same width round a crest instead of pinching on the inside of the
/// bend. Where `half` reaches zero the band closes to a point, which is how a
/// tapered hair ends.
fn ribbon(centre: &[Point], half: &[f64]) -> BezPath {
    let n = centre.len().min(half.len());
    if n < 2 {
        return BezPath::new();
    }
    let normal = |i: usize| -> Vec2 {
        perpendicular(centre[(i + 1).min(n - 1)] - centre[i.saturating_sub(1)])
    };

    let mut path = BezPath::new();
    for i in 0..n {
        let p = centre[i] + normal(i) * half[i];
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    for i in (0..n).rev() {
        path.line_to(centre[i] - normal(i) * half[i]);
    }
    path.close_path();
    path
}

// ---------------------------------------------------------------------------
// Paint
// ---------------------------------------------------------------------------

/// Turn the depth buckets into artwork, in the kind's own colours.
///
/// Back bucket first, so the near strands draw over the far ones — the same
/// rule the effect brushes' depth buckets follow.
fn paint(kind: WaveKind, wave: &Wave<'_>, buckets: Vec<BezPath>) -> Vec<ArtPiece> {
    let color = wave.color;
    let mut out = Vec::with_capacity(DEPTHS);

    for (depth, path) in buckets.into_iter().enumerate() {
        if path.elements().is_empty() {
            continue;
        }
        // 0 is the back of the bundle, `DEPTHS - 1` the front.
        let t = depth as f64 / (DEPTHS - 1) as f64;
        let shape = match kind {
            // Smoke fades out along its own rise, which no flat fill can say:
            // one linear ramp per bucket, laid on the stroke's own axis.
            WaveKind::Smoke => {
                let alpha = 0.16 + 0.22 * t;
                smoke_shape(path, wave, color, alpha)
            }
            WaveKind::River => {
                let tint = match depth {
                    0 => darken(color, 0.3),
                    1 => color,
                    _ => lighten(color, 0.55),
                };
                solid(path, with_alpha(tint, 0.55 + 0.35 * t))
            }
            WaveKind::Hair => {
                let tint = match depth {
                    0 => darken(color, 0.35),
                    1 => color,
                    _ => lighten(color, 0.22),
                };
                solid(path, with_alpha(tint, 0.85 + 0.15 * t))
            }
            WaveKind::Ribbon => solid(path, with_alpha(color, 0.55 + 0.4 * t)),
        };
        out.push(ArtPiece::Shape(shape));
    }
    out
}

fn solid(path: BezPath, color: Color) -> ShapeData {
    ShapeData {
        path,
        fill: Some(FillSpec::solid(color)),
        stroke: None,
        blend: PaintBlend::Normal,
    }
}

/// A plume that thins out as it rises.
///
/// The ramp runs along the stroke's own axis rather than down the page, so
/// smoke blown sideways fades sideways. Built the way `light_rays` builds its
/// beams: the linear ramp lives on the unit segment `(-1,0)..(1,0)` and is
/// carried onto the axis by its transform.
fn smoke_shape(path: BezPath, wave: &Wave<'_>, color: Color, alpha: f64) -> ShapeData {
    let (start, dir, length) = wave.spine.axis();
    let mut ramp = Gradient::new(
        GradientKind::Linear,
        vec![
            GradientStop::new(0.0, with_alpha(color, alpha)),
            GradientStop::new(0.45, with_alpha(color, alpha * 0.7)),
            GradientStop::new(1.0, with_alpha(color, 0.0)),
        ],
    );
    let mid = start + dir * (length / 2.0);
    ramp.transform = Affine::translate(mid.to_vec2())
        * Affine::rotate(dir.atan2())
        * Affine::scale(length / 2.0);

    ShapeData {
        path,
        fill: Some(FillSpec {
            paint: ramp.into(),
            rule: buzz_geom::FillMode::NonZero,
            swatch: None,
        }),
        stroke: None,
        blend: PaintBlend::Normal,
    }
}

/// Where a wave's artwork sits, without developing it — the bounds of every
/// piece together. Empty artwork has no bounds.
pub fn wave_bounds(pieces: &[ArtPiece]) -> Option<Rect> {
    pieces
        .iter()
        .map(ArtPiece::bounds)
        .reduce(|a, b| a.union(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;

    fn drag(from: Point, to: Point, count: usize) -> Vec<StrokeSample> {
        (0..count)
            .map(|i| {
                let t = i as f64 / (count - 1).max(1) as f64;
                StrokeSample::new(from.lerp(to, t), t * 0.8)
            })
            .collect()
    }

    fn stroke<'a>(samples: &'a [StrokeSample], kind: WaveKind) -> WaveStroke<'a> {
        WaveStroke {
            samples,
            size: 24.0,
            color: Color::from_rgba8(0xC8, 0xD4, 0xE8, 0xFF),
            conditioning: buzz_geom::Conditioning::smoothing(0.5),
            settings: kind.preset(),
        }
    }

    fn upward(count: usize) -> Vec<StrokeSample> {
        drag(Point::new(200.0, 400.0), Point::new(220.0, 60.0), count)
    }

    fn area(pieces: &[ArtPiece]) -> f64 {
        pieces
            .iter()
            .filter_map(|p| match p {
                ArtPiece::Shape(s) => Some(s.path.area().abs()),
                ArtPiece::Painting { .. } => None,
            })
            .sum()
    }

    // -- the contract --------------------------------------------------------

    /// Asked twice — as the preview is on every pointer move and the commit is
    /// on release — a wave gives byte-identical artwork.
    #[test]
    fn the_same_wave_gives_identical_artwork_every_time() {
        let samples = upward(60);
        for kind in WaveKind::ALL {
            for phase in [0.0, 0.25, 0.7] {
                let a = wave_artwork(kind, &stroke(&samples, kind), phase);
                let b = wave_artwork(kind, &stroke(&samples, kind), phase);
                assert_eq!(a, b, "{kind:?} is not deterministic at phase {phase}");
            }
        }
    }

    /// **The loop closes exactly.** Not approximately: a baked cycle whose
    /// last frame merely resembles its first jolts once every time round, and
    /// that is the one defect a looping wave cannot have.
    #[test]
    fn a_whole_turn_of_phase_returns_the_very_same_artwork() {
        let samples = upward(60);
        for kind in WaveKind::ALL {
            let at_zero = wave_artwork(kind, &stroke(&samples, kind), 0.0);
            let at_one = wave_artwork(kind, &stroke(&samples, kind), 1.0);
            assert_eq!(at_zero, at_one, "{kind:?} does not close its loop");

            // And from the other side, where `rem_euclid` has to do real work.
            let below = wave_artwork(kind, &stroke(&samples, kind), -1.0);
            assert_eq!(at_zero, below, "{kind:?} mishandles a negative phase");
        }
    }

    #[test]
    fn a_baked_loop_has_one_entry_per_frame_and_starts_where_the_preview_did() {
        let samples = upward(60);
        for kind in WaveKind::ALL {
            let s = stroke(&samples, kind);
            let frames = wave_loop(kind, &s);
            assert_eq!(frames.len(), kind.preset().frames as usize, "{kind:?}");

            let preview = wave_artwork(kind, &s, 0.0);
            assert_eq!(
                frames[0], preview,
                "{kind:?}: the first baked frame must be what was previewed"
            );
        }
    }

    /// A still is one frame, and it is the same one frame the preview showed.
    #[test]
    fn a_frame_count_of_one_bakes_a_still() {
        let samples = upward(40);
        let mut s = stroke(&samples, WaveKind::Ribbon);
        s.settings.frames = 1;
        assert!(!s.settings.is_animated());

        let frames = wave_loop(WaveKind::Ribbon, &s);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], wave_artwork(WaveKind::Ribbon, &s, 0.0));
    }

    /// The point of the phase: the artwork actually *moves*.
    #[test]
    fn advancing_the_phase_moves_the_strands() {
        let samples = upward(60);
        for kind in WaveKind::ALL {
            let s = stroke(&samples, kind);
            let a = wave_artwork(kind, &s, 0.0);
            let b = wave_artwork(kind, &s, 0.25);
            assert_ne!(a, b, "{kind:?} stands still when the phase advances");
        }
    }

    /// Every frame of a loop is distinct: a cycle that repeats a frame is a
    /// cycle that stutters.
    #[test]
    fn no_two_frames_of_a_loop_are_the_same() {
        let samples = upward(50);
        let mut s = stroke(&samples, WaveKind::River);
        s.settings.frames = 12;
        s.settings.cycles = 1;

        let frames = wave_loop(WaveKind::River, &s);
        for i in 0..frames.len() {
            for j in (i + 1)..frames.len() {
                assert_ne!(frames[i], frames[j], "frames {i} and {j} are identical");
            }
        }
    }

    // -- the settings are real ------------------------------------------------

    /// Every slider has to change the artwork, or it is decoration on a panel.
    #[test]
    fn each_setting_changes_what_is_drawn() {
        let samples = upward(60);
        let base = stroke(&samples, WaveKind::Ribbon);
        let reference = wave_artwork(WaveKind::Ribbon, &base, 0.15);

        let variants: [(&str, WaveSettings); 7] = [
            ("amplitude", WaveSettings { amplitude: base.settings.amplitude * 2.5, ..base.settings }),
            ("wavelength", WaveSettings { wavelength: base.settings.wavelength * 0.4, ..base.settings }),
            ("strands", WaveSettings { strands: base.settings.strands + 5, ..base.settings }),
            ("spread", WaveSettings { spread: base.settings.spread * 3.0, ..base.settings }),
            ("thickness", WaveSettings { thickness: base.settings.thickness * 2.0, ..base.settings }),
            ("taper", WaveSettings { taper: 0.95, ..base.settings }),
            ("turbulence", WaveSettings { turbulence: 1.0, ..base.settings }),
        ];

        for (name, settings) in variants {
            let altered = wave_artwork(
                WaveKind::Ribbon,
                &WaveStroke { settings, ..base },
                0.15,
            );
            assert_ne!(altered, reference, "{name} changed nothing");
        }
    }

    #[test]
    fn drift_leans_the_wave_to_one_side() {
        let samples = drag(Point::new(200.0, 400.0), Point::new(200.0, 100.0), 60);
        let base = stroke(&samples, WaveKind::Ribbon);

        let straight = WaveStroke {
            settings: WaveSettings { drift: 0.0, ..base.settings },
            ..base
        };
        let leaning = WaveStroke {
            settings: WaveSettings { drift: 3.0, ..base.settings },
            ..base
        };

        let a = wave_bounds(&wave_artwork(WaveKind::Ribbon, &straight, 0.0)).expect("art");
        let b = wave_bounds(&wave_artwork(WaveKind::Ribbon, &leaning, 0.0)).expect("art");
        assert!(
            b.max_x() > a.max_x() + 20.0,
            "drift should push the far end aside: {a:?} then {b:?}"
        );
    }

    /// Amplitude is measured across the stroke, so a wave drawn straight up
    /// gets wider — not longer — when it is turned up.
    #[test]
    fn amplitude_widens_the_wave_across_the_stroke() {
        let samples = drag(Point::new(200.0, 400.0), Point::new(200.0, 100.0), 60);
        let base = stroke(&samples, WaveKind::Ribbon);
        let calm = WaveStroke {
            settings: WaveSettings { amplitude: 0.2, ..base.settings },
            ..base
        };
        let wild = WaveStroke {
            settings: WaveSettings { amplitude: 3.0, ..base.settings },
            ..base
        };

        let a = wave_bounds(&wave_artwork(WaveKind::Ribbon, &calm, 0.0)).expect("art");
        let b = wave_bounds(&wave_artwork(WaveKind::Ribbon, &wild, 0.0)).expect("art");
        assert!(
            b.width() > a.width() * 2.0,
            "a bigger amplitude should swing wider: {} then {}",
            a.width(),
            b.width()
        );
    }

    /// Settings out of range are clamped, not obeyed: a strand count of a
    /// million is a frozen window.
    #[test]
    fn absurd_settings_are_clamped_rather_than_obeyed() {
        let samples = upward(40);
        let base = stroke(&samples, WaveKind::Hair);
        let absurd = WaveStroke {
            settings: WaveSettings {
                amplitude: 1e6,
                wavelength: 0.0,
                strands: 100_000,
                spread: 1e9,
                thickness: -4.0,
                taper: 12.0,
                turbulence: 90.0,
                drift: -1e6,
                frames: 100_000,
                cycles: 0,
            },
            ..base
        };

        let started = std::time::Instant::now();
        let art = wave_artwork(WaveKind::Hair, &absurd, 0.3);
        assert!(started.elapsed().as_millis() < 250, "clamping did not hold");
        assert!(art.len() <= DEPTHS);
        for piece in &art {
            if let ArtPiece::Shape(s) = piece {
                assert!(s.path.elements().len() <= caps::ELEMENTS + 2 * caps::POINTS + 8);
            }
        }

        // The frame count is clamped too, so a commit cannot bake for ever.
        let frames = wave_loop(WaveKind::Hair, &absurd);
        assert_eq!(frames.len(), caps::FRAMES as usize);
    }

    // -- the kinds look like what they are named after -------------------------

    /// Smoke widens as it rises. Measured as the artwork being wider near the
    /// top of an upward stroke than near its base.
    #[test]
    fn a_plume_of_smoke_widens_as_it_rises() {
        let samples = drag(Point::new(200.0, 400.0), Point::new(200.0, 80.0), 80);
        let art = wave_artwork(WaveKind::Smoke, &stroke(&samples, WaveKind::Smoke), 0.0);

        // Horizontal extent of the paths' points, in two bands of the rise.
        let extent = |low: f64, high: f64| -> f64 {
            let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
            for piece in &art {
                if let ArtPiece::Shape(s) = piece {
                    kurbo::flatten(s.path.iter(), 0.2, |el| {
                        if let kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) = el
                            && p.y >= low
                            && p.y <= high
                        {
                            min = min.min(p.x);
                            max = max.max(p.x);
                        }
                    });
                }
            }
            if min.is_finite() { max - min } else { 0.0 }
        };

        let base = extent(340.0, 400.0);
        let top = extent(80.0, 140.0);
        assert!(
            top > base * 1.5,
            "smoke should spread as it rises: {base:.1} at the source, {top:.1} at the top"
        );
    }

    /// Smoke fades out along its rise, and a flat fill cannot say that — so
    /// the plume must carry a gradient.
    #[test]
    fn smoke_fades_out_along_its_own_rise() {
        let samples = upward(60);
        let art = wave_artwork(WaveKind::Smoke, &stroke(&samples, WaveKind::Smoke), 0.0);
        let faded = art.iter().any(|p| match p {
            ArtPiece::Shape(s) => matches!(
                s.fill.as_ref().map(|f| &f.paint),
                Some(crate::object::Paint::Gradient(_))
            ),
            ArtPiece::Painting { .. } => false,
        });
        assert!(faded, "a plume painted flat does not read as smoke");
    }

    /// Hair tapers to points: the far end of a strand is much thinner than its
    /// root, which is what stops a head of hair reading as a bundle of tubes.
    #[test]
    fn hair_tapers_towards_its_tips() {
        let samples = drag(Point::new(120.0, 80.0), Point::new(140.0, 460.0), 80);
        let s = stroke(&samples, WaveKind::Hair);
        let art = wave_artwork(WaveKind::Hair, &s, 0.0);

        let full = area(&art);
        let untapered = wave_artwork(
            WaveKind::Hair,
            &WaveStroke {
                settings: WaveSettings { taper: 0.0, ..s.settings },
                ..s
            },
            0.0,
        );
        assert!(
            full < area(&untapered) * 0.8,
            "tapered hair should cover less than untapered: {full:.0} vs {:.0}",
            area(&untapered)
        );
    }

    /// Hair leaves its parting gathered and fans out towards the tips. Without
    /// that, the roots are a comb of parallel bars — which is what they were.
    #[test]
    fn hair_is_gathered_at_the_root_and_fanned_at_the_tips() {
        let samples = drag(Point::new(240.0, 60.0), Point::new(240.0, 420.0), 80);
        let art = wave_artwork(WaveKind::Hair, &stroke(&samples, WaveKind::Hair), 0.0);

        // Horizontal extent of the artwork in two bands of the fall.
        let extent = |low: f64, high: f64| -> f64 {
            let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
            for piece in &art {
                if let ArtPiece::Shape(s) = piece {
                    kurbo::flatten(s.path.iter(), 0.2, |el| {
                        if let kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) = el
                            && p.y >= low
                            && p.y <= high
                        {
                            min = min.min(p.x);
                            max = max.max(p.x);
                        }
                    });
                }
            }
            if min.is_finite() { max - min } else { 0.0 }
        };

        let root = extent(60.0, 110.0);
        let tips = extent(370.0, 420.0);
        assert!(root > 0.0, "the roots should be drawn at all");
        assert!(
            tips > root * 1.5,
            "hair should fan out: {root:.1} at the root, {tips:.1} at the tips"
        );
    }

    /// A river spreads across its stroke: many thin currents, wide apart.
    #[test]
    fn a_river_spreads_across_the_flow() {
        let samples = drag(Point::new(60.0, 300.0), Point::new(500.0, 300.0), 80);
        let art = wave_artwork(WaveKind::River, &stroke(&samples, WaveKind::River), 0.0);
        let bounds = wave_bounds(&art).expect("a river");
        assert!(
            bounds.height() > 24.0 * 2.0,
            "the currents should spread across the flow, got {:?}",
            bounds
        );
        assert!(bounds.width() > 400.0, "and run its whole length");
    }

    // -- degenerate input ------------------------------------------------------

    #[test]
    fn empty_and_degenerate_input_do_not_panic() {
        for kind in WaveKind::ALL {
            assert!(wave_artwork(kind, &stroke(&[], kind), 0.0).is_empty());
            assert!(wave_loop(kind, &stroke(&[], kind)).iter().all(Vec::is_empty));

            // A zero-size brush paints nothing.
            let samples = drag(Point::ZERO, Point::new(100.0, 0.0), 20);
            let none = WaveStroke {
                size: 0.0,
                ..stroke(&samples, kind)
            };
            assert!(wave_artwork(kind, &none, 0.4).is_empty());

            // Every sample in one place — a stroke with no length at all.
            let stacked = vec![StrokeSample::new(Point::new(3.0, 3.0), 0.0); 40];
            let _ = wave_artwork(kind, &stroke(&stacked, kind), 0.4);
        }
    }

    /// A tap still marks, in the kind's own direction: smoke rises from where
    /// you tapped rather than doing nothing at all.
    #[test]
    fn a_tap_still_lays_down_a_wave() {
        let tap = vec![StrokeSample::new(Point::new(200.0, 300.0), 0.0)];
        for kind in WaveKind::ALL {
            let art = wave_artwork(kind, &stroke(&tap, kind), 0.0);
            let bounds = wave_bounds(&art).unwrap_or_else(|| panic!("{kind:?} made nothing"));
            assert!(bounds.width() > 0.0 && bounds.height() > 0.0, "{kind:?}");
        }
        // And it rises, rather than lying on its side.
        let art = wave_artwork(WaveKind::Smoke, &stroke(&tap, WaveKind::Smoke), 0.0);
        let bounds = wave_bounds(&art).expect("a plume");
        assert!(
            bounds.min_y() < 300.0 - 24.0,
            "a tapped plume should rise above the tap: {bounds:?}"
        );
    }

    /// The caps are the difference between a drag and a frozen window.
    #[test]
    fn an_enormous_sweep_stays_within_its_budgets() {
        let samples: Vec<StrokeSample> = (0..20_000)
            .map(|i| {
                let t = i as f64;
                StrokeSample::new(Point::new(t * 2.0, (t * 0.01).sin() * 300.0), t * 1e-3)
            })
            .collect();

        for kind in WaveKind::ALL {
            let started = std::time::Instant::now();
            let art = wave_artwork(kind, &stroke(&samples, kind), 0.3);
            let took = started.elapsed();

            assert!(art.len() <= DEPTHS, "{kind:?} made {} pieces", art.len());
            for piece in &art {
                if let ArtPiece::Shape(s) = piece {
                    assert!(
                        s.path.elements().len() <= caps::ELEMENTS + 2 * caps::POINTS + 8,
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

    /// Baking a whole loop of a long stroke has to stay affordable too — it is
    /// the frame count multiplying the per-frame cost, and it happens on
    /// pointer release while the user is waiting.
    #[test]
    fn baking_a_whole_loop_stays_affordable() {
        let samples = upward(120);
        let mut s = stroke(&samples, WaveKind::Smoke);
        s.settings.frames = 48;

        let started = std::time::Instant::now();
        let frames = wave_loop(WaveKind::Smoke, &s);
        assert_eq!(frames.len(), 48);
        assert!(
            started.elapsed().as_millis() < 1_500,
            "baking 48 frames took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn every_kind_labels_describes_and_presets_itself() {
        for kind in WaveKind::ALL {
            assert!(!kind.label().is_empty());
            assert!(!kind.description().is_empty());
            assert!(!kind.color_hint().is_empty());

            // A preset that needed clamping is a preset that is wrong.
            let preset = kind.preset();
            assert_eq!(preset, preset.sane(), "{kind:?}'s preset is out of range");
            assert!(preset.is_animated(), "{kind:?} should move by default");
        }
    }

    #[test]
    fn a_free_running_phase_wraps_with_the_cycle() {
        let s = WaveSettings {
            frames: 12,
            cycles: 1,
            ..WaveKind::Ribbon.preset()
        };
        assert!((s.phase_at(0) - 0.0).abs() < 1e-12);
        assert!((s.phase_at(6) - 0.5).abs() < 1e-12);
        assert!((s.phase_at(12) - 0.0).abs() < 1e-12, "a full cycle returns");
    }
}
