//! Performances: turning "walk from here to there" into keyframes.
//!
//! # What a performance is, and what it deliberately is not
//!
//! A performance is a **function from time to a pose**. [`Action::pose_at`]
//! takes a phase — where in the cycle we are, `0.0..1.0` — and returns one angle
//! per bone, plus how the whole figure is displaced. That is all of it. Writing
//! those poses onto a timeline is a separate job ([`apply`]) and knows nothing
//! about walking.
//!
//! Keeping the two apart is what makes this testable without a document, and it
//! is also the honest description of the feature: this is not a solver, it is
//! not physics, and it is not learned from anything. It is the handful of curves
//! an animator draws for a walk cycle, written down.
//!
//! # It writes keyframes, and then it is gone
//!
//! There is no live "walk" property on the object. [`apply`] leaves ordinary
//! keyframes holding ordinary poses, which the animator then edits, retimes,
//! copies, or throws away one at a time. A generated performance that stayed
//! generated would be a thing you cannot draw on top of, which is the opposite
//! of what a drawing tool is for.
//!
//! # Why keys land on twos
//!
//! [`Performance::step`] defaults to two frames, with a tween between, because
//! that is what hand-drawn animation does and because a key on every frame is a
//! timeline nobody can read or adjust. The tween fills the gap, so the motion is
//! smooth at any step; the step decides how much of it the animator can grab.

use buzz_geom::Affine;
use buzz_scene::{ObjectId, ObjectKind, Scene};

use crate::figure::Joint;

/// What the figure is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A walk cycle: legs and arms swinging in opposition, the body rising and
    /// falling twice per cycle, travelling forward.
    Walk,
    /// The same, quicker and looser: a longer stride, a deeper body drop, arms
    /// bent and driving.
    Run,
    /// **Standing and talking.** The body never holds still while somebody
    /// speaks — a weight shift, a head that moves on the stresses, hands that
    /// come up. This is that, and it deliberately does *not* touch the mouth:
    /// the mouth is lip sync, driven by the soundtrack, and the two are
    /// separate because one of them is a fact about the audio and the other is
    /// a choice about performance.
    Talk,
    /// Standing still, breathing. What a character does between the lines, and
    /// the difference between a held drawing and a dead one.
    Idle,

    // -- one-shots ----------------------------------------------------------
    //
    // Everything above is a **cycle**: it loops, and a longer beat is more
    // strides of the same thing. Everything below happens **once** — a body
    // going from one state to another — and a longer beat is the same move
    // taken more slowly. See [`Action::loops`], which is what tells the two
    // apart everywhere it matters.

    /// **Sitting down.** Thighs to the horizontal, knees folded under, the
    /// body lowered onto a seat, with the small forward tip and settle that
    /// stops it reading as a lift descending.
    ///
    /// The one action here that **leaves the body somewhere else**: it ends
    /// seated and stays seated, because that is what sitting down is. Follow
    /// it with [`Action::Stand`] to get back up; anything else written over
    /// the same character will start from the standing rest pose and snap them
    /// upright, which is the honest consequence of a pose library that has no
    /// idea what a chair is.
    Sit,
    /// **Getting up.** [`Action::Sit`] backwards, to the frame — one curve
    /// read in reverse rather than a second set of numbers that would drift
    /// out of step with the first.
    Stand,
    /// **Turning on the spot.** The head leads, the shoulders follow it and
    /// the hips come last, which is the order a body actually turns in and the
    /// reason a turn animated as one rigid block reads as a statue on a
    /// turntable.
    ///
    /// The *facing* — which way the drawing points — is the caller's, not
    /// this: see `direct::ActorState::face`. What is here is the body's part
    /// of it, and it ends back at rest.
    Turn,
    /// **Pointing at something.** The arm comes up and out, the head turns to
    /// look along it, and it holds — then comes down at the end, so the beat
    /// composes with whatever follows instead of leaving an arm in the air.
    Point,
    /// **Reaching for something.** A point with the whole body behind it: the
    /// chest leans after the arm, the far arm counterbalances, and the near
    /// heel would come up if this rig had one.
    Reach,
    /// **A reaction.** A recoil away, sharply, and then a settle back — the
    /// beat a writer means by *flinches*, *starts*, or *is taken aback*.
    ///
    /// Deliberately short and deliberately overshooting: a reaction that eases
    /// politely into place is not a reaction.
    React,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Walk => "Walk",
            Action::Run => "Run",
            Action::Talk => "Talk",
            Action::Idle => "Idle",
            Action::Sit => "Sit",
            Action::Stand => "Stand Up",
            Action::Turn => "Turn",
            Action::Point => "Point",
            Action::Reach => "Reach",
            Action::React => "React",
        }
    }

    /// The undo label, so the history says what was done.
    pub fn undo_label(self) -> &'static str {
        match self {
            Action::Walk => "Walk Cycle",
            Action::Run => "Run Cycle",
            Action::Talk => "Talking",
            Action::Idle => "Idle",
            Action::Sit => "Sitting Down",
            Action::Stand => "Standing Up",
            Action::Turn => "Turning",
            Action::Point => "Pointing",
            Action::Reach => "Reaching",
            Action::React => "Reacting",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Action::Walk => "Legs and arms in opposition, the body rising twice a stride",
            Action::Run => "A longer stride, a deeper drop, and arms bent and driving",
            Action::Talk => "A weight shift, head movement on the stresses, hands coming up",
            Action::Idle => "Standing and breathing, so a held drawing is not a dead one",
            Action::Sit => "Down onto a seat, with the forward tip and settle that sells the weight",
            Action::Stand => "Back up again — the sit read backwards, so the two cannot drift apart",
            Action::Turn => "The head leads, the shoulders follow, the hips come last",
            Action::Point => "The arm comes up and out, the head looks along it, then it comes down",
            Action::Reach => "A point with the whole body behind it, the chest leaning after the arm",
            Action::React => "A sharp recoil and a settle — a flinch, a start, being taken aback",
        }
    }

    /// How long one cycle runs, in seconds, at an ordinary tempo.
    ///
    /// A walk is about two steps a second, which is a real measurement of real
    /// people and is why an animator's default walk is twelve frames a step at
    /// twenty-four. Talking and idling have no cycle in the same sense; the
    /// numbers here are the period of the slowest curve in them, which is what
    /// stops the motion looking like a metronome.
    pub fn cycle_seconds(self) -> f64 {
        match self {
            Action::Walk => 1.0,
            Action::Run => 0.6,
            Action::Talk => 3.2,
            Action::Idle => 4.0,
            // A one-shot's "cycle" is how long the move takes when nobody says
            // otherwise: how long it takes a person to do the thing. These are
            // the numbers a director uses to size a beat, not a tempo.
            Action::Sit => 1.2,
            Action::Stand => 1.0,
            Action::Turn => 0.8,
            Action::Point => 1.3,
            Action::Reach => 1.5,
            Action::React => 0.7,
        }
    }

    /// Does the figure travel while doing this?
    pub fn travels(self) -> bool {
        matches!(self, Action::Walk | Action::Run)
    }

    /// **Does this repeat, or happen once?**
    ///
    /// The distinction runs through everything: a longer beat of a cycle is
    /// *more strides of the same thing*, and a longer beat of a one-shot is
    /// *the same move taken more slowly*. It also decides whether the phase
    /// wraps — a cycle's does, and a one-shot's must not, or the last frame of
    /// a sit would be the first frame of it and the character would spring
    /// back up on the very keyframe that was meant to hold them down.
    pub fn loops(self) -> bool {
        matches!(
            self,
            Action::Walk | Action::Run | Action::Talk | Action::Idle
        )
    }
}

