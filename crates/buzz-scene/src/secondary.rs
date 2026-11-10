//! Secondary motion for any drawing: a pendulum that swings from a pivot, and
//! (further down) a hair / cloth chain that bends from a pinned edge.
//!
//! # Why these live beside, not inside, [`crate::modifier`]
//!
//! Both are *integrated* motions: where the lamp is at frame 40 depends on
//! everything the pivot did in frames 0..40. That is a different shape of
//! problem from a wiggle (a pure function of time), and it needs the same three
//! things the spring modifier needs — the object's keyed motion read off the
//! timeline, a fixed-step solver, and a per-revision cache — so they share one
//! helper ([`Scene::anchor_track`]) and one cache rather than each growing its
//! own.
//!
//! # Deterministic regardless of playback order
//!
//! Every result is the **prefix of one table** built by integrating frame 0, 1,
//! 2, ... with a fixed sub-step count. Asking for frame 40 and then frame 10, or
//! frame 10 twice, reads the same table; there is no solver state that survives
//! between draws. The table is built once per document revision and extended (by
//! rebuilding longer, which reproduces the same prefix bit for bit) if a frame
//! past its end is asked for.

use std::sync::Arc;

use buzz_geom::{Point, Rect, Vec2};
use buzz_physics::{Spring, SpringState, Wiggle, wiggle_at};

use crate::modifier::SpringTable;
use crate::{LayerId, Object, Scene};

/// Gravity in stage units per second squared: about two hundred units to the
/// metre, a character a few hundred units tall on a 1080p stage. A period of two
/// seconds is then a lamp roughly a metre long — the one place a "natural
/// period" and a physical length meet. A smaller number would make every keyed
/// move shake the pivot proportionally harder.
const GRAVITY: f64 = 2000.0;

/// Integration sub-steps per frame. Eight keeps the stiffest allowed setting
/// (a 0.1 s period at 12 fps) well inside the stable range of the semi-implicit
/// step, with room to spare.
const SUBSTEPS: usize = 8;

/// The longest table ever built, in frames. A nonsense time from a script must
/// not allocate gigabytes; past this the last entry is held.
const MAX_FRAMES: usize = 100_000;

/// Cache keys: these stand in for the "chain root" the spring modifier keys on,
/// so all three share one table per revision without colliding.
const PENDULUM_KEY: usize = usize::MAX;
const CHAIN_KEY: usize = usize::MAX - 1;

// ---------------------------------------------------------------------------
// Pendulum
// ---------------------------------------------------------------------------

/// A pendulum's settings, sanitised: everything finite and in a range the
/// integrator is stable over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendulumSettings {
    /// Natural period of a small swing, in seconds.
    pub period: f64,
    /// Damping ratio. `0` swings forever, `1` returns without overshoot.
    pub damping: f64,
    /// How much of the pivot's own acceleration reaches the swing. `1` is
    /// physical; `0` ignores the object's motion entirely.
    pub reaction: f64,
    /// Where the swing starts, in radians (rotation applied to the drawing).
    pub initial_angle: f64,
    /// Idle sway: a slow wandering push, as the equilibrium angle (radians) it
    /// would hold if the pendulum were infinitely heavily damped.
    pub sway: f64,
    /// Hard stop either side, in radians.
    pub max_angle: f64,
}

impl PendulumSettings {
    /// Build from the modifier's user-facing numbers (angles in degrees).
    pub fn new(
        period: f64,
        damping: f64,
        reaction: f64,
        initial_angle_deg: f64,
        sway_deg: f64,
        max_angle_deg: f64,
    ) -> Self {
        let clean = |v: f64, lo: f64, hi: f64, fallback: f64| {
            if v.is_finite() { v.clamp(lo, hi) } else { fallback }
        };
        let max_angle = clean(max_angle_deg, 1.0, 170.0, 80.0).to_radians();
        Self {
            period: clean(period, 0.1, 30.0, 1.6),
            damping: clean(damping, 0.0, 2.0, 0.12),
            reaction: clean(reaction, 0.0, 4.0, 1.0),
            initial_angle: clean(initial_angle_deg, -180.0, 180.0, 0.0)
                .to_radians()
                .clamp(-max_angle, max_angle),
            sway: clean(sway_deg, 0.0, 45.0, 0.0).to_radians(),
            max_angle,
        }
    }
}

