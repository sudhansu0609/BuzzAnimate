//! Live modifiers: procedural motion evaluated at draw time.
//!
//! # What a modifier is
//!
//! A [`Modifier`] on an [`crate::Object`] is a small rule that changes how the
//! object is drawn at each frame — a spring that makes hair follow through, a
//! wiggle that keeps a held pose breathing. Unlike a baked performance it is
//! **not written to keyframes**: it is evaluated when the frame is drawn and is
//! deterministic in `(object id, frame)`, so the result is reproducible and,
//! crucially, stays in sync when the underlying animation is re-timed. Re-time
//! the walk and the sprung tail re-follows it, with nothing to re-bake.
//!
//! # Why the same maths as the bakers
//!
//! The spring and the wiggle here are the *same* solvers the bakers use
//! ([`buzz_physics`], [`buzz_rig::follow_through`]). "Live" and "bake to
//! keyframes" are two deliveries of one calculation: the baker writes it down
//! once; the modifier runs it every draw. Keeping one solver behind both is why
//! a live spring and a baked one produce the identical motion.
//!
//! The data lives here; the evaluation (and its cache, since a spring must be
//! integrated forward across the whole span) lives on [`crate::Scene`].

/// One live effect on an object, evaluated at draw time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Modifier {
    /// A deterministic wandering offset on the object's placement — idle sway,
    /// a breeze, a handheld shake. Stateless: the offset at a frame depends only
    /// on the object and the frame.
    Wiggle { amplitude: f64, frequency: f64 },
    /// Damped-spring follow-through on the bone chain rooted at `root` (that bone
    /// and everything below it), driven by the object's keyed motion and — when
    /// `coupling` is above zero — by the whole body's movement. Rigs only.
    Spring {
        root: usize,
        stiffness: f64,
        damping: f64,
        coupling: f64,
    },
    /// Turn the object to face a point in stage space — eyes and heads that
    /// track a target. Its own +x axis is aimed at `(x, y)`.
    LookAt { x: f64, y: f64 },
    /// Stretch the object along its direction of motion and squash it across,
    /// by `amount` per unit of speed. Volume-preserving: a fast move thins and
    /// lengthens the drawing, the oldest trick for selling weight and speed.
    AutoSquashStretch { amount: f64 },
    /// **Breathing.** The chest rises and falls: the drawing grows a little
    /// taller and wider about its own feet, in and out, forever.
    ///
    /// # Why a character needs one
    ///
    /// A held pose in animation is never *still*. A drawing that does not move
    /// between two keys reads as a picture of a character rather than as a
    /// character standing there, and the cheapest thing that fixes it — the
    /// thing every animator draws by hand on a hold — is a breath. It is two
    /// per cent of scale and nobody ever notices it consciously; they notice
    /// its absence immediately.
    ///
    /// `rate` is in **breaths per minute** — twelve to sixteen at rest, thirty
    /// and up after running — and `depth` scales the whole thing, `1.0` being
    /// a comfortable resting breath.
    ///
    /// Anchored at the bottom of the drawing, so the feet stay on the ground
    /// and the motion goes into the chest, which is where a breath belongs.
    /// The phase is seeded from the object's id, so a crowd does not breathe
    /// in unison — which is the one thing that would make it visible.
    Breathe { rate: f64, depth: f64 },
    /// **Blinking.** The eye shuts and opens again, every few seconds, for ever.
    ///
    /// # Why a character needs one
    ///
    /// [`Self::Breathe`] is the first thing that stops a held drawing reading
    /// as a picture; a blink is the second, and on a face it is the larger of
    /// the two. An audience does not consciously see a blink either, but a
    /// character who holds a stare for eight seconds while talking is
    /// unnerving in a way nobody can name — which is exactly the failure a
    /// puppet built for limited animation falls into, because its eyes are one
    /// drawing that nothing ever touches.
    ///
    /// # What it is applied to
    ///
    /// **The eye artwork, not the character.** Like [`Self::Sway`] on a tree,
    /// this squashes the drawing it is given: select the eyes — one object, or
    /// the layer they live on — and the lid falls on those. Put it on a whole
    /// figure and the whole figure ducks.
    ///
    /// # The lower lid barely moves
    ///
    /// So the bottom of the drawing is held and the top travels down to meet
    /// it, which is what an eyelid does. Anchoring at the middle would pinch
    /// the eye shut from both sides at once, which reads as a wince rather
    /// than a blink.
    ///
    /// `rate` is in **blinks per minute** — twelve is a comfortable resting
    /// rate, and going much above twenty starts to read as nervousness, which
    /// is a choice rather than a default. `duration` is how long one blink
    /// takes in **seconds**; `0.16` is a real one, and at twenty-four frames a
    /// second that is four frames, which is also what an animator would draw.
    ///
    /// The interval is jittered and the phase is seeded from the object's id,
    /// so a cast does not blink in unison and no single character blinks to a
    /// metronome — see [`blink_at`], where both of those live.
    Blink { rate: f64, duration: f64 },
    /// **Turn a face without drawing another one.**
    ///
    /// # The problem this solves
    ///
    /// A drawing has no information about its own sides. Rotate a flat card in
    /// space and you get a *card* turning — the face foreshortens evenly to
    /// nothing and looks like a photograph on a swivel, because that is
    /// precisely what it is. The honest answers have always been to draw the
    /// other views ([`crate::Turnaround`]) or to accept the character never
    /// turning.
    ///
    /// There is a third answer, and it is the one every 2D puppet in
    /// television has used for forty years: **do not rotate the drawing, move
    /// what is on it.** A head is roughly a cylinder, so a feature at some
    /// distance from the centre line sits at a known angle around it. Turn the
    /// cylinder and every feature's new position falls out — the near ones
    /// sweep across quickly, the far ones crowd toward the edge and go round
    /// the back, and each one narrows by exactly the foreshortening its own
    /// angle earns. Nothing is invented, nothing is guessed, and no drawing is
    /// asked for that does not exist.
    ///
    /// # Where the angle comes from
    ///
    /// **The object's own [`crate::Spatial::rotation_y`]** — not a field here.
    /// That is the yaw an animator already keys, the tween already
    /// interpolates, and the renderer already reads to pick a turnaround view.
    /// Putting the angle on the modifier instead would have made it the one
    /// procedural motion in the program that cannot be animated, since a
    /// modifier's own settings do not tween.
    ///
    /// The modifier then **consumes** that yaw: the copy it hands back is
    /// flat. Otherwise the renderer would project the result a second time and
    /// the turn would be foreshortened twice.
    ///
    /// # A drawn view always wins
    ///
    /// If the object carries a [`crate::Turnaround`] view nearer to the
    /// current angle than its own front is, this does **nothing** and lets the
    /// renderer swap that drawing in. A profile somebody drew is better than
    /// any arithmetic, every time. What this covers is the angles between the
    /// drawings — which, on a puppet with no drawings at all, is all of them.
    ///
    /// # What it needs from the artwork
    ///
    /// **A group.** The backmost child is taken as the head form and the ones
    /// painted over it as its features, which is the order they are already in
    /// if they were drawn in the order a face is drawn. Applied to a lone
    /// shape it warps that shape's own outline instead, which turns a
    /// one-piece drawing as far as a one-piece drawing can honestly go.
    ///
    /// `round` is how much of a cylinder the drawing is treated as: `1.0` for
    /// a head, lower for something flatter, `0.0` for a signboard that should
    /// only slide.
    Turn { round: f64 },
    /// **Wind.** The drawing bends downwind from its base, in gusts.
    ///
    /// A shear rather than a rotation: the bottom stays planted and the lean
    /// grows with height, which is what a trunk does and what a rotation does
    /// not — a rotated tree pivots its roots out of the ground.
    ///
    /// `amount` is how far the top leans at a full gust, as a fraction of the
    /// drawing's own height (`0.1` is a stiff pine, `0.35` a willow); `rate` is
    /// the gust frequency in hertz, around `0.2` for a breeze.
    ///
    /// The gust is **biased downwind** rather than centred, because wind is:
    /// it lulls back towards upright and gusts one way, instead of waving the
    /// tree evenly to both sides like a metronome. Seeded from the object's id,
    /// so a row of trees planted from the same drawing does not sway as one
    /// object — which is exactly what gives a painted background away.
    Sway { amount: f64, rate: f64 },
    /// **A steady drift, wrapping.** The object travels at `(dx, dy)` document
    /// units per second, and every `span` units of travel it is back where it
    /// started.
    ///
    /// # What it is for
    ///
    /// Everything in a background that goes past rather than moves about:
    /// clouds crossing the sky, the surface of a river, a streetscape behind a
    /// window, snow across a shot. All of it is one velocity and a loop, and
    /// all of it used to be two keyframes per object per shot — which is fine
    /// until the shot is re-timed, and then it is wrong everywhere at once.
    ///
    /// # Why the wrap is a distance and not a rectangle
    ///
    /// A wrap needs to know how far to go before starting again, and the honest
    /// answer depends on the *drawing*: a cloud has to be all the way off the
    /// stage before it can come back on, or it pops in mid-frame. That is a
    /// number the thing placing the cloud knows and the modifier does not, so
    /// it is passed in. `span` of zero never wraps, which is what a one-way
    /// move across a single shot wants.
    ///
    /// The distance is measured **along the drift**, not per axis, so a
    /// diagonal drift loops once rather than beating between two periods.
    ///
    /// `phase` is how far into that loop the object already is, `0..1`, and it
    /// is the field that makes a *field* of drifting things possible. Without
    /// it five clouds on one loop are five clouds in a queue: they all start at
    /// the left edge together and cross in formation. Offsetting where each one
    /// is *placed* does not fix it — the wrap then sends the ones placed
    /// further along off the far side and holds them there for most of the
    /// loop, which is exactly what it looked like. The phase has to be inside
    /// the modulo, so it lives here.
    Drift {
        dx: f64,
        dy: f64,
        span: f64,
        phase: f64,
    },
}