/// One performance to write onto the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Performance {
    pub action: Action,
    /// The frames to fill, as a half-open range.
    pub frames: std::ops::Range<u32>,
    /// Multiplies the whole performance. Half is a listless version of it and
    /// two is a broad one; it scales the angles, not the tempo.
    pub amount: f64,
    /// Multiplies the tempo. Two is twice as many strides in the same frames.
    pub tempo: f64,
    /// How far the figure travels over the whole range, in document units.
    ///
    /// **Along x only**, and positive is the way the figure faces. Distance
    /// rather than speed because the animator is placing a character in a shot:
    /// "cross the frame" is the intent, and "one hundred and ninety units per
    /// second" is arithmetic they should not have to do. Ignored by the actions
    /// that do not travel.
    pub distance: f64,
    /// Frames between keys. Two is animating on twos, which is what hand-drawn
    /// animation does; one is on ones.
    pub step: u32,
}

impl Performance {
    /// A performance of `action` over `frames`, with everything else ordinary.
    pub fn new(action: Action, frames: std::ops::Range<u32>) -> Self {
        Self {
            action,
            frames,
            amount: 1.0,
            tempo: 1.0,
            distance: if action.travels() { 400.0 } else { 0.0 },
            step: 2,
        }
    }

    /// How many whole cycles fit in the range at this tempo.
    ///
    /// **Rounded to a whole number, and never less than one.** A walk cut off
    /// three-quarters of the way through its cycle ends on one foot in the air
    /// and snaps back to a stand — which is the single most obvious way a
    /// generated walk announces itself. Stretching the tempo slightly to fit
    /// the frames is invisible; not doing it is not.
    pub fn cycles(&self, fps: f64) -> f64 {
        // **A one-shot happens once, however long the beat is.** Sitting down
        // twice because the animator gave it three seconds is not a slower sit,
        // it is a person bobbing.
        if !self.action.loops() {
            return 1.0;
        }
        let seconds = self.frames.len() as f64 / fps.max(1e-6);
        let wanted = seconds / self.action.cycle_seconds() * self.tempo.max(0.05);
        wanted.round().max(1.0)
    }
}