/// **Integrate a pendulum along a pivot track.**
///
/// `pivot[g]` is where the pivot is at frame `g` (the object's keyed motion);
/// the result has `frames` entries, entry `g` being the rotation, in radians,
/// to apply about the pivot at frame `g`. Before `first` (the first frame the
/// object exists) it sits at the initial angle.
///
/// # The equation
///
/// A rigid pendulum whose pivot accelerates by `a` feels gravity plus the
/// opposite pseudo-force, which in the swing angle `alpha` (positive turns the
/// hanging drawing's bottom towards -x, the direction `Affine::rotate` takes
/// it) is
///
/// ```text
/// alpha'' = -k sin(alpha) - 2 zeta w alpha' + reaction (k / g) (a_x cos(alpha) + a_y sin(alpha))
/// ```
///
/// with `k = w^2 = (2 pi / period)^2`, so the effective length is `g / k`. A
/// pivot pulled to the right (`a_x > 0`) therefore swings the bottom of the
/// drawing to the left, and a lamp whose carrier then stops swings back the
/// other way before settling: the behaviour of a real hanging thing.
///
/// Acceleration is the central second difference of the pivot track, held
/// constant across each frame's sub-steps.
pub fn pendulum_angles(
    settings: PendulumSettings,
    pivot: &[Point],
    first: usize,
    fps: f64,
    seed: u64,
    frames: usize,
) -> Vec<f64> {
    let dt = 1.0 / fps.max(1.0);
    let h = dt / SUBSTEPS as f64;
    let omega = std::f64::consts::TAU / settings.period;
    let k = omega * omega;
    let c = 2.0 * settings.damping * omega;
    let gain = settings.reaction * k / GRAVITY;
    let sway = Wiggle::new(1.0, 1.0 / settings.period);

    let at = |g: usize| -> Point {
        if pivot.is_empty() {
            Point::ORIGIN
        } else {
            pivot[g.min(pivot.len() - 1)]
        }
    };
    let accel = |g: usize| -> Vec2 {
        let (before, here, after) = (at(g.saturating_sub(1)), at(g), at(g + 1));
        ((after.to_vec2() - here.to_vec2() * 2.0) + before.to_vec2()) / (dt * dt)
    };

    let mut angle = settings.initial_angle;
    let mut velocity = 0.0;
    let mut out = Vec::with_capacity(frames);
    for g in 0..frames {
        out.push(angle);
        if g < first {
            continue;
        }
        let a = accel(g);
        for s in 0..SUBSTEPS {
            let t = (g as f64 + s as f64 / SUBSTEPS as f64) * dt;
            let push = if settings.sway > 0.0 {
                k * settings.sway * wiggle_at(sway, seed ^ 0x9E4D_17A5, t).dx
            } else {
                0.0
            };
            let acceleration = -k * angle.sin() - c * velocity
                + gain * (a.x * angle.cos() + a.y * angle.sin())
                + push;
            velocity += acceleration * h;
            angle += velocity * h;
            // Hard stops, inelastic: the swing ends at the limit instead of
            // spinning the drawing round its pivot.
            if angle > settings.max_angle {
                angle = settings.max_angle;
                velocity = velocity.min(0.0);
            } else if angle < -settings.max_angle {
                angle = -settings.max_angle;
                velocity = velocity.max(0.0);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Shared: reading the object's keyed motion, and the cache
// ---------------------------------------------------------------------------

impl Scene {
    /// **Where an anchor on the object is at every frame of its layer**, read
    /// off the keyframes with the object's modifiers *not* applied.
    ///
    /// `pick` chooses the anchor from the object's resolved bounds (the pivot,
    /// or the pinned edge). The first element of the result is the first frame
    /// the object exists on, so a character that arrives at frame 40 does not
    /// start swinging at frame 0; frames before that hold that first position.
    /// Frames past the end of the layer hold the last position, which is what
    /// the timeline does too.
    ///
    /// Returns `(first_frame, track)`.
    pub(crate) fn anchor_track(
        &self,
        layer: LayerId,
        object: &Object,
        pick: impl Fn(Rect) -> Point,
    ) -> Option<(usize, Vec<Point>)> {
        let layers = self.layers();
        let timeline = &layers.get(layer)?.frames;
        let span = (timeline.length().max(1) as usize).min(MAX_FRAMES);
        let table = self.cached_bounds_table();
        let mut track: Vec<Option<Point>> = Vec::with_capacity(span);
        for g in 0..span {
            let resolved = timeline.resolved_at(g as u32);
            track.push(
                resolved
                    .iter()
                    .find(|o| o.id == object.id)
                    .map(|o| pick(self.resolved_bounds_with(o, &table))),
            );
        }
        let first = track.iter().position(Option::is_some)?;
        let mut held = track[first]?;
        let filled = track
            .into_iter()
            .map(|p| {
                if let Some(p) = p {
                    held = p;
                }
                held
            })
            .collect();
        Some((first, filled))
    }

    /// A per-revision cached table of per-frame numbers, built by `build`
    /// (given the number of frames wanted) when absent or too short.
    ///
    /// Shares the spring modifier's cache: one table per `(object, key)`,
    /// dropped whenever the document revision moves on.
    pub(crate) fn cached_table(
        &self,
        object: &Object,
        key: usize,
        needed: usize,
        build: impl FnOnce(usize) -> Option<Vec<Vec<f64>>>,
    ) -> Option<Arc<Vec<Vec<f64>>>> {
        let needed = needed.min(MAX_FRAMES);
        let revision = self.revision;
        let id = (object.id, key);
        if let Ok(cache) = self.modifier_cache.read()
            && let Some((cached, table)) = &*cache
            && *cached == revision
            && let Some(seq) = table.get(&id)
            && seq.len() >= needed
        {
            return Some(seq.clone());
        }
        // Rounded up so a playhead walking forward does not rebuild per frame.
        let frames = needed.next_power_of_two().clamp(64, MAX_FRAMES);
        let arc = Arc::new(build(frames)?);
        if let Ok(mut cache) = self.modifier_cache.write() {
            match &mut *cache {
                Some((cached, table)) if *cached == revision => {
                    table.insert(id, arc.clone());
                }
                slot => {
                    let mut table = SpringTable::new();
                    table.insert(id, arc.clone());
                    *slot = Some((revision, table));
                }
            }
        }
        Some(arc)
    }

    /// The pendulum's rotation (radians) at `time` (in frames, fractional for
    /// a motion-blurred sample, linearly interpolated between whole frames).
    pub(crate) fn pendulum_angle_at(
        &self,
        layer: LayerId,
        object: &Object,
        settings: PendulumSettings,
        offset: Vec2,
        time: f64,
        fps: f64,
    ) -> Option<f64> {
        let whole = if time.is_finite() && time > 0.0 { time.floor() as usize } else { 0 };
        let table = self.cached_table(object, PENDULUM_KEY, whole + 2, |frames| {
            let (first, pivot) =
                self.anchor_track(layer, object, |b| Point::new(b.center().x, b.y0) + offset)?;
            let angles = pendulum_angles(settings, &pivot, first, fps, object.id.0, frames);
            Some(angles.into_iter().map(|a| vec![a]).collect())
        })?;
        sample_table(&table, time, 0)
    }
}

/// Entry `column` of the table at fractional frame `time`, interpolated; held
/// at the last row past the end.
pub(crate) fn sample_table(table: &[Vec<f64>], time: f64, column: usize) -> Option<f64> {
    let last = table.len().checked_sub(1)?;
    let time = if time.is_finite() { time.max(0.0) } else { 0.0 };
    let i = (time.floor() as usize).min(last);
    let frac = if i == last { 0.0 } else { time - i as f64 };
    let a = *table[i].get(column)?;
    let b = *table[(i + 1).min(last)].get(column)?;
    Some(a + (b - a) * frac)
}

// ---------------------------------------------------------------------------
// Hair / cloth: a virtual chain of springs hanging off a pinned edge
// ---------------------------------------------------------------------------

/// The most links a chain is divided into. Fixed so [`ChainBend`] stays `Copy`
/// and rides through the renderer without an allocation per object per frame.
pub const MAX_LINKS: usize = 12;

/// Which edge of the drawing is pinned. The chain runs from that edge across
/// the drawing to the opposite one: a scarf pinned at the *top* hangs down, a
/// flag pinned at the *left* streams right, a tail pinned at the *right* trails
/// left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PinSide {
    #[default]
    Top,
    Left,
    Right,
    Bottom,
}

impl PinSide {
    /// Stable index used by the file format and the agent API.
    pub fn index(self) -> usize {
        match self {
            Self::Top => 0,
            Self::Left => 1,
            Self::Right => 2,
            Self::Bottom => 3,
        }
    }

    /// The side for an index; anything unknown is the top, the common case.
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Left,
            2 => Self::Right,
            3 => Self::Bottom,
            _ => Self::Top,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Bottom => "Bottom",
        }
    }

    pub const ALL: [PinSide; 4] = [Self::Top, Self::Left, Self::Right, Self::Bottom];

    /// The unit direction the chain runs in, away from the pinned edge (stage
    /// space, y down).
    pub fn axis(self) -> Vec2 {
        match self {
            Self::Top => Vec2::new(0.0, 1.0),
            Self::Left => Vec2::new(1.0, 0.0),
            Self::Right => Vec2::new(-1.0, 0.0),
            Self::Bottom => Vec2::new(0.0, -1.0),
        }
    }

    /// The middle of the pinned edge of `bounds`.
    pub fn pin_point(self, bounds: Rect) -> Point {
        let c = bounds.center();
        match self {
            Self::Top => Point::new(c.x, bounds.y0),
            Self::Left => Point::new(bounds.x0, c.y),
            Self::Right => Point::new(bounds.x1, c.y),
            Self::Bottom => Point::new(c.x, bounds.y1),
        }
    }

    /// How long the chain is: the drawing's extent along its axis.
    pub fn extent(self, bounds: Rect) -> f64 {
        match self {
            Self::Top | Self::Bottom => bounds.height(),
            Self::Left | Self::Right => bounds.width(),
        }
    }
}

/// A hair / cloth chain's settings, sanitised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainSettings {
    pub links: usize,
    pub spring: Spring,
    pub wind: f64,
    pub wind_rate: f64,
    pub lag: f64,
}