impl Modifier {
    /// A short name for the status line and menus.
    pub fn label(&self) -> &'static str {
        match self {
            Modifier::Wiggle { .. } => "Wiggle",
            Modifier::Spring { .. } => "Spring",
            Modifier::LookAt { .. } => "Look At",
            Modifier::AutoSquashStretch { .. } => "Squash & Stretch",
            Modifier::Breathe { .. } => "Breathe",
            Modifier::Blink { .. } => "Blink",
            Modifier::Turn { .. } => "Turn",
            Modifier::Sway { .. } => "Sway",
            Modifier::Drift { .. } => "Drift",
        }
    }

    /// Does this modifier change the object's pose/geometry (and so needs an
    /// owned, re-posed copy), rather than only prepending a transform?
    pub fn changes_pose(&self) -> bool {
        matches!(self, Modifier::Spring { .. } | Modifier::Turn { .. })
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use buzz_geom::{Affine, Point};
use buzz_physics::{Spring, Wiggle, wiggle_at};

use crate::{LayerId, Object, ObjectId, ObjectKind, Scene};

/// **One breath, in `-1..=1`.**
///
/// Not a sine. A breath is not symmetric: the chest fills quickly and empties
/// slowly, and a pure sine reads as a machine — which is the difference
/// between a character breathing and a drawing pulsing. A second harmonic at a
/// third of the amplitude sharpens the rise and lengthens the fall, which is
/// the shape of the real thing and costs one more `sin`.
///
/// `rate` is in breaths per minute; `seed` is the object's id, and only moves
/// the phase, so a crowd breathes at the same rate without breathing together.
fn breath_at(seed: u64, rate: f64, t_seconds: f64) -> f64 {
    use std::f64::consts::TAU;
    let per_second = rate.clamp(0.5, 120.0) / 60.0;
    // A stable phase per object, from the same hash the wiggle uses.
    let phase = ((splitmix64(seed ^ 0xB2EA_7115) as f64) / u64::MAX as f64) * TAU;
    let a = TAU * per_second * t_seconds + phase;
    (a.sin() + 0.33 * (2.0 * a).sin()) / 1.33
}

/// **How open an eye is: `1.0` open, `0.0` shut.**
///
/// Unlike the breath and the gust this is not a wave. A blink is a rare, fast
/// event on an eye that is otherwise simply open, so this returns exactly
/// `1.0` for the great majority of every second and dips for a fraction of one
/// every few seconds.
///
/// # Why the interval is jittered rather than fixed
///
/// A blink on a metronome is worse than no blink at all: the eye becomes a
/// ticking clock in the corner of the shot. Real blinking is irregular, so
/// time is cut into slots one period long and each blink is placed at a
/// pseudo-random offset **within its own slot**, seeded from the object and
/// the slot number.
///
/// That keeps two properties that matter. It stays **deterministic in
/// `(object, frame)`** — the promise every modifier here makes, and what lets
/// a re-timed shot blink identically. And because a blink never starts so late
/// in its slot that it would run past the end of it, no slot has to know about
/// its neighbours: the whole thing is a closed-form function of the time, with
/// nothing to integrate and no state to carry.
///
/// # The shape of one blink
///
/// The lid falls faster than it lifts — about a third of the blink is the
/// close and two thirds the open, which is what a real eyelid does and what
/// separates a blink from a pulse. The triangle is then smoothed, because a
/// lid that reverses direction instantaneously at the bottom reads as a
/// flicker rather than as something with weight.
///
/// # Sometimes twice
///
/// People blink in pairs perhaps one time in six. A blink that is *always*
/// single is a regularity an audience reads as mechanical without being able
/// to say why, so roughly a sixth of them — chosen from the same seed, so it
/// is as reproducible as the rest — come as two with a short gap.
fn blink_at(seed: u64, rate: f64, duration: f64, t_seconds: f64) -> f64 {
    // Blinks per second, and the slot each one lives in.
    let per_second = rate.clamp(0.5, 240.0) / 60.0;
    let period = 1.0 / per_second;
    // A blink cannot be longer than the gap between blinks; clamping here
    // rather than at the call site means a nonsense setting degrades to a
    // permanently half-shut eye instead of dividing by something negative.
    let duration = duration.clamp(0.02, 4.0).min(period * 0.4);

    let slot = (t_seconds / period).floor();
    let slot_seed = splitmix64(seed ^ 0x51EE_D0FF ^ (slot as i64 as u64));
    let offset = (slot_seed as f64) / (u64::MAX as f64);
    let double = (splitmix64(slot_seed) as f64) / (u64::MAX as f64) < 0.17;

    // How much of the slot this blink occupies, one or two lids' worth.
    let gap = duration * 0.6;
    let span = if double {
        duration * 2.0 + gap
    } else {
        duration
    };
    let span = span.min(period);
    // Placed so it always finishes inside its own slot — see the note above.
    let start = offset * (period - span).max(0.0);

    let mut local = t_seconds - slot * period - start;
    if local < 0.0 || local >= span {
        return 1.0;
    }
    if double && local >= duration {
        // The gap between the pair, and then the second lid.
        if local < duration + gap {
            return 1.0;
        }
        local -= duration + gap;
    }
    if local >= duration {
        return 1.0;
    }

    // A triangle through the blink: down in the first third, up over the rest.
    const CLOSING: f64 = 0.35;
    let u = local / duration;
    let shut = if u < CLOSING {
        u / CLOSING
    } else {
        1.0 - (u - CLOSING) / (1.0 - CLOSING)
    };
    // Smoothstep, so the lid arrives and leaves rather than snapping.
    let shut = shut * shut * (3.0 - 2.0 * shut);
    1.0 - shut.clamp(0.0, 1.0)
}

/// **The cylinder map: where a point at `u` ends up when the head turns.**
///
/// `u` is how far across the face the point sits, `-1` at the left edge and
/// `+1` at the right. The face is treated as the front half of a cylinder seen
/// end-on, so `u = sin(angle around it)`; adding the yaw and taking the sine
/// again is the whole of the arithmetic.
///
/// Returns the new `u`, and the **local foreshortening** — how much narrower
/// the drawing is at that point, which is what stops a nose sliding across a
/// face at a constant width like a sticker.
///
/// `None` means the point has gone round the side and should not be drawn.
///
/// # The clamp on `u`
///
/// A feature sitting exactly on the silhouette edge is at ninety degrees,
/// where the foreshortening ratio divides by zero. Real artwork never quite
/// gets there, but a rounding error can, so the angle is held just inside the
/// edge; the visible consequence is nothing at all.
fn turn_at(u: f64, yaw: f64, round: f64) -> Option<(f64, f64)> {
    use std::f64::consts::FRAC_PI_2;
    /// Just inside the silhouette, where the arithmetic is still finite.
    const EDGE: f64 = 0.995;

    let round = round.clamp(0.0, 1.0);
    // `round` of zero is a flat board: it slides and never foreshortens, which
    // is the right answer for a signpost and a useless one for a head.
    let u = (u.clamp(-1.0, 1.0) * round).clamp(-EDGE, EDGE);
    let theta = u.asin();
    let turned = theta + yaw;
    if turned.abs() >= FRAC_PI_2 * EDGE {
        // Round the back of the head. Hidden rather than clamped to the edge:
        // features piling up on the silhouette is the single most obvious way
        // a puppet turn gives itself away.
        return None;
    }
    // The ratio of the two cosines is the derivative of the map — how much the
    // drawing is compressed *here*, as against uniformly.
    let squeeze = turned.cos() / theta.cos().max(1.0 - EDGE * EDGE);
    Some((turned.sin(), squeeze.clamp(0.05, 4.0)))
}

/// **Turn `object` in place**, consuming its yaw.
///
/// The copy handed back is flat: the yaw has been spent moving the artwork, and
/// leaving it on would have the renderer project the result a second time.
fn turn_object(object: &mut Object, yaw: f64, round: f64) {
    let extent = object.local_bounds();
    if extent.width() <= 0.0 {
        return;
    }
    let cx = extent.center().x;
    let r = extent.width() * 0.5;

    match &mut object.kind {
        // **A group is a face with its parts separated**, which is what makes a
        // real turn possible: each feature is carried round the cylinder on its
        // own, rather than the whole picture being squeezed as one.
        ObjectKind::Group(children) => {
            let form_shift = r * yaw.sin() * FORM_TRAVEL;
            // A skull does narrow a little as it comes round; much less than a
            // flat card would, which is the whole complaint against rotating
            // the drawing.
            let form_narrow = 1.0 - 0.12 * (1.0 - yaw.cos());

            let mut kept: Vec<Arc<Object>> = Vec::with_capacity(children.len());
            for (index, child) in children.iter().enumerate() {
                let here = child.bounds().center();

                // **The backmost child is the head, the rest are on it.** That
                // is the order a face is drawn in and the order the Layers
                // panel already holds, so it needs no naming, no slots and no
                // setup — see `Modifier::Turn`.
                if index == 0 {
                    let mut form = (**child).clone();
                    form.transform = Affine::translate((cx + form_shift, 0.0))
                        * Affine::scale_non_uniform(form_narrow, 1.0)
                        * Affine::translate((-cx, 0.0))
                        * form.transform;
                    kept.push(Arc::new(form));
                    continue;
                }

                let Some((u, squeeze)) = turn_at((here.x - cx) / r, yaw, round) else {
                    // Round the back of the head: dropped rather than drawn at
                    // the edge. This is the far eye going out of sight.
                    continue;
                };

                // **How much of the head does this part cover?**
                //
                // An eye or a nose is a mark *on* the surface and travels the
                // full way round it. Hair, a hat, a helmet, a beard is a mass
                // *wrapping* the form, and moving one of those like a nose
                // slides it off the skull and bares the forehead — which is
                // exactly what the first version of this did, and what the
                // figure in the guide caught.
                //
                // The tell is width: nothing that spans most of the head is a
                // point on it. Squared, so that only the genuinely wide parts
                // are affected and an eye at a quarter of the width is left
                // very nearly alone.
                let span = (child.bounds().width() / extent.width()).clamp(0.0, 1.0);
                let mass = span * span;

                let as_feature = cx + r * u;
                let as_form = here.x + form_shift;
                let moved = as_feature * (1.0 - mass) + as_form * mass;
                let squeeze = squeeze * (1.0 - mass) + form_narrow * mass;

                let mut feature = (**child).clone();
                // Narrowed about its own middle and then carried to where the
                // turn puts it. Only across: a yaw moves nothing vertically.
                feature.transform = Affine::translate((moved, 0.0))
                    * Affine::scale_non_uniform(squeeze, 1.0)
                    * Affine::translate((-here.x, 0.0))
                    * feature.transform;
                kept.push(Arc::new(feature));
            }
            *children = kept;
        }
        // **One drawing, no parts.** There is nothing to carry round
        // separately, so the outline itself is warped: the drawing turns as far
        // as a single drawing honestly can, and the guide says to separate the
        // features when that is not far enough.
        ObjectKind::Shape(shape) => {
            let map = |p: Point| {
                turn_at((p.x - cx) / r, yaw, round)
                    .map(|(u, _)| Point::new(cx + r * u, p.y))
                    // A point that has gone round the side has nowhere to be on
                    // a path that must stay closed, so it is held at the
                    // silhouette. Dropping it would tear the outline open.
                    .unwrap_or_else(|| {
                        Point::new(cx + r * yaw.signum() * 0.995, p.y)
                    })
            };
            shape.path = map_path(&shape.path, map);
        }
        // A rig or an instance has no artwork here to move: its parts live
        // behind an armature or in the library, and guessing at either from
        // this side would be worse than doing nothing.
        _ => return,
    }

    object.spatial.rotation_y = 0.0;
}

/// Rebuild a path with every point put through `f`.
///
/// Control points go through the same map as the ends, which is what keeps a
/// curve a curve: mapping only the on-curve points would straighten every
/// bulge in the drawing.
fn map_path(path: &buzz_geom::BezPath, f: impl Fn(Point) -> Point) -> buzz_geom::BezPath {
    use buzz_geom::PathEl;
    let mut out = buzz_geom::BezPath::new();
    for el in path.elements() {
        out.push(match *el {
            PathEl::MoveTo(p) => PathEl::MoveTo(f(p)),
            PathEl::LineTo(p) => PathEl::LineTo(f(p)),
            PathEl::QuadTo(a, p) => PathEl::QuadTo(f(a), f(p)),
            PathEl::CurveTo(a, b, p) => PathEl::CurveTo(f(a), f(b), f(p)),
            PathEl::ClosePath => PathEl::ClosePath,
        });
    }
    out
}

/// How much of the features' travel the head form itself takes up.
///
/// A face whose features slide across a silhouette that never moves reads as a
/// mask with things sliding on it. Giving the form a fraction of the same
/// movement reads as a head turning as one mass. It is deliberately small: at
/// a half the two would move together and nothing would look turned at all.
const FORM_TRAVEL: f64 = 0.15;

/// **One gust of wind, in about `-0.3..=1.0`.**
///
/// Biased downwind, because wind is: it lulls back towards upright and pushes
/// one way, rather than waving a tree evenly to both sides. The wander itself
/// is the wiggle's own fractal sum of sines — three octaves, so the branch has
/// a flutter on top of the gust rather than a single frequency, which is what
/// stops a row of trees looking like windscreen wipers.
fn gust_at(seed: u64, rate: f64, t_seconds: f64) -> f64 {
    let wander = buzz_physics::wiggle_at(
        buzz_physics::Wiggle {
            amplitude: 1.0,
            frequency: rate.clamp(0.01, 20.0),
        },
        seed ^ 0x5EED_1A15,
        t_seconds,
    );
    0.35 + 0.65 * wander.dx
}

/// SplitMix64's finalizer, for a stable phase per object. The same mixer
/// `buzz_physics` seeds its wiggles with, so two procedural motions on one
/// object do not share a phase by accident.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The spring cache's table: a full modified pose sequence per `(object, chain
/// root)`, all built for one document revision.
pub(crate) type SpringTable = HashMap<(ObjectId, usize), Arc<Vec<Vec<f64>>>>;

/// The result of evaluating an object's modifiers at a frame.
///
/// `prepend` is composed onto the object's own transform in stage space (a
/// wiggle offset); `object` is a re-posed owned copy when a pose modifier (a
/// spring) changed the geometry, or `None` when only the transform moved and the
/// original — with its stable `Arc` identity — can be drawn.
#[derive(Debug, Clone)]
pub struct ModifierEval {
    pub prepend: Affine,
    pub object: Option<Object>,
}

impl Scene {
    /// Evaluate an object's live modifiers at `frame`.
    ///
    /// Returns `None` for the overwhelming majority of objects, which have no
    /// modifiers — the render path then draws them exactly as before. Otherwise
    /// returns the transform to prepend and, if a spring re-posed the rig, the
    /// modified object to draw in place of the original.
    pub fn modified_object_at(
        &self,
        layer: LayerId,
        object: &Object,
        at: impl crate::time::AtTime,
    ) -> Option<ModifierEval> {
        if object.modifiers.is_empty() {
            return None;
        }
        // A wiggle is a function of *time* and shakes fast enough to smear
        // within one frame, so it is sampled continuously. A spring's pose and
        // a squash's look-back are computed frame by frame and are held at the
        // frame the shutter opened on — see the note on each below.
        let time = at.as_time();
        let frame = at.frame();
        let fps = self.stage().frame_rate.max(1.0);
        let mut prepend = Affine::IDENTITY;
        let mut posed: Option<Object> = None;

        for modifier in &object.modifiers {
            match *modifier {
                Modifier::Wiggle {
                    amplitude,
                    frequency,
                } => {
                    let sample = wiggle_at(
                        Wiggle {
                            amplitude,
                            frequency,
                        },
                        object.id.0,
                        // Continuous: this is what lets a shake blur.
                        time / fps,
                    );
                    prepend = Affine::translate((sample.dx, sample.dy)) * prepend;
                }
                Modifier::Spring {
                    root,
                    stiffness,
                    damping,
                    coupling,
                } => {
                    // Integrated frame by frame, so there is no state between two of
                    // them to ask for: the pose is held across the shutter.
                    let spring = Spring { stiffness, damping };
                    if let Some(seq) = self.spring_sequence(layer, object, root, spring, coupling, fps)
                    {
                        let index = (frame as usize).min(seq.len().saturating_sub(1));
                        if let Some(pose) = seq.get(index) {
                            let target = posed.get_or_insert_with(|| object.clone());
                            if let ObjectKind::Armature(rig) = &mut target.kind {
                                rig.armature.set_pose(pose);
                            }
                        }
                    }
                }
                Modifier::LookAt { x, y } => {
                    // Turn the object about its own anchor so its +x axis points
                    // at the target, on top of whatever rotation it already has.
                    let base = object.transform;
                    let anchor = base.translation();
                    let coeffs = base.as_coeffs();
                    let base_angle = coeffs[1].atan2(coeffs[0]);
                    let desired = (y - anchor.y).atan2(x - anchor.x);
                    let turn = desired - base_angle;
                    prepend = Affine::translate(anchor)
                        * Affine::rotate(turn)
                        * Affine::translate(-anchor)
                        * prepend;
                }
                Modifier::AutoSquashStretch { amount } => {
                    // Speed from the previous frame's placement of this same
                    // object — one look-back, no integration.
                    let here = object.transform.translation();
                    let before = if frame == 0 {
                        here
                    } else {
                        self.layers()
                            .get(layer)
                            .and_then(|l| {
                                l.frames
                                    .resolved_at(frame - 1)
                                    .iter()
                                    .find(|o| o.id == object.id)
                                    .map(|o| o.transform.translation())
                            })
                            .unwrap_or(here)
                    };
                    let velocity = here - before;
                    let speed = velocity.hypot();
                    if speed > 1e-6 {
                        // Stretch along motion, squash across it; clamped so a
                        // teleport does not turn the drawing into a needle.
                        let stretch = (1.0 + amount * speed).clamp(0.25, 4.0);
                        let heading = velocity.y.atan2(velocity.x);
                        let squash = Affine::rotate(heading)
                            * Affine::scale_non_uniform(stretch, 1.0 / stretch)
                            * Affine::rotate(-heading);
                        prepend = Affine::translate(here)
                            * squash
                            * Affine::translate(-here)
                            * prepend;
                    }
                }
                Modifier::Breathe { rate, depth } => {
                    // Continuous in time, like the wiggle and for the same
                    // reason: a breath is slow, and sampling it per frame
                    // rather than per shutter would step it.
                    let bounds = object.bounds();
                    if bounds.width() > 0.0 && bounds.height() > 0.0 {
                        let s = breath_at(object.id.0, rate, time / fps);
                        let depth = depth.clamp(0.0, 4.0);
                        // **Two per cent, and taller than it is wider.** A
                        // breath you can measure is a breath the audience can
                        // see, and a character that visibly inflates reads as a
                        // balloon. The chest fills, so both axes grow; it fills
                        // upwards more than outwards, so y grows about twice as
                        // much as x.
                        let sy = 1.0 + depth * 0.022 * s;
                        let sx = 1.0 + depth * 0.010 * s;
                        // The feet, not the middle: a breath must not lift the
                        // character off the ground.
                        let feet = buzz_geom::Point::new(bounds.center().x, bounds.y1);
                        prepend = Affine::translate(feet.to_vec2())
                            * Affine::scale_non_uniform(sx, sy)
                            * Affine::translate(-feet.to_vec2())
                            * prepend;
                    }
                }
                Modifier::Blink { rate, duration } => {
                    // Continuous in time, like the breath, and for a sharper
                    // version of the same reason: a blink lasts three or four
                    // frames, and holding it across an open shutter is what
                    // lets a motion-blurred exposure catch the lid in mid-fall
                    // rather than either fully open or fully shut.
                    let bounds = object.bounds();
                    if bounds.width() > 0.0 && bounds.height() > 0.0 {
                        let open = blink_at(object.id.0, rate, duration, time / fps);
                        // **Never quite to nothing.** A drawing scaled to zero
                        // height has no area to rasterise: it would vanish and
                        // pop back rather than close. A twentieth of the eye
                        // left is a shut eye with a lid still drawn across it.
                        let sy = 0.05 + 0.95 * open.clamp(0.0, 1.0);
                        // The lower lid barely moves, so the bottom edge is
                        // held and the top travels down to meet it.
                        let lid = bounds.y1;
                        prepend = Affine::translate((0.0, lid))
                            * Affine::scale_non_uniform(1.0, sy)
                            * Affine::translate((0.0, -lid))
                            * prepend;
                    }
                }
                Modifier::Turn { round } => {
                    // The angle is the object's own keyed yaw, not a setting
                    // here — see `Modifier::Turn`. A head that is not turned
                    // costs nothing and is left exactly alone.
                    let yaw = object.spatial.rotation_y;
                    if yaw.abs() < 1e-6 {
                        continue;
                    }
                    // **A drawing somebody made beats one worked out.** If a
                    // turnaround view is nearer to this angle than the front
                    // is, stand aside and let the renderer swap it in.
                    if object.turnaround.view_at(yaw).is_some() {
                        continue;
                    }
                    let target = posed.get_or_insert_with(|| object.clone());
                    turn_object(target, yaw, round);
                }
                Modifier::Drift {
                    dx,
                    dy,
                    span,
                    phase,
                } => {
                    // Continuous in time, like the wiggle: a drift is smooth,
                    // and sampling it per frame rather than per shutter would
                    // step a slow one visibly.
                    let seconds = time / fps;
                    let speed = (dx * dx + dy * dy).sqrt();
                    if speed > 1e-9 {
                        // Wrapped along the drift, so a background loops — and
                        // the head start goes *inside* the wrap, so a cloud
                        // that begins three quarters of the way along still
                        // spends the same share of its loop on screen as one
                        // that begins at the edge.
                        let travelled = if span > 1e-9 {
                            (speed * seconds + phase * span).rem_euclid(span)
                        } else {
                            speed * seconds
                        };
                        let step = travelled / speed;
                        prepend = Affine::translate((dx * step, dy * step)) * prepend;
                    }
                }
                Modifier::Sway { amount, rate } => {
                    let bounds = object.bounds();
                    if bounds.width() > 0.0 && bounds.height() > 0.0 {
                        let gust = gust_at(object.id.0, rate, time / fps);
                        // How far the *top* of the drawing leans, in document
                        // units: a fraction of its own height, so one setting
                        // suits a sapling and a full-grown oak.
                        let lean = amount.clamp(-2.0, 2.0) * gust * bounds.height();
                        // Shear: the displacement grows with height above the
                        // base, so the base itself does not move. `k` is that
                        // displacement per unit of height.
                        let k = lean / bounds.height();
                        let base = bounds.y1;
                        // A bend shortens what it bends — the top of a leaning
                        // trunk is nearer the ground than the top of an upright
                        // one. Without it the crown swings along an arc that is
                        // visibly wrong at the extremes, and the tree looks
                        // rubbery rather than woody.
                        let shorten = 1.0 / (1.0 + k * k).sqrt();
                        prepend = Affine::translate((0.0, base))
                            * Affine::new([1.0, 0.0, -k, shorten, 0.0, 0.0])
                            * Affine::translate((0.0, -base))
                            * prepend;
                    }
                }
            }
        }

        Some(ModifierEval {
            prepend,
            object: posed,
        })
    }

    /// The whole modified pose sequence for a spring on `object`'s chain, built
    /// once per document revision and cached. `None` if the object is not a rig
    /// or the chain root is out of range.
    fn spring_sequence(
        &self,
        layer: LayerId,
        object: &Object,
        root: usize,
        spring: Spring,
        coupling: f64,
        fps: f64,
    ) -> Option<Arc<Vec<Vec<f64>>>> {
        let key = (object.id, root);
        let revision = self.revision;

        // Hit: the cache is for this revision and already holds this chain.
        if let Ok(cache) = self.modifier_cache.read()
            && let Some((cached_revision, table)) = &*cache
            && *cached_revision == revision
            && let Some(seq) = table.get(&key)
        {
            return Some(seq.clone());
        }

        // Miss: reconstruct the primary motion off the timeline and integrate.
        let ObjectKind::Armature(data) = &object.kind else {
            return None;
        };
        let topology = data.armature.clone();
        if root >= topology.bones.len() {
            return None;
        }
        let layers = self.layers();
        let timeline = &layers.get(layer)?.frames;
        let span = timeline.length().max(1);

        let mut primary = Vec::with_capacity(span as usize);
        let mut world = Vec::with_capacity(span as usize);
        for g in 0..span {
            let resolved = timeline.resolved_at(g);
            let here = resolved.iter().find(|o| o.id == object.id);
            primary.push(
                here.and_then(|o| match &o.kind {
                    ObjectKind::Armature(r) => Some(r.armature.pose()),
                    _ => None,
                })
                .unwrap_or_else(|| topology.pose()),
            );
            world.push(here.map_or(Affine::IDENTITY, |o| o.transform));
        }

        let modified = if coupling > 0.0 {
            buzz_rig::follow_through_coupled(&topology, root, spring, &primary, &world, coupling, fps)
        } else {
            buzz_rig::follow_through(&topology, root, spring, &primary, fps)
        };
        let arc = Arc::new(modified);

        if let Ok(mut cache) = self.modifier_cache.write() {
            match &mut *cache {
                Some((cached_revision, table)) if *cached_revision == revision => {
                    table.insert(key, arc.clone());
                }
                // Stale or empty: start a fresh table for this revision.
                slot => {
                    let mut table = SpringTable::new();
                    table.insert(key, arc.clone());
                    *slot = Some((revision, table));
                }
            }
        }
        Some(arc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArmatureData, LayerKind, Object, ShapeData, Tween};
    use buzz_geom::{Affine, Point, Rect, Shape as _};
    use buzz_rig::{Armature, Bone};
    use peniko::Color;

    fn rig_scene() -> (Scene, LayerId, ObjectId) {
        let mut arm = Armature {
            root: Point::ORIGIN,
            bones: Vec::new(),
        };
        for i in 0..4 {
            let parent = if i == 0 { None } else { Some(i - 1) };
            arm.bones.push(Bone::new(format!("b{i}"), parent, 40.0, 0.0));
        }
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Rig", LayerKind::Normal);
        let object = Object {
            kind: ObjectKind::Armature(ArmatureData::new(arm)),
            ..Object::shape(
                ObjectId(1),
                ShapeData::filled(Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9), Color::WHITE),
            )
        };
        let id = scene.add_object(layer, object).unwrap();
        (scene, layer, id)
    }

    /// Swing the base bone to 0.8 by frame 12, hold to `hold_to`.
    fn animate_base(scene: &mut Scene, layer: LayerId, id: ObjectId, hold_to: u32) {
        scene.update_layer(layer, |l| {
            if l.frames.length() <= 12 {
                l.frames.insert_frame(12);
            }
        });
        scene.ensure_keyframe(layer, 12);
        scene.update_object_at(12, id, |o| {
            if let ObjectKind::Armature(r) = &mut o.kind {
                r.armature.set_pose(&[0.8, 0.0, 0.0, 0.0]);
            }
        });
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::motion());
            if l.frames.length() <= hold_to {
                l.frames.insert_frame(hold_to);
            }
        });
    }

    fn resolved(scene: &Scene, layer: LayerId, id: ObjectId, frame: u32) -> Object {
        scene
            .layers()
            .get(layer)
            .unwrap()
            .frames
            .resolved_at(frame)
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("no object at frame {frame}"))
    }

    #[test]
    fn an_object_without_modifiers_evaluates_to_nothing() {
        let (scene, layer, id) = rig_scene();
        let obj = resolved(&scene, layer, id, 0);
        assert!(scene.modified_object_at(layer, &obj, 0).is_none());
    }

    #[test]
    fn a_wiggle_modifier_offsets_by_the_noise_value() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Sign", LayerKind::Normal);
        let mut object = Object::shape(
            ObjectId(9),
            ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
        );
        object.modifiers.push(Modifier::Wiggle {
            amplitude: 10.0,
            frequency: 2.0,
        });
        let id = scene.add_object(layer, object).unwrap();
        // Hold the object across a range so frame 5 resolves.
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(30);
        });

        let obj = resolved(&scene, layer, id, 5);
        let eval = scene.modified_object_at(layer, &obj, 5).expect("a wiggle");
        assert!(eval.object.is_none(), "a wiggle should not re-pose, only offset");

        let fps = scene.stage().frame_rate.max(1.0);
        let want = buzz_physics::wiggle_at(
            buzz_physics::Wiggle {
                amplitude: 10.0,
                frequency: 2.0,
            },
            id.0,
            5.0 / fps,
        );
        let t = eval.prepend.translation();
        assert!((t.x - want.dx).abs() < 1e-9 && (t.y - want.dy).abs() < 1e-9);
    }

    #[test]
    fn a_live_spring_matches_the_solver() {
        let (mut scene, layer, id) = rig_scene();
        animate_base(&mut scene, layer, id, 47);
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Spring {
                root: 1,
                stiffness: 80.0,
                damping: 6.0,
                coupling: 0.0,
            });
        });

        // Reconstruct the primary the same way the evaluator does, and run the
        // solver directly: the live pose must equal it frame for frame.
        let topology = {
            let obj = resolved(&scene, layer, id, 0);
            match obj.kind {
                ObjectKind::Armature(d) => d.armature,
                _ => unreachable!(),
            }
        };
        let span = scene.layers().get(layer).unwrap().frames.length();
        let primary: Vec<Vec<f64>> = (0..span)
            .map(|g| match resolved(&scene, layer, id, g).kind {
                ObjectKind::Armature(d) => d.armature.pose(),
                _ => unreachable!(),
            })
            .collect();
        let fps = scene.stage().frame_rate.max(1.0);
        let expected = buzz_rig::follow_through(
            &topology,
            1,
            buzz_physics::Spring {
                stiffness: 80.0,
                damping: 6.0,
            },
            &primary,
            fps,
        );

        for frame in [4u32, 8, 20, 40] {
            let obj = resolved(&scene, layer, id, frame);
            let eval = scene.modified_object_at(layer, &obj, frame).unwrap();
            let live = match eval.object.unwrap().kind {
                ObjectKind::Armature(d) => d.armature.pose(),
                _ => unreachable!(),
            };
            assert_eq!(live, expected[frame as usize], "frame {frame} differs from the solver");
        }
    }

    /// A square standing on the ground, for the three modifiers that measure
    /// themselves against the drawing's own feet.
    fn standing_square(scene: &mut Scene, id: u64) -> (LayerId, ObjectId) {
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let object = Object::shape(
            ObjectId(id),
            // Top at y = 0, base at y = 100, a hundred wide.
            ShapeData::filled(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9), Color::WHITE),
        );
        let placed = scene.add_object(layer, object).unwrap();
        scene.update_layer(layer, |l| {
            if l.frames.length() <= 60 {
                l.frames.insert_frame(60);
            }
        });
        (layer, placed)
    }

    /// **Breathing moves the chest and leaves the feet alone.**
    ///
    /// Both halves matter. A breath that lifted the whole drawing would be a
    /// character bobbing off the floor, which is worse than not breathing.
    #[test]
    fn breathing_raises_the_chest_and_keeps_the_feet_down() {
        let mut scene = Scene::empty();
        let (layer, id) = standing_square(&mut scene, 11);
        scene.update_object_across(0, 60, id, |o| {
            o.modifiers.push(Modifier::Breathe {
                rate: 14.0,
                depth: 1.0,
            });
        });

        let mut tops = Vec::new();
        for frame in 0..60u32 {
            let obj = resolved(&scene, layer, id, frame);
            let eval = scene.modified_object_at(layer, &obj, frame).unwrap();
            let feet = eval.prepend * buzz_geom::Point::new(50.0, 100.0);
            assert!(
                (feet.y - 100.0).abs() < 1e-9,
                "frame {frame}: the feet moved to {}",
                feet.y
            );
            tops.push((eval.prepend * buzz_geom::Point::new(50.0, 0.0)).y);
        }

        let lo = tops.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = tops.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            hi - lo > 1.0,
            "the chest barely moved over two and a half seconds: {lo}..{hi}"
        );
        // And not by so much that it reads as a balloon.
        assert!(hi - lo < 12.0, "that is not breathing, it is inflating: {lo}..{hi}");
    }

    /// Openness sampled finely over `seconds`, for the blink tests. A blink is
    /// four frames long, so anything sampled per frame would step straight over
    /// most of one.
    fn blink_samples(seed: u64, rate: f64, duration: f64, seconds: f64) -> Vec<f64> {
        let step = 1.0 / 240.0;
        let n = (seconds / step) as usize;
        (0..n)
            .map(|i| blink_at(seed, rate, duration, i as f64 * step))
            .collect()
    }

    /// **An eye is open almost all of the time.**
    ///
    /// This is the property that separates a blink from every other modifier
    /// here: the breath and the gust are always moving, and a blink is a flat
    /// line with rare notches in it. Get this wrong and the character is
    /// squinting through the whole shot.
    #[test]
    fn an_eye_is_open_almost_all_of_the_time() {
        let s = blink_samples(7, 12.0, 0.16, 60.0);
        let open = s.iter().filter(|v| **v > 0.99).count() as f64 / s.len() as f64;
        assert!(
            open > 0.9,
            "the eye is only fully open {:.0}% of the time",
            open * 100.0
        );
    }

    /// **And it does shut.** An eye that only ever half-closes reads as a
    /// twitch; the lid has to arrive.
    #[test]
    fn a_blink_closes_the_eye_and_opens_it_again() {
        let s = blink_samples(7, 12.0, 0.16, 60.0);
        let shut = s.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(shut < 0.05, "the lid never arrived: the closest it came was {shut}");
        assert!(
            s.first().copied().unwrap() > 0.99 && s.last().copied().unwrap() > 0.99,
            "the eye did not come back open"
        );
    }

    /// **Twelve a minute means about twelve a minute.** The interval is
    /// jittered, so this checks the count rather than the spacing.
    #[test]
    fn the_rate_is_blinks_per_minute() {
        let s = blink_samples(3, 12.0, 0.16, 60.0);
        // Count falling edges: each blink shuts once, and a double blink shuts
        // twice, so the honest range is twelve to about fourteen.
        let closes = s.windows(2).filter(|w| w[0] > 0.5 && w[1] <= 0.5).count();
        assert!(
            (10..=16).contains(&closes),
            "twelve a minute produced {closes} in a minute"
        );
    }

    /// **A cast does not blink in unison.** The one thing that would make the
    /// whole effect visible, and the reason the phase is seeded per object.
    #[test]
    fn two_characters_do_not_blink_together() {
        let a = blink_samples(11, 12.0, 0.16, 60.0);
        let b = blink_samples(12, 12.0, 0.16, 60.0);
        let together = a
            .iter()
            .zip(&b)
            .filter(|(x, y)| **x < 0.5 && **y < 0.5)
            .count();
        let each = a.iter().filter(|v| **v < 0.5).count();
        assert!(each > 0, "the first eye never blinked");
        assert!(
            (together as f64) < 0.2 * each as f64,
            "two objects blinked together {together} samples out of {each}"
        );
    }

    /// **Nor to a metronome.** Evenly spaced blinks turn the eye into a
    /// ticking clock in the corner of the shot.
    #[test]
    fn blinks_are_not_evenly_spaced() {
        let s = blink_samples(5, 12.0, 0.16, 180.0);
        let starts: Vec<usize> = s
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] > 0.5 && w[1] <= 0.5)
            .map(|(i, _)| i)
            .collect();
        assert!(starts.len() > 8, "not enough blinks to judge: {}", starts.len());
        let gaps: Vec<f64> = starts.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        let spread = gaps
            .iter()
            .map(|g| (g - mean).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            spread > 0.15 * mean,
            "the gaps between blinks are all but identical: mean {mean:.0}, spread {spread:.0}"
        );
    }

    /// **The same object blinks the same way twice.** Determinism in
    /// `(object, frame)` is what lets a shot be re-timed, re-rendered and
    /// re-exported without the eyes changing.
    #[test]
    fn blinking_is_reproducible() {
        assert_eq!(
            blink_samples(9, 12.0, 0.16, 30.0),
            blink_samples(9, 12.0, 0.16, 30.0)
        );
    }

    /// **The lid falls; the eye does not shrink.** The bottom edge is held, so
    /// the drawing closes downward the way an eyelid does rather than pinching
    /// shut about its middle, which reads as a wince.
    #[test]
    fn the_lid_falls_and_the_lower_lid_stays_put() {
        let mut scene = Scene::empty();
        let (layer, id) = standing_square(&mut scene, 21);
        scene.update_object_across(0, 60, id, |o| {
            o.modifiers.push(Modifier::Blink {
                rate: 120.0,
                duration: 0.16,
            });
        });

        // Sampled between frames, not on them: a blink is four frames long and
        // a per-frame sample steps over most of one.
        let mut tops = Vec::new();
        for step in 0..1200 {
            let time = step as f64 * 0.05;
            let obj = resolved(&scene, layer, id, time as u32);
            let eval = scene.modified_object_at(layer, &obj, time).unwrap();
            let bottom = eval.prepend * buzz_geom::Point::new(50.0, 100.0);
            assert!(
                (bottom.y - 100.0).abs() < 1e-9,
                "at {time}: the lower lid moved to {}",
                bottom.y
            );
            tops.push((eval.prepend * buzz_geom::Point::new(50.0, 0.0)).y);
        }

        let hi = tops.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            hi > 90.0,
            "the top never came down to meet the bottom: it reached {hi} of 100"
        );
        // And it spends most of the shot open, at the top of the drawing.
        let open = tops.iter().filter(|y| **y < 1.0).count();
        assert!(
            open * 2 > tops.len(),
            "the eye was shut or half-shut for most of the shot"
        );
    }

    // -- the head turn ------------------------------------------------------

    /// A face as a group: the form first (backmost), then a left eye, a nose on
    /// the centre line, and a right eye. Returns the layer and the object.
    fn face(scene: &mut Scene, id: u64) -> (LayerId, ObjectId) {
        let layer = scene.add_layer("Head", LayerKind::Normal);
        let part = |n: u64, x0: f64, x1: f64| {
            Arc::new(Object::shape(
                ObjectId(n),
                ShapeData::filled(
                    Rect::new(x0, 40.0, x1, 60.0).to_path(1e-9),
                    Color::WHITE,
                ),
            ))
        };
        let children = vec![
            // The form: the whole head, 0..100.
            Arc::new(Object::shape(
                ObjectId(id * 10),
                ShapeData::filled(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9), Color::WHITE),
            )),
            part(id * 10 + 1, 20.0, 30.0),  // left eye,  centre 25
            part(id * 10 + 2, 47.0, 53.0),  // nose,      centre 50
            part(id * 10 + 3, 70.0, 80.0),  // right eye, centre 75
        ];
        let placed = scene
            .add_object(layer, Object::group(ObjectId(id), children))
            .expect("a face");
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(30);
        });
        (layer, placed)
    }

    /// The centres of a turned face's parts, form first.
    fn part_centres(scene: &Scene, layer: LayerId, id: ObjectId) -> Vec<f64> {
        let object = resolved(scene, layer, id, 0);
        let eval = scene
            .modified_object_at(layer, &object, 0u32)
            .expect("a modifier");
        let drawn = eval.object.expect("the turn re-poses, so it owns a copy");
        match &drawn.kind {
            ObjectKind::Group(children) => {
                children.iter().map(|c| c.bounds().center().x).collect()
            }
            _ => unreachable!("the fixture is a group"),
        }
    }

    fn turned_face(yaw: f64) -> (Scene, LayerId, ObjectId) {
        let mut scene = Scene::empty();
        let (layer, id) = face(&mut scene, 3);
        scene.update_object_across(0, 30, id, |o| {
            o.spatial.rotation_y = yaw;
            o.modifiers.push(Modifier::Turn { round: 1.0 });
        });
        (scene, layer, id)
    }

    /// **A face that is not turned is not touched.** The overwhelmingly common
    /// case, and one where any movement at all would be a bug you would find in
    /// every frame of every shot.
    #[test]
    fn a_head_at_rest_is_left_exactly_alone() {
        let (scene, layer, id) = turned_face(0.0);
        let object = resolved(&scene, layer, id, 0);
        let eval = scene.modified_object_at(layer, &object, 0u32).expect("evaluated");
        assert!(
            eval.object.is_none(),
            "an unturned head should not even be copied"
        );
        assert_eq!(eval.prepend, Affine::IDENTITY);
    }

    /// **Turning moves the features across the face**, and the middle one moves
    /// furthest. That ordering is the whole of the cylinder: a feature on the
    /// centre line is travelling straight at the viewer and sweeps fastest,
    /// while one near the silhouette is already going away and barely shifts.
    #[test]
    fn features_sweep_across_and_the_centre_one_moves_most() {
        let rest = {
            let mut scene = Scene::empty();
            let (layer, id) = face(&mut scene, 3);
            let object = resolved(&scene, layer, id, 0);
            assert!(scene.modified_object_at(layer, &object, 0u32).is_none());
            match &object.kind {
                ObjectKind::Group(c) => {
                    c.iter().map(|c| c.bounds().center().x).collect::<Vec<_>>()
                }
                _ => unreachable!(),
            }
        };

        let (scene, layer, id) = turned_face(0.5);
        let now = part_centres(&scene, layer, id);
        assert_eq!(now.len(), rest.len(), "nothing should have gone round the back yet");

        // Every feature moved the same way as the turn.
        for i in 1..now.len() {
            assert!(
                now[i] > rest[i],
                "feature {i} went the wrong way: {} to {}",
                rest[i],
                now[i]
            );
        }
        // The nose (index 2, on the centre line) outruns both eyes.
        let travel: Vec<f64> = (1..now.len()).map(|i| now[i] - rest[i]).collect();
        assert!(
            travel[1] > travel[0] && travel[1] > travel[2],
            "the centre feature should sweep furthest; travels were {travel:?}"
        );
    }

    /// **The form moves, but far less than the features.** A silhouette that
    /// never moved would read as a mask with things sliding on it; one that
    /// moved as much as the features would not read as turned at all.
    #[test]
    fn the_head_form_carries_a_fraction_of_the_turn() {
        let (scene, layer, id) = turned_face(0.5);
        let now = part_centres(&scene, layer, id);
        let form_travel = now[0] - 50.0;
        let nose_travel = now[2] - 50.0;
        assert!(form_travel > 0.0, "the form did not move at all");
        assert!(
            form_travel < nose_travel * 0.5,
            "the form moved {form_travel:.1} against the nose's {nose_travel:.1}, \
             which is too much to read as a turn"
        );
    }

    /// **Features narrow as they turn away.** Sliding a nose across a face at a
    /// constant width is the thing that makes a cheap puppet look like a
    /// sticker on a balloon.
    #[test]
    fn a_feature_foreshortens_as_it_goes_round() {
        let (scene, layer, id) = turned_face(0.6);
        let object = resolved(&scene, layer, id, 0);
        let drawn = scene
            .modified_object_at(layer, &object, 0u32)
            .expect("a modifier")
            .object
            .expect("an owned copy");
        let widths: Vec<f64> = match &drawn.kind {
            ObjectKind::Group(c) => c.iter().map(|c| c.bounds().width()).collect(),
            _ => unreachable!(),
        };
        // The nose started six units wide and is turning away from straight-on.
        assert!(
            widths[2] < 6.0,
            "the nose did not foreshorten: it is still {:.2} wide",
            widths[2]
        );
        assert!(widths[2] > 0.0, "the nose vanished rather than narrowing");
    }

    /// **A feature that goes round the back is dropped, not squashed onto the
    /// edge.** Features piling up on the silhouette is the single most obvious
    /// way a puppet turn gives itself away.
    #[test]
    fn a_far_feature_goes_out_of_sight() {
        let (scene, layer, id) = turned_face(1.2);
        let now = part_centres(&scene, layer, id);
        assert!(
            now.len() < 4,
            "at a heavy turn something should have gone round the back; {} parts remain",
            now.len()
        );
        assert!(now.len() >= 2, "the whole face disappeared");
    }

    /// **The yaw is consumed.** The renderer projects anything still carrying
    /// one, so leaving it on would foreshorten the turn a second time.
    #[test]
    fn the_turn_spends_the_yaw_it_used() {
        let (scene, layer, id) = turned_face(0.5);
        let object = resolved(&scene, layer, id, 0);
        assert_eq!(object.spatial.rotation_y, 0.5, "the source still carries it");
        let drawn = scene
            .modified_object_at(layer, &object, 0u32)
            .expect("a modifier")
            .object
            .expect("an owned copy");
        assert_eq!(
            drawn.spatial.rotation_y, 0.0,
            "the drawn copy must be flat or the renderer turns it twice"
        );
    }

    /// **A drawn view beats a calculated one.** If the animator has drawn the
    /// profile, the renderer swaps it in and this must stand aside — otherwise
    /// their drawing would be turned on top of already being the turn.
    #[test]
    fn a_drawn_turnaround_view_wins() {
        let mut scene = Scene::empty();
        let (layer, id) = face(&mut scene, 4);
        let profile = Arc::new(Object::shape(
            ObjectId(999),
            ShapeData::filled(Rect::new(0.0, 0.0, 60.0, 100.0).to_path(1e-9), Color::WHITE),
        ));
        scene.update_object_across(0, 30, id, |o| {
            o.spatial.rotation_y = 1.4;
            o.turnaround.set(std::f64::consts::FRAC_PI_2, profile.clone());
            o.modifiers.push(Modifier::Turn { round: 1.0 });
        });

        let object = resolved(&scene, layer, id, 0);
        let eval = scene.modified_object_at(layer, &object, 0u32).expect("evaluated");
        assert!(
            eval.object.is_none(),
            "the drawn profile was nearer, so the turn should have done nothing"
        );
    }

    /// **The map is symmetric.** Turning left and turning right are the same
    /// arithmetic mirrored, and a face that turns better one way than the other
    /// is a bug nobody would think to look for.
    #[test]
    fn turning_the_other_way_mirrors_it() {
        for u in [-0.8, -0.3, 0.0, 0.3, 0.8] {
            let a = turn_at(u, 0.4, 1.0).expect("visible");
            let b = turn_at(-u, -0.4, 1.0).expect("visible");
            assert!(
                (a.0 + b.0).abs() < 1e-9,
                "u={u}: {} against mirrored {}",
                a.0,
                b.0
            );
            assert!((a.1 - b.1).abs() < 1e-9, "u={u}: foreshortening differs");
        }
    }

    /// **Zero roundness is a flat board.** It slides and never foreshortens,
    /// which is the right answer for a signpost.
    #[test]
    fn a_flat_board_slides_without_foreshortening() {
        let (u, squeeze) = turn_at(0.7, 0.5, 0.0).expect("visible");
        assert!((u - 0.5_f64.sin()).abs() < 1e-9, "a board should just slide");
        assert!((squeeze - 0.5_f64.cos()).abs() < 1e-9);
    }

    /// **Sway bends the top and plants the base.** A tree that pivoted about
    /// its middle would lift its roots out of the ground, which is the reason
    /// this is a shear rather than a rotation.
    #[test]
    fn sway_leans_the_top_and_plants_the_base() {
        let mut scene = Scene::empty();
        let (layer, id) = standing_square(&mut scene, 12);
        scene.update_object_across(0, 60, id, |o| {
            o.modifiers.push(Modifier::Sway {
                amount: 0.3,
                rate: 0.5,
            });
        });

        let mut leans = Vec::new();
        for frame in 0..60u32 {
            let obj = resolved(&scene, layer, id, frame);
            let eval = scene.modified_object_at(layer, &obj, frame).unwrap();
            let base = eval.prepend * buzz_geom::Point::new(50.0, 100.0);
            assert!(
                (base - buzz_geom::Point::new(50.0, 100.0)).hypot() < 1e-9,
                "frame {frame}: the base moved to {base:?}"
            );
            leans.push((eval.prepend * buzz_geom::Point::new(50.0, 0.0)).x);
        }
        let lo = leans.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = leans.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(hi - lo > 2.0, "the top hardly moved: {lo}..{hi}");
    }

    /// **A drift loops, and its phase says where in the loop it starts.**
    ///
    /// The phase is what makes a *field* of drifting things possible: without
    /// it every cloud on one loop crosses the sky in formation.
    #[test]
    fn a_drift_wraps_and_its_phase_offsets_it() {
        let mut scene = Scene::empty();
        let (layer, id) = standing_square(&mut scene, 13);
        scene.update_object_across(0, 60, id, |o| {
            o.modifiers.push(Modifier::Drift {
                dx: 100.0,
                dy: 0.0,
                span: 200.0,
                phase: 0.0,
            });
        });
        let fps = scene.stage().frame_rate.max(1.0);
        let at = |frame: u32| {
            let obj = resolved(&scene, layer, id, frame);
            scene
                .modified_object_at(layer, &obj, frame)
                .unwrap()
                .prepend
                .translation()
                .x
        };

        // A hundred units a second, wrapping every two hundred: back to the
        // start after exactly two seconds.
        assert!(at(0).abs() < 1e-9);
        let two_seconds = (2.0 * fps) as u32;
        assert!(
            at(two_seconds).abs() < 1e-6,
            "it did not come back: {}",
            at(two_seconds)
        );
        assert!(at(fps as u32) > 90.0, "it barely moved in a second");

        // And a half phase starts it half way along.
        let mut other = Scene::empty();
        let (layer2, id2) = standing_square(&mut other, 14);
        other.update_object_across(0, 60, id2, |o| {
            o.modifiers.push(Modifier::Drift {
                dx: 100.0,
                dy: 0.0,
                span: 200.0,
                phase: 0.5,
            });
        });
        let obj = resolved(&other, layer2, id2, 0);
        let offset = other
            .modified_object_at(layer2, &obj, 0)
            .unwrap()
            .prepend
            .translation()
            .x;
        assert!(
            (offset - 100.0).abs() < 1e-6,
            "a half phase should start it half way along, got {offset}"
        );
    }

    #[test]
    fn look_at_turns_the_object_toward_its_target() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let mut object = Object::shape(
            ObjectId(3),
            ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
        );
        // At (100,100), facing +x. Target straight below (+y), so it should turn
        // a quarter turn.
        object.transform = Affine::translate((100.0, 100.0));
        object.modifiers.push(Modifier::LookAt { x: 100.0, y: 300.0 });
        let id = scene.add_object(layer, object).unwrap();

        let obj = resolved(&scene, layer, id, 0);
        let eval = scene.modified_object_at(layer, &obj, 0).unwrap();
        let c = eval.prepend.as_coeffs();
        let angle = c[1].atan2(c[0]);
        assert!(
            (angle - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "aimed {angle} rad, not down at the target"
        );
    }

    #[test]
    fn squash_stretch_lengthens_along_the_motion() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let mut object = Object::shape(
            ObjectId(4),
            ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
        );
        object.transform = Affine::translate((0.0, 100.0));
        object.modifiers.push(Modifier::AutoSquashStretch { amount: 0.02 });
        let id = scene.add_object(layer, object).unwrap();

        // Slide it along +x from 0 to 200 over ten frames.
        scene.update_layer(layer, |l| {
            if l.frames.length() <= 10 {
                l.frames.insert_frame(10);
            }
        });
        scene.ensure_keyframe(layer, 10);
        scene.update_object_at(10, id, |o| {
            o.transform = Affine::translate((200.0, 100.0));
        });
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::motion());
        });

        let obj = resolved(&scene, layer, id, 5);
        let c = scene.modified_object_at(layer, &obj, 5).unwrap().prepend.as_coeffs();
        // Motion is +x, so the linear part scales x up and y down, and preserves
        // area (x scale * y scale ~= 1).
        assert!(c[0] > 1.1, "x should stretch, was {}", c[0]);
        assert!(c[3] < 0.95, "y should squash, was {}", c[3]);
        assert!((c[0] * c[3] - 1.0).abs() < 0.02, "should preserve area");
    }

    #[test]
    fn the_spring_cache_recomputes_after_an_edit() {
        let (mut scene, layer, id) = rig_scene();
        animate_base(&mut scene, layer, id, 47);
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Spring {
                root: 1,
                stiffness: 80.0,
                damping: 6.0,
                coupling: 0.0,
            });
        });

        let pose_before = match scene
            .modified_object_at(layer, &resolved(&scene, layer, id, 8), 8)
            .unwrap()
            .object
            .unwrap()
            .kind
        {
            ObjectKind::Armature(d) => d.armature.pose(),
            _ => unreachable!(),
        };

        // Change the primary motion (a bigger swing). This bumps the revision, so
        // the cached spring sequence must be thrown away and rebuilt.
        scene.update_object_at(12, id, |o| {
            if let ObjectKind::Armature(r) = &mut o.kind {
                r.armature.set_pose(&[1.6, 0.0, 0.0, 0.0]);
            }
        });

        let pose_after = match scene
            .modified_object_at(layer, &resolved(&scene, layer, id, 8), 8)
            .unwrap()
            .object
            .unwrap()
            .kind
        {
            ObjectKind::Armature(d) => d.armature.pose(),
            _ => unreachable!(),
        };

        assert_ne!(pose_before, pose_after, "the cache did not recompute after the edit");
    }
}