/// A figure's state at one instant: where every joint is, and how the body as a
/// whole has been displaced from where it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Beat {
    /// One angle per bone, to be *added* to the rest pose.
    pub joints: Vec<f64>,
    /// How far the body has moved from its placed position, in the figure's own
    /// units: x forward, y up-negative.
    pub offset: buzz_geom::Vec2,
}

impl Beat {
    fn rest(bones: usize) -> Self {
        Self {
            joints: vec![0.0; bones],
            offset: buzz_geom::Vec2::ZERO,
        }
    }

    fn set(&mut self, joint: Joint, angle: f64) {
        if let Some(slot) = self.joints.get_mut(joint.index()) {
            *slot = angle;
        }
    }
}

/// Two pi, which every cycle here is measured in.
const TURN: f64 = std::f64::consts::TAU;

/// **The pose at a point in the cycle.**
///
/// `phase` runs `0.0..1.0` over one cycle and is allowed to run past it; only
/// its fractional part matters. `amount` scales every angle.
///
/// This is the whole of the animation, and it is deliberately readable: each
/// line is one curve an animator would recognise, with the reason it is there.
pub fn pose_at(action: Action, phase: f64, amount: f64) -> Beat {
    let mut beat = Beat::rest(Joint::ALL.len());
    let a = amount.clamp(0.0, 3.0);
    // **A cycle wraps; a one-shot does not.** Running a sit past its end would
    // otherwise land back at its beginning, springing the character upright on
    // the very keyframe meant to hold them down. See `Action::loops`.
    let u = if action.loops() {
        phase.rem_euclid(1.0)
    } else {
        phase.clamp(0.0, 1.0)
    };
    let t = u * TURN;

    match action {
        Action::Walk | Action::Run => {
            let running = action == Action::Run;
            // A run has a longer stride and a deeper drop; everything else about
            // the two is the same curve.
            let stride = if running { 0.85 } else { 0.52 } * a;
            let knee_bend = if running { 1.35 } else { 0.75 } * a;
            let arm_swing = if running { 0.75 } else { 0.42 } * a;
            let bob = if running { 0.055 } else { 0.028 } * a;

            // **The legs.** One thigh forward while the other is back, half a
            // cycle apart. The near side leads, which is arbitrary and has to be
            // *some* convention or the arms cannot be put in opposition to it.
            beat.set(Joint::ThighL, stride * t.sin());
            beat.set(Joint::ThighR, stride * (t + std::f64::consts::PI).sin());

            // **The knees.** A knee only bends one way, so this is a rectified
            // curve rather than a sine: the shin folds under as the leg swings
            // through and is straight at the moment the foot takes the weight.
            // Getting this wrong — a plain sine — bends the knee backwards for
            // half of every stride, which is the other unmistakable sign of a
            // generated walk.
            beat.set(Joint::KneeL, -knee_bend * fold(t));
            beat.set(
                Joint::KneeR,
                -knee_bend * fold(t + std::f64::consts::PI),
            );

            // **The arms, opposite the legs.** Left arm with the right leg. A
            // run holds the elbows bent throughout; a walk lets them hang and
            // swing a little.
            beat.set(Joint::ShoulderL, arm_swing * (t + std::f64::consts::PI).sin());
            beat.set(Joint::ShoulderR, arm_swing * t.sin());
            let elbow = if running { -1.4 * a } else { -0.25 * a };
            beat.set(Joint::ElbowL, elbow - 0.2 * a * fold(t));
            beat.set(
                Joint::ElbowR,
                elbow - 0.2 * a * fold(t + std::f64::consts::PI),
            );

            // **The body counter-rotates.** The shoulders turn against the hips,
            // which is what makes a walk read as a person rather than as a
            // puppet on a stick. Small: on flat artwork seen from the side, a
            // little of it goes a long way.
            beat.set(Joint::Hips, 0.05 * a * t.sin());
            beat.set(Joint::Chest, -0.09 * a * t.sin());

            // **The head stays level.** It counteracts the chest almost exactly,
            // because the one thing a walking person's head does is *not* move
            // as much as their shoulders.
            beat.set(Joint::Head, 0.05 * a * t.sin());

            // **The body rises twice per stride**, once for each foot passing
            // under it. One cycle of this curve is *two* steps, so the rise
            // wanted here is `|sin t|` — which peaks twice over the cycle. A
            // plain `sin t` would rise once and read as a limp; `|sin 2t|`
            // peaks four times and reads as a trot.
            beat.offset.y = -bob * t.sin().abs() * 30.0;
            // A slight forward lean into a run.
            if running {
                beat.set(Joint::Hips, beat.joints[Joint::Hips.index()] + 0.12 * a);
            }
        }

        Action::Talk => {
            // **A weight shift, slow.** Somebody standing and speaking moves
            // their weight from one foot to the other every few seconds, and
            // that — not the arms — is what makes them look alive.
            beat.set(Joint::Hips, 0.055 * a * t.sin());
            beat.set(Joint::Chest, -0.035 * a * t.sin());

            // **The head moves on the stresses.** Three times the body's rate,
            // and out of phase with it, so the two never line up into a bounce.
            // The half-turn offset is what stops a nod landing on a sway.
            beat.set(
                Joint::Head,
                0.10 * a * (3.0 * t + 0.9).sin() + 0.04 * a * (7.0 * t).sin(),
            );

            // **The hands come up.** Not in step with each other: a gesture is
            // one hand leading and the other following, and two arms moving
            // identically reads as a swimming stroke.
            beat.set(Joint::ShoulderL, -0.30 * a * (2.0 * t).sin().max(0.0));
            beat.set(
                Joint::ShoulderR,
                -0.22 * a * (2.0 * t + 1.7).sin().max(0.0),
            );
            beat.set(Joint::ElbowL, -0.55 * a * (2.0 * t).sin().max(0.0) - 0.2 * a);
            beat.set(
                Joint::ElbowR,
                -0.45 * a * (2.0 * t + 1.7).sin().max(0.0) - 0.2 * a,
            );

            // The knees stay all but straight: this is standing, not shifting
            // about. A hair of give on the weighted side.
            beat.set(Joint::KneeL, -0.05 * a * t.sin().max(0.0));
            beat.set(Joint::KneeR, -0.05 * a * (-t.sin()).max(0.0));
        }

        Action::Idle => {
            // **Breathing.** The chest rises and the shoulders follow it, slowly
            // and by very little — this is the amplitude at which it reads as
            // life rather than as a wobble.
            let breath = t.sin();
            beat.set(Joint::Chest, -0.028 * a * breath);
            beat.set(Joint::Head, 0.018 * a * breath);
            beat.set(Joint::ShoulderL, -0.03 * a * breath);
            beat.set(Joint::ShoulderR, -0.03 * a * breath);
            // A slow drift of weight underneath it, at a different period so
            // the two never come back into step and the loop never announces
            // itself.
            beat.set(Joint::Hips, 0.022 * a * (0.6 * t + 1.1).sin());
            beat.offset.y = -1.2 * a * breath;
        }

        // -- the one-shots --------------------------------------------------

        Action::Sit => sit_into(&mut beat, ease(u), a),

        // **Read backwards, not written twice.** Two hand-authored sets of
        // numbers for the same move in opposite directions drift apart the
        // first time either is adjusted; one curve read from the far end
        // cannot.
        Action::Stand => sit_into(&mut beat, ease(1.0 - u), a),

        Action::Turn => {
            // **The head leads and the hips come last.** A quarter of the move
            // apart, which is what makes a turn read as a body deciding to
            // turn rather than a statue on a turntable. Each part swings out
            // and comes back: the *facing* is the caller's mirror, and what is
            // animated here is the body's part of getting there.
            let swing = |lag: f64| {
                let p = ((u - lag) / (1.0 - lag)).clamp(0.0, 1.0);
                (p * std::f64::consts::PI).sin()
            };
            beat.set(Joint::Head, 0.45 * a * swing(0.0));
            beat.set(Joint::Chest, 0.30 * a * swing(0.12));
            beat.set(Joint::Hips, 0.18 * a * swing(0.25));

            // The arms trail the shoulders they hang from, and the weight dips
            // through the middle of the turn — a person turning drops a little
            // as they change which foot carries them.
            beat.set(Joint::ShoulderL, -0.25 * a * swing(0.18));
            beat.set(Joint::ShoulderR, 0.25 * a * swing(0.18));
            beat.set(Joint::KneeL, -0.10 * a * swing(0.20));
            beat.set(Joint::KneeR, -0.10 * a * swing(0.20));
            beat.offset.y = 2.0 * a * swing(0.20);
        }

        Action::Point | Action::Reach => {
            let reaching = action == Action::Reach;
            // Out over the first third, held through the middle, down over the
            // last quarter. Holding is what makes it a point rather than a
            // wave: an arm that goes up and immediately comes down is a
            // gesture nobody can follow.
            let out = hold(u, 0.30, 0.75);
            let extent = if reaching { 1.0 } else { 0.8 };

            // **The near arm.** Up to near horizontal.
            beat.set(Joint::ShoulderR, -1.25 * a * extent * out);

            // **The elbow bends on the way up and straightens into the point.**
            //
            // `bend` peaks during the rise and the fall and is zero across the
            // hold, so the arm folds as it lifts, opens out for the point
            // itself, and folds again as it comes down — which is what makes
            // the arrival read as an arrival rather than as a barrier arm.
            //
            // Every term is a multiple of `out`, and that is not incidental: a
            // constant here left the character standing with a permanently
            // bent arm before and after the gesture, which is what the test
            // `a_one_shot_leaves_the_body_where_it_found_it` was written to
            // catch and did.
            let bend = (out * (1.0 - out) * 4.0).clamp(0.0, 1.0);
            beat.set(Joint::ElbowR, (-0.55 * bend - 0.10 * out) * a * extent);

            // **The head looks along the arm**, slightly ahead of it: the eyes
            // get there first, always.
            beat.set(Joint::Head, 0.22 * a * hold(u, 0.22, 0.78));

            if reaching {
                // The whole body commits: the chest leans after the arm, the
                // far arm swings back as a counterweight, and the knees give.
                // Without the counterweight a reach reads as a character being
                // pulled by the wrist.
                beat.set(Joint::Chest, 0.22 * a * out);
                beat.set(Joint::Hips, 0.12 * a * out);
                beat.set(Joint::ShoulderL, 0.40 * a * out);
                beat.set(Joint::ElbowL, -0.30 * a * out);
                beat.set(Joint::KneeL, -0.18 * a * out);
                beat.set(Joint::KneeR, -0.10 * a * out);
                beat.offset.x = 4.0 * a * out;
            } else {
                // A point is the arm and almost nothing else; a little of the
                // chest so the shoulder is not working alone.
                beat.set(Joint::Chest, 0.08 * a * out);
                beat.set(Joint::ShoulderL, 0.10 * a * out);
            }
        }

        Action::React => {
            // **Away sharply, then back past rest, then settle.** A damped
            // swing rather than a curve out and back: a reaction that eases
            // politely into place is not a reaction, and the overshoot is the
            // whole of what makes it read as being *startled* rather than
            // deciding to lean.
            let swing = (-4.0 * u).exp() * (u * TURN * 1.35).sin();
            let recoil = swing * a;

            beat.set(Joint::Chest, -0.42 * recoil);
            beat.set(Joint::Head, -0.55 * recoil);
            beat.set(Joint::Hips, -0.18 * recoil);
            // The arms come up between the character and whatever it was.
            beat.set(Joint::ShoulderL, -0.50 * recoil);
            beat.set(Joint::ShoulderR, -0.44 * recoil);
            beat.set(Joint::ElbowL, -0.70 * recoil);
            beat.set(Joint::ElbowR, -0.62 * recoil);
            // Weight back onto the heels, and a small drop as the knees give.
            beat.set(Joint::KneeL, -0.22 * recoil.abs());
            beat.set(Joint::KneeR, -0.16 * recoil.abs());
            beat.offset.x = -7.0 * recoil;
            beat.offset.y = 2.5 * a * recoil.abs();
        }
    }

    beat
}