impl ChainSettings {
    pub fn new(links: usize, stiffness: f64, damping: f64, wind: f64, wind_rate: f64, lag: f64) -> Self {
        let clean = |v: f64, lo: f64, hi: f64, fallback: f64| {
            if v.is_finite() { v.clamp(lo, hi) } else { fallback }
        };
        Self {
            links: links.clamp(2, MAX_LINKS),
            // The upper bounds keep the explicit step stable at 12 fps.
            spring: Spring::new(clean(stiffness, 1.0, 1500.0, 120.0), clean(damping, 0.0, 120.0, 10.0)),
            wind: clean(wind, 0.0, 5.0, 0.0),
            wind_rate: clean(wind_rate, 0.05, 10.0, 0.6),
            lag: clean(lag, 0.0, 4.0, 1.0),
        }
    }
}

/// Lateral pseudo-acceleration (stage units / s^2) that bends the chain by one
/// "unit" of lag across all its links; and the same scale for wind, so a wind of
/// 1 is as strong as a quick sideways move.
const LATERAL_SCALE: f64 = 1500.0;
/// No single link bends further than this relative to its parent, in radians,
/// however violent the move: the cloth folds in a gentle curve, not a hinge.
const MAX_LINK_BEND: f64 = 0.35;

/// **Integrate the chain along a pin track.**
///
/// Row `g` holds one absolute angle per link (radians, relative to the rest
/// axis; positive turns the link towards `axis`'s left-hand normal
/// `(-axis.y, axis.x)`), the first link nearest the pin. This is
/// [`buzz_rig::follow_through_coupled`]'s model on a virtual skeleton: each
/// link is a damped [`SpringState`] chasing its *parent's* actual angle plus a
/// lean opposite to the pin's lateral acceleration, so the motion travels
/// down the chain with a delay and the tip lags most and settles last. The
/// chain is the object's own keyed motion in, secondary motion out; nothing is
/// read from the previous draw.
///
/// `wind` adds a deterministic gusting push (the wiggle's fractal noise), so a
/// flag keeps rippling after its carrier has stopped.
pub fn chain_angles(
    settings: ChainSettings,
    axis: Vec2,
    pin: &[Point],
    first: usize,
    fps: f64,
    seed: u64,
    frames: usize,
) -> Vec<Vec<f64>> {
    let dt = 1.0 / fps.max(1.0);
    let normal = Vec2::new(-axis.y, axis.x);
    let links = settings.links;
    let gust = Wiggle::new(1.0, settings.wind_rate);
    let at = |g: usize| -> Point {
        if pin.is_empty() { Point::ORIGIN } else { pin[g.min(pin.len() - 1)] }
    };
    // Per-link lean: the same push on every link, scaled so the *whole chain's*
    // bend does not grow with how finely it is divided.
    let lean_per_force = settings.lag / (LATERAL_SCALE * links as f64);

    let mut state = vec![SpringState::settle(0.0); links];
    let mut out = Vec::with_capacity(frames);
    for g in 0..frames {
        if g >= first {
            let a = (at(g + 1).to_vec2() - at(g).to_vec2() * 2.0 + at(g.saturating_sub(1)).to_vec2())
                / (dt * dt);
            // Opposite the acceleration, across the chain: a hanging scarf is
            // left behind by a move to the right.
            let mut force = -(a.x * normal.x + a.y * normal.y);
            if settings.wind > 0.0 {
                let n = wiggle_at(gust, seed ^ 0x57A1_D00F, g as f64 * dt).dx;
                force += settings.wind * LATERAL_SCALE * n;
            }
            let lean = (lean_per_force * force).clamp(-MAX_LINK_BEND, MAX_LINK_BEND);
            let mut parent = 0.0;
            for link in state.iter_mut() {
                link.step_angle(parent + lean, settings.spring, dt);
                parent = link.value;
            }
        }
        out.push(state.iter().map(|l| l.value).collect());
    }
    out
}

