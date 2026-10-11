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

use buzz_geom::{Affine, Point, Vec2};
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
    /// **Waving.** The near arm goes up with the forearm upright and the hand
    /// swings from the elbow three times, then comes down — a greeting, or
    /// *arre, suno!* across a street.
    Wave,
    /// **Nodding.** The head dips forward twice and comes back: yes.
    Nod,
    /// **Shaking the head.** Quick small tilts either side, settling to rest:
    /// no. On flat artwork there is no turn of the face to show, so it is the
    /// head rocking that carries it.
    HeadShake,
    /// **A shrug.** Both forearms come up and out, palms up, the head tips back
    /// and the body lifts a little — *who knows?*
    Shrug,
    /// **Namaste.** Both hands come together in front of the chest and the
    /// head and chest bow a little while they are there.
    Namaste,
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
            Action::Wave => "Wave",
            Action::Nod => "Nod",
            Action::HeadShake => "Shake Head",
            Action::Shrug => "Shrug",
            Action::Namaste => "Namaste",
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
            Action::Wave => "Waving",
            Action::Nod => "Nodding",
            Action::HeadShake => "Shaking Head",
            Action::Shrug => "Shrugging",
            Action::Namaste => "Namaste",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Action::Walk => "Legs and arms in opposition, the body rising twice a stride",
            Action::Run => "A longer stride, a deeper drop, and arms bent and driving",
            Action::Talk => "A weight shift, head movement on the stresses, hands coming up",
            Action::Idle => "Standing and breathing, so a held drawing is not a dead one",
            Action::Sit => {
                "Down onto a seat, with the forward tip and settle that sells the weight"
            }
            Action::Stand => {
                "Back up again — the sit read backwards, so the two cannot drift apart"
            }
            Action::Turn => "The head leads, the shoulders follow, the hips come last",
            Action::Point => {
                "The arm comes up and out, the head looks along it, then it comes down"
            }
            Action::Reach => {
                "A point with the whole body behind it, the chest leaning after the arm"
            }
            Action::React => "A sharp recoil and a settle — a flinch, a start, being taken aback",
            Action::Wave => "The arm up, the hand swinging from the elbow, then down again",
            Action::Nod => "The head dips forward twice and comes back — yes",
            Action::HeadShake => "Small quick tilts of the head either side — no",
            Action::Shrug => "Forearms up and out, palms up, the head tipped back",
            Action::Namaste => "Palms together at the chest, with a small bow",
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
            Action::Wave => 1.4,
            Action::Nod => 0.9,
            Action::HeadShake => 1.1,
            Action::Shrug => 1.0,
            Action::Namaste => 1.4,
        }
    }

    /// Does this move only the head? A rig with no head bone has nothing to
    /// do it with, and the caller skips it rather than turning whatever bone
    /// happens to sit in the head's slot.
    pub fn needs_head(self) -> bool {
        matches!(self, Action::Nod | Action::HeadShake)
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
    /// How far the whole body rises, as a **fraction of the rig's own
    /// height** rather than in units.
    ///
    /// For a move whose size is anatomical — the lift of a shrug — a fixed
    /// number of units is right on one rig and invisible on another: the
    /// procedural figure is 320 units tall and an Ep1 Cast rig is about
    /// 1,300. [`apply_from`] measures the rig and turns this into units; it
    /// is zero for everything authored in units, which therefore lands
    /// exactly where it always did.
    pub lift: f64,
}