/// **Sitting down, at `p` of the way through it.**
///
/// `0.0` is standing and `1.0` is sat. Written once and read forwards for
/// [`Action::Sit`] and backwards for [`Action::Stand`], because two sets of
/// numbers for one move drift apart the first time either is touched.
///
/// The order matters more than the angles do: the hips go back *before* the
/// knees fold, which is how a person finds a chair they cannot see, and the
/// body tips forward on the way down and comes upright at the bottom. Lowering
/// straight down with a level back is what a lift does.
fn sit_into(beat: &mut Beat, p: f64, a: f64) {
    let p = p.clamp(0.0, 1.0);
    // The hips lead: this is at its strongest halfway down and is gone by the
    // time the character is seated.
    let seek = (p * std::f64::consts::PI).sin();

    // Thighs to the horizontal, knees folded under. Nearly the whole of the
    // shape; everything else is the flavour.
    beat.set(Joint::ThighL, 1.35 * a * p);
    beat.set(Joint::ThighR, 1.30 * a * p);
    beat.set(Joint::KneeL, -1.45 * a * p);
    beat.set(Joint::KneeR, -1.40 * a * p);

    // Forward through the descent, upright once seated.
    beat.set(Joint::Hips, 0.30 * a * seek + 0.10 * a * p);
    beat.set(Joint::Chest, -0.18 * a * seek);
    beat.set(Joint::Head, 0.12 * a * seek);

    // The arms swing forward for balance on the way down and settle to the lap.
    beat.set(Joint::ShoulderL, -0.35 * a * seek - 0.12 * a * p);
    beat.set(Joint::ShoulderR, -0.32 * a * seek - 0.12 * a * p);
    beat.set(Joint::ElbowL, -0.30 * a * p);
    beat.set(Joint::ElbowR, -0.28 * a * p);

    // Down by about a third of the figure's height, which is where a seat is.
    beat.offset.y = 26.0 * a * p;
    // And a little back, because a chair is behind you.
    beat.offset.x = -6.0 * a * p;
}