/// A bend to apply to artwork: the chain at one frame, ready to move points.
///
/// Everything in stage space. The mapping is a smooth bijection along the chain
/// — see [`Self::at`] — so a drawing bends instead of tearing into segments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainBend {
    /// The pinned point; nothing on or behind this edge ever moves.
    pub pin: Point,
    /// Unit vector from the pin along the drawing.
    pub axis: Vec2,
    /// Length of the chain, from the pin to the far edge.
    pub length: f64,
    links: usize,
    /// The tangent angle at each joint: `0` at the pin, then each link's angle.
    nodes: [f64; MAX_LINKS + 1],
}

impl ChainBend {
    /// The bend for one frame's link angles. `None` for a degenerate chain
    /// (zero or non-finite length, no angles) or one that is not bent at all,
    /// so the renderer can skip the deformation entirely.
    pub fn new(pin: Point, axis: Vec2, length: f64, angles: &[f64]) -> Option<Self> {
        if !(length.is_finite() && length > 1e-9) || !pin.x.is_finite() || !pin.y.is_finite() {
            return None;
        }
        let links = angles.len().min(MAX_LINKS);
        if links == 0 || angles.iter().any(|a| !a.is_finite()) {
            return None;
        }
        if angles.iter().all(|a| a.abs() < 1e-7) {
            return None;
        }
        let mut nodes = [0.0; MAX_LINKS + 1];
        for (j, a) in angles.iter().take(links).enumerate() {
            nodes[j + 1] = *a;
        }
        Some(Self { pin, axis, length, links, nodes })
    }