impl Beat {
    fn rest(bones: usize) -> Self {
        Self {
            joints: vec![0.0; bones],
            offset: buzz_geom::Vec2::ZERO,
            lift: 0.0,
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
///
/// **On the canonical rig** — the procedural figure, `figure::build` at its
/// default size. The arms of the gestures that reach for something are hand
/// targets solved on a rig's own geometry, so a different rig gets different
/// angles for them: ask [`pose_on`] for those.
pub fn pose_at(action: Action, phase: f64, amount: f64) -> Beat {
    pose_on(action, phase, amount, canonical_rest())
}

/// **The pose at a point in the cycle, on this rig** (`rest` is its rest
/// pose, in its own space).
///
/// The table in [`table_pose`] for the spine, the legs and the actions that
/// stay as tables (walk, run, idle, sit, stand, turn, nod, shake) — with
/// their arms reflected for a rig whose arms hang the other way round
/// ([`arms_mirrored`]) — and, for the gestures that put a hand somewhere,
/// arms solved toward hand targets on this rig's own rest geometry
/// ([`hand_targets`]).
pub fn pose_on(action: Action, phase: f64, amount: f64, rest: &buzz_rig::Armature) -> Beat {
    // **A front or back view walks on the spot under its own hips**
    // ([`front_walk`]): the side view's stride would swing its legs across
    // each other. A side view keeps the table, exactly as it was.
    if action.travels() && rig_view(rest) == RigView::Front {
        return front_walk(action, phase, amount, rest);
    }
    let mut beat = table_pose(action, phase, amount);
    if hand_targeted(action) {
        match Body::of(rest) {
            Some(body) => hand_targets(&mut beat, action, phase, amount, &body, rest),
            // A rig without a body to measure has no arms to aim either.
            None => {}
        }
    } else if arms_mirrored(rest) {
        mirror_arms(&mut beat);
    }
    beat
}

/// The canonical rig [`pose_at`] answers for: the procedural figure at its
/// default size, built once.
fn canonical_rest() -> &'static buzz_rig::Armature {
    static REST: std::sync::OnceLock<buzz_rig::Armature> = std::sync::OnceLock::new();
    REST.get_or_init(|| {
        let mut n = 0u64;
        let figure = crate::figure::build(
            &crate::figure::FigureSpec::default(),
            ObjectId(u64::MAX - 64),
            || {
                n += 1;
                ObjectId(u64::MAX - 64 + n)
            },
        );
        match &figure.kind {
            ObjectKind::Armature(rig) => rig.armature.at_rest(),
            _ => buzz_rig::Armature::default(),
        }
    })
}

/// The pose table: every action's curves, with the arms of the hand-targeted
/// gestures left at rest for [`hand_targets`] to fill.
fn table_pose(action: Action, phase: f64, amount: f64) -> Beat {
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
            beat.set(Joint::KneeR, -knee_bend * fold(t + std::f64::consts::PI));

            // **The arms, opposite the legs.** Left arm with the right leg. A
            // run holds the elbows bent throughout; a walk lets them hang and
            // swing a little.
            beat.set(
                Joint::ShoulderL,
                arm_swing * (t + std::f64::consts::PI).sin(),
            );
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

            // **The hands come up** -- as hand targets, solved on the rig
            // ([`hand_targets`]), so they rise to the belt on every body and
            // never drop into the lap.

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

            // **The near arm** is a hand target ([`hand_targets`]): out to its
            // own side at shoulder height, or up and out for a reach. The
            // leans below are written for the canonical rig, whose near arm
            // is on the picture's right; [`hand_targets`] turns them toward
            // whichever side the near arm is really on.

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
                beat.set(Joint::KneeL, -0.18 * a * out);
                beat.set(Joint::KneeR, -0.10 * a * out);
                beat.offset.x = 4.0 * a * out;
            } else {
                // A point is the arm and almost nothing else; a little of the
                // chest so the shoulder is not working alone.
                beat.set(Joint::Chest, 0.08 * a * out);
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
            // The arms come up between the character and whatever it was:
            // hand targets, see [`hand_targets`].
            // Weight back onto the heels, and a small drop as the knees give.
            beat.set(Joint::KneeL, -0.22 * recoil.abs());
            beat.set(Joint::KneeR, -0.16 * recoil.abs());
            beat.offset.x = -7.0 * recoil;
            beat.offset.y = 2.5 * a * recoil.abs();
        }

        // Every term below is a multiple of an envelope that is zero at both
        // ends, for the reason `Point` gives: a gesture leaves the body where
        // it found it. The arm bones are addressed the way `Point` addresses
        // them — by `Joint` index — which is what makes these work on a Cast
        // rig as well as the puppet: `RigPattern::biped_full` keeps the
        // puppet's eleven bones first and in the same order.
        //
        // **The arms of these gestures are not in this table.** They are hand
        // targets in a body frame, solved on the rig's own rest geometry by
        // [`hand_targets`]: the rigs the director drives differ by more than a
        // mirror (shoulders on the midline or off it, long or short arms), so
        // no single table of arm angles, sign-flipped or not, is right on all
        // of them.
        Action::Wave => {
            // Up, held while the hand swings, down. The arm is a hand target
            // ([`hand_targets`]): above the shoulder and a little out, swinging
            // side to side three times across the hold.
            let up = hold(u, 0.22, 0.80);
            // A little of the head and chest toward the waving arm: written for
            // the canonical rig, turned toward the arm's real side by
            // [`hand_targets`].
            beat.set(Joint::Head, 0.08 * a * up);
            beat.set(Joint::Chest, 0.05 * a * up);
        }

        Action::Nod => {
            // Two dips forward, peaking a quarter and three quarters through,
            // inside an envelope so the second is not cut off by the end.
            //
            // **Big enough to read.** A bone here can only turn — a key cannot
            // slide the head down the neck, and the tween would not carry it
            // if it did — so the dip is the chest going with the head, which
            // on a front view is the neck-and-shoulders drop a real nod has.
            let env = hold(u, 0.15, 0.85);
            let dip = 0.5 - 0.5 * (u * TURN * 2.0).cos();
            beat.set(Joint::Head, 0.34 * a * env * dip);
            beat.set(Joint::Chest, 0.07 * a * env * dip);
        }

        Action::HeadShake => {
            // Three tilts — one side, the other, back — each big enough to
            // read, the chest answering a little against them so the head is
            // not a separate toy on the body. Six quick rocks at this length
            // was a shiver, not a no.
            let env = hold(u, 0.12, 0.85);
            let rock = (u * TURN * 1.5).sin();
            beat.set(Joint::Head, 0.18 * a * env * rock);
            beat.set(Joint::Chest, -0.03 * a * env * rock);
        }

        Action::Shrug => {
            // Out, held long enough to read, back.
            let up = hold(u, 0.30, 0.70);
            // Both forearms out to their own sides and up, palms up, the
            // elbows near the body: hand targets, see [`hand_targets`].
            // A slight tilt of the head, and the body lifts — the shoulders
            // rising, which this rig can only say by raising the whole
            // figure. By a share of its height, so it reads on any rig.
            beat.set(Joint::Head, -0.10 * a * up);
            beat.set(Joint::Chest, -0.04 * a * up);
            beat.lift = 0.025 * a * up;
        }

        Action::Namaste => {
            // The hands come together over the first quarter and stay there.
            // The hands meet at the middle of the chest: hand targets, see
            // [`hand_targets`].
            // **The bow comes while the hands are together**, never before
            // they arrive: a bow with the hands still on their way reads as a
            // stumble.
            let p = ((u - 0.25) / 0.55).clamp(0.0, 1.0);
            let bow = (p * std::f64::consts::PI).sin();
            // Small: on a front view a turn of the head or chest is a tilt to
            // one side, not a bend forwards, and a big one reads as a lean.
            beat.set(Joint::Head, 0.10 * a * bow);
            beat.set(Joint::Chest, 0.03 * a * bow);
            beat.set(Joint::Hips, 0.01 * a * bow);
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

/// **How far through a travelling beat the figure is**, given how far through
/// its frames it is.
///
/// # Why this is not just `u`
///
/// A walk written on a linear progress leaves standing still at full stride and
/// arrives at full stride and stops dead. Nobody does that. It is the same tell
/// the camera had before its keys carried an ease, and on a body it is louder:
/// there is no acceleration anywhere in the shot, so the character reads as
/// being *slid* rather than as walking.
///
/// # A trapezoid, not a smoothstep
///
/// Easing the whole move would make a long cross sluggish in the middle, which
/// is not what walking is either. What a person does is get up to speed over
/// about a stride, hold it, and slow over about a stride — a trapezoidal speed
/// profile — and this is that profile integrated into a position.
///
/// # The phase rides on the same number
///
/// This is the part that matters. The returned value drives **both** the travel
/// and the point in the walk cycle, so the stride rate and the ground speed
/// change together. Easing only the travel would leave the legs cycling at a
/// constant rate under a body that was speeding up, which is foot slip — a
/// worse artefact than the one being fixed.
///
/// `ramp` is the share of the beat spent accelerating at each end, clamped to a
/// half so the two ramps cannot overlap.
fn eased_progress(u: f64, ramp: f64) -> f64 {
    let r = ramp.clamp(0.0, 0.5);
    let u = u.clamp(0.0, 1.0);
    if r <= 1e-9 {
        return u;
    }
    // The area under the speed profile: two half-ramps and a flat middle.
    let total = 1.0 - r;
    let travelled = if u < r {
        u * u / (2.0 * r)
    } else if u <= 1.0 - r {
        r / 2.0 + (u - r)
    } else {
        let left = 1.0 - u;
        total - left * left / (2.0 * r)
    };
    (travelled / total).clamp(0.0, 1.0)
}

/// How much of a travelling beat is spent getting up to speed.
///
/// **About half a stride**, which is what it takes, and never more than a
/// quarter of the beat — on a two-step crossing there is no room for a long
/// acceleration and pretending otherwise would make the whole thing sluggish.
fn ramp_for(cycles: f64) -> f64 {
    (0.5 / cycles.max(1.0)).min(0.25)
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

// ---------------------------------------------------------------------------
// Hand targets: arms aimed in a body frame and solved on the rig.
// ---------------------------------------------------------------------------

/// Does this action place its hands, rather than turn its arms by a table?
///
/// The gestures whose whole point is *where the hand is* — at the belt while
/// talking, above the shoulder for a wave, together at the chest for a
/// namaste. Everything else keeps its table (see [`table_pose`]).
fn hand_targeted(action: Action) -> bool {
    matches!(
        action,
        Action::Talk
            | Action::Point
            | Action::Reach
            | Action::React
            | Action::Wave
            | Action::Shrug
            | Action::Namaste
    )
}

/// **One arm's rest geometry**, in the rig's own space — everything the
/// two-bone solve ([`solve_arm`]) needs, and nothing about how the rig is
/// built, so a layer-parented rig can fill one in as well as an armature.
///
/// Angles are radians, y down, positive clockwise on screen, as everywhere in
/// `buzz_rig`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArmRest {
    /// The joint the upper arm turns about.
    pub shoulder: Point,
    /// The world angle, at rest, of whatever the upper arm hangs from (the
    /// chest): the frame the shoulder's own angle is measured in.
    pub parent_angle: f64,
    /// Shoulder to elbow.
    pub upper: f64,
    /// Elbow to wrist (where the hand starts).
    pub fore: f64,
    /// The upper arm's rest angle, relative to its parent.
    pub shoulder_rest: f64,
    /// The forearm's rest angle, relative to the upper arm.
    pub elbow_rest: f64,
    /// Any point on the body's midline (the hips joint).
    pub midline: Point,
    /// Unit vector from the hips toward the head.
    pub up: Vec2,
    /// How far the hand reaches past the wrist, along the forearm: the
    /// length of a wrist bone, or zero for a rig whose forearm drawing is
    /// the hand.
    pub hand: f64,
}

impl ArmRest {
    /// The body-frame unit: the whole arm, upper arm plus forearm.
    pub fn reach(&self) -> f64 {
        self.upper + self.fore
    }

    /// The perpendicular to `up` that a positive turn swings an upright bone
    /// toward.
    fn across(&self) -> Vec2 {
        Vec2::new(-self.up.y, self.up.x)
    }

    /// Where the hand hangs at rest.
    pub fn hand_rest(&self) -> Point {
        let upper = self.parent_angle + self.shoulder_rest;
        let fore = upper + self.elbow_rest;
        self.shoulder
            + Vec2::new(upper.cos(), upper.sin()) * self.upper
            + Vec2::new(fore.cos(), fore.sin()) * self.fore
    }

    /// **Which side of the body this arm is on**: `+1.0` along [`Self::across`]
    /// and `-1.0` against it. Read from the shoulder's offset from the
    /// midline, or — for a rig that hangs both arms from one point on the
    /// midline, as the procedural figure does — from where the hand hangs.
    pub fn side(&self) -> f64 {
        let across = self.across();
        let shoulder = (self.shoulder - self.midline).dot(across);
        let lean = if shoulder.abs() > 0.05 * self.reach() {
            shoulder
        } else {
            (self.hand_rest() - self.midline).dot(across)
        };
        if lean < 0.0 { -1.0 } else { 1.0 }
    }

    /// Unit vector from the midline toward this arm's own side: "out", for
    /// this arm, whichever way round the rig is drawn.
    pub fn out(&self) -> Vec2 {
        self.across() * self.side()
    }

    /// Which way the shoulder turns to swing the hanging arm out from the
    /// body: `+1.0` or `-1.0`.
    pub fn outward(&self) -> f64 {
        let upper = self.parent_angle + self.shoulder_rest;
        let along = Vec2::new(upper.cos(), upper.sin());
        let turn = Vec2::new(-along.y, along.x);
        if turn.dot(self.out()) < 0.0 { -1.0 } else { 1.0 }
    }

    /// How far the shoulder sits out from the midline, along [`Self::out`].
    pub fn lateral(&self) -> f64 {
        (self.shoulder - self.midline).dot(self.out())
    }

    /// A point in the rig's space, as `(out, up)` body-frame coordinates
    /// from the shoulder, in units of [`Self::reach`].
    pub fn coords(&self, point: Point) -> (f64, f64) {
        let d = point - self.shoulder;
        let reach = self.reach().max(1e-9);
        (d.dot(self.out()) / reach, d.dot(self.up) / reach)
    }

    /// The `(out, up)` body-frame target, as a point in the rig's space.
    pub fn point(&self, out: f64, up: f64) -> Point {
        self.shoulder + (self.out() * out + self.up * up) * self.reach()
    }
}

/// **Analytic two-bone IK for one arm**: the shoulder and elbow turns, *from
/// rest*, that put the hand (the forearm's tip) at `(out, up)` in the arm's
/// body frame — `out` along [`ArmRest::out`], `up` toward the head, both in
/// units of [`ArmRest::reach`] measured from the shoulder.
///
/// The law of cosines gives the angle at the shoulder between the line to
/// the target and the upper arm; that leaves two elbows, one either side of
/// the line, and the one that lies further *down and out* (along
/// `out − up`) is taken — the way an elbow hangs when a hand is lifted. A
/// target out of reach is met at full stretch along its direction; one too
/// close to the shoulder for the forearm to fold to is met as close as it can.
///
/// Returns `(shoulder, elbow)`, each relative to the bone's rest angle and
/// wrapped into `-π..=π`: exactly what `Beat::joints` holds.
pub fn solve_arm(arm: &ArmRest, out: f64, up: f64) -> (f64, f64) {
    if arm.upper <= 1e-9 || arm.fore <= 1e-9 {
        return (0.0, 0.0);
    }
    let o = arm.out();
    let mut d = (o * out + arm.up * up) * arm.reach();
    let far = arm.upper + arm.fore;
    let near = (arm.upper - arm.fore).abs();
    if d.hypot() < 1e-9 {
        // On the shoulder itself: no direction, so straight down.
        d = -arm.up;
    }
    let length = d.hypot();
    let direction = d / length;
    let dist = length.clamp(near.max(1e-9), far);

    let cos_a = ((arm.upper * arm.upper + dist * dist - arm.fore * arm.fore)
        / (2.0 * arm.upper * dist))
        .clamp(-1.0, 1.0);
    let spread = cos_a.acos();
    let base = direction.y.atan2(direction.x);
    let bend = o - arm.up;
    let elbow_at = |theta: f64| Vec2::new(theta.cos(), theta.sin()) * arm.upper;
    let (plus, minus) = (base + spread, base - spread);
    let upper = if elbow_at(minus).dot(bend) > elbow_at(plus).dot(bend) + 1e-12 {
        minus
    } else {
        plus
    };
    let hand = direction * dist;
    let forearm = hand - elbow_at(upper);
    let fore = forearm.y.atan2(forearm.x);

    let shoulder_local = upper - arm.parent_angle;
    let elbow_local = fore - upper;
    (
        buzz_rig::wrap_pi(shoulder_local - arm.shoulder_rest),
        buzz_rig::wrap_pi(elbow_local - arm.elbow_rest),
    )
}

/// **The crotch zone**: the hips region between the thighs, below the belt
/// and above the knees, in the rig's own space. A talking hand that wanders
/// into it reads as something nobody meant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrotchZone {
    /// The hips joint, on the midline.
    pub pelvis: Point,
    /// Unit vector from the hips toward the head.
    pub up: Vec2,
    /// How far either side of the midline the thighs reach (the thigh line).
    pub half_width: f64,
    /// The belt's height above the hips joint, along `up`.
    pub belt: f64,
    /// The knees' height along `up` (negative: below the hips).
    pub floor: f64,
}

impl CrotchZone {
    /// Measured off a rig's rest pose: the midline and belt from the hips
    /// bone, the width and depth from the thighs. `None` for a rig without
    /// the eleven biped bones.
    pub fn of(rest: &buzz_rig::Armature) -> Option<Self> {
        if rest.bones.len() <= Joint::KneeR.index() {
            return None;
        }
        let pelvis = rest.head(Joint::Hips.index());
        let up = rest.tip(Joint::Head.index()) - pelvis;
        if up.hypot() < 1e-9 {
            return None;
        }
        let up = up / up.hypot();
        let across = Vec2::new(-up.y, up.x);
        let mut half_width: f64 = 0.0;
        let mut floor: f64 = 0.0;
        for thigh in [Joint::ThighL, Joint::ThighR] {
            for p in [rest.head(thigh.index()), rest.tip(thigh.index())] {
                half_width = half_width.max((p - pelvis).dot(across).abs());
            }
            floor = floor.min((rest.tip(thigh.index()) - pelvis).dot(up));
        }
        let hips = (rest.tip(Joint::Hips.index()) - pelvis).dot(up).abs();
        Some(Self {
            pelvis,
            up,
            half_width,
            belt: 0.4 * hips,
            floor,
        })
    }

    /// Is `point` inside it?
    pub fn contains(&self, point: Point) -> bool {
        let d = point - self.pelvis;
        let across = Vec2::new(-self.up.y, self.up.x);
        let height = d.dot(self.up);
        d.dot(across).abs() < self.half_width && height < self.belt && height > self.floor
    }
}

/// A rig's body, measured once off its rest pose for [`hand_targets`].
struct Body {
    left: ArmRest,
    right: ArmRest,
    zone: CrotchZone,
}

impl Body {
    fn of(rest: &buzz_rig::Armature) -> Option<Self> {
        let rest = rest.at_rest();
        let zone = CrotchZone::of(&rest)?;
        let arm = |shoulder: Joint, elbow: Joint| -> Option<ArmRest> {
            let (si, ei) = (shoulder.index(), elbow.index());
            let s = rest.bones.get(si)?;
            let e = rest.bones.get(ei)?;
            if e.parent != Some(si) {
                return None;
            }
            // The hand: whatever hangs off the forearm's tip.
            let hand = rest
                .bones
                .iter()
                .find(|b| b.parent == Some(ei))
                .map_or(0.0, |b| b.length);
            Some(ArmRest {
                hand,
                shoulder: rest.head(si),
                parent_angle: s.parent.map_or(0.0, |p| rest.world_angle(p)),
                upper: s.length,
                fore: e.length,
                shoulder_rest: s.rest_angle,
                elbow_rest: e.rest_angle,
                midline: zone.pelvis,
                up: zone.up,
            })
        };
        Some(Self {
            left: arm(Joint::ShoulderL, Joint::ElbowL)?,
            right: arm(Joint::ShoulderR, Joint::ElbowR)?,
            zone,
        })
    }

    /// Where a talking hand goes: `height` (in arm lengths) above the belt,
    /// and out past the thigh line, so that neither the hand nor the way to it
    /// ever crosses the crotch zone.
    fn talk_target(&self, arm: &ArmRest, height: f64) -> (f64, f64) {
        let reach = arm.reach();
        let lateral = self.zone.half_width + 0.22 * reach;
        let at = self.zone.pelvis
            + self.zone.up * (self.zone.belt + height * reach)
            + arm.out() * lateral;
        arm.coords(at)
    }
}

/// **The arms of a hand-targeted gesture, solved on this body.**
///
/// Each arm has a target in its own body frame ([`ArmRest`]) and an envelope
/// over the phase. The target is solved ([`solve_arm`]) and the turns from rest
/// are scaled by the envelope and by `amount`, so the gesture starts and ends
/// exactly at rest and halving the amount halves every angle. A turn past a
/// joint's limit is clamped to it. The head, chest and hips of a one-sided
/// gesture (wave, point, reach) are turned toward the active arm's side.
fn hand_targets(
    beat: &mut Beat,
    action: Action,
    phase: f64,
    amount: f64,
    body: &Body,
    rest: &buzz_rig::Armature,
) {
    let a = amount.clamp(0.0, 3.0);
    let u = if action.loops() {
        phase.rem_euclid(1.0)
    } else {
        phase.clamp(0.0, 1.0)
    };
    let t = u * TURN;
    let (left, right) = (&body.left, &body.right);

    // (arm, its two joints, envelope, (out, up) target)
    let mut aims: Vec<(&ArmRest, Joint, Joint, f64, (f64, f64))> = Vec::new();
    let mut one_sided = false;
    // How far the shoulders swing out mid-way, for a gesture whose hands
    // would otherwise pass low across the body.
    let mut flare = 0.0;
    match action {
        Action::Talk => {
            // One hand leading and the other following — two arms moving
            // identically read as a swimming stroke — to the belt and to the
            // lower chest.
            let lead = (2.0 * t).sin().max(0.0);
            let follow = (2.0 * t + 1.7).sin().max(0.0);
            aims.push((left, Joint::ShoulderL, Joint::ElbowL, lead, body.talk_target(left, 0.12)));
            aims.push((right, Joint::ShoulderR, Joint::ElbowR, follow, body.talk_target(right, 0.30)));
        }
        Action::Point | Action::Reach => {
            let env = hold(u, 0.30, 0.75);
            one_sided = true;
            if action == Action::Reach {
                aims.push((right, Joint::ShoulderR, Joint::ElbowR, env, (0.70, 0.45)));
                // The far arm swings out as a counterweight.
                aims.push((left, Joint::ShoulderL, Joint::ElbowL, env, (0.55, -0.75)));
            } else {
                aims.push((right, Joint::ShoulderR, Joint::ElbowR, env, (0.95, 0.0)));
            }
        }
        Action::React => {
            // Up between the character and whatever it was, on the recoil
            // only: the settle back past rest is the body's, not the arms'.
            let swing = (-4.0 * u).exp() * (u * TURN * 1.35).sin();
            // Faded out over the last quarter, so the arms are home by the
            // end however much of the swing is still ringing.
            let env = (swing * 1.9).clamp(0.0, 1.0) * hold(u, 0.0, 0.75);
            aims.push((left, Joint::ShoulderL, Joint::ElbowL, env, (0.40, 0.30)));
            aims.push((right, Joint::ShoulderR, Joint::ElbowR, env, (0.40, 0.30)));
        }
        Action::Wave => {
            let env = hold(u, 0.22, 0.80);
            let wag = (u * TURN * 3.0).sin();
            one_sided = true;
            aims.push((right, Joint::ShoulderR, Joint::ElbowR, env, (0.40 + 0.12 * wag, 0.82)));
        }
        Action::Shrug => {
            let env = hold(u, 0.30, 0.70);
            aims.push((left, Joint::ShoulderL, Joint::ElbowL, env, (0.55, 0.10)));
            aims.push((right, Joint::ShoulderR, Joint::ElbowR, env, (0.55, 0.10)));
        }
        Action::Namaste => {
            let env = hold(u, 0.25, 0.80);
            for (arm, s, e) in [
                (left, Joint::ShoulderL, Joint::ElbowL),
                (right, Joint::ShoulderR, Joint::ElbowR),
            ] {
                // Each wrist stops a hand's length short of the midline, so
                // the two hands meet there rather than crossing.
                let reach = arm.reach().max(1e-9);
                let short = (0.08 * reach).max(0.9 * arm.hand);
                let to_midline = (short - arm.lateral()) / reach;
                aims.push((arm, s, e, env, (to_midline, NAMASTE_UP)));
            }
            // **The elbows lift out on the way**, and come back in once the
            // hands are up: a forearm folding inwards from a hanging arm
            // sweeps the hand across the groin halfway through.
            flare = 0.70;
        }
        _ => {}
    }

    for (arm, shoulder, elbow, env, (out, up)) in aims {
        let (ds, de) = solve_arm(arm, out, up);
        let k = a * env;
        let lift = flare * (env * std::f64::consts::PI).sin() * arm.outward();
        beat.set(shoulder, within_limits(rest, shoulder, ds * k + lift * a));
        beat.set(elbow, within_limits(rest, elbow, de * k));
    }

    if one_sided {
        let side = right.side();
        for joint in [Joint::Head, Joint::Chest, Joint::Hips] {
            if let Some(slot) = beat.joints.get_mut(joint.index()) {
                *slot *= side;
            }
        }
    }
}

/// How far below the shoulders the hands meet in a namaste, in arm lengths:
/// the middle of the chest.
const NAMASTE_UP: f64 = -0.45;

/// `delta` from the rest angle of `joint`, clamped into the joint's limits.
fn within_limits(rest: &buzz_rig::Armature, joint: Joint, delta: f64) -> f64 {
    let Some(bone) = rest.bones.get(joint.index()) else {
        return delta;
    };
    let Some(limits) = bone.limits else {
        return delta;
    };
    let asked = bone.rest_angle + delta;
    let clamped = limits.clamp(asked);
    if buzz_rig::wrap_pi(clamped - asked).abs() <= 1e-12 {
        delta
    } else {
        buzz_rig::wrap_pi(clamped - bone.rest_angle)
    }
}

/// **Do this rig's arms hang the other way round from the puppet's?**
///
/// [`pose_at`] is written for the procedural figure (`figure::build`), which
/// hangs `Shoulder L` on the picture's *left* of its own drawing. An Ep1 Cast
/// rig is a front view, and hangs `Shoulder L` — the character's own left — on
/// the picture's *right*. Same bones, same order, mirror-image arms: the angle
/// that takes the puppet's right arm out and up takes a Cast rig's right arm
/// in across the body.
///
/// **The rule:** in the rest pose, in the rig's *own* coordinates, the end of
/// `Elbow L` (where the left arm's chain ends) lies to the right of the end of
/// `Elbow R`. The object's transform is deliberately left out of it: a figure
/// facing left is the same rig behind a mirror, and the mirror flips the
/// performance along with the drawing — asking in world space would flip it a
/// second time and send a left-facing puppet's point backwards.
///
/// A rig without both arms reads as the puppet's convention.
pub fn arms_mirrored(rest: &buzz_rig::Armature) -> bool {
    if rest.bones.len() <= Joint::ElbowR.index() {
        return false;
    }
    rest.tip(Joint::ElbowL.index()).x > rest.tip(Joint::ElbowR.index()).x
}

/// **Reflect the arms of a beat**, for a rig whose arms hang the other way
/// round ([`arms_mirrored`]). A reflection negates every turn in a chain; the
/// sides stay as they are, so `Shoulder R` is still the character's right arm.
fn mirror_arms(beat: &mut Beat) {
    for joint in [
        Joint::ShoulderL,
        Joint::ElbowL,
        Joint::ShoulderR,
        Joint::ElbowR,
    ] {
        if let Some(slot) = beat.joints.get_mut(joint.index()) {
            *slot = -*slot;
        }
    }
}

// ---------------------------------------------------------------------------
// The view a rig is drawn in, and the walk of a front (or back) view.
// ---------------------------------------------------------------------------

/// **Which way a rig is drawn**, as far as its legs are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigView {
    /// Seen from the side (or near enough): the legs hang from one place and
    /// swing forward and back past each other — the procedural puppet.
    Side,
    /// Seen from the front or the back: the legs stand side by side, and a
    /// stride that swings them forward and back crosses them.
    Front,
}

/// How far apart, across the body, the two thigh heads have to be — as a
/// share of the leg's length (thigh plus shin) — for the legs to be standing
/// side by side. The puppet's share one point (0.0); every Ep1 Cast rig,
/// front, three-quarter and back, measures 0.21 to 0.41.
const FRONT_SPREAD: f64 = 0.12;

/// **The view a rig is drawn in, read off its rest geometry** — no names.
///
/// Front (or back) when the two thigh heads are spread across the body (the
/// line square to hips-to-head) by more than [`FRONT_SPREAD`] of the leg's
/// length: legs side by side. Otherwise, and for a rig without the eleven
/// biped bones, side.
pub fn rig_view(rest: &buzz_rig::Armature) -> RigView {
    if rest.bones.len() <= Joint::KneeR.index() {
        return RigView::Side;
    }
    let rest = rest.at_rest();
    let pelvis = rest.head(Joint::Hips.index());
    let up = rest.tip(Joint::Head.index()) - pelvis;
    if up.hypot() < 1e-9 {
        return RigView::Side;
    }
    let up = up / up.hypot();
    let across = Vec2::new(-up.y, up.x);
    let spread = (rest.head(Joint::ThighL.index()) - rest.head(Joint::ThighR.index()))
        .dot(across)
        .abs();
    let leg = |thigh: Joint, knee: Joint| {
        rest.bones[thigh.index()].length + rest.bones[knee.index()].length
    };
    let leg = 0.5 * (leg(Joint::ThighL, Joint::KneeL) + leg(Joint::ThighR, Joint::KneeR));
    if leg > 1e-9 && spread > FRONT_SPREAD * leg {
        RigView::Front
    } else {
        RigView::Side
    }
}

/// **Walk (or run) for a rig seen from the front or the back**
/// ([`RigView::Front`]).
///
/// Seen from the front, a step is a weight shift, not a stride: the figure
/// travels by the performance's `distance` as before, but relative to the
/// body each foot lifts and comes down under its own hip.
///
/// One cycle is two steps. The left leg swings through the first half and
/// the right the second. For each:
///
/// - **the swing leg lifts**: the thigh turns a little *inward*, toward the
///   midline, and the knee turns back by enough to put the foot back under
///   its own hip — so the knee shows the bend and the foot never travels
///   toward the other one;
/// - **the hips sway toward the stance leg** with a small tilt, the chest
///   counter-tilts past them and the head stays nearly level;
/// - **the body rises twice a cycle**, at each passing position, by a share
///   of the rig's height ([`Beat::lift`]);
/// - **the arms swing a little in and out at the sides** — hand targets solved
///   on the rig ([`solve_arm`]), the arm opposite the lifting leg coming
///   forward (in and up), the other going back (out).
///
/// A run is the same with a higher lift, a bigger sway and bob, and the
/// forearms carried bent.
///
/// # Why inward, and why so little
///
/// A skirt, a saree or a pair of trousers is split across the two thigh
/// parts and the two shin parts. Turned *outward*, a thigh opens a slit
/// between them that shows the background (Woman1's saree at six degrees:
/// a wedge as wide as a tenth of the thigh at the knee); turned *inward* it
/// laps over its neighbour instead, and a lap of the same cloth is not seen.
/// Even a lap shows the cut edge of the part on top, so the turn stays at
/// about three and a half degrees in a walk; the walk reads from the bob,
/// the sway and the arms as much as from the legs.
fn front_walk(action: Action, phase: f64, amount: f64, rest: &buzz_rig::Armature) -> Beat {
    let mut beat = Beat::rest(Joint::ALL.len());
    let rest = rest.at_rest();
    let running = action == Action::Run;
    let a = amount.clamp(0.0, 3.0);
    let t = phase.rem_euclid(1.0) * TURN;
    let s = t.sin();

    // How far the thigh turns in at the top of the lift, the share of the
    // rig's height the body rises at each passing position, and the hips'
    // tilt over the stance leg.
    let thigh_in = if running { 0.10 } else { 0.06 } * a;
    let bob = if running { 0.022 } else { 0.012 } * a;
    let sway = if running { 0.04 } else { 0.03 } * a;

    let pelvis = rest.head(Joint::Hips.index());
    let up = rest.tip(Joint::Head.index()) - pelvis;
    let up = if up.hypot() < 1e-9 {
        Vec2::new(0.0, -1.0)
    } else {
        up / up.hypot()
    };
    let across = Vec2::new(-up.y, up.x);
    // The way a positive turn swings bone `i`'s tip, at rest.
    let swing_of = |i: usize| {
        let w = rest.world_angle(i);
        Vec2::new(-w.sin(), w.cos())
    };
    // `+1.0` when a positive turn of bone `i` swings it toward `point`'s
    // side of the midline, else `-1.0`.
    let toward = |i: usize, point: Point| {
        let side = (point - pelvis).dot(across);
        if swing_of(i).dot(across) * side < 0.0 { -1.0 } else { 1.0 }
    };

    // Smooth 0..1 envelopes: the left leg lifts in the first half of the
    // cycle, the right in the second.
    let smooth = |x: f64| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    let lift_l = smooth(s.max(0.0));
    let lift_r = smooth((-s).max(0.0));

    for (thigh, knee, env) in [
        (Joint::ThighL, Joint::KneeL, lift_l),
        (Joint::ThighR, Joint::KneeR, lift_r),
    ] {
        let (ti, ki) = (thigh.index(), knee.index());
        let out = toward(ti, rest.head(ti));
        let alpha = -thigh_in * env;
        // Turned back by enough that the foot comes down under the hip:
        // the thigh's sideways reach undone by the shin.
        let (upper, lower) = (rest.bones[ti].length, rest.bones[ki].length.max(1e-9));
        let back = alpha + (upper * alpha.sin() / lower).clamp(-1.0, 1.0).asin();
        beat.set(thigh, out * alpha);
        beat.set(knee, -out * back);
    }

    // The weight goes over the stance leg — the right while the left lifts.
    let stance = rest.head(Joint::ThighR.index());
    let hips = sway * s * toward(Joint::Hips.index(), stance);
    beat.set(Joint::Hips, hips);
    // Kept close to the hips' tilt: the chest turning far against the hips
    // opens the seam between the two parts at the waist.
    beat.set(Joint::Chest, -1.3 * hips);
    beat.set(Joint::Head, 0.25 * hips);

    // Up at each passing position (the foot under the body), down at each
    // contact.
    beat.lift = bob * s * s;

    // The arms, solved on the rig: forward is a little in and up, back a
    // little out. The arm opposite the lifting leg comes forward. Small:
    // an arm swung out uncovers whatever the artwork left under it.
    if let Some(body) = Body::of(&rest) {
        // (in-out swing, forward rise, forearms carried: out, up), in arm
        // lengths from where the hand hangs.
        let (out_swing, up_swing, carry_out, carry_up) = if running {
            (0.05, 0.03, -0.04, 0.12)
        } else {
            (0.03, 0.01, 0.0, 0.0)
        };
        for (arm, shoulder, elbow, forward) in [
            (&body.left, Joint::ShoulderL, Joint::ElbowL, -s),
            (&body.right, Joint::ShoulderR, Joint::ElbowR, s),
        ] {
            let (o0, u0) = arm.coords(arm.hand_rest());
            let (sb, eb) = solve_arm(arm, o0, u0);
            let o = o0 + a * (carry_out - out_swing * forward);
            let u = u0 + a * (carry_up + up_swing * forward.max(0.0) + 0.2 * up_swing * (-forward).max(0.0));
            let (st, et) = solve_arm(arm, o, u);
            beat.set(shoulder, within_limits(&rest, shoulder, buzz_rig::wrap_pi(st - sb)));
            beat.set(elbow, within_limits(&rest, elbow, buzz_rig::wrap_pi(et - eb)));
        }
    }
    beat
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
    // so a walk starts where the animator put the character. For a layer rig,
    // where its root part has been moved from rest (`layer_rig::placement`).
    let placed =
        crate::layer_rig::placement(scene, object).ok_or(PerformError::NoObject)?;
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
    // **Which rig**, asked once (`layer_rig::rig_of`): a layer rig is posed
    // part by part ([`apply_layers`]); an armature carries on below.
    match crate::layer_rig::rig_of(scene, object) {
        Some(crate::layer_rig::CharacterRig::Layers(view)) => {
            return apply_layers(scene, &view, performance, placed);
        }
        Some(crate::layer_rig::CharacterRig::Armature(_)) => {}
        None if scene.find_object(object).is_some() => return Err(PerformError::NotRigged),
        None => return Err(PerformError::NoObject),
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
    let at_rest = rig.armature.at_rest();
    let rest = at_rest.pose();
    // How tall the rig is, read off the rest pose once. Which way round its
    // arms hang, and where its hands can reach, [`pose_on`] reads off the
    // same rest pose.
    let height = at_rest.bounds().map_or(0.0, |b| b.height());

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
        let linear = (frame - performance.frames.start) as f64 / span;
        // **Travelling beats accelerate and settle; everything else does not.**
        //
        // A walk and a run start from a standstill and arrive at one, and a
        // linear progress is what makes a generated one read as sliding. A
        // talk and an idle are ambient — they have no beginning to leave and no
        // end to arrive at, and easing them would only make them listless at
        // both ends. The one-shots already carry their own pacing inside
        // `pose_at`, and easing them here would apply it twice.
        let progress = if performance.action.travels() {
            eased_progress(linear, ramp_for(cycles))
        } else {
            linear
        };
        let beat = pose_on(
            performance.action,
            progress * cycles,
            performance.amount,
            &at_rest,
        );
        // A lift is a share of the rig's height; everything else is already
        // in units, and is left exactly as it was.
        let rise = if beat.lift == 0.0 {
            beat.offset.y
        } else {
            beat.offset.y - beat.lift * height
        };

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
            target.transform = placed * Affine::translate((travel, rise));
        });