/// Smooth in and out, `0..1`. The pacing a body has and a machine does not.
fn ease(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// **Out by `rise`, held, and back by `fall`** — `0..1`.
///
/// The shape of every gesture that has to be *read*: it arrives, it stays long
/// enough to be seen, and it leaves. Both edges are eased, so the hand does not
/// start and stop dead.
fn hold(u: f64, rise: f64, fall: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    if u < rise {
        ease(u / rise.max(1e-6))
    } else if u < fall {
        1.0
    } else {
        ease((1.0 - u) / (1.0 - fall).max(1e-6))
    }
}

/// How far a knee is folded at phase `t`.
///
/// `0.0` at the moment the foot is planted and rising to `1.0` as the leg
/// swings through, never negative — because a knee does not bend backwards.
/// This one function is the difference between a walk and a marionette.
fn fold(t: f64) -> f64 {
    // Shifted so the peak lands in the swing phase rather than under the
    // weight, and squared so the leg straightens sharply into the contact
    // rather than easing into it.
    let raw = (0.5 - 0.5 * (t + 0.9).cos()).clamp(0.0, 1.0);
    raw * raw
}

/// What a run of [`apply`] produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PerformReport {
    /// Keyframes written.
    pub keyframes: u32,
    /// Frames covered.
    pub frames: u32,
    /// Whole cycles fitted into them.
    pub cycles: u32,
    pub message: String,
}