    /// Every number of the bend, as bits, for cache keys.
    pub fn words(&self) -> Vec<u64> {
        let mut words = vec![
            self.pin.x.to_bits(),
            self.pin.y.to_bits(),
            self.axis.x.to_bits(),
            self.axis.y.to_bits(),
            self.length.to_bits(),
            self.links as u64,
        ];
        words.extend(self.nodes.iter().map(|n| n.to_bits()));
        words
    }

    /// The farthest a point can end up from where it started: the chain length
    /// (a chain cannot stretch), plus a margin for the lateral offset. Used to
    /// widen the culling box around a bending drawing.
    pub fn reach(&self) -> f64 {
        self.length * 2.0
    }

    /// Where a point of the artwork goes.
    ///
    /// Let `a` be how far along the chain the point is and `w` how far to its
    /// side. The chain's centre-line is the curve whose tangent angle `phi(a)`
    /// is **piecewise linear between the joints** (not stepped from link to
    /// link), integrated exactly; the point goes to the centre-line at `a`,
    /// pushed `w` along the local normal. Because `phi` is continuous the map
    /// has no seams between links, and because `phi(0) = 0` the pinned edge is a
    /// fixed line. Behind the pin (`a <= 0`) nothing moves; past the far end the
    /// drawing rides rigidly on the tip.
    pub fn at(&self, p: Point) -> Point {
        let rel = p - self.pin;
        let a = rel.x * self.axis.x + rel.y * self.axis.y;
        if !(a > 0.0) {
            return p;
        }
        let normal = Vec2::new(-self.axis.y, self.axis.x);
        let w = rel.x * normal.x + rel.y * normal.y;

        let h = self.length / self.links as f64;
        let reach = a.min(self.length);
        // Integrate cos/sin of the tangent angle along the chain up to `reach`.
        let (mut x, mut y) = (0.0, 0.0);
        let mut phi = 0.0;
        let mut left = reach;
        for j in 0..self.links {
            if left <= 0.0 {
                break;
            }
            let u = left.min(h);
            let (p0, p1) = (self.nodes[j], self.nodes[j + 1]);
            let m = (p1 - p0) / h;
            let end = p0 + m * u;
            if (m * u).abs() < 1e-9 {
                x += u * p0.cos();
                y += u * p0.sin();
            } else {
                x += (end.sin() - p0.sin()) / m;
                y += (p0.cos() - end.cos()) / m;
            }
            phi = end;
            left -= u;
        }
        // Past the end, the drawing continues along the tip's tangent.
        let extra = (a - reach).max(0.0);
        x += extra * phi.cos();
        y += extra * phi.sin();
        // Lateral offset along the rotated normal.
        x += -w * phi.sin();
        y += w * phi.cos();
        let moved = self.pin + self.axis * x + normal * y;
        if moved.x.is_finite() && moved.y.is_finite() { moved } else { p }
    }
}

impl ChainBend {
    /// The path, bent.
    ///
    /// A path is first cut into short pieces (see [`buzz_geom::subdivide`]):
    /// mapping only a segment's end points would leave a long straight edge —
    /// the side of a flag — straight, bending nothing but its corners. The
    /// cuts are about half a link long, which is as fine as the chain's own
    /// angle is smooth, and are skipped entirely for the part of the drawing
    /// behind the pin or off the chain.
    pub fn path(&self, path: &buzz_geom::BezPath) -> buzz_geom::BezPath {
        use buzz_geom::Shape as _;
        if path.elements().is_empty() {
            return path.clone();
        }
        let bb = path.bounding_box();
        let along = |p: Point| (p - self.pin).dot(self.axis);
        let corners = [
            along(Point::new(bb.x0, bb.y0)),
            along(Point::new(bb.x1, bb.y0)),
            along(Point::new(bb.x0, bb.y1)),
            along(Point::new(bb.x1, bb.y1)),
        ];
        let lo = corners.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = corners.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if !(hi > 0.0) || !lo.is_finite() || !hi.is_finite() {
            // Entirely behind the pin (or not a number): nothing to move.
            return path.clone();
        }
        let half_link = self.length / self.links as f64 * 0.5;
        let pieces = (((hi - lo.max(0.0)) / half_link).ceil() as usize).clamp(1, 16);
        buzz_geom::map_points(&buzz_geom::subdivide(path, pieces), |p| self.at(p))
    }
}