        // A tween across the gap, so animating on twos is smooth on ones. The
        // last key gets none: there is nothing after it to tween to, and a
        // tween pointing at nothing holds its keyframe anyway.
        if frame + step < performance.frames.end {
            scene.update_layer(layer, |l| {
                l.frames.set_tween(frame, buzz_scene::Tween::motion());
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

/// [`apply_from`] for a **layer rig** (see [`crate::layer_rig`]).
///
/// The same beats, from [`pose_on`] on an armature built from the layers' rest
/// geometry ([`crate::layer_rig::rest_armature`]), on the same frames with the
/// same step and tween spacing. Each keyed frame, every part layer gets the
/// transform that puts its part where that armature's bone puts it — the
/// bone's turn about its own joint, with whatever the layer already inherits
/// from the layers it follows taken off, so a child does not turn twice. The
/// travel and the rise move the whole rig from `placed`, the rig's rest space
/// to the stage ([`crate::layer_rig::placement`]); the root carries them and
/// the rest follow. Keys are classic tweens.
fn apply_layers(
    scene: &mut Scene,
    view: &crate::layer_rig::LayerRigView,
    performance: &Performance,
    placed: Affine,
) -> Result<PerformReport, PerformError> {
    crate::layer_rig::record_rests(scene, view);
    let (at_rest, parts) =
        crate::layer_rig::rest_armature(scene, view).ok_or(PerformError::NotRigged)?;
    let rest = at_rest.pose();
    let height = at_rest.bounds().map_or(0.0, |b| b.height());

    let fps = scene.stage().frame_rate.max(1.0);
    let cycles = performance.cycles(fps);
    let span = performance.frames.len().max(1) as f64;
    let step = performance.step.max(1);

    let last = performance.frames.end.saturating_sub(1);
    for part in &parts {
        scene.update_layer(part.layer, |l| {
            if l.frames.length() <= last {
                l.frames.insert_frame(last);
            }
        });
    }

    let mut written = 0u32;
    let mut frame = performance.frames.start;
    while frame < performance.frames.end {
        let linear = (frame - performance.frames.start) as f64 / span;
        let progress = if performance.action.travels() {
            eased_progress(linear, ramp_for(cycles))
        } else {
            linear
        };
        let beat = pose_on(
            performance.action,
            progress * cycles,
            performance.amount,
            &at_rest,
        );
        let rise = if beat.lift == 0.0 {
            beat.offset.y
        } else {
            beat.offset.y - beat.lift * height
        };
        let travel = if performance.action.travels() {
            performance.distance * progress
        } else {
            0.0
        };

        // The pose exactly as the armature path sets it: rest plus the beat,
        // through `set_pose`.
        let mut posed = at_rest.clone();
        let angles: Vec<f64> = rest
            .iter()
            .zip(beat.joints.iter())
            .map(|(rest, delta)| rest + delta)
            .collect();
        posed.set_pose(&angles);
        let base = placed * Affine::translate((travel, rise));
        let turns: Vec<Affine> = (0..posed.len()).map(|i| posed.pose_transform(i)).collect();

        // Followed layers first (`parts` is in that order), so what each
        // layer inherits at this frame is already written.
        for part in &parts {
            let Some(turn) = turns.get(part.slot) else {
                continue;
            };
            let world = base * *turn * part.rest;
            scene.ensure_keyframe(part.layer, frame);
            let inherited = scene.layers().inherited_transform(part.layer, frame);
            let c = inherited.as_coeffs();
            let own = if (c[0] * c[3] - c[1] * c[2]).abs() > 1e-12 {
                inherited.inverse() * world
            } else {
                world
            };
            scene.update_object_at(frame, part.object, |target| target.transform = own);
            if frame + step < performance.frames.end {
                scene.update_layer(part.layer, |l| {
                    l.frames.set_tween(frame, buzz_scene::Tween::classic());
                });
            }
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

    // -- handedness ---------------------------------------------------------

    /// The procedural figure, reflected about its own vertical axis and made
    /// that its rest pose: the same bones in the same order, `Shoulder L` now
    /// hanging on the picture's right — an Ep1 Cast rig's handedness.
    fn mirrored(figure: &buzz_scene::Object) -> buzz_scene::Object {
        let mut out = figure.clone();
        let ObjectKind::Armature(rig) = &mut out.kind else {
            panic!("not rigged");
        };
        let a = &mut rig.armature;
        a.root.x = -a.root.x;
        for bone in &mut a.bones {
            bone.angle = if bone.parent.is_some() {
                -bone.angle
            } else {
                std::f64::consts::PI - bone.angle
            };
            bone.offset.x = -bone.offset.x;
        }
        a.set_rest_here();
        out
    }

    /// Every key's pose and transform, frame by frame, after `apply_from`.
    fn keys(scene: &Scene, layer: buzz_scene::LayerId, id: ObjectId, frames: u32) -> Vec<(Vec<f64>, [f64; 6])> {
        (0..frames)
            .step_by(2)
            .map(|f| {
                let objects = scene.layers().get(layer).expect("the layer").frames.resolved_at(f);
                let o = objects.iter().find(|o| o.id == id).expect("posed");
                let ObjectKind::Armature(rig) = &o.kind else {
                    panic!("not rigged");
                };
                (rig.armature.pose(), o.transform.as_coeffs())
            })
            .collect()
    }

    /// A figure (or its mirror) on a fresh layer, performed.
    fn performed(
        mirror: bool,
        action: Action,
        frames: u32,
        distance: f64,
    ) -> (Vec<f64>, Vec<(Vec<f64>, [f64; 6])>) {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Cast", LayerKind::Normal);
        let id = scene.next_object_id();
        let mut figure = figure::build(&FigureSpec::default(), id, || scene_id(&mut scene));
        if mirror {
            figure = mirrored(&figure);
        }
        let rest = match &figure.kind {
            ObjectKind::Armature(rig) => rig.armature.at_rest().pose(),
            _ => unreachable!(),
        };
        let id = scene.add_object(layer, figure).expect("on the layer");
        let performance = Performance {
            distance,
            ..Performance::new(action, 0..frames)
        };
        apply_from(&mut scene, id, &performance, Affine::IDENTITY).expect("performs");
        (rest, keys(&scene, layer, id, frames))
    }

    /// **The rule reads the rig, not the stage.** The puppet is the canonical
    /// handedness facing either way — facing left is a mirror on the object,
    /// which flips the performance with the drawing — and its reflection is
    /// the other.
    #[test]
    fn handedness_is_read_off_the_rest_pose_in_the_rigs_own_space() {
        let rest_of = |o: &buzz_scene::Object| match &o.kind {
            ObjectKind::Armature(rig) => rig.armature.at_rest(),
            _ => panic!("not rigged"),
        };
        let mut n = 0u64;
        let mut next = || {
            n += 1;
            ObjectId(n)
        };
        let right = figure::build(&FigureSpec::default(), ObjectId(1000), &mut next);
        let left = figure::build(
            &FigureSpec {
                facing: -1.0,
                ..FigureSpec::default()
            },
            ObjectId(1001),
            &mut next,
        );
        assert!(!arms_mirrored(&rest_of(&right)), "the puppet is the canonical rig");
        assert!(
            !arms_mirrored(&rest_of(&left)),
            "a puppet facing left is the same rig behind a mirror"
        );
        assert!(
            arms_mirrored(&rest_of(&mirrored(&right))),
            "a rig with Shoulder L on the picture's right is the other way round"
        );
    }

    /// **A mirrored rig gets the mirror image of every one-shot.** Each arm
    /// angle the canonical figure is keyed with arrives negated on its
    /// reflection — the same arm, the other way round. The spine of a
    /// one-sided gesture (wave, point, reach) leans toward the active arm, so
    /// on the reflection it is negated too; everything else (the spine of
    /// every other action, the legs, the lift) arrives as it was.
    #[test]
    fn a_mirrored_rig_gets_the_negated_arm_angles_of_every_one_shot() {
        let arms = [
            Joint::ShoulderL,
            Joint::ElbowL,
            Joint::ShoulderR,
            Joint::ElbowR,
        ];
        let spine = [Joint::Hips, Joint::Chest, Joint::Head];
        for action in [
            Action::Sit,
            Action::Stand,
            Action::Turn,
            Action::Point,
            Action::Reach,
            Action::React,
            Action::Wave,
            Action::Nod,
            Action::HeadShake,
            Action::Shrug,
            Action::Namaste,
            Action::Talk,
        ] {
            let one_sided = matches!(action, Action::Wave | Action::Point | Action::Reach);
            let (rest, canonical) = performed(false, action, 36, 0.0);
            let (rest_m, mirror) = performed(true, action, 36, 0.0);
            let mut moved = false;
            for (k, ((pose, tf), (pose_m, tf_m))) in canonical.iter().zip(mirror.iter()).enumerate() {
                for joint in Joint::ALL {
                    let i = joint.index();
                    let delta = pose[i] - rest[i];
                    let delta_m = pose_m[i] - rest_m[i];
                    let negated = arms.contains(&joint) || (one_sided && spine.contains(&joint));
                    let want = if negated { -delta } else { delta };
                    assert!(
                        (delta_m - want).abs() < 1e-9,
                        "{action:?} key {k}: {joint:?} moved {delta_m} on the mirrored rig, wanted {want}"
                    );
                    if arms.contains(&joint) && delta.abs() > 0.05 {
                        moved = true;
                    }
                }
                for (c, c_m) in tf.iter().zip(tf_m.iter()) {
                    assert!((c - c_m).abs() < 1e-6, "{action:?} key {k}: transform {tf:?} against {tf_m:?}");
                }
            }
            if action != Action::Nod && action != Action::HeadShake && action != Action::Turn {
                assert!(moved, "{action:?} never moved an arm, so this proved nothing");
            }
        }
    }

    /// **The table actions perform exactly as they did before hand targets.**
    ///
    /// A hash of every key's pose and transform, on the puppet and on its
    /// mirror image, for each action that keeps its pose table, captured from
    /// the build before the hand-targeted gestures were added. Only the
    /// gestures that place a hand (talk, point, reach, react, wave, shrug,
    /// namaste) were allowed to change.
    #[test]
    fn the_table_actions_keys_are_unchanged() {
        for (mirror, action, frames, distance, want) in [
            (false, Action::Walk, 48u32, 200.0, 0x0191_b8ad_f21c_8f49_u64),
            (false, Action::Run, 48, 200.0, 0x3837_e021_98e9_d165),
            (false, Action::Idle, 48, 0.0, 0xe1d9_44b4_d253_e895),
            (false, Action::Sit, 36, 0.0, 0xc370_36e4_5f42_5ddf),
            (false, Action::Stand, 36, 0.0, 0x226d_bdcb_62c6_f81d),
            (false, Action::Turn, 36, 0.0, 0x44a3_6fb2_59cf_22f5),
            (false, Action::Nod, 36, 0.0, 0xda04_adbe_9eaf_110d),
            (false, Action::HeadShake, 36, 0.0, 0xfb35_cf49_6684_0c0d),
            (true, Action::Walk, 48, 200.0, 0xa7cd_257d_db31_0780),
            (true, Action::Run, 48, 200.0, 0x76e2_bfb4_645e_25cd),
            (true, Action::Idle, 48, 0.0, 0x18b2_d8ab_8269_4212),
            (true, Action::Sit, 36, 0.0, 0xbcd4_a417_ecee_2bac),
            (true, Action::Stand, 36, 0.0, 0x77f6_2429_b63b_1de3),
            (true, Action::Turn, 36, 0.0, 0xad38_2ae7_fc70_834c),
            (true, Action::Nod, 36, 0.0, 0x76a0_5400_3d13_f6fd),
            (true, Action::HeadShake, 36, 0.0, 0x9e47_ed7b_ec16_9bf1),
        ] {
            let (_, keyed) = performed(mirror, action, frames, distance);
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            let mut mix = |v: f64| {
                for b in v.to_bits().to_le_bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(0x0100_0000_01b3);
                }
            };
            for (pose, tf) in &keyed {
                pose.iter().for_each(|v| mix(*v));
                tf.iter().for_each(|v| mix(*v));
            }
            assert_eq!(h, want, "{action:?} keys changed (mirrored: {mirror})");
        }
    }

    // -- hand targets -------------------------------------------------------

    /// Raju.buzz's rest pose (Ep1 Cast, `RigPattern::biped_full`, a front
    /// view): `(name, parent, head, length, world angle in degrees)`, the
    /// shoulders and thighs offset from the joints a plain chain would put
    /// them at, and a wrist hanging off each forearm.
    fn cast_like() -> buzz_rig::Armature {
        let table: [(&str, Option<usize>, (f64, f64), f64, f64); 13] = [
            ("Hips", None, (545.8, 698.5), 164.5, -91.1),
            ("Chest", Some(0), (542.5, 534.0), 164.5, -91.1),
            ("Head", Some(1), (539.2, 369.5), 309.2, -90.0),
            ("Shoulder L", Some(1), (641.0, 369.5), 153.3, 75.0),
            ("Elbow L", Some(3), (680.7, 517.6), 147.1, 71.5),
            ("Shoulder R", Some(1), (437.4, 369.5), 147.4, -255.0),
            ("Elbow R", Some(5), (399.3, 511.9), 145.2, -252.1),
            ("Thigh L", None, (609.0, 697.6), 301.7, 88.9),
            ("Knee L", Some(7), (614.6, 999.3), 273.6, 87.6),
            ("Thigh R", None, (482.7, 699.5), 304.5, 94.6),
            ("Knee R", Some(9), (458.2, 1003.0), 272.3, 94.4),
            ("Wrist L", Some(4), (727.3, 657.1), 77.6, 71.2),
            ("Wrist R", Some(6), (354.6, 650.0), 84.7, -259.2),
        ];
        let mut a = buzz_rig::Armature::new(Point::new(545.8, 698.5));
        for (name, parent, head, length, world) in table {
            let world = world.to_radians();
            let head = Point::new(head.0, head.1);
            let bone = match parent {
                Some(p) => {
                    let pw = a.world_angle(p);
                    let off = head - a.tip(p);
                    let mut b = buzz_rig::Bone::new(name, Some(p), length, buzz_rig::wrap_pi(world - pw));
                    b.offset = Vec2::new(
                        off.x * pw.cos() + off.y * pw.sin(),
                        -off.x * pw.sin() + off.y * pw.cos(),
                    );
                    b
                }
                None => {
                    let mut b = buzz_rig::Bone::new(name, None, length, world);
                    b.offset = head - a.root;
                    b
                }
            };
            a.push(bone);
        }
        a
    }

    /// `arm` reflected about the vertical through x = 0.
    fn reflected(arm: &ArmRest) -> ArmRest {
        ArmRest {
            shoulder: Point::new(-arm.shoulder.x, arm.shoulder.y),
            parent_angle: std::f64::consts::PI - arm.parent_angle,
            shoulder_rest: -arm.shoulder_rest,
            elbow_rest: -arm.elbow_rest,
            midline: Point::new(-arm.midline.x, arm.midline.y),
            up: Vec2::new(-arm.up.x, arm.up.y),
            ..*arm
        }
    }

    /// Where the elbow and the hand land for the solved turns.
    fn reach_for(arm: &ArmRest, (ds, de): (f64, f64)) -> (Point, Point) {
        let upper = arm.parent_angle + arm.shoulder_rest + ds;
        let fore = upper + arm.elbow_rest + de;
        let elbow = arm.shoulder + Vec2::new(upper.cos(), upper.sin()) * arm.upper;
        (elbow, elbow + Vec2::new(fore.cos(), fore.sin()) * arm.fore)
    }

    /// Every arm the solver is tested on: both arms of the puppet (shoulders
    /// on the midline) and of a Cast rig (shoulders off it), and the
    /// reflection of each.
    fn arms_under_test() -> Vec<ArmRest> {
        let mut arms = Vec::new();
        for rest in [canonical_rest().clone(), cast_like()] {
            let body = Body::of(&rest).expect("a biped");
            for arm in [body.left, body.right] {
                arms.push(arm);
                arms.push(reflected(&arm));
            }
        }
        arms
    }

    const TARGETS: [(f64, f64); 8] = [
        (0.25, 0.85),
        (0.55, 0.10),
        (0.95, 0.0),
        (0.70, 0.45),
        (0.40, 0.30),
        (-0.30, -0.33),
        (0.10, -0.70),
        (0.35, -0.55),
    ];

    /// **The solver puts the hand on a reachable target**, within half a
    /// percent of the arm's length, and meets an unreachable one at full
    /// stretch along its direction.
    #[test]
    fn the_arm_solver_reaches_a_reachable_target() {
        for arm in arms_under_test() {
            let reach = arm.reach();
            for (out, up) in TARGETS {
                let (_, hand) = reach_for(&arm, solve_arm(&arm, out, up));
                let want = arm.point(out, up);
                assert!(
                    (hand - want).hypot() < 0.005 * reach,
                    "{arm:?} aimed at ({out}, {up}): off by {}",
                    (hand - want).hypot() / reach
                );
            }
            let (_, hand) = reach_for(&arm, solve_arm(&arm, 2.0, 0.0));
            let want = arm.point(1.0, 0.0);
            assert!(
                (hand - want).hypot() < 0.005 * reach,
                "out of reach should mean full stretch along the direction: off by {}",
                (hand - want).hypot() / reach
            );
        }
    }

    /// **The elbow is on the down-and-out side** of the line from shoulder
    /// to hand: of the two elbows that reach a target, the one further along
    /// `out − up`.
    #[test]
    fn the_solved_elbow_hangs_down_and_out() {
        for arm in arms_under_test() {
            let bend = arm.out() - arm.up;
            for (out, up) in TARGETS {
                let (elbow, hand) = reach_for(&arm, solve_arm(&arm, out, up));
                // The other elbow: this one reflected across shoulder–hand.
                let line = (hand - arm.shoulder) / (hand - arm.shoulder).hypot();
                let e = elbow - arm.shoulder;
                let other = line * (2.0 * e.dot(line)) - e;
                assert!(
                    e.dot(bend) >= other.dot(bend) - 1e-9,
                    "{arm:?} at ({out}, {up}): the elbow went up/in ({} against {})",
                    e.dot(bend),
                    other.dot(bend)
                );
            }
        }
    }

    /// **A rig and its mirror image get the same pose, mirrored**: every
    /// turn the solver returns for the reflection is the negation of the
    /// original's, so mapped back it is identical.
    #[test]
    fn the_arm_solver_is_handedness_free() {
        for arm in arms_under_test() {
            let mirror = reflected(&arm);
            for (out, up) in TARGETS {
                let (ds, de) = solve_arm(&arm, out, up);
                let (ms, me) = solve_arm(&mirror, out, up);
                assert!(
                    (ms + ds).abs() < 1e-9 && (me + de).abs() < 1e-9,
                    "({out}, {up}): ({ds}, {de}) against the mirror's ({ms}, {me})"
                );
            }
        }
    }

    /// **A talking hand never drops into the crotch zone** — not the wrist,
    /// not the tip of the hand — over a whole cycle, on the puppet and on a
    /// Cast rig; and the hands really do come up to the belt, so this is not
    /// passing on arms that never moved.
    #[test]
    fn a_talking_hand_never_enters_the_crotch_zone() {
        for (name, rest) in [("puppet", canonical_rest().clone()), ("cast", cast_like())] {
            let zone = CrotchZone::of(&rest).expect("a biped");
            let base = rest.pose();
            let mut highest = [f64::NEG_INFINITY; 2];
            for i in 0..=400 {
                let phase = i as f64 / 400.0;
                let beat = pose_on(Action::Talk, phase, 1.0, &rest);
                let pose: Vec<f64> = base
                    .iter()
                    .enumerate()
                    .map(|(k, r)| r + beat.joints.get(k).copied().unwrap_or(0.0))
                    .collect();
                let mut posed = rest.clone();
                posed.set_pose(&pose);
                for (side, elbow) in [Joint::ElbowL, Joint::ElbowR].into_iter().enumerate() {
                    let wrist = posed.tip(elbow.index());
                    let mut points = vec![wrist];
                    for (k, bone) in posed.bones.iter().enumerate() {
                        if bone.parent == Some(elbow.index()) {
                            points.push(posed.tip(k));
                        }
                    }
                    for p in points {
                        assert!(
                            !zone.contains(p),
                            "{name}: {elbow:?}'s hand is in the crotch zone at phase {phase}: {p:?}"
                        );
                    }
                    highest[side] = highest[side].max((wrist - zone.pelvis).dot(zone.up));
                }
            }
            for (side, top) in highest.iter().enumerate() {
                assert!(
                    *top > zone.belt,
                    "{name}: hand {side} never came up to the belt ({top} against {})",
                    zone.belt
                );
            }
        }
    }

    // -- the view a rig is drawn in, and the front-view walk -----------------

    /// `rig` reflected about the vertical through x = 0, as its rest pose:
    /// the other handedness (a back view of a front-view rig). A root's
    /// offset is in the artwork's frame (x flips); a child's is in its
    /// parent's frame, along and across the parent (across flips).
    fn reflect_rig(rig: &buzz_rig::Armature) -> buzz_rig::Armature {
        let mut a = rig.at_rest();
        a.root.x = -a.root.x;
        for bone in &mut a.bones {
            if bone.parent.is_some() {
                bone.angle = -bone.angle;
                bone.offset.y = -bone.offset.y;
            } else {
                bone.angle = std::f64::consts::PI - bone.angle;
                bone.offset.x = -bone.offset.x;
            }
        }
        a.set_rest_here();
        for i in 0..a.bones.len() {
            for (p, q) in [(rig.head(i), a.head(i)), (rig.tip(i), a.tip(i))] {
                assert!(
                    (p.x + q.x).abs() < 1e-6 && (p.y - q.y).abs() < 1e-6,
                    "bone {i} did not reflect: {p:?} against {q:?}"
                );
            }
        }
        a
    }

    /// **The view is read off the legs**: the puppet (one hip joint for both
    /// thighs) is a side view either way round; Raju's rest pose (thighs a
    /// fifth of a leg apart) is a front view either way round.
    #[test]
    fn the_puppet_is_a_side_view_and_a_cast_rig_a_front_view() {
        let puppet = canonical_rest();
        assert_eq!(rig_view(puppet), RigView::Side);
        assert_eq!(rig_view(&reflect_rig(puppet)), RigView::Side);
        assert_eq!(rig_view(&cast_like()), RigView::Front);
        assert_eq!(rig_view(&reflect_rig(&cast_like())), RigView::Front);
        // Too few bones to have legs: a side view, as the table always was.
        assert_eq!(rig_view(&buzz_rig::Armature::default()), RigView::Side);
    }

    /// **A front view's feet never cross.** Over a whole cycle of Walk and of
    /// Run, on Raju's rest pose and on its reflection (both handednesses),
    /// each knee and each foot stays on its own side of the midline, at
    /// least half as far out as it stands at rest — the left foot on the
    /// left thigh's side, the right on the right's. And the legs, the hips
    /// and the body do move, so this is a walk and not a statue.
    #[test]
    fn a_front_views_feet_stay_on_their_own_side_of_the_midline() {
        for (name, rig) in [("front", cast_like()), ("reflected", reflect_rig(&cast_like()))] {
            let rest = rig.pose();
            let pelvis = rig.head(Joint::Hips.index());
            let up = rig.tip(Joint::Head.index()) - pelvis;
            let up = up / up.hypot();
            let across = Vec2::new(-up.y, up.x);
            let lateral = |p: Point| (p - pelvis).dot(across);
            for action in [Action::Walk, Action::Run] {
                let mut moved = [0.0f64; 3];
                let mut lifted = 0.0f64;
                for step in 0..=400 {
                    let phase = step as f64 / 400.0;
                    let beat = pose_on(action, phase, 1.0, &rig);
                    let mut posed = rig.clone();
                    let angles: Vec<f64> = rest
                        .iter()
                        .zip(beat.joints.iter().chain(std::iter::repeat(&0.0)))
                        .map(|(r, d)| r + d)
                        .collect();
                    posed.set_pose(&angles);
                    for (thigh, knee) in [(Joint::ThighL, Joint::KneeL), (Joint::ThighR, Joint::KneeR)] {
                        let side = lateral(rig.head(thigh.index()));
                        for (what, at_rest, now) in [
                            ("knee", rig.tip(thigh.index()), posed.tip(thigh.index())),
                            ("foot", rig.tip(knee.index()), posed.tip(knee.index())),
                        ] {
                            let (r, n) = (lateral(at_rest), lateral(now));
                            assert!(
                                n * side > 0.0 && n.abs() >= 0.5 * r.abs(),
                                "{name} {action:?} phase {phase}: the {thigh:?} {what} is at {n} across \
                                 the midline (at rest {r}, its hip on the {side} side)"
                            );
                        }
                    }
                    moved[0] = moved[0].max(beat.joints[Joint::ThighL.index()].abs());
                    moved[1] = moved[1].max(beat.joints[Joint::ThighR.index()].abs());
                    moved[2] = moved[2].max(beat.joints[Joint::Hips.index()].abs());
                    lifted = lifted.max(beat.lift);
                }
                assert!(
                    moved.iter().all(|m| *m > 0.02) && lifted > 0.005,
                    "{name} {action:?}: thighs and hips turned {moved:?}, body lifted {lifted}"
                );
            }
        }
    }

    /// **A front-view walk lifts each leg in turn**: the left thigh turns
    /// only in the first half of the cycle, the right only in the second,
    /// and the body rises twice — at both passing positions — and is down
    /// at both contacts.
    #[test]
    fn a_front_view_lifts_one_leg_at_a_time_and_rises_twice() {
        let rig = cast_like();
        let at = |u: f64| pose_on(Action::Walk, u, 1.0, &rig);
        for u in [0.1, 0.25, 0.4] {
            let b = at(u);
            assert!(b.joints[Joint::ThighL.index()].abs() > 1e-3, "left leg still at {u}");
            assert!(b.joints[Joint::ThighR.index()].abs() < 1e-12, "right leg moved at {u}");
        }
        for u in [0.6, 0.75, 0.9] {
            let b = at(u);
            assert!(b.joints[Joint::ThighR.index()].abs() > 1e-3, "right leg still at {u}");
            assert!(b.joints[Joint::ThighL.index()].abs() < 1e-12, "left leg moved at {u}");
        }
        assert!(at(0.25).lift > 0.0 && at(0.75).lift > 0.0);
        assert!(at(0.0).lift.abs() < 1e-12 && at(0.5).lift.abs() < 1e-9);
        // A cycle ends where it began.
        assert_eq!(at(0.0).joints, at(1.0).joints);
    }
}