/// Why a performance could not be written.
#[derive(Debug, Clone, PartialEq)]
pub enum PerformError {
    /// The object is not on any layer, or has gone.
    NoObject,
    /// It is not rigged, so there is nothing to pose. Everything here animates
    /// a skeleton; a plain drawing has none.
    NotRigged,
    /// An empty frame range.
    NoFrames,
}

impl std::fmt::Display for PerformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PerformError::NoObject => write!(f, "that object is not on the timeline any more"),
            PerformError::NotRigged => write!(
                f,
                "only rigged artwork can be performed \u{2014} rig it with the Bone tool, \
                 or use Scene > Add Person for one that is already rigged"
            ),
            PerformError::NoFrames => write!(f, "there are no frames in that range to fill"),
        }
    }
}

impl std::error::Error for PerformError {}

/// **Write a performance onto the timeline.**
///
/// Every keyframe holds the figure posed for that instant, as an ordinary pose
/// on an ordinary keyframe: nothing about the result remembers that it was
/// generated, so all of it can be edited afterwards.
///
/// The caller wraps this in one `Document::edit` so that a whole walk is one
/// Ctrl+Z.
pub fn apply(
    scene: &mut Scene,
    object: ObjectId,
    performance: &Performance,
) -> Result<PerformReport, PerformError> {
    // Where it was placed. The travel is added to this rather than replacing it,
    // so a walk starts where the animator put the character.
    let placed = scene
        .find_object(object)
        .ok_or(PerformError::NoObject)?
        .1
        .transform;
    apply_from(scene, object, performance, placed)
}