impl Scene {
    /// The chain's per-link angles at `time` (frames; interpolated), one entry
    /// per link.
    pub(crate) fn chain_angles_at(
        &self,
        layer: LayerId,
        object: &Object,
        side: PinSide,
        settings: ChainSettings,
        time: f64,
        fps: f64,
    ) -> Option<Vec<f64>> {
        let whole = if time.is_finite() && time > 0.0 { time.floor() as usize } else { 0 };
        let table = self.cached_table(object, CHAIN_KEY, whole + 2, |frames| {
            let (first, pin) = self.anchor_track(layer, object, |b| side.pin_point(b))?;
            Some(chain_angles(settings, side.axis(), &pin, first, fps, object.id.0, frames))
        })?;
        (0..settings.links).map(|j| sample_table(&table, time, j)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayerKind, Modifier, ObjectId, ShapeData, Tween};
    use buzz_geom::{Affine, Shape as _};
    use peniko::Color;

    /// A hundred-unit square on a layer held for 120 frames, carrying `modifier`.
    fn hung_square(modifier: Modifier) -> (Scene, LayerId, ObjectId) {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Lamp", LayerKind::Normal);
        let object = Object::shape(
            ObjectId(7),
            ShapeData::filled(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9), Color::WHITE),
        );
        let id = scene.add_object(layer, object).unwrap();
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(120);
        });
        scene.update_object_across(0, u32::MAX, id, |o| o.modifiers.push(modifier));
        (scene, layer, id)
    }

    /// Slide the object 200 units to the right between frames 0 and 12.
    fn slide_right(scene: &mut Scene, layer: LayerId, id: ObjectId) {
        scene.ensure_keyframe(layer, 12);
        scene.update_object_at(12, id, |o| {
            o.transform = Affine::translate((200.0, 0.0)) * o.transform;
        });
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::motion());
        });
    }

    fn evaluate(scene: &Scene, layer: LayerId, id: ObjectId, time: u32) -> Affine {
        let object = scene
            .layers()
            .get(layer)
            .unwrap()
            .frames
            .resolved_at(time.min(119))
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .unwrap();
        scene
            .modified_object_at(layer, &object, time)
            .map_or(Affine::IDENTITY, |e| e.prepend)
    }

    fn rotation(a: Affine) -> f64 {
        let c = a.as_coeffs();
        c[1].atan2(c[0])
    }

    fn pendulum(initial: f64, reaction: f64) -> Modifier {
        Modifier::Pendulum {
            period: 1.2,
            damping: 0.25,
            reaction,
            initial_angle: initial,
            sway: 0.0,
            max_angle: 80.0,
            pivot_dx: 0.0,
            pivot_dy: 0.0,
        }
    }

    #[test]
    fn a_pendulum_settles_when_left_alone() {
        let (scene, layer, id) = hung_square(pendulum(30.0, 1.0));
        let start = rotation(evaluate(&scene, layer, id, 0));
        assert!((start.to_degrees() - 30.0).abs() < 1e-6, "starts at its initial angle");

        // It swings through vertical and out the other side before it dies away.
        let angles: Vec<f64> = (0..=119).map(|f| rotation(evaluate(&scene, layer, id, f))).collect();
        assert!(angles.iter().any(|a| *a < -0.05), "it should swing past hanging straight");
        let late = angles[119].to_degrees().abs();
        assert!(late < 1.0, "after five seconds it should be near rest, got {late} degrees");
    }

    #[test]
    fn a_pivot_pulled_sideways_swings_the_other_way() {
        let (mut scene, layer, id) = hung_square(pendulum(0.0, 1.0));
        slide_right(&mut scene, layer, id);
        // The slide starts at once, so the first frames accelerate to the
        // right: the bottom of the drawing lags to the left, which is a
        // positive rotation on a y-down stage.
        let early: Vec<f64> = (1..=4).map(|f| rotation(evaluate(&scene, layer, id, f))).collect();
        assert!(early.iter().all(|a| *a > 0.0), "expected a lag to the left, got {early:?}");
        // And it does not do that when told to ignore its own motion.
        let (mut calm, layer, id) = hung_square(pendulum(0.0, 0.0));
        slide_right(&mut calm, layer, id);
        assert!(rotation(evaluate(&calm, layer, id, 3)).abs() < 1e-12);
    }

    #[test]
    fn the_pivot_stays_put_while_the_drawing_swings() {
        let (scene, layer, id) = hung_square(Modifier::Pendulum {
            period: 1.2,
            damping: 0.25,
            reaction: 1.0,
            initial_angle: 40.0,
            sway: 0.0,
            max_angle: 80.0,
            pivot_dx: 10.0,
            pivot_dy: -20.0,
        });
        // Top-centre of the square, moved by the offset.
        let pivot = Point::new(50.0 + 10.0, 0.0 - 20.0);
        for frame in [0u32, 5, 17] {
            let t = evaluate(&scene, layer, id, frame);
            assert!(rotation(t).abs() > 1e-3, "frame {frame} should be swinging");
            let moved = t * pivot;
            assert!((moved - pivot).hypot() < 1e-9, "frame {frame}: the pivot moved to {moved:?}");
        }
    }

    #[test]
    fn evaluating_frames_out_of_order_gives_the_same_swing() {
        let (mut scene, layer, id) = hung_square(pendulum(20.0, 1.0));
        slide_right(&mut scene, layer, id);
        let forward: Vec<Affine> = [10u32, 40, 80].iter().map(|f| evaluate(&scene, layer, id, *f)).collect();
        // Backwards, twice, on a scene that has rebuilt its cache since.
        let mut again = scene.clone();
        again.update_object_across(0, u32::MAX, id, |_| {});
        for _ in 0..2 {
            for (frame, want) in [80u32, 40, 10].iter().zip(forward.iter().rev()) {
                let got = evaluate(&again, layer, id, *frame);
                assert_eq!(got.as_coeffs(), want.as_coeffs(), "frame {frame}");
            }
        }
        // And a frame past the end of the layer keeps integrating consistently
        // with one inside it, however large the table is asked to be.
        let far = evaluate(&scene, layer, id, 400);
        let nearer = evaluate(&scene, layer, id, 130);
        assert!(rotation(far).is_finite() && rotation(nearer).is_finite());
        assert_eq!(evaluate(&scene, layer, id, 40).as_coeffs(), forward[1].as_coeffs());
    }

    #[test]
    fn a_pendulum_with_odd_settings_stays_finite() {
        for modifier in [
            Modifier::Pendulum {
                period: 0.0,
                damping: -3.0,
                reaction: f64::NAN,
                initial_angle: 1e9,
                sway: f64::INFINITY,
                max_angle: 0.0,
                pivot_dx: f64::NAN,
                pivot_dy: 1e300,
            },
            pendulum(0.0, 4.0),
        ] {
            let (mut scene, layer, id) = hung_square(modifier);
            slide_right(&mut scene, layer, id);
            for frame in [0u32, 1, 13, 60] {
                let t = evaluate(&scene, layer, id, frame);
                assert!(t.as_coeffs().iter().all(|c| c.is_finite()), "frame {frame}");
            }
        }
    }

    // -- hair / cloth ---------------------------------------------------------

    fn hair_at(scene: &Scene, layer: LayerId, id: ObjectId, time: u32) -> Option<ChainBend> {
        let object = scene
            .layers()
            .get(layer)
            .unwrap()
            .frames
            .resolved_at(time.min(119))
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .unwrap();
        scene
            .modified_object_at(layer, &object, time)
            .and_then(|e| e.deform)
            .and_then(|d| d.bend)
    }

    /// Where the free end of the hung 100-unit square goes at `frame`, and the
    /// pin it hangs from (both move with the object's keyed slide).
    fn tip(scene: &Scene, layer: LayerId, id: ObjectId, frame: u32) -> (Point, Point) {
        match hair_at(scene, layer, id, frame) {
            Some(b) => (b.at(b.pin + b.axis * b.length), b.pin),
            None => {
                let object = scene.layers().get(layer).unwrap().frames.resolved_at(frame.min(119))
                    .iter().find(|o| o.id == id).cloned().unwrap();
                let pin = object.transform * Point::new(50.0, 0.0);
                (pin + Vec2::new(0.0, 100.0), pin)
            }
        }
    }

    #[test]
    fn the_pinned_edge_never_moves() {
        let (mut scene, layer, id) = hung_square(Modifier::hair());
        slide_right(&mut scene, layer, id);
        let mut bent = 0;
        for frame in 0..40 {
            let Some(bend) = hair_at(&scene, layer, id, frame) else { continue };
            bent += 1;
            // The pinned top edge, and everything behind it.
            for x in [0.0, 25.0, 50.0, 75.0, 100.0] {
                for y in [0.0, -30.0] {
                    let p = Point::new(x, y);
                    assert_eq!(bend.at(p), p, "frame {frame}: {p:?} moved");
                }
            }
        }
        assert!(bent > 5, "the hair should have been moving");
    }

    #[test]
    fn the_tip_lags_a_move_and_then_settles() {
        let (mut scene, layer, id) = hung_square(Modifier::hair());
        slide_right(&mut scene, layer, id);
        // The pin accelerates to the right, so the hanging tip is left behind
        // it: to the left of where it hangs from.
        let early = (1..=5)
            .map(|f| {
                let (tip, pin) = tip(&scene, layer, id, f);
                tip.x - pin.x
            })
            .fold(f64::INFINITY, f64::min);
        assert!(early < -3.0, "the tip should trail to the left, got {early}");
        // A chain does not stretch: the tip is never further than the chain is
        // long, and a bend only brings it nearer.
        for frame in 1..30 {
            let (tip, pin) = tip(&scene, layer, id, frame);
            let d = (tip - pin).hypot();
            assert!((60.0..=100.5).contains(&d), "frame {frame}: tip is {d} from the pin");
        }
        // It trails by *time* too: the tip is still swinging after the slide
        // has finished (frame 12), then it comes to rest hanging straight down.
        let (after, pin) = tip(&scene, layer, id, 15);
        assert!((after.x - pin.x).abs() > 0.5, "still swinging after the move ends");
        let (settled, pin) = tip(&scene, layer, id, 119);
        assert!((settled.x - pin.x).abs() < 0.5 && (settled.y - 100.0).abs() < 0.5, "{settled:?}");
    }

    #[test]
    fn the_bend_is_smooth_with_no_tearing_between_links() {
        let bend = ChainBend::new(
            Point::new(50.0, 0.0),
            Vec2::new(0.0, 1.0),
            100.0,
            &[0.1, 0.2, 0.35, 0.5, 0.6, 0.7],
        )
        .unwrap();
        // Walk down the middle of the drawing: neighbouring samples stay a
        // step apart, with no jump at a joint.
        let mut previous = bend.at(Point::new(50.0, 0.001));
        for i in 1..=1000 {
            let y = i as f64 * 0.1;
            let here = bend.at(Point::new(50.0, y));
            let step = (here - previous).hypot();
            assert!((0.05..0.15).contains(&step), "a jump of {step} at y = {y}");
            previous = here;
        }
        // A whole path comes out finite, closed and the same number of subpaths.
        let square = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9);
        let bent = bend.path(&square);
        assert!(bent.elements().len() > square.elements().len(), "its straight sides were cut to bend");
        for el in bent.elements() {
            for p in el.end_point().into_iter() {
                assert!(p.x.is_finite() && p.y.is_finite());
            }
        }
    }

    #[test]
    fn hair_is_the_same_whatever_order_frames_are_drawn_in() {
        let (mut scene, layer, id) = hung_square(hair(PinSide::Top, 0.5));
        slide_right(&mut scene, layer, id);
        let forward: Vec<Option<ChainBend>> = [8u32, 33, 90].iter().map(|f| hair_at(&scene, layer, id, *f)).collect();
        let mut again = scene.clone();
        again.update_object_across(0, u32::MAX, id, |_| {});
        for _ in 0..2 {
            for (frame, want) in [90u32, 33, 8].iter().zip(forward.iter().rev()) {
                assert_eq!(hair_at(&again, layer, id, *frame), *want, "frame {frame}");
            }
        }
        assert!(forward.iter().all(Option::is_some), "wind keeps it moving");
    }

    fn hair(side: PinSide, wind: f64) -> Modifier {
        Modifier::Hair {
            side,
            segments: 6,
            stiffness: 120.0,
            damping: 10.0,
            wind,
            wind_rate: 0.6,
            lag: 1.0,
        }
    }

    #[test]
    fn zero_size_shapes_and_odd_settings_produce_no_nan() {
        for rect in [
            Rect::new(0.0, 0.0, 0.0, 0.0),
            Rect::new(5.0, 5.0, 5.0, 60.0),
            Rect::new(5.0, 5.0, 60.0, 5.0),
        ] {
            for side in PinSide::ALL {
                let mut scene = Scene::empty();
                let layer = scene.add_layer("Hair", LayerKind::Normal);
                let object = Object::shape(ObjectId(3), ShapeData::filled(rect.to_path(1e-9), Color::WHITE));
                let id = scene.add_object(layer, object).unwrap();
                scene.update_layer(layer, |l| {
                    l.frames.insert_frame(30);
                });
                scene.update_object_across(0, u32::MAX, id, |o| {
                    o.modifiers.push(Modifier::Hair {
                        side,
                        segments: 0,
                        stiffness: f64::NAN,
                        damping: -1.0,
                        wind: f64::INFINITY,
                        wind_rate: 0.0,
                        lag: 1e300,
                    });
                });
                slide_right(&mut scene, layer, id);
                for frame in [0u32, 1, 6, 20] {
                    if let Some(bend) = hair_at(&scene, layer, id, frame) {
                        let p = bend.at(Point::new(rect.x1, rect.y1));
                        assert!(p.x.is_finite() && p.y.is_finite(), "{rect:?} {side:?} frame {frame}");
                    }
                }
            }
        }
        // A chain with no length is no chain at all.
        assert!(ChainBend::new(Point::ORIGIN, Vec2::new(0.0, 1.0), 0.0, &[0.3]).is_none());
        assert!(ChainBend::new(Point::ORIGIN, Vec2::new(0.0, 1.0), f64::NAN, &[0.3]).is_none());
        let bend = ChainBend::new(Point::ORIGIN, Vec2::new(0.0, 1.0), 10.0, &[0.3, 0.3]).unwrap();
        assert!(bend.path(&buzz_geom::BezPath::new()).elements().is_empty());
    }

    #[test]
    fn a_flag_pinned_at_the_left_streams_to_the_right_and_trails_a_move_up() {
        let (mut scene, layer, id) = hung_square(hair(PinSide::Left, 0.0));
        // Lift it: the pinned left edge accelerates upwards (y decreasing).
        scene.ensure_keyframe(layer, 12);
        scene.update_object_at(12, id, |o| {
            o.transform = Affine::translate((0.0, -200.0)) * o.transform;
        });
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::motion());
        });
        let tip = |frame: u32| {
            let free_end = Point::new(100.0, 50.0);
            hair_at(&scene, layer, id, frame).map_or(free_end, |b| b.at(free_end))
        };
        // Pulled up, the free end of the flag hangs lower than its pin.
        let lowest = (1..=5).map(|f| tip(f).y).fold(f64::NEG_INFINITY, f64::max);
        assert!(lowest > 53.0, "the free end should droop behind the move, got {lowest}");
    }
}