/// [`apply`], starting from an explicit placement instead of the one on the
/// timeline.
///
/// For a *sequence* of performances — the director's case. Each beat writes
/// keyframes over its own frames, so by the second beat "where the object is"
/// depends on which frame you ask; the caller scheduling the sequence already
/// knows where the last beat left the figure, and says so here.
pub fn apply_from(
    scene: &mut Scene,
    object: ObjectId,
    performance: &Performance,
    placed: Affine,
) -> Result<PerformReport, PerformError> {
    if performance.frames.is_empty() {
        return Err(PerformError::NoFrames);
    }
    let Some((layer, found)) = scene.find_object(object) else {
        return Err(PerformError::NoObject);
    };
    let ObjectKind::Armature(rig) = &found.kind else {
        return Err(PerformError::NotRigged);
    };

    // The rest pose, taken once: every beat is measured from it, so a figure
    // that has been re-rigged or re-posed by hand still performs from where it
    // now stands rather than snapping to where it was built.
    let rest = rig.armature.at_rest().pose();

    let fps = scene.stage().frame_rate.max(1.0);
    let cycles = performance.cycles(fps);
    let span = performance.frames.len().max(1) as f64;
    let step = performance.step.max(1);

    // The layer has to be long enough to hold the performance before any of it
    // can be keyed: `ensure_keyframe` refuses past the end of the span, which
    // would otherwise silently write half a walk.
    let last = performance.frames.end.saturating_sub(1);
    scene.update_layer(layer, |l| {
        if l.frames.length() <= last {
            l.frames.insert_frame(last);
        }
    });

    let mut written = 0u32;
    let mut frame = performance.frames.start;
    while frame < performance.frames.end {
        // How far through the whole performance this frame is, and therefore
        // where in the cycle. Measured against the range rather than the clock
        // so that retiming the range restretches the motion.
        let progress = (frame - performance.frames.start) as f64 / span;
        let beat = pose_at(performance.action, progress * cycles, performance.amount);

        scene.ensure_keyframe(layer, frame);
        scene.update_object_at(frame, object, |target| {
            let ObjectKind::Armature(rig) = &mut target.kind else {
                return;
            };
            let posed: Vec<f64> = rest
                .iter()
                .zip(beat.joints.iter())
                .map(|(rest, delta)| rest + delta)
                .collect();
            rig.armature.set_pose(&posed);

            // **Travel and bob ride on the object's own transform**, not on the
            // root bone. A root bone carrying the figure across the stage would
            // put the translation inside the rig, where a later re-rig or a
            // change of scale would multiply it — and where the animator could
            // not simply drag the character somewhere else.
            let travel = if performance.action.travels() {
                performance.distance * progress
            } else {
                0.0
            };
            target.transform =
                placed * Affine::translate((travel, beat.offset.y));
        });

        // A tween across the gap, so animating on twos is smooth on ones. The
        // last key gets none: there is nothing after it to tween to, and a
        // tween pointing at nothing holds its keyframe anyway.
        if frame + step < performance.frames.end {
            scene.update_layer(layer, |l| {
                l.frames.set_tween(
                    frame,
                    buzz_scene::Tween::motion(),
                );
            });
        }

        written += 1;
        frame += step;
    }

    let frames = performance.frames.len() as u32;
    Ok(PerformReport {
        keyframes: written,
        frames,
        cycles: cycles as u32,
        message: format!(
            "{}: {written} keyframe(s) over {frames} frame(s), {} cycle(s)",
            performance.action.label(),
            cycles as u32
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figure::{self, FigureSpec};
    use buzz_scene::LayerKind;

    fn document() -> (Scene, buzz_scene::LayerId, ObjectId) {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Cast", LayerKind::Normal);
        let id = scene.next_object_id();
        let figure = figure::build(&FigureSpec::default(), id, || scene_id(&mut scene));
        let placed = scene
            .add_object(layer, figure)
            .expect("the figure goes on the layer");
        (scene, layer, placed)
    }

    fn scene_id(scene: &mut Scene) -> ObjectId {
        scene.next_object_id()
    }

    fn angle(scene: &Scene, frame: u32, object: ObjectId, joint: Joint) -> f64 {
        let (_, found) = scene.find_object(object).expect("the figure");
        let _ = frame;
        match &found.kind {
            ObjectKind::Armature(rig) => rig.armature.bones[joint.index()].angle,
            _ => panic!("not rigged"),
        }
    }

    /// **A knee never bends backwards.** The single most visible way a
    /// generated walk gives itself away, and the reason `fold` is not a sine.
    #[test]
    fn the_knees_only_ever_fold_one_way() {
        for i in 0..400 {
            let phase = i as f64 / 400.0;
            for action in [Action::Walk, Action::Run] {
                let beat = pose_at(action, phase, 1.0);
                for knee in [Joint::KneeL, Joint::KneeR] {
                    assert!(
                        beat.joints[knee.index()] <= 1e-12,
                        "{action:?} bent {} the wrong way at phase {phase}: {}",
                        knee.label(),
                        beat.joints[knee.index()]
                    );
                }
            }
        }
    }

    /// The arms swing against the legs. Two arms swinging *with* the legs is a
    /// person marching, and it is what you get if the phase offset is dropped.
    #[test]
    fn the_arms_swing_opposite_the_legs() {
        // A quarter through the cycle, where the leg swing is at its peak.
        let beat = pose_at(Action::Walk, 0.25, 1.0);
        let thigh = beat.joints[Joint::ThighL.index()];
        let shoulder = beat.joints[Joint::ShoulderL.index()];
        assert!(
            thigh.abs() > 0.1 && shoulder.abs() > 0.1,
            "both should be well off rest: thigh {thigh}, shoulder {shoulder}"
        );
        assert!(
            thigh.signum() != shoulder.signum(),
            "the near arm goes back as the near leg goes forward: \
             thigh {thigh}, shoulder {shoulder}"
        );
    }

    /// **A cycle closes.** The pose at the end of a cycle is the pose at its
    /// start, so a walk can be looped and does not jump when it repeats.
    #[test]
    fn a_cycle_ends_where_it_began() {
        for action in [Action::Walk, Action::Run, Action::Talk, Action::Idle] {
            let start = pose_at(action, 0.0, 1.0);
            let end = pose_at(action, 1.0, 1.0);
            for (i, (a, b)) in start.joints.iter().zip(end.joints.iter()).enumerate() {
                assert!(
                    (a - b).abs() < 1e-9,
                    "{action:?} does not close at bone {i}: {a} against {b}"
                );
            }
        }
    }

    /// The body rises twice per stride, once under each foot. A single rise per
    /// cycle is a limp, and is what a naive `sin(t)` gives.
    #[test]
    fn the_body_rises_twice_per_stride() {
        let peaks = (0..1000)
            .map(|i| pose_at(Action::Walk, i as f64 / 1000.0, 1.0).offset.y)
            .collect::<Vec<_>>();
        // Count how many times the height turns round from rising to falling.
        let mut turns = 0;
        for w in peaks.windows(3) {
            if w[1] < w[0] && w[1] < w[2] {
                turns += 1;
            }
        }
        assert_eq!(turns, 2, "two rises per stride, got {turns}");
    }

    /// Nothing is generated for a drawing that has no skeleton, and the reason
    /// is said rather than the command quietly doing nothing.
    #[test]
    fn a_drawing_with_no_bones_cannot_be_performed() {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                buzz_scene::ShapeData::filled(
                    buzz_geom::Shape::to_path(&buzz_geom::Rect::new(0.0, 0.0, 10.0, 10.0), 1e-9),
                    peniko::Color::WHITE,
                ),
            )
            .expect("a shape");
        let err = apply(&mut scene, id, &Performance::new(Action::Walk, 0..24)).unwrap_err();
        assert_eq!(err, PerformError::NotRigged);
    }

    /// A performance really does land on the timeline, on twos by default, and
    /// really does move the figure.
    #[test]
    fn a_walk_lands_on_the_timeline_and_travels() {
        let (mut scene, layer, id) = document();
        let report = apply(
            &mut scene,
            id,
            &Performance {
                distance: 300.0,
                ..Performance::new(Action::Walk, 0..24)
            },
        )
        .expect("the walk applies");

        assert_eq!(report.frames, 24);
        assert!(report.keyframes >= 11, "on twos over 24 frames");
        assert!(report.cycles >= 1);

        let keys = scene
            .layers()
            .get(layer)
            .expect("the layer")
            .frames
            .keyframe_count();
        assert!(keys >= 11, "the keys are really there, got {keys}");

        // It ends further along than it started, by about the distance asked
        // for — "about", because the last key lands a step short of the end.
        let start = scene
            .layers()
            .get(layer)
            .expect("the layer")
            .frames
            .objects_at(0)
            .first()
            .expect("posed at the start")
            .transform
            .as_coeffs()[4];
        let end = scene
            .layers()
            .get(layer)
            .expect("the layer")
            .frames
            .objects_at(22)
            .first()
            .expect("posed at the end")
            .transform
            .as_coeffs()[4];
        assert!(
            end - start > 200.0,
            "it should have crossed most of the distance, {start} to {end}"
        );
    }

    /// The pose really changes frame to frame — a "performance" that wrote the
    /// same pose onto every key would pass every other test here.
    #[test]
    fn the_pose_actually_changes_over_the_performance() {
        let (mut scene, _, id) = document();
        apply(&mut scene, id, &Performance::new(Action::Walk, 0..24)).expect("applies");

        let at = |scene: &Scene, frame: u32| -> f64 {
            let objects = scene
                .layers()
                .iter()
                .find_map(|l| {
                    let objects = l.frames.objects_at(frame);
                    (!objects.is_empty()).then(|| objects.to_vec())
                })
                .expect("something on that frame");
            match &objects[0].kind {
                ObjectKind::Armature(rig) => rig.armature.bones[Joint::ThighL.index()].angle,
                _ => panic!("not rigged"),
            }
        };
        let a = at(&scene, 0);
        let b = at(&scene, 6);
        assert!(
            (a - b).abs() > 0.05,
            "the near thigh should have swung by frame 6: {a} against {b}"
        );
        let _ = angle(&scene, 0, id, Joint::ThighL);
    }

    /// An idle is not a walk: it must not travel, whatever distance is set.
    #[test]
    fn standing_actions_do_not_travel() {
        let (mut scene, layer, id) = document();
        apply(
            &mut scene,
            id,
            &Performance {
                distance: 900.0,
                ..Performance::new(Action::Idle, 0..24)
            },
        )
        .expect("applies");

        let x = |frame: u32| {
            scene
                .layers()
                .get(layer)
                .expect("the layer")
                .frames
                .objects_at(frame)
                .first()
                .expect("posed")
                .transform
                .as_coeffs()[4]
        };
        assert!(
            (x(0) - x(20)).abs() < 1e-9,
            "an idle stands still: {} against {}",
            x(0),
            x(20)
        );
    }

    /// A range too short for a whole cycle still gets one, rather than a walk
    /// that stops with a foot in the air.
    #[test]
    fn a_short_range_still_gets_a_whole_cycle() {
        let performance = Performance::new(Action::Walk, 0..6);
        assert_eq!(performance.cycles(24.0), 1.0);
    }
}
